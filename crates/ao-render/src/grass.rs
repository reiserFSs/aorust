//! Opt-in, deterministic native-material grass. Blade expansion is entirely GPU-side (grass.wgsl).
use super::{GroundRun, Vec3};
use crate::effect_gpu::EffectPassEncoder as _;
use crate::gpu_stats::GpuStatsExt;
#[path = "grass_noise.rs"]
mod noise;
#[path = "grass_mask.rs"]
mod mask;
pub(super) use mask::{build as green_masks, GreenMasks};

pub(super) const MAX_CELLS: usize = 16384;
/// Blades a cell can hold at the top density (2.0); the density knob scales how many of them are drawn.
const MAX_BLADES: f32 = 1536.0;
const MAX_RADIUS: f32 = 180.0;
/// Successive distance bands halve the number of instanced blades.
pub(super) const BANDS: usize = 8;
const MAX_BENDERS: usize = 16;
const MAX_EXPOSURE_CELLS: usize = 32768;
const EXPOSURE_CELLS_PER_FRAME: usize = 4;
/// Narrowest drawn blade width in pixels; thinner blades are widened and tinted towards the ground.
const MIN_BLADE_PIXELS: f32 = 0.8;

const TRAIL_SIDE: usize = 32;
const TRAIL_CELLS: usize = TRAIL_SIDE * TRAIL_SIDE;
const TRAIL_CELL_SIZE: f32 = 1.25;
const TRAIL_RECOVERY_SECONDS: f32 = 6.0;

#[derive(Clone, Copy, Default)]
struct TrailSample {
    grid: [i32; 2],
    /// Horizontal lean, foot height, and remaining compression.
    bend: [f32; 4],
}
struct TrailSource {
    previous: GrassBender,
    cells: Vec<TrailSample>,
}
#[derive(Default)]
struct TrailGrid {
    sources: Vec<TrailSource>,
    origin: [i32; 2],
}
impl TrailGrid {
    fn index(grid: [i32; 2]) -> usize {
        grid[1].rem_euclid(TRAIL_SIDE as i32) as usize * TRAIL_SIDE
            + grid[0].rem_euclid(TRAIL_SIDE as i32) as usize
    }
    fn recover(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 { return; }
        for source in &mut self.sources {
            for cell in &mut source.cells {
                cell.bend[3] = (cell.bend[3] - dt / TRAIL_RECOVERY_SECONDS).max(0.0);
            }
        }
    }
    fn update(&mut self, admitted: &[GrassBender], camera: Vec3) {
        self.origin = [camera.x, camera.z].map(|v| (v / TRAIL_CELL_SIZE).floor() as i32 - TRAIL_SIDE as i32 / 2);
        let admitted = &admitted[..admitted.len().min(MAX_BENDERS)];
        // Keep ownership until recovery completes, but never after admission loss.
        self.sources.retain(|source| admitted.iter().any(|b| b.actor == source.previous.actor));
        for bender in admitted {
            let Some(source) = self.sources.iter_mut().find(|source| source.previous.actor == bender.actor) else {
                self.sources.push(TrailSource { previous: *bender, cells: vec![TrailSample::default(); TRAIL_CELLS] });
                continue;
            };
            for cell in &mut source.cells {
                if (0..2).any(|axis| cell.grid[axis] < self.origin[axis] || cell.grid[axis] >= self.origin[axis] + TRAIL_SIDE as i32) {
                    cell.bend[3] = 0.0;
                }
            }
            let start = Vec3::from_array(source.previous.position);
            let end = Vec3::from_array(bender.position);
            source.previous = *bender;
            let movement = end - start;
            let distance = movement.x.hypot(movement.z);
            // Teleports never draw a line through unobserved ground.
            if !(0.001..=6.0).contains(&distance) || movement.y.abs() > 1.5 { continue; }
            let direction = [movement.x / distance, movement.z / distance];
            let steps = (distance / 0.3).ceil() as usize;
            for step in 0..=steps {
                let position = start.lerp(end, step as f32 / steps as f32);
                let center = [position.x, position.z].map(|v| (v / TRAIL_CELL_SIZE).floor() as i32);
                for z in center[1] - 1..=center[1] + 1 {
                    for x in center[0] - 1..=center[0] + 1 {
                        let grid = [x, z];
                        if (0..2).any(|axis| grid[axis] < self.origin[axis] || grid[axis] >= self.origin[axis] + TRAIL_SIDE as i32) { continue; }
                        let dx = (x as f32 + 0.5) * TRAIL_CELL_SIZE - position.x;
                        let dz = (z as f32 + 0.5) * TRAIL_CELL_SIZE - position.z;
                        let strength = (1.0 - dx.hypot(dz) / 1.15).max(0.0);
                        let cell = &mut source.cells[Self::index(grid)];
                        if cell.grid != grid || strength >= cell.bend[3] {
                            *cell = TrailSample { grid, bend: [direction[0], direction[1], position.y, strength] };
                        }
                    }
                }
            }
        }
    }
    fn write_owner(&self, actor: Option<u32>, output: &mut [[f32; 4]]) {
        output.fill([0.0; 4]);
        if let Some(source) = self.sources.iter().find(|source| source.previous.actor == actor) {
            for cell in &source.cells {
                if cell.bend[3] > 0.0 { output[Self::index(cell.grid)] = cell.bend; }
            }
        }
    }
    #[cfg(test)]
    fn write(&self, output: &mut [[f32; 4]; TRAIL_CELLS]) {
        for source in &self.sources {
            for cell in &source.cells {
                let index = Self::index(cell.grid);
                if cell.bend[3] > output[index][3] { output[index] = cell.bend; }
            }
        }
    }
}

/// Effect-local radians, never derived from the native f32 animation clock.
/// Integer shader harmonics make every component continuous across TAU.
pub(super) fn advance_wind_phase(phase: f64, dt: f32) -> f64 {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
    (phase + f64::from(dt) * 0.05).rem_euclid(std::f64::consts::TAU)
}

pub(super) fn wind_target(wind: [f32; 3]) -> [f32; 3] {
    let speed = wind[0].hypot(wind[2]);
    let direction = if speed > 0.05 { [wind[0] / speed, wind[2] / speed] } else { [0.8, 0.6] };
    [direction[0], direction[1], (0.35 + 0.1 * speed).min(2.5)]
}

/// Smooth the effect's bend vector, not native weather. Do not renormalize the
/// filtered direction: reversals should pass gently through stillness.
pub(super) fn smooth_wind(filtered: [f32; 3], wind: [f32; 3], dt: f32) -> [f32; 3] {
    let target = wind_target(wind);
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
    let share = -(-dt * 0.5).exp_m1();
    std::array::from_fn(|i| filtered[i] + (target[i] - filtered[i]) * share)
}

/// Opt-in native collision capability; unknown tiles fail closed.
#[derive(Clone)]
pub struct GrassExposure {
    pub generation: u64,
    pub query: std::rc::Rc<dyn Fn([f32; 3]) -> bool>,
    pub line_of_sight: std::rc::Rc<dyn Fn([f32; 3], [f32; 3]) -> bool>,
}

