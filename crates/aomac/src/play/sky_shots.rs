//! Offline ICC sky evidence, without login or a window:
//! `AOMAC_SHOT_DIR=/tmp/sky-before cargo test --release -p aomac icc_sky_screenshots -- --nocapture`
//! Repeat on the changed revision with `/tmp/sky-after`; matching filenames are comparison pairs.
//! No pixel baseline is asserted: a capture is evidence, not an invented aesthetic oracle.
use super::camera;
use ao_gui::{DrawList, Gui, InputEvent};
use ao_render::{Camera, Frontend, Host, Offscreen, Vec3};
use ao_scene::{Lens, Scene};
use std::path::PathBuf;

struct Shot {
    gui: Gui,
    scene: Option<Scene>,
    camera: Camera,
    lens: Lens,
}

impl Frontend for Shot {
    fn gui(&self) -> &Gui { &self.gui }
    fn input(&mut self, _: InputEvent, _: &mut Host) {}
    fn frame(&mut self, _: f32, _: (u32, u32), host: &mut Host) -> DrawList {
        if let Some(scene) = self.scene.take() { host.set_scene(scene); }
        host.camera = self.camera;
        host.lens = Some(self.lens);
        DrawList::default()
    }
}

#[test]
fn icc_sky_screenshots() -> anyhow::Result<()> {
    let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return Ok(()) };
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() {
        eprintln!("skipping ICC sky captures: no installed client at {}", dir.display());
        return Ok(());
    }
    std::fs::create_dir_all(&out)?;
    let store = ao_rdb::RecordStore::open(&dir)?;
    let mut shot = Shot {
        gui: Gui::new(&dir, None)?,
        scene: None,
        camera: Camera::look_at(Vec3::ZERO, -Vec3::Z),
        lens: Lens::default(),
    };
    let mut off = Offscreen::new(&shot, (1280, 800))?;
    for (time_name, time) in [("day2648.69", 2648.69), ("night0", 0.0)] {
        let scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, time)?;
        assert_eq!(scene.day_time, time);
        assert!(!scene.sky.is_empty(), "ICC must supply the real sky layers");
        for instance in &scene.sky {
            assert!(instance.transform.iter().flatten().all(|v| v.is_finite()));
            let mesh = &scene.meshes[instance.mesh];
            assert!(!mesh.vertices.is_empty());
            assert!(mesh.vertices.iter().all(|v| v.pos.iter().all(|v| v.is_finite())));
        }
        let base = scene.lens.unwrap_or_default();
        // Offshore ICC sea camera: unobstructed sky, also exercised by liquid_real.
        // Fixed scene-space coordinates (AO world Z is negated), not a movable spawn heuristic.
        let eye = Vec3::new(1010.4, 24.0, -663.1);
        let heading = 2.4288;
        shot.scene = Some(scene);
        for size in [(1280, 800), (5120, 1440)] {
            off.resize(size);
            let aspect = size.0 as f32 / size.1 as f32;
            for wide in [false, true] {
                for scenery in [1.0, 2.0] {
                    // Identical preference mapping to Player::frame; do not substitute viewer defaults.
                    shot.lens = camera::lens(base, wide);
                    shot.lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE) * scenery);
                    assert_eq!(shot.lens.near, camera::NEAR);
                    assert_eq!(shot.lens.far, Some(800.0 * scenery));
                    let vfov = shot.lens.vertical_fov(aspect);
                    let hfov = 2.0 * ((vfov * 0.5).tan() * aspect).atan();
                    assert!((hfov - camera::horizontal_fov(wide, aspect)).abs() < 1e-5);
                    for (heading_name, yaw) in [("forward", heading), ("right", heading + std::f32::consts::FRAC_PI_2)] {
                        for (view, pitch) in [("up", 60.0_f32.to_radians()), ("horizon", 0.0), ("down", -60.0_f32.to_radians())] {
                            shot.camera = Camera { pos: eye, yaw, pitch, roll: 0.0 };
                            let projection = glam::Mat4::perspective_rh(vfov, aspect, shot.lens.near, shot.lens.far.unwrap());
                            let view_matrix = glam::Mat4::look_to_rh(eye, shot.camera.forward(), shot.camera.up());
                            assert!((projection * view_matrix).is_finite());
                            // Freeze renderer time, clouds, movers and sky animations across the matrix.
                            let list = off.frame(&mut shot, 0.0);
                            let name = format!("icc4582-{time_name}-{}x{}-wide{}-scenery{scenery}-{heading_name}-{view}.png", size.0, size.1, u8::from(wide));
                            eprintln!("{name}: eye={eye:?}, yaw={yaw}, pitch={pitch}, lens={:?}", shot.lens);
                            off.png(&shot, &list, &out.join(name))?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Isolates each authored sky mesh without changing instance indices or animation metadata.
/// `AOMAC_SHOT_DIR=/tmp/sky-layers cargo test --release -p aomac icc_sky_layer_screenshots -- --nocapture`
#[test]
fn icc_sky_layer_screenshots() -> anyhow::Result<()> {
    let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return Ok(()) };
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() {
        eprintln!("skipping ICC sky layers: no installed client at {}", dir.display());
        return Ok(());
    }
    std::fs::create_dir_all(&out)?;
    let store = ao_rdb::RecordStore::open(&dir)?;
    let scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, 2648.69)?;
    assert!(!scene.sky.is_empty(), "ICC must supply the real sky layers");
    let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
    lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
    let mut shot = Shot {
        gui: Gui::new(&dir, None)?,
        scene: None,
        camera: Camera { pos: Vec3::new(1010.4, 24.0, -663.1), yaw: 2.4288, pitch: 0.0, roll: 0.0 },
        lens,
    };
    let mut off = Offscreen::new(&shot, (1280, 800))?;
    for (ordinal, instance) in scene.sky.iter().enumerate() {
        let mesh = &scene.meshes[instance.mesh];
        let (lo, hi) = mesh.vertices.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), vertex| {
                let p = Vec3::from(vertex.pos);
                (lo.min(p), hi.max(p))
            },
        );
        eprintln!("layer{ordinal:02}: mesh={} vertices={} bounds={lo:?}..{hi:?} transform={:?}", instance.mesh, mesh.vertices.len(), instance.transform);
        for (material, submesh) in mesh.submeshes.iter().enumerate() {
            eprintln!("  material{material}: texture={:?} blend={:?} base={:?} emissive={:?} sky_fog={} indices={}",
                submesh.texture, submesh.blend, submesh.base_color, submesh.emissive, submesh.sky_fog, submesh.indices.len());
        }
        let mut isolated = scene.clone();
        for other in &scene.sky {
            if other.mesh != instance.mesh {
                isolated.meshes[other.mesh].submeshes.clear();
            }
        }
        shot.scene = Some(isolated);
        let list = off.frame(&mut shot, 0.0);
        off.png(&shot, &list, &out.join(format!("layer{ordinal:02}.png")))?;
    }
    Ok(())
}

