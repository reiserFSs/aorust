//! Dynamic actors (players, NPCs, ...) drawn after the world: the model's index buffers, materials and textures are uploaded once
//! per model key ([`Renderer::add_actor_model`]); the body mesh gets one vertex buffer per actor, rewritten when the app supplies a
//! new pose ([`ao_scene::ActorFrame::skin`]); rigid parts (head, weapons) share the model's vertex buffer and only get a transform.

use super::*;
use ao_scene::ActorFrame;

/// Extra radius around the bind-pose sphere of mesh 0: limbs reach out of it in animation.
const POSE_MARGIN: f32 = 1.0;

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
}

struct ActorGpu {
    /// Own body vertex buffer, created at the first pose.
    vb: Option<wgpu::Buffer>,
    seen: u64,
}

/// One visible (actor, model mesh) with its slot in the instance buffer.
struct Item {
    actor: u32,
    model: u64,
    mesh: usize,
    dist: f32,
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
}

impl Renderer {
    /// Uploads a model for [`ActorFrame::model`] `key`: `scene.meshes[0]` is the (skinned) body, the others are rigid parts.
    /// Replaces a model of the same key.
    pub fn add_actor_model(&mut self, key: u64, scene: &Scene) {
        let mut views = vec![self.texture_view(&[255; 4], 1, 1)];
        let mut view_of: HashMap<TextureKey, usize> = HashMap::new();
        for (k, t) in &scene.textures {
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize {
                view_of.insert(*k, views.len());
                views.push(self.texture_view(&t.rgba, t.width, t.height));
            }
        }
        let mut mat_of: HashMap<(usize, [u32; 20]), usize> = HashMap::new();
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
                let u = mat_uniform(s);
                let mat = *mat_of.entry((view, u.map(f32::to_bits))).or_insert_with(|| {
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
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            wgpu::BindGroupEntry { binding: 2, resource: ub.as_entire_binding() },
                        ],
                    }));
                    mats.len() - 1
                });
                draws.push(Draw { mesh: meshes.len(), first_index: first, count, mat, pipe: s.blend as usize * 2 + s.two_sided as usize });
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
        self.act.models.insert(key, Model { meshes, mats });
        // actors of a replaced model rebuild their body buffer at the next pose
        for a in self.act.actors.values_mut() {
            a.vb = None;
        }
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
            let a = self.act.actors.entry(f.id).or_insert(ActorGpu { vb: None, seen: tick });
            a.seen = tick;
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
        let mut xf: Vec<[[f32; 4]; 4]> = vec![];
        for f in &layer.frames {
            let Some(model) = layer.models.get(&f.model) else { continue };
            let base = Mat4::from_cols_array_2d(&f.transform);
            let scale = base.x_axis.truncate().length().max(base.y_axis.truncate().length()).max(base.z_axis.truncate().length());
            for (mi, mesh) in model.meshes.iter().enumerate() {
                let Some(mesh) = mesh else { continue };
                let m = f.parts.get(mi).map_or(base, |p| base * Mat4::from_cols_array_2d(p));
                let margin = if mi == 0 { POSE_MARGIN } else { 0.0 };
                let c = m.transform_point3(mesh.sphere.0);
                let r = (mesh.sphere.1 + margin) * scale;
                if !f.always && !planes.iter().all(|p| p.dot(c.extend(1.0)) >= -r) {
                    continue;
                }
                layer.items.push(Item { actor: f.id, model: f.model, mesh: mi, dist: (c - cam).length() });
                xf.push(m.to_cols_array_2d());
            }
        }
        if xf.len() > layer.cap || layer.bufs.is_empty() {
            layer.cap = xf.len().next_power_of_two().max(64);
            layer.bufs = (0..INST_RING)
                .map(|_| {
                    self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("actor instances"),
                        size: (layer.cap * 64) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                })
                .collect();
        }
        if !xf.is_empty() {
            self.queue.write_buffer(&layer.bufs[self.frame], 0, bytemuck::cast_slice(&xf));
        }
    }

    /// Draws the prepared actors into the world pass: opaque/cutout first, then blended parts far to near.
    pub(crate) fn draw_actors<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> usize {
        let layer = &self.act;
        let Some(buf) = layer.bufs.get(self.frame).filter(|_| !layer.items.is_empty()) else { return 0 };
        let mut calls = 0;
        let mut order: Vec<usize> = (0..layer.items.len()).collect();
        let mut one = |pass: &mut wgpu::RenderPass<'a>, i: usize, blended: bool| {
            let it = &layer.items[i];
            let model = &layer.models[&it.model];
            let Some(mesh) = model.meshes[it.mesh].as_ref() else { return };
            let own = (it.mesh == 0).then(|| layer.actors.get(&it.actor)?.vb.as_ref()).flatten();
            let mut bound = false;
            for d in &mesh.draws {
                if matches!(d.pipe / 2, 2 | 3) != blended {
                    continue;
                }
                if !bound {
                    pass.set_vertex_buffer(0, own.unwrap_or(&mesh.vb).slice(..));
                    pass.set_vertex_buffer(1, buf.slice(i as u64 * 64..(i as u64 + 1) * 64));
                    pass.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                    bound = true;
                }
                pass.set_pipeline(&self.pipes[d.pipe]);
                pass.set_bind_group(1, &model.mats[d.mat], &[]);
                pass.draw_indexed(d.first_index..d.first_index + d.count, 0, 0..1);
                calls += 1;
            }
        };
        for &i in &order {
            one(pass, i, false);
        }
        order.sort_by(|&a, &b| layer.items[b].dist.total_cmp(&layer.items[a].dist));
        for i in order {
            one(pass, i, true);
        }
        calls
    }
}
