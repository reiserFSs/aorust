//! Installed-client GPU regression: `cargo test --release -p aomac --test liquid_real -- --nocapture`.

#[test]
fn icc_sea_contains_alpha_blended_water_pixels() -> anyhow::Result<()> {
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/rdb.db").exists() {
        eprintln!("skipping ICC water pixels: no installed client at {}", dir.display());
        return Ok(());
    }
    let store = ao_rdb::RecordStore::open(&dir)?;
    let mut scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, 2648.69)?;
    assert!(scene.liquids.iter().any(|surface| surface.style == 0), "ICC must contain native water surfaces");
    assert!(scene.meshes.iter().flat_map(|mesh| &mesh.submeshes).any(|sub| {
        sub.liquid && sub.texture.is_some_and(|key| key.rdb_type == 1_010_004 && key.id == 6329)
    }), "the real water5 texture must be bound to a liquid");
    let eye = [1010.4, 24.0, -663.1];
    let at = [1090.0, 15.0, -571.0];
    let out = std::env::temp_dir().join(format!("aomac-icc-water-{}", std::process::id()));
    std::fs::create_dir_all(&out)?;
    let wet_path = out.join("wet.png");
    let dry_path = out.join("dry.png");
    ao_render::render_to_png_at(&scene, eye, at, 640, 400, &wet_path, 0.0)?;
    // Remove only water draws, preserving every terrain/statel/sky/light and the camera.
    // Clearing the sources also prevents update_liquids from regenerating the removed indices.
    scene.liquids.clear();
    for mesh in &mut scene.meshes {
        mesh.submeshes.retain(|sub| !sub.liquid);
    }
    ao_render::render_to_png_at(&scene, eye, at, 640, 400, &dry_path, 0.0)?;
    let wet = image::open(&wet_path)?.to_rgba8();
    let dry = image::open(&dry_path)?.to_rgba8();
    std::fs::remove_file(wet_path)?;
    std::fs::remove_file(dry_path)?;
    std::fs::remove_dir(out)?;
    // Test actual visible water, not just decoded geometry or a synthetic stand-in.
    // SRCCOLOR/ONE (the stale pipeline slots 28/29) can only brighten the destination;
    // native water's two alpha passes also attenuate it. Ignore the upper sky half.
    let mut changed = 0;
    let mut attenuated = 0;
    for (water, background) in wet.pixels().zip(dry.pixels()).skip(640 * 200) {
        let delta: [i16; 3] = std::array::from_fn(|i| i16::from(water[i]) - i16::from(background[i]));
        changed += usize::from(delta.iter().any(|value| value.abs() > 8));
        attenuated += usize::from(delta.iter().sum::<i16>() < -24);
    }
    eprintln!("ICC sea: {changed} changed pixels, {attenuated} attenuated water pixels");
    assert!(changed > 1000, "real ICC water must cover visible pixels, got {changed}");
    assert!(attenuated > 100, "native water must alpha-blend, not use the sky's additive glow: {attenuated} pixels");
    Ok(())
}

