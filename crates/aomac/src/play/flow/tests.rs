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
