//! Opt-in, deterministic native-material grass. Blade expansion is entirely GPU-side.
use super::{GroundRun, Vec3};
use crate::gpu_stats::GpuStatsExt;
#[path = "grass_atlas.rs"]
mod atlas;

const MAX_CELLS: usize = 1024;
const TUFTS_PER_CELL: u32 = 128;
const BLADES_PER_TUFT: u32 = 7;
const INSTANCES_PER_CELL: u32 = TUFTS_PER_CELL * BLADES_PER_TUFT;
const MAX_RADIUS: f32 = 60.0;
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
    bind: Option<wgpu::BindGroup>,
    layout: wgpu::BindGroupLayout,
    shadows: bool,
    cells_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    atlas: wgpu::TextureView,
    atlas_sampler: wgpu::Sampler,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, globals: &wgpu::BindGroupLayout, material: &wgpu::BindGroupLayout, palette: &wgpu::BindGroupLayout, samples: u32, channels: (bool, bool), shadows: bool, format: wgpu::TextureFormat) -> Self {
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
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
        ];
        let target_count = if channels.0 || channels.1 { 3 } else { 1 };
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
        let (atlas, atlas_sampler) = atlas::upload(device, queue);
        let bind = (!shadows).then(|| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("grass"), layout: &layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: cells_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: params_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&atlas) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&atlas_sampler) },
        ] }));
        Self { pipeline, bind, layout, shadows, cells_buffer, params_buffer, atlas, atlas_sampler, cells: Vec::with_capacity(MAX_CELLS), batches: Vec::with_capacity(MAX_CELLS), fine_mask: [0; 4096], samples, channels, wind: [0.0; 3], benders: [[0.0; 3]; MAX_BENDERS], bender_count: 0, inputs: Vec::with_capacity(MAX_BENDERS), prepare_cpu_ns: 0, exposure: None, exposure_cache: std::collections::HashMap::with_capacity(MAX_EXPOSURE_CELLS * 2), exposure_order: std::collections::VecDeque::with_capacity(MAX_EXPOSURE_CELLS), candidates: Vec::with_capacity(MAX_CELLS), order: Vec::with_capacity(MAX_CELLS), last_selection: None, last_radius: 0, last_path: super::GroundPath::Simple, pending: false }
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
                    if self.fine_mask[index] != 15 || cell.authored_vegetation || !eligible(cell.native_tile, cell.ground_material) || self.candidates.len() == MAX_CELLS { continue; }
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
                    let gx = patch.origin[0].wrapping_add(x as i32);
                    let gz = patch.origin[1].wrapping_add(z as i32);
                    let slope = slope_mask(heights, patch.cell_size, (!(gz as u32) ^ gx as u32) & 1);
                    if slope == 0 { continue; }
                    let path_code = if !ground.has_shadow[patch.mesh] || path == super::GroundPath::Simple { 0.0 }
                        else if path == super::GroundPath::ShadowsTwoPass { 2.0 } else { 1.0 };
                    self.candidates.push(Candidate { patch: patch_index, submesh: cell.submesh, grid: [gx, gz], distance: dx * dx + dz * dz,
                        cell: Cell { origin: [world_x, world_z, patch.cell_size, path_code], heights,
                            identity: [cell_seed(gx as u32, gz as u32, cell.native_tile), u32::from(cell.orientation), (!(gz as u32) ^ gx as u32) & 1, slope], normals: [[0.0; 4]; 4] } });
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
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup, ground: &GroundRun, gpu: &super::Gpu, shadow_bind: Option<&wgpu::BindGroup>) -> usize {
        if self.batches.is_empty() { return 0; }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(3, shadow_bind.or(self.bind.as_ref()).expect("grass receiver prepared"), &[]);
        let mut draws = 0;
        for batch in &self.batches {
            let patch = &ground.detail.patches()[batch.patch];
            let Some(palette) = &ground.materials[patch.mesh] else { continue; };
            let Some(draw) = gpu.opaque.get(ground.draws[batch.patch][batch.submesh]) else { continue; };
            pass.set_bind_group(1, &gpu.mats[draw.mat], &[]);
            pass.set_bind_group(2, palette, &[]);
            pass.draw(0..6, batch.cells.start * INSTANCES_PER_CELL..batch.cells.end * INSTANCES_PER_CELL);
            draws += 1;
        }
        draws
    }
    pub fn resource_bytes(&self) -> usize { MAX_CELLS * std::mem::size_of::<Cell>() + std::mem::size_of::<Params>() + (atlas::WIDTH * atlas::HEIGHT * atlas::SPECIES * 4) as usize }
}
fn eligible(tile: u16, material: u8) -> bool { matches!((tile, material), (2 | 258, 15) | (291 | 685, 6)) }
fn slope_mask([a, b, c, d]: [f32; 4], size: f32, parity: u32) -> u32 {
    // About 40 degrees, measured on the two actual native triangles, not the
    // corner height range (which wrongly rejects diagonal 35-degree slopes).
    let gradients = if parity == 0 { [(b - a, c - a), (d - c, d - b)] }
        else { [(b - a, d - b), (d - c, c - a)] };
    let limit = (size * 0.85).powi(2);
    gradients.into_iter().enumerate().fold(0, |mask, (index, (x, z))| mask | (u32::from(x * x + z * z <= limit) << index))
}
fn hash(mut value: u32) -> u32 { value ^= value >> 16; value = value.wrapping_mul(0x7feb352d); value ^= value >> 15; value = value.wrapping_mul(0x846ca68b); value ^ (value >> 16) }
fn cell_seed(x: u32, z: u32, tile: u16) -> u32 { hash(hash(x) ^ hash(z.rotate_left(16)) ^ u32::from(tile)) }

