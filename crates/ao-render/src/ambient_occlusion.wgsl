// HBAO-lite: current primary-view depth only; no actor lists or history.
struct Params {
    inverse_vp: mat4x4<f32>,
    vp: mat4x4<f32>,
    control: vec4<f32>, // radius, strength, original fog-end depth, height
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
fn depth_at(pixel: vec2<i32>) -> f32 {
    let z = textureLoad(depth, clamp(pixel, vec2<i32>(0), vec2<i32>(textureDimensions(depth)) - 1), 0);
    return select(z, 1.0, z >= params.control.z);
}
fn world(pixel: vec2<i32>, z: f32) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(textureDimensions(depth));
    let p = params.inverse_vp * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
    return p.xyz / p.w;
}
// Pick an actual primary-view sample rather than averaging foreground/background.
fn half_pixel(pixel: vec2<i32>) -> vec2<i32> {
    var selected = pixel * 2;
    var z = depth_at(selected);
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let candidate = pixel * 2 + vec2<i32>(x, y);
            let d = depth_at(candidate);
            if inside(candidate) && d < z { selected = candidate; z = d; }
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
    return cross_n / max(length(cross_n), 0.00001);
}
// Keep depth at full precision; AO needs only half precision for UNORM output.
// Cache the selected 2x2 offset rather than reloading four depths at every
// bilateral tap (including each full-resolution upsample tap).
fn encoded(value: f32, z: f32, pixel: vec2<i32>) -> vec2<u32> {
    let offset = u32(pixel.x & 1) | (u32(pixel.y & 1) << 1u);
    return vec2<u32>(pack2x16float(vec2<f32>(value, 0.0)) | (offset << 16u), bitcast<u32>(z));
}
fn value_of(sample: vec2<u32>) -> f32 { return unpack2x16float(sample.x).x; }
fn depth_of(sample: vec2<u32>) -> f32 { return bitcast<f32>(sample.y); }
fn selected_pixel(half: vec2<i32>, sample: vec2<u32>) -> vec2<i32> {
    return half * 2 + vec2<i32>(i32((sample.x >> 16u) & 1u), i32((sample.x >> 17u) & 1u));
}
@fragment fn estimate(@builtin(position) position: vec4<f32>) -> @location(0) vec2<u32> {
    let pixel = half_pixel(vec2<i32>(position.xy));
    let z = depth_at(pixel);
    if z >= 1.0 { return encoded(0.0, z, pixel); }
    let p = world(pixel, z);
    let n = normal(pixel, p);
    let clip = params.vp * vec4<f32>(p, 1.0);
    let projection_y = length(vec3<f32>(params.vp[0].y, params.vp[1].y, params.vp[2].y));
    let radius_pixels = clamp(params.control.x * projection_y * params.control.w / (2.0 * max(clip.w, 0.01)), 1.0, 64.0);
    var occlusion = 0.0;
    let axes = array<vec2<f32>, 6>(vec2(1.0,0.0), vec2(0.5,0.8660254), vec2(-0.5,0.8660254),
        vec2(-1.0,0.0), vec2(-0.5,-0.8660254), vec2(0.5,-0.8660254));
    for (var direction = 0; direction < 6; direction++) {
        let axis = axes[direction];
        var horizon = 0.0;
        for (var step = 1; step <= 4; step++) {
            let sample_pixel = pixel + vec2<i32>(round(axis * radius_pixels * f32(step) / 4.0));
            if !inside(sample_pixel) { continue; }
            let sample_z = depth_at(sample_pixel);
            if sample_z >= 1.0 { continue; }
            let delta = world(sample_pixel, sample_z) - p;
            let normal_distance = dot(n, delta);
            if normal_distance <= 0.0 { continue; }
            let distance = length(delta);
            if distance < 0.01 || distance >= params.control.x { continue; }
            let elevation = max(normal_distance / distance - 0.15, 0.0);
            horizon = max(horizon, elevation * (1.0 - distance / params.control.x));
        }
        occlusion += horizon;
    }
    return encoded(clamp(occlusion / 6.0 * params.control.y, 0.0, 1.0), z, pixel);
}
// Strict world-space depth rejection prevents background AO crossing silhouettes.
fn bilateral(center: vec3<f32>, candidate: vec3<f32>) -> f32 {
    let distance = length(center - candidate);
    let cutoff = min(params.control.x * 0.15, 0.15);
    if distance >= cutoff { return 0.0; }
    return 1.0 - distance / max(cutoff, 0.001);
}
@fragment fn blur(@builtin(position) position: vec4<f32>) -> @location(0) vec2<u32> {
    let pixel = vec2<i32>(position.xy);
    let center = textureLoad(ao_input, pixel, 0).rg;
    if depth_of(center) >= 1.0 { return center; }
    let p = world(selected_pixel(pixel, center), depth_of(center));
    var sum = value_of(center);
    var weight = 1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            if x == 0 && y == 0 { continue; }
            let neighbor = pixel + vec2<i32>(x, y);
            if any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(ao_input))) { continue; }
            let sample = textureLoad(ao_input, neighbor, 0).rg;
            if depth_of(sample) >= 1.0 { continue; }
            let w = bilateral(p, world(selected_pixel(neighbor, sample), depth_of(sample)));
            sum += value_of(sample) * w;
            weight += w;
        }
    }
    return encoded(sum / weight, depth_of(center), selected_pixel(pixel, center));
}
@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let z = depth_at(pixel);
    if z >= 1.0 { return vec4<f32>(0.0); }
    let p = world(pixel, z);
    let half_coord = (vec2<f32>(pixel) + 0.5) * 0.5 - 0.5;
    let base = vec2<i32>(floor(half_coord));
    let fraction = fract(half_coord);
    var sum = 0.0;
    var weight = 0.0;
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let neighbor = base + vec2<i32>(x, y);
            if any(neighbor < vec2<i32>(0)) || any(neighbor >= vec2<i32>(textureDimensions(ao_input))) { continue; }
            let sample = textureLoad(ao_input, neighbor, 0).rg;
            if depth_of(sample) >= 1.0 { continue; }
            let spatial = select(1.0 - fraction.x, fraction.x, x == 1) * select(1.0 - fraction.y, fraction.y, y == 1);
            let w = spatial * bilateral(p, world(selected_pixel(neighbor, sample), depth_of(sample)));
            sum += value_of(sample) * w;
            weight += w;
        }
    }
    let occlusion = select(0.0, sum / max(weight, 0.00001), weight > 0.0);
    return vec4<f32>(textureLoad(ambient, pixel, 0).rgb * occlusion, 0.0);
}
