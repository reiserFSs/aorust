// Appended after the shared sun-shadow receiver when Enhanced sun shadows are on.
// A blade is a few pixels wide and takes the shadow at its root, so the world's 3x3 (or soft-shadow) PCF per
// fragment would be spent on the same answer thousands of times: one hardware-filtered comparison per fragment
// instead, with the receiver plane's slope as the bias. Same cascades, splits and blends as `sun_visibility`.
fn grass_cascade_visibility(world: vec3<f32>, normal: vec3<f32>, layer: u32) -> f32 {
    let matrix = shadow_frame.matrices[layer];
    let clip = matrix * vec4<f32>(world, 1.0);
    let p = clip.xyz / clip.w;
    let uv = p.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let dimensions = vec2<f32>(textureDimensions(shadow_maps));
    let guard = vec2<f32>(2.0) / dimensions;
    if any(uv <= guard) || any(uv >= vec2<f32>(1.0) - guard) || p.z <= 0.0 || p.z >= 1.0 { return 1.0; }
    let row_x = vec3<f32>(matrix[0].x, matrix[1].x, matrix[2].x);
    let row_y = vec3<f32>(matrix[0].y, matrix[1].y, matrix[2].y);
    let row_z = vec3<f32>(matrix[0].z, matrix[1].z, matrix[2].z);
    let plane = vec3<f32>(dot(normal, row_x) / dot(row_x, row_x),
        dot(normal, row_y) / dot(row_y, row_y), dot(normal, row_z) / dot(row_z, row_z));
    var slope = 0.0;
    if abs(plane.z) > 0.000001 { slope = (abs(plane.x) + abs(plane.y)) * 2.0 / abs(plane.z); }
    let depth = p.z - 0.00002 - slope * 0.5 / dimensions.x;
    return textureSampleCompareLevel(shadow_maps, shadow_sampler, uv, i32(layer), depth);
}
fn grass_sun_visibility(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    if shadow_frame.forward.w <= 0.0 { return 1.0; }
    let depth = dot(world - shadow_frame.eye.xyz, shadow_frame.forward.xyz);
    if depth < shadow_frame.splits.w || depth >= shadow_frame.splits.z { return 1.0; }
    var layer = 0u;
    if depth > shadow_frame.splits.x { layer = 1u; }
    if depth > shadow_frame.splits.y { layer = 2u; }
    var visible = grass_cascade_visibility(world, normal, layer);
    let start = select(shadow_frame.splits.w, shadow_frame.splits[layer - min(layer, 1u)], layer > 0u);
    let end = shadow_frame.splits[layer];
    let blend = smoothstep(end - (end - start) * 0.1, end, depth);
    if layer < 2u { visible = mix(visible, grass_cascade_visibility(world, normal, layer + 1u), blend); }
    else { visible = mix(visible, 1.0, blend); }
    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));
}
