//! Opt-in waterfall-foot mist from exact steep-liquid records, never emissive.
//! Scene-lit alpha blending retains resolved-depth occlusion and native fog extinction.
use glam::{Mat4, Vec3};
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline, water_dynamics::SprayRecord};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    vp: [[f32; 4]; 4],
    eye_time: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    fog: [f32; 4],
    illumination: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance { position_radius: [f32; 4], opacity_seed: [f32; 4] }

pub(super) struct WaterSpray {
    device: wgpu::Device,
    pipeline: RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    staging: Vec<Instance>,
    count: u32,
    depth_bind: Option<(wgpu::TextureView, wgpu::BindGroup)>,
}

impl WaterSpray {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waterfall spray"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waterfall spray"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("water_spray.wgsl", include_str!("water_spray.wgsl"))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("waterfall spray"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some("scene-lit waterfall spray"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Instance, attributes: &attributes }] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState {
                format, blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
                    alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
                }), write_mask: wgpu::ColorWrites::ALL,
            })] }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("waterfall spray params"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let instances = instance_buffer(device, 1);
        Self { device: device.clone(), pipeline, layout, params, instances, capacity: 1, staging: Vec::new(), count: 0, depth_bind: None }
    }

    pub fn is_ready(&self) -> bool { self.pipeline.is_ready() }
    pub fn wait(&self) { self.pipeline.get(); }

    /// `fog` is the effective native [start, end, enabled, unused] tuple.
    /// `illumination` is native ambient plus incident sun/sky light, in gamma space.
    /// The caller supplies no records while the independent waterfalls switch is off.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), vp: Mat4, eye: Vec3, time: f32, fog: [f32; 4], illumination: [f32; 3], records: &[SprayRecord]) {
        self.count = 0;
        self.staging.clear();
        if size.0 == 0 || size.1 == 0 || !vp.is_finite() || !eye.is_finite() || !time.is_finite() { return; }
        self.staging.reserve(records.len());
        for record in records {
            if !record.position.iter().all(|v| v.is_finite()) || !record.radius.is_finite() || record.radius <= 0.0 || !record.opacity.is_finite() || record.opacity <= 0.0 { continue; }
            let p = record.position;
            self.staging.push(Instance { position_radius: [p[0], p[1], p[2], record.radius], opacity_seed: [record.opacity.clamp(0.0, 1.0), p[0] * 0.73 + p[2] * 1.31, 0.0, 0.0] });
        }
        if self.staging.is_empty() { return; }
        if self.staging.len() > self.capacity {
            self.capacity = self.staging.len().next_power_of_two();
            self.instances = instance_buffer(device, self.capacity);
        }
        let right = vp.row(0).truncate().normalize();
        let up = vp.row(1).truncate().normalize();
        let forward = -right.cross(up).normalize();
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params {
            vp: vp.to_cols_array_2d(), eye_time: eye.extend(time).to_array(),
            right: right.extend(0.0).to_array(), up: up.extend(0.0).to_array(), forward: forward.extend(0.0).to_array(), fog,
            illumination: [illumination[0].max(0.0), illumination[1].max(0.0), illumination[2].max(0.0), 0.0],
        }));
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.staging));
        self.count = self.staging.len() as u32;
    }

    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, colour: &wgpu::TextureView, resolved_depth: &wgpu::TextureView) {
        if self.count == 0 { return; }
        if self.depth_bind.as_ref().is_none_or(|(depth, _)| depth != resolved_depth) {
            let bind = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("waterfall spray resolved depth"), layout: &self.layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(resolved_depth) },
                ],
            });
            self.depth_bind = Some((resolved_depth.clone(), bind));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("waterfall foot spray and mist"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: colour, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.depth_bind.as_ref().expect("spray depth bound").1, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("waterfall spray instances"), size: (capacity * std::mem::size_of::<Instance>()) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

#[cfg(test)]
mod tests {
    #[test]
    fn waterfall_spray_shader_validates() {
        assert_eq!(std::mem::size_of::<super::Params>(), 160, "scene illumination retains uniform vec4 alignment");
        let module = wgpu::naga::front::wgsl::parse_str(include_str!("water_spray.wgsl")).expect("spray WGSL parses");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all()).validate(&module).expect("spray WGSL validates");
    }
}
