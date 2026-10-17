//! Actual headless image regression: a wall-concealed actor cannot seed a cascade.
use super::*;

fn quad(points: [[f32; 3]; 4], normal: [f32; 3]) -> ao_scene::Mesh {
    ao_scene::Mesh {
        vertices: points.into_iter().map(|pos| ao_scene::Vertex { pos, normal, uv: [0.0; 2], color: [1.0; 4] }).collect(),
        submeshes: vec![ao_scene::Submesh { two_sided: true, ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
    }
}

#[test]
fn hidden_actor_present_absent_images_are_identical() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless shadow GPU");
    let mut scene = Scene {
        meshes: vec![
            quad([[-8.0, 0.0, -8.0], [8.0, 0.0, -8.0], [8.0, 0.0, 8.0], [-8.0, 0.0, 8.0]], [0.0, 1.0, 0.0]),
            quad([[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 4.0, 0.0], [-2.0, 4.0, 0.0]], [0.0, 0.0, 1.0]),
        ],
        ..Default::default()
    };
    scene.instances = (0..scene.meshes.len()).map(|mesh| ao_scene::Instance { mesh, transform: ao_scene::IDENTITY }).collect();
    renderer.upload(&scene);
    renderer.shadow_sun = Some([-0.8, 0.5, 0.3]);
    renderer.env.sun_dir = [-0.8, 0.5, 0.3];
    renderer.env.sun_color = [0.6; 3];
    renderer.env.ambient = [0.2; 3];
    renderer.env.fog_start = 100.0;
    renderer.env.fog_end = 120.0;
    let actor = Scene { meshes: vec![
        quad([[-0.4, 0.0, -1.0], [0.4, 0.0, -1.0], [0.4, 1.8, -1.0], [-0.4, 1.8, -1.0]], [0.0, 0.0, 1.0]),
        quad([[-0.25, 1.0, -1.2], [0.25, 1.0, -1.2], [0.25, 1.6, -1.2], [-0.25, 1.6, -1.2]], [0.0, 0.0, 1.0]),
    ], ..Default::default() };
    renderer.add_actor_model(1, &actor);
    renderer.set_enhanced_settings(EnhancedSettings { sun_shadows: true, ..Default::default() });
    let (width, height) = (384, 256);
    let targets = Targets::new(&renderer, width, height);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shadow fairness image"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let camera = Camera::look_at(Vec3::new(0.0, 3.0, 8.0), Vec3::new(0.0, 1.0, 0.0));
    let capture = |renderer: &mut Renderer| {
        renderer.render(&view, &targets, &camera);
        texture_to_rgba(renderer, &output, width, height).unwrap()
    };
    let absent = capture(&mut renderer);
    renderer.set_actors(vec![ao_scene::ActorFrame { id: 1, model: 1, ..Default::default() }]);
    let hidden = capture(&mut renderer);
    assert_eq!(absent, hidden, "opaque-wall-hidden actor changed the shadowed image");
    renderer.set_actors(vec![ao_scene::ActorFrame { id: 1, model: 1, priority: Some(8), transform: Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() }]);
    assert_eq!(absent, capture(&mut renderer), "actor outside the executed native world priorities seeded a shadow");
    renderer.set_actors(vec![ao_scene::ActorFrame { id: 1, model: 1, alpha: 0.0, transform: Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() }]);
    assert_eq!(absent, capture(&mut renderer), "invisible actor changed the shadowed image");
    let visible_actor = ao_scene::ActorFrame { id: 1, model: 1, transform: Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() };
    renderer.set_actors(vec![visible_actor.clone()]);
    let visible = capture(&mut renderer);
    write_evidence(&[("absent", &absent), ("hidden", &hidden), ("visible", &visible)], (width, height));
    assert_ne!(absent, visible, "control actor must actually enter the visible image");
    assert_eq!(renderer.sun_shadows.as_ref().unwrap().actor_ids, [1, 1], "body and camera-hidden rigid part must share actor admission");
    // This receiver lies outside both the wall and the actor's camera silhouette.
    let aspect = width as f32 / height as f32;
    let projection = Mat4::perspective_rh(renderer.lens.vertical_fov(aspect), aspect, renderer.lens.near,
        renderer.lens.far.unwrap_or(132.0));
    let receiver = (projection * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up()))
        .project_point3(Vec3::new(-3.5, 0.0, -1.5625));
    let x = ((receiver.x * 0.5 + 0.5) * width as f32) as usize;
    let y = ((0.5 - receiver.y * 0.5) * height as f32) as usize;
    assert!(x < width as usize && y < height as usize);
    let pixel = (y * width as usize + x) * 4;
    let energy = |image: &[u8]| image[pixel..pixel + 3].iter().map(|&value| u32::from(value)).sum::<u32>();
    assert!(energy(&visible) < energy(&absent), "current visible actor did not shadow the exposed receiver");
    let original_environment = renderer.env;
    renderer.set_actors(vec![]);
    renderer.shadow_sun = Some([-0.8, 0.5, -0.3]);
    renderer.env.sun_dir = [-0.8, 0.5, -0.3];
    renderer.env.fog_start = 8.0;
    renderer.env.fog_end = 9.0;
    let fog_absent = capture(&mut renderer);
    renderer.set_actors(vec![visible_actor]);
    let fog_present = capture(&mut renderer);
    let near_receiver = (projection * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up()))
        .project_point3(Vec3::new(-3.5, 0.0, -0.4375));
    let x = ((near_receiver.x * 0.5 + 0.5) * width as f32) as usize;
    let y = ((0.5 - near_receiver.y * 0.5) * height as f32) as usize;
    let pixel = (y * width as usize + x) * 4;
    assert_eq!(fog_absent[pixel..pixel + 4], fog_present[pixel..pixel + 4], "fully fogged actor disclosed itself through a nearer ground shadow");
    renderer.env = original_environment;
    renderer.set_actors(vec![]);
    renderer.shadow_sun = None;
    let unshadowed = capture(&mut renderer);
    assert_ne!(absent, unshadowed, "enabled static shadow must change a receiver (non-vacuous shadow test)");
    assert!(absent.iter().zip(&unshadowed).all(|(shadowed, original)| shadowed <= original), "shadowing increased the original image envelope");
    for y in (height as usize * 2 / 3)..(height as usize * 11 / 12) {
        for x in (width as usize / 16)..(width as usize / 4) {
            let pixel = (y * width as usize + x) * 4;
            assert_eq!(absent[pixel..pixel + 4], unshadowed[pixel..pixel + 4], "unoccluded flat receiver has PCF self-acne at {x},{y}");
        }
    }
    write_evidence(&[("unshadowed", &unshadowed)], (width, height));
    renderer.set_enhanced_settings(EnhancedSettings::default());
    capture(&mut renderer);
    assert!(renderer.sun_shadows.is_none(), "off switch retains shadow resources");
}

#[test]
fn absent_unauthored_or_night_light_has_no_shadow_source() {
    let mut scene = Scene { ground_light: Some(([1.0; 3], [0.0, 1.0, 0.0])), ..Default::default() };
    assert!(shadow_pass::authored_sun(&scene).is_none());
    scene.sun_authored = true;
    assert_eq!(shadow_pass::authored_sun(&scene), Some([0.0, 1.0, 0.0]));
    scene.ground_light = Some(([0.0; 3], [0.0, 1.0, 0.0]));
    assert!(shadow_pass::authored_sun(&scene).is_none());
    scene.ground_light = Some(([1.0; 3], [0.0; 3]));
    assert!(shadow_pass::authored_sun(&scene).is_none());
}

fn write_evidence(images: &[(&str, &[u8])], size: (u32, u32)) {
    let Some(directory) = std::env::var_os("AOMAC_GFX_SHADOW_FAIRNESS_OUT") else { return; };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    for (name, pixels) in images {
        std::fs::write(directory.join(format!("{name}.rgba")), pixels).unwrap();
        let mut encoder = png::Encoder::new(std::fs::File::create(directory.join(format!("{name}.png"))).unwrap(), size.0, size.1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(pixels).unwrap();
    }
}
