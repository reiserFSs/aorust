//! Real-GPU regressions for cached world/actor local-shadow faces.
use super::*;
use ao_scene::{ActorFrame, Instance, Light, Mesh, Scene, Submesh, Vertex, IDENTITY};

fn quad(x: f32, y: f32, z: f32, half_width: f32, half_height: f32) -> Mesh {
    Mesh {
        vertices: [[x - half_width, y - half_height, z], [x + half_width, y - half_height, z],
            [x + half_width, y + half_height, z], [x - half_width, y + half_height, z]]
            .into_iter().map(|pos| Vertex { pos, normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() }).collect(),
        submeshes: vec![Submesh { two_sided: true, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
    }
}

fn scene() -> Scene {
    Scene {
        meshes: vec![quad(0.0, 0.0, -8.0, 4.0, 3.0), quad(0.5, 0.0, -5.5, 0.25, 0.35)],
        instances: (0..2).map(|mesh| Instance { mesh, transform: IDENTITY }).collect(),
        lights: vec![Light { pos: [3.0, 2.0, -3.0], color: [0.5; 3], range: 24.0, ..Default::default() }],
        ..Default::default()
    }
}

fn renderer(scene: &Scene) -> Renderer {
    let gpu = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&gpu, None).expect("point cache regression GPU");
    renderer.set_msaa_samples(1);
    renderer.upload(scene);
    renderer.env.sun_color = [0.0; 3];
    renderer.env.ambient = [0.05; 3];
    renderer.env.sky_color = [0.0; 3];
    renderer.env.fog_start = 1000.0;
    renderer.env.fog_end = 1200.0;
    renderer.set_enhanced_settings(EnhancedSettings { local_light_shadows: true, ..Default::default() });
    renderer
}

struct Capture {
    targets: Targets,
    output: wgpu::Texture,
    camera: Camera,
}
impl Capture {
    fn new(renderer: &Renderer) -> Self {
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("point cache regression receiver"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { targets: Targets::new(renderer, 64, 64), output, camera: Camera::look_at(Vec3::ZERO, -Vec3::Z) }
    }
    fn frame(&self, renderer: &mut Renderer) -> Vec<u8> {
        renderer.render(&self.output.create_view(&Default::default()), &self.targets, &self.camera);
        texture_to_rgba(renderer, &self.output, 64, 64).unwrap()
    }
}

fn words(renderer: &Renderer, source: &wgpu::Buffer) -> Vec<u32> {
    let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-only point cache GPU readback"), size: source.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &readback, 0, source.size());
    renderer.queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| sender.send(result).unwrap());
    renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    bytemuck::cast_slice(&readback.slice(..).get_mapped_range()).to_vec()
}

fn scheduled(renderer: &Renderer, actor: bool) -> [bool; 6] {
    let points = renderer.point_shadows.as_ref().unwrap();
    let commands = words(renderer, &points.indirect);
    let stride = (points.casters.len() + 1) * 5;
    let first_layer = if actor { points.count * 6 } else { 0 };
    std::array::from_fn(|face| commands[(first_layer + face) * stride + 1] != 0)
}

fn caster_instances(renderer: &Renderer, actor: bool) -> u32 {
    let points = renderer.point_shadows.as_ref().unwrap();
    let commands = words(renderer, &points.indirect);
    let stride = (points.casters.len() + 1) * 5;
    let first_layer = if actor { points.count * 6 } else { 0 };
    (first_layer..first_layer + 6).flat_map(|layer| {
        let commands = &commands;
        (1..=points.casters.len()).map(move |caster| commands[layer * stride + caster * 5 + 1])
    }).sum()
}

fn metadata(renderer: &Renderer) -> Vec<u32> {
    words(renderer, &renderer.point_shadows.as_ref().unwrap().metadata)
}

#[test]
fn static_point_cache_hits_issue_no_redundant_caster_instances() {
    let mut renderer = renderer(&scene());
    let capture = Capture::new(&renderer);
    let cold = capture.frame(&mut renderer);
    assert_eq!(renderer.enhanced_settings.point_shadow_count, 4, "default cube quality changed");
    assert_eq!(renderer.point_shadows.as_ref().unwrap().count, 4);
    assert_eq!(scheduled(&renderer, false), [true; 6]);
    assert!(caster_instances(&renderer, false) > 0, "cold cache never drew real world geometry");
    let cached = metadata(&renderer);
    for face in 0..6 {
        assert_eq!(f32::from_bits(cached[128 + face * 36 + 32]), 1.0,
            "real static occlusion was not marked valid");
        assert_eq!(f32::from_bits(cached[128 + face * 36 + 35]), 0.0,
            "static-only cube still requests empty actor-depth sampling");
    }
    for _ in 0..4 {
        assert_eq!(cold, capture.frame(&mut renderer), "frozen cache hit changed the receiver");
        assert_eq!(scheduled(&renderer, false), [false; 6], "cache hit cleared cached world depth");
        assert_eq!(caster_instances(&renderer, false), 0, "cache hit still fetches world caster geometry");
        assert_eq!(cached, metadata(&renderer), "frozen capture advanced cached projections");
    }
}

#[test]
fn moving_point_light_refreshes_two_faces_round_robin_with_stored_projections() {
    let mut scene = scene();
    scene.lights.clear();
    let mut renderer = renderer(&scene);
    let mut lamp = Light { pos: [3.0, 2.0, -3.0], color: [0.5; 3], range: 24.0, ..Default::default() };
    renderer.set_dynamic_lights(&[lamp]);
    let capture = Capture::new(&renderer);
    capture.frame(&mut renderer);
    let mut previous = metadata(&renderer);
    lamp.pos[0] += 0.25;
    renderer.set_dynamic_lights(&[lamp]);
    let mut visited = [false; 6];
    for _ in 0..3 {
        capture.frame(&mut renderer);
        let updates = scheduled(&renderer, false);
        assert_eq!(updates.iter().filter(|&&updated| updated).count(), 2, "moving source exceeded or stalled its face budget");
        let current = metadata(&renderer);
        for (face, updated) in updates.into_iter().enumerate() {
            let start = 128 + face * 36;
            if updated {
                assert!(!visited[face], "round robin repeated a face before covering the cube");
                assert_ne!(previous[start..start + 16], current[start..start + 16], "scheduled face retained the old light projection");
                visited[face] = true;
            } else {
                assert_eq!(previous[start..start + 16], current[start..start + 16], "unrendered depth was paired with the new light origin");
            }
        }
        previous = current;
    }
    assert_eq!(visited, [true; 6]);
    capture.frame(&mut renderer);
    assert_eq!(scheduled(&renderer, false), [false; 6], "settled source did not become a cache hit");
}

fn actor(id: u32, x: f32) -> ActorFrame {
    ActorFrame { id, model: 7, transform: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() }
}

fn add_actors(renderer: &mut Renderer, actors: Vec<ActorFrame>) {
    renderer.add_actor_model(7, &Scene { meshes: vec![quad(0.0, 0.0, -5.5, 0.25, 0.35)], ..Default::default() });
    renderer.set_actors(actors);
}

fn patch_light(renderer: &Renderer, image: &[u8], position: Vec3) -> u32 {
    let projection = Mat4::perspective_rh(renderer.lens.vertical_fov(1.0), 1.0, renderer.lens.near, 100.0);
    let clip = projection * position.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = ((ndc.x * 0.5 + 0.5) * 64.0) as usize;
    let y = ((0.5 - ndc.y * 0.5) * 64.0) as usize;
    assert!((1..63).contains(&x) && (1..63).contains(&y));
    (y - 1..=y + 1).flat_map(|py| (x - 1..=x + 1).map(move |px| (py * 64 + px) * 4))
        .map(|pixel| image[pixel..pixel + 3].iter().map(|&value| u32::from(value)).sum::<u32>()).sum()
}

#[test]
fn actor_admission_loss_erases_history_without_erasing_other_occlusion() {
    let scene = scene();
    let mut renderer = renderer(&scene);
    add_actors(&mut renderer, vec![actor(7, 1.5), actor(8, 2.5)]);
    let capture = Capture::new(&renderer);
    let admitted = capture.frame(&mut renderer);
    assert!(caster_instances(&renderer, true) > 0, "admitted actors never populated their real depth cache");
    capture.frame(&mut renderer);
    assert_eq!(caster_instances(&renderer, true), 0, "unchanged admitted actor cache was not reused");

    // Keep the actor record alive but remove its primary-pass admission. This
    // exercises GPU membership loss, not a CPU actor-list deletion shortcut.
    let survivors = vec![actor(7, 100.0), actor(8, 2.5)];
    renderer.set_actors(survivors.clone());
    let removed = capture.frame(&mut renderer);
    assert_eq!(scheduled(&renderer, true), [true; 6], "admission loss did not clear every potentially stale actor face immediately");
    assert_eq!(scheduled(&renderer, false), [false; 6], "actor loss unnecessarily invalidated unrelated world depth");
    let surviving_metadata = metadata(&renderer);
    for face in 0..6 {
        assert_eq!(f32::from_bits(surviving_metadata[128 + face * 36 + 35]), 1.0,
            "empty-depth optimization disabled a remaining admitted actor's occlusion");
    }
    let mut fresh = self::renderer(&scene);
    add_actors(&mut fresh, survivors);
    let fresh_capture = Capture::new(&fresh);
    assert_eq!(removed, fresh_capture.frame(&mut fresh), "GPU-unadmitted actor left retained shadow history");
    renderer.set_actors(vec![actor(7, 200.0), actor(8, 2.5)]);
    assert_eq!(removed, capture.frame(&mut renderer), "moving an already unadmitted actor changed visible shadow output");
    assert_eq!(scheduled(&renderer, true), [false; 6], "hidden actor pose dirtied admitted actor history");
    assert_eq!(scheduled(&renderer, false), [false; 6], "hidden actor pose dirtied world history");

    renderer.set_enhanced_settings(EnhancedSettings::default());
    let unshadowed = capture.frame(&mut renderer);
    for (name, x) in [("static world", -2.0), ("remaining admitted actor", 2.0)] {
        let position = Vec3::new(x, -2.0, -8.0);
        assert!(patch_light(&renderer, &removed, position) + 9 * 3 * 8 < patch_light(&renderer, &unshadowed, position),
            "{name} occlusion vanished when a different actor lost admission");
    }
    let lost_shadow = Vec3::new(0.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &admitted, lost_shadow) + 9 * 3 * 8 < patch_light(&renderer, &removed, lost_shadow),
        "admission-loss fixture never exercised a positive disappearing actor shadow");
}

