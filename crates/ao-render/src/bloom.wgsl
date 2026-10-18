// Seeds: authored emission (depth-tested world mask, never inferred from brightness) and, only when the
// scene is HDR, the part of the exposed colour above `THRESHOLD` (daylight surfaces the expose pass lifted).
// Pyramid: Karis 13-tap first downsample, [1,3,3,1] box-tent downsamples, 3x3-tent upsample blended with
// the level's own energy (an energy-preserving mix, so the zero border only ever loses energy).
// Ported in structure from ao-vk rvk/shaders/bloom_down.frag + bloom_up.frag (MIT, see THIRD_PARTY).
struct Params { strength: f32, hdr: f32, _pad0: f32, _pad1: f32 }
@group(0) @binding(0) var fine: texture_2d<f32>;
@group(0) @binding(1) var coarse: texture_2d<f32>;
@group(0) @binding(2) var seed: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: Params;
@group(0) @binding(4) var linear_sampler: sampler;
const THRESHOLD: f32 = 1.0;
// One-sided knee: nothing at/below white, with a short smooth onset for exposed highlights.
const KNEE: f32 = 0.03125;
const HIGHLIGHT_GAIN: f32 = 3.0;
// Share of a level's output taken from the coarser, wider level.
const UP_MIX: f32 = 0.6;
@vertex fn fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
// Seed of one full-resolution pixel `q`: rgb = authored + highlight, a = luma of the highlight.
fn seed_at(q: vec2<i32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(fine));
    if any(q < vec2<i32>(0)) || any(q >= dims) { return vec4<f32>(0.0); }
    let emission = max(textureLoad(fine, q, 0).rgb, vec3<f32>(0.0));
    let colour = max(textureLoad(seed, q, 0).rgb, vec3<f32>(0.0));
    var rgb = min(emission, min(colour, vec3<f32>(1.0)));
    var a = 0.0;
    if params.hdr > 0.5 {
        // Peak-channel extraction keeps coloured sky edges/speculars whose luma is still below white.
        // Native-range night/interior/fog colour cannot cross this threshold; only expose adds HDR light.
        let peak = min(max(colour.r, max(colour.g, colour.b)), 64.0);
        let e = peak - THRESHOLD;
        let excess = HIGHLIGHT_GAIN * select(select(e - 0.5 * KNEE, e * e / (2.0 * KNEE), e < KNEE), 0.0, e <= 0.0);
        let highlight = colour * (excess / max(peak, 1e-5));
        rgb += highlight;
        a = dot(highlight, LUMA);
    }
    return vec4<f32>(rgb, a);
}
// Extract once per source pixel, not nine times in overlapping first-downsample footprints.
@fragment fn seed_extract(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return seed_at(vec2<i32>(p.xy));
}
fn seed_box(c: vec2<f32>) -> vec4<f32> {
    return glare_filtered(fine, linear_sampler, c);
}
fn karis(c: vec4<f32>) -> f32 { return 1.0 / (1.0 + max(c.r, max(c.g, c.b))); }
// Thirteen 2x2 boxes of already extracted seeds, then the original five Karis groups.
// Nonlinear extraction stays BEFORE filtering; the transient seed uses the pyramid's f16 format.
@fragment fn seed_down(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let centre = vec2<f32>(vec2<i32>(p.xy) * 2) + 0.5;
    let a = seed_box(centre + vec2<f32>(-2.0, -2.0));
    let b = seed_box(centre + vec2<f32>(0.0, -2.0));
    let c = seed_box(centre + vec2<f32>(2.0, -2.0));
    let d = seed_box(centre + vec2<f32>(-2.0, 0.0));
    let e = seed_box(centre);
    let f = seed_box(centre + vec2<f32>(2.0, 0.0));
    let g = seed_box(centre + vec2<f32>(-2.0, 2.0));
    let h = seed_box(centre + vec2<f32>(0.0, 2.0));
    let i = seed_box(centre + vec2<f32>(2.0, 2.0));
    let j = seed_box(centre + vec2<f32>(-1.0, -1.0));
    let k = seed_box(centre + vec2<f32>(1.0, -1.0));
    let l = seed_box(centre + vec2<f32>(-1.0, 1.0));
    let m = seed_box(centre + vec2<f32>(1.0, 1.0));
    let b0 = (j + k + l + m) * 0.25; let b1 = (a + b + d + e) * 0.25; let b2 = (b + c + e + f) * 0.25;
    let b3 = (d + e + g + h) * 0.25; let b4 = (e + f + h + i) * 0.25;
    let w0 = karis(b0) * 0.5; let w1 = karis(b1) * 0.125; let w2 = karis(b2) * 0.125;
    let w3 = karis(b3) * 0.125; let w4 = karis(b4) * 0.125;
    return max((b0 * w0 + b1 * w1 + b2 * w2 + b3 * w3 + b4 * w4) / (w0 + w1 + w2 + w3 + w4), vec4<f32>(0.0));
}
@fragment fn down(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let origin = vec2<f32>(vec2<i32>(p.xy) * 2);
    // Pair [1,3] and [3,1] with hardware bilinear: exactly [1,3,3,1]/8 per axis.
    return (glare_filtered(fine, linear_sampler, origin + vec2<f32>(-0.25, -0.25))
        + glare_filtered(fine, linear_sampler, origin + vec2<f32>(1.25, -0.25))
        + glare_filtered(fine, linear_sampler, origin + vec2<f32>(-0.25, 1.25))
        + glare_filtered(fine, linear_sampler, origin + vec2<f32>(1.25, 1.25))) * 0.25;
}
// 3x3 tent over bilinear-upsampled coarse light: round, wide falloff instead of nearest-neighbour blocks.
@fragment fn up(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let c = p.xy * 0.5 - 0.5;
    let base = floor(c);
    let f = fract(c);
    // Tent convolved with bilinear gives [1-f, 2-f, 1+f, f]/4 per axis.
    // Pair adjacent weights: four hardware taps instead of nine software bilinears (36 loads).
    let lo_weight = (3.0 - 2.0 * f) * 0.25;
    let hi_weight = (1.0 + 2.0 * f) * 0.25;
    let lo = base - 1.0 + (2.0 - f) / (3.0 - 2.0 * f);
    let hi = base + 1.0 + f / (1.0 + 2.0 * f);
    let tent = glare_filtered(coarse, linear_sampler, lo) * (lo_weight.x * lo_weight.y)
        + glare_filtered(coarse, linear_sampler, vec2<f32>(hi.x, lo.y)) * (hi_weight.x * lo_weight.y)
        + glare_filtered(coarse, linear_sampler, vec2<f32>(lo.x, hi.y)) * (lo_weight.x * hi_weight.y)
        + glare_filtered(coarse, linear_sampler, hi) * (hi_weight.x * hi_weight.y);
    return mix(textureLoad(fine, vec2<i32>(p.xy), 0), tent, UP_MIX);
}
// Non-HDR presentation: UNORM-range glare onto the native colour. In HDR the grade `present` pass adds the same
// glare above white instead.
@fragment fn composite(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let raw_beauty = textureLoad(fine, vec2<i32>(p.xy), 0);
    let beauty = vec4<f32>(clamp(raw_beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), raw_beauty.a);
    // Source visibility comes ONLY from the world pass's depth-tested mask. Never classify/gate receivers:
    // doing so could silhouette a dark actor.
    let received = glare_received(glare_filtered(coarse, linear_sampler, p.xy * 0.5 - 0.5), params.strength);
    return vec4<f32>(clamp(beauty.rgb + received, vec3<f32>(0.0), vec3<f32>(1.0)), beauty.a);
}
