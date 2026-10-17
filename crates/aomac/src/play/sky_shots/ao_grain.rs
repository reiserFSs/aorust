//! Installed-client, frozen ICC AO/contact image regressions; no production renderer API.
use super::{camera, graphics_fixture_settings, Shot};
use anyhow::{Context, ensure};
use ao_render::{Camera, Offscreen, Vec3};
use std::{fmt::Write, path::PathBuf};

mod motion;

const SIZE: (u32, u32) = (1920, 1200);
const FRAMES: usize = 8;
// Observed pre-fix ICC captures, not fitted acceptance tolerances. Keep these
// bounds independent of external PNG availability; never widen them to pass.
const BEFORE_STATIC_GRAIN: f64 = 0.007_092_564_256_341_66;
const BEFORE_MEAN_AO: f64 = 0.225_321_984_057_130_1;

#[derive(Clone, Copy, Debug)]
enum Effect { Ao, Contact }

impl Effect {
    fn name(self) -> &'static str {
        match self { Self::Ao => "ao", Self::Contact => "contact" }
    }

    fn env_prefix(self) -> &'static str {
        match self { Self::Ao => "AOMAC_GFX_AO", Self::Contact => "AOMAC_GFX_CONTACT" }
    }

    fn settings(self, enabled: bool) -> anyhow::Result<ao_render::EnhancedSettings> {
        match self {
            Self::Ao => {
                let mut settings = graphics_fixture_settings("ao")?;
                if !enabled { settings.ao_strength = 0.0; }
                Ok(settings)
            }
            Self::Contact => Ok(ao_render::EnhancedSettings {
                contact_shadows: true,
                contact_strength: if enabled { 0.8 } else { 0.0 },
                ..Default::default()
            }),
        }
    }

    fn static_bounds(self) -> (f64, f64) {
        match self {
            Self::Ao => (BEFORE_STATIC_GRAIN, BEFORE_MEAN_AO),
            Self::Contact => (0.000_632_754_351_091_23, 0.047_383_104_488_108_18),
        }
    }

    fn motion_bounds(self, sequence: &str) -> (f64, f64, usize, f64) {
        match self {
            Self::Ao if sequence == "tiny" => (3.418_431_057_669_665, 254.813_357_257_990_96, 5_661_749, 0.007_105_341_793_134_52),
            Self::Ao => (8.270_776_727_091_926, 254.812_124_984_693_27, 7_257_147, 0.007_122_972_711_002_28),
            Self::Contact if sequence == "tiny" => (1.318_216_025_873_466, 119.633_592_677_761_53, 2_004_863, 0.000_631_453_814_803_43),
            Self::Contact => (2.643_878_401_054_905, 122.251_099_181_583_17, 2_604_297, 0.000_637_803_204_615_05),
        }
    }
}
// Fixed screen-space regions, not content-selected masks: inspect the saved crops when
// changing the camera. These names describe the intended coastal1 coverage, not retail proof.
const CROPS: [(&str, u32, u32, u32, u32); 2] = [
    ("cliff", 80, 300, 800, 500),
    ("grass", 320, 850, 1280, 300),
];

fn contribution(on: &[u8], off: &[u8]) -> Vec<f64> {
    on.as_chunks::<4>().0.iter().zip(off.as_chunks::<4>().0.iter()).map(|(a, b)| {
        let y = |p: &[u8]| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
        // Relative code-space attenuation cancels the multiplicative scene texture.
        // The 16-code floor prevents dark texels amplifying 8-bit quantization noise.
        (y(b) - y(a)) / y(b).max(16.0)
    }).collect()
}

