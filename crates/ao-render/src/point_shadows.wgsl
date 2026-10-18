// ao-vk rvk/pointshadow.cpp face-axis perspective depth (MIT; see THIRD_PARTY).
@group(3) @binding(3) var<storage, read> point_actor_visibility: array<u32>;
@group(3) @binding(4) var point_shadow_maps: texture_depth_2d_array;
@group(3) @binding(9) var point_actor_shadow_maps: texture_depth_2d_array;
@group(3) @binding(5) var point_shadow_sampler: sampler_comparison;
struct PointFaceMetadata {
    static_matrix: mat4x4<f32>,
    dynamic_matrix: mat4x4<f32>,
    params: vec4<f32>,
}
struct PointShadowMetadata {
    slots: array<vec4<f32>, 16>,
    lights: array<vec4<f32>, 16>,
    faces: array<PointFaceMetadata, 96>,
}
@group(3) @binding(6) var<uniform> point_shadow_metadata: PointShadowMetadata;
var<private> point_receiver_character: bool;

fn owner_light_admitted(encoded: f32) -> bool {
    if encoded >= 0.0 { return true; }
    let owner = u32(floor(abs(encoded) / 32.0)) - 1u;
    if owner == 0u || owner >= arrayLength(&point_actor_visibility) { return false; }
    return point_actor_visibility[owner] != 0u;
}

// The producer inserts sphere/AABB candidates in ascending admitted-pool order.
// Querying this conservative list therefore preserves the native eight-light
// budget exactly while empty/outside cells avoid scanning all 64 sources.
fn admitted_light_list(world: vec3<f32>) -> vec2<u32> {
    let grid = dyn_lights[64u];
    let metadata = bitcast<vec4<u32>>(dyn_lights[65u]);
    if any(metadata.xyz == vec3<u32>(0u)) || grid.w <= 0.0 { return vec2<u32>(0u); }
    let upper = grid.xyz + vec3<f32>(metadata.xyz) * grid.w;
    if any(world < grid.xyz) || any(world > upper) { return vec2<u32>(0u); }
    // Include the upper boundary: native coefficient attenuation may be nonzero
    // exactly at range, even when division rounds the coordinate to dims.
    let c = min(vec3<u32>(floor((world - grid.xyz) / grid.w)), metadata.xyz - vec3<u32>(1u));
    let cell = (c.z * metadata.y + c.y) * metadata.x + c.x;
    return bitcast<vec4<u32>>(dyn_lights[metadata.w + cell]).xy;
}

// Enhanced spatial distribution of blocked native gamma diffuse. The receiver
// separately caps this loss to its exact Gouraud-interpolated native local share.
fn grass_point_shadow_loss(world: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var loss = vec3<f32>(0.0);
    let directional_count = u32(g.dyn_info.y);
    var used = directional_count + select(0u, 1u, directional_count > 0u);
    let list = admitted_light_list(world);
    let count = min(list.y, 64u);
    if count == 0u || used >= 8u { return loss; }
    let n = normalize(normal);
    for (var k = 0u; k < count && used < 8u; k = k + 1u) {
        let index = bitcast<u32>(dyn_lights[list.x + k].x);
        let li = 66u + index * 4u;
        let a = dyn_lights[li];
        let packed = dyn_lights[li + 3u].w;
        if !owner_light_admitted(packed) { continue; }
        if a.w <= 0.0 { continue; }
        let d = a.xyz - world;
        // Conservative coarse rejection; retain the exact native range test
        // below around float-rounded sphere boundaries.
        if dot(d, d) > a.w * a.w * 1.000001 { continue; }
        let dist = length(d);
        if dist > a.w { continue; }
        used += 1u;
        // Unshadowed lights still consume their native slot, but need no BRDF
        // evaluation or texture comparison in a shadow-loss-only query.
        if (u32(abs(packed)) % 32u) == 0u { continue; }
        let col = dyn_lights[li + 1u];
        let att = dyn_lights[li + 2u];
        let l = d / max(dist, 1e-3);
        var intensity = select(1.0 - dist / a.w, 1.0 / (att.x + dist * (att.y + dist * att.z)), att.x >= 0.0);
        if col.w < 1.5 {
            let rho = dot(-l, dyn_lights[li + 3u].xyz);
            intensity *= clamp((rho - col.w) / max(att.w - col.w, 1e-4), 0.0, 1.0);
        }
        let nl = dot(n, l);
        if nl > 0.0 {
            loss += col.rgb * (intensity * nl) * (1.0 - point_shadow_visibility(-d, a.w, packed, false));
        }
    }
    return max(loss, vec3<f32>(0.0));
}
