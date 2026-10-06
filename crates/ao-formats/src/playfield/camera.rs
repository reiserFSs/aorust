//! Scripted camera views of a playfield: the per-zone / per-room `PointCameraAttractor_t` list (`n3Zone_t::
//! GetCameraAttractorList`, N3 @0x1001ab3a) and the zone neighbourhood the client searches (docs/zone/camera.md §6).

use anyhow::{anyhow, Result};
use ao_rdb::RecordStore;

use super::record::{self, CameraAttractor};
use super::{ground, RECORD, TILEMAP};

/// The attractors of one playfield and how its zones neighbour each other.
pub struct CameraViews {
    /// Per zone (outdoor) or room (dungeon): `n3Zone_t::GetCameraAttractorList`.
    pub attractors: Vec<Vec<CameraAttractor>>,
    /// Outdoor: zones per row (`W / zone size`); `None` for dungeons.
    zones_per_row: Option<usize>,
    /// Dungeon: per room the rooms behind its doors.
    doors: Vec<Vec<u16>>,
}

impl CameraViews {
    /// `zones_per_row`: `Some` for an outdoor zone grid, `None` for dungeons (`doors`: per room the rooms behind its doors).
    pub fn new(attractors: Vec<Vec<CameraAttractor>>, zones_per_row: Option<usize>, doors: Vec<Vec<u16>>) -> Self {
        Self { attractors, zones_per_row, doors }
    }

    pub fn is_empty(&self) -> bool {
        self.attractors.iter().all(Vec::is_empty)
    }

    /// `CellSpaceBase_t::GenerateNeighborList(zone, out, 1)` (Vehicle.dll @0x10002d9f, a thunk to vtable `+0x8c` of the playfield's
    /// cell space; at most 49 entries, `0xc4` bytes):
    /// * outdoor `GridSpace_t` (`MakeNeighborList` @0x10003f69): `zone` must be `< nx * ny`; the rectangle of zones within 1 of
    ///   `(col, row)`, clamped to the grid, row-major;
    /// * dungeon `RoomSpace_t::MakeNeighborList` @0x10007968 (radius ignored): for each entry `e` of the room's list (its
    ///   door-connected rooms plus itself, sorted: `n3Playfield_t::UpdateRoomSpace` @0x1000d9d8 + `RoomSpace_t::AddRoom`
    ///   @0x10007acf) `e` and then `e`'s own list, sorted and made unique. A room outside the table yields nothing.
    pub fn neighbours(&self, zone: usize) -> Vec<usize> {
        const CAP: usize = 49;
        let n = self.attractors.len();
        let mut out = Vec::new();
        match self.zones_per_row {
            Some(w) if w > 0 => {
                let rows = n / w;
                if zone < w * rows {
                    let (row, col) = (zone / w, zone % w);
                    for r in row.saturating_sub(1)..(row + 2).min(rows) {
                        for c in col.saturating_sub(1)..(col + 2).min(w) {
                            if out.len() < CAP {
                                out.push(r * w + c);
                            }
                        }
                    }
                }
            }
            _ => {
                // the room's own entry list: its valid door rooms and itself (`z < n`, not 0xffff, no duplicates)
                let list = |z: usize| -> Vec<usize> {
                    let mut l: Vec<usize> = self.doors.get(z).into_iter().flatten().map(|&d| d as usize).filter(|&d| d != 0xffff && d < n && d != z).collect();
                    l.push(z);
                    l.sort_unstable();
                    l.dedup();
                    l
                };
                if zone < n {
                    'outer: for e in list(zone) {
                        if out.len() >= CAP {
                            break;
                        }
                        out.push(e);
                        for e2 in list(e) {
                            if out.len() >= CAP {
                                break 'outer;
                            }
                            out.push(e2);
                        }
                    }
                    out.sort_unstable();
                    out.dedup();
                }
            }
        }
        out
    }
}

