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

fn grass_feature_settings(value: &str) -> anyhow::Result<EnhancedSettings> {
    ensure!(matches!(value, "none" | "all" | "tufts" | "species" | "path_thinning" | "gust_bands" | "sunlight" | "sun_shadows" | "point_shadows"),
        "unknown AOMAC_GFX_GRASS_FEATURE {value:?}; expected none, all, tufts, species, path_thinning, gust_bands, sunlight, sun_shadows or point_shadows");
    Ok(EnhancedSettings {
        grass_tufts: matches!(value, "all" | "tufts"),
        grass_species: matches!(value, "all" | "species"),
        grass_path_thinning: matches!(value, "all" | "path_thinning"),
        grass_gust_bands: matches!(value, "all" | "gust_bands"),
        grass_sunlight: matches!(value, "all" | "sunlight"),
        grass_sun_shadows: matches!(value, "all" | "sun_shadows"),
        grass_point_shadows: matches!(value, "all" | "point_shadows"),
        ..Default::default()
    })
}

#[test]
fn grass_feature_selection_is_explicit() -> anyhow::Result<()> {
    let flags = |s: EnhancedSettings| [s.grass_tufts, s.grass_species, s.grass_path_thinning,
        s.grass_gust_bands, s.grass_sunlight, s.grass_sun_shadows, s.grass_point_shadows];
    assert_eq!(flags(grass_feature_settings("none")?), [false; 7]);
    assert_eq!(flags(grass_feature_settings("all")?), [true; 7]);
    for (index, name) in ["tufts", "species", "path_thinning", "gust_bands", "sunlight", "sun_shadows", "point_shadows"].into_iter().enumerate() {
        let mut expected = [false; 7];
        expected[index] = true;
        assert_eq!(flags(grass_feature_settings(name)?), expected, "{name}");
        let settings = grass_feature_settings(name)?;
        assert!(!settings.clustered_lighting && !settings.local_light_shadows);
    }
    for invalid in ["", "All", "tufts,species", "unknown"] {
        assert!(grass_feature_settings(invalid).is_err(), "{invalid:?}");
    }
    Ok(())
}

