//! Data the original client reads for its login-flow screens (login → progress → character selection → loading), with
//! the RE evidence in `docs/screens.md`:
//!
//! * [`CharCreateCameras`] — `CharCreateCamera.dat` (character *creation* cameras, plain text);
//! * [`LOGIN_CAMERA`], [`LOGIN_MESHES`], [`login_world_scene`] — the 3D backdrop of login and character selection
//!   (`LoginWorld_c` in GUI.dll, **not** a playfield);
//! * [`char_select_look`] — the character preview of the selection screen (`CharacterViewer_c` / `CCCharacter_t`);
//! * [`TextDb`] — `cd_image/text/text.mdb` (the `#Key` labels of the GUI views);
//! * [`pfnr_names`] — `data/launcher/pfnrmap.dat` (playfield id → name shown as the character's location).
//!
//! Coordinates are AO's left-handed Y-up space unless a function says `render`; the renderer contract (ao-scene) is
//! right-handed with Z negated, see [`ao_to_render`].

use crate::character::{NameTable, Breed, Gender, Player, Role, Skin, load_player, head_table, CHAR_MESH_TYPE};
use crate::mesh::{decode_mesh_into, decode_mesh_lights};
use anyhow::{bail, ensure, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Instance, Scene, IDENTITY};
use std::{collections::HashMap, path::Path};

/// rdb type of `.abiff` static meshes (`0xf6951` in `InstanceManager_t::GetTypeInstance`).
const MESH_TYPE: u32 = 1010001;
/// rdb type of character animation clips (`.ani`, `0xf6953`).
const ANIM_TYPE: u32 = 1010003;

/// AO space (left-handed) → renderer space (right-handed): negate Z.
pub fn ao_to_render(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

// ---------------------------------------------------------------------------------------------------------------
// CharCreateCamera.dat
// ---------------------------------------------------------------------------------------------------------------

/// One camera of `CharCreateCamera.dat`.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraPose {
    /// Hierarchical key, `ID: 1 1 3` → `[1, 1, 3]`.
    pub id: Vec<u32>,
    pub pos: [f32; 3],
    /// Quaternion `x y z w` as in the file (w last; same layout as `LoginWorld_c`'s stage record).
    pub rot: [f32; 4],
    pub fov_deg: f32,
}

/// A camera move: the path through the listed cameras (one per keyframe) lasts `duration_s`.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraTransition {
    pub id: Vec<u32>,
    pub duration_s: f32,
    pub keyframes: Vec<Vec<u32>>,
}

/// Parsed `CharCreateCamera.dat` (format: `docs/screens.md` §10; loader `FUN_10116d5a` @GUI 0x10116d5a).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharCreateCameras {
    pub cameras: Vec<CameraPose>,
    pub transitions: Vec<CameraTransition>,
}

impl CharCreateCameras {
    pub fn load(client_dir: &Path) -> Result<Self> {
        let p = client_dir.join("CharCreateCamera.dat");
        Self::parse(&std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?)
    }

