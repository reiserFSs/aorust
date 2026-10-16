//! Headless tests of the login flow against the real client GUI data (skipped without the client): the `Play` state
//! machine is driven through `handle`/`pump`/`frame`, no window or renderer. Error pages are recorded, never opened.
use super::*;
use std::net::{Ipv4Addr, TcpListener};
use std::time::{Duration, Instant};

// POSIX thread CPU time excludes descheduling and the background asset workers.
// These probes are test-only; production input contains no clocks or recorder.
fn input_cpu_now() -> Duration {
    #[repr(C)]
    struct Timespec { seconds: std::os::raw::c_long, nanos: std::os::raw::c_long }
    unsafe extern "C" {
        fn clock_gettime(clock: std::os::raw::c_int, time: *mut Timespec) -> std::os::raw::c_int;
    }
    let mut time = Timespec { seconds: 0, nanos: 0 };
    let clock = if cfg!(target_os = "macos") { 16 } else { 3 }; // Darwin / Linux CLOCK_THREAD_CPUTIME_ID
    // SAFETY: clock_gettime writes the supplied native timespec, which remains live for the call.
    assert_eq!(unsafe { clock_gettime(clock, &mut time) }, 0);
    Duration::new(time.seconds as u64, time.nanos as u32)
}

thread_local! {
    static INPUT_TIMING: std::cell::Cell<Option<[Duration; 7]>> = const { std::cell::Cell::new(None) };
}

pub(super) struct InputTiming {
    last: Duration,
    stages: [Duration; 7],
}

impl InputTiming {
    pub(super) fn start() -> Option<Self> {
        INPUT_TIMING.with(|slot| slot.get().map(|_| Self { last: input_cpu_now(), stages: [Duration::ZERO; 7] }))
    }

    pub(super) fn mark(timer: &mut Option<Self>, index: usize) {
        if let Some(timer) = timer {
            let now = input_cpu_now();
            timer.stages[index] += now - timer.last;
            timer.last = now;
        }
    }
}

impl Drop for InputTiming {
    fn drop(&mut self) {
        INPUT_TIMING.with(|slot| slot.set(Some(self.stages)));
    }
}

fn populated_icc_press_rig() -> Option<Rig> {
    let mut r = rig()?;
    let size = (5120, 1440);
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 25988 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_ithaca.rec")));
    r.enter();
    r.p.player.as_mut().expect("captured Testy player").select_camera_mode(3);
    // Play::frame derives the live picker flag from this preference every frame.
    r.p.hud.as_mut().expect("ICC HUD").dvalues.set_i64("BoundingBoxTargeting", 0);
    assert!(r.p.zone.dynels.len() > 50, "requires the populated ICC capture");
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        r.p.frame(0.016, size, &mut r.host);
        if r.p.zone.world.cpu_benchmark_scene().0 { break; }
        assert!(Instant::now() < deadline, "ICC actor models did not finish loading");
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..60 { r.p.frame(0.016, size, &mut r.host); }
    assert!(r.p.zone.triangle_targeting, "native triangle targeting must survive Play::frame preference refresh");
    let bodies = r.p.zone.world.pick_bodies(r.p.zone.char_id as i32);
    let (_, posed_triangles) = r.p.zone.world.cpu_benchmark_scene();
    eprintln!("ICC picker fixture: remote_chars={} posed_pick_bodies={} posed_remote_body_triangles={posed_triangles} native_triangles={}",
        r.p.zone.world.chars.len(), bodies.len(), r.p.zone.triangle_targeting);
    assert!(posed_triangles > 0, "requires real skinned remote CAT bodies, not only loaded templates");
    Some(r)
}

#[test]
fn populated_icc_left_mouse_down_does_not_pick_native_triangles() {
    let Some(mut r) = populated_icc_press_rig() else { return };
    let (x, y, button) = (2560.0, 720.0, ao_gui::MouseButton::Left);
    assert!(!r.p.gui.wants_mouse(x, y) && r.p.game_input_open());
    assert!(r.p.zone.world.cpu_benchmark_scene().1 > 0, "real triangle bodies must be loaded");
    let before = super::super::hud_pick::triangle_pick_calls();
    for body in r.p.zone.world.pick_bodies(r.p.zone.char_id as i32).into_iter()
        .filter(|body| body.id != r.p.zone.char_id as i32) {
        let local = (glam::Vec3::from(body.bounds.0) + glam::Vec3::from(body.bounds.1)) * (0.5 * body.scale);
        let centre = glam::Vec3::from(body.pos) + glam::Quat::from_rotation_y(body.yaw) * local;
        let camera = ao_render::Camera::look_at(centre + glam::Vec3::Z * 3.0, centre);
        let ray = super::super::hud_target::pick_ray(&camera, &r.host.lens.unwrap_or_default(),
            (5120.0, 1440.0), (x, y));
        std::hint::black_box(super::super::interact_use::pick_objects(&ray, &r.p.zone, r.p.player.as_ref()));
        if super::super::hud_pick::triangle_pick_calls() > before { break; }
    }
    assert!(super::super::hud_pick::triangle_pick_calls() > before,
        "aiming through loaded remote body centres must exercise native triangle queries");
    let before = super::super::hud_pick::triangle_pick_calls();
    r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host);
    assert_eq!(super::super::hud_pick::triangle_pick_calls(), before,
        "LMB press must only arm camera capture, not pick triangles");
    assert!(r.host.look);
    r.p.game_input(ao_render::GameInput::MouseMotion { dx: 10.0, dy: 0.0, dt: 0.016 }, &mut r.host);
    r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host);
}

