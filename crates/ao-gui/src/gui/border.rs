//! `WndBorder` border buttons (icon `i`, `?`, pin, close) and the hover fade of unpinned windows (`WindowController_c::FadeWindows`).
//!
//! RE (GUI.dll): `WndBorder::CreateBorderIcons` 0x1015aba4 (styles 0/1/3 and flags & 4 == 0: icon button `BorderButton_c(0x1c5 x3)` as a toggle in list 0,
//! close `BorderButton_c(0x1c0, 0x1c1, 0x1c2)` in list 1, then the pin `BorderButton_c(0x1cd, 0x1ce, 0x1cf)` (toggle, list 1) only when window flag 0x800 is
//! clear), `WndBorder::SetHelpFile` 0x1015ae58 (the `?` button `0x1d0..0x1d2` is created by the first call, list 1, after the pin; the file name is kept at
//! `+0x1ec`), `SlotHelpButton` 0x10159e22 (`GlobalSignals+0x188("file://" + file)`), `Layout` 0x1015a1d9, `BorderButton_c` 0x10159ed2,
//! `Button_c::GetBorderView` 0x10128111 / `SetGfx` 0x10128270 / `StateChanged` 0x10128338 (docs/gui.md 6.2),
//! `WndBorder::GetPinButtonState` 0x10158ea0 / `SetPinButtonState` 0x1015af59, `Window::_CanFade` 0x10154885, `WindowController_c::HandleMouseMoved` 0x101583a0
//! -> `FadeWindows` 0x1015803c.

use super::*;

/// `BorderButton_c` sprites are 15 x 15 px (extent 14).
const BTN_EXT: f32 = 14.0;
/// Every button is placed 5 px below the window top (`Layout`: `Point(x, _DAT_101a8b98 = 5.0)`).
const BTN_Y: f32 = 5.0;
/// `Layout` spacing `_DAT_101a9da0` (the double 5.0) added before every button.
const BTN_GAP: f32 = 5.0;
/// `FadeTo` target of a window that lost the pointer (`_DAT_101ae0ec`) and the durations in microseconds (`0xf4240` = 1 s out, `0x30d40` = 0.2 s in).
const FADE_OUT_ALPHA: f32 = 0.3;
const FADE_OUT_SECS: f32 = 1.0;
const FADE_IN_SECS: f32 = 0.2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Btn {
    Icon,
    Close,
    Pin,
    Help,
}

/// Border state of a window besides the frame art.
#[derive(Clone)]
pub(super) struct Border {
    /// The pin button exists (window flag 0x800 clear).
    pub pin: bool,
    /// The pin button's toggle value (`GetPinButtonState`).
    pub pinned: bool,
    /// `WndBorder::SetHelpFile`: the `?` button exists while this is set.
    pub help: Option<String>,
    /// The window takes part in `FadeWindows` (the application opts chat windows out: they run their own `FadeTo` rule, docs/chat/gui.md).
    pub fade: bool,
    /// `Window::FadeTo` state: current alpha, target, change per second.
    pub cur: f32,
    pub to: f32,
    pub rate: f32,
}

impl Default for Border {
    fn default() -> Self {
        Self { pin: false, pinned: false, help: None, fade: false, cur: 1.0, to: 1.0, rate: 0.0 }
    }
}

impl Border {
    /// `Window::FadeTo(target, secs)`.
    fn fade_to(&mut self, to: f32, secs: f32) {
        self.to = to;
        self.rate = (to - self.cur).abs() / secs;
    }
    /// `Window::_CanFade` (flag 0x800 clear and the pin not pressed) for a window that takes part.
    fn can_fade(&self) -> bool {
        self.fade && self.pin && !self.pinned
    }
}

/// Screen rectangles (inclusive) of the border buttons of a window whose outer rectangle is `o`, in `Layout` order: the icon at the left, then
/// close, pin and `?` from the right edge inwards (`x = R - w - acc`, `acc += 5` before and `+= w - 1` after each button; `w` = 14, the extent of the
/// 15 px sprite, which is what makes the pitch 18 = the `n * 18` of `TabView::SetLeftMargin` in `AddBorderView` 0x1015aa95). `o` is the outer frame: the
/// buttons are children of the `WndBorder` view whose bounds `View::GetBounds` 0x1014af45 returns as `(0, 0, w, h)` (`Layout` uses them without any border inset).
pub(super) fn layout(o: Rect, pin: bool, help: bool) -> Vec<(Btn, Rect)> {
    let y = o.t + BTN_Y;
    let mut v = vec![(Btn::Icon, Rect::new(o.l + BTN_GAP, y, o.l + BTN_GAP + BTN_EXT, y + BTN_EXT))];
    let right = o.r;
    let mut acc = 0.0;
    for b in [Some(Btn::Close), pin.then_some(Btn::Pin), help.then_some(Btn::Help)].into_iter().flatten() {
        acc += BTN_GAP;
        let x = right - BTN_EXT - acc;
        v.push((b, Rect::new(x, y, x + BTN_EXT, y + BTN_EXT)));
        acc += BTN_EXT - 1.0;
    }
    v
}

