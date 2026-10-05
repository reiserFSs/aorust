//! The zone camera of the own character: `n3Camera_t` with the default `CameraVehicleFixedThird_t` (mode 3) and
//! `CameraVehicleFirstPerson_t` (mode 0) from N3.dll. Pure and headless: avatar position/heading in, an
//! `ao_render::Camera` out. Constants, sign conventions and addresses: docs/zone/camera.md.
//!
//! The scene frame is the renderer's (Y up, forward = (sin yaw, 0, -cos yaw)); `avatar_yaw` is the heading of the
//! avatar in that convention (the integration layer converts the server heading).
#![allow(dead_code)] // wired into play/flow.rs by the integration owner

use super::controls::{CamCmd, CamKey, ControlPrefs};
use ao_render::{Camera, Vec3};
use std::f32::consts::{FRAC_PI_2, PI};

/// The world camera is created with `SetViewPlaneWindow(π/2, aspect)` (`n3Camera_t` ctor, N3 @0x10021a76 / `FUN_1002107a`
/// @0x1002107a, constant `_DAT_1003ccf8`): 90° *horizontal*.
pub const FOV_HORIZONTAL: f32 = FRAC_PI_2;

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
/// Height of the look target above the feet when the avatar did not supply one. [UNRESOLVED] The client uses the world
/// position of the `Attractor31_camera` attractor, falling back to `Attractor01_head` (`FUN_10020af1` @N3 0x10020af1);
/// the head attractor's bind-pose height is not extracted yet (solitus male: head top 1.62 m).
pub const DEFAULT_PIVOT_HEIGHT: f32 = 1.5;

/// Third-person (mode 3) and first-person (mode 0) camera of the own character.
pub struct Camera3p {
    prefs: ControlPrefs,
    first_person: bool,
    pivot_height: f32,
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
}

impl Camera3p {
    /// `pivot_height`: height of the look target over the feet ([`DEFAULT_PIVOT_HEIGHT`]).
    pub fn new(prefs: &ControlPrefs, pivot_height: f32) -> Self {
        let elev = DEFAULT_DIRECTION[1].asin();
        Self {
            prefs: prefs.clone(),
            first_person: !prefs.third_person,
            pivot_height,
            yaw_off: 0.0,
            elev,
            dist: DEFAULT_DISTANCE,
            preferred: (0.0, elev, DEFAULT_DISTANCE),
            pending_zoom: 0.0,
            keys: [false; 6],
            fp_yaw: 0.0,
            fp_pitch: 0.0,
        }
    }

    pub fn set_pivot_height(&mut self, h: f32) {
        self.pivot_height = h;
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
            // scripted camera attractors of the playfield: not implemented, see docs/zone/camera.md
            CamCmd::NextView | CamCmd::PrevView => {}
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

    /// One frame without occlusion testing.
    pub fn update(&mut self, avatar_pos: [f32; 3], avatar_yaw: f32, dt: f32) -> Camera {
        self.update_with(avatar_pos, avatar_yaw, dt, &|_, _| true)
    }

    /// One frame. `clear(from, to)` answers whether the segment is free of terrain/statels (scene frame).
    pub fn update_with(&mut self, avatar_pos: [f32; 3], avatar_yaw: f32, dt: f32, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) -> Camera {
        self.step_keys_and_zoom(dt);
        let pivot = Vec3::from(avatar_pos) + Vec3::Y * self.pivot_height;
        if self.first_person {
            return Camera { pos: pivot, yaw: avatar_yaw + self.fp_yaw, pitch: -self.fp_pitch, roll: 0.0 };
        }
        let h = avatar_yaw + self.yaw_off;
        let fwd = Vec3::new(h.sin(), 0.0, -h.cos());
        let dir = -fwd * self.elev.cos() + Vec3::Y * self.elev.sin();
        let want = pivot + dir * self.dist;
        let eye = occlude(pivot, want, dir, clear);
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
    fn fov_is_ninety_degrees_horizontal() {
        assert!(near(vertical_fov(1.0), FRAC_PI_2));
        assert!(near(vertical_fov(16.0 / 9.0).to_degrees(), 58.716));
    }
}
