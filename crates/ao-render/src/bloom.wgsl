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
fn karis(c: vec4<f32>) -> f32 { return 1.0 / (1.0 + max(c.r, max(c.g, c.b))); }
// First downsample: 36 seeded pixels feed 13 box taps (Jimenez layout), five 2x2 groups, Karis-weighted so a
// lone bright pixel cannot flicker. Output energy is a convex mix of box averages of the source.
@fragment fn seed_down(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let origin = vec2<i32>(p.xy) * 2;
    var s: array<vec4<f32>, 36>;
    for (var y = 0; y < 6; y++) {
        for (var x = 0; x < 6; x++) { s[y * 6 + x] = seed_at(origin + vec2<i32>(x - 2, y - 2)); }
    }
    // Box tap at pixel offset o covers source pixels o..o+1 (offsets -2..2 -> indices 0..4 after the +2 shift).
    var t: array<vec4<f32>, 25>;
    for (var y = 0; y < 5; y++) {
        for (var x = 0; x < 5; x++) {
            t[y * 5 + x] = (s[y * 6 + x] + s[y * 6 + x + 1] + s[(y + 1) * 6 + x] + s[(y + 1) * 6 + x + 1]) * 0.25;
        }
    }
    // t index (x,y) is offset (x-2, y-2). Inner diamond j,k,l,m at (+-1,+-1), outer a..i at (0|+-2).
    let a = t[0]; let b = t[2]; let c = t[4]; let d = t[10]; let e = t[12]; let f = t[14]; let g = t[20]; let h = t[22]; let i = t[24];
    let j = t[6]; let k = t[8]; let l = t[16]; let m = t[18];
    let b0 = (j + k + l + m) * 0.25; let b1 = (a + b + d + e) * 0.25; let b2 = (b + c + e + f) * 0.25;
    let b3 = (d + e + g + h) * 0.25; let b4 = (e + f + h + i) * 0.25;
    let w0 = karis(b0) * 0.5; let w1 = karis(b1) * 0.125; let w2 = karis(b2) * 0.125;
    let w3 = karis(b3) * 0.125; let w4 = karis(b4) * 0.125;
    return max((b0 * w0 + b1 * w1 + b2 * w2 + b3 * w3 + b4 * w4) / (w0 + w1 + w2 + w3 + w4), vec4<f32>(0.0));
}
@fragment fn down(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    let origin = vec2<i32>(p.xy) * 2;
    // Fused tent then 2x2 box: [1,3,3,1]/8. Sixteen unique taps, zero border.
    for (var y = -1; y <= 2; y++) {
        for (var x = -1; x <= 2; x++) {
            let weight = select(3.0, 1.0, x == -1 || x == 2) * select(3.0, 1.0, y == -1 || y == 2) / 64.0;
            sum += glare_load(fine, origin + vec2<i32>(x, y)) * weight;
        }
    }
    return sum;
}
// 3x3 tent over bilinear-upsampled coarse light: round, wide falloff instead of nearest-neighbour blocks.
@fragment fn up(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let c = p.xy * 0.5 - 0.5;
    var tent = glare_bilinear(coarse, c) * 4.0;
    tent += (glare_bilinear(coarse, c + vec2<f32>(-1.0, 0.0)) + glare_bilinear(coarse, c + vec2<f32>(1.0, 0.0))
        + glare_bilinear(coarse, c + vec2<f32>(0.0, -1.0)) + glare_bilinear(coarse, c + vec2<f32>(0.0, 1.0))) * 2.0;
    tent += glare_bilinear(coarse, c + vec2<f32>(-1.0, -1.0)) + glare_bilinear(coarse, c + vec2<f32>(1.0, -1.0))
        + glare_bilinear(coarse, c + vec2<f32>(-1.0, 1.0)) + glare_bilinear(coarse, c + vec2<f32>(1.0, 1.0));
    return mix(textureLoad(fine, vec2<i32>(p.xy), 0), tent / 16.0, UP_MIX);
}
// Non-HDR presentation: UNORM-range glare onto the native colour. In HDR the grade `present` pass adds the same
// glare above white instead.
@fragment fn composite(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let raw_beauty = textureLoad(fine, vec2<i32>(p.xy), 0);
    let beauty = vec4<f32>(clamp(raw_beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), raw_beauty.a);
    // Source visibility comes ONLY from the world pass's depth-tested mask. Never classify/gate receivers:
    // doing so could silhouette a dark actor.
    let received = glare_received(glare_reconstruct(coarse, p.xy), params.strength);
    return vec4<f32>(clamp(beauty.rgb + received, vec3<f32>(0.0), vec3<f32>(1.0)), beauty.a);
}
