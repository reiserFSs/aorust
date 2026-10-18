//! Dedicated native-data weather acceptance evidence; never opens a window.
use super::{camera, graphics_fixture_settings, Shot};
use anyhow::{Context, ensure};
use ao_render::{Camera, Offscreen, Vec3};
use std::{cell::RefCell, fmt::Write, path::PathBuf, rc::Rc};
#[path = "weather_effects.rs"]
mod weather_effects;

#[test]
#[ignore = "installed-client headless weather/particles PNG and paired ABBA acceptance; requires GPU lock"]
fn weather_and_particles_graphics() -> anyhow::Result<()> {
    const SIZE: (u32, u32) = (640, 400);
    const FRAMES: usize = 96;
    const DT: f32 = 0.25;
    let out = PathBuf::from(std::env::var_os("AOMAC_WEATHER_OUT").context("set AOMAC_WEATHER_OUT under /tmp/WeatherFx")?);
    ensure!(out.is_absolute() && out.starts_with("/tmp/WeatherFx") && !out.components().any(|p| matches!(p, std::path::Component::ParentDir)), "output must stay under /tmp/WeatherFx");
    ensure!(!out.exists(), "refusing to overwrite {}", out.display());
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "status=running\nvisual_acceptance=unexercised\n")?;
    let result = (|| -> anyhow::Result<()> {
        let dir = ao_gui::client_dir();
        let store = ao_rdb::RecordStore::open(&dir)?;
        let started = std::time::Instant::now();
        let mut manifest = format!("test=weather_and_particles_graphics\nsize={}x{}\nframes_per_phase={FRAMES}\nwarmup_frames=32\nwarmup_dt=0\ndt={DT}\norder=off,on,on,off\nmsaa=1\nanisotropy=1\ncrt_seed=1\nactors=none_except_native_effect_quads\nvisibility=renderer_native_admission\nmetric=whole_frame_not_isolated_pass\npng_readback=outside_timing\nvisual_acceptance=unexercised\nstrip_columns=frames0_31_63_95\nstrip_rows=off_then_on\nbefore_after_rows=off_then_on_at_frame31\ndrying=rain_removed_at_frame8_no_wetness_override\n", SIZE.0, SIZE.1);
        for (label, program, args) in [
            ("revision", "git", vec!["rev-parse".into(), "HEAD".into()]),
            ("source_diff", "git", vec!["diff".into(), "--no-ext-diff".into(), "HEAD".into()]),
            ("resource_sha256", "shasum", vec!["-a".into(), "256".into(), dir.join("cd_image/rdb.db").into_os_string()]),
        ] {
            let output = std::process::Command::new(program).args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
            ensure!(output.status.success(), "{program} failed");
            writeln!(manifest, "{label}={:?}", String::from_utf8(output.stdout)?.trim())?;
        }
        let mut csv = String::from("case,phase,enabled,frame,dt,rain,storm0,storm1,storm2,storm3,wind_x,wind_y,wind_z,gpu_ms,cpu_prepare_ms,cpu_encode_ms,cpu_submit_ms\n");
        let mut summary = String::from("case,phase,enabled,samples,gpu_mean_ms,gpu_p50_ms,gpu_p95_ms,gpu_p99_ms\n");
        let selected = std::env::var("AOMAC_WEATHER_CASE").ok();
        let mut selected_count = 0;
        let cases = [
            ("rain-outdoor", "rain", 0, false, false, 0),
            ("rain-water", "rain", 0, false, false, 0),
            ("rain-roof", "rain", 0, true, false, 0),
            ("rain-indoor", "rain", 0, false, true, 0),
            ("wet-drying", "wet-surfaces", 0, false, false, 0),
            ("sand", "weather-particles", 3, false, false, 0),
            ("ash", "weather-particles", 4, false, false, 0),
            ("fallout-red", "weather-particles", 5, false, false, 0),
            ("fallout-green", "weather-particles", 6, false, false, 0),
            ("sparkle-curl", "spell-particles", 0, false, false, 1018),
            ("fluid-smoke", "fluid-smoke", 0, false, false, 3017),
        ];
        let required: Vec<_> = cases.iter().filter(|(name, _, _, _, _, kind)| {
            *kind == 0 && *name != "rain-water" && selected.as_ref().is_none_or(|selected| selected.as_str() == *name)
        }).map(|(_, _, channel, _, _, _)| *channel).collect();
        let native = native_weather_cases(&store, &required, None)?;
        for (name, profile, channel, roof, indoor, kind) in cases {
            if selected.as_ref().is_some_and(|selected| selected != name) { continue; }
            selected_count += 1;
            ensure!(started.elapsed().as_secs() < 270, "weather acceptance exceeded 270-second render budget");
            let (mut pf, game_day, time, state) = if name == "rain-water" {
                let (_, pf, day, time, state) = native_weather_cases(&store, &[0], Some(4582))?[0];
                (pf, day, time, state)
            } else if kind != 0 {
                (800, 0, 3240.0, native_weather_at(&store, 800, 0, 3240.0)?)
            } else {
                native.iter().find(|(field, _, _, _, _)| *field == channel).map(|(_, pf, day, time, state)| (*pf, *day, *time, *state)).context("missing native weather channel")?
            };
            let weather_pf = pf;
            let mut fixed = Camera::look_at([679.6, 74.5, -476.7].into(), [679.6, 73.0, -486.7].into());
            if indoor { pf = 127; fixed = Camera::look_at([96.0, 109.305, -242.0].into(), [108.0, 108.9, -242.0].into()); }
            if name == "rain-water" {
                fixed = Camera { pos: [1010.4, 24.0, -663.1].into(), yaw: 2.4288, pitch: -0.2, roll: 0.0 };
            }
            ao_scene::crt_srand(1);
            let (mut scene, report) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, game_day, Some(state))?;
            if name == "rain-water" {
                ensure!(!scene.liquids.is_empty(), "ICC water capture has no native liquid surface");
                writeln!(manifest, "case=rain-water native_liquids={} camera_source=existing_ICC_offshore_fixture acceptance=rain_over_existing_native_liquids_only water_rings=deferred_pending_published_native_water_mask_api generic_rain_delta_is_not_ring_acceptance=true", scene.liquids.len())?;
            }
            let collision = Rc::new(RefCell::new(ao_formats::playfield::collision::Collision::load(&store, pf)?));
            super::super::player::bake_sky_open(&collision.borrow(), &mut scene);
            let surface = super::super::player::weather_surface(collision.clone());
            if !indoor && kind == 0 && name != "rain-water" {
                // Search native collision, not guessed roofs or synthetic shelter callbacks.
                let mut selected = None;
                let origin = if pf == 800 { [679.6, 74.5, -476.7] } else if pf == 566 { [414.0, 40.0, -320.0] } else { scene.spawn.context("native playfield has no fixture spawn")? };
                'search: for radius in 0..=64_i32 { for z in -radius..=radius { for x in -radius..=radius {
                    if x.abs().max(z.abs()) != radius { continue; }
                    let p = [origin[0] + x as f32 * 4.0, origin[1], origin[2] + z as f32 * 4.0];
                    if let Some([top, ground]) = surface(p) {
                        if (roof && top > ground + 2.5) || (!roof && top < -1e19) {
                            selected = Some([p[0], ground + 1.7, p[2]]); break 'search;
                        }
                    }
                } } }
                let eye = selected.with_context(|| format!("{name}: no native collision {} point within bounded scan pf{pf}", if roof { "roofed" } else { "open" }))?;
                fixed = Camera::look_at(eye.into(), Vec3::from(eye) + Vec3::new(0.0, -0.7, -8.0));
            }
            ensure!(report.dungeon == indoor, "{name}: native dungeon identity mismatch");
            let sampled = surface(fixed.pos.to_array());
            ensure!(indoor || sampled.is_some(), "{name}: outdoor camera collision unknown");
            if name == "rain-water" {
                ensure!(sampled.is_some_and(|[roof, _]| roof < -1e19), "ICC water camera must have native collision open sky");
            }
            let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
            lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
            writeln!(manifest, "case={name} profile={profile} pf={pf} weather_pf={weather_pf} game_day={game_day} time={time} native_state={state:?} camera={fixed:?} collision={sampled:?} dungeon={} native_meshes={} native_instances={}", report.dungeon, scene.meshes.len(), scene.instances.len())?;
            let mut strip = image::RgbaImage::new(SIZE.0 * 4, SIZE.1 * 2);
            let mut before_after = image::RgbaImage::new(SIZE.0, SIZE.1 * 2);
            let mut baseline_frames = Vec::new();
            let mut means = [0.0; 4];
            let mut visible_delta = 0_usize;
            let mut first_on = None::<Vec<u8>>;
            let mut temporal_changed_bytes = 0;
            for (phase, enabled) in [false, true, true, false].into_iter().enumerate() {
                ao_scene::crt_srand(1);
                let mut shot = Shot { gui: ao_gui::Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
                let mut off = Offscreen::evidence_timing(&shot, SIZE)?;
                off.fixed_camera(Some(fixed));
                off.host.msaa_samples = Some(1);
                off.host.anisotropy = Some(1);
                off.host.fog_density_scale = Some(1.0);
                off.host.enhanced_settings = Some(graphics_fixture_settings(if enabled { profile } else { "faithful" })?);
                off.frame(&mut shot, 0.0); // Host::set_scene resets weather and dynamic effects.
                let position = fixed.pos + fixed.forward() * 7.0;
                let mut effect = (kind != 0).then(|| weather_effects::NativeEffect::new(&dir, kind, position)).transpose()?;
                if let Some(effect) = &effect { writeln!(manifest, "case={name} phase={phase} {}", effect.metadata())?; }
                let input = ao_render::WeatherInput {
                    rain: if channel == 0 { state.fields[0] } else { 0.0 },
                    storm: std::array::from_fn(|i| if channel == i + 3 { state.fields[i + 3] } else { 0.0 }),
                    wind: [state.wind[0], state.wind[1], -state.wind[2]],
                    outdoor: !report.dungeon, surface: Some(surface.clone()),
                };
                off.host.weather_input = input.clone();
                for _ in 0..32 {
                    if let Some(effect) = &mut effect { effect.frame(0.0, &mut off.host)?; }
                    off.frame(&mut shot, 0.0);
                }
                off.wait_for_pipeline_warmup();
                off.set_cpu_timing(true);
                off.set_gpu_timing(true)?;
                let mut timings = Vec::with_capacity(FRAMES);
                for frame in 0..FRAMES {
                    ensure!(started.elapsed().as_secs() < 270, "weather acceptance exceeded 270-second render budget");
                    let mut current = input.clone();
                    if name == "wet-drying" && frame >= 8 { current.rain = 0.0; }
                    // Last A is the independent no-weather-input baseline, not a repeat of supplied input.
                    off.host.weather_input = if phase == 3 { Default::default() } else { current.clone() };
                    if let Some(effect) = &mut effect { effect.frame(DT, &mut off.host)?; }
                    off.frame(&mut shot, DT);
                    let gpu = off.read_frame_gpu_ms()?.context("GPU timestamp unavailable")?;
                    let cpu = off.frame_cpu_timings().context("CPU timing unavailable")?;
                    ensure!(gpu.is_finite() && gpu >= 0.0, "invalid GPU sample");
                    timings.push(gpu);
                    writeln!(csv, "{name},{phase},{enabled},{frame},{DT},{},{},{},{},{},{},{},{},{gpu},{},{},{}", current.rain, current.storm[0], current.storm[1], current.storm[2], current.storm[3], current.wind[0], current.wind[1], current.wind[2], cpu.prepare_ms, cpu.encode_ms, cpu.queue_submit_ms)?;
                    if let Some(column) = [0, 31, 63, 95].iter().position(|&f| f == frame) {
                        let raw = off.frozen_rgba()?;
                        if phase == 0 { baseline_frames.push(raw.clone()); }
                        if phase == 3 { ensure!(raw == baseline_frames[column], "{name} frame{frame}: all-off supplied weather differs from no-input baseline"); }
                        if phase == 1 { visible_delta += raw.iter().zip(&baseline_frames[column]).filter(|(a,b)| a != b).count(); }
                        if phase == 1 {
                            if let Some(previous) = &first_on {
                                temporal_changed_bytes += raw.iter().zip(previous).filter(|(a,b)| a != b).count();
                            } else { first_on = Some(raw.clone()); }
                        }
                        if phase <= 1 {
                            let image = image::RgbaImage::from_raw(SIZE.0, SIZE.1, raw).context("capture length")?;
                            image::imageops::replace(&mut strip, &image, i64::from(column as u32 * SIZE.0), i64::from(phase as u32 * SIZE.1));
                            if frame == 31 { image::imageops::replace(&mut before_after, &image, 0, i64::from(phase as u32 * SIZE.1)); }
                        }
                    }
                }
                means[phase] = timings.iter().sum::<f64>() / FRAMES as f64;
                timings.sort_by(f64::total_cmp);
                writeln!(summary, "{name},{phase},{enabled},{FRAMES},{},{},{},{}", means[phase], timings[47], timings[91], timings[95])?;
                if let Some(effect) = &effect { writeln!(manifest, "case={name} phase={phase} completed {}", effect.metadata())?; }
            }
            strip.save(out.join(format!("{name}-animated-strip.png")))?;
            before_after.save(out.join(format!("{name}-before-after.png")))?;
            writeln!(manifest, "case={name} all_off_equal=true differing_on_bytes={visible_delta} paired_abba_gpu_delta_ms={}", (means[1] + means[2] - means[0] - means[3]) * 0.5)?;
            writeln!(manifest, "case={name} temporal_changed_bytes={temporal_changed_bytes}")?;
            std::fs::write(out.join("manifest.txt"), &manifest)?;
            std::fs::write(out.join("frames.csv"), &csv)?;
            std::fs::write(out.join("summary.csv"), &summary)?;
            if !indoor && !roof { ensure!(visible_delta > 0, "{name}: enabled feature produced no captured pixel change"); }
            if indoor { ensure!(visible_delta == 0, "{name}: native dungeon leaked enhanced weather pixels"); }
            if name == "wet-drying" { ensure!(temporal_changed_bytes > 0, "wet surfaces did not visibly evolve after removing rain"); }
        }
        ensure!(selected_count > 0, "unknown AOMAC_WEATHER_CASE {selected:?}");
        writeln!(manifest, "elapsed_seconds={}\nstatus=complete", started.elapsed().as_secs_f64())?;
        std::fs::write(out.join("manifest.txt"), manifest)?;
        Ok(())
    })();
    std::fs::write(out.join("status.txt"), match &result { Ok(()) => "status=complete\nvisual_acceptance=unexercised\n".into(), Err(error) => format!("status=failed\nerror={error:#}\nvisual_acceptance=unexercised\n") })?;
    result
}

