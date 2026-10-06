//! Chat-server protocol: the `ppj::Client_c` statically linked into GUI.dll (0x1016c000..0x10173bff), an AOChat-family
//! protocol. Packet = `u16 type, u16 len, payload[len]` (big endian). Field codes (`FUN_1017161f` pack, `FUN_10171ae5`
//! unpack): `I` u32, `S`/`D` u16 length + bytes, `G` u8 kind + u32 id, `B` u8. Login = challenge string -> the same
//! half-DH + TEA response as the login server ([`crate::crypto`]), with the login server's public key (GUI 0x10173360,
//! server key string 0x101ca928, identical bytes to Interfaces.dll). Evidence and live captures: docs/chat/net.md.

use crate::conn::Tap;
use crate::crypto::make_challenge_response;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};
use num_bigint::BigUint;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// Group id on the wire (`G`): kind byte + u32 (the kind is the `Identity` type of the group; 0xE = private group).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GroupId {
    pub kind: u8,
    pub id: u32,
}

/// Private chat groups use the plain-`I` messages; this kind value routes a group message there
/// (`FUN_1016c9f4`: `kind == 0xe` -> type 0x39, else 0x41).
pub const KIND_PRIVATE_GROUP: u8 = 0xE;

/// One decoded `S2C_*` packet (names are the client's own error strings, GUI 0x101ca6xx).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    /// S2C_LOGIN_OK (0x05).
    LoggedIn,
    /// S2C_LOGIN_ERROR (0x06).
    LoginFailed,
    /// Socket closed / protocol error.
    Disconnected(String),
    /// S2C_USER_NAME (0x14) `IS`.
    UserName { id: u32, name: String },
    /// S2C_LOOKUP_NAME_RES (0x15) `IS`; `id == u32::MAX` = unknown name.
    Lookup { id: u32, name: String },
    /// S2C_USER_FLAGS (0x16) `IB`.
    UserFlags { id: u32, flags: u8 },
    /// S2C_MESSAGE (0x1e) `ISD`: a tell.
    Tell { from: u32, text: String, data: Vec<u8> },
    /// S2C_VIS_MESSAGE_FMT (0x22) `ISD` sender id, text, data: a character's vicinity/shout/whisper text (the server echoes our own
    /// message back with our id; live, docs/chat/live.md).
    Vicinity { from: u32, text: String, data: Vec<u8> },
    /// S2C_VIS_ANON_MESSAGE (0x23) `SSD` name (empty in every capture), text, data: server notices (MOTD, "This player is currently offline").
    VicinityAnon { name: String, text: String, data: Vec<u8> },
    /// S2C_SYS_MESSAGE (0x24) `S`.
    System(String),
    /// S2C_SYS_MESSAGE_LOCAL_FMT (0x25) `IIID` + arguments: text id `text_id` of category 20000 with `args`.
    SystemFmt { sender: u32, kind: u32, text_id: u32, args: Vec<FmtArg> },
    /// S2C_ADD_BUDDY (0x28) `IID`.
    BuddyAdd { id: u32, online: u32, data: Vec<u8> },
    /// S2C_REM_BUDDY (0x29) `I`.
    BuddyRemove(u32),
    /// S2C_PRIVGRP_INVITED (0x32) `I`.
    PrivInvited(u32),
    /// S2C_PRIVGRP_KICKED (0x33) `I`.
    PrivKicked(u32),
    /// S2C_PRIVGRP_JOINED (0x37) `II`.
    PrivJoined { group: u32, who: u32 },
    /// S2C_PRIVGRP_PARTED (0x38) `II`.
    PrivParted { group: u32, who: u32 },
    /// S2C_PRIVGRP_MESSAGE (0x39) `IISD`.
    PrivMessage { group: u32, from: u32, text: String, data: Vec<u8> },
    /// S2C_PRIVGRP_DECLINED (0x3a) `II`.
    PrivDeclined { group: u32, who: u32 },
    /// S2C_GROUP_JOIN (0x3c) `GSID`: a channel the character is (or can be) in.
    GroupJoin { group: GroupId, name: String, flags: u32, data: Vec<u8> },
    /// S2C_GROUP_PART (0x3d) `G`.
    GroupPart(GroupId),
    /// S2C_GROUP_MESSAGE (0x41) `GISD`.
    GroupMessage { group: GroupId, from: u32, text: String, data: Vec<u8> },
    /// S2C_PONG (0x64) `D`.
    Pong(Vec<u8>),
    /// S2C_LFT_QUERY_RESULT (0x5dd, format `BISIIBBS`, GUI 0x1016f037) = `ppj::Client_c::LftQueryReply_t` (action kind 10).
    LftReply(LftReply),
    /// Any other type (raw, for logging): S2C_FORWARD_DATA 0x6e, S2C_ADM_MUX_INFO 0x44c (three lists, GUI 0x1016f103), ... not decoded.
    Other { ptype: u16, payload: Vec<u8> },
}

