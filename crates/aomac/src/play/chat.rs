//! In-game chat hub: chat-server session ([`net`]), zone chat ([`zone`]), input commands ([`cmd`]), the windows ([`win`]).
//! Evidence: docs/chat/*.md. The hub is the `ChatGUIModule_c` equivalent: it routes everything into the windows and
//! performs what an input line asks for.

mod cmd;
mod line;
mod log;
mod net;
pub(super) mod win;
mod zone;
mod zonecmd;

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

/// Identity kind of characters (`SimpleChar_t`).
const CHAR_KIND: i32 = 0xC350;

/// Chat commands that act on the game, not on the chat (run by the combat layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GameAction {
    /// `/<emote>`, `/emote <name>`: `N3Msg_DoSocialAction(id)`.
    Social(u32),
    /// `/assist`.
    Assist,
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
    /// Actions of the game layer (`/<emote>`, `/assist`): taken by the flow ([`Chat::take_game`]).
    game: Vec<GameAction>,
    /// Text event to drop: the character of the key that opened the input bar.
    swallow: Option<String>,
    /// `s_nCommandRefCntr` of the social commands sent.
    social_counter: i32,
    own_id: u32,
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
        Self { win: None, backlog: vec![], net, afk: None, last_tell_from: None, ignored: HashSet::new(), outbox: vec![], game: vec![], swallow: None, social_counter: 0, own_id: 0 }
    }

    /// The chat windows (`ChatGUIModule_c::Initialize`), once the world is shown.
    pub fn open(&mut self, gui: &mut Gui, screen: (u32, u32)) -> Result<()> {
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
        if let Some(w) = &mut self.win {
            w.resize(gui, screen);
        }
    }

    /// `FUN_10058b00(key)` [GC 0x10058b00]: the `Feedback_*` text (category 110) of the client character as a plain System-window line
    /// (`FUN_10012b05(0, text, 0)`: no colour code, so no `<font>`).
    pub fn feedback(&mut self, gui: &mut Gui, key: &str, texts: &TextDb) {
        if let Some(t) = texts.by_key(110, key) {
            self.line_to(gui, ChatLine::new(ChatKind::System, log::window_html("", &t)), Some("System"));
        }
    }

    pub fn take_game(&mut self) -> Vec<GameAction> {
        std::mem::take(&mut self.game)
    }

    /// Frames to send to the zone server.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// One zone frame: 0x43 (chat server list) connects; N3 chat messages become lines.
    pub fn on_zone_frame(&mut self, gui: &mut Gui, f: &Frame, zone: &Zone, texts: &TextDb) {
        self.own_id = zone.char_id;
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
                    filter: &filter,
                };
                for ev in log::events(&m) {
                    for l in log::classify(&ev, &ctx) {
                        let hint = self.class_name(l.class as u64);
                        self.line_to(gui, l.line, hint.as_deref());
                    }
                }
            }
            _ => {}
        }
    }

    /// Per frame: chat-server events into the windows, window fades.
    pub fn update(&mut self, gui: &mut Gui, dt: f32, texts: &TextDb) {
        for o in self.net.update(dt) {
            match o {
                Out::Msg(m) => {
                    if m.tell {
                        self.last_tell_from = Some(m.from_name.clone());
                    }
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
            }
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
        let Some(w) = &mut self.win else { return false };
        let outs = w.event(gui, ev);
        let handled = !outs.is_empty() || w.owns(ev);
        for o in outs {
            match o {
                WinOut::Submit { text, window_group } => self.submit(gui, &text, window_group.as_deref().and_then(ident_id), zone, texts),
                WinOut::OpenTell(name) => self.focus_text(gui, &format!("/tell {name} ")),
                WinOut::LinkClicked(href) => eprintln!("chat: link {href} (no handler)"),
            }
        }
        handled
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
            lft_on: false,
            script_exists: None,
            text,
        }
    }

    /// A line the GUI runs as if typed (hotbar text macros: the macro text is emitted on GlobalSignals +0x180, GUI `FUN_100d79c9`).
    pub fn run_line(&mut self, gui: &mut Gui, text: &str, zone: &Zone, texts: &TextDb) {
        self.submit(gui, text, None, zone, texts);
    }

    fn submit(&mut self, gui: &mut Gui, text: &str, out_group: Option<u64>, zone: &Zone, texts: &TextDb) {
        let groups = self.groups();
        let tx = |k: &str| texts.by_key(10001, &format!("ChatCmdFeedback_{k}")).or_else(|| texts.by_key(10001, k)).unwrap_or_default();
        // plain lines go to the output group of the window the Enter came from
        let out_name = out_group.map(|g| self.group_name(g));
        let ctx = self.cmd_ctx(zone, &groups, out_name.as_deref(), &tx);
        let actions = cmd::parse(text, &ctx);
        for a in actions {
            self.perform(gui, a, zone, texts);
        }
    }

    /// Actions that leave the chat module (`zonecmd`): game commands, socials, `/played`, `/help`, chat-server requests.
    fn zone_action(&mut self, gui: &mut Gui, a: &ChatAction, zone: &Zone, texts: &TextDb) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let ignored: Vec<(u32, String)> = self.ignored.iter().map(|&i| (i, self.net.name_of(i).unwrap_or_default().to_owned())).collect();
        let secs = (zone.day_time() * 15.0) as u32;
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
        };
        let out = zonecmd::perform(a, &ctx);
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
        if let Some(f) = out.help_file {
            // the help pages open in the InfoView (not ported): say where the page is
            self.line(gui, ChatLine::new(ChatKind::CmdFeedback, format!("Help: text/help/{f}")));
        }
        if out.social_sent {
            self.social_counter += 1;
        }
        for u in out.unsupported {
            eprintln!("chat: text command not implemented: {u}");
        }
        for l in out.local {
            eprintln!("chat: local effect not implemented: {l:?}");
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
                    self.focus_text(gui, &format!("/tell {to} "));
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
            ChatAction::ClientCommand(c) if c.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("/assist")) => self.game.push(GameAction::Assist),
            a if zonecmd::handles(&a) => self.zone_action(gui, &a, zone, texts),
            other => eprintln!("chat: action not implemented: {other:?}"),
        }
    }
}
