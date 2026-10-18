// GGX / height-correlated Smith / Schlick port of ao-vk lighting.glsl
// a0dc47ea2c853d26a4491d5a25ef0331f714b6ce, Copyright (c) 2026 wannabuh.
// MIT License; full permission and warranty notice in THIRD_PARTY.
var<private> authored_normal: vec3<f32>;
fn authored_ggx(n: vec3<f32>, l: vec3<f32>, v: vec3<f32>, alpha: f32, f0: vec3<f32>) -> vec3<f32> {
    let nl = max(dot(n, l), 0.0);
    let h = (l + v) / max(length(l + v), 1e-6);
    let nh = max(dot(n, h), 0.0);
    let nv = max(dot(n, v), 1e-4);
    let vh = max(dot(v, h), 0.0);
    let a2 = alpha * alpha;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    let ndf = a2 / (3.14159265 * d * d);
    let vis = 0.5 / (nl * sqrt(nv * nv * (1.0 - a2) + a2) + nv * sqrt(nl * nl * (1.0 - a2) + a2) + 1e-5);
    let f = pow(1.0 - vh, 5.0);
    // Authored black channels remain black: Schlick must not invent a material.
    let fresnel = select(f0 + (vec3<f32>(1.0) - f0) * f, vec3<f32>(0.0), f0 <= vec3<f32>(0.0));
    return fresnel * (ndf * vis * nl * 3.14159265);
}
fn authored_specular(i: VOut, native_spec: vec3<f32>) -> vec3<f32> {
    let n = authored_normal / max(length(authored_normal), 1e-6);
    // Evaluate derivatives before any varying-dependent branches or alpha discard.
    let dx = dpdx(n);
    let dy = dpdy(n);
    let variance = max(dot(dx, dx), dot(dy, dy));
    let footprint = max(length(fwidth(i.wpos)), 1e-6);
    let visibility = sun_visibility(i.wpos, i.n, i.clip.xy);
    let power = i.tint.w;
    // Blinn exponent -> GGX alpha; normal variance broadens the lobe in alpha^2.
    let alpha = sqrt(clamp(2.0 / (max(power, 0.0) + 2.0) + variance, 0.0004, 1.0));
    let f0 = clamp(vec3<f32>(i.light.w, i.dlight.w, i.spec.w), vec3<f32>(0.0), vec3<f32>(1.0));
    if native_unlit() || mat.emissive.w > 1.5 || i.effect != 0u || power <= 0.0 || !any(f0 > vec3<f32>(0.0)) {
        return native_spec;
    }
    let v = (g.eye.xyz - i.wpos) / max(length(g.eye.xyz - i.wpos), 1e-6);
    let local = authored_local_spec(i.wpos, n, alpha, f0)[1];
    let sun = visibility * smoothstep(0.03, 0.25, g.sun_dir.y) * g.sun_g.rgb * g.sun_g.w * authored_ggx(n, g.sun_dir.xyz, v, alpha, f0);
    // Enhanced may add authored highlights at night; native fog and source
    // admission still apply, and the sun contribution retains receiver shadows.
    return max(local + sun, vec3<f32>(0.0));
}
