//! The zone camera of the own character: `n3Camera_t` with the default `CameraVehicleFixedThird_t` (mode 3) and
//! `CameraVehicleFirstPerson_t` (mode 0) from N3.dll. Pure and headless: avatar position/heading in, an
//! `ao_render::Camera` out. Constants, sign conventions and addresses: docs/zone/camera.md.
//!
//! The scene frame is the renderer's (Y up, forward = (sin yaw, 0, -cos yaw)); `avatar_yaw` is the heading of the
//! avatar in that convention (the integration layer converts the server heading).


use super::camera_views::{Sight, Views};
use super::controls::{CamCmd, CamKey, ControlPrefs};
use ao_render::{Camera, Vec3};
use ao_scene::Lens;
use glam::Quat;
use std::f32::consts::{FRAC_PI_2, PI};

/// The world camera is created with `SetViewPlaneWindow(π/2, aspect)` (`n3Camera_t` ctor, N3 @0x10021a76 / `FUN_1002107a`
/// @0x1002107a, constant `_DAT_1003ccf8`): 90° *horizontal*.
pub const FOV_HORIZONTAL: f32 = FRAC_PI_2;
/// Near clip plane: the third `VisualCamera_t` constructor argument of `n3EngineClient_t::CreateCamera` (N3 @0x10007842, `_DAT_1003ce50`).
pub const NEAR: f32 = 0.2;
/// `ViewDistance` default (`SetDefaultLoginPrefs` GUI @0x10124b33).
pub const VIEW_DISTANCE: f32 = 0.8;

/// Far plane for a `ViewDistance` pref (`FUN_1001fc91`): `view_distance * 1000`, at least `near + 50` (`_DAT_1003e248`).
pub fn far_plane(view_distance: f32) -> f32 {
    (view_distance * 1000.0).max(NEAR + 50.0)
}

/// The lens of the player camera over the playfield's `base` (far plane: the client's [`far_plane`] of the default view
/// distance, which is the fog distance the playfield lens already carries).
pub fn lens(base: Lens) -> Lens {
    Lens { fov: FOV_HORIZONTAL, horizontal: true, near: NEAR, ..base }
}

/// N3 1001ff2f (LocalityListener at camera+0xa4) adds camera+1d4 to the
/// visual eye position and copies the unmodified rotation. Never alter movement.
pub fn apply_ground_shake(camera: &mut Camera, offset: Vec3) {
    camera.pos += offset;
}

#[cfg(test)]
mod ground_shake_test {
    #[test]
    fn shake_changes_visual_eye_not_rotation() {
        let mut camera = ao_render::Camera { pos: glam::Vec3::new(1.0, 2.0, 3.0), yaw: 0.4, pitch: 0.2, roll: 0.0 };
        super::apply_ground_shake(&mut camera, glam::Vec3::new(0.1, 0.2, 0.3));
        assert_eq!(camera.pos, glam::Vec3::new(1.1, 2.2, 3.3));
        assert_eq!((camera.yaw, camera.pitch, camera.roll), (0.4, 0.2, 0.0));
    }
}

/// `PreferredCamPosY` / `PreferredCamPosZ` defaults (GUI `SetDefaultLoginPrefs` @0x10124b33): the unit direction from the
/// look target to the camera in avatar space (+y up, -z behind): elevation `asin(0.316)` = 18.4 degrees.
pub const DEFAULT_DIRECTION: [f32; 3] = [0.0, 0.316, -0.948];
/// `PreferredCamDist` default.
pub const DEFAULT_DISTANCE: f32 = 5.0;
/// Zooming in stops here (`_DAT_1003e48c`); asking for more at < [`FIRST_PERSON_BELOW`] switches to first person.
pub const MIN_DISTANCE: f32 = 0.78;
pub const FIRST_PERSON_BELOW: f32 = 0.8;
/// Zooming out stops here (`_DAT_1003e488`, `1002118c`: `len > 25`).
pub const MAX_DISTANCE: f32 = 25.0;
/// First-person pitch limit, ±89 degrees (`_DAT_1003e320` / `_DAT_1003e31c`).
pub const FIRST_PERSON_PITCH: f32 = 1.553_343;
/// `|dot(direction, up)|` the orbit may reach (`_DAT_1003e300` = 0.9999).
const MAX_ELEVATION_SIN: f32 = 0.9999;
/// Smooth wheel zoom: remaining distance is consumed at `ZOOM_RATE`/s (`_DAT_1005c03c`); below 1 m at a constant
/// `ZOOM_RATE` m/s; the rest is dropped below `ZOOM_STOP` m (`_DAT_1003e29c`).
const ZOOM_RATE: f32 = 3.0;
const ZOOM_STOP: f32 = 0.3;
/// A chase-camera orbit stores the new distance only while the vehicle is at rest below this speed (`_DAT_1003e2e0`), and
/// `ZoomSteer` (@N3 0x1001db64) zooms in until the camera is 0.7 m away (`_DAT_1003e028` is the squared value 0.49).
const ORBIT_REST_SPEED: f32 = 0.02;
const ZOOM_STEER_MIN: f32 = 0.7;
/// Numpad rotation: `0.02 * MouseTurnSensitivity` radians per frame in the original (frame-rate dependent); applied at
/// the 60 Hz equivalent here.
const KEY_ROTATE: f32 = 0.02;
/// Camera collision sphere radius (`n3DynaMeshCollSphere_t` of the camera dynel, `_DAT_1003ce4c`): the line of sight
/// is tested this far beyond the camera position.
pub const COLLISION_RADIUS: f32 = 0.35;
/// Occlusion search (`CameraVehicleFixedThird_t::RecalcOptimalPos` @N3 0x1001f371): bisect `[0.01, 0.95]` of the
/// way to the wanted position, at most 20 steps down to 0.001.
const OCCLUSION_LO: f32 = 0.01;
const OCCLUSION_HI: f32 = 0.95;
const OCCLUSION_EPS: f32 = 0.001;
/// The look target is never lower than this above the feet (`_DAT_1003e29c` clamp at the end of `FUN_10020bdb`, N3
/// @0x10020bdb); it is also what a model without a head attractor gets (`FUN_10020af1` fails, the target stays at 0).
pub const MIN_PIVOT_HEIGHT: f32 = 0.3;
/// Per-call blend of the local target towards the animated head (`FUN_10020bdb`), with no delta-time normalization.
/// Third person keeps 0.8 (`_DAT_1003d9c4`) and takes 0.2 (`_DAT_1003ce50`); first person keeps
/// 0.000999987 (`_DAT_1003e2a8`) and takes 0.999 (`_DAT_1003e2a4`).
const HEAD_BLEND_3RD: f32 = 0.2;
const HEAD_BLEND_1ST: f32 = 0.999;
/// `FUN_10020bdb` calls squared vector norm `FUN_100013a9`: stop below 0.1 m, start above 0.5 m,
/// retaining `DAT_1005c87c` between those thresholds. Cached local x/z do not animate, so only y differs.
const NO_BOB_MIN_SQUARED: f32 = 0.010_000_001;
const NO_BOB_START_SQUARED: f32 = 0.25;
const NO_BOB_KEEP: f32 = 0.99;

