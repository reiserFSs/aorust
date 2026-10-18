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
    /// Compact native fog is in R; the full sun mask keeps it in B.
    pub fog_red: bool,
    /// Positive view-space depth of the exact own-character world position.
    /// Missing/behind-camera focus disables DoF; never substitute screen-centre depth.
    pub focus_depth: Option<f32>,
}

fn finite_clamp(value: f32, max: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, max) } else { 0.0 }
}
fn parameters(settings: SpatialSettings, projection: Projection) -> ([f32; 16], [bool; 4]) {
    let valid_projection = projection.near.is_finite() && projection.far.is_finite()
        && projection.near > 0.0 && projection.far > projection.near;
    let focus = projection.focus_depth.filter(|z| valid_projection && z.is_finite() && *z >= projection.near && *z <= projection.far);
    let strength = finite_clamp(settings.cas_strength, 1.0);
    let aperture = finite_clamp(settings.dof_aperture, 8.0);
    let radius = finite_clamp(settings.dof_max_radius, 16.0);
    let (near, far) = if valid_projection { (projection.near, projection.far) } else { (0.1, 1.0) };
    let motion = finite_clamp(settings.motion_blur, 1.0);
    let fog = projection.fog_color.map(|c| finite_clamp(c, 1.0));
    ([near, far, focus.unwrap_or(0.0), aperture, radius, strength, motion, near.recip(), fog[0], fog[1], fog[2], far.recip(),
        f32::from(u8::from(projection.fog_red)), 0.0, 0.0, 0.0],
        [motion > 0.0 && valid_projection, settings.depth_of_field && focus.is_some() && aperture > 0.0 && radius > 0.0,
         settings.fxaa && valid_projection, settings.cas && strength > 0.0 && valid_projection])
}

fn dof_sizes(size: (u32, u32)) -> [(u32, u32); 2] {
    let half = (size.0.div_ceil(2), size.1.div_ceil(2));
    [half, (half.0.div_ceil(8), half.1.div_ceil(8))]
}

struct DofFrame {
    // Half colour + signed CoC alpha, half fog, raw/dilated tile maxima, near/far results.
    targets: [wgpu::TextureView; 6],
    inputs: [wgpu::BindGroup; 4],
}

