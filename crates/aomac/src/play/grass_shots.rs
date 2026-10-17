//! Paired grass evidence on the shared offline fixture snapshots; no server/window.
//! AOMAC_GFX_VIEW selects one view; AOMAC_GFX_PNG_ONLY=1 omits raw RGBA files.
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

fn run(out: &std::path::Path) -> anyhow::Result<()> {
    let selected = std::env::var("AOMAC_GFX_VIEW").ok();
    let png_only = std::env::var("AOMAC_GFX_PNG_ONLY").is_ok_and(|value| value == "1");
    // AOMAC_GFX_DENSITY: grass density (default 1); AOMAC_GFX_EFFECTS=1: shadows, AO and bloom on in both variants;
    // AOMAC_GFX_HDR=1: the HDR present; AOMAC_GFX_MSAA: samples (default 1).
    let density = std::env::var("AOMAC_GFX_DENSITY").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
    let effects = std::env::var("AOMAC_GFX_EFFECTS").is_ok_and(|value| value == "1");
    let hdr = std::env::var("AOMAC_GFX_HDR").is_ok_and(|value| value == "1");
    let msaa = std::env::var("AOMAC_GFX_MSAA").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let actor = graphics_fixture_actor()?;
    let weather = ao_formats::weather::State::clear();
    let size = (1920, 1200);
    let mut changed_views = 0;
    let mut report = String::from("fixture=graphics_offline_fixture\nsize=1920x1200\nrenderer_time=0\ndt=0\ncrt_seed=1\nweather=clear\nmsaa=1\nanisotropy=1\nwarmup_frames=320\nmeasured_frames=600\ntiming=FrameGpuTimer whole-frame neutral Enhanced UNORM versus grass incremental contribution, not isolated draw timestamp\nvisual_acceptance=unexercised\n");
    writeln!(report, "selected_view={selected:?}\npng_only={png_only}")?;
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
        ("newland-grass-ground", 566, 4700.0, ao_render::Vec3::new(0.0, 0.0, 0.0)),
        ("borealis-grass-ground", 800, 3240.0, ao_render::Vec3::new(652.0, 65.3, -420.0)),
    ] {
        if selected.as_deref().is_some_and(|view| !view.starts_with(name)) { continue; }
        ao_scene::crt_srand(1);
        let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
        let surface = Rc::new(RefCell::new(ao_formats::playfield::collision::Collision::load(&store, pf)?));
        let exposure = super::super::player::grass_exposure_surface(surface.clone());
        // A zero reference means the middle of the playfield's ground.
        let reference = match scene.ground_detail.first() {
            Some(patch) if reference == ao_render::Vec3::ZERO => ao_render::Vec3::new(
                (patch.origin[0] as f32 + patch.cells as f32 * 0.5) * patch.cell_size, 0.0, -(patch.origin[1] as f32 + patch.cells as f32 * 0.5) * patch.cell_size),
            _ => reference,
        };
        let (fixed, tile, grid) = grass_ground_camera(&scene, &surface.borrow(), &exposure, reference)
            .with_context(|| format!("finding exposed grass in {name}"))?;
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
            for (light, light_time) in [("day", time), ("dusk", 4860.0)].into_iter().chain((pf == 800).then_some(("night", 0.0))) {
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
        ao_scene::crt_srand(1);
        let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
        let exposure = super::super::player::grass_exposure_surface(Rc::new(RefCell::new(
            ao_formats::playfield::collision::Collision::load(&store, pf)?)));
        let mut medians = [0.0; 2];
        let mut baseline_raw = None;
        let mut allocated = [None; 2];
        // Repeat opposite ordering to expose simple thermal/order drift.
        for (round, order) in [[0usize, 1], [1, 0]].into_iter().enumerate() {
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
                let settings = EnhancedSettings { sun_shadows: effects, ambient_occlusion: effects, emissive_bloom: effects, hdr, ground_grass: true, grass_density: if variant == 0 { 0.0 } else { density }, ..Default::default() };
                off.host.enhanced_settings = Some(settings);
                off.set_gpu_timing(true)?;
                let mut gpu = Vec::with_capacity(600);
                let mut prepare = Vec::with_capacity(600);
                let mut generation = Vec::with_capacity(320);
                let mut list = Default::default();
                for frame in 0..920 {
                    list = off.frame(&mut shot, 0.0);
                    let ms = off.read_frame_gpu_ms()?.context("GPU timestamps unavailable")?;
                    if frame >= 320 {
                        gpu.push(ms);
                        prepare.push(off.evidence_grass_metrics().1 as f64 / 1_000_000.0);
                    }
                    else { generation.push(off.evidence_grass_metrics().1 as f64 / 1_000_000.0); }
                }
                off.set_gpu_timing(false)?;
                let (bytes, _, cells) = off.evidence_grass_metrics();
                let gpu_stats = percentiles(&mut gpu);
                let prepare_stats = percentiles(&mut prepare);
                let generation_mean = generation.iter().sum::<f64>() / generation.len() as f64;
                let generation_max = generation.iter().copied().fold(0.0, f64::max);
                let generation_stats = percentiles(&mut generation);
                medians[variant] = gpu_stats.0;
                allocated[variant] = off.gpu_allocated_bytes();
                writeln!(report, "view={name} pf={pf} time={time} camera={fixed:?} lens={lens:?} settings={settings:?} round={round} variant={variant} gpu_ms_p50_p95_p99={gpu_stats:?} prepare_wall_ms_p50_p95_p99={prepare_stats:?} effect_gpu_payload_bytes={bytes} cells={cells} max_blade_candidates={}", cells * 256 * variant)?;
                writeln!(report, "generation_wall_ms_p50_p95_p99={generation_stats:?} generation_mean_ms={generation_mean} generation_max_ms={generation_max}")?;
                writeln!(report, "native_metal_allocated_bytes={:?} shared_enhanced_target_bytes={}", allocated[variant], off.enhanced_target_bytes())?;
                if round == 0 && variant == 1 { writeln!(report, "grass_diagnostics_view={name}\n{}", off.evidence_grass_diagnostics())?; }
                std::fs::write(out.join("manifest.txt"), &report)?;
                ensure!(variant == 0 || prepare_stats.1 <= 0.2, "{name}: grass generation/prepare p95 exceeds 0.2 ms: {prepare_stats:?}");
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
                        if differing > 0 { changed_views += 1; }
                        if pf == 127 { ensure!(cells == 0 && differing == 0, "indoor grass changed the image"); }
                    }
                }
            }
            writeln!(report, "view={name} round={round} incremental_grass_gpu_frame_p50_ms={}", medians[1] - medians[0])?;
            if let [Some(before), Some(after)] = allocated {
                writeln!(report, "view={name} round={round} native_metal_allocated_delta_bytes={}", after as i128 - before as i128)?;
            }
            std::fs::write(out.join("manifest.txt"), &report)?;
        }
    }
    writeln!(report, "changed_views={changed_views}")?;
    ensure!(changed_views > 0, "fixture has no visible grass contribution; do not claim a visual/performance pass");
    std::fs::write(out.join("manifest.txt"), &report)?;
    Ok(())
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