#[test]
fn changed_world_caster_invalidates_cached_depth_with_bounded_face_updates() {
    let mut renderer = renderer(&scene());
    let capture = Capture::new(&renderer);
    let before = capture.frame(&mut renderer);
    renderer.gpu.insts[1].m = Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0)).to_cols_array_2d();
    let mut visited = [false; 6];
    for _ in 0..3 {
        capture.frame(&mut renderer);
        let updates = scheduled(&renderer, false);
        assert_eq!(updates.iter().filter(|&&updated| updated).count(), 2,
            "world transform invalidation did not respect the two-face budget");
        for (face, updated) in updates.into_iter().enumerate() {
            if updated {
                assert!(!visited[face], "world invalidation revisited a face before covering the cube");
                visited[face] = true;
            }
        }
    }
    assert_eq!(visited, [true; 6]);
    let after = capture.frame(&mut renderer);
    assert_eq!(scheduled(&renderer, false), [false; 6]);
    let old_shadow = Vec3::new(-2.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &before, old_shadow) + 9 * 3 * 8 < patch_light(&renderer, &after, old_shadow),
        "changed world transform left the old cached occluder depth");
    let new_shadow = Vec3::new(0.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &after, new_shadow) + 9 * 3 * 8 < patch_light(&renderer, &before, new_shadow),
        "invalidated world caster was not rasterized at its new position");
}

fn negative_z_depth(renderer: &Renderer) -> Vec<f32> {
    negative_z_layer_depth(renderer, false)
}

fn negative_z_layer_depth(renderer: &Renderer, actor: bool) -> Vec<f32> {
    let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-only adaptive point depth readback"), size: 256 * 256 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    let points = renderer.point_shadows.as_ref().unwrap();
    let texture = if actor { &points._actor_maps } else { &points._maps };
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture, mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: 5 }, aspect: wgpu::TextureAspect::DepthOnly,
        },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout {
            offset: 0, bytes_per_row: Some(256 * 4), rows_per_image: Some(256),
        } },
        wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
    );
    renderer.queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| sender.send(result).unwrap());
    renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    bytemuck::cast_slice(&readback.slice(..).get_mapped_range()).to_vec()
}

#[test]
fn distant_point_faces_really_rasterize_at_128_and_64_not_256() {
    let scene = Scene {
        meshes: vec![quad(0.0, 0.0, -8.0, 32.0, 32.0)],
        instances: vec![Instance { mesh: 0, transform: IDENTITY }],
        lights: vec![Light { pos: [0.0; 3], color: [0.5; 3], range: 100.0, ..Default::default() }],
        ..Default::default()
    };
    let mut renderer = renderer(&scene);
    let mut capture = Capture::new(&renderer);
    for (distance, size) in [(10.0, 256usize), (50.0, 128), (100.0, 64)] {
        capture.camera = Camera::look_at(Vec3::new(0.0, 0.0, distance), Vec3::new(0.0, 0.0, -8.0));
        // Size changes use the same bounded three-frame face refresh as movement.
        for _ in 0..3 { capture.frame(&mut renderer); }
        let metadata = metadata(&renderer);
        for face in 0..6 {
            assert_eq!(f32::from_bits(metadata[128 + face * 36 + 33]), size as f32 / 256.0);
        }
        let depth = negative_z_depth(&renderer);
        let occupied = depth.iter().filter(|&&value| value > 0.0 && value < 1.0).count();
        assert_eq!(occupied, size * size, "adaptive metadata did not match the actual GPU raster footprint");
        for y in 0..256 {
            for x in 0..256 {
                let value = depth[y * 256 + x];
                if x < size && y < size {
                    assert!(value > 0.0 && value < 1.0, "effective adaptive face contains a depth hole");
                } else {
                    assert_eq!(value, 1.0, "reduced face retained depth outside its bounded quadrant");
                }
            }
        }
    }
}

