//! `DialogBox_c` message / confirmation boxes and the `/afk` message dialog (`AFKMessageDialog_c`), GUI.dll.
//! Evidence: docs/chat/dialogs.md §2. The layouts are built as view XML like the other screens the original creates in code.

use ao_gui::{Event, Gui, WindowId, WindowSize};

/// What a dialog was opened for (decides what its answer does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `/messagebox` (`FUN_100b7a6f`): one OK button, nothing happens afterwards.
    MessageBox,
    /// GlobalSignals+0x184 -> GuiSystem_c::ShowMessageBox (GUI100304df/1002f852).
    Information,
    /// `/terminate` (GUI 0x100b345e): Yes (0) -> callback 0x100b33ef.
    Terminate,
    /// `/org leave` (`OrganizationGUIModule_c::LeaveOrg` 0x10052568): Yes (0) -> `LeaveOrgConfirmed` 0x10052032.
    OrgLeave,
    /// `/org disband` (`OpenDisbandDialog` 0x100520dd): Yes (0) -> `DisbandDialogClosed` 0x10051ffc.
    OrgDisband,
    /// GUI 0x10052978 / 0x1005204d: only Yes accepts the server's organisation invitation.
    OrgInvitation(ao_net::msg::Identity),
    /// GUI 0x100526e6 / 0x100520a4: promotion confirmation keeps the dialog's identity.
    OrgPromotion(ao_net::msg::Identity),
    /// GUI 0x1005221d / 0x10052076: both Yes and No/Escape send a tax answer.
    OrgLeavePayTax(ao_net::msg::Identity),
    /// GUI 0x100523d1 / 0x10052017: only Yes leaves following a tax change.
    OrgTaxChangeLeave,
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

