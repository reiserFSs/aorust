//! The sky objects of a playfield's tweak scripts, drawn camera-locked in `BackgroundSort` order (see `docs/formats.md`
//! § Sky): star dome / nebula (`Universe`), dot stars, moons, the atmosphere gradient, suns and flares (`e_SunRays`), thick
//! cloud dome, single clouds, the `Horizon` map and the Shadowlands vortex.
//!
//! All sky objects are `e_LockToCamera` with `ZFUNC ALWAYS` and no z write, so only their order and angular size matter:
//! meshes are baked at `Scale` times their authored size (the renderer pins the sky to the far plane).

use super::aurora;
use super::script::{self, Ctx, Obj, Quat};
use super::srgb_to_linear;
use crate::character::NameTable;
use crate::mesh::{decode_mesh_object_space, MESH_TYPE};
use crate::texture::decode_texture;
use ao_rdb::RecordStore;
use ao_scene::{Blend, Instance, Mesh, Scene, SkyColors, SkySpin, SkyWaveSpin, Submesh, TextureKey, Vertex, IDENTITY, WHITE};
use std::collections::HashMap;

const TEXTURES: u32 = 1_010_004;
/// Sort value of the atmosphere (`BackgroundSort.Atmosphere`) when the script defines no table.
const ATMOSPHERE_SORT: u32 = 7;

/// AO space (left handed) → scene space.
fn scene(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[1], -v[2]]
}

/// The playfield's fog (linear colour, metres).
pub struct Fog {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
}

struct Builder<'a> {
    fog: Fog,
    store: &'a RecordStore,
    names: NameTable,
    ctx: Ctx,
    objs: &'a [Obj],
    sky: &'a super::Sky,
    scene: &'a mut Scene,
    sorts: HashMap<String, u32>,
}