fn compare_grass_features(feature: &str) -> anyhow::Result<bool> {
    let value = std::env::var("AOMAC_GFX_GRASS_COMPARE").unwrap_or_else(|_| "density".into());
    ensure!(matches!(value.as_str(), "density" | "features"), "AOMAC_GFX_GRASS_COMPARE must be density or features");
    ensure!(value != "features" || feature != "none", "features comparison requires AOMAC_GFX_GRASS_FEATURE other than none");
    Ok(value == "features")
}

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
    let strip = std::env::var("AOMAC_GFX_TEMPORAL_STRIP").is_ok_and(|value| value == "1");
    let dt_step = if strip { 0.1 } else { 1.0 / 60.0 };
    let baseline = strip || std::env::var("AOMAC_GFX_TEMPORAL_BASELINE").is_ok_and(|value| value == "1");
    let feature = std::env::var("AOMAC_GFX_GRASS_FEATURE").unwrap_or_else(|_| "none".into());
    let feature_settings = grass_feature_settings(&feature)?;
    let compare_features = compare_grass_features(&feature)?;
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
        "fixture=grass_offline_temporal_stability\nbaseline={baseline}\nsize={}x{}\nframes={FRAMES}\nfixed_dt={dt_step}\nplayfield=4582\nnative_tile=258\ngrid=[234,189]\ncamera={fixed:?}\nlens={lens:?}\nday={day}\nday_time={day_time}\nscene_weather=clear_fixed\nwind=live_native_Weather_open_weather_update_fixed_day_time_seed1\nwarmup_frames=320\nwarmup_dt=0\nrenderer_initial_time=0\nmsaa={msaa}\nanisotropy=1\nhdr=false\nao=false\nbloom=false\ntiming=FrameGpuTimer_whole_frame_not_isolated_grass\nwind_off_static=frozen_clock_dt0_constant_zero_wind_unchanged_static_deformation_not_zero_bend\nroi=union_of_paired_on_off_RGB_difference_gt8_on_green_OFF_terrain_lower_two_thirds\nrgb_delta=absolute_encoded_RGB_codes_no_alpha\npop=any_RGB_channel_delta_gt40\nresidual_delta=delta_of_signed_ON_minus_OFF_RGB\nraw_pop_bound=1_percent_blade_ROI_per_frame_static_micro_enforced_at_MSAA4_only\noscillation=consecutive_residual_channel_deltas_both_abs_gt2_sign_reversal\noscillation_weight=min_abs_consecutive_delta_squared\nactive_energy_fraction=reversal_weight_sum_over_qualifying_pair_weight_sum_diagnostic_only_amplitude_normalized_not_acceptance\nopportunities=all_ROI_RGB_channels_each_frame_index_ge2_including_inactive_channels\nstatic_micro_reversal_opportunity_fraction_bound=0.01\nstatic_micro_reversal_RMS_codes_bound=1.0\nreversal_RMS=sqrt_weighted_reversal_energy_over_all_channel_opportunities\nbound_rationale=at_most_1_percent_of_all_blade_channel_opportunities_reverse_and_RMS_reversal_energy_at_most_one_encoded_code\nmicro_camera=0.25mm_lateral_per_frame\nslow_walk=0.25m_per_second_forward\ncontact_sheet=columns_frames_0_60_119_rows_baseline_selected\nvisual_acceptance=unexercised\n",
        size.0, size.1);
    writeln!(manifest, "grass_feature={feature}\ncompare_features={compare_features}\nstrip={strip}\ncontact_sheet_times_seconds={:?}\nsun_shadows={}", [dt_step, 61.0 * dt_step, 120.0 * dt_step], feature_settings.grass_sun_shadows)?;
    if strip {
        writeln!(manifest, "sparse_png_pairs=12\nsparse_png_frame_stride=10\nsparse_png_first_time=0.1\nsparse_png_last_time=11.1\nsparse_png_retention=scratch_only_delete_after_animation")?;
    }
    if let Some(shader_dir) = std::env::var_os("AOMAC_SHADER_DIR") {
        writeln!(manifest, "shader_dir={}", PathBuf::from(shader_dir).display())?;
    }
    std::fs::write(out.join("manifest.txt"), &manifest)?;
    let mut failures = Vec::new();
    let scenarios: &[&str] = if strip { &["static-wind-on"] } else { &["static-wind-on", "wind-off-static", "micro-camera", "slow-walk"] };
    for &name in scenarios {
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
                ground_grass: true, grass_density: if grass || compare_features { 1.0 } else { 0.0 },
                sun_shadows: feature_settings.grass_sun_shadows,
                ..if grass { feature_settings } else { EnhancedSettings::default() }
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
            let dt = if name == "wind-off-static" { 0.0 } else { dt_step };
            weather.update(day, f64::from(day_time), dt_step);
            let wind = if name == "wind-off-static" { [0.0; 3] } else { weather.state().wind };
            let mut view = fixed;
            if name == "micro-camera" {
                view.pos.x += frame as f32 * 0.00025;
            } else if name == "slow-walk" {
                let forward = fixed.forward();
                view.pos += Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero() * (frame as f32 * dt_step * 0.25);
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
            if strip && frame % 10 == 0 {
                for (variant, raw) in captures.iter().enumerate() {
                    image::save_buffer(out.join(format!("{name}-variant{variant}-frame{frame:03}.png")),
                        raw, size.0, size.1, image::ColorType::Rgba8)?;
                }
            }
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
    let feature = std::env::var("AOMAC_GFX_GRASS_FEATURE").unwrap_or_else(|_| "none".into());
    let feature_settings = grass_feature_settings(&feature)?;
    let compare_features = compare_grass_features(&feature)?;
    let png_only = std::env::var("AOMAC_GFX_PNG_ONLY").is_ok_and(|value| value == "1");
    let capture_only = std::env::var("AOMAC_GFX_CAPTURE_ONLY").is_ok_and(|value| value == "1");
    let shadow_timing = !capture_only && std::env::var("AOMAC_GFX_GRASS_SHADOW_TIMING").is_ok_and(|value| value == "1");
    let shadow_stage = std::env::var("AOMAC_GFX_GRASS_SHADOW_STAGE").unwrap_or_else(|_| "all".into());
    ensure!(matches!(shadow_stage.as_str(), "all" | "admission" | "light_pool" | "grass_roots" | "point_maps" | "sun_maps" | "grass_sun"),
        "AOMAC_GFX_GRASS_SHADOW_STAGE must be all, admission, light_pool, grass_roots, point_maps, sun_maps or grass_sun");
    let assert_specks = !compare_features && !std::env::var("AOMAC_GFX_ASSERT_SPECKS").is_ok_and(|value| value == "0");
    // AOMAC_GFX_DENSITY: grass density (default 1); AOMAC_GFX_EFFECTS=1: sun/contact shadows, AO and bloom in both variants.
    // AOMAC_GFX_HDR=1: the HDR present; AOMAC_GFX_MSAA: requested samples (default 1).
    // AOMAC_GFX_DISTANCE: grass distance (settings default); AOMAC_GFX_DAY_TIME: Newland day time only (default 3240).
    let density = std::env::var("AOMAC_GFX_DENSITY").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
    let effects = std::env::var("AOMAC_GFX_EFFECTS").is_ok_and(|value| value == "1");
    let effect_override = |key| std::env::var(key).map_or(effects, |value| value != "0");
    let ao = effect_override("AOMAC_GFX_AO");
    let shadows = effect_override("AOMAC_GFX_SHADOWS") || feature_settings.grass_sun_shadows;
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
    let timing = if capture_only { "unexercised (capture only)" } else { "FrameGpuTimer whole-frame paired selected-minus-baseline incremental contribution, not isolated draw timestamp" };
    let mut report = format!("fixture=graphics_offline_fixture\nsize=1920x1200\nrenderer_time=0\ndt=0\ncrt_seed=1\nweather=clear\nmsaa_requested={msaa}\nanisotropy=1\nwarmup_frames=320\nmeasured_frames={measured_frames}\ntiming={timing}\nvisual_acceptance=unexercised\n");
    writeln!(report, "selected_view={selected:?}\npng_only={png_only}\ncapture_only={capture_only}\nassert_specks={assert_specks}\nao={ao}\nsun_shadows={shadows}\ncontact_shadows={contact}")?;
    writeln!(report, "grass_feature={feature}\ncompare_features={compare_features}\ncomparison={}\nvariant_order=off_on_on_off", if compare_features { "plain_grass_vs_selected_grass_features" } else { "density_zero_vs_selected_grass" })?;
    writeln!(report, "shadow_gpu_timing={shadow_timing}\nshadow_samples_per_variant={}\nshadow_timing_phase=after_whole_frame_samples_same_warmed_renderer_dt0\nshadow_span=shared_native_and_grass_shadow_passes_not_isolated_grass\ncsv_sample_index=independent_whole_frame_and_shadow_phase_samples_not_simultaneous\nmissing_shadow_timestamp=blank_csv_and_None_summary_not_measured_zero", if shadow_timing { measured_frames } else { 0 })?;
    writeln!(report, "shadow_gpu_stage={shadow_stage}\nshadow_all_stage_scope=admission_light_pool_grass_roots_point_maps_sun_maps_grass_sun")?;
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
    let mut evidence_roots = std::collections::HashMap::new();
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
        // Additional evidence views are explicit opt-ins; never move the canonical budget camera.
        let special = selected.as_deref().and_then(|view| view.strip_prefix("newland-grass-ground-"))
            .filter(|mode| matches!(*mode, "backlit-dusk" | "path-day" | "path-dusk" | "lightbar-night"));
        if let Some(mode) = special.filter(|_| name == "newland-grass-ground") {
            let light_time = match mode { "backlit-dusk" | "path-dusk" => 4600.0, "lightbar-night" => 0.0, _ => newland_day_time };
            ao_scene::crt_srand(1);
            let (lit_scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
                &store, &dir, pf, light_time, 0, Some(weather))?;
            if matches!(mode, "backlit-dusk" | "path-dusk") {
                let env = lit_scene.environment.context("low-sun grass evidence has no native environment")?;
                ensure!(env.sun_dir[1] > 0.0 && env.sun_color.iter().any(|&channel| channel > 0.0),
                    "low-sun grass evidence requires an above-horizon native sun with positive direct energy");
            }
            let (fixed, tile, grid, root) = if mode == "lightbar-night" {
                let appearance = super::graphics_fixture_lightbar_appearance(&actor);
                let mut avatar = super::super::avatar::Avatar::new(&store, &dir, 25988, &appearance,
                    ao_formats::character::actor::ActorAssets::new(&store)?)?;
                avatar.set_transform([0.0; 3], 0.0);
                let light = avatar.lights().next().context("native LightBar fixture has no mounted light")?;
                let (camera, tile, grid, proof) = grass_lightbar_camera(&lit_scene, &surface.borrow(), &exposure, reference, light)?;
                avatar.set_transform(proof.owner.to_array(), 0.0);
                let transformed = avatar.lights().next().context("transformed LightBar lost its source")?;
                ensure!(ao_render::Vec3::from(transformed.pos).distance(proof.source) < 0.001,
                    "LightBar proof disagrees with actual native bone transform");
                writeln!(report, "lightbar_owner={:?} yaw=0 native_source={:?} native_rgb={:?} native_range={} shadow_receiver={:?} lit_receiver={:?} caster_instance={} caster_mesh={} caster_submesh={} caster_triangle={} collision_segment={:?}->{:?} collision_blocked=true opaque_scene_triangle_intersection=true final_camera_owner_body_feet_receiver_los=true",
                    proof.owner, transformed.pos, transformed.color, transformed.range, proof.receiver, proof.lit_receiver,
                    proof.caster[0], proof.caster[1], proof.caster[2], proof.caster[3], proof.source, proof.receiver)?;
                (camera, tile, grid, proof.owner)
            } else {
                grass_ground_camera(&lit_scene, &surface.borrow(), &exposure, reference, mode)?
            };
            let view_name = format!("{name}-{mode}");
            writeln!(report, "grass_evidence_view={view_name} pf={pf} time={light_time} native_tile={tile} grid={grid:?} root={root:?} camera={fixed:?} sun_dir={:?} sun_alignment={:?}",
                lit_scene.environment.map(|env| env.sun_dir), lit_scene.environment.map(|env| fixed.forward().dot(ao_render::Vec3::from(env.sun_dir).normalize_or_zero())))?;
            evidence_roots.insert(view_name.clone(), root);
            views.push((view_name, pf, light_time, fixed));
            continue;
        }
        // Pin the pre-cutover ICC camera: exposure admission changes must not move the comparison.
        let (fixed, tile, grid) = if pf == 4582 {
            (ao_render::Camera { pos: ao_render::Vec3::new(938.0, 25.7, -750.0), yaw: 0.0, pitch: -0.18534796, roll: 0.0 }, 258, [234, 189])
        } else {
            grass_ground_camera(&scene, &surface.borrow(), &exposure, reference, "")
                .map(|(view, tile, grid, _)| (view, tile, grid))
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
        let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
        let fixed = if matches!(name.as_str(), "newland-day" | "newland-dusk" | "newland-night") {
            super::graphics_fixture_newland_camera(|point| collision.terrain_height(point))?
        } else { fixed };
        let exposure = super::super::player::grass_exposure_surface(Rc::new(RefCell::new(collision)));
        let lightbar = if name.ends_with("-lightbar-night") {
            let root = *evidence_roots.get(&name).context("LightBar fixture has no confirmed green root")?;
            let appearance = super::graphics_fixture_lightbar_appearance(&actor);
            let mut avatar = super::super::avatar::Avatar::new(&store, &dir, 25988, &appearance,
                ao_formats::character::actor::ActorAssets::new(&store)?)?;
            avatar.set_transform(root.to_array(), 0.0);
            ensure!(avatar.lights().next().is_some(), "native LightBar fixture has no mounted light");
            writeln!(report, "lightbar_actor_id=25988 mount=5 mesh=26163 field=0 byte=0 position={root:?} yaw=0 pose=idle time=0")?;
            Some(avatar)
        } else { None };
        let mut medians = [0.0; 2];
        let mut shadow_medians = [None; 2];
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
                let settings = EnhancedSettings {
                    sun_shadows: shadows, contact_shadows: contact, ambient_occlusion: ao,
                    emissive_bloom: effects, hdr, ground_grass: true, grass_distance,
                    grass_density: if variant == 0 && !compare_features { 0.0 } else { density },
                    ..if variant == 1 { feature_settings } else { EnhancedSettings::default() }
                };
                off.host.enhanced_settings = Some(settings);
                // The extended field can need more than 320 streamed exposure frames.
                // Record its up-front cost separately; the paired GPU measurement is stationary/warmed.
                off.frame(&mut shot, 0.0);
                if let Some(avatar) = &lightbar {
                    off.host.actor_models.push((super::super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
                    // Mounted sources arrive after the scene snapshot; dt=0 cannot age their
                    // native 0.25-second admission/slot fades. Prime BOTH variants identically.
                    let mut prime_time = 0.0_f32;
                    for _ in 0..60 {
                        submit_grass_lightbar(&mut off, avatar);
                        off.frame(&mut shot, 1.0 / 60.0);
                        prime_time += 1.0 / 60.0;
                    }
                    writeln!(report, "view={name} round={round} variant={variant} lightbar_prime_frames=60 lightbar_prime_dt={} renderer_frozen_time_after_prime={prime_time} avatar_pose_time=0", 1.0_f32 / 60.0)?;
                }
                let warm_started = std::time::Instant::now();
                off.warm_grass_tiles();
                writeln!(report, "explicit_grass_warm_wall_ms={}", warm_started.elapsed().as_secs_f64() * 1000.0)?;
                if !capture_only { off.set_gpu_timing(true)?; }
                let mut gpu = Vec::with_capacity(measured_frames);
                let mut prepare = Vec::with_capacity(measured_frames);
                let mut generation = Vec::with_capacity(320);
                let mut list = Default::default();
                for frame in 0..320 + measured_frames {
                    if let Some(avatar) = &lightbar {
                        submit_grass_lightbar(&mut off, avatar);
                    }
                    list = off.frame(&mut shot, 0.0);
                    let ms = if capture_only { None } else { Some(off.read_frame_gpu_ms()?.context("GPU timestamps unavailable")?) };
                    if frame >= 320 {
                        gpu.push(ms.context("measured frame missing GPU timestamp")?);
                        prepare.push(off.evidence_grass_metrics().1 as f64 / 1_000_000.0);
                    }
                    else { generation.push(off.evidence_grass_metrics().1 as f64 / 1_000_000.0); }
                }
                if lightbar.is_some() {
                    ensure!(off.host.previous_visible_actors.contains(&25988),
                        "native LightBar body was not admitted in the final evidence camera");
                    writeln!(report, "view={name} round={round} variant={variant} lightbar_body_admitted=true")?;
                }
                if !capture_only { off.set_gpu_timing(false)?; }
                let mut shadow_gpu = Vec::with_capacity(if shadow_timing { measured_frames } else { 0 });
                if shadow_timing { off.set_shadow_gpu_timing_stage(&shadow_stage)?; }
                if !capture_only {
                    let mut csv = String::with_capacity(measured_frames * 80 + 100);
                    writeln!(csv, "sample_index,whole_frame_gpu_ms,prepare_wall_ms,shadow_span_gpu_ms")?;
                    for sample in 0..measured_frames {
                        let shadow_ms = if shadow_timing {
                            if let Some(avatar) = &lightbar {
                                submit_grass_lightbar(&mut off, avatar);
                            }
                            list = off.frame(&mut shot, 0.0);
                            off.read_shadow_gpu_ms()?
                        } else { None };
                        write!(csv, "{sample},{},{},", gpu[sample], prepare[sample])?;
                        if let Some(ms) = shadow_ms {
                            write!(csv, "{ms}")?;
                            shadow_gpu.push(ms);
                        }
                        writeln!(csv)?;
                    }
                    std::fs::write(out.join(format!("{name}-round{round}-variant{variant}-timing.csv")), csv)?;
                }
                if shadow_timing { off.set_shadow_gpu_timing(false)?; }
                let shadow_stats = (!shadow_gpu.is_empty()).then(|| percentiles(&mut shadow_gpu));
                shadow_medians[variant] = shadow_stats.map(|stats| stats.0);
                if shadow_timing {
                    writeln!(report, "view={name} round={round} variant={variant} shadow_stage={shadow_stage} shadow_gpu_samples={} shadow_gpu_ms_p50_p95_p99={shadow_stats:?}", shadow_gpu.len())?;
                }
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
                let max_blade_candidates = cells * (1536.0 * settings.clamped().grass_density / 2.0).round().max(1.0) as usize * usize::from(variant == 1 || compare_features);
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
                        writeln!(report, "view={name} neutral_vs_grass_differing_bytes={differing} comparison_features={compare_features}")?;
                        if name == "borealis-grass-ground-far-day" && !compare_features {
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
                if shadow_timing {
                    let delta = shadow_medians[1].zip(shadow_medians[0]).map(|(on, off)| on - off);
                    writeln!(report, "view={name} round={round} shadow_stage={shadow_stage} incremental_grass_gpu_shadow_p50_ms={delta:?}")?;
                }
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
    green_center: bool,
}

/// Selects an exposed native grass cluster, never a texture-colour heuristic.
/// The chosen native grid/tile and exact fixed camera are retained in the manifest.
fn grass_ground_camera(
    scene: &ao_scene::Scene,
    surface: &ao_formats::playfield::collision::Collision,
    exposure: &ao_render::GrassExposure,
    reference: ao_render::Vec3,
    mode: &str,
) -> anyhow::Result<(ao_render::Camera, u16, [i32; 2], ao_render::Vec3)> {
    use ao_render::Vec3;
    let cells = grass_fixture_cells(scene);
    let mut best: Option<(usize, f32, ao_render::Camera, u16, [i32; 2], Vec3)> = None;
    // Counts survive failure so native CPU diagnostics identify the rejecting stage.
    let mut rejected = [0usize; 9];
    let floor = |point| grass_path_floor(point, |p| surface.terrain_height(p), |p| surface.ground(p));
    let vertical_tan = (camera::lens(scene.lens.unwrap_or_default(), false)
        .vertical_fov(1920.0 / 1200.0) * 0.5).tan();
    for (&grid, cell) in &cells {
        let distance = cell.root.distance_squared(reference);
        let neighbors = (-1..=1).flat_map(|z| (-1..=1).map(move |x| [grid[0]+x, grid[1]+z]))
            .filter(|neighbor| cells.contains_key(neighbor)).count();
        if best.as_ref().is_some_and(|old| neighbors < old.0 || (neighbors == old.0 && distance >= old.1)) { continue; }
        let path = matches!(mode, "path-day" | "path-dusk");
        if !cell.samples.iter().all(|point| (exposure.query)(point.to_array())
            && (path || matches!(surface.ground_material(point.to_array()), 6 | 15))) { rejected[0] += 1; continue; }
        if path && !matches!(surface.ground_material(cell.root.to_array()), 6 | 15) { rejected[1] += 1; continue; }
        if !mode.is_empty() && !cell.green_center { rejected[2] += 1; continue; }
        if mode == "backlit-dusk" {
            let sun = Vec3::from(scene.environment.context("backlit grass has no native environment")?.sun_dir);
            if let Some(camera) = grass_backlit_camera(cell.root, sun,
                |point| surface.ground(point), |a, b| super::super::player::segment_clear(surface, a, b)) {
                best = Some((neighbors, distance, camera, cell.tile, grid, cell.root));
            }
            continue;
        }
        if matches!(mode, "path-day" | "path-dusk") {
            'directions: for direction in [Vec3::X, -Vec3::X, Vec3::Z, -Vec3::Z] {
                // Painted path edges need not coincide with a native fine-cell edge.
                // Search nearby edges without changing any grass/bare/LOS evidence.
                for edge in [cell.size * 0.5, 0.5, 1.0, 1.5, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0] {
                let boundary = cell.root + direction * edge;
                let tangent = direction.cross(Vec3::Y);
                let mut valid = |camera: &ao_render::Camera| {
                    if camera.forward().dot(direction) >= -0.5 { rejected[4] += 1; return false; }
                    // Grass remains exposed native green terrain. The bare side may
                    // instead be a raised native KD pavement, never buried terrain.
                    for index in 0..12 {
                        let across = [-0.75, 0.0, 0.75][index % 3];
                        let mut point = if index < 3 { cell.root + tangent * across }
                            else { boundary + direction * [0.4, 1.2, 2.4][(index - 3) / 3] + tangent * across };
                        let Some(height) = (if index < 3 { surface.terrain_height(point.to_array()) }
                            else { floor(point.to_array()) }) else { rejected[5] += 1; return false; };
                        point.y = height;
                        if index < 3 && (!(exposure.query)(point.to_array())
                            || floor(point.to_array()).is_none_or(|ground| (ground - height).abs() > 0.02)) {
                            rejected[5] += 1; return false;
                        }
                        let material = surface.ground_material(point.to_array());
                        let terrain = surface.terrain_height(point.to_array());
                        let raised = terrain.is_some_and(|height| point.y > height + 0.1);
                        let green = if raised { Some(false) } else { grass_path_green(scene, point) };
                        if (index < 3 && (!matches!(material, 6 | 15) || green != Some(true)))
                            || (index >= 3 && (green != Some(false) || matches!(material, 8..=11))) {
                            rejected[6] += 1; return false;
                        }
                        let low = point + Vec3::Y * 0.08;
                        if index >= 3 && !grass_path_bare_in_frame(camera, boundary, direction, point, vertical_tan) {
                            rejected[4] += 1; return false;
                        }
                        if !super::super::player::segment_clear(surface, low.to_array(), (point + Vec3::Y * 12.0).to_array())
                            || !super::super::player::segment_clear(surface, camera.pos.to_array(), low.to_array()) {
                            rejected[7] += 1; return false;
                        }
                    }
                    let Some(eye_ground) = floor(camera.pos.to_array()) else { rejected[8] += 1; return false; };
                    let eye_root = Vec3::new(camera.pos.x, eye_ground, camera.pos.z);
                    if !(0.7..=1.8).contains(&(camera.pos.y - eye_ground))
                        || matches!(surface.ground_material(eye_root.to_array()), 8..=11)
                        || !super::super::player::segment_clear(surface, (eye_root + Vec3::Y * 0.08).to_array(),
                            (eye_root + Vec3::Y * 12.0).to_array()) { rejected[8] += 1; return false; }
                    true
                };
                let camera = if mode == "path-dusk" {
                    let sun = Vec3::from(scene.environment.context("path dusk has no native environment")?.sun_dir).normalize_or_zero();
                    if direction.dot(-sun) < 0.5 { continue; }
                    grass_backlit_camera(cell.root, sun, floor, |a, b| {
                        super::super::player::segment_clear(surface, a, b)
                            && valid(&ao_render::Camera::look_at(Vec3::from(a), Vec3::from(b)))
                    })
                } else {
                    grass_path_crossing_camera(cell.root, edge * 2.0, direction, floor).filter(|camera| valid(camera))
                };
                let Some(camera) = camera else { rejected[3] += 1; continue; };
                best = Some((neighbors, distance, camera, cell.tile, grid, cell.root));
                break 'directions;
                }
            }
            continue;
        }
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
            best = Some((neighbors, distance, ao_render::Camera::look_at(eye, at), cell.tile, grid, cell.root));
            break;
        }
    }
    best.map(|(_, _, camera, tile, grid, root)| (camera, tile, grid, root)).with_context(|| format!(
        "no exposed native green grass with a clear evidence camera: mode={mode:?} cells={} rejected_exposure_rootmaterial_texture_camera_heading_floor_material_los_eye={rejected:?}", cells.len()))
}

fn grass_fixture_cells(scene: &ao_scene::Scene) -> std::collections::BTreeMap<[i32; 2], GrassFixtureCell> {
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
            let green_center = scene.meshes.get(patch.mesh).and_then(|mesh| mesh.submeshes.get(cell.submesh))
                .and_then(|submesh| submesh.texture).and_then(|key| scene.textures.get(&key))
                .and_then(|texture| {
                    let index = ((texture.height / 2 * texture.width + texture.width / 2) * 4) as usize;
                    texture.rgba.get(index..index + 3)
                }).is_some_and(|rgb| {
                    let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(f32::from);
                    g >= 20.0 && g - (r * 0.95).max(b * 1.1) >= 0.9
                });
            cells.insert(grid, GrassFixtureCell { root, samples, tile: cell.native_tile, size: patch.cell_size, green_center });
        }
    }
    cells
}

struct GrassLightbarProof {
    owner: ao_render::Vec3,
    source: ao_render::Vec3,
    receiver: ao_render::Vec3,
    lit_receiver: ao_render::Vec3,
    caster: [usize; 4],
}

// This is an evidence selector, not scene construction: both the collision hit and
// an opaque, non-ground triangle from the loaded scene must cross the native ray.
fn grass_lightbar_camera(
    scene: &ao_scene::Scene,
    surface: &ao_formats::playfield::collision::Collision,
    exposure: &ao_render::GrassExposure,
    reference: ao_render::Vec3,
    light: ao_scene::Light,
) -> anyhow::Result<(ao_render::Camera, u16, [i32; 2], GrassLightbarProof)> {
    use ao_render::Vec3;
    ensure!(light.range.is_finite() && light.range > 0.0, "native LightBar has no positive range");
    let ground_meshes: std::collections::HashSet<_> = scene.ground_detail.iter().map(|patch| patch.mesh).collect();
    let mut triangles = Vec::new();
    for (instance_index, instance) in scene.instances.iter().enumerate() {
        if ground_meshes.contains(&instance.mesh) { continue; }
        let mesh = &scene.meshes[instance.mesh];
        let transform = |point: [f32; 3]| {
            let m = instance.transform;
            let xyz = |axis: [f32; 4]| Vec3::new(axis[0], axis[1], axis[2]);
            xyz(m[0]) * point[0] + xyz(m[1]) * point[1] + xyz(m[2]) * point[2] + xyz(m[3])
        };
        for (submesh_index, submesh) in mesh.submeshes.iter().enumerate() {
            if submesh.blend != ao_scene::Blend::Opaque || submesh.overlay || submesh.base_color[3] < 1.0 { continue; }
            for (triangle_index, indices) in submesh.indices.as_chunks::<3>().0.iter().enumerate() {
                let points = [indices[0], indices[1], indices[2]].map(|index| transform(mesh.vertices[index as usize].pos));
                let normal = (points[1] - points[0]).cross(points[2] - points[0]).normalize_or_zero();
                if normal == Vec3::ZERO || normal.y.abs() > 0.7 { continue; }
                let lo = points[0].min(points[1]).min(points[2]);
                let hi = points[0].max(points[1]).max(points[2]);
                triangles.push((points, lo, hi, [instance_index, instance.mesh, submesh_index, triangle_index]));
            }
        }
    }
    // Index the authored triangles once; a whole-playfield cell × triangle scan
    // would dominate this opt-in fixture before it ever reaches the renderer.
    let bucket = |value: f32| (value / 8.0).floor() as i32;
    let mut nearby = std::collections::HashMap::<[i32; 2], Vec<usize>>::new();
    for (index, &(_, lo, hi, _)) in triangles.iter().enumerate() {
        for x in bucket(lo.x - light.range)..=bucket(hi.x + light.range) {
            for z in bucket(lo.z - light.range)..=bucket(hi.z + light.range) {
                nearby.entry([x, z]).or_default().push(index);
            }
        }
    }
    let cells = grass_fixture_cells(scene);
    let mut ordered: Vec<_> = cells.iter().filter(|(_, cell)| cell.green_center).collect();
    ordered.sort_by(|(a_grid, a), (b_grid, b)| a.root.distance_squared(reference)
        .total_cmp(&b.root.distance_squared(reference)).then_with(|| a_grid.cmp(b_grid)));
    let clear = |a: Vec3, b: Vec3| super::super::player::segment_clear(surface, a.to_array(), b.to_array());
    let ground = |point: Vec3| -> Option<Vec3> {
        let height = surface.terrain_height(point.to_array())?;
        let point = Vec3::new(point.x, height, point.z);
        let support = surface.ground((point + Vec3::Y * 0.5).to_array())?;
        ((support - height).abs() < 0.02).then_some(point)
    };
    for (&grid, cell) in ordered {
        for sample in cell.samples {
            let Some(root) = ground(sample) else { continue; };
            if !(exposure.query)(root.to_array()) || !matches!(surface.ground_material(root.to_array()), 6 | 15) { continue; }
            let receiver = root + Vec3::Y * 0.15;
            let Some(candidates) = nearby.get(&[bucket(receiver.x), bucket(receiver.z)]) else { continue; };
            for &index in candidates {
                let (points, lo, hi, caster) = triangles[index];
                if receiver.clamp(lo, hi).distance(receiver) >= light.range { continue; }
                let normal = (points[1] - points[0]).cross(points[2] - points[0]);
                let normal = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
                for direction in [normal, -normal] {
                    let tangent = direction.cross(Vec3::Y);
                    for distance in [2.5, 3.5, 4.5, 5.0] {
                        let Some(owner) = ground(root + direction * distance) else { continue; };
                        let source = owner + Vec3::from(light.pos);
                        if !grass_lightbar_blocked_receiver(source, receiver, light.range, points)
                            || clear(source, receiver) { continue; }
                        if !clear(owner + Vec3::Y * 0.05, source)
                            || surface.sphere_hit((owner + Vec3::Y).to_array(), 0.4).is_some() { continue; }
                        for side in [-1.0, 1.0] {
                            for across in [0.8, 1.6, 2.4] {
                                let Some(lit_root) = ground(root + tangent * side * across) else { continue; };
                                let lit_receiver = lit_root + Vec3::Y * 0.15;
                                if source.distance(lit_receiver) >= light.range * 0.9
                                    || !(exposure.query)(lit_root.to_array())
                                    || !matches!(surface.ground_material(lit_root.to_array()), 6 | 15)
                                    || !clear(source, lit_receiver) { continue; }
                                let center = (root + owner) * 0.5;
                                let Some(eye_root) = ground(center + tangent * side * 6.0) else { continue; };
                                let eye = eye_root + Vec3::Y * 3.4;
                                let camera = ao_render::Camera::look_at(eye, center + Vec3::Y * 0.7);
                                let visible = [owner + Vec3::Y * 0.05, owner + Vec3::Y, source, receiver, lit_receiver]
                                    .into_iter().all(|point| clear(eye, point)
                                        && camera.forward().dot((point - eye).normalize_or_zero()) > 0.88);
                                if visible {
                                    return Ok((camera, cell.tile, grid,
                                        GrassLightbarProof { owner, source, receiver, lit_receiver, caster }));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    anyhow::bail!("no native opaque world caster with blocked green receiver, adjacent lit receiver, native LightBar range and visible owner/camera LOS")
}

fn grass_lightbar_blocked_receiver(
    source: ao_render::Vec3,
    receiver: ao_render::Vec3,
    range: f32,
    triangle: [ao_render::Vec3; 3],
) -> bool {
    let ray = receiver - source;
    if !range.is_finite() || range <= 0.0 || !ray.is_finite() || ray.length() >= range * 0.9 { return false; }
    let [a, b, c] = triangle;
    let edge = b - a;
    let other = c - a;
    let cross = ray.cross(other);
    let determinant = edge.dot(cross);
    if determinant.abs() < 1e-6 { return false; }
    let offset = source - a;
    let u = offset.dot(cross) / determinant;
    let q = offset.cross(edge);
    let v = ray.dot(q) / determinant;
    let t = other.dot(q) / determinant;
    (0.0..=1.0).contains(&u) && v >= 0.0 && u + v <= 1.0 && (0.02..0.98).contains(&t)
}

#[test]
fn grass_lightbar_receiver_requires_in_range_non_endpoint_world_intersection() {
    use ao_render::Vec3;
    let triangle = [Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 3.0, -1.0), Vec3::new(0.0, 0.0, 1.0)];
    let source = Vec3::new(-2.0, 2.1, 0.0);
    let receiver = Vec3::new(2.0, 0.15, 0.0);
    assert!(grass_lightbar_blocked_receiver(source, receiver, 6.13, triangle));
    assert!(grass_lightbar_blocked_receiver(source, receiver, 6.13, [triangle[2], triangle[1], triangle[0]]));
    assert!(!grass_lightbar_blocked_receiver(source, receiver, 4.0, triangle));
    assert!(!grass_lightbar_blocked_receiver(source, receiver + Vec3::Z * 4.0, 10.0, triangle));
    assert!(!grass_lightbar_blocked_receiver(source, receiver, 6.13, triangle.map(|p| p - Vec3::X * 2.0)));
}

// Match grass_mask.rs's 64x64 classification and grass.wgsl's oriented,
// linearly filtered repeat sample. A transition tile's material alone cannot
// distinguish its painted road from the green grass beside it.
fn grass_path_green(scene: &ao_scene::Scene, point: ao_render::Vec3) -> Option<bool> {
    for patch in &scene.ground_detail {
        let x = point.x / patch.cell_size - patch.origin[0] as f32;
        let z = -point.z / patch.cell_size - patch.origin[1] as f32;
        if x < 0.0 || z < 0.0 || x >= patch.cells as f32 || z >= patch.cells as f32 { continue; }
        let cell = patch.fine_cells.get(z.floor() as usize * patch.cells + x.floor() as usize)?;
        let key = scene.meshes.get(patch.mesh)?.submeshes.get(cell.submesh)?.texture?;
        let texture = scene.textures.get(&key)?;
        return grass_path_texture_green(texture, [x.fract(), z.fract()], cell.orientation);
    }
    None
}

fn grass_path_texture_green(texture: &ao_scene::Texture, [u, v]: [f32; 2], orientation: u8) -> Option<bool> {
    let (w, h) = (texture.width as usize, texture.height as usize);
    if w == 0 || h == 0 || texture.rgba.len() < w * h * 4 { return None; }
    let [u, v] = match orientation {
        0 => [u, 1.0 - v], 1 => [1.0 - v, 1.0 - u],
        2 => [1.0 - u, v], _ => [v, u],
    };
    let (x, y) = (u * 64.0 - 0.5, v * 64.0 - 0.5);
    let sample = |x: i32, y: i32| {
        let (x, y) = (x.rem_euclid(64) as usize, y.rem_euclid(64) as usize);
        let (x0, x1) = (x * w / 64, ((x + 1) * w / 64).max(x * w / 64 + 1).min(w));
        let (y0, y1) = (y * h / 64, ((y + 1) * h / 64).max(y * h / 64 + 1).min(h));
        let mut sum = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) * 4;
                let [r, g, b] = [texture.rgba[i], texture.rgba[i + 1], texture.rgba[i + 2]].map(f32::from);
                if g >= 20.0 { sum += ((g - (r * 0.95).max(b * 1.1)) / 6.0).clamp(0.0, 1.0); }
            }
        }
        (sum / ((x1 - x0) * (y1 - y0)) as f32 * 255.0 + 0.5).floor() / 255.0
    };
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let top = sample(ix, iy) * (1.0 - fx) + sample(ix + 1, iy) * fx;
    let bottom = sample(ix, iy + 1) * (1.0 - fx) + sample(ix + 1, iy + 1) * fx;
    Some(top * (1.0 - fy) + bottom * fy >= 0.15)
}

#[test]
fn grass_path_texture_distinguishes_oriented_transition_from_material() {
    let texture = ao_scene::Texture {
        width: 64, height: 64,
        rgba: (0..64 * 64).flat_map(|i| if i % 64 < 32 {
            [58, 100, 38, 255]
        } else { [190, 170, 120, 255] }).collect(),
        source: Default::default(),
    };
    for (orientation, green, bare) in [
        (0, [0.25, 0.25], [0.75, 0.25]),
        (1, [0.25, 0.75], [0.25, 0.25]),
        (2, [0.75, 0.25], [0.25, 0.25]),
        (3, [0.25, 0.25], [0.25, 0.75]),
    ] {
        assert_eq!(grass_path_texture_green(&texture, green, orientation), Some(true));
        assert_eq!(grass_path_texture_green(&texture, bare, orientation), Some(false));
    }
}

// Query the native walking floor from near the heightfield, not from a roof-height
// camera probe. Collision retains its native bounded downward ray.
fn grass_path_floor(
    mut point: [f32; 3],
    terrain: impl Fn([f32; 3]) -> Option<f32>,
    ground: impl Fn([f32; 3]) -> Option<f32>,
) -> Option<f32> {
    point[1] = terrain(point)? + 0.5;
    ground(point)
}

fn grass_path_crossing_camera(
    root: ao_render::Vec3,
    size: f32,
    direction: ao_render::Vec3,
    terrain: impl Fn([f32; 3]) -> Option<f32>,
) -> Option<ao_render::Camera> {
    use ao_render::{Camera, Vec3};
    let boundary = root + direction * (size * 0.5);
    let mut eye = boundary + direction * 3.5 + direction.cross(Vec3::Y) * 1.5;
    let mut at = boundary - direction * 0.75;
    eye.y = terrain(eye.to_array())? + 1.7;
    at.y = terrain(at.to_array())? + 0.15;
    (eye.is_finite() && at.is_finite()).then(|| Camera::look_at(eye, at))
}

#[test]
fn grass_path_camera_crosses_the_edge_from_bare_foreground() {
    use ao_render::Vec3;
    for direction in [Vec3::X, -Vec3::X, Vec3::Z, -Vec3::Z] {
        let camera = grass_path_crossing_camera(Vec3::ZERO, 4.0, direction, |_| Some(3.0)).unwrap();
        assert!(camera.pos.dot(direction) > 2.0);
        assert!(camera.forward().dot(direction) < -0.8);
        assert_eq!(camera.pos.y, 4.7);
    }
    assert!(grass_path_crossing_camera(Vec3::ZERO, 4.0, Vec3::X, |_| None).is_none());
}

fn submit_grass_lightbar(off: &mut Offscreen, avatar: &super::super::avatar::Avatar) {
    off.host.dynamic_lights.extend(avatar.lights());
    let mut frame = avatar.frame();
    frame.always = false;
    off.host.actors.push(frame);
}

// Evidence must be in front of the eye, on the bare side of the actual painted
// edge and visibly in the ground half of the unchanged 1920x1200 fixture lens.
fn grass_path_bare_in_frame(
    camera: &ao_render::Camera,
    boundary: ao_render::Vec3,
    direction: ao_render::Vec3,
    point: ao_render::Vec3,
    vertical_tan: f32,
) -> bool {
    use ao_render::Vec3;
    if (camera.pos - boundary).dot(direction) <= 2.4 { return false; }
    let delta = point - camera.pos;
    let forward = camera.forward();
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(forward);
    let depth = delta.dot(forward);
    depth > 0.2 && delta.dot(right).abs() < depth * vertical_tan * (1920.0 / 1200.0) * 0.9
        && (-0.9..=0.05).contains(&(delta.dot(up) / (depth * vertical_tan)))
}

#[test]
fn grass_path_bare_frame_rejects_inside_grass_and_offscreen_probes() {
    use ao_render::{Camera, Vec3};
    let boundary = Vec3::new(0.0, 0.0, -8.0);
    let point = boundary - Vec3::Z;
    let inside = Camera::look_at(Vec3::new(0.0, 0.8, -4.0), Vec3::ZERO);
    assert!(!grass_path_bare_in_frame(&inside, boundary, -Vec3::Z, point, 0.625));
    let bare = Camera::look_at(Vec3::new(0.0, 0.8, -16.0), Vec3::Y * 0.2);
    assert!(grass_path_bare_in_frame(&bare, boundary, -Vec3::Z, point, 0.625));
    assert!(!grass_path_bare_in_frame(&bare, boundary, -Vec3::Z, point + Vec3::X * 20.0, 0.625));
    assert!(!grass_path_bare_in_frame(&bare, boundary, -Vec3::Z, bare.pos - Vec3::Z, 0.625));
}

fn grass_backlit_camera(
    root: ao_render::Vec3,
    sun: ao_render::Vec3,
    ground: impl Fn([f32; 3]) -> Option<f32>,
    mut clear: impl FnMut([f32; 3], [f32; 3]) -> bool,
) -> Option<ao_render::Camera> {
    use ao_render::{Camera, Vec3};
    let sun = sun.normalize_or_zero();
    if !root.is_finite() || !sun.is_finite() || sun.y <= 0.0 { return None; }
    let horizontal = Vec3::new(sun.x, 0.0, sun.z).normalize_or_zero();
    if horizontal == Vec3::ZERO { return None; }
    let at = root + Vec3::Y * 0.2;
    for distance in [4.0, 6.0, 8.0, 12.0, 16.0] {
        let mut eye = root - horizontal * distance;
        let Some(height) = ground((eye + Vec3::Y * 15.0).to_array()) else { continue; };
        eye.y = height + 0.8;
        let camera = Camera::look_at(eye, at);
        if eye.is_finite() && camera.forward().dot(sun) > 0.6 && clear(eye.to_array(), at.to_array()) {
            return Some(camera);
        }
    }
    None
}

#[test]
fn grass_backlit_camera_requires_native_sun_ground_and_los() {
    use ao_render::Vec3;
    let sun = Vec3::new(1.0, 0.15, 0.0);
    let camera = grass_backlit_camera(Vec3::ZERO, sun, |_| Some(0.0), |_, _| true).unwrap();
    assert!(camera.forward().dot(sun.normalize()) > 0.6);
    assert_eq!(camera.pos.y, 0.8);
    assert!(grass_backlit_camera(Vec3::ZERO, -sun, |_| Some(0.0), |_, _| true).is_none());
    assert!(grass_backlit_camera(Vec3::ZERO, sun, |_| None, |_, _| true).is_none());
    assert!(grass_backlit_camera(Vec3::ZERO, sun, |_| Some(0.0), |_, _| false).is_none());
}

#[test]
fn grass_path_camera_uses_raised_floor_not_roof_and_retries_dusk() {
    use ao_render::Vec3;
    // Synthetic native bounded ground ray: pavement is 0.4 m over terrain;
    // a +15 m probe would instead select the 10 m roof.
    let terrain = |_| Some(3.0);
    let ground = |point: [f32; 3]| Some(if point[1] > 13.0 { 13.0 } else { 3.4 });
    let floor = |point| grass_path_floor(point, terrain, ground);
    assert_eq!(ground([0.0, 18.0, 0.0]), Some(13.0));
    assert_eq!(floor([0.0, 18.0, 0.0]), Some(3.4));
    assert!(grass_path_floor([0.0; 3], |_| None, ground).is_none());
    let root = Vec3::new(0.0, 3.0, 0.0);
    let day = grass_path_crossing_camera(root, 4.0, -Vec3::X, floor).unwrap();
    assert!((day.pos.y - 5.1).abs() < 0.0001);
    let mut tried = Vec::new();
    let dusk = grass_backlit_camera(root, Vec3::new(1.0, 0.15, 0.0), floor, |eye, _| {
        tried.push(eye);
        // First eye has LOS but fails the path evidence, so try the next.
        eye[0] < -5.0
    }).unwrap();
    assert_eq!(tried.len(), 2);
    assert!((dusk.pos.x + 6.0).abs() < 0.0001);
    assert!((dusk.pos.y - 4.2).abs() < 0.0001);
}

#[test]
#[ignore = "installed Newland assets; CPU-only native day/dusk path selector acceptance"]
fn grass_native_newland_path_cameras() -> anyhow::Result<()> {
    use ao_render::Vec3;
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let surface = Rc::new(RefCell::new(ao_formats::playfield::collision::Collision::load(&store, 566)?));
    let exposure = super::super::player::grass_exposure_surface(surface.clone());
    let mut failures = Vec::new();
    for (mode, time) in [("path-day", 3240.0), ("path-dusk", 4600.0)] {
        ao_scene::crt_srand(1);
        let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
            &store, &dir, 566, time, 0, Some(ao_formats::weather::State::clear()))?;
        let cells = grass_fixture_cells(&scene);
        let patch = scene.ground_detail.first().context("Newland has no native ground detail")?;
        let reference = Vec3::new(
            (patch.origin[0] as f32 + patch.cells as f32 * 0.5) * patch.cell_size,
            0.0, -(patch.origin[1] as f32 + patch.cells as f32 * 0.5) * patch.cell_size);
        let collision = surface.borrow();
        let exposed = cells.values().filter(|cell| cell.samples.iter()
            .all(|point| (exposure.query)(point.to_array()))).count();
        let green_exposed = cells.values().filter(|cell| cell.green_center
            && matches!(collision.ground_material(cell.root.to_array()), 6 | 15)
            && cell.samples.iter().all(|point| (exposure.query)(point.to_array()))).count();
        let env = scene.environment.context("Newland has no native weather environment")?;
        println!("native_grass_path pf=566 mode={mode} time={time} crt_seed=1 weather=clear cells={} exposed={exposed} green_exposed={green_exposed} reference={reference:?} sun_dir={:?} sun_color={:?}",
            cells.len(), env.sun_dir, env.sun_color);
        ensure!(green_exposed > 0, "pf566 {mode} has no exposed native green cells");
        ensure!(env.sun_dir[1] > 0.0 && env.sun_color.iter().any(|&c| c > 0.0),
            "pf566 {mode} time={time} lacks native direct sunlight");
        match grass_ground_camera(&scene, &collision, &exposure, reference, mode) {
            Ok((camera, tile, grid, root)) => {
                let eye_floor = grass_path_floor(camera.pos.to_array(),
                    |p| collision.terrain_height(p), |p| collision.ground(p))
                    .context("selected native path camera has no collision floor")?;
                println!("native_grass_path_selected mode={mode} tile={tile} grid={grid:?} root={root:?} camera={camera:?} eye_floor={eye_floor} eye_terrain={:?} eye_material={} clearance={}",
                    collision.terrain_height(camera.pos.to_array()),
                    collision.ground_material([camera.pos.x, eye_floor, camera.pos.z]), camera.pos.y - eye_floor);
                ensure!((0.7..=1.8).contains(&(camera.pos.y - eye_floor)), "native path eye not grounded");
                ensure!((exposure.query)(root.to_array())
                    && matches!(collision.ground_material(root.to_array()), 6 | 15),
                    "selected path root lost exposed native green grass");
            }
            Err(error) => failures.push(format!("pf566 time={time} mode={mode}: {error:#}")),
        }
    }
    ensure!(failures.is_empty(), "native Newland path selection failed:\n{}", failures.join("\n"));
    Ok(())
}
