// Enhanced semantic plant cards only. Distance never removes fog or admission.
fn foliage_detail_distant(world: vec3<f32>) -> bool {
    let delta = world - g.eye.xyz;
    return (u32(mat.native.z) & 64u) != 0u && dot(delta, delta) > 2304.0;
}

// Native outdoor plant cards transmit authored sunlight through their backs.
// This is added before the unchanged native fog equation, never after it.
fn foliage_detail_transmission(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    if (u32(mat.native.z) & 64u) == 0u || native_unlit() || mat.emissive.w > 0.5 { return vec3<f32>(0.0); }
    let n = normal / max(length(normal), 0.0001);
    let view = (g.eye.xyz - world) / max(length(g.eye.xyz - world), 0.0001);
    let through_leaf = max(-dot(n, g.sun_dir.xyz), 0.0);
    let toward_eye = pow(max(-dot(view, g.sun_dir.xyz), 0.0), 2.0);
    let visible = sun_visibility(world, n, pixel);
    return max(g.sun_g.rgb, vec3<f32>(0.0)) * (0.15 * through_leaf * toward_eye * visible);
}
