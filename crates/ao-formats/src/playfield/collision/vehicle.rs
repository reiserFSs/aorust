//! Port of the client's character-versus-world resolution: `Vehicle_t::EnsureSurfaceAlignment` (Vehicle.dll @0x1000d1aa)
//! with its sliding sweep `FUN_1000b2e5` (@0x1000b2e5), on top of the playfield `Surface_i` queries of N3.dll:
//!
//! * `GetLineIntersection` (vtable +0x10): outdoor `n3TilemapSurface_t` @0x10018b72 (terrain cells + `CellSurface_t` KD
//!   volumes, nearest hit), dungeon `n3RoomSurface_t` @0x10015018 (room tile floors + the room's KD volume), the KD tree walk
//!   `FUN_1002dd19` and the face test `FUN_10031c6a` (one sided: only faces whose normal opposes the ray are hit);
//! * `CalculateClosestPoint` (vtable +0x04): [`Collision::closest`] (@0x10018f0c / @0x10013ee6);
//! * `VetoPosition` (vtable +0x20): [`Collision::veto`] (@0x10018a7c / @0x10015587 + `VetoRoomTransition` @0x1001462f) with the
//!   dungeon door rule `PlayfieldAnarchy_t::IsDynelRoomTransitionAllowed` (Gamecode @0x10122371).
//!
//! Everything works in scene coordinates (the AO world mirrored in z, `playfield` docs); the algebra of the original does not
//! depend on handedness, so it is applied unchanged. Constants carry their Vehicle.dll / N3.dll addresses; see
//! `docs/zone/collision.md` §3.4 for the walk-through and the deviations.

use super::{add_v as add, cross, dot, scale_v as scale, sub, unit, zone, Collision, Terrain, Tri};

type V = [f32; 3];

/// Radius of the character's collision sphere (Vehicle.dll f32 @0x100127a0, the `param_2` of `FUN_1000b2e5`): one sphere for
/// every vehicle, centred [`super::RAY_LIFT`] above the feet. It is NOT the per dynel `GetBodyCollSphereRadi` (`CollPrim_t`
/// radius at `dynel+0x5c+0x18`), which only feeds the dynel against dynel test `CheckBodyCollision`.
pub const RADIUS: f32 = 0.4;
/// Length of the probe the sweep aims along its heading (f32 @0x10012790).
const AIM: f32 = 10.0;
/// Sweep iterations per step (the `10` passed by `EnsureSurfaceAlignment`).
const ITERATIONS: u32 = 10;
/// Feet are kept this far above the closest point (f64 @0x100124e0).
pub const FOOT_CLEARANCE: f32 = 0.01;
/// Spread of the three ground rays (f32 @0x100127cc).
const RAY_SPREAD: f32 = 0.04;
/// Step tolerance while walking: `|step| * 1.1547005 + 0.48` (f64 @0x100127d8 and @0x100127d0; the Avatar.Movement RE read
/// the first one as the f32 `2.0`, the instruction is `FMUL double ptr [0x100127d8]`).
const STEP_TOL_SLOPE: f32 = 1.154_700_5;
/// A vertical speed above this keeps a character in the air (f64 @0x100127c0).
const LAND_VY: f32 = 0.1;
/// `vy <=` this for five frames at the same height ends a hover (f32 @0x100127b8).
const STUCK_VY: f32 = -0.1;

/// Vehicle state read by [`Collision::align`] (`Vehicle_t` fields).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// `Vehicle+0x50` (`EnableFalling` / `DisableFalling`): ground following; off while flying.
    pub falling_enabled: bool,
    /// `Vehicle+0x52`: a fall or jump is in progress.
    pub airborne: bool,
    /// `Vehicle+0x54`: vertical speed.
    pub vy: f32,
    /// The `bool` argument: a teleport (`SetRelPos*`), no sweep.
    pub teleport: bool,
}

impl Body {
    /// A walking character standing on the ground.
    pub const WALKING: Body = Body {
        falling_enabled: true,
        airborne: false,
        vy: 0.0,
        teleport: false,
    };
}

/// What the liquid medium state machine asks the vehicle's owner to do (`Vehicle_t` vtable `+0x80` / `+0x84`): the player
/// vehicle's `+0x80` (Gamecode `FUN_1006f99e`) clears MechData and runs the movement FSM transition SwitchToSwimMode (0x1a), `+0x84`
/// (`FUN_1006ef74`) runs LeaveSwimMode (0x23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiquidEvent {
    /// The character went into water deeper than [`SWIM_DEPTH`]: it starts swimming.
    Enter,
    /// The character left deep water (shallow water, out of the water, falling disabled).
    Leave,
}

/// `Vehicle+0x100` (ctor `Vehicle_t::Vehicle_t` @0x1000ce2f, f32 @0x100127a4): water deeper than this over the closest point is
/// "swimming" depth, shallower water is waded. The closest point is never deeper than 1.2 m below the surface
/// (`closest`), so a 1.2 m reading means the ground is at least that far down.
pub const SWIM_DEPTH: f32 = 1.19;