/// One landmark-inferred view of the user's current-client screenshot, not retail visual proof.
/// `AOMAC_SHOT_DIR=/tmp/icc-retail cargo test --release -p aomac icc_retail_sky_screenshots -- --nocapture`
/// Optional scene-space calibration: AOMAC_SKY_EYE=x,y,z; AOMAC_SKY_YAW/PITCH (radians);
/// AOMAC_SKY_TIME (game seconds), AOMAC_SKY_WIDE=0|1, AOMAC_SKY_SIZE=widthxheight, AOMAC_SKY_FAR (metres);
/// AOMAC_SKY_SHOTS=all|combined|world|dry|sky|layers|layerNN|omitNN|throughNN.
/// `omitNN` removes one layer from combined; `throughNN` draws sky layers 0..=NN.
/// AOMAC_SKY_PIXELS=x,y;x,y probes SingleCloud layer17 in combined/through17 captures.
/// AOMAC_SKY_PROBE_FAR overrides only the diagnostic projection, retaining rendered GPU geometry.
#[test]
fn icc_retail_sky_screenshots() -> anyhow::Result<()> {
    use std::fmt::Write;
    let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return Ok(()) };
    let dir = ao_gui::client_dir();
    anyhow::ensure!(dir.join("cd_image/rdb.db").exists(), "no installed client at {}", dir.display());
    let scalar = |name: &str, default: f32| -> anyhow::Result<f32> {
        let value = std::env::var(name).map_or(Ok(default), |s| s.parse::<f32>())?;
        anyhow::ensure!(value.is_finite(), "{name} must be finite");
        Ok(value)
    };
    // [INFERENCE] Calibrated against the user's current-client screenshot landmarks.
    // Baseline camera-view-i: wreck mast ~(-3,+3), shuttle antenna ~(-5,0),
    // overhang right edge ~(+7,0) pixels relative to the 2090x1083 screenshot.
    // Foreground shoreline/grass still differs; this is not an exact recovered camera.
    let mut eye = Vec3::new(933.5, 26.0, -748.0);
    if let Ok(value) = std::env::var("AOMAC_SKY_EYE") {
        let xyz = value.split(',').map(str::parse::<f32>).collect::<Result<Vec<_>, _>>()?;
        anyhow::ensure!(xyz.len() == 3 && xyz.iter().all(|v| v.is_finite()), "AOMAC_SKY_EYE must be finite x,y,z");
        eye = Vec3::new(xyz[0], xyz[1], xyz[2]);
    }
    let yaw = scalar("AOMAC_SKY_YAW", 3.765)?;
    let pitch = scalar("AOMAC_SKY_PITCH", 0.046)?;
    let time = scalar("AOMAC_SKY_TIME", 2648.69)?;
    anyhow::ensure!((0.0..6480.0).contains(&time), "AOMAC_SKY_TIME must be in 0..6480");
    let wide = scalar("AOMAC_SKY_WIDE", 0.0)?;
    anyhow::ensure!(wide == 0.0 || wide == 1.0, "AOMAC_SKY_WIDE must be 0 or 1");
    let size = std::env::var("AOMAC_SKY_SIZE").unwrap_or_else(|_| "2090x1083".into());
    let (width, height) = size.split_once('x')
        .ok_or_else(|| anyhow::anyhow!("AOMAC_SKY_SIZE must be widthxheight"))?;
    let size = (width.parse::<u32>()?, height.parse::<u32>()?);
    anyhow::ensure!(size.0 > 0 && size.1 > 0, "AOMAC_SKY_SIZE dimensions must be positive");
    let shots = std::env::var("AOMAC_SKY_SHOTS").unwrap_or_else(|_| "all".into());
    let mut pixels = Vec::new();
    if let Ok(value) = std::env::var("AOMAC_SKY_PIXELS") {
        anyhow::ensure!(matches!(shots.as_str(), "combined" | "through17"),
            "AOMAC_SKY_PIXELS requires AOMAC_SKY_SHOTS=combined or through17");
        for pixel in value.split(';') {
            let (x, y) = pixel.split_once(',')
                .ok_or_else(|| anyhow::anyhow!("AOMAC_SKY_PIXELS must be x,y;x,y"))?;
            let pixel = (x.parse::<u32>()?, y.parse::<u32>()?);
            anyhow::ensure!(pixel.0 < size.0 && pixel.1 < size.1, "AOMAC_SKY_PIXELS must be inside the capture");
            pixels.push(pixel);
        }
    }
    let probe_far = if std::env::var_os("AOMAC_SKY_PROBE_FAR").is_some() {
        anyhow::ensure!(!pixels.is_empty(), "AOMAC_SKY_PROBE_FAR requires AOMAC_SKY_PIXELS");
        Some(scalar("AOMAC_SKY_PROBE_FAR", 0.0)?)
    } else {
        None
    };
    let store = ao_rdb::RecordStore::open(&dir)?;
    let scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, time)?;
    assert!(!scene.sky.is_empty(), "ICC must supply authored sky layers");
    anyhow::ensure!(
        matches!(shots.as_str(), "all" | "combined" | "world" | "dry" | "sky" | "layers")
            || (0..scene.sky.len()).any(|i| {
                ["layer", "omit", "through"].iter().any(|prefix| shots == format!("{prefix}{i:02}"))
            }),
        "AOMAC_SKY_SHOTS must be all, combined, world, dry, sky, layers, or existing layerNN/omitNN/throughNN",
    );
    let mut lens = camera::lens(scene.lens.unwrap_or_default(), wide == 1.0);
    let far = scalar("AOMAC_SKY_FAR", camera::far_plane(camera::VIEW_DISTANCE))?;
    anyhow::ensure!(far > lens.near, "AOMAC_SKY_FAR must be greater than the near plane");
    lens.far = Some(far);
    let mut shot = Shot {
        gui: Gui::new(&dir, None)?,
        scene: None,
        camera: Camera { pos: eye, yaw, pitch, roll: 0.0 },
        lens,
    };
    let mut report = format!(
        "reference=user current-client screenshot (NOT retail visual proof)\nsize={}x{}\ncamera=INFERRED, not recovered from screenshot\n\
         eye={eye:?} yaw={yaw} pitch={pitch} direction={:?} lens={lens:?}\n\
         time={time} (game seconds, NOT screenshot/chat wall time)\n\
         default_camera_source=offscreen landmark calibration camera-view-i; initial area located from docs/zone/camera.md ICC idle capture\n\
         default_camera_residuals_approx_pixels= wreck_mast(-3,+3), shuttle_antenna(-5,0), overhang_right_edge(+7,0)\n\
         default_camera_limit=foreground grass/shoreline differs; no exact camera or saved preferences recovered\n\
         default_camera_landmarks=left sea wreck; raised shuttle right; grass beach; overhang upper-left\n\
         native_ICC_game_time=2648.69, authored FrozenTime override via FXS duplicate-object replacement; independent of wall16:50\n\
         scene_environment={:?}\n\
         Combined frame contains installed static playfield and sky, not captured avatar/NPCs/HUD.\n",
        size.0, size.1, shot.camera.forward(), scene.environment,
    );
    let names = ao_formats::character::NameTable::load(&store)?;
    let tweaks = ao_formats::playfield::Tweaks::load(&dir, 4582, true)
        .ok_or_else(|| anyhow::anyhow!("missing ICC tweaks"))?;
    let objects = tweaks.objects();
    writeln!(report, "\nAuthored pre-render and mesh objects (including objects hidden at this game time):")?;
    for (index, object) in objects.iter().enumerate().filter(|(_, o)| {
        o.field("Priority").is_some_and(|p| p.trim() == "e_RenderPriority_PreRendering")
            || o.string("Mesh").is_some()
    }) {
        let mesh = object.string("Mesh");
        let record = mesh.and_then(|name| names.id(ao_formats::mesh::MESH_TYPE, name));
        writeln!(report, "object{index}: name={} fxid={} mesh={mesh:?} record={record:?} texture={:?} sort={:?} visibility={:?} states={:?}",
            object.name, object.fxid(), object.texture, object.field("Sorting"), object.field("Visibility"), object.states)?;
        writeln!(report, "  authored fields={:?}", object.fields)?;
    }
    writeln!(report, "\nNative liquid surfaces: count={}", scene.liquids.len())?;
    for (index, surface) in scene.liquids.iter().enumerate() {
        let mesh = &scene.meshes[surface.mesh];
        writeln!(report, "liquid{index}: mesh={} style={} vertices={}",
            surface.mesh, surface.style, mesh.vertices.len())?;
        for (material, submesh) in mesh.submeshes.iter().enumerate() {
            writeln!(report, "  material{material}: liquid={} texture={:?} indices={}",
                submesh.liquid, submesh.texture, submesh.indices.len())?;
        }
    }
    let mut off = Offscreen::new(&shot, size)?;
    std::fs::create_dir_all(&out)?;
    if matches!(shots.as_str(), "all" | "combined") {
        shot.scene = Some(scene.clone());
        let list = off.frame(&mut shot, 0.0);
        off.png(&shot, &list, &out.join("icc-retail-combined.png"))?;
    }
    if matches!(shots.as_str(), "all" | "world") {
        let mut world_only = scene.clone();
        for instance in &scene.sky {
            world_only.meshes[instance.mesh].submeshes.clear();
        }
        shot.scene = Some(world_only);
        let list = off.frame(&mut shot, 0.0);
        off.png(&shot, &list, &out.join("icc-retail-world.png"))?;
    }
    if matches!(shots.as_str(), "all" | "dry") {
        let mut dry = scene.clone();
        dry.liquids.clear();
        for mesh in &mut dry.meshes {
            for submesh in &mut mesh.submeshes {
                if submesh.liquid {
                    submesh.indices.clear();
                }
            }
        }
        shot.scene = Some(dry);
        let list = off.frame(&mut shot, 0.0);
        off.png(&shot, &list, &out.join("icc-retail-dry.png"))?;
    }
    // Keep instance indices and animation metadata intact. Empty world draw geometry rather
    // than removing instances that movers/LOD reference.
    let mut sky_only = scene.clone();
    sky_only.liquids.clear();
    sky_only.ground_detail.clear();
    for (index, mesh) in sky_only.meshes.iter_mut().enumerate() {
        if !scene.sky.iter().any(|instance| instance.mesh == index) {
            mesh.submeshes.clear();
        }
    }
    if matches!(shots.as_str(), "all" | "sky") {
        shot.scene = Some(sky_only.clone());
        let list = off.frame(&mut shot, 0.0);
        off.png(&shot, &list, &out.join("icc-retail-sky.png"))?;
    }
    for (prefix, cumulative) in [("omit", false), ("through", true)] {
        if let Some(ordinal) = shots.strip_prefix(prefix) {
            let ordinal = ordinal.parse::<usize>()?;
            let mut probe = if cumulative { sky_only.clone() } else { scene.clone() };
            for (index, instance) in scene.sky.iter().enumerate() {
                let remove = if cumulative { index > ordinal } else { index == ordinal };
                if remove {
                    probe.meshes[instance.mesh].submeshes.clear();
                }
            }
            shot.scene = Some(probe);
            let list = off.frame(&mut shot, 0.0);
            off.png(&shot, &list, &out.join(format!("icc-retail-{shots}.png")))?;
        }
    }
    for (ordinal, instance) in scene.sky.iter().enumerate() {
        let mesh = &scene.meshes[instance.mesh];
        let (lo, hi) = mesh.vertices.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), v| (lo.min(Vec3::from(v.pos)), hi.max(Vec3::from(v.pos))),
        );
        assert!(lo.is_finite() && hi.is_finite());
        writeln!(report, "\nlayer{ordinal:02}: mesh={} vertices={} bounds={lo:?}..{hi:?} transform={:?}",
            instance.mesh, mesh.vertices.len(), instance.transform)?;
        let object_index = scene.sky_pose_objects.iter().find_map(|&(instance, object)| {
            (instance == ordinal).then_some(object)
        }).ok_or_else(|| anyhow::anyhow!("layer{ordinal:02} has no authored object mapping"))?;
        let object = objects.get(object_index)
            .ok_or_else(|| anyhow::anyhow!("layer{ordinal:02} object index {object_index} is out of range"))?;
        let source_mesh = object.string("Mesh");
        let record = source_mesh.and_then(|name| names.id(ao_formats::mesh::MESH_TYPE, name));
        writeln!(report, "  authored object{object_index}: name={} fxid={} mesh={source_mesh:?} record={record:?} sorting={:?}",
            object.name, object.fxid(), object.field("Sorting"))?;
        writeln!(report, "  authored transform: Rotation={:?} Scale={:?} ScaleType={:?} Position={:?} PositionType={:?} MeshIndex={:?}",
            object.field("Rotation"), object.field("Scale"), object.field("ScaleType"),
            object.field("Position"), object.field("PositionType"), object.field("MeshIndex"))?;
        writeln!(report, "  Geometry above is baked; authored formulas and factory-frame transforms are not the identity instance transform.")?;
        for (material, submesh) in mesh.submeshes.iter().enumerate() {
            writeln!(report, "  material{material}: texture={:?} blend={:?} base={:?} emissive={:?} sky_fog={} fog_color={:?} indices={}",
                submesh.texture, submesh.blend, submesh.base_color, submesh.emissive,
                submesh.sky_fog, submesh.sky_fog_color, submesh.indices.len())?;
        }
        if !matches!(shots.as_str(), "all" | "layers") && shots != format!("layer{ordinal:02}") {
            continue;
        }
        let mut isolated = sky_only.clone();
        for (index, other) in scene.sky.iter().enumerate() {
            if index != ordinal {
                anyhow::ensure!(other.mesh != instance.mesh, "shared sky mesh cannot be isolated by clearing submeshes");
                isolated.meshes[other.mesh].submeshes.clear();
            }
        }
        shot.scene = Some(isolated);
        let list = off.frame(&mut shot, 0.0);
        off.png(&shot, &list, &out.join(format!("icc-retail-layer{ordinal:02}.png")))?;
    }
    if !pixels.is_empty() {
        writeln!(report, "\n{}", off.sky_pixel_probe(17, &pixels, &scene, probe_far)?)?;
    }
    std::fs::write(out.join("icc-retail-objects.txt"), &report)?;
    eprintln!("{report}");
    Ok(())
}

