//! Zone N3 messages that set up the world around the player: playfield, own character, game
//! time, org info, quests, appearance, character actions, vending machines and corpses.
//! Layouts, RE addresses and live evidence: docs/zone/world.md.
//!
//! Every message is an `n3InfoItemRemote_t` subclass: the wire message type is
//! `n3InfoItemRemote_t::MapToKey(class name)` (N3.dll 0x10009826, see [`super::misc::key`]) and
//! the body is the class's `ReadSubClass` (Gamecode.dll vtable slot 7).

use super::dynel::{ClothData as WornCloth, TextureData};
use super::{spells, N3Header};
use crate::msg::Identity;
use crate::wire::Reader;
use anyhow::{bail, Result};

pub const PLAYFIELD_ANARCHY_F: u32 = 0x5F4B_1A39; // PlayfieldAnarchyFIIR_t
pub const FULL_CHARACTER: u32 = 0x2930_4349; // FullCharacterIIR_t
pub const VENDING_MACHINE_FULL_UPDATE: u32 = 0x7F54_4905; // VendingMachineFullUpdateIIR_t
pub const GAME_TIME: u32 = 0x5F52_412E; // GameTimeIIR_t
pub const ORG_INFO_PACKET: u32 = 0x2E2A_4A6B; // OrgInfoPacketIIR_t
pub const QUEST_FULL_UPDATE: u32 = 0x465A_4061; // QuestFullUpdateIIR_t
pub const APPEARANCE_UPDATE: u32 = 0x4162_4F0D; // AppearanceUpdateIIR_c
pub const CHARACTER_ACTION: u32 = 0x5E47_7770; // CharacterActionIIR_t
pub const CORPSE_FULL_UPDATE: u32 = 0x4F47_4E05; // CorpseFullUpdateIIR_t
pub const DOOR_FULL_UPDATE: u32 = 0x365A_5071; // DoorFullUpdateIIR_t
pub const DOOR_STATUS_UPDATE: u32 = 0x4C7D_403B; // DoorStatusUpdateIIR_t

/// Container size words are `(count + 1) * 0x3F1` (`x / 0x3f1 - 1` everywhere in Gamecode.dll).
const UNIT: u32 = 0x3F1;
/// "No value" sentinel the client skips when applying a stat (`FUN_10073a2f`: `!= 0x499602d2`).
pub const STAT_UNSET: i32 = 0x4996_02D2;

#[derive(Debug, Clone, PartialEq)]
pub enum World {
    Playfield(PlayfieldAnarchyF),
    FullCharacter(Box<FullCharacter>),
    VendingMachine(VendingMachine),
    GameTime(GameTime),
    OrgInfo(OrgInfo),
    Quests(QuestFullUpdate),
    Appearance(AppearanceUpdate),
    CharacterAction(CharacterAction),
    Corpse(Corpse),
    Door(Door),
    DoorStatus(DoorStatus),
}

/// Decode one of this module's messages; `Ok(None)` for any other message type.
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<World>> {
    Ok(Some(match h.msg_type {
        PLAYFIELD_ANARCHY_F => World::Playfield(PlayfieldAnarchyF::read(h, r)?),
        FULL_CHARACTER => World::FullCharacter(Box::new(FullCharacter::read(r)?)),
        VENDING_MACHINE_FULL_UPDATE => World::VendingMachine(VendingMachine { base: DynelBase::read(r)? }),
        GAME_TIME => World::GameTime(GameTime::read(r)?),
        ORG_INFO_PACKET => World::OrgInfo(OrgInfo { org_id: r.i32()?, name: r.str_i16()? }),
        QUEST_FULL_UPDATE => World::Quests(QuestFullUpdate::read(r)?),
        APPEARANCE_UPDATE => World::Appearance(AppearanceUpdate::read(r)?),
        CHARACTER_ACTION => World::CharacterAction(CharacterAction::read(r)?),
        CORPSE_FULL_UPDATE => World::Corpse(Corpse::read(r)?),
        DOOR_FULL_UPDATE => World::Door(Door::read(r)?),
        DOOR_STATUS_UPDATE => World::DoorStatus(DoorStatus::read(r)?),
        _ => return Ok(None),
    }))
}

// ---------------------------------------------------------------- helpers

fn take_rest(r: &mut Reader) -> Result<Vec<u8>> {
    Ok(r.bytes(r.remaining())?.to_vec())
}

/// Unsupported tail that starts with an already consumed container size word.
fn rest_after(word: u32, r: &mut Reader) -> Result<Vec<u8>> {
    let mut v = word.to_be_bytes().to_vec();
    v.extend(take_rest(r)?);
    Ok(v)
}

/// Read a `(count + 1) * 0x3F1` size word; returns the word and the count.
pub(super) fn counted(r: &mut Reader) -> Result<(u32, usize)> {
    let w = r.u32()?;
    if w == 0 || w % UNIT != 0 {
        bail!("container size word {w:#x} is not (n+1)*0x3f1");
    }
    Ok((w, (w / UNIT - 1) as usize))
}

/// A count read from the wire must be satisfiable by the bytes that are left.
pub(super) fn fits(n: usize, elem: usize, r: &Reader) -> Result<()> {
    if n.checked_mul(elem).is_none_or(|b| b > r.remaining()) {
        bail!("{n} elements of {elem} bytes do not fit in {} remaining bytes", r.remaining());
    }
    Ok(())
}

fn id_pairs(r: &mut Reader) -> Result<Vec<(u32, i32)>> {
    let (_, n) = counted(r)?;
    fits(n, 8, r)?;
    (0..n).map(|_| Ok((r.u32()?, r.i32()?))).collect()
}

pub(super) fn identities(r: &mut Reader) -> Result<Vec<Identity>> {
    let (_, n) = counted(r)?;
    fits(n, 8, r)?;
    (0..n).map(|_| Identity::read(r)).collect()
}

// ---------------------------------------------------------------- PlayfieldAnarchyF

/// `PlayfieldProxy_t` wire form (MessageProtocol.dll 0x1000322e; Gamecode FUN_10038402).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayfieldProxy {
    /// `{0xC79C, playfield}`: kind selects the factory (`n3EngineClientAnarchy_t::GetPlayfieldFactory`,
    /// GC 0x10018804): 0xC79C = normal, 0xC79D = building, 0xC79E, 0xC79F/0xC7A1.
    pub playfield: Identity,
    pub attribute: i32,
    pub exit_door: i32,
    /// `{0x9C50, playfield}`: its `instance` is the RDB key of the playfield record.
    pub exit_door_id: Identity,
}

