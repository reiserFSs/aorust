//! Final raster stage: HDR -> tone map + grade + dither -> the output view.
pub(super) struct Post {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl Post {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let entries = [
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
        ];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 post"), entries: &entries });
        let source = format!("{}\n{}\n{}",
            crate::shader_source::load("v2/common.wgsl", include_str!("common.wgsl")),
            crate::shader_source::load("v2/packing.wgsl", include_str!("packing.wgsl")),
            crate::shader_source::load("v2/post.wgsl", include_str!("post.wgsl")));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 post"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("v2 post"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("v2 post"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        Self { pipeline, layout }
    }

    pub(super) fn bind(&self, device: &wgpu::Device, consts: &wgpu::Buffer, input: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("v2 post"), layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(input) },
            ],
        })
    }

    pub(super) fn encode(&self, enc: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup, output: &wgpu::TextureView, timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("v2 post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
