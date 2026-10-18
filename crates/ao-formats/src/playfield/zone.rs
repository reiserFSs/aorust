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
        match &self.kind {
            Kind::Grid { cell, zs, w, h } => Some(grid_zone(*cell, *zs, *w, *h, scene_pos)),
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

    /// Native tile containment in scene space, with room IDs offset by one (zero means no room).
    pub fn room_volumes(&self) -> Vec<ao_scene::RoomVolume> {
        let Kind::Rooms { gnda, rooms } = &self.kind else { return vec![] };
        let mut volumes = Vec::new();
        for (index, (room, min_floor)) in rooms.iter().enumerate() {
            let Some(frame) = room_frame(room) else { continue };
            let [x1, z1, _, _] = room.rect.map(usize::from);
            let nx = (frame.nx as usize).min(gnda.w.saturating_sub(x1));
            let nz = (frame.nz as usize).min(gnda.h.saturating_sub(z1));
            for uz in 0..nz {
                for ux in 0..nx {
                    let (tx, tz) = (x1 + ux, z1 + uz);
                    let Some([bottom, top]) = tile_heights(gnda, room, *min_floor, tx, tz) else { continue };
                    // Inverse rotation is a signed permutation. Solve the actual f32 tile-index
                    // expression, not just its algebraic inverse: cancellation at a translated
                    // tile edge can move the inclusive endpoint by more than one ULP.
                    let mut min = [0.0, bottom.next_up(), 0.0];
                    let mut max = [0.0, top, 0.0];
                    for (local, tile) in [ux, uz].into_iter().enumerate() {
                        let basis = rotate(if local == 0 { [1.0, 0.0] } else { [0.0, 1.0] }, room.rot);
                        let axis = if basis[0] != 0.0 { 0 } else { 1 };
                        let sign = basis[axis];
                        let world = axis * 2;
                        let offset = frame.half[local];
                        let coord = |p: f32| (sign * (p - room.pos[world]) + offset) / 2.0;
                        let (lo, hi) = if sign > 0.0 {
                            (first_float(|p| coord(p) >= tile as f32), first_float(|p| coord(p) >= (tile + 1) as f32).next_down())
                        } else {
                            (first_float(|p| coord(p) < (tile + 1) as f32), first_float(|p| coord(p) < tile as f32).next_down())
                        };
                        min[world] = lo.max(frame.min[axis]);
                        max[world] = hi.min(frame.max[axis]);
                    }
                    if (0..3).all(|axis| min[axis] <= max[axis]) {
                        (min[2], max[2]) = (-max[2], -min[2]);
                        volumes.push(ao_scene::RoomVolume { min, max, room: index as u32 + 1 });
                    }
                }
            }
        }
        volumes
    }
}

/// Outdoor zone index of a scene-space point (`n3Playfield_t::GetZoneInstance`): the zone grid in statel file order, 0 outside the map.
pub(super) fn grid_zone(cell: f32, zs: usize, w: usize, h: usize, scene_pos: [f32; 3]) -> usize {
    let (x, z) = (scene_pos[0], -scene_pos[2]);
    if x < 0.0 || z < 0.0 || x > w as f32 * cell || z > h as f32 * cell {
        return 0;
    }
    let (col, row) = (((x / cell).floor() as usize) / zs, ((z / cell).floor() as usize) / zs);
    row * (w / zs).max(1) + col
}

/// Rotation of the room frame by `rot` quarter turns about +Y (`FUN_1003afac` table): `x' = x cos + z sin`,
/// `z' = -x sin + z cos`.
fn rotate(v: [f32; 2], rot: u8) -> [f32; 2] {
    let (c, s) = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][(rot & 3) as usize];
    [v[0] * c + v[1] * s, -v[0] * s + v[1] * c]
}

