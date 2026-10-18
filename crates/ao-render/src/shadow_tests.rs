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
#[test]
fn grass_shadow_receiver_preserves_ambient_and_disables_without_caster_layer() {
    let source = include_str!("sun_shadows.wgsl");
    assert!(source.contains("if shadow_frame.control.z == 0u { return 1.0; }"));
    assert!(source.contains("0.45 * (1.0 - edge)"));
    assert!(source.contains("visible *= grass_blade_visibility(world);"));
    assert!(include_str!("grass_shadow.wgsl").contains("visible *= grass_blade_visibility(world);"));
    assert!(source.contains("if shadow_frame.control.w != 0u {"));
    assert!(include_str!("grass_shadow.wgsl").contains("if shadow_frame.control.w != 0u {"));
    assert!(!include_str!("volumetric_shafts.wgsl").contains("shadow_frame.control.w"),
        "shaft ray marching must retain raw solid depths independently of surface receiver permission");
    // Only the directional share is attenuated; the receiver's existing artistic
    // strength remains the final operation, never the ambient or fog budget.
    assert!(source.contains("return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));"));
}

#[test]
fn standalone_grass_sun_shadows_compile_receivers_without_world_shadow_preference() {
    let baseline = EnhancedSettings { ground_grass: true, ..Default::default() };
    let grass = EnhancedSettings { grass_sun_shadows: true, ..baseline };
    assert!(!grass.sun_shadows);
    assert_ne!(variant::Key::of(baseline, 1), variant::Key::of(grass, 1),
        "standalone grass receivers need their own cached world variant");
    let source = enhanced_shader::source(grass);
    assert!(source.contains("fn grass_blade_visibility("));
    assert!(source.contains("i.sun_share.xyz * (1.0 - sun_visibility("),
        "ground must receive real blade depths without world sun shadows");
    let disabled = EnhancedSettings { ground_grass: false, ..grass };
    assert_eq!(variant::Key::of(disabled, 1), variant::Key::of(EnhancedSettings::default(), 1),
        "disabled grass must not change the faithful world pipeline");
}

