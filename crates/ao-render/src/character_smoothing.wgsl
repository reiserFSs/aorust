// Read fully posed native endpoints: no influence truncation or bind-space skin approximation.
@group(0) @binding(0) var<storage, read> original: array<f32>;
@group(0) @binding(1) var<storage, read> samples: array<vec2<u32>>;
@group(0) @binding(2) var<storage, read_write> vertices: array<f32>;
@group(0) @binding(3) var<uniform> counts: vec4<u32>;

fn normal_at(index: u32) -> vec3<f32> {
    let offset = index * 12u + 3u;
    let n = vec3<f32>(original[offset], original[offset + 1u], original[offset + 2u]);
    let magnitude = length(n);
    if magnitude > 1e-12 { return n / magnitude; }
    return vec3<f32>(0.0);
}

@compute @workgroup_size(64)
fn subdivide_pose(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= counts.x + counts.y { return; }
    let output = index * 12u;
    if index < counts.x {
        for (var lane = 0u; lane < 12u; lane++) { vertices[output + lane] = original[output + lane]; }
        return;
    }
    let edge = samples[index - counts.x];
    let a = edge.x;
    let b = edge.y & 0x7fffffffu;
    let ai = a * 12u;
    let bi = b * 12u;
    for (var lane = 0u; lane < 12u; lane++) {
        vertices[output + lane] = (original[ai + lane] + original[bi + lane]) * 0.5;
    }
    let pa = vec3<f32>(original[ai], original[ai + 1u], original[ai + 2u]);
    let pb = vec3<f32>(original[bi], original[bi + 1u], original[bi + 2u]);
    let na = normal_at(a);
    let nb = normal_at(b);
    var midpoint = (pa + pb) * 0.5;
    if (edge.y & 0x80000000u) == 0u {
        midpoint -= (na * dot(midpoint - pa, na) + nb * dot(midpoint - pb, nb)) * 0.5;
    }
    var normal = na + nb;
    let magnitude = length(normal);
    if magnitude > 1e-12 { normal /= magnitude; } else { normal = vec3<f32>(0.0); }
    vertices[output] = midpoint.x;
    vertices[output + 1u] = midpoint.y;
    vertices[output + 2u] = midpoint.z;
    vertices[output + 3u] = normal.x;
    vertices[output + 4u] = normal.y;
    vertices[output + 5u] = normal.z;
}
