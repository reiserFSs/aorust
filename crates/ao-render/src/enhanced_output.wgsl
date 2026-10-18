struct EnhancedOutput {
    @location(0) beauty: vec4<f32>,
    @location(1) ambient: vec4<f32>,
    @location(2) emission: vec4<f32>,
    @location(3) sun: vec4<f32>,
}
// AO removes the sky-fill share and up to 60% of direct diffuse light in a fully
// occluded crease. The estimate stays bounded by diffuse colour, never specular,
// emission, sky or fog. The former 25% direct cap hid contacts in full sun.
// AO_SKY_BOOST must match SKY_BOOST in ambient_occlusion.wgsl.
const AO_SKY_BOOST = 1.25;
const AO_SUN_CAP = 0.6;
fn ao_share(sky_fraction: vec3<f32>) -> vec3<f32> {
    return sky_fraction + (vec3<f32>(1.0) - sky_fraction) * (AO_SUN_CAP / AO_SKY_BOOST);
}
// The receiving colour is evaluated after the native body: no new texture
// samples or derivatives can occur after its cutout discard.
fn enhanced_ao_receiver(i: VOut, beauty: vec4<f32>) -> vec4<f32> {
    if native_unlit() || i.ao_info.x < 0.5 || i.effect != 0u || (u32(mat.native.z) & 16u) != 0u {
        return vec4<f32>(0.0, 0.0, 0.0, beauty.a);
    }
    if mat.emissive.w > 0.5 && mat.emissive.w < 1.5 {
        return vec4<f32>(0.0, 0.0, 0.0, beauty.a);
    }
    let fog = native_fog(i.fog);
    let fog_colour = select(g.fog_g.rgb, to_g(mat.flags.yzw), mat.scroll.z > 1.5);
    let diffuse = max(beauty.rgb - fog_colour * fog - max(i.spec, vec3<f32>(0.0)) * (1.0 - fog), vec3<f32>(0.0));
    let indirect_fraction = clamp(g.ambient_g.rgb / max(i.light, vec3<f32>(0.001)), vec3<f32>(0.0), vec3<f32>(1.0));
    let receiving_fraction = ao_share(indirect_fraction);
    let baked = select(1.0, 0.2, mat.emissive.w > 1.5);
    return vec4<f32>(min(clamp(beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), diffuse * receiving_fraction * baked), beauty.a);
}
fn enhanced_ground_receiver(i: VOut, beauty: vec4<f32>, multiplier: bool) -> vec4<f32> {
    // The native two-pass ground shadow multiplies the previous receiving
    // colour exactly as it multiplies the previous beauty colour.
    if multiplier { return vec4<f32>(clamp(beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), beauty.a); }
    let diffuse = max(beauty.rgb - g.fog_g.rgb * ground_fog(i), vec3<f32>(0.0));
    // The baked terrain light has no sun/sky split here; i.light's ambient share stands in (over-estimating the sky share only darkens more, still <= diffuse).
    let sky_fraction = clamp(g.ambient_g.rgb / max(i.light, vec3<f32>(0.001)), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(min(clamp(beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), diffuse * ao_share(sky_fraction)), beauty.a);
}
fn enhanced_emission(i: VOut, beauty: vec4<f32>) -> vec4<f32> {
    // SetMaterial (Randy1004b199) supplies a lighting floor, not a glow flag.
    // Only channels that alone saturate native D3D7 lighting are self-lit;
    // partial emission (Shore Snake's 0.5 grey) still depends on scene light.
    // Alpha mode 5 (Randy10040645) authors a self-illumination mask: only
    // fully saturated mask texels are independent of scene lighting.
    if (u32(mat.native.z) & 24u) != 0u || native_unlit() { return vec4<f32>(0.0); }
    let authored_emissive = vec3<f32>(i.ao_info.w, i.sun_share.w, i.sun_spec_share.w);
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    var emission = vec3<f32>(0.0);
    if mat.emissive.w < 1.5 {
        let self_lit = select(vec3<f32>(0.0), to_g(authored_emissive), authored_emissive >= vec3<f32>(1.0));
        emission = t.rgb * i.tint * min(i.light, self_lit);
    }
    if mat.emissive.w > 0.5 && mat.emissive.w < 1.5 && (i.effect == 0u || i.effect == 3u || i.effect == 7u) {
        emission = max(emission, t.rgb * i.tint * select(0.0, 1.0, t.a >= 1.0));
    }
    emission = min(beauty.rgb, emission * (1.0 - native_fog(i.fog)));
    return vec4<f32>(emission, beauty.a);
}
// Sample the full conservative lattice, not the selected ROAM triangle's
// vertex alphas: coarse triangles can span many roof/shoreline cells.
fn terrain_sky_open(world: vec3<f32>) -> f32 {
    let dimensions = vec2<f32>(textureDimensions(holo1));
    let lattice = clamp(vec2<f32>(world.x, -world.z) * mat.uv_camera.xy + mat.uv_camera.zw,
        vec2<f32>(0.0), vec2<f32>(1.0));
    let uv = (lattice * (dimensions - vec2<f32>(1.0)) + vec2<f32>(0.5)) / dimensions;
    return textureSampleLevel(holo1, samp_clamp, uv, 0.0).r;
}
var<private> enhanced_sun_visibility: f32 = 1.0;
// r: fog/shadow-attenuated directional share; g: roof-tested, fog-attenuated terrain sky visibility;
// b: native fog amount. Terrain alpha defaults closed until collision baking.
fn enhanced_sun(i: VOut, beauty: vec4<f32>, ground: bool, sky_visibility: f32) -> vec4<f32> {
    // A single frame-uniform fog colour cannot describe material-local fog.
    // Keep those receivers native rather than expose a wrongly subtracted surface.
    if mat.scroll.z > 1.5 { return vec4<f32>(0.0, 0.0, 0.0, beauty.a); }
    // Native liquid lighting uses WaterLight, never the outdoor sun shares.
    // Only cloud shadows opt water in; HDR and geometric sun shadows stay native.
    if !ground && g.water_dir.w > 0.0 && (u32(mat.native.z) & 16u) != 0u && !native_unlit() && i.effect == 0u && !(mat.emissive.w > 0.5 && mat.emissive.w < 1.5) {
        let fog = native_fog(i.fog);
        let direct = clamp(i.light - to_g(mat.emissive.rgb) - g.ambient_g.rgb - i.dlight, vec3<f32>(0.0), vec3<f32>(1.0));
        let share = dot(direct, vec3<f32>(0.2126, 0.7152, 0.0722));
        return vec4<f32>(share * (1.0 - fog), 0.0, fog, beauty.a);
    }
    if !ground && (native_unlit() || i.effect != 0u || (u32(mat.native.z) & 16u) != 0u || (mat.emissive.w > 0.5 && mat.emissive.w < 1.5)) {
        return vec4<f32>(0.0, 0.0, 0.0, beauty.a);
    }
    let fog = select(native_fog(i.fog), ground_fog(i), ground);
    let share = dot(i.sun_share.xyz, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sky_open = sky_visibility * (1.0 - fog);
    return vec4<f32>(share * enhanced_sun_visibility * (1.0 - fog), sky_open, fog, beauty.a);
}
