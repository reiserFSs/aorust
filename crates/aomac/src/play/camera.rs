//! The zone camera of the own character: `n3Camera_t` with the default `CameraVehicleFixedThird_t` (mode 3) and
//! `CameraVehicleFirstPerson_t` (mode 0) from N3.dll. Pure and headless: avatar position/heading in, an
//! `ao_render::Camera` out. Constants, sign conventions and addresses: docs/zone/camera.md.
//!
//! The scene frame is the renderer's (Y up, forward = (sin yaw, 0, -cos yaw)); `avatar_yaw` is the heading of the
//! avatar in that convention (the integration layer converts the server heading).

#![allow(dead_code)] // documented constants and accessors for tests / the live harness

use super::camera_views::Views;
use super::controls::{CamCmd, CamKey, ControlPrefs};
use ao_render::{Camera, Vec3};
use ao_scene::Lens;
use std::f32::consts::{FRAC_PI_2, PI};

/// The world camera is created with `SetViewPlaneWindow(π/2, aspect)` (`n3Camera_t` ctor, N3 @0x10021a76 / `FUN_1002107a`
/// @0x1002107a, constant `_DAT_1003ccf8`): 90° *horizontal*.
pub const FOV_HORIZONTAL: f32 = FRAC_PI_2;
/// Near clip plane: the third `VisualCamera_t` constructor argument of `n3EngineClient_t::CreateCamera` (N3 @0x10007842, `_DAT_1003ce50`).
pub const NEAR: f32 = 0.2;
/// Far clip plane at creation (`_DAT_1003ce54`); the `ViewDistance` callback (`FUN_1001fc91`, registered to fire at once)
/// replaces it immediately, see [`far_plane`].
pub const FAR_AT_CREATE: f32 = 200.0;
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

