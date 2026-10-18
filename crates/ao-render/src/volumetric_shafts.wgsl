// Current-frame native cascades, including authored moon maps from the parent.
fn mapped_visibility(world: vec3<f32>, view_depth: f32) -> f32 {
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
    // Cascade coverage bounds occluders, not the independent atmospheric ray.
    let reach = min(length(end), a.eye.w);
    let direction = end / max(length(end), 0.00001);
    // Beyond native fog extinction every integration weight is exactly zero.
    // Preserve the original interval's normalization when omitting that tail.
    let active_reach = min(reach, a.fog.w);
    let fog_range = max(a.fog.w - a.profile.y, 0.001);
    let view_origin = dot(a.eye.xyz - shadow_frame.eye.xyz, shadow_frame.forward.xyz);
    let view_step = dot(direction, shadow_frame.forward.xyz);
    var lighting = vec2<f32>(0.0);
    // Quadratic strata spend more samples near admitted cascade occluders.
    // Interval weights preserve the integral; fixed midpoints need no history.
    for (var i = 0u; i < 16u; i++) {
        let u0 = f32(i) / 16.0;
        let u1 = f32(i + 1u) / 16.0;
        let weight = u1 * u1 - u0 * u0;
        let distance = active_reach * 0.5 * (u0 * u0 + u1 * u1);
        let q = direction * distance;
        let remaining = clamp((a.fog.w - distance) / fog_range, 0.0, 1.0);
        let lit = mapped_visibility(q + a.eye.xyz, view_origin + view_step * distance) * gap_transmittance(q);
        lighting += vec2<f32>(1.0 - lit, lit) * remaining * weight;
    }
    return encode_transmittance(end, z, pixel, lighting * (active_reach / max(reach, 0.00001)));
}