#[test]
fn moving_admitted_actor_uses_two_faces_without_invalidating_static_depth() {
    let mut renderer = renderer(&scene());
    add_actors(&mut renderer, vec![actor(7, 1.5)]);
    let capture = Capture::new(&renderer);
    let before = capture.frame(&mut renderer);
    renderer.set_actors(vec![actor(7, 2.0)]);
    let mut visited = [false; 6];
    for _ in 0..3 {
        capture.frame(&mut renderer);
        let updates = scheduled(&renderer, true);
        assert_eq!(updates.iter().filter(|&&updated| updated).count(), 2,
            "admitted pose change exceeded or stalled the normal actor face budget");
        assert_eq!(scheduled(&renderer, false), [false; 6], "actor movement invalidated static world depth");
        for (face, updated) in updates.into_iter().enumerate() {
            if updated {
                assert!(!visited[face], "actor round robin repeated a face before covering the cube");
                visited[face] = true;
            }
        }
    }
    assert_eq!(visited, [true; 6]);
    let after = capture.frame(&mut renderer);
    assert_eq!(scheduled(&renderer, true), [false; 6]);
    assert_eq!(caster_instances(&renderer, true), 0);
    let old_shadow = Vec3::new(0.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &before, old_shadow) + 9 * 3 * 8 < patch_light(&renderer, &after, old_shadow),
        "actor movement left its old occluder in the cached depth");
    let new_shadow = Vec3::new(1.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &after, new_shadow) + 9 * 3 * 8 < patch_light(&renderer, &before, new_shadow),
        "actor movement failed to rasterize current admitted pose");
}

#[test]
fn hidden_actor_insertion_cannot_reset_visible_actor_round_robin() {
    let scene = scene();
    let mut baseline = renderer(&scene);
    let mut inserted = renderer(&scene);
    add_actors(&mut baseline, vec![actor(7, 1.5)]);
    add_actors(&mut inserted, vec![actor(7, 1.5)]);
    let baseline_capture = Capture::new(&baseline);
    let inserted_capture = Capture::new(&inserted);
    assert_eq!(baseline_capture.frame(&mut baseline), inserted_capture.frame(&mut inserted));

    // The hidden actor comes first deliberately: dense actor indices and GPU
    // proof groups change, but the admitted actor's identity and pose do not.
    let hidden = actor(99, 100.0);
    inserted.set_actors(vec![hidden.clone(), actor(7, 1.5)]);
    assert_eq!(baseline_capture.frame(&mut baseline), inserted_capture.frame(&mut inserted),
        "hidden insertion changed the visible receiver before pose movement");
    assert_eq!(scheduled(&inserted, true), [false; 6],
        "proof-group remapping reset an unchanged visible actor cache");
    assert_eq!(caster_instances(&inserted, true), 0);

    baseline.set_actors(vec![actor(7, 2.0)]);
    inserted.set_actors(vec![hidden, actor(7, 2.0)]);
    let mut visited = [false; 6];
    for _ in 0..3 {
        let expected = baseline_capture.frame(&mut baseline);
        let actual = inserted_capture.frame(&mut inserted);
        assert_eq!(expected, actual, "hidden insertion changed a moving admitted actor's shadow");
        let updates = scheduled(&baseline, true);
        assert_eq!(updates.iter().filter(|&&updated| updated).count(), 2);
        assert_eq!(scheduled(&inserted, true), updates,
            "hidden insertion changed visible actor round-robin scheduling");
        assert_eq!(scheduled(&inserted, false), [false; 6]);
        assert_eq!(caster_instances(&inserted, true), caster_instances(&baseline, true),
            "hidden geometry changed admitted caster instance counts");
        for (face, updated) in updates.into_iter().enumerate() {
            if updated {
                assert!(!visited[face], "hidden-insertion fixture did not cover a complete round robin");
                visited[face] = true;
            }
        }
    }
    assert_eq!(visited, [true; 6]);
    assert_eq!(baseline_capture.frame(&mut baseline), inserted_capture.frame(&mut inserted));
    assert_eq!(scheduled(&inserted, true), [false; 6]);
}

#[test]
fn advancing_live_frames_omit_static_passes_despite_actor_motion_and_lamp_rgb_changes() {
    let mut renderer = renderer(&scene());
    add_actors(&mut renderer, vec![actor(7, 1.5)]);
    let capture = Capture::new(&renderer);
    // Ordinary unanimated opaque material and an unowned, fixed-position world
    // lamp: actor admission and positive RGB cannot change its static depth.
    for frame in 0..4 {
        renderer.time += 1.0 / 60.0;
        capture.frame(&mut renderer);
        if frame == 0 {
            assert_eq!(renderer.point_shadows.as_ref().unwrap().static_face_mask, 63,
                "cold cache did not encode all six bootstrap world faces");
            assert!(caster_instances(&renderer, false) > 0,
                "live dispatch fixture never rasterized real static caster geometry");
        }
    }
    assert_eq!(renderer.point_shadows.as_ref().unwrap().static_face_mask, 0,
        "settled live world still encodes static depth passes");
    let mut low_light = 0;
    let mut high_light = 0;
    for frame in 0..12 {
        renderer.time += 1.0 / 60.0;
        renderer.set_actors(vec![actor(7, 1.5 + (frame % 4) as f32 * 0.1)]);
        let intensity = if frame % 2 == 0 { 0.15 } else { 0.65 };
        renderer.light_base[1][..3].fill(intensity);
        let image = capture.frame(&mut renderer);
        assert_eq!(renderer.point_shadows.as_ref().unwrap().static_face_mask, 0,
            "advancing time, unrelated actor motion or positive lamp RGB encoded cached static faces");
        assert_eq!(scheduled(&renderer, false), [false; 6],
            "GPU marked omitted static faces as newly rendered");
        assert_eq!(caster_instances(&renderer, false), 0);
        if frame > 0 {
            assert_eq!(scheduled(&renderer, true).into_iter().filter(|&updated| updated).count(), 2,
                "live admitted actor movement did not retain the normal two-face budget");
        }
        let brightness = patch_light(&renderer, &image, Vec3::new(-3.0, 1.0, -8.0));
        if frame % 2 == 0 { low_light = brightness; } else { high_light = brightness; }
    }
    assert!(high_light > low_light + 9 * 3 * 8,
        "lamp RGB changes were ignored instead of changing real receiver illumination");
}