/// State `EnsureSurfaceAlignment` keeps between steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceState {
    /// `Vehicle+0x138`: frames spent hovering at the same height.
    pub stuck: u32,
    /// `n3Dynel_t::GetLastAllowedZoneInst`: the dungeon room of the last accepted position (-1: none yet).
    pub room: i32,
    /// `GetLastAllowedGlobalPositionInZone`.
    pub last: V,
    /// `Vehicle+0xfc`, the liquid medium mode: 0 walker (wades in shallow water, swims in deep water, the default of every
    /// vehicle and never changed for the player: no writer exists in Vehicle.dll / Gamecode.dll), 1 refuses deep water (the step is
    /// undone), 2 callbacks only, 3 refuses everything but deep water, 4 hovers 0.25 m above the surface.
    pub medium: u32,
    /// `Vehicle+0x100`: depth over the closest point that counts as deep ([`SWIM_DEPTH`]).
    pub wade_depth: f32,
    /// `Vehicle+0x120`: the enter callback ran and the leave callback has not.
    pub in_liquid: bool,
    /// `Vehicle+0x10c`: how far the feet are below the surface (set while `feet < level`; -9999 once the feet are above the
    /// surface in modes 0 and 2).
    pub submersion: f32,
    /// The callback this step fired, if any; the caller consumes it (`Option::take`).
    pub event: Option<LiquidEvent>,
    /// `n3Dynel_t::GetBodyCollSphereRadi` (`dynel->vtbl[+0x10]`, @0x10004dd3): radius of the dynel's torso `CollPrim_t`
    /// (`n3VisualDynel_t::UpdateCollision` @0x10019be4: the model's torso sphere radius times the body scale, 0.5 when the model gives a
    /// negative one), the length of the push-back of a refused room transition ([`Collision::veto`]); 0 without a collision primitive.
    pub radius: f32,
    /// Heading of the dynel (yaw, AO world: forward is `(sin, cos)`): the push-back of a refused transition from a standstill goes
    /// along its backwards vector (`rot * (0, 0, -1)`, `n3Dynel_t::GetGlobalRot`).
    pub heading: f32,
    /// `n3Dynel_t` `+0x74 / +0x78` (`SetParentDynelID`, written by `AddChildDynel` @0x100059f7 / `RelocateDynel` @0x10005a7c): the dynel
    /// rides another dynel; `VetoRoomTransition` leaves such dynels alone.
    pub parented: bool,
}

/// `_DAT_1003cb20`: the torso sphere radius of a model that reports a negative one.
pub const DEFAULT_BODY_RADIUS: f32 = 0.5;

impl Default for SurfaceState {
    fn default() -> Self {
        SurfaceState {
            stuck: 0,
            room: -1,
            last: [0.0; 3],
            medium: 0,
            wade_depth: SWIM_DEPTH,
            in_liquid: false,
            submersion: NO_LIQUID,
            event: None,
            radius: DEFAULT_BODY_RADIUS,
            heading: 0.0,
            parented: false,
        }
    }
}

/// Result of one [`Collision::align`] step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aligned {
    /// New feet position.
    pub pos: V,
    /// The `bVar11` of the original: nothing carries the character (start / keep falling); `false` = standing on ground
    /// (the caller lands a falling character, `LandNow`).
    pub airborne: bool,
    /// Ground normal from the three ground rays.
    pub normal: V,
    /// Liquid surface height under the character (`LiquidMediumData_t::m_vLiquidHeight`), -9999 when dry.
    pub liquid: f32,
}

/// A surface hit: point and unit normal (facing the ray origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub p: V,
    pub n: V,
}

/// `CalculateClosestPoint` result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Closest {
    pub pos: V,
    pub normal: V,
    /// Liquid surface height at the point (-9999: none).
    pub liquid: f32,
}

const NO_LIQUID: f32 = -9999.0;
/// Feet clearance over the closest point in liquid medium mode 4 (hover, f64 @0x100127e8).
const HOVER_CLEARANCE: f32 = 0.25;
/// Feet within this of the liquid surface (f64 @0x100127a8) are in the liquid; also the minimum depth of mode 3.
const SURFACE_BAND: f32 = 0.1;
/// Feet float this far under the surface in deep water (f64 @0x100124e0, 0.01).
const FLOAT_OFFSET: f32 = 0.01;
/// Mode 3 keeps the feet at most this far under the surface (f32 @0x100127c0, 0.1).
const SUBMERGE: f32 = 0.1;

