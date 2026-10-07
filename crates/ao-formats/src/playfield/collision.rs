//! Ground following and collision data of a playfield, from the data the original client walks on.
//!
//! The client's world surface is a `Surface_i` (N3.dll): `n3TilemapSurface_t` (outdoor, vtable @0x1004...,
//! `CalculateClosestPoint` @0x10018f0c) or `n3RoomSurface_t` (dungeon, `CalculateClosestPoint` @0x10013ee6), plus one
//! KD-tree surface (`n3SurfaceResource_t`, rdb 1000013) per statel zone / room that holds the collision volumes of the
//! placed objects. See `docs/zone/collision.md` for addresses, constants and the deviations of this port.
//!
//! * outdoor ground = the heightfield, every cell split by the parity rule `(~z ^ x) & 1` (`FUN_1001769d` @0x1001769d /
//!   `FUN_10017800` @0x10017800), raised by the KD volumes under the point;
//! * dungeon ground = the room's tile triangles (`CalculateClosestPoint` @0x10013ee6, atlas tile `(tx, tz)` with corner
//!   heights of the cells `(tx-1..tx, tz-1..tz)`, diagonal `(x, z)-(x+1, z+1)`), raised by the KD volumes of the room;
//! * walls / props = the triangles of the KD volumes ([`kd`]).
//!
//! The API is in **scene coordinates** (the AO world mirrored in z, `playfield` module docs); queries run on a uniform grid.

use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use ao_rdb::RecordStore;
use ao_scene::{Scene, IDENTITY};

use super::dungeon::{floor_min, parse_gnda, Gnda};
use super::record::{self, Rd, Room};
use super::zone::{self, room_contains};
use super::{ground, water, RECORD, TILEMAP};

pub mod kd;
mod portal;
mod vehicle;
pub use vehicle::{Aligned, Body, Closest, Hit, DEFAULT_BODY_RADIUS, LiquidEvent, SurfaceState, FOOT_CLEARANCE, RADIUS, SWIM_DEPTH};

/// A surface triangle is walkable ground when its normal's y is at least this (Vehicle.dll `EnsureSurfaceAlignment`
/// @0x1000d1aa, f32 @0x10012134, found by the `Avatar.Movement` RE); steeper faces are walls.
pub const MIN_FLOOR_NY: f32 = 0.5;
/// Obstacle height a walking character steps over (Vehicle.dll f32 @0x100127e0, 0.48; the full tolerance also adds twice
/// the step length, applied by the caller).
pub const STEP_HEIGHT: f32 = 0.48;
/// The client starts its ground rays 0.4 m above the old position (Vehicle.dll f64 @0x100127f8).
pub const RAY_LIFT: f32 = 0.4;
/// The solid floor is never below `liquid surface - WADE_DEPTH` (`n3TilemapSurface_t::CalculateClosestPoint`: `y = level - _DAT_1003d370`, 1.2).
pub const WADE_DEPTH: f32 = 1.2;

const CELL: f32 = 4.0;
/// A triangle covering more grid cells than this is kept in the always-tested list.
const BIG_CELLS: usize = 256;

/// A liquid volume at a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Liquid {
    /// Surface height (scene y).
    pub level: f32,
    /// `n3WaterData_t` kind (`kind >> 1` selects water / lava / slime / acid / mud, see `water.rs`).
    pub kind: u32,
    /// Liquid collision plane normal, in scene space (`LiquidMediumData_t +8`).
    pub normal: [f32; 3],
}

/// Ground found under a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Support {
    pub y: f32,
    /// Unit surface normal (scene space, y up).
    pub normal: [f32; 3],
}

#[derive(Clone, Copy)]
struct Tri {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    n: [f32; 3],
    /// Dungeon tile floor of room `floor - 1` (0: a KD volume triangle).
    floor: u16,
    /// KD volume triangle of zone / room `zone - 1` (`CellSurface_t::SetSurfaceForCell`, rdb 1000013 `playfield << 16 | index`);
    /// 0 = not part of a zone surface (terrain cells, scene fallback geometry, synthetic test geometry: always considered).
    zone: u32,
}

/// Heightfield of an outdoor playfield (`n3TilemapSurface_t`).
struct Terrain {
    tm: ground::Tilemap,
}

pub struct Collision {
    tris: Vec<Tri>,
    /// CSR grid over xz: `start[cell]..start[cell + 1]` indexes `items`.
    origin: [f32; 2],
    dims: [usize; 2],
    cell: f32,
    start: Vec<u32>,
    items: Vec<u32>,
    big: Vec<u32>,
    terrain: Option<Terrain>,
    rooms: Option<Rooms>,
    liquids: Vec<(Tri, u32)>,
    /// Teleportal polygons by zone and the zone grid size (`PlayfieldRecord::zone_size`), for [`Collision::in_teleportal`].
    portals: portal::Portals,
    zone_size: usize,
    /// Zones / rooms that own a KD surface record (`tri.zone` tags), also those without triangles.
    kd_zones: HashSet<u32>,
    resource: Option<(u32, u32)>,
}

