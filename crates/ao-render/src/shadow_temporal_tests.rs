//! World-registered temporal masks, not screen-pixel differences during a camera move.
use super::*;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const FRAMES: usize = 90;
const SUN: Vec3 = Vec3::new(-0.8, 0.5, -0.3);

fn quad(points: [[f32; 3]; 4], normal: [f32; 3]) -> ao_scene::Mesh {
    ao_scene::Mesh {
        vertices: points.into_iter().map(|pos| ao_scene::Vertex {
            pos, normal, uv: [0.0; 2], color: [1.0; 4],
        }).collect(),
        submeshes: vec![ao_scene::Submesh {
            two_sided: true,
            ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        }],
    }
}

fn camera(frame: usize, moving: bool) -> Camera {
    let t = if moving { frame as f32 / (FRAMES - 1) as f32 } else { 0.0 };
    let angle = t * 0.06;
    Camera::look_at(Vec3::new(10.0 * angle.sin() + 0.3 * t, 5.0, 10.0 * angle.cos()),
        Vec3::new(0.3 * t, 0.0, 1.0))
}

fn actor(frame: usize, bind: &[ao_scene::Vertex]) -> ao_scene::ActorFrame {
    let t = frame as f32 * 0.07;
    let mut skin = bind.to_vec();
    for vertex in &mut skin {
        // Exercise the actual CPU-deformed body upload, not just its rigid transform.
        vertex.pos[0] += 0.18 * t.sin() * vertex.pos[1] / 1.8;
    }
    ao_scene::ActorFrame {
        id: 1, model: 1,
        transform: Mat4::from_translation(Vec3::new(-4.0 + 0.25 * t.sin(), 0.0, 1.0)).to_cols_array_2d(),
        skin: Some(std::sync::Arc::new(skin)),
        ..Default::default()
    }
}

// Positive inside the wall's ground shadow. This analytic margin excludes the
// *physical* penumbra, rather than erasing pixels that happened to flicker.
fn wall_margin(p: Vec3, sun: Vec3) -> f32 {
    let wall_x = p.x - p.z * sun.x / sun.z;
    let wall_y = -p.z * sun.y / sun.z;
    let side = (2.0 - wall_x.abs()) / (1.0 + (sun.x / sun.z).powi(2)).sqrt();
    side.min(p.z).min((4.0 - wall_y) * (-sun.z / sun.y))
}

fn ground_visible(eye: Vec3, p: Vec3, has_actor: bool) -> bool {
    // Reject receiver rays crossing a wall (expanded to protect raster boundaries).
    for (z, left, right, top) in [(0.0, -2.0, 2.0, 4.0), (1.0, -4.8, -3.2, 2.1)] {
        if z == 1.0 && !has_actor { continue; }
        let t = (z - eye.z) / (p.z - eye.z);
        if t > 0.0 && t < 1.0 {
            let hit = eye.lerp(p, t);
            if hit.x > left - 0.15 && hit.x < right + 0.15 && hit.y < top + 0.15 {
                return false;
            }
        }
    }
    true
}

fn pixel_energy(image: &[u8], x: f32, y: f32) -> f32 {
    let (ix, iy) = (x.floor() as usize, y.floor() as usize);
    let (fx, fy) = (x.fract(), y.fract());
    let energy = |x, y| {
        let offset = (y * WIDTH as usize + x) * 4;
        image[offset..offset + 3].iter().map(|&v| f32::from(v)).sum::<f32>() / (3.0 * 255.0)
    };
    (energy(ix, iy) * (1.0 - fx) + energy(ix + 1, iy) * fx) * (1.0 - fy)
        + (energy(ix, iy + 1) * (1.0 - fx) + energy(ix + 1, iy + 1) * fx) * fy
}

fn write_png(path: &std::path::Path, width: u32, height: u32, pixels: &[u8]) {
    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(pixels).unwrap();
}

