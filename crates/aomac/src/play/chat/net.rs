//! The chat-server half of the hub: owns the [`ChatSession`], mirrors the client's connect/reconnect behaviour
//! (`ChatGUIModule_c` GUI 0x10089dfc), keeps the id <-> name tables and turns server events into [`Out`].
//! Protocol and evidence: docs/chat/net.md.

use super::line::{ChatKind, ChatLine, ChatMsg};
use super::zonecmd::NameOp;
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
    /// S2C_SYS_MESSAGE_LOCAL_FMT: text id of category 20000 with its arguments (formatted by the hub with the text db).
    SystemFmt { sender: u32, kind: u32, text_id: u32, args: Vec<ao_net::chat::FmtArg> },
    /// A name lookup requested with [`ChatNet::lookup_op`] finished (`id == u32::MAX`: unknown name).
    NameOp { op: NameOp, id: u32, name: String },
    /// S2C_GROUP_PART (`RemoveGroup` 0x1008603d).
    GroupRemove { group: u64, name: String },
    /// Buddy / private group / LFT events for [`super::social::Social`].
    Social(ChatEvent),
    /// A name lookup requested with [`ChatNet::lookup_open`] finished: open that character's tell window (`id == u32::MAX`: unknown).
    OpenTell { id: u32, name: String },
}

/// `HandleVicinityMessage` [GUI 0x10086728]: data byte kind 4..7 pick fixed window groups, everything else is "vicinity".
fn vicinity_msg(from_id: u32, from_name: String, text: String, data: &[u8]) -> ChatMsg {
    let kind = data.first().copied().unwrap_or(0);
    let group = match kind {
        4 => 0x4100_0000,
        5 => 0x4100_0001,
        6 => 0x4200_001b,
        7 => 0x4200_001a,
        _ => 0x4000_0002,
    };
    // the TLV list after the kind byte (`FUN_10085b4a`): flags + the voice block
    let (voice, flags) = super::voice::parse_block(data.get(1..).unwrap_or_default());
    ChatMsg { group, from_id, from_name, text, kind, flags, voice, ..Default::default() }
}

pub fn group_key(g: GroupId) -> u64 {
    (g.kind as u64) << 32 | g.id as u64
}
fn group_of(key: u64) -> GroupId {
    GroupId { kind: (key >> 32) as u8, id: key as u32 }
}

/// Reconnect pacing (`ChatGUIModule_c` update, GUI 0x10089dfc), `tries` = connection attempts since the last login: the first attempt is
/// immediate, then `1 << (tries + 12)` ms for `tries < 3`, else 0x8000 ms. There is **no upper bound** on the number of attempts.
fn backoff(tries: u32) -> f32 {
    match tries {
        0 => 0.0,
        1..=2 => (1u32 << (tries + 12)) as f32 / 1000.0,
        _ => 0x8000 as f32 / 1000.0,
    }
}

/// With several entries in the 0x43 list the client switches to the next one every 16th attempt (`tries > 0 && tries.is_multiple_of(16)`, GUI 0x10089dfc).
fn rotates(tries: u32, entries: usize) -> bool {
    tries > 0 && tries.is_multiple_of(16) && entries > 1
}

