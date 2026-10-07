//! `#[ignore]`d live walk-through against a real zone server, rendered offscreen (no window, no desktop capture):
//! `printf 'user\npass\n' | AOMAC_LIVE_CHAR=Aomacvolk AOMAC_LIVE_SHOTS=/tmp/x AOMAC_LIVE_STEPS='wait=2,shot=a,W=3,shot=b' \
//!  cargo test --release -p aomac live_walk -- --ignored --nocapture`.
//! Credentials come from stdin only. Steps (comma separated): `KEY=secs` holds a letter/arrow/space key (`W A S D Z C SPACE UP LEFT`),
//! `wait=secs`, `shot=name` (PNG into AOMAC_LIVE_SHOTS), `pos` prints the own position, `press=F8|CTRL+F8|SHIFT+F8` taps keys (camera),
//! `drag=right:dx:dy` / `drag=left:dx:dy` mouse-look with raw counts, `cam` prints the camera and lens. HUD steps: `ui=u|ctrl+1` window hotkey,
//! `heading=server_yaw` rotates in place through right mouse input to the requested heading (radians), before idle/capture steps.
//! `move=x:y` hover, `click=x:y`, `clickdyn=<instance>` world click on a dynel, `mdrag=x1:y1:x2:y2` GUI drag, `watch=secs` own-stat changes.
//! `down=KEY` / `up=KEY` hold across steps; `clickcorpse` / `rightcorpse` click a visible, GUI-unobscured corpse pick point.
//! Frame captures require `AOMAC_LIVE_SHOTS`: `arm=attack:1:note,down=Q,capturewait=30,up=Q` records the frame
//! processing the next own attack note. `arm=burst:1:special,M=0.1,capturewait=30` selects SpecialAttack instead.
//! (60 PNGs for one second, `attack-0000.png` onward). No event queue polling; the processing hook counts events.
//! `npcprobe=target` (or an instance id) logs the NPC's sampled animation clock on each captured frame.
//! Select/frame a naturally walking NPC first with `selname`, camera and movement steps, then use
//! `npcprobe=target,npcwait=walk:30,frames=npc-walk:2` (120 fixed-60Hz frames); repeat with idle.
//! `npcwait=walk|idle[:timeout]` polls the selected NPC at fixed 60Hz before capture. W drives only the own avatar.
//! PNG readback may run slower than real time; simulation dt stays 1/60 s. Use unique prefixes.
//! `arm=prefix:seconds[:note|special|either|equipment|use|level]` defaults to either (note/special).
//! Special stat 148 identifies Burst, 150 Fling Shot. Arm before input, including internally waiting `dclick`/`invuse`.
use super::*;
use ao_render::{GameInput, KeyCode, Offscreen};
use std::io::BufRead;
use std::time::{Duration, Instant};

fn approach_state(zone: &super::super::zone::Zone, fixed: Option<(f32, f32)>, id: i32) -> Option<(&super::super::zone::DynelState, (f32, f32))> {
    let own = zone.own()?;
    let goal = match fixed {
        Some(point) => point,
        None => { let target = zone.dynels.get(&id)?; (target.pos[0], target.pos[2]) }
    };
    Some((own, goal))
}

const CAPTURE_DT: f32 = 1.0 / 60.0;

