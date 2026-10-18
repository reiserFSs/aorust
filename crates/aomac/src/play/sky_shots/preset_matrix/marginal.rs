//! Whole-frame leave-one-out costs; pass timestamps are not exclusive effect costs.
use super::*;
use super::super::super::{dvalue::Variant, enhanced::{controls, graphical, ControlKind}};

const MARGINAL_VIEWS: [&str; 2] = ["icc-landmark-authored", "newland-day"];
const DEFAULT_EFFECTS: [&str; 16] = [
    "AomacAmbientOcclusion", "AomacContactShadows", "AomacVolumetricClouds",
    "AomacWaterReflections", "AomacWaterRefraction", "AomacGroundGrass",
    "AomacFoliageLeaves", "AomacMaterialReflections", "AomacEnvironmentProbe",
    "AomacSunShadows", "AomacTaa", "AomacEmissiveBloom", "AomacCloudShadows",
    "AomacVolumetricShafts", "AomacClusteredLighting", "AomacLocalLightShadows",
];
const ORDER: [bool; 4] = [true, false, false, true];
const WARMUP: usize = 120;
const SAMPLES: usize = 240;

fn effects(requested: Option<&str>, modern: &DValues) -> anyhow::Result<Vec<String>> {
    let names: Vec<_> = match requested {
        Some(list) => list.split(',').collect(),
        None => DEFAULT_EFFECTS.to_vec(),
    };
    let registry = controls();
    let mut selected = Vec::with_capacity(names.len());
    for name in names {
        ensure!(registry.iter().any(|control| control.name == name && graphical(name)
            && matches!(control.kind, ControlKind::Check)),
            "{name:?} is not an exact graphical Bool DValue key");
        ensure!(modern.flag(name), "{name} is already off in Modern; cannot disable one");
        ensure!(!selected.iter().any(|previous| previous == name), "duplicate effect {name}");
        selected.push(name.to_owned());
    }
    Ok(selected)
}

fn percentiles(samples: &mut [f64]) -> (f64, f64) {
    samples.sort_by(f64::total_cmp);
    let rank = |percent: usize| (samples.len() * percent).div_ceil(100) - 1;
    (samples[rank(50)], samples[rank(95)])
}

#[test]
#[ignore = "installed-client whole-frame Modern leave-one-out ABBA; 120 warmup + 240 GPU samples per leg"]
fn graphics_preset_marginal_costs() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT")
        .context("set AOMAC_GFX_OUT to a NEW directory")?);
    ensure!(!out.exists(), "refusing to overwrite {}", out.display());
    let view = std::env::var("AOMAC_GFX_VIEW").ok();
    ensure!(view.as_deref().is_none_or(|name| MARGINAL_VIEWS.contains(&name)),
        "AOMAC_GFX_VIEW must be icc-landmark-authored or newland-day");
    let dir = ao_gui::client_dir();
    let mut modern = DValues::new(&dir);
    apply_preset(&mut modern, GraphicsPreset::Modern);
    let requested = std::env::var("AOMAC_GFX_EFFECTS").ok();
    let selected = effects(requested.as_deref(), &modern)?;
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "status=running\n")?;
    let result = capture_marginals(&out, &dir, view.as_deref(), &selected);
    std::fs::write(out.join("status.txt"), match &result {
        Ok(()) => "status=complete\n".to_owned(),
        Err(error) => format!("status=failed\nerror={error:#}\n"),
    })?;
    result
}