#[derive(Default)]
pub struct ChatNet {
    session: Option<ChatSession>,
    /// The 0x43 server list (`ChatGUIModule_c+0x20`, 0x24-byte entries); the front entry is the one in use.
    servers: std::collections::VecDeque<(String, u16)>,
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
    /// Lookups waiting for a [`NameOp`] (lower-case name).
    ops: Vec<(String, NameOp)>,
    /// Lookups waiting to open a tell window (lower-case name).
    opens: Vec<String>,
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
        // `ChatGUIModule_c` keeps the whole list (vector of 0x24-byte entries) and connects to the front entry (FUN_1008b3b9 = erase(begin),
        // FUN_1008bba5 = push_back: the used entry moves to the back); see `connect` / `rotates`
        if list.is_empty() {
            return;
        }
        self.servers = list.into_iter().map(|s| (s.host, s.port)).collect();
        self.char_id = char_id;
        if self.session.is_none() && self.retry_in.is_none() {
            self.retry_in = Some(0.0);
            self.attempts = 0;
        }
    }

    /// Live-test hook (`AOMAC_LIVE_STEPS=chatdrop`): close our end of the chat socket; the session thread reports `Disconnected("closed")`
    /// and the normal retry pacing takes over.
    #[cfg(test)]
    pub fn drop_connection(&self) {
        if let Some(s) = &self.session {
            s.send(ChatCmd::Quit);
        }
    }

    fn connect(&mut self) -> Option<Out> {
        if rotates(self.attempts, self.servers.len()) {
            self.servers.rotate_left(1);
        }
        let (host, port) = self.servers.front().cloned()?;
        eprintln!("chat: connect attempt {} to {host}:{port}", self.attempts);
        self.attempts += 1;
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
        self.retry_in = Some(backoff(self.attempts));
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
                eprintln!("chat: logged in");
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
                let key = name.to_lowercase();
                if self.opens.contains(&key) {
                    self.opens.retain(|n| *n != key);
                    out.push(Out::OpenTell { id, name: name.clone() });
                }
                let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.ops).into_iter().partition(|(n, _)| *n == key);
                self.ops = rest;
                out.extend(mine.into_iter().map(|(_, op)| Out::NameOp { op, id, name: name.clone() }));
            }
            ChatEvent::Tell { from, text, data } => out.push(Out::Msg(ChatMsg {
                from_id: from,
                from_name: self.name_of(from).unwrap_or_default().to_owned(),
                text,
                kind: data.first().copied().unwrap_or(0),
                tell: true,
                ..Default::default()
            })),
            ChatEvent::Vicinity { from, text, data } => {
                let from_name = self.name_of(from).unwrap_or_default().to_owned();
                out.push(Out::Msg(vicinity_msg(from, from_name, text, &data)));
            }
            ChatEvent::VicinityAnon { name, text, data } => out.push(Out::Msg(vicinity_msg(0, name, text, &data))),
            ChatEvent::SystemFmt { sender, kind, text_id, args } => out.push(Out::SystemFmt { sender, kind, text_id, args }),
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
                let (voice, flags) = super::voice::parse_block(&data);
                out.push(Out::Msg(ChatMsg {
                    group: key,
                    voice,
                    flags,
                    group_name: self.groups.get(&key).cloned().unwrap_or_default(),
                    from_id: from,
                    from_name: self.name_of(from).unwrap_or_default().to_owned(),
                    text,
                    kind: data.first().copied().unwrap_or(0),
                    ..Default::default()
                }));
            }
            // buddy list, private groups, LFT: the hub's social layer decides (social.rs, it has the text db and prefs)
            e @ (ChatEvent::BuddyAdd { .. }
            | ChatEvent::BuddyRemove(_)
            | ChatEvent::PrivInvited(_)
            | ChatEvent::PrivKicked(_)
            | ChatEvent::PrivJoined { .. }
            | ChatEvent::PrivParted { .. }
            | ChatEvent::PrivMessage { .. }
            | ChatEvent::PrivDeclined { .. }
            | ChatEvent::LftReply(_)) => out.push(Out::Social(e)),
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

    /// A raw chat-server request (needs the login to have finished).
    pub fn send(&self, c: ChatCmd) {
        if let (Some(s), true) = (&self.session, self.logged_in) {
            s.send(c);
        }
    }

    /// Look a name up (0x15), then report [`Out::NameOp`].
    pub fn lookup_op(&mut self, name: &str, op: NameOp) {
        if let Some(s) = &self.session {
            self.ops.push((name.to_lowercase(), op));
            s.send(ChatCmd::Lookup(name.to_owned()));
        }
    }

    /// Id of a character by name: known ids answer at once, else the name is looked up (0x15) and [`Out::OpenTell`] follows.
    pub fn lookup_open(&mut self, name: &str) -> Option<u32> {
        if let Some(&id) = self.ids.get(&name.to_lowercase()) {
            return Some(id);
        }
        if let Some(s) = &self.session {
            self.opens.push(name.to_lowercase());
            s.send(ChatCmd::Lookup(name.to_owned()));
        }
        None
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
    /// `/voice` into a chat-server group: the message with the extras block in `D`.
    pub fn say_voice(&mut self, group: u64, text: &str, extras: Vec<u8>) -> bool {
        match (&self.session, self.logged_in) {
            (Some(s), true) => {
                s.send(ChatCmd::GroupVoice { group: group_of(group), text: text.to_owned(), extras });
                true
            }
            _ => false,
        }
    }

    pub fn group_by_name(&self, name: &str) -> Option<u64> {
        self.groups.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(k, _)| *k)
    }

    /// Test hook: feeds one decoded event as if it came from the session.
    #[cfg(test)]
    pub(super) fn inject(&mut self, e: ChatEvent) -> Vec<Out> {
        let mut out = vec![];
        self.on_event(e, &mut out);
        out
    }

    /// A private chat group the character joined / left (`AddGroup` of the 0x37 handler): it is a group like the announced channels, so
    /// `/g <name>`, window selection and [`ChatNet::say`] (type 0x39 for kind 0xE) work on it.
    pub fn set_group(&mut self, key: u64, name: Option<&str>) {
        match name {
            Some(n) => {
                self.groups.insert(key, n.to_owned());
            }
            None => {
                self.groups.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_matches_client() {
        assert_eq!([backoff(0), backoff(1), backoff(2), backoff(3), backoff(9), backoff(1000)], [0.0, 8.192, 16.384, 32.768, 32.768, 32.768]);
    }

    #[test]
    fn retries_are_unbounded_and_every_16th_attempt_switches_server() {
        let mut n = ChatNet::default();
        for tries in [0, 1, 9, 10, 11, 1000] {
            n.attempts = tries;
            n.schedule_retry();
            assert_eq!(n.retry_in, Some(backoff(tries)), "no attempt bound (the client counts forever)");
        }
        assert!(!rotates(15, 2) && rotates(16, 2) && rotates(32, 2) && !rotates(16, 1) && !rotates(0, 2));
        n.servers = [("a".to_string(), 1), ("b".to_string(), 2)].into();
        n.attempts = 16;
        if rotates(n.attempts, n.servers.len()) {
            n.servers.rotate_left(1);
        }
        assert_eq!(n.servers.front().unwrap().0, "b");
    }

    /// Replay of a live session (docs/captures/chat_session_ithaca.rec): groups, MOTD, own group message echo, tell to an offline friend.
    #[test]
    fn live_session_replay() {
        let mut n = ChatNet::default();
        let mut out = vec![];
        for l in include_str!("../../../../../docs/captures/chat_session_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next(), p.next().unwrap(), p.next().unwrap());
            if dir != "<" {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            let t = u16::from_be_bytes([b[0], b[1]]);
            if t != 0 {
                n.on_event(ao_net::chat::decode(t, &b[4..]).unwrap(), &mut out);
            }
        }
        let names: Vec<_> = n.groups.values().map(String::as_str).collect();
        assert_eq!(names, ["Global", "IRRK News Wire", "Server Announcements", "Global Trade", "Neutral", "Clan", "Omni-Tek"]);
        assert_eq!(n.group_by_name("global"), Some(0x5_0000_0014));
        let msgs: Vec<&ChatMsg> = out.iter().filter_map(|o| if let Out::Msg(m) = o { Some(m) } else { None }).collect();
        assert!(msgs.iter().any(|m| m.group_name == "Global" && m.from_name == "Aomacvolk" && m.text == "aomac client test"), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.text.starts_with("This player is currently offline")));
        assert!(msgs.iter().any(|m| m.text.starts_with("Welcome to Project Rubi-Ka!") && m.group == 0x4000_0002));
        // our own vicinity message comes back as 0x22 with our id
        assert!(msgs.iter().any(|m| m.group == 0x4000_0002 && m.from_name == "Aomacvolk" && m.text == "aomac client test"), "{msgs:?}");
    }

    /// Raw packets of the social layer (types 0x28 / 0x32 / 0x37 / 0x39 / 0x5dd) decode and reach the hub as [`Out::Social`].
    #[test]
    fn social_packets_are_routed() {
        let mut n = ChatNet::default();
        let s = |x: &str| [&(x.len() as u16).to_be_bytes()[..], x.as_bytes()].concat();
        let pkts: Vec<(u16, Vec<u8>)> = vec![
            (0x28, [&7u32.to_be_bytes()[..], &1u32.to_be_bytes(), &s("\u{1}")].concat()),
            (0x32, 9u32.to_be_bytes().to_vec()),
            (0x37, [&9u32.to_be_bytes()[..], &1u32.to_be_bytes()].concat()),
            (0x39, [&9u32.to_be_bytes()[..], &7u32.to_be_bytes(), &s("hi"), &s("")].concat()),
            (0x5dd, [&[2u8][..], &0u32.to_be_bytes(), &s(""), &0u32.to_be_bytes(), &0u32.to_be_bytes(), &[0, 0], &s("")].concat()),
        ];
        let mut got = vec![];
        for (t, p) in pkts {
            got.extend(n.inject(ao_net::chat::decode(t, &p).unwrap()));
        }
        assert_eq!(got.len(), 5);
        assert!(matches!(&got[0], Out::Social(ChatEvent::BuddyAdd { id: 7, online: 1, data }) if data == &[1]));
        assert!(matches!(&got[1], Out::Social(ChatEvent::PrivInvited(9))));
        assert!(matches!(&got[2], Out::Social(ChatEvent::PrivJoined { group: 9, who: 1 })));
        assert!(matches!(&got[3], Out::Social(ChatEvent::PrivMessage { group: 9, from: 7, .. })));
        assert!(matches!(&got[4], Out::Social(ChatEvent::LftReply(r)) if r.status == 2));
        // a looked-up tell target opens its window when the answer arrives
        let o = n.inject(ChatEvent::Lookup { id: 5, name: "Bob".into() });
        assert!(o.iter().all(|o| !matches!(o, Out::OpenTell { .. })));
        n.opens.push("bob".into());
        assert_eq!(n.inject(ChatEvent::Lookup { id: 5, name: "Bob".into() }), [Out::OpenTell { id: 5, name: "Bob".into() }]);
        assert_eq!(n.lookup_open("bob"), Some(5));
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