/// Actual CPU scopes, not a callback scheduler model, GPU time, or OS event latency.
#[test]
#[ignore = "loads real ICC assets and prints warmed thread-CPU distributions"]
fn mode3_icc_press_cpu_benchmark() {
    let mut r = populated_icc_press_rig().expect("CPU benchmark requires the installed client assets");
    let size = (5120, 1440);
    let (x, y) = (2560.0, 720.0);
    let (_, triangles) = r.p.zone.world.cpu_benchmark_scene();
    let owns_mouse = r.p.gui.wants_mouse(x, y);
    eprintln!("ICC zone_ithaca.rec Testy mode=3 dynels={} actors={} posed_remote_body_triangles={} native_triangles={} gui_owns_mouse={owns_mouse} text_focused={} input_open={}",
        r.p.zone.dynels.len(), r.host.actors.len(), triangles, r.p.zone.triangle_targeting,
        r.p.gui.text_focused(), r.p.game_input_open());
    assert!(triangles > 0 && !r.host.actors.is_empty());
    assert!(!owns_mouse && r.p.game_input_open(), "press must reach the world");

    fn report(name: &str, samples: &mut [Duration]) {
        samples.sort_unstable();
        let n = samples.len();
        let us = |d: Duration| d.as_secs_f64() * 1e6;
        eprintln!("{name}: n={n} median={:.3}us p95={:.3}us max={:.3}us sum={:.3}us",
            us(samples[n / 2]), us(samples[(n * 95).div_ceil(100) - 1]), us(samples[n - 1]),
            us(samples.iter().copied().sum()));
    }

    const WARM: usize = 20;
    const SAMPLES: usize = 200;
    let names = ["prelude", "Gui.wants_mouse", "Player.mouse", "interact_mouse",
        "chat.input+screen_dispatch", "Hud.input+click_dispatch", "Gui.input+event_dispatch"];
    // First button press after scene loading, before any input warmups: catches
    // one-off input cache stalls. This is not a cold process or unloaded scene.
    let button = ao_gui::MouseButton::Left;
    INPUT_TIMING.with(|slot| slot.set(Some([Duration::ZERO; 7])));
    let wall = Instant::now();
    let cpu = input_cpu_now();
    r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host);
    let press_cpu = input_cpu_now() - cpu;
    let press_wall = wall.elapsed();
    let recorded = INPUT_TIMING.with(|slot| slot.replace(None)).unwrap();
    eprintln!("FIRST LMB press before input warmups: threadCPU={:.3}us wall={:.3}us",
        press_cpu.as_secs_f64() * 1e6, press_wall.as_secs_f64() * 1e6);
    for (name, elapsed) in names.iter().zip(recorded) {
        eprintln!("FIRST LMB {name}: threadCPU={:.3}us", elapsed.as_secs_f64() * 1e6);
    }
    assert!(r.host.look);
    let camera_before = r.host.camera.yaw;
    r.p.game_input(ao_render::GameInput::MouseMotion { dx: 1.0, dy: 0.0, dt: 0.016 }, &mut r.host);
    let wall = Instant::now();
    let cpu = input_cpu_now();
    r.p.frame(0.016, size, &mut r.host);
    let frame_cpu = input_cpu_now() - cpu;
    let frame_wall = wall.elapsed();
    let turn = (r.host.camera.yaw - camera_before + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    assert!((turn - 0.01).abs() < 1e-4, "first LMB count must orbit in its first frame");
    eprintln!("FIRST LMB press+delta frame: threadCPU={:.3}us wall={:.3}us turn={turn}",
        frame_cpu.as_secs_f64() * 1e6, frame_wall.as_secs_f64() * 1e6);
    r.p.game_input(ao_render::GameInput::MouseMotion { dx: -11.0, dy: 0.0, dt: 0.016 }, &mut r.host);
    r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host);
    r.p.frame(0.016, size, &mut r.host);
    for button in [ao_gui::MouseButton::Left, ao_gui::MouseButton::Right] {
        let mut press = Vec::with_capacity(SAMPLES);
        let mut stages: [Vec<Duration>; 7] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
        let mut first_frame = Vec::with_capacity(SAMPLES);
        let mut baseline = Vec::with_capacity(SAMPLES);
        let mut release = Vec::with_capacity(SAMPLES);
        let mut plain_press = Vec::with_capacity(SAMPLES);
        let mut click_release = Vec::with_capacity(SAMPLES);
        let mut press_wall_samples = Vec::with_capacity(SAMPLES);
        let mut plain_press_wall_samples = Vec::with_capacity(SAMPLES);
        let mut frame_wall_samples = Vec::with_capacity(SAMPLES);
        let mut baseline_wall_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..WARM + SAMPLES {
            let wall = Instant::now();
            let before = input_cpu_now();
            r.p.frame(0.016, size, &mut r.host);
            let baseline_cpu = input_cpu_now() - before;
            let baseline_wall = wall.elapsed();
            INPUT_TIMING.with(|slot| slot.set(Some([Duration::ZERO; 7])));
            let wall = Instant::now();
            let before = input_cpu_now();
            r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host);
            let press_cpu = input_cpu_now() - before;
            let press_wall = wall.elapsed();
            let recorded = INPUT_TIMING.with(|slot| slot.replace(None)).unwrap();
            assert!(r.host.look, "{button:?} press must capture in input");
            // Alternate direction to keep the warmed scene/camera local rather than orbiting around ICC.
            let delta = if sample % 2 == 0 { 1.0 } else { -1.0 };
            let camera_before = r.host.camera.yaw;
            r.p.game_input(ao_render::GameInput::MouseMotion { dx: delta, dy: 0.0, dt: 0.016 }, &mut r.host);
            if button == ao_gui::MouseButton::Right {
                // Retail RMB START discards its delta; the next event is movement.
                // Deliver that event before the same first frame, not after a redraw.
                r.p.game_input(ao_render::GameInput::MouseMotion { dx: delta, dy: 0.0, dt: 0.016 }, &mut r.host);
            }
            let wall = Instant::now();
            let before = input_cpu_now();
            r.p.frame(0.016, size, &mut r.host);
            let frame_cpu = input_cpu_now() - before;
            let frame_wall = wall.elapsed();
            let turn = (r.host.camera.yaw - camera_before + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            assert!((turn - 0.01 * delta).abs() < 1e-4,
                "{button:?} movement after START must affect the same first frame; turn={turn}");
            // Make this a drag so repeated releases do not select/use nearby NPCs.
            r.p.game_input(ao_render::GameInput::MouseMotion { dx: 10.0 * delta, dy: 0.0, dt: 0.016 }, &mut r.host);
            let before = input_cpu_now();
            r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host);
            let release_cpu = input_cpu_now() - before;
            assert!(!r.host.look);
            r.p.frame(0.016, size, &mut r.host);
            let wall = Instant::now();
            let before = input_cpu_now();
            r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host);
            let plain_press_cpu = input_cpu_now() - before;
            let plain_press_wall = wall.elapsed();
            let before = input_cpu_now();
            r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host);
            let click_cpu = input_cpu_now() - before;
            if sample >= WARM {
                press.push(press_cpu);
                for (samples, elapsed) in stages.iter_mut().zip(recorded) { samples.push(elapsed); }
                first_frame.push(frame_cpu);
                baseline.push(baseline_cpu);
                release.push(release_cpu);
                plain_press.push(plain_press_cpu);
                click_release.push(click_cpu);
                press_wall_samples.push(press_wall);
                plain_press_wall_samples.push(plain_press_wall);
                frame_wall_samples.push(frame_wall);
                baseline_wall_samples.push(baseline_wall);
            }
        }
        eprintln!("{button:?}: {WARM} discarded warmups; LMB=one count, RMB=START then one count before same first frame; dt=16ms; no GPU/window");
        report("full Play::input MouseDown (stage recorder enabled)", &mut press);
        report("full Play::input MouseDown (stage recorder disabled)", &mut plain_press);
        for (name, samples) in names.iter().zip(&mut stages) { report(name, samples); }
        report("baseline Play::frame", &mut baseline);
        report("first press+delta Play::frame", &mut first_frame);
        report("full Play::input drag MouseUp (not click picking)", &mut release);
        report("full Play::input click MouseUp (one authoritative pick)", &mut click_release);
        report("WALL full Play::input MouseDown (stage recorder enabled)", &mut press_wall_samples);
        report("WALL full Play::input MouseDown (stage recorder disabled)", &mut plain_press_wall_samples);
        report("WALL baseline Play::frame", &mut baseline_wall_samples);
        report("WALL first press+delta Play::frame", &mut frame_wall_samples);
    }
    // Isolated native picker scopes model only the removed duplicate computation;
    // they are NOT measured old Play::input or end-to-end before/after latency.
    let ray = super::super::hud_target::pick_ray(&r.host.camera, &r.host.lens.unwrap_or_default(),
        (size.0 as f32, size.1 as f32), (x, y));
    for count in [1, 2] {
        let mut samples = Vec::with_capacity(SAMPLES);
        for sample in 0..WARM + SAMPLES {
            let before = input_cpu_now();
            for _ in 0..count {
                std::hint::black_box(super::super::interact_use::pick_objects(&ray, &r.p.zone, r.p.player.as_ref()));
            }
            let elapsed = input_cpu_now() - before;
            if sample >= WARM { samples.push(elapsed); }
        }
        report(&format!("MODELED release native pick_objects x{count}, not old full input"), &mut samples);
    }
}

#[test]
fn nano_audio_logs_oneshots_and_refusals_not_successful_rearms() {
    assert!(log_nano_sound(0.0, &[11]));
    assert!(log_nano_sound(-1.0, &[11]));
    assert!(log_nano_sound(0.2, &[]));
    assert!(!log_nano_sound(0.2, &[11]));
}

#[test]
fn anti_aliased_text_applies_on_login_without_a_hud() {
    let Some(mut r) = rig() else { return };
    assert!(r.p.hud.is_none());
    r.host.window_scale = 1;
    r.p.smooth_text = true;
    r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(r.p.gui.glyph_atlas().data.iter().any(|&b| b > 0 && b < 255));
    r.p.smooth_text = false;
    r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(r.p.gui.glyph_atlas().data.iter().all(|&b| b == 0 || b == 255));
}

#[test]
fn ground_texture_refresh_discards_superseded_worker_results() {
    let Some(mut r) = rig() else { return };
    r.p.screen = Screen::InWorld;
    r.p.ground_texture_serial = 3;
    let generation = r.host.scene_generation();
    r.p.tx.send(Bg::GroundTextures(2, Ok(HashMap::new()))).unwrap();
    r.p.pump(&mut r.host);
    assert!(r.host.ground_textures.is_none());
    r.p.tx.send(Bg::GroundTextures(3, Ok(HashMap::new()))).unwrap();
    r.p.pump(&mut r.host);
    assert!(r.host.ground_textures.is_some());
    assert_eq!(r.host.scene_generation(), generation);
}

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
    let scratch = crate::play::prefs::test_dir();
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

