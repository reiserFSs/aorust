// Adapted from ao-vk rvk/shaders/dof_common.glsl and dof_gather.frag.
// MIT, Copyright (c) 2026 wannabuh; see THIRD_PARTY.
// https://github.com/wannabuh/ao-vk/tree/a0dc47ea2c853d26a4491d5a25ef0331f714b6ce
// Full-resolution pixel-disc gather; exact own-character view depth supplied by CPU.
@group(0) @binding(6) var<storage,read> aperture: array<vec4<f32>>;
fn inverse_view_z(p: vec2<i32>) -> f32 {
    let d = textureLoad(depth,p,0);
    return params.effect.w-d*(params.effect.w-params.fog_color.w);
}
fn coc(inverse_z: f32) -> f32 {
    let t = 1.0-params.projection.z*min(inverse_z,1000.0);
    let magnitude = max(abs(t)-0.025,0.0)/0.975 * params.projection.w;
    // A hard clamp made all far geometry beyond ~2x focus equally blurred.
    // Keep near/far sign and approach the configured pixel cap continuously.
    return sign(t)*magnitude/(1.0+magnitude)*params.effect.x;
}
@fragment fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy); let original = textureLoad(image,p,0);
    if params.projection.z <= 0.0 { return original; }
    let signed_radius = coc(inverse_view_z(p)); let radius = abs(signed_radius);
    if radius < 0.5 { return original; }
    var sum = vec3<f32>(0.0); var weight = 0.0;
    // Every integer pixel in the aperture remains covered. Distances are
    // computed once on CPU, sorted to stop before out-of-disc entries.
    let gather_radius = abs(coc(select(params.effect.w,params.fog_color.w,signed_radius>0.0)));
    let centre_fog = fog_fraction(p);
    for (var i = 0u; i < arrayLength(&aperture); i += 1u) {
        let tap = aperture[i];
        let r = tap.z;
        if r > gather_radius+0.5 { break; }
        let q = coord(p+vec2<i32>(tap.xy));
        if fog_fraction(q) < centre_fog { continue; }
        let sample_radius = coc(inverse_view_z(q));
        // Same signed defocus layer and source-aperture support; no focused
        // surface or opposite-side contribution, and no 1% depth truncation.
        if sample_radius*signed_radius <= 0.0 { continue; }
        let cover = clamp(abs(sample_radius)-r+0.5,0.0,1.0);
        if cover <= 0.0 { continue; }
        let c = colour(q);
        let brightness = luma(c);
        let w = cover*(1.0+4.0*brightness*brightness);
        sum += c*w; weight += w;
    }
    if weight <= 0.0 { return original; }
    return safe_output(sum/weight,original.rgb,original.a,p);
}
