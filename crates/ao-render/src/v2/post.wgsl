// Enhanced v2 final stage: exposure, filmic shoulder, grade, vignette, dither, encode to the 2.2-gamma swapchain.
// `common.wgsl` precedes this file.
@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var hdr: texture_2d<f32>;

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

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    var c = textureLoad(hdr, px, 0).rgb * fc.grade.x;
    // Bright, saturated highlights roll to white instead of clipping per channel.
    let peak = max(c.r, max(c.g, c.b));
    c = mix(c, vec3<f32>(luma(c)), smoothstep(1.0, 3.0, peak) * 0.5);
    c = shoulder(c);
    let l = luma(c);
    c = mix(vec3<f32>(l), c, fc.grade.y);
    c = max(mix(vec3<f32>(0.18), c, fc.grade.z), vec3<f32>(0.0)); // contrast pivots on mid grey
    let uv = (frag.xy + vec2<f32>(0.5)) * fc.viewport.zw;
    let d = (uv - vec2<f32>(0.5)) * vec2<f32>(fc.proj.w, 1.0);
    c *= 1.0 - fc.grade.w * dot(d, d) * 1.6;
    var g = pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
    let n = hash12(frag.xy + vec2<f32>(f32(fc.flags.x & 255u) * 17.0, 3.0)) + hash12(frag.yx * 1.37) - 1.0;
    g += vec3<f32>(n / 255.0);
    return vec4<f32>(g, 1.0);
}