/// Real offscreen replay, not the logic-only Rig or PNG encoder. Run explicitly with AOMAC_PERF=1.
#[test]
#[ignore = "client data + GPU; performance replay"]
fn offscreen_icc_performance_replay() {
    assert!(std::env::var_os("AOMAC_PERF").is_some(), "requires AOMAC_PERF=1");
    let mut r = rig().expect("performance replay requires client data");
    let mut o = ao_render::Offscreen::new(&r.p, (1280, 800)).unwrap();
    let load = Instant::now();
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 25988 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_ithaca.rec")));
    o.host = r.host;
    loop {
        o.frame(&mut r.p, 1.0 / 60.0);
        if r.p.screen == Screen::InWorld && r.p.zone.world.cpu_benchmark_scene().0 { break; }
        assert!(load.elapsed() < Duration::from_secs(300), "ICC world/models did not load");
        std::thread::sleep(Duration::from_millis(16));
    }
    eprintln!("AOMAC_PERF replay zone_load_ms={:.3} (handoff through InWorld and actor readiness)", load.elapsed().as_secs_f64() * 1000.0);
    // The capture's Testy spawn and this offset define the same ICC view on both revisions.
    let own = r.p.zone.own().expect("captured Testy");
    let at = Vec3::new(own.pos[0], own.pos[1] + 1.6, -own.pos[2]);
    let camera = Camera::look_at(at + Vec3::new(0.0, 3.0, 12.0), at);
    o.fixed_camera(Some(camera));
    eprintln!("AOMAC_PERF replay camera={camera:?} dt=0.016666667");
    for _ in 0..120 { o.frame(&mut r.p, 1.0 / 60.0); }
    o.perf_reset();
    let mut list = o.frame(&mut r.p, 1.0 / 60.0);
    for _ in 1..600 { list = o.frame(&mut r.p, 1.0 / 60.0); }
    o.perf_report("replay");
    if let Some(path) = std::env::var_os("AOMAC_PERF_SHOT") {
        o.png(&r.p, &list, std::path::Path::new(&path)).unwrap();
    }
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
fn escape_info_mode_is_close_only_text_suppresses_and_dialogs_broadcast() {
    let Some(mut r) = rig() else { return };
    r.p.screen = Screen::InWorld;
    // Mirror the real first-world transition (Play::frame): login's editor no longer owns TextInputMode.
    r.p.gui.clear_focus();
    r.p.chat = Some(super::super::chat::Chat::new(false));
    r.p.hud = Some(super::super::hud::Hud::new(&mut r.p.gui, &r.p.dir, (1280, 800)).unwrap());
    let selected = ao_net::msg::Identity { kind: 50000, instance: 77 };
    let esc = || InputEvent::Key { key: Key::Escape, pressed: true, mods: ao_gui::Modifiers::default() };
    r.p.zone.set_target(Some(selected));
    r.p.chat.as_mut().unwrap().show_url(&mut r.p.gui, &r.p.zone, &r.p.text, "charid://50000/77");
    assert!(r.p.chat.as_ref().unwrap().info_view_open());
    r.p.input(esc(), &mut r.host);
    assert!(!r.p.chat.as_ref().unwrap().info_view_open());
    assert_eq!(r.p.zone.selected_target(), Some(selected));
    r.p.chat.as_mut().unwrap().duel_dialog(&mut r.p.gui, false, "test", &r.p.text);
    assert!(r.p.chat.as_ref().unwrap().esc_closes());
    let w = r.p.gui.open_window_xml("escape_editor", r#"<root><TextInputView name="editor"/></root>"#, (0, 0), ao_gui::WindowSize::Fixed(100, 30)).unwrap();
    r.p.gui.focus(w, "editor");
    r.p.input(esc(), &mut r.host);
    assert_eq!(r.p.zone.selected_target(), Some(selected));
    assert!(r.p.chat.as_ref().unwrap().esc_closes());
    r.p.gui.close_window(w);
    r.p.input(esc(), &mut r.host);
    assert_eq!(r.p.zone.selected_target(), None);
    assert!(!r.p.chat.as_ref().unwrap().esc_closes());
}
#[test]
fn mission_info_raw_row_shift_click_and_double_click_resolve_zone_quest_without_requests() {
    use ao_net::{msg::Identity, n3::quest::{Quest, RewardBox, QUEST_KIND}};
    let Some(mut r) = rig() else { return };
    r.p.screen = Screen::InWorld;
    r.p.gui.set_window_visible(r.p.login_w.unwrap(), false);
    r.p.gui.clear_focus();
    r.p.zone = zone::Zone::new(42);
    r.p.chat = Some(super::super::chat::Chat::new(false));
    r.p.hud = Some(super::super::hud::Hud::new(&mut r.p.gui, &r.p.dir, (1280, 800)).unwrap());
    let quest = Quest {
        id: Identity { kind: QUEST_KIND, instance: 71 },
        name: "Mission input fixture".into(),
        description: "Recover the sealed package.\\nReturn it to the mission contact.".into(),
        reward: RewardBox { ints: [247, 593, 0, 0, 0, 0, 0], ..Default::default() },
        ..Default::default()
    };
    let decoy = Quest { id: Identity { kind: QUEST_KIND, instance: 72 }, name: "Other mission".into(), description: "Not the requested mission.".into(), ..Default::default() };
    for q in [&decoy, &quest] {
        r.p.zone.quests.insert(q.id, q.clone());
    }
    let h = r.p.hud.as_mut().unwrap();
    h.set_mission_quests(&[decoy.clone(), quest.clone()]);
    h.open(&mut r.p.gui, super::super::hud::WindowKind::Mission);
    r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(r.p.chat.as_mut().unwrap().take_outbox().is_empty());
    assert!(r.p.hud.as_mut().unwrap().take_outbox().is_empty());

    let click = |r: &mut Rig, press_mods, release_mods, icon| {
        let (x, y) = r.p.hud.as_ref().unwrap().mission_row_point(&mut r.p.gui, quest.id, icon).expect("real Missions row");
        r.host.mods = press_mods;
        r.p.input(InputEvent::MouseMove { x, y }, &mut r.host);
        r.p.input(InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left }, &mut r.host);
        r.host.mods = release_mods;
        r.p.input(InputEvent::MouseUp { x, y, button: ao_gui::MouseButton::Left }, &mut r.host);
        r.p.frame(0.016, (1280, 800), &mut r.host);
        r.host.mods = Default::default();
    };
    let check_page = |r: &mut Rig| {
        assert!(r.p.chat.as_ref().unwrap().info_view_open());
        let w = r.p.gui.window_ids().into_iter().find(|&w| r.p.gui.has_view(w, "BrowserView")).expect("existing InfoView");
        let page = r.p.gui.text(w, "BrowserView");
        assert!(page.contains(&quest.name), "{page}");
        assert!(page.contains(&quest.description.replace("\\n", "<br>")), "{page}");
        assert!(!page.contains(&decoy.description), "{page}");
        for (key, amount) in [("NumCredits", 247), ("NumPoints", 593)] {
            let expected = r.p.text.by_key(506, key).expect("retail quest reward text").replacen("%d", &amount.to_string(), 1);
            assert!(page.contains(&expected), "{page}");
        }
        assert!(r.p.chat.as_mut().unwrap().take_outbox().is_empty(), "quest InfoView must resolve synchronously, not request character/item data");
        assert!(r.p.hud.as_mut().unwrap().take_outbox().is_empty());
    };

    click(&mut r, Default::default(), Default::default(), false);
    assert!(!r.p.chat.as_ref().unwrap().info_view_open(), "ordinary single click only selects");
    r.p.frame(0.6, (1280, 800), &mut r.host); // Not the second half of a double click.
    let ctrl = ao_gui::Modifiers { ctrl: true, ..Default::default() };
    let shift = ao_gui::Modifiers { shift: true, ..Default::default() };
    click(&mut r, ctrl, ctrl, false);
    assert!(!r.p.chat.as_ref().unwrap().info_view_open(), "Ctrl single click must not inspect");
    r.p.frame(0.6, (1280, 800), &mut r.host);
    click(&mut r, shift, Default::default(), true);
    assert!(!r.p.chat.as_ref().unwrap().info_view_open(), "Shift released before mouseup must not inspect");
    r.p.frame(0.6, (1280, 800), &mut r.host);
    click(&mut r, Default::default(), shift, true);
    check_page(&mut r);
    r.p.input(InputEvent::Key { key: Key::Escape, pressed: true, mods: Default::default() }, &mut r.host);
    assert!(!r.p.chat.as_ref().unwrap().info_view_open());
    r.p.frame(0.6, (1280, 800), &mut r.host);
    click(&mut r, Default::default(), Default::default(), false);
    assert!(!r.p.chat.as_ref().unwrap().info_view_open());
    click(&mut r, Default::default(), Default::default(), false);
    check_page(&mut r);
}


#[test]
fn character_info_first_response_and_refresh_use_current_packet_stats() {
    let Some(mut r) = rig() else { return };
    r.p.zone = zone::Zone::new(42);
    r.p.chat = Some(super::super::chat::Chat::new(false));
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
fn create_reference_physical_binding_fires_on_release_only() {
    let Some(mut r) = rig() else { return };
    r.p.screen = Screen::InWorld;
    r.p.awaiting_alive = false;
    r.p.gui.set_window_visible(r.p.login_w.unwrap(), false);
    r.p.gui.clear_focus();
    r.p.zone = zone::Zone::new(42);
    r.p.chat = Some(super::super::chat::Chat::new(false));
    let target = ao_net::msg::Identity { kind: 50000, instance: 77 };
    r.p.zone.dynels.insert(77, zone::DynelState { name: "Reference77".into(), pos: [0.0; 3], yaw: None, npc: false, side: 0, level: 1, health: 100, max_health: 100 });
    r.p.zone.set_target(Some(target));
    let mut hud = super::super::hud::Hud::new(&mut r.p.gui, &r.p.dir, (1280, 800)).unwrap();
    let provider = super::super::controls::provider_hash("ACTION_CREATEREF");
    hud.dvalues.set("KeyBindings", super::super::dvalue::Variant::Archive(format!("<Archive name=\"KeyBindings\" code=\"0\"><Int32 name=\"Version\" value=\"7\"/><Array name=\"Bind\"><Archive code=\"0\"><Int32 name=\"Input\" value=\"43\"/><Int64 name=\"Provider\" value=\"{provider}\"/></Archive></Array></Archive>")));
    r.p.hud = Some(hud);
    let key = |r: &mut Rig, pressed| {
        r.p.game_input(ao_render::GameInput::Key { code: ao_render::KeyCode::F5, pressed, repeat: false }, &mut r.host);
        r.p.frame(0.016, (1280, 800), &mut r.host);
    };
    key(&mut r, true);
    assert!(r.p.chat.as_mut().unwrap().take_outbox().is_empty(), "press must not request a reference");
    key(&mut r, false);
    assert_eq!(r.p.chat.as_mut().unwrap().take_outbox(), [ao_net::n3::outgoing::n3_frame(0, 42, ao_net::n3::info::request(42, target))]);
    assert!(!r.p.hud.as_mut().unwrap().take_create_reference(), "Play must consume the HUD action exactly once");
    key(&mut r, false);
    assert!(r.p.chat.as_mut().unwrap().take_outbox().is_empty(), "a binding capture's unarmed release must not act");
    key(&mut r, true);
    r.p.game_input(ao_render::GameInput::FocusLost, &mut r.host);
    key(&mut r, false);
    assert!(r.p.chat.as_mut().unwrap().take_outbox().is_empty(), "focus loss cancels the held provider");
}

#[test]
fn create_reference_waits_for_matching_player_info_and_uses_response_stats() {
    use ao_net::msg::Identity;
    let Some(mut r) = rig() else { return };
    r.p.zone = zone::Zone::new(42);
    r.p.chat = Some(super::super::chat::Chat::new(false));
    let player = Identity { kind: 50000, instance: 77 };
    let other = Identity { kind: 50000, instance: 78 };
    for id in [77, 78] {
        r.p.zone.dynels.insert(id, zone::DynelState { name: format!("Reference{id}"), pos: [0.0; 3], yaw: None, npc: false, side: 0, level: 1, health: 100, max_health: 100 });
    }
    let response = |id: Identity, title: u8| {
        let mut w = ao_net::Writer::default();
        w.u32(ao_net::n3::info::INFO_PACKET);
        id.write(&mut w);
        w.u8(0);
        for byte in [0, 1, 1, title, 0, 1] { w.u8(byte); }
        w.i16(0);
        for value in [100, 100, 0, 0] { w.i32(value); }
        for value in ["First", "Last", "", ""] { w.str_i16(value); }
        for value in [731, 29, 0] { w.i32(value); }
        for _ in 0..8 { w.i32(0); }
        ao_net::n3::outgoing::n3_frame(0, 42, w.0)
    };
    r.p.zone.set_target(Some(player));
    let chat = r.p.chat.as_mut().unwrap();
    chat.create_character_reference(&r.p.zone);
    assert_eq!(chat.take_outbox(), [ao_net::n3::outgoing::n3_frame(0, 42, ao_net::n3::info::request(42, player))]);
    assert!(chat.take_character_references().is_empty(), "no local snapshot");
    r.event(LoginEvent::ZoneFrame(response(other, 2)));
    assert!(r.p.chat.as_mut().unwrap().take_character_references().is_empty(), "unrelated response must not consume pending query");
    r.event(LoginEvent::ZoneFrame(response(player, 5)));
    let saved = r.p.chat.as_mut().unwrap().take_character_references();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0, player);
    let html = std::str::from_utf8(&saved[0].1).unwrap();
    assert!(html.contains("TitleLevel 5"), "{html}");
    assert!(html.contains("Reference77"), "{html}");
    let dated = html.find("<div indent=wrapped><font color=CCInfoHeader>Dated: </font>").unwrap();
    assert!(html[..dated].ends_with("</font>\n"));
    assert!(html[dated..].contains(":02</font></div>"));
    r.event(LoginEvent::ZoneFrame(response(player, 6)));
    assert!(r.p.chat.as_mut().unwrap().take_character_references().is_empty(), "completion is one-shot");
    r.p.chat.as_mut().unwrap().create_character_reference(&r.p.zone);
    r.p.zone.set_target(Some(other));
    r.p.chat.as_mut().unwrap().create_character_reference(&r.p.zone);
    r.p.chat.as_mut().unwrap().take_outbox();
    r.event(LoginEvent::ZoneFrame(response(player, 3)));
    assert!(r.p.chat.as_mut().unwrap().take_character_references().is_empty(), "latest CreateRef query supersedes the old target");
    r.event(LoginEvent::ZoneFrame(response(other, 4)));
    let saved = r.p.chat.as_mut().unwrap().take_character_references();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0, other);
    r.p.zone.set_target(Some(player));
    r.p.zone.dynels.get_mut(&77).unwrap().npc = true;
    r.p.chat.as_mut().unwrap().create_character_reference(&r.p.zone);
    assert!(r.p.chat.as_mut().unwrap().take_outbox().is_empty(), "NPC must not request a reference");
    r.p.zone.dynels.get_mut(&77).unwrap().npc = false;
    r.p.chat.as_mut().unwrap().create_character_reference(&r.p.zone);
    r.p.chat.as_mut().unwrap().take_outbox();
    r.p.zone.dynels.get_mut(&77).unwrap().npc = true;
    r.event(LoginEvent::ZoneFrame(response(player, 6)));
    assert!(r.p.chat.as_mut().unwrap().take_character_references().is_empty(), "NPC status must be checked again on completion");
    r.p.zone.dynels.get_mut(&77).unwrap().npc = false;
    r.event(LoginEvent::ZoneFrame(response(player, 6)));
    assert!(r.p.chat.as_mut().unwrap().take_character_references().is_empty(), "rejected completion still clears its query");
}