/// The `DbObject_t` of kind 0xC77D (`TemplatePlayfieldGeneratorData_t`, table `BUILDING_DATA`)
/// the message carries from version 4 on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayfieldGenerator {
    /// `AoDbObject_t::ReadBlob` (DatabaseController 0x10004ad0): three i32.
    pub identity: Identity,
    /// Third ReadBlob word (always 1 live). [GUESS] db revision (`GetOwnedBuildingDbRevision`).
    pub revision: i32,
    /// Must be 1 (`FUN_10124871`).
    pub marker: i32,
    /// `i32 n`, then per entry `{i32 a; i32 m; m * (i32, i32, i32)}` (`FUN_1013e228/e128`).
    pub buildings: Vec<(i32, Vec<[i32; 3]>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayfieldAnarchyF {
    /// Playfield instance = header identity `{0x9C50, id}`; key of `n3Playfield_t::GetPlayfield(uint)`.
    pub playfield_id: i32,
    /// Wire version (4 live). >1 adds the proxy, >3 the generator `DbObject`.
    pub version: i32,
    /// Camera/cell-monitor start position, exactly as on the wire (Y is height).
    pub position: [f32; 3],
    pub proxy: Option<PlayfieldProxy>,
    /// `None` when the DbObject identity on the wire is `{0,0}`.
    pub generator: Option<PlayfieldGenerator>,
    /// `PlayfieldAnarchy_t::PFWorldXPos/ZPos` (+0xAC/+0xB0), read after the sub-class body.
    pub world_x: i32,
    pub world_z: i32,
    /// Bytes after an unsupported `DbObject` kind (then `world_x/z` are 0).
    pub rest: Vec<u8>,
}

impl PlayfieldAnarchyF {
    fn read(h: &N3Header, r: &mut Reader) -> Result<Self> {
        let version = r.i32()?;
        let position = [r.f32()?, r.f32()?, r.f32()?];
        let mut p = Self {
            playfield_id: h.target.instance,
            version,
            position,
            proxy: None,
            generator: None,
            world_x: 0,
            world_z: 0,
            rest: vec![],
        };
        if version > 1 {
            let tag = r.u8()?;
            if tag != b'a' {
                bail!("Invalid playfieldproxy version {tag:#x}");
            }
            p.proxy = Some(PlayfieldProxy {
                playfield: Identity::read(r)?,
                attribute: r.i32()?,
                exit_door: r.i32()?,
                exit_door_id: Identity::read(r)?,
            });
        }
        if version > 3 {
            let id = Identity::read(r)?;
            if id.kind != 0 || id.instance != 0 {
                if id.kind != 0xC77D {
                    // DbObject_t::CreateObject would build another class; layout unknown here.
                    let mut v = Vec::new();
                    v.extend(id.kind.to_be_bytes());
                    v.extend(id.instance.to_be_bytes());
                    v.extend(take_rest(r)?);
                    p.rest = v;
                    return Ok(p);
                }
                let revision = r.i32()?;
                let marker = r.i32()?;
                if marker != 1 {
                    bail!("PlayfieldGenerator marker {marker}, expected 1");
                }
                let n = r.i32()?;
                if n < 0 {
                    bail!("negative building count {n}");
                }
                fits(n as usize, 8, r)?;
                let mut buildings = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let a = r.i32()?;
                    let m = r.i32()?;
                    if m < 0 {
                        bail!("negative building entry count {m}");
                    }
                    fits(m as usize, 12, r)?;
                    let es = (0..m).map(|_| Ok([r.i32()?, r.i32()?, r.i32()?])).collect::<Result<_>>()?;
                    buildings.push((a, es));
                }
                p.generator = Some(PlayfieldGenerator { identity: id, revision, marker, buildings });
            }
        }
        p.world_x = r.i32()?;
        p.world_z = r.i32()?;
        Ok(p)
    }

    /// RDB key of the playfield record (`RDBPlayfieldAnarchy_t`, type 0xF4241): `{1_000_001, id}`
    /// with `id` = `proxy.exit_door_id.instance` (GC `FUN_1011ee6d`).
    pub fn rdb_playfield(&self) -> Option<Identity> {
        self.proxy.map(|p| Identity { kind: 0xF4241, instance: p.exit_door_id.instance })
    }
}

// ---------------------------------------------------------------- FullCharacter

/// `{u32 ignored; Identity; i32; i32}` entry of the three groups (`FUN_1003bb2d`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupEntry {
    pub ignored: u32,
    pub id: Identity,
    pub a: i32,
    pub b: i32,
}

/// `FUN_1003bbad`: `u32 header; u32 count; count * GroupEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntryGroup {
    pub header: u32,
    pub entries: Vec<GroupEntry>,
}

/// `GameData::ACGItem_t` as read by `operator>>` [GameData.dll 0x1000e9d7]: `i32 low_id; i32 high_id; i32 level; i32` (the fourth
/// word is read into a local and dropped). `high_id == 0` is replaced by `low_id`; `level > 0x1ff` is masked with `0x1ff`.
/// `low_id` / `high_id` are `StaticInstance` keys of rdb 1000020 item records (`GetTemplate(0/1)`), `level` is the quality level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AcgItem {
    pub low_id: i32,
    pub high_id: i32,
    pub level: i32,
}

/// One inventory element of `FullCharacterIIR_t` (`FUN_1002a41a` [GC]): `u32 slot` (index into the character's inventory vector:
/// 0..0x3f equipment pages, 0x40.. bag), then `FUN_1002a04a`: `i16; i16; Identity; ACGItem_t` (`a`/`b` are the first two members of
/// the 0x28-byte slot object, meaning UNRESOLVED).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryEntry {
    pub slot: u32,
    pub a: i16,
    pub b: i16,
    pub id: Identity,
    pub item: AcgItem,
}

impl InventoryEntry {
    fn read(r: &mut Reader) -> Result<Self> {
        let slot = r.u32()?;
        let (a, b, id) = (r.i16()?, r.i16()?, Identity::read(r)?);
        let (low_id, high_id, mut level) = (r.i32()?, r.i32()?, r.i32()?);
        r.i32()?;
        if level > 0x1ff {
            level &= 0x1ff;
        }
        Ok(Self { slot, a, b, id, item: AcgItem { low_id, high_id: if high_id == 0 { low_id } else { high_id }, level } })
    }
}

/// `FUN_1002a41a` [GC]: size word `(n+1)*0x3f1`, then `n` [`InventoryEntry`]s (32 bytes each).
pub(crate) fn read_inventory(r: &mut Reader) -> Result<Vec<InventoryEntry>> {
    let (_, n) = counted(r)?;
    fits(n, 32, r)?;
    (0..n).map(|_| InventoryEntry::read(r)).collect()
}

