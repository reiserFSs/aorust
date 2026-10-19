//! Dynamic actors (players, NPCs, ...) drawn after the world: the model's index buffers, materials and textures are uploaded once
//! per model key ([`Renderer::add_actor_model`]); the body mesh gets one vertex buffer per actor, rewritten when the app supplies a
//! new pose ([`ao_scene::ActorFrame::skin_pose`] for native CAT compute, [`ao_scene::ActorFrame::skin`] for CPU-authored effects);
//! rigid parts (head, weapons) share the model's vertex buffer and only get a transform.

use super::*;
use ao_scene::ActorFrame;
use std::sync::{Arc, Weak};

#[path = "skinning.rs"]
mod skinning;
#[path = "character_smoothing.rs"]
mod character_smoothing;
#[path = "smoothing_gpu.rs"]
mod smoothing_gpu;
#[path = "shadow_actors.rs"]
mod shadow_actors;
#[path = "motion_vectors.rs"]
pub(crate) mod motion_vectors;
// Enhanced v2 actor motion (G2): nested here to reuse the private actor layer; v2/mod.rs drives it.
#[path = "v2/motion.rs"]
pub(crate) mod v2_motion;
#[cfg(test)]
#[path = "native_skin_tests.rs"]
mod native_skin_tests;

/// Native archive identity plus every input that changes the uploaded bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TextureIdentity {
    key: TextureKey,
    source: u8,
    width: u32,
    height: u32,
    quality: i32,
    compressed: bool,
    better: bool,
    content: u64,
    materials_revision: u64,
}

impl TextureIdentity {
    fn new(key: TextureKey, texture: &ao_scene::Texture, policy: native_texture::Policy) -> Self {
        let mut identity = Self { key, source: texture.source as u8, width: texture.width, height: texture.height,
            quality: policy.quality, compressed: policy.compressed, better: policy.better, content: 0, materials_revision: 0 };
        if identity.immutable() {
            // RecordStore may load 1010016/17 under a canonical 1010004 key.
            // DisplayOtherTextureQuality is independent of the GPU upload policy.
            use std::hash::{Hash, Hasher};
            let mut hash = std::hash::DefaultHasher::new();
            texture.rgba.hash(&mut hash);
            identity.content = hash.finish();
        }
        identity
    }
    fn immutable(self) -> bool {
        matches!(self.key.rdb_type, 1_010_004 | 1_010_006 | 1_010_009 | 1_010_011 | 1_010_021 | 1_010_022)
            && self.source != ao_scene::TextureSource::Generated as u8
    }
}

struct ActorTexture {
    view: wgpu::TextureView,
    bytes: u64,
    material: Arc<MaterialSource>,
}

/// `(mat, pipe, env (mat, pipe))`: see `ModelMesh::submesh_mats`.
type SubmeshMat = (usize, usize, Option<(usize, usize)>);

/// Extra radius around the bind-pose sphere of mesh 0: limbs reach out of it in animation. Exported so app-side
/// pre-skin culling (e.g. aomac's dynels CPU-skin skip, perf audit item 5) can mirror this margin.
pub const POSE_MARGIN: f32 = 1.0;

/// Frustum-vs-sphere visibility test shared between the renderer's own per-mesh instancing cull below and any
/// app-side pre-skin check: a sphere is culled when it fails any plane (`center`/`radius` in the same space as
/// `planes`). Exported so an app-side test can use identical math to this crate's own final cull.
pub fn frustum_culls_sphere(planes: &[Vec4], center: Vec3, radius: f32) -> bool {
    !planes.iter().all(|p| p.dot(center.extend(1.0)) >= -radius)
}

/// Affine transform (including the existing alpha lane), then independently enabled material overrides.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ActorInstance {
    transform: [[f32; 4]; 4],
    emissive: [f32; 4],
    specular: [f32; 4],
    power: [f32; 4],
    uv: [f32; 4],
    character: u32,
}

struct ModelMesh {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    /// Current GPU buffer sizes in bytes (next power of two of the largest `nverts`/index count seen since the
    /// last full rebuild): `update_actor_model`'s fast path writes into the existing buffer in place and only
    /// recreates it when a frame's geometry no longer fits (mirrors `native_liquid.rs`'s `vertex_capacity`).
    vertex_capacity: u64,
    index_capacity: u64,
    nverts: usize,
    draws: Vec<Draw>,
    /// `(mat, pipe, env (mat, pipe))` per submesh, in `scene.meshes[i].submeshes` order: the fast path reuses these
    /// bind-group/pipeline indices as-is (it only runs once `same_material` has confirmed every submesh's material
    /// inputs are still the ones these indices were built for) and only recomputes `draws`' triangle ranges.
    submesh_mats: Vec<SubmeshMat>,
    /// Bind-pose bounding sphere.
    sphere: (Vec3, f32),
    bounds: (Vec3, Vec3),
    smoothing: Option<smoothing_gpu::MeshGpu>,
}

struct Model {
    /// Geometry/material incarnation, including in-place updates and reused model keys.
    point_cache_revision: u64,
    meshes: Vec<Option<ModelMesh>>,
    mats: Vec<wgpu::BindGroup>,
    material_bindings: Vec<MaterialBinding>,
    holo: Option<wgpu::BindGroup>,
    views: Vec<wgpu::TextureView>,
    view_of: HashMap<TextureKey, usize>,
    textures: Vec<Arc<ActorTexture>>,
    identities: HashMap<TextureKey, TextureIdentity>,
    topology: Vec<(usize, Vec<ao_scene::Submesh>)>,
    /// Attribute/count buffers and the minimum palette length for all actively read bone indices.
    native_skin: Option<(wgpu::Buffer, wgpu::Buffer, u64)>,
}

struct ActorGpu {
    /// Own body vertex buffer, created at the first pose.
    vb: Option<wgpu::Buffer>,
    seen: u64,
    /// Model key of the last submitted frame (a replaced model only invalidates its own actors' buffers).
    model: u64,
    skin: Option<std::sync::Arc<Vec<ao_scene::Vertex>>>,
    skin_bounds: Option<(Vec3, Vec3)>,
    pose: Option<skinning::PoseGpu>,
    smoothing: Option<smoothing_gpu::ActorGpu>,
}

/// One visible (actor, model mesh) with its slot in the instance buffer.
struct Item {
    actor: u32,
    model: u64,
    mesh: usize,
    /// Nearest final-camera depth of the current body bound.
    fog_depth: f32,
    bucket: i32,
    /// `ActorFrame::alpha < 1`: opaque / alpha-tested draws use the [`FADE_PIPE`] variants.
    faded: bool,
    rendering_effect: u32,
    priority: Option<i32>,
    native_shadow_source: Option<u32>,
    native_shadow_receiver: Option<(Vec3, f32)>,
    /// Captured only from the current LOS-admitted frame, never retained actor state.
    water_body: Option<[f32; 3]>,
    swimming: bool,
    water_surface: Option<f32>,
    smoothing: bool,
}

fn bounds_depth((center, half): (Vec3, Vec3), transform: Mat4, eye: Vec3, forward: Vec3) -> f32 {
    let support = Vec3::new(forward.dot(transform.x_axis.truncate()),
        forward.dot(transform.y_axis.truncate()), forward.dot(transform.z_axis.truncate())).abs();
    forward.dot(transform.transform_point3(center) - eye) - support.dot(half)
}

fn actor_visual_key(item: &Item, draw: &Draw) -> Option<VisualKey> {
    draw.node.map(|(node, _)| VisualKey::Actor(item.actor, item.model, item.mesh, node))
}

#[derive(Default)]
pub(crate) struct ActorLayer {
    models: HashMap<u64, Model>,
    /// Never reset by model retirement or zone clears: a reused key gets a fresh incarnation.
    next_point_cache_revision: u64,
    /// Weak lookup only: the last owning model reclaims the GPU allocation.
    textures: HashMap<TextureIdentity, Weak<ActorTexture>>,
    texture_uploads: u64,
    actors: HashMap<u32, ActorGpu>,
    frames: Vec<ActorFrame>,
    enhanced_effects: HashMap<u32, (spell_smoke::Simulation, bool)>,
    effect_walkers: HashMap<Option<u32>, Vec3>,
    effect_time: Option<f32>,
    tick: u64,
    bufs: Vec<wgpu::Buffer>,
    cap: usize,
    items: Vec<Item>,
    motion_instances: Vec<ActorInstance>,
    commands: Vec<(i32, Option<usize>, i32, bool, usize)>,
    /// Evicted actors' skin vertex buffers, by model: every actor of a model shares its vertex count/byte size (see
    /// `set_actors`'s `s.len() == body.nverts` filter), so a freed buffer is safe to hand straight to the next new
    /// actor of that model (short-lived combat/footstep/lightning effect actors otherwise create and drop one GPU
    /// buffer per spawn, which under real in-world churn -- 60+ animating dynels, each firing footstep/swish/impact
    /// effects a few times a second -- creates thousands of buffers a second and floods IOAccelerator (confirmed live
    /// at ICC: 18k+ outstanding Metal buffers, 623 MB resident, within ~13s).
    vb_pool: HashMap<u64, Vec<wgpu::Buffer>>,
    /// Actor skin vertex buffers actually created (not reused from `vb_pool`), since the layer started: a regression
    /// counter, not drained. In steady state with a stable set of models this must stop climbing (every further skin
    /// write reuses a pooled or already-owned buffer).
    pub(crate) vb_created: u64,
    /// `update_actor_model` calls that fell through to a full `upload_actor_model` rebuild (new GPU buffers and
    /// bind groups for the whole model), since the layer started: a regression counter, not drained. In steady
    /// state with stable materials and a capacity that already fits the largest frame seen, this must stop
    /// climbing even while vertex/index *counts* keep varying frame to frame.
    pub(crate) model_rebuilds: u64,
    /// `ModelMesh` vertex/index GPU buffers actually (re)created by `ensure_capacity` growing past the previous
    /// capacity, since the layer started: a regression counter, not drained. Converges once the actor's largest
    /// frame has been seen; a model whose geometry size never exceeds its first frame never grows at all.
    pub(crate) mesh_buffer_grows: u64,
    skin_pipeline: Option<(crate::pipeline_creation::ComputePipeline, wgpu::BindGroupLayout)>,
    smoothing_pipeline: Option<(crate::pipeline_creation::ComputePipeline, wgpu::BindGroupLayout)>,
    pub(super) native_metal: bool,
}

impl ActorLayer {
    fn enhanced_body_admitted(&self, item: &Item, fog_end: f32) -> bool {
        if item.mesh != 0 || item.faded { return false; }
        // Match shade()'s native fog exemptions; these bodies can remain visible.
        if matches!(item.rendering_effect, 2 | 4..=6) { return true; }
        let fogged = item.fog_depth >= fog_end;
        !fogged || self.models[&item.model].topology[item.mesh].1.iter().any(|part| !part.fog_enabled)
    }

    pub(super) fn enhanced_grass_actor_ids(&self, fog_end: f32) -> impl Iterator<Item = u32> + '_ {
        self.items.iter().filter(move |item| self.enhanced_body_admitted(item, fog_end)).map(|item| item.actor)
    }

    pub(super) fn interaction_slot(&self, actor: Option<u32>) -> u32 {
        actor.map_or(0, |id| self.items.iter().position(|item| item.actor == id)
            .map_or(u32::MAX, |index| index as u32 + 1))
    }

    pub(super) fn interaction_buffer_bytes(&self) -> u64 {
        ((self.items.len().max(64).next_power_of_two() + 1) * 4) as u64
    }

    pub(super) fn reset_interaction_effects(&mut self) { self.enhanced_effects.clear(); }

    pub(super) fn enhanced_water_contacts(&self, fog_end: f32, out: &mut Vec<(u32, [f32; 3], f32, bool)>) {
        out.clear();
        out.extend(self.items.iter().filter(|item| self.enhanced_body_admitted(item, fog_end))
            .filter_map(|item| Some((item.actor, item.water_body?, item.water_surface?, item.swimming))));
    }

    pub(super) fn enhanced_water_body_positions(&self, fog_end: f32, out: &mut Vec<(u32, [f32; 3])>) {
        out.clear();
        out.extend(self.items.iter().filter(|item| self.enhanced_body_admitted(item, fog_end))
            .filter_map(|item| item.water_body.map(|position| (item.actor, position))));
    }
    fn remove_model(&mut self, key: u64) {
        self.models.remove(&key);
        self.textures.retain(|_, texture| texture.strong_count() != 0);
        self.actors.retain(|_, actor| actor.model != key);
        self.frames.retain(|frame| frame.model != key);
        self.items.retain(|item| item.model != key);
        self.vb_pool.remove(&key);
        // Instance buffers are shared scratch space, rebuilt from frames before each draw.
    }
}

impl Renderer {
    /// Unique retained actor texture allocations and estimated mip bytes (not total device VRAM).
    pub fn actor_texture_resources(&self) -> (usize, u64) {
        let mut seen = std::collections::HashSet::new();
        let mut bytes = 0;
        for texture in self.act.models.values().flat_map(|model| &model.textures) {
            if seen.insert(Arc::as_ptr(texture)) { bytes += texture.bytes; }
        }
        (seen.len(), bytes)
    }

    /// Actual actor texture uploads, including generated textures and model placeholders.
    pub fn actor_texture_uploads(&self) -> u64 { self.act.texture_uploads }

