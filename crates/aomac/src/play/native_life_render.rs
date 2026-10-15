//! Wildlife VisualMesh instances (Gamecode 0x1011a3c9), using the world actor renderer.
use ao_formats::mesh::{self, NodeRig};
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Scene};
use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;

const MODEL_BASE: u64 = 0x4c49_4645_0000_0000;
const ACTOR_BASE: u32 = 0x7000_0000;

struct Model {
    scene: std::sync::Arc<Scene>,
    rig: Option<NodeRig>,
    uploaded: bool,
    boundaries: std::sync::Arc<[f32]>,
    height: f32,
}

/// Meshes are shared by catalog mesh id; poses and actor identities belong to instances.
#[derive(Default)]
pub struct LifeModels {
    models: HashMap<u32, Model>,
    instances: HashMap<u32, ActorFrame>,
    generation: u64,
    visibility: Vec<bool>,
}

impl LifeModels {
    pub fn load(&mut self, store: &RecordStore, mesh: u32) -> anyhow::Result<()> {
        if self.models.contains_key(&mesh) { return Ok(()); }
        let bytes = store.get(ao_formats::wildlife::MESH_TYPE, mesh)?
            .ok_or_else(|| anyhow::anyhow!("missing wildlife mesh {mesh}"))?;
        let archive = ao_formats::archive::Archive::parse(&bytes)?;
        let boundaries = archive.animation_ranges.iter().map(|(ms, _)| (*ms as f32 as f64 * 0.001) as f32).collect();
        // GetMeshHeight (DS 1006bcfb) is twice the largest geometry-frame sphere
        // radius, not the vertex Y extent (BVolume::Archive, Randy 10029f2d).
        let height = archive.objects.iter().filter_map(|node| {
            let data = archive.objects.get(node.ref1("data")?)?;
            let bound = archive.objects.get(data.ref1("bvol")?)?;
            bound.f32s::<1>("sph_radius").map(|radius| radius[0])
        }).fold(0.0_f32, f32::max) * 2.0;
        let (scene, rig) = match mesh::load_animated_mesh(store, mesh)? {
            Some((scene, rig)) => (scene, Some(rig)),
            None => (mesh::load_mesh(store, mesh)?, None),
        };
        self.models.insert(mesh, Model { scene: std::sync::Arc::new(scene), rig, uploaded: false, boundaries, height });
        Ok(())
    }
    pub fn upload(&mut self, host: &mut ao_render::Host) {
        if self.generation != host.scene_generation() {
            self.generation = host.scene_generation();
            for model in self.models.values_mut() { model.uploaded = false; }
        }
        for (&mesh, model) in &mut self.models {
            if !model.uploaded {
                host.actor_models.push((MODEL_BASE | u64::from(mesh), model.scene.clone()));
                model.uploaded = true;
            }
        }
    }

    /// Native simulation space is left-handed; reflect both position and quaternion into scene space.
    pub fn update(&mut self, id: u32, mesh: u32, pos: Vec3, rotation: Quat, time: f32, visible: bool) {
        if !visible { self.instances.remove(&id); return; }
        let Some(model) = self.models.get_mut(&mesh) else { return; };
        let frame = self.instances.entry(id).or_insert_with(|| ActorFrame {
            id: ACTOR_BASE | id,
            model: MODEL_BASE | u64::from(mesh),
            ..Default::default()
        });
        frame.model = MODEL_BASE | u64::from(mesh);
        frame.transform = Mat4::from_rotation_translation(
            Quat::from_xyzw(-rotation.x, -rotation.y, rotation.z, rotation.w),
            Vec3::new(pos.x, pos.y, -pos.z),
        ).to_cols_array_2d();
        if let Some(rig) = &mut model.rig {
            rig.pose_parts(time, &mut frame.parts);
            rig.pose_visuals(time, &mut frame.part_uvs, &mut self.visibility);
            frame.part_priorities.clear();
            frame.part_priorities.extend(self.visibility.iter().map(|&visible| if visible { None } else { Some(-1) }));
        } else {
            frame.parts.clear();
            frame.part_uvs.clear();
            frame.part_priorities.clear();
        }
    }

    /// Native stable ids remain in ascending creation order after removals.
    pub fn retain(&mut self, ids: &[u32]) {
        self.instances.retain(|id, _| ids.binary_search(id).is_ok());
    }

    /// EnableLife=false destroys the native life objects, rather than merely hiding them.
    pub fn clear(&mut self) { self.instances.clear(); }

    pub fn frames(&self) -> impl Iterator<Item = &ActorFrame> { self.instances.values() }

    pub fn total_time(&self, mesh: u32) -> Option<f32> {
        self.models.get(&mesh).map(|model| model.rig.as_ref().map_or(0.0, NodeRig::total_time))
    }

    pub fn boundaries(&self, mesh: u32) -> std::sync::Arc<[f32]> {
        self.models[&mesh].boundaries.clone()
    }

    pub fn height(&self, mesh: u32) -> Option<f32> {
        self.models.get(&mesh).map(|model| model.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_wildlife_meshes_animate_and_disable_destroys_instances() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = RecordStore::open(&dir).unwrap();
        let catalog = ao_formats::wildlife::load_catalog(&store).unwrap();
        let mut models = LifeModels::default();
        let mut animated = 0;
        for (index, asset) in catalog.iter().enumerate() {
            models.load(&store, asset.mesh).unwrap();
            let id = index as u32;
            let total = models.total_time(asset.mesh).unwrap();
            models.update(id, asset.mesh, Vec3::new(2.0, 3.0, 4.0), Quat::IDENTITY, 0.0, true);
            let first = models.instances[&id].parts.clone();
            models.update(id, asset.mesh, Vec3::new(2.0, 3.0, 4.0), Quat::IDENTITY, total * 0.37, true);
            let frame = &models.instances[&id];
            assert_eq!(frame.transform[3], [2.0, 3.0, -4.0, 1.0]);
            if frame.parts != first { animated += 1; }
            models.update(id, asset.mesh, Vec3::ZERO, Quat::IDENTITY, 0.0, false);
            assert!(!models.instances.contains_key(&id));
            models.update(id, asset.mesh, Vec3::ZERO, Quat::IDENTITY, 0.0, true);
            models.retain(&[]);
            assert!(!models.instances.contains_key(&id));
        }
        assert!(animated > 0, "native catalog must contain authored moving meshes");
        models.update(0, catalog[0].mesh, Vec3::ZERO, Quat::IDENTITY, 0.0, true);
        models.clear();
        assert_eq!(models.frames().count(), 0);
    }
}