#[test]
fn native_push_vegetation_uses_current_dynamic_depth_on_every_frozen_frame() {
    let mut scene = scene();
    scene.meshes[1] = quad(0.5, 0.0, -5.5, 0.25, 1.0);
    scene.meshes[1].submeshes[0].vegetation = true;
    let mut renderer = renderer(&scene);
    renderer.set_enhanced_settings(EnhancedSettings {
        local_light_shadows: true, foliage_push: true, ..Default::default()
    });
    renderer.set_enhanced_grass_inputs(&mut vec![GrassBender {
        actor: None, position: [0.0, 0.0, -5.2],
    }], [0.0; 3]);
    let capture = Capture::new(&renderer);
    let frozen_time = renderer.time;
    for frame in 0..5 {
        capture.frame(&mut renderer);
        assert_eq!(renderer.time, frozen_time);
        assert_eq!(scheduled(&renderer, true), [true; 6],
            "native push vegetation reused private dynamic depth on a frozen frame");
        let points = renderer.point_shadows.as_ref().unwrap();
        let vegetation = points.casters.iter().position(|caster| caster.key[3] == 2)
            .expect("native push vegetation was not separated into a dynamic-world caster");
        assert!(vegetation >= points.static_caster_count,
            "native push vegetation remains inside the actual CPU static dispatch partition");
        let commands = words(&renderer, &points.indirect);
        let stride = (points.casters.len() + 1) * 5;
        for face in 0..6 {
            assert_eq!(commands[face * stride + (vegetation + 1) * 5 + 1], 0,
                "native push vegetation still contributes to cached static depth");
        }
        assert!(caster_instances(&renderer, true) > 0,
            "current dynamic depth never rasterized the native vegetation");
        if frame >= 3 {
            assert_eq!(points.static_face_mask, 0,
                "native push vegetation prevented unrelated static faces from settling");
        }
    }
    let bent = negative_z_layer_depth(&renderer, true);
    assert!(bent.iter().any(|&depth| depth > 0.0 && depth < 1.0),
        "push vegetation fixture produced no real dynamic occlusion");

    // Clear the native current contact input without advancing effect time.
    // Retaining last frame's private depth would preserve the old bent card.
    renderer.set_enhanced_grass_inputs(&mut Vec::new(), [0.0; 3]);
    capture.frame(&mut renderer);
    assert_eq!(renderer.time, frozen_time);
    assert_eq!(renderer.point_shadows.as_ref().unwrap().static_face_mask, 0);
    assert_eq!(scheduled(&renderer, true), [true; 6]);
    let unbent = negative_z_layer_depth(&renderer, true);
    assert!(bent.iter().zip(&unbent).filter(|(before, after)| before != after).count() > 16,
        "removing the native push input left stale bent vegetation in dynamic depth");
    capture.frame(&mut renderer);
    assert_eq!(scheduled(&renderer, true), [true; 6]);
    assert_eq!(unbent, negative_z_layer_depth(&renderer, true),
        "frozen unbent vegetation did not redraw deterministically");
}

fn shader_function(source: &str, name: &str) -> String {
    let start = source.find(&format!("fn {name}(")).unwrap();
    let end = start + source[start..].find("\n}").unwrap() + 2;
    source[start..end].to_owned()
}

