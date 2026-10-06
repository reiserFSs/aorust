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
    sun_g: vec4<f32>,      // rgb = sun colour in D3D (gamma) space, w = SpecularLightIntensity
    ambient_g: vec4<f32>,  // device ambient, gamma space
    fog_g: vec4<f32>,      // fog colour, gamma space
    view: mat4x4<f32>,     // world -> camera space (env map texgen)
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
    @location(7) tint: vec3<f32>,
    @location(8) fade: f32,
}

fn vtx(v: VIn) -> VOut {
    // The instance matrix is affine: its m0.w is not part of the transform and carries `ActorFrame::alpha - 1` (0 for everything else).
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    let wp = model * vec4<f32>(v.pos, 1.0);
    var o: VOut;
    o.clip = g.vp * wp;
    o.wpos = wp.xyz;
    o.n = (model * vec4<f32>(v.normal, 0.0)).xyz;
    o.uv = v.uv;
    o.color = v.color;
    o.fade = 1.0 + v.m0.w;
    return o;
}

@vertex
fn vs(v: VIn) -> VOut {
    var o = vtx(v);
    let prelit = mat.emissive.w > 1.5;
    let l = light_vertex(o.wpos, o.n);
    // prelit surfaces: the vertex colour is the baked additive light in D3D space (see `shade`)
    o.light = select(l.light, v.color.rgb, prelit);
    o.dlight = l.dlight;
    o.spec = l.spec;
    o.tint = to_g(mat.color.rgb * select(v.color.rgb, vec3<f32>(1.0), prelit));
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

// The client computes in framebuffer (gamma) space: D3D7 fixed function lighting, the texture stage (`tex * vertex colour`), the
// specular add, fog and the blend all operate on the 8 bit gamma values, textures are not decoded. The scene contract hands over
// colours as `c^2.2` ("linear"); recover them with `to_g` and re-encode sampled sRGB textures. The UNORM framebuffer stores
// these gamma values directly, so blending and MSAA resolve operate on the same values as D3D7.
// Texture filtering remains the existing hardware sRGB-decode/filter/re-encode path (not D3D7 gamma-space filtering).
fn to_g(v: vec3<f32>) -> vec3<f32> {
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}
fn srgb_enc(l: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(l, vec3<f32>(1.0 / 2.4)) - 0.055, 12.92 * l, l <= vec3<f32>(0.0031308));
}

// D3D7 fixed-function vertex lighting (`IDirect3DDevice7::DrawIndexedPrimitive` with LIGHTING = 1, SHADEMODE = GOURAUD): evaluated
// once per vertex, the results are interpolated across the triangle and modulated with the texture in the fragment stage.
// Material: diffuse and ambient white (`RViewPort_t::SetMaterial` leaves them at `SetDefaultMaterial`'s 1.0), emissive and
// specular from `mat`; device AMBIENT is `g.ambient_g`. Everything below is gamma space.
struct Lit {
    light: vec3<f32>, // saturate(emissive + ambient + sun diffuse + point/spot diffuse) = the D3D vertex diffuse colour
    dlight: vec3<f32>, // point/spot diffuse alone (prelit surfaces add it to the baked vertex colour)
    spec: vec3<f32>, // saturate(material specular * sum of light specular terms) (SPECULARENABLE), added after the texture stage
}

// Sum over the lights of the grid cell containing `p` (colours are gamma space). D3D7 per light: attenuation
// `1 / (a0 + a1 d + a2 d^2)` for d <= dvRange (hard cut), spot factor `clamp((rho - cos(phi/2)) / (cos(theta/2) - cos(phi/2)), 0, 1)`
// with `rho = -L.axis` (dvFalloff = 1), diffuse `colour * max(N.L, 0)`, specular (only when N.L > 0, power > 0, LOCALVIEWER = 1)
// `colour * (N.H)^power`, `H = |L + V|`. Lights without attenuation coefficients (a0 < 0) use the linear ramp `1 - d / range`.
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
    // the sun: a directional light, diffuse = GroundLightCurrent, specular = SpecularLightIntensity * GroundLightCurrent
    let s = g.sun_dir.xyz;
    let sn = dot(n, s);
    var sun_spec = vec3<f32>(0.0);
    if sn > 0.0 && power > 0.0 {
        sun_spec = g.sun_g.rgb * (g.sun_g.w * pow(max(dot(n, normalize(s + normalize(g.eye.xyz - p))), 0.0), power));
    }
    var o: Lit;
    o.dlight = dl[0];
    o.light = min(to_g(mat.emissive.rgb) + g.ambient_g.rgb + g.sun_g.rgb * max(sn, 0.0) + dl[0], vec3<f32>(1.0));
    o.spec = min(to_g(mat.spec.rgb) * (dl[1] + sun_spec), vec3<f32>(1.0));
    return o;
}

