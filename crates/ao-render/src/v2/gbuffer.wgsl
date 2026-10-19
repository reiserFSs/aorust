// Enhanced v2 G-buffer fragment stage, appended to the renamed native `shader.wgsl` (its vertex stages and the
// native fragment bodies are reused: `orig_fs_*`). `packing.wgsl` precedes this file.
//
// Targets of the world pass: 0 sky colour (linear HDR), 1 G0 albedo (gamma) + material flags, 2 G1 octahedral
// normal + roughness/aux, 3 G2 motion (unused until actors write it), 4 G3 authored emission (linear).
struct GOut {
    @location(0) sky: vec4<f32>,
    @location(1) g0: vec4<f32>,
    @location(2) g1: vec4<f32>,
    @location(3) g2: vec4<f32>,
    @location(4) g3: vec4<f32>,
}

fn lin(v: vec3<f32>) -> vec3<f32> { return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2)); }

fn g_pack(albedo_g: vec3<f32>, flags: u32, n: vec3<f32>, aux: f32, emission: vec3<f32>) -> GOut {
    var o: GOut;
    o.sky = vec4<f32>(0.0);
    o.g0 = vec4<f32>(albedo_g, f32(flags) / 255.0);
    o.g1 = vec4<f32>(oct_encode(n), aux, 0.0);
    o.g2 = vec4<f32>(0.0);
    o.g3 = vec4<f32>(emission, 0.0);
    return o;
}

fn g_normal(i: VOut, front: bool) -> vec3<f32> {
    let n = normalize(i.n);
    return select(-n, n, front);
}

// Native opaque / alpha-tested surface: same sampling, alpha and discard rules as `shade`, but nothing is lit here.
// Emission is what the surface itself emits (DESIGN section 10 glow rule): light-independent material emissive times
// texture, after alpha; baked (prelit) and unlit materials keep their native colour as emission.
fn g_surface(i: VOut, front: bool, test: bool) -> GOut {
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    let material_alpha = select(mat.color.a, i.fade, i.fade < 1.0);
    let texture_alpha = select(t.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5);
    let alpha = texture_alpha * i.color.a * material_alpha;
    if native_alpha_test() && !native_alpha_pass(alpha) { discard; }
    if test && !native_alpha_test() && alpha < 0.5 { discard; }
    var flags = 0u;
    if (u32(mat.native.z) & 2u) != 0u || mat.scroll.z > 1.5 { flags |= MF_NO_FOG; }
    if (u32(mat.native.z) & 128u) != 0u { flags |= MF_ACTOR; }
    let n = g_normal(i, front);
    let baked = native_unlit() || mat.emissive.w > 1.5;
    if baked {
        // Room-baked / unlit: the native Gouraud result (light already holds vertex colour and lamps) is the pixel.
        let native = min(t.rgb * i.tint * i.light, vec3<f32>(1.0));
        return g_pack(vec3<f32>(0.0), flags | MF_BAKED, n, 0.9, lin(native));
    }
    var emission_g = vec3<f32>(0.0);
    var lit = vec3<f32>(1.0);
    let emissive = mat.emissive.rgb;
    if any(emissive > vec3<f32>(0.0)) {
        // D3D material emissive is light-independent; channels at >= 1 saturate native lighting entirely.
        emission_g = t.rgb * i.tint * to_g(emissive);
        lit = select(vec3<f32>(1.0), vec3<f32>(0.0), emissive >= vec3<f32>(1.0));
    }
    if mat.emissive.w > 0.5 && mat.emissive.w < 1.5 {
        emission_g = max(emission_g, t.rgb * i.tint * t.a); // alpha is a self-illumination mask
    }
    var aux = 0.85;
    if mat.spec.w > 0.0 && any(mat.spec.rgb > vec3<f32>(0.0)) { aux = clamp(sqrt(2.0 / (mat.spec.w + 2.0)), 0.12, 1.0); }
    return g_pack(t.rgb * i.tint * lit, flags, n, aux, lin(emission_g));
}

@fragment
fn fs_opaque(i: VOut, @builtin(front_facing) front: bool) -> GOut { return g_surface(i, front, false); }
@fragment
fn fs_test(i: VOut, @builtin(front_facing) front: bool) -> GOut { return g_surface(i, front, true); }

// Terrain: tile texture is the albedo; the native lightmap palette replaces the sky-ambient share (it holds the
// baked terrain light incl. building/tree shadow), stored as a [0,1] factor of the device ambient in G1.b.
fn g_ground(i: VOut, front: bool, palette: bool) -> GOut {
    var open = 0.5;
    if palette {
        let p = ground_palette(i);
        open = clamp(luma(p) / max(luma(g.ambient_g.rgb), 0.05), 0.0, 2.0) * 0.5;
    }
    let tile = ground_tile(i); // may discard: after the palette sample, like the native bodies
    return g_pack(tile, MF_GROUND, g_normal(i, front), open, vec3<f32>(0.0));
}
@fragment
fn fs_ground_fused(i: VOut, @builtin(front_facing) front: bool) -> GOut { return g_ground(i, front, true); }
@fragment
fn fs_ground_base(i: VOut, @builtin(front_facing) front: bool) -> GOut { return g_ground(i, front, false); }
@fragment
fn fs_ground_simple(i: VOut, @builtin(front_facing) front: bool) -> GOut { return g_ground(i, front, false); }
