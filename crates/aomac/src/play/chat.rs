//! In-game chat hub: chat-server session ([`net`]), zone chat ([`zone`]), input commands ([`cmd`]), the windows ([`win`]).
//! Evidence: docs/chat/*.md. The hub is the `ChatGUIModule_c` equivalent: it routes everything into the windows and
//! performs what an input line asks for.

mod cmd;
mod dialog;
mod filter;
mod info;
mod line;
mod macros;
mod log;
mod net;
mod social;
mod social_hub;
mod social_win;
pub(super) mod win;
mod voice;
mod zone;
mod zonecmd;

pub(super) use cmd::WindowOp;

use super::zone::Zone;
use ao_formats::screens::TextDb;
use ao_gui::{Event, Gui, InputEvent};
use ao_net::frame::{Frame, PT_N3, PT_SYSTEM};
use ao_net::msg::Identity;
use ao_net::n3::{self, N3};
use anyhow::Result;
use cmd::{ChatAction, CmdCtx, GroupInfo, GroupKind, Target};
use line::{ChatKind, ChatLine};
use net::Out;
use std::collections::HashSet;
use win::{ChatWindows, WinOut, G_VICINITY, LOCAL_GROUPS};

/// Home page of `BrowserWindow_c` mode 3 ("Petition"): GUI.dll's URL table 0x101c32c8.. lists shop, market, **report.project-rk.com** (mode 3), daily login in
/// the order of the modes 1..4 (docs/chat/dialogs.md §6).
const PETITION_URL: &str = "https://report.project-rk.com/";

/// Identity kind of characters (`SimpleChar_t`).
const CHAR_KIND: i32 = 0xC350;

/// Chat commands that act on the game, not on the chat (run by the combat layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GameAction {
    /// `/<emote>`, `/emote <name>`: `N3Msg_DoSocialAction(id)`.
    Social(u32),
    /// `/assist`.
    Assist,
    /// `/camp`: AFCM 0x134 (`StartQuitToLoginMessage`, GUI 0x10027c74): camp, then back to the login screen.
    Camp,
    /// `/selectself`: AFCM 0x1e / 0x126 with the own id (`TargetingModule_t::SetTargetMessage`: `SetTarget(own, false)`).
    SelectSelf,
    /// `/duel`, `/petduel` (GUI 0x100b8a10 / 0x100b8789) and the answers of the duel dialogs: run by the combat layer (`combat::duel`).
    Duel { pet: bool, op: ao_net::n3::action::duel::Op },
    /// Yes in the "StartPvP" dialog (`GuiSystem_c::StartPvPFightResult`, GUI 0x1002f789): `N3Msg_StartPvP(target)`.
    StartPvp(ao_net::msg::Identity),
}

/// Results of chat commands that other layers apply (taken once per frame by the flow with [`Chat::take_requests`]).
#[derive(Default)]
pub(super) struct Requests {
    /// `/waypoint`: playfield and world X/Z of the map marker (`GlobalSignals+0x158`, `HudMap::set_mission`).
    pub waypoint: Option<(u32, [f32; 2])>,
    /// Heard voice messages: sound file below `cd_image/sound` (`PlayPlayerFX`).
    pub sounds: Vec<String>,
    /// `/macro`: the macro just created; the shortcut bar starts dragging it (`FUN_100d82d4`).
    pub macro_drag: Option<macros::Macro>,
}

/// DValues of the voice effects (`VoiceSndFx*`, defaults of `LoginPrefs.xml` / `CharPrefs.xml`); the generic DValue registry sets them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VoicePrefs {
    /// `VoiceSndFxType` 0..=3.
    pub fx_type: i32,
    pub hear_vicinity: bool,
    pub hear_guild: bool,
    pub hear_team: bool,
}

impl Default for VoicePrefs {
    fn default() -> Self {
        Self { fx_type: 0, hear_vicinity: false, hear_guild: false, hear_team: true }
    }
}

pub(super) struct Chat {
    /// Opened when the world appears; everything that arrives earlier (the MOTD comes during loading) waits in `backlog`.
    win: Option<ChatWindows>,
    backlog: Vec<Back>,
    net: net::ChatNet,
    /// `ChatGUIModule_c::SetAFK` message while AFK.
    afk: Option<String>,
    /// Head of the reply list: sender of the last tell.
    last_tell_from: Option<String>,
    /// `IgnoreSystem_t` (local, not persisted yet).
    ignored: HashSet<u32>,
    /// Zone frames queued by input lines (the flow drains them into the session).
    outbox: Vec<Frame>,
    /// Running pet scripts (`NpcHolder_t::m_pScripts`, Gamecode 0x10052230 / 0x100522c9); each frame advances them.
    pet_scripts: Vec<ao_net::n3::textcmd::PetScript>,
    /// Actions of the game layer (`/<emote>`, `/assist`): taken by the flow ([`Chat::take_game`]).
    game: Vec<GameAction>,
    /// Text event to drop: the character of the key that opened the input bar.
    swallow: Option<String>,
    /// `s_nCommandRefCntr` of the social commands sent.
    social_counter: i32,
    own_id: u32,
    /// `InfoView_c` (`/help`, `/showfile`, `/tipoftheday`, `text://` links).
    info: info::InfoView,
    /// `DialogBox_c` boxes and the `/afk` message dialog.
    dialogs: dialog::Dialogs,
    /// DValue `CurrentTipOfTheDay` (CharPrefs.xml default -1).
    tip: i32,
    /// `/open` `/close` `/toggle` of the HUD windows: taken by the flow ([`Chat::take_windows`]).
    windows: Vec<(&'static str, cmd::WindowOp)>,
    /// `/option` `/dvalue` `/viewdist` ... token lists, run by the flow on the DValue store ([`Chat::take_dvalue_cmds`]).
    dvalue_cmds: Vec<Vec<String>>,
    quit: bool,
    screen: (u32, u32),
    /// Buddy list, tell windows, private groups, LFT (docs/chat/social.md).
    social: social::Social,
    swin: social_win::SocialWin,
    /// Lines of each tell partner's window (kept while it is closed).
    tell_log: std::collections::HashMap<u32, Vec<String>>,
    /// Chat windows ticked in the invitation dialog, by group owner id (applied when the group joins).
    pg_windows: std::collections::HashMap<u32, Vec<String>>,
    /// `ChatFilterEnabled` / `ChatFilterRules` (`/filter`).
    filter: filter::FilterState,
    /// `TextMacroSystem_t` (`TextMacro.bin`).
    macros: macros::TextMacros,
    voice_prefs: VoicePrefs,
    voice_throttle: voice::Throttle,
    /// Own stat `Expansion` (0x185), kept from the zone frames (the voice feature needs bit 0).
    expansion: i32,
    /// Output group of the window the last line was typed in (`FUN_1009a26c`).
    last_out_group: Option<u64>,
    /// Text of the open bug report dialog.
    bug: Option<String>,
    requests: Requests,
}

enum Back {
    Line(ChatLine, Option<String>),
    Msg(line::ChatMsg),
    Group(u64, String),
    Ungroup(u64),
}

fn ident_id(g: &str) -> Option<u64> {
    u64::from_str_radix(g.trim_matches('#'), 16).ok()
}

impl Chat {
    /// Created at the zone hand-off (the chat connection starts with the zone's 0x43 message); windows come with [`open`](Self::open).
    pub fn new() -> Self {
        let mut net = net::ChatNet::default();
        net.set_trace(std::env::var_os("AOMAC_CHAT_TRACE").map(Into::into));
        Self { win: None, backlog: vec![], net, afk: None, last_tell_from: None, ignored: HashSet::new(), outbox: vec![], pet_scripts: vec![], game: vec![], swallow: None, social_counter: 0, own_id: 0, info: info::InfoView::new(&ao_gui::client_dir()), dialogs: dialog::Dialogs::default(), tip: -1, windows: vec![], dvalue_cmds: vec![], quit: false, screen: (0, 0), social: social::Social::default(), swin: social_win::SocialWin::new((0, 0)), tell_log: Default::default(), pg_windows: Default::default(), filter: filter::FilterState::load(), macros: macros::TextMacros::open(), voice_prefs: VoicePrefs::default(), voice_throttle: voice::Throttle::default(), expansion: 0, last_out_group: None, bug: None, requests: Requests::default() }
    }

