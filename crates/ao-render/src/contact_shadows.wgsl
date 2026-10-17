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
@group(0) @binding(2) var half_input: texture_2d<u32>;
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
    let cross_n = cross(dy, dx);
    let n = cross_n / max(length(cross_n), 0.00001);
    return select(n, -n, dot(n, params.eye.xyz - p) < 0.0);
}
// Match AO's packed value/selected-offset and full-precision device depth.
// Two more words cache half-precision normals so filter taps never rescan
// primary depth or reconstruct a normal. Degenerate normals stay degenerate.
fn encoded(value: f32, z: f32, pixel: vec2<i32>, n: vec3<f32>) -> vec4<u32> {
    let offset = u32(pixel.x & 1) | (u32(pixel.y & 1) << 1u);
    return vec4<u32>(pack2x16float(vec2<f32>(value, 0.0)) | (offset << 16u), bitcast<u32>(z),
        pack2x16float(n.xy), pack2x16float(vec2<f32>(n.z, 0.0)));
}
fn value_of(sample: vec4<u32>) -> f32 { return unpack2x16float(sample.x).x; }
fn depth_of(sample: vec4<u32>) -> f32 { return bitcast<f32>(sample.y); }
fn selected_pixel(half: vec2<i32>, sample: vec4<u32>) -> vec2<i32> {
    return half * 2 + vec2<i32>(i32((sample.x >> 16u) & 1u), i32((sample.x >> 17u) & 1u));
}
fn normal_of(sample: vec4<u32>) -> vec3<f32> {
    return vec3<f32>(unpack2x16float(sample.z), unpack2x16float(sample.w).x);
}
fn phase_noise(position: vec2<f32>) -> f32 {
    let tile = vec2<u32>(position) % vec2<u32>(3u);
    return (f32((tile.x * 3u + tile.y * 7u) % 9u) + 0.5) / 9.0;
}
@fragment fn estimate(@builtin(position) position: vec4<f32>) -> @location(0) vec4<u32> {
    let pixel = half_pixel(vec2<i32>(position.xy));
    let z = depth_at(pixel);
    if z >= 1.0 { return encoded(0.0, z, pixel, vec3<f32>(0.0)); }
    let p = world(pixel, z);
    let w = view_depth(z);
    let n = normal(pixel, p);
    if w > params.control.w { return encoded(0.0, z, pixel, n); }
    let toward_sun = params.sun.xyz;
    if dot(n, toward_sun) <= 0.0 { return encoded(0.0, z, pixel, n); } // faces away: no direct sun anyway
    // Nine stable phases repeat every 3x3 half-resolution pixels. Unlike the
    // unbounded gradient noise, the box filter covers a complete phase cycle.
    let noise = phase_noise(position.xy);
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
    return encoded(occlusion * fade, z, pixel, n);
}
// Match the AO symmetric tangent-plane rejection: tangential depth gradients
// on sloping ground must not discard phases, but silhouettes must not mix.
// Keep contact's original view-depth-scaled cutoff, rather than AO's radius cutoff.
fn bilateral(center: vec3<f32>, center_normal: vec3<f32>, candidate: vec3<f32>, candidate_normal: vec3<f32>, cutoff: f32) -> f32 {
    let delta = candidate - center;
    var distance = max(abs(dot(delta, center_normal)), abs(dot(delta, candidate_normal)));
    if dot(center_normal, center_normal) < 0.5 || dot(candidate_normal, candidate_normal) < 0.5 {
        distance = length(delta);
    }
    if distance >= cutoff { return 0.0; }
    return 1.0 - distance / max(cutoff, 0.001);
}
@fragment fn blur(@builtin(position) position: vec4<f32>) -> @location(0) vec4<u32> {
    let pixel = vec2<i32>(position.xy);
    let centre = textureLoad(half_input, pixel, 0);
    if depth_of(centre) >= 1.0 { return centre; }
    let p = world(selected_pixel(pixel, centre), depth_of(centre));
    let n = normal_of(centre);
    let cutoff = 0.04 * view_depth(depth_of(centre)) + 0.04;
    var sum = value_of(centre) * 9.0;
    var weight = 9.0;
    // Two convolved 3-tap boxes: 1,2,3,2,1 per axis. Each of the
    // three phase residues has total weight 3, so the wider symmetric
    // footprint still cancels a complete phase cycle on a plane.
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let neighbor = pixel + vec2<i32>(x, y);
            if (x == 0 && y == 0) || any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(half_input))) { continue; }
            let sample = textureLoad(half_input, neighbor, 0);
            if depth_of(sample) >= 1.0 { continue; }
            let sample_p = world(selected_pixel(neighbor, sample), depth_of(sample));
            let spatial = f32((3 - abs(x)) * (3 - abs(y)));
            let wgt = spatial * bilateral(p, n, sample_p, normal_of(sample), cutoff);
            sum += value_of(sample) * wgt;
            weight += wgt;
        }
    }
    // Preserve the exact cached geometry through the filter.
    return vec4<u32>(pack2x16float(vec2<f32>(sum / weight, 0.0)) | (centre.x & 196608u), centre.yzw);
}
// Output alpha is the darkening fraction; the pipeline blends colour * (1 - alpha) onto the native colour.
@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let z = depth_at(pixel);
    // Unlike daylight exposure, darkening also applies to weak moon/dusk light.
    // The world mask already excludes unlit surfaces and attenuates native fog and mapped shadows.
    let sun = clamp(textureLoad(sun_mask, pixel, 0).r, 0.0, 1.0);
    if z >= 1.0 || sun <= 0.0 { return vec4<f32>(0.0); }
    let p = world(pixel, z);
    let n = normal(pixel, p);
    let half_coord = (vec2<f32>(pixel) + 0.5) * 0.5 - 0.5;
    let base = vec2<i32>(floor(half_coord));
    let fraction = fract(half_coord);
    var sum = 0.0;
    var weight = 0.0;
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let neighbor = base + vec2<i32>(x, y);
            if any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(half_input))) { continue; }
            let sample = textureLoad(half_input, neighbor, 0);
            if depth_of(sample) >= 1.0 { continue; }
            let spatial = select(1.0 - fraction.x, fraction.x, x == 1) * select(1.0 - fraction.y, fraction.y, y == 1);
            let sample_p = world(selected_pixel(neighbor, sample), depth_of(sample));
            let wgt = spatial * bilateral(p, n, sample_p, normal_of(sample), 0.04 * view_depth(z) + 0.04);
            sum += value_of(sample) * wgt;
            weight += wgt;
        }
    }
    let occlusion = select(0.0, sum / max(weight, 0.00001), weight > 0.0);
    return vec4<f32>(0.0, 0.0, 0.0, clamp(occlusion * params.control.x * sun, 0.0, 1.0));
}
