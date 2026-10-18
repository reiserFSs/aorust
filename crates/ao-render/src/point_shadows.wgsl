// ao-vk rvk/pointshadow.cpp face-axis perspective depth (MIT; see THIRD_PARTY).
@group(3) @binding(3) var<storage, read> point_actor_visibility: array<u32>;
@group(3) @binding(4) var point_shadow_maps: texture_depth_cube_array;
@group(3) @binding(5) var point_shadow_sampler: sampler_comparison;
struct PointShadowMetadata { slots: array<vec4<f32>, 16>, }
@group(3) @binding(6) var<uniform> point_shadow_metadata: PointShadowMetadata;
var<private> point_receiver_character: bool;

fn owner_light_admitted(encoded: f32) -> bool {
    if encoded >= 0.0 { return true; }
    let owner = u32(floor(abs(encoded) / 32.0)) - 1u;
    if owner == 0u || owner >= arrayLength(&point_actor_visibility) { return false; }
    return point_actor_visibility[owner] != 0u;
}

// Magnitude low five bits hold the one-based slot; negative values carry exact owner group.
fn point_shadow_visibility(delta: vec3<f32>, range: f32, packed_slot: f32, character: bool) -> f32 {
    if !owner_light_admitted(packed_slot) || (packed_slot < 0.0 && character) { return 1.0; }
    let encoded_slot = u32(abs(packed_slot)) % 32u;
    if encoded_slot == 0u { return 1.0; }
    let slot = encoded_slot - 1u;
    if slot >= 16u { return 1.0; }
    let metadata = point_shadow_metadata.slots[slot];
    if metadata.x <= 0.0 || metadata.y < 0.0 { return 1.0; }
    if metadata.y > 0.0 {
        let owner = u32(metadata.y);
        if owner >= arrayLength(&point_actor_visibility) { return 1.0; }
        if point_actor_visibility[owner] == 0u { return 1.0; }
    }
    let axis_distance = max(max(abs(delta.x), abs(delta.y)), abs(delta.z));
    let near = 0.1;
    if axis_distance <= near || axis_distance >= range { return 1.0; }
    // Face raster and receiver share the same near/range perspective equation.
    let depth = range / (range - near) - range * near / ((range - near) * axis_distance);
    let visibility = textureSampleCompareLevel(point_shadow_maps, point_shadow_sampler,
        delta, i32(slot), depth - 0.00015);
    return mix(1.0, visibility, clamp(metadata.x, 0.0, 1.0));
}