#[test]
fn native_ordered_csr_matches_full_scan_on_grid_edges_and_float_range_boundary() {
    use wgpu::util::DeviceExt;
    let renderer = renderer(&scene());
    let device = &renderer.device;
    let producer_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("production admitted CSR producer regression"),
        source: wgpu::ShaderSource::Wgsl(include_str!("clustered_build.wgsl").into()),
    });
    let producer = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("production admitted CSR producer regression"), layout: None,
        module: &producer_shader, entry_point: Some("build"), compilation_options: Default::default(), cache: None,
    });
    let mut native_source = include_str!("shader.wgsl").to_owned();
    admitted_native_pool(&mut native_source, "vertex_lights");
    let native = shader_function(&native_source, "vertex_lights");
    assert!(native.contains("let k = bitcast<u32>(dyn_lights[local_list.x + candidate].x);"));
    let full_scan = native.replace("fn vertex_lights(", "fn full_scan_lights(")
        .replace("    let local_list = admitted_light_list(p);\n", "")
        .replace("for (var candidate = 0u; candidate < min(dyn_count, local_list.y) && used < 8u; candidate += 1u) {\n        let k = bitcast<u32>(dyn_lights[local_list.x + candidate].x);",
            "for (var k = 0u; k < dyn_count && used < 8u; k += 1u) {")
        .replace("        if dot(d, d) > a.w * a.w * 1.000001 { continue; }\n", "");
    assert!(!full_scan.contains("local_list"), "full-scan oracle still depends on CSR");
    let list = shader_function(include_str!("point_shadows.wgsl"), "admitted_light_list");
    let owner = shader_function(include_str!("point_shadows.wgsl"), "owner_light_admitted");
    let query_source = format!(r#"
@group(0) @binding(0) var<storage,read> dyn_lights:array<vec4<f32>>;
@group(0) @binding(1) var<storage,read> point_actor_visibility:array<u32>;
@group(0) @binding(2) var<storage,read> queries:array<vec4<f32>>;
@group(0) @binding(3) var<storage,read_write> results:array<u32>;
struct Globals {{ eye:vec4<f32>, grid:vec4<f32>, dyn_info:vec4<f32>, directional:array<vec4<f32>,16>, }}
var<private> g:Globals;
{list}
{owner}
{native}
{full_scan}
fn budget_mask(p:vec3<f32>, directional:u32, spatial:bool)->u32 {{
    let local=admitted_light_list(p);
    let count=min(bitcast<u32>(dyn_lights[322u].x),64u);
    var used=directional+select(0u,1u,directional>0u);
    var mask=0u;
    for(var candidate=0u;candidate<count && used<8u;candidate++) {{
        var k=candidate;
        if spatial {{
            if candidate>=local.y {{ break; }}
            k=bitcast<u32>(dyn_lights[local.x+candidate].x);
        }}
        let li=66u+k*4u;
        let light=dyn_lights[li];
        if !owner_light_admitted(dyn_lights[li+3u].w) || light.w<=0.0 {{ continue; }}
        if length(light.xyz-p)>light.w {{ continue; }}
        used++;
        mask|=1u<<k;
    }}
    return mask;
}}
@compute @workgroup_size(1)
fn probe(@builtin(global_invocation_id) id:vec3<u32>) {{
    let p=queries[id.x*2u].xyz;
    let directional=u32(queries[id.x*2u].w);
    let n=queries[id.x*2u+1u].xyz;
    g.eye=vec4<f32>(p+n*5.0,1.0);
    g.grid=vec4<f32>(0.0,0.0,0.0,1.0);
    g.dyn_info.y=f32(directional);
    let local=admitted_light_list(p);
    var ordered=1u;
    for(var i=1u;i<local.y;i++) {{
        if bitcast<u32>(dyn_lights[local.x+i-1u].x)>=bitcast<u32>(dyn_lights[local.x+i].x) {{ ordered=0u; }}
    }}
    let actual=vertex_lights(p,n,4.0);
    let expected=full_scan_lights(p,n,4.0);
    let base=id.x*20u;
    results[base]=local.y;
    results[base+1u]=ordered;
    results[base+2u]=budget_mask(p,directional,true);
    results[base+3u]=budget_mask(p,directional,false);
    for(var lane=0u;lane<3u;lane++) {{
        results[base+4u+lane]=bitcast<u32>(actual[0][lane]);
        results[base+8u+lane]=bitcast<u32>(actual[1][lane]);
        results[base+12u+lane]=bitcast<u32>(expected[0][lane]);
        results[base+16u+lane]=bitcast<u32>(expected[1][lane]);
    }}
}}
"#);
    let query_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native CSR and original flat scan GPU oracle"), source: wgpu::ShaderSource::Wgsl(query_source.into()),
    });
    let query_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("native ordered CSR edge regression"), layout: None, module: &query_shader,
        entry_point: Some("probe"), compilation_options: Default::default(), cache: None,
    });
    let owners = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("CSR native owner proof"), contents: bytemuck::cast_slice(&[0u32, 1]),
        usage: wgpu::BufferUsages::STORAGE,
    });
    for (center, range, remote) in [
        (Vec3::ZERO, 4.0, None),
        (Vec3::new(-128.0, 0.0, 0.0), 128.0, None),
        (Vec3::ZERO, 1.0, Some(Vec3::new(-134_217_472.0, 0.0, 0.0))),
    ] {
        let mut input = vec![[0.0f32; 4]; 2];
        let source_count = 12 + u32::from(remote.is_some());
        input[0][0] = f32::from_bits(source_count);
        input[1] = center.extend(0.0).to_array();
        for index in 0..12u32 {
            let value = (index + 1) as f32;
            input.extend_from_slice(&[
                center.extend(range).to_array(),
                [value * 0.01, value * value * 0.002, value * 0.005, 2.0],
                [1.0, 0.0, 0.0, 0.0], // Positive constant attenuation remains nonzero exactly at range.
                [0.0; 4], [0.0, f32::from_bits(index), 0.0, 0.0],
            ]);
        }
        if let Some(position) = remote {
            input.extend_from_slice(&[
                position.extend(1.0).to_array(), [0.2, 0.1, 0.05, 2.0],
                [1.0, 0.0, 0.0, 0.0], [0.0; 4], [0.0, f32::from_bits(12), 0.0, 0.0],
            ]);
        }
        let candidates = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ordered CSR native light candidates"), contents: bytemuck::cast_slice(&input),
            usage: wgpu::BufferUsages::STORAGE,
        });
        // Production ABI: 387 prefix/source rows, <=512 cells, <=64 indices/cell.
        let pool = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("real GPU-produced ordered CSR"), size: (387 + 512 + 512 * 64) * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
        });
        let producer_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("real CSR producer bindings"), layout: &producer.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: pool.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: candidates.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: owners.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&producer);
            pass.set_bind_group(0, &producer_bind, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        renderer.queue.submit([encoder.finish()]);
        let built = words(&renderer, &pool);
        assert_eq!(built[322 * 4], source_count, "ordered CSR fixture did not retain every source");
        let lo = Vec3::new(f32::from_bits(built[64 * 4]), f32::from_bits(built[64 * 4 + 1]), f32::from_bits(built[64 * 4 + 2]));
        let cell = f32::from_bits(built[64 * 4 + 3]);
        let dims = Vec3::new(built[65 * 4] as f32, built[65 * 4 + 1] as f32, built[65 * 4 + 2] as f32);
        let upper = lo + dims * cell;
        let mut points = vec![(center + Vec3::Y * range * 0.25, false)];
        for axis in 0..3 {
            let outside_step = (cell * 1e-4).max(lo[axis].abs().max(upper[axis].abs()) * f32::EPSILON * 2.0);
            for (value, upper_edge) in [(lo[axis], false), (upper[axis], true), (lo[axis] - outside_step, false), (upper[axis] + outside_step, false),
                (center[axis] - range, false), (center[axis] + range, false)] {
                let mut point = center;
                point[axis] = value;
                points.push((point, upper_edge));
            }
        }
        if center.x == -128.0 {
            // Native subtraction rounds this distance to exactly128, despite
            // world.x exceeding the unpadded producer's raw maximum of zero.
            assert_eq!((1e-6f32 - center.x).to_bits(), range.to_bits());
            points.push((Vec3::new(1e-6, 0.0, 0.0), false));
        }
        if remote.is_some() {
            // Reconstructing upper from a distant origin previously rounded it
            // to zero and culled the near lamp's positive native contribution.
            points.push((Vec3::new(0.5, 0.0, 0.0), false));
            points.push((Vec3::new(f32::from_bits(0.5f32.to_bits() + 1), 0.0, 0.0), false));
        }
        let mut queries = Vec::new();
        for &(point, _) in &points {
            for directional in [0.0, 2.0] {
                queries.push(point.extend(directional).to_array());
                queries.push((center - point).normalize().extend(0.0).to_array());
            }
        }
        let query_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native CSR exact boundary queries"), contents: bytemuck::cast_slice(&queries),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native CSR full-scan numeric comparisons"), size: (queries.len() / 2 * 20 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
        });
        let query_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native CSR query bindings"), layout: &query_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: pool.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: owners.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: query_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: output.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&query_pipeline);
            pass.set_bind_group(0, &query_bind, &[]);
            pass.dispatch_workgroups((queries.len() / 2) as u32, 1, 1);
        }
        renderer.queue.submit([encoder.finish()]);
        let results = words(&renderer, &output);
        for (query, result) in results.as_chunks::<20>().0.iter().enumerate() {
            let (point, upper_edge) = points[query / 2];
            assert_eq!(result[1], 1, "CSR candidates are not strictly ascending at {point:?}");
            assert_eq!(result[2], result[3], "CSR changed native-eight light selection at {point:?}");
            assert_eq!(&result[4..7], &result[12..15], "CSR changed native diffuse bytes at {point:?}");
            assert_eq!(&result[8..11], &result[16..19], "CSR changed native specular bytes at {point:?}");
            if point.cmplt(lo).any() || point.cmpgt(upper).any() {
                assert_eq!(result[0], 0, "query outside the conservative grid did not return an empty list");
            }
            if upper_edge {
                assert!(result[0] > 0, "inclusive upper grid boundary incorrectly rejected its touching cell");
            }
        }
        assert_eq!(results[2], 0xff, "native eight-light budget positive control did not consume exactly the first eight");
        assert_eq!(results[22], 0x1f, "sun plus two directionals did not leave exactly five native local slots");
        assert!(f32::from_bits(results[4]) > 0.0, "numeric native light positive control is dark");
        if center.x == -128.0 {
            let exact = (points.len() - 1) * 2 * 20;
            assert_eq!(results[exact + 2], 0xff, "rounded native range endpoint lost its eight contributing sources");
            assert!(f32::from_bits(results[exact + 4]) > 0.0, "rounded range endpoint lost positive coefficient attenuation");
        }
        if remote.is_some() {
            for point in points.len() - 2..points.len() {
                let offset = point * 2 * 20;
                assert_eq!(results[offset + 2], 0xff, "distant grid-origin cancellation discarded a contributing near lamp");
                assert!(f32::from_bits(results[offset + 4]) > 0.0,
                    "near lamp lost positive illumination at0.5 or its adjacent representable float");
            }
        }
    }
}

