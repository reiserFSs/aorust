struct G {
    vp: mat4x4<f32>,
    eye: vec4<f32>,        // xyz = camera position, w = projection far (independent of local fog volumes)
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,        // x = start, y = end, z = time (s), w = alpha-to-coverage active (aomac Enhanced, audit item 4; MSAA > 1)
    grid: vec4<f32>,       // xyz = light grid origin, w = cell size
    dims: vec4<i32>,       // light grid cells (x, y, z)
    wave: array<vec4<f32>, 4>, // GameWaveCurve0..15 (`ao_scene::wave_curves`)
    sun_g: vec4<f32>,      // rgb = sun colour in D3D (gamma) space, w = SpecularLightIntensity
    ambient_g: vec4<f32>,  // device ambient, gamma space
    fog_g: vec4<f32>,      // fog colour, gamma space
    view: mat4x4<f32>,     // world -> camera space (env map texgen)
    water_dir: vec4<f32>,
    water_g: vec4<f32>,    // authored WaterLightCurrent, not doubled GroundLightCurrent
    dyn_info: vec4<i32>,   // x = moving light count, y = extra directional count
    sky_counter: vec4<f32>, // x = DotStars Counter += .01 per FXS frame, independent of time/dt
    directional: array<vec4<f32>, 16>, // toward-light direction / gamma colour pairs
    moon_direction: vec4<f32>, // shared authored moon, w = Moonlit enabled
    moon_g: vec4<f32>, // gamma illumination, w = ambient dark-night attenuation
}
struct Mat {
    color: vec4<f32>,
    emissive: vec4<f32>, // rgb = emissive, w = 1 when texture alpha is a glow mask
    scroll: vec4<f32>,   // xy = uv drift per second; z = sky fog (0 off, 1 live colour, 2 flags.yzw override)
    wave: vec4<f32>,     // sky: (curve_u, amp_u, curve_v, amp_v) uv offset amp * GameWaveCurve; scroll.w = 1 for a flickering sun fan
    spec: vec4<f32>,     // rgb = material specular (spec * shin_str, 0 = SPECULARENABLE off), w = power
    flags: vec4<f32>,    // x = texture clamp; yzw = linear explicit sky fog colour
    uv_camera: vec4<f32>, // u/v translation coefficients for AO camera X/Z
    native: vec4<f32>,    // env ADD, alpha ref, state bits (test/no-fog/unlit), D3DCMPFUNC
}
@group(0) @binding(0) var<uniform> g: G;
@group(0) @binding(1) var<storage, read> lights: array<vec4<f32>>; // pairs: (pos, range), (colour, 0)
@group(0) @binding(2) var<storage, read> cells: array<vec2<u32>>;  // (first index, count) into `light_idx`
@group(0) @binding(3) var<storage, read> light_idx: array<u32>;
@group(0) @binding(4) var<storage, read> dyn_lights: array<vec4<f32>>; // per-frame moving lights, same 4-vec4 layout as `lights`
// Both samplers live in group 0 (not per-material) so Renderer::set_anisotropy only rebuilds this one bind group,
// not every material's: see `sample_clamped` below for the per-material repeat/clamp choice.
@group(0) @binding(5) var samp: sampler;
@group(0) @binding(6) var samp_clamp: sampler;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var<uniform> mat: Mat;
@group(1) @binding(2) var env_tex: texture_2d<f32>;
@group(2) @binding(0) var holo0: texture_2d<f32>;
@group(2) @binding(1) var holo1: texture_2d<f32>;

