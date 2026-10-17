//! Enhanced HDR scene: authored-day sun brightening into RGBA16F, then tonemap + dither to the 8-bit output.
//! Owns no resources unless `EnhancedSettings::hdr` is on; the faithful path never reaches it.
use crate::gpu_stats::GpuStatsExt;

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(super) struct Grade {
    pub size: (u32, u32),
    hdr_bg: wgpu::BindGroup,
    hdr: wgpu::TextureView,
    halo: wgpu::TextureView,
    aux_bg: wgpu::BindGroup,
    params: wgpu::Buffer,
    expose: wgpu::RenderPipeline,
    present: wgpu::RenderPipeline,
}

impl Grade {
    /// `source_layout` is the Enhanced single-texture boundary layout; `sun` is the world pass's sun mask.
    pub fn new(device: &wgpu::Device, output: wgpu::TextureFormat, size: (u32, u32), source_layout: &wgpu::BindGroupLayout, sun: &wgpu::TextureView) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Enhanced HDR grade"), source: wgpu::ShaderSource::Wgsl([crate::bloom::glare().as_ref(), crate::shader_source::load("grade.wgsl", include_str!("grade.wgsl")).as_ref()].concat().into()) });
        let aux_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Enhanced grade inputs"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Enhanced grade"), bind_group_layouts: &[Some(source_layout), Some(&aux_layout)], immediate_size: 0 });
        let make = |entry, format| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let hdr = device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Enhanced HDR scene"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HDR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view(&Default::default());
        let halo_size = crate::bloom::halo_size(size);
        let halo = device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Enhanced bloom halo"), size: wgpu::Extent3d { width: halo_size.0, height: halo_size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HDR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
                | if cfg!(test) { wgpu::TextureUsages::COPY_DST } else { wgpu::TextureUsages::empty() },
            view_formats: &[],
        }).create_view(&Default::default());
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("Enhanced grade parameters"), size: 48, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let hdr_bg = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Enhanced HDR scene"), layout: source_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hdr) }] });
        let aux_bg = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Enhanced grade inputs"), layout: &aux_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(sun) },
            wgpu::BindGroupEntry { binding: 1, resource: params.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&halo) },
        ] });
        Self { size, hdr_bg, hdr, halo, aux_bg, params, expose: make("expose", HDR), present: make("present", output) }
    }

    pub fn bytes(size: (u32, u32)) -> u64 {
        let halo = crate::bloom::halo_size(size);
        (u64::from(size.0) * u64::from(size.1) + u64::from(halo.0) * u64::from(halo.1)) * 8 + 48
    }

    /// Grade strength and day / low-sun / night weights (see [`time_of_day`]); day also gates sun exposure.
    pub fn set_grade(&self, queue: &wgpu::Queue, strength: f32, tod: [f32; 3]) {
        queue.write_buffer(&self.params, 16, bytemuck::cast_slice(&[strength, tod[0], tod[1], tod[2]]));
    }

    /// Framebuffer-space terrain fog colour, excluded from daylight exposure.
    pub fn set_fog_color(&self, queue: &wgpu::Queue, color: [f32; 3]) {
        queue.write_buffer(&self.params, 32, bytemuck::cast_slice(&[color[0], color[1], color[2], 1.0]));
    }

    /// The exposed scene (colour above 1 only where the sun lifted it): the bloom pyramid's input.
    pub fn hdr_view(&self) -> &wgpu::TextureView { &self.hdr }
    /// Half-resolution halo the bloom pyramid writes; `present` adds it above white.
    pub fn halo_view(&self) -> &wgpu::TextureView { &self.halo }

    fn pass(encoder: &mut wgpu::CommandEncoder, label: &str, pipeline: &wgpu::RenderPipeline, input: &wgpu::BindGroup, aux: &wgpu::BindGroup,
        target: &wgpu::TextureView, timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, input, &[]);
        pass.set_bind_group(1, aux, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Lifts daylight-sun pixels of the native-range colour `base` into the HDR scene. `bloom_strength` is the
    /// strength `present` will apply to the halo (0 when bloom is off, so a stale halo is never shown).
    pub fn expose(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, base: &wgpu::BindGroup, boost: f32, bloom_strength: f32) {
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&[boost, bloom_strength, 0.0, 0.0]));
        Self::pass(encoder, "Enhanced HDR expose", &self.expose, base, &self.aux_bg, &self.hdr, None);
    }

    /// Adds the bloom halo above white, tonemaps and dithers into the 8-bit `output`.
    pub fn present(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        Self::pass(encoder, "Enhanced tonemap present", &self.present, &self.hdr_bg, &self.aux_bg, output, timestamps);
    }
}

