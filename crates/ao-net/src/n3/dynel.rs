//! Zone N3 messages about character dynels: see docs/zone/dynel.md for layouts and RE evidence.
//!
//! * `SimpleCharFullUpdateIIR_t` 0x271B3A6B: a character/NPC dynel appears (`SimpleChar_t`).
//! * `CharDCMoveIIR_t` 0x54111123: movement/placement of a dynel (client sends it too).
//! * `StatIIR_t` 0x2B333D6E, `SetWantedDirectionIIR_t` 0x60201D0E, `CastNanoSpellIIR_t` 0x25314D6D
//!   (client sends it too), `WeaponItemFullUpdateIIR_t` 0x3B1D2268, `SpecialAttackWeaponIIR_t` 0x1D3C0F1C.
//!
//! The message id is `n3InfoItemRemote_t::MapToKey(class name)` [N3 0x10009826]: `key ^= (i8)c << ((i & 3) * 8)`.

use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{ensure, Result};

pub const SIMPLE_CHAR_FULL_UPDATE: u32 = 0x271B3A6B;
pub const CHAR_DC_MOVE: u32 = 0x54111123;
pub const STAT: u32 = 0x2B333D6E;
pub const SET_WANTED_DIRECTION: u32 = 0x60201D0E;
pub const WEAPON_ITEM_FULL_UPDATE: u32 = 0x3B1D2268;
pub const SPECIAL_ATTACK_WEAPON: u32 = 0x1D3C0F1C;
pub const CAST_NANO_SPELL: u32 = 0x25314D6D;

/// Identity kind of character/NPC dynels (`SimpleCharFullUpdateIIR_t` validation requires it).
pub const KIND_CHARACTER: i32 = 50000;
/// Identity kind of weapon item dynels (`WeaponItemFullUpdateIIR_t` requires it).
pub const KIND_WEAPON_ITEM: i32 = 0xC74A;

/// `SimpleCharFullUpdate` flag bits (`+0x1c` of the IIR).
pub mod flag {
    /// Set for NPCs/monsters: selects the monster branch of the reader and of the ribosome.
    pub const NPC: u32 = 1 << 0;
    /// Stat `InPlay` (0xC2).
    pub const IN_PLAY: u32 = 1 << 1;
    /// Dynel flag 0x800 is set (and the head attractor mesh is NOT added by the full-update apply).
    pub const SET_DYNEL_800: u32 = 1 << 2;
    /// Stat `CanChangeClothes` (0xDF).
    pub const CAN_CHANGE_CLOTHES: u32 = 1 << 3;
    /// A `TextureData_t` list follows.
    pub const TEXTURES: u32 = 1 << 4;
    /// Parent identity present.
    pub const PARENT: u32 = 1 << 5;
    /// Playfield id present.
    pub const PLAYFIELD: u32 = 1 << 6;
    /// Head mesh id present (stat `HeadMesh` 0x40).
    pub const HEAD_MESH: u32 = 1 << 7;
    /// List of 16-byte entries (`+0x190`) present.
    pub const LIST_190: u32 = 1 << 8;
    /// Rotation quaternion present.
    pub const ROTATION: u32 = 1 << 9;
    /// Identity `+0xbc` present.
    pub const TARGET: u32 = 1 << 10;
    /// Health (`+0x98`) is `u16` instead of `i32`.
    pub const HEALTH_SHORT: u32 = 1 << 11;
    /// Level is `i16` instead of `u8`.
    pub const LEVEL_SHORT: u32 = 1 << 12;
    /// Run speed (`+0x10c`) is `i16` instead of `u8`.
    pub const RUN_SPEED_SHORT: u32 = 1 << 13;
    /// Current health is sent as `max - u8`.
    pub const HEALTH_DELTA: u32 = 1 << 14;
    /// Identity + up to 30 waypoints present.
    pub const PATH: u32 = 1 << 16;
    /// NPC: `+0xb2` is `u8` instead of `i16`.
    pub const NPC_B2_BYTE: u32 = 1 << 17;
    /// NPC: `+0xb4` is `u8` instead of `i16`.
    pub const NPC_B4_BYTE: u32 = 1 << 19;
    /// NPC: `+0xb6` is `u8` instead of `i16`.
    pub const NPC_B6_BYTE: u32 = 1 << 25;
    /// Byte `+0x120` present.
    pub const BYTE_120: u32 = 1 << 23;
    /// Byte `+0x121` present.
    pub const BYTE_121: u32 = 1 << 24;
    /// PC: extra strings (and `i32` for version > 0x39) present.
    pub const PC_EXTRA: u32 = 1 << 26;
    /// Byte `+0x123` present.
    pub const BYTE_123: u32 = 1 << 29;
    /// List of identities (`+0x308`) present.
    pub const IDENTITY_LIST: u32 = 1 << 30;
}

/// Everything this module decodes.
#[derive(Debug, Clone, PartialEq)]
pub enum Dynel {
    SimpleCharFullUpdate(Box<SimpleCharFullUpdate>),
    CharDCMove(CharDCMove),
    Stat(StatUpdate),
    SetWantedDirection(SetWantedDirection),
    WeaponItemFullUpdate(WeaponItemFullUpdate),
    SpecialAttackWeapon(SpecialAttackWeapon),
    CastNanoSpell(CastNanoSpell),
}

/// Decode a message of this module; `Ok(None)` if `h.msg_type` is not one of ours.
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<Dynel>> {
    Ok(Some(match h.msg_type {
        SIMPLE_CHAR_FULL_UPDATE => Dynel::SimpleCharFullUpdate(Box::new(SimpleCharFullUpdate::read(r)?)),
        CHAR_DC_MOVE => Dynel::CharDCMove(CharDCMove::read(r)?),
        STAT => Dynel::Stat(StatUpdate::read(r)?),
        SET_WANTED_DIRECTION => Dynel::SetWantedDirection(SetWantedDirection::read(r)?),
        WEAPON_ITEM_FULL_UPDATE => Dynel::WeaponItemFullUpdate(WeaponItemFullUpdate::read(r)?),
        SPECIAL_ATTACK_WEAPON => Dynel::SpecialAttackWeapon(SpecialAttackWeapon::read(r)?),
        CAST_NANO_SPELL => Dynel::CastNanoSpell(CastNanoSpell::read(r)?),
        _ => return Ok(None),
    }))
}

// ---- shared readers (mirror the client's BinaryStream helpers) ----