#[test]
fn standalone_grass_point_receiver_preserves_native_caps_palette_and_fog() {
    let settings = EnhancedSettings { ground_grass: true, grass_point_shadows: true, ..Default::default() };
    assert!(!settings.clustered_lighting && !settings.local_light_shadows);
    let off = enhanced_shader::source(EnhancedSettings { grass_point_shadows: false, ..settings });
    assert!(!off.contains("fn grass_point_shadow_loss("));
    assert_eq!(off, enhanced_shader::source(EnhancedSettings { ground_grass: false, ..settings }));
    assert_ne!(variant::Key::of(settings, 1), variant::Key::of(EnhancedSettings { grass_point_shadows: false, ..settings }, 1));
    for clustered in [false, true] {
        for packed in [false, true] {
            for directional in 0..8 {
                let source = enhanced_shader::source(EnhancedSettings {
                    clustered_lighting: clustered, material_specular: packed,
                    sun_shadows: directional & 1 != 0, moon_shadows: directional & 2 != 0,
                    grass_sun_shadows: directional & 4 != 0,
                    ambient_occlusion: true, emissive_bloom: true, hdr: true,
                    ..settings
                });
                assert_eq!(source.contains("fn clustered_ground("), clustered,
                    "grass alone must not turn on world per-pixel relighting");
                assert!(source.contains("o.light - ground_without_local") || source.contains("o.light.xyz - ground_without_local"));
                assert!(source.contains("ground_without_local + g.ambient_g.rgb"),
                    "simple terrain's local cap must include its second native ambient saturation");
                for name in ["fs_ground_fused", "fs_ground_shadow", "fs_ground_simple"] {
                    let body = source.split(&format!("fn enhanced_original_{name}(")).nth(1).unwrap().split("\n}").next().unwrap();
                    assert_eq!(body.matches("grass_point_shadow_loss(i.wpos, grass_point_normal)").count(), 1);
                    assert!(!body.contains("grass_point_shadow_loss(i.wpos, i.n)"),
                        "directional depth-bias facet normal is not a diffuse lighting normal");
                    let wrapper = source.split(&format!("@fragment fn {name}(")).nth(1).unwrap().split("\n}").next().unwrap();
                    assert!(wrapper.find("grass_point_normal = i.n;").unwrap() < wrapper.find("let beauty =").unwrap(),
                        "preserve the authored normal before calling the body with the directional surface normal");
                    let share = if name == "fs_ground_simple" { "tint" } else { "spec" };
                    // Clustered lighting packs material fields too, even when
                    // the authored-specular setting itself is disabled.
                    let swizzle = if packed || clustered { ".xyz" } else { "" };
                    assert!(body.contains(&format!("min(i.{share}{swizzle}, grass_point_shadow_loss(")),
                        "{name} must cap the blocked loss to its own exact native local share");
                    let loss = body.find("grass_point_shadow_loss").unwrap();
                    let fog = body.rfind("ground_fog(i)").unwrap();
                    assert!(loss < fog, "native fog must follow the local subtraction");
                    if name != "fs_ground_simple" {
                        assert!(loss < body.find("ground_palette(i)").unwrap(),
                            "native palette saturation must follow the local subtraction");
                    }
                }
                let base = source.split("fn enhanced_original_fs_ground_base(").nth(1).unwrap().split("\n}").next().unwrap();
                assert!(!base.contains("grass_point_shadow_loss"), "two-pass base must not shadow twice");
                let helper = source.split("fn grass_point_shadow_loss(").nth(1).unwrap().split("\n}").next().unwrap();
                assert!(helper.contains("dyn_lights[322u]") && helper.contains("66u + k * 4u"));
                assert!(helper.find("!owner_light_admitted(packed)").unwrap() < helper.find("used += 1u").unwrap());
                assert!(helper.contains("used < 8u"));
                assert!(helper.contains("1.0 - point_shadow_visibility(-d, a.w, packed, false)"));
                assert!(!source.contains("@location(16)") && !source.contains("@group(4)"));
            }
        }
    }
    let shared = enhanced_shader::source(EnhancedSettings { local_light_shadows: true, ..settings });
    assert!(!shared.contains("fn grass_ground_directional("), "existing local shadow receiver samples blade cubes already");
    // Scalar channel model of the native two saturation stages: local light
    // hidden by sun/ambient saturation cannot be removed later at the fragment.
    let cap = |sun: f32, local: f32, ambient: f32| {
        let full = (sun + local).clamp(0.0, 1.0);
        let base = sun.clamp(0.0, 1.0);
        (full - base, (full + ambient).clamp(0.0, 1.0) - (base + ambient).clamp(0.0, 1.0))
    };
    assert_eq!(cap(1.0, 0.5, 0.2), (0.0, 0.0));
    assert_eq!(cap(0.5, 0.75, 0.5), (0.5, 0.0));
    let corners = [cap(0.0, 0.5, 0.0).0, cap(1.0, 0.5, 0.0).0];
    let gouraud_cap = 0.5 * (corners[0] + corners[1]);
    assert_eq!(gouraud_cap, 0.25);
    assert_eq!(gouraud_cap.min(0.5), 0.25, "cap after Gouraud interpolation, not from interpolated inputs");
}

#[test]
fn real_grass_shadow_pipeline_is_opt_in_and_reuses_visible_geometry() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless grass caster GPU");
    let baseline = sun_shadows::SunShadows::new(&renderer, (32, 32), 256);
    assert!(baseline.grass_layer.is_none());
    assert!(baseline.grass_caster.is_none());
    renderer.enhanced_settings.ground_grass = true;
    renderer.enhanced_settings.grass_sun_shadows = true;
    let blades = sun_shadows::SunShadows::new(&renderer, (32, 32), 256);
    blades.grass_caster.as_ref().expect("enabled blade caster").pipeline.get();
    assert!(blades.grass_layer.is_some());
    assert_eq!(blades.resident_bytes() - baseline.resident_bytes(), 256 * 256 * 4);
}

