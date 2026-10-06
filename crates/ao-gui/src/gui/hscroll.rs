//! Horizontal bar of a `ScrollView_c` (`SetHScrollBarMode`, used by `ScrolledListView_c` / `MultiListView_c`, docs/gui.md §13).
//!
//! UNRESOLVED: the horizontal bar art. GUI.dll only has the vertical `GFX_GUI_SCROLLBAR_GRAY_*` sprites, so the bar reuses them (unrotated arrows, the
//! track / thumb stretched along the bar).

use super::*;

impl Gui {
    /// Viewport (px) of a `ScrollView`: the size of its `ScrollViewChild` (the view minus the bars that are shown).
    pub(super) fn viewport(&self, sv: ViewId) -> (f32, f32) {
        let v = &self.tree.views[sv];
        match v.children.first() {
            Some(c) => (self.tree.views[*c].frame.width() + 1.0, self.tree.views[*c].frame.height() + 1.0),
            None => (v.frame.width() + 1.0, v.frame.height() + 1.0),
        }
    }

    /// (content width, viewport width, track length, thumb offset, thumb length) of the horizontal bar, if shown.
    fn hscroll_geometry(&self, sv: ViewId) -> Option<(f32, f32, f32, f32, f32)> {
        let view = &self.tree.views[sv];
        let Kind::ScrollView(sd) = &view.kind else { return None };
        let child = *view.children.first()?;
        let inner = *self.tree.views[child].children.first()?;
        let content = self.tree.views[inner].frame.width() + 1.0;
        let vis = self.viewport(sv).0;
        let shown = matches!(sd.h_mode, ScrollMode::Always) || (matches!(sd.h_mode, ScrollMode::Auto | ScrollMode::AutoReserve) && content > vis);
        if !shown {
            return None;
        }
        let track = (vis - 22.0).max(1.0);
        let thumb = (track * (vis / content).min(1.0)).max(8.0).min(track);
        let max_off = (content - vis).max(0.0);
        let t = if max_off > 0.0 { sd.offset.x / max_off * (track - thumb) } else { 0.0 };
        Some((content, vis, track, t, thumb))
    }

    pub(super) fn draw_hscrollbar(&mut self, out: &mut Vec<DrawCmd>, sv: ViewId, r: Rect, tint: [u8; 3], alpha: f32) {
        let Some((_, vis, _, thumb_t, thumb_h)) = self.hscroll_geometry(sv) else { return };
        let y = r.b - layout::SCROLLBAR_W;
        let w = layout::SCROLLBAR_W;
        let rr = r.l + vis - 1.0;
        let gid = |n: &str| self.gfx.id(n);
        let (up, down, empty, full) = (gid("GFX_GUI_SCROLLBAR_GRAY_UP_NORMAL"), gid("GFX_GUI_SCROLLBAR_GRAY_DOWN_NORMAL"), gid("GFX_GUI_SCROLLBAR_GRAY_EMPTY"), gid("GFX_GUI_SCROLLBAR_GRAY_FULL"));
        if let Some(g) = empty {
            self.push_gfx(out, g, Rect::new(r.l + 11.0, y, rr - 11.0, y + w), tint, alpha);
        }
        if let Some(g) = up {
            self.push_gfx(out, g, Rect::new(r.l, y, r.l + 10.0, y + w), tint, alpha);
        }
        if let Some(g) = down {
            self.push_gfx(out, g, Rect::new(rr - 10.0, y, rr, y + w), tint, alpha);
        }
        if let Some(g) = full {
            let l0 = r.l + 11.0 + thumb_t;
            self.push_gfx(out, g, Rect::new(l0, y, l0 + thumb_h - 1.0, y + w), tint, alpha);
        }
    }

    pub(super) fn hscroll_by(&mut self, sv: ViewId, dx: f32) {
        let Some((content, vis, ..)) = self.hscroll_geometry(sv) else { return };
        let max_off = (content - vis).max(0.0);
        if let Kind::ScrollView(sd) = &mut self.tree.views[sv].kind {
            sd.offset.x = (sd.offset.x + dx).clamp(0.0, max_off);
        }
    }

    /// Press on the horizontal bar (`x`,`y` screen px); false when the press is not on it.
    pub(super) fn hscroll_press(&mut self, sv: ViewId, x: f32, y: f32) -> bool {
        let Some((_, vis, _, th, thh)) = self.hscroll_geometry(sv) else { return false };
        let o = self.origin(sv);
        let win = self.window_of(sv).and_then(|w| self.windows[w].as_ref()).map_or((0, 0), |w| w.pos);
        let (l, t) = (o.0 + win.0 as f32, o.1 + win.1 as f32);
        let bar_top = t + self.tree.views[sv].frame.height() - layout::SCROLLBAR_W;
        if y < bar_top || x < l || x > l + vis {
            return false;
        }
        let rel = x - l;
        if rel < 11.0 {
            self.hscroll_by(sv, -13.0);
        } else if rel > vis - 11.0 {
            self.hscroll_by(sv, 13.0);
        } else if rel < 11.0 + th {
            self.hscroll_by(sv, -vis);
        } else if rel > 11.0 + th + thh {
            self.hscroll_by(sv, vis);
        } else {
            self.wx.hdrag = Some((sv, rel - th));
        }
        true
    }

    pub(super) fn hscroll_drag(&mut self, x: f32) {
        let Some((sv, grab)) = self.wx.hdrag else { return };
        let Some((content, vis, track, _, thh)) = self.hscroll_geometry(sv) else { return };
        let o = self.origin(sv);
        let win_x = self.window_of(sv).and_then(|w| self.windows[w].as_ref()).map_or(0, |w| w.pos.0) as f32;
        let t = (x - (o.0 + win_x) - grab).clamp(0.0, track - thh);
        let max_off = (content - vis).max(0.0);
        if let Kind::ScrollView(sd) = &mut self.tree.views[sv].kind {
            sd.offset.x = if track - thh > 0.0 { t / (track - thh) * max_off } else { 0.0 };
        }
    }
}
