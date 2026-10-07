//! Decodes real records when the game client is installed; skips cleanly otherwise.

use ao_formats::mesh::{load_mesh, MESH_LOW_TYPE, MESH_TYPE};
use ao_rdb::RecordStore;

fn store() -> Option<RecordStore> {
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    dir.join("cd_image/rdb.db").exists().then(|| RecordStore::open(&dir).unwrap())
}

#[test]
fn sampled_static_mesh_records_decode() {
    let Some(store) = store() else { return };
    let mut geometry = 0;
    for ty in [MESH_TYPE, MESH_LOW_TYPE] {
        // every 20th id keeps the debug-build runtime reasonable; `examples/mesh_stats` covers all ids
        for id in store.ids(ty).unwrap().into_iter().step_by(20) {
            let mut scene = ao_scene::Scene::default();
            let idx = ao_formats::mesh::decode_record_into(&store, ty, id, &mut scene)
                .unwrap_or_else(|e| panic!("{ty}/{id}: {e:#}"))
                .unwrap();
            let m = &scene.meshes[idx];
            geometry += usize::from(!m.vertices.is_empty());
            for s in &m.submeshes {
                assert!(s.indices.iter().all(|&i| (i as usize) < m.vertices.len()), "{ty}/{id}: index out of range");
                assert!(s.texture.is_none_or(|k| scene.textures.contains_key(&k)), "{ty}/{id}: texture key without texture");
            }
        }
    }
    assert!(geometry > 400);
}

#[test]
fn building_with_cutout_masts() {
    let Some(store) = store() else { return };
    let scene = load_mesh(&store, 3722).unwrap();
    assert_eq!(scene.instances.len(), 1);
    let m = &scene.meshes[0];
    assert!(m.submeshes.iter().any(|s| s.blend == ao_scene::Blend::AlphaTest && s.texture.is_some()));
    assert!(m.submeshes.iter().any(|s| s.blend == ao_scene::Blend::Opaque));
    // upright building: taller than wide, standing on y ~ 0 (min y near the ground)
    let (min_y, max_y) = m.vertices.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.pos[1]), b.max(v.pos[1])));
    assert!(max_y - min_y > 5.0 && min_y.abs() < 5.0, "y range {min_y}..{max_y}");
}

#[test]
fn materials_carry_opacity_blend_and_cull() {
    let Some(store) = store() else { return };
    // 9918: one flat-colour (untextured) submesh whose `diff` is pink in the data; the D3D material stays white
    let s = &load_mesh(&store, 9918).unwrap().meshes[0].submeshes;
    assert!(s.iter().any(|s| s.texture.is_none()) && s.iter().all(|s| s.base_color[..3] == [1.0; 3]));
    // 2175: translucent submesh (opacity 0.35, alpha blend) next to opaque ones
    let s = &load_mesh(&store, 2175).unwrap().meshes[0].submeshes;
    assert!(s.iter().any(|s| s.blend == ao_scene::Blend::AlphaBlend && (s.base_color[3] - 0.35).abs() < 1e-3));
    assert!(s.iter().any(|s| s.blend == ao_scene::Blend::Opaque));
    // 3722: culled walls plus two-sided cutout masts
    let s = &load_mesh(&store, 3722).unwrap().meshes[0].submeshes;
    assert!(s.iter().any(|s| !s.two_sided) && s.iter().any(|s| s.two_sided));
}

#[test]
fn glow_and_emissive_fields_are_emitted() {
    let Some(store) = store() else { return };
    let (mut glow, mut emissive) = (0, 0);
    for id in store.ids(MESH_TYPE).unwrap().into_iter().step_by(10) {
        let mut scene = ao_scene::Scene::default();
        let idx = ao_formats::mesh::decode_record_into(&store, MESH_TYPE, id, &mut scene).unwrap().unwrap();
        for s in &scene.meshes[idx].submeshes {
            assert!(!s.glow_mask || (s.blend == ao_scene::Blend::Opaque && s.texture.is_some()), "{id}: glow mask on a non-opaque/untextured submesh");
            glow += usize::from(s.glow_mask);
            emissive += usize::from(s.emissive != [0.0; 3]);
        }
    }
    assert!(glow > 100 && emissive > 100, "glow {glow}, emissive {emissive}");
}

