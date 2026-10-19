//! Enhanced v2 actor motion (DESIGN 1.2, G2): the admitted opaque / alpha-tested actor items are drawn again against the G-buffer
//! depth (hardware `Equal`, no writes) and write the displayed-pose screen motion into G2 for TAA. Adapted from `motion_vectors.rs`
//! (previous model matrix + previous displayed vertices per pose), single-sample and actors only: static geometry is reprojected from
//! depth by the TAA itself. Nested under `actors` (see `actors.rs`) to reuse the private actor layer exactly like the native draws.
//!
//! Fairness: poses exist only for items of the current frame's admitted actors (`retain` below), so a hidden actor keeps no history.
use super::*;
use crate::pipelines::Pipelines;
use std::cell::RefCell;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PoseKey(u32, u64, usize);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform { current_vp: [[f32; 4]; 4], previous_vp: [[f32; 4]; 4], previous_model: [[f32; 4]; 4], control: [u32; 4] }

struct Pose {
    transform: [[f32; 4]; 4],
    /// Last displayed vertices of a deforming / smoothed mesh (rigid meshes reuse the live buffer).
    vertices: Option<wgpu::Buffer>,
    bytes: u64,
    smoothing: bool,
    ready: bool,
    cut: bool,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
}

pub(crate) struct ActorMotion {
    layout: wgpu::BindGroupLayout,
    pipes: Pipelines,
    poses: HashMap<PoseKey, Pose>,
    current_vp: Mat4,
    previous_vp: Mat4,
    reset: bool,
}

/// A rigid actor that moved further than this (metres) in one frame teleported: its history is invalid.
fn transform_cut(previous: [[f32; 4]; 4], current: [[f32; 4]; 4]) -> bool {
    Vec3::from_slice(&previous[3][..3]).distance_squared(Vec3::from_slice(&current[3][..3])) > 64.0
}

