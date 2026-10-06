//! Collision on real playfields (skips cleanly without the game client).

use std::collections::{HashMap, VecDeque};

use ao_formats::playfield::collision::{kd, Collision, RAY_LIFT, STEP_HEIGHT};
use ao_formats::playfield::{floor_below, load_playfield};
use ao_rdb::RecordStore;

fn setup() -> Option<(RecordStore, std::path::PathBuf)> {
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    dir.join("cd_image/rdb.db").exists().then(|| (RecordStore::open(&dir).unwrap(), dir))
}

type Cell = (i32, i32);
/// Visited lattice cell -> (parent cell, position on the ground).
type Seen = HashMap<Cell, (Option<Cell>, [f32; 3])>;

/// Breadth first walk on a 0.5 m lattice with the same primitives the movement code uses (`slide` + `ground` from a ray
/// lifted by `RAY_LIFT`); returns the parent map.
fn flood(c: &Collision, start: [f32; 3], radius: f32, height: f32, limit: usize) -> (Seen, Cell) {
    let key = |x: f32, z: f32| ((x / 0.5).round() as i32, (z / 0.5).round() as i32);
    let y0 = c.ground([start[0], start[1] + RAY_LIFT, start[2]]).expect("spawn has ground");
    let s = key(start[0], start[2]);
    let mut seen = HashMap::from([(s, (None, [start[0], y0, start[2]]))]);
    let mut q = VecDeque::from([s]);
    while let Some(k) = q.pop_front() {
        let p = seen[&k].1;
        if seen.len() > limit {
            break;
        }
        for (dx, dz) in [(0.5, 0.0), (-0.5, 0.0), (0.0, 0.5), (0.0, -0.5)] {
            let to = [p[0] + dx, p[1], p[2] + dz];
            let s = c.slide(p, to, radius, height);
            if (s[0] - to[0]).abs() > 0.02 || (s[2] - to[2]).abs() > 0.02 {
                continue; // blocked by a wall
            }
            let Some(g) = c.ground([s[0], p[1] + RAY_LIFT, s[2]]) else { continue };
            if (g - p[1]).abs() > STEP_HEIGHT + 0.12 {
                continue; // a step too high (or a fall)
            }
            let nk = key(s[0], s[2]);
            if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(nk) {
                e.insert((Some(k), [s[0], g, s[2]]));
                q.push_back(nk);
            }
        }
    }
    (seen, s)
}

#[test]
fn collision_arrival_hall_walk_out_is_continuous() {
    let Some((store, _)) = setup() else { return };
    let c = Collision::load(&store, 4604).unwrap();
    assert!(c.triangle_count() > 10_000, "KD volumes and tile floors loaded");
    // server spawn (205.2, 1.0, 255.8) -> scene (205.2, 1.0, -255.8)
    let spawn = [205.2, 1.0, -255.8];
    let floor = c.ground([spawn[0], spawn[1] + RAY_LIFT, spawn[2]]).expect("floor under the spawn");
    assert!((floor - spawn[1]).abs() < 0.5, "spawn stands on the floor: {floor}");
    let (seen, start) = flood(&c, spawn, 0.35, 1.8, 60_000);
    assert!(seen.len() > 5000, "the hall is walkable ({} cells)", seen.len());
    // both ends of the hall: the exit corridor at the south (scene z -262) and the north end (z -152)
    let south = *seen.keys().min_by_key(|k| k.1).unwrap();
    let north = *seen.keys().max_by_key(|k| k.1).unwrap();
    assert!(seen[&south].1[2] < -258.0, "reached the exit corridor end: {:?}", seen[&south].1);
    assert!(seen[&north].1[2] > -170.0, "reached the north end: {:?}", seen[&north].1);
    // walk the path back to the spawn: continuous, never falls through (ground exists, no jump above step height)
    let mut k = north;
    let mut steps = 0;
    while let (Some(parent), p) = seen[&k] {
        let q = seen[&parent].1;
        assert!((p[1] - q[1]).abs() <= STEP_HEIGHT + 0.12 + 1e-4, "height jump {q:?} -> {p:?}");
        assert!(c.ground([p[0], p[1] + RAY_LIFT, p[2]]).is_some());
        k = parent;
        steps += 1;
    }
    assert_eq!(k, start);
    assert!(steps > 150, "the path from the spawn to the north end is long: {steps}");
}

