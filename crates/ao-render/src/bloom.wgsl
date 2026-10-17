// Authored emission only: never threshold beauty. Zero-border averages never
// increase integrated energy, including odd sizes; rejected/edge energy is lost.
struct Params { strength: f32, _pad0: f32, _pad1: f32, _pad2: f32 }
@group(0) @binding(0) var fine: texture_2d<f32>;
@group(0) @binding(1) var coarse: texture_2d<f32>;
@group(0) @binding(2) var seed: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: Params;
@vertex fn fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
fn load_fine(p: vec2<i32>) -> vec4<f32> {
    if any(p < vec2<i32>(0)) || any(p >= vec2<i32>(textureDimensions(fine))) { return vec4<f32>(0.0); }
    return textureLoad(fine, p, 0);
}
fn downsample(p: vec4<f32>, bounded: bool) -> vec4<f32> {
    var rgb = vec3<f32>(0.0);
    let origin = vec2<i32>(p.xy) * 2;
    // Fused tent then 2x2 box: [1,3,3,1]/8. Sixteen unique taps,
    // zero border, no edge renormalization and no invented edge energy.
    for (var y = -1; y <= 2; y++) {
        for (var x = -1; x <= 2; x++) {
            let q = origin + vec2<i32>(x, y);
            var value = load_fine(q);
            if bounded && all(q >= vec2<i32>(0)) && all(q < vec2<i32>(textureDimensions(fine))) {
                value = vec4<f32>(min(max(value.rgb, vec3<f32>(0.0)), clamp(textureLoad(seed, q, 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0))), 0.0);
            }
            let weight = select(3.0, 1.0, x == -1 || x == 2) * select(3.0, 1.0, y == -1 || y == 2) / 64.0;
            rgb += value.rgb * weight;
        }
    }
    return vec4<f32>(rgb, 0.0);
}
@fragment fn down(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return downsample(p, false); }
@fragment fn seed_down(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return downsample(p, true); }
@fragment fn up(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    // ponytail: nearest expansion is conservative even at odd edges; use an
    // energy-accounted tent reconstruction if coarse-level blocks are visible.
    let q = vec2<i32>(p.xy);
    let a = textureLoad(fine, q, 0);
    let b = textureLoad(coarse, q / 2, 0);
    return vec4<f32>((a.rgb + b.rgb) * 0.5, 0.0);
}
@fragment fn composite(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(p.xy);
    let raw_beauty = textureLoad(fine, q, 0);
    let beauty = vec4<f32>(clamp(raw_beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), raw_beauty.a);
    let emission = textureLoad(seed, q, 0);
    let halo = textureLoad(coarse, q / 2, 0);
    let strength = clamp(params.strength, 0.0, 1.0);
    let credit = min(max(emission.rgb, vec3<f32>(0.0)), beauty.rgb) * strength;
    // Lens glare only: world lighting is unchanged. Its energy is paid by
    // dimming visible emission, never by extracting ordinary bright surfaces.
    var received = halo.rgb * strength;
    // Source visibility comes ONLY from the world pass's depth-tested mask.
    // Never classify/gate receivers: doing so could silhouette a dark actor.
    // Net lens-glare luma is capped at 24 display-code levels. Reserve f16
    // rounding headroom; the pyramid's falloff and source debit bound energy.
    let luma = vec3<f32>(0.2126, 0.7152, 0.0722);
    let safe_cap = 24.0 / 255.0 - 1.0 / 2048.0;
    let received_luma = dot(received, luma);
    let credit_luma = dot(credit, luma);
    if received_luma - credit_luma > safe_cap {
        received *= (credit_luma + safe_cap) / received_luma;
    }
    return vec4<f32>(clamp(beauty.rgb - credit + received, vec3<f32>(0.0), vec3<f32>(1.0)), beauty.a);
}
