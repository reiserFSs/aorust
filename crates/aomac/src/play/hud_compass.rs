//! `CompassWindow_c` (GUI.dll `FUN_1006d433`, window `compass_window`, style 3, flags 0xd3c): a 137 x 22 window holding a
//! `CompassView_c` (`FUN_100677b3`) whose surfaces are the strip `GFX_GUI_COMPASS_SCALE` (id 0x8f, 256 x 14, tiled), the
//! frame `GFX_GUI_COMPASS_FRAME` (0x8e) and, while a waypoint of the current playfield is set, `GFX_GUI_COMPASS_WAYPOINT` (0x90).
//! Scrolling and marker maths: `FUN_100670f8` (the view's per-frame update). Evidence: docs/gui.md §10 "Compass".

use super::zone::Zone;
use ao_gui::view::CanvasItem;
use ao_gui::{Gui, GfxId, WindowId, WindowSize};
use std::f32::consts::{FRAC_PI_2, TAU};

/// Surface ids passed to `ViewSurface_c::Load` by `FUN_100677b3` / `FUN_100670f8`.
const FRAME_GFX: u32 = 0x8e;
const STRIP_GFX: u32 = 0x8f;
const WAYPOINT_GFX: u32 = 0x90;
/// `GUIConfig_c::GetAlphaValue(layer 2)` applied with `View::SetAlpha` (the same 0.85 as the buttons).
const ALPHA: f32 = 0.85;
/// `ColorID_e` DEFAULT (`ViewSurface_c::SetColor(GetColor(0x1000000))` on the strip and the frame).
const DEFAULT: u32 = 0x1000000;
/// `CompassWindow_c` position factor: `floor(screenW * (double)0.73f - frameW / 2)` (`_PTR_101b45b0` = 0x3fe75c2900000000), y = 5
/// (`_DAT_101a8b98`).
const X_FACTOR: f32 = 0.73;
const Y: i32 = 5;

/// The window's frame size, inclusive extents (`Rect(1800,5,1936,26)` of the new-character template): 137 x 22 px.
const W: f32 = 137.0;
const H: f32 = 22.0;

/// `Gamecode`'s waypoint (set through GlobalSignals +0x158, GUI `FUN_1006749b`): the playfield it belongs to, and its position in
/// server coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Waypoint {
    pub playfield: u32,
    pub pos: [f32; 3],
}

pub(super) struct Compass {
    pub(super) window: WindowId,
    waypoint: Option<Waypoint>,
    /// Last painted `(heading, bearing)` so a still character repaints nothing.
    shown: Option<(f32, Option<f32>)>,
}

/// `FUN_100670f8`: `atan2(z, x)` of the rotated forward vector (0,0,1), made non-negative. A character with server heading
/// `yaw` (quaternion about +Y; it walks along `(sin yaw, cos yaw)` in x/z, docs/zone/avatar.md §1) has forward `(sin, cos)`.
fn heading_angle(yaw: f32) -> f32 {
    let mut a = yaw.cos().atan2(yaw.sin());
    while a < 0.0 {
        a += TAU;
    }
    a
}

/// Where the strip's source window starts: the texture is `tex` pixels wide (`GetTextureSize` returns the inclusive extent, 255),
/// the window shows `W` of them centred on `255 * frac` (`MoveSrcTo(255 * frac - Width(frame) / 2, 0)`, `frac = -(a - pi/2) / 2pi`).
fn strip_offset(a: f32, tex_extent: f32) -> f32 {
    -((a - FRAC_PI_2) / TAU) * tex_extent - (W - 1.0) * 0.5
}

/// The marker's centre column inside the 256-wide cycle: `256 * (heading - bearing) / 2pi + (W - 1) / 2`, wrapped into [0, 256].
fn marker_x(heading: f32, bearing: f32, tex: f32) -> f32 {
    let mut x = (tex * ((heading - bearing) / TAU) + (W - 1.0) * 0.5).floor();
    while x < 0.0 {
        x += tex;
    }
    while tex < x {
        x -= tex;
    }
    x
}

/// Screen position of the window (`FUN_1006d433`: `MoveTo(floor(screenW * 0.73 - width / 2), 5)`, then `MoveInsideScreen`).
pub(super) fn origin(screen: (u32, u32)) -> (i32, i32) {
    let x = (screen.0 as f32 * X_FACTOR - (W - 1.0) * 0.5).floor() as i32;
    (x.clamp(0, screen.0.saturating_sub(W as u32).max(0) as i32), Y)
}

impl Compass {
    pub(super) fn new(gui: &mut Gui, screen: (u32, u32)) -> anyhow::Result<Self> {
        let src = format!("<root><CanvasView name=\"compass\" min_size=\"Point({},{})\" max_size=\"Point({},{})\"/></root>", W - 1.0, H - 1.0, W - 1.0, H - 1.0);
        let window = gui.open_window_xml("CompassWindow", &src, (0, 0), WindowSize::Preferred)?;
        let mut c = Compass { window, waypoint: None, shown: None };
        c.resize(gui, screen);
        Ok(c)
    }

    pub(super) fn resize(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        gui.set_window_pos(self.window, origin(screen));
    }

    /// `criteria` of the window (`dvalue:cc_section1 && dvalue:cc_compass`).
    pub(super) fn set_visible(&mut self, gui: &mut Gui, on: bool) {
        gui.set_window_visible(self.window, on);
    }