#[test]
fn empty_actor_frames_encode_no_dynamic_passes_and_arrival_loss_remain_exact() {
    let mut renderer = renderer(&scene());
    let capture = Capture::new(&renderer);
    let mut empty = Vec::new();
    for _ in 0..8 {
        renderer.time += 1.0 / 60.0;
        empty = capture.frame(&mut renderer);
        assert_eq!(renderer.point_shadows.as_ref().unwrap().actor_face_mask, 0,
            "empty live actor layer still encodes depth passes");
        let metadata = metadata(&renderer);
        for face in 0..6 {
            assert_eq!(f32::from_bits(metadata[128 + face * 36 + 35]), 0.0,
                "omitted empty actor layer remains sampleable");
        }
    }
    add_actors(&mut renderer, vec![actor(7, 1.5)]);
    renderer.time += 1.0 / 60.0;
    let admitted = capture.frame(&mut renderer);
    assert_eq!(renderer.point_shadows.as_ref().unwrap().actor_face_mask, 63,
        "first current actor did not enable the actual CPU dynamic passes");
    assert_eq!(scheduled(&renderer, true), [true; 6],
        "actor arrival left previously omitted dynamic faces uninitialized");
    assert!(caster_instances(&renderer, true) > 0,
        "actor-arrival positive control did not rasterize real caster geometry");
    let actor_shadow = Vec3::new(0.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &admitted, actor_shadow) + 9 * 3 * 8 < patch_light(&renderer, &empty, actor_shadow),
        "first admitted actor did not cast positive occlusion after empty dispatch frames");

    renderer.set_actors(Vec::new());
    renderer.time += 1.0 / 60.0;
    let departed = capture.frame(&mut renderer);
    assert_eq!(renderer.point_shadows.as_ref().unwrap().actor_face_mask, 63,
        "last actor departure skipped the mandatory one-time depth clear");
    assert_eq!(scheduled(&renderer, true), [true; 6],
        "last actor departure did not clear every retained dynamic face immediately");
    assert_eq!(caster_instances(&renderer, true), 0);
    assert_eq!(empty, departed, "last actor departure retained negative shadow history");
    assert!(negative_z_layer_depth(&renderer, true).iter().all(|&depth| depth == 1.0),
        "one-time departure clear retained old actor depth texels");
    for _ in 0..4 {
        renderer.time += 1.0 / 60.0;
        assert_eq!(departed, capture.frame(&mut renderer));
        assert_eq!(renderer.point_shadows.as_ref().unwrap().actor_face_mask, 0,
            "empty live actor layer did not return to zero CPU passes after departure clear");
        let metadata = metadata(&renderer);
        for face in 0..6 {
            assert_eq!(f32::from_bits(metadata[128 + face * 36 + 35]), 0.0,
                "departed actor's empty depth layer was re-enabled");
        }
    }
    renderer.set_enhanced_settings(EnhancedSettings::default());
    let unshadowed = capture.frame(&mut renderer);
    let static_shadow = Vec3::new(-2.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &departed, static_shadow) + 9 * 3 * 8 < patch_light(&renderer, &unshadowed, static_shadow),
        "omitting the empty actor layer also erased unrelated static world occlusion");
}

fn scrolling_actor_model(blend: ao_scene::Blend) -> Scene {
    let key = ao_scene::TextureKey { rdb_type: 0, id: 719 };
    let mut mesh = quad(0.0, 0.0, -5.5, 0.25, 0.35);
    mesh.submeshes[0].texture = Some(key);
    mesh.submeshes[0].blend = blend;
    mesh.submeshes[0].uv_scroll = [0.5, 0.0];
    let mut scene = Scene { meshes: vec![mesh], ..Default::default() };
    scene.textures.insert(key, ao_scene::Texture {
        width: 2, height: 1, rgba: vec![255, 32, 16, 255, 16, 64, 255, 0],
        source: ao_scene::TextureSource::Generated,
    });
    scene
}

#[test]
fn carried_opaque_rgb_scroll_and_effect_time_settle_to_zero_world_passes() {
    let mut scene = scene();
    let mut lamp = scene.lights.remove(0);
    lamp.owner = Some(7);
    let mut renderer = renderer(&scene);
    renderer.set_enhanced_settings(EnhancedSettings {
        local_light_shadows: true, point_shadow_count: 1, ..Default::default()
    });
    renderer.add_actor_model(7, &scrolling_actor_model(ao_scene::Blend::Opaque));
    renderer.set_actors(vec![actor(7, 1.5)]);
    renderer.set_dynamic_lights(&[lamp]);
    let capture = Capture::new(&renderer);
    capture.frame(&mut renderer);
    assert!(caster_instances(&renderer, false) > 0, "owned source never drew real world depth");
    for _ in 0..32 {
        renderer.time += 1.0 / 60.0;
        capture.frame(&mut renderer);
    }
    let mut shadowed = Vec::new();
    for frame in 0..12 {
        renderer.time += 1.0 / 60.0;
        let rgb = if frame % 2 == 0 { [0.05; 3] } else { [0.2; 3] };
        let mut owner = actor(7, 1.5);
        owner.rendering_effect_time = renderer.time;
        owner.emissive = Some(rgb);
        owner.specular = Some(rgb);
        owner.specular_power = Some(4.0 + frame as f32);
        owner.part_materials = vec![ao_scene::ActorMaterialOverride {
            emissive: Some(rgb), specular: Some(rgb), ..Default::default()
        }];
        renderer.set_actors(vec![owner]);
        lamp.color = if frame % 2 == 0 { [0.35; 3] } else { [0.5; 3] };
        renderer.set_dynamic_lights(&[lamp]);
        shadowed = capture.frame(&mut renderer);
        assert_eq!(renderer.point_shadows.as_ref().unwrap().static_face_mask, 0,
            "opaque RGB-only UV drift, positive overrides or an unused effect timer encoded static depth passes");
        assert_eq!(scheduled(&renderer, false), [false; 6]);
        assert_eq!(caster_instances(&renderer, false), 0,
            "settled carried source still submitted real world geometry");
    }
    assert!(metadata(&renderer)[128..].as_chunks::<36>().0.iter().take(6)
        .all(|face| f32::from_bits(face[32]) == 1.0),
        "zero-work owned fixture was not actually admitted");
    renderer.set_enhanced_settings(EnhancedSettings::default());
    let unshadowed = capture.frame(&mut renderer);
    let static_shadow = Vec3::new(-2.0, -2.0, -8.0);
    assert!(patch_light(&renderer, &shadowed, static_shadow) + 9 * 3 * 8
        < patch_light(&renderer, &unshadowed, static_shadow),
        "zero carried world passes erased positive static occlusion");
}

