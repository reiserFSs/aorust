//! Loads real playfields when the game client is installed; skips cleanly otherwise.

use ao_formats::playfield::{list_playfields, load_playfield_report};
use ao_rdb::RecordStore;

fn setup() -> Option<(RecordStore, std::path::PathBuf)> {
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    dir.join("cd_image/rdb.db").exists().then(|| (RecordStore::open(&dir).unwrap(), dir))
}

#[test]
fn playfield_list_has_known_names() {
    let Some((store, _)) = setup() else { return };
    let list = list_playfields(&store).unwrap();
    assert!(list.len() > 500);
    assert!(list.contains(&(566, "Newland City".to_string())));
    assert!(list.iter().any(|(id, n)| *id == 705 && n == "Omni1 Entertainment"));
}

#[test]
fn outdoor_city_has_terrain_and_buildings() {
    let Some((store, dir)) = setup() else { return };
    let (scene, r) = load_playfield_report(&store, &dir, 566).unwrap();
    assert_eq!(r.terrain_cells, 150 * 150);
    assert!(r.statels > 1000 && r.failed_meshes == 0);
    assert!(scene.spawn.is_some() && !scene.textures.is_empty());
    for m in &scene.meshes {
        for s in &m.submeshes {
            assert!(s.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
            assert!(s.texture.is_none_or(|k| scene.textures.contains_key(&k)));
        }
    }
    // terrain heights stay in the plausible range of the 8 bit map (0..51 m at scale 0.2)
    let max_y = scene.meshes.iter().flat_map(|m| m.vertices.iter()).map(|v| v.pos[1]).fold(f32::MIN, f32::max);
    assert!(max_y > 20.0 && max_y < 400.0, "max y {max_y}");
}

#[test]
fn dungeon_places_room_props() {
    let Some((store, dir)) = setup() else { return };
    let (scene, r) = load_playfield_report(&store, &dir, 127).unwrap();
    assert!(r.statels > 500 && r.failed_meshes == 0);
    assert!(!scene.instances.is_empty() && scene.spawn.is_some());
}
