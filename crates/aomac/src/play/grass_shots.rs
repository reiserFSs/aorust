//! Paired grass evidence on the shared offline fixture snapshots; no server/window.
//! AOMAC_GFX_VIEW selects one view; AOMAC_GFX_PNG_ONLY=1 omits raw RGBA files.
//! AOMAC_GFX_CAPTURE_ONLY=1 retains explicit tile + 320-frame warmup, skips GPU timing/reverse order.
//! AOMAC_GFX_AO / AOMAC_GFX_SHADOWS (sun) / AOMAC_GFX_CONTACT override EFFECTS with 0/1.
//! AOMAC_GFX_ASSERT_SPECKS=0 records baseline ablations; the speck regression is on by default.
//! grass_offline_temporal_stability writes 120-frame native-weather sequences and paired OFF controls.
//! AOMAC_GFX_TEMPORAL_BASELINE=1 records the same metrics without behavioral assertions.
use super::{Shot, camera, graphics_fixture_actor, graphics_fixture_views};
use ao_gui::Gui;
use ao_render::{EnhancedSettings, Offscreen};
use anyhow::{Context, ensure};
use std::{cell::RefCell, fmt::Write, path::PathBuf, rc::Rc};

#[test]
#[ignore = "offline grass capture/timing matrix; requires assets and a new AOMAC_GFX_OUT"]
fn grass_offline_capture_and_timing() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT")?);
    ensure!(!out.exists(), "refusing to overwrite {}", out.display());
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "status=running\n")?;
    let result = run(&out);
    std::fs::write(out.join("status.txt"), match &result {
        Ok(()) => "status=pass\nvisual_acceptance=unexercised\n".into(),
        Err(error) => format!("status=failed\nerror={error:#}\n"),
    })?;
    result
}

#[test]
#[ignore = "headless ICC grass temporal evidence; requires assets and a new AOMAC_GFX_OUT"]
fn grass_offline_temporal_stability() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT")?);
    ensure!(!out.exists(), "refusing to overwrite {}", out.display());
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "status=running\n")?;
    let result = run_temporal(&out);
    std::fs::write(out.join("status.txt"), match &result {
        Ok(()) => "status=pass\nvisual_acceptance=unexercised\n".into(),
        Err(error) => format!("status=failed\nerror={error:#}\n"),
    })?;
    result
}