/// Frozen world evidence: no network, window, wall-clock simulation or desktop capture.
/// Run with `AOMAC_GFX_OUT=$HOME/aomac-handoff/gfx/<new-run>`; see docs/play.md.
#[test]
#[ignore = "offline graphics matrix; requires installed client and writes handoff evidence"]
fn graphics_offline_fixture() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::fmt::Write;
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a new handoff directory")?);
    let profile = std::env::var("AOMAC_GFX_SETTINGS").unwrap_or_else(|_| "faithful".into());
    let settings = graphics_fixture_settings(&profile)?;
    ensure!(!out.exists(), "refusing to overwrite evidence {}", out.display());
    std::fs::create_dir_all(&out)?;
    let status = out.join("status.txt");
    std::fs::write(&status, "status=running\nvisual_acceptance=unexercised\n")?;
    let result = (|| -> anyhow::Result<()> {
        let dir = ao_gui::client_dir();
        let store = ao_rdb::RecordStore::open(&dir)?;
        let command = |program: &str, args: &[&std::ffi::OsStr]| -> anyhow::Result<String> {
            let output = std::process::Command::new(program).args(args)
                .current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
            ensure!(output.status.success(), "{program} failed");
            Ok(String::from_utf8(output.stdout)?.trim().to_owned())
        };
        let revision = command("git", &["rev-parse".as_ref(), "HEAD".as_ref()])?;
        let source_diff = command("git", &["diff".as_ref(), "--no-ext-diff".as_ref(), "HEAD".as_ref()])?;
        let resource = command("shasum", &["-a".as_ref(), "256".as_ref(), dir.join("cd_image/rdb.db").as_os_str()])?;
        let weather = ao_formats::weather::State::clear();
        let size = (1920, 1200);
        let mut manifest = format!("fixture_version=1\nrevision={}\nprofile={profile}\nsize={}x{}\npixel_format=RGBA8\nrow_order=top_to_bottom\nrenderer_time=0\neffect_time=0\ngame_day=0\ncrt_seed=1\ncloud_seed=1\nweather={weather:?}\nmsaa=1\nanisotropy=1\nwide=0\nscenery=1\nvisual_acceptance=unexercised\n",
            revision, size.0, size.1);
        writeln!(manifest, "source_diff={source_diff:?}\nresource_sha256={resource}")?;
        writeln!(manifest, "icc_clock=authored_frozen2648.69\nsun_low_high_night_source=Borealis800")?;
        writeln!(manifest, "enhanced_settings={settings:?}")?;
        let baseline = std::env::var_os("AOMAC_GFX_BASELINE").map(PathBuf::from);
        let png_only = std::env::var_os("AOMAC_GFX_PNG_ONLY").is_some();
        writeln!(manifest, "png_only={png_only}")?;
        let actor = graphics_fixture_actor()?;
        let mut views = graphics_fixture_views(&actor);
        let selected = std::env::var("AOMAC_GFX_VIEW").ok();
        if let Some(name) = &selected {
            views.retain(|(view, _, _, _)| view == name);
            ensure!(!views.is_empty(), "unknown AOMAC_GFX_VIEW {name:?}");
        }
        writeln!(manifest, "selected_view={selected:?}")?;
        writeln!(manifest, "actor_source=docs/captures/zone_ithaca.rec actor_id=25988 actor_name={:?} actor_pos={:?} actor_yaw={:?} actor_pose=idle actor_time=0", actor.name, actor.pos, actor.yaw())?;
        writeln!(manifest, "borealis_lightbar_fixture_override=Testy mount5 mesh26163 field0 byte0 pos[679.6,72.8,-482.7] yaw0 pose_idle_time0 native_mounted_lights=enabled")?;
        std::fs::write(out.join("manifest.txt"), &manifest)?;
        // Weather and game day are fixed for this entire cache; preserve exact time bits.
        let mut snapshots = std::collections::HashMap::new();
        let mut scene_load_count = 0;
        let mut scene_load_time = std::time::Duration::ZERO;
        for (name, pf, time, fixed) in views {
            let (scene, grass_exposure) = match snapshots.entry((pf, time.to_bits())) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    // Every unique snapshot starts with the original loader's CRT seed.
                    ao_scene::crt_srand(1);
                    let started = std::time::Instant::now();
                    let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
                    scene_load_time += started.elapsed();
                    scene_load_count += 1;
                    let grass_exposure = if settings.ground_grass && settings.grass_density > 0.0 {
                        Some(super::player::grass_exposure_surface(std::rc::Rc::new(std::cell::RefCell::new(
                            ao_formats::playfield::collision::Collision::load(&store, pf)?))))
                    } else { None };
                    entry.insert((scene, grass_exposure))
                }
            };
            let capture = |measure: bool| -> anyhow::Result<(Vec<u8>, Lens, usize, usize)> {
                // Fresh renderers and the original per-capture CRT reset remain independent.
                // Both consume clones of the same immutable snapshot, including across views.
                ao_scene::crt_srand(1);
                let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
                lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
                let counts = (scene.meshes.len(), scene.instances.len());
                let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
                let mut off = Offscreen::evidence(&shot, size)?;
                off.fixed_camera(Some(fixed));
                off.host.msaa_samples = Some(1);
                off.host.anisotropy = Some(1);
                off.host.fog_density_scale = Some(1.0);
                off.host.enhanced_settings = Some(settings);
                off.host.grass_exposure = grass_exposure.clone();
                off.host.grass_wind = weather.wind;
                let mut list = off.frame(&mut shot, 0.0);
                let lightbar = matches!(name.as_str(), "borealis-lightbar-night" | "borealis-lightbar-night-close");
                if name == "icc-actor-authored" || lightbar {
                    let mut appearance = actor.clone();
                    if lightbar {
                        appearance.attractors.retain(|mount| mount.place != 5);
                        appearance.attractors.push(ao_net::n3::dynel::AttractorMesh { place: 5, mesh: 26163, field: 0, byte: 0 });
                    }
                    let mut avatar = super::avatar::Avatar::new(&store, &dir, 25988, &appearance,
                        ao_formats::character::actor::ActorAssets::new(&store)?)?;
                    if lightbar { avatar.set_transform([679.6, 72.8, -482.7], 0.0); }
                    off.host.dynamic_lights.extend(avatar.lights());
                    off.host.actor_models.push((super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
                    let frame = avatar.frame();
                    if grass_exposure.is_some() {
                        let [x, y, z, _] = frame.transform[3];
                        off.host.grass_benders.push(ao_render::GrassBender { actor: None, position: [x, y, z] });
                    }
                    off.host.actors.push(frame);
                    list = off.frame(&mut shot, 0.0);
                }
                if grass_exposure.is_some() { off.warm_grass_tiles(); }
                let raw = off.rgba(&shot, &list)?;
                if measure && profile == "bloom" && lightbar {
                    let mask = off.emissive_seed_rgba()?.context("Bloom emission mask")?;
                    image::save_buffer(out.join(format!("{name}-emissive-mask.png")),
                        &mask, size.0, size.1, image::ColorType::Rgba8)?;
                }
                if measure && profile == "shadows" && std::env::var_os("AOMAC_GFX_SHADOW_TIMING").is_some() {
                    graphics_shadow_timing(&mut off, &out, &name)?;
                }
                Ok((raw, lens, counts.0, counts.1))
            };
            let (raw, lens, meshes, instances) = capture(true).with_context(|| format!("capture {name}"))?;
            if !png_only {
                std::fs::write(out.join(format!("{name}.rgba")), &raw)?;
            }
            image::save_buffer(out.join(format!("{name}.png")), &raw, size.0, size.1, image::ColorType::Rgba8)?;
            let (repeated, _, _, _) = capture(false).with_context(|| format!("repeat {name}"))?;
            let differing = |other: &[u8]| raw.iter().zip(other).filter(|(a, b)| a != b).count() + raw.len().abs_diff(other.len());
            let repeat_differences = differing(&repeated);
            if repeat_differences != 0 {
                if png_only {
                    image::save_buffer(out.join(format!("{name}-repeat.png")), &repeated, size.0, size.1, image::ColorType::Rgba8)?;
                } else {
                    std::fs::write(out.join(format!("{name}-repeat.rgba")), &repeated)?;
                }
                for (index, (a, b)) in raw.iter().zip(&repeated).enumerate().filter(|(_, (a, b))| a != b).take(16) {
                    writeln!(manifest, "{name} difference x={} y={} channel={} first={a} repeat={b}", index / 4 % size.0 as usize, index / 4 / size.0 as usize, index % 4)?;
                }
            }
            writeln!(manifest, "view={name} pf={pf} time={time} camera={fixed:?} lens={lens:?} meshes={meshes} instances={instances} raw_bytes={} repeat_differing_bytes={repeat_differences}", raw.len())?;
            std::fs::write(out.join("manifest.txt"), &manifest)?;
            ensure!(repeat_differences == 0, "{name}: independent captures differ in {repeat_differences} bytes");
            if let Some(base) = &baseline {
                let previous = if png_only {
                    image::open(base.join(format!("{name}.png")))?.into_rgba8().into_raw()
                } else {
                    std::fs::read(base.join(format!("{name}.rgba")))?
                };
                ensure!(previous.len() == raw.len(), "{name}: baseline byte length changed");
                let count = differing(&previous);
                let maximum = raw.iter().zip(&previous).map(|(a,b)| a.abs_diff(*b)).max().unwrap_or(0);
                writeln!(manifest, "{name} baseline_differing_bytes={count} baseline_max_byte_error={maximum}")?;
                if profile == "neutral" && name == "icc-platform-authored" {
                    writeln!(manifest, "{name} approved_envelope_max_byte_error=1 approved_envelope_differing_bytes=134")?;
                }
                std::fs::write(out.join("manifest.txt"), &manifest)?;
                ensure!(graphics_fixture_baseline_matches(&profile, &name, count, maximum),
                    "{name}: baseline differs in {count} bytes, maximum byte error {maximum}");
            }
            eprintln!("{name}: {} bytes, independent repeat identical", raw.len());
        }
        let scene_load_ms = scene_load_time.as_secs_f64() * 1000.0;
        writeln!(manifest, "scene_load_count={scene_load_count}\nscene_load_ms={scene_load_ms:.3}")?;
        eprintln!("scene_load_count={scene_load_count} scene_load_ms={scene_load_ms:.3}");
        writeln!(manifest, "status=pass")?;
        std::fs::write(out.join("manifest.txt"), manifest)?;
        Ok(())
    })();
    match &result {
        Ok(()) => std::fs::write(status, "status=pass\nvisual_acceptance=unexercised\n")?,
        Err(error) => std::fs::write(status, format!("status=failed\nerror={error:#}\nvisual_acceptance=unexercised\n"))?,
    }
    result
}

