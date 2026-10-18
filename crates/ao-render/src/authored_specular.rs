//! Authored-material-only per-pixel specular; no extra resources or faithful shader edits.

pub(super) fn append(source: &mut String, moonlit: bool) {
    let start = source.find("fn vertex_lights(").expect("native lights");
    let end = start + source[start..].find("\n}\n").expect("native lights end") + 3;
    let mut local = source[start..end].replace("fn vertex_lights(", "fn authored_local_spec(")
        .replace("power: f32)", "alpha: f32, f0: vec3<f32>)")
        .replace("if power > 0.0", "if true")
        .replace("pow(max(dot(n, normalize(l + v)), 0.0), power)", "authored_ggx(n, l, v, alpha, f0)");
    let clustered = source.find("fn clustered_pixel_lights(").map(|start| {
        let end = start + source[start..].find("\n}\n").expect("cluster lights end") + 3;
        source[start..end].replace("fn clustered_pixel_lights(", "fn authored_cluster_spec(")
            .replace("footprint: f32)", "footprint: f32, alpha: f32, f0: vec3<f32>)")
            .replace("max(dot(n, l), 0.0)", "authored_ggx(n, l, normalize(g.eye.xyz - p), alpha, f0)")
    });
    if let Some(clustered) = clustered {
        local = local.replace("if !(any(c", "if false && !(any(c")
            .replace("let dyn_count = u32(g.dyn_info.x);", "let dyn_count = 0u;");
        source.push_str(&clustered);
    }
    source.push_str(&local);
    pack_material(source);
    *source = source.replace("    let beauty = enhanced_original_", "    authored_normal = i.n;\n    let beauty = enhanced_original_");
    *source = source.replace("    let lit = min(texture_color + spec,", "    spec = authored_specular(i, spec);\n    let lit = min(texture_color + spec,");
    let mut shader = crate::shader_source::load("authored_specular.wgsl", include_str!("authored_specular.wgsl")).into_owned();
    if source.contains("struct ShadowFrame {") {
        shader = shader.replace(
            "    let visibility = sun_visibility(i.wpos, i.n, i.clip.xy);",
            "    enhanced_sun_visibility = 1.0;\n    var visibility = sun_visibility(i.wpos, i.n, i.clip.xy);\n    if shadow_frame.moon_direction.w == 0.0 { visibility = enhanced_sun_visibility; }",
        );
    }
    if source.contains("fn authored_cluster_spec(") {
        shader = shader.replace("authored_local_spec(i.wpos, n, alpha, f0)[1]", "(authored_local_spec(i.wpos, n, alpha, f0)[1] + authored_cluster_spec(i.wpos, n, footprint, alpha, f0))");
    }
    if moonlit {
        shader = shader.replace(
            "    return max(local + sun, vec3<f32>(0.0));",
            "    let moon = enhanced_moon_radiance * authored_ggx(n, g.moon_direction.xyz, v, alpha, f0);\n    return max(local + sun + moon, vec3<f32>(0.0));",
        );
    }
    source.push_str(&shader);
}

pub(super) fn pack_material(source: &mut String) {
    if source.contains("@location(4) light: vec4<f32>") { return; }
    // Protect the native Lit output from the VOut-only widening below.
    let start = source.find("fn light_vertex_sun(").expect("native lighting");
    let end = start + source[start..].find("\n}").expect("native lighting end") + 2;
    let lighting = source[start..end].replace("var o: Lit", "var native_lit: Lit")
        .replace("o.", "native_lit.").replace("return o;", "return native_lit;");
    source.replace_range(start..end, &lighting);
    // Four spare scalar components keep bloom + shadows within 16 varyings.
    for field in ["light", "dlight", "spec", "tint"] {
        pack(source, field);
    }
    for (name, values) in [
        ("vs", "to_g(mat.spec.rgb), mat.spec.w"),
        ("vs_actor", "to_g(eff_spec), eff_power"),
    ] {
        let start = source.find(&format!("fn {name}(")).expect("material vertex");
        let end = start + source[start..].find("\n}").expect("material vertex end");
        let body = source[start..end].replace("    return o;", &format!("    let authored = vec4<f32>({values});\n    o.light.w = authored.x;\n    o.dlight.w = authored.y;\n    o.spec.w = authored.z;\n    o.tint.w = authored.w;\n    return o;"));
        source.replace_range(start..end, &body);
    }
}

