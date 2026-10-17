//! Real RDB/capture regression for emitter admission, not a synthetic emissive material.
use super::*;
use anyhow::{Context, ensure};
use ao_formats::character::{actor::{ActorAssets, PoseCache}, NpcRecord};
use ao_scene::{ActorFrame, Environment};
use std::{fmt::Write, sync::Arc};

#[test]
#[ignore = "headless real-client bloom MRT regression; writes PNG crops to new AOMAC_GFX_OUT"]
fn graphics_bloom_shore_snake_seed() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a new evidence directory")?);
    ensure!(!out.exists(), "refusing to overwrite {}", out.display());
    std::fs::create_dir_all(&out)?;
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let (id, appearance) = captured_shore_snake()?;
    ensure!(appearance.monster_data == 30252, "captured Shore Snake MonsterData changed");
    let record = NpcRecord::load(&store, appearance.monster_data as u32)?;
    ensure!(record.mesh() == Some(23353), "Shore Snake CAT mesh changed");
    let assets = ActorAssets::new(&store)?;
    let rig = super::super::dynels::npc_rig(&store, &assets,
        &super::super::dynels::CharLook::from_update(&appearance), &record, None, &[])?;
    let snake = rig.model_arc();
    ensure!(snake.meshes.iter().flat_map(|m| &m.submeshes).any(|s| s.emissive.iter().any(|&v| v > 0.0)),
        "fixture must retain the D3D emissive tint that caused the bug");
    ensure!(snake.meshes.iter().flat_map(|m| &m.submeshes).all(|s| !s.glow_mask),
        "Shore Snake body unexpectedly has an authored alpha self-illumination mask");
    // Native CAT material emissive is (0.5, 0.5, 0.5), a partial lighting floor.
    ensure!(snake.meshes.iter().flat_map(|m| &m.submeshes).all(|s| s.emissive.iter().all(|&v| v < 1.0)),
        "Shore Snake must exercise partial emissive, not a self-saturating emitter");
    let (skin_pose, parts) = rig.pose_palette_composed_cached(std::iter::empty(), &mut PoseCache::default());
    let snake_frame = ActorFrame { id, model: 1, skin_pose: Some(skin_pose), parts, part_attractors: rig.part_attractors(), ..Default::default() };
    let lamp = Arc::new(ao_formats::mesh::load_mesh(&store, 26163)?);
    ensure!(lamp.meshes.iter().flat_map(|m| &m.submeshes).any(|s|
        s.glow_mask || s.emissive.iter().any(|&v| v >= 1.0)),
        "captured Light Bar mesh26163 must retain authored self-illumination");
    let mut report = format!("source=docs/captures/zone_ithaca.rec actor_id={id} actor_name={:?} MonsterData=1040023:30252 CAT=1010002:23353 texture=giant_snake:23354 pose=bind\npositive_control=Light_Bar mesh=1010001:26163\nsize=512x384 msaa=1 anisotropy=1 time=0\n", appearance.name);
    let mut seed_counts = Vec::new();
    let mut faithful_matches = true;
    let mut snake_beauty_differences = 0;
    let mut objects = vec![
        ("shore-snake", snake, snake_frame),
        ("light-bar", lamp, ActorFrame { id: 2, model: 1, ..Default::default() }),
    ];
    for (name, mesh) in [
        ("spikytree-6264", 6264), ("barchair-8752", 8752), ("cupboard-10295", 10295),
        ("shelf-10298", 10298), ("lamp-13937", 13937), ("screen-sign-6113", 6113),
        ("partial-neon-6203", 6203),
    ] {
        writeln!(report, "{name} archive_mesh=1010001:{mesh}")?;
        objects.push((name, Arc::new(ao_formats::mesh::load_mesh(&store, mesh)?),
            ActorFrame { id: mesh, model: 1, ..Default::default() }));
    }
    for (name, model, actor) in objects {
        for (mesh, body) in model.meshes.iter().enumerate() {
            for (material, submesh) in body.submeshes.iter().enumerate() {
                writeln!(report, "{name} mesh={mesh} material={material} linear_emissive={:?} glow_mask={}", submesh.emissive, submesh.glow_mask)?;
            }
        }
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for vertex in model.meshes.iter().flat_map(|m| &m.vertices) {
            low = low.min(vertex.pos.into());
            high = high.max(vertex.pos.into());
        }
        ensure!(low.is_finite() && high.is_finite(), "{name}: missing real geometry");
        let center = (low + high) * 0.5;
        let radius = (high - low).length() * 0.5;
        ensure!(radius > 0.0, "{name}: empty bounds");
        let offset = if name == "shore-snake" { Vec3::new(radius * 3.5, radius * 0.5, 0.0) }
            else { Vec3::new(0.0, radius * 0.5, radius * 3.5) };
        let camera = Camera::look_at(center + offset, center);
        let scene = Scene { environment: Some(Environment {
            sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 1000.0, fog_end: 2000.0,
            ambient: [0.05; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0,
        }), ..Default::default() };
        let size = (512, 384);
        let mut faithful = None;
        for profile in ["faithful", "bloom"] {
            let lens = Lens { near: (radius * 0.02).max(0.001), ..Default::default() };
            let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera, lens };
            let mut off = Offscreen::evidence(&shot, size)?;
            off.fixed_camera(Some(camera));
            off.host.msaa_samples = Some(1);
            off.host.anisotropy = Some(1);
            off.host.enhanced_settings = Some(graphics_fixture_settings(profile)?);
            let list = off.frame(&mut shot, 0.0);
            let empty = off.rgba(&shot, &list)?;
            off.host.actor_models.push((1, Arc::clone(&model)));
            off.host.actors.push(actor.clone());
            let list = off.frame(&mut shot, 0.0);
            let raw = off.rgba(&shot, &list)?;
            let visible = raw.as_chunks::<4>().0.iter().zip(empty.as_chunks::<4>().0)
                .filter(|(a, b)| a[..3] != b[..3]).count();
            ensure!(visible > 0, "{name}/{profile}: actual geometry did not render");
            image::save_buffer(out.join(format!("{name}-{profile}-crop.png")), &raw, size.0, size.1, image::ColorType::Rgba8)?;
            writeln!(report, "{name}/{profile} camera={camera:?} visible_pixels={visible}")?;
            if profile == "faithful" {
                ensure!(off.emissive_seed_rgba()?.is_none(), "faithful path allocated an enhanced seed target");
                if let Some(base) = std::env::var_os("AOMAC_GFX_BASELINE") {
                    let previous = image::open(PathBuf::from(base).join(format!("{name}-faithful-crop.png")))?.into_rgba8().into_raw();
                    let differences = raw.iter().zip(&previous).filter(|(a, b)| a != b).count() + raw.len().abs_diff(previous.len());
                    faithful_matches &= differences == 0;
                    writeln!(report, "{name}/faithful baseline_differing_bytes={differences}")?;
                }
                faithful = Some(raw);
            } else {
                let seed = off.emissive_seed_rgba()?.context("bloom MRT seed attachment missing")?;
                image::save_buffer(out.join(format!("{name}-emissive-seed-crop.png")), &seed, size.0, size.1, image::ColorType::Rgba8)?;
                let nonzero = seed.as_chunks::<4>().0.iter().filter(|pixel| pixel[..3].iter().any(|&v| v != 0)).count();
                let mut rgb_sum = [0_u64; 3];
                let mut rgb_max = [0_u8; 3];
                let mut rec709_sum = 0.0_f64;
                let mut rec709_max = 0.0_f64;
                for pixel in seed.as_chunks::<4>().0 {
                    for channel in 0..3 {
                        rgb_sum[channel] += u64::from(pixel[channel]);
                        rgb_max[channel] = rgb_max[channel].max(pixel[channel]);
                    }
                    let y = (0.2126 * f64::from(pixel[0]) + 0.7152 * f64::from(pixel[1]) + 0.0722 * f64::from(pixel[2])) / 255.0;
                    rec709_sum += y;
                    rec709_max = rec709_max.max(y);
                }
                writeln!(report, "{name}/bloom seed_rgb_byte_sum={rgb_sum:?} seed_rgb_normalized_sum={:?} seed_rgb_byte_max={rgb_max:?} seed_rec709_normalized_sum={rec709_sum:.9} seed_rec709_normalized_max={rec709_max:.9}",
                    rgb_sum.map(|sum| sum as f64 / 255.0))?;
                seed_counts.push(nonzero);
                let differences = raw.iter().zip(faithful.as_ref().context("missing faithful capture")?).filter(|(a, b)| a != b).count();
                if name == "shore-snake" { snake_beauty_differences = differences; }
                writeln!(report, "{name}/bloom nonzero_seed_rgb_pixels={nonzero} faithful_differing_bytes={differences}")?;
            }
        }
    }
    // Keep all before/after evidence even when the pre-fix shader correctly fails this regression.
    std::fs::write(out.join("bloom-seeds.txt"), &report)?;
    eprintln!("{report}");
    ensure!(seed_counts[0] == 0, "Shore Snake D3D material tint leaked into {} seed pixels", seed_counts[0]);
    ensure!(seed_counts[1] > 0, "real Light Bar authored glow did not seed bloom");
    ensure!(snake_beauty_differences == 0, "zero-emitter Shore Snake bloom changed {snake_beauty_differences} faithful beauty bytes");
    ensure!(faithful_matches, "faithful rendered bytes changed against baseline");
    Ok(())
}

fn captured_shore_snake() -> anyhow::Result<(u32, ao_net::n3::dynel::SimpleCharFullUpdate)> {
    use ao_net::{frame::{Frame, PT_N3}, n3::{self, N3, dynel::Dynel}};
    for line in include_str!("../../../../docs/captures/zone_ithaca.rec").lines() {
        let mut words = line.split_whitespace();
        let (Some(_), Some("<"), Some(hex)) = (words.next(), words.next(), words.next()) else { continue };
        let bytes = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)).collect::<Result<Vec<_>, _>>()?;
        let Some((frame, _)) = Frame::decode_with(&bytes, false)? else { continue };
        if frame.ptype != PT_N3 { continue; }
        let message = n3::decode(&frame)?;
        if let N3::Dynel(Dynel::SimpleCharFullUpdate(update)) = message.body {
            if update.name == "Shore Snake" { return Ok((message.header.target.instance as u32, *update)); }
        }
    }
    anyhow::bail!("zone_ithaca.rec has no captured Shore Snake")
}