    /// Retained actor models, actors, pooled skin buffers, and remembered native visual priorities.
    pub fn actor_cache_sizes(&self) -> (usize, usize, usize, usize) {
        (self.act.models.len(), self.act.actors.len(),
            self.act.vb_pool.values().map(Vec::len).sum(), self.drawn_visuals.len())
    }

    fn actor_texture(&mut self, identity: TextureIdentity, texture: &ao_scene::Texture) -> Arc<ActorTexture> {
        if identity.immutable() {
            if let Some(shared) = self.act.textures.get(&identity).and_then(Weak::upgrade) { return shared; }
        }
        let bc = self.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let built = native_texture::build_texture_mips(self.texture_policy, texture.source, bc,
            &texture.rgba, texture.width, texture.height, false);
        let shared = Arc::new(ActorTexture {
            view: self.upload_built_texture(&built),
            material: self.material_source_build(identity.key, texture),
            bytes: built.levels.iter().map(|level| level.bytes.len() as u64).sum(),
        });
        self.act.texture_uploads += 1;
        if identity.immutable() { self.act.textures.insert(identity, Arc::downgrade(&shared)); }
        shared
    }

    pub(super) fn actor_material_sources(&self) -> Vec<Arc<MaterialSource>> {
        self.act.models.values().flat_map(|model| model.textures.iter().map(|texture| texture.material.clone())).collect()
    }

    pub(super) fn rebind_actor_materials(&mut self) {
        let groups: Vec<_> = self.act.models.iter().map(|(&key, model)|
            (key, model.material_bindings.iter().map(|binding| self.material_bind_group(binding)).collect::<Vec<_>>())).collect();
        for (key, groups) in groups {
            self.act.models.get_mut(&key).unwrap().mats = groups;
        }
    }

    /// Uploads a model for [`ActorFrame::model`] `key`: `scene.meshes[0]` is the (skinned) body, the others are rigid parts.
    /// Replaces a model of the same key.
    ///
    /// # Panics
    /// Panics if native skin attributes do not match mesh 0's vertex count.
    pub fn add_actor_model(&mut self, key: u64, scene: &Scene) {
        self.retire_motion_model(key);
        self.upload_actor_model(key, scene, false);
    }

