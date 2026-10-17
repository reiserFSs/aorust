//! Half-resolution screen-space contact shadows (port of ao-vk `contact.frag`, MIT; see THIRD_PARTY). Inputs are exclusively
//! the current frame's resolved primary-view depth and the world pass's sun mask; the result only darkens the sun's direct share.
//! Weak night/dusk directional light remains eligible; native fog and mapped-shadow attenuation stay in the mask.
//! A finite 3x3 march-phase tile is averaged over a full period using symmetric tangent-plane rejection,
//! including at upsampling; sloping surfaces stay smooth without mixing foreground/background silhouettes.
use crate::gpu_stats::GpuStatsExt;
use crate::pipeline_creation::RenderPipeline;
use glam::{Mat4, Vec3};

/// World-space length of the march towards the sun (metres).
const RAY_LENGTH: f32 = 0.6;
/// Farthest view depth the shadow applies at; fades out over the last 30%.
const FADE_FAR: f32 = 80.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse_vp: [[f32; 4]; 4],
    vp: [[f32; 4]; 4],
    eye: [f32; 4],
    sun: [f32; 4],
    control: [f32; 4],
}

pub(super) struct ContactShadows {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    estimate: RenderPipeline,
    blur: RenderPipeline,
    composite: RenderPipeline,
    targets: Option<Targets>,
}
struct Targets {
    size: (u32, u32),
    depth: wgpu::TextureView,
    sun: wgpu::TextureView,
    raw: wgpu::TextureView,
    filtered: wgpu::TextureView,
    raw_bind: wgpu::BindGroup,
    blur_bind: wgpu::BindGroup,
}

impl ContactShadows {
    pub(super) fn is_ready(&self) -> bool {
        self.estimate.is_ready() && self.blur.is_ready() && self.composite.is_ready()
    }
    pub(super) fn wait(&self) {
        self.estimate.get();
        self.blur.get();
        self.composite.get();
    }

    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("contact shadow inputs"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                texture(1, wgpu::TextureSampleType::Depth),
                texture(2, wgpu::TextureSampleType::Uint),
                texture(3, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("contact shadows"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("contact_shadows.wgsl", include_str!("contact_shadows.wgsl"))) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("contact shadows"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let make_pipeline = |entry, format, blend, write_mask| crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let estimate = make_pipeline("estimate", HALF, None, wgpu::ColorWrites::ALL);
        let blur = make_pipeline("blur", HALF, None, wgpu::ColorWrites::ALL);
        // colour * (1 - alpha): the shader's alpha is the darkened share of the native colour.
        let composite = make_pipeline("composite", format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::REPLACE,
        }), wgpu::ColorWrites::COLOR);
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("contact shadow parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { layout, params, estimate, blur, composite, targets: None }
    }

    pub(super) fn release_targets(&mut self) { self.targets = None; }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, sun: &wgpu::TextureView, colour: &wgpu::TextureView, size: (u32, u32), vp: Mat4, eye: Vec3, toward_sun: Vec3, near: f32, far: f32, strength: f32) {
        let toward_sun = toward_sun.normalize_or_zero();
        if !strength.is_finite() || strength <= 0.0 || toward_sun == Vec3::ZERO || size.0 == 0 || size.1 == 0 { return; }
        if self.targets.as_ref().is_none_or(|targets| targets.size != size || targets.depth != *depth || targets.sun != *sun) {
            let create = |label| device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: Some(label), size: wgpu::Extent3d { width: size.0.div_ceil(2), height: size.1.div_ceil(2), depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HALF,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
            }).create_view(&Default::default());
            let raw = create("contact shadow raw");
            let filtered = create("contact shadow bilateral");
            let bind = |input| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("contact shadow frame inputs"), layout: &self.layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(input) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(sun) },
                ],
            });
            self.targets = Some(Targets { size, depth: depth.clone(), sun: sun.clone(), raw_bind: bind(&filtered), blur_bind: bind(&raw), raw, filtered });
        }
        let targets = self.targets.as_ref().expect("contact shadow targets allocated");
        // Reconstruct and march in camera-relative coordinates: f32 world
        // positions at playfield distances lose precision under tiny motion.
        // Form both transforms in f64 before uploading the relative f32 pair.
        let reference = eye.as_dvec3();
        let vp64 = vp.as_dmat4();
        let relative_vp = vp64 * glam::DMat4::from_translation(reference);
        let inverse_relative_vp = glam::DMat4::from_translation(-reference) * vp64.inverse();
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params {
            inverse_vp: inverse_relative_vp.as_mat4().to_cols_array_2d(), vp: relative_vp.as_mat4().to_cols_array_2d(), eye: [0.0; 4],
            sun: toward_sun.extend(RAY_LENGTH).to_array(), control: [strength.clamp(0.0, 1.0), near, far, FADE_FAR],
        }));
        for (label, pipeline, input, output, load) in [
            ("contact shadow march", &self.estimate, &targets.raw_bind, &targets.raw, wgpu::LoadOp::Clear(wgpu::Color::BLACK)),
            ("contact shadow bilateral blur", &self.blur, &targets.blur_bind, &targets.filtered, wgpu::LoadOp::Clear(wgpu::Color::BLACK)),
            ("contact shadow upsample/darken", &self.composite, &targets.raw_bind, colour, wgpu::LoadOp::Load),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, input, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub(super) fn target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |targets| Self::incremental_bytes(targets.size))
    }

    pub(super) fn incremental_bytes(size: (u32, u32)) -> u64 {
        u64::from(size.0.div_ceil(2)) * u64::from(size.1.div_ceil(2)) * 32 + std::mem::size_of::<Params>() as u64
    }
}