/// The liquid medium state machine at the end of `EnsureSurfaceAlignment` (@0x1000dd8c..0x1000e0ec) for `Vehicle+0xfc`
/// (`st.medium`): returns the feet height, or `None` when the whole step is undone (modes 1 and 3 refuse the liquid).
/// `level` is the liquid surface of the closest-point query (`m_vLiquidHeight`), `ground` its height; `depth = level - ground`
/// is the water over the ground, `deep` means above `Vehicle+0x100`. The owner's callbacks are reported through `st.event`.
pub(super) fn medium(
    st: &mut SurfaceState,
    body: &Body,
    feet: f32,
    level: f32,
    ground: f32,
) -> Option<f32> {
    let depth = level - ground;
    let deep = depth > st.wade_depth;
    let falling = body.falling_enabled;
    let leave = |st: &mut SurfaceState| {
        if st.in_liquid {
            st.in_liquid = false;
            st.event = Some(LiquidEvent::Leave);
        }
    };
    // deep water, modes 0 and 2: the enter callback needs falling enabled; a body that stops falling leaves again
    let swim = |st: &mut SurfaceState| {
        if !st.in_liquid {
            if falling {
                st.in_liquid = true;
                st.event = Some(LiquidEvent::Enter);
            }
        } else if !falling {
            st.in_liquid = false;
            st.event = Some(LiquidEvent::Leave);
        }
    };
    match st.medium {
        0 if feet < level + SURFACE_BAND => {
            if !deep {
                leave(st);
                // wading: stay on the ground under the water
                Some(if body.vy <= LAND_VY && falling {
                    level - depth + FOOT_CLEARANCE
                } else {
                    feet
                })
            } else {
                swim(st);
                Some(feet.max(level - FLOAT_OFFSET))
            }
        }
        0 | 2 if feet >= level + SURFACE_BAND => {
            st.submersion = NO_LIQUID;
            leave(st);
            Some(feet)
        }
        2 => {
            if deep {
                swim(st);
            } else {
                leave(st);
            }
            Some(feet)
        }
        1 => (level <= feet || !deep).then_some(feet),
        3 => (depth >= SURFACE_BAND).then(|| feet.min(level - SUBMERGE)),
        4 => Some(feet.max(level + HOVER_CLEARANCE)),
        _ => Some(feet),
    }
}

fn len2(v: V) -> f32 {
    dot(v, v)
}
fn is_zero(v: V) -> bool {
    v == [0.0; 3]
}
fn neg(v: V) -> V {
    [-v[0], -v[1], -v[2]]
}
/// `FUN_10002673`: scale to unit length (the original divides by the length, 0 gives inf; callers test for zero first).
fn norm(v: V) -> V {
    unit(v).unwrap_or(v)
}
fn hlen(v: V) -> f32 {
    (v[0] * v[0] + v[2] * v[2]).sqrt()
}

impl Tri {
    /// `FUN_10031c6a` / `FUN_10031d7d`: the segment `a -> b` against the face. One sided: `n . (b - a) < 0`; the hit parameter
    /// `t` lies in `[-eps, 1 + eps]` (`FUN_100307ea` / `FUN_100307c9`, eps = f32 epsilon), the point inside the three edge planes.
    fn seg_hit(&self, a: V, b: V) -> Option<(f32, V)> {
        let d = sub(b, a);
        let den = dot(self.n, d);
        if den.partial_cmp(&0.0) != Some(std::cmp::Ordering::Less) {
            return None;
        }
        let t = dot(self.n, sub(self.a, a)) / den;
        if !(-f32::EPSILON..=1.0 + f32::EPSILON).contains(&t) {
            return None;
        }
        let p = add(a, scale(d, t));
        let g = cross(sub(self.b, self.a), sub(self.c, self.a));
        let gl = len2(g).sqrt();
        if gl < 1e-12 {
            return None;
        }
        let s = 1.0 / gl;
        for (u, v) in [(self.a, self.b), (self.b, self.c), (self.c, self.a)] {
            let e = sub(v, u);
            let w = dot(cross(e, sub(p, u)), g) * s;
            // a hit on a shared edge belongs to both faces: 10 um tolerance so that no ray slips between them
            if w < 0.0 && w * w > 1e-10 * len2(e) {
                return None;
            }
        }
        Some((t, p))
    }
}

impl Terrain {
    /// The two triangles of terrain cell `(ix, iz)` in AO world space, normals up (`FUN_1001769d`; the parity split of
    /// [`Terrain::at`]).
    fn cell_tris(&self, ix: usize, iz: usize) -> [Tri; 2] {
        let tm = &self.tm;
        let c = tm.cell_size;
        let (x0, z0) = (ix as f32 * c, iz as f32 * c);
        let h = |dx: usize, dz: usize| tm.height(ix + dx, iz + dz);
        let p = [
            [x0, h(0, 0), z0],
            [x0 + c, h(1, 0), z0],
            [x0 + c, h(1, 1), z0 + c],
            [x0, h(0, 1), z0 + c],
        ];
        let sel = if ((!iz) ^ ix) & 1 == 0 {
            [[1, 2, 3], [0, 1, 3]]
        } else {
            [[0, 2, 3], [0, 1, 2]]
        };
        sel.map(|[i, j, k]| {
            let mut n = cross(sub(p[k], p[i]), sub(p[j], p[i]));
            if n[1] < 0.0 {
                n = neg(n);
            }
            Tri {
                a: p[i],
                b: p[j],
                c: p[k],
                n: norm(n),
                floor: 0,
                zone: 0,
            }
        })
    }

