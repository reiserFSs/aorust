//! Current-frame, bounded directional shadows. No history or simulation ownership.
use crate::effect_gpu::EffectPassEncoder as _;
use glam::{Mat4, Vec3};
use super::{GpuStatsExt, TextureViewStatsExt};

pub(crate) const CASCADES: usize = 3;
pub(crate) const GRASS_RESOLUTION: u32 = 512;
#[cfg(test)]
pub(crate) const RESOLUTION: u32 = 1024;
fn shader_source() -> std::borrow::Cow<'static, str> {
    let mut source = format!("{}\n{}", crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl")),
        crate::shader_source::load("sun_shadows.wgsl", include_str!("sun_shadows.wgsl")));
    crate::foliage::append_shader(&mut source);
    source = source.replace("    let wp = model * vec4<f32>(v.pos, 1.0);\n    var o: ShadowVertex;",
        "    var wp = model * vec4<f32>(v.pos, 1.0);\n    if (u32(mat.native.z) & 64u) != 0u { wp = vec4<f32>(foliage_vertex_at(v, vtx(v), g.foliage_wind.w, id < 0x80000000u).wpos, 1.0); }\n    var o: ShadowVertex;");
    source.into()
}

pub(crate) fn receiver_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let mut entries = receiver_entries().to_vec();
    entries.push(exposure_entry());
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sun shadow receiver"),
        entries: &entries,
    })
}

pub(crate) fn receiver_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
    [
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<ShadowUniform>() as u64) }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), count: None },
            wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
    ]
}

pub(crate) fn exposure_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false, min_binding_size: None }, count: None }
}