/// Open ICC sea seen from above (no terrain in the central block): the native water material must show
/// water5 texture detail and take its colour from the authored WaterLight (DS1003f3db / 10059b98), not
/// from a flat tint. Numbers measured 2026-10-19 (see docs/formats.md, "Native water appearance").
#[test]
fn icc_open_sea_shows_water5_detail_lit_by_authored_water_light() -> anyhow::Result<()> {
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/rdb.db").exists() {
        eprintln!("skipping ICC open sea: no installed client at {}", dir.display());
        return Ok(());
    }
    let store = ao_rdb::RecordStore::open(&dir)?;
    let scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, 2648.69)?;
    let (eye, at) = ([1500.0, 40.0, -1200.0], [1500.0, 17.0, -1160.0]);
    let out = std::env::temp_dir().join(format!("aomac-icc-open-sea-{}", std::process::id()));
    std::fs::create_dir_all(&out)?;
    // Mean and per-channel standard deviation of the central 200x100 block.
    let stats = |scene: &ao_scene::Scene, name: &str| -> anyhow::Result<([f64; 3], [f64; 3])> {
        let path = out.join(name);
        ao_render::render_to_png_at(scene, eye, at, 640, 400, &path, 0.0)?;
        let image = image::open(&path)?.to_rgb8();
        let (mut sum, mut square, mut n) = ([0f64; 3], [0f64; 3], 0f64);
        for y in 250..350 {
            for x in 220..420 {
                for c in 0..3 {
                    let v = f64::from(image.get_pixel(x, y)[c]);
                    sum[c] += v;
                    square[c] += v * v;
                }
                n += 1.0;
            }
        }
        let mean = sum.map(|s| s / n);
        Ok((mean, std::array::from_fn(|c| (square[c] / n - mean[c] * mean[c]).max(0.0).sqrt())))
    };
    let (mean, std) = stats(&scene, "authored.png")?;
    let mut dark = scene.clone();
    dark.water_light = Some(([0.0; 3], dark.water_light.map_or([0.0, 1.0, 0.0], |light| light.1)));
    let (dark_mean, _) = stats(&dark, "dark.png")?;
    let mut flat = scene.clone();
    let key = ao_scene::TextureKey { rdb_type: 1_010_004, id: 6329 };
    let texture = flat.textures.get_mut(&key).expect("water5 texture");
    let grey = (texture.rgba.as_chunks::<4>().0.iter().map(|p| u32::from(p[0])).sum::<u32>() / (texture.rgba.len() as u32 / 4)) as u8;
    texture.rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[..3].fill(grey));
    let (_, flat_std) = stats(&flat, "flat.png")?;
    std::fs::remove_dir_all(out)?;
    eprintln!("ICC open sea: mean {mean:?} std {std:?} dark-light mean {dark_mean:?} flat-texture std {flat_std:?}");
    assert!(mean[2] > mean[1] && mean[1] > mean[0], "authored 0xcc00186a blue water, fog-lifted: {mean:?}");
    assert!(mean[2] - dark_mean[2] > 10.0, "colour must come from the authored WaterLight diffuse: lit {mean:?} vs WaterLight black {dark_mean:?}");
    assert!((2.0..8.0).contains(&std[2]), "water5 detail must be present but low-contrast, blue std {std:?}");
    assert!(flat_std[2] < std[2] * 0.5, "flattening water5 must remove the detail: {flat_std:?} vs {std:?}");
    Ok(())
}

/// Randy offscreen technology (pref `UseOffscreenSurfaceTechnology`, default off) + VisualLiquid `RenderRefraction` (DS 1003d13a/1003b9c9).
/// Off: byte-identical to the default render. On: render target 1 is presented through the heat-haze grid and the horizon
/// band (alpha = tent around the horizon, mirrored target-2 sample) changes the sea there.
#[test]
fn icc_offscreen_technology_is_off_by_default_and_adds_the_horizon_refraction_band() -> anyhow::Result<()> {
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/rdb.db").exists() {
        eprintln!("skipping ICC offscreen technology: no installed client at {}", dir.display());
        return Ok(());
    }
    let store = ao_rdb::RecordStore::open(&dir)?;
    let scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, 2648.69)?;
    let (eye, at) = ([933.5, 26.0, -748.0], [933.5 - 59.0, 26.0 + 4.6, -748.0 + 81.0]);
    let out = std::env::temp_dir().join(format!("aomac-icc-offscreen-{}", std::process::id()));
    std::fs::create_dir_all(&out)?;
    let shot = |name: &str, offscreen: Option<bool>| -> anyhow::Result<image::RgbaImage> {
        let path = out.join(name);
        match offscreen {
            None => { ao_render::render_to_png_at(&scene, eye, at, 640, 400, &path, 0.0)?; }
            Some(on) => { ao_render::render_to_png_offscreen(&scene, eye, at, 640, 400, &path, 0.0, on)?; }
        }
        Ok(image::open(&path)?.to_rgba8())
    };
    let default = shot("default.png", None)?;
    let off = shot("off.png", Some(false))?;
    let on = shot("on.png", Some(true))?;
    std::fs::remove_dir_all(&out)?;
    // The pref off path is the untouched faithful frame (single-frame default vs two-frame option-off differ only by the water phase clock).
    let differing = |a: &image::RgbaImage, b: &image::RgbaImage, rows: std::ops::Range<u32>| a.enumerate_pixels()
        .filter(|(x, y, p)| rows.contains(y) && p.0.iter().zip(b.get_pixel(*x, *y).0).any(|(l, r)| (i16::from(*l) - i16::from(r)).abs() > 6)).count();
    let off_vs_default = differing(&off, &default, 0..400);
    let on_vs_off = differing(&on, &off, 0..400);
    eprintln!("ICC offscreen: option-off vs default {off_vs_default} px, option-on vs off {on_vs_off} px");
    assert!(off_vs_default < 640 * 400 / 50, "option off must not change the frame: {off_vs_default}");
    assert!(on_vs_off > 640 * 400 / 20, "option on must change the frame (target-1 present + band): {on_vs_off}");
    Ok(())
}