fn graphics_fixture_baseline_matches(profile: &str, view: &str, count: usize, maximum: u8) -> bool {
    // User-approved known envelope, never a general tolerance or an all-off exception.
    (count == 0 && maximum == 0)
        || (profile == "neutral" && view == "icc-platform-authored" && count <= 134 && maximum <= 1)
}

#[test]
fn graphics_fixture_baseline_envelope_is_view_specific() {
    assert!(graphics_fixture_baseline_matches("neutral", "icc-platform-authored", 134, 1));
    assert!(!graphics_fixture_baseline_matches("neutral", "icc-platform-authored", 135, 1));
    assert!(!graphics_fixture_baseline_matches("neutral", "icc-platform-authored", 134, 2));
    assert!(!graphics_fixture_baseline_matches("faithful", "icc-platform-authored", 1, 1));
    assert!(!graphics_fixture_baseline_matches("neutral", "icc-landmark-authored", 1, 1));
    assert!(graphics_fixture_baseline_matches("faithful", "icc-platform-authored", 0, 0));
}

fn graphics_fixture_actor() -> anyhow::Result<ao_net::n3::dynel::SimpleCharFullUpdate> {
    use ao_net::{frame::{Frame, PT_N3}, n3::{self, N3, dynel::Dynel}};
    for line in include_str!("../../../../docs/captures/zone_ithaca.rec").lines() {
        let mut words = line.split_whitespace();
        let (Some(_), Some("<"), Some(hex)) = (words.next(), words.next(), words.next()) else { continue };
        let bytes = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
            .collect::<Result<Vec<_>, _>>()?;
        let Some((frame, _)) = Frame::decode_with(&bytes, false)? else { continue };
        if frame.ptype != PT_N3 { continue; }
        let message = n3::decode(&frame)?;
        if message.header.target.instance != 25988 { continue; }
        if let N3::Dynel(Dynel::SimpleCharFullUpdate(update)) = message.body {
            return Ok(*update);
        }
    }
    anyhow::bail!("zone_ithaca.rec has no captured own actor 25988")
}