    /// Nearest hit of the segment (AO world coordinates) with the heightfield from above (`n3TilemapSurface_t::Intersect_Tile`
    /// @0x1001891d walks the cells along the segment; the cells under its bounding box are tested here).
    fn seg_hit(&self, a: V, b: V) -> Option<(f32, V, V)> {
        let tm = &self.tm;
        let c = tm.cell_size;
        let (w, h) = (tm.cells_x as f32 * c, tm.cells_z as f32 * c);
        let (lo, hi) = (
            [a[0].min(b[0]), a[2].min(b[2])],
            [a[0].max(b[0]), a[2].max(b[2])],
        );
        if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] >= w || lo[1] >= h {
            return None;
        }
        let cell = |v: f32, n: usize| ((v / c).floor().max(0.0) as usize).min(n - 1);
        let mut best: Option<(f32, V, V)> = None;
        for iz in cell(lo[1], tm.cells_z)..=cell(hi[1], tm.cells_z) {
            for ix in cell(lo[0], tm.cells_x)..=cell(hi[0], tm.cells_x) {
                for t in self.cell_tris(ix, iz) {
                    if let Some((s, p)) = t.seg_hit(a, b) {
                        if best.is_none_or(|x| s < x.0) {
                            best = Some((s, p, t.n));
                        }
                    }
                }
            }
        }
        best
    }
}

impl Collision {
    /// Nearest front-face hit of the segment among the grid triangles; `floors` includes the dungeon tile floors. `zone` (0 = any)
    /// limits the KD triangles to the one surface of that zone / room (`tri.zone`); untagged triangles always count.
    fn nearest(&self, a: V, b: V, floors: bool, zone: u32) -> Option<(f32, Hit)> {
        let mut best: Option<(f32, Hit)> = None;
        for t in self.near_aabb(
            a[0].min(b[0]),
            a[2].min(b[2]),
            a[0].max(b[0]),
            a[2].max(b[2]),
        ) {
            if (t.floor != 0 && !floors) || (zone != 0 && t.zone != 0 && t.zone != zone) {
                continue;
            }
            if let Some((s, p)) = t.seg_hit(a, b) {
                if best.is_none_or(|x| s < x.0) {
                    best = Some((s, Hit { p, n: t.n }));
                }
            }
        }
        best
    }

    /// `Surface_i::GetLineIntersection` (vtable +0x10): the nearest surface along `a -> b`, or `None`.
    ///
    /// Outdoors the terrain (from above) and the KD volumes compete, the terrain wins a tie (`FUN_10018b72`). Dungeons only
    /// answer when `a` or `b` is inside a room (@0x10015018), with the tile floors and the room KD volume.
    pub fn line(&self, a: V, b: V) -> Option<Hit> {
        if self.rooms.is_some() && self.room_at(a, -1).is_none() && self.room_at(b, -1).is_none() {
            return None;
        }
        let kd = self.nearest(a, b, true, 0);
        let terrain = self.terrain.as_ref().and_then(|t| {
            let w = |p: V| [p[0], p[1], -p[2]];
            t.seg_hit(w(a), w(b))
                .map(|(s, p, n)| (s, Hit { p: w(p), n: w(n) }))
        });
        match (terrain, kd) {
            (Some(t), Some(k)) => Some(if t.0 <= k.0 { t.1 } else { k.1 }),
            (t, k) => t.or(k).map(|x| x.1),
        }
    }

    /// `Surface_i::CalculateClosestPoint` (vtable +0x04) for the FEET position `p` (`hint`: the dungeon room of the last
    /// position, -1 none). `None` when the client leaves its outputs untouched (outside the map); a dungeon position outside
    /// every room answers the point with `y = 0` and a zero normal.
    ///
    /// * outdoor (@0x10018f0c): terrain height `h` and normal; above the terrain a KD ray of length `p.y - h + 0.3` down from
    ///   `p` replaces it when it hits higher;
    /// * dungeon (@0x10013ee6): tile floor plane at the position, the room KD volume asked with a 1 m ray, the KD hit wins when
    ///   it is not lower;
    /// * both: the character cannot stand deeper than 1.2 m below a liquid surface, `y` is reported at least 0.001 in rooms.
    pub fn closest(&self, p: V, hint: i32) -> Option<Closest> {
        let room = self.rooms.as_ref().and_then(|_| self.room_at(p, hint));
        let (pos, normal) = if let Some(t) = &self.terrain {
            let (h, n) = t.at(p[0], -p[2])?;
            let mut out = ([p[0], h, p[2]], n);
            let dy = p[1] - h;
            if dy >= 0.0 {
                // `GetSurfaceForCell(GetCellIdFromPos(p))`: only the KD surface of the zone holding the point (first non-null of the cell)
                let zone = zone::grid_zone(
                    t.tm.cell_size,
                    self.zone_size,
                    t.tm.cells_x,
                    t.tm.cells_z,
                    p,
                ) as u32
                    + 1;
                if let Some((_, k)) = self.nearest(p, [p[0], p[1] - (dy + 0.3), p[2]], false, zone)
                {
                    if k.p[1] > h {
                        out = (k.p, k.n);
                    }
                }
            }
            out
        } else if self.rooms.is_none() {
            // `from_scene` fallback / synthetic geometry (no surface class): the nearest face within 100 m below the point
            let (_, k) = self.nearest(p, [p[0], p[1] - 100.0, p[2]], true, 0)?;
            (k.p, k.n)
        } else {
            let Some(room) = room else {
                return Some(Closest {
                    pos: [p[0], 0.0, p[2]],
                    normal: [0.0; 3],
                    liquid: NO_LIQUID,
                });
            };
            let (fy, fnorm) = self.tile_floor(room, p)?;
            // FUN_1002e48e: a KD miss answers y = 0, a room without KD surface leaves -1000
            let kd = match self.nearest(p, [p[0], p[1] - 1.0, p[2]], false, room as u32 + 1) {
                Some((_, k)) => Some((k.p, k.n)),
                None if self.kd_zones.contains(&(room as u32 + 1)) => {
                    Some(([p[0], 0.0, p[2]], [0.0, 1.0, 0.0]))
                }
                None => None,
            };
            match kd {
                Some((kp, kn)) if fy <= kp[1] => (kp, kn),
                _ => ([p[0], fy, p[2]], fnorm),
            }
        };
        let (mut pos, mut normal) = (pos, normal);
        let liquid = self.liquid_probe(pos, p[1], room);
        let level = liquid.map_or(NO_LIQUID, |l| l.level);
        if liquid.is_some() && pos[1] < level - super::WADE_DEPTH {
            pos[1] = level - super::WADE_DEPTH;
            normal = [0.0, 1.0, 0.0];
        }
        if self.rooms.is_some() {
            pos[1] = pos[1].clamp(0.001, 1999.9);
        }
        Some(Closest {
            pos,
            normal,
            liquid: level,
        })
    }

