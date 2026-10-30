//! Frame-to-frame stability regression: Original vs Enhanced v2 on consecutive offscreen frames of one authored view.
//! Three modes per preset (frozen camera, +-1 mm camera micro-motion, animated scene time with 4..20 ms dt jitter), each
//! with a real actor shown and removed during the run (admission-churn control). Effect time is the renderer clock
//! (`Offscreen::frame(dt)` adds dt to it): dt 0 pins it, so the clock is fully explicit; no wall clock reaches v2.
//! `AOMAC_GFX_TEMPORAL_REGRESSION=1 AOMAC_GFX_SETTINGS=v2-high AOMAC_GFX_VIEW=icc-landmark-authored AOMAC_GFX_OUT=<new dir>
//!  cargo test --profile fast -p aomac graphics_temporal_stability -- --ignored --nocapture` (GPU lock first).
use super::*;
use anyhow::{Context, ensure};
use std::fmt::Write;

const SIZE: (u32, u32) = (1920, 1200);
const WARM: usize = 64;
const MEASURED: usize = 24;
/// Sobel |gx|+|gy| on 0..255 luma above which a pixel is an edge (about a 40-code step).
const EDGE_GRADIENT: i32 = 160;
const MICRO_STEP: f32 = 0.001;

/// One frame plus the pixel classes of its content.
struct Analysed {
    rgba: Vec<u8>,
    edge: Vec<bool>,
    shadow: Vec<bool>,
}

fn luma(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().map(|p| ((u32::from(p[0]) * 54 + u32::from(p[1]) * 183 + u32::from(p[2]) * 19) >> 8) as u8).collect()
}

/// Edge = high Sobel gradient. Shadow = non-edge pixel clearly darker than its lit 17x17 neighbourhood.
fn analyse(rgba: Vec<u8>, (w, h): (usize, usize)) -> Analysed {
    let l = luma(&rgba);
    let at = |x: usize, y: usize| i32::from(l[y * w + x]);
    let mut edge = vec![false; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let gx = at(x + 1, y - 1) + 2 * at(x + 1, y) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2 * at(x - 1, y) - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2 * at(x, y + 1) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2 * at(x, y - 1) - at(x + 1, y - 1);
            edge[y * w + x] = gx.abs() + gy.abs() > EDGE_GRADIENT;
        }
    }
    let mut sum = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(l[y * w + x]);
            sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + row;
        }
    }
    let mut shadow = vec![false; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(8), (y + 9).min(h));
        for x in 0..w {
            if edge[y * w + x] { continue; }
            let (x0, x1) = (x.saturating_sub(8), (x + 9).min(w));
            let total = sum[y1 * (w + 1) + x1] + sum[y0 * (w + 1) + x0] - sum[y0 * (w + 1) + x1] - sum[y1 * (w + 1) + x0];
            let mean = total as f32 / ((y1 - y0) * (x1 - x0)) as f32;
            shadow[y * w + x] = mean > 40.0 && f32::from(l[y * w + x]) < 0.7 * mean;
        }
    }
    Analysed { rgba, edge, shadow }
}

#[derive(Clone, Copy, Default, Debug)]
struct Delta {
    whole: f64,
    shadow: f64,
    edge: f64,
    worst: u8,
}

/// Mean absolute RGB delta overall and inside the previous frame's shadow/edge classes, plus the worst single channel, over the pixels
/// not set in `exclude` (empty = all pixels).
fn delta(a: &Analysed, b: &Analysed, exclude: &[bool]) -> Delta {
    let (mut whole, mut shadow, mut edge, mut worst) = (0u64, 0u64, 0u64, 0u8);
    let (mut whole_n, mut shadow_n, mut edge_n) = (0u64, 0u64, 0u64);
    for (i, (p, q)) in a.rgba.as_chunks::<4>().0.iter().zip(b.rgba.as_chunks::<4>().0.iter()).enumerate() {
        if exclude.get(i).copied().unwrap_or(false) { continue; }
        let d = [0, 1, 2].map(|c| p[c].abs_diff(q[c]));
        let s = u64::from(d[0]) + u64::from(d[1]) + u64::from(d[2]);
        whole += s;
        whole_n += 1;
        worst = worst.max(d[0]).max(d[1]).max(d[2]);
        if a.shadow[i] { shadow += s; shadow_n += 1; }
        if a.edge[i] { edge += s; edge_n += 1; }
    }
    let mean = |sum: u64, n: u64| if n == 0 { 0.0 } else { sum as f64 / (3 * n) as f64 };
    Delta { whole: mean(whole, whole_n), shadow: mean(shadow, shadow_n), edge: mean(edge, edge_n), worst }
}

