//! The sky objects of a playfield's tweak scripts, drawn camera-locked in `BackgroundSort` order (see `docs/formats.md`
//! § Sky): star dome / nebula (`Universe`), dot stars, moons, the atmosphere gradient, suns and flares (`e_SunRays`), thick
//! cloud dome, single clouds, the `Horizon` map and the Shadowlands vortex.
//!
//! Meshes use normalized far=1000; the renderer applies native far/1000 scale,
//! camera translation and the shared camera projection (including clipping).

use super::aurora;
use super::script::{self, Ctx, Obj, Quat};
use super::srgb_to_linear;
use crate::character::NameTable;
use crate::mesh::{decode_generic_mesh, MESH_TYPE};
use crate::texture::decode_texture;
use ao_rdb::RecordStore;
use ao_scene::{Blend, Instance, Mesh, Scene, SkyColors, SkySpin, SkyWaveSpin, Submesh, TextureKey, Vertex, IDENTITY};
use std::collections::HashMap;

const TEXTURES: u32 = 1_010_004;
/// Sort value of the atmosphere (`BackgroundSort.Atmosphere`) when the script defines no table.
const ATMOSPHERE_SORT: u32 = 7;

/// AO space (left handed) → scene space.
fn scene(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[1], -v[2]]
}

struct Builder<'a> {
    store: &'a RecordStore,
    names: NameTable,
    ctx: Ctx,
    objs: &'a [Obj],
    sky: &'a super::Sky,
    scene: &'a mut Scene,
    sorts: HashMap<String, u32>,
}

/// Adds the sky of `objs` (and the atmosphere `dome`) to `scene.sky`, back to front.
pub fn emit(sky: &super::Sky, objs: &[Obj], store: &RecordStore, scene: &mut Scene, dome: Mesh) {
    let Ok(names) = NameTable::load(store) else { return };
    let sorts = objs.iter().find(|o| o.name == "BackgroundSort").map_or_else(HashMap::new, |o| {
        o.fields.iter().filter_map(|(k, v)| Some((k.clone(), script::eval(v, &|_| None)? as u32))).collect()
    });
    let ctx = Ctx {
        cloud_intensity: sky.weather.thick_clouds_intensity(),
        hq_offset: sky.hq_offset,
        delta_time: 0.0,
        wind: [0.0; 2],
        night: sky.night,
        counters: counters(objs),
        waves: [0.0; 16],
        ..super::game_context(objs, sky.day_time)
    };
    scene.water_light=water_light(objs,&ctx);
    scene.ground_light = ground_light(objs, &ctx);
    scene.sun_authored = scene.ground_light.is_some();
    scene.daylight = daylight(objs, &ctx);
    scene.moons = [None; 3];
    scene.directional_lights = horizon_lights(objs, &ctx);
    let mut b = Builder { store, names, ctx, objs, sky, scene, sorts };
    // (sort, mesh, constant spin, aurora simulation, wave driven turns)
    type Layer = (u32, Mesh, Option<SkySpin>, Option<(ao_scene::aurora::GloomySky, f32)>, Vec<SkyWaveSpin>, Option<f32>, Option<usize>);
    let mut layers: Vec<Layer> = vec![(b.sorts.get("Atmosphere").copied().unwrap_or(ATMOSPHERE_SORT), dome, None, None, vec![], None, objs.iter().position(|o| o.name == "Atmosphere"))];
    for (object, o) in objs.iter().enumerate().filter(|(_, o)| o.field("Priority").is_some_and(|p| p.trim() == "e_RenderPriority_PreRendering")) {
        let mut colors = None;
        let mesh = match o.fxid() {
            "GloomySky" => b.gloomy_sky(o).map(|(m, sim, gain)| {
                colors = Some((sim, gain));
                m
            }),
            "GenericMeshObject" => b.mesh_object(o).map(|m| b.fogged(o, m)),
            "SunRays" => b.sun_rays(o),
            "SingleCloud" => b.single_clouds(o).map(|m| b.fogged(o, m)),
            "GenericVisualObject" => b.dot_points(o).map(|m| b.fogged(o, m)),
            _ => None,
        };
        // Keep invisible sun fans: their native horizon fade is evaluated every frame, including sunrise.
        if let Some(m) = mesh.filter(|m| o.fxid() == "SunRays" || m.vertices.iter().any(|v| v.color[3] > 0.0)) {
            let spin = if o.fxid() == "GenericMeshObject" { b.spin(o) } else { None };
            let altitude=(o.fxid()=="SingleCloud").then(||script::float(o,&b.ctx,"Altitude",100.0));
            layers.push((b.sort(o), m, spin, colors, wave_spins(o, &b.ctx), altitude, Some(object)));
        }
    }
    layers.sort_by_key(|l| l.0);
    for (_, m, spin, colors, waves, cloud_altitude, object) in layers {
        scene.meshes.push(m);
        if let Some(altitude)=cloud_altitude {
            scene.single_clouds.push(ao_scene::cloud::CloudLayer {mesh:scene.meshes.len()-1,altitude,
                wind:[sky.weather.high_altitude_wind_rate[0],0.0,sky.weather.high_altitude_wind_rate[1]],
                storm:sky.weather.storm_intensity(),thick:sky.weather.thick_clouds_intensity(),
                sun_y:super::sun_ao(sky.day_time)[1],light:sky.sun.unwrap_or([0.0;3]),light_direction:sky.sun_dir,ambient:sky.ambient.unwrap_or([0.0;3])});
        }
        scene.sky_colors.extend(colors.map(|(sim, gain)| SkyColors { mesh: scene.meshes.len() - 1, sim, gain }));
        scene.sky.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
        scene.sky_pose_objects.extend(object.map(|object| (scene.sky.len() - 1, object)));
        if let Some(moon) = object.and_then(|index|
            ["Moon1", "Moon2", "Moon3"].iter().position(|name| *name == objs[index].name)) {
            let vertices = &scene.meshes[scene.meshes.len() - 1].vertices;
            if !vertices.is_empty() {
                let center = std::array::from_fn(|k|
                    vertices.iter().map(|v| v.pos[k]).sum::<f32>() / vertices.len() as f32);
                if let Some(direction) = unit_direction(center) {
                    scene.moons[moon] = Some(ao_scene::MoonSource { toward_light: direction,
                        world_gamma_budget: None, resource_center: center, sky_instance: scene.sky.len() - 1 });
                }
            }
        }
        if object.is_some_and(|index| objs[index].field("RotationType").is_some_and(|value| value.trim() == "e_LockToCameraY")) {
            scene.sky_yaw.push(scene.sky.len() - 1);
        }
        scene.sky_spin.extend(spin.map(|s| SkySpin { instance: scene.sky.len() - 1, ..s }));
        scene.sky_wave_spin.extend(waves.into_iter().map(|w| SkyWaveSpin { instance: scene.sky.len() - 1, ..w }));
    }
}

/// Exact FXS rotation/scale relative to the resource snapshot, not interpolation between coarse worker samples.
pub(super) fn pose_source(objs: Vec<Obj>, hq_offset: [f32; 2]) -> impl FnMut(f32, &mut Scene) + Send {
    let mut base = Ctx { hq_offset, counters: counters(&objs), ..Ctx::at(0.0) };
    let mut current = base.clone();
    let game = objs.iter().position(|o| o.name == "GAME");
    move |time, snapshot| {
        for (ctx, time) in [(&mut base, snapshot.day_time), (&mut current, time)] {
            ctx.day_time = time.rem_euclid(super::DAY_LENGTH);
            let [sun1, sun2] = super::sun_positions(ctx.day_time);
            ctx.sun1 = super::sun_rotation(sun1);
            ctx.sun2 = super::sun_rotation(sun2);
            ctx.day_factor = None;
            if let Some(game) = game { ctx.bind_game(&objs[game]); }
        }
        for &(instance, object) in &snapshot.sky_pose_objects {
            let Some(o) = objs.get(object).filter(|o| matches!(o.fxid(), "GenericMeshObject" | "SunRays")) else { continue };
            let Some(expr) = o.field("Rotation") else { continue };
            let (Some(q0), Some(q1)) = (script::rotation(o, &base, expr, 0), script::rotation(o, &current, expr, 0)) else { continue };
            let ratio = if o.fxid() == "SunRays" { 1.0 } else { sky_scale(o, &current, q1) / sky_scale(o, &base, q0) };
            let inverse = Quat { w: q0.w, x: -q0.x, y: -q0.y, z: -q0.z };
            let relative = inverse.then(q1);
            let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .map(|axis| scene(relative.rotate(scene(axis))).map(|v| v * ratio));
            let before = scene(sky_offset(o, &base));
            let after = scene(sky_offset(o, &current));
            let translation: [f32; 3] = std::array::from_fn(|k| after[k] - (0..3).map(|j| axes[j][k] * before[j]).sum::<f32>());
            let Some(inst) = snapshot.sky.get_mut(instance) else { continue };
            inst.transform = [
                [axes[0][0], axes[0][1], axes[0][2], 0.0],
                [axes[1][0], axes[1][1], axes[1][2], 0.0],
                [axes[2][0], axes[2][1], axes[2][2], 0.0],
                [translation[0], translation[1], translation[2], 1.0],
            ];
            if o.fxid() == "SunRays" {
                if let Some(mesh) = snapshot.meshes.get_mut(inst.mesh) {
                    let alpha = sun_alpha(q1);
                    if mesh.vertices.first().is_some_and(|vertex| vertex.color[3] != alpha) {
                        for vertex in &mut mesh.vertices { vertex.color[3] = alpha; }
                    }
                }
            }
            // Wave turns are expressed in the current sun frame, not the last worker sample's frame.
            for wave in snapshot.sky_wave_spin.iter_mut().filter(|wave| wave.instance == instance) {
                current.waves[wave.curve & 15] = 1.0;
                let turn = script::rotation(o, &current, expr, 0).and_then(|q| q.spin_from(q1));
                current.waves[wave.curve & 15] = 0.0;
                if let Some((axis, degrees)) = turn {
                    wave.axis = scene(axis);
                    wave.degrees = -degrees;
                    wave.pivot = after;
                }
            }
        }
        snapshot.ground_light = ground_light(&objs, &current);
        snapshot.sun_authored = snapshot.ground_light.is_some();
        snapshot.daylight = daylight(&objs, &current);
        for moon in snapshot.moons.iter_mut().flatten() {
            if let Some(instance) = snapshot.sky.get(moon.sky_instance) {
                let center = std::array::from_fn(|k| instance.transform[3][k]
                    + (0..3).map(|j| instance.transform[j][k] * moon.resource_center[j]).sum::<f32>());
                if let Some(direction) = unit_direction(center) { moon.toward_light = direction; }
            }
        }
        if let Some(env) = &mut snapshot.environment {
            env.sun_dir = scene(current.sun1.rotate(super::SUN_LOCAL));
        }
    }
}

