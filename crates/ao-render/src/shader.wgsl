struct G {
    vp: mat4x4<f32>,
    eye: vec4<f32>,
    sun: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>, // x = start, y = end
}
@group(0) @binding(0) var<uniform> g: G;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
}
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

@vertex
fn vs(v: VIn) -> VOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let wp = model * vec4<f32>(v.pos, 1.0);
    var o: VOut;
    o.clip = g.vp * wp;
    o.wpos = wp.xyz;
    o.n = (model * vec4<f32>(v.normal, 0.0)).xyz;
    o.uv = v.uv;
    return o;
}

fn shade(i: VOut, alpha_test: bool) -> vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    if alpha_test && c.a < 0.5 {
        discard;
    }
    let to_eye = g.eye.xyz - i.wpos;
    let l = length(i.n);
    var n = select(vec3<f32>(0.0, 1.0, 0.0), i.n / l, l > 1e-4);
    if dot(n, to_eye) < 0.0 {
        n = -n; // two-sided: winding from the formats is not trusted
    }
    let light = vec3<f32>(0.35, 0.38, 0.45) + vec3<f32>(1.0, 0.95, 0.85) * max(dot(n, g.sun.xyz), 0.0) * 0.75;
    let lit = c.rgb * light;
    let f = clamp((length(to_eye) - g.fog.x) / (g.fog.y - g.fog.x), 0.0, 1.0);
    return vec4<f32>(mix(lit, g.fog_color.rgb, f * f), 1.0);
}

@fragment
fn fs_opaque(i: VOut) -> @location(0) vec4<f32> { return shade(i, false); }
@fragment
fn fs_alpha(i: VOut) -> @location(0) vec4<f32> { return shade(i, true); }