/// One current-frame interaction source. `None` identifies only the own avatar;
/// remote actor IDs must also survive the renderer's primary body admission.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GrassBender {
    pub actor: Option<u32>,
    pub position: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Cell {
    origin: [f32; 4],
    heights: [f32; 4],
    identity: [u32; 4],
    normals: [[f32; 4]; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    wind_time: [f32; 4],
    limits: [f32; 4],
    wind: [f32; 4],
    features: [f32; 4],
    lighting: [f32; 4],
    /// Blades instanced per cell in each distance band.
    bands: [[f32; 4]; 2],
    benders: [[f32; 4]; MAX_BENDERS],
    trail_origin: [f32; 4],
}
struct Batch {
    patch: usize,
    submesh: usize,
    cells: std::ops::Range<u32>,
}
/// One draw: the visible cells of a ground batch in one distance band.
struct Draw {
    batch: usize,
    band: usize,
    slots: std::ops::Range<u32>,
}
struct Candidate {
    cell: Cell,
    patch: usize,
    submesh: usize,
    grid: [i32; 2],
    distance: f32,
}
/// Grass program and its layout; no ground buffers or textures.
#[derive(Clone)]
pub(super) struct Compiled {
    pipeline: crate::pipeline_creation::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    roots: crate::grass_blades::Compiled,
    samples: u32,
    channels: (bool, bool, bool, bool, bool, bool),
    fog_red: bool,
    shadows: bool,
    admitted: bool,
}
pub(super) struct Grass {
    pipeline: crate::pipeline_creation::RenderPipeline,
    bind: Option<wgpu::BindGroup>,
    layout: wgpu::BindGroupLayout,
    shadows: bool,
    admitted: bool,
    roots: crate::grass_blades::Compiled,
    pub(super) root_cache: crate::grass_blades::Cache,
    pub(super) cells_buffer: wgpu::Buffer,
    pub(super) params_buffer: wgpu::Buffer,
    trails_buffer: wgpu::Buffer,
    trail_samples: Vec<[f32; 4]>,
    params: Params,
    pub(super) visible_buffer: wgpu::Buffer,
    pub(super) sun_visible_buffer: Option<wgpu::Buffer>,
    sun_visible: [Vec<u32>; BANDS],
    pub(super) noise_view: wgpu::TextureView,
    pub(super) noise_sampler: wgpu::Sampler,
    pub(super) mask_view: wgpu::TextureView,
    green_bytes: usize,
    cells: Vec<Cell>,
    batches: Vec<Batch>,
    /// This frame's frustum-culled cells (indices into `cells`) per distance band, and the ranges of them each draw covers.
    visible: [Vec<u32>; BANDS],
    draws: Vec<Draw>,
    stride: u32,
    band_strides: [u32; BANDS],
    #[cfg(test)]
    forced_band: Option<usize>,
    surface_heights: Vec<f32>,
    surface_parities: Vec<u32>,
    samples: u32,
    channels: (bool, bool, bool, bool, bool, bool),
    fog_red: bool,
    wind_phase: f64,
    filtered_wind: [f32; 3],
    benders: [[f32; 3]; MAX_BENDERS],
    owners: [Option<u32>; MAX_BENDERS],
    owner_slots: [u32; MAX_BENDERS],
    bender_count: usize,
    inputs: Vec<GrassBender>,
    trails: TrailGrid,
    prepare_cpu_ns: u64,
    exposure: Option<GrassExposure>,
    exposure_cache: std::collections::HashMap<[u32; 4], Option<[[f32; 4]; 4]>>,
    exposure_order: std::collections::VecDeque<[u32; 4]>,
    candidates: Vec<Candidate>,
    order: Vec<usize>,
    last_selection: Option<([i32; 2], i32, i32)>,
    last_radius: u32,
    last_path: super::GroundPath,
    pending: bool,
}
impl Compiled {
    #[allow(clippy::too_many_arguments)]
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, material: &wgpu::BindGroupLayout, palette: &wgpu::BindGroupLayout, samples: u32, channels: (bool, bool, bool, bool, bool, bool), shadows: bool, admitted: bool, format: wgpu::TextureFormat, sun_format: wgpu::TextureFormat) -> Self {
        let shadows = shadows || admitted;
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 14, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 10, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ];
        if shadows {
            // Root exposure comes from Cell.normals[0].w, not the world's lattice.
            entries.extend(crate::sun_shadows::receiver_entries().into_iter().map(|mut entry| {
                entry.binding = if entry.binding == 8 { 16 } else { entry.binding + 2 };
                entry.visibility = if entry.binding == 2 { wgpu::ShaderStages::VERTEX_FRAGMENT } else { wgpu::ShaderStages::VERTEX };
                entry
            }));
        }
        if admitted {
            entries.extend(crate::point_shadows::grass_receiver_entries());
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass cells, benders and shared sun receiver"), entries: &entries,
        });
        let fog_red = sun_format == wgpu::TextureFormat::R16Float;
        let mut source = shader_source_with_lights(channels, shadows, admitted);
        if fog_red {
            source = source.replace("output.sun = vec4<f32>(sun_luma * grass_direct_visibility * (1.0 - fog), i.normal.w * (1.0 - fog), fog, 1.0);", "output.sun = vec4<f32>(fog, 0.0, 0.0, 1.0);");
            source = source.replace("let sun_luma = dot(sun_share, vec3<f32>(0.2126, 0.7152, 0.0722));", "");
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native material grass"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let roots = crate::grass_blades::Compiled::new(device, globals, &entries, &shader);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("grass"), bind_group_layouts: &[Some(globals), Some(material), Some(palette), Some(&layout)], immediate_size: 0 });
        let compact_indirect = channels.5 && !channels.1 && !channels.2 && !channels.3 && !channels.4;
        let mut targets = [
            Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            (channels.0 || channels.5).then_some(wgpu::ColorTargetState { format: crate::enhanced::ambient_format(&crate::EnhancedSettings { ambient_occlusion: channels.0, indirect_light: channels.5, ..Default::default() }), blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.1.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            // HDR sun mask: blades write the same surviving sun share as the ground.
            channels.2.then_some(wgpu::ColorTargetState { format: sun_format, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.3.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend: None, write_mask: wgpu::ColorWrites::RED }),
            channels.4.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.4.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.5.then_some(wgpu::ColorTargetState { format: crate::enhanced::ALBEDO, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        if compact_indirect { targets.swap(2, 7); }
        let target_count = if compact_indirect { 3 } else if channels.5 { 8 } else if channels.4 { 7 } else if channels.3 { 5 } else if channels.2 { 4 } else if channels.0 || channels.1 { 3 } else { 1 };
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some("ground grass"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_grass"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_grass"), compilation_options: Default::default(), targets: &targets[..target_count] }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: super::DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: samples, alpha_to_coverage_enabled: samples > 1, ..Default::default() }, multiview_mask: None, cache: None,
        });
        Self { pipeline, layout, roots, samples, channels, shadows, admitted, fog_red }
    }
    pub fn is_ready(&self) -> bool {
        self.pipeline.is_ready() && self.roots.is_ready()
    }
    pub fn wait(&self) {
        self.pipeline.get();
        self.roots.wait();
    }
    pub fn warm(&self) {
        self.pipeline.warm();
        self.roots.warm();
    }
}
impl Grass {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, green: &GreenMasks, compiled: &Compiled) -> Self {
        let Compiled { pipeline, layout, roots, samples, channels, shadows, admitted, fog_red } = compiled.clone();
        let cells_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass cell cache"), size: (MAX_CELLS * std::mem::size_of::<Cell>()) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let params_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass wind and benders"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let visible_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass visible cells"), size: (BANDS * MAX_CELLS * 4) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let trails_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass owner trail layers"), size: (MAX_BENDERS * TRAIL_CELLS * 16) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let (noise_view, noise_sampler) = noise::upload(device, queue);
        let mask_view = mask::upload(device, queue, green);
        let bind = None;
        let filtered_wind = wind_target([0.0; 3]);
        let mut grass = Self {
            #[cfg(test)]
            forced_band: None,
            trails: TrailGrid::default(),
            trails_buffer, trail_samples: vec![[0.0; 4]; MAX_BENDERS * TRAIL_CELLS], owners: [None; MAX_BENDERS], owner_slots: [u32::MAX; MAX_BENDERS],
            params: bytemuck::Zeroable::zeroed(),
            sun_visible_buffer: None,
            sun_visible: std::array::from_fn(|_| Vec::new()),
            admitted,
            roots,
            root_cache: crate::grass_blades::Cache::new(device),
            fog_red,
            pipeline, bind, layout, shadows, cells_buffer, params_buffer, visible_buffer, noise_view, noise_sampler, mask_view, green_bytes: green.layers.len(), cells: Vec::with_capacity(MAX_CELLS), batches: Vec::with_capacity(MAX_CELLS), visible: std::array::from_fn(|_| Vec::with_capacity(MAX_CELLS)), draws: Vec::new(), stride: 0, band_strides: [0; BANDS], surface_heights: Vec::new(), surface_parities: Vec::new(), samples, channels, wind_phase: 0.0, filtered_wind, benders: [[0.0; 3]; MAX_BENDERS], bender_count: 0, inputs: Vec::with_capacity(MAX_BENDERS), prepare_cpu_ns: 0, exposure: None, exposure_cache: std::collections::HashMap::with_capacity(MAX_EXPOSURE_CELLS * 2), exposure_order: std::collections::VecDeque::with_capacity(MAX_EXPOSURE_CELLS), candidates: Vec::with_capacity(MAX_CELLS), order: Vec::with_capacity(MAX_CELLS), last_selection: None, last_radius: 0, last_path: super::GroundPath::Simple, pending: false };
        grass.refresh_plain_bind(device);
        grass
    }
    /// A render variant owns programs, not terrain/exposure or admitted motion.
    /// Retain those resources when shadow/AO/MSAA switches change the layout.
    fn reconfigure(&mut self, device: &wgpu::Device, compiled: &Compiled) {
        let Compiled { pipeline, layout, roots, samples, channels, shadows, admitted, fog_red } = compiled.clone();
        self.pipeline = pipeline;
        self.layout = layout;
        self.roots = roots;
        self.root_cache.invalidate();
        self.samples = samples;
        self.channels = channels;
        self.shadows = shadows;
        self.fog_red = fog_red;
        self.admitted = admitted;
        self.refresh_plain_bind(device);
    }
    fn refresh_plain_bind(&mut self, device: &wgpu::Device) {
        self.bind = (!self.shadows).then(|| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("grass"), layout: &self.layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: self.cells_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: self.params_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: self.visible_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&self.noise_view) },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&self.noise_sampler) },
            wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&self.mask_view) },
            wgpu::BindGroupEntry { binding: 10, resource: self.trails_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 14, resource: self.root_cache.buffer.as_entire_binding() },
        ] }));
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, queue: &wgpu::Queue, ground: &GroundRun, liquids: &[super::native_liquid::LiquidRun], camera: Vec3, settings: super::EnhancedSettings, path: super::GroundPath, view: Option<(super::Mat4, f32)>) {
        if !settings.grass_sun_shadows {
            self.sun_visible_buffer = None;
            self.sun_visible.iter_mut().for_each(Vec::clear);
        }
        let size = ground.detail.patches().first().map_or(1.0, |patch| patch.cell_size);
        let radius = settings.grass_distance.clamp(0.0, MAX_RADIUS)
            .min(((MAX_CELLS as f32).sqrt() * 0.5 - 3.5).max(0.0) * size);
        if radius <= 0.0 { self.cells.clear(); self.batches.clear(); self.draws.clear(); return; }
        let selection = ground.detail.selection_key();
        let rebuild = selection != self.last_selection || radius.to_bits() != self.last_radius || path != self.last_path;
        if rebuild {
            self.last_selection = selection;
            self.last_radius = radius.to_bits();
            self.last_path = path;
            self.candidates.clear();
            // Quantized native camera state matches the terrain LOD update. Pad
            // the candidate disc so movement inside a cell only changes GPU fade.
            let center = selection.map_or([camera.x, camera.z], |(cell, _, _)| [cell[0] as f32 * size, -(cell[1] as f32) * size]);
            let padded = radius + 2.5 * size;
            for (patch_index, patch) in ground.detail.patches().iter().enumerate() {
                if !(patch.height_scale > 0.0 && patch.cell_size > 0.0) { continue; }
                let x0 = patch.origin[0] as f32 * patch.cell_size;
                let z0 = -(patch.origin[1] as f32) * patch.cell_size;
                let side = patch.cells as f32 * patch.cell_size;
                if center[0] < x0 - padded || center[0] > x0 + side + padded || center[1] < z0 - side - padded || center[1] > z0 + padded { continue; }
                let Some(materials) = ground.indices.get(patch_index) else { continue; };
                project_surface(patch, materials, &mut self.surface_heights, &mut self.surface_parities);
                // One pass over cells, not one full scan per material.
                for (index, cell) in patch.fine_cells.iter().enumerate() {
                    let Some(layer) = ground.green.green_layer(patch_index, cell.submesh) else { continue; };
                    let (x, z) = (index % patch.cells, index / patch.cells);
                    let world_x = x0 + x as f32 * patch.cell_size;
                    let world_z = z0 - z as f32 * patch.cell_size;
                    let dx = world_x + patch.cell_size * 0.5 - center[0];
                    let dz = world_z - patch.cell_size * 0.5 - center[1];
                    if dx * dx + dz * dz > padded * padded { continue; }
                    let offset = z * (patch.cells + 1) + x;
                    let corners = [offset, offset + 1, offset + patch.cells + 1, offset + patch.cells + 2];
                    if corners.iter().any(|&i| i >= patch.heights.len()) { continue; }
                    let heights = corners.map(|i| self.surface_heights[i]);
                    let gx = patch.origin[0].wrapping_add(x as i32);
                    let gz = patch.origin[1].wrapping_add(z as i32);
                    let parity = self.surface_parities[index];
                    let slope = slope_mask(heights, patch.cell_size, parity);
                    if slope == 0 { continue; }
                    // Neighbours without enough green (path, sand, soil, rock): the blades thin and shorten towards
                    // them. Beyond the patch is taken as grass.
                    let grassy = |dx: isize, dz: isize| {
                        let (nx, nz) = (x as isize + dx, z as isize + dz);
                        if nx < 0 || nz < 0 || nx >= patch.cells as isize || nz >= patch.cells as isize { return true; }
                        patch.fine_cells.get(nz as usize * patch.cells + nx as usize).is_none_or(|n| ground.green.green_layer(patch_index, n.submesh).is_some())
                    };
                    let bare = [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)]
                        .into_iter().enumerate().fold(0u32, |mask, (bit, (dx, dz))| mask | (u32::from(!grassy(dx, dz)) << bit));
                    let path_code = if !ground.has_shadow[patch.mesh] || path == super::GroundPath::Simple { 0.0 }
                        else if path == super::GroundPath::ShadowsTwoPass { 2.0 } else { 1.0 };
                    self.candidates.push(Candidate { patch: patch_index, submesh: cell.submesh, grid: [gx, gz], distance: dx * dx + dz * dz,
                        cell: Cell { origin: [world_x, world_z, patch.cell_size, path_code], heights,
                            identity: [cell_seed(gx as u32, gz as u32, cell.native_tile), u32::from(cell.orientation) | layer << 8, parity, slope | bare << 8], normals: [[0.0; 4]; 4] } });
                }
            }
            self.candidates.sort_unstable_by(|a, b| a.distance.total_cmp(&b.distance).then_with(|| a.patch.cmp(&b.patch)).then_with(|| a.grid.cmp(&b.grid)));
            self.candidates.truncate(MAX_CELLS);
        }
        if rebuild || self.pending {
            self.pending = false;
            self.cells.clear();
            self.batches.clear();
            self.order.clear();
            let mut budget = EXPOSURE_CELLS_PER_FRAME;
            let mut candidates = std::mem::take(&mut self.candidates);
            for (index, candidate) in candidates.iter_mut().enumerate() {
                if self.cell_exposed(&mut candidate.cell, &mut budget, liquids, ground, candidate.grid) { self.order.push(index); }
            }
            self.order.sort_unstable_by_key(|&index| (candidates[index].patch, candidates[index].submesh, candidates[index].grid));
            for &index in &self.order {
                let candidate = &candidates[index];
                let start = self.cells.len() as u32;
                self.cells.push(candidate.cell);
                if let Some(batch) = self.batches.last_mut().filter(|batch| batch.patch == candidate.patch && batch.submesh == candidate.submesh) {
                    batch.cells.end = start + 1;
                } else { self.batches.push(Batch { patch: candidate.patch, submesh: candidate.submesh, cells: start..start + 1 }); }
            }
            self.candidates = candidates;
            if !self.cells.is_empty() { queue.write_buffer(&self.cells_buffer, 0, bytemuck::cast_slice(&self.cells)); }
        }
        self.stride = (settings.grass_density.clamp(0.0, 2.0) * 0.5 * MAX_BLADES).round().max(1.0) as u32;
        self.band_strides = std::array::from_fn(|band| (self.stride >> band).max(1));
        self.visible.iter_mut().for_each(Vec::clear);
        self.draws.clear();
        if let Some((vp, _)) = view {
            let rows = [vp.row(3) + vp.row(0), vp.row(3) - vp.row(0), vp.row(3) + vp.row(1), vp.row(3) - vp.row(1)];
            for (index, batch) in self.batches.iter().enumerate() {
                let starts: [u32; BANDS] = std::array::from_fn(|band| self.visible[band].len() as u32);
                for cell_index in batch.cells.clone() {
                    let cell = &self.cells[cell_index as usize];
                    let (low, high) = cell.heights.into_iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), h| (low.min(h), high.max(h)));
                    let half = cell.origin[2] * 0.5;
                    let centre = Vec3::new(cell.origin[0] + half, (low + high) * 0.5, cell.origin[1] - half);
                    // The cell, its blades' height and the wind's sway; the field's own radius.
                    let bound = half * std::f32::consts::SQRT_2 + (high - low) * 0.5 + 1.0;
                    let distance = Vec3::new(centre.x - camera.x, 0.0, centre.z - camera.z).length();
                    if distance > radius + bound { continue; }
                    if rows.iter().any(|plane| plane.truncate().dot(centre) + plane.w < -bound * plane.truncate().length()) { continue; }
                    // ponytail: frustum/range culling only; a current-frame opaque-depth prepass + Hi-Z is needed
                    // to skip blades behind high floors without misclassifying visible ground under arches.
                    // Instance only ranks that can survive at the cell's nearest point.
                    let keep = thinning_keep((distance - half * std::f32::consts::SQRT_2).max(0.0));
                    let band = (0..BANDS - 1).find(|&band| keep > 0.5 / (1 << band) as f32).unwrap_or(BANDS - 1);
                    #[cfg(test)]
                    let band = self.forced_band.unwrap_or(band);
                    self.visible[band].push(cell_index);
                }
                for (band, &start) in starts.iter().enumerate() {
                    let end = self.visible[band].len() as u32;
                    if end > start { self.draws.push(Draw { batch: index, band, slots: start..end }); }
                }
            }
        } else {
            self.visible[0].extend(0..self.cells.len() as u32);
            self.draws.extend(self.batches.iter().enumerate().map(|(batch, b)| Draw { batch, band: 0, slots: b.cells.clone() }));
        }
        for (band, cells) in self.visible.iter().enumerate().filter(|(_, cells)| !cells.is_empty()) {
            queue.write_buffer(&self.visible_buffer, (band * MAX_CELLS * 4) as u64, bytemuck::cast_slice(cells));
        }
        // Spatial waves stay world-anchored; only the filtered bend follows weather.
        let [wind_x, wind_z, strength] = self.filtered_wind;
        let pixels = view.map_or(0.0, |(_, pixels)| pixels);
        let params = &mut self.params;
        params.wind_time = [wind_x, wind_z, (self.wind_phase / 0.05) as f32, radius];
        params.limits = [self.stride as f32, pixels, MIN_BLADE_PIXELS, self.bender_count as f32];
        params.wind = [strength, 1.0, 0.0, 0.0];
        params.features = [settings.grass_tufts, settings.grass_species, settings.grass_path_thinning, settings.grass_gust_bands].map(|enabled| f32::from(u8::from(enabled)));
        params.lighting = [settings.grass_sunlight, settings.grass_sun_shadows, settings.grass_point_shadows, false].map(|enabled| f32::from(u8::from(enabled)));
        params.bands = std::array::from_fn(|row| std::array::from_fn(|col| self.band_strides[row * 4 + col] as f32));
        params.benders.fill([0.0; 4]);
        params.trail_origin = [self.trails.origin[0] as f32, self.trails.origin[1] as f32, TRAIL_CELL_SIZE, f32::from(u8::from(settings.grass_trails))];
        for index in 0..self.bender_count {
            let position = self.benders[index];
            params.benders[index] = [position[0], position[1], position[2], f32::from_bits(self.owner_slots[index])];
            if settings.grass_trails {
                self.trails.write_owner(self.owners[index], &mut self.trail_samples[index * TRAIL_CELLS..(index + 1) * TRAIL_CELLS]);
            }
        }
        if settings.grass_trails && self.bender_count > 0 {
            queue.write_buffer(&self.trails_buffer, 0, bytemuck::cast_slice(&self.trail_samples[..self.bender_count * TRAIL_CELLS]));
        }
        self.bender_count = 0;
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(params));
    }
    fn cell_exposed(&mut self, cell: &mut Cell, budget: &mut usize, liquids: &[super::native_liquid::LiquidRun], ground: &GroundRun, grid: [i32; 2]) -> bool {
        let key = [cell.origin[0].to_bits(), cell.origin[1].to_bits(), cell.origin[2].to_bits(), cell.identity[0]];
        if let Some(normals) = self.exposure_cache.get(&key) {
            if let Some(normals) = normals { cell.normals = *normals; return true; }
            return false;
        }
        let Some(exposure) = &self.exposure else { return false; };
        if *budget == 0 { self.pending = true; return false; }
        *budget -= 1;
        // Exposure is cached across LOD selections, so query the native surface,
        // never the currently projected draw heights or diagonal.
        let mut native = *cell;
        for (height, [dx, dz]) in native.heights.iter_mut().zip([[0, 0], [1, 0], [0, 1], [1, 1]]) {
            let Some(value) = terrain_height(ground, grid[0] + dx, grid[1] + dz) else { return false; };
            *height = value;
        }
        native.identity[2] = (!(grid[1] as u32) ^ grid[0] as u32) & 1;
        let lowest = native.heights.into_iter().fold(f32::INFINITY, f32::min);
        let submerged = liquids.iter().any(|liquid| liquid.source.overlaps_ground_cell(cell.origin[0], cell.origin[1], cell.origin[2], lowest));
        let exposed = !submerged && [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 0.5]].into_iter().all(|uv| {
            (exposure.query)([cell.origin[0] + uv[0] * cell.origin[2], cell_height(&native, uv), cell.origin[1] - uv[1] * cell.origin[2]])
        });
        if self.exposure_order.len() == MAX_EXPOSURE_CELLS {
            if let Some(old) = self.exposure_order.pop_front() { self.exposure_cache.remove(&old); }
        }
        let normals = exposed.then(|| terrain_normals(ground, grid, cell.origin[2])).flatten();
        if let Some(normals) = normals { cell.normals = normals; }
        self.exposure_cache.insert(key, normals);
        self.exposure_order.push_back(key);
        normals.is_some()
    }
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup, ground: &GroundRun, gpu: &super::Gpu, shadow_bind: Option<&wgpu::BindGroup>) -> usize {
        if self.draws.is_empty() { return 0; }
        let bind = shadow_bind.or(self.bind.as_ref()).expect("grass receiver prepared");
        self.draw_material(pass, globals, ground, gpu, &self.pipeline, bind)
    }
    #[allow(clippy::too_many_arguments)]
    fn draw_material(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup, ground: &GroundRun, gpu: &super::Gpu, pipeline: &wgpu::RenderPipeline, bind: &wgpu::BindGroup) -> usize {
        if self.draws.is_empty() { return 0; }
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(3, bind, &[]);
        let mut draws = 0;
        pass.set_pipeline(pipeline);
        for draw in &self.draws {
            let batch = &self.batches[draw.batch];
            let patch = &ground.detail.patches()[batch.patch];
            let Some(palette) = &ground.materials[patch.mesh] else { continue; };
            let Some(gpu_draw) = gpu.opaque.get(ground.draws[batch.patch][batch.submesh]) else { continue; };
            pass.set_bind_group(1, &gpu.mats[gpu_draw.mat], &[]);
            pass.set_bind_group(2, palette, &[]);
            // The shoulder has collapsed into the tip before distant bands omit it.
            // The instance index retains the distance band in its top four bits.
            let (band, stride) = ((draw.band as u32) << 28, self.band_strides[draw.band]);
            pass.draw(0..if draw.band >= 2 { 3 } else { 6 }, band + draw.slots.start * stride..band + draw.slots.end * stride);
            draws += 1;
        }
        draws
    }
    pub(crate) fn prepare_sun_casters(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, matrix: &[[f32; 4]; 4], eye: Vec3) {
        let planes = super::frustum_planes(&super::Mat4::from_cols_array_2d(matrix));
        let buffer = self.sun_visible_buffer.get_or_insert_with(|| device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some("nearest sun grass cells"), size: (BANDS * MAX_CELLS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        }));
        for band in 0..BANDS {
            let selected = &mut self.sun_visible[band];
            selected.clear();
            selected.reserve(self.visible[band].len());
            for &index in &self.visible[band] {
                let cell = &self.cells[index as usize];
                let half = cell.origin[2] * 0.5;
                let low = cell.heights.into_iter().fold(f32::INFINITY, f32::min);
                let high = cell.heights.into_iter().fold(f32::NEG_INFINITY, f32::max);
                let center = Vec3::new(cell.origin[0] + half, (low + high) * 0.5, cell.origin[1] - half);
                let extent = Vec3::new(half + 1.01, (high - low) * 0.5 + 1.01, half + 1.01);
                if !sun_caster_in_reach(center, extent, eye) { continue; }
                if planes.iter().all(|plane| plane.dot(center.extend(1.0)) >= -plane.truncate().abs().dot(extent)) {
                    selected.push(index);
                }
            }
            if !selected.is_empty() {
                queue.write_buffer(buffer, (band * MAX_CELLS * 4) as u64, bytemuck::cast_slice(selected));
            }
        }
    }
    pub(crate) fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup, pipeline: &wgpu::RenderPipeline, bind: &wgpu::BindGroup) {
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(3, bind, &[]);
        for (band, cells) in self.sun_visible.iter().enumerate().filter(|(_, cells)| !cells.is_empty()) {
            let first = (band as u32) << 28;
            pass.draw(0..3, first..first + cells.len() as u32 * self.band_strides[band]);
        }
    }
    pub fn has_draws(&self) -> bool { !self.draws.is_empty() }
    pub fn resource_bytes(&self) -> usize { MAX_CELLS * (std::mem::size_of::<Cell>() + 4 * BANDS) + std::mem::size_of::<Params>() + MAX_BENDERS * TRAIL_CELLS * 16 + (noise::SIZE * noise::SIZE * 8) as usize + self.green_bytes + self.sun_visible_buffer.as_ref().map_or(0, |buffer| buffer.size() as usize) + self.root_cache.buffer.size() as usize }
}
/// Conservative cell/sphere overlap; the caster shader applies the exact root cutoff.
fn sun_caster_in_reach(center: Vec3, extent: Vec3, eye: Vec3) -> bool {
    ((eye - center).abs() - extent).max(Vec3::ZERO).length_squared() <= 15.0 * 15.0
}