const HALF: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;

impl crate::Renderer {
    pub(super) fn encode_enhanced_contact_shadows(&mut self, encoder: &mut wgpu::CommandEncoder, vp: Mat4, eye: Vec3, toward_sun: Vec3, far: f32) {
        let settings = self.enhanced_settings;
        if !settings.contact_shadows || settings.contact_strength <= 0.0 { return; }
        let Some(frame) = self.enhanced_frame.as_ref() else { return; };
        let Some(sun) = frame.sun.as_ref() else { return; };
        let contact = self.contact_shadows.as_mut().expect("contact pipelines warmed at startup");
        contact.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, sun, &frame.colour, frame.size, vp, eye, toward_sun, self.lens.near, far, settings.contact_strength);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (96, 96);
    const SUN: Vec3 = Vec3::new(0.0, 0.7, -0.7);

    /// Ground plane + a wall the sun sits behind (+ optionally a small quad hidden behind that wall), then the real
    /// contact-shadow passes over a mid-grey colour target with a uniform sun-mask weight; returns the grey channel.
    fn run(strength: f32, sun_weight: f64, hidden_actor: bool, twice: bool) -> Vec<u8> {
        run_scene(strength, sun_weight, hidden_actor, twice, 0.0, false)
    }

    fn run_scene(strength: f32, sun_weight: f64, hidden_actor: bool, twice: bool, eye_shift: f32, phase_fixture: bool) -> Vec<u8> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("contact shadow regression needs an offscreen GPU");
        let eye = Vec3::new(eye_shift, 4.0, 4.0);
        let vp = Mat4::perspective_rh(0.9, 1.0, 0.1, 100.0) * Mat4::look_at_rh(eye, Vec3::new(eye_shift, 0.0, 0.0), Vec3::Y);
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("contact depth fixture"), source: wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var<uniform> vp: mat4x4<f32>;
@vertex fn vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) stage: u32) -> @builtin(position) vec4<f32> {
    var quads = array<array<vec3<f32>, 4>, 3>(
        array<vec3<f32>, 4>(vec3(-4.,0.,-4.), vec3(4.,0.,-4.), vec3(4.,0.,4.), vec3(-4.,0.,4.)),
        array<vec3<f32>, 4>(vec3(-1.,0.,0.), vec3(1.,0.,0.), vec3(1.,1.5,0.), vec3(-1.,1.5,0.)),
        array<vec3<f32>, 4>(vec3(-0.3,0.,-0.5), vec3(0.3,0.,-0.5), vec3(0.3,0.2,-0.5), vec3(-0.3,0.2,-0.5)));
    var corner = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    return vp * vec4<f32>(quads[stage][corner[vertex]], 1.0);
}
"#.into()) });
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }] });
        let matrix = r.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&vp.to_cols_array()), usage: wgpu::BufferUsages::UNIFORM });
        let bind = r.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: matrix.as_entire_binding() }] });
        let pipeline_layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("contact depth fixture"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] }, fragment: None,
            primitive: Default::default(), depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let texture = |format, usage| r.device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("contact fixture"), size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] });
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let depth = texture(wgpu::TextureFormat::Depth32Float, rt).create_view(&Default::default());
        let sun = texture(crate::enhanced::AUX, rt).create_view(&Default::default());
        let colour_texture = texture(r.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let colour = colour_texture.create_view(&Default::default());
        let mut contact = ContactShadows::new(&r.device, r.format);
        if phase_fixture {
            // Exercise the actual phase generator, blur and
            // upsample on the fixture's perspective-sloping ground. A complete
            // phase cycle must become a constant 0.5, not retain a grain tile.
            let source = format!("{}\n{}", crate::shader_source::load("contact_shadows.wgsl", include_str!("contact_shadows.wgsl")), r#"
@fragment fn phase_fixture(@builtin(position) position: vec4<f32>) -> @location(0) vec4<u32> {
    let pixel = half_pixel(vec2<i32>(position.xy));
    let z = depth_at(pixel);
    if z >= 1.0 { return encoded(0.0, z, pixel, vec3<f32>(0.0)); }
    let p = world(pixel, z);
    return encoded(phase_noise(position.xy), z, pixel, normal(pixel, p));
}
"#);
            let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("contact phase fixture"), source: wgpu::ShaderSource::Wgsl(source.into()) });
            let layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&contact.layout)], immediate_size: 0 });
            contact.estimate = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("contact phase fixture"), layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("phase_fixture"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: HALF, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
            });
        }
        let mut encoder = r.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("contact depth fixture"), color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..6, 0..if hidden_actor { 3 } else { 2 });
        }
        for _ in 0..if twice { 2 } else { 1 } {
            for (view, value) in [(&sun, sun_weight), (&colour, 0.5)] {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("contact fixture clear"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: value, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            }
            contact.encode(&r.device, &r.queue, &mut encoder, &depth, &sun, &colour, SIZE, vp, eye, SUN, 0.1, 100.0, strength);
        }
        r.queue.submit([encoder.finish()]);
        crate::texture_to_rgba(&r, &colour_texture, SIZE.0, SIZE.1).unwrap().chunks(4).map(|p| p[0]).collect()
    }

    #[test]
    fn neutral_without_strength_or_directional_light_and_only_darkens() {
        let lit = run(0.8, 1.0, false, false);
        assert!(run(0.0, 1.0, false, false).iter().all(|&v| v == 128), "strength 0 is exactly neutral");
        assert!(run(0.8, 0.0, false, false).iter().all(|&v| v == 128), "no directional share (fully shadowed, fogged or unlit): zero change");
        assert!(lit.iter().all(|&v| v <= 128), "contact shadows never brighten");
        let darker = lit.iter().filter(|&&v| v < 120).count();
        assert!(darker > 20, "the ground at the wall's foot must be visibly shadowed: {darker}");
        assert!(lit.iter().filter(|&&v| v == 128).count() > 96 * 96 / 2, "only the contact region changes");
        let half = run(0.4, 1.0, false, false);
        assert!(half.iter().zip(&lit).all(|(h, f)| h >= f), "strength scales the darkening");
        assert!(half.iter().zip(&lit).any(|(h, f)| h > f));
    }

    #[test]
    fn weak_night_light_casts_contacts_and_fog_attenuates_them() {
        let lit = run(0.8, 1.0, false, false);
        let night = run(0.8, 0.2, false, false);
        let fogged = run(0.8, 0.1, false, false);
        assert!(night.iter().any(|&v| v < 128), "weak directional light must not be discarded by the daylight exposure threshold");
        assert!(night.iter().zip(&lit).all(|(n, l)| n >= l), "weak light removes less colour than full light");
        assert!(fogged.iter().zip(&night).all(|(f, n)| f >= n), "native fog attenuation must reduce contact darkening");
        assert!(fogged.iter().zip(&night).any(|(f, n)| f > n));
        assert!(night[..SIZE.0 as usize].iter().all(|&v| v == 128), "sky stays unchanged even with a nonzero light mask");
    }

    #[test]
    fn hidden_actor_cannot_change_visible_pixels_and_static_frames_repeat() {
        for sun_weight in [1.0, 0.2] {
            assert_eq!(run(0.8, sun_weight, false, false), run(0.8, sun_weight, true, false), "wall-hidden geometry must not alter any pixel");
            assert_eq!(run(0.8, sun_weight, false, false), run(0.8, sun_weight, false, true), "fixed-pattern jitter: identical static frames");
        }
    }

    #[test]
    fn full_phase_period_is_smooth_on_perspective_ground() {
        for shift in [0.0, 0.001, -0.001] {
            let image = run_scene(0.8, 1.0, false, false, shift, true);
            // Below the wall and away from the ground perimeter: one plane,
            // with substantial view-depth change across each filter footprint.
            for y in 64..80 {
                for x in 16..80 {
                    let value = image[y * SIZE.0 as usize + x];
                    assert!((76..=78).contains(&value), "full-period phase mean must be 0.5 on sloping ground: ({x}, {y}), shift {shift}, value {value}");
                }
            }
        }
    }

    #[test]
    fn micro_camera_motion_keeps_contact_darkening_stable() {
        let still = run_scene(0.8, 1.0, false, false, 0.0, false);
        for shift in [0.001, -0.001] {
            let moved = run_scene(0.8, 1.0, false, false, shift, false);
            let difference: u64 = still.iter().zip(&moved).map(|(&a, &b)| u64::from(a.abs_diff(b))).sum();
            let mean = difference as f64 / still.len() as f64;
            assert!(mean < 0.5, "millimetre camera translation must not reshuffle contact grain: mean byte delta {mean}, shift {shift}");
            assert!(moved.iter().filter(|&&v| v < 120).count() > 20, "stability must retain the wall contact");
        }
    }

    #[test]
    fn half_resolution_memory_covers_odd_extents() {
        assert_eq!(ContactShadows::incremental_bytes((3, 5)), 2 * 3 * 32 + std::mem::size_of::<Params>() as u64);
    }
}
