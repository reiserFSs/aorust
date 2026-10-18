// ao-vk rvk/pointshadow.cpp face order and perspective depth (MIT; see THIRD_PARTY).
// Cached faces retain their own world-to-clip transform, including the light origin.
fn point_shadow_face(delta: vec3<f32>) -> u32 {
    let axis = abs(delta);
    if axis.x >= axis.y && axis.x >= axis.z {
        return select(1u, 0u, delta.x >= 0.0);
    }
    if axis.y >= axis.z {
        return select(3u, 2u, delta.y >= 0.0);
    }
    return select(5u, 4u, delta.z >= 0.0);
}

fn point_cached_face_visibility(
    maps: texture_depth_2d_array,
    matrix: mat4x4<f32>,
    world: vec3<f32>,
    layer: i32,
    resolution_ratio: f32,
) -> f32 {
    if !(resolution_ratio > 0.0 && resolution_ratio <= 1.0) { return 1.0; }
    let clip = matrix * vec4<f32>(world, 1.0);
    // A moving light can put the receiver outside this face's OLD frustum.
    // Such a seam is unshadowed: clamping an outside projection would smear
    // unrelated old edge depth onto the receiver and create ghost occlusion.
    if !(clip.w > 0.0 && clip.z >= 0.0 && clip.z <= clip.w
        && all(abs(clip.xy) <= vec2<f32>(clip.w))) { return 1.0; }
    let projected = clip.xyz / clip.w;
    // Raster clip Y is up; native cube T (and the 2D depth attachment) is down.
    let uv = vec2<f32>(projected.x, -projected.y) * 0.5 + vec2<f32>(0.5);
    let half_texel = vec2<f32>(0.5) / vec2<f32>(textureDimensions(maps));
    let scaled_uv = clamp(uv * resolution_ratio, half_texel,
        vec2<f32>(resolution_ratio) - half_texel);
    // Stored perspective projection supplies the cached near/range depth,
    // rather than incorrectly recomputing it from the current light origin.
    return textureSampleCompareLevel(maps, point_shadow_sampler, scaled_uv,
        layer, projected.z - 0.00015);
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
    if axis_distance <= 0.1 || axis_distance >= range { return 1.0; }
    let face_index = slot * 6u + point_shadow_face(delta);
    let face = point_shadow_metadata.faces[face_index];
    let world = point_shadow_metadata.lights[slot].xyz + delta;
    var visibility = 1.0;
    if face.params.x > 0.0 {
        visibility = point_cached_face_visibility(point_shadow_maps,
            face.static_matrix, world, i32(face_index), face.params.y);
    }
    if face.params.w > 0.0 {
        visibility = min(visibility, point_cached_face_visibility(point_actor_shadow_maps,
            face.dynamic_matrix, world, i32(face_index), face.params.z));
    }
    return mix(1.0, visibility, clamp(metadata.x, 0.0, 1.0));
}
