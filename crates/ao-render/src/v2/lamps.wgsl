// Enhanced v2 lamp lighting: resolve plugin at group 2. `common.wgsl`, `packing.wgsl` and `lamps_cluster.wgsl` precede this
// file; `resolve.wgsl` (which declares `fc` and defines `ggx_spec`) follows it and calls `lamps_light`.
@group(2) @binding(0) var<storage, read> lamp_lights: array<vec4<f32>>;     // record i = 4 vec4 (see lamps_build.wgsl)
@group(2) @binding(1) var<storage, read> lamp_status: array<u32>;           // cube slot + 1 (0 = untapped)
@group(2) @binding(2) var<storage, read> lamp_clusters: array<u32>;
@group(2) @binding(3) var<uniform> point_shadow_metadata: PointShadowMetadata;
@group(2) @binding(4) var point_shadow_maps: texture_depth_2d_array;        // static world faces only (cached)
@group(2) @binding(5) var point_actor_shadow_maps: texture_depth_2d_array;  // admitted-actor faces, rebuilt per frame
@group(2) @binding(6) var point_shadow_sampler: sampler_comparison;
@group(2) @binding(7) var<storage, read> lamp_owner_visibility: array<u32>;

struct PointFaceMetadata { static_matrix: mat4x4<f32>, dynamic_matrix: mat4x4<f32>, params: vec4<f32>, }
struct PointShadowMetadata { slots: array<vec4<f32>, 16>, lights: array<vec4<f32>, 16>, faces: array<PointFaceMetadata, 96>, }

// Cube-atlas taps per pixel (DESIGN 1.3, High): the first lights of the cluster list (ranked by influence) that own a
// cube take one tap; the rest are unshadowed with the analytic falloff.
const LAMP_TAPS = 2u;
// Core radius of the soft inverse-square falloff, as a fraction of the authored range.
const LAMP_CORE = 0.25;

fn lamp_face(delta: vec3<f32>) -> u32 {
    let axis = abs(delta);
    if axis.x >= axis.y && axis.x >= axis.z { return select(1u, 0u, delta.x >= 0.0); }
    if axis.y >= axis.z { return select(3u, 2u, delta.y >= 0.0); }
    return select(5u, 4u, delta.z >= 0.0);
}

// One cached face tap (v1 point_sampling.wgsl, ao-vk pointshadow.cpp face order). Receivers outside a moved face's old
// frustum read unshadowed rather than smearing stale edge depth.
fn lamp_face_visibility(maps: texture_depth_2d_array, matrix: mat4x4<f32>, world: vec3<f32>, layer: i32, ratio: f32) -> f32 {
    if !(ratio > 0.0 && ratio <= 1.0) { return 1.0; }
    let clip = matrix * vec4<f32>(world, 1.0);
    if !(clip.w > 0.0 && clip.z >= 0.0 && clip.z <= clip.w && all(abs(clip.xy) <= vec2<f32>(clip.w))) { return 1.0; }
    let p = clip.xyz / clip.w;
    let uv = vec2<f32>(p.x, -p.y) * 0.5 + vec2<f32>(0.5);
    let half_texel = vec2<f32>(0.5) / vec2<f32>(textureDimensions(maps));
    let scaled = clamp(uv * ratio, half_texel, vec2<f32>(ratio) - half_texel);
    return textureSampleCompareLevel(maps, point_shadow_sampler, scaled, layer, p.z - 0.00015);
}

// Visibility of `world` from the light in cube `slot` (0..15).
fn lamp_cube_visibility(world: vec3<f32>, range: f32, slot: u32) -> f32 {
    let meta = point_shadow_metadata.slots[slot];
    if meta.x <= 0.0 || meta.y < 0.0 { return 1.0; }
    if meta.y > 0.0 {
        let owner = u32(meta.y);
        if owner >= arrayLength(&lamp_owner_visibility) || lamp_owner_visibility[owner] == 0u { return 1.0; }
    }
    let delta = world - point_shadow_metadata.lights[slot].xyz;
    let axis_distance = max(max(abs(delta.x), abs(delta.y)), abs(delta.z));
    if axis_distance <= 0.1 || axis_distance >= range { return 1.0; }
    let face_index = slot * 6u + lamp_face(delta);
    let face = point_shadow_metadata.faces[face_index];
    var vis = 1.0;
    if face.params.x > 0.0 {
        vis = lamp_face_visibility(point_shadow_maps, face.static_matrix, world, i32(face_index), face.params.y);
    }
    if face.params.w > 0.0 {
        vis = min(vis, lamp_face_visibility(point_actor_shadow_maps, face.dynamic_matrix, world, i32(face_index), face.params.z));
    }
    return mix(1.0, vis, clamp(meta.x, 0.0, 1.0));
}

// Soft falloff over the authored range: authored D3D coefficients when present, else an inverse-square core; either way
// windowed to exactly zero at the authored range (Karis window), so the authored reach is the hard cutoff.
fn lamp_falloff(dist: f32, range: f32, atten: vec3<f32>) -> f32 {
    let t = dist / range;
    let window = clamp(1.0 - t * t * t * t, 0.0, 1.0);
    var core: f32;
    if atten.x >= 0.0 {
        core = min(1.0 / max(atten.x + dist * (atten.y + dist * atten.z), 1e-3), 4.0);
    } else {
        let r0 = range * LAMP_CORE;
        core = 1.0 / (1.0 + (dist * dist) / (r0 * r0));
    }
    return core * window * window;
}

// Lamp radiance toward the viewer, ALREADY multiplied by `albedo` (diffuse) plus a small GGX highlight.
fn lamps_light(wp: vec3<f32>, n: vec3<f32>, v: vec3<f32>, depth: f32, pixel: vec2<u32>, albedo: vec3<f32>, rough: f32) -> vec3<f32> {
    if depth > cluster_far() { return vec3<f32>(0.0); }
    let base = cluster_index(pixel, depth) * CLUSTER_STRIDE;
    let count = min(lamp_clusters[base], CLUSTER_MAX);
    var diffuse = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    var taps = 0u;
    for (var k = 0u; k < count; k++) {
        let li = lamp_clusters[base + 1u + k] * 4u;
        let a = lamp_lights[li];
        let d = a.xyz - wp;
        let dist2 = dot(d, d);
        if dist2 >= a.w * a.w { continue; }
        let dist = sqrt(dist2);
        let l = d / max(dist, 1e-3);
        let ndl = dot(n, l);
        if ndl <= 0.0 { continue; }
        let b = lamp_lights[li + 1u];
        let c = lamp_lights[li + 2u];
        var att = lamp_falloff(dist, a.w, c.xyz);
        if b.w < 1.5 {
            let rho = dot(-l, lamp_lights[li + 3u].xyz);
            att *= clamp((rho - b.w) / max(c.w - b.w, 1e-4), 0.0, 1.0);
        }
        let cube = lamp_status[li / 4u];
        if cube != 0u && taps < LAMP_TAPS {
            taps++;
            // Offset along the normal by about a shadow texel (256 px per 90 degree face) against self-shadow acne.
            att *= lamp_cube_visibility(wp + n * (0.01 + dist * 0.012), a.w, cube - 1u);
        }
        let e = b.rgb * (att * ndl);
        diffuse += e;
        spec += e * (0.04 * PI * ggx_spec(n, v, l, rough));
    }
    return albedo * diffuse + spec;
}