fn run_temporal(out: &std::path::Path) -> anyhow::Result<()> {
    use ao_render::{Camera, Vec3};
    const FRAMES: usize = 120;
    const DT: f32 = 1.0 / 60.0;
    let baseline = std::env::var("AOMAC_GFX_TEMPORAL_BASELINE").is_ok_and(|value| value == "1");
    let msaa = std::env::var("AOMAC_GFX_TEMPORAL_MSAA").ok().map(|value| value.parse::<u32>()).transpose()?.unwrap_or(4);
    ensure!(matches!(msaa, 1 | 4), "AOMAC_GFX_TEMPORAL_MSAA must be 1 (diagnostic) or 4");
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let day = ao_formats::weather::OFFLINE_DAY;
    let day_time = 2648.69;
    let size = (960, 600);
    let fixed = Camera { pos: Vec3::new(938.0, 25.7, -750.0), yaw: 0.0, pitch: -0.18534796, roll: 0.0 };
    // Same native ICC tile 258 / grid [234,189] and close camera as the paired fixture above.
    ao_scene::crt_srand(1);
    let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
        &store, &dir, 4582, day_time, day, Some(ao_formats::weather::State::clear()))?;
    let exposure = super::super::player::grass_exposure_surface(Rc::new(RefCell::new(
        ao_formats::playfield::collision::Collision::load(&store, 4582)?)));
    let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
    lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
    let mut manifest = format!(
        "fixture=grass_offline_temporal_stability\nbaseline={baseline}\nsize={}x{}\nframes={FRAMES}\nfixed_dt={DT}\nplayfield=4582\nnative_tile=258\ngrid=[234,189]\ncamera={fixed:?}\nlens={lens:?}\nday={day}\nday_time={day_time}\nscene_weather=clear_fixed\nwind=live_native_Weather_open_weather_update_fixed_day_time_seed1\nwarmup_frames=320\nwarmup_dt=0\nrenderer_initial_time=0\nmsaa={msaa}\nanisotropy=1\nhdr=false\nao=false\nshadows=false\nbloom=false\ntiming=FrameGpuTimer_whole_frame_not_isolated_grass\nwind_off_static=frozen_clock_dt0_constant_zero_wind_unchanged_static_deformation_not_zero_bend\nroi=union_of_paired_on_off_RGB_difference_gt8_on_green_OFF_terrain_lower_two_thirds\nrgb_delta=absolute_encoded_RGB_codes_no_alpha\npop=any_RGB_channel_delta_gt40\nresidual_delta=delta_of_signed_ON_minus_OFF_RGB\nraw_pop_bound=1_percent_blade_ROI_per_frame_static_micro_enforced_at_MSAA4_only\noscillation=consecutive_residual_channel_deltas_both_abs_gt2_sign_reversal\noscillation_weight=min_abs_consecutive_delta_squared\nactive_energy_fraction=reversal_weight_sum_over_qualifying_pair_weight_sum_diagnostic_only_amplitude_normalized_not_acceptance\nopportunities=all_ROI_RGB_channels_each_frame_index_ge2_including_inactive_channels\nstatic_micro_reversal_opportunity_fraction_bound=0.01\nstatic_micro_reversal_RMS_codes_bound=1.0\nreversal_RMS=sqrt_weighted_reversal_energy_over_all_channel_opportunities\nbound_rationale=at_most_1_percent_of_all_blade_channel_opportunities_reverse_and_RMS_reversal_energy_at_most_one_encoded_code\nmicro_camera=0.25mm_lateral_per_frame\nslow_walk=0.25m_per_second_forward\ncontact_sheet=columns_frames_0_60_119_rows_OFF_ON\nvisual_acceptance=unexercised\n",
        size.0, size.1);
    if let Some(shader_dir) = std::env::var_os("AOMAC_SHADER_DIR") {
        writeln!(manifest, "shader_dir={}", PathBuf::from(shader_dir).display())?;
    }
    std::fs::write(out.join("manifest.txt"), &manifest)?;
    let mut failures = Vec::new();
    for name in ["static-wind-on", "wind-off-static", "micro-camera", "slow-walk"] {
        let mut weather = ao_formats::playfield::open_weather(&store, 4582)?;
        let mut pair = Vec::with_capacity(2);
        for grass in [false, true] {
            ao_scene::crt_srand(1);
            let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
            let mut off = Offscreen::evidence_timing(&shot, size)?;
            off.fixed_camera(Some(fixed));
            off.host.msaa_samples = Some(msaa);
            off.host.anisotropy = Some(1);
            off.host.fog_density_scale = Some(1.0);
            off.host.grass_exposure = Some(exposure.clone());
            off.host.grass_wind = [0.0; 3];
            off.host.enhanced_settings = Some(EnhancedSettings {
                ground_grass: true, grass_density: if grass { 1.0 } else { 0.0 }, ..Default::default()
            });
            off.frame(&mut shot, 0.0);
            off.warm_grass_tiles();
            for _ in 0..320 { off.frame(&mut shot, 0.0); }
            off.set_gpu_timing(true)?;
            pair.push((off, shot));
        }
        let mut roi = vec![false; (size.0 * size.1) as usize];
        let mut previous: Option<[Vec<u8>; 2]> = None;
        let mut previous_delta = vec![0_i16; roi.len() * 3];
        let mut oscillation_total = [0_u64; 4];
        let mut opportunities_total = 0_u64;
        let mut raw_pop_exceedance_frames = 0;
        // Per-frame compact metric accumulators; do not retain 120 full-resolution frame pairs.
        let mut rows = Vec::with_capacity(FRAMES);
        let mut sheet = image::RgbaImage::new(size.0 * 3, size.1 * 2);
        for frame in 0..FRAMES {
            let dt = if name == "wind-off-static" { 0.0 } else { DT };
            weather.update(day, f64::from(day_time), DT);
            let wind = if name == "wind-off-static" { [0.0; 3] } else { weather.state().wind };
            let mut view = fixed;
            if name == "micro-camera" {
                view.pos.x += frame as f32 * 0.00025;
            } else if name == "slow-walk" {
                let forward = fixed.forward();
                view.pos += Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero() * (frame as f32 * DT * 0.25);
            }
            let mut captures = Vec::with_capacity(2);
            let mut gpu = [0.0; 2];
            for (variant, (off, shot)) in pair.iter_mut().enumerate() {
                off.fixed_camera(Some(view));
                off.host.grass_wind = wind;
                let list = off.frame(shot, dt);
                gpu[variant] = off.read_frame_gpu_ms()?.context("FrameGpuTimer timestamps unavailable")?;
                captures.push(off.rgba(shot, &list)?);
                // rgba renders again: consume that query too before the next frame begins.
                off.read_frame_gpu_ms()?.context("capture GPU timestamps unavailable")?;
            }
            let captures: [Vec<u8>; 2] = captures.try_into().map_err(|_| anyhow::anyhow!("missing temporal pair"))?;
            let column = match frame { 0 => Some(0), 60 => Some(1), 119 => Some(2), _ => None };
            for (pixel, eligible) in roi.iter_mut().enumerate() {
                let i = pixel * 4;
                let off = &captures[0][i..i + 3];
                let green = off[1] as f32 > 1.1 * off[0] as f32 && off[1] as f32 > 1.1 * off[2] as f32;
                let blade = (0..3).any(|c| captures[1][i + c].abs_diff(off[c]) > 8);
                *eligible |= pixel / size.0 as usize >= size.1 as usize / 3 && green && blade;
            }
            if let Some(column) = column {
                for (variant, raw) in captures.iter().enumerate() {
                    for y in 0..size.1 {
                        let start = y as usize * size.0 as usize * 4;
                        let destination = ((variant as u32 * size.1 + y) * sheet.width() + column * size.0) as usize * 4;
                        sheet.as_mut()[destination..destination + size.0 as usize * 4]
                            .copy_from_slice(&raw[start..start + size.0 as usize * 4]);
                    }
                }
            }
            // Growing union of blade-bearing pixels; each CSV row records its actual denominator.
            let count = roi.iter().filter(|&&eligible| eligible).count();
            let mut sums = [0_u64; 3]; // OFF, ON, signed grass residual
            let mut pops = [0_usize; 3];
            let mut oscillation = [0_u64; 4];
            if let Some(previous) = &previous {
                for (pixel, &eligible) in roi.iter().enumerate() {
                    if !eligible { continue; }
                    let i = pixel * 4;
                    let mut max_delta = [0_u16; 3];
                    for c in 0..3 {
                        let a = i16::from(captures[0][i + c]) - i16::from(previous[0][i + c]);
                        let b = i16::from(captures[1][i + c]) - i16::from(previous[1][i + c]);
                        let index = pixel * 3 + c;
                        let delta = b - a;
                        if frame >= 2 {
                            let sample = temporal_oscillation(previous_delta[index], delta);
                            for metric in 0..4 { oscillation[metric] += sample[metric]; }
                        }
                        previous_delta[index] = delta;
                        for (metric, delta) in [a.unsigned_abs(), b.unsigned_abs(), (b - a).unsigned_abs()].into_iter().enumerate() {
                            sums[metric] += u64::from(delta);
                            max_delta[metric] = max_delta[metric].max(delta);
                        }
                    }
                    for metric in 0..3 { pops[metric] += usize::from(max_delta[metric] > 40); }
                }
            }
            for metric in 0..4 { oscillation_total[metric] += oscillation[metric]; }
            raw_pop_exceedance_frames += usize::from(frame > 0 && pops[2] * 100 > count);
            let opportunities = if frame >= 2 { count as u64 * 3 } else { 0 };
            opportunities_total += opportunities;
            rows.push((frame, dt, wind, view, count, sums, pops, gpu, oscillation, opportunities));
            if !baseline && frame > 0 {
                if name == "wind-off-static" && sums[2] != 0 {
                    failures.push(format!("{name} frame={frame}: frozen grass residual changed {} RGB codes", sums[2]));
                }
                if msaa == 4 && matches!(name, "static-wind-on" | "micro-camera") && pops[2] * 100 > count {
                    failures.push(format!("{name} frame={frame}: {} raw residual pops exceed 1% of {count} blade pixels at4xMSAA", pops[2]));
                }
            }
            previous = Some(captures);
        }
        let eligible = roi.iter().filter(|&&v| v).count();
        if !baseline && eligible < 1000 { failures.push(format!("{name}: only {eligible} blade-bearing ROI pixels")); }
        let oscillation_fraction = oscillation_total[2] as f64 / oscillation_total[3].max(1) as f64;
        let (reversal_fraction, reversal_rms) = temporal_opportunity_metrics(oscillation_total, opportunities_total);
        if !baseline && matches!(name, "static-wind-on" | "micro-camera") && (reversal_fraction > 0.01 || reversal_rms > 1.0) {
            failures.push(format!("{name}: reversal opportunity fraction={reversal_fraction} (bound0.01), RMS={reversal_rms} codes (bound1.0)"));
        }
        let mut csv = String::from("frame,renderer_dt,wind_x,wind_y,wind_z,camera_x,camera_y,camera_z,roi_pixels,off_rgb_abs_sum,on_rgb_abs_sum,grass_residual_rgb_abs_sum,off_rgb_abs_mean,on_rgb_abs_mean,grass_residual_rgb_abs_mean,off_pixels_gt40,on_pixels_gt40,grass_residual_pixels_gt40,off_gpu_ms,on_gpu_ms,active_channel_pairs,channel_reversals,weighted_reversal_energy,total_delta_pair_energy,weighted_reversal_energy_fraction,channel_opportunities,reversal_opportunity_fraction,reversal_rms_codes\n");
        let mut timings = [Vec::with_capacity(FRAMES), Vec::with_capacity(FRAMES)];
        let mut max_pop = 0;
        for (frame, dt, wind, view, count, sums, pops, gpu, oscillation, opportunities) in rows {
            let denominator = (count * 3).max(1) as f64;
            let (fraction, rms) = temporal_opportunity_metrics(oscillation, opportunities);
            writeln!(csv, "{frame},{dt},{},{},{},{},{},{},{count},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{opportunities},{fraction},{rms}",
                wind[0], wind[1], wind[2], view.pos.x, view.pos.y, view.pos.z,
                sums[0], sums[1], sums[2], sums[0] as f64 / denominator, sums[1] as f64 / denominator, sums[2] as f64 / denominator,
                pops[0], pops[1], pops[2], gpu[0], gpu[1], oscillation[0], oscillation[1], oscillation[2], oscillation[3],
                oscillation[2] as f64 / oscillation[3].max(1) as f64)?;
            max_pop = max_pop.max(pops[2]);
            for variant in 0..2 { timings[variant].push(gpu[variant]); }
        }
        std::fs::write(out.join(format!("{name}.csv")), csv)?;
        sheet.save(out.join(format!("{name}-contact.png")))?;
        let mask: Vec<u8> = roi.iter().flat_map(|&v| if v { [255_u8, 255, 255, 255] } else { [0_u8, 0, 0, 255] }).collect();
        image::save_buffer(out.join(format!("{name}-roi.png")), &mask, size.0, size.1, image::ColorType::Rgba8)?;
        writeln!(manifest, "scenario={name} final_roi_pixels={eligible} max_grass_residual_pixels_gt40={max_pop} off_gpu_ms_p50_p95_p99={:?} on_gpu_ms_p50_p95_p99={:?}",
            percentiles(&mut timings[0]), percentiles(&mut timings[1]))?;
        writeln!(manifest, "scenario={name} raw_pop_1percent_exceedance_frames={raw_pop_exceedance_frames} active_channel_pairs={} channel_reversals={} weighted_reversal_energy={} total_delta_pair_energy={} weighted_reversal_energy_fraction={oscillation_fraction}",
            oscillation_total[0], oscillation_total[1], oscillation_total[2], oscillation_total[3])?;
        writeln!(manifest, "scenario={name} channel_opportunities={opportunities_total} reversal_opportunity_fraction={reversal_fraction} reversal_rms_codes={reversal_rms}")?;
        std::fs::write(out.join("manifest.txt"), &manifest)?;
        for (variant, (off, _)) in pair.iter_mut().enumerate() {
            off.perf_report(&format!("grass-temporal-{name}-variant{variant}"));
        }
    }
    writeln!(manifest, "behavioral_failures={}", failures.len())?;
    for failure in &failures { writeln!(manifest, "failure={failure}")?; }
    std::fs::write(out.join("manifest.txt"), manifest)?;
    ensure!(failures.is_empty(), "temporal grass regression: {}", failures.join("; "));
    Ok(())
}

