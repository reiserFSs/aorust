//! World-anchored opt-in rain and collision-sheltered wet receivers.
//! Rain anchoring/splash concepts adapted from ao-vk rvk/rain.cpp and rain.vert (MIT).
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use glam::{Mat4, Vec3};

pub type WeatherSurface = std::rc::Rc<dyn Fn([f32; 3]) -> Option<[f32; 2]>>;
#[derive(Clone, Default)]
pub struct WeatherInput {
    pub rain: f32,
    pub storm: [f32; 4],
    pub wind: [f32; 3],
    pub outdoor: bool,
    /// Native collision roof test: roof top and ground height; unknown fails closed.
    pub surface: Option<WeatherSurface>,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse_vp: [[f32; 4]; 4], vp: [[f32; 4]; 4], eye_time: [f32; 4],
    wind_rain: [f32; 4], area: [f32; 4], wet: [f32; 4],
    sun: [f32; 4], fog: [f32; 4], colour: [f32; 4],
    drift: [f32; 4],
}
pub(super) struct RainWet {
    layout: wgpu::BindGroupLayout, params: wgpu::Buffer,
    streaks: RenderPipeline, splashes: RenderPipeline, wet: RenderPipeline, sheen: RenderPipeline,
    pub(super) shelter: wgpu::TextureView, texture: wgpu::Texture,
    pub(super) area: [f32; 4], samples: Vec<[f32; 2]>,
    wetness: f32, last_time: Option<f32>,
    surface: Option<WeatherSurface>,
    drift: Vec3,
    bindings: Option<Bindings>,
}
struct Bindings { depth: wgpu::TextureView, mask: wgpu::TextureView, bind: wgpu::BindGroup }
const MAP: u32 = 128;
impl RainWet {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let tex = |binding, sample_type| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("rain wet inputs"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            tex(1, wgpu::TextureSampleType::Depth), tex(2, wgpu::TextureSampleType::Float { filterable: false }), tex(3, wgpu::TextureSampleType::Float { filterable: false }),
        ] });
        let source = crate::shader_source::load("rain_wet.wgsl", include_str!("rain_wet.wgsl"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("world rain wet surfaces"), source: wgpu::ShaderSource::Wgsl(source) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("rain wet"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let make = |vertex, entry, blend| crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor { label: Some(entry), layout: Some(&pl), vertex: wgpu::VertexState { module: &shader, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &[] }, fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::COLOR })] }), primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() }, depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None });
        let streaks = make("rain_vertex", "rain_streak", wgpu::BlendState::ALPHA_BLENDING);
        let splashes = make("splash_vertex", "splash_fragment", wgpu::BlendState::ALPHA_BLENDING);
        let wet = make("vs", "wet", wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE });
        let sheen = make("vs", "sheen", wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("rain wet parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let texture = device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("native collision weather roof map"), size: wgpu::Extent3d { width: MAP, height: MAP, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rg32Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        let shelter = texture.create_view(&Default::default());
        Self { layout, params, streaks, splashes, wet, sheen, shelter, texture, area: [0.0; 4], samples: vec![[1e20, 0.0]; (MAP * MAP) as usize], wetness: 0.0, last_time: None, surface: None, drift: Vec3::ZERO, bindings: None }
    }
    pub(super) fn is_ready(&self) -> bool { self.streaks.is_ready() && self.splashes.is_ready() && self.wet.is_ready() && self.sheen.is_ready() }
    pub(super) fn wait(&self) { self.streaks.get(); self.splashes.get(); self.wet.get(); self.sheen.get(); }
    pub(super) fn release_targets(&mut self) { self.bindings = None; }
    pub(super) fn prepare_shelter(&mut self, queue: &wgpu::Queue, eye: Vec3, input: &WeatherInput) {
        let area = [(eye.x / 8.0).floor() * 8.0 - 32.0, (eye.z / 8.0).floor() * 8.0 - 32.0, 1.0 / 64.0, 0.0];
        let same_surface = match (&self.surface, &input.surface) { (Some(a), Some(b)) => std::rc::Rc::ptr_eq(a, b), (None, None) => true, _ => false };
        if area == self.area && same_surface { return; }
        if !same_surface { self.last_time = None; self.wetness = 0.0; self.drift = Vec3::ZERO; }
        self.area = area;
        self.surface = input.surface.clone();
        for z in 0..MAP { for x in 0..MAP {
            let p = [self.area[0] + (x as f32 + 0.5) * 0.5, eye.y, self.area[1] + (z as f32 + 0.5) * 0.5];
            self.samples[(z * MAP + x) as usize] = input.surface.as_ref().and_then(|query| query(p)).filter(|v| v.iter().all(|x| x.is_finite())).unwrap_or([1e20, 0.0]);
        } }
        queue.write_texture(self.texture.as_image_copy(), bytemuck::cast_slice(&self.samples), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(MAP * 8), rows_per_image: Some(MAP) }, wgpu::Extent3d { width: MAP, height: MAP, depth_or_array_layers: 1 });
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, mask: &wgpu::TextureView, colour: &wgpu::TextureView, vp: Mat4, eye: Vec3, time: f32, env: ao_scene::Environment, input: &WeatherInput, rain: bool, wet: bool) {
        if self.last_time.is_none() { self.wetness = input.rain.clamp(0.0, 1.0) * 0.3; }
        let dt = self.last_time.map_or(0.0, |old| (time - old).clamp(0.0, 0.25)); self.last_time = Some(time);
        self.wetness = advance_wetness(self.wetness, input.rain, dt);
        self.drift += Vec3::new(input.wind[0], -18.0, input.wind[2]) * dt;
        if !input.outdoor || !vp.is_finite() || vp.determinant().abs() < 1e-12 { return; }
        let params = Params { inverse_vp: vp.inverse().to_cols_array_2d(), vp: vp.to_cols_array_2d(), eye_time: eye.extend(time).to_array(), wind_rain: [input.wind[0], input.wind[1], input.wind[2], input.rain.clamp(0.0, 1.0)], area: self.area, wet: [self.wetness, 0.0, 0.0, 0.0], sun: Vec3::from(env.sun_dir).extend(0.0).to_array(), fog: [env.fog_start, env.fog_end, 0.0, 0.0], colour: [env.ambient[0], env.ambient[1], env.ambient[2], 0.0], drift: self.drift.extend(0.0).to_array() };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        if !self.bindings.as_ref().is_some_and(|b| b.depth == *depth && b.mask == *mask) {
            let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("rain wet frame"), layout: &self.layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) }, wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(mask) }, wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.shelter) }] });
            self.bindings = Some(Bindings { depth: depth.clone(), mask: mask.clone(), bind });
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("rain and wet receivers"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: colour, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
        pass.set_bind_group(0, &self.bindings.as_ref().expect("rain inputs prepared").bind, &[]);
        if wet && self.wetness > 0.0 { pass.set_pipeline(&self.wet); pass.draw(0..3, 0..1); pass.set_pipeline(&self.sheen); pass.draw(0..3, 0..1); }
        if rain && input.rain > 0.0 {
            pass.set_pipeline(&self.streaks); pass.draw(0..6, 0..(12000.0 * input.rain.clamp(0.0, 1.0)) as u32);
            pass.set_pipeline(&self.splashes); pass.draw(0..6, 0..(900.0 * input.rain.clamp(0.0, 1.0)) as u32);
        }
    }
}