#[test]
fn stat_xp_feedback_uses_old_value_before_dispatch_applies_update() {
    let Some(mut r) = rig() else { return };
    r.p.zone = zone::Zone::new(42);
    // Stat feedback is emitted only for announced dynels (`LogCtx::known`).
    r.p.zone.dynels.insert(42, zone::DynelState {
        name: "XP dispatch fixture".into(), pos: [0.0; 3], yaw: None,
        npc: false, side: 0, level: 1, health: 100, max_health: 100,
    });
    let stat_frame = |xp| {
        let mut w = ao_net::Writer::default();
        w.u32(ao_net::n3::dynel::STAT);
        ao_net::msg::Identity { kind: 50000, instance: 42 }.write(&mut w);
        w.u8(0);
        w.i32(1);
        w.i32(52);
        w.i32(xp);
        ao_net::n3::outgoing::n3_frame(0, 42, w.0)
    };
    r.event(LoginEvent::ZoneFrame(stat_frame(100)));
    r.p.chat = Some(super::super::chat::Chat::new(false));
    r.event(LoginEvent::ZoneFrame(stat_frame(137)));
    assert_eq!(r.p.zone.stat(52), Some(137));
    let lines = r.p.chat.as_ref().unwrap().pending_log_lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let template = r.p.text.by_key(110, "Feedback_ReceivedXP").unwrap();
    let expected = super::super::chat::log::ldb_format(&template, &[super::super::chat::log::Arg::N(37)]);
    assert!(lines[0].text.contains(&expected), "{}", lines[0].text);
}