    pub(super) fn set_waypoint(&mut self, w: Option<Waypoint>) {
        self.waypoint = w;
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone) {
        let own = zone.own();
        let yaw = own.and_then(|d| d.yaw).unwrap_or(0.0);
        let a = heading_angle(yaw);
        // `FUN_100670f8` shows the marker only while the waypoint's playfield is the current one (`+0x158 == +0x140`)
        let bearing = self.waypoint.filter(|w| zone.playfield == Some(w.playfield)).zip(own).map(|(w, d)| {
            let (dx, dz) = (w.pos[0] - d.pos[0], w.pos[2] - d.pos[2]);
            let mut b = dz.atan2(dx);
            while b < 0.0 {
                b += TAU;
            }
            b
        });
        if self.shown == Some((a, bearing)) {
            return;
        }
        self.shown = Some((a, bearing));
        gui.set_canvas(self.window, "compass", items(gui, a, bearing));
    }

    pub(super) fn close(self, gui: &mut Gui) {
        gui.close_window(self.window);
    }
}

/// Paint order = `AddRenderSurface` order: strip, waypoint, frame.
fn items(gui: &Gui, a: f32, bearing: Option<f32>) -> Vec<CanvasItem> {
    let mut out = vec![];
    let size = |id: u32| gui.gfx().size(GfxId(id));
    let (sw, sh) = size(STRIP_GFX);
    if sw > 0 {
        // vertically centred in the frame: `floor(Height(frame) / 2 - Height(strip) / 2)` on inclusive extents
        let y = ((H - 1.0) * 0.5 - (sh as f32 - 1.0) * 0.5).floor();
        let tex = sw as f32;
        let sx = strip_offset(a, tex - 1.0).rem_euclid(tex);
        let w1 = (tex - sx).min(W);
        let mut piece = |src_x: f32, dst_x: f32, w: f32| {
            out.push(CanvasItem::ImageTint { id: GfxId(STRIP_GFX), src: [src_x, 0.0, w, sh as f32], dst: [dst_x, y, dst_x + w, y + sh as f32], color: DEFAULT, alpha: ALPHA });
        };
        piece(sx, 0.0, w1);
        if w1 < W {
            piece(0.0, w1, W - w1);
        }
    }
    if let Some(b) = bearing {
        let (mw, mh) = size(WAYPOINT_GFX);
        if mw > 0 {
            // `floor(x - (Width(src) + 1) / 2)`, `floor((Height(view) + 1) / 2 - (Height(src) + 1) / 2)` on inclusive extents
            let x = (marker_x(a, b, sw as f32) - mw as f32 * 0.5).floor();
            let y = (H * 0.5 - mh as f32 * 0.5).floor();
            // the view clips to its bounds
            let (x0, x1) = (x.max(0.0), (x + mw as f32).min(W));
            if x1 > x0 {
                out.push(CanvasItem::Image { id: GfxId(WAYPOINT_GFX), src: [x0 - x, 0.0, x1 - x0, mh as f32], dst: [x0, y, x1, y + mh as f32], alpha: ALPHA });
            }
        }
    }
    let (fw, fh) = size(FRAME_GFX);
    out.push(CanvasItem::ImageTint { id: GfxId(FRAME_GFX), src: [0.0, 0.0, fw as f32, fh as f32], dst: [0.0, 0.0, fw as f32, fh as f32], color: DEFAULT, alpha: ALPHA });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Facing north (server heading 0, forward +z) the strip is centred on its first column (the "N" at x = 0), facing east
    /// (heading pi/2, forward +x) on the "E" a quarter of the way along. The original scrolls by `255 * frac` over the 256 px
    /// texture (`GetTextureSize` is the inclusive extent), so south / west are 0.5 / 0.75 px away from the exact 128 / 192, and east is
    /// 63.75 or, when the float `cos(pi/2)` is negative and `a` wraps to 2pi, 64.75.
    #[test]
    fn strip_is_centred_on_the_heading() {
        let centre = |yaw: f32| (strip_offset(heading_angle(yaw), 255.0) + (W - 1.0) * 0.5).rem_euclid(256.0);
        assert!(centre(0.0).min(256.0 - centre(0.0)) < 0.01, "{}", centre(0.0));
        assert!((centre(FRAC_PI_2) - 64.0).abs() < 1.0, "{}", centre(FRAC_PI_2));
        assert!((centre(std::f32::consts::PI) - 128.5).abs() < 0.01);
        assert!((centre(-FRAC_PI_2) - 192.25).abs() < 0.01);
    }

    /// A waypoint straight ahead sits at the window centre, one 90 degrees to the right a quarter cycle further.
    #[test]
    fn waypoint_marker_sits_at_its_relative_bearing() {
        let bearing = |dx: f32, dz: f32| dz.atan2(dx).rem_euclid(TAU);
        assert_eq!(marker_x(heading_angle(0.0), bearing(0.0, 10.0), 256.0), 68.0);
        assert_eq!(marker_x(heading_angle(0.0), bearing(10.0, 0.0), 256.0), 132.0);
        assert_eq!(marker_x(heading_angle(0.0), bearing(-10.0, 0.0), 256.0), 4.0);
    }

    #[test]
    fn origin_follows_the_screen_width() {
        // the install's template Rect(1800,5,..) was saved on a 2560 px wide screen
        assert_eq!(origin((2560, 1440)), (1800, 5));
        assert_eq!(origin((1920, 1080)), (1333, 5));
        assert_eq!(origin((100, 100)).0, 0);
    }
}