fn ident(r: &mut Reader) -> Result<Identity> {
    Identity::read(r)
}

fn vec3(r: &mut Reader) -> Result<[f32; 3]> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

fn quat(r: &mut Reader) -> Result<[f32; 4]> {
    Ok([r.f32()?, r.f32()?, r.f32()?, r.f32()?])
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// Element count of the client's container reader (`FUN_1002b8b6`, `FUN_10071b49`, …): the wire holds
/// `(n + 1) * 0x3f1`; anything else (or `n >= 0x7531`) flags the stream bad. `elem` = minimum bytes per element.
fn count(r: &mut Reader, elem: usize) -> Result<usize> {
    let c = r.i32()?;
    ensure!(c >= 0x3f1 && c % 0x3f1 == 0 && ((c / 0x3f1 - 1) as u32) < 0x7531, "bad container count {c:#x}");
    let n = (c / 0x3f1 - 1) as usize;
    ensure!(r.remaining() >= n * elem, "container of {n} elements exceeds the message");
    Ok(n)
}

fn rest(r: &mut Reader) -> Result<Vec<u8>> {
    Ok(r.bytes(r.remaining())?.to_vec())
}

fn pairs(r: &mut Reader) -> Result<Vec<(i32, i32)>> {
    let n = count(r, 8)?;
    (0..n).map(|_| Ok((r.i32()?, r.i32()?))).collect()
}

/// `u8` or `i16` depending on a flag (the client widens both to a word).
fn byte_or_short(r: &mut Reader, short: bool) -> Result<i16> {
    if short {
        r.i16()
    } else {
        Ok(r.u8()? as i16)
    }
}

// ---- CharDCMove ----

/// `CharDCMoveIIR_t` 0x54111123 [GC 0x1006b96f read, 0x1006b9d6 write, 0x1006b84b apply]. Body after the N3 header:
/// `u8 type (bit 7 ignored)`, quaternion `x y z w`, position `x y z`, `i32 time`, `f32 extra[2]`.
#[derive(Debug, Clone, PartialEq)]
pub struct CharDCMove {
    /// Move type (`type & 0x7f`): index of the movement state machine transition (see docs).
    pub move_type: u8,
    /// Bit 7 of the type byte (discarded by the client).
    pub type_bit7: bool,
    /// Orientation quaternion `(x, y, z, w)`; identity `(0, 0, 0, 1)`.
    pub rot: [f32; 4],
    /// Position `(x, y, z)`, Y up.
    pub pos: [f32; 3],
    /// Time stamp (`GameTime_t` on send; the server sends 0). The client clamps negative values to 0.
    pub time: i32,
    /// Two floats read by `CharDCMoveIIR_t` itself; used as `(a, b)` by move type 0x16, 0 in every capture.
    pub extra: [f32; 2],
}

impl CharDCMove {
    pub fn read(r: &mut Reader) -> Result<Self> {
        let t = r.u8()?;
        let rot = quat(r)?;
        let pos = vec3(r)?;
        let time = r.i32()?;
        let extra = [r.f32()?, r.f32()?];
        ensure!(finite(&rot) && finite(&pos) && finite(&extra), "non-finite CharDCMove");
        Ok(Self { move_type: t & 0x7f, type_bit7: t & 0x80 != 0, rot, pos, time: time.max(0), extra })
    }

    /// Full N3 payload as the client sends it (`n3InfoItemRemote_t::Write` [N3 0x100098f4]: key, identity,
    /// `pass_on` byte, body).
    pub fn encode(&self, target: Identity, pass_on: bool) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(CHAR_DC_MOVE);
        target.write(&mut w);
        w.u8(pass_on as u8);
        w.u8(self.move_type | (self.type_bit7 as u8) << 7);
        for v in self.rot.iter().chain(&self.pos) {
            w.f32(*v);
        }
        w.i32(self.time);
        w.f32(self.extra[0]);
        w.f32(self.extra[1]);
        w.0
    }

    /// Heading about the Y axis in radians, from the quaternion (`2 * atan2(y, w)`).
    pub fn yaw(&self) -> f32 {
        yaw(&self.rot)
    }
}

/// Rotation about Y (the up axis) of a `(x, y, z, w)` quaternion.
pub fn yaw(q: &[f32; 4]) -> f32 {
    2.0 * q[1].atan2(q[3])
}

// ---- Stat ----

/// `StatIIR_t` 0x2B333D6E [GC 0x100a1a7c read]: `i32 n`, `n × (i32 stat, i32 value)`. The client sets each stat
/// on the dynel and shows damage/heal numbers for Health (0x1b).
#[derive(Debug, Clone, PartialEq)]
pub struct StatUpdate {
    pub stats: Vec<(i32, i32)>,
    pub rest: Vec<u8>,
}

impl StatUpdate {
    pub fn read(r: &mut Reader) -> Result<Self> {
        let n = r.i32()?;
        ensure!(n >= 0 && (n as usize) <= r.remaining() / 8, "stat count {n} exceeds the message");
        let stats = (0..n).map(|_| Ok((r.i32()?, r.i32()?))).collect::<Result<_>>()?;
        Ok(Self { stats, rest: rest(r)? })
    }
}

// ---- SetWantedDirection ----

/// `SetWantedDirectionIIR_t` 0x60201D0E [GC 0x1003a71b read, 0x1003a78a apply]: three floats stored at
/// `SimpleChar_t+0x1f8` (wanted movement direction, a unit vector in the XZ plane live).
#[derive(Debug, Clone, PartialEq)]
pub struct SetWantedDirection {
    pub dir: [f32; 3],
    pub rest: Vec<u8>,
}

impl SetWantedDirection {
    pub fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self { dir: vec3(r)?, rest: rest(r)? })
    }
}

// ---- CastNanoSpell ----

/// `CastNanoSpellIIR_t` 0x25314D6D [GC 0x10072274 read, 0x100722c6 write, 0x10072306 apply]:
/// `i32 spell`, `Identity target`, `i32 flag`, `Identity source`.
#[derive(Debug, Clone, PartialEq)]
pub struct CastNanoSpell {
    pub spell: i32,
    pub target: Identity,
    /// Read as `i32`, kept as `bool` (`!= 0`) by the client; passed as the 4th argument of `FUN_10051754`.
    pub flag: bool,
    /// Second identity (`+0x28`); in the capture equal to the caster (header identity).
    pub source: Identity,
    pub rest: Vec<u8>,
}

