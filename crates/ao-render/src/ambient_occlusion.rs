//! Half-resolution diffuse-contact AO from current visible depth and grass porosity tags.
use crate::gpu_stats::GpuStatsExt;
use glam::Mat4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse_vp: [[f32; 4]; 4],
    vp: [[f32; 4]; 4],
    control: [f32; 4],
}

pub(super) struct AmbientOcclusion {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    estimate: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    targets: Option<Targets>,
}
struct Targets {
    size: (u32, u32),
    depth: wgpu::TextureView,
    ambient: wgpu::TextureView,
    raw: wgpu::TextureView,
    filtered: wgpu::TextureView,
    raw_bind: wgpu::BindGroup,
    blur_bind: wgpu::BindGroup,
}

impl AmbientOcclusion {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ambient AO inputs"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                texture(1, wgpu::TextureSampleType::Depth),
                texture(2, wgpu::TextureSampleType::Uint),
                texture(3, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("diffuse-contact HBAO-lite"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("ambient_occlusion.wgsl", include_str!("ambient_occlusion.wgsl"))) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("ambient AO"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let make_pipeline = |entry, format, blend| crate::pipeline_creation::render(device, &wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN | if entry == "composite" { wgpu::ColorWrites::BLUE } else { wgpu::ColorWrites::empty() } })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let estimate = make_pipeline("estimate", wgpu::TextureFormat::Rg32Uint, None);
        let blur = make_pipeline("blur", wgpu::TextureFormat::Rg32Uint, None);
        let composite = make_pipeline("composite", format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::ReverseSubtract },
            alpha: wgpu::BlendComponent::REPLACE,
        }));
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("ambient AO parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { layout, params, estimate, blur, composite, targets: None }
    }

    pub(super) fn release_targets(&mut self) { self.targets = None; }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, ambient: &wgpu::TextureView, colour: &wgpu::TextureView, size: (u32, u32), vp: Mat4, radius: f32, strength: f32, fog_depth: f32, grass_present: bool, mut timer: Option<&mut crate::FrameGpuTimer>) {
        if !radius.is_finite() || radius <= 0.0 || !strength.is_finite() || strength <= 0.0 || size.0 == 0 || size.1 == 0 { return; }
        if self.targets.as_ref().is_none_or(|targets| targets.size != size || targets.depth != *depth || targets.ambient != *ambient) {
            let create = |label| device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: Some(label), size: wgpu::Extent3d { width: size.0.div_ceil(2), height: size.1.div_ceil(2), depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rg32Uint,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
            }).create_view(&Default::default());
            let raw = create("ambient AO raw");
            let filtered = create("ambient AO bilateral");
            let bind = |ao| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("ambient AO frame inputs"), layout: &self.layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(ao) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(ambient) },
                ],
            });
            self.targets = Some(Targets { size, depth: depth.clone(), ambient: ambient.clone(), raw_bind: bind(&filtered), blur_bind: bind(&raw), raw, filtered });
        }
        let targets = self.targets.as_ref().expect("AO targets allocated");
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params { inverse_vp: vp.inverse().to_cols_array_2d(), vp: vp.to_cols_array_2d(), control: [radius, strength.clamp(0.0, 2.0), fog_depth, u32::from(grass_present) as f32] }));
        for (index, (label, pipeline, input, output, load)) in [
            ("ambient AO estimate", &self.estimate, &targets.raw_bind, &targets.raw, wgpu::LoadOp::Clear(wgpu::Color::BLACK)),
            ("ambient AO bilateral blur", &self.blur, &targets.blur_bind, &targets.filtered, wgpu::LoadOp::Clear(wgpu::Color::BLACK)),
            ("ambient AO upsample/subtract", &self.composite, &targets.raw_bind, colour, wgpu::LoadOp::Load),
        ].into_iter().enumerate() {
            let timestamps = match index {
                0 => timer.as_deref_mut().map(|timer| timer.begin_pass(false)),
                2 => timer.as_deref_mut().map(|timer| timer.end_pass()),
                _ => None,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: timestamps, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, input, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub(super) fn target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |targets| Self::incremental_bytes(targets.size) - std::mem::size_of::<Params>() as u64)
    }

    pub(super) fn incremental_bytes(size: (u32, u32)) -> u64 {
        // Two packed targets retain full32 depth and exact selected-pixel offsets
        // for bilateral edges; AO itself is half precision for native UNORM.
        u64::from(size.0.div_ceil(2)) * u64::from(size.1.div_ceil(2)) * 16 + std::mem::size_of::<Params>() as u64
    }
}

