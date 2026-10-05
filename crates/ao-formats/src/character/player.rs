//! Player characters and animation roles, resolved through the client's resource name table
//! (`names.rs`); see `docs/formats.md` § characters.
//!
//! * clips are named `<set>_<role>_01_01.ani` (`%s_%s_01_01.ani` in Gamecode.dll); the set is `athrox`
//!   for Atrox bodies and `male`/`female` for the other breeds (all human bodies share one skeleton);
//! * heads are meshes `head_<race><sex>[_<ethnicity>]<NN>.abiff` (`head_%s%s%02d.abiff` in GUI.dll);
//! * skin: the body models are textured `<part>_<race><sex>_default.png` (starter clothing, hands a
//!   green placeholder); the naked skins are rdb 1010011 `<part>_<race><sex>[_<ethnicity>]_naked.png`.

use super::{load_cat_mesh, load_character_head_skin, CatAnim, NameTable, CHAR_ANIM_TYPE, CHAR_MESH_TYPE};
use anyhow::{Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Scene, TextureKey};
use std::{collections::HashMap, str::FromStr};

/// Static meshes (heads).
const MESH_TYPE: u32 = 1010001;
/// Body textures (`*_default.png`).
const TEXTURE_TYPE: u32 = 1010004;
/// Naked skin textures of the player bodies.
const SKIN_TYPE: u32 = 1010011;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breed {
    Solitus,
    Opifex,
    Nanomage,
    Atrox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

/// Skin tone; the client only has variants for solitus (`caucation`, sic, `asian`, `african`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Skin {
    #[default]
    Caucasian,
    Asian,
    African,
}

/// A character as picked on the creation screen.
#[derive(Clone, Copy, Debug)]
pub struct Player {
    pub breed: Breed,
    pub gender: Gender,
    /// Solitus only; selects the ethnicity-specific head set and body skin.
    pub skin: Skin,
    /// Head number `NN` (see [`player_heads`]); `None` = the lowest available.
    pub head: Option<u32>,
}

impl Breed {
    fn model_race(self) -> &'static str {
        match self {
            Breed::Solitus => "solitus",
            Breed::Opifex => "opifex",
            Breed::Nanomage => "nanomage",
            Breed::Atrox => "athrox",
        }
    }
}

impl Gender {
    fn name(self) -> &'static str {
        if self == Gender::Male { "male" } else { "female" }
    }
}

impl Skin {
    /// Infix of the head mesh names.
    fn head_infix(self) -> &'static str {
        match self {
            Skin::Caucasian => "",
            Skin::Asian => "_asian",
            Skin::African => "_african",
        }
    }
    /// Infix of the naked skin texture names (the client's spelling).
    fn skin_infix(self) -> &'static str {
        match self {
            Skin::Caucasian => "_caucation",
            Skin::Asian => "_asian",
            Skin::African => "_african",
        }
    }
}

macro_rules! from_str {
    ($t:ty { $($s:literal => $v:expr),+ }) => {
        impl FromStr for $t {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                match s.to_ascii_lowercase().as_str() {
                    $($s => Ok($v),)+
                    _ => Err(format!("unknown {} '{s}' (expected one of: {})", stringify!($t), [$($s),+].join(", "))),
                }
            }
        }
    };
}
from_str!(Breed { "solitus" => Breed::Solitus, "opifex" => Breed::Opifex, "nanomage" => Breed::Nanomage, "atrox" => Breed::Atrox });
from_str!(Gender { "male" => Gender::Male, "female" => Gender::Female });
from_str!(Skin { "caucasian" => Skin::Caucasian, "asian" => Skin::Asian, "african" => Skin::African });

/// What a clip is for. Names are the clip names Gamecode.dll builds file names from
/// (`<set>_<name>_01_01.ani`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Idle,
    /// Idle with raised fists (`idle-unarmed`).
    IdleCombat,
    Walk,
    WalkBack,
    WalkLeft,
    WalkRight,
    Run,
    RunBack,
    /// `sneakcool` (athrox 9382: what earlier notes called the walk clip).
    Sneak,
    SitChair,
    SitGround,
    Crawl,
    Swim,
    IdleSwim,
    JumpStand,
    JumpForward,
    SleepGround,
    Lounge,
    Hover,
    /// `social-<name>` emote, e.g. `Emote("backflip")`.
    Emote(String),
}

