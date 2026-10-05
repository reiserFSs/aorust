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

    /// Replaces what the `CanvasView` called `name` paints (coordinates relative to the view).
    pub fn set_canvas(&mut self, w: WindowId, name: &str, items: Vec<CanvasItem>) {
        if let Some(v) = self.find(w, name) {
            if let Kind::Canvas(c) = &mut self.tree.views[v].kind {
                c.items = items;
            }
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
        out.push(DrawCmd::Clip(Some([rect.l as i32, rect.t as i32, rect.r as i32 + 1, rect.b as i32 + 1])));
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
        out.push(DrawCmd::Clip(None));
    }

    pub(super) fn canvas_down(&mut self, v: ViewId, x: f32, y: f32) {
        if let Kind::Canvas(c) = &mut self.tree.views[v].kind {
            c.drag = Some(Point::new(x, y));
            c.down = Point::new(x, y);
        }
        self.pressed = Some(v);
    }

    /// Left-drag over the pressed canvas: one `Event::CanvasDrag` per mouse step.
    pub(super) fn drag_canvas(&mut self, x: f32, y: f32) {
        let Some(v) = self.pressed else { return };
        let Kind::Canvas(c) = &mut self.tree.views[v].kind else { return };
        let Some(last) = c.drag else { return };
        c.drag = Some(Point::new(x, y));
        if let Some(window) = self.window_of(v) {
            let view = self.tree.views[v].name.clone();
            self.events.push(Event::CanvasDrag { window, view, dx: x - last.x, dy: y - last.y });
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
