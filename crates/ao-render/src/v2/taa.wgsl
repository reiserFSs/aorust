// Enhanced v2 temporal anti-aliasing: Halton-8 jittered input, catmull-rom history, 3x3 variance clip, depth-dilated motion.
// `common.wgsl`, `packing.wgsl`, `post_common.wgsl` precede this file.
//
// Input = resolved HDR + the bloom halo (half-res `halo`, attenuated by the receiving geometry's native fog transmittance, so a
// fully fogged pixel never changes), assembled once per 10x10 workgroup tile. Motion: actors write real per-pixel motion into G2
// (`motion.rs`); everything else is zero there and is reprojected from the depth buffer with the unjittered current/previous
// matrices. A G2 value >= 32 is the "no valid history" sentinel. History is the un-sharpened accumulation (CAS runs in post);
// `ctl.taa.x` discards it (CPU-decided resets).
@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var hdrt: texture_2d<f32>;
@group(0) @binding(2) var deptht: texture_depth_2d;
@group(0) @binding(3) var hist: texture_2d<f32>;
@group(0) @binding(4) var motiont: texture_2d<f32>;
@group(0) @binding(5) var smp: sampler;
@group(0) @binding(6) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var halo: texture_2d<f32>;
@group(0) @binding(8) var<uniform> ctl: Ctl;
@group(0) @binding(9) var g3t: texture_2d<f32>;

// Per-pixel emission mask: a native self-lit pixel (authored G3 emission) keeps its own colour; the bloom halo is a dark-lift that must
// not land on it (the Newland lamp pole, one self-lit material with the head, stays at its native brightness).
fn self_lit(q: vec2<i32>) -> f32 {
    let e = textureLoad(g3t, q, 0).rgb;
    return clamp(max(e.r, max(e.g, e.b)) * 4.0, 0.0, 1.0);
}

const GAMMA: f32 = 1.25;       // variance box half-extent in standard deviations
const ALPHA_STATIC: f32 = 0.1; // history weight 0.9 at rest
const ALPHA_MOVING: f32 = 0.35;
const OUT_MAX: f32 = 16000.0;

// (tone-compressed colour of HDR + halo, hardware depth) of the 10x10 tile around the workgroup.
var<workgroup> tile: array<vec4<f32>, 100>;

fn ndc_uv(clip: vec4<f32>) -> vec2<f32> {
    let n = clip.xy / clip.w;
    return vec2<f32>(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5);
}

// Screen-space motion (uv_current - uv_previous, both unjittered) of a static surface from its depth.
fn static_motion(p: vec2<i32>, d: f32) -> vec2<f32> {
    var cur: vec4<f32>;
    var prev: vec4<f32>;
    let wp = world_pos(vec2<f32>(p), d);
    if d >= 1.0 {
        // Sky: rotation only.
        let dir = vec4<f32>(wp - fc.eye.xyz, 0.0);
        cur = fc.view_proj_unjit * dir;
        prev = fc.prev_view_proj_unjit * dir;
    } else {
        cur = fc.view_proj_unjit * vec4<f32>(wp, 1.0);
        prev = fc.prev_view_proj_unjit * vec4<f32>(wp, 1.0);
    }
    if cur.w <= 1e-6 || prev.w <= 1e-6 { return vec2<f32>(64.0); }
    return ndc_uv(cur) - ndc_uv(prev);
}

// Catmull-rom history fetch in 5 bilinear taps (corner taps dropped, weights renormalised); negative lobes clamp at 0.
fn sample_history(uv: vec2<f32>) -> vec3<f32> {
    let dims = fc.viewport.xy;
    let pos = uv * dims;
    let c = floor(pos - vec2<f32>(0.5)) + vec2<f32>(0.5);
    let f = pos - c;
    let w0 = f * (vec2<f32>(-0.5) + f * (vec2<f32>(1.0) - 0.5 * f));
    let w1 = vec2<f32>(1.0) + f * f * (vec2<f32>(-2.5) + 1.5 * f);
    let w2 = f * (vec2<f32>(0.5) + f * (vec2<f32>(2.0) - 1.5 * f));
    let w3 = f * f * (vec2<f32>(-0.5) + 0.5 * f);
    let w12 = w1 + w2;
    let p0 = (c - vec2<f32>(1.0)) / dims;
    let p3 = (c + vec2<f32>(2.0)) / dims;
    let p12 = (c + w2 / w12) / dims;
    var r = textureSampleLevel(hist, smp, vec2<f32>(p12.x, p0.y), 0.0).rgb * (w12.x * w0.y);
    r += textureSampleLevel(hist, smp, vec2<f32>(p0.x, p12.y), 0.0).rgb * (w0.x * w12.y);
    r += textureSampleLevel(hist, smp, p12, 0.0).rgb * (w12.x * w12.y);
    r += textureSampleLevel(hist, smp, vec2<f32>(p3.x, p12.y), 0.0).rgb * (w3.x * w12.y);
    r += textureSampleLevel(hist, smp, vec2<f32>(p12.x, p3.y), 0.0).rgb * (w12.x * w3.y);
    let wsum = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(r / wsum, vec3<f32>(0.0));
}

