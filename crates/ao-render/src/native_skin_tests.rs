//! Headless GPU proof: exercise the production compute dispatch, not a second test shader.
use super::*;
use ao_scene::{NativeSkinVertex, SkinBone, Vertex};
use ao_formats::character::{actor::{ActorAssets, ActorRig, PoseCache}, *};
use std::sync::Arc;

fn renderer() -> Renderer {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    Renderer::new(&instance, None).expect("native skin regression requires a headless GPU")
}

fn difference(actual: &[u8], expected: &[u8]) -> Option<String> {
    if actual.len() != expected.len() { return Some(format!("byte lengths {} != {}", actual.len(), expected.len())); }
    let first = actual.iter().zip(expected).position(|(a, b)| a != b)?;
    let count = actual.iter().zip(expected).filter(|(a, b)| a != b).count();
    Some(format!("{count} differing bytes; first byte {first}: {} != {}", actual[first], expected[first]))
}

fn body_bytes(r: &mut Renderer, id: u32, count: usize) -> Vec<u8> {
    let size = (count * std::mem::size_of::<Vertex>()) as u64;
    let readback = r.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native skin regression readback"), size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    let mut encoder = r.device.create_command_encoder(&Default::default());
    r.encode_actor_skins(&mut encoder);
    encoder.copy_buffer_to_buffer(r.act.actors[&id].vb.as_ref().expect("computed body buffer"), 0, &readback, 0, size);
    r.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    r.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = readback.slice(..).get_mapped_range().to_vec();
    readback.unmap();
    bytes
}

fn pixels(r: &mut Renderer) -> Vec<u8> {
    let (w, h) = (384, 256);
    let targets = Targets::new(r, w, h);
    let out = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: Some("native skin regression pixels"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    r.render(&out.create_view_tracked(&Default::default()), &targets,
        &Camera::look_at([0.0, 5.0, 17.0].into(), [0.0, 1.0, 0.0].into()));
    texture_to_rgba(r, &out, w, h).unwrap()
}

/// CPU Xf::rot/apply + unit, character.rs:111–135 and actor.rs native skin loop.
/// Deliberately scalar: a dot product, normalize(), reciprocal sqrt or mul_add is not the oracle.
fn scalar_skin(base: Vertex, v: NativeSkinVertex, bones: &[SkinBone]) -> Vertex {
    if bones.is_empty() { return base; }
    let rotate = |bone: &SkinBone, p: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|i| bone.rows[i][0] * p[0] + bone.rows[i][1] * p[1] + bone.rows[i][2] * p[2])
    };
    let apply = |bone: &SkinBone, p| {
        let p = rotate(bone, p);
        std::array::from_fn::<_, 3, _>(|i| p[i] + bone.rows[i][3])
    };
    let b0 = &bones[v.bone0 as usize];
    let p0 = apply(b0, v.local0);
    let p = if v.weight >= 0.999 { p0 } else {
        let p1 = apply(&bones[v.bone1 as usize], v.local1);
        std::array::from_fn(|i| v.weight * p0[i] + (1.0 - v.weight) * p1[i])
    };
    let n = rotate(b0, v.normal);
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let n = if length > 1e-12 { n.map(|x| x / length) } else { [0.0, 1.0, 0.0] };
    Vertex { pos: [p[0], p[1], -p[2]], normal: [n[0], n[1], -n[2]], ..base }
}

