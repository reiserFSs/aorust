//! How a client dynel that is not a `SimpleChar_t` gets its look: corpses, vending machines, weapon
//! items, doors, terminals, ... Evidence, addresses and captured examples: `docs/zone/static.md`.
//!
//! Everything here is a pure function of data the caller already holds (the stat pairs of a
//! `*FullUpdate` message) plus read-only rdb lookups; nothing depends on the network crate.
//!
//! * [`item_template`] reads rdb 1000020 (`0xF4254`) `[StaticInstance]`, the record
//!   `n3ObjectFactory_t::CreateRibosome` [N3 0x1000bd48] / `FUN_10080169` [GC] load for every item-like dynel.
//! * [`effective_stats`] merges it with the message stats exactly like `FUN_100a15f8` [GC].
//! * [`visual`] is `FUN_10086f20` [GC], the visual initialiser shared by `Chest_t` and everything below it
//!   (vending machine, doors, terminals, weapon items, ...).
//! * [`corpse_visual`] is the `Corpse_t` override `FUN_1007e7e2` [GC].
//! * [`placed_dynels`] reads the static dynels a playfield creates itself (`CreateRDBDynels` [GC 0x10121dcb]).

use crate::character::NameTable;
use crate::mesh::MESH_TYPE;
use anyhow::{bail, ensure, Context, Result};
use ao_rdb::RecordStore;

/// rdb type `0xF4254` of the item/dynel template records, keyed by the dynel's `StaticInstance` stat.
pub const ITEM_TEMPLATE_TYPE: u32 = 1_000_020;
/// rdb type `0xF425A` (`PlayfieldDynelData_t`), keyed by playfield id.
pub const PLAYFIELD_DYNELS_TYPE: u32 = 1_000_026;
/// The mesh `FUN_10086f20` falls back to when neither `Mesh` nor `CATMesh` is set (a name lookup,
/// `InstanceManager_t::GetTypeInstance(0xf6951, "pickupbox_misc.abiff")`).
pub const DEFAULT_MESH_NAME: &str = "pickupbox_misc.abiff";

/// Stat ids the visual code reads (names from `stat_names.txt`, i.e. the client's own table).
pub mod stat {
    pub const FLAGS: u32 = 0;
    pub const BREED: u32 = 4;
    pub const MESH: u32 = 12;
    pub const STATIC_INSTANCE: u32 = 23;
    pub const CAT_MESH: u32 = 42;
    pub const SEX: u32 = 59;
    pub const HEAD_MESH: u32 = 64;
    pub const RACE: u32 = 89;
    pub const WEAPON_MESH: u32 = 209;
    pub const CAN_CHANGE_CLOTHES: u32 = 223;
    /// No name in the client's table; read by `FUN_10086f20` and passed to `SetOverrideTexture`.
    pub const OVERRIDE_TEXTURE: u32 = 336;
    pub const MONSTER_SCALE: u32 = 360;
    pub const CORPSE_TYPE: u32 = 415;
    pub const CORPSE_INSTANCE: u32 = 416;
    pub const ANIM_POS: u32 = 500;
    pub const ANIM_PLAY: u32 = 501;
}

/// Last value of `id` (the client applies the pairs in order, later ones win).
pub fn get(stats: &[(u32, i32)], id: u32) -> Option<i32> {
    stats.iter().rev().find(|s| s.0 == id).map(|s| s.1)
}

