// Authored native fog hue supplies distant-land tint; no geometry cutoff.
fn land_haze_tau(distance: f32, z: f32) -> f32 {
    if z >= 1.0 { return 0.0; }
    // Fade admitted ships/skylines into the authored horizon BEFORE clipping.
    // Four optical depths compensate the hue profile's minimum channel weight.
    let far_tau = -4.0 * log(max(1.0 - smoothstep(0.85 * a.eye.w, a.eye.w, distance), 0.00000001));
    return 0.002 * distance * smoothstep(0.12 * a.eye.w, 0.9 * a.eye.w, distance) + far_tau;
}
