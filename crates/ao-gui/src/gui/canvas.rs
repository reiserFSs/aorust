//! `CanvasView`: a view the application paints (the planet / playfield map windows, `docs/gui.md` §12).

use super::*;

impl Gui {
    /// [`Gui::open_framed_window`] for view XML text built by the application.
    pub fn open_framed_window_xml(&mut self, name: &str, src: &str, pos: (i32, i32), size: WindowSize) -> Result<WindowId> {
        let id = self.open_window_xml(name, src, (pos.0 + FRAME_L, pos.1 + FRAME_T), size)?;
        if let Some(Some(w)) = self.windows.get_mut(id) {
            w.framed = true;
        }
        Ok(id)
    }

    /// [`Gui::open_tabbed_window`] for view XML text built by the application.
    pub fn open_tabbed_window_xml(&mut self, name: &str, title: &str, src: &str, pos: (i32, i32), size: WindowSize) -> Result<WindowId> {
        let id = self.open_window_xml(name, src, (pos.0 + TAB_L, pos.1 + TAB_T), size)?;
        self.frame_tabbed(id, title);
        Ok(id)
    }

    /// Replaces what the `CanvasView` called `name` paints (coordinates relative to the view).
    pub fn set_canvas(&mut self, w: WindowId, name: &str, items: Vec<CanvasItem>) {
        if let Some(v) = self.find(w, name) {
            if let Kind::Canvas(c) = &mut self.tree.views[v].kind {
                c.items = items;
            }
        }
    }

    /// Updates `View::SetMinPreferredSize` / `SetMaxPreferredSize` and propagates the layout change.
    pub fn set_view_pref_size(&mut self, w: WindowId, name: &str, min: (f32, f32), max: (f32, f32)) {
        if let Some(v) = self.find(w, name) {
            self.tree.views[v].min_size = Point::new(min.0, min.1);
            self.tree.views[v].max_size = Point::new(max.0, max.1);
            self.relayout_window(w);
        }
    }

    /// Fits a named owner's height to its visible content instead of stretching it
    /// inside a larger saved dock page. Recomputes after collapsed rows change.
    pub fn fit_view_height(&mut self, w: WindowId, name: &str) {
        let Some(id) = self.find(w, name) else { return };
        self.tree.views[id].min_size.y = 0.0;
        self.tree.views[id].max_size.y = 0.0;
        self.tree.views[id].max_limit.y = 16000.0;
        let mut env = Env { gfx: &self.gfx, fonts: &mut self.fonts, colors: &self.colors, groups: Default::default() };
        let height = layout::pref(&mut env, &self.tree, id, false).y;
        self.tree.views[id].min_size.y = height;
        self.tree.views[id].max_size.y = height;
        self.tree.views[id].max_limit.y = height;
        self.relayout_window(w);
    }

    /// Tooltips over rectangles of the `CanvasView` called `name` (replaces the previous ones); texts follow `View::SetToolTip`.
    pub fn set_canvas_tips(&mut self, w: WindowId, name: &str, tips: Vec<CanvasTip>) {
        if let Some(v) = self.find(w, name) {
            if let Kind::Canvas(c) = &mut self.tree.views[v].kind {
                c.tips = tips;
            }
        }
    }

    /// What the `CanvasView` called `name` currently paints (for tests / inspection).
    pub fn canvas_items(&self, w: WindowId, name: &str) -> &[CanvasItem] {
        match self.find(w, name).map(|v| &self.tree.views[v].kind) {
            Some(Kind::Canvas(c)) => &c.items,
            _ => &[],
        }
    }

    /// Size in pixels of a `CanvasView` (0 when the window or view is missing).
    pub fn canvas_size(&self, w: WindowId, name: &str) -> (u32, u32) {
        self.find(w, name).map_or((0, 0), |v| {
            let f = self.tree.views[v].frame;
            ((f.width() + 1.0) as u32, (f.height() + 1.0) as u32)
        })
    }

    pub(super) fn draw_canvas(&self, out: &mut Vec<DrawCmd>, c: &CanvasData, rect: Rect, alpha: f32) {
        // the renderer keeps one scissor: inside a `ScrollView` (itself a clip) the canvas clips to the intersection and restores the outer one
        let outer = push_clip(out, [rect.l as i32, rect.t as i32, rect.r as i32 + 1, rect.b as i32 + 1]);
        for item in &c.items {
            match *item {
                CanvasItem::Image { id, src, dst, alpha: a } => {
                    let d = [rect.l + dst[0], rect.t + dst[1], rect.l + dst[2], rect.t + dst[3]];
                    out.push(DrawCmd::Gfx { id, src, dst: d, tint: [255; 3], alpha: alpha * a });
                }
                CanvasItem::ImageTint { id, src, dst, color, alpha: a } => {
                    let d = [rect.l + dst[0], rect.t + dst[1], rect.l + dst[2], rect.t + dst[3]];
                    out.push(DrawCmd::Gfx { id, src, dst: d, tint: self.map_color(color), alpha: alpha * a });
                }
                CanvasItem::Solid { dst, color, alpha: a } => {
                    let d = [rect.l + dst[0], rect.t + dst[1], rect.l + dst[2], rect.t + dst[3]];
                    out.push(DrawCmd::Solid { dst: d, color: rgb(color), alpha: alpha * a });
                }
            }
        }
        out.push(DrawCmd::Clip(outer));
    }

