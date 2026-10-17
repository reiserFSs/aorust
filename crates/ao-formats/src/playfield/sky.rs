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
//! The server clock drives native GameSun* exports (Gamecode 0x100b879b/0x100b9044), then the final GAME object maps
//! its sky interfaces. ICC explicitly includes `Tweak_GAME_FrozenTime.txt`, freezing sky time and both suns even live.
//! The offline default time is 2648.69; frozen quaternions remain independent ephemeris fixtures, not calibration inputs.

mod aurora;
mod layers;
mod script;
mod traffic;

use std::collections::HashSet;
use std::path::Path;
use ao_scene::{Mesh, Scene, Submesh, Vertex};
use super::environment::srgb_to_linear;

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
pub struct Tweaks(String, u32);

impl Tweaks {
    pub fn load(client_dir: &Path, id: u32, outdoor: bool) -> Option<Self> {
        let dir = client_dir.join("cd_image/twk");
        let own = format!("Tweak_Playfield_{id}.txt");
        let first = if dir.join(&own).is_file() { own } else if outdoor { "Tweak_Playfield_OutdoorDefault.txt".into() } else { "Tweak_Playfield_IndoorDefault.txt".into() };
        let mut text = String::new();
        flatten(&dir, &first, &mut HashSet::new(), &mut text, false);
        (!text.is_empty()).then_some(Tweaks(text, id))
    }

    /// Every `Object` of the flattened script.
    pub fn objects(&self) -> Vec<script::Obj> {
        script::parse_objects(&self.0)
    }