/// Frame-to-frame reversal energy, with a two-code deadband against quantization.
/// min(|a|,|b|)^2 weights the energy shared by both consecutive deltas, so
/// equal-amplitude alternation is 100%, while a monotonic ramp is exactly zero.
fn temporal_oscillation(a: i16, b: i16) -> [u64; 4] {
    if a.unsigned_abs() <= 2 || b.unsigned_abs() <= 2 { return [0; 4]; }
    let energy = u64::from(a.unsigned_abs().min(b.unsigned_abs())).pow(2);
    let reversal = a.signum() != b.signum();
    [1, u64::from(reversal), if reversal { energy } else { 0 }, energy]
}

fn temporal_opportunity_metrics(oscillation: [u64; 4], opportunities: u64) -> (f64, f64) {
    let denominator = opportunities.max(1) as f64;
    (oscillation[1] as f64 / denominator, (oscillation[2] as f64 / denominator).sqrt())
}

#[test]
fn grass_temporal_oscillation_distinguishes_ramp_from_alternation() {
    let integrate = |deltas: &[i16]| {
        deltas.windows(2).fold([0_u64; 4], |mut total, pair| {
            let sample = temporal_oscillation(pair[0], pair[1]);
            for metric in 0..4 { total[metric] += sample[metric]; }
            total
        })
    };
    assert_eq!(integrate(&[5, 5, 5, 5]), [3, 0, 0, 75]);
    assert_eq!(integrate(&[5, -5, 5, -5]), [3, 3, 75, 75]);
    assert_eq!(integrate(&[2, -2, 2, -2]), [0; 4]);
    assert_eq!(temporal_opportunity_metrics(integrate(&[5, 5, 5, 5]), 3), (0.0, 0.0));
    assert_eq!(temporal_opportunity_metrics(integrate(&[5, -5, 5, -5]), 3), (1.0, 5.0));
    // Quiet channels are opportunities too: do not normalize away the absolute amplitude.
    assert_eq!(temporal_opportunity_metrics(integrate(&[5, -5, 5, -5]), 300), (0.01, 0.5));
}

