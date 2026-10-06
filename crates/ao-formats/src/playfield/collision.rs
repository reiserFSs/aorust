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
use ao_rdb::RecordStore;
use ao_scene::{Scene, IDENTITY};

use super::dungeon::{floor_min, parse_gnda, Gnda};
use super::record::{self, Rd, Room};
use super::zone::room_contains;
use super::{ground, water, RECORD, TILEMAP};

pub mod kd;

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

const EPS: f32 = 1e-3;
const CELL: f32 = 4.0;
/// A triangle covering more grid cells than this is kept in the always-tested list.
const BIG_CELLS: usize = 256;
/// Cap of the sub-steps of one [`Collision::slide`] (a move of `MAX_SUBSTEPS * radius` metres is swept completely).
const MAX_SUBSTEPS: usize = 256;

/// A liquid volume at a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Liquid {
    /// Surface height (scene y).
    pub level: f32,
    /// `n3WaterData_t` kind (`kind >> 1` selects water / lava / slime / acid / mud, see `water.rs`).
    pub kind: u32,
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
}

/// Dungeon rooms with the tilemap: a position is only valid inside one (`n3RoomSurface_t::VetoPosition` @0x10015587).
struct Rooms {
    gnda: Box<Gnda>,
    /// Room and its lowest floor (`CalculateRoomHeights`).
    rooms: Vec<(Room, f32)>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
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

impl Tri {
    /// Scene-space triangle with an explicit unit normal.
    fn with_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3], n: [f32; 3]) -> Option<Tri> {
        Some(Tri { a, b, c, n: unit(n)? })
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
        Collision { tris, origin: lo, dims, cell, start, items, big, terrain, rooms, liquids }
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
            for room in &rec.rooms {
                room_floor(&g, room, &mut tris);
            }
            let list = rec.rooms.iter().map(|r| (r.clone(), floor_min(&g, r).unwrap_or(65536.0))).collect();
            rooms = Some(Rooms { gnda: Box::new(g), rooms: list });
        }
        for zone in 0..rec.count {
            let Some((version, data)) = store.get_versioned(kd::SURFACE_TYPE, id << 16 | zone)? else { continue };
            let s = kd::parse(version, &data).with_context(|| format!("collision surface {id}:{zone}"))?;
            for v in &s.volumes {
                for t in &v.tris {
                    if let Some(tri) = Tri::from_world(v.verts[t[0] as usize], v.verts[t[1] as usize], v.verts[t[2] as usize]) {
                        tris.push(tri);
                    }
                }
            }
        }
        let mut tail = Rd::new(&raw, rec.tail);
        let mut liquids = Vec::new();
        for w in water::parse(&mut tail).with_context(|| format!("liquids of playfield {id}"))? {
            for t in &w.tris {
                let p = t.map(|i| w.verts[i as usize]);
                if let Some(tri) = Tri::from_world(p[0], p[1], p[2]) {
                    liquids.push((tri, w.kind));
                }
            }
        }
        Ok(Collision::build(tris, terrain, rooms, liquids))
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
        let c = |v: f32, o: f32, n: usize| (((v - o) / self.cell).floor().max(0.0) as usize).min(n - 1);
        let (x0, x1) = (c(x - r, self.origin[0], self.dims[0]), c(x + r, self.origin[0], self.dims[0]));
        let (z0, z1) = (c(z - r, self.origin[1], self.dims[1]), c(z + r, self.origin[1], self.dims[1]));
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

    /// Highest walkable surface at or below `p.y` under `(p.x, p.z)`: the heightfield (outdoor; also when `p` is under it,
    /// the client clamps the position to the terrain) and every walkable triangle (`normal.y >= MIN_FLOOR_NY`: KD volumes,
    /// dungeon tile floors). `None` when nothing supports the point.
    pub fn support(&self, p: [f32; 3]) -> Option<Support> {
        let mut best: Option<Support> = None;
        let mut offer = |y: f32, normal: [f32; 3]| {
            if best.is_none_or(|b| y > b.y) {
                best = Some(Support { y, normal });
            }
        };
        let mut below_terrain = None;
        if let Some((y, n)) = self.terrain.as_ref().and_then(|t| t.at(p[0], -p[2])) {
            if y <= p[1] + EPS {
                offer(y, n);
            } else {
                below_terrain = Some(Support { y, normal: n });
            }
        }
        for t in self.near(p[0], p[2]).filter(|t| t.n[1] >= MIN_FLOOR_NY) {
            if t.contains_xz(p[0], p[2]) {
                let y = t.y_at(p[0], p[2]);
                if y <= p[1] + EPS {
                    offer(y, t.n);
                }
            }
        }
        best.or(below_terrain)
    }

    /// Height of [`Collision::support`].
    pub fn ground(&self, p: [f32; 3]) -> Option<f32> {
        self.support(p).map(|s| s.y)
    }

    /// Ground with the liquid rule of `CalculateClosestPoint`: never lower than `surface - WADE_DEPTH` under a liquid.
    pub fn wading_ground(&self, p: [f32; 3]) -> Option<f32> {
        let g = self.ground(p);
        match self.liquid_at(p) {
            Some(l) => Some(g.map_or(l.level - WADE_DEPTH, |g| g.max(l.level - WADE_DEPTH))),
            None => g,
        }
    }

    /// Liquid covering `(p.x, p.z)` whose surface is at or above `p.y` (the point is submerged), highest surface first.
    pub fn liquid_at(&self, p: [f32; 3]) -> Option<Liquid> {
        let mut best: Option<Liquid> = None;
        for (t, kind) in &self.liquids {
            if t.n[1].abs() > 1e-6 && t.contains_xz(p[0], p[2]) {
                let level = t.y_at(p[0], p[2]);
                if level >= p[1] && best.is_none_or(|b| level > b.level) {
                    best = Some(Liquid { level, kind: *kind });
                }
            }
        }
        best
    }

    /// Deepest sphere overlap with a wall triangle (`KDTreeSurface_c::GetSphereIntersection`'s contract): contact point,
    /// unit normal from the surface to `centre`, penetration depth. Floors (`normal.y >= MIN_FLOOR_NY`) and ceilings are skipped.
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

    /// Moves a character (vertical capsule `radius` x `height`) horizontally from `from` to `to` (scene space, `to.y` is
    /// kept): pushes out of wall triangles (spheres from `STEP_HEIGHT` above the feet to the head, a few passes) and refuses
    /// steps onto terrain / floors steeper than `MIN_FLOOR_NY` that climb. **Approximation** of the Vehicle sphere sliding,
    /// see `docs/zone/collision.md`.
    pub fn slide(&self, from: [f32; 3], to: [f32; 3], radius: f32, height: f32) -> [f32; 3] {
        // sub-steps of at most one radius: a triangle (zero thickness) cannot be skipped by a sphere that overlaps it
        let len = ((to[0] - from[0]).powi(2) + (to[2] - from[2]).powi(2)).sqrt();
        let n = ((len / radius.max(0.05)).ceil() as usize).clamp(1, MAX_SUBSTEPS);
        let mut p = from;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let want = [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t, from[2] + (to[2] - from[2]) * t];
            let mut q = self.push_out(want, radius, height);
            if !self.can_stand(p, q) {
                // try each axis alone, then stay
                let ax = self.push_out([want[0], want[1], p[2]], radius, height);
                let az = self.push_out([p[0], want[1], want[2]], radius, height);
                q = [ax, az].into_iter().find(|c| self.can_stand(p, *c)).unwrap_or([p[0], want[1], p[2]]);
            }
            p = q;
        }
        p
    }

    fn push_out(&self, mut q: [f32; 3], radius: f32, height: f32) -> [f32; 3] {
        let lowest = STEP_HEIGHT + radius;
        let top = (height - radius).max(lowest);
        let rows = ((top - lowest) / (radius * 1.5)).ceil().max(0.0) as usize + 1;
        for _ in 0..4 {
            let mut moved = false;
            for r in 0..rows {
                let y = if rows == 1 { lowest } else { lowest + (top - lowest) * r as f32 / (rows - 1) as f32 };
                if let Some((_, n, depth)) = self.sphere_hit([q[0], q[1] + y, q[2]], radius) {
                    let h = (n[0] * n[0] + n[2] * n[2]).sqrt();
                    if h > 1e-6 {
                        let push = depth / h;
                        q[0] += n[0] * push.min(radius);
                        q[2] += n[2] * push.min(radius);
                        moved = true;
                    }
                }
            }
            if !moved {
                break;
            }
        }
        q
    }

    /// Dungeons: whether `p` is inside a room (`n3Playfield_t::PosToRoom`, [`room_contains`]: x/z inside the room rectangle on
    /// a non-empty tile with `y` in the tile's height window). Always true outdoors. The client vetoes every position outside
    /// the rooms (`VetoPosition` @0x10015587 -> `VetoRoomTransition` @0x1001462f); these tile boundaries are the dungeon walls.
    pub fn inside(&self, p: [f32; 3]) -> bool {
        self.rooms.as_ref().is_none_or(|r| r.rooms.iter().any(|(room, min_floor)| room_contains(&r.gnda, room, *min_floor, [p[0], p[1], -p[2]])))
    }

    /// False when `to` leaves the dungeon rooms or stands on ground steeper than `MIN_FLOOR_NY` that is higher than the ground
    /// under `from`.
    fn can_stand(&self, from: [f32; 3], to: [f32; 3]) -> bool {
        if !self.inside(to) && self.inside(from) {
            return false;
        }
        let lift = |p: [f32; 3]| [p[0], p[1] + RAY_LIFT, p[2]];
        match (self.support(lift(from)), self.support(lift(to))) {
            (Some(a), Some(b)) => b.normal[1] >= MIN_FLOOR_NY || b.y <= a.y + EPS,
            _ => true,
        }
    }
}