struct Pass {
    output: Option<wgpu::TextureView>,
    input: wgpu::BindGroup,
}
struct Frame {
    size: (u32, u32),
    source: wgpu::TextureView,
    depth: wgpu::TextureView,
    fog: wgpu::TextureView,
    motion: Option<(wgpu::TextureView, wgpu::TextureView)>,
    enabled: [bool; 4],
    passes: [Option<Pass>; 4],
    dof: Option<DofFrame>,
}
pub(super) struct SpatialPost {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    pipelines: [RenderPipeline; 3],
    dof_layout: wgpu::BindGroupLayout,
    dof_pipelines: [RenderPipeline; 5],
    dof_fallback: wgpu::TextureView,
    dof_aperture: wgpu::Buffer,
    dof_sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    fallback_identity: wgpu::TextureView,
    frame: Option<Frame>,
}
impl SpatialPost {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
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
        let dof_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Half-resolution DoF inputs"), entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: false }),
                texture(1, wgpu::TextureSampleType::Depth),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                texture(3, wgpu::TextureSampleType::Float { filterable: false }),
                texture(4, wgpu::TextureSampleType::Float { filterable: true }),
                texture(5, wgpu::TextureSampleType::Float { filterable: true }),
                texture(6, wgpu::TextureSampleType::Float { filterable: false }),
                texture(7, wgpu::TextureSampleType::Float { filterable: true }),
                texture(8, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry { binding: 9, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 10, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let dof_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Half-resolution DoF"), bind_group_layouts: &[Some(&dof_layout)], immediate_size: 0,
        });
        let dof_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dof_half.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("dof_half.wgsl", include_str!("dof_half.wgsl"))),
        });
        let dof_pipelines = ["fs_prepare", "fs_tiles", "fs_dilate", "fs_gather", "fs_upsample"].map(|entry| {
            let target = Some(wgpu::ColorTargetState {
                format: match entry {
                    "fs_upsample" => format,
                    "fs_tiles" | "fs_dilate" => wgpu::TextureFormat::Rg16Float,
                    _ => wgpu::TextureFormat::Rgba16Float,
                },
                blend: None, write_mask: wgpu::ColorWrites::ALL,
            });
            let mut targets = [target.clone(), target];
            if entry == "fs_prepare" { targets[1].as_mut().unwrap().format = wgpu::TextureFormat::R16Float; }
            let count = if matches!(entry, "fs_prepare" | "fs_gather") { 2 } else { 1 };
            crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
                label: Some(entry), layout: Some(&dof_pl),
                vertex: wgpu::VertexState { module: &dof_module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &dof_module, entry_point: Some(entry), compilation_options: Default::default(), targets: &targets[..count] }),
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
            })
        });
        let dof_fallback = device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("DoF unused half input"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view_tracked(&Default::default());
        let dof_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("DoF gather sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("Spatial post parameters"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        // Immutable uniform indexing avoids materializing a shader-local aperture array per fragment.
        let taps: [[f32; 4]; 48] = std::array::from_fn(|i| {
            let radius = ((i as f64 + 0.5) / 48.0).sqrt();
            let (sin, cos) = (i as f64 * 2.399_963_23).sin_cos();
            [(radius * cos) as f32, (radius * sin) as f32, radius as f32, 0.0]
        });
        let dof_aperture = device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some("DoF golden spiral aperture"), size: std::mem::size_of_val(&taps) as u64,
            usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: true,
        });
        dof_aperture.slice(..).get_mapped_range_mut().copy_from_slice(bytemuck::cast_slice(&taps));
        dof_aperture.unmap();
        let fallback_identity = device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Spatial unused identity"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view_tracked(&Default::default());
        Self { format, layout, pipelines, dof_layout, dof_pipelines,
            dof_fallback, dof_aperture, dof_sampler, params, fallback_identity, frame: None }
    }
    pub fn is_ready(&self) -> bool { self.pipelines.iter().chain(&self.dof_pipelines).all(RenderPipeline::is_ready) }
    pub fn wait(&self) { for pipeline in self.pipelines.iter().chain(&self.dof_pipelines) { pipeline.get(); } }
    /// Release targets immediately when controls turn off or the scene changes.
    pub fn clear(&mut self) { self.frame = None; }
    /// Owned texture/uniform storage, excluding shared inputs and driver overhead.
    pub fn bytes(&self) -> u64 {
        self.params.size() + 12 + self.dof_aperture.size() + self.frame.as_ref().map_or(0, |frame| {
            let full = u64::from(frame.size.0) * u64::from(frame.size.1)
                * u64::from(self.format.block_copy_size(None).unwrap_or(4))
                * frame.passes.iter().flatten().filter(|pass| pass.output.is_some()).count() as u64;
            let [half, tiles] = dof_sizes(frame.size);
            full + if frame.dof.is_some() {
                u64::from(half.0) * u64::from(half.1) * 26 + u64::from(tiles.0) * u64::from(tiles.1) * 8
            } else { 0 }
        })
    }
    /// Writes the last enabled effect directly to output. Returns false without
    /// encoding when inactive; the caller then presents its unchanged source.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView, depth: &wgpu::TextureView, fog: &wgpu::TextureView, motion: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
        size: (u32, u32), settings: SpatialSettings, projection: Projection,
        output: &wgpu::TextureView, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) -> bool {
        let (params, mut enabled) = parameters(settings, projection);
        enabled[0] &= motion.is_some();
        let Some(last) = enabled.iter().rposition(|&on| on).filter(|_| size.0 > 0 && size.1 > 0) else {
            self.clear(); return false;
        };
        if self.frame.as_ref().is_none_or(|f| f.size != size || f.source != *source || f.depth != *depth || f.fog != *fog || f.enabled != enabled
            || f.motion.as_ref().map(|(v,i)| (v,i)) != motion) {
            let mut input = source.clone();
            let mut dof = None;
            let passes = std::array::from_fn(|index| {
                if !enabled[index] { return None; }
                let output = (index != last).then(|| device.create_texture_tracked(&wgpu::TextureDescriptor {
                    label: Some("Spatial post display colour"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
                }).create_view_tracked(&Default::default()));
                let bind = if index == 1 {
                    let [half, tiles] = dof_sizes(size);
                    let targets = std::array::from_fn(|i| {
                        let (width, height) = if matches!(i, 2 | 3) { tiles } else { half };
                        device.create_texture_tracked(&wgpu::TextureDescriptor {
                            label: Some(["DoF half colour + CoC", "DoF half fog", "DoF raw tile maxima", "DoF dilated tile maxima", "DoF near result", "DoF far result"][i]),
                            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                            format: match i {
                                1 => wgpu::TextureFormat::R16Float,
                                2 | 3 => wgpu::TextureFormat::Rg16Float,
                                _ => wgpu::TextureFormat::Rgba16Float,
                            },
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
                        }).create_view_tracked(&Default::default())
                    });
                    let [prepare, tiles, dilate, gather, upsample] = [0, 1, 2, 3, 4].map(|stage| {
                        // Even statically unused bindings must not alias this pass's attachments.
                        let views = std::array::from_fn::<_, 5, _>(|i| {
                            let readable = match stage { 0 => false, 1 => i < 2, 2 | 3 => i < 3, _ => true };
                            let target = match i {
                                2 if stage >= 3 => 3, // Gather/upsample use the dilated tile field.
                                3 | 4 => i + 1,
                                _ => i,
                            };
                            if readable { &targets[target] } else { &self.dof_fallback }
                        });
                        device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                            label: Some("Half-resolution DoF frame"), layout: &self.dof_layout, entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&input) },
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                                wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(fog) },
                                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(views[0]) },
                                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(views[1]) },
                                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(views[2]) },
                                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(views[3]) },
                                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(views[4]) },
                                wgpu::BindGroupEntry { binding: 9, resource: self.dof_aperture.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::Sampler(&self.dof_sampler) },
                            ],
                        })
                    });
                    dof = Some(DofFrame { targets, inputs: [prepare, tiles, dilate, gather] });
                    upsample
                } else { device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Spatial post frame"), layout: &self.layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&input) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(motion.map_or(source, |m| m.0)) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(motion.map_or(&self.fallback_identity, |m| m.1)) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(fog) },
                ] }) };
                if let Some(output) = &output { input = output.clone(); }
                Some(Pass { output, input: bind })
            });
            self.frame = Some(Frame { size, source: source.clone(), depth: depth.clone(), fog: fog.clone(), motion: motion.map(|(v,i)| (v.clone(),i.clone())), enabled, passes, dof });
        }
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&params));
        let frame = self.frame.as_ref().expect("active spatial frame allocated");
        for (index, pass) in frame.passes.iter().enumerate() {
            let Some(pass) = pass else { continue; };
            if let Some(dof) = frame.dof.as_ref().filter(|_| index == 1) {
                for (stage, outputs) in [&dof.targets[..2], &dof.targets[2..3], &dof.targets[3..4], &dof.targets[4..]].into_iter().enumerate() {
                    let attachments = std::array::from_fn::<_, 2, _>(|i| outputs.get(i).map(|view| wgpu::RenderPassColorAttachment {
                        view, resolve_target: None, depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    }));
                    let mut render = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some(["DoF prepare", "DoF tiles", "DoF dilation", "DoF gather"][stage]), color_attachments: &attachments[..outputs.len()],
                        depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                    });
                    render.set_pipeline(&self.dof_pipelines[stage]);
                    render.set_bind_group(0, &dof.inputs[stage], &[]);
                    render.draw(0..3, 0..1);
                }
            }
            let mut render = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(["motion blur", "DoF upsample", "FXAA", "CAS"][index]), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pass.output.as_ref().unwrap_or(output), resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None, timestamp_writes: if index == last { timestamps.clone() } else { None }, occlusion_query_set: None, multiview_mask: None,
            });
            let pipeline = match index { 0 => &self.pipelines[0], 1 => &self.dof_pipelines[4], _ => &self.pipelines[index-1] };
            render.set_pipeline(pipeline); render.set_bind_group(0, &pass.input, &[]); render.draw(0..3, 0..1);
        }
        true
    }
}

