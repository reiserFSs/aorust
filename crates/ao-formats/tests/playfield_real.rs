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

/// Statel attributes replace `SimpleMesh` textures (505: rocks `(0 -> 6311)`), fog volumes and sound emitters are exposed.
#[test]
fn statel_attributes_fog_and_sound_are_decoded() {
    let Some((store, dir)) = setup() else { return };
    let (scene, r) = load_playfield_report(&store, &dir, 505).unwrap();
    assert!(scene.textures.keys().any(|k| k.rdb_type == 1_010_004 && k.id == 6311), "override texture 6311 not loaded");
    assert_eq!(r.fogs.len(), 1);
    let (scene, r) = load_playfield_report(&store, &dir, 630).unwrap();
    let m = scene.fog_model.expect("fog model");
    assert_eq!(m.volumes.len(), r.fogs.len());
    let v = m.volumes[0];
    // inside a volume the fog is denser than the base fog, outside it is the base fog
    assert!(m.at(v.pos).1 < m.at([1e6, 0.0, 0.0]).1);
}

/// All 600 resources: 594 native-valid records and six native-rejected zero-count blobs.
#[test]
fn area_info_survey_preserves_native_acceptance() {
    use ao_formats::playfield::area::{self, PointData};
    let Some((store, _)) = setup() else { return };
    let ids = store.ids(area::RECORD).unwrap();
    assert_eq!(ids.len(), 600);
    let (mut count, mut valid, mut rejected, mut point_types) = (0, 0, Vec::new(), [0usize; 9]);
    for id in ids {
        let raw = store.get(area::RECORD, id).unwrap().unwrap();
        let areas = match area::parse(&raw) {
            Ok(areas) => areas,
            Err(error) => {
                assert!([1424, 1673, 2006, 2014, 2024, 4352].contains(&id), "{id}: {error:#}");
                assert_eq!(raw.len(), 19);
                assert_eq!(&raw[..7], &[6, 0, 0, 0, 0, 0, 0]);
                assert!(area::load(&store, id).is_err());
                rejected.push(id);
                continue;
            }
        };
        valid += 1;
        count += areas.areas.len();
        for zone in 0..areas.zone_to_area.len() {
            assert!(areas.area_for_zone(zone).is_some(), "{id}: zone {zone}");
        }
        for area in &areas.areas {
            for point in area.points.iter().flatten() {
                let kind = if point.flags >> 24 == 0 { point.flags & 3 } else { point.flags & 0x00ff_ffff };
                point_types[kind as usize] += 1;
                assert!(matches!((&point.data, kind),
                    (PointData::Base, 0 | 4..=6) | (PointData::Acg { .. }, 1..=3)
                    | (PointData::Fence { .. }, 7) | (PointData::Placement { .. }, 8)));
            }
        }
    }
    rejected.sort_unstable();
    assert_eq!(rejected, [1424, 1673, 2006, 2014, 2024, 4352]);
    assert_eq!(valid, 594);
    assert_eq!(count, 1998);
    assert_eq!(point_types, [4482, 2256, 2255, 2255, 237, 17, 6847, 179, 866]);
    assert!(area::load(&store, u32::MAX).unwrap().is_none());
}

#[test]
fn native_grid_names_and_nonidentity_zone_zero_mapping() {
    use ao_formats::playfield::area;
    let Some((store, _)) = setup() else { return };
    let mut grids = Vec::new();
    for id in store.ids(area::RECORD).unwrap() {
        if let Ok(Some(areas)) = area::load(&store, id) {
            if areas.is_grid() { grids.push(id); }
        } else {
            assert!([1424, 1673, 2006, 2014, 2024, 4352].contains(&id));
        }
    }
    grids.sort_unstable();
    assert_eq!(grids, [152, 4107]);
    let areas = area::load(&store, 560).unwrap().unwrap();
    assert_eq!(areas.zone_to_area[0], 3);
    assert_eq!(areas.areas[0].name, "Mort Crater");
    assert_eq!(areas.area_for_zone(0).unwrap().name, "Czeffyr Desert");
    assert!(!areas.is_grid());
}