#[test]
fn grass_and_shafts_do_not_enable_world_solid_shadow_receivers() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless grass/shaft independence GPU");
    let scene = Scene {
        meshes: vec![
            quad([[-8.0, 0.0, -8.0], [8.0, 0.0, -8.0], [8.0, 0.0, 8.0], [-8.0, 0.0, 8.0]], [0.0, 1.0, 0.0]),
            quad([[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 3.0, 0.0], [-1.0, 3.0, 0.0]], [0.0, 0.0, 1.0]),
        ],
        instances: (0..2).map(|mesh| ao_scene::Instance { mesh, transform: ao_scene::IDENTITY }).collect(),
        ..Default::default()
    };
    renderer.upload(&scene);
    renderer.shadow_sun = Some([-0.8, 0.5, -0.3]);
    renderer.env.sun_dir = [-0.8, 0.5, -0.3];
    renderer.env.sun_color = [0.6; 3];
    renderer.env.ambient = [0.2; 3];
    renderer.env.fog_start = 100.0;
    renderer.env.fog_end = 120.0;
    let settings = EnhancedSettings { ground_grass: true, volumetric_shafts: true, ..Default::default() };
    renderer.set_enhanced_settings(settings);
    let (width, height) = (256, 192);
    let targets = Targets::new(&renderer, width, height);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grass/shaft surface independence"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let camera = Camera::look_at(Vec3::new(0.0, 3.0, 8.0), Vec3::ZERO);
    let capture = |renderer: &mut Renderer| {
        renderer.render(&view, &targets, &camera);
        texture_to_rgba(renderer, &output, width, height).unwrap()
    };
    let shafts_only = capture(&mut renderer);
    renderer.set_enhanced_settings(EnhancedSettings { grass_sun_shadows: true, ..settings });
    let grass_and_shafts = capture(&mut renderer);
    let shadows = renderer.sun_shadows.as_ref().expect("shafts retain real maps");
    assert!(!shadows.static_commands.is_empty(), "control must populate solid maps for shafts");
    assert_eq!(shadows.coverage_uniform.control[3], 0, "solid surface reception must remain disabled");
    // No grass terrain exists in this scene: enabling its receiver must not
    // reveal the solid silhouettes that only the shaft ray march requested.
    assert_eq!(shafts_only, grass_and_shafts);
    renderer.set_enhanced_settings(EnhancedSettings { sun_shadows: true, grass_sun_shadows: true, ..settings });
    let world_shadows = capture(&mut renderer);
    assert_eq!(renderer.sun_shadows.as_ref().unwrap().coverage_uniform.control[3], 1);
    assert_ne!(grass_and_shafts, world_shadows, "control must visibly receive the wall's solid shadow");
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
    let blob = ao_scene::ActorFrame { id: 2, model: 1, native_shadow_source: Some(1),
        transform: Mat4::from_translation(Vec3::new(4.0, 0.0, 2.0)).to_cols_array_2d(), ..Default::default() };
    renderer.set_actors(vec![visible_actor.clone(), blob.clone()]);
    assert_eq!(visible, capture(&mut renderer), "native blob remained beside a valid sun replacement");
    renderer.shadow_sun = None;
    renderer.set_actors(vec![visible_actor.clone()]);
    let no_sun = capture(&mut renderer);
    renderer.set_actors(vec![visible_actor.clone(), blob.clone()]);
    assert_ne!(no_sun, capture(&mut renderer), "missing sun replacement incorrectly removed the native blob");
    renderer.shadow_sun = Some([-0.8, 0.5, -0.3]);
    renderer.set_enhanced_settings(EnhancedSettings {sun_shadows: true, shadow_strength: 0.0, ..Default::default()});
    renderer.set_actors(vec![visible_actor.clone()]);
    let zero_strength = capture(&mut renderer);
    renderer.set_actors(vec![visible_actor.clone(), blob.clone()]);
    assert_ne!(zero_strength, capture(&mut renderer), "zero-strength replacement incorrectly removed the native blob");
    renderer.set_enhanced_settings(EnhancedSettings {sun_shadows: true, ..Default::default()});
    renderer.set_actors(vec![visible_actor.clone()]);
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
    renderer.set_actors(vec![ao_scene::ActorFrame { id: 1, model: 1, ..Default::default() }]);
    let faithful_without_blob = capture(&mut renderer);
    renderer.set_actors(vec![ao_scene::ActorFrame { id: 1, model: 1, ..Default::default() }, blob]);
    assert_ne!(faithful_without_blob, capture(&mut renderer), "all-off faithful path removed its native blob");
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
        // The blades this fixture exercises are daylight-only (Enhanced night rule), so the authored game day is 1.
        daylight: Some(1.0),
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
    assert!(renderer.sun_shadows.as_ref().unwrap().static_commands.iter()
        .any(|(draw, range)| renderer.gpu.opaque[*draw].mesh == 0 && !range.is_empty()),
        "native terrain was excluded from the sun caster volume");
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
    // Night presentation must retain native lighting and non-emissive blades.
    // The user-approved moon/lamp policy no longer caps grass to the bare framebuffer.
    renderer.sky.pose.daylight = Some(0.0);
    let night_bare = capture(&mut renderer, EnhancedSettings { grass_density: 0.0, ..grass });
    let night_blades = capture(&mut renderer, grass);
    assert_eq!(renderer.enhanced_grass_cell_count(), 4, "the night capture dropped the fixture's grass cells");
    let mut visible_night_blades = 0;
    for (blade, ground) in night_blades.as_chunks::<4>().0.iter().zip(night_bare.as_chunks::<4>().0) {
        visible_night_blades += usize::from(blade[..3] != ground[..3]);
    }
    assert!(visible_night_blades > 8, "night grass was removed instead of shaded");
    assert_eq!(night_blades, capture(&mut renderer, EnhancedSettings { emissive_bloom: true, bloom_strength: 1.0, ..grass }),
        "night grass must not become self-emissive");
    renderer.sky.pose.daylight = Some(1.0);
    assert_eq!(faithful, capture(&mut renderer, EnhancedSettings::default()), "all-off roundtrip changed the native image");
    assert!(renderer.grass.is_none(), "disabled grass retained its resources");
    write_evidence(&[
        ("grass-bare", &bare), ("grass-blades", &blades),
        ("grass-shadow-bare", &shadow_bare), ("grass-shadow-blades", &shadow_blades),
        ("grass-ao-bare", &ao_bare), ("grass-ao-blades", &ao_blades),
        ("grass-emission", &emission),
    ], (width, height));
}

