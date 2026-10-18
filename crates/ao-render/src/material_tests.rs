//! Asset-free material evidence. Run explicitly under the main agent's GPU timing lock.
use super::*;
use anyhow::{Context, ensure};
use std::{fmt::Write as _, path::PathBuf, time::Instant};

const W: u32 = 640;
const H: u32 = 400;
const NAMES: [&str; 4] = ["alloff", "generated_normals", "material_maps", "material_pbr"];

fn png_file(path: &Path, w: u32, h: u32, pixels: &[u8]) -> anyhow::Result<()> {
    let mut encoder = png::Encoder::new(std::fs::File::create(path)?, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}

struct MapDirectory(PathBuf);
impl Drop for MapDirectory {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn fixture(root: &Path) -> anyhow::Result<Scene> {
    use ao_scene::{Texture, TextureSource, Mesh, Vertex, Submesh, Instance, IDENTITY};
    use ao_scene::ground_detail::{GroundCell, GroundPatch};
    let tile = TextureKey { rdb_type: 1_010_006, id: 71 };
    let mesh_key = TextureKey { rdb_type: 1_010_004, id: 72 };
    let composite = TextureKey { rdb_type: 0, id: 73 };
    let palette = TextureKey { rdb_type: 1_000_007, id: 74 };
    let mut albedo = Vec::new();
    let mut normal = Vec::new();
    let mut orm = Vec::new();
    let mut ao = Vec::new();
    let mut rough = Vec::new();
    let mut metal = Vec::new();
    let mut emissive = Vec::new();
    for y in 0..64 { for x in 0..64 {
        // The native coarse composite keeps the 8x8 tile mip: retain features
        // wider than its source bins so generated normals have a real signal.
        let stripe = (x / 16 + y / 16) % 2 == 0;
        albedo.extend_from_slice(if stripe { &[172, 137, 90, 255] } else { &[58, 70, 85, 255] });
        normal.extend_from_slice(if stripe { &[220, 128, 214, 255] } else { &[45, 170, 214, 255] });
        orm.extend_from_slice(&[if stripe { 64 } else { 255 }, if x < 32 { 38 } else { 215 }, 180, 255]);
        ao.extend_from_slice(&[if stripe { 48 } else { 255 }; 4]);
        rough.extend_from_slice(&[64; 4]);
        metal.extend_from_slice(&[180; 4]);
        emissive.extend_from_slice(if x % 16 < 3 && y % 16 < 8 { &[180, 38, 9, 255] } else { &[0, 0, 0, 255] });
    }}
    for key in [tile, mesh_key, composite] {
        for (suffix, bytes) in [("n", &normal), ("e", &emissive)] {
            png_file(&root.join(format!("{}_{}_{suffix}.png", key.rdb_type, key.id)), 64, 64, bytes)?;
        }
        if key == mesh_key {
            png_file(&root.join(format!("{}_{}_orm.png", key.rdb_type, key.id)), 64, 64, &orm)?;
        } else {
            for (suffix, bytes) in [("ao", &ao), ("r", &rough), ("m", &metal)] {
                png_file(&root.join(format!("{}_{}_{suffix}.png", key.rdb_type, key.id)), 64, 64, bytes)?;
            }
        }
    }
    let mut scene = Scene::default();
    for (key, source) in [(tile, TextureSource::GroundRgba), (mesh_key, TextureSource::Rgba)] {
        scene.textures.insert(key, Texture { width: 64, height: 64, rgba: albedo.clone(), source });
    }
    scene.textures.insert(composite, Texture { width: 8, height: 8, rgba: [130, 110, 80, 255].repeat(64), source: TextureSource::GroundComposite565 });
    scene.textures.insert(palette, Texture { width: 1, height: 1, rgba: vec![60, 60, 60, 255], source: TextureSource::Rgba });
    let vertices = [[0.0, 0.0, 0.0], [6.0, 0.0, 0.0], [0.0, 0.0, -6.0], [6.0, 0.0, -6.0]].into_iter().enumerate().map(|(i, pos)| Vertex {
        pos, normal: [0.0, 1.0, 0.0], uv: [(i % 2) as f32, 1.0 - (i / 2) as f32], color: [1.0; 4],
    }).collect();
    let mut fine = Submesh::new(vec![0, 1, 2, 1, 3, 2], Some(tile));
    fine.two_sided = true;
    let mut coarse = Submesh::new(vec![0; 6], Some(composite));
    coarse.two_sided = true;
    scene.meshes.push(Mesh { vertices, submeshes: vec![fine, coarse] });
    scene.ground_detail.push(GroundPatch { mesh: 0, cells: 1, cell_size: 6.0, heights: vec![0; 4], coarse_submesh: 1,
        fine_cells: vec![GroundCell { submesh: 0, vertices: [0, 1, 2, 3], ..Default::default() }],
        shadow_texture: Some(palette), shadow_uv_scale: [1.0 / 6.0; 2], ..Default::default() });
    // A real tilted plate aligns the sun/view half-vector with its geometric
    // normal, making the independently enabled per-pixel GGX lobe unmistakable.
    let vertices = [[1.0, 0.0, -4.0], [5.0, 0.0, -4.0], [5.0, 3.0, -5.12], [1.0, 3.0, -5.12]].into_iter().enumerate().map(|(i, pos)| Vertex {
        pos, normal: [0.0, 0.3498, 0.9368], uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][i], color: [1.0; 4],
    }).collect();
    let mut material = Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(mesh_key));
    material.two_sided = true;
    material.specular = [0.04; 3];
    material.shininess = 128.0;
    scene.meshes.push(Mesh { vertices, submeshes: vec![material] });
    scene.instances = (0..2).map(|mesh| Instance { mesh, transform: IDENTITY }).collect();
    scene.environment = Some(ao_scene::Environment { sky_color: [0.04, 0.06, 0.09], fog_color: [0.04; 3], fog_start: 100.0, fog_end: 200.0,
        ambient: [0.12; 3], sun_color: [0.65, 0.6, 0.52], sun_dir: [0.0, 0.5, 0.8660254], sun_specular: 0.6 });
    let source = &scene.textures[&tile];
    let built = native_texture::build_texture_mips(Default::default(), source.source, false,
        &source.rgba, source.width, source.height, false);
    let mips = [(tile, built.ground_tile_mip().expect("ground fixture tile mip"))].into_iter().collect();
    let images = native_ground::composite_images(&native_ground::composites(&scene), &scene.textures, &mips);
    let image = &images[&composite];
    assert!(image.rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[..3] != image.rgba[..3]),
        "native 8x8 composite must retain fixture brightness variation");
    let generated = native_materials::build(composite, &image.rgba, image.width, image.height, image.source, None,
        native_materials::Selection { generated: true, maps: false }).generated_normal.unwrap();
    assert!(generated.levels[0].as_chunks::<4>().0.iter().any(|pixel| pixel[..3] != [128, 128, 255]),
        "native coarse composite must generate nonflat normals");
    Ok(scene)
}