/// Fine tile vertices and coarse lattice vertices share the same native samples.
fn surface_vertex(patch: &ao_scene::ground_detail::GroundPatch, vertex: u32) -> [i32; 2] {
    if vertex >= patch.lattice_start {
        let index = (vertex - patch.lattice_start) as usize;
        [(index % (patch.cells + 1)) as i32, (index / (patch.cells + 1)) as i32]
    } else {
        let index = vertex as usize / 4;
        let corner = patch.fine_cells[index].vertices.iter().position(|&v| v == vertex).expect("native fine vertex");
        [(index % patch.cells + corner % 2) as i32, (index / patch.cells + corner / 2) as i32]
    }
}

/// ROAM edges lie on lattice axes or diagonals. Project their planes onto the
/// lattice and retain crossed cell diagonals, reproducing the active surface exactly.
/// Both scratch allocations are reused when the native selection changes.
fn project_surface(patch: &ao_scene::ground_detail::GroundPatch, materials: &[Vec<u32>], heights: &mut Vec<f32>, parities: &mut Vec<u32>) {
    heights.clear();
    heights.extend(patch.heights.iter().map(|&h| h as f32 * (patch.height_scale / 256.0)));
    parities.clear();
    parities.extend((0..patch.cells * patch.cells).map(|i| {
        let x = patch.origin[0].wrapping_add((i % patch.cells) as i32) as u32;
        let z = patch.origin[1].wrapping_add((i / patch.cells) as i32) as u32;
        (!z ^ x) & 1
    }));
    for triangle in materials.iter().flat_map(|indices| indices.as_chunks::<3>().0) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|v| surface_vertex(patch, v));
        let cross = |p: [i32; 2], q: [i32; 2], r: [i32; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
        let area = cross(a, b, c);
        if area == 0 { continue; }
        let sample = |v: [i32; 2]| patch.heights[v[1] as usize * (patch.cells + 1) + v[0] as usize] as f32 * (patch.height_scale / 256.0);
        let [ha, hb, hc] = [a, b, c].map(sample);
        for z in a[1].min(b[1]).min(c[1])..=a[1].max(b[1]).max(c[1]) {
            for x in a[0].min(b[0]).min(c[0])..=a[0].max(b[0]).max(c[0]) {
                let weights = [cross(b, c, [x, z]), cross(c, a, [x, z]), cross(a, b, [x, z])];
                if weights.iter().all(|&w| w == 0 || w.signum() == area.signum()) {
                    heights[z as usize * (patch.cells + 1) + x as usize] =
                        (weights[0] as f32 * ha + weights[1] as f32 * hb + weights[2] as f32 * hc) / area as f32;
                }
            }
        }
        for (from, to) in [(a, b), (b, c), (c, a)] {
            let dx = to[0] - from[0];
            let dz = to[1] - from[1];
            if dx == 0 || dx.abs() != dz.abs() { continue; }
            let step = [dx.signum(), dz.signum()];
            for i in 0..dx.abs() {
                let x = (from[0] + i * step[0]).min(from[0] + (i + 1) * step[0]);
                let z = (from[1] + i * step[1]).min(from[1] + (i + 1) * step[1]);
                parities[z as usize * patch.cells + x as usize] = u32::from(step[0] == step[1]);
            }
        }
    }
}

fn slope_mask([a, b, c, d]: [f32; 4], size: f32, parity: u32) -> u32 {
    // Measure each actual terrain triangle, not the corner height range.
    let gradients = if parity == 0 { [(b - a, c - a), (d - c, d - b)] }
        else { [(b - a, d - b), (d - c, c - a)] };
    // Green hills remain covered through 75 degrees; cliff faces stay bare.
    let limit = (size * 3.732).powi(2);
    gradients.into_iter().enumerate().fold(0, |mask, (index, (x, z))| mask | (u32::from(x * x + z * z <= limit) << index))
}
/// Share of a cell's blades that survive at `distance` from the eye (grass.wgsl `thinning_keep`).
fn thinning_keep(distance: f32) -> f32 {
    let d = (distance - 12.0).max(0.0) / 10.0;
    1.0 / (1.0 + d * d)
}
fn hash(mut value: u32) -> u32 { value ^= value >> 16; value = value.wrapping_mul(0x7feb352d); value ^= value >> 15; value = value.wrapping_mul(0x846ca68b); value ^ (value >> 16) }
fn cell_seed(x: u32, z: u32, tile: u16) -> u32 { hash(hash(x) ^ hash(z.rotate_left(16)) ^ u32::from(tile)) }

