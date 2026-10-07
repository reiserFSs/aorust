//! Collision on real playfields (skips cleanly without the game client).

use std::collections::{HashMap, VecDeque};

use ao_formats::playfield::collision::{kd, Collision, STEP_HEIGHT};
use ao_formats::playfield::{floor_below, load_playfield};
use ao_rdb::RecordStore;

fn setup() -> Option<(RecordStore, std::path::PathBuf)> {
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    dir.join("cd_image/rdb.db").exists().then(|| (RecordStore::open(&dir).unwrap(), dir))
}

type Cell = (i32, i32);
/// Visited lattice cell -> (parent cell, position on the ground).
type Seen = HashMap<Cell, (Option<Cell>, [f32; 3])>;

/// Breadth first walk on a 0.5 m lattice with the movement code's primitive (`Collision::walk` = one
/// `EnsureSurfaceAlignment` step with a standing body); returns the parent map.
fn flood(c: &Collision, start: [f32; 3], limit: usize) -> (Seen, Cell) {
    let key = |x: f32, z: f32| ((x / 0.5).round() as i32, (z / 0.5).round() as i32);
    let y0 = c.ground(start).expect("spawn has ground") + 0.01;
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
            let w = c.walk(p, to);
            let s = w.pos;
            if (s[0] - to[0]).abs() > 0.02 || (s[2] - to[2]).abs() > 0.02 {
                continue; // blocked by a wall
            }
            if w.airborne || (s[1] - p[1]).abs() > STEP_HEIGHT + 0.12 {
                continue; // nothing carries the character, a step too high (or a fall)
            }
            let nk = key(s[0], s[2]);
            if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(nk) {
                e.insert((Some(k), s));
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
    let floor = c.ground(spawn).expect("floor under the spawn");
    assert!((floor - spawn[1]).abs() < 0.5, "spawn stands on the floor: {floor}");
    let (seen, start) = flood(&c, spawn, 60_000);
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
        assert!(c.ground(p).is_some());
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
    let (seen, _) = flood(&c, [205.2, 1.0, -255.8], 60_000);
    let xs: Vec<f32> = seen.keys().filter(|k| (k.1 as f32 * 0.5 + 256.0).abs() < 1.0).map(|k| k.0 as f32 * 0.5).collect();
    let (lo, hi) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
    assert!(hi - lo > 3.0 && hi - lo < 40.0, "corridor width {lo}..{hi}");
    // pushing far outside never leaves the walkable area
    let p = c.walk([205.2, 1.0, -255.8], [205.2 + 40.0, 1.0, -255.8]).pos;
    assert!(p[0] < hi + 1.0, "{p:?} beyond the corridor wall {hi}");
}

/// One frame steps (0.1 m) with the movement code's body state across lattice points of 4582: wherever the character is
/// carried the feet follow the ground (`ground + 0.01`), and on the open land around the spawn it is carried almost always.
#[test]
fn collision_vehicle_walks_the_newbie_plain() {
    use ao_formats::playfield::collision::{Body, SurfaceState};
    let Some((store, dir)) = setup() else { return };
    let c = Collision::load(&store, 4582).unwrap();
    let spawn = load_playfield(&store, &dir, 4582).unwrap().spawn.unwrap();
    let (mut steps, mut airborne, mut walks, mut far) = (0, 0, 0, 0);
    for gx in -6..=6 {
        for gz in -6..=6 {
            let at = [spawn[0] + gx as f32 * 12.0, spawn[1], spawn[2] + gz as f32 * 12.0];
            let Some(g) = c.ground(at) else { continue };
            let mut p = [at[0], g + 0.01, at[2]];
            let mut st = SurfaceState::default();
            walks += 1;
            for _ in 0..50 {
                let w = c.align(p, [p[0] + 0.1, p[1], p[2]], &Body::WALKING, &mut st);
                steps += 1;
                airborne += usize::from(w.airborne);
                if !w.airborne {
                    let ground = c.ground(w.pos).unwrap();
                    // deep water floats the feet 1 cm under the surface (liquid medium state machine), else they follow the ground
                    let want = if st.in_liquid { w.liquid - 0.01 } else { ground + 0.01 };
                    assert!((w.pos[1] - want).abs() < 0.2, "feet {:?} ground {ground} liquid {} swimming {}", w.pos, w.liquid, st.in_liquid);
                }
                far += usize::from((w.pos[0] - p[0] - 0.1).abs() < 1e-3);
                p = w.pos;
            }
        }
    }
    eprintln!("4582: {walks} walks, {steps} steps, {airborne} airborne, {far} unobstructed");
    assert!(walks > 100 && airborne * 5 < steps && far * 2 > steps, "{airborne}/{steps} airborne, {far} unobstructed");
}

/// The KD faces are one sided with the normal `(b - a) x (c - a)`; the data volumes are wound so that this normal is the outward one
/// (positive signed volume), which is what the client's walls (hit from outside) require.
#[test]
fn collision_kd_faces_point_outwards() {
    let Some((store, _)) = setup() else { return };
    let (v, d) = store.get_versioned(kd::SURFACE_TYPE, 301_727_744).unwrap().unwrap();
    let s = kd::parse(v, &d).unwrap();
    let (mut pos, mut neg) = (0, 0);
    for vol in &s.volumes {
        let mut volume = 0.0f64;
        for t in &vol.tris {
            let [a, b, c] = t.map(|i| vol.verts[i as usize].map(f64::from));
            volume += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]);
        }
        if volume > 0.0 { pos += 1 } else { neg += 1 }
    }
    eprintln!("KD volumes: {pos} wound outward, {neg} inward");
    assert!(pos > neg, "outward {pos} inward {neg}");
}