fn sheet(images: &[Vec<u8>], columns: usize) -> Vec<u8> {
    let rows = images.len().div_ceil(columns);
    let mut bytes = vec![0; W as usize * H as usize * columns * rows * 4];
    for (index, image) in images.iter().enumerate() {
        for y in 0..H as usize {
            let dst = ((index / columns * H as usize + y) * W as usize * columns + index % columns * W as usize) * 4;
            let src = y * W as usize * 4;
            bytes[dst..dst + W as usize * 4].copy_from_slice(&image[src..src + W as usize * 4]);
        }
    }
    bytes
}

#[test]
fn material_uploads_are_lazy_and_live_toggles_release_maps() -> anyhow::Result<()> {
    let root = MapDirectory(std::env::temp_dir().join(format!("aomac-material-lazy-{}-{}",
        std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos())));
    std::fs::create_dir(&root.0)?;
    let scene = fixture(&root.0)?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None)?;
    renderer.set_materials_root(Some(root.0.clone()));
    renderer.upload(&scene);
    renderer.add_actor_model(1, &scene);
    let actor_sources = renderer.actor_material_sources();
    let world_sources: Vec<_> = renderer.material_bindings.iter().map(|binding| binding.source.clone()).collect();
    assert!(!world_sources.is_empty() && !actor_sources.is_empty());
    assert!(world_sources.iter().chain(&actor_sources).all(|source| source.views.lock().is_none()),
        "alloff upload must allocate only shared fallback maps");
    let vertices = renderer.gpu.meshes[0].as_ref().unwrap().0.clone();
    renderer.set_enhanced_settings(EnhancedSettings { material_pbr: true, ..Default::default() });
    assert!(world_sources.iter().chain(&actor_sources).all(|source| source.views.lock().is_none()),
        "PBR alone must not generate normals or load side maps");
    renderer.set_enhanced_settings(EnhancedSettings { generated_normals: true, ..Default::default() });
    let fallback = renderer.material_fallbacks();
    for source in world_sources.iter().chain(&actor_sources) {
        let views = source.views.lock();
        let maps = views.as_ref().expect("live enable populates existing texture cache");
        assert_ne!(maps.generated, fallback.generated);
        assert_eq!(maps.normal, fallback.normal, "generated-only does not load normal PNGs");
        assert_eq!(maps.orm, fallback.orm, "generated-only does not load ORM PNGs");
        assert_eq!(maps.emissive, fallback.emissive, "generated-only does not load emissive PNGs");
    }
    renderer.set_enhanced_settings(EnhancedSettings { material_maps: true, ..Default::default() });
    assert!(world_sources.iter().chain(&actor_sources).all(|source|
        source.views.lock().as_ref().is_some_and(|maps| maps.generated == renderer.material_fallbacks().generated)),
        "map-only switch releases generated normal allocations");
    assert!(world_sources.iter().any(|source|
        source.views.lock().as_ref().is_some_and(|maps| maps.normal != renderer.material_fallbacks().normal)),
        "live map enable loads side maps for already uploaded textures");
    renderer.request_enhanced_settings(EnhancedSettings { generated_normals: true, ..Default::default() });
    renderer.finish_materials(true);
    assert!(renderer.pending_materials.is_none(), "explicit barrier drains the one-shot background material load");
    renderer.set_enhanced_settings(EnhancedSettings::default());
    assert!(world_sources.iter().chain(&actor_sources).all(|source| source.views.lock().is_none()),
        "alloff releases per-texture material GPU maps");
    assert_eq!(renderer.gpu.meshes[0].as_ref().unwrap().0, vertices, "toggles rebind materials without geometry upload");
    Ok(())
}