    /// Dungeon tile floor plane of `room` under `p` (the ray from `y + 100` down onto the two tile triangles).
    fn tile_floor(&self, room: usize, p: V) -> Option<(f32, V)> {
        self.near(p[0], p[2])
            .find(|t| t.floor as usize == room + 1 && t.contains_xz(p[0], p[2]))
            .map(|t| (t.y_at(p[0], p[2]), t.n))
    }

    /// `PlayfieldAnarchy_t::IsDynelRoomTransitionAllowed` (Gamecode @0x10122371, playfield vtable +0x38) for a character
    /// moving from room `from` to room `to` (-1: no room). Leaving "no room" is always allowed (`n3Playfield_t` base);
    /// otherwise the rooms must be joined by a door link, directly or through one intermediate room, and every `Door_t` on
    /// the way must let the character pass ([`Collision::set_door_passable`]).
    pub fn room_transition_allowed(&self, from: i32, to: i32) -> bool {
        let Some(r) = &self.rooms else { return true };
        if from == -1 {
            return true;
        }
        if to < 0 || from < 0 || from as usize >= r.rooms.len() {
            return false;
        }
        let key = |a: i32, b: i32| (a.min(b) as u16, a.max(b) as u16);
        let doors = |i: i32| {
            r.rooms[i as usize]
                .0
                .door_zones
                .iter()
                .map(|&z| z as i16 as i32)
                .filter(move |&z| z >= 0 && z != i && (z as usize) < r.rooms.len())
        };
        let mut path: Vec<(u16, u16)> = Vec::new();
        if r.links.contains(&key(from, to)) {
            path.push(key(from, to));
        } else {
            // two hops: from -> c -> to
            for c in doors(from) {
                if doors(c).any(|z| z != c && z == to) {
                    path.push(key(from, c));
                    path.push(key(c, to));
                }
            }
        }
        !path.is_empty() && path.iter().all(|k| !r.blocked.contains(k))
    }

    /// `Surface_i::VetoPosition` (vtable +0x20): clamps `p` into the playfield and returns `true` when the position was
    /// refused (the client then asks again closer to the old position, `EnsureSurfaceAlignment`'s retry loop).
    ///
    /// * outdoor (@0x10018a7c): `0 < x < width`, `0 < z < height`, `0 < y < 2000`, clamped to `[0.1, size - 0.1]` / `[0.01, 1999.9]`;
    /// * dungeon (@0x10015587): the fractional part of x / z is kept within `[0.01, 0.99]`, the position clamped to
    ///   `[0.1, 7999.9]`, then `VetoRoomTransition` (@0x1001462f): a position outside every room, or in a room the character
    ///   may not enter ([`Collision::room_transition_allowed`]), goes back to the last allowed position (pushed 0.05 radius
    ///   towards where it came from); accepted positions are remembered in `st`.
    pub fn veto(&self, p: &mut V, st: &mut SurfaceState) -> bool {
        if let Some(t) = &self.terrain {
            let c = t.tm.cell_size;
            let (w, h) = (t.tm.cells_x as f32 * c, t.tm.cells_z as f32 * c);
            let wz = -p[2];
            let ok = p[0] > 0.0 && p[0] < w && wz > 0.0 && wz < h && p[1] > 0.0 && p[1] < 2000.0;
            if !ok {
                p[0] = p[0].clamp(0.1, w - 0.1);
                p[2] = -wz.clamp(0.1, h - 0.1);
                p[1] = p[1].clamp(0.01, 1999.9);
            }
            return !ok;
        }
        if self.rooms.is_none() {
            return false;
        }
        // AO world z = -scene z
        let frac = |v: f32| {
            let f = (v as f64).floor();
            let v = v as f64;
            if v < f + 0.01 {
                (f + 0.01) as f32
            } else if f + 0.99 < v {
                (f + 0.99) as f32
            } else {
                v as f32
            }
        };
        p[0] = frac(p[0]).clamp(0.1, 7999.9);
        p[2] = -frac(-p[2]).clamp(0.1, 7999.9);
        p[1] = p[1].clamp(0.01, 1999.9);
        if st.parented {
            return false;
        }
        let cur = self.room_at(*p, st.room).map_or(-1, |i| i as i32);
        if cur == st.room {
            if cur == -1 {
                *p = self.safe_pos();
                return true;
            }
            st.last = *p;
            return false;
        }
        if !self.room_transition_allowed(st.room, cur) {
            // back to the last allowed position, pushed `0.1 * R / 2` away from the wall: along `last - p` (normalised), from a
            // standstill along the body's backwards vector (scene z is mirrored); a nudge that leaves every room goes the other way
            let to_last = sub(st.last, *p);
            let dir = if is_zero(to_last) {
                [-st.heading.sin(), 0.0, st.heading.cos()]
            } else {
                norm(to_last)
            };
            let nudge = scale([dir[0] * 0.1, 0.0, dir[2] * 0.1], st.radius / 2.0);
            *p = add(st.last, nudge);
            if self.room_at(*p, -1).is_none() {
                *p = sub(st.last, nudge);
            }
            if self.room_at(*p, -1).is_none() {
                *p = self.safe_pos();
            }
            return true;
        }
        st.room = cur;
        st.last = *p;
        false
    }

