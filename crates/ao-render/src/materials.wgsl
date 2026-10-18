// ao-vk ffp_main.glsl BendNormal/NormalMapNormal and lighting.glsl GGX material
// conventions, Copyright (c) 2026 wannabuh, MIT; see THIRD_PARTY.
@group(1) @binding(3) var generated_normal_tex: texture_2d<f32>;
@group(1) @binding(4) var material_normal_tex: texture_2d<f32>;
@group(1) @binding(5) var material_orm_tex: texture_2d<f32>;
@group(1) @binding(6) var material_emissive_tex: texture_2d<f32>;
const MATERIAL_GENERATED: bool = false;
const MATERIAL_MAPS: bool = false;
const MATERIAL_PBR: bool = false;
const MATERIAL_STATIC_PROVENANCE: bool = false;
var<private> material_normal_value: vec3<f32>;
var<private> material_base_normal: vec3<f32>;
var<private> material_orm_value: vec3<f32>;
var<private> material_emissive_value: vec3<f32>;
var<private> material_mask_value: u32;
var<private> material_variance: f32;
var<private> material_static_visibility_value: f32;
var<private> material_visibility_value: f32;
var<private> material_footprint: f32;
var<private> material_albedo_value: vec3<f32>;

fn material_uv(i: VOut) -> vec2<f32> { return i.uv + mat.scroll.xy * g.fog.z; }
fn material_safe_normal(n: vec3<f32>) -> vec3<f32> {
    return n / max(length(n), 1e-6);
}
fn material_bend(n: vec3<f32>, p: vec3<f32>, uv: vec2<f32>, t: vec3<f32>) -> vec3<f32> {
    let nu = material_safe_normal(n);
    let px = dpdx(p);
    let py = dpdy(p);
    let ux = dpdx(uv);
    let uy = dpdy(uv);
    let r1 = cross(py, nu);
    let r2 = cross(nu, px);
    let det = dot(px, r1);
    // PNGs are OpenGL (+Y up); native texture coordinates run down the image.
    let slope = vec2<f32>(-t.x, t.y) / max(t.z, 0.05);
    let world_per_uv = sqrt(max(dot(px, px), dot(py, py)) / max(max(dot(ux, ux), dot(uy, uy)), 1e-12));
    let grad = sign(det) * (dot(slope, ux) * r1 + dot(slope, uy) * r2) * world_per_uv;
    let bent = material_safe_normal(abs(det) * nu - grad);
    return select(bent, nu, abs(det) < 1e-12);
}
// Called before any native alpha discard. The private cache also lets SSR use
// precisely the beauty normal and roughness without another derivative/sample.
fn material_prepare(i: VOut) {
    let uv = material_uv(i);
    material_base_normal = material_safe_normal(i.n);
    material_footprint = max(length(fwidth(i.wpos)), 1e-6);
    material_visibility_value = sun_visibility(i.wpos, i.n, i.clip.xy);
    material_static_visibility_value = material_visibility_value;
    if MATERIAL_STATIC_PROVENANCE {
        material_static_visibility_value = min(material_visibility_value, sun_visibility(i.wpos, cross(dpdx(i.wpos), dpdy(i.wpos)), i.clip.xy));
    }
    material_albedo_value = sample_clamped(tex, uv).rgb * i.tint.xyz;
    let generated = sample_clamped(generated_normal_tex, uv);
    let normal_map = sample_clamped(material_normal_tex, uv);
    let orm = sample_clamped(material_orm_tex, uv);
    let emissive = sample_clamped(material_emissive_tex, uv).rgb;
    material_mask_value = select(0u, u32(round(orm.a * 255.0)), MATERIAL_MAPS);
    var t = vec3<f32>(0.0, 0.0, 1.0);
    var coherence = 1.0;
    if MATERIAL_GENERATED {
        t = (generated.xyz * 255.0 - vec3<f32>(128.0)) / 127.0;
        coherence = generated.a;
    }
    if (material_mask_value & 1u) != 0u {
        t = (normal_map.xyz * 255.0 - vec3<f32>(128.0)) / 127.0;
        coherence = normal_map.a;
    }
    material_normal_value = material_bend(i.n, i.wpos, uv, t);
    let authored_roughness = select(0.7, pow(2.0 / (max(i.tint.w, 0.0) + 2.0), 0.25), i.tint.w > 0.0);
    material_orm_value = select(vec3<f32>(1.0, authored_roughness, 0.0), orm.rgb, (material_mask_value & 2u) != 0u);
    material_emissive_value = select(vec3<f32>(0.0), to_g(emissive), (material_mask_value & 4u) != 0u);
    let dx = dpdx(material_normal_value);
    let dy = dpdy(material_normal_value);
    // Normal mips preserve vector length before renormalization; unresolved
    // slopes widen GGX alpha squared rather than producing distant sparkles.
    let len = clamp(coherence, 0.001, 1.0);
    let normal_variance = 2.0 * (1.0 - len * len) / max(len * (3.0 - len * len), 0.001);
    material_variance = min(1.0, normal_variance + max(dot(dx, dx), dot(dy, dy)));
}
fn material_shading_normal(i: VOut) -> vec3<f32> { return material_normal_value; }
fn material_roughness(i: VOut) -> f32 {
    let r = material_orm_value.y;
    return sqrt(sqrt(clamp(r * r * r * r + material_variance, 0.0004, 1.0)));
}
fn material_metallic(i: VOut) -> f32 { return material_orm_value.z; }
fn material_emissive(i: VOut) -> vec3<f32> { return material_emissive_value; }
fn material_mask(i: VOut) -> u32 { return material_mask_value; }
fn material_alpha(i: VOut) -> f32 {
    let r = material_roughness(i);
    return r * r;
}
fn material_diffuse(i: VOut, native_light: vec3<f32>) -> vec3<f32> {
    let n = material_shading_normal(i);
    let base = material_base_normal;
    let sun_visibility_value = material_visibility_value;
    let sun_delta = g.sun_g.rgb * sun_visibility_value * (max(dot(n, g.sun_dir.xyz), 0.0) - max(dot(base, g.sun_dir.xyz), 0.0));
    let local_delta = vertex_lights(i.wpos, n, 0.0)[0] - vertex_lights(i.wpos, base, 0.0)[0];
    let diffuse_weight = select(1.0, 1.0 - material_metallic(i), MATERIAL_PBR || (material_mask(i) & 2u) != 0u);
    return max(native_light + sun_delta + local_delta, vec3<f32>(0.0)) * material_orm_value.x * diffuse_weight;
}
fn material_reflectivity(i: VOut) -> f32 {
    let authored_f0 = clamp(max(i.light.w, max(i.dlight.w, i.spec.w)), 0.04, 1.0);
    let f0 = mix(authored_f0, 1.0, material_metallic(i));
    return f0 * pow(1.0 - material_roughness(i), 2.0);
}