const ROLE_NAMES: &[(Role, &str)] = &[
    (Role::Idle, "idle-stand"),
    (Role::IdleCombat, "idle-unarmed"),
    (Role::Walk, "walk"),
    (Role::WalkBack, "walk-back"),
    (Role::WalkLeft, "walk-left"),
    (Role::WalkRight, "walk-right"),
    (Role::Run, "run"),
    (Role::RunBack, "run-back"),
    (Role::Sneak, "sneakcool"),
    (Role::SitChair, "idle-chair"),
    (Role::SitGround, "idle-ground"),
    (Role::Crawl, "idle-crawl"),
    (Role::Swim, "swim"),
    (Role::IdleSwim, "idle-swim"),
    (Role::JumpStand, "jump-stand"),
    (Role::JumpForward, "jump-forward"),
    (Role::SleepGround, "idle-sleep-ground"),
    (Role::Lounge, "idle-lounging"),
    (Role::Hover, "idle-hover"),
];

impl Role {
    /// The clip name (the part between the set prefix and `_01_01.ani`).
    pub fn clip_name(&self) -> String {
        match self {
            Role::Emote(e) => format!("social-{e}"),
            r => ROLE_NAMES.iter().find(|(x, _)| x == r).unwrap().1.to_string(),
        }
    }
}

impl FromStr for Role {
    type Err = String;
    /// A clip name (`walk`, `idle-stand`, `social-bow`) or `idle`; `bow` alone is an emote too.
    fn from_str(s: &str) -> Result<Self, String> {
        let s = if s == "idle" { "idle-stand" } else { s };
        if let Some((r, _)) = ROLE_NAMES.iter().find(|(_, n)| *n == s) {
            return Ok(r.clone());
        }
        let e = s.strip_prefix("social-").unwrap_or(s);
        if e.is_empty() {
            return Err("empty role".into());
        }
        Ok(Role::Emote(e.to_string()))
    }
}

/// Clip-name prefixes that can belong to a model named `<race>[_<sex>…].cir`, best first.
fn clip_sets(model: &str) -> Vec<String> {
    let stem = model.strip_suffix(".cir").unwrap_or(model);
    let race = stem.split('_').next().unwrap_or(stem);
    let mut v = vec![];
    if race == "athrox" {
        v.push(race.to_string());
    }
    if stem.contains("_female") {
        v.push("female".into());
    } else if stem.contains("_male") {
        v.push("male".into());
    }
    v.push(stem.to_string());
    v.push(race.to_string());
    v
}

