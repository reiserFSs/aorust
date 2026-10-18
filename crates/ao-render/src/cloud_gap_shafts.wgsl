// Sample the shared current-frame world-space procedural transmittance field.
// The field integrates native cloud layers once; shaft steps never re-march noise.
fn gap_transmittance(p: vec3<f32>) -> f32 {
    if a.counts.z == 0u || a.counts.y == 0u || a.cloud_sun.w < 0.5 || a.cloud_sun.y <= 0.001 || a.cloud_area.z <= 0.0 { return 1.0; }
    // A ground-plane field is valid below all cloud layers, not inside them.
    if p.y >= a.cloud_shift.w { return 1.0; }
    let relative = p + a.cloud_shift.xyz;
    let q = relative.xz + a.cloud_sun.xz / a.cloud_sun.y * (a.cloud_area.w - relative.y);
    let uv = (q - a.cloud_area.xy) / a.cloud_area.z;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { return 1.0; }
    let dimensions = vec2<i32>(textureDimensions(cloud_shadow_field));
    let texel = uv * vec2<f32>(dimensions) - 0.5;
    let lo = vec2<i32>(floor(texel));
    let f = fract(texel);
    let t00 = textureLoad(cloud_shadow_field, clamp(lo, vec2<i32>(0), dimensions - 1), 0).r;
    let t10 = textureLoad(cloud_shadow_field, clamp(lo + vec2<i32>(1,0), vec2<i32>(0), dimensions - 1), 0).r;
    let t01 = textureLoad(cloud_shadow_field, clamp(lo + vec2<i32>(0,1), vec2<i32>(0), dimensions - 1), 0).r;
    let t11 = textureLoad(cloud_shadow_field, clamp(lo + vec2<i32>(1,1), vec2<i32>(0), dimensions - 1), 0).r;
    return clamp(mix(mix(t00, t10, f.x), mix(t01, t11, f.x), f.y), 0.0, 1.0);
}
fn cloud_occlusion(p: vec3<f32>) -> vec2<f32> {
    if a.counts.z == 0u || a.counts.y == 0u { return vec2<f32>(0.0); }
    let reach = min(length(p), a.eye.w);
    let active_reach = min(reach, a.fog.w);
    let direction = p / max(length(p), 0.00001);
    let fog_range = max(a.fog.w - a.profile.y, 0.001);
    var lighting = vec2<f32>(0.0);
    for (var i = 0u; i < 24u; i++) {
        let distance = active_reach * (f32(i) + 0.5) / 24.0;
        let lit = gap_transmittance(direction * distance);
        lighting += vec2<f32>(1.0 - lit, lit) * clamp((a.fog.w - distance) / fog_range, 0.0, 1.0);
    }
    return lighting * (active_reach / max(reach, 0.00001) / 24.0);
}