    /// The chat windows (`ChatGUIModule_c::Initialize`), once the world is shown.
    pub fn open(&mut self, gui: &mut Gui, screen: (u32, u32)) -> Result<()> {
        self.screen = screen;
        if self.win.is_none() {
            let mut w = ChatWindows::new(gui, screen)?;
            // HUD footprint (wings 190/65 px, shortcut bar 38 px at 1280x828, measured from the HUD art): only the template default windows avoid it
            w.set_reserved(gui, win::Reserved { left: 190, right: 65, bottom: 38 });
            self.win = Some(w);
            for b in std::mem::take(&mut self.backlog) {
                match b {
                    Back::Line(l, h) => self.line_to(gui, l, h.as_deref()),
                    Back::Msg(m) => self.msg(gui, m),
                    Back::Group(g, n) => self.group(g, n),
                    Back::Ungroup(g) => self.ungroup(g),
                }
            }
        }
        Ok(())
    }

    fn line(&mut self, gui: &mut Gui, l: ChatLine) {
        self.line_to(gui, l, None);
    }
    /// `hint` = name of the group (window class) the line belongs to.
    fn line_to(&mut self, gui: &mut Gui, l: ChatLine, hint: Option<&str>) {
        match &mut self.win {
            Some(w) => w.push(gui, &l, hint),
            None => self.backlog.push(Back::Line(l, hint.map(str::to_owned))),
        }
    }
    fn class_name(&self, class: u64) -> Option<String> {
        LOCAL_GROUPS.iter().find(|g| g.0 == class).map(|g| g.1.to_owned()).or_else(|| self.net.groups.get(&class).cloned())
    }
    fn msg(&mut self, gui: &mut Gui, m: line::ChatMsg) {
        // `FUN_10084f9e`: a message matching a rule of an enabled filter never reaches the windows
        if self.filter.drops(&m.text) {
            return;
        }
        match &mut self.win {
            Some(w) => w.push_msg(gui, &m),
            None => self.backlog.push(Back::Msg(m)),
        }
    }
    fn group(&mut self, g: u64, n: String) {
        match &mut self.win {
            Some(w) => w.add_group(g, &n),
            None => self.backlog.push(Back::Group(g, n)),
        }
    }
    fn ungroup(&mut self, g: u64) {
        match &mut self.win {
            Some(w) => w.remove_group(g),
            None => self.backlog.push(Back::Ungroup(g)),
        }
    }

    /// `cPlayerName` / `cPlayerPasswd`: kept in memory for the chat login, like the original's globals.
    pub fn set_credentials(&mut self, user: &str, password: &str) {
        self.net.set_credentials(user, password);
    }

    pub fn resize(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        self.swin.resize(screen);
        self.screen = screen;
        if let Some(w) = &mut self.win {
            w.resize(gui, screen);
        }
    }

    /// `IgnoreSystem_t::IsCharacterIgnored(id)`.
    pub fn is_ignored(&self, id: u32) -> bool {
        self.ignored.contains(&id)
    }

    /// `FUN_10058b00(key)` [GC 0x10058b00]: the `Feedback_*` text (category 110) of the client character as a plain System-window line
    /// (`FUN_10012b05(0, text, 0)`: no colour code, so no `<font>`).
    pub fn feedback(&mut self, gui: &mut Gui, key: &str, texts: &TextDb) {
        if let Some(t) = texts.by_key(110, key) {
            self.line_to(gui, ChatLine::new(ChatKind::System, log::window_html("", &t)), Some("System"));
        }
    }

    /// `FUN_1009b37f(text, 0x51)`: a red `CCChatCmdFeedbackError` line of the chat window (usage / error text of a chat command, GUI 0x1009b37f).
    pub fn cmd_error(&mut self, gui: &mut Gui, text: &str) {
        self.line(gui, ChatLine::new(ChatKind::Other("CCChatCmdFeedbackError"), format!("<div><font color=CCChatCmdFeedbackError>{text}</font></div>")));
    }

    /// `LDBformat(GetText(110, key)).Feed(name).Dump()` + `FUN_10012b05(0, text, 0)` (the lines of `FUN_1005b821` / `FUN_1005c514`, e.g. `Feedback_DuelChallenge`):
    /// a `Feedback_*` text with the name of a dynel fed in, as a plain System-window line.
    pub fn feedback_named(&mut self, gui: &mut Gui, key: &str, name: &str, texts: &TextDb) {
        if let Some(t) = texts.by_key(110, key) {
            let t = log::ldb_format(&t, &[log::Arg::S(name.to_string())]);
            self.line_to(gui, ChatLine::new(ChatKind::System, log::window_html("", &t)), Some("System"));
        }
    }

