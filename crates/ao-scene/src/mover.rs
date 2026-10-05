//! Traffic ships (`Tweak_Spaceship_*` / `Template_Spaceship_*`): an FXS object follows `UniversePosition[Counter]` and
//! smooths position / heading / banking with `GS_*` accumulators that the script engine evaluates once per frame.
//!
//! Semantics (FXS.dll, see `docs/formats.md` § Traffic ships):
//! * `Counter` is `frac(DayTimeFactor * rate + offset)` ([`Counter`]).
//! * `V[t]` of a `Vector` array (vtable slot `+0x58`, `FUN_10003e31` @0x10003e31): `x = n·t`, `i = round(x − 0.49999)`
//!   (clamped to `n−1`), `V[i] + (V[(i+1 < n) ? i+1 : 0] − V[i])·(x − i)` ([`waypoint_at`]).
//! * A variable's formula runs once per frame (`FUN_10008bd3` stamps `frame + 1`, `FXS_t::FrameProcess` @0x100010ef
//!   increments the frame); a reference to a variable that is being evaluated returns its stored value of the previous
//!   frame. That is what `This.GS_AheadTimePos = UP[c] + This.GS_PreviousPos − This.GS_PreviousPos` relies on
//!   (`GS_PreviousPos` = last frame's `GS_AheadTimePos`).
//! * Operators run left to right (`FUN_10005600` @0x10005600 applies one operand at a time): vector `+ − *` are
//!   per component, `v * s`, `v / s` (s = 0 gives the zero vector), `a [NORMALIZE] s` = `a / |a| · s` (unchanged for the zero
//!   vector), `a [ROT] b` on two vectors = quaternion `(a × h, a·h)` with `h = (a + b)/|a + b|` (`FUN_1000502c`).
//!
//! One frame ([`MoverState::step`]): with `T = UP[Counter]`, `H` = last `GS_AheadTimePos`,
//! `V = (T − H) · CutY`, `A' = ((A + T) − P)·0.15`, `P' = P + A'`, `F' = normalize(F·gain + V)`,
//! `M' = normalize((M·10 + Up)·10 + V)`.

/// Length of the game day in seconds (`GAME.DayTimeFactor = GameDayTime / 6480`).
pub const DAY_LENGTH: f32 = 6480.0;
/// `This.GS_AccCurrent … − This.Position * 0.15`.
pub const SPRING: f32 = 0.15;
/// Banking: `This.GS_AccMod * 10.0 + GAME.UpDirection * 10.0 + This.GS_AccVec`.
pub const BANK_GAIN: f32 = 10.0;
/// Frames simulated by [`MoverState::settled`] and their rate. The filters are per frame, the client frame rate is
/// not stored anywhere **[GUESS: 60 fps]**; 300 frames let every accumulator converge (slowest: 0.816^n).
pub const WARMUP_FRAMES: u32 = 300;
pub const WARMUP_FPS: f32 = 60.0;

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, b: V3) -> V3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
/// `a [NORMALIZE] 1.0`; the zero vector stays zero.
fn normalize(a: V3) -> V3 {
    if a == [0.0; 3] {
        return a;
    }
    scale(a, 1.0 / dot(a, a).sqrt())
}

/// `Counter: GAME.DayTimeFactor * rate + offset [% 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Counter {
    pub rate: f32,
    pub offset: f32,
    pub wrap: bool,
}

impl Default for Counter {
    fn default() -> Self {
        Counter { rate: 1.0, offset: 0.0, wrap: false }
    }
}

impl Counter {
    pub fn at(&self, day_time: f32) -> f32 {
        let v = day_time.rem_euclid(DAY_LENGTH) / DAY_LENGTH * self.rate + self.offset;
        if self.wrap {
            v.rem_euclid(1.0)
        } else {
            v
        }
    }
}

/// `UniversePosition[t]` (`FUN_10003e31`): piecewise linear through the points, the last segment closes the loop.
pub fn waypoint_at(points: &[[f32; 3]], t: f32) -> [f32; 3] {
    let n = points.len();
    if n == 0 {
        return [0.0; 3];
    }
    let x = n as f32 * t;
    let r = (x - 0.49999f32).round_ties_even() as i32;
    let i = if r < 0 || r as usize >= n { n - 1 } else { r as usize };
    let next = if i + 1 < n { i + 1 } else { 0 };
    add(scale(sub(points[next], points[i]), x - i as f32), points[i])
}

