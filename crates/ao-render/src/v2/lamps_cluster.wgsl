// View-space light clusters, shared by the build pass (lamps_build.wgsl) and the resolve plugin (lamps.wgsl). `common.wgsl`
// (struct FC) precedes this file and the including module declares `fc`. Mirror: `cluster_*` functions in lamps.rs (tests).
// 16x9 screen tiles x 24 exponential depth slices over [near, min(far, 400 m)]; each cluster lists <= 8 lights.
const CLUSTER_X = 16u;
const CLUSTER_Y = 9u;
const CLUSTER_Z = 24u;
const CLUSTER_COUNT = 3456u;
const CLUSTER_MAX = 8u;
const CLUSTER_STRIDE = 9u; // list word 0 = count, words 1..8 = light indices
const CLUSTER_FAR_CAP = 400.0;

fn cluster_near() -> f32 { return max(fc.proj.x, 0.05); }
fn cluster_far() -> f32 { return max(min(fc.proj.y, CLUSTER_FAR_CAP), cluster_near() * 2.0); }

// Depth of slice boundary k (0..=24).
fn cluster_boundary(k: u32) -> f32 {
    let n = cluster_near();
    return n * pow(cluster_far() / n, f32(k) / f32(CLUSTER_Z));
}

fn cluster_slice(depth: f32) -> u32 {
    let n = cluster_near();
    let s = log(max(depth, n) / n) / log(cluster_far() / n) * f32(CLUSTER_Z);
    return min(u32(max(s, 0.0)), CLUSTER_Z - 1u);
}

// Cluster holding pixel `px` at view depth `depth` (pixel centre decides the tile).
fn cluster_index(px: vec2<u32>, depth: f32) -> u32 {
    let tx = min(u32((f32(px.x) + 0.5) * f32(CLUSTER_X) * fc.viewport.z), CLUSTER_X - 1u);
    let ty = min(u32((f32(px.y) + 0.5) * f32(CLUSTER_Y) * fc.viewport.w), CLUSTER_Y - 1u);
    return (cluster_slice(depth) * CLUSTER_Y + ty) * CLUSTER_X + tx;
}