/// `Vehicle_t` steering of the camera dynel (Vehicle.dll `SteeringArrive` @0x1000ab28, integrator `FUN_1000e3d3`). Mass,
/// top speed and brake distance are `FUN_1001faa1` / `UpdateMotionConstraints` @N3 0x1001e602 for an avatar that runs
/// (top speed `min(16, 6 * avatar speed)`, force `mass * v / 0.3`, brake `0.3 * v`).
const VEHICLE_MASS: f32 = 20.0;
const VEHICLE_MAX_SPEED: f32 = 16.0;
const VEHICLE_BRAKE: f32 = 0.3 * VEHICLE_MAX_SPEED;
const VEHICLE_MAX_FORCE: f32 = VEHICLE_MASS * VEHICLE_MAX_SPEED / 0.3;
/// Largest integration substep, `Vehicle +0x104`: `CameraVehicle_t`'s constructor stores `_DAT_1003df54` there (N3 @0x1001d440).
/// `FUN_1000e3d3` runs `CalcSteering` once per substep `min(left, +0x104)` (and frames over 4 s, `_DAT_10012804`, not at all).
const VEHICLE_STEP: f32 = 0.05;
const VEHICLE_MAX_FRAME: f32 = 4.0;
/// Arrival radius of the camera vehicle (`_DAT_1003d618`, `SteeringCamArrive`) and of an attractor (`_DAT_1003d61c`).
const ARRIVE_RADIUS: f32 = 0.01;
const ATTRACTOR_RADIUS: f32 = 0.1;
/// `CameraVehicle_t +0x198`, the distance the chase camera keeps from the look target: the constructor sets `_DAT_1003d2e8`
/// (N3 @0x1001d440); `Update` @0x1001e54f sets `|target - pos|`, at least `_DAT_1003e04c` below `_DAT_1003d3a8`; `ForcedUpdate`
/// @0x1001e5ac sets the exact distance.
const CHASE_DEFAULT_DISTANCE: f32 = 5.0;
/// `CalcSteering` lifts the wanted spot by this much while the camera is below the look target or less than this above the
/// ground (`_DAT_1003d9d0`).
const CHASE_LIFT: f32 = 0.4;
/// `SteeringCamArrive` @N3 0x1001dc46: a substep longer than this many times its running average halts the camera
/// (`_DAT_1003d140`); the average moves half way to every new substep (`_DAT_1003c868`).
const HITCH_RATIO: f32 = 10.0;
/// `SteeringCamArrive` sidestep: only when the wanted spot lies farther than this horizontally (`1.0`), within this sine of the
/// direction to the look target (`_DAT_1003d9d0`); it is then moved sideways of the look target by this fraction of the
/// camera's horizontal distance (`_DAT_1003cb20`).
const SIDESTEP_MIN_DISTANCE: f32 = 1.0;
const SIDESTEP_MAX_SINE: f32 = 0.4;
const SIDESTEP: f32 = 0.5;
/// `CalculateSensorSteerDir` @N3 0x1001d955: probe distances `(1 << i) * 0.5` for `i` = 0, 2, 4, 6 and the margin
/// `CanSeeFlexedPos` @0x1001d8c6 steps back from the probe point (`_DAT_1003cb20`).
const SENSOR_STEPS: [f32; 4] = [0.5, 2.0, 8.0, 32.0];
const SENSOR_MARGIN: f32 = 0.5;
/// `FUN_10022345`: the camera counts as blind (`+0x1e9`) while no probe direction sees the target; blind for more than this many
/// seconds (`_DAT_1003caf8`, frames up to `_DAT_1003e29c` long only) it is cut to a new spot (`ReposCutOnAxis(0)`).
const BLIND_CUT_AFTER: f32 = 1.5;
const BLIND_MAX_FRAME: f32 = 0.3;
/// `ReposCutOnAxis` @N3 0x1001e27e: swing about the vertical (`_DAT_1003e048`), the fallback direction (0, 1, `_DAT_1003e040`),
/// the distance halves until the spot sees the target or this limit (`_DAT_1003e038`), at most 10 tries.
const CUT_ANGLE: f32 = 0.541_052_04;
const CUT_FALLBACK: [f32; 3] = [0.0, 1.0, -1.3];
const CUT_MIN_DISTANCE: f32 = 0.8;

/// Scene <-> AO world: the client's world has z negated. The steering code below runs in AO coordinates wherever it uses cross
/// products or a rotation sense, so the decompiled formulas apply as read.
fn ao(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, -v.z)
}

/// The camera dynel as a steered vehicle (modes 1 and 2); mode 3 snaps inward and blends outward radius.
#[derive(Clone, Copy)]
struct Vehicle {
    pos: Vec3,
    vel: Vec3,
    /// `CameraVehicle_t +0x198`, see [`CHASE_DEFAULT_DISTANCE`].
    dist: f32,
    /// `+0x1bc`: `UpdateSensors` found a clear line from the camera to the look target.
    sees: bool,
    /// `+0x1c0..0x1c8`: the direction `CalculateSensorSteerDir` found to get the target in view (scene space).
    steer: Vec3,
    /// `+0x1e9`: no probe direction sees the target.
    blind: bool,
    /// `SteeringCamArrive`'s function-static running average of the substep time (`_DAT_1005c864`, first call: that substep).
    frame_avg: Option<f32>,
}

impl Default for Vehicle {
    fn default() -> Self {
        Self { pos: Vec3::ZERO, vel: Vec3::ZERO, dist: CHASE_DEFAULT_DISTANCE, sees: false, steer: Vec3::ZERO, blind: false, frame_avg: None }
    }
}

impl Vehicle {
    /// A new `CameraVehicle_t` at `pos` (`n3Camera_t` swaps the vehicle on a mode change, FUN_10021859 @N3 0x10021859); the
    /// substep average is a function static and survives.
    fn replaced(&self) -> Self {
        Self { pos: self.pos, frame_avg: self.frame_avg, ..Self::default() }
    }

    /// `Vehicle_t::Run`'s substep loop (`FUN_1000e3d3`): `step(vehicle, h)` once per substep `h <= VEHICLE_STEP`.
    fn run(&mut self, dt: f32, mut step: impl FnMut(&mut Self, f32)) {
        if dt > VEHICLE_MAX_FRAME {
            return;
        }
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(VEHICLE_STEP);
            left -= h;
            step(self, h);
        }
    }

    /// `SteeringArrive(target, radius)` + the integration over one substep: desired velocity = towards the target at
    /// `min(max_speed, distance / brake * max_speed)`, force = `(desired - vel) * mass * 4` limited to `max_force`.
    fn arrive_step(&mut self, target: Vec3, radius: f32, h: f32) {
        let to = target - self.pos;
        let d2 = to.length_squared();
        if d2 < radius * radius || d2 < 0.01 {
            self.vel = Vec3::ZERO; // SteeringHalt
            return;
        }
        let d = d2.sqrt();
        let desired = to / d * (d / VEHICLE_BRAKE * VEHICLE_MAX_SPEED).min(VEHICLE_MAX_SPEED);
        let force = ((desired - self.vel) * (VEHICLE_MASS * 4.0)).clamp_length_max(VEHICLE_MAX_FORCE);
        self.vel = (self.vel + force * h / VEHICLE_MASS).clamp_length_max(VEHICLE_MAX_SPEED);
        self.pos += self.vel * h;
    }

    fn arrive(&mut self, target: Vec3, radius: f32, dt: f32) {
        self.run(dt, |v, h| v.arrive_step(target, radius, h));
    }

    /// `CameraVehicle_t::SteeringCamArrive(target, force, 0.01)` @N3 0x1001dc46 for one substep: a hitch halts the camera;
    /// a wanted spot farther away than the look target in about the same direction is swapped for a spot beside the look
    /// target, so the camera swings round the character instead of flying through it.
    fn cam_arrive_step(&mut self, target: Vec3, look: Vec3, h: f32) {
        let avg = *self.frame_avg.get_or_insert(h);
        if HITCH_RATIO * avg < h {
            self.vel = Vec3::ZERO;
            return;
        }
        self.frame_avg = Some(avg * 0.5 + h * 0.5);
        let (to_look, to_target) = (ao(look - self.pos), ao(target - self.pos));
        let (l, t) = (Vec3::new(to_look.x, 0.0, to_look.z), Vec3::new(to_target.x, 0.0, to_target.z));
        let mut target = target;
        if l != Vec3::ZERO && t != Vec3::ZERO {
            let (c, d) = (l.length(), t.length());
            let (ul, ut) = (l / c, t / d);
            let cross_y = ul.z * ut.x - ul.x * ut.z; // (ul x ut).y
            if c < d && d > SIDESTEP_MIN_DISTANCE && cross_y.abs() < SIDESTEP_MAX_SINE && ut.dot(ul) > 0.0 {
                let side = Vec3::new(l.z, 0.0, -l.x) * if cross_y < 0.0 { -1.0 } else { 1.0 };
                target = look + ao(side * SIDESTEP);
            }
        }
        self.arrive_step(target, ARRIVE_RADIUS, h);
    }

    fn snap(&mut self, pos: Vec3) {
        self.pos = pos;
        self.vel = Vec3::ZERO;
    }

    /// `CameraVehicle_t::CalcSteering` @N3 0x1001e797 without an attractor, zoom or direct control (none of them is reachable
    /// in mode 1: `+0x1a4` is only ever 0, the zoom is the n3Camera's) and with `+0x1cc` (two-shot) and `+0x1e8` (stay behind)
    /// off, which `FUN_10022345` forces every frame (@0x100223b2, @0x100223c3): the camera wants to sit `dist` from the look
    /// target along its current line to it; blocked, the sensor's steer direction is added, in the clear it is lifted off the
    /// ground (< 0.4 m); below the look target it is lifted again.
    fn calc_steering(&self, look: Vec3, sight: &Sight) -> Vec3 {
        let off = look - self.pos;
        let len = off.length();
        let mut want = if len > 0.0 { look - off / len * self.dist } else { look };
        if self.sees {
            if (sight.ground)(self.pos.to_array()).is_some_and(|g| (self.pos.y - g).abs() < CHASE_LIFT) {
                want.y += CHASE_LIFT;
            }
        } else {
            want += self.steer;
        }
        if self.pos.y < look.y {
            want.y += CHASE_LIFT;
        }
        want
    }

    /// `CameraVehicle_t::UpdateSensors` @N3 0x1001e71f (mode 1, every frame before the vehicle runs).
    fn update_sensors(&mut self, look: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) {
        if clear(self.pos.to_array(), look.to_array()) {
            self.blind = false;
            self.sees = true;
        } else {
            self.sees = false;
            self.steer_to_view(look, clear);
        }
    }

    /// `CameraVehicle_t::CalculateSensorSteerDir` @N3 0x1001d955: with the probe distances 0.5, 2, 8, 32 m the first direction
    /// of right, left, up, down, forward, back (of the camera, which faces the look target: `VetoForward` /
    /// `VetoUpAlignment`) from which a point that far away sees the target (`CanSeeFlexedPos` @0x1001d8c6: the line from the
    /// point `0.5 m` short of it to the target and the line to the point are clear) becomes the steer direction (unit length).
    /// None: blind.
    fn steer_to_view(&mut self, look: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) {
        self.steer = Vec3::ZERO;
        self.blind = false;
        let mut fwd = ao(look - self.pos).normalize_or_zero();
        let base = if fwd.x == 0.0 && fwd.z == 0.0 { Vec3::Z } else { Vec3::Y };
        if fwd == Vec3::ZERO {
            fwd = Vec3::Z;
        }
        let up = (base - fwd * fwd.dot(base)).normalize_or_zero();
        let (right, fwd) = (ao(up.cross(fwd)), ao(fwd));
        let dirs = [right, -right, Vec3::Y, -Vec3::Y, fwd, -fwd];
        for step in SENSOR_STEPS {
            for dir in dirs {
                let p = self.pos + dir * step;
                if clear((p - dir * SENSOR_MARGIN).to_array(), look.to_array()) && clear(self.pos.to_array(), p.to_array()) {
                    self.steer = dir;
                    return;
                }
            }
        }
        self.blind = true;
    }

    /// `CameraVehicle_t::ReposCutOnAxis(0)` @N3 0x1001e27e (no attractor, zero axis): halt and put the camera on the far side
    /// of the look target (the line from the camera through it, swung 31 degrees about the vertical to the side `facing`
    /// crosses it, always above it), `dist` away; while that spot does not see the target the distance halves.
    fn cut_on_axis(&mut self, look: Vec3, facing: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) {
        self.vel = Vec3::ZERO;
        let (look_ao, facing) = (ao(look), ao(facing));
        let mut p = ao(self.pos);
        for i in 0.. {
            if i > 0 {
                self.dist *= 0.5;
            }
            let to = look_ao - p;
            let angle = if facing.cross(to).y < 0.0 { CUT_ANGLE } else { -CUT_ANGLE };
            let ahead = Vec3::new(to.x * 2.0, (to.y * 2.0).abs(), to.z * 2.0);
            let v = if ahead != Vec3::ZERO {
                Quat::from_axis_angle(Vec3::Y, angle) * (ahead.normalize() * self.dist)
            } else {
                Vec3::from(CUT_FALLBACK).normalize() * self.dist
            };
            p = look_ao + v;
            let probe = p + v.normalize_or_zero();
            if clear(look.to_array(), ao(probe).to_array()) || i > 9 || self.dist <= CUT_MIN_DISTANCE {
                break;
            }
        }
        self.pos = ao(p);
    }
}