/// `a [ROT] b` on two vectors (`FUN_1000502c`): `(a × h, a·h)`, `h = (a + b)/|a + b|`, as `[x, y, z, w]`. Exactly opposite
/// vectors use `(a.z, 0, −a.x)` as the axis (the client divides by the tiny length there and leaves the quaternion
/// unnormalised; normalised here).
pub fn rot_between(a: V3, b: V3) -> [f32; 4] {
    let mut h = add(a, b);
    if dot(h, h) < 1e-12 {
        h = [a[2], 0.0, -a[0]];
    }
    let h = scale(h, 1.0 / dot(h, h).sqrt());
    let c = cross(a, h);
    [c[0], c[1], c[2], dot(a, h)]
}

/// Hamilton product `a·b` (`a [ROT] b`: `b` acts first), `[x, y, z, w]`.
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// One factor of the `Rotation` product, in script order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RotTerm {
    /// A constant `[x, y, z, w]` (`v(0,1,0), 90`).
    Fixed([f32; 4]),
    /// `GAME.ForwardDirection [ROT] This.GS_ForwardVec`.
    Forward,
    /// `GAME.UpDirection [ROT] This.GS_AccMod`.
    Banking,
}

/// A scenery ship: static description, the dynamics live in [`MoverState`]. Vectors are AO space (left handed,
/// playfield metres); [`Mover::transform`] converts to the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Mover {
    /// Index into `Scene::instances`; the renderer overwrites its transform every frame. The mesh is the object-space
    /// mesh (no rotation, no scale).
    pub instance: usize,
    /// `UniversePosition [N]`.
    pub waypoints: Vec<[f32; 3]>,
    pub counter: Counter,
    /// `GS_ForwardVec * gain` (20; freighters 200; VIP shuttles 10).
    pub forward_gain: f32,
    /// `GAME.ForwardDirection`, `GAME.UpDirection`, `GAME.CutY`.
    pub forward_dir: [f32; 3],
    pub up_dir: [f32; 3],
    pub cut_y: [f32; 3],
    pub rotation: Vec<RotTerm>,
    /// `Scale` (`e_ScaleVisibleFarAway` factor as decided by the emitter).
    pub scale: f32,
}

impl Default for Mover {
    fn default() -> Self {
        Mover {
            instance: 0,
            waypoints: vec![],
            counter: Counter::default(),
            forward_gain: 20.0,
            forward_dir: [0.0, 0.0, 1.0],
            up_dir: [0.0, 1.0, 0.0],
            cut_y: [1.0, 0.0, 1.0],
            rotation: vec![],
            scale: 1.0,
        }
    }
}

/// The accumulators of one ship (`GS_AheadTimePos`, `GS_AccCurrent`, `Position`, `GS_ForwardVec`, `GS_AccMod`); all start
/// at zero like every FXS vector variable.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoverState {
    pub ahead: [f32; 3],
    pub acc: [f32; 3],
    pub pos: [f32; 3],
    pub forward: [f32; 3],
    pub banking: [f32; 3],
}

impl Mover {
    /// Waypoint target `UniversePosition[Counter]` at game day time `day_time`.
    pub fn target(&self, day_time: f32) -> [f32; 3] {
        waypoint_at(&self.waypoints, self.counter.at(day_time))
    }

    /// `Rotation` (AO space, `[x, y, z, w]`).
    pub fn rotation(&self, s: &MoverState) -> [f32; 4] {
        self.rotation.iter().fold([0.0, 0.0, 0.0, 1.0], |q, t| {
            quat_mul(
                q,
                match *t {
                    RotTerm::Fixed(f) => f,
                    RotTerm::Forward => rot_between(self.forward_dir, s.forward),
                    RotTerm::Banking => rot_between(self.up_dir, s.banking),
                },
            )
        })
    }

