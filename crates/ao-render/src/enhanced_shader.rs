//! Enhanced-only shader generation; the faithful WGSL is never modified.

pub(super) fn source(settings: crate::EnhancedSettings) -> String {
    let sun_shadows = settings.sun_shadows || (settings.ground_grass && settings.grass_sun_shadows);
    let shadows = sun_shadows || settings.moon_shadows;
    let ao = settings.ambient_occlusion;
    let bloom = settings.emissive_bloom;
    let hdr = settings.needs_sun_mask();
    let materials = settings.generated_normals || settings.material_maps || settings.material_pbr
        || settings.material_reflections || settings.environment_probe;
    let material_specular = settings.material_specular || settings.material_pbr;
    let reflections = settings.material_reflections || settings.environment_probe;
    let clustered_lighting = settings.clustered_lighting;
    let local_light_shadows = settings.local_light_shadows;
    let night = settings.night_mode != crate::NightMode::AsGame;
    // HDR and night receivers need the native directional shares independently.
    let sun_shares = shadows || hdr || night;
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
    if sun_shadows {
        source = source.replace("    var light = i.light;\n    var spec = i.spec;", "    var shadow = 0.0;\n    if any(i.sun_share > vec3<f32>(0.0)) || any(i.sun_spec_share > vec3<f32>(0.0)) { shadow = 1.0 - sun_visibility(i.wpos); }\n    var light = max(i.light - i.sun_share * shadow, vec3<f32>(0.0));\n    var spec = max(i.spec - i.sun_spec_share * shadow, vec3<f32>(0.0));");
    }
    source = source.replace("    o.light = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + vertex_lights(o.wpos, n, 0.0)[0],", "    let ground_local = vertex_lights(o.wpos, n, 0.0)[0];\n    o.light = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + ground_local,");
    source = source.replace("    o.dlight = clamp(o.light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));", "    o.dlight = clamp(o.light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    o.sun_share = vec4<f32>(max(o.light - clamp(ground_local, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.0)), 0.0);\n    o.sun_spec_share = vec4<f32>(max(o.dlight - clamp(ground_local + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.0)), 0.0);");
    if reflections {
        source = source.replace("all(emissive <= vec3<f32>(0.0))), 0.0, 0.0)", "all(emissive <= vec3<f32>(0.0))), select(0.0, 1.0, any(dl[0] > vec3<f32>(0.0)) || any(dl[1] > vec3<f32>(0.0))), 0.0)");
        source = source.replace("    let ground_local = vertex_lights(o.wpos, n, 0.0)[0];", "    let ground_local = vertex_lights(o.wpos, n, 0.0)[0];\n    o.ao_info.y = select(0.0, 1.0, any(ground_local > vec3<f32>(0.0)));");
    }
    if sun_shadows {
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
    source.push_str(&crate::shader_source::load("enhanced_output.wgsl", OUTPUT));
    if settings.cloud_shadows || reflections {
        source = source.replace("    @location(3) sun: vec4<f32>,", "    @location(3) sun: vec4<f32>,\n    @location(4) water_cloud: vec4<f32>,");
    }
    if reflections {
        source = source.replace("    @location(4) water_cloud: vec4<f32>,", "    @location(4) water_cloud: vec4<f32>,\n    @location(5) reflection_surface: vec4<f32>,\n    @location(6) reflection_mask: vec4<f32>,");
    }
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
        // Presentation text shares native full-bright materials with authored effects.
        // Gate its final seed once, preserving coverage and every native beauty calculation.
        let emission_result = if bloom {
            format!("select({emission_result}, vec4<f32>(0.0, 0.0, 0.0, beauty.a), (u32(mat.native.z) & 32u) != 0u)")
        } else {
            emission_result.to_owned()
        };
        let receiving = shadows && matches!(name.as_str(), "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" | "fs_sprite" | "fs_add" | "fs_mesh_add" | "fs_add_one" | "fs_ground_fused" | "fs_ground_shadow" | "fs_ground_simple");
        let geometric_normal = if receiving { "    var surface = i;\n    surface.n = cross(dpdx(i.wpos), dpdy(i.wpos));\n" } else { "" };
        let fragment_input = if receiving { "surface" } else { "i" };
        let sun = match name.as_str() {
            _ if !hdr => "vec4<f32>(0.0)",
            name if name.starts_with("fs_sky_") => "vec4<f32>(0.0)",
            "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" => "enhanced_sun(i, beauty, false, 0.0)",
            "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" => "enhanced_sun(i, beauty, true, terrain_sky_open(i.wpos))",
            // ZERO/SRCCOLOR must extinguish only direct exposure under a roof;
            // sky eligibility, native fog and coverage belong to the base pass.
            "fs_ground_shadow" if shadows => "vec4<f32>(enhanced_sun_visibility, 1.0, 1.0, 1.0)",
            "fs_ground_shadow" => "vec4<f32>(1.0)",
            _ => "vec4<f32>(0.0, 0.0, 0.0, beauty.a)",
        };
        let water = if settings.cloud_shadows {
            if name == "fs_ground_shadow" { ", vec4<f32>(1.0)".to_owned() }
            else if matches!(name.as_str(), "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend") { ", vec4<f32>(enhanced_water_cloud(i, beauty), 0.0, 0.0, beauty.a)".to_owned() }
            else { ", vec4<f32>(0.0, 0.0, 0.0, beauty.a)".to_owned() }
        } else if reflections { ", vec4<f32>(0.0)".to_owned() } else { String::new() };
        let reflection = if reflections {
            let primary = matches!(name.as_str(), "fs_opaque" | "fs_test" | "fs_fade_opaque" | "fs_fade_test" | "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple");
            if primary {
                ", vec4<f32>(material_shading_normal(i), material_roughness(i)), vec4<f32>(material_reflectivity(i), native_fog(i.fog), material_static_source(i), select(0.0, 1.0, i.effect == 0u && (u32(mat.native.z) & 128u) == 0u))"
            } else { ", vec4<f32>(0.0), vec4<f32>(0.0)" }
        } else { "" };
        source.push_str(&format!("\n@fragment fn {name}(i: VOut) -> EnhancedOutput {{\n{geometric_normal}    let envelope = vec4<f32>(1.0);\n    let emission_sample = {emission_sample};\n    let beauty = enhanced_original_{name}({fragment_input});\n    let bounded_beauty = clamp(beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    return EnhancedOutput(beauty, {ambient}, {emission_result}, {sun}{water}{reflection});\n}}\n"));
    }
    source = source.replace("sun_visibility(i.wpos)", "sun_visibility(i.wpos, i.n, i.clip.xy)");
    if shadows {
        append_shadow_receiver(&mut source, 0);
        if hdr || material_specular {
            // HDR eligibility uses actual light visibility, not the artistic shade floor.
            // Capture before the final mix: beauty retains its native Enhanced shadow strength.
            source = source.replace(
                "    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
                "    enhanced_sun_visibility = visible;\n    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
            );
        }
    } else {
        source.push_str("\nfn sun_visibility(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 { return 1.0; }\n");
    }
    // Remove disabled effects structurally, not merely by multiplying by zero.
    for (enabled, name) in [(ao, "enhanced_ao_receiver"), (ao, "enhanced_ground_receiver"), (bloom, "enhanced_emission"), (hdr, "enhanced_sun"), (settings.cloud_shadows, "enhanced_water_cloud")] {
        if !enabled {
            let start = source.find(&format!("fn {name}(")).expect("enhanced helper");
            let end = start + source[start..].find("\n}\n").expect("enhanced helper end") + 3;
            source.replace_range(start..end, "");
        }
    }
    for (enabled, field, location) in [(ao || reflections, "ao_info", 13), (sun_shares, "sun_share", 14), (sun_shares, "sun_spec_share", 15)] {
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
    if clustered_lighting || local_light_shadows { crate::clustered_lighting::apply(&mut source); }
    if material_specular { crate::authored_specular::append(&mut source, settings.night_mode == crate::NightMode::Moonlit); }
    if materials { crate::authored_specular::append_materials(&mut source, settings); }
    if settings.needs_admitted_lights() { crate::point_shadows::apply_shader(&mut source, local_light_shadows, clustered_lighting); }
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

/// Keep grass and world receivers on the same runtime-loaded AO budget.
pub(super) fn append_ao_share(source: &mut String) {
    let shader = crate::shader_source::load("enhanced_output.wgsl", OUTPUT);
    let start = shader.find("const AO_SKY_BOOST").expect("AO sky budget");
    let function = shader[start..].find("fn ao_share(").expect("AO share helper") + start;
    let end = shader[function..].find("\n}").expect("AO share helper end") + function + 2;
    source.push('\n');
    source.push_str(&shader[start..end]);
    source.push('\n');
}

const OUTPUT: &str = include_str!("enhanced_output.wgsl");

#[cfg(test)]
mod tests {
    #[test]
    fn grass_ao_share_reuses_runtime_world_budget() {
        let shader = crate::shader_source::load("enhanced_output.wgsl", super::OUTPUT);
        let mut helper = String::new();
        super::append_ao_share(&mut helper);
        assert!(shader.contains(helper.trim()), "grass must use the runtime-loaded world AO definition verbatim");
        assert!(helper.trim_start().starts_with("const AO_SKY_BOOST"));
        assert!(helper.contains("const AO_SUN_CAP"));
        assert!(helper.contains("fn ao_share("));
        assert_eq!(helper.matches("fn ").count(), 1, "do not copy world-only receiver dependencies");
        assert!(helper.trim_end().ends_with('}'));
    }

    #[test]
    fn enhanced_preserves_native_entrypoints_and_separates_baked_light() {
        let source = super::source(crate::EnhancedSettings { ambient_occlusion: true, emissive_bloom: true, ..Default::default() });
        assert!(source.contains("@fragment fn fs_ground_shadow"));
        assert!(source.contains("@fragment fn fs_lava_crust"));
        assert!(!source.contains("@group(3)"));
        assert!(source.contains("o.ao_info = vec3<f32>(select(0.0, 1.0, all(emissive"));
        assert!(source.contains("let baked = select(1.0, 0.2"));
        assert!(!include_str!("shader.wgsl").contains("EnhancedOutput"));
        // The receiver's sun cap assumes the composite's boost; the two constants must agree.
        let boost = |text: &str, name: &str| text.split(name).nth(1).and_then(|rest| rest.split(['=', ';']).nth(1)).map(str::trim).map(str::to_owned);
        assert_eq!(boost(super::OUTPUT, "const AO_SKY_BOOST"), boost(include_str!("ambient_occlusion.wgsl"), "const SKY_BOOST"));
        assert_eq!(boost(super::OUTPUT, "const AO_SKY_BOOST").as_deref(), Some("1.25"));
        assert!(super::OUTPUT.contains("const AO_SUN_CAP = 0.6;"), "sunlit creases retain a bounded 60% direct-diffuse budget");
        let hdr = super::source(crate::EnhancedSettings { hdr: true, ..Default::default() });
        let multiplier = hdr.split("@fragment fn fs_ground_shadow(").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(multiplier.contains(", vec4<f32>(1.0));"),
            "without shadows, terrain multiplication preserves the base-pass daylight mask");
        assert!(super::source(crate::EnhancedSettings { sun_shadows: true, hdr: true, ..Default::default() }).contains(
            "enhanced_sun_visibility = visible;\n    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));"),
            "HDR uses raw visibility before beauty's artistic shadow floor");
        let ao_only = super::source(crate::EnhancedSettings { ambient_occlusion: true, ..Default::default() });
        assert!(!ao_only.contains("sun_share:"));
        assert!(!ao_only.contains("sun_spec_share:"));
        assert!(!ao_only.contains("fn enhanced_emission"));
        let shadows_only = super::source(crate::EnhancedSettings { sun_shadows: true, ..Default::default() });
        assert!(!shadows_only.contains("ao_info:"));
        assert!(!shadows_only.contains("fn enhanced_ao_receiver"));
        let clouds = super::source(crate::EnhancedSettings { cloud_shadows: true, ..Default::default() });
        assert!(clouds.contains("@location(4) water_cloud: vec4<f32>"));
        assert!(clouds.contains("vec4<f32>(enhanced_water_cloud(i, beauty), 0.0, 0.0, beauty.a)"));
        assert!(!hdr.contains("@location(4) water_cloud:"));
        assert!(!hdr.contains("fn enhanced_water_cloud("));
    }

    #[test]
    fn daylight_mask_preserves_fog_and_closes_custom_fog_roof_and_native_water_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("daylight mask regression requires offscreen GPU");
        let helper = &super::OUTPUT[super::OUTPUT.find("var<private> enhanced_sun_visibility").unwrap()..];
        let meadow = include_str!("grass.wgsl").lines().find(|line| line.trim_start().starts_with("let daylight =")).unwrap();
        let source = format!(r#"
struct VOut {{ effect: u32, fog: f32, sun_share: vec4<f32>, color: vec4<f32>, light: vec3<f32>, dlight: vec3<f32> }}
struct Material {{ scroll: vec4<f32>, native: vec4<f32>, emissive: vec4<f32> }}
var<private> mat: Material;
struct Globals {{ ambient_g: vec4<f32>, sun_dir: vec4<f32>, water_dir: vec4<f32> }}
var<private> g: Globals;
var<private> grass_direct_visibility: f32;
const LUMA: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);
fn native_unlit() -> bool {{ return (u32(mat.native.z) & 4u) != 0u; }}
fn to_g(v: vec3<f32>) -> vec3<f32> {{ return v; }}
fn native_fog(f: f32) -> f32 {{ return f; }}
fn ground_fog(i: VOut) -> f32 {{ return i.fog; }}
{helper}
@vertex fn vs(@builtin(vertex_index) n: u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(vec2<f32>(f32((n << 1u) & 2u), f32(n & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    if p.x >= 7.0 {{
        mat.native.z = select(16.0, 20.0, p.x >= 9.0 && p.x < 10.0);
        g.water_dir.w = select(0.0, 1.0, p.x >= 8.0);
        mat.emissive = vec4<f32>(vec3<f32>(0.125), select(0.0, 1.0, p.x >= 14.0));
        g.ambient_g = vec4<f32>(vec3<f32>(0.125), 0.0);
        let local = vec3<f32>(0.25);
        var light = vec3<f32>(1.0);
        if p.x >= 10.0 && p.x < 11.0 {{ light = vec3<f32>(0.5); }}
        if p.x >= 12.0 && p.x < 13.0 {{ light = vec3<f32>(1.0, 0.5, 0.5); }}
        let fog = select(0.5, 1.0, p.x >= 11.0 && p.x < 12.0);
        mat.scroll.z = select(0.0, 2.0, p.x >= 13.0 && p.x < 14.0);
        let water = VOut(0u, fog, vec4<f32>(0.0), vec4<f32>(1.0), light, local);
        let beauty = vec4<f32>(0.75, 0.75, 0.75, 0.25);
        let mask = enhanced_sun(water, beauty, false, 1.0);
        let share = select(0.0, enhanced_water_cloud(water, beauty), g.water_dir.w > 0.0);
        return vec4<f32>(mask.r, share, mask.b, mask.a);
    }}
    if p.x >= 4.0 {{
        g.ambient_g.w = select(0.0, 1.0, p.x >= 5.0);
        g.sun_dir.y = 1.0;
        grass_direct_visibility = select(1.0, 0.0, p.x >= 5.0 && p.x < 6.0);
        let sun_share = vec3<f32>(1.0);
        {meadow}
        return vec4<f32>(vec3<f32>(daylight), 1.0);
    }}
    mat.scroll.z = select(0.0, 2.0, p.x >= 1.0 && p.x < 2.0);
    enhanced_sun_visibility = select(1.0, 0.0, p.x >= 2.0);
    let fog = select(0.5, 0.0, p.x >= 2.0);
    let alpha = select(1.0, 0.0, p.x >= 3.0);
    return enhanced_sun(VOut(0u, fog, vec4<f32>(1.0), vec4<f32>(1.0, 1.0, 1.0, alpha), vec3<f32>(1.0), vec3<f32>(0.0)), vec4<f32>(1.0), p.x >= 2.0, 1.0 - alpha);
}}
"#);
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("daylight fog mask regression"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: None,
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format: r.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default() }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let texture = r.device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 15, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut encoder = r.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })], ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.draw(0..3, 0..1);
        }
        r.queue.submit([encoder.finish()]);
        let pixels = crate::texture_to_rgba(&r, &texture, 15, 1).unwrap();
        assert_eq!(&pixels[..4], &[128, 0, 128, 255], "default-fog meshes retain native fog amount");
        assert_eq!(&pixels[4..8], &[0, 0, 0, 255], "custom-fog receivers cannot gain daylight energy");
        assert_eq!(&pixels[8..12], &[0, 0, 0, 255], "roof umbra has neither direct exposure nor sky fill");
        assert_eq!(&pixels[12..16], &[0, 255, 0, 255], "open shade receives only independently proven sky fill");
        assert_eq!(&pixels[16..20], &[0, 0, 0, 255], "bright moonlight cannot lift meadow albedo at authored night");
        assert_eq!(&pixels[20..24], &[0, 0, 0, 255], "the retained shadow floor cannot lift roofed meadow albedo");
        assert_eq!(&pixels[24..28], &[255; 4], "actual direct daylight retains the meadow lift");
        assert_eq!(&pixels[28..32], &[0, 0, 0, 64], "cloud switch zero leaves native water excluded even with HDR");
        assert_eq!(&pixels[32..36], &[0, 64, 0, 64], "cloud-only WaterLight budget is independent of the native sun mask");
        assert_eq!(&pixels[36..40], &[0, 0, 0, 64], "unlit water remains excluded");
        assert_eq!(&pixels[40..44], &[0, 0, 0, 64], "emission, ambient and local lights cannot become water directional light");
        assert_eq!(&pixels[44..48], &[0, 0, 0, 64], "full native fog extinguishes water directional light");
        assert_eq!(&pixels[48..52], &[0, 14, 0, 64], "water cloud share uses luminance, not the brightest channel");
        assert_eq!(&pixels[52..56], &[0, 0, 0, 64], "custom-fog water remains excluded");
        assert_eq!(&pixels[56..60], &[0, 0, 0, 64], "emissive-mode water remains excluded with the cloud switch on");
    }

    #[test]
    fn alpha_water_cloud_budget_preserves_sky_open_sun_mask_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("water blending regression requires GPU");
        let helper = &super::OUTPUT[super::OUTPUT.find("var<private> enhanced_sun_visibility").unwrap()..];
        let source = format!(r#"
struct VOut {{ effect: u32, fog: f32, sun_share: vec4<f32>, light: vec3<f32>, dlight: vec3<f32> }}
struct Material {{ scroll: vec4<f32>, native: vec4<f32>, emissive: vec4<f32> }}
var<private> mat: Material;
struct Globals {{ ambient_g: vec4<f32> }}
var<private> g: Globals;
fn native_unlit() -> bool {{ return false; }}
fn to_g(v: vec3<f32>) -> vec3<f32> {{ return v; }}
fn native_fog(f: f32) -> f32 {{ return f; }}
fn ground_fog(i: VOut) -> f32 {{ return i.fog; }}
{helper}
struct Output {{ @location(3) sun: vec4<f32>, @location(4) water: vec4<f32> }}
@vertex fn vs(@builtin(vertex_index) n: u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(vec2<f32>(f32((n << 1u) & 2u), f32(n & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}}
@fragment fn terrain() -> Output {{
    let i = VOut(0u, 0.25, vec4<f32>(2.0 / 3.0), vec3<f32>(1.0), vec3<f32>(0.0));
    return Output(enhanced_sun(i, vec4<f32>(1.0), true, 1.0), vec4<f32>(0.0));
}}
@fragment fn water() -> Output {{
    mat.native.z = 16.0;
    let i = VOut(0u, 0.5, vec4<f32>(0.0), vec3<f32>(0.5), vec3<f32>(0.0));
    let beauty = vec4<f32>(0.75, 0.75, 0.75, 0.25);
    return Output(enhanced_sun(i, beauty, false, 0.0), vec4<f32>(enhanced_water_cloud(i, beauty), 0.0, 0.0, beauty.a));
}}
"#);
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("real water alpha MRT"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let read_shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water MRT readback"), source: wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var sun: texture_2d<f32>;
@group(0) @binding(1) var water: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) n: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((n << 1u) & 2u), f32(n & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let mask = textureLoad(sun, vec2<i32>(0), 0);
    if p.x < 1.0 { return mask; }
    return vec4<f32>(textureLoad(water, vec2<i32>(0), 0).rrr, 1.0);
}
"#.into()) });
        let read_pipe = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: None,
            vertex: wgpu::VertexState { module: &read_shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &read_shader, entry_point: Some("fs"), targets: &[Some(wgpu::ColorTargetState { format: r.format, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        for samples in [1, 4] {
            if !r.msaa_supported.contains(&samples) || !r.auxiliary_msaa.sample_count_supported(samples) || !r.water_cloud_msaa.sample_count_supported(samples) { continue; }
            let make = |format, count| r.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("water blending regression"), size: wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: count, dimension: wgpu::TextureDimension::D2, format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
            }).create_view(&Default::default());
            let sun = make(crate::enhanced::AUX, 1);
            let budget = make(wgpu::TextureFormat::R16Float, 1);
            let msaa_sun = (samples > 1).then(|| make(crate::enhanced::AUX, samples));
            let msaa_budget = (samples > 1).then(|| make(wgpu::TextureFormat::R16Float, samples));
            let mut captures = Vec::new();
            for clouds in [false, true] {
                let pipeline = |entry, blend| r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("native alpha over sky-open terrain"), layout: None,
                    vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
                    fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), targets: &[
                        None, None, None,
                        Some(wgpu::ColorTargetState { format: crate::enhanced::AUX, blend, write_mask: wgpu::ColorWrites::ALL }),
                        clouds.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend, write_mask: wgpu::ColorWrites::RED }),
                    ][..if clouds { 5 } else { 4 }], compilation_options: Default::default() }),
                    primitive: Default::default(), depth_stencil: None, multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
                });
                let terrain = pipeline("terrain", None);
                let water = pipeline("water", Some(wgpu::BlendState::ALPHA_BLENDING));
                let output = r.device.create_texture(&wgpu::TextureDescriptor {
                    label: None, size: wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
                });
                let output_view = output.create_view(&Default::default());
                fn attachment<'a>(resolved: &'a wgpu::TextureView, msaa: Option<&'a wgpu::TextureView>) -> Option<wgpu::RenderPassColorAttachment<'a>> {
                    Some(wgpu::RenderPassColorAttachment {
                        view: msaa.unwrap_or(resolved), resolve_target: msaa.map(|_| resolved), depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })
                }
                let mut encoder = r.device.create_command_encoder(&Default::default());
                {
                    let attachments = [None, None, None, attachment(&sun, msaa_sun.as_ref()), if clouds { attachment(&budget, msaa_budget.as_ref()) } else { None }];
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { color_attachments: &attachments[..if clouds { 5 } else { 4 }], ..Default::default() });
                    pass.set_pipeline(&terrain); pass.draw(0..3, 0..1);
                    pass.set_pipeline(&water); pass.draw(0..3, 0..1);
                }
                let bind = r.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &read_pipe.get_bind_group_layout(0), entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&sun) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&budget) },
                ] });
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { color_attachments: &[attachment(&output_view, None)], ..Default::default() });
                    pass.set_pipeline(&read_pipe); pass.set_bind_group(0, &bind, &[]); pass.draw(0..3, 0..1);
                }
                r.queue.submit([encoder.finish()]);
                captures.push(crate::texture_to_rgba(&r, &output, 2, 1).unwrap());
            }
            assert_eq!(&captures[0][..4], &captures[1][..4], "{samples}x: cloud switch cannot alter any native sun-mask channel");
            assert_eq!(&captures[1][..4], &[96, 143, 48, 255], "positive terrain sky openness survives native water alpha blending");
            assert_eq!(&captures[0][4..8], &[0, 0, 0, 255]);
            assert_eq!(&captures[1][4..8], &[16, 16, 16, 255], "0.5 WaterLight * 0.5 native fog survival * 0.25 water alpha");
        }
    }

    #[test]
    fn ground_two_pass_roof_mask_multiplies_only_direct_exposure_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("ground two-pass regression requires offscreen GPU");
        for shadows in [true, false] {
            let generated = super::source(crate::EnhancedSettings { sun_shadows: shadows, hdr: true, ..Default::default() });
            let wrapper = generated[generated.find("@fragment fn fs_ground_shadow(").unwrap()..]
                .split("\n}").next().unwrap().replacen("@fragment ", "", 1);
            // Exercise the generated wrapper with a fake roof receiver: beauty
            // retains its artistic floor while raw direct visibility is zero.
            let source = format!(r#"
struct VOut {{ clip: vec4<f32>, n: vec3<f32>, wpos: vec3<f32> }}
struct EnhancedOutput {{ beauty: vec4<f32>, ambient: vec4<f32>, emission: vec4<f32>, sun: vec4<f32> }}
var<private> enhanced_sun_visibility: f32 = 1.0;
fn enhanced_original_fs_ground_shadow(i: VOut) -> vec4<f32> {{
    enhanced_sun_visibility = select(0.0, 1.0, i.clip.x >= 1.0);
    return vec4<f32>(0.4, 0.4, 0.4, 1.0);
}}
{wrapper}
}}
@vertex fn vs(@builtin(vertex_index) n: u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(vec2<f32>(f32((n << 1u) & 2u), f32(n & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}}
@fragment fn base() -> @location(0) vec4<f32> {{
    return vec4<f32>(0.8, 0.6, 0.4, 1.0);
}}
@fragment fn multiply(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    return fs_ground_shadow(VOut(p, vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(p.xy, 0.0))).sun;
}}
"#);
            let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ground two-pass roof mask regression"), source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = |entry, blend| r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None, layout: None,
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState { format: r.format, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default() }),
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
            });
            let base = pipeline("base", None);
            // The native ground shadow pass uses ZERO/SRCCOLOR on every MRT.
            let multiply = pipeline("multiply", Some(wgpu::BlendState {
                color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add },
                alpha: wgpu::BlendComponent::REPLACE,
            }));
            let texture = r.device.create_texture(&wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let mut encoder = r.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view, resolve_target: None, depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })], ..Default::default()
                });
                pass.set_pipeline(&base);
                pass.draw(0..3, 0..1);
                pass.set_pipeline(&multiply);
                pass.draw(0..3, 0..1);
            }
            r.queue.submit([encoder.finish()]);
            let pixels = crate::texture_to_rgba(&r, &texture, 2, 1).unwrap();
            assert_eq!(&pixels[..4], &[if shadows { 0 } else { 204 }, 153, 102, 255],
                "roof shadow removes base direct exposure only; sky/fog/coverage survive, and HDR without shadows stays neutral");
            assert_eq!(&pixels[4..], &[204, 153, 102, 255], "sunlit terrain preserves every base-mask channel");
        }
    }

    #[test]
    fn enhanced_shader_validates_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("Enhanced WGSL requires offscreen GPU");
        for flags in 0..128 {
            let (shadows, ao, bloom, hdr) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Enhanced ambient/emissive/shadow syntax regression"),
                source: wgpu::ShaderSource::Wgsl(super::source(crate::EnhancedSettings {
                    sun_shadows: shadows, ambient_occlusion: ao, emissive_bloom: bloom, hdr,
                    material_specular: flags & 16 != 0, clustered_lighting: flags & 32 != 0,
                    local_light_shadows: flags & 64 != 0, ..Default::default()
                }).into()),
            });
        }
    }
}
