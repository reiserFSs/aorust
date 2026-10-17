//! Actual headless image regression: a wall-concealed actor cannot seed a cascade.
use super::*;

#[test]
fn static_shadow_ranges_preserve_draw_order_and_instance_holes() {
    let commands = [(0, 0), (0, 1), (0, 3), (1, 4), (0, 5), (0, 5), (0, 4)];
    let mut batches = Vec::new();
    for (draw, instance) in commands {
        shadow_pass::push_static_shadow_command(&mut batches, draw, instance);
    }
    assert_eq!(batches, [(0, 0..2), (0, 3..4), (1, 4..5), (0, 5..6), (0, 5..6), (0, 4..5)]);
    let expanded: Vec<_> = batches.iter().flat_map(|(draw, range)| range.clone().map(|instance| (*draw, instance))).collect();
    assert_eq!(expanded, commands);
}

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
    // The camera sees the wall front; the sun sees its back. A one-sided statel
    // must still occlude sunlight and cast all the way down to its base.
    scene.meshes[1].submeshes[0].two_sided = false;
    scene.instances = (0..scene.meshes.len()).map(|mesh| ao_scene::Instance { mesh, transform: ao_scene::IDENTITY }).collect();
    renderer.upload(&scene);
    renderer.shadow_sun = Some([-0.8, 0.5, -0.3]);
    renderer.env.sun_dir = [-0.8, 0.5, -0.3];
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
        .project_point3(Vec3::new(-3.5, 0.0, -0.4375));
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
    assert_ne!(absent, unshadowed, "one-sided statel back face did not cast a shadow");
    assert!(absent.iter().zip(&unshadowed).all(|(shadowed, original)| shadowed <= original), "shadowing increased the original image envelope");
    let contact = (projection * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up()))
        .project_point3(Vec3::new(0.0, 0.0, 0.25));
    let contact_pixel = (((0.5 - contact.y * 0.5) * height as f32) as usize * width as usize
        + ((contact.x * 0.5 + 0.5) * width as f32) as usize) * 4;
    assert!(absent[contact_pixel] < unshadowed[contact_pixel], "wall shadow detached from its base");
    let ambient_floor = (renderer.env.ambient[0].powf(1.0 / 2.2) * 255.0).floor() as u8;
    assert!(absent[contact_pixel] >= ambient_floor.saturating_sub(1), "shadow removed ambient, not just sun");
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

