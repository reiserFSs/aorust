//! Actor admission (frame slot 1): which actors are really on screen this frame (DESIGN section 8).
//!
//! After the G-buffer pass only the actors the host gave this frame (`ActorFrame::alpha > 1e-5`, i.e. the items of the actor
//! layer) are drawn into an `R32Uint` id target, depth-tested `LessEqual` (read-only, with a 4-ulp bias) against the G-buffer
//! depth, so an id survives exactly where that actor is the nearest surface. Native fully fogged fragments are discarded, like
//! v1's primary pass. A compute reduction then publishes the result.
//!
//! # `Admission::visibility()` layout (frozen; read by the dynamic shadow layer and by V2Lamps' carried-light owner check)
//! * `var<storage, read> visibility: array<u32>`, fixed length [`SLOTS`]` + 1` (= 8193 words, never reallocated).
//! * Index = the actor's interaction slot = `ActorLayer::interaction_slot(Some(actor))` = 1-based index of the first
//!   actor-layer item of that actor (all meshes of one actor share the slot). Element 0 is always 0 (slot 0 means "no actor"/world).
//! * Value 1 = at least one pixel of that actor passed the depth test against this frame's G-buffer and is not fully fogged;
//!   0 = not admitted. A slot `>= arrayLength(&visibility)` is NOT admitted (fail closed).
//! * Cleared and rewritten every frame in slot 1, strictly before shadows (slot 2), AO (3), clusters (4) and the resolve. Nothing
//!   survives into the next frame, and an actor with alpha 0 has no item, hence no slot, hence always 0.
//! * Usage `STORAGE | COPY_SRC | COPY_DST`.
//!
//! `Admission::slots()` is the matching per-actor-item vertex stream (item index -> slot) the shadow casters read at location 12.
use super::timers::PassTimers;
use crate::pipeline_creation::{self, RenderPipeline};
use crate::pipelines::{Pipelines, Spec};
use crate::{Renderer, DEPTH};

/// Highest interaction slot that can be admitted.
pub(super) const SLOTS: usize = 8192;
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

/// Item index -> interaction slot (1-based index of the actor's first item) for a list of per-item actor ids.
pub(crate) fn item_slots(actors: impl Iterator<Item = u32>, out: &mut Vec<u32>) {
    out.clear();
    let mut first = std::collections::HashMap::new();
    for (index, actor) in actors.enumerate() { out.push(*first.entry(actor).or_insert(index as u32 + 1)); }
}

/// CPU mirror of `admission.wgsl`: which slots end up admitted for an id image.
#[cfg(test)]
fn reduce(ids: &[u32], len: usize) -> Vec<u32> {
    let mut visible = vec![0; len];
    for &id in ids { if id != 0 && (id as usize) < len { visible[id as usize] = 1; } }
    visible
}

pub(super) struct Admission {
    pipes: Pipelines,
    reduce: wgpu::ComputePipeline,
    reduce_layout: wgpu::BindGroupLayout,
    visibility: wgpu::Buffer,
    slots: wgpu::Buffer,
    slots_cap: usize,
    scratch: Vec<u32>,
    sized: Option<((u32, u32), wgpu::TextureView, wgpu::BindGroup)>,
}

fn compile(device: &wgpu::Device, shader: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout, spec: Spec) -> RenderPipeline {
    let test = matches!(spec.blend, crate::Blend::AlphaTest);
    let f4 = |o| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: o, shader_location: 4 + (o / 16) as u32 };
    let actor_attrs = [f4(0), f4(16), f4(32), f4(48), f4(64), f4(80), f4(96), f4(112),
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 128, shader_location: 13 }];
    let vert_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
    let id_attrs = wgpu::vertex_attr_array![12 => Uint32];
    pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
        label: Some("v2 actor admission ids"), layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("vs_shadow_primary_actor"), compilation_options: Default::default(), buffers: &[
            wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vert_attrs },
            wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<crate::actors::ActorInstance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &actor_attrs },
            wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Instance, attributes: &id_attrs },
        ] },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(if test { "fs_shadow_primary_test" } else { "fs_shadow_primary_opaque" }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: ID_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
        primitive: wgpu::PrimitiveState { cull_mode: (!spec.two_sided).then_some(wgpu::Face::Back), ..Default::default() },
        // Read-only against the G-buffer depth: an actor id survives only where that actor is the nearest surface.
        // The bias absorbs last-bit differences between this vertex path and the G-buffer's (a few cm at 100 m).
        depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(), bias: wgpu::DepthBiasState { constant: -4, slope_scale: 0.0, clamp: 0.0 } }),
        multisample: Default::default(), multiview_mask: None, cache: None,
    })
}

