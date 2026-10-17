//! Opt-in, deterministic native-material grass. Blade expansion is entirely GPU-side (grass.wgsl).
use super::{GroundRun, Vec3};
use crate::gpu_stats::GpuStatsExt;
#[path = "grass_noise.rs"]
mod noise;
#[path = "grass_mask.rs"]
mod mask;
pub(super) use mask::{build as green_masks, GreenMasks};

const MAX_CELLS: usize = 16384;
/// Blades a cell can hold at the top density (2.0); the density knob scales how many of them are drawn.
const MAX_BLADES: f32 = 1536.0;
const MAX_RADIUS: f32 = 180.0;
/// Successive distance bands halve the number of instanced blades.
const BANDS: usize = 8;
const MAX_BENDERS: usize = 16;
const MAX_EXPOSURE_CELLS: usize = 32768;
const EXPOSURE_CELLS_PER_FRAME: usize = 4;
/// Narrowest drawn blade width in pixels; thinner blades are widened and tinted towards the ground.
const MIN_BLADE_PIXELS: f32 = 0.8;

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
    /// Blades instanced per cell in each distance band.
    bands: [[f32; 4]; 2],
    benders: [[f32; 4]; MAX_BENDERS],
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
pub(super) struct Grass {
    pipeline: wgpu::RenderPipeline,
    bind: Option<wgpu::BindGroup>,
    layout: wgpu::BindGroupLayout,
    shadows: bool,
    cells_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    visible_buffer: wgpu::Buffer,
    noise_view: wgpu::TextureView,
    noise_sampler: wgpu::Sampler,
    mask_view: wgpu::TextureView,
    green_bytes: usize,
    cells: Vec<Cell>,
    batches: Vec<Batch>,
    /// This frame's frustum-culled cells (indices into `cells`) per distance band, and the ranges of them each draw covers.
    visible: [Vec<u32>; BANDS],
    draws: Vec<Draw>,
    stride: u32,
    band_strides: [u32; BANDS],
    surface_heights: Vec<f32>,
    surface_parities: Vec<u32>,
    samples: u32,
    channels: (bool, bool, bool),
    wind: [f32; 3],
    benders: [[f32; 3]; MAX_BENDERS],
    bender_count: usize,
    inputs: Vec<GrassBender>,
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
impl Grass {
    #[allow(clippy::too_many_arguments)]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, green: &GreenMasks, globals: &wgpu::BindGroupLayout, material: &wgpu::BindGroupLayout, palette: &wgpu::BindGroupLayout, samples: u32, channels: (bool, bool, bool), shadows: bool, format: wgpu::TextureFormat) -> Self {
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
        ];
        if shadows {
            entries.extend(crate::sun_shadows::receiver_entries().map(|mut entry| { entry.binding += 2; entry }));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass cells, benders and shared sun receiver"), entries: &entries,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native material grass"), source: wgpu::ShaderSource::Wgsl(shader_source(channels, shadows).into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("grass"), bind_group_layouts: &[Some(globals), Some(material), Some(palette), Some(&layout)], immediate_size: 0 });
        let targets = [
            Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.0.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.1.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            // HDR sun mask: blades write the same surviving sun share as the ground.
            channels.2.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        let target_count = if channels.2 { 4 } else if channels.0 || channels.1 { 3 } else { 1 };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ground grass"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_grass"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_grass"), compilation_options: Default::default(), targets: &targets[..target_count] }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: super::DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: samples, alpha_to_coverage_enabled: samples > 1, ..Default::default() }, multiview_mask: None, cache: None,
        });
        let cells_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass cell cache"), size: (MAX_CELLS * std::mem::size_of::<Cell>()) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let params_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass wind and benders"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let visible_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass visible cells"), size: (BANDS * MAX_CELLS * 4) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let (noise_view, noise_sampler) = noise::upload(device, queue);
        let mask_view = mask::upload(device, queue, green);
        let bind = (!shadows).then(|| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("grass"), layout: &layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: cells_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: params_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: visible_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&noise_view) },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&noise_sampler) },
            wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&mask_view) },
        ] }));
        Self { pipeline, bind, layout, shadows, cells_buffer, params_buffer, visible_buffer, noise_view, noise_sampler, mask_view, green_bytes: green.layers.len(), cells: Vec::with_capacity(MAX_CELLS), batches: Vec::with_capacity(MAX_CELLS), visible: std::array::from_fn(|_| Vec::with_capacity(MAX_CELLS)), draws: Vec::new(), stride: 0, band_strides: [0; BANDS], surface_heights: Vec::new(), surface_parities: Vec::new(), samples, channels, wind: [0.0; 3], benders: [[0.0; 3]; MAX_BENDERS], bender_count: 0, inputs: Vec::with_capacity(MAX_BENDERS), prepare_cpu_ns: 0, exposure: None, exposure_cache: std::collections::HashMap::with_capacity(MAX_EXPOSURE_CELLS * 2), exposure_order: std::collections::VecDeque::with_capacity(MAX_EXPOSURE_CELLS), candidates: Vec::with_capacity(MAX_CELLS), order: Vec::with_capacity(MAX_CELLS), last_selection: None, last_radius: 0, last_path: super::GroundPath::Simple, pending: false }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, queue: &wgpu::Queue, ground: &GroundRun, liquids: &[super::native_liquid::LiquidRun], camera: Vec3, settings: super::EnhancedSettings, time: f32, path: super::GroundPath, view: Option<(super::Mat4, f32)>) {
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
        // Wind: the zone's weather wind, or a light breeze where the zone has none.
        let speed = self.wind[0].hypot(self.wind[2]);
        let direction = if speed > 0.05 { [self.wind[0] / speed, self.wind[2] / speed] } else { [0.8, 0.6] };
        let pixels = view.map_or(0.0, |(_, pixels)| pixels);
        let mut params = Params { wind_time: [direction[0], direction[1], time % 3600.0, radius], limits: [self.stride as f32, pixels, MIN_BLADE_PIXELS, self.bender_count as f32],
            wind: [(0.35 + 0.1 * speed).min(2.5), 1.0, 0.0, 0.0], bands: std::array::from_fn(|row| std::array::from_fn(|col| self.band_strides[row * 4 + col] as f32)), benders: [[0.0; 4]; MAX_BENDERS] };
        for (slot, position) in params.benders.iter_mut().zip(&self.benders[..self.bender_count]) { *slot = [position[0], position[1], position[2], 1.0]; }
        self.bender_count = 0;
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
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
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup, ground: &GroundRun, gpu: &super::Gpu, shadow_bind: Option<&wgpu::BindGroup>) -> usize {
        if self.draws.is_empty() { return 0; }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(3, shadow_bind.or(self.bind.as_ref()).expect("grass receiver prepared"), &[]);
        let mut draws = 0;
        for draw in &self.draws {
            let batch = &self.batches[draw.batch];
            let patch = &ground.detail.patches()[batch.patch];
            let Some(palette) = &ground.materials[patch.mesh] else { continue; };
            let Some(gpu_draw) = gpu.opaque.get(ground.draws[batch.patch][batch.submesh]) else { continue; };
            pass.set_bind_group(1, &gpu.mats[gpu_draw.mat], &[]);
            pass.set_bind_group(2, palette, &[]);
            // Near blades have three cross sections; distant blades need only root and tip.
            // The instance index retains the distance band in its top four bits.
            let (band, stride) = ((draw.band as u32) << 28, self.band_strides[draw.band]);
            pass.draw(0..if draw.band >= 2 { 3 } else { 6 }, band + draw.slots.start * stride..band + draw.slots.end * stride);
            draws += 1;
        }
        draws
    }
    pub fn has_draws(&self) -> bool { !self.draws.is_empty() }
    pub fn resource_bytes(&self) -> usize { MAX_CELLS * (std::mem::size_of::<Cell>() + 4 * BANDS) + std::mem::size_of::<Params>() + (noise::SIZE * noise::SIZE * 8) as usize + self.green_bytes }
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

