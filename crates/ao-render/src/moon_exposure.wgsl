// Native baked openness, quantized identically to each ground holo1 R8 texture.
// Header: lattice origin x/z, reciprocal cell size, cell width/height, 3 reserved.
@group(3) @binding(7) var<storage, read> moon_sky_cells: array<u32>;

fn enhanced_sky_open(world: vec3<f32>) -> f32 {
    let size = vec2<u32>(moon_sky_cells[3], moon_sky_cells[4]);
    if any(size == vec2<u32>(0u)) { return 0.0; }
    let origin = vec2<f32>(bitcast<f32>(moon_sky_cells[0]), bitcast<f32>(moon_sky_cells[1]));
    let lattice = vec2<f32>(world.x, -world.z) * bitcast<f32>(moon_sky_cells[2]) - origin;
    // Do not clamp outside the native lattice: unknown locations are indoors.
    if !all(lattice >= vec2<f32>(0.0)) || !all(lattice <= vec2<f32>(size)) { return 0.0; }
    let cell = min(vec2<u32>(lattice), size - vec2<u32>(1u));
    let t = lattice - vec2<f32>(cell);
    let packed = moon_sky_cells[8u + cell.y * size.x + cell.x];
    let corners = vec4<f32>(f32(packed & 255u), f32((packed >> 8u) & 255u),
        f32((packed >> 16u) & 255u), f32((packed >> 24u) & 255u)) / 255.0;
    return mix(mix(corners.x, corners.y, t.x), mix(corners.z, corners.w, t.x), t.y);
}