pub(super) fn horizon_fade(elevation: f32) -> f32 {
    // Input is normalized toward-sun Y: full strength at 4°, fading to zero
    // only at the actual horizon (sin(4°) = 0.06975647).
    let t = (elevation / 0.06975647).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ShadowUniform {
    pub matrices: [[[f32; 4]; 4]; CASCADES],
    pub splits: [f32; 4],
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub control: [u32; 4],
    pub moon_direction: [f32; 4],
    pub moon_budget: [f32; 4],
}


pub(crate) struct CascadeFrame {
    pub uniform: ShadowUniform,
}
impl CascadeFrame {
    #[allow(clippy::too_many_arguments)]
    pub fn new(eye: Vec3, forward: Vec3, up: Vec3, fovy: f32,
        aspect: f32, near: f32, useful_far: f32, toward_sun: Vec3, resolution: u32) -> Option<Self> {
        if !eye.is_finite() || !forward.is_finite() || !up.is_finite()
            || !fovy.is_finite() || !aspect.is_finite() || !near.is_finite() || !useful_far.is_finite()
            || near <= 0.0 || useful_far <= near || aspect <= 0.0 || fovy <= 0.0
            || fovy >= std::f32::consts::PI || !toward_sun.is_finite()
            || toward_sun.length_squared() < 1e-8 || resolution == 0 { return None; }
        let authored_sun = toward_sun.normalize();
        // Only the shadow raster grid is quantized; sky/direct light stay smooth.
        let angle_step = std::f32::consts::TAU / 16384.0;
        let azimuth = (authored_sun.x.atan2(authored_sun.z) / angle_step).round() * angle_step;
        let elevation = (authored_sun.y.clamp(-1.0, 1.0).asin() / angle_step).round() * angle_step;
        let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
        let (sin_elevation, cos_elevation) = elevation.sin_cos();
        let sun = Vec3::new(sin_azimuth * cos_elevation, sin_elevation, cos_azimuth * cos_elevation);
        let forward = forward.try_normalize()?;
        forward.cross(up).try_normalize()?;
        let splits = [near + (useful_far - near) / 9.0, near + (useful_far - near) / 3.0, useful_far];
        // Analytic azimuth basis has no up-axis switch as the sun crosses 0.95Y.
        // World-origin rotation is independent of camera position and orientation.
        let right = Vec3::new(cos_azimuth, 0.0, -sin_azimuth);
        let basis = Mat4::from_cols(right.extend(0.0), sun.cross(right).extend(0.0),
            sun.extend(0.0), glam::Vec4::W).transpose();
        let mut start = near;
        let matrices = splits.map(|end| {
            let center = eye + forward * ((start + end) * 0.5);
            let half_height = (fovy * 0.5).tan() * end;
            let half_width = half_height * aspect;
            // Bounding sphere is invariant under camera rotation and includes the
            // overlap used for the final 10% border blend.
            let radius = ((end - start).powi(2) * 0.25 + half_height.powi(2) + half_width.powi(2)).sqrt();
            // Two texels of guard cover both snapping and the 3x3 PCF border.
            let radius = ((radius * 16.0).ceil() / 16.0) * (1.0 + 4.0 / resolution as f32);
            let texel = 2.0 * radius / resolution as f32;
            let light_center = basis.transform_point3(center);
            let x = (light_center.x / texel).round() * texel;
            let y = (light_center.y / texel).round() * texel;
            // Snap depth too: continuously shifting the depth origin perturbs
            // caster/receiver float comparisons even when XY stays in one texel.
            let depth_center = (light_center.z / texel).round() * texel;
            // Include upstream casters and the depth-snapping half-texel guard.
            let z = useful_far + radius + texel;
            let projection = Mat4::orthographic_rh(x - radius, x + radius, y - radius, y + radius,
                -depth_center - z, -depth_center + z);
            start = end - (end - start) * 0.1;
            (projection * basis).to_cols_array_2d()
        });
        Some(Self { uniform: ShadowUniform { matrices,
            splits: [splits[0], splits[1], splits[2], near], eye: eye.extend(useful_far).to_array(),
            forward: forward.extend(horizon_fade(authored_sun.y)).to_array(), control: [0; 4],
            moon_direction: [0.0; 4], moon_budget: [0.0; 4] } })
    }
    pub(crate) fn include_caster_depth(&mut self, casters: impl Clone + Iterator<Item = (Vec3, f32)>) {
        for matrix in &mut self.uniform.matrices {
            *matrix = crate::shadow_coverage::include_caster_depth(Mat4::from_cols_array_2d(matrix), casters.clone()).to_cols_array_2d();
        }
    }
}

pub(crate) struct SunShadows {
    pub receiver: wgpu::BindGroup,
    pub grass_receiver: Option<wgpu::BindGroup>,
    pub grass_layer: Option<wgpu::TextureView>,
    pub grass_caster: Option<GrassCaster>,
    array: wgpu::TextureView,
    grass_depth: wgpu::TextureView,
    sampler: wgpu::Sampler,
    pub cascade_groups: [wgpu::BindGroup; CASCADES],
    pub layers: [wgpu::TextureView; CASCADES],
    /// Developer/offscreen cascade readback only; never a gameplay debug mode.
    _maps: wgpu::Texture,
    pub primary_depth: wgpu::TextureView,
    pub primary_ids: wgpu::TextureView,
    pub size: (u32, u32),
    pub resolution: u32,
    pub sun_replacement_active: bool,
    pub coverage_uniform: ShadowUniform,
    pub native_fog_end: f32,
    pub compiled: std::sync::Arc<Compiled>,
    /// Current-frame union-of-cascades casters, in original draw/instance order.
    pub static_commands: Vec<(usize, std::ops::Range<u32>)>,
    pub static_transforms: Vec<[[f32; 4]; 4]>,
    pub static_instances: Option<wgpu::Buffer>,
    pub static_capacity: usize,
    pub actor_ids: Vec<u32>,
    pub actor_groups: std::collections::HashMap<u32, u32>,
    pub actor_ids_buffer: wgpu::Buffer,
    pub(crate) actor_visibility: wgpu::Buffer,
    actor_capacity: usize,
    visibility_group: wgpu::BindGroup,
    uniforms: [wgpu::Buffer; CASCADES],
    exposure: wgpu::Buffer,
}
/// Size-independent shadow programs, shared by every runtime target allocation.
pub(crate) struct Compiled {
    pub pipelines: std::collections::HashMap<(bool, usize, bool), Option<crate::pipeline_creation::RenderPipeline>>,
    pub(crate) caster_layout: wgpu::BindGroupLayout,
    visibility_layout: wgpu::BindGroupLayout,
    visibility_pipeline: crate::pipeline_creation::ComputePipeline,
}
pub(crate) struct GrassCaster {
    pub pipeline: crate::pipeline_creation::RenderPipeline,
    pub layout: wgpu::BindGroupLayout,
}
impl GrassCaster {
    pub(crate) fn new(renderer: &super::Renderer) -> Self {
        let device = &renderer.device;
        let buffer = |binding, storage| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer { ty: if storage { wgpu::BufferBindingType::Storage { read_only: true } } else { wgpu::BufferBindingType::Uniform },
                has_dynamic_offset: false, min_binding_size: None }, count: None,
        };
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass shadow geometry"), entries: &[buffer(0, true), buffer(1, false), buffer(2, false),
                buffer(5, true), texture(6), wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }, texture(8), buffer(14, true)],
        });
        let mut source = crate::grass::shader_source((false, false, false, false, false), false);
        source.push_str(&crate::shader_source::load("grass_caster.wgsl", include_str!("grass_caster.wgsl")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shared grass caster geometry"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass caster"), bind_group_layouts: &[Some(&renderer.g_layout), None, None, Some(&layout)], immediate_size: 0,
        });
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some("near triangular grass blade shadows"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_grass_shadow"), compilation_options: Default::default(), buffers: &[] },
            fragment: None, primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: wgpu::DepthBiasState { constant: 1, slope_scale: 1.0, clamp: 0.0 } }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        Self { pipeline, layout }
    }
}
impl Compiled {
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, material: &wgpu::BindGroupLayout) -> Self {
        let uniform_entry = wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<ShadowUniform>() as u64) }, count: None };
        let storage_entry = wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false, min_binding_size: None }, count: None };
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sun shadow current-frame admission"), entries: &[uniform_entry, storage_entry] });
        let visibility_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("current-frame actor visibility reduction"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let visibility_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("current-frame actor visibility reduction"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("shadow_visibility.wgsl", include_str!("shadow_visibility.wgsl"))),
        });
        let visibility_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("current-frame actor visibility reduction"), bind_group_layouts: &[Some(&visibility_layout)], immediate_size: 0,
        });
        let visibility_pipeline = crate::pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("current-frame actor visibility reduction"), layout: Some(&visibility_pipeline_layout),
            module: &visibility_shader, entry_point: Some("visible_actors"), compilation_options: Default::default(), cache: None,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native current-frame shadow casters"),
            source: wgpu::ShaderSource::Wgsl(shader_source()),
        });
        let caster_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sun caster layout"),
            bind_group_layouts: &[Some(globals), Some(material), None, Some(&caster_layout)],
            immediate_size: 0,
        });
        let primary_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("primary shadow admission layout"),
            bind_group_layouts: &[Some(globals), Some(material)],
            immediate_size: 0,
        });
        let (world, actors, _) = super::build_pipelines();
        let mut pipelines = std::collections::HashMap::new();
        let mut unique = std::collections::HashMap::new();
        for (actor, slots) in [(false, &world), (true, &actors)] {
            for slot in 0..slots.len() {
                let spec = slots.spec(slot);
                for primary in [false, true] {
                    let pipeline = pipeline_key(spec, primary).map(|key| {
                        unique.entry(key).or_insert_with(|| {
                            let device = device.clone();
                            let shader = shader.clone();
                            let layout = if primary { &primary_pipeline_layout } else { &caster_pipeline_layout }.clone();
                            crate::pipeline_creation::Deferred::new(move || {
                                compile_pipeline(&device, &shader, &layout, spec, primary)
                                    .expect("eligible shadow pipeline")
                            })
                        }).clone()
                    });
                    pipelines.insert((actor, slot, primary), pipeline);
                }
            }
        }
        Self { pipelines, caster_layout, visibility_layout, visibility_pipeline }
    }
    pub fn is_ready(&self) -> bool {
        self.visibility_pipeline.is_ready()
            && self.pipelines.values().flatten().all(crate::pipeline_creation::RenderPipeline::is_ready)
    }
    pub fn wait(&self) {
        self.visibility_pipeline.get();
        for pipeline in self.pipelines.values().flatten() {
            pipeline.get();
        }
    }
}
impl SunShadows {
    pub fn new(renderer: &super::Renderer, size: (u32, u32), resolution: u32) -> Self {
        let device = &renderer.device;
        let compiled = renderer.shadow_pipelines.clone();
        let caster_layout = &compiled.caster_layout;
        let visibility_layout = &compiled.visibility_layout;
        let receiver_layout = receiver_layout(device);
        let texture = |label, width, height, layers, format| device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width, height, depth_or_array_layers: layers },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
        let primary_depth = device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("transient sun caster admission depth"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT, view_formats: &[],
        }).create_view_tracked(&Default::default());
        let primary_ids = texture("sun caster admission identities", size.0, size.1, 1, wgpu::TextureFormat::R32Uint).create_view_tracked(&Default::default());
        let grass_enabled = renderer.enhanced_settings.ground_grass && renderer.enhanced_settings.grass_sun_shadows;
        // Grass keeps the nearest native projection, but never its allocation size.
        let maps = texture("current-frame sun cascades", resolution, resolution, CASCADES as u32, wgpu::TextureFormat::Depth32Float);
        let array = maps.create_view_tracked(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let layers = std::array::from_fn(|i| maps.create_view_tracked(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: i as u32,
            array_layer_count: Some(1), ..Default::default() }));
        let grass_resolution = if grass_enabled { GRASS_RESOLUTION } else { 1 };
        let grass_depth = texture("near grass sun depth", grass_resolution, grass_resolution, 1, wgpu::TextureFormat::Depth32Float)
            .create_view_tracked(&Default::default());
        let grass_layer = grass_enabled.then(|| grass_depth.clone());
        let grass_caster = grass_enabled.then(|| GrassCaster::new(renderer));
        let uniforms: [wgpu::Buffer; CASCADES] = std::array::from_fn(|_| device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
            label: Some("current cascade matrices"), contents: bytemuck::bytes_of(&ShadowUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST }));
        let sampler = device.create_sampler_tracked(&wgpu::SamplerDescriptor { label: Some("sun shadow PCF"),
            compare: Some(wgpu::CompareFunction::LessEqual), mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest, ..Default::default() });
        let exposure = renderer.moon_exposure.buffer().cloned().unwrap_or_else(|| device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some("disabled moon exposure header"), size: 32,
            usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false,
        }));
        let receiver_entries = [wgpu::BindGroupEntry { binding: 0, resource: uniforms[0].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&array) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            wgpu::BindGroupEntry { binding: 7, resource: exposure.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&grass_depth) }];
        let receiver = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("sun shadow receiver"),
            layout: &receiver_layout, entries: &receiver_entries });
        let actor_capacity = 64;
        let (actor_ids_buffer, actor_visibility) = admission_buffers(device, actor_capacity);
        let cascade_groups = caster_groups(device, caster_layout, &uniforms, &actor_visibility);
        let visibility_group = visibility_group(device, visibility_layout, &primary_ids, &actor_visibility);
        Self { receiver, grass_receiver: None, grass_layer, grass_caster, array, grass_depth, sampler, cascade_groups, layers, _maps: maps,
            primary_depth, primary_ids, size, resolution, uniforms, compiled, exposure,
            sun_replacement_active: false, coverage_uniform: ShadowUniform::zeroed(),
            native_fog_end: f32::INFINITY,
            static_commands: Vec::new(), static_transforms: Vec::new(),
            static_instances: None, static_capacity: 0, actor_ids: Vec::new(), actor_groups: Default::default(),
            actor_ids_buffer, actor_visibility, actor_capacity, visibility_group }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn grass_bind(&self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, cells: &wgpu::Buffer, params: &wgpu::Buffer, visible: &wgpu::Buffer, noise: &wgpu::TextureView, noise_sampler: &wgpu::Sampler, mask: &wgpu::TextureView, trails: &wgpu::Buffer, roots: &wgpu::Buffer, points: Option<&crate::point_shadows::PointShadows>) -> wgpu::BindGroup {
        let mut entries = vec![
                wgpu::BindGroupEntry { binding: 0, resource: cells.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.uniforms[0].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.array) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: visible.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(noise) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(noise_sampler) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(mask) },
                wgpu::BindGroupEntry { binding: 10, resource: trails.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 14, resource: roots.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 16, resource: wgpu::BindingResource::TextureView(&self.grass_depth) },
        ];
        if let Some(points) = points {
            entries.extend([
                wgpu::BindGroupEntry { binding: 9, resource: self.actor_visibility.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 17, resource: wgpu::BindingResource::TextureView(&points.array) },
                wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::Sampler(&points.sampler) },
                wgpu::BindGroupEntry { binding: 12, resource: points.metadata.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 13, resource: wgpu::BindingResource::TextureView(&points.actor_array) },
            ]);
        }
        device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("grass cells and admitted shadow receivers"), layout, entries: &entries,
        })
    }
    pub fn grass_caster_bind(&self, device: &wgpu::Device, grass: &crate::grass::Grass) -> Option<wgpu::BindGroup> {
        let caster = self.grass_caster.as_ref()?;
        let visible = grass.sun_visible_buffer.as_ref()?;
        Some(device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("current grass blade caster"), layout: &caster.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: grass.cells_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: grass.params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.uniforms[0].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: visible.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&grass.noise_view) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&grass.noise_sampler) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&grass.mask_view) },
                wgpu::BindGroupEntry { binding: 14, resource: grass.root_cache.buffer.as_entire_binding() },
            ],
        }))
    }
    pub fn prepare_actor_admission(&mut self, device: &wgpu::Device, count: usize) {
        if count > self.actor_capacity {
            self.actor_capacity = count.next_power_of_two();
            (self.actor_ids_buffer, self.actor_visibility) = admission_buffers(device, self.actor_capacity);
            self.grass_receiver = None;
            self.cascade_groups = caster_groups(device, &self.compiled.caster_layout, &self.uniforms, &self.actor_visibility);
            self.visibility_group = visibility_group(device, &self.compiled.visibility_layout, &self.primary_ids, &self.actor_visibility);
        }
        self.actor_ids.clear();
        self.actor_groups.clear();
    }

    pub fn encode_actor_visibility(&self, encoder: &mut wgpu::CommandEncoder, timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>) {
        if self.actor_ids.is_empty() { return; }
        encoder.clear_buffer(&self.actor_visibility, 0, None);
        let mut pass = encoder.begin_effect_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("current-frame actor visibility reduction"), timestamp_writes,
        });
        pass.set_pipeline(&self.compiled.visibility_pipeline);
        pass.set_bind_group(0, &self.visibility_group, &[]);
        pass.dispatch_workgroups(self.size.0.div_ceil(8), self.size.1.div_ceil(8), 1);
    }
    pub fn update(&mut self, queue: &wgpu::Queue, frame: &CascadeFrame) {
        self.coverage_uniform = frame.uniform;
        for (i, buffer) in self.uniforms.iter().enumerate() {
            let mut uniform = frame.uniform;
            uniform.control[0] = i as u32;
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
        }
    }
    pub fn resident_bytes(&self) -> u64 {
        u64::from(self.resolution).pow(2) * 4 * CASCADES as u64
            + u64::from(if self.grass_layer.is_some() { GRASS_RESOLUTION } else { 1 }).pow(2) * 4
            + u64::from(self.size.0) * u64::from(self.size.1) * 4
            + (std::mem::size_of::<ShadowUniform>() * CASCADES) as u64
            + (self.static_capacity * 64) as u64
            + ((self.actor_capacity * 2 + 1) * 4) as u64
    }
    pub(crate) fn point_receiver(&self, device: &wgpu::Device, points: &crate::point_shadows::PointShadows) -> wgpu::BindGroup {
        device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("directional and local shadow receivers"),
            layout: &crate::point_shadows::receiver_layout(device),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms[0].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.array) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: self.actor_visibility.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&points.array) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&points.sampler) },
                wgpu::BindGroupEntry { binding: 6, resource: points.metadata.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: self.exposure.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&self.grass_depth) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&points.actor_array) },
            ],
        })
    }
}

