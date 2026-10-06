//! `#[ignore]`d live walk-through against a real zone server, rendered offscreen (no window, no desktop capture):
//! `printf 'user\npass\n' | AOMAC_LIVE_CHAR=Aomacvolk AOMAC_LIVE_SHOTS=/tmp/x AOMAC_LIVE_STEPS='wait=2,shot=a,W=3,shot=b' \
//!  cargo test --release -p aomac live_walk -- --ignored --nocapture`.
//! Credentials come from stdin only. Steps (comma separated): `KEY=secs` holds a letter/arrow/space key (`W A S D Z C SPACE UP LEFT`),
//! `wait=secs`, `shot=name` (PNG into AOMAC_LIVE_SHOTS), `pos` prints the own position.
use super::*;
use ao_render::{GameInput, KeyCode, Offscreen};
use std::io::BufRead;
use std::time::{Duration, Instant};

struct Live {
    p: Play,
    o: Offscreen,
    last: Instant,
    shots: Option<std::path::PathBuf>,
}

impl Live {
    fn tick(&mut self) -> ao_gui::DrawList {
        std::thread::sleep(Duration::from_millis(16));
        let dt = self.last.elapsed().as_secs_f32();
        self.last = Instant::now();
        self.o.frame(&mut self.p, dt)
    }
    fn until(&mut self, what: &str, secs: u64, f: impl Fn(&Play) -> bool) {
        let t = Instant::now();
        while !f(&self.p) {
            assert!(t.elapsed() < Duration::from_secs(secs), "timeout waiting for {what}");
            self.tick();
        }
    }
    fn wait(&mut self, secs: f32) {
        let t = Instant::now();
        while t.elapsed().as_secs_f32() < secs {
            self.tick();
        }
    }
    fn shot(&mut self, name: &str) {
        let list = self.tick();
        if let Some(dir) = &self.shots {
            let path = dir.join(format!("{name}.png"));
            self.o.png(&self.p, &list, &path).unwrap();
            eprintln!("shot {}", path.display());
        }
    }
    fn pos(&self) -> String {
        self.p.zone.own().map_or("?".into(), |d| format!("server pos {:.2} {:.2} {:.2} yaw {:.2}", d.pos[0], d.pos[1], d.pos[2], d.yaw.unwrap_or(0.0)))
    }
    fn key(&mut self, code: KeyCode, pressed: bool) {
        self.p.game_input(GameInput::Key { code, pressed, repeat: false }, &mut self.o.host);
    }
}

fn code(name: &str) -> KeyCode {
    match name {
        "W" => KeyCode::KeyW,
        "A" => KeyCode::KeyA,
        "S" => KeyCode::KeyS,
        "D" => KeyCode::KeyD,
        "Z" => KeyCode::KeyZ,
        "C" => KeyCode::KeyC,
        "SPACE" => KeyCode::Space,
        "UP" => KeyCode::ArrowUp,
        "LEFT" => KeyCode::ArrowLeft,
        "RIGHT" => KeyCode::ArrowRight,
        "BACKSPACE" => KeyCode::Backspace,
        n => panic!("unknown key {n}"),
    }
}

#[test]
#[ignore]
fn live_walk() {
    let dir = ao_gui::client_dir();
    let mut lines = std::io::stdin().lock().lines();
    let (user, pass) = (lines.next().unwrap().unwrap(), lines.next().unwrap().unwrap());
    let want = std::env::var("AOMAC_LIVE_CHAR").unwrap_or_else(|_| "Aomacvolk".into());
    std::env::set_var("AOMAC_PREFS_DIR", std::env::temp_dir().join("aomac-live-prefs"));
    let mut p = Play::new(dir, None, Some("Ithaca".into()), None).unwrap();
    p.servers = Some(ao_net::client::fetch_servers().map_err(|e| e.to_string()));
    let o = Offscreen::new(&p, (1280, 800)).unwrap();
    let shots = std::env::var_os("AOMAC_LIVE_SHOTS").map(std::path::PathBuf::from);
    if let Some(d) = &shots {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut l = Live { p, o, last: Instant::now(), shots };
    l.tick();
    let w = l.p.login_w.unwrap();
    l.p.gui.set_text(w, "username", &user);
    l.p.gui.set_text(w, "password", &pass);
    l.p.handle(Event::Clicked { window: w, view: "login_btn".into(), item: None }, &mut l.o.host);
    l.until("character list", 60, |p| p.screen == Screen::CharSelect);
    let i = l.p.chars.iter().position(|c| c.info.name == want).expect("character not on the account");
    l.p.select_row(i, &mut l.o.host);
    let cw = l.p.char_w.unwrap();
    l.p.handle(Event::Clicked { window: cw, view: "login_btn".into(), item: None }, &mut l.o.host);
    l.until("world", 120, |p| p.screen == Screen::InWorld && matches!(p.fade, Fade::Hold));
    eprintln!("in world: {} (player built: {})", l.pos(), l.p.player.is_some());
    for step in std::env::var("AOMAC_LIVE_STEPS").unwrap_or_else(|_| "wait=2,shot=enter".into()).split(',') {
        let (k, v) = step.split_once('=').unwrap_or((step, ""));
        match k {
            "wait" => l.wait(v.parse().unwrap()),
            "shot" => l.shot(v),
            "pos" => eprintln!("{}", l.pos()),
            key => {
                let c = code(key);
                l.key(c, true);
                l.wait(v.parse().unwrap());
                l.key(c, false);
                eprintln!("after {step}: {}", l.pos());
            }
        }
    }
    l.wait(1.0);
    eprintln!("final: {}", l.pos());
}
