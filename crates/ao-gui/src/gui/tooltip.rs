//! Tooltips (`docs/gui.md` §11): `View::SetToolTip` texts shown by `WindowController_c` after the mouse rests on a view.
//!
//! Original (GUI.dll): `WindowController_c::UpdateToolTip` 0x101577fd finds the topmost window under the mouse and
//! `FUN_10156d66` walks its views top child first (a view that contains the point, children before the view itself) until
//! `View::GetToolTipText` 0x1014db1a yields the texts set by `View::SetToolTip` 0x1014dadf (title `+0x140`, body `+0x15c`).
//! Every call re-arms a **500 ms** timer (`timeGetTime() + 500`); a different view (or a mouse button event,
//! `HandleMouseDown/Up` → `UpdateToolTip(true)`) closes the current tooltip. `WindowController_c::Render` 0x101572a1 creates
//! `ToolTip_c(title, body)` 0x10149a0d when the timer expires, replacing a shown one. The tooltip is a
//! `Window("", "info_window", style 2, flags 0x105)` (style 2 = `GFX_GUI_WINDOW_BACKGROUND` at layer-2 alpha, `SetBorderGfx`
//! 0x1015a82d) holding an `InfoContainer_c` (`FUN_101492c6`), placed by `Window::MoveToMouse` 0x10154e16.

use super::*;

/// `timeGetTime() + 500` in `WindowController_c::UpdateToolTip`.
const DELAY: f32 = 0.5;
/// `FontID_e` 5 of the `title_view` / `text_view` `TextView_c`s in `FUN_101492c6`.
const FONT: FontId = FontId::Normal;
/// `TextRenderer_c::SetAspectRatio(2.0)` on the body view (`_DAT_101ae17c`).
const ASPECT: f32 = 2.0;
/// `GFX_GUI_WINDOW_BACKGROUND` (`SetGfx(…, 0x1bf)`).
const WINDOW_BACKGROUND: GfxId = GfxId(0x1bf);

#[derive(Default)]
pub(super) struct TipState {
    /// `WindowController_c+0xa0`: the view (and, over a canvas, the 1-based index of its tip rectangle) the pending / shown texts belong to.
    view: Option<(ViewId, usize)>,
    title: String,
    body: String,
    deadline: Option<f32>,
    shown: Option<Shown>,
    pub(super) screen: (f32, f32),
}

struct Shown {
    /// Window top-left in screen pixels.
    pos: (i32, i32),
    m: Metrics,
}

/// `InfoContainer_c` geometry (inclusive extents, view-local, see `FUN_101492c6`).
struct Metrics {
    /// Window size in pixels.
    size: (i32, i32),
    /// Offset of the view inside the window (`Rect::Resize(1,1,-1,-1)` when a body exists).
    inset: f32,
    view: Rect,
    title: String,
    title_rect: Rect,
    body: Option<(text::TextLayout, Rect)>,
    separator: Option<Rect>,
    edges: [Rect; 4],
    black: bool,
}

impl Gui {
    /// Tooltip `title` / `body` (`View::SetToolTip`) of the named view; empty strings remove it.
    pub fn set_tooltip(&mut self, w: WindowId, name: &str, title: &str, body: &str) {
        if let Some(v) = self.find(w, name) {
            let tip = (!title.is_empty() || !body.is_empty()).then(|| (title.to_string(), body.to_string()));
            if let Kind::CcEntry(c) = &mut self.tree.views[v].kind {
                c.tip = tip.clone();
            }
            self.tree.views[v].tip = tip;
        }
    }

    /// Same for a view inside an `add_view` instance.
    pub fn set_tooltip_in(&mut self, h: ViewHandle, name: &str, title: &str, body: &str) {
        for v in self.find_all_in(h, name) {
            self.tree.views[v].tip = (!title.is_empty() || !body.is_empty()).then(|| (title.to_string(), body.to_string()));
        }
    }

    /// Screen size used by `Window::MoveToMouse` (default: the largest visible window).
    pub fn set_screen_size(&mut self, w: u32, h: u32) {
        self.tip.screen = (w as f32, h as f32);
    }

