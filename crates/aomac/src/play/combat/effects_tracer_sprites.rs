//! Class 3025 is GfxControlEffectMesh_t, not a sprite (GC 100ce4f7 -> 1010d314).
//! GC 1010c5a6 loads its parameters; 1010cf05 selects an ABIFF VisualMesh;
//! 1010c94a advances translation, axis-angle, scale and the opacity envelope.
use super::{EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::NameTable, mesh::{load_mesh, MESH_TYPE}};
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
    config_scale: f32,
}

impl TracerMesh {
    pub(super) fn new(template: &Template, store: &RecordStore, names: &NameTable, source: Mat4, resources: &mut HashMap<u32, Arc<Scene>>, config: EffectConfig) -> Result<Self> {
        ensure!(template.kind == 3025, "not a native effect-mesh template");
        ensure!(template.word(0)? & !7 == 0 && template.word(9)? == 0 && template.word(31)? == 0,
            "effect-mesh vehicle, oscillation, camera and rendering-effect variants require their native controllers");
        let name = RESOURCES.get(template.word(10)? as usize).context("invalid effect-mesh selector")?;
        let id = names.id(MESH_TYPE, name).with_context(|| format!("missing native effect mesh {name}"))?;
        // Separate the effect-mesh selector namespace from the rock-list selectors.
        let key = 0x3025_0000 | template.word(10)?;
        let scene = match resources.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::hash_map::Entry::Vacant(entry) => Arc::clone(entry.insert(Arc::new(load_mesh(store, id).with_context(|| format!("effect mesh {name} ({id})"))?))),
        };
        let count = template.word(32)? as usize;
        ensure!(count <= template.words.len().saturating_sub(33)/2, "short effect-mesh opacity envelope");
        let points = (0..count).map(|i| Ok((template.float(33+i*2)?, template.float(34+i*2)?))).collect::<Result<Vec<_>>>()?;
        let vector = |at| -> Result<Vec3> { Ok(Vec3::new(template.float(at)?, template.float(at+1)?, -template.float(at+2)?)) };
        let axis = vector(22)? * Vec3::new(-1.0,-1.0,-1.0);
        Ok(Self {
            template: template.clone(), scenes: vec![(key, scene)], source: super::sprites::connector(template, source)?,
            offset: vector(13)?, velocity: vector(16)?, acceleration: vector(19)?, axis,
            angle: template.float(25)?, angular_velocity: template.float(26)?, angular_acceleration: template.float(27)?,
            scale: template.float(28)?, scale_velocity: template.float(29)?, scale_acceleration: template.float(30)?,
            envelope: points, duration: config.duration.unwrap_or(template.float(8)?), elapsed: 0.0,
            config_scale: config.scale.unwrap_or(1.0),
        })
    }
    pub(super) fn scenes(&self) -> &[(u32, Arc<Scene>)] { &self.scenes }
    pub(super) fn actor_count(&self) -> usize { 1 }
    pub(super) fn set_anchors(&mut self, source: Mat4, _target: Vec3) -> Result<()> {
        ensure!(source.is_finite(), "invalid effect-mesh source transform");
        self.source = super::sprites::connector(&self.template, source)?;
        Ok(())
    }
    pub(super) fn frame(&mut self, dt: f32) -> Result<bool> {
        ensure!(dt.is_finite() && dt >= 0.0, "invalid effect-mesh timestep");
        self.elapsed += dt;
        if self.duration > 0.0 && self.elapsed > self.duration { return Ok(false); }
        self.offset += self.velocity*dt;
        self.velocity += self.acceleration*dt;
        self.angle += self.angular_velocity*dt;
        self.angular_velocity += self.angular_acceleration*dt;
        self.scale += self.scale_velocity*dt;
        self.scale_velocity += self.scale_acceleration*dt;
        Ok(true)
    }
    pub(super) fn actors(&self, actor_base: u32, model_base: u64) -> Vec<ActorFrame> {
        let (_, source_rotation, position) = self.source.to_scale_rotation_translation();
        let (sine, cosine) = (self.angle*0.5).sin_cos();
        let rotation = source_rotation * Quat::from_xyzw(self.axis.x*sine, self.axis.y*sine, self.axis.z*sine, cosine);
        let transform = Mat4::from_scale_rotation_translation(Vec3::splat(self.scale*self.config_scale), rotation, position+self.offset);
        let alpha = if self.elapsed > 0.0 && self.duration > 0.0 { envelope(&self.envelope, self.elapsed/self.duration) } else { 1.0 };
        vec![ActorFrame { id: actor_base, model: model_base | u64::from(self.scenes[0].0), transform: transform.to_cols_array_2d(), parts: Vec::new(), skin: None, always: false, alpha }]
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
}