/// The dynel class (`*Ribosome_t` / `*_t`) that the kind of an rdb 1000020 / 1000026 entry or of a
/// `*FullUpdate` identity creates. Template kinds: ribosome registrations at GC 0x101528ad..0x10152a08;
/// 0xC74A, 0xC76A: the message handlers' `kind` checks (GC 0x100a27a5, 0x1009f5a4 / `GetItemByTemplate`).
pub fn dynel_class(kind: u32) -> Option<&'static str> {
    Some(match kind {
        0xC75B => "VendingMachine",
        0xC790 => "PlayerShop",
        0xC748 | 0xDAC6 | 0xC73A => "Door",
        0xC73D | 0xC758 | 0xC77E | 0xC77F | 0xC757 | 0xC75C | 0xC76F | 0xC353 | 0xC76D | 0xC76E => "SimpleItem",
        0xC418 => "CityTerminal",
        0xDAC1 => "QuestBooth",
        0xC773 => "ReclaimBooth",
        0xC742 => "CentralController",
        0xC74A => "WeaponItem",
        0xC76A => "Corpse",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------- item template

/// The first elements of an rdb 1000020 record. The record is `u32 kind; u32 element count;` then
/// elements `{u32 type; u32 sub; payload}` (`FUN_1002b297` [GC]); element `{0xF, 0x17}` is always first
/// (all 119 540 records) and is the stat list, `{0x15, 0x21}` the name and description.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemTemplate {
    /// First `u32`: the dynel kind, see [`dynel_class`].
    pub kind: u32,
    pub stats: Vec<(u32, i32)>,
    pub name: Option<String>,
}

impl ItemTemplate {
    pub fn stat(&self, id: u32) -> Option<i32> {
        get(&self.stats, id)
    }
}

/// Parse the leading stat list (and name, if present) of an rdb 1000020 record.
pub fn parse_item_template(rec: &[u8]) -> Result<ItemTemplate> {
    let mut at = 0usize;
    let u32_at = |at: &mut usize| -> Result<u32> {
        let b = rec.get(*at..*at + 4).context("item template truncated")?;
        *at += 4;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    };
    let kind = u32_at(&mut at)?;
    let elements = u32_at(&mut at)?;
    ensure!(elements > 0, "item template without elements");
    let (ty, sub) = (u32_at(&mut at)?, u32_at(&mut at)?);
    ensure!((ty, sub) == (0xF, 0x17), "item template starts with element ({ty:#x}, {sub:#x}), not the stat list");
    // container size word: (n + 1) * 0x3F1
    let w = u32_at(&mut at)?;
    ensure!(w >= 0x3F1 && w % 0x3F1 == 0, "bad stat list size word {w:#x}");
    let n = (w / 0x3F1 - 1) as usize;
    ensure!(n <= (rec.len() - at) / 8, "stat list of {n} pairs exceeds the record");
    let mut stats = Vec::with_capacity(n);
    for _ in 0..n {
        let (id, v) = (u32_at(&mut at)?, u32_at(&mut at)?);
        stats.push((id, v as i32));
    }
    // optional name element {0x15, 0x21}: u16 name length, u16 description length, name, description
    let name = (|| {
        if elements < 2 || rec.get(at..at + 8)? != [0x15, 0, 0, 0, 0x21, 0, 0, 0] {
            return None;
        }
        let len = u16::from_le_bytes(rec.get(at + 8..at + 10)?.try_into().ok()?) as usize;
        let s = rec.get(at + 12..at + 12 + len)?;
        Some(String::from_utf8_lossy(s).into_owned())
    })();
    Ok(ItemTemplate { kind, stats, name })
}

/// `None` when rdb 1000020 has no record for `static_instance`.
pub fn item_template(store: &RecordStore, static_instance: u32) -> Result<Option<ItemTemplate>> {
    match store.get(ITEM_TEMPLATE_TYPE, static_instance)? {
        Some(rec) => Ok(Some(parse_item_template(&rec).with_context(|| format!("rdb {ITEM_TEMPLATE_TYPE}:{static_instance}"))?)),
        None => Ok(None),
    }
}

/// The stats the dynel ends up with: the template record's, then every message pair on top
/// (`FUN_100a15f8`: `FUN_10080169` loads the record, then each message stat is `SetStat`).
pub fn effective_stats(template: Option<&ItemTemplate>, message: &[(u32, i32)]) -> Vec<(u32, i32)> {
    let mut v = template.map(|t| t.stats.clone()).unwrap_or_default();
    for &(id, val) in message {
        match v.iter_mut().find(|s| s.0 == id) {
            Some(s) => s.1 = val,
            None => v.push((id, val)),
        }
    }
    v
}

/// `StaticInstance` (stat 23) of a message, the key of its template record; `None` if absent or 0.
pub fn static_instance(message: &[(u32, i32)]) -> Option<u32> {
    get(message, stat::STATIC_INSTANCE).filter(|&v| v > 0).map(|v| v as u32)
}

// ---------------------------------------------------------------------------- visual

/// What `FUN_10086f20` gives a `Chest_t`-family dynel.
#[derive(Debug, Clone, PartialEq)]
pub struct DynelVisual {
    /// rdb 1010001 static mesh (`n3VisualDynel_t::SetMesh` [N3 0x10019e8f] builds `{0xf6951, id}`).
    pub mesh: Option<u32>,
    /// rdb 1010002 CAT model (`SetCatMesh` [N3 0x10019fb2] builds `{0xf6952, id}`).
    pub cat_mesh: Option<u32>,
    /// `CanChangeClothes` > 0: the CAT model takes cloth layers.
    pub can_change_clothes: bool,
    /// Stat 336, an rdb 1010004 texture id for `SetOverrideTexture`.
    pub override_texture: Option<u32>,
    /// `MonsterScale / 100` (`SetBodyScale`); `1.0` when the stat is absent.
    pub scale: f32,
    /// `Flags` bit 0 (`FUN_10086f20`: `if (!(vtable+0x14)(1)) DisableVisibility()`).
    pub visible: bool,
}

/// `FUN_10086f20` [GC 0x10086f20] on already merged stats. `default_mesh` is [`default_mesh`].
pub fn visual(stats: &[(u32, i32)], default_mesh: u32) -> DynelVisual {
    let pos = |id| get(stats, id).filter(|&v| v > 0).map(|v| v as u32);
    let (mut mesh, cat_mesh) = (pos(stat::MESH), pos(stat::CAT_MESH));
    if mesh.is_none() && cat_mesh.is_none() {
        mesh = Some(default_mesh);
    }
    DynelVisual {
        mesh,
        cat_mesh,
        can_change_clothes: get(stats, stat::CAN_CHANGE_CLOTHES).is_some_and(|v| v > 0),
        override_texture: pos(stat::OVERRIDE_TEXTURE),
        scale: get(stats, stat::MONSTER_SCALE).map_or(1.0, |v| v as f32 / 100.0),
        visible: get(stats, stat::FLAGS).is_some_and(|f| f & 1 != 0),
    }
}

/// rdb 1010001 id of [`DEFAULT_MESH_NAME`] (9013 in the shipped client).
pub fn default_mesh(names: &NameTable) -> Result<u32> {
    names.id(MESH_TYPE, DEFAULT_MESH_NAME).with_context(|| format!("{DEFAULT_MESH_NAME} is not in the name table"))
}

/// Visual of any item-like dynel from its `*FullUpdate` stat pairs: loads the `StaticInstance`
/// template record when there is one, merges, applies [`visual`]. Unknown template record: message
/// stats only (the client does the same: `FUN_10080169` returns 0).
pub fn item_visual(store: &RecordStore, names: &NameTable, message: &[(u32, i32)]) -> Result<DynelVisual> {
    let template = match static_instance(message) {
        Some(id) => item_template(store, id)?,
        None => None,
    };
    Ok(visual(&effective_stats(template.as_ref(), message), default_mesh(names)?))
}

// ---------------------------------------------------------------------------- weapons

/// The mesh a held weapon shows: stat `WeaponMesh` (209) of its template record, an rdb 1010001 id. It is the
/// mesh the holder's `AppearanceUpdate` attractor carries; the weapon dynel itself is invisible while it has
/// a parent (`FUN_100a1216`: `DisableVisibility`).
pub fn weapon_mesh(template: &ItemTemplate) -> Option<u32> {
    template.stat(stat::WEAPON_MESH).filter(|&v| v > 0).map(|v| v as u32)
}

/// `AttractorPlace_e` for a weapon in body location `CurrBodyLocation` (message byte `byte_71`): the client
/// treats 6 and 8 as the weapon hands (`FUN_10047873` [GC]); place 1 = `Attractor02_righthand`,
/// 2 = `Attractor03_lefthand`. [INFERENCE] from the captures (docs/zone/static.md §4), not traced to the enum.
pub fn weapon_attractor_place(body_location: i32) -> Option<u8> {
    match body_location {
        6 => Some(1),
        8 => Some(2),
        _ => None,
    }
}

// ---------------------------------------------------------------------------- corpses

/// One worn cloth entry of a corpse (`ClothData_t` +0 / +4 / +8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClothLayer {
    /// 0 hands, 1 body, 2 feet, 3 arms, 4 legs (`ClothData_t::GetName`).
    pub part: i32,
    /// rdb 1010004 texture drawn as layer 2 (0 clears it).
    pub texture: u32,
    /// Layer 3 texture, only set when non-zero.
    pub texture2: u32,
}

