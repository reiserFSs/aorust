// Analytic exponential-height extinction; stable for horizontal/downhill rays.
fn height_integral(start_height: f32, end_height: f32, distance: f32, scale: f32) -> f32 {
    let lo = min(start_height, end_height) / scale;
    let difference = abs(end_height - start_height) / scale;
    let mean = select((1.0 - exp(-difference)) / max(difference, 0.0001), 1.0, difference < 0.0001);
    return distance * exp(-clamp(lo, -2.0, 60.0)) * mean;
}
fn height_haze_tau(p: vec3<f32>, distance: f32) -> f32 {
    return 0.0015 * height_integral(-a.profile.x, p.y - a.profile.x, distance, 100.0);
}
