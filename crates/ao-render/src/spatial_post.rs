//! Opt-in display-space spatial effects, after tonemapping and before the HUD.
//! Algorithms adapted from ao-vk (MIT, Copyright (c) 2026 wannabuh).
//! See THIRD_PARTY. All samples come exclusively from the current visible frame.
use crate::effect_gpu::EffectPassEncoder as _;
use crate::gpu_stats::{GpuStatsExt, TextureViewStatsExt};
use crate::pipeline_creation::RenderPipeline;

#[derive(Clone, Copy, Default)]
pub(super) struct SpatialSettings {
    pub motion_blur: f32,
    pub fxaa: bool,
    pub cas: bool,
    pub cas_strength: f32,
    pub depth_of_field: bool,
    pub dof_aperture: f32,
    pub dof_max_radius: f32,
}

#[derive(Clone, Copy)]
pub(super) struct Projection {
    pub near: f32,
    pub far: f32,
    /// Native gamma-space fog colour, matching the current frame's fog fraction.
    pub fog_color: [f32; 3],
    /// Positive view-space depth of the exact own-character world position.
    /// Missing/behind-camera focus disables DoF; never substitute screen-centre depth.
    pub focus_depth: Option<f32>,
}

fn finite_clamp(value: f32, max: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, max) } else { 0.0 }
}
fn parameters(settings: SpatialSettings, projection: Projection) -> ([f32; 12], [bool; 4]) {
    let valid_projection = projection.near.is_finite() && projection.far.is_finite()
        && projection.near > 0.0 && projection.far > projection.near;
    let focus = projection.focus_depth.filter(|z| valid_projection && z.is_finite() && *z >= projection.near && *z <= projection.far);
    let strength = finite_clamp(settings.cas_strength, 1.0);
    let aperture = finite_clamp(settings.dof_aperture, 8.0);
    let radius = finite_clamp(settings.dof_max_radius, 16.0);
    let (near, far) = if valid_projection { (projection.near, projection.far) } else { (0.1, 1.0) };
    let motion = finite_clamp(settings.motion_blur, 1.0);
    let fog = projection.fog_color.map(|c| finite_clamp(c, 1.0));
    ([near, far, focus.unwrap_or(0.0), aperture, radius, strength, motion, 0.0, fog[0], fog[1], fog[2], 0.0],
        [motion > 0.0 && valid_projection, settings.depth_of_field && focus.is_some() && aperture > 0.0 && radius > 0.0,
         settings.fxaa && valid_projection, settings.cas && strength > 0.0 && valid_projection])
}