#[test]
fn native_skin_gpu_bytes_cover_weight_normal_guard_and_static_fallback() {
    let mut r = renderer();
    let bones = Arc::new(vec![
        SkinBone { rows: [[0.8, -0.6, 0.0, 0.125], [0.6, 0.8, 0.0, -0.25], [0.0, 0.0, 1.0, 0.5]] },
        SkinBone { rows: [[1.0, 0.0, 0.0, -0.75], [0.0, 0.0, -1.0, 0.375], [0.0, 1.0, 0.0, 0.0]] },
    ]);
    let mut source = Vec::new();
    for weight in [0.0, 0.375, f32::from_bits(0.999f32.to_bits() - 1), 0.999, 1.0] {
        for normal in [[0.0; 3], [0.0, 1e-13, 0.0], [0.0, 1e-12, 0.0], [0.0, 2e-12, 0.0], [0.25, -0.5, 0.75]] {
            source.push(NativeSkinVertex { local0: [0.125, -0.75, 0.375], local1: [-0.25, 0.5, -0.625], weight,
                bone0: 0, bone1: if weight >= 0.999 { u32::MAX } else { 1 }, normal });
        }
    }
    let base: Vec<_> = (0..source.len()).map(|i| Vertex { pos: [i as f32, 0.25, -0.5], normal: [0.0, 0.0, 1.0],
        uv: [0.125, 0.875], color: [0.25, 0.5, 0.75, 1.0] }).collect();
    let mut scene = Scene::default();
    scene.meshes.push(ao_scene::Mesh { vertices: base.clone(), submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2], None)] });
    scene.native_skin = Some(Arc::new(source.clone()));
    r.add_actor_model(1, &scene);
    for palette in [bones.clone(), Arc::new(Vec::new()), bones] {
        let expected: Vec<_> = base.iter().zip(&source).map(|(&b, &v)| scalar_skin(b, v, &palette)).collect();
        r.set_actors(vec![ActorFrame { id: 1, model: 1, skin_pose: Some(palette), ..Default::default() }]);
        let bytes = body_bytes(&mut r, 1, expected.len());
        assert!(difference(&bytes, bytemuck::cast_slice(&expected)).is_none(), "{:?}", difference(&bytes, bytemuck::cast_slice(&expected)));
        // No new palette must retain the exact previous output, including the bind fallback.
        r.set_actors(vec![ActorFrame { id: 1, model: 1, ..Default::default() }]);
        assert_eq!(body_bytes(&mut r, 1, expected.len()), bytes);
        // Source-mesh effects may deliberately deform/recolour a CPU skin. It wins over a simultaneous palette.
        let mut effect = expected.clone();
        effect[0].pos[0] += 0.5;
        effect[0].color = [0.125, 0.25, 0.5, 0.75];
        r.set_actors(vec![ActorFrame { id: 1, model: 1, skin: Some(Arc::new(effect.clone())),
            skin_pose: Some(Arc::new(Vec::new())), ..Default::default() }]);
        assert_eq!(body_bytes(&mut r, 1, effect.len()), bytemuck::cast_slice::<_, u8>(&effect),
            "explicit source-effect CPU skin must win over native palette");
    }
}

#[test]
fn native_skin_effect_two_environment_uses_current_normal_not_authored_uv() {
    let key = TextureKey { rdb_type: 0, id: 84 };
    let vertices: Vec<_> = [(-0.6, -0.6), (0.6, -0.6), (0.6, 0.6), (-0.6, 0.6)].into_iter()
        .map(|(x, y)| Vertex { pos: [x, y, 0.0], normal: [0.0, 0.0, 1.0], uv: [0.05, 0.1], color: ao_scene::WHITE }).collect();
    let native = vertices.iter().map(|vertex| NativeSkinVertex {
        local0: vertex.pos, local1: [0.0; 3], weight: 1.0, bone0: 0, bone1: u32::MAX, normal: [0.0, 0.0, -1.0],
    }).collect();
    let mut scene = Scene {
        meshes: vec![ao_scene::Mesh { vertices, submeshes: vec![ao_scene::Submesh {
            base_color: [0.0, 0.0, 0.0, 1.0], two_sided: true, env_texture: Some(key),
            ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        }] }],
        native_skin: Some(Arc::new(native)), ..Default::default()
    };
    scene.textures.insert(key, ao_scene::Texture { source: Default::default(), width: 4, height: 4,
        rgba: (0..4).flat_map(|y| (0..4).flat_map(move |x| [(x * 85) as u8, (y * 85) as u8, 17, 255])).collect() });
    let mut renderer = renderer();
    renderer.add_actor_model(1, &scene);
    let mut centers = Vec::new();
    for sine in [-0.6, 0.6] {
        let palette = Arc::new(vec![SkinBone { rows: [[0.8, 0.0, sine, 0.0], [0.0, 1.0, 0.0, 1.0], [-sine, 0.0, 0.8, 0.0]] }]);
        renderer.set_actors(vec![ActorFrame { id: 1, model: 1, skin_pose: Some(palette.clone()), always: true, ..Default::default() }]);
        let ordinary = pixels(&mut renderer);
        renderer.set_actors(vec![ActorFrame { id: 1, model: 1, skin_pose: Some(palette), always: true, rendering_effect: 2, ..Default::default() }]);
        let effect_two = pixels(&mut renderer);
        assert!(difference(&ordinary, &effect_two).is_none(), "effect 2 must retain the native normal-map redraw: {:?}", difference(&ordinary, &effect_two));
        let center = (128 * 384 + 192) * 4;
        centers.push(effect_two[center..center + 3].to_vec());
    }
    assert_ne!(centers[0], centers[1], "rotating the skinned normal must move the sampled environment colour even with fixed authored UVs");
}