    /// `GuiSystem_c::DuelChallengeReceived` / `DuelChallengeSent` [GUI 0x1002fd9c / 0x1002ff80]: the "Duel Challenge" dialog (text `Feedback_DuelChallenge`
    /// / `Feedback_DuelChallengeSent` of category 110 fed with the name; buttons `MsgBox_Accept` + `MsgBox_Reject`, resp. `MsgBox_Cancel`, category 10000).
    pub fn duel_dialog(&mut self, gui: &mut Gui, sent: bool, name: &str, texts: &TextDb) {
        let (kind, key, buttons) = if sent {
            (dialog::Kind::DuelSent, "Feedback_DuelChallengeSent", &["MsgBox_Cancel"][..])
        } else {
            (dialog::Kind::DuelReceived, "Feedback_DuelChallenge", &["MsgBox_Accept", "MsgBox_Reject"][..])
        };
        let body = log::ldb_format(&texts.by_key(110, key).unwrap_or_default(), &[log::Arg::S(name.to_string())]);
        let buttons = buttons.iter().map(|b| texts.by_key(10000, b).unwrap_or_default()).collect();
        self.dialog(gui, kind, body, buttons, None);
    }

    /// `GuiSystem_c::CloseDuelWindows` [GUI 0x1002f833].
    pub fn close_duel_dialog(&mut self, gui: &mut Gui) {
        self.dialogs.close_duel(gui);
    }

    /// `GuiSystem_c::StartPvPFightDialogue(text, target)` [GUI 0x1002fa8e] (action 0x7b): the "StartPvP" dialog with the server's question (`Combat_PvPTargetLvl`
    /// / `Combat_PvPTargetLvlTeam`, LDB category 101, shown as it is) and the buttons `MsgBox_Yes` / `MsgBox_No` (category 10000).
    pub fn pvp_dialog(&mut self, gui: &mut Gui, team: bool, target: ao_net::msg::Identity, texts: &TextDb) {
        let key = if team { "Combat_PvPTargetLvlTeam" } else { "Combat_PvPTargetLvl" };
        let body = texts.by_key(101, key).unwrap_or_default();
        let buttons = ["MsgBox_Yes", "MsgBox_No"].iter().map(|b| texts.by_key(10000, b).unwrap_or_default()).collect();
        self.dialog(gui, dialog::Kind::StartPvp(target), body, buttons, None);
    }

    /// `GlobalSignals+0x17c (0, text, colorCode)` [GC `FUN_10012b05`]: a coloured line of the System window (`FlowControlModule_t`
    /// `TeleportStartedMessage` / `TeleportEndedMessage` emit it with code 12 `CCRed`, docs/zone/world.md §10.2).
    pub fn system_line(&mut self, gui: &mut Gui, text: &str, code: u32) {
        self.line_to(gui, ChatLine::new(ChatKind::System, log::window_html(log::color_name(code), text)), Some("System"));
    }

    /// `FlowControlModule_t` logout texts (LDB category 200 `ClosingClient` / `LogoutStarted` (fed 30) / `TimedLogoutAborted`): GlobalSignals+0x17c, code 12.
    pub fn logout_line(&mut self, gui: &mut Gui, key: &str, secs: Option<i32>, texts: &TextDb) {
        let t = texts.by_key(200, key).unwrap_or_default();
        let t = log::ldb_format(&t, &secs.map(log::Arg::N).into_iter().collect::<Vec<_>>());
        self.system_line(gui, &t, 12);
    }

    /// Live-test hook: drop the chat-server connection (see [`net::ChatNet::drop_connection`]).
    #[cfg(test)]
    pub fn drop_connection(&self) {
        self.net.drop_connection();
    }

    /// `/waypoint`, heard voices, `/macro` results for the flow.
    pub fn take_requests(&mut self) -> Requests {
        std::mem::take(&mut self.requests)
    }

    /// Voice DValues from the registry.
    pub fn set_voice_prefs(&mut self, p: VoicePrefs) {
        self.voice_prefs = p;
    }

    /// Chat window DValues (`ChatShowOGrpInTitleBar`, `ChatShowOGrpInInputBar`, `ChatFont*`) from the registry, applied live.
    pub fn set_window_prefs(&mut self, gui: &mut Gui, p: &win::WinPrefs) {
        if let Some(w) = &mut self.win {
            w.set_prefs(gui, p);
        }
    }

    pub fn take_game(&mut self) -> Vec<GameAction> {
        std::mem::take(&mut self.game)
    }

    /// `/open` `/close` `/toggle` requests for the HUD windows: `(DValue name, op)`, applied by the flow (`WindowKind::from_dvalue`).
    pub fn take_windows(&mut self) -> Vec<(&'static str, cmd::WindowOp)> {
        std::mem::take(&mut self.windows)
    }

    /// Token lists of `/option` `/setoption` `/dvalue` `/chardist` `/viewdist` `/char&viewdist`: the flow runs them on the HUD's
    /// [`DValues`](super::dvalue::DValues) and answers with [`Chat::dvalue_feedback`].
    pub fn take_dvalue_cmds(&mut self) -> Vec<Vec<String>> {
        std::mem::take(&mut self.dvalue_cmds)
    }

    /// The feedback lines of those commands (`FUN_1009b37f(text, 0x51 | 0x52)`): the texts are the binary's HTML.
    pub fn dvalue_feedback(&mut self, gui: &mut Gui, outs: Vec<super::dvalue::Out>) {
        for o in outs {
            let name = if o.error { "CCChatCmdFeedbackError" } else { "CCChatCmdFeedbackInfo" };
            self.line(gui, ChatLine::new(ChatKind::Other(name), format!("<div><font color={name}>{}</font></div>", o.text)));
        }
    }

    /// `/quit` (`StartQuitToSystemMessage`, GUI 0x10029a0d): taken by the flow, which ends the session.
    pub fn take_quit(&mut self) -> bool {
        std::mem::take(&mut self.quit)
    }

    /// True while a dialog box or the InfoView is open: Esc closes those first (`DialogBox_c::SlotEscPressed`, `esc_dialogs` / `esc_infoview` default true).
    pub fn esc_closes(&self) -> bool {
        self.dialogs.is_open() || self.info.window().is_some()
    }

    /// Esc key: closes the topmost dialog / the InfoView; `true` when something was closed.
    pub fn escape(&mut self, gui: &mut Gui, zone: &Zone, texts: &TextDb) -> bool {
        if let Some(w) = self.dialogs.windows().last() {
            let (_, a) = self.dialogs.event(gui, &Event::Escape { window: w });
            if let Some(a) = a {
                self.answer(gui, a, Some(zone), texts);
            }
            return true;
        }
        if self.info.window().is_some() {
            self.info.close(gui);
            return true;
        }
        false
    }

