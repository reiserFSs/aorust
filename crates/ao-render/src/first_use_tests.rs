//! Run alone: cargo test --profile fast -p ao-render first_use_after_zone_ready -- --ignored --nocapture --test-threads=1
use super::*;
use std::time::{Duration, Instant};

fn quad(material: ao_scene::Submesh) -> Scene {
    Scene {
        meshes: vec![ao_scene::Mesh {
            vertices: [[-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], [0.5, 1.0, 0.0], [-0.5, 1.0, 0.0]]
                .into_iter().map(|pos| ao_scene::Vertex { pos, normal: [0.0, 0.0, 1.0], ..Default::default() }).collect(),
            submeshes: vec![material],
        }],
        instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
        ..Default::default()
    }
}

fn material(blend: ao_scene::Blend) -> ao_scene::Submesh {
    ao_scene::Submesh { blend, two_sided: true, emissive: [0.7, 0.3, 0.1],
        ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }
}

// Same Surface -> geometry -> per-pass Submesh construction as native_liquid's offscreen fixture.
fn liquids() -> Scene {
    let mut scene = Scene::default();
    for (style, kind, passes) in [(0, 0, 2), (1, 2, 3), (2, 6, 2), (3, 14, 2), (4, 18, 1)] {
        let mesh = scene.meshes.len();
        let x = style as f32 * 0.8;
        let positions = [[x, 0.0, 0.0], [x + 0.7, 0.0, 0.0], [x + 0.7, 0.0, -1.0], [x, 0.0, -1.0]];
        let surface = ao_scene::liquid::Surface::new(mesh, style, [[0, 1, 2], [0, 2, 3]].map(|triangle|
            ao_scene::liquid::SourceTriangle { kind, vertices: triangle.map(|index|
                ao_scene::Vertex { pos: positions[index], ..Default::default() }) }).into()).unwrap();
        let (mut vertices, mut indices, mut colors) = (vec![], vec![], vec![]);
        surface.geometry([0.0; 3], false, &mut vertices, &mut indices, &mut colors);
        assert!(!indices.is_empty(), "liquid fixture must produce triangles");
        let count = vertices.len() as u32;
        let mut geometry = ao_scene::Mesh { vertices, submeshes: vec![] };
        for pass in 0..passes {
            if pass != 0 { geometry.vertices.extend_from_within(..count as usize); }
            geometry.submeshes.push(ao_scene::Submesh {
                liquid: true, two_sided: true, blend: ao_scene::Blend::AlphaBlend,
                ..ao_scene::Submesh::new(indices.iter().map(|index| index + count * pass).collect(), None)
            });
        }
        scene.meshes.push(geometry);
        scene.instances.push(ao_scene::Instance { mesh, transform: ao_scene::IDENTITY });
        scene.liquids.push(surface);
    }
    scene
}

