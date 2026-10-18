// Sequential selection runs AFTER current-frame owner visibility. A hidden
// source cannot consume a top-64 slot, influence grid bounds, or evict a lamp.
@group(0) @binding(0) var<storage, read_write> output: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> candidates: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> owners: array<u32>;

fn admitted(encoded: f32) -> bool {
    if encoded >= 0.0 { return true; }
    let group = u32(floor(abs(encoded) / 32.0)) - 1u;
    return group > 0u && group < arrayLength(&owners) && owners[group] != 0u;
}

// Native eligibility uses rounded f32 subtraction and length, not an exact
// sphere: e.g. (-128, range 128) accepts x=1e-6. Sixteen relative epsilon
// units cover subtraction/norm and grid-coordinate rounding; a unit scale
// floor also covers squared distances flushed below the smallest normal f32.
// This expands candidates only; receivers still use the original exact range.
fn conservative_radius(light: vec4<f32>, grid_scale: f32) -> f32 {
    let scale = max(1.0, max(max(max(abs(light.x), abs(light.y)), abs(light.z)), max(light.w, grid_scale)));
    return light.w + scale * 0.0000019073486328125;
}

@compute @workgroup_size(1)
fn build() {
    output[64u] = vec4<f32>(0.0);
    output[65u] = vec4<f32>(0.0);
    output[322u] = vec4<f32>(0.0);
    var selected: array<u32, 64>;
    var scores: array<f32, 64>;
    var count = 0u;
    let total = bitcast<u32>(candidates[0u].x);
    let eye = candidates[1u].xyz;
    for (var source = 0u; source < total; source += 1u) {
        let row = 2u + source * 5u;
        if !admitted(candidates[row + 3u].w) { continue; }
        let delta = candidates[row].xyz - eye;
        let score = dot(delta, delta);
        var at = 0u;
        loop {
            if at >= count { break; }
            if score < scores[at] { break; }
            at += 1u;
        }
        if at >= 64u { continue; }
        var shift_index = min(count, 63u);
        loop {
            if shift_index <= at { break; }
            selected[shift_index] = selected[shift_index - 1u];
            scores[shift_index] = scores[shift_index - 1u];
            shift_index -= 1u;
        }
        selected[at] = source;
        scores[at] = score;
        count = min(count + 1u, 64u);
    }
    output[322u] = bitcast<vec4<f32>>(vec4<u32>(count, 0u, 0u, 0u));
    if count == 0u { return; }
    var lo = vec3<f32>(3.402823e38);
    var hi = vec3<f32>(-3.402823e38);
    for (var k = 0u; k < count; k += 1u) {
        let source = 2u + selected[k] * 5u;
        let destination = 66u + k * 4u;
        for (var row = 0u; row < 4u; row += 1u) { output[destination + row] = candidates[source + row]; }
        output[323u + k] = candidates[source + 4u];
        let light = candidates[source];
        let radius = conservative_radius(light, 0.0);
        lo = min(lo, light.xyz - radius);
        hi = max(hi, light.xyz + radius);
    }
    // The helper reconstructs upper = lo + dims * cell_size. Reserve global
    // rounding margin too: a distant negative origin can cancel a small hi.
    let bounds_scale = max(max(max(abs(lo.x), abs(lo.y)), abs(lo.z)),
        max(max(max(abs(hi.x), abs(hi.y)), abs(hi.z)), max(max(hi.x - lo.x, hi.y - lo.y), hi.z - lo.z)));
    let bounds_margin = max(1.0, bounds_scale) * 0.0000019073486328125;
    lo -= vec3<f32>(bounds_margin);
    hi += vec3<f32>(bounds_margin);
    var cell_size = 8.0;
    var dims = vec3<u32>(1u);
    loop {
        let d = max(ceil((hi - lo) / cell_size), vec3<f32>(1.0));
        if d.x * d.y * d.z <= 512.0 { dims = vec3<u32>(d); break; }
        cell_size *= 2.0;
    }
    let cells_start = 387u;
    let cells_count = dims.x * dims.y * dims.z;
    var indices = cells_start + cells_count;
    output[64u] = vec4<f32>(lo, cell_size);
    output[65u] = bitcast<vec4<f32>>(vec4<u32>(dims, cells_start));
    // Include the global coordinate scale because cell construction/query
    // arithmetic may cancel a distant grid origin near a small local light.
    let grid_scale = max(max(max(abs(lo.x), abs(lo.y)), abs(lo.z)),
        max(max(max(abs(hi.x), abs(hi.y)), abs(hi.z)), cell_size * f32(max(max(dims.x, dims.y), dims.z))));
    for (var z = 0u; z < dims.z; z += 1u) {
        for (var y = 0u; y < dims.y; y += 1u) {
            for (var x = 0u; x < dims.x; x += 1u) {
                let cell = (z * dims.y + y) * dims.x + x;
                let cmin = lo + vec3<f32>(f32(x), f32(y), f32(z)) * cell_size;
                let first = indices;
                for (var k = 0u; k < count; k += 1u) {
                    let light = output[66u + k * 4u];
                    let delta = light.xyz - clamp(light.xyz, cmin, cmin + cell_size);
                    let radius = conservative_radius(light, grid_scale);
                    if dot(delta, delta) <= radius * radius {
                        output[indices] = bitcast<vec4<f32>>(vec4<u32>(k, 0u, 0u, 0u));
                        indices += 1u;
                    }
                }
                output[cells_start + cell] = bitcast<vec4<f32>>(vec4<u32>(first, indices - first, 0u, 0u));
            }
        }
    }
}