/// One LFT search result (S2C 0x5dd). `ChatGUIModule_c::HandleLFTMessage` [GUI 0x10087069]: `status` 0 = a candidate (emitted on
/// GlobalSignals+0x1f0 as `(id, name, level, profession, playfield, side, description)`), 2 = end of the result list
/// (emitted with id 0 / empty strings / -1s: re-enables the Search button), anything else is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LftReply {
    pub status: u8,
    pub id: u32,
    pub name: String,
    pub level: u32,
    /// Playfield id (`N3Msg_GetPFName`).
    pub playfield: u32,
    /// Text id of category 2005 (0 neutral, 1 clan, 2 omni).
    pub side: u8,
    /// Text id of category 2004 (1..15).
    pub profession: u8,
    pub description: String,
}

/// Argument of [`ChatEvent::SystemFmt`]: type char 'I' / 'S' / 'l' (= text-db id of category 20000) followed by its value.
#[derive(Debug, Clone, PartialEq)]
pub enum FmtArg {
    Int(u32),
    Str(String),
    TextId(u32),
}

fn data(r: &mut Reader) -> Result<Vec<u8>> {
    let n = r.u16()? as usize;
    Ok(r.bytes(n)?.to_vec())
}
fn string(r: &mut Reader) -> Result<String> {
    let n = r.u16()? as usize;
    Ok(String::from_utf8_lossy(r.bytes(n)?).into_owned())
}
fn group(r: &mut Reader) -> Result<GroupId> {
    Ok(GroupId { kind: r.u8()?, id: r.u32()? })
}

/// Decode one packet. The challenge (0x00) is handled by the session and returned as `Other`.
pub fn decode(ptype: u16, payload: &[u8]) -> Result<ChatEvent> {
    let mut r = Reader::new(payload);
    let r = &mut r;
    Ok(match ptype {
        0x05 => ChatEvent::LoggedIn,
        0x06 => ChatEvent::LoginFailed,
        0x14 => ChatEvent::UserName { id: r.u32()?, name: string(r)? },
        0x15 => ChatEvent::Lookup { id: r.u32()?, name: string(r)? },
        0x16 => ChatEvent::UserFlags { id: r.u32()?, flags: r.u8()? },
        0x1e => ChatEvent::Tell { from: r.u32()?, text: string(r)?, data: data(r)? },
        0x22 => ChatEvent::Vicinity { from: r.u32()?, text: string(r)?, data: data(r)? },
        0x23 => ChatEvent::VicinityAnon { name: string(r)?, text: string(r)?, data: data(r)? },
        0x24 => ChatEvent::System(string(r)?),
        0x25 => {
            let (sender, kind, text_id) = (r.u32()?, r.u32()?, r.u32()?);
            let desc = data(r)?;
            let mut args = vec![];
            for t in desc {
                args.push(match t {
                    b'I' => FmtArg::Int(r.u32()?),
                    b'S' => FmtArg::Str(string(r)?),
                    b'l' => FmtArg::TextId(r.u32()?),
                    t => bail!("SYS_MESSAGE_LOCAL_FMT: argument type {t}"),
                });
            }
            ChatEvent::SystemFmt { sender, kind, text_id, args }
        }
        0x28 => ChatEvent::BuddyAdd { id: r.u32()?, online: r.u32()?, data: data(r)? },
        0x29 => ChatEvent::BuddyRemove(r.u32()?),
        0x32 => ChatEvent::PrivInvited(r.u32()?),
        0x33 => ChatEvent::PrivKicked(r.u32()?),
        0x37 => ChatEvent::PrivJoined { group: r.u32()?, who: r.u32()? },
        0x38 => ChatEvent::PrivParted { group: r.u32()?, who: r.u32()? },
        0x39 => ChatEvent::PrivMessage { group: r.u32()?, from: r.u32()?, text: string(r)?, data: data(r)? },
        0x3a => ChatEvent::PrivDeclined { group: r.u32()?, who: r.u32()? },
        0x3c => ChatEvent::GroupJoin { group: group(r)?, name: string(r)?, flags: r.u32()?, data: data(r)? },
        0x3d => ChatEvent::GroupPart(group(r)?),
        0x41 => ChatEvent::GroupMessage { group: group(r)?, from: r.u32()?, text: string(r)?, data: data(r)? },
        0x64 => ChatEvent::Pong(data(r)?),
        0x5dd => ChatEvent::LftReply(LftReply {
            status: r.u8()?,
            id: r.u32()?,
            name: string(r)?,
            level: r.u32()?,
            playfield: r.u32()?,
            side: r.u8()?,
            profession: r.u8()?,
            description: string(r)?,
        }),
        _ => ChatEvent::Other { ptype, payload: payload.to_vec() },
    })
}

