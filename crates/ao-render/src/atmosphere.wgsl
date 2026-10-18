// Extinction composes AFTER native fog. Scattering uses current admitted
// directional light, never actor positions, a UTC clock or temporal history.
struct AtmosphereParams {
    inverse_vp: mat4x4<f32>, eye: vec4<f32>, sun: vec4<f32>, fog: vec4<f32>,
    profile: vec4<f32>, light: vec4<f32>, horizon: vec4<f32>,
    cloud_area: vec4<f32>, cloud_sun: vec4<f32>, cloud_shift: vec4<f32>,
    flags: vec4<u32>, counts: vec4<u32>,
}
struct MistVolume { center_radius: vec4<f32>, density: vec4<f32> }
@group(0) @binding(0) var<uniform> a: AtmosphereParams;
@group(0) @binding(1) var primary_depth: texture_depth_2d;
@group(0) @binding(2) var directional_mask: texture_2d<f32>;
@group(0) @binding(3) var atmosphere_half: texture_2d<f32>;
@group(0) @binding(4) var<storage, read> mist_volumes: array<MistVolume>;
@group(0) @binding(5) var cloud_shadow_field: texture_2d<f32>;
// Matches native sun_shadows::ShadowUniform. Parent supplies current sun OR
// authored moon cascades. Parent supplies the shared authored moonlight energy.
struct ShadowFrame {
    matrices: array<mat4x4<f32>, 3>, splits: vec4<f32>, eye: vec4<f32>, forward: vec4<f32>, control: vec4<u32>,
}
@group(1) @binding(0) var<uniform> shadow_frame: ShadowFrame;
@group(1) @binding(1) var shadow_maps: texture_depth_2d_array;
@group(1) @binding(2) var shadow_sampler: sampler_comparison;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
fn point(pixel: vec2<i32>, z: f32) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(textureDimensions(primary_depth));
    let h = a.inverse_vp * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
    return h.xyz / h.w;
}
fn full_pixel(half_pixel: vec2<i32>) -> vec2<i32> {
    return min(half_pixel * 2 + vec2<i32>(1), vec2<i32>(textureDimensions(primary_depth)) - 1);
}
// Native cloud sheets may write depth, but only admitted world fragments write
// coverage into the native world-light MRT. Preserve native depth/occlusion;
// classify sky independently for atmosphere integration and bilateral taps.
fn scene_depth(pixel: vec2<i32>) -> f32 {
    let raw_depth = textureLoad(primary_depth, pixel, 0);
    let world_coverage = textureLoad(directional_mask, pixel, 0).a;
    return select(1.0, raw_depth, world_coverage > 0.0001);
}
fn haze_transmittance(p: vec3<f32>, z: f32) -> vec3<f32> {
    let distance = min(length(p), a.eye.w);
    var tau = 0.0;
    if a.flags.x != 0u { tau += height_haze_tau(p, distance); }
    if a.flags.y != 0u { tau += land_haze_tau(distance, z); }
    if a.flags.z != 0u { tau += valley_mist_tau(p, distance); }
    return exp(-max(tau, 0.0) * (vec3<f32>(1.0) - 0.7 * a.fog.rgb));
}
fn native_remaining(p: vec3<f32>) -> f32 {
    let range = max(a.fog.w - a.profile.y, 0.001);
    return clamp((a.fog.w - length(p)) / range, 0.0, 1.0);
}
// Coverage-classified sky is not an unlit/fogged world surface. Its air uses
// the chosen directional radiance and current admitted occluder fields directly.
fn air_light_admission(z: f32, surface_share: f32, remaining: f32) -> f32 {
    if z >= 1.0 { return 1.0; }
    return clamp(surface_share, 0.0, 1.0) * clamp(remaining, 0.0, 1.0);
}
fn encode_transmittance(p: vec3<f32>, z: f32, pixel: vec2<i32>, lighting: vec2<f32>) -> vec4<f32> {
    let surface_share = textureLoad(directional_mask, pixel, 0).r;
    let share = select(clamp(surface_share, 0.0, 1.0), 1.0, z >= 1.0);
    let shaft_t = exp(-max(lighting.x, 0.0) * min(length(p) * 0.006, 1.0) * share);
    let enabled = a.flags.w != 0u || (a.counts.z != 0u && a.counts.y != 0u);
    let admitted = air_light_admission(z, surface_share, native_remaining(p));
    let scatter = select(0.0, clamp(lighting.y, 0.0, 1.0) * 0.08 * min(length(p) / 80.0, 1.0) * admitted, enabled);
    return vec4<f32>(clamp(haze_transmittance(p, z) * shaft_t, vec3<f32>(0.0), vec3<f32>(1.0)), scatter);
}
@fragment fn haze(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = full_pixel(vec2<i32>(position.xy));
    let z = scene_depth(pixel);
    let p = point(pixel, min(z, 0.999999));
    return encode_transmittance(p, z, pixel, cloud_occlusion(p));
}
fn filtered_atmosphere(position: vec4<f32>) -> vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let z = scene_depth(pixel);
    let p = point(pixel, min(z, 0.999999));
    let center = (vec2<f32>(pixel) + 0.5) * 0.5 - 0.5;
    let lo = vec2<i32>(floor(center));
    let fraction = fract(center);
    var total = 0.0;
    var transmittance = vec3<f32>(0.0);
    var scattering = 0.0;
    for (var y = 0; y < 2; y++) { for (var x = 0; x < 2; x++) {
        let tap = clamp(lo + vec2<i32>(x,y), vec2<i32>(0), vec2<i32>(textureDimensions(atmosphere_half)) - 1);
        let sample = textureLoad(atmosphere_half, tap, 0);
        let tap_z = scene_depth(full_pixel(tap));
        let q = point(full_pixel(tap), min(tap_z, 0.999999));
        let spatial = select(1.0 - fraction.x, fraction.x, x == 1) * select(1.0 - fraction.y, fraction.y, y == 1);
        let weight = spatial * exp(-abs(length(q) - length(p)) / max(0.2, length(p) * 0.015));
        transmittance += sample.rgb * weight;
        scattering += sample.a * weight;
        total += weight;
    }}
    if total < 0.00001 { return vec4<f32>(1.0, 1.0, 1.0, 0.0); }
    return vec4<f32>(clamp(transmittance / total, vec3<f32>(0.0), vec3<f32>(1.0)), max(scattering / total, 0.0));
}
@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(filtered_atmosphere(position).rgb, 1.0);
}
@fragment fn composite_light(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let filtered = filtered_atmosphere(position);
    let pixel = vec2<i32>(position.xy);
    let z = scene_depth(pixel);
    let p = point(pixel, min(z, 0.999999));
    // Keep world fog/admission guards at full resolution. Native sky has no
    // surface direct-light mask, so admit its independently marched air instead.
    let admitted = air_light_admission(z, textureLoad(directional_mask, pixel, 0).r, native_remaining(p));
    var aerial = vec3<f32>(0.0);
    if a.flags.x != 0u || a.flags.y != 0u || a.flags.z != 0u {
        let haze_t = haze_transmittance(p, z);
        let shaft_t = clamp(filtered.rgb / max(haze_t, vec3<f32>(0.00001)), vec3<f32>(0.0), vec3<f32>(1.0));
        // Smoothly replace lost surface radiance with the authored native fog
        // horizon. This adds no surface visibility and never undoes extinction.
        aerial = a.horizon.rgb * (vec3<f32>(1.0) - haze_t) * shaft_t;
    }
    return vec4<f32>(aerial + a.light.rgb * filtered.a * filtered.rgb * admitted, 1.0);
}
