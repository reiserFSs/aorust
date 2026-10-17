// Screen-space contact shadows, ported from ao-vk (MIT) rvk/shaders/contact.frag + ao_blur.frag: a short half-resolution march
// from each surface point towards the sun through the current frame's resolved depth, a depth-aware blur, and a depth-aware
// upsample that darkens only the sun's direct share (frame.sun.r). Primary-view depth only; no actor lists, no history.
const STEPS = 12;
struct Params {
    inverse_vp: mat4x4<f32>,
    vp: mat4x4<f32>,
    eye: vec4<f32>,
    sun: vec4<f32>,     // world direction towards the sun, ray length (m)
    control: vec4<f32>, // strength, near, far, farthest distance the shadow applies at
}
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var half_input: texture_2d<f32>; // r = occlusion, g = view depth (0 = sky)
@group(0) @binding(3) var sun_mask: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
fn size() -> vec2<i32> { return vec2<i32>(textureDimensions(depth)); }
fn depth_at(pixel: vec2<i32>) -> f32 {
    return textureLoad(depth, clamp(pixel, vec2<i32>(0), size() - 1), 0);
}
fn view_depth(z: f32) -> f32 {
    let n = params.control.y;
    let f = params.control.z;
    return f * n / (f - z * (f - n));
}
fn world(pixel: vec2<i32>, z: f32) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(size());
    let p = params.inverse_vp * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
    return p.xyz / p.w;
}
// Pick an actual primary-view sample of the 2x2 block rather than averaging foreground/background.
fn half_pixel(pixel: vec2<i32>) -> vec2<i32> {
    var selected = pixel * 2;
    var z = depth_at(selected);
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let candidate = pixel * 2 + vec2<i32>(x, y);
            let d = depth_at(candidate);
            if all(candidate < size()) && d < z { selected = candidate; z = d; }
        }
    }
    return selected;
}
fn normal(pixel: vec2<i32>, p: vec3<f32>) -> vec3<f32> {
    let left = p - world(pixel - vec2<i32>(1, 0), depth_at(pixel - vec2<i32>(1, 0)));
    let right = world(pixel + vec2<i32>(1, 0), depth_at(pixel + vec2<i32>(1, 0))) - p;
    let up = p - world(pixel - vec2<i32>(0, 1), depth_at(pixel - vec2<i32>(0, 1)));
    let down = world(pixel + vec2<i32>(0, 1), depth_at(pixel + vec2<i32>(0, 1))) - p;
    let dx = select(right, left, dot(left, left) < dot(right, right));
    let dy = select(down, up, dot(up, up) < dot(down, down));
    let n = normalize(cross(dy, dx) + vec3<f32>(0.0, 0.00001, 0.0));
    return select(n, -n, dot(n, params.eye.xyz - p) < 0.0);
}
@fragment fn estimate(@builtin(position) position: vec4<f32>) -> @location(0) vec2<f32> {
    let pixel = half_pixel(vec2<i32>(position.xy));
    let z = depth_at(pixel);
    if z >= 1.0 { return vec2<f32>(0.0); }
    let p = world(pixel, z);
    let w = view_depth(z);
    if w > params.control.w { return vec2<f32>(0.0, w); }
    let n = normal(pixel, p);
    let toward_sun = params.sun.xyz;
    if dot(n, toward_sun) <= 0.0 { return vec2<f32>(0.0, w); } // faces away: no direct sun anyway
    // Fixed-pattern interleaved gradient noise (no time term): static frames stay identical.
    let noise = fract(52.9829189 * fract(dot(position.xy, vec2<f32>(0.06711056, 0.00583715))));
    let origin = p + n * (0.01 * w + 0.02);
    let dims = vec2<f32>(size());
    var occlusion = 0.0;
    for (var i = 0; i < STEPS; i++) {
        let t = (f32(i) + noise) / f32(STEPS) * params.sun.w;
        let clip = params.vp * vec4<f32>(origin + toward_sun * t, 1.0);
        if clip.w <= params.control.y { break; }
        let ndc = clip.xy / clip.w;
        if any(abs(ndc) >= vec2<f32>(1.0)) { break; }
        let hit = vec2<i32>(vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * dims);
        let d = depth_at(hit);
        if d >= 1.0 { continue; }
        let behind = clip.w - view_depth(d);
        if behind > 0.02 && behind < 0.3 + 0.01 * clip.w {
            occlusion = 1.0 - f32(i) / f32(STEPS); // nearer occluders: darker
            break;
        }
    }
    let fade = 1.0 - smoothstep(0.7 * params.control.w, params.control.w, w);
    return vec2<f32>(occlusion * fade, w);
}
// Depth-aware weight: samples across a depth discontinuity (silhouettes) contribute nothing.
fn similar(centre: f32, other: f32) -> f32 {
    if other <= 0.0 { return 0.0; }
    return max(1.0 - abs(centre - other) / (0.04 * centre + 0.04), 0.0);
}
@fragment fn blur(@builtin(position) position: vec4<f32>) -> @location(0) vec2<f32> {
    let pixel = vec2<i32>(position.xy);
    let centre = textureLoad(half_input, pixel, 0).rg;
    if centre.g <= 0.0 { return centre; }
    var sum = centre.r;
    var weight = 1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let neighbor = pixel + vec2<i32>(x, y);
            if (x == 0 && y == 0) || any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(half_input))) { continue; }
            let sample = textureLoad(half_input, neighbor, 0).rg;
            let wgt = similar(centre.g, sample.g);
            sum += sample.r * wgt;
            weight += wgt;
        }
    }
    return vec2<f32>(sum / weight, centre.g);
}
// Output alpha is the darkening fraction; the pipeline blends colour * (1 - alpha) onto the native colour.
@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let z = depth_at(pixel);
    // Unlike daylight exposure, darkening also applies to weak moon/dusk light.
    // The world mask already excludes unlit surfaces and attenuates native fog and mapped shadows.
    let sun = clamp(textureLoad(sun_mask, pixel, 0).r, 0.0, 1.0);
    if z >= 1.0 || sun <= 0.0 { return vec4<f32>(0.0); }
    let w = view_depth(z);
    let half_coord = (vec2<f32>(pixel) + 0.5) * 0.5 - 0.5;
    let base = vec2<i32>(floor(half_coord));
    let fraction = fract(half_coord);
    var sum = 0.0;
    var weight = 0.0;
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let neighbor = base + vec2<i32>(x, y);
            if any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(half_input))) { continue; }
            let sample = textureLoad(half_input, neighbor, 0).rg;
            let spatial = select(1.0 - fraction.x, fraction.x, x == 1) * select(1.0 - fraction.y, fraction.y, y == 1);
            let wgt = spatial * similar(w, sample.g);
            sum += sample.r * wgt;
            weight += wgt;
        }
    }
    let occlusion = select(0.0, sum / weight, weight > 0.0);
    return vec4<f32>(0.0, 0.0, 0.0, clamp(occlusion * params.control.x * sun, 0.0, 1.0));
}