/// Adds the sky of `objs` (and the atmosphere `dome`) to `scene.sky`, back to front.
pub fn emit(sky: &super::Sky, objs: &[Obj], store: &RecordStore, scene: &mut Scene, dome: Mesh, fog: Fog) {
    let Ok(names) = NameTable::load(store) else { return };
    let sorts = objs.iter().find(|o| o.name == "BackgroundSort").map_or_else(HashMap::new, |o| {
        o.fields.iter().filter_map(|(k, v)| Some((k.clone(), script::eval(v, &|_| None)? as u32))).collect()
    });
    let ctx = Ctx {
        day_time: sky.day_time,
        sun1: Quat::between([0.0, 0.0, -1.0], sky.sun_ao),
        sun2: Quat::between([0.0, 0.0, -1.0], sky.sun2_ao),
        cloud_intensity: sky.weather.thick_clouds_intensity(),
        hq_offset: hq_offset(objs),
        delta_time: 0.0,
        wind: [0.0; 2],
        night: sky.night,
        counters: counters(objs),
        waves: [0.0; 16],
    };
    let mut b = Builder { fog, store, names, ctx, objs, sky, scene, sorts };
    // (sort, mesh, constant spin, aurora simulation, wave driven turns)
    type Layer = (u32, Mesh, Option<SkySpin>, Option<(ao_scene::aurora::GloomySky, f32)>, Vec<SkyWaveSpin>);
    let mut layers: Vec<Layer> = vec![(b.sorts.get("Atmosphere").copied().unwrap_or(ATMOSPHERE_SORT), dome, None, None, vec![])];
    for o in objs.iter().filter(|o| o.field("Priority").is_some_and(|p| p.trim() == "e_RenderPriority_PreRendering")) {
        let mut colors = None;
        let mesh = match o.fxid() {
            "GloomySky" => b.gloomy_sky(o).map(|(m, sim, gain)| {
                colors = Some((sim, gain));
                m
            }),
            "GenericMeshObject" => b.mesh_object(o).map(|m| b.fogged(o, m)),
            "SunRays" => b.sun_rays(o),
            "SingleCloud" => b.single_clouds(o),
            "GenericVisualObject" => b.dot_points(o).map(|m| b.fogged(o, m)),
            _ => None,
        };
        // a layer whose every vertex alpha is 0 (sun below the horizon, `TFACTOR` 0) draws nothing
        if let Some(m) = mesh.filter(|m| m.vertices.iter().any(|v| v.color[3] > 0.0)) {
            let spin = if o.fxid() == "GenericMeshObject" { b.spin(o) } else { None };
            layers.push((b.sort(o), m, spin, colors, wave_spins(o, &b.ctx)));
        }
    }
    layers.sort_by_key(|l| l.0);
    for (_, m, spin, colors, waves) in layers {
        scene.meshes.push(m);
        scene.sky_colors.extend(colors.map(|(sim, gain)| SkyColors { mesh: scene.meshes.len() - 1, sim, gain }));
        scene.sky.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
        scene.sky_spin.extend(spin.map(|s| SkySpin { instance: scene.sky.len() - 1, ..s }));
        scene.sky_wave_spin.extend(waves.into_iter().map(|w| SkyWaveSpin { instance: scene.sky.len() - 1, ..w }));
    }
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
        if !matches!(decode_mesh_object_space(store, id, &mut tmp), Ok(Some(_))) {
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
}

/// `ScaleType e_ScaleVisibleFarAway` (= 3): the client pulls the object in towards the camera every frame, see
/// [`ao_scene::far_away_pose`] (`FUN_1005c124`, the `GenericMeshObject` process function).
pub(super) fn is_far_away(o: &Obj) -> bool {
    o.field("ScaleType").is_some_and(|t| t.trim() == "e_ScaleVisibleFarAway")
}

/// The scale the client applies to a world-space mesh object. `GenericMeshObject` (`FUN_1005e716` @0x1005e716, DisplaySystem)
/// reads the `Scale` field **only while `ScaleType` is 0** (`FUN_1001d156` = `RRefFrame::SetScale`); `e_ScaleByViewDistance` = 1,
/// `e_ScaleByViewDistanceAndDirectionHeigh` = 2 and `e_ScaleVisibleFarAway` = 3 (value blocks of the FXS enum parser
/// @0x1000b015 / 0x1000c7f8 / 0x1000c810 / 0x1000c827) do not use the `Scale` field either: `FUN_1005e716` tests `type == 0`
/// only, and for type 3 the per-frame `FUN_1005c124` calls `SetScale` itself ([`ao_scene::far_away_pose`]). Outside its
/// 950..5000 m window that scale is 1, so the object keeps the reference frame's default scale 1 (the 2 ships with
/// `Scale 0.70`, `Alien_Craft_War_04/05`, are drawn at 1). The default of 1 is [INFERENCE]: the base `RRefFrame_t`
/// constructor lives in randy31.dll.
pub(super) fn object_scale(o: &Obj, ctx: &Ctx) -> f32 {
    if o.field("ScaleType").is_some_and(|t| t.trim().starts_with("e_Scale")) {
        1.0
    } else {
        script::float(o, ctx, "Scale", 1.0)
    }
}

/// AO <-> scene (z mirrored).
fn scene_v(v: [f32; 3]) -> [f32; 3] {
    scene(v)
}

/// Growth per second of every `Counter` field that integrates `GameDeltaTime` (`Counter: GAME.GameDeltaTime * 3 + This.Counter
/// % 360`), keyed `Object.Counter`; per-frame counters without a time term (`DotStars`) are not time driven here.
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

/// `PlayfieldData.UniversePosition` (`RKPP.<name>`) minus `RKPP.Omni_1_HQ`: `GAME.OffsetFromHQ_X/Z` of a camera standing at
/// the playfield's origin.
fn hq_offset(objs: &[Obj]) -> [f32; 2] {
    let rkpp = objs.iter().find(|o| o.name == "RKPP");
    let pos = |name: &str| -> Option<[f32; 3]> {
        let o = rkpp?;
        script::vector(o, &Ctx::at(0.0), o.field(name)?, 0)
    };
    let pf = objs.iter().find(|o| o.name == "PlayfieldData").and_then(|o| o.field("UniversePosition")).and_then(|e| e.trim().strip_prefix("RKPP."));
    match (pf.and_then(pos), pos("Omni_1_HQ")) {
        (Some(p), Some(h)) => [p[0] - h[0], p[2] - h[2]],
        _ => [0.0; 2],
    }
}

/// How an object is lit (`SunlightType`).
enum Light {
    Unlit,
    /// Directional light from the sun, colour `SpaceLight<n>Current`.
    Moon([f32; 3]),
    /// `CloudLightCurrent`, no direction term.
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

    /// `FOGENABLE`: the linear fog of the playfield, evaluated per vertex at the distance the object's authored radius has at
    /// the view distance (like the atmosphere strip). Opaque and blended layers get a fog coloured overlay (`FOGCOLOR`,
    /// default the world fog colour); additive ones fade out (their fog colour is black). Without an explicit state the
    /// scenery sorted at or after `BackgroundSort.Horizon` is fogged, everything before it is not [GUESS: the client's default
    /// fog state is not stored in the scripts; the horizon map must not show as a brown band behind a fogged terrain].
    fn fogged(&self, o: &Obj, mut mesh: Mesh) -> Mesh {
        let horizon = self.sorts.get("Horizon").copied().unwrap_or(u32::MAX);
        if !o.flag("FOGENABLE").unwrap_or_else(|| self.sort(o) >= horizon) {
            return mesh;
        }
        let amount = |v: &Vertex| {
            let d = v.pos.iter().map(|c| c * c).sum::<f32>().sqrt() / 1000.0 * super::VIEW_DISTANCE;
            ((d - self.fog.start) / (self.fog.end - self.fog.start).max(1e-3)).clamp(0.0, 1.0)
        };
        let colour = match o.states.get("FOGCOLOR").and_then(|c| u32::from_str_radix(c.trim_start_matches("0x"), 16).ok()) {
            Some(c) => [16, 8, 0].map(|s| srgb_to_linear(((c >> s) & 255) as f32 / 255.0)),
            None => self.fog.color,
        };
        let n = mesh.vertices.len() as u32;
        let overlay: Vec<Vertex> = mesh.vertices.iter().map(|v| Vertex { color: [colour[0], colour[1], colour[2], amount(v) * v.color[3]], ..*v }).collect();
        let originals = mesh.submeshes.len();
        for v in &mut mesh.vertices {
            let f = amount(v);
            if mesh.submeshes.iter().any(|s| s.blend == Blend::Additive) {
                v.color[3] *= 1.0 - f;
            }
        }
        if mesh.submeshes.iter().all(|s| s.blend == Blend::Additive) {
            return mesh;
        }
        mesh.vertices.extend(overlay);
        for i in 0..originals {
            if mesh.submeshes[i].blend != Blend::Additive {
                let idx = mesh.submeshes[i].indices.iter().map(|k| k + n).collect();
                mesh.submeshes.push(Submesh { two_sided: true, blend: Blend::AlphaBlend, ..Submesh::new(idx, None) });
            }
        }
        mesh
    }

    /// `e_GenericMeshObject`: the authored mesh, rotated by `Rotation`, scaled by `Scale`, with the object's blend state,
    /// texture scroll matrix, light colour and `TFACTOR` alpha baked in.
    fn mesh_object(&mut self, o: &Obj) -> Option<Mesh> {
        let id = self.names.id(MESH_TYPE, o.string("Mesh")?)?;
        let mut tmp = Scene::default();
        // the visual draws the mesh data as stored; the tweak `Rotation` supplies the orientation (Max Z-up -> world)
        decode_mesh_object_space(self.store, id, &mut tmp).ok()??;
        let mut mesh = tmp.meshes.pop()?;
        self.scene.textures.extend(tmp.textures);
        let q = o.field("Rotation").and_then(|e| script::rotation(o, &self.ctx, e, 0)).unwrap_or(Quat::IDENTITY);
        // `InitDirection`: where the object lies before `Rotation` (the moons: -X, the mesh itself is authored towards -Z)
        let q = match o.field("InitDirection").and_then(|e| script::vector(o, &self.ctx, e, 0)) {
            Some(init) => {
                let mut c = mesh.vertices.iter().fold([0.0f32; 3], |a, v| [a[0] + v.pos[0], a[1] + v.pos[1], a[2] - v.pos[2]]);
                let l = c.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
                c = c.map(|v| v / l);
                let il = init.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
                Quat::between(c, init.map(|v| v / il)).then(q)
            }
            None => q,
        };
        let scale = script::float(o, &self.ctx, "Scale", 1.0);
        let offset = o.field("Position").and_then(|e| script::vector(o, &self.ctx, e, 0)).unwrap_or([0.0; 3]);
        let light = self.light(o);
        let lit = o.flag("LIGHTING") != Some(false);
        let gain = if o.states.get("TSS_COLOROP").is_some_and(|v| v == "e_D3DTOP_MODULATE2X") { 2.0 } else { 1.0 };
        // `ALPHAOP SELECTARG2` with `ALPHAARG2 = TFACTOR`: the alpha is the top byte of `TFACTOR`
        let alpha = match (o.states.get("TSS_ALPHAARG2").map(String::as_str), o.field("TFACTOR")) {
            (Some("e_D3DTA_TFACTOR"), Some(e)) => script::eval_field(o, &self.ctx, e, 0).map_or(1.0, |t| ((t as u32) >> 24) as f32 / 255.0),
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
                Light::Moon(l) => {
                    let d = (0..3).map(|k| v.normal[k] * to_sun[k]).sum::<f32>().max(0.0);
                    l.map(|c| c * d)
                }
                Light::Cloud(l) => l,
            };
            // cloud layers thin out towards the horizon (haze) instead of ending in the straight line where the fogged horizon
            // dish starts [GUESS: the client's own cloud/fog blend was not found]
            let haze = match light {
                Light::Cloud(_) => (v.pos[1] / v.pos.iter().map(|c| c * c).sum::<f32>().sqrt().max(1e-6) / 0.25).clamp(0.0, 1.0),
                _ => 1.0,
            };
            v.color = [srgb_to_linear(c[0] * gain), srgb_to_linear(c[1] * gain), srgb_to_linear(c[2] * gain), alpha * haze];
            v.uv = [v.uv[0] * scroll[0] + v.uv[1] * scroll[2] + scroll[4], v.uv[0] * scroll[1] + v.uv[1] * scroll[3] + scroll[5]];
        }
        let uv_scroll = self.uv_scroll(o);
        let uv_wave = uv_wave(o, &self.ctx);
        for s in &mut mesh.submeshes {
            s.uv_scroll = uv_scroll;
            s.uv_wave = uv_wave;
            s.two_sided = true;
            s.blend = match (o.flag("ALPHABLENDENABLE"), o.states.get("DESTBLEND").map(String::as_str)) {
                (Some(true), Some("e_D3DBLEND_ONE")) => Blend::Additive,
                (Some(true), _) => Blend::AlphaBlend,
                (Some(false), _) => Blend::Opaque,
                (None, _) => s.blend,
            };
            if !lit {
                s.base_color = WHITE;
            }
        }
        // Opaque unlit backdrops (star dome / nebula belt, horizon dish, vortex) end in a straight line at elevation 0:
        // fade their rim out over the last 6 degrees below the horizon so the sky meets them without a seam [GUESS]
        if matches!(light, Light::Unlit) && mesh.submeshes.iter().all(|s| s.blend == Blend::Opaque) {
            if self.sort(o) < self.sorts.get("Atmosphere").copied().unwrap_or(ATMOSPHERE_SORT) {
                below_horizon_fade(&mut mesh);
            } else {
                rim_fade(&mut mesh);
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
        let offset = o.field("Position").and_then(|e| script::vector(o, &self.ctx, e, 0)).unwrap_or([0.0; 3]);
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
        let c = q.rotate([0.0, 0.0, -1.0]);
        let fade = if c[1] <= 0.0 { return None } else if c[1] < 0.01 { (100.0 * c[1]).powi(4) } else { 1.0 };
        let key = self.texture(o.texture.as_deref()?)?;
        let col = ["ColorR", "ColorG", "ColorB"].map(|n| script::float(o, &self.ctx, n, 255.0) / 255.0).map(srgb_to_linear);
        let size = script::float(o, &self.ctx, "Size", 6.0);
        let uv_size = script::float(o, &self.ctx, "UVSize", 0.8);
        let n = (script::float(o, &self.ctx, "Vertices", 33.0) as usize).clamp(4, 256);
        let blend = if o.states.get("DESTBLEND").is_some_and(|v| v == "e_D3DBLEND_ONE") { Blend::Additive } else { Blend::AlphaBlend };
        let color = [col[0], col[1], col[2], fade];
        let mut mesh = Mesh::default();
        // the sky is drawn on a sphere of 300 m, the client's fan sits at 100 units: scale by 3
        let mut push = |local: [f32; 3], uv: [f32; 2], rim: f32| {
            let p = q.rotate(local).map(|v| v * 3.0);
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

    /// `e_SingleCloud`, `Altitude` metres above the camera. The client (`VisualSingleCloud_t`, `FUN_1005485a` update,
    /// `FUN_10053e45` respawn) keeps 10 clouds (high detail; 2 in low detail) as randomly warped 10 x 10 vertex sheets inside a
    /// +-1600 m square around the camera that drift with the wind and respawn when they leave it; the sheet shapes, sizes
    /// and vertex colours are procedural **[UNRESOLVED]**: we place 10 fixed 260 x 130 m billboards at random points of
    /// the square (the textures are the client's `single_cloud01..04.png`).
    fn single_clouds(&mut self, o: &Obj) -> Option<Mesh> {
        let altitude = script::float(o, &self.ctx, "Altitude", 100.0);
        let keys: Vec<TextureKey> = (1..=4).filter_map(|n| self.texture(&format!("single_cloud0{n}.png"))).collect();
        if keys.is_empty() {
            return None;
        }
        let mut rng = Lcg(0x2545_F491);
        let tint = self.sky.cloud_light.map(srgb_to_linear);
        let mut mesh = Mesh::default();
        let mut per_key: Vec<Vec<u32>> = vec![vec![]; keys.len()];
        for i in 0..10 {
            let (x, z) = ((rng.next() * 2.0 - 1.0) * 1600.0, (rng.next() * 2.0 - 1.0) * 1600.0);
            let dir = [x, altitude, z];
            let len = x.hypot(z).hypot(altitude);
            let base = mesh.vertices.len() as u32;
            // a 260 x 130 m cloud seen at its distance
            quad(&mut mesh, scene(dir.map(|c| c / len)), (130.0 / len * 300.0, 65.0 / len * 300.0), [tint[0], tint[1], tint[2], 0.9], [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)]);
            per_key[i % keys.len()].extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        for (key, idx) in keys.into_iter().zip(per_key) {
            mesh.submeshes.push(Submesh { two_sided: true, blend: Blend::AlphaBlend, ..Submesh::new(idx, Some(key)) });
        }
        Some(mesh)
    }

    /// `e_GenericVisualObject` with a `<name>Mesh` blueprint (`DotStars`): `Vertices` dots at random points on the
    /// blueprint's surface (`e_RandomOnSurface`), scaled so its farthest vertex lies at `Size`, additive, each dot
    /// coloured by a random texel (`e_Random` texture mapping) of the object's texture.
    fn dot_points(&mut self, o: &Obj) -> Option<Mesh> {
        let bp = self.objs.iter().find(|b| b.name == format!("{}Mesh", o.name))?;
        let corners: Vec<[f32; 3]> = bp.field("Vertex")?.split("v(").skip(1).filter_map(|s| script::vector(bp, &self.ctx, &format!("v({}", s.split(')').next()?), 0)).collect();
        let tris: Vec<u32> = super::eval_list(bp.field("Indice")?).iter().map(|&v| v as u32).collect();
        let n = script::float(bp, &self.ctx, "Vertices", 0.0) as usize;
        let far = corners.iter().map(|c| c.iter().map(|v| v * v).sum::<f32>().sqrt()).fold(f32::MIN, f32::max);
        let key = self.texture(o.texture.as_deref()?)?;
        if corners.is_empty() || tris.len() < 3 || n == 0 || far <= 0.0 {
            return None;
        }
        let mut rng = Lcg(0x9E37_79B9);
        let mut mesh = Mesh::default();
        let mut idx = Vec::new();
        for _ in 0..n {
            let t = (rng.next() * (tris.len() / 3) as f32) as usize % (tris.len() / 3);
            let tri = [0, 1, 2].map(|k| corners[tris[t * 3 + k] as usize % corners.len()]);
            let (mut a, mut b) = (rng.next(), rng.next());
            if a + b > 1.0 {
                (a, b) = (1.0 - a, 1.0 - b);
            }
            let p: [f32; 3] = std::array::from_fn(|k| tri[0][k] + (tri[1][k] - tri[0][k]) * a + (tri[2][k] - tri[0][k]) * b);
            let l = p.iter().map(|v| v * v).sum::<f32>().sqrt();
            if l <= 0.0 {
                continue;
            }
            let uv = (rng.next(), rng.next());
            let base = mesh.vertices.len() as u32;
            quad(&mut mesh, scene(p.map(|c| c / l)), (300.0 * 0.1f32.to_radians(), 300.0 * 0.1f32.to_radians()), [1.0; 4], [uv; 4]);
            idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        mesh.submeshes.push(Submesh { two_sided: true, blend: Blend::Additive, ..Submesh::new(idx, Some(key)) });
        Some(mesh)
    }
}

/// Vertex alpha 1 at 6 degrees below the horizon and lower, 0 at elevation 0 and above; the
/// submeshes become alpha blended.
fn rim_fade(mesh: &mut Mesh) {
    for v in &mut mesh.vertices {
        let sin_el = v.pos[1] / v.pos.iter().map(|c| c * c).sum::<f32>().sqrt().max(1e-6);
        v.color[3] *= (-sin_el / 6f32.to_radians().sin()).clamp(0.0, 1.0);
    }
    mesh.submeshes.iter_mut().for_each(|s| s.blend = Blend::AlphaBlend);
}

/// Star dome / nebula belt: the client's atmosphere strip hides everything below the horizon (our dome only reaches 15 degrees
/// down), so the backdrop fades out from the horizon (alpha 1) to 10 degrees below it (0); alpha blended.
fn below_horizon_fade(mesh: &mut Mesh) {
    for v in &mut mesh.vertices {
        let sin_el = v.pos[1] / v.pos.iter().map(|c| c * c).sum::<f32>().sqrt().max(1e-6);
        v.color[3] *= (1.0 + sin_el / 10f32.to_radians().sin()).clamp(0.0, 1.0);
    }
    mesh.submeshes.iter_mut().for_each(|s| s.blend = Blend::AlphaBlend);
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

/// Camera facing quad of half width/height `half` (metres at 300 m) at `dir * 300`.
fn quad(mesh: &mut Mesh, dir: [f32; 3], half: (f32, f32), color: [f32; 4], uv: [(f32, f32); 4]) {
    let up = if dir[1].abs() > 0.99 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let norm = |a: [f32; 3]| {
        let l = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        a.map(|v| v / l)
    };
    let r = norm(cross(up, dir));
    let u = cross(dir, r);
    for ((sx, sy), (tu, tv)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].into_iter().zip(uv) {
        let pos = std::array::from_fn(|k| dir[k] * 300.0 + r[k] * sx * half.0 + u[k] * sy * half.1);
        mesh.vertices.push(Vertex { pos, normal: dir.map(|c| -c), uv: [tu, tv], color });
    }
}

/// Fixed seed generator (the sky must look the same on every load).
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Turns driven by `GameWaveCurve*` (`Sun1_Moving`: `RotMove = v(0,0,1), GAME.GameWaveCurve0 * 4 - 2`, `Rotation = This.RotMove [ROT]
/// GAME.Sun1Rotation`): per curve the rotation at `GameWaveCurve_k = 1` relative to the baked one at 0 (the expressions are
/// affine in the angle), turned by `degrees * curve(time)` in the renderer. `instance` is filled in by the caller.
fn wave_spins(o: &Obj, base: &Ctx) -> Vec<SkyWaveSpin> {
    let Some(e) = o.field("Rotation") else { return vec![] };
    let Some(q0) = script::rotation(o, base, e, 0) else { return vec![] };
    let offset = o.field("Position").and_then(|e| script::vector(o, base, e, 0)).unwrap_or([0.0; 3]);
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
    use super::*;

    #[test]
    fn self_referencing_waypoint_is_rejected() {
        let objs = super::super::script::parse_objects("Object RKPP\n{\n  Vector A v(1,2,3)\n}\nObject RKWP\n{\n  Vector P RKWP.P\n}\n");
        let o = objs[1].clone();
        assert_eq!(position_expr(&objs, &o, "RKWP.P"), None);
    }

    #[test]
    fn scale_field_is_ignored_unless_scale_type_is_zero() {
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
}
