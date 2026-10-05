//! Camera position -> zone index (`n3Playfield_t::GetZoneInstance`, N3 @0x1000c97b), used to look up the audio district.
//!
//! Outdoor: out of range (`x < 0`, `z < 0`, `x > W*cell`, `z > H*cell`) is zone 0, else
//! `zone = (z / cell / zs) * (W / zs) + (x / cell / zs)` (floors, row-major = statel file order).
//! Dungeon: `PosToRoom` (@0x1000c8aa) takes the first room whose `n3Room_t::IsPosInside` (@0x10011664, ported in
//! [`room_contains`]) holds, the room index is the zone, none = zone 0. The client takes the candidates from a spatial
//! grid (`RoomGridLookup`), which only prunes: we test every room in list order.

use anyhow::{anyhow, Context, Result};
use ao_rdb::RecordStore;

use super::dungeon::{floor_min, parse_gnda, stretch, Gnda};
use super::record::Room;
use super::{ground, record, RECORD, TILEMAP};

pub struct ZoneLocator {
    kind: Kind,
}

enum Kind {
    /// tile size (m), zone size (tiles), map size (tiles)
    Grid { cell: f32, zs: usize, w: usize, h: usize },
    /// dungeon tilemap, rooms and each room's lowest floor (`CalculateRoomHeights`)
    Rooms { gnda: Box<Gnda>, rooms: Vec<(Room, f32)> },
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
            Kind::Rooms { .. } => Some(self.room_at(scene_pos).unwrap_or(0)),
        }
    }

    pub fn is_dungeon(&self) -> bool {
        matches!(self.kind, Kind::Rooms { .. })
    }

    /// Dungeon: first room whose `IsPosInside` holds (`None` when none does, where the client uses zone 0).
    pub fn room_at(&self, scene_pos: [f32; 3]) -> Option<usize> {
        let Kind::Rooms { gnda, rooms } = &self.kind else { return None };
        let p = [scene_pos[0], scene_pos[1], -scene_pos[2]];
        rooms.iter().position(|(r, min_floor)| room_contains(gnda, r, *min_floor, p))
    }
}

/// Rotation of the room frame by `rot` quarter turns about +Y (`FUN_1003afac` table): `x' = x cos + z sin`,
/// `z' = -x sin + z cos`.
fn rotate(v: [f32; 2], rot: u8) -> [f32; 2] {
    let (c, s) = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][(rot & 3) as usize];
    [v[0] * c + v[1] * s, -v[0] * s + v[1] * c]
}

/// `n3Room_t::IsPosInside` (N3 @0x10011664) for an AO world position: (1) x/z inside the room rectangle
/// (`GetRoomRect` @0x100101cb: `(x2-x1)*2 m` by `(z2-z1)*2 m`, swapped for odd `rot`, origin `pos - rotate(W'+1, H'+1)` plus
/// the rot dependent shifts, both ends inclusive; `W' = (((x2-x1)-1) & !1) + 1`), (2) the tile under the position
/// (`GetTilemapIndexFromPos` @0x10011077: inverse-rotated offset, 2 m cells) is inside the room's atlas window and is not
/// empty (`DCGA & 0x7f != 0`), (3) `y` lies in the tile's height window: from `floorY - 0.5` to
/// `v * (1 + 0.25 s) + floorY + 0.5` with `floorY = min(DHGA of the 4 corners) - room.min_floor + pos.y`, `v` the tile
/// template height (`HSTA`, indexed by the `DCGA` value) and `s = stretch(HCDA) * height_scale`.
pub(super) fn room_contains(g: &Gnda, room: &Room, min_floor: f32, p: [f32; 3]) -> bool {
    let [x1, z1, x2, z2] = room.rect.map(|v| v as i32);
    let (nx, nz) = (x2 - x1, z2 - z1);
    if nx <= 0 || nz <= 0 {
        return false;
    }
    let (wp, hp) = ((((nx - 1) & !1) + 1) as f32, (((nz - 1) & !1) + 1) as f32);
    let (sx, sz) = (nx as f32 * 2.0, nz as f32 * 2.0);
    let mut o = rotate([wp + 1.0, hp + 1.0], room.rot);
    match room.rot & 3 {
        1 => o[1] += sx,
        2 => {
            o[0] += sx;
            o[1] += sz;
        }
        3 => o[0] += sz,
        _ => {}
    }
    let (ex, ez) = if room.rot & 1 == 1 { (sz, sx) } else { (sx, sz) };
    let (x0, z0) = (room.pos[0] - o[0], room.pos[2] - o[1]);
    if !(p[0] >= x0 && p[0] <= x0 + ex && p[2] >= z0 && p[2] <= z0 + ez) {
        return false;
    }
    let d = rotate([p[0] - room.pos[0], p[2] - room.pos[2]], (4 - (room.rot & 3)) & 3);
    let (ux, uz) = (((d[0] + wp + 1.0) / 2.0).floor() as i32, ((d[1] + hp + 1.0) / 2.0).floor() as i32);
    if ux < 0 || uz < 0 || ux >= nx || uz >= nz {
        return false;
    }
    let (tx, tz) = (x1 + ux, z1 + uz);
    if tx as usize >= g.w || tz as usize >= g.h {
        return false;
    }
    let i = tz as usize * g.w + tx as usize;
    let t = (g.ty[i] & 0x7f) as usize;
    if t == 0 {
        return false;
    }
    let (fx, fz) = ((tx != 0) as usize, (tz != 0) as usize);
    let hs = g.height_scale;
    let dh = |x: usize, z: usize| g.floor[z * g.w + x] as f32 * hs;
    let (tx, tz) = (tx as usize, tz as usize);
    let m = dh(tx, tz).min(dh(tx - fx, tz)).min(dh(tx - fx, tz - fz)).min(dh(tx, tz - fz));
    let floor_y = m - min_floor + room.pos[1];
    let v = g.tile_height.get(t).copied().unwrap_or(0.0);
    let s = stretch(g.ceil[i]) as f32 * hs;
    p[1] > floor_y - 0.5 && p[1] <= v * (1.0 + 0.25 * s) + floor_y + 0.5
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
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        let gnda = parse_gnda(&d).with_context(|| format!("dungeon tilemap {}", rec.tilemap))?;
        let rooms = rec.rooms.iter().map(|r| (r.clone(), floor_min(&gnda, r).unwrap_or(65536.0))).collect();
        Kind::Rooms { gnda: Box::new(gnda), rooms }
    };
    Ok(ZoneLocator { kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outdoor_grid() {
        let g = ZoneLocator { kind: Kind::Grid { cell: 4.0, zs: 10, w: 150, h: 150 } };
        assert_eq!(g.zone_at([1.0, 0.0, -1.0]), Some(0));
        assert_eq!(g.zone_at([85.0, 0.0, -41.0]), Some(15 + 2));
        assert_eq!(g.zone_at([-50.0, 0.0, 5000.0]), Some(0)); // out of range -> 0
        assert_eq!(g.zone_at([599.0, 0.0, -599.0]), Some(224));
        assert_eq!(g.zone_at([601.0, 0.0, -10.0]), Some(0));
    }

    #[test]
    fn rotation_table() {
        assert_eq!(rotate([1.0, 0.0], 1), [0.0, -1.0]);
        assert_eq!(rotate([1.0, 0.0], 2), [-1.0, 0.0]);
        for r in 0..4 {
            let v = rotate(rotate([3.0, 5.0], r), (4 - r) & 3);
            assert_eq!(v, [3.0, 5.0]);
        }
    }
}
