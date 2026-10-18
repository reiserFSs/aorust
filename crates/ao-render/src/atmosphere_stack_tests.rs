use crate::*;

#[test]
fn atmosphere_switches_retire_resources_and_restore_faithful_pixels() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("Atmosphere regression needs offscreen GPU");
    renderer.set_msaa_samples(4);
    let actual_samples = renderer.msaa_samples();
    if renderer.msaa_supported.contains(&4) {
        assert_eq!(actual_samples, 4);
    } else {
        assert!(renderer.msaa_supported.contains(&actual_samples));
    }
    let capture = |renderer: &mut Renderer| {
        let output = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Atmosphere switch pixel oracle"),
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let targets = Targets::new(renderer, 16, 16);
        renderer.render(&output.create_view_tracked(&Default::default()), &targets,
            &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
        texture_to_rgba(renderer, &output, 16, 16).unwrap()
    };
    let defaults = EnhancedSettings::default();
    renderer.set_enhanced_settings(defaults);
    let faithful = capture(&mut renderer);
    assert_eq!(renderer.enhanced_target_bytes(), 0);
    for (name, settings) in [
        ("height_haze", EnhancedSettings { height_haze: true, ..defaults }),
        ("land_haze", EnhancedSettings { land_haze: true, ..defaults }),
        ("valley_mist", EnhancedSettings { valley_mist: true, ..defaults }),
        ("volumetric_shafts", EnhancedSettings { volumetric_shafts: true, ..defaults }),
        ("cloud_gap_shafts", EnhancedSettings { cloud_gap_shafts: true, ..defaults }),
        ("volumetric_clouds", EnhancedSettings { volumetric_clouds: true, ..defaults }),
        ("cloud_shadows", EnhancedSettings { cloud_shadows: true, ..defaults }),
        ("sky_discs", EnhancedSettings { sky_discs: true, ..defaults }),
    ] {
        renderer.set_enhanced_settings(settings);
        capture(&mut renderer);
        assert!(renderer.enhanced_target_bytes() > 0, "{name} must allocate its Enhanced frame");
        assert_eq!(renderer.msaa_samples(), renderer.clamp_msaa(4, settings));
        renderer.set_enhanced_settings(defaults);
        assert_eq!(renderer.enhanced_target_bytes(), 0, "{name} must retire its frame on reset");
        assert_eq!(capture(&mut renderer), faithful, "{name} all-off roundtrip changed faithful pixels");
        assert_eq!(renderer.enhanced_target_bytes(), 0);
    }
}