    /// Scene space (z mirrored) instance transform, column-major: `Z · R · Z · scale`, translation `Z · position`.
    pub fn transform(&self, s: &MoverState) -> [[f32; 4]; 4] {
        let [x, y, z, w] = self.rotation(s);
        let n = (x * x + y * y + z * z + w * w).sqrt().max(1e-12);
        let (x, y, z, w) = (x / n, y / n, z / n, w / n);
        let r = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
            [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
            [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
        ];
        let flip = [1.0, 1.0, -1.0];
        let mut m = [[0.0, 0.0, 0.0, 0.0]; 4];
        for (col, mc) in m.iter_mut().enumerate().take(3) {
            for row in 0..3 {
                mc[row] = flip[row] * r[row][col] * flip[col] * self.scale;
            }
        }
        m[3] = [s.pos[0], s.pos[1], -s.pos[2], 1.0];
        m
    }
}

impl MoverState {
    /// One FXS frame towards `target` (`UniversePosition[Counter]` of this frame).
    pub fn step(&mut self, m: &Mover, target: [f32; 3]) {
        let prev = self.ahead; // GS_PreviousPos
        let ahead = sub(add(target, prev), prev); // GS_AheadTimePos
        let vel = mul(sub(ahead, prev), m.cut_y); // GS_AccVec
        // GS_AccCurrent: This.GS_AccCurrent + This.GS_AheadTimePos - This.Position * 0.15 (left to right)
        self.acc = scale(sub(add(self.acc, ahead), self.pos), SPRING);
        self.pos = add(self.acc, self.pos);
        self.forward = normalize(add(scale(self.forward, m.forward_gain), vel));
        self.banking = normalize(add(scale(add(scale(self.banking, BANK_GAIN), m.up_dir), BANK_GAIN), vel));
        self.ahead = ahead;
    }

    /// The state a ship has after `WARMUP_FRAMES` frames at `WARMUP_FPS` that end at `day_time`, while the game clock runs
    /// at `rate` game seconds per second (0 = frozen clock).
    pub fn settled(m: &Mover, day_time: f32, rate: f32) -> MoverState {
        let mut s = MoverState::default();
        for k in (0..=WARMUP_FRAMES).rev() {
            s.step(m, m.target(day_time - k as f32 / WARMUP_FPS * rate));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 0.0, 100.0], [0.0, 0.0, 100.0]];

    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
    }