    /// Replaces dynamic geometry while retaining the model's already uploaded textures and bind groups.
    /// Any newly referenced texture must be supplied in `scene.textures`.
    ///
    /// Fast path: as long as every submesh's material inputs are unchanged ([`same_material`]) and every
    /// referenced texture is already uploaded, this writes the new vertex/index data into the existing GPU
    /// buffers in place (growing them, capacity-doubled, only when a frame needs more room than before) and
    /// recomputes the draw ranges from the cached bind-group indices -- no buffer or bind group is created for
    /// dynamic buff/particle/effect geometry whose vertex or index *count* merely varies frame to frame. A
    /// changed material (a different texture, blend, tint, ...) still falls through to a full rebuild, which is
    /// the only place bind groups are created.
    ///
    /// # Panics
    /// Panics if native skin attributes do not match mesh 0's vertex count.
    pub fn update_actor_model(&mut self, key: u64, scene: &Scene) {
        self.retire_motion_model(key);
        let eligible = self.act.models.get(&key).is_some_and(|model| {
            scene.native_skin.is_none() && model.native_skin.is_none()
                && scene.textures.iter().all(|(k, texture)| {
                    let mut identity = TextureIdentity::new(*k, texture, self.texture_policy);
                    identity.materials_revision = self.materials_revision;
                    model.identities.get(k) == Some(&identity)
                })
                && model.topology.len() == scene.meshes.len()
                && model.meshes.len() == scene.meshes.len()
                && model.topology.iter().zip(&scene.meshes).zip(&model.meshes).all(|(((_, subs), mesh), gpu)| {
                    subs.len() == mesh.submeshes.len()
                        && subs.iter().zip(&mesh.submeshes).all(|(a, b)| same_material(a, b))
                        && gpu.is_some() == (!mesh.vertices.is_empty() && mesh.submeshes.iter().map(|s| s.indices.len()).sum::<usize>() > 0)
                })
        });
        if eligible {
            self.act.next_point_cache_revision = self.act.next_point_cache_revision.checked_add(1)
                .expect("actor model revision exhausted");
            let device = &self.device;
            let queue = &self.queue;
            let model = self.act.models.get_mut(&key).unwrap();
            model.point_cache_revision = self.act.next_point_cache_revision;
            let grows = &mut self.act.mesh_buffer_grows;
            for (i, mesh) in scene.meshes.iter().enumerate() {
                let Some(gpu) = model.meshes[i].as_mut() else {
                    model.topology[i] = (mesh.vertices.len(), submesh_shape(&mesh.submeshes));
                    continue;
                };
                let vbytes = (mesh.vertices.len() * 48) as u64;
                if ensure_capacity(device, &mut gpu.vb, &mut gpu.vertex_capacity, vbytes, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, "actor body vertices") {
                    *grows += 1;
                }
                if vbytes > 0 {
                    queue.write_buffer(&gpu.vb, 0, bytemuck::cast_slice(&mesh.vertices));
                }

                let mut indices = Vec::with_capacity(mesh.submeshes.iter().map(|s| s.indices.len()).sum());
                let mut draws = Vec::with_capacity(gpu.submesh_mats.len());
                let nv = mesh.vertices.len() as u32;
                for (s, &(mat, pipe, env)) in mesh.submeshes.iter().zip(&gpu.submesh_mats) {
                    let first = indices.len() as u32;
                    for t in s.indices.as_chunks::<3>().0 {
                        if t.iter().all(|&idx| idx < nv) {
                            indices.extend_from_slice(t.as_slice());
                        }
                    }
                    let count = indices.len() as u32 - first;
                    if count == 0 {
                        continue;
                    }
                    draws.push(Draw { mesh: i, first_index: first, count, mat, pipe,
                        priority: s.render_priority, node: s.render_node, origin: s.render_origin, projection_pass: s.projection_pass,
                        vegetation: false, time_alpha: false });
                    if let Some((emat, epipe)) = env {
                        draws.push(Draw { mesh: i, first_index: first, count, mat: emat, pipe: epipe,
                            priority: s.render_priority, node: s.render_node, origin: s.render_origin, projection_pass: s.projection_pass,
                            vegetation: false, time_alpha: false });
                    }
                }
                let ibytes = (indices.len() * 4) as u64;
                if ensure_capacity(device, &mut gpu.ib, &mut gpu.index_capacity, ibytes, wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, "actor body indices") {
                    *grows += 1;
                }
                if ibytes > 0 {
                    queue.write_buffer(&gpu.ib, 0, bytemuck::cast_slice(&indices));
                }

                gpu.draws = draws;
                gpu.nverts = mesh.vertices.len();
                let (lo, hi) = mesh.vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), v| (l.min(Vec3::from(v.pos)), h.max(Vec3::from(v.pos))));
                let center = (lo + hi) * 0.5;
                gpu.sphere = (center, mesh.vertices.iter().map(|v| (Vec3::from(v.pos) - center).length()).fold(0.0, f32::max));
                gpu.bounds = (center, (hi - lo) * 0.5);
                model.topology[i] = (mesh.vertices.len(), submesh_shape(&mesh.submeshes));
            }
            return;
        }
        self.act.model_rebuilds += 1;
        self.upload_actor_model(key, scene, true);
    }

    fn upload_actor_model(&mut self, key: u64, scene: &Scene, retain_textures: bool) {
        if let Some(skin) = &scene.native_skin {
            assert!(scene.meshes.first().is_some_and(|body| body.vertices.len() == skin.len()),
                "native skin attributes must match the body mesh vertex count");
        }
        let old = if retain_textures { self.act.models.remove(&key) } else { None };
        let (mut views, mut view_of, mut textures, mut identities) = match old {
            Some(model) => (model.views, model.view_of, model.textures, model.identities),
            None => {
                let white = ao_scene::Texture { source: ao_scene::TextureSource::Generated, width: 1, height: 1, rgba: vec![255; 4] };
                let identity = TextureIdentity::new(TextureKey { rdb_type: 0, id: 0 }, &white, self.texture_policy);
                let texture = self.actor_texture(identity, &white);
                (vec![texture.view.clone()], HashMap::new(), vec![texture], HashMap::new())
            }
        };
        for (k, t) in &scene.textures {
            let mut identity = TextureIdentity::new(*k, t, self.texture_policy);
            identity.materials_revision = self.materials_revision;
            if identities.get(k) == Some(&identity) { continue; }
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize {
                let texture = self.actor_texture(identity, t);
                if let Some(&index) = view_of.get(k) {
                    views[index] = texture.view.clone();
                    textures[index] = texture;
                } else {
                    view_of.insert(*k, views.len());
                    views.push(texture.view.clone());
                    textures.push(texture);
                }
                identities.insert(*k, identity);
            }
        }
        let holo_views = [0x28022, 0x28023].map(|id| view_of.iter().find(|(key, _)| key.id == id).map(|(_, &view)| view));
        let holo = match holo_views {
            [Some(a), Some(b)] => Some(self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("native Holo textures"), layout: &self.holo_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[a]) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[b]) },
                ],
            })),
            _ => None,
        };
        let mut mat_of: HashMap<(usize, usize, bool, [u32; 32]), usize> = HashMap::new();
        let mut mats: Vec<wgpu::BindGroup> = vec![];
        let mut material_bindings = Vec::new();
        let mut meshes = vec![];
        for mesh in &scene.meshes {
            let idx_total: usize = mesh.submeshes.iter().map(|s| s.indices.len()).sum();
            if mesh.vertices.is_empty() || idx_total == 0 {
                meshes.push(None);
                continue;
            }
            let vbytes = mesh.vertices.len() * 12;
            let vertex_capacity = ((vbytes * 4) as u64).next_power_of_two().max(1);
            let vb = self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: None, size: vertex_capacity, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC | if scene.native_skin.is_some() { wgpu::BufferUsages::STORAGE } else { wgpu::BufferUsages::empty() }, mapped_at_creation: false,
            });
            self.queue.write_buffer(&vb, 0, bytemuck::cast_slice(&mesh.vertices));
            let (mut indices, mut draws, mut submesh_mats) = (Vec::with_capacity(idx_total), vec![], Vec::with_capacity(mesh.submeshes.len()));
            let nv = mesh.vertices.len() as u32;
            for s in &mesh.submeshes {
                let first = indices.len() as u32;
                for t in s.indices.as_chunks::<3>().0 {
                    if t.iter().all(|&i| i < nv) {
                        indices.extend_from_slice(t.as_slice());
                    }
                }
                let count = indices.len() as u32 - first;
                let view = s.texture.and_then(|k| view_of.get(&k).copied()).unwrap_or(0);
                let env_view = s.stage1_env_texture.and_then(|key| view_of.get(&key).copied()).unwrap_or(0);
                let mut bind = |view: usize, clamp: bool, mut u: [f32; 32]| {
                    u[30] = ((u[30] as u32) | 128) as f32;
                    *mat_of.entry((view, env_view, clamp, u.map(f32::to_bits))).or_insert_with(|| {
                        let ub = self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                            label: None,
                            contents: bytemuck::bytes_of(&u),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });
                        let binding = MaterialBinding { base: views[view].clone(), env: views[env_view].clone(),
                            uniform: ub, source: textures[view].material.clone() };
                        mats.push(self.material_bind_group(&binding));
                        material_bindings.push(binding);
                        mats.len() - 1
                    })
                };
                // bound regardless of this frame's triangle count, so `submesh_mats` stays index-aligned with
                // `mesh.submeshes` for `update_actor_model`'s fast path even if a submesh is briefly empty
                let mat = bind(view, s.texture_clamp, mat_uniform(s));
                let pipe = material_pipe(s);
                // the env layer redraws the same triangles right after them, in the same phase (`FUN_10056ed6`)
                let env = s.env_texture.and_then(|k| view_of.get(&k).copied()).map(|ev| (bind(ev, false, mat_uniform(s)), env_pipe(s)));
                submesh_mats.push((mat, pipe, env));
                if count == 0 {
                    continue;
                }
                draws.push(Draw { mesh: meshes.len(), first_index: first, count, mat, pipe,
                    priority: s.render_priority, node: s.render_node, origin: s.render_origin, projection_pass: s.projection_pass,
                    vegetation: false, time_alpha: false });
                if let Some((emat, epipe)) = env {
                    draws.push(Draw { mesh: meshes.len(), first_index: first, count, mat: emat, pipe: epipe,
                        priority: s.render_priority, node: s.render_node, origin: s.render_origin, projection_pass: s.projection_pass,
                        vegetation: false, time_alpha: false });
                }
            }
            let index_capacity = ((indices.len() * 4) as u64).next_power_of_two().max(1);
            let ib = self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: None, size: index_capacity, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            });
            self.queue.write_buffer(&ib, 0, bytemuck::cast_slice(&indices));
            let (lo, hi) = mesh.vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), v| (l.min(Vec3::from(v.pos)), h.max(Vec3::from(v.pos))));
            let c = (lo + hi) * 0.5;
            let r = mesh.vertices.iter().map(|v| (Vec3::from(v.pos) - c).length()).fold(0.0, f32::max);
            // ponytail: attractor identities arrive with frames, so CAT rigid parts cache topology
            // once too; carry model-time attractor metadata if unused weapon allocations matter.
            let smoothing = scene.native_skin.as_ref().filter(|skin| !skin.is_empty())
                .and_then(|_| smoothing_gpu::MeshGpu::new(&self.device, mesh, &draws));
            meshes.push(Some(ModelMesh { vb, ib, vertex_capacity, index_capacity, nverts: mesh.vertices.len(), draws, submesh_mats, sphere: (c, r), bounds: (c, (hi - lo) * 0.5), smoothing }));
        }
        let topology = scene.meshes.iter().map(|mesh| (mesh.vertices.len(), submesh_shape(&mesh.submeshes))).collect();
        let native_skin = scene.native_skin.as_ref().filter(|s| !s.is_empty()).map(|s| (
            self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: Some("immutable CAT attributes"), contents: bytemuck::cast_slice(s.as_slice()), usage: wgpu::BufferUsages::STORAGE,
            }),
            self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: Some("CAT vertex count"), contents: bytemuck::cast_slice(&[s.len() as u32, 0, 0, 0]), usage: wgpu::BufferUsages::UNIFORM,
            }),
            s.iter().map(|v| {
                let bone = if v.weight >= 0.999 { v.bone0 } else { v.bone0.max(v.bone1) };
                u64::from(bone) + 1
            }).max().unwrap_or(0),
        ));
        self.act.next_point_cache_revision = self.act.next_point_cache_revision.checked_add(1)
            .expect("actor model revision exhausted");
        self.act.models.insert(key, Model { point_cache_revision: self.act.next_point_cache_revision,
            meshes, mats, material_bindings, holo, views, view_of, textures, identities, topology, native_skin });
        if !retain_textures {
            self.drawn_visuals.retain(|visual, _| !matches!(visual, VisualKey::Actor(_, model, _, _) if *model == key));
        }
        self.act.vb_pool.remove(&key);
        self.act.textures.retain(|_, texture| texture.strong_count() != 0);
        // actors of a replaced model rebuild their body buffer at the next pose
        for a in self.act.actors.values_mut().filter(|a| a.model == key) {
            a.vb = None;
            a.skin = None;
            a.pose = None;
            a.smoothing = None;
        }
    }

    /// Retires one model and its actors' skin buffers, submitted frames and draw items.
    /// Other models and actors remain available; missing keys are harmless.
    pub fn remove_actor_model(&mut self, key: u64) {
        self.retire_motion_model(key);
        self.act.remove_model(key);
        self.drawn_visuals.retain(|visual, _| !matches!(visual, VisualKey::Actor(_, model, _, _) if *model == key));
        self.pending_visuals.get_mut().retain(|visual, _| !matches!(visual, VisualKey::Actor(_, model, _, _) if *model == key));
    }

    pub(crate) fn queue_actor_visual_retirements(&mut self, ranges: &mut Vec<(u32, u32)>) {
        // Final ActorFrames can be rendered again by F12/capture. Keep their native
        // priority until the next apply replaces them, never just until render ends.
        // This also bounds logic-only hosts to one frame of deferred retirement.
        let mut retired = std::mem::take(&mut self.actor_visual_retirements);
        self.retire_actor_visuals(&retired);
        retired.clear();
        retired.append(ranges);
        self.actor_visual_retirements = retired;
    }

    /// Native visual destruction, independent of model sharing and temporary non-submission.
    pub fn retire_actor_visuals(&mut self, ranges: &[(u32, u32)]) {
        if ranges.is_empty() { return; }
        self.retire_motion_actor_visuals(ranges);
        let keep = |key: &VisualKey, _: &mut i32| !matches!(key, VisualKey::Actor(id, ..)
            if ranges.iter().any(|&(first, count)| id.wrapping_sub(first) < count));
        self.drawn_visuals.retain(keep);
        self.pending_visuals.get_mut().retain(keep);
    }

    /// Forgets every model and actor (a new zone).
    pub fn clear_actors(&mut self) {
        self.reset_motion_vectors();
        self.drawn_visuals.retain(|key, _| matches!(key, VisualKey::World(..)));
        self.pending_visuals.get_mut().retain(|key, _| matches!(key, VisualKey::World(..)));
        self.actor_visual_retirements.clear();
        self.act.models.clear();
        self.act.textures.clear();
        self.act.actors.clear();
        self.act.vb_pool.clear(); // New model uploads invalidate these model-specific pools anyway.
        self.act.frames.clear();
    }

    /// The actors to draw from now on (until the next call); new body poses are uploaded immediately.
    ///
    /// # Panics
    /// Panics if an active native pose has a nonempty palette too short for its model's bone indices.
    /// Panics if two drawable frames share an actor ID.
    /// A correctly sized CPU skin takes precedence over the palette; an empty palette selects bind pose.
    pub fn set_actors(&mut self, mut frames: Vec<ActorFrame>) {
        // Validate the whole batch before changing actors or leaving any pending GPU work behind.
        for frame in &frames {
            let Some(model) = self.act.models.get(&frame.model) else { continue };
            let Some((_, _, required_bones)) = &model.native_skin else { continue };
            let Some(body) = model.meshes.first().and_then(Option::as_ref) else { continue };
            if frame.skin.as_ref().is_some_and(|skin| skin.len() == body.nverts) { continue; }
            if let Some(palette) = &frame.skin_pose {
                assert!(palette.is_empty() || palette.len() as u64 >= *required_bones,
                    "native skin palette is too short for the model's active bone indices");
            }
        }
        self.clear_enhanced_grass_inputs();
        self.act.tick += 1;
        let tick = self.act.tick;
        for f in &mut frames {
            let Some(body) = self.act.models.get(&f.model).and_then(|m| m.meshes.first()).and_then(Option::as_ref) else { continue };
            let a = match self.act.actors.entry(f.id) {
                std::collections::hash_map::Entry::Occupied(entry) => {
                    assert_ne!(entry.get().seen, tick, "actor IDs must be unique within a frame batch");
                    entry.into_mut()
                }
                std::collections::hash_map::Entry::Vacant(entry) => entry.insert(ActorGpu {
                    vb: None, seen: tick, model: f.model, skin: None, skin_bounds: None, pose: None, smoothing: None,
                }),
            };
            a.seen = tick;
            if a.model != f.model { if let Some(vb) = a.vb.take() { self.act.vb_pool.entry(a.model).or_default().push(vb); } a.skin = None; a.pose = None; a.smoothing = None; }
            a.model = f.model;
            if f.skin.as_ref().is_none_or(|skin| skin.len() != body.nverts) && f.skin_pose.is_some() && self.act.models.get(&f.model).is_some_and(|m| m.native_skin.is_some()) {
                continue;
            }
            if let Some(skin) = f.skin.take().filter(|s| s.len() == body.nverts) {
                f.skin_pose = None;
                if a.skin.as_ref().is_some_and(|previous| std::sync::Arc::ptr_eq(previous, &skin)) { continue; }
                let bytes: &[u8] = bytemuck::cast_slice(skin.as_slice());
                match &a.vb {
                    Some(vb) => self.queue.write_buffer(vb, 0, bytes),
                    None => {
                        let reused = self.act.vb_pool.get_mut(&f.model).and_then(Vec::pop);
                        a.vb = Some(match reused {
                            Some(vb) => { self.queue.write_buffer(&vb, 0, bytes); vb }
                            None => { self.act.vb_created += 1; self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                                label: None,
                                contents: bytes,
                                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::STORAGE,
                            }) }
                        });
                    }
                }
                let (lo, hi) = skin.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), v| (lo.min(Vec3::from(v.pos)), hi.max(Vec3::from(v.pos))));
                a.skin_bounds = Some(((lo + hi) * 0.5, (hi - lo) * 0.5));
                a.skin = Some(skin);
                a.pose = None;
            }
        }
        self.prepare_actor_skins(&frames);
        let (keep, evicted): (HashMap<_, _>, HashMap<_, _>) = std::mem::take(&mut self.act.actors).into_iter().partition(|(_, a)| a.seen == tick);
        self.act.actors = keep;
        self.retain_motion_actors();
        for (_, a) in evicted {
            if let Some(vb) = a.vb { self.act.vb_pool.entry(a.model).or_default().push(vb); }
        }
        self.act.frames = frames;
    }

    /// Actor skin vertex buffers actually created (not reused from the pool) since this `Renderer` started; for
    /// leak regressions: in steady state (a stable actor population) this must stop climbing.
    pub fn actor_vb_creations(&self) -> u64 { self.act.vb_created }

    /// `update_actor_model` calls since the layer started that fell through to a full model rebuild (new GPU
    /// buffers and bind groups), for leak regressions: in steady state with stable materials this must stop
    /// climbing even as vertex/index counts keep varying frame to frame.
    pub fn actor_model_rebuilds(&self) -> u64 { self.act.model_rebuilds }

    /// `ModelMesh` vertex/index GPU buffers actually (re)created by capacity growth since the layer started,
    /// for leak regressions: converges once the largest frame of each model's dynamic geometry has been seen.
    pub fn actor_mesh_buffer_grows(&self) -> u64 { self.act.mesh_buffer_grows }

    /// Previous rasterization's body visibility feeds native CAT animation refresh counters.
    pub fn visible_actor_ids(&self, out: &mut std::collections::HashSet<u32>) {
        out.clear();
        out.extend(self.act.items.iter().filter(|item| item.mesh == 0).map(|item| item.actor));
    }
    /// Current native water contacts; actual swim mode is separate from pre-swim entry contact.
    pub fn enhanced_water_contacts(&self, fog_end: f32, out: &mut Vec<(u32, [f32; 3], f32, bool)>) {
        self.act.enhanced_water_contacts(fog_end, out);
    }
    /// Current LOS-admitted bodies, including non-swimmers, for first-entry velocity history.
    pub fn enhanced_water_body_positions(&self, fog_end: f32, out: &mut Vec<(u32, [f32; 3])>) {
        self.act.enhanced_water_body_positions(fog_end, out);
    }



    /// Current-frame admitted native emitters only. Revocation drops both fluid and particle history.
    pub(crate) fn encode_enhanced_effects(&mut self, encoder: &mut wgpu::CommandEncoder, cam: &Camera) {
        let settings = self.enhanced_settings();
        let (spell, smoke) = (settings.gpu_spell_particles, settings.fluid_smoke);
        let layer = &mut self.act;
        if !spell && !smoke {
            layer.enhanced_effects.clear(); layer.effect_walkers.clear(); layer.effect_time = None; return;
        }
        let dt = layer.effect_time.map_or(0.0, |previous| (self.time - previous).clamp(0.0, 1.0 / 30.0));
        layer.effect_time = Some(self.time);
        let mut bodies = Vec::new();
        let mut walkers = HashMap::new();
        let mut owners = Vec::new();
        if smoke {
            for selected in self.foliage.selected() {
                let owner = layer.interaction_slot(selected.actor);
                owners.push(owner);
                let position = Vec3::from_array(selected.position);
                if let Some(previous) = layer.effect_walkers.get(&selected.actor).filter(|_| dt > 0.0) {
                    let velocity = (position - *previous) / dt;
                    if velocity.length_squared() > 0.0025 && velocity.length_squared() < 144.0 {
                        bodies.push(spell_smoke::Body { feet: selected.position, height: 1.8, velocity: velocity.to_array(), radius: 0.35, owner });
                    }
                }
                walkers.insert(selected.actor, position);
            }
        }
        if layer.effect_walkers.keys().any(|id| !walkers.contains_key(id)) {
            // A revoked character cannot leave a pressure/density wake in temporal state.
            layer.enhanced_effects.clear();
        }
        layer.effect_walkers = walkers;
        let admitted: std::collections::HashSet<_> = layer.items.iter().filter(|item| item.mesh == 0).map(|item| item.actor).collect();
        let mut sources: Vec<_> = layer.frames.iter().filter_map(|frame| {
            let emitter = frame.enhanced_emitter?;
            let is_smoke = matches!(emitter.kind, 1009 | 3017);
            if !admitted.contains(&frame.id) || !spell_smoke::enabled(emitter.kind, spell, smoke) { return None; }
            // set_actors moves valid CPU skins into ActorGpu; frame.skin is consumed.
            let vertices = layer.actors.get(&frame.id)?.skin.as_ref()?;
            let quad = vertices.as_chunks::<4>().0.iter().find(|quad| quad.iter().all(|v| v.color[3] > 0.0 && v.pos.iter().all(|v| v.is_finite())))?;
            let center = quad.iter().map(|v| Vec3::from_array(v.pos)).sum::<Vec3>() * 0.25;
            let radius = quad.iter().map(|v| center.distance(Vec3::from_array(v.pos))).fold(0.0f32, f32::max);
            let uv_min = quad.iter().fold(glam::Vec2::splat(f32::MAX), |uv,v| uv.min(glam::Vec2::from_array(v.uv)));
            let uv_max = quad.iter().fold(glam::Vec2::splat(f32::MIN), |uv,v| uv.max(glam::Vec2::from_array(v.uv)));
            let center = if is_smoke { Vec3::from_array(emitter.position) } else { center };
            Some((center.distance_squared(cam.pos), frame.id, is_smoke, spell_smoke::Source { position: center.to_array(), radius, color: quad[0].color,
                uv: [uv_min.x, uv_min.y, uv_max.x-uv_min.x, uv_max.y-uv_min.y], wind: emitter.wind.map(|v| v*0.1), lifetime: emitter.lifetime, rise: emitter.rise }))
        }).collect();
        sources.sort_unstable_by(|a,b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut counts = [0usize;2];
        sources.retain(|source| { let count = &mut counts[usize::from(source.2)]; *count += 1; *count <= spell_smoke::MAX_SOURCES });
        layer.enhanced_effects.retain(|id,_| sources.iter().any(|source| source.1 == *id));
        let forward = cam.forward();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        for (_, id, smoke, source) in sources {
            let programs = if smoke { &self.smoke_programs } else { &self.spell_programs };
            if !programs.ready() { continue; }
            let source_owner = layer.interaction_slot(Some(id));
            let (simulation, ready) = layer.enhanced_effects.entry(id).or_insert_with(|| (spell_smoke::Simulation::new(&self.device, programs, &self.interaction_admission.buffer), false));
            simulation.update(&self.queue, source, &bodies, &owners, source_owner, dt, self.time, right, up);
            *ready = simulation.encode(encoder, programs);
        }
    }

    /// Frustum-culls the submitted actors, fills the instance buffer; called by `render` before the pass.
    pub(crate) fn prepare_actors(&mut self, cam: Vec3, forward: Vec3, planes: &[Vec4; 6]) {
        let body_fog_enabled = self.enhanced_settings().ground_grass || self.enhanced_settings().water_wakes
            || self.enhanced_settings().foliage_enabled() || self.enhanced_settings().fluid_smoke;
        let sun_enabled = self.enhanced_settings().sun_shadows;
        let smoothing_enabled = self.enhanced_settings().character_smoothing;
        let layer = &mut self.act;
        layer.items.clear();
        let mut xf: Vec<ActorInstance> = vec![];
        for f in &layer.frames {
            // `RVisual_t::Rasterize` (randy31 0x1004d84a) draws nothing at a transparency <= 1e-5 (`_DAT_10095a08`)
            if f.alpha <= 1e-5 {
                continue;
            }
            let Some(model) = layer.models.get(&f.model) else { continue };
            let base = Mat4::from_cols_array_2d(&f.transform);
            let scale = base.x_axis.truncate().length().max(base.y_axis.truncate().length()).max(base.z_axis.truncate().length());
            for (mi, mesh) in model.meshes.iter().enumerate() {
                let Some(mesh) = mesh else { continue };
                let material=f.part_materials.get(mi);
                let alpha=material.and_then(|m|m.alpha).unwrap_or(f.alpha);
                if alpha<=1e-5 {continue;}
                let priority = f.part_priorities.get(mi).copied().flatten().or(f.priority);
                if priority.is_some_and(|p| p < 0) { continue; }
                let m = f.parts.get(mi).map_or(base, |p| base * Mat4::from_cols_array_2d(p));
                let margin = if mi == 0 { POSE_MARGIN } else { 0.0 };
                let c = m.transform_point3(mesh.sphere.0);
                let r = (mesh.sphere.1 + margin) * scale;
                if !f.always && frustum_culls_sphere(planes, c, r) {
                    continue;
                }
                let bucket = f.render_bucket.filter(|b| (0..=1800).contains(b))
                    .unwrap_or_else(|| render_bucket(m.w_axis.truncate().distance(cam)));
                let fog_depth = if (mi == 0 && body_fog_enabled) || sun_enabled {
                    let bounds = if let Some(actor) = layer.actors.get(&f.id).filter(|actor| mi == 0 && actor.vb.is_some()) {
                        actor.skin.as_ref().and(actor.skin_bounds)
                    } else { Some(mesh.bounds) };
                    // Native CAT vertices live on the GPU. Keep the conservative pose
                    // sphere: it may straddle fog even when all actual vertices do not.
                    bounds.map_or_else(|| {
                        let support = Vec3::new(forward.dot(m.x_axis.truncate()),
                            forward.dot(m.y_axis.truncate()), forward.dot(m.z_axis.truncate())).length();
                        (c - cam).dot(forward) - (mesh.sphere.1 + margin) * support
                    }, |bounds| bounds_depth(bounds, m, cam, forward))
                } else { f32::NEG_INFINITY };
                let native_shadow_receiver = f.native_shadow_source.filter(|_| sun_enabled).map(|_| {
                    // Native blobs have CPU-projected, terrain-warped vertices;
                    // their receiver can lie far from the owning actor's origin.
                    let (center, half) = layer.actors.get(&f.id).filter(|actor| mi == 0 && actor.vb.is_some())
                        .and_then(|actor| actor.skin_bounds).unwrap_or(mesh.bounds);
                    let extent = m.x_axis.truncate().abs() * half.x
                        + m.y_axis.truncate().abs() * half.y + m.z_axis.truncate().abs() * half.z;
                    (m.transform_point3(center), extent.length())
                });
                layer.items.push(Item { actor: f.id, model: f.model, mesh: mi, fog_depth, bucket, faded: alpha < 1.0, rendering_effect: f.rendering_effect, priority,
                    native_shadow_source: f.native_shadow_source, native_shadow_receiver,
                    water_body: (mi == 0 && f.water_admitted).then_some([f.transform[3][0], f.transform[3][1], f.transform[3][2]]), swimming: f.swimming, water_surface: f.water_surface,
                    smoothing: smoothing_enabled && f.character && model.native_skin.is_some()
                        && base.w_axis.truncate().distance_squared(cam) <= 225.0
                        && (mi == 0 || f.part_attractors.get(mi) == Some(&Some(0))) && mesh.smoothing.is_some() });
                // m0.w carries `alpha - 1` to the shader (the affine matrix' unused column element)
                let mut a = m.to_cols_array_2d();
                a[0][3] = alpha.min(1.0) - 1.0;
                let color = |value: Option<[f32; 3]>| {
                    let rgb = value.unwrap_or([0.0; 3]);
                    [rgb[0], rgb[1], rgb[2], if value.is_some() { 1.0 } else { 0.0 }]
                };
                xf.push(ActorInstance {
                    transform: a,
                    emissive: color(material.and_then(|m|m.emissive).or(f.emissive)),
                    specular: color(material.and_then(|m|m.specular).or(f.specular)),
                    power: [f.specular_power.unwrap_or(0.0), if f.specular_power.is_some() { 1.0 } else { 0.0 }, f.rendering_effect as f32, f.rendering_effect_time],
                    uv: f.part_uvs.get(mi).copied().unwrap_or([1.0, 1.0, 0.0, 0.0]),
                    character: u32::from(f.character),
                });
            }
        }
        if xf.len() > layer.cap || layer.bufs.is_empty() {
            layer.cap = xf.len().next_power_of_two().max(64);
            layer.bufs = (0..INST_RING)
                .map(|_| {
                    self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                        label: Some("actor instances"),
                        size: (layer.cap * std::mem::size_of::<ActorInstance>()) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                })
                .collect();
        }
        if !xf.is_empty() {
            self.queue.write_buffer(&layer.bufs[self.frame], 0, bytemuck::cast_slice(&xf));
        }
        if self.enhanced_settings.taa || self.enhanced_settings.motion_blur > 0.0 || self.enhanced_settings.enhanced_v2 {
            layer.motion_instances = xf;
        } else {
            layer.motion_instances.clear();
        }
        layer.commands.clear();
        for (index,item) in layer.items.iter().enumerate() {
            let model=&layer.models[&item.model];
            let Some(mesh)=&model.meshes[item.mesh] else {continue};
            for priority in 0..=10 {
                if mesh.draws.iter().any(|d| actor_priority(item.priority.or(visual_priority(d, actor_visual_key(item, d), &self.drawn_visuals)), item.faded, item.rendering_effect, blended_pipe(d.pipe)) == priority) {
                    layer.commands.push((priority, Some(index), item.bucket, item.rendering_effect == 3, layer.commands.len()));
                }
            }
        }
        for clear in &self.screen.depth_clears {
            layer.commands.push((clear.priority, None, render_bucket(Vec3::from(clear.position).distance(cam)), true, layer.commands.len()));
        }
        // AddToRenderList prepends: later submitted visuals lead equal-distance buckets.
        layer.commands.sort_unstable_by(|a, b| bucket_order(a.0, a.2, b.0, b.2).then_with(|| b.4.cmp(&a.4)));
        if smoothing_enabled { self.prepare_actor_smoothing(); }
    }

    /// Actors the host gave this frame that are drawn at all (`ActorFrame::alpha > 1e-5`): the CPU admission set Enhanced v2 builds
    /// every consumer from (shadow casters, lamp owners, benders, temporal history). A zero-alpha actor is simply absent.
    pub(crate) fn admitted_actor_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.act.frames.iter().filter(|frame| frame.alpha > 1e-5).map(|frame| frame.id)
    }

    /// Draws a native render list without changing its material pipelines.
    /// With sun shadows enabled, the caller binds the shared receiver at group 3.
    /// Draws a native render list restricted to native distance buckets `buckets` (RViewPort::Render's first/last bucket).
    pub(crate) fn draw_actors_in(&self,pass:&mut wgpu::RenderPass,priority:i32,buckets:std::ops::RangeInclusive<i32>)->usize {
        self.draw_actors_with(pass,&|pipe|Some(&self.world.actor_pipes[pipe]),Some(&*self.world.depth_clear_pipe),priority,buckets)
    }
    /// [`Self::draw_actors_in`] over an arbitrary pipeline table: `pipe_for` returns `None` for a slot the caller skips (Enhanced v2 splits opaque and blended slots across passes).
    pub(crate) fn draw_actors_with<'a>(&self,pass:&mut wgpu::RenderPass,pipe_for:&dyn Fn(usize)->Option<&'a wgpu::RenderPipeline>,depth_clear:Option<&wgpu::RenderPipeline>,priority:i32,buckets:std::ops::RangeInclusive<i32>)->usize {
        let layer=&self.act;
        let buf=layer.bufs.get(self.frame);
        let calls=std::cell::Cell::new(0usize);
        let mut start=layer.commands.partition_point(|command|command.0<priority);
        let limit=layer.commands.partition_point(|command|command.0<=priority);
        while start<limit {
            let end=(start+1..limit).find(|&i|layer.commands[i].3).unwrap_or(limit);
            if layer.commands[start].3 && buckets.contains(&layer.commands[start].2) {
                if let Some(clear)=depth_clear {
                    pass.set_pipeline(clear);
                    pass.draw(0..3,0..1);
                }
            }
            pass.set_bind_group(0,&self.globals_bg,&[]);
            if let Some(buf) = buf {
                pass.set_vertex_buffer(1, buf.slice(..));
            }
            let (pipe_c, vb_c, mat_c) = (std::cell::Cell::new(usize::MAX), std::cell::Cell::new(0usize), std::cell::Cell::new((u64::MAX, usize::MAX)));
            let one=|pass:&mut wgpu::RenderPass,i:usize| {
                if buf.is_none() {return}
                let it = &layer.items[i];
                if it.native_shadow_source.is_some_and(|source| self.replaces_native_shadow(source, it.native_shadow_receiver)) { return; }
                let model = &layer.models[&it.model];
                let Some(mesh) = model.meshes[it.mesh].as_ref() else { return };
                let smooth = it.smoothing.then_some(mesh.smoothing.as_ref()).flatten();
                let own = (it.mesh == 0).then(|| layer.actors.get(&it.actor)?.vb.as_ref()).flatten();
                let vb = if let Some(smooth) = smooth {
                    if it.mesh == 0 { &layer.actors[&it.actor].smoothing.as_ref().unwrap().vertices } else { &smooth.vertices }
                } else { own.unwrap_or(&mesh.vb) };
                let draws = smooth.map_or(mesh.draws.as_slice(), |smooth| smooth.draws.as_slice());
                let ib = smooth.map_or(&mesh.ib, |smooth| &smooth.indices);
                for d in draws {
                    let effect = it.rendering_effect;
                    if effect == 1 && model.holo.is_none() { continue; }
                    if actor_priority(it.priority.or(visual_priority(d, actor_visual_key(it, d), &self.drawn_visuals)), it.faded, effect, blended_pipe(d.pipe)) != priority {
                        continue;
                    }
                    let vb_ptr = vb as *const wgpu::Buffer as usize;
                    if vb_c.get() != vb_ptr {
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                        vb_c.set(vb_ptr);
                    }
                    let pipe = if it.faded { faded_pipe(d.pipe) } else { d.pipe };
                    let pipe = mesh_effect_pipe(pipe, effect, d.projection_pass);
                    if pipe_c.get() != pipe {
                        let Some(actor_pipe)=pipe_for(pipe) else { continue };
                        pass.set_pipeline(actor_pipe);
                        pipe_c.set(pipe);
                    }
                    if mat_c.get() != (it.model, d.mat) {
                        pass.set_bind_group(1, &model.mats[d.mat], &[]);
                        mat_c.set((it.model, d.mat));
                    }
                    if effect == 1 {
                        pass.set_bind_group(2, model.holo.as_ref().unwrap(), &[]);
                    }
                    let enhanced = (it.mesh == 0 && std::ptr::eq(d, &mesh.draws[0]))
                        .then(|| layer.enhanced_effects.get(&it.actor)).flatten().filter(|(_, ready)| *ready);
                    let replace_smoke = enhanced.is_some() && layer.frames.iter().find(|f| f.id == it.actor)
                        .and_then(|f| f.enhanced_emitter).is_some_and(|e| matches!(e.kind, 1009 | 3017));
                    if !replace_smoke { pass.draw_indexed(d.first_index..d.first_index + d.count, 0, i as u32..i as u32 + 1); }
                    if let Some((simulation, _)) = enhanced {
                        pass.set_vertex_buffer(0, simulation.vertices.slice(..));
                        pass.draw(0..simulation.vertex_count, i as u32..i as u32 + 1);
                        // The next native command must restore its own vertex buffer.
                        vb_c.set(0);
                    }
                    if let (Some(key), Some((_, next))) = (actor_visual_key(it, d), d.node) {
                        self.pending_visuals.borrow_mut().insert(key, next);
                    }
                    calls.set(calls.get() + 1);
                }
            };
            for command in &layer.commands[start..end] {
                if let Some(index)=command.1.filter(|_|buckets.contains(&command.2)) {one(pass,index);}
            }
            start=end;
        }
        calls.get()
    }
}

