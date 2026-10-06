//! Style-0 window frame interaction: `WndBorder` hit-testing, move / resize, the multi-tab strip with tab dragging and the frame's
//! icon / context events (docs/chat/gui.md §2, §10; docs/gui.md §6.1).
//!
//! RE: `WndBorder::HitTest` 0x101593d6 (hit items: 1 move, 2 left, 3 top, 4 right, 5 bottom, 6 TL, 7 TR, 8 BL, 9 BR; window flag 0x8 = not movable, 0x10 / 0x20 =
//! not resizable horizontally / vertically, 0x1 = no hit at all), `Layout` 0x1015a1d9 (rect +0x23c = the move zone), `MouseDown` 0x101595f7,
//! `MouseMove` 0x10159c27, `MouseUp` 0x101596a7, `DoSetFrame` 0x10159888.

use super::*;

/// Outer-width floor of `DoSetFrame`: `+0x2ec - 1`, the width of the border buttons (`Layout`: each button adds 5 + (14 - 1) = 18 px, icon button left
/// + close button right) minus 1 (extent). **GUESS** that the pin/help buttons are absent (chat windows are created with the default flags).
pub const MIN_OUTER_EXTENT_W: f32 = 35.0;
/// Pointer travel (px) before a pressed tab turns into a drag. **GUESS** (the `TabView` drag threshold was not found).
const TAB_DRAG_START: f32 = 4.0;
/// Bottom row of the move zone inside the window (`_DAT_101c894c` = 18.0, `Layout` +0x248); its top is the first row below the top border (7).
const MOVE_ZONE_BOTTOM: f32 = 18.0;
/// Style-0 `WndBorder` border (3, 7, 3, 3) (`UpdateBorderSizes` 0x10159f98).
const BORDER: (f32, f32, f32, f32) = (3.0, 7.0, 3.0, 3.0);
/// Unselected tabs: `Tab::SetSelected` 0x10146110 draws the inactive art at alpha × `_DAT_101c4978` (= 2.0, a multiplier we cannot apply to 0.85:
/// **GUESS**: the inactive art is drawn at half the layer alpha).
pub(super) const INACTIVE_TAB_ALPHA: f32 = BUTTON_ALPHA * 0.5;

/// Per-window frame state set by the application.
#[derive(Clone, Default)]
pub(super) struct WinFx {
    pub movable: bool,
    pub resizable: bool,
    pub context: bool,
    pub tabs: Vec<String>,
    pub sel: usize,
    /// Client size limits in pixels (`WndBorder::SetSizeLimits`); a max of 0 = unlimited.
    pub min: (u32, u32),
    pub max: (u32, u32),
}

pub(super) struct FrameDrag {
    pub window: WindowId,
    pub hit: u8,
    pub mouse0: Point,
    pub start: Rect,
}

pub(super) struct TabDrag {
    pub window: WindowId,
    pub tab: usize,
    pub mouse0: Point,
    pub moved: bool,
}

/// All the interaction state the engine adds on top of the plain widgets.
#[derive(Default)]
pub(super) struct Ix {
    pub frame_drag: Option<FrameDrag>,
    pub tab_drag: Option<TabDrag>,
    pub menu: Option<super::popup::Menu>,
    pub sel: Option<super::select::Sel>,
    /// Seconds since the last auto-scroll step of a selection drag.
    pub sel_timer: f32,
}

/// `WndBorder::HitTest` on the *outer* rectangle `o` (inclusive extents): 0 = nothing, 1..9 as in the module docs.
pub fn hit_item(o: Rect, p: Point, movable: bool, resizable: bool) -> u8 {
    let (bl, bt, br, bb) = BORDER;
    let inside = p.x >= o.l && p.x < o.r + 1.0 && p.y >= o.t && p.y < o.b + 1.0;
    if !inside {
        return 0;
    }
    let left = p.x < o.l + bl;
    let right = p.x >= o.r + 1.0 - br;
    let top = p.y < o.t + bt;
    let bottom = p.y >= o.b + 1.0 - bb;
    if resizable {
        match (left, right, top, bottom) {
            (true, _, true, _) => return 6,
            (_, true, true, _) => return 7,
            (true, _, _, true) => return 8,
            (_, true, _, true) => return 9,
            (true, ..) => return 2,
            (_, true, ..) => return 4,
            (_, _, _, true) => return 5,
            (_, _, true, _) => return 3,
            _ => {}
        }
    }
    // `Layout`: the move zone spans the top border between the corners, from the row below the top border down to `_DAT_101c894c`
    if movable && p.x >= o.l + bl && p.x < o.r + 1.0 - br && p.y >= o.t + bt && p.y <= o.t + MOVE_ZONE_BOTTOM {
        return 1;
    }
    0
}

