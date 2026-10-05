//! Dungeon room test against the real client data (skips without the client).
use std::path::PathBuf;

use ao_formats::playfield::{load_playfield, zone_locator};
use ao_rdb::RecordStore;

#[test]
fn dungeon_spawn_is_inside_its_room() {
    let Some(home) = std::env::var_os("HOME") else { return };
    let dir = PathBuf::from(home).join("Games/ProjectRubiKa/client");
    if !dir.join("cd_image/rdb.db").exists() {
        return;
    }
    let store = RecordStore::open(&dir).unwrap();
    // every dungeon playfield: the entry spot (eye 1.7 m above the floor of a room) lies inside a room by IsPosInside
    let (mut inside, mut total) = (0, 0);
    for (id, _) in ao_formats::playfield::list_playfields(&store).unwrap() {
        let zl = zone_locator(&store, id).unwrap();
        if !zl.is_dungeon() {
            continue;
        }
        let Ok(scene) = load_playfield(&store, &dir, id) else { continue };
        let Some(spawn) = scene.spawn else { continue };
        total += 1;
        if zl.room_at(spawn).is_some() {
            inside += 1;
        }
    }
    eprintln!("dungeon entry spots inside a room (IsPosInside): {inside}/{total}");
    assert!(total > 200 && inside == total, "{inside}/{total}");
}
