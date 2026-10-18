//! Preset acceptance uses the existing snapshot cameras and the native preference registry.
use super::*;
use super::super::{dvalue::DValues, enhanced::{GraphicsPreset, apply_preset}};
use anyhow::{Context, ensure};
use std::fmt::Write;

mod marginal;

const SIZE: (u32, u32) = (1920, 1200);
const VIEWS: [&str; 4] = ["icc-landmark-authored", "borealis-foliage-sun-high", "borealis-foliage-night", "newland-day"];
const PRESETS: [GraphicsPreset; 4] = GraphicsPreset::ALL;

#[test]
#[ignore = "installed-client headless preset acceptance; 120 warmup + 600 GPU samples and compact 120-frame interaction sheets"]
fn graphics_preset_acceptance() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a NEW directory")?);
    ensure!(!out.exists(), "refusing to overwrite {}", out.display());
    let selected_view = std::env::var("AOMAC_GFX_VIEW").ok();
    let selected_preset = std::env::var("AOMAC_GFX_PRESET").ok();
    ensure!(selected_view.as_deref().is_none_or(|name| VIEWS.contains(&name)), "unknown matrix AOMAC_GFX_VIEW");
    ensure!(selected_preset.as_deref().is_none_or(|name| name == "combined-all" || GraphicsPreset::from_name(name).is_some()), "unknown AOMAC_GFX_PRESET; use Original, Classic+, Modern, Ultra, combined-all");
    let phase = std::env::var("AOMAC_GFX_ACCEPTANCE_PHASE").unwrap_or_else(|_| "both".into());
    ensure!(matches!(phase.as_str(), "both" | "timing" | "interactions"), "AOMAC_GFX_ACCEPTANCE_PHASE must be timing, interactions, or both");
    ensure!(phase != "timing" || selected_preset.as_deref() != Some("combined-all"), "combined-all is an interactions profile, not a measured preset");
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "status=running\nvisual_acceptance=unexercised\n")?;
    let result = capture(&out, selected_view.as_deref(), selected_preset.as_deref(), &phase);
    std::fs::write(out.join("status.txt"), match &result {
        Ok(()) => "status=complete\nvisual_acceptance=unexercised\n".to_owned(),
        Err(error) => format!("status=failed\nerror={error:#}\nvisual_acceptance=unexercised\n"),
    })?;
    result
}

