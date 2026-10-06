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

    /// The zone itself and its neighbours: `CellSpaceBase_t::GenerateNeighborList(zone, out, 1)` (N3 `FUN_100220bb`, a
    /// virtual of the playfield's cell space that was not traced). [INFERENCE] the 8-neighbourhood for the zone grid, the
    /// door-connected rooms for dungeons.
    pub fn neighbours(&self, zone: usize) -> Vec<usize> {
        let n = self.attractors.len();
        let mut out = vec![zone];
        match self.zones_per_row {
            Some(w) if w > 0 => {
                let (row, col) = ((zone / w) as isize, (zone % w) as isize);
                for (dr, dc) in [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)] {
                    let (r, c) = (row + dr, col + dc);
                    if r >= 0 && c >= 0 && (c as usize) < w && (r as usize * w + c as usize) < n {
                        out.push(r as usize * w + c as usize);
                    }
                }
            }
            _ => out.extend(self.doors.get(zone).into_iter().flatten().map(|&z| z as usize).filter(|&z| z < n && z != zone)),
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
        CameraViews { attractors: vec![Vec::new(); n], zones_per_row, doors: vec![vec![1, 0xffff], vec![0, 2], vec![1]] }
    }

    #[test]
    fn grid_neighbours_stay_inside_the_map() {
        let v = views(Some(15), 225);
        assert_eq!(v.neighbours(0).len(), 4); // corner: itself + 3
        assert_eq!(v.neighbours(16).len(), 9);
        assert_eq!(v.neighbours(14).len(), 4);
        assert!(v.neighbours(224).iter().all(|&z| z < 225));
    }

    #[test]
    fn room_neighbours_follow_doors() {
        let v = views(None, 3);
        assert_eq!(v.neighbours(1), vec![1, 0, 2]);
        assert_eq!(v.neighbours(0), vec![0, 1]); // 0xffff = no room
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
