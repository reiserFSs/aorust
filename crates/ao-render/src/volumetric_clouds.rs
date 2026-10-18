//! Opt-in native-weather volumes. Original cloud sheets remain the baseline.
//! ao-vk (MIT) cloud_layer.frag/cloud_resolve.frag/cloud_shadow.frag provide
//! height profiles, erosion, extinction and rotation-only checker reprojection.
//! Water scalar view changes rebind frame inputs without retiring cloud history.
use crate::effect_gpu::EffectPassEncoder as _;
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
    // cloud.rs10054e07 puts the overhead sheet at altitude*far/1000;
    // `thick` changes its shade, not its height. Keep opt-in volume relief
    // proportional to that sheet altitude instead of inventing a low deck
    // whose height is dominated by unrelated storm/thick constants.
    let thickness = (layer.altitude.abs() * scale * 0.25 * (1.0 + storm)).max(1.0);
    let grey = ((layer.sun_y + 0.89).clamp(0.14, 1.0) - 0.14) * (1.0 - thick * 0.9) + 0.14;
    let drift = Vec3::new(layer.wind[0], layer.wind[1], -layer.wind[2]) * (time * 100.0 * scale);
    CloudDensityLayer {
        shape: [layer.altitude * scale, thickness.max(1.0), (0.65 + thick * 0.25 + storm * 0.1).clamp(0.0, 1.0), 5.0 / thickness.max(1.0)],
        drift: [eye.x - drift.x, -drift.y, eye.z - drift.z, 1500.0 * scale],
        light: [layer.light[0], layer.light[1], layer.light[2], storm],
        sun: [layer.light_direction[0], layer.light_direction[1], layer.light_direction[2], thick],
        ambient: [layer.ambient[0], layer.ambient[1], layer.ambient[2], grey],
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
    sky_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    layers: wgpu::Buffer,
    capacity: usize,
    prepared: Vec<CloudDensityLayer>,
    estimate: RenderPipeline,
    composite: RenderPipeline,
    cloud_shadows: crate::cloud_shadows::CloudShadows,
    shadow_fallback: wgpu::TextureView,
    sky_composites: Vec<(u32, RenderPipeline)>,
    sky_active: bool,
    shadows_active: bool,
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
    water_cloud: wgpu::TextureView,
    shadow: wgpu::TextureView,
    history: Option<[wgpu::TextureView; 2]>,
    binds: [wgpu::BindGroup; 2],
    sky_binds: Option<[wgpu::BindGroup; 2]>,
    current: usize,
    valid: bool,
}
const HALF: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
impl VolumetricClouds {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_counts: &[u32]) -> Self {
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("native cloud inputs"), entries: &[
            buffer(0, wgpu::BufferBindingType::Uniform), buffer(1, wgpu::BufferBindingType::Storage { read_only: true }),
            texture(2, wgpu::TextureSampleType::Depth), texture(3, wgpu::TextureSampleType::Float { filterable: false }), texture(4, wgpu::TextureSampleType::Float { filterable: false }),
            texture(5, wgpu::TextureSampleType::Float { filterable: false }),
            texture(6, wgpu::TextureSampleType::Float { filterable: false }),
        ] });
        let sky_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("cloud sky history only"), entries: &[texture(4, wgpu::TextureSampleType::Float { filterable: false })] });
        let sky_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("native cloud sky boundary"), bind_group_layouts: &[Some(&sky_layout)], immediate_size: 0 });
        let source = format!("{}\n{}", crate::shader_source::load("cloud_density.wgsl", CLOUD_DENSITY_WGSL), crate::shader_source::load("volumetric_clouds.wgsl", include_str!("volumetric_clouds.wgsl")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native weather volumes"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("native clouds"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let make = |entry, format, blend, write_mask, samples| crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(if entry == "sky_background" { &sky_pipeline_layout } else { &pipeline_layout }),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask })] }),
            primitive: Default::default(), depth_stencil: (entry == "sky_background").then_some(wgpu::DepthStencilState { format: crate::DEPTH, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }), multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
        });
        let estimate = make("estimate", HALF, None, wgpu::ColorWrites::ALL, 1);
        let composite = make("composite", format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE,
        }), wgpu::ColorWrites::COLOR, 1);
        let cloud_shadows = crate::cloud_shadows::CloudShadows::new(device);
        let shadow_fallback = device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("unused cloud field"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R16Float, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
        let sky_composites = std::iter::once(1).chain(sample_counts.iter().copied().filter(|&samples| samples != 1))
            .map(|samples| (samples, make("sky_background", format, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), wgpu::ColorWrites::COLOR, samples))).collect();
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("cloud parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let layers = Self::layer_buffer(device, 1);
        Self { layout, sky_layout, params, layers, capacity: 1, prepared: Vec::new(), estimate, composite, cloud_shadows, shadow_fallback, sky_composites, sky_active: false, shadows_active: false, targets: None, previous_vp: Mat4::IDENTITY, previous_eye: Vec3::ZERO, previous_time: f32::NAN, previous_layers: Vec::new(), previous_fog: [f32::NAN; 6], phase: 0 }
    }
    fn layer_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("native cloud layers"), size: (capacity * std::mem::size_of::<CloudDensityLayer>()) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }
    pub(super) fn is_ready(&self) -> bool { self.estimate.is_ready() && self.composite.is_ready() && self.cloud_shadows.is_ready() && self.sky_composites.iter().all(|(_, pipeline)| pipeline.is_ready()) }
    pub(super) fn wait(&self) { self.estimate.get(); self.composite.get(); self.cloud_shadows.wait(); for (_, pipeline) in &self.sky_composites { pipeline.get(); } }
    pub(super) fn release_targets(&mut self) { self.targets = None; self.cloud_shadows.release_targets(); self.sky_active = false; self.shadows_active = false; }
    pub(super) fn release_history(&mut self) {
        if self.targets.as_ref().is_some_and(|t| t.history.is_some()) { self.targets = None; }
        self.sky_active = false;
    }
    pub(super) fn release_shadow_field(&mut self) {
        if self.cloud_shadows.target_bytes() != 0 {
            self.cloud_shadows.release_targets();
            self.targets = None; // Frame bindings retain the old field view.
        }
    }
    pub(super) fn target_bytes(&self) -> u64 {
        let field_bytes = self.cloud_shadows.target_bytes();
        if self.targets.is_none() && field_bytes == 0 { return 0; }
        self.targets.as_ref().filter(|t| t.history.is_some()).map_or(0, |t| u64::from(t.size.0.div_ceil(2)) * u64::from(t.size.1.div_ceil(2)) * 16)
            + self.layers.size() + self.params.size() + field_bytes + 2
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
    fn prepare_targets(&self, mut targets: Option<Targets>, device: &wgpu::Device, depth: &wgpu::TextureView, sun: &wgpu::TextureView, water_cloud: &wgpu::TextureView, shadow: &wgpu::TextureView, size: (u32, u32), clouds: bool) -> Targets {
        let bind = |view| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("cloud frame inputs"), layout: &self.layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: self.layers.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) }, wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(sun) }, wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(shadow) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(water_cloud) },
        ] });
        if targets.as_ref().is_none_or(|t| t.size != size || t.depth != *depth || t.sun != *sun || t.shadow != *shadow || t.history.is_some() != clouds) {
            let create = || device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("cloud-only temporal checker"), size: wgpu::Extent3d { width: size.0.div_ceil(2), height: size.1.div_ceil(2), depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HALF, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
            let history = clouds.then(|| [create(), create()]);
            let binds = std::array::from_fn(|i| bind(history.as_ref().map_or(&self.shadow_fallback, |history| &history[i])));
            let sky_binds = history.as_ref().map(|history| std::array::from_fn(|i| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("native cloud sky history"), layout: &self.sky_layout, entries: &[wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&history[i]) }] })));
            targets = Some(Targets { size, depth: depth.clone(), sun: sun.clone(), water_cloud: water_cloud.clone(), shadow: shadow.clone(), history, binds, sky_binds, current: 0, valid: false });
        } else if let Some(t) = targets.as_mut().filter(|t| t.water_cloud != *water_cloud) {
            // Scalar attachment changes only affect frame inputs, not cloud history.
            t.binds = std::array::from_fn(|i| bind(t.history.as_ref().map_or(&self.shadow_fallback, |history| &history[i])));
            t.water_cloud = water_cloud.clone();
        }
        targets.expect("cloud targets allocated")
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, sun: Option<&wgpu::TextureView>, water_cloud: Option<&wgpu::TextureView>, size: (u32, u32), vp: Mat4, eye: Vec3, layers: &[CloudLayer], time: f32, far: f32, clouds: bool, shadows: bool, env: ao_scene::Environment) {
        self.sky_active = false;
        self.shadows_active = false;
        if layers.is_empty() {
            // Empty weather must not retain obsolete primary-world scalar views.
            self.release_targets();
            return;
        }
        if (!clouds && !shadows) || size.0 == 0 || size.1 == 0 || !time.is_finite() || !far.is_finite() || far <= 0.0 || !vp.is_finite() || !eye.is_finite() {
            if let Some(t) = &mut self.targets { t.valid = false; }
            return;
        }
        if !self.upload_layers(device, queue, eye, layers, time, far) { return; }
        if shadows { self.cloud_shadows.prepare(device, queue, encoder, &self.layers, &self.prepared, eye, far); }
        let field = self.cloud_shadows.field();
        let shadow = field.as_ref().map_or(&self.shadow_fallback, |field| &field.view);
        // Clouds-only frames have no directional mask. Never sample the colour
        // attachment as a placeholder: at 1x that aliases the native sky target.
        let sun = sun.unwrap_or(&self.shadow_fallback);
        let water_cloud = water_cloud.unwrap_or(&self.shadow_fallback);
        let targets = self.targets.take();
        self.targets = Some(self.prepare_targets(targets, device, depth, sun, water_cloud, shadow, size, clouds));
        let t = self.targets.as_mut().expect("cloud targets allocated");
        let fog = [env.fog_start, env.fog_end, env.fog_color[0], env.fog_color[1], env.fog_color[2], far];
        let weather_matches = self.previous_layers.len() == layers.len() && self.previous_layers.iter().zip(layers).all(|(a, b)| a.altitude == b.altitude && a.wind == b.wind && a.storm == b.storm && a.thick == b.thick);
        let valid = t.valid && weather_matches && self.previous_fog == fog && time >= self.previous_time && time - self.previous_time < 0.25 && eye.distance(self.previous_eye) < 10.0;
        let relative_inverse = glam::DMat4::from_translation(-eye.as_dvec3()) * vp.as_dmat4().inverse();
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params { inverse_vp: relative_inverse.as_mat4().to_cols_array_2d(), previous_vp: self.previous_vp.to_cols_array_2d(), eye: eye.extend(0.0).to_array(), control: [layers.len() as f32, far * 2.0, self.phase as f32, if valid { 1.0 } else { 0.0 }], enabled: [if clouds { 1.0 } else { 0.0 }, if shadows { 1.0 } else { 0.0 }, self.prepared.iter().map(|l| l.shape[0]).fold(f32::INFINITY, f32::min), if *water_cloud == self.shadow_fallback { 0.0 } else { 1.0 }], fog: [env.fog_start, env.fog_end, 0.0, 0.0], fog_colour: [env.fog_color[0], env.fog_color[1], env.fog_color[2], 0.0], shadow_area: field.as_ref().map_or([0.0; 4], |f| f.area), shadow_sun: field.as_ref().map_or([0.0; 4], |f| f.sun) }));
        let next = 1 - t.current;
        if clouds {
            Self::draw(encoder, &self.estimate, &t.binds[t.current], &t.history.as_ref().expect("enabled cloud history")[next], wgpu::LoadOp::Clear(wgpu::Color::WHITE));
            t.current = next;
        }
        t.valid = clouds;
        self.previous_vp = vp;
        self.previous_eye = eye;
        self.previous_time = time;
        self.previous_fog = fog;
        self.previous_layers.clear();
        self.previous_layers.extend_from_slice(layers);
        self.sky_active = clouds;
        self.shadows_active = shadows;
        self.phase = (self.phase + 1) & 3;
    }
    /// Draw only at the native sky/world boundary, before any foreground FX.
    pub(super) fn draw_sky(&self, pass: &mut wgpu::RenderPass<'_>, samples: u32) {
        if !self.sky_active { return; }
        let targets = self.targets.as_ref().expect("prepared cloud history");
        let pipeline = &self.sky_composites.iter().find(|(count, _)| *count == samples).expect("cloud sky sample count warmed at startup").1;
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &targets.sky_binds.as_ref().expect("prepared native cloud sky bindings")[targets.current], &[]);
        pass.draw(0..3, 0..1);
    }
    /// Current-frame primary-world directional attenuation, never sky colour.
    pub(super) fn composite_world(&self, encoder: &mut wgpu::CommandEncoder, colour: &wgpu::TextureView) {
        if !self.shadows_active { return; }
        let targets = self.targets.as_ref().expect("prepared cloud shadow targets");
        Self::draw(encoder, &self.composite, &targets.binds[targets.current], colour, wgpu::LoadOp::Load);
    }
    fn draw(encoder: &mut wgpu::CommandEncoder, pipeline: &RenderPipeline, bind: &wgpu::BindGroup, output: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) {
        let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor { label: Some("native cloud atmosphere"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
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
        assert_eq!(a.drift, [18.0, 0.0, 34.0, 1500.0]);
        assert_eq!(a.light, [0.8, 0.3, 0.1, 0.5]);
        assert_eq!(std::mem::size_of::<CloudDensityLayer>(), 80);
        assert_eq!(std::mem::size_of::<Params>() % 16, 0);
        assert!((a.shape[1] - 37.5).abs() < 0.00001);
        assert!((a.ambient[3] - 0.6904).abs() < 0.00001);
    }
    #[test]
    fn checker_visits_every_subpixel() {
        let mut visited = [false; 4];
        for phase in 0..4u32 { visited[((phase & 1) | (((phase >> 1) & 1) << 1)) as usize] = true; }
        assert!(visited.into_iter().all(|v| v));
    }
    #[test]
    fn sky_scattering_preserves_native_gamma_and_ignores_world_fog() {
        let shader = include_str!("volumetric_clouds.wgsl");
        let march = shader.split("fn cloud_march(").nth(1).expect("cloud march")
            .split("@fragment fn estimate").next().expect("cloud estimate");
        assert!(!march.contains("params.fog"), "native unfogged sheets cannot import world fog colour/extinction");
        assert!(!march.contains("pow("), "native gamma colours must not be decoded into the gamma UNORM target");
        assert!(march.contains("layer.ambient.w"), "native sun_y/thick sheet shade is the cloud lighting baseline");
    }
    #[test]
    fn curved_native_bounds_intersect_horizontal_rays_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("curved cloud regression needs an offscreen GPU");
        let source = format!("{}\n{}", CLOUD_DENSITY_WGSL, r#"
@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    return vec4(vec2(f32((i<<1u)&2u),f32(i&2u))*2.0-1.0,0.0,1.0);
}
@fragment fn check()->@location(0) vec4<f32> {
    let layer = CloudDensityLayer(vec4(100.0,25.0,0.95,0.2),vec4(0.0,0.0,0.0,1500.0),
        vec4(1.0),vec4(0.0,1.0,0.0,0.0),vec4(0.1));
    let overhead = cloud_base(vec3(0.0),layer);
    let halfway = cloud_base(vec3(750.0,0.0,0.0),layer);
    let edge = cloud_base(vec3(1500.0,0.0,0.0),layer);
    let horizontal = cloud_interval(vec3(0.0),vec3(1.0,0.0,0.0),layer,3000.0);
    let vertical = cloud_interval(vec3(0.0),vec3(0.0,1.0,0.0),layer,3000.0);
    let inside = cloud_interval(vec3(0.0,110.0,0.0),vec3(0.0,1.0,0.0),layer,3000.0);
    let heights = abs(overhead-100.0)<0.001 && abs(halfway-62.5)<0.001 && abs(edge+50.0)<0.001;
    let horizon_hit = abs(horizontal.x-sqrt(1500000.0))<0.05 && abs(horizontal.y-sqrt(1875000.0))<0.05;
    let vertical_hit = abs(vertical.x-100.0)<0.001 && abs(vertical.y-125.0)<0.001;
    let inside_hit = abs(inside.x)<0.001 && abs(inside.y-15.0)<0.001;
    return vec4(select(0.0,1.0,heights),select(0.0,1.0,horizon_hit),select(0.0,1.0,vertical_hit && inside_hit),1.0);
}
"#);
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native curved cloud bounds regression"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline = crate::pipeline_creation::render(&r.device, &wgpu::RenderPipelineDescriptor { label: Some("native curved cloud bounds regression"), layout: None, vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] }, fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("check"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })] }), primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None });
        let texture = r.device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("curved cloud bounds result"), size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
        let view = texture.create_view(&Default::default());
        let mut encoder = r.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("native curved cloud bounds regression"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(&pipeline); pass.draw(0..3,0..1);
        }
        r.queue.submit([encoder.finish()]);
        let pixels = crate::texture_to_rgba(&r, &texture, 4, 4).expect("curved cloud bounds readback");
        assert!(pixels.as_chunks::<4>().0.iter().all(|pixel| *pixel == [255,255,255,255]), "native cloud curvature/horizontal or inside-volume intersection failed: {pixels:?}");
    }
    #[test]
    fn native_renderer_preserves_translucent_foreground_over_cloud_sky() {
        use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, Vertex};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = crate::Renderer::new(&instance, None).expect("native cloud ordering regression needs an offscreen GPU");
        for samples in [1, 4] {
            for blend in [Blend::AlphaBlend, Blend::Additive] {
                let vertices = [(-2.0,-2.0),(0.0,-2.0),(0.0,2.0),(-2.0,2.0)].into_iter()
                    .map(|(x,y)| Vertex { pos:[x,y,-3.0], normal:[0.0,0.0,1.0], color:[1.0;4], ..Default::default() }).collect();
                let foreground = Mesh { vertices, submeshes: vec![Submesh {
                    blend, depth_write: Some(false), lighting_enabled: false,
                    base_color: [1.0,0.0,0.0,0.5], two_sided: true,
                    ..Submesh::new(vec![0,1,2,0,2,3], None)
                }] };
                let env = ao_scene::Environment { sky_color:[0.0;3], fog_color:[0.0;3], fog_start:80.0, fog_end:100.0, ambient:[0.0;3], sun_color:[0.0;3], sun_dir:[0.0,1.0,0.0], sun_specular:1.0 };
                let mut native_sheets = ao_scene::cloud::Clouds::mesh();
                native_sheets.submeshes = [(0,2),(2,4),(4,27),(27,30)].map(|(first,end)| Submesh {
                    blend:Blend::AlphaBlend, two_sided:true, base_color:[0.0;4],
                    ..Submesh::new(ao_scene::cloud::Clouds::indices(10,first,end),None)
                }).into();
                r.upload(&Scene {
                    meshes: vec![native_sheets, foreground],
                    sky: vec![Instance { mesh:0, transform:Mat4::IDENTITY.to_cols_array_2d() }],
                    single_clouds: vec![CloudLayer { mesh:0, altitude:100.0, wind:[0.0;3], storm:0.0, thick:0.0, sun_y:1.0, light:[1.0;3], light_direction:[0.0,1.0,0.0], ambient:[0.0;3] }],
                    instances: vec![Instance { mesh:1, transform:Mat4::IDENTITY.to_cols_array_2d() }],
                    environment: Some(env), ..Default::default()
                });
                r.set_enhanced_settings(crate::EnhancedSettings { volumetric_clouds:true, ..Default::default() });
                r.set_msaa_samples(samples);
                assert_eq!(r.msaa_samples(),samples,"ordering regression requires the requested native MSAA mode");
                r.set_cloud_preferences(true,true,false);
                r.set_screen_effects(vec![],true);
                // Deterministic cloud radiance is the fixture input, not a
                // second compositing implementation. Renderer::render still
                // owns preparation, native sky/world order and foreground draw.
                let clouds = r.volumetric_clouds.as_mut().expect("startup cloud pass");
                let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label:Some("known cloud radiance fixture"), source:wgpu::ShaderSource::Wgsl(r#"
@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    return vec4(vec2(f32((i<<1u)&2u),f32(i&2u))*2.0-1.0,0.0,1.0);
}
@fragment fn estimate()->@location(0) vec4<f32> { return vec4(0.1,0.1,0.1,0.5); }
"#.into()) });
                let layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label:Some("known cloud radiance fixture"), bind_group_layouts:&[Some(&clouds.layout)], immediate_size:0 });
                clouds.estimate = crate::pipeline_creation::render_deferred(&r.device, &wgpu::RenderPipelineDescriptor { label:Some("known cloud radiance fixture"), layout:Some(&layout), vertex:wgpu::VertexState { module:&shader, entry_point:Some("vs"), compilation_options:Default::default(), buffers:&[] }, fragment:Some(wgpu::FragmentState { module:&shader, entry_point:Some("estimate"), compilation_options:Default::default(), targets:&[Some(wgpu::ColorTargetState { format:HALF, blend:None, write_mask:wgpu::ColorWrites::ALL })] }), primitive:Default::default(), depth_stencil:None, multisample:Default::default(), multiview_mask:None, cache:None });
                r.wait_for_pipeline_warmup();
                let output = r.device.create_texture_tracked(&wgpu::TextureDescriptor { label:Some("native foreground cloud ordering"), size:wgpu::Extent3d { width:64,height:64,depth_or_array_layers:1 }, mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:r.format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[] });
                let targets = crate::Targets::new(&r,64,64);
                r.render(&output.create_view(&Default::default()),&targets,&crate::Camera::look_at(Vec3::ZERO,Vec3::NEG_Z));
                let pixels = crate::texture_to_rgba(&r,&output,64,64).expect("native cloud foreground readback");
                let pixel = |x:usize| &pixels[(32*64+x)*4..(32*64+x)*4+3];
                for &value in pixel(48) { assert!((i16::from(value)-26).abs()<=2,"sky fixture must contain real cloud radiance, samples={samples} blend={blend:?}: {:?}",pixel(48)); }
                let expected = if blend == Blend::AlphaBlend { [140i16,13,13] } else { [153i16,26,26] };
                assert!(pixel(16).iter().zip(expected).all(|(&actual,expected)| (i16::from(actual)-expected).abs()<=2),
                    "native cloud sky must precede non-depthwriting foreground, samples={samples} blend={blend:?}: actual={:?}, expected={expected:?}",pixel(16));
            }
        }
    }
    #[test]
    fn native_renderer_real_clouds_advance_once_across_four_phases() {
        use ao_scene::{Blend, Instance, Scene, Submesh};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = crate::Renderer::new(&instance, None).expect("native cloud lifecycle needs an offscreen GPU");
        let mut sheets = ao_scene::cloud::Clouds::mesh();
        sheets.submeshes = [(0,2),(2,4),(4,27),(27,30)].map(|(first,end)| Submesh {
            blend: Blend::AlphaBlend, two_sided: true, base_color: [0.0;4],
            ..Submesh::new(ao_scene::cloud::Clouds::indices(10,first,end),None)
        }).into();
        r.upload(&Scene {
            meshes: vec![sheets],
            sky: vec![Instance { mesh:0, transform:Mat4::IDENTITY.to_cols_array_2d() }],
            single_clouds: vec![CloudLayer {
                mesh:0, altitude:100.0, wind:[0.01,0.0,0.02], storm:0.5, thick:1.0,
                sun_y:1.0, light:[1.0;3], light_direction:[0.0,1.0,0.0], ambient:[0.2;3],
            }],
            environment: Some(ao_scene::Environment {
                sky_color:[0.0;3], fog_color:[0.0;3], fog_start:800.0, fog_end:1000.0,
                ambient:[0.2;3], sun_color:[1.0;3], sun_dir:[0.0,1.0,0.0], sun_specular:1.0,
            }), ..Default::default()
        });
        r.set_cloud_preferences(true,true,false);
        r.set_screen_effects(vec![],true);
        let output = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label:Some("real native cloud lifecycle"), size:wgpu::Extent3d { width:64,height:64,depth_or_array_layers:1 },
            mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:r.format,
            usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[],
        });
        let view = output.create_view(&Default::default());
        for samples in [1,4] {
            r.set_msaa_samples(samples);
            assert_eq!(r.msaa_samples(),samples);
            let targets = crate::Targets::new(&r,64,64);
            for shadows in [false,true] {
                r.set_enhanced_settings(crate::EnhancedSettings {
                    volumetric_clouds:true, cloud_shadows:shadows, ..Default::default()
                });
                r.wait_for_pipeline_warmup();
                let mut previous_current = None;
                for frame in 0..4 {
                    let eye = Vec3::new(frame as f32 * 0.1,0.0,0.0);
                    r.time = frame as f32 * 0.016;
                    let camera = crate::Camera::look_at(eye,eye + Vec3::new(frame as f32 * 0.002,1.0,-0.1));
                    let phase = r.volumetric_clouds.as_ref().unwrap().phase;
                    // Renderer owns prepare -> native sky -> post-world shadow.
                    // No replacement estimator or direct history writes here.
                    r.render(&view,&targets,&camera);
                    let clouds = r.volumetric_clouds.as_ref().unwrap();
                    let history = clouds.targets.as_ref().expect("real native cloud history");
                    assert_eq!(clouds.phase,(phase+1)&3,"post-world shadows must not prepare clouds a second time");
                    if let Some(current) = previous_current {
                        assert_eq!(history.current,1-current,"exactly one history advance per renderer frame");
                    }
                    previous_current = Some(history.current);
                    assert!(history.valid && clouds.sky_active);
                    assert_eq!(clouds.shadows_active,shadows);
                    assert_eq!(clouds.previous_eye,eye);
                    assert_eq!(clouds.previous_time,r.time);
                    if shadows { assert!(clouds.cloud_field().is_some()); }
                    else { assert_eq!(history.sun,clouds.shadow_fallback,"clouds-only inputs must not alias frame colour"); }
                    let pixels = crate::texture_to_rgba(&r,&output,64,64).expect("real cloud lifecycle readback");
                    assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[..3].iter().any(|&v| v>4)),
                        "real native cloud volume produced no radiance, samples={samples}, shadows={shadows}, phase={frame}");
                }
            }
        }
    }
    #[test]
    fn water_scalar_toggle_preserves_cloud_history() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("cloud water toggle regression needs an offscreen GPU");
        let mut clouds = VolumetricClouds::new(&r.device, r.format, &[1]);
        let texture = |format| r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("cloud water toggle regression"), size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view(&Default::default());
        let depth = texture(crate::DEPTH);
        let sun = texture(crate::enhanced::AUX);
        let scalar = texture(wgpu::TextureFormat::R16Float);
        let replacement_scalar = texture(wgpu::TextureFormat::R16Float);
        let mut targets = clouds.prepare_targets(None, &r.device, &depth, &sun, &scalar, &clouds.shadow_fallback, (16, 16), true);
        targets.current = 1;
        targets.valid = true;
        let history = targets.history.clone();
        let sky_binds = targets.sky_binds.clone();
        for scalar in [None, Some(&replacement_scalar)] {
            let previous_binds = targets.binds.clone();
            let scalar = scalar.unwrap_or(&clouds.shadow_fallback);
            targets = clouds.prepare_targets(Some(targets), &r.device, &depth, &sun, scalar, &clouds.shadow_fallback, (16, 16), true);
            assert_eq!(targets.water_cloud, *scalar);
            assert_ne!(targets.binds, previous_binds, "water input must be rebound");
            assert_eq!(targets.history, history, "both history handles must survive scalar attachment toggles");
            assert_eq!(targets.sky_binds, sky_binds, "sky history bindings do not use the water scalar");
            assert_eq!(targets.current, 1);
            assert!(targets.valid);
        }
        clouds.targets = Some(targets);
        let mut encoder = r.device.create_command_encoder(&Default::default());
        let env = ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 80.0, fog_end: 100.0, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0 };
        clouds.prepare(&r.device, &r.queue, &mut encoder, &depth, Some(&sun), None, (16, 16), Mat4::IDENTITY, Vec3::ZERO, &[], 0.0, 100.0, true, false, env);
        assert!(clouds.targets.is_none(), "empty weather must release bindings retaining the removed scalar view");
    }
    #[test]
    fn cloud_foreground_and_empty_layers_are_unchanged() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("cloud regression needs an offscreen GPU");
        let mut clouds = VolumetricClouds::new(&r.device, r.format, &[1]);
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
            clouds.prepare(&r.device, &r.queue, &mut encoder, &depth, Some(&sun), None, size, vp, Vec3::ZERO, layers, frame as f32 * 0.016, 100.0, true, true, env);
            clouds.composite_world(&mut encoder, &colour);
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
            (&targets.history.as_ref().expect("enabled cloud history")[targets.current], wgpu::Color { r: 0.1, g: 0.02, b: 0.01, a: 0.5 }),
        ] {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("native-sheet coverage fixture"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(value), store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("native sky boundary with retained sheet depth"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &colour, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })], depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            clouds.draw_sky(&mut pass, 1);
        }
        r.queue.submit([encoder.finish()]);
        let sheet_pixels = crate::texture_to_rgba(&r, &colour_texture, size.0, size.1).expect("native sheet regression readback");
        assert_ne!(Some(sheet_pixels), baseline, "native sheet depth incorrectly excluded the cloud sky composite");
        let history_bytes = u64::from(size.0.div_ceil(2)) * u64::from(size.1.div_ceil(2)) * 16;
        let cloud_bytes = clouds.target_bytes();
        clouds.release_history();
        assert_eq!(cloud_bytes - clouds.target_bytes(), history_bytes, "disabling clouds must retire both half-resolution RGBA16 histories (9,216,000 bytes at 1920x1200)");
        assert!(clouds.cloud_field().is_some(), "gap/cloud shadows retain their resolution-independent field");
        let shadow_bytes = clouds.target_bytes();
        for enabled in [false, true, false] {
            let mut encoder = r.device.create_command_encoder(&Default::default());
            clouds.prepare(&r.device, &r.queue, &mut encoder, &depth, Some(&sun), None, size, vp, Vec3::ZERO,
                std::slice::from_ref(&native), 0.0, 100.0, enabled, true, env);
            clouds.composite_world(&mut encoder, &colour);
            r.queue.submit([encoder.finish()]);
            assert_eq!(clouds.targets.as_ref().unwrap().history.is_some(), enabled);
            assert_eq!(clouds.target_bytes(), shadow_bytes + if enabled { history_bytes } else { 0 },
                "shadow-only encoding must use a one-texel history placeholder, not frame-sized cloud targets");
        }
        clouds.release_shadow_field();
        assert!(clouds.cloud_field().is_none());
        assert!(clouds.targets.is_none(), "disabled cloud-field bindings must release their retained texture view");
        assert_eq!(clouds.target_bytes(), 0);
    }
}