/// `FullCharacterIIR_t::ReadSubClass` (GC 0x10073881). Reading stops (and `rest` holds the
/// unread bytes, starting with the size word/flag that could not be decoded) at the first
/// non-empty equipment block, perk map or a spell list holding a spell kind that `spells::read_spell` cannot read: those layouts (perk
/// entries, team blocks) are not decoded yet; every capture has them empty. The inventory
/// (`InventoryEntry`) is decoded from the client code only: every capture has it empty
/// (unit test on synthetic bytes).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FullCharacter {
    /// 26 (= `DAT_101c01ec`).
    pub version: u32,
    /// `this+0x18`: `i32` list (`FUN_100742d5`).
    pub list_18: Vec<i32>,
    /// `this+0x24`: 3-byte records (`FUN_1002cfe5`).
    pub triples_24: Vec<[u8; 3]>,
    /// `this+0x28/0x2C/0x30`.
    pub groups: [EntryGroup; 3],
    /// `this+0x34` / `this+0x38`: `(stat id, i32 value)`; applied in this order.
    pub stats_a: Vec<(u32, i32)>,
    pub stats_b: Vec<(u32, i32)>,
    /// `this+0x3C`: `(u8 stat id, u8 value)`.
    pub stats_u8: Vec<(u8, u8)>,
    /// `this+0x40`: `(u8 stat id, i16 value)`.
    pub stats_i16: Vec<(u8, i16)>,
    /// `this+0x44`: plain `i32` count then `(i32 id, i32 value)` (empty live).
    pub stat_map: Vec<(i32, i32)>,
    /// bit0: equipment block follows; bit1: 6 blocks + `i32 +0x68` instead of one.
    pub equipment_flags: u32,
    /// `this+0x6C`: identity list (empty live).
    pub list_6c: Vec<Identity>,
    /// `this+0x70`: inventory elements in wire order.
    pub inventory: Vec<InventoryEntry>,
    /// `this+0x78`: the active spells (empty in every capture).
    pub spells: Vec<spells::Spell>,
    pub rest: Vec<u8>,
}

impl FullCharacter {
    pub fn stat(&self, id: u32) -> Option<i32> {
        let wide = self.stats_a.iter().chain(&self.stats_b).find(|s| s.0 == id).map(|s| s.1);
        wide.or_else(|| self.stats_u8.iter().find(|s| s.0 as u32 == id).map(|s| s.1 as i32))
            .or_else(|| self.stats_i16.iter().find(|s| s.0 as u32 == id).map(|s| s.1 as i32))
    }

    fn read(r: &mut Reader) -> Result<Self> {
        let mut c = Self { version: r.u32()?, ..Self::default() };
        if c.version != 26 {
            bail!("FullCharacter version {} (client expects 26)", c.version);
        }
        c.body(r)?;
        Ok(c)
    }

    fn body(&mut self, r: &mut Reader) -> Result<()> {
        // this+0x70: `FUN_1002a41a`: size word (n+1)*0x3f1, per element `u32 slot; i16; i16; Identity; ACGItem_t` (`FUN_1002a04a`).
        self.inventory = read_inventory(r)?;
        let (_, n) = counted(r)?;
        fits(n, 4, r)?;
        self.list_18 = (0..n).map(|_| r.i32()).collect::<Result<_>>()?;
        let (_, n) = counted(r)?;
        fits(n, 3, r)?;
        self.triples_24 = (0..n).map(|_| Ok([r.u8()?, r.u8()?, r.u8()?])).collect::<Result<_>>()?;
        for g in &mut self.groups {
            g.header = r.u32()?;
            let n = r.u32()? as usize;
            fits(n, 20, r)?;
            g.entries = (0..n)
                .map(|_| {
                    Ok(GroupEntry {
                        ignored: r.u32()?,
                        id: Identity::read(r)?,
                        a: r.i32()?,
                        b: r.i32()?,
                    })
                })
                .collect::<Result<_>>()?;
        }
        self.stats_a = id_pairs(r)?;
        self.stats_b = id_pairs(r)?;
        let (_, n) = counted(r)?;
        fits(n, 2, r)?;
        self.stats_u8 = (0..n).map(|_| Ok((r.u8()?, r.u8()?))).collect::<Result<_>>()?;
        let (_, n) = counted(r)?;
        fits(n, 3, r)?;
        self.stats_i16 = (0..n).map(|_| Ok((r.u8()?, r.i16()?))).collect::<Result<_>>()?;
        let n = r.i32()?;
        if n < 0 {
            bail!("negative stat map size {n}");
        }
        fits(n as usize, 8, r)?;
        self.stat_map = (0..n).map(|_| Ok((r.i32()?, r.i32()?))).collect::<Result<_>>()?;
        self.equipment_flags = r.u32()?;
        if self.equipment_flags & 1 != 0 {
            // Identity (+0x60), then team-data blocks (FUN_10125c59) with `ACGItem`-like entries.
            self.rest = rest_after(self.equipment_flags, r)?;
            return Ok(());
        }
        self.list_6c = identities(r)?;
        // this+0x78 spell list (`FUN_100a6c58`: `GameData::SpellData_t` records, decoded by `super::spells::read_spell`). `FUN_10073a2f` runs it on the
        // character after the stats (`vtable +0x30`, `FUN_100026d0`): the active effects whose bonuses `GetSkill(stat, 2)` adds. A list this decoder cannot
        // read (a spell of the not decoded kinds) stays in `rest` like before.
        let (w, n) = counted(r)?;
        if n != 0 {
            let mut probe = r.clone();
            match (0..n).map(|_| spells::read_spell(&mut probe)).collect::<Result<Vec<_>>>() {
                Ok(v) => {
                    self.spells = v;
                    *r = probe;
                }
                Err(_) => {
                    self.rest = rest_after(w, r)?;
                    return Ok(());
                }
            }
        }
        // this+0x90 perk map
        let (w, n) = counted(r)?;
        if n != 0 {
            self.rest = rest_after(w, r)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- dynel base (vending machine, corpse)

/// Common start of the `*FullUpdate` messages: the `FUN_1009f340` -> `FUN_100a0730` -> `FUN_100a110a`
/// chain (versions 3 / 2 / 11, `DAT_101c0f80`, `DAT_101c1084`, `DAT_101c1160`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DynelBase {
    pub version: u32,
    /// `this+0x24`; position and rotation are present only when its kind is 0.
    pub parent: Identity,
    pub position: Option<[f32; 3]>,
    /// Quaternion `(x, y, z, w)` as on the wire.
    pub rotation: Option<[f32; 4]>,
    /// `this+0x48`: playfield instance (4582 live).
    pub playfield: i32,
    /// `this+0x4C`.
    pub template: Identity,
    /// `this+0x70/0x71`.
    pub flags: [u8; 2],
    /// `this+0x5C`: `(stat id, value)` pairs applied to the dynel's stat table.
    pub stats: Vec<(u32, i32)>,
    /// `this+0x6C`: `i32 n` + n bytes; the corpse name, NUL terminated.
    pub blob: Vec<u8>,
    pub version2: u32,
    /// Stored at `(*(this+0x60))+0x4AC` (50 live).
    pub x4ac: i32,
    /// `this+0x74` identity list (empty live).
    pub list_74: Vec<Identity>,
    pub version3: u32,
}

impl DynelBase {
    fn read(r: &mut Reader) -> Result<Self> {
        Self::read_with(r, 3)
    }

    /// The chain up to `version3`, which the subclass fixes (`DAT_101c0f80` = 3 for the vending machine and the corpse,
    /// `DAT_101c0ff8` = 2 for the door).
    fn read_with(r: &mut Reader, version3: u32) -> Result<Self> {
        let mut b = Self { version: r.u32()?, ..Self::default() };
        if b.version != 11 {
            bail!("dynel base version {} (client expects 11)", b.version);
        }
        b.parent = Identity::read(r)?;
        if b.parent.kind == 0 {
            b.position = Some([r.f32()?, r.f32()?, r.f32()?]);
            b.rotation = Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?]);
        }
        b.playfield = r.i32()?;
        b.template = Identity::read(r)?;
        b.flags = [r.u8()?, r.u8()?];
        b.stats = id_pairs(r)?;
        let n = r.i32()?;
        if n > 0 {
            b.blob = r.bytes(n as usize)?.to_vec();
        }
        b.version2 = r.u32()?;
        if b.version2 != 2 {
            bail!("dynel base version2 {} (client expects 2)", b.version2);
        }
        b.x4ac = r.i32()?;
        b.list_74 = identities(r)?;
        b.version3 = r.u32()?;
        if b.version3 != version3 {
            bail!("dynel base version3 {} (client expects {version3})", b.version3);
        }
        Ok(b)
    }