#[test]
fn captured_health_damage_applies_before_same_batch_stat_feedback() {
    let Some(mut r) = rig() else { return };
    let own = 0x8334;
    r.p.zone = zone::Zone::new(own);
    r.p.zone.dynels.insert(own as i32, zone::DynelState {
        name: "Captured heal fixture".into(), pos: [0.0; 3], yaw: None,
        npc: false, side: 0, level: 1, health: 46, max_health: 66,
    });
    r.p.zone.stats.insert(27, 46);
    r.p.zone.character_stats.entry(own as i32).or_default().insert(27, 46);
    r.p.chat = Some(super::super::chat::Chat::new(false));
    let frames = captured(include_str!("../../../../../docs/captures/health_damage_stat_clamp.rec"));
    assert_eq!(frames.len(), 3);
    // One pump consumes the entire real incoming batch, without a frame/tick sync.
    r.p.fake_events.extend(frames.into_iter().map(LoginEvent::ZoneFrame));
    r.p.pump(&mut r.host);
    assert_eq!(r.p.zone.stat(27), Some(66));
    assert_eq!(r.p.zone.character_stats[&(own as i32)][&27], 66);
    assert_eq!(r.p.zone.dynels[&(own as i32)].health, 66);
    let template = r.p.text.by_key(110, "Feedback_HealedForPoints").unwrap();
    let expected = |amount| super::super::chat::log::ldb_format(&template, &[super::super::chat::log::Arg::N(amount)]);
    let lines = r.p.chat.as_ref().unwrap().pending_log_lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].text.contains(&expected(50)), "{}", lines[0].text);
    assert!(!lines[0].text.contains(&expected(20)));
    // A genuine later Stat-only increase must still produce feedback.
    let mut w = ao_net::Writer::default();
    w.u32(ao_net::n3::dynel::STAT);
    ao_net::msg::Identity { kind: 50000, instance: own as i32 }.write(&mut w);
    w.u8(0);
    for value in [1, 27, 73] { w.i32(value); }
    r.event(LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(0, own, w.0)));
    let lines = r.p.chat.as_ref().unwrap().pending_log_lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[1].text.contains(&expected(7)), "{}", lines[1].text);
    // The same absolute write also updates an announced foreign character.
    let foreign: i32 = 77;
    let mut dynel = r.p.zone.dynels[&(own as i32)].clone();
    dynel.health = 46;
    r.p.zone.dynels.insert(foreign, dynel);
    r.p.zone.character_stats.entry(foreign).or_default().insert(27, 46);
    let mut frames = captured(include_str!("../../../../../docs/captures/health_damage_stat_clamp.rec"));
    for frame in &mut frames {
        frame.payload[8..12].copy_from_slice(&foreign.to_be_bytes());
    }
    r.event(LoginEvent::ZoneFrame(frames[0].clone()));
    assert_eq!(r.p.zone.stat_of(foreign, 27), Some(66));
    assert_eq!(r.p.zone.dynels[&foreign].health, 66);
    r.event(LoginEvent::ZoneFrame(frames[2].clone()));
    assert_eq!(r.p.chat.as_ref().unwrap().pending_log_lines().count(), 2);
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
fn fixture_handoffs_never_start_chat_connections() {
    let Some(mut r) = rig() else { return };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = [
        &1i32.to_be_bytes()[..], &9i32.to_be_bytes(), b"127.0.0.1",
        &i32::from(port).to_be_bytes(), &0f32.to_be_bytes(),
    ].concat();
    for fake in [false, true] {
        r.p.fake = fake;
        r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
        r.event(LoginEvent::ZoneFrame(ao_net::frame::Frame::system(1, 0, 0, 0x43, &body)));
        for _ in 0..3 {
            r.p.chat.as_mut().unwrap().update(&mut r.p.gui, 60.0, &r.p.text);
        }
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock,
            "fixture/replay handoff opened a chat socket");
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

#[test]
fn explicit_login_host_and_local_list_do_not_require_api_or_cached_endpoint() {
    let Some(mut r) = rig() else { return };
    r.p.server_arg = Some(format!("127.0.0.1:{}", r.port));
    r.p.servers = Some(Err("API unreachable".into()));
    r.p.request_servers();
    assert!(!r.p.servers_fetching, "explicit endpoint must bypass discovery");
    r.login();
    r.pump_until("explicit endpoint session", |p| p.session.is_some());
    r.cancel();
    // This is the merged local-file result when the remote API is unavailable.
    r.p.server_arg = Some("local".into());
    r.p.servers = Some(Ok(vec![ServerEntry { name: "Local".into(), ip: Ipv4Addr::LOCALHOST, port: r.port, players: 0 }]));
    r.login();
    r.pump_until("local entry session", |p| p.session.is_some());
    r.cancel();
    r.p.server_arg = Some("unrelated".into());
    assert!(r.p.server().unwrap_err().contains("unrelated"));
    r.p.server_arg = Some("127.0.0.1:0".into());
    assert!(r.p.server().unwrap_err().contains("Invalid login endpoint"));
    r.p.server_arg = None;
    r.p.prefs.server = "Local".into();
    r.p.servers = Some(Err("API unreachable".into()));
    assert!(r.p.server().unwrap_err().contains("Server list unavailable"));
    assert_eq!(r.p.prefs.server, "Local", "failed lookup preserves the selection");
}

#[test]
fn failed_discovery_retries_only_on_new_login_requests() {
    use std::io::Write;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let Some(mut r) = rig() else { return };
    let api = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    api.set_nonblocking(true).unwrap();
    let root = super::super::prefs::dir().unwrap();
    std::fs::write(root.join("apiServer.txt"), format!("http://{}", api.local_addr().unwrap())).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let worker = std::thread::spawn(move || {
        let start = Instant::now();
        while seen.load(Ordering::SeqCst) < 2 {
            assert!(start.elapsed() < Duration::from_secs(10), "missing explicit discovery retry");
            match api.accept() {
                Ok((mut stream, _)) => {
                    stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    seen.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => panic!("fixture API: {e}"),
            }
        }
    });
    r.p.server_arg = Some("Local".into());
    r.p.servers = Some(Err("API unreachable".into()));
    let w = r.p.login_w.unwrap();
    r.p.gui.set_text(w, "username", "aomac-verify");
    r.p.gui.set_text(w, "password", "not-a-password");
    r.click(w, "login_btn");
    assert!(r.p.servers_fetching && r.p.screen == Screen::Login);
    r.pump_until("first failed API retry", |p| !p.servers_fetching);
    assert!(r.p.opened_urls.last().unwrap().ends_with("/1.html"));
    for _ in 0..10 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1, "frames must not refetch");
    r.click(w, "login_btn");
    assert!(r.p.servers_fetching);
    r.pump_until("second failed API retry", |p| !p.servers_fetching);
    worker.join().unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    std::fs::remove_file(root.join("apiServer.txt")).unwrap();
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
    /// Install the captured world, drain its real asset worker, then relay the server's own CharInPlay echo.
    fn enter(&mut self) {
        let t = Instant::now();
        while self.p.screen != Screen::InWorld {
            assert!(t.elapsed() < Duration::from_secs(120), "timeout waiting for the world");
            self.p.frame(0.5, (1280, 800), &mut self.host);
            std::thread::sleep(Duration::from_millis(20));
        }
        while self.p.zone.world.has_workload() {
            assert!(t.elapsed() < Duration::from_secs(120), "timeout waiting for captured assets");
            self.p.frame(0.5, (1280, 800), &mut self.host);
            std::thread::sleep(Duration::from_millis(20));
        }
        let id = self.p.zone.char_id;
        self.event(LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(0, id, ao_net::n3::outgoing::char_in_play(id as i32))));
        self.p.frame(FADE_OUT, (1280, 800), &mut self.host);
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
        while self.p.teleporting || self.p.zone.world.has_workload() {
            assert!(t.elapsed() < Duration::from_secs(120), "timeout waiting for the new world");
            self.p.frame(0.5, (1280, 800), &mut self.host);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn loading_waits_for_alive_and_countdown_uses_native_boundaries() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    assert!(!r.p.game_input_open());
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    let deadline = Instant::now() + Duration::from_secs(120);
    while r.p.screen != Screen::InWorld {
        assert!(Instant::now() < deadline);
        r.p.frame(0.5, (1280, 800), &mut r.host);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(r.p.loading_active && matches!(r.p.fade, Fade::Hold) && !r.p.game_input_open(),
        "world installation must not close loading or enable input before the echo");
    r.p.zone.in_play_sent = false;
    r.p.world_frames = IN_PLAY_FRAMES;
    r.p.world_seconds = IN_PLAY_SECONDS;
    // Use the real queued placed-dynel request, before it is dispatched to the worker.
    r.p.zone.world.on_playfield(4604);
    for _ in 0..IN_PLAY_FRAMES + 2 { assert!(!r.p.in_play_due(0.0)); }
    assert!(r.p.world_frames < 0, "pending assets suppress sending even after countdown expiry");
    while r.p.zone.world.has_workload() {
        assert!(Instant::now() < deadline);
        r.p.frame(0.016, (1280, 800), &mut r.host);
        std::thread::sleep(Duration::from_millis(20));
    }
    r.p.zone.in_play_sent = false;
    assert!(r.p.in_play_due(0.0), "drained assets release the expired countdown");
    r.p.world_frames = IN_PLAY_FRAMES;
    r.p.world_seconds = IN_PLAY_SECONDS;
    for _ in 0..IN_PLAY_FRAMES { assert!(!r.p.in_play_due(0.0)); }
    assert_eq!(r.p.world_frames, 0);
    assert!(r.p.in_play_due(0.0), "strictly negative frame count, not zero");
    r.p.world_frames = IN_PLAY_FRAMES;
    r.p.world_seconds = IN_PLAY_SECONDS;
    assert!(!r.p.in_play_due(120.0), "zero seconds is not expired");
    assert!(r.p.in_play_due(0.25), "time alone releases before frame expiry");
    let echo = |id| LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(0, id, ao_net::n3::outgoing::char_in_play(id as i32)));
    r.event(echo(99));
    assert!(matches!(r.p.fade, Fade::Hold) && !r.p.game_input_open());
    r.event(echo(33512));
    assert!(matches!(r.p.fade, Fade::Out(0.0)) && r.p.game_input_open());
    r.p.frame(FADE_OUT, (1280, 800), &mut r.host);
    assert!(!r.p.loading_active);
    r.p.teleport_started(&mut r.host);
    r.p.teleport_ended();
    assert_eq!((r.p.world_frames, r.p.world_seconds), (IN_PLAY_FRAMES, IN_PLAY_SECONDS));
    assert!(!r.p.game_input_open() && !r.p.zone.in_play_sent);
    r.event(echo(33512));
    assert!(r.p.game_input_open() && !r.p.loading_active && matches!(r.p.fade, Fade::Hold));
    r.p.show_login(&mut r.host);
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    assert!(r.p.awaiting_alive && r.p.loading_active && !r.p.world_ready && !r.p.teleporting);
}

#[test]
fn own_scfu_after_world_install_builds_player_and_allows_camp() {
    use ao_net::n3::{dynel::Dynel, outgoing::n3_frame, N3};
    let Some(mut r) = rig() else { return };
    let own = 33512;
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: own });
    let frames = captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec"));
    let (late, burst): (Vec<_>, Vec<_>) = frames.into_iter().partition(|f| {
        matches!(ao_net::n3::decode(f), Ok(m) if m.header.target.instance == own as i32
            && matches!(m.body, N3::Dynel(Dynel::SimpleCharFullUpdate(_))))
    });
    assert!(!late.is_empty(), "fixture must contain the own SCFU");
    r.burst(&burst);
    let deadline = Instant::now() + Duration::from_secs(120);
    while r.p.screen != Screen::InWorld || r.p.zone.world.has_workload() {
        assert!(Instant::now() < deadline, "world must install without the own SCFU");
        r.p.frame(0.5, (1280, 800), &mut r.host);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(r.p.world_ready && r.p.world_scene.is_none() && r.p.player.is_none() && r.p.zone.own().is_none());
    // Neither native countdown expiry nor idle assets can replace GetClientControlDynel.
    r.p.frame(IN_PLAY_SECONDS + 1.0, (1280, 800), &mut r.host);
    assert!(!r.p.zone.in_play_sent && !r.p.game_input_open());
    r.host.fly = true; // A missing-avatar fallback must not survive successful late construction.
    r.burst(&late);
    r.p.frame(0.0, (1280, 800), &mut r.host);
    let player = r.p.player.as_ref().expect("late own SCFU constructs the installed world's player");
    assert_eq!(player.serial(), r.p.zone.own_serial);
    assert!(player.effect_surface().is_some(), "late construction loads the same native collision surface");
    assert!(!r.host.fly && !r.p.zone.in_play_sent);
    assert_eq!((r.p.world_frames, r.p.world_seconds), (IN_PLAY_FRAMES - 1, IN_PLAY_SECONDS),
        "SetMainDynel re-arms the native countdown before its first frame");
    while !r.p.zone.in_play_sent {
        assert!(Instant::now() < deadline, "late own dynel must release CharInPlay");
        r.p.frame(0.016, (1280, 800), &mut r.host);
        std::thread::sleep(Duration::from_millis(20));
    }
    r.event(LoginEvent::ZoneFrame(n3_frame(0, own, ao_net::n3::outgoing::char_in_play(own as i32))));
    assert!(r.p.game_input_open());
    r.p.frame(FADE_OUT, (1280, 800), &mut r.host);
    r.p.camp();
    assert_eq!(r.p.logout.state, super::super::logout::State::Login,
        "camp must reach the restored player rather than silently returning");
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
    assert_eq!(r.p.world_frames, IN_PLAY_FRAMES, "the countdown starts at TeleportEnded, not while the world loads");
    r.settle();
    assert!(r.p.hud.is_some() && matches!(r.p.fade, Fade::Hold), "TeleportEnded: no fade");
    for _ in 0..IN_PLAY_FRAMES + 2 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    assert!(!r.p.zone.in_play_sent, "native SendInPlay retries until the own control dynel exists");
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

#[test]
fn camp_queues_sit_movement_before_start_camping_without_next_frame_duplicate() {
    use ao_net::n3::{dynel::Dynel, world::World, N3};
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    r.p.fight.as_mut().unwrap().take_outbox();
    r.p.camp();
    let sent = r.p.fight.as_mut().unwrap().take_outbox();
    let ordered = sent.iter().filter_map(|f| match ao_net::n3::decode(f).unwrap().body {
        N3::Dynel(Dynel::CharDCMove(m)) => Some((false, i32::from(m.move_type))),
        N3::World(World::CharacterAction(a)) => Some((true, a.action)),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(ordered, [(false, 0x1e), (true, 0x78)]);
    let movement = r.p.player.as_mut().unwrap().frame(0.016, &mut r.host, &mut r.p.zone, false);
    assert!(!movement.iter().any(|f| matches!(
        ao_net::n3::decode(f).unwrap().body,
        N3::Dynel(Dynel::CharDCMove(m)) if m.move_type == 0x1e
    )));
}

#[test]
fn camp_keyboard_stand_waits_for_own_server_cancel_action() {
    use ao_net::{msg::Identity, n3::{action::{self, id}, outgoing::n3_frame, world::World, N3}};
    use super::super::logout::State;
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    for _ in 0..IN_PLAY_FRAMES + 2 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    assert!(r.p.game_input_open() && !r.p.gui.text_focused());
    r.p.chat.as_mut().unwrap().run_line(&mut r.p.gui, "/camp", &r.p.zone, &r.p.text);
    r.p.frame(0.016, (1280, 800), &mut r.host);
    r.p.frame(5.0, (1280, 800), &mut r.host);
    assert!(r.p.camp.is_some() && r.p.camp_bar.is_some());
    assert_eq!(r.p.logout.state, State::Login);
    r.p.fight.as_mut().unwrap().take_outbox();
    for pressed in [true, false] {
        r.p.game_input(ao_render::GameInput::Key { code: ao_render::KeyCode::KeyX, pressed, repeat: false }, &mut r.host);
    }
    r.p.frame(0.016, (1280, 800), &mut r.host);
    let sent = r.p.fight.as_mut().unwrap().take_outbox();
    let actions = sent.iter().filter_map(|f| match ao_net::n3::decode(f).unwrap().body {
        N3::World(World::CharacterAction(a)) => Some(a.action),
        _ => None,
    }).collect::<Vec<_>>();
    assert!(actions.contains(&id::STAND_UP), "{actions:?}");
    assert!(!actions.contains(&id::STOP_CAMPING), "{actions:?}");
    assert!(r.p.camp.is_some() && r.p.camp_bar.is_some(), "X waits for the server cancellation");
    assert_eq!(r.p.logout.state, State::Login);
    let own = Identity { kind: 50000, instance: r.p.zone.char_id as i32 };
    let cancel = action::simple(0x7a, Identity::default(), Identity::default());
    let stop = |who| LoginEvent::ZoneFrame(n3_frame(0, own.instance as u32, action::character_action_for(who, &cancel)));
    r.event(stop(Identity { instance: own.instance + 1, ..own }));
    assert!(r.p.camp.is_some() && r.p.camp_bar.is_some(), "another character cannot cancel our logout");
    assert_eq!(r.p.logout.state, State::Login);
    let stand = action::simple(id::STAND_UP, Identity::default(), Identity::default());
    r.event(LoginEvent::ZoneFrame(n3_frame(0, own.instance as u32, action::character_action_for(own, &stand))));
    r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(matches!(r.p.player.as_ref().unwrap().mode(), 2 | 3), "server stand restores an upright mode");
    assert!(r.p.camp.is_some() && r.p.camp_bar.is_some(), "stand echo alone does not cancel the countdown");
    r.event(stop(own));
    r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(r.p.camp.is_none() && r.p.camp_bar.is_none());
    assert_eq!(r.p.logout.state, State::None);
    let expected = r.p.text.by_key(200, "TimedLogoutAborted").expect("localized cancellation text");
    assert!(!expected.is_empty());
    assert!(r.p.gui.window_ids().into_iter().filter(|&w| r.p.gui.window_visible(w)).any(|w|
        r.p.gui.view_names(w).into_iter().any(|name|
            r.p.gui.is_visible(w, &name) && r.p.gui.text(w, &name).contains(&expected)
        )
    ), "localized cancellation line missing from visible chat: {expected:?}");
    for _ in 0..41 {
        r.p.frame(1.0, (1280, 800), &mut r.host);
    }
    assert!(r.p.screen == Screen::InWorld && !r.host.quit, "cancelled camp must neither log out nor quit later");
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

#[test]
fn focus_loss_resets_input_even_while_game_input_is_closed() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    let frames = |r: &mut Rig, n: i32| (0..n).for_each(|_| drop(r.p.frame(0.016, (1280, 800), &mut r.host)));
    frames(&mut r, IN_PLAY_FRAMES + 2);
    assert!(r.p.game_input_open());
    let key = |r: &mut Rig, code| r.p.game_input(ao_render::GameInput::Key { code, pressed: true, repeat: false }, &mut r.host);
    key(&mut r, ao_render::KeyCode::KeyW);
    frames(&mut r, 10);
    key(&mut r, ao_render::KeyCode::ControlLeft);
    key(&mut r, ao_render::KeyCode::ShiftLeft);
    r.p.player.as_mut().unwrap().mouse(
        &ao_gui::InputEvent::MouseDown { x: 640.0, y: 400.0, button: ao_gui::MouseButton::Right },
        false,
        &mut r.host,
    );
    r.p.game_input(ao_render::GameInput::MouseMotion { dx: 10.0, dy: 0.0, dt: 0.016 }, &mut r.host);
    frames(&mut r, 1);
    assert!(r.host.look);
    r.p.awaiting_alive = true;
    assert!(!r.p.game_input_open());
    r.host.look = false; // the viewer releases cursor capture before forwarding focus loss
    r.p.game_input(ao_render::GameInput::FocusLost, &mut r.host);
    frames(&mut r, 10);
    assert!(!r.host.look, "the player's stale look must not recapture the cursor");
    let stopped = r.p.zone.own().unwrap().pos;
    frames(&mut r, 60);
    let after = r.p.zone.own().unwrap().pos;
    assert!((after[0] - stopped[0]).abs() < 0.05 && (after[2] - stopped[2]).abs() < 0.05, "held movement stopped");
    r.p.awaiting_alive = false;
    key(&mut r, ao_render::KeyCode::KeyW);
    frames(&mut r, 60);
    let resumed = r.p.zone.own().unwrap().pos;
    let distance = ((resumed[0] - after[0]).powi(2) + (resumed[2] - after[2]).powi(2)).sqrt();
    assert!(distance > 1.0, "a fresh unmodified W press moves after refocus: {distance}");
    r.p.game_input(ao_render::GameInput::FocusLost, &mut r.host);
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
    // F8 below triggers OnChangeCameraMode: the shipped IsTipEnabled=true default would otherwise centre a
    // TipView window over the point this test later right-clicks and drags on.
    if let Some(hud) = r.p.hud.as_mut() { hud.dvalues.set_i64("IsTipEnabled", 0); }
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
        r.p.game_input(ao_render::GameInput::MouseMotion { dx: 25.0, dy: 0.0, dt: 0.016 }, &mut r.host);
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    r.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut r.host);
    for _ in 0..60 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    let turned = (r.p.zone.own().unwrap().yaw.unwrap_or(0.0) - yaw0 + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI;
    assert!(turned.abs() > 0.5 && turned.abs() < 1.5, "turned {turned} rad");
}

/// Real-window event ordering regression: `ao_render::viewer`'s `window_event` dispatches the RMB-down
/// `WindowEvent::MouseInput` straight to `Frontend::input`, and a `DeviceEvent::MouseMotion` can arrive
/// before the next `RedrawRequested`/`frame()` call -- the viewer only forwards raw deltas while
/// `host.look` is set (it gates `RawMouse::set_active`/cursor capture on it). `host.look` must therefore
/// flip inside `Frontend::input` itself, not wait for the next `frame()`: before the fix `Player::mouse`
/// left `host.look` stale until `Player::frame` ran, so a motion delta delivered between the button-down
/// event and the first subsequent frame was silently dropped by the viewer's `!s.looking` guard.
#[test]
fn mouse_look_captures_before_the_next_frame_not_after() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    for _ in 0..30 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    let yaw0 = r.p.zone.own().unwrap().yaw.unwrap_or(0.0);
    let (x, y, button) = (640.0, 400.0, ao_gui::MouseButton::Right);
    // No `r.p.frame()` call here: this is the exact order a real window delivers
    // WindowEvent::MouseInput(Right, pressed) followed by DeviceEvent::MouseMotion before any redraw.
    r.p.input(ao_gui::InputEvent::MouseDown { x, y, button }, &mut r.host);
    assert!(r.host.look, "RMB down must capture the cursor within the same input event, not the next frame()");
    for _ in 0..4 {
        r.p.game_input(ao_render::GameInput::MouseMotion { dx: 25.0, dy: 0.0, dt: 0.016 }, &mut r.host);
    }
    r.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut r.host);
    for _ in 0..60 {
        r.p.frame(0.016, (1280, 800), &mut r.host);
    }
    let turned = (r.p.zone.own().unwrap().yaw.unwrap_or(0.0) - yaw0 + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI;
    assert!(turned.abs() > 0.5 && turned.abs() < 1.5, "turned {turned} rad without an intervening frame() before the first motion");
}

/// Cursor capture freezes absolute pointer coordinates: only Controls' raw-count
/// release result may select the character under that unchanged pointer.
#[test]
fn world_lmb_release_uses_raw_click_classification_not_frozen_cursor_pixels() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    r.p.player.as_mut().unwrap().select_camera_mode(3);
    let size = (1280, 800);
    for _ in 0..30 { r.p.frame(0.016, size, &mut r.host); }
    // Use the actual posed character from the capture, with the retail
    // BoundingBoxTargeting preference, rather than a fabricated picking result.
    r.p.zone.triangle_targeting = false;
    let body = r.p.zone.world.pick_bodies(r.p.zone.char_id as i32).into_iter()
        .find(|b| b.id == r.p.zone.char_id as i32).expect("third-person own body is pickable");
    let local = (glam::Vec3::from(body.bounds.0) + glam::Vec3::from(body.bounds.1)) * (0.5 * body.scale);
    let centre = glam::Vec3::from(body.pos) + glam::Quat::from_rotation_y(body.yaw) * local;
    r.host.camera = ao_render::Camera::look_at(centre + glam::Vec3::Z * 3.0, centre);
    let (x, y, button) = (640.0, 400.0, ao_gui::MouseButton::Left);
    assert!(!r.p.gui.wants_mouse(x, y));
    let ray = super::super::hud_target::pick_ray(&r.host.camera, &r.host.lens.unwrap_or_default(), (1280.0, 800.0), (x, y));
    let under = super::super::interact_use::pick_objects(&ray, &r.p.zone, r.p.player.as_ref())[0];
    assert_eq!(under.kind, ao_net::n3::outgoing::DYNEL_CHAR);
    for (raw_counts, selects) in [(21.0, false), (1.0, true), (0.0, true)] {
        r.p.zone.set_target(None);
        r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host);
        assert!(r.host.look);
        if raw_counts != 0.0 {
            r.p.game_input(ao_render::GameInput::MouseMotion { dx: raw_counts, dy: 0.0, dt: 0.016 }, &mut r.host);
        }
        // No MouseMove and no redraw: the real locked-cursor release ordering.
        r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host);
        assert!(!r.host.look);
        assert_eq!(r.p.zone.selected_target(), selects.then_some(under), "{raw_counts} raw counts");
    }
}

