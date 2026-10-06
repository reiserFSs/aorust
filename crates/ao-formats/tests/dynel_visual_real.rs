//! `dynel_visual` against the real rdb and the dynels of `docs/captures/zone_ithaca.rec` (skips without the client).
//! The stat arrays are copied from the capture (decoded by ao-net's `n3::world` / `n3::dynel` tests).
use std::path::PathBuf;

use ao_formats::character::{NameTable, CHAR_MESH_TYPE};
use ao_formats::dynel_visual::*;
use ao_formats::mesh::MESH_TYPE;
use ao_rdb::RecordStore;

fn open() -> Option<(RecordStore, NameTable)> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    if !dir.join("cd_image/rdb.db").exists() {
        return None;
    }
    let store = RecordStore::open(&dir).unwrap();
    let names = NameTable::load(&store).unwrap();
    Some((store, names))
}

#[test]
fn captured_vending_machine() {
    let Some((store, names)) = open() else { return };
    // VendingMachineFullUpdate {0xC75B, 100}: stats as sent
    let stats = [(0, 0x8002_3203u32 as i32), (23, 248_371), (701, 0), (702, 0), (703, 0), (412, 1), (501, 2), (500, 0), (12, 93_117)];
    let t = item_template(&store, static_instance(&stats).unwrap()).unwrap().unwrap();
    assert_eq!((t.kind, dynel_class(t.kind), t.name.as_deref()), (0xC75B, Some("VendingMachine"), Some("Newcomer's Nano Programs")));
    assert_eq!(t.stat(stat::MESH), Some(93_117)); // the message repeats the record's Mesh
    let v = item_visual(&store, &names, &stats).unwrap();
    assert_eq!((v.mesh, v.cat_mesh, v.scale, v.visible), (Some(93_117), None, 1.0, true));
    assert_eq!(names.name(MESH_TYPE, 93_117), Some("shop_neutral_nano_general.abiff"));
    ao_formats::mesh::load_mesh(&store, 93_117).unwrap();
}

#[test]
fn captured_weapons() {
    let Some((store, names)) = open() else { return };
    // (StaticInstance, parent) of the 10 WeaponItemFullUpdates: ICC guards' rifles and Stanko's shotguns
    for (si, mesh, mesh_name, item) in [
        (265_090u32, 262_556u32, "EP3_assault_rifle_03.abiff", "Ofab Shark Mk 5"),
        (0x3ca19, 7796, "weapon_shotgunsmall01.abiff", "Polished Eliminator"),
    ] {
        let stats = [(0, 0x403), (23, si as i32), (0x2bd, 25), (0x2be, si as i32), (0x19c, 1), (0x1a, 0), (0xd4, 25), (0x1a4, 2)];
        let t = item_template(&store, static_instance(&stats).unwrap()).unwrap().unwrap();
        assert_eq!((t.kind, t.name.as_deref()), (0xC74A, Some(item)));
        assert_eq!(weapon_mesh(&t), Some(mesh));
        assert_eq!(names.name(MESH_TYPE, mesh), Some(mesh_name));
        // the dynel's own look (when lying on the ground) is the generic pickup box
        let v = item_visual(&store, &names, &stats).unwrap();
        assert_eq!((v.mesh, v.cat_mesh), (Some(default_mesh(&names).unwrap()), None));
        assert_eq!(names.name(MESH_TYPE, v.mesh.unwrap()), Some(DEFAULT_MESH_NAME));
    }
    assert_eq!((weapon_attractor_place(6), weapon_attractor_place(8), weapon_attractor_place(3)), (Some(1), Some(2), None));
}

#[test]
fn captured_corpses() {
    let Some((store, names)) = open() else { return };
    // (CATMesh, MonsterScale, HeadMesh or 0) of the 7 corpses of the capture, in message order
    let corpses = [
        (45_857, 92, "junkbot.cir"),
        (15_222, 181, "cutecreature.cir"),
        (15_222, 181, "cutecreature.cir"),
        (15_222, 90, "cutecreature.cir"),
        (15_222, 90, "cutecreature.cir"),
        (23_353, 181, "giant_snake.cir"),
        (45_857, 93, "junkbot.cir"),
    ];
    for (cat, scale, name) in corpses {
        let stats = [(0, 1_579_013), (23, 0), (701, 0), (702, 0), (703, 0), (412, 1), (360, scale), (223, 0), (59, 1), (4, 6), (89, 1), (415, 50000), (416, 1_025_286), (42, cat), (61, 6), (34, 600), (8, 18000)];
        let cloth = [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0)];
        let c = corpse_visual(&stats, &cloth, &[]).unwrap();
        assert_eq!((c.cat_mesh, c.scale, c.head_mesh), (cat as u32, scale as f32 / 100.0, None));
        assert_eq!(names.name(CHAR_MESH_TYPE, c.cat_mesh), Some(name));
        assert!(store.get(CHAR_MESH_TYPE, c.cat_mesh).unwrap().is_some());
        // a corpse is not an item: no template (StaticInstance 0), CATMesh alone selects the model
        let v = item_visual(&store, &names, &stats).unwrap();
        assert_eq!((v.mesh, v.cat_mesh), (None, Some(cat as u32)));
    }
}