#[test]
#[ignore = "offscreen GPU pipeline first-use regression and timings; run alone with --ignored --nocapture"]
fn first_use_after_zone_ready() {
    let started = Instant::now();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("first-use regression requires an offscreen GPU");
    eprintln!("pipeline_first_use startup_new_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
    let upload_started = Instant::now();
    renderer.set_msaa_samples(1);
    // Zone-ready deliberately contains no actors, tags, sprites, or effects.
    renderer.upload(&quad(material(ao_scene::Blend::Opaque)));
    renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    eprintln!("pipeline_first_use upload_ready_ms={:.3} world_compiles={}", upload_started.elapsed().as_secs_f64() * 1000.0, renderer.world_pipeline_compiles());
    let size = std::cell::Cell::new(128);
    let camera = Camera::look_at(Vec3::new(0.0, 1.5, 6.0), Vec3::new(0.0, 0.8, 0.0));
    let mut failures = Vec::new();
    let mut frame = |renderer: &mut Renderer, label: &str| {
        let (width, height) = (size.get(), size.get());
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("first-use regression"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let targets = Targets::new(renderer, width, height);
        let before_total = renderer.frame_thread_compiles();
        let before = renderer.world_frame_thread_compiles();
        let start = Instant::now();
        renderer.render(&view, &targets, &camera);
        let submit_ms = start.elapsed().as_secs_f64() * 1000.0;
        renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let created = renderer.world_frame_thread_compiles() - before;
        eprintln!("pipeline_first_use {label} submit_ms={submit_ms:.3} complete_ms={elapsed_ms:.3} frame_thread_compiles={created} world_compiles={}", renderer.world_pipeline_compiles());
        let total_created = renderer.frame_thread_compiles() - before_total;
        eprintln!("pipeline_first_use {label} total_frame_thread_compiles={total_created}");
        if total_created != 0 { failures.push((format!("{label}:total"), total_created)); }
        if created != 0 { failures.push((label.to_owned(), created)); }
    };
    frame(&mut renderer, "zone_ready_first");
    frame(&mut renderer, "zone_ready_warm");

    renderer.add_actor_model(1, &quad(material(ao_scene::Blend::Opaque)));
    // Same renderer-facing RSprite / TargetIndicatorMat states used by play/tags.rs.
    let tag = ao_scene::Submesh { sprite_alpha_test: true, emissive: [1.0; 3], ..material(ao_scene::Blend::AlphaBlend) };
    renderer.add_actor_model(2, &quad(tag));
    renderer.add_actor_model(3, &quad(material(ao_scene::Blend::Additive)));
    let actors = |effect| (1..=3).map(|id| {
        let mut transform = ao_scene::IDENTITY;
        transform[3][0] = (id as f32 - 2.0) * 1.2;
        ao_scene::ActorFrame { id, model: u64::from(id), transform, always: true,
            rendering_effect: if id == 1 { effect } else { 0 }, rendering_effect_time: 0.375, ..Default::default() }
    }).collect();
    renderer.set_actors(actors(0));
    frame(&mut renderer, "actors_sprites_tags_first");
    frame(&mut renderer, "actors_sprites_tags_warm");
    for effect in [1, 2, 4, 5, 6, 7] {
        renderer.set_actors(actors(effect));
        frame(&mut renderer, &format!("effect_{effect}_first"));
        frame(&mut renderer, &format!("effect_{effect}_warm"));
    }

    renderer.upload(&liquids());
    renderer.set_realistic_water(true);
    frame(&mut renderer, "liquids_first");
    frame(&mut renderer, "liquids_warm");

    // A cold async world variant must also cover slots first seen after installation.
    renderer.set_actors(vec![]);
    let settings = EnhancedSettings {
        sun_shadows: true, ambient_occlusion: true, emissive_bloom: true,
        contact_shadows: true, hdr: true, ground_grass: true, ..Default::default()
    };
    renderer.shadow_sun = Some([-0.8, 0.5, -0.3]);
    renderer.env.sun_dir = [-0.8, 0.5, -0.3];
    let requested = Instant::now();
    renderer.request_enhanced_settings(settings);
    while renderer.enhanced_settings() != settings {
        renderer.settle_enhanced_world();
        assert!(requested.elapsed() < Duration::from_secs(600), "async world variant did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    eprintln!("pipeline_first_use async_variant_ready_ms={:.3}", requested.elapsed().as_secs_f64() * 1000.0);
    // This blend was absent in both the initial and async-variant preparation.
    renderer.add_actor_model(4, &quad(material(ao_scene::Blend::ZeroSourceColor)));
    renderer.set_actors(vec![ao_scene::ActorFrame { id: 4, model: 4, always: true, ..Default::default() }]);
    frame(&mut renderer, "async_new_slot_first");
    frame(&mut renderer, "async_new_slot_warm");
    renderer.set_screen_effects(vec![ScreenLayer {
        source_blend: 5, destination_blend: 6, color: [0.1, 0.2, 0.3, 0.25],
        repetitions: 1, ..Default::default()
    }], false);
    frame(&mut renderer, "screen_layer_first");
    frame(&mut renderer, "screen_layer_warm");
    size.set(192);
    frame(&mut renderer, "enhanced_resize_first");
    frame(&mut renderer, "enhanced_resize_warm");
    assert!(failures.is_empty(), "frame-thread pipeline creations after startup: {failures:?}");
}
