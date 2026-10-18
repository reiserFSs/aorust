// Adapted from ao-vk (MIT), rvk/shaders/cloud_layer.frag Profile/Density and
// cloud_shadow.frag: wind-advected coverage, height profiles, fine erosion and
// Beer-Lambert extinction. Procedural value noise replaces its sampled 3-D noise.
// Enhanced-only approximation; the native cloud sheets are never removed.
struct CloudDensityLayer {
    shape: vec4<f32>, // eye-relative base, thickness, coverage, extinction / metre
    drift: vec4<f32>, // world phase offset including native high-altitude wind
    light: vec4<f32>, // native ground-light RGB, storm
    sun: vec4<f32>,   // native scene-space light direction, thick
    ambient: vec4<f32>, // native authored ambient RGB, unused
}
fn cloud_hash(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}
fn cloud_noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let v = fract(p);
    let f = v * v * (3.0 - 2.0 * v);
    return mix(mix(mix(cloud_hash(i), cloud_hash(i + vec3(1.,0.,0.)), f.x),
                   mix(cloud_hash(i + vec3(0.,1.,0.)), cloud_hash(i + vec3(1.,1.,0.)), f.x), f.y),
               mix(mix(cloud_hash(i + vec3(0.,0.,1.)), cloud_hash(i + vec3(1.,0.,1.)), f.x),
                   mix(cloud_hash(i + vec3(0.,1.,1.)), cloud_hash(i + vec3(1.,1.,1.)), f.x), f.y), f.z);
}
fn cloud_density(p: vec3<f32>, layer: CloudDensityLayer) -> f32 {
    let h = (p.y - layer.shape.x) / layer.shape.y;
    if h <= 0.0 || h >= 1.0 { return 0.0; }
    let q = (p + layer.drift.xyz) / max(layer.shape.y * 2.5, 1.0);
    let stratus = smoothstep(0.0, 0.06, h) * (1.0 - smoothstep(0.12, 0.28, h));
    let cumulus = smoothstep(0.0, 0.1, h) * (1.0 - smoothstep(0.35, 0.65, h));
    let storm = smoothstep(0.0, 0.1, h) * (1.0 - smoothstep(0.7, 1.0, h));
    let profile = mix(mix(stratus, cumulus, 0.7), storm, layer.light.w);
    let broad = cloud_noise(q * vec3(1.0, 1.6, 1.0));
    let fine = cloud_noise(q * 3.1 + 0.17);
    let cover = clamp(layer.shape.z + (cloud_noise(q * 0.31) - 0.5) * 0.3, 0.0, 1.0);
    let body = broad * profile;
    let density = clamp((body - (1.0 - cover)) / max(cover, 0.001), 0.0, 1.0) * cover;
    return max(density - (1.0 - fine) * 0.12 * (1.0 - layer.light.w * 0.7), 0.0);
}
fn cloud_interval(origin: vec3<f32>, direction: vec3<f32>, layer: CloudDensityLayer, reach: f32) -> vec2<f32> {
    if abs(direction.y) < 0.0001 { return vec2(0.0); }
    let a = (layer.shape.x - origin.y) / direction.y;
    let b = (layer.shape.x + layer.shape.y - origin.y) / direction.y;
    return vec2(max(min(a, b), 0.0), min(max(a, b), reach));
}
fn cloud_transmittance(origin: vec3<f32>, direction: vec3<f32>, layer: CloudDensityLayer, reach: f32) -> f32 {
    let interval = cloud_interval(origin, direction, layer, reach);
    if interval.y <= interval.x { return 1.0; }
    let step = (interval.y - interval.x) / 12.0;
    var optical = 0.0;
    for (var i = 0; i < 12; i++) {
        optical += cloud_density(origin + direction * (interval.x + (f32(i) + 0.5) * step), layer) * step * layer.shape.w;
    }
    return exp(-optical);
}