/// Vertical field of view for a window aspect ratio (width / height) given [`FOV_HORIZONTAL`].
pub fn vertical_fov(aspect: f32) -> f32 {
    2.0 * ((FOV_HORIZONTAL * 0.5).tan() / aspect).atan()
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
/// Per-frame blend of the look target height towards the animated head attractor (`FUN_10020bdb`): third person keeps 0.8 of
/// the old value (`_DAT_1003d9c4`) and takes 0.2 (`_DAT_1003ce50`); first person takes 0.999 and keeps 0.0001.
const HEAD_BLEND_3RD: f32 = 0.2;
const HEAD_BLEND_1ST: f32 = 0.999;

/// `Vehicle_t` steering of the camera dynel (Vehicle.dll `SteeringArrive` @0x1000ab28, integrator `FUN_1000e3d3`). Mass,
/// top speed and brake distance are `FUN_1001faa1` / `UpdateMotionConstraints` @N3 0x1001e602 for an avatar that runs
/// (top speed `min(16, 6 * avatar speed)`, force `mass * v / 0.3`, brake `0.3 * v`).
const VEHICLE_MASS: f32 = 20.0;
const VEHICLE_MAX_SPEED: f32 = 16.0;
const VEHICLE_BRAKE: f32 = 0.3 * VEHICLE_MAX_SPEED;
const VEHICLE_MAX_FORCE: f32 = VEHICLE_MASS * VEHICLE_MAX_SPEED / 0.3;
/// Integration substep. [GUESS] the client limits it by `Vehicle_t +0x104` (not read).
const VEHICLE_STEP: f32 = 1.0 / 60.0;
/// Arrival radius of the camera vehicle (`_DAT_1003d618`, `SteeringCamArrive`) and of an attractor (`_DAT_1003d61c`).
const ARRIVE_RADIUS: f32 = 0.01;
const ATTRACTOR_RADIUS: f32 = 0.1;
/// Mode 1 pushes the camera out to this distance from the look target (`CameraVehicle_t::Update` @0x1001e54f, `_DAT_1003e04c`)
/// and lifts it by `CHASE_LIFT` per frame while it is below the target (`_DAT_1003d9d0`).
const CHASE_MIN_DISTANCE: f32 = 0.9;
const CHASE_LIFT: f32 = 0.4;

/// The camera dynel as a steered vehicle (modes 1 and 2); mode 3 snaps it every frame.
#[derive(Clone, Copy, Default)]
struct Vehicle {
    pos: Vec3,
    vel: Vec3,
}

impl Vehicle {
    /// `SteeringArrive(target, radius)` + the integration over `dt`: desired velocity = towards the target at
    /// `min(max_speed, distance / brake * max_speed)`, force = `(desired - vel) * mass * 4` limited to `max_force`.
    fn arrive(&mut self, target: Vec3, radius: f32, dt: f32) {
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(VEHICLE_STEP);
            left -= h;
            let to = target - self.pos;
            let d2 = to.length_squared();
            if d2 < radius * radius || d2 < 0.01 {
                self.vel = Vec3::ZERO; // SteeringHalt
                continue;
            }
            let d = d2.sqrt();
            let desired = to / d * (d / VEHICLE_BRAKE * VEHICLE_MAX_SPEED).min(VEHICLE_MAX_SPEED);
            let force = ((desired - self.vel) * (VEHICLE_MASS * 4.0)).clamp_length_max(VEHICLE_MAX_FORCE);
            self.vel = (self.vel + force * h / VEHICLE_MASS).clamp_length_max(VEHICLE_MAX_SPEED);
            self.pos += self.vel * h;
        }
    }

    fn snap(&mut self, pos: Vec3) {
        self.pos = pos;
        self.vel = Vec3::ZERO;
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
    views: Option<Views>,
    /// Shift+F8 was pressed; handled in the next frame (it needs the character's position).
    prev_view: bool,
}

impl Camera3p {
    /// `head_height`: height of the head attractor over the feet ([`MIN_PIVOT_HEIGHT`] without one).
    pub fn new(prefs: &ControlPrefs, head_height: f32) -> Self {
        let elev = DEFAULT_DIRECTION[1].asin();
        let head_height = head_height.max(MIN_PIVOT_HEIGHT);
        Self {
            prefs: prefs.clone(),
            first_person: !prefs.third_person,
            mode: prefs.preferred_camera_mode.clamp(1, 3),
            pivot_height: head_height,
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
            views: None,
            prev_view: false,
        }
    }

    /// The animated head attractor height (feet-relative, body scale applied) the look target follows each frame.
    pub fn set_head(&mut self, h: f32) {
        self.head = h.max(MIN_PIVOT_HEIGHT);
    }

    /// The playfield's scripted views (`n3Zone_t::GetCameraAttractorList`).
    pub fn set_views(&mut self, v: Views) {
        self.views = Some(v);
    }

    pub fn views(&self) -> Option<&Views> {
        self.views.as_ref()
    }

    /// `PreferredCameraMode` in use (1, 2 or 3).
    pub fn mode(&self) -> u8 {
        self.mode
    }

    pub fn is_first_person(&self) -> bool {
        self.first_person
    }

    /// Distance look target - camera (third person).
    pub fn distance(&self) -> f32 {
        self.dist
    }

    /// Camera heading relative to the avatar and elevation (third person), radians.
    pub fn orbit(&self) -> (f32, f32) {
        (self.yaw_off, self.elev)
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
        }
    }

    fn rotate(&mut self, dx: f32, dy: f32) {
        if self.first_person {
            self.fp_yaw += dx;
            self.fp_pitch = (self.fp_pitch + dy).clamp(-FIRST_PERSON_PITCH, FIRST_PERSON_PITCH);
        } else {
            self.yaw_off += dx;
            self.elev = (self.elev + dy).clamp(-MAX_ELEVATION_SIN.asin(), MAX_ELEVATION_SIN.asin());
        }
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
        self.dist -= metres;
        if self.dist <= FIRST_PERSON_BELOW && metres > 0.0 && self.prefs.zoom_to_1st_person {
            self.first_person = true;
            self.pending_zoom = 0.0;
            self.dist = self.dist.max(MIN_DISTANCE);
            return;
        }
        self.dist = self.dist.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    fn step_keys_and_zoom(&mut self, dt: f32) {
        let k = |c: CamKey| self.keys[c as usize];
        let turn = KEY_ROTATE * self.prefs.mouse_turn_sensitivity * dt * 60.0;
        let dx = if k(CamKey::RotateLeft) { turn } else { 0.0 } - if k(CamKey::RotateRight) { turn } else { 0.0 };
        let dy = if k(CamKey::RotateDown) { turn } else { 0.0 } - if k(CamKey::RotateUp) { turn } else { 0.0 };
        let zoom = (if k(CamKey::ZoomIn) { 1.0 } else { 0.0 } - if k(CamKey::ZoomOut) { 1.0 } else { 0.0 }) * dt * self.prefs.zoom_speed;
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
    }

    /// `FUN_10020bdb`: the look target height follows the animated head attractor, blended per frame (60 Hz equivalent).
    fn follow_head(&mut self, dt: f32) {
        let keep = if self.first_person { 1.0 - HEAD_BLEND_1ST } else { 1.0 - HEAD_BLEND_3RD };
        let a = 1.0 - keep.powf(dt * 60.0);
        self.pivot_height = (self.pivot_height + (self.head - self.pivot_height) * a).max(MIN_PIVOT_HEIGHT);
    }

    /// One frame without occlusion testing.
    pub fn update(&mut self, avatar_pos: [f32; 3], avatar_yaw: f32, dt: f32) -> Camera {
        self.update_with(avatar_pos, avatar_yaw, dt, &|_, _| true)
    }

    /// One frame. `clear(from, to)` answers whether the segment is free of terrain/statels (scene frame).
    pub fn update_with(&mut self, avatar_pos: [f32; 3], avatar_yaw: f32, dt: f32, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) -> Camera {
        self.step_keys_and_zoom(dt);
        self.follow_head(dt);
        let feet = Vec3::from(avatar_pos);
        let pivot = feet + Vec3::Y * self.pivot_height;
        if self.first_person {
            return Camera { pos: pivot, yaw: avatar_yaw + self.fp_yaw, pitch: -self.fp_pitch, roll: 0.0 };
        }
        // scripted views: ranked every 10th frame and Shift+F8 steps them
        let guide = self.views.as_ref().and_then(Views::selected).map_or(self.vehicle.pos, |v| v.pos);
        if let Some(v) = &mut self.views {
            v.tick(dt, feet, guide, clear);
            if std::mem::take(&mut self.prev_view) {
                v.prev(feet, guide, clear);
            }
        }
        let h = avatar_yaw + self.yaw_off;
        let fwd = Vec3::new(h.sin(), 0.0, -h.cos());
        let dir = -fwd * self.elev.cos() + Vec3::Y * self.elev.sin();
        let want = pivot + dir * self.dist;
        let optimal = occlude(pivot, want, dir, clear);
        let eye = match self.mode {
            // CameraVehicleFixedThird_t(rigid): `DecideSnap` places the camera on the optimal position every frame
            3 => {
                self.vehicle.snap(optimal);
                optimal
            }
            // CameraVehicleFixedThird_t(damped): `SteeringCamArrive(optimal, 0.01)`
            2 => {
                self.vehicle.arrive(optimal, ARRIVE_RADIUS, dt);
                self.vehicle.pos
            }
            // CameraVehicle_t (`CalcSteering` @N3 0x1001e797)
            _ => {
                match self.views.as_ref().and_then(Views::selected) {
                    Some(v) => self.vehicle.arrive(v.pos, ATTRACTOR_RADIUS, dt),
                    None => {
                        let off = pivot - self.vehicle.pos;
                        let len = off.length().max(1e-4);
                        let mut desired = pivot - off / len * len.max(CHASE_MIN_DISTANCE);
                        if self.vehicle.pos.y < pivot.y {
                            desired.y += CHASE_LIFT;
                        }
                        self.vehicle.arrive(desired, ARRIVE_RADIUS, dt);
                    }
                }
                self.vehicle.pos
            }
        };
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
    for _ in 0..20 {
        let mid = (lo + hi) * 0.5;
        if free(pivot + (want - pivot) * mid) {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= OCCLUSION_EPS {
            break;
        }
    }
    pivot + (want - pivot) * lo
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera3p {
        Camera3p::new(&ControlPrefs::default(), 1.5)
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
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
        let mut c = Camera3p::new(&ControlPrefs { zoom_to_1st_person: false, ..Default::default() }, 1.5);
        c.apply(&CamCmd::Key { key: CamKey::ZoomIn, down: true });
        for _ in 0..120 {
            c.update([0.0; 3], 0.0, 0.016);
        }
        assert!(!c.is_first_person() && near(c.distance(), MIN_DISTANCE));
    }

    #[test]
    fn first_person_look_and_heading_sync() {
        let mut c = Camera3p::new(&ControlPrefs { third_person: false, ..Default::default() }, 1.5);
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
        let mut c = Camera3p::new(&ControlPrefs { rmb_mouse_look_3rd: false, ..Default::default() }, 1.5);
        c.apply(&CamCmd::Pitch { dy: 0.2 });
        assert!(near(c.orbit().1, DEFAULT_DIRECTION[1].asin()));
    }

    #[test]
    fn walls_pull_the_camera_in() {
        let mut c = cam();
        // a wall 2 m behind the pivot (z >= -3 + 2 for an avatar at z = -3 facing -Z): nothing beyond z = pivot.z + 2 is reachable
        let wall = |_: [f32; 3], to: [f32; 3]| to[2] < -1.0;
        let v = c.update_with([0.0, 0.0, -3.0], 0.0, 0.016, &wall);
        assert!(v.pos.z <= -1.0 - COLLISION_RADIUS * 0.948 + 0.01, "{}", v.pos.z);
        assert!(v.pos.z > -3.0 + 1.5, "still pulled out of the avatar: {}", v.pos.z);
        // an unobstructed view keeps the full distance
        let v = c.update_with([0.0, 0.0, -3.0], 0.0, 0.016, &|_, _| true);
        assert!(near((v.pos - Vec3::new(0.0, 1.5, -3.0)).length(), 5.0));
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
        c.set_head(1.7);
        let v = c.update([0.0; 3], 0.0, 1.0 / 60.0);
        // one 60 Hz frame of 0.2: 1.5 -> 1.54
        assert!(near(Camera::look_at(v.pos, Vec3::ZERO).pos.y, v.pos.y));
        assert!(near(c.pivot_height, 1.5 + 0.2 * 0.2));
        for _ in 0..120 {
            c.update([0.0; 3], 0.0, 1.0 / 60.0);
        }
        assert!(near(c.pivot_height, 1.7));
        // first person tracks (almost) at once
        let mut f = Camera3p::new(&ControlPrefs { third_person: false, ..Default::default() }, 1.5);
        f.set_head(1.7);
        assert!(near(f.update([0.0; 3], 0.0, 1.0 / 60.0).pos.y, 1.7));
        // never below 0.3 m
        c.set_head(0.0);
        assert!(near(c.head, MIN_PIVOT_HEIGHT));
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
        let mut f = Camera3p::new(&ControlPrefs { third_person: false, ..Default::default() }, 1.5);
        f.apply(&CamCmd::NextView);
        assert_eq!(f.mode(), 3);
        // the pref picks the start mode, 0 maps to 1
        assert_eq!(ControlPrefs::from_xml(r#"<Value name="PreferredCameraMode" value="1"/>"#).preferred_camera_mode, 1);
        assert_eq!(ControlPrefs::from_xml(r#"<Value name="PreferredCameraMode" value="0"/>"#).preferred_camera_mode, 1);
        assert_eq!(Camera3p::new(&ControlPrefs { preferred_camera_mode: 2, ..Default::default() }, 1.5).mode(), 2);
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

    #[test]
    fn chase_mode_stays_put_and_backs_off_a_close_target() {
        let mut c = cam();
        let p0 = c.update([0.0; 3], 0.0, 0.016).pos;
        c.apply(&CamCmd::NextView);
        c.apply(&CamCmd::NextView); // mode 1
        assert_eq!(c.mode(), 1);
        // the avatar walks off: the camera is above the target already and does not follow, but keeps watching it
        let v = c.update([0.0, 0.0, -8.0], 0.0, 1.0 / 60.0);
        assert!((v.pos - p0).length() < 0.5);
        assert!((v.forward() - (Vec3::new(0.0, 1.5, -8.0) - v.pos).normalize()).length() < 1e-3);
        // a target inside 0.9 m is pushed out to 0.9 m
        let mut d = cam();
        d.update([0.0; 3], 0.0, 0.016);
        d.apply(&CamCmd::NextView);
        d.apply(&CamCmd::NextView);
        for _ in 0..600 {
            d.update([d.vehicle.pos.x, -0.4, d.vehicle.pos.z + 0.2], 0.0, 1.0 / 60.0);
        }
        assert!(d.vehicle.pos.is_finite());
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
        assert!(c.views().unwrap().selected().is_some());
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
        assert!(c.views().unwrap().selected().is_none());
    }

    #[test]
    fn fov_is_ninety_degrees_horizontal() {
        assert!(near(vertical_fov(1.0), FRAC_PI_2));
        assert!(near(vertical_fov(16.0 / 9.0).to_degrees(), 58.716));
    }
}
