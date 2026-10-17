//! Optional HDR boundary. Native shaders already produce display-space colour: identity
//! tonemapping deliberately performs no exposure, transfer-function or black-level change.
use crate::{gpu_stats::{GpuStatsExt, TextureViewStatsExt}, DEPTH};
pub(super) const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(super) struct EnhancedFrame {
    pub size: (u32, u32),
    pub sample_count: u32,
    pub hdr: wgpu::TextureView,
    pub ambient: Option<wgpu::TextureView>,
    pub emissive: Option<wgpu::TextureView>,
    pub depth: wgpu::TextureView,
    pub resolved_depth: wgpu::TextureView,
    pub msaa: Option<[Option<wgpu::TextureView>; 3]>,
    pub composite: wgpu::RenderPipeline,
    composite_bg: wgpu::BindGroup,
    depth_resolve: Option<(wgpu::RenderPipeline, wgpu::BindGroup)>,
}
impl EnhancedFrame {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32), samples: u32, settings: crate::EnhancedSettings) -> Self {
        let make = |format, count, label| device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: count, dimension: wgpu::TextureDimension::D2, format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view_tracked(&Default::default());
        let hdr = make(HDR, 1, "Enhanced HDR");
        let ambient = settings.ambient_occlusion.then(|| make(HDR, 1, "Enhanced ambient share"));
        let emissive = settings.emissive_bloom.then(|| make(HDR, 1, "Enhanced authored emission"));
        let depth = make(DEPTH, samples, "Enhanced sampleable depth");
        let resolved_depth = if samples == 1 { depth.clone() } else { make(DEPTH, 1, "Enhanced resolved depth") };
        let msaa = (samples > 1).then(|| [Some(make(HDR, samples, "Enhanced MSAA HDR")), settings.ambient_occlusion.then(|| make(HDR, samples, "Enhanced MSAA ambient")), settings.emissive_bloom.then(|| make(HDR, samples, "Enhanced MSAA emission"))]);
        let (composite, composite_bg) = pipeline(device, format, &hdr, false);
        let depth_resolve = (samples > 1).then(|| pipeline(device, DEPTH, &depth, true));
        Self { size, sample_count: samples, hdr, ambient, emissive, depth, resolved_depth, msaa, composite, composite_bg, depth_resolve }
    }
    pub fn attachment(&self, index: usize) -> Option<wgpu::RenderPassColorAttachment<'_>> {
        let resolved = [Some(&self.hdr), self.ambient.as_ref(), self.emissive.as_ref()][index]?;
        let msaa = self.msaa.as_ref().and_then(|views| views[index].as_ref());
        Some(wgpu::RenderPassColorAttachment { view: msaa.unwrap_or(resolved), resolve_target: msaa.map(|_| resolved), depth_slice: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: if msaa.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store } } })
    }
    pub fn resolve_depth(&self, enc: &mut wgpu::CommandEncoder) {
        let Some((pipe, bg)) = &self.depth_resolve else { return; };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Enhanced depth resolve"), color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.resolved_depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None,
            }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(pipe); pass.set_bind_group(0, bg, &[]); pass.draw(0..3, 0..1);
    }
    pub fn composite(&self, enc: &mut wgpu::CommandEncoder, output: &wgpu::TextureView, input: Option<&wgpu::BindGroup>) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Enhanced identity tonemap"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&self.composite); pass.set_bind_group(0, input.unwrap_or(&self.composite_bg), &[]); pass.draw(0..3, 0..1);
    }
}
fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat, input: &wgpu::TextureView, depth: bool) -> (wgpu::RenderPipeline, wgpu::BindGroup) {
    let source = if depth {
        "@group(0) @binding(0) var image: texture_depth_multisampled_2d;\n@fragment fn fs(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 { var z=1.0; for(var s=0u;s<textureNumSamples(image);s+=1u){z=min(z,textureLoad(image,vec2<i32>(p.xy),i32(s)));} return z; }"
    } else {
        "@group(0) @binding(0) var image: texture_2d<f32>;\n@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return clamp(textureLoad(image,vec2<i32>(p.xy),0),vec4<f32>(0.0),vec4<f32>(1.0)); }"
    };
    let source = format!("{source}\n@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{{let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));return vec4<f32>(p[i],0.0,1.0);}}");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Enhanced boundary"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Enhanced boundary"), entries: &[wgpu::BindGroupLayoutEntry {
        binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: if depth { wgpu::TextureSampleType::Depth } else { wgpu::TextureSampleType::Float { filterable: false } }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: depth }, count: None,
    }] });
    let bg = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Enhanced boundary"), layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) }] });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Enhanced boundary"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    let targets = [Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })];
    let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Enhanced boundary"), layout: Some(&pl),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: if depth { &[] } else { &targets } }),
        primitive: Default::default(), depth_stencil: depth.then_some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
        multisample: Default::default(), multiview_mask: None, cache: None,
    });
    (pipe, bg)
}

#[cfg(test)]
mod tests {
    use crate::*;
    #[test]
    fn neutral_hdr_pixels_and_resource_retirement() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("Enhanced regression needs offscreen GPU");
        let scene = Scene { meshes: vec![ao_scene::Mesh {
            vertices: [(-0.5,-0.5),(0.5,-0.5),(0.5,0.5),(-0.5,0.5)].into_iter().map(|(x,y)|
                ao_scene::Vertex { pos: [x,y,0.0], normal: [0.0,0.0,1.0], ..Default::default() }).collect(),
            submeshes: vec![ao_scene::Submesh { two_sided: true, emissive: [0.5,0.2,0.1],
                ..ao_scene::Submesh::new(vec![0,1,2,0,2,3], None) }],
        }], ..Default::default() };
        r.add_actor_model(1, &scene);
        let mut actor = ao_scene::ActorFrame { id: 1, model: 1, ..Default::default() };
        actor.transform[3][2] = -3.0;
        r.set_actors(vec![actor]);
        let capture = |r: &mut Renderer| {
            let output = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: Some("Enhanced pixel oracle"), size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
            });
            let targets = Targets::new(r,32,32);
            r.render(&output.create_view_tracked(&Default::default()), &targets, &Camera::look_at(Vec3::ZERO,Vec3::NEG_Z));
            texture_to_rgba(r,&output,32,32).unwrap()
        };
        for samples in r.msaa_supported.clone() {
            r.set_msaa_samples(samples);
            let faithful = capture(&mut r);
            assert!(r.enhanced_frame.is_none() && r.enhanced_shader.is_none());
            assert_eq!(r.enhanced_target_bytes(), 0);
            r.set_enhanced_settings(EnhancedSettings { ground_grass: true, grass_density: 0.0, ..Default::default() });
            let neutral = capture(&mut r);
            let sample_count = u64::from(r.msaa_samples());
            let expected = 32 * 32 * (8 + 4 * sample_count + if sample_count > 1 { 8 * sample_count + 4 } else { 0 });
            assert_eq!(r.enhanced_target_bytes(), expected, "neutral allocates no ambient/emission masks");
            let max_error = faithful.iter().zip(&neutral).map(|(a,b)| a.abs_diff(*b)).max().unwrap();
            assert!(max_error <= 1, "neutral HDR {samples}x max byte difference {max_error}");
            r.set_enhanced_settings(EnhancedSettings::default());
            assert!(r.enhanced_frame.is_none() && r.enhanced_shader.is_none());
            assert_eq!(r.enhanced_target_bytes(), 0);
            assert_eq!(faithful,capture(&mut r),"all-off roundtrip must be byte identical");
        }
    }
}