fn clip_aabb(lo: vec3<f32>, hi: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let c = 0.5 * (hi + lo);
    let e = 0.5 * (hi - lo) + vec3<f32>(1e-5);
    let v = p - c;
    let a = abs(v / e);
    let m = max(a.x, max(a.y, a.z));
    return select(p, c + v / m, m > 1.0);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    let dims = vec2<i32>(fc.viewport.xy);
    let origin = vec2<i32>(wg.xy) * 8 - vec2<i32>(1);
    for (var i = li; i < 100u; i += 64u) {
        let q = clamp(origin + vec2<i32>(i32(i % 10u), i32(i / 10u)), vec2<i32>(0), dims - vec2<i32>(1));
        let d = textureLoad(deptht, q, 0);
        let uv = (vec2<f32>(q) + vec2<f32>(0.5)) * fc.viewport.zw;
        let c = pow(max(textureLoad(hdrt, q, 0).rgb, vec3<f32>(0.0)), vec3<f32>(2.2)) // gamma-encoded resolve + FX (see resolve.wgsl)
            + textureSampleLevel(halo, smp, uv, 0.0).rgb * (ctl.bloom * t_native_depth(d) * (1.0 - self_lit(q)));
        tile[i] = vec4<f32>(tm(min(c, vec3<f32>(OUT_MAX))), d);
    }
    workgroupBarrier();
    if id.x >= u32(dims.x) || id.y >= u32(dims.y) { return; }
    let px = vec2<i32>(id.xy);
    let centre = vec2<i32>(lid.xy) + vec2<i32>(1);

    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var lo = vec3<f32>(1e9);
    var hi = vec3<f32>(-1e9);
    var near_d = 2.0;
    var near_off = vec2<i32>(0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let s = tile[(centre.y + y) * 10 + centre.x + x];
            m1 += s.rgb;
            m2 += s.rgb * s.rgb;
            lo = min(lo, s.rgb);
            hi = max(hi, s.rgb);
            if s.w < near_d { near_d = s.w; near_off = vec2<i32>(x, y); }
        }
    }
    let mean = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    let blo = max(mean - GAMMA * sigma, lo);
    let bhi = min(mean + GAMMA * sigma, hi);
    let cur_t = tile[centre.y * 10 + centre.x].rgb;
    let cur_lin = tm_inv(cur_t);

    // Depth-dilated motion: the nearest surface of the 3x3 decides (actor G2 motion wins over depth reprojection).
    let near_px = clamp(px + near_off, vec2<i32>(0), dims - vec2<i32>(1));
    var mv = textureLoad(motiont, near_px, 0).xy;
    if mv.x == 0.0 && mv.y == 0.0 { mv = static_motion(near_px, near_d); }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) * fc.viewport.zw;
    let huv = uv - mv;
    let valid = ctl.taa.x == 0u && abs(mv.x) < 32.0 && abs(mv.y) < 32.0
        && all(huv > vec2<f32>(0.0)) && all(huv < vec2<f32>(1.0));
    var out_lin = cur_lin;
    if valid {
        let hist_t = tm(min(sample_history(huv), vec3<f32>(OUT_MAX)));
        let clipped = clip_aabb(blo, bhi, hist_t);
        let speed = length(mv * fc.viewport.xy);
        let reject = clamp(length(hist_t - clipped) * 4.0, 0.0, 1.0);
        let a = clamp(mix(ALPHA_STATIC, ALPHA_MOVING, clamp(speed * 0.25, 0.0, 1.0)) + 0.3 * reject, 0.0, 1.0);
        out_lin = tm_inv(mix(clipped, cur_t, a));
    }
    textureStore(dst, px, vec4<f32>(min(out_lin, vec3<f32>(OUT_MAX)), 1.0));
}