fn capture_marginals(out: &std::path::Path, dir: &std::path::Path, view: Option<&str>, selected: &[String]) -> anyhow::Result<()> {
    let store = ao_rdb::RecordStore::open(dir)?;
    let actor = graphics_fixture_actor()?;
    let weather = ao_formats::weather::State::clear();
    let legs = selected.len() * ORDER.len() * if view.is_some() { 1 } else { MARGINAL_VIEWS.len() };
    let mut manifest = format!("fixture=graphics_preset_marginal_costs\nsize=1920x1200\nreference=Modern\norder=A,B,B,A\nA=Modern\nB=Modern_with_exactly_one_Bool_false\neffects={selected:?}\nplanned_legs={legs}\nwarmup={WARMUP}\nsamples_per_leg={SAMPLES}\nmsaa_requested=4\nanisotropy=16\ntexture_quality=2\ntexture_compressed=false\ntexture_better_mips=Modern_except_when_that_is_the_selected_control\npercentile=nearest_rank_ceil_np\ndelta_ms=mean(A_leg_p50)-mean(B_leg_p50)\ntiming=GPU_frame_timestamp_not_wall_time\npass_profiling=false\npng=false\ngame_day=0\ncrt_seed=1\nsimulation_dt=1/60\ngame_clock=frozen_authored_snapshot\nrenderer_time=continuous_across_reused_legs_reset_on_rebuild\nweather={weather:?}\nactor_source=docs/captures/zone_ithaca.rec\nactor_id=25988\nshader_dir={:?}\nexecutable={:?}\n", std::env::var_os("AOMAC_SHADER_DIR"), std::env::current_exe()?);
    for (label, program, args) in [
        ("revision", "git", vec!["rev-parse".into(), "HEAD".into()]),
        ("source_diff", "git", vec!["diff".into(), "--no-ext-diff".into(), "HEAD".into()]),
        ("resource_sha256", "shasum", vec!["-a".into(), "256".into(), dir.join("cd_image/rdb.db").into_os_string()]),
    ] {
        let output = std::process::Command::new(program).args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
        ensure!(output.status.success(), "{program} failed");
        writeln!(manifest, "{label}={:?}", String::from_utf8(output.stdout)?.trim())?;
    }
    writeln!(manifest, "harness_source={:?}", include_str!("marginal.rs"))?;
    let mut frames = String::from("view,effect,leg,condition,sample,gpu_frame_ms\n");
    let mut summary = String::from("view,effect,leg,condition,pf,game_time,width,height,msaa_actual,anisotropy,better_mips,warmup,samples,p50_ms,p95_ms\n");
    let mut deltas = String::from("view,effect,a_mean_p50_ms,b_mean_p50_ms,delta_ms\n");
    std::fs::write(out.join("manifest.txt"), &manifest)?;
    for (name, pf, time, fixed) in graphics_fixture_views(&actor).into_iter().filter(|(name, _, _, _)| {
        MARGINAL_VIEWS.contains(&name.as_str()) && view.is_none_or(|selected| selected == name)
    }) {
        ao_scene::crt_srand(1);
        let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, dir, pf, time, 0, Some(weather))?;
        let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
        super::super::super::player::bake_sky_open(&collision, &mut scene);
        let fixed = if pf == 566 { graphics_fixture_newland_camera(|point| collision.terrain_height(point))? } else { fixed };
        let exposure = super::super::super::player::grass_exposure_surface(std::rc::Rc::new(std::cell::RefCell::new(collision)));
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        writeln!(manifest, "view={name} pf={pf} game_time={time} camera={fixed:?} lens={lens:?} meshes={} instances={} environment={:?} daylight={:?}", scene.meshes.len(), scene.instances.len(), scene.environment, scene.daylight)?;
        let mut shot = Shot { gui: Gui::new(dir, None)?, scene: None, camera: fixed, lens };
        let mut renderer: Option<Offscreen> = None;
        for effect in selected {
            let mut medians = [0.0; 4];
            for (leg, enabled) in ORDER.into_iter().enumerate() {
                let condition = if enabled { "A" } else { "B" };
                let mut values = DValues::new(dir);
                apply_preset(&mut values, GraphicsPreset::Modern);
                values.set_raw(effect, Variant::Bool(enabled));
                let settings = values.enhanced_settings();
                let better_mips = values.flag("AomacBetterMipFiltering");
                // Texture policy is consumed during upload: rebuild for this control, not a stale resident texture set.
                if effect == "AomacBetterMipFiltering" { renderer = None; }
                let rebuilt = renderer.is_none();
                if rebuilt {
                    ao_scene::crt_srand(1);
                    shot.scene = Some(scene.clone());
                    let mut off = Offscreen::evidence_timing(&shot, SIZE)?;
                    off.fixed_camera(Some(fixed));
                    off.host.msaa_samples = Some(4);
                    off.host.anisotropy = Some(16);
                    off.host.texture_policy = Some((2, false, better_mips));
                    off.host.fog_density_scale = Some(1.0);
                    off.host.enhanced_settings = Some(settings);
                    off.host.grass_exposure = Some(exposure.clone());
                    off.host.grass_wind = weather.wind;
                    off.frame(&mut shot, 0.0);
                    renderer = Some(off);
                }
                let off = renderer.as_mut().context("marginal renderer")?;
                // Immediate fixture API waits for the exact variant; never measure an async old-settings frame.
                off.set_enhanced_settings(settings);
                off.wait_for_pipeline_warmup();
                off.warm_grass_tiles();
                ensure!(off.msaa_samples() == 4, "{name}/{effect}/{leg}: actual MSAA != 4");
                off.set_gpu_timing(true)?;
                let mut samples = Vec::with_capacity(SAMPLES);
                for frame in 0..WARMUP + SAMPLES {
                    off.frame(&mut shot, 1.0 / 60.0);
                    let ms = off.read_frame_gpu_ms()?.context("GPU frame timestamp missing")?;
                    ensure!(ms.is_finite() && ms > 0.0, "invalid GPU frame timestamp {ms}");
                    if frame >= WARMUP {
                        writeln!(frames, "{name},{effect},{leg},{condition},{},{ms:.9}", frame - WARMUP)?;
                        samples.push(ms);
                    }
                }
                off.set_gpu_timing(false)?;
                ensure!(samples.len() == SAMPLES && off.msaa_samples() == 4, "incomplete or wrong-quality timing leg");
                let (p50, p95) = percentiles(&mut samples);
                medians[leg] = p50;
                writeln!(summary, "{name},{effect},{leg},{condition},{pf},{time},1920,1200,{},16,{better_mips},{WARMUP},{},{p50:.9},{p95:.9}", off.msaa_samples(), samples.len())?;
                writeln!(manifest, "completed_view={name} effect={effect} leg={leg} condition={condition} renderer_rebuilt={rebuilt} msaa_actual={} samples={} settings={settings:?}", off.msaa_samples(), samples.len())?;
                std::fs::write(out.join("frames.csv"), &frames)?;
                std::fs::write(out.join("summary.csv"), &summary)?;
                std::fs::write(out.join("manifest.txt"), &manifest)?;
                eprintln!("{name}/{effect}/{leg}/{condition}: samples={} MSAA={} p50={p50:.3}ms p95={p95:.3}ms", samples.len(), off.msaa_samples());
            }
            let a = (medians[0] + medians[3]) / 2.0;
            let b = (medians[1] + medians[2]) / 2.0;
            writeln!(deltas, "{name},{effect},{a:.9},{b:.9},{:.9}", a - b)?;
            std::fs::write(out.join("deltas.csv"), &deltas)?;
            eprintln!("{name}/{effect}: whole-frame marginal GPU delta={:.3}ms", a - b);
            // Restore Modern's texture policy before reusing a renderer for the next effect.
            if effect == "AomacBetterMipFiltering" { renderer = None; }
        }
    }
    Ok(())
}

