//! Offline ICC sky evidence, without login or a window:
//! `AOMAC_SHOT_DIR=/tmp/sky-before cargo test --release -p aomac icc_sky_screenshots -- --nocapture`
//! Repeat on the changed revision with `/tmp/sky-after`; matching filenames are comparison pairs.
//! No pixel baseline is asserted: a capture is evidence, not an invented aesthetic oracle.
use super::camera;
use ao_gui::{DrawList, Gui, InputEvent};
use ao_render::{Camera, Frontend, Host, Offscreen, Vec3};
use ao_scene::{Lens, Scene};
use std::path::PathBuf;

mod ao_grain;

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
    let features = std::env::var("AOMAC_GFX_FEATURE").unwrap_or_default();
    let settings = graphics_fixture_post_features(graphics_fixture_settings(&profile)?, &features)?;
    let msaa = graphics_fixture_msaa(&std::env::var("AOMAC_GFX_MSAA").unwrap_or_else(|_| "1".into()))?;
    let own_avatar = match std::env::var("AOMAC_GFX_OWN_AVATAR").as_deref() {
        Ok("1") => true,
        Ok("0") | Err(std::env::VarError::NotPresent) => false,
        _ => anyhow::bail!("AOMAC_GFX_OWN_AVATAR must be 0 or 1"),
    };
    let interaction = match std::env::var_os("AOMAC_GFX_FOLIAGE_INTERACTION") {
        None => false,
        Some(value) if value == "1" => true,
        Some(value) => anyhow::bail!("AOMAC_GFX_FOLIAGE_INTERACTION must be 1 when set, got {value:?}"),
    };
    let sequence_env = |name| -> anyhow::Result<Option<String>> {
        match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(error.into()),
        }
    };
    let sequence = graphics_fixture_sequence(
        sequence_env("AOMAC_GFX_SEQUENCE")?.as_deref(),
        sequence_env("AOMAC_GFX_SEQUENCE_START")?.as_deref(),
        sequence_env("AOMAC_GFX_SEQUENCE_DT")?.as_deref(),
        sequence_env("AOMAC_GFX_SEQUENCE_COUNT")?.as_deref(),
    )?;
    if sequence.is_some() {
        ensure!(!interaction, "sequence mode replaces AOMAC_GFX_FOLIAGE_INTERACTION");
        ensure!(std::env::var_os("AOMAC_GFX_FRAME_TIMING").is_none()
            && std::env::var_os("AOMAC_GFX_SHADOW_TIMING").is_none()
            && std::env::var_os("AOMAC_GFX_TIMING_PAN").is_none(),
            "sequence evidence and frozen timing must be separate runs");
    }
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
        let renderer_time = if sequence.is_some() { "per_sequence_frame" } else { "0" };
        let mut manifest = format!("fixture_version=1\nrevision={}\nprofile={profile}\nsize={}x{}\npixel_format=RGBA8\nrow_order=top_to_bottom\nrenderer_time={renderer_time}\neffect_time={renderer_time}\ngame_day=0\ncrt_seed=1\ncloud_seed=1\nweather={weather:?}\nmsaa_requested={msaa}\nmsaa_actual={msaa}\nanisotropy=1\nwide=0\nscenery=1\nvisual_acceptance=unexercised\n",
            revision, size.0, size.1);
        writeln!(manifest, "source_diff={source_diff:?}\nresource_sha256={resource}")?;
        writeln!(manifest, "icc_clock=authored_frozen2648.69\nsun_low_high_night_source=Borealis800")?;
        writeln!(manifest, "enhanced_settings={settings:?}")?;
        writeln!(manifest, "post_features={features:?}\nforce_own_avatar={own_avatar}\npost_focus_policy=actual_fixture_avatar_position_when_present\npost_warmup_frames=8")?;
        writeln!(manifest, "foliage_interaction={interaction}\nfoliage_interaction_steps=16\nfoliage_interaction_dt=0\nfoliage_interaction_source=captured_visible_own_avatar\nfoliage_interaction_pose=idle")?;
        writeln!(manifest, "sequence={sequence:?}\nsequence_simulation_dt_max={}\nsequence_game_clock=frozen_snapshot\nsequence_avatar_pose=captured_idle_translated_on_terrain", 1.0 / 60.0)?;
        let baseline = std::env::var_os("AOMAC_GFX_BASELINE").map(PathBuf::from);
        let png_only = std::env::var_os("AOMAC_GFX_PNG_ONLY").is_some();
        writeln!(manifest, "png_only={png_only}")?;
        let sky_pixels = std::env::var("AOMAC_SKY_PIXELS").ok().map(|value| {
            value.split(';').map(|pixel| {
                let (x, y) = pixel.split_once(',').context("AOMAC_SKY_PIXELS must be x,y;x,y")?;
                let pixel = (x.parse::<u32>()?, y.parse::<u32>()?);
                ensure!(pixel.0 < size.0 && pixel.1 < size.1, "AOMAC_SKY_PIXELS must be inside the capture");
                Ok(pixel)
            }).collect::<anyhow::Result<Vec<_>>>()
        }).transpose()?.unwrap_or_default();
        let actor = graphics_fixture_actor()?;
        let mut views = graphics_fixture_views(&actor);
        let selected = std::env::var("AOMAC_GFX_VIEW").ok();
        if let Some(name) = &selected {
            views.retain(|(view, _, _, _)| view == name);
            ensure!(!views.is_empty(), "unknown AOMAC_GFX_VIEW {name:?}");
        }
        writeln!(manifest, "selected_view={selected:?}")?;
        if sequence.is_some_and(|sequence| sequence.interaction) {
            ensure!(views.iter().all(|(name, _, _, _)| matches!(name.as_str(), "newland-day" | "newland-dusk" | "newland-night")),
                "interaction sequence requires AOMAC_GFX_VIEW=newland-day, newland-dusk, or newland-night");
        }
        writeln!(manifest, "actor_source=docs/captures/zone_ithaca.rec actor_id=25988 actor_name={:?} actor_pos={:?} actor_yaw={:?} actor_pose=idle actor_time=0", actor.name, actor.pos, actor.yaw())?;
        writeln!(manifest, "borealis_lightbar_fixture_override=Testy mount5 mesh26163 field0 byte0 pos[679.6,72.8,-482.7] yaw0 pose_idle_time0 native_mounted_lights=enabled")?;
        std::fs::write(out.join("manifest.txt"), &manifest)?;
        // Weather and game day are fixed for this entire cache; preserve exact time bits.
        let mut snapshots = std::collections::HashMap::new();
        let mut scene_load_count = 0;
        let mut scene_load_time = std::time::Duration::ZERO;
        for (name, pf, time, fixed) in views {
            let (scene, grass_exposure, grounded_camera, collision) = match snapshots.entry((pf, time.to_bits())) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    // Every unique snapshot starts with the original loader's CRT seed.
                    ao_scene::crt_srand(1);
                    let started = std::time::Instant::now();
                    let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
                    let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
                    super::player::bake_sky_open(&collision, &mut scene);
                    let grounded_camera = (pf == 566).then(||
                        graphics_fixture_newland_camera(|point| collision.terrain_height(point))).transpose()?;
                    scene_load_time += started.elapsed();
                    scene_load_count += 1;
                    let collision = std::rc::Rc::new(std::cell::RefCell::new(collision));
                    let grass_exposure = if settings.foliage_enabled() || settings.interaction_enabled() {
                        Some(super::player::grass_exposure_surface(collision.clone()))
                    } else { None };
                    entry.insert((scene, grass_exposure, grounded_camera, collision))
                }
            };
            let fixed = grounded_camera.unwrap_or(fixed);
            let fixed = if matches!(name.as_str(), "borealis-foliage-ground-sun-high" | "newland-foliage-ground-day") {
                graphics_fixture_ground_foliage_camera(scene, fixed,
                    |point| collision.borrow().terrain_height(point),
                    |eye, target| collision.borrow().line(eye, target).is_none(),
                    &mut manifest)?
            } else { fixed };
            let fixed = if name == "newland-backlit-day" {
                let (camera, instance, submesh, canopy, alignment) = graphics_fixture_backlit_camera(
                    scene, fixed.pos, |point| collision.borrow().terrain_height(point),
                    |eye, target| collision.borrow().line(eye, target).is_none())?;
                writeln!(manifest, "{name} backlit_instance={instance} backlit_mesh={} backlit_submesh={submesh} canopy_scene={canopy:?} toward_sun_alignment={alignment} camera_source=native_vegetation_world_bounds_and_authored_sun collision_los=clear material_lighting=true material_fog=true material_depth_write=true", scene.instances[instance].mesh)?;
                camera
            } else { fixed };
            let (fixed, interaction_root) = if sequence.is_some_and(|sequence| sequence.interaction) {
                let names = ao_formats::character::NameTable::load(&store)?;
                let selected = graphics_fixture_shrub(scene, fixed.pos, &names, &collision.borrow(), &mut manifest);
                std::fs::write(out.join("manifest.txt"), &manifest)?;
                let (root, camera) = selected?;
                (camera, Some(root))
            } else { (fixed, None) };
            let interaction_path = if interaction {
                // Newland pf566 statel+0x4060, mesh95575 dry_edge_grass_small:
                // native scene root, not a camera-derived or guessed plant position.
                let endpoint = (pf == 566).then_some(Vec3::new(436.432_34, 25.000_1, -320.404_08));
                let path = graphics_fixture_interaction_path(fixed, endpoint,
                    |point| collision.borrow().terrain_height(point))?;
                writeln!(manifest, "{name} interaction_start_scene={:?} interaction_end_scene={:?}", path.0, path.1)?;
                Some(path)
            } else { None };
            let fixed = match interaction_path {
                Some((_, interaction_end)) if name.starts_with("borealis-foliage-") =>
                    Camera::look_at(fixed.pos, interaction_end + Vec3::Y * 0.8),
                _ => fixed,
            };
            if let Some(sequence) = sequence {
                for index in 0..sequence.count {
                    let target = sequence.start + sequence.dt * index as f32;
                    let position = if sequence.interaction {
                        Some(graphics_fixture_sequence_position(interaction_root.context("missing native shrub")?, index as f32 / (sequence.count - 1) as f32,
                            |point| collision.borrow().terrain_height(point))?)
                    } else { None };
                    writeln!(manifest, "{name} sequence_frame={index:02} renderer_time={target} sample_dt={} avatar_scene={position:?}", sequence.dt)?;
                }
            }
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
                off.host.msaa_samples = Some(msaa);
                off.host.anisotropy = Some(1);
                off.host.fog_density_scale = Some(1.0);
                off.host.enhanced_settings = Some(settings);
                off.host.grass_exposure = grass_exposure.clone();
                off.host.grass_wind = weather.wind;
                let mut list = off.frame(&mut shot, 0.0);
                let actual_msaa = off.msaa_samples();
                ensure!(actual_msaa == msaa, "requested MSAA {msaa}, adapter selected {actual_msaa}; evidence requires the requested sample count");
                if measure { std::fs::write(out.join(format!("{name}-msaa.txt")), format!("requested={msaa}\nactual={actual_msaa}\n"))?; }
                if let Some(sequence) = sequence {
                    let mut avatar = if sequence.interaction {
                        Some(super::avatar::Avatar::new(&store, &dir, 25988, &actor,
                            ao_formats::character::actor::ActorAssets::new(&store)?)?)
                    } else { None };
                    if let Some(avatar) = &mut avatar {
                        let start = graphics_fixture_sequence_position(interaction_root.context("missing native shrub")?, 0.0,
                            |point| collision.borrow().terrain_height(point))?;
                        avatar.set_transform(start.to_array(), std::f32::consts::FRAC_PI_2);
                        off.host.actor_models.push((super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
                        off.host.actors.push(avatar.frame());
                        off.host.post_focus = Some(start.to_array());
                        off.host.dynamic_lights.extend(avatar.lights());
                        off.host.grass_benders.push(ao_render::GrassBender { actor: None, position: start.to_array() });
                        off.frame(&mut shot, 0.0);
                    }
                    if grass_exposure.is_some() { off.warm_grass_tiles(); }
                    off.prepare_frozen_rendering();
                    off.render_frozen()?;
                    off.wait_for_pipeline_warmup();
                    let mut elapsed = 0.0_f32;
                    let mut raw = Vec::new();
                    for index in 0..sequence.count {
                        let target = sequence.start + sequence.dt * index as f32;
                        // Render every real simulation step: wind filters and interaction springs
                        // cannot be sampled by seeking the shader clock or by zero-dt renders.
                        loop {
                            let dt = (target - elapsed).min(1.0 / 60.0);
                            if let Some(avatar) = &mut avatar {
                                let progress = ((elapsed + dt - sequence.start)
                                    / (sequence.dt * (sequence.count - 1) as f32)).max(0.0);
                                let position = graphics_fixture_sequence_position(interaction_root.context("missing native shrub")?, progress,
                                    |point| collision.borrow().terrain_height(point))?;
                                avatar.set_transform(position.to_array(), std::f32::consts::FRAC_PI_2);
                                off.host.post_focus = Some(position.to_array());
                                off.host.dynamic_lights.extend(avatar.lights());
                                off.host.actors.push(avatar.frame());
                                off.host.grass_benders.clear();
                                off.host.grass_benders.push(ao_render::GrassBender { actor: None, position: position.to_array() });
                            }
                            off.frame(&mut shot, dt);
                            elapsed += dt;
                            if elapsed >= target { break; }
                        }
                        let file = format!("{name}-frame{index:02}.png");
                        raw = off.frozen_rgba()?;
                        if measure {
                            image::save_buffer(out.join(&file), &raw, size.0, size.1, image::ColorType::Rgba8)?;
                            if let Some(base) = &baseline {
                                let previous = image::open(base.join(&file))?.into_rgba8().into_raw();
                                ensure!(previous == raw, "{file}: sequence baseline differs");
                            }
                        } else {
                            let previous = image::open(out.join(&file))?.into_rgba8().into_raw();
                            ensure!(previous == raw, "{file}: independent sequence captures differ");
                        }
                    }
                    return Ok((raw, lens, counts.0, counts.1));
                }
                let lightbar = matches!(name.as_str(), "borealis-lightbar-night" | "borealis-lightbar-night-close");
                let authored_actor = name.starts_with("icc-actor-") || lightbar;
                if let Some((interaction_start, interaction_end)) = interaction_path {
                    let mut avatar = super::avatar::Avatar::new(&store, &dir, 25988, &actor,
                        ao_formats::character::actor::ActorAssets::new(&store)?)?;
                    off.host.actor_models.push((super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
                    if grass_exposure.is_some() { off.warm_grass_tiles(); }
                    off.prepare_frozen_rendering();
                    for step in 0..16 {
                        let position = interaction_start.lerp(interaction_end, step as f32 / 15.0);
                        avatar.set_transform(position.to_array(), 0.0);
                        off.host.post_focus = Some(position.to_array());
                        off.host.dynamic_lights.extend(avatar.lights());
                        off.host.actors.push(avatar.frame());
                        off.host.grass_benders.clear();
                        off.host.grass_benders.push(ao_render::GrassBender { actor: None, position: position.to_array() });
                        list = off.frame(&mut shot, 0.0);
                        off.render_frozen()?;
                    }
                } else if authored_actor || settings.depth_of_field || own_avatar {
                    let mut appearance = actor.clone();
                    if lightbar {
                        appearance.attractors.retain(|mount| mount.place != 5);
                        appearance.attractors.push(ao_net::n3::dynel::AttractorMesh { place: 5, mesh: 26163, field: 0, byte: 0 });
                    }
                    let mut avatar = super::avatar::Avatar::new(&store, &dir, 25988, &appearance,
                        ao_formats::character::actor::ActorAssets::new(&store)?)?;
                    if lightbar { avatar.set_transform([679.6, 72.8, -482.7], 0.0); }
                    else if !authored_actor {
                        avatar.set_transform((fixed.pos + fixed.forward() * 12.0).to_array(), 0.0);
                    }
                    off.host.dynamic_lights.extend(avatar.lights());
                    off.host.actor_models.push((super::avatar::MODEL_KEY, std::sync::Arc::new(avatar.model().clone())));
                    let frame = avatar.frame();
                    let [x, y, z, _] = frame.transform[3];
                    let focus = [x, y, z];
                    if measure { std::fs::write(out.join(format!("{name}-focus.txt")), format!("actor_id=25988\nscene_position={focus:?}\n"))?; }
                    if grass_exposure.is_some() {
                        let [x, y, z, _] = frame.transform[3];
                        off.host.grass_benders.push(ao_render::GrassBender { actor: None, position: [x, y, z] });
                    }
                    off.host.actors.push(frame);
                    off.host.post_focus = Some(focus);
                    list = off.frame(&mut shot, 0.0);
                }
                if grass_exposure.is_some() { off.warm_grass_tiles(); }
                if settings.post_enabled() {
                    off.prepare_frozen_rendering();
                    for _ in 0..8 { off.render_frozen()?; }
                }
                if interaction || settings.foliage_leaves || settings.foliage_flutter || settings.foliage_falling {
                    off.rgba(&shot, &list)?;
                    off.wait_for_pipeline_warmup();
                }
                let raw = off.rgba(&shot, &list)?;
                if !sky_pixels.is_empty() {
                    let report = off.sky_pixel_probe(17, &sky_pixels, scene, None)?;
                    std::fs::write(out.join(format!("{name}-sky-{}.txt", if measure { "first" } else { "repeat" })), report)?;
                }
                if measure && profile == "bloom" && lightbar {
                    let mask = off.emissive_seed_rgba()?.context("Bloom emission mask")?;
                    image::save_buffer(out.join(format!("{name}-emissive-mask.png")),
                        &mask, size.0, size.1, image::ColorType::Rgba8)?;
                }
                if measure && std::env::var_os("AOMAC_GFX_FRAME_TIMING").is_some() {
                    off.prepare_frozen_rendering();
                    off.set_gpu_timing(true)?;
                    let mut frames = Vec::with_capacity(600);
                    let camera_pan = std::env::var_os("AOMAC_GFX_TIMING_PAN").is_some();
                    for sample in 0..720 {
                        if camera_pan { off.host.camera.yaw = fixed.yaw + (sample as f32 * 0.02).sin() * 0.1; }
                        off.render_frozen()?;
                        let ms = off.read_frame_gpu_ms()?.context("frame GPU timer did not record")?;
                        if sample >= 120 { frames.push(ms); }
                    }
                    off.set_gpu_timing(false)?;
                    if camera_pan {
                        let motion = off.frozen_rgba()?;
                        image::save_buffer(out.join(format!("{name}-motion.png")),
                            &motion, size.0, size.1, image::ColorType::Rgba8)?;
                    }
                    frames.sort_by(f64::total_cmp);
                    let percentiles = [0.50_f64, 0.95, 0.99].map(|p| frames[(p * frames.len() as f64).ceil() as usize - 1]);
                    std::fs::write(out.join(format!("{name}-frame-gpu.txt")),
                        format!("profile={profile}\npost_features={features:?}\nmsaa_requested={msaa}\nmsaa_actual={actual_msaa}\ncamera_pan={camera_pan}\nresolution=1920x1200\nwarmup=120\nsamples=600\nunits=milliseconds\nframe_p50_p95_p99={percentiles:?}\n"))?;
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
            writeln!(manifest, "{name} environment={:?} ground_light={:?} sun_authored={} daylight={:?}", scene.environment, scene.ground_light, scene.sun_authored, scene.daylight)?;
            if settings.night_mode == ao_render::NightMode::Moonlit {
                writeln!(manifest, "{name} enhanced_moon_sources={:?}", scene.moons)?;
            }
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
                std::fs::write(out.join("manifest.txt"), &manifest)?;
                ensure!(count == 0 && maximum == 0,
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
    views.push(("icc-shadow-edges-authored".into(), 4582, 2648.69,
        Camera::look_at([922.0, 26.0, -750.0].into(), [905.0, 23.0, -770.0].into())));
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
    // Newland camera heights here are terrain-relative offsets; resolve them
    // against the snapshot's collision before capture and manifest output.
    for (light, time) in [("day", 3240.0), ("dusk", 4860.0), ("night", 0.0)] {
        views.push((format!("newland-{light}"), 566, time,
            Camera::look_at([414.0, 2.5, -320.0].into(), [440.0, 1.0, -335.0].into())));
    }
    views.push(("newland-backlit-day".into(), 566, 3240.0,
        Camera::look_at([414.0, 2.5, -320.0].into(), [440.0, 1.0, -335.0].into())));
    for (name, pf, eye, at) in [
        ("borealis-foliage-ground-sun-high", 800, [652.0, 65.3, -420.0], [651.8296, 73.42179, -428.4731]),
        ("newland-foliage-ground-day", 566, [414.0, 2.5, -320.0], [440.0, 1.0, -335.0]),
    ] {
        views.push((name.into(), pf, 3240.0, Camera::look_at(eye.into(), at.into())));
    }
    views.push(("borealis-lightbar-night".into(), 800, 0.0,
        Camera::look_at([679.6, 74.5, -476.7].into(), [679.6, 74.0, -482.7].into())));
    views.push(("borealis-lightbar-night-close".into(), 800, 0.0,
        Camera::look_at([679.6, 74.0, -480.2].into(), [679.6, 74.0, -482.7].into())));
    views
}

fn graphics_fixture_ground_foliage_camera(
    scene: &Scene,
    original: Camera,
    terrain_height: impl Fn([f32; 3]) -> Option<f32>,
    clear: impl Fn([f32; 3], [f32; 3]) -> bool,
    manifest: &mut String,
) -> anyhow::Result<Camera> {
    use std::fmt::Write;
    struct Selection {
        distance: f32,
        camera: Camera,
        instance: usize,
        submesh: usize,
        lo: Vec3,
        hi: Vec3,
        aim: Vec3,
        landing: [Vec3; 4],
    }
    let heading = Vec3::new(original.forward().x, 0.0, original.forward().z).normalize_or_zero();
    anyhow::ensure!(heading.length_squared() > 0.0, "ground foliage view has no horizontal heading");
    let vertical_tan = (camera::lens(scene.lens.unwrap_or_default(), false).vertical_fov(1920.0 / 1200.0) * 0.5).tan();
    let mut best: Option<Selection> = None;
    for (index, instance) in scene.instances.iter().enumerate() {
        let mesh = &scene.meshes[instance.mesh];
        let transform = glam::Mat4::from_cols_array_2d(&instance.transform);
        for (material, submesh) in mesh.submeshes.iter().enumerate() {
            if !submesh.vegetation || !submesh.lighting_enabled || !submesh.fog_enabled { continue; }
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for &vertex in &submesh.indices {
                let point = transform.transform_point3(Vec3::from(mesh.vertices[vertex as usize].pos));
                lo = lo.min(point); hi = hi.max(point);
            }
            if !lo.is_finite() || !hi.is_finite() || hi.y - lo.y < 2.5 { continue; }
            let center = (lo + hi) * 0.5;
            let distance = center.distance_squared(original.pos);
            if distance > 80.0 * 80.0 || best.as_ref().is_some_and(|best| distance >= best.distance) { continue; }
            let mut landing = [
                Vec3::new(lo.x, 0.0, lo.z), Vec3::new(hi.x + 5.0, 0.0, lo.z),
                Vec3::new(lo.x, 0.0, hi.z + 5.0), Vec3::new(hi.x + 5.0, 0.0, hi.z + 5.0),
            ];
            let mut grounded = true;
            for point in &mut landing {
                if let Some(height) = terrain_height(point.to_array()).filter(|height| height.is_finite()) {
                    point.y = height;
                } else { grounded = false; break; }
            }
            if !grounded { continue; }
            let ground_low = landing.iter().map(|point| point.y).fold(f32::INFINITY, f32::min);
            let ground_high = landing.iter().map(|point| point.y).fold(f32::NEG_INFINITY, f32::max);
            let crown = Vec3::new(center.x, lo.y + (hi.y - lo.y) * 0.75, center.z);
            let aim = Vec3::new(center.x + 2.5, ground_low + (hi.y - ground_low) * 0.35, center.z + 2.5);
            let retreat = ((hi.y - ground_low) * 1.5).max((hi.x - lo.x).max(hi.z - lo.z) + 5.0);
            for scale in [1.0, 1.3, 1.7, 2.2] {
                let mut eye = aim - heading * retreat * scale;
                let Some(ground) = terrain_height(eye.to_array()).filter(|height| height.is_finite()) else { continue; };
                eye.y = ground + 1.7;
                let camera = Camera::look_at(eye, aim);
                let forward = camera.forward();
                let right = forward.cross(Vec3::Y).normalize_or_zero();
                let up = right.cross(forward);
                let fits = |point: Vec3| {
                    let delta = point - eye;
                    let depth = delta.dot(forward);
                    depth > 0.2 && delta.dot(right).abs() < depth * 0.85
                        && delta.dot(up).abs() < depth * vertical_tan * 0.85
                };
                let mut fits_crown = true;
                for x in [lo.x, hi.x] {
                    for y in [lo.y, hi.y] {
                        for z in [lo.z, hi.z] { fits_crown &= fits(Vec3::new(x, y, z)); }
                    }
                }
                let ground_center = Vec3::new(aim.x, ground_high, aim.z) - eye;
                let ground_ndc = ground_center.dot(up) / (ground_center.dot(forward) * vertical_tan);
                if !fits_crown || !landing.iter().all(|point| fits(*point))
                    || !(-0.85..=-0.25).contains(&ground_ndc)
                    || !clear(eye.to_array(), crown.to_array())
                    || !landing.iter().all(|point| clear(eye.to_array(), (*point + Vec3::Y * 0.15).to_array())) { continue; }
                best = Some(Selection { distance, camera, instance: index, submesh: material, lo, hi, aim, landing });
                break;
            }
        }
    }
    let selected = best.ok_or_else(|| anyhow::anyhow!("no nearby native tall canopy fits crown and collision-grounded fall footprint with clear LOS"))?;
    writeln!(manifest, "ground_foliage instance={} mesh={} submesh={} world_lo={:?} world_hi={:?} eye={:?} aim={:?} landing_scene={:?} source=native_transformed_canopy_and_collision_terrain heading=existing_view drift_extent_positive_xz=5 ground_margin=lower_third collision_los=clear",
        selected.instance, scene.instances[selected.instance].mesh, selected.submesh, selected.lo, selected.hi,
        selected.camera.pos, selected.aim, selected.landing)?;
    Ok(selected.camera)
}

fn graphics_fixture_shrub(
    scene: &Scene,
    anchor: Vec3,
    names: &ao_formats::character::NameTable,
    collision: &ao_formats::playfield::collision::Collision,
    manifest: &mut String,
) -> anyhow::Result<(Vec3, Camera)> {
    use std::fmt::Write;
    let mut best: Option<(f32, Vec3, Camera, usize, usize)> = None;
    for (index, instance) in scene.instances.iter().enumerate() {
        let mesh = &scene.meshes[instance.mesh];
        let transform = glam::Mat4::from_cols_array_2d(&instance.transform);
        for (material, submesh) in mesh.submeshes.iter().enumerate().filter(|(_, submesh)| submesh.vegetation) {
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for &vertex in &submesh.indices {
                let point = transform.transform_point3(Vec3::from(mesh.vertices[vertex as usize].pos));
                lo = lo.min(point);
                hi = hi.max(point);
            }
            if !lo.is_finite() || !hi.is_finite() { continue; }
            let mut root = (lo + hi) * 0.5;
            let distance = Vec3::new(root.x - anchor.x, 0.0, root.z - anchor.z).length_squared();
            if distance > 80.0 * 80.0 { continue; }
            let texture = submesh.texture.and_then(|key| names.name(key.rdb_type, key.id));
            let height = hi.y - lo.y;
            writeln!(manifest, "interaction_candidate instance={index} mesh={} submesh={material} texture={texture:?} world_lo={lo:?} world_hi={hi:?} distance={} height={height}", instance.mesh, distance.sqrt())?;
            if !(0.5..=3.0).contains(&height) || !texture.is_some_and(graphics_fixture_shrub_texture) { continue; }
            let Some(ground) = collision.terrain_height(root.to_array()) else { continue; };
            root.y = ground;
            let ground = |point| collision.terrain_height(point);
            let (Ok(start), Ok(end)) = (
                graphics_fixture_sequence_position(root, 0.0, ground),
                graphics_fixture_sequence_position(root, 0.5, ground),
            ) else { continue; };
            if collision.line((start + Vec3::Y * 0.8).to_array(),
                (end + Vec3::Y * 0.8).to_array()).is_some() { continue; }
            let mut eye = root + Vec3::new(-5.0, 0.0, 4.0);
            let Some(ground) = collision.terrain_height(eye.to_array()) else { continue; };
            eye.y = ground + 1.8;
            let at = Vec3::new(root.x, (lo.y + height * 0.6).max(root.y + 0.5), root.z);
            if collision.line(eye.to_array(), at.to_array()).is_some() { continue; }
            if best.as_ref().is_none_or(|best| distance < best.0) {
                best = Some((distance, root, Camera::look_at(eye, at), index, material));
            }
        }
    }
    let (_, root, camera, instance, submesh) = best.ok_or_else(|| anyhow::anyhow!(
        "no native leafy shrub (world height0.5..3m, not grass/flower, grounded clear LOS) within80m; see interaction_candidate inventory in manifest"))?;
    writeln!(manifest, "interaction_selected instance={instance} mesh={} submesh={submesh} root={root:?} root_source=native_leaf_world_bounds_center_grounded camera={camera:?} collision_los=clear", scene.instances[instance].mesh)?;
    Ok((root, camera))
}

fn graphics_fixture_shrub_texture(name: &str) -> bool {
    let contains = |word: &str| name.as_bytes().windows(word.len())
        .any(|window| window.eq_ignore_ascii_case(word.as_bytes()));
    !["grass", "flower"].iter().any(|word| contains(word))
        && ["leaf", "leav", "plant", "bush", "palm"].iter().any(|word| contains(word))
}

#[test]
fn graphics_fixture_shrub_semantics_exclude_tufts_and_flowers() {
    for name in ["leaves_hires", "plant_billboards_1024_lars2", "palmblad", "Bush_A"] {
        assert!(graphics_fixture_shrub_texture(name));
    }
    for name in ["dry_edge_grass_small", "grass_plant", "flower_leaves", "tree_trunk", "unknown"] {
        assert!(!graphics_fixture_shrub_texture(name));
    }
}

fn graphics_fixture_backlit_camera(
    scene: &Scene,
    anchor: Vec3,
    terrain_height: impl Fn([f32; 3]) -> Option<f32>,
    clear: impl Fn([f32; 3], [f32; 3]) -> bool,
) -> anyhow::Result<(Camera, usize, usize, Vec3, f32)> {
    use anyhow::Context;
    let sun = Vec3::from(scene.environment.context("backlit view has no authored environment")?.sun_dir)
        .normalize_or_zero();
    anyhow::ensure!(sun.y > 0.0, "backlit view requires an above-horizon authored sun");
    let mut best: Option<(f32, Camera, usize, usize, Vec3, f32)> = None;
    for (index, instance) in scene.instances.iter().enumerate() {
        let mesh = &scene.meshes[instance.mesh];
        let transform = glam::Mat4::from_cols_array_2d(&instance.transform);
        for (submesh_index, submesh) in mesh.submeshes.iter().enumerate() {
            if !submesh.vegetation || !submesh.lighting_enabled || !submesh.fog_enabled
                || submesh.depth_write == Some(false)
                || (submesh.depth_write.is_none() && !matches!(submesh.blend, ao_scene::Blend::Opaque | ao_scene::Blend::AlphaTest)) {
                continue;
            }
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for &vertex in &submesh.indices {
                let point = transform.transform_point3(Vec3::from(mesh.vertices[vertex as usize].pos));
                lo = lo.min(point);
                hi = hi.max(point);
            }
            if !lo.is_finite() || !hi.is_finite() || hi.y - lo.y < 2.5 { continue; }
            let canopy = Vec3::new((lo.x + hi.x) * 0.5, lo.y + (hi.y - lo.y) * 0.75, (lo.z + hi.z) * 0.5);
            let distance = canopy.distance_squared(anchor);
            if distance > 80.0 * 80.0 || best.as_ref().is_some_and(|best| distance >= best.0) { continue; }
            for offset in [4.0, 8.0, 12.0] {
                let mut eye = canopy - sun * offset;
                let Some(ground) = terrain_height(eye.to_array()) else { continue; };
                eye.y = ground + 1.8;
                let camera = Camera::look_at(eye, canopy);
                let alignment = camera.forward().dot(sun);
                if !eye.is_finite() || alignment < 0.6 || !clear(eye.to_array(), canopy.to_array()) { continue; }
                best = Some((distance, camera, index, submesh_index, canopy, alignment));
                break;
            }
        }
    }
    best.map(|(_, camera, index, submesh, canopy, alignment)| (camera, index, submesh, canopy, alignment))
        .context("no nearby native canopy has a grounded, unobstructed toward-sun camera")
}

#[test]
fn graphics_fixture_backlit_view_requires_native_canopy_ground_and_los() {
    let mut material = ao_scene::Submesh::new(vec![0, 1], None);
    material.vegetation = true;
    let mut scene = Scene {
        environment: Some(ao_scene::Environment {
            sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 0.0, fog_end: 100.0,
            ambient: [0.0; 3], sun_color: [1.0; 3],
            sun_dir: [std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2, 0.0],
            sun_specular: 1.0,
        }),
        meshes: vec![ao_scene::Mesh {
            vertices: vec![
                ao_scene::Vertex { pos: [-1.0, 2.0, -1.0], ..Default::default() },
                ao_scene::Vertex { pos: [1.0, 6.0, 1.0], ..Default::default() },
            ],
            submeshes: vec![material],
        }],
        instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
        ..Default::default()
    };
    let (camera, instance, submesh, canopy, alignment) = graphics_fixture_backlit_camera(
        &scene, Vec3::ZERO, |_| Some(0.0), |_, _| true).unwrap();
    assert_eq!(instance, 0);
    assert_eq!(submesh, 0);
    assert_eq!(canopy, Vec3::new(0.0, 5.0, 0.0));
    assert_eq!(camera.pos.y, 1.8);
    assert!(alignment >= 0.6);
    let original = Camera::look_at(Vec3::new(0.0, 1.7, 15.0), Vec3::new(0.0, 4.0, 0.0));
    let mut report = String::new();
    let ground_camera = graphics_fixture_ground_foliage_camera(
        &scene, original, |_| Some(0.0), |_, _| true, &mut report).unwrap();
    assert_eq!(ground_camera.pos.y, 1.7);
    assert!(report.contains("landing_scene=") && report.contains("instance=0"));
    assert!(graphics_fixture_ground_foliage_camera(
        &scene, original, |_| None, |_, _| true, &mut String::new()).is_err());
    assert!(graphics_fixture_ground_foliage_camera(
        &scene, original, |_| Some(0.0), |_, _| false, &mut String::new()).is_err());
    assert!(graphics_fixture_backlit_camera(&scene, Vec3::ZERO, |_| None, |_, _| true).is_err());
    assert!(graphics_fixture_backlit_camera(&scene, Vec3::ZERO, |_| Some(0.0), |_, _| false).is_err());
    scene.instances[0].transform = glam::Mat4::from_translation(Vec3::new(20.0, 10.0, 30.0)).to_cols_array_2d();
    let (_, _, _, transformed, _) = graphics_fixture_backlit_camera(
        &scene, Vec3::ZERO, |_| Some(10.0), |_, _| true).unwrap();
    assert_eq!(transformed, Vec3::new(20.0, 15.0, 30.0));
    scene.instances[0].transform = ao_scene::IDENTITY;
    scene.meshes[0].submeshes[0].lighting_enabled = false;
    assert!(graphics_fixture_backlit_camera(&scene, Vec3::ZERO, |_| Some(0.0), |_, _| true).is_err());
    scene.meshes[0].submeshes[0].lighting_enabled = true;
    scene.meshes[0].submeshes[0].fog_enabled = false;
    assert!(graphics_fixture_backlit_camera(&scene, Vec3::ZERO, |_| Some(0.0), |_, _| true).is_err());
    scene.meshes[0].submeshes[0].fog_enabled = true;
    scene.meshes[0].submeshes[0].depth_write = Some(false);
    assert!(graphics_fixture_backlit_camera(&scene, Vec3::ZERO, |_| Some(0.0), |_, _| true).is_err());
    scene.meshes[0].submeshes[0].depth_write = None;
    scene.meshes[0].submeshes[0].vegetation = false;
    assert!(graphics_fixture_backlit_camera(&scene, Vec3::ZERO, |_| Some(0.0), |_, _| true).is_err());
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GraphicsFixtureSequence {
    interaction: bool,
    start: f32,
    dt: f32,
    count: usize,
}

fn graphics_fixture_sequence(
    mode: Option<&str>, start: Option<&str>, dt: Option<&str>, count: Option<&str>,
) -> anyhow::Result<Option<GraphicsFixtureSequence>> {
    use anyhow::ensure;
    let Some(mode) = mode else {
        ensure!(start.is_none() && dt.is_none() && count.is_none(),
            "AOMAC_GFX_SEQUENCE_START/DT/COUNT require AOMAC_GFX_SEQUENCE");
        return Ok(None);
    };
    ensure!(matches!(mode, "stationary" | "interaction"),
        "AOMAC_GFX_SEQUENCE must be stationary or interaction");
    let start = start.unwrap_or("2").parse::<f32>()?;
    let dt = dt.unwrap_or("0.5").parse::<f32>()?;
    let count = count.unwrap_or("6").parse::<usize>()?;
    ensure!(start.is_finite() && (0.0..=10.0).contains(&start),
        "AOMAC_GFX_SEQUENCE_START must be finite seconds in 0..=10");
    ensure!(dt.is_finite() && (0.05..=2.0).contains(&dt),
        "AOMAC_GFX_SEQUENCE_DT must be finite sample spacing in 0.05..=2 seconds");
    ensure!((5..=8).contains(&count), "AOMAC_GFX_SEQUENCE_COUNT must be 5..=8");
    Ok(Some(GraphicsFixtureSequence { interaction: mode == "interaction", start, dt, count }))
}

fn graphics_fixture_sequence_position(
    root: Vec3,
    progress: f32,
    terrain_height: impl Fn([f32; 3]) -> Option<f32>,
) -> anyhow::Result<Vec3> {
    use anyhow::Context;
    // Cross the selected native shrub, leave its bend radius in the first half,
    // then remain admitted at the departure position while its history rebounds.
    let mut point = root + Vec3::X * (-3.0 + 6.0 * (progress * 2.0).clamp(0.0, 1.0));
    point.y = terrain_height(point.to_array()).context("sequence path has no terrain height")?;
    anyhow::ensure!(point.is_finite(), "sequence path has invalid terrain height");
    Ok(point)
}

#[test]
fn graphics_fixture_sequence_controls_and_grounded_rebound() {
    assert_eq!(graphics_fixture_sequence(None, None, None, None).unwrap(), None);
    let defaults = graphics_fixture_sequence(Some("stationary"), None, None, None).unwrap().unwrap();
    assert_eq!((defaults.start, defaults.dt, defaults.count, defaults.interaction), (2.0, 0.5, 6, false));
    let interaction = graphics_fixture_sequence(Some("interaction"), Some("0"), Some("2"), Some("8")).unwrap().unwrap();
    assert!(interaction.interaction);
    for (mode, start, dt, count) in [
        (None, Some("1"), None, None),
        (Some("wind"), None, None, None),
        (Some("stationary"), Some("NaN"), None, None),
        (Some("stationary"), Some("-1"), None, None),
        (Some("stationary"), Some("11"), None, None),
        (Some("stationary"), None, Some("inf"), None),
        (Some("stationary"), None, Some("0"), None),
        (Some("stationary"), None, Some("3"), None),
        (Some("stationary"), None, None, Some("4")),
        (Some("stationary"), None, None, Some("9")),
        (Some("stationary"), None, None, Some("six")),
    ] {
        assert!(graphics_fixture_sequence(mode, start, dt, count).is_err());
    }
    let ground = |point: [f32; 3]| Some(point[0] * 0.1);
    let root = Vec3::new(30.0, 0.0, -20.0);
    let start = graphics_fixture_sequence_position(root, 0.0, ground).unwrap();
    let contact = graphics_fixture_sequence_position(root, 0.25, ground).unwrap();
    let leave = graphics_fixture_sequence_position(root, 0.5, ground).unwrap();
    let rebound = graphics_fixture_sequence_position(root, 1.0, ground).unwrap();
    assert_eq!(contact.x, root.x);
    assert!((contact.x - start.x - 3.0).abs() < 0.001);
    assert!((leave.x - contact.x - 3.0).abs() < 0.001);
    assert_eq!(leave, rebound);
    assert_eq!(contact.y, contact.x * 0.1);
    assert!(graphics_fixture_sequence_position(root, 0.0, |_| None).is_err());
    assert!(graphics_fixture_sequence_position(root, 0.0, |_| Some(f32::NAN)).is_err());
}

fn graphics_fixture_newland_camera(terrain_height: impl Fn([f32; 3]) -> Option<f32>) -> anyhow::Result<Camera> {
    use anyhow::Context;
    let mut eye = [414.0, 2.5, -320.0];
    let mut at = [440.0, 1.0, -335.0];
    eye[1] += terrain_height(eye).context("Newland street camera eye has no terrain height")?;
    at[1] += terrain_height(at).context("Newland street camera target has no terrain height")?;
    Ok(Camera::look_at(eye.into(), at.into()))
}

fn graphics_fixture_interaction_path(
    camera: Camera,
    endpoint: Option<Vec3>,
    terrain_height: impl Fn([f32; 3]) -> Option<f32>,
) -> anyhow::Result<(Vec3, Vec3)> {
    use anyhow::Context;
    let mut end = endpoint.unwrap_or_else(|| camera.pos + camera.forward() * 3.0 - Vec3::Y);
    let mut start = end - Vec3::X;
    start.y = terrain_height(start.to_array()).context("interaction start has no terrain height")?;
    end.y = terrain_height(end.to_array()).context("interaction end has no terrain height")?;
    Ok((start, end))
}

#[test]
fn graphics_fixture_interaction_path_grounds_each_endpoint() {
    let camera = Camera::look_at([10.0, 80.0, -20.0].into(), [10.0, 80.0, -30.0].into());
    let (start, end) = graphics_fixture_interaction_path(camera, None, |point| match (point[0], point[2]) {
        (9.0, -23.0) => Some(4.0),
        (10.0, -23.0) => Some(6.0),
        _ => None,
    }).unwrap();
    assert_eq!(start, Vec3::new(9.0, 4.0, -23.0));
    assert_eq!(end, Vec3::new(10.0, 6.0, -23.0));
    let endpoint = Vec3::new(436.432_34, 25.000_1, -320.404_08);
    let (start, end) = graphics_fixture_interaction_path(camera, Some(endpoint),
        |point| (point[2] == endpoint.z).then_some(point[0])).unwrap();
    assert_eq!(start, Vec3::new(endpoint.x - 1.0, endpoint.x - 1.0, endpoint.z));
    assert_eq!(end, Vec3::new(endpoint.x, endpoint.x, endpoint.z));
    assert!(graphics_fixture_interaction_path(camera, None, |_| None).is_err());
    assert!(graphics_fixture_interaction_path(camera, None,
        |point| (point[0] == 9.0).then_some(4.0)).is_err());
}

#[test]
fn graphics_fixture_newland_camera_uses_both_ground_heights() {
    let camera = graphics_fixture_newland_camera(|point| match (point[0], point[2]) {
        (414.0, -320.0) => Some(7.0),
        (440.0, -335.0) => Some(9.0),
        _ => None,
    }).unwrap();
    let expected = Camera::look_at([414.0, 9.5, -320.0].into(), [440.0, 10.0, -335.0].into());
    assert_eq!(camera.pos, expected.pos);
    assert_eq!((camera.yaw, camera.pitch, camera.roll), (expected.yaw, expected.pitch, expected.roll));
    assert!(graphics_fixture_newland_camera(|_| None).is_err());
    assert!(graphics_fixture_newland_camera(|point| (point[0] == 414.0).then_some(7.0)).is_err());
}

#[test]
fn graphics_fixture_snapshot_keys_share_only_matching_playfield_and_time() -> anyhow::Result<()> {
    let views = graphics_fixture_views(&graphics_fixture_actor()?);
    let mut keys = std::collections::HashMap::<_, usize>::new();
    for (_, pf, time, _) in &views {
        *keys.entry((*pf, time.to_bits())).or_default() += 1;
    }
    assert_eq!(views.len(), 21);
    assert_eq!(keys.len(), 9);
    assert_eq!(keys[&(4582, 2648.69_f32.to_bits())], 4);
    assert_eq!(keys[&(800, 0.0_f32.to_bits())], 4);
    assert_eq!(keys[&(800, 3240.0_f32.to_bits())], 3);
    assert_eq!(keys[&(127, 3240.0_f32.to_bits())], 1);
    for time in [3240.0_f32, 4860.0, 0.0] {
        assert_eq!(keys[&(566, time.to_bits())], if time == 3240.0 { 3 } else { 1 });
    }
    Ok(())
}

/// Night lighting may exceed faithful; fog extinction and non-admitted actor isolation may not.
#[test]
#[ignore = "offline night fog/admission regression; requires installed client and a new AOMAC_GFX_OUT"]
fn graphics_offline_night_preserves_fog_and_hidden_actor_isolation() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::fmt::Write;
    let out = PathBuf::from(std::env::var_os("AOMAC_GFX_OUT").context("set AOMAC_GFX_OUT to a new handoff directory")?);
    ensure!(!out.exists(), "refusing to overwrite evidence {}", out.display());
    std::fs::create_dir_all(&out)?;
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let weather = ao_formats::weather::State::clear();
    let size = (1920, 1200);
    let actor = graphics_fixture_actor()?;
    let nights: Vec<_> = graphics_fixture_views(&actor).into_iter().filter(|(name, _, time, _)| {
        *time == 0.0 && matches!(name.as_str(), "newland-night" | "borealis-foliage-night")
    }).collect();
    ensure!(nights.len() == 2, "the night-view set changed");
    let mut report = String::from("rule=night lighting may brighten; native fog must extinguish body colour; non-admitted actors must equal absence\n");
    for (name, pf, time, fixed) in nights {
        ao_scene::crt_srand(1);
        let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(&store, &dir, pf, time, 0, Some(weather))?;
        ensure!(scene.daylight == Some(0.0), "{name}: not an authored-night snapshot");
        let environment = scene.environment.as_mut().context("night environment missing")?;
        environment.fog_start = -2.0;
        environment.fog_end = -1.0;
        scene.fog_model = None;
        // This controlled extinction fixture tests fogged surfaces, not authored
        // no-fog materials, whose visible surface illumination may change.
        for mesh in &mut scene.meshes {
            for material in &mut mesh.submeshes { material.fog_enabled = true; }
        }
        let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
        super::player::bake_sky_open(&collision, &mut scene);
        let fixed = if pf == 566 {
            graphics_fixture_newland_camera(|point| collision.terrain_height(point))?
        } else { fixed };
        let exposure = super::player::grass_exposure_surface(std::rc::Rc::new(std::cell::RefCell::new(collision)));
        let mut avatar = super::avatar::Avatar::new(&store, &dir, 25988, &actor,
            ao_formats::character::actor::ActorAssets::new(&store)?)?;
        let position = fixed.pos + fixed.forward() * 3.0 - Vec3::Y;
        avatar.set_transform(position.to_array(), 0.0);
        // Keep real captured body geometry but remove material fog exemptions and textures:
        // two saturated body colours provide a positive control for the extinction comparison.
        let render = |profile: &str, fog: bool, body: u8| -> anyhow::Result<Vec<u8>> {
            let mut settings = graphics_fixture_settings(profile)?;
            if settings.night_mode == ao_render::NightMode::Moonlit { settings.moon_intensity = 4.0; }
            if settings.night_mode == ao_render::NightMode::Dark { settings.dark_night_strength = 1.0; }
            let mut snapshot = scene.clone();
            snapshot.fog_model = None;
            let environment = snapshot.environment.as_mut().context("night fixture has no authored environment")?;
            environment.fog_start = if fog { 0.0 } else { 10_000.0 };
            environment.fog_end = if fog { 0.01 } else { 20_000.0 };
            ao_scene::crt_srand(1);
            let mut lens = camera::lens(snapshot.lens.unwrap_or_default(), false);
            lens.far = Some(camera::far_plane(camera::VIEW_DISTANCE));
            let mut shot = Shot { gui: Gui::new(&dir, None)?, scene: Some(snapshot), camera: fixed, lens };
            let mut off = Offscreen::evidence(&shot, size)?;
            off.fixed_camera(Some(fixed));
            off.host.msaa_samples = Some(1);
            off.host.anisotropy = Some(1);
            off.host.fog_density_scale = Some(1.0);
            off.host.enhanced_settings = Some(settings);
            off.host.grass_exposure = Some(exposure.clone());
            off.host.grass_wind = weather.wind;
            off.frame(&mut shot, 0.0);
            if body != 0 {
                let mut model = avatar.model().clone();
                for mesh in &mut model.meshes {
                    for material in &mut mesh.submeshes {
                        material.texture = None;
                        material.base_color = if body == 3 { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] };
                        material.fog_enabled = true;
                        material.overlay = false;
                        material.emissive = [1.0; 3];
                        material.specular = [0.0; 3];
                    }
                }
                off.host.actor_models.push((super::avatar::MODEL_KEY, std::sync::Arc::new(model)));
                let mut frame = avatar.frame();
                frame.id = 25988;
                frame.emissive = None;
                frame.part_materials.clear();
                frame.alpha = if body == 1 { 0.0 } else { 1.0 };
                off.host.grass_benders.push(ao_render::GrassBender { actor: Some(frame.id), position: position.to_array() });
                off.host.actors.push(frame);
            }
            let list = off.frame(&mut shot, 0.0);
            if settings.ground_grass { off.warm_grass_tiles(); }
            if settings.foliage_leaves || settings.foliage_flutter || settings.foliage_falling {
                off.rgba(&shot, &list)?;
                off.wait_for_pipeline_warmup();
            }
            off.rgba(&shot, &list)
        };
        for profile in ["faithful", "grass", "all-no-bloom", "all-foliage", "moonlit", "moon-shadows", "dark-night", "lighting"] {
            let absent = render(profile, false, 0)?;
            let hidden = render(profile, false, 1)?;
            ensure!(absent == hidden, "{name}/{profile}: zero-alpha non-admitted body or its bender changed the image");
            let red = render(profile, false, 2)?;
            let green = render(profile, false, 3)?;
            ensure!(red != green, "{name}/{profile}: body-colour positive control is not visible");
            let fog_red = render(profile, true, 2)?;
            let fog_green = render(profile, true, 3)?;
            ensure!(fog_red == fog_green, "{name}/{profile}: fully fogged body colour leaked through extinction");
            for (suffix, raw) in [("absent", &absent), ("hidden", &hidden), ("red", &red), ("green", &green), ("fog-red", &fog_red), ("fog-green", &fog_green)] {
                image::save_buffer(out.join(format!("{name}-{profile}-{suffix}.png")), raw, size.0, size.1, image::ColorType::Rgba8)?;
            }
            writeln!(report, "{name} profile={profile} non_admitted_equals_absent=true unfogged_colour_control_differs=true fully_fogged_colours_equal=true")?;
            std::fs::write(out.join("night-fog-admission.txt"), &report)?;
        }
    }
    std::fs::write(out.join("status.txt"), "status=pass\n")?;
    Ok(())
}

fn graphics_fixture_settings(profile: &str) -> anyhow::Result<ao_render::EnhancedSettings> {
    let mut settings = ao_render::EnhancedSettings::default();
    match profile {
        "faithful" => {}
        "neutral" => { settings.ground_grass = true; settings.grass_density = 0.0; }
        "shadows" => settings.sun_shadows = true,
        "clustered" => settings.clustered_lighting = true,
        "local-shadows" => settings.local_light_shadows = true,
        "local-shadows-clustered" => { settings.clustered_lighting = true; settings.local_light_shadows = true; }
        "moon-shadows" => { settings.night_mode = ao_render::NightMode::Moonlit; settings.moon_shadows = true; }
        "moonlit" => settings.night_mode = ao_render::NightMode::Moonlit,
        "dark-night" => settings.night_mode = ao_render::NightMode::Dark,
        "specular" => settings.material_specular = true,
        "lighting" => { settings.clustered_lighting = true; settings.local_light_shadows = true; settings.moon_shadows = true; settings.material_specular = true; settings.night_mode = ao_render::NightMode::Moonlit; }
        // Pre-soft-shadow look: 1024 maps, hard 3x3 PCF, full-strength shadow.
        "shadows-hard" => {
            settings.sun_shadows = true;
            (settings.shadow_map_size, settings.shadow_softness, settings.shadow_strength) = (1024, 0.0, 1.0);
        }
        "shadow-edges" | "shadow-edges-hdr" | "shadow-edges-hdr-only" => {
            settings.sun_shadows = profile != "shadow-edges-hdr-only";
            settings.hdr = profile != "shadow-edges";
            (settings.shadow_map_size, settings.shadow_strength, settings.shadow_softness) = (4096, 0.45, 1.5);
            (settings.sun_boost, settings.grade_strength) = (0.5, 0.8);
        }
        "ao" => settings.ambient_occlusion = true,
        "bloom" => settings.emissive_bloom = true,
        "grass" => settings.ground_grass = true,
        "foliage-edges" => settings.foliage_edges = true,
        "foliage-sway" => settings.foliage_sway = true,
        "foliage-push" => settings.foliage_push = true,
        "foliage-leaves" => settings.foliage_leaves = true,
        "foliage-flutter" => settings.foliage_flutter = true,
        "foliage-falling" => settings.foliage_falling = true,
        "grass-trails" => { settings.grass_trails = true; settings.ground_grass = true; }
        "foliage-subdivision" => settings.foliage_subdivision = true,
        "foliage-translucency" => settings.foliage_translucency = true,
        "foliage-lod" => settings.foliage_lod = true,
        "all-foliage" | "all-foliage-shadows" => {
            settings.sun_shadows = profile == "all-foliage-shadows";
            settings.ground_grass = true;
            settings.foliage_edges = true;
            settings.foliage_sway = true;
            settings.foliage_push = true;
            settings.foliage_leaves = true;
            settings.foliage_flutter = true;
            settings.foliage_falling = true;
            settings.grass_trails = true;
            settings.foliage_subdivision = true;
            settings.foliage_translucency = true;
            settings.foliage_lod = true;
        }
        "hdr" => settings.hdr = true,
        "hdr-bloom" => { settings.hdr = true; settings.emissive_bloom = true; }
        "hdr-bloom-max" => { settings.hdr = true; settings.emissive_bloom = true; settings.sun_boost = 0.6; }
        "hdr-max" => { settings.hdr = true; settings.sun_boost = 0.6; }
        "hdr-neutral" => { settings.hdr = true; settings.sun_boost = 0.0; settings.grade_strength = 0.0; }
        "contact" => settings.contact_shadows = true,
        "height_haze" => settings.height_haze = true,
        "land_haze" => settings.land_haze = true,
        "valley_mist" => settings.valley_mist = true,
        "volumetric_shafts" => settings.volumetric_shafts = true,
        "cloud_gap_shafts" => settings.cloud_gap_shafts = true,
        "volumetric_clouds" => settings.volumetric_clouds = true,
        "cloud_shadows" => settings.cloud_shadows = true,
        "sky_discs" => settings.sky_discs = true,
        "atmosphere" => {
            settings.height_haze = true;
            settings.land_haze = true;
            settings.valley_mist = true;
            settings.volumetric_shafts = true;
            settings.cloud_gap_shafts = true;
            settings.volumetric_clouds = true;
            settings.cloud_shadows = true;
            settings.sky_discs = true;
        }
        // Isolates the contact pass: shadows + hdr + contact vs `shadows-hdr`.
        "shadows-hdr" => { settings.sun_shadows = true; settings.hdr = true; }
        "shadows-hdr-contact" => { settings.sun_shadows = true; settings.hdr = true; settings.contact_shadows = true; }
        "all" | "all-no-bloom" => {
            settings.sun_shadows = true;
            settings.ambient_occlusion = true;
            settings.emissive_bloom = profile == "all";
            settings.ground_grass = true;
            settings.hdr = true;
            settings.contact_shadows = true;
            settings.height_haze = true;
            settings.land_haze = true;
            settings.valley_mist = true;
            settings.volumetric_shafts = true;
            settings.cloud_gap_shafts = true;
            settings.volumetric_clouds = true;
            settings.cloud_shadows = true;
            settings.sky_discs = true;
        }
        // 9f78245 all-on defaults, independent of subsequent tuning.
        "all-before" => settings = ao_render::EnhancedSettings {
            sun_shadows: true, ambient_occlusion: true, emissive_bloom: true, ground_grass: true,
            hdr: true, contact_shadows: true,
            shadow_map_size: 2048, shadow_strength: 0.65, shadow_softness: 1.5,
            ao_radius: 1.75, ao_strength: 1.5, bloom_strength: 0.6,
            grass_density: 1.0, grass_distance: 50.0, sun_boost: 0.5, grade_strength: 0.8, contact_strength: 0.4,
            ..Default::default()
        },
        _ => anyhow::bail!("unknown AOMAC_GFX_SETTINGS profile {profile:?}; use faithful|neutral|shadows|shadows-hard|shadow-edges|shadow-edges-hdr|shadow-edges-hdr-only|clustered|local-shadows|local-shadows-clustered|moonlit|moon-shadows|dark-night|specular|lighting|ao|bloom|grass|hdr|hdr-bloom|hdr-bloom-max|hdr-max|hdr-neutral|contact|shadows-hdr|shadows-hdr-contact|height_haze|land_haze|valley_mist|volumetric_shafts|cloud_gap_shafts|volumetric_clouds|cloud_shadows|sky_discs|atmosphere|all|all-no-bloom|all-before|foliage-edges|foliage-sway|foliage-push|foliage-leaves|foliage-flutter|foliage-falling|grass-trails|foliage-subdivision|foliage-translucency|foliage-lod|all-foliage|all-foliage-shadows"),
    }
    Ok(settings)
}

fn graphics_fixture_msaa(value: &str) -> anyhow::Result<u32> {
    let samples: u32 = value.parse()?;
    anyhow::ensure!([1, 2, 4, 8].contains(&samples), "AOMAC_GFX_MSAA must be 1, 2, 4 or 8");
    Ok(samples)
}

#[test]
fn graphics_fixture_msaa_selector_is_explicit_and_bounded() {
    for count in [1, 2, 4, 8] { assert_eq!(graphics_fixture_msaa(&count.to_string()).unwrap(), count); }
    for invalid in ["", "0", "3", "16", "-1", "invalid"] { assert!(graphics_fixture_msaa(invalid).is_err()); }
}

/// Independent effect selectors layer on any existing immutable scene/profile.
fn graphics_fixture_post_features(mut settings: ao_render::EnhancedSettings, features: &str) -> anyhow::Result<ao_render::EnhancedSettings> {
    if features.is_empty() { return Ok(settings); }
    for feature in features.split([',', '+']) {
        match feature {
            "sun_shadows" => settings.sun_shadows = true,
            "taa" => settings.taa = true,
            "fxaa" => settings.fxaa = true,
            "cas" => settings.cas = true,
            "motion_blur" => settings.motion_blur = 0.5,
            "dof" => settings.depth_of_field = true,
            "grading" => {
                settings.grading_enabled = true;
                settings.zone_lut = true;
                settings.vignette = 0.15;
                settings.contrast = 1.1;
                settings.saturation = 1.1;
                settings.warmth = 0.55;
                settings.night_cool = 0.2;
            }
            _ => anyhow::bail!("unknown AOMAC_GFX_FEATURE {feature:?}; use comma-separated taa,fxaa,cas,motion_blur,dof,grading,sun_shadows"),
        }
    }
    Ok(settings)
}

#[test]
fn graphics_fixture_post_selectors_are_independent_and_composable() {
    let defaults = ao_render::EnhancedSettings::default();
    assert_eq!(graphics_fixture_post_features(defaults, "").unwrap(), defaults);
    for feature in ["taa", "fxaa", "cas", "motion_blur", "dof", "grading"] {
        let s = graphics_fixture_post_features(defaults, feature).unwrap();
        assert_eq!([s.taa, s.fxaa, s.cas, s.motion_blur > 0.0, s.depth_of_field, s.grading_enabled],
            ["taa", "fxaa", "cas", "motion_blur", "dof", "grading"].map(|name| name == feature));
        assert!(!s.hdr && !s.sun_shadows && !s.ambient_occlusion && !s.ground_grass);
    }
    let s = graphics_fixture_post_features(defaults, "taa,fxaa,cas,motion_blur,dof,grading").unwrap();
    assert!(s.taa && s.fxaa && s.cas && s.motion_blur > 0.0 && s.depth_of_field && s.grading_enabled);
    assert!(graphics_fixture_post_features(defaults, "unknown").is_err());
    assert!(graphics_fixture_post_features(defaults, "taa,").is_err());
    let foliage = graphics_fixture_settings("all-foliage").unwrap();
    let combined = graphics_fixture_post_features(foliage, "taa,dof,grading").unwrap();
    assert_eq!(combined, ao_render::EnhancedSettings {
        taa: true, depth_of_field: true, grading_enabled: true, zone_lut: true,
        vignette: 0.15, contrast: 1.1, saturation: 1.1, warmth: 0.55, night_cool: 0.2,
        ..foliage
    });
    assert_eq!(graphics_fixture_post_features(foliage, "sun_shadows").unwrap(),
        ao_render::EnhancedSettings { sun_shadows: true, ..foliage });
}

#[test]
fn graphics_fixture_foliage_profiles_are_independent() {
    let all = graphics_fixture_settings("all-foliage").unwrap();
    let defaults = ao_render::EnhancedSettings::default();
    macro_rules! check {
        ($($name:literal => $field:ident),+ $(,)?) => {$(
            assert_eq!(graphics_fixture_settings($name).unwrap(),
                ao_render::EnhancedSettings { $field: true, ..defaults });
            assert!(all.$field);
        )+};
    }
    check!(
        "foliage-edges" => foliage_edges, "foliage-sway" => foliage_sway,
        "foliage-push" => foliage_push, "foliage-leaves" => foliage_leaves,
        "foliage-flutter" => foliage_flutter, "foliage-falling" => foliage_falling,
        "foliage-subdivision" => foliage_subdivision,
        "foliage-translucency" => foliage_translucency, "foliage-lod" => foliage_lod,
    );
    assert_eq!(graphics_fixture_settings("grass-trails").unwrap(),
        ao_render::EnhancedSettings { grass_trails: true, ground_grass: true, ..defaults });
    assert!(all.grass_trails && all.ground_grass);
    assert_eq!(graphics_fixture_settings("all-foliage-shadows").unwrap(),
        ao_render::EnhancedSettings { sun_shadows: true, ..all });
    assert!(!all.sun_shadows && !all.hdr
        && !all.ambient_occlusion && !all.emissive_bloom && !all.contact_shadows);
    for old in ["faithful", "neutral", "grass", "all", "all-no-bloom", "all-before"] {
        assert!(!graphics_fixture_settings(old).unwrap().foliage_enabled());
    }
}

#[test]
fn graphics_fixture_profiles_are_explicit_and_off_by_default() {
    assert!(!graphics_fixture_settings("faithful").unwrap().any_enabled());
    for name in ["clustered", "local-shadows", "local-shadows-clustered", "moonlit", "moon-shadows", "dark-night", "specular", "lighting"] {
        assert!(graphics_fixture_settings(name).unwrap().any_enabled());
    }
    let local = graphics_fixture_settings("local-shadows").unwrap();
    assert!(local.local_light_shadows && !local.clustered_lighting && !local.material_specular);
    let combined = graphics_fixture_settings("local-shadows-clustered").unwrap();
    assert!(combined.local_light_shadows && combined.clustered_lighting);
    assert_eq!(graphics_fixture_settings("moonlit").unwrap().night_mode, ao_render::NightMode::Moonlit);
    assert_eq!(graphics_fixture_settings("dark-night").unwrap().night_mode, ao_render::NightMode::Dark);
    let neutral = graphics_fixture_settings("neutral").unwrap();
    assert!(neutral.any_enabled() && neutral.grass_density == 0.0);
    let atmosphere_flags = |s: ao_render::EnhancedSettings| [
        s.height_haze, s.land_haze, s.valley_mist, s.volumetric_shafts,
        s.cloud_gap_shafts, s.volumetric_clouds, s.cloud_shadows, s.sky_discs,
    ];
    for (bit, name) in [
        "height_haze", "land_haze", "valley_mist", "volumetric_shafts",
        "cloud_gap_shafts", "volumetric_clouds", "cloud_shadows", "sky_discs",
    ].iter().enumerate() {
        let s = graphics_fixture_settings(name).unwrap();
        let mut expected = [false; 8];
        expected[bit] = true;
        assert_eq!(atmosphere_flags(s), expected);
        assert!(s.any_enabled());
        assert!(!s.sun_shadows && !s.ambient_occlusion && !s.emissive_bloom && !s.ground_grass && !s.hdr && !s.contact_shadows);
    }
    for name in ["atmosphere", "all", "all-no-bloom"] {
        assert_eq!(atmosphere_flags(graphics_fixture_settings(name).unwrap()), [true; 8]);
    }
    for name in ["faithful", "all-before"] {
        assert_eq!(atmosphere_flags(graphics_fixture_settings(name).unwrap()), [false; 8]);
    }
    for (name, expected) in [
        ("neutral", [false, false, false, true]), ("shadows", [true, false, false, false]),
        ("shadows-hard", [true, false, false, false]),
        ("ao", [false, true, false, false]), ("bloom", [false, false, true, false]),
        ("grass", [false, false, false, true]), ("all", [true; 4]), ("all-before", [true; 4]),
    ] {
        let s = graphics_fixture_settings(name).unwrap();
        assert_eq!([s.sun_shadows, s.ambient_occlusion, s.emissive_bloom, s.ground_grass], expected);
        assert_eq!((s.hdr, s.contact_shadows), (matches!(name, "all" | "all-before"), matches!(name, "all" | "all-before")));
    }
    for (name, shadows, hdr) in [
        ("shadow-edges", true, false), ("shadow-edges-hdr", true, true), ("shadow-edges-hdr-only", false, true),
    ] {
        let s = graphics_fixture_settings(name).unwrap();
        assert_eq!((s.sun_shadows, s.hdr), (shadows, hdr));
        assert!(!s.ambient_occlusion && !s.emissive_bloom && !s.ground_grass && !s.contact_shadows);
        assert_eq!((s.shadow_map_size, s.shadow_strength, s.shadow_softness), (4096, 0.45, 1.5));
        assert_eq!((s.sun_boost, s.grade_strength), (0.5, 0.8));
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
    assert_eq!(graphics_fixture_settings("all-no-bloom").unwrap(),
        ao_render::EnhancedSettings { emissive_bloom: false, ..graphics_fixture_settings("all").unwrap() });
    let before = graphics_fixture_settings("all-before").unwrap();
    assert_eq!((before.shadow_map_size, before.shadow_strength, before.shadow_softness), (2048, 0.65, 1.5));
    assert_eq!((before.ao_radius, before.ao_strength, before.bloom_strength), (1.75, 1.5, 0.6));
    assert_eq!((before.grass_density, before.grass_distance), (1.0, 50.0));
    assert_eq!((before.sun_boost, before.grade_strength, before.contact_strength), (0.5, 0.8, 0.4));
    let actor = graphics_fixture_actor().unwrap();
    assert_eq!(actor.name, "Testy");
    let views = graphics_fixture_views(&actor);
    assert_eq!(views.iter().map(|view| &view.0).collect::<std::collections::HashSet<_>>().len(), 21);
    for (light, expected_time) in [("day", 3240.0), ("dusk", 4860.0), ("night", 0.0)] {
        let newland = views.iter().find(|view| view.0 == format!("newland-{light}")).unwrap();
        assert_eq!((newland.1, newland.2), (566, expected_time));
        let city = Camera::look_at([414.0, 2.5, -320.0].into(), [440.0, 1.0, -335.0].into());
        assert_eq!(newland.3.pos, city.pos);
        assert_eq!((newland.3.yaw, newland.3.pitch, newland.3.roll), (city.yaw, city.pitch, city.roll));
    }
    assert_eq!(views.iter().filter(|view| view.0.starts_with("icc-actor-")).count(), 1);
    assert!(views.iter().filter(|view| view.1 == 4582).all(|view| view.0.ends_with("-authored") && view.2 == 2648.69));
    let beach = views.iter().find(|view| view.0 == "icc-shadow-edges-authored").unwrap();
    let expected = Camera::look_at([922.0, 26.0, -750.0].into(), [905.0, 23.0, -770.0].into());
    assert_eq!(beach.3.pos, expected.pos);
    assert_eq!((beach.3.yaw, beach.3.pitch, beach.3.roll), (expected.yaw, expected.pitch, expected.roll));
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
        let (mut scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
            &store, &dir, pf, time, 0, Some(weather))?;
        let collision = ao_formats::playfield::collision::Collision::load(&store, pf)?;
        super::player::bake_sky_open(&collision, &mut scene);
        let exposure = super::player::grass_exposure_surface(std::rc::Rc::new(std::cell::RefCell::new(collision)));
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
            if settings.foliage_enabled() || settings.interaction_enabled() { off.host.grass_exposure = Some(exposure.clone()); }
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
#[path = "bloom_seeds.rs"]
mod bloom_seeds;