fn daylight(objs: &[Obj], ctx: &Ctx) -> Option<f32> {
    let game = objs.iter().find(|object| object.name == "GAME")?;
    let value = script::eval_field(game, ctx, game.field("CurrentDayIntensity")?, 0)?;
    value.is_finite().then(|| value.clamp(0.0, 1.0))
}

/// DS1005a3a9: quartic horizon fade, converted to a byte before the rim modulation.
fn sun_alpha(rotation: Quat) -> f32 {
    let height = rotation.rotate([0.0, 0.0, -1.0])[1];
    ((height * 100.0).clamp(0.0, 1.0).powi(4) * 255.0).trunc() / 255.0
}

/// DS1005d589 binds authored GroundLightCurrent/Direction, including Penumbra's GLD [ROT] GLR.
/// Gamma colour is metadata only; the faithful Environment is left untouched.
pub(super) fn ground_light(objs: &[Obj], ctx: &Ctx) -> Option<([f32; 3], [f32; 3])> {
    let light = objs.iter().find(|o| o.name == "SunLight")?;
    let color = script::vector_expr(objs, light, ctx, light.field("GroundLightCurrent")?, 0)?;
    let rotation = script::rotation_expr(objs, light, ctx, light.field("GroundLightDirection")?, 0)?;
    let direction = scene(rotation.rotate(super::SUN_LOCAL));
    if !color.iter().chain(direction.iter()).all(|v| v.is_finite()) { return None; }
    Some((color, direction))
}

fn unit_direction(vector: [f32; 3]) -> Option<[f32; 3]> {
    let length = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    (length.is_finite() && length > 1e-8).then(|| vector.map(|v| v / length))
}

/// DS1005d589 binds the actual SunLight formulas, not merely the RGB tracks.
/// Shadowlands has populated tracks but deliberately sets WaterLightCurrent black.
fn water_light(objs:&[Obj],ctx:&Ctx)->Option<([f32;3],[f32;3])> {
    let light=objs.iter().find(|o|o.name=="SunLight")?;
    let color=script::vector(light,ctx,light.field("WaterLightCurrent")?,0)?;
    let rotation=script::rotation(light,ctx,light.field("WaterLightDirection")?,0)?;
    Some((color.map(srgb_to_linear),scene(rotation.rotate([0.0,0.0,-1.0]))))
}

/// Inferno's four HorizonLight objects are lights, not renderable horizon geometry.
/// DS1005bdad binds Use/Direction/Color and creates a root RLight of type1.
fn horizon_lights(objs: &[Obj], ctx: &Ctx) -> Vec<ao_scene::DirectionalLight> {
    objs.iter().filter(|o| o.fxid() == "DirectionalLight" && o.name.starts_with("HorizonLight_"))
        .filter_map(|o| {
            if o.field("Use")?.trim() == "e_No" { return None; }
            let direction = scene(script::vector(o, ctx, o.field("Direction")?, 0)?).map(|v| -v);
            let expression = o.field("Color")?;
            let (track, rest) = expression.split_once('[')?;
            let (index, factor) = rest.split_once(']')?;
            let (owner, field) = track.trim().split_once('.')?;
            let owner = objs.iter().find(|o| o.name == owner)?;
            let factor = script::eval_field_in(objs, o, ctx, factor.trim().strip_prefix('*')?.trim(), 0)?;
            let entries = owner.field(field)?;
            let palette: Option<Vec<_>> = entries.match_indices("v(")
                .map(|(start, _)| script::vector(owner, ctx, &entries[start..], 0)).collect();
            let palette = palette.filter(|p| !p.is_empty())?;
            let phase = script::eval_field_in(objs, o, ctx, index, 0)?;
            let rate = ctx.counters.iter().find_map(|(key, rate)| {
                (key.strip_prefix(&owner.name)? == ".Counter").then_some(*rate)
            })?;
            let mut light = ao_scene::DirectionalLight {
                direction, color: [0.0; 3],
                cycle: Some(ao_scene::LightCycle { palette, phase, rate, intensity: factor }),
            };
            light.color = light.color_at(0.0);
            Some(light)
        }).collect()
}

/// Position expressions of the scenery objects: `RKPP.<place> - PlayfieldData.UniversePosition + This.UniversePosition + v(..)`
/// (terms separated by `+` / `-`, evaluated left to right).
pub(super) fn position_expr(objs: &[Obj], o: &Obj, expr: &str) -> Option<[f32; 3]> {
    position_expr_at(objs, o, expr, 0)
}

/// `depth` counts the `RKWP.` waypoint hops (a self-referencing script would otherwise recurse forever).
fn position_expr_at(objs: &[Obj], o: &Obj, expr: &str, depth: u8) -> Option<[f32; 3]> {
    let ctx = Ctx::at(0.0);
    let rkpp = objs.iter().find(|r| r.name == "RKPP")?;
    let place = |name: &str| script::vector(rkpp, &ctx, rkpp.field(name)?, 0);
    let term = |t: &str| -> Option<[f32; 3]> {
        let t = t.trim();
        if let Some(n) = t.strip_prefix("RKPP.") {
            return place(n);
        }
        if t == "PlayfieldData.UniversePosition" {
            let pf = objs.iter().find(|p| p.name == "PlayfieldData")?;
            return place(pf.field("UniversePosition")?.trim().strip_prefix("RKPP.")?);
        }
        if let Some(n) = t.strip_prefix("RKWP.") {
            // `RKWP.Pos_*` waypoint of `Tweak_RubiKa_Waypoints`: a place position plus a local offset
            let w = objs.iter().find(|r| r.name == "RKWP")?;
            return position_expr_at(objs, w, w.field(n)?, depth.checked_add(1).filter(|&d| d < 8)?);
        }
        if t == "This.WND" {
            // `GAME.LowAltitudeWind * GameDeltaTime * 1.1` smoothed: the wind drift of the `_A` waypoints, 0 for a still frame
            return Some([0.0; 3]);
        }
        if let Some(f) = t.strip_prefix("This.") {
            return script::vector(o, &ctx, o.field(f)?, 0);
        }
        script::vector(o, &ctx, t, 0)
    };
    let mut total = [0.0f32; 3];
    let mut sign = 1.0;
    let mut rest = expr.trim();
    loop {
        let cut = rest.char_indices().skip(1).find(|&(i, c)| (c == '+' || c == '-') && rest[..i].ends_with(' ') && rest[i + 1..].starts_with(' ')).map_or(rest.len(), |(i, _)| i);
        let v = term(&rest[..cut])?;
        total = std::array::from_fn(|k| total[k] + sign * v[k]);
        if cut >= rest.len() {
            return Some(total);
        }
        sign = if rest.as_bytes()[cut] == b'+' { 1.0 } else { -1.0 };
        rest = rest[cut + 1..].trim_start();
    }
}

/// Placed scenery of the tweak script that is not a sky layer: the `Priority Normal` world-space mesh objects whose
/// `Visibility` is `GAME.GameShowSpaceship` (default 1: `FUN_10062d59` DisplaySystem sets the flag at `+0x189`). A
/// `UniversePosition` that is a single point is a static city skyline (`simplecity_oldathen.abiff`, ...), added to
/// `scene.instances` with the object-space mesh and the tweak `Rotation`, at
/// `RKPP.<place> - PlayfieldData.UniversePosition + UniversePosition`; `UniversePosition [N]` waypoint loops are traffic
/// ships ([`super::traffic`], `Scene::movers`). Their scale is [`object_scale`].
pub fn emit_distant(objs: &[Obj], store: &RecordStore, scene: &mut Scene, day_time: f32) {
    let Ok(names) = NameTable::load(store) else { return };
    let ctx = Ctx::at(0.0);
    scene.day_time = day_time;
    let first_instance = scene.instances.len();
    let mut ship_meshes = HashMap::new();
    for o in objs {
        let is_scenery = o.fxid() == "GenericMeshObject"
            && o.field("Priority").is_some_and(|p| p.trim() == "e_RenderPriority_Normal")
            && o.field("PositionType").is_some_and(|p| p.trim() == "e_World")
            && o.field("Visibility").is_some_and(|v| v.contains("GameShowSpaceship"));
        if !is_scenery {
            continue;
        }
        if !o.field("UniversePosition").is_some_and(|u| u.trim_start().starts_with("v(")) {
            super::traffic::emit(objs, o, store, &names, scene, &mut ship_meshes, day_time);
            continue;
        }
        let (Some(pos), Some(id)) = (o.field("Position").and_then(|e| position_expr(objs, o, e)), o.string("Mesh").and_then(|m| names.id(MESH_TYPE, m))) else { continue };
        let mut tmp = Scene::default();
        if !matches!(decode_generic_mesh(store, id, mesh_index(o, &ctx), false, &mesh_states(o), &mut tmp), Ok(Some(_))) {
            continue;
        }
        let Some(mut mesh) = tmp.meshes.pop() else { continue };
        scene.textures.extend(tmp.textures);
        let q = o.field("Rotation").and_then(|e| script::rotation(o, &ctx, e, 0)).unwrap_or(Quat::IDENTITY);
        let scale = object_scale(o, &ctx);
        for v in &mut mesh.vertices {
            v.pos = scene_v(q.rotate(scene_v(v.pos)).map(|c| c * scale));
            v.normal = scene_v(q.rotate(scene_v(v.normal)));
        }
        scene.meshes.push(mesh);
        let mut transform = IDENTITY;
        transform[3] = [pos[0], pos[1], -pos[2], 1.0];
        scene.instances.push(Instance { mesh: scene.meshes.len() - 1, transform });
        if is_far_away(o) {
            scene.far_away.push(scene.instances.len() - 1);
        }
    }
    scene.spaceships.extend(first_instance..scene.instances.len());
}