/// The CAT env blob changes texture/blend, not the material's ZFUNC/ZWRITE.
/// Separate phase indices keep its redraw immediately after the base submesh.
fn env_pipe(s: &ao_scene::Submesh) -> usize {
    let phase = blended_material(s.blend) as usize;
    let write = material_depth_write(s) as usize;
    let two = s.two_sided as usize;
    match s.depth_func {
        Some(2) => MATERIAL_ENV_COMPARE_PIPE + phase * 4 + write * 2 + two,
        Some(8) => MATERIAL_ENV_COMPARE_PIPE + 8 + phase * 4 + write * 2 + two,
        _ if write != 0 => MATERIAL_ENV_WRITE_PIPE + phase * 2 + two,
        _ => (if phase != 0 { ENV_BLEND_PIPE } else { ENV_PIPE }) + two,
    }
}

/// Grows `buf` to `needed` bytes (next power of two), in place, only when it no longer fits (returns `true`);
/// a no-op otherwise (returns `false`). Mirrors `native_liquid.rs`'s `vertex_capacity`/`index_capacity` buffers.
pub(crate) fn ensure_capacity(device: &wgpu::Device, buf: &mut wgpu::Buffer, capacity: &mut u64, needed: u64, usage: wgpu::BufferUsages, label: &str) -> bool {
    if needed <= *capacity {
        return false;
    }
    *capacity = needed.next_power_of_two().max(1);
    *buf = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some(label), size: *capacity, usage, mapped_at_creation: false });
    true
}

