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
//! The server clock drives the native binary-sun orbit (Gamecode 0x100b879b/0x100b9044).
//! Offline scenes use `Tweak_GAME_FrozenTime.txt`'s `CurrentDayTime 2648.69`; its authored
//! Sun1/Sun2 quaternions are independent regression fixtures, never ephemeris calibration inputs.

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
#[cfg(test)]
const SUN1_ROT: [f32; 4] = [0.900351, -0.0951056, 0.344078, 0.248863];
/// `Sun2Rotation` of the same file.
#[cfg(test)]
const SUN2_ROT: [f32; 4] = [0.836913, -0.16263, 0.384351, 0.354122];
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

    /// `Float <name>: <number>` (first definition).
    fn scalar(&self, name: &str) -> Option<f32> {
        self.0.lines().find_map(|l| l.trim_start().strip_prefix("Float")?.trim_start().strip_prefix(name)?.trim_start().strip_prefix(':')?.split_whitespace().next()?.parse().ok())
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
    /// `SpecularLightIntensity` of the sun light (`Float SpecularLightIntensity: 1.0`; 0.01 in `Tweak_Playfield_Alien_LandingCraft`).
    pub sun_specular: f32,
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
#[cfg(test)]
fn rot(q: [f32; 4]) -> script::Quat {
    script::Quat { x: q[0], y: q[1], z: q[2], w: q[3] }
}

/// Where a `GAME.SunNRotation` points the sun: the `SunRays` fan sits at local z = -100 (`FUN_1005a7f6` DisplaySystem).
#[cfg(test)]
const SUN_LOCAL: [f32; 3] = [0.0, 0.0, -1.0];

/// Native binary-sun positions (Gamecode 0x100b86ca/0x100b8ea6/0x100b9044).
/// `GameTime_t::GetCurrentRealTime` is game seconds within the 97200-second day;
/// DisplaySystem's `GameDayTime` is that value divided by the default time speed 15.
/// The secondary orbit makes six complete revolutions per day, so no game-day index is needed.
fn sun_positions(day_time: f32) -> [[f32; 3]; 2] {
    use script::Quat;
    let game_seconds = day_time.rem_euclid(DAY_LENGTH) * 15.0;
    // GC 0x1016749c = 0x3b72b9d6; 0x10158ba0 is the double multiplier 6.
    let speed = f32::from_bits(0x3b72b9d6);
    let primary_angle = 176.0 + speed * game_seconds;
    let secondary_angle = speed * 6.0 * game_seconds;
    // Native row-vector matrices: Ry(phase) * Rx(tilt) * Ry(azimuth).
    // Quat uses column vectors, hence the negative angles and reverse composition.
    let primary = Quat::axis_angle([0.0, 1.0, 0.0], -primary_angle)
        .then(Quat::axis_angle([1.0, 0.0, 0.0], -45.0))
        .then(Quat::axis_angle([0.0, 1.0, 0.0], -90.0))
        .rotate([0.0, 0.0, 70.0]);
    let secondary = Quat::axis_angle([0.0, 1.0, 0.0], -secondary_angle)
        .then(Quat::axis_angle([1.0, 0.0, 0.0], -145.0))
        .rotate([0.0, 0.0, 1.0]);
    [5.0, -15.0].map(|radius| {
        let p = std::array::from_fn::<_, 3, _>(|k| primary[k] + secondary[k] * radius);
        let length = p.iter().map(|v| v * v).sum::<f32>().sqrt();
        p.map(|v| v / length)
    })
}

/// Native SunNRotation look frame: forward = −sun, right = forward × world-up,
/// up = forward × right (GC 0x100b879b → 0x1013c1f7 → 0x1013c2eb).
/// Unlike a shortest-arc rotation, this retains the authored sun fan's roll.
fn sun_rotation(direction: [f32; 3]) -> script::Quat {
    use script::Quat;
    let forward = direction.map(|v| -v);
    Quat::axis_angle([0.0, 0.0, 1.0], 180.0)
        .then(Quat::axis_angle([1.0, 0.0, 0.0], -forward[1].asin().to_degrees()))
        .then(Quat::axis_angle([0.0, 1.0, 0.0], forward[0].atan2(forward[2]).to_degrees()))
}

fn sun_ao(day_time: f32) -> [f32; 3] {
    sun_positions(day_time)[0]
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
        let [sun_ao, sun2_ao] = sun_positions(day_time);
        Some(Sky { top, bottom, top_i, bottom_i, fog, sun, ambient, sun_specular: t.scalar("SpecularLightIntensity").unwrap_or(1.0), cloud_light, sun_dir: [sun_ao[0], sun_ao[1], -sun_ao[2]], sun_ao, sun2_ao, day_time, night: t.at("NightIntensity", f).unwrap_or(1.0), weather: crate::weather::State::clear() })
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

/// `Color` (sRGB) of the playfield's `e_ClearScreen` object (`Tweak_BlackBackground`, `Color 0xFFFF0000`, included by
/// `Tweak_Playfield_IndoorDefault` and a few others): the client never clears the colour buffer per frame (the `DisplaySystem`
/// frame functions pass the clear flag only for the `ToggleClearViewPort` debug toggle and the S3 driver quirk
/// `Randy_t::NeedsClearFix`), so this full-screen quad is the only background there is. It is a pre-transformed quad
/// (`FUN_10056f10`: x, y, z = 0.1, rhw = 10, diffuse = `Color`, `FUN_100570db` draws it with `ZFUNC ALWAYS`); its `FOGENABLE`
/// has no effect because the fog starts at the near plane, beyond the quad's depth 0.1 [INFERENCE]. The colour is therefore the
/// ARGB value as is: red, whatever the file name says. `None`: no such object, see docs *Clear colour*.
pub fn clear_color(t: &Tweaks) -> Option<[f32; 3]> {
    t.objects().iter().find(|o| o.fxid() == "ClearScreen").and_then(|o| {
        let c = u32::from_str_radix(o.field("Color")?.trim().trim_start_matches("0x"), 16).ok()?;
        Some([16, 8, 0].map(|s| (c >> s & 255) as f32 / 255.0))
    })
}

/// Re-evaluates an outdoor playfield's sky, fog tint and sun/ambient light at any `day_time` (the live time-of-day mode of the
/// viewer). Textures already handed out are not sent again, so a long run only ships the vertex colours that change.
pub struct SkyClock {
    store: ao_rdb::RecordStore,
    tweaks: Tweaks,
    env: super::environment::Env,
    sent: HashSet<ao_scene::TextureKey>,
    /// Game day seeding the weather schedule ([`crate::weather::OFFLINE_DAY`] unless [`SkyClock::on_day`]).
    day: u32,
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
        Ok(Tweaks::load(client_dir, id, true).map(|tweaks| SkyClock { store, tweaks, env, sent: HashSet::new(), day: crate::weather::OFFLINE_DAY }))
    }

    /// The weather follows the server's game day (`GameTimeIIR_t.arg3`) instead of the offline day.
    pub fn on_day(mut self, game_day: u32) -> Self {
        self.day = game_day;
        self
    }

    /// A scene with only `sky`, `meshes`, new `textures`, `environment` and the base `fog_model` (no volumes) filled in.
    pub fn at(&mut self, day_time: f32) -> Scene {
        let day_time = day_time.rem_euclid(DAY_LENGTH);
        let mut scene = Scene::default();
        let Some(mut sky) = Sky::new(&self.tweaks, day_time) else { return scene };
        sky.set_weather(weather_at(&self.env, self.day, day_time));
        let mut environment = super::environment::to_scene(&self.env, true, Some(&sky));
        environment.sky_color = clear_color(&self.tweaks).map_or(environment.sky_color, |c| c.map(srgb_to_linear));
        emit(&sky, &self.tweaks, &self.store, &mut scene, environment.fog_color, environment.fog_end);
        scene.fog_model = Some(super::environment::fog_model(&self.env, Some(&sky), vec![]));
        scene.environment = Some(environment);
        scene.textures.retain(|k, _| self.sent.insert(*k));
        scene
    }
}

/// The weather a static scene shows at `day_time`: the client's schedule of the playfield's `EnvironmentData` for the offline
/// game day ([`crate::weather::OFFLINE_DAY`]) plus [`WIND_WARMUP`] seconds of wind.
pub(super) fn weather_at(env: &super::environment::Env, game_day: u32, day_time: f32) -> crate::weather::State {
    crate::weather::Weather::sample(&env.raw, game_day, day_time as f64, WIND_WARMUP).state()
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
    fn native_binary_sun_matches_authored_quaternions_and_clock_edges() {
        // Independent client-authored fixture, not a calibration input.
        for (direction, authored) in sun_positions(DEFAULT_DAY_TIME).into_iter().zip([SUN1_ROT, SUN2_ROT]) {
            let frozen = rot(authored).rotate(SUN_LOCAL);
            assert!((0..3).all(|k| (direction[k] - frozen[k]).abs() < 1e-5));
            let q = sun_rotation(direction);
            let dot = q.x * authored[0] + q.y * authored[1] + q.z * authored[2] + q.w * authored[3];
            assert!((dot.abs() - 1.0).abs() < 1e-5, "{q:?}");
            let ray = q.rotate([0.0, 0.0, 1.0]);
            assert!((0..3).all(|k| (ray[k] + direction[k]).abs() < 1e-5), "native 3008 ray is +Z, not the sun fan's -Z");
        }
        // GC matrix evaluations: primary radius70 plus secondary radius5 / -15.
        for (time, expected) in [
            (0.0, [[0.7216359, -0.6797222, -0.1312225], [0.6453176, -0.7577605, 0.0967688]]),
            (3240.0, [[-0.6868368, 0.7267292, 0.0109499], [-0.7447781, 0.6150048, 0.2589878]]),
            (4860.0, [[-0.0524522, 0.0088852, -0.9985839], [-0.0415651, 0.1451375, -0.988_538]]),
        ] {
            for (actual, expected) in sun_positions(time).into_iter().zip(expected) {
                assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-5), "{time}: {actual:?}");
            }
        }
        for time in [-1.0, 0.0, 6480.0, 6481.0] {
            let actual = sun_positions(time);
            let wrapped = sun_positions(time.rem_euclid(DAY_LENGTH));
            assert_eq!(actual, wrapped);
            for direction in actual {
                assert!((direction.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
            }
        }
        assert!(sun_positions(0.0)[0][1] < 0.0 && sun_positions(3240.0)[0][1] > 0.0);
    }

    #[test]
    fn native_sun_look_frame_preserves_roll_through_the_day() {
        // GC1013c1f7 builds right=forward×worldUp, up=forward×right.
        // Check the complete frame independently of the quaternion construction.
        for minute in 0..=108 {
            for direction in sun_positions(minute as f32 * 60.0) {
                let forward = direction.map(|v| -v);
                let length = (forward[0] * forward[0] + forward[2] * forward[2]).sqrt();
                let right = [-forward[2] / length, 0.0, forward[0] / length];
                let up = [
                    forward[1] * right[2],
                    forward[2] * right[0] - forward[0] * right[2],
                    -forward[1] * right[0],
                ];
                let rotation = sun_rotation(direction);
                for (axis, expected) in [([1.0, 0.0, 0.0], right), ([0.0, 1.0, 0.0], up), ([0.0, 0.0, 1.0], forward)] {
                    let actual = rotation.rotate(axis);
                    assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-5));
                }
            }
        }
        let ao = sun_positions(DEFAULT_DAY_TIME)[0];
        assert_eq!(sun_dir(DEFAULT_DAY_TIME), [ao[0], ao[1], -ao[2]]);
    }

    #[test]
    fn real_sky_clock_scrolls_clouds_moves_the_sun_and_ships_textures_once() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        let Ok(Some(mut clock)) = SkyClock::open(&dir, 566) else { return };
        let (noon, night) = (clock.at(3240.0), clock.at(0.0));
        assert!(!noon.sky.is_empty() && !noon.textures.is_empty() && night.textures.is_empty(), "textures are sent once");
        // the cloud dome drifts, the layers other than clouds do not
        let drifting = noon.meshes.iter().flat_map(|m| &m.submeshes).filter(|s| s.uv_scroll != [0.0, 0.0]).count();
        // pf 566 has only the clear weight (w3 = 100): clear weather leaves the cloud dome invisible, so it is dropped
        assert!(drifting <= 1, "only ThickClouds scrolls");
        let (e1, e2) = (noon.environment.unwrap(), night.environment.unwrap());
        assert!(e1.sun_dir[1] > 0.3 && e2.sun_dir[1] < -0.3, "{:?} {:?}", e1.sun_dir, e2.sun_dir);
    }

    #[test]
    fn clear_screen_object_gives_the_background_colour() {
        let t = Tweaks("Object S\n{\n  Unsigned FXID: e_ClearScreen\n  Unsigned Color: 0xFFFF0000\n}\n".into());
        assert_eq!(clear_color(&t), Some([1.0, 0.0, 0.0]));
        assert_eq!(clear_color(&Tweaks("Object A\n{\n  Unsigned FXID: e_GenericMeshObject\n}\n".into())), None);
        if let (Some(indoor), Some(rk)) = (real_tweaks(125), real_tweaks(566)) {
            assert_eq!(clear_color(&indoor), Some([1.0, 0.0, 0.0]), "Tweak_BlackBackground");
            assert_eq!(clear_color(&rk), None, "Rubi-Ka outdoors has no ClearScreen");
        }
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