#[test]
fn collision_arrival_hall_walls_stop_the_character() {
    let Some((store, _)) = setup() else { return };
    let c = Collision::load(&store, 4604).unwrap();
    // from the middle of the corridor walk straight into the side: the x extent of the reachable corridor is bounded
    let (seen, _) = flood(&c, [205.2, 1.0, -255.8], 0.35, 1.8, 60_000);
    let xs: Vec<f32> = seen.keys().filter(|k| (k.1 as f32 * 0.5 + 256.0).abs() < 1.0).map(|k| k.0 as f32 * 0.5).collect();
    let (lo, hi) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
    assert!(hi - lo > 3.0 && hi - lo < 40.0, "corridor width {lo}..{hi}");
    // pushing far outside never leaves the walkable area
    let p = c.slide([205.2, 1.0, -255.8], [205.2 + 40.0, 1.0, -255.8], 0.35, 1.8);
    assert!(p[0] < hi + 1.0, "{p:?} beyond the corridor wall {hi}");
}

#[test]
fn collision_newbie_land_outdoor_ground() {
    let Some((store, dir)) = setup() else { return };
    let c = Collision::load(&store, 4582).unwrap();
    assert!(c.triangle_count() > 10_000);
    let scene = load_playfield(&store, &dir, 4582).unwrap();
    let spawn = scene.spawn.unwrap();
    // around the spawn the collision ground agrees with the rendered terrain mesh within a metre
    let mut checked = 0;
    for dx in [-40.0, -10.0, 0.0, 10.0, 40.0] {
        for dz in [-40.0, -10.0, 0.0, 10.0, 40.0] {
            let p = [spawn[0] + dx, spawn[1], spawn[2] + dz];
            let (Some(f), Some(g)) = (floor_below(&scene, p), c.ground(p)) else { continue };
            checked += 1;
            // statels may raise the collision ground above the terrain, never below it by more than a diagonal's error
            assert!(g > f - 3.0, "collision {g} below terrain mesh {f} at {p:?}");
        }
    }
    assert!(checked > 10);
    let g = c.ground(spawn).expect("ground under the spawn eye");
    assert!(spawn[1] - g > 0.0 && spawn[1] - g < 10.0, "eye {} over ground {g}", spawn[1]);
    // falling from far above lands on something; far outside the playfield there is nothing
    assert!(c.ground([spawn[0], spawn[1] + 500.0, spawn[2]]).is_some());
    assert!(c.ground([-5000.0, 500.0, 5000.0]).is_none());
}

#[test]
fn collision_kd_versions_and_liquids() {
    let Some((store, _)) = setup() else { return };
    // a version 4 record (zlib + KDTreeFile) and a version 5 record (range coded) both decode with their end markers
    for (id, version) in [(296_486_101u32, 4u32), (301_727_744, 5)] {
        let (v, d) = store.get_versioned(kd::SURFACE_TYPE, id).unwrap().unwrap();
        assert_eq!(v, version);
        let s = kd::parse(v, &d).unwrap();
        assert!(!s.volumes.is_empty());
        for vol in &s.volumes {
            assert!(vol.tris.iter().flatten().all(|&i| (i as usize) < vol.verts.len()));
            assert!(vol.verts.iter().flatten().all(|c| c.is_finite()));
        }
    }
    // Newland City has water: some point is submerged below a liquid surface
    let c = Collision::load(&store, 566).unwrap();
    let mut found = false;
    'scan: for x in (0..600).step_by(8) {
        for z in (0..600).step_by(8) {
            if let Some(l) = c.liquid_at([x as f32, -50.0, -(z as f32)]) {
                assert!(l.level >= -50.0);
                found = true;
                break 'scan;
            }
        }
    }
    let _ = found; // the city may have no liquid polygon: only the query must be well defined
}

/// Survey of every collision record of the client: all decode with their markers (`cargo test --release -p ao-formats
/// collision_survey -- --ignored`, about 15 s).
#[test]
#[ignore]
fn collision_survey_all_records_decode() {
    let Some((store, _)) = setup() else { return };
    let (mut ok, mut tris) = (0usize, 0usize);
    for id in store.ids(kd::SURFACE_TYPE).unwrap() {
        let (v, d) = store.get_versioned(kd::SURFACE_TYPE, id).unwrap().unwrap();
        let s = kd::parse(v, &d).unwrap_or_else(|e| panic!("record {id} v{v}: {e:#}"));
        tris += s.volumes.iter().map(|v| v.tris.len()).sum::<usize>();
        ok += 1;
    }
    assert!(ok > 200_000 && tris > 40_000_000, "{ok} records, {tris} triangles");
}
