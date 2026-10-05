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
fn materials_carry_colour_blend_and_cull() {
    let Some(store) = store() else { return };
    // 9918: one flat-colour (untextured) submesh, red/pink diffuse
    let s = &load_mesh(&store, 9918).unwrap().meshes[0].submeshes;
    assert!(s.iter().any(|s| s.texture.is_none() && s.base_color[..3] != [1.0; 3]));
    // 2175: translucent submesh (opacity 0.35, alpha blend) next to opaque ones
    let s = &load_mesh(&store, 2175).unwrap().meshes[0].submeshes;
    assert!(s.iter().any(|s| s.blend == ao_scene::Blend::AlphaBlend && (s.base_color[3] - 0.35).abs() < 1e-3));
    assert!(s.iter().any(|s| s.blend == ao_scene::Blend::Opaque));
    // 3722: culled walls plus two-sided cutout masts
    let s = &load_mesh(&store, 3722).unwrap().meshes[0].submeshes;
    assert!(s.iter().any(|s| !s.two_sided) && s.iter().any(|s| s.two_sided));
}
