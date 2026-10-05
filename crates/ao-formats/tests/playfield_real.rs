//! Loads real playfields when the game client is installed; skips cleanly otherwise.

use ao_formats::playfield::{floor_below, list_playfields, load_playfield_report, scene_bounds};
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
    let sky: Vec<usize> = scene.sky.iter().map(|i| i.mesh).collect();
    let max_y = scene.meshes.iter().enumerate().filter(|(i, _)| !sky.contains(i)).flat_map(|(_, m)| m.vertices.iter()).map(|v| v.pos[1]).fold(f32::MIN, f32::max);
    assert!(max_y > 20.0 && max_y < 400.0, "max y {max_y}");
}

#[test]
fn dungeon_places_room_props() {
    let Some((store, dir)) = setup() else { return };
    let (scene, r) = load_playfield_report(&store, &dir, 127).unwrap();
    assert!(r.statels > 500 && r.failed_meshes == 0);
    assert!(!scene.instances.is_empty() && scene.spawn.is_some());
}

/// Default camera: inside the scene bounds, 0.5..30 m above terrain / room floor, looking somewhere else.
#[test]
fn spawn_points_are_inside_and_above_the_floor() {
    let Some((store, dir)) = setup() else { return };
    for id in [566, 705, 505, 127, 386, 152, 4327, 331] {
        let (scene, _) = load_playfield_report(&store, &dir, id).unwrap();
        let (s, at) = (scene.spawn.expect("spawn"), scene.spawn_look_at.expect("look-at"));
        let (lo, hi) = scene_bounds(&scene).unwrap();
        assert!((0..3).all(|i| s[i] >= lo[i] && s[i] <= hi[i] + if i == 1 { 50.0 } else { 0.0 }), "{id}: spawn {s:?} outside {lo:?}..{hi:?}");
        let floor = floor_below(&scene, s).unwrap_or_else(|| panic!("{id}: no floor below {s:?}"));
        assert!((0.5..30.0).contains(&(s[1] - floor)), "{id}: {} m above the floor", s[1] - floor);
        assert!((at[0] - s[0]).hypot(at[2] - s[2]) > 1.0, "{id}: look-at above/below spawn");
    }
}

/// Statel lights reach `Scene.lights`: valid, inside a plausible distance of the scene and present in dungeons and cities.
#[test]
fn statel_lights_are_valid() {
    let Some((store, dir)) = setup() else { return };
    for (id, min) in [(127, 100), (362, 100), (566, 1)] {
        let (scene, _) = load_playfield_report(&store, &dir, id).unwrap();
        assert!(scene.lights.len() >= min, "{id}: {} lights", scene.lights.len());
        let (lo, hi) = scene_bounds(&scene).unwrap();
        for l in &scene.lights {
            assert!(l.range > 0.0 && l.color.iter().all(|c| c.is_finite() && *c >= 0.0), "{id}: {l:?}");
            assert!((0..3).all(|i| l.pos[i] >= lo[i] - 60.0 && l.pos[i] <= hi[i] + 60.0), "{id}: light {l:?} outside {lo:?}..{hi:?}");
        }
    }
}
