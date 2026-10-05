//! Camera position -> zone index (`n3Playfield_t::GetZoneInstance`, N3 @0x1000c97b), used to look up the audio district.
//!
//! Outdoor: out of range (`x < 0`, `z < 0`, `x > W*cell`, `z > H*cell`) is zone 0, else
//! `zone = (z / cell / zs) * (W / zs) + (x / cell / zs)` (floors, row-major = statel file order).
//! Dungeon: `PosToRoom` takes the first room whose `n3Room_t::IsPosInside` (@0x10011664) holds, the room index is the
//! zone, none = zone 0. Implemented part of `IsPosInside`: the x/z test against `GetRoomRect` (@0x100101cb): the
//! tile rectangle is `(x2-x1) * 2.0 m` by `(z2-z1) * 2.0 m` (`_DAT_1003c880`), width/depth swapped for odd `rot`,
//! centred on the room position. The template tile test and the height window (floor ±0.5 m, top of room formula only
//! partly decoded) are **not** applied [UNRESOLVED], so overlapping rooms at different heights resolve to the first
//! room of the list.

use anyhow::{anyhow, Context, Result};
use ao_rdb::RecordStore;

use super::{ground, record, RECORD, TILEMAP};

pub struct ZoneLocator {
    kind: Kind,
}

enum Kind {
    /// tile size (m), zone size (tiles), map size (tiles)
    Grid { cell: f32, zs: usize, w: usize, h: usize },
    /// room centre (AO world x, z) and half extents after rotation
    Rooms(Vec<([f32; 2], [f32; 2])>),
}

impl ZoneLocator {
    /// Zone containing the scene-space point (scene z is the negated AO world z).
    pub fn zone_at(&self, scene_pos: [f32; 3]) -> Option<usize> {
        let (x, z) = (scene_pos[0], -scene_pos[2]);
        match &self.kind {
            Kind::Grid { cell, zs, w, h } => {
                if x < 0.0 || z < 0.0 || x > *w as f32 * cell || z > *h as f32 * cell {
                    return Some(0);
                }
                let (col, row) = (((x / cell).floor() as usize) / zs, ((z / cell).floor() as usize) / zs);
                Some(row * (w / zs).max(1) + col)
            }
            Kind::Rooms(rooms) => Some(rooms.iter().position(|(c, h)| x >= c[0] - h[0] && x < c[0] + h[0] && z >= c[1] - h[1] && z < c[1] + h[1]).unwrap_or(0)),
        }
    }
}

/// Half extents of the room rectangle (2 m per dungeon tile, swapped for odd quarter turns).
fn room_half(rect: [u16; 4], rot: u8) -> [f32; 2] {
    let (w, d) = ((rect[2] - rect[0]) as f32, (rect[3] - rect[1]) as f32);
    if rot & 1 == 1 {
        [d, w]
    } else {
        [w, d]
    }
}

pub fn zone_locator(store: &RecordStore, id: u32) -> Result<ZoneLocator> {
    let raw = store.get(RECORD, id)?.ok_or_else(|| anyhow!("no playfield {id}"))?;
    let rec = record::parse(&raw)?;
    let kind = if rec.is_outdoor() {
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        let tm = ground::parse(&d).with_context(|| format!("tilemap {}", rec.tilemap))?;
        let zs = rec.zone_size.max(1) as usize;
        Kind::Grid { cell: tm.cell_size, zs, w: tm.cells_x, h: tm.cells_z }
    } else {
        Kind::Rooms(rec.rooms.iter().map(|r| ([r.pos[0], r.pos[2]], room_half(r.rect, r.rot))).collect())
    };
    Ok(ZoneLocator { kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_and_rooms() {
        let g = ZoneLocator { kind: Kind::Grid { cell: 4.0, zs: 10, w: 150, h: 150 } };
        assert_eq!(g.zone_at([1.0, 0.0, -1.0]), Some(0));
        assert_eq!(g.zone_at([85.0, 0.0, -41.0]), Some(15 + 2));
        assert_eq!(g.zone_at([-50.0, 0.0, 5000.0]), Some(0)); // out of range -> 0
        assert_eq!(g.zone_at([599.0, 0.0, -599.0]), Some(224));
        assert_eq!(g.zone_at([601.0, 0.0, -10.0]), Some(0));
        let r = ZoneLocator { kind: Kind::Rooms(vec![([0.0, 0.0], room_half([0, 0, 3, 3], 0)), ([100.0, 50.0], room_half([0, 0, 4, 2], 1))]) };
        assert_eq!(r.zone_at([1.0, 0.0, 0.0]), Some(0));
        assert_eq!(r.zone_at([98.0, 0.0, -46.0]), Some(1)); // rot 1: half extents 2 x 4
        assert_eq!(r.zone_at([90.0, 0.0, -50.0]), Some(0)); // outside every room -> 0
    }
}