    /// The blob as text (corpse name), NUL trimmed.
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.blob).trim_end_matches('\0').to_owned()
    }
}

/// `VendingMachineFullUpdateIIR_t`: only the base (GC slot 7 = `FUN_100a2608` -> `FUN_1009f340`).
/// The client creates the dynel only when the header identity kind is 0xC75B.
#[derive(Debug, Clone, PartialEq)]
pub struct VendingMachine {
    pub base: DynelBase,
}

/// `DoorFullUpdateIIR_t` [GC vtable 0x10166aac, slot 7 = `FUN_1009fa37`]: the dynel base chain (`FUN_100a0730`, `FUN_100a110a`) with
/// `version3` = 2, then one `i32` stored at `this+0x78` (`FUN_1009fa81` writes it back). The client creates the dynel from it
/// (`FUN_1009faaf`, slot 2) only when the header identity kind is a door's.
#[derive(Debug, Clone, PartialEq)]
pub struct Door {
    pub base: DynelBase,
    /// `this+0x78`. **[UNRESOLVED]** meaning (nothing in the apply path reads it).
    pub value_78: i32,
}

impl Door {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self { base: DynelBase::read_with(r, 2)?, value_78: r.i32()? })
    }
}

/// `DoorStatusUpdateIIR_t` [GC vtable 0x10166ae0; slot 7 = `FUN_1009fde6` reads, slot 8 = `FUN_1009fcb8` writes, slot 2 = the apply
/// `FUN_1009fc10`]: `u32 version` (= 2, `DAT_101c1020`), three `u8` (`== 1` is true), `i32`, one more `u8`, then a stat list
/// (`FUN_1002d8bd`: `(n + 1) * 0x3F1` size word, `n` pairs) that the apply path does not read. The header identity is the door.
#[derive(Debug, Clone, PartialEq)]
pub struct DoorStatus {
    /// `this+0x18`: lock the door (`Flags` bit 0x40; a locked open door closes first).
    pub locked: bool,
    /// `this+0x19`: open (true) or close (false) the door.
    pub open: bool,
    /// `this+0x1c`: stored with `SetStat(0xC3, value)` (stat 195, unnamed in the client table).
    pub value_c3: i32,
    /// `this+0x1a`: sets the door's byte `+0x1d5` (`FUN_1007ed64` sets the same byte and calls vtable `+0xac`).
    pub flag_1a: bool,
    /// `this+0x20`.
    pub stats: Vec<(u32, i32)>,
}

impl DoorStatus {
    fn read(r: &mut Reader) -> Result<Self> {
        let version = r.u32()?;
        if version != 2 {
            bail!("DoorStatusUpdate version {version} (client expects 2)");
        }
        let locked = r.u8()? == 1;
        let open = r.u8()? == 1;
        let value_c3 = r.i32()?;
        let flag_1a = r.u8()? == 1;
        Ok(Self { locked, open, value_c3, flag_1a, stats: id_pairs(r)? })
    }
}

/// One `GameData::SpellData_t` of a corpse (`SpellFormats_c::ReadBinary` [GD 0x1000f4a6]): header
/// `{i32 type, i32 id, i32 format version}`, `i32` criteria count (`SpellData_t::ReadBinaryCriteria`
/// [GD 0x1000d49f]), then the type's fixed arguments (`SpellFormat_c::ReadBinary` [GD 0x1000f39e],
/// one `i32` per `Add` of the format built in `SpellFormats_c::SpellFormats_c` [GD 0x1000fb0a]).
/// Only the live type 0xCF27 is decoded: its format has 7 `i32` arguments (stats 5, 6, 7, 0x2d, 0x2f,
/// 0x30, 0xb); the 4 `i32` that follow them in every capture are kept in `tail` ([UNRESOLVED]: no reader
/// for them was found in `SpellData_t`'s `>>`; docs/zone/static.md §5). Other types are an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpseSpell {
    pub type_id: u32,
    pub id: i32,
    pub version: i32,
    pub args: [i32; 7],
    pub tail: [i32; 4],
}

/// `CorpseFullUpdateIIR_t` (GC `FUN_1009f502`, ReadSubClass slot 7): version 8, base, a `SpellData_t`
/// list (`FUN_100a6c58`), the owner identity (`+0x84`, `FUN_1013cda9`), a `ClothData_t` vector (`+0x8c`)
/// and `i32 n` + (if `n != 0`) a `TextureData_t` vector (`+0x90`). The corpse is drawn from
/// `base.stats` (`CATMesh` 42 ...) plus `cloth` and `textures`, see `ao_formats::dynel_visual`.
#[derive(Debug, Clone, PartialEq)]
pub struct Corpse {
    pub version: u32,
    pub base: DynelBase,
    pub spells: Vec<CorpseSpell>,
    /// The dead character `{0xC350, id}` (equals stats `CorpseType` 415 / `CorpseInstance` 416).
    pub owner: Identity,
    pub cloth: Vec<WornCloth>,
    pub textures: Vec<TextureData>,
}

