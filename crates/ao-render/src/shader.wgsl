struct G {
    vp: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,        // x = start, y = end
    grid: vec4<f32>,       // xyz = light grid origin, w = cell size
    dims: vec4<i32>,       // light grid cells (x, y, z)
}
struct Mat {
    color: vec4<f32>,
    emissive: vec4<f32>, // rgb = emissive, w = 1 when texture alpha is a glow mask
}
@group(0) @binding(0) var<uniform> g: G;
@group(0) @binding(1) var<storage, read> lights: array<vec4<f32>>; // pairs: (pos, range), (colour, 0)
@group(0) @binding(2) var<storage, read> cells: array<vec2<u32>>;  // (first index, count) into `light_idx`
@group(0) @binding(3) var<storage, read> light_idx: array<u32>;
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

fn vtx(v: VIn) -> VOut {
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

@vertex
fn vs(v: VIn) -> VOut { return vtx(v); }

// Sky: pinned to the far plane so domes larger than the view distance are not clipped.
@vertex
fn vs_sky(v: VIn) -> VOut {
    var o = vtx(v);
    o.clip.z = o.clip.w;
    return o;
}

// Sum of static point lights from the cell containing `p` (linear falloff to 0 at range, wrapped lambert).
fn point_lights(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let c = vec3<i32>(floor((p - g.grid.xyz) / g.grid.w));
    if any(c < vec3<i32>(0)) || any(c >= g.dims.xyz) {
        return vec3<f32>(0.0);
    }
    let e = cells[u32((c.z * g.dims.y + c.y) * g.dims.x + c.x)];
    var sum = vec3<f32>(0.0);
    for (var k = 0u; k < e.y; k = k + 1u) {
        let li = light_idx[e.x + k] * 2u;
        let a = lights[li];
        let d = a.xyz - p;
        let dist = length(d);
        if dist < a.w {
            let ndl = clamp((dot(n, d / max(dist, 1e-3)) + 0.5) / 1.5, 0.0, 1.0);
            sum += lights[li + 1u].rgb * ((1.0 - dist / a.w) * ndl);
        }
    }
    return sum;
}

// mode: 0 opaque, 1 alpha test, 2 alpha blend, 3 additive
fn shade(i: VOut, mode: u32) -> vec4<f32> {
    let t = textureSample(tex, samp, i.uv);
    let c = t * i.color * mat.color;
    if mode == 1u && c.a < 0.5 {
        discard;
    }
    let to_eye = g.eye.xyz - i.wpos;
    let l = length(i.n);
    var n = select(vec3<f32>(0.0, 1.0, 0.0), i.n / l, l > 1e-4);
    if dot(n, to_eye) < 0.0 {
        n = -n; // lit from the viewer's side (two-sided surfaces, unreliable normals)
    }
    var light = g.ambient.rgb + g.sun_color.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + point_lights(i.wpos, n);
    var lit: vec3<f32>;
    if mat.emissive.w > 1.5 {
        // prelit room shell: vertex colour is emissive light (engine: tex * (emissive + 0.8 * ambient))
        lit = t.rgb * mat.color.rgb * (i.color.rgb + 0.8 * g.ambient.rgb);
    } else {
    if mat.emissive.w > 0.5 {
        light = max(light, min(light + t.a, vec3<f32>(1.0))); // alpha = self-illumination mask (engine: saturate(a + lighting))
    }
    lit = c.rgb * (light + mat.emissive.rgb); // engine: tex * (emissive + lighting)
    }
    let f = clamp((length(to_eye) - g.fog.x) / max(g.fog.y - g.fog.x, 1e-3), 0.0, 1.0);
    // opaque/test: fog towards fog colour, alpha 1; blend: same with alpha; additive: fade out instead of tinting.
    let add = mode == 3u;
    let rgb = select(mix(lit, g.fog_color.rgb, f), lit, add);
    let a = select(select(1.0, c.a, mode == 2u), c.a * (1.0 - f), add);
    return vec4<f32>(rgb, a);
}

// Sky: unlit, unfogged; opaque ignores alpha.
fn shade_sky(i: VOut, mode: u32) -> vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color * mat.color;
    if mode == 1u && c.a < 0.5 {
        discard;
    }
    return vec4<f32>(c.rgb * (vec3<f32>(1.0) + mat.emissive.rgb), select(1.0, c.a, mode >= 2u));
}

@fragment
fn fs_opaque(i: VOut) -> @location(0) vec4<f32> { return shade(i, 0u); }
@fragment
fn fs_test(i: VOut) -> @location(0) vec4<f32> { return shade(i, 1u); }
@fragment
fn fs_blend(i: VOut) -> @location(0) vec4<f32> { return shade(i, 2u); }
@fragment
fn fs_add(i: VOut) -> @location(0) vec4<f32> { return shade(i, 3u); }
@fragment
fn fs_sky_opaque(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 0u); }
@fragment
fn fs_sky_test(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 1u); }
@fragment
fn fs_sky_blend(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 2u); }
@fragment
fn fs_sky_add(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 3u); }