/// `Corpse_t` look (`FUN_1007e7e2` [GC 0x1007e7e2] after the `CATMesh` stat selected the model).
#[derive(Debug, Clone, PartialEq)]
pub struct CorpseVisual {
    /// rdb 1010002 model of the dead character (stat `CATMesh`).
    pub cat_mesh: u32,
    /// `MonsterScale / 100`.
    pub scale: f32,
    /// rdb 1010001 head mounted on attractor place 0 (stat `HeadMesh`, skipped when 0).
    pub head_mesh: Option<u32>,
    /// `SetSkinData(breed, sex, race)` inputs, stats 4 / 59 / 89.
    pub breed: i32,
    pub sex: i32,
    pub race: i32,
    /// Cloth textures in wire order.
    pub cloth: Vec<ClothLayer>,
    /// `TextureData_t` list: `(material name, rdb 1010004 texture)` retexturing materials of the model.
    pub textures: Vec<(String, u32)>,
}

/// Look of a `CorpseFullUpdate`: `stats` = its `DynelBase` stat pairs, `cloth` = `(part, texture, extra texture)`
/// per `ClothData_t`, `textures` = its `TextureData_t` list. `Err` if no `CATMesh` (the client would have no model).
pub fn corpse_visual(stats: &[(u32, i32)], cloth: &[(i32, i32, i32)], textures: &[(&str, i32)]) -> Result<CorpseVisual> {
    let Some(cat_mesh) = get(stats, stat::CAT_MESH).filter(|&v| v > 0) else {
        bail!("corpse without CATMesh stat");
    };
    let id = |v: i32| v.max(0) as u32;
    Ok(CorpseVisual {
        cat_mesh: cat_mesh as u32,
        // the constructor sets MonsterScale to 100 (`FUN_1007e652`), the message overwrites it
        scale: get(stats, stat::MONSTER_SCALE).unwrap_or(100) as f32 / 100.0,
        head_mesh: get(stats, stat::HEAD_MESH).filter(|&v| v > 0).map(|v| v as u32),
        breed: get(stats, stat::BREED).unwrap_or(0),
        sex: get(stats, stat::SEX).unwrap_or(0),
        race: get(stats, stat::RACE).unwrap_or(0),
        cloth: cloth.iter().map(|&(part, t, t2)| ClothLayer { part, texture: id(t), texture2: id(t2) }).collect(),
        textures: textures.iter().map(|&(m, t)| (m.to_owned(), id(t))).collect(),
    })
}

