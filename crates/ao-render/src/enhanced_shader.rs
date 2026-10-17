//! Enhanced-only shader generation; the faithful WGSL is never modified.

pub(super) fn source(shadows: bool, ao: bool, bloom: bool, hdr: bool) -> String {
    // The HDR sun mask needs the per-vertex sun shares even when shadows are off.
    let sun_shares = shadows || hdr;
    let mut source = crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl")).into_owned();
    source = source.replace("    @location(12) env_uv: vec2<f32>,", "    @location(12) env_uv: vec2<f32>,\n    @location(13) ao_info: vec3<f32>,\n    @location(14) sun_share: vec3<f32>,\n    @location(15) sun_spec_share: vec3<f32>,");
    source = source.replace("    var o: VOut;", "    var o: VOut;\n    o.ao_info = vec3<f32>(0.0);\n    o.sun_share = vec3<f32>(0.0);\n    o.sun_spec_share = vec3<f32>(0.0);");
    source = source.replace("    o.dlight = l.dlight;", "    o.dlight = l.dlight;\n    o.ao_info = l.ao_info;\n    o.sun_share = select(l.sun_share, vec3<f32>(0.0), prelit);\n    o.sun_spec_share = select(l.sun_spec_share, vec3<f32>(0.0), prelit);");
    // Water uses authored WaterLight, not the outdoor sun, and has no AO.
    let liquid_start = source.find("fn vs_liquid(").expect("native liquid vertex");
    let liquid_end = liquid_start + source[liquid_start..].find("\n}").expect("liquid end");
    let liquid = source[liquid_start..liquid_end].replace("    o.ao_info = l.ao_info;\n    o.sun_share = select(l.sun_share, vec3<f32>(0.0), prelit);\n    o.sun_spec_share = select(l.sun_spec_share, vec3<f32>(0.0), prelit);\n", "");
    source.replace_range(liquid_start..liquid_end, &liquid);
    source = source.replace("struct Lit {", "struct Lit {\n    ao_info: vec3<f32>,\n    sun_share: vec3<f32>,\n    sun_spec_share: vec3<f32>,");
    // Reuse the complete native sum, never a regrouped partial sum: Metal's
    // fast math otherwise reassociates the beauty calculation by an UNORM LSB.
    source = source.replace("    o.light = min(to_g(emissive) + g.ambient_g.rgb + sun_diffuse * max(sn, 0.0) + dl[0], vec3<f32>(1.0));", "    let native_raw = to_g(emissive) + g.ambient_g.rgb + sun_diffuse * max(sn, 0.0) + dl[0];\n    o.light = min(native_raw, vec3<f32>(1.0));");
    source = source.replace("    o.spec = min(to_g(specular) * (dl[1] + sun_spec), vec3<f32>(1.0));", "    let native_spec = to_g(specular) * (dl[1] + sun_spec);\n    o.spec = min(native_spec, vec3<f32>(1.0));");
    source = source.replace("    return o;\n}\n\n// mode:", "    let excess = max(native_raw - vec3<f32>(1.0), vec3<f32>(0.0));\n    o.ao_info = vec3<f32>(select(0.0, 1.0, all(emissive <= vec3<f32>(0.0))), 0.0, 0.0);\n    o.sun_share = max(sun_diffuse * max(sn, 0.0) - excess, vec3<f32>(0.0));\n    o.sun_spec_share = max(to_g(specular) * sun_spec - max(native_spec - vec3<f32>(1.0), vec3<f32>(0.0)), vec3<f32>(0.0));\n    return o;\n}\n\n// mode:");
    if shadows {
        source = source.replace("    var light = i.light;\n    var spec = i.spec;", "    var shadow = 0.0;\n    if any(i.sun_share > vec3<f32>(0.0)) || any(i.sun_spec_share > vec3<f32>(0.0)) { shadow = 1.0 - sun_visibility(i.wpos); }\n    var light = max(i.light - i.sun_share * shadow, vec3<f32>(0.0));\n    var spec = max(i.spec - i.sun_spec_share * shadow, vec3<f32>(0.0));");
    }
    source = source.replace("    o.light = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + vertex_lights(o.wpos, n, 0.0)[0],", "    let ground_local = vertex_lights(o.wpos, n, 0.0)[0];\n    o.light = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + ground_local,");
    source = source.replace("    o.dlight = clamp(o.light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));", "    o.dlight = clamp(o.light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    o.sun_share = vec4<f32>(max(o.light - clamp(ground_local, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.0)), 0.0);\n    o.sun_spec_share = vec4<f32>(max(o.dlight - clamp(ground_local + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.0)), 0.0);");
    if shadows {
        source = source.replace("clamp(i.light + ground_palette(i),", "clamp(i.light - i.sun_share * (1.0 - sun_visibility(i.wpos)) + ground_palette(i),");
        source = source.replace("ground_tile(i) * i.dlight", "ground_tile(i) * max(i.dlight - i.sun_spec_share * (1.0 - sun_visibility(i.wpos)), vec3<f32>(0.0))");
    }
    // Spare varying components carry emission without exceeding 16 variables.
    for (location, field, channel) in [(13, "ao_info", "x"), (14, "sun_share", "y"), (15, "sun_spec_share", "z")] {
        source = source.replace(&format!("@location({location}) {field}: vec3<f32>"), &format!("@location({location}) {field}: vec4<f32>"));
        source = source.replace(&format!("o.{field} = vec3<f32>(0.0);"), &format!("o.{field} = vec4<f32>(0.0, 0.0, 0.0, mat.emissive.{channel});"));
        source = source.replace(&format!("o.{field} = select(l.{field}, vec3<f32>(0.0), prelit);"), &format!("o.{field} = vec4<f32>(select(l.{field}, vec3<f32>(0.0), prelit), o.{field}.w);"));
        source = source.replace("o.ao_info = l.ao_info;", "o.ao_info = vec4<f32>(l.ao_info, o.ao_info.w);");
        source = source.replace(&format!("i.{field}"), &format!("i.{field}.xyz"));
    }
    source = source.replace("    o.effect_time = power.w;", "    o.effect_time = power.w;\n    let enhanced_emissive = select(mat.emissive.rgb, emissive.rgb, emissive.w > 0.5);\n    o.ao_info.w = enhanced_emissive.x;\n    o.sun_share.w = enhanced_emissive.y;\n    o.sun_spec_share.w = enhanced_emissive.z;");
    source = source.replace("        o.light = vec3<f32>(1.0);", "        o.light = vec3<f32>(1.0);\n        o.sun_share = vec4<f32>(0.0, 0.0, 0.0, o.sun_share.w);\n        o.sun_spec_share = vec4<f32>(0.0, 0.0, 0.0, o.sun_spec_share.w);");
    // Retain the native fragment bodies (including discard/coverage), adding
    // attachment wrappers rather than maintaining a second copy of the shader.
    let names: Vec<String> = source.lines().filter_map(|line| line.strip_prefix("fn fs_").and_then(|rest| rest.split_once('(')).map(|(name, _)| format!("fs_{name}"))).collect();
    source = source.replace("@fragment\nfn fs_", "fn enhanced_original_fs_");
    source = source.replace(" -> @location(0) vec4<f32>", " -> vec4<f32>");
    source.push_str(OUTPUT);
    for name in names {
        let ambient = match name.as_str() {
            "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" => "enhanced_ao_receiver(i, beauty)",
            "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" => "enhanced_ground_receiver(i, beauty, false)",
            "fs_ground_shadow" => "enhanced_ground_receiver(i, beauty, true)",
            _ => "vec4<f32>(0.0, 0.0, 0.0, beauty.a)",
        };
        let emission = if name == "fs_ground_shadow" { "vec4<f32>(1.0)" } else if name.starts_with("fs_sky_") || name.starts_with("fs_ground_") || matches!(name.as_str(), "fs_env" | "fs_lava" | "fs_lava_crust") { "vec4<f32>(0.0, 0.0, 0.0, beauty.a)" } else if matches!(name.as_str(), "fs_add" | "fs_mesh_add" | "fs_add_one" | "fs_holo") { "vec4<f32>(beauty.rgb, beauty.a)" } else { "enhanced_emission(i, beauty)" };
        let ambient = if ao { ambient } else { "vec4<f32>(0.0)" };
        let emission = if bloom { emission } else { "vec4<f32>(0.0)" };
        let emission_sample = emission.replace("beauty", "envelope");
        let emission_result = if bloom && name == "fs_ground_shadow" {
            "emission_sample"
        } else if bloom && matches!(name.as_str(), "fs_add" | "fs_mesh_add" | "fs_add_one" | "fs_holo") {
            "vec4<f32>(select(bounded_beauty * (1.0 - native_fog(i.fog)), vec3<f32>(0.0), (u32(mat.native.z) & 16u) != 0u), beauty.a)"
        } else if bloom && name == "fs_blend" {
            "vec4<f32>(select(min(bounded_beauty, max(emission_sample.rgb, vec3<f32>(0.0))), bounded_beauty * (1.0 - native_fog(i.fog)), ((u32(mat.native.z) & 24u) == 8u || native_unlit()) && (u32(mat.native.z) & 16u) == 0u), beauty.a)"
        } else if bloom && matches!(name.as_str(), "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_sprite") {
            "vec4<f32>(select(min(bounded_beauty, max(emission_sample.rgb, vec3<f32>(0.0))), bounded_beauty * (1.0 - native_fog(i.fog)), native_unlit() && (u32(mat.native.z) & 16u) == 0u), beauty.a)"
        } else {
            "vec4<f32>(min(bounded_beauty, max(emission_sample.rgb, vec3<f32>(0.0))), beauty.a)"
        };
        let receiving = shadows && matches!(name.as_str(), "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" | "fs_sprite" | "fs_add" | "fs_mesh_add" | "fs_add_one" | "fs_ground_fused" | "fs_ground_shadow" | "fs_ground_simple");
        let geometric_normal = if receiving { "    var surface = i;\n    surface.n = cross(dpdx(i.wpos), dpdy(i.wpos));\n" } else { "" };
        let fragment_input = if receiving { "surface" } else { "i" };
        let sun = match name.as_str() {
            _ if !hdr => "vec4<f32>(0.0)",
            "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" => "enhanced_sun(i, beauty, false)",
            "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" => "enhanced_sun(i, beauty, true)",
            _ => "vec4<f32>(0.0, 0.0, 0.0, beauty.a)",
        };
        source.push_str(&format!("\n@fragment fn {name}(i: VOut) -> EnhancedOutput {{\n{geometric_normal}    let envelope = vec4<f32>(1.0);\n    let emission_sample = {emission_sample};\n    let beauty = enhanced_original_{name}({fragment_input});\n    let bounded_beauty = clamp(beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    return EnhancedOutput(beauty, {ambient}, {emission_result}, {sun});\n}}\n"));
    }
    source = source.replace("sun_visibility(i.wpos)", "sun_visibility(i.wpos, i.n, i.clip.xy)");
    if shadows {
        append_shadow_receiver(&mut source, 0);
        if hdr {
            // Remember the final cascade-blended visibility for the sun-mask output.
            source = source.replace("fn sun_visibility(", "fn sun_visibility_raw(");
            source.push_str("\nfn sun_visibility(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 { let v = sun_visibility_raw(world, normal, pixel); enhanced_sun_visibility = v; return v; }\n");
        }
    } else {
        source.push_str("\nfn sun_visibility(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 { return 1.0; }\n");
    }
    // Remove disabled effects structurally, not merely by multiplying by zero.
    for (enabled, name) in [(ao, "enhanced_ao_receiver"), (ao, "enhanced_ground_receiver"), (bloom, "enhanced_emission"), (hdr, "enhanced_sun")] {
        if !enabled {
            let start = source.find(&format!("fn {name}(")).expect("enhanced helper");
            let end = start + source[start..].find("\n}\n").expect("enhanced helper end") + 3;
            source.replace_range(start..end, "");
        }
    }
    for (enabled, field, location) in [(ao, "ao_info", 13), (sun_shares, "sun_share", 14), (sun_shares, "sun_spec_share", 15)] {
        if !enabled {
            source = source.replace(&format!("select(l.{field}, vec3<f32>(0.0), prelit)"), "vec3<f32>(0.0)");
            source = source.replace(&format!("l.{field}"), "vec3<f32>(0.0)");
            source = source.lines().filter(|line| {
                !line.starts_with(&format!("    {field}: vec3<f32>,"))
                    && !line.starts_with(&format!("    o.{field} = max("))
                    && !line.starts_with(&format!("    o.{field} = vec3<f32>("))
                    && !line.starts_with(&format!("    o.{field} = vec4<f32>(max("))
            }).collect::<Vec<_>>().join("\n");
            if !bloom {
                source = source.lines().filter(|line| {
                    !line.contains(&format!("@location({location}) {field}:"))
                        && !line.trim_start().starts_with(&format!("o.{field} ="))
                }).collect::<Vec<_>>().join("\n");
            }
        }
    }
    if !sun_shares {
        source = source.replace("i.sun_share.xyz", "vec3<f32>(0.0)");
        source = source.replace("i.sun_spec_share.xyz", "vec3<f32>(0.0)");
    }
    if !bloom {
        source = source.lines().filter(|line| !line.contains("let enhanced_emissive =") && !line.contains(".w = enhanced_emissive.")).collect::<Vec<_>>().join("\n");
        for field in ["ao_info", "sun_share", "sun_spec_share"] {
            source = source.replace(&format!(", o.{field}.w)"), ", 0.0)");
        }
    }
    source
}

