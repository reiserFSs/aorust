//! Opt-in atmosphere layered over, never substituted for, native FogModel extinction.
//! Ray-marched shafts use only the current admitted directional cascade maps.
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use crate::volumetric_clouds::CloudDensityLayer;
use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Default)]
pub(super) struct AtmosphereSwitches {
    pub height_haze: bool,
    pub land_haze: bool,
    pub valley_mist: bool,
    pub volumetric_shafts: bool,
    pub cloud_gap_shafts: bool,
}
impl AtmosphereSwitches {
    fn any(self) -> bool { self.height_haze || self.land_haze || self.valley_mist || self.volumetric_shafts || self.cloud_gap_shafts }
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse_vp: [[f32; 4]; 4],
    eye: [f32; 4],
    sun: [f32; 4],
    fog: [f32; 4],
    profile: [f32; 4],
    light: [f32; 4],
    horizon: [f32; 4],
    cloud_area: [f32; 4],
    cloud_sun: [f32; 4],
    cloud_shift: [f32; 4],
    flags: [u32; 4],
    counts: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MistVolume { center_radius: [f32; 4], density: [f32; 4] }

pub(super) struct Atmosphere {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    volumes: wgpu::Buffer,
    volume_capacity: usize,
    volume_upload: Vec<MistVolume>,
    haze: RenderPipeline,
    shafts: RenderPipeline,
    composite: RenderPipeline,
    scatter: RenderPipeline,
    targets: Option<Targets>,
}
struct Targets {
    size: (u32, u32),
    depth: wgpu::TextureView,
    mask: wgpu::TextureView,
    cloud_map: wgpu::TextureView,
    half: wgpu::TextureView,
    bind: wgpu::BindGroup,
    estimate_bind: wgpu::BindGroup,
}
const HALF: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
fn storage(device: &wgpu::Device, label: &str, bytes: usize) -> wgpu::Buffer {
    device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some(label), size: bytes as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}
impl Atmosphere {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("atmosphere inputs"), entries: &[
            buffer(0, wgpu::BufferBindingType::Uniform), texture(1, wgpu::TextureSampleType::Depth),
            texture(2, wgpu::TextureSampleType::Float { filterable: false }),
            texture(3, wgpu::TextureSampleType::Float { filterable: false }),
            buffer(4, wgpu::BufferBindingType::Storage { read_only: true }),
            texture(5, wgpu::TextureSampleType::Float { filterable: false }),
        ] });
        let source = [
            crate::shader_source::load("atmosphere.wgsl", include_str!("atmosphere.wgsl")),
            crate::shader_source::load("height_haze.wgsl", include_str!("height_haze.wgsl")),
            crate::shader_source::load("land_haze.wgsl", include_str!("land_haze.wgsl")),
            crate::shader_source::load("valley_mist.wgsl", include_str!("valley_mist.wgsl")),
            crate::shader_source::load("volumetric_shafts.wgsl", include_str!("volumetric_shafts.wgsl")),
            crate::shader_source::load("cloud_gap_shafts.wgsl", include_str!("cloud_gap_shafts.wgsl")),
        ].join("\n");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native layered atmosphere"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let shadow_layout = crate::sun_shadows::receiver_layout(device);
        let plain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("atmosphere"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let shaft_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("cascade atmosphere"), bind_group_layouts: &[Some(&layout), Some(&shadow_layout)], immediate_size: 0 });
        let make = |entry, layout: &wgpu::PipelineLayout, format, blend| crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: if format == HALF { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::COLOR } })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let haze = make("haze", &plain_layout, HALF, None);
        let shafts = make("shafts", &shaft_layout, HALF, None);
        let composite = make("composite", &plain_layout, format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::REPLACE,
        }));
        let scatter = make("composite_light", &plain_layout, format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::REPLACE,
        }));
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("atmosphere parameters"), size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { layout, params, volumes: storage(device, "mist volumes", std::mem::size_of::<MistVolume>()),
            volume_capacity: 1,
            volume_upload: Vec::new(), haze, shafts, composite, scatter, targets: None }
    }
    pub(super) fn is_ready(&self) -> bool { self.haze.is_ready() && self.shafts.is_ready() && self.composite.is_ready() && self.scatter.is_ready() }
    pub(super) fn wait(&self) { self.haze.get(); self.shafts.get(); self.composite.get(); self.scatter.get(); }
    pub(super) fn release_targets(&mut self) { self.targets = None; }
    pub(super) fn target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |t| u64::from(t.size.0.div_ceil(2)) * u64::from(t.size.1.div_ceil(2)) * 8)
            + self.params.size() + self.volumes.size()
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView, directional_mask: &wgpu::TextureView, colour: &wgpu::TextureView, size: (u32, u32),
        vp: Mat4, eye: Vec3, far: f32, env: ao_scene::Environment, fog: Option<&ao_scene::FogModel>, base_height: f32,
        switches: AtmosphereSwitches, shadow_receiver: Option<&wgpu::BindGroup>, cloud_layers: &[CloudDensityLayer], native_sun_y: f32,
        cloud_field: Option<&crate::cloud_shadows::CloudShadowField>) {
        if !switches.any() || size.0 == 0 || size.1 == 0 || !vp.is_finite() || vp.determinant().abs() < 1e-12
            || !eye.is_finite() || !far.is_finite() || far <= 0.0 || !base_height.is_finite() || !native_sun_y.is_finite() { return; }
        // Outdoor atmospheric profiles must not leak through dungeon walls.
        if fog.is_some_and(|f| f.rooms.is_some()) { return; }
        self.volume_upload.clear();
        if let Some(fog) = fog {
            for v in &fog.volumes {
                if v.room.is_none() && v.radius.is_finite() && v.radius > 0.0 && v.density.is_finite() && v.density > 0.0
                    && v.pos.iter().all(|v| v.is_finite()) {
                    self.volume_upload.push(MistVolume { center_radius: (Vec3::from(v.pos) - eye).extend(v.radius).to_array(), density: [v.density.clamp(0.0, 1.0), 0.0, 0.0, 0.0] });
                }
            }
        }
        if self.volume_upload.len() > self.volume_capacity {
            self.volume_capacity = self.volume_upload.len().next_power_of_two();
            self.volumes = storage(device, "mist volumes", self.volume_capacity * std::mem::size_of::<MistVolume>());
            self.targets = None;
        }
        if !self.volume_upload.is_empty() { queue.write_buffer(&self.volumes, 0, bytemuck::cast_slice(&self.volume_upload)); }
        let sun = Vec3::from(env.sun_dir).normalize_or_zero();
        let tint = Vec3::from(env.fog_color).clamp(Vec3::ZERO, Vec3::ONE);
        let tint = tint / tint.max_element().max(1e-5);
        let relative_inverse = glam::DMat4::from_translation(-eye.as_dvec3()) * vp.as_dmat4().inverse();
        let cloud_map = cloud_field.map_or(directional_mask, |field| &field.view);
        let cloud_area = cloud_field.map_or([0.0; 4], |field| field.area);
        let cloud_sun = cloud_field.map_or([0.0; 4], |field| field.sun);
        let cloud_shift = cloud_field.map_or(Vec3::ZERO, |field| eye - field.eye);
        let cloud_base = cloud_layers.iter().map(|layer| layer.shape[0]).fold(f32::INFINITY, f32::min);
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params {
            inverse_vp: relative_inverse.as_mat4().to_cols_array_2d(), eye: eye.extend(far).to_array(),
            sun: sun.extend(0.0).to_array(), fog: tint.extend(env.fog_end.max(0.01)).to_array(),
            light: Vec3::from(env.sun_color).max(Vec3::ZERO).extend(0.0).to_array(),
            horizon: Vec3::from(env.fog_color).lerp(Vec3::from(env.sky_color), if switches.land_haze { 0.35 } else { 0.0 })
                .clamp(Vec3::ZERO, Vec3::ONE).extend(0.0).to_array(),
            cloud_area, cloud_sun, cloud_shift: cloud_shift.extend(cloud_base).to_array(),
            profile: [base_height - eye.y, env.fog_start, mist_weight(native_sun_y), far],
            flags: [u32::from(switches.height_haze), u32::from(switches.land_haze), u32::from(switches.valley_mist), u32::from(switches.volumetric_shafts)],
            counts: [self.volume_upload.len() as u32, cloud_layers.len() as u32, u32::from(switches.cloud_gap_shafts && cloud_field.is_some()), 0],
        }));
        if self.targets.as_ref().is_none_or(|t| t.size != size || t.depth != *depth || t.mask != *directional_mask || t.cloud_map != *cloud_map) {
            let half = device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("half resolution atmosphere"),
                size: wgpu::Extent3d { width: size.0.div_ceil(2), height: size.1.div_ceil(2), depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HALF,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
            let bind_frame = |input| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("atmosphere frame"), layout: &self.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(directional_mask) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(input) },
                wgpu::BindGroupEntry { binding: 4, resource: self.volumes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(cloud_map) },
            ] });
            // Do not bind the estimate attachment for reading in its own pass.
            let estimate_bind = bind_frame(directional_mask);
            let bind = bind_frame(&half);
            self.targets = Some(Targets { size, depth: depth.clone(), mask: directional_mask.clone(), cloud_map: cloud_map.clone(), half, bind, estimate_bind });
        }
        let t = self.targets.as_ref().expect("atmosphere targets allocated");
        let mapped = switches.volumetric_shafts && shadow_receiver.is_some();
        for (stage, output) in [(0, &t.half), (1, colour), (2, colour)] {
            let estimate = stage == 0;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("layered atmosphere"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: if estimate { wgpu::LoadOp::Clear(wgpu::Color::WHITE) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(if stage == 2 { &self.scatter } else if !estimate { &self.composite } else if mapped { &self.shafts } else { &self.haze });
            pass.set_bind_group(0, if estimate { &t.estimate_bind } else { &t.bind }, &[]);
            if estimate && mapped { pass.set_bind_group(1, shadow_receiver.expect("mapped shafts receiver"), &[]); }
            pass.draw(0..3, 0..1);
        }
    }
}
fn mist_weight(elevation: f32) -> f32 { 1.0 - ((elevation - 0.02) / 0.35).clamp(0.0, 1.0) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_switches_and_authored_sun_mist_burnoff() {
        assert!(!AtmosphereSwitches::default().any());
        for switches in [AtmosphereSwitches { height_haze: true, ..Default::default() }, AtmosphereSwitches { land_haze: true, ..Default::default() },
            AtmosphereSwitches { valley_mist: true, ..Default::default() }, AtmosphereSwitches { volumetric_shafts: true, ..Default::default() },
            AtmosphereSwitches { cloud_gap_shafts: true, ..Default::default() }] { assert!(switches.any()); }
        assert_eq!(mist_weight(-0.7), 1.0);
        assert_eq!(mist_weight(0.0), 1.0);
        assert!(mist_weight(0.1) > mist_weight(0.2));
        assert_eq!(mist_weight(0.7), 0.0);
    }
    #[test]
    fn offscreen_independent_haze_and_empty_cloud_gap_preserve_fog() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("atmosphere regression requires offscreen GPU");
        let size = (16, 16);
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |format, usage| r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("atmosphere regression"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
        });
        let depth = texture(wgpu::TextureFormat::Depth32Float, rt).create_view(&Default::default());
        let mask = texture(crate::enhanced::AUX, rt).create_view(&Default::default());
        let colour_texture = texture(r.format, rt | wgpu::TextureUsages::COPY_SRC);
        let colour = colour_texture.create_view(&Default::default());
        let mut atmosphere = Atmosphere::new(&r.device, r.format);
        atmosphere.wait();
        let mut outputs = Vec::new();
        for (background, coverage) in [(0.25, 1.0), (0.75, 1.0), (0.5, 0.0)] {
        for switches in [AtmosphereSwitches::default(),
            AtmosphereSwitches { height_haze: true, ..Default::default() },
            AtmosphereSwitches { land_haze: true, ..Default::default() },
            AtmosphereSwitches { valley_mist: true, ..Default::default() },
            AtmosphereSwitches { cloud_gap_shafts: true, ..Default::default() }] {
            let mut encoder = r.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("atmosphere depth clear"), color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.999), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                    timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
            }
            for (view, value, alpha) in [(&colour, background, 1.0), (&mask, 1.0, coverage)] {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("atmosphere colour clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: value, g: value, b: value, a: alpha }), store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
            }
            let env = ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.2, 0.3, 0.5], fog_start: 0.1, fog_end: 100.0,
                ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, -1.0, 0.0], sun_specular: 0.0 };
            atmosphere.encode(&r.device, &r.queue, &mut encoder, &depth, &mask, &colour, size,
                Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0), Vec3::ZERO, 100.0, env, None, 0.0, switches, None, &[], -1.0, None);
            r.queue.submit([encoder.finish()]);
            outputs.push(crate::texture_to_rgba(&r, &colour_texture, size.0, size.1).unwrap());
        }
        }
        for feature in 0..5 {
            for (dark, light) in outputs[feature].as_chunks::<4>().0.iter().zip(outputs[feature + 5].as_chunks::<4>().0) {
                for channel in 0..3 {
                    assert!(i16::from(light[channel]) - i16::from(dark[channel]) <= 129,
                        "added haze must not increase native fogged surface contrast");
                }
            }
        }
        for offset in [0, 5] {
            assert_eq!(outputs[offset], outputs[offset + 4], "cloud-gap shafts without native cloud layers are neutral");
            for image in &outputs[offset + 1..offset + 4] { assert_ne!(image, &outputs[offset], "each haze switch acts independently"); }
        }
        assert_eq!(outputs[10], outputs[12], "native cloud-sheet depth is not admitted land");
        atmosphere.release_targets();
        assert_eq!(atmosphere.target_bytes(), atmosphere.params.size() + atmosphere.volumes.size());
    }
}