/// Whether two submeshes need the same GPU bind group and pipeline: every `Submesh` field except `indices` (the
/// triangle list, which varies frame to frame for dynamic actor geometry -- buff auras, particle effects -- and
/// never affects which bind group/pipeline a draw uses). Compares fields directly instead of cloning the whole
/// struct (and its potentially large `indices` `Vec`) just to strip and compare it.
pub(crate) fn same_material(a: &ao_scene::Submesh, b: &ao_scene::Submesh) -> bool {
    let ao_scene::Submesh {
        indices: _,
        texture,
        texture_clamp,
        blend,
        sprite_alpha_test,
        depth_write, depth_func, alpha_test, alpha_func, fog_enabled, lighting_enabled,
        render_priority, render_node, render_origin, stage1_env_texture,
        projection_pass,
        base_color,
        two_sided,
        emissive,
        overlay,
        glow_mask,
        prelit,
        uv_scroll,
        sky_points,
        sky_fog,
        sky_fog_color,
        uv_wave,
        uv_camera,
        sun_flicker,
        specular,
        shininess,
        env_texture,
        liquid,
        vegetation,
    } = a;
    *texture == b.texture
        && *texture_clamp == b.texture_clamp
        && *blend == b.blend
        && *sprite_alpha_test == b.sprite_alpha_test
        && *depth_write == b.depth_write && *depth_func == b.depth_func
        && *alpha_test == b.alpha_test && *alpha_func == b.alpha_func
        && *fog_enabled == b.fog_enabled && *lighting_enabled == b.lighting_enabled
        && *render_priority == b.render_priority && *render_node == b.render_node
        && *render_origin == b.render_origin && *stage1_env_texture == b.stage1_env_texture
        && *projection_pass == b.projection_pass
        && *base_color == b.base_color
        && *two_sided == b.two_sided
        && *emissive == b.emissive
        && *overlay == b.overlay
        && *glow_mask == b.glow_mask
        && *prelit == b.prelit
        && *uv_scroll == b.uv_scroll
        && *sky_points == b.sky_points
        && *sky_fog == b.sky_fog
        && *sky_fog_color == b.sky_fog_color
        && *uv_wave == b.uv_wave
        && *uv_camera == b.uv_camera
        && *sun_flicker == b.sun_flicker
        && *specular == b.specular
        && *shininess == b.shininess
        && *env_texture == b.env_texture
        && *liquid == b.liquid
        && *vegetation == b.vegetation
}

/// Material-and-shape summary of a mesh's submeshes for [`Model::topology`]: every field `same_material` compares,
/// with `indices` left empty so storing it every frame (dynamic buff/particle/effect geometry) never allocates or
/// copies the triangle list, only the handful of material fields actually needed to detect a change.
fn submesh_shape(submeshes: &[ao_scene::Submesh]) -> Vec<ao_scene::Submesh> {
    submeshes
        .iter()
        .map(|s| ao_scene::Submesh {
            indices: Vec::new(),
            texture: s.texture,
            texture_clamp: s.texture_clamp,
            blend: s.blend,
            sprite_alpha_test: s.sprite_alpha_test,
            depth_write: s.depth_write,
            depth_func: s.depth_func,
            alpha_test: s.alpha_test,
            alpha_func: s.alpha_func,
            fog_enabled: s.fog_enabled,
            lighting_enabled: s.lighting_enabled,
            render_priority: s.render_priority,
            render_node: s.render_node,
            render_origin: s.render_origin,
            stage1_env_texture: s.stage1_env_texture,
            projection_pass: s.projection_pass,
            base_color: s.base_color,
            two_sided: s.two_sided,
            emissive: s.emissive,
            overlay: s.overlay,
            glow_mask: s.glow_mask,
            prelit: s.prelit,
            uv_scroll: s.uv_scroll,
            sky_points: s.sky_points,
            sky_fog: s.sky_fog,
            sky_fog_color: s.sky_fog_color,
            uv_wave: s.uv_wave,
            uv_camera: s.uv_camera,
            sun_flicker: s.sun_flicker,
            specular: s.specular,
            shininess: s.shininess,
            env_texture: s.env_texture,
            liquid: s.liquid,
            vegetation: s.vegetation,
        })
        .collect()
}

/// Whether a draw belongs to the blended actor phase (client render list 6) rather than the opaque one (list 3).
fn blended_pipe(pipe: usize) -> bool {
    if pipe >= MATERIAL_ENV_COMPARE_PIPE {
        return (pipe - MATERIAL_ENV_COMPARE_PIPE) % 8 / 4 != 0;
    }
    if pipe >= MATERIAL_COMPARE_FADE_PIPE { return true; }
    if pipe >= MATERIAL_COMPARE_PIPE { return (pipe - MATERIAL_COMPARE_PIPE) % 18 / 2 >= Blend::AlphaBlend as usize; }
    if pipe >= MATERIAL_ONE_PIPE { return true; }
    if pipe >= MATERIAL_ENV_WRITE_PIPE { return pipe >= MATERIAL_ENV_WRITE_PIPE + 2; }
    if pipe >= MATERIAL_FADE_PIPE { return false; }
    if pipe >= MATERIAL_STATE_PIPE { return (pipe - MATERIAL_STATE_PIPE) / 2 >= Blend::AlphaBlend as usize; }
    matches!(pipe / 2, 2 | 3) && pipe < SKY_PIPE || (ENV_BLEND_PIPE..ENV_BLEND_PIPE + 2).contains(&pipe) || (SPRITE_PIPE..NATIVE_BLEND_PIPE + 8).contains(&pipe)
}

fn faded_pipe(pipe: usize) -> usize {
    if (MATERIAL_COMPARE_PIPE..MATERIAL_COMPARE_FADE_PIPE).contains(&pipe) {
        let variant = pipe - MATERIAL_COMPARE_PIPE;
        if variant % 18 < 4 {
            return MATERIAL_COMPARE_FADE_PIPE + variant / 36 * 8 + variant % 36 / 18 * 4 + variant % 4;
        }
    }
    if pipe < 4 { FADE_PIPE + pipe }
    else if (MATERIAL_STATE_PIPE..MATERIAL_STATE_PIPE + 4).contains(&pipe) {
        MATERIAL_FADE_PIPE + pipe - MATERIAL_STATE_PIPE
    } else { pipe }
}

/// Each delta state overrides only the native states it owns; mode 3 clears depth before its draw.
fn mesh_effect_pipe(pipe: usize, effect: u32, projection_pass: bool) -> usize {
    match effect {
        2 if !projection_pass => pipe, // Native RTriMesh cast fails for CAT/procedural visuals.
        1 | 2 | 4..=7 => MESH_EFFECT_PIPE + (effect as usize - 1) * MESH_EFFECT_STRIDE + pipe,
        _ => pipe,
    }
}

/// Process rejects disabled visuals before transparency moves enabled visuals to list 6.
fn actor_priority(priority: Option<i32>, faded: bool, effect: u32, blended: bool) -> i32 {
    if priority.is_some_and(|p| p < 0) { return -1; }
    if faded { return 6; }
    priority.unwrap_or(match effect {
        1 => 5,
        4..=6 => 6,
        _ => if blended { 6 } else { 3 },
    })
}

/// RVisual::AddToRenderList 1004ca33..1004ca81; constants are f64, near product stored as f32.
pub(super) fn render_bucket(distance: f32) -> i32 {
    let (value, offset) = if distance < 200.0 { ((distance * 5.0) as f64, 0) } else { (distance as f64, 800) };
    ((value - 0.49999).round_ties_even() as i32).saturating_add(offset).clamp(0, 1800)
}