    pub(super) fn canvas_down(&mut self, v: ViewId, x: f32, y: f32) {
        if let Kind::Canvas(c) = &mut self.tree.views[v].kind {
            c.drag = Some(Point::new(x, y));
            c.down = Point::new(x, y);
        }
        self.pressed = Some(v);
        self.canvas_press(v, x, y, MouseButton::Left);
    }

    /// Right button over a canvas: `Event::CanvasPress` only (no drag / click semantics).
    pub(super) fn canvas_right_down(&mut self, x: f32, y: f32) {
        if let Some((_, v)) = self.hit(x, y).filter(|(_, v)| matches!(self.tree.views[*v].kind, Kind::Canvas(_))) {
            self.canvas_press(v, x, y, MouseButton::Right);
        }
    }

    fn canvas_press(&mut self, v: ViewId, x: f32, y: f32, button: MouseButton) {
        let Some(window) = self.window_of(v) else { return };
        let m = Point::new(x, y);
        let double = self.canvas_press.is_some_and(|(pv, pb, t, p)| pv == v && pb == button && self.time - t <= DOUBLE_CLICK_TIME && (p.x - x).abs() <= 4.0 && (p.y - y).abs() <= 4.0);
        // a double click is consumed: the third press starts a new one
        self.canvas_press = if double { None } else { Some((v, button, self.time, m)) };
        let o = self.origin(v);
        let pos = self.windows[window].as_ref().map_or((0, 0), |w| w.pos);
        let view = self.tree.views[v].name.clone();
        self.events.push(Event::CanvasPress { window, view, x: x - o.0 - pos.0 as f32, y: y - o.1 - pos.1 as f32, button, clicks: 1 + double as u8 });
    }

    /// The pressed canvas lost the left button.
    pub(super) fn canvas_release(&mut self, v: ViewId) {
        if !matches!(self.tree.views[v].kind, Kind::Canvas(_)) {
            return;
        }
        if let Some(window) = self.window_of(v) {
            let view = self.tree.views[v].name.clone();
            self.events.push(Event::CanvasRelease { window, view });
        }
    }

    /// Left-drag over the pressed canvas: one `Event::CanvasDrag` per mouse step.
    pub(super) fn drag_canvas(&mut self, x: f32, y: f32) {
        let Some(v) = self.pressed else { return };
        let Kind::Canvas(c) = &mut self.tree.views[v].kind else { return };
        let Some(last) = c.drag else { return };
        c.drag = Some(Point::new(x, y));
        if let Some(window) = self.window_of(v) {
            let view = self.tree.views[v].name.clone();
            let o = self.origin(v);
            let pos = self.windows[window].as_ref().map_or((0, 0), |w| w.pos);
            self.events.push(Event::CanvasDrag { window, view, dx: x - last.x, dy: y - last.y, x: x - o.0 - pos.0 as f32, y: y - o.1 - pos.1 as f32 });
        }
    }

    /// Button released over the pressed canvas: a short press is an `Event::CanvasClick`.
    pub(super) fn canvas_up(&mut self, v: ViewId) {
        let m = self.mouse;
        let Kind::Canvas(c) = &mut self.tree.views[v].kind else { return };
        c.drag = None;
        if (m.x - c.down.x).abs() > 3.0 || (m.y - c.down.y).abs() > 3.0 {
            return;
        }
        let Some(window) = self.window_of(v) else { return };
        let o = self.origin(v);
        let pos = self.windows[window].as_ref().map_or((0, 0), |w| w.pos);
        let view = self.tree.views[v].name.clone();
        self.events.push(Event::CanvasClick { window, view, x: m.x - o.0 - pos.0 as f32, y: m.y - o.1 - pos.1 as f32 });
    }

    /// Wheel over a canvas; `true` when it was consumed.
    pub(super) fn canvas_wheel(&mut self, x: f32, y: f32, dy: f32) -> bool {
        let Some((_, v)) = self.hit(x, y) else { return false };
        if !matches!(self.tree.views[v].kind, Kind::Canvas(_)) {
            return false;
        }
        let Some(window) = self.window_of(v) else { return false };
        let o = self.origin(v);
        let pos = self.windows[window].as_ref().map_or((0, 0), |w| w.pos);
        let view = self.tree.views[v].name.clone();
        self.events.push(Event::CanvasWheel { window, view, dy, x: x - o.0 - pos.0 as f32, y: y - o.1 - pos.1 as f32 });
        true
    }
}
