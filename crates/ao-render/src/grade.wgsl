// Enhanced daylight exposure; roof-tested terrain can receive mild sky fill.
// Sky fill changes only surface radiance, never the native fog contribution or extinction.
struct Params {
    control: vec4<f32>, // x: peak sun boost (0 = faithful), y: bloom strength (0 = no halo)
    grade: vec4<f32>,   // x: chroma grade strength, y: day, z: low-sun, w: night weights
    fog: vec4<f32>,     // native gamma-space terrain fog colour
    extras: vec4<f32>,  // contrast, saturation, warmth (0.5 neutral), vignette
    flags: vec4<f32>,   // LUT enabled, night cool, controls initialized, reserved
    day_min: vec4<f32>,
    day_max: vec4<f32>,
    night_min: vec4<f32>,
    night_max: vec4<f32>,
    lut_size: vec4<f32>,
}
@group(0) @binding(0) var source: texture_2d<f32>;
@group(1) @binding(0) var sun: texture_2d<f32>;
@group(1) @binding(1) var<uniform> params: Params;
@group(1) @binding(2) var halo: texture_2d<f32>;
@group(1) @binding(3) var<storage, read> day_lut: array<vec4<f32>>;
@group(1) @binding(4) var<storage, read> night_lut: array<vec4<f32>>;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
// Share ramp is shared with the chroma grade; authored day, not moonlight share, gates added light.
fn sunlit_at(q: vec2<i32>) -> f32 {
    return smoothstep(0.12, 0.5, textureLoad(sun, q, 0).r);
}
// The native framebuffer is gamma-space: a full 60% direct lift washed out stone.
// Keep direct exposure at 60% of the control, without reducing restored open shade.
fn boost_at(q: vec2<i32>) -> f32 {
    let sky_open = textureLoad(sun, q, 0).g;
    return params.control.x * params.grade.y * 0.6 * max(sunlit_at(q), sky_open);
}
@fragment fn expose(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(p.xy);
    let c = textureLoad(source, q, 0);
    let rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let surface = max(rgb - params.fog.rgb * textureLoad(sun, q, 0).b, vec3<f32>(0.0));
    return vec4<f32>(rgb + surface * boost_at(q), c.a);
}
// Identity up to a knee, then a cubic that reaches 1.0 exactly at the pixel's own boosted white.
// Monotone for beta <= 0.6 (k = 1 - beta/2 makes the end tangent exactly 3x the chord).
fn tone(x: f32, beta: f32) -> f32 {
    if beta <= 0.0001 { return min(x, 1.0); }
    let top = 1.0 + beta;
    let k = min(0.85, 1.0 - 0.5 * beta);
    if x <= k { return x; }
    if x >= top { return 1.0; }
    let d = top - k;
    let u = (x - k) / d;
    let u2 = u * u;
    let u3 = u2 * u;
    return (2.0 * u3 - 3.0 * u2 + 1.0) * k + (u3 - 2.0 * u2 + u) * d + (3.0 * u2 - 2.0 * u3);
}
// Fixed screen-space interleaved-gradient noise: stable between identical frames. +-0.35 code
// levels never moves an exact 8-bit code (f16 round-trip error is under 0.07 level).
fn dither(p: vec2<f32>) -> f32 {
    return (fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715)))) - 0.5) * 0.7 / 255.0;
}
fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }
// Chroma-only grade: saturation plus a warm sunlit / cool shade / cool night-dark tint.
// Renormalisation and same-luma gamut compression preserve input luminance in both directions,
// so grading changes neither fog-to-geometry contrast nor black.
fn grade_chroma(rgb: vec3<f32>, sunlit: f32) -> vec3<f32> {
    let strength = params.grade.x;
    let y = luma(rgb);
    if strength <= 0.0 || y <= 0.0 { return rgb; }
    let day = params.grade.y;
    let low = params.grade.z;
    let night = params.grade.w;
    var c = vec3<f32>(y) + (rgb - vec3<f32>(y)) * (1.0 + 0.18 * strength * day);
    let warm = day * mix(0.65, 1.0, low) * sunlit;
    let cool = day * 0.25 * (1.0 - sunlit) + night * (1.0 - smoothstep(0.0, 0.55, y));
    c *= vec3<f32>(1.0) + strength * (warm * vec3<f32>(0.20, 0.015, -0.22) + cool * vec3<f32>(-0.06, -0.005, 0.10));
    // Match luminance in both directions, then compress chroma toward the same-luma grey
    // to fit the display gamut. Clamping first used to throw away daylight luminance.
    c *= y / max(luma(c), 1e-5);
    let delta = c - vec3<f32>(y);
    var gamut = 1.0;
    for (var channel = 0; channel < 3; channel++) {
        if delta[channel] > 0.0 { gamut = min(gamut, (1.0 - y) / delta[channel]); }
        if delta[channel] < 0.0 { gamut = min(gamut, -y / delta[channel]); }
    }
    return vec3<f32>(y) + delta * gamut;
}

