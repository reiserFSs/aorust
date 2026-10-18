// Surface lighting changes before the native material/fog equation; admission,
// authored normals, alpha and fog extinction remain native.
fn authored_moon_add(input: VOut, authored_normal: vec3<f32>) -> vec3<f32> {
    if g.moon_direction.w <= 0.0 || native_unlit() || mat.emissive.w > 1.5 || (input.effect & 0x7fffffffu) != 0u {
        return vec3<f32>(0.0);
    }
    let normal_length = length(authored_normal);
    if normal_length <= 0.000001 { return vec3<f32>(0.0); }
    // One global copy of the native lattice serves both materials and terrain.
    let exposed = enhanced_sky_open(input.wpos);
    let cosine = max(dot(authored_normal / normal_length, g.moon_direction.xyz), 0.0);
    return g.moon_g.rgb * cosine * exposed * MOON_VISIBILITY;
}
fn authored_moon_receiver(input: VOut, authored_normal: vec3<f32>, ground: bool) -> VOut {
    var receiver = input;
    if native_unlit() || mat.emissive.w > 1.5 || (input.effect & 0x7fffffffu) != 0u { return receiver; }
    var light = input.light;
    var dlight = input.dlight;
    let retained = max(dlight, to_g(mat.emissive.rgb));
    light -= max(light - retained, vec3<f32>(0.0)) * g.moon_g.w;
    if ground {
        light = max(input.light - input.sun_share.xyz * g.moon_g.w, vec3<f32>(0.0));
        dlight = max(dlight - g.ambient_g.rgb * g.moon_g.w, vec3<f32>(0.0));
    }
    let illumination = authored_moon_add(input, authored_normal);
    light += illumination;
    dlight += illumination;
    receiver.light = light;
    receiver.dlight = dlight;
    return receiver;
}
