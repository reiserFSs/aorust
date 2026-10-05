//! Decodes real character records when the game client is installed; skips cleanly otherwise.

use ao_formats::character::*;
use ao_rdb::RecordStore;
use ao_scene::Scene;

fn store() -> Option<RecordStore> {
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    dir.join("cd_image/rdb.db").exists().then(|| RecordStore::open(&dir).unwrap())
}

/// Fraction of triangles whose counter-clockwise geometric normal agrees with the vertex normals.
fn normal_agreement(scene: &Scene) -> f32 {
    let (mut agree, mut total) = (0, 0);
    for m in &scene.meshes {
        for t in m.submeshes.iter().flat_map(|s| s.indices.as_chunks::<3>().0.iter()) {
            let [a, b, c] = [0, 1, 2].map(|k| m.vertices[t[k] as usize]);
            let e1: [f32; 3] = std::array::from_fn(|k| b.pos[k] - a.pos[k]);
            let e2: [f32; 3] = std::array::from_fn(|k| c.pos[k] - a.pos[k]);
            let cr = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let d: f32 = (0..3).map(|k| cr[k] * (a.normal[k] + b.normal[k] + c.normal[k])).sum();
            if d != 0.0 {
                total += 1;
                agree += usize::from(d > 0.0);
            }
        }
    }
    agree as f32 / total as f32
}

fn positions(s: &Scene) -> Vec<[f32; 3]> {
    s.meshes[0].vertices.iter().map(|v| v.pos).collect()
}

#[test]
fn every_model_and_animation_parses() {
    let Some(store) = store() else { return };
    for ty in [CHAR_MESH_TYPE, CHAR_MESH_LOW_TYPE] {
        for id in store.ids(ty).unwrap() {
            let m = load_cat_mesh(&store, ty, id).unwrap_or_else(|e| panic!("{ty}/{id}: {e:#}"));
            assert!(!m.submeshes.is_empty() && !m.bones.is_empty());
        }
    }
    for id in store.ids(CHAR_ANIM_TYPE).unwrap().into_iter().step_by(4) {
        let a = load_anim(&store, id).unwrap_or_else(|e| panic!("anim {id}: {e:#}"));
        assert!(a.tracks.iter().all(|t| t.rot.windows(2).all(|w| w[0].0 <= w[1].0)), "{id}: key times not sorted");
    }
}

#[test]
fn bind_pose_is_upright_unmirrored_and_textured() {
    let Some(store) = store() else { return };
    // 5900 athrox male, 5927 solitus female (root bone `CollSphere01`), 5941 nanomage female
    for id in [5900, 5927, 5941] {
        let scene = load_character(&store, id).unwrap();
        let ys: Vec<f32> = scene.meshes[0].vertices.iter().map(|v| v.pos[1]).collect();
        let (lo, hi) = ys.iter().fold((f32::MAX, f32::MIN), |(l, h), &y| (l.min(y), h.max(y)));
        assert!(lo > -0.1 && lo < 0.3 && hi > 1.3 && hi < 2.0, "{id}: y range {lo}..{hi}");
        assert!(normal_agreement(&scene) > 0.99, "{id}: winding/normals disagree (mirrored?)");
        assert!(scene.meshes[0].submeshes.iter().any(|s| s.texture.is_some_and(|k| scene.textures.contains_key(&k))));
    }
}