/// The outer rectangle `start` after the pointer moved by `d` while grabbing `hit` (before clamping): `MouseMove` 0x10159c27.
pub fn dragged_rect(start: Rect, hit: u8, d: Point) -> Rect {
    let mut r = start;
    if hit == 1 {
        return r.translate(d.x, d.y);
    }
    if matches!(hit, 2 | 6 | 8) {
        r.l += d.x;
    }
    if matches!(hit, 4 | 7 | 9) {
        r.r += d.x;
    }
    if matches!(hit, 3 | 6 | 7) {
        r.t += d.y;
    }
    if matches!(hit, 5 | 8 | 9) {
        r.b += d.y;
    }
    r
}

/// `DoSetFrame` 0x10159888: clamps the *client* size to the limits, then the outer width to the button row, keeping the edge opposite to the
/// dragged one fixed (top edge moves for hit 3/6/7, left edge for 2/6/8). `insets` = client insets (l, t, r, b) of the frame.
pub fn clamp_outer(mut o: Rect, hit: u8, insets: (i32, i32, i32, i32), min: (u32, u32), max: (u32, u32)) -> Rect {
    if hit == 1 {
        return o;
    }
    let (ih, iv) = ((insets.0 + insets.2) as f32, (insets.1 + insets.3) as f32);
    let mut cw = o.width() + 1.0 - ih;
    let mut ch = o.height() + 1.0 - iv;
    cw = cw.max(min.0.max(1) as f32);
    ch = ch.max(min.1.max(1) as f32);
    if max.0 > 0 {
        cw = cw.min(max.0 as f32);
    }
    if max.1 > 0 {
        ch = ch.min(max.1 as f32);
    }
    let ow = (cw + ih).max(MIN_OUTER_EXTENT_W + 1.0);
    let oh = ch + iv;
    if matches!(hit, 2 | 6 | 8) {
        o.l = o.r - (ow - 1.0);
    } else {
        o.r = o.l + (ow - 1.0);
    }
    if matches!(hit, 3 | 6 | 7) {
        o.t = o.b - (oh - 1.0);
    } else {
        o.b = o.t + (oh - 1.0);
    }
    o
}

/// Insert index of a tab dropped at `x` among tabs whose rectangles are `tabs`: before the first tab whose centre is right of `x`.
pub fn insert_index(tabs: &[Rect], x: f32) -> usize {
    tabs.iter().position(|r| (r.l + r.r) * 0.5 > x).unwrap_or(tabs.len())
}

