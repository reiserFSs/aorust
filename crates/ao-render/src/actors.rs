//! Dynamic actors (players, NPCs, ...) drawn after the world: the model's index buffers, materials and textures are uploaded once
//! per model key ([`Renderer::add_actor_model`]); the body mesh gets one vertex buffer per actor, rewritten when the app supplies a
//! new pose ([`ao_scene::ActorFrame::skin`]); rigid parts (head, weapons) share the model's vertex buffer and only get a transform.

use super::*;
use ao_scene::ActorFrame;

/// Extra radius around the bind-pose sphere of mesh 0: limbs reach out of it in animation.
const POSE_MARGIN: f32 = 1.0;

/// Affine transform (including the existing alpha lane), then independently enabled material overrides.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ActorInstance {
    transform: [[f32; 4]; 4],
    emissive: [f32; 4],
    specular: [f32; 4],
    power: [f32; 4],
    uv: [f32; 4],
}

struct ModelMesh {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    nverts: usize,
    draws: Vec<Draw>,
    /// Bind-pose bounding sphere.
    sphere: (Vec3, f32),
}

struct Model {
    meshes: Vec<Option<ModelMesh>>,
    mats: Vec<wgpu::BindGroup>,
    holo: Option<wgpu::BindGroup>,
    views: Vec<wgpu::TextureView>,
    view_of: HashMap<TextureKey, usize>,
    topology: Vec<(usize, Vec<ao_scene::Submesh>)>,
    vertex_scratch: Vec<f32>,
}

struct ActorGpu {
    /// Own body vertex buffer, created at the first pose.
    vb: Option<wgpu::Buffer>,
    seen: u64,
    /// Model key of the last submitted frame (a replaced model only invalidates its own actors' buffers).
    model: u64,
}

/// One visible (actor, model mesh) with its slot in the instance buffer.
struct Item {
    actor: u32,
    model: u64,
    mesh: usize,
    bucket: i32,
    /// `ActorFrame::alpha < 1`: opaque / alpha-tested draws use the [`FADE_PIPE`] variants.
    faded: bool,
    rendering_effect: u32,
    priority: Option<i32>,
}

#[derive(Default)]
pub(crate) struct ActorLayer {
    models: HashMap<u64, Model>,
    actors: HashMap<u32, ActorGpu>,
    frames: Vec<ActorFrame>,
    tick: u64,
    bufs: Vec<wgpu::Buffer>,
    cap: usize,
    items: Vec<Item>,
    commands: Vec<(i32, Option<usize>, i32, bool, usize)>,
}

impl ActorLayer {
    fn remove_model(&mut self, key: u64) {
        self.models.remove(&key);
        self.actors.retain(|_, actor| actor.model != key);
        self.frames.retain(|frame| frame.model != key);
        self.items.retain(|item| item.model != key);
        // Instance buffers are shared scratch space, rebuilt from frames before each draw.
    }
}

impl Renderer {
    /// Uploads a model for [`ActorFrame::model`] `key`: `scene.meshes[0]` is the (skinned) body, the others are rigid parts.
    /// Replaces a model of the same key.
    pub fn add_actor_model(&mut self, key: u64, scene: &Scene) {
        self.upload_actor_model(key, scene, false);
    }

