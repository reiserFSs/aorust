//! Time-of-day sky, fog tint and sun light, driven by the client's tweak scripts (`cd_image/twk/*.txt`).
//!
//! The client builds its sky from FXS tweak objects, not from playfield data: `Tweak_Playfield_<id>.txt` (or
//! `Tweak_Playfield_OutdoorDefault/IndoorDefault.txt`) `!include`s `Tweak_Rubi-Ka_Atmosphere.txt`,
//! `Tweak_Rubi-Ka_SunLight.txt`, `Tweak_Rubi-Ka_Sun.txt` ... (Shadowlands: `Tweak_Shadowlands_*`). Colour tracks are
//! float arrays indexed by `GAME.DayTimeFactor = GameDayTime / 6480` (`Tweak_GAME.txt`), expressions are evaluated left to
//! right (`... + GAME.HighAltitudeWindX % 1` is `(a + b) % 1`).
//!
//! * `Atmosphere`: a camera-locked 6 vertex strip (`ZFUNC ALWAYS`, no z write, `FOGENABLE`) with colours `ColorBottom` at the
//!   horizon and `ColorTop` above, alpha = the `I` track. `AddFog(avg(bottom, middle) * avg(I), 0.025)` tints the fog.
//! * `SunLight`: `GroundLightCurrent = min(2 * GroundLight[t], 1)` is the sun colour, `AmbientLightCurrent` competes with the
//!   record ambient (`VisualAmbientLight_t::AddAmbientLight` keeps the maximum, DisplaySystem @0x10059d2c).
//! * `Sun1`: additive `newsun_frame01.png` rays, colour (255,155,55), rotated by `GAME.Sun1Rotation`.
//!
//! The game writes `GameDayTime` and the sun rotations from the server clock (not available offline); we evaluate at the
//! values of `Tweak_GAME_FrozenTime.txt` (`CurrentDayTime 2648.69`, `Sun1Rotation q(0.900351, -0.0951056, 0.344078, 0.248863)`, components x y z w),
//! the client's own fixed-time debug setting.

mod aurora;
mod layers;
mod script;
mod traffic;

use std::collections::HashSet;
use std::path::Path;
use ao_scene::{Mesh, Scene, Submesh, Vertex};
use super::environment::{srgb_to_linear, VIEW_DISTANCE};

/// `Tweak_GAME_FrozenTime.txt` `CurrentDayTime`: the time `load_playfield` uses.
pub const DEFAULT_DAY_TIME: f32 = 2648.69;
/// `GAME.CurrentDayTime / 6480` (27 * 60 * 4) is the factor that indexes every colour track.
pub(super) const DAY_LENGTH: f32 = 6480.0;
/// `Sun1Rotation` of `Tweak_GAME_FrozenTime.txt` as written, (x, y, z, w) (see [`rot`]), valid at [`DEFAULT_DAY_TIME`].
const SUN1_ROT: [f32; 4] = [0.900351, -0.0951056, 0.344078, 0.248863];
/// `Sun2Rotation` of the same file.
const SUN2_ROT: [f32; 4] = [0.836913, -0.16263, 0.384351, 0.354122];
/// Day time factor of solar noon [FIT]: midway between the sunrise and sunset edges of the `GroundLight` tracks.
const NOON: f32 = 0.525;
/// Seconds of 60 Hz wind history simulated before the weather of a static scene is read [GUESS]: the wind is a random walk
/// started when the client enters the playfield (`weather` module docs), so there is no single faithful value.
pub const WIND_WARMUP: f32 = 30.0;
/// `AddFogI` of the atmosphere object.
pub(super) const ATMOSPHERE_FOG_DENSITY: f32 = 0.025;

/// All tweak files a playfield includes, concatenated (comments removed).
pub struct Tweaks(String);

impl Tweaks {
    pub fn load(client_dir: &Path, id: u32, outdoor: bool) -> Option<Self> {
        let dir = client_dir.join("cd_image/twk");
        let own = format!("Tweak_Playfield_{id}.txt");
        let first = if dir.join(&own).is_file() { own } else if outdoor { "Tweak_Playfield_OutdoorDefault.txt".into() } else { "Tweak_Playfield_IndoorDefault.txt".into() };
        let mut text = String::new();
        flatten(&dir, &first, &mut HashSet::new(), &mut text, false);
        (!text.is_empty()).then_some(Tweaks(text))
    }

