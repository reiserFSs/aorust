//! Threaded login/zone client: `connect` -> `login` -> character list -> `select_character` ->
//! zone hand-off -> zone login + keepalive. Blocking socket I/O runs on one background thread;
//! the owner talks to it through [`LoginSession::login`] / [`select_character`] and
//! [`LoginSession::poll`]. Protocol details and evidence: docs/protocol.md.
//!
//! The password is moved into the session thread, used once for the `UserCredentials`
//! response and dropped; it is never logged, cloned or stored in an event.

use crate::conn::{Conn, Tap};
use crate::crypto::{login_server_pub, make_challenge_response_with};
use crate::frame::Frame;
use crate::msg::{CharacterList, Message};
use anyhow::{anyhow, bail, Result};
use num_bigint::BigUint;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const STATUS_URL: &str = "https://site.project-rk.com/api/status";
/// `version.id` of the PRK client this reimplements (docs/protocol.md §3).
pub const CLIENT_VERSION: &str = "00.7.2_EP1";
const PT_PING: u16 = 0xB;
const TICK: Duration = Duration::from_millis(100);
/// How long after `ZoneLogin` the first zone frames are collected before `ZoneConnected` fires.
const ZONE_COLLECT: Duration = Duration::from_secs(3);
const ZONE_COLLECT_MAX: usize = 16;
/// Own keepalive ping period on the zone connection. ponytail: guess; the original's period was
/// not found (`SendPingMessageToServer` caller unresolved), the server pings us anyway.
const ZONE_PING_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub name: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub players: u32,
}

/// `GET` [`STATUS_URL`]: `{"data":[{"loginIp","loginPort","serverName","count"}]}`.
pub fn fetch_servers() -> Result<Vec<ServerEntry>> {
    let v: serde_json::Value = ureq::get(STATUS_URL)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| anyhow!("{STATUS_URL}: {e}"))?
        .into_json()?;
    parse_servers(&v)
}

fn parse_servers(v: &serde_json::Value) -> Result<Vec<ServerEntry>> {
    v["data"]
        .as_array()
        .ok_or_else(|| anyhow!("status response has no `data` array"))?
        .iter()
        .map(|e| {
            Ok(ServerEntry {
                name: e["serverName"].as_str().ok_or_else(|| anyhow!("serverName"))?.to_owned(),
                ip: e["loginIp"].as_str().ok_or_else(|| anyhow!("loginIp"))?.parse()?,
                port: u16::try_from(e["loginPort"].as_u64().ok_or_else(|| anyhow!("loginPort"))?)?,
                players: e["count"].as_u64().unwrap_or(0) as u32,
            })
        })
        .collect()
}

/// One frame received from the zone server, for M3 planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneMsgSummary {
    pub ptype: u16,
    /// System messages: the `u32` message type. Other ptypes: first payload `u32` (N3 header word, unverified).
    pub msg_id: Option<u32>,
    pub sender: u32,
    pub receiver: u32,
    /// Payload length (without the 16-byte header).
    pub len: usize,
    /// First (up to) 32 payload bytes.
    pub head: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    Status(String),
    CharacterList(CharacterList),
    /// After this the session thread ends silently (no `Disconnected`).
    LoginError { code: u32, message: String },
    ZoneHandoff { zone_ip: Ipv4Addr, zone_port: u16, character_id: u32 },
    ZoneConnected { messages: Vec<ZoneMsgSummary> },
    Disconnected(String),
}

pub fn login_error_text(code: u32) -> String {
    match code {
        0x14 => "account is already logged in".into(),
        0x6A => "invalid username or password".into(),
        0x6C => "account banned or not paid".into(),
        c => format!("login refused (code {c:#x})"),
    }
}

enum Cmd {
    Login(String, String),
    Select(u32),
}

pub struct LoginSession {
    cmds: Sender<Cmd>,
    events: Receiver<LoginEvent>,
}

impl LoginSession {
    /// Blocking TCP connect (10 s timeout) to the login server; the session thread then waits for [`login`](Self::login).
    pub fn connect(server: &ServerEntry) -> Result<Self> {
        Self::connect_with(server, None)
    }

    /// As [`connect`](Self::connect) with a wire tap on the login connection (hex dumps; UserCredentials redacted).
    pub fn connect_traced(server: &ServerEntry, tap: Tap) -> Result<Self> {
        Self::connect_with(server, Some(tap))
    }

