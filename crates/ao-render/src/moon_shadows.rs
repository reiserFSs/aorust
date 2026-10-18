//! Opt-in moon illumination derived from authored moon/sun poses and the night sky.
use glam::Vec3;

impl crate::Renderer {
    /// Shared moon source for world lighting, sky, haze and shafts. RGB is gamma-space.
    pub fn enhanced_moonlight(&self) -> Option<([f32; 3], [f32; 3])> {
        if self.enhanced_settings.night_mode != crate::NightMode::Moonlit
            || self.enhanced_settings.moon_intensity <= 0.0 { return None; }
        let moon = self.shadow_moon?;
        Some((moon.toward_light, moon.world_gamma_budget?.map(|v| v * self.enhanced_settings.moon_intensity)))
    }
    /// Current encoded directional maps, not merely an above-horizon moon.
    /// Call after this frame's shadow encoding when selecting a shaft light.
    pub fn enhanced_shadow_moonlight(&self) -> Option<([f32; 3], [f32; 3])> {
        let frame = &self.sun_shadows.as_ref()?.coverage_uniform;
        if frame.forward[3] <= 0.0 || frame.moon_direction[3] <= 0.0 { return None; }
        Some(([frame.moon_direction[0], frame.moon_direction[1], frame.moon_direction[2]],
            [frame.moon_budget[0], frame.moon_budget[1], frame.moon_budget[2]]))
    }
}

pub(super) fn source(scene: &ao_scene::Scene) -> Option<ao_scene::MoonSource> {
    let daylight = scene.daylight?.clamp(0.0, 1.0);
    if !daylight.is_finite() || daylight >= 1.0 { return None; }
    let env = scene.environment?;
    let sun = Vec3::from(env.sun_dir).try_normalize()?;
    if !sun.is_finite() { return None; }
    let sky = Vec3::from(env.sky_color).max(Vec3::ZERO).powf(1.0 / 2.2);
    if !sky.is_finite() { return None; }
    scene.moons.iter().flatten().copied().filter_map(|mut moon| {
        let direction = Vec3::from(moon.toward_light).try_normalize()?;
        if !direction.is_finite() || direction.y <= 0.0 { return None; }
        // Illuminated fraction from the real authored sun-to-moon angle, not an
        // opposite-sun moon pose. These energy constants are Enhanced art tuning,
        // not a claim that the original client's disc light lit the world.
        let phase = (1.0 - direction.dot(sun)).clamp(0.0, 2.0) * 0.5;
        let altitude = crate::sun_shadows::horizon_fade(direction.y);
        let energy = phase * altitude * (1.0 - daylight);
        if energy <= 0.0 { return None; }
        let budget = (Vec3::new(0.075, 0.09, 0.12) + sky * 0.12) * energy;
        moon.toward_light = direction.to_array();
        moon.world_gamma_budget = Some(budget.to_array());
        Some(moon)
    }).max_by(|a, b| {
        let energy = |moon: &ao_scene::MoonSource| moon.world_gamma_budget.unwrap().iter().sum::<f32>();
        energy(a).total_cmp(&energy(b))
    })
}

