//! Decodes real character records when the game client is installed; skips cleanly otherwise.

use ao_formats::character::*;
use ao_formats::character::actor::{ActorAssets, ActorRig, PlayerLook};
use ao_formats::texture::load_texture;
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
fn concurrent_model_clips_and_actor_caches() {
    let Some(probe) = store() else { return };
    drop(probe);
    let start = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                // Each loader owns its SQLite connection and non-Sync ActorAssets.
                start.wait();
                let store = store().unwrap();
                let mut assets = ActorAssets::new(&store).unwrap();
                for (model, head) in [(5900, 40098), (5914, 40250)] {
                    let clips = model_clips(&store, model).unwrap();
                    let cached = assets.clips(&store, model).unwrap();
                    assert_eq!(*cached, clips);
                    let rig = ActorRig::new(&store, &assets, model, Some(head), &Default::default(), &Default::default(), &[]).unwrap();
                    for _ in 0..8 {
                        assert_eq!(model_clips(&store, model).unwrap(), clips);
                        assert!(std::sync::Arc::ptr_eq(&cached, &assets.clips(&store, model).unwrap()));
                        let anim = assets.role(&store, model, &Role::Walk).unwrap().unwrap();
                        assert_eq!(anim.source_id, clips.iter().find(|c| c.0 == "walk").unwrap().1);
                        assert_eq!(anim.signature, rig.cat().signature);
                        let (vertices, mounts) = rig.pose(Some((&anim, 0.6)));
                        assert!(vertices.iter().all(|v| v.pos.iter().all(|n| n.is_finite())));
                        assert!(mounts.iter().flatten().flatten().all(|n| n.is_finite()));
                    }
                }
            });
        }
    });
}

const COMPOSITE: u32 = 0x4000_0000;

/// (skin key, texture) of the composite on body material `part` (skin id → key `COMPOSITE | skin`).
fn slot<'a>(s: &'a ao_scene::Scene, names: &NameTable, skin_name: &str) -> Option<(ao_scene::TextureKey, &'a ao_scene::Texture)> {
    let id = names.id(1010011, skin_name)?;
    s.textures.iter().find(|(k, _)| (k.rdb_type, k.id) == (1010011, id) || k.rdb_type == COMPOSITE | id).map(|(k, t)| (*k, t))
}

#[test]
fn unequipped_players_wear_the_model_texture_over_the_skin_on_every_slot_and_build() {
    let Some(store) = store() else { return };
    let names = NameTable::load(&store).unwrap();
    let green = |t: &ao_scene::Texture| t.rgba.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 255, 0]);
    for (breed, gender, model) in PLAYERS {
        let s = load_player_character(&store, breed, gender, 1, Some((Role::Idle, 0.0))).unwrap();
        assert!(s.instances.len() == 2, "body + head");
        assert!(s.textures.values().all(|t| !green(t)), "{breed:?} {gender:?} still has a green texture");
        // five composites, none of the bare model textures reaches the body
        assert_eq!(s.textures.keys().filter(|k| k.rdb_type & COMPOSITE != 0).count(), 5, "{breed:?} {gender:?}");
        assert!(!s.meshes[0].submeshes.iter().any(|m| m.texture.is_some_and(|k| k.rdb_type == 1010004)), "body slot with the bare model texture");
        // the overlay is the model's own `body` texture
        let mesh = load_cat_mesh(&store, CHAR_MESH_TYPE, model).unwrap();
        let own = mesh.parts.iter().find(|p| p.name == "body").unwrap().texture;
        assert!(s.textures.keys().any(|k| k.rdb_type & COMPOSITE != 0 && k.id == own), "{breed:?} {gender:?} body is not its default texture");
        for build in [0, 2] {
            let p = Player::new(breed, gender, Skin::Caucasian, Some(1));
            let s = load_player_build(&store, &p, build, None).unwrap();
            assert_eq!(s.textures.keys().filter(|k| k.rdb_type & COMPOSITE != 0).count(), 5, "{breed:?} {gender:?} build {build}");
        }
    }
    // the all-green hands show the skin everywhere; solitus ethnicity picks the race's skin
    let s = load_player_character(&store, Breed::Atrox, Gender::Male, 1, None).unwrap();
    let skin = load_texture(&store, ao_scene::TextureKey { rdb_type: 1010011, id: names.id(1010011, "hands_athroxmale_naked.png").unwrap() }).unwrap().unwrap();
    assert_eq!(slot(&s, &names, "hands_athroxmale_naked.png").unwrap().1.rgba, skin.rgba);
    for skin in [Skin::Asian, Skin::African] {
        let p = Player::new(Breed::Solitus, Gender::Male, skin, None);
        let s = load_player(&store, &p, Some((Role::Walk, 0.6))).unwrap();
        assert_eq!(s.textures.keys().filter(|k| k.rdb_type & COMPOSITE != 0).count(), 5);
        let n = format!("hands_solitusmale_{}_naked.png", if skin == Skin::Asian { "asian" } else { "african" });
        assert!(slot(&s, &names, &n).is_some(), "{n}");
    }
}

