// Adapted from ao-vk rvk/shaders/dof_common.glsl and dof_gather.frag.
// MIT, Copyright (c) 2026 wannabuh; see THIRD_PARTY.
// https://github.com/wannabuh/ao-vk/tree/a0dc47ea2c853d26a4491d5a25ef0331f714b6ce
// Full-resolution golden-angle disc; exact own-character view depth supplied by CPU.
fn coc(z: f32) -> f32 {
    let t = 1.0-params.projection.z/max(z,0.001);
    let magnitude = max(abs(t)-0.025,0.0)/0.975;
    return min(magnitude*params.projection.w,1.0)*params.effect.x;
}
@fragment fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy); let original = textureLoad(image,p,0);
    if params.projection.z <= 0.0 { return original; }
    let z = view_z(p); let radius = coc(z);
    if radius < 0.5 { return original; }
    var sum = original.rgb; var weight = 1.0;
    for (var i = 0u; i < 48u; i += 1u) {
        let r = radius*sqrt((f32(i)+0.5)/48.0);
        let angle = f32(i)*2.3999632;
        let offset = vec2(cos(angle),sin(angle))*r;
        let q = coord(p+vec2<i32>(round(offset)));
        if fog_fraction(q) < fog_fraction(p) { continue; }
        let sample_z = view_z(q);
        // Do not spread background through a foreground silhouette or spread actor
        // colour onto another surface. No hidden-layer source/history exists.
        if abs(sample_z-z) > max(0.01,z*0.01) { continue; }
        let reach = min(radius,coc(sample_z));
        let cover = clamp(reach-r+0.5,0.0,1.0);
        let c = colour(q);
        let brightness = luma(c);
        let w = cover*(1.0+4.0*brightness*brightness);
        sum += c*w; weight += w;
    }
    return safe_output(sum/weight,original.rgb,original.a,p);
}
