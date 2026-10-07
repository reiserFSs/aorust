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
    crate::play::prefs::set_test_dir(&scratch);
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let mut p = Play::new(dir, None, None, None).unwrap();
    p.servers = Some(Ok(vec![ServerEntry { name: "t".into(), ip: Ipv4Addr::LOCALHOST, port, players: 0 }]));
    let mut host = Host::headless();
    p.frame(0.016, (1280, 800), &mut host); // first frame: Show(0)
    assert!(p.screen == Screen::Login && p.login_w.is_some());
    Some(Rig { p, host, port, _l: l })
}

#[test]
fn character_selection_expands_only_the_selected_row() {
    let Some(mut r) = rig() else { return };
    r.p.prefs.selected_character = -1;
    r.p.show_characters(fake_list(), &mut r.host);
    let name_tops = |p: &Play| p.rows.iter().map(|row| p.gui.frame_in(row.handle, "name_btn").unwrap().t).collect::<Vec<_>>();
    let collapsed = name_tops(&r.p);
    let compact_height = collapsed[1] - collapsed[0];
    for selected in 0..r.p.rows.len() {
        r.p.select_row(selected, &mut r.host);
        let tops = name_tops(&r.p);
        for i in 0..tops.len() - 1 {
            let height = tops[i + 1] - tops[i];
            if i == selected {
                assert!(height > compact_height, "selected row did not expand");
            } else {
                assert_eq!(height, compact_height, "unselected row reserves hidden details");
            }
        }
        let row = r.p.rows[selected].handle;
        let details = r.p.gui.frame_in(row, "detailed_view").unwrap();
        for field in ["gender", "location"] {
            let value = r.p.gui.frame_in(row, field).unwrap();
            assert!(value.t >= details.t && value.b <= details.b);
            assert!(!r.p.gui.text_in(row, field).is_empty());
        }
        assert_eq!(r.p.gui.text_in(row, "level"), r.p.chars[selected].info.level.to_string());
    }
}

#[test]
fn character_info_first_response_and_refresh_use_current_packet_stats() {
    let Some(mut r) = rig() else { return };
    r.p.zone = zone::Zone::new(42);
    r.p.chat = Some(super::super::chat::Chat::new());
    r.p.chat.as_mut().unwrap().show_url(&mut r.p.gui, &r.p.zone, &r.p.text, "charid://50000/77");
    let window = r.p.gui.window_ids().into_iter().find(|&w| r.p.gui.view_names(w).iter().any(|name| name == "BrowserView")).unwrap();
    assert!(r.p.gui.text(window, "BrowserView").contains("Transferring information"));
    for (title_level, killed, deaths) in [(3, 731, 29), (5, 947, 41)] {
        let mut w = ao_net::Writer::default();
        w.u32(ao_net::n3::info::INFO_PACKET);
        ao_net::msg::Identity { kind: 50000, instance: 77 }.write(&mut w);
        w.u8(0);
        // Legacy player response: flags, breed/profession/title/obsolete/visual profession.
        for byte in [0, 1, 1, title_level, 0, 1] { w.u8(byte); }
        w.i16(0);
        for value in [100, 100, 0, 0] { w.i32(value); }
        for value in ["First", "Last", "", ""] { w.str_i16(value); }
        for value in [killed, deaths, 0] { w.i32(value); }
        for _ in 0..8 { w.i32(0); }
        r.event(LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(0, 42, w.0)));
        let page = r.p.gui.text(window, "BrowserView");
        assert!(page.contains(&format!("TitleLevel {title_level}")), "{page}");
        for (key, value) in [("NumInvadersKilled", killed), ("KilledByInvaders", deaths)] {
            let label = r.p.text.by_key(506, key).unwrap();
            assert!(page.contains(&format!("{label}</font><font color=CCInfoText>{value}</font>")), "{page}");
        }
    }
}

