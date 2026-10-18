// Shared by bloom.wgsl (pyramid, UNORM composite) and grade.wgsl (HDR present). Pure functions only.
// Glare energy is carried as rgb = authored seed + highlight seed, a = luma of the highlight share.
const GLARE_GAIN: f32 = 4.0;
// Authored emitters (night lamps, light bars) keep the 64-display-code luma cap; highlight energy
// (daylight surfaces the expose pass pushed above white) is bounded by its seed instead.
const GLARE_CAP: f32 = 64.0 / 255.0 - 1.0 / 2048.0;
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
fn glare_load(t: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    if any(p < vec2<i32>(0)) || any(p >= vec2<i32>(textureDimensions(t))) { return vec4<f32>(0.0); }
    return textureLoad(t, p, 0);
}
// Zero-border bilinear fetch at texel-space position `c` (texel i has its centre at c = i).
fn glare_bilinear(t: texture_2d<f32>, c: vec2<f32>) -> vec4<f32> {
    let base = vec2<i32>(floor(c));
    let f = fract(c);
    let a = mix(glare_load(t, base), glare_load(t, base + vec2<i32>(1, 0)), f.x);
    let b = mix(glare_load(t, base + vec2<i32>(0, 1)), glare_load(t, base + vec2<i32>(1, 1)), f.x);
    return mix(a, b, f.y);
}
// Hardware filtering in the interior; keep the exact zero border (not clamp-to-edge) at the rim.
// The explicit sampler keeps HDR presentation's existing sampler-free bindings unchanged.
fn glare_filtered(t: texture_2d<f32>, s: sampler, c: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(t));
    if all(c >= vec2<f32>(0.0)) && all(c < dims - 1.0) {
        return textureSampleLevel(t, s, (c + 0.5) / dims, 0.0);
    }
    return glare_bilinear(t, c);
}
// Fine-pixel centre `p` (fragment position) into the half-resolution `halo`.
fn glare_reconstruct(t: texture_2d<f32>, p: vec2<f32>) -> vec4<f32> {
    return glare_bilinear(t, p * 0.5 - 0.5);
}
// Light added by the halo: authored share capped in luma, highlight share uncapped.
fn glare_received(halo: vec4<f32>, strength: f32) -> vec3<f32> {
    let received = max(halo.rgb, vec3<f32>(0.0)) * (clamp(strength, 0.0, 1.0) * GLARE_GAIN);
    let luma = dot(received, LUMA);
    let share = clamp(halo.a * (clamp(strength, 0.0, 1.0) * GLARE_GAIN) / max(luma, 1e-6), 0.0, 1.0);
    var authored = received * (1.0 - share);
    let authored_luma = dot(authored, LUMA);
    if authored_luma > GLARE_CAP { authored *= GLARE_CAP / authored_luma; }
    return authored + received * share;
}