/// Reuse the world receiver verbatim; grass packs it beside its cell buffers.
pub(super) fn append_shadow_receiver(source: &mut String, binding_offset: u32) {
    let shader = crate::shader_source::load("sun_shadows.wgsl", include_str!("sun_shadows.wgsl"));
    let declarations = shader.split("@group(3) @binding(3)").next().expect("shadow receiver declarations");
    for line in declarations.lines() {
        let mut line = line.to_owned();
        for binding in 0..3 {
            let old = format!("@group(3) @binding({binding})");
            if line.contains(&old) {
                line = line.replace(&old, &format!("@group(3) @binding({})", binding + binding_offset));
                break;
            }
        }
        source.push_str(&line);
        source.push('\n');
    }
    source.push_str(&shader[shader.find("// Vogel (golden-angle) disk").expect("shadow receiver function")..]);
}

const OUTPUT: &str = r#"
struct EnhancedOutput {
    @location(0) beauty: vec4<f32>,
    @location(1) ambient: vec4<f32>,
    @location(2) emission: vec4<f32>,
    @location(3) sun: vec4<f32>,
}
// AO removes (receiver share) x (occlusion x AO_SKY_BOOST, <= 1) of the diffuse
// colour. Sky-fill is fully eligible; only AO_SUN_CAP of the direct-sun share is,
// so contact creases ground the sun-lit surface without cutting its light:
// darkening only, never below the ambient-only look by more than that cap, and
// bounded by the diffuse colour (no effect on fog, specular, emission or sky).
// AO_SKY_BOOST must match SKY_BOOST in ambient_occlusion.wgsl.
const AO_SKY_BOOST = 2.0;
const AO_SUN_CAP = 0.25;
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
    // Liquid emission is native lighting authoring, not a bloom source.
    // SourceColorOne and authored unlit materials use actual post-body beauty.
    if (u32(mat.native.z) & 24u) != 0u || native_unlit() { return vec4<f32>(0.0); }
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    var emission = vec3<f32>(0.0);
    if mat.emissive.w < 1.5 {
        emission = t.rgb * i.tint * min(i.light, to_g(vec3<f32>(i.ao_info.w, i.sun_share.w, i.sun_spec_share.w)));
    }
    if mat.emissive.w > 0.5 && mat.emissive.w < 1.5 && (i.effect == 0u || i.effect == 3u || i.effect == 7u) {
        emission = max(emission, t.rgb * i.tint * max(min(i.light + t.a, vec3<f32>(1.0)) - i.light, vec3<f32>(0.0)));
    }
    emission = min(beauty.rgb, emission * (1.0 - native_fog(i.fog)));
    return vec4<f32>(emission, beauty.a);
}
var<private> enhanced_sun_visibility: f32 = 1.0;
// Daylight sun weight for the HDR stage: the surface's own direct-sun share, after the shadow map and
// the native fog ramp. Zero for unlit/glow/effect surfaces, so only receivers lit by the sun can brighten.
fn enhanced_sun(i: VOut, beauty: vec4<f32>, ground: bool) -> vec4<f32> {
    if !ground && (native_unlit() || i.effect != 0u || (u32(mat.native.z) & 16u) != 0u || (mat.emissive.w > 0.5 && mat.emissive.w < 1.5)) {
        return vec4<f32>(0.0, 0.0, 0.0, beauty.a);
    }
    let fog = select(native_fog(i.fog), ground_fog(i), ground);
    let share = dot(i.sun_share.xyz, vec3<f32>(0.2126, 0.7152, 0.0722));
    return vec4<f32>(share * enhanced_sun_visibility * (1.0 - fog), share, enhanced_sun_visibility, beauty.a);
}
"#;