/// Every clip that fits `model_id` (1010002), with its clip name (`walk`, `social-bow`, …), ascending by
/// id: the clips of the model's own set (`athrox`/`male`/`female`/creature name) that share its skeleton.
pub fn model_clips(store: &RecordStore, model_id: u32) -> Result<Vec<(String, u32)>> {
    let names = NameTable::load(store)?;
    let model = names.name(CHAR_MESH_TYPE, model_id).with_context(|| format!("model {model_id} has no name"))?;
    let sig = load_cat_mesh(store, CHAR_MESH_TYPE, model_id)?.signature;
    let mut clips: Vec<(&str, u32)> = names.entries(CHAR_ANIM_TYPE).filter_map(|(id, n)| Some((n.strip_suffix(".ani")?, id))).collect();
    clips.sort_by_key(|c| c.1);
    let set_clips = |set: &str| -> Result<Vec<(String, u32)>> {
        let mut out = vec![];
        for (n, id) in &clips {
            let Some(n) = n.strip_prefix(set).and_then(|n| n.strip_prefix('_')) else { continue };
            if store.get(CHAR_ANIM_TYPE, *id)?.and_then(|b| CatAnim::signature_of(&b).ok()) == Some(sig) {
                out.push((n.trim_end_matches("_01_01").to_string(), *id));
            }
        }
        Ok(out)
    };
    for set in clip_sets(model) {
        let out = set_clips(&set)?;
        if !out.is_empty() {
            return Ok(out);
        }
    }
    // A model that no set is named after (`skeleton_solitus`): clips of other sets with the same skeleton
    // hash have other bone lengths and tear the limbs apart, so take the set that fits best.
    let mut fitting: Vec<(&str, String, u32)> = vec![]; // (set, clip name, id) of every clip with this skeleton hash
    for (n, id) in &clips {
        let Some((set, rest)) = n.split_once('_') else { continue };
        if store.get(CHAR_ANIM_TYPE, *id)?.and_then(|b| CatAnim::signature_of(&b).ok()) == Some(sig) {
            fitting.push((set, rest.trim_end_matches("_01_01").to_string(), *id));
        }
    }
    let mut sets: Vec<&str> = fitting.iter().map(|c| c.0).collect();
    sets.sort_unstable();
    sets.dedup();
    let mut best: Option<(f32, Vec<(String, u32)>)> = None;
    for set in sets {
        let out: Vec<_> = fitting.iter().filter(|c| c.0 == set).map(|c| (c.1.clone(), c.2)).collect();
        if out.len() < 5 {
            continue;
        }
        let probe = out.iter().find(|c| c.0 == "walk").unwrap_or(&out[0]).1;
        let gap = super::pose_detachment(store, model_id, probe, 0.6)?;
        if best.as_ref().is_none_or(|b| gap < b.0) {
            best = Some((gap, out));
        }
    }
    best.map(|b| b.1).with_context(|| format!("no named clips for model {model_id} ({model})"))
}

/// The clip of every [`Role`] the model's set has (emotes included), ascending by clip id.
pub fn character_animations(store: &RecordStore, model_id: u32) -> Result<Vec<(Role, u32)>> {
    Ok(model_clips(store, model_id)?.into_iter().filter_map(|(n, id)| Some((n.parse().ok()?, id))).collect())
}

/// Clip id playing `role` on model `model_id`.
pub fn role_anim(store: &RecordStore, model_id: u32, role: &Role) -> Result<u32> {
    let name = role.clip_name();
    model_clips(store, model_id)?
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|c| c.1)
        .with_context(|| format!("model {model_id} has no clip '{name}'"))
}

/// The head choices of a player type: `(NN, 1010001 mesh id)` ascending by `NN`. `skin` picks the
/// solitus ethnicity set; it is ignored for the other breeds, whose heads have no ethnicity infix.
pub fn player_heads(store: &RecordStore, breed: Breed, gender: Gender, skin: Skin) -> Result<Vec<(u32, u32)>> {
    let names = NameTable::load(store)?;
    heads(&names, breed, gender, skin)
}

fn heads(names: &NameTable, breed: Breed, gender: Gender, skin: Skin) -> Result<Vec<(u32, u32)>> {
    // `head_athrox%02d`, `head_nanomale%02d`, `head_solitusfemale_asian%02d`, …
    let (race, sex, eth) = match breed {
        Breed::Atrox => ("athrox", "", ""),
        Breed::Nanomage => ("nano", gender.name(), ""),
        Breed::Opifex => ("opifex", gender.name(), ""),
        Breed::Solitus => ("solitus", gender.name(), skin.head_infix()),
    };
    let prefix = format!("head_{race}{sex}{eth}");
    let mut v: Vec<(u32, u32)> = names
        .entries(MESH_TYPE)
        .filter_map(|(id, n)| {
            let num = n.strip_prefix(&prefix)?.strip_suffix(".abiff")?;
            Some((num.parse().ok()?, id)) // rejects `_asian…`, named heads, `_lores`
        })
        .collect();
    v.sort_unstable();
    Ok(v)
}

/// One entry of the creation head table ([`head_table`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeadEntry {
    /// `NN` of `head_<race><sex>[_<ethnicity>]NN.abiff`.
    pub num: u32,
    /// rdb 1010001 id of that mesh = the `head` value of `CreateCharacter` (`CCCharacter_t::GetHeadMeshID`).
    pub mesh: u32,
    /// Ethnicity of the entry (`BreedRace_e` 1 / 2 / 3 = caucasian / african / asian), selects the naked skin.
    pub skin: Skin,
}