fn shader_source(channels: (bool, bool, bool), shadows: bool) -> String {
    let grass = crate::shader_source::load("grass.wgsl", include_str!("grass.wgsl"))
        .replace("//__AMBIENT_FIELD__", if channels.0 { "@location(1) ambient: vec4<f32>," } else { "" })
        .replace("//__EMISSIVE_FIELD__", if channels.1 { "@location(2) emissive: vec4<f32>," } else { "" })
        .replace("//__SUN_FIELD__", if channels.2 { "@location(3) sun: vec4<f32>," } else { "" })
        .replace("//__AMBIENT_WRITE__", if channels.0 { "output.ambient = vec4<f32>(0.0, 0.0, 0.0, 1.0);" } else { "" })
        // Alpha is AO porosity, not beauty opacity: -16 survives even one of eight covered MSAA samples.
        .replace("//__AO_RECEIVE__", if channels.0 {
            "let indirect = select(i.dlight, max(clamp(i.light + palette, vec3<f32>(0.0), vec3<f32>(1.0)) - i.light, vec3<f32>(0.0)), i.path > 0u);\n    let extinction = (1.0 - fog) * select(1.0, 1.0 - fog, i.path > 1u);\n    let sky_fraction = clamp(indirect / max(light, vec3<f32>(0.001)), vec3<f32>(0.0), vec3<f32>(1.0));\n    let ao_lod = i.coverage.y;\n    let receiving = albedo * light * ao_share(sky_fraction) * extinction * ao_lod;\n    output.ambient = vec4<f32>(min(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), max(receiving, vec3<f32>(0.0))), mix(-16.0, 1.0, ao_lod));"
        } else { "" })
        .replace("//__SHADOW_VISIBILITY__", if shadows { "visibility = grass_sun_visibility(i.root + i.normal.xyz * 0.05, i.normal.xyz);" } else { "" })
        .replace("//__EMISSIVE_WRITE__", if channels.1 { "output.emissive = vec4<f32>(0.0);" } else { "" })
        // Same roof-tested terrain alpha and fog contract as the ground receiver.
        .replace("//__SUN_WRITE__", if channels.2 { "let sun_luma = dot(sun_share, vec3<f32>(0.2126, 0.7152, 0.0722));\n    output.sun = vec4<f32>(sun_luma * grass_direct_visibility * (1.0 - fog), i.normal.w * (1.0 - fog), fog, 1.0);" } else { "" });
    let mut source = format!("{}\nconst GRASS_MAX_CELLS: u32 = {MAX_CELLS}u;\n{grass}", crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl")));
    if channels.0 { crate::enhanced_shader::append_ao_share(&mut source); }
    if shadows {
        crate::enhanced_shader::append_shadow_receiver(&mut source, 2);
        let receiver = crate::shader_source::load("grass_shadow.wgsl", include_str!("grass_shadow.wgsl"));
        source.push_str(&receiver.replace(
            "    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
            "    grass_direct_visibility = visible;\n    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
        ));
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


fn select_benders(
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
    fn ensure_grass(&mut self) {
        let settings = self.enhanced_settings();
        let enabled = settings.ground_grass && settings.grass_density > 0.0 && settings.grass_distance > 0.0;
        let Some(ground) = self.ground.as_ref().filter(|_| enabled) else {
            self.grass = None;
            if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
            return;
        };
        if self.grass.as_ref().is_none_or(|grass| grass.samples != self.msaa_samples() || grass.channels != (settings.ambient_occlusion, settings.emissive_bloom, settings.needs_sun_mask()) || grass.shadows != settings.sun_shadows) {
            self.grass = Some(Grass::new(&self.device, &self.queue, &ground.green, &self.g_layout, &self.tex_layout, &self.holo_layout, self.msaa_samples(), (settings.ambient_occlusion, settings.emissive_bloom, settings.needs_sun_mask()), settings.sun_shadows, self.format));
            if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
        }
    }
    pub(super) fn prepare_grass_shadow_receiver(&mut self) {
        if let (Some(grass), Some(shadows)) = (&self.grass, &mut self.sun_shadows) {
            if grass.shadows && shadows.grass_receiver.is_none() {
                shadows.grass_receiver = Some(shadows.grass_bind(&self.device, &grass.layout, &grass.cells_buffer, &grass.params_buffer, &grass.visible_buffer, &grass.noise_view, &grass.noise_sampler, &grass.mask_view));
            }
        }
    }
    pub(super) fn clear_enhanced_grass_inputs(&mut self) {
        if let Some(grass) = &mut self.grass { grass.inputs.clear(); }
    }
    /// Current-frame opaque character candidates. Admission, LOS and nearest-source
    /// capping use the renderer's final camera, after body preparation.
    /// Reuse the caller's allocation; no candidate copies or persistent trail state.
    /// Set after `set_actors`, which invalidates the previous batch's snapshot.
    /// The snapshot remains valid for repeated draws/screenshots until the next
    /// actor batch replaces it; visibility is still checked again on every render.
    pub fn set_enhanced_grass_inputs(&mut self, benders: &mut Vec<GrassBender>, wind: [f32; 3]) {
        self.ensure_grass();
        if let Some(grass) = &mut self.grass {
            std::mem::swap(&mut grass.inputs, benders);
            grass.inputs.retain(|b| b.position.iter().all(|v| v.is_finite()));
            benders.clear();
            grass.wind = wind.map(|v| if v.is_finite() { v } else { 0.0 });
        } else {
            *benders = Vec::new();
        }
    }
    pub fn set_enhanced_grass_exposure(&mut self, exposure: Option<GrassExposure>) {
        self.ensure_grass();
        if let Some(grass) = &mut self.grass {
            if grass.exposure.as_ref().map(|old| old.generation) != exposure.as_ref().map(|new| new.generation) {
                grass.exposure_cache.clear();
                grass.exposure_order.clear();
                grass.last_selection = None;
                grass.pending = true;
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
                &mut grass.inputs, cam.pos, vp, self.act.enhanced_grass_actor_ids(fog_end),
                grass.exposure.as_ref().map(|surface| surface.line_of_sight.as_ref()),
            );
            grass.benders = selected.map(|bender| bender.position);
            grass.bender_count = count;
            grass.prepare(&self.queue, ground, &self.liquids, cam.pos, settings, self.time, self.ground_state.path(), Some((vp, pixels_per_unit)));
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
    pub fn enhanced_grass_resource_bytes(&self) -> usize { self.grass.as_ref().map_or(0, Grass::resource_bytes) }
    pub fn enhanced_grass_prepare_cpu_ns(&self) -> u64 { self.grass.as_ref().map_or(0, |grass| grass.prepare_cpu_ns) }
    pub fn enhanced_grass_cell_count(&self) -> usize { self.grass.as_ref().map_or(0, |grass| grass.cells.len()) }
    /// Offline fixture preparation only: do not advance native draw-count clocks.
    pub(super) fn warm_grass_tiles(&mut self, camera: &super::Camera) {
        self.update_ground_detail(camera.pos);
        self.ensure_grass();
        let settings = self.enhanced_settings();
        if let (Some(grass), Some(ground)) = (&mut self.grass, &self.ground) {
            for _ in 0..=MAX_CELLS.div_ceil(EXPOSURE_CELLS_PER_FRAME) {
                grass.prepare(&self.queue, ground, &self.liquids, camera.pos, settings, self.time, self.ground_state.path(), None);
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
        assert!(thinning_keep(27.0) > 0.25, "geometry morph must finish before a cell can use three vertices");
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
    #[test]
    fn blades_never_hide_targets_and_every_receiver_variant_builds() {
        let grass = include_str!("grass.wgsl");
        assert!(grass.contains("const GRASS_MAX_HEIGHT: f32 = 0.5;"), "fairness: no blade, flower or seed head above half a unit");
        assert!(grass.contains("height = min(height, GRASS_MAX_HEIGHT);"));
        assert!(grass.contains("o.fog = max(vertex_fog(-(g.view * vec4<f32>(p, 1.0)).z), root_fog);"),
            "fog guard: raised tips cannot reduce native extinction");
        assert!(grass.contains("let sun_share = max(i.light - i.spec, vec3<f32>(0.0));"),
            "grass and native ground must remove direct sun before palette saturation");
        assert!(grass.contains("textureSampleLevel(tex, samp_clamp, i.uv, coarse_level)"),
            "distant grass must use the same 8x8 tile mip as coarse terrain");
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("grass WGSL requires offscreen GPU");
        for flags in 0..16 {
            let (ao, bloom, shadows, hdr) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0);
            let source = super::shader_source((ao, bloom, hdr), shadows);
            assert_eq!(source.contains("output.emissive = vec4<f32>(0.0);"), bloom);
            assert_eq!(source.contains("let receiving = albedo *"), ao);
            assert_eq!(source.contains("fn ao_share("), ao, "grass must reuse the terrain AO budget");
            assert_eq!(source.contains("let ao_lod = i.coverage.y;"), ao,
                "subpixel distant tufts must not retain near-blade self-occlusion");
            assert_eq!(source.contains("mix(-16.0, 1.0, ao_lod)"), ao, "AO-only porosity must survive MSAA resolve");
            assert_eq!(source.contains("output.sun = "), hdr);
            assert_eq!(source.contains("dot(sun_share, vec3<f32>(0.2126, 0.7152, 0.0722))"), hdr,
                "HDR daylight eligibility must use the native terrain's luma weights");
            assert_eq!(source.contains("visibility = grass_sun_visibility("), shadows);
            assert_eq!(source.contains("i.normal.w * (1.0 - fog), fog, 1.0)"), hdr,
                "grass must share terrain sky openness and native fog channels");
            assert_eq!(source.contains("grass_direct_visibility = visible;"), shadows,
                "the HDR sun mask excludes beauty's retained shadow floor");
            // Creating the pipeline validates the shader against the real bind-group layouts.
            let _grass = super::Grass::new(&renderer.device, &renderer.queue, &super::GreenMasks::default(), &renderer.g_layout, &renderer.tex_layout, &renderer.holo_layout, 1, (ao, bloom, hdr), shadows, renderer.format);
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
    }
}