#[test]
fn primary_alpha_scroll_and_uv_mutations_invalidate_owner_admission_potential() {
    let mut renderer = renderer(&scene());
    renderer.add_actor_model(7, &scrolling_actor_model(ao_scene::Blend::AlphaTest));
    renderer.set_actors(vec![actor(7, 1.5)]);
    let capture = Capture::new(&renderer);
    capture.frame(&mut renderer);
    let capsules = [[0.0, 0.0, 0.0, f32::INFINITY]];
    let initial = renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules);
    renderer.time += 0.25;
    assert_ne!(initial, renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules),
        "actual alpha-tested scrolling texture no longer invalidates current primary admission potential");
    let scrolled = renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules);
    let mut changed = actor(7, 1.5);
    changed.part_uvs = vec![[1.0, 1.0, 0.5, 0.0]];
    renderer.set_actors(vec![changed.clone()]);
    assert_ne!(scrolled, renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules),
        "alpha coverage UV mutation was omitted from primary admission dependencies");
    let remapped = renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules);
    changed.part_materials = vec![ao_scene::ActorMaterialOverride {
        alpha: Some(0.25), ..Default::default()
    }];
    renderer.set_actors(vec![changed]);
    assert_ne!(remapped, renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules),
        "primary alpha override mutation was mistaken for an RGB-only material change");
    let mut glow = scrolling_actor_model(ao_scene::Blend::AlphaTest);
    glow.meshes[0].submeshes[0].glow_mask = true;
    renderer.add_actor_model(7, &glow);
    renderer.set_actors(vec![actor(7, 1.5)]);
    capture.frame(&mut renderer);
    let constant_alpha = renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules);
    renderer.time += 0.25;
    assert_eq!(constant_alpha, renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules),
        "glow-mask texture alpha used only as RGB illumination dirtied primary coverage");
    let mut skinned = actor(7, 1.5);
    let mut vertices = glow.meshes[0].vertices.clone();
    vertices[0].pos[0] += 0.125;
    skinned.skin = Some(std::sync::Arc::new(vertices.clone()));
    renderer.set_actors(vec![skinned]);
    let retained = renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules);
    renderer.set_actors(vec![actor(7, 1.5)]);
    assert_eq!(retained, renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules),
        "submitting None forgot the actual retained GPU skin used by primary admission");
    vertices[0].pos[0] += 0.125;
    let mut reposed = actor(7, 1.5);
    reposed.skin = Some(std::sync::Arc::new(vertices));
    renderer.set_actors(vec![reposed]);
    assert_ne!(retained, renderer.point_owner_occluder_hash(Vec3::ZERO, &capsules),
        "actual retained GPU skin mutation failed to invalidate primary admission potential");
}

#[test]
fn carried_point_faces_rasterize_at_128_near_and_64_beyond_fifteen_metres() {
    let scene = Scene {
        meshes: vec![quad(0.0, 0.0, -8.0, 32.0, 32.0)],
        instances: vec![Instance { mesh: 0, transform: IDENTITY }],
        lights: vec![Light {
            pos: [0.0; 3], color: [0.5; 3], range: 100.0, owner: Some(7), ..Default::default()
        }],
        ..Default::default()
    };
    let mut renderer = renderer(&scene);
    renderer.set_enhanced_settings(EnhancedSettings {
        local_light_shadows: true, point_shadow_count: 1, ..Default::default()
    });
    renderer.add_actor_model(7, &Scene {
        meshes: vec![quad(0.0, 0.0, -5.5, 1.0, 1.0)], ..Default::default()
    });
    renderer.set_actors(vec![actor(7, 0.0)]);
    let mut capture = Capture::new(&renderer);
    for (distance, size) in [(0.0, 128usize), (20.0, 64)] {
        capture.camera = Camera::look_at(Vec3::new(0.0, 0.0, distance), Vec3::new(0.0, 0.0, -8.0));
        for _ in 0..6 { capture.frame(&mut renderer); }
        let state = metadata(&renderer);
        for face in 0..6 {
            assert_eq!(f32::from_bits(state[128 + face * 36 + 32]), 1.0,
                "adaptive carried light was not admitted by actual primary geometry");
            assert_eq!(f32::from_bits(state[128 + face * 36 + 33]), size as f32 / 256.0);
        }
        let depth = negative_z_depth(&renderer);
        assert_eq!(depth.iter().filter(|&&value| value > 0.0 && value < 1.0).count(), size * size,
            "carried face metadata did not match actual world raster work");
        for y in 0..256 {
            for x in 0..256 {
                let value = depth[y * 256 + x];
                if x < size && y < size {
                    assert!(value > 0.0 && value < 1.0, "carried effective face contains a depth hole");
                } else {
                    assert_eq!(value, 1.0, "carried reduced face retained depth outside its viewport");
                }
            }
        }
    }
}

#[test]
fn carried_motion_accumulates_from_last_trigger_then_refreshes_two_stored_faces() {
    let mut scene = scene();
    let mut lamp = scene.lights.remove(0);
    lamp.owner = Some(7);
    let mut renderer = renderer(&scene);
    renderer.set_enhanced_settings(EnhancedSettings {
        local_light_shadows: true, point_shadow_count: 1, ..Default::default()
    });
    add_actors(&mut renderer, vec![actor(7, 1.5)]);
    renderer.set_dynamic_lights(&[lamp]);
    let capture = Capture::new(&renderer);
    for _ in 0..32 {
        renderer.time += 1.0 / 60.0;
        capture.frame(&mut renderer);
    }
    assert!(negative_z_depth(&renderer).iter().any(|&depth| depth > 0.0 && depth < 1.0),
        "carried movement fixture never populated actual world depth");
    let origin = lamp.pos[0];
    let mut previous = metadata(&renderer);
    for trigger in [0.0, 0.21] {
        // Every individual step is below 0.2m, including the triggering step.
        // Resetting the baseline per frame would therefore never refresh.
        for offset in [0.05, 0.10, 0.15, 0.19] {
            lamp.pos[0] = origin + trigger + offset;
            renderer.set_dynamic_lights(&[lamp]);
            renderer.time += 1.0 / 60.0;
            capture.frame(&mut renderer);
            assert_eq!(scheduled(&renderer, false), [false; 6],
                "carried world cache refreshed before cumulative motion exceeded 0.2m");
            assert_eq!(caster_instances(&renderer, false), 0);
            let current = metadata(&renderer);
            for face in 0..6 {
                let start = 128 + face * 36;
                assert_eq!(previous[start..start + 16], current[start..start + 16],
                    "subthreshold motion paired retained depth with a new projection");
            }
            previous = current;
        }
        lamp.pos[0] = origin + trigger + 0.21;
        renderer.set_dynamic_lights(&[lamp]);
        let mut visited = [false; 6];
        for _ in 0..3 {
            renderer.time += 1.0 / 60.0;
            capture.frame(&mut renderer);
            let updates = scheduled(&renderer, false);
            assert_eq!(updates.into_iter().filter(|&updated| updated).count(), 2,
                "cumulative carried motion exceeded or stalled the normal two-face budget");
            let current = metadata(&renderer);
            for (face, updated) in updates.into_iter().enumerate() {
                let start = 128 + face * 36;
                if updated {
                    assert!(!visited[face], "carried round robin repeated a face before covering the cube");
                    assert_ne!(previous[start..start + 16], current[start..start + 16],
                        "triggered carried face kept its old projection");
                    visited[face] = true;
                } else {
                    assert_eq!(previous[start..start + 16], current[start..start + 16],
                        "unrendered carried depth was paired with the current light position");
                }
            }
            previous = current;
        }
        assert_eq!(visited, [true; 6]);
        renderer.time += 1.0 / 60.0;
        capture.frame(&mut renderer);
        assert_eq!(scheduled(&renderer, false), [false; 6], "carried trigger did not drain to a cache hit");
    }
}