/// Dungeon rooms with the tilemap: a position is only valid inside one (`n3RoomSurface_t::VetoPosition` @0x10015587).
struct Rooms {
    gnda: Box<Gnda>,
    /// Room and its lowest floor (`CalculateRoomHeights`).
    rooms: Vec<(Room, f32)>,
    /// Room pairs `(min, max)` joined by a door (`n3Playfield_t` room link map, built in `n3Playfield_t::InitializeSpace`
    /// from `n3Room_t::GetDoorConnectZone` @0x1000dd29..).
    links: HashSet<(u16, u16)>,
    /// Links whose `Door_t` does not let the character pass (`FUN_1007f74d` false); empty = every door passable.
    blocked: HashSet<(u16, u16)>,
    /// Links whose door is open: the flag byte of the room link map entry (`n3Playfield_t::ChangeRoomStatus` @0x1000d17e, set by
    /// `DoorOpened` / `DoorClosed`; entries start closed). Separate from `blocked`: this is what `IsDoorOpenBetweenRooms` reads.
    open: HashSet<(u16, u16)>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add_v(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale_v(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(v, v).sqrt();
    (l > 1e-12).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

/// DS `FUN_1003a3b4`: split cross-product magnitude > 10000 into four midpoint triangles,
/// depth-first in corner A/B/C/centre order before appending collision infos.
fn add_outdoor_liquid(p: [[f32; 3]; 3], kind: u32, out: &mut Vec<(Tri, u32)>) -> Result<()> {
    let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
    let area = dot(n, n).sqrt();
    anyhow::ensure!(area.is_finite(), "non-finite liquid triangle");
    anyhow::ensure!(out.len() < 1_000_000, "implausible liquid collision triangle count");
    if area > 10_000.0 {
        let [a, b, c] = p;
        let midpoint = |a, b| scale_v(add_v(a, b), 0.5);
        let (ab, bc, ca) = (midpoint(a, b), midpoint(b, c), midpoint(c, a));
        for child in [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]] {
            add_outdoor_liquid(child, kind, out)?;
        }
    } else if let Some(mut tri) = Tri::from_world(p[0], p[1], p[2]) {
        // DS 0x1003a0d6 rejects centroids outside its 60×60, 100 m bucket index.
        let x = ((p[1][0] + p[0][0] + p[2][0]) / 3.0) as f64 / 100.0;
        let z = ((p[1][2] + p[0][2] + p[2][2]) / 3.0) as f64 / -100.0;
        let bucket = (x as i64).wrapping_sub((z as i64).wrapping_mul(60)) as u64;
        if bucket >= 3600 { return Ok(()); }
        if tri.n[1] < 0.0 { tri.n = tri.n.map(|v| -v); }
        out.push((tri, kind));
    }
    Ok(())
}

impl Tri {
    /// Scene-space triangle with an explicit unit normal.
    fn with_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3], n: [f32; 3]) -> Option<Tri> {
        Some(Tri { a, b, c, n: unit(n)?, floor: 0, zone: 0 })
    }
    /// AO world triangle (left handed, outward normal `(b-a) x (c-a)`, see `kd`) mirrored into scene space.
    fn from_world(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<Tri> {
        let n = cross(sub(b, a), sub(c, a));
        let m = |p: [f32; 3]| [p[0], p[1], -p[2]];
        Tri::with_normal(m(a), m(b), m(c), [n[0], n[1], -n[2]])
    }
    /// Plane height at `(x, z)` for a non-vertical triangle.
    fn y_at(&self, x: f32, z: f32) -> f32 {
        self.a[1] - (self.n[0] * (x - self.a[0]) + self.n[2] * (z - self.a[2])) / self.n[1]
    }
    /// Barycentric xz containment (with a small tolerance so that shared edges do not leak).
    fn contains_xz(&self, x: f32, z: f32) -> bool {
        let (a, b, c) = (self.a, self.b, self.c);
        let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if d.abs() < 1e-12 {
            return false;
        }
        let l1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
        let l2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
        let e = -1e-4;
        l1 >= e && l2 >= e && 1.0 - l1 - l2 >= e
    }
    /// Native liquid tests exclude every edge (DS 0x10039d3c / N3 0x1000b498).
    fn liquid_contains_xz(&self, x: f32, z: f32) -> bool {
        let sides = [(self.a, self.b), (self.b, self.c), (self.c, self.a)]
            .map(|(a, b)| (z - a[2]) * (b[0] - a[0]) - (x - a[0]) * (b[2] - a[2]));
        sides.iter().all(|v| *v > 0.0) || sides.iter().all(|v| *v < 0.0)
    }
    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = self.a;
        let mut hi = self.a;
        for p in [self.b, self.c] {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }
    /// Closest point of the triangle to `p` (Ericson, *Real-Time Collision Detection* 5.1.5).
    fn closest(&self, p: [f32; 3]) -> [f32; 3] {
        let (a, b, c) = (self.a, self.b, self.c);
        let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
        let (d1, d2) = (dot(ab, ap), dot(ac, ap));
        if d1 <= 0.0 && d2 <= 0.0 {
            return a;
        }
        let bp = sub(p, b);
        let (d3, d4) = (dot(ab, bp), dot(ac, bp));
        if d3 >= 0.0 && d4 <= d3 {
            return b;
        }
        let vc = d1 * d4 - d3 * d2;
        let at = |t: f32, u: [f32; 3], o: [f32; 3]| [o[0] + t * u[0], o[1] + t * u[1], o[2] + t * u[2]];
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            return at(d1 / (d1 - d3), ab, a);
        }
        let cp = sub(p, c);
        let (d5, d6) = (dot(ab, cp), dot(ac, cp));
        if d6 >= 0.0 && d5 <= d6 {
            return c;
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            return at(d2 / (d2 - d6), ac, a);
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
            return at((d4 - d3) / ((d4 - d3) + (d5 - d6)), sub(c, b), b);
        }
        let s = 1.0 / (va + vb + vc);
        let (v, w) = (vb * s, vc * s);
        [a[0] + ab[0] * v + ac[0] * w, a[1] + ab[1] * v + ac[1] * w, a[2] + ab[2] * v + ac[2] * w]
    }
}

impl Terrain {
    /// Height and unit normal at AO world `(x, z)`: the cell is split along `(x,z)-(x+1,z+1)` when `(~z ^ x) & 1` is 1,
    /// else along `(x+1,z)-(x,z+1)`; the point lies in one of the two triangles (`FUN_10017800` @0x10017800).
    fn at(&self, x: f32, z: f32) -> Option<(f32, [f32; 3])> {
        let tm = &self.tm;
        if x < 0.0 || z < 0.0 || x >= tm.cells_x as f32 * tm.cell_size || z >= tm.cells_z as f32 * tm.cell_size {
            return None;
        }
        let (fx, fz) = (x / tm.cell_size, z / tm.cell_size);
        let (ix, iz) = (fx as usize, fz as usize);
        let c = tm.cell_size;
        let (x0, z0) = (ix as f32 * c, iz as f32 * c);
        let h = [tm.height(ix, iz), tm.height(ix + 1, iz), tm.height(ix + 1, iz + 1), tm.height(ix, iz + 1)];
        let p = [[x0, h[0], z0], [x0 + c, h[1], z0], [x0 + c, h[2], z0 + c], [x0, h[3], z0 + c]];
        let (dx, dz) = (x - x0, z - z0);
        let t = if ((!iz) ^ ix) & 1 == 0 {
            if dx + dz > c { [p[1], p[2], p[3]] } else { [p[0], p[1], p[3]] }
        } else if dx <= dz {
            [p[0], p[2], p[3]]
        } else {
            [p[0], p[1], p[2]]
        };
        let mut n = unit(cross(sub(t[2], t[0]), sub(t[1], t[0])))?;
        if n[1] < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        let y = t[0][1] - (n[0] * (x - t[0][0]) + n[2] * (z - t[0][2])) / n[1];
        Some((y, n))
    }
}

impl Collision {
    /// Native playfield resource tilemap id (`+0x1c`) and flags (`+0x50`).
    pub fn effect_resource(&self) -> Option<(u32, u32)> { self.resource }