    /// Every `Object` of the flattened script.
    pub fn objects(&self) -> Vec<script::Obj> {
        script::parse_objects(&self.0)
    }

    /// Values of `Float <name> [N]: ...` (first definition).
    fn array(&self, name: &str) -> Option<Vec<f32>> {
        let mut lines = self.0.lines().peekable();
        while let Some(l) = lines.next() {
            let Some(rest) = l.trim_start().strip_prefix("Float") else { continue };
            let rest = rest.trim_start();
            let end = rest.find(|c: char| c.is_whitespace() || c == '[' || c == ':').unwrap_or(rest.len());
            if &rest[..end] != name || !rest[end..].trim_start().starts_with('[') {
                continue;
            }
            let mut expr = rest[rest.find(':')? + 1..].to_string();
            while let Some(n) = lines.peek() {
                let t = n.trim_start();
                if !t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') {
                    break;
                }
                expr.push(' ');
                expr.push_str(t);
                lines.next();
            }
            return Some(eval_list(&expr));
        }
        None
    }

    /// Array `name` at day time factor `f` (linear interpolation over the entries, [INFERENCE]: FXS array access).
    fn at(&self, name: &str, f: f32) -> Option<f32> {
        let a = self.array(name)?;
        (!a.is_empty()).then(|| sample(&a, f))
    }

    fn rgb(&self, names: [&str; 3], f: f32) -> Option<[f32; 3]> {
        Some([self.at(names[0], f)?, self.at(names[1], f)?, self.at(names[2], f)?])
    }
}