pub(super) fn shader_source(channels: (bool, bool, bool, bool, bool, bool), shadows: bool) -> String {
    shader_source_with_lights(channels, shadows, false)
}
fn shader_source_with_lights(channels: (bool, bool, bool, bool, bool, bool), shadows: bool, admitted: bool) -> String {
    let source_active = channels.0 || channels.5;
    let compact_indirect = channels.5 && !channels.1 && !channels.2 && !channels.3 && !channels.4;
    let grass = crate::shader_source::load("grass.wgsl", include_str!("grass.wgsl"))
        .replace("//__AMBIENT_FIELD__", if source_active { "@location(1) ambient: vec4<f32>," } else { "" })
        .replace("//__EMISSIVE_FIELD__", if channels.1 { "@location(2) emissive: vec4<f32>," } else { "" })
        .replace("//__SUN_FIELD__", if channels.3 { "@location(3) sun: vec4<f32>, @location(4) water_cloud: f32," } else if channels.2 { "@location(3) sun: vec4<f32>," } else { "" })
        .replace("//__AMBIENT_WRITE__", if source_active { "output.ambient = vec4<f32>(0.0, 0.0, 0.0, 1.0);" } else { "" })
        // Alpha is AO porosity, not beauty opacity: -16 survives even one of eight covered MSAA samples.
        // SSGI-only grass owns depth but is not an admitted donor; keep its source zero.
        .replace("//__AO_RECEIVE__", if channels.0 {
            "let indirect = select(i.dlight, max(clamp(i.light + palette, vec3<f32>(0.0), vec3<f32>(1.0)) - i.light, vec3<f32>(0.0)), i.path > 0u);\n    let extinction = (1.0 - fog) * select(1.0, 1.0 - fog, i.path > 1u);\n    let sky_fraction = clamp(indirect / max(light, vec3<f32>(0.001)), vec3<f32>(0.0), vec3<f32>(1.0));\n    let ao_lod = i.coverage.y;\n    let receiving = albedo * light * ao_share(sky_fraction) * extinction * ao_lod;\n    output.ambient = vec4<f32>(min(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), max(receiving, vec3<f32>(0.0))), mix(-16.0, 1.0, ao_lod));"
        } else { "" })
        .replace("//__VERTEX_SHADOW_VISIBILITY__", if shadows {
            "if shadow_frame.moon_direction.w > 0.0 {\n        if g.moon_direction.w > 0.0 && dot(shadow_frame.moon_direction.xyz, g.moon_direction.xyz) > 0.9999 {\n            o.sunlight.z = grass_sun_visibility(o.root + o.normal.xyz * 0.05, o.normal.xyz);\n        }\n    } else {\n        o.sunlight.z = grass_sun_visibility(o.world + o.normal.xyz * 0.005, o.normal.xyz);\n    }\n    o.sunlight.w = grass_direct_visibility;"
        } else { "" })
        .replace("//__SHADOW_VISIBILITY__", if shadows {
            "if shadow_frame.moon_direction.w > 0.0 {\n        if g.moon_direction.w > 0.0 && dot(shadow_frame.moon_direction.xyz, g.moon_direction.xyz) > 0.9999 {\n            moon_visibility = i.sunlight.z;\n            grass_direct_visibility = 1.0;\n        }\n    } else {\n        visibility = i.sunlight.z;\n        grass_direct_visibility = i.sunlight.w;\n    }"
        } else { "" })
        .replace("//__VERTEX_POINT_SHADOW_LOSS__", if admitted {
            "if grass_params.lighting.z > 0.5 && any(b.local_share.xyz > vec3<f32>(0.0)) { o.point_loss = min(b.local_share.xyz, grass_point_shadow_loss(b.root_coverage.xyz + b.normal_sky.xyz * 0.05, b.normal_sky.xyz)); }"
        } else { "" })
        .replace("//__POINT_SHADOW_LOSS__", if admitted {
            "light = max(light - i.point_loss, vec3<f32>(0.0));"
        } else { "" })
        .replace("//__EMISSIVE_WRITE__", if channels.1 { "output.emissive = vec4<f32>(0.0);" } else { "" })
        // Same roof-tested terrain alpha and fog contract as the ground receiver.
        .replace("//__SUN_WRITE__", if channels.2 { "let sun_luma = dot(sun_share, vec3<f32>(0.2126, 0.7152, 0.0722));\n    output.sun = vec4<f32>(sun_luma * grass_direct_visibility * (1.0 - fog), i.normal.w * (1.0 - fog), fog, 1.0);" } else { "" });
    let mut source = format!("{}\nconst GRASS_MAX_CELLS: u32 = {MAX_CELLS}u;\n{grass}", crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl")));
    if channels.4 || (channels.5 && !compact_indirect) {
        source = source.replace("struct GrassOutput {", "struct GrassOutput {\n @location(5) reflection_surface: vec4<f32>,\n @location(6) reflection_mask: vec4<f32>,");
    }
    // Procedural blades own depth but are not primary terrain receivers.
    if channels.5 {
        source = source.replace("struct GrassOutput {", &format!("struct GrassOutput {{\n @location({}) albedo: vec4<f32>,", if compact_indirect { 2 } else { 7 }));
        if !channels.3 && !compact_indirect {
            source = source.replace("struct GrassOutput {", "struct GrassOutput {\n @location(4) water_cloud: f32,");
        }
    }
    if channels.3 {
        source = source.replace("output.sun =", "output.water_cloud = 0.0;\n    output.sun =");
    }
    if channels.0 { crate::enhanced_shader::append_ao_share(&mut source); }
    if shadows {
        crate::enhanced_shader::append_shadow_receiver(&mut source, 2);
        let receiver = crate::shader_source::load("grass_shadow.wgsl", include_str!("grass_shadow.wgsl"));
        source.push_str(&receiver.replace(
            "    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
            "    grass_direct_visibility = visible;\n    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
        ));
    }
    if admitted {
        crate::point_shadows::admitted_native_pool(&mut source, "vertex_lights");
        let mut receiver = crate::shader_source::load("point_shadows.wgsl", include_str!("point_shadows.wgsl")).into_owned();
        receiver.push_str(&crate::shader_source::load("point_sampling.wgsl", include_str!("point_sampling.wgsl")));
        // Keep owner trails at 10 and the shared blade cache at 14.
        receiver = receiver.replace("@binding(9)", "@binding(13)");
        for binding in 3..=6 {
            receiver = receiver.replace(&format!("@binding({binding})"), &format!("@binding({})", if binding == 4 { 17 } else { binding + 6 }));
        }
        source.push_str(&receiver);
    }
    source
}
fn cell_height(cell: &Cell, uv: [f32; 2]) -> f32 {
    let [a, b, c, d] = cell.heights;
    let [u, v] = uv;
    if cell.identity[2] == 0 {
        if u + v <= 1.0 { a + u * (b - a) + v * (c - a) }
        else { d + (1.0 - u) * (c - d) + (1.0 - v) * (b - d) }
    } else if u >= v { a + u * (b - a) + v * (d - b) }
    else { a + v * (c - a) + u * (d - c) }
}

fn terrain_height(ground: &GroundRun, x: i32, z: i32) -> Option<f32> {
    ground.detail.patches().iter().find_map(|patch| {
        let dx = x.checked_sub(patch.origin[0])?;
        let dz = z.checked_sub(patch.origin[1])?;
        if dx < 0 || dz < 0 || dx as usize > patch.cells || dz as usize > patch.cells { return None; }
        patch.heights.get(dz as usize * (patch.cells + 1) + dx as usize)
            .map(|&height| height as f32 * (patch.height_scale / 256.0))
    })
}

pub(super) fn baked_sky_open(scene: &ao_scene::Scene) -> Vec<Vec<f32>> {
    scene.ground_detail.iter().map(|patch| patch.fine_cells.iter().map(|cell| {
        cell.vertices.iter().map(|&index| {
            scene.meshes.get(patch.mesh).and_then(|mesh| mesh.vertices.get(index as usize))
                .filter(|vertex| vertex.color[3].is_finite())
                .map_or(0.0, |vertex| (1.0 - vertex.color[3]).clamp(0.0, 1.0))
        }).fold(1.0, f32::min)
    }).collect()).collect()
}

/// Matches ao-formats/terrain.rs's central differences, including global edges.
/// Computed once with tile exposure, never in the steady-state CPU prepare loop.
fn terrain_normals(ground: &GroundRun, grid: [i32; 2], size: f32) -> Option<[[f32; 4]; 4]> {
    let mut normals = [[0.0; 4]; 4];
    let sky_open = ground.detail.patches().iter().enumerate().find_map(|(index, patch)| {
        let x = grid[0].checked_sub(patch.origin[0])?;
        let z = grid[1].checked_sub(patch.origin[1])?;
        if x < 0 || z < 0 || x as usize >= patch.cells || z as usize >= patch.cells { return None; }
        ground.sky_open.get(index)?.get(z as usize * patch.cells + x as usize).copied()
    }).unwrap_or(0.0);
    for (out, [dx, dz]) in normals.iter_mut().zip([[0, 0], [1, 0], [0, 1], [1, 1]]) {
        let (x, z) = (grid[0] + dx, grid[1] + dz);
        let center = terrain_height(ground, x, z)?;
        let left = terrain_height(ground, x - 1, z);
        let right = terrain_height(ground, x + 1, z);
        let back = terrain_height(ground, x, z - 1);
        let front = terrain_height(ground, x, z + 1);
        let width = (usize::from(left.is_some()) + usize::from(right.is_some())) as f32 * size;
        let depth = (usize::from(back.is_some()) + usize::from(front.is_some())) as f32 * size;
        if width == 0.0 || depth == 0.0 { return None; }
        let dx = (right.unwrap_or(center) - left.unwrap_or(center)) / width;
        let dz = -(front.unwrap_or(center) - back.unwrap_or(center)) / depth;
        let length = (dx * dx + 1.0 + dz * dz).sqrt();
        *out = [-dx / length, 1.0 / length, -dz / length, sky_open];
    }
    Some(normals)
}


pub(super) fn select_benders(
    inputs: &mut [GrassBender], eye: Vec3, vp: super::Mat4,
    admitted: impl Iterator<Item = u32>,
    line_of_sight: Option<&dyn Fn([f32; 3], [f32; 3]) -> bool>,
) -> ([GrassBender; MAX_BENDERS], usize) {
    let mut selected = [GrassBender::default(); MAX_BENDERS];
    let mut count = 0;
    if let Some(own) = inputs.iter().find(|bender| bender.actor.is_none()) {
        selected[0] = *own;
        count = 1;
    }
    let Some(clear) = line_of_sight else { return (selected, count); };
    if inputs.len() == count { return (selected, count); }
    inputs.sort_unstable_by_key(|bender| bender.actor);
    let distance = |bender: &GrassBender| eye.distance_squared(Vec3::from_array(bender.position));
    let order = |a: &GrassBender, b: &GrassBender| distance(a).total_cmp(&distance(b)).then(a.actor.cmp(&b.actor));
    for id in admitted {
        let Ok(index) = inputs.binary_search_by_key(&Some(id), |bender| bender.actor) else { continue; };
        let bender = inputs[index];
        if distance(&bender) > 15.0 * 15.0 { continue; }
        // Near-feet proof cannot clear a low wall concealing the whole body.
        let target = Vec3::from_array(bender.position) + Vec3::Y * 0.05;
        let clip = vp * target.extend(1.0);
        if clip.w <= 0.0 || clip.z < 0.0 || clip.z > clip.w || clip.x.abs() > clip.w || clip.y.abs() > clip.w { continue; }
        if !clear(eye.to_array(), target.to_array()) { continue; }
        if count < MAX_BENDERS {
            selected[count] = bender;
            count += 1;
        } else {
            let farthest = selected[..count].iter().enumerate()
                .filter(|(_, old)| old.actor.is_some())
                .max_by(|(_, a), (_, b)| order(a, b)).map(|(index, _)| index);
            if let Some(index) = farthest.filter(|&index| order(&bender, &selected[index]).is_lt()) {
                selected[index] = bender;
            }
        }
    }
    selected[..count].sort_unstable_by_key(|bender| bender.actor);
    (selected, count)
}

impl super::Renderer {
    /// Conservative primary-geometry dependency signature, not CPU owner admission.
    pub(crate) fn point_grass_admission_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let settings = self.enhanced_settings();
        let (Some(grass), Some(ground)) = (&self.grass, &self.ground) else { return 0; };
        if !settings.ground_grass || settings.grass_density <= 0.0 || settings.grass_distance <= 0.0 || !grass.has_draws() {
            return 0;
        }
        let mut hash = std::hash::DefaultHasher::new();
        // Hash exactly the retained uniform upload; owner layers are hashed below.
        let params = bytemuck::bytes_of(&grass.params);
        hash.write(params);
        // Wind and recovery advance independently of Renderer::time, including
        // between preparation and the next primary draw.
        ((grass.wind_phase / 0.05) as f32).to_bits().hash(&mut hash);
        hash.write(bytemuck::cast_slice(&grass.filtered_wind));
        if settings.grass_trails {
            grass.trails.origin.hash(&mut hash);
            for source in &grass.trails.sources {
                source.previous.actor.hash(&mut hash);
                source.cells.len().hash(&mut hash);
                for cell in &source.cells {
                    cell.grid.hash(&mut hash);
                    hash.write(bytemuck::bytes_of(&cell.bend));
                }
            }
        }
        // ponytail: global grass signature; spatially restrict cells only if
        // owned-light grass invalidation becomes a measured bottleneck.
        hash.write(bytemuck::cast_slice(&grass.cells));
        for visible in &grass.visible {
            visible.len().hash(&mut hash);
            hash.write(bytemuck::cast_slice(visible));
        }
        grass.band_strides.hash(&mut hash);
        ground.detail.selection_key().hash(&mut hash);
        hash.write(&ground.green.layers);
        for draw in &grass.draws {
            (draw.batch, draw.band, draw.slots.start, draw.slots.end).hash(&mut hash);
            let batch = &grass.batches[draw.batch];
            let patch = &ground.detail.patches()[batch.patch];
            (batch.patch, batch.submesh).hash(&mut hash);
            self.point_mesh_revisions.get(patch.mesh).hash(&mut hash);
            ground.materials[patch.mesh].is_some().hash(&mut hash);
            self.gpu.opaque.get(ground.draws[batch.patch][batch.submesh]).is_some().hash(&mut hash);
        }
        // Reserve zero for the inactive path, not a possible hasher output.
        hash.finish().max(1)
    }
    /// Advance only Enhanced grass with real frame seconds, once per frontend frame.
    /// Stalls are bounded independently of native animation and game-day clocks.
    pub fn advance_grass_wind(&mut self, dt: f32) {
        self.foliage.advance(dt);
        self.ensure_grass();
        let (phase, wind) = self.foliage.wind_state();
        if let Some(grass) = &mut self.grass {
            grass.wind_phase = phase;
            grass.filtered_wind = wind;
            grass.trails.recover(dt);
        }
    }
    fn ensure_grass(&mut self) {
        let settings = self.enhanced_settings();
        let enabled = settings.ground_grass && settings.grass_density > 0.0 && settings.grass_distance > 0.0;
        let Some(ground) = self.ground.as_ref().filter(|_| enabled) else {
            self.grass = None;
            if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
            return;
        };
        let samples = self.msaa_samples();
        let channels = (settings.ambient_occlusion, settings.emissive_bloom, settings.needs_sun_mask(), self.world_key.water_cloud, settings.material_reflections || settings.environment_probe, settings.indirect_light);
        let admitted = settings.needs_admitted_lights();
        let shadows = settings.sun_shadows || settings.moon_shadows || (settings.ground_grass && settings.grass_sun_shadows) || admitted;
        if let Some(grass) = &mut self.grass {
            if grass.samples == samples && (grass.channels.0, grass.channels.1, grass.channels.2, grass.channels.4, grass.channels.5) == (channels.0, channels.1, channels.2, channels.4, channels.5) && grass.shadows == shadows && grass.admitted == admitted {
                if grass.fog_red != self.world.grass.fog_red {
                    grass.pipeline = self.world.grass.pipeline.clone();
                    grass.fog_red = self.world.grass.fog_red;
                }
                if grass.channels.3 != channels.3 {
                    // Paired water-output programs share the same resource layout.
                    // Keep buffers, receiver bindings and interaction history intact.
                    grass.pipeline = self.world.grass.pipeline.clone();
                    grass.channels = self.world.grass.channels;
                }
            } else {
                grass.reconfigure(&self.device, &self.world.grass);
                if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
            }
            return;
        }
        let mut grass = Grass::new(&self.device, &self.queue, &ground.green, &self.world.grass);
        (grass.wind_phase, grass.filtered_wind) = self.foliage.wind_state();
        self.grass = Some(grass);
        if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
    }
    pub(crate) fn encode_grass_roots(&mut self, encoder: &mut wgpu::CommandEncoder, mut shadows: Option<&mut crate::sun_shadows::SunShadows>, last_shadow_span: bool) {
        let Some(mut grass) = self.grass.take() else { return; };
        if !grass.has_draws() { self.grass = Some(grass); return; }
        let (grown, active) = grass.root_cache.prepare(&self.device, &self.queue, &grass.visible, &grass.band_strides);
        if grown {
            grass.refresh_plain_bind(&self.device);
            if let Some(shadows) = shadows.as_deref_mut() { shadows.grass_receiver = None; }
        }
        let owner = grass.admitted.then(|| &shadows.as_deref().expect("admitted grass requires current-frame shadow admission").actor_visibility);
        grass.root_cache.bind(&self.device, &grass.roots, &grass.cells_buffer, &grass.params_buffer,
            &grass.visible_buffer, &grass.noise_view, &grass.noise_sampler, &grass.mask_view, &grass.trails_buffer, owner);
        let timestamp_writes = if self.shadow_timer.is_some() {
            self.shadow_stage_compute(crate::shadow_pass::ShadowGpuStage::GrassRoots, true, last_shadow_span)
        } else {
            self.gpu_timer.as_mut().filter(|timer| !timer.is_recording()).map(crate::FrameGpuTimer::begin_compute_pass)
        };
        {
            let mut pass = encoder.begin_effect_compute_pass(&wgpu::ComputePassDescriptor { label: Some("shared grass blade roots"), timestamp_writes });
            grass.root_cache.encode(&mut pass, &self.globals_bg, &grass.roots, active);
        }
        self.grass = Some(grass);
    }
    pub(super) fn prepare_grass_shadow_receiver(&mut self) {
        if let (Some(grass), Some(shadows)) = (&self.grass, &mut self.sun_shadows) {
            if grass.shadows && shadows.grass_receiver.is_none() {
                shadows.grass_receiver = Some(shadows.grass_bind(&self.device, &grass.layout, &grass.cells_buffer, &grass.params_buffer, &grass.visible_buffer, &grass.noise_view, &grass.noise_sampler, &grass.mask_view, &grass.trails_buffer, &grass.root_cache.buffer, self.point_shadows.as_ref().filter(|_| grass.admitted)));
            }
        }
    }
    pub(super) fn clear_enhanced_grass_inputs(&mut self) {
        self.foliage.clear_inputs();
        if let Some(grass) = &mut self.grass { grass.inputs.clear(); }
    }
    /// Current-frame opaque character candidates. Admission, LOS and nearest-source
    /// capping use the renderer's final camera, after body preparation.
    /// Reuse the caller's candidate allocation; trails retain only admitted sources.
    /// Set after `set_actors`, which invalidates the previous batch's snapshot.
    /// The snapshot remains valid for repeated draws/screenshots until the next
    /// actor batch replaces it; visibility is still checked again on every render.
    pub fn set_enhanced_grass_inputs(&mut self, benders: &mut Vec<GrassBender>, wind: [f32; 3]) {
        self.foliage.set_inputs(benders, wind);
        self.ensure_grass();
        let (phase, filtered_wind) = self.foliage.wind_state();
        if let Some(grass) = &mut self.grass {
            std::mem::swap(&mut grass.inputs, benders);
            grass.inputs.retain(|b| b.position.iter().all(|v| v.is_finite()));
            benders.clear();
            grass.wind_phase = phase;
            grass.filtered_wind = filtered_wind;
        } else {
            *benders = Vec::new();
        }
    }
    pub fn set_enhanced_grass_exposure(&mut self, exposure: Option<GrassExposure>) {
        self.foliage.set_exposure(exposure.clone());
        self.ensure_grass();
        if let Some(grass) = &mut self.grass {
            if grass.exposure.as_ref().map(|old| old.generation) != exposure.as_ref().map(|new| new.generation) {
                grass.exposure_cache.clear();
                grass.exposure_order.clear();
                grass.last_selection = None;
                grass.pending = true;
                grass.trails.sources.clear();
            }
            grass.exposure = exposure;
        }
    }
    pub(super) fn prepare_enhanced_grass(&mut self, cam: &super::Camera, vp: super::Mat4, fog_end: f32, pixels_per_unit: f32) {
        self.ensure_grass();
        if self.grass.is_none() { return; }
        let started = std::time::Instant::now();
        let settings = self.enhanced_settings();
        if let (Some(grass), Some(ground)) = (&mut self.grass, &self.ground) {
            let (selected, count) = select_benders(
                &mut grass.inputs, cam.pos, vp, self.act.enhanced_grass_actor_ids(fog_end).filter(|id| self.interaction_admission.admitted.contains(id)),
                grass.exposure.as_ref().map(|surface| surface.line_of_sight.as_ref()),
            );
            grass.benders = selected.map(|bender| bender.position);
            grass.owners = selected.map(|bender| bender.actor);
            grass.owner_slots = selected.map(|bender| self.act.interaction_slot(bender.actor));
            grass.bender_count = count;
            if settings.grass_trails {
                grass.trails.update(&selected[..count], cam.pos);
            } else {
                grass.trails.sources.clear();
            }
            grass.prepare(&self.queue, ground, &self.liquids, cam.pos, settings, self.ground_state.path(), Some((vp, pixels_per_unit)));
            grass.prepare_cpu_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        }
    }
    pub(super) fn draw_enhanced_grass(&self, pass: &mut wgpu::RenderPass<'_>) -> usize {
        match (&self.grass, &self.ground) {
            (Some(grass), Some(ground)) => grass.draw(pass, &self.globals_bg, ground, &self.gpu, self.sun_shadows.as_ref().and_then(|shadows| shadows.grass_receiver.as_ref())),
            _ => 0,
        }
    }
    /// Bounded effect-owned buffers, excluding shared Enhanced targets.
    pub fn enhanced_grass_resource_bytes(&self) -> usize {
        self.grass.as_ref().map_or(0, Grass::resource_bytes)
    }
    pub fn enhanced_grass_prepare_cpu_ns(&self) -> u64 { self.grass.as_ref().map_or(0, |grass| grass.prepare_cpu_ns) }
    pub fn enhanced_grass_cell_count(&self) -> usize { self.grass.as_ref().map_or(0, |grass| grass.cells.len()) }
    /// Offline fixture preparation only: do not advance native draw-count clocks.
    pub(super) fn warm_grass_tiles(&mut self, camera: &super::Camera) {
        self.update_ground_detail(camera.pos);
        self.ensure_grass();
        let settings = self.enhanced_settings();
        if let (Some(grass), Some(ground)) = (&mut self.grass, &self.ground) {
            for _ in 0..=MAX_CELLS.div_ceil(EXPOSURE_CELLS_PER_FRAME) {
                grass.prepare(&self.queue, ground, &self.liquids, camera.pos, settings, self.ground_state.path(), None);
                self.queue.submit([]);
                if !grass.pending { break; }
            }
        }
    }
    /// Offline-only material/exposure/frustum rejection evidence, grouped by native tile.
    pub(super) fn grass_diagnostics(&self, camera: &super::Camera) -> String {
        use std::fmt::Write;
        let (Some(grass), Some(ground)) = (&self.grass, &self.ground) else { return "grass_state=absent\n".into(); };
        let radius = f32::from_bits(grass.last_radius);
        let mut counts = std::collections::BTreeMap::<(u16, u8), [usize; 8]>::new();
        let mut report = format!("grass_radius={radius} generated_cells={} candidates={} batches={}\n", grass.cells.len(), grass.candidates.len(), grass.batches.len());
        let unique = grass.cells.iter().map(|cell| [cell.origin[0], cell.origin[1], cell.origin[2]].map(f32::to_bits)).collect::<std::collections::BTreeSet<_>>().len();
        writeln!(report, "grass_duplicate_cells={}", grass.cells.len() - unique).unwrap();
        for band in 0..BANDS {
            writeln!(report, "grass_band={band} camera_cells={} sun_cells={} camera_candidate_blades={} sun_candidate_blades={}",
                grass.visible[band].len(), grass.sun_visible[band].len(),
                grass.visible[band].len() * grass.band_strides[band] as usize,
                grass.sun_visible[band].len() * grass.band_strides[band] as usize).unwrap();
        }
        for batch in &grass.batches {
            let patch = &ground.detail.patches()[batch.patch];
            let draw = ground.draws[batch.patch][batch.submesh];
            writeln!(report, "grass_batch patch={} submesh={} cells={:?} palette_present={} draw_index={draw} draw_present={}",
                batch.patch, batch.submesh, batch.cells, ground.materials[patch.mesh].is_some(), self.gpu.opaque.get(draw).is_some()).unwrap();
        }
        let (mut heights, mut parities) = (Vec::new(), Vec::new());
        for (pi, patch) in ground.detail.patches().iter().enumerate() {
            project_surface(patch, &ground.indices[pi], &mut heights, &mut parities);
            for (index, cell) in patch.fine_cells.iter().enumerate() {
                let gx = patch.origin[0] + (index % patch.cells) as i32;
                let gz = patch.origin[1] + (index / patch.cells) as i32;
                let x = gx as f32 * patch.cell_size;
                let z = -(gz as f32) * patch.cell_size;
                let delta = Vec3::new(x + patch.cell_size * 0.5 - camera.pos.x, 0.0, z - patch.cell_size * 0.5 - camera.pos.z);
                if delta.length() > radius + 2.5 * patch.cell_size { continue; }
                let count = counts.entry((cell.native_tile, cell.ground_material)).or_default();
                count[0] += 1;
                if ground.green.green_layer(pi, cell.submesh).is_none() { count[1] += 1; continue; }
                let offset = (index / patch.cells) * (patch.cells + 1) + index % patch.cells;
                let corners = [offset, offset + 1, offset + patch.cells + 1, offset + patch.cells + 2].map(|i| heights[i]);
                let slope = slope_mask(corners, patch.cell_size, parities[index]);
                if slope == 0 {
                    count[2] += 1;
                    writeln!(report, "grass_rejected_slope grid={gx},{gz} tile={} heights={corners:?} allowed_triangles={slope}", cell.native_tile).unwrap();
                    continue;
                }
                let Some(candidate) = grass.candidates.iter().find(|c| c.patch == pi && c.grid == [gx, gz]) else { count[3] += 1; continue; };
                let c = &candidate.cell;
                let key = [c.origin[0].to_bits(), c.origin[1].to_bits(), c.origin[2].to_bits(), c.identity[0]];
                match grass.exposure_cache.get(&key) {
                    None => { count[4] += 1; }
                    Some(None) => { count[5] += 1; }
                    Some(Some(_)) => {
                        count[6] += 1;
                        let visible = usize::from(grass.visible.iter().flatten().any(|&i| grass.cells[i as usize].origin == c.origin));
                        count[7] += visible;
                        writeln!(report, "grass_cell grid={gx},{gz} tile={} material={} origin={:?} heights={:?} allowed_triangles={} bare_neighbours={:#x} visible_this_frame={visible}", cell.native_tile, cell.ground_material, c.origin, c.heights, c.identity[3] & 3, c.identity[3] >> 8).unwrap();
                    }
                }
            }
        }
        for ((tile, material), c) in counts {
            writeln!(report, "grass_tile tile={tile} material={material} nearby={} rejected_not_green={} rejected_slope={} rejected_padding={} pending={} rejected_exposure={} generated={} visible_cells={}", c[0],c[1],c[2],c[3],c[4],c[5],c[6],c[7]).unwrap();
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grass_trail_upload_preserves_overlapping_owner_provenance() {
        let mut grid = TrailGrid::default();
        let mut actors = [
            GrassBender { actor: Some(7), position: [0.0; 3] },
            GrassBender { actor: Some(9), position: [0.0; 3] },
        ];
        grid.update(&actors, Vec3::ZERO);
        for actor in &mut actors { actor.position[0] = 2.0; }
        grid.update(&actors, Vec3::ZERO);
        grid.recover(1.0);
        let mut first = [[0.0; 4]; TRAIL_CELLS];
        let mut second = first;
        grid.write_owner(Some(7), &mut first);
        grid.write_owner(Some(9), &mut second);
        assert!(first.iter().any(|cell| cell[3] > 0.0));
        assert_eq!(first, second, "overlap must retain both owners, not just the strongest");
        grid.update(&actors[1..], Vec3::ZERO);
        grid.write_owner(Some(7), &mut first);
        assert!(first.iter().all(|cell| cell[3] == 0.0));
        let retained = second;
        grid.write_owner(Some(9), &mut second);
        assert_eq!(retained, second, "removing one owner never erases the other's recovered trail");
    }

    #[test]
    fn grass_current_gpu_hidden_owner_has_no_push_or_recovered_trail() {
        use wgpu::util::DeviceExt;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("grass admission regression requires GPU");
        let device = &renderer.device;
        let queue = &renderer.queue;
        let grass = include_str!("grass.wgsl");
        // Exercise the actual draw helpers, not a CPU approximation of the gate.
        let params_struct = grass.split("struct GrassParams {").nth(1).unwrap().split("}\n").next().unwrap();
        let helpers = grass.split("fn grass_owner_admitted").nth(1).unwrap().split("struct GOut").next().unwrap();
        let source = format!(r#"
struct GrassParams {{{params_struct}}}
@group(0) @binding(0) var<uniform> grass_params: GrassParams;
@group(0) @binding(1) var<storage, read> grass_trails: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> interaction_visibility: array<u32>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
fn grass_owner_admitted{helpers}
@compute @workgroup_size(1) fn main() {{
    let pos = vec3<f32>(0.625, 0.0, 0.625);
    result[0] = vec4<f32>(grass_push(pos, 1.0, 0.4), 0.0);
    result[1] = vec4<f32>(grass_trail(pos), 0.0, 0.0);
}}
"#);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("actual grass admission helpers"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("grass admission regression"), layout: None, module: &shader, entry_point: Some("main"),
            compilation_options: Default::default(), cache: None,
        });
        let mut params: Params = bytemuck::Zeroable::zeroed();
        params.limits[3] = 2.0;
        params.trail_origin = [-16.0, -16.0, TRAIL_CELL_SIZE, 1.0];
        params.benders[0] = [0.8, 0.0, 0.625, f32::from_bits(1)];
        params.benders[1] = [0.8, 0.0, 0.625, f32::from_bits(2)];
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: bytemuck::bytes_of(&params), usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let mut samples = vec![[0.0f32; 4]; MAX_BENDERS * TRAIL_CELLS];
        samples[0] = [1.0, 0.0, 0.0, 0.9];
        samples[TRAIL_CELLS] = [0.0, 1.0, 0.0, 0.3];
        let trails = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: bytemuck::cast_slice(&samples), usage: wgpu::BufferUsages::STORAGE,
        });
        let visibility = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 12,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let result = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 6 * 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let bindings = [&uniform, &trails, &visibility, &result];
        let entries: Vec<_> = bindings.iter().enumerate().map(|(binding, buffer)|
            wgpu::BindGroupEntry { binding: binding as u32, resource: buffer.as_entire_binding() }).collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &pipeline.get_bind_group_layout(0), entries: &entries,
        });
        // Both hidden; first visible; hidden stronger overlapping visible second;
        // same second without first; own avatar; invalid owner slot.
        for (index, (slots, count, visibility_words)) in [
            ([1, 2], 2, [0u32, 0, 0]),
            ([1, 2], 1, [0, 1, 0]),
            ([1, 2], 2, [0, 0, 1]),
            ([u32::MAX, 2], 2, [0, 0, 1]),
            ([0, 2], 1, [0, 0, 0]),
            ([u32::MAX, 2], 1, [0, 0, 0]),
        ].into_iter().enumerate() {
            params.limits[3] = count as f32;
            for (bender, slot) in params.benders.iter_mut().zip(slots) { bender[3] = f32::from_bits(slot); }
            queue.write_buffer(&uniform, 0, bytemuck::bytes_of(&params));
            queue.write_buffer(&visibility, 0, bytemuck::cast_slice(&visibility_words));
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&result, 0, &readback, index as u64 * 32, 32);
            queue.submit([encoder.finish()]);
        }
        // Blocking readback is confined to this offline regression, never rendering.
        let (send, receive) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |value| send.send(value).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receive.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        let values: &[[f32; 8]] = bytemuck::cast_slice(&mapped);
        assert_eq!(values[0], [0.0; 8], "GPU-hidden selected owners emit neither geometry push nor history");
        assert!(values[1].iter().any(|value| value.abs() > 0.0), "visible-owner positive control");
        assert_eq!(values[2], values[3], "hidden stronger trail cannot suppress or leak into visible owner's trail");
        assert_eq!(&values[2][4..6], &[0.0, 0.3]);
        assert_eq!(values[1], values[4], "own-avatar slot zero preserves interaction");
        assert_eq!(values[5], [0.0; 8], "missing or stale slots fail closed");
    }

    #[test]
    fn primary_grass_uniform_hash_tracks_wind_benders_and_trail_switch() {
        use std::hash::Hasher;
        let signature = |params: &Params| {
            let bytes = bytemuck::bytes_of(params);
            let mut hash = std::hash::DefaultHasher::new();
            hash.write(bytes);
            hash.finish()
        };
        let mut params: Params = bytemuck::Zeroable::zeroed();
        let initial = signature(&params);
        assert_eq!(initial, signature(&params));
        params.wind_time[2] = advance_wind_phase(0.0, 0.1) as f32 / 0.05;
        assert_ne!(initial, signature(&params));
        let phase = signature(&params);
        params.wind[0] = 0.85;
        assert_ne!(phase, signature(&params));
        let wind = signature(&params);
        params.benders[0] = [1.0, 0.0, 2.0, 1.0];
        params.limits[3] = 1.0;
        assert_ne!(wind, signature(&params));
        let no_trails = signature(&params);
        params.trail_origin[3] = 1.0;
        assert_ne!(no_trails, signature(&params));
    }

    #[test]
    fn sun_caster_cutoff_uses_actual_eye_and_keeps_intersecting_cells() {
        let eye = Vec3::new(100.0, 8.0, -50.0);
        assert!(sun_caster_in_reach(eye, Vec3::ONE, eye));
        assert!(sun_caster_in_reach(eye + Vec3::X * 16.0, Vec3::ONE, eye));
        assert!(!sun_caster_in_reach(eye + Vec3::X * 16.01, Vec3::ONE, eye));
        assert!(!sun_caster_in_reach(eye + Vec3::Y * 16.01, Vec3::ONE, eye));
        assert!(!sun_caster_in_reach(Vec3::ZERO, Vec3::ONE, eye));
    }

    #[test]
    fn grass_params_match_uniform_layout() {
        assert_eq!(std::mem::offset_of!(Params, features), 48);
        assert_eq!(std::mem::offset_of!(Params, lighting), 64);
        assert_eq!(std::mem::offset_of!(Params, bands), 80);
        assert_eq!(std::mem::offset_of!(Params, benders), 112);
        assert_eq!(std::mem::offset_of!(Params, trail_origin), 112 + MAX_BENDERS * 16);
        assert_eq!(std::mem::size_of::<Params>(), 128 + MAX_BENDERS * 16);
        let source = include_str!("grass.wgsl");
        assert!(source.contains("@group(3) @binding(10) var<storage, read> grass_trails: array<vec4<f32>>;"));
        assert!(source.contains("@group(0) @binding(8) var<storage, read> interaction_visibility: array<u32>;"));
        assert!(source.contains("let slot = bitcast<u32>(grass_params.benders[index].w);"));
        assert!(source.contains("if slot == 0u { return true; }"));
        assert!(source.contains("if slot >= arrayLength(&interaction_visibility) { return false; }"));
        assert!(source.contains("owner * 1024u"));
        let sections = source.split_once("fn grass_build_section(").unwrap().1.split_once("fn grass_vertex(").unwrap().0;
        assert!(sections.contains("grass_push(pos, t, height)"));
        let shader = shader_source((false, false, false, false, false, false), false);
        for contract in ["grass_params.features.x", "grass_params.features.y",
            "grass_params.features.z", "grass_params.features.w", "grass_params.lighting.x",
            "fn grass_vertex(", "o.world = edge.xyz;", "grass_build_section(", "sections[vertex >> 1u]"] {
            assert!(shader.contains(contract), "missing grass shader contract: {contract}");
        }
    }

    #[test]
    fn grass_shared_admitted_pool_uses_native_rows_and_owner_proof() {
        let source = shader_source_with_lights((false, false, false, false, false, false), true, true);
        assert!(source.contains("let dyn_count = min(bitcast<u32>(dyn_lights[322u].x), 64u);"));
        assert!(source.contains("let li = 66u + k * 4u;"));
        assert!(source.contains("if !owner_light_admitted(dyn_lights[li + 3u].w) { continue; }"));
        for binding in [9, 11, 12, 13, 17] {
            assert!(source.contains(&format!("@group(3) @binding({binding})")));
        }
        assert!(source.contains("@location(15) @interpolate(flat) point_loss: vec3<f32>"));
        let vertex_start = source.find("fn grass_vertex(").unwrap();
        let vertex_end = vertex_start + source[vertex_start..].find("\n}").unwrap();
        let vertex = &source[vertex_start..vertex_end];
        assert!(vertex.contains("o.point_loss = vec3<f32>(0.0);"));
        assert!(vertex.contains("if grass_params.lighting.z > 0.5 && any(b.local_share.xyz > vec3<f32>(0.0))"),
            "disabled shadows and zero surviving native local energy must skip the root lookup");
        assert!(vertex.contains("o.point_loss = min(b.local_share.xyz, grass_point_shadow_loss(b.root_coverage.xyz + b.normal_sky.xyz * 0.05, b.normal_sky.xyz));"),
            "root-sampled blocked light must retain the exact native energy cap");
        assert_eq!(vertex.matches("grass_point_shadow_loss(").count(), 1);
        assert!(vertex.contains("o.sunlight.z = grass_sun_visibility(o.world + o.normal.xyz * 0.005, o.normal.xyz);"),
            "admitted-light variants must retain sun visibility at the actual cached blade vertex");
        let fragment_start = source.find("fn fs_grass(").unwrap();
        let fragment_end = fragment_start + source[fragment_start..].find("\n}").unwrap();
        let fragment = &source[fragment_start..fragment_end];
        assert!(!fragment.contains("grass_point_shadow_loss("));
        assert!(!fragment.contains("point_shadow_visibility("));
        assert!(!fragment.contains("textureSampleCompare"));
        assert!(!fragment.contains("grass_sun_visibility("));
        assert!(!fragment.contains("grass_blade_visibility("));
        assert!(!fragment.contains("grass_cascade_visibility("));
        assert!(fragment.contains("visibility = i.sunlight.z;"));
        assert!(fragment.contains("grass_direct_visibility = i.sunlight.w;"));
        let sun = fragment.find("let sun_share = max(i.light - i.spec,").unwrap();
        let loss = fragment.find("light = max(light - i.point_loss, vec3<f32>(0.0));").unwrap();
        let palette = fragment.find("clamp(light + palette,").unwrap();
        assert!(sun < loss && loss < palette, "point loss cannot alter direct sun attribution or escape the native palette clamp");
        let compute_start = source.find("fn cs_grass_blades(").unwrap();
        let compute_end = compute_start + source[compute_start..].find("\n}").unwrap();
        assert!(!source[compute_start..compute_end].contains("grass_point_shadow_loss("));
        assert!(source.contains("var local_share = vec3<f32>(0.0);"));
        assert!(source.contains("b.local_share = vec4<f32>(local_share, 0.0);"));
        assert!(!source.contains("o.local_share"),
            "native energy accumulation must not reuse the renamed point-loss raster varying");
        let plain = shader_source((false, false, false, false, false, false), false);
        assert!(!plain.contains("grass_point_shadow_loss("));
        for binding in [9, 11, 12, 13, 17] {
            assert!(!plain.contains(&format!("@group(3) @binding({binding})")));
        }
    }

    #[test]
    fn grass_point_local_budget_excludes_fxs_directional_light_in_day_and_dark() {
        let source = shader_source_with_lights((false, false, false, false, false, false), true, true);
        assert!(source.contains("fn grass_directional_light("));
        assert!(source.contains("grass_directional_light(ground_n)"), "daylight must retain its root-normal directional evaluation");
        assert!(source.contains("grass_directional_light(n)"), "night must retain per-corner directional evaluation");
        assert!(source.matches("grass_directional_light(").count() >= 3);
        // Saturation and the night blend happen before native Gouraud
        // interpolation. Directional FXS energy is not a point-shadow budget.
        let shade = |sun: f32, fxs: f32, point: f32, ambient: f32, dark: f32, simple: bool| {
            let diffuse = (sun + fxs + point).clamp(0.0, 1.0);
            let native = if simple { (diffuse + ambient).clamp(0.0, 1.0) } else { diffuse };
            native * (1.0 - dark) + (fxs + point).clamp(0.0, 1.0) * dark
        };
        let saturated_directional = shade(0.0, 0.9, 0.5, 0.0, 1.0, false);
        let directional_only = shade(0.0, 0.9, 0.0, 0.0, 1.0, false);
        assert!((saturated_directional - directional_only - 0.1).abs() < 0.000001,
            "a saturated 0.9 directional + 0.5 point can lose only the surviving 0.1 local energy");
        for dark in [0.0, 0.5, 1.0] {
            for simple in [false, true] {
                let no_points = shade(0.25, 0.4, 0.0, 0.1, dark, simple);
                let with_points = shade(0.25, 0.4, 0.2, 0.1, dark, simple);
                assert!((with_points - no_points - 0.2).abs() < 0.000001);
                let saturated = shade(0.6, 0.4, 0.3, 0.1, dark, simple);
                let baseline = shade(0.6, 0.4, 0.0, 0.1, dark, simple);
                assert!((saturated - baseline) <= 0.3 + 0.000001);
            }
        }
    }

    #[test]
    fn grass_trails_recover_without_stationary_repainting() {
        let mut grid = TrailGrid::default();
        let mut actor = GrassBender { actor: Some(7), position: [0.0; 3] };
        grid.update(&[actor], Vec3::ZERO);
        assert!(grid.sources[0].cells.iter().all(|cell| cell.bend[3] == 0.0));
        actor.position[0] = 2.0;
        grid.update(&[actor], Vec3::ZERO);
        let strongest = grid.sources[0].cells.iter().map(|cell| cell.bend[3]).fold(0.0_f32, f32::max);
        assert!(strongest > 0.0);
        grid.recover(1.0);
        grid.update(&[actor], Vec3::ZERO);
        let recovered = grid.sources[0].cells.iter().map(|cell| cell.bend[3]).fold(0.0_f32, f32::max);
        assert!((recovered - (strongest - 1.0 / TRAIL_RECOVERY_SECONDS).max(0.0)).abs() < 1e-6);
        grid.recover(TRAIL_RECOVERY_SECONDS);
        let mut output = [[0.0; 4]; TRAIL_CELLS];
        grid.write(&mut output);
        assert!(output.iter().all(|cell| cell[3] == 0.0));
    }

    #[test]
    fn grass_trails_revoke_only_the_lost_source_and_stay_world_anchored() {
        let mut grid = TrailGrid::default();
        let mut actors = [
            GrassBender { actor: None, position: [-2.0, 0.0, -1.0] },
            GrassBender { actor: Some(7), position: [-2.0, 0.0, -1.0] },
        ];
        grid.update(&actors, Vec3::ZERO);
        for actor in &mut actors { actor.position[0] = 1.0; }
        grid.update(&actors, Vec3::ZERO);
        let mut before = [[0.0; 4]; TRAIL_CELLS];
        grid.write(&mut before);
        assert!(before.iter().any(|cell| cell[3] > 0.0));
        grid.update(&actors[..1], Vec3::new(2.0, 0.0, 2.0));
        let mut after = [[0.0; 4]; TRAIL_CELLS];
        grid.write(&mut after);
        assert_eq!(before, after, "camera movement never relocates world cells; overlapping admitted contribution survives");
        assert_eq!(grid.sources.len(), 1);
        grid.update(&[], Vec3::ZERO);
        assert!(grid.sources.is_empty(), "admission loss removes history immediately");
        let mut revoked = [[0.0; 4]; TRAIL_CELLS];
        grid.write(&mut revoked);
        assert!(revoked.iter().all(|cell| cell[3] == 0.0));
        // Readmission cannot resurrect a trail or bridge an unobserved movement.
        grid.update(&actors, Vec3::ZERO);
        for actor in &mut actors { actor.position[0] += 20.0; }
        grid.update(&actors, Vec3::ZERO);
        assert!(grid.sources.iter().flat_map(|source| &source.cells).all(|cell| cell.bend[3] == 0.0));
        assert!(grid.sources.iter().all(|source| source.cells.len() == TRAIL_CELLS));
    }
    #[test]
    fn first_weather_target_is_correct_without_advancing_time() {
        let raw = [3.0, 0.0, 4.0];
        let seeded = wind_target(raw);
        assert_eq!(seeded, [0.6, 0.8, 0.85]);
        assert_eq!(smooth_wind(seeded, raw, 0.0), seeded);
        assert_eq!(wind_target([0.0; 3]), [0.8, 0.6, 0.35]);
    }
    #[test]
    fn grass_wind_is_smooth_across_wrap_and_large_native_times() {
        let tau = std::f64::consts::TAU;
        let dt = 1.0_f32 / 60.0;
        let step = f64::from(dt) * 0.05;
        let before = tau - step * 0.5;
        let after = advance_wind_phase(before, dt);
        assert!((after - step * 0.5).abs() < 1e-12);
        // All temporal harmonics used by the wind shader, evaluated through
        // the actual f32 upload boundary on both sides of the wrap.
        for harmonic in [1.0_f32, 2.0, 8.0, 12.0, 14.0, 24.0] {
            let previous = (((before / 0.05) as f32 * 0.05) * harmonic + 0.7).sin();
            let next = (((after / 0.05) as f32 * 0.05) * harmonic + 0.7).sin();
            assert!((next - previous).abs() < harmonic * step as f32 + 0.00002);
        }
        let mut phase = 0.7;
        // At this native clock magnitude f32 loses a whole frame. Grass must
        // still advance, without consulting or changing that clock.
        let native_time = 16_777_216.0_f32;
        assert_eq!(native_time + dt, native_time);
        for _ in 0..600 {
            let previous = phase;
            phase = advance_wind_phase(phase, dt);
            assert!((phase - previous - step).abs() < 1e-12);
        }
        assert_eq!(advance_wind_phase(phase, 100.0), advance_wind_phase(phase, 0.1));
        for invalid in [f32::NAN, f32::INFINITY, -1.0] {
            assert_eq!(advance_wind_phase(phase, invalid), phase);
        }
    }
    #[test]
    fn grass_weather_random_walk_cannot_rephase_the_icc_field() {
        // Native weather changes its vector every frame. Evaluate tip motion
        // at ICC-scale coordinates, where rotating those coordinates by the
        // raw wind direction used to jump several wave cycles while idle.
        let root = [900.0_f32, 900.0_f32];
        let dt = 1.0 / 60.0;
        let mut filtered = [0.8, 0.6, 0.35];
        let mut raw = [0.0; 3];
        let mut phase = 0.0;
        // Analytical shader model, not GPU readback. Deterministic native-sized
        // kicks use the native 1/1.02 decay (weather.rs Wind::walk), with zero
        // base wind as at ICC; the old and new paths share this same sequence.
        let tip = |wind: [f32; 3], phase: f64, old: bool| {
            let time = (phase / 0.05) as f32;
            let p = time * 0.05;
            let [x, z] = if old {
                [root[0] * wind[0] + root[1] * wind[1], -root[0] * wind[1] + root[1] * wind[0]]
            } else { [root[0] * 0.8 + root[1] * 0.6, -root[0] * 0.6 + root[1] * 0.8] };
            let s = x * 0.39 - if old { time * 2.3 } else { p * 14.0 }
                + 1.6 * (z * 0.07 + if old { time * 0.11 } else { p }).sin()
                + 0.9 * (z * 0.21 - x * 0.05).sin();
            let gust = (0.5 + 0.5 * s.sin()).powi(4)
                * (0.55 + 0.45 * (z * 0.043 + x * 0.021 - if old { time * 0.21 } else { p * 2.0 }).sin());
            let blade_phase = root[0] * 0.3 + root[1] * 0.25;
            let sway = 0.45 + 0.35 * (x * 0.55 - if old { time * 2.4 } else { p * 12.0 } + 0.7 * blade_phase.sin()).sin()
                + 0.15 * (z * 0.23 + x * 0.31 - if old { time * 1.7 } else { p * 8.0 } + 1.3).sin()
                + (0.12 + 0.35 * gust) * (if old { time * 5.3 } else { p * 24.0 } + blade_phase * 1.7).sin();
            let strength = wind[2];
            let lean = if strength <= 1.0 { strength }
                else { 1.0 + 1.3 * (1.0 - (-(strength - 1.0) / 1.2).exp()) };
            let bend = 0.5 * lean * (0.16 * sway + 0.7 * gust);
            let mut x = wind[0] * bend;
            let mut z = wind[1] * bend;
            let length = x.hypot(z);
            if length > 0.45 { x *= 0.45 / length; z *= 0.45 / length; }
            [x, (0.25 - x * x - z * z).max(0.0).sqrt() - 0.5, z]
        };
        let displacement = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let mut previous = tip(filtered, phase, false);
        let mut previous_old = tip(wind_target(raw), phase, true);
        let mut sums = [0.0_f32; 2];
        let mut maxima = [0.0_f32; 2];
        println!("analytical_native_like_wind_csv,frame,old_tip_displacement_m,new_tip_displacement_m");
        for frame in 0..3600_u32 {
            raw[0] = raw[0] / 1.02 + (hash(frame * 2) as f32 / u32::MAX as f32 - 0.5) * 3.4;
            raw[2] = raw[2] / 1.02 + (hash(frame * 2 + 1) as f32 / u32::MAX as f32 - 0.5) * 3.4;
            filtered = smooth_wind(filtered, raw, dt);
            phase = advance_wind_phase(phase, dt);
            let next = tip(filtered, phase, false);
            let step = displacement(next, previous);
            assert!(step < 0.01, "frame {frame}: tip moved {step} world units");
            previous = next;
            if frame < 120 {
                let next_old = tip(wind_target(raw), phase, true);
                let old_step = displacement(next_old, previous_old);
                previous_old = next_old;
                for (i, value) in [old_step, step].into_iter().enumerate() {
                    sums[i] += value;
                    maxima[i] = maxima[i].max(value);
                }
                println!("analytical_native_like_wind_csv,{frame},{old_step:.9},{step:.9}");
            }
        }
        println!("analytical_native_like_wind_summary,frames=120,old_max_m={:.9},old_mean_m={:.9},new_max_m={:.9},new_mean_m={:.9},hard_bound_m=0.01",
            maxima[0], sums[0] / 120.0, maxima[1], sums[1] / 120.0);
    }
    #[test]
    fn grass_hidden_or_non_admitted_character_cannot_bend_evict_or_leave_a_trail() {
        use ao_scene::{Instance, Mesh, Scene, Submesh, Vertex, IDENTITY};
        let wall = Scene {
            meshes: vec![Mesh {
                vertices: [[0.0, 0.0, -2.0], [0.0, 0.5, -2.0], [0.0, 0.5, 2.0], [0.0, 0.0, 2.0]]
                    .map(|pos| Vertex { pos, ..Default::default() }).to_vec(),
                submeshes: vec![Submesh::new(vec![0, 1, 2, 0, 2, 3], None)],
            }],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }],
            ..Default::default()
        };
        let surface = ao_formats::playfield::collision::Collision::from_scene(&wall);
        let clear = |a, b| surface.inside(a) && surface.inside(b)
            && surface.line(a, b).is_none() && surface.line(b, a).is_none();
        let eye = Vec3::new(-2.0, 1.0, 0.0);
        let projection = super::super::Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.2, 100.0);
        let vp = projection * super::super::Mat4::look_at_rh(eye, Vec3::X, Vec3::Y);
        let own = GrassBender { actor: None, position: [2.0, 0.0, 0.0] };
        let visible = GrassBender { actor: Some(1), position: [-1.0, 0.0, 0.0] };
        let hidden = GrassBender { actor: Some(2), position: [0.1, 0.0, 0.0] };
        let run = |mut inputs: Vec<GrassBender>, admitted: Vec<u32>| {
            select_benders(&mut inputs, eye, vp, admitted.into_iter(), Some(&clear))
        };
        let mut snapshot = vec![own, visible, hidden];
        let first = select_benders(&mut snapshot, eye, vp, [1, 2].into_iter(), Some(&clear));
        assert_eq!(first, select_benders(&mut snapshot, eye, vp, [1, 2].into_iter(), Some(&clear)), "re-rendering one actor snapshot preserves exactly the same bending");
        assert_eq!(first.1, 2);
        assert_eq!(first, run(vec![own, visible], vec![1]), "low-wall-hidden actor is identical to absence");
        assert_eq!(run(vec![own, visible], vec![]), run(vec![own], vec![]), "loss of current body admission removes bending immediately");
        assert_eq!(run(vec![], vec![]).1, 0, "no previous-frame trail survives");
        let crowd = (3..=18).map(|id| GrassBender { actor: Some(id), position: [-0.2, 0.0, 1.5] });
        let mut candidates = std::iter::once(own).chain(crowd).collect::<Vec<_>>();
        let ids = (3..=18).collect::<Vec<_>>();
        let before = run(candidates.clone(), ids.clone());
        assert_eq!(before.1, MAX_BENDERS);
        candidates.push(hidden);
        assert_eq!(before, run(candidates.clone(), std::iter::once(2).chain(ids.clone()).collect()), "hidden actor cannot evict a visible source at the cap");
        candidates.last_mut().unwrap().position = visible.position;
        assert_eq!(before, run(candidates, ids), "nearer non-admitted body cannot evict a visible source");
        let final_eye = Vec3::new(2.0, 1.0, 0.0);
        let final_vp = projection * super::super::Mat4::look_at_rh(final_eye, Vec3::from_array(visible.position), Vec3::Y);
        let selected = select_benders(&mut [own, visible], final_eye, final_vp, [1].into_iter(), Some(&clear));
        assert_eq!(selected.1, 1, "LOS uses the final render camera, not the pre-shake camera");
        assert_eq!(select_benders(&mut [own, visible], eye, vp, [1].into_iter(), None).1, 1, "missing collision admits only own-character interaction");
    }

    #[test]
    fn cells_exactly_follow_current_fine_and_coarse_roam_triangles() {
        use ao_scene::ground_detail::{GroundCell, GroundDetail, GroundPatch};
        let cells = 16;
        let patch = GroundPatch {
            origin: [3, -5], cells, cell_size: 2.0, height_scale: 1.0,
            heights: (0..(cells + 1) * (cells + 1))
                .map(|i| (256 + (i * 73 + i * i * 11) % 211) as u16).collect(),
            lattice_start: (cells * cells * 4) as u32, coarse_submesh: 1,
            fine_cells: (0..cells * cells).map(|i| GroundCell {
                vertices: std::array::from_fn(|corner| (i * 4 + corner) as u32), ..Default::default()
            }).collect(),
            ..Default::default()
        };
        let mut detail = GroundDetail::new(vec![patch]);
        let mut indices = vec![vec![Vec::new(), Vec::new()]];
        let (mut heights, mut parities) = (Vec::new(), Vec::new());
        for eye in [[22.0, 2.0, -6.0], [200.0, 2.0, -200.0]] {
            detail.update(eye, 100, 7, &mut indices);
            let patch = &detail.patches()[0];
            if eye[0] < 100.0 {
                assert!(!indices[0][0].is_empty(), "near selection includes fine tile triangles");
                assert!(!indices[0][1].is_empty(), "same selection also includes coarse triangles");
            } else {
                assert!(indices[0][0].is_empty(), "far selection is entirely coarse");
            }
            project_surface(patch, &indices[0], &mut heights, &mut parities);
            assert!(heights.iter().zip(&patch.heights).any(|(&draw, &native)| (draw - native as f32 / 256.0).abs() > 0.01),
                "nonplanar native samples must actually be replaced by active coarse planes");
            for triangle in indices[0].iter().flat_map(|list| list.as_chunks::<3>().0) {
                let vertices = [triangle[0], triangle[1], triangle[2]].map(|v| surface_vertex(patch, v));
                for weights in [[0.17, 0.31, 0.52], [0.63, 0.11, 0.26], [0.04, 0.93, 0.03]] {
                    let point: [f32; 2] = std::array::from_fn(|axis| (0..3).map(|i| vertices[i][axis] as f32 * weights[i]).sum());
                    let expected: f32 = (0..3).map(|i| {
                        patch.heights[vertices[i][1] as usize * (cells + 1) + vertices[i][0] as usize] as f32 / 256.0 * weights[i]
                    }).sum();
                    let [x, z] = point.map(|v| (v.floor() as usize).min(cells - 1));
                    let offset = z * (cells + 1) + x;
                    let cell = Cell {
                        origin: [0.0; 4],
                        heights: [offset, offset + 1, offset + cells + 1, offset + cells + 2].map(|i| heights[i]),
                        identity: [0, 0, parities[z * cells + x], 0], normals: [[0.0; 4]; 4],
                    };
                    let actual = cell_height(&cell, [point[0] - x as f32, point[1] - z as f32]);
                    assert!((actual - expected).abs() < 0.00002, "point {point:?}: active triangle {expected}, grass {actual}");
                }
            }
        }
    }

    #[test]
    fn deterministic_native_material_density_is_bounded() {
        assert_eq!(hash(1), 0x688990c0);
        assert_eq!(cell_seed(100, 200, 2), 0xf2964aa6);
        assert_ne!(cell_seed(100, 200, 2), cell_seed(101, 200, 2));
        for parity in 0..2 {
            assert_eq!(slope_mask([0.0, 2.0, 2.0, 4.0], 4.0, parity), 3, "diagonal 35-degree grass slopes stay covered");
            assert_eq!(slope_mask([0.0, 0.0, 3.36, 3.36], 4.0, parity), 3);
            assert_eq!(slope_mask([0.0, 0.0, 4.0, 4.0], 4.0, parity), 3, "45-degree green hills stay covered");
            assert_eq!(slope_mask([0.0, 0.0, 6.928, 6.928], 4.0, parity), 3, "60-degree green hills stay covered");
            assert_eq!(slope_mask([0.0, 0.0, 14.92, 14.92], 4.0, parity), 3, "slopes just below 75 degrees stay covered");
            assert_eq!(slope_mask([0.0, 0.0, 14.93, 14.93], 4.0, parity), 0, "75-degree and steeper cliffs stay bare");
        }
        assert_eq!(slope_mask([0.0, 0.0, 0.0, 16.0], 4.0, 0), 1, "one cliff triangle cannot erase its flat neighbor");
        assert!(MAX_CELLS * (std::mem::size_of::<Cell>() + 4 * BANDS) + std::mem::size_of::<Params>() < 3 * 1024 * 1024);
        assert_eq!(thinning_keep(12.0), 1.0);
        assert_eq!(thinning_keep(22.0), 0.5);
        assert_eq!(thinning_keep(42.0), 0.1);
        assert!(thinning_keep(27.0) > 0.25, "geometry morph must finish before the next thinning band");
        let shader = include_str!("grass.wgsl");
        assert!(shader.contains("max(dist - 12.0, 0.0) / 10.0"), "CPU and GPU thinning must agree");
        assert!(shader.contains("smoothstep(22.0, 27.0, dist)"));
        assert!(shader.contains("select(0.0, 1.0 - simple_tuft, is_head)"), "head colour must finish fading before the topology switch");
        let mut cell = Cell { origin: [0.0; 4], heights: [0.0, 2.0, 4.0, 8.0], identity: [0; 4], normals: [[0.0; 4]; 4] };
        for parity in 0..2 {
            cell.identity[2] = parity;
            for (uv, expected) in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].into_iter().zip(cell.heights) {
                assert_eq!(cell_height(&cell, uv), expected);
            }
        }
        cell.identity[2] = 0;
        assert_eq!(cell_height(&cell, [0.5, 0.5]), 3.0);
        cell.identity[2] = 1;
        assert_eq!(cell_height(&cell, [0.5, 0.5]), 4.0);
    }
}