#[test]
fn worn_cloth_replaces_the_models_own_texture_over_the_skin() {
    let Some(store) = store() else { return };
    let names = NameTable::load(&store).unwrap();
    let underwear = names.id(1010004, "body_mens-underwear3.png").unwrap();
    let mut p = Player::new(Breed::Solitus, Gender::Male, Skin::Caucasian, Some(1));
    let bare = load_player(&store, &p, None).unwrap();
    p.equipment.wear(ClothPart::Body, underwear);
    let s = load_player(&store, &p, None).unwrap();
    let name = "body_solitusmale_caucation_naked.png";
    let (k, worn) = slot(&s, &names, name).unwrap();
    assert!(k.rdb_type & COMPOSITE != 0 && k.id == underwear, "composite key of the worn texture");
    let (_, base) = slot(&bare, &names, name).unwrap();
    assert_ne!(base.rgba, worn.rgba);
    // no pure-green texel survives the key
    assert!(worn.rgba.as_chunks::<4>().0.iter().all(|p| p[..3] != [0, 255, 0]));
}

#[test]
fn cached_character_renders_with_its_cloth() {
    let Some(store) = store() else { return };
    let names = NameTable::load(&store).unwrap();
    let mut c = CachedCharacter { id: 1, time: 1, mesh_id: 5907, head_id: 40681, breed: 1, sex: 2, fatness: 1, ..Default::default() };
    let mut e = Equipment::default();
    e.wear(ClothPart::Legs, names.id(1010004, "legs_mens-underwear3.png").unwrap());
    c.set_equipment(&e);
    let s = load_cached_character(&store, &c, Some((Role::Idle, 0.0))).unwrap();
    assert_eq!(s.instances.len(), 2);
    let (k, _) = slot(&s, &names, "legs_solitusmale_caucation_naked.png").unwrap();
    assert_eq!(k.id, names.id(1010004, "legs_mens-underwear3.png").unwrap());
}