    fn build(tris: Vec<Tri>, terrain: Option<Terrain>, rooms: Option<Rooms>, liquids: Vec<(Tri, u32)>) -> Collision {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for t in &tris {
            let (a, b) = t.bounds();
            lo = [lo[0].min(a[0]), lo[1].min(a[2])];
            hi = [hi[0].max(b[0]), hi[1].max(b[2])];
        }
        if tris.is_empty() {
            (lo, hi) = ([0.0; 2], [0.0; 2]);
        }
        let mut cell = CELL;
        // keep the grid below ~4M cells even for huge hulls
        while ((hi[0] - lo[0]) / cell + 1.0) * ((hi[1] - lo[1]) / cell + 1.0) > 4.0e6 {
            cell *= 2.0;
        }
        let dims = [((hi[0] - lo[0]) / cell) as usize + 1, ((hi[1] - lo[1]) / cell) as usize + 1];
        let span = |t: &Tri| {
            let (a, b) = t.bounds();
            let c = |v: f32, o: f32, n: usize| (((v - o) / cell).floor().max(0.0) as usize).min(n - 1);
            ([c(a[0], lo[0], dims[0]), c(a[2], lo[1], dims[1])], [c(b[0], lo[0], dims[0]), c(b[2], lo[1], dims[1])])
        };
        let mut count = vec![0u32; dims[0] * dims[1] + 1];
        let mut big = Vec::new();
        let mut small = Vec::with_capacity(tris.len());
        for (i, t) in tris.iter().enumerate() {
            let (a, b) = span(t);
            if (b[0] - a[0] + 1) * (b[1] - a[1] + 1) > BIG_CELLS {
                big.push(i as u32);
                continue;
            }
            small.push((i as u32, a, b));
            for z in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    count[z * dims[0] + x] += 1;
                }
            }
        }
        let mut start = vec![0u32; count.len()];
        for i in 0..count.len() - 1 {
            start[i + 1] = start[i] + count[i];
        }
        let mut fill = start.clone();
        let mut items = vec![0u32; start[start.len() - 1] as usize];
        for (i, a, b) in small {
            for z in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    let c = z * dims[0] + x;
                    items[fill[c] as usize] = i;
                    fill[c] += 1;
                }
            }
        }
        Collision { tris, origin: lo, dims, cell, start, items, big, terrain, rooms, liquids, portals: portal::Portals::default(), zone_size: 1, kd_zones: HashSet::new(), resource: None }
    }

    /// Collision of the loaded scene's identity-placed meshes (terrain, room shells): the documented fallback when the
    /// client's collision records are unavailable. Placed statels are not included.
    pub fn from_scene(scene: &Scene) -> Collision {
        let mut tris = Vec::new();
        for inst in scene.instances.iter().filter(|i| i.transform == IDENTITY) {
            let m = &scene.meshes[inst.mesh];
            for s in &m.submeshes {
                for t in s.indices.as_chunks::<3>().0 {
                    let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].pos);
                    if let Some(tri) = Tri::with_normal(a, b, c, cross(sub(b, a), sub(c, a))) {
                        tris.push(tri);
                    }
                }
            }
        }
        Collision::build(tris, None, None, Vec::new())
    }

    /// Loads the collision of playfield `id`: heightfield or dungeon tile floors, the KD surfaces of every zone / room and
    /// the liquid polygons.
    pub fn load(store: &RecordStore, id: u32) -> Result<Collision> {
        let raw = store.get(RECORD, id)?.ok_or_else(|| anyhow!("no playfield {id}"))?;
        let rec = record::parse(&raw)?;
        let mut tris = Vec::new();
        let mut terrain = None;
        let mut rooms = None;
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        if rec.is_outdoor() {
            terrain = Some(Terrain { tm: ground::parse(&d).with_context(|| format!("tilemap {}", rec.tilemap))? });
        } else {
            let g = parse_gnda(&d).with_context(|| format!("dungeon tilemap {}", rec.tilemap))?;
            for (i, room) in rec.rooms.iter().enumerate() {
                room_floor(&g, room, i as u16 + 1, &mut tris);
            }
            let list: Vec<(Room, f32)> = rec.rooms.iter().map(|r| (r.clone(), floor_min(&g, r).unwrap_or(65536.0))).collect();
            let mut links = HashSet::new();
            for (i, room) in rec.rooms.iter().enumerate() {
                for &z in room.door_zones.iter().filter(|&&z| z != 0xffff && z as usize != i) {
                    links.insert(((i as u16).min(z), (i as u16).max(z)));
                }
            }
            rooms = Some(Rooms { gnda: Box::new(g), rooms: list, links, blocked: HashSet::new(), open: HashSet::new() });
        }
        let mut portals = portal::Portals::default();
        let mut kd_zones = HashSet::new();
        for zone in 0..rec.count {
            let Some((version, data)) = store.get_versioned(kd::SURFACE_TYPE, id << 16 | zone)? else { continue };
            let mut s = kd::parse(version, &data).with_context(|| format!("collision surface {id}:{zone}"))?;
            if !s.portal.is_empty() {
                portals.zones.insert(zone, std::mem::take(&mut s.portal));
            }
            kd_zones.insert(zone + 1);
            for v in &s.volumes {
                for t in &v.tris {
                    if let Some(mut tri) = Tri::from_world(v.verts[t[0] as usize], v.verts[t[1] as usize], v.verts[t[2] as usize]) {
                        tri.zone = zone + 1;
                        tris.push(tri);
                    }
                }
            }
        }
        let mut tail = Rd::new(&raw, rec.tail);
        let mut liquids = Vec::new();
        for w in water::parse(&mut tail).with_context(|| format!("liquids of playfield {id}"))?.into_iter().filter(|w| w.kind & 1 == 0) {
            for t in &w.tris {
                let p = t.map(|i| w.verts[i as usize]);
                add_outdoor_liquid(p, w.kind, &mut liquids)?;
            }
        }
        // room liquids (`n3Room_t` reader N3 @0x10012803 -> `n3Zone_t::AddLiquidCollisionData` @0x1001a9c5): room-local
        // vertices, never rotated; the collision data skips sloped (odd) kinds (`FUN_1000b0d1`: `if (kind & 1) return`).
        for (i, room) in rec.rooms.iter().enumerate() {
            for w in room.waters.iter().filter(|w| w.kind & 1 == 0) {
                for t in &w.tris {
                    let p = t.map(|i| add_v(w.verts[i as usize], room.pos));
                    if let Some(mut tri) = Tri::from_world(p[0], p[1], p[2]) {
                        tri.floor = i as u16 + 1;
                        liquids.push((tri, w.kind));
                    }
                }
            }
        }
        let mut c = Collision::build(tris, terrain, rooms, liquids);
        c.portals = portals;
        c.kd_zones = kd_zones;
        c.resource = Some((rec.tilemap, rec.flags));
        c.zone_size = rec.zone_size.max(1) as usize;
        Ok(c)
    }

    /// Number of grid triangles (KD volumes, dungeon floors); the heightfield is analytic.
    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    fn cell_of(&self, x: f32, z: f32) -> Option<usize> {
        let (cx, cz) = ((x - self.origin[0]) / self.cell, (z - self.origin[1]) / self.cell);
        (cx >= 0.0 && cz >= 0.0 && (cx as usize) < self.dims[0] && (cz as usize) < self.dims[1]).then(|| cz as usize * self.dims[0] + cx as usize)
    }

    /// Triangles that may touch the grid cell holding `(x, z)`.
    fn near(&self, x: f32, z: f32) -> impl Iterator<Item = &Tri> {
        let cell = self.cell_of(x, z);
        let a = cell.map_or(0, |c| self.start[c] as usize);
        let b = cell.map_or(0, |c| self.start[c + 1] as usize);
        self.items[a..b].iter().chain(&self.big).map(|&i| &self.tris[i as usize])
    }

    /// Triangles that may touch the xz square `[x - r, x + r] x [z - r, z + r]`.
    fn near_box(&self, x: f32, z: f32, r: f32) -> Vec<&Tri> {
        self.near_aabb(x - r, z - r, x + r, z + r)
    }

    /// Triangles that may touch the xz box `[x0, x1] x [z0, z1]`.
    fn near_aabb(&self, x0: f32, z0: f32, x1: f32, z1: f32) -> Vec<&Tri> {
        let c = |v: f32, o: f32, n: usize| (((v - o) / self.cell).floor().max(0.0) as usize).min(n - 1);
        let (x0, x1) = (c(x0, self.origin[0], self.dims[0]), c(x1, self.origin[0], self.dims[0]));
        let (z0, z1) = (c(z0, self.origin[1], self.dims[1]), c(z1, self.origin[1], self.dims[1]));
        let mut seen: Vec<u32> = Vec::new();
        if !self.items.is_empty() {
            for cz in z0..=z1 {
                for cx in x0..=x1 {
                    let i = cz * self.dims[0] + cx;
                    seen.extend_from_slice(&self.items[self.start[i] as usize..self.start[i + 1] as usize]);
                }
            }
        }
        seen.sort_unstable();
        seen.dedup();
        seen.iter().chain(&self.big).map(|&i| &self.tris[i as usize]).collect()
    }

    /// Ground height under the FEET position `p` (`Surface_i::CalculateClosestPoint`, [`Collision::closest`]): the heightfield
    /// or room tile floor, raised by a KD volume within the client's limited ray (terrain delta + 0.3 m outdoors, 1 m in
    /// dungeons), and never below `liquid level - 1.2`. `None` outside the map / the dungeon rooms.
    pub fn ground(&self, p: [f32; 3]) -> Option<f32> {
        self.closest(p, -1).map(|c| c.pos[1])
    }

    /// Liquid at the ground point `p` (`liquid_probe` with the point itself as the probed height).
    pub fn liquid_at(&self, p: [f32; 3]) -> Option<Liquid> {
        self.liquid_probe(p, p[1], self.rooms.as_ref().and_then(|_| self.room_at(p, -1)))
    }

    /// Liquid test of a closest-point query: `ground` is the closest point found, `y` the queried height, `room` the dungeon room.
    ///
    /// * dungeon (`n3Zone_t::PerformLiquidCollisionTest` @0x1001a80c, `FUN_1000b498`; the room's own list only): the **first**
    ///   triangle whose xz projection holds the point, with `ground.y <` the triangle's top and a plane height above the room origin
    ///   `> 0`, decides; it counts when `level - depth < y` with `depth = (kind >> 5) / 10` m (`kind >> 5 == 0`: 100 000 m, the
    ///   `n3WaterData_t` kind keeps the liquid type in bits 0..4, `FUN_1000b0d1`);
    /// * outdoors: collision infos are appended in authored triangle order by DS `0x1003a0d6`;
    ///   GC `0x100b7f61` registers that list in each intersected zone. N3 `0x1001ab68` returns
    ///   the first positive collision and stops even when that candidate fails authored depth.
    fn liquid_probe(&self, ground: [f32; 3], y: f32, room: Option<usize>) -> Option<Liquid> {
        if let Some(r) = &self.rooms {
            let tag = room? as u16 + 1;
            let oy = r.rooms[tag as usize - 1].0.pos[1];
            for (t, kind) in self.liquids.iter().filter(|(t, _)| t.floor == tag) {
                let top = t.a[1].max(t.b[1]).max(t.c[1]);
                if t.n[1].abs() > 1e-6 && ground[1] < top && t.liquid_contains_xz(ground[0], ground[2]) {
                    let level = t.y_at(ground[0], ground[2]);
                    if level - oy <= 0.0 {
                        continue;
                    }
                    let depth = if kind >> 5 == 0 { 100_000.0 } else { (kind >> 5) as f32 / 10.0 };
                    return (level - depth < y).then_some(Liquid { level, kind: *kind & 0x1f, normal: t.n });
                }
            }
            return None;
        }
        for (t, kind) in &self.liquids {
            let top = t.a[1].max(t.b[1]).max(t.c[1]);
            if t.n[1].abs() > 1e-6 && ground[1] < top && t.liquid_contains_xz(ground[0], ground[2]) {
                let level = t.y_at(ground[0], ground[2]);
                if level > 0.0 {
                    let depth = if kind >> 5 == 0 { 100_000.0 } else { (kind >> 5) as f32 / 10.0 };
                    return (level - depth < y).then_some(Liquid { level, kind: *kind & 0x1f, normal: t.n });
                }
            }
        }
        None
    }

    /// Deepest sphere overlap with a wall triangle (`KDTreeSurface_c::GetSphereIntersection`'s contract): contact point,
    /// unit normal from the surface to `centre`, penetration depth. Floors (`normal.y >= MIN_FLOOR_NY`) and ceilings are skipped.
    /// Wall-overlap query; camera visibility uses [`Collision::line`], and character movement uses [`Collision::align`].
    pub fn sphere_hit(&self, centre: [f32; 3], radius: f32) -> Option<([f32; 3], [f32; 3], f32)> {
        let mut best: Option<([f32; 3], [f32; 3], f32)> = None;
        for t in self.near_box(centre[0], centre[2], radius) {
            if t.n[1].abs() >= MIN_FLOOR_NY {
                continue;
            }
            let q = t.closest(centre);
            let d = sub(centre, q);
            let dist = dot(d, d).sqrt();
            if dist >= radius {
                continue;
            }
            let n = unit(d).unwrap_or(t.n);
            if best.is_none_or(|b| radius - dist > b.2) {
                best = Some((q, n, radius - dist));
            }
        }
        best
    }

    /// Dungeon rooms: index of the room holding `p` (`n3Playfield_t::PosToRoom` @0x1000c8aa: the hint room first, else the
    /// first room that contains it; [`room_contains`]).
    fn room_at(&self, p: [f32; 3], hint: i32) -> Option<usize> {
        let r = self.rooms.as_ref()?;
        let has = |i: usize| room_contains(&r.gnda, &r.rooms[i].0, r.rooms[i].1, [p[0], p[1], -p[2]]);
        if hint >= 0 && (hint as usize) < r.rooms.len() && has(hint as usize) {
            return Some(hint as usize);
        }
        (0..r.rooms.len()).find(|&i| has(i))
    }

    /// Dungeons: whether `p` is inside a room (`n3Playfield_t::PosToRoom`: x/z inside the room rectangle on a non-empty tile
    /// with `y` in the tile's height window). Always true outdoors. The client vetoes every position outside the rooms
    /// (`VetoPosition` @0x10015587 -> `VetoRoomTransition` @0x1001462f); these tile boundaries are the dungeon walls.
    pub fn inside(&self, p: [f32; 3]) -> bool {
        self.rooms.is_none() || self.room_at(p, -1).is_some()
    }

    /// Dungeon: the room holding `p` (index into the playfield record's rooms), see [`Collision::inside`].
    pub fn room_of(&self, p: [f32; 3]) -> Option<usize> {
        self.room_at(p, -1)
    }

    /// `n3Zone_t::IsPosInTeleportal` [N3 0x1001a86a] for a scene-space position (feet; the client passes `Vehicle_t::GetGlobalPos`): the
    /// polygon of the zone holding the point (`GetZoneInstance`: the zone grid outdoors, `PosToRoom` or 0 in a dungeon) contains its
    /// x/z. Zones without a portal never answer. [`Collision::from_scene`] has none.
    pub fn in_teleportal(&self, p: [f32; 3]) -> bool {
        let zone = match (&self.terrain, &self.rooms) {
            (Some(t), _) => zone::grid_zone(t.tm.cell_size, self.zone_size, t.tm.cells_x, t.tm.cells_z, p),
            (None, Some(_)) => self.room_at(p, -1).unwrap_or(0),
            (None, None) => return false,
        };
        self.portals.contains(zone as u32, [p[0], p[1], -p[2]])
    }

    /// Dungeon: the door links `(a, b)`, `a < b`, sorted.
    pub fn room_links(&self) -> Vec<(u16, u16)> {
        let mut v: Vec<_> = self.rooms.iter().flat_map(|r| r.links.iter().copied()).collect();
        v.sort_unstable();
        v
    }

    /// `Door_t` state of the door joining rooms `a` and `b` (`n3Playfield_t::DoorOpened/DoorClosed` @0x1000d2bf; the pass
    /// check of the character is `Door_t::CanPass`, Gamecode `FUN_1007f74d`): `false` keeps the character from crossing.
    /// Links without a registered door (every link at load time) are passable.
    pub fn set_door_passable(&mut self, a: u16, b: u16, passable: bool) {
        if let Some(r) = &mut self.rooms {
            let k = (a.min(b), a.max(b));
            if passable {
                r.blocked.remove(&k);
            } else {
                r.blocked.insert(k);
            }
        }
    }

    /// `n3Playfield_t::DoorOpened` / `DoorClosed` (vtable +0x3c / +0x40, @0x1000d2bf / @0x1000d2e8) -> `ChangeRoomStatus`
    /// @0x1000d17e: sets the open flag of the link joining rooms `a` and `b` (nothing for a pair without a link, equal rooms or
    /// `0xffff`). Every link starts closed.
    pub fn set_door_open(&mut self, a: u16, b: u16, open: bool) {
        if let Some(r) = &mut self.rooms {
            let k = (a.min(b), a.max(b));
            if a != b && a != 0xffff && b != 0xffff && r.links.contains(&k) {
                if open {
                    r.open.insert(k);
                } else {
                    r.open.remove(&k);
                }
            }
        }
    }

    /// `n3Playfield_t::IsDoorOpenBetweenRooms` @0x1000d1e9: the link's flag is 1 (`-1` rooms and unlinked pairs: closed).
    pub fn door_open_between(&self, a: usize, b: usize) -> bool {
        let (a, b) = (a.min(b), a.max(b));
        self.rooms.as_ref().is_some_and(|r| r.open.contains(&(a as u16, b as u16)))
    }

    /// `n3Playfield_t::PosToRoom(p, hint)` @0x1000c8aa for a scene position: the `hint` room first when it holds `p`, else the
    /// first room that does. `None` outdoors and outside every room.
    pub fn pos_to_room(&self, p: [f32; 3], hint: Option<usize>) -> Option<usize> {
        self.room_at(p, hint.map_or(-1, |h| h as i32))
    }

    /// The door link whose door stands within 1.2 m (x and z, a box) of the scene position `p`: `n3Room_t::GetDoorLinkFromPos`
    /// @0x100105f9 asked of every room in order (`Door_t::LinkDoorToRooms`), result `(room, connected room)` of the first match
    /// (feed it to [`Collision::set_door_open`] / [`Collision::set_door_passable`]). A door entry's `tile << 2 | orientation`
    /// word gives the tile of the room's rectangle (row length `x2 - x1`); the door sits on that tile's edge `orientation`
    /// (0: +z, 1: +x, 2: -z, 3: -x, 0.99 m off the tile centre, f64 @0x1003d368), room-local with the room's centre as origin
    /// (`dungeon::room_origin`), turned `rot` quarter turns about +Y and moved by the room position. `None` outdoors / no door.
    pub fn door_link_from_pos(&self, p: [f32; 3]) -> Option<(u16, u16)> {
        const NUDGE: f32 = 0.99;
        const TOLERANCE: f32 = 1.2;
        let r = self.rooms.as_ref()?;
        let (px, pz) = (p[0], -p[2]); // AO world
        for (i, (room, _)) in r.rooms.iter().enumerate() {
            let (w, h) = (room.rect[2].saturating_sub(room.rect[0]) as i32, room.rect[3].saturating_sub(room.rect[1]) as i32);
            if w == 0 {
                continue;
            }
            let half = |n: i32| (((n - 1) & !1) + 1) as f32 * 0.5 * r.gnda.cell;
            let (c, s) = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][room.rot as usize & 3];
            for (&zone, &word) in room.door_zones.iter().zip(&room.door_tiles) {
                let tile = (word >> 2) as i32;
                let (mut x, mut z) = ((tile % w) as f32 * r.gnda.cell - half(w), (tile / w) as f32 * r.gnda.cell - half(h));
                match word & 3 {
                    0 => z += NUDGE,
                    1 => x += NUDGE,
                    2 => z -= NUDGE,
                    _ => x -= NUDGE,
                }
                let (gx, gz) = (room.pos[0] + x * c + z * s, room.pos[2] - x * s + z * c);
                if (px - TOLERANCE..px + TOLERANCE).contains(&gx) && (pz - TOLERANCE..pz + TOLERANCE).contains(&gz) {
                    return Some((i as u16, zone));
                }
            }
        }
        None
    }
}