fn native_skin_fixture() -> Scene {
    let vertices = (0..3).map(|i| Vertex {
        pos: [i as f32, 0.25, -0.5], normal: [0.0, 1.0, 0.0],
        uv: [0.125, 0.875], color: [0.25, 0.5, 0.75, 1.0],
    }).collect();
    Scene {
        meshes: vec![ao_scene::Mesh { vertices, submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2], None)] }],
        native_skin: Some(Arc::new(vec![NativeSkinVertex {
            local0: [0.125, -0.75, 0.375], local1: [-0.25, 0.5, -0.625],
            weight: 0.999, bone0: 0, bone1: u32::MAX, normal: [0.0, 1.0, 0.0],
        }; 3])),
        ..Default::default()
    }
}

#[test]
fn native_skin_pending_initialization_survives_palette_buffer_growth() {
    let mut r = renderer();
    let scene = native_skin_fixture();
    r.add_actor_model(1, &scene);
    let bone = SkinBone { rows: [[1.0, 0.0, 0.0, 0.25], [0.0, 1.0, 0.0, -0.5], [0.0, 0.0, 1.0, 0.125]] };
    let grown = Arc::new(vec![bone; 2]);
    // Both the bind-pose -> animation case and two animated submissions replace a pending pose.
    for (id, first) in [(1, Arc::new(Vec::new())), (2, Arc::new(vec![bone]))] {
        r.set_actors(vec![ActorFrame { id, model: 1, skin_pose: Some(first), ..Default::default() }]);
        r.set_actors(vec![ActorFrame { id, model: 1, skin_pose: Some(grown.clone()), ..Default::default() }]);
        let expected: Vec<_> = scene.meshes[0].vertices.iter().zip(scene.native_skin.as_ref().unwrap().iter())
            .map(|(&base, &source)| scalar_skin(base, source, &grown)).collect();
        assert_eq!(body_bytes(&mut r, id, expected.len()), bytemuck::cast_slice::<_, u8>(&expected),
            "palette growth before encoding must retain bind UV and colour bytes");
    }
}

#[test]
fn native_skin_rejects_mismatched_attributes_at_model_admission() {
    let mut r = renderer();
    for count in [0, 2, 4] {
        let mut scene = native_skin_fixture();
        let vertex = scene.native_skin.as_ref().unwrap()[0];
        scene.native_skin = Some(Arc::new(vec![vertex; count]));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.add_actor_model(1, &scene))).is_err());
        assert!(!r.act.models.contains_key(&1), "malformed models must not be uploaded");
    }
    let mut scene = native_skin_fixture();
    r.add_actor_model(1, &scene);
    scene.meshes[0].vertices.pop();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.update_actor_model(1, &scene))).is_err());
    assert_eq!(r.act.models[&1].meshes[0].as_ref().unwrap().nverts, 3,
        "rejected replacement must leave the valid model intact");
}

