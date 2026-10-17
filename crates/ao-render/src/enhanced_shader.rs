//! Enhanced-only shader generation; the faithful WGSL is never modified.

pub(super) fn source(shadows: bool, ao: bool, bloom: bool) -> String {
    let mut source = include_str!("shader.wgsl").to_owned();
    source = source.replace("    @location(12) env_uv: vec2<f32>,", "    @location(12) env_uv: vec2<f32>,\n    @location(13) ambient_share: vec3<f32>,\n    @location(14) sun_share: vec3<f32>,\n    @location(15) sun_spec_share: vec3<f32>,");
    source = source.replace("    var o: VOut;", "    var o: VOut;\n    o.ambient_share = vec3<f32>(0.0);\n    o.sun_share = vec3<f32>(0.0);\n    o.sun_spec_share = vec3<f32>(0.0);");
    source = source.replace("    o.dlight = l.dlight;", "    o.dlight = l.dlight;\n    o.ambient_share = select(l.ambient_share, vec3<f32>(0.0), prelit);\n    o.sun_share = select(l.sun_share, vec3<f32>(0.0), prelit);\n    o.sun_spec_share = select(l.sun_spec_share, vec3<f32>(0.0), prelit);");
    // Water uses authored WaterLight, not the outdoor sun, and has no AO.
    let liquid_start = source.find("fn vs_liquid(").expect("native liquid vertex");
    let liquid_end = liquid_start + source[liquid_start..].find("\n}").expect("liquid end");
    let liquid = source[liquid_start..liquid_end].replace("    o.ambient_share = select(l.ambient_share, vec3<f32>(0.0), prelit);\n    o.sun_share = select(l.sun_share, vec3<f32>(0.0), prelit);\n    o.sun_spec_share = select(l.sun_spec_share, vec3<f32>(0.0), prelit);\n", "");
    source.replace_range(liquid_start..liquid_end, &liquid);
    source = source.replace("struct Lit {", "struct Lit {\n    ambient_share: vec3<f32>,\n    sun_share: vec3<f32>,\n    sun_spec_share: vec3<f32>,");
    source = source.replace("    return o;\n}\n\n// mode:", "    let base = to_g(emissive) + dl[0];\n    let direct = sun_diffuse * max(sn, 0.0);\n    o.ambient_share = max(o.light - min(base + direct, vec3<f32>(1.0)), vec3<f32>(0.0));\n    o.sun_share = max(o.light - min(base + g.ambient_g.rgb, vec3<f32>(1.0)), vec3<f32>(0.0));\n    o.sun_spec_share = max(o.spec - min(to_g(specular) * dl[1], vec3<f32>(1.0)), vec3<f32>(0.0));\n    return o;\n}\n\n// mode:");
    source = source.replace("    var light = i.light;\n    var spec = i.spec;", "    var shadow = 0.0;\n    if any(i.sun_share > vec3<f32>(0.0)) || any(i.sun_spec_share > vec3<f32>(0.0)) { shadow = 1.0 - sun_visibility(i.wpos); }\n    var light = max(i.light - i.sun_share * shadow, vec3<f32>(0.0));\n    var spec = max(i.spec - i.sun_spec_share * shadow, vec3<f32>(0.0));");
    source = source.replace("    o.light = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + vertex_lights(o.wpos, n, 0.0)[0],", "    let ground_local = vertex_lights(o.wpos, n, 0.0)[0];\n    o.light = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + ground_local,");
    source = source.replace("    o.dlight = clamp(o.light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));", "    o.dlight = clamp(o.light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));\n    o.ambient_share = o.dlight - o.light;\n    o.sun_share = vec4<f32>(max(o.light - clamp(ground_local, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.0)), 0.0);\n    o.sun_spec_share = vec4<f32>(max(o.dlight - clamp(ground_local + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.0)), 0.0);");
    source = source.replace("clamp(i.light + ground_palette(i),", "clamp(i.light - i.sun_share * (1.0 - sun_visibility(i.wpos)) + ground_palette(i),");
    source = source.replace("ground_tile(i) * i.dlight", "ground_tile(i) * max(i.dlight - i.sun_spec_share * (1.0 - sun_visibility(i.wpos)), vec3<f32>(0.0))");
    // Spare varying components carry emission without exceeding 16 variables.
    for (location, field, channel) in [(13, "ambient_share", "x"), (14, "sun_share", "y"), (15, "sun_spec_share", "z")] {
        source = source.replace(&format!("@location({location}) {field}: vec3<f32>"), &format!("@location({location}) {field}: vec4<f32>"));
        source = source.replace(&format!("o.{field} = vec3<f32>(0.0);"), &format!("o.{field} = vec4<f32>(0.0, 0.0, 0.0, mat.emissive.{channel});"));
        source = source.replace(&format!("o.{field} = select(l.{field}, vec3<f32>(0.0), prelit);"), &format!("o.{field} = vec4<f32>(select(l.{field}, vec3<f32>(0.0), prelit), o.{field}.w);"));
        source = source.replace(&format!("i.{field}"), &format!("i.{field}.xyz"));
    }
    source = source.replace("o.ambient_share = o.dlight - o.light;", "o.ambient_share = vec4<f32>(o.dlight - o.light, 0.0);");
    source = source.replace("    o.effect_time = power.w;", "    o.effect_time = power.w;\n    let enhanced_emissive = select(mat.emissive.rgb, emissive.rgb, emissive.w > 0.5);\n    o.ambient_share.w = enhanced_emissive.x;\n    o.sun_share.w = enhanced_emissive.y;\n    o.sun_spec_share.w = enhanced_emissive.z;");
    source = source.replace("        o.light = vec3<f32>(1.0);", "        o.light = vec3<f32>(1.0);\n        o.sun_share = vec4<f32>(0.0, 0.0, 0.0, o.sun_share.w);\n        o.sun_spec_share = vec4<f32>(0.0, 0.0, 0.0, o.sun_spec_share.w);");
    // Retain the native fragment bodies (including discard/coverage), adding
    // attachment wrappers rather than maintaining a second copy of the shader.
    let names: Vec<String> = source.lines().filter_map(|line| line.strip_prefix("fn fs_").and_then(|rest| rest.split_once('(')).map(|(name, _)| format!("fs_{name}"))).collect();
    source = source.replace("@fragment\nfn fs_", "fn enhanced_original_fs_");
    source = source.replace(" -> @location(0) vec4<f32>", " -> vec4<f32>");
    source.push_str(OUTPUT);
    for name in names {
        let ambient = match name.as_str() {
            "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" => "enhanced_ambient(i, beauty)",
            "fs_ground_simple" => "vec4<f32>(min(beauty.rgb, ground_tile(i) * i.ambient_share.xyz * (1.0 - i.fog)), beauty.a)",
            "fs_ground_shadow" => "vec4<f32>(1.0)",
            _ => "vec4<f32>(0.0, 0.0, 0.0, beauty.a)",
        };
        let emission = if name == "fs_ground_shadow" { "vec4<f32>(1.0)" } else if name.starts_with("fs_sky_") || name.starts_with("fs_ground_") || matches!(name.as_str(), "fs_env" | "fs_lava" | "fs_lava_crust") { "vec4<f32>(0.0, 0.0, 0.0, beauty.a)" } else if matches!(name.as_str(), "fs_add" | "fs_mesh_add" | "fs_add_one" | "fs_holo") { "vec4<f32>(beauty.rgb, beauty.a)" } else { "enhanced_emission(i, beauty)" };
        let ambient = if ao { ambient } else { "vec4<f32>(0.0)" };
        let emission = if bloom { emission } else { "vec4<f32>(0.0)" };
        let ambient_sample = ambient.replace("beauty", "envelope");
        let emission_sample = emission.replace("beauty", "envelope");
        let ambient_result = if ao && name == "fs_ground_shadow" { "ambient_sample" } else { "vec4<f32>(min(beauty.rgb, ambient_sample.rgb), beauty.a)" };
        let emission_result = if bloom && name == "fs_ground_shadow" {
            "emission_sample"
        } else if bloom && matches!(name.as_str(), "fs_add" | "fs_mesh_add" | "fs_add_one" | "fs_holo") {
            "vec4<f32>(select(beauty.rgb * (1.0 - native_fog(i.fog)), vec3<f32>(0.0), (u32(mat.native.z) & 16u) != 0u), beauty.a)"
        } else if bloom && name == "fs_blend" {
            "vec4<f32>(select(min(beauty.rgb, emission_sample.rgb), beauty.rgb * (1.0 - native_fog(i.fog)), (u32(mat.native.z) & 24u) == 8u), beauty.a)"
        } else {
            "vec4<f32>(min(beauty.rgb, emission_sample.rgb), beauty.a)"
        };
        let receiving = shadows && matches!(name.as_str(), "fs_opaque" | "fs_fade_opaque" | "fs_fade_test" | "fs_test" | "fs_blend" | "fs_sprite" | "fs_add" | "fs_mesh_add" | "fs_add_one" | "fs_ground_fused" | "fs_ground_shadow" | "fs_ground_simple");
        let geometric_normal = if receiving { "    var surface = i;\n    surface.n = cross(dpdx(i.wpos), dpdy(i.wpos));\n" } else { "" };
        let fragment_input = if receiving { "surface" } else { "i" };
        source.push_str(&format!("\n@fragment fn {name}(i: VOut) -> EnhancedOutput {{\n{geometric_normal}    let envelope = vec4<f32>(1.0);\n    let ambient_sample = {ambient_sample};\n    let emission_sample = {emission_sample};\n    let beauty = enhanced_original_{name}({fragment_input});\n    return EnhancedOutput(beauty, {ambient_result}, {emission_result});\n}}\n"));
    }
    source = source.replace("sun_visibility(i.wpos)", "sun_visibility(i.wpos, i.n)");
    if shadows {
        let shader = include_str!("sun_shadows.wgsl");
        source.push_str(shader.split("@group(3) @binding(3)").next().expect("shadow receiver declarations"));
        source.push_str(&shader[shader.find("fn cascade_visibility(").expect("shadow receiver function")..]);
    } else {
        source.push_str("\nfn sun_visibility(world: vec3<f32>, normal: vec3<f32>) -> f32 { return 1.0; }\n");
    }
    // Remove disabled effects structurally, not merely by multiplying by zero.
    for (enabled, name) in [(ao, "enhanced_ambient"), (bloom, "enhanced_emission")] {
        if !enabled {
            let start = source.find(&format!("fn {name}(")).expect("enhanced helper");
            let end = start + source[start..].find("\n}\n").expect("enhanced helper end") + 3;
            source.replace_range(start..end, "");
        }
    }
    for (enabled, field, location) in [(ao, "ambient_share", 13), (shadows, "sun_share", 14), (shadows, "sun_spec_share", 15)] {
        if !enabled {
            source = source.replace(&format!("select(l.{field}, vec3<f32>(0.0), prelit)"), "vec3<f32>(0.0)");
            source = source.lines().filter(|line| {
                !line.starts_with(&format!("    {field}: vec3<f32>,"))
                    && !line.starts_with(&format!("    o.{field} = max("))
                    && !line.starts_with(&format!("    o.{field} = vec4<f32>(max("))
                    && !(field == "ambient_share" && line.starts_with("    o.ambient_share = vec4<f32>(o.dlight"))
            }).collect::<Vec<_>>().join("\n");
            if !bloom {
                source = source.lines().filter(|line| {
                    !line.contains(&format!("@location({location}) {field}:"))
                        && !line.trim_start().starts_with(&format!("o.{field} ="))
                }).collect::<Vec<_>>().join("\n");
            }
        }
    }
    if !shadows {
        source = source.replace("i.sun_share.xyz", "vec3<f32>(0.0)");
        source = source.replace("i.sun_spec_share.xyz", "vec3<f32>(0.0)");
    }
    if !bloom {
        source = source.lines().filter(|line| !line.contains("let enhanced_emissive =") && !line.contains(".w = enhanced_emissive.")).collect::<Vec<_>>().join("\n");
        for field in ["ambient_share", "sun_share", "sun_spec_share"] {
            source = source.replace(&format!(", o.{field}.w)"), ", 0.0)");
        }
    }
    source
}

