//! Specialized native factories, not aliases for the Vector3/Matrix/Dynel factories.
//! Projectile2013: GC100d145c ->100ec41a; load100ec307/init100ec363,
//! process100ec273/graceful100ec2ec/delete100ec218+100ec559.
//! GroundImpact5000: GC100cf872 ->100e103e; load100e0f3b/init100e0fe1,
//! process100e0ec0/graceful100a719a/delete100e0f8f+100e12db.
//! Its visual is ForceSword, DS init10015730/process100156e9/render10015f21.
use super::Template;
use anyhow::{ensure, Context, Result};
use ao_formats::{character::{CrtRand, NameTable}, mesh::{decode_mesh_into, MESH_TYPE}};
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Scene, Vertex};
use glam::{Mat4, Quat, Vec3};
use std::sync::Arc;

pub(super) struct ProjectileEffect {
    pub resource: (u32, Arc<Scene>),
    start: Vec3,
    delta: Vec3,
    distance: f32,
    speed: f32,
    elapsed: f32,
    ticks: u32,
    duration: f32,
    position: Vec3,
    rotation: Quat,
    done: bool,
}
impl ProjectileEffect {
    pub fn new(t: &Template, hit: Option<(Vec3, Vec3)>, store: &RecordStore, names: &NameTable, resources: &mut std::collections::HashMap<u32, Arc<Scene>>) -> Result<Self> {
        ensure!(t.kind == 2013, "invalid projectile template");
        let selector = t.word(8)? as i32;
        let selector = if (0..=2).contains(&selector) { selector } else { 0 };
        // The third native pointer102c4c28 is not a resource name (10171bd8).
        // No authored record uses it: do not substitute a different arrow.
        ensure!(selector != 2, "native projectile selector2 has no named resource");
        let name = if selector == 0 { "arrow_long.abiff" } else { "arrow_short.abiff" };
        let id = names.id(MESH_TYPE, name).with_context(|| format!("missing native projectile {name}"))?;
        let scene = match resources.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                // GC100ec363 GetSync may return null; retain the control, not fake art.
                let mut scene = Scene::default();
                if let Some(mesh) = decode_mesh_into(store,id,&mut scene)? {
                    scene.instances.push(ao_scene::Instance {mesh,transform:Mat4::IDENTITY.to_cols_array_2d()});
                }
                Arc::clone(entry.insert(Arc::new(scene)))
            },
        };
        let resource = (id,scene);
        let (start, end) = hit.unwrap_or((Vec3::ZERO, Vec3::X));
        let delta = end - start;
        let distance = delta.length().max(1.0);
        let delta = if delta.length_squared() == 0.0 { Vec3::X } else { delta };
        let direction = delta.normalize();
        Ok(Self { resource, start, delta, distance, speed: t.float(9)?, elapsed: 0.0, ticks:0, duration:-1.0,
            position: start, rotation: Quat::from_rotation_arc(Vec3::NEG_Z, direction), done: hit.is_none() })
    }
    pub fn advance(&mut self, dt: f32) -> bool {
        // GC100d2531: first Process resets time; only second Process caps delta.
        self.elapsed += match self.ticks { 0 => 0.0, 1 => dt.min(0.033), _ => dt };
        self.ticks = self.ticks.saturating_add(1);
        if self.duration > 0.0 && self.elapsed > self.duration { self.done = true; }
        // GC100ec273 only moves/expires by distance when the cloned mesh exists.
        if !self.done && !self.resource.1.meshes.is_empty() {
            (self.position, self.done) = projectile_position(self.start, self.delta, self.distance, self.speed, self.elapsed);
        }
        !self.done
    }
    pub fn terminate_gracefully(&mut self) { self.done = true; }
    pub fn set_duration(&mut self, duration: f32) { self.duration = duration; }
    pub fn actor(&self, id: u32, model: u64) -> ActorFrame {
        ActorFrame { id, model, transform: Mat4::from_rotation_translation(self.rotation, self.position).to_cols_array_2d(),
            alpha: if self.resource.1.meshes.is_empty() {0.0} else {1.0}, ..Default::default() }
    }
}

fn projectile_position(start: Vec3, delta: Vec3, distance: f32, speed: f32, elapsed: f32) -> (Vec3, bool) {
    let fraction = speed * elapsed / distance;
    (start + delta * fraction.min(1.0), fraction > 1.0)
}

