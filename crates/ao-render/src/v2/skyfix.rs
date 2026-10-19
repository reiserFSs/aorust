//! Native sky layers that write depth (`Spec::flip_depth` on a sky slot) occlude world geometry beyond the sky dome in the faithful
//! path. The G-buffer pass keeps that depth so far geometry (e.g. a 780 m station) is hidden exactly like the original, then this
//! pass restores the "depth == 1 means no geometry" contract: wherever no G-class draw wrote the G-buffer (G1.a == 0, geometry
//! writes 1), depth is reset to 1.0. Everything after it (admission, shadows, AO, resolve, FX, TAA) sees only real surfaces.
pub(super) struct SkyFix {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl SkyFix {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 sky depth fix"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
        ] });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 sky depth fix"),
            source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("v2/skyfix.wgsl", include_str!("skyfix.wgsl"))) });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("v2 sky depth fix"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("v2 sky depth fix"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 })),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[] }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: crate::DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        Self { pipeline, layout }
    }

    pub(super) fn bind(&self, device: &wgpu::Device, g1: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 sky depth fix"), layout: &self.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(g1) }] })
    }

    pub(super) fn encode(&self, enc: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup, depth: &wgpu::TextureView) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("v2 sky depth fix"), color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }),
            timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