fn over(r: Rect, p: Point) -> bool {
    p.x >= r.l && p.x <= r.r + 1.0 && p.y >= r.t && p.y <= r.b + 1.0
}

impl Gui {
    /// Shows or hides the pin button (window flag 0x800 clear = shown). Tabbed windows start with it (`DockWindow_c` flags 0x1000).
    pub fn set_window_pin_button(&mut self, w: WindowId, shown: bool) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.bd.pin = shown;
            if !shown {
                win.bd.pinned = false;
            }
        }
    }

    /// `WndBorder::SetPinButtonState`: sets the toggle and fades the window back to full alpha in 0.2 s.
    pub fn set_window_pinned(&mut self, w: WindowId, pinned: bool) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            if win.bd.pin {
                win.bd.pinned = pinned;
                win.bd.fade_to(1.0, FADE_IN_SECS);
            }
        }
    }

    /// `WndBorder::GetPinButtonState`.
    pub fn window_pinned(&self, w: WindowId) -> bool {
        matches!(self.windows.get(w), Some(Some(win)) if win.bd.pin && win.bd.pinned)
    }

    /// `Window::SetHelpFile`: adds the `?` button; clicking it raises [`Event::FrameHelp`] with `file://<file>`. `None` removes it.
    pub fn set_window_help(&mut self, w: WindowId, file: Option<&str>) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.bd.help = file.map(str::to_string);
        }
    }

    /// Lets the window take part in the hover fade of `WindowController_c::FadeWindows` (default for tabbed windows; chat windows opt out).
    pub fn set_window_fade(&mut self, w: WindowId, on: bool) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.bd.fade = on;
        }
    }

    /// Current `FadeWindows` alpha of the window (1.0 = opaque).
    pub fn window_fade(&self, w: WindowId) -> f32 {
        self.windows.get(w).and_then(|w| w.as_ref()).map_or(1.0, |w| w.bd.cur)
    }

    /// Border buttons of a framed window, screen coordinates.
    pub(super) fn border_buttons(&self, w: WindowId) -> Vec<(Btn, Rect)> {
        let Some(Some(win)) = self.windows.get(w) else { return vec![] };
        if !win.framed {
            return vec![];
        }
        let Some((x, y, ow, oh)) = self.window_outer_frame(w) else { return vec![] };
        layout(Rect::new(x as f32, y as f32, (x + ow as i32 - 1) as f32, (y + oh as i32 - 1) as f32), win.bd.pin, win.bd.help.is_some())
    }

    /// Draws the border buttons over the frame art: the raised view in DEFAULT, the pressed view (`Button_c` state 1: pressed, or the toggle value) in SELECTED
    /// instead, and the highlight view (state 2) in HOVER on top while the pointer is over the button; all at the layer-2 alpha.
    pub(super) fn draw_border_buttons(&mut self, w: WindowId, out: &mut Vec<DrawCmd>) {
        let (pinned, press) = (self.window_pinned(w), self.frame_press);
        let m = self.mouse;
        for (b, r) in self.border_buttons(w) {
            let names = match b {
                Btn::Icon => ["GFX_GUI_WINDOW_ICON_I"; 3],
                Btn::Close => ["GFX_GUI_WINDOW_CLOSE_X", "GFX_GUI_WINDOW_CLOSE_X_STATE2", "GFX_GUI_WINDOW_CLOSE_X_STATE3"],
                Btn::Pin => ["GFX_GUI_WINDOW_PIN", "GFX_GUI_WINDOW_PIN_STATE2", "GFX_GUI_WINDOW_PIN_STATE3"],
                Btn::Help => ["GFX_GUI_WINDOW_QUESTIONMARK", "GFX_GUI_WINDOW_QUESTIONMARK_STATE2", "GFX_GUI_WINDOW_QUESTIONMARK_STATE3"],
            };
            let held = press == Some((w, b)) && over(r, m);
            let pressed = held || (b == Btn::Pin && pinned);
            let ids = names.map(|n| self.gfx.id(n));
            let (base, col) = if pressed { (ids[1], 0x2000000) } else { (ids[0], 0x1000000) };
            if let Some(g) = base {
                let t = self.map_color(col);
                self.push_gfx(out, g, r, t, BUTTON_ALPHA);
            }
            if over(r, m) && !held {
                if let Some(g) = ids[2] {
                    let t = self.map_color(0x3000000);
                    self.push_gfx(out, g, r, t, BUTTON_ALPHA);
                }
            }
        }
    }

    /// Left press on close / pin / `?` of the topmost framed window under the pointer. The icon button is handled by the frame code. True = consumed.
    pub(super) fn border_press(&mut self, x: f32, y: f32) -> bool {
        let p = Point::new(x, y);
        for (wid, _, _) in self.windows_at(x, y) {
            let Some((ox, oy, ow, oh)) = self.window_outer_frame(wid).filter(|_| self.windows[wid].as_ref().is_some_and(|w| w.framed)) else { continue };
            if !(x >= ox as f32 && x < (ox + ow as i32) as f32 && y >= oy as f32 && y < (oy + oh as i32) as f32) {
                continue;
            }
            return match self.border_buttons(wid).into_iter().find(|(b, r)| *b != Btn::Icon && over(*r, p)) {
                Some((b, _)) => {
                    self.frame_press = Some((wid, b));
                    true
                }
                None => false,
            };
        }
        false
    }

    /// Left release: a button pressed by [`Gui::border_press`] acts when the pointer is still over it.
    pub(super) fn border_release(&mut self) {
        let Some((wid, b)) = self.frame_press.take() else { return };
        let m = self.mouse;
        if !self.border_buttons(wid).iter().any(|(bb, r)| *bb == b && over(*r, m)) {
            return;
        }
        match b {
            Btn::Close => self.events.push(Event::CloseRequested { window: wid }),
            Btn::Pin => {
                let pinned = !self.window_pinned(wid);
                self.set_window_pinned(wid, pinned);
                self.events.push(Event::FramePin { window: wid, pinned });
            }
            Btn::Help => {
                if let Some(f) = self.windows[wid].as_ref().and_then(|w| w.bd.help.clone()) {
                    self.events.push(Event::FrameHelp { window: wid, url: format!("file://{f}") });
                }
            }
            Btn::Icon => {}
        }
    }

    /// `Window::FadeTo` animation step.
    pub(super) fn tick_window_fades(&mut self, dt: f32) {
        for win in self.windows.iter_mut().flatten() {
            let b = &mut win.bd;
            if b.cur != b.to {
                let d = b.rate * dt;
                b.cur = if (b.to - b.cur).abs() <= d { b.to } else { b.cur + d * (b.to - b.cur).signum() };
            }
        }
    }

    /// `WindowController_c::HandleMouseMoved` 0x101583a0: when the window under the pointer changes and no button is held, the window left fades to 0.3 over
    /// 1 s and the window entered to 1.0 over 0.2 s (`FadeWindows` 0x1015803c; only windows for which `Window::_CanFade` holds).
    pub(super) fn update_window_hover(&mut self) {
        let m = self.mouse;
        let hovered = self
            .windows_at(m.x, m.y)
            .into_iter()
            .find(|(wid, root, pos)| match self.windows[*wid].as_ref() {
                Some(w) if w.framed => self.window_outer_frame(*wid).is_some_and(|(x, y, ow, oh)| m.x >= x as f32 && m.x < (x + ow as i32) as f32 && m.y >= y as f32 && m.y < (y + oh as i32) as f32),
                _ => self.covers(*root, m.x, m.y, true, pos.0 as f32, pos.1 as f32),
            })
            .map(|t| t.0);
        if hovered == self.border_hover {
            return;
        }
        let idle = self.pressed.is_none() && self.frame_press.is_none() && self.ix.frame_drag.is_none() && self.ix.tab_drag.is_none();
        let old = std::mem::replace(&mut self.border_hover, hovered);
        if !idle {
            return;
        }
        if let Some(Some(w)) = old.and_then(|o| self.windows.get_mut(o)) {
            if w.bd.can_fade() {
                w.bd.fade_to(FADE_OUT_ALPHA, FADE_OUT_SECS);
            }
        }
        if let Some(Some(w)) = hovered.and_then(|h| self.windows.get_mut(h)) {
            if w.bd.can_fade() {
                w.bd.fade_to(1.0, FADE_IN_SECS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_run_inwards_from_the_edges() {
        let o = Rect::new(100.0, 50.0, 399.0, 249.0);
        let v = layout(o, true, true);
        let r = |b: Btn| v.iter().find(|x| x.0 == b).unwrap().1;
        // `Layout` 0x1015a1d9: icon 5 from the left edge of the WndBorder frame, close 5 from the right (box = 15 px); pin / help 18 px apart
        assert_eq!((r(Btn::Icon).l, r(Btn::Icon).t), (105.0, 55.0));
        assert_eq!(r(Btn::Close).r, 399.0 - 5.0);
        assert_eq!(r(Btn::Close).width() + 1.0, 15.0);
        assert_eq!(r(Btn::Close).l - r(Btn::Pin).l, 18.0);
        assert_eq!(r(Btn::Pin).l - r(Btn::Help).l, 18.0);
        assert_eq!(layout(o, false, false).len(), 2);
    }

    #[test]
    fn fade_runs_linearly_and_pin_stops_it() {
        let mut b = Border { pin: true, fade: true, ..Default::default() };
        assert!(b.can_fade());
        b.fade_to(FADE_OUT_ALPHA, FADE_OUT_SECS);
        assert!((b.rate - 0.7).abs() < 1e-6);
        b.pinned = true;
        assert!(!b.can_fade());
        b.pin = false;
        b.pinned = false;
        assert!(!b.can_fade());
    }
}