/// Door rules of the Arrival Hall: leaving "no room" is free, every link is crossable until a door blocks it.
#[test]
fn collision_arrival_hall_rooms_and_doors() {
    let Some((store, _)) = setup() else { return };
    let mut c = Collision::load(&store, 4604).unwrap();
    let links = c.room_links();
    eprintln!("4604 links {links:?}, spawn room {:?}", c.room_of([205.2, 1.0, -255.8]));
    assert!(c.room_transition_allowed(-1, 0) && !c.room_transition_allowed(0, -1));
    assert!(c.room_of([205.2, 1.0, -255.8]).is_some());
    if let Some(&(a, b)) = links.first() {
        assert!(c.room_transition_allowed(a as i32, b as i32) && c.room_transition_allowed(b as i32, a as i32));
        c.set_door_passable(a, b, false);
        assert!(!c.room_transition_allowed(a as i32, b as i32));
        c.set_door_passable(b, a, true);
        assert!(c.room_transition_allowed(a as i32, b as i32));
    }
}

/// The camera's `IsDoorOpenBetweenRooms` flag (`ChangeRoomStatus`): links start closed, only real links change, independent of `Door_t::CanPass`.
#[test]
fn collision_door_open_flag_of_a_real_link() {
    let Some((store, _)) = setup() else { return };
    let mut c = Collision::load(&store, 6131).unwrap(); // ICC Holodeck Alien Training: one link
    let (a, b) = c.room_links()[0];
    let (au, bu) = (a as usize, b as usize);
    assert!(!c.door_open_between(au, bu) && !c.door_open_between(bu, au));
    c.set_door_open(b, a, true);
    assert!(c.door_open_between(au, bu) && c.door_open_between(bu, au));
    assert!(c.room_transition_allowed(a as i32, b as i32), "the pass check is a different state");
    c.set_door_open(a, a, true);
    c.set_door_open(a, 0xffff, true);
    assert!(!c.door_open_between(au, au));
    c.set_door_open(a, b, false);
    assert!(!c.door_open_between(au, bu));
    // a pair without a link cannot be opened
    let free = (0..64u16).find(|&r| r != a && !c.room_links().contains(&(a.min(r), a.max(r)))).unwrap();
    c.set_door_open(a, free, true);
    assert!(!c.door_open_between(au, free as usize));
}