    fn connect_with(server: &ServerEntry, tap: Option<Tap>) -> Result<Self> {
        let mut conn = Conn::connect(SocketAddr::from((server.ip, server.port)))?;
        conn.tap = tap;
        Ok(Self::spawn(conn, login_server_pub()))
    }

    pub(crate) fn spawn(conn: Conn, server_pub: BigUint) -> Self {
        let (cmds, cmd_rx) = channel();
        let (ev_tx, events) = channel();
        thread::spawn(move || {
            let quiet = run(conn, &cmd_rx, &ev_tx, &server_pub);
            match quiet {
                Ok(true) => {}
                Ok(false) => {
                    let _ = ev_tx.send(LoginEvent::Disconnected("closed".into()));
                }
                Err(e) => {
                    let _ = ev_tx.send(LoginEvent::Disconnected(e.to_string()));
                }
            }
        });
        Self { cmds, events }
    }

    /// Start authentication. The password is consumed by the session thread and never stored elsewhere.
    pub fn login(&self, username: &str, password: &str) {
        let _ = self.cmds.send(Cmd::Login(username.to_owned(), password.to_owned()));
    }

    pub fn select_character(&self, character_id: u32) {
        let _ = self.cmds.send(Cmd::Select(character_id));
    }

    pub fn poll(&self) -> Option<LoginEvent> {
        self.events.try_recv().ok()
    }
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    SentLogin,
    SentCredentials,
    Listed,
    Selecting,
}

/// Login-server phase. `Ok(true)` = ended after a reported `LoginError` (no Disconnected event).
fn run(
    mut conn: Conn,
    cmds: &Receiver<Cmd>,
    ev: &Sender<LoginEvent>,
    server_pub: &BigUint,
) -> Result<bool> {
    let send = |e: LoginEvent| {
        let _ = ev.send(e);
    };
    let (mut name, mut password) = (String::new(), None::<String>);
    let mut phase = Phase::Idle;
    loop {
        match cmds.try_recv() {
            Ok(Cmd::Login(u, p)) if phase == Phase::Idle => {
                conn.send_message(&Message::UserLogin {
                    protocol: 2,
                    name: u.clone(),
                    client_version: CLIENT_VERSION.into(),
                })?;
                (name, password, phase) = (u, Some(p), Phase::SentLogin);
                send(LoginEvent::Status("contacting login server".into()));
            }
            Ok(Cmd::Select(id)) if phase == Phase::Listed => {
                conn.send_message(&Message::SelectCharacter { char_id: id as i32 })?;
                phase = Phase::Selecting;
                send(LoginEvent::Status("requesting zone".into()));
            }
            Ok(_) => send(LoginEvent::Status("command ignored in current state".into())),
            Err(TryRecvError::Disconnected) => return Ok(true),
            Err(TryRecvError::Empty) => {}
        }
        let Some(f) = conn.recv(TICK)? else { continue };
        if f.ptype == PT_PING {
            reply_ping(&mut conn, &f, 0)?;
            continue;
        }
        match Message::from_frame(&f) {
            Ok(Message::ServerSalt(salt)) if phase == Phase::SentLogin => {
                let Some(pw) = password.take() else { bail!("salt without pending login") };
                let (mut e, mut p) = ([0u8; 16], [0u8; 8]);
                getrandom::getrandom(&mut e)?;
                getrandom::getrandom(&mut p)?;
                let response = make_challenge_response_with(
                    server_pub,
                    &name,
                    &salt,
                    &pw,
                    &BigUint::from_bytes_be(&e),
                    p,
                )?;
                conn.send_message(&Message::UserCredentials { name: name.clone(), response })?;
                phase = Phase::SentCredentials;
                send(LoginEvent::Status("authenticating".into()));
            }
            Ok(Message::LoginError(code)) => {
                let code = code as u32;
                send(LoginEvent::LoginError { code, message: login_error_text(code) });
                return Ok(true);
            }
            Ok(Message::RequestRejected(detail)) => {
                send(LoginEvent::LoginError {
                    code: detail as u32,
                    message: format!("server rejected the login request (system message 0x21, detail {detail})"),
                });
                return Ok(true);
            }
            Ok(Message::CharacterList(l)) => {
                phase = Phase::Listed;
                send(LoginEvent::CharacterList(l));
            }
            Ok(Message::ZoneInfo(z)) => {
                send(LoginEvent::ZoneHandoff {
                    zone_ip: z.ip,
                    zone_port: z.port,
                    character_id: z.char_id as u32,
                });
                drop(conn);
                return zone(z, cmds, ev).map(|()| false);
            }
            Ok(m) => send(LoginEvent::Status(format!("login server: {m:?}"))),
            Err(_) => send(LoginEvent::Status(format!(
                "login server frame ptype={:#x} id={:#x?} ({} bytes) not decoded",
                f.ptype,
                f.system_parts().ok().map(|p| p.0),
                f.payload.len()
            ))),
        }
    }
}

