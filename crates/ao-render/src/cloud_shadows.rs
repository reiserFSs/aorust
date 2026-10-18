//! Bounded native-weather transmittance field shared by cloud shadows and shafts.
//! Adapted from ao-vk cloud_shadow.frag (MIT); no actor or scene-depth inputs.
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use crate::volumetric_clouds::{CloudDensityLayer, CLOUD_DENSITY_WGSL};
use glam::Vec3;
const SIZE: u32 = 256;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params { area: [f32; 4], sun: [f32; 4], control: [f32; 4] }
#[derive(Clone)]
pub(super) struct CloudShadowField {
    pub(super) view: wgpu::TextureView,
    /// Eye-relative x/z corner, span, eye-relative projection-plane height.
    pub(super) area: [f32; 4],
    /// Authored unit direction towards the source; w=0 disables the field.
    pub(super) sun: [f32; 4],
    pub(super) eye: Vec3,
}
pub(super) struct CloudShadows {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    pipeline: RenderPipeline,
    target: Option<Target>,
    field: Option<CloudShadowField>,
    key: Option<u64>,
}
struct Target { view: wgpu::TextureView, bind: wgpu::BindGroup, layers: wgpu::Buffer }
impl CloudShadows {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("cloud field inputs"), entries: &[entry(0, wgpu::BufferBindingType::Uniform), entry(1, wgpu::BufferBindingType::Storage { read_only: true })] });
        let source = format!("{}\n{}", crate::shader_source::load("cloud_density.wgsl", CLOUD_DENSITY_WGSL), crate::shader_source::load("cloud_shadows.wgsl", include_str!("cloud_shadows.wgsl")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("world cloud transmittance"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("cloud field"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor { label: Some("256-square cloud field"), layout: Some(&pipeline_layout), vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] }, fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("shadow"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend: None, write_mask: wgpu::ColorWrites::RED })] }), primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("cloud field parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { layout, params, pipeline, target: None, field: None, key: None }
    }
    pub(super) fn is_ready(&self) -> bool { self.pipeline.is_ready() }
    pub(super) fn wait(&self) { self.pipeline.get(); }
    pub(super) fn release_targets(&mut self) { self.target = None; self.field = None; self.key = None; }
    pub(super) fn target_bytes(&self) -> u64 { self.target.as_ref().map_or(0, |_| self.params.size() + u64::from(SIZE) * u64::from(SIZE) * 2) }
    pub(super) fn field(&self) -> Option<CloudShadowField> { self.field.clone() }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer, layers: &[CloudDensityLayer], eye: Vec3, far: f32) {
        if layers.is_empty() { self.field = None; self.key = None; return; }
        let sun = Vec3::from_array(layers[0].sun[..3].try_into().expect("three direction components")).normalize_or_zero();
        let enabled = sun.y > 0.03;
        let plane = -far;
        let span = far * 4.0;
        let texel = span / SIZE as f32;
        let shift = if enabled { Vec3::new(sun.x, 0.0, sun.z) * (plane / sun.y) } else { Vec3::ZERO };
        let centre = eye + shift;
        let area = [(centre.x / texel).floor() * texel - eye.x - span * 0.5, (centre.z / texel).floor() * texel - eye.z - span * 0.5, span, plane];
        let params = Params { area, sun: sun.extend(if enabled { 1.0 } else { 0.0 }).to_array(), control: [layers.len() as f32, far * 128.0, 0.0, 0.0] };
        let mut key = 0xcbf29ce484222325u64;
        for byte in bytemuck::bytes_of(&params).iter().chain(bytemuck::cast_slice::<CloudDensityLayer, u8>(layers)) { key = (key ^ u64::from(*byte)).wrapping_mul(0x100000001b3); }
        if self.target.as_ref().is_none_or(|target| target.layers != *buffer) {
            let view = device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("world cloud field 256x256"), size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R16Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] }).create_view(&Default::default());
            let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("cloud field layer inputs"), layout: &self.layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: buffer.as_entire_binding() }] });
            self.target = Some(Target { view, bind, layers: buffer.clone() }); self.key = None;
        }
        let target = self.target.as_ref().expect("cloud map allocated");
        if self.key != Some(key) {
            queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("bounded cloud transmittance"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target.view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(&self.pipeline); pass.set_bind_group(0, &target.bind, &[]); pass.draw(0..3, 0..1);
            self.key = Some(key);
        }
        self.field = Some(CloudShadowField { view: target.view.clone(), area, sun: params.sun, eye });
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn field_cost_is_resolution_independent() { assert_eq!(u64::from(super::SIZE) * u64::from(super::SIZE) * 2, 131072); }
}