impl Admission {
    pub(super) fn new(r: &Renderer) -> Self {
        let device = &r.device;
        let source = format!("{}\n{}", crate::shader_source::load("shader.wgsl", include_str!("../shader.wgsl")),
            crate::shader_source::load("sun_shadows.wgsl", include_str!("../sun_shadows.wgsl")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 actor admission ids"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("v2 actor admission ids"), bind_group_layouts: &[Some(&r.g_layout), Some(&r.tex_layout)], immediate_size: 0 });
        let (_, mut pipes, _) = crate::build_pipelines();
        for index in 0..pipes.len() {
            if crate::sun_shadows::pipeline_key(pipes.spec(index), true).is_some() {
                let (device, shader, layout) = (device.clone(), shader.clone(), layout.clone());
                pipes.ensure(index, &move |spec| compile(&device, &shader, &layout, spec));
            }
        }
        pipes.wait();
        let reduce_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 admission reduce"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ] });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 admission reduce"),
            source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("v2/admission.wgsl", include_str!("admission.wgsl"))) });
        let reduce = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("v2 admission reduce"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("v2 admission reduce"), bind_group_layouts: &[Some(&reduce_layout)], immediate_size: 0 })),
            module: &module, entry_point: Some("reduce"), compilation_options: Default::default(), cache: None });
        let visibility = device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 actor visibility"), size: ((SLOTS + 1) * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let slots_cap = 64;
        Self { pipes, reduce, reduce_layout, visibility, slots: Self::slot_buffer(device, slots_cap), slots_cap, scratch: Vec::new(), sized: None }
    }

    fn slot_buffer(device: &wgpu::Device, cap: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 actor item slots"), size: (cap * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }

    /// The frozen per-actor visibility words (layout in the module docs).
    pub(super) fn visibility(&self) -> &wgpu::Buffer { &self.visibility }

    /// Per actor item: its interaction slot (vertex stream, location 12 of the actor shadow casters).
    pub(super) fn slots(&self) -> &wgpu::Buffer { &self.slots }

    /// Slot 1. Always encodes the id pass and the reduction (empty when there are no actors) so the timer label is stable.
    pub(super) fn encode(&mut self, r: &Renderer, enc: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, size: (u32, u32), timers: Option<&PassTimers>) {
        if self.sized.as_ref().is_none_or(|(s, ..)| *s != size) {
            let ids = r.device.create_texture(&wgpu::TextureDescriptor { label: Some("v2 actor ids"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2, format: ID_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
            let bind = r.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 admission reduce"), layout: &self.reduce_layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&ids) },
                wgpu::BindGroupEntry { binding: 1, resource: self.visibility.as_entire_binding() },
            ] });
            self.sized = Some((size, ids, bind));
        }
        let (_, ids, bind) = self.sized.as_ref().expect("sized above");
        r.v2_item_slots(&mut self.scratch);
        if self.scratch.len() > self.slots_cap {
            self.slots_cap = self.scratch.len().next_power_of_two();
            self.slots = Self::slot_buffer(&r.device, self.slots_cap);
        }
        if !self.scratch.is_empty() { r.queue.write_buffer(&self.slots, 0, bytemuck::cast_slice(&self.scratch)); }
        enc.clear_buffer(&self.visibility, 0, None);
        let span = timers.and_then(|t| t.span("admission"));
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("v2 actor admission ids"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: ids, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: depth, depth_ops: None, stencil_ops: None }),
                timestamp_writes: span.as_ref().map(|s| s.begin_render()), occlusion_query_set: None, multiview_mask: None,
            });
            if !self.scratch.is_empty() {
                pass.set_bind_group(0, &r.globals_bg, &[]);
                r.v2_draw_actor_parts(&mut pass, &self.slots, true, &|slot| self.pipes.get(slot));
            }
        }
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 admission reduce"), timestamp_writes: span.as_ref().map(|s| s.end_compute()) });
        pass.set_pipeline(&self.reduce);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_of_one_actor_share_the_slot_of_its_first_item() {
        let mut out = Vec::new();
        item_slots([7, 9, 7, 7, 12, 9].into_iter(), &mut out);
        assert_eq!(out, [1, 2, 1, 1, 5, 2]);
        item_slots(std::iter::empty(), &mut out);
        assert!(out.is_empty(), "no items (e.g. only zero-alpha actors) means no slot is ever published");
    }

    #[test]
    fn reduction_admits_only_slots_present_in_the_id_image() {
        let len = SLOTS + 1;
        let visible = reduce(&[0, 0, 3, 3, 3, 8, 0, SLOTS as u32, SLOTS as u32 + 1, u32::MAX], len);
        let admitted: Vec<usize> = (0..len).filter(|&i| visible[i] != 0).collect();
        assert_eq!(admitted, [3, 8, SLOTS], "slot 0 and out-of-range ids are never admitted");
        assert!(reduce(&[], len).iter().all(|&v| v == 0), "an empty id image admits nobody (cleared every frame)");
    }
}
