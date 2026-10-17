// Enhanced HDR scene: daylight-sun brightening, highlight tonemap and dithered 8-bit present.
// `expose` is the only stage that can add light, and only where the world pass wrote a daylight,
// unshadowed, unfogged sun weight; `present` returns exactly the native colour wherever that weight is 0.
struct Params {
    control: vec4<f32>, // x: peak sun boost (0 = faithful)
}
@group(0) @binding(0) var source: texture_2d<f32>;
@group(1) @binding(0) var sun: texture_2d<f32>;
@group(1) @binding(1) var<uniform> params: Params;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
// Night, dusk and deep-shade sun shares stay below the ramp, so they are never lifted.
fn boost_at(q: vec2<i32>) -> f32 {
    return params.control.x * smoothstep(0.3, 0.7, textureLoad(sun, q, 0).r);
}
@fragment fn expose(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(p.xy);
    let c = textureLoad(source, q, 0);
    let rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb * (1.0 + boost_at(q)), c.a);
}
// Identity up to a knee, then a cubic that reaches 1.0 exactly at the pixel's own boosted white.
// Monotone for beta <= 0.6 (k = 1 - beta/2 makes the end tangent exactly 3x the chord).
fn tone(x: f32, beta: f32) -> f32 {
    if beta <= 0.0001 { return min(x, 1.0); }
    let top = 1.0 + beta;
    let k = min(0.85, 1.0 - 0.5 * beta);
    if x <= k { return x; }
    if x >= top { return 1.0; }
    let d = top - k;
    let u = (x - k) / d;
    let u2 = u * u;
    let u3 = u2 * u;
    return (2.0 * u3 - 3.0 * u2 + 1.0) * k + (u3 - 2.0 * u2 + u) * d + (3.0 * u2 - 2.0 * u3);
}
// Fixed screen-space interleaved-gradient noise: stable between identical frames. +-0.35 code
// levels never moves an exact 8-bit code (f16 round-trip error is under 0.07 level).
fn dither(p: vec2<f32>) -> f32 {
    return (fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715)))) - 0.5) * 0.7 / 255.0;
}
@fragment fn present(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(p.xy);
    let h = textureLoad(source, q, 0);
    let beta = boost_at(q);
    let mapped = vec3<f32>(tone(h.r, beta), tone(h.g, beta), tone(h.b, beta));
    return vec4<f32>(clamp(max(mapped, vec3<f32>(0.0)) + vec3<f32>(dither(p.xy)), vec3<f32>(0.0), vec3<f32>(1.0)), clamp(h.a, 0.0, 1.0));
}