    pub fn parse(text: &str) -> Result<Self> {
        // The original tokenises the stream by whitespace: a "Tag:" word followed by its values.
        let mut w = text.split_whitespace().peekable();
        fn tag<'a>(w: &mut impl Iterator<Item = &'a str>, want: &str) -> Result<()> {
            let t = w.next().with_context(|| format!("expected '{want}', found end of file"))?;
            ensure!(t == want, "expected '{want}', found '{t}'");
            Ok(())
        }
        fn num<'a, T: std::str::FromStr>(w: &mut impl Iterator<Item = &'a str>) -> Result<T> {
            let t = w.next().context("unexpected end of file")?;
            t.parse().map_err(|_| anyhow::anyhow!("bad number '{t}'"))
        }
        /// Integers up to the next `Tag:` word.
        fn ids<'a>(w: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>) -> Result<Vec<u32>> {
            let mut v = vec![];
            while let Some(t) = w.peek() {
                if t.ends_with(':') {
                    break;
                }
                v.push(w.next().unwrap().parse().map_err(|_| anyhow::anyhow!("bad id"))?);
            }
            Ok(v)
        }
        tag(&mut w, "CameraCount:")?;
        let n: usize = num(&mut w)?;
        let mut out = Self::default();
        for _ in 0..n {
            tag(&mut w, "ID:")?;
            let id = ids(&mut w)?;
            tag(&mut w, "Pos:")?;
            let pos = [num(&mut w)?, num(&mut w)?, num(&mut w)?];
            tag(&mut w, "Rot:")?;
            let rot = [num(&mut w)?, num(&mut w)?, num(&mut w)?, num(&mut w)?];
            tag(&mut w, "degFOV:")?;
            out.cameras.push(CameraPose { id, pos, rot, fov_deg: num(&mut w)? });
        }
        tag(&mut w, "TransitionCount:")?;
        let m: usize = num(&mut w)?;
        for _ in 0..m {
            tag(&mut w, "TransitionId:")?;
            let id = ids(&mut w)?;
            tag(&mut w, "Duration:")?;
            let duration_s = num(&mut w)?;
            tag(&mut w, "sec")?;
            tag(&mut w, "KeyframeCount:")?;
            let k: usize = num(&mut w)?;
            let mut keyframes = Vec::new(); // k is untrusted: the loop fails at the first missing `C:`
            for _ in 0..k {
                tag(&mut w, "C:")?;
                keyframes.push(ids(&mut w)?);
            }
            out.transitions.push(CameraTransition { id, duration_s, keyframes });
        }
        ensure!(w.next().is_none(), "trailing data after the last transition");
        Ok(out)
    }

    pub fn camera(&self, id: &[u32]) -> Option<&CameraPose> {
        self.cameras.iter().find(|c| c.id == id)
    }

    pub fn transition(&self, id: &[u32]) -> Option<&CameraTransition> {
        self.transitions.iter().find(|t| t.id == id)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// LoginWorld_c: the 3D backdrop of login + character selection
// ---------------------------------------------------------------------------------------------------------------

/// A D3D camera as `VisualCamera_t(fov, aspect, near, far)` + position/rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// AO space.
    pub pos: [f32; 3],
    /// Quaternion `x y z w`.
    pub rot: [f32; 4],
    pub fov_rad: f32,
    pub near: f32,
    pub far: f32,
}

/// The fixed camera of `LoginWorld_c` (floats at GUI 0x101a9a94.., 0x101aa058.., 0x101a9f98..).
pub const LOGIN_CAMERA: Camera = Camera {
    pos: [0.657694, 2.27458, -10.5489],
    rot: [0.000_490_287, -0.994_476, 0.104_805, -0.005_759_12],
    fov_rad: std::f32::consts::FRAC_PI_3, // the float 0x3f860a92 at GUI 0x101a9f98
    near: 0.5,
    far: 1000.0,
};

impl Camera {
    /// View direction in AO space: the quaternion rotates +Z (active rotation `q v q⁻¹`). For [`LOGIN_CAMERA`] this is
    /// (0.012, −0.208, −0.978): looking along −Z with a 12° downward pitch, which centres the character preview.
    pub fn forward(&self) -> [f32; 3] {
        let [x, y, z, w] = self.rot;
        [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)]
    }
}

/// `charactercreation_*.abiff` meshes `LoginWorld_c` loads for every stage (`AddMesh`, GUI 0x10015dc4), in load order.
pub const LOGIN_MESHES: [&str; 5] = [
    "charactercreation_main.abiff",
    "charactercreation_professions.abiff",
    "charactercreation_professions02.abiff",
    "charactercreation_nanoeffect.abiff",
    "charactercreation_adventurer.abiff",
];

/// Stage `s` (0 = login/progress, 1 = character selection) offsets its meshes by `(0, s * LOGIN_STAGE_Y_STEP, 0)`.
pub const LOGIN_STAGE_Y_STEP: f32 = -0.01;