/// First finite f32 satisfying a monotone predicate, or +infinity when none does.
fn first_float(mut predicate: impl FnMut(f32) -> bool) -> f32 {
    // Float-flip keys order negative then positive floats, including both signed zeros.
    let decode = |key: u32| f32::from_bits(if key & 0x8000_0000 != 0 { key ^ 0x8000_0000 } else { !key });
    let (mut lo, mut hi) = (0x0080_0000u32, 0xff80_0000u32);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if predicate(decode(mid)) { hi = mid; } else { lo = mid + 1; }
    }
    decode(lo)
}

struct RoomFrame {
    nx: i32,
    nz: i32,
    half: [f32; 2],
    min: [f32; 2],
    max: [f32; 2],
}

fn room_frame(room: &Room) -> Option<RoomFrame> {
    let [x1, z1, x2, z2] = room.rect.map(i32::from);
    let (nx, nz) = (x2 - x1, z2 - z1);
    if nx <= 0 || nz <= 0 { return None; }
    let half = [(((nx - 1) & !1) + 2) as f32, (((nz - 1) & !1) + 2) as f32];
    let (sx, sz) = (nx as f32 * 2.0, nz as f32 * 2.0);
    let mut o = rotate(half, room.rot);
    match room.rot & 3 {
        1 => o[1] += sx,
        2 => { o[0] += sx; o[1] += sz; }
        3 => o[0] += sz,
        _ => {}
    }
    let (ex, ez) = if room.rot & 1 == 1 { (sz, sx) } else { (sx, sz) };
    let min = [room.pos[0] - o[0], room.pos[2] - o[1]];
    Some(RoomFrame { nx, nz, half, min, max: [min[0] + ex, min[1] + ez] })
}

/// `n3Room_t::IsPosInside` (N3 @0x10011664) for an AO world position: (1) x/z inside the room rectangle
/// (`GetRoomRect` @0x100101cb: `(x2-x1)*2 m` by `(z2-z1)*2 m`, swapped for odd `rot`, origin `pos - rotate(W'+1, H'+1)` plus
/// the rot dependent shifts, both ends inclusive; `W' = (((x2-x1)-1) & !1) + 1`), (2) the tile under the position
/// (`GetTilemapIndexFromPos` @0x10011077: inverse-rotated offset, 2 m cells) is inside the room's atlas window and is not
/// empty (`DCGA & 0x7f != 0`), (3) `y` lies in the tile's height window: from `floorY - 0.5` to
/// `v * (1 + 0.25 s) + floorY + 0.5` with `floorY = min(DHGA of the 4 corners) - room.min_floor + pos.y`, `v` the tile
/// template height (`HSTA`, indexed by the `DCGA` value) and `s = stretch(HCDA) * height_scale`.
pub(super) fn room_contains(g: &Gnda, room: &Room, min_floor: f32, p: [f32; 3]) -> bool {
    let Some(frame) = room_frame(room) else { return false };
    let [x1, z1, _, _] = room.rect.map(i32::from);
    let RoomFrame { nx, nz, half, min: [x0, z0], max: [xmax, zmax] } = frame;
    if !(p[0] >= x0 && p[0] <= xmax && p[2] >= z0 && p[2] <= zmax) {
        return false;
    }
    let d = rotate([p[0] - room.pos[0], p[2] - room.pos[2]], (4 - (room.rot & 3)) & 3);
    let (ux, uz) = (((d[0] + half[0]) / 2.0).floor() as i32, ((d[1] + half[1]) / 2.0).floor() as i32);
    if ux < 0 || uz < 0 || ux >= nx || uz >= nz {
        return false;
    }
    let Some([bottom, top]) = tile_heights(g, room, min_floor, (x1 + ux) as usize, (z1 + uz) as usize) else { return false };
    p[1] > bottom && p[1] <= top
}

