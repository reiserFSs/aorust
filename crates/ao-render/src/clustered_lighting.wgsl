// The receiver's smooth-normal scan is also material_diffuse's base term.
var<private> clustered_local_value: vec3<f32>;

// Native gamma-space local lighting, with conservative one-pixel range-edge
// coverage instead of an invented broad radial falloff. Spatial lists contain
// only the current frame's 64 admitted sources; no actor history is retained.
fn clustered_pixel_lights(p: vec3<f32>, n: vec3<f32>, footprint: f32) -> vec3<f32> {
    let grid = dyn_lights[64u];
    let metadata = bitcast<vec4<u32>>(dyn_lights[65u]);
    if any(metadata.xyz == vec3<u32>(0u)) { return vec3<f32>(0.0); }
    let c = vec3<i32>(floor((p - grid.xyz) / grid.w));
    if any(c < vec3<i32>(0)) || any(c >= vec3<i32>(metadata.xyz)) { return vec3<f32>(0.0); }
    let cell = (u32(c.z) * metadata.y + u32(c.y)) * metadata.x + u32(c.x);
    let list = bitcast<vec4<u32>>(dyn_lights[metadata.w + cell]);
    var diffuse = vec3<f32>(0.0);
    for (var k = 0u; k < min(list.y, 64u); k += 1u) {
        let index = bitcast<u32>(dyn_lights[list.x + k].x);
        let li = 66u + min(index, 63u) * 4u;
        let a = dyn_lights[li];
        let d = a.xyz - p;
        let dist = length(d);
        if a.w <= 0.0 || dist > a.w { continue; }
        let col = dyn_lights[li + 1u];
        let att = dyn_lights[li + 2u];
        let l = d / max(dist, 1e-3);
        var intensity = select(1.0 - dist / a.w, 1.0 / (att.x + dist * (att.y + dist * att.z)), att.x >= 0.0);
        if col.w < 1.5 {
            let rho = dot(-l, dyn_lights[li + 3u].xyz);
            intensity *= clamp((rho - col.w) / max(att.w - col.w, 1e-4), 0.0, 1.0);
        }
        intensity *= 1.0 - smoothstep(max(0.0, a.w - footprint), a.w, dist);
        diffuse += col.rgb * (intensity * max(dot(n, l), 0.0));
    }
    return diffuse;
}

fn clustered_directional(n_in: vec3<f32>) -> vec3<f32> {
    let n = n_in / max(length(n_in), 1e-6);
    var base = g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0);
    for (var k = 0u; k < u32(g.dyn_info.y); k += 1u) {
        base += g.directional[k * 2u + 1u].rgb * max(dot(n, g.directional[k * 2u].xyz), 0.0);
    }
    return base;
}

fn clustered_base(n: vec3<f32>, emissive: vec3<f32>) -> vec3<f32> {
    return to_g(emissive) + g.ambient_g.rgb + clustered_directional(n);
}

fn clustered_receiver(i: VOut, envelope: vec3<f32>) -> vec3<f32> {
    let n = clustered_normal / max(length(clustered_normal), 1e-6);
    let footprint = max(length(fwidth(i.wpos)), 1e-6);
    let local = clustered_pixel_lights(i.wpos, n, footprint);
    clustered_local_value = local;
    if native_unlit() || (i.effect != 0u && i.effect != 3u && i.effect != 7u) {
        return envelope;
    }
    let base = max(i.color.rgb - CLUSTER_SUN_SHADOW, vec3<f32>(0.0));
    let candidate = clustered_night_light(i, n, false, base + local, local + to_g(mat.emissive.rgb));
    return min(candidate, vec3<f32>(1.0));
}

fn clustered_ground(i: VOut, ambient: bool) -> vec3<f32> {
    let n = clustered_normal / max(length(clustered_normal), 1e-6);
    let local = clustered_pixel_lights(i.wpos, n, max(length(fwidth(i.wpos)), 1e-6));
    clustered_local_value = local;
    let base = max(i.color.rgb - CLUSTER_SUN_SHADOW, vec3<f32>(0.0)) + select(vec3<f32>(0.0), g.ambient_g.rgb, ambient);
    return min(clustered_night_light(i, n, true, base + local, local), vec3<f32>(1.0));
}

fn clustered_receiver_spec(i: VOut, original: vec3<f32>) -> vec3<f32> {
    let n = clustered_normal / max(length(clustered_normal), 1e-6);
    let footprint = max(length(fwidth(i.wpos)), 1e-6);
    let power = i.tint.w;
    let local = clustered_native_spec(i.wpos, n, footprint, power);
    if native_unlit() || mat.emissive.w > 1.5 || i.effect != 0u {
        return original;
    }
    let material_spec = vec3<f32>(i.light.w, i.dlight.w, i.spec.w);
    let v = normalize(g.eye.xyz - i.wpos);
    var directional = vec3<f32>(0.0);
    if power > 0.0 && dot(n, g.sun_dir.xyz) > 0.0 {
        directional = g.sun_g.rgb * g.sun_g.w * pow(max(dot(n, normalize(g.sun_dir.xyz + v)), 0.0), power) * sun_visibility(i.wpos, i.n, i.clip.xy);
    }
    for (var k = 0u; k < u32(g.dyn_info.y); k += 1u) {
        let l = g.directional[k * 2u].xyz;
        if power > 0.0 && dot(n, l) > 0.0 {
            directional += g.directional[k * 2u + 1u].rgb * pow(max(dot(n, normalize(l + v)), 0.0), power);
        }
    }
    return min(material_spec * (local + directional), vec3<f32>(1.0));
}

// The moon/night shader replaces this no-op only when a night mode is enabled.
// Keep local source energy separate so Dark never suppresses admitted lamps.
fn clustered_night_light(i: VOut, normal: vec3<f32>, ground: bool, light: vec3<f32>, retained: vec3<f32>) -> vec3<f32> {
    return light;
}
