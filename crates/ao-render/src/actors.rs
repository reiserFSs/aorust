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
    dist: f32,
    /// `ActorFrame::alpha < 1`: opaque / alpha-tested draws use the [`FADE_PIPE`] variants.
    faded: bool,
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
                let mut bind = |view: usize, u: [f32; 20]| {
                    *mat_of.entry((view, u.map(f32::to_bits))).or_insert_with(|| {
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
                    })
                };
                let mat = bind(view, mat_uniform(s));
                draws.push(Draw { mesh: meshes.len(), first_index: first, count, mat, pipe: material_pipe(s) });
                // the env layer redraws the same triangles right after them, in the same phase (`FUN_10056ed6`)
                if let Some(ev) = s.env_texture.and_then(|k| view_of.get(&k).copied()) {
                    let mat = bind(ev, [0.0; 20]);
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
        self.act.models.insert(key, Model { meshes, mats });
        // actors of a replaced model rebuild their body buffer at the next pose
        for a in self.act.actors.values_mut().filter(|a| a.model == key) {
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
                let m = f.parts.get(mi).map_or(base, |p| base * Mat4::from_cols_array_2d(p));
                let margin = if mi == 0 { POSE_MARGIN } else { 0.0 };
                let c = m.transform_point3(mesh.sphere.0);
                let r = (mesh.sphere.1 + margin) * scale;
                if !f.always && !planes.iter().all(|p| p.dot(c.extend(1.0)) >= -r) {
                    continue;
                }
                layer.items.push(Item { actor: f.id, model: f.model, mesh: mi, dist: (c - cam).length(), faded: f.alpha < 1.0 });
                // m0.w carries `alpha - 1` to the shader (the affine matrix' unused column element)
                let mut a = m.to_cols_array_2d();
                a[0][3] = f.alpha.min(1.0) - 1.0;
                let color = |value: Option<[f32; 3]>| {
                    let rgb = value.unwrap_or([0.0; 3]);
                    [rgb[0], rgb[1], rgb[2], if value.is_some() { 1.0 } else { 0.0 }]
                };
                xf.push(ActorInstance {
                    transform: a,
                    emissive: color(f.emissive),
                    specular: color(f.specular),
                    power: [f.specular_power.unwrap_or(0.0), if f.specular_power.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0],
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
    }

    /// Draws one phase of the prepared actors into the world pass: the opaque/cutout parts (`blended == false`, the client's render
    /// list 3, drawn with the world's opaque meshes before the liquids) or the blended parts far to near (list 6, after the liquids
    /// and the blended world).
    pub(crate) fn draw_actors<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, blended: bool) -> usize {
        let layer = &self.act;
        let Some(buf) = layer.bufs.get(self.frame).filter(|_| !layer.items.is_empty()) else { return 0 };
        let mut calls = 0;
        let mut order: Vec<usize> = (0..layer.items.len()).collect();
        let mut one = |pass: &mut wgpu::RenderPass<'a>, i: usize| {
            let it = &layer.items[i];
            let model = &layer.models[&it.model];
            let Some(mesh) = model.meshes[it.mesh].as_ref() else { return };
            let own = (it.mesh == 0).then(|| layer.actors.get(&it.actor)?.vb.as_ref()).flatten();
            let mut bound = false;
            for d in &mesh.draws {
                if blended_pipe(d.pipe) != blended {
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
                pass.set_pipeline(&self.actor_pipes[pipe]);
                pass.set_bind_group(1, &model.mats[d.mat], &[]);
                pass.draw_indexed(d.first_index..d.first_index + d.count, 0, 0..1);
                calls += 1;
            }
        };
        if blended {
            order.sort_by(|&a, &b| layer.items[b].dist.total_cmp(&layer.items[a].dist));
        }
        for i in order {
            one(pass, i);
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

#[cfg(test)]
mod tests {
    use super::*;

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
