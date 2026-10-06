//! `#[ignore]`d live walk-through against a real zone server, rendered offscreen (no window, no desktop capture):
//! `printf 'user\npass\n' | AOMAC_LIVE_CHAR=Aomacvolk AOMAC_LIVE_SHOTS=/tmp/x AOMAC_LIVE_STEPS='wait=2,shot=a,W=3,shot=b' \
//!  cargo test --release -p aomac live_walk -- --ignored --nocapture`.
//! Credentials come from stdin only. Steps (comma separated): `KEY=secs` holds a letter/arrow/space key (`W A S D Z C SPACE UP LEFT`),
//! `wait=secs`, `shot=name` (PNG into AOMAC_LIVE_SHOTS), `pos` prints the own position, `press=F8|CTRL+F8|SHIFT+F8` taps keys (camera),
//! `drag=right:dx:dy` / `drag=left:dx:dy` mouse-look with raw counts, `cam` prints the camera and lens. HUD steps: `ui=u|ctrl+1` window hotkey,
//! `move=x:y` hover, `click=x:y`, `clickdyn=<instance>` world click on a dynel, `mdrag=x1:y1:x2:y2` GUI drag, `watch=secs` own-stat changes.
use super::*;
use ao_render::{GameInput, KeyCode, Offscreen};
use std::io::BufRead;
use std::time::{Duration, Instant};

struct Live {
    p: Play,
    o: Offscreen,
    last: Instant,
    shots: Option<std::path::PathBuf>,
    /// Longest frame time handed to the game (the autopilot sets it: a slow offscreen frame must not skip over a ramp edge).
    dt_cap: f32,
}

impl Live {
    fn tick(&mut self) -> ao_gui::DrawList {
        std::thread::sleep(Duration::from_millis(16));
        let dt = self.last.elapsed().as_secs_f32().min(self.dt_cap);
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
        let mode = self.p.player.as_ref().map_or(0, |p| p.mode());
        self.p.zone.own().map_or("?".into(), |d| format!("server pos {:.2} {:.2} {:.2} yaw {:.2} fsm mode {mode}", d.pos[0], d.pos[1], d.pos[2], d.yaw.unwrap_or(0.0)))
    }
    fn key(&mut self, code: KeyCode, pressed: bool) {
        // the window host tracks the modifier state from the modifier keys (`viewer.rs`): the hot keys read it as `host.mods`
        let m = &mut self.o.host.mods;
        match code {
            KeyCode::ShiftLeft => m.shift = pressed,
            KeyCode::ControlLeft => m.ctrl = pressed,
            KeyCode::AltLeft => m.alt = pressed,
            _ => {}
        }
        self.p.game_input(GameInput::Key { code, pressed, repeat: false }, &mut self.o.host);
    }
}

/// Presses W/S/C/Z so the character follows a [`route`] (it never turns: strafing does the sideways part, whatever the heading).
pub(super) struct Pilot {
    path: Vec<(f32, f32)>,
    i: usize,
    held: Vec<&'static str>,
    /// Swimming has no strafing: the pilot steers with the turn keys instead. `+1`: `A` raises the yaw; flipped when a turn made the heading error
    /// grow (`probe` = when the turn key went down and the error then).
    turn_sign: f32,
    probe: Option<(std::time::Instant, f32)>,
}