/// `GetDoorLinkFromPos`: scanning the plane around the spawn finds exactly the playfield's door links, each door once per room
/// entry, and nothing far from every door.
#[test]
fn collision_door_links_are_found_by_door_position() {
    let Some((store, dir)) = setup() else { return };
    let c = Collision::load(&store, 6131).unwrap(); // ICC Holodeck Alien Training
    let s = load_playfield(&store, &dir, 6131).unwrap().spawn.unwrap();
    let mut found = std::collections::BTreeSet::new();
    let mut hits = 0;
    for x in -300..=300 {
        for z in -300..=300 {
            if let Some((a, b)) = c.door_link_from_pos([s[0] + x as f32, s[1], s[2] + z as f32]) {
                hits += 1;
                if b != 0xffff {
                    found.insert((a.min(b), a.max(b))); // a door entry may lead nowhere (0xffff)
                }
            }
        }
    }
    eprintln!("6131 door links found {found:?} ({hits} lattice hits), links {:?}", c.room_links());
    let links: std::collections::BTreeSet<_> = c.room_links().into_iter().collect();
    assert!(found == links, "found {found:?} links {links:?}");
    assert!(hits < 600 * 600 / 50, "doors are small: {hits} of the lattice points match");
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

/// `Collision::in_teleportal` (`n3Zone_t::IsPosInTeleportal`) on real portals: the centroid of every teleportal polygon of an outdoor
/// playfield (4001) and a dungeon (4310) lies in it (at the polygon's own height), a point off the polygon's bounds does not, and the
/// plain spawn is no portal. The survey of every record: `cargo run --release -p ao-formats --example portal_survey`.
#[test]
fn collision_teleportal_of_real_polygons() {
    let Some((store, dir)) = setup() else { return };
    for pf in [4001u32, 4310] {
        let c = Collision::load(&store, pf).unwrap();
        let (mut seen, mut hit) = (0, 0);
        for zone in 0..4096u32 {
            let Some((v, d)) = store.get_versioned(kd::SURFACE_TYPE, pf << 16 | zone).unwrap() else { continue };
            let s = kd::parse(v, &d).unwrap();
            if s.portal.len() < 3 {
                continue;
            }
            seen += 1;
            let n = s.portal.len() as f32;
            let mid = s.portal.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
            // scene space mirrors z; a zone whose polygon is another zone's replica answers through its own zone
            hit += usize::from(c.in_teleportal([mid[0], mid[1], -mid[2]]));
            let far = [s.portal.iter().map(|p| p[0]).fold(f32::MIN, f32::max) + 50.0, mid[1], -mid[2]];
            assert!(!c.in_teleportal(far), "pf {pf} zone {zone}: a point right of the polygon is no portal");
        }
        assert!(seen > 0 && hit * 2 >= seen, "pf {pf}: {hit} of {seen} portal centroids are inside");
    }
    let spawn = load_playfield(&store, &dir, 4582).unwrap().spawn.unwrap();
    assert!(!Collision::load(&store, 4582).unwrap().in_teleportal(spawn), "Newland City has no teleportal polygon");
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

/// Dungeon liquids of the room records (`n3Room_t` reader N3 @0x10012803): playfield 120 has a room liquid of kind 2 at local
/// y 5.0 (room pos y 0.395); a character standing on the floor in it is wading, the closest point never lies deeper than 1.2 m
/// below the surface, and a point outside every room has no liquid.
#[test]
fn collision_room_liquids_are_found_in_dungeons() {
    let Some((store, _)) = setup() else { return };
    let c = Collision::load(&store, 120).unwrap();
    let mut found = None;
    'scan: for x in (0..400).step_by(2) {
        for z in (0..500).step_by(2) {
            for y in [1.0f32, 6.0, 10.0, 20.0] {
                let p = [x as f32, y, -(z as f32)];
                if c.room_of(p).is_none() {
                    continue;
                }
                let cp = c.closest(p, -1).unwrap();
                if cp.liquid > -9000.0 {
                    found = Some((p, cp));
                    break 'scan;
                }
            }
        }
    }
    let (p, cp) = found.expect("a room liquid of playfield 120 is reachable");
    assert!(cp.liquid > cp.pos[1] - 40.0 && cp.pos[1] >= cp.liquid - 1.2 - 1e-3, "{p:?} {cp:?}");
    assert_eq!(c.liquid_at(cp.pos).map(|l| l.level), Some(cp.liquid));
    let liquid = cp.liquid_info.expect("closest query retains LiquidMediumData flags and direction");
    assert_eq!(liquid, c.liquid_at(cp.pos).unwrap());
    assert_eq!(liquid.kind & 0x1e, 2);
    assert!((liquid.normal.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
    assert!(c.liquid_at([p[0], p[1] + 5000.0, p[2]]).is_none(), "outside every room");
}
