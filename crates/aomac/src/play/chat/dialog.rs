//! `DialogBox_c` message / confirmation boxes and the `/afk` message dialog (`AFKMessageDialog_c`), GUI.dll.
//! Evidence: docs/chat/dialogs.md §2. The layouts are built as view XML like the other screens the original creates in code.

use ao_gui::{Event, Gui, WindowId, WindowSize};

/// What a dialog was opened for (decides what its answer does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `/messagebox` (`FUN_100b7a6f`): one OK button, nothing happens afterwards.
    MessageBox,
    /// `/org leave` (`OrganizationGUIModule_c::LeaveOrg` 0x10052568): Yes (0) -> `LeaveOrgConfirmed` 0x10052032.
    OrgLeave,
    /// `/org disband` (`OpenDisbandDialog` 0x100520dd): Yes (0) -> `DisbandDialogClosed` 0x10051ffc.
    OrgDisband,
    /// Bare `/afk` (`FUN_10082a6f`): OK / Enter / the 30 s timeout (0) with the typed text, Esc (-1).
    Afk,
    /// `/bug` (`FlowControlModule_t::SetBugReportStringMessage` 0x1002aa28, dialog "BugReport"): OK (0) sends the report.
    BugReport,
    /// "DuelChallenge" window of `GuiSystem_c::DuelChallengeReceived` (GUI 0x1002fd9c): Accept (0) -> `N3Msg_Duel_Accept`, anything else -> `_Refuse`.
    DuelReceived,
    /// "DuelChallenge" window of `GuiSystem_c::DuelChallengeSent` (GUI 0x1002ff80): its only button (Cancel) or Esc -> `N3Msg_Duel_Refuse`.
    DuelSent,
    /// "StartPvP" dialog of `GuiSystem_c::StartPvPFightDialogue` (GUI 0x1002fa8e, action 0x7b): Yes (0) -> `N3Msg_StartPvP(target)` (the dialog's identity).
    StartPvp(ao_net::msg::Identity),
}

/// A decided dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub kind: Kind,
    /// Index of the pressed button, -1 = Esc (`DialogBox_c::SlotEscPressed`).
    pub button: i32,
    /// The edited text of the AFK dialog.
    pub text: String,
}

pub struct Spec {
    pub kind: Kind,
    /// HTML body.
    pub body: String,
    pub buttons: Vec<String>,
    /// AFK dialog: the prefilled input text.
    pub input: Option<String>,
}

struct Open {
    win: WindowId,
    kind: Kind,
    /// Seconds since `Go` (AFK countdown).
    age: f32,
}

#[derive(Default)]
pub struct Dialogs {
    open: Vec<Open>,
}

/// `AFKDialogView_c`: the dialog answers 0 once more than this many whole seconds have passed (`FUN_10082e78`; the text shows `30 - elapsed`).
const AFK_SECONDS: u32 = 30;

/// Wrap width of the body: `TextRenderer_c::SetAspectRatio(2.0)` (`_DAT_101ae17c`) asks for a text block twice as wide as high; the
/// exact algorithm is not decoded (**GUESS**): about `sqrt(2 * chars * 7 * 14)` px clamped to 160..=520.
fn wrap_width(body: &str) -> u32 {
    let plain = body.chars().count() as f32;
    ((2.0 * plain * 7.0 * 14.0).sqrt() as u32).clamp(160, 520)
}

