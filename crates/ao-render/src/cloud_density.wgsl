// Adapted from ao-vk (MIT), rvk/shaders/cloud_layer.frag Profile/Density and
// cloud_shadow.frag: wind-advected coverage, height profiles, fine erosion and
// Beer-Lambert extinction. Procedural value noise replaces its sampled 3-D noise.
// Enhanced-only approximation; the native cloud sheets are never removed.
struct CloudDensityLayer {
    shape: vec4<f32>, // eye-relative base, thickness, coverage, extinction / metre
    drift: vec4<f32>, // native wind/world phase xyz; native bend radius1500*far/1000
    light: vec4<f32>, // native ground-light RGB, storm
    sun: vec4<f32>,   // native scene-space light direction, thick
    ambient: vec4<f32>, // native authored ambient gamma RGB, native sun_y/thick sheet grey
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
// Native cloud.rs10054e07: centreY = altitude*(1-1.5*distance²),
// distance=min(horizontalRadius/1500,1). Coordinates already have far/1000.
fn cloud_base(p: vec3<f32>, layer: CloudDensityLayer) -> f32 {
    let radius = max(layer.drift.w, 1.0);
    let distance_squared = min(dot(p.xz, p.xz) / (radius * radius), 1.0);
    return layer.shape.x * (1.0 - 1.5 * distance_squared);
}
fn cloud_density(p: vec3<f32>, layer: CloudDensityLayer) -> f32 {
    let radius = max(layer.drift.w, 1.0);
    let radial = length(p.xz) / radius;
    if radial >= 1.0 { return 0.0; }
    let h = (p.y - cloud_base(p, layer)) / layer.shape.y;
    if h <= 0.0 || h >= 1.0 { return 0.0; }
    let q = (p + layer.drift.xyz) / max(layer.shape.y * 2.5, 1.0);
    let stratus = smoothstep(0.0, 0.06, h) * (1.0 - smoothstep(0.12, 0.28, h));
    let cumulus = smoothstep(0.0, 0.1, h) * (1.0 - smoothstep(0.35, 0.65, h));
    let storm = smoothstep(0.0, 0.1, h) * (1.0 - smoothstep(0.7, 1.0, h));
    let profile = mix(mix(stratus, cumulus, 0.7), storm, layer.light.w);
    if profile <= 0.0 { return 0.0; }
    let broad = cloud_noise(q * vec3(1.0, 1.6, 1.0));
    let cover = clamp(layer.shape.z + (cloud_noise(q * 0.31) - 0.5) * 0.3, 0.0, 1.0);
    let body = broad * profile;
    let density = clamp((body - (1.0 - cover)) / max(cover, 0.001), 0.0, 1.0) * cover;
    if density <= 0.0 { return 0.0; }
    let fine = cloud_noise(q * 3.1 + 0.17);
    // Native sheets have finite footprints, not an infinite opaque horizon
    // shelf. Fade the opt-in relief before the bend's capped outer boundary.
    return max(density - (1.0 - fine) * 0.12 * (1.0 - layer.light.w * 0.7), 0.0)
        * (1.0 - smoothstep(0.85, 1.0, radial));
}
fn cloud_roots(a: f32, b: f32, c: f32) -> vec2<f32> {
    if abs(a) < 0.00000001 {
        if abs(b) < 0.00000001 { return vec2(1e20, -1e20); }
        return vec2(-c / b);
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 { return vec2(1e20, -1e20); }
    // Stable quadratic roots: avoid subtracting near-equal positive values
    // for an upward view ray crossing the cloud close to the camera.
    let q = -0.5 * (b + select(-1.0, 1.0, b >= 0.0) * sqrt(discriminant));
    if abs(q) < 0.00000001 { return vec2(-b / (2.0 * a)); }
    let x = q / a;
    let y = c / q;
    return vec2(min(x, y), max(x, y));
}
fn cloud_interval(origin: vec3<f32>, direction: vec3<f32>, layer: CloudDensityLayer, reach: f32) -> vec2<f32> {
    let radius = max(layer.drift.w, 1.0);
    let horizontal = dot(direction.xz, direction.xz);
    var bounds = vec2(0.0, reach);
    if horizontal > 0.00000001 {
        let cylinder = cloud_roots(horizontal, 2.0 * dot(origin.xz, direction.xz), dot(origin.xz, origin.xz) - radius * radius);
        bounds = vec2(max(bounds.x, cylinder.x), min(bounds.y, cylinder.y));
    } else if dot(origin.xz, origin.xz) >= radius * radius {
        return vec2(0.0);
    }
    if bounds.y <= bounds.x { return vec2(0.0); }
    let curvature = 1.5 * layer.shape.x / (radius * radius);
    let a = curvature * horizontal;
    let b = direction.y + 2.0 * curvature * dot(origin.xz, direction.xz);
    let c = origin.y + curvature * dot(origin.xz, origin.xz) - layer.shape.x;
    let lower = cloud_roots(a, b, c);
    let upper = cloud_roots(a, b, c - layer.shape.y);
    var start = 1e20;
    var end = -1e20;
    // A camera inside the volume starts at the clipped ray origin; a vertical
    // ray has linear roots. A horizontal ray still intersects the curved deck.
    for (var i = 0; i < 2; i++) {
        let t = bounds[i];
        let height = (a * t + b) * t + c;
        if height >= 0.0 && height <= layer.shape.y { start = min(start, t); end = max(end, t); }
    }
    let crossings = array<f32, 4>(lower.x, lower.y, upper.x, upper.y);
    for (var i = 0; i < 4; i++) {
        let t = crossings[i];
        if t >= bounds.x && t <= bounds.y { start = min(start, t); end = max(end, t); }
    }
    return vec2(start, end);
}
fn cloud_transmittance_steps(origin: vec3<f32>, direction: vec3<f32>, layer: CloudDensityLayer, reach: f32, steps: u32) -> f32 {
    let interval = cloud_interval(origin, direction, layer, reach);
    if interval.y <= interval.x { return 1.0; }
    let step = (interval.y - interval.x) / f32(steps);
    var optical = 0.0;
    for (var i = 0u; i < steps; i++) {
        optical += cloud_density(origin + direction * (interval.x + (f32(i) + 0.5) * step), layer) * step * layer.shape.w;
        if optical >= 6.0 { break; }
    }
    return exp(-optical);
}
fn cloud_transmittance(origin: vec3<f32>, direction: vec3<f32>, layer: CloudDensityLayer, reach: f32) -> f32 {
    return cloud_transmittance_steps(origin, direction, layer, reach, 12u);
}