    /// Frames to send to the zone server.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// One zone frame: 0x43 (chat server list) connects; N3 chat messages become lines.
    pub fn on_zone_frame(&mut self, gui: &mut Gui, f: &Frame, zone: &Zone, texts: &TextDb) {
        self.own_id = zone.char_id;
        self.expansion = zone.stat(0x185).unwrap_or(0);
        match f.ptype {
            PT_SYSTEM => self.net.on_system_frame(f, zone.char_id),
            PT_N3 => {
                let Ok(m) = n3::decode(f) else { return };
                if let N3::Chat(c) = &m.body {
                    let ctx = zone::ZoneChatCtx {
                        texts,
                        name_of: &|id| zone.dynels.get(&(id as i32)).map(|d| d.name.clone()),
                        header_is_char: m.header.target.kind == CHAR_KIND,
                        target: zone.target.map(|t| t as u32),
                        fighting: None,
                        mouse: None,
                    };
                    for t in zone::routed(c, &ctx) {
                        let hint = self.class_name(t.channel as u64);
                        self.line_to(gui, t.line, hint.as_deref());
                    }
                    return;
                }
                // combat / stat / level feedback lines (docs/chat/log.md); evaluated before `zone.on_frame` applies the stat change
                self.log_events(gui, &log::events(&m), zone, texts);
            }
            _ => {}
        }
    }

    /// The chat lines of game events (docs/chat/log.md).
    fn log_events(&mut self, gui: &mut Gui, events: &[log::LogEvent], zone: &Zone, texts: &TextDb) {
        let own = Identity { kind: CHAR_KIND, instance: zone.char_id as i32 };
        let dyn_of = |id: Identity| (id.kind == CHAR_KIND).then(|| zone.dynels.get(&id.instance)).flatten();
        let filter = log::ChatFilter::default();
        let ctx = log::LogCtx {
            own,
            name: &|id| dyn_of(id).map(|d| d.name.clone()),
            text: &|c, i| texts.by_id(c, i),
            is_npc: &|id| dyn_of(id).is_some_and(|d| d.npc),
            is_own_pet: &|_| false,
            nano_name: &|_| None,
            stat: &|id, st| (id == own).then(|| zone.stat(st as u32)).flatten(),
            weapon: &|id, slot, special| (id.kind == CHAR_KIND).then(|| zone.world.arms.damage_type(id.instance, slot, special)).flatten(),
            filter: &filter,
        };
        for l in events.iter().flat_map(|ev| log::classify(ev, &ctx)) {
            let hint = self.class_name(l.class as u64);
            self.line_to(gui, l.line, hint.as_deref());
        }
    }

    /// `N3Msg_CastNanoSpell` of the own character: the "Executing Nano Program" line.
    pub fn cast_nano(&mut self, gui: &mut Gui, zone: &Zone, texts: &TextDb, nano_name: String) {
        let who = Identity { kind: CHAR_KIND, instance: zone.char_id as i32 };
        self.log_events(gui, &[log::LogEvent::CastNano { who, nano_name }], zone, texts);
    }

    /// Per frame: chat-server events into the windows, window fades.
    pub fn update(&mut self, gui: &mut Gui, dt: f32, texts: &TextDb) {
        self.pet_scripts.retain_mut(|s| match s.step(dt, self.own_id as i32) {
            Some(frame) => {
                self.outbox.extend(frame.map(|p| ao_net::n3::outgoing::n3_frame(0, self.own_id, p)));
                true
            }
            None => false,
        });
        for o in self.net.update(dt) {
            match o {
                Out::Msg(m) => {
                    if m.tell {
                        self.last_tell_from = Some(m.from_name.clone());
                        self.social_tell_in(gui, m.from_id, &m.from_name, &m.text, m.flags & 1 != 0, texts);
                    }
                    self.hear_voice(&m);
                    self.msg(gui, m);
                }
                Out::Line(l) => self.line(gui, l),
                Out::SystemFmt { text_id, args, .. } => {
                    // HandleSystemMessage-style local format: template = text id of category 20000, 'l' args are text ids of the same category
                    let a: Vec<log::Arg> = args
                        .into_iter()
                        .map(|a| match a {
                            ao_net::chat::FmtArg::Int(v) => log::Arg::N(v as i32),
                            ao_net::chat::FmtArg::Str(s) => log::Arg::S(s),
                            ao_net::chat::FmtArg::TextId(i) => log::Arg::S(texts.by_id(20000, i).unwrap_or_default()),
                        })
                        .collect();
                    let t = log::ldb_format(&texts.by_id(20000, text_id).unwrap_or_default(), &a);
                    self.line(gui, ChatLine::new(ChatKind::System, t));
                }
                Out::NameOp { op, id, name } => self.name_op(gui, op, id, &name, texts),
                Out::GroupAdd { group, name, .. } => self.group(group, name),
                Out::GroupRemove { group, .. } => self.ungroup(group),
                Out::Social(e) => self.social_event(gui, e, texts),
                Out::OpenTell { id, name } => self.social_open_tell_out(gui, id, &name, texts),
            }
        }
        self.social_update(gui, dt);
        for a in self.dialogs.update(gui, dt) {
            self.answer(gui, a, None, texts);
        }
        if let Some(w) = &mut self.win {
            w.update(gui, dt);
        }
    }

    /// Keys that open the input bar while no text field has the keyboard (`TextInputModule_t::StartChatMessage` /
    /// `StartChatCmdMessage` "/" / `StartChatReplyMessage` Shift+R, GUI 0x10021f18 / 0x10021fd0). `true` = consumed.
    pub fn input(&mut self, gui: &mut Gui, ev: &InputEvent, zone: &Zone, texts: &TextDb) -> bool {
        if let (InputEvent::Text(t), Some(s)) = (ev, &self.swallow) {
            // the text of the key press that just opened the bar (the viewer may send the Key and then its Text)
            let hit = t == s;
            self.swallow = None;
            if hit {
                return true;
            }
        }
        if gui.focused_view().is_some() || self.win.is_none() {
            return false;
        }
        let open = |this: &mut Self, gui: &mut Gui, prefill: Option<String>, swallow: &str| {
            match prefill {
                Some(p) => this.focus_text(gui, &p),
                None => {
                    if let Some(w) = &mut this.win {
                        w.focus_input(gui);
                    }
                }
            }
            this.swallow = (!swallow.is_empty()).then(|| swallow.to_owned());
            true
        };
        let reply = |this: &Self| {
            let (groups, text) = (this.groups(), |k: &str| texts.by_key(10001, &format!("ChatCmdFeedback_{k}")).unwrap_or_default());
            cmd::reply_prefill(&this.cmd_ctx(zone, &groups, None, &text))
        };
        match ev {
            InputEvent::Key { key: ao_gui::Key::Enter, pressed: true, .. } => open(self, gui, None, ""),
            InputEvent::Key { key: ao_gui::Key::Letter('/'), pressed: true, .. } => open(self, gui, Some("/".into()), "/"),
            InputEvent::Text(t) if t == "/" => open(self, gui, Some("/".into()), ""),
            InputEvent::Key { key: ao_gui::Key::Letter('r'), pressed: true, mods, .. } if mods.shift => {
                let p = reply(self);
                open(self, gui, Some(p), "R")
            }
            InputEvent::Text(t) if t == "R" => {
                let p = reply(self);
                open(self, gui, Some(p), "")
            }
            _ => false,
        }
    }