impl crate::Renderer {
    pub(super) fn encode_weather(&mut self, encoder: &mut wgpu::CommandEncoder, vp: Mat4, eye: Vec3, env: ao_scene::Environment) {
        let settings = self.enhanced_settings;
        if !settings.enhanced_rain && !settings.wet_surfaces && !settings.weather_particles { self.rain_wet = None; self.weather_particles = None; return; }
        let Some(frame) = self.enhanced_frame.as_ref() else { return; };
        let pass = self.rain_wet.get_or_insert_with(|| RainWet::new(&self.device, self.format));
        if !pass.is_ready() { return; }
        pass.prepare_shelter(&self.queue, eye, &self.weather_input);
        pass.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, frame.sun.as_ref().expect("weather admitted receiver mask"), &frame.colour, vp, eye, self.time, env, &self.weather_input, settings.enhanced_rain, settings.wet_surfaces);
        if settings.weather_particles {
            let particles = self.weather_particles.get_or_insert_with(|| crate::weather_particles::WeatherParticles::new(&self.device, self.format));
            if !particles.is_ready() { return; }
            particles.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, &frame.colour, frame.size, vp, eye, self.time, &env, self.weather_input.storm, self.weather_input.wind, self.weather_input.outdoor, &pass.shelter, pass.area);
        } else { self.weather_particles = None; }
    }
}
fn advance_wetness(wet: f32, rain: f32, dt: f32) -> f32 { (wet + (rain.clamp(0.0, 1.0) * 0.12 - 0.002) * dt).clamp(0.0, 1.0) }
#[cfg(test)]
mod tests {
    #[test] fn wetness_accumulates_and_drains_after_rain() { let wet = super::advance_wetness(0.2, 1.0, 1.0); assert!(wet > 0.2); let drying = super::advance_wetness(wet, 0.0, 1.0); assert!(drying > 0.0 && drying < wet); assert_eq!(super::advance_wetness(0.0, 0.0, 1.0), 0.0); }
    #[test]
    fn weather_switches_are_independent_and_default_off() {
        let defaults = crate::EnhancedSettings::default();
        assert!(!defaults.any_enabled());
        for enabled in [
            crate::EnhancedSettings { enhanced_rain: true, ..defaults },
            crate::EnhancedSettings { wet_surfaces: true, ..defaults },
            crate::EnhancedSettings { weather_particles: true, ..defaults },
            crate::EnhancedSettings { gpu_spell_particles: true, ..defaults },
            crate::EnhancedSettings { fluid_smoke: true, ..defaults },
        ] {
            assert!(enabled.any_enabled() && enabled.needs_sun_mask());
            assert_eq!(enabled.clamped(), enabled);
        }
    }

    #[test]
    fn rain_wet_gpu_pipeline_and_collision_map() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("weather GPU regression");
        let mut pass = super::RainWet::new(&r.device, r.format);
        pass.wait();
        assert!(pass.is_ready());
        let open = super::WeatherInput { outdoor: true, surface: Some(std::rc::Rc::new(|_| Some([-1e20, 2.0]))), ..Default::default() };
        pass.prepare_shelter(&r.queue, glam::Vec3::ZERO, &open);
        assert!(pass.samples.iter().all(|v| *v == [-1e20, 2.0]));
        pass.prepare_shelter(&r.queue, glam::Vec3::ZERO, &super::WeatherInput::default());
        assert!(pass.samples.iter().all(|v| v[0] > 1e19));
    }
}
