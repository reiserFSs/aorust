// Soft twenty-spike and exponential-halo treatment adapted from ao-vk's
// rvk/shaders/sun.frag, Copyright (c) 2026 wannabuh (MIT License).
// Native triangles, UVs, tint and flickering fan alpha remain authoritative.
struct Params { control: vec4<f32> }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var primary_depth: texture_depth_2d;
@group(0) @binding(2) var authored: texture_2d<f32>;
@group(0) @binding(3) var authored_sampler: sampler;
@group(0) @binding(4) var<storage, read> clouds: array<CloudDensityLayer>;
@group(0) @binding(5) var directional_mask: texture_2d<f32>;
struct In {
    @location(0) clip: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radial: vec2<f32>,
    @location(4) ray: vec3<f32>,
    @location(5) sun: f32,
}
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) radial: vec2<f32>,
    @location(3) ray: vec3<f32>,
    @location(4) @interpolate(flat) sun: f32,
}
@vertex fn vs(i: In) -> Out {
    var o: Out;
    o.clip = i.clip; o.uv = i.uv; o.color = i.color;
    o.radial = i.radial; o.ray = i.ray; o.sun = i.sun;
    return o;
}
fn hash(n: f32) -> f32 { return fract(sin(n * 12.9898 + 4.1414) * 43758.5453); }
fn spikes(angle: f32, radius: f32, time: f32) -> f32 {
    let x = angle * 20.0;
    let h = hash(floor(x));
    let extent = (0.15 + 0.1 * h) * (1.0 + 0.25 * sin(time * (0.35 + 0.5 * h) + h * 40.0));
    let peak = exp(-pow((fract(x) - 0.5) / 0.2, 2.0));
    return peak * (1.0 - smoothstep(0.06, extent, radius));
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> {
    let texel = textureSample(authored, authored_sampler, i.uv);
    // Native cloud sheets can write primary depth. Only admitted world coverage
    // in the directional mask blocks celestial detail; clouds attenuate below.
    let pixel = vec2<i32>(i.clip.xy);
    if textureLoad(directional_mask, pixel, 0).a > 0.0001 { discard; }
    let alpha = clamp(texel.a * i.color.a, 0.0, 1.0);
    if alpha <= 0.0 { discard; }
    let direction = normalize(i.ray);
    var transmission = 1.0;
    for (var layer = 0u; layer < u32(p.control.z); layer++) {
        transmission *= cloud_transmittance(vec3<f32>(0.0), direction, clouds[layer], p.control.w);
    }
    // Strong extinction closes detail rather than leaving needle-like rays
    // punched through dense cloud. Native cloud rendering is never replaced.
    let seen = transmission * smoothstep(0.15, 0.65, transmission);
    let low = 1.0 - smoothstep(0.0, 0.3, abs(direction.y));
    // Spread the low sun within its enlarged authored halo fan. Its native
    // radius remains the base; no fixed astronomical angular size is invented.
    let q = vec2<f32>(i.radial.x / (1.0 + low * 0.25), i.radial.y);
    let radius = length(q);
    let angle = fract(atan2(q.y, q.x) / 6.2831853 + 0.5 + p.control.x * 0.004);
    let soft = min(exp(-(radius - 0.12) / 0.2), 1.0) * (1.0 - smoothstep(0.6, 0.85, radius));
    let detail = spikes(angle, radius, p.control.x);
    let ray_angle = angle * 48.0;
    let rays = exp(-pow((fract(ray_angle) - 0.5) / 0.12, 2.0)) * hash(floor(ray_angle)) * (1.0 - smoothstep(0.12, 0.8, radius));
    let horizon = low * exp(-abs(q.x) / 1.8) * exp(-abs(q.y) / 0.22);
    let native = max(texel.rgb * i.color.rgb, vec3<f32>(0.0));
    let core = exp(-pow(radius / 0.09, 2.0));
    let sun_energy = (native * (soft * 0.25 + horizon * 0.08 + detail * 0.12 + rays * 0.025) + vec3<f32>(max(native.r, max(native.g, native.b))) * core * 0.35) * p.control.y;
    // Moon refinement retains its authored texture/phase, never substitutes a
    // procedural sphere or an astronomical clock. Night energy is adjustable
    // by the parent's enabled sky-disc mode rather than a world-light claim.
    let moon_energy = native * (1.0 - p.control.y) * soft * 0.08;
    let energy = select(moon_energy, sun_energy, i.sun > 0.5) * alpha * seen;
    let soften = alpha * transmission * (1.0 - soft) * 0.025;
    return vec4<f32>(energy, 1.0 - soften);
}