fn button_row(buttons: &[String]) -> String {
    // `DialogButtons_c::_Initialize` 0x1012ad5f: leading spacer, every button with 8 px left/right borders (`_DAT_101b0ed8`), a trailing spacer
    // when there is exactly one button; the row has a 5 px bottom border (`_DAT_101a8b98`)
    let mut x = String::from("<View view_layout=\"horizontal\" layout_borders=\"Rect(0,0,0,5)\"><HLayoutSpacer/>");
    for (i, b) in buttons.iter().enumerate() {
        x += &format!("<Button name=\"btn{i}\" label=\"{}\" layout_borders=\"Rect(8,0,8,0)\"/>", xml_escape(b));
    }
    if buttons.len() == 1 {
        x += "<HLayoutSpacer/>";
    }
    x + "</View>"
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// View XML of a dialog.
fn xml(spec: &Spec) -> String {
    let w = wrap_width(&spec.body);
    let text = |name: &str, borders: &str| {
        format!("<TextView name=\"{name}\" feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP\" min_size=\"Point({w},-1)\" max_size=\"Point({w},16000)\" layout_borders=\"{borders}\"/>")
    };
    match spec.kind {
        // (the bug report is a `DialogBox_c` with the text and two buttons, 0x1002aa28)
        // `DialogBoxView_c` 0x1012aa23: text borders (15, 5, 15, 20) = `_DAT_101b00e0 / _DAT_101a8b98 / _DAT_101b00e0 / _DAT_101b4e08`
        Kind::MessageBox | Kind::OrgLeave | Kind::OrgDisband | Kind::BugReport | Kind::DuelReceived | Kind::DuelSent | Kind::StartPvp(_) => {
            format!("<root><View view_layout=\"vertical\">{}{}</View></root>", text("text", "Rect(15,5,15,20)"), button_row(&spec.buttons))
        }
        // `FUN_10082a6f`: form borders (15, 10, 15, 15) (`_DAT_101b00e0 / _DAT_101a98e4`), body bottom border 10, countdown, input
        Kind::Afk => format!(
            "<root><View view_layout=\"vertical\" layout_borders=\"Rect(15,10,15,15)\">{}<TextView name=\"countdown\" value=\"{AFK_SECONDS}\" layout_borders=\"Rect(0,0,0,10)\"/><TextInputView name=\"input\" value=\"\" min_size=\"Point({w},-1)\" max_size=\"Point(16000,-1)\"/>{}</View></root>",
            text("text", "Rect(0,0,0,10)"),
            button_row(&spec.buttons)
        ),
    }
}

impl Dialogs {
    pub fn is_open(&self) -> bool {
        !self.open.is_empty()
    }

    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.open.iter().map(|o| o.win)
    }

    pub fn close_all(&mut self, gui: &mut Gui) {
        for o in self.open.drain(..) {
            gui.close_window(o.win);
        }
    }

    /// `GuiSystem_c::CloseDuelWindows` [GUI 0x1002f833]: `FindWindowName("DuelChallenge")` (the first of the received / sent dialogs) is closed
    /// without an answer.
    pub fn close_duel(&mut self, gui: &mut Gui) {
        if let Some(i) = self.open.iter().position(|o| matches!(o.kind, Kind::DuelReceived | Kind::DuelSent)) {
            gui.close_window(self.open.remove(i).win);
        }
    }

    /// `DialogBox_c::Go` + `Window::MoveToCenter`.
    pub fn go(&mut self, gui: &mut Gui, screen: (u32, u32), spec: &Spec) {
        let Ok(win) = gui.open_framed_window_xml("DialogBox", &xml(spec), (0, 0), WindowSize::Preferred) else { return };
        gui.set_text(win, "text", &spec.body);
        if let Some(i) = &spec.input {
            gui.set_text(win, "input", i);
            gui.focus(win, "input");
        }
        gui.set_default_button(win, "btn0");
        gui.relayout_window(win);
        let (w, h) = gui.outer_size(win);
        gui.set_window_pos(win, ((screen.0 as i32 - w as i32) / 2, (screen.1 as i32 - h as i32) / 2));
        self.open.push(Open { win, kind: spec.kind, age: 0.0 });
    }

    fn finish(&mut self, gui: &mut Gui, win: WindowId, button: i32) -> Option<Answer> {
        let i = self.open.iter().position(|o| o.win == win)?;
        let o = self.open.remove(i);
        let text = if o.kind == Kind::Afk { gui.text(win, "input") } else { String::new() };
        gui.close_window(win);
        Some(Answer { kind: o.kind, button, text })
    }

    /// The AFK countdown (`FUN_10082e78`, per frame).
    pub fn update(&mut self, gui: &mut Gui, dt: f32) -> Vec<Answer> {
        let mut done = vec![];
        for i in 0..self.open.len() {
            if self.open[i].kind != Kind::Afk {
                continue;
            }
            self.open[i].age += dt;
            let whole = self.open[i].age as u32;
            if whole > AFK_SECONDS {
                done.push(self.open[i].win);
            } else {
                gui.set_text(self.open[i].win, "countdown", &(AFK_SECONDS - whole).to_string());
            }
        }
        done.into_iter().filter_map(|w| self.finish(gui, w, 0)).collect()
    }

    /// `(belongs to a dialog, its answer)`.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event) -> (bool, Option<Answer>) {
        let own = |w: &WindowId| self.open.iter().any(|o| o.win == *w);
        match ev {
            Event::Clicked { window, view, .. } if own(window) => {
                let n = view.strip_prefix("btn").and_then(|n| n.parse().ok()).unwrap_or(0);
                (true, self.finish(gui, *window, n))
            }
            // `FUN_10082f50`: Enter in the AFK input = OK
            Event::EnterPressed { window, .. } if own(window) => (true, self.finish(gui, *window, 0)),
            // `esc_dialogs` defaults to true (LoginPrefs.xml): `SlotEscPressed` = `SlotSelected(-1)`
            Event::Escape { window } if own(window) => (true, self.finish(gui, *window, -1)),
            Event::CloseRequested { window } if own(window) => (true, self.finish(gui, *window, -1)),
            Event::TextChanged { window, .. } | Event::LinkClicked { window, .. } if own(window) => (true, None),
            _ => (false, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> Option<Gui> {
        let client = ao_gui::client_dir();
        client.join("cd_image/gui").exists().then(|| Gui::new(&client, None).unwrap())
    }

    fn spec(kind: Kind, buttons: &[&str]) -> Spec {
        Spec { kind, body: "Do you really want to leave the organization?".into(), buttons: buttons.iter().map(|s| s.to_string()).collect(), input: (kind == Kind::Afk).then(|| "I am away".into()) }
    }

    #[test]
    fn wrap_width_is_clamped() {
        assert_eq!(wrap_width("x"), 160);
        assert_eq!(wrap_width(&"x".repeat(10_000)), 520);
    }

    #[test]
    fn xml_has_one_button_row_with_spacers() {
        let x = xml(&spec(Kind::MessageBox, &["OK"]));
        assert_eq!(x.matches("<HLayoutSpacer/>").count(), 2);
        let x = xml(&spec(Kind::OrgLeave, &["Yes", "No"]));
        assert_eq!((x.matches("<HLayoutSpacer/>").count(), x.matches("<Button").count()), (1, 2));
        assert!(xml(&spec(Kind::MessageBox, &["A&B"])).contains("A&amp;B"));
    }

    #[test]
    fn yes_no_escape_and_buttons_answer() {
        let Some(mut gui) = rig() else { return };
        let mut d = Dialogs::default();
        d.go(&mut gui, (1280, 800), &spec(Kind::OrgLeave, &["Yes", "No"]));
        let w = d.open[0].win;
        assert!(gui.text(w, "text").contains("really"));
        let (hit, a) = d.event(&mut gui, &Event::Clicked { window: w, view: "btn1".into(), item: None });
        assert!(hit);
        assert_eq!(a, Some(Answer { kind: Kind::OrgLeave, button: 1, text: String::new() }));
        assert!(!d.is_open());
        d.go(&mut gui, (1280, 800), &spec(Kind::OrgDisband, &["Yes", "No"]));
        let w = d.open[0].win;
        let (_, a) = d.event(&mut gui, &Event::Escape { window: w });
        assert_eq!(a.map(|a| a.button), Some(-1));
    }

    /// The duel challenge dialogs (GUI 0x1002fd9c / 0x1002ff80): Accept = 0, Reject = 1, Esc = -1; `CloseDuelWindows` closes one without an answer.
    #[test]
    fn duel_dialogs_answer_and_close() {
        let Some(mut gui) = rig() else { return };
        let mut d = Dialogs::default();
        d.go(&mut gui, (1280, 800), &spec(Kind::DuelReceived, &["Accept", "Reject"]));
        let w = d.open[0].win;
        let (_, a) = d.event(&mut gui, &Event::Clicked { window: w, view: "btn0".into(), item: None });
        assert_eq!(a.map(|a| (a.kind, a.button)), Some((Kind::DuelReceived, 0)));
        d.go(&mut gui, (1280, 800), &spec(Kind::DuelSent, &["Cancel"]));
        d.go(&mut gui, (1280, 800), &spec(Kind::OrgLeave, &["Yes", "No"]));
        d.close_duel(&mut gui);
        assert_eq!(d.open.iter().map(|o| o.kind).collect::<Vec<_>>(), [Kind::OrgLeave]);
        d.close_duel(&mut gui);
        assert_eq!(d.open.len(), 1);
    }

    #[test]
    fn afk_dialog_counts_down_and_answers_with_the_input() {
        let Some(mut gui) = rig() else { return };
        let mut d = Dialogs::default();
        d.go(&mut gui, (1280, 800), &spec(Kind::Afk, &["Ok"]));
        let w = d.open[0].win;
        assert_eq!(gui.text(w, "input"), "I am away");
        assert!(d.update(&mut gui, 4.5).is_empty());
        assert_eq!(gui.text(w, "countdown"), "26");
        // more than 30 whole seconds: answers 0 with the typed text (`FUN_10082e78`)
        assert!(d.update(&mut gui, 20.0).is_empty());
        let a = d.update(&mut gui, 7.0);
        assert_eq!(a, [Answer { kind: Kind::Afk, button: 0, text: "I am away".into() }]);
        assert!(!d.is_open());
        d.go(&mut gui, (1280, 800), &spec(Kind::Afk, &["Ok"]));
        let w = d.open[0].win;
        let (_, a) = d.event(&mut gui, &Event::EnterPressed { window: w, view: "input".into() });
        assert_eq!(a.map(|a| a.button), Some(0));
    }
}
