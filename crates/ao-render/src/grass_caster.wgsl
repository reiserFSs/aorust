// Real sun/point blade casters share grass_vertex: placement, native heights,
// green masks, species, camera LOD, wind and admitted interactions stay identical.
// Point-cell compaction and near-light sparse prefixes follow ao-vk grass.cpp
// DrawGrassPointShadow (MIT; see THIRD_PARTY).
struct GrassCasterFrame {
    matrices: array<mat4x4<f32>, 3>,
    splits: vec4<f32>, eye: vec4<f32>, forward: vec4<f32>, control: vec4<u32>,
    moon_direction: vec4<f32>, moon_budget: vec4<f32>,
}
@group(3) @binding(2) var<uniform> grass_caster_frame: GrassCasterFrame;
fn grass_caster_cell_visible(cell: GrassCell) -> bool {
    let half = cell.origin.z * 0.5;
    let low = min(min(cell.heights.x, cell.heights.y), min(cell.heights.z, cell.heights.w));
    let high = max(max(cell.heights.x, cell.heights.y), max(cell.heights.z, cell.heights.w));
    let center = vec4<f32>(cell.origin.x + half, (low + high) * 0.5, cell.origin.y - half, 1.0);
    let extent = vec3<f32>(half + 1.0, (high - low) * 0.5 + 1.0, half + 1.0);
    let rows = transpose(grass_caster_frame.matrices[0]);
    let planes = array<vec4<f32>, 6>(rows[3] + rows[0], rows[3] - rows[0],
        rows[3] + rows[1], rows[3] - rows[1], rows[2], rows[3] - rows[2]);
    for (var index = 0u; index < 6u; index += 1u) {
        if dot(planes[index], center) < -dot(abs(planes[index].xyz), extent) { return false; }
    }
    return true;
}
@vertex fn vs_grass_shadow(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> @builtin(position) vec4<f32> {
    let clipped = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    let point = (grass_caster_frame.control.y & 0x80000000u) != 0u;
    if point && grass_caster_frame.control.z != 0u { return clipped; }
    // Indirect first_instance stays zero: no optional GPU feature is required.
    let shape_vertex = select(vertex, vertex % 6u, point);
    let packed = select(instance, ((vertex / 6u) << 28u) | instance, point);
    let band = packed >> 28u;
    let stride = u32(grass_params.bands[band / 4u][band % 4u]);
    let cell = grass_cells[grass_visible[band * GRASS_MAX_CELLS + (packed & 0x0fffffffu) / stride]];
    // Reject other cube faces before the expensive mask/noise/shape evaluation.
    if !grass_caster_cell_visible(cell) { return clipped; }
    let blade = grass_vertex(shape_vertex, packed);
    if all(blade.clip == clipped) { return clipped; }
    return grass_caster_frame.matrices[0] * vec4<f32>(blade.world, 1.0);
}
