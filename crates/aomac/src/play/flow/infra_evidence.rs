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

/// Run alone under the GPU timing lock; output must be an absolute directory.
#[test]
#[ignore = "installed client + GPU; real captured ICC character smoothing ABBA evidence"]
fn character_smoothing_crowd_evidence() {
    const CROWD_SIZE: (u32, u32) = (1920, 1200);
    use super::super::super::dvalue::Variant;
    let out = std::path::PathBuf::from(std::env::var_os("AOMAC_CHAR_SMOOTH_OUT")
        .expect("set AOMAC_CHAR_SMOOTH_OUT to an absolute output directory"));
    assert!(out.is_absolute(), "AOMAC_CHAR_SMOOTH_OUT must be absolute");
    std::fs::create_dir_all(&out).unwrap();
    let warmup = count("AOMAC_CHAR_SMOOTH_WARMUP", 60);
    let measured = count("AOMAC_CHAR_SMOOTH_FRAMES", 120);
    let started = Instant::now();
    let within_lock = || assert!(started.elapsed() < Duration::from_secs(270),
        "character evidence exceeded its 270s GPU-lock execution allowance");
    let mut report = std::fs::File::create(out.join("report.txt")).unwrap();
    writeln!(report, "zone_ithaca.rec Testy 25988; real Play/HUD/registry; no copied or synthetic actors; {}x{}; requested MSAA=4; warmup={warmup} measured={measured}; frozen camera/time ABBA off/on/on/off. Headless only; no real-window claim.", CROWD_SIZE.0, CROWD_SIZE.1).unwrap();
    let mut r = rig().expect("character smoothing evidence requires installed client data");
    let mut o = ao_render::Offscreen::evidence_timing(&r.p, CROWD_SIZE).unwrap();
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 25988 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_ithaca.rec")));
    o.host = std::mem::replace(&mut r.host, Host::headless());
    o.host.msaa_samples = Some(4);
    loop {
        within_lock();
        o.frame(&mut r.p, 1.0 / 60.0);
        if r.p.screen == Screen::InWorld && r.p.zone.world.cpu_benchmark_scene().0 && r.p.zone.in_play_sent { break; }
        std::thread::sleep(Duration::from_millis(16));
    }
    let id = r.p.zone.char_id;
    r.p.fake_events.push_back(LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(
        0, id, ao_net::n3::outgoing::char_in_play(id as i32),
    )));
    o.frame(&mut r.p, 0.0);
    o.frame(&mut r.p, FADE_OUT);
    assert!(r.p.game_input_open() && !r.p.loading_active);
    {
        let values = &mut r.p.hud.as_mut().unwrap().dvalues;
        for control in super::super::super::enhanced::controls().into_iter()
            .filter(|control| control.name.starts_with("Aomac")) {
            values.set(control.name, control.default);
        }
        assert!(!values.enhanced_settings().any_enabled(), "registry defaults must be all-off");
        assert!(values.persists("AomacCharacterSmoothing"), "smoothing choice must persist");
        values.set_i64("AomacEdgeSmoothing", 2); // Play maps this registry index to host MSAA=4.
        values.set_i64("BoundingBoxTargeting", 0);
    }
    let own = r.p.zone.own().expect("captured Testy");
    let at = Vec3::new(own.pos[0], own.pos[1] + 1.6, -own.pos[2]);
    let camera = Camera::look_at(at + Vec3::new(0.0, 3.0, 12.0), at);
    o.fixed_camera(Some(camera));
    o.frame(&mut r.p, 0.0);
    o.wait_for_pipeline_warmup();
    o.frame(&mut r.p, 0.0);
    assert_eq!(o.msaa_samples(), 4, "actual renderer MSAA must be 4, not a quality fallback");
    assert!(r.p.zone.triangle_targeting);
    let mut bodies = r.p.zone.world.pick_bodies(id as i32);
    bodies.sort_by_key(|body| body.id);
    let remote = bodies.iter().filter(|body| body.id != id as i32).count();
    let near = bodies.iter().filter(|body| body.id != id as i32
        && Vec3::from(body.pos).distance_squared(camera.pos) <= 225.0).count();
    let (_, triangles) = r.p.zone.world.cpu_benchmark_scene();
    assert!(remote > 0 && near > 0 && triangles > 0, "requires real nearby posed CAT crowd");
    writeln!(report, "actual_msaa={} camera={camera:?} dynels={} real_remote_characters={} visible_pick_bodies={remote} near_pick_bodies_15m={near} original_remote_body_triangles={triangles}; near count uses real visible body origins, not a claim that every CAT is eligible for smoothing.", o.msaa_samples(), r.p.zone.dynels.len(), r.p.zone.world.chars.len()).unwrap();
    let rays: Vec<_> = bodies.iter().filter(|body| body.id != id as i32).map(|body| {
        let local = (Vec3::from(body.bounds.0) + Vec3::from(body.bounds.1)) * (0.5 * body.scale);
        let centre = Vec3::from(body.pos) + glam::Quat::from_rotation_y(body.yaw) * local;
        super::super::super::hud_target::Ray { origin: camera.pos,
            dir: (centre - camera.pos).normalize(), len: 799.5 }
    }).collect();
    let pick_snapshot = |p: &Play| {
        rays.iter().map(|ray| (
            bodies.iter().map(|body| p.zone.world.pick_character_hit(body, ray, false).map(f32::to_bits)).collect::<Vec<_>>(),
            super::super::super::interact_use::pick_objects(ray, &p.zone, p.player.as_ref()),
        )).collect::<Vec<_>>()
    };
    let original_picks = pick_snapshot(&r.p);
    assert!(original_picks.iter().any(|(hits, _)| hits.iter().any(Option::is_some)),
        "the CPU invariant must exercise real native triangle hits");
    let mut csv = std::fs::File::create(out.join("frames.csv")).unwrap();
    writeln!(csv, "leg,smoothing,frame,render_submit_cpu_wall_ms,world_gpu_ms").unwrap();
    let mut gpu = [Vec::new(), Vec::new()];
    let mut cpu = [Vec::new(), Vec::new()];
    let mut before = None;
    for (leg, enabled) in [false, true, true, false].into_iter().enumerate() {
        within_lock();
        r.p.hud.as_mut().unwrap().dvalues.set("AomacCharacterSmoothing", Variant::Bool(enabled));
        // Apply the real Play registry route once, then retain its actual actors and camera.
        o.frame(&mut r.p, 0.0);
        o.wait_for_pipeline_warmup();
        o.frame(&mut r.p, 0.0);
        assert_eq!(o.enhanced_settings().character_smoothing, enabled);
        assert_eq!(o.msaa_samples(), 4);
        let mut current = r.p.zone.world.pick_bodies(id as i32);
        current.sort_by_key(|body| body.id);
        assert_eq!(current, bodies, "render smoothing must retain native posed pick bounds");
        assert_eq!(pick_snapshot(&r.p), original_picks,
            "native triangle distances and CPU target selection must not change with smoothing");
        o.set_effect_gpu_profiling_for_evidence(false).unwrap();
        o.set_gpu_timing(true).expect("actual GPU timestamps required; no CPU fallback");
        for frame in 0..warmup + measured {
            within_lock();
            let start = Instant::now();
            o.render_frozen().unwrap();
            let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
            let gpu_ms = o.read_frame_gpu_ms().unwrap().expect("actual completed GPU sample");
            assert!(gpu_ms.is_finite() && gpu_ms >= 0.0);
            if frame >= warmup {
                gpu[usize::from(enabled)].push(gpu_ms);
                cpu[usize::from(enabled)].push(cpu_ms);
                writeln!(csv, "{leg},{enabled},{},{cpu_ms:.9},{gpu_ms:.9}", frame - warmup).unwrap();
            }
        }
        o.set_gpu_timing(false).unwrap();
        let pixels = o.frozen_rgba().unwrap();
        image::save_buffer(out.join(format!("abba_{leg}_smoothing_{enabled}.png")),
            &pixels, CROWD_SIZE.0, CROWD_SIZE.1, image::ColorType::Rgba8).unwrap();
        if leg == 0 { before = Some(pixels); }
        else if enabled {
            assert!(Some(&pixels) != before.as_ref(), "smoothing ON must visibly differ from the original OFF snapshot");
        }
        else if leg == 3 {
            assert!(pixels == before.take().unwrap(), "all-off repeat must have exact pixel parity");
        }
        writeln!(report, "leg={leg} smoothing={enabled} actual_msaa={} CPU original bounds/triangle hits/selection unchanged", o.msaa_samples()).unwrap();
        report.flush().unwrap();
    }
    for (name, samples) in [("world_gpu", &gpu), ("render_submit_cpu_wall", &cpu)] {
        for (enabled, values) in samples.iter().enumerate() {
            writeln!(report, "{name} smoothing={} samples={} mean_ms={:.9} p50_ms={:.9} p95_ms={:.9}",
                enabled == 1, values.len(), mean(values), percentile(values, 0.5), percentile(values, 0.95)).unwrap();
        }
        writeln!(report, "{name} ABBA_on_minus_off_ms={:.9}", mean(&samples[1]) - mean(&samples[0])).unwrap();
    }
    writeln!(report, "All-off before/after exact pixel parity PASS. Frozen GPU scope includes native skinning and smoothing compute/world passes; CPU scope is render submission only, not Play/update or GPU waiting. PNG/readback excluded from measured samples.").unwrap();
    // Three genuinely advancing Play poses, paired off/on without advancing between each pair.
    r.p.gui.clear_focus();
    r.p.game_input(ao_render::GameInput::Key { code: KeyCode::KeyW, pressed: true, repeat: false }, &mut o.host);
    let start_anchor = r.p.player.as_ref().and_then(|player| player.effect_anchor(0));
    let mut strips = [image::RgbaImage::new(CROWD_SIZE.0 * 3, CROWD_SIZE.1),
        image::RgbaImage::new(CROWD_SIZE.0 * 3, CROWD_SIZE.1)];
    for frame in 0..3 {
        within_lock();
        r.p.hud.as_mut().unwrap().dvalues.set("AomacCharacterSmoothing", Variant::Bool(false));
        o.frame(&mut r.p, 1.0 / 15.0);
        assert!(r.p.player.as_ref().unwrap().movement_active(), "motion evidence must exercise actual native walking");
        std::thread::sleep(Duration::from_millis(40));
        for enabled in [false, true] {
            let values = &mut r.p.hud.as_mut().unwrap().dvalues;
            values.set("AomacCharacterSmoothing", Variant::Bool(enabled));
            o.set_enhanced_settings(values.enhanced_settings());
            o.wait_for_pipeline_warmup();
            o.render_frozen().unwrap();
            assert_eq!(o.msaa_samples(), 4);
            let image = image::RgbaImage::from_raw(CROWD_SIZE.0, CROWD_SIZE.1, o.frozen_rgba().unwrap()).unwrap();
            image::imageops::replace(&mut strips[usize::from(enabled)], &image, i64::from(frame * CROWD_SIZE.0), 0);
        }
    }
    r.p.game_input(ao_render::GameInput::Key { code: KeyCode::KeyW, pressed: false, repeat: false }, &mut o.host);
    assert_ne!(r.p.player.as_ref().and_then(|player| player.effect_anchor(0)), start_anchor,
        "walking strips must change the actual character anchor, not merely advance sky time");
    for (enabled, strip) in strips.iter().enumerate() {
        strip.save(out.join(format!("animation_smoothing_{}.png", enabled == 1))).unwrap();
    }
    writeln!(report, "Animation strips: physical W through real Play, three walking frames advancing dt=1/15s each, fixed camera; actual own head anchor changed; each off/on pair shares one pose and time, no copied actors. elapsed_seconds={:.3}", started.elapsed().as_secs_f64()).unwrap();
    report.flush().unwrap();
    csv.flush().unwrap();
    within_lock();
}