/// `C2S_*` requests (senders GUI 0x1016c8c9..0x1016cdab).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatCmd {
    /// 0x15 `S`: look up a character name -> [`ChatEvent::Lookup`].
    Lookup(String),
    /// 0x1e `ISD`.
    Tell { to: u32, text: String },
    /// 0x41 `GSD` (kind 0xE -> private group message 0x39 `ISD`).
    Group { group: GroupId, text: String },
    /// 0x28 `ID` / 0x29 `I`: buddy list. `D` is one byte: 1 = "add to buddy list" (menu action `user_id`, GUI 0x100a6940 via 0x100a7e73 / 0x100a6210),
    /// 0 = the temporary entry the client creates when it opens a tell window (0x100a5e7a) to follow the character's online state.
    BuddyAdd { id: u32, permanent: bool },
    BuddyRemove(u32),
    /// Private chat group (`/invite`, `/kick`, accept an invite, `/leave`): 0x32 `I` invite, 0x33 `I` kick (the player's id),
    /// 0x34 `I` join (the group = its owner's id) [GUI 0x1016c96e; caller 0x100a6e0c = accepting an invite], 0x35 `I` part
    /// [GUI 0x1016c988; `/leave <nick>`, declining]. Senders: 0x1016d57d (invite; class 3 of the action switch GUI 0x1008a5e0),
    /// 0x1016c954 (class 4), 0x1016c988 (class 5); docs/chat/cmd.md.
    PrivInvite(u32),
    PrivKick(u32),
    PrivJoin(u32),
    PrivPart(u32),
    /// 0x78 `sI`: `/cc <args..>` — the argument strings after `/cc` (each `ExpandChatTextArgs`'d) and the window id [GUI 0x1016cadf].
    Cc { args: Vec<String>, window: u32 },
    /// 0x6e `IM`: forward data to a character [GUI 0x1016cac2]. `/cc info <name>` sends `{"commane": "ccinfo", "destination": "chatserver"}`
    /// (GUI 0x1008947b branch type 1; which string is key and which value is inferred from the names).
    Forward { to: u32, entries: Vec<(String, String)> },
    /// 0x5dc `S`: looking-for-team ON with the team description [GUI 0x1016cafc]; 0x5dd (no fields): OFF [GUI 0x1016cb22].
    LftOn(String),
    LftOff,
    /// 0x5de `IIII`: search for looking-for-team characters [GUI 0x1016cb36, caller `FUN_100ef912` = the LFT window's Search button]:
    /// `side` = the Side dropdown's item id (7 "any" -> -1), `professions` = `1 << profession id` (item id 0x10 "any" -> -1),
    /// `location` = the Location dropdown's selected *index* (0 this playfield, 1 anywhere, 2 Rubi-Ka, 3 Shadowlands); the fourth word is always -1.
    LftQuery { side: u32, professions: u32, location: u32 },
    /// 0x40 `GID`: set group flags (mute etc.).
    GroupFlags { group: GroupId, flags: u32 },
    Quit,
}