/// The animation matrix of every node at time 0 is the `anim_matrix` the static decode bakes in (doors: docs/zone/doors.md §3):
/// the rig posed at 0 reproduces the static vertices, and the door meshes move when posed at their total time. The one exception is
/// mesh 224248, whose node 1 repeats its `local_rot` in its rotation key: `M(t) * R(local_rot)` (the engine's formula, randy31
/// `UpdateWorldMatrix` @0x10044fa2) applies it twice once the animation runs.
#[test]
fn door_meshes_pose_to_the_static_decode_at_time_zero() {
    let Some(store) = store() else { return };
    let mut meshes = std::collections::BTreeSet::new();
    for pf in store.ids(1000026).unwrap() {
        for d in ao_formats::dynel_visual::placed_dynels(&store, pf).unwrap() {
            if !matches!(d.kind, 0xC748 | 0xDAC6 | 0xC73A) || d.template == 0 {
                continue;
            }
            let tpl = ao_formats::dynel_visual::item_template(&store, d.template).unwrap();
            let stats = ao_formats::dynel_visual::effective_stats(tpl.as_ref(), &ao_formats::dynel_visual::blob_stats(&d.blob).unwrap_or_default());
            if let Some(m) = ao_formats::dynel_visual::get(&stats, 12).filter(|&m| m > 0) {
                meshes.insert(m as u32);
            }
        }
    }
    assert!(meshes.len() > 50, "{} door meshes", meshes.len());
    let (mut animated, mut mismatched) = (0, vec![]);
    for id in meshes {
        let Some(rig) = ao_formats::mesh::NodeRig::load(&store, id).unwrap() else { continue };
        let mut scene = ao_scene::Scene::default();
        if ao_formats::mesh::decode_mesh_into(&store, id, &mut scene).unwrap().is_none() {
            continue;
        }
        let rest = scene.meshes[0].vertices.clone();
        assert_eq!(rig.vertex_count(), rest.len(), "mesh {id}");
        let mut posed = rest.clone();
        rig.pose(0.0, &mut posed);
        if rest.iter().zip(&posed).any(|(a, b)| (0..3).any(|k| (a.pos[k] - b.pos[k]).abs() > 2e-3)) {
            mismatched.push(id);
            continue;
        }
        rig.pose(rig.total_time(), &mut posed);
        animated += usize::from(rest.iter().zip(&posed).any(|(a, b)| (0..3).any(|k| (a.pos[k] - b.pos[k]).abs() > 0.05)));
    }
    assert!(animated > 20, "{animated} door meshes move");
    assert_eq!(mismatched, [224248]);
}

#[test]
fn authored_3025_rigid_frames_match_vertex_sampler_and_native_timing() {
    let Some(store) = store() else { return };
    let names = ao_formats::character::NameTable::load(&store).unwrap();
    for name in ["hoverboard_c_fx01.abiff"] {
        let id = names.id(MESH_TYPE, name).unwrap();
        assert_eq!(id, 284022);
        let Some((scene, mut rig)) = ao_formats::mesh::load_animated_mesh(&store, id).unwrap() else { panic!("{name}: missing authored frame animation") };
        assert_eq!(rig.total_time().to_bits(), 0x3fd55558, "{name}/{id}");
        let reference = ao_formats::mesh::NodeRig::load(&store, id).unwrap().unwrap();
        let mut posed = load_mesh(&store, id).unwrap().meshes.remove(0).vertices;
        let mut parts = Vec::new();
        for fraction in [0.0, 0.25, 0.5, 0.75, 1.0, 1.25] {
            let time = fraction * rig.total_time();
            rig.pose_parts(time, &mut parts);
            reference.pose(time, &mut posed);
            assert_eq!(parts.len(), scene.meshes.len());
            let mut index = 0;
            for (mesh, matrix) in scene.meshes.iter().zip(&parts) {
                for vertex in &mesh.vertices {
                    for (axis, expected) in posed[index].pos.iter().enumerate() {
                        let actual = (0..3).map(|k| vertex.pos[k] * matrix[k][axis]).sum::<f32>() + matrix[3][axis];
                        assert!((actual - expected).abs() < 2e-3, "{name}/{id} t={time} vertex={index} axis={axis}");
                    }
                    index += 1;
                }
            }
            assert_eq!(index, posed.len());
        }
        rig.pose_parts(0.25 * rig.total_time(), &mut parts);
        let first = parts.clone();
        rig.pose_parts(1.25 * rig.total_time(), &mut parts);
        for (a, b) in first.iter().flatten().flatten().zip(parts.iter().flatten().flatten()) {
            assert!((a - b).abs() < 2e-4, "{name}/{id}: native time wrapping");
        }
    }
}

#[test]
fn authored_3025_circle_and_shoulder_uv_keys_scroll_without_reallocating() {
    let Some(store) = store() else { return };
    for (id, total) in [(272411, 0.8), (271013, 0.8), (269941, 3.333333)] {
        let (scene, mut rig) = ao_formats::mesh::load_animated_mesh(&store, id).unwrap().unwrap();
        assert!((rig.total_time() - total).abs() < 1e-6, "record {id}");
        let (mut uvs, mut visible, mut parts) = (Vec::new(), Vec::new(), Vec::new());
        rig.pose_parts(0.0, &mut parts);
        rig.pose_visuals(0.0, &mut uvs, &mut visible);
        assert_eq!(uvs.len(), scene.meshes.len());
        assert!(uvs.iter().all(|uv| *uv == [1.0, 1.0, 0.0, 0.0]));
        assert!(visible.iter().all(|visible| *visible));
        let pointers = (uvs.as_ptr(), visible.as_ptr(), parts.as_ptr());
        rig.pose_visuals(total * 0.5, &mut uvs, &mut visible);
        assert!(uvs.iter().any(|uv| (uv[2] - 0.5).abs() < 1e-3), "record {id}: no authored UV scroll");
        for tick in 0..100 {
            let time = tick as f32 * total / 100.0;
            rig.pose_parts(time, &mut parts);
            rig.pose_visuals(time, &mut uvs, &mut visible);
            assert_eq!((uvs.as_ptr(), visible.as_ptr(), parts.as_ptr()), pointers);
        }
    }
}
