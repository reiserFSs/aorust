//! The chat-server half of the hub: owns the [`ChatSession`], mirrors the client's connect/reconnect behaviour
//! (`ChatGUIModule_c` GUI 0x10089dfc), keeps the id <-> name tables and turns server events into [`Out`].
//! Protocol and evidence: docs/chat/net.md.

use super::line::{ChatKind, ChatLine, ChatMsg};
use ao_net::chat::{self, ChatCmd, ChatEvent, ChatSession, GroupId};
use ao_net::frame::{Frame, PT_SYSTEM};
use std::collections::{BTreeMap, HashMap};

/// What the hub gets out of the network layer each frame.
#[derive(Debug, PartialEq)]
pub enum Out {
    Msg(ChatMsg),
    Line(ChatLine),
    /// S2C_GROUP_JOIN: a channel to show/subscribe (`ChatGUIModule_c::AddGroup` 0x10085f91).
    GroupAdd { group: u64, name: String, flags: u32 },
    /// S2C_GROUP_PART (`RemoveGroup` 0x1008603d).
    GroupRemove { group: u64, name: String },
}

pub fn group_key(g: GroupId) -> u64 {
    (g.kind as u64) << 32 | g.id as u64
}
fn group_of(key: u64) -> GroupId {
    GroupId { kind: (key >> 32) as u8, id: key as u32 }
}

/// Reconnect pacing (GUI 0x10089dfc): `1 << (attempt + 12)` ms, capped at 0x8000, for at most 10 attempts.
fn backoff(attempt: u32) -> f32 {
    (if attempt < 3 { 1u32 << (attempt + 12) } else { 0x8000 }) as f32 / 1000.0
}
const MAX_ATTEMPTS: u32 = 10;

#[derive(Default)]
pub struct ChatNet {
    session: Option<ChatSession>,
    server: Option<(String, u16)>,
    user: String,
    password: String,
    char_id: u32,
    /// Seconds until the next connect attempt; `None` = no attempt pending.
    retry_in: Option<f32>,
    attempts: u32,
    logged_in: bool,
    names: HashMap<u32, String>,
    ids: HashMap<String, u32>,
    pub groups: BTreeMap<u64, String>,
    /// Tells waiting for their name lookup (lower-case name -> texts).
    pending: Vec<(String, String)>,
    tap: Option<std::path::PathBuf>,
}

impl ChatNet {
    /// `cPlayerName` / `cPlayerPasswd` of the original, kept in memory for the chat login only.
    pub fn set_credentials(&mut self, user: &str, password: &str) {
        self.user = user.to_owned();
        self.password = password.to_owned();
    }

    /// `AOMAC_CHAT_TRACE=<file>`: record every chat frame (login key redacted).
    pub fn set_trace(&mut self, p: Option<std::path::PathBuf>) {
        self.tap = p;
    }

    pub fn logged_in(&self) -> bool {
        self.logged_in
    }

    pub fn name_of(&self, id: u32) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// Zone system frames: 0x43 = chat server list. Connects once (the client only creates its connection if it has none).
    pub fn on_system_frame(&mut self, f: &Frame, char_id: u32) {
        if f.ptype != PT_SYSTEM {
            return;
        }
        let Some(list) = chat::parse_server_list(&f.payload) else { return };
        // more than one entry -> a selection runs in the client (FUN_1008b3b9/1008bba5, untraced): first entry wins here
        let Some(s) = list.first() else { return };
        self.server = Some((s.host.clone(), s.port));
        self.char_id = char_id;
        if self.session.is_none() && self.retry_in.is_none() {
            self.retry_in = Some(0.0);
            self.attempts = 0;
        }
    }

    fn connect(&mut self) -> Option<Out> {
        let (host, port) = self.server.clone()?;
        let tap = self.tap.clone().map(ao_net::conn::record_tap);
        match ChatSession::connect((host.as_str(), port), tap) {
            Ok(s) => {
                s.login(&self.user, &self.password, self.char_id);
                self.session = Some(s);
                None
            }
            Err(e) => {
                eprintln!("chat: connect {host}:{port}: {e:#}");
                self.schedule_retry();
                None
            }
        }
    }

    fn schedule_retry(&mut self) {
        self.session = None;
        self.logged_in = false;
        self.retry_in = (self.attempts < MAX_ATTEMPTS).then(|| backoff(self.attempts));
        self.attempts += 1;
    }

    /// Poll the session; call every frame.
    pub fn update(&mut self, dt: f32) -> Vec<Out> {
        let mut out = vec![];
        if let Some(t) = self.retry_in.as_mut() {
            *t -= dt;
            if *t <= 0.0 {
                self.retry_in = None;
                self.connect();
            }
        }
        while let Some(e) = self.session.as_ref().and_then(|s| s.poll()) {
            self.on_event(e, &mut out);
        }
        out
    }