/// Tile floor triangles of a dungeon room, in scene space (`n3RoomSurface_t::CalculateClosestPoint` @0x10013ee6).
///
/// Atlas tile `(tx, tz)` spans `[tx*2, tx*2+2] x [tz*2, tz*2+2]` m with the corner heights `A = h(tx-1, tz-1)`,
/// `B = h(tx, tz-1)`, `C = h(tx, tz)`, `D = h(tx-1, tz)` (`DHGA` bytes x height scale - room floor minimum, indices clamped at
/// 0) and is split `(A, C, D) | (A, B, C)` along the diagonal A-C. Atlas -> world is `pos + Ry(rot) (atlas - rect origin -
/// (W' + 1, H' + 1))` (`CalcGlobalFromLocalPos` @0x109b6) with `W' = ((x2 - x1 - 1) & !1) + 1`, `y + pos.y`.
fn room_floor(g: &super::dungeon::Gnda, room: &Room, id: u16, out: &mut Vec<Tri>) {
    let [x1, z1, x2, z2] = room.rect.map(|v| v as i32);
    if x2 <= x1 || z2 <= z1 {
        return;
    }
    let min_floor = floor_min(g, room).unwrap_or(0.0);
    let (wp, hp) = ((((x2 - x1 - 1) & !1) + 1) as f32, (((z2 - z1 - 1) & !1) + 1) as f32);
    let (c, s) = [(1.0f32, 0.0f32), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][(room.rot & 3) as usize];
    let hs = g.height_scale;
    let h = |x: i32, z: i32| {
        let (x, z) = (x.max(0).min(g.w as i32 - 1) as usize, z.max(0).min(g.h as i32 - 1) as usize);
        g.floor[z * g.w + x] as f32 * hs - min_floor + room.pos[1]
    };
    // atlas (metres) -> AO world xz -> scene
    let world = |ax: f32, ay: f32, az: f32| {
        let (dx, dz) = (ax - x1 as f32 * 2.0 - (wp + 1.0), az - z1 as f32 * 2.0 - (hp + 1.0));
        let (rx, rz) = (dx * c + dz * s, -dx * s + dz * c);
        [room.pos[0] + rx, ay, -(room.pos[2] + rz)]
    };
    for tz in z1..z2 {
        for tx in x1..x2 {
            if tx as usize >= g.w || tz as usize >= g.h || g.ty[tz as usize * g.w + tx as usize] & 0x7f == 0 {
                continue;
            }
            let (px, pz) = (tx as f32 * 2.0, tz as f32 * 2.0);
            let a = world(px, h(tx - 1, tz - 1), pz);
            let b = world(px + 2.0, h(tx, tz - 1), pz);
            let cc = world(px + 2.0, h(tx, tz), pz + 2.0);
            let d = world(px, h(tx - 1, tz), pz + 2.0);
            for (p, q, r) in [(a, cc, d), (a, b, cc)] {
                let mut n = cross(sub(q, p), sub(r, p));
                if n[1] < 0.0 {
                    n = [-n[0], -n[1], -n[2]];
                }
                if let Some(mut t) = Tri::with_normal(p, q, r, n) {
                    t.floor = id;
                    out.push(t);
                }
            }
        }
    }
}