impl ActorMotion {
    pub(crate) fn new(r: &Renderer) -> Self {
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("v2 actor motion pose"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let source = format!("{}\n{}", crate::shader_source::load("shader.wgsl", include_str!("../shader.wgsl")),
            crate::shader_source::load("v2/motion.wgsl", include_str!("motion.wgsl")));
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 actor motion"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("v2 actor motion"), bind_group_layouts: &[Some(&r.g_layout), Some(&r.tex_layout), Some(&layout)], immediate_size: 0 });
        let vertices = wgpu::vertex_attr_array![0=>Float32x3, 1=>Float32x3, 2=>Float32x2, 3=>Float32x4];
        let f4 = |offset| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset, shader_location: 4 + (offset / 16) as u32 };
        let instances = [f4(0), f4(16), f4(32), f4(48), f4(64), f4(80), f4(96), f4(112)];
        let previous = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 12 }];
        let (_, mut pipes, _) = build_pipelines();
        let cache: RefCell<HashMap<(bool, bool, bool), crate::pipeline_creation::RenderPipeline>> = Default::default();
        for slot in 0..pipes.len() {
            let d = pipes.spec(slot).descriptor();
            // The slots the G-buffer pass draws for actors (opaque / alpha-tested); blended actors go through the FX pass.
            if !matches!(d.fs, "fs_opaque" | "fs_test") || d.state.is_some() || d.holo { continue; }
            pipes.ensure(slot, &|spec| {
                let d = spec.descriptor();
                let test = d.fs == "fs_test";
                cache.borrow_mut().entry((test, d.cull, d.bias)).or_insert_with(|| crate::pipeline_creation::render_deferred(&r.device, &wgpu::RenderPipelineDescriptor {
                    label: Some("v2 actor motion"), layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_motion_actor"), compilation_options: Default::default(), buffers: &[
                        wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertices },
                        wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<ActorInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &instances },
                        wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &previous },
                    ] },
                    fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(if test { "fs_motion_test" } else { "fs_motion_opaque" }), compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState { format: crate::v2::G2_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                    primitive: wgpu::PrimitiveState { cull_mode: d.cull.then_some(wgpu::Face::Back), ..Default::default() },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Equal), stencil: Default::default(),
                        bias: if d.bias { wgpu::DepthBiasState { constant: -1, slope_scale: 0.0, clamp: 0.0 } } else { Default::default() },
                    }),
                    multisample: Default::default(), multiview_mask: None, cache: None,
                })).clone()
            });
        }
        pipes.wait();
        Self { layout, pipes, poses: HashMap::new(), current_vp: Mat4::IDENTITY, previous_vp: Mat4::IDENTITY, reset: true }
    }

    /// After `prepare_actors`: admits one pose per drawn actor item, drops every other. `vp` / `previous_vp` are unjittered.
    /// `reset` marks every pose history-less this frame (camera cut, resize, admission change).
    pub(crate) fn prepare(&mut self, r: &Renderer, vp: Mat4, previous_vp: Mat4, reset: bool) {
        self.current_vp = vp;
        self.previous_vp = previous_vp;
        self.reset = reset;
        let layer = &r.act;
        let admitted: std::collections::HashSet<PoseKey> = layer.items.iter().map(|item| PoseKey(item.actor, item.model, item.mesh)).collect();
        self.poses.retain(|key, _| admitted.contains(key));
        for (index, item) in layer.items.iter().enumerate() {
            let Some(mesh) = layer.models[&item.model].meshes[item.mesh].as_ref() else { continue };
            let deforming = item.mesh == 0 && layer.actors.get(&item.actor).is_some_and(|actor| actor.vb.is_some());
            let (_, _, _, nverts) = displayed_actor_geometry(layer, item, mesh);
            let bytes = if deforming || item.smoothing { nverts as u64 * 48 } else { 0 };
            let key = PoseKey(item.actor, item.model, item.mesh);
            if !self.poses.get(&key).is_some_and(|pose| pose.bytes == bytes && pose.smoothing == item.smoothing) {
                let uniform = r.device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 actor pose"), size: std::mem::size_of::<Uniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                let group = r.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 actor pose"), layout: &self.layout,
                    entries: &[wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() }] });
                let vertices = (bytes > 0).then(|| r.device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 previous displayed vertices"), size: bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
                self.poses.insert(key, Pose { transform: Mat4::IDENTITY.to_cols_array_2d(), vertices, bytes, smoothing: item.smoothing, ready: false, cut: false, uniform, group });
            }
            let pose = self.poses.get_mut(&key).expect("admitted above");
            pose.cut = pose.ready && transform_cut(pose.transform, layer.motion_instances[index].transform);
        }
    }

    /// The G2 pass; nothing is encoded (and no timer slot taken, `timestamp_writes` is only called when it runs) without actor items.
    pub(crate) fn encode<'t>(&self, r: &Renderer, enc: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, g2: &wgpu::TextureView, timestamp_writes: impl FnOnce() -> Option<wgpu::RenderPassTimestampWrites<'t>>) {
        let layer = &r.act;
        if layer.items.is_empty() { return; }
        for pose in self.poses.values() {
            let control = [0, u32::from(!pose.ready || pose.cut || self.reset), 0, 0];
            r.queue.write_buffer(&pose.uniform, 0, bytemuck::bytes_of(&Uniform {
                current_vp: self.current_vp.to_cols_array_2d(), previous_vp: self.previous_vp.to_cols_array_2d(), previous_model: pose.transform, control }));
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("v2 actor motion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: g2, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: depth, depth_ops: None, stencil_ops: None }),
            timestamp_writes: timestamp_writes(), occlusion_query_set: None, multiview_mask: None,
        });
        let Some(instances) = layer.bufs.get(r.frame) else { return };
        pass.set_bind_group(0, &r.globals_bg, &[]);
        pass.set_vertex_buffer(1, instances.slice(..));
        for (index, item) in layer.items.iter().enumerate() {
            if item.native_shadow_source.is_some_and(|source| r.replaces_native_shadow(source, item.native_shadow_receiver)) { continue; }
            let model = &layer.models[&item.model];
            let Some(mesh) = model.meshes[item.mesh].as_ref() else { continue };
            let Some(pose) = self.poses.get(&PoseKey(item.actor, item.model, item.mesh)) else { continue };
            let (vb, ib, draws, _) = displayed_actor_geometry(layer, item, mesh);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_vertex_buffer(2, pose.vertices.as_ref().filter(|_| pose.ready).unwrap_or(vb).slice(..));
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_bind_group(2, &pose.group, &[]);
            for draw in draws {
                if item.rendering_effect == 1 && model.holo.is_none() { continue; }
                let slot = mesh_effect_pipe(if item.faded { faded_pipe(draw.pipe) } else { draw.pipe }, item.rendering_effect, draw.projection_pass);
                let Some(pipeline) = self.pipes.get(slot) else { continue };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(1, &model.mats[draw.mat], &[]);
                pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, index as u32..index as u32 + 1);
            }
        }
    }

    /// Encode last (after the current skin compute and the motion pass): the displayed pose becomes next frame's previous pose.
    pub(crate) fn finish(&mut self, r: &Renderer, enc: &mut wgpu::CommandEncoder) {
        let layer = &r.act;
        for (index, item) in layer.items.iter().enumerate() {
            let Some(mesh) = &layer.models[&item.model].meshes[item.mesh] else { continue };
            let (vb, _, _, _) = displayed_actor_geometry(layer, item, mesh);
            let Some(pose) = self.poses.get_mut(&PoseKey(item.actor, item.model, item.mesh)) else { continue };
            if pose.bytes > 0 { enc.copy_buffer_to_buffer(vb, 0, pose.vertices.as_ref().expect("bytes > 0"), 0, pose.bytes); }
            pose.transform = layer.motion_instances[index].transform;
            pose.transform[0][3] = 0.0;
            pose.ready = true;
        }
    }
}