fn grain(values: &[f64], report: &mut String, prefix: &str) -> f64 {
    let mut total = 0.0;
    let mut samples = 0;
    for (name, x, y, width, height) in CROPS {
        let mut energy = 0.0;
        let mut count = 0;
        for row in y + 1..y + height - 1 {
            for col in x + 1..x + width - 1 {
                let i = (row * SIZE.0 + col) as usize;
                let residual = values[i] - 0.25 * (values[i - 1] + values[i + 1]
                    + values[i - SIZE.0 as usize] + values[i + SIZE.0 as usize]);
                energy += residual * residual;
                count += 1;
            }
        }
        writeln!(report, "{prefix}_{name}_crop={x},{y},{width},{height}\n{prefix}_{name}_samples={count}\n{prefix}_{name}_grain_mse={:.17}", energy / f64::from(count)).unwrap();
        total += energy;
        samples += count;
    }
    let metric = total / f64::from(samples);
    writeln!(report, "{prefix}_grain_mse={metric:.17}").unwrap();
    metric
}

fn save_contribution(out: &std::path::Path, prefix: &str, values: &[f64]) -> anyhow::Result<()> {
    // Signed normalized contribution: zero = midgray, positive attenuation = brighter.
    let image = image::GrayImage::from_fn(SIZE.0, SIZE.1, |x, y| {
        image::Luma([((0.5 + values[(y * SIZE.0 + x) as usize] * 0.5).clamp(0.0, 1.0) * 255.0).round() as u8])
    });
    image.save(out.join(format!("{prefix}-contribution.png")))?;
    for (name, x, y, width, height) in CROPS {
        image::imageops::crop_imm(&image, x, y, width, height).to_image()
            .save(out.join(format!("{prefix}-{name}-contribution.png")))?;
    }
    Ok(())
}

fn code_diff(a: &[u8], b: &[u8]) -> (f64, u8, usize, usize, Vec<u8>) {
    let mut sum = 0_u64;
    let mut max = 0;
    let mut over_two = 0;
    let mut changed = 0;
    let mut image = Vec::with_capacity(a.len());
    for (a, b) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
        let rgb = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
        let pixel_max = *rgb.iter().max().unwrap();
        max = max.max(pixel_max);
        over_two += usize::from(pixel_max > 2);
        changed += a.iter().zip(b).filter(|(a, b)| a != b).count();
        sum += rgb.iter().map(|v| u64::from(*v)).sum::<u64>();
        image.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    (sum as f64 / (SIZE.0 as f64 * SIZE.1 as f64 * 3.0), max, over_two, changed, image)
}

#[test]
#[ignore = "installed-client headless ICC AO grain regression; writes a new AOMAC_GFX_OUT"]
fn graphics_ao_static_grain() -> anyhow::Result<()> {
    capture(Effect::Ao)
}

#[test]
#[ignore = "installed-client headless ICC contact-shadow grain regression; writes a new AOMAC_GFX_OUT"]
fn graphics_contact_static_grain() -> anyhow::Result<()> {
    capture(Effect::Contact)
}