    /// `new ToolTip_c(title, body)` at the current mouse position (what the timer does for a view's `SetToolTip` texts).
    pub fn show_tooltip(&mut self, title: &str, body: &str) {
        self.tip.shown = Some(self.make_tip(title, body));
    }

    /// Texts of the tooltip window currently shown.
    pub fn tooltip_shown(&self) -> Option<(&str, &str)> {
        self.tip.shown.as_ref().map(|_| (self.tip.title.as_str(), self.tip.body.as_str()))
    }

    /// Top-left and size of the shown tooltip window.
    pub fn tooltip_rect(&self) -> Option<Rect> {
        self.tip.shown.as_ref().map(|s| Rect::new(s.pos.0 as f32, s.pos.1 as f32, (s.pos.0 + s.m.size.0 - 1) as f32, (s.pos.1 + s.m.size.1 - 1) as f32))
    }

    fn view_tip(&self, (id, region): (ViewId, usize)) -> Option<(String, String)> {
        let v = &self.tree.views[id];
        if let (Kind::Canvas(c), true) = (&v.kind, region > 0) {
            return c.tips.get(region - 1).map(|t| (t.title.clone(), t.body.clone()));
        }
        let t = v.tip.clone().or_else(|| if let Kind::CcEntry(d) = &v.kind { d.tip.clone() } else { None })?;
        (!t.0.is_empty() || !t.1.is_empty()).then_some(t)
    }

    /// `FUN_10156d66`: deepest-first search for a view with tooltip texts under `(x, y)` (window-local).
    #[allow(clippy::too_many_arguments)]
    fn tip_in(&self, id: ViewId, x: f32, y: f32, is_root: bool, ox: f32, oy: f32) -> Option<(ViewId, usize)> {
        let v = &self.tree.views[id];
        if !v.visible {
            return None;
        }
        let (l, t) = if is_root { (0.0, 0.0) } else { (ox + v.frame.l, oy + v.frame.t) };
        let r = Rect::new(l, t, l + v.frame.width(), t + v.frame.height());
        if !r.contains(Point::new(x, y)) {
            return None;
        }
        let (mut cox, mut coy) = (l, t);
        if matches!(v.kind, Kind::ScrollChild) {
            if let Some(Kind::ScrollView(sd)) = v.parent.map(|p| &self.tree.views[p].kind) {
                cox -= sd.offset.x;
                coy -= sd.offset.y;
            }
        }
        for c in v.children.iter().rev() {
            if let Some(h) = self.tip_in(*c, x, y, false, cox, coy) {
                return Some(h);
            }
        }
        if let Kind::Canvas(c) = &v.kind {
            // the rectangles stand for child controls of the original view: the topmost (last) one under the pointer
            let (cx, cy) = (x - l, y - t);
            if let Some(i) = c.tips.iter().rposition(|t| cx >= t.rect[0] && cx < t.rect[2] && cy >= t.rect[1] && cy < t.rect[3]) {
                return Some((id, i + 1));
            }
        }
        self.view_tip((id, 0)).map(|_| (id, 0))
    }

    /// Topmost window under the mouse decides (`UpdateToolTip` stops at the first window containing the pointer; a
    /// full-screen transparent window only counts where it has visible content, as for `wants_mouse`).
    fn tip_view_at(&self, x: f32, y: f32) -> Option<(ViewId, usize)> {
        for (_, root, pos) in self.windows_at(x, y) {
            let (lx, ly) = (x - pos.0 as f32, y - pos.1 as f32);
            if let Some(v) = self.tip_in(root, lx, ly, true, 0.0, 0.0) {
                return Some(v);
            }
            if self.covers(root, lx, ly, true, 0.0, 0.0) {
                return None;
            }
        }
        None
    }