    /// A GUI event; `true` if it was a chat window's.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone, texts: &TextDb) -> bool {
        let (hit, ans) = self.dialogs.event(gui, ev);
        if hit {
            if let Some(a) = ans {
                self.answer(gui, a, Some(zone), texts);
            }
            return true;
        }
        let (hit, outs) = self.info.event(gui, self.screen, ev);
        if hit {
            self.info_out(gui, outs, zone, texts);
            return true;
        }
        if self.social_ui_event(gui, ev, zone, texts) {
            return true;
        }
        let Some(w) = &mut self.win else { return false };
        let outs = w.event(gui, ev);
        let handled = !outs.is_empty() || w.owns(ev);
        for o in outs {
            match o {
                WinOut::Submit { text, window_group } => self.submit(gui, &text, window_group.as_deref().and_then(ident_id), zone, texts),
                // `user://NAME` -> `OpenTellWindow` 0x10085df8: the tell window opens (docs/chat/social.md)
                WinOut::OpenTell(name) => self.open_tell_named(gui, &name, texts),
                // user-link menu "IgnoreUser" (`FUN_1008dd24`): the `/ignore <nick>` path
                WinOut::IgnoreUser(name) => self.run_line(gui, &format!("/ignore {name}"), zone, texts),
                // `FUN_1008e322` -> `ChatGUIModule_c::ShowItemRefLink` 0x10085cb5: itemref:// itemid:// charref:// text:// open in the InfoView;
                // chatcmd:// runs the command (**GUESS**: the chat view's own handler is not decoded, the InfoView treats them the same way)
                WinOut::LinkClicked(href) => {
                    let lower = href.to_ascii_lowercase();
                    if ["itemref://", "itemid://", "charref://", "text://", "chatcmd://"].iter().any(|p| lower.starts_with(p)) {
                        let outs = self.info.show_url(gui, self.screen, &href, true);
                        self.info_out(gui, outs, zone, texts);
                    } else {
                        eprintln!("chat: link {href} (no handler)");
                    }
                }
            }
        }
        handled
    }

    /// `InfoViewModule_c::ShowURL(url)` (also Shift+click `charid://` / `itemid://` links).
    pub fn show_url(&mut self, gui: &mut Gui, zone: &Zone, texts: &TextDb, url: &str) {
        let outs = self.info.show_url(gui, self.screen, url, true);
        self.info_out(gui, outs, zone, texts);
    }

    fn info_out(&mut self, gui: &mut Gui, outs: Vec<info::InfoOut>, zone: &Zone, texts: &TextDb) {
        for o in outs {
            match o {
                info::InfoOut::Command(c) => self.run_line(gui, &c, zone, texts),
                // `GlobalSignals+0x17c(0, text, 0xc)`: red line in the System window
                info::InfoOut::Error(t) => self.system_line(gui, &t, 12),
            }
        }
    }

    /// `DialogBox_c` + `Go` + `MoveToCenter`.
    fn dialog(&mut self, gui: &mut Gui, kind: dialog::Kind, body: String, buttons: Vec<String>, input: Option<String>) {
        self.dialogs.go(gui, self.screen, &dialog::Spec { kind, body, buttons, input });
    }

    /// What a decided dialog does: org leave / disband send `N3Msg_Org*Confirmed` on Yes (button 0); the AFK dialog is `FUN_100b6576`.
    fn answer(&mut self, gui: &mut Gui, a: dialog::Answer, zone: Option<&Zone>, texts: &TextDb) {
        match (a.kind, a.button) {
            (dialog::Kind::OrgLeave, 0) => self.outbox.extend(zone.map(|z| zonecmd::org_confirmed(z.char_id, true, Self::target_identity(z)))),
            (dialog::Kind::OrgDisband, 0) => self.outbox.extend(zone.map(|z| zonecmd::org_confirmed(z.char_id, false, Self::target_identity(z)))),
            (dialog::Kind::Afk, 0) => {
                let text = a.text;
                if text.is_empty() {
                    self.system_line(gui, "Using default AFK message.", 0);
                } else if self.afk.as_deref() != Some(text.as_str()) {
                    let t = texts.by_key(10001, "ChatCmdFeedback_ChangedAFKMessageTo").unwrap_or_default();
                    let old = self.afk.clone().unwrap_or_default();
                    self.system_line(gui, &log::ldb_format(&t, &[log::Arg::S(old), log::Arg::S(text.clone())]), 0);
                    self.afk = Some(text);
                }
            }
            // `FlowControlModule_t::SendBugReportResult` 0x1002a028: OK (0) sends, everything else is "cancelled"
            (dialog::Kind::BugReport, button) => {
                let text = self.bug.take().unwrap_or_default();
                let key = if button == 0 && !text.is_empty() {
                    if Self::send_bug_report(&text) { "BugReport_Sent" } else { "BugReport_SendFailed" }
                } else {
                    "BugReport_SendCancelled"
                };
                let t = texts.by_key(5000, key).unwrap_or_default();
                self.system_line(gui, &t, 0);
            }
            (dialog::Kind::Afk, _) => {
                self.afk = None;
                self.system_line(gui, "AFK off.", 0);
            }
            // `GuiSystem_c::DuelChallengeReceivedResult` [GUI 0x1002f802]: Accept (0) -> `N3Msg_Duel_Accept`, every other answer (Reject, Esc) -> `_Refuse`
            (dialog::Kind::DuelReceived, button) => {
                let op = if button == 0 { ao_net::n3::action::duel::Op::Accept } else { ao_net::n3::action::duel::Op::Refuse };
                self.game.push(GameAction::Duel { pet: false, op });
            }
            // `DuelChallengeSentResult` [GUI 0x1002f825]: the Cancel button (or Esc) retracts with `N3Msg_Duel_Refuse`
            (dialog::Kind::DuelSent, _) => self.game.push(GameAction::Duel { pet: false, op: ao_net::n3::action::duel::Op::Refuse }),
            (dialog::Kind::StartPvp(target), 0) => self.game.push(GameAction::StartPvp(target)),
            _ => {}
        }
    }

    /// `BugReport_t::SendUserMadeBugReport` (Interfaces 0x1000f931) mails the report over SMTP to `ingamebugs@anarchy-online.com` through
    /// `bugreport.funcom.com:7501` (Funcom's server, not Project Rubi-Ka's). The port deliberately does not transmit the text (and the system
    /// report `GenerateReport` adds) to that third party: it reports the failure path of the original. Reports for PRK go to report.project-rk.com.
    fn send_bug_report(_text: &str) -> bool {
        false
    }

