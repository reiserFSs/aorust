// Enhanced semantic plant cards only. Distance never removes fog or admission.
fn foliage_detail_distant(world: vec3<f32>) -> bool {
    let delta = world - g.eye.xyz;
    return (u32(mat.native.z) & 64u) != 0u && dot(delta, delta) > 2304.0;
}

// Native outdoor plant cards transmit authored sunlight through their backs.
// This is added before the unchanged native fog equation, never after it.
fn foliage_detail_transmission(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    let sunlight = max(g.sun_g.rgb, vec3<f32>(0.0));
    if (u32(mat.native.z) & 64u) == 0u || native_unlit() || mat.emissive.w > 0.5
        || !any(sunlight > vec3<f32>(0.0)) || dot(normal, normal) < 0.00000001
        || dot(g.sun_dir.xyz, g.sun_dir.xyz) < 0.00000001 { return vec3<f32>(0.0); }
    let view = (g.eye.xyz - world) / max(length(g.eye.xyz - world), 0.0001);
    let sun = normalize(g.sun_dir.xyz); // aomac points TO the sun; rvk points from it.
    let n = normalize(normal);
    let facing = select(-n, n, dot(n, view) >= 0.0);
    // Cards are two-sided: artist winding must not decide whether a leaf glows.
    // Broad transmission plus forward scatter, as rvk's 0.5 through + 0.7 glare.
    let through_leaf = max(-dot(facing, sun), 0.0);
    let toward_eye = pow(max(dot(-view, sun), 0.0), 4.0);
    // Share the exact receiver plane with beauty; normalize only the lighting cosines.
    let visible = sun_visibility(world, normal, pixel);
    return sunlight * ((0.5 * through_leaf + 0.7 * toward_eye) * visible);
}