/// Zone phase: connect (3 tries, 1 s / 2 s backoff), send `ZoneLogin`, answer pings, report the first frames.
fn zone(z: crate::msg::ZoneInfo, cmds: &Receiver<Cmd>, ev: &Sender<LoginEvent>) -> Result<()> {
    let addr = SocketAddr::from((z.ip, z.port));
    let mut delay = Duration::from_secs(1);
    let mut conn = loop {
        match Conn::connect(addr) {
            Ok(c) => break c,
            Err(e) if delay > Duration::from_secs(2) => return Err(e),
            Err(_) => {
                thread::sleep(delay);
                delay *= 2;
            }
        }
    };
    let id = z.char_id as u32;
    conn.send_message(&Message::ZoneLogin { char_id: z.char_id, cookie1: z.cookie1, cookie2: z.cookie2 })?;
    let _ = ev.send(LoginEvent::Status("zone login sent".into()));
    let (mut seen, mut announced) = (Vec::new(), false);
    let (start, mut last_ping) = (Instant::now(), Instant::now());
    loop {
        if matches!(cmds.try_recv(), Err(TryRecvError::Disconnected)) {
            return Ok(());
        }
        if let Some(f) = conn.recv(TICK)? {
            if f.ptype == PT_PING {
                reply_ping(&mut conn, &f, id)?;
            }
            if !announced {
                let s = summarize(&f);
                eprintln!("[ao-net] zone frame {s:?}");
                seen.push(s);
            }
        }
        if !announced && (seen.len() >= ZONE_COLLECT_MAX || start.elapsed() >= ZONE_COLLECT) {
            announced = true;
            let _ = ev.send(LoginEvent::ZoneConnected { messages: std::mem::take(&mut seen) });
        }
        if last_ping.elapsed() >= ZONE_PING_EVERY {
            last_ping = Instant::now();
            conn.send(ping_frame(1, id, 2, [0, ms_since_midnight(), 0, 0, 0], &[]))?;
        }
    }
}

fn summarize(f: &Frame) -> ZoneMsgSummary {
    ZoneMsgSummary {
        ptype: f.ptype,
        msg_id: f.payload.get(..4).map(|b| u32::from_be_bytes(b.try_into().unwrap())),
        sender: f.sender,
        receiver: f.receiver,
        len: f.payload.len(),
        head: f.payload[..f.payload.len().min(32)].to_vec(),
    }
}

fn ms_since_midnight() -> u32 {
    (SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis()) % 86_400_000) as u32
}

/// Ping frame (`PingMessage_t::CreateDataBlock`, MessageProtocol.dll 0x10002a9e): payload =
/// `type, f14, t_orig, t_recv, t_send, f24` (BE u32, `w` = `[f14, t_orig, t_recv, t_send, f24]`) + extra data.
fn ping_frame(kind: u32, sender: u32, receiver: u32, w: [u32; 5], extra: &[u8]) -> Frame {
    let mut payload = kind.to_be_bytes().to_vec();
    w.iter().for_each(|v| payload.extend_from_slice(&v.to_be_bytes()));
    payload.extend_from_slice(extra);
    Frame { seq: 0, ptype: PT_PING, sender, receiver, payload }
}

/// `Client_t::Receive` (Interfaces.dll 0x10001d36): a type-1 ping addressed to our character id
/// is answered with a type-2 copy (sender/receiver swapped, recv/send timestamps = now).
fn reply_ping(conn: &mut Conn, f: &Frame, char_id: u32) -> Result<()> {
    if f.receiver != char_id || f.payload.len() < 24 || f.payload[..4] != 1u32.to_be_bytes() {
        return Ok(());
    }
    let w = |i: usize| u32::from_be_bytes(f.payload[4 + 4 * i..8 + 4 * i].try_into().unwrap());
    let now = ms_since_midnight();
    conn.send(ping_frame(2, char_id, f.sender, [w(0), w(1), now, now, w(4)], &f.payload[24..]))
}

#[cfg(test)]
mod tests;