    #[test]
    fn waypoints_interpolate_and_close_the_loop() {
        // n = 4: t = 0.125 is half way along segment 0, the last segment returns to point 0
        assert!(close(waypoint_at(&SQUARE, 0.0), SQUARE[0], 1e-4));
        assert!(close(waypoint_at(&SQUARE, 0.125), [50.0, 0.0, 0.0], 1e-3));
        assert!(close(waypoint_at(&SQUARE, 0.25), SQUARE[1], 1e-3));
        assert!(close(waypoint_at(&SQUARE, 0.875), [0.0, 0.0, 50.0], 1e-3));
        // t = 1 clamps to the last segment's end = point 0; beyond the loop it keeps extrapolating along it
        assert!(close(waypoint_at(&SQUARE, 1.0), SQUARE[0], 1e-3));
        assert_eq!(waypoint_at(&[], 0.3), [0.0; 3]);
        assert_eq!(waypoint_at(&[[1.0, 2.0, 3.0]], 0.7), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn counter_wraps_and_offsets() {
        let c = Counter { rate: 120.0, offset: 0.0, wrap: true };
        assert!((c.at(27.0) - (27.0 / 6480.0 * 120.0)).abs() < 1e-5, "first loop is 54 s");
        assert!(c.at(54.0 + 27.0) - c.at(27.0) < 1e-3);
        let shifted = Counter { rate: 1.0, offset: 0.2, wrap: true };
        assert!((shifted.at(0.0) - 0.2).abs() < 1e-6 && (shifted.at(6480.0 * 0.9) - 0.1).abs() < 1e-5);
        assert!((Counter::default().at(3240.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn first_frame_follows_the_left_to_right_formulas() {
        let m = Mover { waypoints: vec![[10.0, 5.0, 20.0]], forward_gain: 20.0, ..Default::default() };
        let mut s = MoverState::default();
        s.step(&m, [10.0, 5.0, 20.0]);
        // A = ((0 + T) - 0) * 0.15, P = A + 0
        assert!(close(s.acc, [1.5, 0.75, 3.0], 1e-6) && close(s.pos, s.acc, 1e-6));
        // V = (T - 0) * (1,0,1); F = normalize(0 * 20 + V)
        let l = (100.0f32 + 400.0).sqrt();
        assert!(close(s.forward, [10.0 / l, 0.0, 20.0 / l], 1e-6));
        // M = normalize((0*10 + up) * 10 + V) = normalize((10, 10, 20))
        let l = (100.0f32 + 100.0 + 400.0).sqrt();
        assert!(close(s.banking, [10.0 / l, 10.0 / l, 20.0 / l], 1e-6));
        // second frame: A' = ((A + T) - P) * 0.15 = (A + T - A) * 0.15
        s.step(&m, [10.0, 5.0, 20.0]);
        assert!(close(s.acc, [1.5, 0.75, 3.0], 1e-5) && close(s.pos, [3.0, 1.5, 6.0], 1e-5));
    }

    #[test]
    fn position_converges_to_a_fixed_target_and_banking_to_up() {
        let m = Mover { waypoints: vec![[300.0, 40.0, -70.0]], rotation: vec![RotTerm::Forward, RotTerm::Banking], ..Default::default() };
        let s = MoverState::settled(&m, 100.0, 0.0);
        assert!(close(s.pos, [300.0, 40.0, -70.0], 1e-2), "{:?}", s.pos);
        assert!(close(s.banking, [0.0, 1.0, 0.0], 1e-3) && s.acc.iter().all(|v| v.abs() < 1e-3));
    }

    #[test]
    fn a_moving_target_is_followed_with_lag_and_heading() {
        // the loop is flown in 54 s; the ship trails the target and points along the velocity
        let m = Mover {
            waypoints: SQUARE.to_vec(),
            counter: Counter { rate: 120.0, offset: 0.0, wrap: true },
            rotation: vec![RotTerm::Forward],
            ..Default::default()
        };
        let t = 6.0; // on the first segment (x grows)
        let s = MoverState::settled(&m, t, 1.0);
        let target = m.target(t);
        assert!(s.pos[0] < target[0] && target[0] - s.pos[0] < 5.0, "{:?} {:?}", s.pos, target);
        assert!(close(s.forward, [1.0, 0.0, 0.0], 1e-3), "{:?}", s.forward);
        // forward is exactly the direction the mesh's +Z is turned to: Forward rotation takes (0,0,1) to it
        let q = m.rotation(&s);
        let r = rot_between([0.0, 0.0, 1.0], s.forward);
        assert_eq!(q, r);
    }

    #[test]
    fn rot_between_matches_the_shortest_arc() {
        let q = rot_between([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        // 90 degrees about +Y in the (x, y, z, w) convention of q v q^-1: (0, s, 0, c)
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close([q[0], q[1], q[2]], [0.0, h, 0.0], 1e-6) && (q[3] - h).abs() < 1e-6);
        // zero target: identity, opposite target: a unit half turn
        assert_eq!(rot_between([0.0, 0.0, 1.0], [0.0; 3]), [0.0, 0.0, 0.0, 1.0]);
        let flip = rot_between([0.0, 0.0, 1.0], [0.0, 0.0, -1.0]);
        assert!(flip[3].abs() < 1e-6 && (flip[0] * flip[0] + flip[1] * flip[1] + flip[2] * flip[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn transform_mirrors_z_and_places_the_ship() {
        // forward turned to +X: the mesh's +Z axis (AO) ends on +X, which stays +X in the scene
        let m = Mover { rotation: vec![RotTerm::Forward], scale: 2.0, ..Default::default() };
        let s = MoverState { pos: [5.0, 6.0, 7.0], forward: [1.0, 0.0, 0.0], ..Default::default() };
        let t = m.transform(&s);
        assert_eq!(t[3], [5.0, 6.0, -7.0, 1.0]);
        // AO +Z = scene -Z: scene vector (0,0,-1) maps to 2 * (1,0,0)
        let out: [f32; 3] = std::array::from_fn(|r| -t[2][r]);
        assert!(close(out, [2.0, 0.0, 0.0], 1e-5), "{out:?}");
    }
}
