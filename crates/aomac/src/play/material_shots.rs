//! Real immutable fixture snapshots; no material overrides or synthetic scene receivers.
use super::{camera, graphics_fixture_actor, graphics_fixture_settings, graphics_fixture_views, Shot};
use anyhow::{ensure, Context};
use ao_render::{Camera, Offscreen, Vec3};
use glam::Mat4;
use std::{cell::RefCell, fmt::Write, path::PathBuf, rc::Rc};

#[test]
#[ignore = "real 1920x1200 4xMSAA material evidence; installed client, explicit output/profile and GPU lease required"]
fn real_material_evidence() -> anyhow::Result<()> {
    const SIZE: (u32, u32) = (1920, 1200);
    const SAMPLES: usize = 60;
    let profile = std::env::var("AOMAC_MATERIAL_PROFILE").context("set AOMAC_MATERIAL_PROFILE to generated-normals, generated-normals-ggx, material-maps, material-pbr, wet-reflections, or environment-probe (one lease each)")?;
    ensure!(["generated-normals", "generated-normals-ggx", "material-maps", "material-pbr", "wet-reflections", "environment-probe"].contains(&profile.as_str()), "unknown material evidence profile");
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set explicit AOMAC_GFX_OUT to a new directory")?);
    ensure!(out.is_absolute() && !out.exists(), "output must be absolute and new");
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "status=running\nvisual_acceptance=unexercised\n")?;
    let result = (|| -> anyhow::Result<()> {
        let started = std::time::Instant::now();
        let dir = ao_gui::client_dir();
        let store = ao_rdb::RecordStore::open(&dir)?;
        let view = if profile == "environment-probe" { "icc-platform-authored" } else { "borealis-street-sun-high" };
        let (_, pf, time, mut fixed) = graphics_fixture_views(&graphics_fixture_actor()?).into_iter().find(|(name, _, _, _)| name == view).context("real fixture camera missing")?;
        // Keep the existing street eye/yaw; tilt down only for the native wet floor.
        if profile == "wet-reflections" { fixed.pitch = -0.25; }
        ao_scene::crt_srand(1);
        let (mut scene, report) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(ao_formats::weather::State::clear()))?;
        let collision = Rc::new(RefCell::new(ao_formats::playfield::collision::Collision::load(&store, pf)?));
        super::super::player::bake_sky_open(&collision.borrow(), &mut scene);
        let surface = super::super::player::weather_surface(collision.clone());
        let mut weather = ao_render::WeatherInput { outdoor: !report.dungeon, surface: Some(surface.clone()), ..Default::default() };
        let mut manifest = format!("test=real_material_evidence\nprofile={profile}\nview={view}\npf={pf}\ngame_time={time}\ncamera={fixed:?}\nsize=1920x1200\nmsaa_requested=4\norder=ABBA\nsamples_per_phase={SAMPLES}\nmetric=whole_frame_gpu_timestamp_ms\nmaterial_overrides=none\nactors=none\nvisual_acceptance=unexercised\n");
        if profile == "wet-reflections" {
            let mut native = None;
            'days: for day in 0..32 { for tick in 0..54 {
                let at = tick as f32 * 100.0;
                let mut source = ao_formats::playfield::open_weather(&store, pf)?;
                source.update(day, at as f64, 1.0 / 60.0);
                let state = source.state();
                if state.fields[0] >= 0.8 { native = Some((day, at, state)); break 'days; }
            } }
            let (day, at, state) = native.context("Borealis native rainy schedule not found; never substitute fake wetness")?;
            weather.rain = state.fields[0];
            weather.wind = [state.wind[0], state.wind[1], -state.wind[2]];
            writeln!(manifest, "rain_source_pf={pf}\nrain_source_day={day}\nrain_source_time={at}\nnative_weather={state:?}\nweather_policy=native_rain_input_on_frozen_sunhigh_snapshot\nwetness_policy=physical_accumulation_120_steps_dt_0.25_then_freeze\ncollision_at_eye={:?}", surface(fixed.pos.to_array()))?;
            writeln!(manifest, "rain_capture=0\nrain_stop_transition_dt=0\nwetness_capture=retained_native_accumulation_after_rain")?;
            ensure!(surface(fixed.pos.to_array()).is_some(), "wet floor needs native collision");
            let floor = surface((fixed.pos + fixed.forward() * 3.0).to_array());
            writeln!(manifest, "visible_floor_collision={floor:?}")?;
            ensure!(floor.is_some_and(|[roof, ground]| roof < -1e19 && ground.is_finite()),
                "wet reflection camera must see a native open-sky floor");
        }
        let roof_camera = if profile == "wet-reflections" {
            let mut covered = None;
            'roof: for z in -64..=64 { for x in -64..=64 {
                let point = fixed.pos + Vec3::new(x as f32, 0.0, z as f32);
                let Some([roof, ground]) = surface(point.to_array()) else { continue };
                let eye = Vec3::new(point.x, ground + 1.7, point.z);
                if roof <= eye.y + 0.3 || !roof.is_finite() { continue; }
                if (-6..=6).all(|dz| (-6..=6).all(|dx| {
                    surface((eye + Vec3::new(dx as f32 * 0.5, 0.0, dz as f32 * 0.5)).to_array())
                        .is_some_and(|[top, y]| top > eye.y + 0.3 && (y - ground).abs() <= 0.3)
                })) {
                    covered = Some(Camera { pos: eye, yaw: 0.0, pitch: -std::f32::consts::FRAC_PI_2, roll: 0.0 });
                    break 'roof;
                }
            } }
            Some(covered.context("native Borealis covered floor control not found")?)
        } else { None };
        writeln!(manifest, "native_roof_control_camera={roof_camera:?}")?;
        let root = std::env::temp_dir().join(format!("aomac-real-material-{}", std::process::id()));
        ensure!(!root.exists(), "temporary material root already exists");
        std::fs::create_dir(&root)?;
        let scratch = MaterialScratch(root);
        if profile == "material-maps" {
            // Rank real visible wall textures by projected triangle area, not
            // nearest triangle: a tiny foreground trim is poor channel evidence.
            let mut areas = std::collections::HashMap::new();
            let up = fixed.right().cross(fixed.forward());
            for instance in &scene.instances {
                let mesh = &scene.meshes[instance.mesh];
                let transform = Mat4::from_cols_array_2d(&instance.transform);
                for sub in &mesh.submeshes {
                    if sub.blend != ao_scene::Blend::Opaque || !sub.lighting_enabled { continue; }
                    let Some(key) = sub.texture.filter(|key| key.rdb_type == 1_010_004) else { continue };
                    for triangle in sub.indices.as_chunks::<3>().0 {
                        let points = triangle.map(|i| transform.transform_point3(Vec3::from(mesh.vertices[i as usize].pos)));
                        let Some(normal) = (points[1] - points[0]).cross(points[2] - points[0]).try_normalize() else { continue };
                        if normal.y.abs() > 0.6 { continue; }
                        let center = (points[0] + points[1] + points[2]) / 3.0;
                        let delta = center - fixed.pos;
                        let depth = delta.dot(fixed.forward());
                        if !(1.0..80.0).contains(&depth) || delta.dot(fixed.right()).abs() > depth || delta.dot(up).abs() > depth * 0.625 { continue; }
                        let target = center - delta.normalize() * 0.2;
                        if collision.borrow().line(fixed.pos.to_array(), target.to_array()).is_some() { continue; }
                        let depths = points.map(|point| (point - fixed.pos).dot(fixed.forward()));
                        if depths.iter().any(|&z| z <= 0.5) { continue; }
                        let projected = std::array::from_fn::<_, 3, _>(|i| {
                            let p = points[i] - fixed.pos;
                            [(p.dot(fixed.right()) / depths[i]).clamp(-1.0, 1.0),
                                (p.dot(up) / depths[i]).clamp(-0.625, 0.625)]
                        });
                        let [a, b, c] = projected;
                        let area = ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5;
                        *areas.entry(key).or_insert(0.0_f32) += area;
                    }
                }
            }
            let mut candidates: Vec<_> = areas.into_iter().collect();
            candidates.sort_by(|a, b| b.1.total_cmp(&a.1)
                .then_with(|| a.0.rdb_type.cmp(&b.0.rdb_type)).then_with(|| a.0.id.cmp(&b.0.id)));
            let (key, projected_area) = *candidates.first().context("street view has no visible native RDB wall receiver")?;
            ensure!(projected_area > 0.0, "selected wall must cover a nonzero projected area");
            let normals = image::RgbaImage::from_fn(64, 64, |x, y| {
                let nx = ((x as f32 * std::f32::consts::TAU / 16.0).sin() * 30.0 + 128.0) as u8;
                let ny = ((y as f32 * std::f32::consts::TAU / 16.0).sin() * 30.0 + 128.0) as u8;
                image::Rgba([nx, ny, 248, 255])
            });
            normals.save(scratch.0.join(format!("{}_{}_n.png", key.rdb_type, key.id)))?;
            let orm = image::RgbaImage::from_fn(64, 64, |x, y| {
                let ao = (0.45 + 0.55 * y as f32 / 63.0) * 255.0;
                let roughness = (0.2 + 0.6 * x as f32 / 63.0) * 255.0;
                image::Rgba([ao as u8, roughness as u8, 77, 255])
            });
            orm.save(scratch.0.join(format!("{}_{}_orm.png", key.rdb_type, key.id)))?;
            let emissive = image::RgbaImage::from_fn(64, 64, |x, y| {
                if y % 16 < 2 && x % 32 < 24 { image::Rgba([80, 35, 10, 255]) }
                else { image::Rgba([0, 0, 0, 255]) }
            });
            emissive.save(scratch.0.join(format!("{}_{}_e.png", key.rdb_type, key.id)))?;
            writeln!(manifest, "side_map_rdb_type={}\nside_map_rdb_id={}\nside_map_source=procedurally_authored_normal_orm_emissive\nside_map_selection=largest_summed_projected_visible_native_wall_area\nside_map_projected_area={projected_area}\nside_map_ao_range=0.45,1\nside_map_roughness_range=0.2,0.8\nside_map_metalness=0.3\nside_map_emissive=modest_warm_grooves\nnative_albedo_alpha=unchanged\nside_map_root={}\nsource_assets_committed=false", key.rdb_type, key.id, scratch.0.display())?;
        }
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        let mut csv = String::from("phase,enabled,sample,gpu_ms\n");
        let mut summary = String::new();
        let mut strip = image::RgbaImage::new(960 * 4, 600 * 2);
        let mut before_after = image::RgbaImage::new(SIZE.0 * 2, SIZE.1);
        let mut baseline = Vec::<Vec<u8>>::new();
        let mut changed = 0usize;
        let mut probe_counts = None;
        let mut roof_wet_delta = None;
        for (phase, enabled) in [false, true, true, false].into_iter().enumerate() {
            ensure!(started.elapsed().as_secs() < 270, "material evidence exceeded lease budget");
            ao_scene::crt_srand(1);
            let mut shot = Shot { gui: ao_gui::Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
            let mut off = Offscreen::evidence_timing(&shot, SIZE)?;
            off.set_materials_root(Some(scratch.0.clone()));
            off.fixed_camera(Some(fixed));
            off.host.msaa_samples = Some(4);
            off.host.anisotropy = Some(1);
            off.host.fog_density_scale = Some(1.0);
            let isolated = if profile == "generated-normals-ggx" { "generated-normals" } else { profile.as_str() };
            let mut settings = graphics_fixture_settings(if enabled { isolated } else { "faithful" })?;
            // Supplemental interaction: PBR is held on; only bump generation changes.
            if profile == "generated-normals-ggx" { settings.material_pbr = true; }
            writeln!(manifest, "phase={phase} enabled={enabled} material_pbr={} generated_normals={}", settings.material_pbr, settings.generated_normals)?;
            // Isolate reflection cost from the wet-surface producer, which stays on in A/B.
            if profile == "wet-reflections" { settings.wet_surfaces = true; }
            off.host.enhanced_settings = Some(settings);
            off.frame(&mut shot, 0.0);
            off.host.weather_input = weather.clone();
            off.frame(&mut shot, 0.0); // Submit native weather before frozen renders.
            off.set_enhanced_settings(settings); // Explicit material-worker completion barrier.
            off.wait_for_pipeline_warmup();
            ensure!(off.msaa_samples() == 4, "real evidence requires actual 4xMSAA, not adapter fallback");
            ensure!(off.pipeline_warmup_complete(), "startup pipelines not ready");
            off.prepare_frozen_rendering();
            if profile == "wet-reflections" {
                for _ in 0..120 { off.advance_effect_time(0.25); off.render_frozen()?; }
                let mut after_rain = weather.clone();
                after_rain.rain = 0.0;
                off.host.weather_input = after_rain;
                // Retain the same collision surface and accumulated wetness; stop
                // precipitation without advancing the native drying simulation.
                off.frame(&mut shot, 0.0);
                off.prepare_frozen_rendering();
            }
            for _ in 0..32 { off.render_frozen()?; }
            // Probe learns only actually rendered static geometry, by real camera rotation.
            for (column, yaw) in [0.0, 0.6, -0.6, 0.0].into_iter().enumerate() {
                off.host.camera = Camera { yaw: fixed.yaw + yaw, ..fixed };
                for _ in 0..12 { off.render_frozen()?; }
                let raw = off.frozen_rgba()?;
                if phase == 0 { baseline.push(raw.clone()); }
                if phase == 1 { changed += raw.iter().zip(&baseline[column]).filter(|(a,b)| a != b).count(); }
                if phase < 2 {
                    let image = image::RgbaImage::from_raw(SIZE.0, SIZE.1, raw.clone()).context("RGBA dimensions")?;
                    let thumb = image::imageops::resize(&image, 960, 600, image::imageops::FilterType::Triangle);
                    image::imageops::replace(&mut strip, &thumb, column as i64 * 960, phase as i64 * 600);
                    if column == 3 {
                        image.save(out.join(if enabled { "after.png" } else { "before.png" }))?;
                        image::imageops::replace(&mut before_after, &image, phase as i64 * i64::from(SIZE.0), 0);
                    }
                }
            }
            if matches!(profile.as_str(), "environment-probe" | "wet-reflections") && phase == 1 {
                let (probe, learned, analytic) = off.material_probe_evidence()?.context("material reflection diagnostics unavailable")?;
                ensure!(learned.len() == (SIZE.0 * SIZE.1 * 4) as usize && analytic.len() == learned.len(),
                    "matched reflection-stage diagnostics disagree with the real frame dimensions");
                let confident = if profile == "environment-probe" {
                    ensure!(probe.len() == 128 * 128 * 4, "probe diagnostic must be 128x128 RGBA");
                    image::save_buffer(out.join("probe-radiance-confidence.png"), &probe, 128, 128, image::ColorType::Rgba8)?;
                    let confidence: Vec<u8> = probe.as_chunks::<4>().0.iter()
                        .flat_map(|pixel| [pixel[3], pixel[3], pixel[3], 255]).collect();
                    image::save_buffer(out.join("probe-confidence.png"), &confidence, 128, 128, image::ColorType::Rgba8)?;
                    probe.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] > 0).count()
                } else { 0 };
                let different = learned.as_chunks::<4>().0.iter().zip(analytic.as_chunks::<4>().0.iter())
                    .filter(|(a, b)| a[..3] != b[..3]).count();
                let label = if profile == "environment-probe" { "probe" } else { "ssr" };
                image::save_buffer(out.join(format!("{label}-analytic-only.png")), &analytic, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                image::save_buffer(out.join(format!("{label}-observed-reflection-stage.png")), &learned, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                let mut sheet = image::RgbaImage::new(SIZE.0 * 2, SIZE.1);
                let analytic = image::RgbaImage::from_raw(SIZE.0, SIZE.1, analytic).context("analytic frame dimensions")?;
                let learned = image::RgbaImage::from_raw(SIZE.0, SIZE.1, learned).context("observed reflection frame dimensions")?;
                image::imageops::replace(&mut sheet, &analytic, 0, 0);
                image::imageops::replace(&mut sheet, &learned, i64::from(SIZE.0), 0);
                sheet.save(out.join(format!("{label}-analytic-vs-observed.png")))?;
                writeln!(manifest, "{label}_confident_texels={confident}\n{label}_observed_vs_analytic_changed_pixels={different}\n{label}_comparison_columns=analytic_only,observed_scene\n{label}_source=actually_visible_static_geometry_after_camera_rotation")?;
                writeln!(manifest, "{label}_comparison_stage=pre_weather_reflection\n{label}_comparison_snapshot=identical_native_primary_snapshot\n{label}_comparison_excludes=weather_sheen_and_post\n{label}_user_before_after_stage=final_native_weather")?;
                probe_counts = Some((confident, different));
            }
            off.host.camera = fixed;
            for _ in 0..32 { off.render_frozen()?; }
            off.set_gpu_timing(true)?;
            let mut samples = Vec::with_capacity(SAMPLES);
            for sample in 0..SAMPLES {
                ensure!(started.elapsed().as_secs() < 270, "material evidence exceeded lease budget");
                off.render_frozen()?;
                let ms = off.read_frame_gpu_ms()?.context("GPU timestamps unavailable")?;
                ensure!(ms.is_finite() && ms > 0.0, "invalid GPU timestamp");
                samples.push(ms);
                writeln!(csv, "{phase},{enabled},{sample},{ms}")?;
            }
            off.set_gpu_timing(false)?;
            samples.sort_by(f64::total_cmp);
            writeln!(summary, "phase={phase} enabled={enabled} actual_msaa={} gpu_p50_ms={} gpu_p95_ms={} gpu_p99_ms={}", off.msaa_samples(), samples[29], samples[56], samples[59])?;
            if phase < 2 {
                if let Some(roof) = roof_camera {
                    off.host.camera = roof;
                    for _ in 0..12 { off.render_frozen()?; }
                    let raw = off.frozen_rgba()?;
                    image::save_buffer(out.join(if enabled { "roof-after.png" } else { "roof-before.png" }),
                        &raw, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                    if enabled {
                        let previous = image::open(out.join("roof-before.png"))?.into_rgba8().into_raw();
                        let delta = raw.iter().zip(&previous).filter(|(a,b)| a != b).count();
                        writeln!(manifest, "ssr_roof_changed_bytes={delta}")?;
                        // Generic native gloss is valid under roofs. Hold SSR,
                        // camera and time fixed; isolate only the wet producer.
                        let dry_settings = ao_render::EnhancedSettings { wet_surfaces: false, ..settings };
                        off.set_enhanced_settings(dry_settings);
                        off.wait_for_pipeline_warmup();
                        for _ in 0..12 { off.render_frozen()?; }
                        let dry = off.frozen_rgba()?;
                        let wet_delta = raw.iter().zip(&dry).filter(|(a,b)| a != b).count();
                        image::save_buffer(out.join("roof-dry-reflections.png"),
                            &dry, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                        let mut sheet = image::RgbaImage::new(SIZE.0 * 2, SIZE.1);
                        let wet = image::RgbaImage::from_raw(SIZE.0, SIZE.1, raw).context("roof wet frame dimensions")?;
                        let dry = image::RgbaImage::from_raw(SIZE.0, SIZE.1, dry).context("roof dry frame dimensions")?;
                        image::imageops::replace(&mut sheet, &wet, 0, 0);
                        image::imageops::replace(&mut sheet, &dry, i64::from(SIZE.0), 0);
                        sheet.save(out.join("roof-wet-vs-dry.png"))?;
                        writeln!(manifest, "roof_wet_vs_dry_changed_bytes={wet_delta}\nroof_oracle_columns=wet_surfaces_on,wet_surfaces_off\nroof_oracle_material_reflections=on_in_both\nroof_oracle_camera_time=unchanged")?;
                        roof_wet_delta = Some(wet_delta);
                    }
                }
            }
        }
        strip.save(out.join("frame-strip.png"))?;
        before_after.save(out.join("before-after.png"))?;
        writeln!(manifest, "strip_columns=fixed,right,left,returned_fixed\nstrip_rows=off,on\nbefore_after_columns=off,on\nchanged_bytes={changed}\nmsaa_actual=4\nelapsed_seconds={}", started.elapsed().as_secs_f64())?;
        std::fs::write(out.join("manifest.txt"), manifest)?;
        std::fs::write(out.join("gpu.csv"), csv)?;
        std::fs::write(out.join("summary.txt"), summary)?;
        ensure!(changed > 0, "real native content shows no feature effect; do not replace it with fake gloss or material overrides");
        if let Some((confident, different)) = probe_counts {
            if profile == "environment-probe" {
                ensure!(confident > 0, "native static geometry did not populate probe confidence");
            }
            ensure!(different > 0, "real reflection is only analytic fallback, not observed native scene geometry");
        }
        if let Some(delta) = roof_wet_delta {
            ensure!(delta == 0, "collision-covered native floor changed with wet surfaces while SSR stayed enabled");
        }
        Ok(())
    })();
    std::fs::write(out.join("status.txt"), format!("status={}\nvisual_acceptance=unexercised\nerror={:?}\n", if result.is_ok() { "complete" } else { "failed" }, result.as_ref().err()))?;
    result
}

struct MaterialScratch(PathBuf);
impl Drop for MaterialScratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