#[cfg(test)]
mod blade_appearance_tests {
    fn night_grass_scene() -> ao_scene::Scene {
        use ao_scene::{ground_detail::{GroundCell, GroundPatch}, Environment, Instance, Mesh, Scene, Submesh, Texture, TextureKey, TextureSource, Vertex, IDENTITY};

        let tile = TextureKey { rdb_type: 1_010_006, id: 1 };
        let palette = TextureKey { rdb_type: 1_000_007, id: 2 };
        let mut scene = Scene {
            daylight: Some(0.0),
            environment: Some(Environment {
                sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 100.0, fog_end: 200.0,
                ambient: [0.12f32, 0.16, 0.10].map(|v| v.powf(2.2)),
                sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0,
            }),
            meshes: vec![Mesh {
                vertices: [[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [0.0, 0.0, -8.0], [8.0, 0.0, -8.0]]
                    .map(|pos| Vertex { pos, normal: [0.0, 1.0, 0.0], uv: [0.5; 2], color: [1.0; 4] }).to_vec(),
                submeshes: vec![
                    Submesh::new(vec![0, 1, 2, 1, 3, 2], Some(tile)),
                    Submesh::new(vec![0; 6], Some(tile)),
                ],
            }],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }],
            ground_detail: vec![GroundPatch {
                mesh: 0, cells: 1, cell_size: 8.0, height_scale: 1.0, heights: vec![0; 4],
                coarse_submesh: 1,
                fine_cells: vec![GroundCell { submesh: 0, vertices: [0, 1, 2, 3], native_tile: 2, ..Default::default() }],
                shadow_texture: Some(palette), shadow_uv_scale: [0.125; 2],
                ..Default::default()
            }],
            ..Default::default()
        };
        for (key, rgb, source) in [
            (tile, [60, 104, 40], TextureSource::GroundRgb),
            (palette, [17, 31, 11], TextureSource::default()),
        ] {
            scene.textures.insert(key, Texture {
                width: 8, height: 8, rgba: [rgb[0], rgb[1], rgb[2], 255].repeat(64), source,
            });
        }
        scene
    }

    #[test]
    fn grass_point_receives_cached_world_shadows_and_restores_without_world_overrides() {
        use crate::{Camera, EnhancedSettings, Renderer, Targets, Vec3};
        let mut scene = night_grass_scene();
        scene.lights.push(ao_scene::Light {
            pos: [3.0, 0.8, -4.0], color: [0.6, 0.45, 0.25], range: 12.0, ..Default::default()
        });
        // A real opaque world wall blocks the lamp; blades receive its cached cube
        // but never populate a separate grass caster map.
        scene.meshes.push(ao_scene::Mesh {
            vertices: [[3.5, 0.0, -2.0], [3.5, 1.5, -2.0], [3.5, 0.0, -6.0], [3.5, 1.5, -6.0]]
                .map(|pos| ao_scene::Vertex { pos, normal: [-1.0, 0.0, 0.0], uv: [0.5; 2], color: [1.0; 4] }).to_vec(),
            submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2, 1, 3, 2], None)],
        });
        scene.instances.push(ao_scene::Instance { mesh: 1, transform: ao_scene::IDENTITY });
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("grass cached point receiver GPU");
        renderer.set_msaa_samples(1);
        renderer.upload(&scene);
        let camera = Camera::look_at(Vec3::new(4.0, 3.0, 1.0), Vec3::new(4.0, 0.0, -4.0));
        let settings = EnhancedSettings {
            ground_grass: true, grass_density: 2.0, grass_distance: 60.0,
            point_shadow_count: 1, ..Default::default()
        };
        renderer.set_enhanced_settings(settings);
        renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
            generation: 1, query: std::rc::Rc::new(|_| true),
            line_of_sight: std::rc::Rc::new(|_, _| true),
        }));
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("independent grass cached point receiver"),
            size: wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let targets = Targets::new(&renderer, 256, 256);
        for mode in [3, 1, 0] {
            renderer.set_ground_rendering(mode);
            renderer.warm_grass_tiles(&camera);
            let mut images = Vec::new();
            for enabled in [false, true, false] {
                renderer.set_enhanced_settings(EnhancedSettings { grass_point_shadows: enabled, ..settings });
                renderer.render(&view, &targets, &camera);
                assert!(!renderer.enhanced_settings.clustered_lighting);
                assert!(!renderer.enhanced_settings.local_light_shadows);
                assert!(!renderer.grass.as_ref().unwrap().cells.is_empty());
                if enabled {
                    assert!(renderer.point_shadows.is_some(), "independent grass receiving must request the shared cached cubes");
                }
                images.push(crate::texture_to_rgba(&renderer, &output, 256, 256).unwrap());
            }
            assert_ne!(images[0], images[1], "native mode {mode}: cached world occlusion must shade grass pixels");
            assert_eq!(images[0], images[2], "native mode {mode}: point off must restore frozen native light");
        }
    }

    #[test]
    fn grass_sun_casters_change_pixels_and_toggle_back_exactly() {
        use crate::{Camera, EnhancedSettings, Renderer, Targets, Vec3};
        let mut scene = night_grass_scene();
        let sun = [0.6, 0.8, 0.0];
        scene.daylight = Some(1.0);
        scene.sun_authored = true;
        scene.ground_light = Some(([0.7; 3], sun));
        let environment = scene.environment.as_mut().unwrap();
        environment.ambient = [0.15; 3];
        environment.sun_color = [0.7; 3];
        environment.sun_dir = sun;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("grass caster requires an offscreen GPU");
        renderer.set_msaa_samples(1);
        renderer.upload(&scene);
        renderer.set_ground_rendering(3);
        let camera = Camera::look_at(Vec3::new(4.0, 5.0, 1.0), Vec3::new(4.0, 0.0, -4.0));
        let settings = EnhancedSettings {
            ground_grass: true, grass_density: 2.0, grass_distance: 60.0,
            grass_sunlight: true, sun_shadows: false, shadow_softness: 0.0,
            shadow_strength: 1.0, ..Default::default()
        };
        renderer.set_enhanced_settings(settings);
        renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
            generation: 1, query: std::rc::Rc::new(|_| true),
            line_of_sight: std::rc::Rc::new(|_, _| true),
        }));
        renderer.warm_grass_tiles(&camera);
        assert!(!renderer.grass.as_ref().unwrap().cells.is_empty());
        let cells_buffer = renderer.grass.as_ref().unwrap().cells_buffer.clone();
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grass caster toggle regression"),
            size: wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let targets = Targets::new(&renderer, 256, 256);
        let mut images = Vec::new();
        for enabled in [false, true, false] {
            renderer.set_enhanced_settings(EnhancedSettings { grass_sun_shadows: enabled, ..settings });
            renderer.render(&view, &targets, &camera);
            let grass = renderer.grass.as_ref().unwrap();
            assert_eq!(grass.cells_buffer, cells_buffer, "shader switches must retain the terrain cache");
            assert_eq!(grass.exposure.as_ref().map(|exposure| exposure.generation), Some(1),
                "shader switches must not revoke the native terrain exposure capability");
            assert!(!grass.cells.is_empty(), "changed-image evidence must contain actual blades on every toggle");
            if enabled {
                assert!(grass.sun_visible.iter().any(|cells| !cells.is_empty()),
                    "15m selection must retain actual near blade casters");
            } else {
                assert!(grass.sun_visible.iter().all(Vec::is_empty),
                    "disabled sun casters must not retain stale selections");
            }
            images.push(crate::texture_to_rgba(&renderer, &output, 256, 256).unwrap());
        }
        assert_ne!(images[0], images[1], "blade casters must change visible ground/grass pixels");
        assert_eq!(images[0], images[2], "disabling blade casters must restore the frozen baseline");
    }

    #[test]
    fn night_blades_inherit_native_lighting_without_becoming_black() {
        use crate::{Camera, EnhancedSettings, GroundPath, Renderer, Targets, Vec3};
        let scene = night_grass_scene();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("night grass requires an offscreen GPU");
        renderer.set_msaa_samples(1);
        renderer.upload(&scene);
        let camera = Camera::look_at(Vec3::new(4.0, 5.0, 1.0), Vec3::new(4.0, 0.0, -4.0));
        let out = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("night grass regression"), size: wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        let luma = |pixel: &[u8]| {
            0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2])
        };
        // The native mode history is significant: zero after shadows selects Simple.
        for (mode, expected) in [(3, GroundPath::Fused), (1, GroundPath::ShadowsTwoPass), (0, GroundPath::Simple)] {
            renderer.set_ground_rendering(mode);
            assert_eq!(renderer.ground_rendering_path(), expected);
            renderer.set_enhanced_settings(EnhancedSettings::default());
            renderer.render(&view, &Targets::new(&renderer, 256, 256), &camera);
            let ground = crate::texture_to_rgba(&renderer, &out, 256, 256).unwrap();
            renderer.set_enhanced_settings(EnhancedSettings {
                ground_grass: true, grass_density: 1.0, grass_distance: 60.0, ..Default::default()
            });
            renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
                generation: 1, query: std::rc::Rc::new(|_| true), line_of_sight: std::rc::Rc::new(|_, _| true),
            }));
            renderer.warm_grass_tiles(&camera);
            renderer.render(&view, &Targets::new(&renderer, 256, 256), &camera);
            let blades = crate::texture_to_rgba(&renderer, &out, 256, 256).unwrap();
            let reference = &ground[(128 * 256 + 128) * 4..][..3];
            let mut visible = 0;
            // Only interior ground pixels: no silhouette against black sky can masquerade as a blade.
            for y in 96..160 {
                for x in 96..160 {
                    let offset = (y * 256 + x) * 4;
                    let bare = &ground[offset..offset + 3];
                    let blade = &blades[offset..offset + 3];
                    assert_eq!(bare, reference, "{expected:?}: the reference ground must be flat and constant");
                    assert!(luma(bare) > 1.0, "{expected:?}: night ambient/palette must remain nonzero");
                    if luma(blade) > 0.0 && blade != bare {
                        visible += 1;
                    }
                }
            }
            assert!(visible > 0, "{expected:?}: lit night blades must remain visible, not culled or blacked out");
        }
    }

    #[test]
    fn native_grass_moon_uses_root_exposure_without_reducing_fog() {
        use crate::{Camera, EnhancedSettings, GroundPath, NightMode, Renderer, Targets, Vec3};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("grass moon requires an offscreen GPU");
        renderer.set_msaa_samples(1);
        let camera = Camera::look_at(Vec3::new(4.0, 5.0, 1.0), Vec3::new(4.0, 0.0, -4.0));
        let out = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native grass moon regression"),
            size: wgpu::Extent3d { width: 128, height: 128, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        for fogged in [false, true] {
            let mut scene = night_grass_scene();
            scene.environment.as_mut().unwrap().sun_dir = [0.0, -1.0, 0.0];
            scene.moons[0] = Some(ao_scene::MoonSource {
                toward_light: [0.0, 1.0, 0.0], ..Default::default()
            });
            // Keep the shared terrain receiver closed: every changed pixel must
            // come from grass's existing cell exposure, not global lattice fill.
            for vertex in &mut scene.meshes[0].vertices { vertex.color[3] = 1.0; }
            if fogged {
                let environment = scene.environment.as_mut().unwrap();
                environment.fog_start = 0.0;
                environment.fog_end = 0.1;
            }
            renderer.upload(&scene);
            for (mode, expected) in [(3, GroundPath::Fused), (1, GroundPath::ShadowsTwoPass), (0, GroundPath::Simple)] {
                renderer.set_ground_rendering(mode);
                assert_eq!(renderer.ground_rendering_path(), expected);
                let settings = EnhancedSettings {
                    ground_grass: true, grass_density: 1.0, grass_distance: 60.0,
                    night_mode: NightMode::Moonlit, moon_intensity: 0.0,
                    sun_shadows: false, moon_shadows: false, ..Default::default()
                };
                renderer.set_enhanced_settings(settings);
                renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
                    generation: 1, query: std::rc::Rc::new(|_| true),
                    line_of_sight: std::rc::Rc::new(|_, _| true),
                }));
                renderer.warm_grass_tiles(&camera);
                assert!(!renderer.grass.as_ref().unwrap().cells.is_empty());
                let targets = Targets::new(&renderer, 128, 128);
                let mut images = Vec::new();
                for (exposed, intensity) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)] {
                    renderer.set_enhanced_settings(EnhancedSettings { moon_intensity: intensity, ..settings });
                    let grass = renderer.grass.as_mut().unwrap();
                    for cell in &mut grass.cells { cell.normals[0][3] = exposed; }
                    renderer.queue.write_buffer(&grass.cells_buffer, 0, bytemuck::cast_slice(&grass.cells));
                    renderer.render(&view, &targets, &camera);
                    images.push(crate::texture_to_rgba(&renderer, &out, 128, 128).unwrap());
                }
                assert_eq!(images[0], images[1], "{expected:?}: closed roots cannot receive moon");
                assert_eq!(images[0], images[2], "{expected:?}: exposure alone adds no ambient/local energy");
                if fogged {
                    assert_eq!(images[2], images[3], "{expected:?}: moon cannot reveal fully fogged roots or tips");
                } else {
                    let brighter = images[2].as_chunks::<4>().0.iter().zip(images[3].as_chunks::<4>().0.iter())
                        .filter(|(off, on)| on[..3].iter().zip(&off[..3]).any(|(a, b)| a > b)).count();
                    assert!(brighter > 0, "{expected:?}: open native grass must receive unshadowed moon with sun shadows off");
                }
                renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
                    generation: 2, query: std::rc::Rc::new(|_| false),
                    line_of_sight: std::rc::Rc::new(|_, _| false),
                }));
                renderer.warm_grass_tiles(&camera);
                assert!(renderer.grass.as_ref().unwrap().cells.is_empty(),
                    "moon cannot admit hidden/non-admitted grass");
            }
        }
    }

    #[test]
    fn completed_grass_morph_has_identical_pixels_across_distance_bands() {
        use crate::{Camera, EnhancedSettings, Renderer, Targets, Vec3};
        let mut scene = night_grass_scene();
        scene.daylight = Some(1.0);
        let environment = scene.environment.as_mut().unwrap();
        environment.ambient = [0.3; 3];
        environment.sun_color = [0.7; 3];
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("grass topology requires an offscreen GPU");
        renderer.set_msaa_samples(1);
        renderer.upload(&scene);
        renderer.set_enhanced_settings(EnhancedSettings {
            ground_grass: true, grass_density: 2.0, grass_distance: 60.0, ..Default::default()
        });
        renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
            generation: 1, query: std::rc::Rc::new(|_| true), line_of_sight: std::rc::Rc::new(|_, _| true),
        }));
        // Every root is beyond the completed morph; keep the camera and all uniforms frozen.
        let camera = Camera::look_at(Vec3::new(4.0, 3.0, 30.0), Vec3::new(4.0, 0.0, -4.0));
        renderer.warm_grass_tiles(&camera);
        let out = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grass topology regression"), size: wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        let targets = Targets::new(&renderer, 256, 256);
        let mut images = Vec::new();
        for band in [1, 2] {
            renderer.grass.as_mut().expect("enabled grass").forced_band = Some(band);
            renderer.render(&view, &targets, &camera);
            let grass = renderer.grass.as_ref().unwrap();
            assert!(!grass.visible[band].is_empty(), "fixture must submit grass in forced band {band}");
            images.push(crate::texture_to_rgba(&renderer, &out, 256, 256).unwrap());
        }
        renderer.set_enhanced_settings(EnhancedSettings::default());
        renderer.render(&view, &targets, &camera);
        let ground = crate::texture_to_rgba(&renderer, &out, 256, 256).unwrap();
        assert!(images[0] != ground, "fixture must produce visible grass, not two bare-ground images");
        let mismatch = images[0].iter().zip(&images[1]).position(|(a, b)| a != b);
        assert_eq!(mismatch, None, "changing only the distance band must not change grass pixels");
    }

    #[test]
    fn beauty_alone_writes_grass_coverage_and_depth() {
        let source = include_str!("grass.rs");
        let compiled = source.split("impl Compiled {").nth(1).unwrap().split("impl Grass {").next().unwrap();
        assert_eq!(compiled.matches("render_deferred(").count(), 1, "grass needs only its beauty geometry pipeline");
        assert!(compiled.contains("depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual)"),
            "beauty must depth-test against terrain and write surviving blade samples");
        assert!(compiled.contains("alpha_to_coverage_enabled: samples > 1"),
            "MSAA colour and depth must retain the same blade coverage");
        let draw = source.split("pub fn draw(&self,").nth(1).unwrap().split("fn draw_material(").next().unwrap();
        assert_eq!(draw.matches("self.draw_material(").count(), 1);
        assert!(!draw.contains("pass.draw("), "no full-field geometry submission before material batches");
        let grass = include_str!("grass.wgsl");
        assert!(grass.contains("output.color = vec4<f32>(color, i.coverage.x);"),
            "beauty alpha must drive the hardware coverage mask for both colour and depth");
    }

    #[test]
    fn blades_never_hide_targets_and_every_receiver_variant_builds() {
        let grass = include_str!("grass.wgsl");
        assert!(grass.contains("const GRASS_MAX_HEIGHT: f32 = 0.5;"), "fairness: no blade, flower or seed head above half a unit");
        assert!(grass.contains("height = min(height, GRASS_MAX_HEIGHT);"));
        assert!(grass.contains("max(vertex_fog(-(g.view * vec4<f32>(left, 1.0)).z), root_fog)")
            && grass.contains("max(vertex_fog(-(g.view * vec4<f32>(right_edge, 1.0)).z), root_fog)")
            && grass.contains("o.fog = edge.w;"),
            "fog guard: both cached edges retain the native root extinction floor");
        assert!(grass.contains("let sun_share = max(i.light - i.spec, vec3<f32>(0.0));"),
            "grass and native ground must remove direct sun before palette saturation");
        assert!(grass.contains("textureSampleLevel(tex, samp_clamp, i.uv, coarse_level)"),
            "distant grass must use the same 8x8 tile mip as coarse terrain");
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("grass WGSL requires offscreen GPU");
        for flags in 0..32 {
            let water = flags & 16 != 0;
            let (ao, bloom, shadows, hdr) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0 || water);
            let source = super::shader_source((ao, bloom, hdr, water, false, false), shadows);
            assert_eq!(source.contains("output.water_cloud = 0.0;"), water);
            assert_eq!(source.contains("output.emissive = vec4<f32>(0.0);"), bloom);
            assert_eq!(source.contains("let receiving = albedo *"), ao);
            assert_eq!(source.contains("fn ao_share("), ao, "grass must reuse the terrain AO budget");
            assert_eq!(source.contains("let ao_lod = i.coverage.y;"), ao,
                "subpixel distant tufts must not retain near-blade self-occlusion");
            assert_eq!(source.contains("mix(-16.0, 1.0, ao_lod)"), ao, "AO-only porosity must survive MSAA resolve");
            assert_eq!(source.contains("output.sun = "), hdr);
            assert_eq!(source.contains("dot(sun_share, vec3<f32>(0.2126, 0.7152, 0.0722))"), hdr,
                "HDR daylight eligibility must use the native terrain's luma weights");
            assert_eq!(source.contains("o.sunlight.z = grass_sun_visibility("), shadows);
            assert_eq!(source.contains("i.normal.w * (1.0 - fog), fog, 1.0)"), hdr,
                "grass must share terrain sky openness and native fog channels");
            assert_eq!(source.contains("grass_direct_visibility = visible;"), shadows,
                "the HDR sun mask excludes beauty's retained shadow floor");
            // Creating the pipeline validates the shader against the real bind-group layouts.
            let grass = super::Compiled::new(&renderer.device, &renderer.g_layout, &renderer.tex_layout, &renderer.holo_layout, 1, (ao, bloom, hdr, water, false, false), shadows, false, renderer.format, crate::enhanced::AUX);
            grass.wait();
            assert!(grass.is_ready());
        }
        for shadows in [false, true] {
            let grass = super::Compiled::new(&renderer.device, &renderer.g_layout, &renderer.tex_layout, &renderer.holo_layout,
                1, (false, false, false, false, false, false), shadows, true, renderer.format, crate::enhanced::AUX);
            grass.wait();
            assert!(grass.is_ready(), "admitted grass receiver must validate against its actual bindings");
        }
    }
}