fn pack(source: &mut String, field: &str) {
    let start = source.find("struct VOut {").expect("native VOut");
    let end = start + source[start..].find("\n}").expect("native VOut end");
    let varying = source[start..end].replace(&format!("{field}: vec3<f32>,"), &format!("{field}: vec4<f32>,"));
    source.replace_range(start..end, &varying);
    for variable in ["i", "o"] {
        *source = source.replace(&format!("{variable}.{field}"), &format!("{variable}.{field}.xyz"));
        *source = source.replace(&format!("{variable}.{field}.xyz.w"), &format!("{variable}.{field}.w"));
    }
    let marker = format!("    o.{field}.xyz = ");
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find(&marker) {
        let start = cursor + relative;
        let end = start + source[start..].find(';').expect("varying assignment end");
        let expr = source[start + marker.len()..end].to_owned();
        let replacement = format!("    o.{field} = vec4<f32>({expr}, 0.0)");
        source.replace_range(start..end, &replacement);
        cursor = start + replacement.len() + 1;
    }
}

/// Materials are an Enhanced-only assembly step; native textures and alpha/depth
/// equations remain untouched. All image processing happens on texture upload.
pub(super) fn append_materials(source: &mut String, settings: crate::EnhancedSettings) {
    pack_material(source);
    let mut shader = crate::shader_source::load("materials.wgsl", include_str!("materials.wgsl")).into_owned();
    for (name, enabled) in [
        ("MATERIAL_GENERATED", settings.generated_normals),
        ("MATERIAL_MAPS", settings.material_maps),
        ("MATERIAL_PBR", settings.material_pbr),
        ("MATERIAL_STATIC_PROVENANCE", settings.material_reflections || settings.environment_probe),
    ] {
        shader = shader.replace(&format!("const {name}: bool = false;"), &format!("const {name}: bool = {enabled};"));
    }
    if source.contains("fn clustered_pixel_lights(") {
        shader = shader.replace(
            "vertex_lights(i.wpos, n, 0.0)[0] - vertex_lights(i.wpos, base, 0.0)[0]",
            "clustered_pixel_lights(i.wpos, n, material_footprint) - clustered_pixel_lights(i.wpos, base, material_footprint) + clustered_directional(n) - clustered_directional(base) - g.sun_g.rgb * (max(dot(n, g.sun_dir.xyz), 0.0) - max(dot(base, g.sun_dir.xyz), 0.0))",
        );
    }
    source.push_str(&shader);
    *source = source.replace("    let envelope = vec4<f32>(1.0);", "    material_prepare(i);\n    let envelope = vec4<f32>(1.0);");
    *source = source.replace("    authored_normal = i.n;", "    authored_normal = material_shading_normal(i);");
    if settings.generated_normals || settings.material_maps || settings.material_pbr {
        *source = source.replace("    var texture_color = min(t.rgb", "    if !native_unlit() && mat.emissive.w < 1.5 && i.effect == 0u { light = material_diffuse(i, light); }\n    var texture_color = min(t.rgb");
        // Emission is added in the native pre-fog equation, never as an overlay.
        *source = source.replace("    let lit = min(texture_color + spec,", "    texture_color += material_emissive(i);\n    let lit = min(texture_color + spec,");
        if settings.emissive_bloom && settings.material_maps {
            let mut multiplier = false;
            *source = source.lines().map(|line| {
                if line.starts_with("@fragment fn ") { multiplier = line.starts_with("@fragment fn fs_ground_shadow("); }
                if !multiplier && line.starts_with("    let emission_sample = ") {
                    format!("{} + vec4<f32>(material_emissive(i) * (1.0 - native_fog(i.fog)), 0.0);", line.trim_end_matches(';'))
                } else { line.to_owned() }
            }).collect::<Vec<_>>().join("\n");
        }
        // Ground has both a fused path and the native base * shadow path.
        // Add specular/emission only once; divide by base for ZERO/SRCCOLOR.
        for (name, fused, simple) in [
            ("enhanced_original_fs_ground_fused", true, false),
            ("enhanced_original_fs_ground_shadow", false, false),
            ("enhanced_original_fs_ground_simple", true, true),
        ] {
            let start = source.find(&format!("fn {name}(")).expect("ground material receiver");
            let end = start + source[start..].find("\n}").expect("ground receiver end");
            let body = source[start..end].to_owned();
            let body = if simple {
                body.replace("ground_tile(i) * i.dlight.xyz", "material_ground(i, ground_tile(i), i.dlight.xyz, true)")
                    .replace("ground_tile(i) * cluster_light", "material_ground(i, ground_tile(i), cluster_light, true)")
                    .replace("ground_tile(i) * max(i.dlight.xyz - i.sun_spec_share.xyz * (1.0 - sun_visibility(i.wpos, i.n, i.clip.xy)), vec3<f32>(0.0))",
                        "material_ground(i, ground_tile(i), max(i.dlight.xyz - i.sun_spec_share.xyz * (1.0 - sun_visibility(i.wpos, i.n, i.clip.xy)), vec3<f32>(0.0)), true)")
            } else if fused {
                body.replace("tile * light", "material_ground(i, tile, light, true)")
            } else {
                body.replace("mix(light, vec3<f32>(1.0)", "mix(material_ground(i, sample_clamped(tex, material_uv(i)).rgb, light, false), vec3<f32>(1.0)")
            };
            source.replace_range(start..end, &body);
        }
    }
    if settings.material_pbr {
        *source = source.replace("    let alpha = sqrt(clamp(2.0 / (max(power, 0.0) + 2.0) + variance, 0.0004, 1.0));", "    let alpha = material_alpha(i);");
        *source = source.replace("    let f0 = clamp(vec3<f32>(i.light.w, i.dlight.w, i.spec.w), vec3<f32>(0.0), vec3<f32>(1.0));",
            "    let albedo = material_albedo_value;\n    let f0 = mix(vec3<f32>(0.04), albedo, material_metallic(i));");
        *source = source.replace(" || power <= 0.0 || !any(f0 > vec3<f32>(0.0))", "");
        // Ground alpha testing can discard inside ground_tile; all derivatives
        // and implicit samples for GGX were cached by material_prepare first.
        let start = source.find("fn authored_specular(").expect("material GGX");
        let end = start + source[start..].find("\n}").expect("material GGX end");
        let body = source[start..end]
            .replace("    let dx = dpdx(n);\n    let dy = dpdy(n);\n    let variance = max(dot(dx, dx), dot(dy, dy));\n", "    let variance = material_variance;\n")
            .replace("max(length(fwidth(i.wpos)), 1e-6)", "material_footprint")
            .replace("sun_visibility(i.wpos, i.n, i.clip.xy)", "material_visibility_value")
            .replace("return max(local + sun", "return max(material_ambient_specular(i, n, v, f0) + local + sun");
        source.replace_range(start..end, &body);
    }
    let specular = if settings.material_pbr { "authored_specular(i, vec3<f32>(0.0))" } else { "vec3<f32>(0.0)" };
    source.push_str(&format!("\nfn material_ground(i: VOut, base: vec3<f32>, light: vec3<f32>, fused: bool) -> vec3<f32> {{\n    let diffuse = material_diffuse(i, light);\n    let extra = {specular} + material_emissive(i);\n    return select(diffuse + extra / max(base, vec3<f32>(0.01)), base * diffuse + extra, fused);\n}}\n"));
    if settings.material_reflections || settings.environment_probe {
        let clustered = if source.contains("fn clustered_pixel_lights(") {
            " && !any(clustered_pixel_lights(i.wpos, material_shading_normal(i), material_footprint) > vec3<f32>(0.0))"
        } else { "" };
        source.push_str(&format!("\nfn material_static_source(i: VOut) -> f32 {{\n    let local = vertex_lights(i.wpos, material_shading_normal(i), 0.0);\n    let visibility = material_static_visibility_value;\n    return select(0.0, 1.0, (u32(mat.native.z) & 128u) == 0u && (u32(mat.native.z) & 64u) == 0u && i.effect == 0u && all(mat.scroll.xy == vec2<f32>(0.0)) && i.ao_info.y == 0.0 && !any(local[0] > vec3<f32>(0.0)) && !any(local[1] > vec3<f32>(0.0)){clustered} && visibility >= 0.999);\n}}\n"));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn material_switches_are_independent_and_prepare_before_discard() {
        for settings in [
            crate::EnhancedSettings { generated_normals: true, ..Default::default() },
            crate::EnhancedSettings { material_maps: true, ..Default::default() },
            crate::EnhancedSettings { material_pbr: true, ..Default::default() },
            crate::EnhancedSettings { material_reflections: true, ..Default::default() },
            crate::EnhancedSettings { environment_probe: true, ..Default::default() },
        ] {
            let source = crate::enhanced_shader::source(settings);
            assert!(source.contains(&format!("const MATERIAL_GENERATED: bool = {};", settings.generated_normals)));
            assert!(source.contains(&format!("const MATERIAL_MAPS: bool = {};", settings.material_maps)));
            assert!(source.contains(&format!("const MATERIAL_PBR: bool = {};", settings.material_pbr)));
            assert!(source.contains("material_prepare(i);"));
            assert!(!source.contains("@location(16)"));
            if settings.material_pbr {
                assert!(source.contains("let alpha = material_alpha(i);"));
                assert!(source.contains("let f0 = mix(vec3<f32>(0.04), albedo, material_metallic(i));"));
                assert!(!source.contains(" || power <= 0.0"));
            }
            if settings.material_reflections || settings.environment_probe {
                assert!(source.contains("@location(5) reflection_surface"));
                assert!(source.contains("@location(6) reflection_mask"));
                assert!(source.contains("i.ao_info.y == 0.0"));
                assert!(source.contains("all(mat.scroll.xy == vec2<f32>(0.0))"));
                assert!(source.contains("visibility >= 0.999"));
                assert!(source.contains("any(dl[0] > vec3<f32>(0.0))"));
                assert!(source.contains("128u"));
            }
        }
        let off = crate::enhanced_shader::source(crate::EnhancedSettings::default());
        assert!(!off.contains("fn material_prepare"));
        assert!(!off.contains("var generated_normal_tex"));
    }

    #[test]
    fn generated_relief_strength_does_not_scale_painted_normals() {
        let source = crate::enhanced_shader::source(crate::EnhancedSettings {
            generated_normals: true, material_maps: true, ..Default::default()
        });
        let generated = source.split("if MATERIAL_GENERATED {").nth(1).unwrap().split("\n    }").next().unwrap();
        let painted = source.split("if (material_mask_value & 1u) != 0u {").nth(1).unwrap().split("\n    }").next().unwrap();
        assert!(generated.contains("t = vec3<f32>(t.xy * 3.0, t.z);"));
        assert!(!painted.contains("* 3.0"));
        assert!(source.contains("select(1.0, 9.0, MATERIAL_GENERATED && (material_mask_value & 1u) == 0u)"));
    }

    #[test]
    fn generic_pbr_adds_reference_split_sum_without_changing_authored_only_ggx() {
        let generic = crate::enhanced_shader::source(crate::EnhancedSettings {
            material_pbr: true, ..Default::default()
        });
        assert!(generic.contains("return max(material_ambient_specular(i, n, v, f0) + local + sun"));
        assert!(generic.contains("vec4<f32>(-1.0, -0.0275, -0.572, 0.022)"));
        assert!(generic.contains("vec4<f32>(1.0, 0.0425, 1.04, -0.04)"));
        assert!(generic.contains("mix(ambient * 0.5, sky * 1.3, smoothstep(-0.4, 0.6, reflected.y))"));
        assert!(generic.contains("exp2(-16.0 * roughness - 1.0)"));
        let authored = crate::enhanced_shader::source(crate::EnhancedSettings {
            material_specular: true, ..Default::default()
        });
        assert!(!authored.contains("material_ambient_specular("));
        let brdf = |roughness: f64, nv: f64| {
            let r = [1.0 - roughness, 0.0425 - 0.0275 * roughness, 1.04 - 0.572 * roughness, -0.04 + 0.022 * roughness];
            let a004 = (r[0] * r[0]).min((-9.28 * nv).exp2()) * r[0] + r[1];
            0.04 * (-1.04 * a004 + r[2]) + 1.04 * a004 + r[3]
        };
        assert!((brdf(0.7, 1.0) - 0.02468).abs() < 0.00001);
        assert!(brdf(0.7, 0.01) > brdf(0.7, 1.0), "grazing dielectric reflection retains split-sum Fresnel");
        assert!(brdf(1.0, 1.0) > 0.0, "roughness broadens rather than deleting integrated specular energy");
    }

    #[test]
    fn native_reflection_budget_survives_default_roughness_but_fades_matte() {
        let source = crate::enhanced_shader::source(crate::EnhancedSettings {
            environment_probe: true, ..Default::default()
        });
        assert!(source.contains("mix(max(0.3, authored_f0), 1.0, material_metallic(i))"));
        assert!(source.contains("1.0 - smoothstep(0.6, 1.0, material_roughness(i))"));
        let t = (0.7_f64 - 0.6) / (1.0 - 0.6);
        assert!((0.3 * (1.0 - t * t * (3.0 - 2.0 * t)) - 0.253125).abs() < 1e-9);
    }

    #[test]
    fn probe_provenance_excludes_foliage_and_the_complete_clustered_light_pool() {
        for clustered_lighting in [false, true] {
            let source = crate::enhanced_shader::source(crate::EnhancedSettings {
                environment_probe: true, clustered_lighting, ..Default::default()
            });
            let helper = source.split("fn material_static_source(").nth(1).unwrap().split("\n}").next().unwrap();
            assert!(helper.contains("(u32(mat.native.z) & 64u) == 0u"));
            assert!(helper.contains("let visibility = material_static_visibility_value;"));
            assert_eq!(helper.contains("!any(clustered_pixel_lights("), clustered_lighting);
            if clustered_lighting {
                assert!(helper.contains("clustered_pixel_lights(i.wpos, material_shading_normal(i), material_footprint)"));
            }
        }
    }
    #[test]
    fn standalone_ggx_uses_admitted_sources_without_pixel_diffuse_override() {
        let source = crate::enhanced_shader::source(crate::EnhancedSettings {
            material_specular: true, ..Default::default()
        });
        assert!(!source.contains("fn clustered_receiver("));
        for name in ["vertex_lights", "authored_local_spec"] {
            let start = source.find(&format!("fn {name}(")).unwrap();
            let body = source[start..].split("\n}\n").next().unwrap();
            assert!(body.contains("dyn_lights[322u]"));
            assert!(body.contains("used < 8u"));
            let admission = body.find("if !owner_light_admitted(").unwrap();
            let budget = body.find("used = used + 1u;").unwrap();
            assert!(admission < budget, "hidden ownership consumed the native eight-light budget");
        }
    }
    #[test]
    fn packing_changes_only_varying_declarations_not_helper_parameters() {
        let source = crate::enhanced_shader::source(crate::EnhancedSettings {
            clustered_lighting: true, ..Default::default()
        });
        assert!(source.contains("light: vec3<f32>, retained: vec3<f32>"));
    }

    #[test]
    fn moon_ggx_uses_only_shared_precosine_radiance_when_moonlit() {
        for mode in [crate::NightMode::AsGame, crate::NightMode::Dark, crate::NightMode::Moonlit] {
            let generated = crate::enhanced_shader::source(crate::EnhancedSettings {
                material_specular: true, night_mode: mode, ..Default::default()
            });
            let specular = generated.split("fn authored_specular(").nth(1).unwrap().split("\n}").next().unwrap();
            assert_eq!(specular.contains("enhanced_moon_radiance * authored_ggx(n, g.moon_direction.xyz"), mode == crate::NightMode::Moonlit);
            assert!(!specular.contains("enhanced_sky_open("), "GGX must reuse the receiver's exposure sample");
            assert!(!specular.contains("authored_moon_visibility("), "GGX must reuse the receiver's moon-shadow sample");
            if mode == crate::NightMode::AsGame {
                assert!(!generated.contains("enhanced_moon_radiance"));
            }
        }
    }

    #[test]
    fn authored_material_specular_is_opt_in_and_uses_authored_values() {
        let off = crate::enhanced_shader::source(crate::EnhancedSettings::default());
        assert!(!off.contains("fn authored_ggx"));
        for shadows in [false, true] {
            for bloom in [false, true] {
                let source = crate::enhanced_shader::source(crate::EnhancedSettings {
                    sun_shadows: shadows, ambient_occlusion: true, emissive_bloom: bloom,
                    hdr: true, material_specular: true, ..Default::default()
                });
                assert!(source.contains("fn authored_ggx"));
                assert!(source.contains("let dx = dpdx(n)"));
                assert!(source.contains("to_g(eff_spec), eff_power"));
                assert!(source.contains("return max(local + sun,"));
                assert!(!source.contains("@location(16)"));
                assert!(source.contains("spec = authored_specular(i, spec);"));
            }
        }
    }
}

#[cfg(test)]
mod receiver_tests {
    #[test]
    fn specular_preserves_fog_unauthored_materials_and_off_roundtrip() {
        use crate::{Camera, EnhancedSettings, Renderer, Targets, Vec3, texture_to_rgba};
        use ao_scene::{Environment, Mesh, Scene, Submesh, Vertex};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("specular regression GPU");
        renderer.set_msaa_samples(1);
        for (sun, fog_end, prelit, authored) in [
            (0.0, 100.0, false, true), (0.7, 100.0, true, true),
            (0.7, 2.0, false, true), (0.7, 100.0, false, false),
        ] {
            renderer.upload(&Scene {
                environment: Some(Environment {
                    sky_color: [0.0; 3], fog_color: [0.05; 3], fog_start: 0.0, fog_end,
                    ambient: [0.1; 3], sun_color: [sun; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 1.0,
                }),
                meshes: vec![Mesh {
                    vertices: [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                        .into_iter().map(|(x, y)| Vertex {
                            pos: [x, y, -3.0], normal: [0.0, 0.0, 1.0], ..Default::default()
                        }).collect(),
                    submeshes: vec![Submesh {
                        specular: [if authored { 0.2 } else { 0.0 }; 3],
                        shininess: 32.0, prelit, two_sided: true,
                        ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
                    }],
                }],
                instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
                ..Default::default()
            });
            let capture = |renderer: &mut Renderer, enabled| {
                renderer.set_enhanced_settings(EnhancedSettings { material_specular: enabled, ..Default::default() });
                let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("authored specular receiver envelope"),
                    size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                    format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                renderer.render(&output.create_view(&Default::default()), &Targets::new(renderer, 32, 32),
                    &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
                texture_to_rgba(renderer, &output, 32, 32).unwrap()
            };
            let native = capture(&mut renderer, false);
            if fog_end > 3.0 {
                assert!(native.as_chunks::<4>().0.iter().any(|p| p[0] > 0), "receiver must draw");
            }
            let enhanced = capture(&mut renderer, true);
            assert!(enhanced.as_chunks::<4>().0.iter().zip(native.as_chunks::<4>().0)
                .all(|(a, b)| a[3] == b[3]), "specular must not change coverage");
            if prelit || !authored || sun == 0.0 || fog_end < 3.0 {
                assert_eq!(native, enhanced, "no authored specular must remain faithful");
            }
            assert_eq!(native, capture(&mut renderer, false), "all-off restores faithful bytes");
        }
    }
}

#[cfg(test)]
mod moon_numeric_tests {
    #[test]
    fn shared_moon_radiance_drives_ggx_and_zero_radiance_or_full_fog_is_dark() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("moon GGX numeric GPU");
        let ggx = include_str!("authored_specular.wgsl").split("fn authored_specular(").next().unwrap();
        let source = format!("{ggx}\n{}", r#"
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[index], 0.0, 1.0);
}
@fragment fn fs(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    // The shared receiver cache can be positive or zero (closed roof/shadow).
    let radiance = select(vec3<f32>(0.25), vec3<f32>(0.0), pixel.x >= 1.0 && pixel.x < 2.0);
    let moon = radiance * authored_ggx(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), 0.25, vec3<f32>(0.04));
    let fog = select(0.0, 1.0, pixel.x >= 2.0);
    return vec4<f32>(mix(moon, vec3<f32>(0.0), fog), 1.0);
}
"#);
        let shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shared moon GGX numeric control"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = renderer.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[], immediate_size: 0,
        });
        let pipeline = renderer.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shared moon GGX numeric control"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"),
                compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState {
                    format: renderer.format, blend: None, write_mask: wgpu::ColorWrites::ALL,
                })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            multiview_mask: None, cache: None,
        });
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("moon GGX numeric pixels"), size: wgpu::Extent3d { width: 3, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("moon GGX numeric control"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.draw(0..3, 0..1);
        }
        renderer.queue.submit([encoder.finish()]);
        let pixels = crate::texture_to_rgba(&renderer, &output, 3, 1).unwrap();
        // Normal incidence, alpha=.25: GGX*pi is almost 4, so .25*.04*4=.04.
        assert!(pixels[..3].iter().all(|value| (9..=11).contains(value)), "positive moon GGX control: {pixels:?}");
        assert_eq!(&pixels[4..8], &[0, 0, 0, 255], "zero shared radiance adds no highlight");
        assert_eq!(&pixels[8..12], &[0, 0, 0, 255], "full native fog extinguishes the highlight");
    }
}

