//! Opt-in sand, ash and red/green fallout driven only by native weather f[3..=6].
//! Procedural wrapped particle boxes adapt ao-vk rain.vert/rain_common.glsl (MIT).
use crate::effect_gpu::EffectPassEncoder as _;
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use glam::{Mat4, Vec3};

const PER_KIND: u32 = 2048;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    vp: [[f32; 4]; 4],
    eye: [f32; 4],
    drift: [f32; 4],
    intensity: [f32; 4],
    viewport: [f32; 4],
    fog: [f32; 4],
    light: [f32; 4],
    shelter: [f32; 4],
}

fn intensities(values: [f32; 4], outdoor: bool) -> [f32; 4] {
    values.map(|v| if outdoor && v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })
}

/// No actor sources, history textures, generated assets or per-particle CPU buffers.
pub(super) struct WeatherParticles {
    pipeline: RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    bindings: Option<(wgpu::TextureView, wgpu::TextureView, wgpu::BindGroup)>,
    last_time: Option<f32>,
    drift: Vec3,
}
impl WeatherParticles {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("native storm particles"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
        ] });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sand ash fallout"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("weather_particles.wgsl", include_str!("weather_particles.wgsl"))) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some("native storm particles"), layout: Some(&pl),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN | wgpu::ColorWrites::BLUE })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("storm particle frame"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { pipeline, layout, params, bindings: None, last_time: None, drift: Vec3::ZERO }
    }
    pub(super) fn is_ready(&self) -> bool { self.pipeline.is_ready() }
    pub(super) fn wait(&self) { self.pipeline.get(); }
    pub(super) fn release_targets(&mut self) { self.bindings = None; self.last_time = None; self.drift = Vec3::ZERO; }
    pub(super) fn target_bytes(&self) -> u64 { 0 }

    /// `shelter`: R absolute roof height (negative sentinel = open), G absolute ground.
    /// Transform is corner x/z and reciprocal world extent. Depth is current admitted depth.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView, colour: &wgpu::TextureView, size: (u32, u32), vp: Mat4, eye: Vec3, time: f32,
        env: &ao_scene::Environment, intensity: [f32; 4], wind: [f32; 3], outdoor: bool,
        shelter: &wgpu::TextureView, shelter_transform: [f32; 4]) {
        let intensity = intensities(intensity, outdoor);
        if !time.is_finite() || intensity == [0.0; 4] || size.0 == 0 || size.1 == 0 {
            self.last_time = None;
            return;
        }
        let dt = self.last_time.map_or(0.0, |old| (time - old).clamp(0.0, 0.25));
        self.last_time = Some(time);
        // Integrating wind, rather than wind*time, avoids teleporting the field on native wind turns.
        let velocity = Vec3::from(wind.map(|v| if v.is_finite() { v } else { 0.0 }));
        self.drift += velocity * dt;
        // 64 is the wrapping box period, so keeping offsets bounded loses no spatial phase.
        self.drift = Vec3::from(self.drift.to_array().map(|v| v.rem_euclid(64.0)));
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params {
            vp: vp.to_cols_array_2d(), eye: eye.extend(time).to_array(), drift: self.drift.extend(0.0).to_array(), intensity,
            viewport: [size.0 as f32, size.1 as f32, 0.0, 0.0], fog: [env.fog_start, env.fog_end, 0.0, 0.0],
            light: [env.ambient[0] + env.sun_color[0] * 0.25, env.ambient[1] + env.sun_color[1] * 0.25, env.ambient[2] + env.sun_color[2] * 0.25, 0.0], shelter: shelter_transform,
        }));
        if !self.bindings.as_ref().is_some_and(|(d, s, _)| d == depth && s == shelter) {
            let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("current storm particle inputs"), layout: &self.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(shelter) },
            ] });
            self.bindings = Some((depth.clone(), shelter.clone(), bind));
        }
        let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor { label: Some("sand ash red green fallout"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: colour, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings.as_ref().unwrap().2, &[]);
        for (kind, strength) in intensity.into_iter().enumerate() {
            if strength > 0.0 {
                let first = kind as u32 * PER_KIND;
                pass.draw(0..6, first..first + PER_KIND);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weather_particles_preserve_exact_native_channels_and_indoor_exclusion() {
        for kind in 0..4 {
            let mut native = [0.0; 4]; native[kind] = 0.375;
            assert_eq!(intensities(native, true), native);
            assert_eq!(intensities(native, false), [0.0; 4]);
        }
        assert_eq!(intensities([f32::NAN, -1.0, 2.0, f32::INFINITY], true), [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(std::mem::size_of::<Params>(), 176);
    }

    #[test]
    fn weather_particles_gpu_roof_fog_depth_and_native_kinds() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("storm regression needs an offscreen GPU");
        let mut particles = WeatherParticles::new(&r.device, r.format);
        particles.wait();
        let size = (128, 128);
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |format, usage, width, height| r.device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("native storm regression"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] });
        let depth = texture(wgpu::TextureFormat::Depth32Float, rt, size.0, size.1).create_view(&Default::default());
        let colour_texture = texture(r.format, rt | wgpu::TextureUsages::COPY_SRC, size.0, size.1);
        let colour = colour_texture.create_view(&Default::default());
        let shelter_texture = texture(wgpu::TextureFormat::Rgba32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, 1, 1);
        let shelter = shelter_texture.create_view(&Default::default());
        let env = ao_scene::Environment { sky_color: [0.2; 3], fog_color: [0.2; 3], fog_start: 80.0, fog_end: 100.0, ambient: [1.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0 };
        let vp = Mat4::perspective_rh(1.5, 1.0, 0.1, 100.0);
        let mut baseline = None;
        for frame in 0..10 {
            let roof = if frame == 6 { 1000.0 } else { -10000.0 };
            r.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &shelter_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&[roof, -10000.0_f32, 0.0, 0.0]), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16), rows_per_image: Some(1) }, wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });
            let mut encoder = r.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("storm fixture clear"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &colour, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(if frame == 8 { 0.0 } else { 1.0 }), store: wgpu::StoreOp::Store }), stencil_ops: None }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            }
            let mut intensity = [0.0; 4];
            if frame != 0 { intensity[if frame <= 4 { frame - 1 } else { 0 }] = 1.0; }
            let mut environment = env;
            if frame == 7 { environment.fog_start = 0.0; environment.fog_end = 0.01; }
            particles.encode(&r.device, &r.queue, &mut encoder, &depth, &colour, size, vp, Vec3::ZERO, 1.0,
                &environment, intensity, [2.0, 0.0, -1.0], frame != 5, &shelter, if frame == 9 { [0.0; 4] } else { [-64.0, -64.0, 1.0 / 128.0, 0.0] });
            r.queue.submit([encoder.finish()]);
            let pixels = crate::texture_to_rgba(&r, &colour_texture, size.0, size.1).expect("storm regression readback");
            if frame == 0 { baseline = Some(pixels); }
            else if frame <= 4 { assert_ne!(Some(&pixels), baseline.as_ref(), "native storm kind {frame} drew no particles"); }
            else { assert_eq!(Some(&pixels), baseline.as_ref(), "storm escaped indoor/roof/fog/depth/missing-shelter exclusion in case {frame}"); }
        }
    }
}