/// Sample a timestamp independently: native Area timers count frames, not elapsed day time.
fn native_weather_at(store: &ao_rdb::RecordStore, pf: u32, day: u32, time: f32) -> anyhow::Result<ao_formats::weather::State> {
    let mut weather = ao_formats::playfield::open_weather(store, pf)?;
    weather.update(day, time as f64, 1.0 / 60.0);
    Ok(weather.state())
}

type NativeWeatherCase = (usize, u32, u32, f32, ao_formats::weather::State);

/// Bounded native schedule discovery, without overwriting native channels or wind.
fn native_weather_cases(store: &ao_rdb::RecordStore, required: &[usize], only_pf: Option<u32>) -> anyhow::Result<Vec<NativeWeatherCase>> {
    let mut found = Vec::new();
    if required.is_empty() { return Ok(found); }
    let mut ids = if let Some(pf) = only_pf { vec![pf] } else {
        let mut ids = vec![800, 566, 4582];
        ids.extend(ao_formats::playfield::list_playfields(store)?.into_iter().map(|(pf, _)| pf).filter(|pf| ![800, 566, 4582].contains(pf)));
        ids
    };
    let mut outdoors = Vec::new();
    for pf in ids.drain(..) {
        if !ao_formats::playfield::zone_locator(store, pf)?.is_dungeon() { outdoors.push(pf); }
    }
    for day in 0..32 {
        for &pf in &outdoors {
            for time in (0..6480).step_by(30) {
                let state = native_weather_at(store, pf, day, time as f32)?;
                for &field in required {
                    if state.fields[field] > 0.4 && !found.iter().any(|(previous, _, _, _, _)| *previous == field) { found.push((field, pf, day, time as f32, state)); }
                }
                if required.iter().all(|field| found.iter().any(|(previous, _, _, _, _)| previous == field)) { return Ok(found); }
            }
        }
    }
    anyhow::bail!("native weather days0..32 lack requested channels {required:?} pf={only_pf:?}; found {:?}", found.iter().map(|(field, pf, day, time, _)| (*field, *pf, *day, *time)).collect::<Vec<_>>())
}