impl CastNanoSpell {
    pub fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self { spell: r.i32()?, target: ident(r)?, flag: r.i32()? != 0, source: ident(r)?, rest: rest(r)? })
    }

    /// Full N3 payload as the client sends it.
    pub fn encode(&self, caster: Identity, pass_on: bool) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(CAST_NANO_SPELL);
        caster.write(&mut w);
        w.u8(pass_on as u8);
        w.i32(self.spell);
        self.target.write(&mut w);
        w.i32(self.flag as i32);
        self.source.write(&mut w);
        w.bytes(&self.rest);
        w.0
    }
}

// ---- SpecialAttackWeapon ----

/// One special attack of the wielded weapon: four `i32` read in the order `f0 f1 f3 f2`
/// (`FUN_10065687` [GC]). `f2` is four ASCII characters in every capture; `f3` equals `f2` on NPCs and is a
/// small number (100, 144, 142, 1) on the player. Meanings unresolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialAttackEntry {
    pub f0: i32,
    pub f1: i32,
    pub f2: u32,
    pub f3: u32,
}

impl SpecialAttackEntry {
    /// The four-character code (`f2`, big-endian bytes) if printable ASCII, e.g. `"MAAT"`.
    pub fn code(&self) -> Option<String> {
        let b = self.f2.to_be_bytes();
        b.iter().all(|c| c.is_ascii_graphic()).then(|| String::from_utf8_lossy(&b).into_owned())
    }
}

/// `SpecialAttackWeaponIIR_t` 0x1D3C0F1C [GC 0x10079a83 read, 0x100799fc write, 0x1007989a apply]: a list of special
/// attacks, then five `i32` that the client stores as stats 0x76, 0x77, 0x78, 0x95, 0x33.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecialAttackWeapon {
    pub attacks: Vec<SpecialAttackEntry>,
    /// Stat `CloseCombatInitiative` (0x76).
    pub close_combat_initiative: i32,
    /// Stat `DistanceWeaponInitiative` (0x77).
    pub distance_weapon_initiative: i32,
    /// Stat `PhysicalProwessInitiative` (0x78).
    pub physical_prowess_initiative: i32,
    /// Stat `NanoProwessInitiative` (0x95).
    pub nano_prowess_initiative: i32,
    /// Stat `AggDef` (0x33).
    pub agg_def: i32,
    pub rest: Vec<u8>,
}

impl SpecialAttackWeapon {
    pub fn read(r: &mut Reader) -> Result<Self> {
        let n = count(r, 16)?;
        let attacks = (0..n)
            .map(|_| {
                let (f0, f1, f3, f2) = (r.i32()?, r.i32()?, r.u32()?, r.u32()?);
                Ok(SpecialAttackEntry { f0, f1, f2, f3 })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            attacks,
            close_combat_initiative: r.i32()?,
            distance_weapon_initiative: r.i32()?,
            physical_prowess_initiative: r.i32()?,
            nano_prowess_initiative: r.i32()?,
            agg_def: r.i32()?,
            rest: rest(r)?,
        })
    }
}

// ---- WeaponItemFullUpdate ----

/// `WeaponItemFullUpdateIIR_t` 0x3B1D2268, identity kind 0xC74A [GC 0x100a2754 read via `FUN_100a110a`,
/// 0x100a27a5 apply]: a weapon item dynel (carried by `parent` or lying at `pos`).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponItemFullUpdate {
    /// Layout version, must be 11 (`DAT_101c1160`).
    pub version: i32,
    /// Holder (e.g. an NPC `(50000, id)`); `(0, 0)` when the item is free in the playfield.
    pub parent: Identity,
    /// Only if `parent` is null: position and rotation `(x, y, z, w)` (an all-zero rotation becomes identity).
    pub placement: Option<([f32; 3], [f32; 4])>,
    /// Playfield id (`GetPlayfield(+0x48)`).
    pub playfield: i32,
    /// Identity at `+0x4c`; `(1000015, 0)` in the capture. Meaning unresolved.
    pub template: Identity,
    pub byte_70: u8,
    pub byte_71: u8,
    /// `(stat id, value)` list, e.g. `(0x17 StaticInstance, 265090)`, `(0x2bd ACGItemLevel, 25)`.
    pub stats: Vec<(i32, i32)>,
    /// `i32 len` + raw bytes (only read if `len > 0`).
    pub blob: Vec<u8>,
    pub rest: Vec<u8>,
}

impl WeaponItemFullUpdate {
    pub fn read(r: &mut Reader) -> Result<Self> {
        let version = r.i32()?;
        ensure!(version == 11, "WeaponItemFullUpdate version {version}, client accepts 11");
        let parent = ident(r)?;
        let placement = if parent == Identity::default() {
            let pos = vec3(r)?;
            let mut rot = quat(r)?;
            if rot == [0.0; 4] {
                rot = [0.0, 0.0, 0.0, 1.0];
            }
            Some((pos, rot))
        } else {
            None
        };
        let playfield = r.i32()?;
        let template = ident(r)?;
        let (byte_70, byte_71) = (r.u8()?, r.u8()?);
        let stats = pairs(r)?;
        let len = r.i32()?;
        let blob = if len > 0 {
            ensure!(len as usize <= r.remaining(), "blob of {len} bytes exceeds the message");
            r.bytes(len as usize)?.to_vec()
        } else {
            Vec::new()
        };
        Ok(Self { version, parent, placement, playfield, template, byte_70, byte_71, stats, blob, rest: rest(r)? })
    }
}

// ---- SimpleCharFullUpdate ----

/// `TextureData_t` [GameData 0x1000c0b4]: `char name[32]`, `i32`, `i32`, `i32`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureData {
    /// Material name of the CAT model (NUL trimmed from the 32-byte field).
    pub material: String,
    /// rdb 1010004 texture id (`+0x20`), e.g. 22768 = `lizard_brown.png`.
    pub texture: i32,
    /// `+0x24`, 0 in every capture.
    pub field_24: i32,
    /// Third `i32`; the client stores `5` if non-zero and `0` otherwise (`+0x28`). 0 in every capture.
    pub flag: i32,
}