fn run(out: &std::path::Path) -> anyhow::Result<()> {
    let selected = std::env::var("AOMAC_GFX_VIEW").ok();
    let png_only = std::env::var("AOMAC_GFX_PNG_ONLY").is_ok_and(|value| value == "1");
    let capture_only = std::env::var("AOMAC_GFX_CAPTURE_ONLY").is_ok_and(|value| value == "1");
    let assert_specks = !std::env::var("AOMAC_GFX_ASSERT_SPECKS").is_ok_and(|value| value == "0");
    // AOMAC_GFX_DENSITY: grass density (default 1); AOMAC_GFX_EFFECTS=1: sun/contact shadows, AO and bloom in both variants.
    // AOMAC_GFX_HDR=1: the HDR present; AOMAC_GFX_MSAA: requested samples (default 1).
    // AOMAC_GFX_DISTANCE: grass distance (settings default); AOMAC_GFX_DAY_TIME: Newland day time only (default 3240).
    let density = std::env::var("AOMAC_GFX_DENSITY").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
    let effects = std::env::var("AOMAC_GFX_EFFECTS").is_ok_and(|value| value == "1");
    let effect_override = |key| std::env::var(key).map_or(effects, |value| value != "0");
    let ao = effect_override("AOMAC_GFX_AO");
    let shadows = effect_override("AOMAC_GFX_SHADOWS");
    let contact = effect_override("AOMAC_GFX_CONTACT");
    let hdr = std::env::var("AOMAC_GFX_HDR").is_ok_and(|value| value == "1");
    let msaa = std::env::var("AOMAC_GFX_MSAA").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
    let grass_distance = std::env::var("AOMAC_GFX_DISTANCE").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or_else(|| EnhancedSettings::default().grass_distance);
    let newland_day_time = std::env::var("AOMAC_GFX_DAY_TIME").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(3240.0);
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let actor = graphics_fixture_actor()?;
    let weather = ao_formats::weather::State::clear();
    let size = (1920, 1200);
    let mut changed_views = 0;
    let mut negative_views = 0;
    let mut positive_views = 0;
    let measured_frames = if capture_only { 0 } else { 600 };
    let timing = if capture_only { "unexercised (capture only)" } else { "FrameGpuTimer whole-frame neutral Enhanced UNORM versus grass incremental contribution, not isolated draw timestamp" };
    let mut report = format!("fixture=graphics_offline_fixture\nsize=1920x1200\nrenderer_time=0\ndt=0\ncrt_seed=1\nweather=clear\nmsaa_requested={msaa}\nanisotropy=1\nwarmup_frames=320\nmeasured_frames={measured_frames}\ntiming={timing}\nvisual_acceptance=unexercised\n");
    writeln!(report, "selected_view={selected:?}\npng_only={png_only}\ncapture_only={capture_only}\nassert_specks={assert_specks}\nao={ao}\nsun_shadows={shadows}\ncontact_shadows={contact}")?;
    for (key, program, args) in [
        ("revision", "git", vec!["rev-parse".into(), "HEAD".into()]),
        ("resource_sha256", "shasum", vec!["-a".into(), "256".into(), dir.join("cd_image/rdb.db").into_os_string()]),
        ("source_diff", "git", vec!["diff".into(), "--no-ext-diff".into(), "HEAD".into()]),
    ] {
        let output = std::process::Command::new(program).args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
        ensure!(output.status.success(), "{program} failed");
        writeln!(report, "{key}={:?}", String::from_utf8(output.stdout)?.trim())?;
    }
    let mut views = Vec::new();
    for (name, pf, time, reference) in [
        ("icc-grass-ground", 4582, 2648.69, ao_render::Vec3::new(933.5, 26.0, -748.0)),
        ("newland-grass-ground", 566, 3240.0, ao_render::Vec3::new(0.0, 0.0, 0.0)),
        ("borealis-grass-ground", 800, 3240.0, ao_render::Vec3::new(652.0, 65.3, -420.0)),
    ] {
        if selected.as_deref().is_some_and(|view| !view.starts_with(name)) { continue; }
        ao_scene::crt_srand(1);
        // Keep Newland's original scene time for the exact same reference/autopicked camera.
        let camera_time = if pf == 566 { 4700.0 } else { time };
        let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, camera_time, 0, Some(weather))?;
        let surface = Rc::new(RefCell::new(ao_formats::playfield::collision::Collision::load(&store, pf)?));
        let exposure = super::super::player::grass_exposure_surface(surface.clone());
        // A zero reference means the middle of the playfield's ground.
        let reference = match scene.ground_detail.first() {
            Some(patch) if reference == ao_render::Vec3::ZERO => ao_render::Vec3::new(
                (patch.origin[0] as f32 + patch.cells as f32 * 0.5) * patch.cell_size, 0.0, -(patch.origin[1] as f32 + patch.cells as f32 * 0.5) * patch.cell_size),
            _ => reference,
        };
        // Pin the pre-cutover ICC camera: exposure admission changes must not move the comparison.
        let (fixed, tile, grid) = if pf == 4582 {
            (ao_render::Camera { pos: ao_render::Vec3::new(938.0, 25.7, -750.0), yaw: 0.0, pitch: -0.18534796, roll: 0.0 }, 258, [234, 189])
        } else {
            grass_ground_camera(&scene, &surface.borrow(), &exposure, reference)
                .with_context(|| format!("finding exposed grass in {name}"))?
        };
        writeln!(report, "grass_view={name} pf={pf} native_tile={tile} grid={grid:?} camera={fixed:?}")?;
        // The same ground from further back and higher: mid and far fields, thinning towards the distance.
        let forward = fixed.forward();
        let flat = ao_render::Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let target = fixed.pos + forward * (8.0 / ao_render::Vec3::new(forward.x, 0.0, forward.z).length().max(0.1));
        for (distance, lift, tag) in [(0.0, 0.0, ""), (14.0, 1.5, "mid-"), (30.0, 6.0, "far-")] {
            let mut eye = fixed.pos - flat * distance;
            if distance > 0.0 {
                // Ground below the original eye's height: never a roof or a bridge above the field.
                let ground = [-2.0, 0.0, 2.0].into_iter().flat_map(|x| [-2.0, 0.0, 2.0].into_iter().map(move |z| [eye.x + x, fixed.pos.y + 3.0, eye.z + z]))
                    .filter_map(|point| surface.borrow().ground(point)).max_by(f32::total_cmp);
                eye.y = ground.unwrap_or(fixed.pos.y - 1.7) + 1.7 + lift;
            }
            let view = if distance > 0.0 { ao_render::Camera::look_at(eye, target) } else { fixed };
            let day_time = if pf == 566 { newland_day_time } else { time };
            for (light, light_time) in [("day", day_time), ("dusk", 4860.0)].into_iter().chain((pf == 800).then_some(("night", 0.0))) {
                views.push((format!("{name}-{tag}{light}"), pf, light_time, view));
            }
        }
    }
    views.extend(graphics_fixture_views(&actor));
    for (name, pos) in [
        ("icc-coastal1", ao_render::Vec3::new(660.0, 24.3, -714.0)),
        ("icc-coastal2", ao_render::Vec3::new(700.0, 24.0, -710.0)),
    ] {
        views.push((name.into(), 4582, 2648.69, ao_render::Camera { pos, yaw: 0.0, pitch: -0.3, roll: 0.0 }));
    }
    views.retain(|(name, _, _, _)| !matches!(name.as_str(), "icc-actor-authored" | "borealis-lightbar-night" | "borealis-lightbar-night-close"));
    if let Some(name) = &selected {
        views.retain(|(view, _, _, _)| view == name);
        ensure!(!views.is_empty(), "unknown AOMAC_GFX_VIEW {name:?}");
    }
    std::fs::write(out.join("manifest.txt"), &report)?;
    for (name, pf, time, fixed) in views {
        positive_views += usize::from(pf != 127 && name != "icc-platform-authored");
        ao_scene::crt_srand(1);
        let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
        let exposure = super::super::player::grass_exposure_surface(Rc::new(RefCell::new(
            ao_formats::playfield::collision::Collision::load(&store, pf)?)));
        let mut medians = [0.0; 2];
        let mut baseline_raw = None;
        let mut allocated = [None; 2];
        // Repeat opposite ordering to expose simple thermal/order drift.
        for (round, order) in [[0usize, 1], [1, 0]].into_iter().take(if capture_only { 1 } else { 2 }).enumerate() {
            for variant in order {
                ao_scene::crt_srand(1);
                let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
                lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
                let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
                let mut off = Offscreen::evidence_timing(&shot, size)?;
                off.fixed_camera(Some(fixed));
                off.host.msaa_samples = Some(msaa);
                off.host.anisotropy = Some(1);
                off.host.fog_density_scale = Some(1.0);
                off.host.grass_exposure = Some(exposure.clone());
                off.host.grass_wind = weather.wind;
                let settings = EnhancedSettings { sun_shadows: shadows, contact_shadows: contact, ambient_occlusion: ao, emissive_bloom: effects, hdr, ground_grass: true, grass_distance, grass_density: if variant == 0 { 0.0 } else { density }, ..Default::default() };
                off.host.enhanced_settings = Some(settings);
                // The extended field can need more than 320 streamed exposure frames.
                // Record its up-front cost separately; the paired GPU measurement is stationary/warmed.
                off.frame(&mut shot, 0.0);
                let warm_started = std::time::Instant::now();
                off.warm_grass_tiles();
                writeln!(report, "explicit_grass_warm_wall_ms={}", warm_started.elapsed().as_secs_f64() * 1000.0)?;
                if !capture_only { off.set_gpu_timing(true)?; }
                let mut gpu = Vec::with_capacity(measured_frames);
                let mut prepare = Vec::with_capacity(measured_frames);
                let mut generation = Vec::with_capacity(320);
                let mut list = Default::default();
                for frame in 0..320 + measured_frames {
                    list = off.frame(&mut shot, 0.0);
                    let ms = if capture_only { None } else { Some(off.read_frame_gpu_ms()?.context("GPU timestamps unavailable")?) };
                    if frame >= 320 {
                        gpu.push(ms.context("measured frame missing GPU timestamp")?);
                        prepare.push(off.evidence_grass_metrics().1 as f64 / 1_000_000.0);
                    }
                    else { generation.push(off.evidence_grass_metrics().1 as f64 / 1_000_000.0); }
                }
                if !capture_only { off.set_gpu_timing(false)?; }
                let (bytes, _, cells) = off.evidence_grass_metrics();
                let generation_mean = generation.iter().sum::<f64>() / generation.len() as f64;
                let generation_max = generation.iter().copied().fold(0.0, f64::max);
                let generation_stats = percentiles(&mut generation);
                let timing_stats = if capture_only { None } else {
                    let gpu_stats = percentiles(&mut gpu);
                    let prepare_stats = percentiles(&mut prepare);
                    medians[variant] = gpu_stats.0;
                    Some((gpu_stats, prepare_stats))
                };
                allocated[variant] = off.gpu_allocated_bytes();
                let max_blade_candidates = cells * (1536.0 * settings.clamped().grass_density / 2.0).round().max(1.0) as usize * variant;
                writeln!(report, "view={name} pf={pf} time={time} camera={fixed:?} lens={lens:?} settings={settings:?} round={round} variant={variant} effect_gpu_payload_bytes={bytes} cells={cells} max_blade_candidates={max_blade_candidates}")?;
                if let Some((gpu_stats, prepare_stats)) = timing_stats {
                    writeln!(report, "gpu_ms_p50_p95_p99={gpu_stats:?} prepare_wall_ms_p50_p95_p99={prepare_stats:?}")?;
                } else {
                    writeln!(report, "gpu_timing=unexercised prepare_measured_timing=unexercised")?;
                }
                writeln!(report, "generation_wall_ms_p50_p95_p99={generation_stats:?} generation_mean_ms={generation_mean} generation_max_ms={generation_max}")?;
                writeln!(report, "native_metal_allocated_bytes={:?} shared_enhanced_target_bytes={}", allocated[variant], off.enhanced_target_bytes())?;
                if round == 0 && variant == 1 { writeln!(report, "grass_diagnostics_view={name}\n{}", off.evidence_grass_diagnostics())?; }
                std::fs::write(out.join("manifest.txt"), &report)?;
                if let Some((_, prepare_stats)) = timing_stats {
                    ensure!(variant == 0 || prepare_stats.1 <= 0.2, "{name}: grass generation/prepare p95 exceeds 0.2 ms: {prepare_stats:?}");
                }
                ensure!(variant == 0 || generation_mean <= 0.2, "{name}: amortised grass generation exceeds 0.2 ms: {generation_mean}");
                if round == 0 {
                    let raw = off.rgba(&shot, &list)?;
                    if !png_only { std::fs::write(out.join(format!("{name}-{variant}.rgba")), &raw)?; }
                    image::save_buffer(out.join(format!("{name}-{variant}.png")), &raw, size.0, size.1, image::ColorType::Rgba8)?;
                    if variant == 0 { baseline_raw = Some(raw); }
                    else {
                        let baseline = baseline_raw.as_ref().context("neutral capture missing")?;
                        let differing = raw.iter().zip(baseline).filter(|(a, b)| a != b).count();
                        writeln!(report, "view={name} neutral_vs_grass_differing_bytes={differing}")?;
                        if name == "borealis-grass-ground-far-day" {
                            // The retained AFTER detail places this rectangle on the far green hills.
                            // Allow new isolated dark pixels in at most 0.05% of this run's green area.
                            // This permits real blade edges, not the old carpet's 3.4% pepper noise.
                            let specks = dark_specks(baseline, &raw, size.0 as usize, size.1 as usize, [550, 300, 1400, 600]);
                            let tolerance = specks.0.div_ceil(2000).max(8);
                            writeln!(report, "view={name} speck_roi=[550,300,1400,600] speck_window=7x7 speck_luma=rec709_encoded_rgb speck_ratio=0.6 speck_drop=0.06 speck_green=40_of_49_paired_off_g_gt_1.15r_and_1.15b eligible_pixels={} variant=0 dark_outliers={} variant=1 dark_outliers={} new_dark_outliers={} tolerance={tolerance} speck_acceptance={}", specks.0, specks.1, specks.2, specks.3, if assert_specks { "enforced" } else { "unexercised_ablation" })?;
                            std::fs::write(out.join("manifest.txt"), &report)?;
                            ensure!(specks.0 > 100_000, "{name}: paired green terrain mask is empty or camera/palette drifted: {specks:?}");
                            if assert_specks {
                                ensure!(specks.3 <= tolerance, "{name}: new grass dark specks exceed 0.05% of eligible pixels ({tolerance}): {specks:?}");
                            }
                        }
                        if differing > 0 { changed_views += 1; }
                        if pf == 127 { ensure!(cells == 0 && differing == 0, "indoor grass changed the image"); }
                        if pf == 127 || name == "icc-platform-authored" {
                            ensure!(differing == 0, "{name}: grass leaked into a negative-control floor view");
                            negative_views += 1;
                        }
                    }
                }
            }
            if !capture_only {
                writeln!(report, "view={name} round={round} incremental_grass_gpu_frame_p50_ms={}", medians[1] - medians[0])?;
            }
            if let [Some(before), Some(after)] = allocated {
                writeln!(report, "view={name} round={round} native_metal_allocated_delta_bytes={}", after as i128 - before as i128)?;
            }
            std::fs::write(out.join("manifest.txt"), &report)?;
        }
    }
    writeln!(report, "changed_views={changed_views}\nnegative_views={negative_views}\npositive_views={positive_views}")?;
    ensure!(changed_views > 0 || (positive_views == 0 && negative_views > 0), "fixture has no visible grass contribution or verified negative-only selection");
    std::fs::write(out.join("manifest.txt"), &report)?;
    Ok(())
}