#[test]
fn graphics_preset_marginal_contract() {
    let mut modern = DValues::new(&ao_gui::client_dir());
    apply_preset(&mut modern, GraphicsPreset::Modern);
    assert_eq!(effects(None, &modern).unwrap().len(), 16);
    assert_eq!(ORDER, [true, false, false, true]);
    assert_eq!((WARMUP, SAMPLES), (120, 240));
    assert_eq!(effects(Some("AomacAmbientOcclusion,AomacBetterMipFiltering"), &modern).unwrap(),
        ["AomacAmbientOcclusion", "AomacBetterMipFiltering"]);
    for invalid in ["", "AmbientOcclusion", "AomacAoRadius", "AomacEffectGpuOverlay", "AomacPinchZoom",
        "AomacDepthOfField", "AomacFxaa", "AomacAmbientOcclusion,", "AomacTaa,AomacTaa"] {
        assert!(effects(Some(invalid), &modern).is_err(), "accepted {invalid:?}");
    }
    for effect in DEFAULT_EFFECTS {
        let mut disabled = DValues::new(&ao_gui::client_dir());
        apply_preset(&mut disabled, GraphicsPreset::Modern);
        disabled.set_raw(effect, Variant::Bool(false));
        let changed: Vec<_> = controls().into_iter().filter(|control| graphical(control.name)
            && modern.get(control.name) != disabled.get(control.name)).map(|control| control.name).collect();
        assert_eq!(changed, [effect]);
    }
    let baseline = modern.enhanced_settings();
    modern.set_raw("AomacAmbientOcclusion", Variant::Bool(false));
    assert_eq!(modern.enhanced_settings(), ao_render::EnhancedSettings { ambient_occlusion: false, ..baseline });
    let mut samples: Vec<_> = (1..=240).rev().map(f64::from).collect();
    assert_eq!(percentiles(&mut samples), (120.0, 228.0));
}