/// Third-person (mode 3) and first-person (mode 0) camera of the own character.
pub struct Camera3p {
    prefs: ControlPrefs,
    first_person: bool,
    /// `PreferredCameraMode` 1..3 (the third-person vehicle).
    mode: u8,
    /// Height of the look target over the feet now (blended towards `head`).
    pivot_height: f32,
    /// First successful scaled local head sample; x/z remain fixed (`N3 FUN_10020af1`).
    head_local: Vec3,
    head_sampled: bool,
    /// Animated head attractor height the look target follows.
    head: f32,
    /// Camera heading relative to the avatar heading (0 = behind it), radians, +dx = looking right.
    yaw_off: f32,
    /// Elevation of the camera above the look target, radians.
    elev: f32,
    dist: f32,
    /// `PreferredCamPos*` / `PreferredCamDist`.
    preferred: (f32, f32, f32),
    /// Wheel zoom still to apply, metres, positive = in (`n3Camera_t +0x204`).
    pending_zoom: f32,
    keys: [bool; 6],
    /// First person: view offset from the avatar heading and look-down pitch (`CameraVehicleFirstPerson_t::SetRotAngles`).
    fp_yaw: f32,
    fp_pitch: f32,
    vehicle: Vehicle,
    vehicle_placed: bool,
    views: Option<Views>,
    /// Shift+F8 was pressed; handled in the next frame (it needs the character's position).
    prev_view: bool,
    /// The look target of the last frame (what an orbit of the vehicle swings about).
    pivot: Vec3,
    /// Seconds the camera has been blind (`n3Camera_t +0x178`).
    blind_time: f32,
    /// `DAT_1005c87c` of `FUN_10020bdb`: the no-bob camera is following the head.
    no_bob_following: bool,
}

impl Camera3p {
    /// The options changed (`LMBMouseLook`, `ZoomSpeed`, ... read live from the DValues, docs/gui.md "Options window"); camera state (mode, first person) stays.
    pub fn set_prefs(&mut self, prefs: &ControlPrefs) {
        self.prefs = ControlPrefs { third_person: self.prefs.third_person, preferred_camera_mode: self.prefs.preferred_camera_mode, ..prefs.clone() };
    }

    /// Optional successful scaled head sample; `None` waits for the first live `set_head`.
    pub fn new(prefs: &ControlPrefs, head_local: Option<Vec3>) -> Self {
        let elev = DEFAULT_DIRECTION[1].asin();
        let head_height = head_local.map_or(MIN_PIVOT_HEIGHT, |h| h.y.max(MIN_PIVOT_HEIGHT));
        Self {
            prefs: prefs.clone(),
            first_person: !prefs.third_person,
            mode: prefs.preferred_camera_mode.clamp(1, 3),
            pivot_height: head_height,
            head_local: head_local.unwrap_or(Vec3::ZERO),
            head_sampled: head_local.is_some(),
            head: head_height,
            yaw_off: 0.0,
            elev,
            dist: DEFAULT_DISTANCE,
            preferred: (0.0, elev, DEFAULT_DISTANCE),
            pending_zoom: 0.0,
            keys: [false; 6],
            fp_yaw: 0.0,
            fp_pitch: 0.0,
            vehicle: Vehicle::default(),
            vehicle_placed: false,
            views: None,
            prev_view: false,
            pivot: Vec3::ZERO,
            blind_time: 0.0,
            no_bob_following: false,
        }
    }

    /// Cache the first successful xyz sample, then follow animated y only (`N3 FUN_10020af1`).
    pub fn set_head(&mut self, head_local: Vec3) {
        if !self.head_sampled {
            self.head_local = head_local;
            self.pivot_height = head_local.y.max(MIN_PIVOT_HEIGHT);
            self.head_sampled = true;
        }
        self.head = head_local.y.max(MIN_PIVOT_HEIGHT);
    }

    /// The playfield's scripted views (`n3Zone_t::GetCameraAttractorList`).
    pub fn set_views(&mut self, v: Views) {
        self.views = Some(v);
    }

    /// Direct CameraMenu selection (CameraCoordinator GUI 0x10064387): 0 first-person, 1/2/3 third-person vehicle.
    pub fn select_mode(&mut self, mode: u8) {
        if mode > 3 { return; }
        self.first_person = mode == 0;
        if mode != 0 && self.mode != mode {
            self.mode = mode;
            self.vehicle = self.vehicle.replaced();
            self.blind_time = 0.0;
        }
    }

    pub fn selected_mode(&self) -> u8 {
        if self.first_person { 0 } else { self.mode }
    }

    /// `PreferredCameraMode` in use (1, 2 or 3).
    #[cfg(test)]
    pub fn mode(&self) -> u8 {
        self.mode
    }

    #[cfg(test)]
    pub fn is_first_person(&self) -> bool {
        self.first_person
    }

    /// Distance look target - camera (third person).
    #[cfg(test)]
    pub fn distance(&self) -> f32 {
        self.dist
    }

    /// Camera heading relative to the avatar and elevation (third person), radians.
    #[cfg(test)]
    pub fn orbit(&self) -> (f32, f32) {
        (self.yaw_off, self.elev)
    }

    /// One frame without occlusion testing.
    #[cfg(test)]
    pub fn update(&mut self, avatar_pos: [f32; 3], avatar_yaw: f32, dt: f32) -> Camera {
        self.update_with(avatar_pos, avatar_yaw, dt, &Sight::OPEN)
    }

    /// Whether the own avatar is drawn: always in third person, in first person only with `ShowMyCharacter`.
    pub fn show_avatar(&self) -> bool {
        !self.first_person || self.prefs.show_my_character
    }

    /// Applies a camera command. Returns `Some(radians)` when a first-person look ended: turn the character by that much
    /// (`N3Msg_EndCameraMouseLook` sends `MovementChanged(0x16, fmod(offset, 2π))`).
    pub fn apply(&mut self, cmd: &CamCmd) -> Option<f32> {
        match *cmd {
            CamCmd::Orbit { dx, dy } => self.rotate(dx, dy),
            CamCmd::Pitch { dy } => {
                let allowed = if self.first_person { self.prefs.rmb_mouse_look_1st } else { self.prefs.rmb_mouse_look_3rd };
                if allowed {
                    self.rotate(0.0, dy);
                }
            }
            CamCmd::Zoom(notches) => self.wheel(notches),
            CamCmd::Key { key, down } => self.keys[key as usize] = down,
            CamCmd::Reset => {
                (self.yaw_off, self.elev, self.dist) = self.preferred;
                self.pending_zoom = 0.0;
                self.fp_yaw = 0.0;
                self.fp_pitch = 0.0;
            }
            CamCmd::SetPreferred => self.preferred = (self.yaw_off, self.elev, self.dist),
            CamCmd::ToggleView => self.first_person = !self.first_person,
            CamCmd::NextView => self.next_view(),
            // The attractor only steers the plain `CameraVehicle_t` (mode 1), see `update_with`; stepping needs a position.
            CamCmd::PrevView => self.prev_view = true,
            CamCmd::EndLook => {
                if self.first_person {
                    let turn = self.fp_yaw.rem_euclid(2.0 * PI);
                    self.fp_yaw = 0.0;
                    return Some(if turn > PI { turn - 2.0 * PI } else { turn });
                }
            }
        }
        None
    }