#[test]
fn coarse_terrain_hdr_sky_fill_preserves_closed_island_without_triangle_wedges() {
    use ao_scene::ground_detail::{GroundCell, GroundPatch};
    let mut ground = ao_scene::Mesh { vertices: vec![], submeshes: vec![] };
    let mut fine_cells = Vec::new();
    let mut fine_indices = Vec::new();
    // Conservatively close every corner of the central roofed cells. The
    // surrounding one-cell ramps must be bilinear, not ROAM-triangle linear.
    let openness = |x: usize, z: usize| if (2..=6).contains(&x) && (2..=6).contains(&z) { 0.0 } else { 1.0 };
    let vertex = |x: usize, z: usize| ao_scene::Vertex {
        pos: [x as f32, 0.0, -(z as f32)], normal: [0.0, 1.0, 0.0],
        uv: [0.5; 2], color: [1.0, 1.0, 1.0, 1.0 - openness(x, z)],
    };
    for z in 0..8 {
        for x in 0..8 {
            let first = ground.vertices.len() as u32;
            ground.vertices.extend([[0, 0], [1, 0], [0, 1], [1, 1]].map(|[dx, dz]| vertex(x + dx, z + dz)));
            fine_cells.push(GroundCell { submesh: 0, vertices: [first, first + 1, first + 2, first + 3], ..Default::default() });
            fine_indices.extend([first, first + 1, first + 2, first + 1, first + 3, first + 2]);
        }
    }
    let lattice_start = ground.vertices.len() as u32;
    for z in 0..=8 { for x in 0..=8 { ground.vertices.push(vertex(x, z)); } }
    let sand = ao_scene::TextureKey { rdb_type: 1_010_006, id: 41 };
    let palette = ao_scene::TextureKey { rdb_type: 1_000_007, id: 42 };
    // Native fine tiles and coarse composite have separate material draws.
    let composite = ao_scene::TextureKey { rdb_type: 1_010_006, id: 43 };
    ground.submeshes = vec![
        ao_scene::Submesh::new(fine_indices, Some(sand)),
        ao_scene::Submesh::new(vec![lattice_start, lattice_start + 8, lattice_start + 72,
            lattice_start + 8, lattice_start + 80, lattice_start + 72], Some(composite)),
    ];
    let sun = [0.0, 0.8, -0.6];
    let scene = Scene {
        textures: [
            (sand, ao_scene::Texture { width: 8, height: 8, rgba: [180, 156, 112, 255].repeat(64), source: ao_scene::TextureSource::GroundRgb }),
            (composite, ao_scene::Texture { width: 8, height: 8, rgba: [180, 156, 112, 255].repeat(64), source: ao_scene::TextureSource::GroundRgb }),
            (palette, ao_scene::Texture { width: 1, height: 1, rgba: vec![128, 128, 128, 255], source: Default::default() }),
        ].into(),
        meshes: vec![ground],
        instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
        ground_detail: vec![GroundPatch {
            mesh: 0, cells: 8, cell_size: 1.0, height_scale: 1.0, heights: vec![0; 81],
            lattice_start, coarse_submesh: 1, fine_cells, shadow_texture: Some(palette),
            shadow_uv_scale: [0.125; 2], ..Default::default()
        }],
        sun_authored: true, ground_light: Some(([0.5; 3], sun)), daylight: Some(1.0),
        ..Default::default()
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless terrain sky-fill GPU");
    renderer.set_msaa_samples(1);
    renderer.upload(&scene);
    renderer.set_ground_detail(100, 7);
    renderer.shadow_sun = Some(sun);
    renderer.env.sun_dir = sun;
    // The authored palette supplies native brightness. A positive but tiny
    // direct light keeps cascades active, below HDR's directional-share ramp.
    renderer.env.sun_color = [0.000001; 3];
    renderer.env.ambient = [0.15; 3];
    renderer.env.fog_start = 100.0;
    renderer.env.fog_end = 120.0;
    renderer.time = 0.0;
    // ROAM selection uses horizontal camera distance, not camera height.
    let camera = Camera::look_at(Vec3::new(4.0, 16.0, 12.0), Vec3::new(4.0, 0.0, -4.0));
    let (width, height) = (512, 512);
    let targets = Targets::new(&renderer, width, height);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain sky-fill image"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let settings = EnhancedSettings {
        sun_shadows: true, shadow_map_size: 4096, shadow_strength: 0.45, shadow_softness: 1.5,
        sun_boost: 0.6, grade_strength: 0.0, ..Default::default()
    };
    let capture = |renderer: &mut Renderer, hdr| {
        renderer.set_enhanced_settings(EnhancedSettings { hdr, ..settings });
        renderer.render(&view, &targets, &camera);
        texture_to_rgba(renderer, &output, width, height).unwrap()
    };
    let aspect = width as f32 / height as f32;
    let projection = Mat4::perspective_rh(renderer.lens.vertical_fov(aspect), aspect,
        renderer.lens.near, renderer.lens.far.unwrap_or(132.0));
    let inverse = (projection * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up())).inverse();
    let world: Vec<_> = (0..width * height).map(|pixel| {
        let ndc = Vec3::new((pixel % width) as f32 + 0.5, (pixel / width) as f32 + 0.5, 0.5);
        let ray = inverse.project_point3(Vec3::new(ndc.x / width as f32 * 2.0 - 1.0,
            1.0 - ndc.y / height as f32 * 2.0, ndc.z)) - camera.pos;
        let p = camera.pos - ray * (camera.pos.y / ray.y);
        (p.is_finite() && p.x > 0.25 && p.x < 7.75 && -p.z > 0.25 && -p.z < 7.75)
            .then_some([p.x, -p.z])
    }).collect();
    for (mode, name) in [(3, "terrain-fused"), (1, "terrain-two-pass")] {
        renderer.set_ground_rendering(mode);
        let native = capture(&mut renderer, false);
        let hdr = capture(&mut renderer, true);
        write_evidence(&[(&format!("{name}-native"), &native), (&format!("{name}-hdr"), &hdr)], (width, height));
        assert_eq!(renderer.sun_shadows.as_ref().expect("fixture did not execute real sun shadows").resolution, 4096);
        let ground = renderer.ground.as_ref().unwrap();
        assert!(ground.has_shadow[0] && ground.materials[0].is_some(), "native shadow palette was not bound");
        assert!(ground.indices[0][0].is_empty(), "fine cells unexpectedly replaced coarse ROAM triangles");
        assert_eq!(ground.indices[0][1].len(), 6, "fixture must retain only its two coarse root triangles");
        assert!(ground.indices[0][1].iter().all(|&index| scene.meshes[0].vertices[index as usize].color[3] == 0.0),
            "coarse vertices must skip every closed interior lattice point");
        let (mut closed, mut exposed, mut transitions, mut edges) = (0, 0, 0, 0);
        let (mut max_error, mut max_gradient) = (0.0f32, 0i16);
        for (pixel, point) in world.iter().enumerate() {
            let Some([x, z]) = *point else { continue; };
            let (ix, iz) = (x.floor() as usize, z.floor() as usize);
            let (u, v) = (x.fract(), z.fract());
            let sky = (openness(ix, iz) * (1.0 - u) + openness(ix + 1, iz) * u) * (1.0 - v)
                + (openness(ix, iz + 1) * (1.0 - u) + openness(ix + 1, iz + 1) * u) * v;
            let at = pixel * 4;
            assert!((32..=160).contains(&native[at]), "fixture lost its native midtone sand");
            for channel in 0..3 {
                let base = f32::from(native[at + channel]);
                let delta = i16::from(hdr[at + channel]) - i16::from(native[at + channel]);
                // Below the tone knee: exactly native * (1 + .6 * .6 * sky).
                // Two codes cover native/output quantization, f16 and dither.
                let error = (f32::from(delta) - base * 0.36 * sky).abs();
                max_error = max_error.max(error);
                assert!(error <= 2.0, "{name}: non-bilinear sky-fill wedge at {x},{z}, channel {channel}: delta {delta}, sky {sky}, error {error}");
                if (3.0..5.0).contains(&x) && (3.0..5.0).contains(&z) {
                    assert!(delta.abs() <= 1, "{name}: closed island received HDR sky fill");
                }
                if sky == 1.0 { assert!(delta >= 8, "{name}: exposed sand control received no positive boost"); }
                for neighbor in [pixel + 1, pixel + width as usize] {
                    let Some(Some([nx, nz])) = world.get(neighbor) else { continue; };
                    let gradient = (i16::from(hdr[neighbor * 4 + channel]) - i16::from(hdr[at + channel])).abs();
                    // Bilinear 0/1 lattice has |dsky/dx|, |dsky/dz| <= 1.
                    // Thus this is a geometric Lipschitz bound, not a tuned
                    // smoothness tolerance that could hide diagonal wedges.
                    let bound = (base * 0.36 * ((nx - x).abs() + (nz - z).abs())).ceil() as i16 + 2;
                    max_gradient = max_gradient.max(gradient);
                    assert!(gradient <= bound, "{name}: terrain pixel gradient {gradient} exceeds bilinear bound {bound}");
                    edges += 1;
                }
            }
            closed += usize::from((3.0..5.0).contains(&x) && (3.0..5.0).contains(&z));
            exposed += usize::from(sky == 1.0);
            transitions += usize::from(sky > 0.0 && sky < 1.0);
        }
        assert!(closed > 100 && exposed > 100 && transitions > 100 && edges > 1000,
            "insufficient rendered terrain coverage: closed {closed}, exposed {exposed}, transitions {transitions}, edges {edges}");
        eprintln!("{name}: max exposure residual {max_error:.3} codes (bound 2), max adjacent gradient {max_gradient} codes; closed {closed}, exposed {exposed}, transitions {transitions}");
    }
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

