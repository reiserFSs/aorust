// Current-frame native cascades, including authored moon maps from the parent.
fn mapped_visibility(p: vec3<f32>) -> f32 {
    let world = p + a.eye.xyz;
    let view_depth = dot(world - shadow_frame.eye.xyz, shadow_frame.forward.xyz);
    if view_depth <= shadow_frame.splits.w || view_depth >= shadow_frame.eye.w || shadow_frame.forward.w <= 0.0 { return 1.0; }
    var layer = 0u;
    if view_depth > shadow_frame.splits.x { layer = 1u; }
    if view_depth > shadow_frame.splits.y { layer = 2u; }
    let h = shadow_frame.matrices[layer] * vec4<f32>(world, 1.0);
    let clip = h.xyz / h.w;
    let uv = clip.xy * vec2<f32>(0.5, -0.5) + 0.5;
    if any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) || clip.z <= 0.0 || clip.z >= 1.0 { return 1.0; }
    return textureSampleCompareLevel(shadow_maps, shadow_sampler, uv, i32(layer), clip.z - 0.00002);
}
@fragment fn shafts(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = full_pixel(vec2<i32>(position.xy));
    let z = scene_depth(pixel);
    let end = point(pixel, min(z, 0.999999));
    let reach = min(length(end), min(a.eye.w, shadow_frame.eye.w));
    let direction = normalize(end);
    var lighting = vec2<f32>(0.0);
    // True world-space ray march; no screen-space radial approximation/history.
    for (var i = 0u; i < 48u; i++) {
        let q = direction * (reach * (f32(i) + 0.5) / 48.0);
        let lit = mapped_visibility(q) * gap_transmittance(q);
        lighting += vec2<f32>(1.0 - lit, lit) * native_remaining(q);
    }
    return encode_transmittance(end, z, pixel, lighting / 48.0);
}