#[cfg(test)]
mod green_ground_admission_tests {
    use ao_scene::ground_detail::{GroundCell, GroundPatch};
    use ao_scene::{Instance, Mesh, Scene, Submesh, Texture, TextureKey, TextureSource, Vertex, IDENTITY};

    #[test]
    fn grass_covers_every_green_tile_and_only_those() {
        // Separate fine vertices and a coarse lattice just like native terrain.
        // Cell (0, 0) is soil; all other tiles are green at either LOD.
        let key = |id| TextureKey { rdb_type: 1_010_006, id };
        let picture = |rgb: [u8; 3]| Texture { width: 8, height: 8, rgba: [rgb[0], rgb[1], rgb[2], 255].repeat(64), source: TextureSource::GroundRgb };
        let mut ground = Mesh { vertices: vec![], submeshes: vec![] };
        let mut fine_cells = Vec::new();
        for z in 0..2 {
            for x in 0..2 {
                let first = ground.vertices.len() as u32;
                for [dx, dz] in [[0, 0], [1, 0], [0, 1], [1, 1]] {
                    ground.vertices.push(Vertex {
                        pos: [(x + dx) as f32 * 2.0, 0.0, -((z + dz) as f32) * 2.0],
                        normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4],
                    });
                }
                let soil = x == 0 && z == 0;
                fine_cells.push(GroundCell {
                    submesh: ground.submeshes.len(), vertices: [first, first + 1, first + 2, first + 3],
                    native_tile: if soil { 3 } else { 2 }, ..Default::default()
                });
                ground.submeshes.push(Submesh::new(vec![first, first + 1, first + 2, first + 1, first + 3, first + 2], Some(key(if soil { 2 } else { 1 }))));
            }
        }
        let lattice_start = ground.vertices.len() as u32;
        for z in 0..=2 {
            for x in 0..=2 {
                ground.vertices.push(Vertex {
                    pos: [x as f32 * 2.0, 0.0, -(z as f32) * 2.0],
                    normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4],
                });
            }
        }
        let coarse_submesh = ground.submeshes.len();
        ground.submeshes.push(Submesh::new(vec![lattice_start, lattice_start + 2, lattice_start + 6,
            lattice_start + 2, lattice_start + 8, lattice_start + 6], None));
        let mut scene = Scene {
            meshes: vec![ground],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }],
            ground_detail: vec![GroundPatch {
                mesh: 0, cells: 2, cell_size: 2.0, height_scale: 1.0, heights: vec![0; 9],
                lattice_start, coarse_submesh, fine_cells,
                ..Default::default()
            }],
            ..Default::default()
        };
        scene.textures.insert(key(1), picture([60, 104, 40]));
        scene.textures.insert(key(2), picture([150, 110, 70]));
        // Floor occupancy admits all green cells, but only one has roof-tested open sky.
        for vertex in &mut scene.meshes[0].vertices[4..8] { vertex.color[3] = 0.0; }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = crate::Renderer::new(&instance, None).expect("native grass admission GPU");
        renderer.set_msaa_samples(1);
        renderer.upload(&scene);
        renderer.set_enhanced_settings(crate::EnhancedSettings {
            ground_grass: true, grass_density: 1.0, grass_distance: 60.0, ..Default::default()
        });
        renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
            generation: 1,
            // All four flat cells are exposed; only the tile's colour may reject one.
            query: std::rc::Rc::new(|_| true),
            line_of_sight: std::rc::Rc::new(|_, _| true),
        }));
        let camera = crate::Camera::look_at(
            super::Vec3::new(2.0, 1.5, 3.0), super::Vec3::new(2.0, 0.2, -2.0),
        );
        renderer.warm_grass_tiles(&camera);
        let grass = renderer.grass.as_ref().expect("enabled grass");
        assert!(grass.candidates.iter().all(|candidate| candidate.grid != [0, 0]),
            "a soil tile must be rejected before candidate exposure");
        let grids: std::collections::BTreeSet<_> = grass.cells.iter()
            .map(|cell| [(cell.origin[0] / cell.origin[2]) as i32,
                (-cell.origin[1] / cell.origin[2]) as i32])
            .collect();
        assert_eq!(grids, [[0, 1], [1, 0], [1, 1]].into_iter().collect(),
            "the soil cell stays absent while every green neighbour generates");
        assert_eq!(grass.cells.len(), grids.len(), "native cells must not generate duplicate grass");
        let check_sky = |cells: &[super::Cell]| {
            for cell in cells {
                let expected = if cell.origin[0] == 2.0 && cell.origin[1] == 0.0 { 1.0 } else { 0.0 };
                assert!(cell.normals.iter().all(|normal| normal[3] == expected),
                    "floor occupancy must never stand in for baked terrain sky proof");
            }
        };
        check_sky(&grass.cells);
        let bare = |grid: [i32; 2]| grass.cells.iter().find(|cell| [(cell.origin[0] / cell.origin[2]) as i32, (-cell.origin[1] / cell.origin[2]) as i32] == grid).map(|cell| cell.identity[3] >> 8);
        assert_eq!((bare([1, 0]), bare([0, 1]), bare([1, 1])), (Some(0b0000_0001), Some(0b0000_0100), Some(0b0001_0000)),
            "each green cell records which side or corner touches the soil tile (west, north, north-west)");
        let coarse_camera = crate::Camera::look_at(
            super::Vec3::new(50.0, 1.5, -2.0), super::Vec3::new(2.0, 0.2, -2.0),
        );
        renderer.warm_grass_tiles(&coarse_camera);
        let ground = renderer.ground.as_ref().unwrap();
        assert!(ground.indices[0][..coarse_submesh].iter().all(Vec::is_empty),
            "camera outside the near radius must select fully coarse terrain");
        assert!(!ground.indices[0][coarse_submesh].is_empty());
        let coarse_grass = renderer.grass.as_ref().unwrap();
        let coarse_grids = coarse_grass.cells.iter()
            .map(|cell| [(cell.origin[0] / cell.origin[2]) as i32, (-cell.origin[1] / cell.origin[2]) as i32])
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(coarse_grids, grids, "every green tile still generates on fully coarse terrain");
        assert_eq!(coarse_grass.cells.len(), grids.len());
        check_sky(&coarse_grass.cells);

        // Reuse this fixture's GPU setup; water-only flips must not recreate grass.
        let settings = crate::EnhancedSettings { cloud_shadows: true, ..renderer.enhanced_settings() };
        renderer.set_enhanced_settings(settings);
        renderer.ensure_grass();
        let grass = renderer.grass.as_mut().unwrap();
        let mut actor = super::GrassBender { actor: Some(7), position: [0.0; 3] };
        grass.trails.update(&[actor], super::Vec3::ZERO);
        actor.position[0] = 1.0;
        grass.trails.update(&[actor], super::Vec3::ZERO);
        grass.inputs.push(actor);
        grass.benders[0] = actor.position;
        grass.bender_count = 1;
        grass.wind_phase = 0.75;
        grass.filtered_wind = [0.2, 0.3, 0.4];
        grass.exposure_cache.insert([1, 2, 3, 4], None);
        let resources = (grass.cells_buffer.clone(), grass.params_buffer.clone(), grass.visible_buffer.clone(),
            grass.layout.clone(), grass.bind.clone(), grass.noise_view.clone(), grass.noise_sampler.clone(), grass.mask_view.clone());
        let inputs = grass.inputs.clone();
        let mut trails = [[0.0; 4]; super::TRAIL_CELLS];
        grass.trails.write(&mut trails);
        assert!(trails.iter().any(|sample| sample[3] > 0.0));
        for water in [true, false, true, false] {
            let key = renderer.world_key.with_water_cloud(water);
            renderer.request_world(key, settings, false);
            renderer.ensure_grass();
            let grass = renderer.grass.as_ref().unwrap();
            assert_eq!(grass.channels.3, water);
            assert_eq!((&grass.cells_buffer, &grass.params_buffer, &grass.visible_buffer,
                &grass.layout, &grass.bind, &grass.noise_view, &grass.noise_sampler, &grass.mask_view),
                (&resources.0, &resources.1, &resources.2, &resources.3, &resources.4, &resources.5, &resources.6, &resources.7),
                "water flips retain every grass GPU resource and receiver-compatible layout");
            assert_eq!(grass.inputs, inputs);
            assert_eq!((grass.benders[0], grass.bender_count), (actor.position, 1));
            assert_eq!((grass.wind_phase, grass.filtered_wind), (0.75, [0.2, 0.3, 0.4]));
            assert!(grass.exposure_cache.contains_key(&[1, 2, 3, 4]));
            let mut after = [[0.0; 4]; super::TRAIL_CELLS];
            grass.trails.write(&mut after);
            assert_eq!(after, trails, "water flips preserve world-anchored trail compression");
        }
    }
}
