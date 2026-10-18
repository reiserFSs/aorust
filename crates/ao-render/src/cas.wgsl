// Adapted from ao-vk rvk/shaders/sharpen.frag, MIT, Copyright (c) 2026 wannabuh.
// https://github.com/wannabuh/ao-vk/tree/a0dc47ea2c853d26a4491d5a25ef0331f714b6ce
// See THIRD_PARTY. Sharpen the bounded surface term, retaining native extinction.
fn recovered_surface(p: vec2<i32>) -> vec3<f32> {
    let fog = fog_fraction(p);
    return clamp((colour(p)-fog*params.fog_color.rgb)/max(1.0-fog,0.0001),vec3(0.0),vec3(1.0));
}
fn cas_surface(p: vec2<i32>, centre: vec2<i32>) -> vec3<f32> {
    let z = view_z(centre);
    if fog_fraction(p) >= 1.0 || abs(view_z(p)-z) > max(0.01,z*0.01) { return recovered_surface(centre); }
    return recovered_surface(p);
}
@fragment fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy); let original = textureLoad(image,p,0);
    let fog = fog_fraction(p);
    if params.effect.y <= 0.0 || fog >= 1.0 { return original; }
    let e = recovered_surface(p);
    let b = cas_surface(p+vec2(0,-1),p);
    let d = cas_surface(p+vec2(-1,0),p);
    let f = cas_surface(p+vec2(1,0),p);
    let h = cas_surface(p+vec2(0,1),p);
    let low = min(e,min(min(b,d),min(f,h)));
    let high = max(e,max(max(b,d),max(f,h)));
    let amplitude = sqrt(clamp(min(low,vec3(1.0)-high)/max(high,vec3(0.0001)),vec3(0.0),vec3(1.0)));
    let w = -amplitude/mix(8.0,5.0,params.effect.y);
    let sharpened = (e+w*(b+d+f+h))/(vec3(1.0)+4.0*w);
    let surface = mix(e,clamp(sharpened,vec3(0.0),vec3(1.0)),params.effect.y);
    // Delta form keeps any quantization/prior-pass residual neutral and retains
    // the same fog colour and transmittance instead of sharpening fog itself.
    return safe_output(original.rgb+(surface-e)*(1.0-fog),original.rgb,original.a,p);
}
