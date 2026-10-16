//! `DialogBox_c` confirmation / message boxes of the HUD windows (team invitation, perk train / untrain confirmations and the perk "Info" boxes).
//! Layout and button row are those of `DialogBoxView_c` 0x1012aa23 / `DialogButtons_c::_Initialize` 0x1012ad5f, read in docs/chat/dialogs.md §2
//! (the chat side keeps its own copy for `/messagebox`, `/org leave`, `/afk`); the dialog opens centred (`DialogBox_c::Go` + `Window::MoveToCenter`).

use ao_gui::{Event, Gui, WindowId, WindowSize};

/// One open dialog and what it was opened for (`tag` is the owner's meaning).
pub(super) struct Open<T> {
    win: WindowId,
    pub tag: T,
    escape: bool,
}

pub(super) struct Dialogs<T> {
    open: Vec<Open<T>>,
    escape: bool,
}

impl<T> Default for Dialogs<T> {
    fn default() -> Self {
        Self { open: vec![], escape: true }
    }
}

/// GUI `101623ea`: sqrt(total glyph area) * aspect, then rewrap and use the measured extent.
fn wrap_width(gui: &mut Gui, body: &str) -> u32 {
    let (fonts, colors) = gui.text_parts();
    let flags = ao_gui::view::tvf::MULTILINE | ao_gui::view::tvf::WORD_WRAP;
    let flat = ao_gui::text::layout_text(fonts, colors, ao_gui::FontId::Normal, body, flags, None);
    let area = flat.lines.iter().flat_map(|line| &line.runs).map(|run| run.advance(fonts.font(ao_gui::FontId::Normal))).sum::<i32>() * 13;
    let width = (area as f32).sqrt() * 2.0;
    let wrapped = ao_gui::text::layout_text(fonts, colors, ao_gui::FontId::Normal, body, flags, Some(width as i32));
    wrapped.max_width.max(1) as u32 - 1
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn xml(gui: &mut Gui, body: &str, buttons: &[String]) -> String {
    let w = wrap_width(gui, body);
    // text borders (15, 5, 15, 20), button row with a leading spacer, 8 px left/right borders and a trailing spacer for a single button
    let mut row = String::from("<View view_layout=\"horizontal\" layout_borders=\"Rect(0,0,0,5)\"><HLayoutSpacer/>");
    for (i, b) in buttons.iter().enumerate() {
        row += &format!("<Button name=\"btn{i}\" label=\"{}\" layout_borders=\"Rect(8,0,8,0)\"/>", escape(b));
    }
    if buttons.len() == 1 {
        row += "<HLayoutSpacer/>";
    }
    row += "</View>";
    format!(
        "<root><View view_layout=\"vertical\"><TextView name=\"text\" feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP\" value=\"{}\" min_size=\"Point({w},-1)\" max_size=\"Point({w},-1)\" layout_borders=\"Rect(15,5,15,20)\"/>{row}</View></root>",
        escape(body)
    )
}

pub(super) fn open(gui: &mut Gui, screen: (u32, u32), name: &str, body: &str, buttons: &[String]) -> Option<WindowId> {
    let source = xml(gui, body, buttons);
    let win = gui.open_framed_window_xml(name, &source, (0, 0), WindowSize::Preferred).ok()?;
    gui.set_text(win, "text", body);
    gui.relayout_window(win);
    let (w, h) = gui.outer_size(win);
    gui.set_window_pos(win, ((screen.0 as i32 - w as i32) / 2, (screen.1 as i32 - h as i32) / 2));
    Some(win)
}

impl<T: Copy> Dialogs<T> {
    pub(super) fn configure_escape(&mut self, values: &super::dvalue::DValues) {
        self.escape = values.get_i64("esc_dialogs").unwrap_or(1) != 0;
    }

    pub(super) fn escape_enabled(&self) -> bool {
        self.escape
    }

    pub(super) fn go(&mut self, gui: &mut Gui, screen: (u32, u32), tag: T, body: &str, buttons: &[String]) {
        self.go_named(gui, screen, "DialogBox", tag, body, buttons);
    }

    pub(super) fn go_named(&mut self, gui: &mut Gui, screen: (u32, u32), name: &str, tag: T, body: &str, buttons: &[String]) {
        let Some(win) = open(gui, screen, name, body, buttons) else { return };
        self.open.push(Open { win, tag, escape: self.escape });
    }

    pub(super) fn is_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// The windows of the open dialogs (tests click their buttons).
    #[cfg(test)]
    pub(super) fn windows(&self) -> Vec<WindowId> {
        self.open.iter().map(|o| o.win).collect()
    }

    /// Replaces the body text of the open dialogs (`DialogBox_c` text view; the bind dialog shows the pressed key).
    pub(super) fn set_body(&self, gui: &mut Gui, body: &str) {
        for o in &self.open {
            gui.set_text(o.win, "text", body);
        }
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui) {
        for o in self.open.drain(..) {
            gui.close_window(o.win);
        }
    }

    fn finish(&mut self, gui: &mut Gui, win: WindowId, button: i32) -> Option<(T, i32)> {
        let i = self.open.iter().position(|o| o.win == win)?;
        let o = self.open.remove(i);
        gui.close_window(win);
        Some((o.tag, button))
    }

    /// `(belongs to a dialog, (tag, pressed button; -1 = Esc / close))`.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event) -> (bool, Option<(T, i32)>) {
        let own = |w: &WindowId| self.open.iter().any(|o| o.win == *w);
        match ev {
            Event::Clicked { window, view, .. } if own(window) => {
                let n = view.strip_prefix("btn").and_then(|n| n.parse().ok()).unwrap_or(0);
                (true, self.finish(gui, *window, n))
            }
            // `esc_dialogs` defaults to true: `DialogBox_c::SlotEscPressed` = `SlotSelected(-1)`
            Event::Escape { window } if own(window) && self.open.iter().any(|o| o.win == *window && o.escape) => (true, self.finish(gui, *window, -1)),
            Event::Escape { window } if own(window) => (true, None),
            Event::CloseRequested { window } if own(window) => (true, self.finish(gui, *window, -1)),
            Event::TextChanged { window, .. } | Event::LinkClicked { window, .. } if own(window) => (true, None),
            _ => (false, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dialog_escape_policy_is_captured_at_go() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut values = super::super::dvalue::DValues::new(&dir);
        let mut dialogs = Dialogs::default();
        values.set_i64("esc_dialogs", 0);
        dialogs.configure_escape(&values);
        dialogs.go(&mut gui, (800, 600), (), "Question", &["OK".into()]);
        let win = dialogs.windows()[0];
        values.set_i64("esc_dialogs", 1);
        dialogs.configure_escape(&values);
        assert_eq!(dialogs.event(&mut gui, &Event::Escape { window: win }), (true, None));
        assert!(dialogs.is_open());
        assert_eq!(dialogs.event(&mut gui, &Event::CloseRequested { window: win }), (true, Some(((), -1))));
        dialogs.go(&mut gui, (800, 600), (), "Question", &["OK".into()]);
        let win = dialogs.windows()[0];
        values.set_i64("esc_dialogs", 0);
        dialogs.configure_escape(&values);
        assert_eq!(dialogs.event(&mut gui, &Event::Escape { window: win }), (true, Some(((), -1))));
    }

    #[test]
    fn native_1012aa23_dialog_layout_has_no_implicit_default_button() {
        let dir = ao_gui::client_dir();
        let Ok(mut gui) = Gui::new(&dir, None) else { return };
        let body = "Do you really want to reset the skill Strength?";
        let mut dialogs = Dialogs::default();
        dialogs.go_named(&mut gui, (800, 600), "SkillReset", (), body, &["Yes".into(), "No".into()]);
        let win = dialogs.windows()[0];
        let text = gui.view_rect(win, "text").unwrap();
        let yes = gui.view_rect(win, "btn0").unwrap();
        let no = gui.view_rect(win, "btn1").unwrap();
        assert_eq!(gui.text(win, "text"), body);
        assert_eq!(yes.t, no.t);
        assert_eq!(no.l - yes.r - 1.0, 16.0); // two8px button borders, 1012ad5f
        assert_eq!(yes.t - text.b - 1.0, 20.0); // body bottom border
        let (x, y, w, h) = gui.window_outer_frame(win).unwrap();
        assert_eq!((x, y), ((800 - w as i32) / 2, (600 - h as i32) / 2));
        // 10156885 initializes default=null; 1012a2f6/1012ad5f/1012a3bf never installs one.
        assert!(gui.input(ao_gui::InputEvent::Key { key: ao_gui::Key::Enter, pressed: true, mods: Default::default() }).iter().all(|event| !matches!(event, Event::Clicked { .. })));
        assert!(dialogs.is_open());
        assert_eq!(dialogs.event(&mut gui, &Event::Clicked { window: win, view: "btn1".into(), item: None }), (true, Some(((), 1))));
    }
}
