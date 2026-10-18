//! Explicit real-client/GPU evidence; never run by the ordinary test suite.
//! Upstream diagnostic reference: ao-vk 47fceedf61d375947be20b78d97238cb9c971ca9,
//! rvk/profile.cpp: completed timestamp intervals, separately named CPU stages, 600-frame averages.
use super::*;
use std::io::Write;
use std::path::Path;
use ao_render::KeyCode;

const SIZE: (u32, u32) = (1280, 800);

fn count(name: &str, default: usize) -> usize {
    let value = std::env::var(name).map(|v| v.parse::<usize>().expect("infrastructure frame count must be an integer")).unwrap_or(default);
    assert!(value > 0, "{name} must be positive");
    value
}

fn persistent(p: &Play) -> Vec<String> {
    let values = &p.hud.as_ref().expect("captured HUD").dvalues;
    (0..=4).map(|category| values.save_config(category)).collect()
}

fn hotkey(p: &mut Play, host: &mut Host, code: KeyCode) {
    let saved = persistent(p);
    let features = p.hud.as_ref().unwrap().dvalues.enhanced_settings();
    p.gui.clear_focus();
    let mods = host.mods;
    host.mods = ao_gui::Modifiers { ctrl: true, shift: true, ..Default::default() };
    p.game_input(ao_render::GameInput::Key { code, pressed: true, repeat: false }, host);
    p.game_input(ao_render::GameInput::Key { code, pressed: false, repeat: false }, host);
    host.mods = mods;
    assert_eq!(persistent(p), saved, "session hotkeys must not alter any persistent category");
    assert_eq!(p.hud.as_ref().unwrap().dvalues.enhanced_settings(), features, "master must retain every chosen feature");
}

fn capture(o: &ao_render::Offscreen, out: &Path, name: &str) {
    // Read the actual completed target: .png() rerenders and would not preserve the exact hotkey frame.
    let pixels = o.frozen_rgba().expect("completed target readback");
    image::save_buffer(out.join(format!("{name}.png")), &pixels, SIZE.0, SIZE.1, image::ColorType::Rgba8).unwrap();
}

fn mean(values: &[f64]) -> f64 { values.iter().sum::<f64>() / values.len() as f64 }
fn percentile(values: &[f64], fraction: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() as f64 * fraction).ceil().max(1.0) as usize - 1]
}

#[test]
fn infrastructure_percentiles_use_nearest_rank() {
    assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.5), 2.0);
    assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.95), 4.0);
}