/// `ScaleType e_ScaleVisibleFarAway` (= 3): the client pulls the object in towards the camera every frame, see
/// [`ao_scene::far_away_pose`] (`FUN_1005c124`, the `GenericMeshObject` process function).
pub(super) fn is_far_away(o: &Obj) -> bool {
    o.field("ScaleType").is_some_and(|t| t.trim() == "e_ScaleVisibleFarAway")
}

/// Base scale of world-space distant scenery. The installed world callers use
/// type 3; `1005c124` sets their per-frame scale via [`ao_scene::far_away_pose`].
/// Types 1/2 are processed separately for normalized camera-locked sky geometry.
pub(super) fn object_scale(o: &Obj, ctx: &Ctx) -> f32 {
    if o.field("ScaleType").is_some_and(|t| t.trim().starts_with("e_Scale")) {
        1.0
    } else {
        script::float(o, ctx, "Scale", 1.0)
    }
}

pub(super) fn mesh_index(o: &Obj, ctx: &Ctx) -> Option<usize> {
    o.field("MeshIndex").and_then(|e| script::eval_field(o, ctx, e, 0)).map(|v| v as usize)
}

/// Camera-locked geometry is authored at far=1000; projection cancels the common
/// far/1000 scale. Type 2 additionally scales by the rotated InitDirection height
/// (DisplaySystem1005c334; constants1008c078=.9,10089e20=.1).
fn sky_scale(o: &Obj, ctx: &Ctx, rotation: Quat) -> f32 {
    let scale = script::float(o, ctx, "Scale", 1.0);
    if o.field("ScaleType").is_some_and(|t| t.trim() == "e_ScaleByViewDistanceAndDirectionHeigh") {
        let direction = o.field("InitDirection").and_then(|e| script::vector(o, ctx, e, 0)).unwrap_or([0.0; 3]);
        let height = rotation.rotate(direction)[1];
        scale * (0.1 + 0.9 * if height <= 0.0 { 1.0 } else { 1.0 - height })
    } else {
        scale
    }
}

fn sky_offset(o: &Obj, ctx: &Ctx) -> [f32; 3] {
    // GenericMesh1005c334 uses Position*far/1000 (double1008aa78=1000),
    // the same projection-relative units as its geometry and rotation pivots.
    o.field("Position").and_then(|e| script::vector(o, ctx, e, 0)).unwrap_or([0.0; 3])
}

/// AO <-> scene (z mirrored).
fn scene_v(v: [f32; 3]) -> [f32; 3] {
    scene(v)
}

/// Growth per second of every `Counter` field that integrates `GameDeltaTime` (`Counter: GAME.GameDeltaTime * 3 + This.Counter
/// % 360`), keyed `Object.Counter`; DotStars' time-independent counter is stepped by the native point renderer once per frame.
fn counters(objs: &[Obj]) -> std::collections::HashMap<String, f32> {
    let at = |o: &Obj, e: &str, dt: f32| script::eval_field(o, &Ctx { delta_time: dt, ..Ctx::at(0.0) }, e, 0);
    objs.iter()
        .filter_map(|o| {
            let e = o.field("Counter")?;
            let rate = at(o, e, 1.0)? - at(o, e, 0.0)?;
            (rate != 0.0).then(|| (format!("{}.Counter", o.name), rate))
        })
        .collect()
}


/// How an object is lit (`SunlightType`).
enum Light {
    Unlit,
    /// Directional light from the sun, colour `SpaceLight<n>Current`.
    Moon([f32; 3]),
    /// `CloudLightCurrent`, direction `CloudLightDirection` (`Tweak_Rubi-Ka_SunLight.txt`: `GAME.Sun1Rotation`, the same
    /// value as `SpaceLight*Direction`).
    Cloud([f32; 3]),
}

