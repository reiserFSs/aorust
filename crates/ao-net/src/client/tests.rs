//! State machine against in-process fake servers on loopback. Server frames for the failure
//! paths are the bytes captured from Ithaca (docs/protocol.md §8); no external network.

use super::*;
use crate::crypto::open_challenge_response;
use crate::frame::Frame;
use crate::msg::{CharacterEntry, CharacterInfo, ZoneInfo};
use std::io::Write;
use std::net::{TcpListener, TcpStream};

/// ServerSalt frame, Ithaca 199.241.136.157:7000, 2026-10 (probe1).
const SALT_FRAME: &str = "00010001000100340000000100000000000000243261656663666332656337656333373133663830653638386534366333643034";
/// LoginError 0x6A for a decryptable-but-unknown account (probe2).
const LOGIN_ERROR_FRAME: &str = "00020001000100180000000000001f83000000 0d0000006a";
/// 0x21 / detail 9, the answer to an undecryptable UserCredentials (probe4).
const REJECTED_FRAME: &str = "00020001000100180000000000001f83000000210000 0009";

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// Server end of one connection: framed reads, raw writes (so captured bytes replay verbatim).
struct Fake {
    rx: Conn,
    tx: TcpStream,
    seq: u16,
}

impl Fake {
    fn accept(l: &TcpListener) -> Fake {
        let (s, _) = l.accept().unwrap();
        Fake { tx: s.try_clone().unwrap(), rx: Conn::from_stream(s), seq: 0 }
    }
    fn recv(&mut self) -> Message {
        let f = self.rx.recv(Duration::from_secs(10)).unwrap().expect("client frame");
        Message::from_frame(&f).unwrap()
    }
    fn raw(&mut self, bytes: &[u8]) {
        self.tx.write_all(bytes).unwrap();
    }
    fn send(&mut self, m: &Message) {
        self.seq += 1;
        self.raw(&m.to_frame(self.seq).encode().unwrap());
    }
}