    /// Ctrl+F8, `n3Camera_t::GetNextVisibleAttractor` @N3 0x10021987: drops the selected attractor and the wheel zoom still
    /// pending and, in third person, switches `PreferredCameraMode` 1 -> 3 -> 2 -> 1 (the old and new vehicle are swapped,
    /// the camera keeps its place; the avatar's heading offset is the same).
    fn next_view(&mut self) {
        if let Some(v) = &mut self.views {
            v.clear();
        }
        self.pending_zoom = 0.0;
        if !self.first_person {
            self.mode = match self.mode {
                1 => 3,
                3 => 2,
                _ => 1,
            };
            self.vehicle = self.vehicle.replaced();
            self.blind_time = 0.0;
        }
    }

    fn rotate(&mut self, dx: f32, dy: f32) {
        if self.first_person {
            self.fp_yaw += dx;
            self.fp_pitch = (self.fp_pitch + dy).clamp(-FIRST_PERSON_PITCH, FIRST_PERSON_PITCH);
        } else {
            self.yaw_off += dx;
            self.elev = (self.elev + dy).clamp(-MAX_ELEVATION_SIN.asin(), MAX_ELEVATION_SIN.asin());
            if self.mode == 1 {
                self.orbit_vehicle(dx, dy);
            }
        }
    }

    /// `FUN_1002118c` @N3 0x1002118c for the plain chase camera (modes != 3 place the vehicle directly): the camera swings about
    /// the look target by `dx` about the vertical and `dy` of elevation (refused when it would pass `|sin| > 0.9999`); when
    /// the vehicle is (nearly) at rest (`< 0.02`, `_DAT_1003e2e0`) `Update` + `ForcedUpdate` then store the new distance.
    fn orbit_vehicle(&mut self, dx: f32, dy: f32) {
        let off = self.vehicle.pos - self.pivot;
        let len = off.length();
        if len <= 0.0 {
            return;
        }
        let dy = if (off.y / len + dy).abs() > MAX_ELEVATION_SIN { 0.0 } else { dy };
        let el = (off.y / len).asin() + dy;
        let (c, s) = (dx.cos(), dx.sin());
        let (x, z) = (off.x * c - off.z * s, off.x * s + off.z * c);
        let h = x.hypot(z);
        let k = if h > 0.0 { len * el.cos() / h } else { 0.0 };
        if self.vehicle.vel.length() < ORBIT_REST_SPEED {
            self.vehicle.dist = len;
        }
        self.vehicle.pos = self.pivot + Vec3::new(x * k, len * el.sin(), z * k);
    }

    /// `n3Camera_t` wheel handler (N3 @0x100200ef): `ZoomSpeed / 10` metres per notch, accumulated; scrolling the
    /// other way first cancels what is left. Scrolling out of first person goes back to third person.
    fn wheel(&mut self, notches: f32) {
        if self.first_person {
            if notches < 0.0 && self.prefs.zoom_to_1st_person {
                self.first_person = false;
            }
            return;
        }
        if self.pending_zoom * notches < 0.0 {
            self.pending_zoom = 0.0;
        }
        self.pending_zoom += self.prefs.zoom_speed / 10.0 * notches;
    }