impl Builder<'_> {
    fn sort(&self, o: &Obj) -> u32 {
        o.field("Sorting").and_then(|s| self.sorts.get(s.trim().strip_prefix("BackgroundSort.")?)).copied().unwrap_or(500)
    }

    fn texture(&mut self, name: &str) -> Option<TextureKey> {
        let id = self.names.id(TEXTURES, name)?;
        let key = TextureKey { rdb_type: TEXTURES, id };
        if !self.scene.textures.contains_key(&key) {
            let t = decode_texture(&self.store.get(TEXTURES, id).ok()??).ok()?;
            self.scene.textures.insert(key, t);
        }
        Some(key)
    }

    fn light(&self, o: &Obj) -> Light {
        let sun_light = self.objs.iter().find(|s| s.name == "SunLight");
        let current = |n: u32| {
            sun_light.and_then(|s| script::vector(s, &self.ctx, s.field(&format!("SpaceLight{n}Current"))?, 0)).unwrap_or([1.0; 3])
        };
        match o.field("SunlightType").map(str::trim) {
            Some("e_SunLight_Space0") => Light::Moon(current(0)),
            Some("e_SunLight_Space1") => Light::Moon(current(1)),
            Some("e_SunLight_Space2") => Light::Moon(current(2)),
            Some(t) if t.starts_with("e_SunLight_CloudLayer") => Light::Cloud(self.sky.cloud_light),
            _ => Light::Unlit,
        }
    }

    /// Fog is applied to the sampled source RGB before the native blend equation.
    /// In particular, black fog on `SourceColorOne` must not become a black
    /// alpha-blended overlay: that darkens the destination even at black texels.
    /// VisualFog10058443 installs frame fog; GenericMesh1005c334 brackets each
    /// object with StateBlob::Set10025032 / Reset10024d1e. Missing state inherits it.
    fn fogged(&self, o: &Obj, mut mesh: Mesh) -> Mesh {
        if !o.flag("FOGENABLE").unwrap_or(true) {
            return mesh;
        }
        let colour = o.states.get("FOGCOLOR")
            .and_then(|c| u32::from_str_radix(c.trim_start_matches("0x"), 16).ok())
            .map(|c| [16, 8, 0].map(|s| srgb_to_linear(((c >> s) & 255) as f32 / 255.0)));
        for submesh in &mut mesh.submeshes {
            submesh.sky_fog = true;
            submesh.sky_fog_color = colour;
        }
        mesh
    }

    /// `e_GenericMeshObject`: the authored mesh, rotated by `Rotation`, scaled by `Scale`, with the object's blend state,
    /// texture scroll matrix, light colour and `TFACTOR` alpha baked in.
    fn mesh_object(&mut self, o: &Obj) -> Option<Mesh> {
        let id = self.names.id(MESH_TYPE, o.string("Mesh")?)?;
        let mut tmp = Scene::default();
        let camera_locked = o.field("PositionType").is_some_and(|p| p.trim() == "e_LockToCamera");
        decode_generic_mesh(self.store, id, mesh_index(o, &self.ctx), camera_locked, &mesh_states(o), &mut tmp).ok()??;
        let mut mesh = tmp.meshes.pop()?;
        self.scene.textures.extend(tmp.textures);
        let q = o.field("Rotation").and_then(|e| script::rotation(o, &self.ctx, e, 0)).unwrap_or(Quat::IDENTITY);
        let scale = sky_scale(o, &self.ctx, q);
        let offset = sky_offset(o, &self.ctx);
        let light = self.light(o);
        let lit = o.flag("LIGHTING") != Some(false);
        let gain = if o.states.get("TSS_COLOROP").is_some_and(|v| v == "e_D3DTOP_MODULATE2X") { 2.0 } else { 1.0 };
        let ambient = o.states.get("AMBIENT")
            .and_then(|c| u32::from_str_radix(c.trim_start_matches("0x"), 16).ok())
            .map(|c| [16, 8, 0].map(|s| ((c >> s) & 255) as f32 / 255.0))
            .unwrap_or_else(|| self.scene.environment.map_or(self.sky.ambient.unwrap_or([0.0; 3]), |e| e.ambient.map(|c| c.max(0.0).powf(1.0 / 2.2))));
        let factor = o.field("TFACTOR").and_then(|e| script::unsigned(self.objs, o, &self.ctx, e, 0)).unwrap_or(u32::MAX);
        let stage1_rgb = o.states.get("TSS1_COLOROP").is_some_and(|v| v == "e_D3DTOP_MODULATE")
            && o.states.get("TSS1_COLORARG1").is_some_and(|v| v == "e_D3DTA_CURRENT")
            && o.states.get("TSS1_COLORARG2").is_some_and(|v| v == "e_D3DTA_TFACTOR");
        let stage1_alpha = o.states.get("TSS1_ALPHAOP").is_some_and(|v| v == "e_D3DTOP_MODULATE")
            && o.states.get("TSS1_ALPHAARG1").is_some_and(|v| v == "e_D3DTA_CURRENT")
            && o.states.get("TSS1_ALPHAARG2").is_some_and(|v| v == "e_D3DTA_TFACTOR");
        // `ALPHAOP SELECTARG2` with `ALPHAARG2 = TFACTOR`: the alpha is the top byte of `TFACTOR`
        let alpha = match (o.states.get("TSS_ALPHAARG2").map(String::as_str), o.field("TFACTOR")) {
            (Some("e_D3DTA_TFACTOR"), Some(e)) => script::unsigned(self.objs, o, &self.ctx, e, 0).map_or(1.0, |t| (t >> 24) as f32 / 255.0),
            _ => 1.0,
        };
        let scroll = if o.states.contains_key("TSS_TEXTURETRANSFORMFLAGS") { scroll_matrix(o, &self.ctx) } else { [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] };
        let to_sun = scene(self.sky.sun_ao);
        for v in &mut mesh.vertices {
            let p = q.rotate(scene(v.pos)).map(|c| c * scale);
            v.pos = scene([p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]);
            v.normal = scene(q.rotate(scene(v.normal)));
            let c = match light {
                Light::Unlit => [1.0; 3],
                Light::Moon(l) | Light::Cloud(l) => {
                    let d = (0..3).map(|k| v.normal[k] * to_sun[k]).sum::<f32>().max(0.0);
                    std::array::from_fn(|k| l[k] * d + ambient[k])
                }
            };
            v.color = [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), alpha];
            v.uv = [v.uv[0] * scroll[0] + v.uv[1] * scroll[2] + scroll[4], v.uv[0] * scroll[1] + v.uv[1] * scroll[3] + scroll[5]];
        }
        let uv_scroll = self.uv_scroll(o);
        let uv_wave = uv_wave(o, &self.ctx);
        let uv_camera = if o.states.contains_key("TSS_TEXTURETRANSFORMFLAGS") { uv_camera(o, &self.ctx) } else { [0.0; 4] };
        for s in &mut mesh.submeshes {
            s.uv_scroll = uv_scroll;
            s.uv_wave = uv_wave;
            s.uv_camera = uv_camera;
            if !lit {
                s.emissive = [0.0; 3];
            }
            for (k, shift) in [16, 8, 0].into_iter().enumerate() {
                s.base_color[k] = srgb_to_linear(gain * if stage1_rgb { ((factor >> shift) & 255) as f32 / 255.0 } else { 1.0 });
            }
            if stage1_alpha {
                s.base_color[3] *= (factor >> 24) as f32 / 255.0;
            }
        }
        Some(mesh)
    }

    /// The turn the object's `Rotation` makes per second when it reads a time driven `Counter` (the Shadowlands vortex:
    /// `Ry(Counter) [ROT] Rx(15)` with `Counter` growing by 3 per second): the rotation at `GameDeltaTime = 1` relative to the
    /// baked one at 0, about the point the object's `Position` puts its origin. `instance` is filled in by the caller.
    fn spin(&self, o: &Obj) -> Option<SkySpin> {
        let e = o.field("Rotation")?;
        let q0 = script::rotation(o, &self.ctx, e, 0)?;
        let q1 = script::rotation(o, &Ctx { delta_time: 1.0, ..self.ctx.clone() }, e, 0)?;
        let (axis, degrees) = q1.spin_from(q0)?;
        let offset = sky_offset(o, &self.ctx);
        // AO space is left handed: the same turn about the mirrored axis runs the other way in scene space
        Some(SkySpin { instance: 0, axis: scene(axis), pivot: scene(offset), degrees_per_second: -degrees })
    }

    /// Texture drift in uv units per second of the object's `ScrollU` / `ScrollV` integrators (`This.ScrollU + GAME.GameDeltaTime
    /// * k`, `... + GAME.HighAltitudeWindX % 1 * 10`): the first step of the per-frame expression evaluated with
    /// `GameDeltaTime = 1 s` and the wind rate of `Sky::weather`. Added to the matrix translation, so it moves the baked uv 1:1.
    fn uv_scroll(&self, o: &Obj) -> [f32; 2] {
        let ctx = Ctx { delta_time: 1.0, wind: self.sky.weather.high_altitude_wind_rate, ..self.ctx.clone() };
        ["ScrollU", "ScrollV"].map(|f| o.field(f).and_then(|e| script::eval_field(o, &ctx, e, 0)).unwrap_or(0.0))
    }

    /// `e_GloomySky` (Shadowlands aurora): the dome of `FUN_10046ccc` oriented by `Rotation`, with the colour map simulation
    /// (`ao_scene::aurora`, `FUN_10047300`) the renderer runs for its vertex colours. The state blob gives the blend
    /// (`DESTBLEND ONE` = additive, else alpha blend), the texture (`COLORARG1 ... | COMPLEMENT` = the inverted texture) and
    /// the `MODULATE2X/4X` gain on the diffuse colour. Alpha is `tex.a * diffuse.a` [INFERENCE: the stage-0 alpha default].
    fn gloomy_sky(&mut self, o: &Obj) -> Option<(Mesh, ao_scene::aurora::GloomySky, f32)> {
        let params = aurora::params(o, &self.ctx)?;
        let (mut vertices, indices) = aurora::mesh(o, &self.ctx, &params);
        let q = o.field("Rotation").and_then(|e| script::rotation(o, &self.ctx, e, 0)).unwrap_or(Quat::IDENTITY);
        for v in &mut vertices {
            v.pos = scene(q.rotate(v.pos));
            v.normal = [0.0, -1.0, 0.0];
        }
        let mut key = o.texture.as_deref().and_then(|n| self.texture(n));
        if o.states.get("TSS_COLORARG1").is_some_and(|c| c.contains("COMPLEMENT")) {
            key = key.and_then(|k| {
                let mut t = self.scene.textures.get(&k)?.clone();
                t.rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[..3].iter_mut().for_each(|c| *c = 255 - *c));
                let inv = TextureKey { rdb_type: 0, id: 0x8000_0000 | k.id };
                self.scene.textures.insert(inv, t);
                Some(inv)
            });
        }
        let blend = if o.states.get("DESTBLEND").is_some_and(|v| v == "e_D3DBLEND_ONE") { Blend::Additive } else { Blend::AlphaBlend };
        let gain = match o.states.get("TSS_COLOROP").map(String::as_str) {
            Some("e_D3DTOP_MODULATE4X") => 4.0,
            Some("e_D3DTOP_MODULATE2X") => 2.0,
            _ => 1.0,
        };
        let mesh = Mesh { vertices, submeshes: vec![Submesh { two_sided: true, blend, ..Submesh::new(indices, key) }] };
        Some((mesh, ao_scene::aurora::GloomySky::new(params), gain))
    }

    /// `e_SunRays` (`FUN_1005a7f6` builds the geometry, `FUN_1005a3a9` the per-frame alpha, DisplaySystem): a triangle fan of
    /// `Vertices` points whose rim `i` (of `Vertices - 1`) sits at local `(Size * 9 sin a, Size * 9 cos a, -100)`,
    /// `a = i / (Vertices - 1) * 360 * 3.14 / 180`, uv `0.5 + UVSize * 0.5 (sin a, cos a)` (default `UVSize` 0.8), centre
    /// `(0, 0, -100)` uv (0.5, 0.5); rotated by `Rotation`. Colour `ColorR/G/B`, alpha `(100 y)^4` below elevation 0.01 and 0
    /// below the horizon. The rim alpha is `(255 - table[i & 7]) * A / 255` (`FUN_1005a3a9` @0x1005a3a9, `A` = the alpha above,
    /// `table` = [`ao_scene::sun_flicker_table`], the game's per-frame `trunc(255 * GameWaveCurve_i)`): vertex `normal.z` carries
    /// `i` (centre -1) and `Submesh::sun_flicker` makes the renderer apply it every frame.
    #[allow(clippy::approx_constant)] // the client's literal 3.14 (double 1008c380), not pi
    fn sun_rays(&mut self, o: &Obj) -> Option<Mesh> {
        let q = script::rotation(o, &self.ctx, o.field("Rotation")?, 0)?;
        let fade = sun_alpha(q);
        let key = self.texture(o.texture.as_deref()?)?;
        let col = ["ColorR", "ColorG", "ColorB"].map(|n| script::float(o, &self.ctx, n, 255.0) / 255.0).map(srgb_to_linear);
        let size = script::float(o, &self.ctx, "Size", 6.0);
        let uv_size = script::float(o, &self.ctx, "UVSize", 0.8);
        let n = (script::float(o, &self.ctx, "Vertices", 33.0) as usize).clamp(4, 256);
        let blend = if o.states.get("DESTBLEND").is_some_and(|v| v == "e_D3DBLEND_ONE") { Blend::Additive } else { Blend::AlphaBlend };
        let color = [col[0], col[1], col[2], fade];
        let mut mesh = Mesh::default();
        // Native 100-unit fan; the renderer applies ScaleByViewDistance (far/1000).
        let mut push = |local: [f32; 3], uv: [f32; 2], rim: f32| {
            let p = q.rotate(local);
            mesh.vertices.push(Vertex { pos: scene(p), normal: [0.0, -1.0, rim], uv, color });
        };
        for i in 0..n - 1 {
            let a = i as f32 / (n - 1) as f32 * 360.0 * 3.14 / 180.0;
            push([size * 9.0 * a.sin(), size * 9.0 * a.cos(), -100.0], [uv_size * 0.5 * a.sin() + 0.5, uv_size * 0.5 * a.cos() + 0.5], i as f32);
        }
        push([0.0, 0.0, -100.0], [0.5, 0.5], -1.0);
        let centre = (n - 1) as u32;
        let mut idx: Vec<u32> = (0..centre - 1).flat_map(|i| [i, i + 1, centre]).collect();
        idx.extend([centre - 1, 0, centre]);
        mesh.submeshes.push(Submesh { two_sided: true, blend, sun_flicker: true, ..Submesh::new(idx, Some(key)) });
        Some(mesh)
    }

    /// Native30 sheets: prefs15/16 select10x10 warped lit /2x2 unlit grids,
    /// not the cloud count (DisplaySystem100532b8/10053681/1005485a).
    fn single_clouds(&mut self, _o: &Obj) -> Option<Mesh> {
        let keys: Vec<TextureKey> = (1..=4).map(|n|self.texture(&format!("single_cloud0{n}.png"))).collect::<Option<_>>()?;
        let mut mesh=ao_scene::cloud::Clouds::mesh();
        for ((first,end),key) in [(0,2),(2,4),(4,27),(27,30)].into_iter().zip(keys) {
            mesh.submeshes.push(Submesh {two_sided:true,blend:Blend::AlphaBlend,
                ..Submesh::new(ao_scene::cloud::Clouds::indices(10,first,end),Some(key))});
        }
        Some(mesh)
    }

    /// FXS BlueprintGenerateMesh1000e863 / BlueprintRender1000e500: surface points, not angular billboards.
    fn dot_points(&mut self, o: &Obj) -> Option<Mesh> {
        let bp = self.objs.iter().find(|b| b.name == format!("{}Mesh", o.name))?;
        let corners: Vec<[f32; 3]> = bp.field("Vertex")?.split("v(").skip(1).filter_map(|s| script::vector(bp, &self.ctx, &format!("v({}", s.split(')').next()?), 0)).collect();
        let tris: Vec<u32> = super::eval_list(bp.field("Indice")?).iter().map(|&v| v as u32).collect();
        let n = script::float(bp, &self.ctx, "Vertices", 0.0) as usize;
        let key = self.texture(o.texture.as_deref()?)?;
        let mut mesh = dot_surface(&corners, &tris, n, script::float(bp, &self.ctx, "Size", 1.0))?;
        mesh.submeshes.push(Submesh { two_sided: true, blend: Blend::Additive, sky_points: true,
            ..Submesh::new((0..n as u32).collect(), Some(key)) });
        Some(mesh)
    }
}