/// Preview character position relative to the camera (`CharSelectWindow_c` ctor: floats at GUI 0x101a9aa4 / 0x101a9aa0).
pub const CHAR_VIEWER_OFFSET: [f32; 3] = [0.0, -1.9, -5.0];

/// rdb 1010001 ids of [`LOGIN_MESHES`].
pub fn login_world_ids(store: &RecordStore) -> Result<Vec<(&'static str, u32)>> {
    let names = NameTable::load(store)?;
    LOGIN_MESHES.iter().map(|n| Ok((*n, names.id(MESH_TYPE, n).with_context(|| format!("no mesh {n}"))?))).collect()
}

/// The lens of `LoginWorld_c`: `VisualCamera_t(1.0472, DisplayWidth / DisplayHeight, 0.5, 1000)`. `RCamera_t` reads the angle
/// as the **horizontal** field of view (randy31 `RCamera_t::RCamera_t` @0x1002a68a, docs/screens.md §2).
pub const LOGIN_LENS: ao_scene::Lens = ao_scene::Lens { fov: LOGIN_CAMERA.fov_rad, horizontal: true, near: LOGIN_CAMERA.near, far: Some(LOGIN_CAMERA.far) };

/// What the login world is lit with. No code of the login classes touches lights, ambient or fog, so the device state
/// `Randy_t`'s reset (randy31 @0x10041ede) leaves is all there is: `D3DRS_AMBIENT` = 0, `FOGENABLE` = 0, no sun, and a
/// viewport clear colour of (0, 0, 0.2) (`DisplaySystem` @0x10079121). Fog is "off" as a start beyond any distance.
pub fn login_environment() -> ao_scene::Environment {
    let clear = [0.0, 0.0, 0.2_f32.powf(2.2)];
    ao_scene::Environment { sky_color: clear, fog_color: clear, fog_start: 1.0e9, fog_end: 2.0e9, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0 }
}

/// The backdrop of `stage` as a renderer-space scene (meshes decoded with the usual Z mirror; `spawn` /
/// `spawn_look_at` are [`LOGIN_CAMERA`] converted with [`ao_to_render`], looking 5 m ahead), lit by the `RLight_t` nodes of
/// the meshes only ([`login_environment`], [`LOGIN_LENS`]).
pub fn login_world_scene(store: &RecordStore, stage: u32) -> Result<Scene> {
    let mut scene = Scene::default();
    let y = stage as f32 * LOGIN_STAGE_Y_STEP;
    for (_, id) in login_world_ids(store)? {
        let mesh = decode_mesh_into(store, id, &mut scene)?.with_context(|| format!("mesh {id} missing"))?;
        let mut transform = IDENTITY;
        transform[3][1] = y;
        scene.instances.push(Instance { mesh, transform });
        scene.lights.extend(decode_mesh_lights(store, MESH_TYPE, id, [0.0, y, 0.0])?);
    }
    scene.environment = Some(login_environment());
    scene.lens = Some(LOGIN_LENS);
    let c = LOGIN_CAMERA;
    let f = c.forward();
    scene.spawn = Some(ao_to_render(c.pos));
    scene.spawn_look_at = Some(ao_to_render([c.pos[0] + 5.0 * f[0], c.pos[1] + 5.0 * f[1], c.pos[2] + 5.0 * f[2]]));
    Ok(scene)
}