#[test]
fn every_item_template_and_placed_dynel_parses() {
    let Some((store, names)) = open() else { return };
    let default = default_mesh(&names).unwrap();
    let meshes: std::collections::HashSet<u32> = store.ids(MESH_TYPE).unwrap().into_iter().collect();
    let (mut n, mut with_mesh, mut weapons, mut bad_weapon) = (0, 0, 0, 0);
    for id in store.ids(ITEM_TEMPLATE_TYPE).unwrap() {
        let t = item_template(&store, id).unwrap().unwrap();
        n += 1;
        if let Some(m) = t.stat(stat::MESH).filter(|&m| m > 0) {
            with_mesh += 1;
            assert!(meshes.contains(&(m as u32)), "template {id}: Mesh {m} is not in rdb 1010001");
        }
        if let Some(m) = weapon_mesh(&t) {
            weapons += 1;
            bad_weapon += !meshes.contains(&m) as u32;
        }
    }
    assert_eq!((n, with_mesh), (119_540, 107_607));
    // 3 weapon templates name a mesh that the client data does not contain
    assert_eq!((weapons, bad_weapon), (10_922, 3));

    // playfield 4582 (the capture's zone): a door and an interactive billboard
    let d = placed_dynels(&store, 4582).unwrap();
    assert_eq!(d.iter().map(|d| (d.kind, d.template)).collect::<Vec<_>>(), [(0xC748, 41_565), (0xC73D, 41_560)]);
    assert_eq!(d[0].instance, 0xC000_11E6);
    let door = placed_visual(&store, &names, &d[0]).unwrap().unwrap();
    let board = placed_visual(&store, &names, &d[1]).unwrap().unwrap();
    // the blob's Mesh stat overrides the template (door 41798 -> 245910) or supplies it (billboard has none)
    assert_eq!((door.mesh, board.mesh), (Some(245_910), Some(258_365)));
    assert_ne!(board.mesh, Some(default));
    assert!(names.name(MESH_TYPE, 258_365).is_some());

    // all 601 playfield records parse; every template of every dynel is known except 34
    let (mut total, mut unknown, mut no_ribosome) = (0, 0, 0);
    for pf in store.ids(PLAYFIELD_DYNELS_TYPE).unwrap() {
        for d in placed_dynels(&store, pf).unwrap() {
            total += 1;
            assert_eq!(d.playfield, pf);
            if dynel_class(d.kind).is_none() {
                // 0xC73C / 0xC749 / 0xC770 are DbObject kinds without a ribosome: `CreateFromTemplate` makes no dynel
                assert!(matches!(d.kind, 0xC73C | 0xC749 | 0xC770), "playfield {pf}: kind {:#x}", d.kind);
                no_ribosome += 1;
                continue;
            }
            if d.template != 0 {
                let t = item_template(&store, d.template).unwrap();
                unknown += t.is_none() as u32;
                if t.is_some() {
                    let v = placed_visual(&store, &names, &d).unwrap().unwrap();
                    assert!(v.mesh.is_some_and(|m| meshes.contains(&m)) || v.cat_mesh.is_some(), "playfield {pf} template {}", d.template);
                }
            }
        }
    }
    assert_eq!((total, no_ribosome), (12_509, 29));
    assert!(unknown <= 34);
}

/// Door templates carry the open / close sound lists (keys 0x83 / 0x84, fallbacks 0x64 / 0x66), docs/zone/doors.md §5.
#[test]
fn door_templates_have_open_and_close_sounds() {
    let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")).filter(|d| d.join("cd_image/rdb.db").exists()) else { return };
    let store = ao_rdb::RecordStore::open(&dir).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for pf in store.ids(1000026).unwrap() {
        for d in ao_formats::dynel_visual::placed_dynels(&store, pf).unwrap() {
            if matches!(d.kind, 0xC748 | 0xDAC6 | 0xC73A) && d.template != 0 {
                seen.insert(d.template);
            }
        }
    }
    let (mut open, mut close, mut keys) = (0, 0, std::collections::BTreeMap::<u32, usize>::new());
    for t in &seen {
        let Some(t) = ao_formats::dynel_visual::item_template(&store, *t).unwrap() else { continue };
        open += usize::from(t.sounds.iter().any(|s| s.0 == 0x83 || s.0 == 0x64));
        close += usize::from(t.sounds.iter().any(|s| s.0 == 0x84 || s.0 == 0x66));
        for s in &t.sounds {
            *keys.entry(s.0).or_default() += 1;
        }
    }
    eprintln!("{} door templates, {open} with an open sound, {close} with a close sound, keys {keys:?}", seen.len());
    assert!(open > 10 && close > 10);
}