#[test]
fn temporal_shadow_world_receivers_remain_stable() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("headless temporal shadow GPU");
    renderer.set_msaa_samples(1);
    let mut scene = Scene {
        meshes: vec![
            quad([[-10.0, 0.0, -10.0], [10.0, 0.0, -10.0], [10.0, 0.0, 10.0], [-10.0, 0.0, 10.0]], [0.0, 1.0, 0.0]),
            quad([[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 4.0, 0.0], [-2.0, 4.0, 0.0]], [0.0, 0.0, 1.0]),
        ],
        fog_model: Some(ao_scene::FogModel {
            far: 132.0,
            volumes: vec![ao_scene::FogVolume {
                pos: [5.0, 5.0, 10.0], color: [0.3; 3], density: 0.65,
                radius: 6.0, room: None,
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    scene.instances = (0..2).map(|mesh| ao_scene::Instance { mesh, transform: ao_scene::IDENTITY }).collect();
    renderer.upload(&scene);
    renderer.env.sun_color = [0.6; 3];
    renderer.env.ambient = [0.2; 3];
    renderer.env.fog_start = 100.0;
    renderer.env.fog_end = 120.0;
    let model = Scene {
        meshes: vec![quad([[-0.4, 0.0, 0.0], [0.4, 0.0, 0.0], [0.4, 1.8, 0.0], [-0.4, 1.8, 0.0]], [0.0, 0.0, 1.0])],
        ..Default::default()
    };
    renderer.add_actor_model(1, &model);
    renderer.set_enhanced_settings(EnhancedSettings { sun_shadows: true, ..Default::default() });
    let targets = Targets::new(&renderer, WIDTH, HEIGHT);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("temporal shadow evidence"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let capture = |renderer: &mut Renderer, camera: &Camera| {
        renderer.render(&view, &targets, camera);
        texture_to_rgba(renderer, &output, WIDTH, HEIGHT).unwrap()
    };
    let evidence = std::env::var_os("AOMAC_GFX_SHADOW_TEMPORAL_OUT")
        .or_else(|| std::env::var_os("AOMAC_GFX_SHADOW_FAIRNESS_OUT"))
        .map(std::path::PathBuf::from);
    if let Some(directory) = &evidence { std::fs::create_dir_all(directory).unwrap(); }
    let grid: Vec<_> = (0..29).flat_map(|z| (0..71).map(move |x|
        Vec3::new(-6.0 + x as f32 * 0.2, 0.0, 0.4 + z as f32 * 0.2))).collect();
    let mut failures = Vec::new();
    for case in ["orbit_walk", "sun_time", "animated_actor"] {
        let has_actor = case == "animated_actor";
        // Cross a real fog gradient: receiver clipping must not resize texels.
        renderer.fog = if case == "orbit_walk" { scene.fog_model.clone() } else { None };
        renderer.set_fog_density_scale(1.0);
        let mut previous = vec![None::<(f32, bool)>; grid.len()];
        let mut first = Vec::new();
        let mut last = Vec::new();
        let mut interior_sum = 0.0_f32;
        let mut interior_max = 0.0_f32;
        let mut interior_pairs = 0;
        let mut changed = 0;
        let mut edge_sum = 0.0_f32;
        let mut edge_max = 0.0_f32;
        let mut edge_pairs = 0;
        let mut min_shadowed = usize::MAX;
        let mut min_lit = usize::MAX;
        let mut actor_motion = 0.0_f32;
        let mut sheet = vec![0; (WIDTH * 5 * HEIGHT * 2 * 4) as usize];
        for frame in 0..FRAMES {
            let camera = camera(frame, case == "orbit_walk");
            let sun = if case == "sun_time" {
                Mat4::from_rotation_y(frame as f32 * 0.000025).transform_vector3(SUN)
            } else { SUN };
            renderer.time = frame as f32 / 60.0;
            renderer.env.sun_dir = sun.to_array();
            renderer.set_actors(if has_actor { vec![actor(frame, &model.meshes[0].vertices)] } else { vec![] });
            renderer.shadow_sun = Some(sun.to_array());
            let shadowed = capture(&mut renderer, &camera);
            if has_actor {
                assert!(renderer.sun_shadows.as_ref().unwrap().actor_ids.contains(&1), "animated actor never entered the shadow pass");
            }
            renderer.shadow_sun = None;
            let unshadowed = capture(&mut renderer, &camera);
            let aspect = WIDTH as f32 / HEIGHT as f32;
            let vp = Mat4::perspective_rh(renderer.lens.vertical_fov(aspect), aspect,
                renderer.lens.near, renderer.lens.far.unwrap_or(132.0))
                * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up());
            let inverse = vp.inverse();
            let mut dark = 0;
            let mut lit = 0;
            for (index, &point) in grid.iter().enumerate() {
                let clip = vp * point.extend(1.0);
                if clip.w <= 0.0 { previous[index] = None; continue; }
                let ndc = clip.truncate() / clip.w;
                let x = (ndc.x * 0.5 + 0.5) * WIDTH as f32 - 0.5;
                let y = (0.5 - ndc.y * 0.5) * HEIGHT as f32 - 0.5;
                if x < 3.0 || y < 3.0 || x >= (WIDTH - 4) as f32 || y >= (HEIGHT - 4) as f32 {
                    previous[index] = None; continue;
                }
                // Protect the whole reconstruction footprint, not only its centre:
                // a projected ground sample next to visible geometry is not ground.
                let footprint_clear = [-2.0, 2.0].into_iter().all(|dx| [-2.0, 2.0].into_iter().all(|dy| {
                    let ray = inverse.project_point3(Vec3::new(
                        ((x + dx + 0.5) / WIDTH as f32) * 2.0 - 1.0,
                        1.0 - ((y + dy + 0.5) / HEIGHT as f32) * 2.0, 0.5)) - camera.pos;
                    let receiver = camera.pos + ray * (-camera.pos.y / ray.y);
                    receiver.x.abs() < 9.0 && receiver.z.abs() < 9.0
                        && ground_visible(camera.pos, receiver, has_actor)
                }));
                if !footprint_clear { previous[index] = None; continue; }
                let difference = (pixel_energy(&unshadowed, x, y) - pixel_energy(&shadowed, x, y)).max(0.0);
                // Native fog mixes in gamma space. Divide out its transmittance
                // so legitimate fog fading is not reported as shadow flicker.
                let transmission = renderer.fog.as_ref().map_or(1.0, |fog| {
                    let (_, end) = fog.at(camera.pos.to_array());
                    let depth = (point - camera.pos).dot(camera.forward());
                    ((end - depth) / (end - renderer.lens.near)).clamp(0.05, 1.0)
                });
                let mask = difference / transmission;
                let margin = wall_margin(point, sun);
                // The union of all actor silhouettes and shadows is deliberately
                // excluded from STATIC interior assertions, but measured separately.
                let actor_region = has_actor && point.x > -5.6 && point.x < 0.0 && point.z < 3.4;
                let interior = margin.abs() > 0.3 && !actor_region;
                if interior {
                    if margin > 0.3 && mask > 0.08 { dark += 1; }
                    if margin < -0.3 && mask < 0.01 { lit += 1; }
                }
                if let Some((old, was_interior)) = previous[index] {
                    let delta = (mask - old).abs();
                    if actor_region { actor_motion += delta; }
                    else if interior && was_interior {
                        interior_sum += delta;
                        interior_max = interior_max.max(delta);
                        interior_pairs += 1;
                        if delta > 2.0 / 255.0 { changed += 1; }
                    } else {
                        edge_sum += delta;
                        edge_max = edge_max.max(delta);
                        edge_pairs += 1;
                    }
                }
                previous[index] = Some((mask, interior));
            }
            min_shadowed = min_shadowed.min(dark);
            min_lit = min_lit.min(lit);
            if frame == 0 { first = shadowed.clone(); }
            if frame == FRAMES - 1 { last = shadowed.clone(); }
            if frame % 10 == 0 || frame == FRAMES - 1 {
                let tile = if frame == FRAMES - 1 { 9 } else { frame / 10 };
                for row in 0..HEIGHT as usize {
                    let source = row * WIDTH as usize * 4;
                    let destination = (((tile / 5) * HEIGHT as usize + row) * (WIDTH * 5) as usize
                        + (tile % 5) * WIDTH as usize) * 4;
                    sheet[destination..destination + WIDTH as usize * 4]
                        .copy_from_slice(&shadowed[source..source + WIDTH as usize * 4]);
                }
                if let Some(directory) = &evidence {
                    write_png(&directory.join(format!("temporal_{case}_{frame:03}.png")), WIDTH, HEIGHT, &shadowed);
                    let mask: Vec<_> = shadowed.chunks_exact(4).zip(unshadowed.chunks_exact(4))
                        .flat_map(|(a, b)| {
                            let d = b[0].saturating_sub(a[0]).saturating_mul(3);
                            [d, d, d, 255]
                        }).collect();
                    write_png(&directory.join(format!("temporal_{case}_{frame:03}_mask.png")), WIDTH, HEIGHT, &mask);
                }
            }
        }
        // Return to the exact first state after 90 updates: expose initialization,
        // stale pose uploads, and history-dependent shadow projection changes.
        renderer.time = 0.0;
        renderer.env.sun_dir = SUN.to_array();
        renderer.shadow_sun = Some(SUN.to_array());
        renderer.set_actors(if has_actor { vec![actor(0, &model.meshes[0].vertices)] } else { vec![] });
        let repeated = capture(&mut renderer, &camera(0, false));
        let repeated_pixels = first.chunks_exact(4).zip(repeated.chunks_exact(4)).filter(|(a, b)| a != b).count();
        let mean = interior_sum / interior_pairs.max(1) as f32;
        let edge_mean = edge_sum / edge_pairs.max(1) as f32;
        let metrics = format!("case={case} frames={FRAMES} interior_pairs={interior_pairs} interior_mean={mean:.8} interior_max={interior_max:.8} changed_over_2_codes={changed} edge_pairs={edge_pairs} edge_mean={edge_mean:.8} edge_max={edge_max:.8} min_dark={min_shadowed} min_lit={min_lit} repeated_changed_pixels={repeated_pixels} actor_motion={actor_motion:.8}\n");
        eprint!("{metrics}");
        if let Some(directory) = &evidence {
            write_png(&directory.join(format!("temporal_{case}_contact.png")), WIDTH * 5, HEIGHT * 2, &sheet);
            write_png(&directory.join(format!("temporal_{case}_repeated.png")), WIDTH, HEIGHT, &repeated);
            std::fs::write(directory.join(format!("temporal_{case}_metrics.txt")), &metrics).unwrap();
        }
        if interior_pairs < 10_000 || min_shadowed < 10 || min_lit < 100 {
            failures.push(format!("{case}: insufficient visible lit/shadowed ground controls: {metrics}"));
        }
        if mean > 0.5 / 255.0 || interior_max > 3.0 / 255.0 || changed * 1000 > interior_pairs {
            failures.push(format!("{case}: static interior shadow flicker: {metrics}"));
        }
        if edge_pairs < 100 || edge_mean > 0.004 || edge_max > 0.06 {
            failures.push(format!("{case}: missing or unstable shadow edges: {metrics}"));
        }
        if repeated_pixels != 0 {
            failures.push(format!("{case}: first/later fixed frame differs: {metrics}"));
        }
        if has_actor && (first == last || actor_motion <= 0.05) {
            failures.push(format!("{case}: animated actor control did not move a ground shadow: {metrics}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