/// `max(1.2 x Original, floor)`.
fn bound(original: f64, floor: f64) -> f64 { (1.2 * original).max(floor) }

struct Run {
    /// (frame index, delta, skipped: an actor toggle at this frame legitimately changes pixels).
    rows: Vec<(usize, Delta, bool)>,
    /// The same frames' delta over the whole image (actor rects included), for the report only.
    all: Vec<Delta>,
    worst: (Vec<u8>, Vec<u8>),
}

#[derive(Clone, Copy, Default, Debug)]
struct Summary { whole: f64, shadow: f64, edge: f64, worst: u8 }

impl Run {
    fn summary(&self) -> Summary {
        let used: Vec<_> = self.rows.iter().filter(|r| !r.2).map(|r| r.1).collect();
        let mean = |f: fn(&Delta) -> f64| used.iter().map(f).sum::<f64>() / used.len().max(1) as f64;
        Summary {
            whole: mean(|d| d.whole), shadow: mean(|d| d.shadow), edge: mean(|d| d.edge),
            worst: used.iter().map(|d| d.worst).max().unwrap_or(0),
        }
    }
}

/// Bound failures of `v2` against the same-run `original` (frozen and micro-motion modes only).
fn failures(mode: &str, original: Summary, v2: Summary) -> Vec<String> {
    let mut out = Vec::new();
    for (name, got, limit) in [
        ("whole", v2.whole, bound(original.whole, 0.25)),
        ("shadow", v2.shadow, bound(original.shadow, 0.25)),
        ("edge", v2.edge, bound(original.edge, 1.0)),
        ("worst", f64::from(v2.worst), bound(f64::from(original.worst), 64.0)),
    ] {
        if got > limit { out.push(format!("{mode}: {name} {got:.3} > bound {limit:.3}")); }
    }
    out
}

fn lcg(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state >> 8) as f32 / (1u32 << 24) as f32
}