/// Loads the attractors of playfield `id`.
pub fn camera_views(store: &RecordStore, id: u32) -> Result<CameraViews> {
    let raw = store.get(RECORD, id)?.ok_or_else(|| anyhow!("no playfield {id}"))?;
    let rec = record::parse(&raw)?;
    let zones_per_row = if rec.is_outdoor() {
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        let tm = ground::parse(&d)?;
        Some((tm.cells_x / rec.zone_size.max(1) as usize).max(1))
    } else {
        None
    };
    let doors = rec.rooms.iter().map(|r| r.door_zones.clone()).collect();
    Ok(CameraViews { attractors: rec.attractors, zones_per_row, doors })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn views(zones_per_row: Option<usize>, n: usize) -> CameraViews {
        CameraViews { attractors: vec![Vec::new(); n], zones_per_row, doors: vec![vec![1, 0xffff], vec![0, 2], vec![1], vec![], vec![]] }
    }

    #[test]
    fn grid_neighbours_stay_inside_the_map() {
        let v = views(Some(15), 225);
        assert_eq!(v.neighbours(0), vec![0, 1, 15, 16]); // corner: row-major, clamped (`GridSpace_t::MakeNeighborList` @0x10003f69)
        assert_eq!(v.neighbours(16).len(), 9);
        assert_eq!(v.neighbours(16), vec![0, 1, 2, 15, 16, 17, 30, 31, 32]);
        assert_eq!(v.neighbours(14), vec![13, 14, 28, 29]);
        assert!(v.neighbours(224).iter().all(|&z| z < 225));
        assert!(v.neighbours(225).is_empty(), "a zone outside the grid has no neighbours");
    }

    #[test]
    fn room_neighbours_are_the_door_rooms_and_theirs() {
        // rooms 0-1-2 chained, 0xffff = no room, rooms 3 and 4 have no doors
        let v = views(None, 5);
        assert_eq!(v.neighbours(0), vec![0, 1, 2]); // 0 -> {0, 1}, 1 -> {0, 1, 2}: two hops
        assert_eq!(v.neighbours(1), vec![0, 1, 2]);
        assert_eq!(v.neighbours(3), vec![3]); // the room itself is in its own list
        assert!(v.neighbours(5).is_empty(), "a room outside the table yields nothing");
    }

    /// Real data: outdoors a zone has 4..=9 neighbours (itself included), in a dungeon every room sees itself and its door rooms.
    #[test]
    fn real_neighbour_lists_follow_the_cell_space() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let store = RecordStore::open(&dir).unwrap();
        let v = camera_views(&store, 4582).unwrap(); // Newbie Land
        let w = v.zones_per_row.unwrap();
        for z in 0..v.attractors.len() {
            let n = v.neighbours(z);
            assert!((4..=9).contains(&n.len()) && n.windows(2).all(|p| p[0] < p[1]) && n.contains(&z), "zone {z}: {n:?}");
            assert!(n.iter().all(|&m| (m / w).abs_diff(z / w) <= 1 && (m % w).abs_diff(z % w) <= 1), "zone {z}: {n:?}");
        }
        let d = camera_views(&store, 6131).unwrap(); // ICC Holodeck Alien Training: rooms with a door link
        assert!(d.zones_per_row.is_none());
        let mut linked = 0;
        for z in 0..d.attractors.len() {
            let n = d.neighbours(z);
            assert!(n.contains(&z) && n.windows(2).all(|p| p[0] < p[1]), "room {z}: {n:?}");
            for &door in d.doors[z].iter().filter(|&&r| r != 0xffff && (r as usize) < d.attractors.len()) {
                assert!(n.contains(&(door as usize)), "room {z} door {door}: {n:?}");
                linked += 1;
            }
        }
        assert!(linked > 0);
    }

    /// Real data: every playfield record parses and its attractors have finite positions.
    #[test]
    fn real_playfields_have_sane_attractors() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let store = RecordStore::open(&dir).unwrap();
        let (mut with, mut total, mut usable) = (0, 0, 0);
        for (id, _) in super::super::list_playfields(&store).unwrap() {
            let v = camera_views(&store, id).unwrap_or_else(|e| panic!("playfield {id}: {e:#}"));
            let n: usize = v.attractors.iter().map(Vec::len).sum();
            with += usize::from(n > 0);
            total += n;
            for a in v.attractors.iter().flatten() {
                assert!(a.pos.iter().chain(&a.target).chain(&a.rot).chain([&a.range]).all(|c| c.is_finite()), "playfield {id}: {a:?}");
                usable += usize::from(!a.disabled());
            }
        }
        // docs/zone/camera.md §6: the whole game has 208 attractors in 53 playfields, 6 of them (playfields 322 and 1892) usable
        assert_eq!((total, with, usable), (208, 53, 6));
    }
}