/// Tile floor triangles of a dungeon room, in scene space (`n3RoomSurface_t::CalculateClosestPoint` @0x10013ee6).
///
/// Atlas tile `(tx, tz)` spans `[tx*2, tx*2+2] x [tz*2, tz*2+2]` m with the corner heights `A = h(tx-1, tz-1)`,
/// `B = h(tx, tz-1)`, `C = h(tx, tz)`, `D = h(tx-1, tz)` (`DHGA` bytes x height scale - room floor minimum, indices clamped at
/// 0) and is split `(A, C, D) | (A, B, C)` along the diagonal A-C. Atlas -> world is `pos + Ry(rot) (atlas - rect origin -
/// (W' + 1, H' + 1))` (`CalcGlobalFromLocalPos` @0x109b6) with `W' = ((x2 - x1 - 1) & !1) + 1`, `y + pos.y`.
fn room_floor(g: &super::dungeon::Gnda, room: &Room, out: &mut Vec<Tri>) {
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
                if let Some(t) = Tri::with_normal(p, q, r, n) {
                    out.push(t);
                }
            }
        }
    }
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

    #[test]
    fn floor_ground_and_wall_slide() {
        let mut t = quad(1.0, -20.0, 20.0, -20.0, 20.0);
        t.extend(quad(3.0, 10.0, 12.0, -2.0, 2.0)); // a raised platform
        t.extend(wall());
        let c = Collision::build(t, None, None, Vec::new());
        assert_eq!(c.ground([0.0, 5.0, 0.0]), Some(1.0));
        assert_eq!(c.ground([0.0, 0.5, 0.0]), None, "the point is below the floor");
        assert_eq!(c.ground([11.0, 5.0, 0.0]), Some(3.0));
        assert_eq!(c.ground([11.0, 2.0, 0.0]), Some(1.0), "under the platform the floor is the support");
        assert_eq!(c.ground([30.0, 5.0, 0.0]), None);
        // walking into the wall at x = 5 stops at radius, keeps z progress (slide)
        let p = c.slide([3.0, 1.0, 0.0], [4.9, 1.0, 0.5], 0.4, 1.8);
        assert!((p[0] - 4.6).abs() < 0.02, "pushed out to the radius: {p:?}");
        assert!((p[2] - 0.5).abs() < 1e-4, "slides along the wall: {p:?}");
        // open ground is untouched
        assert_eq!(c.slide([0.0, 1.0, 0.0], [1.0, 1.0, 1.0], 0.4, 1.8), [1.0, 1.0, 1.0]);
        // a knee-high obstacle below the step height does not block
        let mut low = quad(1.0, -20.0, 20.0, -20.0, 20.0);
        low.extend([([5.0, 1.0, -10.0], [5.0, 1.3, -10.0], [5.0, 1.3, 10.0])].into_iter().filter_map(|(p, q, r)| Tri::with_normal(p, q, r, [-1.0, 0.0, 0.0])));
        let c = Collision::build(low, None, None, Vec::new());
        assert_eq!(c.slide([3.0, 1.0, 0.0], [5.1, 1.0, 0.0], 0.4, 1.8), [5.1, 1.0, 0.0]);
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
}