    fn on_event(&mut self, e: ChatEvent, out: &mut Vec<Out>) {
        match e {
            ChatEvent::LoggedIn => {
                self.logged_in = true;
                self.attempts = 0;
            }
            ChatEvent::LoginFailed => {
                eprintln!("chat: login rejected");
                self.session = None;
            }
            ChatEvent::Disconnected(why) => {
                eprintln!("chat: disconnected: {why}");
                if self.session.is_some() {
                    self.schedule_retry();
                }
            }
            ChatEvent::UserName { id, name } => self.learn(id, name),
            ChatEvent::Lookup { id, name } => {
                if id != u32::MAX {
                    self.learn(id, name.clone());
                }
                self.finish_tells(&name, id, out);
            }
            ChatEvent::Tell { from, text, data } => out.push(Out::Msg(ChatMsg {
                from_id: from,
                from_name: self.name_of(from).unwrap_or_default().to_owned(),
                text,
                kind: data.first().copied().unwrap_or(0),
                tell: true,
                ..Default::default()
            })),
            ChatEvent::Vicinity { name, text, data, .. } => out.push(Out::Msg(ChatMsg {
                group: 0x4000_0002,
                from_name: name,
                text,
                kind: data.first().copied().unwrap_or(0),
                ..Default::default()
            })),
            ChatEvent::System(text) => out.push(Out::Line(ChatLine::new(ChatKind::System, text))),
            ChatEvent::GroupJoin { group, name, flags, .. } => {
                let key = group_key(group);
                self.groups.insert(key, name.clone());
                out.push(Out::GroupAdd { group: key, name, flags });
            }
            ChatEvent::GroupPart(g) => {
                let key = group_key(g);
                let name = self.groups.remove(&key).unwrap_or_default();
                out.push(Out::GroupRemove { group: key, name });
            }
            ChatEvent::GroupMessage { group, from, text, data } => {
                let key = group_key(group);
                out.push(Out::Msg(ChatMsg {
                    group: key,
                    group_name: self.groups.get(&key).cloned().unwrap_or_default(),
                    from_id: from,
                    from_name: self.name_of(from).unwrap_or_default().to_owned(),
                    text,
                    kind: data.first().copied().unwrap_or(0),
                    ..Default::default()
                }));
            }
            other => eprintln!("chat: unhandled {other:?}"),
        }
    }

    fn learn(&mut self, id: u32, name: String) {
        self.ids.insert(name.to_lowercase(), id);
        self.names.insert(id, name);
    }

    fn finish_tells(&mut self, name: &str, id: u32, out: &mut Vec<Out>) {
        let key = name.to_lowercase();
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending).into_iter().partition(|(n, _)| *n == key);
        self.pending = rest;
        for (_, text) in mine {
            if id == u32::MAX {
                out.push(Out::Line(ChatLine::new(ChatKind::Error, format!("Unknown user {name}")))); // [GUESS] ChatCmdFeedback_UnknownUser text
            } else if let Some(s) = &self.session {
                s.send(ChatCmd::Tell { to: id, text });
            }
        }
    }

    /// Send a tell to a character by name (looked up first when its id is unknown).
    pub fn tell(&mut self, name: &str, text: &str) {
        let Some(s) = &self.session else { return };
        match self.ids.get(&name.to_lowercase()) {
            Some(&to) => s.send(ChatCmd::Tell { to, text: text.to_owned() }),
            None => {
                self.pending.push((name.to_lowercase(), text.to_owned()));
                s.send(ChatCmd::Lookup(name.to_owned()));
            }
        }
    }

    /// Say something in a chat-server channel by its group key; `false` if not connected.
    pub fn say(&mut self, group: u64, text: &str) -> bool {
        match (&self.session, self.logged_in) {
            (Some(s), true) => {
                s.send(ChatCmd::Group { group: group_of(group), text: text.to_owned() });
                true
            }
            _ => false,
        }
    }

    /// Group key of a channel name (case-insensitive exact match).
    pub fn group_by_name(&self, name: &str) -> Option<u64> {
        self.groups.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_matches_client() {
        assert_eq!([backoff(0), backoff(1), backoff(2), backoff(3), backoff(9)], [4.096, 8.192, 16.384, 32.768, 32.768]);
    }

    #[test]
    fn events_become_messages() {
        let mut n = ChatNet::default();
        let mut out = vec![];
        n.on_event(ChatEvent::UserName { id: 5, name: "Bob".into() }, &mut out);
        n.on_event(ChatEvent::GroupJoin { group: GroupId { kind: 3, id: 9 }, name: "OOC".into(), flags: 0, data: vec![] }, &mut out);
        n.on_event(ChatEvent::GroupMessage { group: GroupId { kind: 3, id: 9 }, from: 5, text: "hi".into(), data: vec![] }, &mut out);
        assert_eq!(n.group_by_name("ooc"), Some(0x3_0000_0009));
        let Out::Msg(m) = &out[1] else { panic!("{out:?}") };
        assert_eq!((m.group_name.as_str(), m.from_name.as_str(), m.text.as_str()), ("OOC", "Bob", "hi"));
    }
}