fn graphics_fixture_views(actor: &ao_net::n3::dynel::SimpleCharFullUpdate) -> Vec<(String, u32, f32, Camera)> {
    // Recorded positions retained from the graphics baseline; headings are explicit fixture
    // choices, not claims of matching a retail reference screenshot.
    let mut views = Vec::new();
    for (name, eye, yaw, pitch) in [
        ("icc-landmark", Vec3::new(933.5, 26.0, -748.0), 3.765, 0.046),
        ("icc-platform", Vec3::new(940.0, 48.72, -874.78), 3.765, -0.2),
    ] {
        views.push((format!("{name}-authored"), 4582, 2648.69, Camera { pos: eye, yaw, pitch, roll: 0.0 }));
    }
    for (name, eye, at) in [
        ("borealis-street", [679.6, 74.5, -476.7], [679.6, 74.5, -477.7]),
        ("borealis-foliage", [652.0, 65.3, -420.0], [651.8296, 73.42179, -428.4731]),
    ] {
        for (light, time) in [("sun-high", 3240.0), ("sun-low", 4600.0), ("sunset", 4860.0), ("night", 0.0)] {
            views.push((format!("{name}-{light}"), 800, time, Camera::look_at(eye.into(), at.into())));
        }
    }
    views.push(("subway-corridor".into(), 127, 3240.0,
        Camera::look_at([96.0, 109.305, -242.0].into(), [108.0, 108.9, -242.0].into())));
    let actor_at = Vec3::new(actor.pos[0], actor.pos[1] + 1.2, -actor.pos[2]);
    views.push(("icc-actor-authored".into(), 4582, 2648.69,
        Camera::look_at(actor_at + Vec3::new(0.0, 1.0, 5.0), actor_at)));
    views.push(("borealis-lightbar-night".into(), 800, 0.0,
        Camera::look_at([679.6, 74.5, -476.7].into(), [679.6, 74.0, -482.7].into())));
    views.push(("borealis-lightbar-night-close".into(), 800, 0.0,
        Camera::look_at([679.6, 74.0, -480.2].into(), [679.6, 74.0, -482.7].into())));
    views
}