#[test]
fn animation_poses_the_mesh_and_checks_the_skeleton() {
    let Some(store) = store() else { return };
    let bind = positions(&load_character(&store, 5900).unwrap());
    let walk = load_character_posed(&store, 5900, 9382, 0.6).unwrap();
    assert!(normal_agreement(&walk) > 0.97);
    let moved = |a: &[[f32; 3]], b: &[[f32; 3]]| a.iter().zip(b).map(|(p, q)| (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f32>().sqrt()).fold(0.0, f32::max);
    assert!(moved(&bind, &positions(&walk)) > 0.2, "walk cycle barely moves the mesh");
    // the clip loops: one full period later is the same pose
    let again = load_character_posed(&store, 5900, 9382, 0.6 + 2.5).unwrap();
    assert!(moved(&positions(&walk), &positions(&again)) < 1e-3);
    // an animation of a different skeleton is refused
    let other = store.ids(CHAR_ANIM_TYPE).unwrap().into_iter().find(|&a| load_anim(&store, a).unwrap().signature != 0xbd94_5cbe).unwrap();
    assert!(load_character_posed(&store, 5900, other, 0.0).is_err());
}

#[test]
fn head_mounts_above_the_body_and_follows_the_pose() {
    let Some(store) = store() else { return };
    // 5900 + head_athroxmale001; 5914 (opifex) has no skin on its head bone: the mount frame is derived
    for (body, head, pose) in [(5900, 40098, None), (5900, 40098, Some((9382, 0.6))), (5914, 40250, None)] {
        let scene = load_character_with_head(&store, body, head, pose).unwrap();
        assert_eq!(scene.instances.len(), 2);
        let m = scene.instances[1].transform;
        assert!((1.2..2.0).contains(&m[3][1]), "head mount height {}", m[3][1]);
        let head_up = (0..3).map(|c| m[c][1]).collect::<Vec<_>>(); // image of +Y
        assert!(head_up.iter().map(|v| v * v).sum::<f32>() > 0.99, "mount transform is not rigid");
    }
}

#[test]
fn compatible_animations_share_the_skeleton_hash() {
    let Some(store) = store() else { return };
    let mesh = load_cat_mesh(&store, CHAR_MESH_TYPE, 5900).unwrap();
    let clips = compatible_animations(&store, &mesh).unwrap();
    assert!(clips.len() > 800 && clips.iter().any(|c| c.0 == 9382));
}

const PLAYERS: [(Breed, Gender, u32); 7] = [
    (Breed::Atrox, Gender::Male, 5900),
    (Breed::Solitus, Gender::Male, 5907),
    (Breed::Opifex, Gender::Male, 5914),
    (Breed::Nanomage, Gender::Male, 5921),
    (Breed::Solitus, Gender::Female, 5927),
    (Breed::Opifex, Gender::Female, 5934),
    (Breed::Nanomage, Gender::Female, 5941),
];

#[test]
fn player_tables_resolve_models_heads_and_clips() {
    let Some(store) = store() else { return };
    for (breed, gender, id) in PLAYERS {
        assert_eq!(player_model(&store, breed, gender).unwrap(), id, "{breed:?} {gender:?}");
        assert!(!player_heads(&store, breed, gender, Skin::Caucasian).unwrap().is_empty());
    }
    assert!(player_model(&store, Breed::Atrox, Gender::Female).is_err(), "the client has no atrox female");
    let n = |b, g, s| player_heads(&store, b, g, s).unwrap().len();
    assert_eq!(n(Breed::Atrox, Gender::Male, Skin::Caucasian), 41);
    assert_eq!(n(Breed::Solitus, Gender::Male, Skin::Caucasian), 57);
    assert_eq!(n(Breed::Solitus, Gender::Male, Skin::Asian), 9);
    assert_eq!(n(Breed::Nanomage, Gender::Female, Skin::Caucasian), 43);
    // 40098 is the mesh whose file is head_athrox12.abiff
    assert!(player_heads(&store, Breed::Atrox, Gender::Male, Skin::Caucasian).unwrap().contains(&(12, 40098)));

    let find = |model, role: Role| character_animations(&store, model).unwrap().into_iter().find(|c| c.0 == role).map(|c| c.1);
    assert_eq!(find(5900, Role::Walk), Some(10078));
    assert_eq!(find(5900, Role::Sneak), Some(9382)); // 9382 is `athrox_sneakcool`, not the walk
    assert_eq!(find(5900, Role::Run), Some(9386));
    assert_eq!(find(5900, Role::Idle), Some(9992));
    assert_eq!(find(5907, Role::Walk), Some(10191));
    assert_eq!(find(5927, Role::Walk), Some(10162));
    assert_eq!(find(5934, Role::Idle), Some(10135)); // opifex women share the `female` set
    assert_eq!(find(5900, Role::Emote("backflip".into())), Some(9375));
    assert!(role_anim(&store, 5907, &Role::Emote("no-such-emote".into())).is_err());
}

#[test]
fn players_have_skin_not_the_green_placeholder() {
    let Some(store) = store() else { return };
    let green = |t: &ao_scene::Texture| t.rgba.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 255, 0]);
    for (breed, gender, _) in PLAYERS {
        let s = load_player_character(&store, breed, gender, 1, Some((Role::Idle, 0.0))).unwrap();
        assert!(s.instances.len() == 2, "body + head");
        assert!(s.textures.values().all(|t| !green(t)), "{breed:?} {gender:?} still has a green texture");
        assert!(s.textures.keys().any(|k| k.rdb_type == 1010011), "naked skin textures applied");
    }
    for skin in [Skin::Asian, Skin::African] {
        let p = Player { breed: Breed::Solitus, gender: Gender::Male, skin, head: None };
        assert!(load_player(&store, &p, Some((Role::Walk, 0.6))).is_ok());
    }
}

#[test]
fn limbs_stay_attached_with_the_models_own_clip_set() {
    let Some(store) = store() else { return };
    // 41664 `skeleton_solitus` shares the human skeleton hash but has solitus bone lengths: the `male` set fits,
    // the athrox set (9375 `athrox_social-backflip`, 9382) tears the arms and feet off.
    for (id, clip, ok) in [(41664, 10191, true), (41664, 9375, false), (41664, 9382, false), (5907, 10191, true), (5907, 9375, false), (5900, 9375, true)] {
        let gap = pose_detachment(&store, id, clip, 0.6).unwrap();
        assert_eq!(gap < 0.06, ok, "model {id} clip {clip}: gap {gap}");
    }
    // role lookup picks the fitting set for models no set is named after
    assert_eq!(role_anim(&store, 41664, &Role::Walk).unwrap(), 10191);
}