impl Pilot {
    pub(super) fn new(path: Vec<(f32, f32)>) -> Self {
        Pilot { i: 1.min(path.len().saturating_sub(1)), path, held: vec![], turn_sign: 1.0, probe: None }
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
        if p.player.as_ref().is_some_and(|pl| pl.mode() == 4) {
            // swimming (FSM mode 4): forward and turn only, towards a point four metres along the route
            let tgt = self.path[(self.i + 8).min(last)];
            let yaw = p.zone.own().unwrap().yaw.unwrap_or(0.0);
            let tau = 2.0 * std::f32::consts::PI;
            let err = ((tgt.0 - pos[0]).atan2(tgt.1 - pos[2]) - yaw + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
            let turn = (err.abs() > 0.3).then_some(if (err > 0.0) == (self.turn_sign > 0.0) { "A" } else { "D" });
            match (turn, self.probe) {
                (Some(_), None) => self.probe = Some((std::time::Instant::now(), err)),
                (Some(_), Some((t, e0))) if t.elapsed().as_secs_f32() > 0.6 => {
                    if err.abs() > e0.abs() + 0.1 {
                        self.turn_sign = -self.turn_sign;
                    }
                    self.probe = None;
                }
                (None, _) => self.probe = None,
                _ => {}
            }
            let mut want = vec![];
            want.extend(turn);
            if err.abs() < 1.2 {
                want.push("W");
            }
            self.set(p, host, want);
            return false;
        }
        let tgt = self.path[(self.i + 3).min(last)];
        let (dx, dz) = (tgt.0 - pos[0], tgt.1 - pos[2]);
        // the heading stays put, so the world offset goes to the keys through it: forward = (sin yaw, cos yaw), left = (-cos yaw, sin yaw)
        let (s, c) = p.zone.own().unwrap().yaw.unwrap_or(0.0).sin_cos();
        let (fwd, left) = (dx * s + dz * c, -dx * c + dz * s);
        let want = [(fwd > 0.2, "W"), (fwd < -0.2, "S"), (left > 0.2, "Z"), (left < -0.2, "C")].iter().filter(|w| w.0).map(|w| w.1).collect();
        self.set(p, host, want);
        false
    }
}

/// Axis-aligned route over the 0.5 m cells the collision lets the character (`Collision::walk`) cross (server x, z): `(x, z)` waypoints.
pub(super) fn route(c: &ao_formats::playfield::collision::Collision, from: [f32; 3], to: (f32, f32)) -> Vec<(f32, f32)> {
    use std::collections::{HashMap, VecDeque};
    let cell = 0.5f32;
    let at = |x: f32, z: f32| ((x / cell).round() as i32, (z / cell).round() as i32);
    let (start, goal) = (at(from[0], from[2]), at(to.0, to.1));
    // Breadth first over the lattice; `fall` also accepts stepping off a ledge (down at most 12 m) when no walkable route reaches the goal
    // (a character standing on an isolated ledge can only leave it that way).
    let search = |fall: bool| {
        // the ground position reached in every cell (the heights differ: slopes and cliffs decide what is walkable)
        let mut here: HashMap<(i32, i32), [f32; 3]> = HashMap::from([(start, [from[0], from[1], -from[2]])]);
        let mut prev: HashMap<(i32, i32), (i32, i32)> = HashMap::from([(start, start)]);
        let mut q = VecDeque::from([start]);
        while let Some(n) = q.pop_front() {
            if n == goal {
                break;
            }
            for d in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let m = (n.0 + d.0, n.1 + d.1);
                let a = here[&n];
                let b = [m.0 as f32 * cell, a[1], -(m.1 as f32 * cell)];
                let Some(g) = c.ground(b) else { continue };
                if prev.contains_key(&m) {
                    continue;
                }
                let mut p = a;
                let walked = (1..=5).all(|i| {
                    let t = i as f32 / 5.0;
                    let want = [a[0] + (b[0] - a[0]) * t, p[1], a[2] + (b[2] - a[2]) * t];
                    let w = c.walk(p, want);
                    p = w.pos;
                    !w.airborne && (p[0] - want[0]).abs() + (p[2] - want[2]).abs() < 0.03
                }) && (p[1] - a[1]).abs() <= ao_formats::playfield::collision::STEP_HEIGHT + 0.12 + if c.liquid_at(p).is_some() { 1.2 } else { 0.0 };
                if !walked {
                    let w = c.walk(a, [b[0], a[1], b[2]]);
                    let open = (w.pos[0] - b[0]).abs() + (w.pos[2] - b[2]).abs() < 0.03;
                    if !(fall && open && w.airborne && g < a[1] - 0.6 && a[1] - g <= 12.0) {
                        continue;
                    }
                    p = [b[0], g, b[2]];
                }
                prev.insert(m, n);
                here.insert(m, p);
                q.push_back(m);
            }
        }
        prev
    };
    let mut prev = search(false);
    if !prev.contains_key(&goal) {
        prev = search(true);
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
    use KeyCode::*;
    match name.to_ascii_uppercase().as_str() {
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
        "I" => KeyCode::KeyI,
        "F8" => KeyCode::F8,
        "SHIFT" => KeyCode::ShiftLeft,
        "CTRL" => KeyCode::ControlLeft,
        "NUMPAD8" => KeyCode::Numpad8,
        "NUMPAD5" => KeyCode::Numpad5,
        "ALT" => KeyCode::AltLeft,
        "F10" => F10,
        "F9" => F9,
        "F11" => F11,
        "F12" => F12,
        "0" => Digit0,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "E" => KeyE,
        "F" => KeyF,
        "G" => KeyG,
        "H" => KeyH,
        "J" => KeyJ,
        "K" => KeyK,
        "L" => KeyL,
        "M" => KeyM,
        "N" => KeyN,
        "O" => KeyO,
        "P" => KeyP,
        "R" => KeyR,
        "T" => KeyT,
        "U" => KeyU,
        "V" => KeyV,
        "Y" => KeyY,
        "ESCAPE" => Escape,
        "ENTER" => Enter,
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
    // `AOMAC_AUDIO_LOG=1`: the real audio engine runs in the harness and `audio` steps print its status (combat music, voices)
    let audio = std::env::var_os("AOMAC_AUDIO_LOG").and_then(|_| ao_audio::Audio::start(&dir).map_err(|e| eprintln!("audio disabled: {e:#}")).ok()).inspect(|a| {
        // muted to the speakers unless `AOMAC_AUDIO_UNMUTE` is set (voices / RMS are still logged)
        if std::env::var_os("AOMAC_AUDIO_UNMUTE").is_none() {
            a.set_output_gain(0.0);
        }
    });
    let mut p = Play::new(dir, None, Some("Ithaca".into()), audio).unwrap();
    p.servers = Some(ao_net::client::fetch_servers().map_err(|e| e.to_string()));
    let o = Offscreen::new(&p, (1280, 800)).unwrap();
    let shots = std::env::var_os("AOMAC_LIVE_SHOTS").map(std::path::PathBuf::from);
    if let Some(d) = &shots {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut l = Live { p, o, last: Instant::now(), shots, dt_cap: f32::INFINITY };
    l.tick();
    let w = l.p.login_w.unwrap();
    l.p.gui.set_text(w, "username", &user);
    l.p.gui.set_text(w, "password", &pass);
    l.p.handle(Event::Clicked { window: w, view: "login_btn".into(), item: None }, &mut l.o.host);
    l.until("character list", 60, |p| p.screen == Screen::CharSelect);
    let cw = l.p.char_w.unwrap();
    // `AOMAC_LIVE_CC=<name>`: New Character, then the four scenes with real clicks at window coordinates (the offline `create::shots` ones: Atrox,
    // Tall/Heavy/head arrow, Soldier, the name typed), a `cc-<scene>` shot each (`AOMAC_LIVE_SHOTS`), nothing is sent; the close button's exit
    // dialog is answered No and the session ends. The intro runs as in the game (`AOMAC_CC_SKIP_INTRO=1` skips it).
    if let Ok(name) = std::env::var("AOMAC_LIVE_CC") {
        l.p.handle(Event::Clicked { window: cw, view: "create_btn".into(), item: None }, &mut l.o.host);
        let click = |l: &mut Live, x: f32, y: f32| {
            l.p.input(ao_gui::InputEvent::MouseMove { x, y }, &mut l.o.host);
            l.wait(0.1);
            for ev in [ao_gui::InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left }, ao_gui::InputEvent::MouseUp { x, y, button: ao_gui::MouseButton::Left }] {
                l.p.input(ev, &mut l.o.host);
                l.tick();
            }
            l.wait(0.1);
        };
        for scene in 0..4 {
            let t = Instant::now();
            while !l.p.live_cc_active(scene) {
                assert!(t.elapsed() < Duration::from_secs(180), "creation scene {scene} never became active");
                l.tick();
            }
            l.wait(3.0); // the actors' background loads
            match scene {
                0 => click(&mut l, 1100.0, 380.0),
                1 => [(1147.0, 221.0), (1150.0, 461.0), (127.0, 135.0)].into_iter().for_each(|(x, y)| click(&mut l, x, y)),
                2 => click(&mut l, 1150.0, 280.0),
                _ => l.p.input(ao_gui::InputEvent::Text(name.clone()), &mut l.o.host),
            }
            l.wait(1.5);
            l.shot(&format!("cc-{scene}"));
            if scene < 3 {
                click(&mut l, 1170.0, 760.0); // Next
            }
        }
        click(&mut l, 1247.0, 38.0); // close: AskExitMessage
        l.shot("cc-exit-dialog");
        l.p.input(ao_gui::InputEvent::Key { key: ao_gui::Key::Escape, pressed: true, mods: Default::default() }, &mut l.o.host);
        l.wait(0.5);
        return;
    }
    // `AOMAC_LIVE_NEW=<name>:<CC breed 1..7>:<CC profession 1..14>`: New Character, the creation module sends its request (no scene clicks),
    // the login server's `CharacterCreated` + `ZoneHandoff` take the app into the world
    if let Ok(spec) = std::env::var("AOMAC_LIVE_NEW") {
        let mut it = spec.split(':');
        let (name, breed, prof) = (it.next().unwrap().to_string(), it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap());
        l.p.handle(Event::Clicked { window: cw, view: "create_btn".into(), item: None }, &mut l.o.host);
        let t = Instant::now();
        while !l.p.live_create(&name, breed, prof) {
            assert!(t.elapsed() < Duration::from_secs(120), "creation module did not start");
            l.tick();
        }
    } else {
        let i = l.p.chars.iter().position(|c| c.info.name == want).expect("character not on the account");
        l.p.select_row(i, &mut l.o.host);
        l.wait(2.0);
        l.shot("charselect"); // the detail panel of the picked character (breed / gender / profession / location)
        l.p.handle(Event::Clicked { window: cw, view: "login_btn".into(), item: None }, &mut l.o.host);
    }
    l.until("world", 120, |p| p.screen == Screen::InWorld && matches!(p.fade, Fade::Hold));
    eprintln!("in world: {} (player built: {}, focus {:?})", l.pos(), l.p.player.is_some(), l.p.gui.focused_view());
    for step in std::env::var("AOMAC_LIVE_STEPS").unwrap_or_else(|_| "wait=2,shot=enter".into()).split(',') {
        let (k, v) = step.split_once('=').unwrap_or((step, ""));
        match k {
            "wait" => l.wait(v.parse().unwrap()),
            "shot" => l.shot(v),
            "pos" => eprintln!("{}", l.pos()),
            "audio" => eprintln!("audio: {} music={:?}", l.p.audio.as_ref().map_or("off".into(), |a| a.status()), l.p.audio.as_ref().and_then(|a| a.now_playing())),
            // combat steps: `near` lists the dynels within 60 m, `tab` = TAB (next hostile target), `sel=<instance>` selects,
            // `fight` prints the combat layer's state (Q / X / B hold the keys: attack / sit / brawl)
            "near" => {
                let me = l.p.zone.own().unwrap().pos;
                let mut v: Vec<_> = l.p.zone.dynels.iter().map(|(i, d)| (((d.pos[0] - me[0]).powi(2) + (d.pos[2] - me[2]).powi(2)).sqrt(), *i, d.clone())).filter(|e| e.0 < v.parse().unwrap_or(60.0)).collect();
                v.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (dist, i, d) in v {
                    eprintln!("near {i} {:?} npc={} lvl={} hp={}/{} side={} dist={dist:.1} at {:.0},{:.1},{:.0}", d.name, d.npc, d.level, d.health, d.max_health, d.side, d.pos[0], d.pos[1], d.pos[2]);
                }
            }
            // `approach=<instance>`: servo the heading with A/D and walk with W until 2.5 m from the dynel (the conventions of the yaw
            // and of the turn keys are detected on the fly: the distance must shrink while walking, the error while turning)
            "approach" => {
                // `approach=x:z` walks to a point instead
                let fixed = v.split_once(':').map(|(x, z)| (x.parse::<f32>().unwrap(), z.parse::<f32>().unwrap()));
                let id: i32 = if v == "target" { l.p.zone.target.unwrap() } else { v.parse().unwrap_or(0) };
                let goal = |l: &Live| fixed.unwrap_or_else(|| (l.p.zone.dynels[&id].pos[0], l.p.zone.dynels[&id].pos[2]));
                let dist = |l: &Live| {
                    let (a, b) = (l.p.zone.own().unwrap().pos, goal(l));
                    ((b.0 - a[0]).powi(2) + (b.1 - a[2]).powi(2)).sqrt()
                };
                let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI;
                let (mut sign, mut turn_left_is_pos) = (1.0f32, true);
                let (t0, mut held): (Instant, Option<&str>) = (Instant::now(), None);
                let (mut last_chk, mut last_err, mut last_dist) = (Instant::now(), 0.0f32, dist(&l));
                while t0.elapsed().as_secs() < 60 && dist(&l) > 2.5 {
                    l.tick();
                    let (me, to) = (l.p.zone.own().unwrap().clone(), goal(&l));
                    let want = (sign * (to.0 - me.pos[0])).atan2(to.1 - me.pos[2]);
                    let err = wrap(want - me.yaw.unwrap_or(0.0));
                    let key = if err.abs() > 0.12 { Some(if (err > 0.0) == turn_left_is_pos { "A" } else { "D" }) } else { Some("W") };
                    if key != held {
                        if let Some(k) = held {
                            l.key(code(k), false);
                        }
                        if let Some(k) = key {
                            l.key(code(k), true);
                        }
                        held = key;
                    }
                    if last_chk.elapsed().as_secs_f32() > 0.7 {
                        if held == Some("W") {
                            if dist(&l) > last_dist + 0.3 {
                                sign = -sign; // walking away: the heading convention is mirrored
                            }
                        } else if err.abs() > last_err.abs() + 0.05 {
                            turn_left_is_pos = !turn_left_is_pos;
                        }
                        (last_chk, last_err, last_dist) = (Instant::now(), err, dist(&l));
                    }
                }
                if let Some(k) = held {
                    l.key(code(k), false);
                }
                eprintln!("approached {id} to {:.1} m: {}", dist(&l), l.pos());
            }
            // chat: `say=<line>` runs the line as if typed in the chat bar (`/say hi`, `/g Global hi`, `/tell X hi`; a line without a leading `/` is dropped by `run_line`)
            "chatdrop" => l.p.chat.as_ref().expect("chat hub").drop_connection(),
            "say" => {
                let p = &mut l.p;
                p.chat.as_mut().expect("chat hub").run_line(&mut p.gui, v, &p.zone, &p.text);
            }
            // `buddyadd=<name>` / `buddyrm=<name>` / `lftsearch=<side>:<profession>:<location>` (dropdown ids, 7 / 16 = any): the Friends / Team Search
            // window requests; `friendswin` / `lftwin` toggle those windows through the HUD dvalues (as Ctrl+R / the right menu do)
            "buddyadd" | "buddyrm" | "lftsearch" => {
                for _ in 0..2 {
                    let p = &mut l.p;
                    if p.chat.as_mut().expect("chat hub").live_social(&mut p.gui, k, v, &p.zone, &p.text) {
                        break;
                    }
                    l.wait(3.0);
                }
            }
            "friendswin" | "lftwin" => {
                let d = if k == "friendswin" { "friends_window" } else { "lft_window" };
                let h = l.p.hud.as_mut().expect("hud");
                h.set_dvalue(&mut l.p.gui, d, v != "off");
                l.tick();
            }
            "tab" => {
                l.p.input(ao_gui::InputEvent::Key { key: ao_gui::Key::Tab, pressed: true, mods: ao_gui::Modifiers::default() }, &mut l.o.host);
                eprintln!("target {:?}", l.p.zone.target);
            }
            "stats" => {
                let mut v: Vec<_> = l.p.zone.stats.iter().collect();
                v.sort();
                eprintln!("stats {}", v.iter().map(|(k, x)| format!("{k}={x}")).collect::<Vec<_>>().join(" "));
            }
            // the spells running on the own character (docs/gui.md §11.14) and the maps they fill
            "spells" => {
                eprintln!("spells {}", l.p.zone.active_spells.iter().map(|s| format!("{:#x}(stat {} amount {} target {})", s.function, s.stat(0), s.stat(0x27), s.stat(0x20))).collect::<Vec<_>>().join(" "));
            }
            // `selname=<name>`: select the nearest dynel with that name
            "selname" => {
                let me = l.p.zone.own().unwrap().pos;
                let best = l.p.zone.dynels.iter().filter(|(_, d)| d.name == v).min_by(|a, b| {
                    let dd = |d: &crate::play::zone::DynelState| (d.pos[0] - me[0]).powi(2) + (d.pos[2] - me[2]).powi(2);
                    dd(a.1).total_cmp(&dd(b.1))
                });
                l.p.zone.target = best.map(|b| *b.0);
                eprintln!("target {:?}", l.p.zone.target);
            }
            "sel" => l.p.zone.target = Some(v.parse().unwrap()),
            "fight" => {
                let m = l.p.fight.as_ref().unwrap();
                eprintln!("fight: attacking={} numbers={:?}", m.attacking(), m.numbers().iter().map(|n| n.text.as_str()).collect::<Vec<_>>());
            }
            // `water`: the nearest deep-water ground points (liquid surface > 0.5 m above the ground) within 300 m, for `goto=x:z` (swimming runs)
            "water" => {
                let pos = l.p.zone.own().unwrap().pos;
                let col = ao_formats::playfield::collision::Collision::load(&ao_rdb::RecordStore::open(&ao_gui::client_dir()).unwrap(), l.p.zone.playfield.unwrap()).unwrap();
                let mut found = vec![];
                for ix in -75..=75 {
                    for iz in -75..=75 {
                        let (x, z) = (pos[0] + ix as f32 * 4.0, pos[2] + iz as f32 * 4.0);
                        let Some(g) = col.ground([x, pos[1] + 50.0, z]) else { continue };
                        if col.liquid_at([x, g, z]).is_some_and(|w| w.level - g > 0.5) {
                            found.push((((x - pos[0]).powi(2) + (z - pos[2]).powi(2)).sqrt(), x, g, z));
                        }
                    }
                }
                found.sort_by(|a, b| a.0.total_cmp(&b.0));
                eprintln!("water: {} cells; nearest {:?}", found.len(), &found[..found.len().min(5)]);
            }
            "goto" => {
                // autopilot along a collision route: W/S/C/Z by the offset to the next waypoint (the heading stays put)
                // `goto=<instance>` walks to the dynel (2 m short of it is close enough: the route ends on its cell)
                let resolved;
                // `goto=hunt`: a Beach Leet first, else the weakest (lowest max health, then nearest) living hostile (side 3) NPC of level <= 2 within 60 m, selected first
                let hunted = (v == "hunt").then(|| {
                    let me = l.p.zone.own().unwrap().pos;
                    let d = |d: &crate::play::zone::DynelState| (d.pos[0] - me[0]).powi(2) + (d.pos[2] - me[2]).powi(2);
                    let (id, t) = l.p.zone.dynels.iter().filter(|(_, x)| x.npc && x.side == 3 && x.level <= 2 && x.health > 0 && d(x) < 3600.0).min_by(|a, b| (!a.1.name.contains("Leet"), a.1.max_health, d(a.1)).partial_cmp(&(!b.1.name.contains("Leet"), b.1.max_health, d(b.1))).unwrap()).expect("no hostile low-level NPC near");
                    eprintln!("hunt: {id} {:?} lvl {} hp {}/{}", t.name, t.level, t.health, t.max_health);
                    *id
                });
                if let Some(id) = hunted {
                    l.p.zone.target = Some(id);
                }
                let hunted_s = hunted.map(|i| i.to_string());
                let v = hunted_s.as_deref().unwrap_or(v);
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
                let (route_len, pf0) = (path.len(), l.p.zone.playfield);
                let mut pilot = Pilot::new(path);
                let t = Instant::now();
                l.dt_cap = 0.1;
                let (mut last_log, mut logs) = (Instant::now(), 0);
                loop {
                    l.tick();
                    if last_log.elapsed().as_secs() >= 4 {
                        last_log = Instant::now();
                        eprintln!("goto progress: cell {}/{} {} held {:?}", pilot.idx(), route_len, l.pos(), pilot.held());
                        logs += 1;
                        if logs % 4 == 1 {
                            l.shot(&format!("goto{logs}"));
                        }
                    }
                    if pilot.step(&mut l.p, &mut l.o.host) || t.elapsed().as_secs() > 240 || l.p.zone.playfield != pf0 {
                        break;
                    }
                }
                pilot.release(&mut l.p, &mut l.o.host);
                l.dt_cap = f32::INFINITY;
                eprintln!("after goto {v}: {}", l.pos());
            }
            // `press=F8` / `press=CTRL+F8` / `press=SHIFT+F8`: modifiers down, tap the last key, release (F8 = first/third person,
            // Ctrl+F8 cycles the camera vehicle, Shift+F8 steps the scripted views)
            "press" | "ui" => {
                let keys: Vec<KeyCode> = v.split('+').map(code).collect();
                for &c in &keys {
                    l.key(c, true);
                    l.tick();
                }
                for &c in keys.iter().rev() {
                    l.key(c, false);
                    l.tick();
                }
            }
            // `drag=right:dx:dy` / `drag=left:dx:dy`: button down at the screen centre, `dx`/`dy` raw mouse counts spread over 8
            // frames (the cursor-captured `GameInput::MouseMotion` stream), button up (right: turn the character and pitch, left: orbit the camera)
            "drag" => {
                let mut it = v.split(':');
                let button = if it.next() == Some("left") { ao_gui::MouseButton::Left } else { ao_gui::MouseButton::Right };
                let (dx, dy): (f32, f32) = (it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap());
                let (x, y) = (640.0, 400.0);
                l.p.input(ao_gui::InputEvent::MouseDown { x, y, button }, &mut l.o.host);
                for _ in 0..8 {
                    l.tick();
                    l.p.game_input(GameInput::MouseMotion { dx: dx / 8.0, dy: dy / 8.0 }, &mut l.o.host);
                }
                l.tick();
                l.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut l.o.host);
                l.tick();
                eprintln!("after {step}: {}", l.pos());
            }
            // UI steps: `ui=ctrl+6` / `ui=u` are `press` (a key stroke through `game_input`, where the window hot keys live, with `host.mods`
            // following the modifier keys), `move=x:y` hover, `click=x:y`, `clickdyn=<instance>` (left-clicks the first screen point whose pick ray
            // hits the dynel)
            "move" | "click" => {
                let (x, y) = v.split_once(':').map(|(x, y)| (x.parse().unwrap(), y.parse().unwrap())).unwrap();
                l.p.input(ao_gui::InputEvent::MouseMove { x, y }, &mut l.o.host);
                if k == "click" {
                    for ev in [ao_gui::InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left }, ao_gui::InputEvent::MouseUp { x, y, button: ao_gui::MouseButton::Left }] {
                        l.tick();
                        l.p.input(ev, &mut l.o.host);
                    }
                }
                l.tick();
            }
            // `clickdyn=[shift+|ctrl+]<instance>` / `hoverdyn=…`: left-click / hover the first screen point whose pick ray hits the dynel's box
            // with those modifiers held; `hoverdyn` prints the pointer sprites (GFX_GUI_POINTER* 0x135..0x14e) of the frame
            "clickdyn" | "hoverdyn" => {
                let id: i32 = v.rsplit('+').next().unwrap().parse().unwrap();
                let (cam, lens) = (l.o.host.camera, l.o.host.lens.unwrap_or_default());
                let mut hit = None;
                'g: for y in (0..800).step_by(4) {
                    for x in (0..1280).step_by(4) {
                        let ray = crate::play::hud_target::pick_ray(&cam, &lens, (1280.0, 800.0), (x as f32, y as f32));
                        if crate::play::hud_target::pick_all(&ray, &l.p.zone).contains(&id) {
                            hit = Some((x as f32, y as f32));
                            break 'g;
                        }
                    }
                }
                let (x, y) = hit.expect("dynel not on screen");
                eprintln!("{k} {id} at {x},{y}");
                l.o.host.mods = ao_gui::Modifiers { shift: v.contains("shift+"), ctrl: v.contains("ctrl+"), ..Default::default() };
                if k == "hoverdyn" {
                    l.p.input(ao_gui::InputEvent::MouseMove { x, y }, &mut l.o.host);
                    l.tick();
                    let list = l.tick();
                    let sprites: Vec<String> = list.cmds.iter().filter_map(|c| if let ao_gui::DrawCmd::Gfx { id, dst, .. } = c { (0x135..=0x14e).contains(&id.0).then(|| format!("{:#x}@{},{}", id.0, dst[0], dst[1])) } else { None }).collect();
                    eprintln!("pointer sprites {sprites:?} hide_os_cursor {}", l.o.host.hide_cursor);
                } else {
                    for ev in [ao_gui::InputEvent::MouseMove { x, y }, ao_gui::InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left }, ao_gui::InputEvent::MouseUp { x, y, button: ao_gui::MouseButton::Left }] {
                        l.tick();
                        l.p.input(ev, &mut l.o.host);
                    }
                    l.tick();
                    eprintln!("target {:?}", l.p.zone.target);
                }
                l.o.host.mods = Default::default();
            }
            // `mdrag=x1:y1:x2:y2` GUI left drag (slider knob, windows) in 10 steps; `watch=secs` prints every own stat that changed meanwhile
            "mdrag" => {
                let c: Vec<f32> = v.split(':').map(|n| n.parse().unwrap()).collect();
                l.p.input(ao_gui::InputEvent::MouseMove { x: c[0], y: c[1] }, &mut l.o.host);
                l.p.input(ao_gui::InputEvent::MouseDown { x: c[0], y: c[1], button: ao_gui::MouseButton::Left }, &mut l.o.host);
                for i in 1..=10 {
                    l.tick();
                    let t = i as f32 / 10.0;
                    l.p.input(ao_gui::InputEvent::MouseMove { x: c[0] + (c[2] - c[0]) * t, y: c[1] + (c[3] - c[1]) * t }, &mut l.o.host);
                }
                l.p.input(ao_gui::InputEvent::MouseUp { x: c[2], y: c[3], button: ao_gui::MouseButton::Left }, &mut l.o.host);
                l.tick();
            }
            // `agg=<v>`: drags the AGG/DEF knob (-100..=100) with the mouse, then deletes the locally applied stat 0x33 and waits 4 s for the server's
            // own `StatIIR` echo (docs/gui.md 10.6); prints the stat before / after
            "agg" => {
                let want: f32 = v.parse().unwrap();
                let r = l.p.hud.as_ref().unwrap().aggdef_rect(&l.p.gui).expect("slider");
                let cur = l.p.zone.stat(0x33).unwrap_or(0) as f32;
                // knob left edge = floor((v + 100) * 117 / 200); the grab point is 5 px inside the knob, the release lands on the exact value
                let (from, to) = (r.l + ((cur + 100.0) * 117.0 / 200.0).floor() + 5.0, r.l + (want + 100.0) * 117.0 / 200.0 + 5.01);
                let y = r.t + 9.0;
                l.p.input(ao_gui::InputEvent::MouseMove { x: from, y }, &mut l.o.host);
                l.p.input(ao_gui::InputEvent::MouseDown { x: from, y, button: ao_gui::MouseButton::Left }, &mut l.o.host);
                for i in 1..=10 {
                    l.tick();
                    l.p.input(ao_gui::InputEvent::MouseMove { x: from + (to - from) * i as f32 / 10.0, y }, &mut l.o.host);
                }
                l.p.input(ao_gui::InputEvent::MouseUp { x: to, y, button: ao_gui::MouseButton::Left }, &mut l.o.host);
                l.tick();
                eprintln!("agg: after drag local stat 0x33 = {:?}", l.p.zone.stat(0x33));
                l.p.zone.stats.remove(&0x33);
                l.wait(4.0);
                eprintln!("agg: 4 s after the local value was removed, stat 0x33 = {:?}", l.p.zone.stat(0x33));
            }
            "watch" => {
                let before = l.p.zone.stats.clone();
                l.wait(v.parse().unwrap());
                let mut d: Vec<_> = l.p.zone.stats.iter().filter(|(k, x)| before.get(*k) != Some(*x)).map(|(k, x)| format!("{k}: {:?}->{x}", before.get(k))).collect();
                d.sort();
                eprintln!("watch: {}", d.join(" "));
            }
            // interaction steps (docs/zone/interact.md): `npcs` lists the dynels flagged talkable, `talk=<instance>` / `talkname=<name>` is the right click
            // (`N3Msg_DefaultActionOnDynel`), `dlg` prints the dialogue window, `answer=<n>` clicks an answer link, `useobj=<kind>:<instance>` `N3Msg_UseItem`
            "npcs" => {
                let i = l.p.interact.as_ref().unwrap();
                for (id, v) in i.flagged() {
                    let d = l.p.zone.dynels.get(&id);
                    eprintln!("npc {id} stat0x300={v} {:?} at {:?}", d.map(|d| d.name.as_str()), d.map(|d| d.pos));
                }
            }
            "talk" | "talkname" => {
                let id = if k == "talk" {
                    v.parse().unwrap()
                } else {
                    let me = l.p.zone.own().unwrap().pos;
                    let d = |d: &crate::play::zone::DynelState| (d.pos[0] - me[0]).powi(2) + (d.pos[2] - me[2]).powi(2);
                    *l.p.zone.dynels.iter().filter(|(_, x)| x.name == v).min_by(|a, b| d(a.1).total_cmp(&d(b.1))).expect("no such dynel").0
                };
                eprintln!("talk {id}: {:?}", l.p.interact.as_mut().unwrap().default_action(id));
                l.wait(3.0);
            }
            "dlg" => eprintln!("{}", l.p.interact.as_ref().unwrap().dump(&l.p.gui)),
            "answer" => {
                let p = &mut l.p;
                eprintln!("answer {v}: {}", p.interact.as_mut().unwrap().answer(&mut p.gui, &p.zone, v.parse().unwrap()));
                l.wait(3.0);
            }
            "useobj" => {
                let (kind, inst) = v.split_once(':').unwrap();
                l.p.interact.as_mut().unwrap().use_object(ao_net::msg::Identity { kind: kind.parse().unwrap(), instance: inst.parse().unwrap() });
                l.wait(3.0);
            }
            // `inv`: the own inventory (slot, item ids, count) as the zone state holds it
            "inv" => {
                let mut v: Vec<_> = l.p.zone.inventory.iter().collect();
                v.sort_by_key(|e| *e.0);
                let items: Vec<(u32, i32)> = v.iter().map(|(s, e)| (**s, e.item.low_id)).collect();
                for (slot, low) in items {
                    let line = l.p.hud.as_mut().map(|h| h.live_item_line(&mut l.p.gui, low)).unwrap_or_default();
                    eprintln!("inv item {slot:#x} {line}");
                }
                for (slot, e) in v {
                    eprintln!("inv slot {slot:#x}: {e:?}");
                }
            }
            // `zc=secs`: runs frames for that long and prints the zone change state whenever it changes and every 0.5 s (playfield, teleporting, awaiting
            // the CharInPlay echo, world ready, own position, dynel count) and writes a frame at every change (`zc<n>`, at most 14)
            "zc" => {
                let (t, mut n, mut shots, mut prev) = (Instant::now(), 0, 0, String::new());
                while t.elapsed().as_secs_f32() < v.parse().unwrap() {
                    let list = l.tick();
                    let state = format!("pf {:?} teleporting {} awaiting_alive {} world_ready {} in_play_sent {} player {} input_open {} dynels {}", l.p.zone.playfield, l.p.teleporting, l.p.awaiting_alive, l.p.world_ready, l.p.zone.in_play_sent, l.p.player.is_some(), l.p.game_input_open(), l.p.zone.dynels.len());
                    let changed = state != prev;
                    if changed || t.elapsed().as_secs_f32() >= n as f32 * 0.5 {
                        n += 1;
                        eprintln!("zc {:.2}s{}: {state} {}", t.elapsed().as_secs_f32(), if changed { " CHANGE" } else { "" }, l.pos());
                        if changed && shots < 14 {
                            shots += 1;
                            if let Some(dir) = &l.shots {
                                l.o.png(&l.p, &list, &dir.join(format!("zc{shots}.png"))).unwrap();
                            }
                        }
                        prev = state;
                    }
                }
            }
            // `props=<radius>`: the world objects (doors, terminals, vending machines, corpses, items) near the character, with their `Can` stat
            "props" => {
                let me = l.p.zone.own().unwrap().pos;
                let mut v: Vec<_> = l.p.zone.world.prop_list().into_iter().map(|(k, i, p, c)| (((p[0] - me[0]).powi(2) + (p[2] - me[2]).powi(2)).sqrt(), k, i, p, c)).filter(|e| e.0 < v.parse().unwrap_or(30.0)).collect();
                v.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (d, k, i, p, c) in v {
                    eprintln!("prop {k}:{i} kind={k:#x} can={c:?} dist={d:.1} at {:.1},{:.1},{:.1}", p[0], p[1], p[2]);
                }
            }
            // `use=<kind>:<instance>`: `N3Msg_DefaultActionOnDynel` on a world object (`Can` bit 0 get, bit 3 use), then what the UI shows
            "use" => {
                let (kind, inst) = v.split_once(':').unwrap();
                let id = ao_net::msg::Identity { kind: kind.parse().unwrap(), instance: inst.parse().unwrap() };
                let p = &mut l.p;
                eprintln!("use {v}: can={:?} -> {:?}", crate::play::interact::Interact::can_of(&p.zone, id), p.interact.as_mut().unwrap().default_action_on(&p.zone, id));
                l.wait(4.0);
                let p = &mut l.p;
                let i = p.interact.as_mut().unwrap();
                eprintln!("feedback {:?}\ngrid: {}\nloot: {:?}\n{}", i.take_feedback(), i.grid_dump(&p.gui), i.loot_dump(&mut p.gui), i.dump(&p.gui));
            }
            // NPC window button bar / NPC trade / player trade (docs/zone/interact.md §2, §10, §11)
            "btn" => {
                let p = &mut l.p;
                eprintln!("btn {v}: {}", p.interact.as_mut().unwrap().press_button(&mut p.gui, v.parse().unwrap()));
                l.wait(4.0);
                let p = &l.p;
                let i = p.interact.as_ref().unwrap();
                eprintln!("{}\n{}", i.dump(&p.gui), i.trade_dump(&p.gui));
            }
            "tdump" => {
                let p = &l.p;
                let i = p.interact.as_ref().unwrap();
                eprintln!("{}\n{}\n{}", i.dump(&p.gui), i.trade_dump(&p.gui), i.ptrade_dump(&p.gui));
            }
            "tadd" => {
                let (kind, inst) = v.split_once(':').unwrap();
                let item = if kind == "slot" {
                    ao_net::n3::inventory::item_identity(u32::from_str_radix(inst.trim_start_matches("0x"), 16).unwrap())
                } else {
                    ao_net::msg::Identity { kind: kind.parse().unwrap(), instance: inst.parse().unwrap() }
                };
                let p = &mut l.p;
                eprintln!("tadd {v}: {}", p.interact.as_mut().unwrap().trade_add(&mut p.gui, item));
                l.wait(3.0);
            }
            "taccept" | "tdecline" => {
                let p = &mut l.p;
                let i = p.interact.as_mut().unwrap();
                let ok = if k == "taccept" { i.trade_accept(&mut p.gui, &p.zone, v.parse().unwrap_or(0)) } else { i.trade_decline(&mut p.gui, &p.zone) };
                eprintln!("{k} {v}: {ok}");
                l.wait(4.0);
                let p = &l.p;
                eprintln!("{}", p.interact.as_ref().unwrap().trade_dump(&p.gui));
            }
            // `ruse=<kind>:<instance>`: the right click on a world object (`FUN_1002c469`: `N3Msg_UseItem(id, false)` whatever its `Can`)
            // `ruse=<kind>:<instance>` (right click use) or `ruse=corpse` (the nearest corpse prop, kind 0xC76A)
            "ruse" => {
                let id = if v == "corpse" {
                    let me = l.p.zone.own().unwrap().pos;
                    let d = |p: [f32; 3]| (p[0] - me[0]).powi(2) + (p[2] - me[2]).powi(2);
                    let c = l.p.zone.world.prop_list().into_iter().filter(|e| e.0 == 0xC76A).min_by(|a, b| d(a.2).total_cmp(&d(b.2))).expect("no corpse prop");
                    ao_net::msg::Identity { kind: c.0, instance: c.1 }
                } else {
                    let (kind, inst) = v.split_once(':').unwrap();
                    ao_net::msg::Identity { kind: kind.parse().unwrap(), instance: inst.parse().unwrap() }
                };
                let p = &mut l.p;
                eprintln!("ruse {v}: {:?}", p.interact.as_mut().unwrap().use_item(&p.zone, id, false));
                l.wait(5.0);
                let p = &mut l.p;
                let i = p.interact.as_mut().unwrap();
                eprintln!("feedback {:?}\ngrid: {}\nloot: {:?}\n{}", i.take_feedback(), i.grid_dump(&p.gui), i.loot_dump(&mut p.gui), i.dump(&p.gui));
            }
            // `invuse=<slot>`: `N3Msg_UseItem` on the bag item in `slot` (0x40 ..): the double click of the inventory window
            "invuse" => {
                let slot = u32::from_str_radix(v.trim_start_matches("0x"), if v.starts_with("0x") { 16 } else { 10 }).unwrap();
                l.p.interact.as_mut().unwrap().use_object(ao_net::n3::inventory::item_identity(slot));
                l.wait(5.0);
            }
            // `useon=<slot>:<kind>:<instance>`: the bag item in `slot` released over the world object (`N3Msg_UseItemOnItem` / `UseItemOnCharacter`)
            // `dclick=<slot hex>`: the double click on the item of an inventory slot (bag item: wear / use, worn item: back to the bag)
            "dclick" => {
                let slot = u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap();
                let p = &mut l.p;
                p.hud.as_mut().unwrap().live_double_click(&p.zone, slot);
                l.wait(3.0);
            }
            "useon" => {
                let mut it = v.split(':');
                let slot = u32::from_str_radix(it.next().unwrap().trim_start_matches("0x"), 16).unwrap();
                let (kind, inst) = (it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap());
                let p = &mut l.p;
                p.hud.as_mut().unwrap().use_item_on(&p.zone, slot, ao_net::msg::Identity { kind, instance: inst });
                l.wait(5.0);
                let p = &mut l.p;
                let i = p.interact.as_mut().unwrap();
                eprintln!("feedback {:?}", i.take_feedback());
            }
            // `loottake=<cell>`: the double click on a cell of the open loot window (`FUN_100ca1e7` -> `MoveItemToInventory`)
            "loottake" => {
                let p = &mut l.p;
                eprintln!("loottake {v}: {}", p.interact.as_mut().unwrap().loot_take(&mut p.gui, &p.zone, v.parse().unwrap()));
                l.wait(4.0);
                let p = &mut l.p;
                let i = p.interact.as_mut().unwrap();
                eprintln!("feedback {:?}\nloot: {:?}", i.take_feedback(), i.loot_dump(&mut p.gui));
            }
            // `lootid=<cell>[:<kind>:<instance>]`: `MoveItemToInventory` of a loot cell with its inventory entry id (or an explicit identity)
            "lootid" => {
                let mut it = v.split(':');
                let n: usize = it.next().unwrap().parse().unwrap();
                let p = &mut l.p;
                let i = p.interact.as_mut().unwrap();
                let item = match (it.next(), it.next()) {
                    (Some(k), Some(x)) => ao_net::msg::Identity { kind: k.parse().unwrap(), instance: x.parse().unwrap() },
                    _ => i.loot_entry_id(n).expect("no such cell"),
                };
                eprintln!("lootid {item:?}");
                i.move_to_bag(item);
                l.wait(4.0);
                let p = &mut l.p;
                let i = p.interact.as_mut().unwrap();
                eprintln!("feedback {:?}\nloot: {:?}", i.take_feedback(), i.loot_dump(&mut p.gui));
            }
            "grid" => {
                let p = &l.p;
                eprintln!("grid: {}", p.interact.as_ref().unwrap().grid_dump(&p.gui));
            }
            "gridsel" => {
                let p = &mut l.p;
                eprintln!("gridsel {v}: {}", p.interact.as_mut().unwrap().grid_select(&mut p.gui, v.parse().unwrap()));
                l.wait(6.0);
            }
            // vending machine window (docs/zone/interact.md "Vending machines / shops"): `shop` prints it, `shopbuy=<i>` is the plain double click on stock item i
            // (`MoveItemToInventory({0x6f, i})`), `shopadd=<i>` the Shift double click (`TradeAddItem`), `shoprm=<i>` removes bought item i, `shopaccept` / `shopdecline`
            "shop" => {
                let p = &l.p;
                eprintln!("{}", p.interact.as_ref().unwrap().shop_dump(&p.gui));
            }
            "shopbuy" | "shopadd" | "shoprm" | "shopaccept" | "shopdecline" => {
                let p = &mut l.p;
                let (i, gui) = (p.interact.as_mut().unwrap(), &mut p.gui);
                let n = v.parse().unwrap_or(0);
                let ok = match k {
                    "shopbuy" => i.shop_buy(gui, n),
                    "shopadd" => i.shop_add(gui, n),
                    "shoprm" => i.shop_remove(gui, n),
                    other => i.shop_press(gui, other == "shopaccept"),
                };
                eprintln!("{step} {v}: {ok}");
                l.wait(4.0);
                let p = &l.p;
                eprintln!("{}", p.interact.as_ref().unwrap().shop_dump(&p.gui));
            }
            // `useq=<kind>:<instance>`: `N3Msg_UseItem` (`GenericCmd` 3) without any wait (follow it with `zc=secs` to watch what the server does)
            "useq" => {
                let (kind, inst) = v.split_once(':').unwrap();
                l.p.interact.as_mut().unwrap().use_object(ao_net::msg::Identity { kind: kind.parse().unwrap(), instance: inst.parse().unwrap() });
            }
            "cam" => {
                let c = l.o.host.camera;
                eprintln!("camera pos {:.2} {:.2} {:.2} yaw {:.2} pitch {:.2} lens {:?}", c.pos.x, c.pos.y, c.pos.z, c.yaw, c.pitch, l.o.host.lens);
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

#[cfg(test)]
mod pilot_tests {
    use super::Pilot;

    /// A one-cell route (target in the own cell) starts on its last cell, so `step` can report arrival.
    #[test]
    fn one_cell_route_starts_at_the_last_cell() {
        assert_eq!(Pilot::new(vec![(1.0, 2.0)]).idx(), 0);
        assert_eq!(Pilot::new(vec![(1.0, 2.0), (2.0, 2.0), (3.0, 2.0)]).idx(), 1);
    }
}