/// `ClothData_t` [GameData 0x1000a661]: worn cloth of one body part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClothData {
    /// First `i32` as sent: low 16 bits (sign-extended) = part (`hands body feet arms legs` = 0..4, the table
    /// behind `ClothData_t::GetName`), high 16 bits = number of extra `i32` pairs (only `> 0` means "present").
    pub raw: i32,
    /// rdb 1010004 texture id drawn over the skin (`+4`); 0 = nothing worn on this part.
    pub texture: i32,
    /// `+0x10`: slot/page; the dynel stores cloth at index `page * 5 + part`.
    pub page: i32,
    /// `+8` and `+0xc`, only read when `raw > 0 && (raw >> 16) as i16 > 0`.
    pub extra: Option<(i32, i32)>,
}

impl ClothData {
    /// Body part index (0 hands, 1 body, 2 feet, 3 arms, 4 legs).
    pub fn part(&self) -> i32 {
        if self.raw < 1 {
            self.raw
        } else {
            self.raw as i16 as i32
        }
    }
}

/// `AttractorMeshData_t` [GC 0x10001da1]: a mesh mounted on a CAT attractor (`CharacterMesh::AddAttractors`).
/// Wire: `u8 place`, `i32 mesh`, `i32 field`, `u8 byte`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttractorMesh {
    /// `AttractorPlace_e`; by the `Attractor<place+1>_<name>` strings in DisplaySystem: 0 head, 1 right hand,
    /// 2 left hand, 3 right shoulder, 4 left shoulder, 5 back, … (consistent with every capture, not traced
    /// to the enum itself).
    pub place: u8,
    /// rdb 1010001 mesh id (e.g. 40629 = `head_solitusfemale00.abiff`).
    pub mesh: i32,
    pub field: i32,
    /// 4 for heads, 2 for weapons, 0 for a light in the capture; meaning unresolved.
    pub byte: u8,
}

/// List entry at `+0x17c` [GC 0x10051b40]: `Identity`, three `i32` (the client derives a time stamp from two
/// of them against `GameTime_t`). Always empty in the capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectEntry {
    pub source: Identity,
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

/// Fields of a player character (flag bit 0 clear).
#[derive(Debug, Clone, PartialEq)]
pub struct PcData {
    /// `+0xa0` → stat `CurrentNano` (0xD6).
    pub current_nano: i32,
    /// `+0x108`, 0 in the capture; passed to `FUN_100586da` with the tag 0xDEA9. Meaning unresolved.
    pub field_108: i32,
    /// `i16` stats `[0x8a Swim?, 0x10 Strength, 0x11 Agility, 0x12 Stamina, 0x13 Intelligence, 0x14 Sense,
    /// 0x15 Psychic]`; the first one is named `Swim` in the client's stat table.
    pub stats: [i16; 7],
    /// Two strings when `flags2 & 0x400000` (set the dynel's two name parts, `FUN_1005b64b` slots 1/2).
    pub name_parts: Option<(String, String)>,
    /// Flag bit 26: optional `i32` (version > 0x39, stat `Clan` 0x5) and a string.
    pub extra: Option<(Option<i32>, String)>,
}

/// Fields of an NPC/monster (flag bit 0 set).
#[derive(Debug, Clone, PartialEq)]
pub struct NpcData {
    /// `+0xb2` → stat `NPCFamily` (0x1C7).
    pub npc_family: u16,
    /// `+0xb4` → stat 0x1D2 (no name in the client's table).
    pub stat_1d2: u16,
    /// `+0xb6` → stat `PetType` (0x200).
    pub pet_type: u16,
    /// `+0x10e` → stat `TowerType` (0x184).
    pub tower_type: i16,
    /// `+0x122`, only when `tower_type > 0`: selects a spawn effect (1→0xEEA4, 2→0xEEA5, 3/5/6→0xEEA3, 4→0xEEA2).
    pub tower_effect: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CharClass {
    Pc(PcData),
    Npc(NpcData),
}

/// Identity plus up to 30 waypoints (flag bit 16).
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub id: Identity,
    pub waypoints: Vec<[f32; 3]>,
}

/// `SimpleCharFullUpdateIIR_t` 0x271B3A6B, identity kind 50000 [GC 0x10078c24 read, 0x10077e13 apply,
/// 0x10077a21 validate, 0x10077a84 constructs `SimpleChar_t`]. Field order = wire order.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleCharFullUpdate {
    /// Layout version, 0x39 or 0x3a.
    pub version: u8,
    /// See [`flag`].
    pub flags: u32,
    pub playfield: Option<i32>,
    pub parent: Option<Identity>,
    /// Position `(x, y, z)`, Y up.
    pub pos: [f32; 3],
    /// Rotation quaternion `(x, y, z, w)` (about Y live).
    pub rot: Option<[f32; 4]>,
    /// Stat `Side` (0x21), 3 bits.
    pub side: u8,
    /// Stat `Fatness` (0x2F), 2 bits.
    pub fatness: u8,
    /// Stat `Breed` (4), 3 bits: 1 Solitus, 2 Opifex, 3 Nanomage, 4 Atrox.
    pub breed: u8,
    /// Stat `Sex` (0x3B), 2 bits: 1 neuter, 2 male, 3 female (create.rs `CC_BREEDS`).
    pub sex: u8,
    /// Stat `Race` (0x59), 2 bits (skin tone; 1 in every capture).
    pub race: u8,
    /// Stat 0x1A7 (no name), 5 bits, 0 in the capture.
    pub stat_1a7: u8,
    pub name: String,
    /// Stat `Flags` (0), also stored at `SimpleChar_t+0x134`.
    pub flags2: u32,
    /// Stat `Expansion` (0x185), low 16 bits of the `i32` after `flags2`.
    pub expansion: u16,
    /// Stat `AccountFlags` (0x294), high 16 bits of the same `i32`.
    pub account_flags: u16,
    pub class: CharClass,
    /// Stat `Level` (0x36).
    pub level: i16,
    /// Stat 1 (`+0x98`).
    pub max_health: i32,
    /// Stat `Health` (0x1B) (`+0x9c`, decoded from the delta form if flag bit 14).
    pub health: i32,
    /// Stat `MonsterData` (0x167), `+0xa4`.
    pub monster_data: i32,
    /// Stat `MonsterScale` (0x168, percent; the client divides by 100.0 for the body scale).
    pub monster_scale: i16,
    /// Stat `VisualFlags` (0x2A1).
    pub visual_flags: i16,
    /// `+0x32e`, stored at `SimpleChar_t+0x208` via `FUN_100572b9`. 0 (79×) or 1 (2×) in the capture.
    pub mode: u8,
    /// Raw blob (`i32 len`, bytes; 28 or 42 bytes live) handed to the object at `SimpleChar_t+0x50` (`+0xc4`).
    pub blob: Vec<u8>,
    /// Stat `HeadMesh` (0x40) = rdb 1010001 head mesh id (flag bit 7).
    pub head_mesh: Option<i32>,
    /// Stat `RunSpeed` (0x9C), `+0x10c`.
    pub run_speed: i16,
    /// Identity `+0xbc` (flag bit 10); passed to the combat routine `FUN_10069c68`. [GUESS] current target.
    pub target: Option<Identity>,
    pub textures: Vec<TextureData>,
    pub byte_120: Option<u8>,
    pub byte_121: Option<u8>,
    pub effects: Vec<EffectEntry>,
    pub path: Option<Path>,
    pub cloth: Vec<ClothData>,
    pub attractors: Vec<AttractorMesh>,
    /// Flag bit 8: 16-byte entries read in the order `[0] [1] [3] [2]`, stored here in struct order.
    pub list_190: Vec<[i32; 4]>,
    /// Stat 0x214 `ShadowBreed` (flag bit 29).
    pub shadow_breed: Option<u8>,
    pub identity_list: Vec<Identity>,
    /// Third flag word (`bit0`: pair list + `i32` + identity, `bit1`: `u8` → stat 0x29C, `bit2`: `i32` → stat 0xC4).
    pub flags3: u32,
    /// `bit0`: `(index, value)` pairs.
    pub pairs: Vec<(i32, i32)>,
    /// `bit0`: `i32` → `+0x31c`, then identity → `+0x324`.
    pub ext: Option<(i32, Identity)>,
    /// `bit1`: stat `BattlestationSide` (0x29C).
    pub battlestation_side: Option<u8>,
    /// `bit2`: stat `PetMaster` (0xC4).
    pub pet_master: Option<i32>,
    /// Last byte (`+0x32f`, copied to `n3Dynel_t+0x21d`).
    pub flag_32f: bool,
    pub rest: Vec<u8>,
}