    /// Distance change request (positive = in); switches to first person when it runs into the minimum.
    fn zoom_in_by(&mut self, metres: f32) {
        if self.mode == 1 {
            // [INFERENCE] `ZoomSteer` (@0x1001db64: seek along the line to the look target, stop at 0.7 m / 25 m) followed by
            // `ForcedUpdate` leaves the chase distance changed by the zoom: stored directly.
            self.vehicle.dist = (self.vehicle.dist - metres).clamp(ZOOM_STEER_MIN, MAX_DISTANCE);
        }
        self.dist -= metres;
        if self.dist <= FIRST_PERSON_BELOW && metres > 0.0 && self.prefs.zoom_to_1st_person {
            self.first_person = true;
            self.pending_zoom = 0.0;
            self.dist = self.dist.max(MIN_DISTANCE);
            return;
        }
        self.dist = self.dist.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    fn step_keys_and_zoom(&mut self, dt: f32) -> bool {
        let k = |c: CamKey| self.keys[c as usize];
        let turn = KEY_ROTATE * self.prefs.mouse_turn_sensitivity * dt * 60.0;
        let dx = if k(CamKey::RotateLeft) { turn } else { 0.0 } - if k(CamKey::RotateRight) { turn } else { 0.0 };
        let dy = if k(CamKey::RotateDown) { turn } else { 0.0 } - if k(CamKey::RotateUp) { turn } else { 0.0 };
        let zoom = (if k(CamKey::ZoomIn) { 1.0 } else { 0.0 } - if k(CamKey::ZoomOut) { 1.0 } else { 0.0 }) * dt * self.prefs.zoom_speed;
        let distance_change = !self.first_person && (zoom != 0.0 || self.pending_zoom != 0.0);
        if dx != 0.0 || dy != 0.0 {
            self.rotate(dx, dy);
        }
        if zoom != 0.0 && !self.first_person {
            self.zoom_in_by(zoom);
        }
        if !self.first_person && self.pending_zoom != 0.0 {
            let p = self.pending_zoom;
            let step = if p.abs() >= 1.0 { dt * p * ZOOM_RATE } else { dt * ZOOM_RATE * p.signum() };
            self.pending_zoom = p - step;
            if self.pending_zoom * p <= 0.0 || self.pending_zoom.abs() < ZOOM_STOP {
                self.pending_zoom = 0.0;
            }
            self.zoom_in_by(step);
        }
        distance_change
    }

    /// `FUN_10020bdb`: blend once per call, independent of frame time.
    fn follow_head(&mut self) {
        if self.prefs.no_bob_camera {
            let gap = self.head - self.pivot_height;
            let squared_gap = gap * gap;
            if squared_gap < NO_BOB_MIN_SQUARED {
                self.no_bob_following = false;
            } else if self.no_bob_following || squared_gap > NO_BOB_START_SQUARED {
                self.no_bob_following = true;
                self.pivot_height = (NO_BOB_KEEP * self.pivot_height + 0.01 * self.head).max(MIN_PIVOT_HEIGHT);
            }
            return;
        }
        let (keep, take) = if self.first_person { (0.000_999_987, HEAD_BLEND_1ST) } else { (0.8, HEAD_BLEND_3RD) };
        self.pivot_height = (keep * self.pivot_height + take * self.head).max(MIN_PIVOT_HEIGHT);
    }


    /// One frame. `sight` answers what the camera asks of the world: line of sight (free of terrain/statels), closed doors
    /// between rooms and the ground under a point (scene frame).
    pub fn update_with(&mut self, avatar_pos: [f32; 3], avatar_yaw: f32, dt: f32, sight: &Sight) -> Camera {
        let clear = sight.clear;
        let distance_change = self.step_keys_and_zoom(dt);
        self.follow_head();
        let feet = Vec3::from(avatar_pos);
        let local = Vec3::new(self.head_local.x, self.pivot_height, self.head_local.z);
        let pivot = feet + glam::Quat::from_rotation_y(super::zone::scene_yaw(avatar_yaw)) * local;
        self.pivot = pivot;
        if self.first_person {
            return Camera { pos: pivot, yaw: avatar_yaw + self.fp_yaw, pitch: -self.fp_pitch, roll: 0.0 };
        }
        // scripted views: ranked every 10th frame and Shift+F8 steps them
        let guide = self.views.as_ref().and_then(Views::selected).map_or(self.vehicle.pos, |v| v.pos);
        if let Some(v) = &mut self.views {
            v.tick(dt, feet, guide, sight);
            if std::mem::take(&mut self.prev_view) {
                v.prev(feet, guide, sight);
            }
        }
        let h = avatar_yaw + self.yaw_off;
        let fwd = Vec3::new(h.sin(), 0.0, -h.cos());
        let dir = -fwd * self.elev.cos() + Vec3::Y * self.elev.sin();
        let want = pivot + dir * self.dist;
        let optimal = occlude(pivot, want, dir, clear);
        if self.mode == 3 && distance_change {
            // `FUN_10022345` (between labels 0x100226bd and 0x100227e9): zoom places the wanted eye
            // (`SetRelPos`, `Update`, `ForcedUpdate`) before `DecideSnap`.
            // Key zoom `FUN_1002118c` likewise calls `SetRelPos` before `UpdateHeadingToPos`.
            // Occlusion still snaps inward below; only passive radial return blends.
            self.vehicle.snap(want);
        }
        if self.mode != 3 && self.vehicle.pos.x == 0.0 && self.vehicle.pos.z == 0.0 {
            // `FUN_10022345`: a vehicle that was never placed starts at the camera's own spot and takes its distance
            // (`SetRelPosIgnoreCollision`, `Update`, `ForcedUpdate(0)`)
            self.vehicle.snap(optimal);
            self.vehicle.dist = (pivot - optimal).length();
        }
        let eye = match self.mode {
            // `DecideSnap` @N3 0x1001f537: inward immediate, outward 0.9 old + 0.1 candidate radius.
            3 => {
                let offset = optimal - pivot;
                let radius = offset.length();
                let current = (self.vehicle.pos - pivot).length();
                // Preserve the existing initial placement until retail initialization is resolved:
                // never blend an unplaced Vec3::ZERO eye across the world.
                let eye = if self.vehicle_placed && radius > current {
                    // Candidate direction, not current direction; no angular smoothing or dt normalization.
                    // `_DAT_1003d3a8` / `_DAT_1003c890` are the f32 values 0.9 / 0.1 per call.
                    pivot + offset / radius * (current * 0.9 + radius * 0.1)
                } else {
                    optimal
                };
                self.vehicle.snap(eye);
                eye
            }
            // CameraVehicleFixedThird_t(damped): `SteeringCamArrive(optimal, 0.01)`
            2 => {
                self.vehicle.run(dt, |v, h| v.cam_arrive_step(optimal, pivot, h));
                self.vehicle.pos
            }
            // CameraVehicle_t (`CalcSteering` @N3 0x1001e797), with the sensors and the blind-camera cut of `FUN_10022345`
            _ => {
                self.vehicle.update_sensors(pivot, clear);
                if !self.vehicle.blind {
                    self.blind_time = 0.0;
                } else if dt < BLIND_MAX_FRAME {
                    self.blind_time += dt;
                }
                if self.blind_time > BLIND_CUT_AFTER {
                    let facing = Vec3::new(avatar_yaw.sin(), 0.0, -avatar_yaw.cos()); // the look target's rotation x `cReferenceForward`
                    self.vehicle.cut_on_axis(pivot, facing, clear);
                    self.blind_time = 0.0;
                }
                match self.views.as_ref().and_then(Views::selected) {
                    Some(v) => self.vehicle.arrive(v.pos, ATTRACTOR_RADIUS, dt),
                    None => self.vehicle.run(dt, |veh, h| {
                        let want = veh.calc_steering(pivot, sight);
                        veh.cam_arrive_step(want, pivot, h);
                    }),
                }
                self.vehicle.pos
            }
        };
        self.vehicle_placed = true;
        Camera::look_at(eye, pivot)
    }
}

/// Pulls the camera towards the look target until the line of sight is free.
fn occlude(pivot: Vec3, want: Vec3, dir: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) -> Vec3 {
    let free = |p: Vec3| clear(pivot.to_array(), (p + dir * COLLISION_RADIUS).to_array());
    if free(want) {
        return want;
    }
    let (mut lo, mut hi) = (OCCLUSION_LO, OCCLUSION_HI);
    let mut mid = lo;
    for _ in 0..20 {
        mid = (lo + hi) * 0.5;
        if free(pivot + (want - pivot) * mid) {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= OCCLUSION_EPS {
            break;
        }
    }
    // The original stores every tested midpoint at +0x208, including the final
    // blocked one; it does not replace it with the last-clear lower bound.
    pivot + (want - pivot) * mid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera3p {
        Camera3p::new(&ControlPrefs::default(), Some(Vec3::Y * 1.5))
    }

    #[test]
    fn zoom_out_places_requested_radius_before_radial_return() {
        for wheel in [true, false] {
            let mut c = cam();
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
            if wheel {
                c.apply(&CamCmd::Zoom(-1.0));
            } else {
                c.apply(&CamCmd::Key { key: CamKey::ZoomOut, down: true });
            }
            let eye = c.update([0.0; 3], 0.0, 1.0 / 60.0);
            let requested = if wheel { 5.1 } else { 5.0 + 20.0 / 60.0 };
            assert!(((eye.pos - c.pivot).length() - requested).abs() < 1e-5);
            assert!((c.dist - requested).abs() < 1e-5);
        }
    }

    #[test]
    fn missing_initial_head_waits_for_successful_sample_in_both_views() {
        for third_person in [true, false] {
            let mut c = Camera3p::new(&ControlPrefs { third_person, ..Default::default() }, None);
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
            assert!(!c.head_sampled);
            assert!(near(c.pivot.y, MIN_PIVOT_HEIGHT));
            c.set_head(Vec3::new(0.4, 1.8, -0.2));
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
            assert!((c.pivot - Vec3::new(0.4, 1.8, -0.2)).length() < 1e-5);
            c.set_head(Vec3::new(8.0, 1.8, 9.0));
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
            assert!((c.pivot - Vec3::new(0.4, 1.8, -0.2)).length() < 1e-5);
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn fixed_third_snaps_inward_and_blends_only_outward_radius_per_call() {
        let feet = [1200.0, 50.0, -900.0];
        let pivot = Vec3::from(feet) + Vec3::Y * 1.5;
        let mut c = cam();
        let initial = c.update(feet, 0.0, 0.016);
        assert!(near((initial.pos - pivot).length(), 5.0), "initial eye is not blended from world zero");
        c.dist = 2.0;
        let inward = c.update(feet, 0.0, 0.016);
        assert!(near((inward.pos - pivot).length(), 2.0));
        c.dist = 5.0;
        let outward = c.update(feet, FRAC_PI_2, 0.001);
        let direction = Vec3::new(-c.elev.cos(), c.elev.sin(), 0.0);
        assert!((outward.pos - (pivot + direction * 2.3)).length() < 1e-3, "new direction applies immediately");
        let next = c.update(feet, FRAC_PI_2, 0.2);
        assert!(near((next.pos - pivot).length(), 2.3 * 0.9 + 5.0 * 0.1), "blend is per call, not per second");
    }

    #[test]
    fn menu_selects_each_vehicle_and_retains_third_person_preference() {
        let mut c = cam();
        let orbit = c.orbit();
        let distance = c.distance();
        for mode in [1, 2, 3, 0] {
            c.select_mode(mode);
            assert_eq!(c.selected_mode(), mode);
            assert_eq!(c.orbit(), orbit);
            assert_eq!(c.distance(), distance);
            assert_eq!(c.is_first_person(), mode == 0);
        }
        assert_eq!(c.mode(), 3);
        c.apply(&CamCmd::ToggleView);
        assert_eq!(c.selected_mode(), 3);
        c.select_mode(2);
        c.set_prefs(&ControlPrefs::default());
        assert_eq!(c.selected_mode(), 2);
    }

    #[test]
    fn default_view_is_five_metres_behind_and_above() {
        let mut c = cam();
        // avatar at (10, 2, -3) facing -Z (yaw 0): the camera is on the +Z side
        let v = c.update([10.0, 2.0, -3.0], 0.0, 0.016);
        let pivot = Vec3::new(10.0, 3.5, -3.0);
        assert!(near((v.pos - pivot).length(), 5.0));
        assert!(near(v.pos.x, 10.0) && v.pos.z > -3.0);
        assert!(near(v.pos.y - pivot.y, 5.0 * 0.316));
        assert!(near(v.pos.z - pivot.z, 5.0 * (1.0 - 0.316f32 * 0.316).sqrt()));
        // it looks at the pivot: forward points from the camera to it
        assert!((v.forward() - (pivot - v.pos).normalize()).length() < 1e-3);
    }

    #[test]
    fn follows_avatar_heading() {
        let mut c = cam();
        // facing +X (yaw 90 deg): camera at -X of the avatar
        let v = c.update([0.0, 0.0, 0.0], FRAC_PI_2, 0.016);
        assert!(v.pos.x < -4.0 && near(v.pos.z, 0.0));
        // facing +Z (yaw 180 deg): camera at -Z
        let v = c.update([0.0, 0.0, 0.0], PI, 0.016);
        assert!(v.pos.z < -4.0);
    }

    #[test]
    fn orbit_signs_and_elevation_limit() {
        let mut c = cam();
        // +dx: look right -> the camera swings to the left of the avatar (-X when facing -Z)
        c.apply(&CamCmd::Orbit { dx: 0.5, dy: 0.0 });
        let v = c.update([0.0; 3], 0.0, 0.016);
        assert!(v.pos.x < -1.0);
        // +dy raises the camera, clamped just below straight up
        c.apply(&CamCmd::Orbit { dx: 0.0, dy: 10.0 });
        let v = c.update([0.0; 3], 0.0, 0.016);
        assert!(v.pos.y - 1.5 > 4.99 && v.pos.y - 1.5 <= 5.0);
        c.apply(&CamCmd::Orbit { dx: 0.0, dy: -10.0 });
        assert!(c.orbit().1 < -1.5);
    }

    #[test]
    fn reset_and_preferred() {
        let mut c = cam();
        c.apply(&CamCmd::Orbit { dx: 1.0, dy: 0.3 });
        c.apply(&CamCmd::SetPreferred);
        c.apply(&CamCmd::Orbit { dx: -2.0, dy: -0.5 });
        c.apply(&CamCmd::Reset);
        assert!(near(c.orbit().0, 1.0));
        let mut d = cam();
        d.apply(&CamCmd::Orbit { dx: 1.0, dy: 0.3 });
        d.apply(&CamCmd::Reset);
        assert!(near(d.orbit().0, 0.0) && near(d.distance(), 5.0));
    }

    #[test]
    fn wheel_zoom_is_smooth_and_clamped() {
        let mut c = cam();
        // one notch = ZoomSpeed/10 = 2 m, consumed over time: 3.0 m from the target afterwards
        c.apply(&CamCmd::Zoom(1.0));
        let mut d_prev = c.distance();
        for _ in 0..600 {
            c.update([0.0; 3], 0.0, 0.016);
            assert!(c.distance() <= d_prev + 1e-6, "monotone");
            d_prev = c.distance();
        }
        // the tail below 0.3 m is dropped
        assert!(c.distance() > 3.0 && c.distance() < 3.35, "{}", c.distance());
        // far out: capped at 25 m
        for _ in 0..40 {
            c.apply(&CamCmd::Zoom(-10.0));
            for _ in 0..300 {
                c.update([0.0; 3], 0.0, 0.016);
            }
        }
        assert!(near(c.distance(), MAX_DISTANCE));
        // opposite wheel direction cancels the rest
        c.apply(&CamCmd::Zoom(5.0));
        c.apply(&CamCmd::Zoom(-1.0));
        assert!(near(c.pending_zoom, -2.0));
    }

    #[test]
    fn zooming_in_to_the_limit_switches_to_first_person_and_back() {
        let mut c = cam();
        c.apply(&CamCmd::Key { key: CamKey::ZoomIn, down: true });
        for _ in 0..120 {
            c.update([0.0; 3], 0.0, 0.016);
        }
        assert!(c.is_first_person());
        assert!(!c.show_avatar());
        let v = c.update([1.0, 0.0, 2.0], 0.7, 0.016);
        assert!(near(v.pos.y, 1.5) && near(v.pos.x, 1.0) && near(v.yaw, 0.7));
        // wheel out returns to third person at the old distance
        c.apply(&CamCmd::Key { key: CamKey::ZoomIn, down: false });
        c.apply(&CamCmd::Zoom(-1.0));
        assert!(!c.is_first_person() && c.show_avatar());
    }

    #[test]
    fn zoom_to_first_person_pref_off_clamps_instead() {
        let mut c = Camera3p::new(&ControlPrefs { zoom_to_1st_person: false, ..Default::default() }, Some(Vec3::Y * 1.5));
        c.apply(&CamCmd::Key { key: CamKey::ZoomIn, down: true });
        for _ in 0..120 {
            c.update([0.0; 3], 0.0, 0.016);
        }
        assert!(!c.is_first_person() && near(c.distance(), MIN_DISTANCE));
    }

    #[test]
    fn first_person_look_and_heading_sync() {
        let mut c = Camera3p::new(&ControlPrefs { third_person: false, ..Default::default() }, Some(Vec3::Y * 1.5));
        assert!(c.is_first_person());
        c.apply(&CamCmd::Orbit { dx: 0.4, dy: 5.0 });
        let v = c.update([0.0; 3], 0.0, 0.016);
        assert!(near(v.yaw, 0.4) && near(v.pitch, -FIRST_PERSON_PITCH));
        // the look ends: the avatar is turned to the view direction, the view re-centres
        assert!(near(c.apply(&CamCmd::EndLook).unwrap(), 0.4));
        let v = c.update([0.0; 3], 0.4, 0.016);
        assert!(near(v.yaw, 0.4));
        assert_eq!(c.apply(&CamCmd::EndLook), Some(0.0));
    }

    #[test]
    fn right_drag_pitch_follows_prefs() {
        let mut c = cam();
        let e = c.orbit().1;
        c.apply(&CamCmd::Pitch { dy: 0.2 });
        assert!(near(c.orbit().1, e + 0.2));
        let mut c = Camera3p::new(&ControlPrefs { rmb_mouse_look_3rd: false, ..Default::default() }, Some(Vec3::Y * 1.5));
        c.apply(&CamCmd::Pitch { dy: 0.2 });
        assert!(near(c.orbit().1, DEFAULT_DIRECTION[1].asin()));
    }

    #[test]
    fn occlusion_keeps_the_original_final_midpoint() {
        let wall = |_: [f32; 3], to: [f32; 3]| to[0] <= 4.8001 + COLLISION_RADIUS;
        let eye = occlude(Vec3::ZERO, Vec3::X * 10.0, Vec3::X, &wall);
        assert!(eye.x > 4.8001 && eye.x < 4.81, "{eye:?}");
        assert!(!wall([0.0; 3], (eye + Vec3::X * COLLISION_RADIUS).to_array()));
    }

    #[test]
    fn walls_pull_the_camera_in() {
        let mut c = cam();
        // a wall 2 m behind the pivot (z >= -3 + 2 for an avatar at z = -3 facing -Z): nothing beyond z = pivot.z + 2 is reachable
        let wall = |_: [f32; 3], to: [f32; 3]| to[2] < -1.0;
        let v = c.update_with([0.0, 0.0, -3.0], 0.0, 0.016, &Sight::with_clear(&wall));
        assert!(v.pos.z <= -1.0 - COLLISION_RADIUS * 0.948 + 0.01, "{}", v.pos.z);
        assert!(v.pos.z > -3.0 + 1.5, "still pulled out of the avatar: {}", v.pos.z);
        // An unobstructed view returns outward by 10% of the remaining radius per call.
        let pivot = Vec3::new(0.0, 1.5, -3.0);
        let old = (v.pos - pivot).length();
        let v = c.update_with([0.0, 0.0, -3.0], 0.0, 0.016, &Sight::OPEN);
        assert!(near((v.pos - pivot).length(), old * 0.9 + 5.0 * 0.1));
    }

    #[test]
    fn numpad_rotation_is_frame_rate_independent() {
        let mut a = cam();
        let mut b = cam();
        a.apply(&CamCmd::Key { key: CamKey::RotateLeft, down: true });
        b.apply(&CamCmd::Key { key: CamKey::RotateLeft, down: true });
        for _ in 0..60 {
            a.update([0.0; 3], 0.0, 1.0 / 60.0);
        }
        for _ in 0..30 {
            b.update([0.0; 3], 0.0, 1.0 / 30.0);
        }
        assert!(near(a.orbit().0, b.orbit().0));
        assert!(near(a.orbit().0, 0.2 * 60.0)); // 0.02 * sensitivity 10 rad per frame at 60 Hz
    }

    #[test]
    fn the_look_target_follows_the_head_attractor_with_a_blend() {
        let mut c = cam();
        c.set_head(Vec3::Y * (1.7));
        let v = c.update([0.0; 3], 0.0, 1.0 / 60.0);
        // One call takes 0.2: 1.5 -> 1.54.
        assert!(near(Camera::look_at(v.pos, Vec3::ZERO).pos.y, v.pos.y));
        assert!(near(c.pivot_height, 1.5 + 0.2 * 0.2));
        for _ in 0..120 {
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
        }
        assert!(near(c.pivot_height, 1.7));
        // first person tracks (almost) at once
        let mut f = Camera3p::new(&ControlPrefs { third_person: false, ..Default::default() }, Some(Vec3::Y * 1.5));
        f.set_head(Vec3::Y * (1.7));
        assert!(near(f.update([0.0; 3], 0.0, 1.0 / 60.0).pos.y, 0.000_999_987 * 1.5 + 0.999 * 1.7));
        // never below 0.3 m
        c.set_head(Vec3::Y * (0.0));
        assert!(near(c.head, MIN_PIVOT_HEIGHT));
    }

    /// `UseNoBobCamera`: motion under 0.5 m does not start following; the latch stops below 0.1 m.
    #[test]
    fn no_bob_camera_ignores_small_head_motion() {
        let mut c = Camera3p::new(&ControlPrefs { no_bob_camera: true, ..Default::default() }, Some(Vec3::Y * 1.5));
        for i in 0..120 {
            c.set_head(Vec3::Y * (1.5 + if i % 2 == 0 { 0.015 } else { -0.015 }));
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
        }
        assert!(near(c.pivot_height, 1.5), "bobbing ignored: {}", c.pivot_height);
        // A crouch of 0.6 m starts following (0.99 * old + 0.01 * new per call).
        c.set_head(Vec3::Y * 0.9);
        c.update([0.0; 3], 0.0, 1.0 / 60.0);
        assert!(near(c.pivot_height, 1.5 - 0.006), "{}", c.pivot_height);
        // ... and keeps following until it is within 0.1 m.
        for _ in 0..1000 {
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
        }
        assert!((c.pivot_height - 0.9).abs() < 0.1 && !c.no_bob_following, "{}", c.pivot_height);
        // the default camera follows at once
        let mut d = cam();
        d.set_head(Vec3::Y * (1.515));
        d.update([0.0; 3], 0.0, 1.0 / 60.0);
        assert!(d.pivot_height > 1.5 && !near(d.pivot_height, 1.5));
    }

    #[test]
    fn no_bob_uses_squared_local_gap_and_strict_thresholds() {
        for (gap, latched, follows) in [(0.05, true, false), (0.2, false, false), (0.2, true, true), (0.6, false, true)] {
            let mut c = Camera3p::new(&ControlPrefs { no_bob_camera: true, ..Default::default() }, Some(Vec3::Y * 1.5));
            c.no_bob_following = latched;
            c.set_head(Vec3::new(100.0, 1.5 + gap, -100.0));
            c.follow_head();
            assert_eq!(c.no_bob_following, follows, "gap {gap}, initially latched {latched}");
            assert!(near(c.pivot_height, if follows { 0.99 * 1.5 + 0.01 * (1.5 + gap) } else { 1.5 }));
        }
        // Exact f32 0.1 squared equals the retail lower threshold: equality must retain the latch.
        let mut c = Camera3p::new(&ControlPrefs { no_bob_camera: true, ..Default::default() }, Some(Vec3::Y * 1.5));
        c.pivot_height = 0.0;
        c.head = 0.1;
        c.no_bob_following = true;
        assert_eq!(c.head * c.head, NO_BOB_MIN_SQUARED);
        c.follow_head();
        assert!(c.no_bob_following);
        // Equality at the upper threshold does not start following.
        c.pivot_height = 1.0;
        c.head = 1.5;
        c.no_bob_following = false;
        c.follow_head();
        assert!(!c.no_bob_following);
        assert_eq!(c.pivot_height, 1.0);
    }

    #[test]
    fn head_blends_are_per_call_not_delta_time_normalized() {
        for prefs in [
            ControlPrefs::default(),
            ControlPrefs { third_person: false, ..Default::default() },
            ControlPrefs { no_bob_camera: true, ..Default::default() },
        ] {
            let mut reference = None;
            for dt in [0.0, 1.0 / 120.0, 1.0 / 60.0, 0.5] {
                let mut c = Camera3p::new(&prefs, Some(Vec3::Y * 1.5));
                c.set_head(Vec3::Y * 2.1);
                for _ in 0..3 {
                    c.update([0.0; 3], 0.0, dt);
                }
                if let Some(height) = reference {
                    assert_eq!(c.pivot_height, height);
                } else {
                    reference = Some(c.pivot_height);
                }
            }
        }
    }

    #[test]
    fn near_far_and_lens() {
        assert_eq!(NEAR, 0.2);
        assert_eq!(far_plane(VIEW_DISTANCE), 800.0);
        assert_eq!(far_plane(0.0), NEAR + 50.0);
        let l = lens(Lens { fov: 1.0, horizontal: false, near: 0.5, far: Some(900.0) });
        assert_eq!((l.fov, l.horizontal, l.near, l.far), (FOV_HORIZONTAL, true, 0.2, Some(900.0)));
    }

    #[test]
    fn next_view_cycles_the_camera_vehicle_3_2_1() {
        let mut c = cam();
        assert_eq!(c.mode(), 3);
        for want in [2, 1, 3, 2] {
            c.apply(&CamCmd::NextView);
            assert_eq!(c.mode(), want);
        }
        // first person: only the attractor is dropped
        let mut f = Camera3p::new(&ControlPrefs { third_person: false, ..Default::default() }, Some(Vec3::Y * 1.5));
        f.apply(&CamCmd::NextView);
        assert_eq!(f.mode(), 3);
        // the pref picks the start mode, 0 maps to 1
        assert_eq!(ControlPrefs::from_xml(r#"<Value name="PreferredCameraMode" value="1"/>"#).preferred_camera_mode, 1);
        assert_eq!(ControlPrefs::from_xml(r#"<Value name="PreferredCameraMode" value="0"/>"#).preferred_camera_mode, 1);
        assert_eq!(Camera3p::new(&ControlPrefs { preferred_camera_mode: 2, ..Default::default() }, Some(Vec3::Y * 1.5)).mode(), 2);
    }

    #[test]
    fn initial_head_offset_rotates_but_animation_only_changes_height() {
        let feet = [10.0, 20.0, 30.0];
        let mut c = Camera3p::new(&ControlPrefs::default(), Some(Vec3::new(0.4, 1.5, -0.2)));
        c.update(feet, 0.0, 1.0 / 60.0);
        assert!((c.pivot - Vec3::new(10.4, 21.5, 29.8)).length() < 1e-4);
        c.set_head(Vec3::new(8.0, 1.7, 9.0));
        c.update(feet, std::f32::consts::FRAC_PI_2, 1.0 / 60.0);
        // Scene rotation is -server heading: (x, z) -> (-z, x).
        assert!((c.pivot - Vec3::new(10.2, 21.54, 30.4)).length() < 1e-4);
    }

    #[test]
    fn damped_mode_arrives_at_the_rigid_position_without_overshoot() {
        let mut rigid = cam();
        let target = rigid.update([0.0; 3], 0.0, 0.016).pos;
        let mut c = cam();
        c.update([0.0; 3], 0.0, 0.016);
        c.apply(&CamCmd::NextView); // 2
        // the avatar steps 10 m: the camera trails and then settles on the rigid spot
        let first = c.update([10.0, 0.0, 0.0], 0.0, 1.0 / 60.0).pos;
        let rigid_now = rigid.update([10.0, 0.0, 0.0], 0.0, 1.0 / 60.0).pos;
        assert!((first - rigid_now).length() > 5.0, "lags behind the avatar");
        let mut dist = (first - rigid_now).length();
        for _ in 0..300 {
            let d = (c.update([10.0, 0.0, 0.0], 0.0, 1.0 / 60.0).pos - rigid_now).length();
            assert!(d <= dist + 1e-3, "monotone approach");
            dist = d;
        }
        assert!(dist < 0.05, "{dist}");
        assert!(near(target.y, rigid_now.y));
    }

    /// A camera in mode 1 at its own spot (the way `n3Camera_t` swaps to `CameraVehicle_t`).
    fn chase() -> Camera3p {
        let mut c = cam();
        c.update([0.0; 3], 0.0, 0.016);
        c.apply(&CamCmd::NextView);
        c.apply(&CamCmd::NextView);
        assert_eq!(c.mode(), 1);
        c
    }

    #[test]
    fn chase_mode_keeps_its_distance_from_the_look_target() {
        // the swapped-in vehicle has the constructor's chase distance, 5 m (`+0x198`, `_DAT_1003d2e8`)
        let mut c = chase();
        assert_eq!(c.vehicle.dist, CHASE_DEFAULT_DISTANCE);
        // the avatar walks off: the camera trails it at that distance, above the target, watching it
        let mut v = c.update([0.0, 0.0, -8.0], 0.0, 1.0 / 60.0);
        for _ in 0..600 {
            v = c.update([0.0, 0.0, -8.0], 0.0, 1.0 / 60.0);
        }
        let pivot = Vec3::new(0.0, 1.5, -8.0);
        assert!(((v.pos - pivot).length() - CHASE_DEFAULT_DISTANCE).abs() < 0.11, "{}", (v.pos - pivot).length()); // `SteeringArrive` stops within 0.1 m (d2 < 0.01)
        assert!((v.forward() - (pivot - v.pos).normalize()).length() < 1e-3);
        // a target inside 0.9 m is not pushed out any more (`Update` only runs on input): the distance stays what it was
        let mut d = chase();
        for _ in 0..600 {
            d.update([d.vehicle.pos.x, -0.4, d.vehicle.pos.z + 0.2], 0.0, 1.0 / 60.0);
        }
        assert!(d.vehicle.pos.is_finite());
    }

    #[test]
    fn the_chase_distance_follows_orbit_and_zoom_input() {
        let mut c = chase();
        for _ in 0..120 {
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
        }
        let before = c.vehicle.pos - c.pivot;
        // at rest an orbit swings the camera about the target and stores the distance (`Update` + `ForcedUpdate`)
        c.apply(&CamCmd::Orbit { dx: 0.5, dy: 0.0 });
        let after = c.vehicle.pos - c.pivot;
        assert!(near(after.length(), before.length()) && near(c.vehicle.dist, before.length()));
        assert!(after.x < before.x - 1.0, "+dx swings the camera to -X like the rigid modes: {before:?} -> {after:?}");
        assert!(near(after.y, before.y));
        // wheel zoom changes the distance it keeps
        c.apply(&CamCmd::Zoom(1.0));
        for _ in 0..240 {
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
        }
        assert!(c.vehicle.dist < before.length() - 1.0, "{}", c.vehicle.dist);
    }

    #[test]
    fn the_vehicle_runs_in_substeps_of_at_most_0_05_s() {
        let mut v = Vehicle::default();
        let mut hs = Vec::new();
        v.run(0.12, |_, h| hs.push(h));
        assert_eq!(hs.len(), 3);
        assert!(near(hs[0], 0.05) && near(hs[1], 0.05) && near(hs[2], 0.02));
        v.run(5.0, |_, _| panic!("frames over 4 s are not run (`_DAT_10012804`)"));
    }

    #[test]
    fn a_wanted_spot_behind_the_look_target_is_swapped_for_one_beside_it() {
        // `SteeringCamArrive`: the look target is 2 m ahead (-Z), the wanted spot 6 m ahead in the same direction
        let look = Vec3::new(0.0, 0.0, -2.0);
        let mut v = Vehicle::default();
        v.cam_arrive_step(Vec3::new(0.0, 0.0, -6.0), look, 0.016);
        // heads for (1, 0, -2): half the distance to the look target to the side (cross y = 0: the + side)
        assert!(v.vel.x > 0.0 && v.vel.z < 0.0 && near(v.vel.x / v.vel.z, -0.5), "{:?}", v.vel);
        // a wanted spot nearer than the look target goes straight there
        let mut w = Vehicle::default();
        w.cam_arrive_step(Vec3::new(0.0, 0.0, -1.0), look, 0.016);
        assert!(near(w.vel.x, 0.0) && w.vel.z < 0.0);
        // off to the side by more than the sine limit (0.4): straight there as well
        let mut s = Vehicle::default();
        s.cam_arrive_step(Vec3::new(6.0, 0.0, -6.0), look, 0.016);
        assert!(near(s.vel.x, -s.vel.z), "{:?}", s.vel);
    }

    #[test]
    fn a_hitch_in_the_frame_time_halts_the_camera() {
        let mut v = Vehicle::default();
        let far = Vec3::new(0.0, 0.0, -10.0);
        for _ in 0..10 {
            v.cam_arrive_step(far, far, 0.016);
        }
        assert!(v.vel.length() > 1.0);
        // a substep more than 10 times the running average stops it dead
        let p = v.pos;
        v.cam_arrive_step(far, far, 0.2);
        assert_eq!((v.vel, v.pos), (Vec3::ZERO, p));
        // the average only moves half way: the next normal substep goes on
        v.cam_arrive_step(far, far, 0.016);
        assert!(v.vel.length() > 0.0);
    }

    /// A wall across z = 2.5 for |x| < 1 (any height): segments through it are blocked.
    fn wall(a: [f32; 3], b: [f32; 3]) -> bool {
        let (lo, hi) = (a[2].min(b[2]), a[2].max(b[2]));
        if lo >= 2.5 || hi <= 2.5 {
            return true;
        }
        let t = (2.5 - a[2]) / (b[2] - a[2]);
        (a[0] + (b[0] - a[0]) * t).abs() >= 1.0
    }

    #[test]
    fn the_sensors_steer_round_an_obstacle_to_the_nearest_view() {
        // camera 5 m behind the look target (-Z is ahead), a wall in between: right and left are blocked at 0.5 and 2 m,
        // at 8 m the right-hand probe sees the target past the wall's edge
        let look = Vec3::ZERO;
        let mut v = Vehicle { pos: Vec3::new(0.0, 0.0, 5.0), ..Vehicle::default() };
        v.update_sensors(look, &wall);
        assert!(!v.sees && !v.blind);
        assert_eq!(v.steer, Vec3::X);
        // CalcSteering adds the unit steer direction to the wanted spot while the target is out of view
        let want = v.calc_steering(look, &Sight::OPEN);
        assert!((want - Vec3::new(1.0, 0.0, 5.0)).length() < 1e-4, "{want:?}");
        // in the clear the sensor reports a view and no steer applies
        let mut c = Vehicle { pos: Vec3::new(3.0, 0.0, 5.0), ..Vehicle::default() };
        c.update_sensors(look, &wall);
        assert!(c.sees && !c.blind && c.steer == Vec3::ZERO);
        // nothing sees it: blind
        let mut b = Vehicle { pos: Vec3::new(0.0, 0.0, 5.0), ..Vehicle::default() };
        b.update_sensors(look, &|_, _| false);
        assert!(b.blind && !b.sees && b.steer == Vec3::ZERO);
    }

    #[test]
    fn the_left_probe_wins_when_only_it_sees_the_target() {
        // the wall reaches 9 m to the right: only the left-hand probes see past it
        let wide = |a: [f32; 3], b: [f32; 3]| {
            let (lo, hi) = (a[2].min(b[2]), a[2].max(b[2]));
            if lo >= 2.5 || hi <= 2.5 {
                return true;
            }
            let x = a[0] + (b[0] - a[0]) * (2.5 - a[2]) / (b[2] - a[2]);
            !(-1.0..9.0).contains(&x)
        };
        let mut v = Vehicle { pos: Vec3::new(0.0, 0.0, 5.0), ..Vehicle::default() };
        v.update_sensors(Vec3::ZERO, &wide);
        assert_eq!(v.steer, -Vec3::X);
    }

    #[test]
    fn in_the_clear_the_camera_is_lifted_off_the_ground_and_below_the_target() {
        let look = Vec3::new(0.0, 1.5, 0.0);
        let v = Vehicle { pos: Vec3::new(0.0, 3.0, 5.0), sees: true, ..Vehicle::default() };
        let plain = v.calc_steering(look, &Sight::OPEN);
        let near_ground = Sight { ground: &|_| Some(2.9), ..Sight::OPEN };
        assert!(near(v.calc_steering(look, &near_ground).y, plain.y + CHASE_LIFT));
        let high = Sight { ground: &|_| Some(2.0), ..Sight::OPEN };
        assert!(near(v.calc_steering(look, &high).y, plain.y));
        // below the look target: lifted as well, both lifts add up
        let low = Vehicle { pos: Vec3::new(0.0, 1.0, 5.0), sees: true, ..Vehicle::default() };
        let base = low.calc_steering(look, &Sight::OPEN).y;
        assert!(near(low.calc_steering(look, &Sight { ground: &|_| Some(0.9), ..Sight::OPEN }).y, base + CHASE_LIFT));
    }

    #[test]
    fn a_camera_blind_for_1_5_s_is_cut_to_the_far_side_of_the_target() {
        let mut c = chase();
        let blocked = Sight::with_clear(&|_, _| false);
        let pivot = Vec3::new(0.0, 1.5, 0.0);
        let mut last = c.vehicle.pos;
        let mut cut = None;
        for frame in 0..120 {
            let v = c.update_with([0.0; 3], 0.0, 1.0 / 60.0, &blocked);
            if (v.pos - last).length() > 1.0 && cut.is_none() {
                cut = Some(frame);
            }
            last = v.pos;
        }
        let frame = cut.expect("the camera was cut");
        assert!((88..=93).contains(&frame), "cut after {frame} frames");
        // every try that does not see the target halves the distance and swings on from the previous spot (the loop reuses
        // `pos'`), so with nothing visible it ends 0.625 m from the target, above it
        assert!(c.vehicle.pos.y > pivot.y && near((c.vehicle.pos - pivot).length(), 0.625), "{:?}", c.vehicle.pos);
        assert!(c.vehicle.dist <= CUT_MIN_DISTANCE, "{}", c.vehicle.dist);
    }

    #[test]
    fn the_cut_picks_the_side_the_character_faces_away_from() {
        let look = Vec3::new(0.0, 1.5, 0.0);
        let mut a = Vehicle { pos: Vec3::new(2.0, 3.0, 3.0), ..Vehicle::default() };
        a.cut_on_axis(look, Vec3::new(0.0, 0.0, -1.0), &|_, _| true);
        let mut b = Vehicle { pos: Vec3::new(-2.0, 3.0, 3.0), ..Vehicle::default() };
        b.cut_on_axis(look, Vec3::new(0.0, 0.0, -1.0), &|_, _| true);
        // with a free line the first try stands: distance 5 from the target, beyond it, above it, mirrored left/right
        for v in [&a, &b] {
            assert!(near((v.pos - look).length(), 5.0) && v.pos.z < look.z && v.pos.y > look.y, "{:?}", v.pos);
        }
        assert!(a.pos.x * b.pos.x < 0.0 || near(a.pos.x + b.pos.x, 0.0), "{:?} {:?}", a.pos, b.pos);
    }

    #[test]
    fn previous_view_selects_an_attractor_that_steers_mode_one() {
        use ao_formats::playfield::{CameraAttractor, CameraViews};
        let a = CameraAttractor { pos: [10.0, 5.0, 0.0], rot: [0.0, 0.0, 0.0, 1.0], target: [10.0, 1.5, 1.0], range: 1.0 };
        let mut c = cam();
        c.set_views(Views::new(CameraViews::new(vec![vec![a]], None, vec![]), Box::new(|_| 0)));
        // mode 3 ignores attractors
        c.apply(&CamCmd::PrevView);
        let v = c.update([10.0, 0.0, -1.0], 0.0, 0.016);
        assert!(c.views.as_ref().unwrap().selected().is_some());
        assert!((v.pos - Vec3::new(10.0, 0.0, -1.0)).length() > 4.0);
        // mode 1: the camera flies to the attractor (scene z = -server z) and stops there
        c.apply(&CamCmd::NextView); // 2
        c.apply(&CamCmd::NextView); // 1
        c.apply(&CamCmd::PrevView);
        let mut v = c.update([10.0, 0.0, -1.0], 0.0, 1.0 / 60.0);
        for _ in 0..600 {
            v = c.update([10.0, 0.0, -1.0], 0.0, 1.0 / 60.0);
        }
        assert!((v.pos - Vec3::new(10.0, 5.0, 0.0)).length() < 0.2, "{:?}", v.pos);
        // Ctrl+F8 drops it again
        c.apply(&CamCmd::NextView);
        assert!(c.views.as_ref().unwrap().selected().is_none());
    }

    #[test]
    fn fov_is_ninety_degrees_horizontal() {
        let l = lens(Lens::default());
        assert!(near(l.vertical_fov(1.0), FRAC_PI_2));
        assert!(near(l.vertical_fov(16.0 / 9.0).to_degrees(), 58.716));
    }
}