/// Pixel bounds of the bounding cube of a sphere (the actor's body) under `view_proj`, grown 2x + 64 px per side: the padded rect
/// the renderer drops history in (1.25x + 24 px) with slack, so the churn metric excludes only what the region reset may touch.
fn actor_rect(view_proj: glam::Mat4, centre: Vec3, radius: f32) -> [f32; 4] {
    let (w, h) = (SIZE.0 as f32, SIZE.1 as f32);
    let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
    for corner in 0..8 {
        let o = Vec3::new(if corner & 1 == 0 { -radius } else { radius }, if corner & 2 == 0 { -radius } else { radius }, if corner & 4 == 0 { -radius } else { radius });
        let clip = view_proj * (centre + o).extend(1.0);
        assert!(clip.w > 1e-3, "fixture actor behind the camera");
        let p = glam::Vec2::new((clip.x / clip.w * 0.5 + 0.5) * w, (0.5 - clip.y / clip.w * 0.5) * h);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let (c, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5 * 2.0 + glam::Vec2::splat(64.0));
    [(c.x - half.x).max(0.0), (c.y - half.y).max(0.0), (c.x + half.x).min(w), (c.y + half.y).min(h)]
}

/// `true` for every pixel inside any of `rects`.
fn rect_mask(rects: &[[f32; 4]]) -> Vec<bool> {
    let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
    let mut mask = vec![false; w * h];
    for r in rects {
        for y in (r[1] as usize)..(r[3].ceil() as usize).min(h) {
            for x in (r[0] as usize)..(r[2].ceil() as usize).min(w) { mask[y * w + x] = true; }
        }
    }
    mask
}

#[test]
#[ignore = "offline temporal stability regression; opt-in AOMAC_GFX_TEMPORAL_REGRESSION=1, installed client, GPU timing lock"]
fn graphics_temporal_stability_matches_original() -> anyhow::Result<()> {
    if std::env::var("AOMAC_GFX_TEMPORAL_REGRESSION").as_deref() != Ok("1") { return Ok(()); }
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a new directory")?);
    ensure!(!out.exists(), "refusing to overwrite evidence {}", out.display());
    std::fs::create_dir_all(&out)?;
    let profile = std::env::var("AOMAC_GFX_SETTINGS").unwrap_or_else(|_| "v2-high".into());
    let view = std::env::var("AOMAC_GFX_VIEW").unwrap_or_else(|_| "icc-landmark-authored".into());
    ensure!(!profile.eq_ignore_ascii_case("original"), "AOMAC_GFX_SETTINGS must name the Enhanced preset under test");
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let weather = ao_formats::weather::State::clear();
    let actor = graphics_fixture_actor()?;
    let (_, pf, time, fixed) = graphics_fixture_views(&actor).into_iter().find(|v| v.0 == view).context("unknown AOMAC_GFX_VIEW")?;
    ao_scene::crt_srand(1);
    let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
    let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
    super::super::player::bake_sky_open(&collision, &mut scene);
    // A real captured body 12 m ahead of the camera, on the ground, shown for frames 8..16 of every 16.
    let mut actor_at = fixed.pos + fixed.forward() * 12.0;
    actor_at.y = collision.terrain_height(actor_at.to_array()).unwrap_or(fixed.pos.y - 1.6);
    let collision = std::rc::Rc::new(std::cell::RefCell::new(collision));
    let mut avatar = super::super::avatar::Avatar::new(&store, &dir, 25988, &actor,
        ao_formats::character::actor::ActorAssets::new(&store)?)?;
    avatar.set_transform(actor_at.to_array(), 0.0);
    // Stress mode: 12 actors on a 3 x 4 grid around the single churn actor, toggling at staggered periods (a crowded platform).
    let right = fixed.forward().cross(Vec3::Y).normalize();
    let stress_spots: Vec<Vec3> = std::iter::once(actor_at).chain((0..11).map(|k| {
        let mut p = actor_at + right * ((k % 4) as f32 - 1.5) * 2.5 + fixed.forward() * ((k / 4) as f32 * 3.0 - 3.0);
        p.y = collision.borrow().terrain_height(p.to_array()).unwrap_or(actor_at.y);
        p
    })).collect();
    let masks: Vec<[f32; 4]> = {
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        let view_proj = glam::Mat4::perspective_rh(lens.vertical_fov(aspect), aspect, lens.near, lens.far.unwrap_or(1000.0))
            * glam::Mat4::look_to_rh(fixed.pos, fixed.forward(), fixed.up());
        stress_spots.iter().map(|p| actor_rect(view_proj, *p + Vec3::Y, 1.6)).collect()
    };

    let probe = match std::env::var("AOMAC_GFX_TEMPORAL_PROBE") {
        Ok(value) => {
            let (x, y) = value.split_once(',').context("AOMAC_GFX_TEMPORAL_PROBE must be x,y")?;
            let (x, y) = (x.trim().parse::<usize>()?, y.trim().parse::<usize>()?);
            ensure!(x < SIZE.0 as usize && y < SIZE.1 as usize, "AOMAC_GFX_TEMPORAL_PROBE outside the frame");
            Some((x, y))
        }
        Err(_) => None,
    };
    let mut run = |mode: &str, preset: &str, settings: ao_render::EnhancedSettings| -> anyhow::Result<Run> {
        let churn = mode == "churn" || mode == "stress";
        let spots = if mode == "stress" { &stress_spots[..] } else { &stress_spots[..1] };
        let outside: Vec<bool> = if churn { rect_mask(&masks[..spots.len()]) } else { Vec::new() };
        ao_scene::crt_srand(1);
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
        let mut off = Offscreen::evidence(&shot, SIZE)?;
        off.fixed_camera(Some(fixed));
        off.host.msaa_samples = Some(1);
        off.host.anisotropy = Some(1);
        off.host.fog_density_scale = Some(1.0);
        off.host.enhanced_settings = Some(settings);
        off.host.grass_wind = weather.wind;
        let foliage = settings.foliage_enabled() || settings.interaction_enabled();
        if foliage { off.host.grass_exposure = Some(super::super::player::grass_exposure_surface(collision.clone())); }
        off.frame(&mut shot, 0.0);
        if foliage { off.warm_grass_tiles(); }
        off.wait_for_pipeline_warmup();
        // The scene upload of the first frame resets actors: the model must be handed over AFTER it, or the churn actor is never drawn.
        off.host.actor_models.push((super::super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
        off.prepare_frozen_rendering();
        let (mut rng, mut z) = (12345u32, 0.0_f32);
        let (mut previous, mut rows, mut all) = (None::<Analysed>, Vec::new(), Vec::new());
        let mut worst = (0u8, Vec::new(), Vec::new());
        for index in 0..=WARM + MEASURED {
            let mut camera = fixed;
            let mut dt = 0.0;
            match mode {
                "micro" => {
                    let step = if lcg(&mut rng) < 0.5 { -0.5 } else { 0.5 } * MICRO_STEP;
                    z = (z + step).clamp(-MICRO_STEP, MICRO_STEP);
                    camera.pos.x += if index % 2 == 0 { MICRO_STEP } else { -MICRO_STEP };
                    camera.pos.z += z;
                }
                "animated" => dt = 0.004 + lcg(&mut rng) * 0.016,
                _ => {}
            }
            off.fixed_camera(Some(camera));
            for (k, &spot) in spots.iter().enumerate() {
                let shown = if mode == "stress" { (index + 3 * k) % (6 + 2 * k) >= 3 + k } else { churn && index % 16 >= 8 };
                if !shown { continue; }
                avatar.set_transform(spot.to_array(), 0.0);
                let mut frame = avatar.frame();
                frame.id += k as u32;
                off.host.actors.push(frame);
                if mode != "stress" {
                    off.host.dynamic_lights.extend(avatar.lights());
                    off.host.post_focus = Some(actor_at.to_array());
                    if foliage { off.host.grass_benders.push(ao_render::GrassBender { actor: None, position: actor_at.to_array() }); }
                }
            }
            off.frame(&mut shot, dt);
            if index < WARM { continue; }
            let current = analyse(off.frozen_rgba()?, (SIZE.0 as usize, SIZE.1 as usize));
            if let Some((px, py)) = probe {
                let at = (py * SIZE.0 as usize + px) * 4;
                let p = &current.rgba[at..at + 3];
                eprintln!("PROBE {mode} {preset} {index} {} {} {}", p[0], p[1], p[2]);
            }
            if let Some(before) = &previous {
                let d = delta(before, &current, &outside);
                // Only the single-actor churn pair where the actor appears or disappears changes real pixels outside its rect
                // legitimately (its lights); the stress mode toggles somebody every frame and skips nothing.
                let toggle = mode == "churn" && index % 8 == 0;
                rows.push((index, d, toggle));
                all.push(delta(before, &current, &[]));
                if !toggle && d.worst >= worst.0 { worst = (d.worst, before.rgba.clone(), current.rgba.clone()); }
            }
            previous = Some(current);
        }
        Ok(Run { rows, all, worst: (worst.1, worst.2) })
    };

    let (original_settings, v2_settings) = (graphics_fixture_settings("Original")?, graphics_fixture_settings(&profile)?);
    let (mut failed, mut table) = (Vec::new(), String::new());
    writeln!(table, "mode,preset,whole,shadow,edge,worst")?;
    // `AOMAC_GFX_TEMPORAL_MODES=churn,stress` runs only those modes (default: all five).
    let only = std::env::var("AOMAC_GFX_TEMPORAL_MODES").unwrap_or_default();
    for mode in ["frozen", "micro", "animated", "churn", "stress"].into_iter().filter(|m| only.is_empty() || only.split(',').any(|o| o == *m)) {
        let mut summaries = Vec::new();
        for (preset, settings) in [("original", original_settings), (profile.as_str(), v2_settings)] {
            let result = run(mode, preset, settings)?;
            let mut csv = String::from("frame,whole,shadow,edge,worst,skipped_actor_toggle\n");
            for (frame, d, skipped) in &result.rows {
                writeln!(csv, "{frame},{:.4},{:.4},{:.4},{},{skipped}", d.whole, d.shadow, d.edge, d.worst)?;
            }
            std::fs::write(out.join(format!("temporal-{mode}-{preset}.csv")), csv)?;
            let s = result.summary();
            writeln!(table, "{mode},{preset},{:.4},{:.4},{:.4},{}", s.whole, s.shadow, s.edge, s.worst)?;
            let all: Vec<_> = result.rows.iter().zip(&result.all).filter(|(r, _)| !r.2).map(|(_, d)| d.whole).collect();
            eprintln!("TEMPORAL mode={mode} preset={preset} whole={:.4} shadow={:.4} edge={:.4} worst={} (whole image incl. actor rects: mean {:.4} peak {:.4})",
                s.whole, s.shadow, s.edge, s.worst, all.iter().sum::<f64>() / all.len().max(1) as f64, all.iter().fold(0.0_f64, |a, &b| a.max(b)));
            summaries.push((s, result));
        }
        // Animated time is real motion: numbers only, no bound. Churn / stress measure OUTSIDE the toggling actors' padded rects (the
        // region-limited admission reset leaves everything else converged), so they hold the frozen bounds, worst pixel included.
        if mode == "animated" { continue; }
        let broken = failures(mode, summaries[0].0, summaries[1].0);
        if !broken.is_empty() {
            let (a, b) = &summaries[1].1.worst;
            for (suffix, image) in [("a", a), ("b", b)] {
                image::save_buffer(out.join(format!("temporal-{mode}-{profile}-worst-{suffix}.png")), image, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
            }
        }
        failed.extend(broken);
    }
    std::fs::write(out.join("temporal-summary.csv"), &table)?;
    eprintln!("{table}");
    ensure!(failed.is_empty(), "temporal stability bounds failed:\n{}", failed.join("\n"));
    Ok(())
}

fn flat(w: usize, h: usize, v: u8) -> Vec<u8> { [v, v, v, 255].repeat(w * h) }

#[test]
fn temporal_metrics_are_zero_for_identical_frames_and_see_single_pixel_changes() {
    let (w, h) = (40, 30);
    let mut a = flat(w, h, 100);
    for y in 0..h { for x in w / 2..w { let i = (y * w + x) * 4; a[i..i + 3].fill(200); } }
    let before = analyse(a.clone(), (w, h));
    let same = delta(&before, &analyse(a.clone(), (w, h)), &[]);
    assert_eq!((same.whole, same.shadow, same.edge, same.worst), (0.0, 0.0, 0.0, 0));
    assert!(before.edge[15 * w + w / 2] && !before.edge[15 * w + 5], "the step is an edge, the flat side is not");
    let mut b = a;
    b[(15 * w + w / 2) * 4] = 150;
    let d = delta(&before, &analyse(b.clone(), (w, h)), &[]);
    let mut masked = vec![false; w * h];
    masked[15 * w + w / 2] = true;
    assert_eq!(delta(&before, &analyse(b, (w, h)), &masked).worst, 0, "the changed pixel lies in the excluded rect");
    assert_eq!(d.worst, 50);
    assert!(d.edge > 0.0 && d.whole > 0.0 && d.shadow == 0.0);
}

#[test]
fn temporal_shadow_class_is_dark_pixels_inside_lit_surroundings() {
    let (w, h) = (40, 40);
    let mut a = flat(w, h, 180);
    for y in 18..22 { for x in 18..22 { let i = (y * w + x) * 4; a[i..i + 3].fill(120); } }
    let frame = analyse(a, (w, h));
    assert!(frame.shadow[20 * w + 19], "the interior of the dark patch is shadow");
    assert!(!frame.shadow[5 * w + 5], "uniform lit ground is not shadow");
}

#[test]
fn temporal_bounds_are_relative_to_original_with_floors() {
    let original = Summary { whole: 0.0, shadow: 0.0, edge: 0.0, worst: 0 };
    assert!(failures("m", original, Summary { whole: 0.25, shadow: 0.25, edge: 1.0, worst: 64 }).is_empty());
    assert_eq!(failures("m", original, Summary { whole: 0.3, shadow: 0.0, edge: 1.5, worst: 65 }).len(), 3);
    let noisy = Summary { whole: 1.0, shadow: 1.0, edge: 2.0, worst: 100 };
    assert!(failures("m", noisy, Summary { whole: 1.2, shadow: 1.2, edge: 2.4, worst: 120 }).is_empty());
    assert_eq!(failures("m", noisy, Summary { whole: 1.3, shadow: 1.0, edge: 2.0, worst: 100 }).len(), 1);
}
