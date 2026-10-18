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

#[cfg(test)]
mod tests {
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