struct Pass {
    output: wgpu::TextureView,
    input: wgpu::BindGroup,
    present: wgpu::BindGroup,
}
struct Frame {
    size: (u32, u32),
    source: wgpu::TextureView,
    depth: wgpu::TextureView,
    fog: wgpu::TextureView,
    motion: Option<(wgpu::TextureView, wgpu::TextureView)>,
    enabled: [bool; 4],
    passes: [Option<Pass>; 4],
}
pub(super) struct SpatialPost {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    source_layout: wgpu::BindGroupLayout,
    pipelines: [RenderPipeline; 4],
    params: wgpu::Buffer,
    fallback_identity: wgpu::TextureView,
    frame: Option<Frame>,
}
impl SpatialPost {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, source_layout: &wgpu::BindGroupLayout) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spatial post inputs"), entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: false }),
                texture(1, wgpu::TextureSampleType::Depth),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                texture(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture(4, wgpu::TextureSampleType::Uint),
                texture(5, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Spatial post"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipelines = [
            ("motion_blur.wgsl", include_str!("motion_blur.wgsl")),
            ("depth_of_field.wgsl", include_str!("depth_of_field.wgsl")),
            ("fxaa.wgsl", include_str!("fxaa.wgsl")),
            ("cas.wgsl", include_str!("cas.wgsl")),
        ].map(|(name, embedded)| {
            let source = format!("{COMMON}\n{}", crate::shader_source::load(name, embedded));
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(source.into()) });
            crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
                label: Some(name), layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
            })
        });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("Spatial post parameters"), size: 48, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let fallback_identity = device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Spatial unused identity"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view_tracked(&Default::default());
        Self { format, layout, source_layout: source_layout.clone(), pipelines, params, fallback_identity, frame: None }
    }
    pub fn is_ready(&self) -> bool { self.pipelines.iter().all(RenderPipeline::is_ready) }
    pub fn wait(&self) { for pipeline in &self.pipelines { pipeline.get(); } }
    /// Release targets immediately when controls turn off or the scene changes.
    pub fn clear(&mut self) { self.frame = None; }
    /// Owned texture/uniform storage, excluding shared inputs and driver overhead.
    pub fn bytes(&self) -> u64 {
        52 + self.frame.as_ref().map_or(0, |frame| {
            u64::from(frame.size.0) * u64::from(frame.size.1)
                * u64::from(self.format.block_copy_size(None).unwrap_or(4))
                * frame.passes.iter().filter(|pass| pass.is_some()).count() as u64
        })
    }
    /// Returns a source-layout bind group for EnhancedFrame::composite. With no
    /// active effects no pass is encoded and the caller retains its original input.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView, depth: &wgpu::TextureView, fog: &wgpu::TextureView, motion: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
        size: (u32, u32), settings: SpatialSettings, projection: Projection,
    ) -> Option<&wgpu::BindGroup> {
        let (params, mut enabled) = parameters(settings, projection);
        enabled[0] &= motion.is_some();
        if !enabled.iter().any(|&on| on) || size.0 == 0 || size.1 == 0 { self.clear(); return None; }
        if self.frame.as_ref().is_none_or(|f| f.size != size || f.source != *source || f.depth != *depth || f.fog != *fog || f.enabled != enabled
            || f.motion.as_ref().map(|(v,i)| (v,i)) != motion) {
            let mut input = source.clone();
            let passes = std::array::from_fn(|index| {
                if !enabled[index] { return None; }
                let output = device.create_texture_tracked(&wgpu::TextureDescriptor {
                    label: Some("Spatial post display colour"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
                }).create_view_tracked(&Default::default());
                let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Spatial post frame"), layout: &self.layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&input) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(motion.map_or(source, |m| m.0)) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(motion.map_or(&self.fallback_identity, |m| m.1)) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(fog) },
                ] });
                let present = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Spatial post presentation"), layout: &self.source_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&output) }] });
                input = output.clone();
                Some(Pass { output, input: bind, present })
            });
            self.frame = Some(Frame { size, source: source.clone(), depth: depth.clone(), fog: fog.clone(), motion: motion.map(|(v,i)| (v.clone(),i.clone())), enabled, passes });
        }
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&params));
        let frame = self.frame.as_ref()?;
        let mut result = None;
        for (index, pass) in frame.passes.iter().enumerate() {
            let Some(pass) = pass else { continue; };
            let mut render = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(["motion blur", "depth of field", "FXAA", "CAS"][index]), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pass.output, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            render.set_pipeline(&self.pipelines[index]); render.set_bind_group(0, &pass.input, &[]); render.draw(0..3, 0..1);
            result = Some(&pass.present);
        }
        result
    }
}

