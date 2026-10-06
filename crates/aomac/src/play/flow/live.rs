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

/// Presses W/S/C/Z so the character follows a [`route`] (it never turns: strafing does the sideways part).
pub(super) struct Pilot {
    path: Vec<(f32, f32)>,
    i: usize,
    held: Vec<&'static str>,
}

impl Pilot {
    pub(super) fn new(path: Vec<(f32, f32)>) -> Self {
        Pilot { path, i: 1, held: vec![] }
    }

    pub(super) fn held(&self) -> &[&'static str] {
        &self.held
    }

    pub(super) fn idx(&self) -> usize {
        self.i
    }

    fn set(&mut self, p: &mut Play, host: &mut Host, want: Vec<&'static str>) {
        for k in self.held.clone() {
            if !want.contains(&k) {
                p.game_input(GameInput::Key { code: code(k), pressed: false, repeat: false }, host);
            }
        }
        for k in &want {
            if !self.held.contains(k) {
                p.game_input(GameInput::Key { code: code(k), pressed: true, repeat: false }, host);
            }
        }
        self.held = want;
    }

    pub(super) fn release(&mut self, p: &mut Play, host: &mut Host) {
        self.set(p, host, vec![]);
    }

    /// One frame's decision; true = arrived.
    pub(super) fn step(&mut self, p: &mut Play, host: &mut Host) -> bool {
        let pos = p.zone.own().unwrap().pos;
        let dist = |c: (f32, f32)| (c.0 - pos[0]).abs() + (c.1 - pos[2]).abs();
        // the nearest waypoint ahead, aiming three cells past it
        while self.i + 1 < self.path.len() && dist(self.path[self.i + 1]) <= dist(self.path[self.i]) {
            self.i += 1;
        }
        let last = self.path.len() - 1;
        if self.i == last && dist(self.path[last]) < 0.6 {
            return true;
        }
        let tgt = self.path[(self.i + 3).min(last)];
        let (dx, dz) = (tgt.0 - pos[0], tgt.1 - pos[2]);
        let want = [(dz < -0.2, "W"), (dz > 0.2, "S"), (dx < -0.2, "C"), (dx > 0.2, "Z")].iter().filter(|w| w.0).map(|w| w.1).collect();
        self.set(p, host, want);
        false
    }
}