// Shared WGSL is prepended to each standalone feature kernel, including override sources.
const COMMON: &str = r#"
struct Params { projection: vec4<f32>, effect: vec4<f32>, fog_color: vec4<f32>, mask: vec4<f32> }
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
fn fog_fraction(p: vec2<i32>) -> f32 {
    let mask = textureLoad(fog_mask,coord(p),0);
    return clamp(select(mask.b,mask.r,params.mask.x>0.5),0.0,1.0);
}
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
    fn dof_targets_round_up_without_losing_edge_pixels() {
        assert_eq!(dof_sizes((1, 1)), [(1, 1), (1, 1)]);
        assert_eq!(dof_sizes((64, 32)), [(32, 16), (4, 2)]);
        assert_eq!(dof_sizes((65, 33)), [(33, 17), (5, 3)]);
    }
    #[test]
    fn reciprocal_coc_matches_depth_reconstruction() {
        let settings = SpatialSettings { dof_aperture: 2.0, dof_max_radius: 8.0, ..Default::default() };
        for (near,far) in [(0.1,100.0),(0.2,600.0)] {
            let (params,_) = parameters(settings, Projection { near,far,focus_depth: Some(4.0),fog_color: [0.0;3],fog_red: false });
            for z in [near,2.0,4.0,8.0,64.0,far] {
                let depth = far/(far-near)*(1.0-near/z);
                let inverse = params[7]-depth*(params[7]-params[11]);
                let reconstructed = near*far/(far-depth*(far-near)).max(0.00001);
                let coc = |t: f32| {
                    let magnitude = (t.abs()-0.025).max(0.0)/0.975*params[3];
                    t.signum()*magnitude/(1.0+magnitude)*params[4]
                };
                assert!((coc(1.0-params[2]/reconstructed.max(0.001))
                    -coc(1.0-params[2]*inverse)).abs() < 0.001,
                    "reciprocal CoC must match depth reconstruction at {z}");
            }
        }
    }
    #[test]
    fn unavailable_exact_focus_disables_only_dof() {
        let settings = SpatialSettings { motion_blur: 0.0, fxaa: true, cas: true, cas_strength: 0.5, depth_of_field: true, dof_aperture: 2.0, dof_max_radius: 8.0 };
        let projection = Projection { near: 0.1, far: 100.0, fog_color: [0.3;3], fog_red: false, focus_depth: None };
        assert_eq!(parameters(settings, projection).1, [false,false,true,true]);
        for focus in [f32::NAN, f32::INFINITY, -1.0, 0.0, 101.0] {
            assert!(!parameters(settings, Projection { focus_depth: Some(focus), ..projection }).1[1]);
        }
        assert_eq!(parameters(settings, Projection { focus_depth: Some(4.0), ..projection }).1, [false,true,true,true]);
        assert_eq!(parameters(SpatialSettings::default(), projection).1, [false;4]);
        let blue = parameters(settings, projection).0;
        let red = parameters(settings, Projection { fog_red: true, ..projection }).0;
        assert_eq!(&blue[..12], &red[..12], "mask channel must not overwrite projection/effect/fog fields");
        assert_eq!(blue[12], 0.0);
        assert_eq!(red[12], 1.0);
        assert_eq!(std::mem::size_of_val(&red), 64);
    }
    #[test]
    fn spatial_shaders_validate_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("spatial shader validation requires offscreen GPU");
        let scope = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let post = SpatialPost::new(&renderer.device, renderer.format);
        post.wait();
        assert!(post.is_ready());
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        let size = (64, 32);
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
        let mut post = post;
        let projection = Projection { near: 0.1, far: 100.0, fog_color: [0.3;3], fog_red: false, focus_depth: Some(4.0) };
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
        let red_texture = texture(wgpu::TextureFormat::R16Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let red_fog = red_texture.create_view_tracked(&Default::default());
        for fog_case in 0..4 {
            let mut blue_outputs = Vec::new();
            for fog_red in [false, true] {
            let projection = Projection { fog_red, ..projection };
            let fog_view = if fog_red { &red_fog } else { &fog };
            let mask: Vec<[f32;4]> = (0..size.0*size.1).map(|i| {
                let fraction = match fog_case { 0 => 0.0, 1 => 0.5, 3 => 1.0, _ => if i%size.0 < size.0/2 {0.25} else {0.75} };
                [0.0,0.0,fraction,0.0]
            }).collect();
            renderer.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &fog_texture, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&mask), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0*16), rows_per_image: Some(size.1) },
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 });
            // Exact binary16 fractions match the full mask without introducing quantization differences.
            let red: Vec<u16> = mask.iter().map(|p| match p[2] {
                0.0 => 0x0000, 0.25 => 0x3400, 0.5 => 0x3800, 0.75 => 0x3a00, 1.0 => 0x3c00,
                _ => unreachable!(),
            }).collect();
            renderer.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &red_texture, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&red), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0*2), rows_per_image: Some(size.1) },
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
            assert!(post.encode(&renderer.device, &renderer.queue, &mut encoder, &source, &depth, fog_view, Some((&motion,&ids)), size, settings, projection, &output_view, None));
            renderer.queue.submit([encoder.finish()]);
            let actual = crate::texture_to_rgba(&renderer, &output, size.0, size.1).unwrap();
            if fog_red {
                assert_eq!(actual,blue_outputs[bits as usize-1], "R16F and full B fog must guard identically (case {fog_case}, effects {bits})");
            } else {
                blue_outputs.push(actual.clone());
            }
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
            let [half, tiles] = dof_sizes(size);
            let dof_bytes = if bits&1 != 0 { u64::from(half.0)*u64::from(half.1)*26 + u64::from(tiles.0)*u64::from(tiles.1)*8 } else { 0 };
            assert_eq!(post.bytes(), 844 + dof_bytes + u64::from(size.0) * u64::from(size.1) * 4 * u64::from((bits as u32).count_ones()-1));
        }
            }
        }
        // Exercise the shader, not a CPU copy of CoC: exact focus is untouched,
        // and far depths must not collapse to one hard-clamped blur radius.
        let mut dof_frames = Vec::new();
        for z in [4.0, 8.0, 64.0] {
            let mask = vec![[0.0f32; 4]; (size.0*size.1) as usize];
            renderer.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &fog_texture, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&mask), wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0*16), rows_per_image: Some(size.1) },
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 });
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            {
                let d = projection.far/(projection.far-projection.near)
                    * (1.0-projection.near/z);
                let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("DoF focal depth regression"), color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(d), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                    timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
            }
            let settings = SpatialSettings { depth_of_field: true, dof_aperture: 2.0, dof_max_radius: 8.0, ..Default::default() };
            assert!(post.encode(&renderer.device, &renderer.queue, &mut encoder, &source, &depth, &fog, None, size, settings, projection, &output_view, None));
            renderer.queue.submit([encoder.finish()]);
            dof_frames.push(crate::texture_to_rgba(&renderer, &output, size.0, size.1).unwrap());
        }
        assert_eq!(dof_frames[0], pixels, "own-character focal plane must remain sharp");
        assert_ne!(dof_frames[1], pixels, "background must blur");
        assert_ne!(dof_frames[1], dof_frames[2], "far blur must keep progressing past twice the focus depth");
        // One native-sized bright texel at z=64 over a z=8 background. Its
        // aperture must survive that real depth difference, but never touch
        // the exact-focus foreground strip at x<30.
        let spot = texture(renderer.format, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let spot_pixels: Vec<u8> = (0..size.0*size.1).flat_map(|i| {
            let (x,y) = (i%size.0, i/size.0);
            if x < 30 { [40,80,120,255] } else if (x,y)==(32,16) { [255;4] } else { [0,0,0,255] }
        }).collect();
        renderer.queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &spot, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All },
            &spot_pixels, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0*4), rows_per_image: Some(size.1) },
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 });
        let spot = spot.create_view_tracked(&Default::default());
        let depth_shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DoF highlight depth regression"),
            source: wgpu::ShaderSource::Wgsl(r#"
                @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                    return vec4<f32>(array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0))[i],0.0,1.0);
                }
                @fragment fn fs(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
                    var z = 8.0;
                    if p.x < 30.0 { z = 4.0; }
                    else if all(vec2<i32>(p.xy) == vec2<i32>(32,16)) { z = 64.0; }
                    return 100.0/99.9*(1.0-0.1/z);
                }
                @fragment fn fs_gap(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
                    let gap = p.x >= 15.0 && p.x < 17.0;
                    let z = select(4.0,64.0,gap || p.x >= 30.0);
                    return 100.0/99.9*(1.0-0.1/z);
                }
                @fragment fn fs_near_gap(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
                    let gap = p.x >= 15.0 && p.x < 17.0;
                    let z = select(4.0,3.7,gap || p.x >= 30.0);
                    return 100.0/99.9*(1.0-0.1/z);
                }
                @fragment fn fs_near(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
                    let z = select(64.0,2.0,p.x < 30.0);
                    return 100.0/99.9*(1.0-0.1/z);
                }
            "#.into()),
        });
        let make_depth_pipeline = |entry| renderer.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("DoF highlight depth regression"), layout: None,
            vertex: wgpu::VertexState { module: &depth_shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &depth_shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[] }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: crate::DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let depth_pipeline = make_depth_pipeline("fs");
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("DoF varying far depth"), color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(&depth_pipeline);
            pass.draw(0..3,0..1);
        }
        assert!(post.encode(&renderer.device, &renderer.queue, &mut encoder, &spot, &depth, &fog, None, size,
            SpatialSettings { depth_of_field: true, dof_aperture: 2.0, dof_max_radius: 8.0, ..Default::default() }, projection, &output_view, None));
        renderer.queue.submit([encoder.finish()]);
        let disc = crate::texture_to_rgba(&renderer, &output, size.0, size.1).unwrap();
        let pixel = |x: usize,y: usize| &disc[(y*size.0 as usize+x)*4..(y*size.0 as usize+x+1)*4];
        for (x,y) in [(37,16),(32,11),(32,21)] {
            assert!(pixel(x,y)[0] > 0, "single far highlight must fill its disc across different far depths");
        }
        assert_eq!(pixel(37,21), &[0,0,0,255], "disc must not become a square/box blur");
        for y in 0..size.1 as usize {
            for x in 0..30 {
                assert_eq!(pixel(x,y), &[40,80,120,255], "far bokeh must never bleed onto exact-focus body pixels");
            }
        }
        // A two-pixel gap spans two focused half-resolution footprints. Neither
        // that gap nor the outer silhouette may inherit the focused red body.
        // Constant colour across a near/far edge must also stay constant even
        // where covered and uncovered near results meet during reconstruction.
        for entry in ["fs_gap", "fs_near", "fs_near_gap"] {
            let near_case = entry == "fs_near";
            let input_texture = texture(renderer.format, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
            let input_pixels: Vec<u8> = (0..size.0*size.1).flat_map(|i| {
                let x = i%size.0;
                if near_case { [80,80,80,255] }
                else if x < 30 && !(15..17).contains(&x) { [255,0,0,255] }
                else { [0,80,0,255] }
            }).collect();
            renderer.queue.write_texture(wgpu::TexelCopyTextureInfo {
                texture: &input_texture, mip_level: 0, origin: Default::default(), aspect: wgpu::TextureAspect::All,
            }, &input_pixels, wgpu::TexelCopyBufferLayout {
                offset: 0, bytes_per_row: Some(size.0*4), rows_per_image: Some(size.1),
            }, wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 });
            let input = input_texture.create_view_tracked(&Default::default());
            let pipeline = make_depth_pipeline(entry);
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("DoF silhouette regression"), color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth, depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store,
                        }), stencil_ops: None,
                    }),
                    timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.draw(0..3,0..1);
            }
            assert!(post.encode(&renderer.device, &renderer.queue, &mut encoder, &input, &depth, &fog, None, size,
                SpatialSettings { depth_of_field: true, dof_aperture: 2.0, dof_max_radius: 8.0, ..Default::default() }, projection, &output_view, None));
            renderer.queue.submit([encoder.finish()]);
            let actual = crate::texture_to_rgba(&renderer, &output, size.0, size.1).unwrap();
            for (i, (pixel, original)) in actual.as_chunks::<4>().0.iter().zip(input_pixels.as_chunks::<4>().0).enumerate() {
                if near_case {
                    assert!(pixel[..3].iter().all(|&c| c.abs_diff(80) <= 1),
                        "near coverage must preserve constant colour without dark fringes at pixel {i}: {pixel:?}");
                } else if original[0] == 0 {
                    assert_eq!(pixel[0], 0, "focused actor colour leaked into gap/outline ({entry}) at pixel {i}");
                } else {
                    assert_eq!(pixel, original, "focused actor must stay exact at pixel {i}");
                }
                assert_eq!(pixel[3], 255);
            }
        }
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        assert!(!post.encode(&renderer.device, &renderer.queue, &mut encoder, &source, &depth, &fog, None, size, SpatialSettings::default(), projection, &output_view, None));
        assert!(post.frame.is_none(), "all-off must release every target and bind group");
        assert_eq!(post.bytes(), 844, "off retains uniforms, immutable aperture and fallback texels only");
    }
}
