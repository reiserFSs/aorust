//! Opt-in, deterministic native-material grass. Blade expansion is entirely GPU-side.
use super::{GroundRun, Vec3};
use crate::gpu_stats::GpuStatsExt;

const MAX_CELLS: usize = 1024;
const BLADES: u32 = 16;
const MAX_RADIUS: f32 = 35.0;
const MAX_BENDERS: usize = 16;
const MAX_EXPOSURE_CELLS: usize = 4096;
const EXPOSURE_CELLS_PER_FRAME: usize = 4;

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
    benders: [[f32; 4]; MAX_BENDERS],
}
struct Batch {
    patch: usize,
    submesh: usize,
    cells: std::ops::Range<u32>,
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
    bind: wgpu::BindGroup,
    cells_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    cells: Vec<Cell>,
    batches: Vec<Batch>,
    fine_mask: [u8; 4096],
    samples: u32,
    channels: (bool, bool),
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
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, material: &wgpu::BindGroupLayout, palette: &wgpu::BindGroupLayout, samples: u32, channels: (bool, bool), format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass cells and current visible benders"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native material grass"), source: wgpu::ShaderSource::Wgsl(shader_source(channels).into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("grass"), bind_group_layouts: &[Some(globals), Some(material), Some(palette), Some(&layout)], immediate_size: 0 });
        let targets = [
            Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.0.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            channels.1.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        let target_count = if channels.0 || channels.1 { 3 } else { 1 };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ground grass"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_grass"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_grass"), compilation_options: Default::default(), targets: &targets[..target_count] }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: super::DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
        });
        let cells_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass cell cache"), size: (MAX_CELLS * std::mem::size_of::<Cell>()) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let params_buffer = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("grass wind and benders"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("grass"), layout: &layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: cells_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: params_buffer.as_entire_binding() },
        ] });
        Self { pipeline, bind, cells_buffer, params_buffer, cells: Vec::with_capacity(MAX_CELLS), batches: Vec::with_capacity(MAX_CELLS), fine_mask: [0; 4096], samples, channels, wind: [0.0; 3], benders: [[0.0; 3]; MAX_BENDERS], bender_count: 0, inputs: Vec::with_capacity(MAX_BENDERS), prepare_cpu_ns: 0, exposure: None, exposure_cache: std::collections::HashMap::with_capacity(MAX_EXPOSURE_CELLS * 2), exposure_order: std::collections::VecDeque::with_capacity(MAX_EXPOSURE_CELLS), candidates: Vec::with_capacity(MAX_CELLS), order: Vec::with_capacity(MAX_CELLS), last_selection: None, last_radius: 0, last_path: super::GroundPath::Simple, pending: false }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, queue: &wgpu::Queue, ground: &GroundRun, liquids: &[super::native_liquid::LiquidRun], camera: Vec3, settings: super::EnhancedSettings, time: f32, path: super::GroundPath) {
        let size = ground.detail.patches().first().map_or(1.0, |patch| patch.cell_size);
        let radius = settings.grass_distance.clamp(0.0, MAX_RADIUS)
            .min(ground.detail.fine_radius()).min(((MAX_CELLS as f32).sqrt() * 0.5 - 3.5).max(0.0) * size);
        if radius <= 0.0 { self.cells.clear(); self.batches.clear(); return; }
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
                if !(patch.height_scale > 0.0 && patch.cell_size > 0.0) || patch.fine_cells.len() > self.fine_mask.len() { continue; }
                let x0 = patch.origin[0] as f32 * patch.cell_size;
                let z0 = -(patch.origin[1] as f32) * patch.cell_size;
                let side = patch.cells as f32 * patch.cell_size;
                if center[0] < x0 - padded || center[0] > x0 + side + padded || center[1] < z0 - side - padded || center[1] > z0 + padded { continue; }
                let Some(materials) = ground.indices.get(patch_index) else { continue; };
                self.fine_mask.fill(0);
                for indices in materials {
                    for &vertex in indices {
                        if vertex < patch.lattice_start {
                            if let Some(mask) = self.fine_mask.get_mut(vertex as usize / 4) { *mask |= 1 << (vertex % 4); }
                        }
                    }
                }
                // One pass over cells, not one full scan per material.
                for (index, cell) in patch.fine_cells.iter().enumerate() {
                    if self.fine_mask[index] != 15 || !eligible(cell.native_tile, cell.ground_material) || self.candidates.len() == MAX_CELLS { continue; }
                    let (x, z) = (index % patch.cells, index / patch.cells);
                    let world_x = x0 + x as f32 * patch.cell_size;
                    let world_z = z0 - z as f32 * patch.cell_size;
                    let dx = world_x + patch.cell_size * 0.5 - center[0];
                    let dz = world_z - patch.cell_size * 0.5 - center[1];
                    if dx * dx + dz * dz > padded * padded { continue; }
                    let offset = z * (patch.cells + 1) + x;
                    let corners = [offset, offset + 1, offset + patch.cells + 1, offset + patch.cells + 2];
                    if corners.iter().any(|&i| i >= patch.heights.len()) { continue; }
                    let heights = corners.map(|i| patch.heights[i] as f32 * (patch.height_scale / 256.0));
                    let lowest = heights.into_iter().fold(f32::INFINITY, f32::min);
                    if heights.into_iter().fold(f32::NEG_INFINITY, f32::max) - lowest > patch.cell_size * 0.8 { continue; }
                    let gx = patch.origin[0].wrapping_add(x as i32);
                    let gz = patch.origin[1].wrapping_add(z as i32);
                    let path_code = if !ground.has_shadow[patch.mesh] || path == super::GroundPath::Simple { 0.0 }
                        else if path == super::GroundPath::ShadowsTwoPass { 2.0 } else { 1.0 };
                    self.candidates.push(Candidate { patch: patch_index, submesh: cell.submesh, grid: [gx, gz], distance: dx * dx + dz * dz,
                        cell: Cell { origin: [world_x, world_z, patch.cell_size, path_code], heights,
                            identity: [cell_seed(gx as u32, gz as u32, cell.native_tile), u32::from(cell.orientation), (!(gz as u32) ^ gx as u32) & 1, 0], normals: [[0.0; 4]; 4] } });
                }
            }
            self.candidates.sort_unstable_by(|a, b| a.distance.total_cmp(&b.distance).then_with(|| a.patch.cmp(&b.patch)).then_with(|| a.grid.cmp(&b.grid)));
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
        let mut params = Params { wind_time: [self.wind[0], self.wind[2], time, radius], limits: [settings.grass_density.clamp(0.0, 2.0) * 0.5, 0.0, self.bender_count as f32, 0.0], benders: [[0.0; 4]; MAX_BENDERS] };
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
        let lowest = cell.heights.into_iter().fold(f32::INFINITY, f32::min);
        let submerged = liquids.iter().any(|liquid| liquid.source.overlaps_ground_cell(cell.origin[0], cell.origin[1], cell.origin[2], lowest));
        let exposed = !submerged && [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 0.5]].into_iter().all(|uv| {
            (exposure.query)([cell.origin[0] + uv[0] * cell.origin[2], cell_height(cell, uv), cell.origin[1] - uv[1] * cell.origin[2]])
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
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup, ground: &GroundRun, gpu: &super::Gpu) -> usize {
        if self.batches.is_empty() { return 0; }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(3, &self.bind, &[]);
        let mut draws = 0;
        for batch in &self.batches {
            let patch = &ground.detail.patches()[batch.patch];
            let Some(palette) = &ground.materials[patch.mesh] else { continue; };
            let Some(draw) = gpu.opaque.get(ground.draws[batch.patch][batch.submesh]) else { continue; };
            pass.set_bind_group(1, &gpu.mats[draw.mat], &[]);
            pass.set_bind_group(2, palette, &[]);
            pass.draw(0..6, batch.cells.start * BLADES..batch.cells.end * BLADES);
            draws += 1;
        }
        draws
    }
    pub fn resource_bytes(&self) -> usize { MAX_CELLS * std::mem::size_of::<Cell>() + std::mem::size_of::<Params>() }
}
fn eligible(tile: u16, material: u8) -> bool { matches!((tile, material), (2 | 258, 15) | (291 | 685, 6)) }
fn hash(mut value: u32) -> u32 { value ^= value >> 16; value = value.wrapping_mul(0x7feb352d); value ^= value >> 15; value = value.wrapping_mul(0x846ca68b); value ^ (value >> 16) }
fn cell_seed(x: u32, z: u32, tile: u16) -> u32 { hash(hash(x) ^ hash(z.rotate_left(16)) ^ u32::from(tile)) }

