// Enhanced v2 bloom: 5-level dual filter from half resolution, threshold-free, seeded ONLY by G3 authored emission
// (DESIGN 1.9 / glow rule section 10). `common.wgsl`, `packing.wgsl`, `post_common.wgsl` precede this file.
//
// Seed = authored emission x native fog transmittance, with a soft knee on the brightest channel: a dull self-lit
// surface (the Newland lamp pole, one self-lit material with the head) keeps its native look, a bright emitter glows.
// Resolve highlights, sun glints and "self-lit" flags are never seeds.
@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var g3t: texture_2d<f32>;
@group(0) @binding(2) var deptht: texture_depth_2d;
@group(0) @binding(3) var src: texture_2d<f32>;
@group(0) @binding(4) var src2: texture_2d<f32>;
@group(0) @binding(5) var smp: sampler;
@group(0) @binding(6) var dst: texture_storage_2d<rgba16float, write>;

// Linear emission peak below LO adds nothing, above HI passes unchanged (pole ~0.05 .. lamp head >= 0.5, measured).
const KNEE_LO: f32 = 0.12;
const KNEE_HI: f32 = 0.45;
const SEED_MAX: f32 = 16.0;
// Weight of the wider (lower) levels in the up chain; the chain is an energy-preserving average of the five levels.
const UP_MIX: f32 = 0.6;

fn seed_at(p: vec2<i32>) -> vec3<f32> {
    let q = min(p, vec2<i32>(fc.viewport.xy) - vec2<i32>(1));
    let e = max(textureLoad(g3t, q, 0).rgb, vec3<f32>(0.0));
    let peak = max(e.r, max(e.g, e.b));
    if peak <= 0.0 { return vec3<f32>(0.0); }
    let t = t_native_depth(textureLoad(deptht, q, 0));
    return min(e * (smoothstep(KNEE_LO, KNEE_HI, peak) * t), vec3<f32>(SEED_MAX));
}

@compute @workgroup_size(8, 8)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(dst);
    if id.x >= dims.x || id.y >= dims.y { return; }
    let p = vec2<i32>(id.xy) * 2;
    let s = (seed_at(p) + seed_at(p + vec2<i32>(1, 0)) + seed_at(p + vec2<i32>(0, 1)) + seed_at(p + vec2<i32>(1, 1))) * 0.25;
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(s, 1.0));
}

fn dst_uv(id: vec3<u32>) -> vec2<f32> { return (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(textureDimensions(dst)); }
fn tap(t: texture_2d<f32>, uv: vec2<f32>) -> vec3<f32> { return textureSampleLevel(t, smp, uv, 0.0).rgb; }

// Dual-filter downsample: centre x4 plus the four diagonal bilinear taps.
@compute @workgroup_size(8, 8)
fn down(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(dst);
    if id.x >= dims.x || id.y >= dims.y { return; }
    let uv = dst_uv(id);
    let ts = 1.0 / vec2<f32>(textureDimensions(src));
    let s = tap(src, uv) * 4.0 + tap(src, uv + ts * vec2<f32>(-1.0, -1.0)) + tap(src, uv + ts * vec2<f32>(1.0, -1.0))
        + tap(src, uv + ts * vec2<f32>(-1.0, 1.0)) + tap(src, uv + ts * vec2<f32>(1.0, 1.0));
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(s * 0.125, 1.0));
}

// Dual-filter upsample (8-tap tent) of the lower level `src`, blended with this level's own `src2`.
@compute @workgroup_size(8, 8)
fn up(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(dst);
    if id.x >= dims.x || id.y >= dims.y { return; }
    let uv = dst_uv(id);
    let h = 0.5 / vec2<f32>(textureDimensions(src));
    var s = tap(src, uv + vec2<f32>(-2.0 * h.x, 0.0)) + tap(src, uv + vec2<f32>(2.0 * h.x, 0.0))
        + tap(src, uv + vec2<f32>(0.0, -2.0 * h.y)) + tap(src, uv + vec2<f32>(0.0, 2.0 * h.y));
    s += 2.0 * (tap(src, uv + vec2<f32>(-h.x, h.y)) + tap(src, uv + vec2<f32>(h.x, h.y))
        + tap(src, uv + vec2<f32>(-h.x, -h.y)) + tap(src, uv + vec2<f32>(h.x, -h.y)));
    let own = textureLoad(src2, vec2<i32>(id.xy), 0).rgb;
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(mix(own, s / 12.0, UP_MIX), 1.0));
}