/// Object StateBlob precedes node/material deltas; pass its states into decoding
/// instead of overwriting the final material blend/culling after batching.
pub(super) fn mesh_states(o: &Obj) -> [(i32, i32); 5] {
    let blend = |name: &str| match o.states.get(name).map(String::as_str) {
        Some("e_D3DBLEND_ZERO") => Some(1),
        Some("e_D3DBLEND_ONE") => Some(2),
        Some("e_D3DBLEND_SRCCOLOR") => Some(3),
        Some("e_D3DBLEND_INVSRCCOLOR") => Some(4),
        Some("e_D3DBLEND_SRCALPHA") => Some(5),
        Some("e_D3DBLEND_INVSRCALPHA") => Some(6),
        Some("e_D3DBLEND_DESTALPHA") => Some(7),
        Some("e_D3DBLEND_INVDESTALPHA") => Some(8),
        Some("e_D3DBLEND_DESTCOLOR") => Some(9),
        Some("e_D3DBLEND_INVDESTCOLOR") => Some(10),
        _ => None,
    };
    [
        (15, o.flag("ALPHATESTENABLE").map(i32::from)),
        (27, o.flag("ALPHABLENDENABLE").map(i32::from)),
        (19, blend("SRCBLEND")),
        (20, blend("DESTBLEND")),
        (22, match o.states.get("CULLMODE").map(String::as_str) {
            Some("e_D3DCULL_NONE") => Some(1),
            Some("e_D3DCULL_CW") => Some(2),
            _ => None,
        }),
    ].map(|(ty, value)| value.map_or((-1, 0), |value| (ty, value)))
}


/// `Matrix ScrollMatrix` as the 2x3 affine texture transform `[m00 m01 m10 m11 m20 m21]` (identity when absent).
fn scroll_matrix(o: &Obj, ctx: &Ctx) -> [f32; 6] {
    const ID: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let Some(m) = o.field("ScrollMatrix").and_then(|m| m.trim().strip_prefix("m(")) else { return ID };
    let v: Option<Vec<f32>> = script::eval_matrix(o, ctx, m.trim_end_matches(')'));
    match v {
        Some(v) if v.len() >= 12 => [v[0], v[1], v[4], v[5], v[8], v[9]],
        _ => ID,
    }
}

/// Native sampling is nested lerp (not uniform triangle area): lerp(lerp(A,B,a),C,b)*Size.
/// FXS10001dff, helpers10001d26/10001d64/10001ce8; random position pass precedes the UV pass (1000e863).
fn dot_surface(corners: &[[f32; 3]], tris: &[u32], n: usize, size: f32) -> Option<Mesh> {
    if n == 0 || n > u32::MAX as usize || tris.len() < 3 || !tris.len().is_multiple_of(3)
        || tris.iter().any(|&i| i as usize >= corners.len()) { return None; }
    // Unresolved: retail rand is process-global, reseeded/consumed by other effects. Local seed1 is reproducible,
    // not the retail realization; preserve its CRT algorithm, draw order and discrete mapping, not invented equivalence.
    let mut rng = crate::weather::CrtRand::default();
    let random = |rng: &mut crate::weather::CrtRand| (rng.rand() % 10000) as f32 / 10000.0;
    let mut vertices = Vec::with_capacity(n);
    for _ in 0..n {
        let t = (rng.rand() as usize % (tris.len() / 3)) * 3;
        let [a, b, c] = [0, 1, 2].map(|k| corners[tris[t + k] as usize]);
        let (u, v) = (random(&mut rng), random(&mut rng));
        let p = std::array::from_fn(|k| {
            let edge = a[k] + (b[k] - a[k]) * u;
            (edge + (c[k] - edge) * v) * size
        });
        vertices.push(Vertex { pos: scene(p), color: [1.0; 4], ..Vertex::default() });
    }
    for vertex in &mut vertices { vertex.uv = [random(&mut rng), random(&mut rng)]; }
    Some(Mesh { vertices, submeshes: vec![] })
}

/// Turns driven by `GameWaveCurve*` (`Sun1_Moving`: `RotMove = v(0,0,1), GAME.GameWaveCurve0 * 4 - 2`, `Rotation = This.RotMove [ROT]
/// GAME.Sun1Rotation`): per curve the rotation at `GameWaveCurve_k = 1` relative to the baked one at 0 (the expressions are
/// affine in the angle), turned by `degrees * curve(time)` in the renderer. `instance` is filled in by the caller.
fn wave_spins(o: &Obj, base: &Ctx) -> Vec<SkyWaveSpin> {
    let Some(e) = o.field("Rotation") else { return vec![] };
    let Some(q0) = script::rotation(o, base, e, 0) else { return vec![] };
    let offset = sky_offset(o, base);
    wave_curves_read(o, e, 0)
        .into_iter()
        .filter_map(|k| {
            let ctx = Ctx { waves: std::array::from_fn(|i| (i == k) as u32 as f32), ..base.clone() };
            let (axis, degrees) = script::rotation(o, &ctx, e, 0)?.spin_from(q0)?;
            // AO space is left handed: the same turn about the mirrored axis runs the other way in scene space
            Some(SkyWaveSpin { instance: 0, axis: scene(axis), pivot: scene(offset), curve: k, degrees: -degrees })
        })
        .collect()
}

/// `[curve_u, amp_u, curve_v, amp_v]` of the `ScrollMatrix` translation (`GAME.GameWaveCurve0 - 0.5 * 0.005`: the horizon map of
/// the Shadowlands vortex wobbles by 0.005 uv): the first curve the u / v translation depends on and its change per unit curve; the
/// constant part stays baked.
fn uv_wave(o: &Obj, base: &Ctx) -> [f32; 4] {
    let t0 = scroll_matrix(o, base);
    let mut out = [0.0; 4];
    for k in o.field("ScrollMatrix").map_or_else(Vec::new, |e| wave_curves_read(o, e, 0)) {
        let t1 = scroll_matrix(o, &Ctx { waves: std::array::from_fn(|i| (i == k) as u32 as f32), ..base.clone() });
        for (axis, slot) in [(4, 0), (5, 2)] {
            if out[slot + 1] == 0.0 && t1[axis] != t0[axis] {
                out[slot] = k as f32;
                out[slot + 1] = t1[axis] - t0[axis];
            }
        }
    }
    out
}

/// Affine ScrollMatrix translation coefficients [u/cameraAOx,u/cameraAOz,v/cameraAOx,v/cameraAOz].
/// Evaluate about zero to avoid subtracting large native HQ offsets when extracting a one-metre coefficient.
fn uv_camera(o: &Obj, base: &Ctx) -> [f32; 4] {
    let mut ctx = base.clone();
    ctx.hq_offset = [0.0; 2];
    let zero = scroll_matrix(o, &ctx);
    let mut coefficients = [0.0; 4];
    for axis in 0..2 {
        ctx.hq_offset = std::array::from_fn(|k| if k == axis { 1.0 } else { 0.0 });
        let unit = scroll_matrix(o, &ctx);
        coefficients[axis] = unit[4] - zero[4];
        coefficients[axis + 2] = unit[5] - zero[5];
    }
    coefficients
}

