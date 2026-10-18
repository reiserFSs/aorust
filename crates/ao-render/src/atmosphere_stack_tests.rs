use crate::*;

#[test]
fn atmosphere_uses_shared_general_or_exact_map_moon_but_prefers_native_sun() {
    let sun = ao_scene::Environment { sun_dir: [0.0, 1.0, 0.0], sun_color: [0.5; 3], ..default_environment(100.0) };
    let general = ([1.0, 0.5, 0.0], [0.25, 0.5, 0.75]);
    let mapped = ([-1.0, 0.25, 0.0], [0.1, 0.2, 0.3]);
    let gap = EnhancedSettings { cloud_gap_shafts: true, ..Default::default() };
    let volume = EnhancedSettings { volumetric_shafts: true, ..Default::default() };
    assert_eq!(atmosphere_stack::effect_environment(sun, Some(general), Some(mapped), volume), (sun, false));
    let night = ao_scene::Environment { sun_dir: [0.0, -1.0, 0.0], ..sun };
    let expected = |(direction, gamma_rgb): ([f32; 3], [f32; 3])| ao_scene::Environment {
        sun_dir: direction, sun_color: gamma_rgb.map(srgb_to_linear), ..night
    };
    let unlit = ao_scene::Environment { sun_color: [0.0; 3], ..night };
    assert_eq!(atmosphere_stack::effect_environment(night, Some(general), None, gap), (expected(general), true),
        "cloud-gap-only Moon rays must not require unrelated mapped/surface shadows");
    assert_eq!(atmosphere_stack::effect_environment(night, Some(general), Some(mapped), volume), (expected(mapped), true),
        "mapped shafts must use the exact current encoded Moon map's source and energy");
    assert_eq!(atmosphere_stack::effect_environment(night, Some(general), None, volume), (unlit, false),
        "mapped-only shafts cannot substitute unencoded Moon maps");
    assert_eq!(atmosphere_stack::effect_environment(night, Some(general), None,
        EnhancedSettings { cloud_gap_shafts: true, ..volume }), (expected(general), true),
        "adding mapped shafts cannot suppress independent cloud-gap Moon light when no map is encoded");
    assert_eq!(atmosphere_stack::effect_environment(night, None, None, gap), (unlit, false),
        "no local Moon-energy fallback is allowed");
}

#[test]
fn atmosphere_switches_retire_resources_and_restore_faithful_pixels() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("Atmosphere regression needs offscreen GPU");
    renderer.wait_for_pipeline_warmup();
    assert!(renderer.pipeline_warmup_complete());
    let frame_compiles = renderer.frame_thread_compiles();
    let frame_waits = renderer.frame_pipeline_waits();
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
        let pixels = texture_to_rgba(renderer, &output, 16, 16).unwrap();
        assert_eq!(renderer.frame_thread_compiles() - frame_compiles, 0, "atmosphere capture compiled a pipeline on the frame thread");
        assert_eq!(renderer.frame_pipeline_waits() - frame_waits, 0, "atmosphere capture waited for a cold pipeline");
        pixels
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
        assert_eq!(renderer.frame_thread_compiles() - frame_compiles, 0);
        assert_eq!(renderer.frame_pipeline_waits() - frame_waits, 0);
    }
}

#[test]
fn atmosphere_startup_pipelines_survive_frame_scoped_toggles() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("Atmosphere warmup regression needs offscreen GPU");
    renderer.wait_for_pipeline_warmup();
    let frame_compiles = renderer.frame_thread_compiles();
    let frame_waits = renderer.frame_pipeline_waits();
    let defaults = EnhancedSettings::default();
    let enabled = EnhancedSettings {
        height_haze: true, land_haze: true, valley_mist: true, volumetric_shafts: true,
        cloud_gap_shafts: true, volumetric_clouds: true, cloud_shadows: true, sky_discs: true,
        ..defaults
    };
    let _frame = crate::pipeline_creation::FrameGuard::enter();
    for settings in [enabled, defaults, enabled, defaults] {
        renderer.release_disabled_atmosphere(settings);
        assert!(renderer.atmosphere.as_ref().expect("cached startup atmosphere").is_ready());
        assert!(renderer.volumetric_clouds.as_ref().expect("cached startup clouds").is_ready());
        assert!(renderer.sky_discs.as_ref().expect("cached startup sky discs").is_ready());
        assert!(renderer.pipeline_warmup_complete());
        assert_eq!(renderer.enhanced_target_bytes(), 0, "pipeline caching must not count static buffers as frame targets");
    }
    assert_eq!(renderer.frame_thread_compiles() - frame_compiles, 0);
    assert_eq!(renderer.frame_pipeline_waits() - frame_waits, 0);
}

