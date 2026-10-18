// Enhanced cloud-only history, adapted from ao-vk (MIT) cloud_resolve.frag.
// No scene colour, actor data or depth is ever read by the temporal estimator.
struct Params {
    inverse_vp: mat4x4<f32>,
    previous_vp: mat4x4<f32>,
    eye: vec4<f32>,
    control: vec4<f32>, // layer count, reach, checker phase, history valid
    enabled: vec4<f32>, // clouds, shadows
    fog: vec4<f32>, // native start/end, cloud quality tier
    fog_colour: vec4<f32>,
    shadow_area: vec4<f32>,
    shadow_sun: vec4<f32>,
}
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> layers: array<CloudDensityLayer>;
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var sun_mask: texture_2d<f32>;
@group(0) @binding(4) var history: texture_2d<f32>;
@group(0) @binding(5) var shadow_field: texture_2d<f32>;
@group(0) @binding(6) var water_cloud: texture_2d<f32>;
fn mapped_transmittance(p: vec3<f32>) -> f32 {
    if params.shadow_sun.w < 0.5 || p.y >= params.enabled.z { return 1.0; }
    let projected = p.xz + params.shadow_sun.xz / params.shadow_sun.y * (params.shadow_area.w - p.y);
    let uv = (projected - params.shadow_area.xy) / params.shadow_area.z;
    if any(uv < vec2(0.0)) || any(uv >= vec2(1.0)) { return 1.0; }
    let size = vec2<i32>(textureDimensions(shadow_field));
    let q = uv * vec2<f32>(size) - 0.5;
    let base = vec2<i32>(floor(q));
    let f = fract(q);
    let a = textureLoad(shadow_field, clamp(base, vec2(0), size - 1), 0).r;
    let b = textureLoad(shadow_field, clamp(base + vec2(1, 0), vec2(0), size - 1), 0).r;
    let c = textureLoad(shadow_field, clamp(base + vec2(0, 1), vec2(0), size - 1), 0).r;
    let d = textureLoad(shadow_field, clamp(base + vec2(1, 1), vec2(0), size - 1), 0).r;
    let edge = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    return mix(1.0, mix(mix(a, b, f.x), mix(c, d, f.x), f.y), smoothstep(0.0, 0.03, edge));
}
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4(vec2(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
fn world(uv: vec2<f32>, z: f32) -> vec3<f32> {
    let p = params.inverse_vp * vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), z, 1.0);
    return p.xyz / p.w;
}
fn cloud_march(direction: vec3<f32>) -> vec4<f32> {
    var radiance = vec3(0.0);
    var transmittance = 1.0;
    for (var l = 0u; l < u32(params.control.x); l++) {
        if transmittance <= 0.005 { break; }
        let layer = layers[l];
        let interval = cloud_interval(vec3(0.0), direction, layer, params.control.y);
        if interval.y <= interval.x { continue; }
        // Half resolution plus four checker phases amortizes the curved march;
        // the short light ray needs fewer samples than the visible silhouette.
        let steps = 16u * u32(params.fog.z);
        let step = (interval.y - interval.x) / f32(steps);
        let sun_length = length(layer.sun.xyz);
        let sun = layer.sun.xyz / max(sun_length, 0.001);
        for (var i = 0u; i < steps; i++) {
            if transmittance <= 0.005 { break; }
            let p = direction * (interval.x + (f32(i) + 0.5) * step);
            let density = cloud_density(p, layer);
            if density <= 0.0 { continue; }
            let extinction = 1.0 - exp(-density * step * layer.shape.w);
            // Native sheets are unfogged and their authored colours are gamma
            // values. This pass blends into the native gamma UNORM sky target:
            // neither decode them to linear nor inject primary-world fog.
            var direct = 0.0;
            if sun_length > 0.001 {
                direct = cloud_transmittance_steps(p, sun, layer, layer.shape.y * 8.0, 4u * u32(params.fog.z))
                    * (0.5 + 0.5 * max(dot(direction, sun), 0.0));
            }
            let grey = layer.ambient.w;
            let illumination = clamp(vec3(grey) * (0.45 + 0.55 * direct)
                + layer.ambient.rgb + grey * layer.light.rgb * (0.55 * direct),
                vec3(0.0), vec3(1.0));
            radiance += transmittance * extinction * illumination;
            transmittance *= 1.0 - extinction;
        }
    }
    return vec4(radiance, 1.0 - transmittance);
}
@fragment fn estimate(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let dimensions = vec2<i32>(textureDimensions(history));
    let pixel = vec2<i32>(position.xy);
    let uv = position.xy / vec2<f32>(dimensions);
    let direction = normalize(world(uv, 1.0));
    let phase = u32(params.control.z);
    let fresh = (u32(pixel.x) & 1u) == (phase & 1u) && (u32(pixel.y) & 1u) == ((phase >> 1u) & 1u);
    if params.control.w > 0.5 && !fresh {
        let clip = params.previous_vp * vec4(direction, 0.0);
        if clip.w > 0.0001 {
            let previous_uv = clip.xy / clip.w * vec2(0.5, -0.5) + 0.5;
            if all(previous_uv >= vec2(0.0)) && all(previous_uv < vec2(1.0)) {
                return textureLoad(history, vec2<i32>(previous_uv * vec2<f32>(dimensions)), 0);
            }
        }
    }
    return cloud_march(direction);
}
@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let z = textureLoad(depth, pixel, 0);
    var modulation = vec3(1.0);
    let mask = textureLoad(sun_mask, pixel, 0);
    if z < 1.0 && mask.a > 0.0001 && params.enabled.y > 0.5 {
        var water_share = 0.0;
        if params.enabled.w > 0.5 { water_share = textureLoad(water_cloud, pixel, 0).r; }
        let share = clamp(mask.r + water_share, 0.0, 1.0);
        if share > 0.0 {
            let uv = position.xy / vec2<f32>(textureDimensions(depth));
            let p = world(uv, z);
            let transmittance = mapped_transmittance(p);
            // Remove only the native fog/shadow-attenuated directional share.
            modulation = vec3(1.0 - share * (1.0 - transmittance));
        }
    }
    return vec4(clamp(modulation, vec3(0.0), vec3(1.0)), 1.0);
}
// Native sky boundary only: no primary depth/coverage read. Foreground geometry,
// including non-depthwriting translucent FX, is submitted after this draw.
@fragment fn sky_background(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(history));
    let q = position.xy * 0.5 - 0.5;
    let base = vec2<i32>(floor(q));
    let f = fract(q);
    let a = textureLoad(history, clamp(base, vec2(0), size - 1), 0);
    let b = textureLoad(history, clamp(base + vec2(1, 0), vec2(0), size - 1), 0);
    let c = textureLoad(history, clamp(base + vec2(0, 1), vec2(0), size - 1), 0);
    let d = textureLoad(history, clamp(base + vec2(1, 1), vec2(0), size - 1), 0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}