/// Indices `N` of the `GAME.GameWaveCurveN` variables `expr` reads, following `This.<field>` references of `o` (depth limited).
fn wave_curves_read(o: &Obj, expr: &str, depth: u32) -> Vec<usize> {
    let mut out: Vec<usize> = vec![];
    for word in expr.split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_')) {
        let k = word.strip_prefix("GAME.GameWaveCurve").and_then(|n| n.parse::<usize>().ok()).filter(|&k| k < 16);
        let nested = word.strip_prefix("This.").filter(|_| depth < 4).and_then(|f| o.field(f)).map(|e| wave_curves_read(o, e, depth + 1));
        for k in k.into_iter().chain(nested.into_iter().flatten()) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn authored_ground_metadata_keeps_penumbra_direction_and_night_energy() {
        let objects = script::parse_objects(concat!(
            "Object GAME\n{\n Float CurrentDayTime: This.GameDayTime\n Float DayTimeFactor: This.CurrentDayTime / 6480\n",
            " Float DayIntensity [5]: 0,0,1,1,0\n Float CurrentDayIntensity: This.DayIntensity[This.DayTimeFactor]\n",
            " Quaternion Sun1Rotation: This.GameSun1Rotation\n}\n",
            "Object SunLight\n{\n Vector GLD: v(0,0,1)\n",
            " Vector GLR: This.GLD [ROT] GAME.Sun1Rotation * GAME.CutY * 2 - GAME.UpDirection [NORMALIZE] 1\n",
            " Quaternion GroundLightDirection: This.GLD [ROT] This.GLR\n",
            " Vector GroundLightCurrent: v(GAME.CurrentDayIntensity,GAME.CurrentDayIntensity,GAME.CurrentDayIntensity)\n}\n"
        ));
        let mut source = pose_source(objects.clone(), [0.0; 2]);
        let environment = ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 0.5, fog_end: 800.0,
            ambient: [0.0; 3], sun_color: [0.2, 0.3, 0.4], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0 };
        let mut snapshot = Scene { environment: Some(environment), ..Default::default() };
        for time in [0.0, 1620.0, 3240.0, 4860.0] {
            let ctx = super::super::game_context(&objects, time);
            let incident = ctx.sun1.rotate([0.0, 0.0, 1.0]);
            let expected = unit_direction([-2.0 * incident[0], 1.0, 2.0 * incident[2]]).unwrap();
            let (color, direction) = ground_light(&objects, &ctx).unwrap();
            assert!((0..3).all(|k| (direction[k] - expected[k]).abs() < 2e-5));
            assert!(direction[1] > 0.0);
            if time == 0.0 { assert_eq!(color, [0.0; 3]); }
            if time == 3240.0 { assert_eq!(color, [1.0; 3]); }
            source(time, &mut snapshot);
            assert_eq!(snapshot.ground_light, Some((color, direction)));
            assert!(snapshot.sun_authored);
            assert_eq!(snapshot.daylight, Some(color[0]), "native GAME.DayIntensity, not the environment's bright sun colour, gates daylight");
            assert_eq!(snapshot.environment.as_ref().unwrap().sun_color, [0.2, 0.3, 0.4]);
            let actual_sun = scene(ctx.sun1.rotate(super::super::SUN_LOCAL));
            assert_eq!(snapshot.environment.as_ref().unwrap().sun_dir, actual_sun);
        }
        let unsupported = script::parse_objects("Object SunLight\n{\n Vector GroundLightCurrent: v(GAME.Unsupported,0,0)\n Quaternion GroundLightDirection: q(0,0,0,1)\n}\n");
        assert!(ground_light(&unsupported, &Ctx::at(0.0)).is_none());
        assert!(ground_light(&[], &Ctx::at(0.0)).is_none());
        assert!(daylight(&[], &Ctx::at(0.0)).is_none(), "missing GAME authoring stays closed");
    }

    #[test]
    fn moon_source_keeps_loaded_factory_pose_and_current_frame_without_world_energy() {
        let objects = script::parse_objects("Object Moon1\n{\n Unsigned FXID e_GenericMeshObject\n Quaternion Rotation v(0,1,0), GAME.CurrentDayTime / 6480 * -360 - 90\n}\n");
        let rotation = |time: f32| Quat::axis_angle([0.0, 1.0, 0.0], time / 6480.0 * -360.0 - 90.0);
        let factory_center = [3.0, 4.0, -5.0];
        let mut source = pose_source(objects, [0.0; 2]);
        let mut snapshot = Scene { sky: vec![Instance { mesh: 0, transform: IDENTITY }],
            sky_pose_objects: vec![(0, 0)], ..Default::default() };
        for baked in [0.0, 10.0] {
            snapshot.day_time = baked;
            let center = scene(rotation(baked).rotate(factory_center));
            snapshot.moons[0] = Some(ao_scene::MoonSource { toward_light: unit_direction(center).unwrap(),
                resource_center: center, sky_instance: 0, world_gamma_budget: None });
            source(11.0, &mut snapshot);
            let moon = snapshot.moons[0].unwrap();
            let expected = unit_direction(scene(rotation(11.0).rotate(factory_center))).unwrap();
            assert!((0..3).all(|k| (moon.toward_light[k] - expected[k]).abs() < 1e-5));
            assert!(moon.world_gamma_budget.is_none());
            assert!(snapshot.ground_light.is_none());
            assert!(!snapshot.sun_authored, "a posed disc never invents world sunlight");
        }
    }

    #[test]
    fn inferno_horizon_lights_use_authored_cardinal_directions_and_palette_clock() {
        let Some(tweaks) = real_tweaks(4605) else { return };
        let objects = tweaks.objects();
        let ctx = Ctx { counters: counters(&objects), ..Ctx::at(0.0) };
        let lights = horizon_lights(&objects, &ctx);
        assert_eq!(lights.len(), 4, "HorizonLight_Base is a formula owner, not a fifth light");
        let directions = [[0.0, 0.3, 1.0], [0.0, 0.3, -1.0], [1.0, 0.3, 0.0], [-1.0, 0.3, 0.0]];
        for ((light, direction), phase) in lights.iter().zip(directions).zip([0.1, 0.3, 0.9, 0.7]) {
            assert_eq!(light.direction, direction, "D3D incident direction must be negated after AO Z mirroring");
            let cycle = light.cycle.as_ref().unwrap();
            assert_eq!((cycle.phase, cycle.rate, cycle.intensity), (phase, 0.4, 0.45));
            assert_eq!(cycle.palette.len(), 10);
            for (time, intensity) in [(0.0, 0.8), (0.125, 0.9), (0.25, 1.0), (2.5, 0.8)] {
                let expected = [1.0, 0.5, 0.04].map(|v| v * intensity * 0.45);
                let actual = light.gamma_at(time);
                assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-6), "{time}: {actual:?} != {expected:?}");
            }
        }
        let mut disabled = objects.clone();
        for object in &mut disabled {
            if object.fxid() == "DirectionalLight" { object.fields.insert("Use".into(), "e_No".into()); }
        }
        assert!(horizon_lights(&disabled, &ctx).is_empty());
        assert!(horizon_lights(&[], &ctx).is_empty(), "no invented horizon replacement");
    }

    use super::*;

    #[test]
    fn horizon_camera_uv_uses_native_hq_base_and_live_ao_camera_axes() {
        let objects = script::parse_objects(concat!(
            "Object Horizon\n{\n Matrix ScrollMatrix: m(1,0,0,0\n 0,1,0,0\n",
            " GAME.OffsetFromHQ_X / 51200.0, GAME.OffsetFromHQ_Z / -51200.0,1,0\n 0,0,0,1)\n}\n"
        ));
        let ctx = Ctx { hq_offset: super::super::native_hq_offset(4582), ..Ctx::at(0.0) };
        let coefficients = uv_camera(&objects[0], &ctx);
        assert_eq!(coefficients, [1.0 / 51200.0, 0.0, 0.0, -1.0 / 51200.0]);
        let baked = scroll_matrix(&objects[0], &ctx);
        for eye in [[0.0, 100.0, 0.0], [1010.4, 24.0, -663.1], [2010.4, 24.0, -1663.1]] {
            let camera = [eye[0], -eye[2]];
            let uv = [
                baked[4] + coefficients[0] * camera[0] + coefficients[1] * camera[1],
                baked[5] + coefficients[2] * camera[0] + coefficients[3] * camera[1],
            ];
            let native = [(camera[0] - 43809.0) / 51200.0, (camera[1] - 27521.0) / -51200.0];
            assert!((0..2).all(|k| (uv[k] - native[k]).abs() < 1e-7));
        }
        assert_eq!(uv_camera(&Obj::default(), &ctx), [0.0; 4]);
    }

    #[test]
    fn dot_surface_uses_native_nested_lerp_and_separate_random_uv_pass() {
        let corners = [[0.0, 0.3, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 0.0, -1.0]];
        let tris = [0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 1];
        // CRT seed1: face41%4, a18467%10000, b6334%10000; UV only after all 3*n position draws.
        let mesh = dot_surface(&corners, &tris, 2, 990.0).unwrap();
        let edge = [0.0, 0.3 * (1.0 - 0.8467), 0.8467];
        let expected = scene([-0.6334 * 990.0, edge[1] * (1.0 - 0.6334) * 990.0, edge[2] * (1.0 - 0.6334) * 990.0]);
        assert!((0..3).all(|k| (mesh.vertices[0].pos[k] - expected[k]).abs() < 0.0001));
        assert_eq!(mesh.vertices[0].uv, [0.1478, 0.9358]);
        let all = dot_surface(&corners, &tris, 500, 990.0).unwrap();
        assert_eq!(all.vertices.len(), 500, "one vertex per native point, not four billboard corners");
        assert!(all.vertices.iter().all(|v| {
            let [x, y, z] = v.pos;
            y >= 0.0 && ((x.abs() + z.abs()) / 990.0 + y / 297.0 - 1.0).abs() < 0.00001
                && v.color == [1.0; 4] && v.uv.iter().all(|u| (0.0..1.0).contains(u))
        }), "points stay on authored pyramid faces, not a normalized sphere");
        assert!(dot_surface(&corners, &[0, 1, 9], 1, 990.0).is_none());
    }

    #[test]
    fn icc_frozen_interfaces_keep_frame_poses_and_lighting_fixed() {
        let objects = script::parse_objects(concat!(
            "Object GAME\n{\n Float CurrentDayTime: 2648.69\n Float DayTimeFactor: This.CurrentDayTime / 6480\n",
            " Quaternion Sun1Rotation: q(0.900351,-0.0951056,0.344078,0.248863)\n",
            " Quaternion Sun2Rotation: q(0.836913,-0.1626300,0.384351,0.354122)\n}\n",
            "Object Sun1\n{\n Unsigned FXID: e_SunRays\n Quaternion Rotation: GAME.Sun1Rotation\n}\n",
            "Object Sun2\n{\n Unsigned FXID: e_SunRays\n Quaternion Rotation: GAME.Sun2Rotation\n}\n",
            "Object Moon\n{\n Unsigned FXID: e_GenericMeshObject\n Quaternion Rotation: v(0,1,1), GAME.CurrentDayTime / 3240 * 360 + 90\n}\n",
            "Object SunLight\n{\n Vector GroundLightCurrent: v(0.2,0.3,0.4)\n Quaternion GroundLightDirection: GAME.Sun1Rotation\n}\n"
        ));
        let expected = scene(super::super::rot(super::super::SUN1_ROT).rotate(super::super::SUN_LOCAL));
        let mut pose = pose_source(objects, [0.0; 2]);
        let mut snapshot = Scene {
            sky: (0..3).map(|mesh| Instance { mesh, transform: IDENTITY }).collect(),
            sky_pose_objects: (0..3).map(|index| (index, index + 1)).collect(),
            environment: Some(ao_scene::Environment {
                sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 0.5, fog_end: 800.0,
                ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0,
            }),
            ..Scene::default()
        };
        for baked in [0.0, 3240.0] {
            snapshot.day_time = baked;
            for time in [0.0, 3240.0, 6481.0] {
                pose(time, &mut snapshot);
                for instance in &snapshot.sky {
                    assert!((0..4).all(|i| (0..4).all(|j| (instance.transform[i][j] - IDENTITY[i][j]).abs() < 1e-5)));
                }
                let actual = snapshot.environment.as_ref().unwrap().sun_dir;
                assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-6));
                assert_eq!(snapshot.ground_light, Some(([0.2, 0.3, 0.4], expected)));
                assert!(snapshot.sun_authored);
                assert_eq!(snapshot.day_time, baked, "native clock must not be rewritten");
            }
        }
    }

    #[test]
    fn celestial_frame_positions_follow_dt_across_resource_snapshot_replacement() {
        // Authored Tweak_Rubi-Ka_Sun/Moon1..3 expressions; no client assets/GPU needed.
        let expressions = [
            ("Sun1", "GAME.Sun1Rotation"), ("Sun2", "GAME.Sun2Rotation"),
            ("Moon1", "v(0,1,0), GAME.CurrentDayTime / 6480 * -360 - 90"),
            ("Moon2", "v(0,1,1), GAME.CurrentDayTime / 3240 * 360 + 90"),
            ("Moon3", "v(0,1,-1), GAME.CurrentDayTime / 2160 * 360 + 60"),
        ];
        let objs: Vec<_> = expressions.iter().enumerate().map(|(index, (name, rotation))| Obj {
            name: (*name).into(),
            fields: [
                ("FXID".into(), if index < 2 { "e_SunRays" } else { "e_GenericMeshObject" }.into()),
                ("Rotation".into(), (*rotation).into()),
                ("Scale".into(), "11".into()),
                ("ScaleType".into(), "e_ScaleByViewDistanceAndDirectionHeigh".into()),
                ("InitDirection".into(), "v(-1,0,0)".into()),
            ].into(),
            ..Obj::default()
        }).collect();
        let orbit = |index: usize, time: f32| {
            let q = match index {
                0 | 1 => super::super::sun_rotation(super::super::sun_positions(time)[index]),
                2 => Quat::axis_angle([0.0, 1.0, 0.0], time / 6480.0 * -360.0 - 90.0),
                3 => Quat::axis_angle([0.0, 1.0, 1.0], time / 3240.0 * 360.0 + 90.0),
                _ => Quat::axis_angle([0.0, 1.0, -1.0], time / 2160.0 * 360.0 + 60.0),
            };
            let height = q.rotate([-1.0, 0.0, 0.0])[1].max(0.0);
            let scale = if index < 2 { 1.0 } else { 11.0 * (0.1 + 0.9 * (1.0 - height)) };
            scene(q.rotate([0.0, 0.0, -100.0]).map(|value| value * scale))
        };
        let transform = |matrix: [[f32; 4]; 4], point: [f32; 3]| -> [f32; 3] {
            std::array::from_fn(|k| matrix[3][k] + (0..3).map(|j| matrix[j][k] * point[j]).sum::<f32>())
        };
        let distance = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt();
        let mut pose = pose_source(objs, [0.0; 2]);
        let start = 2648.69;
        let mut snapshot = Scene {
            sky: (0..5).map(|mesh| Instance { mesh, transform: IDENTITY }).collect(),
            sky_pose_objects: (0..5).map(|index| (index, index)).collect(),
            ..Scene::default()
        };
        let mut previous = std::array::from_fn::<_, 5, _>(|index| orbit(index, start));
        let mut held = previous;
        let mut first_rate = [0.0; 5];
        let mut before_plateaus = [0; 5];
        let mut elapsed = 0.0f64;
        println!("celestial_csv,object,frame,dt,baked_time,before_x,before_y,before_z,after_x,after_y,after_z,delta");
        for frame in 0..120 {
            let dt = if frame % 2 == 0 { 1.0 / 60.0 } else { 1.0 / 30.0 };
            elapsed += f64::from(dt);
            let time = start + elapsed as f32;
            // Replace resource geometry every half-second, as the real worker does;
            // the per-frame pose must neither reset nor wait for that snapshot.
            snapshot.day_time = start + ((elapsed / 0.5).floor() * 0.5) as f32;
            pose(time, &mut snapshot);
            for index in 0..5 {
                let before = orbit(index, snapshot.day_time);
                let after = transform(snapshot.sky[index].transform, before);
                let expected = orbit(index, time);
                assert!(distance(after, expected) < 0.001, "frame {frame} {}: {after:?} != {expected:?}", expressions[index].0);
                let delta = distance(after, previous[index]);
                let rate = delta / dt;
                if frame == 0 { first_rate[index] = rate; }
                assert!(delta > 0.0 && (rate / first_rate[index] - 1.0).abs() < 0.1,
                    "frame {frame} {}: displacement {delta} must follow dt {dt}, rate {rate}", expressions[index].0);
                before_plateaus[index] += usize::from(before == held[index]);
                println!("celestial_csv,{},{frame},{dt:.9},{:.6},{:.9},{:.9},{:.9},{:.9},{:.9},{:.9},{delta:.9}",
                    expressions[index].0, snapshot.day_time, before[0], before[1], before[2], after[0], after[1], after[2]);
                held[index] = before;
                previous[index] = after;
            }
        }
        assert!(before_plateaus.iter().all(|&count| count > 100), "the old 2Hz hold reproduces the reported stepping");
    }

    #[test]
    fn native_locked_scale_and_position_use_projection_relative_units() {
        let objects = script::parse_objects("Object Moon\n{\n Float Scale: 11\n Unsigned ScaleType: e_ScaleByViewDistanceAndDirectionHeigh\n Vector InitDirection: v( -1,0,0 )\n Vector Position: v( 1,2,3 )\n Unsigned PositionType: e_LockToCamera\n}\n");
        let o = &objects[0];
        let ctx = Ctx::at(0.0);
        let close = |a: f32, b: f32| assert!((a - b).abs() < 1e-5, "{a} != {b}");
        close(sky_scale(o, &ctx, Quat::IDENTITY), 11.0);
        // Rotation alone determines height; the inherited mesh frame must not
        // rotate InitDirection a second time.
        close(sky_scale(o, &ctx, Quat::axis_angle([0.0, 0.0, 1.0], -90.0)), 1.1);
        close(sky_scale(o, &ctx, Quat::axis_angle([0.0, 0.0, 1.0], 90.0)), 11.0);
        close(sky_scale(o, &ctx, Quat::axis_angle([0.0, 0.0, 1.0], -30.0)), 6.05);
        assert_eq!(sky_offset(o, &ctx), [1.0, 2.0, 3.0]);
        let mut world = o.clone();
        world.fields.insert("PositionType".into(), "e_World".into());
        assert_eq!(sky_offset(&world, &ctx), [1.0, 2.0, 3.0]);
        world.fields.insert("ScaleType".into(), "e_ScaleByViewDistance".into());
        close(sky_scale(&world, &ctx, Quat::IDENTITY), 11.0);
    }

    #[test]
    fn water_light_uses_authored_current_not_ground_or_unused_tracks() {
        let make=|current:&str|script::parse_objects(&format!("Object SunLight\n{{\n Quaternion WaterLightDirection: GAME.Sun1Rotation\n Float WaterLightR [2]: 0.25,0.75\n Float WaterLightG [2]: 0.5,1\n Float WaterLightB [2]: 0.75,0.25\n Vector WaterLightCurrent: {current}\n}}\n"));
        let ctx=Ctx::at(0.0);
        let tracked=make("v(This.WaterLightR[GAME.DayTimeFactor],This.WaterLightG[GAME.DayTimeFactor],This.WaterLightB[GAME.DayTimeFactor])");
        assert_eq!(water_light(&tracked,&ctx),Some(([0.25,0.5,0.75].map(srgb_to_linear),[0.0,0.0,1.0])));
        assert_eq!(water_light(&make("v(0,0,0)"),&ctx),Some(([0.0;3],[0.0,0.0,1.0])));
        assert_eq!(water_light(&[],&ctx),None);
    }

    #[test]
    fn self_referencing_waypoint_is_rejected() {
        let objs = super::super::script::parse_objects("Object RKPP\n{\n  Vector A v(1,2,3)\n}\nObject RKWP\n{\n  Vector P RKWP.P\n}\n");
        let o = objs[1].clone();
        assert_eq!(position_expr(&objs, &o, "RKWP.P"), None);
    }

    #[test]
    fn distant_world_scale_field_is_ignored_unless_scale_type_is_zero() {
        let obj = |ty: Option<&str>| {
            let mut o = Obj::default();
            o.fields.insert("Scale".into(), "0.70".into());
            o.fields.extend(ty.map(|t| ("ScaleType".to_string(), t.to_string())));
            o
        };
        let ctx = Ctx::at(0.0);
        assert_eq!(object_scale(&obj(None), &ctx), 0.7);
        assert_eq!(object_scale(&obj(Some("e_World")), &ctx), 0.7);
        for t in ["e_ScaleVisibleFarAway", "e_ScaleByViewDistance", "e_ScaleByViewDistanceAndDirectionHeigh"] {
            assert_eq!(object_scale(&obj(Some(t)), &ctx), 1.0, "{t}");
        }
    }

    const WAVES: &str = "