fn events_until(s: &LoginSession, done: impl Fn(&LoginEvent) -> bool) -> Vec<LoginEvent> {
    let end = Instant::now() + Duration::from_secs(15);
    let mut got = Vec::new();
    while Instant::now() < end {
        match s.poll() {
            Some(e) => {
                let stop = done(&e);
                got.push(e);
                if stop {
                    return got;
                }
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
    panic!("timed out; events so far: {got:?}");
}

const ZONE_DELAY: Duration = Duration::from_millis(700);

fn start(server_pub: BigUint) -> (TcpListener, LoginSession) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let conn = Conn::connect(l.local_addr().unwrap()).unwrap();
    (l, LoginSession::spawn(conn, server_pub, ZONE_DELAY))
}

#[test]
fn captured_failure_replies_end_the_session_quietly() {
    for (frame, want) in [
        (LOGIN_ERROR_FRAME, LoginEvent::LoginError { code: 0x6A, message: "invalid username or password".into() }),
        (REJECTED_FRAME, LoginEvent::Rejected { code: 0x21, detail: 9 }),
    ] {
        let (l, s) = start(login_server_pub());
        s.login("aomac-probe", "not-a-real-password");
        let mut srv = Fake::accept(&l);
        let Message::UserLogin { protocol, name, client_version } = srv.recv() else { panic!() };
        assert_eq!((protocol, name.as_str(), client_version.as_str()), (2, "aomac-probe", CLIENT_VERSION));
        srv.raw(&hex(SALT_FRAME));
        let Message::UserCredentials { name, response } = srv.recv() else { panic!() };
        assert_eq!(name, "aomac-probe");
        assert!(response.contains('-') && !response.contains("not-a-real"));
        srv.raw(&hex(frame));
        let ev = events_until(&s, |e| matches!(e, LoginEvent::LoginError { .. } | LoginEvent::Rejected { .. }));
        assert_eq!(*ev.last().unwrap(), want);
        drop(srv); // server closes: no Disconnected after a reported error
        thread::sleep(Duration::from_millis(400));
        assert_eq!(s.poll(), None);
    }
}

fn entry(id: i32) -> CharacterEntry {
    CharacterEntry {
        id,
        created: true,
        info: CharacterInfo { id, name: "Testy".into(), breed: 1, gender: 2, profession: 6, level: 33, head: 4, ..Default::default() },
        ..Default::default()
    }
}

#[test]
fn login_select_zone_handoff_pings_and_first_zone_frames() {
    let server_priv = BigUint::from(0x1234_5678_9abc_def0u64);
    let server_pub = BigUint::from(5u32).modpow(&server_priv, &crate::crypto::dh_prime());
    let (l, s) = start(server_pub);
    let zone_l = TcpListener::bind("127.0.0.1:0").unwrap();
    let zone_port = zone_l.local_addr().unwrap().port();

    s.login("Testy", "hunter2");
    let mut srv = Fake::accept(&l);
    assert!(matches!(srv.recv(), Message::UserLogin { .. }));
    srv.raw(&hex(SALT_FRAME));
    srv.seq = 1;
    let Message::UserCredentials { response, .. } = srv.recv() else { panic!() };
    let (name, salt, pw) = open_challenge_response(&server_priv, &response).unwrap();
    assert_eq!((name.as_str(), pw.as_str()), ("Testy", "hunter2"));
    assert_eq!(salt, hex(SALT_FRAME)[20..].to_vec());

    let list = CharacterList { characters: vec![entry(77)], allowed_characters: 3, expansions: 255, sl_profs_enabled: 1 };
    srv.send(&Message::CharacterList(list.clone()));
    let ev = events_until(&s, |e| matches!(e, LoginEvent::CharacterList(_)));
    assert_eq!(*ev.last().unwrap(), LoginEvent::CharacterList(list));

    s.select_character(77);
    assert_eq!(srv.recv(), Message::SelectCharacter { char_id: 77 });
    srv.send(&Message::ZoneInfo(ZoneInfo {
        char_id: 77,
        ip: Ipv4Addr::LOCALHOST,
        port: zone_port,
        cookie1: 0xAABB_CCDD,
        cookie2: 0x1122_3344,
        event_server_type: 0,
        player_id: 5,
    }));
    let ev = events_until(&s, |e| matches!(e, LoginEvent::ZoneHandoff { .. }));
    assert_eq!(
        *ev.last().unwrap(),
        LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port, character_id: 77 }
    );

    // the zone connect happens only after the loadscreen delay; the login socket is still serviced
    zone_l.set_nonblocking(true).unwrap();
    thread::sleep(ZONE_DELAY / 3);
    assert!(zone_l.accept().is_err(), "connected before the loadscreen fade finished");
    zone_l.set_nonblocking(false).unwrap();
    let mut zone = Fake::accept(&zone_l);
    assert_eq!(zone.recv(), Message::ZoneLogin { char_id: 77, cookie1: 0xAABB_CCDD, cookie2: 0x1122_3344 });
    // server ping (type 1) addressed to the character, t_orig = 1000, then 15 N3-ish frames
    zone.seq += 1;
    let mut ping = vec![0u8; 0];
    for v in [1u32, 0, 1000, 0, 0, 7] {
        ping.extend_from_slice(&v.to_be_bytes());
    }
    zone.raw(&Frame { seq: zone.seq, ptype: PT_PING, sender: 2, receiver: 77, payload: ping }.encode().unwrap());
    for i in 0..15u32 {
        zone.seq += 1;
        let payload = [0xDEAD_0000 + i, 1, 2].iter().flat_map(|v| v.to_be_bytes()).collect();
        zone.raw(&Frame { seq: zone.seq, ptype: 0xA, sender: 9, receiver: 77, payload }.encode().unwrap());
    }
    let reply = zone.rx.recv(Duration::from_secs(10)).unwrap().unwrap();
    assert_eq!((reply.ptype, reply.sender, reply.receiver), (PT_PING, 77, 2));
    let w = |i: usize| u32::from_be_bytes(reply.payload[4 * i..4 * i + 4].try_into().unwrap());
    assert_eq!((w(0), w(2), w(5)), (2, 1000, 7)); // type 2, original timestamp and cookie echoed

    let ev = events_until(&s, |e| matches!(e, LoginEvent::ZoneConnected { .. }));
    let LoginEvent::ZoneConnected { messages } = ev.last().unwrap() else { unreachable!() };
    assert_eq!(messages.len(), ZONE_COLLECT_MAX);
    assert_eq!((messages[0].ptype, messages[0].msg_id), (PT_PING, Some(1)));
    assert_eq!((messages[1].ptype, messages[1].msg_id, messages[1].len), (0xA, Some(0xDEAD_0000), 12));
    drop(zone);
    let ev = events_until(&s, |e| matches!(e, LoginEvent::Disconnected(_)));
    assert!(matches!(ev.last().unwrap(), LoginEvent::Disconnected(_)));
}

/// Login + credentials against a fake server, returns the connected server end with the list already delivered.
fn listed(l: &TcpListener, s: &LoginSession, server_priv: &BigUint) -> Fake {
    s.login("Testy", "hunter2");
    let mut srv = Fake::accept(l);
    assert!(matches!(srv.recv(), Message::UserLogin { .. }));
    srv.raw(&hex(SALT_FRAME));
    srv.seq = 1;
    let Message::UserCredentials { response, .. } = srv.recv() else { panic!() };
    assert!(open_challenge_response(server_priv, &response).is_ok());
    let list = CharacterList { characters: vec![entry(77)], allowed_characters: 3, expansions: 255, sl_profs_enabled: 1 };
    srv.send(&Message::CharacterList(list));
    events_until(s, |e| matches!(e, LoginEvent::CharacterList(_)));
    srv
}

#[test]
fn create_delete_and_name_suggestion_in_the_char_select_phase() {
    let server_priv = BigUint::from(0x1234_5678_9abc_def0u64);
    let server_pub = BigUint::from(5u32).modpow(&server_priv, &crate::crypto::dh_prime());
    let (l, s) = start(server_pub);
    let mut srv = listed(&l, &s, &server_priv);

    s.request_random_name(1, 2, 6);
    assert_eq!(srv.recv(), Message::RandomNameRequest { breed: 1, gender: 2, profession: 6 });
    srv.send(&Message::SuggestName("Zorbak".into()));
    assert_eq!(*events_until(&s, |e| matches!(e, LoginEvent::RandomName(_))).last().unwrap(), LoginEvent::RandomName("Zorbak".into()));

    let req = CreateCharacterRequest {
        breed: 1, gender: 2, profession: 6, head: 4, height: 110, width: 1, name: "Zorbak".into(), starter_area: 0,
    };
    s.create_character(req.clone());
    assert_eq!(srv.recv(), Message::CreateCharacter(req.clone()));
    srv.send(&Message::NameInUse(0x1E));
    assert_eq!(
        *events_until(&s, |e| matches!(e, LoginEvent::CharacterCreateFailed { .. })).last().unwrap(),
        LoginEvent::CharacterCreateFailed { code: 0x1E }
    );
    // a LoginError during the char-select phase is reported but does not end the session
    srv.send(&Message::LoginError(0x14));
    assert!(matches!(events_until(&s, |e| matches!(e, LoginEvent::LoginError { .. })).last().unwrap(), LoginEvent::LoginError { code: 0x14, .. }));
    srv.send(&Message::RequestRejected(9));
    assert_eq!(*events_until(&s, |e| matches!(e, LoginEvent::Rejected { .. })).last().unwrap(), LoginEvent::Rejected { code: 0x21, detail: 9 });

    s.delete_character(77);
    assert_eq!(srv.recv(), Message::DeleteCharacter { char_id: 77 });
    srv.send(&Message::CharacterDeleted { char_id: None });
    assert_eq!(
        *events_until(&s, |e| matches!(e, LoginEvent::CharacterDeleted { .. })).last().unwrap(),
        LoginEvent::CharacterDeleted { character_id: 77 }
    );

    s.create_character(req);
    assert!(matches!(srv.recv(), Message::CreateCharacter(_)));
    srv.send(&Message::CharacterCreated { char_id: 99 });
    assert_eq!(
        *events_until(&s, |e| matches!(e, LoginEvent::CharacterCreated { .. })).last().unwrap(),
        LoginEvent::CharacterCreated { character_id: 99 }
    );
    // the original logs the new character in immediately
    assert_eq!(srv.recv(), Message::SelectCharacter { char_id: 99 });
}

#[test]
fn status_api_json() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"data":[{"loginIp":"199.241.136.157","loginPort":7000,"serverName":"Ithaca","count":199}]}"#,
    )
    .unwrap();
    assert_eq!(
        parse_servers(&v).unwrap(),
        vec![ServerEntry { name: "Ithaca".into(), ip: Ipv4Addr::new(199, 241, 136, 157), port: 7000, players: 199 }]
    );
    assert!(parse_servers(&serde_json::json!({})).is_err());
}
