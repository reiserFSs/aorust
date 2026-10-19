// Shared by the v2 bloom / TAA / post stages (after `common.wgsl` and `packing.wgsl`, which need `fc` declared first).

// Positive view distance of hardware depth `d` (standard-Z D32F, projection near/far in `fc.proj`); same value `view_depth(world_pos)` gives.
fn lin_z(d: f32) -> f32 {
    let n = fc.proj.x;
    let f = fc.proj.y;
    return n * f / (f + d * (n - f));
}
// Native fog transmittance of the geometry at hardware depth `d`; the sky (depth 1) is not fogged geometry: 1.
fn t_native_depth(d: f32) -> f32 {
    return select(native_transmittance(lin_z(d)), 1.0, d >= 1.0);
}

// Per-frame post controls (`post::Ctl`).
struct Ctl {
    bloom: f32,       // bloom gain (EnhancedSettings::bloom_strength)
    cas: f32,         // CAS sharpness 0..1 (EnhancedSettings::cas_strength)
    pad: vec2<f32>,
    taa: vec4<u32>,   // x = 1: discard the history (frame 0, resize, camera cut, admitted-actor set changed)
}

// Cheap reversible tone-compression used for neighbourhood statistics and sharpening of HDR values.
fn tm(c: vec3<f32>) -> vec3<f32> { return c / (1.0 + luma(c)); }
fn tm_inv(t: vec3<f32>) -> vec3<f32> { return t / max(1.0 - luma(t), 1e-4); }