/// Viewer release ordering: flush queued raw counts, then dispatch MouseUp,
/// without a redraw between the press and release. A tiny delta still orbits;
/// the 0.02 threshold classifies only the eventual click, never look activation.
#[test]
fn left_drag_orbits_on_first_delta_and_keeps_release_tail_at_ultrawide_size() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    let size = (5120, 1440);
    for _ in 0..30 {
        r.p.frame(0.016, size, &mut r.host);
    }
    let (x, y, button) = (2560.0, 720.0, ao_gui::MouseButton::Left);
    assert!(!r.p.gui.wants_mouse(x, y), "test press must reach the world");
    let yaw0 = r.p.zone.own().unwrap().yaw.unwrap_or(0.0);
    let camera0 = r.host.camera.yaw;
    r.p.input(ao_gui::InputEvent::MouseDown { x, y, button }, &mut r.host);
    assert!(r.host.look, "left press captures before any redraw");
    r.p.game_input(ao_render::GameInput::MouseMotion { dx: 1.0, dy: 0.0, dt: 0.016 }, &mut r.host);
    r.p.frame(0.016, size, &mut r.host);
    let angle = |a: f32, b: f32| (a - b + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    assert!((angle(r.host.camera.yaw, camera0) - 0.01).abs() < 1e-4,
        "one count orbits immediately, with no deadzone or angular damping");
    // Tail counts are delivered by viewer::State::flush_mouse BEFORE MouseUp,
    // not discarded when set_look(false) clears the raw accumulator.
    r.p.game_input(ao_render::GameInput::MouseMotion { dx: 30.0, dy: 0.0, dt: 0.016 }, &mut r.host);
    r.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut r.host);
    assert!(!r.host.look);
    assert!(r.p.player.as_mut().unwrap().take_clicks().is_empty(), "drag release must not become a click");
    r.p.frame(0.016, size, &mut r.host);
    assert!((angle(r.host.camera.yaw, camera0) - 0.31).abs() < 1e-4, "release tail included without another look frame");
    assert!(angle(r.p.zone.own().unwrap().yaw.unwrap_or(0.0), yaw0).abs() < 1e-6,
        "left orbit must not turn the character");
    // Entire second drag between redraws: no activation frame is needed.
    let before_flick = r.host.camera.yaw;
    r.p.input(ao_gui::InputEvent::MouseDown { x, y, button }, &mut r.host);
    assert!(r.host.look);
    let buffered_counts = (12.0 + 19.0, 0.0);
    r.p.game_input(ao_render::GameInput::MouseMotion { dx: buffered_counts.0, dy: buffered_counts.1, dt: 0.016 }, &mut r.host);
    r.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut r.host);
    assert!(!r.host.look);
    assert!(r.p.player.as_mut().unwrap().take_clicks().is_empty());
    r.p.frame(0.016, size, &mut r.host);
    assert!((angle(r.host.camera.yaw, before_flick) - 0.31).abs() < 1e-4);
}