#[cfg(test)]
mod tests {
    #[test]
    fn enhanced_preserves_native_entrypoints_and_separates_baked_light() {
        let source = super::source(false, true, true, false);
        assert!(source.contains("@fragment fn fs_ground_shadow"));
        assert!(source.contains("@fragment fn fs_lava_crust"));
        assert!(!source.contains("@group(3)"));
        assert!(source.contains("o.ao_info = vec3<f32>(select(0.0, 1.0, all(emissive"));
        assert!(source.contains("let baked = select(1.0, 0.2"));
        assert!(!include_str!("shader.wgsl").contains("EnhancedOutput"));
        // The receiver's sun cap assumes the composite's boost; the two constants must agree.
        let boost = |text: &str, name: &str| text.split(name).nth(1).and_then(|rest| rest.split(['=', ';']).nth(1)).map(str::trim).map(str::to_owned);
        assert_eq!(boost(super::OUTPUT, "const AO_SKY_BOOST"), boost(include_str!("ambient_occlusion.wgsl"), "const SKY_BOOST"));
        assert_eq!(boost(super::OUTPUT, "const AO_SKY_BOOST").as_deref(), Some("2.0"));
        assert!(super::OUTPUT.contains("const AO_SUN_CAP = 0.25;"), "sun share removal is capped at 25%");
        let ao_only = super::source(false, true, false, false);
        assert!(!ao_only.contains("sun_share:"));
        assert!(!ao_only.contains("sun_spec_share:"));
        assert!(!ao_only.contains("fn enhanced_emission"));
        let shadows_only = super::source(true, false, false, false);
        assert!(!shadows_only.contains("ao_info:"));
        assert!(!shadows_only.contains("fn enhanced_ao_receiver"));
    }

    #[test]
    fn enhanced_shader_validates_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("Enhanced WGSL requires offscreen GPU");
        for flags in 0..16 {
            let (shadows, ao, bloom, hdr) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Enhanced ambient/emissive/shadow syntax regression"),
                source: wgpu::ShaderSource::Wgsl(super::source(shadows, ao, bloom, hdr).into()),
            });
        }
    }
}