    /// `WindowController_c::UpdateToolTip(close)`.
    pub(super) fn tip_update(&mut self, close: bool) {
        if close {
            self.tip_hide();
            return;
        }
        let hit = self.tip_view_at(self.mouse.x, self.mouse.y);
        if self.tip.view != hit {
            self.tip_hide();
        }
        if let Some((title, body)) = hit.and_then(|v| self.view_tip(v)) {
            self.tip.view = hit;
            self.tip.title = title;
            self.tip.body = body;
            self.tip.deadline = Some(self.time + DELAY);
        }
    }

    pub(super) fn tip_forget_views(&mut self, dead: &[ViewId]) {
        if self.tip.view.is_some_and(|(v, _)| dead.binary_search(&v).is_ok()) {
            self.tip_hide();
        }
    }

    fn tip_hide(&mut self) {
        self.tip.view = None;
        self.tip.title.clear();
        self.tip.body.clear();
        self.tip.deadline = None;
        self.tip.shown = None;
    }

    /// Timer expiry of `WindowController_c::Render` + drawing of the tooltip window (topmost, after every window).
    pub(super) fn tip_frame(&mut self, out: &mut Vec<DrawCmd>) {
        if self.tip.deadline.is_some_and(|d| self.time >= d) {
            self.tip.deadline = None;
            let (t, b) = (self.tip.title.clone(), self.tip.body.clone());
            self.tip.shown = Some(self.make_tip(&t, &b));
        }
        let Some(s) = self.tip.shown.take() else { return };
        self.draw_tip(&s, out);
        self.tip.shown = Some(s);
    }

    /// `InfoContainer_c` constructor `FUN_101492c6` geometry (see the module docs of `docs/gui.md` §11).
    fn tip_metrics(&mut self, title: &str, body: &str) -> Metrics {
        let has_body = !body.is_empty();
        let p = if has_body { 2.0 } else { 1.0 };
        let tp = text::string_size(&mut self.fonts, &self.colors, FONT, title);
        let (th, mut pref_x) = (tp.y, tp.x);
        let mut body_lay = None;
        let mut bh = 0.0;
        if has_body {
            // `TextRenderer_c::CalculatePreferredSize` 0x101623ea with an unset width: wrap width = ftol(sqrt(area) * aspect)
            // ([INFERENCE] the area `+0x18c` is the unwrapped text width × line height).
            let flat = text::layout_text(&mut self.fonts, &self.colors, FONT, body, tvf::MULTILINE | tvf::WORD_WRAP, None);
            let area = (flat.max_width * self.fonts.font(FONT).height) as f32;
            let w0 = (area.sqrt() * ASPECT) as i32;
            let lay = text::layout_text(&mut self.fonts, &self.colors, FONT, body, tvf::MULTILINE | tvf::WORD_WRAP, Some(w0));
            // the body is at least as wide as the title (`if (bodyW < titleW) bodyW = titleW`), then re-measured
            let bw = ((lay.max_width as f32 - 1.0).max(tp.x)).max(0.0);
            let lay = text::layout_text(&mut self.fonts, &self.colors, FONT, body, tvf::MULTILINE | tvf::WORD_WRAP, Some(bw as i32 + 1));
            bh = lay.height as f32 - 1.0;
            pref_x = tp.x.max(bw);
            body_lay = Some(lay);
        }
        let pref = Point::new(pref_x + 4.0 * p, if has_body { th + bh + 1.0 + 7.0 * p } else { th + 4.0 * p });
        let inset = if has_body { 1.0 } else { 0.0 };
        // window extents = pref (+2 when the view is inset by 1 on each side); the view fills the window minus the inset
        let win = Point::new(pref.x + 2.0 * inset, pref.y + 2.0 * inset);
        let view = Rect::new(0.0, 0.0, pref.x, pref.y);
        let edges = [
            Rect::new(0.0, 0.0, p - 1.0, view.b),
            Rect::new(0.0, 0.0, view.r, p - 1.0),
            Rect::new(view.r - p + 1.0, 0.0, view.r, view.b),
            Rect::new(0.0, view.b - p + 1.0, view.r, view.b),
        ];
        let title_rect = Rect::new(2.0 * p, 2.0 * p, view.r - 2.0 * p, th + 2.0 * p - 1.0);
        let body = body_lay.map(|l| (l, Rect::new(2.0 * p, th + 5.0 * p, view.r - 2.0 * p, view.b - 2.0 * p)));
        let separator = has_body.then(|| Rect::new(0.0, th + 3.0 * p, view.r, th + 4.0 * p - 1.0));
        Metrics {
            size: (win.x as i32 + 1, win.y as i32 + 1),
            inset,
            view,
            title: title.to_string(),
            title_rect,
            body,
            separator,
            edges,
            black: !has_body,
        }
    }