#[test]
fn moon_receivers_positive_roof_fog_and_all_off_roundtrip() {
    use ao_scene::ground_detail::{GroundCell, GroundPatch};
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless moon receiver GPU");
    let mut ground = quad([[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [0.0, 0.0, -8.0], [8.0, 0.0, -8.0]], [0.0, 1.0, 0.0]);
    ground.submeshes[0].indices = vec![0, 1, 2, 1, 3, 2];
    for vertex in &mut ground.vertices { vertex.color[3] = 0.0; }
    let mut scene = Scene {
        environment: Some(Environment {
            sky_color: [0.01; 3], fog_color: [0.02; 3], fog_start: 100.0, fog_end: 120.0,
            ambient: [0.01; 3], sun_color: [0.0; 3], sun_dir: [0.0, -1.0, 0.0], sun_specular: 0.0,
        }),
        daylight: Some(0.0),
        meshes: vec![ground, quad([[1.0, 0.1, -1.0], [3.0, 0.1, -1.0], [1.0, 0.1, -3.0], [3.0, 0.1, -3.0]], [0.0, 1.0, 0.0])],
        instances: (0..2).map(|mesh| ao_scene::Instance {mesh, transform: ao_scene::IDENTITY}).collect(),
        ground_detail: vec![GroundPatch {
            mesh: 0, cells: 1, cell_size: 8.0, heights: vec![0; 4], coarse_submesh: 0,
            fine_cells: vec![GroundCell {submesh: 0, vertices: [0, 1, 2, 3], ..Default::default()}],
            ..Default::default()
        }],
        ..Default::default()
    };
    scene.meshes[1].submeshes[0].indices = vec![0, 1, 2, 1, 3, 2];
    scene.moons[0] = Some(ao_scene::MoonSource {toward_light: [0.0, 1.0, 0.0], ..Default::default()});
    renderer.upload(&scene);
    let camera = Camera::look_at(Vec3::new(4.0, 9.0, 3.0), Vec3::new(4.0, 0.0, -4.0));
    let (width, height) = (128, 128);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("moon numeric receiver"), size: wgpu::Extent3d {width, height, depth_or_array_layers: 1},
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let capture = |renderer: &mut Renderer, settings| {
        renderer.set_enhanced_settings(settings);
        renderer.render(&output.create_view(&Default::default()), &Targets::new(renderer, width, height), &camera);
        texture_to_rgba(renderer, &output, width, height).unwrap()
    };
    let moonlit = EnhancedSettings {night_mode: NightMode::Moonlit, ..Default::default()};
    let faithful = capture(&mut renderer, EnhancedSettings::default());
    let lit = capture(&mut renderer, moonlit);
    let brighter = lit.as_chunks::<4>().0.iter().zip(faithful.as_chunks::<4>().0)
        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| i16::from(*x) - i16::from(*y) > 4)).count();
    assert!(brighter > 100, "authored moon must add measurable native receiver energy: {brighter}");
    let vp = Mat4::perspective_rh(renderer.lens.vertical_fov(1.0), 1.0, renderer.lens.near,
        renderer.lens.far.unwrap_or(180.0)) * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up());
    for (name, point) in [("material", Vec3::new(2.0, 0.1, -2.0)), ("ground", Vec3::new(6.0, 0.0, -5.0))] {
        let clip = vp * point.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let x = ((ndc.x * 0.5 + 0.5) * width as f32) as usize;
        let y = ((0.5 - ndc.y * 0.5) * height as f32) as usize;
        let offset = (y * width as usize + x) * 4;
        assert!(lit[offset..offset + 3].iter().zip(&faithful[offset..offset + 3])
            .all(|(a, b)| i16::from(*a) - i16::from(*b) > 4), "{name} must receive positive Moon RGB");
    }
    assert_eq!(renderer.sun_shadows.as_ref().unwrap().resolution, 1, "unshadowed Moonlit must not allocate cascades");
    assert!(renderer.enhanced_moonlight().is_some());
    assert!(renderer.enhanced_shadow_moonlight().is_none(), "dummy bindings are not encoded Moon maps");
    let mapped = EnhancedSettings {moon_shadows: true, shadow_map_size: 256, ..moonlit};
    capture(&mut renderer, mapped);
    assert!(renderer.enhanced_shadow_moonlight().is_some(), "encoded Moon metadata must be public");
    capture(&mut renderer, EnhancedSettings {volumetric_shafts: true, shadow_map_size: 256, ..moonlit});
    assert!(renderer.enhanced_shadow_moonlight().is_some(), "shafts request Moon maps independently of surface shadows");
    assert_eq!(faithful, capture(&mut renderer, EnhancedSettings::default()), "all-off must roundtrip exactly");
    for vertex in &mut scene.meshes[0].vertices { vertex.color[3] = 1.0; }
    renderer.upload(&scene);
    let roofed = capture(&mut renderer, EnhancedSettings::default());
    assert_eq!(roofed, capture(&mut renderer, moonlit), "native roof exposure must close material and ground receivers");
    for vertex in &mut scene.meshes[0].vertices { vertex.color[3] = 0.0; }
    renderer.upload(&scene);
    renderer.env.fog_start = 0.0;
    renderer.env.fog_end = 1.0;
    let fogged = capture(&mut renderer, EnhancedSettings::default());
    assert_eq!(fogged, capture(&mut renderer, moonlit), "fully native-fogged geometry must retain identity");
    scene.daylight = None;
    renderer.upload(&scene);
    let unknown = capture(&mut renderer, EnhancedSettings::default());
    assert_eq!(unknown, capture(&mut renderer, EnhancedSettings {night_mode: NightMode::Dark, ..Default::default()}),
        "unknown authored clock must not darken login, preview or interior geometry");
}