#[test]
fn native_liquid_cloud_attachment_tracks_admitted_water_and_reuses_variants() {
    use ao_scene::{liquid::{SourceTriangle, Surface}, Blend, Instance, Mesh, Scene, Submesh, Vertex, IDENTITY};
    let vertices = [[-20.0,0.0,-20.0],[20.0,0.0,-20.0],[20.0,0.0,20.0],[-20.0,0.0,20.0]]
        .map(|pos| Vertex { pos, normal:[0.0,1.0,0.0], ..Default::default() });
    let indices = vec![0,1,2,0,2,3];
    let source = Surface::new(0,0,indices.as_chunks::<3>().0.iter().map(|triangle| SourceTriangle {
        kind:0x80ff_ffff, vertices:std::array::from_fn(|i| vertices[triangle[i] as usize]),
    }).collect()).expect("native liquid source");
    let first = Submesh {
        liquid:true, two_sided:true, blend:Blend::AlphaBlend, base_color:[0.2,0.4,0.6,0.5],
        ..Submesh::new(indices.clone(),None)
    };
    let mut second = first.clone();
    second.indices.iter_mut().for_each(|index| *index += 4);
    let scene = Scene {
        meshes:vec![Mesh { vertices:vertices.into_iter().cycle().take(8).collect(), submeshes:vec![first,second] }],
        instances:vec![Instance { mesh:0, transform:IDENTITY }], liquids:vec![source],
        water_light:Some(([0.3;3],[0.0,1.0,0.0])),
        environment:Some(default_environment(100.0)), ..Default::default()
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance,None).expect("water attachment regression needs offscreen GPU");
    renderer.upload(&scene);
    let defaults = EnhancedSettings::default();
    let eye = Vec3::new(0.0,10.0,0.0);
    let visible = Camera::look_at(eye,Vec3::new(0.0,0.0,-0.1));
    let away = Camera::look_at(Vec3::new(0.0,10.0,100.0),Vec3::new(0.0,10.0,200.0));
    for (samples, hdr) in [(1,true),(4,true),(1,false),(4,false)] {
        let enabled = EnhancedSettings { cloud_shadows:true, hdr, ..defaults };
        // Warm the faithful sample-count variant before measuring a roundtrip;
        // its first-ever 4x build is not a water-visibility cache miss.
        renderer.set_enhanced_settings(defaults);
        renderer.set_msaa_samples(samples);
        renderer.wait_for_pipeline_warmup();
        if hdr {
            renderer.set_enhanced_settings(EnhancedSettings { hdr:true, ..defaults });
        }
        renderer.set_enhanced_settings(enabled);
        renderer.wait_for_pipeline_warmup();
        assert_eq!(renderer.msaa_samples(),samples,"water regression requires native 1x/4x");
        let output = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label:Some("native liquid cloud attachment lifecycle"),
            size:wgpu::Extent3d { width:32,height:32,depth_or_array_layers:1 },
            mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:renderer.format,
            usage:wgpu::TextureUsages::RENDER_ATTACHMENT,view_formats:&[],
        });
        let view = output.create_view(&Default::default());
        let targets = Targets::new(&renderer,32,32);
        let frame_compiles = renderer.frame_thread_compiles();
        let frame_waits = renderer.frame_pipeline_waits();
        let world_compiles = renderer.world_pipeline_compiles();
        renderer.render(&view,&targets,&visible);
        assert!(renderer.enhanced_frame.as_ref().unwrap().water_cloud.is_some(),"admitted native water needs its scalar MRT");
        let water_key = renderer.world_key;
        let frame = renderer.enhanced_frame.as_ref().unwrap();
        let primary = (frame.colour.clone(), frame.depth.clone(), frame.sun.clone());
        renderer.render(&view,&targets,&away);
        assert!(renderer.enhanced_frame.as_ref().unwrap().water_cloud.is_none(),"frustum-rejected water must not retain its MRT");
        let frame = renderer.enhanced_frame.as_ref().unwrap();
        assert_eq!((frame.colour.clone(), frame.depth.clone(), frame.sun.clone()), primary,
            "water visibility must change only the scalar attachment, not primary targets or effect histories");
        renderer.set_enhanced_settings(defaults);
        renderer.render(&view,&targets,&visible);
        assert!(renderer.enhanced_frame.is_none(),"all-off must retire the Enhanced frame");
        assert_eq!(renderer.enhanced_target_bytes(),0);
        renderer.set_enhanced_settings(enabled);
        renderer.render(&view,&targets,&visible);
        assert!(renderer.enhanced_frame.as_ref().unwrap().water_cloud.is_some());
        assert_eq!(renderer.world_key,water_key,"returning water must reinstall the cached scalar-MRT variant");
        assert!(renderer.pending_world.is_none(),"visibility changes must use already-warmed variants");
        assert_eq!(renderer.world_pipeline_compiles(),world_compiles,"visibility/all-off roundtrip compiled a world variant");
        assert_eq!(renderer.frame_thread_compiles(),frame_compiles,"water visibility compiled on the frame thread");
        assert_eq!(renderer.frame_pipeline_waits(),frame_waits,"water visibility waited on a pipeline");
        if samples == 1 && hdr {
            let requested = EnhancedSettings { ambient_occlusion:true, ..enabled };
            renderer.request_enhanced_settings(requested);
            let pending_key = renderer.pending_world.as_ref().expect("cold AO switch must remain pending").key;
            renderer.render(&view,&targets,&away);
            assert_eq!(renderer.pending_world.as_ref().map(|pending| pending.key), Some(pending_key),
                "a cached water-shape swap must not drop the user's pending settings switch");
            renderer.await_enhanced_world();
        }
    }
}
