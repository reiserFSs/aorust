// HBAO-lite: current primary-view depth only; no actor lists or history.
// Direction/step sampling and the sum/(directions*steps) normalisation follow
// ao-vk rvk/shaders/ao.frag (MIT, wannabuh/ao-vk), see THIRD_PARTY.
const TAU = 6.2831853;
// Sky-fill share is darkened SKY_BOOST x harder than the raw occlusion; the
// receiver weights the sun share by SUN_CAP / SKY_BOOST so at most SUN_CAP of it
// is removed. Must match AO_SKY_BOOST / AO_SUN_CAP in enhanced_output.wgsl.
const SKY_BOOST = 1.25;
// ao.frag uses 2; sparse jittered samples of a 1.75 m radius read weaker in the native dim lighting.
const OCCLUSION_GAIN = 4.0;
struct Params {
    inverse_vp: mat4x4<f32>,
    vp: mat4x4<f32>,
    control: vec4<f32>, // radius, strength, original fog-end depth, grass present
    budget: vec4<u32>, // directions, steps, reserved, reserved
}
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var ao_input: texture_2d<u32>;
@group(0) @binding(3) var ambient: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
fn inside(pixel: vec2<i32>) -> bool {
    return all(pixel >= vec2<i32>(0)) && all(pixel < vec2<i32>(textureDimensions(depth)));
}
fn raw_depth_at(pixel: vec2<i32>) -> f32 {
    let coord = clamp(pixel, vec2<i32>(0), vec2<i32>(textureDimensions(depth)) - 1);
    let z = textureLoad(depth, coord, 0);
    return select(z, 1.0, z >= params.control.z);
}
fn occluder_share_at(pixel: vec2<i32>) -> f32 {
    if params.control.w > 0.0 {
        let coord = clamp(pixel, vec2<i32>(0), vec2<i32>(textureDimensions(depth)) - 1);
        // Current visible grass writes a negative alpha; ordinary surfaces stay opaque.
        // ponytail: mixed MSAA edge pixels become porous; use min-depth-owned tags if edge AO fidelity matters.
        return clamp(textureLoad(ambient, coord, 0).a + 1.0, 0.0, 1.0);
    }
    return 1.0;
}
fn depth_sample(pixel: vec2<i32>) -> vec2<f32> {
    let z = raw_depth_at(pixel);
    if z >= 1.0 { return vec2<f32>(1.0, 1.0); }
    let share = occluder_share_at(pixel);
    return vec2<f32>(select(z, 1.0, share == 0.0), share);
}
// inverse_vp reconstructs world-axis positions relative to a nearby reference;
// translation cancels from all AO deltas without losing small vectors in f32.
fn world_homogeneous(pixel: vec2<i32>, z: f32) -> vec4<f32> {
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(textureDimensions(depth));
    return params.inverse_vp * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
}
fn world(pixel: vec2<i32>, z: f32) -> vec3<f32> {
    let p = world_homogeneous(pixel, z);
    return p.xyz / p.w;
}
struct HalfSample {
    pixel: vec2<i32>,
    z: f32,
    share: f32,
}
// Pick and cache an actual primary-view sample rather than averaging depths.
fn half_sample(pixel: vec2<i32>) -> HalfSample {
    let first = pixel * 2;
    let sample = depth_sample(first);
    var selected = HalfSample(first, sample.x, sample.y);
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            if x == 0 && y == 0 { continue; }
            let candidate = first + vec2<i32>(x, y);
            if !inside(candidate) { continue; }
            let d = depth_sample(candidate);
            if d.x < selected.z { selected = HalfSample(candidate, d.x, d.y); }
        }
    }
    return selected;
}
fn normal(pixel: vec2<i32>, p: vec3<f32>) -> vec3<f32> {
    // Porosity must not turn neighboring blade depths into far-plane normals.
    let left = p - world(pixel - vec2<i32>(1, 0), raw_depth_at(pixel - vec2<i32>(1, 0)));
    let right = world(pixel + vec2<i32>(1, 0), raw_depth_at(pixel + vec2<i32>(1, 0))) - p;
    let up = p - world(pixel - vec2<i32>(0, 1), raw_depth_at(pixel - vec2<i32>(0, 1)));
    let down = world(pixel + vec2<i32>(0, 1), raw_depth_at(pixel + vec2<i32>(0, 1))) - p;
    let dx = select(right, left, dot(left, left) < dot(right, right));
    let dy = select(down, up, dot(up, up) < dot(down, down));
    let cross_n = cross(dy, dx);
    return cross_n / max(length(cross_n), 0.00001);
}
// Keep depth at full precision and selected 2x2 offsets exact. Cache the
// selected-surface normal once so bilateral taps need no depth-normal gathers.
// AO and normals use half precision; degenerate normals stay degenerate.
fn encoded(value: f32, z: f32, pixel: vec2<i32>, n: vec3<f32>) -> vec4<u32> {
    let offset = u32(pixel.x & 1) | (u32(pixel.y & 1) << 1u);
    return vec4<u32>(pack2x16float(vec2<f32>(value, 0.0)) | (offset << 16u), bitcast<u32>(z),
        pack2x16float(n.xy), pack2x16float(vec2<f32>(n.z, 0.0)));
}
fn value_of(sample: vec4<u32>) -> f32 { return unpack2x16float(sample.x).x; }
fn depth_of(sample: vec4<u32>) -> f32 { return bitcast<f32>(sample.y); }
fn normal_of(sample: vec4<u32>) -> vec3<f32> {
    return vec3<f32>(unpack2x16float(sample.z), unpack2x16float(sample.w).x);
}
fn selected_pixel(half: vec2<i32>, sample: vec4<u32>) -> vec2<i32> {
    return half * 2 + vec2<i32>(i32((sample.x >> 16u) & 1u), i32((sample.x >> 17u) & 1u));
}
@fragment fn estimate(@builtin(position) position: vec4<f32>) -> @location(0) vec4<u32> {
    let center = half_sample(vec2<i32>(position.xy));
    let pixel = center.pixel;
    let z = center.z;
    if z >= 1.0 { return encoded(0.0, z, pixel, vec3<f32>(0.0)); }
    let homogeneous = world_homogeneous(pixel, z);
    let p = homogeneous.xyz / homogeneous.w;
    let n = normal(pixel, p);
    // Translation leaves homogeneous w unchanged; vp cannot project relative p.
    let clip_w = 1.0 / homogeneous.w;
    let projection_y = length(vec3<f32>(params.vp[0].y, params.vp[1].y, params.vp[2].y));
    let radius_pixels = clamp(params.control.x * projection_y * f32(textureDimensions(depth).y) / (2.0 * max(clip_w, 0.01)), 1.0, 64.0);
    // Nine fixed phases repeat every 3x3 half-resolution pixels. The filter
    // below weights every phase equally; neither rotation nor jitter uses time.
    let tile = vec2<u32>(position.xy) % vec2<u32>(3u);
    let noise = (f32((tile.x * 3u + tile.y * 7u) % 9u) + 0.5) / 9.0;
    var occlusion = 0.0;
    let directions = clamp(params.budget.x, 4u, 8u);
    let steps = clamp(params.budget.y, 3u, 6u);
    for (var direction = 0u; direction < directions; direction++) {
        let angle = (f32(direction) + noise) * (TAU / f32(directions));
        let axis = vec2<f32>(cos(angle), sin(angle));
        for (var step = 0u; step < steps; step++) {
            let t = (f32(step) + fract(noise * 7.0 + f32(step) * 0.618)) / f32(steps);
            let sample_pixel = pixel + vec2<i32>(round(axis * (t * radius_pixels + 1.0)));
            if !inside(sample_pixel) { continue; }
            let sample_z = raw_depth_at(sample_pixel);
            if sample_z >= 1.0 { continue; }
            let delta = world(sample_pixel, sample_z) - p;
            let normal_distance = dot(n, delta);
            if normal_distance <= 0.0 { continue; }
            let distance = length(delta);
            if distance < 0.01 || distance >= params.control.x { continue; }
            let contribution = max(normal_distance / distance - 0.15, 0.0) * (1.0 - distance / params.control.x);
            if contribution > 0.0 { occlusion += contribution * occluder_share_at(sample_pixel); }
        }
    }
    // Strength is an exponent on the open fraction: 0 -> neutral, always within [0, 1].
    let open = 1.0 - clamp(OCCLUSION_GAIN * occlusion / f32(directions * steps), 0.0, 1.0);
    return encoded((1.0 - pow(open, params.control.y)) * center.share, z, pixel, n);
}
// Reject separation across either surface, not tangential separation along it:
// a fixed Euclidean cutoff otherwise rejects even adjacent texels at distance.
// The symmetric test also rejects a background lying in the foreground's plane
// when its own plane disagrees (a grazing-angle silhouette).
fn bilateral(center: vec3<f32>, center_normal: vec3<f32>, candidate: vec3<f32>, candidate_normal: vec3<f32>) -> f32 {
    let delta = candidate - center;
    var distance = max(abs(dot(delta, center_normal)), abs(dot(delta, candidate_normal)));
    // Degenerate depth normals cannot establish surface continuity.
    if dot(center_normal, center_normal) < 0.5 || dot(candidate_normal, candidate_normal) < 0.5 {
        distance = length(delta);
    }
    let cutoff = min(params.control.x * 0.15, 0.15);
    if distance >= cutoff { return 0.0; }
    return 1.0 - distance / max(cutoff, 0.001);
}
@fragment fn blur(@builtin(position) position: vec4<f32>) -> @location(0) vec4<u32> {
    let pixel = vec2<i32>(position.xy);
    let center = textureLoad(ao_input, pixel, 0);
    if depth_of(center) >= 1.0 { return center; }
    let p = world(selected_pixel(pixel, center), depth_of(center));
    let n = normal_of(center);
    var sum = value_of(center) * 9.0;
    var weight = 9.0;
    // Two convolved 3-wide boxes give symmetric weights 1,2,3,2,1. Every
    // modulo-3 phase has equal total weight, unlike a truncated Gaussian.
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            if x == 0 && y == 0 { continue; }
            let neighbor = pixel + vec2<i32>(x, y);
            if any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(ao_input))) { continue; }
            let sample = textureLoad(ao_input, neighbor, 0);
            if depth_of(sample) >= 1.0 { continue; }
            let sample_pixel = selected_pixel(neighbor, sample);
            let sample_p = world(sample_pixel, depth_of(sample));
            let spatial = f32((3 - abs(x)) * (3 - abs(y)));
            let w = spatial * bilateral(p, n, sample_p, normal_of(sample));
            sum += value_of(sample) * w;
            weight += w;
        }
    }
    // Filter only AO; preserve the exact cached geometry through the blur.
    return vec4<u32>(pack2x16float(vec2<f32>(sum / weight, 0.0)) | (center.x & 196608u), center.yzw);
}
@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let z = raw_depth_at(pixel);
    if z >= 1.0 { return vec4<f32>(0.0); }
    let receiver = textureLoad(ambient, pixel, 0);
    if params.control.w > 0.0 && clamp(receiver.a + 1.0, 0.0, 1.0) == 0.0 { return vec4<f32>(0.0); }
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
            if any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(ao_input))) { continue; }
            let sample = textureLoad(ao_input, neighbor, 0);
            if depth_of(sample) >= 1.0 { continue; }
            let spatial = select(1.0 - fraction.x, fraction.x, x == 1) * select(1.0 - fraction.y, fraction.y, y == 1);
            let sample_pixel = selected_pixel(neighbor, sample);
            let sample_p = world(sample_pixel, depth_of(sample));
            let w = spatial * bilateral(p, n, sample_p, normal_of(sample));
            sum += value_of(sample) * w;
            weight += w;
        }
    }
    let occlusion = select(0.0, sum / max(weight, 0.00001), weight > 0.0);
    return vec4<f32>(receiver.rgb * min(occlusion * SKY_BOOST, 1.0), 0.0);
}
