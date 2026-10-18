//! Asset-free, current-frame diffuse-bounce regressions through the real renderer.
use crate::*;
use ao_scene::{ActorFrame, FogModel, Instance, Mesh, RoomLocator, RoomVolume, Submesh, Texture, TextureKey, Vertex};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;
const GREEN: TextureKey = TextureKey { rdb_type: 0, id: 0x6769_0001 };

fn quad(points: [[f32; 3]; 4], normal: [f32; 3], texture: Option<TextureKey>) -> Mesh {
    Mesh {
        vertices: points.into_iter().enumerate().map(|(i, pos)| Vertex {
            pos, normal, color: [1.0; 4], uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]][i],
        }).collect(),
        submeshes: vec![Submesh { two_sided: true, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], texture) }],
    }
}

fn corner() -> Scene {
    let floor = quad([[-6.0, 0.0, 2.0], [6.0, 0.0, 2.0], [6.0, 0.0, -6.0], [-6.0, 0.0, -6.0]], [0.0, 1.0, 0.0], Some(GREEN));
    let mut wall = quad([[-6.0, 0.0, -6.0], [6.0, 0.0, -6.0], [6.0, 6.0, -6.0], [-6.0, 6.0, -6.0]], [0.0, 0.0, 1.0], None);
    wall.submeshes[0].base_color = [0.45, 0.45, 0.45, 1.0];
    Scene {
        textures: [(GREEN, Texture { width: 2, height: 2, rgba: [12, 220, 12, 255].repeat(4), source: ao_scene::TextureSource::Generated })].into(),
        meshes: vec![floor, wall],
        instances: (0..2).map(|mesh| Instance { mesh, transform: ao_scene::IDENTITY }).collect(),
        environment: Some(ao_scene::Environment {
            // Native lighting adds gamma-space terms before clamping: keep
            // gamma(ambient) + gamma(sun) < 1 so emission has a visible control.
            sky_color: [0.0; 3], fog_color: [0.03; 3], ambient: [0.025; 3],
            sun_color: [0.15; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0,
            fog_start: 80.0, fog_end: 120.0,
        }),
        lens: Some(ao_scene::Lens { far: Some(120.0), ..Default::default() }),
        // No terrain lattice is supplied: these known indoor surfaces share a
        // closed room, not an invented open-sky admission signal.
        room_volumes: vec![RoomVolume { min: [-20.0; 3], max: [20.0; 3], room: 1 }],
        fog_model: Some(FogModel {
            far: 120.0, near: 0.1,
            rooms: Some(RoomLocator(std::sync::Arc::new(|_| Some(0)))),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn settings(strength: f32) -> EnhancedSettings {
    EnhancedSettings { indirect_light: true, indirect_strength: strength, ..Default::default() }
}

fn capture(r: &mut Renderer, output: &wgpu::Texture, camera: &Camera, settings: EnhancedSettings) -> anyhow::Result<Vec<u8>> {
    r.set_enhanced_settings(settings);
    r.wait_for_pipeline_warmup();
    assert_eq!(r.msaa_samples(), 4, "indirect regression must exercise real 4x MSAA, not a single-sample fallback");
    let targets = Targets::new(r, WIDTH, HEIGHT);
    r.render(&output.create_view(&Default::default()), &targets, camera);
    texture_to_rgba(r, output, WIDTH, HEIGHT)
}

fn receiver_pixels(native: &[u8]) -> Vec<usize> {
    // The untextured neutral wall is grey; neither the green floor nor black
    // sky can enter this mask. Stay away from the wall's outer silhouettes.
    let pixels: Vec<_> = (48..128).flat_map(|y| (64..192).map(move |x| ((y * WIDTH + x) * 4) as usize))
        .filter(|&i| native[i] > 0 && native[i] == native[i + 1] && native[i] == native[i + 2]).collect();
    assert!(pixels.len() > 500, "fixture must expose a substantial neutral wall");
    pixels
}

fn assert_receiver_equal(a: &[u8], b: &[u8], wall: &[usize], message: &str) {
    for &i in wall { assert_eq!(&a[i..i + 4], &b[i..i + 4], "{message}, pixel {}", i / 4); }
}

fn assert_green_bounce(off: &[u8], on: &[u8], wall: &[usize], message: &str) {
    let mut delta = [0i64; 3];
    for &i in wall {
        for channel in 0..3 { delta[channel] += i64::from(on[i + channel]) - i64::from(off[i + channel]); }
    }
    assert!(delta[1] > 0 && delta[1] > delta[0] && delta[1] > delta[2], "{message}: receiver RGB deltas {delta:?}");
}
#[test]
#[ignore = "real 4x MSAA GPU regression; run alone under the GPU timing lock"]
fn indirect_light_current_admission_rooms_fog_and_exact_roundtrip() -> anyhow::Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r = Renderer::new(&instance, None)?;
    r.set_msaa_samples(4);
    r.time = 0.0;
    let camera = Camera::look_at(Vec3::new(0.0, 1.5, 3.0), Vec3::new(0.0, 1.5, -6.0));
    let scene = corner();
    r.upload(&scene);
    let output = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: Some("indirect light integration regression"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let native = capture(&mut r, &output, &camera, EnhancedSettings::default())?;
    let zero = capture(&mut r, &output, &camera, settings(0.0))?;
    assert_eq!(native, zero, "enabled strength zero must preserve every native output byte");
    let wall = receiver_pixels(&native);
    let bounced = capture(&mut r, &output, &camera, settings(1.0))?;
    assert_green_bounce(&zero, &bounced, &wall, "textured lit green floor must tint the grey wall");
    assert_eq!(r.enhanced_frame.as_ref().expect("compact indirect MRT frame").target_count(), 3);
    assert_eq!(native, capture(&mut r, &output, &camera, EnhancedSettings::default())?, "all-off cutover must restore the exact native image");
    assert!(r.enhanced_frame.is_none(), "all-off must retire enhanced attachments");
    assert_eq!(bounced, capture(&mut r, &output, &camera, settings(1.0))?, "re-enabling must use only this frame, not retained light");

    // Identical geometry and light, with only the floor/wall room boundary
    // changed. Coplanar points cannot provide a positive horizon increment.
    // The shared depth resolve takes the nearest MSAA subsample, whereas
    // world reconstruction uses the pixel-centre ray. On this horizontal
    // floor that lifts reconstructed Y by about 0.02 m at the junction.
    // Enclose that footprint in the floor room; the neutral-wall ROI starts
    // above this 0.05 m boundary (junction coverage is mixed green, not grey).
    let mut separated = scene.clone();
    separated.room_volumes = vec![
        RoomVolume { min: [-20.0, -1.0, -20.0], max: [20.0, 0.05, 20.0], room: 1 },
        RoomVolume { min: [-20.0, 0.05, -20.0], max: [20.0, 20.0, 20.0], room: 2 },
    ];
    r.upload(&separated);
    let separated_zero = capture(&mut r, &output, &camera, settings(0.0))?;
    assert_eq!(zero, separated_zero, "room metadata alone must not change native rendering");
    let separated_on = capture(&mut r, &output, &camera, settings(1.0))?;
    assert_receiver_equal(&separated_zero, &separated_on, &wall, "different room IDs must block floor-to-wall transfer");

    // Emission/unlit colour is visibly present but is not diffuse radiance.
    for emissive in [true, false] {
        let mut excluded = scene.clone();
        let material = &mut excluded.meshes[0].submeshes[0];
        if emissive { material.emissive = [1.0; 3]; } else { material.lighting_enabled = false; }
        r.upload(&excluded);
        let excluded_zero = capture(&mut r, &output, &camera, settings(0.0))?;
        let excluded_on = capture(&mut r, &output, &camera, settings(1.0))?;
        assert!(zero != excluded_zero, "excluded source must still be visibly rendered (emissive={emissive})");
        assert_receiver_equal(&excluded_zero, &excluded_on, &wall, "emissive/unlit floor cannot bounce onto wall");
        // Exercise the generated-map/PBR/GGX shader combination as well as
        // standalone SSGI: Enhanced additions must not re-admit this source.
        let combined = EnhancedSettings {
            material_specular: true, material_maps: true, material_pbr: true,
            ..settings(0.0)
        };
        let combined_zero = capture(&mut r, &output, &camera, combined)?;
        let combined_on = capture(&mut r, &output, &camera, EnhancedSettings { indirect_strength: 1.0, ..combined })?;
        assert_receiver_equal(&combined_zero, &combined_on, &wall, "combined GGX/maps/PBR must not make emissive/unlit floor bounce");
    }

    // Alpha-one source-over still uses the blended pipeline. It must consume
    // the covered opaque receiver's budget even without writing primary depth.
    let mut covered = scene.clone();
    let mut overlay = quad(
        [[-6.0, 0.0, -5.9], [6.0, 0.0, -5.9], [6.0, 6.0, -5.9], [-6.0, 6.0, -5.9]],
        [0.0, 0.0, 1.0], None,
    );
    overlay.submeshes[0].blend = ao_scene::Blend::AlphaBlend;
    overlay.submeshes[0].depth_write = Some(false);
    overlay.submeshes[0].lighting_enabled = false;
    overlay.submeshes[0].base_color = [0.0, 0.0, 0.0, 1.0];
    covered.instances.push(Instance { mesh: covered.meshes.len(), transform: ao_scene::IDENTITY });
    covered.meshes.push(overlay);
    r.upload(&covered);
    let covered_zero = capture(&mut r, &output, &camera, settings(0.0))?;
    assert!(wall.iter().all(|&i| covered_zero[i] < zero[i]), "black source-over control must cover the neutral wall");
    let covered_on = capture(&mut r, &output, &camera, settings(1.0))?;
    assert_receiver_equal(&covered_zero, &covered_on, &wall, "opaque-alpha blended overlay must block the underlying receiver's bounce");

    // A visible actor is a positive source control. Revoking it, hiding it
    // behind the wall, or declining its native render-list admission must not
    // leave either a current signal or history in the very next image.
    let mut neutral = scene.clone();
    neutral.textures.get_mut(&GREEN).unwrap().rgba = [40, 40, 40, 255].repeat(4);
    r.upload(&neutral);
    let mut model = Scene { meshes: vec![scene.meshes[0].clone()], textures: scene.textures.clone(), ..Default::default() };
    for vertex in &mut model.meshes[0].vertices { vertex.pos[1] = 0.025; }
    r.add_actor_model(91, &model);
    let absent = capture(&mut r, &output, &camera, settings(1.0))?;
    let actor = ActorFrame { id: 91, model: 91, character: true, ..Default::default() };
    r.set_actors(vec![actor.clone()]);
    let visible_zero = capture(&mut r, &output, &camera, settings(0.0))?;
    let visible_on = capture(&mut r, &output, &camera, settings(1.0))?;
    assert_green_bounce(&visible_zero, &visible_on, &wall, "admitted lit actor must exercise the same diffuse source path");
    assert!(visible_on != absent, "visible actor positive control must alter the image");
    r.set_actors(Vec::new());
    assert_eq!(absent, capture(&mut r, &output, &camera, settings(1.0))?, "actor disappearance must match the no-actor current frame immediately");
    for (name, alpha, priority, translation) in [
        ("alpha-zero", 0.0, None, Vec3::ZERO),
        ("non-admitted", 1.0, Some(-1), Vec3::ZERO),
        ("occluded", 1.0, None, Vec3::new(0.0, 0.0, -12.0)),
        ("offscreen", 1.0, None, Vec3::new(100.0, 0.0, 0.0)),
    ] {
        r.set_actors(vec![ActorFrame { alpha, priority, transform: Mat4::from_translation(translation).to_cols_array_2d(), ..actor.clone() }]);
        assert_eq!(absent, capture(&mut r, &output, &camera, settings(1.0))?, "{name} actor must leave no direct or indirect signal");
    }
    r.set_actors(Vec::new());
    r.remove_actor_model(91);

    let mut fogged = scene.clone();
    fogged.fog_model.as_mut().unwrap().base_density = 1.0;
    let far_camera = Camera::look_at(Vec3::new(0.0, 1.5, 10.0), Vec3::new(0.0, 1.5, -6.0));
    r.upload(&fogged);
    let fog_zero = capture(&mut r, &output, &far_camera, settings(0.0))?;
    let fog_on = capture(&mut r, &output, &far_camera, settings(1.0))?;
    assert!(fog_zero != zero, "fog control must actually change the rendered scene");
    assert_eq!(fog_zero, fog_on, "full native receiver extinction must leave no indirect signal");
    assert_eq!(fog_zero, capture(&mut r, &output, &far_camera, EnhancedSettings::default())?, "fogged all-off roundtrip must also be exact");
    Ok(())
}