#[test]
fn grass_pixels_receive_sun_shadows_and_ao_without_emission() {
    use ao_scene::ground_detail::{GroundCell, GroundPatch};
    // A 1x1 patch has terminal roots: native selection never marks those
    // high-quality, so it cannot exercise grass's fine-cell admission.
    let mut ground = ao_scene::Mesh { vertices: vec![], submeshes: vec![] };
    let mut fine_cells = Vec::new();
    let mut fine_indices = Vec::new();
    for z in 0..2 {
        for x in 0..2 {
            let first = ground.vertices.len() as u32;
            for [dx, dz] in [[0, 0], [1, 0], [0, 1], [1, 1]] {
                ground.vertices.push(ao_scene::Vertex {
                    pos: [(x + dx) as f32 * 2.0, 0.0, -((z + dz) as f32) * 2.0],
                    normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4],
                });
            }
            fine_cells.push(GroundCell { submesh: 0, vertices: [first, first + 1, first + 2, first + 3],
                native_tile: 2, ground_material: 15, ..Default::default() });
            fine_indices.extend([first, first + 1, first + 2, first + 1, first + 3, first + 2]);
        }
    }
    let lattice_start = ground.vertices.len() as u32;
    for z in 0..=2 {
        for x in 0..=2 {
            ground.vertices.push(ao_scene::Vertex {
                pos: [x as f32 * 2.0, 0.0, -(z as f32) * 2.0],
                normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4],
            });
        }
    }
    let green = ao_scene::TextureKey { rdb_type: 1_010_006, id: 1 };
    ground.submeshes = vec![
        ao_scene::Submesh::new(fine_indices, Some(green)),
        ao_scene::Submesh::new(vec![lattice_start, lattice_start + 2, lattice_start + 6,
            lattice_start + 2, lattice_start + 8, lattice_start + 6], None),
    ];
    let sun = [0.0, 0.8, -0.6];
    let scene = Scene {
        textures: [(green, ao_scene::Texture { width: 8, height: 8, rgba: [60, 104, 40, 255].repeat(64), source: ao_scene::TextureSource::GroundRgb })].into(),
        meshes: vec![
            ground,
            quad([[0.0, 0.0, -3.0], [4.0, 0.0, -3.0], [4.0, 3.0, -3.0], [0.0, 3.0, -3.0]], [0.0, 0.0, 1.0]),
        ],
        instances: (0..2).map(|mesh| ao_scene::Instance { mesh, transform: ao_scene::IDENTITY }).collect(),
        ground_detail: vec![GroundPatch {
            mesh: 0, cells: 2, cell_size: 2.0, height_scale: 1.0, heights: vec![0; 9],
            lattice_start, coarse_submesh: 1, fine_cells,
            ..Default::default()
        }],
        sun_authored: true,
        ground_light: Some(([0.5; 3], sun)),
        ..Default::default()
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless grass receiver GPU");
    renderer.set_msaa_samples(1);
    renderer.upload(&scene);
    renderer.shadow_sun = Some(sun);
    renderer.env.sun_dir = sun;
    renderer.env.sun_color = [0.5; 3];
    renderer.env.ambient = [0.15; 3];
    renderer.env.fog_start = 100.0;
    renderer.env.fog_end = 120.0;
    renderer.time = 0.0;
    let camera = Camera::look_at(Vec3::new(2.0, 1.5, 3.0), Vec3::new(2.0, 0.2, -2.0));
    let (width, height) = (384, 256);
    let targets = Targets::new(&renderer, width, height);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grass receiver image"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let grass = EnhancedSettings { ground_grass: true, grass_density: 1.0, grass_distance: 30.0, ..Default::default() };
    let capture = |renderer: &mut Renderer, settings: EnhancedSettings| {
        renderer.set_enhanced_settings(settings);
        if settings.ground_grass && settings.grass_density > 0.0 {
            // This synthetic fixture explicitly supplies an exposed native cell;
            // collision/exposure rejection is covered separately.
            renderer.set_enhanced_grass_exposure(Some(GrassExposure {
                generation: 1, query: std::rc::Rc::new(|_| true),
                line_of_sight: std::rc::Rc::new(|_, _| true),
            }));
            renderer.warm_grass_tiles(&camera);
        }
        renderer.render(&view, &targets, &camera);
        texture_to_rgba(renderer, &output, width, height).unwrap()
    };
    let faithful = capture(&mut renderer, EnhancedSettings::default());
    let bare = capture(&mut renderer, EnhancedSettings { grass_density: 0.0, ..grass });
    assert_eq!(faithful, bare, "zero-density grass changed the native image");
    let blades = capture(&mut renderer, grass);
    assert_eq!(renderer.enhanced_grass_cell_count(), 4, "fixture did not generate its native fine grass cells");
    // With one sample and no postprocessing, only visible grass fragments can
    // differ from the density-zero image. Ground/wall-only changes cannot pass.
    let blade_mask: Vec<_> = blades.as_chunks::<4>().0.iter().zip(bare.as_chunks::<4>().0.iter()).map(|(a, b)| a != b).collect();
    assert!(blade_mask.iter().filter(|&&blade| blade).count() > 100, "fixture has no substantial visible blade coverage");
    let darkened_blades = |effect: &[u8], effect_bare: &[u8]| {
        blade_mask.iter().zip(blades.as_chunks::<4>().0.iter()).zip(effect.as_chunks::<4>().0.iter().zip(effect_bare.as_chunks::<4>().0.iter()))
            .filter(|((blade, original), (changed, ground))| {
                **blade && changed != ground
                    && changed[..3].iter().map(|&c| u32::from(c)).sum::<u32>()
                        < original[..3].iter().map(|&c| u32::from(c)).sum::<u32>()
            }).count()
    };
    let shadows = EnhancedSettings { sun_shadows: true, ..grass };
    let shadow_bare = capture(&mut renderer, EnhancedSettings { grass_density: 0.0, ..shadows });
    let shadow_blades = capture(&mut renderer, shadows);
    assert!(darkened_blades(&shadow_blades, &shadow_bare) > 8, "caster did not darken actual grass pixels");
    let ao = EnhancedSettings { ambient_occlusion: true, ao_radius: 2.0, ..grass };
    let ao_bare = capture(&mut renderer, EnhancedSettings { grass_density: 0.0, ..ao });
    let ao_blades = capture(&mut renderer, ao);
    assert!(darkened_blades(&ao_blades, &ao_bare) > 8, "AO did not darken actual grass pixels");
    assert_eq!(blades, capture(&mut renderer, EnhancedSettings { ao_strength: 0.0, ..ao }), "zero AO strength must be neutral on grass");
    assert_eq!(blades, capture(&mut renderer, EnhancedSettings { ao_radius: 0.0, ..ao }), "zero AO radius must be neutral on grass");
    let bloom = capture(&mut renderer, EnhancedSettings { emissive_bloom: true, bloom_strength: 1.0, ..grass });
    assert_eq!(blades, bloom, "non-emissive grass seeded bloom");
    let emission = renderer.emissive_seed_rgba().unwrap().expect("bloom emission attachment");
    for (pixel, &blade) in emission.as_chunks::<4>().0.iter().zip(&blade_mask) {
        if blade { assert_eq!(*pixel, [0; 4], "grass fragment wrote authored emission"); }
    }
    renderer.shadow_sun = None;
    let no_sun = capture(&mut renderer, grass);
    assert_eq!(no_sun, capture(&mut renderer, shadows), "missing authored shadow source must be neutral on grass");
    assert_eq!(blades, no_sun, "disabling the source changed the shadows-off grass image");
    assert_eq!(faithful, capture(&mut renderer, EnhancedSettings::default()), "all-off roundtrip changed the native image");
    assert!(renderer.grass.is_none(), "disabled grass retained its resources");
    write_evidence(&[
        ("grass-bare", &bare), ("grass-blades", &blades),
        ("grass-shadow-bare", &shadow_bare), ("grass-shadow-blades", &shadow_blades),
        ("grass-ao-bare", &ao_bare), ("grass-ao-blades", &ao_blades),
        ("grass-emission", &emission),
    ], (width, height));
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

#[test]
fn soft_shadow_penumbra_grows_with_caster_distance_and_is_stable() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless shadow GPU");
    let awning = |height: f32, z0: f32, z1: f32|
        quad([[0.0, height, z0], [20.0, height, z0], [20.0, height, z1], [0.0, height, z1]], [0.0, 1.0, 0.0]);
    // Two awnings with a common x = 0 edge, 1 m and 12 m above the ground.
    let scene = Scene {
        meshes: vec![
            quad([[-30.0, 0.0, -30.0], [30.0, 0.0, -30.0], [30.0, 0.0, 30.0], [-30.0, 0.0, 30.0]], [0.0, 1.0, 0.0]),
            awning(1.0, -8.0, -2.0), awning(12.0, 2.0, 8.0),
        ],
        ..Default::default()
    };
    renderer.upload(&Scene {
        instances: (0..3).map(|mesh| ao_scene::Instance { mesh, transform: ao_scene::IDENTITY }).collect(), ..scene });
    let sun = [0.6, 0.8, 0.0];
    renderer.shadow_sun = Some(sun);
    renderer.env.sun_dir = sun;
    renderer.env.sun_color = [0.6; 3];
    renderer.env.ambient = [0.2; 3];
    renderer.env.fog_start = 100.0;
    renderer.env.fog_end = 120.0;
    let (width, height) = (512u32, 512u32);
    let targets = Targets::new(&renderer, width, height);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("soft shadow image"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let camera = Camera::look_to_up(Vec3::new(-6.0, 30.0, 0.0), -Vec3::Y, Vec3::Z);
    let capture = |renderer: &mut Renderer, settings| {
        renderer.set_enhanced_settings(settings);
        renderer.render(&view, &targets, &camera);
        texture_to_rgba(renderer, &output, width, height).unwrap()
    };
    let aspect = width as f32 / height as f32;
    let view_projection = Mat4::perspective_rh(renderer.lens.vertical_fov(aspect), aspect, renderer.lens.near,
        renderer.lens.far.unwrap_or(132.0)) * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up());
    let pixel_of = |x: f32, z: f32| {
        let p = view_projection.project_point3(Vec3::new(x, 0.0, z));
        (((p.x * 0.5 + 0.5) * width as f32) as usize, ((0.5 - p.y * 0.5) * height as f32) as usize)
    };
    let pixels_per_metre = (pixel_of(1.0, 0.0).0 as f32 - pixel_of(0.0, 0.0).0 as f32).abs();
    // Metres of ground across which the row's green channel is neither lit nor shadowed.
    let transition = |image: &[u8], z: f32, x0: f32, x1: f32| {
        let row: Vec<f32> = (pixel_of(x0, z).0.min(pixel_of(x1, z).0)..pixel_of(x0, z).0.max(pixel_of(x1, z).0)).map(|x| f32::from(image[(pixel_of(0.0, z).1 * width as usize + x) * 4 + 1])).collect();
        let (lo, hi) = row.iter().fold((255.0f32, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(hi - lo > 20.0, "row {z} has no shadow edge: {lo}..{hi}");
        row.iter().filter(|&&v| v > lo + 0.1 * (hi - lo) && v < lo + 0.9 * (hi - lo)).count() as f32 / pixels_per_metre
    };
    let shadows = EnhancedSettings { sun_shadows: true, shadow_strength: 1.0, ..Default::default() };
    let soft = capture(&mut renderer, EnhancedSettings { shadow_softness: 4.0, ..shadows });
    write_evidence(&[("soft-penumbra", &soft)], (width, height));
    let (contact, far) = (transition(&soft, -5.0, -5.0, -0.2), transition(&soft, 5.0, -12.0, -5.0));
    assert!(contact < 0.35, "edge under a 1 m awning must stay sharp: {contact} m");
    assert!(far > 3.0 * contact && far > 0.6, "edge under a 12 m awning must be much wider: {contact} m vs {far} m");
    assert_eq!(soft, capture(&mut renderer, EnhancedSettings { shadow_softness: 4.0, ..shadows }), "static soft shadows must repeat exactly");
    let hard = capture(&mut renderer, EnhancedSettings { shadow_softness: 0.0, ..shadows });
    assert!(transition(&hard, 5.0, -12.0, -5.0) < 0.35, "softness 0 keeps the hard 3x3 PCF");
    let original = capture(&mut renderer, EnhancedSettings::default());
    assert!(soft.iter().zip(&original).all(|(shadowed, lit)| shadowed <= lit), "soft shadows brightened the original image");
    // Partial sun survives full shadow: strength 0.5 is brighter than 1.0 inside the shadow but never above the original.
    let (x, y) = pixel_of(-9.0, 5.0);
    let half = capture(&mut renderer, EnhancedSettings { shadow_softness: 4.0, shadow_strength: 0.5, ..shadows });
    let at = (y * width as usize + x) * 4 + 1;
    assert!(half[at] > soft[at] && half[at] <= original[at], "shadow strength must keep part of the sun: {} {} {}", soft[at], half[at], original[at]);
}
