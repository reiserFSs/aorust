//! Player characters and animation roles, resolved through the client's resource name table
//! (`names.rs`); see `docs/formats.md` § characters.
//!
//! * clips are named `<set>_<role>_01_01.ani` (`%s_%s_01_01.ani` in Gamecode.dll); the set is `athrox`
//!   for Atrox bodies and `male`/`female` for the other breeds (all human bodies share one skeleton);
//! * heads are meshes `head_<race><sex>[_<ethnicity>]<NN>.abiff` (`head_%s%s%02d.abiff` in GUI.dll);
//! * skin: a player's body materials (`hands body feet arms legs`) show the rdb 1010011 naked skin
//!   `<part>_<race><sex>[_<ethnicity>]_naked.png` with worn cloth textures composited over it, see [`part_textures`];
//!   the models' `*_default.png` are the textures of non-player users of the same bodies (NPCs).

use super::{load_cat_mesh, load_character_head_skin, CatAnim, NameTable, PartTextures, CHAR_ANIM_TYPE, CHAR_MESH_TYPE};
use crate::texture::load_texture;
use anyhow::{Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Scene, Texture, TextureKey};
use std::str::FromStr;

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
    /// Worn cloth per body slot ([`Equipment`]); default = nothing worn.
    pub equipment: Equipment,
}

impl Player {
    /// An unequipped player.
    pub fn new(breed: Breed, gender: Gender, skin: Skin, head: Option<u32>) -> Self {
        Self { breed, gender, skin, head, equipment: Equipment::default() }
    }
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

/// `ClothData_t::ClothPart_e`, the body slots the client textures separately (the model's materials of the same
/// names). Order and names: table behind `ClothData_t::GetName` [GameData.dll 0x1000a5f0] (`hands body feet arms legs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClothPart {
    Hands,
    Body,
    Feet,
    Arms,
    Legs,
}

impl ClothPart {
    pub const ALL: [ClothPart; 5] = [ClothPart::Hands, ClothPart::Body, ClothPart::Feet, ClothPart::Arms, ClothPart::Legs];

    pub fn name(self) -> &'static str {
        ["hands", "body", "feet", "arms", "legs"][self as usize]
    }
}

/// What a player wears, per [`ClothPart`]: the rdb 1010004 texture id the client draws over the skin
/// (`SetCATTexture(part, id, TextureLayer 2)`, Gamecode `FUN_1004b5ab` @0x1004b5ab / GUI `CharacterViewer_c::Update`),
/// `None` = nothing worn on that slot. Filled from server data by the caller; an unequipped player is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Equipment(pub [Option<u32>; 5]);

impl Equipment {
    pub fn wear(&mut self, part: ClothPart, texture: u32) {
        self.0[part as usize] = Some(texture);
    }
}

/// rdb 1010011 name of the naked skin of a body slot: `FUN_1006ff7f` [DisplaySystem 0x1006ff7f] =
/// `<part>_<breed><sex>[_<race>]_naked.png` with `sex` forced to `male` for Atrox (`breed == 4`) and the race
/// (`caucation`/`african`/`asian`) only for Solitus (`breed == 1`).
pub fn skin_texture_name(breed: Breed, gender: Gender, skin: Skin, part: ClothPart) -> String {
    let sex = if breed == Breed::Atrox { "male" } else { gender.name() };
    let race = if breed == Breed::Solitus { skin.skin_infix() } else { "" };
    format!("{}_{}{sex}{race}_naked.png", part.name(), breed.model_race())
}

/// `RGB565` green (0, 255, 0): the chroma key of cloth textures (`FUN_10074393` @0x10074393 compares the 16-bit
/// texel with `0x07e0`). The 8-bit → 565 conversion is `r>>3, g>>2, b>>3` (`RTexture_t::Load`'s rounding is not
/// traced; an unresolved guess that only matters for near-green texels).
fn is_key(px: &[u8]) -> bool {
    px[0] >> 3 == 0 && px[1] >> 2 == 0x3f && px[2] >> 3 == 0
}

/// The client's cloth-over-skin composite (`FUN_1007457f` → `FUN_10074393`): every texel of `cloth` except the
/// green key replaces the skin texel at the same position; the skin is stretched to the cloth's size.
/// A cloth without key texels is used as is.
pub fn overlay_on_skin(skin: &Texture, cloth: &Texture) -> Texture {
    if !cloth.rgba.as_chunks::<4>().0.iter().any(|c| is_key(c)) {
        return cloth.clone();
    }
    let mut out = Texture { width: cloth.width, height: cloth.height, rgba: Vec::with_capacity(cloth.rgba.len()) };
    for (i, c) in cloth.rgba.as_chunks::<4>().0.iter().enumerate() {
        if is_key(c) {
            let (x, y) = (i as u32 % cloth.width, i as u32 / cloth.width);
            let s = ((y * skin.height / cloth.height) * skin.width + x * skin.width / cloth.width) as usize * 4;
            out.rgba.extend_from_slice(&skin.rgba[s..s + 4]);
        } else {
            out.rgba.extend_from_slice(c);
        }
    }
    out
}

