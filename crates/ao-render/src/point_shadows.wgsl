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