// Picks the repeat or clamp-to-edge sampler per material (`Mat::flags.x`); WGSL can't `select()` between opaque
// sampler handles, so this is a branch instead of a value expression.
fn sample_clamped(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    if (mat.flags.x > 0.5) {
        return textureSample(t, samp_clamp, uv);
    }
    return textureSample(t, samp, uv);
}

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
    // Native fixed-function base/env passes transform the same VB identically.
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) light: vec3<f32>,
    @location(5) dlight: vec3<f32>,
    @location(6) spec: vec3<f32>,
    @location(7) tint: vec3<f32>,
    @location(8) fade: f32,
    @location(9) @interpolate(flat) effect: u32,
    @location(10) @interpolate(flat) effect_time: f32,
    @location(11) fog: f32,
    @location(12) env_uv: vec2<f32>,
}

// Randy10041876: linear vertex fog, no table/range fog (10042065).
fn vertex_fog(depth: f32) -> f32 {
    return clamp((depth - g.fog.x) / max(g.fog.y - g.fog.x, 1e-3), 0.0, 1.0);
}

// Native StateBlob bits: alpha-test enable, fog disable, lighting disable.
fn native_alpha_test() -> bool { return (u32(mat.native.z) & 1u) != 0u; }
fn native_unlit() -> bool { return (u32(mat.native.z) & 4u) != 0u; }
fn native_fog(factor: f32) -> f32 {
    return select(factor, 0.0, (u32(mat.native.z) & 2u) != 0u);
}
fn native_alpha_pass(alpha: f32) -> bool {
    switch u32(mat.native.w) {
        case 1u: { return false; }
        case 2u: { return alpha < mat.native.y; }
        case 3u: { return alpha == mat.native.y; }
        case 4u: { return alpha <= mat.native.y; }
        case 5u: { return alpha > mat.native.y; }
        case 6u: { return alpha != mat.native.y; }
        case 7u: { return alpha >= mat.native.y; }
        default: { return true; } // ALWAYS8; archive input validates D3DCMPFUNC.
    }
}
fn unlit_vertex(v: VIn, vertex: VOut) -> VOut {
    var o = vertex;
    o.light = to_g(v.color.rgb);
    o.dlight = vec3<f32>(0.0);
    o.spec = vec3<f32>(0.0);
    o.tint = to_g(mat.color.rgb);
    return o;
}

fn vtx(v: VIn) -> VOut {
    // The instance matrix is affine: its m0.w is not part of the transform and carries `ActorFrame::alpha - 1` (0 for everything else).
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    let wp = model * vec4<f32>(v.pos, 1.0);
    var o: VOut;
    o.clip = g.vp * wp;
    o.wpos = wp.xyz;
    o.fog = vertex_fog(-(g.view * wp).z);
    o.n = (model * vec4<f32>(v.normal, 0.0)).xyz;
    if mat.native.x > 0.5 {
        o.env_uv = normalize((g.view * vec4<f32>(o.n, 0.0)).xyz).xy * 0.5 + vec2<f32>(0.5);
    }
    o.uv = v.uv;
    o.color = v.color;
    o.fade = 1.0 + v.m0.w;
    o.effect = 0u;
    o.effect_time = 0.0;
    return o;
}

@vertex
fn vs(v: VIn) -> VOut {
    var o = vtx(v);
    if native_unlit() { return unlit_vertex(v, o); }
    let prelit = mat.emissive.w > 1.5;
    let l = light_vertex(o.wpos, o.n, mat.emissive.rgb, mat.spec.rgb, mat.spec.w);
    // DS1006e133: COLOR1 emissive + material ambient 0.8 + lamps, saturated before Gouraud interpolation.
    o.light = select(l.light, clamp(v.color.rgb + 0.8 * g.ambient_g.rgb + l.dlight, vec3<f32>(0.0), vec3<f32>(1.0)), prelit);
    o.dlight = l.dlight;
    o.spec = l.spec;
    o.tint = to_g(mat.color.rgb * select(v.color.rgb, vec3<f32>(1.0), prelit));
    return o;
}