impl Gui {
    /// `Window::SetStyle` flags 0x8 / 0x10|0x20 inverted: lets the user move / resize a style-0 window by its frame (docs/chat/gui.md §2).
    pub fn set_window_frame(&mut self, w: WindowId, movable: bool, resizable: bool) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.fx.movable = movable;
            win.fx.resizable = resizable;
        }
    }

    /// `WndBorder::SetSizeLimits(min, max)`: client size limits in pixels (max 0 = unlimited).
    pub fn set_window_size_limits(&mut self, w: WindowId, min: (u32, u32), max: (u32, u32)) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.fx.min = min;
            win.fx.max = max;
        }
    }

    /// Right-clicks inside the window (frame included) raise [`Event::ContextMenu`].
    pub fn set_window_context(&mut self, w: WindowId, on: bool) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.fx.context = on;
        }
    }

    /// `Window::InsertTab` titles of a style-0 window and the selected tab (the first one is the window title). The window must be tabbed.
    pub fn set_window_tabs(&mut self, w: WindowId, tabs: &[String], selected: usize) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.fx.tabs = tabs.to_vec();
            win.fx.sel = selected.min(tabs.len().saturating_sub(1));
            if let Some(t) = tabs.get(win.fx.sel) {
                win.title = Some(t.clone());
            }
        }
    }

    /// Tab titles and the selected index.
    pub fn window_tabs(&self, w: WindowId) -> (Vec<String>, usize) {
        match self.windows.get(w) {
            Some(Some(win)) if !win.fx.tabs.is_empty() => (win.fx.tabs.clone(), win.fx.sel),
            Some(Some(win)) => (win.title.iter().cloned().collect(), 0),
            _ => (vec![], 0),
        }
    }

    /// Outer frame (x, y, width, height in pixels) of a framed window; the client rectangle for unframed ones.
    pub fn window_outer_frame(&self, w: WindowId) -> Option<(i32, i32, u32, u32)> {
        let win = self.windows.get(w)?.as_ref()?;
        let (cw, ch) = self.window_size(w);
        let i = if win.framed { self.insets(win) } else { (0, 0, 0, 0) };
        Some((win.pos.0 - i.0, win.pos.1 - i.1, cw + (i.0 + i.2) as u32, ch + (i.1 + i.3) as u32))
    }

    /// Moves / resizes a window to the outer frame `(x, y, w, h)` (no limit clamping: the caller clamps).
    pub fn set_window_outer_frame(&mut self, w: WindowId, f: (i32, i32, u32, u32)) {
        let Some(Some(win)) = self.windows.get(w) else { return };
        let i = if win.framed { self.insets(win) } else { (0, 0, 0, 0) };
        let (cw, ch) = (f.2.saturating_sub((i.0 + i.2) as u32).max(1), f.3.saturating_sub((i.1 + i.3) as u32).max(1));
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.pos = (f.0 + i.0, f.1 + i.1);
        }
        self.resize_window(w, WindowSize::Fixed(cw, ch));
    }

    fn outer_of(&self, w: WindowId) -> Option<Rect> {
        let (x, y, ow, oh) = self.window_outer_frame(w)?;
        Some(Rect::new(x as f32, y as f32, (x + ow as i32 - 1) as f32, (y + oh as i32 - 1) as f32))
    }

    /// Tab rectangles (screen pixels, inclusive) of a tabbed window: tabs sit side by side, the first 20 px from the left edge of the `TabView`
    /// (`TabView::SetLeftMargin`), each `title width + 1 + 5 + 16` wide (`FUN_101461dc`). **GUESS**: no gap between tabs.
    pub(super) fn tab_rects(&mut self, w: WindowId) -> Vec<Rect> {
        let (tabs, _) = self.window_tabs(w);
        let Some(o) = self.outer_of(w) else { return vec![] };
        let tv = o.resize(BORDER.0, BORDER.1, -BORDER.2, -BORDER.3);
        let mut l = tv.l + TAB_LEFT_MARGIN;
        tabs.iter()
            .map(|t| {
                let tw: i32 = t.chars().map(|c| self.fonts.font(FontId::Normal).advance(c)).sum();
                let wd = tw as f32 + 1.0 + TAB_PAD_L + TAB_PAD_R;
                let r = Rect::new(l, tv.t, l + wd - 1.0, tv.t + TAB_H - 1.0);
                l += wd;
                r
            })
            .collect()
    }

    fn is_tabbed(&self, w: WindowId) -> bool {
        matches!(self.windows.get(w), Some(Some(win)) if win.framed && win.title.is_some())
    }

    /// Left press on the frame of a tabbed window. True = consumed.
    pub(super) fn frame_mouse_down(&mut self, x: f32, y: f32) -> bool {
        let p = Point::new(x, y);
        for (wid, root, pos) in self.windows_top_down() {
            if !self.is_tabbed(wid) {
                continue;
            }
            let Some(o) = self.outer_of(wid) else { continue };
            if !(x >= o.l && x < o.r + 1.0 && y >= o.t && y < o.b + 1.0) {
                continue;
            }
            let fx = self.windows[wid].as_ref().map(|w| w.fx.clone()).unwrap_or_default();
            if fx.movable {
                let icon = self.frame_buttons(root, pos, true).0;
                if p.x >= icon.l && p.x <= icon.r + 1.0 && p.y >= icon.t && p.y <= icon.b + 1.0 {
                    self.events.push(Event::FrameIcon { window: wid, x: icon.l as i32, y: icon.b as i32 + 1 });
                    return true;
                }
            }
            let rects = self.tab_rects(wid);
            if let Some(i) = rects.iter().position(|r| r.contains(p)) {
                if let Some(Some(win)) = self.windows.get_mut(wid) {
                    if !win.fx.tabs.is_empty() {
                        win.fx.sel = i;
                        win.title = win.fx.tabs.get(i).cloned();
                    }
                }
                self.events.push(Event::TabSelected { window: wid, index: i });
                self.ix.tab_drag = Some(TabDrag { window: wid, tab: i, mouse0: p, moved: false });
                return true;
            }
            let hit = hit_item(o, p, fx.movable, fx.resizable);
            if hit != 0 {
                self.ix.frame_drag = Some(FrameDrag { window: wid, hit, mouse0: p, start: o });
                return true;
            }
            return false;
        }
        false
    }

    pub(super) fn frame_mouse_move(&mut self, x: f32, y: f32) {
        if let Some(d) = &self.ix.frame_drag {
            let (wid, hit, m0, start) = (d.window, d.hit, d.mouse0, d.start);
            let Some(Some(win)) = self.windows.get(wid) else {
                self.ix.frame_drag = None;
                return;
            };
            let (min, max) = (win.fx.min, win.fx.max);
            let insets = self.insets(win);
            let r = dragged_rect(start, hit, Point::new(x - m0.x, y - m0.y));
            let r = clamp_outer(r, hit, insets, min, max);
            let new = (r.l as i32, r.t as i32, (r.width() + 1.0) as u32, (r.height() + 1.0) as u32);
            if self.window_outer_frame(wid) != Some(new) {
                self.set_window_outer_frame(wid, new);
                self.events.push(Event::WindowFrame { window: wid });
            }
        }
        if let Some(d) = &mut self.ix.tab_drag {
            if !d.moved && ((x - d.mouse0.x).powi(2) + (y - d.mouse0.y).powi(2)).sqrt() > TAB_DRAG_START {
                d.moved = true;
            }
        }
    }

    pub(super) fn frame_mouse_up(&mut self) {
        self.ix.frame_drag = None;
        let Some(d) = self.ix.tab_drag.take() else { return };
        if !d.moved {
            return;
        }
        let m = self.mouse;
        let mut target = None;
        for (wid, _, _) in self.windows_top_down() {
            if !self.is_tabbed(wid) {
                continue;
            }
            let Some(o) = self.outer_of(wid) else { continue };
            let tv = o.resize(BORDER.0, BORDER.1, -BORDER.2, -BORDER.3);
            if m.x >= tv.l && m.x <= tv.r + 1.0 && m.y >= tv.t && m.y < tv.t + TAB_H + 1.0 {
                let rects = self.tab_rects(wid);
                target = Some((wid, insert_index(&rects, m.x)));
                break;
            }
        }
        self.events.push(Event::TabDropped { window: d.window, tab: d.tab, x: m.x as i32, y: m.y as i32, target });
    }

    /// Right press: [`Event::ContextMenu`] for the topmost window with the context flag whose frame / client contains the pointer.
    pub(super) fn frame_right_down(&mut self, x: f32, y: f32) {
        for (wid, _, _) in self.windows_top_down() {
            let ctx = matches!(self.windows.get(wid), Some(Some(w)) if w.fx.context);
            let Some(o) = self.outer_of(wid) else { continue };
            if !(x >= o.l && x < o.r + 1.0 && y >= o.t && y < o.b + 1.0) {
                continue;
            }
            if !ctx {
                return;
            }
            let link = self.hit(x, y).filter(|h| h.0 == wid).and_then(|(_, v)| match self.tree.views[v].kind.clone() {
                Kind::Text(t) => self.link_at(v, &t, x, y),
                _ => None,
            });
            self.events.push(Event::ContextMenu { window: wid, x: x as i32, y: y as i32, link });
            return;
        }
    }

    /// The tab being dragged follows the pointer (`TabView::CreateDragImage`): drawn at half alpha.
    pub(super) fn draw_tab_ghost(&mut self, out: &mut Vec<DrawCmd>) {
        let Some(d) = &self.ix.tab_drag else { return };
        if !d.moved {
            return;
        }
        let (wid, tab) = (d.window, d.tab);
        let (tabs, _) = self.window_tabs(wid);
        let Some(title) = tabs.get(tab).cloned() else { return };
        let tw: i32 = title.chars().map(|c| self.fonts.font(FontId::Normal).advance(c)).sum();
        let w = tw as f32 + 1.0 + TAB_PAD_L + TAB_PAD_R;
        let m = self.mouse;
        let r = Rect::new(m.x - w * 0.5, m.y - TAB_H * 0.5, m.x + w * 0.5 - 1.0, m.y + TAB_H * 0.5 - 1.0);
        let col = self.map_color(0x1000000);
        self.draw_tab(out, r, &title, true, col, 0.5);
    }

    /// One tab: 3-slice art (active / inactive) and the title (`Tab::SetSelected` 0x10146110: selected text white, unselected colour 0).
    pub(super) fn draw_tab(&mut self, out: &mut Vec<DrawCmd>, tab: Rect, title: &str, selected: bool, col: [u8; 3], fade: f32) {
        let (names, alpha) = if selected {
            (["GFX_GUI_TAB_ACTIVE_LEFT", "GFX_GUI_TAB_ACTIVE_MIDDLE", "GFX_GUI_TAB_ACTIVE_RIGHT"], BUTTON_ALPHA)
        } else {
            (["GFX_GUI_TAB_INACTIVE_LEFT", "GFX_GUI_TAB_INACTIVE_MIDDLE", "GFX_GUI_TAB_INACTIVE_RIGHT"], INACTIVE_TAB_ALPHA)
        };
        let alpha = alpha * fade;
        if let [Some(gl), Some(gm), Some(gr)] = names.map(|n| self.gfx.id(n)) {
            let (wl, wr) = (self.gfx.size(gl).0 as f32, self.gfx.size(gr).0 as f32);
            self.push_gfx(out, gm, Rect::new(tab.l + wl, tab.t, tab.r - wr, tab.b), col, alpha);
            self.push_gfx(out, gl, Rect::new(tab.l, tab.t, tab.l + wl - 1.0, tab.b), col, alpha);
            self.push_gfx(out, gr, Rect::new(tab.r - wr + 1.0, tab.t, tab.r, tab.b), col, alpha);
        }
        let text = if selected { [255; 3] } else { [0; 3] };
        self.draw_string(out, FontId::Normal, title, (tab.l + TAB_PAD_L) as i32, tab.t as i32, text, fade, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outer() -> Rect {
        Rect::new(100.0, 200.0, 399.0, 349.0) // 300 x 150 px
    }

    #[test]
    fn hit_zones_follow_hittest() {
        let o = outer();
        let h = |x, y, m, r| hit_item(o, Point::new(x, y), m, r);
        assert_eq!(h(101.0, 201.0, true, true), 6); // TL corner
        assert_eq!(h(398.0, 201.0, true, true), 7);
        assert_eq!(h(101.0, 348.0, true, true), 8);
        assert_eq!(h(398.0, 348.0, true, true), 9);
        assert_eq!(h(101.0, 260.0, true, true), 2);
        assert_eq!(h(398.0, 260.0, true, true), 4);
        assert_eq!(h(250.0, 348.0, true, true), 5);
        assert_eq!(h(250.0, 201.0, true, true), 3);
        // the strip row below the top border moves the window, the content does not hit
        assert_eq!(h(250.0, 212.0, true, true), 1);
        assert_eq!(h(250.0, 230.0, true, true), 0);
        // flags: not resizable -> edges do nothing, not movable -> strip does nothing
        assert_eq!(h(101.0, 260.0, true, false), 0);
        assert_eq!(h(250.0, 212.0, false, true), 0);
        assert_eq!(h(90.0, 212.0, true, true), 0);
    }

    #[test]
    fn drag_moves_and_resize_clamps() {
        let o = outer();
        assert_eq!(dragged_rect(o, 1, Point::new(10.0, -5.0)), o.translate(10.0, -5.0));
        let insets = (5, 26, 5, 5);
        // dragging the right edge left far past the minimum: width stops at the button-row floor, left edge stays
        let r = clamp_outer(dragged_rect(o, 4, Point::new(-1000.0, 0.0)), 4, insets, (0, 0), (0, 0));
        assert_eq!((r.l, r.width()), (100.0, MIN_OUTER_EXTENT_W));
        // dragging the left edge right: the right edge stays fixed
        let r = clamp_outer(dragged_rect(o, 2, Point::new(1000.0, 0.0)), 2, insets, (0, 0), (0, 0));
        assert_eq!((r.r, r.width()), (399.0, MIN_OUTER_EXTENT_W));
        // top-left corner with client limits (min 100x80, max 250x120): growth and shrink clamp, bottom-right stays
        let r = clamp_outer(dragged_rect(o, 6, Point::new(-500.0, -500.0)), 6, insets, (100, 80), (250, 120));
        assert_eq!((r.r, r.b, r.width() + 1.0 - 10.0, r.height() + 1.0 - 31.0), (399.0, 349.0, 250.0, 120.0));
        let r = clamp_outer(dragged_rect(o, 9, Point::new(-500.0, -500.0)), 9, insets, (100, 80), (250, 120));
        assert_eq!((r.l, r.t, r.width() + 1.0 - 10.0, r.height() + 1.0 - 31.0), (100.0, 200.0, 100.0, 80.0));
    }

    #[test]
    fn tab_insert_index() {
        let tabs = [Rect::new(0.0, 0.0, 49.0, 16.0), Rect::new(50.0, 0.0, 99.0, 16.0)];
        assert_eq!(insert_index(&tabs, 10.0), 0);
        assert_eq!(insert_index(&tabs, 40.0), 1);
        assert_eq!(insert_index(&tabs, 90.0), 2);
    }
}