/// Paired 7x7 median test in encoded PNG RGB, not a linear-light radiometry claim.
/// The retained far AFTER image locates this ROI on hills; a 40/49 green-neighbour
/// mask from grass-OFF rejects rocks, structures, water and their silhouette edges.
/// Count broad shadows only if they contain a genuinely isolated dark outlier.
fn dark_specks(
    baseline: &[u8],
    grass: &[u8],
    width: usize,
    height: usize,
    roi: [usize; 4],
) -> (usize, usize, usize, usize) {
    assert_eq!(baseline.len(), width * height * 4);
    assert_eq!(grass.len(), baseline.len());
    assert!(roi[0] >= 3 && roi[1] >= 3 && roi[2] + 3 <= width && roi[3] + 3 <= height);
    let luma = |rgb: &[u8]| (0.2126 * rgb[0] as f32 + 0.7152 * rgb[1] as f32 + 0.0722 * rgb[2] as f32) / 255.0;
    let mut counts = (0, 0, 0, 0);
    for y in roi[1]..roi[3] {
        for x in roi[0]..roi[2] {
            let mut off_luma = [0.0; 49];
            let mut on_luma = [0.0; 49];
            let mut green = 0;
            let mut sample = 0;
            for ny in y - 3..=y + 3 {
                for nx in x - 3..=x + 3 {
                    let offset = (ny * width + nx) * 4;
                    let rgb = &baseline[offset..offset + 3];
                    green += usize::from(rgb[1] as f32 > 1.15 * rgb[0] as f32 && rgb[1] as f32 > 1.15 * rgb[2] as f32);
                    off_luma[sample] = luma(rgb);
                    on_luma[sample] = luma(&grass[offset..offset + 3]);
                    sample += 1;
                }
            }
            if green < 40 { continue; }
            let offset = (y * width + x) * 4;
            let off_median = *off_luma.select_nth_unstable_by(24, f32::total_cmp).1;
            let on_median = *on_luma.select_nth_unstable_by(24, f32::total_cmp).1;
            let is_outlier = |value: f32, median: f32| value < median * 0.6 && median - value > 0.06;
            let off_dark = is_outlier(luma(&baseline[offset..offset + 3]), off_median);
            let on_dark = is_outlier(luma(&grass[offset..offset + 3]), on_median);
            counts.0 += 1;
            counts.1 += usize::from(off_dark);
            counts.2 += usize::from(on_dark);
            counts.3 += usize::from(on_dark && !off_dark);
        }
    }
    counts
}