impl Corpse {
    fn read(r: &mut Reader) -> Result<Self> {
        let version = r.u32()?;
        if version != 8 {
            bail!("corpse version {version} (client expects 8)");
        }
        let base = DynelBase::read(r)?;
        let (_, n) = counted(r)?;
        if n >= 1000 {
            bail!("corpse spell count {n}");
        }
        let mut spells = Vec::new();
        for _ in 0..n {
            let (type_id, id, version) = (r.u32()?, r.i32()?, r.i32()?);
            if type_id != 0xCF27 || version != 4 {
                bail!("corpse spell type {type_id:#x} version {version}: only 0xCF27 v4 is decoded");
            }
            let criteria = r.i32()?;
            if criteria != 0 {
                bail!("corpse spell with {criteria} criteria is not decoded");
            }
            let mut args = [0; 7];
            for a in &mut args {
                *a = r.i32()?;
            }
            let tail = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
            spells.push(CorpseSpell { type_id, id, version, args, tail });
        }
        let owner = Identity::read(r)?;
        let (_, n) = counted(r)?;
        fits(n, 12, r)?;
        let cloth = (0..n)
            .map(|_| {
                let raw = r.i32()?;
                let (texture, page) = (r.i32()?, r.i32()?);
                let extra = if raw > 0 && ((raw >> 16) as i16) > 0 { Some((r.i32()?, r.i32()?)) } else { None };
                Ok(WornCloth { raw, texture, page, extra })
            })
            .collect::<Result<_>>()?;
        let n = r.i32()?;
        let mut textures = Vec::new();
        if n != 0 {
            let (_, n) = counted(r)?;
            fits(n, 0x2c, r)?;
            for _ in 0..n {
                let raw = r.bytes(0x20)?;
                let end = raw.iter().position(|&c| c == 0).unwrap_or(0x20);
                textures.push(TextureData {
                    material: String::from_utf8_lossy(&raw[..end]).into_owned(),
                    texture: r.i32()?,
                    field_24: r.i32()?,
                    flag: r.i32()?,
                });
            }
        }
        Ok(Self { version, base, spells, owner, cloth, textures })
    }
}

// ---------------------------------------------------------------- small messages

/// `GameTimeIIR_t` (GC 0x10039564 read, 0x100395ea -> `GameTime_t::Update(float, DayPeriod_e, int, int)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameTime {
    /// `this+0x18`, seconds of game time (`GameTime_t::Update` arg 1; split into the `+0x40/+0x44/+0x48` fields).
    pub time: f32,
    /// `this+0x1C` `DayPeriod_e` (arg 2; `Update` ignores it and recomputes the period).
    pub day_period: i32,
    /// `this+0x20` -> `GameTime_t+0x4C`: the game day number (incremented by the 27 h day carry in `RunFunction` [GC 0x1000b214], docs/zone/world.md §10.1).
    pub arg3: i32,
    /// `this+0x24`: server unix time (secs); `GameTime_t+0xC0`, and `+0xB8 = arg4 - local _time64()`.
    pub arg4: i32,
}

impl GameTime {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self { time: r.f32()?, day_period: r.i32()?, arg3: r.i32()?, arg4: r.i32()? })
    }
}

/// `OrgInfoPacketIIR_t` (GC 0x10075faa read, 0x10075f20 activate): stat 5 of the dynel is set
/// to `org_id`; the name goes to the org-name registry (`FUN_1003f4fb`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgInfo {
    pub org_id: i32,
    pub name: String,
}

/// `QuestFullUpdateIIR_t` (GC 0x100aca12): `QuestList` (`FUN_100abd64`: size word, per quest an
/// `Identity` + body `FUN_100ab951`) then a `u8` flag. The quests are decoded by [`super::quest`] (layout from the disassembly, no capture has a
/// mission); when that fails the list stays undecoded: everything after the size word is in `rest` (`flag` is then 0, `quests` empty).
#[derive(Debug, Clone, PartialEq)]
pub struct QuestFullUpdate {
    pub quest_count: usize,
    pub quests: Vec<super::quest::Quest>,
    pub flag: u8,
    pub rest: Vec<u8>,
}

impl QuestFullUpdate {
    fn read(r: &mut Reader) -> Result<Self> {
        let (w, n) = counted(r)?;
        if n != 0 {
            let saved = r.clone();
            match super::quest::quest_list(r, n).and_then(|q| Ok((q, r.u8()?))) {
                Ok((quests, flag)) => return Ok(Self { quest_count: n, quests, flag, rest: vec![] }),
                Err(_) => *r = saved,
            }
            return Ok(Self { quest_count: n, quests: vec![], flag: 0, rest: rest_after(w, r)? });
        }
        Ok(Self { quest_count: 0, quests: vec![], flag: r.u8()?, rest: vec![] })
    }
}

/// `ClothData_t` (GameData.dll 0x1000a661): `i32 packed; i32 b; i32 c;` and, when the packed
/// word is positive with a positive high half, `i32 d; i32 e`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClothData {
    /// Low 16 bits (sign-extended) of the packed word when it is positive, else the whole word.
    pub id: i32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
    pub e: i32,
    /// Raw packed word.
    pub packed: i32,
}

/// `AttractorMeshData_t` (GC `FUN_10001da1`): `u8 a; i32 b; i32 c; u8 d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attractor {
    pub a: u8,
    pub b: i32,
    pub c: i32,
    pub d: u8,
}

fn read_cloth(r: &mut Reader) -> Result<Vec<ClothData>> {
    let (_, n) = counted(r)?;
    fits(n, 12, r)?;
    (0..n)
        .map(|_| {
            let packed = r.i32()?;
            let b = r.i32()?;
            let c = r.i32()?;
            let mut cd = ClothData { packed, b, c, ..Default::default() };
            if packed < 1 {
                cd.id = packed;
            } else {
                cd.id = packed as i16 as i32;
                if ((packed >> 16) as i16) > 0 {
                    cd.d = r.i32()?;
                    cd.e = r.i32()?;
                }
            }
            Ok(cd)
        })
        .collect()
}

/// `AppearanceUpdateIIR_c` (GC 0x100718dc read, 0x10071679 activate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceUpdate {
    pub cloth: Vec<ClothData>,
    pub attractors: Vec<Attractor>,
    /// Becomes stat 0x2A1 (673 `VisualFlags`) of the dynel.
    pub visual_flags: i16,
    /// Passed to `FUN_100572b9(u8, 0)`. [UNRESOLVED]
    pub extra: u8,
}

impl AppearanceUpdate {
    fn read(r: &mut Reader) -> Result<Self> {
        let cloth = read_cloth(r)?;
        let (_, n) = counted(r)?;
        fits(n, 10, r)?;
        let attractors = (0..n)
            .map(|_| Ok(Attractor { a: r.u8()?, b: r.i32()?, c: r.i32()?, d: r.u8()? }))
            .collect::<Result<_>>()?;
        Ok(Self { cloth, attractors, visual_flags: r.i16()?, extra: r.u8()? })
    }
}

/// `CharacterActionIIR_t` (GC 0x100724e8 read, 0x10072410 -> `FUN_1005d0d8`); acts on a SimpleChar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterAction {
    /// `this+0x18`, read first.
    pub action: i32,
    /// `this+0x1C`.
    pub param: i32,
    pub identity_a: Identity,
    pub identity_b: Identity,
    pub text: String,
}

impl CharacterAction {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            action: r.i32()?,
            param: r.i32()?,
            identity_a: Identity::read(r)?,
            identity_b: Identity::read(r)?,
            text: r.str_i16()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::{capture_n3, misc::key};

    /// Every captured message of `msg_type` as `(frame sender, header, decoded)`.
    fn all(msg_type: u32) -> Vec<(u32, N3Header, World)> {
        capture_n3()
            .into_iter()
            .filter_map(|f| {
                let (h, mut r) = N3Header::parse(&f.payload).unwrap();
                if h.msg_type != msg_type {
                    return None;
                }
                let w = decode(&h, &mut r).unwrap().unwrap();
                assert_eq!(r.remaining(), 0, "{msg_type:#x} left bytes");
                Some((f.sender, h, w))
            })
            .collect()
    }