// ---------------------------------------------------------------------------- playfield-placed dynels

/// One `DynelData_t` of `PlayfieldDynelData_t` (rdb 1000026): a door, terminal, shop, ... the playfield
/// creates by itself (`PlayfieldAnarchy_t::CreateRDBDynels` [GC 0x10121dcb]).
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedDynel {
    /// Identity kind (see [`dynel_class`]) and instance (`0xC0000000 | index << 16 | playfield`, e.g. 0xC00011E6 in 4582).
    pub kind: u32,
    pub instance: u32,
    pub playfield: u32,
    /// World position as stored (Y up).
    pub position: [f32; 3],
    /// Quaternion `(x, y, z, w)` (the record stores `w, x, y, z`).
    pub rotation: [f32; 4],
    /// Key of the rdb 1000020 template record (`CreateFromTemplate`'s first argument); 0 = none.
    pub template: u32,
    /// Extra stat stream (`FUN_1002b297`), not decoded.
    pub blob: Vec<u8>,
}

/// Parse an rdb 1000026 record: `u32 n`, `n × { u32 size; DynelData }`, 12 zero bytes. `DynelData_t`
/// `operator>>` [GD 0x10005fbf]: `i32 kind, instance, ?, +0xc, +8, playfield; f32 x y z, w qx qy qz; i32 +0x30,
/// template; i32 len; bytes`.
pub fn parse_placed_dynels(rec: &[u8]) -> Result<Vec<PlacedDynel>> {
    let mut at = 0usize;
    let u32_at = |at: &mut usize| -> Result<u32> {
        let b = rec.get(*at..*at + 4).context("playfield dynel record truncated")?;
        *at += 4;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    };
    let n = u32_at(&mut at)? as usize;
    ensure!(n <= rec.len() / 68, "{n} dynels cannot fit in {} bytes", rec.len());
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let size = u32_at(&mut at)? as usize;
        let start = at;
        let (kind, instance) = (u32_at(&mut at)?, u32_at(&mut at)?);
        at += 12; // three ints the client keeps in DynelData_t but CreateRDBDynels never reads
        let playfield = u32_at(&mut at)?;
        let f = |at: &mut usize| u32_at(at).map(f32::from_bits);
        let position = [f(&mut at)?, f(&mut at)?, f(&mut at)?];
        let w = f(&mut at)?;
        let rotation = [f(&mut at)?, f(&mut at)?, f(&mut at)?, w];
        at += 4;
        let template = u32_at(&mut at)?;
        let len = u32_at(&mut at)? as usize;
        let blob = rec.get(at..at.checked_add(len).context("blob length overflow")?).context("dynel blob truncated")?.to_vec();
        at += len;
        ensure!(at - start == size, "dynel entry is {} bytes, its size word says {size}", at - start);
        out.push(PlacedDynel { kind, instance, playfield, position, rotation, template, blob });
    }
    ensure!(rec.len() - at == 12, "{} bytes after the dynel list, expected the 12 byte trailer", rec.len() - at);
    Ok(out)
}