// Shared WGSL is prepended to each standalone feature kernel, including override sources.
const COMMON: &str = r#"
struct Params { projection: vec4<f32>, effect: vec4<f32>, fog_color: vec4<f32> }
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(5) var fog_mask: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(array<vec2<f32>,3>(vec2(-1.0,-1.0), vec2(3.0,-1.0), vec2(-1.0,3.0))[i],0.0,1.0);
}
fn coord(p: vec2<i32>) -> vec2<i32> { return clamp(p, vec2<i32>(0), vec2<i32>(textureDimensions(image))-vec2<i32>(1)); }
fn colour(p: vec2<i32>) -> vec3<f32> { return textureLoad(image,coord(p),0).rgb; }
fn view_z(p: vec2<i32>) -> f32 {
    let d = textureLoad(depth,coord(p),0);
    let n = params.projection.x; let f = params.projection.y;
    return n*f / max(f-d*(f-n), 0.00001);
}
fn luma(c: vec3<f32>) -> f32 { return dot(c,vec3(0.299,0.587,0.114)); }
// Normalized positive gathers only admit taps at least as fogged as the centre.
// CAS sharpens bounded recovered surface colour, then restores native extinction.
fn fog_fraction(p: vec2<i32>) -> f32 { return clamp(textureLoad(fog_mask,coord(p),0).b,0.0,1.0); }
fn safe_output(c: vec3<f32>, _original: vec3<f32>, alpha: f32, _p: vec2<i32>) -> vec4<f32> {
    return vec4<f32>(clamp(c,vec3(0.0),vec3(1.0)),alpha);
}
// Conservative surface guard: never import farther geometry through foreground.
// Also reject nearer foreground across depth silhouettes (no actor-colour halos).
fn surface_sample(p: vec2<i32>, centre: vec2<i32>) -> vec3<f32> {
    if fog_fraction(p) < fog_fraction(centre) { return colour(centre); }
    let z = view_z(centre);
    if abs(view_z(p)-z) > max(0.01,z*0.01) { return colour(centre); }
    return colour(p);
}
fn linear_sample(p: vec2<f32>, centre: vec2<i32>) -> vec3<f32> {
    let b = vec2<i32>(floor(p)); let t = fract(p);
    return mix(mix(surface_sample(b,centre),surface_sample(b+vec2(1,0),centre),t.x),
        mix(surface_sample(b+vec2(0,1),centre),surface_sample(b+vec2(1,1),centre),t.x),t.y);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_exact_focus_disables_only_dof() {
        let settings = SpatialSettings { motion_blur: 0.0, fxaa: true, cas: true, cas_strength: 0.5, depth_of_field: true, dof_aperture: 2.0, dof_max_radius: 8.0 };
        let projection = Projection { near: 0.1, far: 100.0, fog_color: [0.3;3], focus_depth: None };
        assert_eq!(parameters(settings, projection).1, [false,false,true,true]);
        for focus in [f32::NAN, f32::INFINITY, -1.0, 0.0, 101.0] {
            assert!(!parameters(settings, Projection { focus_depth: Some(focus), ..projection }).1[1]);
        }
        assert_eq!(parameters(settings, Projection { focus_depth: Some(4.0), ..projection }).1, [false,true,true,true]);
        assert_eq!(parameters(SpatialSettings::default(), projection).1, [false;4]);
    }
    #[test]
    fn spatial_shaders_validate_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("spatial shader validation requires offscreen GPU");
        let scope = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let post = SpatialPost::new(&renderer.device, renderer.format, renderer.enhanced_pipelines.source_layout());
        post.wait();
        assert!(post.is_ready());
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        let size = (64, 4);
        let texture = |format, usage| renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Spatial regression"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
        });
        let source = texture(renderer.format, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let pixels: Vec<u8> = (0..size.0*size.1).flat_map(|i| {
            let value = if i%size.0 < size.0/2 { 80 } else { 150 };
            [value,value,value,255]
        }).collect();
        renderer.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &source, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All },
            &pixels, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0*4), rows_per_image: Some(size.1) },
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 });
        let source = source.create_view_tracked(&Default::default());
        let depth = texture(crate::DEPTH, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT).create_view_tracked(&Default::default());
        let output = texture(renderer.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let output_view = output.create_view_tracked(&Default::default());
        let boundary = crate::enhanced::EnhancedFrame::new(&renderer.device, renderer.format, size, 1, crate::EnhancedSettings::default(), false, &renderer.enhanced_pipelines);
        let mut post = post;
        let projection = Projection { near: 0.1, far: 100.0, fog_color: [0.3;3], focus_depth: Some(4.0) };
        let motion = texture(wgpu::TextureFormat::Rgba32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let ids = texture(wgpu::TextureFormat::R32Uint, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let vectors: Vec<[f32;4]> = vec![[0.1,0.0,0.5,0.0]; (size.0*size.1) as usize];
        let identities = vec![1u32; (size.0*size.1) as usize];
        for (target, bytes, stride) in [(&motion, bytemuck::cast_slice(&vectors),16), (&ids,bytemuck::cast_slice(&identities),4)] {
            renderer.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All },
                bytes, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0*stride), rows_per_image: Some(size.1) },
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 });
        }
        let motion = motion.create_view_tracked(&Default::default());
        let ids = ids.create_view_tracked(&Default::default());
        let fog_texture = texture(wgpu::TextureFormat::Rgba32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let fog = fog_texture.create_view_tracked(&Default::default());
        for fog_case in 0..4 {
            let mask: Vec<[f32;4]> = (0..size.0*size.1).map(|i| {
                let fraction = match fog_case { 0 => 0.0, 1 => 0.5, 3 => 1.0, _ => if i%size.0 < size.0/2 {0.2} else {0.8} };
                [0.0,0.0,fraction,0.0]
            }).collect();
            renderer.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &fog_texture, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&mask), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0*16), rows_per_image: Some(size.1) },
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 });
        for bits in 1..16 {
            let settings = SpatialSettings {
                depth_of_field: bits&1 != 0, fxaa: bits&2 != 0, cas: bits&4 != 0,
                dof_aperture: 2.0, dof_max_radius: 8.0, cas_strength: 0.5,
                motion_blur: if bits&8 != 0 { 1.0 } else { 0.0 },
            };
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            {
                let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Spatial regression depth"), color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.5), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                    timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
            }
            let result = post.encode(&renderer.device, &renderer.queue, &mut encoder, &source, &depth, &fog, Some((&motion,&ids)), size, settings, projection).unwrap();
            boundary.composite(&mut encoder, &output_view, Some(result), None);
            renderer.queue.submit([encoder.finish()]);
            let actual = crate::texture_to_rgba(&renderer, &output, size.0, size.1).unwrap();
            if fog_case < 2 && (bits == 1 || bits == 4) {
                assert_ne!(actual,pixels,"uniform-fog bokeh/sharpening must remain functional");
                assert!(actual.as_chunks::<4>().0.iter().zip(pixels.as_chunks::<4>().0)
                    .any(|(a,n)| a[0] > n[0]),"bokeh highlights/sharpening must brighten some texels even in uniform native fog");
            } else if fog_case == 3 && bits == 4 {
                assert_eq!(actual,pixels,"fully extinguished pixels must bypass CAS exactly");
            } else if fog_case == 2 && matches!(bits,1|2|8) {
                for (i,(a,n)) in actual.as_chunks::<4>().0.iter().zip(pixels.as_chunks::<4>().0).enumerate() {
                    let left = (i%size.0 as usize) < size.0 as usize/2;
                    if !left {
                        assert_eq!(a,n,"signed fog guard imported forbidden tap ({bits}, pixel {i})");
                    }
                }
            }
            assert!(actual.as_chunks::<4>().0.iter().all(|p|p[3]==255));
            assert_eq!(post.bytes(), 52 + u64::from(size.0) * u64::from(size.1) * 4 * u64::from((bits as u32).count_ones()));
        }
        }
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        assert!(post.encode(&renderer.device, &renderer.queue, &mut encoder, &source, &depth, &fog, None, size, SpatialSettings::default(), projection).is_none());
        assert!(post.frame.is_none(), "all-off must release every target and bind group");
        assert_eq!(post.bytes(), 52, "off retains only uniform and fallback texel");
    }
}