impl SimpleCharFullUpdate {
    pub fn is_npc(&self) -> bool {
        self.flags & flag::NPC != 0
    }

    #[cfg(test)]
    fn pk_check(&self) -> (u8, u8, u8, u8, u8) {
        (self.side, self.fatness, self.breed, self.sex, self.race)
    }

    /// Heading about Y in radians, if a rotation was sent.
    pub fn yaw(&self) -> Option<f32> {
        self.rot.as_ref().map(yaw)
    }

    pub fn read(r: &mut Reader) -> Result<Self> {
        let version = r.u8()?;
        ensure!(version == 0x39 || version == 0x3a, "SimpleCharFullUpdate version {version:#x}");
        let flags = r.u32()?;
        let has = |b: u32| flags & b != 0;
        let playfield = if has(flag::PLAYFIELD) { Some(r.i32()?) } else { None };
        let parent = if has(flag::PARENT) { Some(ident(r)?) } else { None };
        let pos = vec3(r)?;
        let rot = if has(flag::ROTATION) { Some(quat(r)?) } else { None };
        ensure!(finite(&pos) && rot.as_ref().is_none_or(|q| finite(q)), "non-finite position/rotation");
        let pk = r.u32()?;
        let n = r.u8()? as usize;
        let name = String::from_utf8_lossy(r.bytes(n)?).split('\0').next().unwrap_or("").to_owned();
        let flags2 = r.u32()?;
        let pk2 = r.u32()?;
        let class = if !has(flag::NPC) {
            let current_nano = r.i32()?;
            let field_108 = r.i32()?;
            let mut stats = [0i16; 7];
            for s in &mut stats {
                *s = r.i16()?;
            }
            let name_parts = if flags2 & 0x400000 != 0 { Some((r.str_i16()?, r.str_i16()?)) } else { None };
            let extra = if has(flag::PC_EXTRA) {
                let clan = if version > 0x39 { Some(r.i32()?) } else { None };
                Some((clan, r.str_i16()?))
            } else {
                None
            };
            CharClass::Pc(PcData { current_nano, field_108, stats, name_parts, extra })
        } else {
            let npc_family = byte_or_short(r, !has(flag::NPC_B2_BYTE))? as u16;
            let stat_1d2 = byte_or_short(r, !has(flag::NPC_B4_BYTE))? as u16;
            let pet_type = byte_or_short(r, !has(flag::NPC_B6_BYTE))? as u16;
            let tower_type = r.i16()?;
            let tower_effect = if tower_type > 0 { Some(r.u8()?) } else { None };
            CharClass::Npc(NpcData { npc_family, stat_1d2, pet_type, tower_type, tower_effect })
        };
        let level = byte_or_short(r, has(flag::LEVEL_SHORT))?;
        let max_health = if has(flag::HEALTH_SHORT) { r.u16()? as i32 } else { r.i32()? };
        let health = if has(flag::HEALTH_DELTA) {
            max_health.wrapping_sub(r.u8()? as i32)
        } else if has(flag::HEALTH_SHORT) {
            r.u16()? as i32
        } else {
            r.i32()?
        };
        let monster_data = r.i32()?;
        let monster_scale = r.i16()?;
        let visual_flags = r.i16()?;
        let mode = r.u8()?;
        let blob_len = r.i32()?;
        ensure!(blob_len >= 0 && blob_len as usize <= r.remaining(), "blob of {blob_len} bytes exceeds the message");
        let blob = r.bytes(blob_len as usize)?.to_vec();
        let head_mesh = if has(flag::HEAD_MESH) { Some(r.i32()?) } else { None };
        let run_speed = byte_or_short(r, has(flag::RUN_SPEED_SHORT))?;
        let target = if has(flag::TARGET) { Some(ident(r)?) } else { None };
        let textures = if has(flag::TEXTURES) {
            let n = count(r, 0x2c)?;
            (0..n)
                .map(|_| {
                    let raw = r.bytes(0x20)?;
                    let end = raw.iter().position(|&c| c == 0).unwrap_or(0x20);
                    Ok(TextureData {
                        material: String::from_utf8_lossy(&raw[..end]).into_owned(),
                        texture: r.i32()?,
                        field_24: r.i32()?,
                        flag: r.i32()?,
                    })
                })
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let byte_120 = if has(flag::BYTE_120) { Some(r.u8()?) } else { None };
        let byte_121 = if has(flag::BYTE_121) { Some(r.u8()?) } else { None };
        let n = count(r, 20)?;
        let effects = (0..n)
            .map(|_| {
                let source = ident(r)?;
                Ok(EffectEntry { source, a: r.i32()?, b: r.i32()?, c: r.i32()? })
            })
            .collect::<Result<_>>()?;
        let path = if has(flag::PATH) {
            let id = ident(r)?;
            let k = r.i32()?;
            // the client reads `k` vectors into a 30-entry array without a bound; we refuse more.
            ensure!((0..=30).contains(&k), "waypoint count {k}");
            Some(Path { id, waypoints: (0..k).map(|_| vec3(r)).collect::<Result<_>>()? })
        } else {
            None
        };
        let n = count(r, 12)?;
        let cloth = (0..n)
            .map(|_| {
                let raw = r.i32()?;
                let (texture, page) = (r.i32()?, r.i32()?);
                let extra = if raw > 0 && ((raw >> 16) as i16) > 0 { Some((r.i32()?, r.i32()?)) } else { None };
                Ok(ClothData { raw, texture, page, extra })
            })
            .collect::<Result<_>>()?;
        let n = count(r, 10)?;
        let attractors = (0..n)
            .map(|_| {
                let place = r.u8()?;
                let (mesh, field) = (r.i32()?, r.i32()?);
                Ok(AttractorMesh { place, mesh, field, byte: r.u8()? })
            })
            .collect::<Result<_>>()?;
        let list_190 = if has(flag::LIST_190) {
            let n = count(r, 16)?;
            (0..n)
                .map(|_| {
                    let (a, b, d, c) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
                    Ok([a, b, c, d])
                })
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let shadow_breed = if has(flag::BYTE_123) { Some(r.u8()?) } else { None };
        let identity_list = if has(flag::IDENTITY_LIST) {
            let n = count(r, 8)?;
            (0..n).map(|_| ident(r)).collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let flags3 = r.u32()?;
        let (mut pairs_v, mut ext) = (Vec::new(), None);
        if flags3 & 1 != 0 {
            let k = r.i32()?;
            ensure!(k >= 0 && (k as usize) <= r.remaining() / 8, "pair count {k}");
            pairs_v = (0..k).map(|_| Ok((r.i32()?, r.i32()?))).collect::<Result<_>>()?;
            ext = Some((r.i32()?, ident(r)?));
        }
        let battlestation_side = if flags3 & 2 != 0 { Some(r.u8()?) } else { None };
        let pet_master = if flags3 & 4 != 0 { Some(r.i32()?) } else { None };
        let flag_32f = r.u8()? != 0;
        Ok(Self {
            version,
            flags,
            playfield,
            parent,
            pos,
            rot,
            side: (pk & 7) as u8,
            fatness: (pk >> 3 & 3) as u8,
            breed: (pk >> 5 & 7) as u8,
            sex: (pk >> 8 & 3) as u8,
            race: (pk >> 10 & 3) as u8,
            stat_1a7: (pk >> 12 & 0x1f) as u8,
            name,
            flags2,
            expansion: (pk2 & 0xffff) as u16,
            account_flags: (pk2 >> 16) as u16,
            class,
            level,
            max_health,
            health,
            monster_data,
            monster_scale,
            visual_flags,
            mode,
            blob,
            head_mesh,
            run_speed,
            target,
            textures,
            byte_120,
            byte_121,
            effects,
            path,
            cloth,
            attractors,
            list_190,
            shadow_breed,
            identity_list,
            flags3,
            pairs: pairs_v,
            ext,
            battlestation_side,
            pet_master,
            flag_32f,
            rest: rest(r)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::capture_n3;

    /// All captured messages with `msg_type`: `(frame sender, header, decoded)`.
    fn all(msg_type: u32) -> Vec<(u32, N3Header, Dynel)> {
        capture_n3()
            .into_iter()
            .filter_map(|f| {
                let (h, mut r) = N3Header::parse(&f.payload).ok()?;
                if h.msg_type != msg_type {
                    return None;
                }
                let d = decode(&h, &mut r).unwrap().expect("ours");
                assert_eq!(r.remaining(), 0, "{msg_type:#x} has undecoded bytes");
                Some((f.sender, h, d))
            })
            .collect()
    }

    fn chars() -> Vec<(N3Header, SimpleCharFullUpdate)> {
        all(SIMPLE_CHAR_FULL_UPDATE)
            .into_iter()
            .map(|(_, h, d)| match d {
                Dynel::SimpleCharFullUpdate(c) => (h, *c),
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn other_ids_are_not_ours() {
        let h = N3Header { msg_type: 0x5F4B1A39, target: Identity { kind: 0x9C50, instance: 1 }, flag: 0 };
        assert!(decode(&h, &mut Reader::new(&[0; 64])).unwrap().is_none());
    }

    #[test]
    fn message_keys_are_class_name_hashes() {
        fn key(s: &str) -> u32 {
            s.bytes().enumerate().fold(0, |k, (i, c)| k ^ (c as i8 as i32 as u32) << ((i & 3) * 8))
        }
        assert_eq!(key("SimpleCharFullUpdateIIR_t"), SIMPLE_CHAR_FULL_UPDATE);
        assert_eq!(key("CharDCMoveIIR_t"), CHAR_DC_MOVE);
        assert_eq!(key("StatIIR_t"), STAT);
        assert_eq!(key("SetWantedDirectionIIR_t"), SET_WANTED_DIRECTION);
        assert_eq!(key("WeaponItemFullUpdateIIR_t"), WEAPON_ITEM_FULL_UPDATE);
        assert_eq!(key("SpecialAttackWeaponIIR_t"), SPECIAL_ATTACK_WEAPON);
        assert_eq!(key("CastNanoSpellIIR_t"), CAST_NANO_SPELL);
    }

    #[test]
    fn simple_char_full_update_captured() {
        let all = chars();
        assert_eq!(all.len(), 81);
        assert!(all.iter().all(|(h, c)| h.target.kind == KIND_CHARACTER && c.playfield == Some(4582) && c.version == 0x3a));
        let pcs: Vec<_> = all.iter().filter(|(_, c)| !c.is_npc()).map(|(h, c)| (h.target.instance, c.name.as_str())).collect();
        assert_eq!(pcs, [(33402, "Bergdoktor"), (25988, "Testy"), (33491, "Stanko")]);
        // NPCs: instance ids >= 1_000_000, flag bit 0, a monster record and a Level.
        assert!(all.iter().filter(|(_, c)| c.is_npc()).all(|(h, c)| h.target.instance > 1_000_000 && c.monster_data != 0 && c.level > 0));
        assert!(all.iter().all(|(_, c)| c.rest.is_empty() && !c.name.is_empty()));
    }

    #[test]
    fn own_character_testy() {
        let (h, c) = chars().into_iter().find(|(h, _)| h.target.instance == 25988).unwrap();
        assert_eq!(h.flag, 0);
        assert_eq!((c.flags, c.flags2, c.pk_check()), (0x4ac2, 0x81241, (0, 1, 1, 3, 1)));
        assert_eq!(c.pos, [927.02765, 23.460379, 742.7181]);
        assert_eq!(c.rot, Some([0.0, -0.34683028, -0.0, 0.9379279]));
        assert!((c.yaw().unwrap() + 0.7087).abs() < 1e-3);
        assert_eq!((c.breed, c.sex, c.race, c.side, c.fatness), (1, 3, 1, 0, 1)); // solitus female
        assert_eq!((c.level, c.max_health, c.health, c.run_speed), (1, 34, 34, 6));
        assert_eq!((c.monster_scale, c.visual_flags, c.expansion, c.account_flags), (100, 31, 27, 0));
        assert_eq!(c.head_mesh, Some(40629)); // head_solitusfemale00.abiff
        assert_eq!(c.attractors, [AttractorMesh { place: 0, mesh: 40629, field: 0, byte: 4 }]);
        assert!(c.cloth.is_empty() && c.textures.is_empty() && c.effects.is_empty() && c.target.is_none());
        assert_eq!(c.blob.len(), 42);
        let CharClass::Pc(pc) = &c.class else { panic!("player") };
        assert_eq!((pc.current_nano, pc.stats), (32, [5, 6, 6, 6, 6, 6, 6]));
    }

    #[test]
    fn other_players_wear_cloth() {
        let (_, c) = chars().into_iter().find(|(h, _)| h.target.instance == 33402).unwrap();
        assert_eq!((c.breed, c.sex, c.level, c.max_health, c.head_mesh), (4, 1, 8, 273, Some(40103))); // head_athrox05
        assert_eq!(c.pos, [874.73346, 40.004997, 699.3138]);
        let got: Vec<_> = c.cloth.iter().map(|k| (k.part(), k.texture, k.page, k.extra)).collect();
        // feet combatboots, legs flakarmour, arms flakarmour, hands lowtecharmour, body salamander skin
        assert_eq!(got, [(2, 9616, 0, None), (4, 22626, 0, None), (3, 27422, 0, None), (0, 9402, 0, None), (1, 248372, 0, None)]);
        assert_eq!(c.attractors.iter().map(|a| (a.place, a.mesh)).collect::<Vec<_>>(), [(5, 26163), (0, 40103)]);
        let (_, s) = chars().into_iter().find(|(h, _)| h.target.instance == 33491).unwrap();
        assert_eq!((s.head_mesh, s.attractors.len(), s.attractors[0].mesh), (Some(223940), 3, 7796)); // dual shotguns
    }

    #[test]
    fn npcs() {
        let all = chars();
        let lizard = all.iter().find(|(_, c)| c.name == "Surf Lizard").map(|(_, c)| c).unwrap();
        assert_eq!((lizard.flags, lizard.breed, lizard.monster_data, lizard.monster_scale), (0x20a4a53, 6, 22794, 90));
        assert_eq!(lizard.textures, [TextureData { material: "lizard_green".into(), texture: 22768, field_24: 0, flag: 0 }]);
        let CharClass::Npc(n) = &lizard.class else { panic!("npc") };
        assert_eq!((n.npc_family, n.tower_type, n.tower_effect), (37, 0, None));
        let guards: Vec<_> = all.iter().filter(|(_, c)| c.name == "ICC Shuttle Guard").collect();
        assert_eq!(guards.len(), 8);
        let g = guards.iter().find(|(h, _)| h.target.instance == 1002053).map(|(_, c)| c).unwrap();
        assert_eq!((g.max_health, g.monster_data, g.head_mesh), (941, 254118, Some(40627)));
        assert_eq!(g.cloth.iter().map(|k| k.texture).collect::<Vec<_>>(), [286226, 286227, 286228, 286229, 286225]);
        // a guard that is fighting names another dynel and has less than full health
        let f = guards.iter().find(|(h, _)| h.target.instance == 1002059).map(|(_, c)| c).unwrap();
        assert_eq!((f.target, f.health), (Some(Identity { kind: 50000, instance: 1026249 }), 447));
    }

    #[test]
    fn char_dc_move_captured() {
        let all = all(CHAR_DC_MOVE);
        assert_eq!(all.len(), 144);
        let mut by_type = std::collections::BTreeMap::new();
        for (sender, h, d) in &all {
            let Dynel::CharDCMove(m) = d else { unreachable!() };
            assert_eq!((*sender, h.target.kind, h.flag, m.time, m.extra, m.type_bit7), (1, 50000, 0, 0, [0.0, 0.0], false));
            let len = m.rot.iter().map(|v| v * v).sum::<f32>();
            assert!((len - 1.0).abs() < 1e-4, "unit quaternion");
            *by_type.entry(m.move_type).or_insert(0) += 1;
        }
        assert_eq!(by_type.into_iter().collect::<Vec<_>>(), [(1, 31), (2, 1), (7, 7), (8, 1), (9, 12), (10, 6), (11, 7), (12, 4), (13, 5), (14, 3), (22, 7), (30, 60)]);
        let Dynel::CharDCMove(m) = &all[0].2 else { unreachable!() };
        assert_eq!((all[0].1.target.instance, m.move_type, m.rot, m.pos), (1025299, 30, [0.0, 0.0, 0.0, 1.0], [856.64246, 40.034237, 691.2252]));
    }

    #[test]
    fn char_dc_move_roundtrip() {
        let (f, (_, h, d)) = (capture_n3().into_iter().find(|f| f.payload[..4] == CHAR_DC_MOVE.to_be_bytes()).unwrap(), all(CHAR_DC_MOVE).remove(0));
        let Dynel::CharDCMove(m) = d else { unreachable!() };
        assert_eq!(m.encode(h.target, h.flag == 1), f.payload);
    }

    #[test]
    fn stat_and_direction() {
        let stats = all(STAT);
        assert_eq!(stats.len(), 69);
        let mut seen = Vec::new();
        for (sender, h, d) in &stats {
            let Dynel::Stat(s) = d else { unreachable!() };
            assert!(*sender == h.target.instance as u32 && h.target.kind == 50000 && s.stats.len() == 1 && s.rest.is_empty());
            seen.push(s.stats[0]);
        }
        assert_eq!(seen[0], (768, 0));
        assert!(seen.contains(&(0x40, 40629)) && seen.contains(&(27, 215)) && seen.contains(&(214, 162)));
        let dirs = all(SET_WANTED_DIRECTION);
        assert_eq!(dirs.len(), 72);
        let Dynel::SetWantedDirection(w) = &dirs[0].2 else { unreachable!() };
        assert_eq!(w.dir, [0.36750728, 0.0, 0.93002063]);
        for (_, _, d) in &dirs {
            let Dynel::SetWantedDirection(w) = d else { unreachable!() };
            assert_eq!(w.dir[1], 0.0);
            assert!((w.dir[0].powi(2) + w.dir[2].powi(2) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn cast_nano_spell_captured() {
        let casts = all(CAST_NANO_SPELL);
        assert_eq!(casts.len(), 3);
        for (sender, h, d) in &casts {
            let Dynel::CastNanoSpell(c) = d else { unreachable!() };
            assert_eq!((c.spell, c.target, c.flag), (163449, Identity { kind: 50000, instance: 33402 }, true));
            assert_eq!((c.source, *sender), (h.target, h.target.instance as u32)); // caster = NPC
        }
        let (f, (_, h, d)) = (capture_n3().into_iter().find(|f| f.payload[..4] == CAST_NANO_SPELL.to_be_bytes()).unwrap(), casts.into_iter().next().unwrap());
        let Dynel::CastNanoSpell(c) = d else { unreachable!() };
        assert_eq!(c.encode(h.target, false), f.payload);
    }

    #[test]
    fn weapons() {
        let ws = all(WEAPON_ITEM_FULL_UPDATE);
        assert_eq!(ws.len(), 10);
        let Dynel::WeaponItemFullUpdate(w) = &ws[0].2 else { unreachable!() };
        assert_eq!((ws[0].1.target.kind, ws[0].1.target.instance), (KIND_WEAPON_ITEM, 178411));
        assert_eq!((w.version, w.parent, w.placement, w.playfield, w.template), (11, Identity { kind: 50000, instance: 1002053 }, None, 4582, Identity { kind: 1000015, instance: 0 }));
        assert_eq!((w.byte_70, w.byte_71, w.blob.len()), (1, 6, 0));
        assert_eq!(w.stats, [(0, 0x403), (0x17, 0x40b82), (0x2bd, 25), (0x2be, 0x40b82), (0x2bf, 0x40b83), (0x19c, 1), (0x1a, 0), (0xd4, 25), (0x1a4, 2)]);
        // the shotgun pair of the player "Stanko" (33491 = 0x82d3)
        let Dynel::WeaponItemFullUpdate(s) = &ws[8].2 else { unreachable!() };
        assert_eq!((s.parent.instance, s.byte_71, s.stats[1]), (33491, 6, (0x17, 0x3ca19)));
    }

    #[test]
    fn special_attack_weapon() {
        let sa = all(SPECIAL_ATTACK_WEAPON);
        assert_eq!(sa.len(), 58);
        let Dynel::SpecialAttackWeapon(own) = &sa[0].2 else { unreachable!() };
        assert_eq!(sa[0].1.target.instance, 25988);
        let codes: Vec<_> = own.attacks.iter().map(|a| a.code().unwrap()).collect();
        assert_eq!(codes, ["MAAT", "DIIT", "BRAW", "NBCK"]);
        assert_eq!((own.attacks[0].f0, own.attacks[0].f1, own.attacks[0].f3), (43712, 144745, 100));
        assert_eq!(
            [own.close_combat_initiative, own.distance_weapon_initiative, own.physical_prowess_initiative, own.nano_prowess_initiative, own.agg_def],
            [6, 6, 6, 6, 100]
        );
        let mut sizes = std::collections::BTreeMap::new();
        for (_, _, d) in &sa {
            let Dynel::SpecialAttackWeapon(w) = d else { unreachable!() };
            *sizes.entry(w.attacks.len()).or_insert(0) += 1;
        }
        assert_eq!(sizes.into_iter().collect::<Vec<_>>(), [(1, 3), (3, 26), (4, 6), (5, 23)]);
    }

    #[test]
    fn malformed_input_is_an_error() {
        let (h, _) = N3Header::parse(&capture_n3().into_iter().find(|f| f.payload[..4] == SIMPLE_CHAR_FULL_UPDATE.to_be_bytes()).unwrap().payload).unwrap();
        let body = |id: u32| capture_n3().into_iter().find(|f| f.payload[..4] == id.to_be_bytes()).unwrap().payload[13..].to_vec();
        for id in [SIMPLE_CHAR_FULL_UPDATE, CHAR_DC_MOVE, STAT, SET_WANTED_DIRECTION, WEAPON_ITEM_FULL_UPDATE, SPECIAL_ATTACK_WEAPON, CAST_NANO_SPELL] {
            let b = body(id);
            let hh = N3Header { msg_type: id, ..h };
            for cut in 0..b.len() {
                assert!(decode(&hh, &mut Reader::new(&b[..cut])).is_err(), "{id:#x} truncated to {cut} decoded");
            }
            assert!(decode(&hh, &mut Reader::new(&b)).is_ok());
        }
        // bad version, bad container count, absurd stat count, non-finite position
        let mut b = body(SIMPLE_CHAR_FULL_UPDATE);
        b[0] = 0x40;
        assert!(SimpleCharFullUpdate::read(&mut Reader::new(&b)).is_err());
        assert!(StatUpdate::read(&mut Reader::new(&[0x7f, 0, 0, 0, 0, 0, 0, 0])).is_err());
        assert!(SpecialAttackWeapon::read(&mut Reader::new(&[0, 0, 0, 5, 0, 0, 0, 0])).is_err());
        let mut b = body(CHAR_DC_MOVE);
        b[1..5].copy_from_slice(&f32::NAN.to_be_bytes());
        assert!(CharDCMove::read(&mut Reader::new(&b)).is_err());
    }
}