@vertex
fn vs_liquid(v: VIn) -> VOut {
    var o = vtx(v);
    // FUN_10059b98(..., mono=1): diffuse is white max(RGB); specular keeps the authored RGB.
    let intensity = max(max(g.water_g.r, g.water_g.g), g.water_g.b);
    let l = light_vertex_sun(o.wpos, o.n, mat.emissive.rgb, mat.spec.rgb, mat.spec.w, g.water_dir.xyz, g.water_g, vec3<f32>(intensity));
    o.light = l.light;
    o.dlight = l.dlight;
    o.spec = l.spec;
    o.tint = to_g(mat.color.rgb * v.color.rgb);
    return o;
}

@vertex
fn vs_lava(v: VIn) -> VOut {
    var o = vtx(v);
    o.light = vec3<f32>(1.0);
    o.dlight = vec3<f32>(0.0);
    o.spec = vec3<f32>(0.0);
    o.tint = to_g(v.color.rgb);
    return o;
}

@fragment
fn fs_lava(i: VOut) -> @location(0) vec4<f32> {
    let t = sample_clamped(tex, i.uv);
    return vec4<f32>(mix(t.rgb * i.tint, g.fog_g.rgb, i.fog), i.color.a);
}

// DS1003db41: crust stage0 selects texture RGB/alpha; stage1 keeps
// CURRENT RGB and multiplies alpha. COUNT3 camera texgen cancels camera
// rotation/translation, leaving native world X/Z at scales75 and250.
@fragment
fn fs_lava_crust(i: VOut) -> @location(0) vec4<f32> {
    let p = vec2<f32>(i.wpos.x, -i.wpos.z);
    let a = sample_clamped(tex, p / 75.0 + vec2<f32>(fract(g.fog.z / 750.0), 0.0));
    let b = sample_clamped(tex, p / 250.0 + vec2<f32>(0.0, fract(g.fog.z / 5000.0)));
    return vec4<f32>(mix(a.rgb, g.fog_g.rgb, i.fog), a.a * b.a);
}

// RViewPort::RenderRefraction list 4 -> VisualLiquid slot +0x3c = DS1003d13a -> 1003b9c9(this, viewport, 0 | 4). Only with Randy's
// offscreen technology (render target 2 = copy of the scene so far). Blob this+0x5d8: LIGHTING off, SRCALPHA/INVSRCALPHA, COLOROP
// SELECTARG1 TEXTURE, ALPHAOP SELECTARG1 DIFFUSE, address MIRROR, fog on. The lit RGB of the transformed buffer is therefore unused;
// only the diffuse alpha byte written below survives. Wave table `(camera+0x90)[16]` is indexed i & 15 for u and (i+1) & 15 for v.
fn wave16(i: u32) -> f32 { let k = i & 15u; return g.wave[k >> 2u][k & 3u]; }
fn mirror_uv(t: f32) -> f32 { let m = t - 2.0 * floor(t * 0.5); return select(m, 2.0 - m, m > 1.0); }
// 1003b9c9: fwd.y = camera quaternion applied to (0,0,1); band centre c = .5 - .5 * (fwd.y * 1.414); tent width 5; alpha = round(m*m*(1-|fy|)^2*255 - .49999) when m > .2.
fn refraction_band_alpha(y_up: f32, fy: f32) -> f32 {
    let c = 0.5 - 0.5 * (fy * 1.414);
    let t = (y_up - c) * 5.0;
    let m = select(1.0 - t, t + 1.0, t <= 0.0);
    let k = (1.0 - abs(fy)) * (1.0 - abs(fy));
    return select(0.0, clamp(round(m * k * m * 255.0 - 0.49999), 0.0, 255.0) / 255.0, m > 0.2);
}
@vertex
fn vs_refraction(v: VIn, @builtin(vertex_index) vi: u32) -> VOut {
    var o = vtx(v);
    let ndc = o.clip.xy / o.clip.w;
    let fy = -g.view[1][2];
    let y_up = 0.5 + 0.5 * ndc.y;
    // u = x/W + wave[i]*.005; v = (1 - y/H) + wave[i+1]*.015 + fwd.y*1.414 (1003b9c9: the image is vertically mirrored about the horizon).
    o.uv = vec2<f32>(wave16(vi) * 0.005 + (0.5 + 0.5 * ndc.x), wave16(vi + 1u) * 0.015 + y_up + fy * 1.414);
    o.color = vec4<f32>(1.0, 1.0, 1.0, refraction_band_alpha(y_up, fy));
    return o;
}
// param 4 (shallow): u = wave[i]*.165 + x/W, v = wave[i+1]*.165 + y/H (screen y down), alpha 255.
@vertex
fn vs_refraction_shallow(v: VIn, @builtin(vertex_index) vi: u32) -> VOut {
    var o = vtx(v);
    let ndc = o.clip.xy / o.clip.w;
    o.uv = vec2<f32>(wave16(vi) * 0.165 + (0.5 + 0.5 * ndc.x), wave16(vi + 1u) * 0.165 + (0.5 - 0.5 * ndc.y));
    o.color = vec4<f32>(1.0);
    return o;
}
@fragment
fn fs_refraction(i: VOut) -> @location(0) vec4<f32> {
    let t = sample_clamped(tex, vec2<f32>(mirror_uv(i.uv.x), mirror_uv(i.uv.y)));
    return vec4<f32>(mix(t.rgb, g.fog_g.rgb, native_fog(i.fog)), i.color.a);
}