pub(super) fn fog_depth_limit(near: f32, far: f32, fog_end: f32) -> f32 {
    // Conservatively reject the original fully fogged depth, allowing four
    // normalized-depth ULPs for CPU/GPU projection rounding at distant terrain.
    (far / (far - near) * (1.0 - near / fog_end.max(near)) - 4.0 * f32::EPSILON).clamp(0.0, 1.0)
}

impl crate::Renderer {
    pub(super) fn encode_enhanced_ao(&mut self, encoder: &mut wgpu::CommandEncoder, vp: Mat4, fog_depth: f32) {
        let settings = self.enhanced_settings;
        if !settings.ambient_occlusion || settings.ao_radius <= 0.0 || settings.ao_strength <= 0.0 { return; }
        let Some(frame) = self.enhanced_frame.as_ref() else { return; };
        let Some(ambient) = frame.ambient.as_ref() else { return; };
        let ao = self.ambient_occlusion.as_mut().expect("AO pipelines warmed at startup");
        ao.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, ambient, &frame.colour,
            frame.size, vp, settings.ao_radius, settings.ao_strength, fog_depth, self.grass.as_ref().is_some_and(|grass| grass.has_draws()), None);
    }

    /// AO-owned textures/uniform plus its ambient MRT, excluding shared colour/depth
    /// and driver allocation overhead. Zero after the switch is disabled.
    pub fn ambient_occlusion_incremental_bytes(&self) -> u64 {
        let Some(frame) = self.enhanced_frame.as_ref() else { return 0; };
        if frame.ambient.is_none() { return 0; }
        let samples = if frame.sample_count > 1 { u64::from(frame.sample_count) + 1 } else { 1 };
        let ambient = u64::from(frame.size.0) * u64::from(frame.size.1) * 8 * samples;
        ambient + self.ambient_occlusion.as_ref().map_or(0, |ao| ao.targets.as_ref().map_or(0, |targets| AmbientOcclusion::incremental_bytes(targets.size)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn half_resolution_memory_covers_odd_extents() {
        assert_eq!(super::AmbientOcclusion::incremental_bytes((3, 5)), 2 * 3 * 16 + 144);
    }

    #[test]
    fn porous_grass_tag_survives_one_of_eight_msaa_samples() {
        let share = |alpha: f32| (alpha + 1.0).clamp(0.0, 1.0);
        for covered in 1..=8 {
            // Remaining samples belong to ordinary surfaces (alpha 1).
            let resolved = (-16.0 * covered as f32 + (8 - covered) as f32) / 8.0;
            assert!(resolved <= -1.0);
            assert_eq!(share(resolved), 0.0);
        }
        assert_eq!(share(1.0), 1.0);
        assert_eq!(share(0.0), 1.0);
        assert_eq!(share(-0.5), 0.5);
    }

    #[test]
    fn fog_cutoff_matches_original_projection_conservatively() {
        let projection = glam::Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0);
        let fog = projection * glam::Vec4::new(0.0, 0.0, -3.25, 1.0);
        let cutoff = super::fog_depth_limit(0.1, 100.0, 3.25);
        assert!(cutoff < fog.z / fog.w);
        assert!((cutoff - fog.z / fog.w).abs() < 0.000001);
    }

    #[test]
    fn hidden_actor_cannot_change_visible_ao_and_zero_ambient_is_untouched() {
        check_primary_depth((64, 64), false);
    }

    #[test]
    #[ignore = "explicit offline GPU timing at the design resolution"]
    fn ambient_occlusion_gpu_timing() {
        check_primary_depth((1920, 1200), true);
    }

    #[test]
    fn world_mrt_reduces_baked_excludes_unlit_glow_emissive_and_occluded_emission() {
        use crate::*;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("MRT fairness requires offscreen GPU");
        r.env.ambient = [0.2; 3];
        r.env.sun_color = [0.0; 3];
        r.set_enhanced_settings(EnhancedSettings { ambient_occlusion: true, emissive_bloom: true, ao_radius: 1.5, ao_strength: 0.5, ..Default::default() });
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("MRT readback"), source: wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) v:u32)->@builtin(position) vec4<f32> {
    return vec4<f32>(array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.))[v],0.,1.);
}
@fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> { return vec4<f32>(textureLoad(image,vec2<i32>(p.xy),0).rgb,1.); }
"#.into()) });
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }] });
        let pipeline_layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("MRT byte probe"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: r.format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let output = r.device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("MRT byte probe"), size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
        let view = output.create_view(&Default::default());
        let targets = Targets::new(&r, 32, 32);
        let capture = |r: &Renderer, emission: Option<bool>| {
            let frame = r.enhanced_frame.as_ref().unwrap();
            let input = match emission {
                Some(true) => frame.emissive.as_ref().unwrap(),
                Some(false) => frame.ambient.as_ref().unwrap(),
                None => &frame.colour,
            };
            let bind = r.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) }] });
            let mut encoder = r.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("MRT byte probe"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
                pass.set_pipeline(&pipeline); pass.set_bind_group(0, &bind, &[]); pass.draw(0..3, 0..1);
            }
            r.queue.submit([encoder.finish()]);
            texture_to_rgba(r, &output, 32, 32).unwrap()
        };
        for mode in 0..5 {
            let mesh = ao_scene::Mesh {
                vertices: [(-0.5,-0.5),(0.5,-0.5),(0.5,0.5),(-0.5,0.5)].into_iter().map(|(x,y)| ao_scene::Vertex { pos: [x,y,0.0], normal: [0.0,0.0,1.0], color: [0.1,0.1,0.1,1.0], ..Default::default() }).collect(),
                submeshes: vec![ao_scene::Submesh { two_sided: true, prelit: mode == 1, lighting_enabled: mode != 2, glow_mask: mode == 3, emissive: if mode == 4 { [0.2; 3] } else { [0.0; 3] }, ..ao_scene::Submesh::new(vec![0,1,2,0,2,3], None) }],
            };
            let scene = Scene { meshes: vec![mesh], ..Default::default() };
            r.add_actor_model(mode + 1, &scene);
            for actor_path in [false, true] {
                r.set_actors(Vec::new());
                if actor_path {
                    r.upload(&Scene::default());
                    let mut actor = ao_scene::ActorFrame { id: (mode + 1) as u32, model: mode + 1, ..Default::default() };
                    actor.transform[3][2] = -3.0;
                    r.set_actors(vec![actor]);
                } else {
                    let mut world = scene.clone();
                    let mut transform = ao_scene::IDENTITY;
                    transform[3][2] = -3.0;
                    world.instances.push(ao_scene::Instance { mesh: 0, transform });
                    r.upload(&world);
                }
                r.env.ambient = [0.2; 3];
                r.env.sun_color = [0.0; 3];
                r.render(&view, &targets, &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
                let ambient = capture(&r, Some(false));
                let has_ambient = ambient.as_chunks::<4>().0.iter().any(|pixel| pixel[..3] != [0,0,0]);
                assert_eq!(has_ambient, mode <= 1, "AO receiver MRT mode={mode} actor={actor_path}: ordinary and reduced baked materials only");
                if mode == 1 {
                    let beauty = capture(&r, None);
                    for (receiver, colour) in ambient.iter().zip(beauty.iter()).enumerate().filter_map(|(index, pair)| (index % 4 != 3).then_some(pair)) {
                        assert!(i32::from(*receiver) * 5 <= i32::from(*colour) + 5, "baked receivers are bounded to 20% of native beauty");
                    }
                }
                if mode == 0 {
                    assert!(capture(&r, Some(true)).as_chunks::<4>().0.iter().all(|pixel| pixel[..3] == [0,0,0]), "bright ordinary material is not emission");
                } else if mode == 2 || mode == 3 {
                    assert!(capture(&r, Some(true)).as_chunks::<4>().0.iter().any(|pixel| pixel[..3] != [0,0,0]), "authored unlit/glow produces a nonzero visible emission mask");
                }
            }
        }
        let mut wall = ao_scene::ActorFrame { id: 10, model: 1, ..Default::default() };
        wall.transform[3][2] = -2.0;
        let mut hidden_emitter = ao_scene::ActorFrame { id: 11, model: 4, ..Default::default() };
        hidden_emitter.transform[3][2] = -3.0;
        r.set_actors(vec![wall, hidden_emitter]);
        r.render(&view, &targets, &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
        assert!(capture(&r, Some(true)).as_chunks::<4>().0.iter().all(|pixel| pixel[..3] == [0,0,0]), "occluded glow actor cannot seed bloom");
        assert!(r.ambient_occlusion_incremental_bytes() > 0);
        assert!(r.ambient_occlusion.is_some(), "AO must have encoded before testing resource retirement");
        let enabled = r.enhanced_settings;
        let frame_compiles = crate::pipeline_creation::frame_thread_compiles();
        r.set_enhanced_settings(EnhancedSettings::default());
        assert_eq!(r.ambient_occlusion_incremental_bytes(), 0);
        let ao = r.ambient_occlusion.as_ref().expect("off retains precompiled AO pipelines");
        assert!(ao.targets.is_none(), "off immediately releases AO targets and their frame views");
        assert_eq!(ao.target_bytes(), 0);
        for _ in 0..2 {
            r.set_enhanced_settings(enabled);
            r.render(&view, &targets, &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
            assert!(r.ambient_occlusion_incremental_bytes() > 0);
            r.set_enhanced_settings(EnhancedSettings::default());
            r.render(&view, &targets, &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
            assert!(r.ambient_occlusion.as_ref().unwrap().targets.is_none());
            assert_eq!(r.ambient_occlusion_incremental_bytes(), 0);
        }
        assert_eq!(crate::pipeline_creation::frame_thread_compiles(), frame_compiles,
            "repeated AO toggles must reuse startup/variant pipelines without frame-thread compilation");
    }

    fn check_primary_depth((width, height): (u32, u32), measure: bool) {
        use crate::{Renderer, gpu_stats::GpuStatsExt};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = Renderer::new(&instance, None).expect("AO fairness requires offscreen GPU");
        let device = &renderer.device;
        let queue = &renderer.queue;
        let vp = glam::Mat4::perspective_rh(1.0, width as f32 / height as f32, 0.1, 100.0);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("AO wall and hidden actor regression"),
            source: wgpu::ShaderSource::Wgsl(r#"
struct Params { vp: mat4x4<f32> }
@group(0) @binding(0) var<uniform> params: Params;
@vertex fn vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) stage: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 6>(vec2(-1.,-1.),vec2(1.,-1.),vec2(1.,1.),vec2(-1.,-1.),vec2(1.,1.),vec2(-1.,1.))[vertex];
    var z = -4.0;
    if stage == 1u { p.x = (p.x - 1.0) * 0.5; z = -3.0; }
    if stage == 2u { p = p * 0.2 + vec2(-0.5, 0.0); z = -3.5; }
    if stage == 3u { p = p * 0.2 + vec2(0.5, 0.0); z = -3.5; }
    let clip = params.vp * vec4<f32>(0.0, 0.0, z, 1.0);
    return vec4<f32>(p * clip.w, clip.z, clip.w);
}
"#.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }] });
        let matrix = device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&vp.to_cols_array()), usage: wgpu::BufferUsages::UNIFORM });
        let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: matrix.as_entire_binding() }] });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("AO primary view depth only"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] }, fragment: None,
            primitive: Default::default(), depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let texture = |format, usage| device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("AO fairness fixture"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] });
        let depth = texture(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let depth = depth.create_view(&Default::default());
        let ambient = texture(wgpu::TextureFormat::Rgba16Float, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let ambient = ambient.create_view(&Default::default());
        let alternate_ambient = texture(wgpu::TextureFormat::Rgba16Float, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default());
        let colour = texture(renderer.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let colour_view = colour.create_view(&Default::default());
        let mut ao = super::AmbientOcclusion::new(device, renderer.format);
        let mut timer = measure.then(|| crate::gpu_stats::FrameGpuTimer::new(device, queue).expect("AO timing requires timestamp support"));
        let mut timings = Vec::with_capacity(if measure { 85 } else { 0 });
        if measure {
            let owned = super::AmbientOcclusion::incremental_bytes((width, height));
            let pixels = u64::from(width) * u64::from(height);
            eprintln!("AO {width}x{height} owned_bytes={owned} ambient_mrt_bytes={} effect_incremental_bytes={} ao_only_including_framework_bytes={}",
                pixels * 8, owned + pixels * 8, owned + pixels * 16);
        }
        let mut render = |actor, ambient_value, fog_end, fog_actor| {
            // Same-size Enhanced target replacement must invalidate cached binds.
            let ambient_view = if ambient_value == 0.0 { &alternate_ambient } else { &ambient };
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("AO occlusion fixture"), color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                    timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..6, 0..if actor { 3 } else { 2 });
                if fog_actor { pass.draw(0..6, 3..4); }
            }
            for (view, value) in [(ambient_view, ambient_value), (&colour_view, 0.5)] {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("AO ambient-only fixture"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: value, g: value, b: value, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
            }
            ao.encode(device, queue, &mut encoder, &depth, ambient_view, &colour_view, (width, height), vp, 1.5, 1.0, super::fog_depth_limit(0.1, 100.0, fog_end), false, timer.as_mut());
            let readback = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("AO fairness readback"), size: u64::from(width) * u64::from(height) * 4, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            encoder.copy_texture_to_buffer(wgpu::TexelCopyTextureInfo { texture: &colour, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All }, wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) } }, wgpu::Extent3d { width, height, depth_or_array_layers: 1 });
            queue.submit([encoder.finish()]);
            if let Some(timer) = timer.as_mut() {
                timings.push(timer.read_ms(device, queue).expect("AO GPU timestamp readback"));
            }
            let (send, receive) = std::sync::mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |result| { send.send(result).unwrap(); });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            receive.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range().to_vec();
            readback.unmap();
            bytes
        };
        let absent = render(false, 0.25, 100.0, false);
        let present = render(true, 0.25, 100.0, false);
        assert_eq!(absent, present, "wall-hidden actor must not alter any AO pixel");
        let untouched = render(false, 0.0, 100.0, false);
        for pixel in untouched.as_chunks::<4>().0 {
            assert_eq!(*pixel, [128, 128, 128, 255], "zero ambient leaves selflit/prelit beauty exactly unchanged");
        }
        assert_ne!(absent, untouched, "visible wall depth must produce actual AO");
        for (ao, original) in absent.iter().zip(&untouched) {
            assert!(ao <= original, "AO never brightens");
        }
        for y in 0..height {
            for x in 0..width / 2 {
                let offset = ((y * width + x) * 4) as usize;
                assert_eq!(&absent[offset..offset + 4], &untouched[offset..offset + 4],
                    "background occlusion must not halo onto the nearer flat wall at {x},{y}");
            }
        }
        let fog_absent = render(false, 0.25, 3.25, false);
        let fog_present = render(false, 0.25, 3.25, true);
        assert_eq!(fog_absent, fog_present, "fully original-fogged actors cannot affect visible AO");
        if measure {
            for _ in 0..80 { render(true, 0.25, 100.0, false); }
            let steady = &mut timings[25..];
            steady.sort_by(f64::total_cmp);
            eprintln!("AO steady-state 1920x1200 full-coverage samples={} mean_ms={:.6} p50_ms={:.6} p95_ms={:.6} min_ms={:.6} max_ms={:.6}",
                steady.len(), steady.iter().sum::<f64>() / steady.len() as f64, steady[steady.len() / 2],
                steady[steady.len() * 95 / 100], steady[0], steady[steady.len() - 1]);
        }
    }
}