#[test]
#[ignore = "Explicit asset-free material image evidence and ABBA GPU timestamps; main holds GPU timing lock"]
fn material_graphics_evidence() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a new directory")?);
    std::fs::create_dir(&out).context("evidence directory must not already exist")?;
    let root = MapDirectory(std::env::temp_dir().join(format!("aomac-material-maps-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos())));
    std::fs::create_dir(&root.0)?;
    let scene = fixture(&root.0)?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let profiles = [EnhancedSettings::default(), EnhancedSettings { generated_normals: true, ..Default::default() },
        EnhancedSettings { material_maps: true, ..Default::default() }, EnhancedSettings { material_pbr: true, ..Default::default() }];
    let mut renderers = Vec::new();
    for settings in profiles {
        let mut renderer = Renderer::new(&instance, None)?;
        renderer.set_materials_root(Some(root.0.clone()));
        renderer.upload(&scene);
        ensure!(renderer.ground.as_ref().is_some_and(|ground| !ground.composites.is_empty()), "native ground composite was not uploaded");
        renderer.refresh_ground_textures(&scene.textures);
        renderer.set_enhanced_settings(settings);
        renderer.wait_for_pipeline_warmup();
        renderers.push(renderer);
    }
    let outputs: Vec<_> = renderers.iter().map(|renderer| {
        let texture = renderer.device.create_texture(&wgpu::TextureDescriptor { label: Some("material evidence"),
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
        let view = texture.create_view(&Default::default());
        (texture, view, Targets::new(renderer, W, H))
    }).collect();
    let camera = Camera::look_at(Vec3::new(3.0, 3.4, 5.5), Vec3::new(3.0, 1.0, -2.7));
    let mut images = Vec::new();
    let mut manifest = String::from("fixture=procedural_native_ground_and_mesh\nsize=640x400\nvisual_acceptance=unexercised\nprofiles=alloff,generated_normals,material_maps,material_pbr\nwarmup_per_profile=30\nsamples_per_profile=60\norder=ABBA\n");
    for (index, renderer) in renderers.iter_mut().enumerate() {
        let (texture, view, targets) = &outputs[index];
        renderer.render(view, targets, &camera);
        let pixels = texture_to_rgba(renderer, texture, W, H)?;
        png_file(&out.join(format!("{}.png", NAMES[index])), W, H, &pixels)?;
        images.push(pixels);
        let mut frames = Vec::new();
        for frame in 0..4 {
            let moving_camera = Camera::look_at(Vec3::new(2.4 + frame as f32 * 0.4, 3.4, 5.5), Vec3::new(3.0, 1.0, -2.7));
            renderer.render(view, targets, &moving_camera);
            frames.push(texture_to_rgba(renderer, texture, W, H)?);
        }
        png_file(&out.join(format!("{}_frame_strip.png", NAMES[index])), W * 4, H, &sheet(&frames, 4))?;
    }
    png_file(&out.join("contact_sheet.png"), W * 2, H * 2, &sheet(&images, 2))?;
    for index in 1..4 {
        let changed = images[0].as_chunks::<4>().0.iter().zip(images[index].as_chunks::<4>().0).filter(|(a, b)| a != b).count();
        writeln!(manifest, "{}_changed_pixels={changed}", NAMES[index])?;
        ensure!(changed > 100, "{} did not visibly change the procedural scene ({changed} pixels)", NAMES[index]);
    }
    let highlight_pixels = images[0].as_chunks::<4>().0.iter().zip(images[3].as_chunks::<4>().0)
        .filter(|(before, after)| (0..3).any(|channel| after[channel].saturating_sub(before[channel]) >= 32)).count();
    writeln!(manifest, "material_pbr_highlight_pixels_delta_ge32={highlight_pixels}")?;
    ensure!(highlight_pixels > 100, "PBR must produce a clearly visible GGX highlight, not a subtle normal difference ({highlight_pixels} brightened pixels)");
    let (_, view, targets) = &outputs[0];
    renderers[0].set_enhanced_settings(EnhancedSettings { generated_normals: true, material_maps: true, material_pbr: true, ..Default::default() });
    renderers[0].wait_for_pipeline_warmup();
    renderers[0].render(view, targets, &camera);
    renderers[0].set_enhanced_settings(EnhancedSettings::default());
    renderers[0].wait_for_pipeline_warmup();
    renderers[0].render(view, targets, &camera);
    let roundtrip = texture_to_rgba(&renderers[0], &outputs[0].0, W, H)?;
    png_file(&out.join("alloff_roundtrip.png"), W, H, &roundtrip)?;
    ensure!(roundtrip == images[0], "alloff roundtrip changed original pixels");
    writeln!(manifest, "alloff_roundtrip=byte_identical")?;
    let mut ground_scene = scene.clone();
    ground_scene.meshes.truncate(1);
    ground_scene.instances.truncate(1);
    let mut ground_images = Vec::new();
    for (index, renderer) in renderers.iter_mut().enumerate() {
        renderer.upload(&ground_scene);
        renderer.refresh_ground_textures(&ground_scene.textures);
        let (texture, view, targets) = &outputs[index];
        renderer.render(view, targets, &camera);
        let pixels = texture_to_rgba(renderer, texture, W, H)?;
        png_file(&out.join(format!("ground_{}.png", NAMES[index])), W, H, &pixels)?;
        ground_images.push(pixels);
        renderer.upload(&scene);
        renderer.refresh_ground_textures(&scene.textures);
    }
    png_file(&out.join("ground_contact_sheet.png"), W * 2, H * 2, &sheet(&ground_images, 2))?;
    for index in 1..4 {
        let changed = ground_images[0].as_chunks::<4>().0.iter().zip(ground_images[index].as_chunks::<4>().0).filter(|(a, b)| a != b).count();
        writeln!(manifest, "{}_ground_changed_pixels={changed}", NAMES[index])?;
        ensure!(changed > 100, "{} did not visibly change native ground ({changed} pixels)", NAMES[index]);
    }
    std::fs::write(out.join("manifest.txt"), format!("{manifest}timing_status=pending\n"))?;
    for renderer in &mut renderers { renderer.set_gpu_timing(true)?; }
    let mut csv = String::from("feature,pair,profile,cpu_render_encode_submit_ms,gpu_frame_ms\n");
    for (feature, name) in NAMES.iter().enumerate().skip(1) {
        let mut gpu = [Vec::new(), Vec::new()];
        for pair in 0..90 {
            for side in if pair % 2 == 0 { [0, 1] } else { [1, 0] } {
                let index = if side == 0 { 0 } else { feature };
                let renderer = &mut renderers[index];
                let (_, view, targets) = &outputs[index];
                let start = Instant::now();
                renderer.render(view, targets, &camera);
                let cpu = start.elapsed().as_secs_f64() * 1000.0;
                let elapsed = renderer.read_frame_gpu_ms()?.context("GPU timestamp timer disabled")?;
                if pair >= 30 {
                    gpu[side].push(elapsed);
                    writeln!(csv, "{name},{},{},{cpu:.9},{elapsed:.9}", pair - 30, NAMES[index])?;
                }
            }
        }
        let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
        writeln!(manifest, "{name}_gpu_mean_delta_ms={:.9}", mean(&gpu[1]) - mean(&gpu[0]))?;
        for (side, values) in gpu.iter_mut().enumerate() {
            values.sort_by(f64::total_cmp);
            for percentile in [50usize, 95, 99] {
                writeln!(manifest, "{name}_{}_gpu_p{percentile}_ms={:.9}", if side == 0 { "off" } else { "on" }, values[(values.len() * percentile).div_ceil(100) - 1])?;
            }
        }
    }
    std::fs::write(out.join("samples.csv"), csv)?;
    writeln!(manifest, "timing_status=complete")?;
    std::fs::write(out.join("manifest.txt"), &manifest)?;
    eprintln!("{manifest}");
    Ok(())
}
