// Enhanced v2 lamp clusters (slot 4). `common.wgsl` and `lamps_cluster.wgsl` precede this file.
//   prep:  one thread per candidate light: GPU owner revocation (carried lights whose actor is not admitted THIS frame
//          are dropped), view-space centre, cube-atlas slot lookup.
//   build: one thread per cluster: the (<= CLUSTER_MAX) most influential lights whose range sphere overlaps the cluster.
// Light record i = lights[4i..4i+4]: (pos, range), (linear colour, cos(phi/2) | 2 = no cone), (D3D atten0..2 | a0 < 0
// linear, cos(theta/2)), (spot axis, owner interaction slot as u32 bits; 0 = not carried).
@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var<uniform> params: vec4<u32>; // x = light count
@group(0) @binding(2) var<storage, read> lights: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> view_lights: array<vec4<f32>>; // view-space centre, range; w < 0 = revoked
@group(0) @binding(4) var<storage, read_write> status: array<u32>;            // cube slot + 1 (0 = no cube tap)
@group(0) @binding(5) var<storage, read> owner_visibility: array<u32>;        // index = actor interaction slot
@group(0) @binding(6) var<storage, read> cube_meta: array<vec4<f32>>;         // v1 point metadata: slots[16], lights[16], ..
@group(0) @binding(7) var<storage, read_write> clusters: array<u32>;

fn owner_admitted(slot: u32) -> bool {
    if slot == 0u { return true; }
    return slot < arrayLength(&owner_visibility) && owner_visibility[slot] != 0u;
}

@compute @workgroup_size(64)
fn prep(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.x { return; }
    let a = lights[i * 4u];
    let owner = bitcast<u32>(lights[i * 4u + 3u].w);
    if !owner_admitted(owner) {
        view_lights[i] = vec4<f32>(0.0, 0.0, 0.0, -1.0);
        status[i] = 0u;
        return;
    }
    var cube = 0u;
    for (var slot = 0u; slot < 16u; slot++) {
        // The atlas copies the selected light verbatim, so an exact position + range match identifies its cube.
        if cube_meta[slot].x > 0.0 && all(cube_meta[16u + slot] == a) { cube = slot + 1u; break; }
    }
    status[i] = cube;
    view_lights[i] = vec4<f32>((fc.view * vec4<f32>(a.xyz, 1.0)).xyz, a.w);
}

struct Box { lo: vec3<f32>, hi: vec3<f32> }

// View-space AABB of cluster `c`, padded by one pixel laterally and 0.1% in depth (TAA jitter / rounding).
fn cluster_box(c: u32) -> Box {
    let tx = c % CLUSTER_X;
    let ty = (c / CLUSTER_X) % CLUSTER_Y;
    let tz = c / (CLUSTER_X * CLUSTER_Y);
    let x0 = -1.0 + 2.0 * f32(tx) / f32(CLUSTER_X) - 2.0 * fc.viewport.z;
    let x1 = -1.0 + 2.0 * f32(tx + 1u) / f32(CLUSTER_X) + 2.0 * fc.viewport.z;
    let y0 = 1.0 - 2.0 * f32(ty + 1u) / f32(CLUSTER_Y) - 2.0 * fc.viewport.w;
    let y1 = 1.0 - 2.0 * f32(ty) / f32(CLUSTER_Y) + 2.0 * fc.viewport.w;
    let d0 = cluster_boundary(tz) * 0.999;
    let d1 = cluster_boundary(tz + 1u) * 1.001;
    let sx = fc.proj.z * fc.proj.w;
    let sy = fc.proj.z;
    var lo = vec3<f32>(3.0e38);
    var hi = vec3<f32>(-3.0e38);
    for (var k = 0u; k < 8u; k++) {
        let d = select(d0, d1, (k & 4u) != 0u);
        let p = vec3<f32>(select(x0, x1, (k & 1u) != 0u) * sx * d, select(y0, y1, (k & 2u) != 0u) * sy * d, -d);
        lo = min(lo, p);
        hi = max(hi, p);
    }
    return Box(lo, hi);
}

@compute @workgroup_size(64)
fn build(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = id.x;
    if c >= CLUSTER_COUNT { return; }
    let cb = cluster_box(c);
    var best: array<u32, 8>;
    var score: array<f32, 8>;
    var n = 0u;
    for (var i = 0u; i < params.x; i++) {
        let v = view_lights[i];
        if v.w <= 0.0 { continue; }
        let q = clamp(v.xyz, cb.lo, cb.hi) - v.xyz;
        let d2 = dot(q, q);
        if d2 > v.w * v.w { continue; }
        // Influence: colour luminance x reach, falling with the distance to the cluster volume.
        let col = lights[i * 4u + 1u].rgb;
        let s = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722)) * v.w / (d2 + 1.0);
        var at = n;
        loop {
            if at == 0u || score[at - 1u] >= s { break; }
            at--;
        }
        if at >= CLUSTER_MAX { continue; }
        var k = min(n, CLUSTER_MAX - 1u);
        loop {
            if k <= at { break; }
            score[k] = score[k - 1u];
            best[k] = best[k - 1u];
            k--;
        }
        score[at] = s;
        best[at] = i;
        n = min(n + 1u, CLUSTER_MAX);
    }
    clusters[c * CLUSTER_STRIDE] = n;
    for (var k = 0u; k < n; k++) { clusters[c * CLUSTER_STRIDE + 1u + k] = best[k]; }
}

// Empty dispatch carrying a timer begin/end (v2 timers.rs `compute_span`).
@compute @workgroup_size(1)
fn tick() {}
