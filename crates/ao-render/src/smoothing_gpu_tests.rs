//! Production-dispatch regression; run with the renderer's normal headless GPU tests.
use super::*;
use ao_scene::{NativeSkinVertex, SkinBone, Vertex};

fn fixture() -> Scene {
    let positions = [[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.2, 1.0], [-1.0, 0.0, 1.0]];
    let vertices: Vec<_> = positions.into_iter().enumerate().map(|(i, pos)| Vertex {
        pos, normal: [i as f32 * 0.1, 1.0, 0.0], uv: [i as f32 * 0.25, 0.5], color: [0.25, 0.5, 0.75, 1.0],
    }).collect();
    let native = vertices.iter().enumerate().map(|(i, vertex)| NativeSkinVertex {
        local0: [vertex.pos[0], vertex.pos[1], -vertex.pos[2]],
        local1: [vertex.pos[0] + 0.1, vertex.pos[1] - 0.2, -vertex.pos[2] + 0.3],
        weight: 0.375, bone0: if i == 2 { 2 } else { 0 }, bone1: if i == 2 { 3 } else { 1 }, normal: vertex.normal,
    }).collect();
    let mesh = ao_scene::Mesh { vertices, submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None)] };
    Scene { meshes: vec![mesh.clone(), mesh.clone(), mesh], native_skin: Some(Arc::new(native)), ..Default::default() }
}

fn frame() -> ActorFrame {
    ActorFrame { id: 1, model: 1, character: true, always: true,
        part_attractors: vec![None, Some(0), Some(3)], ..Default::default() }
}

fn read_vertices(r: &mut Renderer, smooth: bool) -> Vec<Vertex> {
    let count = if smooth { r.act.models[&1].meshes[0].as_ref().unwrap().smoothing.as_ref().unwrap().vertex_count as usize } else { 4 };
    let size = (count * 48) as u64;
    let readback = r.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoothing regression readback"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    let mut encoder = r.device.create_command_encoder(&Default::default());
    r.encode_actor_skins(&mut encoder);
    let actor = &r.act.actors[&1];
    let buffer = if smooth { &actor.smoothing.as_ref().unwrap().vertices } else { actor.vb.as_ref().unwrap_or(&r.act.models[&1].meshes[0].as_ref().unwrap().vb) };
    encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, size);
    r.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    r.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let result = bytemuck::cast_slice::<u8, Vertex>(&readback.slice(..).get_mapped_range()).to_vec();
    readback.unmap();
    result
}

fn prepare(r: &mut Renderer, camera: Vec3) { r.prepare_actors(camera, -Vec3::Z, &[Vec4::ZERO; 6]); }