    #[test]
    fn ids_are_the_class_name_hash() {
        assert_eq!(key("PlayfieldAnarchyFIIR_t"), PLAYFIELD_ANARCHY_F);
        assert_eq!(key("FullCharacterIIR_t"), FULL_CHARACTER);
        assert_eq!(key("VendingMachineFullUpdateIIR_t"), VENDING_MACHINE_FULL_UPDATE);
        assert_eq!(key("GameTimeIIR_t"), GAME_TIME);
        assert_eq!(key("OrgInfoPacketIIR_t"), ORG_INFO_PACKET);
        assert_eq!(key("QuestFullUpdateIIR_t"), QUEST_FULL_UPDATE);
        assert_eq!(key("AppearanceUpdateIIR_c"), APPEARANCE_UPDATE);
        assert_eq!(key("CharacterActionIIR_t"), CHARACTER_ACTION);
        assert_eq!(key("CorpseFullUpdateIIR_t"), CORPSE_FULL_UPDATE);
        assert_eq!(key("DoorFullUpdateIIR_t"), DOOR_FULL_UPDATE);
        assert_eq!(key("DoorStatusUpdateIIR_t"), DOOR_STATUS_UPDATE);
    }

    #[test]
    fn playfield() {
        let v = all(PLAYFIELD_ANARCHY_F);
        assert_eq!(v.len(), 1);
        let World::Playfield(p) = &v[0].2 else { panic!() };
        assert_eq!((p.playfield_id, p.version), (4582, 4));
        assert_eq!(p.position.map(f32::to_bits), [0x4467_C1C5, 0x41BB_AEDB, 0x4439_ADF5]);
        assert!((p.position[0] - 927.03).abs() < 0.01);
        assert!((p.position[1] - 23.46).abs() < 0.01);
        assert!((p.position[2] - 742.72).abs() < 0.01);
        let px = p.proxy.unwrap();
        assert_eq!(px.playfield, Identity { kind: 0xC79C, instance: 4582 });
        assert_eq!((px.attribute, px.exit_door), (0, 0));
        assert_eq!(px.exit_door_id, Identity { kind: 0x9C50, instance: 4582 });
        assert_eq!(p.rdb_playfield(), Some(Identity { kind: 1_000_001, instance: 4582 }));
        let g = p.generator.as_ref().unwrap();
        assert_eq!(g.identity, Identity { kind: 0xC77D, instance: 0 });
        assert_eq!((g.revision, g.marker, g.buildings.len()), (1, 1, 0));
        assert_eq!((p.world_x, p.world_z), (100_000, 100_000));
        assert!(p.rest.is_empty());
    }

    #[test]
    fn full_character() {
        let v = all(FULL_CHARACTER);
        assert_eq!(v.len(), 1);
        let (sender, h, World::FullCharacter(c)) = &v[0] else { panic!() };
        assert_eq!((*sender, h.target), (1, Identity { kind: 0xC350, instance: 0x6584 }));
        assert_eq!(c.version, 26);
        assert!(c.rest.is_empty());
        assert!(c.list_18.is_empty() && c.triples_24.is_empty() && c.stat_map.is_empty());
        assert!(c.list_6c.is_empty());
        for g in &c.groups {
            assert_eq!((g.header, g.entries.len()), (1, 0));
        }
        assert_eq!((c.stats_a.len(), c.stats_b.len(), c.stats_u8.len(), c.stats_i16.len()), (81, 147, 8, 14));
        assert_eq!(c.equipment_flags, 0);
        // Names from the client's own stat table (Gamecode FUN_1002f009).
        assert_eq!(c.stat(54), Some(1)); // Level
        assert_eq!(c.stat(60), Some(1)); // Profession
        assert_eq!(c.stat(4), Some(1)); // Breed
        assert_eq!(c.stat(59), Some(3)); // Sex
        assert_eq!(c.stat(12), Some(17530)); // Mesh
        assert_eq!(c.stat(27), Some(34)); // Health
        assert_eq!(c.stat(61), Some(1000)); // Cash
        assert_eq!(c.stat(53), Some(1500)); // IP
        assert_eq!(c.stat(52), Some(0)); // XP
        assert_eq!(c.stat(16), Some(6)); // Strength
        assert_eq!(c.stat(673), Some(31)); // VisualFlags
        assert_eq!(c.stat(181), Some(8)); // MaxNCU (i16 group)
        assert_eq!(c.stat(173), Some(3)); // CurrentMovementMode (u8 group)
        assert_eq!(c.stat(0), Some(528_961)); // Energy
        assert_eq!(c.stats_a[0], (7, 0));
        assert_eq!(c.stats_b[0], (68, 0));
        assert_eq!(c.stats_b[1], (69, 0));
    }

    /// No capture has a non-empty inventory: the captured body with the empty inventory word replaced by two synthetic elements
    /// (layout from `FUN_1002a41a` / `FUN_1002a04a` / `GameData::operator>>(ACGItem_t)`) must decode to the same character.
    #[test]
    fn full_character_inventory_layout() {
        let f = capture_n3().into_iter().find(|f| N3Header::parse(&f.payload).unwrap().0.msg_type == FULL_CHARACTER).unwrap();
        let (_, mut r) = N3Header::parse(&f.payload).unwrap();
        let body = take_rest(&mut r).unwrap();
        assert_eq!(&body[4..8], &UNIT.to_be_bytes(), "captured inventory is empty");
        let mut b = body[..4].to_vec();
        b.extend((3 * UNIT).to_be_bytes());
        let mut elem = |slot: u32, a: i16, bb: i16, id: (u32, i32), item: [i32; 4]| {
            b.extend(slot.to_be_bytes());
            b.extend(a.to_be_bytes());
            b.extend(bb.to_be_bytes());
            b.extend(id.0.to_be_bytes());
            b.extend(id.1.to_be_bytes());
            item.iter().for_each(|v| b.extend(v.to_be_bytes()));
        };
        elem(0x40, 0, 1, (0xC74A, 7), [265_090, 0, 25, 99]);
        elem(6, 2, 3, (0xC74A, 8), [248_345, 248_346, 0x2ff, 0]);
        b.extend(&body[8..]);
        let c = FullCharacter::read(&mut Reader::new(&b)).unwrap();
        assert_eq!(c.inventory.len(), 2);
        assert_eq!(c.inventory[0], InventoryEntry { slot: 0x40, a: 0, b: 1, id: Identity { kind: 0xC74A, instance: 7 }, item: AcgItem { low_id: 265_090, high_id: 265_090, level: 25 } });
        assert_eq!(c.inventory[1].item, AcgItem { low_id: 248_345, high_id: 248_346, level: 0xff }); // 0x2ff masked with 0x1ff
        assert_eq!((c.stat(54), c.stats_a.len(), c.rest.len()), (Some(1), 81, 0));
        // a count that the bytes cannot satisfy is an error, not a panic
        let mut bad = body[..4].to_vec();
        bad.extend((1000 * UNIT).to_be_bytes());
        assert!(FullCharacter::read(&mut Reader::new(&bad)).is_err());
    }