/// `LoginWorld_c::SetStage`: moves the backdrop (meshes and the lights inside them) to `stage`. Call before adding other instances.
pub fn set_login_stage(scene: &mut Scene, stage: u32) {
    let y = stage as f32 * LOGIN_STAGE_Y_STEP;
    let from = scene.instances.first().map_or(y, |i| i.transform[3][1]);
    for i in &mut scene.instances {
        i.transform[3][1] = y;
    }
    for l in &mut scene.lights {
        l.pos[1] += y - from;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Character selection preview
// ---------------------------------------------------------------------------------------------------------------

/// `BreedSex_e` / `Breed_e` on the wire (`CharacterInfo.breed`, `.gender`): 1 Solitus 2 Opifex 3 Nanomage 4 Atrox;
/// sex 1 unisex 2 male 3 female. The preview forces Atrox to male (`CCCharacter_t` ctor, GUI 0x1011ad5f).
pub fn wire_breed_sex(breed: i32, sex: i32) -> Result<(Breed, Gender)> {
    let b = match breed {
        1 => Breed::Solitus,
        2 => Breed::Opifex,
        3 => Breed::Nanomage,
        4 => Breed::Atrox,
        _ => bail!("breed {breed} has no player model"),
    };
    let g = match (b, sex) {
        (Breed::Atrox, _) | (_, 2) => Gender::Male,
        (_, 3) => Gender::Female,
        _ => bail!("sex {sex} has no player model"),
    };
    Ok((b, g))
}

/// How many entries the idle selector draws from and how many of them are socials: `rand() % 115 < 23` (GUI 0x1011a7d6).
pub const IDLE_DRAW: u32 = 0x73;
pub const SOCIAL_COUNT: u32 = 0x17;

/// `social_table` at GUI 0x101c3f40 (23 entries); clip name is `<set>_social-<x>.ani`.
pub const SOCIALS: [&str; 23] = [
    "angry", "applause", "blowkiss", "bow", "bulge", "curt", "fishsize", "giggle", "greet", "italian", "lookout", "rocky",
    "salute", "scared", "scratch", "strong1", "strong2", "strong3", "surprised", "surrender", "thinker", "thumbs", "wave",
];

/// Everything the selection screen derives from a list entry (`CharacterViewer_c::Update`, GUI 0x100054cd).
#[derive(Clone, Debug)]
pub struct CharSelectLook {
    pub breed: Breed,
    pub gender: Gender,
    /// rdb 1010002 id of `<breed>_<sex>.cir` (Build 1, no `_thin`/`_fat` suffix).
    pub model: u32,
    /// `(NN, rdb 1010001 id)` of head index 0: the lowest existing `head_<race><sex>NN.abiff`.
    pub head: (u32, u32),
    pub skin: Skin,
    /// `<set>_idle-stand_01_01.ani` (rdb 1010003): played once, never looped.
    pub idle_clip: (String, u32),
    /// The 23 `<set>_social-<x>.ani` clips; after each finished clip `rand()%115 < 23` picks `SOCIALS[r]`, else idle.
    pub social_clips: Vec<(String, u32)>,
    /// `SetScale` of Height 1.
    pub scale: f32,
    /// World position in AO space: camera + [`CHAR_VIEWER_OFFSET`] + the feet tweak of `CCCharacter_t::RunFunction`.
    pub position: [f32; 3],
}

/// Feet alignment added to Y by `CCCharacter_t::RunFunction` for Build 1 (normal): −0.02 for Solitus and Nanomage, 0 otherwise.
pub fn feet_offset_build1(breed: Breed) -> f32 {
    if matches!(breed, Breed::Solitus | Breed::Nanomage) { -0.02 } else { 0.0 }
}

/// The look the original gives list entry `(breed, sex)` on first run (no `CharacterViewer.xml` appearance cache): the
/// character's `head`, `height` and `width` fields are *not* used (docs/screens.md §5.5).
pub fn char_select_look(store: &RecordStore, breed: i32, sex: i32) -> Result<CharSelectLook> {
    let (b, g) = wire_breed_sex(breed, sex)?;
    let names = NameTable::load(store)?;
    let race = match b {
        Breed::Solitus => "solitus",
        Breed::Opifex => "opifex",
        Breed::Nanomage => "nanomage",
        Breed::Atrox => "athrox",
    };
    let sexname = if g == Gender::Male { "male" } else { "female" };
    let model_name = format!("{race}_{sexname}.cir");
    let model = names.id(CHAR_MESH_TYPE, &model_name).with_context(|| format!("no model {model_name}"))?;
    // head index 0 of the table the client builds before the character list arrives (`ExpansionFlags` 0)
    let first = *head_table(store, b, g, 0)?.first().with_context(|| format!("no heads for {b:?} {g:?}"))?;
    let head = (first.num, first.mesh);
    let set = if b == Breed::Atrox { "athrox" } else { sexname };
    let clip = |n: String| -> Result<(String, u32)> {
        let id = names.id(ANIM_TYPE, &n).with_context(|| format!("no clip {n}"))?;
        Ok((n, id))
    };
    let idle_clip = clip(format!("{set}_idle-stand_01_01.ani"))?;
    let social_clips = SOCIALS.iter().map(|s| clip(format!("{set}_social-{s}.ani"))).collect::<Result<Vec<_>>>()?;
    let mut position = [LOGIN_CAMERA.pos[0] + CHAR_VIEWER_OFFSET[0], LOGIN_CAMERA.pos[1] + CHAR_VIEWER_OFFSET[1], LOGIN_CAMERA.pos[2] + CHAR_VIEWER_OFFSET[2]];
    position[1] += feet_offset_build1(b);
    Ok(CharSelectLook { breed: b, gender: g, model, head, skin: Skin::Caucasian, idle_clip, social_clips, scale: 1.0, position })
}

/// Which clip plays next after a clip finished, given `r = rand()` (`CCCharacter_t::RunFunction`): `Some(i)` = `SOCIALS[i]`.
pub fn next_social(r: u32) -> Option<usize> {
    let k = r % IDLE_DRAW;
    (k < SOCIAL_COUNT).then_some(k as usize)
}

impl CharSelectLook {
    /// The body + head + naked skin (bind pose or the idle clip at `time` seconds), at the origin in renderer space;
    /// place it with [`ao_to_render`]`(self.position)`.
    pub fn scene(&self, store: &RecordStore, role: Option<(Role, f32)>) -> Result<Scene> {
        load_player(store, &Player::new(self.breed, self.gender, self.skin, Some(self.head.0)), role)
    }
}

/// `CCSelectedSize` / `CharacterInfo.width`: 0 thin, 1 normal, 2 fat (text.mdb category 2006); selects the `_thin` / `_fat` body model.
pub fn build_suffix(width: i32) -> Option<&'static str> {
    match width {
        0 => Some("_thin"),
        1 => Some(""),
        2 => Some("_fat"),
        _ => None,
    }
}

/// Creation preview scale of `Height_e` 0/1/2 (`CCCharacter_t::RunFunction`: 0x101c3fec, 1.0, 0x101c3fe8). The wire
/// `CharacterInfo.height` is 90/100/110 (`NameScene_t::SetState(0x1006)` @GUI 0x1011f75e).
pub fn creation_preview_scale(height_idx: u32) -> f32 {
    match height_idx {
        0 => 0.95,
        2 => 1.05,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// text.mdb
// ---------------------------------------------------------------------------------------------------------------

/// `LDBface::ElfHash`: the classic PJW hash used for string-keyed text categories.
pub fn elf_hash(s: &str) -> u32 {
    let mut h: u32 = 0;
    for &c in s.as_bytes() {
        h = (h << 4).wrapping_add(c as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// `cd_image/text/text.mdb` ("MMDB"): categories of `(key, string offset)` pairs into a NUL-terminated Latin-1 pool.
pub struct TextDb {
    data: Vec<u8>,
    cats: HashMap<u32, Vec<(u32, u32)>>,
}

/// View labels / button texts (`#Login` …).
pub const CAT_LABELS: u32 = 700;
/// GUI messages (`AO_Loading`, `DeleteCharacter` …).
pub const CAT_GUI: u32 = 10000;

impl TextDb {
    pub fn load(client_dir: &Path) -> Result<Self> {
        let p = client_dir.join("cd_image/text/text.mdb");
        Self::parse(std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?)
    }

    pub fn parse(data: Vec<u8>) -> Result<Self> {
        ensure!(data.get(..4) == Some(b"MMDB"), "not an MMDB file");
        let u32_at = |o: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(data.get(o..o + 4).context("text.mdb truncated")?.try_into().unwrap()))
        };
        let n = u32_at(4)? as usize;
        let mut table: Vec<(u32, usize)> = (0..n).map(|i| Ok((u32_at(8 + 8 * i)?, u32_at(12 + 8 * i)? as usize))).collect::<Result<_>>()?;
        table.sort_by_key(|t| t.1);
        let mut cats = HashMap::new();
        for (i, &(cat, start)) in table.iter().enumerate() {
            let end = table.get(i + 1).map_or(data.len(), |t| t.1);
            if cat == u32::MAX {
                continue; // the string pool
            }
            let mut v = Vec::with_capacity(end.saturating_sub(start) / 8);
            let mut o = start;
            while o + 8 <= end {
                v.push((u32_at(o)?, u32_at(o + 4)?));
                o += 8;
            }
            cats.insert(cat, v);
        }
        Ok(Self { data, cats })
    }

    /// Numeric categories (`GetText(cat, id)`), e.g. 2004 = professions, 1005 = breed 600+n / sex 100+n names.
    pub fn by_id(&self, cat: u32, id: u32) -> Option<String> {
        self.raw(cat, id)
    }

    /// String-key categories (`GetText(cat, "Key")`).
    pub fn by_key(&self, cat: u32, key: &str) -> Option<String> {
        self.raw(cat, elf_hash(key))
    }

    /// A view label: `#Key` is looked up in [`CAT_LABELS`] then [`CAT_GUI`]; anything else is a literal.
    pub fn label(&self, s: &str) -> String {
        match s.strip_prefix('#') {
            Some(k) => self.by_key(CAT_LABELS, k).or_else(|| self.by_key(CAT_GUI, k)).unwrap_or_else(|| s.to_string()),
            None => s.to_string(),
        }
    }

    fn raw(&self, cat: u32, key: u32) -> Option<String> {
        let (_, off) = self.cats.get(&cat)?.iter().find(|e| e.0 == key)?;
        // Latin-1 → String (the pool is not UTF-8: "\xa0" etc.)
        let s = self.data.get(*off as usize..)?;
        let end = s.iter().position(|&b| b == 0)?;
        Some(s[..end].iter().map(|&b| b as char).collect())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// pfnrmap.dat
// ---------------------------------------------------------------------------------------------------------------

/// `data/launcher/pfnrmap.dat` (`id;name` per line; lines without `;` such as the `#` comments never match) → playfield id → name, as `LoginModule_c` ctor reads it.
pub fn pfnr_names(client_dir: &Path) -> Result<HashMap<u32, String>> {
    let p = client_dir.join("cd_image/data/launcher/pfnrmap.dat");
    let s = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    let s: String = s.iter().map(|&b| b as char).collect();
    Ok(parse_pfnr(&s))
}

fn parse_pfnr(s: &str) -> HashMap<u32, String> {
    s.lines()
        .filter_map(|l| {
            let (id, name) = l.trim_end_matches('\r').split_once(';')?;
            Some((id.trim().parse().ok()?, name.to_string()))
        })
        .collect()
}

/// The location text of a character row: `"<name> (<id>)"`, or `"PF<id> (<id>)"` for an unknown playfield
/// (`SetupView`, GUI 0x1000e2ec + `LoginModule_c::GetPlayfieldName`).
pub fn location_text(names: &HashMap<u32, String>, pf: u32) -> String {
    match names.get(&pf) {
        Some(n) => format!("{n} ({pf})"),
        None => format!("PF{pf} ({pf})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMS: &str = "CameraCount: 2\r\nID: 1 \r\nPos: 1 2 3\r\nRot: 0 1 0 0\r\ndegFOV: 60\r\nID: 1 1 \r\nPos: -1.5 2 3e-2\r\nRot: 0 0 0 1\r\ndegFOV: 38\r\nTransitionCount: 1\r\nTransitionId: 1 \r\nDuration: 1.8 sec\r\nKeyframeCount: 2\r\nC: 1 \r\nC: 1 1 \r\n";

    #[test]
    fn fuzz_repros_are_errors() {
        assert!(CharCreateCameras::parse(&CAMS.replace("KeyframeCount: 2", "KeyframeCount: 18446744073709551615")).is_err());
        // table entry starting past the end of the file
        let mut d = b"MMDB".to_vec();
        for v in [1u32, 5, 0x1000] {
            d.extend(v.to_le_bytes());
        }
        let _ = TextDb::parse(d);
    }

    #[test]
    fn camera_file_grammar() {
        let c = CharCreateCameras::parse(CAMS).unwrap();
        assert_eq!(c.cameras.len(), 2);
        assert_eq!(c.camera(&[1, 1]).unwrap().pos, [-1.5, 2.0, 0.03]);
        assert_eq!(c.camera(&[1]).unwrap().rot, [0.0, 1.0, 0.0, 0.0]);
        let t = c.transition(&[1]).unwrap();
        assert_eq!((t.duration_s, t.keyframes.clone()), (1.8, vec![vec![1], vec![1, 1]]));
        assert!(CharCreateCameras::parse(&CAMS.replace("degFOV: 38", "degFOV: x")).is_err());
        assert!(CharCreateCameras::parse(&format!("{CAMS} junk")).is_err());
    }

    #[test]
    fn elf_hash_matches_client_table() {
        // keys whose hashes were found in text.mdb category 700 (docs/screens.md §8.2)
        assert_eq!(elf_hash("Login"), 0x535dfe);
        assert_eq!(elf_hash("Quit"), 0x58c04);
        assert_eq!(elf_hash("AO_Loading"), 0x353cb07);
    }

    #[test]
    fn mdb_roundtrip() {
        // header, 2 categories (700 label table, pool marker), one pair, pool
        let mut d = b"MMDB".to_vec();
        d.extend(2u32.to_le_bytes());
        let pool = 8 + 16 + 8;
        for (c, o) in [(700u32, 24u32), (u32::MAX, pool as u32)] {
            d.extend(c.to_le_bytes());
            d.extend(o.to_le_bytes());
        }
        d.extend(elf_hash("Login").to_le_bytes());
        d.extend((pool as u32).to_le_bytes());
        d.extend(b"Log\xe9in\0");
        let db = TextDb::parse(d).unwrap();
        assert_eq!(db.by_key(700, "Login").as_deref(), Some("Log\u{e9}in"));
        assert_eq!(db.label("#Login"), "Log\u{e9}in");
        assert_eq!(db.label("#Nope"), "#Nope");
        assert_eq!(db.label("Username:"), "Username:");
    }

    #[test]
    fn camera_forward_is_minus_z_pitched_down() {
        let f = LOGIN_CAMERA.forward();
        assert!((f[2] + 0.978).abs() < 1e-3 && (f[1] + 0.208).abs() < 1e-3, "{f:?}");
        assert!((f.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn idle_draw() {
        assert_eq!(next_social(0), Some(0));
        assert_eq!(next_social(22), Some(22));
        assert_eq!(next_social(23), None);
        assert_eq!(next_social(115 + 3), Some(3));
        assert_eq!(SOCIALS.len() as u32, SOCIAL_COUNT);
    }

    #[test]
    fn wire_mapping() {
        assert_eq!(wire_breed_sex(4, 3).unwrap(), (Breed::Atrox, Gender::Male));
        assert_eq!(wire_breed_sex(1, 3).unwrap(), (Breed::Solitus, Gender::Female));
        assert!(wire_breed_sex(5, 2).is_err() && wire_breed_sex(2, 1).is_err());
        assert_eq!(build_suffix(2), Some("_fat"));
    }

    #[test]
    fn pfnr_lines() {
        let m = parse_pfnr("# c\r\n120;Camelot Castle\r\n566;Newland City\r\n");
        assert_eq!(location_text(&m, 566), "Newland City (566)");
        assert_eq!(location_text(&m, 7), "PF7 (7)");
    }
}