pub(super) fn daylight_weight(daylight: Option<f32>) -> f32 {
    daylight.filter(|value| value.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0)
}

/// Day follows native GAME.CurrentDayIntensity, never sun colour (which can be bright at night).
/// Missing authoring stays closed; the authored sun elevation only controls the warm low-sun tint.
pub(super) fn time_of_day(daylight: Option<f32>, sun_dir: [f32; 3]) -> [f32; 3] {
    let smooth = |a: f32, b: f32, x: f32| { let t = ((x - a) / (b - a)).clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) };
    let day = daylight_weight(daylight);
    let elevation = glam::Vec3::from(sun_dir).normalize_or_zero().y;
    [day, (1.0 - smooth(0.2, 0.6, elevation)) * day, 1.0 - day]
}

impl crate::Renderer {
    pub(super) fn prepare_enhanced_grade(&mut self, fog_color: [f32; 3]) {
        if !self.enhanced_settings.hdr { self.grade = None; return; }
        let Some(frame) = self.enhanced_frame.as_ref() else { self.grade = None; return; };
        if self.grade.as_ref().is_none_or(|grade| grade.size != frame.size) {
            let sun = frame.sun.as_ref().expect("HDR owns its world sun mask");
            self.grade = Some(Grade::new(&self.device, self.format, frame.size, &frame.composite.get_bind_group_layout(0), sun));
        }
        if let Some(grade) = &self.grade {
            grade.set_grade(&self.queue, self.enhanced_settings.grade_strength, time_of_day(self.sky.pose.daylight, self.env.sun_dir));
            grade.set_fog_color(&self.queue, fog_color.map(crate::gamma));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Colour ramp through expose + present with no sun boost, returning (native, graded) RGB.
    fn grade_colours(strength: f32, tod: [f32; 3]) -> (Vec<[u8; 3]>, Vec<[u8; 3]>) {
        let native: Vec<[u8; 3]> = (0..=255u8).map(|i| [i, i / 2, 255 - i]).collect();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("grade regression needs an offscreen GPU");
        let texture = |format, usage| r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grade regression"), size: wgpu::Extent3d { width: 256, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
        });
        let base = texture(wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let sun_tex = texture(wgpu::TextureFormat::Rgba32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let out = texture(r.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let pixels: Vec<u8> = native.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        let sun_data: Vec<f32> = (0..256).flat_map(|_| [0.0_f32, 0.0, 0.0, 1.0]).collect();
        let copy = wgpu::Extent3d { width: 256, height: 1, depth_or_array_layers: 1 };
        r.queue.write_texture(base.as_image_copy(), &pixels, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(1024), rows_per_image: Some(1) }, copy);
        r.queue.write_texture(sun_tex.as_image_copy(), bytemuck::cast_slice(&sun_data), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4096), rows_per_image: Some(1) }, copy);
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }] });
        let base_view = base.create_view(&Default::default());
        let base_bg = r.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&base_view) }] });
        let grade = Grade::new(&r.device, r.format, (256, 1), &layout, &sun_tex.create_view(&Default::default()));
        grade.set_grade(&r.queue, strength, tod);
        let mut encoder = r.device.create_command_encoder(&Default::default());
        grade.expose(&r.queue, &mut encoder, &base_bg, 0.0, 0.0);
        grade.present(&mut encoder, &out.create_view(&Default::default()), None);
        r.queue.submit([encoder.finish()]);
        let rgba = crate::texture_to_rgba(&r, &out, 256, 1).unwrap();
        let bgra = r.format == wgpu::TextureFormat::Bgra8Unorm;
        (native, rgba.chunks(4).map(|p| if bgra { [p[2], p[1], p[0]] } else { [p[0], p[1], p[2]] }).collect())
    }

    /// Runs the real expose + present shaders over a 256x1 grey ramp; `sun` is the sun weight written to every pixel.
    fn ramp(boost: f32, sun: f32, tod: [f32; 3]) -> Vec<u8> { ramp_halo(boost, [sun, 0.0], tod, [0.0; 4], 0.0) }

    /// As `ramp`, with sun-mask [directional share, sky-open], and uniform `halo` applied at bloom `strength`.
    fn ramp_halo(boost: f32, sun: [f32; 2], tod: [f32; 3], halo: [f32; 4], strength: f32) -> Vec<u8> {
        ramp_fog(boost, [sun[0], sun[1], 0.0], tod, halo, strength, [0.0; 3])
    }

    /// As `ramp_halo`, with mask [directional share, attenuated sky-open, fog] and framebuffer-space fog colour.
    fn ramp_fog(boost: f32, sun: [f32; 3], tod: [f32; 3], halo: [f32; 4], strength: f32, fog_color: [f32; 3]) -> Vec<u8> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("grade regression needs an offscreen GPU");
        let size = (256, 1);
        let texture = |format, usage| r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grade regression"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
        });
        let base = texture(wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let sun_tex = texture(wgpu::TextureFormat::Rgba32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let out = texture(r.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let ramp: Vec<u8> = (0..=255u8).flat_map(|v| [v, v, v, 255]).collect();
        let sun_data: Vec<f32> = (0..256).flat_map(|_| [sun[0], sun[1], sun[2], 1.0]).collect();
        let copy = wgpu::Extent3d { width: size.0, height: 1, depth_or_array_layers: 1 };
        r.queue.write_texture(base.as_image_copy(), &ramp, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(1024), rows_per_image: Some(1) }, copy);
        r.queue.write_texture(sun_tex.as_image_copy(), bytemuck::cast_slice(&sun_data), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4096), rows_per_image: Some(1) }, copy);
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }] });
        let base_view = base.create_view(&Default::default());
        let base_bg = r.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&base_view) }] });
        let grade = Grade::new(&r.device, r.format, size, &layout, &sun_tex.create_view(&Default::default()));
        grade.set_grade(&r.queue, 0.0, tod);
        grade.set_fog_color(&r.queue, fog_color);
        let halo_size = crate::bloom::halo_size(size);
        let halo_bits: Vec<[u16; 4]> = (0..halo_size.0 * halo_size.1).map(|_| halo.map(crate::bloom::tests::f16_bits)).collect();
        r.queue.write_texture(grade.halo_view().texture().as_image_copy(), bytemuck::cast_slice(&halo_bits),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(halo_size.0 * 8), rows_per_image: Some(1) },
            wgpu::Extent3d { width: halo_size.0, height: 1, depth_or_array_layers: 1 });
        let mut encoder = r.device.create_command_encoder(&Default::default());
        grade.expose(&r.queue, &mut encoder, &base_bg, boost, strength);
        grade.present(&mut encoder, &out.create_view(&Default::default()), None);
        r.queue.submit([encoder.finish()]);
        let pixels = crate::texture_to_rgba(&r, &out, size.0, 1).unwrap();
        pixels.chunks(4).map(|p| p[0]).collect()
    }

    #[test]
    fn neutral_and_unlit_pixels_keep_every_native_8_bit_code() {
        let native: Vec<u8> = (0..=255).collect();
        assert_eq!(ramp(0.0, 1.0, [1.0, 0.0, 0.0]), native, "zero boost must reproduce the faithful codes despite dither");
        assert_eq!(ramp(0.6, 0.0, [1.0, 0.0, 0.0]), native, "pixels without a sun weight (shadow, fog, sky) are never lifted");
        assert_eq!(ramp(0.6, 0.26, [0.0, 0.0, 1.0]), native, "authored night never brightens even with a moon share inside the ramp");
        assert_eq!(ramp(0.6, 1.0, [0.0, 0.0, 1.0]), native, "authored night gates exposure regardless of directional share");
    }

    #[test]
    fn authored_night_blocks_even_bright_environment_sun_on_gpu() {
        let scene = ao_scene::Scene {
            daylight: Some(0.0),
            environment: Some(ao_scene::Environment {
                sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 0.5, fog_end: 800.0,
                ambient: [0.0; 3], sun_color: [1.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0,
            }),
            ..Default::default()
        };
        let weights = time_of_day(scene.daylight, scene.environment.unwrap().sun_dir);
        assert_eq!(weights, [0.0, 0.0, 1.0]);
        let native: Vec<u8> = (0..=255).collect();
        assert_eq!(ramp_halo(0.6, [1.0, 1.0], weights, [0.0; 4], 0.0), native);
        for missing in [None, Some(f32::NAN), Some(f32::INFINITY)] {
            assert_eq!(time_of_day(missing, [0.0, 1.0, 0.0]), weights);
        }
        assert_eq!(time_of_day(Some(1.0), [0.0, 1.0, 0.0]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn sky_open_daylight_shade_lifts_but_closed_and_night_stay_native() {
        let native: Vec<u8> = (0..=255).collect();
        let shade = |boost, mask, tod| ramp_halo(boost, mask, tod, [0.0; 4], 0.0);
        let day = [1.0, 0.0, 0.0];
        let open = shade(0.5, [0.0, 1.0], day);
        assert!((165..=168).contains(&open[128]), "sky-open shade gets a bounded 30% lift: {}", open[128]);
        assert_eq!(open, ramp(0.5, 1.0, day), "sky-open shade retains its lift while direct sunlight uses the same bounded gamma-space exposure");
        assert_eq!(shade(0.5, [0.0, 0.0], day), native, "closed shade remains native");
        assert_eq!(shade(0.5, [0.26, 1.0], [0.0, 0.0, 1.0]), native, "sky-open moonlit night remains native");
        assert_eq!(shade(0.0, [0.0, 1.0], day), native, "zero boost ignores sky openness");
    }

    #[test]
    fn daylight_exposure_never_boosts_the_fog_component() {
        let day = [1.0, 0.0, 0.0];
        let fogged = |mask, color| ramp_fog(0.5, mask, day, [0.0; 4], 0.0, color);
        let native: Vec<u8> = (0..=255).collect();
        assert_eq!(fogged([0.0, 0.0, 1.0], [1.0; 3]), native, "fully fogged terrain has no sky-open lift");
        let partial = fogged([0.0, 0.5, 0.5], [0.4; 3]);
        assert_eq!(&partial[..=51], &native[..=51], "pixels with no radiance beyond the fog contribution are not lifted");
        assert!((139..=141).contains(&partial[128]), "only the non-fog 77 codes receive the 15% sky lift: {}", partial[128]);
        let without_fog_color = fogged([0.0, 0.5, 0.5], [0.0; 3]);
        assert!((146..=149).contains(&without_fog_color[128]), "zero fog radiance leaves all native radiance eligible: {}", without_fog_color[128]);
        assert!(partial[128] < without_fog_color[128], "fog subtraction must reduce the added light");
        let direct = fogged([1.0, 0.0, 0.5], [0.4; 3]);
        assert_eq!(&direct[..=51], &native[..=51], "direct sun must not brighten fog radiance either");
        assert!((150..=152).contains(&direct[128]), "only the non-fog 77 codes receive the 30% direct lift: {}", direct[128]);
        let camera_fog = fogged([0.0, 0.5, 0.5], [0.8; 3]);
        assert_eq!(&camera_fog[..=102], &native[..=102], "the camera volume's brighter fog contribution is excluded");
        assert!((131..=133).contains(&camera_fog[128]), "camera-local fog colour, not the zone colour, controls exposed radiance: {}", camera_fog[128]);
    }

    /// Grey ramp is chroma-free, so grade it through colour: (i, i/2, 255-i) pixels, luminance compared per code.
    #[test]
    fn chroma_grade_preserves_luminance_and_is_off_at_zero_strength() {
        let strength = |s: f32, tod: [f32; 3]| grade_colours(s, tod);
        let luma = |p: [u8; 3]| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
        for tod in [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            let (native, graded) = strength(1.0, tod);
            assert!(native.iter().zip(&graded).any(|(a, b)| a != b), "grade must change chroma for {tod:?}");
            for (a, b) in native.iter().zip(&graded) {
                assert!(luma(*b) <= luma(*a) + 1.5, "grade lifted luminance {a:?} -> {b:?} at {tod:?}");
                assert!(luma(*b) >= luma(*a) - 1.5, "grade dimmed luminance {a:?} -> {b:?} at {tod:?}");
            }
        }
        let (native, graded) = strength(0.0, [1.0, 1.0, 0.0]);
        assert_eq!(native, graded, "strength 0 keeps native codes");
    }

    #[test]
    fn daylight_sun_brightens_monotonically_and_reaches_white_exactly() {
        let day = [1.0, 0.0, 0.0];
        let lit = ramp(0.35, 1.0, day);
        assert_eq!(lit, ramp(0.35, 1.0, day), "static frames are identical (fixed-pattern dither)");
        assert_eq!(lit[255], 255);
        assert!(lit.windows(2).all(|w| i32::from(w[0]) <= i32::from(w[1]) + 1), "tonemap must stay monotone within dither");
        assert!(lit.iter().enumerate().all(|(i, &v)| i32::from(v) + 1 >= i as i32), "never darker than native");
        assert!(lit[128] >= 128 + 24, "mid-tone sunlit surfaces gain visible brightness without the former washout: {}", lit[128]);
        let defaults = ramp(0.6, 1.0, day);
        assert!((173..=175).contains(&defaults[128]), "default direct exposure is bounded to 36%, not the washed-out 60%: {}", defaults[128]);
        let daylight_share = ramp(0.5, 0.26, day);
        assert!(daylight_share[128] >= 128 + 10, "ordinary daylight shares gain visible brightness: {}", daylight_share[128]);
        assert_eq!(ramp(0.5, 0.5, day), ramp(0.5, 1.0, day), "sun exposure reaches its full authored boost at share 0.5");
        assert_eq!(ramp(0.6, 1.0, [0.5, 0.0, 0.0]), ramp(0.3, 1.0, day), "authored day continuously weights the boost");
    }

    #[test]
    fn bloom_halo_adds_above_white_with_authored_cap_and_graceful_shoulder() {
        let native: Vec<u8> = (0..=255).collect();
        assert_eq!(ramp_halo(0.0, [0.0; 2], [0.0, 0.0, 1.0], [0.2; 4], 0.0), native, "strength 0 (bloom off) ignores a stale halo");
        assert_eq!(ramp_halo(0.0, [0.0; 2], [0.0, 0.0, 1.0], [0.0; 4], 0.6), native, "an empty halo changes nothing");
        // Authored (a = 0) glare over black: 0.2 * 0.6 * 4 = 0.48 luma, capped to the 64-code authored budget.
        let authored = ramp_halo(0.0, [0.0; 2], [0.0, 0.0, 1.0], [0.2, 0.2, 0.2, 0.0], 0.6);
        assert!((61..=65).contains(&(authored[100] - 100)), "authored night halo keeps the 64 display-code cap: {}", authored[100]);
        // Highlight glare (a = its luma) is not capped.
        let lit = ramp_halo(0.35, [1.0, 0.0], [1.0, 0.0, 0.0], [0.1, 0.1, 0.1, 0.1], 0.6);
        assert!(lit[100] >= 100 + 58, "highlight halo adds its full energy: {}", lit[100]);
        assert!(lit.windows(2).all(|w| i32::from(w[0]) <= i32::from(w[1]) + 1), "tonemap stays monotone with halo");
        assert_eq!(lit[255], 255);
        assert!(lit[200] < 255 && lit[220] < 255, "halo energy rolls into the shoulder instead of clipping at the pixel's old white: {} {}", lit[200], lit[220]);
        let plain = ramp(0.35, 1.0, [1.0, 0.0, 0.0]);
        for (i, (&with, &without)) in lit.iter().zip(&plain).enumerate() { assert!(i32::from(with) + 1 >= i32::from(without), "halo never darkens a pixel: {i} {with} {without}"); }
    }
}