fn put_s(w: &mut Writer, s: &[u8]) {
    w.u16(s.len().min(0xFFFF) as u16);
    w.bytes(&s[..s.len().min(0xFFFF)]);
}
fn put_group(w: &mut Writer, g: GroupId) {
    w.u8(g.kind);
    w.u32(g.id);
}
/// `D` of an outgoing tell (0x1e) without link attachment: one byte = the chat request's kind field, 0 for a tell
/// [GUI 0x1008947b -> 0x10089c00: `buf[0] = req.kind; buf[1..] = FUN_10089163(attachment)`, which returns 0 without attachment; 0x1016c8c9 gets `(buf, 1)`].
const TELL_DATA: &[u8] = &[0];
/// `D` of an outgoing group / private group message (0x41 / 0x39) without attachment: `(NULL, 0)`, an empty block
/// [GUI 0x1008a400..0x1008a436: `n = FUN_10089163(att, buf, 0x10000)`; `FUN_1016c9f4(.., n > 0 ? buf : NULL, n)`].
/// With an attachment (item link macro) the block is `pack("BBBSS", 1, b0, b1, s1, s2)` [FUN_10089163], not produced by this client.
const NO_DATA: &[u8] = &[];

/// Serialize `ptype` + payload into a wire packet.
pub fn packet(ptype: u16, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::default();
    w.u16(ptype);
    w.u16(body.len() as u16);
    w.bytes(body);
    w.0
}

pub fn encode(cmd: &ChatCmd) -> Option<Vec<u8>> {
    let mut w = Writer::default();
    let t = match cmd {
        ChatCmd::Lookup(n) => {
            put_s(&mut w, n.as_bytes());
            0x15
        }
        ChatCmd::Tell { to, text } => {
            w.u32(*to);
            put_s(&mut w, text.as_bytes());
            put_s(&mut w, TELL_DATA);
            0x1e
        }
        ChatCmd::Group { group, text } if group.kind == KIND_PRIVATE_GROUP => {
            w.u32(group.id);
            put_s(&mut w, text.as_bytes());
            put_s(&mut w, NO_DATA);
            0x39
        }
        ChatCmd::Group { group, text } => {
            put_group(&mut w, *group);
            put_s(&mut w, text.as_bytes());
            put_s(&mut w, NO_DATA);
            0x41
        }
        ChatCmd::BuddyAdd { id, permanent } => {
            w.u32(*id);
            put_s(&mut w, &[*permanent as u8]);
            0x28
        }
        ChatCmd::BuddyRemove(id) => {
            w.u32(*id);
            0x29
        }
        ChatCmd::PrivInvite(id) => {
            w.u32(*id);
            0x32
        }
        ChatCmd::PrivKick(id) => {
            w.u32(*id);
            0x33
        }
        ChatCmd::PrivJoin(id) => {
            w.u32(*id);
            0x34
        }
        ChatCmd::PrivPart(id) => {
            w.u32(*id);
            0x35
        }
        ChatCmd::Cc { args, window } => {
            // pack code `s` (GUI 0x1017161f): u16 count, then `S` strings
            w.u16(args.len().min(0xFFFF) as u16);
            for a in args.iter().take(0xFFFF) {
                put_s(&mut w, a.as_bytes());
            }
            w.u32(*window);
            0x78
        }
        ChatCmd::Forward { to, entries } => {
            // pack code `M`: u8 count, per entry `u8 (keylen << 4 | len >> 8), u8 len, key, value` (keys <= 16 bytes, values <= 0x1000)
            w.u32(*to);
            let ok: Vec<_> = entries.iter().filter(|(k, v)| k.len() <= 0x10 && v.len() <= 0x1000).take(0xFF).collect();
            w.u8(ok.len() as u8);
            for (k, v) in ok {
                w.u8((k.len() << 4) as u8 | (v.len() >> 8) as u8);
                w.u8(v.len() as u8);
                w.bytes(k.as_bytes());
                w.bytes(v.as_bytes());
            }
            0x6e
        }
        ChatCmd::LftOn(text) => {
            put_s(&mut w, text.as_bytes());
            0x5dc
        }
        ChatCmd::LftOff => 0x5dd,
        ChatCmd::LftQuery { side, professions, location } => {
            w.u32(*side);
            w.u32(*professions);
            w.u32(*location);
            w.u32(u32::MAX);
            0x5de
        }
        ChatCmd::GroupFlags { group, flags } => {
            put_group(&mut w, *group);
            w.u32(*flags);
            put_s(&mut w, NO_DATA);
            0x40
        }
        ChatCmd::Quit => return None,
    };
    Some(packet(t, &w.0))
}