/// The head table of a (breed, sex) exactly as the client builds it: `HeadMeshData_t` ctor `FUN_1011d368` [GUI 0x1011d368],
/// read by `CCCharacter_t::MakeHeadMeshTable` [0x1011aacb]. Per (sex, breed) a `std::vector<(mesh id, ethnicity)>`
/// in **insertion order**, filled block by block with `head_<name>%02d.abiff` for `NN = 0..count` (entries whose mesh does
/// not exist in rdb 1010001 and the skip lists are left out). Index `i` of the Appearance head selector = `table[i]`.
///
/// `expansions` is DValue `ExpansionFlags`; bit 2 (Shadowlands) raises the `NN` ranges. The table is built once, when the
/// first `CCCharacter_t` exists — `CharacterViewer_c` of `CharSelectWindow_c`, constructed by `LoginModule_c::SlotInitialize`
/// before the character list arrives — so a fresh client has `expansions == 0` (docs/screens.md § 12).
pub fn head_table(store: &RecordStore, breed: Breed, gender: Gender, expansions: u32) -> Result<Vec<HeadEntry>> {
    let names = NameTable::load(store)?;
    let sl = expansions & 2 != 0;
    let pick = |base: u32, full: u32| if sl { full } else { base };
    // (name, ethnicity, count, skipped NN)
    let blocks: Vec<(&str, Skin, u32, &[u32])> = match (breed, gender) {
        (Breed::Atrox, _) => vec![("athrox", Skin::Caucasian, pick(0x1e, 0x29), &[])],
        (Breed::Opifex, Gender::Female) => vec![("opifexfemale", Skin::Caucasian, pick(0x20, 0x2b), &[30])],
        (Breed::Opifex, Gender::Male) => vec![("opifexmale", Skin::Caucasian, pick(0x1e, 0x2b), &[])],
        (Breed::Nanomage, Gender::Female) => vec![("nanofemale", Skin::Caucasian, pick(0x1e, 0x2b), &[])],
        (Breed::Nanomage, Gender::Male) => vec![("nanomale", Skin::Caucasian, pick(0x1e, 0x29), &[])],
        (Breed::Solitus, Gender::Female) => vec![
            ("solitusfemale", Skin::Caucasian, pick(0x32, 0x4b), &[3, 4, 8, 9, 10, 13, 14, 22, 29, 30, 43, 46, 47]),
            ("solitusfemale_african", Skin::African, 8, &[]),
            ("solitusfemale_asian", Skin::Asian, 7, &[]),
        ],
        (Breed::Solitus, Gender::Male) => vec![
            ("solitusmale", Skin::Caucasian, pick(0x33, 0xff), &[3, 4, 10, 11, 12, 14, 28, 29, 31, 32, 34, 37, 41, 47]),
            ("solitusmale_african", Skin::African, 9, &[]),
            ("solitusmale_asian", Skin::Asian, pick(6, 9), &[]),
        ],
    };
    let mut table = vec![];
    for (name, skin, count, skip) in blocks {
        for num in (0..count).filter(|n| !skip.contains(n)) {
            if let Some(mesh) = names.id(MESH_TYPE, &format!("head_{name}{num:02}.abiff")) {
                table.push(HeadEntry { num, mesh, skin });
            }
        }
    }
    Ok(table)
}

/// Naked-skin textures for a body's `*_default.png` parts: original texture id → rdb 1010011 key.
fn skin_swaps(names: &NameTable, store: &RecordStore, model_id: u32, skin: Skin) -> Result<HashMap<u32, TextureKey>> {
    let mesh = load_cat_mesh(store, CHAR_MESH_TYPE, model_id)?;
    let solitus = names.name(CHAR_MESH_TYPE, model_id).is_some_and(|n| n.starts_with("solitus"));
    let mut swaps = HashMap::new();
    for part in &mesh.parts {
        let Some(stem) = names.name(TEXTURE_TYPE, part.texture).and_then(|n| n.strip_suffix("_default.png")) else { continue };
        let eth = if solitus { skin.skin_infix() } else { "" };
        if let Some(id) = names.id(SKIN_TYPE, &format!("{stem}{eth}_naked.png")) {
            swaps.insert(part.texture, TextureKey { rdb_type: SKIN_TYPE, id });
        }
    }
    Ok(swaps)
}

