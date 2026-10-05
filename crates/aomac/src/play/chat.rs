//! In-game chat hub: chat-server session ([`net`]), zone chat ([`zone`]), input commands ([`cmd`]), the windows ([`win`]).
//! Evidence: docs/chat/*.md. The hub is the `ChatGUIModule_c` equivalent: it routes everything into the windows and
//! performs what an input line asks for.

mod cmd;
mod line;
mod net;
pub(super) mod win;
mod zone;

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
use win::{ChatWindows, WinOut, G_VICINITY};

/// Identity kind of characters (`SimpleChar_t`).
const CHAR_KIND: i32 = 0xC350;

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
}

enum Back {
    Line(ChatLine),
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
        Self { win: None, backlog: vec![], net, afk: None, last_tell_from: None, ignored: HashSet::new(), outbox: vec![] }
    }

    /// The chat windows (`ChatGUIModule_c::Initialize`), once the world is shown.
    pub fn open(&mut self, gui: &mut Gui, screen: (u32, u32)) -> Result<()> {
        if self.win.is_none() {
            self.win = Some(ChatWindows::new(gui, screen)?);
            for b in std::mem::take(&mut self.backlog) {
                match b {
                    Back::Line(l) => self.line(gui, l),
                    Back::Msg(m) => self.msg(gui, m),
                    Back::Group(g, n) => self.group(g, n),
                    Back::Ungroup(g) => self.ungroup(g),
                }
            }
        }
        Ok(())
    }

    fn line(&mut self, gui: &mut Gui, l: ChatLine) {
        match &mut self.win {
            Some(w) => w.push(gui, &l, None),
            None => self.backlog.push(Back::Line(l)),
        }
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

    /// Frames to send to the zone server.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// One zone frame: 0x43 (chat server list) connects; N3 chat messages become lines.
    pub fn on_zone_frame(&mut self, gui: &mut Gui, f: &Frame, zone: &Zone, texts: &TextDb) {
        match f.ptype {
            PT_SYSTEM => self.net.on_system_frame(f, zone.char_id),
            PT_N3 => {
                let Ok(m) = n3::decode(f) else { return };
                let N3::Chat(c) = &m.body else { return };
                let ctx = zone::ZoneChatCtx {
                    texts,
                    name_of: &|id| zone.dynels.get(&(id as i32)).map(|d| d.name.clone()),
                    header_is_char: m.header.target.kind == CHAR_KIND,
                    target: zone.target.map(|t| t as u32),
                    fighting: None,
                    mouse: None,
                };
                for l in zone::lines(c, &ctx) {
                    self.line(gui, l);
                }
            }
            _ => {}
        }
    }

    /// Per frame: chat-server events into the windows, window fades.
    pub fn update(&mut self, gui: &mut Gui, dt: f32) {
        for o in self.net.update(dt) {
            match o {
                Out::Msg(m) => {
                    if m.tell {
                        self.last_tell_from = Some(m.from_name.clone());
                    }
                    self.msg(gui, m);
                }
                Out::Line(l) => self.line(gui, l),
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
        if gui.focused_view().is_some() || self.win.is_none() {
            return false;
        }
        match ev {
            InputEvent::Key { key: ao_gui::Key::Enter, pressed: true, .. } => {
                self.win.as_mut().map(|w| w.focus_input(gui));
                true
            }
            InputEvent::Text(t) if t == "/" => {
                self.focus_text(gui, "/");
                true
            }
            InputEvent::Key { key: ao_gui::Key::Letter('R'), pressed: true, mods, .. } if mods.shift => {
                let prefill = {
                    let (groups, text) = (self.groups(), |k: &str| texts.by_key(10001, &format!("ChatCmdFeedback_{k}")).unwrap_or_default());
                    cmd::reply_prefill(&self.cmd_ctx(zone, &groups, None, &text))
                };
                self.focus_text(gui, &prefill);
                true
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
                    // [GUESS] outgoing tell echo format ("To [name]: text"); the original prints it through the tell window (FUN_10084f9e)
                    self.line(gui, ChatLine::new(ChatKind::TellOut, format!("To {to}: {text}")));
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
            other => eprintln!("chat: action not implemented: {other:?}"),
        }
    }
}