/// `FUN_1016c832(this, 0, "IISS", 0, char_id, user, key)` (state 2 -> 3, GUI 0x1016dbb8..).
fn login_packet(char_id: u32, user: &str, key: &str) -> Vec<u8> {
    let mut w = Writer::default();
    w.u32(0);
    w.u32(char_id);
    put_s(&mut w, user.as_bytes());
    put_s(&mut w, key.as_bytes());
    packet(0, &w.0)
}

/// System message 0x43 (`Client_t::ProcessMessage`, IF 0x10002a9e; docs/zone/misc.md §15): the chat servers to connect to.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatServer {
    pub host: String,
    pub port: u16,
    /// Second word; always 0.0 in captures (unresolved: load/weight).
    pub extra: f32,
}

/// `payload` of a ptype-1 frame; `None` unless it is a well-formed 0x43 message.
pub fn parse_server_list(payload: &[u8]) -> Option<Vec<ChatServer>> {
    let mut r = Reader::new(payload);
    (r.u32().ok()? == 0x43).then_some(())?;
    let n = r.i32().ok()?;
    (0..n.clamp(0, 64)).map(|_| Some(ChatServer { host: r.str_i32(255).ok()?, port: r.i32().ok()? as u16, extra: r.f32().ok()? })).collect()
}

/// Idle time before the client pings (`FUN_1016f6cb`: > 59 s since the last receive, then every 30 s).
const PING_IDLE: Duration = Duration::from_secs(60);
const PING_EVERY: Duration = Duration::from_secs(30);
/// No packet for 5 minutes -> the client drops the connection.
const DEAD_AFTER: Duration = Duration::from_secs(300);
const TICK: Duration = Duration::from_millis(50);

enum Req {
    Login { user: String, password: String, char_id: u32 },
    Cmd(ChatCmd),
}

pub struct ChatSession {
    cmds: Sender<Req>,
    events: Receiver<ChatEvent>,
}

impl ChatSession {
    /// Blocking TCP connect (10 s timeout); the session thread then waits for the challenge and for [`login`](Self::login).
    pub fn connect(addr: impl ToSocketAddrs, tap: Option<Tap>) -> Result<Self> {
        let a = addr.to_socket_addrs()?.next().ok_or_else(|| anyhow::anyhow!("no address"))?;
        let s = TcpStream::connect_timeout(&a, Duration::from_secs(10))?;
        s.set_nodelay(true)?;
        let (cmds, cmd_rx) = channel();
        let (ev_tx, events) = channel();
        thread::spawn(move || {
            let why = match run(s, &cmd_rx, &ev_tx, tap) {
                Ok(()) => "closed".to_owned(),
                Err(e) => e.to_string(),
            };
            let _ = ev_tx.send(ChatEvent::Disconnected(why));
        });
        Ok(Self { cmds, events })
    }

    /// Answer the challenge as soon as it arrives. `char_id` = the logged-in character (`N3InterfaceModule::GetClientInst`).
    /// The password is consumed by the session thread.
    pub fn login(&self, user: &str, password: &str, char_id: u32) {
        let _ = self.cmds.send(Req::Login { user: user.into(), password: password.into(), char_id });
    }

    pub fn send(&self, c: ChatCmd) {
        let _ = self.cmds.send(Req::Cmd(c));
    }

    pub fn poll(&self) -> Option<ChatEvent> {
        self.events.try_recv().ok()
    }
}

