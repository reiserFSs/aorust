//! Headless screenshots of the play screens (login, character select, delete, every creation scene) through
//! `ao_render::Offscreen`. Runs only with `AOMAC_SHOT_DIR=<dir>` and the client installed; otherwise a no-op.
//! `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac shots -- --nocapture`, then inspect the PNGs.
use super::*;
use ao_render::{Frontend, Offscreen};
use std::path::Path;
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (1280, 800);

fn backdrop(p: &Play) {
    let store = RecordStore::open(&p.dir).unwrap();
    let scene = screens::login_world_scene(&store, 0).unwrap();
    p.tx.send(Bg::Backdrop(Ok(Box::new(scene)))).unwrap();
}

fn steps(p: &mut Play, o: &mut Offscreen, n: usize, dt: f32) -> ao_gui::DrawList {
    let mut list = Default::default();
    for _ in 0..n {
        list = o.frame(p, dt);
        std::thread::sleep(Duration::from_millis(10));
    }
    list
}

fn shot(p: &mut Play, o: &mut Offscreen, dir: &Path, name: &str) {
    let list = steps(p, o, 2, 0.016);
    o.png(p, &list, &dir.join(format!("{name}.png"))).unwrap();
    eprintln!("wrote {name}.png");
}

#[test]
fn shots() {
    let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return };
    let client = ao_gui::client_dir();
    if !client.join("cd_image/gui").exists() {
        return eprintln!("skipping: no client");
    }
    prefs::set_test_dir(out.join("prefs"));
    std::fs::create_dir_all(&out).unwrap();

    // login
    let mut p = Play::new(client.clone(), None, None, None).unwrap();
    let mut o = Offscreen::new(&p, SIZE).unwrap();
    backdrop(&p);
    steps(&mut p, &mut o, 3, 0.016);
    let w = p.login_w.unwrap();
    p.gui.set_text(w, "username", "aomac-verify");
    shot(&mut p, &mut o, &out, "1-login");

    // character select (fake list), delete dialog
    let mut p = Play::new(client, Some(0), None, None).unwrap();
    let mut o = Offscreen::new(&p, SIZE).unwrap();
    backdrop(&p);
    let t = Instant::now();
    while !p.char_ready && t.elapsed() < Duration::from_secs(20) {
        steps(&mut p, &mut o, 1, 0.05);
    }
    steps(&mut p, &mut o, 20, 0.05);
    shot(&mut p, &mut o, &out, "2-charselect");
    p.delete_pressed();
    shot(&mut p, &mut o, &out, "3-delete");
    p.close_dialog();

    // creation: every scene (intro skipped), then the next-scene transitions
    p.start_creation(&mut Host::headless());
    // Skip the intro locally; process-wide environment mutation races native getenv readers.
    let t = Instant::now();
    while p.cc.as_ref().is_some_and(|c| c.actors.is_empty()) {
        assert!(t.elapsed() < Duration::from_secs(20), "creation actors did not initialize");
        steps(&mut p, &mut o, 1, 0.05);
    }
    let c = p.cc.as_mut().unwrap();
    c.rig.jump(&[1, 1]);
    c.st = St::Pick(Sc::Breed);
    let t = Instant::now();
    let _ = t;
    // drive the module with real mouse/keyboard input at window coordinates (the screens' own button rects)
    wait_active(&mut p, &mut o, 0);
    steps(&mut p, &mut o, 150, 0.02);
    click(&mut p, &mut o, 1100.0, 380.0); // Atrox
    steps(&mut p, &mut o, 100, 0.02);
    shot(&mut p, &mut o, &out, "4-cc-breed");
    click(&mut p, &mut o, 1170.0, 760.0); // Next
    wait_active(&mut p, &mut o, 1);
    click(&mut p, &mut o, 1147.0, 221.0); // Tall
    click(&mut p, &mut o, 1150.0, 461.0); // Heavy
    click(&mut p, &mut o, 127.0, 135.0); // head arrow
    steps(&mut p, &mut o, 100, 0.02);
    shot(&mut p, &mut o, &out, "5-cc-appearance");
    click(&mut p, &mut o, 1170.0, 760.0);
    wait_active(&mut p, &mut o, 2);
    click(&mut p, &mut o, 1150.0, 280.0); // Soldier
    steps(&mut p, &mut o, 100, 0.02);
    shot(&mut p, &mut o, &out, "6-cc-profession");
    click(&mut p, &mut o, 1170.0, 760.0);
    wait_active(&mut p, &mut o, 3);
    p.input(InputEvent::Text("Aomacverify".into()), &mut o.host);
    steps(&mut p, &mut o, 100, 0.02);
    shot(&mut p, &mut o, &out, "7-cc-name");
    click(&mut p, &mut o, 1247.0, 38.0); // the close button: AskExitMessage
    assert!(p.dialog_w.is_some_and(|d| d.1 == DialogKind::ExitCc));
    shot(&mut p, &mut o, &out, "8-cc-exit-dialog");
    p.input(InputEvent::Key { key: Key::Escape, pressed: true, mods: Default::default() }, &mut o.host); // = No
    assert!(p.dialog_w.is_none() && p.screen == Screen::Create);
}

fn click(p: &mut Play, o: &mut Offscreen, x: f32, y: f32) {
    p.input(InputEvent::MouseMove { x, y }, &mut o.host);
    steps(p, o, 3, 0.02);
    p.input(InputEvent::MouseDown { x, y, button: MouseButton::Left }, &mut o.host);
    p.input(InputEvent::MouseUp { x, y, button: MouseButton::Left }, &mut o.host);
    steps(p, o, 3, 0.02);
}

fn wait_active(p: &mut Play, o: &mut Offscreen, scene: usize) {
    let t = Instant::now();
    while !p.cc.as_ref().is_some_and(|c| c.st == St::Active && c.cur == Some(Sc::from(scene))) {
        assert!(t.elapsed() < Duration::from_secs(90), "scene {scene} never became active");
        steps(p, o, 1, 0.05);
    }
    steps(p, o, 150, 0.02); // the actors' background loads
}