fn admission_buffers(device: &wgpu::Device, capacity: usize) -> (wgpu::Buffer, wgpu::Buffer) {
    (device.create_buffer_tracked(&wgpu::BufferDescriptor {
        label: Some("current-frame actor shadow IDs"), size: (capacity * 4) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    }), device.create_buffer_tracked(&wgpu::BufferDescriptor {
        label: Some("current-frame actor shadow visibility"), size: ((capacity + 1) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
    }))
}

fn caster_groups(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniforms: &[wgpu::Buffer; CASCADES], visibility: &wgpu::Buffer) -> [wgpu::BindGroup; CASCADES] {
    std::array::from_fn(|index| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
        label: Some("current-frame shadow caster admission"), layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms[index].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: visibility.as_entire_binding() }],
    }))
}

fn visibility_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, ids: &wgpu::TextureView, visibility: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
        label: Some("current-frame actor visibility reduction"), layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(ids) },
            wgpu::BindGroupEntry { binding: 1, resource: visibility.as_entire_binding() }],
    })
}
use bytemuck::Zeroable;


// These are exactly the descriptor fields that vary between eligible slots.
fn pipeline_key(spec: super::PipelineSpec, primary: bool) -> Option<(bool, bool, bool, bool, wgpu::CompareFunction, bool)> {
    use super::Blend;
    if primary {
        let base_write = spec.sprite || matches!(spec.blend, Blend::Opaque | Blend::AlphaTest);
        let writes_depth = match spec.effect {
            1 | 4..=6 | 10 | 14 => false,
            2 | 8 | 9 | 11..=13 | 15..=17 => true,
            _ => base_write ^ spec.flip_depth,
        };
        if spec.sky || spec.env || !writes_depth { return None; }
    } else if spec.sky || spec.env || spec.fade || spec.sprite || spec.flip_depth
        || !matches!(spec.blend, Blend::Opaque | Blend::AlphaTest)
        || !matches!(spec.effect, 0 | 2 | 3 | 7..=9 | 11) { return None; }
    let compare = if !primary || spec.effect == 2 { wgpu::CompareFunction::LessEqual } else {
        spec.compare.unwrap_or(if matches!(spec.effect, 8 | 9 | 11) { wgpu::CompareFunction::Less } else { wgpu::CompareFunction::LessEqual })
    };
    Some((primary, spec.actor, matches!(spec.blend, Blend::AlphaTest), primary && !spec.two_sided, compare, primary && spec.effect == 7))
}