#[test]
fn preview_first_waits_for_backdrop_instead_of_losing_upload() {
    let Some(mut r) = rig() else { return };
    r.p.backdrop = None;
    r.p.char_ready = false;
    r.p.worker = Some(preview::Worker::queued_first(Scene::default()));
    r.p.tick_preview(0.016, &mut r.host);
    assert!(!r.p.char_ready);
    r.p.backdrop = Some(Box::new(Scene::default()));
    r.p.tick_preview(0.016, &mut r.host);
    assert!(r.p.char_ready, "queued initial mesh must upload once the backdrop exists");
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
    assert!(r.host.live_sky.is_some(), "the world installed its live sky");
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
    assert!(matches!(r.p.zone.own_events.last(), Some(zone::OwnEvent::Place { pos, yaw, full_reset: true }) if *pos == [1.0, 2.0, 3.0] && (*yaw - std::f32::consts::PI).abs() < 1e-5));
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
    let path = live::route(&col, r.p.zone.own().unwrap().pos, (193.0, 157.0), &[], 0.0);
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

/// The live harness' `press=F8` and `drag=right:dx:dy`, headless: the player lens has the client's near plane, F8 puts the camera at the
/// head (first person), a right drag turns the character by `counts / 1000 * MouseTurnSensitivity` radians (docs/zone/camera.md §4, §5).
#[test]
fn first_person_and_mouse_look_from_the_input_events() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    for _ in 0..30 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    assert_eq!(r.host.lens.unwrap().near, 0.2, "the player camera's near plane (VisualCamera_t, CreateCamera)");
    let feet = |r: &Rig| {
        let p = r.p.zone.own().unwrap().pos;
        ao_render::Vec3::new(p[0], p[1], -p[2])
    };
    let d3 = (r.host.camera.pos - feet(&r)).length();
    assert!(d3 > 4.0 && d3 < 6.5, "third person: {d3} m from the feet");
    let tap = |r: &mut Rig, code| {
        for pressed in [true, false] {
            r.p.game_input(ao_render::GameInput::Key { code, pressed, repeat: false }, &mut r.host);
            r.p.frame(0.016, (1280, 800), &mut r.host);
        }
    };
    tap(&mut r, ao_render::KeyCode::F8);
    for _ in 0..30 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    let eye = r.host.camera.pos - feet(&r);
    assert!(eye.y > 1.2 && eye.y < 2.2 && eye.x.hypot(eye.z) < 0.3, "first person eye {eye:?}");
    // right drag: 100 counts at sensitivity 10 = 1 rad of character turn
    let yaw0 = r.p.zone.own().unwrap().yaw.unwrap_or(0.0);
    let (x, y, button) = (640.0, 400.0, ao_gui::MouseButton::Right);
    r.p.input(ao_gui::InputEvent::MouseDown { x, y, button }, &mut r.host);
    r.p.frame(0.016, (1280, 800), &mut r.host);
    for _ in 0..4 {
        r.p.game_input(ao_render::GameInput::MouseMotion { dx: 25.0, dy: 0.0 }, &mut r.host);
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    r.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut r.host);
    for _ in 0..60 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    let turned = (r.p.zone.own().unwrap().yaw.unwrap_or(0.0) - yaw0 + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI;
    assert!(turned.abs() > 0.5 && turned.abs() < 1.5, "turned {turned} rad");
}

/// The client-initiated teleport try in the flow (docs/zone/world.md §10.2): `StartTryingTeleport` = `TeleportStarted` while the playfield keeps
/// running (black viewport, game input stopped), the 30 s timeout = `TeleportFailed` = `TeleportEnded` (countdown restarts), and only the server's
/// echo of our `CharInPlay` (`AliveMessage`) brings the viewport and the input back.
#[test]
fn teleport_try_stops_the_input_until_the_char_in_play_echo() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    let frames = |r: &mut Rig, n: u32| (0..n).for_each(|_| drop(r.p.frame(0.016, (1280, 800), &mut r.host)));
    frames(&mut r, IN_PLAY_FRAMES + 2);
    assert!(r.p.zone.in_play_sent && !r.p.awaiting_alive && r.p.player.is_some());
    let key = |r: &mut Rig, pressed| r.p.game_input(ao_render::GameInput::Key { code: ao_render::KeyCode::KeyW, pressed, repeat: false }, &mut r.host);
    let walked = |r: &mut Rig| {
        let p0 = r.p.zone.own().unwrap().pos;
        key(r, true);
        frames(r, 60);
        key(r, false);
        let p1 = r.p.zone.own().unwrap().pos;
        ((p1[0] - p0[0]).powi(2) + (p1[2] - p0[2]).powi(2)).sqrt()
    };
    assert!(walked(&mut r) > 1.0, "the input is live before the try");

    // `StartTeleportTry` (`Player::teleport_try` found the own position in a teleportal): the old world stays, black under the GUI, input stopped
    assert!(r.p.zone.start_teleport_try());
    frames(&mut r, 1);
    assert!(r.p.teleporting && r.p.awaiting_alive && r.p.world_ready && r.p.player.is_some() && r.p.zone.trier.is_some());
    let list = r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(matches!(list.cmds.get(1), Some(DrawCmd::Solid { color: [0, 0, 0], alpha, .. }) if *alpha == 1.0), "viewport off: black under the GUI");
    assert!(walked(&mut r) < 0.05, "InputConfig+0x18: the game keys are dropped");

    // no answer within 30 s: `TeleportFailed` posts `TeleportEnded`, the countdown for `CharInPlay` starts again, the input stays stopped
    r.p.zone.run_trier(31.0);
    frames(&mut r, 1);
    assert!(r.p.zone.trier.is_none() && !r.p.teleporting && r.p.awaiting_alive && !r.p.zone.in_play_sent);
    assert!(r.p.text.by_key(110, "Feedback_AreaChangeNotInitiated").is_some());
    frames(&mut r, IN_PLAY_FRAMES + 2);
    assert!(r.p.zone.in_play_sent && r.p.awaiting_alive, "CharInPlay is sent again, the echo is still missing");
    assert!(walked(&mut r) < 0.05);

    // the server relays our own `CharInPlayIIR_t`: `AliveMessage` (another character's relay does nothing)
    let echo = |id: u32| LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(0, id, ao_net::n3::outgoing::char_in_play(id as i32)));
    r.event(echo(33401));
    assert!(r.p.awaiting_alive);
    r.event(echo(33512));
    assert!(!r.p.awaiting_alive);
    let list = r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(!matches!(list.cmds.get(1), Some(DrawCmd::Solid { color: [0, 0, 0], alpha, .. }) if *alpha == 1.0), "the viewport is back");
    assert!(walked(&mut r) > 1.0, "EnableUserInput");
}