/// Caller supplies a real resource connector's world matrix and validity/visibility.
/// Native loader reads four authored floats but discards all four (100e0f3b).
/// Geometry and random walk below are DS10015730/10015f21, not those floats.
pub(super) struct GroundImpactEffect {
    count: usize,
    vertices: Vec<Vertex>,
    indices: Vec<usize>,
    size: f32,
    visible: bool,
    done: bool,
    pub terminating: bool,
}
impl GroundImpactEffect {
    pub const MATERIAL: u32 = 1;
    pub fn new(t: &Template, connector: Option<Mat4>, rng: &mut CrtRand) -> Result<Self> {
        ensure!(t.kind == 5000, "invalid ground impact template");
        for i in 0..4 { t.float(i)?; }
        let count = rng.rand() as usize % 10 + 5;
        let mut vertices = vec![Vertex::default(); count * 9];
        let color = [1.0, (155.0f32/255.0).powf(2.2), (50.0f32/255.0).powf(2.2), 0.0];
        for vertex in &mut vertices { vertex.color = color; vertex.uv = [0.5;2]; }
        for section in 0..count {
            for corner in 0..4 { vertices[section*4+corner].uv = [(corner%2) as f32, 0.65]; }
            if section + 1 < count { for corner in 0..4 { vertices[section*4+corner].color[3] = 1.0; } }
            // InitMesh consumes these two rand() draws, even though Render replaces the core.
            rng.rand(); rng.rand();
        }
        let mut indices = Vec::with_capacity((count-1)*36);
        for section in 0..count-1 {
            let a = section*4;
            indices.extend_from_slice(&[a,a+1,a+4,a+1,a+5,a+4,a+2,a+3,a+6,a+3,a+7,a+6]);
        }
        for trail in 0..4 {
            for section in 0..count-1 {
                let a = (4+trail)*count+section;
                indices.extend_from_slice(&[a,a+count,a+1,a+count,a+count+1,a+1]);
            }
        }
        Ok(Self { count, vertices, indices, size: 1.0, visible: connector.is_some(), done: connector.is_none(), terminating: false })
    }
    pub fn advance(&mut self, connector: Option<Mat4>, visible: bool) -> bool {
        if connector.is_none() { self.done = true; }
        self.visible = visible;
        !self.done
    }
    pub fn terminate_gracefully(&mut self) { self.terminating = true; }
    pub fn set_size(&mut self, size: f32) { self.size = size; }
    pub fn set_color(&mut self, rgba: [f32;4]) {
        // DS1001564b changes RGB only; alpha remains the per-render random envelope.
        for vertex in &mut self.vertices { for (channel, value) in rgba[..3].iter().enumerate() {
            vertex.color[channel] = (((value*255.0).trunc() as i32 as u8) as f32/255.0).powf(2.2);
        } }
    }
    pub fn render(&mut self, connector: Mat4, rng: &mut CrtRand, out: &mut Vec<Vertex>) {
        if self.done || !self.visible { return; }
        let step = Vec3::new(0.0, 0.0, self.size/(self.count-1) as f32);
        let mut center = Vec3::ZERO;
        let mut rise = 0.0f32;
        let mut fall = 2.0f32;
        let increment = 2.0/self.count as f32;
        let offsets = [Vec3::new(-0.05,0.0,0.0), Vec3::new(0.05,0.0,0.0), Vec3::new(0.0,-0.05,0.0), Vec3::new(0.0,0.05,0.0)];
        for section in 0..self.count {
            let noise = Vec3::new((rng.rand()%2000) as f32/1000.0-1.0,(rng.rand()%2000) as f32/1000.0-1.0,0.0)*2.0*rise*fall;
            let mut average = Vec3::ZERO;
            let mut max_alpha = 0u8;
            for (corner, offset) in offsets.iter().enumerate() {
                let vertex = &mut self.vertices[section*4+corner];
                let position = connector.transform_point3(center+noise+*offset);
                vertex.pos = position.to_array();
                average += position;
                if section != 0 && section+1 < self.count {
                    let alpha = (rng.rand()%200+55) as u8;
                    vertex.color[3] = alpha as f32/255.0;
                    max_alpha = max_alpha.max(alpha);
                }
            }
            if section != 0 && section+1 < self.count { for trail in 0..5 {
                self.vertices[(4+trail)*self.count+section].color[3] = (max_alpha >> (trail+1)) as f32/255.0;
            } }
            average *= 0.25;
            for trail in (0..5).rev() {
                let previous = if trail == 0 { Vec3::ZERO } else { Vec3::from_array(self.vertices[(3+trail)*self.count+section].pos) };
                self.vertices[(4+trail)*self.count+section].pos = if previous == Vec3::ZERO { average } else { previous }.to_array();
            }
            center += step; rise += increment; fall -= increment;
        }
        out.extend(self.indices.iter().map(|&index| self.vertices[index]));
    }
    pub fn scene(&self, store: &RecordStore, names: &NameTable) -> Result<Scene> {
        use ao_scene::{Blend, Mesh, Submesh, TextureKey};
        let key = TextureKey { rdb_type:1010004, id:names.id(1010004,super::materials::MATERIALS[Self::MATERIAL as usize].0).context("missing ForceSword circle.png")? };
        let mut scene = Scene::default();
        scene.textures.insert(key,ao_formats::texture::load_texture(store,key)?.context("missing ForceSword texture")?);
        let mut submesh = Submesh::new((0..self.indices.len() as u32).collect(),Some(key));
        submesh.blend = Blend::Additive;
        submesh.two_sided = true;
        submesh.emissive = [1.0;3];
        scene.meshes.push(Mesh {vertices:vec![Vertex::default();self.indices.len()],submeshes:vec![submesh]});
        Ok(scene)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_ground_impact_90000_uses_native_generated_mesh() {
        let t = Template {kind:5000,words:vec![0x3f800000,0,0,0x41200000]};
        let mut rng = CrtRand::new(1);
        let mut effect = GroundImpactEffect::new(&t,Some(Mat4::IDENTITY),&mut rng).unwrap();
        assert!((5..=14).contains(&effect.count));
        assert_eq!(effect.vertices.len(),effect.count*9);
        assert_eq!(effect.indices.len(),(effect.count-1)*36);
        let mut vertices = Vec::new();
        effect.render(Mat4::IDENTITY,&mut rng,&mut vertices);
        assert_eq!(vertices.len(),effect.indices.len());
        assert!(vertices.iter().all(|v| v.pos.iter().all(|x|x.is_finite())));
        let previous = effect.vertices[4*effect.count+1].pos;
        effect.render(Mat4::from_translation(Vec3::X),&mut rng,&mut vertices);
        assert_eq!(effect.vertices[5*effect.count+1].pos,previous);
        effect.terminate_gracefully();
        assert!(effect.advance(Some(Mat4::IDENTITY),true));
        assert!(!effect.advance(None,true));
    }
    #[test]
    fn authored_special_factory_payloads() {
        // Installed gfxtweak2693 short arrow /2694 long arrow, both speed7.
        for selector in [1,0] {
            let t = Template {kind:2013,words:vec![1,0,0,0,0,0,0,1000,selector,0x40e00000]};
            assert_eq!(t.word(8).unwrap(),selector);
            assert_eq!(t.float(9).unwrap(),7.0);
            let (position, done) = projectile_position(Vec3::ZERO,Vec3::X*7.0,7.0,t.float(9).unwrap(),1.0);
            assert_eq!(position,Vec3::X*7.0);
            assert!(!done, "native expires strictly after fraction1");
            assert!(projectile_position(Vec3::ZERO,Vec3::X*7.0,7.0,t.float(9).unwrap(),1.001).1);
        }
        let mut rng=CrtRand::new(1);
        let t=Template {kind:5000,words:vec![0x3f800000,0,0]};
        assert!(GroundImpactEffect::new(&t,Some(Mat4::IDENTITY),&mut rng).is_ok());
    }
    #[test]
    #[ignore = "requires installed retail assets"]
    fn installed_2693_missing_arrow_retains_stationary_control() -> Result<()> {
        let dir=ao_gui::client_dir();let store=RecordStore::open(&dir)?;
        let names=NameTable::load(&store)?;let templates=super::super::Templates::open(&dir)?;
        assert_eq!(names.id(MESH_TYPE,"arrow_short.abiff"),Some(27728));
        let mut resources=std::collections::HashMap::new();
        let mut effect=ProjectileEffect::new(&templates.by_id[&2693],Some((Vec3::ZERO,Vec3::X*7.0)),&store,&names,&mut resources)?;
        assert!(effect.resource.1.meshes.is_empty());
        for _ in 0..3 {assert!(effect.advance(10.0));}
        assert_eq!(effect.position,Vec3::ZERO);assert_eq!(effect.actor(1,1).alpha,0.0);
        effect.set_duration(1.0);assert!(!effect.advance(0.0));
        let mut malformed=templates.by_id[&2693].clone();malformed.words[9]=f32::NAN.to_bits();
        assert!(ProjectileEffect::new(&malformed,Some((Vec3::ZERO,Vec3::X)),&store,&names,&mut resources).is_err());
        let mut renderer=super::super::Renderer::open(&dir)?;
        let handle=renderer.spawn_configured(super::super::Binding {group:0,attractor:0,effect:2693,note:0,color:0},
            Mat4::IDENTITY,Vec3::X*7.0,super::super::EffectConfig {
                creation:super::super::Creation::HitLocation,hit_location:Some((Vec3::ZERO,Vec3::X*7.0)),..Default::default()})?;
        assert!(handle!=0);assert!(!renderer.mesh_uploaded.contains_key(&27728));
        let mut host=ao_render::Host::headless();renderer.frame(0.1,&mut host,None);
        assert!(host.actors.is_empty());assert!(host.actor_models.is_empty());
        assert!(renderer.active.iter().any(|a|a.actor==handle));
        Ok(())
    }
    #[test]
    #[ignore = "requires installed retail assets and offscreen renderer; AOMAC_EFFECT_FRAMES selects output"]
    fn authored_special_effect_frames() -> Result<()> {
        let dir = ao_gui::client_dir();
        let store = RecordStore::open(&dir)?;
        let names = NameTable::load(&store)?;
        let templates = super::super::Templates::open(&dir)?;
        let out = std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/tmp/FxClasses/special-frames".into());
        std::fs::create_dir_all(&out)?;
        let mut resources = std::collections::HashMap::new();
        for id in [2693,2694] {
            let mut effect = ProjectileEffect::new(&templates.by_id[&id],Some((Vec3::ZERO,Vec3::X*7.0)),&store,&names,&mut resources)?;
            assert_eq!(effect.resource.1.meshes.is_empty(),id==2693,"exact installed arrow hole");
            let models = if id==2693 {Vec::new()} else {vec![(1,(*effect.resource.1).clone())]};
            let mut previous = 0.0;
            for time in [0.01,0.1,0.5,1.0] {
                assert!(effect.advance(time-previous));
                previous = time;
                let actors = if id==2693 {Vec::new()} else {vec![effect.actor(1,1)]};
                assert_eq!(actors.is_empty(),id==2693);
                ao_render::render_to_png_actors(&Scene::default(),&models,actors,[3.5,1.5,5.0],[3.5,0.0,0.0],640,480,&out.join(format!("{id}_{time:.2}.png")),time)?;
            }
            if id==2693 {
                assert!(effect.advance(10.0),"missing clone retains native base lifetime");
                assert_eq!(effect.position,Vec3::ZERO);
                assert_eq!(effect.actor(1,1).alpha,0.0);
                effect.terminate_gracefully();
                assert!(!effect.advance(0.0));
            } else {assert!(!effect.advance(0.1));}
        }
        let mut rng = CrtRand::new(1);
        let mut effect = GroundImpactEffect::new(&templates.by_id[&90000],Some(Mat4::IDENTITY),&mut rng)?;
        let scene = effect.scene(&store,&names)?;
        let models = vec![(1,scene)];
        for frame in 0..8 {
            let connector = Mat4::from_rotation_translation(Quat::from_rotation_y(frame as f32*0.15),Vec3::X*frame as f32*0.1);
            assert!(effect.advance(Some(connector),true));
            let mut vertices = Vec::new();
            effect.render(connector,&mut rng,&mut vertices);
            assert_eq!(vertices.len(),effect.indices.len());
            let actor = ActorFrame {id:1,model:1,skin:Some(vertices),alpha:1.0,always:true,rendering_effect:5,priority:Some(6),..Default::default()};
            ao_render::render_to_png_actors(&Scene::default(),&models,vec![actor],[1.5,1.0,3.0],[0.0,0.0,0.5],640,480,&out.join(format!("90000_{frame:02}.png")),frame as f32/60.0)?;
        }
        assert!(!effect.advance(None,true));
        Ok(())
    }
}