#[cfg(test)]
mod directional_visibility_tests {
    #[test]
    fn ggx_uses_raw_selected_directional_visibility_and_ao_vk_horizon_fade() {
        for mode in [crate::NightMode::AsGame, crate::NightMode::Moonlit] {
            for shadows in [false, true] {
                let settings = crate::EnhancedSettings {
                    material_specular: true, sun_shadows: shadows, moon_shadows: shadows,
                    night_mode: mode, ..Default::default()
                };
                let source = crate::enhanced_shader::source(settings);
                let source = if mode == crate::NightMode::Moonlit {
                    crate::moon_shadows::shader(source, shadows, true)
                } else { source };
                let specular = source.split("fn authored_specular(").nth(1).unwrap().split("\n}").next().unwrap();
                assert!(specular.contains("smoothstep(0.03, 0.25, g.sun_dir.y)"), "the ao-vk low-sun highlight fade must remain in the GGX sun path");
                assert_eq!(specular.contains("if shadow_frame.moon_direction.w == 0.0 { visibility = enhanced_sun_visibility; }"), shadows);
                if shadows {
                    assert!(source.contains("enhanced_sun_visibility = visible;\n    return mix(1.0, visible,"), "GGX without HDR still captures visibility before the artistic shade floor");
                    assert!(specular.contains("enhanced_sun_visibility = 1.0;"), "outside-cascade early returns must not reuse another receiver's raw sample");
                } else {
                    assert!(!specular.contains("shadow_frame"), "unshadowed variants cannot reference an absent frame binding");
                }
                if mode == crate::NightMode::Moonlit {
                    let moon = source.split("fn authored_moon_add(").nth(1).unwrap().split("\n}").next().unwrap();
                    assert!(moon.contains("return energy * visibility * cosine;"), "moon diffuse retains artistic shadow strength");
                    assert_eq!(moon.contains("select(visibility, enhanced_sun_visibility, shadow_frame.moon_direction.w > 0.0)"), shadows);
                    assert_eq!(moon.matches("let visibility = authored_moon_visibility(input);").count(), usize::from(shadows), "moon specular reuses the one diffuse shadow sample");
                    assert_eq!(moon.matches("enhanced_sky_open(input.wpos)").count(), 1, "moon specular reuses the one roof sample");
                    assert!(!moon.contains("MOON_"), "all visibility templates must be resolved");
                }
            }
        }
    }
}