// Actor overrides share the submesh's linear storage domain. Independent enable lanes preserve `None`, including power 0.
@vertex
fn vs_actor(v: VIn, @location(8) emissive: vec4<f32>, @location(9) specular: vec4<f32>, @location(10) power: vec4<f32>, @location(11) uv: vec4<f32>) -> VOut {
    var o = vtx(v);
    o.uv = v.uv * uv.xy + uv.zw;
    o.effect = u32(power.z);
    o.effect_time = power.w;
    if native_unlit() { return unlit_vertex(v, o); }
    // Actor overrides replace the submesh material's emissive/specular/power per instance (holo, degraded nanobots,
    // CAT effects) before evaluating vertex lighting.
    let eff_emissive = select(mat.emissive.rgb, emissive.rgb, emissive.w > 0.5);
    let eff_spec = select(mat.spec.rgb, specular.rgb, specular.w > 0.5);
    let eff_power = select(mat.spec.w, power.x, power.y > 0.5);
    let prelit = mat.emissive.w > 1.5 && (o.effect == 0u || o.effect == 3u || o.effect == 7u);
    let l = light_vertex(o.wpos, o.n, eff_emissive, eff_spec, eff_power);
    o.light = select(l.light, clamp(v.color.rgb + 0.8 * g.ambient_g.rgb + l.dlight, vec3<f32>(0.0), vec3<f32>(1.0)), prelit);
    o.dlight = l.dlight;
    o.spec = l.spec;
    o.tint = to_g(mat.color.rgb * select(v.color.rgb, vec3<f32>(1.0), prelit));
    if o.effect == 1u || o.effect == 5u || o.effect == 6u {
        o.light = vec3<f32>(1.0);
        o.dlight = vec3<f32>(0.0);
        o.spec = vec3<f32>(0.0);
    }
    return o;
}