/// Body model id (rdb 1010002) of a player type (`athrox_male.cir`, `solitus_female.cir`, …).
pub fn player_model(store: &RecordStore, breed: Breed, gender: Gender) -> Result<u32> {
    let name = format!("{}_{}.cir", breed.model_race(), gender.name());
    NameTable::load(store)?
        .id(CHAR_MESH_TYPE, &name)
        .with_context(|| format!("no player model {name}: the client has no {breed:?} {gender:?}"))
}

/// A player character as on the creation screen: body model, head, naked skin, optionally posed with a
/// role's clip at `time` seconds (looped).
pub fn load_player(store: &RecordStore, p: &Player, pose: Option<(Role, f32)>) -> Result<Scene> {
    load_player_build(store, p, 1, pose)
}

/// Body model id of a player type with `build` 0 / 1 / 2 = `_thin` / normal / `_fat` (`CCCharacter_t::ChangeMesh`,
/// GUI 0x1011ab53: `<race>_<sex><suffix>.cir`).
pub fn player_model_build(store: &RecordStore, breed: Breed, gender: Gender, build: u8) -> Result<u32> {
    let suffix = match build {
        0 => "_thin",
        2 => "_fat",
        _ => "",
    };
    let name = format!("{}_{}{suffix}.cir", breed.model_race(), gender.name());
    NameTable::load(store)?
        .id(CHAR_MESH_TYPE, &name)
        .with_context(|| format!("no player model {name}: the client has no {breed:?} {gender:?}"))
}

/// [`load_player`] with the body of `build` ([`player_model_build`]).
pub fn load_player_build(store: &RecordStore, p: &Player, build: u8, pose: Option<(Role, f32)>) -> Result<Scene> {
    let names = NameTable::load(store)?;
    let model = player_model_build(store, p.breed, p.gender, build)?;
    let all = heads(&names, p.breed, p.gender, p.skin)?;
    let head = match p.head {
        Some(n) => all.iter().find(|h| h.0 == n).with_context(|| format!("no head {n} for {p:?}"))?.1,
        None => all.first().with_context(|| format!("no heads for {p:?}"))?.1,
    };
    let pose = pose.map(|(r, t)| role_anim(store, model, &r).map(|a| (a, t))).transpose()?;
    load_character_head_skin(store, model, Some(head), pose, &skin_swaps(&names, store, model, p.skin)?)
}

/// [`load_player`] with the default (caucasian) skin: the (breed, gender, head index) tuple of the
/// creation screen.
pub fn load_player_character(store: &RecordStore, breed: Breed, gender: Gender, head: u32, pose: Option<(Role, f32)>) -> Result<Scene> {
    load_player(store, &Player { breed, gender, skin: Skin::Caucasian, head: Some(head) }, pose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_roundtrip() {
        for (r, n) in ROLE_NAMES {
            assert_eq!(r.clip_name(), *n);
            assert_eq!(n.parse::<Role>().unwrap(), *r);
        }
        assert_eq!("idle".parse::<Role>().unwrap(), Role::Idle);
        assert_eq!("social-bow".parse::<Role>().unwrap(), Role::Emote("bow".into()));
        assert_eq!(Role::Emote("bow".into()).clip_name(), "social-bow");
    }

    #[test]
    fn clip_set_choice() {
        assert_eq!(clip_sets("athrox_male.cir"), ["athrox", "male", "athrox_male", "athrox"]);
        assert_eq!(clip_sets("opifex_female.cir")[0], "female");
        assert_eq!(clip_sets("solitus_male_fat.cir")[0], "male");
        assert_eq!(clip_sets("rhinoman.cir")[0], "rhinoman");
    }

    #[test]
    fn enums_parse() {
        assert_eq!("Atrox".parse::<Breed>().unwrap(), Breed::Atrox);
        assert!("troll".parse::<Breed>().unwrap_err().contains("solitus"));
        assert_eq!("african".parse::<Skin>().unwrap(), Skin::African);
    }
}