/// Clocked callback-scheduler model through real GUI/Controls/Camera code.
/// This is a reproducible backlog scenario, not measured macOS runtime latency.
#[test]
fn lmb_press_to_first_orbit_before_and_after_native_queue_backlog() {
    let Some(mut r) = rig() else { return };
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
    r.enter();
    let size = (5120, 1440);
    for _ in 0..30 { r.p.frame(0.016, size, &mut r.host); }
    let (x, y, button) = (2560.0, 720.0, ao_gui::MouseButton::Left);
    assert!(!r.p.gui.wants_mouse(x, y));
    // Hardware moves one count 1 ms after press. Old default-main callback is
    // unavailable until 1000 ms; new independent callback is available at 1 ms.
    // At 16 ms redraw boundaries these produce first orbit at 1008 vs 16 ms.
    // The native API smoke separately asserts the real installed handler queue.
    for (callback_ms, expected_ms) in [(1000, 1008), (1, 16)] {
        let before = r.host.camera.yaw;
        let avatar = r.p.zone.own().unwrap().yaw;
        r.p.input(ao_gui::InputEvent::MouseDown { x, y, button }, &mut r.host);
        assert!(r.host.look, "capture must activate synchronously on press");
        let mut delivered = false;
        let mut first_orbit = None;
        for frame_ms in (16..=1024).step_by(16) {
            if !delivered && frame_ms >= callback_ms {
                r.p.game_input(ao_render::GameInput::MouseMotion { dx: 1.0, dy: 0.0, dt: 0.016 }, &mut r.host);
                delivered = true;
            }
            r.p.frame(0.016, size, &mut r.host);
            let angle = (r.host.camera.yaw - before + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            if angle.abs() > 1e-4 {
                assert!((angle - 0.01).abs() < 1e-4, "first count must orbit without an activation threshold");
                first_orbit = Some(frame_ms);
                break;
            }
        }
        assert_eq!(first_orbit, Some(expected_ms));
        assert_eq!(r.p.zone.own().unwrap().yaw, avatar, "orbit must not turn the avatar");
        r.p.input(ao_gui::InputEvent::MouseUp { x, y, button }, &mut r.host);
        assert!(!r.host.look);
    }
}

/// Synthetic event times, not measured native latency: press at 0 ms, one count at
/// 1 ms, first redraw at 16 ms. Compare capture and the actual camera/heading consumers.
#[test]
fn mouse_first_frame_timeline_across_camera_modes() {
    let size = (5120, 1440);
    let (x, y) = (2560.0, 720.0);
    let angle = |a: f32, b: f32| (a - b + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let motion = |r: &mut Rig, dx| {
        // `dt` is the host frame duration, not the synthetic event timestamp.
        r.p.game_input(ao_render::GameInput::MouseMotion { dx, dy: 0.0, dt: 0.016 }, &mut r.host);
    };
    for mode in 0..=3 {
        let Some(mut r) = rig() else { return };
        r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
        r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
        r.enter();
        r.p.player.as_mut().unwrap().select_camera_mode(mode);
        assert_eq!(r.p.player.as_ref().unwrap().camera_mode(), mode);
        for _ in 0..120 { r.p.frame(0.016, size, &mut r.host); }
        assert!(!r.p.gui.wants_mouse(x, y));

        // Retail RMB's first movement starts the turn; its next movement changes
        // heading. Mode 1's chase camera need not follow that heading immediately.
        let button = ao_gui::MouseButton::Right;
        let yaw0 = r.p.zone.own().unwrap().yaw.unwrap_or(0.0);
        r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host); // t=0
        assert!(r.host.look, "mode {mode}: RMB captures at t=0 before a frame");
        motion(&mut r, 1.0); // t=1
        r.p.frame(0.016, size, &mut r.host); // t=16
        let started = angle(r.p.zone.own().unwrap().yaw.unwrap_or(0.0), yaw0);
        assert!(started.abs() < 1e-4, "mode {mode}: initial RMB delta starts the turn");
        motion(&mut r, 2.0); // t=17
        r.p.frame(0.016, size, &mut r.host); // t=32
        let turned = angle(r.p.zone.own().unwrap().yaw.unwrap_or(0.0), yaw0);
        eprintln!("synthetic mode={mode} RMB capture=0ms delta=1ms frame16_heading={started:.6} delta=17ms frame32_heading={turned:.6} rad");
        assert!((turned - 0.02).abs() < 1e-4, "mode {mode}: next RMB delta turns on its first eligible frame");
        r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host);
        assert!(!r.host.look);

        for (label, button, ctrl) in [
            ("LMB", ao_gui::MouseButton::Left, false),
            ("Ctrl+RMB", ao_gui::MouseButton::Right, true),
        ] {
            for _ in 0..120 { r.p.frame(0.016, size, &mut r.host); }
            if ctrl {
                r.p.game_input(ao_render::GameInput::Key { code: ao_render::KeyCode::ControlLeft, pressed: true, repeat: false }, &mut r.host);
            }
            let camera0 = r.host.camera.yaw;
            let yaw0 = r.p.zone.own().unwrap().yaw.unwrap_or(0.0);
            r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host); // t=0
            assert!(r.host.look, "mode {mode}: {label} captures at t=0 before a frame");
            motion(&mut r, 1.0); // t=1, below the click-classification threshold
            r.p.frame(0.016, size, &mut r.host); // t=16
            let first = angle(r.host.camera.yaw, camera0);
            eprintln!("synthetic mode={mode} {label} capture=0ms delta=1ms frame16_camera={first:.6} rad");
            assert!((first - 0.01).abs() < 1e-4, "mode {mode}: {label} applies the full first count without a threshold or delay");
            assert!(angle(r.p.zone.own().unwrap().yaw.unwrap_or(0.0), yaw0).abs() < 1e-4);
            r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host);
            assert!(!r.host.look);

            // A second drag starts, delivers several early deltas and releases
            // entirely between redraws, including first-person EndLook's heading transfer.
            for _ in 0..120 { r.p.frame(0.016, size, &mut r.host); }
            let camera0 = r.host.camera.yaw;
            r.p.input(InputEvent::MouseDown { x, y, button }, &mut r.host); // t=0
            assert!(r.host.look);
            for dx in [1.0, 2.0, 3.0] { motion(&mut r, dx); } // t=1,2,3
            r.p.input(InputEvent::MouseUp { x, y, button }, &mut r.host); // t=4
            assert!(!r.host.look);
            r.p.frame(0.016, size, &mut r.host); // t=16
            let total = angle(r.host.camera.yaw, camera0);
            eprintln!("synthetic mode={mode} {label} deltas=1,2,3ms release=4ms frame16_camera={total:.6} rad");
            assert!((total - 0.06).abs() < 1e-4, "mode {mode}: {label} loses none of the early counts");
            if ctrl {
                r.p.game_input(ao_render::GameInput::Key { code: ao_render::KeyCode::ControlLeft, pressed: false, repeat: false }, &mut r.host);
            }
            r.p.player.as_mut().unwrap().take_clicks();
        }
    }
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
    let frames = |r: &mut Rig, n: i32| (0..n).for_each(|_| drop(r.p.frame(0.016, (1280, 800), &mut r.host)));
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
    r.p.audio = Some(ao_audio::Audio::offline(&r.p.dir, 44100));
    let audio = r.p.audio.as_ref().unwrap();
    audio.play_startup_music();
    assert!(audio.now_playing().is_some());
    r.host.live_sky = None; // The viewer drains the world's live-sky request.
    r.p.camp = Some(30.0); // CAMP_SECONDS, the retail countdown
    r.p.frame(0.016, (1280, 800), &mut r.host);
    assert!(r.p.screen == Screen::Login);
    assert!(matches!(r.host.live_sky, Some(None)), "returning to login stops the world's live sky");
    assert!(r.p.chat.is_none() && r.p.interact.is_none() && r.p.hud.is_none());
    let audio = r.p.audio.as_ref().unwrap();
    assert!(audio.music_layer().is_none() && audio.now_playing().is_none());
    audio.update(30.0, [0.0; 3], 3240.0);
    let mut samples = vec![0.0; 4410 * 2];
    audio.render(&mut samples);
    assert!(samples.iter().all(|&sample| sample == 0.0), "login has no surviving world audio");
    let left: Vec<_> = r.p.gui.window_ids().into_iter().map(|w| (w, r.p.gui.view_names(w))).collect();
    assert_eq!(r.p.gui.window_ids(), vec![r.p.login_w.unwrap()], "only the login window is left: {left:?}");
}

