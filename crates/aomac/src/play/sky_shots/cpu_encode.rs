//! v2 main-thread CPU cost (record + wgpu finish + submit) vs Original on one authored view, offscreen.
//! `AOMAC_GFX_CPU_ENCODE=1 AOMAC_GFX_VIEW=icc-landmark-authored cargo test --release -p aomac graphics_v2_cpu_encode -- --ignored --nocapture`
//! (GPU timing lock first). The GPU is drained after every sample so the numbers are CPU only. Optional
//! `AOMAC_GFX_CPU_ENCODE_BUDGET_MS` fails the test when the v2-high mean wall exceeds it.
use super::*;
use anyhow::{Context, ensure};

const FRAMES: usize = 300;
const WARM: usize = 60;

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    v.sort_by(f64::total_cmp);
    (mean, v[(0.95 * v.len() as f64).ceil() as usize - 1])
}

#[test]
#[ignore = "opt-in AOMAC_GFX_CPU_ENCODE=1; installed client, GPU timing lock"]
fn graphics_v2_cpu_encode() -> anyhow::Result<()> {
    if std::env::var("AOMAC_GFX_CPU_ENCODE").as_deref() != Ok("1") { return Ok(()); }
    let view = std::env::var("AOMAC_GFX_VIEW").unwrap_or_else(|_| "icc-landmark-authored".into());
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let weather = ao_formats::weather::State::clear();
    let actor = graphics_fixture_actor()?;
    let (_, pf, time, fixed) = graphics_fixture_views(&actor).into_iter().find(|v| v.0 == view).context("unknown AOMAC_GFX_VIEW")?;
    ao_scene::crt_srand(1);
    let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
    let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
    super::super::player::bake_sky_open(&collision, &mut scene);
    // Bodies stand on the collision ground below the camera, a few metres ahead.
    let mut actor_at = fixed.pos + fixed.forward() * 5.0;
    actor_at.y = collision.ground(actor_at.to_array()).unwrap_or(fixed.pos.y - 1.7);
    let mut avatar = super::super::avatar::Avatar::new(&store, &dir, 25988, &actor,
        ao_formats::character::actor::ActorAssets::new(&store)?)?;
    avatar.set_transform(actor_at.to_array(), 0.0);
    let mut means = Vec::new();
    for profile in ["faithful", "v2-high"] {
        let settings = graphics_fixture_settings(profile)?;
        ao_scene::crt_srand(1);
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
        let mut off = Offscreen::evidence_timing(&shot, (1280, 828))?;
        off.fixed_camera(Some(fixed));
        off.host.msaa_samples = Some(1);
        off.host.anisotropy = Some(1);
        off.host.fog_density_scale = Some(1.0);
        off.host.enhanced_settings = Some(settings);
        off.host.grass_wind = weather.wind;
        off.host.actor_models.push((super::super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
        // The actors persist in the renderer after one logic frame (`frame` also renders once the perf target exists, so it
        // is never called inside the timed loop: a second identical render would hit every per-frame cache).
        let actors: usize = std::env::var("AOMAC_GFX_CPU_ENCODE_ACTORS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        off.host.dynamic_lights.extend(avatar.lights());
        let mut frame = avatar.frame();
        off.host.actors.push(frame.clone());
        for k in 1..actors {
            // Extra bodies share the captured model/pose on a ring around the first, each its own actor id.
            frame.id = 1000 + k as u32;
            frame.transform = avatar.frame().transform;
            let angle = k as f32 * 0.9;
            frame.transform[3][0] += angle.cos() * 2.5;
            frame.transform[3][2] += angle.sin() * 2.5;
            off.host.actors.push(frame.clone());
        }
        off.host.post_focus = Some(actor_at.to_array());
        off.frame(&mut shot, 0.0);
        off.wait_for_pipeline_warmup();
        off.prepare_frozen_rendering();
        off.set_cpu_timing(true);
        let sync_every: usize = std::env::var("AOMAC_GFX_CPU_ENCODE_SYNC_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        if settings.enhanced_v2 && std::env::var_os("AOMAC_GFX_CPU_ENCODE_TIMERS").is_some() { off.set_v2_pass_timing(true)?; }
        let (mut wall, mut prepare, mut encode, mut finish, mut submit) = (vec![], vec![], vec![], vec![], vec![]);
        let frames = std::env::var("AOMAC_GFX_CPU_ENCODE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(FRAMES);
        for index in 0..WARM + frames {
            // A walking camera (up to ~0.09 m per frame along its heading) and advancing scene time, as in the real window.
            let mut camera = fixed;
            camera.pos += fixed.forward() * (index as f32 * 0.03).sin() * 3.0;
            off.fixed_camera(Some(camera));
            off.host.camera = camera;
            off.advance_effect_time(1.0 / 60.0);
            let start = std::time::Instant::now();
            off.render_frozen()?;
            let ms = start.elapsed().as_secs_f64() * 1e3;
            let cpu = off.frame_cpu_timings().context("cpu timings")?;
            // Frames stay in flight like the real window's; drain every `sync_every` frames.
            if index % sync_every == sync_every - 1 { off.wait_idle(); }
            if index < WARM { continue; }
            wall.push(ms);
            prepare.push(cpu.prepare_ms);
            encode.push(cpu.encode_ms);
            finish.push(cpu.finish_ms);
            submit.push(cpu.queue_submit_ms);
        }
        let st = off.frame_stats();
        eprintln!("CPU_ENCODE {view} {profile} instances {} draws {}", st.instances, st.draw_calls);
        let (wall_mean, wall_p95) = stats(wall);
        eprintln!("CPU_ENCODE {view} {profile} {}x{} wall mean {wall_mean:.3} p95 {wall_p95:.3} | prepare {:.3} encode {:.3} finish {:.3} submit {:.3} ms",
            1280, 828, stats(prepare).0, stats(encode).0, stats(finish).0, stats(submit).0);
        means.push(wall_mean);
    }
    if let Some(budget) = std::env::var("AOMAC_GFX_CPU_ENCODE_BUDGET_MS").ok().and_then(|v| v.parse::<f64>().ok()) {
        ensure!(means[1] <= budget, "v2-high CPU wall {:.3} ms exceeds budget {budget} ms", means[1]);
    }
    Ok(())
}
