//! Headless tests of the login flow against the real client GUI data (skipped without the client): the `Play` state
//! machine is driven through `handle`/`pump`/`frame`, no window or renderer. Error pages are recorded, never opened.
use super::*;
use std::net::{Ipv4Addr, TcpListener};
use std::time::{Duration, Instant};

struct Rig {
    p: Play,
    host: Host,
    port: u16,
    _l: TcpListener,
}

fn rig() -> Option<Rig> {
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/gui").exists() {
        eprintln!("skipping: no client at {}", dir.display());
        return None;
    }
    let scratch = std::env::temp_dir().join(format!("aomac-play-test-{}", std::process::id()));
    std::env::set_var("AOMAC_PREFS_DIR", &scratch);
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let mut p = Play::new(dir, None, None, None).unwrap();
    p.servers = Some(Ok(vec![ServerEntry { name: "t".into(), ip: Ipv4Addr::LOCALHOST, port, players: 0 }]));
    let mut host = Host::headless();
    p.frame(0.016, (1280, 800), &mut host); // first frame: Show(0)
    assert!(p.screen == Screen::Login && p.login_w.is_some());
    Some(Rig { p, host, port, _l: l })
}

impl Rig {
    fn click(&mut self, window: WindowId, view: &str) {
        self.p.handle(Event::Clicked { window, view: view.into(), item: None }, &mut self.host);
    }
    fn login(&mut self) {
        let w = self.p.login_w.unwrap();
        self.p.gui.set_text(w, "username", "aomac-verify");
        self.p.gui.set_text(w, "password", "not-a-password");
        self.click(w, "login_btn");
        assert!(matches!(self.p.screen, Screen::Progress { joining: false, .. }));
        assert!(!self.login_visible());
    }
    fn login_visible(&self) -> bool {
        self.p.gui.window_visible(self.p.login_w.unwrap())
    }
    fn cancel(&mut self) {
        let w = self.p.progress_w.unwrap();
        self.click(w, "cancel_btn");
        assert!(self.p.screen == Screen::Login && self.p.session.is_none() && self.login_visible());
    }
    fn pump_until(&mut self, what: &str, f: impl Fn(&Play) -> bool) {
        let t = Instant::now();
        while !f(&self.p) {
            assert!(t.elapsed() < Duration::from_secs(10), "timeout waiting for {what}");
            self.p.pump(&mut self.host);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn session(&self) -> LoginSession {
        LoginSession::connect(&ServerEntry { name: "t".into(), ip: Ipv4Addr::LOCALHOST, port: self.port, players: 0 }).unwrap()
    }
    fn event(&mut self, ev: LoginEvent) {
        self.p.fake_events.push_back(ev);
        self.p.pump(&mut self.host);
    }
}

#[test]
fn login_flow_headless() {
    let Some(mut r) = rig() else { return };
    let login_w = r.p.login_w;

    // Login -> real connect succeeds -> session installed; Cancel closes it and the login window is *shown again*, not rebuilt
    r.login();
    r.pump_until("connected", |p| p.session.is_some());
    let gen = r.p.conn_gen;
    r.cancel();
    assert_eq!(r.p.login_w, login_w, "LoginWindow is hidden/shown, not recreated");
    assert!(r.p.conn_gen > gen);
    assert_eq!(r.p.gui.text(login_w.unwrap(), "password"), "");

    // late results of the cancelled attempt: neither a session nor an error page
    r.login();
    let stale = r.p.conn_gen;
    r.cancel();
    let s = r.session();
    r.p.tx.send(Bg::Connected(stale, Ok(s))).unwrap();
    r.p.tx.send(Bg::Connected(stale, Err("boom".into()))).unwrap();
    r.p.pump(&mut r.host);
    // the attempt's own connect thread also reports (also stale); give it time to arrive
    std::thread::sleep(Duration::from_millis(300));
    r.p.pump(&mut r.host);
    assert!(r.p.session.is_none(), "stale Connected(Ok) must not install a session");
    assert!(r.p.screen == Screen::Login && r.p.opened_urls.is_empty(), "stale Connected(Err) must not open an error page");

    // progress timeout behaves like Cancel
    r.login();
    r.p.frame(CONNECT_TIMEOUT + 1.0, (1280, 800), &mut r.host);
    assert!(r.p.screen == Screen::Login && r.login_visible());
    let s = r.session();
    let g = r.p.conn_gen - 1;
    r.p.tx.send(Bg::Connected(g, Ok(s))).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    r.p.pump(&mut r.host);
    assert!(r.p.session.is_none() && r.p.screen == Screen::Login);

    // a current failed connect -> ShowError(1,0)
    r.p.servers = Some(Ok(vec![ServerEntry { name: "t".into(), ip: Ipv4Addr::LOCALHOST, port: 1, players: 0 }]));
    r.login();
    r.pump_until("connect error", |p| !p.opened_urls.is_empty());
    assert!(r.p.screen == Screen::Login && r.p.opened_urls[0].ends_with("/1.html"), "{:?}", r.p.opened_urls);

    // reply routing (SlotLoginReply): LoginError -> (0x0d, code), RequestRejected -> (0x21, detail) signed, lost -> (3,0)
    r.p.opened_urls.clear();
    r.event(LoginEvent::LoginError { code: 0x6a, message: String::new() });
    r.event(LoginEvent::Rejected { code: 0x21, detail: -1 });
    r.event(LoginEvent::Rejected { code: 0x21, detail: 7 });
    r.event(LoginEvent::Disconnected("x".into()));
    let tail: Vec<&str> = r.p.opened_urls.iter().map(|u| u.rsplit('/').next().unwrap()).collect();
    assert_eq!(tail, ["13-106.html", "33--1.html", "33-7.html", "3.html"]);
    assert!(r.p.screen == Screen::Login && r.login_visible());

    // a 0x21 during character creation is ShowError too (not the name-scene box) and leaves the creation module
    let list = fake_list();
    r.p.show_characters(list, &mut r.host);
    r.p.start_creation(&mut r.host);
    assert!(r.p.screen == Screen::Create && r.p.cc.is_some());
    r.p.opened_urls.clear();
    r.event(LoginEvent::Rejected { code: 0x21, detail: 5 });
    assert!(r.p.opened_urls.last().is_some_and(|u| u.ends_with("33-5.html")), "{:?}", r.p.opened_urls);
    assert!(r.p.screen == Screen::Login && r.p.cc.is_none() && r.login_visible());
}

/// Frames the server sent (`<`) in a capture of `docs/captures`.
fn captured(rec: &str) -> Vec<ao_net::frame::Frame> {
    rec.lines()
        .filter_map(|l| {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            (dir == "<").then(|| ao_net::frame::Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
        })
        .collect()
}

impl Rig {
    /// Frames until the loading screen has dissolved into the world (`Screen::InWorld`); the playfield loads in the background.
    fn enter(&mut self) {
        let t = Instant::now();
        while self.p.screen != Screen::InWorld {
            assert!(t.elapsed() < Duration::from_secs(120), "timeout waiting for the world");
            self.p.frame(0.5, (1280, 800), &mut self.host);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// A burst of captured server frames, one `pump` per frame.
    fn burst(&mut self, frames: &[ao_net::frame::Frame]) {
        for f in frames {
            self.event(LoginEvent::ZoneFrame(f.clone()));
        }
    }
    /// Frames until the teleport ended (the new world is shown again).
    fn settle(&mut self) {
        let t = Instant::now();
        while self.p.teleporting {
            assert!(t.elapsed() < Duration::from_secs(120), "timeout waiting for the new world");
            self.p.frame(0.5, (1280, 800), &mut self.host);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A second `PlayfieldAnarchyFIIR_t` while in the world: `TeleportStarted` (no loading screen, interface kept, world hidden), the
/// dynels dropped, the new playfield loaded, `CharInPlay` owed again after `TeleportEnded`; `GameTimeIIR_t` drives the zone clock
/// (docs/zone/world.md §10, docs/zone/world.md §10.2).
#[test]
fn second_playfield_keeps_the_interface_and_hides_the_world() {
    let Some(mut r) = rig() else { return };
    let first = captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec"));
    let second = captured(include_str!("../../../../../docs/captures/zone_ithaca.rec"));

    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    assert!(r.p.screen == Screen::Loading && r.p.zone.day_time() == ao_formats::playfield::DEFAULT_DAY_TIME);
    r.burst(&first);
    assert_eq!(r.p.zone.playfield, Some(4604));
    assert_eq!(r.p.zone.day_time(), 5229.0, "GameTimeIIR_t 78435 s of the 97200 s day, 15 game seconds per clock second");
    r.enter();
    assert!(r.p.world_ready || r.p.world_scene.is_none());
    for _ in 0..IN_PLAY_FRAMES + 2 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    assert!(r.p.zone.in_play_sent && !r.p.zone.dynels.is_empty());
    assert!(matches!(r.host.live_sky, Some(_)), "the world installed its live sky");
    r.host.live_sky = None;

    // the clock runs with the frames
    let t0 = r.p.zone.day_time();
    r.p.frame(2.0, (1280, 800), &mut r.host);
    assert!((r.p.zone.day_time() - t0 - 2.0).abs() < 1e-3);

    // the server sends another playfield: everything up to its `PlayfieldAnarchyF` frame is the old burst
    let pf = second.iter().position(|f| matches!(ao_net::n3::decode(f), Ok(m) if matches!(m.body, ao_net::n3::N3::World(ao_net::n3::world::World::Playfield(_))))).unwrap();
    r.burst(&second[..pf]);
    assert!(r.p.screen == Screen::InWorld);
    r.event(LoginEvent::ZoneFrame(second[pf].clone()));
    // `TeleportStartedMessage`: no loading screen, the HUD and chat stay, the world is hidden behind the GUI, input locked
    assert!(r.p.screen == Screen::InWorld && r.p.teleporting && !r.p.world_ready && !matches!(r.p.fade, Fade::In(_)));
    assert!(r.p.hud.is_some() && r.p.chat.is_some() && r.p.player.is_none());
    let list = r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(matches!(list.cmds.get(1), Some(DrawCmd::Solid { color: [0, 0, 0], alpha, .. }) if *alpha == 1.0), "black under the GUI");
    assert_eq!(r.p.text.by_key(110, "ChangingArea").as_deref(), Some("Changing area. Please wait."));
    assert!(r.p.zone.dynels.is_empty() && !r.p.zone.in_play_sent && r.p.zone.playfield == Some(4582));
    r.burst(&second[pf + 1..]);
    assert_eq!(r.p.zone.day_time(), 4478.0, "resynced by the new burst's GameTime (67170 s = 18:39:30)");
    assert!(r.host.sky_clock.take().is_some(), "the live sky clock is resynced");
    assert_eq!(r.p.world_frames, 0, "the countdown starts at TeleportEnded, not while the world loads");
    r.settle();
    assert!(r.p.hud.is_some() && matches!(r.p.fade, Fade::Hold), "TeleportEnded: no fade");
    for _ in 0..IN_PLAY_FRAMES + 2 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    assert!(r.p.zone.in_play_sent, "CharInPlay is due again in the new world");
    assert!(r.p.zone.own().is_none() && r.p.zone.dynels.len() > 10);
    assert!(matches!(r.host.live_sky, Some(Some(_))), "4582 is outdoors: live sky");
    assert_eq!(r.p.text.by_key(110, "EnteringPF").as_deref(), Some("Entering '%s'"));
}

/// `ZoneRedirection` (the session thread already reconnected): while in the world `TeleportStarted` runs before the new burst.
#[test]
fn zone_redirect_starts_the_teleport() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    r.event(LoginEvent::ZoneRedirect { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 2 });
    assert!(r.p.screen == Screen::InWorld && r.p.teleporting && r.p.zone.dynels.is_empty() && !r.p.zone.in_play_sent);
}

/// `n3TeleportIIR_t` for the own character (`StartTeleport`) starts the zone change before the playfield arrives, once (the
/// `m_isTeleporting` guard); in the same playfield it only places a dynel and starts nothing (docs/zone/world.md §10.2).
#[test]
fn teleport_iir_starts_the_zone_change_only_with_a_destination() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    let tp = |who: i32, dest: i32| {
        let proxy = ao_net::msg::PlayfieldProxy { exit_door_id: ao_net::msg::Identity { kind: 0x9C50, instance: dest }, ..Default::default() };
        let t = ao_net::n3::teleport::Teleport { target: ao_net::msg::Identity { kind: 50000, instance: who }, pos: [1.0, 2.0, 3.0], rot: [0.0, 1.0, 0.0, 0.0], proxy, ..Default::default() };
        LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(0, 1, t.encode(1)))
    };
    r.event(tp(33512, 0));
    assert!(!r.p.teleporting, "same playfield: no TeleportStarted");
    assert_eq!(r.p.zone.own().unwrap().pos, [1.0, 2.0, 3.0]);
    r.event(tp(99, 4604));
    assert!(!r.p.teleporting, "another character's destination is ignored");
    r.event(tp(33512, 4604));
    assert!(r.p.screen == Screen::InWorld && r.p.teleporting && r.p.hud.is_some() && r.p.zone.dynels.is_empty());
    r.event(tp(33512, 4604)); // `if (!m_isTeleporting)`: the second one does nothing
    assert!(r.p.teleporting);
}

/// The own character on the captured Arrival Hall start: the avatar is built, the login window's focus is gone (it blocked every key),
/// holding W walks it along its heading at about the client's run speed, and the position is the movement state's, not the server's.
#[test]
fn own_character_walks_from_the_keyboard() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    assert!(r.p.player.is_some() && !r.p.gui.text_focused());
    let (p0, yaw) = {
        let d = r.p.zone.own().unwrap();
        (d.pos, d.yaw.unwrap_or(0.0))
    };
    let key = |r: &mut Rig, pressed| r.p.game_input(ao_render::GameInput::Key { code: ao_render::KeyCode::KeyW, pressed, repeat: false }, &mut r.host);
    key(&mut r, true);
    for _ in 0..125 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    key(&mut r, false);
    let p1 = r.p.zone.own().unwrap().pos;
    let d = ((p1[0] - p0[0]).powi(2) + (p1[2] - p0[2]).powi(2)).sqrt();
    assert!(d > 4.0 && d < 12.0, "walked {d} m in 2 s");
    let along = (p1[0] - p0[0]) * yaw.sin() + (p1[2] - p0[2]) * yaw.cos();
    assert!(along > 0.9 * d, "moved along the heading {yaw}: {along} of {d}");
}

/// The route autopilot of the live harness, headless: Arrival Hall start -> the northernmost reachable spot of the collision grid
/// (collision + movement + avatar glue; fails when the character gets stuck on geometry the route calls free).
#[test]
fn autopilot_crosses_the_arrival_hall() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    let col = ao_formats::playfield::collision::Collision::load(&ao_rdb::RecordStore::open(&ao_gui::client_dir()).unwrap(), 4604).unwrap();
    let path = live::route(&col, r.p.zone.own().unwrap().pos, (193.0, 157.0));
    eprintln!("path {} {:?}", path.len(), &path[..path.len().min(12)]);
    let mut pilot = live::Pilot::new(path);
    let mut frames = 0;
    while !pilot.step(&mut r.p, &mut r.host) {
        r.p.frame(0.016, (1280, 800), &mut r.host);
        frames += 1;
        if frames % 30 == 0 && frames < 400 {
            eprintln!("f{frames} {:?} held {:?} i {}", r.p.zone.own().unwrap().pos, pilot.held(), pilot.idx());
        }
        assert!(frames < 6000, "stuck at {:?}", r.p.zone.own().unwrap().pos);
    }
    eprintln!("arrived after {frames} frames at {:?}", r.p.zone.own().unwrap().pos);
}