#[test]
fn native_skin_rejects_active_out_of_range_bones_but_accepts_unused_bone1() {
    let mut r = renderer();
    let palette = Arc::new(vec![SkinBone {
        rows: [[1.0, 0.0, 0.0, 0.25], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
    }]);
    // bone0 is always read; bone1 is read below the threshold, including a NaN weight.
    for (bone0, bone1, weight) in [(1, 0, 1.0), (u32::MAX, 0, 1.0),
        (0, 1, f32::from_bits(0.999f32.to_bits() - 1)), (0, u32::MAX, f32::NAN)] {
        let mut scene = native_skin_fixture();
        for vertex in Arc::make_mut(scene.native_skin.as_mut().unwrap()) {
            vertex.bone0 = bone0;
            vertex.bone1 = bone1;
            vertex.weight = weight;
        }
        r.add_actor_model(1, &scene);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.set_actors(vec![
            ActorFrame { id: 1, model: 1, skin_pose: Some(palette.clone()), ..Default::default() },
        ]))).is_err());
        assert!(!r.act.actors.contains_key(&1), "invalid palette must be rejected before creating GPU work");
    }
    let scene = native_skin_fixture();
    r.add_actor_model(1, &scene);
    r.set_actors(vec![ActorFrame { id: 1, model: 1, skin_pose: Some(palette.clone()), ..Default::default() }]);
    let expected: Vec<_> = scene.meshes[0].vertices.iter().zip(scene.native_skin.as_ref().unwrap().iter())
        .map(|(&base, &source)| scalar_skin(base, source, &palette)).collect();
    assert_eq!(body_bytes(&mut r, 1, 3), bytemuck::cast_slice::<_, u8>(&expected),
        "bone1 MAX is valid when the native >= .999 branch does not read it");
}

#[test]
#[should_panic(expected = "actor IDs must be unique within a frame batch")]
fn native_skin_rejects_duplicate_actor_ids_before_dispatch() {
    let mut r = renderer();
    let scene = native_skin_fixture();
    r.add_actor_model(1, &scene);
    let mut other = scene.clone();
    other.meshes[0].vertices.extend_from_slice(&scene.meshes[0].vertices);
    Arc::make_mut(other.native_skin.as_mut().unwrap()).extend_from_slice(scene.native_skin.as_ref().unwrap());
    r.add_actor_model(2, &other);
    r.set_actors(vec![
        ActorFrame { id: 1, model: 1, skin_pose: Some(Arc::new(Vec::new())), ..Default::default() },
        ActorFrame { id: 1, model: 2, skin_pose: Some(Arc::new(Vec::new())), ..Default::default() },
    ]);
}

