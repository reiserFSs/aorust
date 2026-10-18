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
        writeln!(manifest, "all_off_reference=same_native_snapshot_weather_composition")?;
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
            ("rain-splashes", "rain", 0, false, false, 0),
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
            *kind == 0 && selected.as_ref().is_none_or(|selected| selected.as_str() == *name)
        }).map(|(_, _, channel, _, _, _)| *channel).collect();
        let native = native_weather_cases(&store, &required)?;
        for (name, profile, channel, roof, indoor, kind) in cases {
            if selected.as_ref().is_some_and(|selected| selected != name) { continue; }
            selected_count += 1;
            // A 0.25 s step aliases the crown shader's four-Hz age cycle.
            let frame_dt = if name == "rain-splashes" { 1.0 / 60.0 } else { DT };
            writeln!(manifest, "case={name} dt={frame_dt}")?;
            ensure!(started.elapsed().as_secs() < 270, "weather acceptance exceeded 270-second render budget");
            let (mut pf, game_day, time, state) = if kind != 0 {
                (800, 0, 3240.0, native_weather_at(&store, 800, 0, 3240.0)?)
            } else {
                native.iter().find(|(field, _, _, _, _)| *field == channel).map(|(_, pf, day, time, state)| (*pf, *day, *time, *state)).context("missing native weather channel")?
            };
            let weather_pf = pf;
            let mut fixed = Camera::look_at([679.6, 74.5, -476.7].into(), [679.6, 73.0, -486.7].into());
            if indoor { pf = 127; fixed = Camera::look_at([96.0, 109.305, -242.0].into(), [108.0, 108.9, -242.0].into()); }
            let controlled_native_input = name == "rain-water";
            if controlled_native_input {
                // ICC's own searched schedule is dry: this is explicitly a controlled
                // native-rain input, not evidence that the ICC climate produces rain.
                pf = 4582;
                ensure!(weather_pf != pf && state.fields[0] >= 0.8, "water control requires a different native rainy playfield with precipitation >=0.8");
                fixed = Camera { pos: [1010.4, 24.0, -663.1].into(), yaw: 2.4288, pitch: -0.8, roll: 0.0 };
            }
            ao_scene::crt_srand(1);
            let (mut scene, report) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, game_day, Some(state))?;
            if controlled_native_input {
                ensure!(!scene.liquids.is_empty(), "ICC controlled ring capture requires actual native liquid surfaces");
                writeln!(manifest, "case={name} source_weather_pf={weather_pf} render_pf={pf} controlled_native_input=true native_precipitation={} native_fields={:?} native_wind={:?} native_liquids={} water_switches=all_off ring_visual_acceptance=inspect_water_not_generic_streak_delta", state.fields[0], state.fields, state.wind, scene.liquids.len())?;
                let (low, high) = scene.liquids.iter().filter(|liquid| matches!(liquid.style, 0 | 4))
                    .flat_map(|liquid| liquid.triangles()).flatten()
                    .fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(low, high), vertex| {
                        let p = Vec3::from_array(vertex.pos); (low.min(p), high.max(p))
                    });
                writeln!(manifest, "case={name} native_water_bounds_low={low:?} native_water_bounds_high={high:?} centre_ray_at_high_water={:?} centre_ray_at_low_water={:?}",
                    fixed.pos + fixed.forward() * ((high.y - fixed.pos.y) / fixed.forward().y),
                    fixed.pos + fixed.forward() * ((low.y - fixed.pos.y) / fixed.forward().y))?;
            }
            let collision = Rc::new(RefCell::new(ao_formats::playfield::collision::Collision::load(&store, pf)?));
            super::super::player::bake_sky_open(&collision.borrow(), &mut scene);
            let surface = super::super::player::weather_surface(collision.clone());
            if controlled_native_input {
                if let Some(triangle) = scene.liquids.iter().filter(|liquid| matches!(liquid.style, 0 | 4))
                    .flat_map(|liquid| liquid.triangles()).next() {
                    let level = triangle[0].pos[1];
                    let hit = fixed.pos + fixed.forward() * ((level - fixed.pos.y) / fixed.forward().y);
                    writeln!(manifest, "case={name} centre_native_water_plane={hit:?} plane_collision={:?}", surface(hit.to_array()))?;
                }
            }
            if !indoor && kind == 0 && !controlled_native_input {
                // Keep the evidence camera on native level ground, looking along a clear
                // low-terrain corridor. A merely exposed spawn can face straight into a hill.
                let mut selected = None;
                let origin = if pf == 800 { [679.6, 74.5, -476.7] } else if pf == 566 { [414.0, 40.0, -320.0] } else { scene.spawn.context("native playfield has no fixture spawn")? };
                'search: for radius in 0..=64_i32 { for z in -radius..=radius { for x in -radius..=radius {
                    if x.abs().max(z.abs()) != radius { continue; }
                    let p = [origin[0] + x as f32 * 4.0, origin[1], origin[2] + z as f32 * 4.0];
                    let Some([top, ground]) = surface(p) else { continue; };
                    if !ground.is_finite() || (roof && top <= ground + 2.5) || (!roof && top >= -1e19) { continue; }
                    let eye = Vec3::new(p[0], ground + 1.7, p[2]);
                    if roof {
                        if !covered_ground_patch(eye, surface.as_ref()) { continue; }
                        selected = Some((eye, -Vec3::Y, top, ground)); break 'search;
                    }
                    if !roof && ![[4.0, 0.0], [-4.0, 0.0], [0.0, 4.0], [0.0, -4.0]].into_iter().all(|[dx, dz]| {
                        surface([p[0] + dx, eye.y, p[2] + dz]).is_some_and(|[roof, y]| roof < -1e19 && (y - ground).abs() <= 0.6)
                    }) { continue; }
                    for heading in 0..8 {
                        let angle = heading as f32 * std::f32::consts::FRAC_PI_4;
                        let forward = Vec3::new(angle.sin(), 0.0, -angle.cos());
                        let side = Vec3::new(-forward.z, 0.0, forward.x);
                        let distances: &[f32] = &[4.0, 8.0, 16.0, 32.0, 64.0];
                        let clear = distances.iter().all(|&distance| {
                            [-0.25, 0.0, 0.25].into_iter().all(|width| {
                                let probe = eye + forward * distance + side * (width * distance);
                                surface(probe.to_array()).is_some_and(|[top, y]| {
                                    y <= ground + 0.6 && top < -1e19 && (distance > 8.0 || y >= ground - 1.0)
                                }) && collision.borrow().line(eye.to_array(), probe.to_array()).is_none()
                            })
                        });
                        if clear { selected = Some((eye, forward, top, ground)); break 'search; }
                    }
                } } }
                let (eye, forward, roof_height, ground_height) = selected.with_context(|| format!("{name}: no native collision {} within bounded scan pf{pf}", if roof { "covered ground patch" } else { "open level-ground clear corridor" }))?;
                fixed = if roof {
                    // A roof above the eye does not shelter the outdoor corridor in
                    // front of it. Look only at the proven covered ground footprint.
                    Camera { pos: eye, yaw: 0.0, pitch: -std::f32::consts::FRAC_PI_2, roll: 0.0 }
                } else { Camera::look_at(eye, eye + forward * 16.0 - Vec3::Y * 1.6) };
                writeln!(manifest, "case={name} camera_selection={} roof={roof_height} ground={ground_height} eye_height=1.7 forward={forward:?} shelter_patch_half_extent={} ground_relief={}", if roof { "native_covered_ground_downward" } else { "native_level_ground_clear_corridor" }, if roof { 3.0 } else { 0.0 }, if roof { 0.3 } else { 0.6 })?;
            }
            if name == "rain-splashes" { fixed.pitch = -0.7; }
            ensure!(report.dungeon == indoor, "{name}: native dungeon identity mismatch");
            let sampled = surface(fixed.pos.to_array());
            ensure!(indoor || sampled.is_some(), "{name}: outdoor camera collision unknown");
            if controlled_native_input {
                ensure!(sampled.is_some_and(|[roof, ground]| roof < -1e19 && ground.is_finite()), "controlled water camera needs actual native open-sky ground proof");
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
            let supplied = ao_render::WeatherInput {
                rain: if channel == 0 { state.fields[0] } else { 0.0 },
                storm: std::array::from_fn(|i| if channel == i + 3 { state.fields[i + 3] } else { 0.0 }),
                wind: [state.wind[0], state.wind[1], -state.wind[2]],
                outdoor: !report.dungeon, surface: Some(surface.clone()),
            };
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
                // Last A has absent input during warmup and timed frames. Alternate
                // input is tested only by the untimed composition reference below.
                let input = if phase == 3 { ao_render::WeatherInput::default() } else { supplied.clone() };
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
                    off.host.weather_input = current.clone();
                    if let Some(effect) = &mut effect { effect.frame(frame_dt, &mut off.host)?; }
                    off.frame(&mut shot, frame_dt);
                    let gpu = off.read_frame_gpu_ms()?.context("GPU timestamp unavailable")?;
                    let cpu = off.frame_cpu_timings().context("CPU timing unavailable")?;
                    ensure!(gpu.is_finite() && gpu >= 0.0, "invalid GPU sample");
                    timings.push(gpu);
                    writeln!(csv, "{name},{phase},{enabled},{frame},{frame_dt},{},{},{},{},{},{},{},{},{gpu},{},{},{}", current.rain, current.storm[0], current.storm[1], current.storm[2], current.storm[3], current.wind[0], current.wind[1], current.wind[2], cpu.prepare_ms, cpu.encode_ms, cpu.queue_submit_ms)?;
                    if let Some(column) = [0, 31, 63, 95].iter().position(|&f| f == frame) {
                        let raw = off.frozen_rgba()?;
                        if !enabled {
                            let alternate = if phase == 0 { ao_render::WeatherInput::default() } else { supplied.clone() };
                            let reference = off.weather_off_reference_rgba(alternate)?;
                            ensure!(raw == reference, "{name} phase{phase} frame{frame}: all-off supplied/absent weather changed the same native snapshot");
                        }
                        if phase == 0 { baseline_frames.push(raw.clone()); }
                        if roof && enabled {
                            ensure!(raw == baseline_frames[column], "{name} frame{frame}: rain changed the collision-proven sheltered ground viewport");
                        }
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
            if indoor || roof { ensure!(visible_delta == 0, "{name}: enhanced weather changed the sheltered viewport"); }
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

/// The downward 90-degree horizontal lens sees at most ±2 m here: the eye is
/// 1.7 m above ground and the patch permits 0.3 m of relief. Include a full
/// metre of margin and test the renderer's actual 0.5 m shelter texel centres,
/// not just the eye's collision ray. A roof must also clear the whole eye.
fn covered_ground_patch(eye: Vec3, surface: &dyn Fn([f32; 3]) -> Option<[f32; 2]>) -> bool {
    let ground = eye.y - 1.7;
    let x0 = ((eye.x - 3.0) * 2.0).floor() as i32;
    let x1 = ((eye.x + 3.0) * 2.0).ceil() as i32;
    let z0 = ((eye.z - 3.0) * 2.0).floor() as i32;
    let z1 = ((eye.z + 3.0) * 2.0).ceil() as i32;
    (z0..=z1).all(|z| (x0..=x1).all(|x| {
        surface([x as f32 * 0.5 + 0.25, eye.y, z as f32 * 0.5 + 0.25])
            .is_some_and(|[roof, y]| roof.is_finite() && y.is_finite() && roof > eye.y + 0.3 && (y - ground).abs() <= 0.3)
    }))
}

#[test]
fn weather_roof_fixture_requires_covered_visible_ground() {
    let eye = Vec3::new(0.1, 1.7, -0.1);
    assert!(covered_ground_patch(eye, &|_| Some([4.0, 0.0])));
    assert!(!covered_ground_patch(eye, &|p| Some([if p[0] > 1.0 { -1e20 } else { 4.0 }, 0.0])),
        "a roof over the eye cannot certify exposed ground in view");
    assert!(!covered_ground_patch(eye, &|p| if p[2] < -1.0 { None } else { Some([4.0, 0.0]) }),
        "unknown visible shelter cannot certify exact exclusion");
    assert!(!covered_ground_patch(eye, &|_| Some([4.0, -2.0])),
        "low ground expands the footprint beyond its proof");
}

/// Sample a timestamp independently: native Area timers count frames, not elapsed day time.
fn native_weather_at(store: &ao_rdb::RecordStore, pf: u32, day: u32, time: f32) -> anyhow::Result<ao_formats::weather::State> {
    let mut weather = ao_formats::playfield::open_weather(store, pf)?;
    weather.update(day, time as f64, 1.0 / 60.0);
    Ok(weather.state())
}

type NativeWeatherCase = (usize, u32, u32, f32, ao_formats::weather::State);

/// Bounded native schedule discovery, without overwriting native channels or wind.
fn native_weather_cases(store: &ao_rdb::RecordStore, required: &[usize]) -> anyhow::Result<Vec<NativeWeatherCase>> {
    let mut found = Vec::new();
    if required.is_empty() { return Ok(found); }
    let mut ids = vec![800, 566, 4582];
    ids.extend(ao_formats::playfield::list_playfields(store)?.into_iter().map(|(pf, _)| pf).filter(|pf| ![800, 566, 4582].contains(pf)));
    let mut outdoors = Vec::new();
    for pf in ids.drain(..) {
        if !ao_formats::playfield::zone_locator(store, pf)?.is_dungeon() { outdoors.push(pf); }
    }
    for day in 0..32 {
        for &pf in &outdoors {
            for time in (0..6480).step_by(30) {
                let state = native_weather_at(store, pf, day, time as f32)?;
                for &field in required {
                    if state.fields[field] > (if field == 0 { 0.8 } else { 0.4 }) && !found.iter().any(|(previous, _, _, _, _)| *previous == field) { found.push((field, pf, day, time as f32, state)); }
                }
                if required.iter().all(|field| found.iter().any(|(previous, _, _, _, _)| previous == field)) { return Ok(found); }
            }
        }
    }
    anyhow::bail!("native weather days0..32 lack requested channels {required:?}; found {:?}", found.iter().map(|(field, pf, day, time, _)| (*field, *pf, *day, *time)).collect::<Vec<_>>())
}

#[test]
#[ignore = "installed-client CPU native timestamp sampling regression; no GPU"]
fn weather_fixture_samples_fresh_native_timestamps() -> anyhow::Result<()> {
    let store = ao_rdb::RecordStore::open(&ao_gui::client_dir())?;
    let (_, pf, day, _, _) = native_weather_cases(&store, &[0])?[0];
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