#[test]
fn carried_far_primary_animation_omits_world_passes_but_capsule_entry_revokes_and_restores() {
    let mut scene = scene();
    let mut lamp = scene.lights.remove(0);
    lamp.owner = Some(7);
    let key = ao_scene::TextureKey { rdb_type: 0, id: 720 };
    let mut occluder = quad(0.0, 0.0, -3.0, 8.0, 8.0);
    occluder.submeshes[0].texture = Some(key);
    occluder.submeshes[0].blend = ao_scene::Blend::AlphaTest;
    occluder.submeshes[0].uv_scroll = [0.5, 0.0];
    let mut liquid = occluder.clone();
    liquid.submeshes[0].blend = ao_scene::Blend::AlphaBlend;
    liquid.submeshes[0].liquid = true;
    scene.meshes.extend([occluder, liquid]);
    scene.instances.extend([
        Instance { mesh: 2, transform: Mat4::from_translation(Vec3::new(100.0, 0.0, 0.0)).to_cols_array_2d() },
        Instance { mesh: 3, transform: Mat4::from_translation(Vec3::new(-100.0, 0.0, 0.0)).to_cols_array_2d() },
    ]);
    // Fully covered texels make entry deterministically occlude the owner;
    // the authored alpha-test/scroll still requires current primary proof.
    scene.textures.insert(key, ao_scene::Texture {
        width: 2, height: 1, rgba: vec![255, 32, 16, 255, 16, 64, 255, 255],
        source: ao_scene::TextureSource::Generated,
    });
    let mut renderer = renderer(&scene);
    renderer.set_enhanced_settings(EnhancedSettings {
        local_light_shadows: true, point_shadow_count: 1, ..Default::default()
    });
    add_actors(&mut renderer, vec![actor(7, 1.5)]);
    renderer.set_dynamic_lights(&[lamp]);
    let capture = Capture::new(&renderer);
    // Empty near-mesh placeholders retain their GPU vertices. Only the two
    // distinct far meshes are re-uploaded/revisioned by the real repose API.
    let mut posed = Scene {
        meshes: vec![Mesh::default(), Mesh::default(), scene.meshes[2].clone(), scene.meshes[3].clone()],
        instances: scene.instances.clone(), ..Default::default()
    };
    let mut admitted = Vec::new();
    for _ in 0..32 {
        renderer.time += 1.0 / 60.0;
        admitted = capture.frame(&mut renderer);
    }
    assert!(caster_instances(&renderer, true) == 0
        && metadata(&renderer)[128..].as_chunks::<36>().0.iter().take(6)
            .all(|face| f32::from_bits(face[32]) == 1.0),
        "far-animation fixture did not settle an actually admitted carried cube");
    for frame in 0..12 {
        renderer.time += 1.0 / 60.0;
        posed.instances[2].transform = Mat4::from_translation(Vec3::new(100.0 + frame as f32, 0.0, 0.0)).to_cols_array_2d();
        posed.instances[3].transform = Mat4::from_translation(Vec3::new(-100.0 - frame as f32, 0.0, 0.0)).to_cols_array_2d();
        posed.meshes[2].vertices[0].pos[0] += 0.001;
        posed.meshes[3].vertices[0].pos[0] -= 0.001;
        renderer.repose(&posed);
        assert_eq!(admitted, capture.frame(&mut renderer),
            "offscreen distant primary animation changed an admitted carried receiver");
        assert_eq!(renderer.point_shadows.as_ref().unwrap().static_face_mask, 0,
            "far alpha-scroll/liquid time or world reposing dirtied unrelated carried world passes");
        assert_eq!(scheduled(&renderer, false), [false; 6]);
        assert_eq!(caster_instances(&renderer, false), 0);
    }
    // Move the very same authored alpha-scrolling primary occluder into the
    // eye-to-owner capsule, without changing actor records or source identity.
    posed.instances[2].transform = IDENTITY;
    renderer.repose(&posed);
    renderer.time += 1.0 / 60.0;
    let blocked = capture.frame(&mut renderer);
    renderer.set_dynamic_lights(&[Light { color: [0.0; 3], ..lamp }]);
    assert_eq!(blocked, capture.frame(&mut renderer),
        "current primary occlusion failed to revoke the hidden owner's light/shadow signal");
    renderer.set_dynamic_lights(&[lamp]);
    capture.frame(&mut renderer);
    posed.instances[2].transform = Mat4::from_translation(Vec3::new(100.0, 0.0, 0.0)).to_cols_array_2d();
    renderer.repose(&posed);
    renderer.time += 1.0 / 60.0;
    let restored = capture.frame(&mut renderer);
    assert_eq!(scheduled(&renderer, false), [true; 6],
        "mature owner restoration after capsule exit did not immediately render all six world faces");
    assert!(caster_instances(&renderer, false) > 0,
        "restored carried source did not rasterize real world occlusion");
    assert_eq!(admitted, restored,
        "capsule exit restarted mature light fading or retained the occluder's private shadow history");
    renderer.set_enhanced_settings(EnhancedSettings::default());
    let unshadowed = capture.frame(&mut renderer);
    assert!(patch_light(&renderer, &restored, Vec3::new(-2.0, -2.0, -8.0)) + 9 * 3 * 8
        < patch_light(&renderer, &unshadowed, Vec3::new(-2.0, -2.0, -8.0)),
        "far-animation/capsule regression never preserved positive world-shadow geometry");
}