#[test]
#[ignore = "[INFERENCE] single-pixel per-render flip with byte-identical captured inputs; see docs/formats.md material raster quarantine"]
fn native_skin_installed_cat_rigs_and_sixty_actor_crowd_are_pixel_identical_to_cpu() {
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/rdb.db").exists() {
        eprintln!("SKIP native CAT asset proof: installed client missing at {}", dir.display());
        return;
    }
    let store = ao_rdb::RecordStore::open(&dir).unwrap();
    let mut assets = ActorAssets::new(&store).unwrap();
    // Seven player bodies plus non-humanoid CAT; real head, two weapons and light bar.
    let specs = [(5900, Some(40098)), (5907, Some(40681)), (5914, Some(40250)), (5921, None),
        (5927, Some(40630)), (5934, Some(40209)), (5941, Some(40135)), (29902, None)];
    let rigs: Vec<_> = specs.iter().map(|&(id, head)| ActorRig::new(&store, &assets, id, head,
        &PartTextures::new(), &PartLayers::new(), &[(1, 15839), (2, 7796), (5, 26163)]).unwrap()).collect();
    let clips: Vec<_> = rigs.iter().map(|rig| {
        let id = assets.clips(&store, rig.model_id).unwrap().iter().find(|(name, _)| name == "walk")
            .map(|(_, id)| *id)
            .unwrap_or_else(|| compatible_animations(&store, &load_cat_mesh(&store, CHAR_MESH_TYPE, rig.model_id).unwrap()).unwrap()[0].0);
        assets.anim(&store, id).unwrap()
    }).collect();
    assert!(rigs[1].model().meshes.len() >= 5, "head and all three installed attachments must load");
    let (mut gpu, mut cpu) = (renderer(), renderer());
    for (i, rig) in rigs.iter().enumerate() {
        assert!(rig.model().native_skin.is_some(), "model {} must take native skin path", rig.model_id);
        gpu.add_actor_model(i as u64 + 1, rig.model());
        let mut model = rig.model().clone();
        model.native_skin = None;
        cpu.add_actor_model(i as u64 + 1, &model);
    }
    let blank = pixels(&mut cpu);
    let mut byte_differences = Vec::new();
    // Individually frame each rig as well: no crowd neighbour may hide a broken body or rigid attachment.
    for (i, rig) in rigs.iter().enumerate() {
        let layers = [AnimLayer { clip: &clips[i], ms: 173.0, layers: 3, blend: 1.0 }];
        let (vertices, parts) = rig.pose_layered(&layers);
        let (palette, native_parts) = rig.pose_palette_composed_cached(layers.iter().copied(), &mut PoseCache::default());
        assert_eq!(parts, native_parts);
        let frame = ActorFrame { id: 1, model: i as u64 + 1, parts, always: true, ..Default::default() };
        gpu.set_actors(vec![ActorFrame { skin_pose: Some(palette), ..frame.clone() }]);
        cpu.set_actors(vec![ActorFrame { skin: Some(Arc::new(vertices.clone())), ..frame }]);
        let actual = body_bytes(&mut gpu, 1, vertices.len());
        if let Some(diff) = difference(&actual, bytemuck::cast_slice(&vertices)) {
            byte_differences.push(format!("individual model {}: {diff}", rig.model_id));
        }
        let (actual, expected) = (pixels(&mut gpu), pixels(&mut cpu));
        assert_ne!(expected, blank, "individual model {} must draw", rig.model_id);
        assert!(difference(&actual, &expected).is_none(), "individual model {}: {:?}; vertex diagnostics: {byte_differences:?}",
            rig.model_id, difference(&actual, &expected));
    }
    let mut previous = None;
    for (step, time) in [None, Some(0.0), Some(173.0), Some(611.0)].into_iter().enumerate() {
        let mut gpu_frames = Vec::new();
        let mut cpu_frames = Vec::new();
        for ordinal in 0..60 {
            let i = ordinal % rigs.len();
            let rig = &rigs[i];
            let layers: Vec<_> = time.map(|ms| AnimLayer { clip: &clips[i], ms: ms + (ordinal / rigs.len()) as f32 * 31.0, layers: 3, blend: 1.0 }).into_iter().collect();
            let (expected, parts) = rig.pose_composed_cached(layers.iter().copied(), &mut PoseCache::default());
            let mut cache = PoseCache::default();
            let (palette, native_parts) = rig.pose_palette_composed_cached(layers.iter().copied(), &mut cache);
            assert_eq!(parts, native_parts, "mount transforms must not change");
            assert_eq!(rig.weapon_effect_anchor_layered(1, &layers), rig.cached_weapon_effect_anchor(1, &cache));
            let mut transform = ao_scene::IDENTITY;
            transform[3] = [(ordinal % 10) as f32 * 1.4 - 6.3, 0.0, (ordinal / 10) as f32 * -2.0 + 4.0, 1.0];
            let frame = ActorFrame { id: ordinal as u32 + 1, model: i as u64 + 1, parts, transform,
                always: true, rendering_effect: [0, 1, 2, 4, 5, 6, 7][ordinal % 7], rendering_effect_time: 0.375,
                alpha: if ordinal % 3 == 0 { 0.65 } else { 1.0 }, ..Default::default() };
            gpu_frames.push(ActorFrame { skin_pose: Some(palette), ..frame.clone() });
            cpu_frames.push(ActorFrame { skin: Some(Arc::new(expected)), ..frame });
        }
        gpu.set_actors(gpu_frames);
        // Dispatch/read back every actor's actual production body buffer before rendering the exact same poses.
        for frame in &cpu_frames {
            let expected = frame.skin.as_ref().unwrap();
            if let Some(diff) = difference(&body_bytes(&mut gpu, frame.id, expected.len()), bytemuck::cast_slice(expected.as_slice())) {
                byte_differences.push(format!("step {step} actor {} model {}: {diff}", frame.id, frame.model));
            }
        }
        cpu.set_actors(cpu_frames);
        let actual = pixels(&mut gpu);
        let expected = pixels(&mut cpu);
        let actual_repeat = pixels(&mut gpu);
        let expected_repeat = pixels(&mut cpu);
        assert_ne!(expected, blank, "asset proof must draw visible geometry");
        assert!(difference(&actual, &expected).is_none(), "step {step}: pixels must be identical, no tolerance: {:?}; vertex diagnostics: {byte_differences:?}", difference(&actual, &expected));
        assert!(difference(&actual, &actual_repeat).is_none(), "step {step}: unchanged GPU renderer must repeat the exact image: {:?}", difference(&actual, &actual_repeat));
        assert!(difference(&expected, &expected_repeat).is_none(), "step {step}: unchanged CPU renderer must repeat the exact image: {:?}", difference(&expected, &expected_repeat));
        assert!(difference(&actual_repeat, &expected_repeat).is_none(), "step {step}: repeated GPU/CPU images must remain identical: {:?}", difference(&actual_repeat, &expected_repeat));
        if let Some(previous) = previous { assert_ne!(expected, previous, "sampled animation must change visible pixels"); }
        previous = Some(expected);
    }
    assert!(byte_differences.is_empty(), "GPU/CPU vertex bytes differ (pixels were checked independently): {byte_differences:#?}");
}