pub(super) fn bucket_order(priority_a: i32, bucket_a: i32, priority_b: i32, bucket_b: i32) -> std::cmp::Ordering {
    priority_a.cmp(&priority_b).then_with(|| if matches!(priority_a, 5..=7) {
        bucket_b.cmp(&bucket_a)
    } else {
        bucket_a.cmp(&bucket_b)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn actor_character_metadata_preserves_the_native_instance_prefix() {
        assert_eq!(std::mem::offset_of!(super::ActorInstance, transform), 0);
        assert_eq!(std::mem::offset_of!(super::ActorInstance, emissive), 64);
        assert_eq!(std::mem::offset_of!(super::ActorInstance, specular), 80);
        assert_eq!(std::mem::offset_of!(super::ActorInstance, power), 96);
        assert_eq!(std::mem::offset_of!(super::ActorInstance, uv), 112);
        assert_eq!(std::mem::offset_of!(super::ActorInstance, character), 128);
        assert_eq!(std::mem::size_of::<super::ActorInstance>(), 132);
        assert!(!ao_scene::ActorFrame::default().character);
    }
    #[test]
    fn mesh_delta_state_pipeline_modes_preserve_material_states() {
        for pipe in 0..MESH_EFFECT_STRIDE {
            for projection_pass in [false, true] {
                assert_eq!(mesh_effect_pipe(pipe, 0, projection_pass), pipe);
                assert_eq!(mesh_effect_pipe(pipe, 3, projection_pass), pipe);
                for effect in [1, 2, 4, 5, 6, 7] {
                    let expected = if effect == 2 && !projection_pass { pipe } else { effect as usize * MESH_EFFECT_STRIDE + pipe };
                    assert_eq!(mesh_effect_pipe(pipe, effect, projection_pass), expected);
                }
            }
        }
    }
    use super::*;
    #[test]
    fn native_effect_admission_uses_uploaded_skin_and_revokes_hidden_sources() {
        use ao_scene::Vertex;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("native effect regression requires GPU");
        renderer.set_enhanced_settings(EnhancedSettings {
            gpu_spell_particles: true, fluid_smoke: true, ..Default::default()
        });
        renderer.spell_programs.wait();
        renderer.smoke_programs.wait();
        let skin = std::sync::Arc::new(vec![
            Vertex { pos: [-0.3, -0.3, -3.0], uv: [0.0, 0.0], color: [1.0; 4], ..Default::default() },
            Vertex { pos: [0.3, -0.3, -3.0], uv: [1.0, 0.0], color: [1.0; 4], ..Default::default() },
            Vertex { pos: [-0.3, 0.3, -3.0], uv: [0.0, 1.0], color: [1.0; 4], ..Default::default() },
            Vertex { pos: [0.3, 0.3, -3.0], uv: [1.0, 1.0], color: [1.0; 4], ..Default::default() },
        ]);
        let scene = Scene { meshes: vec![ao_scene::Mesh {
            vertices: skin.as_ref().clone(),
            submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2, 2, 1, 3], None)],
        }], ..Default::default() };
        renderer.add_actor_model(1, &scene);
        let camera = Camera::look_at(Vec3::ZERO, Vec3::NEG_Z);
        let encode = |renderer: &mut Renderer| {
            renderer.prepare_actors(camera.pos, camera.forward(), &[Vec4::ZERO; 6]);
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            renderer.encode_enhanced_effects(&mut encoder, &camera);
            renderer.queue.submit([encoder.finish()]);
        };
        let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
        for kind in [1018, 1009, 3017] {
            let mut frame = ActorFrame { id: 7, model: 1, always: true, skin: Some(skin.clone()),
                enhanced_emitter: Some(ao_scene::EnhancedEmitter { kind, position: [0.0, 0.0, -3.0],
                    wind: [0.0; 3], lifetime: 1.4, rise: 1.0 }), ..Default::default() };
            renderer.set_actors(vec![frame.clone()]);
            assert!(renderer.act.frames[0].skin.is_none(), "upload consumes the submitted skin");
            assert!(std::sync::Arc::ptr_eq(renderer.act.actors[&7].skin.as_ref().unwrap(), &skin));
            encode(&mut renderer);
            renderer.time += 1.0 / 60.0;
            encode(&mut renderer);
            assert!(renderer.act.enhanced_effects.get(&7).is_some_and(|(_, ready)| *ready),
                "native class {kind} must reach its real GPU simulation after skin upload");
            frame.alpha = 0.0;
            renderer.set_actors(vec![frame]);
            encode(&mut renderer);
            assert!(renderer.act.enhanced_effects.is_empty(), "hidden sources revoke all GPU history");
        }
        renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert!(pollster::block_on(errors.pop()).is_none(), "admitted native effect GPU dispatch");
    }
    #[test]
    fn water_contacts_require_native_surface_and_current_primary_admission() {
        let mut layer = ActorLayer::default();
        layer.models.insert(1, Model { point_cache_revision: 0, meshes: vec![], mats: vec![], material_bindings: vec![], holo: None, views: vec![],
            view_of: HashMap::new(), textures: vec![], identities: HashMap::new(),
            topology: vec![(0, vec![ao_scene::Submesh::new(vec![], None)])], native_skin: None });
        assert!(!ActorFrame::default().swimming);
        assert!(!ActorFrame::default().water_admitted);
        assert_eq!(ActorFrame::default().water_surface, None);
        let feet = [1.0, 2.0, 3.0];
        layer.items.push(Item { actor: 7, model: 1, mesh: 0, fog_depth: 9.0, bucket: 0,
            faded: false, rendering_effect: 0, priority: None, native_shadow_source: None, native_shadow_receiver: None, water_body: Some(feet), swimming: false, water_surface: None, smoothing: false });
        let mut positions = vec![];
        layer.enhanced_water_body_positions(20.0, &mut positions);
        assert_eq!(positions, [(7, feet)], "dry admitted bodies seed first-entry velocity history");
        let mut contacts = vec![];
        layer.enhanced_water_contacts(20.0, &mut contacts);
        assert!(contacts.is_empty(), "bank bodies have no native contact");
        layer.items[0].water_surface = Some(2.5);
        layer.enhanced_water_contacts(20.0, &mut contacts);
        assert_eq!(contacts, [(7, feet, 2.5, false)], "wet pre-swim entry preserves actual non-swim mode");
        layer.items[0].swimming = true;
        layer.enhanced_water_contacts(20.0, &mut contacts);
        assert_eq!(contacts, [(7, feet, 2.5, true)]);
        layer.enhanced_water_contacts(9.0, &mut contacts);
        assert!(contacts.is_empty(), "fully fogged contacts are absent");
        layer.items[0].faded = true;
        layer.enhanced_water_contacts(20.0, &mut contacts);
        assert!(contacts.is_empty(), "faded primary bodies are absent");
        layer.items[0].faded = false;
        layer.items[0].mesh = 1;
        layer.enhanced_water_contacts(20.0, &mut contacts);
        assert!(contacts.is_empty(), "mounts cannot create contacts");
        layer.items.clear();
        layer.enhanced_water_contacts(20.0, &mut contacts);
        assert!(contacts.is_empty(), "previous contacts cannot persist");
        layer.enhanced_water_body_positions(20.0, &mut positions);
        assert!(positions.is_empty(), "absent bodies cannot retain history");
    }

    #[test]
    fn grass_admission_uses_final_camera_body_depth_and_native_fog_exemptions() {
        let mut layer = ActorLayer::default();
        layer.models.insert(1, Model { meshes: vec![], mats: vec![], material_bindings: vec![], holo: None, views: vec![],
            view_of: HashMap::new(), textures: vec![], identities: HashMap::new(),
            topology: vec![(0, vec![ao_scene::Submesh::new(vec![], None)])], native_skin: None, point_cache_revision: 0 });
        layer.items.push(Item { actor: 7, model: 1, mesh: 0, fog_depth: 9.0,
            bucket: 0, faded: false, rendering_effect: 0, priority: None, native_shadow_source: None, native_shadow_receiver: None, water_body: None, swimming: false, water_surface: None, smoothing: false });
        let camera = Camera::look_at(Vec3::ZERO, -Vec3::Z);
        let ids = |layer: &ActorLayer, end| layer.enhanced_grass_actor_ids(end).collect::<Vec<_>>();
        assert_eq!(ids(&layer, 20.0), [7], "positive control: a drawn opaque body bends");
        assert!(ids(&layer, 9.0).is_empty(), "the entire bound at fog end is equivalent to absence");
        assert_eq!(ids(&layer, 9.01), [7], "a body crossing fog end must not be culled by its feet or center");
        assert_eq!(ids(&layer, f32::MAX), [7], "disabled fog preserves admission");
        let bounds = (Vec3::new(10.0, 1.0, -10.0), Vec3::new(0.4, 0.9, 0.0));
        assert_eq!(bounds_depth(bounds, Mat4::IDENTITY, camera.pos, camera.forward()), 10.0, "view depth, not radial distance");
        let moved = Camera::look_at(Vec3::new(0.0, 0.0, -2.0), Vec3::new(0.0, 0.0, -10.0));
        assert_eq!(bounds_depth(bounds, Mat4::IDENTITY, moved.pos, moved.forward()), 8.0, "final camera movement");
        let pitched = Camera::look_at(Vec3::ZERO, Vec3::new(0.0, 1.0, -1.0));
        assert!(bounds_depth(bounds, Mat4::IDENTITY, pitched.pos, pitched.forward()) < 9.0, "camera pitch can expose part of the body");
        let fixture_camera = Camera::look_at(Vec3::new(0.0, 3.0, 8.0), Vec3::new(0.0, 1.0, 0.0));
        let fixture_bounds = (Vec3::new(0.0, 0.9, -1.0), Vec3::new(0.4, 0.9, 0.0));
        let transform = Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0));
        layer.items[0].fog_depth = bounds_depth(fixture_bounds, transform, fixture_camera.pos, fixture_camera.forward());
        assert!((layer.items[0].fog_depth - 9.0223).abs() < 0.0001);
        assert!(ids(&layer, 9.0).is_empty(), "the original rigid fog leak is rejected without the sphere's pose margin");
        let transform = Mat4::from_scale_rotation_translation(Vec3::new(2.0, 0.5, 3.0),
            glam::Quat::from_rotation_y(0.7), Vec3::new(1.0, -2.0, -4.0));
        let corner_min = [-1.0, 1.0].into_iter().flat_map(|x| [-1.0, 1.0].into_iter().flat_map(move |y|
            [-1.0, 1.0].into_iter().map(move |z| fixture_bounds.0 + fixture_bounds.1 * Vec3::new(x, y, z))))
            .map(|corner| (transform.transform_point3(corner) - fixture_camera.pos).dot(fixture_camera.forward()))
            .fold(f32::INFINITY, f32::min);
        assert!((bounds_depth(fixture_bounds, transform, fixture_camera.pos, fixture_camera.forward()) - corner_min).abs() < 0.00001,
            "scaled and rotated part bounds use their nearest transformed corner");
        layer.models.get_mut(&1).unwrap().topology[0].1[0].fog_enabled = false;
        assert_eq!(ids(&layer, 1.0), [7], "native material fog disable remains visible");
        layer.models.get_mut(&1).unwrap().topology[0].1[0].fog_enabled = true;
        for effect in [2, 4, 5, 6] {
            layer.items[0].rendering_effect = effect;
            assert_eq!(ids(&layer, 1.0), [7], "native effect {effect} disables fog");
        }
        layer.items[0].rendering_effect = 0;
        layer.items[0].faded = true;
        assert!(ids(&layer, f32::MAX).is_empty(), "fog disable does not bypass body alpha admission");
        layer.items[0].faded = false;
        layer.items[0].mesh = 1;
        assert!(ids(&layer, f32::MAX).is_empty(), "only the current primary body admits a remote bender");
    }
    #[test]
    fn immutable_actor_textures_share_uploads_pixels_and_reclaim() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("resource regression requires GPU");
        assert_eq!(renderer.materials_root, native_materials::default_root());
        renderer.set_materials_root(None);
        assert!(renderer.materials_root.is_none());
        let key = TextureKey { rdb_type: 1010004, id: 481 };
        let mut scene = Scene::default();
        scene.textures.insert(key, ao_scene::Texture { source: ao_scene::TextureSource::Rgba,
            width: 2, height: 2, rgba: [213, 77, 31, 255].repeat(4) });
        scene.meshes.push(ao_scene::Mesh {
            vertices: [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].into_iter()
                .map(|(x, y)| ao_scene::Vertex { pos: [x, y, 0.0], normal: [0.0, 0.0, 1.0], ..Default::default() }).collect(),
            submeshes: vec![ao_scene::Submesh { two_sided: true, emissive: [1.0; 3],
                ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key)) }],
        });
        let capture = |renderer: &mut Renderer, model| {
            let mut frame = ActorFrame { id: 1, model, ..Default::default() };
            frame.transform[3][2] = -3.0;
            renderer.set_actors(vec![frame]);
            let targets = Targets::new(renderer, 32, 32);
            let output = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            renderer.render(&output.create_view_tracked(&Default::default()), &targets,
                &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
            super::texture_to_rgba(renderer, &output, 32, 32).unwrap()
        };
        renderer.add_actor_model(1, &scene);
        let before = capture(&mut renderer, 1);
        assert!(before.as_chunks::<4>().0.iter().any(|p| p[0] > 180 && p[1] < 100), "textured quad must actually render");
        let uploads = renderer.actor_texture_uploads();
        renderer.add_actor_model(2, &scene);
        assert_eq!(renderer.actor_texture_uploads(), uploads + 1, "only model-local white placeholder uploads");
        assert_eq!(renderer.act.models[&1].views[1], renderer.act.models[&2].views[1]);
        assert_eq!(renderer.actor_texture_resources(), (3, 28), "two white texels plus one shared 2x2 mip chain");
        assert_eq!(before, capture(&mut renderer, 2), "interning preserves every pixel");
        assert_ne!(renderer.act.models[&1].meshes[0].as_ref().unwrap().vb, renderer.act.models[&2].meshes[0].as_ref().unwrap().vb);
        assert_ne!(renderer.act.models[&1].meshes[0].as_ref().unwrap().ib, renderer.act.models[&2].meshes[0].as_ref().unwrap().ib);
        let mut moved = scene.clone();
        for vertex in &mut moved.meshes[0].vertices { vertex.pos[0] += 100.0; }
        renderer.update_actor_model(1, &moved);
        assert_eq!(before, capture(&mut renderer, 2), "mutable geometry updates must not affect the other model");
        assert_eq!(renderer.actor_texture_uploads(), uploads + 1, "geometry updates never reupload an identical texture");
        let shared = Arc::downgrade(&renderer.act.models[&1].textures[1]);
        renderer.remove_actor_model(1);
        assert!(shared.upgrade().is_some());
        renderer.remove_actor_model(2);
        assert!(shared.upgrade().is_none(), "no strong reference in the lookup cache");
        assert_eq!(renderer.actor_texture_resources(), (0, 0));
        assert!(renderer.act.textures.is_empty());

        // Generated bitmaps may reuse archive-looking keys (composited CAT skins).
        scene.textures.get_mut(&key).unwrap().source = ao_scene::TextureSource::Generated;
        renderer.add_actor_model(1, &scene);
        scene.textures.get_mut(&key).unwrap().rgba = [31, 213, 77, 255].repeat(4);
        renderer.add_actor_model(2, &scene);
        assert_ne!(renderer.act.models[&1].views[1], renderer.act.models[&2].views[1]);
        assert_ne!(capture(&mut renderer, 1), capture(&mut renderer, 2));

        scene.textures.get_mut(&key).unwrap().source = ao_scene::TextureSource::Rgba;
        renderer.add_actor_model(3, &scene);
        let original = renderer.act.models[&3].views[1].clone();
        scene.textures.get_mut(&key).unwrap().rgba = [77, 31, 213, 255].repeat(4);
        renderer.add_actor_model(6, &scene);
        assert_ne!(original, renderer.act.models[&6].views[1], "same canonical key and dimensions can load different asset families");
        assert_ne!(capture(&mut renderer, 3), capture(&mut renderer, 6), "different decoded native contents retain their pixels");
        scene.textures.get_mut(&key).unwrap().rgba = [31, 213, 77, 255].repeat(4);
        renderer.set_texture_policy(0, false, false);
        renderer.add_actor_model(4, &scene);
        assert_ne!(original, renderer.act.models[&4].views[1], "policy variants cannot alias");
        renderer.update_actor_model(3, &scene);
        assert_eq!(renderer.act.models[&3].views[1], renderer.act.models[&4].views[1], "updates migrate to the new policy variant");
        scene.textures.get_mut(&key).unwrap().source = ao_scene::TextureSource::PngRgba;
        renderer.add_actor_model(5, &scene);
        assert_ne!(renderer.act.models[&4].views[1], renderer.act.models[&5].views[1], "source format participates in identity");
        let previous = renderer.act.models[&5].views[1].clone();
        let root = std::env::temp_dir().join("aomac-explicit-material-root-regression");
        renderer.set_materials_root(Some(root.clone()));
        let revision = renderer.materials_revision;
        renderer.set_materials_root(Some(root.clone()));
        assert_eq!(renderer.materials_revision, revision, "same root does not invalidate cached materials");
        assert_eq!(renderer.materials_root.as_ref(), Some(&root));
        assert_eq!(renderer.act.models[&5].views[1], previous, "setter leaves already-uploaded materials intact");
        renderer.update_actor_model(5, &scene);
        assert_ne!(renderer.act.models[&5].views[1], previous, "next model upload resolves the new root");
        let previous = renderer.act.models[&5].views[1].clone();
        renderer.set_materials_root(None);
        renderer.update_actor_model(5, &scene);
        assert_ne!(renderer.act.models[&5].views[1], previous, "disabling side-loading also invalidates the next upload");
        renderer.clear_actors();
        assert_eq!(renderer.actor_texture_resources(), (0, 0));
        assert!(renderer.act.textures.is_empty());
    }

    #[test]
    fn actor_texture_identity_excludes_generated_namespaces_and_tracks_upload_policy() {
        let key = TextureKey { rdb_type: 1_010_004, id: 7 };
        let texture = ao_scene::Texture { width: 4, height: 4, rgba: vec![255; 64], source: ao_scene::TextureSource::Rgba };
        let policy = native_texture::Policy::default();
        let identity = TextureIdentity::new(key, &texture, policy);
        assert!(identity.immutable());
        for rdb_type in [0, 1, 0x4000_0007] {
            assert!(!TextureIdentity::new(TextureKey { rdb_type, id: 7 }, &texture, policy).immutable());
        }
        for changed in [
            native_texture::Policy { quality: 0, ..policy },
            native_texture::Policy { compressed: true, ..policy },
            native_texture::Policy { better: true, ..policy },
        ] {
            assert!(identity != TextureIdentity::new(key, &texture, changed));
        }
        let changed = ao_scene::Texture { width: 2, ..texture.clone() };
        assert!(identity != TextureIdentity::new(key, &changed, policy));
        let changed = ao_scene::Texture { rgba: vec![127; 64], ..texture.clone() };
        assert!(identity != TextureIdentity::new(key, &changed, policy), "canonical keys may load different native quality families");
    }

    #[test]
    fn dynamic_geometry_update_retains_uploaded_texture_views() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("resource regression requires GPU");
        let key = TextureKey { rdb_type: 1010004, id: 1 };
        let mut scene = Scene::default();
        scene.textures.insert(key, ao_scene::Texture { source: Default::default(), width: 1, height: 1, rgba: vec![255; 4] });
        scene.meshes.push(ao_scene::Mesh {
            vertices: vec![ao_scene::Vertex::default(); 3],
            submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2], Some(key))],
        });
        renderer.add_actor_model(7, &scene);
        let view = renderer.act.models[&7].views[1].clone();
        let mesh = renderer.act.models[&7].meshes[0].as_ref().unwrap();
        let (vb, ib, mat) = (mesh.vb.clone(), mesh.ib.clone(), renderer.act.models[&7].mats[0].clone());
        scene.meshes[0].vertices[0].pos = [3.0, 0.0, 0.0];
        scene.textures.clear();
        renderer.update_actor_model(7, &scene);
        let model = &renderer.act.models[&7];
        assert_eq!(model.view_of.get(&key), Some(&1));
        assert_eq!(model.views.len(), 2);
        assert_eq!(model.views[1], view);
        let mesh = model.meshes[0].as_ref().unwrap();
        assert_eq!(mesh.vb, vb);
        assert_eq!(mesh.ib, ib);
        assert_eq!(model.mats[0], mat);
        assert_eq!(mesh.sphere.0.x, 1.5, "vertex update refreshes culling bounds");
        // Equal index counts with the same material: connectivity changing alone (dynamic buff/particle/effect
        // geometry) is not a material change, so the fast path rewrites the existing index buffer in place.
        scene.meshes[0].submeshes[0].indices.swap(1, 2);
        renderer.update_actor_model(7, &scene);
        let mesh = renderer.act.models[&7].meshes[0].as_ref().unwrap();
        assert_eq!(mesh.ib, ib, "same-material connectivity change reuses the index buffer in place");
        let changed = mesh.vb.clone();
        scene.meshes[0].submeshes[0].texture_clamp = true;
        renderer.update_actor_model(7, &scene);
        assert_ne!(renderer.act.models[&7].meshes[0].as_ref().unwrap().vb, changed);
        assert_eq!(renderer.act.models[&7].views[1], view);
        renderer.add_actor_model(7, &scene);
        assert!(renderer.act.models[&7].view_of.is_empty(), "ordinary replacement must not retain stale resources");
    }
    #[test]
    fn native_priority_suppression_and_material_order() {
        assert_eq!(actor_priority(None, false, 0, false), 3);
        assert_eq!(actor_priority(None, false, 0, true), 6);
        assert_eq!(actor_priority(Some(-1), true, 5, true), -1);
        assert_eq!(actor_priority(Some(3), false, 0, true), 3);
        assert_eq!(actor_priority(Some(3), true, 0, true), 6);
        assert_eq!(actor_priority(None, false, 1, true), 5);
        assert_eq!(actor_priority(None, false, 5, false), 6);
        assert_eq!(actor_priority(Some(7), false, 0, true), 7);
    }
    #[test]
    fn same_material_covers_every_submesh_field_but_indices() {
        let base = ao_scene::Submesh::new(vec![0, 1, 2], Some(TextureKey { rdb_type: 1, id: 1 }));
        // Indices alone (the triangle list, which dynamic buff/particle/effect geometry varies every frame)
        // never make two submeshes incompatible for GPU resource reuse.
        let mut only_indices = base.clone();
        only_indices.indices = vec![2, 1, 0];
        assert!(same_material(&base, &only_indices));
        // Every other field is material-relevant: listed explicitly so a new `Submesh` field that is never
        // exercised here is a visible gap in this test, not a silent correctness hole in `same_material`.
        let variants: Vec<fn(&mut ao_scene::Submesh)> = vec![
            |s| s.texture = Some(TextureKey { rdb_type: 1, id: 2 }),
            |s| s.texture_clamp = true,
            |s| s.blend = Blend::AlphaBlend,
            |s| s.sprite_alpha_test = true,
            |s| s.base_color = [0.0, 0.0, 0.0, 1.0],
            |s| s.two_sided = true,
            |s| s.emissive = [1.0, 0.0, 0.0],
            |s| s.overlay = true,
            |s| s.glow_mask = true,
            |s| s.prelit = true,
            |s| s.uv_scroll = [1.0, 0.0],
            |s| s.sky_points = true,
            |s| s.sky_fog = true,
            |s| s.sky_fog_color = Some([0.1, 0.2, 0.3]),
            |s| s.uv_wave = [1.0, 0.0, 0.0, 0.0],
            |s| s.uv_camera = [1.0, 0.0, 0.0, 0.0],
            |s| s.sun_flicker = true,
            |s| s.specular = [1.0, 0.0, 0.0],
            |s| s.shininess = 10.0,
            |s| s.env_texture = Some(TextureKey { rdb_type: 1, id: 3 }),
            |s| s.liquid = true,
            |s| s.vegetation = true,
        ];
        for change in &variants {
            let mut other = base.clone();
            change(&mut other);
            assert!(!same_material(&base, &other), "a changed material field must compare as a different material");
        }
    }

    /// Leak regression (user report, ICC pf 4582, 60 dynels): `update_actor_model`'s old fast path only reused
    /// GPU buffers when vertex/index *counts* matched exactly, so dynamic buff/particle/effect geometry -- whose
    /// counts vary every frame -- fell through to a full rebuild (new vb/ib/bind groups) on every call. With
    /// capacity-based buffers, a same-material actor whose geometry size oscillates must create zero further
    /// buffers or bind groups once its largest frame has been seen.
    #[test]
    fn dynamic_counts_stop_creating_buffers_once_capacity_covers_the_largest_frame() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("resource regression requires GPU");
        let key = TextureKey { rdb_type: 1010004, id: 1 };
        let mesh_of = |n: usize| {
            let mut scene = Scene::default();
            scene.textures.insert(key, ao_scene::Texture { source: Default::default(), width: 1, height: 1, rgba: vec![255; 4] });
            let indices: Vec<u32> = (0..n as u32 * 3).map(|i| i % n as u32).collect();
            scene.meshes.push(ao_scene::Mesh { vertices: vec![ao_scene::Vertex::default(); n], submeshes: vec![ao_scene::Submesh::new(indices, Some(key))] });
            scene
        };
        renderer.add_actor_model(9, &mesh_of(4));
        assert_eq!(renderer.actor_model_rebuilds(), 0);
        // Warm-up: grow to the largest frame this actor will ever submit, with counts varying every call.
        for n in [4usize, 8, 3, 10, 2, 10, 6, 10] {
            renderer.update_actor_model(9, &mesh_of(n));
        }
        let (rebuilds, grows) = (renderer.actor_model_rebuilds(), renderer.actor_mesh_buffer_grows());
        assert_eq!(rebuilds, 0, "same-material geometry count churn must never fall back to a full rebuild");
        assert!(grows > 0, "capacity must have grown at least once to reach the largest warm-up frame");
        // Steady state: further frames whose counts vary but never exceed the warm-up maximum create nothing.
        for n in [5usize, 2, 9, 1, 10, 4, 7] {
            renderer.update_actor_model(9, &mesh_of(n));
        }
        assert_eq!(renderer.actor_model_rebuilds(), 0);
        assert_eq!(renderer.actor_mesh_buffer_grows(), grows, "steady state with varying counts must create zero further buffers");
    }

    /// Visual-correctness proof: `update_actor_model`'s capacity-buffer fast path must render byte-identical
    /// pixels to a full rebuild of the same geometry, across vertex/index counts that grow and shrink (dynamic
    /// buff/particle/effect geometry, a Body Boost-style pulsing aura or a remote character's animated
    /// attachment), and a material (texture) change must still fall through to a full rebuild and visibly pick
    /// up the new texture (a remote character's equipment/texture change), proving bind groups refresh.
    #[test]
    fn dynamic_geometry_fast_path_renders_identically_to_a_full_rebuild() {
        let (w, h) = (64u32, 64u32);
        let (eye, look_at) = ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]);
        let (key_a, key_b) = (TextureKey { rdb_type: 1010004, id: 1 }, TextureKey { rdb_type: 1010004, id: 2 });
        // A fixed, always-visible 1x1m emissive red quad (indices 0,1,2,0,2,3) plus `extra` churn vertices and
        // triangles placed far behind the camera (world z ~= 995, well outside the view frustum): they exercise
        // dynamic vertex/index counts without ever contributing a rasterized pixel, so any pixel difference can
        // only come from the fast path mishandling the capacity buffers, not from the churn geometry itself.
        let mesh_of = |extra: usize, key: TextureKey| {
            let mut vertices = vec![
                ao_scene::Vertex { pos: [-0.5, -0.5, 0.0], normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() },
                ao_scene::Vertex { pos: [0.5, -0.5, 0.0], normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() },
                ao_scene::Vertex { pos: [0.5, 0.5, 0.0], normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() },
                ao_scene::Vertex { pos: [-0.5, 0.5, 0.0], normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() },
            ];
            let mut indices = vec![0u32, 1, 2, 0, 2, 3];
            for i in 0..extra {
                vertices.push(ao_scene::Vertex { pos: [0.0, 0.0, 1000.0 + i as f32], normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() });
            }
            for i in 0..extra / 3 {
                let base = (4 + i * 3) as u32;
                indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
            let sub = ao_scene::Submesh { emissive: [1.0, 0.0, 0.0], base_color: [1.0; 4], two_sided: true, ..ao_scene::Submesh::new(indices, Some(key)) };
            let mut scene = Scene::default();
            scene.textures.insert(key_a, ao_scene::Texture { source: Default::default(), width: 1, height: 1, rgba: vec![255, 0, 0, 255] });
            scene.textures.insert(key_b, ao_scene::Texture { source: Default::default(), width: 1, height: 1, rgba: vec![0, 255, 0, 255] });
            scene.meshes.push(ao_scene::Mesh { vertices, submeshes: vec![sub] });
            scene
        };
        let frame = ActorFrame { id: 1, model: 1, transform: { let mut t = ao_scene::IDENTITY; t[3][2] = -5.0; t }, ..Default::default() };
        let capture = |r: &mut Renderer| -> Vec<u8> {
            r.set_actors(vec![frame.clone()]);
            let targets = Targets::new(r, w, h);
            let out = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2, format: r.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
            });
            r.render(&out.create_view_tracked(&Default::default()), &targets, &Camera::look_at(eye.into(), look_at.into()));
            super::texture_to_rgba(r, &out, w, h).unwrap()
        };
        let new_renderer = || {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            Renderer::new(&instance, None).expect("visual regression requires GPU")
        };
        let extras = [0usize, 9, 3, 12, 6, 12];
        // Reference: a brand-new `Renderer` every frame, so `add_actor_model` always does a full rebuild.
        let reference: Vec<Vec<u8>> = extras.iter().map(|&n| {
            let mut r = new_renderer();
            r.add_actor_model(1, &mesh_of(n, key_a));
            capture(&mut r)
        }).collect();
        // Fast path: one `Renderer`, `add_actor_model` once, `update_actor_model` for every further frame.
        let mut r = new_renderer();
        for (i, &n) in extras.iter().enumerate() {
            if i == 0 { r.add_actor_model(1, &mesh_of(n, key_a)); } else { r.update_actor_model(1, &mesh_of(n, key_a)); }
            let pixels = capture(&mut r);
            assert_eq!(pixels, reference[i], "frame {i} (extra={n}): fast path must render identically to a full rebuild");
        }
        assert_eq!(r.actor_model_rebuilds(), 0, "same-material geometry churn must never fall back to a full rebuild");
        // Texture swap (a remote character's equipment/texture change): same connectivity, different material;
        // must trigger a full rebuild and visibly pick up the new texture, proving bind groups refresh.
        let before_swap = capture(&mut r);
        r.update_actor_model(1, &mesh_of(*extras.last().unwrap(), key_b));
        let after_swap = capture(&mut r);
        assert_eq!(r.actor_model_rebuilds(), 1, "a material change must fall through to a full rebuild (new bind groups)");
        assert_ne!(after_swap, before_swap, "a texture swap must change the rendered pixels");
        let mut reference_swap = new_renderer();
        reference_swap.add_actor_model(1, &mesh_of(*extras.last().unwrap(), key_b));
        assert_eq!(capture(&mut reference_swap), after_swap, "the rebuilt (texture-swapped) frame must also match a from-scratch render");
    }

    #[test]
    fn native_render_buckets_share_explicit_and_world_distance_keys() {
        for (distance, expected) in [(0.0, 0), (0.19, 0), (0.2, 1), (20.0, 100), (199.99, 999), (200.0, 1000), (250.0, 1050), (1000.0, 1800), (2000.0, 1800)] {
            assert_eq!(render_bucket(distance), expected, "distance {distance}");
        }
        assert_eq!(bucket_order(1, 100, 1, render_bucket(20.0)), std::cmp::Ordering::Equal);
        assert!(bucket_order(3, 100, 3, 101).is_lt());
        assert!(bucket_order(6, 100, 6, 101).is_gt());
        assert!(bucket_order(3, 1800, 6, 0).is_lt());
    }

    #[test]
    fn highlight_root_suppression_keeps_mount_and_shield_priority_precedes_particles() {
        use ao_scene::{Mesh, Submesh, Vertex};
        let quad = |color: [f32; 3]| Mesh {
            vertices: [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].into_iter()
                .map(|(x, y)| Vertex { pos: [x, y, 0.0], normal: [0.0, 0.0, 1.0], ..Default::default() }).collect(),
            submeshes: vec![Submesh { two_sided: true, blend: Blend::AlphaBlend,
                base_color: [color[0], color[1], color[2], 0.5], emissive: [1.0; 3],
                ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
        };
        let model = Scene { meshes: vec![quad([1.0, 0.0, 0.0]), quad([0.0, 0.0, 1.0])], ..Default::default() };
        let frame = |id, priorities: Vec<Option<i32>>, bucket| {
            let mut f = ActorFrame { id, model: 7, part_priorities: priorities, render_bucket: bucket, ..Default::default() };
            f.transform[3][2] = -3.0;
            f
        };
        // Isolate actor ordering from the default blue sky blended behind translucent mounts.
        let world = Scene { environment: Some(ao_scene::Environment {
            sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 100.0, fog_end: 200.0,
            ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 0.0,
        }), ..Default::default() };
        let shot = |frames: Vec<ActorFrame>, name: &str| {
            let path = std::env::temp_dir().join(format!("native-priority-{}-{name}.png", std::process::id()));
            render_to_png_actors(&world, &[(7, model.clone())], frames,
                [0.0; 3], [0.0, 0.0, -1.0], 64, 64, &path, 0.0)
                .expect("native priority regression requires an offscreen GPU adapter");
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
            let mut pixels = vec![0; decoder.output_buffer_size()];
            let info = decoder.next_frame(&mut pixels).unwrap();
            let center = (info.width as usize * (info.height as usize / 2) + info.width as usize / 2) * 4;
            [pixels[center], pixels[center + 2]]
        };
        let root_hidden = shot(vec![frame(1, vec![Some(-1)], None)], "highlight2011");
        assert!(root_hidden[0] < 3 && root_hidden[1] > 100, "mounted visual survives root suppression: {root_hidden:?}");
        let shield_first = shot(vec![frame(1, vec![Some(3), Some(6)], None)], "shield3034-particle3024");
        let shield_last = shot(vec![frame(1, vec![Some(6), Some(3)], None)], "reversed");
        assert!(shield_first[1] > shield_first[0], "priority6 follows priority3: {shield_first:?}");
        assert!(shield_last[0] > shield_last[1], "priority overrides submission order: {shield_last:?}");
        // Native3015 explicit bucket100 must compare to world buckets, not metres.
        let blue_world = || frame(2, vec![Some(-1), Some(6)], None); // world origin3m => bucket15
        let red = |bucket| frame(1, vec![Some(6), Some(-1)], Some(bucket));
        let explicit_far = shot(vec![red(100), blue_world()], "explicit100");
        let explicit_near = shot(vec![red(0), blue_world()], "explicit0");
        assert!(explicit_far[1] > explicit_far[0], "world bucket15 follows explicit100 in list6: {explicit_far:?}");
        assert!(explicit_near[0] > explicit_near[1], "explicit0 follows world bucket15 in list6: {explicit_near:?}");
        let tied = shot(vec![red(15), blue_world()], "equal-bucket-prepend");
        assert!(tied[0] > tied[1], "later insertion draws first within a bucket: {tied:?}");
    }

    #[test]
    fn model_removal_keeps_unrelated_actors_and_is_idempotent() {
        let mut layer = ActorLayer::default();
        for (id, model) in [(1, 7), (2, 8), (3, 7)] {
            layer.models.entry(model).or_insert_with(|| Model { meshes: vec![], mats: vec![], material_bindings: vec![], holo: None, views: vec![], view_of: HashMap::new(), textures: vec![], identities: HashMap::new(), topology: vec![], native_skin: None, point_cache_revision: 0 });
            layer.actors.insert(id, ActorGpu { vb: None, seen: 1, model, skin: None, skin_bounds: None, pose: None, smoothing: None });
            layer.frames.push(ActorFrame { id, model, skin: Some(std::sync::Arc::new(vec![ao_scene::Vertex::default()])), ..Default::default() });
            layer.items.push(Item { actor: id, model, mesh: 0, fog_depth: 0.0, bucket: 0, faded: false, rendering_effect: 0, priority: None, native_shadow_source: None, native_shadow_receiver: None, water_body: None, swimming: false, water_surface: None, smoothing: false });
        }
        for key in [7, 7, 99] {
            layer.remove_model(key);
            assert_eq!(layer.models.len(), 1);
            assert!(layer.models.contains_key(&8));
            assert_eq!(layer.actors.len(), 1);
            assert_eq!(layer.actors[&2].model, 8);
            assert_eq!(layer.frames.len(), 1);
            assert_eq!(layer.frames[0].id, 2);
            assert!(layer.frames[0].skin.is_some());
            assert_eq!(layer.items.len(), 1);
            assert_eq!(layer.items[0].actor, 2);
        }
    }

    #[test]
    fn env_layer_follows_the_phase_of_its_submesh() {
        let (_, actor_pipes, _) = build_pipelines();
        for (blend, blended) in [(Blend::Opaque, false), (Blend::AlphaTest, false), (Blend::AlphaBlend, true), (Blend::Additive, true), (Blend::ZeroSourceColor, true), (Blend::DestinationColorSourceColor, true), (Blend::PremultipliedAlpha, true), (Blend::SourceColorOne, true)] {
            for two in [false, true] {
                for depth_func in [None, Some(2), Some(4), Some(8)] {
                    for write in [false, true] {
                        let sub = ao_scene::Submesh { blend, two_sided: two, depth_func, depth_write: Some(write), ..ao_scene::Submesh::new(vec![], None) };
                        assert_eq!(blended_pipe(material_pipe(&sub)), blended);
                        assert_eq!(blended_pipe(env_pipe(&sub)), blended, "{blend:?}/{depth_func:?}/{write}");
                        assert_eq!(faded_pipe(env_pipe(&sub)), env_pipe(&sub), "fading does not replace the env blob's blend/depth");
                        for effect in [0, 2] {
                            let spec = actor_pipes.spec(mesh_effect_pipe(env_pipe(&sub), effect, sub.projection_pass));
                            assert!(spec.env);
                            assert_eq!(spec.flip_depth, write, "env ZWRITE must inherit the base material");
                            let compare = match depth_func { Some(2) => wgpu::CompareFunction::Less, Some(8) => wgpu::CompareFunction::Always, _ => wgpu::CompareFunction::LessEqual };
                            assert_eq!(spec.compare.unwrap_or(wgpu::CompareFunction::LessEqual), compare);
                            let base = actor_pipes.spec(mesh_effect_pipe(material_pipe(&sub), effect, sub.projection_pass));
                            let base_write = matches!(base.blend, Blend::Opaque | Blend::AlphaTest) ^ base.flip_depth;
                            assert_eq!(base_write, write, "effect {effect} base must retain authored ZWRITE");
                            assert_eq!(base.compare.unwrap_or(wgpu::CompareFunction::LessEqual), compare);
                        }
                    }
                }
            }
        }
        let sub = ao_scene::Submesh::new(vec![], None);
        assert_eq!(env_pipe(&ao_scene::Submesh { two_sided: true, ..sub.clone() }) - env_pipe(&sub), 1, "two-sided pipeline follows the one-sided one");
    }
    #[test]
    fn sprite_pipelines_are_blended_without_changing_existing_indices() {
        for two_sided in [false, true] {
            let mut s = ao_scene::Submesh::new(vec![], None);
            s.blend = Blend::AlphaBlend;
            s.two_sided = two_sided;
            assert_eq!(material_pipe(&s), 4 + two_sided as usize);
            s.sprite_alpha_test = true;
            assert_eq!(material_pipe(&s), SPRITE_PIPE + two_sided as usize);
            assert!(blended_pipe(material_pipe(&s)));
        }
    }
    #[test]
    fn native_effect_factors_match_display_system_states() {
        use wgpu::BlendFactor::*;
        // Sprite3 0x100283ec: raw D3DBLEND 1/3 and 9/3; Cylinder 0x100109a4: 2/6; HorizonCloudsRim (Tweak_Rubi-Ka_HorizonClouds.txt) 3/2.
        for (blend, src, dst, pipe) in [
            (Blend::ZeroSourceColor, Zero, Src, 22),
            (Blend::DestinationColorSourceColor, Dst, Src, 24),
            (Blend::PremultipliedAlpha, One, OneMinusSrcAlpha, 26),
            (Blend::SourceColorOne, Src, One, 28),
        ] {
            let state = native_blend_state(blend);
            assert_eq!((state.color.src_factor, state.color.dst_factor), (src, dst));
            assert_eq!(state.alpha, state.color, "D3D7 uses the same factors for alpha");
            assert_eq!(state.color.operation, wgpu::BlendOperation::Add);
            for two_sided in [false, true] {
                let sub = ao_scene::Submesh { blend, two_sided, ..ao_scene::Submesh::new(vec![], None) };
                assert_eq!(material_pipe(&sub), pipe + two_sided as usize);
                assert!(blended_pipe(material_pipe(&sub)));
            }
        }
    }

    /// Leak regression: a steady population of short-lived actors sharing one model (a footstep/swish/impact effect
    /// quad, or a lightning fan/bolt segment -- every one of them spawns a brand new `ActorFrame::id` each time) must
    /// not keep creating new GPU skin buffers forever. Simulates 60 "dynels" each retiring and respawning one effect
    /// actor (a fresh id) every frame: confirmed live at ICC, this exact pattern created 18k+ outstanding Metal
    /// buffers (623 MB resident) in ~13 unthrottled seconds before pooling.
    #[test]
    fn short_lived_actor_churn_reuses_pooled_buffers_after_warmup() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("resource regression requires GPU");
        let model = 0xfac2_0000_0000_0000u64;
        let mut scene = Scene::default();
        scene.meshes.push(ao_scene::Mesh { vertices: vec![ao_scene::Vertex::default(); 4], submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2, 1, 3, 2], None)] });
        renderer.add_actor_model(model, &scene);
        let skin = || std::sync::Arc::new(vec![ao_scene::Vertex::default(); 4]);
        let mut next_id = 0u32;
        for frame in 0..600 {
            let actors = (0..60)
                .map(|_| { next_id += 1; ActorFrame { id: next_id, model, transform: ao_scene::IDENTITY, skin: Some(skin()), always: true, ..Default::default() } })
                .collect();
            renderer.set_actors(actors);
            if frame == 119 {
                // one pass of 60 actors each frame for 2 "seconds" (120 frames) must have created at most their peak
                // concurrent count worth of buffers, not kept growing with the cumulative spawn count (7200 by now).
                assert!(renderer.actor_vb_creations() <= 120, "warm-up created {} buffers, pooling isn't reusing them", renderer.actor_vb_creations());
            }
        }
        let warm = renderer.actor_vb_creations();
        for _ in 0..600 {
            let actors = (0..60)
                .map(|_| { next_id += 1; ActorFrame { id: next_id, model, transform: ao_scene::IDENTITY, skin: Some(skin()), always: true, ..Default::default() } })
                .collect();
            renderer.set_actors(actors);
        }
        assert_eq!(renderer.actor_vb_creations(), warm, "steady-state churn of {} more spawns created {} more buffers instead of reusing the pool", 600 * 60, renderer.actor_vb_creations() - warm);
        assert!(!renderer.act.vb_pool.is_empty(), "churn must exercise a retained buffer pool");
        renderer.clear_actors();
        assert!(renderer.act.vb_pool.is_empty(), "a new zone must release pools belonging to removed models");
    }
}