fn shader_source(channels: (bool, bool), shadows: bool) -> String {
    let grass = include_str!("grass.wgsl")
        .replace("//__AMBIENT_FIELD__", if channels.0 { "@location(1) ambient: vec4<f32>," } else { "" })
        .replace("//__EMISSIVE_FIELD__", if channels.1 { "@location(2) emissive: vec4<f32>," } else { "" })
        .replace("//__AMBIENT_WRITE__", if channels.0 { "output.ambient = vec4<f32>(0.0, 0.0, 0.0, 1.0);" } else { "" })
        .replace("//__AO_RECEIVE__", if channels.0 {
            "let indirect = select(i.dlight, max(clamp(i.light + palette, vec3<f32>(0.0), vec3<f32>(1.0)) - i.light, vec3<f32>(0.0)), i.color.r > 0.5);\n    let extinction = (1.0 - fog) * select(1.0, 1.0 - fog, i.color.r > 1.5);\n    let receiving = tile * (light * 0.8 + min(light, indirect) * 0.2) * extinction;\n    output.ambient = vec4<f32>(min(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), max(receiving, vec3<f32>(0.0))), 1.0);"
        } else { "" })
        .replace("//__SHADOW_NORMAL__", if shadows { "let receiver_normal = cross(dpdx(i.n), dpdy(i.n));" } else { "" })
        .replace("//__SHADOW_RECEIVE__", if shadows {
            "let sun_share = max(light - without_sun, vec3<f32>(0.0));\n    if any(sun_share > vec3<f32>(0.0)) {\n        light = max(light - sun_share * (1.0 - sun_visibility(i.n, receiver_normal, i.clip.xy)), vec3<f32>(0.0));\n    }"
        } else { "" })
        .replace("//__EMISSIVE_WRITE__", if channels.1 { "output.emissive = vec4<f32>(0.0);" } else { "" });
    let mut source = format!("{}\nconst GRASS_TUFTS: u32 = {TUFTS_PER_CELL}u;\nconst GRASS_BLADES: u32 = {BLADES_PER_TUFT}u;\n{grass}", include_str!("shader.wgsl"));
    if shadows { crate::enhanced_shader::append_shadow_receiver(&mut source, 2); }
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
            if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
        } else if self.grass.as_ref().is_none_or(|grass| grass.samples != self.msaa_samples() || grass.channels != (settings.ambient_occlusion, settings.emissive_bloom) || grass.shadows != settings.sun_shadows) {
            self.grass = Some(Grass::new(&self.device, &self.queue, &self.g_layout, &self.tex_layout, &self.holo_layout, self.msaa_samples(), (settings.ambient_occlusion, settings.emissive_bloom), settings.sun_shadows, self.format));
            if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
        }
    }
    pub(super) fn prepare_grass_shadow_receiver(&mut self) {
        if let (Some(grass), Some(shadows)) = (&self.grass, &mut self.sun_shadows) {
            if grass.shadows && shadows.grass_receiver.is_none() {
                shadows.grass_receiver = Some(shadows.grass_bind(&self.device, &grass.layout, &grass.cells_buffer, &grass.params_buffer, &grass.atlas, &grass.atlas_sampler));
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
    pub(super) fn prepare_enhanced_grass(&mut self, cam: &super::Camera, vp: super::Mat4, fog_end: f32) {
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
            grass.prepare(&self.queue, ground, &self.liquids, cam.pos, settings, self.time, self.ground_state.path());
            grass.prepare_cpu_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        }
    }
    pub(super) fn draw_enhanced_grass(&self, pass: &mut wgpu::RenderPass<'_>) -> usize {
        match (&self.grass, &self.ground) {
            (Some(grass), Some(ground)) => grass.draw(pass, &self.globals_bg, ground, &self.gpu, self.sun_shadows.as_ref().and_then(|shadows| shadows.grass_receiver.as_ref())),
            _ => 0,
        }
    }
    /// Bounded effect-owned buffers and atlas, excluding shared Enhanced targets.
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
        let mut counts = std::collections::BTreeMap::<(u16, u8), [usize; 10]>::new();
        let mut report = format!("grass_radius={radius} generated_cells={} candidates={} batches={}\n", grass.cells.len(), grass.candidates.len(), grass.batches.len());
        let unique = grass.cells.iter().map(|cell| [cell.origin[0], cell.origin[1], cell.origin[2]].map(f32::to_bits)).collect::<std::collections::BTreeSet<_>>().len();
        writeln!(report, "grass_duplicate_cells={}", grass.cells.len() - unique).unwrap();
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
                if cell.authored_vegetation { count[9] += 1; continue; }
                let offset = (index / patch.cells) * (patch.cells + 1) + index % patch.cells;
                let heights = [offset, offset + 1, offset + patch.cells + 1, offset + patch.cells + 2]
                    .map(|i| patch.heights[i] as f32 * (patch.height_scale / 256.0));
                let slope = slope_mask(heights, patch.cell_size, (!(gz as u32) ^ gx as u32) & 1);
                if slope == 0 {
                    count[3] += 1;
                    writeln!(report, "grass_rejected_slope grid={gx},{gz} tile={} heights={heights:?} allowed_triangles={slope}", cell.native_tile).unwrap();
                    continue;
                }
                let Some(candidate) = grass.candidates.iter().find(|c| c.patch == pi && c.grid == [gx, gz]) else { count[4] += 1; continue; };
                let c = &candidate.cell;
                let key = [c.origin[0].to_bits(), c.origin[1].to_bits(), c.origin[2].to_bits(), c.identity[0]];
                match grass.exposure_cache.get(&key) {
                    None => { count[5] += 1; }
                    Some(None) => { count[6] += 1; }
                    Some(Some(_)) => {
                        count[7] += 1;
                        let mut visible = 0;
                        for tuft in 0..TUFTS_PER_CELL {
                            let seed = hash(c.identity[0] ^ tuft);
                            let uv = [((seed & 0xffffff) as f32 / 16777216.0).clamp(0.0125, 0.9875), ((hash(seed) & 0xffffff) as f32 / 16777216.0).clamp(0.0125, 0.9875)];
                            let root = Vec3::new(x + uv[0] * patch.cell_size, cell_height(c, uv), z - uv[1] * patch.cell_size);
                            let distance = root.distance(camera.pos);
                            let t = ((distance - radius * 0.3) / (radius * 0.7)).clamp(0.0, 1.0);
                            let density = self.enhanced_settings().grass_density * 0.5 * (1.0 - t * t * (3.0 - 2.0 * t));
                            if (hash(seed ^ 0x993) & 0xffffff) as f32 / 16777216.0 >= density { continue; }
                            let first_triangle = if c.identity[2] == 0 { uv[0] + uv[1] <= 1.0 } else { uv[0] >= uv[1] };
                            let triangle = if first_triangle { 1 } else { 2 };
                            if c.identity[3] & triangle == 0 { continue; }
                            let clip = vp * root.extend(1.0);
                            if root.distance(camera.pos) < radius.min(far) && clip.w > 0.0 && clip.z >= 0.0 && clip.z <= clip.w && clip.x.abs() <= clip.w && clip.y.abs() <= clip.w { visible += 1; }
                        }
                        count[8] += visible;
                        writeln!(report, "grass_cell grid={gx},{gz} tile={} material={} origin={:?} heights={:?} allowed_triangles={} root_frustum_tuft_sites={visible}", cell.native_tile, cell.ground_material, c.origin, c.heights, c.identity[3]).unwrap();
                    }
                }
            }
        }
        for ((tile, material), c) in counts {
            writeln!(report, "grass_tile tile={tile} material={material} nearby={} rejected_material={} rejected_fine={} rejected_slope={} rejected_padding={} pending={} rejected_exposure={} generated={} root_frustum_tuft_sites={} rejected_authored_vegetation={}", c[0],c[1],c[2],c[3],c[4],c[5],c[6],c[7],c[8],c[9]).unwrap();
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
    fn deterministic_native_material_density_is_bounded() {
        for tile in 0..=u16::MAX {
            assert_eq!(eligible(tile, 15), matches!(tile, 2 | 258));
            assert_eq!(eligible(tile, 6), matches!(tile, 291 | 685));
            assert!(!eligible(tile, 0));
        }
        assert_eq!(hash(1), 0x688990c0);
        let tufts = |x, z| (0..TUFTS_PER_CELL).filter(|&i| hash(hash(cell_seed(x, z, 2) ^ i) ^ 0x993) & 0xffffff < 0x800000).collect::<Vec<_>>();
        assert_eq!(cell_seed(100, 200, 2), 0xf2964aa6);
        assert_eq!(&tufts(100, 200)[..7], [0, 1, 5, 6, 7, 8, 12]);
        assert_eq!(tufts(100, 200), tufts(100, 200));
        assert_ne!(tufts(100, 200), tufts(101, 200));
        for parity in 0..2 {
            assert_eq!(slope_mask([0.0, 2.0, 2.0, 4.0], 4.0, parity), 3, "diagonal 35-degree grass slopes stay covered");
            assert_eq!(slope_mask([0.0, 0.0, 3.36, 3.36], 4.0, parity), 3);
            assert_eq!(slope_mask([0.0, 0.0, 4.0, 4.0], 4.0, parity), 0, "45-degree slopes exceed the grass bound");
        }
        assert_eq!(slope_mask([0.0, 0.0, 0.0, 8.0], 4.0, 0), 1, "one steep triangle cannot erase its flat neighbor");
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

#[cfg(test)]
mod blade_appearance_tests {
    #[test]
    fn directional_transmission_preserves_native_budget_and_shadow_floor() {
        let grass = include_str!("grass.wgsl");
        assert!(grass.contains("max(dot(normalize(i.n - g.eye.xyz), g.sun_dir.xyz), 0.0)"));
        assert!(grass.contains("without_sun + max(light - without_sun, vec3<f32>(0.0)) * mix(0.9, 1.0, backlit * backlit)"));
        assert!(grass.find("//__SHADOW_RECEIVE__").unwrap() < grass.find("let backlit =").unwrap());
        assert!(grass.find("fwidth(leaf.a)").unwrap() < grass.find("{ discard; }").unwrap());
        assert!(grass.contains("(g.fog.w <= 0.5 && leaf.a < 0.5) || (g.fog.w > 0.5 && coverage <= 0.0)"));
        assert!(grass.contains("select(1.0, coverage, g.fog.w > 0.5)"));
        for indirect in [0.0_f32, 0.2, 0.7, 1.0] {
            for direct in [0.0_f32, 0.1, 0.3] {
                let native = (indirect + direct).min(1.0);
                for shadow in [0.0_f32, 0.4, 1.0] {
                    let surviving = (native - indirect).max(0.0) * shadow;
                    let front = indirect + surviving * 0.9;
                    for backlit in [0.0_f32, 0.25, 0.5, 1.0] {
                        let transmitted = indirect + surviving * (0.9 + 0.1 * backlit * backlit);
                        assert!(transmitted >= front && transmitted <= native);
                        if direct == 0.0 || shadow == 0.0 { assert_eq!(transmitted, indirect); }
                    }
                }
            }
        }
    }

    #[test]
    fn procedural_blades_preserve_terrain_receivers_and_validate_all_outputs() {
        let grass = include_str!("grass.wgsl");
        assert!(grass.contains("textureSample(grass_atlas, grass_atlas_sampler"));
        assert!(grass.contains("o.tint = vec3<f32>(side * 0.5 + 0.5, 1.0 - along, blade_shape);"));
        assert!(grass.contains("blade == 0u && accent < 3u"));
        assert!(grass.contains("ground_tile(i) * leaf.rgb * jitter"));
        assert!(grass.contains("o.wpos = root;"));
        assert!(grass.contains("o.n = p;"));
        assert!(grass.contains("o.uv = grass_uv(c.identity.y, uv);"));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("grass WGSL requires offscreen GPU");
        for flags in 0..8 {
            let (ao, bloom, shadows) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
            let source = super::shader_source((ao, bloom), shadows);
            assert_eq!(source.contains("output.emissive = vec4<f32>(0.0);"), bloom);
            assert_eq!(source.contains("let receiving = tile *"), ao);
            assert_eq!(source.contains("let receiver_normal = cross(dpdx(i.n), dpdy(i.n));"), shadows);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("procedural grass appearance and receiver regression"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        }
    }
}

#[cfg(test)]
mod authored_vegetation_admission_tests {
    use ao_scene::ground_detail::{GroundCell, GroundPatch};
    use ao_scene::{Instance, Mesh, Scene, Submesh, Vertex, IDENTITY};

    #[test]
    fn authored_vegetation_excludes_only_covered_native_grass_cells() {
        // Native fine admission requires a nonterminal 2x2 patch, with separate
        // fine vertices and a coarse lattice just like the receiver fixture.
        let mut ground = Mesh { vertices: vec![], submeshes: vec![] };
        let mut fine_cells = Vec::new();
        let mut fine_indices = Vec::new();
        for z in 0..2 {
            for x in 0..2 {
                let first = ground.vertices.len() as u32;
                for [dx, dz] in [[0, 0], [1, 0], [0, 1], [1, 1]] {
                    ground.vertices.push(Vertex {
                        pos: [(x + dx) as f32 * 2.0, 0.0, -((z + dz) as f32) * 2.0],
                        normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4],
                    });
                }
                fine_cells.push(GroundCell {
                    submesh: 0, vertices: [first, first + 1, first + 2, first + 3],
                    native_tile: 2, ground_material: 15,
                    authored_vegetation: x == 0 && z == 0,
                    ..Default::default()
                });
                fine_indices.extend([first, first + 1, first + 2, first + 1, first + 3, first + 2]);
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
        ground.submeshes = vec![
            Submesh::new(fine_indices, None),
            Submesh::new(vec![lattice_start, lattice_start + 2, lattice_start + 6,
                lattice_start + 2, lattice_start + 8, lattice_start + 6], None),
        ];
        let scene = Scene {
            meshes: vec![ground],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }],
            ground_detail: vec![GroundPatch {
                mesh: 0, cells: 2, cell_size: 2.0, height_scale: 1.0, heights: vec![0; 9],
                lattice_start, coarse_submesh: 1, fine_cells,
                ..Default::default()
            }],
            ..Default::default()
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = crate::Renderer::new(&instance, None).expect("native grass admission GPU");
        renderer.set_msaa_samples(1);
        renderer.upload(&scene);
        renderer.set_enhanced_settings(crate::EnhancedSettings {
            ground_grass: true, grass_density: 1.0, grass_distance: 30.0, ..Default::default()
        });
        renderer.set_enhanced_grass_exposure(Some(super::GrassExposure {
            generation: 1,
            // All four flat cells are exposed; only authored coverage may reject one.
            query: std::rc::Rc::new(|_| true),
            line_of_sight: std::rc::Rc::new(|_, _| true),
        }));
        let camera = crate::Camera::look_at(
            super::Vec3::new(2.0, 1.5, 3.0), super::Vec3::new(2.0, 0.2, -2.0),
        );
        renderer.warm_grass_tiles(&camera);
        let grass = renderer.grass.as_ref().expect("enabled grass");
        assert!(grass.candidates.iter().all(|candidate| candidate.grid != [0, 0]),
            "authored vegetation must be excluded before candidate exposure");
        let grids: std::collections::BTreeSet<_> = grass.cells.iter()
            .map(|cell| [(cell.origin[0] / cell.origin[2]) as i32,
                (-cell.origin[1] / cell.origin[2]) as i32])
            .collect();
        assert_eq!(grids, [[0, 1], [1, 0], [1, 1]].into_iter().collect(),
            "covered cell must stay absent while every uncovered neighbor generates");
        assert_eq!(grass.cells.len(), grids.len(), "native cells must not generate duplicate grass");
    }
}
