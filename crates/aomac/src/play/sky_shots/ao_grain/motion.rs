use super::*;

pub(super) fn capture(off: &mut Offscreen, shot: &mut Shot,
    out: &std::path::Path, baseline: Option<&std::path::Path>, baseline_only: bool, effect: Effect, report: &mut String,
) -> anyhow::Result<()> {
    let fixed = shot.camera;
    for sequence in ["tiny", "orbit"] {
        // docs/zone/camera.md records ~1.7mm idle eye-height movement.
        // A fixed 1mm sine cycle and a slow 10m-radius orbit exercise that failure
        // without advancing scene/animation time. Each pose has a paired control.
        const N: usize = 16;
        writeln!(report, "sequence={sequence} frames={N} tiny_y_amplitude_m=0.001 orbit_step_rad=0.0005 orbit_radius_m=10")?;
        let target = fixed.pos + fixed.forward() * 10.0;
        let mut previous: [Option<Vec<f64>>; 2] = [None, None];
        let mut totals = [(0.0_f64, 0.0_f64, 0_usize, 0_usize); 2];
        let mut hf = [0.0; 2];
        for frame in 0..N {
            let pose = if sequence == "tiny" {
                Camera { pos: fixed.pos + Vec3::Y * (0.001 * (frame as f32 * std::f32::consts::TAU / N as f32).sin()), ..fixed }
            } else {
                Camera::look_at(target + glam::Quat::from_rotation_y(frame as f32 * 0.0005) * (fixed.pos - target), target)
            };
            shot.camera = pose;
            off.fixed_camera(Some(pose));
            writeln!(report, "{sequence}_frame={frame} camera={pose:?}")?;
            let mut pairs = [Vec::new(), Vec::new()];
            for enabled in [false, true] {
                let label = if enabled { "on" } else { "off" };
                let settings = effect.settings(enabled)?;
                off.host.enhanced_settings = Some(settings);
                let list = off.frame(shot, 0.0);
                let raw = off.rgba(shot, &list)?;
                let filename = format!("icc-coastal1-{sequence}-{label}-frame{frame:02}.png");
                image::save_buffer(out.join(&filename), &raw, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                if let Some(baseline) = baseline {
                    let old = image::open(baseline.join(&filename))?.into_rgba8();
                    ensure!(old.dimensions() == SIZE, "motion baseline dimensions changed");
                    let old = old.into_raw();
                    let (_, _, _, _, diff) = code_diff(&old, &raw);
                    image::save_buffer(out.join(format!("before-{filename}")), &old, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                    image::save_buffer(out.join(format!("diff-{filename}")), &diff, SIZE.0, SIZE.1, image::ColorType::Rgba8)?;
                    if !enabled { ensure!(old == raw, "{sequence} frame {frame}: {effect:?}-off baseline changed"); }
                    pairs[1].push(old);
                }
                pairs[0].push(raw);
            }
            for index in 0..if baseline.is_some() { 2 } else { 1 } {
                let values = contribution(&pairs[index][1], &pairs[index][0]);
                ensure!(values.iter().any(|v| *v > 0.0), "{sequence}: empty {effect:?} contribution");
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                writeln!(report, "{sequence}_baseline={index} frame={frame} mean_attenuation={mean:.17}")?;
                if index == 0 && !baseline_only {
                    let (_, before_mean) = effect.static_bounds();
                    ensure!(mean >= before_mean * 0.5, "{sequence}: {effect:?} lost more than half its recorded pre-fix mean contribution");
                }
                ensure!(pairs[index][1].as_chunks::<4>().0.iter().zip(pairs[index][0].as_chunks::<4>().0.iter()).all(|(on, off)| (0..3).all(|c| on[c] <= off[c])), "{sequence}: {effect:?} brightens an RGB channel");
                save_contribution(out, &format!("{sequence}-baseline{index}-frame{frame:02}"), &values)?;
                hf[index] += grain(&values, report, &format!("{sequence}_baseline{index}_frame{frame:02}"));
                if let Some(previous) = &previous[index] {
                    let (mut sum, mut max, mut over_two, mut count) = (0.0, 0.0_f64, 0, 0);
                    for (_, x, y, width, height) in CROPS {
                        for row in y..y + height {
                            for col in x..x + width {
                                let pixel = (row * SIZE.0 + col) as usize;
                                let delta = (values[pixel] - previous[pixel]).abs() * 255.0;
                                sum += delta;
                                max = max.max(delta);
                                over_two += usize::from(delta > 2.0);
                                count += 1;
                            }
                        }
                    }
                    writeln!(report, "{sequence}_baseline={index} pair={}-{} mean_abs_contribution_codes={:.17} max_contribution_codes={max:.17} pixels_gt2={over_two}", frame - 1, frame, sum / count as f64)?;
                    totals[index].0 += sum;
                    totals[index].1 = totals[index].1.max(max);
                    totals[index].2 += over_two;
                    totals[index].3 += count;
                }
                previous[index] = Some(values);
            }
        }
        for (index, (sum, max, over_two, count)) in totals.iter().enumerate() {
            if *count > 0 {
                writeln!(report, "{sequence}_baseline={index} temporal_mean_codes={:.17} temporal_max_codes={max:.17} temporal_pixels_gt2={over_two} grain_mse={:.17}", sum / *count as f64, hf[index] / N as f64)?;
            }
        }
        let (before_mean, before_max, before_count, before_grain) = effect.motion_bounds(sequence);
        writeln!(report, "{sequence}_recorded_before_temporal_mean_codes={before_mean:.17} recorded_before_temporal_max_codes={before_max:.17} recorded_before_temporal_pixels_gt2={before_count} recorded_before_grain_mse={before_grain:.17}")?;
        std::fs::write(out.join("manifest.txt"), &*report)?;
        if !baseline_only {
            let mean = totals[0].0 / totals[0].3 as f64;
            ensure!(mean < before_mean, "{sequence}: temporal mean contribution change did not improve over recorded pre-fix bound: after={mean:.17}, before={before_mean:.17}");
            ensure!(totals[0].1 <= before_max, "{sequence}: temporal max contribution change exceeds recorded pre-fix bound");
            ensure!(totals[0].2 <= before_count, "{sequence}: temporal pixels >2 exceed recorded pre-fix bound");
            ensure!(hf[0] / (N as f64) < before_grain, "{sequence}: mean spatial grain did not improve over recorded pre-fix bound");
        }
        if baseline.is_some() {
            ensure!(totals[0].0 < totals[1].0, "{sequence}: temporal mean contribution change did not improve");
            ensure!(totals[0].1 <= totals[1].1, "{sequence}: temporal max contribution change regressed");
            ensure!(totals[0].2 <= totals[1].2, "{sequence}: temporal pixels >2 regressed");
            ensure!(hf[0] < hf[1], "{sequence}: mean spatial grain did not improve");
        }
    }
    Ok(())
}