fn run(mut s: TcpStream, cmds: &Receiver<Req>, ev: &Sender<ChatEvent>, mut tap: Option<Tap>) -> Result<()> {
    s.set_read_timeout(Some(TICK))?;
    let mut rx = Vec::<u8>::new();
    let mut login: Option<(String, String, u32)> = None;
    let mut challenge: Option<Vec<u8>> = None;
    let mut logged_in = false;
    let (mut last_rx, mut last_ping) = (Instant::now(), Instant::now());
    let send = |s: &mut TcpStream, tap: &mut Option<Tap>, bytes: &[u8], shown: Option<&[u8]>| -> Result<()> {
        if let Some(t) = tap {
            t(true, shown.unwrap_or(bytes));
        }
        s.write_all(bytes)?;
        Ok(())
    };
    loop {
        loop {
            match cmds.try_recv() {
                Ok(Req::Login { user, password, char_id }) => login = Some((user, password, char_id)),
                Ok(Req::Cmd(ChatCmd::Quit)) | Err(TryRecvError::Disconnected) => return Ok(()),
                Ok(Req::Cmd(c)) if logged_in => {
                    if let Some(b) = encode(&c) {
                        send(&mut s, &mut tap, &b, None)?;
                    }
                }
                Ok(Req::Cmd(_)) => {}
                Err(TryRecvError::Empty) => break,
            }
        }
        if let (Some(seed), Some((user, pw, id))) = (&challenge, &login) {
            let (mut e, mut p) = ([0u8; 16], [0u8; 8]);
            getrandom::getrandom(&mut e)?;
            getrandom::getrandom(&mut p)?;
            let key = make_challenge_response(user, seed, pw, &BigUint::from_bytes_be(&e), p)?;
            let real = login_packet(*id, user, &key);
            let mut shown = real.clone();
            let n = shown.len();
            shown[n - key.len()..].fill(b'*');
            send(&mut s, &mut tap, &real, Some(&shown))?;
            challenge = None;
            login = None;
        }
        let mut buf = [0u8; 4096];
        match s.read(&mut buf) {
            Ok(0) => bail!("connection closed by peer"),
            Ok(n) => rx.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e.into()),
        }
        while rx.len() >= 4 {
            let (t, n) = (u16::from_be_bytes([rx[0], rx[1]]), u16::from_be_bytes([rx[2], rx[3]]) as usize);
            if rx.len() < 4 + n {
                break;
            }
            last_rx = Instant::now();
            if let Some(t) = &mut tap {
                t(false, &rx[..4 + n]);
            }
            let body = rx[4..4 + n].to_vec();
            rx.drain(..4 + n);
            if t == 0 {
                // raw bytes: the seed is 32 random bytes, not text (docs/captures/chat_login_ithaca.rec)
                challenge = Some(data(&mut Reader::new(&body))?);
                continue;
            }
            let e = decode(t, &body).unwrap_or(ChatEvent::Other { ptype: t, payload: body });
            logged_in |= e == ChatEvent::LoggedIn;
            let stop = e == ChatEvent::LoginFailed;
            let _ = ev.send(e);
            if stop {
                return Ok(());
            }
        }
        let now = Instant::now();
        if now - last_rx > DEAD_AFTER {
            bail!("no data for {} s", DEAD_AFTER.as_secs());
        }
        if logged_in && now - last_rx > PING_IDLE && now - last_ping > PING_EVERY {
            // `FUN_1016c832(this, 100, "D", &2, 1)`
            send(&mut s, &mut tap, &packet(0x64, &[0, 1, 2]), None)?;
            last_ping = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn s(x: &str) -> Vec<u8> {
        let mut v = (x.len() as u16).to_be_bytes().to_vec();
        v.extend_from_slice(x.as_bytes());
        v
    }

    /// Live 2026-10-06 (Ithaca): the chat server's answer to the vicinity line `aomac vicinity test`: sender = own id, text, data = one byte 0.
    #[test]
    fn decodes_vicinity_echo_of_own_text() {
        let mut p = 0x82e8u32.to_be_bytes().to_vec();
        p.extend(s("aomac vicinity test"));
        p.extend(s("\0"));
        assert_eq!(decode(0x22, &p).unwrap(), ChatEvent::Vicinity { from: 0x82e8, text: "aomac vicinity test".into(), data: vec![0] });
    }

    #[test]
    fn decodes_tell_and_group_message() {
        let mut p = 7u32.to_be_bytes().to_vec();
        p.extend(s("hi"));
        p.extend(s("\0"));
        assert_eq!(decode(0x1e, &p).unwrap(), ChatEvent::Tell { from: 7, text: "hi".into(), data: vec![0] });
        let mut p = vec![3];
        p.extend(5u32.to_be_bytes());
        p.extend(9u32.to_be_bytes());
        p.extend(s("yo"));
        p.extend(s(""));
        assert_eq!(
            decode(0x41, &p).unwrap(),
            ChatEvent::GroupMessage { group: GroupId { kind: 3, id: 5 }, from: 9, text: "yo".into(), data: vec![] }
        );
        assert!(decode(0x1e, &p[..3]).is_err());
    }

    #[test]
    fn decodes_local_fmt_arguments() {
        let mut p = [1u32, 2, 3].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>();
        p.extend(s("ISl"));
        p.extend(42u32.to_be_bytes());
        p.extend(s("x"));
        p.extend(7u32.to_be_bytes());
        assert_eq!(
            decode(0x25, &p).unwrap(),
            ChatEvent::SystemFmt { sender: 1, kind: 2, text_id: 3, args: vec![FmtArg::Int(42), FmtArg::Str("x".into()), FmtArg::TextId(7)] }
        );
    }

    #[test]
    fn encodes_group_and_tell() {
        let g = GroupId { kind: 3, id: 5 };
        let b = encode(&ChatCmd::Group { group: g, text: "a".into() }).unwrap();
        assert_eq!(b, [0, 0x41, 0, 10, 3, 0, 0, 0, 5, 0, 1, b'a', 0, 0]); // D = (NULL, 0)
        let b = encode(&ChatCmd::Tell { to: 1, text: "a".into() }).unwrap();
        assert_eq!(b, [0, 0x1e, 0, 10, 0, 0, 0, 1, 0, 1, b'a', 0, 1, 0]); // D = one kind byte 0
        assert_eq!(encode(&ChatCmd::BuddyAdd { id: 1, permanent: true }).unwrap(), [0, 0x28, 0, 7, 0, 0, 0, 1, 0, 1, 1]);
        assert_eq!(encode(&ChatCmd::BuddyAdd { id: 1, permanent: false }).unwrap(), [0, 0x28, 0, 7, 0, 0, 0, 1, 0, 1, 0]);
    }

    /// Field codes `I`/`s`/`M` (pack function GUI 0x1017161f) and the private-group request ids (GUI 0x1016c8c9..).
    #[test]
    fn encodes_private_group_cc_forward_lft() {
        assert_eq!(encode(&ChatCmd::PrivInvite(5)).unwrap(), [0, 0x32, 0, 4, 0, 0, 0, 5]);
        assert_eq!(encode(&ChatCmd::PrivKick(5)).unwrap(), [0, 0x33, 0, 4, 0, 0, 0, 5]);
        assert_eq!(encode(&ChatCmd::PrivJoin(5)).unwrap(), [0, 0x34, 0, 4, 0, 0, 0, 5]);
        assert_eq!(encode(&ChatCmd::PrivPart(5)).unwrap(), [0, 0x35, 0, 4, 0, 0, 0, 5]);
        let cc = encode(&ChatCmd::Cc { args: vec!["addbuddy".into(), "Bob".into()], window: 2 }).unwrap();
        assert_eq!(cc, [0, 0x78, 0, 21, 0, 2, 0, 8, b'a', b'd', b'd', b'b', b'u', b'd', b'd', b'y', 0, 3, b'B', b'o', b'b', 0, 0, 0, 2]);
        let f = encode(&ChatCmd::Forward { to: 9, entries: vec![("ab".into(), "xyz".into())] }).unwrap();
        assert_eq!(f, [0, 0x6e, 0, 12, 0, 0, 0, 9, 1, 0x20, 3, b'a', b'b', b'x', b'y', b'z']);
        assert_eq!(encode(&ChatCmd::LftOn("hi".into())).unwrap(), [0x05, 0xdc, 0, 4, 0, 2, b'h', b'i']);
        assert_eq!(encode(&ChatCmd::LftOff).unwrap(), [0x05, 0xdd, 0, 0]);

        // 0x5de `IIII` (side, profession mask, location, -1) [GUI 0x1016cb36; caller FUN_100ef912]
        assert_eq!(
            encode(&ChatCmd::LftQuery { side: u32::MAX, professions: 1 << 3, location: 2 }).unwrap(),
            [0x05, 0xde, 0, 16, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 8, 0, 0, 0, 2, 0xff, 0xff, 0xff, 0xff]
        );
    }

    /// S2C 0x5dd `BISIIBBS` = status, id, name, level, playfield, side, profession, description (GUI 0x1016f037; fields per HandleLFTMessage).
    #[test]
    fn decodes_lft_reply() {
        let mut p = vec![0];
        p.extend(77u32.to_be_bytes());
        p.extend(s("Bob"));
        p.extend(123u32.to_be_bytes());
        p.extend(4001u32.to_be_bytes());
        p.extend([2, 6]);
        p.extend(s("need heals"));
        assert_eq!(
            decode(0x5dd, &p).unwrap(),
            ChatEvent::LftReply(LftReply { status: 0, id: 77, name: "Bob".into(), level: 123, playfield: 4001, side: 2, profession: 6, description: "need heals".into() })
        );
        assert!(decode(0x5dd, &p[..8]).is_err());
    }

    /// Live capture (Ithaca, 2026-10-05): challenge, login, OK, own name, the three MOTD lines as anonymous vicinity messages.
    #[test]
    fn live_login_capture_decodes() {
        let mut ev = vec![];
        for l in include_str!("../../../docs/captures/chat_login_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next(), p.next().unwrap(), p.next().unwrap());
            if dir != "<" {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            let n = u16::from_be_bytes([b[2], b[3]]) as usize;
            assert_eq!(b.len(), 4 + n);
            ev.push((u16::from_be_bytes([b[0], b[1]]), decode(u16::from_be_bytes([b[0], b[1]]), &b[4..]).unwrap()));
        }
        assert_eq!(ev[0].0, 0);
        assert_eq!(ev[1].1, ChatEvent::LoggedIn);
        assert_eq!(ev[2].1, ChatEvent::UserName { id: 33512, name: "Aomacvolk".into() });
        assert!(matches!(&ev[3].1, ChatEvent::VicinityAnon { text, .. } if text.starts_with("Welcome to Project Rubi-Ka!")));
        assert_eq!(ev.len(), 6);
    }

    #[test]
    fn server_list_from_zone_capture() {
        let l = include_str!("../../../docs/captures/zone_ithaca.rec")
            .lines()
            .find_map(|l| {
                let hex = l.split(' ').nth(2)?;
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                let (f, _) = crate::frame::Frame::decode_with(&b, false).ok()??;
                (l.split(' ').nth(1)? == "<").then(|| parse_server_list(&f.payload)).flatten()
            })
            .unwrap();
        assert_eq!(l, vec![ChatServer { host: "199.241.136.157".into(), port: 7005, extra: 0.0 }]);
    }

    /// A mock server: challenge -> login packet (type 0, `IISS`, key redacted by the tap) -> LOGIN_OK -> a tell round trip.
    #[test]
    fn session_logs_in_and_exchanges() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let srv = thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.write_all(&packet(0, &s("0123456789abcdef0123456789abcdef"))).unwrap();
            let mut h = [0u8; 4];
            c.read_exact(&mut h).unwrap();
            assert_eq!(h[..2], [0, 0]);
            let mut body = vec![0; u16::from_be_bytes([h[2], h[3]]) as usize];
            c.read_exact(&mut body).unwrap();
            let mut r = Reader::new(&body);
            assert_eq!((r.u32().unwrap(), r.u32().unwrap(), string(&mut r).unwrap()), (0, 33512, "usr".into()));
            let key = string(&mut r).unwrap();
            assert!(key.contains('-'), "{key}");
            c.write_all(&packet(5, &[])).unwrap();
            let mut t = [0u8; 4];
            c.read_exact(&mut t).unwrap();
            assert_eq!(t[..2], [0, 0x15]);
            let mut body = vec![0; u16::from_be_bytes([t[2], t[3]]) as usize];
            c.read_exact(&mut body).unwrap();
            let mut p = u32::MAX.to_be_bytes().to_vec();
            p.extend(s("Nobody"));
            c.write_all(&packet(0x15, &p)).unwrap();
            thread::sleep(Duration::from_millis(200));
        });
        let sess = ChatSession::connect(addr, None).unwrap();
        sess.login("usr", "pw", 33512);
        let mut got = vec![];
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) && !got.contains(&ChatEvent::Lookup { id: u32::MAX, name: "Nobody".into() }) {
            if let Some(e) = sess.poll() {
                if e == ChatEvent::LoggedIn {
                    sess.send(ChatCmd::Lookup("Nobody".into()));
                }
                got.push(e);
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got[0], ChatEvent::LoggedIn);
        assert!(got.contains(&ChatEvent::Lookup { id: u32::MAX, name: "Nobody".into() }), "{got:?}");
        srv.join().unwrap();
    }
}
