//! Enhanced v2 post regressions (offscreen, real client data, GPU lock required): the glow rule (Newland lamp pole stays native,
//! the lamp head still glows) and the temporal fairness / stability rules (admission flip leaves no history residue, a moving actor
//! leaves no trail, a static camera does not flicker). Same fixture machinery as the v1 lamp regression in `sky_shots.rs`.
use super::*;
use anyhow::{Context, ensure};
use ao_scene::ActorFrame;
use std::sync::Arc;

const SIZE: (u32, u32) = (1920, 1200);

struct View {
    scene: Scene,
    fixed: Camera,
    exposure: ao_render::GrassExposure,
}

fn load_view(name: &str) -> anyhow::Result<View> {
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let weather = ao_formats::weather::State::clear();
    let actor = graphics_fixture_actor()?;
    let (_, pf, time, camera) = graphics_fixture_views(&actor).into_iter().find(|view| view.0 == name).context("fixture view")?;
    ao_scene::crt_srand(1);
    let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
    let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
    super::super::player::bake_sky_open(&collision, &mut scene);
    let fixed = if pf == 566 { graphics_fixture_newland_camera(|point| collision.terrain_height(point))? } else { camera };
    let exposure = super::super::player::grass_exposure_surface(std::rc::Rc::new(std::cell::RefCell::new(collision)));
    Ok(View { scene, fixed, exposure })
}

/// One renderer session of `view` under `settings`; `steps` is the actor frame (if any) handed to the renderer on each frame.
/// Returns one RGBA8 capture per step.
fn render_sequence(view: &View, settings: ao_render::EnhancedSettings, model: Option<&Arc<Scene>>, steps: &[Option<ActorFrame>]) -> anyhow::Result<Vec<Vec<u8>>> {
    let dir = ao_gui::client_dir();
    ao_scene::crt_srand(1);
    let mut lens = camera::lens(view.scene.lens.unwrap_or_default(), false);
    lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
    let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(view.scene.clone()), camera: view.fixed, lens };
    let mut off = Offscreen::evidence(&shot, SIZE)?;
    off.fixed_camera(Some(view.fixed));
    off.host.msaa_samples = Some(1);
    off.host.anisotropy = Some(1);
    off.host.enhanced_settings = Some(settings);
    off.host.grass_exposure = Some(view.exposure.clone());
    off.host.grass_wind = ao_formats::weather::State::clear().wind;
    if let Some(model) = model { off.host.actor_models.push((super::super::avatar::MODEL_KEY, model.clone())); }
    let mut frames = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        if let Some(frame) = step { off.host.actors.push(frame.clone()); }
        let list = off.frame(&mut shot, 0.0);
        if index == 0 {
            off.warm_grass_tiles();
            off.wait_for_pipeline_warmup();
        }
        frames.push(off.rgba(&shot, &list)?);
    }
    Ok(frames)
}