    fn open_url(&self, url: &str) {
        if let Err(e) = std::process::Command::new("open").arg(url).spawn() {
            eprintln!("chat: open {url}: {e}");
        }
    }

    /// `/voice <sound>` (GUI 0x100b82c2).
    fn voice_cmd(&mut self, gui: &mut Gui, zone: &Zone, cmd: &str, sound: Option<String>) {
        let err = |t: &str| ChatLine::new(ChatKind::Other("CCChatCmdFeedbackError"), format!("<div><font color=CCChatCmdFeedbackError>{t}</font></div>"));
        // stat 0x185 (Expansion) bit 0; the required name `FUN_1016c316` joins is not decoded: [GUESS] "Notum Wars" (bit 0 of the expansion flags)
        if zone.stat(0x185).unwrap_or(0) & 1 == 0 {
            return self.line(gui, err("This function requires Notum Wars."));
        }
        let Some(sound) = sound else { return self.line(gui, err(&format!("Usage: {cmd} &lt;sound name&gt;"))) };
        let own = zone.own().map_or(String::new(), |d| d.name.clone());
        let v = voice::Voice { breed: zone.stat(4).unwrap_or(0), sex: zone.stat(0x3b).unwrap_or(0), fx: voice::fx_name(self.voice_prefs.fx_type).into(), sound };
        let cd = ao_gui::client_dir().join("cd_image");
        let cands = voice::candidates(&own, &v);
        if !voice::installed(&cd, &cands) {
            return self.line(gui, err("You do not have that voice effect installed."));
        }
        // `[T]` / `[M]` flags (team-view flash / `GlobalSignals+0x150`) are consumed; only the text is sent
        let (text, _flags) = voice::strip_flags(&voice::spoken_text(&cd, &cands));
        let text = text.trim_end_matches(['\r', '\n']).to_owned();
        let group = self.last_out_group.unwrap_or(G_VICINITY);
        let extras = voice::extras(&v);
        if group == G_VICINITY {
            let tx = zone::text_bytes(&text);
            if let Some(buf) = ao_net::n3::chat::chat_buffer(&tx, zone::Speech::Say as u8) {
                let buf = [buf, extras].concat();
                let p = ao_net::n3::outgoing::text_payload(ao_net::n3::outgoing::TextKind::Vicinity, Self::target_identity(zone), &buf);
                self.outbox.push(ao_net::n3::outgoing::text_frame(0, zone.char_id, p));
            }
        } else if !self.net.say_voice(group, &text, extras) {
            self.line(gui, err(&format!("Error: Chat group {} is currently not available.", self.group_name(group))));
        }
    }

