struct G {
    vp: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>, // x = start, y = end
}
struct Mat {
    color: vec4<f32>,
}
@group(0) @binding(0) var<uniform> g: G;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var<uniform> mat: Mat;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) m3: vec4<f32>,
}
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
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
    o.color = v.color;
    return o;
}

// mode: 0 opaque, 1 alpha test, 2 alpha blend, 3 additive
fn shade(i: VOut, mode: u32) -> vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color * mat.color;
    if mode == 1u && c.a < 0.5 {
        discard;
    }
    let to_eye = g.eye.xyz - i.wpos;
    let l = length(i.n);
    var n = select(vec3<f32>(0.0, 1.0, 0.0), i.n / l, l > 1e-4);
    if dot(n, to_eye) < 0.0 {
        n = -n; // lit from the viewer's side (two-sided surfaces, unreliable normals)
    }
    let light = g.ambient.rgb + g.sun_color.rgb * max(dot(n, g.sun_dir.xyz), 0.0);
    let lit = c.rgb * light;
    let f = clamp((length(to_eye) - g.fog.x) / max(g.fog.y - g.fog.x, 1e-3), 0.0, 1.0);
    switch mode {
        case 2u: { return vec4<f32>(mix(lit, g.fog_color.rgb, f), c.a); }
        case 3u: { return vec4<f32>(lit, c.a * (1.0 - f)); } // fade glow out instead of tinting it
        default: { return vec4<f32>(mix(lit, g.fog_color.rgb, f), 1.0); }
    }
}

@fragment
fn fs_opaque(i: VOut) -> @location(0) vec4<f32> { return shade(i, 0u); }
@fragment
fn fs_test(i: VOut) -> @location(0) vec4<f32> { return shade(i, 1u); }
@fragment
fn fs_blend(i: VOut) -> @location(0) vec4<f32> { return shade(i, 2u); }
@fragment
fn fs_add(i: VOut) -> @location(0) vec4<f32> { return shade(i, 3u); }