/// `N3Msg_IsDungeon` (`Report::dungeon`): a dungeon always says `EnteringNewArea`, a named outdoor playfield `EnteringPF` with its name.
#[test]
fn entering_text_of_a_dungeon_is_the_new_area_line() {
    let Some(mut r) = rig() else { return };
    r.p.zone.playfield = Some(4582);
    r.p.pf_names.insert(4582, "Newland City".into());
    assert_eq!(r.p.entering_line().as_deref(), Some("Entering 'Newland City'"));
    r.p.dungeon = true;
    assert_eq!(r.p.entering_line(), r.p.text.by_key(110, "EnteringNewArea"));
    assert_ne!(r.p.entering_line().as_deref(), Some("Entering 'Newland City'"));
    r.p.pf_names.clear();
    r.p.dungeon = false;
    assert_eq!(r.p.entering_line(), r.p.text.by_key(110, "EnteringNewArea"), "no name: the same line");
}

/// Post-creation flow (live harness path): `CharacterCreated` + `ZoneHandoff` while the Name scene is up -> `StopScene(0x65)` -> the exit
/// cinematic (docs/screens.md §12) -> loading screen; the zone frames of a new character's capture then bring the world (`Screen::InWorld`).
#[test]
fn character_created_and_handoff_reach_the_world() {
    let Some(mut r) = rig() else { return };
    r.p.show_characters(fake_list(), &mut r.host);
    r.p.start_creation(&mut r.host);
    let t = Instant::now();
    while !r.p.live_create("Aomactest", 1, 1) {
        assert!(t.elapsed() < Duration::from_secs(120), "creation module did not start");
        r.p.frame(0.05, (1280, 800), &mut r.host);
        std::thread::sleep(Duration::from_millis(20));
    }
    r.event(LoginEvent::CharacterCreated { character_id: 33512 });
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    // zone frames may arrive while the exit cinematic still runs
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    let t = Instant::now();
    while r.p.screen == Screen::Create {
        assert!(t.elapsed() < Duration::from_secs(60), "the exit cinematic never handed over");
        r.p.frame(0.25, (1280, 800), &mut r.host);
    }
    assert!(r.p.screen == Screen::Loading && r.p.cc.is_none());
    r.enter();
    assert!(r.p.player.is_some());
}

/// Leaving the world for the login screen (`/camp`, `ActivateGameClosing(2)`): the chat windows close and the chat/interact layers are
/// dropped (their chat-server session ends), so nothing of the world stays over the login backdrop and a re-entry opens no duplicates.
#[test]
fn leaving_the_world_closes_chat_and_interact() {
    let Some(mut r) = rig() else { return };
    let first = captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec"));
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&first);
    r.enter();
    for _ in 0..IN_PLAY_FRAMES + 2 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    assert!(r.p.chat.is_some() && r.p.interact.is_some() && r.p.gui.window_ids().len() > 3);
    r.p.show_login(&mut r.host);
    assert!(r.p.chat.is_none() && r.p.interact.is_none() && r.p.hud.is_none());
    let left: Vec<_> = r.p.gui.window_ids().into_iter().map(|w| (w, r.p.gui.view_names(w))).collect();
    assert_eq!(r.p.gui.window_ids(), vec![r.p.login_w.unwrap()], "only the login window is left: {left:?}");
}

/// `GetSexStr` (Gamecode): NONE / uni / male / female, first letter upper-cased; the Atrox are sex 1 = "Uni".
#[test]
fn sex_names_follow_the_get_sex_str_table() {
    assert_eq!([0, 1, 2, 3].map(sex_name), ["NONE", "Uni", "Male", "Female"]);
    assert_eq!(format!("{} {}", breed_name(4), sex_name(1)), "Atrox Uni");
}