    /// `FUN_1008c953` after `HandleVicinityMessage` / `HandleGroupMessage`: plays the voice of a heard message.
    fn hear_voice(&mut self, m: &line::ChatMsg) {
        let Some(v) = &m.voice else { return };
        let on = match m.group >> 32 {
            0 => self.voice_prefs.hear_vicinity,
            3 => self.voice_prefs.hear_guild,
            0x82 => self.voice_prefs.hear_team,
            _ => false,
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        if !on || v.sound.is_empty() || self.expansion & 1 == 0 || !self.voice_throttle.allow(&m.from_name, now) {
            return;
        }
        let cd = ao_gui::client_dir().join("cd_image");
        if let Some(rel) = voice::wav_rel(&cd, &voice::candidates(&m.from_name, v)) {
            self.requests.sounds.push(rel);
        }
    }

    fn yes_no(texts: &TextDb) -> Vec<String> {
        ["MsgBox_Yes", "MsgBox_No"].iter().map(|k| texts.by_key(10000, k).unwrap_or_else(|| k.trim_start_matches("MsgBox_").to_owned())).collect()
    }

    /// `/open` `/close` `/toggle` (`FUN_100b77b6`): the 24-entry name table drives a DValue; the InfoView is ours, the other windows go to the HUD
    /// through [`Chat::take_windows`]. Names that are not in the table select a *chat window* by name in the original (`FUN_10093c1f`): not ported.
    fn window_cmd(&mut self, gui: &mut Gui, name: &str, op: cmd::WindowOp) {
        let Some(dv) = cmd::window_dvalue(name) else { return };
        if dv == "info_window" {
            let open = self.info.window().is_some();
            match (op, open) {
                (cmd::WindowOp::Open, false) | (cmd::WindowOp::Toggle, false) => self.info.open(gui, self.screen),
                (cmd::WindowOp::Close, true) | (cmd::WindowOp::Toggle, true) => self.info.close(gui),
                _ => {}
            }
        } else {
            self.windows.push((dv, op));
        }
    }

    fn groups(&self) -> Vec<GroupInfo> {
        let mut v = vec![GroupInfo { name: "Vicinity".into(), kind: GroupKind::Vicinity, read_only: false, active: true, subscribed: true }];
        for (&key, name) in &self.net.groups {
            let kind = match (key >> 32) as u8 {
                3 => GroupKind::Org,
                0x82 => GroupKind::Team,
                _ => GroupKind::Other,
            };
            v.push(GroupInfo { name: name.clone(), kind, read_only: false, active: true, subscribed: true });
        }
        v
    }

    fn group_name(&self, id: u64) -> String {
        if id == G_VICINITY { "Vicinity".to_owned() } else { self.net.groups.get(&id).cloned().unwrap_or_default() }
    }

    fn cmd_ctx<'a>(&'a self, zone: &'a Zone, groups: &'a [GroupInfo], out_name: Option<&'a str>, text: &'a dyn Fn(&str) -> String) -> CmdCtx<'a> {
        let own_name = zone.own().map_or("", |d| d.name.as_str());
        CmdCtx {
            own_name,
            own_id: zone.char_id,
            groups,
            output_group: out_name,
            afk: self.afk.as_deref(),
            last_tell_from: self.last_tell_from.as_deref(),
            target: zone.target.and_then(|t| zone.dynels.get(&t).map(|d| Target { kind: CHAR_KIND as u32, id: t as u32, name: d.name.clone() })),
            fight_target: None,
            gm_level: 0,
            warn_unsub: true,
            chat_connected: self.net.logged_in(),
            lft_on: self.social.lft.on,
            script_exists: None,
            text,
        }
    }

    /// A line the GUI runs as if typed (hotbar text macros and `chatcmd://` links: the text is emitted on GlobalSignals +0x180 and
    /// split by GUI `FUN_100a3f43`: `%` args expanded, then cut at every literal `\n ` (backslash, n, blank); each stripped part goes
    /// to the command dispatcher `FUN_100a39e5`, whose result is ignored: parts without a leading `/` are dropped).
    pub fn run_line(&mut self, gui: &mut Gui, text: &str, zone: &Zone, texts: &TextDb) {
        for part in text.split("\\n ") {
            let part = part.trim();
            if part.starts_with('/') {
                self.submit(gui, part, None, zone, texts);
            }
        }
    }

    fn submit(&mut self, gui: &mut Gui, text: &str, out_group: Option<u64>, zone: &Zone, texts: &TextDb) {
        let groups = self.groups();
        let tx = |k: &str| texts.by_key(10001, &format!("ChatCmdFeedback_{k}")).or_else(|| texts.by_key(10001, k)).unwrap_or_default();
        // plain lines go to the output group of the window the Enter came from
        let out_name = out_group.map(|g| self.group_name(g));
        let ctx = self.cmd_ctx(zone, &groups, out_name.as_deref(), &tx);
        let actions = cmd::parse(text, &ctx);
        self.last_out_group = out_group;
        for a in actions {
            self.perform(gui, a, zone, texts);
        }
    }

    /// Actions that leave the chat module (`zonecmd`): game commands, socials, `/played`, `/help`, chat-server requests.
    fn zone_action(&mut self, gui: &mut Gui, a: &ChatAction, zone: &Zone, texts: &TextDb) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let ignored: Vec<(u32, String)> = self.ignored.iter().map(|&i| (i, self.net.name_of(i).unwrap_or_default().to_owned())).collect();
        let secs = (zone.day_time() * 15.0) as u32;
        let pets: Vec<ao_net::n3::textcmd::Pet> =
            zone.pets.iter().map(|&id| ao_net::n3::textcmd::Pet { id, name: zone.dynels.get(&id.instance).map(|d| d.name.clone()) }).collect();
        let ctx = zonecmd::ZoneCmdCtx {
            texts,
            char_id: zone.char_id,
            window: 0,
            target: Self::target_identity(zone),
            target_is_tower: false,
            gm: false,
            in_team: false,
            team_leader: false,
            stat_id: &|n| ao_formats::stats::id_of(n).map(|i| i as i32),
            anim_by_name: &|_| None,
            move_mode: 0,
            vehicle_equipped: false,
            social_counter: self.social_counter,
            ignored: &ignored,
            now_unix: now,
            tz_offset_min: zonecmd::local_offset_minutes(now),
            game_time: (secs / 3600, secs / 60 % 60),
            pet: ao_net::n3::textcmd::PetState {
                pets: &pets,
                target_name: zone.target.and_then(|t| zone.dynels.get(&t)).map(|d| d.name.clone()),
                // `FUN_10044b6e`: stat `Features` (0xE0); only the own one is tracked, the target's stats are not kept
                own_features: zone.stat(0xE0).unwrap_or(0) as u32,
                target_features: 0,
                // `FUN_1003e1d0` ([`Zone::fight_level`]); [INFERENCE] both `FUN_1003e228` calls of the `/follow` gate take no explicit dynel in
                // the decompile, so the target uses the same district level
                own_fight_level: zone.fight_level.unwrap_or(super::fightmode::DEFAULT_LEVEL),
                target_fight_level: zone.fight_level.unwrap_or(super::fightmode::DEFAULT_LEVEL),
                // [UNRESOLVED] `vtbl[0x90]` of the own vehicle (docs/chat/cmd.md §/follow): a controllable avatar
                can_move: true,
                move_mode: 0,
            },
            visual_flags: zone.stat(0x2a1).unwrap_or(0),
            reclaim_open: false,
        };
        let out = zonecmd::perform(a, &ctx);
        if let (Some(on), ChatAction::Lft { text, .. }) = (out.lft, a) {
            // `/lft` shares `DAT_10276620` and `TeamDesc` with the LFT window (`FUN_100f01a3`)
            self.social.lft.on = on;
            if on {
                self.social.lft.description = text.clone();
            }
        }
        self.outbox.extend(out.frames);
        for l in out.lines {
            self.line(gui, l);
        }
        for r in out.chat {
            match r {
                zonecmd::ChatReq::Send(c) => self.net.send(c),
                zonecmd::ChatReq::ByName { name, op } => self.net.lookup_op(&name, op),
            }
        }
        if out.social_sent {
            self.social_counter += 1;
        }
        for u in out.unsupported {
            eprintln!("chat: text command not implemented: {u}");
        }
        for l in out.local {
            match l {
                // `OrganizationGUIModule_c::LeaveOrg` 0x10052568 / `OpenDisbandDialog` 0x100520dd
                ao_net::n3::textcmd::Local::OrgLeaveDialog => {
                    let body = texts.by_key(10000, "ReallyLeaveOrg").unwrap_or_default();
                    self.dialog(gui, dialog::Kind::OrgLeave, body, Self::yes_no(texts), None);
                }
                // `FUN_10052230(path, pets, tower)`: the file is opened relative to the client directory; an unreadable file is an empty script
                ao_net::n3::textcmd::Local::PetScript { path, pets, tower } => {
                    let src = std::fs::read(ao_gui::client_dir().join(path)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                    self.pet_scripts.push(ao_net::n3::textcmd::PetScript::parse(&src, pets, tower));
                }
                ao_net::n3::textcmd::Local::OrgDisbandDialog => {
                    let body = texts.by_key(501, "Org_ConfirmDisband").unwrap_or_default();
                    self.dialog(gui, dialog::Kind::OrgDisband, body, Self::yes_no(texts), None);
                }
                l => eprintln!("chat: local effect not implemented: {l:?}"),
            }
        }
    }

    fn name_op(&mut self, gui: &mut Gui, op: zonecmd::NameOp, id: u32, name: &str, texts: &TextDb) {
        let own = self.own_id;
        if id == u32::MAX {
            self.line(gui, ChatLine::new(ChatKind::Error, format!("Unknown user {name}"))); // [GUESS] text
            return;
        }
        for r in zonecmd::resolve(op, id, own, texts) {
            match r {
                zonecmd::Resolved::Chat(c) => self.net.send(c),
                zonecmd::Resolved::Line(l) => self.line(gui, l),
                zonecmd::Resolved::IgnoreToggle(i) => {
                    if !self.ignored.remove(&i) {
                        self.ignored.insert(i);
                    }
                }
            }
        }
    }

    fn focus_text(&mut self, gui: &mut Gui, t: &str) {
        if let Some(w) = &mut self.win {
            w.focus_input_text(gui, t);
        }
    }

    fn target_identity(zone: &Zone) -> Identity {
        zone.target.map_or(Identity { kind: 0, instance: 0 }, |t| Identity { kind: CHAR_KIND, instance: t })
    }

    fn speak(&mut self, zone: &Zone, texts: &TextDb, speech: zone::Speech, text: &str) {
        let ctx = zone::ZoneChatCtx {
            texts,
            name_of: &|id| zone.dynels.get(&(id as i32)).map(|d| d.name.clone()),
            header_is_char: true,
            target: zone.target.map(|t| t as u32),
            fighting: None,
            mouse: None,
        };
        let text = zone::expand_chat_text_args(text, &ctx);
        if let Some(f) = zone::frame(0, zone.char_id, speech, &text, Self::target_identity(zone)) {
            self.outbox.push(f);
        }
    }

    fn perform(&mut self, gui: &mut Gui, a: ChatAction, zone: &Zone, texts: &TextDb) {
        match a {
            ChatAction::Feedback(l) => self.line(gui, l),
            ChatAction::Tell { to, text } => {
                if text.is_empty() {
                    self.open_tell_named(gui, &to, texts);
                } else {
                    self.net.tell(&to, &text);
                    // outgoing tell echo: text-db template "To [%s]: " (cat 10001 key ChatTellMsgToField, text.mdb line 6574) + text
                    let head = texts.by_key(10001, "ChatTellMsgToField").unwrap_or_else(|| "To [%s]: ".into()).replacen("%s", &to, 1);
                    self.line(gui, ChatLine::new(ChatKind::TellOut, format!("{head}{text}")));
                }
            }
            ChatAction::Vicinity(t) => self.speak(zone, texts, zone::Speech::Say, &t),
            ChatAction::Whisper(t) => self.speak(zone, texts, zone::Speech::Whisper, &t),
            ChatAction::Shout(t) => self.speak(zone, texts, zone::Speech::Shout, &t),
            ChatAction::Emote(t) => self.speak(zone, texts, zone::Speech::Emote, &t),
            ChatAction::Group { group, text } => match self.net.group_by_name(&group) {
                Some(k) if self.net.say(k, &text) => {}
                _ => self.line(gui, ChatLine::new(ChatKind::Error, format!("Chat group {group} is currently not available."))),
            },
            ChatAction::SetInputTarget(name) => {
                let id = if name == "Vicinity" { Some(G_VICINITY) } else { self.net.group_by_name(&name) };
                if let (Some(id), Some(w)) = (id, &mut self.win) {
                    w.set_output_group(id);
                }
            }
            ChatAction::SetAfk(m) => self.afk = m,
            ChatAction::IgnoreToggle { id, name } => {
                let now = self.ignored.insert(id) || !self.ignored.remove(&id);
                let _ = (now, name); // feedback text needs the TextDb: see IgnoreList; persistence is not ported
            }
            ChatAction::Social(id) => self.game.push(GameAction::Social(id)),
            ChatAction::DValue(t) => self.dvalue_cmds.push(t),
            ChatAction::ClientCommand(c) if c.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("/assist")) => self.game.push(GameAction::Assist),
            ChatAction::ShowUrl(u) => self.show_url(gui, zone, texts, &u),
            // FUN_100b6f0d: only from level 4 on (stat 0x36 > 3); `prev` steps back, anything else forward, never below 0
            ChatAction::TipOfTheDay { prev } => {
                if zone.stat(0x36).unwrap_or(0) > 3 {
                    self.tip = (self.tip + if prev { -1 } else { 1 }).max(0);
                    let u = self.info.tip_url(self.tip);
                    let outs = self.info.show_url(gui, self.screen, &u, true);
                    self.info_out(gui, outs, zone, texts);
                }
            }
            ChatAction::MessageBox(t) => {
                let ok = texts.by_key(10000, "MsgBox_OK").unwrap_or_else(|| "OK".into());
                self.dialog(gui, dialog::Kind::MessageBox, t, vec![ok], None);
            }
            ChatAction::AfkPrompt { default, body } => self.dialog(gui, dialog::Kind::Afk, body, vec!["Ok".into()], Some(default)),
            ChatAction::Camp => self.game.push(GameAction::Camp),
            ChatAction::SelectSelf => self.game.push(GameAction::SelectSelf),
            ChatAction::Duel { pet, op } => self.game.push(GameAction::Duel { pet, op }),
            // `BrowserWindow_c` type 3 ("Petition", an embedded browser) opens `https://report.project-rk.com/` (docs/chat/dialogs.md §6);
            // the system browser stands in for the embedded Awesomium view
            ChatAction::Petition => self.open_url(PETITION_URL),
            ChatAction::Waypoint { x, z, pf } => self.requests.waypoint = Some((pf as u32, [x, z])),
            ChatAction::Filter(t) => {
                for l in self.filter.command(&t) {
                    self.line(gui, zonecmd::info_line(&l));
                }
            }
            ChatAction::Macro { name, command } => {
                let id = self.macros.create(&name, &command, 0, true);
                self.requests.macro_drag = self.macros.get(id).cloned();
            }
            ChatAction::Bug(text) => {
                let head = texts.by_key(5000, "BugReport_ReportMsgH").unwrap_or_default();
                let ok = texts.by_key(10000, "MsgBox_OK").unwrap_or_else(|| "OK".into());
                let cancel = texts.by_key(10000, "MsgBox_Cancel").unwrap_or_else(|| "Cancel".into());
                self.bug = Some(text.clone());
                self.dialog(gui, dialog::Kind::BugReport, format!("{head}{text}"), vec![ok, cancel], None);
            }
            ChatAction::Voice { cmd, sound } => self.voice_cmd(gui, zone, &cmd, sound),
            ChatAction::Quit => self.quit = true,
            // FUN_100b94e0: `ShellExecuteA("open", url)`; the macOS equivalent (`Play::show_error` does the same for the login error pages)
            ChatAction::Start(u) => {
                if let Err(e) = std::process::Command::new("open").arg(&u).spawn() {
                    eprintln!("chat: /start {u}: {e}");
                }
            }
            ChatAction::Window { name, op } => self.window_cmd(gui, &name, op),
            a if zonecmd::handles(&a) => self.zone_action(gui, &a, zone, texts),
            other => eprintln!("chat: action not implemented: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// text.mdb 10001 `ChatTellMsgToField` is the client's own outgoing-tell template (pool offset 269621, preceded by " joined the group.").
    #[test]
    fn tell_template_key_is_in_the_real_db() {
        let Some(h) = std::env::var_os("HOME") else { return };
        let Ok(db) = TextDb::load(&std::path::Path::new(&h).join("Games/ProjectRubiKa/client")) else { return };
        assert_eq!(db.by_key(10001, "ChatTellMsgToField").as_deref(), Some("To [%s]: "));
    }
}