impl Kind {
    /// GUI org callbacks 0x10052017..0x100520a4; No/Escape is a negative payment answer, not silence.
    pub fn org_answer(self, button: i32, target: ao_net::msg::Identity) -> Option<(u8, ao_net::msg::Identity)> {
        use ao_net::n3::textcmd::org;
        match self {
            Self::OrgInvitation(identity) if button == 0 => Some((0x0f, identity)),
            Self::OrgPromotion(identity) if button == 0 => Some((org::PROMOTE, identity)),
            Self::OrgLeavePayTax(identity) => Some((if button == 0 { 0x1d } else { 0x1e }, identity)),
            Self::OrgTaxChangeLeave if button == 0 => Some((org::LEAVE_CONFIRMED, target)),
            _ => None,
        }
    }
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

/// The original organisation module uses ordinary `DialogBox_c` views, not an organisation roster.
pub fn org_spec(request: &ao_net::n3::textcmd::OrgDialog, name: Option<&str>, texts: &ao_formats::screens::TextDb) -> Option<Spec> {
    use ao_net::n3::textcmd::OrgDialog;
    use super::log::{ldb_format, Arg};
    let nonzero = |id: &ao_net::msg::Identity| id.kind != 0 || id.instance != 0;
    let (kind, category, key, args) = match request {
        // 0x10052b59 passes the original Org_Invite formatter to DialogBox_c; the tax suffix built at
        // 0x10052a29..0x10052ad2 is not passed to the dialog in this client.
        OrgDialog::Invitation { identity, name, .. } if nonzero(identity) =>
            (Kind::OrgInvitation(*identity), 501, "Org_Invite", vec![Arg::S(name.clone())]),
        OrgDialog::LeavePayTax { identity, tax } if nonzero(identity) && (1..1_000_000_000).contains(tax) =>
            (Kind::OrgLeavePayTax(*identity), 501, "Org_LeaveAskPayTax", vec![Arg::N(*tax as i32)]),
        OrgDialog::TaxChangeLeave { tax } if (1..1_000_000_000).contains(tax) =>
            (Kind::OrgTaxChangeLeave, 501, "Org_TaxChangedAskLeave", vec![Arg::N(*tax as i32)]),
        OrgDialog::Promotion { identity } =>
            (Kind::OrgPromotion(*identity), 501, "ConfirmPromotion", vec![Arg::S(name?.to_owned())]),
        _ => return None,
    };
    Some(Spec {
        kind,
        body: ldb_format(&texts.by_key(category, key).unwrap_or_default(), &args),
        buttons: ["MsgBox_Yes", "MsgBox_No"].iter().map(|key| texts.by_key(10000, key).unwrap_or_default()).collect(),
        input: None,
    })
}

struct Open {
    win: WindowId,
    kind: Kind,
    escape: bool,
    /// Seconds since `Go` (AFK countdown).
    age: f32,
}

pub struct Dialogs {
    open: Vec<Open>,
    escape: bool,
}

impl Default for Dialogs {
    fn default() -> Self { Self { open: vec![], escape: true } }
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
        format!("<TextView name=\"{name}\" feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP\" min_size=\"Point({w},-1)\" max_size=\"Point({w},-1)\" layout_borders=\"{borders}\"/>")
    };
    match spec.kind {
        // (the bug report is a `DialogBox_c` with the text and two buttons, 0x1002aa28)
        // `DialogBoxView_c` 0x1012aa23: text borders (15, 5, 15, 20) = `_DAT_101b00e0 / _DAT_101a8b98 / _DAT_101b00e0 / _DAT_101b4e08`
        Kind::Information | Kind::MessageBox | Kind::Terminate | Kind::OrgLeave | Kind::OrgDisband | Kind::OrgInvitation(_) | Kind::OrgPromotion(_) | Kind::OrgLeavePayTax(_) | Kind::OrgTaxChangeLeave | Kind::BugReport | Kind::DuelReceived | Kind::DuelSent | Kind::StartPvp(_) => {
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
    pub fn configure_escape(&mut self, values: &super::super::dvalue::DValues) {
        self.escape = values.get_i64("esc_dialogs").unwrap_or(1) != 0;
    }

    pub fn esc_closes(&self) -> bool { self.open.iter().any(|o| o.escape) }
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        !self.open.is_empty()
    }
    /// InputConfig +0x1c8: editing the AFK reply does not clear AFK on keyboard/mouse activity.
    pub fn afk_input_active(&self) -> bool {
        self.open.iter().any(|o| o.kind == Kind::Afk)
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
        let name = if spec.kind == Kind::Information { "Information" } else if spec.kind == Kind::Terminate { "Terminate" } else { "DialogBox" };
        let Ok(win) = gui.open_framed_window_xml(name, &xml(spec), (0, 0), WindowSize::Preferred) else { return };
        gui.set_text(win, "text", &spec.body);
        if let Some(i) = &spec.input {
            gui.set_text(win, "input", i);
            gui.focus(win, "input");
        }
        gui.resize_window(win, WindowSize::Preferred);
        let (w, h) = gui.outer_size(win);
        gui.set_window_pos(win, ((screen.0 as i32 - w as i32) / 2, (screen.1 as i32 - h as i32) / 2));
        self.open.push(Open { win, kind: spec.kind, age: 0.0, escape: self.escape });
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
    fn organisation_answers_preserve_identity_and_payment_decline() {
        let id = ao_net::msg::Identity { kind: 50000, instance: 41 };
        let target = ao_net::msg::Identity { kind: 50000, instance: 77 };
        for (kind, code) in [(Kind::OrgInvitation(id), 15), (Kind::OrgPromotion(id), 10)] {
            assert_eq!(kind.org_answer(0, target), Some((code, id)));
            assert_eq!(kind.org_answer(1, target), None);
            assert_eq!(kind.org_answer(-1, target), None);
        }
        assert_eq!(Kind::OrgTaxChangeLeave.org_answer(0, target), Some((16, target)));
        assert_eq!(Kind::OrgTaxChangeLeave.org_answer(-1, target), None);
        for button in [0, 1, -1] {
            assert_eq!(Kind::OrgLeavePayTax(id).org_answer(button, target), Some((if button == 0 { 29 } else { 30 }, id)));
        }
    }

    #[test]
    fn organisation_requests_use_native_text_and_existing_dialog_view() {
        use ao_net::n3::textcmd::OrgDialog;
        let Some(mut gui) = rig() else { return };
        let texts = ao_formats::screens::TextDb::load(&ao_gui::client_dir()).unwrap();
        let id = ao_net::msg::Identity { kind: 50000, instance: 41 };
        let mut dialogs = Dialogs::default();
        for (request, key, name) in [
            (OrgDialog::Invitation { identity: id, name: "Test Org".into(), tax: 100 }, "Org_Invite", None),
            (OrgDialog::LeavePayTax { identity: id, tax: 100 }, "Org_LeaveAskPayTax", None),
            (OrgDialog::TaxChangeLeave { tax: 100 }, "Org_TaxChangedAskLeave", None),
            (OrgDialog::Promotion { identity: id }, "ConfirmPromotion", Some("Testy")),
        ] {
            let spec = org_spec(&request, name, &texts).unwrap();
            assert!(!spec.body.is_empty(), "{key}");
            assert_eq!(spec.buttons, ["MsgBox_Yes", "MsgBox_No"].map(|key| texts.by_key(10000, key).unwrap()));
            dialogs.go(&mut gui, (1280, 800), &spec);
            let win = dialogs.open.last().unwrap().win;
            assert_eq!(gui.text(win, "text"), spec.body);
            let (_, answer) = dialogs.event(&mut gui, &Event::Clicked { window: win, view: "btn0".into(), item: None });
            assert_eq!(answer.unwrap().kind, spec.kind);
        }
        assert!(org_spec(&OrgDialog::Promotion { identity: id }, None, &texts).is_none());
        for tax in [0, 1_000_000_000, u32::MAX] {
            assert!(org_spec(&OrgDialog::TaxChangeLeave { tax }, None, &texts).is_none());
        }
        assert!(org_spec(&OrgDialog::Invitation { identity: Default::default(), name: "Invalid".into(), tax: 0 }, None, &texts).is_none());
    }

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

#[cfg(test)]
mod escape_tests {
    use super::*;
    #[test]
    fn chat_dialog_escape_policy_is_captured_at_go() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut d = super::super::super::dvalue::DValues::new(&dir);
        let mut dialogs = Dialogs::default();
        let spec = Spec { kind: Kind::MessageBox, body: "Message".into(), buttons: vec!["OK".into()], input: None };
        d.set_i64("esc_dialogs", 0);
        dialogs.configure_escape(&d);
        dialogs.go(&mut gui, (800, 600), &spec);
        let win = dialogs.windows().next().unwrap();
        d.set_i64("esc_dialogs", 1);
        dialogs.configure_escape(&d);
        assert!(dialogs.event(&mut gui, &Event::Escape { window: win }).1.is_none());
        assert!(dialogs.is_open());
        assert_eq!(dialogs.event(&mut gui, &Event::CloseRequested { window: win }).1.unwrap().button, -1);
        dialogs.go(&mut gui, (800, 600), &spec);
        let win = dialogs.windows().next().unwrap();
        d.set_i64("esc_dialogs", 0);
        dialogs.configure_escape(&d);
        assert_eq!(dialogs.event(&mut gui, &Event::Escape { window: win }).1.unwrap().button, -1);
    }
}
