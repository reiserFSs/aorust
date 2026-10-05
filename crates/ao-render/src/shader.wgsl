struct G {
    vp: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,        // x = start, y = end, z = time (s)
    grid: vec4<f32>,       // xyz = light grid origin, w = cell size
    dims: vec4<i32>,       // light grid cells (x, y, z)
    wave: array<vec4<f32>, 4>, // GameWaveCurve0..15 (`ao_scene::wave_curves`)
}
struct Mat {
    color: vec4<f32>,
    emissive: vec4<f32>, // rgb = emissive, w = 1 when texture alpha is a glow mask
    scroll: vec4<f32>,   // xy = uv drift per second
    wave: vec4<f32>,     // sky: (curve_u, amp_u, curve_v, amp_v) uv offset amp * GameWaveCurve; scroll.w = 1 for a flickering sun fan
    spec: vec4<f32>,     // rgb = material specular (spec * shin_str, 0 = SPECULARENABLE off), w = power
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
    @location(4) light: vec3<f32>,
    @location(5) dlight: vec3<f32>,
    @location(6) spec: vec3<f32>,
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
fn vs(v: VIn) -> VOut {
    var o = vtx(v);
    let l = light_vertex(o.wpos, o.n);
    o.light = l.light;
    o.dlight = l.dlight;
    o.spec = l.spec;
    return o;
}

// Sky: pinned to the far plane so domes larger than the view distance are not clipped.
@vertex
fn vs_sky(v: VIn) -> VOut {
    var o = vtx(v);
    o.clip.z = o.clip.w;
    if mat.scroll.w > 0.5 && v.normal.z >= 0.0 {
        // e_SunRays rim alpha * (255 - trunc(255 * curve[i & 7])) / 255 (FUN_1005a3a9), interpolated per vertex like the client
        let k = u32(v.normal.z + 0.5) & 7u;
        o.color.a = o.color.a * (1.0 - floor(255.0 * g.wave[k >> 2u][k & 3u]) / 255.0);
    }
    return o;
}

// D3D7 fixed-function vertex lighting (`IDirect3DDevice7::DrawIndexedPrimitive` with LIGHTING = 1, SHADEMODE = GOURAUD): evaluated
// once per vertex, the results are interpolated across the triangle and modulated with the texture in the fragment stage.
// Material sources are the material (render states 145..147 = 0, FVF 0x112 has no vertex colour); device AMBIENT is `g.ambient`.
struct Lit {
    light: vec3<f32>, // saturate(ambient + sun diffuse + point/spot diffuse); the emissive term is added in the fragment stage
    dlight: vec3<f32>, // point/spot diffuse alone (prelit surfaces add it to the baked vertex colour)
    spec: vec3<f32>, // material specular * sum of light specular terms (SPECULARENABLE), added after the texture stage
}

// Sum over the lights of the grid cell containing `p`. D3D7 per light: attenuation `1 / (a0 + a1 d + a2 d^2)` for d <= dvRange (hard
// cut), spot factor `clamp((rho - cos(phi/2)) / (cos(theta/2) - cos(phi/2)), 0, 1)` with `rho = -L.axis` (dvFalloff = 1),
// diffuse `colour * max(N.L, 0)`, specular (only when N.L > 0, power > 0, LOCALVIEWER = 1) `colour * (N.H)^power`, `H = |L + V|`.
// Lights without attenuation coefficients (a0 < 0) use the linear ramp `1 - d / range` (demo lights).
fn vertex_lights(p: vec3<f32>, n: vec3<f32>, power: f32) -> array<vec3<f32>, 2> {
    var diff = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    let c = vec3<i32>(floor((p - g.grid.xyz) / g.grid.w));
    if any(c < vec3<i32>(0)) || any(c >= g.dims.xyz) {
        return array<vec3<f32>, 2>(diff, spec);
    }
    let e = cells[u32((c.z * g.dims.y + c.y) * g.dims.x + c.x)];
    let v = normalize(g.eye.xyz - p);
    // D3D7 fixed function: at most 8 active lights; the cell list is sorted strongest first.
    // Lights switched off by the statel distance LOD (range 0) do not count against the 8.
    var used = 0u;
    for (var k = 0u; k < e.y && used < 8u; k = k + 1u) {
        let li = light_idx[e.x + k] * 4u;
        let a = lights[li];
        if a.w <= 0.0 {
            continue;
        }
        used = used + 1u;
        let col = lights[li + 1u];
        let att = lights[li + 2u];
        let d = a.xyz - p;
        let dist = length(d);
        if dist <= a.w {
            let l = d / max(dist, 1e-3);
            var i = select(1.0 - dist / a.w, 1.0 / (att.x + dist * (att.y + dist * att.z)), att.x >= 0.0);
            if col.w < 1.5 {
                // spot: rho = cos(angle between -L and the axis); 1 inside theta/2, 0 outside phi/2, linear between
                let rho = dot(-l, lights[li + 3u].xyz);
                i = i * clamp((rho - col.w) / max(att.w - col.w, 1e-4), 0.0, 1.0);
            }
            let nl = dot(n, l);
            if nl > 0.0 {
                diff += col.rgb * (i * nl);
                if power > 0.0 {
                    spec += col.rgb * (i * pow(max(dot(n, normalize(l + v)), 0.0), power));
                }
            }
        }
    }
    return array<vec3<f32>, 2>(diff, spec);
}

fn light_vertex(p: vec3<f32>, n_in: vec3<f32>) -> Lit {
    let nl = length(n_in);
    let n = select(vec3<f32>(0.0, 1.0, 0.0), n_in / nl, nl > 1e-6);
    let power = mat.spec.w;
    let dl = vertex_lights(p, n, power);
    var o: Lit;
    o.dlight = dl[0];
    o.light = min(g.ambient.rgb + g.sun_color.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + dl[0], vec3<f32>(1.0));
    o.spec = min(mat.spec.rgb * dl[1], vec3<f32>(1.0));
    return o;
}

// mode: 0 opaque, 1 alpha test, 2 alpha blend, 3 additive
fn shade(i: VOut, mode: u32) -> vec4<f32> {
    let t = textureSample(tex, samp, i.uv + mat.scroll.xy * g.fog.z);
    let c = t * i.color * mat.color;
    if mode == 1u && c.a < 0.5 {
        discard;
    }
    let to_eye = g.eye.xyz - i.wpos;
    var light = i.light;
    var spec = i.spec;
    var lit: vec3<f32>;
    if mat.emissive.w > 1.5 {
        // prelit room shell: vertex colour is emissive light (engine: tex * saturate(lightmap + 0.8 * ambient + dlight))
        lit = t.rgb * mat.color.rgb * min(i.color.rgb + 0.8 * g.ambient.rgb + i.dlight, vec3<f32>(1.0));
        spec = vec3<f32>(0.0);
    } else {
        if mat.emissive.w > 0.5 {
            light = max(light, min(light + t.a, vec3<f32>(1.0))); // alpha = self-illumination mask (engine: saturate(a + lighting))
        }
        lit = c.rgb * (light + mat.emissive.rgb) + spec; // engine: tex * (emissive + lighting) + specular
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
    let wu = u32(mat.wave.x);
    let wv = u32(mat.wave.z);
    let wobble = vec2<f32>(mat.wave.y * g.wave[wu >> 2u][wu & 3u], mat.wave.w * g.wave[wv >> 2u][wv & 3u]);
    let c = textureSample(tex, samp, i.uv + mat.scroll.xy * g.fog.z + wobble) * i.color * mat.color;
    if mode == 1u && c.a < 0.5 {
        discard;
    }
    var rgb = c.rgb * (vec3<f32>(1.0) + mat.emissive.rgb);
    if mat.scroll.z > 0.5 {
        // atmosphere strip: fogged with the live fog at the camera (normal.x = its distance at the view distance)
        rgb = mix(rgb, g.fog_color.rgb, clamp((i.n.x - g.fog.x) / max(g.fog.y - g.fog.x, 1e-3), 0.0, 1.0));
    }
    return vec4<f32>(rgb, select(1.0, c.a, mode >= 2u));
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