// mode: 0 opaque, 1 alpha test, 2 alpha blend, 3 additive; 4 / 5 = opaque / alpha test of an actor with alpha < 1 (`ActorFrame::alpha`: alpha blended
// with the material alpha = the frame's transparency, `RVisual_t::RenderWithTransparency`; the opaque texture alpha is not used)
fn shade(i: VOut, fade_mode: u32) -> vec4<f32> {
    let sprite = fade_mode == 6u;
    let faded = fade_mode >= 4u && !sprite;
    let mode = select(select(fade_mode, fade_mode - 4u, faded), 2u, sprite);
    let t = textureSample(tex, samp, i.uv + mat.scroll.xy * g.fog.z);
    let alpha = t.a * i.color.a * mat.color.a * i.fade;
    if sprite && alpha <= 30.0 / 255.0 {
        discard;
    }
    if mode == 1u && alpha < 0.5 {
        discard;
    }
    let to_eye = g.eye.xyz - i.wpos;
    var light = i.light;
    var spec = i.spec;
    if mat.emissive.w > 1.5 {
        // prelit: vertex colour is additive light (engine: tex * saturate(lightmap + 0.8 * ambient + dlight))
        light = clamp(i.light + 0.8 * g.ambient_g.rgb + i.dlight, vec3<f32>(0.0), vec3<f32>(1.0));
        spec = vec3<f32>(0.0);
    } else if mat.emissive.w > 0.5 {
        light = min(light + t.a, vec3<f32>(1.0)); // alpha = self-illumination mask (stage 0 ADD: saturate(a + lighting))
    }
    // texture stage MODULATE, then the specular add (both clamped to the framebuffer range), then fog: all in gamma space
    let lit = min(srgb_enc(t.rgb) * i.tint * light + spec, vec3<f32>(1.0));
    let f = clamp((length(to_eye) - g.fog.x) / max(g.fog.y - g.fog.x, 1e-3), 0.0, 1.0);
    // opaque/test: fog towards fog colour, alpha 1; blend: same with alpha; additive: fade out instead of tinting.
    let add = mode == 3u;
    let rgb = select(mix(lit, g.fog_g.rgb, f), lit, add);
    var a = select(select(1.0, alpha, mode == 2u), alpha * (1.0 - f), add);
    if faded {
        a = select(i.fade, alpha, mode == 1u);
    }
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
    // Preserve the sky's existing shading; only move the former target encoding before gamma-space blending.
    return vec4<f32>(srgb_enc(rgb), select(1.0, c.a, mode >= 2u));
}

@fragment
fn fs_opaque(i: VOut) -> @location(0) vec4<f32> { return shade(i, 0u); }
@fragment
fn fs_fade_opaque(i: VOut) -> @location(0) vec4<f32> { return shade(i, 4u); }
@fragment
fn fs_fade_test(i: VOut) -> @location(0) vec4<f32> { return shade(i, 5u); }
@fragment
fn fs_test(i: VOut) -> @location(0) vec4<f32> { return shade(i, 1u); }
@fragment
fn fs_blend(i: VOut) -> @location(0) vec4<f32> { return shade(i, 2u); }
@fragment
fn fs_sprite(i: VOut) -> @location(0) vec4<f32> { return shade(i, 6u); }
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

// CAT environment-map pass (randy31 `FUN_10056ed6`, mirrors `ao_scene::env_uv`): the uv is generated from the camera-space normal
// (D3DTSS_TCI_CAMERASPACENORMAL) through scale(0.5, 0.5) + translation (0.5, 0.5).
@vertex
fn vs_env(v: VIn) -> VOut {
    var o = vtx(v);
    let nv = normalize((g.view * vec4<f32>(o.n, 0.0)).xyz);
    o.uv = 0.5 * nv.xy + vec2<f32>(0.5);
    return o;
}

// Stage 0 SELECTARG1 texture (no lighting, no vertex colour), blend `ONE, ONE`; the fixed-function fog still blends the colour
// towards the fog colour before the add (the env state blob @RCATMesh+0x224 sets no fog colour, `FUN_10055a3e`).
@fragment
fn fs_env(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, i.uv);
    let f = clamp((length(g.eye.xyz - i.wpos) - g.fog.x) / max(g.fog.y - g.fog.x, 1e-3), 0.0, 1.0);
    return vec4<f32>(mix(srgb_enc(t.rgb), g.fog_g.rgb, f), 1.0);
}