/// `GetSexStr` (Gamecode): NONE / uni / male / female, first letter upper-cased; the Atrox are sex 1 = "Uni".
#[test]
fn sex_names_follow_the_get_sex_str_table() {
    assert_eq!([0, 1, 2, 3].map(sex_name), ["NONE", "Uni", "Male", "Female"]);
    assert_eq!(format!("{} {}", breed_name(4), sex_name(1)), "Atrox Uni");
}

/// `App::exiting()` (`ao-render`'s `viewer.rs`, winit `ApplicationHandler`) relies on dropping the frontend to run
/// `Play::drop` when Cmd+Q (or anything else that skips `show_login`/`leave_world`) terminates the app instead of
/// going through `WindowEvent::CloseRequested`. A plain scope-exit `drop` must save the character-viewer cache
/// exactly like an explicit `save_viewer_cache()` call would -- regression for the no-`exiting()` bug where that
/// drop never used to run at all, so the cache was lost on every Cmd+Q.
#[test]
fn dropping_play_without_explicit_shutdown_still_saves_the_viewer_cache() {
    let Some(mut r) = rig() else { return };
    let u = include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec").lines().filter_map(|line| {
        let mut p = line.split(' ');
        let (_, direction, hex) = (p.next()?, p.next()?, p.next()?);
        if direction != "<" { return None; }
        let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let (frame, _) = ao_net::frame::Frame::decode_with(&bytes, false).ok()??;
        match ao_net::n3::decode(&frame).ok()?.body {
            ao_net::n3::N3::Dynel(ao_net::n3::dynel::Dynel::SimpleCharFullUpdate(u)) if u.name == "Aomacvolk" => Some(*u),
            _ => None,
        }
    }).next().expect("captured own update");
    const CHAR_ID: u32 = 0x5a11ee;
    r.p.zone.char_id = CHAR_ID;
    r.p.zone.own_update = Some(Box::new(u.clone()));
    drop(r.p); // no explicit save_viewer_cache()/show_login() call: only `Drop` can save it
    let scratch = crate::play::prefs::test_dir();
    let cache = ao_formats::character::ViewerCache::load(&scratch);
    let saved = cache.0.get(&(CHAR_ID as i32)).expect("Play::drop must have saved the viewer cache entry");
    assert_eq!(saved.head_mesh() as i32, u.head_mesh.unwrap_or(saved.head_mesh() as i32), "the saved entry must reflect the live update's appearance");
}

/// ServerLostMessage (GUI 0x10028d50) chooses the timed-logout destination, not the displayed timer's expiry.
#[test]
fn server_eof_completes_camp_or_reports_unexpected_loss() {
    use super::super::logout::State;
    for destination in [State::Login, State::System, State::None] {
        let Some(mut r) = rig() else { return };
        r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 33512 });
        r.burst(&captured(include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")));
        r.enter();
        let bar = if destination != State::None {
            r.p.camp();
            assert_eq!(r.p.logout.state, State::Login);
            assert_eq!(r.p.camp, Some(0.0));
            let bar = r.p.camp_bar.expect("camp timer opened");
            if destination == State::System {
                r.p.quit_cmd(&mut r.host);
                assert_eq!(r.p.logout.state, State::System);
            }
            Some(bar)
        } else {
            None
        };
        r.p.session = Some(r.session());
        r.event(LoginEvent::Disconnected("server EOF before countdown expiry".into()));
        assert!(r.p.session.is_none(), "the dead session must be dropped");
        assert!(r.p.camp.is_none() && r.p.camp_bar.is_none());
        assert!(bar.is_none_or(|w| !r.p.gui.window_ids().contains(&w)), "the camp timer window must close");
        assert_eq!(r.p.logout.state, State::None);
        assert_eq!(r.host.quit, destination == State::System);
        if destination == State::System {
            assert!(r.p.screen == Screen::InWorld, "system logout must not reopen login");
        } else {
            assert!(r.p.screen == Screen::Login && r.login_visible());
            assert!(r.p.hud.is_none() && r.p.chat.is_none() && r.p.interact.is_none());
            assert_eq!(r.p.gui.window_ids(), vec![r.p.login_w.unwrap()]);
        }
        if destination == State::None {
            assert_eq!(r.p.opened_urls.len(), 1);
            assert!(r.p.opened_urls[0].ends_with("/3.html"));
        } else {
            assert!(r.p.opened_urls.is_empty(), "intentional camp EOF is not a connection-loss error");
        }
    }
}

#[test]
fn retained_limbo_saves_user_populates_native_view_and_returns_on_timeout() {
    let Some(mut r) = rig() else { return };
    r.p.pending_user = "aomac-limbo-synthetic".into();
    let info = CharacterInfo {
        id: 77, name: "Synthetic".into(), level: 33, breed: 1, gender: 2,
        profession: 6, area: "Synthetic area".into(), ..Default::default()
    };
    r.event(LoginEvent::LoginLimbo { timeout: -1, info });
    assert!(matches!(r.p.screen, Screen::Limbo { timeout: 10.0, character_id: 77 }));
    assert!(r.p.pending_user.is_empty());
    assert!(r.p.prefs.accounts.iter().any(|name| name == "aomac-limbo-synthetic"));
    let w = r.p.progress_w.expect("native LimboWindow XML");
    assert_eq!(r.p.gui.text(w, "name"), "Synthetic");
    assert_eq!(r.p.gui.text(w, "level"), "33");
    assert_eq!(r.p.gui.text(w, "location"), "", "retail XML comments out the optional location view");
    r.p.frame(11.0, (1280, 800), &mut r.host);
    assert!(r.p.screen == Screen::Login && r.login_visible());
    assert!(r.p.progress_w.is_none());
}

#[test]
fn protocol_not_ok_closes_login_progress_and_displays_literal_error() {
    let Some(mut r) = rig() else { return };
    r.p.show_progress(CONNECT_TIMEOUT, false, &mut r.host);
    r.event(LoginEvent::ProtocolNotOk);
    assert!(r.p.screen == Screen::Login && r.login_visible());
    assert!(r.p.progress_w.is_none() && r.p.session.is_none());
    assert!(r.p.dialog_w.is_some(), "fatal text must not disappear into a status line");
    let (w, kind) = r.p.dialog_w.unwrap();
    assert!(kind == DialogKind::Fatal);
    assert_eq!(r.p.gui.text(w, "message"), "Connection error:\nProtocol Not OK.");
    assert!(!r.host.quit);
    r.click(w, "cancel_btn");
    assert!(r.host.quit && r.p.dialog_w.is_none());
}