/// Match the primary actor raster exactly (smoothed / skinned displayed vertices).
fn displayed_actor_geometry<'a>(layer: &'a ActorLayer, item: &Item, mesh: &'a ModelMesh)
    -> (&'a wgpu::Buffer, &'a wgpu::Buffer, &'a [Draw], usize)
{
    if let Some(smooth) = mesh.smoothing.as_ref().filter(|_| item.smoothing) {
        let vb = if item.mesh == 0 { &layer.actors[&item.actor].smoothing.as_ref().unwrap().vertices } else { &smooth.vertices };
        return (vb, &smooth.indices, &smooth.draws, smooth.vertex_count as usize);
    }
    let own = (item.mesh == 0).then(|| layer.actors.get(&item.actor)?.vb.as_ref()).flatten();
    (own.unwrap_or(&mesh.vb), &mesh.ib, &mesh.draws, mesh.nverts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teleport_rejects_history() {
        let old = Mat4::IDENTITY.to_cols_array_2d();
        assert!(!transform_cut(old, Mat4::from_translation(Vec3::new(8.0, 0.0, 0.0)).to_cols_array_2d()));
        assert!(transform_cut(old, Mat4::from_translation(Vec3::new(8.01, 0.0, 0.0)).to_cols_array_2d()));
    }

    #[test]
    fn motion_pass_matches_the_g_buffer_raster() {
        let shader = include_str!("motion.wgsl");
        assert!(shader.contains("o.clip = g.vp * wp"));
        let source = include_str!("motion.rs");
        assert!(source.contains("wgpu::CompareFunction::Equal") && source.contains("depth_write_enabled: Some(false)"));
        assert!(source.contains("self.poses.retain(|key, _| admitted.contains(key))"));
    }
}