fn capture(effect: Effect) -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a new evidence directory")?);
    ensure!(!out.exists(), "refusing to overwrite {}", out.display());
    let baseline = std::env::var_os(format!("{}_BASELINE", effect.env_prefix())).map(PathBuf::from);
    let baseline_only = std::env::var_os(format!("{}_CAPTURE_BASELINE", effect.env_prefix())).is_some();
    // Normal acceptance uses recorded bounds even when no baseline PNGs survive.
    ensure!(!baseline_only || baseline.is_none(), "baseline capture and baseline comparison are mutually exclusive");
    let dir = ao_gui::client_dir();
    ensure!(dir.join("cd_image/rdb.db").exists(), "installed client required at {}", dir.display());
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "RUNNING\n")?;
    let fixture = format!("graphics_{}_static_grain", effect.name());
    let mut report = format!("fixture={fixture}\neffect={effect:?}\nmetric_version=1\nbaseline_only={baseline_only}\nsize=1920x1200\npf=4582\ngame_time=2648.69\ngame_day=0\ncrt_seed=1\ncloud_seed=1\nweather=clear\nrenderer_time=0\ndt=0\nmsaa=1\nanisotropy=1\nframes_per_profile={FRAMES}\nmetric=mean_squared_cardinal_highpass_of_relative_code_luminance_attenuation\nnormalization_floor_codes=16\ncontribution_png=128_is_zero_positive_is_brighter\n");
    writeln!(report, "shader_dir={:?}\nbaseline={baseline:?}", std::env::var_os("AOMAC_SHADER_DIR"))?;
    let result = (|| -> anyhow::Result<()> {
        let store = ao_rdb::RecordStore::open(&dir)?;
        let weather = ao_formats::weather::State::clear();
        ao_scene::crt_srand(1);
        let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
            &store, &dir, 4582, 2648.69, 0, Some(weather))?;
        let collision = ao_formats::playfield::collision::Collision::load(&store, 4582)?;
        super::super::player::bake_sky_open(&collision, &mut scene);
        let fixed = Camera { pos: Vec3::new(660.0, 24.3, -714.0), yaw: 0.0, pitch: -0.3, roll: 0.0 };
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        let mut shot = Shot { gui: ao_gui::Gui::new(&dir, None)?, scene: Some(scene), camera: fixed, lens };
        let mut off = Offscreen::evidence(&shot, SIZE)?;
        off.fixed_camera(Some(fixed));
        off.host.msaa_samples = Some(1);
        off.host.anisotropy = Some(1);
        off.host.fog_density_scale = Some(1.0);
        off.host.grass_wind = weather.wind;
        writeln!(report, "camera={fixed:?}\nlens={lens:?}")?;
        off.host.enhanced_settings = Some(effect.settings(false)?);
        for _ in 0..32 {
            let list = off.frame(&mut shot, 0.0);
            off.rgba(&shot, &list)?;
        }
        writeln!(report, "initial_frozen_warmup_frames=32")?;
        let mut captures = Vec::new();
        let mut stability_failures = 0;
        for enabled in [false, true] {
            // Both phases use the SAME renderer/scene/sky state; only strength changes.
            let settings = effect.settings(enabled)?;
            off.host.enhanced_settings = Some(settings);
            let label = if enabled { "on" } else { "off" };
            writeln!(report, "{label}_settings={settings:?}")?;
            // Settle newly enabled effect targets before measuring consecutive frames.
            for _ in 0..4 {
                let list = off.frame(&mut shot, 0.0);
                off.rgba(&shot, &list)?;
            }
            writeln!(report, "{label}_effect_warmup_frames=4")?;
            let mut previous: Option<Vec<u8>> = None;
            for frame in 0..FRAMES {
                let list = off.frame(&mut shot, 0.0);
                let raw = off.rgba(&shot, &list)?;
                image::save_buffer(out.join(format!("icc-coastal1-{label}-frame{frame:02}.png")), &raw, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                if let Some(previous) = &previous {
                    let (mean, max, count, bytes, diff) = code_diff(previous, &raw);
                    writeln!(report, "{label}_pair={}-{} mean_abs_rgb_codes={mean:.17} max_rgb_codes={max} pixels_gt2={count} different_rgba_bytes={bytes}", frame - 1, frame)?;
                    stability_failures += bytes;
                    if bytes != 0 {
                        image::save_buffer(out.join(format!("icc-coastal1-{label}-pair{:02}-{frame:02}-diff.png", frame - 1)), &diff, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                    }
                } else {
                    captures.push(raw.clone());
                }
                previous = Some(raw);
            }
        }
        let values = contribution(&captures[1], &captures[0]);
        ensure!(values.iter().any(|v| *v > 0.0), "{effect:?} contribution must not be empty");
        ensure!(captures[1].as_chunks::<4>().0.iter().zip(captures[0].as_chunks::<4>().0.iter()).all(|(on, off)| (0..3).all(|c| on[c] <= off[c])), "{effect:?} must not brighten any RGB channel");
        let after = grain(&values, &mut report, "after");
        save_contribution(&out, "after", &values)?;
        let after_mean = values.iter().sum::<f64>() / values.len() as f64;
        writeln!(report, "after_mean_attenuation={after_mean:.17}")?;
        let (before_grain, before_mean) = effect.static_bounds();
        writeln!(report, "recorded_before_static_grain={before_grain:.17}\nrecorded_before_mean_attenuation={before_mean:.17}")?;
        if !baseline_only {
            ensure!(after < before_grain, "{effect:?} grain must strictly improve over recorded pre-fix grain: after={after:.17}, before={before_grain:.17}");
            ensure!(after_mean >= before_mean * 0.5, "{effect:?} lost more than half its recorded pre-fix mean contribution");
        }
        if let Some(baseline) = &baseline {
            let load = |label: &str| -> anyhow::Result<Vec<u8>> {
                let image = image::open(baseline.join(format!("icc-coastal1-{label}-frame00.png")))?.into_rgba8();
                ensure!(image.dimensions() == SIZE, "baseline dimensions must be 1920x1200");
                Ok(image.into_raw())
            };
            let before_off = load("off")?;
            let before_on = load("on")?;
            let before_values = contribution(&before_on, &before_off);
            let before = grain(&before_values, &mut report, "before");
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
            // The optional PNG comparison is additional to the durable bounds.
            let before_mean = mean(&before_values);
            writeln!(report, "after_mean_attenuation={after_mean:.17}\nbefore_mean_attenuation={before_mean:.17}")?;
            ensure!(after_mean >= before_mean * 0.5, "{effect:?} lost more than half its mean contribution");
            save_contribution(&out, "before", &before_values)?;
            for (label, old, new) in [("off", &before_off, &captures[0]), ("on", &before_on, &captures[1])] {
                let (mean, max, count, bytes, diff) = code_diff(old, new);
                writeln!(report, "baseline_{label} mean_abs_rgb_codes={mean:.17} max_rgb_codes={max} pixels_gt2={count} different_rgba_bytes={bytes}")?;
                for (suffix, pixels) in [("before", old), ("after", new), ("diff", &diff)] {
                    image::save_buffer(out.join(format!("icc-coastal1-{label}-{suffix}.png")), pixels, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                }
            }
            std::fs::write(out.join("manifest.txt"), &report)?;
            ensure!(before_off == captures[0], "{effect:?}-off baseline changed: camera/scene/shaders are not a controlled pair");
            ensure!(after < before, "{effect:?} grain must strictly improve: after={after:.17}, before={before:.17}");
        } else if baseline_only {
            writeln!(report, "grain_acceptance=BASELINE_ONLY_NO_GRAIN_BOUND_ASSERTED")?;
        } else {
            writeln!(report, "grain_acceptance=RECORDED_PRE_FIX_BOUNDS")?;
        }
        ensure!(stability_failures == 0, "static frames differ in {stability_failures} RGBA bytes; see manifest and PNGs");
        motion::capture(&mut off, &mut shot, &out, baseline.as_deref(), baseline_only, effect, &mut report)?;
        if std::env::var_os(format!("{}_TIMING", effect.env_prefix())).is_some() {
            off.prepare_frozen_rendering();
            off.set_gpu_timing(true)?;
            let mut means = Vec::new();
            for (phase, enabled) in [false, true, true, false].into_iter().enumerate() {
                let settings = effect.settings(enabled)?;
                off.set_enhanced_settings(settings);
                let mut sum = 0.0;
                for frame in 0..90 {
                    off.render_frozen()?;
                    let ms = off.read_frame_gpu_ms()?.context("GPU timestamps unavailable")?;
                    if frame >= 30 { sum += ms; }
                }
                let mean = sum / 60.0;
                means.push(mean);
                writeln!(report, "timing_phase={phase} effect={effect:?} enabled={enabled} warmup=30 samples=60 mean_wholeframe_gpu_ms={mean:.9}")?;
            }
            writeln!(report, "paired_{}_on_minus_off_wholeframe_gpu_ms={:.9}\ntiming_scope=whole_frame_not_isolated_effect", effect.name(), (means[1] + means[2] - means[0] - means[3]) * 0.5)?;
        } else {
            writeln!(report, "gpu_timing=unexercised")?;
        }
        Ok(())
    })();
    eprintln!("{report}");
    if let Err(error) = &result { writeln!(report, "error={error:#}")?; }
    std::fs::write(out.join("manifest.txt"), &report)?;
    std::fs::write(out.join("status.txt"), if result.is_ok() { "PASS\n" } else { "FAIL\n" })?;
    result
}
