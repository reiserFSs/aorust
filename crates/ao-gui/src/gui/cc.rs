//! Control-centre support of the GUI engine (`docs/gui.md` §10): criteria-driven visibility, views built from XML text,
//! the `CCMenuEntry_c` button art.

use super::*;
use crate::expr;

/// `CCMenuEntry_c` art ids (`SetGfx` in GUI.dll 0x10065b4e): (left, middle, right) of the raised, pressed and hover sets.
const RAISED: [u32; 3] = [0x9b, 0x9d, 0x9f];
const RAISED_GOLD: [u32; 3] = [0x9c, 0x9e, 0xa0];
const PRESSED: [u32; 3] = [0xa4, 0xa6, 0xa8];
const PRESSED_GOLD: [u32; 3] = [0xa5, 0xa7, 0xa9];
const HOVER: [u32; 3] = [0xa1, 0xa2, 0xa3];

impl Gui {
    /// `FUN_1006ee1e` (GUI.dll 0x1006ee1e): shows every view with an `activate_criteria`/`criteria` attribute iff its expression holds
    /// (`View::Show(CriteriaMonitor_c::Evaluate())`), then relayouts the window. `res` resolves `dvalue:` / `stat:` names.
    pub fn apply_criteria(&mut self, w: WindowId, res: &dyn Fn(&str, &str) -> Option<i64>) {
        let Some(Some(win)) = self.windows.get(w) else { return };
        let root = win.root;
        let mut stack = vec![root];
        let mut changed = false;
        while let Some(v) = stack.pop() {
            stack.extend(self.tree.views[v].children.iter().copied());
            if self.tree.views[v].criteria.is_empty() {
                continue;
            }
            let show = expr::truthy(&self.tree.views[v].criteria, res);
            if self.tree.views[v].visible != show {
                self.tree.views[v].visible = show;
                changed = true;
            }
        }
        if changed {
            self.relayout_window(w);
        }
    }

    /// Top-left of a window in screen pixels (for framed windows the client origin).
    pub fn window_pos(&self, w: WindowId) -> (i32, i32) {
        self.windows.get(w).and_then(|w| w.as_ref()).map_or((0, 0), |w| w.pos)
    }

    /// Resolves `id:GFX_NAME` (`ExpressionParser_c` skin ids) for [`Gui::apply_criteria`] resolvers.
    pub fn gfx_id(&self, name: &str) -> Option<u32> {
        self.gfx.id(name).map(|g| g.0)
    }

    /// Sets the dvalue state of a `CCMenuEntry` (toggle entries show the pressed art while active).
    pub fn set_cc_active(&mut self, w: WindowId, name: &str, active: bool) {
        if let Some(v) = self.find(w, name) {
            if let Kind::CcEntry(c) = &mut self.tree.views[v].kind {
                c.active = active;
            }
        }
    }

    /// `Button_c::SlotFadeTimer` 0x10127c6d: the fade runs toward 1 while hovered, toward 0 otherwise, 0.075 per tick
    /// (`_DAT_101c5780`). UNRESOLVED: the timer period (50 Hz assumed).
    pub(super) fn tick_cc_fades(&mut self, dt: f32) {
        let step = 0.075 * dt / 0.02;
        for v in &mut self.tree.views {
            if let Kind::CcEntry(c) = &mut v.kind {
                c.fade = if c.hover { (c.fade + step).min(1.0) } else { (c.fade - step).max(0.0) };
            }
        }
    }

    fn slice3(&self, out: &mut Vec<DrawCmd>, ids: [u32; 3], r: Rect, tint: [u8; 3], alpha: f32) {
        let [l, m, rt] = ids.map(GfxId);
        let (wl, wr) = (self.gfx.size(l).0 as f32, self.gfx.size(rt).0 as f32);
        self.push_gfx(out, m, Rect::new(r.l + wl, r.t, r.r - wr, r.b), tint, alpha);
        self.push_gfx(out, l, Rect::new(r.l, r.t, r.l + wl - 1.0, r.b), tint, alpha);
        self.push_gfx(out, rt, Rect::new(r.r - wr + 1.0, r.t, r.r, r.b), tint, alpha);
    }

    /// `Button_c` rendering (`StateChanged` 0x10128338) of a control-centre menu entry: raised/pressed 3-slice art tinted DEFAULT/SELECTED,
    /// the hover set tinted HOVER on top, the `bgicon` bitmap centred at layer-2 alpha with `Button_c::UpdateBgIconFade` 0x10127bdd tint
    /// (0x20 grey, full on hover; the ramp of `SlotFadeTimer` is not animated: UNRESOLVED).
    pub(super) fn draw_cc_entry(&mut self, out: &mut Vec<DrawCmd>, c: &CcEntryData, enabled: bool, r: Rect, tint: [u8; 3], alpha: f32) {
        let tint = mul(tint, if enabled { [255; 3] } else { [0x90; 3] });
        let pressed = c.pressed || (c.toggle && c.active);
        let (set, col) = match (pressed, c.golden) {
            (true, false) => (PRESSED, 0x2000000),
            (true, true) => (PRESSED_GOLD, 0x2000000),
            (false, false) => (RAISED, 0x1000000),
            (false, true) => (RAISED_GOLD, 0x1000000),
        };
        let t = mul(tint, self.map_color(col));
        self.slice3(out, set, r, t, alpha * BUTTON_ALPHA);
        if c.hover && enabled {
            let t = mul(tint, self.map_color(0x3000000));
            self.slice3(out, HOVER, r, t, alpha * BUTTON_ALPHA);
        }
        if let Some(g) = c.icon {
            let (w, h) = self.gfx.size(g);
            let l = r.l + ((r.width() + 1.0 - w as f32) * 0.5).floor();
            let t = r.t + ((r.height() + 1.0 - h as f32) * 0.5).floor();
            // `UpdateBgIconFade` 0x10127bdd: tint 0x20 + (1 - fade) * 223 (bright at rest, dimmed while the label fades in)
            let grey = 0x20 + ((1.0 - c.fade) * 223.0) as u32;
            self.push_gfx(out, g, Rect::new(l, t, l + w as f32 - 1.0, t + h as f32 - 1.0), mul(tint, [grey as u8; 3]), alpha * BUTTON_ALPHA);
        }
        // the label view is shown iff fade > 0.8 (`_DAT_101c5778`) once an icon exists; label-only entries always show it
        if !c.label.is_empty() && (c.icon.is_none() || c.fade > 0.8) {
            let s = text::string_size(&mut self.fonts, &self.colors, FontId::Normal, &c.label);
            let (lw, lh) = (s.x + 1.0, s.y + 1.0);
            let lx = r.l + ((r.width() + 1.0 - lw) * 0.5).floor();
            let ly = r.t + ((r.height() + 1.0 - lh) * 0.5).floor();
            let lt = mul(tint, self.map_color(col));
            self.draw_string(out, FontId::Normal, &c.label, lx as i32, ly as i32, lt, alpha, false);
        }
    }
}