#[test]
fn cached_meshes_follow_the_rigs_head_hands_and_other_attractors() {
    let Some(store) = store() else { return };
    let assets = ActorAssets::new(&store).unwrap();
    let mut c = CachedCharacter { mesh_id: 5907, head_id: 40629, breed: 1, sex: 2, fatness: 1, ..Default::default() };
    // Head override and captured rifle / dual-shotgun flags (zone_wear_rifle_borealis.rec, dynel.md §1.3).
    c.meshes = vec![
        MeshEntry { attractor: 0, flags: 4, mesh_id: 40681, ..Default::default() },
        MeshEntry { attractor: 1, flags: 2, mesh_id: 15839, ..Default::default() },
        MeshEntry { attractor: 2, flags: 2, mesh_id: 7796, ..Default::default() },
        MeshEntry { attractor: 5, flags: 0, mesh_id: 26163, ..Default::default() },
        MeshEntry { attractor: -1, mesh_id: 15839, ..Default::default() },
        MeshEntry { attractor: 1, mesh_id: 0, ..Default::default() },
    ];
    let saved = c.clone();
    let look = PlayerLook { breed: Breed::Solitus, gender: Gender::Male, skin: Skin::Caucasian, build: 1, head: Some(40681), equipment: Equipment::default() };
    let rig = ActorRig::player(&store, &assets, &look, &[(1, 15839), (2, 7796), (5, 26163)]).unwrap();
    let anim = load_anim(&store, role_anim(&store, 5907, &Role::Walk).unwrap()).unwrap();
    let mut poses = Vec::new();
    for time in [0.0, 0.6] {
        let s = load_cached_character(&store, &c, Some((Role::Walk, time))).unwrap();
        let (vertices, mounts) = rig.pose(Some((&anim, (time * 1000.0).rem_euclid(anim.duration))));
        assert_eq!(s.instances.len(), 5, "body, overridden head, both hands and non-hand equipment");
        assert_eq!(s.meshes[0].vertices.iter().map(|v| v.pos).collect::<Vec<_>>(), vertices.iter().map(|v| v.pos).collect::<Vec<_>>());
        for (i, instance) in s.instances.iter().enumerate() {
            assert_eq!(instance.mesh, i);
            assert_eq!(instance.transform, mounts[i], "mount {i} at {time}s");
        }
        poses.push(mounts);
    }
    assert_ne!(poses[0][2], poses[1][2], "weapon follows the animated hand");
    let bind = load_cached_character(&store, &c, None).unwrap();
    assert_eq!(bind.instances.iter().map(|i| i.transform).collect::<Vec<_>>(), rig.pose(None).1);
    assert_eq!(c, saved, "head flags and cached appearance remain intact");
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

/// `FUN_1011d368` (GUI 0x1011d368): insertion-order head table; counts/orders measured against the client's rdb.
#[test]
fn head_table_follows_the_clients_builder() {
    let Some(store) = store() else { return };
    let t = |b, g, e| head_table(&store, b, g, e).unwrap();
    let nums = |v: &[HeadEntry], s| v.iter().filter(|h| h.skin == s).map(|h| h.num).collect::<Vec<_>>();
    // ExpansionFlags 0: ranges 0..30 (athrox, nano, opifex male), 0..32 minus #30 (opifex female), 0..50 / 0..51 (solitus)
    let a = t(Breed::Atrox, Gender::Male, 0);
    assert_eq!(nums(&a, Skin::Caucasian), (0..30).collect::<Vec<_>>());
    assert_eq!(t(Breed::Atrox, Gender::Female, 0), a, "atrox is always the male table");
    assert_eq!(a[12], HeadEntry { num: 12, mesh: 40098, skin: Skin::Caucasian });
    let of = t(Breed::Opifex, Gender::Female, 0);
    assert_eq!(nums(&of, Skin::Caucasian), (0..32).filter(|&n| n != 30).collect::<Vec<_>>());
    // solitus: caucasian by number (skip list removes gaps), then african, then asian (not asian first)
    let sm = t(Breed::Solitus, Gender::Male, 0);
    let skins: Vec<Skin> = sm.iter().map(|h| h.skin).collect();
    let (first_african, first_asian) = (skins.iter().position(|&s| s == Skin::African).unwrap(), skins.iter().position(|&s| s == Skin::Asian).unwrap());
    assert!(first_african < first_asian && skins[..first_african].iter().all(|&s| s == Skin::Caucasian));
    assert_eq!(nums(&sm, Skin::African), (0..9).collect::<Vec<_>>());
    assert_eq!(nums(&sm, Skin::Asian), (0..6).collect::<Vec<_>>());
    assert!(nums(&sm, Skin::Caucasian).iter().all(|n| *n < 51 && ![3, 4, 10, 11, 12, 14, 28, 29, 31, 32, 34, 37, 41, 47].contains(n)));
    // the Shadowlands flag raises the ranges
    assert!(t(Breed::Solitus, Gender::Female, 2).len() > t(Breed::Solitus, Gender::Female, 0).len());
    assert_eq!(nums(&t(Breed::Solitus, Gender::Male, 2), Skin::Asian).len(), 9);
    // every entry's mesh id is the record named by `head_<…>NN.abiff`
    let names = NameTable::load(&store).unwrap();
    for h in &sm {
        assert!(names.name(1010001, h.mesh).is_some_and(|n| n.starts_with("head_solitusmale") && n.ends_with(&format!("{:02}.abiff", h.num))));
    }
}