Object Sun1_Moving
{
  Quaternion  RotMove:            v( 0,0,1 ), GAME.GameWaveCurve0 * 4 - 2
  Quaternion  Rotation:           This.RotMove [ROT] GAME.Sun1Rotation
  Vector      Position:           v( 0,0,0 )
}
Object VortexHorizon
{
  Matrix      ScrollMatrix:       m(                                 1,                                0, 0, 0
                                                                     0,                                1, 0, 0
                                      GAME.GameWaveCurve0 - 0.5 * 0.005, GAME.GameWaveCurve1 - 0.5 * 0.005, 1, 0
                                                                     0,                                0, 0, 1 )
}
";

    #[test]
    fn wave_curve_terms_become_a_turn_and_a_uv_wobble() {
        let objs = script::parse_objects(WAVES);
        let ctx = Ctx::at(0.0);
        let spins = wave_spins(&objs[0], &ctx);
        assert_eq!(spins.len(), 1);
        assert_eq!(spins[0].curve, 0);
        assert!((spins[0].degrees.abs() - 4.0).abs() < 1e-3, "{}", spins[0].degrees);
        assert_eq!(uv_wave(&objs[1], &ctx), [0.0, 0.005, 1.0, 0.005]);
        assert!(wave_spins(&objs[1], &ctx).is_empty());
    }

    fn real_tweaks(id: u32) -> Option<super::super::Tweaks> {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        super::super::Tweaks::load(&dir, id, true)
    }

    /// `Tweak_Rubi-Ka_HorizonClouds.txt` (pf 4582 `!include`s it): `HorizonClouds`' own `StateBlob` sets no `SRCBLEND`/
    /// `DESTBLEND`, so it stays a plain `SRCALPHA` alpha blend through its own (soft) texture alpha; `HorizonCloudsRim`'s
    /// explicit `SRCBLEND SRCCOLOR`/`DESTBLEND ONE` must select the self-modulated glow, not a flat `SRCALPHA` additive wash
    /// (the regression for the hard-edged saturated-blue night wall at ICC pf 4582).
    #[test]
    fn real_horizon_clouds_rim_self_modulates_while_the_base_layer_alpha_blends() {
        let Some(t) = real_tweaks(4582) else { return };
        let objs = t.objects();
        let clouds = objs.iter().find(|o| o.name == "HorizonClouds").expect("HorizonClouds");
        let rim = objs.iter().find(|o| o.name == "HorizonCloudsRim").expect("HorizonCloudsRim");
        assert_eq!(clouds.states.get("SRCBLEND"), None);
        assert_eq!(rim.states.get("SRCBLEND").map(String::as_str), Some("e_D3DBLEND_SRCCOLOR"));
        assert_eq!(rim.states.get("DESTBLEND").map(String::as_str), Some("e_D3DBLEND_ONE"));
        assert!(mesh_states(clouds).contains(&(27, 1)));
        assert!(mesh_states(rim).contains(&(19, 3)));
        assert!(mesh_states(rim).contains(&(20, 2)));
    }

    /// Native frame fog survives per-object Set/Reset; GenericMesh resource loading
    /// clears cloud material emission before lighting and texture MODULATE2X.
    #[test]
    fn real_sky_materials_preserve_native_fog_emissive_blend_and_alpha() {
        let Some(t) = real_tweaks(4582) else { return };
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let (Ok(store), objs) = (ao_rdb::RecordStore::open(&dir), t.objects()) else { return };
        let Ok(names) = NameTable::load(&store) else { return };
        let sky = super::super::Sky::new(&t, 3240.0).expect("noon sky"); // noon: the brightest CloudLightCurrent of the day
        let sorts = objs.iter().find(|o| o.name == "BackgroundSort").map_or_else(HashMap::new, |o| {
            o.fields.iter().filter_map(|(k, v)| Some((k.clone(), script::eval(v, &|_| None)? as u32))).collect()
        });
        let ctx = Ctx {
            cloud_intensity: sky.weather.thick_clouds_intensity(),
            hq_offset: sky.hq_offset,
            delta_time: 0.0,
            wind: [0.0; 2],
            night: sky.night,
            counters: counters(&objs),
            waves: [0.0; 16],
            ..super::super::game_context(&objs, sky.day_time)
        };
        let mut scene = Scene::default();
        let mut b = Builder { store: &store, names, ctx, objs: &objs, sky: &sky, scene: &mut scene, sorts };

        let clouds = objs.iter().find(|o| o.name == "HorizonClouds").expect("HorizonClouds");
        let mesh = b.mesh_object(clouds).expect("HorizonClouds mesh");
        assert!(mesh.vertices.iter().all(|v| v.color[3] == 1.0), "cloud alpha must not acquire an invented horizon haze");
        assert!(mesh.submeshes.iter().all(|s| !s.two_sided), "base cloud material keeps native default culling");
        assert!(mesh.submeshes.iter().all(|s| s.emissive == [0.0; 3] && s.specular == [0.0; 3]), "10061505 resets the generic resource's first material");
        let n_submeshes = mesh.submeshes.len();
        let fogged = b.fogged(clouds, mesh);
        assert_eq!(fogged.submeshes.len(), n_submeshes, "source fog does not add overlay geometry");
        assert!(fogged.submeshes.iter().all(|s| s.sky_fog && s.sky_fog_color.is_none()), "missing FOGENABLE inherits frame fog");
        let mut unfogged = clouds.clone();
        unfogged.states.insert("FOGENABLE".into(), "e_FALSE".into());
        let mesh = b.mesh_object(&unfogged).unwrap();
        assert!(b.fogged(&unfogged, mesh).submeshes.iter().all(|s| !s.sky_fog));
        let mut no_cull = clouds.clone();
        no_cull.states.insert("CULLMODE".into(), "e_D3DCULL_NONE".into());
        assert!(b.mesh_object(&no_cull).unwrap().submeshes.iter().all(|s| s.two_sided));
        let horizon = objs.iter().find(|o| o.name == "Horizon").expect("Horizon");
        let dish = b.mesh_object(horizon).expect("horizon dish");
        assert!(dish.submeshes.iter().all(|s| s.blend == Blend::Opaque), "explicit opaque Horizon must remain opaque at its rim");
        assert!(dish.vertices.iter().all(|v| v.color[3] == 1.0));
        let universe = objs.iter().find(|o| o.name == "Universe").expect("Universe");
        let backdrop = b.mesh_object(universe).expect("Universe mesh");
        assert!(backdrop.submeshes.iter().all(|s| s.blend == Blend::Opaque));
        assert!(backdrop.vertices.iter().all(|v| v.color[3] == 1.0), "no below-horizon backdrop fade");
        assert!(backdrop.submeshes.iter().all(|s| s.emissive == [0.0; 3]), "LIGHTING FALSE ignores the asset's emissive1");
        let bounds: [[f32; 2]; 3] = std::array::from_fn(|k| {
            dish.vertices.iter().fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], v| [lo.min(v.pos[k]), hi.max(v.pos[k])])
        });
        assert!(bounds[1][1] - bounds[1][0] < 200.0, "native factory + source-ordered tweak must keep the dish horizontal: {bounds:?}");
        assert!(bounds[0][1] - bounds[0][0] > 1900.0 && bounds[2][1] - bounds[2][0] > 1900.0, "{bounds:?}");

        let rim = objs.iter().find(|o| o.name == "HorizonCloudsRim").expect("HorizonCloudsRim");
        let rim_mesh = b.mesh_object(rim).expect("HorizonCloudsRim mesh");
        assert!(rim_mesh.submeshes.iter().all(|s| s.two_sided), "rim material explicitly sets CULL_NONE");
        assert!(rim_mesh.submeshes.iter().all(|s| s.blend == Blend::SourceColorOne));
        for name in ["Moon1", "Moon2", "Moon3"] {
            let moon = objs.iter().find(|o| o.name == name).expect(name);
            assert_eq!(moon.flag("ALPHABLENDENABLE"), Some(false));
            assert!(b.mesh_object(moon).unwrap().submeshes.iter().all(|s| s.blend == Blend::AlphaBlend), "moon material enables blending after the script disables it");
        }
        let moon_texture = b.scene.textures.get(&TextureKey { rdb_type: TEXTURES, id: 16807 }).expect("moon texture");
        assert!(moon_texture.rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 0), "moon texture has a real transparent mask: opaque blending is not equivalent");
        assert!(moon_texture.rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 255));
        assert!(rim_mesh.submeshes.iter().all(|s| s.emissive == [0.0; 3] && s.base_color[..3] == [srgb_to_linear(2.0); 3]), "native reset precedes lighting; MODULATE2X follows saturation");
        let original_vertices = rim_mesh.vertices.clone();
        let mut culled = rim.clone();
        culled.states.insert("CULLMODE".into(), "e_D3DCULL_CW".into());
        assert!(b.mesh_object(&culled).unwrap().submeshes.iter().all(|s| s.two_sided), "material CULL_NONE applies after the object's CULL_CW");
        let original_indices = rim_mesh.submeshes[0].indices.clone();
        let original_texture = rim_mesh.submeshes[0].texture;
        let fogged = b.fogged(rim, rim_mesh);
        assert_eq!(fogged.submeshes.len(), 1, "source fog must never add a destination-darkening overlay");
        assert_eq!(fogged.vertices.len(), original_vertices.len());
        let material = &fogged.submeshes[0];
        assert_eq!(material.blend, Blend::SourceColorOne);
        assert_eq!(material.texture, original_texture);
        assert_eq!(material.indices, original_indices);
        assert!(material.sky_fog);
        assert_eq!(material.sky_fog_color, Some([0.0; 3]));
        for (v, original) in fogged.vertices.iter().zip(original_vertices) {
            assert_eq!(v.color, original.color, "source fog must preserve the texture/vertex alpha mask");
            assert_eq!(v.normal, original.normal, "fog uses camera depth in the vertex shader, not overwritten normals");
        }
        for (name, alpha) in [("BattleStation_01", 48), ("BattleStation_02", 128), ("BattleStation_03", 128), ("BattleStation_04", 128)] {
            let station = objs.iter().find(|o| o.name == name).expect("BattleStation");
            let station_mesh = b.mesh_object(station).expect("station");
            let factor = script::unsigned(&objs, station, &b.ctx, station.field("TFACTOR").unwrap(), 0).unwrap();
            assert_eq!(factor >> 24, alpha);
            assert!([16, 8, 0].iter().all(|shift| (factor >> shift) & 255 > 0), "authored atmosphere tint must not become black");
            for submesh in station_mesh.submeshes {
                assert!((submesh.base_color[3] - alpha as f32 / 255.0).abs() < 1e-6);
                assert!(submesh.base_color[..3].iter().all(|&channel| channel > 0.0));
                assert_eq!(submesh.base_color[..3], [16, 8, 0].map(|s| srgb_to_linear(((factor >> s) & 255) as f32 / 255.0)));
            }
        }
        let dots = objs.iter().find(|o| o.name == "DotStars").expect("DotStars");
        let points = b.dot_points(dots).expect("native dots");
        let points = b.fogged(dots, points);
        assert_eq!(points.vertices.len(), 500);
        assert!(points.vertices.iter().all(|v| v.color == ao_scene::WHITE));
        let material = &points.submeshes[0];
        assert!(material.sky_points && material.sky_fog);
        assert_eq!(material.sky_fog_color, Some([0.0; 3]));
        assert_eq!(material.uv_scroll, [0.0; 2]);
        assert_eq!(material.indices, (0..500).collect::<Vec<u32>>());
    }
}