const OUTPUT: &str = r#"
struct EnhancedOutput {
    @location(0) beauty: vec4<f32>,
    @location(1) ambient: vec4<f32>,
    @location(2) emission: vec4<f32>,
}
fn enhanced_ambient(i: VOut, beauty: vec4<f32>) -> vec4<f32> {
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    var share = i.ambient_share.xyz;
    // Authored room/prelit illumination is already baked. Self-light masks
    // replace lighting rather than receiving additional ambient occlusion.
    if native_unlit() || mat.emissive.w > 1.5 || i.effect != 0u {
        share = vec3<f32>(0.0);
    }
    if mat.emissive.w > 0.5 && mat.emissive.w < 1.5 && t.a > 0.0 {
        share = vec3<f32>(0.0);
    }
    let lit = min(t.rgb * i.tint * i.light + i.spec, vec3<f32>(1.0));
    let without = min(t.rgb * i.tint * max(i.light - share, vec3<f32>(0.0)) + i.spec, vec3<f32>(1.0));
    return vec4<f32>(min(beauty.rgb, max(lit - without, vec3<f32>(0.0)) * (1.0 - native_fog(i.fog))), beauty.a);
}
fn enhanced_emission(i: VOut, beauty: vec4<f32>) -> vec4<f32> {
    // Liquid emission is native lighting authoring, not a bloom source.
    // SourceColorOne is handled after the native body, using actual beauty.
    if (u32(mat.native.z) & 24u) != 0u { return vec4<f32>(0.0); }
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    var emission = vec3<f32>(0.0);
    if mat.emissive.w < 1.5 {
        emission = t.rgb * i.tint * min(i.light, to_g(vec3<f32>(i.ambient_share.w, i.sun_share.w, i.sun_spec_share.w)));
    }
    if mat.emissive.w > 0.5 && mat.emissive.w < 1.5 && (i.effect == 0u || i.effect == 3u || i.effect == 7u) {
        emission = max(emission, t.rgb * i.tint * max(min(i.light + t.a, vec3<f32>(1.0)) - i.light, vec3<f32>(0.0)));
    }
    emission = min(beauty.rgb, emission * (1.0 - native_fog(i.fog)));
    return vec4<f32>(emission, beauty.a);
}
"#;