/// `in_object`: the file is the body of an `Object` (an include inside an object block). Those are templates that every
/// object needs its own copy of (`Template_Spaceship_*` in each ship), so only top-level includes are read once.
fn flatten(dir: &Path, name: &str, seen: &mut HashSet<String>, out: &mut String, in_object: bool) {
    if in_object {
        if seen.len() > 64 {
            return;
        }
    } else if !seen.insert(name.to_string()) || seen.len() > 64 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(dir.join(name)) else { return };
    let mut depth = in_object as u32;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        match line.trim() {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
        if let Some(inc) = line.trim().strip_prefix("!include") {
            let inc = inc.trim().trim_start_matches('{').trim_end_matches('}').trim();
            if in_object || depth > 0 {
                // a template inside an object: no `seen` bookkeeping, nesting is bounded by the include chain length
                let mut chain = seen.clone();
                if chain.insert(format!("{name}>{inc}")) {
                    flatten(dir, inc, &mut chain, out, true);
                }
            } else {
                flatten(dir, inc, seen, out, false);
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// `a, b / 255, c * 0.5 ...` (commas optional): every value is evaluated left to right.
fn eval_list(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let mut op = None;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_digit() || c == '.' || (c == '-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == b'.') && op.is_none()) {
            let st = i;
            i += 1;
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            let v: f32 = s[st..i].parse().unwrap_or(0.0);
            if i < b.len() && b[i] == b'f' {
                i += 1;
            }
            match (op.take(), out.last_mut()) {
                (Some('/'), Some(l)) => *l /= v,
                (Some('*'), Some(l)) => *l *= v,
                (Some('+'), Some(l)) => *l += v,
                (Some('-'), Some(l)) => *l -= v,
                _ => out.push(v),
            }
            continue;
        }
        if "/*+-".contains(c) && !out.is_empty() {
            op = Some(c);
        } else if c == ',' {
            op = None;
        }
        i += 1;
    }
    out
}

fn sample(a: &[f32], f: f32) -> f32 {
    let p = f.clamp(0.0, 1.0) * (a.len() - 1) as f32;
    let (i, t) = (p as usize, p.fract());
    a[i] * (1.0 - t) + a[(i + 1).min(a.len() - 1)] * t
}

/// Everything the tweak scripts contribute to a scene (colours in sRGB 0..1 like the client's D3D values).
pub struct Sky {
    /// `ColorTop/Bottom` and their intensity `I` (the vertex alpha of the atmosphere strip), sRGB.
    top: [f32; 3],
    bottom: [f32; 3],
    top_i: f32,
    bottom_i: f32,
    /// `AddFog` colour of the atmosphere object (sRGB).
    pub fog: [f32; 3],
    /// `GroundLightCurrent` (sRGB) and `AmbientLightCurrent`.
    pub sun: Option<[f32; 3]>,
    pub ambient: Option<[f32; 3]>,
    /// `CloudLightCurrent` (sRGB), white when the script has no cloud light track.
    cloud_light: [f32; 3],
    /// Unit vector towards the sun in scene space.
    pub sun_dir: [f32; 3],
    /// Unit vectors towards sun 1 and sun 2 in AO space (left handed).
    sun_ao: [f32; 3],
    sun2_ao: [f32; 3],
    /// `GAME.CurrentDayTime` the sky was evaluated at.
    day_time: f32,
    /// `GAME.CurrentNightIntensity` (`NightIntensity` track of `Tweak_GAME.txt`, 1 when the script has none).
    night: f32,
    /// `ThickCloudsIntensity` / `HighAltitudeWind` source (`crate::weather`); clear sky until [`Sky::set_weather`].
    pub weather: crate::weather::State,
}

/// `DayTimeForGroundShadows = GameDayTime * 15`.
pub fn ground_shadow_time(day_time: f32) -> f32 {
    day_time * 15.0
}

pub fn day_factor(day_time: f32) -> f32 {
    day_time / DAY_LENGTH
}

/// `q(a, b, c, d)` of the scripts: FXS stores the four values as they are written, i.e. (x, y, z, w)
/// (`FXS.dll` `FUN_10009c3b` copies the expressions in order; `RRefFrame::SetRotation` stores x, y, z, w).
fn rot(q: [f32; 4]) -> script::Quat {
    script::Quat { x: q[0], y: q[1], z: q[2], w: q[3] }
}

/// Where a `GAME.SunNRotation` points the sun: the `SunRays` fan sits at local z = -100 (`FUN_1005a7f6` DisplaySystem).
const SUN_LOCAL: [f32; 3] = [0.0, 0.0, -1.0];

/// Unit vector towards sun 1 in AO space at `day_time`. The server clock and the game's sun ephemeris are not available
/// offline [FIT]: the sun runs on a great circle at one turn per day whose horizon crossings are at `NOON ± 0.25` (the
/// `GroundLight` tracks switch on at factor ~0.27 and off at ~0.79) and that passes through the direction of the frozen
/// `Sun1Rotation` at the frozen time, which fixes the noon elevation and azimuth.
fn sun_ao(day_time: f32) -> [f32; 3] {
    let d0 = rot(SUN1_ROT).rotate(SUN_LOCAL);
    let w0 = std::f32::consts::TAU * (day_factor(DEFAULT_DAY_TIME) - NOON);
    let noon_elevation = (d0[1] / w0.cos()).clamp(-1.0, 1.0).asin();
    let (sin_e, cos_e) = noon_elevation.sin_cos();
    // azimuth of the noon sun: that of the frozen sun minus the azimuth swept since noon
    let a = d0[0].atan2(d0[2]) - w0.sin().atan2(w0.cos() * cos_e);
    let w = std::f32::consts::TAU * (day_factor(day_time) - NOON);
    let (sin_w, cos_w) = w.sin_cos();
    [cos_w * cos_e * a.sin() + sin_w * a.cos(), sin_e * cos_w, cos_w * cos_e * a.cos() - sin_w * a.sin()]
}

/// Sun 2 keeps its frozen offset from sun 1 in sun 1's own (azimuth, elevation) frame [INFERENCE: its ephemeris is not
/// stored, the two frozen rotations are 13 degrees apart].
fn sun2_ao(day_time: f32) -> [f32; 3] {
    // basis (to the side, up along the sky) around a sun direction
    let basis = |d: [f32; 3]| {
        let side = [d[2], 0.0, -d[0]];
        let l = (side[0] * side[0] + side[2] * side[2]).sqrt().max(1e-6);
        let side = side.map(|c| c / l);
        let up = [d[1] * side[2] - d[2] * side[1], d[2] * side[0] - d[0] * side[2], d[0] * side[1] - d[1] * side[0]];
        (side, up)
    };
    let dot = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| a[k] * b[k]).sum::<f32>();
    let (d1, d2) = (rot(SUN1_ROT).rotate(SUN_LOCAL), rot(SUN2_ROT).rotate(SUN_LOCAL));
    let (side, up) = basis(d1);
    let (c, a, b) = (dot(d2, d1), dot(d2, side), dot(d2, up));
    let d = sun_ao(day_time);
    let (side, up) = basis(d);
    std::array::from_fn(|k| c * d[k] + a * side[k] + b * up[k])
}

/// Unit vector towards sun 1 in scene space (`z` mirrored) at `day_time`.
pub fn sun_dir(day_time: f32) -> [f32; 3] {
    let d = sun_ao(day_time);
    [d[0], d[1], -d[2]]
}

impl Sky {
    pub fn new(t: &Tweaks, day_time: f32) -> Option<Sky> {
        let f = day_factor(day_time);
        let colour = |side: &str| -> Option<([f32; 3], f32)> {
            let i = t.at(&format!("Color{side}I"), f)?;
            let c = t.rgb([&format!("Color{side}R"), &format!("Color{side}G"), &format!("Color{side}B")].map(|s| s.as_str()), f)?;
            Some((c, i))
        };
        let ((top, top_i), (bottom, bottom_i)) = (colour("Top")?, colour("Bottom")?);
        // AddFogIntensity = (bottom I + middle I) / 2, AddFogR = (bottom R + middle R) / 2 * intensity (left to right)
        let (bi, mi) = (t.at("ColorBottomI", f)?, t.at("ColorMiddleI", f)?);
        let (b, m) = (t.rgb(["ColorBottomR", "ColorBottomG", "ColorBottomB"], f)?, t.rgb(["ColorMiddleR", "ColorMiddleG", "ColorMiddleB"], f)?);
        let fog = [0, 1, 2].map(|k| ((b[k] + m[k]) / 2.0 * (bi + mi) / 2.0).clamp(0.0, 1.0));
        let sun = t.rgb(["GroundLightR", "GroundLightG", "GroundLightB"], f).map(|c| c.map(|v| (v * 2.0).min(1.0)));
        let ambient = t.at("AmbientLight", f).map(|a| [a; 3]).or_else(|| t.rgb(["AmbientLightR", "AmbientLightG", "AmbientLightB"], f));
        let cloud_light = t.rgb(["CloudLightR", "CloudLightG", "CloudLightB"], f).unwrap_or([1.0; 3]);
        Some(Sky { top, bottom, top_i, bottom_i, fog, sun, ambient, cloud_light, sun_dir: sun_dir(day_time), sun_ao: sun_ao(day_time), sun2_ao: sun2_ao(day_time), day_time, night: t.at("NightIntensity", f).unwrap_or(1.0), weather: crate::weather::State::clear() })
    }

    /// The weather of the playfield at this moment (`ThickCloudsIntensity`, `HighAltitudeWind`).
    pub fn set_weather(&mut self, w: crate::weather::State) {
        self.weather = w;
    }

    /// Camera-locked sky dome (`Scene::sky`, drawn unlit and unfogged by the renderer): the atmosphere strip's gradient
    /// fogged at render time with the live fog (the client draws the strip with `FOGENABLE`; `Submesh::sky_fog`, vertex normal.x = distance). Vertex alpha is the strip's
    /// intensity `I`: the dome lets the star dome, dot stars and moons behind it show through at night.
    pub fn dome(&self) -> Mesh {
        const SEG: usize = 24;
        const ELEV: [f32; 12] = [-15.0, 0.0, 4.0, 9.0, 16.0, 25.0, 35.0, 45.0, 55.0, 65.0, 78.0, 90.0];
        let radius = 400.0;
        let mut vertices = Vec::new();
        let (bottom, top) = (self.bottom.map(srgb_to_linear), self.top.map(srgb_to_linear));
        for &e in &ELEV {
            let (er, tan) = (e.to_radians(), e.to_radians().tan());
            // ray from the camera hits the strip (y = 0 @ z = 1000 -> y = 400 @ z = 200 -> y = 400 @ z = -500)
            let s = if e <= 0.0 { 0.0 } else { (1000.0 * tan / (400.0 + 800.0 * tan)).min(1.0) };
            let dist = if e <= 0.0 { 1000.0 } else if s < 1.0 { (400.0 * s).hypot(1000.0 - 800.0 * s) } else { 400.0 / er.sin() };
            let d = dist / 1000.0 * VIEW_DISTANCE;
            let c = [0, 1, 2].map(|k| bottom[k] * (1.0 - s) + top[k] * s);
            let alpha = self.bottom_i * (1.0 - s) + self.top_i * s;
            for j in 0..=SEG {
                let a = j as f32 / SEG as f32 * std::f32::consts::TAU;
                vertices.push(Vertex {
                    pos: [radius * er.cos() * a.cos(), radius * er.sin(), radius * er.cos() * a.sin()],
                    normal: [d, 0.0, 0.0],
                    color: [c[0], c[1], c[2], alpha],
                    ..Default::default()
                });
            }
        }
        let mut indices = Vec::new();
        for r in 0..ELEV.len() - 1 {
            for j in 0..SEG {
                let a = (r * (SEG + 1) + j) as u32;
                let b = a + SEG as u32 + 1;
                indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        Mesh { vertices, submeshes: vec![Submesh { two_sided: true, blend: ao_scene::Blend::AlphaBlend, sky_fog: true, ..Submesh::new(indices, None) }] }
    }
}

/// Adds the sky of the playfield's tweak script to `scene.sky`: the atmosphere dome and every `BackgroundSort` object
/// (see `layers`).
pub fn emit(sky: &Sky, tweaks: &Tweaks, store: &ao_rdb::RecordStore, scene: &mut Scene, fog_color: [f32; 3], fog_end: f32) {
    layers::emit(sky, &tweaks.objects(), store, scene, sky.dome(), layers::Fog { color: fog_color, start: super::environment::NEAR, end: fog_end });
}

/// Re-evaluates an outdoor playfield's sky, fog tint and sun/ambient light at any `day_time` (the live time-of-day mode of the
/// viewer). Textures already handed out are not sent again, so a long run only ships the vertex colours that change.
pub struct SkyClock {
    store: ao_rdb::RecordStore,
    tweaks: Tweaks,
    env: super::environment::Env,
    sent: HashSet<ao_scene::TextureKey>,
}

impl SkyClock {
    /// `None` for indoor playfields and playfields without a tweak script.
    pub fn open(client_dir: &Path, id: u32) -> anyhow::Result<Option<SkyClock>> {
        use anyhow::Context;
        let store = ao_rdb::RecordStore::open(client_dir)?;
        let raw = store.get(super::RECORD, id)?.with_context(|| format!("no playfield {id}"))?;
        let rec = super::record::parse(&raw)?;
        if !rec.is_outdoor() {
            return Ok(None);
        }
        let mut tail = super::record::Rd::new(&raw, rec.tail);
        super::water::parse(&mut tail)?;
        let env = super::environment::parse(&mut tail)?;
        Ok(Tweaks::load(client_dir, id, true).map(|tweaks| SkyClock { store, tweaks, env, sent: HashSet::new() }))
    }

    /// A scene with only `sky`, `meshes`, new `textures`, `environment` and the base `fog_model` (no volumes) filled in.
    pub fn at(&mut self, day_time: f32) -> Scene {
        let day_time = day_time.rem_euclid(DAY_LENGTH);
        let mut scene = Scene::default();
        let Some(mut sky) = Sky::new(&self.tweaks, day_time) else { return scene };
        sky.set_weather(weather_at(&self.env, day_time));
        let environment = super::environment::to_scene(&self.env, true, Some(&sky));
        emit(&sky, &self.tweaks, &self.store, &mut scene, environment.fog_color, environment.fog_end);
        scene.fog_model = Some(super::environment::fog_model(&self.env, Some(&sky), vec![]));
        scene.environment = Some(environment);
        scene.textures.retain(|k, _| self.sent.insert(*k));
        scene
    }
}

/// The weather a static scene shows at `day_time`: the client's schedule of the playfield's `EnvironmentData` for the offline
/// game day ([`crate::weather::OFFLINE_DAY`]) plus [`WIND_WARMUP`] seconds of wind.
pub(super) fn weather_at(env: &super::environment::Env, day_time: f32) -> crate::weather::State {
    crate::weather::Weather::sample(&env.raw, crate::weather::OFFLINE_DAY, day_time as f64, WIND_WARMUP).state()
}

/// A live weather for playfield `id` (`FUN_100bdb64` with the playfield's `EnvironmentData`): call
/// [`crate::weather::Weather::update`] every frame.
pub fn open_weather(store: &ao_rdb::RecordStore, id: u32) -> anyhow::Result<crate::weather::Weather> {
    use anyhow::Context;
    let raw = store.get(super::RECORD, id)?.with_context(|| format!("no playfield {id}"))?;
    let rec = super::record::parse(&raw)?;
    let mut tail = super::record::Rd::new(&raw, rec.tail);
    super::water::parse(&mut tail)?;
    let env = super::environment::parse(&mut tail)?;
    Ok(crate::weather::Weather::new(&env.raw, crate::weather::OFFLINE_DAY))
}

/// Adds the playfield's distant scenery (city skylines, traffic ships) to `scene.instances` / `scene.movers`, see
/// `layers::emit_distant`; `day_time` is the game day time the ships are posed at.
pub fn emit_distant(tweaks: &Tweaks, store: &ao_rdb::RecordStore, scene: &mut Scene, day_time: f32) {
    layers::emit_distant(&tweaks.objects(), store, scene, day_time);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_run_left_to_right_with_optional_commas() {
        assert_eq!(eval_list(" 0 / 255,  64 / 255, 1.0"), vec![0.0, 64.0 / 255.0, 1.0]);
        assert_eq!(eval_list("23 / 255 * 0.0   57 / 255 * 3  0.5"), vec![0.0, 57.0 / 255.0 * 3.0, 0.5]);
        assert_eq!(eval_list("-1.5, 2"), vec![-1.5, 2.0]);
    }

    #[test]
    fn arrays_follow_continuation_lines_and_interpolate() {
        let t = Tweaks("Object A\n{\n  Float  Foo [3]:  0 / 2, 1\n                  4\n  Float Bar: 1\n}\n".into());
        assert_eq!(t.array("Foo").unwrap(), vec![0.0, 1.0, 4.0]);
        assert!(t.array("Bar").is_none());
        assert!((t.at("Foo", 0.75).unwrap() - 2.5).abs() < 1e-6);
    }

    fn real_tweaks(id: u32) -> Option<Tweaks> {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        Tweaks::load(&dir, id, true)
    }

    #[test]
    fn real_rubi_ka_noon_is_a_bright_blue_sky() {
        let Some(t) = real_tweaks(566) else { return };
        let s = Sky::new(&t, DEFAULT_DAY_TIME).unwrap();
        assert!(s.top[2] > s.top[0] && s.bottom.iter().all(|&v| v > 0.3), "{:?} {:?}", s.top, s.bottom);
        assert!(s.sun.unwrap()[0] > 0.5);
        let d = sun_dir(DEFAULT_DAY_TIME);
        assert!(d[1] > 0.0 && (d.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn sun_orbit_hits_the_frozen_sample_and_sets_at_night() {
        // the frozen Sun1Rotation direction is reproduced exactly at the frozen time
        let frozen = rot(SUN1_ROT).rotate(SUN_LOCAL);
        let d = sun_ao(DEFAULT_DAY_TIME);
        assert!((0..3).all(|k| (d[k] - frozen[k]).abs() < 1e-4), "{d:?} {frozen:?}");
        // above the horizon between the track's sunrise and sunset, below it at midnight, always a unit vector
        let height = |f: f32| sun_ao(f * DAY_LENGTH)[1];
        assert!(height(NOON) > 0.3 && height(0.0) < -0.3 && height(0.97) < 0.0);
        assert!(height(0.28) > 0.0 && height(0.76) > 0.0 && height(0.22) < 0.0 && height(0.82) < 0.0);
        let u = sun_ao(1234.0);
        assert!((u.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);
        // sun 2 keeps its frozen separation from sun 1
        let sep = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| a[k] * b[k]).sum::<f32>().acos().to_degrees();
        let frozen2 = rot(SUN2_ROT).rotate(SUN_LOCAL);
        let (want, got) = (sep(frozen, frozen2), sep(sun_ao(5000.0), sun2_ao(5000.0)));
        assert!(want > 5.0 && (want - got).abs() < 0.05, "{want} {got}");
    }

    #[test]
    fn real_sky_clock_scrolls_clouds_moves_the_sun_and_ships_textures_once() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        let Ok(Some(mut clock)) = SkyClock::open(&dir, 566) else { return };
        let (noon, night) = (clock.at(3240.0), clock.at(0.0));
        assert!(!noon.sky.is_empty() && !noon.textures.is_empty() && night.textures.is_empty(), "textures are sent once");
        // the cloud dome drifts, the layers other than clouds do not
        let drifting = noon.meshes.iter().flat_map(|m| &m.submeshes).filter(|s| s.uv_scroll != [0.0, 0.0]).count();
        // clear weather (pf 566 has all-zero weights) leaves the cloud dome invisible, so it is dropped
        assert!(drifting <= 1, "only ThickClouds scrolls");
        let (e1, e2) = (noon.environment.unwrap(), night.environment.unwrap());
        assert!(e1.sun_dir[1] > 0.3 && e2.sun_dir[1] < -0.3, "{:?} {:?}", e1.sun_dir, e2.sun_dir);
    }

    #[test]
    fn real_city_skylines_are_placed_far_from_the_playfield() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        let (Some(t), Ok(store)) = (Tweaks::load(&dir, 566, true), ao_rdb::RecordStore::open(&dir)) else { return };
        let mut scene = Scene::default();
        emit_distant(&t, &store, &mut scene, DEFAULT_DAY_TIME);
        // Newland City includes the Old Athen skyline (a UniversePosition point), at its universe offset; the traffic ships
        // (`Scene::movers`) fly near the city
        let ships: Vec<usize> = scene.movers.iter().map(|m| m.instance).collect();
        assert!(scene.instances.len() > ships.len(), "{}", scene.instances.len());
        assert!(scene.instances.iter().enumerate().filter(|(i, _)| !ships.contains(i)).all(|(_, i)| i.transform[3][0].abs() + i.transform[3][2].abs() > 1000.0), "far away");
    }

    #[test]
    fn real_sky_objects_are_all_found_and_night_hides_the_atmosphere() {
        let Some(t) = real_tweaks(566) else { return };
        let objs = t.objects();
        let by = |n: &str| objs.iter().find(|o| o.name == n);
        assert_eq!(by("Moon2").unwrap().string("Mesh"), Some("moonmesh_small.abiff"));
        assert_eq!(by("ThickClouds").unwrap().string("Mesh"), Some("skydome_clouds.abiff"));
        assert_eq!(by("Horizon").unwrap().string("Mesh"), Some("horizon_object.abiff"));
        assert_eq!(by("Universe").unwrap().string("Mesh"), Some("nebulas_sphere.abiff"), "BP01 overrides the star dome");
        assert_eq!(by("Sun1").unwrap().texture.as_deref(), Some("newsun_frame01.png"));
        assert_eq!(by("DotStars").unwrap().texture.as_deref(), Some("clouds3.png"));
        assert!(by("DotStarsMesh").unwrap().field("Vertex").unwrap().matches("v(").count() == 5);
        assert!(by("RKPP").is_some() && by("PlayfieldData").unwrap().field("UniversePosition") == Some("RKPP.Newland_City"));
        let (noon, night) = (Sky::new(&t, 3300.0).unwrap(), Sky::new(&t, 0.0).unwrap());
        assert!(noon.top_i > 0.99 && night.top_i < 0.1, "{} {}", noon.top_i, night.top_i);
    }

    #[test]
    fn real_playfield_weather_drives_the_cloud_layer() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        let Ok(store) = ao_rdb::RecordStore::open(&dir) else { return };
        // 566 Newland City has no weather weights (clear), 560 has (docs: weather exists on 560, 590, 620, ...)
        let clear = open_weather(&store, 566).unwrap();
        assert_eq!(clear.state().thick_clouds_intensity(), 0.0);
        let mut seen = 0f32;
        for day in 0..4 {
            for step in 0..216 {
                let mut w = open_weather(&store, 560).unwrap();
                w.update(day, step as f64 * 30.0, 1.0 / 60.0);
                seen = seen.max(w.state().thick_clouds_intensity());
            }
        }
        assert!(seen > 0.0 && seen <= 1.0, "{seen}");
    }
}