fn luma_at(rgba: &[u8], x: usize, y: usize) -> f64 {
    let p = &rgba[(y * SIZE.0 as usize + x) * 4..];
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn mean_luma(rgba: &[u8], at: &dyn Fn(usize, usize) -> bool) -> f64 {
    let (mut sum, mut count) = (0.0_f64, 0.0_f64);
    for y in 0..SIZE.1 as usize { for x in 0..SIZE.0 as usize { if at(x, y) { sum += luma_at(rgba, x, y); count += 1.0; } } }
    sum / count.max(1.0)
}

fn mean_abs_diff(a: &[u8], b: &[u8], at: &dyn Fn(usize, usize) -> bool) -> f64 {
    let (mut sum, mut count) = (0.0_f64, 0.0_f64);
    for y in 0..SIZE.1 as usize { for x in 0..SIZE.0 as usize {
        if !at(x, y) { continue; }
        let i = (y * SIZE.0 as usize + x) * 4;
        sum += (0..3).map(|c| (f64::from(a[i + c]) - f64::from(b[i + c])).abs()).sum::<f64>() / 3.0;
        count += 1.0;
    } }
    sum / count.max(1.0)
}

fn save(out: &Option<PathBuf>, name: &str, rgba: &[u8]) -> anyhow::Result<()> {
    if let Some(out) = out { image::save_buffer(out.join(name), rgba, SIZE.0, SIZE.1, image::ColorType::Rgba8)?; }
    Ok(())
}

/// Optional evidence directory (`AOMAC_GFX_OUT`, created if missing).
fn evidence_dir() -> anyhow::Result<Option<PathBuf>> {
    let out = std::env::var_os("AOMAC_GFX_OUT").map(PathBuf::from);
    if let Some(out) = &out { std::fs::create_dir_all(out)?; }
    Ok(out)
}

/// Glow rule (DESIGN section 10, v1 fix abc9343b): the Newland street lamp is one self-lit material (pole and head). v2 bloom is
/// seeded by what the surface emits behind a soft knee, so the dull pole keeps its native look and the head still glows.
#[test]
#[ignore = "offline v2 lamp regression; requires installed client and the GPU timing lock"]
fn graphics_v2_newland_lamp_pole_stays_native_and_head_glows() -> anyhow::Result<()> {
    let out = evidence_dir()?;
    for name in ["newland-day", "newland-night"] {
        let view = load_view(name)?;
        let one = |profile: &str, bloom: Option<f32>| -> anyhow::Result<Vec<u8>> {
            let mut settings = graphics_fixture_settings(profile)?;
            if let Some(bloom) = bloom { settings.bloom_strength = bloom; }
            Ok(render_sequence(&view, settings, None, &[None])?.remove(0))
        };
        let original = one("Original", None)?;
        let v2 = one("v2-high", None)?;
        let v2_flat = one("v2-high", Some(0.0))?;
        save(&out, &format!("{name}-original.png"), &original)?;
        save(&out, &format!("{name}-v2.png"), &v2)?;
        save(&out, &format!("{name}-v2-nobloom.png"), &v2_flat)?;
        // Pole: the dark (< 75 luma) Original pixels of the screen box holding the foreground lamp pole (as the v1 regression).
        let pole = |x: usize, y: usize| (1300..1700).contains(&x) && (400..850).contains(&y) && luma_at(&original, x, y) < 75.0;
        let head = |x: usize, y: usize| (490..570).contains(&x) && (205..265).contains(&y);
        let (pole_original, pole_v2) = (mean_luma(&original, &pole), mean_luma(&v2, &pole));
        let (head_original, head_v2, head_flat) = (mean_luma(&original, &head), mean_luma(&v2, &head), mean_luma(&v2_flat, &head));
        eprintln!("{name}: pole original {pole_original:.1} v2 {pole_v2:.1} ({:.3}x); head original {head_original:.1} v2 {head_v2:.1} v2-no-bloom {head_flat:.1}", pole_v2 / pole_original);
        ensure!(pole_v2 <= 1.25 * pole_original, "{name}: pole {pole_v2:.1} vs Original {pole_original:.1}");
        ensure!(head_v2 >= 1.05 * head_flat, "{name}: head {head_v2:.1} has no bloom halo over its non-bloom value {head_flat:.1}");
    }
    Ok(())
}

/// Fairness (e), DESIGN section 8: an admission flip leaves no TAA history residue. Run A shows the actor on frame 1 and hides it
/// (alpha 0) on frames 2-3; run B admits the same actor on frame 1 behind the camera (invisible, same admission hash) and hides it
/// the same way. Both histories are discarded on frame 2, so frames 2 and 3 must match byte for byte: nothing of the frame-1 actor
/// survived. Frame 1 must differ (positive control).
#[test]
#[ignore = "offline v2 fairness regression; requires installed client and the GPU timing lock"]
fn graphics_v2_admission_flip_leaves_no_history_residue() -> anyhow::Result<()> {
    let out = evidence_dir()?;
    let view = load_view("newland-day")?;
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let mut avatar = super::super::avatar::Avatar::new(&store, &dir, 25988, &graphics_fixture_actor()?, ao_formats::character::actor::ActorAssets::new(&store)?)?;
    let model = Arc::new(avatar.model().clone());
    let frame_at = |avatar: &mut super::super::avatar::Avatar, position: Vec3, alpha: f32| {
        avatar.set_transform(position.to_array(), 0.0);
        let mut frame = avatar.frame();
        frame.id = 25988;
        frame.alpha = alpha;
        frame
    };
    let front = view.fixed.pos + view.fixed.forward() * 3.0 - Vec3::Y;
    let behind = view.fixed.pos - view.fixed.forward() * 6.0 - Vec3::Y;
    let settings = graphics_fixture_settings("v2-high")?;
    let a: Vec<_> = [(front, 1.0), (front, 0.0), (front, 0.0)].into_iter().map(|(p, alpha)| Some(frame_at(&mut avatar, p, alpha))).collect();
    let b: Vec<_> = [(behind, 1.0), (front, 0.0), (front, 0.0)].into_iter().map(|(p, alpha)| Some(frame_at(&mut avatar, p, alpha))).collect();
    let (run_a, run_b) = (render_sequence(&view, settings, Some(&model), &a)?, render_sequence(&view, settings, Some(&model), &b)?);
    for (i, (fa, fb)) in run_a.iter().zip(&run_b).enumerate() {
        save(&out, &format!("flip-a{}.png", i + 1), fa)?;
        save(&out, &format!("flip-b{}.png", i + 1), fb)?;
    }
    ensure!(run_a[0] != run_b[0], "positive control: the visible actor must change frame 1");
    for i in 1..3 {
        let differing = run_a[i].iter().zip(&run_b[i]).filter(|(x, y)| x != y).count();
        ensure!(differing == 0, "frame {}: {differing} bytes differ, the frame-1 actor left history residue", i + 1);
    }
    Ok(())
}

/// Ghost trails: a character walking 0.4 m per frame across a static camera must not leave its colour behind. Compared with the
/// identical run without the actor, the pixels the actor covered early on (and no longer covers) must be back to the background.
/// The same background-only run is the flicker check: a static camera's TAA output may only move by dither-scale amounts.
#[test]
#[ignore = "offline v2 temporal regression; requires installed client and the GPU timing lock"]
fn graphics_v2_moving_actor_leaves_no_trail_and_static_camera_is_stable() -> anyhow::Result<()> {
    let out = evidence_dir()?;
    let view = load_view("newland-day")?;
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let mut avatar = super::super::avatar::Avatar::new(&store, &dir, 25988, &graphics_fixture_actor()?, ao_formats::character::actor::ActorAssets::new(&store)?)?;
    let model = Arc::new(avatar.model().clone());
    let right = view.fixed.forward().cross(Vec3::Y).normalize();
    let start = view.fixed.pos + view.fixed.forward() * 4.0 - Vec3::Y - right * 2.0;
    const FRAMES: usize = 12;
    let walk: Vec<_> = (0..FRAMES).map(|k| {
        avatar.set_transform((start + right * (0.4 * k as f32)).to_array(), 0.0);
        let mut frame = avatar.frame();
        frame.id = 25988;
        Some(frame)
    }).collect();
    let settings = graphics_fixture_settings("v2-high")?;
    let with_actor = render_sequence(&view, settings, Some(&model), &walk)?;
    let background = render_sequence(&view, settings, None, &vec![None; FRAMES])?;
    save(&out, "walk-first.png", &with_actor[2])?;
    save(&out, "walk-last.png", &with_actor[FRAMES - 1])?;
    save(&out, "background-last.png", &background[FRAMES - 1])?;
    let covered = |k: usize| -> Vec<bool> {
        (0..(SIZE.0 * SIZE.1) as usize).map(|i| {
            let (a, b) = (&with_actor[k][i * 4..i * 4 + 3], &background[k][i * 4..i * 4 + 3]);
            (0..3).map(|c| i32::from(a[c]).abs_diff(i32::from(b[c]))).sum::<u32>() > 60
        }).collect()
    };
    let (early, now) = (covered(2), covered(FRAMES - 1));
    let width = SIZE.0 as usize;
    // Trail = covered early, not covered now, and not within 12 px of the current actor (its own edge AA).
    let near_now = |x: usize, y: usize| (y.saturating_sub(12)..(y + 13).min(SIZE.1 as usize)).any(|yy| (x.saturating_sub(12)..(x + 13).min(width)).any(|xx| now[yy * width + xx]));
    let trail: Vec<bool> = (0..early.len()).map(|i| early[i] && !near_now(i % width, i / width)).collect();
    let count = trail.iter().filter(|&&t| t).count();
    ensure!(count > 500, "fixture: the actor covered only {count} trail pixels, the walk is not exercising the test");
    let residue = mean_abs_diff(&with_actor[FRAMES - 1], &background[FRAMES - 1], &|x, y| trail[y * width + x]);
    eprintln!("trail pixels {count}, mean |actor run - background| over them: {residue:.2} LSB");
    ensure!(residue <= 3.0, "ghost trail: {residue:.2} LSB mean over {count} pixels");
    // Static camera: converged consecutive frames differ only by dither / jitter-cycle noise.
    let flicker = (FRAMES - 3..FRAMES - 1).map(|k| mean_abs_diff(&background[k], &background[k + 1], &|_, _| true)).fold(0.0, f64::max);
    eprintln!("static-camera frame-to-frame mean |diff|: {flicker:.3} LSB");
    ensure!(flicker <= 1.0, "static camera flickers by {flicker:.3} LSB");
    Ok(())
}