fn lut_fetch(q: vec3<u32>, size: u32, night: bool) -> vec3<f32> {
    let i = q.x + size * (q.y + size * q.z);
    if night { return night_lut[i].rgb; }
    return day_lut[i].rgb;
}
// Eight bounded storage-buffer reads implement trilinear 3D .cube sampling.
fn sample_lut(rgb: vec3<f32>, night: bool) -> vec3<f32> {
    let lo = select(params.day_min.rgb, params.night_min.rgb, night);
    let hi = select(params.day_max.rgb, params.night_max.rgb, night);
    let size = u32(select(params.lut_size.x, params.lut_size.y, night));
    let p = clamp((rgb - lo) / (hi - lo), vec3<f32>(0.0), vec3<f32>(1.0)) * f32(size - 1u);
    let a = vec3<u32>(floor(p));
    let b = min(a + vec3<u32>(1u), vec3<u32>(size - 1u));
    let f = fract(p);
    return mix(mix(mix(lut_fetch(a, size, night), lut_fetch(vec3<u32>(b.x, a.y, a.z), size, night), f.x),
                   mix(lut_fetch(vec3<u32>(a.x, b.y, a.z), size, night), lut_fetch(vec3<u32>(b.xy, a.z), size, night), f.x), f.y),
               mix(mix(lut_fetch(vec3<u32>(a.xy, b.z), size, night), lut_fetch(vec3<u32>(b.x, a.y, b.z), size, night), f.x),
                   mix(lut_fetch(vec3<u32>(a.x, b.yz), size, night), lut_fetch(b, size, night), f.x), f.y), f.z);
}
fn same_luma(c: vec3<f32>, y: f32) -> vec3<f32> {
    if y <= 0.0 { return vec3<f32>(0.0); }
    if luma(c) <= 1e-6 { return vec3<f32>(y); }
    let delta = c * (y / luma(c)) - vec3<f32>(y);
    var gamut = 1.0;
    for (var channel = 0; channel < 3; channel++) {
        if delta[channel] > 0.0 { gamut = min(gamut, (1.0 - y) / delta[channel]); }
        if delta[channel] < 0.0 { gamut = min(gamut, -y / delta[channel]); }
    }
    return vec3<f32>(y) + delta * gamut;
}
fn extra_grade(rgb: vec3<f32>) -> vec3<f32> {
    let y = luma(rgb);
    if y <= 0.0 { return rgb; }
    var c = rgb;
    if params.flags.z > 0.0 {
        // User-selected contrast may change day/night surface radiance.
        let curved = mix(vec3<f32>(0.5), c, min(params.extras.x, 1.0));
        let contrasted = mix(curved, smoothstep(vec3<f32>(0.0), vec3<f32>(1.0), c), max(params.extras.x - 1.0, 0.0));
        c = contrasted;
        let contrast_y = luma(c);
        c = vec3<f32>(contrast_y) + (c - vec3<f32>(contrast_y)) * params.extras.y;
        let warmth = (params.extras.z - 0.5) * 0.4;
        c *= vec3<f32>(1.0 + warmth, 1.0, 1.0 - warmth);
        c *= vec3<f32>(1.0) + params.flags.y * params.grade.w * vec3<f32>(-0.06, -0.005, 0.10);
        c = same_luma(c, contrast_y);
    }
    if params.flags.x > 0.0 {
        c = mix(sample_lut(c, true), sample_lut(c, false), params.grade.y);
    }
    return c;
}
fn grade_surface(rgb: vec3<f32>, q: vec2<i32>, p: vec2<f32>) -> vec3<f32> {
    let controls_neutral = params.flags.z <= 0.0 ||
        (params.extras.x == 1.0 && params.extras.y == 1.0 && params.extras.z == 0.5 && params.flags.y == 0.0);
    if controls_neutral && params.flags.x <= 0.0 && params.extras.w <= 0.0 { return rgb; }
    let fog = clamp(textureLoad(sun, q, 0).b, 0.0, 1.0);
    if fog >= 1.0 { return rgb; }
    // Recover only the surface term; preserve the native extinction coefficient and fog radiance.
    let transmission = 1.0 - fog;
    let surface = (rgb - params.fog.rgb * fog) / max(transmission, 1e-6);
    let bounded_surface = clamp(surface, vec3<f32>(0.0), vec3<f32>(1.0));
    let uv = p / vec2<f32>(textureDimensions(source));
    let edge = smoothstep(0.15, 0.72, length(uv - vec2<f32>(0.5)));
    let vignette = 1.0 - 0.65 * params.extras.w * edge;
    let graded = extra_grade(bounded_surface) * vignette;
    // Delta composition retains the native fog/quantization residual and exact identity codes.
    // For an in-gamut surface this is graded*T + fog*F with the identical native F.
    return rgb + (graded - bounded_surface) * transmission;
}
// Bloom halo lands here in HDR, above white. Pixels it reaches get the tone curve's white lifted by the
// halo's own size (`tone(x, beta + head)`): the pixel's own white then maps slightly below 1 and the halo's
// energy rolls into the shoulder instead of clipping at the pixel's old white. head = 0 (no bloom, or no halo
// there) is exactly the previous curve. Capped so the cubic stays in its monotone range (beta <= 0.6).
@fragment fn present(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(p.xy);
    let h = textureLoad(source, q, 0);
    let glare = glare_received(glare_reconstruct(halo, p.xy), params.control.y);
    let x = max(h.rgb, vec3<f32>(0.0)) + glare;
    let beta = min(boost_at(q) + max(glare.r, max(glare.g, glare.b)), 0.6);
    let mapped = max(vec3<f32>(tone(x.r, beta), tone(x.g, beta), tone(x.b, beta)), vec3<f32>(0.0));
    let sunlit = sunlit_at(q);
    let graded = grade_surface(grade_chroma(mapped, sunlit), q, p.xy);
    return vec4<f32>(clamp(graded + vec3<f32>(dither(p.xy)), vec3<f32>(0.0), vec3<f32>(1.0)), clamp(h.a, 0.0, 1.0));
}
