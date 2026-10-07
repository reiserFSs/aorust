//! Class 3025 is GfxControlEffectMesh_t, not a sprite (GC 100ce4f7 -> 1010d314).
//! GC 1010c5a6 loads its parameters; 1010cf05 selects an ABIFF VisualMesh;
//! 1010c94a advances translation, axis-angle, scale and the opacity envelope.
use super::{EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::NameTable, mesh::{decode_mesh_into, MESH_TYPE}};
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Scene};
use glam::{Mat4, Quat, Vec3};
use std::{collections::HashMap, sync::Arc};

// GC 1010cf05, InstanceManager_t::GetTypeInstance(1010001, name).
const RESOURCES: [&str; 28] = [
    "EP03_shoulder_rocket.abiff", "EP03_mech_heal_effect.abiff",
    "EP03_blast_wave_effect.abiff", "EP03_bullet_casing.abiff", "EP03_EMP_blast.abiff",
    "EP03_bomber_debris_01.abiff", "EP03_bomber_debris_02.abiff", "EP03_bomber_debris_03.abiff",
    "EP03_RUBIKA_asteroidbig.abiff", "EP03_RUBIKA_asteroidbig2.abiff",
    "EP03_preorder_mech_statel.abiff", "EP03_scout_mech_statel.abiff", "EP03_heavy_mech_statel.abiff",
    "EP03_anti_personnel_gun_statel.abiff", "EP03_anti_vehicle_gun_statel.abiff",
    "EP03_preorder_mech_upgraded_statel.abiff", "EP03_scout_mech_upgraded_statel.abiff",
    "EP03_heavy_mech_upgraded_statel.abiff", "EP03_anti_personnel_gun_upgraded_statel.abiff",
    "EP03_anti_vehicle_gun_upgraded_statel.abiff", "hoverbike_a_effect_circle.abiff",
    "hoverbike_b_effect_circle.abiff", "hoverboard_a_fan.abiff", "hoverboard_b_fan.abiff",
    "xan_grid_dependencybox.abiff", "penumbra_shoulderpads_atrox.abiff", "hoverboard_c_fx01.abiff",
    "hoverbike_b_effect_circle_red.abiff",
];

