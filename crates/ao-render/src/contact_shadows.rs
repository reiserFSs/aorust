//! Half-resolution screen-space contact shadows (port of ao-vk `contact.frag`, MIT; see THIRD_PARTY). Inputs are exclusively
//! the current frame's resolved primary-view depth and the world pass's sun mask; the result only darkens the sun's direct share.
use crate::gpu_stats::GpuStatsExt;
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
    estimate: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
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
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("contact shadow inputs"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                texture(1, wgpu::TextureSampleType::Depth),
                texture(2, wgpu::TextureSampleType::Float { filterable: false }),
                texture(3, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("contact shadows"), source: wgpu::ShaderSource::Wgsl(include_str!("contact_shadows.wgsl").into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("contact shadows"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let make_pipeline = |entry, format, blend, write_mask| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let estimate = make_pipeline("estimate", HALF, None, wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN);
        let blur = make_pipeline("blur", HALF, None, wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN);
        // colour * (1 - alpha): the shader's alpha is the darkened share of the native colour.
        let composite = make_pipeline("composite", format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::REPLACE,
        }), wgpu::ColorWrites::COLOR);
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("contact shadow parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { layout, params, estimate, blur, composite, targets: None }
    }

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
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params {
            inverse_vp: vp.inverse().to_cols_array_2d(), vp: vp.to_cols_array_2d(), eye: eye.extend(0.0).to_array(),
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
        u64::from(size.0.div_ceil(2)) * u64::from(size.1.div_ceil(2)) * 16 + std::mem::size_of::<Params>() as u64
    }
}

const HALF: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;

impl crate::Renderer {
    pub(super) fn encode_enhanced_contact_shadows(&mut self, encoder: &mut wgpu::CommandEncoder, vp: Mat4, eye: Vec3, toward_sun: Vec3, far: f32) {
        let settings = self.enhanced_settings;
        if !settings.contact_shadows || settings.contact_strength <= 0.0 { return; }
        let Some(frame) = self.enhanced_frame.as_ref() else { return; };
        let Some(sun) = frame.sun.as_ref() else { return; };
        let contact = self.contact_shadows.get_or_insert_with(|| ContactShadows::new(&self.device, self.format));
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("contact shadow regression needs an offscreen GPU");
        let eye = Vec3::new(0.0, 4.0, 4.0);
        let vp = Mat4::perspective_rh(0.9, 1.0, 0.1, 100.0) * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
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
    fn neutral_without_strength_or_sun_and_only_darkens() {
        let lit = run(0.8, 1.0, false, false);
        assert!(run(0.0, 1.0, false, false).iter().all(|&v| v == 128), "strength 0 is exactly neutral");
        assert!(run(0.8, 0.0, false, false).iter().all(|&v| v == 128), "no sun weight (night, shadow, fog, sky, unlit): zero change");
        assert!(lit.iter().all(|&v| v <= 128), "contact shadows never brighten");
        let darker = lit.iter().filter(|&&v| v < 120).count();
        assert!(darker > 20, "the ground at the wall's foot must be visibly shadowed: {darker}");
        assert!(lit.iter().filter(|&&v| v == 128).count() > 96 * 96 / 2, "only the contact region changes");
        let half = run(0.4, 1.0, false, false);
        assert!(half.iter().zip(&lit).all(|(h, f)| h >= f), "strength scales the darkening");
        assert!(half.iter().zip(&lit).any(|(h, f)| h > f));
    }

    #[test]
    fn hidden_actor_cannot_change_visible_pixels_and_static_frames_repeat() {
        assert_eq!(run(0.8, 1.0, false, false), run(0.8, 1.0, true, false), "wall-hidden geometry must not alter any pixel");
        assert_eq!(run(0.8, 1.0, false, false), run(0.8, 1.0, false, true), "fixed-pattern jitter: identical static frames");
    }

    #[test]
    fn half_resolution_memory_covers_odd_extents() {
        assert_eq!(ContactShadows::incremental_bytes((3, 5)), 2 * 3 * 16 + std::mem::size_of::<Params>() as u64);
    }
}