    /// `PlayfieldAnarchy_t::GetSafePos` dungeon branch (Gamecode @0x10121815): the centre of room `playfield + 0x44`, the index copied by
    /// `n3Playfield_t::CreatePlayfieldFromResource` (@0x1000e006) from the resource's `+0x4c`, which `RDBPlayfield_t` zeroes in its
    /// constructor (@0x1001bf0e) and `ReadBlob` never writes: room 0. Not the last allowed position. See [`room_safe_pos`].
    fn safe_pos(&self) -> V {
        self.rooms
            .as_ref()
            .and_then(|r| {
                r.rooms
                    .first()
                    .map(|(room, min_floor)| super::room_safe_pos(&r.gnda, room, *min_floor))
            })
            .unwrap_or([0.0; 3])
    }

    /// `Vehicle_t::EnsureSurfaceAlignment` (Vehicle.dll @0x1000d1aa) for one integration step `old -> new` (feet positions):
    /// the veto retry loop, the sphere sweep ([`Collision::sweep`]), the closest-point clamp, the three ground rays and the
    /// grounded test with the step tolerance. Orientation mode 0 (the character: heading only, no body tilt).
    ///
    /// The liquid medium of `Vehicle+0xfc` is [`medium`]; its callbacks come back in `st.event`. Not ported (see docs):
    /// orientation modes 1/3/4 (body tilt), `Vehicle+0x13c` (steep slopes allowed, 0 for characters).
    pub fn align(&self, old: V, new: V, body: &Body, st: &mut SurfaceState) -> Aligned {
        st.event = None;
        let flag = if !body.falling_enabled || body.airborne || body.teleport {
            1.0
        } else {
            0.0
        };
        // veto loop: back off along the step in tenths until the position is accepted
        let mut cur = new;
        let step = scale(sub(cur, old), 0.1);
        let mut n = ITERATIONS;
        while self.veto(&mut cur, st) {
            cur = add(old, scale(step, n as f32));
            n -= 1;
            if n == 0 {
                break;
            }
        }
        let delta = sub(cur, old);
        let up = [0.0, super::RAY_LIFT, 0.0];
        let mut c = add(old, up); // sphere centre
        let mut max_y = old[1] + super::RAY_LIFT;
        if len2(delta) > 0.0 {
            if body.teleport {
                c = add([delta[0], delta[1] * flag, delta[2]], c);
            } else {
                let mut h = hlen(delta);
                let mut t = if flag <= 0.0 && h >= 1e-6 {
                    10000.0
                } else {
                    len2(delta).sqrt()
                };
                let mut dir = [delta[0], delta[1] * flag, delta[2]];
                dir = if is_zero(dir) {
                    [0.0, -1.0, 0.0]
                } else {
                    norm(dir)
                };
                let slope = if body.falling_enabled {
                    super::MIN_FLOOR_NY
                } else {
                    -1.0
                };
                self.sweep(
                    &mut c,
                    RADIUS,
                    dir,
                    &mut h,
                    &mut t,
                    &mut max_y,
                    if body.falling_enabled { 3 } else { 1 },
                    ITERATIONS,
                    slope,
                );
            }
        }
        if max_y < c[1] {
            max_y = c[1];
        }
        let (x, z) = (c[0], c[2]);
        let mut feet = c[1] - super::RAY_LIFT;
        let cp = self.closest([x, feet, z], st.room).unwrap_or(Closest {
            pos: [0.0; 3],
            normal: [0.0; 3],
            liquid: NO_LIQUID,
        });
        // mode 4 (hover) keeps 0.25 m (f64 @0x100127e8) instead of 0.01 over the closest point
        let clearance = if st.medium == 4 {
            HOVER_CLEARANCE
        } else {
            FOOT_CLEARANCE
        };
        if feet < cp.pos[1] + clearance {
            feet = cp.pos[1] + clearance;
        }
        max_y = if body.teleport {
            feet + 1.0
        } else {
            max_y.max(feet)
        };
        let mut tol = super::STEP_HEIGHT;
        if body.falling_enabled && !body.airborne && !body.teleport {
            tol = (hlen(delta) * STEP_TOL_SLOPE + 0.48).min(max_y);
        }
        // three ground rays, 0.04 m apart, from the highest point of the sweep (+0.4 m while falling is enabled) down to y = 0
        let from_y = max_y
            + if body.falling_enabled {
                super::RAY_LIFT
            } else {
                0.0
            };
        let s2 = std::f32::consts::FRAC_1_SQRT_2;
        let mut pts = [[0.0; 3]; 3];
        for (pt, d) in pts
            .iter_mut()
            .zip([[1.0, 0.0, 0.0], [-s2, 0.0, s2], [-s2, 0.0, -s2]])
        {
            let (dx, dz) = (d[0] * RAY_SPREAD, d[2] * RAY_SPREAD);
            *pt = self
                .line([x + dx, from_y, z + dz], [x + dx, 0.0, z + dz])
                .map_or([x + dx, 0.0, z + dz], |h| h.p);
        }
        let mut normal =
            unit(cross(sub(pts[1], pts[0]), sub(pts[2], pts[0]))).unwrap_or([0.0, 1.0, 0.0]);
        if normal[1] < 0.0 {
            normal = neg(normal);
        }
        let ray_y = pts.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        let reach = ray_y.max(cp.liquid);
        let mut airborne = true;
        if feet - tol <= reach {
            let ground = ray_y.max(cp.pos[1]);
            if body.vy <= LAND_VY && body.falling_enabled {
                feet = ground + FOOT_CLEARANCE;
            }
            if normal[1] >= super::MIN_FLOOR_NY && body.vy <= LAND_VY {
                airborne = false;
            }
        }
        if feet == old[1] && body.vy <= STUCK_VY && airborne {
            st.stuck += 1;
            if st.stuck > 4 {
                airborne = false;
            }
        } else {
            st.stuck = 0;
        }
        // the liquid medium state machine closes the step (`EnsureSurfaceAlignment` @0x1000dd8c..)
        let (mut x, mut z) = (x, z);
        match medium(st, body, feet, cp.liquid, cp.pos[1]) {
            Some(f) => feet = f,
            None => (x, feet, z) = (old[0], old[1], old[2]),
        }
        if feet < cp.liquid {
            st.submersion = cp.liquid - feet;
        }
        let mut pos = [x, feet, z];
        self.veto(&mut pos, st);
        Aligned {
            pos,
            airborne,
            normal,
            liquid: cp.liquid,
        }
    }

