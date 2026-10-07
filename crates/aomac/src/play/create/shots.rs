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

/// Actual login assets, including a world sky answer arriving after the backdrop replacement.
#[test]
fn backdrop_after_live_sky_matches_fresh_pixels() {
    let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return };
    let client = ao_gui::client_dir();
    if !client.join("cd_image/gui").exists() {
        return eprintln!("skipping: no client");
    }
    std::fs::create_dir_all(&out).unwrap();
    let store = RecordStore::open(&client).unwrap();
    for stage in 0..=1 {
        let mut p = Play::new(client.clone(), None, None, None).unwrap();
        if stage == 1 {
            p.screen = Screen::CharSelect;
        }
        let mut o = Offscreen::new(&p, SIZE).unwrap();
        backdrop(&p);
        steps(&mut p, &mut o, 3, 0.0);
        let blank = ao_gui::DrawList::default();
        let fresh = out.join(format!("backdrop-{stage}-fresh.png"));
        o.png(&p, &blank, &fresh).unwrap();
        let expected = image::open(&fresh).unwrap().to_rgba8();

        let mut world = screens::login_world_scene(&store, stage).unwrap();
        let env = world.environment.as_mut().expect("login environment");
        env.sky_color = [1.0, 0.0, 1.0];
        env.fog_color = [1.0, 0.0, 1.0];
        env.fog_start = 0.0;
        env.fog_end = 1.0;
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        o.host.live_sky = Some(Some(ao_render::LiveSky {
            start: 100.0,
            scale: 10.0,
            source: Box::new(move |time| {
                started_tx.send(time).unwrap();
                release_rx.recv().unwrap();
                world.clone()
            }),
        }));
        o.frame(&mut p, 0.0); // install the world worker
        o.host.sky_clock = Some(200.0);
        o.frame(&mut p, 0.0); // request at the old clock, then apply resync
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 100.0);
        release_tx.send(()).unwrap();
        // Drain the first answer and ask for another; the second stays blocked across reset.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            o.frame(&mut p, 0.5);
            if let Ok(time) = started_rx.try_recv() {
                assert!(time >= 205.0, "offscreen must apply the host sky clock");
                break;
            }
            assert!(Instant::now() < deadline, "live sky worker did not advance");
            std::thread::sleep(Duration::from_millis(10));
        }
        let world_path = out.join(format!("backdrop-{stage}-world.png"));
        o.png(&p, &blank, &world_path).unwrap();
        let world_pixels = image::open(&world_path).unwrap().to_rgba8();
        assert!(expected.pixels().zip(world_pixels.pixels()).any(|(a, b)| a.0.iter().zip(b.0).any(|(a, b)| a.abs_diff(b) > 2)),
            "live sky must visibly change the real backdrop before reset");

        backdrop(&p); // same delivery path as fresh login/character selection
        o.frame(&mut p, 0.0);
        release_tx.send(()).unwrap(); // obsolete in-flight worker result
        for tick in 0..8 {
            std::thread::sleep(Duration::from_millis(10));
            o.frame(&mut p, 0.5);
            let restored = out.join(format!("backdrop-{stage}-restored-{tick}.png"));
            o.png(&p, &blank, &restored).unwrap();
            let actual = image::open(&restored).unwrap().to_rgba8();
            assert_eq!(expected.dimensions(), actual.dimensions());
            assert!(expected.pixels().zip(actual.pixels()).all(|(a, b)| a.0.iter().zip(b.0).all(|(a, b)| a.abs_diff(b) <= 2)),
                "stage {stage}, delayed tick {tick}: backdrop differs from fresh login by more than 2/255 per channel");
        }
    }
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
