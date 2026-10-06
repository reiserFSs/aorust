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

/// Axis-aligned route over the 0.5 m cells the collision lets the character (`Collision::walk`) cross (server x, z): `(x, z)` waypoints.
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
            if prev.contains_key(&m) || c.ground(b).is_none() {
                continue;
            }
            let mut p = a;
            let ok = (1..=5).all(|i| {
                let t = i as f32 / 5.0;
                let want = [a[0] + (b[0] - a[0]) * t, p[1], a[2] + (b[2] - a[2]) * t];
                let w = c.walk(p, want);
                p = w.pos;
                !w.airborne && (p[0] - want[0]).abs() + (p[2] - want[2]).abs() < 0.03
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
        "F8" => KeyCode::F8,
        "SHIFT" => KeyCode::ShiftLeft,
        "CTRL" => KeyCode::ControlLeft,
        "NUMPAD8" => KeyCode::Numpad8,
        "NUMPAD5" => KeyCode::Numpad5,
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
    let audio = std::env::var_os("AOMAC_AUDIO_LOG").and_then(|_| ao_audio::Audio::start(&dir).map_err(|e| eprintln!("audio disabled: {e:#}")).ok());
    let mut p = Play::new(dir, None, Some("Ithaca".into()), audio).unwrap();
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
            // chat: `say=<line>` runs the line as if typed in the chat bar (`/g Global hi`, `/tell X hi`, plain = vicinity)
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
            // `press=F8` / `press=CTRL+F8` / `press=SHIFT+F8`: modifiers down, tap the last key, release (F8 = first/third person,
            // Ctrl+F8 cycles the camera vehicle, Shift+F8 steps the scripted views)
            "press" => {
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
            // UI steps: `ui=ctrl+6` / `ui=u` (window hotkey strokes), `move=x:y` hover, `click=x:y`, `clickdyn=<instance>` (left-clicks
            // the first screen point whose pick ray hits the dynel)
            "ui" => {
                let mods = ao_gui::Modifiers { ctrl: v.contains("ctrl+"), ..Default::default() };
                let c = v.rsplit('+').next().unwrap().chars().next().unwrap();
                l.p.input(ao_gui::InputEvent::Key { key: ao_gui::Key::Letter(c), pressed: true, mods }, &mut l.o.host);
                l.tick();
            }
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