/// Controlled offline throughput evidence; correctness remains the separate strict byte/pixel tests above.
#[test]
#[ignore = "installed-client 60-actor CPU/GPU performance measurement; run alone with --nocapture"]
fn native_skin_sixty_actor_performance() {
    use std::time::Instant;
    const WARMUP: usize = 32;
    const SAMPLES: usize = 120;
    let dir = ao_gui::client_dir();
    assert!(dir.join("cd_image/rdb.db").exists(), "performance fixture requires installed client at {}", dir.display());
    let store = ao_rdb::RecordStore::open(&dir).unwrap();
    let mut assets = ActorAssets::new(&store).unwrap();
    let specs = [(5900, Some(40098)), (5907, Some(40681)), (5914, Some(40250)), (5921, None),
        (5927, Some(40630)), (5934, Some(40209)), (5941, Some(40135)), (29902, None)];
    let rigs: Vec<_> = specs.iter().map(|&(id, head)| ActorRig::new(&store, &assets, id, head,
        &PartTextures::new(), &PartLayers::new(), &[(1, 15839), (2, 7796), (5, 26163)]).unwrap()).collect();
    let clips: Vec<_> = rigs.iter().map(|rig| {
        let id = assets.clips(&store, rig.model_id).unwrap().iter().find(|(name, _)| name == "walk").map(|(_, id)| *id)
            .unwrap_or_else(|| compatible_animations(&store, &load_cat_mesh(&store, CHAR_MESH_TYPE, rig.model_id).unwrap()).unwrap()[0].0);
        assets.anim(&store, id).unwrap()
    }).collect();
    let mut cpu = renderer();
    let mut gpu = renderer();
    for (i, rig) in rigs.iter().enumerate() {
        assert!(rig.model().native_skin.is_some());
        gpu.add_actor_model(i as u64 + 1, rig.model());
        let mut model = rig.model().clone();
        model.native_skin = None;
        cpu.add_actor_model(i as u64 + 1, &model);
    }
    // Allocate once, outside both warm-up and measured loops. Normal render shaders and persistent targets.
    let target = |r: &Renderer| {
        let targets = Targets::new(r, 1280, 800);
        let texture = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("native skin performance persistent output"),
            size: wgpu::Extent3d { width: 1280, height: 800, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let view = texture.create_view_tracked(&Default::default());
        (targets, texture, view)
    };
    let (cpu_targets, _cpu_texture, cpu_view) = target(&cpu);
    let (gpu_targets, _gpu_texture, gpu_view) = target(&gpu);
    let camera = Camera::look_at([0.0, 5.0, 17.0].into(), [0.0, 1.0, 0.0].into());
    let mut cpu_cache: Vec<_> = (0..60).map(|_| PoseCache::default()).collect();
    let mut gpu_cache: Vec<_> = (0..60).map(|_| PoseCache::default()).collect();
    // These are CPU wall-clock intervals. wait is completion latency, not a GPU timestamp measurement;
    // render_encode_submit includes the normal Renderer::render queue submission.
    let sample = |native: bool, iteration: usize, r: &mut Renderer, targets: &Targets, view: &wgpu::TextureView, caches: &mut [PoseCache]| {
        let start = Instant::now();
        let mut frames = Vec::with_capacity(60);
        for (ordinal, cache) in caches.iter_mut().enumerate() {
            let i = ordinal % rigs.len();
            let rig = &rigs[i];
            let ms = (iteration as f32 * (1000.0 / 60.0) + (ordinal / rigs.len()) as f32 * 31.0) % clips[i].duration.max(1.0);
            let layers = [AnimLayer { clip: &clips[i], ms, layers: 3, blend: 1.0 }];
            let (skin, skin_pose, parts) = if native {
                let (palette, parts) = rig.pose_palette_composed_cached(layers.iter().copied(), cache);
                (None, Some(palette), parts)
            } else {
                // No precomputed poses: CPU work includes the existing full scalar body skin every frame.
                let (vertices, parts) = rig.pose_composed_cached(layers.iter().copied(), cache);
                (Some(Arc::new(vertices)), None, parts)
            };
            let mut transform = ao_scene::IDENTITY;
            transform[3] = [(ordinal % 10) as f32 * 1.4 - 6.3, 0.0, (ordinal / 10) as f32 * -2.0 + 4.0, 1.0];
            frames.push(ActorFrame { id: ordinal as u32 + 1, model: i as u64 + 1, transform, parts, skin, skin_pose,
                always: true, rendering_effect: [0, 1, 2, 4, 5, 6, 7][ordinal % 7], rendering_effect_time: 0.375,
                alpha: if ordinal % 3 == 0 { 0.65 } else { 1.0 }, ..Default::default() });
        }
        let prepared = Instant::now();
        r.set_actors(frames);
        let uploaded = Instant::now();
        r.render(view, targets, &camera);
        let submitted = Instant::now();
        r.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let completed = Instant::now();
        [prepared.duration_since(start).as_secs_f64() * 1000.0,
            uploaded.duration_since(prepared).as_secs_f64() * 1000.0,
            submitted.duration_since(uploaded).as_secs_f64() * 1000.0,
            completed.duration_since(submitted).as_secs_f64() * 1000.0,
            completed.duration_since(start).as_secs_f64() * 1000.0]
    };
    let mut cpu_samples = Vec::with_capacity(SAMPLES);
    let mut gpu_samples = Vec::with_capacity(SAMPLES);
    for iteration in 0..WARMUP + SAMPLES {
        // Each pair samples identical animation times. Alternate order to avoid a systematic first-run bias.
        let (cpu_times, gpu_times) = if iteration % 2 == 0 {
            let a = sample(false, iteration, &mut cpu, &cpu_targets, &cpu_view, &mut cpu_cache);
            let b = sample(true, iteration, &mut gpu, &gpu_targets, &gpu_view, &mut gpu_cache);
            (a, b)
        } else {
            let b = sample(true, iteration, &mut gpu, &gpu_targets, &gpu_view, &mut gpu_cache);
            let a = sample(false, iteration, &mut cpu, &cpu_targets, &cpu_view, &mut cpu_cache);
            (a, b)
        };
        if iteration >= WARMUP {
            cpu_samples.push(cpu_times);
            gpu_samples.push(gpu_times);
        }
    }
    eprintln!("native_skin_sixty_actor_performance: headless installed assets, 8 rigs/60 actors, 1280x800, {WARMUP} warm-up pairs discarded, {SAMPLES} alternating pairs; no timed PNG/readback; wall milliseconds, not GPU timestamps");
    for (name, values) in [("cpu", &cpu_samples), ("gpu", &gpu_samples)] {
        for (stage, label) in ["pose_prepare", "set_actors_upload", "render_encode_submit", "gpu_wait", "total_wall"].iter().enumerate() {
            let mut sorted: Vec<_> = values.iter().map(|v| v[stage]).collect();
            sorted.sort_by(f64::total_cmp);
            let percentile = |p: usize| sorted[(sorted.len() * p).div_ceil(100).saturating_sub(1)];
            eprintln!("{name} {label}: p50={:.6} p95={:.6}", percentile(50), percentile(95));
        }
    }
}