    pub(super) fn ground_shadow_time(&self, day_time: f32) -> f32 {
        let objects = self.objects();
        let ctx = game_context(&objects, day_time);
        objects.iter().find(|o| o.name == "GAME")
            .and_then(|o| o.field("DayTimeForGroundShadows").and_then(|e| script::eval_field(o, &ctx, e, 0)))
            .unwrap_or_else(|| ground_shadow_time(day_time))
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

    /// Array `name` at factor `f` (FXS10003d24: N*f, cyclic last-to-first segment).
    fn at(&self, name: &str, f: f32) -> Option<f32> {
        let a = self.array(name)?;
        (!a.is_empty()).then(|| sample(&a, f))
    }

    fn rgb(&self, names: [&str; 3], f: f32) -> Option<[f32; 3]> {
        Some([self.at(names[0], f)?, self.at(names[1], f)?, self.at(names[2], f)?])
    }
}

/// Gamecode100bb7c9 origin registry (double constants101680d8..10168380);100bb5a2 uses origin zero on a miss.
/// This is not RKPP: in this client ICC4582 is absent and Jobe4526 uses the native HQ origin.
fn native_hq_offset(playfield: u32) -> [f32; 2] {
    let [x, z] = match playfield {
        505 => [35214.0, 44566.0],
        540 => [39621.0, 40670.0],
        545 => [39122.0, 40646.0],
        550 => [37085.0, 39823.0],
        551 => [37078.0, 41004.0],
        560 => [41975.0, 45030.0],
        565 => [41959.0, 42152.0],
        566 => [42862.0, 42389.0],
        567 => [41950.0, 42166.0],
        570 => [45486.0, 45364.0],
        585 => [39721.0, 39418.0],
        586 => [39743.0, 40342.0],
        590 => [42836.0, 36053.0],
        595 => [46219.0, 35172.0],
        600 => [41975.0, 39148.0],
        605 => [46248.0, 32165.0],
        610 => [42839.0, 33283.0],
        615 => [46680.0, 24729.0],
        620 => [46689.0, 27892.0],
        625 => [41891.0, 30614.0],
        630 => [43499.0, 27918.0],
        635 => [40889.0, 33041.0],
        640 => [47921.0, 39716.0],
        646 => [46178.0, 39071.0],
        647 => [46175.0, 39073.0],
        650 => [40866.0, 36044.0],
        655 => [37182.0, 31311.0],
        665 => [31002.0, 29384.0],
        670 => [37169.0, 27031.0],
        685 => [34495.0, 29814.0],
        687 => [34518.0, 29814.0],
        695 => [39962.0, 27028.0],
        696 => [41950.0, 27705.0],
        700 | 4526 => [43204.0, 27102.0],
        705 => [43756.0, 26526.0],
        710 => [43308.0, 27595.0],
        716 | 717 => [44526.0, 24729.0],
        730 => [34049.0, 30363.0],
        735 => [33917.0, 30370.0],
        740 => [33837.0, 30358.0],
        760 => [38486.0, 33943.0],
        790 => [38062.0, 36352.0],
        791 => [38081.0, 37660.0],
        795 => [33860.0, 39227.0],
        800 => [37107.0, 38360.0],
        _ => [0.0, 0.0],
    };
    //100b8917 subtracts universe(PF700,local(605,0,419)); local constants10167f0c/10167f08.
    [x - (43204.0 + 605.0), z - (27102.0 + 419.0)]
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

/// FXS10003d24 Float array interpolation; vector arrays10003e31 use the same N*f indexing.
fn sample(a: &[f32], f: f32) -> f32 {
    let p = f * a.len() as f32;
    let i = ((p - 0.49999).round_ties_even() as i32 as u32 as usize).min(a.len() - 1);
    let next = if i + 1 < a.len() { i + 1 } else { 0 };
    (a[i] as f64 + (a[next] as f64 - a[i] as f64) * (p - i as f32) as f64) as f32
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
    /// Unit vector towards the mapped sun 1 in AO space (left handed).
    sun_ao: [f32; 3],
    /// Native `GAME.GameDayTime`; weather and native cloud sun-height stay live when sky interfaces freeze.
    day_time: f32,
    /// Native HQ offsets at local camera origin; material coefficients add the live camera displacement.
    hq_offset: [f32; 2],
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

/// Native exports followed by the final GAME object's authored interface mapping.
fn game_context(objects: &[script::Obj], day_time: f32) -> script::Ctx {
    let [sun1, sun2] = sun_positions(day_time);
    let mut ctx = script::Ctx { sun1: sun_rotation(sun1), sun2: sun_rotation(sun2), ..script::Ctx::at(day_time) };
    if let Some(game) = objects.iter().find(|o| o.name == "GAME") { ctx.bind_game(game); }
    ctx
}

impl Sky {
    pub fn new(t: &Tweaks, day_time: f32) -> Option<Sky> {
        let ctx = game_context(&t.objects(), day_time);
        let f = ctx.day_factor.unwrap_or_else(|| day_factor(ctx.day_time));
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
        let sun_ao = ctx.sun1.rotate(SUN_LOCAL);
        Some(Sky { top, bottom, top_i, bottom_i, fog, sun, ambient, sun_specular: t.scalar("SpecularLightIntensity").unwrap_or(1.0), cloud_light, sun_dir: [sun_ao[0], sun_ao[1], -sun_ao[2]], sun_ao, day_time, hq_offset: native_hq_offset(t.1), night: t.at("NightIntensity", f).unwrap_or(1.0), weather: crate::weather::State::clear() })
    }

    /// The weather of the playfield at this moment (`ThickCloudsIntensity`, `HighAltitudeWind`).
    pub fn set_weather(&mut self, w: crate::weather::State) {
        self.weather = w;
    }

    /// Native yaw-locked `AtmosphereMesh` strip, in scene coordinates at normalized far=1000.
    /// FXS `GetPoint10001efd` multiplies each authored point by Size=1000;
    /// DisplaySystem `1005c917` then applies camera far/1000 and camera yaw only.
    pub fn atmosphere(&self) -> Mesh {
        let vertices = [
            [-1000.0, 0.0, -1000.0], [1000.0, 0.0, -1000.0],
            [-1000.0, 400.0, -200.0], [1000.0, 400.0, -200.0],
            [-1000.0, 400.0, 500.0], [1000.0, 400.0, 500.0],
        ].into_iter().enumerate().map(|(i, pos)| {
            let (rgb, alpha) = if i < 2 { (self.bottom, self.bottom_i) } else { (self.top, self.top_i) };
            let rgb = rgb.map(srgb_to_linear);
            Vertex { pos, color: [rgb[0], rgb[1], rgb[2], alpha], ..Default::default() }
        }).collect();
        // Expand the native triangle strip; culling is explicitly disabled.
        let indices = vec![0, 1, 2, 2, 1, 3, 2, 3, 4, 4, 3, 5];
        Mesh { vertices, submeshes: vec![Submesh { two_sided: true, blend: ao_scene::Blend::AlphaBlend, sky_fog: true, ..Submesh::new(indices, None) }] }
    }
}

/// Adds the sky of the playfield's tweak script to `scene.sky`: the atmosphere strip and every `BackgroundSort` object
/// (see `layers`).
pub fn emit(sky: &Sky, tweaks: &Tweaks, store: &ao_rdb::RecordStore, scene: &mut Scene) {
    layers::emit(sky, &tweaks.objects(), store, scene, sky.atmosphere());
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

    /// Resource quality for textures first loaded by this independent sky store.
    pub fn set_texture_quality(&mut self, quality: i32) {
        self.store.set_other_texture_quality(quality);
    }

    /// The weather follows the server's game day (`GameTimeIIR_t.arg3`) instead of the offline day.
    pub fn on_day(mut self, game_day: u32) -> Self {
        self.day = game_day;
        self
    }

    /// Per-frame scripted poses, independent of the resource/colour worker's cadence.
    pub fn pose_source(&self) -> impl FnMut(f32, &mut Scene) + Send + 'static {
        layers::pose_source(self.tweaks.objects(), native_hq_offset(self.tweaks.1))
    }

    /// A scene with only `sky`, `meshes`, new `textures`, `environment` and the base `fog_model` (no volumes) filled in.
    pub fn at(&mut self, day_time: f32) -> Scene {
        let day_time = day_time.rem_euclid(DAY_LENGTH);
        let mut scene = Scene { day_time, ..Scene::default() };
        let Some(mut sky) = Sky::new(&self.tweaks, day_time) else { return scene };
        sky.set_weather(weather_at(&self.env, self.day, day_time));
        let mut environment = super::environment::to_scene(&self.env, true, Some(&sky));
        environment.sky_color = clear_color(&self.tweaks).map_or(environment.sky_color, |c| c.map(srgb_to_linear));
        emit(&sky, &self.tweaks, &self.store, &mut scene);
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
    fn native_horizon_offsets_use_registered_origins_and_unknown_local_fallback() {
        assert_eq!(native_hq_offset(700), [-605.0, -419.0]);
        assert_eq!(native_hq_offset(566), [-947.0, 14868.0]);
        assert_eq!(native_hq_offset(716), [717.0, -2792.0]);
        assert_eq!(native_hq_offset(4526), [-605.0, -419.0], "native table, not authored RKPP.Jobe");
        for unknown in [0, 4582, u32::MAX] {
            assert_eq!(native_hq_offset(unknown), [-43809.0, -27521.0]);
        }
        let Some(tweaks) = real_tweaks(4582) else { return };
        assert_eq!(tweaks.1, 4582);
        assert_eq!(Sky::new(&tweaks, 0.0).unwrap().hq_offset, [-43809.0, -27521.0]);
    }

    #[test]
    fn expressions_run_left_to_right_with_optional_commas() {
        assert_eq!(eval_list(" 0 / 255,  64 / 255, 1.0"), vec![0.0, 64.0 / 255.0, 1.0]);
        assert_eq!(eval_list("23 / 255 * 0.0   57 / 255 * 3  0.5"), vec![0.0, 57.0 / 255.0 * 3.0, 0.5]);
        assert_eq!(eval_list("-1.5, 2"), vec![-1.5, 2.0]);
    }
    #[test]
    fn final_game_object_freezes_interfaces_but_not_native_exports() {
        let t = Tweaks(concat!(
            "Object GAME\n{\n Float Obsolete: 1\n Float CurrentDayTime: This.GameDayTime\n}\n",
            "Object GAME\n{\n Float GameDayTime: 0\n Float CurrentDayTime: 2648.69\n",
            " Float DayTimeFactor: This.CurrentDayTime / 6480\n Float DayTimeForGroundShadows: 2648.69 * 15\n",
            " Quaternion Sun1Rotation: q(0.900351,-0.0951056,0.344078,0.248863)\n",
            " Quaternion Sun2Rotation: This.GameSun2Rotation\n}\n"
        ).into(), 0);
        let objects = t.objects();
        assert_eq!(objects.len(), 1);
        assert!(objects[0].field("Obsolete").is_none(), "duplicate Object replaces, not merges");
        for time in [0.0, 3240.0] {
            let ctx = game_context(&objects, time);
            assert_eq!(ctx.day_time, DEFAULT_DAY_TIME);
            assert_eq!(ctx.day_factor, Some(DEFAULT_DAY_TIME / DAY_LENGTH));
            assert_eq!(script::eval_field(&objects[0], &ctx, "GAME.GameDayTime", 0), Some(time));
            let expected = rot(SUN1_ROT).rotate(SUN_LOCAL);
            let actual = ctx.sun1.rotate(SUN_LOCAL);
            assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-6));
            let expected = sun_positions(time)[1];
            let actual = ctx.sun2.rotate(SUN_LOCAL);
            assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-5));
            assert_eq!(t.ground_shadow_time(time), DEFAULT_DAY_TIME * 15.0);
        }
    }

    #[test]
    fn real_icc_freezes_fxs_sky_but_clouds_keep_native_sun_height() {
        let Some(t) = real_tweaks(4582) else { return };
        let (night, noon) = (Sky::new(&t, 0.0).unwrap(), Sky::new(&t, 3240.0).unwrap());
        assert_eq!(night.top, noon.top);
        assert_eq!(night.sun_dir, noon.sun_dir);
        assert_eq!(night.night, noon.night);
        assert_eq!((night.day_time, noon.day_time), (0.0, 3240.0), "weather and physical clock remain live");
        let mapped = game_context(&t.objects(), 0.0);
        for (actual, authored) in [mapped.sun1, mapped.sun2].map(|q| q.rotate(SUN_LOCAL)).into_iter().zip([SUN1_ROT, SUN2_ROT]) {
            let expected = rot(authored).rotate(SUN_LOCAL);
            assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-6));
        }
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        let Ok(store) = ao_rdb::RecordStore::open(&dir) else { return };
        let mut heights = Vec::new();
        for sky in [&night, &noon] {
            let mut snapshot = Scene::default();
            emit(sky, &t, &store, &mut snapshot);
            let cloud = snapshot.single_clouds.first().expect("ICC SingleCloud layer");
            assert!((cloud.sun_y - sun_ao(sky.day_time)[1]).abs() < 1e-6);
            assert_eq!(cloud.light_direction, sky.sun_dir, "FXS ground light remains frozen");
            assert!(snapshot.meshes[cloud.mesh].submeshes.iter().all(|material| material.sky_fog), "native SingleCloud inherits frame fog");
            heights.push(cloud.sun_y);
        }
        assert!(heights[0] < 0.0 && heights[1] > 0.0);
    }


    #[test]
    fn arrays_follow_continuation_lines_and_interpolate() {
        let t = Tweaks("Object A\n{\n  Float  Foo [3]:  0 / 2, 1\n                  4\n  Float Bar: 1\n}\n".into(), 0);
        assert_eq!(t.array("Foo").unwrap(), vec![0.0, 1.0, 4.0]);
        assert!(t.array("Bar").is_none());
        assert!((t.at("Foo", 0.75).unwrap() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn float_arrays_follow_native_count_scaled_cyclic_end_and_unsigned_clamp() {
        // FXS10003d24, N=4, x=N*f; indices >=N (including converted negative indices) clamp to3.
        for (factor, expected) in [(0.0, 0.0), (0.125, 5.0), (0.25, 10.0), (0.75, 30.0), (0.875, 15.0), (1.0, 0.0), (1.125, -15.0), (-0.125, 135.0)] {
            assert_eq!(sample(&[0.0, 10.0, 20.0, 30.0], factor), expected, "{factor}");
        }
        assert_eq!(sample(&[7.0], 0.75), 7.0);
    }

    #[test]
    fn installed_penumbra_uses_authored_ground_expression_not_rgb_track_or_sun1() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client/cd_image/twk");
        if !dir.join("Tweak_Shadowlands_SunLight_Penumbra.txt").is_file() { return; }
        let mut text = String::new();
        let mut seen = HashSet::new();
        flatten(&dir, "Tweak_GAME.txt", &mut seen, &mut text, false);
        flatten(&dir, "Tweak_Shadowlands_SunLight_Penumbra.txt", &mut seen, &mut text, false);
        let objects = script::parse_objects(&text);
        for time in [0.0, 1620.0, 3240.0, 4860.0] {
            let ctx = game_context(&objects, time);
            let (color, direction) = layers::ground_light(&objects, &ctx).expect("native Penumbra expressions");
            let incident = ctx.sun1.rotate([0.0, 0.0, 1.0]);
            // DS1005d589 rotates local +Z by GroundLightDirection (slot +2f4).
            // Metadata is toward-light: negate that AO ray, then mirror scene Z.
            let expected = [-2.0 * incident[0], 1.0, 2.0 * incident[2]];
            let length = expected.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((0..3).all(|k| (direction[k] - expected[k] / length).abs() < 2e-5));
            let light = objects.iter().find(|object| object.name == "SunLight").unwrap();
            let glr = script::vector_expr(&objects, light, &ctx, light.field("GLR").unwrap(), 0).unwrap();
            let ray = script::rotation_expr(&objects, light, &ctx, light.field("GroundLightDirection").unwrap(), 0).unwrap().rotate([0.0, 0.0, 1.0]);
            let expected_ao = [2.0 * incident[0] / length, -1.0 / length, 2.0 * incident[2] / length];
            assert!((0..3).all(|k| (glr[k] - expected_ao[k]).abs() < 2e-5), "authored GLR vector");
            assert!((0..3).all(|k| (ray[k] - glr[k]).abs() < 2e-5), "GLD [ROT] GLR maps native +Z onto GLR");
            assert!(direction[1] > 0.0);
            if time == 0.0 { assert_eq!(color, [0.0; 3]); }
            if time == 3240.0 {
                let expected = [1.0 - 0.5 * 62.0 / 255.0, 1.0 - 0.6 * 145.0 / 255.0, 1.0 - 0.5 * 211.0 / 255.0];
                assert!((0..3).all(|k| (color[k] - expected[k]).abs() < 2e-5));
            }
        }
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
        assert_eq!(noon.single_clouds.len(),1,"native SingleCloud layer stays live independently of clear weather");
        let clouds=&noon.meshes[noon.single_clouds[0].mesh];
        assert_eq!(clouds.vertices.len(),ao_scene::cloud::COUNT*ao_scene::cloud::GRID*ao_scene::cloud::GRID);
        assert_eq!(clouds.submeshes.len(),4,"four native single_cloud textures");
        // the cloud dome drifts, the layers other than clouds do not
        let drifting = noon.meshes.iter().flat_map(|m| &m.submeshes).filter(|s| s.uv_scroll != [0.0, 0.0]).count();
        // pf 566 has only the clear weight (w3 = 100): clear weather leaves the cloud dome invisible, so it is dropped
        assert!(drifting <= 1, "only ThickClouds scrolls");
        let (e1, e2) = (noon.environment.unwrap(), night.environment.unwrap());
        assert!(e1.sun_dir[1] > 0.3 && e2.sun_dir[1] < -0.3, "{:?} {:?}", e1.sun_dir, e2.sun_dir);
    }

    #[test]
    fn clear_screen_object_gives_the_background_colour() {
        let t = Tweaks("Object S\n{\n  Unsigned FXID: e_ClearScreen\n  Unsigned Color: 0xFFFF0000\n}\n".into(), 0);
        assert_eq!(clear_color(&t), Some([1.0, 0.0, 0.0]));
        assert_eq!(clear_color(&Tweaks("Object A\n{\n  Unsigned FXID: e_GenericMeshObject\n}\n".into(), 0)), None);
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
    fn atmosphere_is_the_native_six_vertex_strip_not_a_below_horizon_dome() {
        let Some(t) = real_tweaks(566) else { return };
        let sky = Sky::new(&t, 3300.0).unwrap();
        let mesh = sky.atmosphere();
        assert_eq!(mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>(), vec![
            [-1000.0, 0.0, -1000.0], [1000.0, 0.0, -1000.0],
            [-1000.0, 400.0, -200.0], [1000.0, 400.0, -200.0],
            [-1000.0, 400.0, 500.0], [1000.0, 400.0, 500.0],
        ]);
        assert_eq!(mesh.submeshes[0].indices, [0, 1, 2, 2, 1, 3, 2, 3, 4, 4, 3, 5]);
        assert!(mesh.submeshes[0].sky_fog && mesh.submeshes[0].two_sided);
        assert_eq!(mesh.submeshes[0].blend, ao_scene::Blend::AlphaBlend);
        assert!(mesh.vertices[..2].iter().all(|v| v.color[3] == sky.bottom_i));
        assert!(mesh.vertices[2..].iter().all(|v| v.color[3] == sky.top_i));
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