#[test]
fn graphics_fixture_snapshot_keys_share_only_matching_playfield_and_time() -> anyhow::Result<()> {
    let views = graphics_fixture_views(&graphics_fixture_actor()?);
    let mut keys = std::collections::HashMap::<_, usize>::new();
    for (_, pf, time, _) in &views {
        *keys.entry((*pf, time.to_bits())).or_default() += 1;
    }
    assert_eq!(views.len(), 14);
    assert_eq!(keys.len(), 6);
    assert_eq!(keys[&(4582, 2648.69_f32.to_bits())], 3);
    assert_eq!(keys[&(800, 0.0_f32.to_bits())], 4);
    assert_eq!(keys[&(800, 3240.0_f32.to_bits())], 2);
    assert_eq!(keys[&(127, 3240.0_f32.to_bits())], 1);
    Ok(())
}

fn graphics_fixture_settings(profile: &str) -> anyhow::Result<ao_render::EnhancedSettings> {
    let mut settings = ao_render::EnhancedSettings::default();
    match profile {
        "faithful" => {}
        "neutral" => { settings.ground_grass = true; settings.grass_density = 0.0; }
        "shadows" => settings.sun_shadows = true,
        // Pre-soft-shadow look: 1024 maps, hard 3x3 PCF, full-strength shadow.
        "shadows-hard" => {
            settings.sun_shadows = true;
            (settings.shadow_map_size, settings.shadow_softness, settings.shadow_strength) = (1024, 0.0, 1.0);
        }
        "ao" => settings.ambient_occlusion = true,
        "bloom" => settings.emissive_bloom = true,
        "grass" => settings.ground_grass = true,
        "hdr" => settings.hdr = true,
        "hdr-bloom" => { settings.hdr = true; settings.emissive_bloom = true; }
        "hdr-bloom-max" => { settings.hdr = true; settings.emissive_bloom = true; settings.sun_boost = 0.6; }
        "hdr-max" => { settings.hdr = true; settings.sun_boost = 0.6; }
        "hdr-neutral" => { settings.hdr = true; settings.sun_boost = 0.0; settings.grade_strength = 0.0; }
        "contact" => settings.contact_shadows = true,
        // Isolates the contact pass: shadows + hdr + contact vs `shadows-hdr`.
        "shadows-hdr" => { settings.sun_shadows = true; settings.hdr = true; }
        "shadows-hdr-contact" => { settings.sun_shadows = true; settings.hdr = true; settings.contact_shadows = true; }
        "all" => {
            settings.sun_shadows = true;
            settings.ambient_occlusion = true;
            settings.emissive_bloom = true;
            settings.ground_grass = true;
            settings.hdr = true;
            settings.contact_shadows = true;
        }
        _ => anyhow::bail!("unknown AOMAC_GFX_SETTINGS profile {profile:?}; use faithful|neutral|shadows|shadows-hard|ao|bloom|grass|hdr|hdr-bloom|hdr-bloom-max|hdr-max|hdr-neutral|contact|shadows-hdr|shadows-hdr-contact|all"),
    }
    Ok(settings)
}

#[test]
fn graphics_fixture_profiles_are_explicit_and_off_by_default() {
    assert!(!graphics_fixture_settings("faithful").unwrap().any_enabled());
    let neutral = graphics_fixture_settings("neutral").unwrap();
    assert!(neutral.any_enabled() && neutral.grass_density == 0.0);
    for (name, expected) in [
        ("neutral", [false, false, false, true]), ("shadows", [true, false, false, false]),
        ("shadows-hard", [true, false, false, false]),
        ("ao", [false, true, false, false]), ("bloom", [false, false, true, false]),
        ("grass", [false, false, false, true]), ("all", [true; 4]),
    ] {
        let s = graphics_fixture_settings(name).unwrap();
        assert_eq!([s.sun_shadows, s.ambient_occlusion, s.emissive_bloom, s.ground_grass], expected);
        assert_eq!((s.hdr, s.contact_shadows), (name == "all", name == "all"));
    }
    assert!(graphics_fixture_settings("contact").unwrap().contact_shadows);
    let isolated = graphics_fixture_settings("shadows-hdr-contact").unwrap();
    assert!(isolated.sun_shadows && isolated.hdr && isolated.contact_shadows);
    assert!(!graphics_fixture_settings("shadows-hdr").unwrap().contact_shadows);
    assert!(graphics_fixture_settings("hdr").unwrap().hdr);
    let hdr_bloom = graphics_fixture_settings("hdr-bloom").unwrap();
    assert!(hdr_bloom.hdr && hdr_bloom.emissive_bloom && !hdr_bloom.sun_shadows);
    let neutral_hdr = graphics_fixture_settings("hdr-neutral").unwrap();
    assert!(neutral_hdr.hdr && neutral_hdr.sun_boost == 0.0);
    assert!(graphics_fixture_settings("unknown").is_err());
    let actor = graphics_fixture_actor().unwrap();
    assert_eq!(actor.name, "Testy");
    let views = graphics_fixture_views(&actor);
    for name in ["borealis-street", "borealis-foliage"] {
        for (light, expected_time) in [("sun-low", 4600.0), ("sunset", 4860.0)] {
            let view_name = format!("{name}-{light}");
            let view = views.iter().find(|view| view.0 == view_name).unwrap();
            assert_eq!(view.1, 800);
            assert_eq!(view.2, expected_time);
        }
    }
}