    /// No capture has an active spell: the captured body with its empty spell word (`this+0x78`, between the identity list and the perk map: the last two words)
    /// replaced by two `ModifyStat` spells (the real words of rdb 1000020:101103 / :101105, Eye Implants) must decode to the same character plus the spells.
    #[test]
    fn full_character_spell_list() {
        let f = capture_n3().into_iter().find(|f| N3Header::parse(&f.payload).unwrap().0.msg_type == FULL_CHARACTER).unwrap();
        let (_, mut r) = N3Header::parse(&f.payload).unwrap();
        let body = take_rest(&mut r).unwrap();
        let n = body.len();
        assert_eq!((&body[n - 8..n - 4], &body[n - 4..]), (&UNIT.to_be_bytes()[..], &UNIT.to_be_bytes()[..]), "captured spell list and perk map are empty");
        let mut b = body[..n - 8].to_vec();
        b.extend((3 * UNIT).to_be_bytes());
        for words in [[53045i32, 0, 4, 0, 1, 0, 2, 9, 108, 2], [53045, 0, 4, 0, 1, 0, 2, 9, 101, 3]] {
            words.iter().for_each(|w| b.extend(w.to_be_bytes()));
        }
        b.extend(UNIT.to_be_bytes());
        let c = FullCharacter::read(&mut Reader::new(&b)).unwrap();
        assert!(c.rest.is_empty());
        let got: Vec<_> = c.spells.iter().map(|s| (s.function, s.stat(spells::stat::STAT), s.stat(spells::stat::VALUE))).collect();
        assert_eq!(got, [(0xCF35, 108, 2), (0xCF35, 101, 3)]);
        assert_eq!((c.stat(54), c.stats_a.len()), (Some(1), 81));
        // an undecodable spell keeps the old behaviour: everything from the size word on is `rest`
        let mut bad = body[..n - 8].to_vec();
        bad.extend((2 * UNIT).to_be_bytes());
        bad.extend([0u8; 12]);
        let c = FullCharacter::read(&mut Reader::new(&bad)).unwrap();
        assert!(c.spells.is_empty() && !c.rest.is_empty());
    }

    /// `DoorStatusUpdateIIR_t` body as `FUN_1009fcb8` writes it: version 2, locked, open, `value_c3`, flag, an (empty) stat list.
    fn door_status_body(locked: u8, open: u8, value: i32, flag: u8, stats: &[(u32, i32)]) -> Vec<u8> {
        let mut b = 2u32.to_be_bytes().to_vec();
        b.extend([locked, open]);
        b.extend(value.to_be_bytes());
        b.push(flag);
        b.extend(((stats.len() as u32 + 1) * UNIT).to_be_bytes());
        for (s, v) in stats {
            b.extend(s.to_be_bytes());
            b.extend(v.to_be_bytes());
        }
        b
    }

    #[test]
    fn door_status_update() {
        let d = DoorStatus::read(&mut Reader::new(&door_status_body(0, 1, 7, 1, &[(0x62, 100)]))).unwrap();
        assert_eq!(d, DoorStatus { locked: false, open: true, value_c3: 7, flag_1a: true, stats: vec![(0x62, 100)] });
        // the client's `FUN_1009fbee` reads `== 1`: any other byte is false
        let d = DoorStatus::read(&mut Reader::new(&door_status_body(1, 2, -1, 0, &[]))).unwrap();
        assert_eq!((d.locked, d.open, d.value_c3, d.flag_1a), (true, false, -1, false));
        let b = door_status_body(0, 0, 0, 0, &[]);
        for n in 0..b.len() {
            assert!(DoorStatus::read(&mut Reader::new(&b[..n])).is_err(), "{n} bytes");
        }
        let mut bad = b.clone();
        bad[3] = 3;
        assert!(DoorStatus::read(&mut Reader::new(&bad)).is_err());
        let mut huge = b[..11].to_vec();
        huge.extend((60_000 * UNIT).to_be_bytes());
        assert!(DoorStatus::read(&mut Reader::new(&huge)).is_err());
    }

    /// A door update is the dynel base chain with `version3` 2 and one more `i32` (the captured vending machine's chain, edited).
    #[test]
    fn door_full_update() {
        let f = capture_n3().into_iter().find(|f| f.payload[..4] == VENDING_MACHINE_FULL_UPDATE.to_be_bytes()).unwrap();
        let mut body = f.payload[13..].to_vec();
        let n = body.len();
        body[n - 4..].copy_from_slice(&2u32.to_be_bytes());
        body.extend(9i32.to_be_bytes());
        let d = Door::read(&mut Reader::new(&body)).unwrap();
        let v = DynelBase::read(&mut Reader::new(&f.payload[13..])).unwrap();
        assert_eq!((d.value_78, d.base.version3, d.base.stats.len()), (9, 2, v.stats.len()));
        assert_eq!((d.base.position, d.base.template), (v.position, v.template));
        // a vending machine's `version3` (3) is not a door's
        assert!(Door::read(&mut Reader::new(&f.payload[13..])).is_err());
    }

    #[test]
    fn vending_machine() {
        let v = all(VENDING_MACHINE_FULL_UPDATE);
        assert_eq!(v.len(), 1);
        let (_, h, World::VendingMachine(m)) = &v[0] else { panic!() };
        assert_eq!(h.target, Identity { kind: 0xC75B, instance: 100 });
        let b = &m.base;
        assert_eq!((b.version, b.version2, b.version3, b.x4ac), (11, 2, 3, 50));
        assert_eq!(b.parent, Identity::default());
        let p = b.position.unwrap();
        assert!((p[0] - 940.23).abs() < 0.01 && (p[1] - 47.21).abs() < 0.01 && (p[2] - 875.34).abs() < 0.01);
        let q = b.rotation.unwrap();
        assert!(q[0] == 0.0 && (q[1] - 0.7133).abs() < 1e-3 && (q[3] + 0.7009).abs() < 1e-3);
        assert_eq!(b.playfield, 4582);
        assert_eq!(b.template, Identity { kind: 1_000_015, instance: 0 });
        assert_eq!(b.flags, [0, 0x6F]);
        assert_eq!(b.stats.len(), 9);
        assert_eq!(b.stats[1], (23, 248_371));
        assert_eq!(b.stats[8], (12, 93_117));
        assert!(b.blob.is_empty() && b.list_74.is_empty());
    }

    #[test]
    fn game_time() {
        let v = all(GAME_TIME);
        assert_eq!(v.len(), 1);
        let World::GameTime(t) = &v[0].2 else { panic!() };
        assert_eq!(t.time, 67170.0);
        assert_eq!((t.day_period, t.arg3), (1, 0x0004_37C9));
        assert_eq!(t.arg4 as u32, 0x6AC4_214E);
    }