// Sky shares the native camera projection, clipping and vertex fog.
fn sky_vertex(v: VIn) -> VOut {
    var o = vtx(v);
    let camera_xz = vec2<f32>(g.eye.x, -g.eye.z);
    o.uv += vec2<f32>(dot(mat.uv_camera.xy, camera_xz), dot(mat.uv_camera.zw, camera_xz));
    // Native material emissive adds before vertex saturation, not tex*(1+emissive).
    o.light = min(to_g(v.color.rgb) + to_g(mat.emissive.rgb), vec3<f32>(1.0));
    if mat.scroll.w > 0.5 && v.normal.z >= 0.0 {
        // e_SunRays rim alpha * (255 - trunc(255 * curve[i & 7])) / 255 (FUN_1005a3a9), interpolated per vertex like the client
        let k = u32(v.normal.z + 0.5) & 7u;
        o.color.a = o.color.a * (1.0 - floor(255.0 * g.wave[k >> 2u][k & 3u]) / 255.0);
    }
    return o;
}
@vertex
fn vs_sky(v: VIn) -> VOut { return sky_vertex(v); }
@vertex
fn vs_sky_points(v: VIn) -> VOut {
    var o = sky_vertex(v);
    o.uv.x += g.sky_counter.x;
    return o;
}

// The client computes in framebuffer (gamma) space: D3D7 fixed function lighting, the texture stage (`tex * vertex colour`), the
// specular add, fog and the blend all operate on the 8 bit gamma values, textures are not decoded. The scene contract hands over
// colours as `c^2.2` ("linear"); recover them with `to_g`. Native textures use UNORM views:
// interpolation operates directly on encoded bytes, before lighting, fog, blending and MSAA resolve,
// matching the D3D7 fixed-function path (no sRGB texture decode).
fn to_g(v: vec3<f32>) -> vec3<f32> {
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

// D3D7 fixed-function vertex lighting (`IDirect3DDevice7::DrawIndexedPrimitive` with LIGHTING = 1, SHADEMODE = GOURAUD): evaluated
// once per vertex, the results are interpolated across the triangle and modulated with the texture in the fragment stage.
// Material: diffuse and ambient white (`RViewPort_t::SetMaterial` leaves them at `SetDefaultMaterial`'s 1.0), emissive and
// specular from `mat`; device AMBIENT is `g.ambient_g`. Everything below is gamma space.
// Absent FVF0x112 colours: D3D7 reads the material for COLOR1/COLOR2 sources.
// Native VB upload/rebuild preserves FVF (Randy10048a5c/100488ed/10011745/10011853).
// DX7 SDK Triangle tutorial + secondary D3D7 Windows conformance: docs/formats.md.
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
    let v = normalize(g.eye.xyz - p);
    let c = vec3<i32>(floor((p - g.grid.xyz) / g.grid.w));
    // D3D7 fixed function: at most 8 active lights; the cell list is sorted strongest first.
    // Lights switched off by the statel distance LOD (range 0) do not count against the 8.
    let directional_count = u32(g.dyn_info.y);
    // The pre-existing sun path does not reserve a slot without FXS lights.
    // With root FXS directionals, keep sun + extra + point/spot within D3D's eight.
    var used = directional_count + select(0u, 1u, directional_count > 0u);
    for (var k = 0u; k < directional_count; k = k + 1u) {
        let l = g.directional[k * 2u].xyz;
        let col = g.directional[k * 2u + 1u].rgb;
        let nl = dot(n, l);
        if nl > 0.0 {
            diff += col * nl;
            if power > 0.0 {
                spec += col * pow(max(dot(n, normalize(l + v)), 0.0), power);
            }
        }
    }
    if !(any(c < vec3<i32>(0)) || any(c >= g.dims.xyz)) {
        let e = cells[u32((c.z * g.dims.y + c.y) * g.dims.x + c.x)];
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
    }
    // Per-frame moving lights (worn/attached item lights such as the Light Bar, `Renderer::set_dynamic_lights`): a
    // small flat list, not range-gridded (few are ever active live), the same D3D7 formula, sharing the 8-light
    // budget above -- and evaluated even where the point falls outside the static grid (indoor/edge cases).
    let dyn_count = u32(g.dyn_info.x);
    for (var k = 0u; k < dyn_count && used < 8u; k = k + 1u) {
        let li = k * 4u;
        let a = dyn_lights[li];
        if a.w <= 0.0 {
            continue;
        }
        let d = a.xyz - p;
        let dist = length(d);
        if dist > a.w {
            continue;
        }
        used = used + 1u;
        let col = dyn_lights[li + 1u];
        let att = dyn_lights[li + 2u];
        let l = d / max(dist, 1e-3);
        var i = select(1.0 - dist / a.w, 1.0 / (att.x + dist * (att.y + dist * att.z)), att.x >= 0.0);
        if col.w < 1.5 {
            let rho = dot(-l, dyn_lights[li + 3u].xyz);
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
    return array<vec3<f32>, 2>(diff, spec);
}

fn light_vertex(p: vec3<f32>, n_in: vec3<f32>, emissive: vec3<f32>, specular: vec3<f32>, power: f32) -> Lit {
    return light_vertex_sun(p, n_in, emissive, specular, power, g.sun_dir.xyz, g.sun_g, g.sun_g.rgb);
}

fn light_vertex_sun(p: vec3<f32>, n_in: vec3<f32>, emissive: vec3<f32>, specular: vec3<f32>, power: f32, sun_dir: vec3<f32>, sun: vec4<f32>, sun_diffuse: vec3<f32>) -> Lit {
    let nl = length(n_in);
    let n = select(vec3<f32>(0.0, 1.0, 0.0), n_in / nl, nl > 1e-6);
    let dl = vertex_lights(p, n, power);
    // DisplaySystem temporarily installs the authored WaterLight for liquid passes.
    let s = sun_dir;
    let sn = dot(n, s);
    var sun_spec = vec3<f32>(0.0);
    if sn > 0.0 && power > 0.0 {
        sun_spec = sun.rgb * (sun.w * pow(max(dot(n, normalize(s + normalize(g.eye.xyz - p))), 0.0), power));
    }
    var o: Lit;
    o.dlight = dl[0];
    o.light = min(to_g(emissive) + g.ambient_g.rgb + sun_diffuse * max(sn, 0.0) + dl[0], vec3<f32>(1.0));
    o.spec = min(to_g(specular) * (dl[1] + sun_spec), vec3<f32>(1.0));
    return o;
}

// mode: 0 opaque, 1 alpha test, 2 alpha blend, 3 additive; 4 / 5 = opaque / alpha test of an actor with alpha < 1 (`ActorFrame::alpha`: alpha blended
// with the material alpha = the frame's transparency, `RVisual_t::RenderWithTransparency`; the opaque texture alpha is not used)
fn shade(i: VOut, fade_mode: u32) -> vec4<f32> {
    let sprite = fade_mode == 6u;
    let one_one = fade_mode == 9u;
    let faded = fade_mode == 4u || fade_mode == 5u;
    let mode = select(select(fade_mode, fade_mode - 4u, faded), 2u, sprite);
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    // CAT10056ed6 / RVisual1004d2d8 replace material alpha with frame opacity.
    // Mode5 disables the alpha texture operation: its texture alpha is light, not coverage.
    let material_alpha = select(mat.color.a, i.fade, i.fade < 1.0);
    let texture_alpha = select(t.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5);
    let alpha = texture_alpha * i.color.a * material_alpha;
    // Audit item 4 (aomac Enhanced, opt-in via MSAA > 1): the pipeline's `alpha_to_coverage_enabled` (`lib.rs::build_pipelines`)
    // turns the real alpha this fragment outputs into per-sample coverage instead of a hard cutoff, softening foliage/fence/hair
    // edges. At MSAA = 1 (the retail-faithful default) `a2c` is always false and behaviour is exactly the former hard cutout.
    let a2c = g.fog.w > 0.5;
    var light = i.light;
    var spec = i.spec;
    if !native_unlit() && mat.emissive.w > 1.5 && i.effect != 2u && i.effect != 4u && i.effect != 5u && i.effect != 6u {
        // Room diffuse is already saturated per vertex; only interpolate and texture-modulate it here.
        spec = vec3<f32>(0.0);
    } else if mat.emissive.w > 0.5 && mat.emissive.w < 1.5 && (i.effect == 0u || i.effect == 3u || i.effect == 7u) {
        light = min(light + t.a, vec3<f32>(1.0)); // alpha = self-illumination mask (stage 0 ADD: saturate(a + lighting))
    }
    // texture stage MODULATE, then the specular add (both clamped to the framebuffer range), then fog: all in gamma space
    var texture_color = min(t.rgb * i.tint * light, vec3<f32>(1.0));
    if mat.native.x > 0.5 {
        // RVisual1004d18a: stage1 TEXTURE + CURRENT, before specular and fog.
        let env = sample_clamped(env_tex, i.env_uv);
        texture_color = min(texture_color + env.rgb, vec3<f32>(1.0));
    }
    let lit = min(texture_color + spec, vec3<f32>(1.0));
    // D3D fixed function evaluates every texture stage before ALPHATEST. Keep
    // implicit texture derivatives outside the divergent discard control flow.
    if native_alpha_test() && !native_alpha_pass(alpha) {
        discard;
    }
    if mode == 1u && !native_alpha_test() && !a2c && alpha < 0.5 {
        discard;
    }
    let f = native_fog(select(i.fog, 0.0, i.effect == 2u || (i.effect >= 4u && i.effect <= 6u)));
    // opaque/test: fog towards fog colour, alpha 1; blend: same with alpha; additive: fade out instead of tinting.
    let add = mode == 3u;
    let authored_fog = select(g.fog_g.rgb, to_g(mat.flags.yzw), mat.scroll.z > 1.5);
    let rgb = select(mix(lit, authored_fog, f), lit, add);
    var a = select(select(1.0, alpha, one_one || mode == 2u || (mode == 1u && a2c)), alpha * (1.0 - f), add);
    if faded {
        a = alpha;
    }
    return vec4<f32>(rgb, a);
}

// Shared by the opt-in GPU pixel probe: never reconstruct cloud UVs on the CPU.
fn sky_uv(i: VOut) -> vec2<f32> {
    let wu = u32(mat.wave.x);
    let wv = u32(mat.wave.z);
    let wobble = vec2<f32>(mat.wave.y * g.wave[wu >> 2u][wu & 3u], mat.wave.w * g.wave[wv >> 2u][wv & 3u]);
    return i.uv + mat.scroll.xy * g.fog.z + wobble;
}

// Sky: prelit vertex colour; optional source fog precedes the native blend equation.
fn shade_sky(i: VOut, mode: u32) -> vec4<f32> {
    let t = sample_clamped(tex, sky_uv(i));
    let alpha = t.a * i.color.a * mat.color.a;
    let a2c = g.fog.w > 0.5; // audit item 4, see `shade`
    if mode == 1u && !a2c && alpha < 0.5 {
        discard;
    }
    // MODULATE2X / stage-1 TFACTOR live in material RGB, after saturated lighting.
    var rgb = clamp(t.rgb * i.light * to_g(mat.color.rgb), vec3<f32>(0.0), vec3<f32>(1.0));
    if mat.scroll.z > 0.5 {
        // D3D fog follows the texture stage in gamma space, before blending (as in shade).
        let fog_color = select(g.fog_g.rgb, to_g(mat.flags.yzw), mat.scroll.z > 1.5);
        rgb = mix(rgb, fog_color, i.fog);
    }
    return vec4<f32>(rgb, select(1.0, alpha, mode >= 2u || (mode == 1u && a2c)));
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
fn fs_mesh_add(i: VOut) -> @location(0) vec4<f32> { return shade(i, 3u); }
@fragment
fn fs_add_one(i: VOut) -> @location(0) vec4<f32> { return shade(i, 9u); }

// HoloDeltaState, DS1006c005: camera-space position transformed back through the camera's world
// matrix, with X erased and Y scrolled. COUNT3 is not D3DTTFF_PROJECTED (no divide).
@fragment
fn fs_holo(i: VOut) -> @location(0) vec4<f32> {
    let t0 = sample_clamped(holo0, vec2<f32>(0.0, i.wpos.y + fract(0.2 * i.effect_time)));
    let t1 = sample_clamped(holo1, i.uv + vec2<f32>(fract(0.15 * i.effect_time), 0.0));
    let rgb = min(t0.rgb + t1.rgb, vec3<f32>(1.0));
    let f = i.fog;
    return vec4<f32>(mix(rgb, g.fog_g.rgb, f), t0.a * i.color.a * mat.color.a * i.fade);
}
@fragment
fn fs_sky_opaque(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 0u); }
@fragment
fn fs_sky_test(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 1u); }
@fragment
fn fs_sky_blend(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 2u); }
@fragment
fn fs_sky_add(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 3u); }
@fragment
fn fs_sky_add_one(i: VOut) -> @location(0) vec4<f32> { return shade_sky(i, 3u); }

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
    let t = sample_clamped(tex, i.uv);
    // The env blob10055a3e changes neither ALPHATEST nor material alpha.
    // ONE/ONE means RGB itself is deliberately independent of opacity.
    let alpha = t.a * i.color.a * select(mat.color.a, i.fade, i.fade < 1.0);
    if native_alpha_test() && !native_alpha_pass(alpha) { discard; }
    let f = native_fog(i.fog);
    return vec4<f32>(mix(t.rgb, g.fog_g.rgb, f), 1.0);
}

