//! Opt-in native-weather volumes. Original cloud sheets remain the baseline.
//! ao-vk (MIT) cloud_layer.frag/cloud_resolve.frag/cloud_shadow.frag provide
//! height profiles, erosion, extinction and rotation-only checker reprojection.
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use ao_scene::cloud::CloudLayer;
use glam::{Mat4, Vec3};

pub(super) const CLOUD_DENSITY_WGSL: &str = include_str!("cloud_density.wgsl");
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct CloudDensityLayer {
    pub(super) shape: [f32; 4],
    pub(super) drift: [f32; 4],
    pub(super) light: [f32; 4],
    pub(super) sun: [f32; 4],
    pub(super) ambient: [f32; 4],
}

/// Matches native sheets' far/1000 sky scaling and AO-to-scene wind handedness.
/// Enhanced-only finite-height profile; it does not replace native weather.
pub(super) fn density_layer(layer: &CloudLayer, eye: Vec3, far: f32, time: f32) -> CloudDensityLayer {
    let scale = far / 1000.0;
    let storm = layer.storm.clamp(0.0, 1.0);
    let thick = layer.thick.clamp(0.0, 1.0);
    let thickness = (35.0 + thick * 120.0 + storm * 180.0) * scale;
    let drift = Vec3::new(layer.wind[0], layer.wind[1], -layer.wind[2]) * (time * 100.0 * scale);
    CloudDensityLayer {
        shape: [layer.altitude * scale, thickness.max(1.0), (0.65 + thick * 0.25 + storm * 0.1).clamp(0.0, 1.0), 5.0 / thickness.max(1.0)],
        drift: [eye.x - drift.x, -drift.y, eye.z - drift.z, 0.0],
        light: [layer.light[0], layer.light[1], layer.light[2], storm],
        sun: [layer.light_direction[0], layer.light_direction[1], layer.light_direction[2], thick],
        ambient: [layer.ambient[0], layer.ambient[1], layer.ambient[2], 0.0],
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse_vp: [[f32; 4]; 4],
    previous_vp: [[f32; 4]; 4],
    eye: [f32; 4],
    control: [f32; 4],
    enabled: [f32; 4],
    fog: [f32; 4],
    fog_colour: [f32; 4],
    shadow_area: [f32; 4],
    shadow_sun: [f32; 4],
}
pub(super) struct VolumetricClouds {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    layers: wgpu::Buffer,
    capacity: usize,
    prepared: Vec<CloudDensityLayer>,
    estimate: RenderPipeline,
    composite: RenderPipeline,
    cloud_shadows: crate::cloud_shadows::CloudShadows,
    shadow_fallback: wgpu::TextureView,
    sky_composite: RenderPipeline,
    targets: Option<Targets>,
    previous_vp: Mat4,
    previous_eye: Vec3,
    previous_fog: [f32; 6],
    previous_time: f32,
    previous_layers: Vec<CloudLayer>,
    phase: u32,
}
struct Targets {
    size: (u32, u32),
    depth: wgpu::TextureView,
    sun: wgpu::TextureView,
    shadow: wgpu::TextureView,
    history: [wgpu::TextureView; 2],
    binds: [wgpu::BindGroup; 2],
    current: usize,
    valid: bool,
}
const HALF: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
impl VolumetricClouds {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("native cloud inputs"), entries: &[
            buffer(0, wgpu::BufferBindingType::Uniform), buffer(1, wgpu::BufferBindingType::Storage { read_only: true }),
            texture(2, wgpu::TextureSampleType::Depth), texture(3, wgpu::TextureSampleType::Float { filterable: false }), texture(4, wgpu::TextureSampleType::Float { filterable: false }),
            texture(5, wgpu::TextureSampleType::Float { filterable: false }),
        ] });
        let source = format!("{}\n{}", crate::shader_source::load("cloud_density.wgsl", CLOUD_DENSITY_WGSL), crate::shader_source::load("volumetric_clouds.wgsl", include_str!("volumetric_clouds.wgsl")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native weather volumes"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("native clouds"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let make = |entry, format, blend, write_mask| crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let estimate = make("estimate", HALF, None, wgpu::ColorWrites::ALL);
        let composite = make("composite", format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE,
        }), wgpu::ColorWrites::COLOR);
        let cloud_shadows = crate::cloud_shadows::CloudShadows::new(device);
        let shadow_fallback = device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("unused cloud field"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R16Float, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
        let sky_composite = make("sky_composite", format, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), wgpu::ColorWrites::COLOR);
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("cloud parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let layers = Self::layer_buffer(device, 1);
        Self { layout, params, layers, capacity: 1, prepared: Vec::new(), estimate, composite, cloud_shadows, shadow_fallback, sky_composite, targets: None, previous_vp: Mat4::IDENTITY, previous_eye: Vec3::ZERO, previous_time: f32::NAN, previous_layers: Vec::new(), previous_fog: [f32::NAN; 6], phase: 0 }
    }
    fn layer_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("native cloud layers"), size: (capacity * std::mem::size_of::<CloudDensityLayer>()) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }
    pub(super) fn is_ready(&self) -> bool { self.estimate.is_ready() && self.composite.is_ready() && self.cloud_shadows.is_ready() && self.sky_composite.is_ready() }
    pub(super) fn wait(&self) { self.estimate.get(); self.composite.get(); self.cloud_shadows.wait(); self.sky_composite.get(); }
    pub(super) fn release_targets(&mut self) { self.targets = None; self.cloud_shadows.release_targets(); }
    pub(super) fn target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |t| u64::from(t.size.0.div_ceil(2)) * u64::from(t.size.1.div_ceil(2)) * 16)
            + self.layers.size() + self.params.size() + self.cloud_shadows.target_bytes() + 2
    }
    fn upload_layers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, eye: Vec3, layers: &[CloudLayer], time: f32, far: f32) -> bool {
        self.prepared.clear();
        if layers.is_empty() || !time.is_finite() || !far.is_finite() || far <= 0.0 || !eye.is_finite() { return false; }
        self.prepared.extend(layers.iter().map(|layer| density_layer(layer, eye, far, time)));
        if bytemuck::cast_slice::<CloudDensityLayer, f32>(&self.prepared).iter().any(|v| !v.is_finite()) { return false; }
        if layers.len() > self.capacity {
            self.capacity = layers.len().next_power_of_two();
            self.layers = Self::layer_buffer(device, self.capacity);
            self.targets = None;
        }
        queue.write_buffer(&self.layers, 0, bytemuck::cast_slice(&self.prepared));
        true
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_cloud_field(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, eye: Vec3, layers: &[CloudLayer], time: f32, far: f32) {
        if self.upload_layers(device, queue, eye, layers, time, far) {
            self.cloud_shadows.prepare(device, queue, encoder, &self.layers, &self.prepared, eye, far);
        } else {
            self.cloud_shadows.release_targets();
        }
    }
    pub(super) fn cloud_field(&self) -> Option<crate::cloud_shadows::CloudShadowField> { self.cloud_shadows.field() }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, sun: &wgpu::TextureView, colour: &wgpu::TextureView, size: (u32, u32), vp: Mat4, eye: Vec3, layers: &[CloudLayer], time: f32, far: f32, clouds: bool, shadows: bool, env: ao_scene::Environment) {
        if (!clouds && !shadows) || layers.is_empty() || size.0 == 0 || size.1 == 0 || !time.is_finite() || !far.is_finite() || far <= 0.0 || !vp.is_finite() || !eye.is_finite() {
            if let Some(t) = &mut self.targets { t.valid = false; }
            return;
        }
        if !self.upload_layers(device, queue, eye, layers, time, far) { return; }
        if shadows { self.cloud_shadows.prepare(device, queue, encoder, &self.layers, &self.prepared, eye, far); }
        let field = self.cloud_shadows.field();
        let shadow = field.as_ref().map_or(&self.shadow_fallback, |field| &field.view);
        if self.targets.as_ref().is_none_or(|t| t.size != size || t.depth != *depth || t.sun != *sun || t.shadow != *shadow) {
            let create = || device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("cloud-only temporal checker"), size: wgpu::Extent3d { width: size.0.div_ceil(2), height: size.1.div_ceil(2), depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HALF, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
            let history = [create(), create()];
            let bind = |view| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("cloud frame inputs"), layout: &self.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: self.layers.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) }, wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(sun) }, wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(shadow) },
            ] });
            let binds = [bind(&history[0]), bind(&history[1])];
            self.targets = Some(Targets { size, depth: depth.clone(), sun: sun.clone(), shadow: shadow.clone(), history, binds, current: 0, valid: false });
        }
        let t = self.targets.as_mut().expect("cloud targets allocated");
        let fog = [env.fog_start, env.fog_end, env.fog_color[0], env.fog_color[1], env.fog_color[2], far];
        let weather_matches = self.previous_layers.len() == layers.len() && self.previous_layers.iter().zip(layers).all(|(a, b)| a.altitude == b.altitude && a.wind == b.wind && a.storm == b.storm && a.thick == b.thick);
        let valid = t.valid && weather_matches && self.previous_fog == fog && time >= self.previous_time && time - self.previous_time < 0.25 && eye.distance(self.previous_eye) < 10.0;
        let relative_inverse = glam::DMat4::from_translation(-eye.as_dvec3()) * vp.as_dmat4().inverse();
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params { inverse_vp: relative_inverse.as_mat4().to_cols_array_2d(), previous_vp: self.previous_vp.to_cols_array_2d(), eye: eye.extend(0.0).to_array(), control: [layers.len() as f32, far * 2.0, self.phase as f32, if valid { 1.0 } else { 0.0 }], enabled: [if clouds { 1.0 } else { 0.0 }, if shadows { 1.0 } else { 0.0 }, self.prepared.iter().map(|l| l.shape[0]).fold(f32::INFINITY, f32::min), 0.0], fog: [env.fog_start, env.fog_end, 0.0, 0.0], fog_colour: [env.fog_color[0], env.fog_color[1], env.fog_color[2], 0.0], shadow_area: field.as_ref().map_or([0.0; 4], |f| f.area), shadow_sun: field.as_ref().map_or([0.0; 4], |f| f.sun) }));
        let next = 1 - t.current;
        if clouds {
            Self::draw(encoder, &self.estimate, &t.binds[t.current], &t.history[next], wgpu::LoadOp::Clear(wgpu::Color::WHITE));
            t.current = next;
            Self::draw(encoder, &self.sky_composite, &t.binds[t.current], colour, wgpu::LoadOp::Load);
        }
        if shadows { Self::draw(encoder, &self.composite, &t.binds[t.current], colour, wgpu::LoadOp::Load); }
        t.valid = clouds;
        self.previous_vp = vp;
        self.previous_eye = eye;
        self.previous_time = time;
        self.previous_fog = fog;
        self.previous_layers.clear();
        self.previous_layers.extend_from_slice(layers);
        self.phase = (self.phase + 1) & 3;
    }
    fn draw(encoder: &mut wgpu::CommandEncoder, pipeline: &RenderPipeline, bind: &wgpu::BindGroup, output: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("native cloud atmosphere"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
        pass.set_pipeline(pipeline); pass.set_bind_group(0, bind, &[]); pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_layer_scale_wind_and_layout() {
        let native = CloudLayer { mesh: 0, altitude: 100.0, wind: [0.01, 0.0, 0.02], storm: 0.5, thick: 0.4, sun_y: 0.2, light: [0.8, 0.3, 0.1], light_direction: [0.0, 1.0, 0.0], ambient: [0.1; 3] };
        let a = density_layer(&native, Vec3::new(20.0, 40.0, 30.0), 1000.0, 2.0);
        assert_eq!(a.shape[0], 100.0);
        assert_eq!(a.drift, [18.0, 0.0, 34.0, 0.0]);
        assert_eq!(a.light, [0.8, 0.3, 0.1, 0.5]);
        assert_eq!(std::mem::size_of::<CloudDensityLayer>(), 80);
        assert_eq!(std::mem::size_of::<Params>() % 16, 0);
    }
    #[test]
    fn checker_visits_every_subpixel() {
        let mut visited = [false; 4];
        for phase in 0..4u32 { visited[((phase & 1) | (((phase >> 1) & 1) << 1)) as usize] = true; }
        assert!(visited.into_iter().all(|v| v));
    }
    #[test]
    fn cloud_foreground_and_empty_layers_are_unchanged() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("cloud regression needs an offscreen GPU");
        let mut clouds = VolumetricClouds::new(&r.device, r.format);
        clouds.wait();
        let size = (16, 16);
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |format, usage| r.device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("cloud foreground regression"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] });
        let depth = texture(wgpu::TextureFormat::Depth32Float, rt).create_view(&Default::default());
        let sun = texture(crate::enhanced::AUX, rt).create_view(&Default::default());
        let colour_texture = texture(r.format, rt | wgpu::TextureUsages::COPY_SRC);
        let colour = colour_texture.create_view(&Default::default());
        let native = CloudLayer { mesh: 0, altitude: 100.0, wind: [0.01, 0.0, 0.02], storm: 0.5, thick: 1.0, sun_y: 0.8, light: [0.8, 0.3, 0.1], light_direction: [0.0, 1.0, 0.0], ambient: [0.1; 3] };
        let env = ao_scene::Environment { sky_color: [0.2; 3], fog_color: [0.2; 3], fog_start: 80.0, fog_end: 100.0, ambient: [0.1; 3], sun_color: [0.8; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0 };
        let vp = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0) * Mat4::look_at_rh(Vec3::ZERO, Vec3::Y, Vec3::Z);
        let mut baseline = None;
        // First capture the empty-layer baseline; then run a full checker cycle
        // with a real native layer. Admitted world coverage excludes the cloud
        // composite and zero directional share excludes shadows.
        for frame in 0..5 {
            let mut encoder = r.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("cloud fixture clear"), color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment { view: &colour, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.2, g: 0.3, b: 0.4, a: 1.0 }), store: wgpu::StoreOp::Store } }),
                ], depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.5), store: wgpu::StoreOp::Store }), stencil_ops: None }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            }
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("world coverage with zero directional share"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &sun, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            }
            let layers = if frame == 0 { &[][..] } else { std::slice::from_ref(&native) };
            clouds.encode(&r.device, &r.queue, &mut encoder, &depth, &sun, &colour, size, vp, Vec3::ZERO, layers, frame as f32 * 0.016, 100.0, true, true, env);
            r.queue.submit([encoder.finish()]);
            let pixels = crate::texture_to_rgba(&r, &colour_texture, size.0, size.1).expect("cloud regression readback");
            if let Some(expected) = &baseline { assert_eq!(&pixels, expected, "clouds or history changed a foreground pixel with zero directional share"); } else { baseline = Some(pixels); }
        }
        let field = clouds.cloud_field().expect("cloud shadow field was marched");
        assert_eq!(field.area[2], 400.0);
        assert_eq!(field.sun[3], 1.0);
        // A native sheet has depth but no primary-world coverage. Known cloud
        // radiance must composite there without clearing or changing its depth.
        let targets = clouds.targets.as_ref().expect("cloud history exists");
        let mut encoder = r.device.create_command_encoder(&Default::default());
        for (view, value) in [
            (&sun, wgpu::Color::TRANSPARENT),
            (&targets.history[targets.current], wgpu::Color { r: 0.1, g: 0.02, b: 0.01, a: 0.5 }),
        ] {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("native-sheet coverage fixture"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(value), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
        }
        VolumetricClouds::draw(&mut encoder, &clouds.sky_composite, &targets.binds[targets.current], &colour, wgpu::LoadOp::Load);
        r.queue.submit([encoder.finish()]);
        let sheet_pixels = crate::texture_to_rgba(&r, &colour_texture, size.0, size.1).expect("native sheet regression readback");
        assert_ne!(Some(sheet_pixels), baseline, "native sheet depth incorrectly excluded the cloud sky composite");
    }
}