#[test]
fn grass_dark_specks_reject_uniform_shadow() {
    let plane = [90, 180, 70, 255].repeat(32 * 32);
    let mut speck = plane.clone();
    speck[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 3].fill(0);
    let roi = [3, 3, 29, 29];
    assert_eq!(dark_specks(&plane, &plane, 32, 32, roi), (676, 0, 0, 0));
    assert_eq!(dark_specks(&plane, &speck, 32, 32, roi), (676, 0, 1, 1));
    let shadow = [27, 54, 21, 255].repeat(32 * 32);
    assert_eq!(dark_specks(&plane, &shadow, 32, 32, roi), (676, 0, 0, 0));
    assert_eq!(dark_specks(&speck, &speck, 32, 32, roi), (676, 1, 1, 0));
}

fn percentiles(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_unstable_by(f64::total_cmp);
    let at = |percent: usize| samples[(samples.len() * percent).div_ceil(100).saturating_sub(1)];
    (at(50), at(95), at(99))
}

struct GrassFixtureCell {
    root: ao_render::Vec3,
    samples: [ao_render::Vec3; 5],
    tile: u16,
    size: f32,
}

/// Selects an exposed native grass cluster, never a texture-colour heuristic.
/// The chosen native grid/tile and exact fixed camera are retained in the manifest.
fn grass_ground_camera(
    scene: &ao_scene::Scene,
    surface: &ao_formats::playfield::collision::Collision,
    exposure: &ao_render::GrassExposure,
    reference: ao_render::Vec3,
) -> anyhow::Result<(ao_render::Camera, u16, [i32; 2])> {
    use ao_render::Vec3;
    let mut cells = std::collections::BTreeMap::new();
    for patch in &scene.ground_detail {
        for (index, cell) in patch.fine_cells.iter().enumerate() {
            if !matches!(cell.ground_material, 6 | 15) { continue; }
            let (x, z) = (index % patch.cells, index / patch.cells);
            let grid = [patch.origin[0] + x as i32, patch.origin[1] + z as i32];
            let (wx, wz) = (grid[0] as f32 * patch.cell_size, -(grid[1] as f32) * patch.cell_size);
            let offset = z * (patch.cells + 1) + x;
            let h = [offset, offset + 1, offset + patch.cells + 1, offset + patch.cells + 2]
                .map(|i| patch.heights[i] as f32 * (patch.height_scale / 256.0));
            if h.into_iter().fold(f32::NEG_INFINITY, f32::max) - h.into_iter().fold(f32::INFINITY, f32::min) > patch.cell_size * 0.8 { continue; }
            let height = if (!grid[1] ^ grid[0]) & 1 == 0 { (h[1] + h[2]) * 0.5 } else { (h[0] + h[3]) * 0.5 };
            let root = Vec3::new(wx + patch.cell_size * 0.5, height, wz - patch.cell_size * 0.5);
            let samples = [Vec3::new(wx,h[0],wz), Vec3::new(wx+patch.cell_size,h[1],wz),
                Vec3::new(wx,h[2],wz-patch.cell_size), Vec3::new(wx+patch.cell_size,h[3],wz-patch.cell_size), root];
            cells.insert(grid, GrassFixtureCell { root, samples, tile: cell.native_tile, size: patch.cell_size });
        }
    }
    let mut best: Option<(usize, f32, ao_render::Camera, u16, [i32; 2])> = None;
    for (&grid, cell) in &cells {
        let distance = cell.root.distance_squared(reference);
        let neighbors = (-1..=1).flat_map(|z| (-1..=1).map(move |x| [grid[0]+x, grid[1]+z]))
            .filter(|neighbor| cells.contains_key(neighbor)).count();
        if best.as_ref().is_some_and(|old| neighbors < old.0 || (neighbors == old.0 && distance >= old.1)) { continue; }
        if !cell.samples.iter().all(|point| (exposure.query)(point.to_array()) && matches!(surface.ground_material(point.to_array()), 6 | 15)) { continue; }
        let full_radius = 0.72 * 50.0f32.min((44.0 / 3.0 - 3.0) * cell.size).min(12.5 * cell.size);
        let offset = 8.0f32.min(full_radius * 0.6);
        for direction in [Vec3::Z, Vec3::X, -Vec3::Z, -Vec3::X] {
            let mut eye = cell.root + direction * offset;
            let ground = [-2.0, 0.0, 2.0].into_iter().flat_map(|x| [-2.0, 0.0, 2.0].into_iter().map(move |z| [eye.x + x, eye.y + 15.0, eye.z + z]))
                .filter_map(|point| surface.ground(point)).max_by(f32::total_cmp);
            let Some(ground) = ground else { continue; };
            eye.y = ground + 1.7;
            if eye.distance(cell.root) >= full_radius { continue; }
            let at = cell.root + Vec3::Y * 0.2;
            if !super::super::player::segment_clear(surface, eye.to_array(), at.to_array()) { continue; }
            best = Some((neighbors, distance, ao_render::Camera::look_at(eye, at), cell.tile, grid));
            break;
        }
    }
    best.map(|(_, _, camera, tile, grid)| (camera, tile, grid)).context("no exposed native grass with a clear close camera")
}