    /// `ToolTip_c::ToolTip_c` + `Window::MoveToMouse(false)` 0x10154e16 + `MoveInsideScreen`.
    fn make_tip(&mut self, title: &str, body: &str) -> Shown {
        let m = self.tip_metrics(title, body);
        let (sw, sh) = if self.tip.screen.0 > 0.0 {
            self.tip.screen
        } else {
            self.windows.iter().flatten().filter(|w| w.visible).fold((0.0f32, 0.0f32), |a, w| {
                let f = self.tree.views[w.root].frame;
                (a.0.max(w.pos.0 as f32 + f.width() + 1.0), a.1.max(w.pos.1 as f32 + f.height() + 1.0))
            })
        };
        // the window size passed through `Rect::Size` is an extent, hence `+ 1`
        let (ww, wh) = (m.size.0 as f32 - 1.0, m.size.1 as f32 - 1.0);
        let (mx, my) = (self.mouse.x, self.mouse.y);
        let x = if sw * 0.5 <= mx { mx - (ww + 1.0) } else { mx + 16.0 };
        let y = if sh * 0.5 <= my { my - (wh + 1.0) } else { my + 32.0 };
        let (x, y) = if sw > 0.0 { (x.min(sw - m.size.0 as f32).max(0.0), y.min(sh - m.size.1 as f32).max(0.0)) } else { (x, y) };
        Shown { pos: (x as i32, y as i32), m }
    }

    fn draw_tip(&mut self, s: &Shown, out: &mut Vec<DrawCmd>) {
        let (ox, oy) = (s.pos.0 as f32, s.pos.1 as f32);
        let m = &s.m;
        out.push(DrawCmd::Clip(None));
        let win = Rect::new(ox, oy, ox + m.size.0 as f32 - 1.0, oy + m.size.1 as f32 - 1.0);
        self.push_gfx(out, WINDOW_BACKGROUND, win, [255; 3], BUTTON_ALPHA);
        let (vx, vy) = (ox + m.inset, oy + m.inset);
        let solid = |out: &mut Vec<DrawCmd>, r: Rect, c: [u8; 3]| out.push(DrawCmd::Solid { dst: [vx + r.l, vy + r.t, vx + r.r + 1.0, vy + r.b + 1.0], color: c, alpha: 1.0 });
        if m.black {
            solid(out, m.view, [0, 0, 0]);
        }
        let hover = self.map_color(0x3000000);
        if let Some(r) = m.separator {
            solid(out, r, hover);
        }
        for e in &m.edges {
            solid(out, *e, hover);
        }
        // title colour: DEFAULT only when there is no body (`TextRenderer_c::SetDefaultColor`, `FUN_101492c6`); with a body the
        // default text colour is used (UNRESOLVED GUESS: TEXT).
        let title_col = if m.black { self.map_color(0x1000000) } else { self.map_color(0x4000000) };
        self.draw_string(out, FONT, &m.title, (vx + m.title_rect.l) as i32, (vy + m.title_rect.t) as i32, title_col, 1.0, false);
        if let Some((lay, r)) = &m.body {
            let text_col = self.map_color(0x4000000);
            for line in &lay.lines {
                let mut pen = (vx + r.l) as i32;
                if let crate::view::Align::Center = line.align {
                    pen += (r.width() as i32 + 1 - line.width) / 2;
                }
                for run in &line.runs {
                    let c = run.color.map_or(text_col, rgb);
                    pen += self.draw_string(out, FONT, &run.text, pen, (vy + r.t) as i32 + line.y, c, 1.0, false);
                }
            }
        }
    }
}