fn shader_source(channels: (bool, bool)) -> String {
    let grass = include_str!("grass.wgsl")
        .replace("//__AMBIENT_FIELD__", if channels.0 { "@location(1) ambient: vec4<f32>," } else { "" })
        .replace("//__EMISSIVE_FIELD__", if channels.1 { "@location(2) emissive: vec4<f32>," } else { "" })
        .replace("//__AMBIENT_WRITE__", if channels.0 { "output.ambient = vec4<f32>(0.0, 0.0, 0.0, 1.0);" } else { "" })
        .replace("//__EMISSIVE_WRITE__", if channels.1 { "output.emissive = vec4<f32>(0.0, 0.0, 0.0, 1.0);" } else { "" });
    format!("{}\n{grass}", include_str!("shader.wgsl"))
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

/// Matches ao-formats/terrain.rs's central differences, including global edges.
/// Computed once with tile exposure, never in the steady-state CPU prepare loop.
fn terrain_normals(ground: &GroundRun, grid: [i32; 2], size: f32) -> Option<[[f32; 4]; 4]> {
    let mut normals = [[0.0; 4]; 4];
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
        *out = [-dx / length, 1.0 / length, -dz / length, 0.0];
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
        if !settings.ground_grass || settings.grass_density <= 0.0 || settings.grass_distance <= 0.0 || self.ground.is_none() {
            self.grass = None;
        } else if self.grass.as_ref().is_none_or(|grass| grass.samples != self.msaa_samples() || grass.channels != (settings.ambient_occlusion, settings.emissive_bloom)) {
            self.grass = Some(Grass::new(&self.device, &self.g_layout, &self.tex_layout, &self.holo_layout, self.msaa_samples(), (settings.ambient_occlusion, settings.emissive_bloom), self.format));
        }
    }
    /// Current-frame opaque character candidates. Admission, LOS and nearest-source
    /// capping use the renderer's final camera, after body preparation.
    /// Reuse the caller's allocation; no candidate copies or persistent trail state.
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
    pub(super) fn prepare_enhanced_grass(&mut self, cam: &super::Camera, vp: super::Mat4) {
        self.ensure_grass();
        if self.grass.is_none() { return; }
        let started = std::time::Instant::now();
        let settings = self.enhanced_settings();
        if let (Some(grass), Some(ground)) = (&mut self.grass, &self.ground) {
            let (selected, count) = select_benders(
                &mut grass.inputs, cam.pos, vp, self.act.enhanced_grass_actor_ids(),
                grass.exposure.as_ref().map(|surface| surface.line_of_sight.as_ref()),
            );
            grass.benders = selected.map(|bender| bender.position);
            grass.bender_count = count;
            grass.inputs.clear();
            grass.prepare(&self.queue, ground, &self.liquids, cam.pos, settings, self.time, self.ground_state.path());
            grass.prepare_cpu_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        }
    }
    pub(super) fn draw_enhanced_grass(&self, pass: &mut wgpu::RenderPass<'_>) -> usize {
        match (&self.grass, &self.ground) {
            (Some(grass), Some(ground)) => grass.draw(pass, &self.globals_bg, ground, &self.gpu),
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
                grass.prepare(&self.queue, ground, &self.liquids, camera.pos, settings, self.time, self.ground_state.path());
                self.queue.submit([]);
                if !grass.pending { break; }
            }
        }
    }
    /// Offline-only material/exposure/frustum rejection evidence, grouped by native tile.
    pub(super) fn grass_diagnostics(&self, camera: &super::Camera, size: (u32, u32)) -> String {
        use std::fmt::Write;
        let (Some(grass), Some(ground)) = (&self.grass, &self.ground) else { return "grass_state=absent\n".into(); };
        let Some(far) = self.lens.far else { return "grass_diagnostics=explicit_lens_required\n".into(); };
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let vp = super::Mat4::perspective_rh(self.lens.vertical_fov(aspect), aspect, self.lens.near, far)
            * super::Mat4::look_to_rh(camera.pos, camera.forward(), camera.up());
        let radius = f32::from_bits(grass.last_radius);
        let mut counts = std::collections::BTreeMap::<(u16, u8), [usize; 8]>::new();
        let mut report = format!("grass_radius={radius} generated_cells={} candidates={} batches={}\n", grass.cells.len(), grass.candidates.len(), grass.batches.len());
        for batch in &grass.batches {
            let patch = &ground.detail.patches()[batch.patch];
            let draw = ground.draws[batch.patch][batch.submesh];
            writeln!(report, "grass_batch patch={} submesh={} cells={:?} palette_present={} draw_index={draw} draw_present={}",
                batch.patch, batch.submesh, batch.cells, ground.materials[patch.mesh].is_some(), self.gpu.opaque.get(draw).is_some()).unwrap();
        }
        for (pi, patch) in ground.detail.patches().iter().enumerate() {
            let mut mask = [0u8; 4096];
            for indices in &ground.indices[pi] {
                for &vertex in indices {
                    if vertex < patch.lattice_start {
                        if let Some(slot) = mask.get_mut(vertex as usize / 4) { *slot |= 1 << (vertex % 4); }
                    }
                }
            }
            for (index, cell) in patch.fine_cells.iter().enumerate() {
                let gx = patch.origin[0] + (index % patch.cells) as i32;
                let gz = patch.origin[1] + (index / patch.cells) as i32;
                let x = gx as f32 * patch.cell_size;
                let z = -(gz as f32) * patch.cell_size;
                let delta = Vec3::new(x + patch.cell_size * 0.5 - camera.pos.x, 0.0, z - patch.cell_size * 0.5 - camera.pos.z);
                if delta.length() > radius + 2.5 * patch.cell_size { continue; }
                let count = counts.entry((cell.native_tile, cell.ground_material)).or_default();
                count[0] += 1;
                if !eligible(cell.native_tile, cell.ground_material) { count[1] += 1; continue; }
                if mask.get(index).copied().unwrap_or(0) != 15 { count[2] += 1; continue; }
                let Some(candidate) = grass.candidates.iter().find(|c| c.patch == pi && c.grid == [gx, gz]) else { count[3] += 1; continue; };
                let c = &candidate.cell;
                let key = [c.origin[0].to_bits(), c.origin[1].to_bits(), c.origin[2].to_bits(), c.identity[0]];
                match grass.exposure_cache.get(&key) {
                    None => { count[4] += 1; }
                    Some(None) => { count[5] += 1; }
                    Some(Some(_)) => {
                        count[6] += 1;
                        let mut visible = 0;
                        for blade in 0..BLADES {
                            let seed = hash(c.identity[0] ^ blade);
                            if (hash(seed ^ 0x993) & 0xffffff) as f32 / 16777216.0 >= self.enhanced_settings().grass_density * 0.5 { continue; }
                            let uv = [(seed & 0xffffff) as f32 / 16777216.0, (hash(seed) & 0xffffff) as f32 / 16777216.0];
                            let root = Vec3::new(x + uv[0] * patch.cell_size, cell_height(c, uv), z - uv[1] * patch.cell_size);
                            let clip = vp * root.extend(1.0);
                            if root.distance(camera.pos) < radius.min(far) && clip.w > 0.0 && clip.z >= 0.0 && clip.z <= clip.w && clip.x.abs() <= clip.w && clip.y.abs() <= clip.w { visible += 1; }
                        }
                        count[7] += visible;
                        writeln!(report, "grass_cell grid={gx},{gz} tile={} material={} origin={:?} heights={:?} root_frustum_blades={visible}", cell.native_tile, cell.ground_material, c.origin, c.heights).unwrap();
                    }
                }
            }
        }
        for ((tile, material), c) in counts {
            writeln!(report, "grass_tile tile={tile} material={material} nearby={} rejected_material={} rejected_fine={} rejected_slope_or_padding={} pending={} rejected_exposure={} generated={} root_frustum_blades={}", c[0],c[1],c[2],c[3],c[4],c[5],c[6],c[7]).unwrap();
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
        let first = run(vec![own, visible, hidden], vec![1, 2]);
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
    fn deterministic_native_material_density_is_bounded() {
        for tile in 0..=u16::MAX {
            assert_eq!(eligible(tile, 15), matches!(tile, 2 | 258));
            assert_eq!(eligible(tile, 6), matches!(tile, 291 | 685));
            assert!(!eligible(tile, 0));
        }
        assert_eq!(hash(1), 0x688990c0);
        let blades = |x, z| (0..BLADES).filter(|&i| hash(hash(cell_seed(x, z, 2) ^ i) ^ 0x993) & 0xffffff < 0x800000).collect::<Vec<_>>();
        assert_eq!(cell_seed(100, 200, 2), 0xf2964aa6);
        assert_eq!(blades(100, 200), [0, 1, 5, 6, 7, 8, 12]);
        assert_ne!(blades(100, 200), blades(101, 200));
        assert!(MAX_CELLS * std::mem::size_of::<Cell>() + std::mem::size_of::<Params>() < 128 * 1024);
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