/// Compile only the native opaque/cutout states whose geometry is unchanged.
/// Projection/effect and blended depth writers are not fair reusable casters.
pub(crate) fn compile_pipeline(
    device: &wgpu::Device, shader: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout,
    spec: super::PipelineSpec, primary: bool,
) -> Option<wgpu::RenderPipeline> {
    use super::Blend;
    pipeline_key(spec, primary)?;
    let test = matches!(spec.blend, Blend::AlphaTest);
    let vertex = match (primary, spec.actor) {
        (true, false) => "vs_shadow_primary_static",
        (true, true) => "vs_shadow_primary_actor",
        (false, false) => "vs_shadow_static",
        (false, true) => "vs_shadow_actor",
    };
    let fragment = match (primary, spec.actor, test) {
        (true, _, false) => "fs_shadow_primary_opaque",
        (true, _, true) => "fs_shadow_primary_test",
        (false, false, false) => "fs_shadow_opaque",
        (false, false, true) => "fs_shadow_test",
        (false, true, false) => "fs_shadow_dynamic_opaque",
        (false, true, true) => "fs_shadow_dynamic_test",
    };
    let vertices = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
    let instances = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
    let actors = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4,
        8 => Float32x4, 9 => Float32x4, 10 => Float32x4, 11 => Float32x4, 13 => Uint32];
    let actor_ids = wgpu::vertex_attr_array![12 => Uint32];
    let buffers = [
        wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertices },
        wgpu::VertexBufferLayout { array_stride: if spec.actor { std::mem::size_of::<super::actors::ActorInstance>() as u64 } else { 64 },
            step_mode: wgpu::VertexStepMode::Instance, attributes: if spec.actor { &actors } else { &instances } },
        wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Instance, attributes: &actor_ids },
    ];
    let targets = [Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Uint,
        blend: None, write_mask: wgpu::ColorWrites::ALL })];
    Some(crate::pipeline_creation::render(device, &wgpu::RenderPipelineDescriptor {
        label: Some(fragment), layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some(vertex), compilation_options: Default::default(),
            buffers: if spec.actor { &buffers } else { &buffers[..2] } },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(fragment),
            compilation_options: Default::default(), targets: if primary { &targets } else { &[] } }),
        // Thin statel walls block sunlight from either side. Only camera admission
        // retains the native material culling; light-view backface culling opens
        // building bases and detaches their roof shadows from the sidewalk.
        primitive: wgpu::PrimitiveState { cull_mode: (primary && !spec.two_sided).then_some(wgpu::Face::Back), ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(if !primary || spec.effect == 2 { wgpu::CompareFunction::LessEqual } else {
                spec.compare.unwrap_or(if matches!(spec.effect, 8 | 9 | 11) { wgpu::CompareFunction::Less } else { wgpu::CompareFunction::LessEqual })
            }),
            stencil: Default::default(),
            // Preserve native mode7 primary bias. Receiver-plane PCF corrects
            // cascade filter slope without separating caster contact points.
            bias: if primary {
                if spec.effect == 7 { wgpu::DepthBiasState { constant: -1, slope_scale: 0.0, clamp: 0.0 } } else { Default::default() }
            } else { wgpu::DepthBiasState { constant: 1, slope_scale: 0.0, clamp: 0.0 } },
        }),
        multisample: Default::default(), multiview_mask: None, cache: None,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_bundle_covers_every_world_and_actor_caster_slot() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("shadow WGSL requires offscreen GPU");
        let compiled = &renderer.shadow_pipelines;
        let (world, actors, _) = crate::build_pipelines();
        assert_eq!(compiled.pipelines.len(), 2 * (world.len() + actors.len()));
        for (actor, slots) in [(false, &world), (true, &actors)] {
            for slot in 0..slots.len() {
                for primary in [false, true] {
                    assert_eq!(compiled.pipelines[&(actor, slot, primary)].is_some(),
                        pipeline_key(slots.spec(slot), primary).is_some(),
                        "caster slot {actor}/{slot}/{primary} was not prepared at startup");
                }
            }
        }
        compiled.wait();
        assert!(compiled.is_ready(), "all caster and visibility pipelines must finish warming");
        assert!(renderer.sun_shadows.is_none(), "warming must not allocate cascade targets");
    }
    fn frame(eye: Vec3, sun: Vec3) -> CascadeFrame {
        CascadeFrame::new(eye, -Vec3::Z, Vec3::Y, 1.0, 1.5, 0.1, 90.0, sun, RESOLUTION).unwrap()
    }
    #[test]
    fn current_frame_bounded_snapped_and_no_guessed_moon() {
        let a = frame(Vec3::ZERO, Vec3::Y);
        let b = frame(Vec3::new(1e-6, 0.0, 0.0), Vec3::Y);
        for i in 0..CASCADES {
            assert_eq!(a.uniform.matrices[i][3][0], b.uniform.matrices[i][3][0]);
            assert_eq!(a.uniform.matrices[i][3][1], b.uniform.matrices[i][3][1]);
        }
        assert_eq!(a.uniform.splits[2], 90.0);
        assert_ne!(a.uniform.matrices, frame(Vec3::ZERO, Vec3::X).uniform.matrices);
        assert!(CascadeFrame::new(Vec3::ZERO, -Vec3::Z, Vec3::Y,
            1.0, 1.5, 0.1, 90.0, Vec3::ZERO, RESOLUTION).is_none());
    }
    #[test]
    fn subtexel_depth_and_sun_motion_do_not_rerasterize_cascades() {
        let sun = Vec3::new(0.0, 0.5, 0.75_f32.sqrt());
        let a = frame(Vec3::ZERO, sun);
        for tick in 0..90 {
            let eye = Vec3::splat(tick as f32 * 1e-8);
            let moving_sun = Mat4::from_rotation_y(tick as f32 * 1e-6).transform_vector3(sun);
            assert_eq!(a.uniform.matrices, frame(eye, moving_sun).uniform.matrices);
        }
        // The former look-at up-vector switch rotated the grid abruptly at .95Y.
        let below = frame(Vec3::ZERO, Vec3::new(0.0, 0.95 - 1e-6, (1.0_f32 - 0.95_f32.powi(2)).sqrt()));
        let above = frame(Vec3::ZERO, Vec3::new(0.0, 0.95 + 1e-6, (1.0_f32 - 0.95_f32.powi(2)).sqrt()));
        for (a, b) in below.uniform.matrices.iter().flatten().flatten()
            .zip(above.uniform.matrices.iter().flatten().flatten()) {
            assert!((a - b).abs() < 0.001, "sun basis flipped at the former up-axis threshold");
        }
    }
    #[test]
    fn horizon_fades_without_night_or_extra_energy() {
        assert_eq!(horizon_fade(-1.0), 0.0);
        assert_eq!(horizon_fade(0.0), 0.0);
        let near_horizon = horizon_fade(0.5_f32.to_radians().sin());
        assert!(near_horizon > 0.0 && near_horizon < 0.1);
        assert!(horizon_fade(3.0_f32.to_radians().sin()) > 0.8);
        assert_eq!(horizon_fade(4.0_f32.to_radians().sin()), 1.0);
        assert_eq!(horizon_fade(0.5), 1.0);
        for elevation in -100..=100 {
            assert!((0.0..=1.0).contains(&horizon_fade(elevation as f32 / 100.0)));
        }
    }
}