fn envelope(points: &[(f32, f32)], time: f32) -> f32 {
    // GC 1011634c: out-of-range samples return the LAST value, not the first.
    for pair in points.windows(2) {
        let [(start, a), (end, b)] = pair else { unreachable!() };
        if *start <= time && time < *end {
            let fraction = (time-start)/(end-start);
            return a*(1.0-fraction)+b*fraction;
        }
    }
    points.last().map_or(1.0, |p| p.1)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MeshContext {
    pub camera: Vec3,
    pub first_person: bool,
    pub body_scale: f32,
    pub breed: i32,
    pub vehicle_speed: Option<f32>,
    pub vehicle_direction: i32,
    pub visible: bool,
    pub source_alive: bool,
    pub ground_height: Option<f32>,
}

pub(super) struct TracerMesh {
    template: Template,
    scenes: Vec<(u32, Arc<Scene>)>,
    source: Mat4,
    offset: Vec3,
    velocity: Vec3,
    acceleration: Vec3,
    axis: Vec3,
    angle: f32,
    angular_velocity: f32,
    angular_acceleration: f32,
    scale: f32,
    scale_velocity: f32,
    scale_acceleration: f32,
    envelope: Vec<(f32, f32)>,
    duration: f32,
    elapsed: f32,
    age: f32,
    config_scale: f32,
    context: Option<MeshContext>,
    vehicle_alpha: f32,
    track_source: bool,
    rig: Option<ao_formats::mesh::NodeRig>,
    visible: bool,
    record_id: Option<u32>,
}

impl TracerMesh {
    pub(super) fn new(template: &Template, store: &RecordStore, names: &NameTable, source: Mat4, resources: &mut HashMap<u32, Arc<Scene>>, config: EffectConfig) -> Result<Self> {
        ensure!(template.kind == 3025, "not a native effect-mesh template");
        let rendering_effect = template.word(31)?;
        ensure!(rendering_effect <= 7, "invalid native mesh rendering effect");
        let selector = template.word(10)?;
        let atrox = template.word(0)? & 0x10000 != 0 && config.source_appearance.is_some_and(|a| a[0] == 4);
        let name = if atrox {
            match selector {
                20 => "hoverbike_a_effect_circle_atrox.abiff",
                21 => "hoverbike_b_effect_circle_atrox.abiff",
                27 => "hoverbike_b_effect_circle_red_atrox.abiff",
                _ => RESOURCES.get(selector as usize).context("invalid effect-mesh selector")?,
            }
        } else { RESOURCES.get(selector as usize).context("invalid effect-mesh selector")? };
        let id = names.id(MESH_TYPE, name);
        // Separate the effect-mesh selector namespace from the rock-list selectors.
        let animated = template.word(0)? & 0x1000 != 0;
        let key = 0x3025_0000 | selector | (u32::from(atrox) << 5) | (u32::from(animated) << 6) | (rendering_effect << 8);
        let (mut animated_scene, rig) = if let Some(id)=id.filter(|_|animated) {
            match ao_formats::mesh::load_animated_mesh(store, id)? {
                Some((scene, rig)) => (Some(scene), Some(rig)),
                None => (None, None),
            }
        } else { (None, None) };
        let scenes = if let Some(id)=id {let scene = match resources.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut scene = match animated_scene.take() {
                    Some(scene) => scene,
                    None => {
                        // GC1010cf05 retains VisualMesh while AsyncMesh has no payload.
                        let mut scene=Scene::default();
                        if let Some(mesh)=decode_mesh_into(store,id,&mut scene)? {
                            scene.instances.push(ao_scene::Instance {mesh,transform:Mat4::IDENTITY.to_cols_array_2d()});
                        }
                        scene
                    },
                };
                if rendering_effect == 1 {
                    for id in [0x28022, 0x28023] {
                        let key = ao_scene::TextureKey { rdb_type: 1010004, id };
                        let texture = ao_formats::texture::load_texture(store, key)?.with_context(|| format!("missing native hologram texture {id}"))?;
                        scene.textures.insert(key, texture);
                    }
                }
                Arc::clone(entry.insert(Arc::new(scene)))
            }
        };vec![(key,scene)]} else {Vec::new()};
        let count = template.word(32)? as usize;
        ensure!(count <= template.words.len().saturating_sub(33)/2, "short effect-mesh opacity envelope");
        let points = (0..count).map(|i| Ok((template.float(33+i*2)?, template.float(34+i*2)?))).collect::<Result<Vec<_>>>()?;
        let vector = |at| -> Result<Vec3> { Ok(Vec3::new(template.float(at)?, template.float(at+1)?, -template.float(at+2)?)) };
        let axis = vector(22)? * Vec3::new(-1.0,-1.0,-1.0);
        let source = super::sprites::connector(template, source)?;
        Ok(Self {
            template: template.clone(), scenes, source,
            offset: vector(13)?, velocity: source.transform_vector3(vector(16)?), acceleration: source.transform_vector3(vector(19)?), axis,
            angle: template.float(25)?, angular_velocity: template.float(26)?, angular_acceleration: template.float(27)?,
            scale: template.float(28)?, scale_velocity: template.float(29)?, scale_acceleration: template.float(30)?,
            envelope: points, duration: config.duration.unwrap_or(template.float(8)?), elapsed: 0.0,
            age: 0.0,
            config_scale: config.scale.unwrap_or(1.0),
            context: None,
            vehicle_alpha: if template.word(0)? & 0x6000 != 0 { 0.0 } else { 1.0 },
            track_source: config.track_source,
            rig,
            visible: true,
            record_id: id,
        })
    }
    pub(super) fn scenes(&self) -> &[(u32, Arc<Scene>)] { &self.scenes }
    pub(super) fn record_id(&self) -> Option<u32> { self.record_id }
    pub(super) fn is_animated(&self) -> bool { self.rig.is_some() }
    pub(super) fn connector_transform(&self, frame: usize) -> Option<Mat4> {
        self.rig.as_ref()?.frame_transform(frame).map(|m| Mat4::from_cols_array_2d(&m))
    }
    pub(super) fn set_context(&mut self, context: MeshContext) { self.context = Some(context); }
    pub(super) fn actor_count(&self) -> usize { self.scenes.len() }
    pub(super) fn set_anchors(&mut self, source: Mat4, _target: Vec3) -> Result<()> {
        ensure!(source.is_finite(), "invalid effect-mesh source transform");
        self.source = super::sprites::connector(&self.template, source)?;
        Ok(())
    }
    pub(super) fn frame(&mut self, dt: f32) -> Result<bool> {
        ensure!(dt.is_finite() && dt >= 0.0, "invalid effect-mesh timestep");
        // GC1010cf05: failed GetTypeInstance marks done+0x14 without allocating VisualMesh.
        if self.record_id.is_none() {return Ok(false);}
        self.age += dt;
        if self.duration > 0.0 && self.age > self.duration { return Ok(false); }
        let flags = self.template.word(0)?;
        ensure!(flags & !7 == 0 || self.context.is_some(), "effect-mesh missing native source/camera context");
        if let Some(context) = self.context {
            if self.track_source && !context.source_alive { return Ok(false); }
            if self.track_source { self.visible = context.visible; }
            if flags & 0x6000 == 0 && !self.visible { return Ok(true); }
            ensure!(flags & 0x100 == 0 || context.ground_height.is_some(), "effect-mesh missing native ground height");
            self.elapsed += dt;
            if flags & 0x2000 != 0 && context.vehicle_direction >= 1 {
                if let Some(speed) = context.vehicle_speed {
                    // GC1010cb98/1010cbb8, double10155f00 = 0.5.
                    if speed > 0.0 && self.vehicle_alpha < 1.0 { self.visible = true; }
                    if speed <= 0.0 && self.vehicle_alpha < 0.0 { self.visible = false; }
                    self.vehicle_alpha = if speed > 0.0 {
                        if self.vehicle_alpha >= 1.0 { 1.0 } else { self.vehicle_alpha + dt * 0.5 }
                    } else if self.vehicle_alpha <= 0.0 { 0.0 } else { self.vehicle_alpha - dt * 0.5 };
                }
            }
        }
        if self.context.is_none() { self.elapsed += dt; }
        if !self.visible { return Ok(true); }
        if self.template.word(9)? == 1 {
            // GC1010cb4e: sin(base elapsed / duration * 2π) * authored velocity.y * dt.
            self.offset.y += (self.age / self.duration * std::f32::consts::TAU).sin() * self.velocity.y * dt;
        } else {
            self.offset += self.velocity*dt;
            self.velocity += self.acceleration*dt;
        }
        self.angle += self.angular_velocity*dt;
        self.angular_velocity += self.angular_acceleration*dt;
        self.scale += self.scale_velocity*dt;
        self.scale_velocity += self.scale_acceleration*dt;
        Ok(true)
    }
    pub(super) fn actors(&mut self, actor_base: u32, model_base: u64) -> Vec<ActorFrame> {
        if self.record_id.is_none() {return Vec::new();}
        let (_, source_rotation, position) = self.source.to_scale_rotation_translation();
        let (sine, cosine) = (self.angle*0.5).sin_cos();
        let mut rotation = source_rotation * Quat::from_xyzw(self.axis.x*sine, self.axis.y*sine, self.axis.z*sine, cosine);
        let flags = self.template.word(0).unwrap_or(0);
        let mut point = position;
        if flags & 0x100 != 0 {
            if let Some(height) = self.context.and_then(|context| context.ground_height) { point.y = height; }
        }
        point += self.offset;
        let mut scale = self.scale*self.config_scale;
        if let Some(context) = self.context {
            if flags & 0x400 != 0 && !context.first_person && context.camera != point {
                rotation = Quat::from_mat4(&Mat4::look_at_rh(point, context.camera, Vec3::Y).inverse());
            }
            if flags & 0x200 != 0 { scale *= context.body_scale; }
            // GC1010c5a6 double1016e080 = 1.45.
            if flags & 0x8000 != 0 && context.breed == 4 { scale *= 1.45; }
        }
        let transform = Mat4::from_scale_rotation_translation(Vec3::splat(scale), rotation, point);
        let alpha = if self.elapsed > 0.0 && self.duration > 0.0 { envelope(&self.envelope, self.elapsed/self.duration) } else { 1.0 };
        let mut alpha = alpha * self.vehicle_alpha;
        if let Some(context) = self.context {
            if flags & 0x800 != 0 {
                let distance = context.camera.distance(point);
                // GC1010ce44: constants 5,10; (distance-5)/10, not /5.
                if context.first_person || distance < 5.0 { alpha = 0.0; }
                else if distance < 10.0 { alpha = (distance - 5.0) / 10.0 * self.vehicle_alpha; }
            }
        }
        if !self.visible { alpha = 0.0; }
        let mut parts = Vec::new();
        let mut part_uvs = Vec::new();
        let mut part_visibility = Vec::new();
        if let Some(rig) = &mut self.rig {
            let total = rig.total_time();
            let time = if total > 0.0 && self.elapsed > total { self.elapsed % total } else { self.elapsed };
            rig.pose_parts(time, &mut parts);
            rig.pose_visuals(time, &mut part_uvs, &mut part_visibility);
        }
        let part_priorities = part_visibility.iter().map(|&visible| if visible { None } else { Some(-1) }).collect();
        vec![ActorFrame { id: actor_base, model: model_base | u64::from(self.scenes[0].0), transform: transform.to_cols_array_2d(), parts, part_uvs, part_priorities, skin: None, always: false, alpha, rendering_effect: self.template.word(31).unwrap_or(0), rendering_effect_time: self.age, ..Default::default() }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_mesh_selection_and_envelope() {
        assert_eq!(RESOURCES[0], "EP03_shoulder_rocket.abiff");
        assert_eq!(envelope(&[(0.0, 1.0),(1.0,0.0)],0.5),0.5);
        assert_eq!(envelope(&[(0.0, 1.0),(1.0,0.0)],-1.0),0.0);
        assert_eq!(envelope(&[],0.5),1.0);
    }
    #[test]
    fn native_oscillation_and_vehicle_camera_gain() {
        // Actual gfxtweak.bin71123 (mech-heal): all41 authored words.
        let words = vec![3,0,0,0,0x437b53d1,0x43cc341a,0,0,0x41000000,1,1,
            0x3e4ccccd,0x3e4ccccd,0,0,0,0,0x3fc00000,0,0,0,0,0,0x3f800000,0,
            0,0x3f800000,0,0x40000000,0,0,4,4,0,0,0x3e4ccccd,0x3f19999a,
            0x3f4ccccd,0x3f19999a,0x3f800000,0];
        let mut effect = TracerMesh {
            template: Template { kind: 3025, words }, scenes: vec![(0, Arc::new(Scene::default()))],
            source: Mat4::IDENTITY, offset: Vec3::ZERO, velocity: Vec3::Y * 1.5,
            acceleration: Vec3::ONE, axis: Vec3::Y, angle: 0.0, angular_velocity: 0.0,
            angular_acceleration: 0.0, scale: 1.0, scale_velocity: 0.0,
            scale_acceleration: 0.0, envelope: Vec::new(), duration: 8.0, elapsed: 0.0,
            age: 0.0,
            config_scale: 1.0, context: None, vehicle_alpha: 1.0, track_source: false,
            rig: None,
            visible: true,
            record_id: Some(0),
        };
        assert!(effect.frame(2.0).unwrap());
        assert_eq!(effect.offset, Vec3::Y * 3.0);
        assert_eq!(effect.velocity, Vec3::Y * 1.5);
        effect.template.words[9] = 0;
        effect.velocity = Vec3::ZERO;
        effect.acceleration = Vec3::ZERO;
        effect.template.words[0] = 0x2800;
        effect.set_context(MeshContext { camera: Vec3::new(0.0,3.0,7.5),
            first_person: false, body_scale: 1.0, breed: 1, vehicle_speed: Some(1.0),
            vehicle_direction: 1, visible: true, source_alive: true, ground_height: None });
        effect.vehicle_alpha = 0.0;
        assert!(effect.frame(0.5).unwrap());
        assert_eq!(effect.vehicle_alpha, 0.25);
        assert!((effect.actors(1, 0)[0].alpha - 0.0625).abs() < 1e-6);
        // A resolved identity with pending payload keeps its allocated control.
        assert!(effect.scenes()[0].1.meshes.is_empty());
        assert!(effect.frame(0.0).unwrap());
        effect.record_id=None;
        effect.scenes.clear();
        assert!(!effect.frame(0.0).unwrap());
        assert!(effect.actors(1,0).is_empty());
    }
    #[test]
    #[ignore = "requires installed retail assets"]
    fn installed_71123_missing_heal_terminates_control() -> Result<()> {
        let dir=ao_gui::client_dir();let store=RecordStore::open(&dir)?;
        let names=NameTable::load(&store)?;let templates=super::super::Templates::open(&dir)?;
        let t=&templates.by_id[&71123];assert_eq!(t.kind,3025);assert_eq!(t.word(10)?,1);
        assert!(names.id(MESH_TYPE,RESOURCES[1]).is_none(),"regression requires the proven installed name hole");
        let mut resources=HashMap::new();
        let mut effect=TracerMesh::new(t,&store,&names,Mat4::IDENTITY,&mut resources,EffectConfig::default())?;
        assert!(effect.record_id().is_none());assert!(effect.scenes().is_empty());
        assert!(!effect.frame(1.0/60.0)?,"failed native identity lookup terminates control");
        assert!(effect.actors(1,0).is_empty());
        assert!(resources.is_empty(),"failed lookup allocates no visual resource");
        let mut malformed=t.clone();malformed.words[28]=f32::NAN.to_bits();
        assert!(TracerMesh::new(&malformed,&store,&names,Mat4::IDENTITY,&mut resources,EffectConfig::default()).is_err());
        Ok(())
    }
    #[test]
    #[ignore = "retail assets and offscreen Metal rendering"]
    fn authored_native_tracer_mesh_frames() {
        let dir = ao_gui::client_dir();
        let store = RecordStore::open(&dir).unwrap();
        let names = NameTable::load(&store).unwrap();
        let templates = super::super::Templates::open(&dir).unwrap();
        let template = &templates.by_id[&71520];
        assert_eq!((template.kind, template.words.len()), (3025, 37));
        let mut resources = HashMap::new();
        let mut effect = TracerMesh::new(template, &store, &names, Mat4::IDENTITY, &mut resources, EffectConfig::default()).unwrap();
        assert!(effect.scenes()[0].1.meshes.iter().any(|mesh| !mesh.vertices.is_empty()));
        let models: Vec<_> = effect.scenes().iter().map(|(key, scene)| (u64::from(*key), scene.as_ref().clone())).collect();
        let out = std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp/FxRest/frames".into());
        std::fs::create_dir_all(&out).unwrap();
        for frame in 1..=60 {
            assert!(effect.frame(1.0/60.0).unwrap());
            if [1,30,60].contains(&frame) {
                ao_render::render_to_png_actors(&Scene::default(), &models, effect.actors(1000, 0),
                    [6.0,5.0,9.0], [0.0,0.0,0.0], 640,480,
                    &out.join(format!("tracer_mesh_71520_{frame}.png")), frame as f32/60.0).unwrap();
            }
        }
        assert!(!effect.frame(10.0).unwrap());
    }
    #[test]
    #[ignore = "all authored3025 retail assets and offscreen Metal rendering"]
    fn all_authored_mesh_modes_frames() {
        let dir = ao_gui::client_dir();
        let store = RecordStore::open(&dir).unwrap();
        let names = NameTable::load(&store).unwrap();
        let templates = super::super::Templates::open(&dir).unwrap();
        let out = std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").expect("AOMAC_EFFECT_FRAMES output directory"));
        std::fs::create_dir_all(&out).unwrap();
        let mut resources = HashMap::new();
        let mut ids: Vec<_> = templates.by_id.iter().filter(|(_, t)| t.kind == 3025).map(|(&id, _)| id).collect();
        ids.sort_unstable();
        assert!(!ids.is_empty());
        for id in ids {
            let t = &templates.by_id[&id];
            let mut effect = TracerMesh::new(t, &store, &names, Mat4::IDENTITY, &mut resources,
                EffectConfig { source_appearance: Some([4,1,0,100]), ..Default::default() }).unwrap();
            effect.set_context(MeshContext { camera: Vec3::new(6.0,5.0,9.0), first_person: false,
                body_scale: 1.0, breed: 4, vehicle_speed: Some(1.0), vehicle_direction: 1,
                visible: true, source_alive: true, ground_height: Some(0.0) });
            let missing_name=effect.record_id().is_none();
            assert_eq!(missing_name,id==71123,"exact installed3025 missing name");
            assert_eq!(effect.scenes().is_empty(),missing_name);
            let models: Vec<_> = effect.scenes().iter().filter(|(_,scene)|!scene.meshes.is_empty()).map(|(key, scene)| (u64::from(*key), scene.as_ref().clone())).collect();
            for frame in 1..=30 {
                let alive=effect.frame(1.0/60.0).unwrap();
                if missing_name {assert!(!alive,"failed native lookup terminates control");}
                if !alive {break;}
                if [1,15,30].contains(&frame) {
                    let actors=effect.actors(1000,0).into_iter().filter(|actor|models.iter().any(|(model,_)|*model==actor.model)).collect();
                    ao_render::render_to_png_actors(&Scene::default(), &models, actors,
                        [6.0,5.0,9.0], [0.0,0.0,0.0], 640,480,
                        &out.join(format!("mesh3025_{id}_{frame}.png")), frame as f32/60.0).unwrap();
                }
            }
        }
    }
}
