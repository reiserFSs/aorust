// Enhanced v2 final stage: CAS on the TAA output, exposure, filmic shoulder, grade, vignette, dither, encode to the 2.2-gamma swapchain.
// `common.wgsl`, `packing.wgsl`, `post_common.wgsl` precede this file.
@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var taat: texture_2d<f32>;
@group(0) @binding(2) var deptht: texture_depth_2d;
@group(0) @binding(8) var<uniform> ctl: Ctl;

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vi], 0.0, 1.0);
}

// Identity below the knee (the authored mid-tones stay exactly as authored), soft exponential shoulder above.
fn shoulder(x: vec3<f32>) -> vec3<f32> {
    let k = 0.72;
    let over = max(x - vec3<f32>(k), vec3<f32>(0.0));
    return min(x, vec3<f32>(k)) + (1.0 - k) * (vec3<f32>(1.0) - exp(-over / (1.0 - k)));
}

fn hash12(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

const CAS_GAIN: f32 = 0.5;
// Contrast-sharpening (AMD CAS shape) in tone-compressed space; `strength` already includes the fog gate.
fn cas(px: vec2<i32>, centre: vec3<f32>, strength: f32) -> vec3<f32> {
    let dims = vec2<i32>(fc.viewport.xy) - vec2<i32>(1);
    let b = tm(textureLoad(taat, clamp(px + vec2<i32>(0, -1), vec2<i32>(0), dims), 0).rgb);
    let d = tm(textureLoad(taat, clamp(px + vec2<i32>(-1, 0), vec2<i32>(0), dims), 0).rgb);
    let f = tm(textureLoad(taat, clamp(px + vec2<i32>(1, 0), vec2<i32>(0), dims), 0).rgb);
    let h = tm(textureLoad(taat, clamp(px + vec2<i32>(0, 1), vec2<i32>(0), dims), 0).rgb);
    let e = tm(centre);
    let mn = min(min(min(b, d), min(e, f)), h);
    let mx = max(max(max(b, d), max(e, f)), h);
    let amp = sqrt(clamp(min(mn, vec3<f32>(1.0) - mx) / max(mx, vec3<f32>(1e-4)), vec3<f32>(0.0), vec3<f32>(1.0)));
    let w = -amp / mix(8.0, 5.0, strength);
    return tm_inv(clamp(((b + d + f + h) * w + e) / (1.0 + 4.0 * w), mn, mx));
}

// Display grade. Saturation and contrast fade to identity over fogged geometry (native transmittance < 0.5: weight 0, full
// weight from 0.75), so the grade can never recover contrast the native fog removed; every other step is a monotone function of
// the pixel value alone, hence fully fogged pixels still equal the fog colour whatever is behind them.
fn grade(c_in: vec3<f32>, t_native: f32) -> vec3<f32> {
    let k = smoothstep(0.5, 0.75, t_native);
    var c = c_in * fc.grade.x;
    // Bright, saturated highlights roll to white instead of clipping per channel.
    let peak = max(c.r, max(c.g, c.b));
    c = mix(c, vec3<f32>(luma(c)), smoothstep(1.0, 3.0, peak) * 0.5);
    c = shoulder(c);
    c = mix(vec3<f32>(luma(c)), c, mix(1.0, fc.grade.y, k));
    return max(mix(vec3<f32>(0.18), c, mix(1.0, fc.grade.z, k)), vec3<f32>(0.0)); // contrast pivots on mid grey
}

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    var c = textureLoad(taat, px, 0).rgb;
    let t = t_native_depth(textureLoad(deptht, px, 0));
    // CAS is skipped where the native fog has taken the detail away (T_native < 0.15).
    // CAS_GAIN: full-strength CAS measurably re-sharpens alpha-tested canopy edges into speckle (isolated bright pixels +13% vs none).
    let sharp = CAS_GAIN * ctl.cas * smoothstep(0.15, 0.25, t);
    if sharp > 0.0 { c = cas(px, c, sharp); }
    c = grade(c, t);
    let uv = (frag.xy + vec2<f32>(0.5)) * fc.viewport.zw;
    let d = (uv - vec2<f32>(0.5)) * vec2<f32>(fc.proj.w, 1.0);
    c *= 1.0 - fc.grade.w * dot(d, d) * 1.6;
    var g = pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
    let n = hash12(frag.xy + vec2<f32>(f32(fc.flags.x & 255u) * 17.0, 3.0)) + hash12(frag.yx * 1.37) - 1.0;
    g += vec3<f32>(n / 255.0);
    return vec4<f32>(g, 1.0);
}
