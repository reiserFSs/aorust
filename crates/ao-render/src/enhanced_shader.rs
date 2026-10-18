//! Enhanced-only shader generation; the faithful WGSL is never modified.

pub(super) fn source(settings: crate::EnhancedSettings) -> String {
    let sun_shadows = settings.sun_shadows || (settings.ground_grass && settings.grass_sun_shadows);
    let shadows = sun_shadows || settings.moon_shadows;
    let ao = settings.ambient_occlusion || settings.indirect_light;
    let bloom = settings.emissive_bloom;
    let hdr = settings.needs_sun_mask();
    let fog_only = settings.fog_only_mask();
    let compact_indirect = crate::enhanced::compact_indirect(&settings);
    let materials = settings.generated_normals || settings.material_maps || settings.material_pbr
        || settings.material_reflections || settings.environment_probe;
    let material_specular = settings.material_specular || settings.material_pbr;
    let reflections = settings.material_reflections || settings.environment_probe;
    let clustered_lighting = settings.clustered_lighting;
    let local_light_shadows = settings.local_light_shadows;
    let night = settings.night_mode != crate::NightMode::AsGame;
    // HDR and night receivers need the native directional shares independently.
    let sun_shares = shadows || settings.needs_full_sun_mask() || night;
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
    if fog_only {
        // Specialize the existing helper, retaining its native material gates and
        // ground_fog selection without evaluating directional or roof metadata.
        let start = source.find("    let share = dot(i.sun_share.xyz,").expect("sun share");
        let end = start + source[start..].find("\n}").expect("sun helper end");
        source.replace_range(start..end, "    return vec4<f32>(fog, 0.0, 0.0, beauty.a);");
    }
    if settings.cloud_shadows || reflections || (settings.indirect_light && !compact_indirect) {
        source = source.replace("    @location(3) sun: vec4<f32>,", "    @location(3) sun: vec4<f32>,\n    @location(4) water_cloud: vec4<f32>,");
    }
    if reflections || (settings.indirect_light && !compact_indirect) {
        source = source.replace("    @location(4) water_cloud: vec4<f32>,", "    @location(4) water_cloud: vec4<f32>,\n    @location(5) reflection_surface: vec4<f32>,\n    @location(6) reflection_mask: vec4<f32>,");
    }
    if compact_indirect {
        source = source.replace("@location(2) emission:", "@location(2) albedo:");
    } else if settings.indirect_light {
        source = source.replace("    @location(6) reflection_mask: vec4<f32>,", "    @location(6) reflection_mask: vec4<f32>,\n    @location(7) albedo: vec4<f32>,");
    }
    for name in names {
        let ambient = match name.as_str() {
            "fs_blend" if settings.indirect_light => "vec4<f32>(0.0, 0.0, 0.0, beauty.a)",
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
        let geometric_normal = if receiving { "    var surface = i;\n    surface.n = early_geometric_normal;\n" } else { "" };
        let fragment_input = if receiving { "surface" } else { "i" };
        let sun = match name.as_str() {
            _ if !hdr => "vec4<f32>(0.0)",
            name if name.starts_with("fs_sky_") => "vec4<f32>(0.0)",
            "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" => "enhanced_sun(i, beauty, false, 0.0)",
            "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" if fog_only => "enhanced_sun(i, beauty, true, 0.0)",
            "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" => "enhanced_sun(i, beauty, true, terrain_sky_open(i.wpos))",
            // ZERO/SRCCOLOR must extinguish only direct exposure under a roof;
            // sky eligibility, native fog and coverage belong to the base pass.
            "fs_ground_shadow" if fog_only => "vec4<f32>(1.0)",
            "fs_ground_shadow" if shadows => "vec4<f32>(enhanced_sun_visibility, 1.0, 1.0, 1.0)",
            "fs_ground_shadow" => "vec4<f32>(1.0)",
            _ => "vec4<f32>(0.0, 0.0, 0.0, beauty.a)",
        };
        let water = if settings.cloud_shadows {
            if name == "fs_ground_shadow" { ", vec4<f32>(1.0)".to_owned() }
            else if matches!(name.as_str(), "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend") { ", vec4<f32>(enhanced_water_cloud(i, beauty), 0.0, 0.0, beauty.a)".to_owned() }
            else { ", vec4<f32>(0.0, 0.0, 0.0, beauty.a)".to_owned() }
        } else if reflections || (settings.indirect_light && !compact_indirect) { ", vec4<f32>(0.0)".to_owned() } else { String::new() };
        let reflection = if reflections {
            let primary = matches!(name.as_str(), "fs_opaque" | "fs_test" | "fs_fade_opaque" | "fs_fade_test" | "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple");
            if primary {
                ", vec4<f32>(material_shading_normal(i), material_roughness(i)), vec4<f32>(material_reflectivity(i), native_fog(i.fog), material_static_source(i), select(0.0, 1.0, i.effect == 0u && (u32(mat.native.z) & 128u) == 0u))"
            } else { ", vec4<f32>(0.0), vec4<f32>(0.0)" }
        } else if settings.indirect_light && !compact_indirect { ", vec4<f32>(0.0), vec4<f32>(0.0)" } else { "" };
        let albedo_sample = if settings.indirect_light {
            match name.as_str() {
                "fs_opaque" | "fs_test" | "fs_fade_opaque" | "fs_fade_test" => "    let albedo_sample = enhanced_albedo(i, false);\n",
                "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" => "    let albedo_sample = enhanced_albedo(i, true);\n",
                _ => "    let albedo_sample = vec4<f32>(0.0);\n",
            }
        } else { "" };
        let albedo = if settings.indirect_light {
            if matches!(name.as_str(), "fs_opaque" | "fs_test" | "fs_fade_opaque" | "fs_fade_test" | "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple") {
                ", vec4<f32>(albedo_sample.rgb, beauty.a)"
            } else {
                // Alpha-over/additive targets suppress source RGB; native
                // multiplicative targets consume beauty only as a filter.
                ", vec4<f32>(beauty.rgb, beauty.a)"
            }
        } else { "" };
        let third = if compact_indirect { albedo.strip_prefix(", ").expect("compact albedo output") } else { emission_result.as_str() };
        let albedo = if compact_indirect { "" } else { albedo };
        source.push_str(&format!("\n@fragment fn {name}(i: VOut) -> EnhancedOutput {{\n{geometric_normal}    let envelope = vec4<f32>(1.0);\n    let emission_sample = {emission_sample};\n{albedo_sample}    let beauty = enhanced_original_{name}({fragment_input});\n    let bounded_beauty = clamp(beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    return EnhancedOutput(beauty, {ambient}, {third}, {sun}{water}{reflection}{albedo});\n}}\n"));
    }
    source = source.replace("sun_visibility(i.wpos)", "sun_visibility(i.wpos, i.n, i.clip.xy)");
    if shadows {
        append_shadow_receiver(&mut source, 0);
        if settings.needs_full_sun_mask() || material_specular {
            // HDR eligibility uses actual light visibility, not the artistic shade floor.
            // Capture before the final mix: beauty retains its native Enhanced shadow strength.
            source = source.replace(
                "    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
                "    sun_memo_wrote_raw = true;\n    enhanced_sun_visibility = visible;\n    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));",
            );
        }
        source = source.replace("fn sun_visibility(", "fn sun_visibility_uncached(");
        if settings.needs_full_sun_mask() || material_specular {
            source.push_str(SUN_VISIBILITY_MEMO);
        } else {
            // Native-only consumers have no raw HDR/GGX side channel.
            source.push_str(&SUN_VISIBILITY_MEMO.lines().filter(|line| {
                !line.contains("sun_memo_raw") && !line.contains("sun_memo_has_raw")
                    && !line.contains("sun_memo_wrote_raw")
            }).collect::<Vec<_>>().join("\n"));
        }
        source = source.replace("    let envelope = vec4<f32>(1.0);", "    sun_memo_valid = false;\n    let envelope = vec4<f32>(1.0);");
    } else {
        source.push_str("\nfn sun_visibility(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 { return 1.0; }\n");
    }
    // Remove disabled effects structurally, not merely by multiplying by zero.
    for (enabled, name) in [(ao, "enhanced_ao_receiver"), (ao, "enhanced_ground_receiver"), (settings.indirect_light, "enhanced_albedo"), (bloom, "enhanced_emission"), (hdr, "enhanced_sun"), (settings.cloud_shadows, "enhanced_water_cloud")] {
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
        source = source.replace("    let excess = max(native_raw - vec3<f32>(1.0), vec3<f32>(0.0));\n", "");
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
    if (crate::EnhancedSettings { grass_point_shadows: false, ..settings }).needs_admitted_lights() {
        crate::point_shadows::apply_shader(&mut source, local_light_shadows, clustered_lighting);
    }
    if settings.indirect_light {
        capture_indirect_diffuse(&mut source);
    }
    early_rejection(&mut source);
    source
}

// Keep all derivative-dependent work in the uniform prefix. In particular,
// mapped-normal variance differentiates sampled normals, so that preparation
// cannot move below discard; visibility queries can and must.
fn early_rejection(source: &mut String) {
    if source.contains("fn material_prepare(") {
        *source = source.replace("fn material_prepare(i: VOut) {", "fn material_prepare_derivatives(i: VOut) {");
        let start = source.find("    material_visibility_value = sun_visibility(").expect("material receiver");
        let end = source[start..].find("    material_albedo_value =").unwrap() + start;
        let receiver = source[start..end].replace("cross(dpdx(i.wpos), dpdy(i.wpos))", "early_geometric_normal");
        source.replace_range(start..end, "");
        source.push_str(&format!("\nfn material_prepare(i: VOut) {{\n{receiver}}}\n"));
        *source = source.replace("    material_prepare(i);", "    material_prepare_derivatives(i);\n    early_normal_dx = dpdx(material_normal_value / max(length(material_normal_value), 1e-6));\n    early_normal_dy = dpdy(material_normal_value / max(length(material_normal_value), 1e-6));\n    early_reject(i, early_mode);\n    material_prepare(i);");
    }
    // These are the only native entrypoint families with discard. Cache their
    // primary sample once, retaining the exact sampler, UV and implicit mip.
    for sample in [
        "sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z)",
        "sample_clamped(tex, sky_uv(i))",
        "sample_clamped(tex, i.uv)",
    ] {
        // Lava/crust are unrelated native operations; only change the helpers
        // and env entrypoint that actually participate in early rejection.
        for function in ["shade", "shade_sky", "ground_tile", "enhanced_original_fs_env"] {
            let marker = format!("fn {function}(");
            if let Some(start) = source.find(&marker) {
                let end = source[start..].find("\n}").unwrap() + start;
                let body = source[start..end].replace(sample, "early_primary");
                source.replace_range(start..end, &body);
            }
        }
    }
    *source = source.replace("sample_clamped(env_tex, i.env_uv)", "early_environment");
    *source = source.replace("sample_clamped(holo0, vec2<f32>(i.wpos.x, -i.wpos.z) * mat.wave.xz)", "early_palette");
    *source = source.replace("sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z)", "early_primary");
    *source = source.replace("sample_clamped(tex, material_uv(i))", "early_material_primary");
    for (function, cached) in [("enhanced_albedo", "early_primary"), ("material_prepare_derivatives", "early_material_primary")] {
        if let Some(start) = source.find(&format!("fn {function}(")) {
            let end = source[start..].find("\n}").unwrap() + start;
            let body = source[start..end].replace("sample_clamped(tex, uv)", cached);
            source.replace_range(start..end, &body);
        }
    }
    // Authored/clustered derivatives must also precede the divergent discard.
    *source = source.replace("max(length(fwidth(i.wpos)), 1e-6)", "early_footprint");
    *source = source.replace("    let dx = dpdx(n);\n    let dy = dpdy(n);", "    let dx = early_normal_dx;\n    let dy = early_normal_dy;");
    let names: Vec<String> = source.lines().filter_map(|line| line.strip_prefix("@fragment fn ").and_then(|s| s.split_once('(')).map(|(name, _)| name.to_owned())).collect();
    for name in names {
        let mode = match name.as_str() {
            "fs_opaque" => 0, "fs_test" => 1, "fs_blend" => 2,
            "fs_add" | "fs_mesh_add" => 3, "fs_fade_opaque" => 4,
            "fs_fade_test" => 5, "fs_sprite" => 6, "fs_add_one" => 9,
            "fs_sky_test" => 11, "fs_sky_opaque" | "fs_sky_blend" | "fs_sky_add" | "fs_sky_add_one" => 12,
            "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" => 13,
            "fs_env" => 14, _ => 15,
        };
        let uv = if (11..=12).contains(&mode) { "sky_uv(i)" } else if mode == 13 || mode == 14 { "i.uv" } else { "i.uv + mat.scroll.xy * g.fog.z" };
        let material_sample = if source.contains("fn material_prepare_derivatives(") {
            if mode <= 9 || mode == 15 { "    early_material_primary = early_primary;\n" }
            else { "    early_material_primary = sample_clamped(tex, material_uv(i));\n" }
        } else { "" };
        let environment_sample = if mode <= 9 { "    if mat.native.x > 0.5 { early_environment = sample_clamped(env_tex, i.env_uv); }\n" } else { "" };
        let palette_sample = if name.starts_with("fs_ground_") { "    early_palette = sample_clamped(holo0, vec2<f32>(i.wpos.x, -i.wpos.z) * mat.wave.xz);\n" } else { "" };
        let prefix = format!("\n    let early_mode = {mode}u;\n    early_primary = sample_clamped(tex, {uv});\n{material_sample}{environment_sample}{palette_sample}    early_geometric_normal = cross(dpdx(i.wpos), dpdy(i.wpos));\n    early_footprint = max(length(fwidth(i.wpos)), 1e-6);\n    let early_n = i.n / max(length(i.n), 1e-6);\n    early_normal_dx = dpdx(early_n);\n    early_normal_dy = dpdy(early_n);\n");
        let marker = format!("@fragment fn {name}(i: VOut) -> EnhancedOutput {{");
        *source = source.replace(&marker, &format!("{marker}{prefix}"));
        if !source.contains("fn material_prepare_derivatives(") {
            let start = source.find(&marker).unwrap();
            let end = source[start..].find("\n}").unwrap() + start;
            let body = source[start..end].replacen("    let envelope =", "    early_reject(i, early_mode);\n    let envelope =", 1);
            source.replace_range(start..end, &body);
        }
        if mode == 12 || mode == 15 {
            let start = source.find(&marker).unwrap();
            let end = source[start..].find("\n}").unwrap() + start;
            let body = source[start..end].replace("    early_reject(i, early_mode);\n", "");
            source.replace_range(start..end, &body);
        }
    }
    source.push_str(EARLY_REJECTION);
}

const EARLY_REJECTION: &str = r#"
var<private> early_primary: vec4<f32>;
var<private> early_material_primary: vec4<f32>;
var<private> early_environment: vec4<f32>;
var<private> early_palette: vec4<f32>;
var<private> early_geometric_normal: vec3<f32>;
var<private> early_footprint: f32;
var<private> early_normal_dx: vec3<f32>;
var<private> early_normal_dy: vec3<f32>;
fn early_reject(i: VOut, mode: u32) {
    if mode <= 9u {
        let material_alpha = select(mat.color.a, i.fade, i.fade < 1.0);
        let texture_alpha = select(early_primary.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5);
        let alpha = texture_alpha * i.color.a * material_alpha;
        if native_alpha_test() && !native_alpha_pass(alpha) { discard; }
        if (mode == 1u || mode == 5u) && !native_alpha_test() {
            if g.fog.w <= 0.5 && alpha < 0.5 { discard; }
        }
        // Only fs_test enables A2C; frame-faded tests remain native blending.
        if mode == 1u && g.fog.w > 0.5 && alpha <= 0.0 { discard; }
    } else if mode == 11u {
        let alpha = early_primary.a * i.color.a * mat.color.a;
        if g.fog.w <= 0.5 && alpha < 0.5 { discard; }
        if g.fog.w > 0.5 && alpha <= 0.0 { discard; }
    } else if mode == 13u {
        if early_primary.a <= 128.0 / 255.0 { discard; }
    } else if mode == 14u {
        let alpha = early_primary.a * i.color.a * select(mat.color.a, i.fade, i.fade < 1.0);
        if native_alpha_test() && !native_alpha_pass(alpha) { discard; }
    }
}
"#;

// ponytail: one entry covers adjacent repeated receivers, not arbitrary history;
// a different normal evicts it rather than risking a changed receiver-plane bias.
const SUN_VISIBILITY_MEMO: &str = r#"
var<private> sun_memo_valid: bool = false;
var<private> sun_memo_key0: vec4<u32>;
var<private> sun_memo_key1: vec4<u32>;
var<private> sun_memo_value: f32;
var<private> sun_memo_raw: f32;
var<private> sun_memo_wrote_raw: bool = false;
var<private> sun_memo_has_raw: bool;
fn sun_visibility(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 {
    let key0 = bitcast<vec4<u32>>(vec4<f32>(world, pixel.x));
    let key1 = bitcast<vec4<u32>>(vec4<f32>(normal, pixel.y));
    if sun_memo_valid && all(key0 == sun_memo_key0) && all(key1 == sun_memo_key1) {
        if sun_memo_has_raw { enhanced_sun_visibility = sun_memo_raw; }
        return sun_memo_value;
    }
    sun_memo_wrote_raw = false;
    sun_memo_value = sun_visibility_uncached(world, normal, pixel);
    sun_memo_has_raw = sun_memo_wrote_raw;
    sun_memo_raw = enhanced_sun_visibility;
    sun_memo_key0 = key0;
    sun_memo_key1 = key1;
    sun_memo_valid = true;
    return sun_memo_value;
}
"#;

/// Capture the actual diffuse term before environment/specular/emission additions.
/// Beauty expressions and the AO-only generated shader remain untouched.
fn capture_indirect_diffuse(source: &mut String) {
    *source = source.replace("    let envelope = vec4<f32>(1.0);", "    enhanced_diffuse = vec3<f32>(0.0);\n    let envelope = vec4<f32>(1.0);");
    let diffuse = source.find("    var texture_color = min(t.rgb").expect("fragment diffuse calculation");
    let diffuse_end = diffuse + source[diffuse..].find('\n').expect("fragment diffuse line end");
    source.insert_str(diffuse_end, "\n    enhanced_diffuse = texture_color;");
    *source = source.replace("    let authored_fog = ", "    enhanced_diffuse *= 1.0 - f;\n    let authored_fog = ");
    *source = source.lines().map(|line| {
        if line.trim_start().starts_with("let diffuse = max(beauty.rgb - ") {
            "    let diffuse = max(enhanced_diffuse, vec3<f32>(0.0));"
        } else { line }
    }).collect::<Vec<_>>().join("\n");
    *source = source.replace(
        "if multiplier { return vec4<f32>(clamp(beauty.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), beauty.a); }",
        "if multiplier { return vec4<f32>(clamp(enhanced_diffuse, vec3<f32>(0.0), vec3<f32>(1.0)), beauty.a); }",
    );
    if source.contains("fn material_ground(") {
        *source = source.replace(
            "    let diffuse = material_diffuse(i, light);",
            "    let diffuse = material_diffuse(i, light);\n    enhanced_diffuse = select(mix(diffuse, vec3<f32>(1.0), ground_fog(i)), base * diffuse * (1.0 - ground_fog(i)), fused);",
        );
    }
    for name in ["fused", "base", "shadow", "simple"] {
        let start = source.find(&format!("fn enhanced_original_fs_ground_{name}(")).expect("ground diffuse capture");
        let end = start + source[start..].find("\n}").expect("ground fragment end");
        if source[start..end].contains("material_ground(") { continue; }
        // Wrap the already evaluated diffuse expression, without another texture sample.
        let argument = start + source[start..end].find("mix(").expect("ground native fog") + 4;
        let mut depth = 0;
        let length = source[argument..end].char_indices().find_map(|(offset, ch)| {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => return Some(offset),
                _ => {}
            }
            None
        }).expect("ground fog first argument");
        let value = source[argument..argument + length].to_owned();
        source.replace_range(argument..argument + length,
            &format!("enhanced_capture_ground({value}, ground_fog(i), {})", name == "shadow"));
    }
    source.push_str("\nvar<private> enhanced_diffuse: vec3<f32>;\nfn enhanced_capture_ground(value: vec3<f32>, fog: f32, multiplier: bool) -> vec3<f32> {\n    enhanced_diffuse = select(value * (1.0 - fog), mix(value, vec3<f32>(1.0), fog), multiplier);\n    return value;\n}\n");
}

/// Reuse the world receiver verbatim; grass packs it beside its cell buffers.
pub(super) fn append_shadow_receiver(source: &mut String, binding_offset: u32) {
    let shader = crate::shader_source::load("sun_shadows.wgsl", include_str!("sun_shadows.wgsl"));
    let declarations = shader.split("@group(3) @binding(3)").next().expect("shadow receiver declarations");
    for line in declarations.lines() {
        let mut line = line.to_owned();
        for binding in [0, 1, 2, 8] {
            let old = format!("@group(3) @binding({binding})");
            if line.contains(&old) {
                let mapped = if binding == 8 { if binding_offset == 2 { 16 } else { 8 } } else { binding + binding_offset };
                line = line.replace(&old, &format!("@group(3) @binding({mapped})"));
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
    fn early_alpha_rejection_precedes_every_receiver_and_preserves_native_source() {
        let faithful = include_str!("shader.wgsl");
        for pbr in [false, true] {
            let source = super::source(crate::EnhancedSettings {
                sun_shadows: true, moon_shadows: true, generated_normals: true,
                material_maps: true, material_pbr: pbr, material_specular: true,
                material_reflections: true, clustered_lighting: true,
                local_light_shadows: true, indirect_light: true, emissive_bloom: true,
                hdr: true, ..Default::default()
            });
            for wrapper in source.split("@fragment fn ").skip(1) {
                let wrapper = wrapper.split("\n}").next().unwrap();
                let prepare = wrapper.find("material_prepare(i);").unwrap();
                if let Some(reject) = wrapper.find("early_reject(i, early_mode);") {
                    assert!(wrapper.find("early_geometric_normal = cross(dpdx(i.wpos), dpdy(i.wpos));").unwrap() < reject);
                    assert!(wrapper.find("material_prepare_derivatives(i);").unwrap() < reject);
                    assert!(reject < prepare);
                    assert!(reject < wrapper.find("let beauty =").unwrap());
                }
            }
            let derivatives = source.split("fn material_prepare_derivatives(").nth(1).unwrap().split("\n}").next().unwrap();
            assert!(!derivatives.contains("sun_visibility("));
            assert!(derivatives.contains("dpdx(material_normal_value)"));
            assert!(source.contains("early_primary = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z)"));
            assert!(source.contains("early_environment = sample_clamped(env_tex, i.env_uv)"));
        }
        assert_eq!(crate::shader_source::load("shader.wgsl", faithful).as_ref(), faithful);
        assert!(!faithful.contains("early_reject"));
        assert!(faithful.find("let lit = min(texture_color + spec,").unwrap() < faithful.find("if native_alpha_test() && !native_alpha_pass(alpha)").unwrap());
    }

    #[test]
    fn dense_cutout_early_alpha_matches_native_comparisons_and_frame_fade_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("early-alpha coverage GPU");
        let faithful = include_str!("shader.wgsl");
        let function = |name: &str| {
            let start = faithful.find(&format!("fn {name}(")).unwrap();
            &faithful[start..start + faithful[start..].find("\n}").unwrap() + 2]
        };
        for samples in [1, 4] { for mode in [0, 1, 2, 5, 6, 11, 13, 14] {
            let source = format!(r#"
struct VOut {{ color: vec4<f32>, fade: f32 }}
struct Material {{ color: vec4<f32>, emissive: vec4<f32>, native: vec4<f32> }}
struct Globals {{ fog: vec4<f32> }}
var<private> mat: Material;
var<private> g: Globals;
fn native_alpha_test() -> bool {{ return (u32(mat.native.z) & 1u) != 0u; }}
{}
{}
@vertex fn vs(@builtin(vertex_index) n: u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(vec2<f32>(f32((n << 1u) & 2u), f32(n & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}}
fn pixel(p: vec4<f32>, early: bool) -> vec4<f32> {{
    let col = u32(p.x);
    let row = u32(p.y);
    // Every native compare, disabled ALPHATEST, material/frame replacement and
    // Mode5 texture-alpha-as-light, including exact 128/255 terrain threshold.
    mat.native = vec4<f32>(0.0, 128.0 / 255.0, select(1.0, 0.0, col / 8u == 0u), f32(col / 8u));
    mat.color = vec4<f32>(1.0, 1.0, 1.0, select(0.75, 1.0, row < 4u));
    mat.emissive.w = select(0.0, 1.0, row == 6u);
    g.fog.w = {};
    let a = array<f32, 8>(0.0, 1.0 / 255.0, 0.25, 0.49, 0.5, 128.0 / 255.0, 0.75, 1.0)[col % 8u];
    early_primary = vec4<f32>(1.0, 1.0, 1.0, a);
    let i = VOut(vec4<f32>(1.0), select(1.0, 0.5, row == 5u));
    if early {{ early_reject(i, {}u); }}
    let alpha = select(a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5) * i.color.a * select(mat.color.a, i.fade, i.fade < 1.0);
    if {}u <= 9u {{
        if native_alpha_test() && !native_alpha_pass(alpha) {{ discard; }}
        if ({}u == 1u || {}u == 5u) && !native_alpha_test() && g.fog.w <= 0.5 && alpha < 0.5 {{ discard; }}
    }} else if {}u == 11u {{
        if g.fog.w <= 0.5 && a * mat.color.a < 0.5 {{ discard; }}
    }} else if {}u == 13u {{
        if a <= 128.0 / 255.0 {{ discard; }}
    }} else {{
        if native_alpha_test() && !native_alpha_pass(a * select(mat.color.a, i.fade, i.fade < 1.0)) {{ discard; }}
    }}
    return vec4<f32>(0.75, 0.5, 0.25, select(alpha, a * mat.color.a, {}u == 11u));
}}
@fragment fn reference(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{ return pixel(p, false); }}
@fragment fn optimized(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{ return pixel(p, true); }}
"#, function("native_alpha_pass"), super::EARLY_REJECTION, if samples > 1 { "1.0" } else { "0.0" }, mode, mode, mode, mode, mode, mode, mode);
            let source = if mode == 2 || mode == 6 {
                source.replace("if native_alpha_test() && !native_alpha_pass(alpha) {",
                    "if native_alpha_test() && select(!native_alpha_pass(alpha), alpha <= 0.0, u32(mat.native.w) == 5u || u32(mat.native.w) == 7u) {")
            } else { source };
            let module = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("dense native alpha coverage"), source: wgpu::ShaderSource::Wgsl(source.into()) });
            let mut captures = Vec::new();
            for entry in ["reference", "optimized"] {
                let pipeline = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: None, layout: None,
                    vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
                    fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(entry), targets: &[Some(wgpu::ColorTargetState { format: r.format, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
                    primitive: Default::default(), depth_stencil: None,
                    multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: samples > 1 && (mode == 1 || mode == 11) },
                    multiview_mask: None, cache: None,
                });
                let texture = |count| r.device.create_texture(&wgpu::TextureDescriptor {
                    label: None, size: wgpu::Extent3d { width: 72, height: 8, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: count,
                    dimension: wgpu::TextureDimension::D2, format: r.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | if count == 1 { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() }, view_formats: &[],
                });
                let output = texture(1);
                let view = output.create_view(&Default::default());
                let msaa = (samples > 1).then(|| texture(samples));
                let msaa_view = msaa.as_ref().map(|t| t.create_view(&Default::default()));
                let mut encoder = r.device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: msaa_view.as_ref().unwrap_or(&view), resolve_target: msaa_view.as_ref().map(|_| &view), depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })], ..Default::default()
                    });
                    pass.set_pipeline(&pipeline);
                    pass.draw(0..3, 0..128);
                }
                r.queue.submit([encoder.finish()]);
                captures.push(crate::texture_to_rgba(&r, &output, 72, 8).unwrap());
            }
            assert_eq!(captures[0], captures[1], "{samples}x mode {mode}: native comparisons/frame fade/CAT coverage");
            assert!(captures[0].iter().any(|&v| v != 0), "coverage positive control");
            if mode == 2 || mode == 6 {
                for compare in [5, 7] {
                    let pixel = (compare * 8 + 2) * 4;
                    assert!(captures[1][pixel] > 0 && captures[1][pixel + 3] > 0,
                        "{samples}x: positive 0.25 native soft-edge alpha must survive GT/GE root rejection");
                }
            }
        }}
    }
    #[test]
    fn pbr_shadow_raw_reuse_and_exact_memo_side_effects_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("PBR raw visibility regression GPU");
        for pbr in [false, true] { for moon in [false, true] {
            let combined = super::source(crate::EnhancedSettings {
                material_pbr: pbr, material_specular: true, sun_shadows: true,
                moon_shadows: true, hdr: true, material_reflections: true,
                environment_probe: true, clustered_lighting: true,
                night_mode: if moon { crate::NightMode::Moonlit } else { crate::NightMode::AsGame },
                ..Default::default()
            });
            let mut combined = if moon { crate::moon_shadows::shader(combined, true, true) } else { combined };
            crate::foliage::append_shader(&mut combined);
            crate::foliage_detail::append_shader(&mut combined, true, true);
            let _shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("combined PBR/specular HDR probe clustered foliage moon visibility"),
                source: wgpu::ShaderSource::Wgsl(combined.into()),
            });
        } }
        let generated = super::source(crate::EnhancedSettings {
            material_pbr: true, material_specular: true, sun_shadows: true, hdr: true,
            ..Default::default()
        });
        let ggx = generated[generated.find("fn authored_specular(").unwrap()..].split("\n}").next().unwrap();
        let math = include_str!("authored_specular.wgsl").split("fn authored_specular(").next().unwrap();
        // Extend the existing numeric memo probe with the generated caller chain,
        // not a second scene fixture. The instrumented uncached body counts misses;
        // fog/admission image regressions continue to cover actual shadow maps.
        let mut receiver_source = crate::moon_shadows::shader(generated.clone(), true, true);
        crate::foliage_detail::append_shader(&mut receiver_source, true, false);
        let function = |name: &str| {
            let start = receiver_source.find(&format!("fn {name}(")).unwrap();
            let end = start + receiver_source[start..].find("\n}").unwrap() + 2;
            receiver_source[start..end].to_owned()
        };
        let prepare = function("material_prepare");
        let no_provenance = prepare.replace("fn material_prepare(", "fn material_prepare_without_provenance(")
            .replace("MATERIAL_STATIC_PROVENANCE", "false");
        let sunlight = function("authored_sun_visibility");
        let moon_visibility = function("authored_moon_visibility");
        let transmission = function("foliage_detail_transmission");
        let source = format!(r#"
struct VOut {{ wpos: vec3<f32>, n: vec3<f32>, clip: vec4<f32>, tint: vec4<f32>, effect: u32 }}
struct Globals {{ eye: vec4<f32>, sun_dir: vec4<f32>, sun_g: vec4<f32> }}
struct Material {{ emissive: vec4<f32>, native: vec4<f32> }}
struct ShadowFrame {{ moon_direction: vec4<f32> }}
var<private> g: Globals;
var<private> mat: Material;
var<private> shadow_frame: ShadowFrame;
var<private> enhanced_sun_visibility: f32 = 1.0;
var<private> calls: u32 = 0u;
const MATERIAL_STATIC_PROVENANCE: bool = true;
var<private> early_geometric_normal: vec3<f32>;
var<private> material_visibility_value: f32;
var<private> material_static_visibility_value: f32;
var<private> material_footprint: f32 = 1.0;
var<private> material_variance: f32 = 0.0;
var<private> material_albedo_value: vec3<f32> = vec3<f32>(0.5);
fn native_unlit() -> bool {{ return false; }}
fn material_alpha(i: VOut) -> f32 {{ return 0.5; }}
fn material_metallic(i: VOut) -> f32 {{ return 0.0; }}
fn material_ambient_specular(i: VOut, n: vec3<f32>, v: vec3<f32>, f0: vec3<f32>) -> vec3<f32> {{ return vec3<f32>(0.0); }}
fn authored_local_spec(p: vec3<f32>, n: vec3<f32>, a: f32, f: vec3<f32>) -> array<vec3<f32>, 2> {{
    return array<vec3<f32>, 2>(vec3<f32>(0.0), vec3<f32>(0.0));
}}
fn sun_visibility_uncached(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 {{
    calls += 1u;
    if world.x < 0.0 {{ return 1.0; }}
    sun_memo_wrote_raw = true;
    enhanced_sun_visibility = select(0.0, 1.0, world.x > 1.0);
    return mix(1.0, enhanced_sun_visibility, 0.6);
}}
{memo}
{math}
{ggx}
}}
{prepare}
{no_provenance}
{sunlight}
{moon_visibility}
{transmission}
@vertex fn vs(@builtin(vertex_index) n: u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(vec2<f32>(f32((n << 1u) & 2u), f32(n & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    sun_memo_valid = false;
    let world = vec3<f32>(p.x, 0.0, 0.0);
    let normal = vec3<f32>(0.0, 0.6, 0.8);
    g = Globals(vec4<f32>(world + normal, 1.0), vec4<f32>(normal, 0.0), vec4<f32>(1.0));
    authored_normal = normal;
    mat.native.z = 64.0;
    // The raw facet differs from both the artistic normal and its normalized/
    // flipped counterpart, so either old caller would force a second miss.
    early_geometric_normal = normal * 3.0;
    var chain_ok = true;
    for (var mode = 0u; mode < 4u; mode++) {{
        calls = 0u;
        sun_memo_valid = false;
        enhanced_sun_visibility = 1.0;
        shadow_frame.moon_direction.w = f32(mode / 2u);
        let artist = VOut(world, vec3<f32>(1.0, 0.0, 0.0), p, vec4<f32>(32.0), 0u);
        if mode % 2u == 0u {{ material_prepare_without_provenance(artist); }}
        else {{ material_prepare(artist); }}
        chain_ok = chain_ok && calls == select(1u, 0u, mode == 2u);
        var surface = artist;
        surface.n = early_geometric_normal;
        let transfer = foliage_detail_transmission(world, surface.n, p.xy);
        let beauty_visibility = authored_sun_visibility(world, surface.n, p.xy);
        let moon_visibility_value = authored_moon_visibility(surface);
        chain_ok = chain_ok && calls == 1u && all(transfer >= vec3<f32>(0.0));
        chain_ok = chain_ok && select(material_visibility_value == beauty_visibility,
            material_visibility_value == 1.0, mode >= 2u);
        if mode % 2u != 0u {{
            chain_ok = chain_ok && material_static_visibility_value == select(beauty_visibility, moon_visibility_value, mode >= 2u);
        }}
        // Sunlight's moon-neutral reset must not destroy raw visibility on a hit.
        chain_ok = chain_ok && enhanced_sun_visibility == select(0.0, 1.0, world.x > 1.0);
    }}
    shadow_frame.moon_direction.w = 0.0;
    sun_memo_valid = false;
    calls = 0u;
    let floor_value = sun_visibility(world, normal, p.xy);
    let first_raw = enhanced_sun_visibility;
    let i = VOut(world, normal, p, vec4<f32>(32.0), 0u);
    let highlight = authored_specular(i, vec3<f32>(0.0));
    var ok = chain_ok && calls == 1u && enhanced_sun_visibility == first_raw;
    ok = ok && select(all(highlight == vec3<f32>(0.0)) && floor_value > 0.0,
        any(highlight > vec3<f32>(0.0)), p.x > 1.0);
    let repeated = sun_visibility(world, normal, p.xy);
    ok = ok && bitcast<u32>(repeated) == bitcast<u32>(floor_value);
    // Distinct foliage-normal scales, positions, signed zero and both pixel
    // components miss even when their resulting visibility is identical.
    let a = sun_visibility(world, normal * 2.0, p.xy);
    let b = sun_visibility(world + vec3<f32>(0.0, 0.0, 1.0), normal * 2.0, p.xy);
    let c = sun_visibility(world, normal, vec2<f32>(0.0, p.y));
    let d = sun_visibility(world, normal, vec2<f32>(bitcast<f32>(0x80000000u), p.y));
    let e = sun_visibility(world, normal, vec2<f32>(bitcast<f32>(0x80000000u), p.y + 1.0));
    ok = ok && calls == 6u;
    // No-write early returns preserve the current caller state on a hit,
    // rather than replaying the state preceding the original query.
    let early = sun_visibility(vec3<f32>(-1.0), normal, p.xy);
    enhanced_sun_visibility = 0.375;
    let early_hit = sun_visibility(vec3<f32>(-1.0), normal, p.xy);
    ok = ok && calls == 7u && early == early_hit && enhanced_sun_visibility == 0.375;
    sun_memo_valid = false;
    let reset = sun_visibility(vec3<f32>(-1.0), normal, p.xy);
    ok = ok && calls == 8u && reset == 1.0;
    return vec4<f32>(vec3<f32>(select(0.0, 1.0, ok)), 1.0);
}}
"#, memo = super::SUN_VISIBILITY_MEMO);
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("generated PBR raw visibility and exact receiver memo"), source: wgpu::ShaderSource::Wgsl(source.into()),
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
            label: None, size: wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut encoder = r.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
        assert_eq!(crate::texture_to_rgba(&r, &texture, 2, 1).unwrap(), vec![255; 8],
            "shadowed GGX is zero despite beauty's floor; lit GGX is positive; exact hits restore raw writes only");
    }

    #[test]
    fn exact_sun_receiver_reuse_preserves_combined_paths_and_moon() {
        for pbr in [false, true] { for clustered in [false, true] { for moon in [false, true] {
            let settings = crate::EnhancedSettings {
                sun_shadows: true, moon_shadows: true, hdr: true,
                material_specular: true, material_pbr: pbr, clustered_lighting: clustered,
                material_reflections: true, environment_probe: true, ..Default::default()
            };
            let source = super::source(settings);
            let mut expected = String::new();
            super::append_shadow_receiver(&mut expected, 0);
            let expected = expected.split("fn sun_visibility(").nth(1).unwrap();
            let actual = source.split("fn sun_visibility_uncached(").nth(1).unwrap().split("\n}").next().unwrap();
            assert_eq!(actual.replace("    sun_memo_wrote_raw = true;\n    enhanced_sun_visibility = visible;\n", ""),
                expected.split("\n}").next().unwrap(), "fog, cascade, admission and grass receiver math stays verbatim");
            let source = if moon { crate::moon_shadows::shader(source, true, true) } else { source };
            let prepare = source.split("fn material_prepare(").nth(1).unwrap().split("\n}").next().unwrap();
            let sunlight = if moon { "authored_sun_visibility" } else { "sun_visibility" };
            assert!(prepare.contains(&format!("material_visibility_value = {sunlight}(i.wpos, early_geometric_normal, i.clip.xy);")));
            assert!(prepare.contains("material_static_visibility_value = material_visibility_value;"));
            assert!(!prepare.contains("i.n") && !prepare.contains("min("),
                "material visibility and provenance share only the facet receiver");
            assert_eq!(prepare.contains("if MATERIAL_STATIC_PROVENANCE {"), moon,
                "only moon provenance needs a separately guarded raw query");
            if moon {
                assert!(prepare.contains("if MATERIAL_STATIC_PROVENANCE { material_static_visibility_value = sun_visibility(i.wpos, early_geometric_normal, i.clip.xy); }"));
            }
            assert!(!prepare.contains("dpdx(") && !prepare.contains("dpdy("),
                "receiver preparation after discard must use the hoisted geometric normal");
            let ggx = source.split("fn authored_specular(").nth(1).unwrap().split("\n}").next().unwrap();
            assert!(ggx.contains("var visibility = "));
            assert!(ggx.contains("sun_visibility(i.wpos, i.n, i.clip.xy)"));
            assert!(!ggx.contains("var visibility = material_visibility_value"));
            assert!(source.contains("share * enhanced_sun_visibility * (1.0 - fog)"));
            for wrapper in source.split("@fragment fn ").skip(1) {
                let wrapper = wrapper.split("\n}").next().unwrap();
                assert!(wrapper.find("sun_memo_valid = false;").unwrap() < wrapper.find("material_prepare(i);").unwrap());
                if wrapper.contains("var surface = i;") {
                    let hoisted = wrapper.find("early_geometric_normal = cross(dpdx(i.wpos), dpdy(i.wpos));").unwrap();
                    let normal = wrapper.find("surface.n = early_geometric_normal;").unwrap();
                    assert!(hoisted < normal && normal < wrapper.find("material_prepare(i);").unwrap());
                    assert!(wrapper.contains("(surface);") || wrapper.contains("authored_moon_receiver(surface, i.n,"),
                        "beauty or its moon receiver must consume the exact geometric normal retained by material_prepare");
                }
            }
            if clustered {
                assert!(source.contains("fn clustered_receiver("));
                assert!(source.contains("fn clustered_receiver_spec("));
            }
            if moon {
                assert!(source.contains("return sun_visibility(world, normal, pixel);"));
                assert!(source.contains("sun_memo_value = sun_visibility_uncached(world, normal, pixel);"),
                    "moon renaming must not redirect the memo miss back into itself");
            }
        } } }
        let mut foliage = super::source(crate::EnhancedSettings { sun_shadows: true, ..Default::default() });
        crate::foliage_detail::append_shader(&mut foliage, true, false);
        assert!(foliage.contains("sun_visibility(world, normal, pixel)"));
        assert!(foliage.contains("bitcast<vec4<u32>>(vec4<f32>(normal, pixel.y))"));
        assert!(!super::source(crate::EnhancedSettings::default()).contains("sun_memo_"));
    }
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
    fn indirect_receiver_blends_track_native_transmittance_once() {
        let over = wgpu::BlendState::ALPHA_BLENDING;
        let target = crate::albedo_target(Some(over), false, "fs_blend");
        assert_eq!(target.format, crate::enhanced::ALBEDO);
        let blend = target.blend.unwrap();
        assert_eq!(blend.color.src_factor, wgpu::BlendFactor::Zero);
        assert_eq!(blend.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(blend.alpha, blend.color, "overlay extinction affects RGB and eligibility together");
        let additive = wgpu::BlendState { color: wgpu::BlendComponent { dst_factor: wgpu::BlendFactor::One, ..over.color }, alpha: over.alpha };
        assert_eq!(crate::albedo_target(Some(additive), false, "fs_add").blend.unwrap().color.dst_factor, wgpu::BlendFactor::One);
        for native in [ao_scene::Blend::ZeroSourceColor, ao_scene::Blend::DestinationColorSourceColor] {
            let state = crate::native_blend_state(native);
            assert_eq!(crate::albedo_target(Some(state), false, "fs_blend").blend.unwrap().color, state.color,
                "multiplicative overlays filter existing receiver RGB with the actual native colour equation");
        }
        let fade = crate::albedo_target(Some(over), true, "fs_fade_opaque").blend.unwrap();
        assert_eq!(fade.color, over.color, "native fade alpha applies once through RGB blending");
        assert_eq!(fade.alpha, wgpu::BlendComponent::REPLACE);
        let excluded = crate::albedo_target(Some(over), true, "fs_sprite").blend.unwrap();
        assert_eq!(excluded.alpha.src_factor, wgpu::BlendFactor::Zero);
        assert_eq!(excluded.alpha.dst_factor, wgpu::BlendFactor::Zero);
        assert_eq!(excluded.color, excluded.alpha, "excluded depth owners must not leak beauty RGB through partial MSAA coverage");
        assert!(crate::albedo_target(Some(over), false, "fs_ground_shadow").write_mask.is_empty());
    }

    #[test]
    fn indirect_only_reuses_emission_slot_and_compacts_bounded_source() {
        for ao in [false, true] {
            let settings = crate::EnhancedSettings { indirect_light: true, ambient_occlusion: ao, ..Default::default() };
            assert!(crate::enhanced::compact_indirect(&settings));
            assert_eq!(crate::enhanced::target_count(&settings), 3);
            assert_eq!(crate::enhanced::ambient_format(&settings), if ao { crate::enhanced::AUX } else { crate::enhanced::ALBEDO });
            let source = super::source(settings);
            assert!(source.contains("@location(2) albedo: vec4<f32>"));
            assert!(!source.contains("@location(2) emission:"));
            for (location, field) in [(4, "water_cloud"), (5, "reflection_surface"), (6, "reflection_mask"), (7, "albedo")] {
                assert!(!source.contains(&format!("@location({location}) {field}:")));
            }
            assert!(source.contains("return EnhancedOutput(beauty, enhanced_ao_receiver(i, beauty), vec4<f32>(albedo_sample.rgb, beauty.a),"));
            let grass = crate::grass::shader_source((ao, false, false, false, false, true), false);
            assert!(grass.contains("@location(1) ambient:"));
            assert!(grass.contains("@location(2) albedo:"));
            assert!(!grass.contains("@location(7) albedo:"));
            assert!(!grass.contains("@location(4) water_cloud:"));
            assert_eq!(grass.contains("fn ao_share("), ao, "SSGI-only excluded grass does not compute an unused AO receiver");
        }
        for settings in [
            crate::EnhancedSettings { indirect_light: true, emissive_bloom: true, ..Default::default() },
            crate::EnhancedSettings { indirect_light: true, hdr: true, ..Default::default() },
            crate::EnhancedSettings { indirect_light: true, cloud_shadows: true, ..Default::default() },
            crate::EnhancedSettings { indirect_light: true, material_reflections: true, ..Default::default() },
        ] {
            assert!(!crate::enhanced::compact_indirect(&settings));
            assert_eq!(crate::enhanced::target_count(&settings), 8);
        }
    }

    #[test]
    fn post_only_mask_preserves_native_fog_without_directional_work() {
        use crate::EnhancedSettings;
        let defaults = EnhancedSettings::default();
        assert!(!defaults.fog_only_mask() && !defaults.needs_full_sun_mask());
        for settings in [
            EnhancedSettings { depth_of_field: true, ..defaults },
            EnhancedSettings { taa: true, ..defaults },
            EnhancedSettings { fxaa: true, ..defaults },
            EnhancedSettings { cas: true, ..defaults },
            EnhancedSettings { motion_blur: 1.0, ..defaults },
        ] {
            assert!(settings.fog_only_mask() && settings.needs_sun_mask() && !settings.needs_full_sun_mask());
            assert_eq!(crate::enhanced::sun_format(&settings), wgpu::TextureFormat::R16Float);
            assert_eq!(crate::enhanced::target_count(&settings), 4);
            let full = EnhancedSettings { hdr: true, ..settings };
            assert!(!full.fog_only_mask() && full.needs_full_sun_mask());
            assert_eq!(crate::enhanced::sun_format(&full), crate::enhanced::AUX);
            assert_ne!(crate::variant::Key::of(settings, 4), crate::variant::Key::of(full, 4));
            let source = super::source(settings);
            let helper = source.split("fn enhanced_sun(").nth(1).unwrap().split("\n}").next().unwrap();
            let full_source = super::source(full);
            let full_helper = full_source.split("fn enhanced_sun(").nth(1).unwrap().split("\n}").next().unwrap();
            assert_eq!(helper.split("    return vec4<f32>(fog,").next().unwrap(),
                full_helper.split("    let share =").next().unwrap());
            assert!(helper.contains("select(native_fog(i.fog), ground_fog(i), ground)"));
            assert!(helper.contains("return vec4<f32>(fog, 0.0, 0.0, beauty.a);"));
            assert!(!source.contains("sun_share:") && !source.contains("sun_spec_share:"));
            assert!(!source.contains("terrain_sky_open(i.wpos)"));
            let multiplier = source.split("@fragment fn fs_ground_shadow(").nth(1).unwrap().split("\n}").next().unwrap();
            assert!(multiplier.contains(", vec4<f32>(1.0));"));
            assert!(full_helper.contains("sky_open, fog, beauty.a"));
            let indirect = EnhancedSettings { indirect_light: true, ..settings };
            assert!(indirect.fog_only_mask());
            assert_eq!(crate::enhanced::target_count(&indirect), 8);
            let indirect_source = super::source(indirect);
            assert!(indirect_source.contains("@location(7) albedo:"));
            assert!(indirect_source.contains("enhanced_diffuse *= 1.0 - f;"));
        }
        for settings in [
            EnhancedSettings { depth_of_field: true, sun_shadows: true, ..defaults },
            EnhancedSettings { depth_of_field: true, night_mode: crate::NightMode::Moonlit, ..defaults },
        ] {
            assert!(settings.fog_only_mask());
            assert!(super::source(settings).contains("sun_share:"));
        }
    }

    #[test]
    fn indirect_albedo_is_primary_textured_and_keeps_sparse_mrt_shape() {
        let settings = crate::EnhancedSettings { indirect_light: true, emissive_bloom: true, ..Default::default() };
        let source = super::source(settings);
        assert_eq!(crate::enhanced::target_count(&settings), 8);
        for (location, field) in [(4, "water_cloud"), (5, "reflection_surface"), (6, "reflection_mask"), (7, "albedo")] {
            assert!(source.contains(&format!("@location({location}) {field}: vec4<f32>")));
        }
        assert!(source.contains("fn enhanced_ao_receiver("), "SSGI reuses the conservative native diffuse share without enabling AO");
        assert!(source.contains("all(emissive <= vec3<f32>(0.0))"));
        assert!(source.contains("i.ao_info.x < 0.5"));
        let receiver = source.split("fn enhanced_albedo(").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(!receiver.contains("i.ao_info.x"), "partially emissive lit receivers are not forbidden just because they cannot donate");
        assert!(receiver.contains("let actor = (u32(mat.native.z) & 128u) != 0u;"));
        assert!(receiver.contains("if actor && (native_unlit()"), "static baked/unlit facades can receive while unlit actors remain excluded");
        let static_gate = receiver.split("if actor &&").next().unwrap();
        assert!(!static_gate.contains("i.effect"), "ordinary static primary effect modes retain a receiver RGB budget");
        assert!(!static_gate.contains("mat.scroll.z"), "material-local fog colour cannot disqualify a static receiver");
        assert!(receiver.contains("if actor && (native_unlit() || i.effect != 0u || mat.scroll.z > 1.5"),
            "actor effect/local-fog exclusions remain explicit");
        assert!(receiver.contains("if (u32(mat.native.z) & 48u) != 0u"), "water and presentation overlays remain excluded");
        let donor = source.split("fn enhanced_ao_receiver(").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(donor.contains("native_unlit() || i.ao_info.x < 0.5"), "static receiver admission must not weaken the pure-lit donor gate");
        assert!(receiver.contains("let texel = early_primary;"));
        assert!(source.contains("let fog = select(native_fog(i.fog), ground_fog(i), ground);"));
        assert!(source.contains("vec3<f32>(1.0)) * (1.0 - fog), 1.0);"), "receiver RGB carries native extinction exactly once");
        assert!(source.contains("vec4<f32>(albedo_sample.rgb, beauty.a)"), "even ineligible native primary fades retain opacity for destination extinction");
        assert!(!source.contains("albedo_sample.a * beauty.a"));
        for name in ["fs_opaque", "fs_test", "fs_fade_opaque", "fs_fade_test", "fs_ground_fused", "fs_ground_base", "fs_ground_simple"] {
            let body = source.split(&format!("@fragment fn {name}(")).nth(1).unwrap().split("\n}").next().unwrap();
            let cached = body.find("early_primary = sample_clamped(tex,").unwrap();
            let reject = body.find("early_reject(i, early_mode);").unwrap();
            assert!(cached < reject && reject < body.find("let albedo_sample = enhanced_albedo(").unwrap(),
                "primary native sampling precedes rejection; SSGI reuses surviving cached texels");
            assert!(body.find("let albedo_sample = enhanced_albedo(").unwrap() < body.find("let beauty = enhanced_original_").unwrap(),
                "sample derivatives before the native discard");
        }
        for name in ["fs_blend", "fs_sprite", "fs_add", "fs_env", "fs_lava", "fs_ground_shadow"] {
            let body = source.split(&format!("@fragment fn {name}(")).nth(1).unwrap().split("\n}").next().unwrap();
            assert!(body.contains("let albedo_sample = vec4<f32>(0.0);"), "{name} must not become a diffuse receiver");
            assert!(body.contains(", vec4<f32>(beauty.rgb, beauty.a));"),
                "{name}: overlays retain native colour/opacity only as destination transmittance");
        }
        assert!(!super::source(crate::EnhancedSettings::default()).contains("fn enhanced_albedo("));
    }

    #[test]
    fn indirect_source_captures_diffuse_before_material_extras() {
        for settings in [
            crate::EnhancedSettings { indirect_light: true, ..Default::default() },
            crate::EnhancedSettings { indirect_light: true, material_specular: true, ..Default::default() },
            crate::EnhancedSettings { indirect_light: true, material_maps: true, material_pbr: true, clustered_lighting: true, sun_shadows: true, ..Default::default() },
        ] {
            let source = super::source(settings);
            let vertex = source.split("fn vtx(").nth(1).unwrap().split("\n}").next().unwrap();
            assert!(!vertex.contains("enhanced_diffuse"), "fragment capture must never match the vertex environment branch");
            assert!(!vertex.contains("texture_color"));
            let shade = source.split("fn shade(").nth(1).unwrap().split("\n}").next().unwrap();
            let capture = shade.find("enhanced_diffuse = texture_color;").unwrap();
            assert!(capture < shade.find("if mat.native.x > 0.5").unwrap(), "native environment texture is not diffuse");
            assert!(capture < shade.find("let lit = min(texture_color + spec,").unwrap());
            if settings.material_pbr {
                assert!(capture < shade.find("texture_color += material_emissive(i);").unwrap());
                assert!(shade.find("light = material_diffuse(i, light);").unwrap() < capture);
                let ground = source.split("fn material_ground(").nth(1).unwrap().split("\n}").next().unwrap();
                assert!(ground.find("enhanced_diffuse = select(").unwrap() < ground.find("let extra =").unwrap());
            } else {
                assert!(source.contains("enhanced_capture_ground(tile * light, ground_fog(i), false)"));
                assert!(source.contains("enhanced_capture_ground(light, ground_fog(i), true)"));
            }
            assert!(shade.contains("enhanced_diffuse *= 1.0 - f;"));
            assert!(source.contains("let diffuse = max(enhanced_diffuse, vec3<f32>(0.0));"));
            assert!(!source.contains("let diffuse = max(beauty.rgb - "));
            assert!(source.contains("if multiplier { return vec4<f32>(clamp(enhanced_diffuse,"));
        }
        assert!(!super::source(crate::EnhancedSettings { ambient_occlusion: true, material_pbr: true, ..Default::default() }).contains("enhanced_diffuse"));
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
            // Isolate the multiplier from texture sampling, retaining the real
            // pre-discard geometric derivative consumed by receiving wrappers.
            let start = wrapper.find("\n    let early_mode").unwrap();
            let end = wrapper.find("    early_normal_dy = dpdy(early_n);\n").unwrap() + "    early_normal_dy = dpdy(early_n);\n".len();
            let mut wrapper = wrapper;
            wrapper.replace_range(start..end, "\n    early_geometric_normal = cross(dpdx(i.wpos), dpdy(i.wpos));\n");
            // Exercise the generated wrapper with a fake roof receiver: beauty
            // retains its artistic floor while raw direct visibility is zero.
            let source = format!(r#"
struct VOut {{ clip: vec4<f32>, n: vec3<f32>, wpos: vec3<f32> }}
struct EnhancedOutput {{ beauty: vec4<f32>, ambient: vec4<f32>, emission: vec4<f32>, sun: vec4<f32> }}
var<private> enhanced_sun_visibility: f32 = 1.0;
var<private> sun_memo_valid: bool = false;
var<private> early_geometric_normal: vec3<f32>;
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
        for pbr in [false, true] {
            let source = super::source(crate::EnhancedSettings {
                sun_shadows: true, moon_shadows: true, ambient_occlusion: true,
                emissive_bloom: true, hdr: true, generated_normals: true,
                material_maps: true, material_pbr: pbr, material_specular: true,
                material_reflections: true, environment_probe: true,
                clustered_lighting: true, local_light_shadows: true,
                indirect_light: true, cloud_shadows: true, ..Default::default()
            });
            let source = crate::moon_shadows::shader(source, true, true);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("combined early-alpha derivative legality"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        }
    }
}