/// Run alone under the GPU timing lock, with an absolute AOMAC_INFRA_OUT directory.
/// Defaults: 120 warmup + 600 measured frames per ABBA leg. For an initial short GPU-lock
/// chunk use AOMAC_INFRA_WARMUP=60 AOMAC_INFRA_FRAMES=120; counts are recorded, not hidden.
#[test]
#[ignore = "installed client + GPU; captured ICC infrastructure evidence and PNG readbacks"]
fn enhanced_infrastructure_evidence() {
    let out = std::path::PathBuf::from(std::env::var_os("AOMAC_INFRA_OUT").expect("set AOMAC_INFRA_OUT to an absolute output directory"));
    assert!(out.is_absolute(), "AOMAC_INFRA_OUT must be absolute");
    std::fs::create_dir_all(&out).unwrap();
    let warmup = count("AOMAC_INFRA_WARMUP", 120);
    let measured = count("AOMAC_INFRA_FRAMES", 600);
    let mut report = std::fs::File::create(out.join("report.txt")).unwrap();
    writeln!(report, "Captured zone_ithaca.rec, Testy 25988; headless real Play/HUD/world/GUI; {}x{}; warmup={warmup} measured={measured}; dt=0 after loading; no environment mutation.\nSources: ao-vk 47fceedf61d375947be20b78d97238cb9c971ca9 rvk/profile.cpp; local flow/tests.rs captured replay.\nPNG files are exact completed-target readbacks, not reconstructed or rerendered screenshots. No real-window claim.", SIZE.0, SIZE.1).unwrap();
    let mut r = rig().expect("infrastructure evidence requires installed client data");
    let mut o = ao_render::Offscreen::evidence_timing(&r.p, SIZE).unwrap();
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 25988 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_ithaca.rec")));
    o.host = std::mem::replace(&mut r.host, Host::headless());
    o.host.msaa_samples = Some(1);
    let load = Instant::now();
    loop {
        o.frame(&mut r.p, 1.0 / 60.0);
        if r.p.screen == Screen::InWorld && r.p.zone.world.cpu_benchmark_scene().0 { break; }
        assert!(load.elapsed() < Duration::from_secs(300), "captured ICC models did not load");
        std::thread::sleep(Duration::from_millis(16));
    }
    // Complete the same native countdown/CharInPlay echo as Rig::enter, but route every
    // frontend frame and the echo through the renderer-owned Host so actor batches are drained.
    while !r.p.zone.in_play_sent {
        assert!(load.elapsed() < Duration::from_secs(300), "captured ICC never sent CharInPlay");
        o.frame(&mut r.p, 1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(16));
    }
    let id = r.p.zone.char_id;
    r.p.fake_events.push_back(LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(
        0, id, ao_net::n3::outgoing::char_in_play(id as i32),
    )));
    o.frame(&mut r.p, 0.0);
    o.frame(&mut r.p, FADE_OUT);
    assert!(r.p.game_input_open() && !r.p.loading_active, "native CharInPlay echo must release loading and physical input before hotkeys");
    let own = r.p.zone.own().expect("captured Testy");
    let at = Vec3::new(own.pos[0], own.pos[1] + 1.6, -own.pos[2]);
    let camera = Camera::look_at(at + Vec3::new(0.0, 3.0, 12.0), at);
    o.fixed_camera(Some(camera));
    writeln!(report, "camera={camera:?} dynels={} posed_triangles={}", r.p.zone.dynels.len(), r.p.zone.world.cpu_benchmark_scene().1).unwrap();
    {
        let values = &mut r.p.hud.as_mut().unwrap().dvalues;
        for name in ["AomacSunShadows", "AomacAmbientOcclusion", "AomacEmissiveBloom", "AomacHdr"] {
            values.set(name, super::super::super::dvalue::Variant::Bool(true));
        }
        for name in ["AomacEnhancedSuspended", "AomacEffectGpuOverlay", "AomacSlowFrameLog"] {
            assert!(!values.persists(name), "session controls must not be persisted");
            values.set(name, super::super::super::dvalue::Variant::Bool(false));
        }
    }
    let selected = r.p.hud.as_ref().unwrap().dvalues.enhanced_settings();
    assert!(selected.any_enabled());
    // Prepare the actual selected combination once, outside every measurement and frame scope.
    o.set_enhanced_settings(selected);
    o.wait_for_pipeline_warmup();
    for _ in 0..warmup { o.frame(&mut r.p, 0.0); }
    capture(&o, &out, "00_enhanced_on");
    let frame_compiles = o.frame_thread_compiles();
    let world_compiles = o.world_pipeline_compiles();
    let pipeline_waits = o.frame_pipeline_waits();
    let mut immediate_master_ms = Vec::new();
    for (name, suspended) in [("01_master_off", true), ("02_master_on", false), ("03_master_off", true), ("04_master_on", false)] {
        hotkey(&mut r.p, &mut o.host, KeyCode::KeyE);
        assert_eq!(o.host.enhanced_suspended, suspended, "physical Ctrl+Shift+E must toggle master");
        for frame in 0..3 {
            let start = Instant::now();
            o.frame(&mut r.p, 0.0);
            let frame_ms = start.elapsed().as_secs_f64() * 1000.0;
            immediate_master_ms.push(frame_ms);
            writeln!(report, "immediate_master_frame phase={name} frame={frame} wall_ms={frame_ms:.9} excludes_capture_readback=true").unwrap();
            capture(&o, &out, &format!("{name}_f{frame:02}"));
            assert_eq!(o.enhanced_settings(), if suspended { Default::default() } else { selected });
            assert_eq!(o.frame_thread_compiles(), frame_compiles, "warm master directions must compile no frame-thread pipeline");
            assert_eq!(o.world_pipeline_compiles(), world_compiles, "warm master directions must compile nothing");
            assert_eq!(o.frame_pipeline_waits(), pipeline_waits, "warm master directions must never await a pipeline");
        }
    }
    writeln!(report, "Physical Ctrl+Shift+E off/on/off/on: persistent category XML unchanged, feature selection retained; warm frame-thread compiles=0 world compiles=0 pipeline waits=0. Four three-frame strips preserved as PNG sources.").unwrap();
    writeln!(report, "immediate_master_frames count={} max_wall_ms={:.9} p95_wall_ms={:.9}; no threshold substituted for actual no-hitch timings", immediate_master_ms.len(), immediate_master_ms.iter().copied().fold(0.0, f64::max), percentile(&immediate_master_ms, 0.95)).unwrap();
    // Expire the transient master notification once, before the frozen ABBA comparisons.
    o.frame(&mut r.p, 3.0);
    let mut csv = std::fs::File::create(out.join("frames.csv")).unwrap();
    writeln!(csv, "leg,overlay,frame,wall_ms,frontend_ms,gui_cpu_wall_ms,gpu_wait_ms,world_gpu_ms").unwrap();
    let mut wall = [Vec::new(), Vec::new()];
    let mut gui = [Vec::new(), Vec::new()];
    let mut world_gpu = [Vec::new(), Vec::new()];
    let mut gui_gpu = [Vec::new(), Vec::new()];
    let mut active_frame_gpu = [Vec::new(), Vec::new()];
    let mut timer_wall = [Vec::new(), Vec::new()];
    let mut timer_gui = [Vec::new(), Vec::new()];
    for (leg, overlay) in [false, true, true, false].into_iter().enumerate() {
        if o.host.effect_gpu_overlay != overlay { hotkey(&mut r.p, &mut o.host, KeyCode::KeyO); }
        assert_eq!(o.host.effect_gpu_overlay, overlay, "physical Ctrl+Shift+O must toggle overlay");
        // Keep profiling active for the hidden-overlay baseline too; this compares the actual GUI
        // overlay cost, not profiler-enabled vs profiler-disabled instrumentation overhead.
        r.p.hud.as_mut().unwrap().dvalues.set("AomacSlowFrameLog", super::super::super::dvalue::Variant::Bool(true));
        o.set_effect_gpu_profiling_for_evidence(false).unwrap();
        o.set_effect_gpu_profiling_for_evidence(true).unwrap();
        for _ in 0..warmup { o.frame(&mut r.p, 0.0); }
        for frame in 0..measured {
            let start = Instant::now();
            o.frame(&mut r.p, 0.0);
            let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
            let stages = o.diagnostic_frame_wall_ms();
            let index = usize::from(overlay);
            wall[index].push(wall_ms);
            gui[index].push(stages[5]);
            writeln!(csv, "{leg},{overlay},{frame},{wall_ms:.9},{:.9},{:.9},{:.9},", stages[2], stages[5], stages[6]).unwrap();
        }
        o.poll_effect_gpu_costs();
        let gui_cost = o.effect_gpu_costs().iter().find(|cost| cost.label == "GUI").expect("actual GUI pass must produce GPU timestamp results");
        assert!(gui_cost.samples > 0 && gui_cost.average_ms.is_finite());
        gui_gpu[usize::from(overlay)].push(gui_cost.average_ms);
        active_frame_gpu[usize::from(overlay)].push(o.effect_gpu_costs().iter().map(|cost| cost.average_ms).sum::<f64>());
        for cost in o.effect_gpu_costs() {
            writeln!(report, "leg={leg} overlay={overlay} completed_rolling_gpu label={} average_ms={:.9} samples={} (backend rolling window; not independent per-frame samples)", cost.label, cost.average_ms, cost.samples).unwrap();
        }
        assert!(o.host.effect_gpu_error.is_none(), "real GPU profiler failed: {:?}", o.host.effect_gpu_error);
        capture(&o, &out, &format!("abba_{leg}_overlay_{overlay}"));
        assert_eq!(o.frame_thread_compiles(), frame_compiles, "overlay measurement must not compile frame-thread pipelines");
        report.flush().unwrap();
    }
    for (name, values) in [("wall", &wall), ("gui_cpu_wall", &gui)] {
        for (index, samples) in values.iter().enumerate() {
            writeln!(report, "{name} overlay={} mean_ms={:.9} p50_ms={:.9} p95_ms={:.9} samples={}", index == 1, mean(samples), percentile(samples, 0.5), percentile(samples, 0.95), samples.len()).unwrap();
        }
        writeln!(report, "{name} ABBA_overlay_minus_hidden_ms={:.9}", mean(&values[1]) - mean(&values[0])).unwrap();
    }
    // Actual pass descriptor timestamps cannot be owned by both profilers. Keep the real overlay
    // visible while disabling only the per-effect collector for a separate ABBA GPU timer sweep.
    o.set_effect_gpu_profiling_for_evidence(false).unwrap();
    r.p.hud.as_mut().unwrap().dvalues.set("AomacSlowFrameLog", super::super::super::dvalue::Variant::Bool(false));
    o.set_gpu_timing(true).expect("GPU frame timestamp support is required, no wall-time fallback");
    for (leg, overlay) in [false, true, true, false].into_iter().enumerate() {
        if o.host.effect_gpu_overlay != overlay { hotkey(&mut r.p, &mut o.host, KeyCode::KeyO); }
        for frame in 0..warmup + measured {
            let start = Instant::now();
            o.frame(&mut r.p, 0.0);
            let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
            let gpu_ms = o.read_frame_gpu_ms().unwrap().expect("enabled GPU timer must return an actual sample");
            assert!(gpu_ms.is_finite() && gpu_ms >= 0.0);
            if frame >= warmup {
                let stages = o.diagnostic_frame_wall_ms();
                world_gpu[usize::from(overlay)].push(gpu_ms);
                timer_wall[usize::from(overlay)].push(wall_ms);
                timer_gui[usize::from(overlay)].push(stages[5]);
                writeln!(csv, "gpu_{leg},{overlay},{},{wall_ms:.9},{:.9},{:.9},{:.9},{gpu_ms:.9}", frame - warmup, stages[2], stages[5], stages[6]).unwrap();
            }
        }
        report.flush().unwrap();
    }
    // The exclusive world timer is still enabled here. Master on/off/off/on is a separate
    // experiment from overlay visibility; keep the scene, selected feature set and GUI fixed.
    let mut master_csv = std::fs::File::create(out.join("master_gpu.csv")).unwrap();
    writeln!(master_csv, "leg,enhanced_enabled,frame,wall_ms,world_gpu_ms").unwrap();
    let mut master_gpu = [Vec::new(), Vec::new()];
    for (leg, enabled) in [true, false, false, true].into_iter().enumerate() {
        if o.host.enhanced_suspended == enabled { hotkey(&mut r.p, &mut o.host, KeyCode::KeyE); }
        r.p.enhanced_notice = None;
        for frame in 0..warmup + measured {
            let start = Instant::now();
            o.frame(&mut r.p, 0.0);
            let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
            let gpu_ms = o.read_frame_gpu_ms().unwrap().expect("master sweep requires actual GPU timestamps");
            assert_eq!(o.enhanced_settings(), if enabled { selected } else { Default::default() });
            assert_eq!(o.frame_thread_compiles(), frame_compiles, "master GPU sweep frame-thread compile");
            assert_eq!(o.world_pipeline_compiles(), world_compiles, "master GPU sweep must reuse both warmed directions");
            assert_eq!(o.frame_pipeline_waits(), pipeline_waits, "master GPU sweep must never await pipelines");
            if frame >= warmup {
                master_gpu[usize::from(enabled)].push(gpu_ms);
                writeln!(master_csv, "{leg},{enabled},{},{wall_ms:.9},{gpu_ms:.9}", frame - warmup).unwrap();
            }
        }
        report.flush().unwrap();
    }
    writeln!(report, "MASTER_ABBA world_GPU_on_mean_ms={:.9} world_GPU_off_mean_ms={:.9} on_minus_off_ms={:.9}; actual world-only timestamp readbacks, fixed chosen settings, no overlay or master notification, zero warm pipeline compiles/waits", mean(&master_gpu[1]), mean(&master_gpu[0]), mean(&master_gpu[1]) - mean(&master_gpu[0])).unwrap();
    o.set_gpu_timing(false).unwrap();
    // Keep the actual overlay (and therefore the same GPU profiler) active for both logger states.
    // Thread CPU and completed-frame wall are separately observed; neither is renamed GPU time.
    hotkey(&mut r.p, &mut o.host, KeyCode::KeyO);
    assert!(o.host.effect_gpu_overlay);
    o.set_effect_gpu_profiling_for_evidence(true).unwrap();
    let mut logger_csv = std::fs::File::create(out.join("slow_logger.csv")).unwrap();
    writeln!(logger_csv, "leg,logging,frame,wall_ms,thread_cpu_ms,frontend_wall_ms,gui_encoding_wall_ms,gpu_wait_ms").unwrap();
    let mut logger_wall = [Vec::new(), Vec::new()];
    let mut logger_cpu = [Vec::new(), Vec::new()];
    for (leg, logging) in [false, true, true, false].into_iter().enumerate() {
        r.p.hud.as_mut().unwrap().dvalues.set("AomacSlowFrameLog", super::super::super::dvalue::Variant::Bool(logging));
        for frame in 0..warmup + measured {
            let start = Instant::now();
            let cpu_start = input_cpu_now();
            o.frame(&mut r.p, 0.0);
            let cpu_ms = (input_cpu_now() - cpu_start).as_secs_f64() * 1000.0;
            let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(o.frame_thread_compiles(), frame_compiles, "logger sweep frame-thread compile");
            assert_eq!(o.world_pipeline_compiles(), world_compiles, "logger sweep must not compile pipelines");
            assert_eq!(o.frame_pipeline_waits(), pipeline_waits, "logger sweep must not await pipelines");
            if frame >= warmup {
                let stages = o.diagnostic_frame_wall_ms();
                logger_wall[usize::from(logging)].push(wall_ms);
                logger_cpu[usize::from(logging)].push(cpu_ms);
                writeln!(logger_csv, "{leg},{logging},{},{wall_ms:.9},{cpu_ms:.9},{:.9},{:.9},{:.9}", frame - warmup, stages[2], stages[5], stages[6]).unwrap();
            }
        }
        report.flush().unwrap();
    }
    writeln!(report, "SLOW_LOG_ABBA incremental_logger_wall_ms={:.9} incremental_logger_thread_CPU_ms={:.9} logging_off_wall_ms={:.9} logging_on_wall_ms={:.9} logging_off_thread_CPU_ms={:.9} logging_on_thread_CPU_ms={:.9}; overlay and shared GPU profiler active throughout; measures enabled logger/stage bookkeeping and any naturally triggered output, not fabricated forced slow frames", mean(&logger_wall[1]) - mean(&logger_wall[0]), mean(&logger_cpu[1]) - mean(&logger_cpu[0]), mean(&logger_wall[0]), mean(&logger_wall[1]), mean(&logger_cpu[0]), mean(&logger_cpu[1])).unwrap();
    // Intrinsic GUI attribution: drain all preceding world work, then submit only the real frozen
    // Play draw list. Ordinary pass timestamps can overlap/depend on world work on tiled GPUs.
    let mut isolated_csv = std::fs::File::create(out.join("isolated_gui.csv")).unwrap();
    writeln!(isolated_csv, "leg,overlay,frame,gui_encoding_cpu_side_wall_ms").unwrap();
    let mut isolated_gpu = [Vec::new(), Vec::new()];
    let mut isolated_encoding = [Vec::new(), Vec::new()];
    r.p.hud.as_mut().unwrap().dvalues.set("AomacSlowFrameLog", super::super::super::dvalue::Variant::Bool(true));
    for (leg, overlay) in [false, true, true, false].into_iter().enumerate() {
        if o.host.effect_gpu_overlay != overlay { hotkey(&mut r.p, &mut o.host, KeyCode::KeyO); }
        // Refill the actual world+GUI profiler before freezing geometry, so visible lists contain
        // populated world-effect rows rather than waiting/GUI-only rows from the preceding leg.
        let mut list = o.frame(&mut r.p, 0.0);
        for _ in 0..warmup { list = o.frame(&mut r.p, 0.0); }
        o.set_effect_gpu_profiling_for_evidence(false).unwrap();
        o.set_effect_gpu_profiling_for_evidence(true).unwrap();
        for frame in 0..warmup + measured {
            let cpu_ms = o.render_gui_only_for_evidence(&r.p, &list).expect("isolated actual GUI render");
            assert!(cpu_ms.is_finite() && cpu_ms >= 0.0);
            if frame >= warmup {
                isolated_encoding[usize::from(overlay)].push(cpu_ms);
                writeln!(isolated_csv, "{leg},{overlay},{},{cpu_ms:.9}", frame - warmup).unwrap();
            }
        }
        o.poll_effect_gpu_costs();
        let cost = o.effect_gpu_costs().iter().find(|cost| cost.label == "GUI").expect("isolated actual GUI timestamp results");
        assert!(cost.samples > 0 && cost.average_ms.is_finite());
        isolated_gpu[usize::from(overlay)].push(cost.average_ms);
        writeln!(report, "ISOLATED_GUI leg={leg} overlay={overlay} TOTAL_GPU_ms={:.9} completed_window_samples={} TOTAL_encoding_cpu_side_wall_ms={:.9}; prior world drained, GUI-only submissions, frozen actual Play list, reset profiler per leg; rolling GPU window may include warmup", cost.average_ms, cost.samples, mean(&isolated_encoding[usize::from(overlay)])).unwrap();
        assert_eq!(o.frame_thread_compiles(), frame_compiles, "isolated GUI must compile no pipelines");
        report.flush().unwrap();
    }
    writeln!(report, "ISOLATED_GUI_ABBA GPU_overlay_minus_hidden_ms={:.9} encoding_overlay_minus_hidden_ms={:.9}", mean(&isolated_gpu[1]) - mean(&isolated_gpu[0]), mean(&isolated_encoding[1]) - mean(&isolated_encoding[0])).unwrap();
    hotkey(&mut r.p, &mut o.host, KeyCode::KeyO);
    assert!(o.host.effect_gpu_overlay, "restore overlay-visible controlled logger acceptance state");
    // Explicit threshold acceptance, deliberately outside every distribution and CSV sweep.
    // The real Play still builds the GUI frame; only this wrapper's frontend stage is delayed.
    struct ControlledSlowFrontend<'a>(&'a mut Play);
    impl ao_render::Frontend for ControlledSlowFrontend<'_> {
        fn gui(&self) -> &ao_gui::Gui { self.0.gui() }
        fn input(&mut self, event: InputEvent, host: &mut Host) { self.0.input(event, host); }
        fn game_input(&mut self, event: ao_render::GameInput, host: &mut Host) { self.0.game_input(event, host); }
        fn mover_sound(&mut self, event: ao_scene::mover::SoundEvent, host: &Host) { self.0.mover_sound(event, host); }
        fn interface_scale(&self) -> u8 { self.0.interface_scale() }
        fn frame(&mut self, dt: f32, size: (u32, u32), host: &mut Host) -> ao_gui::DrawList {
            let list = self.0.frame(dt, size, host);
            std::thread::sleep(Duration::from_millis(55));
            list
        }
    }
    r.p.hud.as_mut().unwrap().dvalues.set("AomacSlowFrameLog", super::super::super::dvalue::Variant::Bool(true));
    eprintln!("INFRA_CONTROLLED_SLOW_FRAME deliberately injecting 55ms frontend delay; excluded from all performance metrics");
    o.frame(&mut ControlledSlowFrontend(&mut r.p), 0.0);
    let controlled = o.diagnostic_frame_wall_ms();
    assert!(controlled[2] >= 50.0, "controlled frontend delay must cross the 50ms log threshold: {controlled:?}");
    assert!(controlled[0] >= 50.0, "controlled completed frame must cross the 50ms log threshold: {controlled:?}");
    writeln!(report, "CONTROLLED slow-log threshold acceptance: deliberately injected frontend sleep=55ms; actual frontend_wall_ms={:.9} actual_total_wall_ms={:.9}; session slow logging enabled, actual timestamped stage line emitted by production logger to stderr. This is NOT a naturally observed hitch and is excluded from every distribution/CSV sweep.", controlled[2], controlled[0]).unwrap();
    r.p.hud.as_mut().unwrap().dvalues.set("AomacSlowFrameLog", super::super::super::dvalue::Variant::Bool(false));
    hotkey(&mut r.p, &mut o.host, KeyCode::KeyO);
    // Navigate actual registered option controls through the existing headless GUI-input planner.
    // Asking for each current value selects its real category and scrolls its actual bound widget
    // into view, but returns no click events and therefore cannot alter the setting for the capture.
    r.p.hud.as_mut().unwrap().dvalues.set("optionpanel_window", super::super::super::dvalue::Variant::Bool(true));
    for name in ["AomacEnhancedSuspended", "AomacEffectGpuOverlay", "AomacSlowFrameLog"] {
        let hud = r.p.hud.as_mut().unwrap();
        let value = hud.live_option_value(name);
        let inputs = hud.live_option_inputs(&mut r.p.gui, name, value);
        assert!(inputs.is_empty(), "unchanged session check box requires no synthetic value-changing input");
        o.frame(&mut r.p, 0.0);
        assert_eq!(r.p.hud.as_ref().unwrap().live_option_value(name), value);
        capture(&o, &out, &format!("performance_{name}"));
        writeln!(report, "Performance page actual bound widget verified and captured: {name} value={value}; existing options probe selected its category and scrolled control into view, no checkbox mutation.").unwrap();
    }
    for (name, values) in [("world_gpu_separate_sweep", &world_gpu), ("GUI_gpu_rolling_leg_mean", &gui_gpu), ("nonexclusive_GPU_pass_interval_sum", &active_frame_gpu), ("effect_profiler_disabled_world_timer_enabled_wall", &timer_wall), ("effect_profiler_disabled_GUI_encoding_wall", &timer_gui), ("isolated_GUI_GPU_leg_mean", &isolated_gpu), ("isolated_GUI_encoding_wall", &isolated_encoding)] {
        for (index, samples) in values.iter().enumerate() {
            writeln!(report, "{name} overlay={} mean_ms={:.9} p50_ms={:.9} p95_ms={:.9} samples={}", index == 1, mean(samples), percentile(samples, 0.5), percentile(samples, 0.95), samples.len()).unwrap();
        }
        writeln!(report, "{name} ABBA_overlay_minus_hidden_ms={:.9}", mean(&values[1]) - mean(&values[0])).unwrap();
    }
    writeln!(report, "Instrumentation boundary: first ABBA keeps actual per-effect profiling enabled for BOTH hidden and visible overlay, isolating overlay draw cost rather than profiler query/readback overhead. Second ABBA disables per-effect profiling but enables the world timer; its wall totals are separately reported, not an isolated profiler-overhead estimate. GUI encoding wall is the GuiRenderer::draw CPU-side elapsed interval, separate from frontend/layout; it is not thread CPU or GPU duration.").unwrap();
    for overlay in [false, true] {
        let index = usize::from(overlay);
        let gpu_ms = mean(&isolated_gpu[index]);
        let encoding_ms = mean(&isolated_encoding[index]);
        let frame_wall_ms = mean(&wall[index]);
        writeln!(report, "GUI layer redraw-cap decision overlay={overlay}: ISOLATED_TOTAL_GUI_GPU_ms={gpu_ms:.9} ISOLATED_TOTAL_GUI_encoding_CPU_side_wall_ms={encoding_ms:.9} ordinary_completed_frame_wall_ms={frame_wall_ms:.9} isolated_GPU_fraction_of_ordinary_wall={:.9} isolated_encoding_fraction_of_ordinary_wall={:.9}; {}. Decision uses isolated GUI-only totals, not potentially overlapping ordinary GPU intervals. CPU and GPU costs are independent/concurrent and are NOT summed. Decision threshold is 0.2ms for either total GUI measure, not an upstream constant; headless cannot establish a display refresh cadence.",
            gpu_ms / frame_wall_ms, encoding_ms / frame_wall_ms,
            if gpu_ms < 0.2 && encoding_ms < 0.2 { "GUI layer redraw cap not justified by these measured totals" } else { "measured total GUI layer cost warrants evaluating a GUI redraw cap; cadence requires display/input-latency evidence" }).unwrap();
    }
    writeln!(report, "Timer definitions: FrameGpuTimer and per-effect pass timestamps are mutually exclusive. FrameGpuTimer spans world GPU work, excluding GUI and presentation. nonexclusive_GPU_pass_interval_sum adds actual world/GUI interval means which may overlap or include cross-pass dependency waits; it is NOT exclusive active-frame cost or continuous full-frame latency. Ordinary GUI intervals may include world dependencies; cap decisions instead use the separate drained GUI-only experiment. Different labels may have different completed-window sample counts, preserved above. Performance page PNGs preserve actual options category/widget navigation for all three session controls.").unwrap();
    report.flush().unwrap();
    eprintln!("INFRA_EVIDENCE report={} frames={} warmup={warmup} measured={measured}", out.join("report.txt").display(), out.join("frames.csv").display());
}