/// Per-instance stat overrides inside [`PlacedDynel::blob`]. The blob is a big endian `BinaryStream` of 12 prefix
/// bytes (`0x66, 0x67, 0x3F1`; `CreateRDBDynels` skips them) and `FUN_1002b297` elements `{type, sub, payload}`;
/// the stats are the `{0xF, 0x18}` (or 0x17 / 0x2B) element, a `(n + 1) * 0x3F1` size word and `n` `(i32 stat,
/// i32 value)` pairs. Elements before it can be `SpellData_t` lists whose layout is not fully decoded
/// (docs/zone/static.md §5), so the element is **located by scanning** for its header instead of walking the
/// list: [GUESS], but over all 12 509 placed dynels of the shipped client exactly one valid header exists in
/// every blob that has stats (8 415) and every `Mesh` it sets is a real rdb 1010001 id.
pub fn blob_stats(blob: &[u8]) -> Result<Vec<(u32, i32)>> {
    let be = |p: usize| blob.get(p..p + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let mut found = None;
    for p in 16..blob.len().saturating_sub(11) {
        if be(p) != Some(0xF) || !matches!(be(p + 4), Some(0x17 | 0x18 | 0x2B)) {
            continue;
        }
        let Some(w) = be(p + 8).filter(|&w| w >= 0x3F1 && w % 0x3F1 == 0) else { continue };
        let n = (w / 0x3F1 - 1) as usize;
        if n > (blob.len() - p - 12) / 8 {
            continue;
        }
        ensure!(found.is_none(), "two stat lists in a placed dynel blob");
        found = Some((p + 12, n));
    }
    let Some((at, n)) = found else { return Ok(Vec::new()) };
    Ok((0..n).map(|i| (be(at + 8 * i).unwrap(), be(at + 8 * i + 4).unwrap() as i32)).collect())
}

/// Look of a placed dynel: its template record's stats with the blob's on top, then [`visual`].
/// `None` for `template == 0` (the client creates nothing then).
pub fn placed_visual(store: &RecordStore, names: &NameTable, d: &PlacedDynel) -> Result<Option<DynelVisual>> {
    if d.template == 0 {
        return Ok(None);
    }
    let template = item_template(store, d.template)?;
    let stats = effective_stats(template.as_ref(), &blob_stats(&d.blob)?);
    Ok(Some(visual(&stats, default_mesh(names)?)))
}

/// The dynels playfield `playfield` creates itself; empty if it has no record.
pub fn placed_dynels(store: &RecordStore, playfield: u32) -> Result<Vec<PlacedDynel>> {
    match store.get(PLAYFIELD_DYNELS_TYPE, playfield)? {
        Some(rec) => parse_placed_dynels(&rec).with_context(|| format!("rdb {PLAYFIELD_DYNELS_TYPE}:{playfield}")),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u32, stats: &[(u32, i32)], name: &str) -> Vec<u8> {
        let mut v = Vec::new();
        for w in [kind, 2, 0xF, 0x17, (stats.len() as u32 + 1) * 0x3F1] {
            v.extend(w.to_le_bytes());
        }
        for &(id, val) in stats {
            v.extend(id.to_le_bytes());
            v.extend(val.to_le_bytes());
        }
        for w in [0x15u32, 0x21] {
            v.extend(w.to_le_bytes());
        }
        v.extend((name.len() as u16).to_le_bytes());
        v.extend(0u16.to_le_bytes());
        v.extend(name.as_bytes());
        v
    }

    #[test]
    fn template_parses_and_rejects_garbage() {
        let rec = record(0xC74A, &[(12, 9013), (209, 7796)], "Polished Eliminator");
        let t = parse_item_template(&rec).unwrap();
        assert_eq!((t.kind, t.stat(209), t.stat(12), t.name.as_deref()), (0xC74A, Some(7796), Some(9013), Some("Polished Eliminator")));
        assert_eq!(weapon_mesh(&t), Some(7796));
        assert_eq!(dynel_class(t.kind), Some("WeaponItem"));
        for cut in 0..28 {
            assert!(parse_item_template(&rec[..cut]).is_err());
        }
        let mut bad = rec.clone();
        bad[16..20].copy_from_slice(&0xFFFF_FFF1u32.to_le_bytes()); // size word that claims 4 billion pairs
        assert!(parse_item_template(&bad).is_err());
        bad[16..20].copy_from_slice(&5u32.to_le_bytes()); // not a multiple of 0x3F1
        assert!(parse_item_template(&bad).is_err());
    }

    #[test]
    fn message_stats_override_template_and_default_mesh_applies() {
        let t = parse_item_template(&record(0xC75B, &[(0, 3), (12, 100), (360, 50)], "")).unwrap();
        let eff = effective_stats(Some(&t), &[(12, 200), (23, 7)]);
        assert_eq!((get(&eff, 12), get(&eff, 360), get(&eff, 23)), (Some(200), Some(50), Some(7)));
        let v = visual(&eff, 9013);
        assert_eq!((v.mesh, v.cat_mesh, v.scale, v.visible), (Some(200), None, 0.5, true));
        // no Mesh, no CATMesh: pickupbox
        assert_eq!(visual(&[(0, 1)], 9013).mesh, Some(9013));
        // CATMesh alone: no static mesh, not the box
        let c = visual(&[(0, 1), (42, 45857), (223, 1)], 9013);
        assert_eq!((c.mesh, c.cat_mesh, c.can_change_clothes), (None, Some(45857), true));
        // Flags bit 0 clear: hidden
        assert!(!visual(&[(0, 0x402)], 9013).visible);
    }

    #[test]
    fn corpse_needs_a_model() {
        assert!(corpse_visual(&[(360, 90)], &[], &[]).is_err());
        let c = corpse_visual(&[(42, 15222), (360, 181), (64, 0), (4, 6)], &[(1, 0, 0), (2, 77, 5)], &[("body", 9)]).unwrap();
        assert_eq!((c.cat_mesh, c.scale, c.head_mesh, c.breed), (15222, 1.81, None, 6));
        assert_eq!(c.cloth[1], ClothLayer { part: 2, texture: 77, texture2: 5 });
        assert_eq!(c.textures, [("body".to_owned(), 9)]);
    }

    #[test]
    fn blob_stat_scan() {
        let mut b = vec![0, 0, 0, 0x66, 0, 0, 0, 0x67, 0, 0, 0x03, 0xF1, 0, 0, 0, 1];
        for w in [0xFu32, 0x18, 2 * 0x3F1, 12, 258365] {
            b.extend(w.to_be_bytes());
        }
        b.extend([0, 0x15, 0, 0]); // trailing element bytes are not part of the list
        assert_eq!(blob_stats(&b).unwrap(), [(12, 258365)]);
        assert!(blob_stats(&b[..b.len() - 12]).unwrap().is_empty()); // list cut off: not a list
        assert!(blob_stats(&[]).unwrap().is_empty());
    }

    #[test]
    fn placed_dynels_roundtrip_and_reject() {
        let mut rec = 1u32.to_le_bytes().to_vec();
        let mut e = Vec::new();
        for w in [0xC748u32, 0xC000_11E6, 1, 0, 0, 4582] {
            e.extend(w.to_le_bytes());
        }
        for f in [936.5f32, 47.5, 888.25, 0.5, 0.0, -0.5, 0.0] {
            e.extend(f.to_le_bytes());
        }
        for w in [0u32, 41565, 3] {
            e.extend(w.to_le_bytes());
        }
        e.extend([1, 2, 3]);
        rec.extend((e.len() as u32).to_le_bytes());
        rec.extend(&e);
        rec.extend([0u8; 12]);
        let d = parse_placed_dynels(&rec).unwrap();
        assert_eq!((d[0].kind, d[0].playfield, d[0].template, d[0].position, d[0].rotation, d[0].blob.len()), (0xC748, 4582, 41565, [936.5, 47.5, 888.25], [0.0, -0.5, 0.0, 0.5], 3));
        for cut in 0..rec.len() {
            assert!(parse_placed_dynels(&rec[..cut]).is_err(), "cut {cut}");
        }
        let mut bad = rec.clone();
        bad[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_placed_dynels(&bad).is_err());
    }
}
