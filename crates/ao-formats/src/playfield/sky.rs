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
//! values of `Tweak_GAME_FrozenTime.txt` (`CurrentDayTime 2648.69`, `Sun1Rotation q(0.900351, -0.0951056, 0.344078, 0.248863)`),
//! the client's own fixed-time debug setting.

use std::collections::HashSet;
use std::path::Path;

use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, Texture, TextureKey, Vertex, IDENTITY};

use super::environment::{srgb_to_linear, NEAR, VIEW_DISTANCE};

/// `Tweak_GAME_FrozenTime.txt`.
const DAY_TIME: f32 = 2648.69;
/// `GAME.DayTimeFactor = CurrentDayTime / 6480` (27 * 60 * 4).
const DAY_LENGTH: f32 = 6480.0;
/// `Sun1Rotation` as (w, x, y, z) of that file.
const SUN1_ROT: [f32; 4] = [0.900351, -0.0951056, 0.344078, 0.248863];
/// `AddFogI` of the atmosphere object.
pub(super) const ATMOSPHERE_FOG_DENSITY: f32 = 0.025;
const MESH_TEXTURES: u32 = 1_010_004;

/// All tweak files a playfield includes, concatenated (comments removed).
pub struct Tweaks(String);

impl Tweaks {
    pub fn load(client_dir: &Path, id: u32, outdoor: bool) -> Option<Self> {
        let dir = client_dir.join("cd_image/twk");
        let own = format!("Tweak_Playfield_{id}.txt");
        let first = if dir.join(&own).is_file() { own } else if outdoor { "Tweak_Playfield_OutdoorDefault.txt".into() } else { "Tweak_Playfield_IndoorDefault.txt".into() };
        let mut text = String::new();
        flatten(&dir, &first, &mut HashSet::new(), &mut text);
        (!text.is_empty()).then_some(Tweaks(text))
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

fn flatten(dir: &Path, name: &str, seen: &mut HashSet<String>, out: &mut String) {
    if !seen.insert(name.to_string()) || seen.len() > 64 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(dir.join(name)) else { return };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        if let Some(inc) = line.trim().strip_prefix("!include") {
            let inc = inc.trim().trim_start_matches('{').trim_end_matches('}').trim();
            flatten(dir, inc, seen, out);
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
    /// `ColorTop/Bottom` (I scaled), sRGB.
    top: [f32; 3],
    bottom: [f32; 3],
    /// `AddFog` colour of the atmosphere object (sRGB).
    pub fog: [f32; 3],
    /// `GroundLightCurrent` (sRGB) and `AmbientLightCurrent`.
    pub sun: Option<[f32; 3]>,
    pub ambient: Option<[f32; 3]>,
    /// Unit vector towards the sun in scene space.
    pub sun_dir: [f32; 3],
}

pub fn day_factor() -> f32 {
    DAY_TIME / DAY_LENGTH
}

/// `Unit1ZDirection [ROT] Sun1Rotation` (AO world, left handed) mirrored to scene space.
pub fn sun_dir() -> [f32; 3] {
    let [w, x, y, z] = SUN1_ROT;
    let d = [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)];
    [d[0], d[1], -d[2]]
}

impl Sky {
    /// Horizon colour (sRGB).
    pub fn bottom(&self) -> [f32; 3] {
        self.bottom
    }

    pub fn new(t: &Tweaks) -> Option<Sky> {
        let f = day_factor();
        let colour = |side: &str| -> Option<[f32; 3]> {
            let i = t.at(&format!("Color{side}I"), f)?;
            let c = t.rgb([&format!("Color{side}R"), &format!("Color{side}G"), &format!("Color{side}B")].map(|s| s.as_str()), f)?;
            Some(c.map(|v| (v * i).clamp(0.0, 1.0)))
        };
        let (top, bottom) = (colour("Top")?, colour("Bottom")?);
        // AddFogIntensity = (bottom I + middle I) / 2, AddFogR = (bottom R + middle R) / 2 * intensity (left to right)
        let (bi, mi) = (t.at("ColorBottomI", f)?, t.at("ColorMiddleI", f)?);
        let (b, m) = (t.rgb(["ColorBottomR", "ColorBottomG", "ColorBottomB"], f)?, t.rgb(["ColorMiddleR", "ColorMiddleG", "ColorMiddleB"], f)?);
        let fog = [0, 1, 2].map(|k| ((b[k] + m[k]) / 2.0 * (bi + mi) / 2.0).clamp(0.0, 1.0));
        let sun = t.rgb(["GroundLightR", "GroundLightG", "GroundLightB"], f).map(|c| c.map(|v| (v * 2.0).min(1.0)));
        let ambient = t.at("AmbientLight", f).map(|a| [a; 3]).or_else(|| t.rgb(["AmbientLightR", "AmbientLightG", "AmbientLightB"], f));
        Some(Sky { top, bottom, fog, sun, ambient, sun_dir: sun_dir() })
    }

    /// Camera-locked sky dome (`Scene::sky`, drawn unlit and unfogged by the renderer): the atmosphere strip's gradient
    /// with the fog of the world baked in (the client draws the strip with `FOGENABLE`).
    pub fn dome(&self, fog_color: [f32; 3], fog_start: f32, fog_end: f32) -> Mesh {
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
            let fog = ((d - fog_start) / (fog_end - fog_start).max(1e-3)).clamp(0.0, 1.0);
            let c = [0, 1, 2].map(|k| (bottom[k] * (1.0 - s) + top[k] * s) * (1.0 - fog) + fog_color[k] * fog);
            for j in 0..=SEG {
                let a = j as f32 / SEG as f32 * std::f32::consts::TAU;
                vertices.push(Vertex {
                    pos: [radius * er.cos() * a.cos(), radius * er.sin(), radius * er.cos() * a.sin()],
                    normal: [0.0, -1.0, 0.0],
                    color: [c[0], c[1], c[2], 1.0],
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
        Mesh { vertices, submeshes: vec![Submesh { two_sided: true, ..Submesh::new(indices, None) }] }
    }

    /// `Sun1` rays: additive `newsun_frame01.png` quad towards the sun, tinted (255,155,55)
    /// [guess: the half angle of 6 degrees stands for `Size 6.0` at view-distance scale].
    pub fn sun(&self, store: &ao_rdb::RecordStore, scene: &mut Scene) -> Option<Mesh> {
        let id = crate::character::NameTable::load(store).ok()?.id(MESH_TEXTURES, "newsun_frame01.png")?;
        let key = TextureKey { rdb_type: MESH_TEXTURES, id };
        let tex: Texture = crate::texture::decode_texture(&store.get(MESH_TEXTURES, id).ok()??).ok()?;
        scene.textures.insert(key, tex);
        let d = self.sun_dir;
        let up = if d[1].abs() > 0.99 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let norm = |a: [f32; 3]| { let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt(); a.map(|v| v / l) };
        let r = norm(cross(up, d));
        let u = cross(d, r);
        let (dist, half) = (300.0, 300.0 * 6f32.to_radians().tan());
        let colour = [255.0, 155.0, 55.0].map(|v| srgb_to_linear(v / 255.0));
        let p = |sx: f32, sy: f32| [0, 1, 2].map(|k| d[k] * dist + (r[k] * sx + u[k] * sy) * half);
        let vertices = [(-1.0, -1.0, 0.0, 1.0), (1.0, -1.0, 1.0, 1.0), (1.0, 1.0, 1.0, 0.0), (-1.0, 1.0, 0.0, 0.0)]
            .map(|(x, y, tu, tv)| Vertex { pos: p(x, y), normal: [-d[0], -d[1], -d[2]], uv: [tu, tv], color: [colour[0], colour[1], colour[2], 1.0] })
            .to_vec();
        let sub = Submesh { two_sided: true, blend: Blend::Additive, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key)) };
        Some(Mesh { vertices, submeshes: vec![sub] })
    }
}

/// Adds the dome and the sun to `scene.sky`.
pub fn emit(sky: &Sky, store: &ao_rdb::RecordStore, scene: &mut Scene, fog_color: [f32; 3], fog_end: f32) {
    let meshes = [Some(sky.dome(fog_color, NEAR, fog_end)), sky.sun(store, scene)];
    for m in meshes.into_iter().flatten() {
        scene.meshes.push(m);
        scene.sky.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
    }
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

    #[test]
    fn real_rubi_ka_noon_is_a_bright_blue_sky() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Some(t) = Tweaks::load(&dir, 566, true) else { return };
        let s = Sky::new(&t).unwrap();
        assert!(s.top[2] > s.top[0] && s.bottom.iter().all(|&v| v > 0.3), "{:?} {:?}", s.top, s.bottom);
        assert!(s.sun.unwrap()[0] > 0.5);
        let d = sun_dir();
        assert!(d[1] > 0.0 && (d.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);
    }
}