/// Fixture-only synthetic crowd: captured appearances, native rigs and animation, never live dynels.
#[test]
#[ignore = "installed client + GPU; synthetic real-asset 60-humanoid ICC stress evidence"]
fn character_smoothing_synthetic_crowd_evidence() {
    use super::super::super::avatar::{Avatar, AvatarPose};
    use super::super::super::dvalue::Variant;
    use ao_formats::character::{actor::ActorAssets, Role};
    use std::sync::Arc;
    const RES: (u32, u32) = (1920, 1200);
    const ACTORS: usize = 60;
    const MODEL_BASE: u64 = 0x5359_4e54_0000_0000;
    const ACTOR_BASE: u32 = 0xee00_0000;
    let out = std::path::PathBuf::from(std::env::var_os("AOMAC_CHAR_SMOOTH_OUT")
        .expect("set AOMAC_CHAR_SMOOTH_OUT to an absolute output directory"));
    assert!(out.is_absolute(), "AOMAC_CHAR_SMOOTH_OUT must be absolute");
    std::fs::create_dir_all(&out).unwrap();
    let started = Instant::now();
    let within_lock = || assert!(started.elapsed() < Duration::from_secs(270),
        "synthetic crowd exceeded its 270s GPU-lock execution allowance");
    let warmup = count("AOMAC_CHAR_SMOOTH_WARMUP", 60);
    let measured = count("AOMAC_CHAR_SMOOTH_FRAMES", 120);
    assert!(warmup >= 60 && measured >= 120, "stress evidence requires 60 warmup + 120 measured per leg");
    let mut report = std::fs::File::create(out.join("report.txt")).unwrap();
    writeln!(report, "SYNTHETIC REAL-ASSET CROWD: fixture-only replicas, NOT a captured/live population. ICC world from zone_ithaca.rec, real Play/HUD registry and native Avatar/ActorRig path. 60 actors; 1920x1200; MSAA4; frozen OFF/ON/ON/OFF; warmup={warmup} measured={measured}. Headless only.").unwrap();
    let mut r = rig().expect("synthetic crowd requires installed client data");
    let mut o = ao_render::Offscreen::evidence_timing(&r.p, RES).unwrap();
    r.event(LoginEvent::ZoneHandoff { zone_ip: Ipv4Addr::LOCALHOST, zone_port: 1, character_id: 25988 });
    r.burst(&captured(include_str!("../../../../../docs/captures/zone_ithaca.rec")));
    o.host = std::mem::replace(&mut r.host, Host::headless());
    o.host.msaa_samples = Some(4);
    loop {
        within_lock();
        o.frame(&mut r.p, 1.0 / 60.0);
        if r.p.screen == Screen::InWorld && r.p.zone.world.cpu_benchmark_scene().0 && r.p.zone.in_play_sent { break; }
        std::thread::sleep(Duration::from_millis(16));
    }
    let id = r.p.zone.char_id;
    r.p.fake_events.push_back(LoginEvent::ZoneFrame(ao_net::n3::outgoing::n3_frame(
        0, id, ao_net::n3::outgoing::char_in_play(id as i32))));
    o.frame(&mut r.p, 0.0);
    o.frame(&mut r.p, FADE_OUT);
    assert!(r.p.game_input_open() && !r.p.loading_active);
    {
        let values = &mut r.p.hud.as_mut().unwrap().dvalues;
        for control in super::super::super::enhanced::controls().into_iter()
            .filter(|control| control.name.starts_with("Aomac")) {
            values.set(control.name, control.default);
        }
        values.set_i64("AomacEdgeSmoothing", 2);
    }
    // Decode the same recorded wire frames as Play; no invented head, cloth or attachment choices.
    let mut sources = Vec::new();
    for (capture, rec) in [
        ("zone_ithaca.rec", include_str!("../../../../../docs/captures/zone_ithaca.rec")),
        ("zone_newchar_ithaca.rec", include_str!("../../../../../docs/captures/zone_newchar_ithaca.rec")),
    ] {
        for frame in captured(rec) {
            let Ok(message) = ao_net::n3::decode(&frame) else { continue };
            if let ao_net::n3::N3::Dynel(ao_net::n3::dynel::Dynel::SimpleCharFullUpdate(update)) = message.body {
                if (1..=4).contains(&update.breed) {
                    sources.push((capture, message.header.target.instance, *update));
                }
            }
        }
    }
    // First cover every captured breed/sex pair, then fill with distinct captured outfits.
    let mut selected = Vec::new();
    let mut breeds_sexes = std::collections::BTreeSet::new();
    let mut appearances = std::collections::BTreeSet::new();
    for first_pass in [true, false] {
        for (capture, identity, update) in &sources {
            let signature = format!("{:?}/{:?}/{:?}/{:?}/{:?}/{:?}", update.breed, update.sex,
                update.head_mesh, update.fatness, update.cloth, update.attractors);
            if selected.len() < 8 && !appearances.contains(&signature)
                && (!first_pass || !breeds_sexes.contains(&(update.breed, update.sex))) {
                appearances.insert(signature);
                breeds_sexes.insert((update.breed, update.sex));
                selected.push((*capture, *identity, update));
            }
        }
    }
    assert!(selected.len() >= 2 && breeds_sexes.len() >= 2,
        "requires at least two distinct captured humanoid appearances and breed/sex pairs");
    let breeds: std::collections::BTreeSet<_> = breeds_sexes.iter().map(|&(breed, _)| breed).collect();
    let sexes: std::collections::BTreeSet<_> = breeds_sexes.iter().map(|&(_, sex)| sex).collect();
    assert!(breeds.len() >= 2 && sexes.contains(&2) && sexes.contains(&3),
        "capture-sourced crowd must cover multiple breeds plus male and female humanoids");
    assert!(selected.iter().any(|(_, _, update)| !update.cloth.is_empty()),
        "captured armour/cloth coverage is required");
    writeln!(report, "captured_breeds={breeds:?} captured_sexes={sexes:?}; wire sex2=male sex3=female; appearance selection first covers breed/sex pairs, then distinct head/cloth/attractor outfits (maximum8).").unwrap();
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir).unwrap();
    let own = r.p.zone.own().unwrap();
    let origin = Vec3::new(own.pos[0], own.pos[1], -own.pos[2]);
    // Ten columns by six rows, all origins inside the camera's 15m eligibility sphere.
    let camera = Camera::look_at(origin + Vec3::new(0.0, 4.5, 9.0), origin + Vec3::new(0.0, 1.0, -1.0));
    o.fixed_camera(Some(camera));
    let mut avatars = Vec::new();
    for (index, (capture, identity, update)) in selected.iter().enumerate() {
        within_lock();
        let mut avatar = Avatar::new(&store, &dir, ACTOR_BASE + index as u32, update,
            ActorAssets::new(&store).unwrap()).expect("captured humanoid must load faithfully");
        assert!(avatar.model().native_skin.is_some(), "real native CAT skin required");
        avatar.set_pose(&store, AvatarPose { role: Role::Walk, speed: 1.5, ref_speed: 1.5 }).unwrap();
        avatar.update(0.2 + index as f32 * 0.037);
        o.host.actor_models.push((MODEL_BASE + index as u64, Arc::new(avatar.model().clone())));
        writeln!(report, "source={capture} identity={identity} name={:?} breed={} sex={} head={:?} fatness={} cloth={:?} attractors={:?} model_key={:#x} native_vertices={} meshes={} replicas={}",
            update.name, update.breed, update.sex, update.head_mesh, update.fatness, update.cloth,
            update.attractors, MODEL_BASE + index as u64, avatar.model().native_skin.as_ref().unwrap().len(),
            avatar.model().meshes.len(), (0..ACTORS).filter(|ordinal| ordinal % selected.len() == index).count()).unwrap();
        avatars.push(avatar);
    }
    struct Crowd<'a> {
        play: &'a mut Play,
        avatars: &'a mut [Avatar],
        origin: Vec3,
        frames: Vec<ao_scene::ActorFrame>,
    }
    impl ao_render::Frontend for Crowd<'_> {
        fn gui(&self) -> &ao_gui::Gui { self.play.gui() }
        fn input(&mut self, event: InputEvent, host: &mut Host) { self.play.input(event, host); }
        fn frame(&mut self, dt: f32, size: (u32, u32), host: &mut Host) -> ao_gui::DrawList {
            let list = self.play.frame(dt, size, host);
            for avatar in self.avatars.iter_mut() { avatar.update(dt); }
            let poses: Vec<_> = self.avatars.iter().map(Avatar::frame).collect();
            self.frames = (0..ACTORS).map(|ordinal| {
                let source = ordinal % poses.len();
                let mut actor = poses[source].clone();
                actor.id = ACTOR_BASE + ordinal as u32;
                actor.model = MODEL_BASE + source as u64;
                // Only world placement/identity change: native scale, skin, mounts and materials survive.
                actor.transform[3] = (self.origin + Vec3::new(
                    (ordinal % 10) as f32 * 0.9 - 4.05, 0.0,
                    1.5 - (ordinal / 10) as f32 * 1.1)).extend(1.0).to_array();
                actor.always = false;
                actor
            }).collect();
            host.actors.clone_from(&self.frames);
            host.dynamic_lights.clear(); // No light from the replaced real actor batch.
            list
        }
    }
    let mut crowd = Crowd { play: &mut r.p, avatars: &mut avatars, origin, frames: Vec::new() };
    o.frame(&mut crowd, 0.0);
    o.wait_for_pipeline_warmup();
    o.frame(&mut crowd, 0.0);
    assert_eq!(o.msaa_samples(), 4);
    let visible = o.host.previous_visible_actors.iter()
        .filter(|id| (ACTOR_BASE..ACTOR_BASE + ACTORS as u32).contains(*id)).count();
    assert_eq!(visible, ACTORS, "all 60 bodies must reach the actual renderer primary draw batch");
    writeln!(report, "camera={camera:?}; origin={origin:?}; grid=10x6 spacing=(0.9,1.1)m; actual_msaa={}; actual_primary_body_draws={visible}; appearances={} breed_sex_pairs={:?}; native palettes/rigid mounted part matrices/part_attractors retained; ordinary primary visibility (always=false), not forced visibility.", o.msaa_samples(), selected.len(), breeds_sexes).unwrap();
    for actor in &crowd.frames {
        let pos = Vec3::from_slice(&actor.transform[3][..3]);
        assert!(pos.distance_squared(camera.pos) <= 225.0);
        assert!(actor.character && actor.skin.is_none() && actor.skin_pose.is_some());
        writeln!(report, "replica_id={} model_key={:#x} position={pos:?} camera_distance_m={:.6} palette_bones={} mounted_parts={} attractor_slots={}", actor.id, actor.model,
            pos.distance(camera.pos), actor.skin_pose.as_ref().unwrap().len(), actor.parts.len(), actor.part_attractors.len()).unwrap();
    }
    let mut csv = std::fs::File::create(out.join("frames.csv")).unwrap();
    writeln!(csv, "leg,smoothing,frame,render_submit_cpu_wall_ms,world_gpu_ms").unwrap();
    let mut gpu = [Vec::new(), Vec::new()];
    let mut cpu = [Vec::new(), Vec::new()];
    let mut baseline = None;
    for (leg, enabled) in [false, true, true, false].into_iter().enumerate() {
        crowd.play.hud.as_mut().unwrap().dvalues.set("AomacCharacterSmoothing", Variant::Bool(enabled));
        o.frame(&mut crowd, 0.0);
        o.wait_for_pipeline_warmup();
        o.frame(&mut crowd, 0.0);
        assert_eq!(o.msaa_samples(), 4);
        assert_eq!(o.enhanced_settings().character_smoothing, enabled);
        o.set_effect_gpu_profiling_for_evidence(false).unwrap();
        o.set_gpu_timing(true).unwrap();
        for frame in 0..warmup + measured {
            within_lock();
            let start = Instant::now();
            o.render_frozen().unwrap();
            let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
            let gpu_ms = o.read_frame_gpu_ms().unwrap().expect("completed GPU timestamp required");
            assert!(gpu_ms.is_finite() && gpu_ms >= 0.0);
            if frame >= warmup {
                gpu[usize::from(enabled)].push(gpu_ms);
                cpu[usize::from(enabled)].push(cpu_ms);
                writeln!(csv, "{leg},{enabled},{},{cpu_ms:.9},{gpu_ms:.9}", frame - warmup).unwrap();
            }
        }
        o.set_gpu_timing(false).unwrap();
        let pixels = o.frozen_rgba().unwrap();
        image::save_buffer(out.join(format!("abba_{leg}_smoothing_{enabled}.png")),
            &pixels, RES.0, RES.1, image::ColorType::Rgba8).unwrap();
        if leg == 0 { baseline = Some(pixels); }
        else if enabled { assert_ne!(Some(&pixels), baseline.as_ref(), "ON must visibly change the crowd"); }
        else { assert_eq!(pixels, baseline.take().unwrap(), "OFF repeat must be pixel exact"); }
    }
    for (name, samples) in [("world_gpu", &gpu), ("render_submit_cpu_wall", &cpu)] {
        for (enabled, values) in samples.iter().enumerate() {
            writeln!(report, "{name} smoothing={} samples={} mean_ms={:.9} p50_ms={:.9} p95_ms={:.9}",
                enabled == 1, values.len(), mean(values), percentile(values, 0.5), percentile(values, 0.95)).unwrap();
        }
        writeln!(report, "{name} ABBA_on_minus_off_ms={:.9}", mean(&samples[1]) - mean(&samples[0])).unwrap();
    }
    // Full-resolution strips keep the actual replicated bodies in view, not the original own avatar.
    let mut strips = [image::RgbaImage::new(RES.0 * 3, RES.1), image::RgbaImage::new(RES.0 * 3, RES.1)];
    let mut previous_palettes: Option<Vec<Vec<u32>>> = None;
    let mut previous_pixels: Option<Vec<u8>> = None;
    for pose in 0..3 {
        within_lock();
        // Advance only native limbs: world/sky/camera stay frozen, so rendered differences
        // cannot be satisfied by moving scenery or the replaced original own avatar.
        for avatar in crowd.avatars.iter_mut() { avatar.update(0.17); }
        o.frame(&mut crowd, 0.0);
        let palettes: Vec<Vec<_>> = crowd.frames.iter().map(|actor| actor.skin_pose.as_ref().unwrap()
            .iter().flat_map(|bone| bone.rows.iter().flatten().map(|value| value.to_bits())).collect()).collect();
        if let Some(previous) = &previous_palettes {
            assert!(palettes.iter().zip(previous).all(|(current, before)| current != before),
                "every replicated body's native limb palette must advance");
        }
        previous_palettes = Some(palettes);
        for enabled in [false, true] {
            o.set_enhanced_settings(ao_render::EnhancedSettings { character_smoothing: enabled, ..Default::default() });
            o.wait_for_pipeline_warmup();
            o.render_frozen().unwrap();
            let pixels = o.frozen_rgba().unwrap();
            if !enabled {
                if let Some(previous) = &previous_pixels { assert_ne!(&pixels, previous, "rendered native walking poses must change"); }
                previous_pixels = Some(pixels.clone());
            }
            let image = image::RgbaImage::from_raw(RES.0, RES.1, pixels).unwrap();
            image::imageops::replace(&mut strips[usize::from(enabled)], &image, i64::from(pose * RES.0), 0);
        }
    }
    for (enabled, strip) in strips.iter().enumerate() {
        strip.save(out.join(format!("animation_smoothing_{}.png", enabled == 1))).unwrap();
    }
    writeln!(report, "PASS: 60/60 submitted native humanoids within15m; frozen OFF exact parity; both ON legs visibly differ; three advancing native walking palettes and rendered poses. Animation is synthetic fixture playback of native walk, not captured movement. GPU scope is frozen world rendering including native skin/smoothing; CPU is render submission only. elapsed_seconds={:.3}", started.elapsed().as_secs_f64()).unwrap();
    report.flush().unwrap();
    csv.flush().unwrap();
    within_lock();
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
