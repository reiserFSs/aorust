// Native cell compaction for the current GPU-admitted point-light cube slots.
const MAX_CELLS: u32 = 16384u;
const BANDS: u32 = 8u;
const MAX_LIGHTS: u32 = 16u;

struct GrassCell {
    origin: vec4<f32>,
    heights: vec4<f32>,
    identity: vec4<u32>,
    normals: array<vec4<f32>, 4>,
}
struct GrassPointParams {
    counts: array<vec4<u32>, 2>,
    strides: array<vec4<u32>, 2>,
    control: vec4<u32>, // x: face stride in vec4s; y: admitted cube slots; z: dynamic face base
}
struct GrassPointDraw {
    vertices: u32,
    instances: atomic<u32>,
    first_vertex: u32,
    first_instance: u32,
}

@group(0) @binding(0) var<storage, read> cells: array<GrassCell>;
@group(0) @binding(1) var<storage, read> visible: array<u32>;
@group(0) @binding(2) var<uniform> params: GrassPointParams;
@group(0) @binding(3) var<storage, read> faces: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> selected: array<u32>;
@group(0) @binding(5) var<storage, read_write> draws: array<GrassPointDraw>;

@compute @workgroup_size(64)
fn select_grass_cells(@builtin(global_invocation_id) id: vec3<u32>) {
    let band = id.y;
    let slot = id.z;
    if band >= BANDS || slot >= min(params.control.y, MAX_LIGHTS) { return; }
    let count = min(params.counts[band / 4u][band % 4u], MAX_CELLS);
    if id.x >= count { return; }

    let face_stride = params.control.x;
    if face_stride < 16u { return; }
    // Require the complete dynamic cube, avoiding overflow even for malformed strides.
    let face_count = arrayLength(&faces) / face_stride;
    if params.control.z > face_count || slot >= (face_count - params.control.z) / 6u { return; }
    let face = (params.control.z + slot * 6u) * face_stride;
    // Current grass refreshes all dynamic faces, so the first face proves this selected cube.
    if bitcast<u32>(faces[face + 15u].z) != 0u { return; }
    let light = faces[face + 13u];
    if !(light.w > 0.0) || !all(abs(light) <= vec4<f32>(3.402823e38)) { return; }

    let input = band * MAX_CELLS + id.x;
    if input >= arrayLength(&visible) { return; }
    let cell_id = visible[input];
    if cell_id >= min(arrayLength(&cells), MAX_CELLS) { return; }
    let cell = cells[cell_id];
    let size = cell.origin.z;
    if !(size > 0.0) { return; }
    let min_y = min(min(cell.heights.x, cell.heights.y), min(cell.heights.z, cell.heights.w)) - 0.1;
    let max_y = max(max(cell.heights.x, cell.heights.y), max(cell.heights.z, cell.heights.w)) + 1.0;
    let lower = vec3<f32>(cell.origin.x, min_y, cell.origin.y - size);
    let upper = vec3<f32>(cell.origin.x + size, max_y, cell.origin.y);
    let delta = light.xyz - clamp(light.xyz, lower, upper);
    let distance_squared = dot(delta, delta);
    if !(distance_squared <= light.w * light.w) { return; }

    var target_band = band;
    if distance_squared > 100.0 { target_band = min(band + 2u, BANDS - 1u); }
    let stride = params.strides[target_band / 4u][target_band % 4u];
    // Instance indices reserve their top four bits for the target distance band.
    if stride == 0u || stride > 0x0fffffffu / MAX_CELLS { return; }
    let draw = slot * BANDS + target_band;
    let output = draw * MAX_CELLS;
    if draw >= arrayLength(&draws) || output + MAX_CELLS > arrayLength(&selected) { return; }
    // Visible cell IDs are globally unique across input bands, so no target
    // bucket can exceed MAX_CELLS. Each cell retains its existing R2 rank prefix.
    let instance = atomicAdd(&draws[draw].instances, stride);
    let cell_slot = instance / stride;
    if cell_slot < MAX_CELLS { selected[output + cell_slot] = cell_id; }
}