fn graphics_shadow_timing(off: &mut Offscreen, directory: &std::path::Path, name: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::fmt::Write;
    let mut report = String::from("resolution=1920x1200\nwarmup=120\nsamples=600\nunits=milliseconds\nmap_and_frame_spans_sampled_separately=true\n");
    off.prepare_frozen_rendering();
    let percentiles = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        [0.50, 0.95, 0.99].map(|p| samples[((samples.len() - 1) as f64 * p).round() as usize])
    };
    for (label, profile) in [("faithful", "faithful"), ("shadows", "shadows"), ("neutral", "neutral"),
        ("neutral-repeat", "neutral"), ("shadows-repeat", "shadows")] {
        let settings = graphics_fixture_settings(profile)?;
        off.set_enhanced_settings(settings);
        let mut maps = Vec::with_capacity(600);
        if settings.sun_shadows {
            off.set_shadow_gpu_timing(true)?;
            for sample in 0..720 {
                off.render_frozen()?;
                let Some(ms) = off.read_shadow_gpu_ms()? else { break; }; // Authored night/no-sun: no shadow pass exists.
                if sample >= 120 { maps.push(ms); }
            }
            off.set_shadow_gpu_timing(false)?;
        }
        let mut frames = Vec::with_capacity(600);
        off.set_gpu_timing(true)?;
        for sample in 0..720 {
            off.render_frozen()?;
            let ms = off.read_frame_gpu_ms()?.context("frame GPU timer did not record")?;
            if sample >= 120 { frames.push(ms); }
        }
        let frame_ms = percentiles(&mut frames);
        let map_ms = (!maps.is_empty()).then(|| percentiles(&mut maps));
        writeln!(report, "profile={label} frame_p50_p95_p99={frame_ms:?} shadow_map_p50_p95_p99={map_ms:?} shadow_samples={} logical_shadow_bytes={} enhanced_target_bytes={} native_metal_allocated_bytes={:?}",
            maps.len(), off.shadow_resident_bytes(), off.enhanced_target_bytes(), off.gpu_allocated_bytes())?;
        off.set_gpu_timing(false)?;
    }
    std::fs::write(directory.join(format!("{name}-gpu.txt")), report)?;
    Ok(())
}

/// Frozen Enhanced CPU/GPU ABBA benchmark; no capture or PNG work.
#[test]
#[ignore = "installed-client headless Enhanced CPU/GPU benchmark; 120 warmup + 600 samples per phase"]
fn graphics_enhanced_cpu_timing() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::fmt::Write;
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a new directory")?);
    ensure!(!out.exists(), "refusing to overwrite evidence {}", out.display());
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("status.txt"), "status=running\n")?;
    let result = (|| -> anyhow::Result<()> {
        let dir = ao_gui::client_dir();
        let store = ao_rdb::RecordStore::open(&dir)?;
        let actor = graphics_fixture_actor()?;
        let selected = std::env::var("AOMAC_GFX_VIEW").unwrap_or_else(|_| "borealis-foliage-sun-high".into());
        ensure!(selected.starts_with("borealis-foliage-"), "CPU timing requires a Borealis foliage fixture");
        let (name, pf, time, fixed) = graphics_fixture_views(&actor).into_iter()
            .find(|(name, _, _, _)| *name == selected).context("unknown AOMAC_GFX_VIEW")?;
        let weather = ao_formats::weather::State::clear();
        ao_scene::crt_srand(1);
        // One immutable scene, shared by every ABBA phase, exactly as in graphics_offline_fixture.
        let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
            &store, &dir, pf, time, 0, Some(weather))?;
        let exposure = super::player::grass_exposure_surface(std::rc::Rc::new(std::cell::RefCell::new(
            ao_formats::playfield::collision::Collision::load(&store, pf)?)));
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        let mut manifest = format!("fixture=graphics_offline_fixture\nbenchmark=graphics_enhanced_cpu_timing\nview={name}\npf={pf}\ngame_time={time}\ncamera={fixed:?}\nlens={lens:?}\nsize=1920x1200\nrenderer_time=0\ndt=0\ncrt_seed=1\nweather={weather:?}\nmsaa=1\nanisotropy=1\nwarmup=120\nsamples_per_phase=600\norder=all,faithful,faithful,all\npercentile=nearest_rank_ceil_np\nunits=milliseconds\ncpu=wall_not_thread_cpu\npixel_readback=false\npng=false\ngpu_readback=after_cpu_sample_serialized_each_frame\n");
        for (label, program, args) in [
            ("revision", "git", vec!["rev-parse".into(), "HEAD".into()]),
            ("source_diff", "git", vec!["diff".into(), "--no-ext-diff".into(), "HEAD".into()]),
            ("resource_sha256", "shasum", vec!["-a".into(), "256".into(), dir.join("cd_image/rdb.db").into_os_string()]),
        ] {
            let output = std::process::Command::new(program).args(args)
                .current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
            ensure!(output.status.success(), "{program} failed");
            writeln!(manifest, "{label}={:?}", String::from_utf8(output.stdout)?.trim())?;
        }
        std::fs::write(out.join("manifest.txt"), &manifest)?;
        let columns = ["gpu_frame_ms", "prepare_ms", "encode_ms", "queue_submit_ms",
            "cpu_render_ms", "shadow_prepare_encode_ms", "grass_prepare_ms", "grass_encode_ms", "ao_ms", "bloom_ms", "finish_ms"];
        let mut csv = format!("phase,profile,sample,{}\n", columns.join(","));
        let mut summary = String::from("phase,profile,metric,samples,mean_ms,p50_ms,p95_ms,p99_ms\n");
        for (phase, profile) in ["all", "faithful", "faithful", "all"].into_iter().enumerate() {
            ao_scene::crt_srand(1);
            let settings = graphics_fixture_settings(profile)?;
            let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
            let mut off = Offscreen::evidence_timing(&shot, (1920, 1200))?;
            off.fixed_camera(Some(fixed));
            off.host.msaa_samples = Some(1);
            off.host.anisotropy = Some(1);
            off.host.fog_density_scale = Some(1.0);
            off.host.enhanced_settings = Some(settings);
            if settings.ground_grass { off.host.grass_exposure = Some(exposure.clone()); }
            off.host.grass_wind = weather.wind;
            off.frame(&mut shot, 0.0);
            if settings.ground_grass { off.warm_grass_tiles(); }
            off.set_cpu_timing(true);
            off.set_gpu_timing(true)?;
            let mut samples = Vec::with_capacity(600);
            for frame in 0..720 {
                off.render_frozen()?;
                // CPU spans have ended before the blocking timestamp resolve/readback.
                let cpu = off.frame_cpu_timings().context("CPU timing disabled")?;
                let gpu = off.read_frame_gpu_ms()?.context("GPU timestamp sample")?;
                if frame < 120 { continue; }
                let row = [gpu, cpu.prepare_ms, cpu.encode_ms, cpu.queue_submit_ms,
                    cpu.prepare_ms + cpu.encode_ms + cpu.queue_submit_ms, cpu.shadow_prepare_encode_ms,
                    cpu.grass_prepare_ms, cpu.grass_encode_ms, cpu.ao_ms, cpu.bloom_ms, cpu.finish_ms];
                write!(csv, "{phase},{profile},{}", frame - 120)?;
                for value in row { write!(csv, ",{value:.9}")?; }
                writeln!(csv)?;
                samples.push(row);
            }
            ensure!(samples.len() == 600, "incomplete measured phase");
            for (column, metric) in columns.iter().enumerate() {
                let mut values: Vec<f64> = samples.iter().map(|row| row[column]).collect();
                ensure!(values.iter().all(|value| value.is_finite() && *value >= 0.0), "invalid {metric}");
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                values.sort_by(f64::total_cmp);
                writeln!(summary, "{phase},{profile},{metric},600,{mean:.9},{:.9},{:.9},{:.9}",
                    values[299], values[569], values[593])?;
            }
            writeln!(manifest, "phase={phase} profile={profile} settings={settings:?} meshes={} instances={} enhanced_target_bytes={} shadow_bytes={} grass_metrics={:?}",
                scene.meshes.len(), scene.instances.len(), off.enhanced_target_bytes(),
                off.shadow_resident_bytes(), off.evidence_grass_metrics())?;
            let (static_instances, ranges_per_cascade) = off.shadow_static_command_counts();
            writeln!(manifest, "phase={phase} shadow_static_admitted_instances={static_instances} shadow_static_batched_ranges_per_cascade={ranges_per_cascade}")?;
            std::fs::write(out.join("frames.csv"), &csv)?;
            std::fs::write(out.join("summary.csv"), &summary)?;
            std::fs::write(out.join("manifest.txt"), &manifest)?;
        }
        eprintln!("ENHANCED_CPU_TIMING {}\n{summary}", out.display());
        Ok(())
    })();
    std::fs::write(out.join("status.txt"), if result.is_ok() { "status=complete\n" } else { "status=failed\n" })?;
    result
}