/// Key of a skin/cloth composite: a type with this bit set carries the skin id, `id` the cloth texture id.
const COMPOSITE: u32 = 0x4000_0000;

/// The textures of a player's body materials, as `VisualCATMesh_t::SetSkinData` [DisplaySystem 0x1007298b →
/// 0x10070439] and the cloth list set them: layer 0 = the naked skin of every [`ClothPart`]; layer 2 = the worn
/// cloth texture, composited over the skin. The model's own `*_default.png` (layer 1) is **not** used: for a
/// player (mesh with a head, `FUN_10058078` @Gamecode 0x10058078) no layer-1 texture is ever set, and the
/// record's base texture is null (`FUN_1007269e`), so `FUN_1007457f` yields the skin alone when nothing is worn.
fn part_textures(names: &NameTable, store: &RecordStore, p: &Player) -> Result<PartTextures> {
    let mut out = PartTextures::new();
    for part in ClothPart::ALL {
        let Some(id) = names.id(SKIN_TYPE, &skin_texture_name(p.breed, p.gender, p.skin, part)) else { continue };
        let key = TextureKey { rdb_type: SKIN_TYPE, id };
        let Some(skin) = load_texture(store, key)? else { continue };
        let worn = p.equipment.0[part as usize].and_then(|t| Some((t, load_texture(store, TextureKey { rdb_type: TEXTURE_TYPE, id: t }).ok()??)));
        out.insert(
            part.name().to_string(),
            match worn {
                Some((t, cloth)) => (TextureKey { rdb_type: COMPOSITE | id, id: t }, overlay_on_skin(&skin, &cloth)),
                None => (key, skin),
            },
        );
    }
    Ok(out)
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
    load_character_head_skin(store, model, Some(head), pose, &part_textures(&names, store, p)?)
}

/// [`load_player`] with the default (caucasian) skin: the (breed, gender, head index) tuple of the
/// creation screen.
pub fn load_player_character(store: &RecordStore, breed: Breed, gender: Gender, head: u32, pose: Option<(Role, f32)>) -> Result<Scene> {
    load_player(store, &Player { breed, gender, skin: Skin::Caucasian, head: Some(head), equipment: Equipment::default() }, pose)
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

    #[test]
    fn skin_names_follow_setskindata() {
        let n = |b, g, s, p| skin_texture_name(b, g, s, p);
        assert_eq!(n(Breed::Atrox, Gender::Male, Skin::African, ClothPart::Hands), "hands_athroxmale_naked.png");
        assert_eq!(n(Breed::Solitus, Gender::Male, Skin::Caucasian, ClothPart::Body), "body_solitusmale_caucation_naked.png");
        assert_eq!(n(Breed::Solitus, Gender::Female, Skin::Asian, ClothPart::Legs), "legs_solitusfemale_asian_naked.png");
        assert_eq!(n(Breed::Opifex, Gender::Female, Skin::African, ClothPart::Arms), "arms_opifexfemale_naked.png");
        assert_eq!(n(Breed::Nanomage, Gender::Male, Skin::Caucasian, ClothPart::Feet), "feet_nanomagemale_naked.png");
    }

    fn tex(w: u32, h: u32, px: &[[u8; 4]]) -> Texture {
        Texture { width: w, height: h, rgba: px.iter().flatten().copied().collect() }
    }

    #[test]
    fn cloth_shows_over_skin_except_where_green() {
        let skin = tex(2, 1, &[[200, 150, 100, 255], [210, 160, 110, 255]]);
        let cloth = tex(2, 1, &[[10, 20, 30, 255], [0, 255, 0, 255]]);
        let t = overlay_on_skin(&skin, &cloth);
        assert_eq!(t.rgba, [10, 20, 30, 255, 210, 160, 110, 255]);
        // near-green that is not 565 green (0x07e0) is cloth
        assert!(!is_key(&[8, 255, 0, 255]) && !is_key(&[0, 251, 0, 255]) && is_key(&[7, 252, 7, 255]));
        // no key texel: the cloth as is, whatever the skin size
        let opaque = tex(1, 1, &[[1, 2, 3, 4]]);
        assert_eq!(overlay_on_skin(&skin, &opaque).rgba, [1, 2, 3, 4]);
        // the skin is stretched to the cloth's size
        let big = tex(4, 2, &[[0, 255, 0, 255]; 8]);
        let t = overlay_on_skin(&skin, &big);
        assert_eq!((t.width, t.height), (4, 2));
        assert_eq!(&t.rgba[..8], [200, 150, 100, 255, 200, 150, 100, 255]);
        assert_eq!(&t.rgba[8..16], [210, 160, 110, 255, 210, 160, 110, 255]);
    }

    #[test]
    fn equipment_is_per_slot_and_empty_by_default() {
        let mut e = Equipment::default();
        assert!(ClothPart::ALL.iter().all(|p| e.0[*p as usize].is_none()));
        e.wear(ClothPart::Legs, 42);
        assert_eq!(e.0[4], Some(42));
        assert_eq!(ClothPart::ALL.map(ClothPart::name), ["hands", "body", "feet", "arms", "legs"]);
    }
}