    #[test]
    fn org_info_and_quests() {
        let v = all(ORG_INFO_PACKET);
        let World::OrgInfo(o) = &v[0].2 else { panic!() };
        assert_eq!((o.org_id, o.name.as_str()), (0, ""));
        let v = all(QUEST_FULL_UPDATE);
        let World::Quests(q) = &v[0].2 else { panic!() };
        assert_eq!((q.quest_count, q.flag), (0, 0));
    }

    #[test]
    fn appearance() {
        let v = all(APPEARANCE_UPDATE);
        assert_eq!(v.len(), 2);
        let World::Appearance(own) = &v[0].2 else { panic!() };
        assert_eq!(v[0].1.target.instance, 0x6584);
        assert_eq!(own.cloth.len(), 5);
        assert_eq!(own.cloth.iter().map(|c| (c.id, c.b, c.c)).collect::<Vec<_>>(), [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0)]);
        assert_eq!(own.attractors, [Attractor { a: 0, b: 0x9EB5, c: 0, d: 4 }]);
        assert_eq!((own.visual_flags, own.extra), (0x1F, 0));
        let World::Appearance(npc) = &v[1].2 else { panic!() };
        assert_eq!(v[1].1.target.instance, 1_026_282);
        assert_eq!(npc.cloth.len(), 5);
        assert!(npc.attractors.is_empty());
        assert_eq!((npc.visual_flags, npc.extra), (0x1F, 0));
    }

    #[test]
    fn character_actions() {
        let v = all(CHARACTER_ACTION);
        assert_eq!(v.len(), 22);
        let acts: Vec<_> = v
            .iter()
            .map(|(_, h, w)| {
                let World::CharacterAction(a) = w else { panic!() };
                (h.target.instance, a.action)
            })
            .collect();
        assert_eq!(acts[0], (0x6584, 0xA7));
        assert_eq!(acts[1], (1_026_263, 0x63));
        let World::CharacterAction(a) = &v[1].2 else { panic!() };
        assert_eq!((a.identity_a, a.identity_b), (Identity::default(), Identity { kind: 0, instance: 503 }));
        let World::CharacterAction(a) = &v[4].2 else { panic!() };
        assert_eq!(a.action, 0x62);
        assert_eq!(a.identity_a, Identity { kind: 0xCF1B, instance: 0x27E79 });
        assert_eq!(a.identity_b, Identity { kind: 1_026_282, instance: 0 });
        assert!(a.text.is_empty());
    }

    #[test]
    fn corpses() {
        let v = all(CORPSE_FULL_UPDATE);
        assert_eq!(v.len(), 7);
        let World::Corpse(c) = &v[0].2 else { panic!() };
        assert_eq!(v[0].1.target, Identity { kind: 0xC76A, instance: 0x161F });
        assert_eq!((c.version, c.base.version), (8, 11));
        assert_eq!(c.base.name(), "Remains of Cross-Wired Junkbot");
        assert_eq!(c.base.playfield, 4582);
        assert_eq!(c.base.stats.len(), 18);
        assert_eq!(c.base.stats[0], (0, 0x0018_1805));
        let p = c.base.position.unwrap();
        assert!((p[0] - 873.94).abs() < 0.01 && (p[1] - 40.10).abs() < 0.01);
        // one SpellData_t (type 0xCF27), the dead character, five empty cloth parts, no texture list
        assert_eq!(c.spells, [CorpseSpell { type_id: 0xCF27, id: 0x1238, version: 4, args: [1, 0, 0, 0, 0, 0, 0x1F7], tail: [1, 4, 0xB331, 0] }]);
        assert_eq!(c.owner, Identity { kind: 0xC350, instance: 1_025_286 });
        assert_eq!(c.cloth.iter().map(|k| (k.part(), k.texture, k.page)).collect::<Vec<_>>(), [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0)]);
        assert!(c.textures.is_empty());
        // every corpse names its dead character in stats CorpseType 415 / CorpseInstance 416
        for (_, h, w) in &v {
            let World::Corpse(c) = w else { unreachable!() };
            let stat = |id| c.base.stats.iter().find(|s| s.0 == id).map(|s| s.1);
            assert_eq!((stat(415), stat(416)), (Some(c.owner.kind), Some(c.owner.instance)), "{:?}", h.target);
            assert_eq!(c.spells.len(), 1);
        }
        let names: Vec<_> = v
            .iter()
            .map(|(_, _, w)| {
                let World::Corpse(c) = w else { panic!() };
                c.base.name()
            })
            .collect();
        assert_eq!(names[1], "Remains of Pwnsfoot the Jolly Hunter");
        assert_eq!(names[4], "Remains of Beach Leet");
    }

    #[test]
    fn unknown_ids_and_short_input() {
        let h = N3Header { msg_type: 0x1234_5678, target: Identity::default(), flag: 0 };
        assert!(decode(&h, &mut Reader::new(&[])).unwrap().is_none());
        // truncated copies of every captured message must error, never panic
        for f in capture_n3() {
            let (h, _) = N3Header::parse(&f.payload).unwrap();
            if !matches!(
                h.msg_type,
                PLAYFIELD_ANARCHY_F
                    | FULL_CHARACTER
                    | VENDING_MACHINE_FULL_UPDATE
                    | GAME_TIME
                    | ORG_INFO_PACKET
                    | QUEST_FULL_UPDATE
                    | APPEARANCE_UPDATE
                    | CHARACTER_ACTION
                    | CORPSE_FULL_UPDATE
            ) {
                continue;
            }
            for cut in [13, 14, 17, 40, f.payload.len() - 1] {
                // the corpse tail is kept raw, so only a cut inside the decoded part fails
                if cut >= f.payload.len() || (h.msg_type == CORPSE_FULL_UPDATE && cut > 40) {
                    continue;
                }
                let (h, mut r) = N3Header::parse(&f.payload[..cut]).unwrap();
                assert!(decode(&h, &mut r).is_err(), "{:#x} cut at {cut}", h.msg_type);
            }
        }
        // bad proxy tag / bad container word
        let mut p = vec![0u8; 0];
        p.extend(0x5F4B_1A39u32.to_be_bytes());
        p.extend([0, 0, 0x9C, 0x50, 0, 0, 0x11, 0xE6, 0]);
        p.extend(4i32.to_be_bytes());
        p.extend([0u8; 12]);
        p.push(b'b');
        p.extend([0u8; 40]);
        let (h, mut r) = N3Header::parse(&p).unwrap();
        assert!(decode(&h, &mut r).is_err());
        let mut q = Vec::new();
        q.extend(FULL_CHARACTER.to_be_bytes());
        q.extend([0, 0, 0xC3, 0x50, 0, 0, 0x65, 0x84, 0]);
        q.extend(26u32.to_be_bytes());
        q.extend(1000u32.to_be_bytes());
        let (h, mut r) = N3Header::parse(&q).unwrap();
        assert!(decode(&h, &mut r).is_err());
    }
}