fn capture(out: &std::path::Path, selected_view: Option<&str>, selected_preset: Option<&str>, phase: &str) -> anyhow::Result<()> {
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let actor = graphics_fixture_actor()?;
    let weather = ao_formats::weather::State::clear();
    let combined_weather = if phase != "timing" && selected_preset.is_none_or(|profile| profile == "combined-all") {
        super::weather_shots::native_weather_cases(&store, &[0, 3, 4, 5, 6])?
    } else { Vec::new() };
    let mut manifest = format!("fixture=graphics_preset_acceptance\nsize=1920x1200\nmsaa_requested=4\noriginal_msaa_override=4_for_fair_comparison\nwarmup=120\nsamples=600\nunits=milliseconds\npercentile=nearest_rank_ceil_np\ngame_day=0\ncrt_seed=1\nweather={weather:?}\nsimulation_dt=1/60\ngame_clock=frozen_authored_snapshot\ntiming=GPU_frame_timestamp_not_wall_time\nretained_rgba=false\nvisual_acceptance=unexercised\nshader_dir={:?}\nactor_source=docs/captures/zone_ithaca.rec\nactor_id=25988\n", std::env::var_os("AOMAC_SHADER_DIR"));
    for (label, program, args) in [
        ("revision", "git", vec!["rev-parse".into(), "HEAD".into()]),
        ("source_diff", "git", vec!["diff".into(), "--no-ext-diff".into(), "HEAD".into()]),
        ("resource_sha256", "shasum", vec!["-a".into(), "256".into(), dir.join("cd_image/rdb.db").into_os_string()]),
    ] {
        let output = std::process::Command::new(program).args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
        ensure!(output.status.success(), "{program} failed");
        writeln!(manifest, "{label}={:?}", String::from_utf8(output.stdout)?.trim())?;
    }
    let mut summary = String::from("view,preset,pf,game_time,width,height,msaa_actual,anisotropy,warmup,samples,p50_ms,p95_ms\n");
    let mut frames = String::from("view,preset,sample,gpu_frame_ms\n");
    let mut temporal = String::from("view,preset,frame,mean_abs_rgb_codes,max_rgb_codes,pixels_gt2,different_rgba_bytes\n");
    let mut beauties = image::RgbaImage::new(4 * 384, 4 * 240);
    for (name, pf, time, fixed) in graphics_fixture_views(&actor).into_iter().filter(|(name, _, _, _)| {
        VIEWS.contains(&name.as_str()) && selected_view.is_none_or(|selected| selected == name)
    }) {
        ao_scene::crt_srand(1);
        let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
        let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
        super::super::player::bake_sky_open(&collision, &mut scene);
        let fixed = if pf == 566 { graphics_fixture_newland_camera(|point| collision.terrain_height(point))? } else { fixed };
        let collision = std::rc::Rc::new(std::cell::RefCell::new(collision));
        let exposure = super::super::player::grass_exposure_surface(collision.clone());
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        writeln!(manifest, "view={name} pf={pf} game_time={time} camera={fixed:?} lens={lens:?} meshes={} instances={} environment={:?} daylight={:?}", scene.meshes.len(), scene.instances.len(), scene.environment, scene.daylight)?;
        for (column, choice) in PRESETS.into_iter().map(Some).chain(std::iter::once(None)).enumerate().filter(|(_, choice)| {
            let profile = choice.map_or("combined-all", GraphicsPreset::name);
            selected_preset.is_none_or(|selected| selected == profile) && (choice.is_some() || phase != "timing")
        }) {
            let combined = choice.is_none();
            let preset = choice.unwrap_or(GraphicsPreset::Ultra);
            let profile = if combined { "combined-all" } else { preset.name() };
            let mut values = DValues::new(&dir);
            apply_preset(&mut values, preset);
            if combined {
                use super::super::{dvalue::Variant, enhanced::{controls, graphical, ControlKind}};
                for control in controls().into_iter().filter(|control| graphical(control.name) && matches!(control.kind, ControlKind::Check)) {
                    values.set_raw(control.name, Variant::Bool(true));
                }
                values.set_raw("AomacMotionBlur", Variant::Float(0.15));
            }
            let settings = values.enhanced_settings();
            ao_scene::crt_srand(1);
            let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
            let mut off = Offscreen::evidence_timing(&shot, SIZE)?;
            off.fixed_camera(Some(fixed));
            off.host.msaa_samples = Some(4);
            off.host.anisotropy = Some(preset.anisotropy());
            off.host.texture_policy = Some((2, false, values.flag("AomacBetterMipFiltering")));
            off.host.fog_density_scale = Some(1.0);
            off.host.enhanced_settings = Some(settings);
            off.host.grass_exposure = Some(exposure.clone());
            off.host.grass_wind = weather.wind;
            off.frame(&mut shot, 0.0);
            off.wait_for_pipeline_warmup();
            ensure!(off.msaa_samples() == 4, "{name}/{profile}: actual MSAA {} != 4", off.msaa_samples());
            off.warm_grass_tiles();
            let measured = phase != "interactions" && !combined;
            if measured {
                off.set_gpu_timing(true)?;
                let mut samples = Vec::with_capacity(600);
                for frame in 0..720 {
                    off.frame(&mut shot, 1.0 / 60.0);
                    let ms = off.read_frame_gpu_ms()?.context("GPU frame timestamp missing")?;
                    ensure!(ms.is_finite() && ms > 0.0, "invalid GPU frame timestamp {ms}");
                    if frame >= 120 {
                        writeln!(frames, "{name},{profile},{},{ms:.9}", frame - 120)?;
                        samples.push(ms);
                    }
                }
                ensure!(samples.len() == 600 && off.msaa_samples() == 4, "incomplete or wrong-quality timing phase");
                off.set_gpu_timing(false)?;
                samples.sort_by(f64::total_cmp);
                writeln!(summary, "{name},{profile},{pf},{time},1920,1200,{},{},120,{},{:.9},{:.9}", off.msaa_samples(), preset.anisotropy(), samples.len(), samples[299], samples[569])?;
                eprintln!("{name}/{profile}: samples=600 GPU p50={:.3}ms p95={:.3}ms", samples[299], samples[569]);
            } else {
                for _ in 0..120 { off.frame(&mut shot, 1.0 / 60.0); }
            }
            let list = off.frame(&mut shot, 0.0);
            let beauty = off.rgba(&shot, &list)?;
            image::save_buffer(out.join(format!("{name}-{profile}-beauty.png")), &beauty, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
            if !combined {
                let row = VIEWS.iter().position(|view| *view == name).context("matrix row")?;
                image::imageops::replace(&mut beauties, &thumbnail(&beauty)?, column as i64 * 384, row as i64 * 240);
                beauties.save(out.join("beauties.png"))?;
            }
            writeln!(manifest, "completed_view={name} preset={profile:?} phase={phase} settings={settings:?} msaa_actual={} anisotropy={} gpu_samples={} enhanced_target_bytes={} grass_metrics={:?}", off.msaa_samples(), preset.anisotropy(), if measured { 600 } else { 0 }, off.enhanced_target_bytes(), off.evidence_grass_metrics())?;
            // Checkpoint measured results before the slower readback/interaction checks.
            std::fs::write(out.join("frames.csv"), &frames)?;
            std::fs::write(out.join("summary.csv"), &summary)?;
            std::fs::write(out.join("manifest.txt"), &manifest)?;
            if phase == "timing" { continue; }
            let placement = {
                let collision = collision.borrow();
                graphics_fixture_post_avatar(fixed, |point| collision.terrain_height(point), |eye, target| collision.line(eye, target).is_none())?
            };
            let mut avatar = super::super::avatar::Avatar::new(&store, &dir, 25988, &actor, ao_formats::character::actor::ActorAssets::new(&store)?)?;
            avatar.set_transform(placement.1.to_array(), 0.0);
            off.host.actor_models.push((super::super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
            let mut native_effects = Vec::new();
            let mut effect_admitted_frames = [0_usize; 3];
            if combined {
                let surface = super::super::player::weather_surface(collision.clone());
                let mut input = ao_render::WeatherInput { outdoor: true, surface: Some(surface), ..Default::default() };
                for &(channel, source_pf, source_day, source_time, state) in &combined_weather {
                    if channel == 0 { input.rain = state.fields[0]; input.wind = [state.wind[0], state.wind[1], -state.wind[2]]; }
                    else { input.storm[channel - 3] = state.fields[channel]; }
                    writeln!(manifest, "view={name} profile=combined-all controlled_native_weather_channel={channel} source_pf={source_pf} source_day={source_day} source_time={source_time} state={state:?}")?;
                }
                off.host.weather_input = input;
                let source = placement.0.pos + placement.0.forward() * 7.0;
                for (index, kind) in [1009, 1018, 3017].into_iter().enumerate() {
                    let position = source + placement.0.right() * (index as f32 - 1.0);
                    let effect = super::weather_shots::weather_effects::NativeEffect::new(&dir, kind, position)?;
                    writeln!(manifest, "view={name} profile=combined-all source_position={position:?} {}", effect.metadata())?;
                    native_effects.push(effect);
                }
                writeln!(manifest, "view={name} profile=combined-all stimuli=controlled_combination_of_unmodified_native_weather_channels,authored_Smoke1009,Sparks1018,GlobalSmoke3017,visible_actor_motion collision_surface=true native_liquid_surfaces={} underwater_stimulus=false water_wake_stimulus=false user_lut_asset=false water_visibility=not_asserted mutually_exclusive_night=Moonlit", scene.liquids.len())?;
            }
            let mut sheet = image::RgbaImage::new(4 * 384, 3 * 240);
            let mut previous: Option<Vec<u8>> = None;
            for frame in 0..120 {
                let root = placement.1 + Vec3::X * (frame as f32 / 119.0 - 0.5);
                avatar.set_transform(root.to_array(), 0.0);
                off.host.actors.push(avatar.frame());
                off.host.dynamic_lights.extend(avatar.lights());
                off.host.post_focus = Some(root.to_array());
                off.host.grass_benders.clear();
                off.host.grass_benders.push(ao_render::GrassBender { actor: Some(25988), position: root.to_array() });
                let mut moving = placement.0;
                moving.yaw += frame as f32 * 0.0005;
                off.fixed_camera(Some(moving));
                for (index, effect) in native_effects.iter_mut().enumerate() {
                    let (actors, resets, retired) = (off.host.actors.len(), off.host.actor_visual_resets.len(), off.host.actor_visual_retirements.len());
                    effect.frame(1.0 / 60.0, &mut off.host)?;
                    // Independent native fixture renderers start at the same actor ID.
                    // Namespace only their submitted visuals; authored controls stay untouched.
                    let offset = (index as u32 + 1) * 0x0010_0000;
                    for actor in &mut off.host.actors[actors..] { actor.id = actor.id.checked_add(offset).context("effect fixture actor ID overflow")?; }
                    for (first, _) in off.host.actor_visual_resets[resets..].iter_mut().chain(off.host.actor_visual_retirements[retired..].iter_mut()) {
                        *first = first.checked_add(offset).context("effect fixture retired ID overflow")?;
                    }
                    effect_admitted_frames[index] += usize::from(off.host.actors[actors..].iter().any(|actor| off.host.previous_visible_actors.contains(&actor.id)));
                }
                let list = off.frame(&mut shot, 1.0 / 60.0);
                let raw = off.rgba(&shot, &list)?;
                if let Some(previous) = &previous {
                    let (mean, max, over_two, different) = consecutive_diff(previous, &raw);
                    writeln!(temporal, "{name},{profile},{frame},{mean:.9},{max},{over_two},{different}")?;
                }
                if frame % 10 == 9 {
                    let index = frame / 10;
                    image::imageops::replace(&mut sheet, &thumbnail(&raw)?, (index % 4) as i64 * 384, (index / 4) as i64 * 240);
                }
                previous = Some(raw);
            }
            sheet.save(out.join(format!("{name}-{profile}-temporal120.png")))?;
            for (index, effect) in native_effects.iter().enumerate() {
                ensure!(effect_admitted_frames[index] > 0, "combined native effect {index} never admitted");
                writeln!(manifest, "view={name} profile=combined-all native_effect_index={index} admitted_frames={} {}", effect_admitted_frames[index], effect.metadata())?;
            }
            fog_and_admission(&dir, &scene, placement.0, lens, settings, &avatar, exposure.clone())
                .with_context(|| format!("{name}/{profile} fog/admission"))?;
            writeln!(manifest, "checked_view={name} preset={profile:?} temporal_frames=120 temporal_pairs=119 sheet_frames=9,19,29,39,49,59,69,79,89,99,109,119 temporal_dt=1/60 camera_yaw_per_frame=0.0005 actor_path_m=1 invariant_resolution=384x240 fully_fogged_colours_equal=true unfogged_colours_differ=true non_admitted_equals_absent=true")?;
            std::fs::write(out.join("temporal.csv"), &temporal)?;
            std::fs::write(out.join("manifest.txt"), &manifest)?;
        }
    }
    // No raw frames survive: enforce the handoff budget, including encoded beauty PNGs.
    let bytes = std::fs::read_dir(out)?.try_fold(0_u64, |sum, entry| -> std::io::Result<u64> { Ok(sum + entry?.metadata()?.len()) })?;
    ensure!(bytes <= 100_000_000, "retained evidence {bytes} bytes exceeds 100 MB");
    Ok(())
}

fn thumbnail(raw: &[u8]) -> anyhow::Result<image::RgbaImage> {
    let full = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(SIZE.0, SIZE.1, raw).context("RGBA dimensions")?;
    Ok(image::imageops::resize(&full, 384, 240, image::imageops::FilterType::Triangle))
}

fn consecutive_diff(previous: &[u8], current: &[u8]) -> (f64, u8, usize, usize) {
    let (mut sum, mut max, mut over_two, mut different) = (0_u64, 0_u8, 0_usize, 0_usize);
    for (a, b) in previous.as_chunks::<4>().0.iter().zip(current.as_chunks::<4>().0.iter()) {
        let mut pixel_max = 0;
        for channel in 0..4 {
            let delta = a[channel].abs_diff(b[channel]);
            different += usize::from(delta != 0);
            if channel < 3 { sum += u64::from(delta); max = max.max(delta); pixel_max = pixel_max.max(delta); }
        }
        over_two += usize::from(pixel_max > 2);
    }
    (sum as f64 / (previous.len() / 4 * 3) as f64, max, over_two, different)
}

fn fog_and_admission(dir: &std::path::Path, scene: &Scene, fixed: Camera, lens: Lens, settings: ao_render::EnhancedSettings,
    avatar: &super::super::avatar::Avatar, exposure: ao_render::GrassExposure) -> anyhow::Result<()> {
    // Existing night regression's saturated-body positive control, applied to every preset.
    let render = |fog: bool, body: u8| -> anyhow::Result<Vec<u8>> {
        let mut snapshot = scene.clone();
        snapshot.fog_model = None;
        let environment = snapshot.environment.as_mut().context("fixture environment missing")?;
        environment.fog_start = if fog { 0.0 } else { 10_000.0 };
        environment.fog_end = if fog { 0.01 } else { 20_000.0 };
        for mesh in &mut snapshot.meshes { for material in &mut mesh.submeshes { material.fog_enabled = true; } }
        ao_scene::crt_srand(1);
        let mut shot = Shot { gui: Gui::new(dir, None)?, scene: Some(snapshot), camera: fixed, lens };
        let mut off = Offscreen::evidence(&shot, (384, 240))?;
        off.fixed_camera(Some(fixed));
        off.host.msaa_samples = Some(4);
        off.host.enhanced_settings = Some(settings);
        off.host.fog_density_scale = Some(1.0);
        off.host.grass_exposure = Some(exposure.clone());
        off.frame(&mut shot, 0.0);
        if body != 0 {
            let mut model = avatar.model().clone();
            for mesh in &mut model.meshes { for material in &mut mesh.submeshes {
                material.texture = None;
                material.base_color = if body == 3 { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] };
                material.fog_enabled = true;
                material.overlay = false;
                material.emissive = [1.0; 3];
                material.specular = [0.0; 3];
            } }
            off.host.actor_models.push((super::super::avatar::MODEL_KEY, std::sync::Arc::new(model)));
            let mut frame = avatar.frame();
            frame.id = 25988;
            frame.emissive = None;
            frame.part_materials.clear();
            frame.alpha = if body == 1 { 0.0 } else { 1.0 };
            let [x, y, z, _] = frame.transform[3];
            off.host.grass_benders.push(ao_render::GrassBender { actor: Some(25988), position: [x, y, z] });
            off.host.actors.push(frame);
        }
        let list = off.frame(&mut shot, 0.0);
        off.warm_grass_tiles();
        off.rgba(&shot, &list)?;
        off.wait_for_pipeline_warmup();
        off.rgba(&shot, &list)
    };
    ensure!(render(false, 0)? == render(false, 1)?, "non-admitted body/bender leaks a signal");
    ensure!(render(false, 2)? != render(false, 3)?, "unfogged body positive control invisible");
    ensure!(render(true, 2)? == render(true, 3)?, "fully fogged body colour leaks through extinction");
    Ok(())
}

#[test]
fn preset_matrix_contract() {
    assert_eq!(VIEWS.len() * PRESETS.len(), 16);
    assert_eq!(consecutive_diff(&[10, 20, 30, 255], &[11, 24, 30, 255]), (5.0 / 3.0, 4, 1, 2));
    for preset in PRESETS {
        assert_eq!(graphics_fixture_settings(preset.name()).unwrap(), {
            let mut values = DValues::new(&ao_gui::client_dir());
            apply_preset(&mut values, preset);
            values.enhanced_settings()
        });
    }
}