pub(super) fn shader(mut source: String, shadows: bool, moonlit: bool) -> String {
    let packed = source.contains("@location(4) light: vec4<f32>");
    let clustered = source.contains("fn clustered_receiver(");
    if source.contains("struct ShadowFrame {") {
        source = source.replace("sun_visibility(", "authored_sun_visibility(")
            .replace("fn authored_sun_visibility(", "fn sun_visibility(");
        // Beauty deliberately treats the moon maps as neutral for sunlight.
        // Probe provenance must instead reject either smooth/geometric raw
        // moon shadow, including actor casters, after early coverage rejection.
        source = source.replace(
            "material_static_visibility_value = min(material_visibility_value, authored_sun_visibility(i.wpos, early_geometric_normal, i.clip.xy));",
            "material_static_visibility_value = min(sun_visibility(i.wpos, i.n, i.clip.xy), sun_visibility(i.wpos, early_geometric_normal, i.clip.xy));",
        );
        let reset = if source.contains("var<private> enhanced_sun_visibility:") { "enhanced_sun_visibility = 1.0; " } else { "" };
        source.push_str(&format!("\nfn authored_sun_visibility(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 {{\n    if shadow_frame.moon_direction.w > 0.0 {{ {reset}return 1.0; }}\n    return sun_visibility(world, normal, pixel);\n}}\n"));
    }
    for name in ["fs_opaque", "fs_fade_opaque", "fs_fade_test", "fs_test", "fs_blend", "fs_ground_fused", "fs_ground_shadow", "fs_ground_simple"] {
        let ground = name.starts_with("fs_ground");
        for input in ["i", "surface"] {
            let original = format!("    let beauty = enhanced_original_{name}({input});");
            if !clustered {
                source = source.replace(&original,
                    &format!("    let moon_receiver = authored_moon_receiver({input}, i.n, {ground});\n    let beauty = enhanced_original_{name}(moon_receiver);"));
            }
        }
    }
    source = source.replace("ground_palette(i)", "(ground_palette(i) * (1.0 - g.moon_g.w))");
    let mut moon = crate::shader_source::load("moon_shadows.wgsl", include_str!("moon_shadows.wgsl")).into_owned();
    if packed {
        for field in ["light", "dlight"] {
            moon = moon.replace(&format!("input.{field}"), &format!("input.{field}.xyz"));
            moon = moon.replace(&format!("receiver.{field} = {field};"),
                &format!("receiver.{field} = vec4<f32>({field}, input.{field}.w);"));
        }
    }
    if clustered {
        let original = "fn clustered_night_light(i: VOut, normal: vec3<f32>, ground: bool, light: vec3<f32>, retained: vec3<f32>) -> vec3<f32> {\n    return light;\n}";
        let replacement = "fn clustered_night_light(i: VOut, normal: vec3<f32>, ground: bool, light: vec3<f32>, retained: vec3<f32>) -> vec3<f32> {\n    if mat.emissive.w > 1.5 { return light; }\n    return light - max(light - retained, vec3<f32>(0.0)) * g.moon_g.w + authored_moon_add(i, normal);\n}";
        source = source.replace(original, replacement);
    }
    if moonlit { crate::moon_exposure::append_shader(&mut source); }
    else { moon = moon.replace("enhanced_sky_open(input.wpos)", "0.0"); }
    let visibility = if shadows && moonlit {
        source.push_str("\nfn authored_moon_visibility(input: VOut) -> f32 {\n    if shadow_frame.moon_direction.w <= 0.0 { return 1.0; }\n    return sun_visibility(input.wpos, input.n, input.clip.xy);\n}\n");
        "authored_moon_visibility(input)"
    } else { "1.0" };
    let raw_specular = shadows && moonlit && source.contains("struct ShadowFrame {")
        && source.contains("fn authored_ggx(");
    let specular_visibility = if raw_specular {
        "select(visibility, enhanced_sun_visibility, shadow_frame.moon_direction.w > 0.0)"
    } else { "visibility" };
    let raw_reset = if raw_specular { "enhanced_sun_visibility = 1.0;" } else { "" };
    source.push_str(&moon.replace("MOON_RAW_RESET", raw_reset)
        .replace("MOON_SPEC_VISIBILITY", specular_visibility)
        .replace("MOON_VISIBILITY", visibility));
    source
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_provenance_keeps_raw_moon_shadows_separate_from_sunlight() {
        for moonlit in [false, true] {
            let original = crate::enhanced_shader::source(crate::EnhancedSettings {
                environment_probe: true, sun_shadows: true, moon_shadows: true,
                night_mode: if moonlit { crate::NightMode::Moonlit } else { crate::NightMode::Dark },
                ..Default::default()
            });
            let generated = shader(original, true, moonlit);
            let prepare = generated.split("fn material_prepare(").nth(1).unwrap().split("\n}").next().unwrap();
            assert!(prepare.contains("material_visibility_value = authored_sun_visibility(i.wpos, i.n, i.clip.xy);"),
                "material sunlight still uses neutral visibility when moon maps are active");
            assert!(prepare.contains("material_static_visibility_value = min(sun_visibility(i.wpos, i.n, i.clip.xy), sun_visibility(i.wpos, early_geometric_normal, i.clip.xy));"),
                "persistent probe observations must see actual moon actor shadows");
            assert!(!prepare.contains("min(material_visibility_value, authored_sun_visibility("));
            assert!(generated.contains("let visibility = material_static_visibility_value;"));
        }
    }
    #[test]
    fn native_consumers_do_not_require_the_hdr_private_cache() {
        let source = crate::enhanced_shader::source(crate::EnhancedSettings {
            sun_shadows: true, ..Default::default()
        });
        let source = source.lines()
            .filter(|line| !line.contains("var<private> enhanced_sun_visibility:"))
            .collect::<Vec<_>>().join("\n");
        let generated = shader(source, true, true);
        assert!(!generated.contains("enhanced_sun_visibility"),
            "native foliage consumers have no Enhanced HDR output cache");
    }

    #[test]
    fn authored_pose_phase_and_daylight_control_energy() {
        let mut scene = ao_scene::Scene {
            daylight: Some(0.0),
            environment: Some(ao_scene::Environment {
                sky_color: [0.01; 3], fog_color: [0.0; 3], fog_start: 1.0, fog_end: 100.0,
                ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, -1.0, 0.0], sun_specular: 0.0,
            }), ..Default::default()
        };
        scene.moons[0] = Some(ao_scene::MoonSource { toward_light: [0.0, 1.0, 0.0], ..Default::default() });
        let moon = source(&scene).unwrap();
        assert!(moon.world_gamma_budget.unwrap().iter().all(|v| *v > 0.0));
        assert_eq!(moon.toward_light, [0.0, 1.0, 0.0]);
        scene.environment.as_mut().unwrap().sun_dir = [0.0, 1.0, 0.0];
        assert!(source(&scene).is_none(), "new moon has no reflected energy");
        scene.environment.as_mut().unwrap().sun_dir = [0.0, -1.0, 0.0];
        scene.daylight = Some(1.0);
        assert!(source(&scene).is_none());
        scene.daylight = None;
        assert!(source(&scene).is_none());
        scene.daylight = Some(0.0);
        scene.moons[0].as_mut().unwrap().toward_light[1] = -1.0;
        assert!(source(&scene).is_none());
    }
    #[test]
    fn unshadowed_moon_has_no_cascade_dependency() {
        let shader = shader(String::new(), false, true);
        assert!(!shader.contains("sun_visibility("));
        assert!(!shader.contains("shadow_frame"));
        assert!(shader.contains("g.moon_g"));
    }
    #[test]
    fn independent_night_shader_variants_validate_on_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("night shader regression requires GPU");
        for moonlit in [false, true] {
            for flags in 0..128 {
                for moon_shadows in [false, true] {
                    let shadows = flags & 1 != 0 || moon_shadows;
                    let original = crate::enhanced_shader::source(crate::EnhancedSettings {
                        sun_shadows: flags & 1 != 0, moon_shadows,
                        ambient_occlusion: flags & 2 != 0, emissive_bloom: flags & 4 != 0,
                        hdr: flags & 8 != 0, material_specular: flags & 16 != 0,
                        clustered_lighting: flags & 32 != 0, local_light_shadows: flags & 64 != 0,
                        night_mode: if moonlit { crate::NightMode::Moonlit } else { crate::NightMode::Dark },
                        ..Default::default()
                    });
                    let generated = shader(original, moon_shadows, moonlit);
                    assert!(generated.contains("@location(14) sun_share:"));
                    if flags & 0x70 != 0 {
                        assert!(generated.contains("receiver.light = vec4<f32>(light, input.light.w)"));
                    }
                    if shadows {
                        assert!(generated.contains("if shadow_frame.moon_direction.w > 0.0 { enhanced_sun_visibility = 1.0; return 1.0; }"),
                            "moon maps cannot darken the sunlight HDR eligibility cache");
                    }
                    if flags & (32 | 64) != 0 {
                        assert!(generated.contains("+ authored_moon_add(i, normal)"));
                    } else {
                        assert!(generated.contains("authored_moon_receiver(i, i.n,") || generated.contains("authored_moon_receiver(surface, i.n,"));
                    }
                    let moon_add = generated.split("fn authored_moon_add(").nth(1).unwrap().split("\n}").next().unwrap();
                    assert!(!moon_add.contains("terrain_sky_open"), "ordinary receivers cannot reach the ground-only bind group");
                    assert_eq!(generated.contains("@group(3) @binding(7)"), moonlit);
                    let _ = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("composed independent night modes"),
                        source: wgpu::ShaderSource::Wgsl(generated.into()),
                    });
                }
            }
        }
    }
}