    /// A walking step on the ground: [`Collision::align`] with a standing [`Body`] and fresh state seeded with the room of
    /// `from` (tests and the autopilot; the movement code keeps its own state).
    pub fn walk(&self, from: V, to: V) -> Aligned {
        let mut st = SurfaceState {
            room: self.room_at(from, -1).map_or(-1, |i| i as i32),
            last: from,
            ..SurfaceState::default()
        };
        self.align(from, to, &Body::WALKING, &mut st)
    }

    /// `FUN_1000b2e5`: slides the sphere centre `p` (radius `r`) along `dir` (unit) until the horizontal budget `h` or the total
    /// budget `t` is used up, `iters` aim changes at most. Per iteration (the original's loop):
    ///
    /// 1. aim `d` at `q = p + 10 dir`; with `mode > 1` two probe rays of length `r` to both sides shorten to the walls, the
    ///    centre moves to their midpoint and two rays parallel to the heading are cast from `p +- side / 2`;
    /// 2. the centre ray `p -> q`; the nearest of the three hits (side hits are projected onto the centre line through
    ///    their plane) is the obstacle `(hp, n)`, none: free flight `|q - p| - r` long;
    /// 3. moving up into a face steeper than `slope` (`n.y < slope`) treats it as a vertical wall (effective radius
    ///    `r (1 + n.y^2)`);
    /// 4. the sphere moves to the plane (`hp - d r_eff / cos`), spending the budgets;
    /// 5. unless the contact is head-on (or reversed), the new heading is the tangent `(nc x n) x n` of the contact normal
    ///    `nc = normalize(p - hp)`, kept only if it still points along `dir`.
    #[allow(clippy::too_many_arguments)]
    fn sweep(
        &self,
        p: &mut V,
        r: f32,
        dir: V,
        h: &mut f32,
        t: &mut f32,
        max_y: &mut f32,
        mode: u8,
        mut iters: u32,
        slope: f32,
    ) {
        let mut q = add(*p, scale(dir, AIM));
        // moves `p` by `mv` within the budgets; `false` when a budget ran out (the sweep ends)
        let advance = |p: &mut V, mv: V, h: &mut f32, t: &mut f32, max_y: &mut f32| -> bool {
            let hl = hlen(mv);
            let len = len2(mv).sqrt();
            let frac = if *h <= 0.0 || hl <= 0.0 {
                if len <= 0.0 {
                    return true;
                }
                (*t <= len).then(|| *t / len)
            } else {
                (*h <= hl).then(|| *h / hl)
            };
            match frac {
                Some(f) => {
                    *p = add(*p, scale(mv, f));
                    *max_y = max_y.max(p[1]);
                    *h = 0.0;
                    *t = 0.0;
                    false
                }
                None => {
                    *p = add(*p, mv);
                    *max_y = max_y.max(p[1]);
                    *h -= hl;
                    *t -= len;
                    true
                }
            }
        };
        while iters > 0 {
            iters -= 1;
            let to_q = sub(q, *p);
            if len2(to_q) < 1e-8 {
                return;
            }
            let d = norm(to_q);
            let (mut hit_a, mut hit_b): (Option<Hit>, Option<Hit>) = (None, None);
            if mode > 1 {
                let (mut a, mut b);
                if d[1] * d[1] < 0.999_999 {
                    a = scale(norm(cross(d, [0.0, 1.0, 0.0])), r);
                    b = neg(a);
                } else {
                    (a, b) = ([r, 0.0, 0.0], [-r, 0.0, 0.0]);
                }
                if let Some(x) = self.line(*p, add(*p, a)) {
                    a = sub(x.p, *p);
                }
                if let Some(x) = self.line(*p, add(*p, b)) {
                    b = sub(x.p, *p);
                }
                if len2(a) != len2(b) {
                    let (pa, pb) = (add(*p, a), add(*p, b));
                    *p = scale(add(pa, pb), 0.5);
                    a = sub(pa, *p);
                    b = sub(pb, *p);
                    *max_y = max_y.max(p[1]);
                }
                a = scale(a, 0.5);
                b = scale(b, 0.5);
                // rays parallel to the heading, offset to both sides; the hit is projected onto the centre line
                let side = |off: V, hit: &mut Option<Hit>, p: &V| {
                    if is_zero(off) {
                        return;
                    }
                    let Some(x) = self.line(add(*p, off), add(q, off)) else {
                        return;
                    };
                    let dn = dot(d, x.n);
                    if dot(norm(off), x.n) > 0.0 || dn == 0.0 {
                        return;
                    }
                    let t = dot(sub(x.p, *p), x.n) / dn;
                    *hit = Some(Hit {
                        p: add(*p, scale(d, t)),
                        n: x.n,
                    });
                };
                side(a, &mut hit_a, p);
                side(b, &mut hit_b, p);
            }
            let hit_c = self.line(*p, q);
            if hit_a.is_none() && hit_b.is_none() && hit_c.is_none() {
                // nothing within reach: fly towards q, `r` short of it
                if *p == q {
                    return;
                }
                let l = len2(to_q).sqrt();
                let mv = scale(to_q, (l - r) / l);
                if !advance(p, mv, h, t, max_y) {
                    return;
                }
                q = add(*p, scale(dir, AIM));
                continue;
            }
            // the nearest of the hits that exist (ties: centre, then B, then A; the original's 10000 sentinel for "no hit" is not a
            // distance: a non-finite or very far hit must still win over nothing)
            let obstacle = [hit_c, hit_b, hit_a]
                .into_iter()
                .flatten()
                .min_by(|x, y| len2(sub(x.p, *p)).total_cmp(&len2(sub(y.p, *p))))
                .expect("a hit was found");
            let (hp, n0) = (obstacle.p, obstacle.n);
            let (mut n, mut reff) = (n0, r);
            if d[1] > 0.0 && n0[1] < slope {
                reff = n0[1] * n0[1] * r + r;
                n = if n0[1] <= -0.99
                    || len2(n0) <= 0.001
                    || (n0[0].abs() <= 0.001 && n0[2].abs() <= 0.001)
                {
                    neg(d)
                } else {
                    norm([n0[0], 0.0, n0[2]])
                };
            }
            let back = neg(d);
            let cos = dot(back, n).abs();
            let to_hit = len2(sub(hp, *p));
            let target = if cos <= 0.0 {
                if to_hit > r * r {
                    add(hp, scale(back, r))
                } else {
                    *p
                }
            } else {
                let tt = reff / cos;
                if tt * tt <= to_hit {
                    add(hp, scale(back, tt))
                } else {
                    *p
                }
            };
            if target != *p && !advance(p, sub(target, *p), h, t, max_y) {
                return;
            }
            let nc = sub(*p, hp);
            if is_zero(nc) {
                return;
            }
            let nc = norm(nc);
            let diff = len2(sub(nc, n));
            if !(1e-5..=3.999_99).contains(&diff) || is_zero(n) {
                return;
            }
            let v = norm(cross(cross(nc, n), n));
            if dot(dir, v) <= 0.0 {
                return;
            }
            q = add(*p, scale(v, AIM));
            if n[1] == 0.0 && p[1] < q[1] {
                q[1] = p[1];
            }
        }
    }
}