fn tile_heights(g: &Gnda, room: &Room, min_floor: f32, tx: usize, tz: usize) -> Option<[f32; 2]> {
    if tx >= g.w || tz >= g.h { return None; }
    let i = tz * g.w + tx;
    let t = (g.ty[i] & 0x7f) as usize;
    if t == 0 { return None; }
    let (fx, fz) = ((tx != 0) as usize, (tz != 0) as usize);
    let hs = g.height_scale;
    let dh = |x: usize, z: usize| g.floor[z * g.w + x] as f32 * hs;
    let m = dh(tx, tz).min(dh(tx - fx, tz)).min(dh(tx - fx, tz - fz)).min(dh(tx, tz - fz));
    let floor_y = m - min_floor + room.pos[1];
    let v = g.tile_height.get(t).copied().unwrap_or(0.0);
    let s = stretch(g.ceil[i]) as f32 * hs;
    Some([floor_y - 0.5, v * (1.0 + 0.25 * s) + floor_y + 0.5])
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
        assert!(g.room_volumes().is_empty());
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

    #[test]
    fn room_volume_iteration_is_bounded_by_tilemap() {
        let gnda = parse_gnda(&super::super::dungeon::tests::fixture(&[])).unwrap();
        let room = Room { rot: 0, rect: [0, 0, u16::MAX, u16::MAX], pos: [0.0; 3], name: None,
            lightmap: None, door_zones: vec![], door_tiles: vec![], waters: vec![] };
        let mut outside = room.clone();
        outside.rect = [100, 100, u16::MAX, u16::MAX];
        let locator = ZoneLocator { kind: Kind::Rooms { gnda: Box::new(gnda), rooms: vec![(room, 0.0), (outside, 0.0)] } };
        let volumes = locator.room_volumes();
        assert_eq!(volumes.len(), 8);
        assert!(volumes.iter().all(|v| v.room == 1));
    }

    #[test]
    fn tile_volumes_match_native_containment() {
        let fixture = super::super::dungeon::tests::fixture(&[]);
        for rot in 0..4 {
            let mut gnda = parse_gnda(&fixture).unwrap();
            gnda.tile_height = vec![0.0, 3.0];
            for i in 0..gnda.floor.len() {
                gnda.floor[i] = (i % 5) as u8;
                gnda.ceil[i] = (i % 3) as u8;
            }
            gnda.ty[2 * gnda.w + 2] = 0; // interior hole must not become a room-wide box
            gnda.ty[gnda.w + 1] |= 0x80; // native special/door bit does not empty a tile
            // Fractional translation exercises cancellation at tile boundaries.
            let room = Room { rot, rect: [1, 1, 4, 3], pos: [10.1, 7.0, -20.3], name: None,
                lightmap: None, door_zones: vec![], door_tiles: vec![], waters: vec![] };
            let min_floor = floor_min(&gnda, &room).unwrap();
            let locator = ZoneLocator { kind: Kind::Rooms { gnda: Box::new(gnda), rooms: vec![(room.clone(), min_floor), (room.clone(), min_floor)] } };
            let volumes = locator.room_volumes();
            assert_eq!(volumes.len(), 10);
            assert!(volumes[..5].iter().all(|v| v.room == 1));
            assert!(volumes[5..].iter().all(|v| v.room == 2));
            let check = |p: [f32; 3]| {
                let found = volumes.iter().find(|v| (0..3).all(|a| p[a] >= v.min[a] && p[a] <= v.max[a])).map(|v| v.room);
                assert_eq!(found, locator.room_at(p).map(|r| r as u32 + 1), "rot={rot}, p={p:?}");
            };
            for v in &volumes[..5] {
                let center = std::array::from_fn(|a| (v.min[a] + v.max[a]) * 0.5);
                check(center);
                for a in 0..3 {
                    for edge in [v.min[a].next_down(), v.min[a], v.min[a].next_up(),
                        v.max[a].next_down(), v.max[a], v.max[a].next_up()] {
                        let mut p = center;
                        p[a] = edge;
                        check(p);
                    }
                }
            }
            // Tile centers include empty atlas cells outside the room and its interior hole.
            for uz in -1..5 {
                for ux in -1..5 {
                    let d = rotate([ux as f32 * 2.0 - 3.0, uz as f32 * 2.0 - 1.0], rot);
                    for y in [6.0, 7.0, 8.0, 10.0, 14.0] {
                        check([room.pos[0] + d[0], y, -(room.pos[2] + d[1])]);
                    }
                }
            }
        }
    }
}