#[test]
#[ignore = "installed-client CPU native timestamp sampling regression; no GPU"]
fn weather_fixture_samples_fresh_native_timestamps() -> anyhow::Result<()> {
    let store = ao_rdb::RecordStore::open(&ao_gui::client_dir())?;
    let (_, pf, day, _, _) = native_weather_cases(&store, &[0], None)?[0];
    let mut reused = ao_formats::playfield::open_weather(&store, pf)?;
    let mut differs = false;
    for time in (0..6480).step_by(30) {
        reused.update(day, time as f64, 1.0 / 60.0);
        let fresh = native_weather_at(&store, pf, day, time as f32)?;
        let mut independent = ao_formats::playfield::open_weather(&store, pf)?;
        independent.update(day, time as f64, 1.0 / 60.0);
        assert_eq!(fresh, independent.state(), "timestamp={time}");
        differs |= fresh.fields != reused.state().fields;
    }
    assert!(differs, "native frame-timer reuse must differ from independent timestamp sampling");
    Ok(())
}

#[test]
fn weather_fixture_profiles_are_one_switch_only() {
    let default = ao_render::EnhancedSettings::default();
    assert!(!default.any_enabled());
    for profile in ["rain", "wet-surfaces", "weather-particles", "spell-particles", "fluid-smoke"] {
        let mut settings = graphics_fixture_settings(profile).unwrap();
        let switches = [settings.enhanced_rain, settings.wet_surfaces, settings.weather_particles,
            settings.gpu_spell_particles, settings.fluid_smoke];
        assert_eq!(switches.into_iter().filter(|enabled| *enabled).count(), 1);
        settings.enhanced_rain = false;
        settings.wet_surfaces = false;
        settings.weather_particles = false;
        settings.gpu_spell_particles = false;
        settings.fluid_smoke = false;
        assert_eq!(settings, default);
    }
}