    /// Replaces dynamic geometry while retaining the model's already uploaded textures.
    /// Any newly referenced texture must be supplied in `scene.textures`.
    pub fn update_actor_model(&mut self, key: u64, scene: &Scene) {
        if let Some(model) = self.act.models.get_mut(&key) {
            if scene.textures.keys().all(|key| model.view_of.contains_key(key))
                && model.topology.len() == scene.meshes.len()
                && model.topology.iter().zip(&scene.meshes).all(|((count, subs), mesh)| *count == mesh.vertices.len() && *subs == mesh.submeshes)
            {
                for (gpu, mesh) in model.meshes.iter_mut().zip(&scene.meshes) {
                    let Some(gpu) = gpu else { continue };
                    model.vertex_scratch.clear();
                    for vertex in &mesh.vertices {
                        model.vertex_scratch.extend_from_slice(&[
                            vertex.pos[0], vertex.pos[1], vertex.pos[2],
                            vertex.normal[0], vertex.normal[1], vertex.normal[2],
                            vertex.uv[0], vertex.uv[1],
                            vertex.color[0], vertex.color[1], vertex.color[2], vertex.color[3],
                        ]);
                    }
                    self.queue.write_buffer(&gpu.vb, 0, bytemuck::cast_slice(&model.vertex_scratch));
                    let (lo, hi) = mesh.vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), v| (l.min(Vec3::from(v.pos)), h.max(Vec3::from(v.pos))));
                    let center = (lo + hi) * 0.5;
                    gpu.sphere = (center, mesh.vertices.iter().map(|v| (Vec3::from(v.pos) - center).length()).fold(0.0, f32::max));
                }
                return;
            }
        }
        self.upload_actor_model(key, scene, true);
    }

    fn upload_actor_model(&mut self, key: u64, scene: &Scene, retain_textures: bool) {
        let old = if retain_textures { self.act.models.remove(&key) } else { None };
        let (mut views, mut view_of) = old.map_or_else(
            || (vec![self.texture_view(&[255; 4], 1, 1)], HashMap::new()),
            |model| (model.views, model.view_of),
        );
        for (k, t) in &scene.textures {
            if view_of.contains_key(k) { continue; }
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize {
                view_of.insert(*k, views.len());
                views.push(self.texture_view(&t.rgba, t.width, t.height));
            }
        }
        let holo = [0x28022, 0x28023].map(|id| view_of.iter().find(|(key, _)| key.id == id).map(|(_, &view)| view));
        let holo = match holo {
            [Some(a), Some(b)] => Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native Holo textures"), layout: &self.holo_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[a]) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[b]) },
                ],
            })),
            _ => None,
        };
        let mut mat_of: HashMap<(usize, bool, [u32; 20]), usize> = HashMap::new();
        let mut mats: Vec<wgpu::BindGroup> = vec![];
        let mut meshes = vec![];
        for mesh in &scene.meshes {
            let idx_total: usize = mesh.submeshes.iter().map(|s| s.indices.len()).sum();
            if mesh.vertices.is_empty() || idx_total == 0 {
                meshes.push(None);
                continue;
            }
            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&vertex_bytes(&mesh.vertices)),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let (mut indices, mut draws) = (Vec::with_capacity(idx_total), vec![]);
            let nv = mesh.vertices.len() as u32;
            for s in &mesh.submeshes {
                let first = indices.len() as u32;
                for t in s.indices.as_chunks::<3>().0 {
                    if t.iter().all(|&i| i < nv) {
                        indices.extend_from_slice(t.as_slice());
                    }
                }
                let count = indices.len() as u32 - first;
                if count == 0 {
                    continue;
                }
                let view = s.texture.and_then(|k| view_of.get(&k).copied()).unwrap_or(0);
                let mut bind = |view: usize, clamp: bool, u: [f32; 20]| {
                    *mat_of.entry((view, clamp, u.map(f32::to_bits))).or_insert_with(|| {
                        let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: None,
                            contents: bytemuck::bytes_of(&u),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });
                        mats.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: None,
                            layout: &self.tex_layout,
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[view]) },
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(self.material_sampler(clamp)) },
                                wgpu::BindGroupEntry { binding: 2, resource: ub.as_entire_binding() },
                            ],
                        }));
                        mats.len() - 1
                    })
                };
                let mat = bind(view, s.texture_clamp, mat_uniform(s));
                draws.push(Draw { mesh: meshes.len(), first_index: first, count, mat, pipe: material_pipe(s) });
                // the env layer redraws the same triangles right after them, in the same phase (`FUN_10056ed6`)
                if let Some(ev) = s.env_texture.and_then(|k| view_of.get(&k).copied()) {
                    let mat = bind(ev, false, [0.0; 20]);
                    draws.push(Draw { mesh: meshes.len(), first_index: first, count, mat, pipe: env_pipe(s.blend, s.two_sided) });
                }
            }
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let (lo, hi) = mesh.vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), v| (l.min(Vec3::from(v.pos)), h.max(Vec3::from(v.pos))));
            let c = (lo + hi) * 0.5;
            let r = mesh.vertices.iter().map(|v| (Vec3::from(v.pos) - c).length()).fold(0.0, f32::max);
            meshes.push(Some(ModelMesh { vb, ib, nverts: mesh.vertices.len(), draws, sphere: (c, r) }));
        }
        let topology = scene.meshes.iter().map(|mesh| (mesh.vertices.len(), mesh.submeshes.clone())).collect();
        let vertex_scratch = Vec::with_capacity(scene.meshes.iter().map(|mesh| mesh.vertices.len() * 12).max().unwrap_or(0));
        self.act.models.insert(key, Model { meshes, mats, holo, views, view_of, topology, vertex_scratch });
        // actors of a replaced model rebuild their body buffer at the next pose
        for a in self.act.actors.values_mut().filter(|a| a.model == key) {
            a.vb = None;
        }
    }

    /// Retires one model and its actors' skin buffers, submitted frames and draw items.
    /// Other models and actors remain available; missing keys are harmless.
    pub fn remove_actor_model(&mut self, key: u64) {
        self.act.remove_model(key);
    }

    /// Forgets every model and actor (a new zone).
    pub fn clear_actors(&mut self) {
        self.act.models.clear();
        self.act.actors.clear();
        self.act.frames.clear();
    }

    /// The actors to draw from now on (until the next call); new body poses are uploaded immediately.
    pub fn set_actors(&mut self, mut frames: Vec<ActorFrame>) {
        self.act.tick += 1;
        let tick = self.act.tick;
        for f in &mut frames {
            let Some(body) = self.act.models.get(&f.model).and_then(|m| m.meshes.first()).and_then(Option::as_ref) else { continue };
            let a = self.act.actors.entry(f.id).or_insert(ActorGpu { vb: None, seen: tick, model: f.model });
            a.seen = tick;
            a.model = f.model;
            if let Some(skin) = f.skin.take().filter(|s| s.len() == body.nverts) {
                let bytes = bytemuck::cast_slice(&vertex_bytes(&skin)).to_vec();
                match &a.vb {
                    Some(vb) => self.queue.write_buffer(vb, 0, &bytes),
                    None => {
                        a.vb = Some(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: None,
                            contents: &bytes,
                            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        }));
                    }
                }
            }
        }
        self.act.actors.retain(|_, a| a.seen == tick);
        self.act.frames = frames;
    }

    /// Frustum-culls the submitted actors, fills the instance buffer; called by `render` before the pass.
    pub(crate) fn prepare_actors(&mut self, cam: Vec3, planes: &[Vec4; 6]) {
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
                if !f.always && !planes.iter().all(|p| p.dot(c.extend(1.0)) >= -r) {
                    continue;
                }
                let bucket = f.render_bucket.filter(|b| (0..=1800).contains(b))
                    .unwrap_or_else(|| render_bucket(m.w_axis.truncate().distance(cam)));
                layer.items.push(Item { actor: f.id, model: f.model, mesh: mi, bucket, faded: alpha < 1.0, rendering_effect: f.rendering_effect, priority });
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
                });
            }
        }
        if xf.len() > layer.cap || layer.bufs.is_empty() {
            layer.cap = xf.len().next_power_of_two().max(64);
            layer.bufs = (0..INST_RING)
                .map(|_| {
                    self.device.create_buffer(&wgpu::BufferDescriptor {
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
        layer.commands.clear();
        for (index,item) in layer.items.iter().enumerate() {
            let model=&layer.models[&item.model];
            let Some(mesh)=&model.meshes[item.mesh] else {continue};
            for priority in 0..=10 {
                if mesh.draws.iter().any(|d|actor_priority(item.priority,item.faded,item.rendering_effect,blended_pipe(d.pipe))==priority) {
                    layer.commands.push((priority, Some(index), item.bucket, item.rendering_effect == 3, layer.commands.len()));
                }
            }
        }
        for clear in &self.screen.depth_clears {
            layer.commands.push((clear.priority, None, render_bucket(Vec3::from(clear.position).distance(cam)), true, layer.commands.len()));
        }
        // AddToRenderList prepends: later submitted visuals lead equal-distance buckets.
        layer.commands.sort_unstable_by(|a, b| bucket_order(a.0, a.2, b.0, b.2).then_with(|| b.4.cmp(&a.4)));
    }

    /// Draws a native render list without changing its material pipelines.
    pub(crate) fn draw_actors(&self,enc:&mut wgpu::CommandEncoder,t:&Targets,resolve:&wgpu::TextureView,priority:i32)->usize {
        let layer=&self.act;
        let buf=layer.bufs.get(self.frame);
        let mut calls=0;
        let mut one=|pass:&mut wgpu::RenderPass,i:usize| {
            let Some(buf)=buf else {return};
            let it = &layer.items[i];
            let model = &layer.models[&it.model];
            let Some(mesh) = model.meshes[it.mesh].as_ref() else { return };
            let own = (it.mesh == 0).then(|| layer.actors.get(&it.actor)?.vb.as_ref()).flatten();
            let mut bound = false;
            for d in &mesh.draws {
                let effect = it.rendering_effect;
                if effect == 1 && model.holo.is_none() { continue; }
                if actor_priority(it.priority, it.faded, effect, blended_pipe(d.pipe)) != priority {
                    continue;
                }
                if !bound {
                    pass.set_vertex_buffer(0, own.unwrap_or(&mesh.vb).slice(..));
                    let stride = std::mem::size_of::<ActorInstance>() as u64;
                    pass.set_vertex_buffer(1, buf.slice(i as u64 * stride..(i as u64 + 1) * stride));
                    pass.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                    bound = true;
                }
                let pipe = if it.faded && d.pipe < SKY_PIPE && d.pipe / 2 < 2 { FADE_PIPE + d.pipe } else { d.pipe };
                let pipe = mesh_effect_pipe(pipe, effect);
                pass.set_pipeline(&self.actor_pipes[pipe]);
                pass.set_bind_group(1, &model.mats[d.mat], &[]);
                if effect == 1 {
                    pass.set_bind_group(2, model.holo.as_ref().unwrap(), &[]);
                }
                pass.draw_indexed(d.first_index..d.first_index + d.count, 0, 0..1);
                calls += 1;
            }
        };
        let mut start=layer.commands.partition_point(|command|command.0<priority);
        let limit=layer.commands.partition_point(|command|command.0<=priority);
        while start<limit {
            let end=(start+1..limit).find(|&i|layer.commands[i].3).unwrap_or(limit);
            let mut pass=super::screen::world_pass(enc,t,resolve,layer.commands[start].3);
            pass.set_bind_group(0,&self.globals_bg,&[]);
            for command in &layer.commands[start..end] {
                if let Some(index)=command.1 {one(&mut pass,index);}
            }
            start=end;
        }
        calls
    }
}

/// Pipeline of the env layer of a submesh that draws with `blend`: the env pipelines live behind the scene/sky ones and come in
/// an opaque-phase and a blended-phase set (same pipeline, different phase index).
fn env_pipe(blend: Blend, two_sided: bool) -> usize {
    (if blended_material(blend) { ENV_BLEND_PIPE } else { ENV_PIPE }) + two_sided as usize
}

/// Whether a draw belongs to the blended actor phase (client render list 6) rather than the opaque one (list 3).
fn blended_pipe(pipe: usize) -> bool {
    matches!(pipe / 2, 2 | 3) && pipe < SKY_PIPE || (ENV_BLEND_PIPE..ENV_BLEND_PIPE + 2).contains(&pipe) || (SPRITE_PIPE..NATIVE_BLEND_PIPE + 6).contains(&pipe)
}

/// Each delta state overrides only the native states it owns; mode 3 clears depth before its draw.
fn mesh_effect_pipe(pipe: usize, effect: u32) -> usize {
    match effect {
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
fn render_bucket(distance: f32) -> i32 {
    let (value, offset) = if distance < 200.0 { ((distance * 5.0) as f64, 0) } else { (distance as f64, 800) };
    ((value - 0.49999).round_ties_even() as i32).saturating_add(offset).clamp(0, 1800)
}

fn bucket_order(priority_a: i32, bucket_a: i32, priority_b: i32, bucket_b: i32) -> std::cmp::Ordering {
    priority_a.cmp(&priority_b).then_with(|| if matches!(priority_a, 5 | 6) {
        bucket_b.cmp(&bucket_a)
    } else {
        bucket_a.cmp(&bucket_b)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn mesh_delta_state_pipeline_modes_preserve_material_states() {
        for pipe in 0..MESH_EFFECT_STRIDE {
            assert_eq!(mesh_effect_pipe(pipe, 0), pipe);
            assert_eq!(mesh_effect_pipe(pipe, 3), pipe);
            for effect in [1, 2, 4, 5, 6, 7] {
                assert_eq!(mesh_effect_pipe(pipe, effect), effect as usize * MESH_EFFECT_STRIDE + pipe);
            }
        }
    }
    use super::*;
    #[test]
    fn dynamic_geometry_update_retains_uploaded_texture_views() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("resource regression requires GPU");
        let key = TextureKey { rdb_type: 1010004, id: 1 };
        let mut scene = Scene::default();
        scene.textures.insert(key, ao_scene::Texture { width: 1, height: 1, rgba: vec![255; 4] });
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
        // Equal index counts are insufficient: actual connectivity must trigger a rebuild.
        scene.meshes[0].submeshes[0].indices.swap(1, 2);
        renderer.update_actor_model(7, &scene);
        let mesh = renderer.act.models[&7].meshes[0].as_ref().unwrap();
        assert_ne!(mesh.ib, ib);
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
            layer.models.entry(model).or_insert_with(|| Model { meshes: vec![], mats: vec![], holo: None, views: vec![], view_of: HashMap::new(), topology: vec![], vertex_scratch: vec![] });
            layer.actors.insert(id, ActorGpu { vb: None, seen: 1, model });
            layer.frames.push(ActorFrame { id, model, skin: Some(vec![ao_scene::Vertex::default()]), ..Default::default() });
            layer.items.push(Item { actor: id, model, mesh: 0, bucket: 0, faded: false, rendering_effect: 0, priority: None });
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
        for (blend, blended) in [(Blend::Opaque, false), (Blend::AlphaTest, false), (Blend::AlphaBlend, true), (Blend::Additive, true), (Blend::ZeroSourceColor, true), (Blend::DestinationColorSourceColor, true), (Blend::PremultipliedAlpha, true)] {
            for two in [false, true] {
                let base = material_pipe(&ao_scene::Submesh { blend, two_sided: two, ..ao_scene::Submesh::new(vec![], None) });
                assert_eq!(blended_pipe(base), blended);
                assert_eq!(blended_pipe(env_pipe(blend, two)), blended, "{blend:?}");
                assert!(env_pipe(blend, two) >= ENV_PIPE && env_pipe(blend, two) < ENV_PIPE + 4);
            }
        }
        assert_eq!(env_pipe(Blend::Opaque, true) - env_pipe(Blend::Opaque, false), 1, "two-sided pipeline follows the one-sided one");
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
        // Sprite3 0x100283ec: raw D3DBLEND 1/3 and 9/3; Cylinder 0x100109a4: 2/6.
        for (blend, src, dst, pipe) in [
            (Blend::ZeroSourceColor, Zero, Src, 22),
            (Blend::DestinationColorSourceColor, Dst, Src, 24),
            (Blend::PremultipliedAlpha, One, OneMinusSrcAlpha, 26),
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
}