/// `PlayfieldAnarchy_t::GetSafePos` for a room (Gamecode @0x10121815): the room centre with `y` forced to the highest of the four tile
/// corner heights (`n3Room_t::ForcePosInY` @0x100112cb) when the centre tile is walkable, else the centre of the first walkable tile
/// (x outer, z inner, `GetAPosInRoom` @0x100111a0; its height uses the same corner rule: [GUESS], `GetTileCenterGlobalPos` not decoded).
fn room_safe_pos(g: &Gnda, room: &Room, min_floor: f32) -> [f32; 3] {
    let [x1, z1, x2, z2] = room.rect.map(|v| v as i32);
    let (wp, hp) = ((((x2 - x1 - 1) & !1) + 1), (((z2 - z1 - 1) & !1) + 1));
    let h = |x: i32, z: i32| {
        let (x, z) = (x.max(0).min(g.w as i32 - 1) as usize, z.max(0).min(g.h as i32 - 1) as usize);
        g.floor[z * g.w + x] as f32 * g.height_scale - min_floor + room.pos[1]
    };
    let walkable = |tx: i32, tz: i32| tx >= 0 && tz >= 0 && (tx as usize) < g.w && (tz as usize) < g.h && g.ty[tz as usize * g.w + tx as usize] & 0x7f != 0;
    let top = |tx: i32, tz: i32| h(tx, tz).max(h(tx - 1, tz)).max(h(tx - 1, tz - 1)).max(h(tx, tz - 1));
    let (c, s) = [(1.0f32, 0.0f32), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][(room.rot & 3) as usize];
    let (ctx, ctz) = ((x1 * 2 + wp + 1).div_euclid(2), (z1 * 2 + hp + 1).div_euclid(2));
    if walkable(ctx, ctz) {
        return [room.pos[0], top(ctx, ctz), -room.pos[2]];
    }
    for tx in x1..x2 {
        for tz in z1..z2 {
            if walkable(tx, tz) {
                let (dx, dz) = (tx as f32 * 2.0 + 1.0 - x1 as f32 * 2.0 - (wp + 1) as f32, tz as f32 * 2.0 + 1.0 - z1 as f32 * 2.0 - (hp + 1) as f32);
                return [room.pos[0] + dx * c + dz * s, top(tx, tz), -(room.pos[2] - dx * s + dz * c)];
            }
        }
    }
    [room.pos[0], room.pos[1], -room.pos[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32, x0: f32, x1: f32, z0: f32, z1: f32) -> Vec<Tri> {
        let v = |x, z| [x, y, z];
        [(v(x0, z0), v(x0, z1), v(x1, z1)), (v(x0, z0), v(x1, z1), v(x1, z0))]
            .into_iter()
            .filter_map(|(a, b, c)| {
                let mut n = cross(sub(b, a), sub(c, a));
                if n[1] < 0.0 {
                    n = [-n[0], -n[1], -n[2]];
                }
                Tri::with_normal(a, b, c, n)
            })
            .collect()
    }

    /// A wall `x = 5` spanning z in -10..10, y in 0..3 (two triangles facing -x).
    fn wall() -> Vec<Tri> {
        let (a, b, c, d) = ([5.0, 0.0, -10.0], [5.0, 3.0, -10.0], [5.0, 3.0, 10.0], [5.0, 0.0, 10.0]);
        [(a, b, c), (a, c, d)].into_iter().filter_map(|(p, q, r)| Tri::with_normal(p, q, r, [-1.0, 0.0, 0.0])).collect()
    }

    /// Two triangles `a b c d` (a quad) facing `n`.
    fn panel(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], n: [f32; 3]) -> Vec<Tri> {
        [(a, b, c), (a, c, d)].into_iter().filter_map(|(p, q, r)| Tri::with_normal(p, q, r, n)).collect()
    }

    fn flat_world(extra: Vec<Tri>) -> Collision {
        let mut t = quad(1.0, -20.0, 20.0, -20.0, 20.0);
        t.extend(extra);
        Collision::build(t, None, None, Vec::new())
    }

    #[test]
    fn line_is_one_sided_and_nearest() {
        let c = flat_world(quad(3.0, 10.0, 12.0, -2.0, 2.0));
        // from above: the platform (nearest), through the platform from below: the floor of the platform's underside is not a face
        let h = c.line([11.0, 5.0, 0.0], [11.0, 0.0, 0.0]).unwrap();
        assert!((h.p[1] - 3.0).abs() < 1e-5 && h.n[1] > 0.99);
        assert!((c.line([5.0, 5.0, 0.0], [5.0, 0.0, 0.0]).unwrap().p[1] - 1.0).abs() < 1e-5);
        assert!(c.line([5.0, -5.0, 0.0], [5.0, 0.5, 0.0]).is_none(), "faces are hit from their front only");
        assert!(c.line([5.0, 5.0, 0.0], [5.0, 1.5, 0.0]).is_none(), "the segment ends above the floor");
        assert!(c.line([30.0, 5.0, 0.0], [30.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn walks_head_on_into_a_wall_and_slides_along_it() {
        let c = flat_world(wall());
        let w = c.walk([3.0, 1.0, 0.0], [4.9, 1.0, 0.0]);
        assert!((w.pos[0] - 4.6).abs() < 0.02, "stops one radius from the wall: {w:?}");
        assert!((w.pos[1] - 1.01).abs() < 1e-4 && !w.airborne, "{w:?}");
        // diagonal: the horizontal budget (|step| = 1.965 m) is kept; 1.655 m reach the wall, the rest 0.31 m slides along it
        let w = c.walk([3.0, 1.0, 0.0], [4.9, 1.0, 0.5]);
        assert!((w.pos[0] - 4.6).abs() < 0.02 && (w.pos[2] - 0.731).abs() < 0.02, "{w:?}");
        // open ground is untouched
        let w = c.walk([0.0, 1.0, 0.0], [1.0, 1.0, 1.0]);
        assert!((w.pos[0] - 1.0).abs() < 1e-3 && (w.pos[2] - 1.0).abs() < 1e-3, "{w:?}");
    }

    #[test]
    fn low_obstacles_are_stepped_over_tall_ones_block() {
        // the sphere centre rides 0.4 m above the feet: a 0.3 m riser passes under every ray, 0.6 m does not
        let low = flat_world(panel([5.0, 1.0, -10.0], [5.0, 1.3, -10.0], [5.0, 1.3, 10.0], [5.0, 1.0, 10.0], [-1.0, 0.0, 0.0]));
        assert!((low.walk([3.0, 1.0, 0.0], [5.1, 1.0, 0.0]).pos[0] - 5.1).abs() < 1e-3);
        let tall = flat_world(panel([5.0, 1.0, -10.0], [5.0, 1.6, -10.0], [5.0, 1.6, 10.0], [5.0, 1.0, 10.0], [-1.0, 0.0, 0.0]));
        assert!((tall.walk([3.0, 1.0, 0.0], [5.1, 1.0, 0.0]).pos[0] - 4.6).abs() < 0.02);
    }

    #[test]
    fn ramps_carry_below_sixty_degrees_and_do_not_support_above() {
        let ramp = |rise: f32| {
            let n = unit([-rise, 4.0, 0.0]).unwrap();
            flat_world(panel([5.0, 1.0, -10.0], [9.0, 1.0 + rise, -10.0], [9.0, 1.0 + rise, 10.0], [5.0, 1.0, 10.0], n))
        };
        // 30 degrees (rise 2.31 over 4 m): the walker follows the ramp, the horizontal budget is spent horizontally
        let w = ramp(2.31).walk([4.5, 1.01, 0.0], [5.5, 1.01, 0.0]);
        assert!((w.pos[0] - 5.5).abs() < 0.05 && (w.pos[1] - (1.0 + 0.5 * 0.5775 + 0.01)).abs() < 0.05 && !w.airborne, "{w:?}");
        // 70 degrees (rise 10.99 over 4 m): the sweep still glides up the single plane, but the ground normal of the three
        // rays is below 0.5 (`a4 < 0.5`, Vehicle.dll f32 @0x10012134): nothing carries the character, it keeps falling
        let steep = ramp(10.99);
        let mut w = steep.walk([4.5, 1.01, 0.0], [4.8, 1.01, 0.0]);
        assert!(!w.airborne, "the flat ground in front of the ramp carries: {w:?}");
        for _ in 0..4 {
            w = steep.walk(w.pos, [w.pos[0] + 0.3, w.pos[1], w.pos[2]]);
        }
        assert!(w.airborne && w.normal[1] < 0.5, "{w:?}");
    }

    #[test]
    fn outdoor_liquids_use_authored_order_depth_and_strict_edges() {
        let mut c = Collision::build(Vec::new(), None, None, Vec::new());
        let triangle = |height| [[0.0, height, 0.0], [10.0, height, 0.0], [0.0, height, 10.0]];
        add_outdoor_liquid(triangle(2.0), 2 | (10 << 5), &mut c.liquids).unwrap();
        add_outdoor_liquid(triangle(4.0), 4, &mut c.liquids).unwrap();
        assert_eq!(c.liquid_at([1.0, 1.5, -1.0]).unwrap().level, 2.0, "first collision, not highest");
        assert_eq!(c.liquid_at([1.0, 1.5, -1.0]).unwrap().kind, 2, "depth bits are not liquid flags");
        assert!(c.liquid_at([1.0, 0.5, -1.0]).is_none(), "failed first depth does not select the next liquid");
        assert!(c.liquid_at([0.0, 1.5, -1.0]).is_none(), "native boundary is strict");
        let mut split = Vec::new();
        add_outdoor_liquid([[0.0, 2.0, 0.0], [200.0, 2.0, 0.0], [0.0, 2.0, 200.0]], 2, &mut split).unwrap();
        assert_eq!(split.len(), 4);
        assert!(split.iter().all(|(t, _)| t.n[1] > 0.0));
    }

    #[test]
    fn walking_off_the_edge_is_airborne() {
        let c = Collision::build(quad(1.0, -20.0, 20.0, -20.0, 2.0), None, None, Vec::new());
        let w = c.walk([0.0, 1.0, 1.95], [0.0, 1.0, 2.05]);
        assert!(w.airborne && (w.pos[1] - 1.0).abs() < 1e-5, "{w:?}");
        let w = c.walk([0.0, 1.0, 0.0], [0.0, 1.0, 0.5]);
        assert!(!w.airborne);
    }

    #[test]
    fn closest_point_rays_are_limited_to_the_terrain_delta_plus_0_3() {
        use super::ground::Tilemap;
        let tm = Tilemap { cells_x: 4, cells_z: 4, cell_size: 4.0, height_scale: 256.0, tile_texture: vec![], verts_x: 5, verts_z: 5, heights: vec![0; 25], tiles: vec![0; 16], tile_mask: 0xff };
        let mut kd = quad(3.0, 4.0, 8.0, -8.0, -4.0); // a platform 3 m over the terrain
        kd.extend(quad(-0.5, 8.0, 12.0, -8.0, -4.0)); // a slab 0.5 m under it
        kd.extend(quad(0.2, 0.0, 3.0, -3.0, 0.0)); // a step 0.2 m high
        let c = Collision::build(kd, Some(Terrain { tm }), None, Vec::new());
        let g = |x: f32, y: f32, z: f32| c.ground([x, y, z]).unwrap();
        assert!((g(6.0, 3.2, -6.0) - 3.0).abs() < 1e-5, "standing on the platform");
        assert!((g(6.0, 10.0, -6.0) - 3.0).abs() < 1e-5, "the ray reaches down from a fall");
        assert!(g(6.0, 2.0, -6.0).abs() < 1e-5, "under the platform the terrain carries");
        assert!(g(10.0, 1.0, -6.0).abs() < 1e-5, "a slab below the terrain is never found");
        assert!((g(1.0, 0.5, -1.0) - 0.2).abs() < 1e-5, "the step");
        assert!(g(1.0, 0.1, -1.0).abs() < 1e-5, "below the step top the ray starts under it");
        assert!(c.ground([-1.0, 1.0, -1.0]).is_none(), "outside the map");
        // the retry loop of `EnsureSurfaceAlignment`: a step out of the map is refused and clamped
        let mut p = [-5.0, 1.0, -1.0];
        assert!(c.veto(&mut p, &mut SurfaceState::default()) && (p[0] - 0.1).abs() < 1e-6);
        let w = c.walk([0.5, 0.0, -1.0], [-3.0, 0.0, -1.0]);
        assert!(w.pos[0] >= 0.0, "{w:?}");
    }

    #[test]
    fn terrain_parity_split() {
        use super::ground::Tilemap;
        // 2x2 cells of 4 m, heights chosen so that each parity yields a different plane for the same point
        let mut tm = Tilemap { cells_x: 2, cells_z: 2, cell_size: 4.0, height_scale: 256.0, tile_texture: vec![], verts_x: 3, verts_z: 3, heights: vec![0; 9], tiles: vec![0; 4], tile_mask: 0xff };
        tm.heights[3 + 1] = 4; // vertex (1,1): 4 m (height = value * scale / 256)
        let t = Terrain { tm };
        // cell (0,0): same parity -> diagonal (0,0)-(1,1); point (3,1) is below the diagonal (dx > dz): triangle (P0,P1,P2)
        let (y, n) = t.at(3.0, 1.0).unwrap();
        // plane through (0,0,0), (4,0,0), (4,4,4): y = z -> at z = 1 -> 1.0
        assert!((y - 1.0).abs() < 1e-4, "{y}");
        assert!(n[1] > 0.0);
        // cell (1,0): different parity -> diagonal (1,0)-(0,1) of the cell, i.e. P1-P3; point (5,3): dx+dz = 1+3 = 4 = c -> lower triangle
        assert!(t.at(5.0, 3.0).is_some());
        assert!(t.at(-1.0, 0.0).is_none() && t.at(8.0, 0.0).is_none());
    }

    #[test]
    fn closest_point_on_triangle() {
        let t = Tri::with_normal([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]).unwrap();
        let m = t.closest([0.2, 0.2, 5.0]);
        assert!((m[0] - 0.2).abs() < 1e-6 && (m[1] - 0.2).abs() < 1e-6 && m[2] == 0.0);
        assert_eq!(t.closest([-1.0, -1.0, 0.0]), [0.0, 0.0, 0.0]);
        assert_eq!(t.closest([2.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        let e = t.closest([1.0, 1.0, 0.0]);
        assert!((e[0] - 0.5).abs() < 1e-6 && (e[1] - 0.5).abs() < 1e-6);
    }

    /// Liquid medium state machine (`EnsureSurfaceAlignment` tail): wading follows the ground, deep water floats the feet 1 cm under
    /// the surface and fires the callbacks once, leaving fires the other one; the refusing / hover modes of `Vehicle+0xfc`.
    #[test]
    fn liquid_medium_modes() {
        use super::vehicle::medium;
        let body = Body::WALKING;
        let mut st = SurfaceState::default();
        // shallow (0.5 m over the ground): feet stand on the ground, no callback
        assert_eq!(medium(&mut st, &body, 0.2, 1.0, 0.5), Some(0.51));
        assert!(st.event.is_none() && !st.in_liquid);
        // deep (ground 2 m under the surface, the closest point stops at 1.2 m): feet float 1 cm under the surface, Enter once
        assert_eq!(medium(&mut st, &body, -0.19, 1.0, -0.2), Some(0.99));
        assert_eq!((st.event.take(), st.in_liquid), (Some(LiquidEvent::Enter), true));
        assert_eq!(medium(&mut st, &body, 0.99, 1.0, -0.2), Some(0.99));
        assert!(st.event.is_none(), "still swimming");
        // back to shallow water: Leave, feet on the ground again
        assert_eq!(medium(&mut st, &body, 0.3, 1.0, 0.5), Some(0.51));
        assert_eq!((st.event.take(), st.in_liquid), (Some(LiquidEvent::Leave), false));
        // out of the water (feet >= level + 0.1): nothing, the submersion marker is cleared
        st.submersion = 0.5;
        assert_eq!(medium(&mut st, &body, 1.2, 1.0, 0.5), Some(1.2));
        assert_eq!(st.submersion, -9999.0);
        // without falling the enter callback never fires
        let flying = Body { falling_enabled: false, ..body };
        assert_eq!(medium(&mut st, &flying, 0.0, 1.0, -0.2), Some(0.99));
        assert!(st.event.is_none() && !st.in_liquid);
        // mode 1 refuses deep water (the step is undone), shallow water and dry land pass
        let mut st1 = SurfaceState { medium: 1, ..SurfaceState::default() };
        assert_eq!(medium(&mut st1, &body, 0.0, 1.0, -0.2), None);
        assert_eq!(medium(&mut st1, &body, 0.2, 1.0, 0.5), Some(0.2));
        assert_eq!(medium(&mut st1, &body, 2.0, 1.0, -0.2), Some(2.0));
        // mode 3 only allows deep water, held at most 0.1 m under the surface; mode 4 hovers 0.25 m above it
        let mut st3 = SurfaceState { medium: 3, ..SurfaceState::default() };
        assert_eq!(medium(&mut st3, &body, 2.0, 1.0, -0.2), Some(0.9));
        assert_eq!(medium(&mut st3, &body, 0.0, 1.0, 0.95), None);
        let mut st4 = SurfaceState { medium: 4, ..SurfaceState::default() };
        assert_eq!(medium(&mut st4, &body, 0.0, 1.0, -0.2), Some(1.25));
        // mode 2: callbacks only, the feet are never moved
        let mut st2 = SurfaceState { medium: 2, ..SurfaceState::default() };
        assert_eq!(medium(&mut st2, &body, 0.0, 1.0, -0.2), Some(0.0));
        assert_eq!(st2.event.take(), Some(LiquidEvent::Enter));
    }

    /// A step into a deep pool through `align`: the character ends up floating, the event is reported once.
    #[test]
    fn align_floats_in_deep_water() {
        let pool = vec![(Tri::with_normal([-20.0, 13.0, -20.0], [-20.0, 13.0, 20.0], [20.0, 13.0, 20.0], [0.0, 1.0, 0.0]).unwrap(), 0)];
        let c = Collision::build(quad(10.0, -20.0, 20.0, -20.0, 20.0), None, None, pool);
        let mut st = SurfaceState::default();
        let a = c.align([0.0, 10.1, 5.0], [0.5, 10.1, 5.0], &Body::WALKING, &mut st);
        assert!((a.pos[1] - 12.99).abs() < 1e-4 && a.liquid == 13.0, "{a:?}");
        assert_eq!(st.event, Some(LiquidEvent::Enter));
        assert!((st.submersion - 0.01).abs() < 1e-5, "{}", st.submersion);
        let b = c.align(a.pos, [1.0, a.pos[1], 5.0], &Body::WALKING, &mut st);
        assert!(st.event.is_none() && (b.pos[1] - 12.99).abs() < 1e-4, "{b:?}");
    }
}