#[test]
fn moonlit_hidden_and_nonadmitted_actors_leave_no_shadow_signal() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless moon shadow GPU");
    let environment = Environment {
        sky_color: [0.01; 3], fog_color: [0.0; 3], fog_start: 100.0, fog_end: 120.0,
        ambient: [0.03; 3], sun_color: [0.0; 3], sun_dir: [0.0, -1.0, 0.0], sun_specular: 0.0,
    };
    let mut scene = Scene {
        meshes: vec![
            quad([[-8.0, 0.0, -8.0], [8.0, 0.0, -8.0], [8.0, 0.0, 8.0], [-8.0, 0.0, 8.0]], [0.0, 1.0, 0.0]),
            quad([[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 4.0, 0.0], [-2.0, 4.0, 0.0]], [0.0, 0.0, 1.0]),
        ],
        environment: Some(environment), daylight: Some(0.0), ..Default::default()
    };
    scene.instances = (0..2).map(|mesh| ao_scene::Instance {mesh, transform: ao_scene::IDENTITY}).collect();
    scene.moons[0] = Some(ao_scene::MoonSource {toward_light: [-0.8, 0.5, -0.3], ..Default::default()});
    renderer.upload(&scene);
    renderer.add_actor_model(1, &Scene {meshes: vec![
        quad([[-0.4, 0.0, -1.0], [0.4, 0.0, -1.0], [0.4, 1.8, -1.0], [-0.4, 1.8, -1.0]], [0.0, 0.0, 1.0]),
    ], ..Default::default()});
    renderer.set_enhanced_settings(EnhancedSettings {
        night_mode: NightMode::Moonlit, moon_shadows: true, shadow_map_size: 256, ..Default::default()
    });
    let (width, height) = (384, 256);
    let targets = Targets::new(&renderer, width, height);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("moon hidden actor regression"), size: wgpu::Extent3d {width, height, depth_or_array_layers: 1},
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
    assert!(renderer.enhanced_shadow_moonlight().is_some(), "fixture needs active Moon maps");
    let shadows = renderer.sun_shadows.as_ref().expect("moon cascade targets");
    assert!(shadows.coverage_uniform.moon_budget[..3].iter().any(|&value| value > 0.0),
        "fixture has no active authored moon illumination");
    assert!(!shadows.sun_replacement_active, "moon incorrectly replaced native sun blobs");
    let body = ao_scene::ActorFrame {id: 1, model: 1, ..Default::default()};
    for actor in [
        body.clone(),
        ao_scene::ActorFrame {priority: Some(8), transform: Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0)).to_cols_array_2d(), ..body.clone()},
        ao_scene::ActorFrame {alpha: 0.0, transform: Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)).to_cols_array_2d(), ..body.clone()},
    ] {
        renderer.set_actors(vec![actor]);
        assert_eq!(absent, capture(&mut renderer), "hidden/nonadmitted actor disclosed itself in moonlight");
    }
    renderer.set_actors(vec![ao_scene::ActorFrame {
        transform: Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0)).to_cols_array_2d(), ..body
    }]);
    assert_ne!(absent, capture(&mut renderer), "positive-control moonlit actor was not visible");
}