#[test]
#[ignore = "explicit headless Bloom ABBA benchmark: 120 warmup, 600 GPU samples per phase"]
fn graphics_bloom_gpu_timing() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let actor = graphics_fixture_actor()?;
    let (_, pf, time, fixed) = graphics_fixture_views(&actor).into_iter()
        .find(|(name, _, _, _)| name == "borealis-lightbar-night").context("Light Bar fixture view")?;
    ao_scene::crt_srand(1);
    let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
        &store, &dir, pf, time, 0, Some(ao_formats::weather::State::clear()))?;
    let mut summaries = [[0.0_f64; 3]; 4];
    let mut bytes = [0_u64; 4];
    for (phase, profile) in ["neutral", "bloom", "bloom", "neutral"].into_iter().enumerate() {
        ao_scene::crt_srand(1);
        let mut lens = camera::lens(scene.lens.unwrap_or_default(), false);
        lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
        let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(scene.clone()), camera: fixed, lens };
        let mut off = Offscreen::evidence_timing(&shot, (1920,1200))?;
        off.fixed_camera(Some(fixed));
        off.host.msaa_samples = Some(1);
        off.host.anisotropy = Some(1);
        off.host.fog_density_scale = Some(1.0);
        off.host.enhanced_settings = Some(graphics_fixture_settings(profile)?);
        off.frame(&mut shot, 0.0);
        // Same captured appearance, native mount and placement as the PNG fixture.
        let mut appearance = actor.clone();
        appearance.attractors.retain(|mount| mount.place != 5);
        appearance.attractors.push(ao_net::n3::dynel::AttractorMesh { place: 5, mesh: 26163, field: 0, byte: 0 });
        let mut avatar = super::avatar::Avatar::new(&store, &dir, 25988, &appearance,
            ao_formats::character::actor::ActorAssets::new(&store)?)?;
        avatar.set_transform([679.6,72.8,-482.7], 0.0);
        off.host.dynamic_lights.extend(avatar.lights());
        off.host.actor_models.push((super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
        off.host.actors.push(avatar.frame());
        off.frame(&mut shot, 0.0);
        off.prepare_frozen_rendering();
        off.set_gpu_timing(true)?;
        let mut samples = Vec::with_capacity(600);
        for frame in 0..720 {
            off.render_frozen()?;
            let ms = off.read_frame_gpu_ms()?.context("GPU timestamp sample")?;
            if frame >= 120 { samples.push(ms); }
        }
        samples.sort_by(f64::total_cmp);
        summaries[phase] = [samples[299], samples[569], samples[593]];
        bytes[phase] = off.enhanced_target_bytes();
        eprintln!("BLOOM_FIXTURE_GPU phase={phase} profile={profile} view=borealis-lightbar-night size=1920x1200 warmup=120 samples=600 p50_ms={:.6} p95_ms={:.6} p99_ms={:.6} attachment_bytes={}",
            summaries[phase][0], summaries[phase][1], summaries[phase][2], bytes[phase]);
    }
    ensure!(bytes[0] == bytes[3] && bytes[1] == bytes[2], "ABBA memory accounting changed");
    let delta = std::array::from_fn::<_, 3, _>(|i|
        (summaries[1][i] + summaries[2][i] - summaries[0][i] - summaries[3][i]) * 0.5);
    eprintln!("BLOOM_FIXTURE_GPU_DELTA p50_ms={:.6} p95_ms={:.6} p99_ms={:.6} attachment_bytes={} uniform_bytes=16",
        delta[0], delta[1], delta[2], bytes[1].checked_sub(bytes[0]).context("Bloom allocation delta")?);
    Ok(())
}
#[path = "grass_shots.rs"]
mod grass_shots;