// N3 10033b97: +668 ADD palette/diffuse then MODULATE tile; +6ec
// SELECTARG1 tile followed by +770 ZERO/SRCCOLOR palette/diffuse.
@vertex
fn vs_ground(v: VIn) -> VOut {
    var o = vtx(v);
    let n = normalize(o.n);
    o.light = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + vertex_lights(o.wpos, n, 0.0)[0], vec3<f32>(0.0), vec3<f32>(1.0));
    o.dlight = clamp(o.light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    o.spec = vec3<f32>(0.0);
    o.tint = vec3<f32>(1.0);
    return o;
}
fn ground_fog(i: VOut) -> f32 {
    return i.fog;
}
fn ground_palette(i: VOut) -> vec3<f32> {
    return sample_clamped(holo0, vec2<f32>(i.wpos.x, -i.wpos.z) * mat.wave.xz).rgb;
}
fn ground_tile(i: VOut) -> vec3<f32> {
    let tile = sample_clamped(tex, i.uv);
    if tile.a <= 128.0 / 255.0 { discard; }
    return tile.rgb;
}
@fragment
fn fs_ground_fused(i: VOut) -> @location(0) vec4<f32> {
    let light = clamp(i.light + ground_palette(i), vec3<f32>(0.0), vec3<f32>(1.0));
    let tile = ground_tile(i);
    return vec4<f32>(mix(tile * light, g.fog_g.rgb, ground_fog(i)), 1.0);
}
@fragment
fn fs_ground_base(i: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(mix(ground_tile(i), g.fog_g.rgb, ground_fog(i)), 1.0);
}
@fragment
fn fs_ground_shadow(i: VOut) -> @location(0) vec4<f32> {
    let light = clamp(i.light + ground_palette(i), vec3<f32>(0.0), vec3<f32>(1.0));
    // +770 sets FOGCOLOR white so fog tends toward the identity multiplier.
    return vec4<f32>(mix(light, vec3<f32>(1.0), ground_fog(i)), 1.0);
}
@fragment
fn fs_ground_simple(i: VOut) -> @location(0) vec4<f32> {
    // +a0c material has white ambient; +a08 has black ambient.
    return vec4<f32>(mix(ground_tile(i) * i.dlight, g.fog_g.rgb, ground_fog(i)), 1.0);
}
