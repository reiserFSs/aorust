//! Lighting resolve compute pass: G-buffer + depth (+ AO, shadows, lamps as they land) -> linear HDR.
use super::gbuffer::Targets;

pub(super) struct Resolve {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    dummy_ao: wgpu::TextureView,
}

/// A lighting module that extends the resolve: `wgsl` is concatenated before `resolve.wgsl` and declares its bindings at
/// group `k + 1` (k = its index in the plugin list); `resolve.wgsl` calls the functions it defines.
pub(super) struct Plugin<'a> {
    pub wgsl: String,
    pub layout: &'a wgpu::BindGroupLayout,
}

fn float_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}

impl Resolve {
    pub(super) fn new(device: &wgpu::Device, plugins: &[Plugin<'_>]) -> Self {
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            float_entry(1), float_entry(2), float_entry(3),
            wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            float_entry(5), float_entry(6),
        ];
        entries.push(wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: super::gbuffer::HDR, view_dimension: wgpu::TextureViewDimension::D2 }, count: None });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 resolve"), entries: &entries });
        let mut source = format!("{}\n{}\n", crate::shader_source::load("v2/common.wgsl", include_str!("common.wgsl")),
            crate::shader_source::load("v2/packing.wgsl", include_str!("packing.wgsl")));
        for plugin in plugins { source.push_str(&plugin.wgsl); source.push('\n'); }
        source.push_str(&crate::shader_source::load("v2/resolve.wgsl", include_str!("resolve.wgsl")));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 resolve"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let mut layouts = vec![Some(&layout)];
        layouts.extend(plugins.iter().map(|plugin| Some(plugin.layout)));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("v2 resolve"), bind_group_layouts: &layouts, immediate_size: 0 });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("v2 resolve"), layout: Some(&pipeline_layout), module: &module, entry_point: Some("main"),
            compilation_options: Default::default(), cache: None,
        });
        let dummy_ao = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("v2 AO placeholder"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view(&Default::default());
        Self { pipeline, layout, dummy_ao }
    }

    pub(super) fn bind(&self, device: &wgpu::Device, consts: &wgpu::Buffer, t: &Targets, ao: Option<&wgpu::TextureView>) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("v2 resolve"), layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&t.g0) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.g1) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&t.g3) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&t.depth) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&t.sky) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(ao.unwrap_or(&self.dummy_ao)) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&t.hdr) },
            ],
        })
    }

    /// `extra[k]` is bound at group `k + 1`, in the order of the plugins given to [`Resolve::new`].
    pub(super) fn encode(&self, enc: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup, extra: &[&wgpu::BindGroup], size: (u32, u32), timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 resolve"), timestamp_writes });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        for (k, group) in extra.iter().enumerate() { pass.set_bind_group(k as u32 + 1, *group, &[]); }
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
    }
}