fn heading_error(target: f32, actual: f32) -> f32 {
    (target - actual + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

#[test]
fn live_heading_shortest_error() {
    assert!((heading_error(0.01, std::f32::consts::TAU - 0.01) - 0.02).abs() < 1e-6);
    assert!((heading_error(std::f32::consts::TAU - 0.01, 0.01) + 0.02).abs() < 1e-6);
    assert_eq!(heading_error(0.98020107, 0.98020107), 0.0);
}

struct FrameCapture {
    name: String,
    frames: usize,
    next: usize,
    event: Option<[u64; 5]>,
    kind: &'static str,
}

impl FrameCapture {
    fn new(spec: &str, event: Option<[u64; 5]>) -> Self {
        let mut parts = spec.split(':');
        let name = parts.next().unwrap();
        let secs: f64 = parts.next().expect("capture=prefix:seconds[:note|special|either|equipment|use|level]").parse().expect("capture seconds");
        let kind = match parts.next().unwrap_or("either") {
            "note" => "note",
            "special" => "special",
            "either" => "either",
            "equipment" => "equipment",
            "use" => "use",
            "level" => "level",
            _ => panic!("capture trigger must be note, special, either, equipment, use or level"),
        };
        assert!(parts.next().is_none(), "extra capture arguments");
        assert!(!name.is_empty() && !name.contains(['/', '\\']), "capture prefix must be a filename");
        assert!(secs.is_finite() && secs > 0.0 && secs <= 60.0, "capture duration must be in (0, 60]");
        Self { name: name.into(), frames: (secs * 60.0).ceil() as usize, next: 0, event, kind }
    }

    fn frame(&mut self, event: [u64; 5]) -> Option<usize> {
        let unchanged = self.event.is_some_and(|before| match self.kind {
            "note" => event[0] == before[0],
            "special" => event[1] == before[1],
            "equipment" => event[2] == before[2],
            "use" => event[3] == before[3],
            "level" => event[4] == before[4],
            _ => event[..2] == before[..2],
        });
        if unchanged || self.next == self.frames {
            return None;
        }
        self.event = None;
        let index = self.next;
        self.next += 1;
        Some(index)
    }
}

#[test]
fn live_capture_frame_accounting() {
    let mut capture = FrameCapture::new("attack:1:note", Some([4, 2, 0, 0, 0]));
    assert_eq!(capture.frame([4, 3, 0, 0, 0]), None); // an unrelated special cannot consume a note capture
    assert_eq!(capture.frame([5, 3, 0, 0, 0]), Some(0)); // triggering frame, not the following frame
    for index in 1..60 {
        assert_eq!(capture.frame([5, 3, 0, 0, 0]), Some(index));
    }
    assert_eq!(capture.frame([6, 4, 0, 0, 0]), None); // additional events never restart the sequence
    let mut special = FrameCapture::new("burst:1:special", Some([4, 2, 0, 0, 0]));
    assert_eq!(special.frame([5, 2, 0, 0, 0]), None);
    assert_eq!(special.frame([5, 3, 0, 0, 0]), Some(0));
    let mut direct = FrameCapture::new("npc:2", None);
    for index in 0..120 {
        assert_eq!(direct.frame([0; 5]), Some(index));
    }
    assert_eq!(direct.frame([0; 5]), None);
    assert_eq!(120.0 * CAPTURE_DT, 2.0);
}

#[test]
fn live_capture_action_selectors() {
    let before = [4, 2, 7, 8, 9];
    for (kind, selected) in [("equipment", 2), ("use", 3), ("level", 4)] {
        let mut capture = FrameCapture::new(&format!("action:1:{kind}"), Some(before));
        assert_eq!(capture.frame(before), None);
        for other in 0..5 {
            if other != selected {
                let mut event = before;
                event[other] += 1;
                assert_eq!(capture.frame(event), None);
            }
        }
        let mut event = before;
        event[selected] += 1;
        assert_eq!(capture.frame(event), Some(0));
        for index in 1..60 {
            assert_eq!(capture.frame(before), Some(index));
        }
        assert_eq!(capture.frame([99; 5]), None);
    }
    for spec in ["attack:1", "attack:1:either"] {
        let mut capture = FrameCapture::new(spec, Some(before));
        assert_eq!(capture.frame([4, 2, 8, 9, 10]), None);
        assert_eq!(capture.frame([4, 3, 8, 9, 10]), Some(0));
    }
}

struct Live {
    p: Play,
    o: Offscreen,
    last: Instant,
    shots: Option<std::path::PathBuf>,
    /// Longest frame time handed to the game (the autopilot sets it: a slow offscreen frame must not skip over a ramp edge).
    dt_cap: f32,
    capture: Option<FrameCapture>,
    npc_probe: Option<i32>,
}

impl Live {
    fn tick(&mut self) -> ao_gui::DrawList {
        self.tick_dt(None)
    }
    fn tick_dt(&mut self, fixed_dt: Option<f32>) -> ao_gui::DrawList {
        std::thread::sleep(Duration::from_millis(16));
        let dt = fixed_dt.unwrap_or_else(|| if self.capture.is_some() { CAPTURE_DT } else { self.last.elapsed().as_secs_f32().min(self.dt_cap) });
        self.last = Instant::now();
        let list = self.o.frame(&mut self.p, dt);
        if let Some(capture) = &mut self.capture {
            if let Some(index) = capture.frame(self.p.live_attack_events) {
                let path = self.shots.as_ref().expect("capture requires AOMAC_LIVE_SHOTS").join(format!("{}-{index:04}.png", capture.name));
                self.o.png(&self.p, &list, &path).unwrap();
                let camera = self.o.host.camera;
                let own = self.p.zone.own();
                eprintln!(
                    "frame shot {} sim_offset={:.6}s events={:?} camera_eye=({:.6},{:.6},{:.6}) camera_yaw={:.6} camera_pitch={:.6} own_server_pos={:?} own_server_yaw={:?}",
                    path.display(), index as f32 * CAPTURE_DT, self.p.live_attack_events,
                    camera.pos.x, camera.pos.y, camera.pos.z, camera.yaw, camera.pitch,
                    own.map(|d| d.pos), own.and_then(|d| d.yaw),
                );
                if let Some(id) = self.npc_probe {
                    match self.p.zone.world.npc_animation_probe(id) {
                        Some(probe) => eprintln!("npc frame={index:04} {probe}"),
                        None => eprintln!("npc frame={index:04} id={id} animation unavailable"),
                    }
                }
                self.last = Instant::now(); // readback must not inflate the next ordinary simulation step
            }
            if capture.next == capture.frames {
                self.capture = None;
            }
        }
        list
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
    fn capture(&mut self, spec: &str, triggered: bool) {
        assert!(self.shots.is_some(), "capture requires AOMAC_LIVE_SHOTS");
        assert!(self.capture.is_none(), "previous capture still armed or running");
        self.capture = Some(FrameCapture::new(spec, triggered.then_some(self.p.live_attack_events)));
    }

    fn capture_wait(&mut self, secs: f32) {
        assert!(secs.is_finite() && secs > 0.0, "capturewait requires positive finite seconds");
        let start = Instant::now();
        while self.capture.is_some() {
            assert!(start.elapsed().as_secs_f32() < secs, "capture timeout: selected own event missing or PNG sequence incomplete");
            self.tick();
        }
    }
    fn pos(&self) -> String {
        let mode = self.p.player.as_ref().map_or(0, |p| p.mode());
        let last_speed = self.p.player.as_ref().map(|p| if p.last_speed_walk() { "walk" } else { "run" });
        self.p.zone.own().map_or("?".into(), |d| format!("server pos {:.2} {:.2} {:.2} yaw {:.2} fsm mode {mode} last speed {last_speed:?}", d.pos[0], d.pos[1], d.pos[2], d.yaw.unwrap_or(0.0)))
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
        // the own dynel is dropped while a zone change (teleport / door) runs: the route is over
        let Some(pos) = p.zone.own().map(|d| d.pos) else { return true };
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

/// Deep-water ground points around the server position `pos` (liquid surface >= 1.19 m above the ground: `Collision::ground` is never below
/// level - 1.2, that is deep enough to swim), nearest first: `(distance, x, ground y, server z)`. A 151 x 151 lattice, 4 m apart or coarser for
/// a larger `radius`. The collision space has the z axis flipped (`player::to_col`).
fn deep_water(col: &ao_formats::playfield::collision::Collision, pos: [f32; 3], radius: f32) -> Vec<(f32, f32, f32, f32)> {
    let step = (radius / 75.0).max(4.0);
    let mut found = vec![];
    for ix in -75..=75 {
        for iz in -75..=75 {
            let (x, z) = (pos[0] + ix as f32 * step, pos[2] + iz as f32 * step);
            let Some(g) = col.ground([x, pos[1] + 50.0, -z]) else { continue };
            if col.liquid_at([x, g, -z]).is_some_and(|w| w.level - g >= 1.19) {
                found.push((((x - pos[0]).powi(2) + (z - pos[2]).powi(2)).sqrt(), x, g, z));
            }
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found
}

/// Axis-aligned route over the 0.5 m cells the collision lets the character (`Collision::walk`) cross (server x, z): `(x, z)` waypoints.
/// `avoid` = hostile NPC positions: cells within `avoid_radius` metres of one are not crossed (except the neighbourhood of the start), so a walk past a
/// camp does not wake it.
pub(super) fn route(c: &ao_formats::playfield::collision::Collision, from: [f32; 3], to: (f32, f32), avoid: &[(f32, f32)], avoid_radius: f32) -> Vec<(f32, f32)> {
    use std::collections::{HashMap, VecDeque};
    let cell = 0.5f32;
    let at = |x: f32, z: f32| ((x / cell).round() as i32, (z / cell).round() as i32);
    let (start, goal) = (at(from[0], from[2]), at(to.0, to.1));
    let direct = ((from[0] - to.0).powi(2) + (from[2] - to.1).powi(2)).sqrt();
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
                let near = |p: &(f32, f32), r: f32| (p.0 - b[0]).powi(2) + (p.1 + b[2]).powi(2) < r * r;
                // an unreachable goal (a closed door) must not flood the whole playfield: stay inside an ellipse around start and goal
                let d = |p: (f32, f32)| ((p.0 - b[0]).powi(2) + (p.1 + b[2]).powi(2)).sqrt();
                if d((from[0], from[2])) + d(to) > 1.6 * direct + 40.0 {
                    continue;
                }
                if prev.contains_key(&m) || (avoid.iter().any(|p| near(p, avoid_radius)) && !near(&(from[0], from[2]), 10.0)) {
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
    if std::env::var_os("AOMAC_PREFS_DIR").is_none() {
        super::super::prefs::set_test_dir(std::env::temp_dir().join("aomac-live-prefs"));
    }
    // `AOMAC_AUDIO_LOG=1`: the real audio engine runs in the harness and `audio` steps print its status (combat music, voices)
    let audio = std::env::var_os("AOMAC_AUDIO_LOG").and_then(|_| ao_audio::Audio::start(&dir).map_err(|e| eprintln!("audio disabled: {e:#}")).ok()).inspect(|a| {
        // muted to the speakers unless `AOMAC_AUDIO_UNMUTE` is set (voices / RMS are still logged)
        if std::env::var_os("AOMAC_AUDIO_UNMUTE").is_none() {
            a.set_output_gain(0.0);
        }
    });
    let mut p = Play::new(dir, None, Some("Ithaca".into()), audio).unwrap();
    p.start_backdrop();
    p.servers = Some(ao_net::client::fetch_servers().map_err(|e| e.to_string()));
    let o = Offscreen::new(&p, (1280, 800)).unwrap();
    let shots = std::env::var_os("AOMAC_LIVE_SHOTS").map(std::path::PathBuf::from);
    if let Some(d) = &shots {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut l = Live { p, o, last: Instant::now(), shots, dt_cap: f32::INFINITY, capture: None, npc_probe: None };
    l.tick();
    let w = l.p.login_w.unwrap();
    l.p.gui.set_text(w, "username", &user);
    l.p.gui.set_text(w, "password", &pass);
    l.p.handle(Event::Clicked { window: w, view: "login_btn".into(), item: None }, &mut l.o.host);
    l.until("character list", 60, |p| p.screen == Screen::CharSelect);
    let cw = l.p.char_w.unwrap();
    // `AOMAC_LIVE_CC=<name>`: New Character, then the four scenes with real clicks at window coordinates (the offline `create::shots` ones: Atrox,
    // Tall/Heavy/head arrow, Soldier, the name typed), a `cc-<scene>` shot each (`AOMAC_LIVE_SHOTS`), Next on the first three and Finish on the
    // name scene: the request goes to the login server for real (a NEW character, announce it), `CharacterCreated` + `ZoneHandoff` take the app
    // through the exit cinematic into the world and `AOMAC_LIVE_STEPS` run there. The intro runs as in the game (`AOMAC_CC_SKIP_INTRO=1` skips it).
    let cc_name = std::env::var("AOMAC_LIVE_CC").ok();
    if let Some(name) = &cc_name {
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
            click(&mut l, 1170.0, 760.0); // Next / Finish
        }
    }
    // `AOMAC_LIVE_NEW=<name>:<CC breed 1..7>:<CC profession 1..14>`: New Character, the creation module sends its request (no scene clicks),
    // the login server's `CharacterCreated` + `ZoneHandoff` take the app into the world
    if cc_name.is_some() {
        // the creation module's Finish click sent the request: wait for the zone hand-off below
    } else if let Ok(spec) = std::env::var("AOMAC_LIVE_NEW") {
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
        l.until("selected preview upload", 120, |p| p.char_ready && p.preview_error.is_none());
        l.shot("charselect"); // the detail panel of the picked character (breed / gender / profession / location)
        l.p.handle(Event::Clicked { window: cw, view: "login_btn".into(), item: None }, &mut l.o.host);
    }
    l.until("world", 120, |p| p.screen == Screen::InWorld && matches!(p.fade, Fade::Hold));
    eprintln!("in world: {} (player built: {}, focus {:?})", l.pos(), l.p.player.is_some(), l.p.gui.focused_view());
    for step in std::env::var("AOMAC_LIVE_STEPS").unwrap_or_else(|_| "wait=2,shot=enter".into()).split(',') {
        let (k, v) = step.split_once('=').unwrap_or((step, ""));
        match k {
            "wait" => l.wait(v.parse().unwrap()),
            "arm" => l.capture(v, true),
            "capturewait" => l.capture_wait(v.parse().unwrap()),
            "npcprobe" => {
                let id = if v == "target" { l.p.zone.target.expect("npcprobe requires a selected NPC") } else { v.parse().expect("npcprobe requires target or instance id") };
                assert!(l.p.zone.world.chars.get(&id).is_some_and(|c| c.npc), "npcprobe requires an NPC");
                l.npc_probe = Some(id);
            }
            "npcwait" => {
                let (state, timeout) = v.split_once(':').unwrap_or((v, "30"));
                let state = match state {
                    "walk" => ao_net::n3::motion::AnimState::Walk,
                    "idle" => ao_net::n3::motion::AnimState::Idle,
                    _ => panic!("npcwait requires walk or idle"),
                };
                let timeout: f32 = timeout.parse().expect("npcwait timeout must be seconds");
                assert!(timeout.is_finite() && timeout > 0.0, "npcwait timeout must be positive");
                let id = l.npc_probe.expect("npcwait requires npcprobe");
                assert!(l.capture.is_none(), "npcwait cannot consume an armed capture");
                let start = Instant::now();
                loop {
                    l.tick_dt(Some(CAPTURE_DT));
                    if l.p.zone.world.npc_movement_probe(id) == Some(state) {
                        break;
                    }
                    assert!(start.elapsed().as_secs_f32() < timeout, "timeout waiting for NPC {id} {state:?}");
                }
                eprintln!("npcwait id={id} movement={state:?}");
            }
            "frames" => {
                l.capture(v, false);
                // PNG readback is wall-clock work, not simulation time.
                while l.capture.is_some() {
                    l.tick();
                }
            }
            // Keep movement held across screenshots for gait/ground-speed comparisons.
            "down" | "up" => l.key(code(v), k == "down"),
            "resize" => {
                let (w, h) = v.split_once('x').expect("resize=WxH");
                l.o.resize((w.parse().unwrap(), h.parse().unwrap()));
                l.wait(0.2);
            }
            "shot" => l.shot(v),
            "pos" => eprintln!("{}", l.pos()),
            "audio" => eprintln!("audio: {} combat={:?} layer={:?} music={:?}", l.p.audio.as_ref().map_or("off".into(), |a| a.status()), l.p.audio.as_ref().map(|a| a.combat_state()), l.p.audio.as_ref().and_then(|a| a.music_layer()), l.p.audio.as_ref().and_then(|a| a.now_playing())),
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
                // `approach=x:z` walks to a point; `approach=corpse` uses the nearest existing corpse without combat.
                let fixed = if v == "corpse" {
                    let me = l.p.zone.own().expect("approach requires an own dynel").pos;
                    let corpse = l.p.zone.world.prop_list().into_iter().filter(|p| p.0 == 0xc76a).min_by(|a, b| {
                        let dist = |p: [f32; 3]| (p[0] - me[0]).powi(2) + (p[2] - me[2]).powi(2);
                        dist(a.2).total_cmp(&dist(b.2))
                    }).expect("no corpse");
                    eprintln!("approaching corpse {}:{} at {},{}", corpse.0, corpse.1, corpse.2[0], corpse.2[2]);
                    Some((corpse.2[0], corpse.2[2]))
                } else {
                    v.split_once(':').map(|(x, z)| (x.parse::<f32>().unwrap(), z.parse::<f32>().unwrap()))
                };
                let id: i32 = if v == "target" { l.p.zone.target.unwrap() } else { v.parse().unwrap_or(0) };
                let dist = |pos: [f32; 3], goal: (f32, f32)| ((goal.0 - pos[0]).powi(2) + (goal.1 - pos[2]).powi(2)).sqrt();
                let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI;
                let (mut sign, mut turn_left_is_pos) = (1.0f32, true);
                let (t0, mut held): (Instant, Option<&str>) = (Instant::now(), None);
                let pf0 = l.p.zone.playfield;
                let (own, goal) = approach_state(&l.p.zone, fixed, id).expect("approach requires an own dynel and a destination");
                let (mut last_chk, mut last_err, mut last_dist) = (Instant::now(), 0.0f32, dist(own.pos, goal));
                while t0.elapsed().as_secs() < 60 {
                    l.tick();
                    // Match goto: death or a zone hand-off ends the old-world movement.
                    if l.p.fight.as_ref().is_some_and(|m| m.is_dying()) || l.p.zone.playfield != pf0 {
                        eprintln!("approach ended on death or zone change: {}", l.pos());
                        break;
                    }
                    let Some((me, to)) = approach_state(&l.p.zone, fixed, id) else {
                        eprintln!("approach ended: own dynel or destination removed: {}", l.pos());
                        break;
                    };
                    let distance = dist(me.pos, to);
                    if distance <= 2.5 { last_dist = distance; break; }
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
                            if distance > last_dist + 0.3 {
                                sign = -sign; // walking away: the heading convention is mirrored
                            }
                        } else if err.abs() > last_err.abs() + 0.05 {
                            turn_left_is_pos = !turn_left_is_pos;
                        }
                        (last_chk, last_err, last_dist) = (Instant::now(), err, distance);
                    }
                }
                if let Some(k) = held {
                    l.key(code(k), false);
                }
                eprintln!("after approach {id} (last measured {last_dist:.1} m): {}", l.pos());
            }
            // chat: `say=<line>` runs the line as if typed in the chat bar (`/say hi`, `/g Global hi`, `/tell X hi`; a line without a leading `/` is dropped by `run_line`)
            "chatdrop" => l.p.chat.as_ref().expect("chat hub").drop_connection(),
            // `login`: wait for the camp (`say=/camp`: the 40 s `Logout` timer) to end in the login screen; prints the open window count (the
            // chat / interact / target layers are torn down by `show_login`)
            "login" => {
                l.until("login screen after camp", 120, |p| p.screen == Screen::Login);
                l.wait(1.0);
                eprintln!("login screen: {} gui windows, chat layer {}, session {}", l.p.gui.window_ids().len(), l.p.chat.is_some(), l.p.session.is_some());
            }
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
            "buffs" => eprintln!("buffs time={} ncu={} entries={:?}", l.p.zone.nanos.time, l.p.zone.nanos.buffs.iter().map(|b| b.ncu_cost).sum::<i32>(), l.p.zone.nanos.buffs),
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
                let target = best.map(|b| ao_net::msg::Identity { kind: 50000, instance: *b.0 });
                l.p.zone.set_target(target);
                eprintln!("target {:?}", l.p.zone.selected_target());
            }
            "sel" => l.p.zone.set_target(Some(ao_net::msg::Identity { kind: 50000, instance: v.parse().unwrap() })),
            "fight" => {
                let m = l.p.fight.as_ref().unwrap();
                eprintln!("fight: attacking={} numbers={:?}", m.attacking(), m.numbers().iter().map(|n| n.text.as_str()).collect::<Vec<_>>());
            }
            // `water`: the nearest deep-water ground points (liquid surface >= 1.19 m above the ground: `Collision::ground` is never below level - 1.2, that is deep enough to swim) within 300 m, for `goto=x:z` (swimming runs)
            "water" => {
                let pos = l.p.zone.own().unwrap().pos;
                let col = ao_formats::playfield::collision::Collision::load(&ao_rdb::RecordStore::open(&ao_gui::client_dir()).unwrap(), l.p.zone.playfield.unwrap()).unwrap();
                let found = deep_water(&col, pos, v.parse().unwrap_or(300.0));
                eprintln!("water: {} cells; nearest (dist, x, ground, server z) {:?}", found.len(), &found[..found.len().min(5)]);
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
                    l.p.zone.set_target(Some(ao_net::msg::Identity { kind: 50000, instance: id }));
                }
                let hunted_s = hunted.map(|i| i.to_string());
                let v = hunted_s.as_deref().unwrap_or(v);
                let v = match v.parse::<i32>() {
                    Ok(id) => {
                        let d = &l.p.zone.dynels[&id];
                        resolved = format!("{}:{}", d.pos[0], d.pos[2]);
                        resolved.as_str()
                    }
                    Err(_) if !v.contains(':') => {
                        let me = l.p.zone.own().unwrap().pos;
                        let dist = |d: &crate::play::zone::DynelState| (d.pos[0] - me[0]).powi(2) + (d.pos[2] - me[2]).powi(2);
                        let d = l.p.zone.dynels.values().filter(|d| d.name == v).min_by(|a, b| dist(a).total_cmp(&dist(b))).expect("no such dynel");
                        resolved = format!("{}:{}", d.pos[0], d.pos[2]);
                        resolved.as_str()
                    }
                    Err(_) => v,
                };
                let (x, z) = v.split_once(':').unwrap();
                let col = ao_formats::playfield::collision::Collision::load(&ao_rdb::RecordStore::open(&ao_gui::client_dir()).unwrap(), l.p.zone.playfield.unwrap()).unwrap();
                // `AOMAC_LIVE_AVOID=<metres>` (e.g. 15): `goto` walks around living hostile NPCs (side 3) of the dynel list by that distance
                let avoid_radius: f32 = std::env::var("AOMAC_LIVE_AVOID").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let avoid: Vec<(f32, f32)> = if avoid_radius > 0.0 { l.p.zone.dynels.values().filter(|d| d.npc && d.side == 3 && d.health > 0).map(|d| (d.pos[0], d.pos[2])).collect() } else { vec![] };
                let path = route(&col, l.p.zone.own().unwrap().pos, (x.parse().unwrap(), z.parse().unwrap()), &avoid, avoid_radius);
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
                    // dying on the way (hostiles on the route) ends the walk: the server respawns the character at the playfield start
                    let died = l.p.fight.as_ref().is_some_and(|m| m.is_dying());
                    if died {
                        eprintln!("goto: the own character died at {}", l.pos());
                    }
                    if died || pilot.step(&mut l.p, &mut l.o.host) || t.elapsed().as_secs() > 240 || l.p.zone.playfield != pf0 {
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
            "heading" => {
                let target: f64 = v.parse().expect("heading requires server yaw in radians");
                assert!(target.is_finite(), "heading must be finite");
                let target = target.rem_euclid(std::f64::consts::TAU) as f32;
                let prefs_xml = std::fs::read_to_string(l.p.dir.join("cd_image/gui/Default/CharPrefs.xml")).unwrap_or_default();
                let sensitivity = super::super::controls::ControlPrefs::from_xml(&prefs_xml).mouse_turn_sensitivity;
                assert!(sensitivity.is_finite() && sensitivity > 0.0, "heading requires positive finite mouse sensitivity");
                let (x, y, button) = (640.0, 400.0, ao_gui::MouseButton::Right);
                l.p.input(ao_gui::InputEvent::MouseDown { x, y, button }, &mut l.o.host);
                for _ in 0..120 {
                    l.tick();
                    let yaw = l.p.zone.own().and_then(|own| own.yaw).expect("heading requires own server yaw");
                    let error = heading_error(target, yaw);
                    if error.abs() <= 1e-5 {
                        break;
                    }
                    l.p.game_input(GameInput::MouseMotion { dx: error * 1000.0 / sensitivity, dy: 0.0 }, &mut l.o.host);
                }
                l.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut l.o.host);
                l.tick();
                let own = l.p.zone.own().expect("heading requires own avatar");
                let yaw = own.yaw.expect("heading requires own server yaw");
                eprintln!("after {step}: own_server_yaw={yaw:?} own_server_pos={:?}", own.pos);
                assert!(heading_error(target, yaw).abs() <= 1e-4, "heading failed to converge: target={target:?} actual={yaw:?}");
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
            // `clickcorpse` / `rightcorpse`: real mouse click at a visible pick point of the nearest corpse (no direct use command).
            // `clickobj=<kind>:<instance>` / `rightobj=…`: same real mouse path for any visible world-object identity.
            "clickdyn" | "rightdyn" | "doubledyn" | "hoverdyn" | "clickcorpse" | "rightcorpse" | "clickobj" | "rightobj" => {
                let corpse = k.ends_with("corpse").then(|| {
                    let me = l.p.zone.own().unwrap().pos;
                    l.p.zone.world.prop_list().into_iter().filter(|p| p.0 == 0xC76A).min_by(|a, b| {
                        let dist = |p: [f32; 3]| (p[0] - me[0]).powi(2) + (p[2] - me[2]).powi(2);
                        dist(a.2).total_cmp(&dist(b.2))
                    }).map(|p| ao_net::msg::Identity { kind: p.0, instance: p.1 }).expect("no corpse")
                });
                let value = v.rsplit('+').next().unwrap();
                let id = if let Some(corpse) = corpse {
                    corpse
                } else if k.ends_with("obj") {
                    let (kind, instance) = value.split_once(':').expect("object identity must be kind:instance");
                    ao_net::msg::Identity { kind: kind.parse().unwrap(), instance: instance.parse().unwrap() }
                } else {
                    let instance = value.parse().unwrap_or_else(|_| {
                        let me = l.p.zone.own().unwrap().pos;
                        let dist = |d: &crate::play::zone::DynelState| (d.pos[0] - me[0]).powi(2) + (d.pos[2] - me[2]).powi(2);
                        *l.p.zone.dynels.iter().filter(|(_, d)| d.name == value).min_by(|a, b| dist(a.1).total_cmp(&dist(b.1))).expect("no such dynel").0
                    });
                    ao_net::msg::Identity { kind: 50000, instance }
                };
                let (cam, lens) = (l.o.host.camera, l.o.host.lens.unwrap_or_default());
                let mut hit = None;
                'g: for y in (0..800).step_by(4) {
                    for x in (0..1280).step_by(4) {
                        if l.p.gui.wants_mouse(x as f32, y as f32) {
                            continue;
                        }
                        let ray = crate::play::hud_target::pick_ray(&cam, &lens, (1280.0, 800.0), (x as f32, y as f32));
                        let hits = crate::play::interact_use::pick_objects(&ray, &l.p.zone);
                        let picked = if id.kind == 50000 { hits.contains(&id) } else { hits.first() == Some(&id) };
                        if picked {
                            hit = Some((x as f32, y as f32));
                            break 'g;
                        }
                    }
                }
                let (x, y) = hit.expect("dynel not on screen");
                eprintln!("{k} {id:?} at {x},{y} on_ground={}", l.p.zone.target_on_ground(id));
                l.o.host.mods = ao_gui::Modifiers { shift: v.contains("shift+"), ctrl: v.contains("ctrl+"), ..Default::default() };
                if k == "hoverdyn" {
                    l.p.input(ao_gui::InputEvent::MouseMove { x, y }, &mut l.o.host);
                    l.tick();
                    let list = l.tick();
                    let sprites: Vec<String> = list.cmds.iter().filter_map(|c| if let ao_gui::DrawCmd::Gfx { id, dst, .. } = c { (0x135..=0x14e).contains(&id.0).then(|| format!("{:#x}@{},{}", id.0, dst[0], dst[1])) } else { None }).collect();
                    eprintln!("pointer sprites {sprites:?} hide_os_cursor {}", l.o.host.hide_cursor);
                } else {
                    let button = if k.starts_with("right") { ao_gui::MouseButton::Right } else { ao_gui::MouseButton::Left };
                    // Dispatch against the camera/pose just picked: ticking between events moves a settling camera off thin corpse-edge hits.
                    for _ in 0..if k == "doubledyn" { 2 } else { 1 } {
                        for ev in [ao_gui::InputEvent::MouseMove { x, y }, ao_gui::InputEvent::MouseDown { x, y, button }, ao_gui::InputEvent::MouseUp { x, y, button }] {
                            if matches!(ev, ao_gui::InputEvent::MouseUp { .. }) {
                                let ray = crate::play::hud_target::pick_ray(&l.o.host.camera, &l.o.host.lens.unwrap_or_default(), (1280.0, 800.0), (x, y));
                                eprintln!("release hits {:?}", crate::play::interact_use::pick_objects(&ray, &l.p.zone));
                            }
                            l.p.input(ev, &mut l.o.host);
                        }
                    }
                    eprintln!("target after release {:?}", l.p.zone.selected_target());
                    l.tick();
                    eprintln!("target {:?}", l.p.zone.selected_target());
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
                for (id, v) in i.flagged(&l.p.zone) {
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
                eprintln!("talk {id}: {:?}", l.p.interact.as_mut().unwrap().default_action_on(&l.p.zone, ao_net::msg::Identity { kind: 50000, instance: id }));
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
                    eprintln!("prop {k}:{i} kind={k:#x} class={:#x?} can={c:?} dist={d:.1} at {:.1},{:.1},{:.1}", l.p.zone.world.item_class_of(k, i), p[0], p[1], p[2]);
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
                let slot = p.zone.inventory.iter().find_map(|(&slot, entry)| (ao_net::n3::inventory::item_identity(slot) == item || entry.id == item).then_some((slot, entry.item.low_id)));
                let added = slot.and_then(|(slot, low)| {
                    let info = p.hud.as_mut()?.item_info(&mut p.gui, low)?;
                    Some(p.interact.as_mut().unwrap().trade_add(&mut p.gui, slot, info))
                }).unwrap_or(false);
                eprintln!("tadd {v}: {added}");
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
            // `dclick=<slot hex>` or `dclick=item:<template id>` uses the actual inventory UI path.
            // Template selection is useful when unwearing moves an item to the next free bag slot.
            "dclick" => {
                let slot = if let Some(template) = v.strip_prefix("item:") {
                    let template: i32 = template.parse().expect("dclick item template id");
                    l.p.zone.inventory.iter().filter(|(_, item)| item.item.low_id == template)
                        .map(|(slot, _)| *slot).min().expect("dclick item not in inventory")
                } else {
                    u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap()
                };
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
                    "shopadd" => i.shop_add(gui, n, &p.zone),
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
    if std::env::var_os("AOMAC_LIVE_SELECT_AFTER").is_some() {
        let id = l.p.zone.char_id as i32;
        let dir = super::super::prefs::dir().expect("preferences directory");
        let live_head = l.p.zone.stat(64).expect("live HeadMesh stat");
        let created_head = cc_name.as_ref().map(|_| l.p.prefs.cc.head as usize);
        let shots = l.shots.clone();
        let client = l.p.dir.clone();
        drop(l); // Exercise SlotShuttingDown/SaveCache before the next selection loads it.
        let cache = ao_formats::character::ViewerCache::load(&dir);
        let appearance = cache.0.get(&id).expect("live appearance was persisted");
        assert_eq!(appearance.head_mesh(), live_head as u32, "cached head must match the live HeadMesh stat");
        let store = ao_rdb::RecordStore::open(&client).unwrap();
        let (breed, sex) = ao_formats::screens::wire_breed_sex(appearance.breed, appearance.sex).unwrap();
        let heads = ao_formats::character::head_table(&store, breed, sex, 2).unwrap();
        if let Some(head) = created_head {
            assert_eq!(appearance.head_mesh(), heads[head].mesh);
            assert_ne!(appearance.head_mesh(), heads[0].mesh, "live creation must choose a non-default head");
        }
        if want == "Aomacchvq" { assert_eq!(appearance.head_mesh(), 40099); }
        let entry = CharacterEntry {
            id, created: true, status: 1,
            info: ao_net::msg::CharacterInfo { id: 0, breed: appearance.breed, gender: appearance.sex, name: cc_name.unwrap_or_else(|| want.clone()), ..Default::default() },
            ..Default::default()
        };
        cached_select_shot(client, entry, shots);
    }
}

fn cached_select_shot(client: std::path::PathBuf, entry: CharacterEntry, shots: Option<std::path::PathBuf>) {
    let p = Play::new(client, None, None, None).unwrap();
    p.start_backdrop();
    let o = Offscreen::new(&p, (1280, 800)).unwrap();
    let mut select = Live { p, o, last: Instant::now(), shots, dt_cap: f32::INFINITY, capture: None, npc_probe: None };
    select.tick();
    select.p.show_characters(CharacterList { characters: vec![entry], allowed_characters: 1, ..Default::default() }, &mut select.o.host);
    select.p.select_row(0, &mut select.o.host);
    select.until("cached select preview upload", 120, |p| {
        assert!(p.preview_error.is_none(), "cached preview failed: {:?}", p.preview_error);
        p.char_ready
    });
    select.shot("select-after");
}

/// Offline retry of a real created character's saved appearance; never connects to the login/zone server.
#[test]
#[ignore]
fn cached_created_select_shot() {
    let id: i32 = std::env::var("AOMAC_SELECT_ID").expect("AOMAC_SELECT_ID").parse().unwrap();
    let dir = super::super::prefs::dir().expect("AOMAC_PREFS_DIR");
    let cache = ao_formats::character::ViewerCache::load(&dir);
    let appearance = cache.0.get(&id).expect("saved character");
    let client = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&client).unwrap();
    let (breed, sex) = ao_formats::screens::wire_breed_sex(appearance.breed, appearance.sex).unwrap();
    let heads = ao_formats::character::head_table(&store, breed, sex, 2).unwrap();
    assert_ne!(appearance.head_mesh(), heads[0].mesh);
    let shots = std::env::var_os("AOMAC_LIVE_SHOTS").map(std::path::PathBuf::from);
    if let Some(dir) = &shots { std::fs::create_dir_all(dir).unwrap(); }
    cached_select_shot(client, CharacterEntry {
        id, created: true, status: 1,
        info: ao_net::msg::CharacterInfo { id: 0, breed: appearance.breed, gender: appearance.sex, name: format!("Cached{id}"), ..Default::default() },
        ..Default::default()
    }, shots);
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

    #[test]
    fn approach_stops_when_own_or_destination_is_removed() {
        use crate::play::zone::{DynelState, Zone};
        let mut zone = Zone::new(1);
        let dynel = |pos| DynelState { name: String::new(), pos, yaw: None, npc: false, side: 0, level: 1, health: 5, max_health: 5 };
        zone.dynels.insert(1, dynel([1.0, 0.0, 2.0]));
        zone.dynels.insert(2, dynel([3.0, 0.0, 4.0]));
        assert_eq!(super::approach_state(&zone, None, 2).unwrap().1, (3.0, 4.0));
        zone.dynels.remove(&2);
        assert!(super::approach_state(&zone, None, 2).is_none());
        assert!(super::approach_state(&zone, Some((3.0, 4.0)), 2).is_some());
        zone.dynels.remove(&1);
        assert!(super::approach_state(&zone, Some((3.0, 4.0)), 2).is_none());
    }
}

#[cfg(test)]
mod water_tests {
    /// The deep-water scan is in server coordinates (collision z flipped): Borealis' lake is where the swimming `goto` of the live runs expects it.
    #[test]
    fn borealis_lake_is_found_in_server_coordinates() {
        let Ok(store) = ao_rdb::RecordStore::open(&ao_gui::client_dir()) else { return };
        let Ok(col) = ao_formats::playfield::collision::Collision::load(&store, 800) else { return };
        let found = super::deep_water(&col, [679.6, 72.8, 476.7], 700.0);
        let n = found.first().expect("deep water within 700 m");
        assert!(col.liquid_at([n.1, n.2, -n.3]).is_some_and(|w| w.level - n.2 >= 1.19), "{n:?}");
        eprintln!("nearest deep water of Borealis: {n:?}");
    }
}