#[test]
fn character_smoothing_preserves_native_pose_and_all_endpoint_influences() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r = Renderer::new(&instance, None).expect("smoothing regression requires headless GPU");
    let scene = fixture();
    let topology = character_smoothing::subdivide(&scene.meshes[0]);
    assert!(topology.samples.contains(&[0, 2]), "shared diagonal must exercise Phong projection");
    r.add_actor_model(1, &scene);
    let palette = Arc::new((0..4).map(|i| SkinBone { rows: [
        [1.0, 0.0, 0.0, i as f32 * 0.125], [0.0, 1.0, 0.0, i as f32 * 0.25], [0.0, 0.0, 1.0, i as f32 * -0.0625],
    ] }).collect::<Vec<_>>());
    let mut posed = scene.meshes[0].vertices.clone();
    for (out, native) in posed.iter_mut().zip(scene.native_skin.as_ref().unwrap().iter()) {
        out.pos = std::array::from_fn(|axis| {
            let p0 = native.local0[axis] + palette[native.bone0 as usize].rows[axis][3];
            let p1 = native.local1[axis] + palette[native.bone1 as usize].rows[axis][3];
            let value = native.weight * p0 + (1.0 - native.weight) * p1;
            if axis == 2 { -value } else { value }
        });
        let n = Vec3::from_array(native.normal).normalize();
        out.normal = [n.x, n.y, -n.z];
    }
    let cpu = Arc::new(posed.iter().map(|vertex| Vertex { pos: [vertex.pos[0] + 0.25, vertex.pos[1], vertex.pos[2]], ..*vertex }).collect::<Vec<_>>());
    // Bind pose, native GPU skin, empty-palette fallback, and explicit CPU skin.
    for submitted in [frame(), ActorFrame { skin_pose: Some(palette.clone()), ..frame() },
        ActorFrame { skin_pose: Some(Arc::new(Vec::new())), ..frame() },
        ActorFrame { skin: Some(cpu.clone()), skin_pose: Some(palette), ..frame() }] {
        let expected = if submitted.skin.is_some() { cpu.as_ref().clone() } else if submitted.skin_pose.as_ref().is_some_and(|p| !p.is_empty()) { posed.clone() } else { scene.meshes[0].vertices.clone() };
        r.set_actors(vec![submitted]);
        r.enhanced_settings.character_smoothing = false;
        prepare(&mut r, Vec3::ZERO);
        assert!(r.act.items.iter().all(|item| !item.smoothing));
        let original = read_vertices(&mut r, false);
        for (actual, expected) in original.iter().zip(&expected) {
            for (a, b) in actual.pos.iter().zip(expected.pos) { assert!((a - b).abs() < 1e-6); }
        }
        r.enhanced_settings.character_smoothing = true;
        prepare(&mut r, Vec3::ZERO);
        assert_eq!(r.act.items.iter().map(|item| item.smoothing).collect::<Vec<_>>(), [true, true, false]);
        let smooth = read_vertices(&mut r, true);
        assert_eq!(bytemuck::cast_slice::<_, u8>(&smooth[..4]), bytemuck::cast_slice::<_, u8>(&original));
        for (midpoint, edge) in smooth[4..].iter().zip(&topology.samples) {
            let a = original[edge[0] as usize];
            let b = original[(edge[1] & 0x7fff_ffff) as usize];
            let pa = Vec3::from_array(a.pos);
            let pb = Vec3::from_array(b.pos);
            let na = Vec3::from_array(a.normal).normalize_or_zero();
            let nb = Vec3::from_array(b.normal).normalize_or_zero();
            let mut point = (pa + pb) * 0.5;
            if edge[1] & 0x8000_0000 == 0 { point -= (na * (point - pa).dot(na) + nb * (point - pb).dot(nb)) * 0.5; }
            assert!(Vec3::from_array(midpoint.pos).distance(point) < 1e-5);
            assert!(Vec3::from_array(midpoint.normal).distance((na + nb).normalize_or_zero()) < 1e-5);
            for axis in 0..2 { assert!((midpoint.uv[axis] - (a.uv[axis] + b.uv[axis]) * 0.5).abs() < 1e-6); }
        }
        // Frozen rerender cannot depend on a dirty pose; original storage remains unchanged.
        prepare(&mut r, Vec3::ZERO);
        assert_eq!(bytemuck::cast_slice::<_, u8>(&read_vertices(&mut r, true)), bytemuck::cast_slice::<_, u8>(&smooth));
        assert_eq!(bytemuck::cast_slice::<_, u8>(&read_vertices(&mut r, false)), bytemuck::cast_slice::<_, u8>(&original));
        prepare(&mut r, Vec3::new(15.0, 0.0, 0.0));
        assert!(r.act.items[0].smoothing);
        prepare(&mut r, Vec3::new(15.001, 0.0, 0.0));
        assert!(r.act.items.iter().all(|item| !item.smoothing));
    }
    r.set_actors(vec![ActorFrame { character: false, ..frame() }]);
    prepare(&mut r, Vec3::ZERO);
    assert!(r.act.items.iter().all(|item| !item.smoothing));
    let mut noncat = scene;
    noncat.native_skin = None;
    r.add_actor_model(2, &noncat);
    r.set_actors(vec![ActorFrame { model: 2, ..frame() }]);
    prepare(&mut r, Vec3::ZERO);
    assert!(r.act.items.iter().all(|item| !item.smoothing));
}