#[cfg(test)]
mod tests {
    #[test]
    fn enhanced_preserves_native_entrypoints_and_separates_baked_light() {
        let source = super::source(false, true, true);
        assert!(source.contains("@fragment fn fs_ground_shadow"));
        assert!(source.contains("@fragment fn fs_lava_crust"));
        assert!(!source.contains("@group(3)"));
        assert!(source.contains("o.ambient_share = max(o.light - min(base + direct"));
        assert!(source.contains("mat.emissive.w > 1.5 || i.effect != 0u"));
        assert!(!include_str!("shader.wgsl").contains("EnhancedOutput"));
        let ao_only = super::source(false, true, false);
        assert!(!ao_only.contains("sun_share:"));
        assert!(!ao_only.contains("sun_spec_share:"));
        assert!(!ao_only.contains("fn enhanced_emission"));
        let shadows_only = super::source(true, false, false);
        assert!(!shadows_only.contains("ambient_share:"));
        assert!(!shadows_only.contains("fn enhanced_ambient"));
    }

    #[test]
    fn enhanced_shader_validates_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("Enhanced WGSL requires offscreen GPU");
        for flags in 0..8 {
            let (shadows, ao, bloom) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Enhanced ambient/emissive/shadow syntax regression"),
                source: wgpu::ShaderSource::Wgsl(super::source(shadows, ao, bloom).into()),
            });
        }
    }
}
