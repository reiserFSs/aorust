// CAT native scalar order: multiply/add separately, never dot/fma/normalize.
struct NativeVertex {
    local0: array<f32, 3>, weight: f32,
    local1: array<f32, 3>, bone0: u32,
    normal: array<f32, 3>, bone1: u32,
}
struct Bone { rows: array<vec4<f32>, 3> }
@group(0) @binding(0) var<storage, read> native: array<NativeVertex>;
@group(0) @binding(1) var<storage, read> bones: array<Bone>;
@group(0) @binding(2) var<storage, read_write> vertices: array<f32>;
@group(0) @binding(3) var<uniform> vertex_count: vec4<u32>;
fn rotate(row: vec4<f32>, v: array<f32, 3>) -> f32 {
    let x = row.x * v[0];
    let y = row.y * v[1];
    let xy = x + y;
    let z = row.z * v[2];
    return xy + z;
}
@compute @workgroup_size(64)
fn skin(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= vertex_count.x { return; }
    let v = native[i];
    let b0 = bones[v.bone0];
    var p: array<f32, 3>;
    var n: array<f32, 3>;
    for (var j = 0u; j < 3u; j++) {
        let p0 = rotate(b0.rows[j], v.local0) + b0.rows[j].w;
        p[j] = p0;
        if !(v.weight >= 0.999) {
            let row = bones[v.bone1].rows[j];
            let p1 = rotate(row, v.local1) + row.w;
            let a = v.weight * p0;
            let b = (1.0 - v.weight) * p1;
            p[j] = a + b;
        }
        n[j] = rotate(b0.rows[j], v.normal);
    }
    let xx = n[0] * n[0];
    let yy = n[1] * n[1];
    let xy = xx + yy;
    let zz = n[2] * n[2];
    let length = sqrt(xy + zz);
    if length > 1e-12 {
        n[0] = n[0] / length;
        n[1] = n[1] / length;
        n[2] = n[2] / length;
    } else { n = array<f32, 3>(0.0, 1.0, 0.0); }
    vertices[i * 12u] = p[0];
    vertices[i * 12u + 1u] = p[1];
    vertices[i * 12u + 2u] = -p[2];
    vertices[i * 12u + 3u] = n[0];
    vertices[i * 12u + 4u] = n[1];
    vertices[i * 12u + 5u] = -n[2];
}
