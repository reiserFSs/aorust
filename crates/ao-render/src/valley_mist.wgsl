// Stable terrain-height anchor and native local fog volumes, never camera height.
fn local_mist(p: vec3<f32>) -> f32 {
    var density = 0.0;
    for (var i = 0u; i < a.counts.x; i++) {
        let v = mist_volumes[i];
        let r = length(p - v.center_radius.xyz) / v.center_radius.w;
        density = max(density, v.density.x * max(0.0, 1.0 - r * r * r * r));
    }
    return density;
}
fn valley_mist_tau(p: vec3<f32>, distance: f32) -> f32 {
    let low = height_integral(-a.profile.x, p.y - a.profile.x, distance, 14.0);
    var local = 0.0;
    for (var i = 0u; i < 16u; i++) { local += local_mist(p * ((f32(i) + 0.5) / 16.0)); }
    return (0.004 * low + 0.012 * distance * local / 16.0) * a.profile.z;
}