/// Axis-aligned route over the 0.5 m cells the collision lets a 0.35 m capsule cross (server x, z): `(x, z)` waypoints.
pub(super) fn route(c: &ao_formats::playfield::collision::Collision, from: [f32; 3], to: (f32, f32)) -> Vec<(f32, f32)> {
    use std::collections::{HashMap, VecDeque};
    let cell = 0.5f32;
    let at = |x: f32, z: f32| ((x / cell).round() as i32, (z / cell).round() as i32);
    let (start, goal) = (at(from[0], from[2]), at(to.0, to.1));
    let pos = |n: (i32, i32)| [n.0 as f32 * cell, from[1], -(n.1 as f32 * cell)];
    let mut prev: HashMap<(i32, i32), (i32, i32)> = HashMap::from([(start, start)]);
    let mut q = VecDeque::from([start]);
    while let Some(n) = q.pop_front() {
        if n == goal {
            break;
        }
        for d in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let m = (n.0 + d.0, n.1 + d.1);
            let (a, b) = (pos(n), pos(m));
            if prev.contains_key(&m) || c.ground([b[0], b[1] + 0.4, b[2]]).is_none() {
                continue;
            }
            let mut p = a;
            let ok = (1..=5).all(|i| {
                let t = i as f32 / 5.0;
                let want = [a[0] + (b[0] - a[0]) * t, a[1], a[2] + (b[2] - a[2]) * t];
                p = c.slide(p, want, 0.35, 1.8);
                (p[0] - want[0]).abs() + (p[2] - want[2]).abs() < 0.03
            });
            if ok {
                prev.insert(m, n);
                q.push_back(m);
            }
        }
    }
    let end = if prev.contains_key(&goal) { goal } else { *prev.keys().min_by_key(|n| (n.0 - goal.0).pow(2) + (n.1 - goal.1).pow(2)).unwrap() };
    let mut path = vec![end];
    let mut n = end;
    while n != start {
        n = prev[&n];
        path.push(n);
    }
    path.reverse();
    path.iter().map(|n| (n.0 as f32 * cell, n.1 as f32 * cell)).collect()
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
        "Q" => KeyCode::KeyQ,
        "X" => KeyCode::KeyX,
        "B" => KeyCode::KeyB,
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
    eprintln!("in world: {} (player built: {}, focus {:?})", l.pos(), l.p.player.is_some(), l.p.gui.focused_view());
    for step in std::env::var("AOMAC_LIVE_STEPS").unwrap_or_else(|_| "wait=2,shot=enter".into()).split(',') {
        let (k, v) = step.split_once('=').unwrap_or((step, ""));
        match k {
            "wait" => l.wait(v.parse().unwrap()),
            "shot" => l.shot(v),
            "pos" => eprintln!("{}", l.pos()),
            // combat steps: `near` lists the dynels within 60 m, `tab` = TAB (next hostile target), `sel=<instance>` selects,
            // `fight` prints the combat layer's state (Q / X / B hold the keys: attack / sit / brawl)
            "near" => {
                let me = l.p.zone.own().unwrap().pos;
                let mut v: Vec<_> = l.p.zone.dynels.iter().map(|(i, d)| (((d.pos[0] - me[0]).powi(2) + (d.pos[2] - me[2]).powi(2)).sqrt(), *i, d.clone())).filter(|e| e.0 < 60.0).collect();
                v.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (dist, i, d) in v {
                    eprintln!("near {i} {:?} npc={} lvl={} hp={}/{} side={} dist={dist:.1}", d.name, d.npc, d.level, d.health, d.max_health, d.side);
                }
            }
            "tab" => {
                l.p.input(ao_gui::InputEvent::Key { key: ao_gui::Key::Tab, pressed: true, mods: ao_gui::Modifiers::default() }, &mut l.o.host);
                eprintln!("target {:?}", l.p.zone.target);
            }
            "sel" => l.p.zone.target = Some(v.parse().unwrap()),
            "fight" => {
                let m = l.p.fight.as_ref().unwrap();
                eprintln!("fight: attacking={} numbers={:?}", m.attacking(), m.numbers().iter().map(|n| n.text.as_str()).collect::<Vec<_>>());
            }
            "goto" => {
                // autopilot along a collision route: W/S/C/Z by the offset to the next waypoint (the heading stays put)
                // `goto=<instance>` walks to the dynel (2 m short of it is close enough: the route ends on its cell)
                let resolved;
                let v = match v.parse::<i32>() {
                    Ok(id) => {
                        let d = &l.p.zone.dynels[&id];
                        resolved = format!("{}:{}", d.pos[0], d.pos[2]);
                        resolved.as_str()
                    }
                    Err(_) => v,
                };
                let (x, z) = v.split_once(':').unwrap();
                let col = ao_formats::playfield::collision::Collision::load(&ao_rdb::RecordStore::open(&ao_gui::client_dir()).unwrap(), l.p.zone.playfield.unwrap()).unwrap();
                let path = route(&col, l.p.zone.own().unwrap().pos, (x.parse().unwrap(), z.parse().unwrap()));
                eprintln!("route of {} cells", path.len());
                let mut pilot = Pilot::new(path);
                let t = Instant::now();
                loop {
                    l.tick();
                    if pilot.step(&mut l.p, &mut l.o.host) || t.elapsed().as_secs() > 120 {
                        break;
                    }
                }
                pilot.release(&mut l.p, &mut l.o.host);
                eprintln!("after goto {v}: {}", l.pos());
            }
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
