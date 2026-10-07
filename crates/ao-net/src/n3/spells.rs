//! `ApplySpellsIIR_t` 0x342C1D1D (server -> client): spells (nano effects) the client runs on a character.
//!
//! Wire (read slot 7 `FUN_10128956` [GC 0x10170644 vftable]): `i32 (n+1)*0x3f1` size word (`FUN_100a6c58`, n < 1000), `n x SpellData_t`, the
//! target `Identity` (`+0x18`), `u8` apply flag (`+0x38`: non-zero = apply, 0 = undo; `FUN_101288f2` passes `flag == 0` as the "undo"
//! argument of `Beholder_t` vtable [0xc] `FUN_100026d0`, which sets bit 0 of `Spell_c+0x24`).
//!
//! One `SpellData_t` (`GameData::operator>>` @ GameData.dll 0x1000d686 -> `SpellFormats_c::ReadBinary` @ 0x1000f4a6, `SpellFormat_c::ReadBinary`
//! @ 0x1000f39e): `i32 function` (`TypeID_e`, 53002 + ...), `i32 p2` (`SpellData+0x10`), `i32 version` (must equal the format's, 4), the
//! criteria (`SpellData_t::ReadBinaryCriteria` @ 0x1000d49f: `i32 count` <= 1000, `count x {i32 stat, i32 value, i32 op < 0x91}`
//! (`GameData::operator>>(Criterion_t)` @ 0x1000b42c)), then one value per argument of the function's `SpellFormat_c` **after the four standard
//! arguments** [`STD`] (`i32`, or for
//! `ComplexType 1` a `i32 len + bytes` string; `SpellFormat_c::BinaryToValue` @ 0x1000f01d) stored as `SetStat(argument stat, value)`
//! (`ValueToSpell` @ 0x1000ebae -> `SpellData_t::SetSpellStat` = `SetStat`), and finally `SpellData_t::PostBinarySpellRead` (vtable [11]
//! @ 0x1000d3ba): function 0xcf17 reads one more criteria list (`(n+1)*0x3f1` + triples), function 0xcf20 an `ExpressionData_t` stream
//! (not decoded: such a spell is an error). `SpellData_t::IsValid` (0xcf0a needs stat 2 or 0x25) is checked after the read.
//!
//! The format table is the constructor `SpellFormats_c::SpellFormats_c` @ 0x1000fb0a (130 formats, 234 function ids; an id without a format
//! uses the empty default format `this+0x10`), extracted mechanically from its decompilation (`Add(ComplexType, SpellStat, default)` calls and the
//! `map[function] = format` stores). Docs: docs/zone/movement.md §10.1.

use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, ensure, Result};
use std::collections::BTreeMap;

pub const APPLY_SPELLS: u32 = 0x342C_1D1D;

/// `SpellFormat_c` version every table entry carries (`SpellFormat_c(true, false, 4)`).
pub const FORMAT_VERSION: i32 = 4;
/// `SpellArgument_c::GetBaseType(ComplexType)`: only `ComplexType 1` is a string.
const STRING_TYPE: u8 = 1;
/// The four standard arguments `SpellFormat_c::SpellFormat_c(bool, bool, int)` [GameData.dll 0x1000fa93] adds before every format's own list:
/// `Add(0, stat 3, 1)`, `Add(0, 4, 0)`, `Add(4, 0x20 target, 0)`, `Add(0, 0x23, -1)` (stat 3 / 4 / 0x23: meaning not traced; 0x20 is `Target`, read by
/// `FUN_100026d0`). Real record rdb 1000020:101103 (fn 0xcf35): `... crit 0 | 1 0 2 9 | 108 2`, i.e. std `1 0 2 9` then stat 108, amount 2.
const STD: [Arg; 4] = [(0, 3), (0, 4), (4, 0x20), (0, 0x23)];
const UNIT: i32 = 0x3F1;

/// `(ComplexType, SpellStat)` of one format argument.
type Arg = (u8, u16);

/// `SpellStat_e` ids the movement code reads (`SpellData_t::GetStat(spell, id)`).
pub mod stat {
    /// Stat id a `ModifyStat` spell changes (argument of functions 0xcf22..0xcf29, ...).
    pub const STAT: u16 = 0;
    /// `Target` (standard argument, `FUN_100026d0`: 2 = the caster's own dynel, 3 = the explicit target, 0xe / 0x17 = owner / pet relations).
    pub const TARGET: u16 = 0x20;
    /// Value of the spell (`GetStat(0x27)`).
    pub const VALUE: u16 = 0x27;
    /// `0xcf81` Fear: `GetStat(0x42) == 1` ends the effect instead of starting it (also the effect handle `0xcf26` stores).
    pub const EFFECT_OR_END: u16 = 0x42;
    /// `0xcf4b` / `0xcf4c`: the Features bit mask (`FUN_100a4999(0x49)`).
    pub const FEATURES_MASK: u16 = 0x49;
    /// `0xcf4b` / `0xcf4c`: duration argument (`FUN_100a4999(0x19)`).
    pub const DURATION: u16 = 0x19;
}

/// Function ids (`SpellData_t +0xc`, the `switch` of `FUN_100a59f5` [GC]) the own-character movement reacts to.
pub mod function {
    /// `FUN_100a767e`: grant the Features bits of stat 0x49 (undo: revoke).
    pub const FEATURES_GRANT: u32 = 0xCF4B;
    /// `FUN_100a7723`: revoke the Features bits of stat 0x49 (undo: grant).
    pub const FEATURES_REVOKE: u32 = 0xCF4C;
    /// `FUN_100a8161`: Fear (crowd-control state 1).
    pub const FEAR: u32 = 0xCF81;
    /// `FUN_100a8246`: full stop, revoke Features 6, grant 0x400 (undo: grant 6, revoke 0x400).
    pub const STUN: u32 = 0xCF82;
    /// `FUN_100a81eb`: crowd-control state 5 (input lock and `NPCVehicle_t` swap).
    pub const CONTROL: u32 = 0xCFF0;
}

/// One `SpellData_t`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Spell {
    /// `TypeID_e` function id (`SpellData+0xc`).
    pub function: u32,
    /// `SpellData+0x10`.
    pub p2: i32,
    /// `SpellData+0x14`.
    pub version: i32,
    /// `{stat, value, op}` criteria (`SpellData+0x28`; for 0xcf17 the second list is appended, `+0x38`).
    pub criteria: Vec<[i32; 3]>,
    /// `SetStat(stat, value)` of the integer arguments, in argument order (a repeated stat keeps the last value).
    pub stats: BTreeMap<u16, i32>,
    /// String arguments by `SpellStat`.
    pub strings: BTreeMap<u16, String>,
}

impl Spell {
    /// `SpellData_t::GetStat`: 0 when the stat was never set.
    pub fn stat(&self, id: u16) -> i32 {
        self.stats.get(&id).copied().unwrap_or(0)
    }
}

/// The decoded message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplySpells {
    pub spells: Vec<Spell>,
    /// The character the spells run on (`+0x18`).
    pub target: Identity,
    /// `+0x38 != 0`: apply; 0: undo.
    pub apply: bool,
}

/// The argument list of `function`'s format (`SpellFormats_c::GetFormat`; an unknown id gets the empty default format).
pub fn format_of(function: u32) -> &'static [Arg] {
    match IDS.binary_search_by_key(&function, |e| e.0) {
        Ok(i) => FORMATS[IDS[i].1 as usize],
        Err(_) => FORMATS[0],
    }
}

// TextureSpellFormat_c (GD 10014440 / ReadBinary 1001474b) owns a
// conditional payload, not the ordinary argument list used by CF2B/2D/2E.
fn read_texture(r: &mut Reader, s: &mut Spell) -> Result<()> {
    s.stats.insert(0x27, 0); // TextureSpellFormat_c::SetDefaultValues 100144f1.
    s.stats.insert(0x39, r.i32()?);
    let marker = r.i32()?;
    let value = if marker < 100 { marker } else { r.i32()? };
    s.stats.insert(0x38, value);
    s.stats.insert(0x3a, r.i32()?);
    s.stats.insert(0x42, if marker < 100 { 0 } else { r.i32()? });
    s.stats.insert(0x43, if marker < 100 { 0 } else { r.i32()? });
    Ok(())
}

fn read_criteria(r: &mut Reader) -> Result<Vec<[i32; 3]>> {
    let n = r.i32()?;
    ensure!((0..=1000).contains(&n), "spell criteria count {n}");
    (0..n).map(|_| read_criterion(r)).collect()
}

fn read_criterion(r: &mut Reader) -> Result<[i32; 3]> {
    let c = [r.i32()?, r.i32()?, r.i32()?];
    ensure!((c[2] as u32) < 0x91, "spell criterion operator {}", c[2]);
    Ok(c)
}

/// `SpellData_t::PostBinarySpellRead` list: `(n+1)*0x3f1` size word, `n` criteria.
fn read_sized_criteria(r: &mut Reader) -> Result<Vec<[i32; 3]>> {
    let w = r.i32()?;
    ensure!(w >= UNIT && w % UNIT == 0, "invalid criteria list found in spell");
    let n = (w / UNIT - 1) as usize;
    ensure!(n <= r.remaining() / 12, "criteria list of {n} does not fit the stream");
    (0..n).map(|_| read_criterion(r)).collect()
}

/// One `SpellData_t` (`GameData::operator>>`).
pub fn read_spell(r: &mut Reader) -> Result<Spell> {
    let mut s = Spell { function: r.u32()?, p2: r.i32()?, version: r.i32()?, ..Spell::default() };
    ensure!(s.version == FORMAT_VERSION, "spell {:#x}: version {} (format {FORMAT_VERSION})", s.function, s.version);
    s.criteria = read_criteria(r)?;
    for &(ct, stat) in STD.iter().chain(format_of(s.function)) {
        if ct == STRING_TYPE {
            let n = r.i32()?;
            ensure!(n >= 0 && n as usize <= r.remaining(), "spell string length {n}");
            let b = r.bytes(n as usize)?;
            let end = b.iter().position(|&byte| byte == 0).unwrap_or(b.len());
            s.strings.insert(stat, String::from_utf8_lossy(&b[..end]).into_owned());
        } else {
            s.stats.insert(stat, r.i32()?);
        }
    }
    if s.function == 0xCF2F {
        read_texture(r, &mut s)?;
    }
    match s.function {
        0xCF17 => s.criteria.extend(read_sized_criteria(r)?),
        0xCF20 => bail!("spell 0xcf20 carries an ExpressionData_t stream (not decoded)"),
        _ => {}
    }
    // `SpellData_t::IsValid`
    if s.function == 0xCF0A && s.stat(2) == 0 && s.stat(0x25) == 0 {
        bail!("invalid spell 0xcf0a: stats 2 and 0x25 are both 0");
    }
    Ok(s)
}

/// Body of an `ApplySpellsIIR_t` (after the 13-byte N3 header).
pub fn parse(body: &[u8]) -> Result<ApplySpells> {
    let mut r = Reader::new(body);
    let w = r.i32()?;
    ensure!(w > 0 && w % UNIT == 0 && w / UNIT - 1 < 1000, "spell list size word {w:#x}");
    let n = w / UNIT - 1;
    let spells = (0..n).map(|_| read_spell(&mut r)).collect::<Result<Vec<_>>>()?;
    let target = Identity::read(&mut r)?;
    let apply = r.u8()? != 0;
    ensure!(r.remaining() == 0, "{} trailing bytes after ApplySpells", r.remaining());
    Ok(ApplySpells { spells, target, apply })
}

fn write_spell(w: &mut Writer, s: &Spell) -> Result<()> {
    w.u32(s.function);
    w.i32(s.p2);
    w.i32(s.version);
    w.i32(s.criteria.len() as i32);
    for c in &s.criteria {
        c.iter().for_each(|v| w.i32(*v));
    }
    for &(ct, stat) in STD.iter().chain(format_of(s.function)) {
        if ct == STRING_TYPE {
            let t = s.strings.get(&stat).map_or("", |t| t.as_str());
            if t.is_empty() {
                w.i32(0);
            } else {
                w.i32(t.len() as i32 + 1);
                w.bytes(t.as_bytes());
                w.u8(0);
            }
        } else {
            w.i32(s.stat(stat));
        }
    }
    if s.function == 0xCF2F {
        w.i32(s.stat(0x39));
        if s.stat(0x42) == 0 && s.stat(0x43) == 0 {
            ensure!(s.stat(0x38) < 100, "texture short-form value must be below 100");
            w.i32(s.stat(0x38));
            w.i32(s.stat(0x3a));
        } else {
            w.i32(100);
            for stat in [0x38, 0x3a, 0x42, 0x43] { w.i32(s.stat(stat)); }
        }
    }
    ensure!(!matches!(s.function, 0xCF17 | 0xCF20), "spell {:#x} has a second stream part", s.function);
    Ok(())
}

/// Encoder (tests, harnesses): the body `parse` reads, for the spell functions without a post-read part.
pub fn encode(target: Identity, spells: &[Spell], apply: bool) -> Result<Vec<u8>> {
    let mut w = Writer::default();
    w.i32((spells.len() as i32 + 1) * UNIT);
    for s in spells {
        write_spell(&mut w, s)?;
    }
    target.write(&mut w);
    w.u8(apply as u8);
    Ok(w.0)
}

/// A spell with `function` and the given integer stats (version 4, no criteria).
pub fn spell(function: u32, stats: &[(u16, i32)]) -> Spell {
    let defaults = STD.iter().map(|&(_, stat)| (stat, 0));
    Spell { function, version: FORMAT_VERSION, stats: defaults.chain(stats.iter().copied()).collect(), ..Spell::default() }
}

// ---- the format table of `SpellFormats_c::SpellFormats_c` [GameData.dll 0x1000fb0a] ------------------------------------------------

static FORMATS: [&[Arg]; 130] = [
    &[],
    &[(0, 39)],
    &[(2, 0), (0, 2), (0, 37), (0, 71)],
    &[(5, 55), (10, 7)],
    &[(2, 0), (0, 39)],
    &[(0, 0), (0, 39)],
    &[(5, 55), (2, 0), (0, 39)],
    &[(5, 55), (0, 39)],
    &[(0, 59)],
    &[(2, 0), (0, 39), (0, 24)],
    &[(0, 5), (0, 6), (10, 7), (0, 45), (0, 11)],
    &[(0, 5), (0, 6), (10, 7), (0, 45), (0, 47), (0, 48), (0, 11)],
    &[(0, 28), (0, 29), (0, 30), (0, 43), (0, 39)],
    &[(0, 39), (6, 77), (0, 84), (0, 72), (0, 73), (0, 28), (0, 29), (0, 30), (0, 102), (0, 81), (0, 103)],
    &[(6, 77), (0, 84), (0, 72), (0, 73), (0, 28), (0, 29), (0, 30), (0, 102), (0, 81), (0, 103)],
    &[(6, 77), (0, 84), (0, 72), (0, 73), (0, 28), (0, 29), (0, 30), (0, 102), (0, 81), (0, 103), (0, 151)],
    &[(6, 77), (0, 84), (0, 72), (0, 73), (0, 28), (0, 29), (0, 30), (0, 102), (0, 81), (0, 103), (0, 128), (0, 129), (0, 130), (0, 131), (0, 132), (0, 133)],
    &[(6, 77), (0, 84), (0, 28), (0, 29), (0, 30), (0, 151), (0, 72), (0, 168), (0, 73)],
    &[(0, 39), (6, 77), (0, 84), (0, 85), (0, 72), (0, 73), (0, 28), (0, 29), (0, 30)],
    &[(6, 77), (0, 84), (0, 85), (0, 72), (0, 73), (0, 28), (0, 29), (0, 30)],
    &[(6, 77), (0, 84), (0, 85)],
    &[(0, 84), (0, 116), (0, 39)],
    &[(7, 57), (8, 8)],
    &[(9, 44)],
    &[(0, 9), (0, 10), (0, 60), (0, 61), (0, 62), (0, 63), (0, 64)],
    &[(6, 77), (0, 79), (0, 80), (0, 60), (0, 61), (0, 62), (0, 63), (0, 64)],
    &[(0, 9), (0, 10), (0, 28), (0, 29), (0, 30)],
    &[(0, 21), (0, 22), (2, 0), (0, 2), (0, 37), (0, 11), (0, 88)],
    &[(0, 12)],
    &[(0, 21)],
    &[(0, 13), (0, 11)],
    &[(3, 17), (0, 18), (0, 19), (0, 20), (0, 11)],
    &[(2, 0), (0, 39), (0, 11), (0, 46), (0, 25)],
    &[(5, 55)],
    &[(0, 9), (0, 10), (0, 31), (0, 100)],
    &[(0, 39), (0, 25)],
    &[(0, 28), (0, 29), (0, 30), (0, 31)],
    &[(0, 36)],
    &[(0, 39)],
    &[(0, 41), (0, 42), (0, 43)],
    &[(2, 0), (0, 40), (0, 78)],
    &[(0, 58), (0, 56), (8, 8)],
    &[(0, 56), (8, 8)],
    &[(0, 56), (1, 0)],
    &[(1, 0)],
    &[(1, 0), (1, 1), (0, 39)],
    &[(1, 0), (0, 39)],
    &[(1, 0), (0, 39), (0, 117)],
    &[(1, 0), (0, 66), (0, 67), (0, 68), (0, 69), (0, 78)],
    &[(0, 39), (0, 49), (0, 50), (0, 51), (0, 52), (0, 53), (0, 54)],
    &[(0, 39), (0, 49), (0, 50), (0, 153), (0, 154), (0, 155), (0, 156), (0, 157), (0, 158), (0, 159), (0, 160), (0, 161), (0, 86)],
    &[(0, 51), (0, 52), (0, 53)],
    &[(0, 92), (0, 93), (0, 11), (0, 39)],
    &[(0, 109)],
    &[(0, 86), (0, 87), (0, 73), (11, 89)],
    &[(0, 88)],
    &[(0, 88), (0, 39)],
    &[(0, 73), (0, 25)],
    &[(3, 17)],
    &[(2, 0), (0, 2), (0, 37), (0, 71), (0, 11)],
    &[(0, 119), (0, 127)],
    &[(0, 125)],
    &[(0, 39)],
    &[(1, 0), (0, 39)],
    &[(0, 23)],
    &[(0, 87), (0, 73), (0, 25)],
    &[(0, 39)],
    &[(0, 94), (0, 95), (0, 96), (0, 97), (0, 98), (0, 99)],
    &[(0, 94), (0, 95), (0, 96), (0, 97), (0, 98), (0, 99), (0, 100)],
    &[(0, 101)],
    &[(0, 39)],
    &[(0, 88), (0, 11)],
    &[(0, 122)],
    &[(0, 106), (0, 107), (0, 108)],
    &[(6, 104)],
    &[(0, 110), (0, 111), (0, 112), (0, 113), (0, 114)],
    &[(0, 118), (0, 119), (0, 127)],
    &[(0, 120), (0, 121), (0, 39), (0, 25), (0, 11)],
    &[(0, 120), (0, 121), (0, 39)],
    &[(0, 123)],
    &[(0, 124)],
    &[(0, 28), (0, 29), (0, 30), (0, 123), (0, 11)],
    &[(0, 123), (0, 11)],
    &[(7, 57), (0, 56), (0, 58), (0, 39)],
    &[(0, 120), (0, 122)],
    &[(0, 121)],
    &[(0, 152), (0, 39)],
    &[(0, 127)],
    &[(0, 123)],
    &[(1, 0)],
    &[(0, 39)],
    &[(0, 28), (0, 29), (0, 30), (0, 134), (0, 135), (0, 136), (0, 87), (0, 73), (11, 89)],
    &[(0, 39), (6, 77), (0, 84), (0, 88)],
    &[(0, 137)],
    &[(0, 39), (0, 138), (0, 139), (0, 140)],
    &[(2, 0), (0, 2), (0, 37), (0, 71), (0, 39)],
    &[(1, 0)],
    &[(0, 39)],
    &[(0, 150)],
    &[(9, 44), (10, 7)],
    &[(0, 39)],
    &[(0, 28), (0, 29), (0, 30), (0, 39)],
    &[(0, 118)],
    &[(1, 0), (0, 39)],
    &[(0, 162), (0, 48), (0, 47), (0, 73), (0, 163), (0, 164), (0, 165), (0, 166)],
    &[(6, 77), (0, 169), (0, 73)],
    &[(6, 77)],
    &[],
    &[(0, 28), (0, 30), (0, 72), (1, 0)],
    &[(0, 101)],
    &[(0, 39), (0, 10)],
    &[(0, 88)],
    &[(0, 39), (0, 11), (0, 72)],
    &[(0, 98), (0, 99), (0, 90)],
    &[(0, 66)],
    &[(0, 48), (0, 47)],
    &[(0, 47)],
    &[(0, 88)],
    &[(0, 43)],
    &[(1, 0), (0, 72), (0, 62), (0, 95), (0, 73)],
    &[(1, 0), (0, 11)],
    &[(2, 0)],
    &[(0, 1), (0, 39)],
    &[(0, 1), (0, 39)],
    &[(0, 73), (0, 39)],
    &[(0, 39)],
    &[(0, 39)],
    &[(1, 0), (1, 1), (1, 2), (6, 77), (0, 84), (0, 161), (0, 66)],
    &[(0, 124), (0, 73)],
    &[(0, 39), (0, 73)],
];

static IDS: [(u32, u8); 234] = [
    (0xcf0a, 2), (0xcf0b, 10), (0xcf0c, 22), (0xcf0d, 26), (0xcf0e, 27), (0xcf10, 28), (0xcf11, 30), (0xcf13, 31), (0xcf14, 9), (0xcf15, 29),
    (0xcf16, 32), (0xcf17, 33), (0xcf18, 36), (0xcf19, 37), (0xcf1a, 37), (0xcf1b, 38), (0xcf1f, 23), (0xcf20, 40), (0xcf21, 39), (0xcf22, 6),
    (0xcf23, 6), (0xcf24, 6), (0xcf25, 7), (0xcf26, 49), (0xcf27, 11), (0xcf28, 36), (0xcf29, 6), (0xcf2a, 10), (0xcf2b, 22), (0xcf2d, 22),
    (0xcf2e, 22), (0xcf2f, 0), (0xcf30, 8), (0xcf31, 8), (0xcf32, 51), (0xcf33, 24), (0xcf34, 48), (0xcf35, 4), (0xcf37, 3), (0xcf38, 44),
    (0xcf3a, 107), (0xcf3b, 1), (0xcf3c, 41), (0xcf3d, 43), (0xcf3e, 44), (0xcf3f, 42), (0xcf40, 12), (0xcf41, 46), (0xcf43, 34), (0xcf44, 35),
    (0xcf45, 25), (0xcf46, 52), (0xcf47, 14), (0xcf48, 19), (0xcf49, 54), (0xcf4a, 55), (0xcf4b, 57), (0xcf4c, 57), (0xcf4d, 107), (0xcf4e, 107),
    (0xcf50, 58), (0xcf51, 59), (0xcf53, 54), (0xcf54, 54), (0xcf55, 60), (0xcf56, 64), (0xcf57, 65), (0xcf58, 66), (0xcf59, 66), (0xcf5a, 67),
    (0xcf5b, 68), (0xcf5c, 36), (0xcf5d, 1), (0xcf5e, 107), (0xcf5f, 71), (0xcf60, 69), (0xcf61, 56), (0xcf62, 61), (0xcf63, 72), (0xcf64, 107),
    (0xcf65, 20), (0xcf66, 107), (0xcf67, 76), (0xcf68, 62), (0xcf69, 107), (0xcf6a, 107), (0xcf6b, 74), (0xcf6c, 74), (0xcf6d, 75), (0xcf6e, 53),
    (0xcf6f, 73), (0xcf70, 63), (0xcf71, 21), (0xcf72, 107), (0xcf73, 62), (0xcf74, 107), (0xcf75, 107), (0xcf76, 4), (0xcf77, 4), (0xcf79, 107),
    (0xcf7a, 107), (0xcf7b, 47), (0xcf7c, 107), (0xcf7d, 77), (0xcf7e, 107), (0xcf7f, 79), (0xcf80, 80), (0xcf81, 107), (0xcf82, 107), (0xcf83, 78),
    (0xcf84, 18), (0xcf85, 13), (0xcf86, 107), (0xcf87, 107), (0xcf88, 107), (0xcf89, 107), (0xcf8a, 107), (0xcf8b, 107), (0xcf8c, 45), (0xcf8d, 38),
    (0xcf8e, 45), (0xcf8f, 107), (0xcf90, 36), (0xcf91, 70), (0xcf92, 107), (0xcf93, 4), (0xcf94, 4), (0xcf95, 4), (0xcf96, 36), (0xcf97, 107),
    (0xcf98, 107), (0xcf99, 107), (0xcf9a, 107), (0xcf9b, 107), (0xcf9c, 107), (0xcf9d, 82), (0xcf9e, 83), (0xcfa0, 71), (0xcfa1, 107),
    (0xcfa2, 107), (0xcfa3, 107), (0xcfa4, 84), (0xcfa5, 85), (0xcfa7, 81), (0xcfa8, 107), (0xcfa9, 107), (0xcfaa, 86), (0xcfab, 107), (0xcfac, 107),
    (0xcfad, 67), (0xcfae, 107), (0xcfaf, 16), (0xcfb0, 107), (0xcfb1, 107), (0xcfb2, 107), (0xcfb3, 87), (0xcfb4, 107), (0xcfb5, 107), (0xcfb6, 88),
    (0xcfb7, 4), (0xcfb8, 107), (0xcfb9, 5), (0xcfba, 107), (0xcfbb, 90), (0xcfbc, 91), (0xcfbd, 93), (0xcfbe, 92), (0xcfbf, 94), (0xcfc0, 4),
    (0xcfc1, 95), (0xcfc3, 6), (0xcfc4, 96), (0xcfc5, 4), (0xcfc6, 97), (0xcfc7, 98), (0xcfc8, 15), (0xcfc9, 99), (0xcfca, 107), (0xcfcb, 100),
    (0xcfcc, 2), (0xcfcd, 107), (0xcfce, 101), (0xcfcf, 102), (0xcfd0, 89), (0xcfd1, 38), (0xcfd3, 121), (0xcfd4, 50), (0xcfd5, 4), (0xcfd6, 1),
    (0xcfd7, 1), (0xcfd8, 107), (0xcfd9, 107), (0xcfda, 103), (0xcfdb, 103), (0xcfdc, 1), (0xcfdd, 104), (0xcfde, 122), (0xcfe1, 107), (0xcfe3, 107),
    (0xcfe4, 17), (0xcfe5, 108), (0xcfe6, 109), (0xcfe7, 109), (0xcfe8, 110), (0xcfe9, 107), (0xcfea, 105), (0xcfeb, 110), (0xcfec, 111),
    (0xcfed, 106), (0xcfee, 112), (0xcfef, 107), (0xcff0, 107), (0xcff1, 68), (0xcff2, 107), (0xcff3, 113), (0xcff4, 114), (0xcff5, 4),
    (0xcff6, 115), (0xcff7, 116), (0xcff8, 117), (0xcff9, 107), (0xcffa, 117), (0xcffb, 118), (0xcffc, 119), (0xcffd, 107), (0xcffe, 107),
    (0xcfff, 120), (0xd000, 123), (0xd001, 124), (0xd002, 125), (0xd003, 126), (0xd004, 127), (0xd005, 128), (0xd006, 129),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Identity {
        Identity { kind: 50000, instance: 77 }
    }

    #[test]
    fn table_matches_the_client() {
        // 234 ids sorted for the binary search, every id maps to an existing format
        assert!(IDS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(IDS.iter().all(|e| (e.1 as usize) < FORMATS.len()));
        // FUN_100a59f5's switch handles these; the formats read from SpellFormats_c::SpellFormats_c
        assert_eq!(format_of(function::FEAR), &[] as &[Arg]);
        assert_eq!(format_of(function::STUN), &[] as &[Arg]);
        assert_eq!(format_of(function::CONTROL), &[] as &[Arg]);
        assert_eq!(format_of(function::FEATURES_GRANT), &[(0, 73), (0, 25)]);
        assert_eq!(format_of(function::FEATURES_REVOKE), &[(0, 73), (0, 25)]);
        assert_eq!(format_of(0xCF22), &[(5, 55), (2, 0), (0, 39)], "ModifyStat: item, stat id, value");
        assert_eq!(format_of(0xCFFF_FFFF), &[] as &[Arg], "unknown ids use the default format");
    }

    #[test]
    fn roundtrip_with_criteria_and_strings() {
        let mut features = spell(function::FEATURES_REVOKE, &[(stat::FEATURES_MASK, 4), (stat::DURATION, 12)]);
        features.criteria = vec![[1, 2, 3], [4, 5, 0x90]];
        features.p2 = 9;
        let fear = spell(function::FEAR, &[]);
        let body = encode(me(), &[fear.clone(), features.clone()], true).unwrap();
        let m = parse(&body).unwrap();
        assert_eq!(m.spells, vec![fear, features]);
        assert_eq!((m.target, m.apply), (me(), true));
        assert!(!parse(&encode(me(), &[], false).unwrap()).unwrap().apply);
        // GD 1000fb0a: CF41 adds string stat0, then integer stat0x27.
        assert_eq!(format_of(0xCF41), &[(1, 0), (0, 39)]);
        let mut s = spell(0xCF41, &[(39, 5)]);
        s.strings.insert(0, "hello".into());
        assert_eq!(parse(&encode(me(), &[s.clone()], true).unwrap()).unwrap().spells, vec![s]);
    }

    /// The spell words of rdb 1000020:101103 ("Eye Implant: Sharp Obj, Faded", item event 14) and :101104 as the little-endian record stores them, swapped to the
    /// wire's byte order: `function id version | criteria 0 | std 1 0 2 9 | stat amount`.
    #[test]
    fn real_item_record_spell_words() {
        for (words, amount) in [([53045, 0, 4, 0, 1, 0, 2, 9, 108, 2], 2), ([53045, 0, 4, 0, 1, 0, 2, 9, 108, 42], 42)] {
            let bytes: Vec<u8> = words.iter().flat_map(|w: &i32| w.to_be_bytes()).collect();
            let mut r = Reader::new(&bytes);
            let s = read_spell(&mut r).unwrap();
            assert_eq!(r.remaining(), 0);
            assert_eq!(s.function, 0xCF35);
            assert_eq!((s.stat(stat::STAT), s.stat(stat::VALUE), s.stat(stat::TARGET)), (108, amount, 2));
            assert_eq!((s.stat(3), s.stat(4), s.stat(0x23)), (1, 0, 9));
        }
    }

    #[test]
    fn texture_and_chat_payload_boundaries() {
        // Installed 1000020:29426 at 232: CF2F's short payload is 14038,1,0.
        // The following element begins at 276, not inside that final zero.
        for tail in [vec![14038, 1, 0], vec![14038, 100, 7, 8, 9, 10]] {
            let mut words = vec![0xcf2f_i32, 0, 4, 0, 1, 0, 2, 9];
            words.extend(tail);
            let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let mut r = Reader::little_endian(&bytes);
            let s = read_spell(&mut r).unwrap();
            assert_eq!(r.remaining(), 0);
            assert_eq!(s.stat(0x39), 14038);
            let mut wire = Writer::default();
            write_spell(&mut wire, &s).unwrap();
            let mut wire_reader = Reader::new(&wire.0);
            assert_eq!(read_spell(&mut wire_reader).unwrap(), s);
            assert_eq!(wire_reader.remaining(), 0);
            for end in 0..bytes.len() {
                assert!(read_spell(&mut Reader::little_endian(&bytes[..end])).is_err());
            }
        }
        // Installed 1000020:29740 at 419: CF41 string "Gulp!" then stat39=2.
        let mut bytes: Vec<_> = [0xcf41_i32, 0, 4, 0, 1, 0, 2, 9, 6]
            .into_iter().flat_map(i32::to_le_bytes).collect();
        bytes.extend(b"Gulp!\0");
        bytes.extend(2i32.to_le_bytes());
        let mut r = Reader::little_endian(&bytes);
        let s = read_spell(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(s.strings.get(&0).unwrap(), "Gulp!");
        assert_eq!(s.stat(39), 2);
        let mut wire = Writer::default();
        write_spell(&mut wire, &s).unwrap();
        let mut expected: Vec<_> = [0xcf41_i32, 0, 4, 0, 1, 0, 2, 9, 6]
            .into_iter().flat_map(i32::to_be_bytes).collect();
        expected.extend(b"Gulp!\0");
        expected.extend(2i32.to_be_bytes());
        assert_eq!(wire.0, expected);
        let mut wire_reader = Reader::new(&wire.0);
        assert_eq!(read_spell(&mut wire_reader).unwrap(), s);
        assert_eq!(wire_reader.remaining(), 0);
        for end in 0..bytes.len() {
            assert!(read_spell(&mut Reader::little_endian(&bytes[..end])).is_err());
        }
    }

    #[test]
    fn native_string_overload_arguments_are_not_omitted() {
        // GD 1000fb0a string-overload Add calls, artifact22543.
        // 120802:354 authors CF8C "John\0","Doe\0",3.
        // 257579:168 authors CF7B "Please enter your nickname:;/name\0",0,1.
        // 284161:195 authors CF70 "{stock_pet_attack_accept}\0",1.
        let cases: &[(u32, &[&str], &[(u8, u16)], &[i32])] = &[
            (0xCF8C, &["John", "Doe"], &[(0, 39)], &[3]),
            (0xCF8E, &["John", "Doe"], &[(0, 39)], &[3]),
            (0xCF7B, &["Please enter your nickname:;/name"], &[(0, 39), (0, 117)], &[0, 1]),
            (0xCF70, &["{stock_pet_attack_accept}"], &[(0, 39)], &[1]),
            (0xCFD0, &["text"], &[], &[]),
            (0xCFC4, &["text"], &[], &[]),
            (0xCFDA, &["text"], &[(0, 39)], &[2]),
            (0xCFDB, &["text"], &[(0, 39)], &[2]),
            (0xCFFC, &["text"], &[(0, 72), (0, 62), (0, 95), (0, 73)], &[2, 3, 4, 5]),
            (0xCFFF, &["text"], &[(0, 11)], &[2]),
            (0xD004, &["one", "two", "three"], &[(6, 77), (0, 84), (0, 161), (0, 66)], &[2, 3, 4, 5]),
        ];
        for &(function, strings, integers, values) in cases {
            let expected: Vec<_> = (0..strings.len()).map(|stat| (1, stat as u16))
                .chain(integers.iter().copied()).collect();
            assert_eq!(format_of(function), expected);
            let mut record: Vec<_> = [function as i32, 0, 4, 0, 1, 0, 2, 9]
                .into_iter().flat_map(i32::to_le_bytes).collect();
            let mut wire: Vec<_> = [function as i32, 0, 4, 0, 1, 0, 2, 9]
                .into_iter().flat_map(i32::to_be_bytes).collect();
            for text in strings {
                record.extend((text.len() as i32 + 1).to_le_bytes());
                wire.extend((text.len() as i32 + 1).to_be_bytes());
                for bytes in [&mut record, &mut wire] {
                    bytes.extend(text.as_bytes());
                    bytes.push(0);
                }
            }
            record.extend(values.iter().flat_map(|value| value.to_le_bytes()));
            wire.extend(values.iter().flat_map(|value| value.to_be_bytes()));
            let mut r = Reader::little_endian(&record);
            let spell = read_spell(&mut r).unwrap();
            assert_eq!(r.remaining(), 0);
            for (stat, text) in strings.iter().enumerate() {
                assert_eq!(spell.strings.get(&(stat as u16)).unwrap(), text);
            }
            let mut r = Reader::new(&wire);
            assert_eq!(read_spell(&mut r).unwrap(), spell);
            assert_eq!(r.remaining(), 0);
            let mut encoded = Writer::default();
            write_spell(&mut encoded, &spell).unwrap();
            assert_eq!(encoded.0, wire);
            for cut in 0..record.len() {
                assert!(read_spell(&mut Reader::little_endian(&record[..cut])).is_err());
                assert!(read_spell(&mut Reader::new(&wire[..cut])).is_err());
            }
        }
    }

    #[test]
    fn installed_cf38_cf3e_string_boundaries() {
        // GD 1000fb0a registers both ids with Add(1, stat0, "").
        // RDB 1000020:40082 offsets 1739..1780: CF3E + "robe\0";
        // the next CF35 begins at 1780. Same payload at 21797:1099
        // and 231353:1471 precedes the next item element instead.
        for function in [0xCF38, 0xCF3E] {
            assert_eq!(format_of(function), &[(1, 0)]);
            let prefix = [function as i32, 0, 4, 0, 1, 0, 2, 9, 5];
            let mut record: Vec<_> = prefix.iter().flat_map(|word| word.to_le_bytes()).collect();
            record.extend(b"robe\0");
            let mut r = Reader::little_endian(&record);
            let s = read_spell(&mut r).unwrap();
            assert_eq!(r.remaining(), 0);
            assert_eq!(s.strings.get(&0).unwrap(), "robe");
            let mut wire: Vec<_> = prefix.iter().flat_map(|word| word.to_be_bytes()).collect();
            wire.extend(b"robe\0");
            let mut r = Reader::new(&wire);
            assert_eq!(read_spell(&mut r).unwrap(), s);
            assert_eq!(r.remaining(), 0);
            let mut encoded = Writer::default();
            write_spell(&mut encoded, &s).unwrap();
            assert_eq!(encoded.0, wire);
            for cut in 0..record.len() {
                assert!(read_spell(&mut Reader::little_endian(&record[..cut])).is_err());
                assert!(read_spell(&mut Reader::new(&wire[..cut])).is_err());
            }
        }
    }

    #[test]
    fn installed_cf34_string_precedes_integer_arguments() {
        // RDB 1000020:43551, event 0, record offsets 598..692. The 38-byte
        // string at 634 includes NUL; the next CF41 spell starts at 692.
        // GD 1000fb0a adds (1,0) before integer stats 66/67/68/69/78.
        assert_eq!(format_of(0xCF34), &[(1, 0), (0, 66), (0, 67), (0, 68), (0, 69), (0, 78)]);
        let prefix: [i32; 9] = [53044, 0, 4, 0, 1, 0, 3, 9, 38];
        let text = b"You have 30 seconds to swap implants.\0";
        let mut record: Vec<u8> = prefix.iter().flat_map(|word| word.to_le_bytes()).collect();
        record.extend_from_slice(text);
        record.extend_from_slice(&[0; 20]);
        assert_eq!(record.len(), 94);
        let mut r = Reader::little_endian(&record);
        let s = read_spell(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(s.strings.get(&0).unwrap(), "You have 30 seconds to swap implants.");
        assert_eq!((s.stat(3), s.stat(4), s.stat(0x20), s.stat(0x23)), (1, 0, 3, 9));
        for stat in [66, 67, 68, 69, 78] {
            assert_eq!(s.stats.get(&stat), Some(&0));
        }
        let mut wire: Vec<u8> = prefix.iter().flat_map(|word| word.to_be_bytes()).collect();
        wire.extend_from_slice(text);
        wire.extend_from_slice(&[0; 20]);
        let mut r = Reader::new(&wire);
        assert_eq!(read_spell(&mut r).unwrap(), s);
        assert_eq!(r.remaining(), 0);
        let mut encoded = Writer::default();
        write_spell(&mut encoded, &s).unwrap();
        assert_eq!(encoded.0, wire);
        for cut in 0..record.len() {
            assert!(read_spell(&mut Reader::little_endian(&record[..cut])).is_err(), "record cut {cut}");
            assert!(read_spell(&mut Reader::new(&wire[..cut])).is_err(), "wire cut {cut}");
        }
        for length in [-1i32, i32::MAX] {
            let mut bad = wire.clone();
            bad[32..36].copy_from_slice(&length.to_be_bytes());
            assert!(read_spell(&mut Reader::new(&bad)).is_err());
        }
    }

    #[test]
    fn malformed_bodies_are_errors() {
        let good = encode(me(), &[spell(function::FEAR, &[])], true).unwrap();
        for cut in 0..good.len() {
            assert!(parse(&good[..cut]).is_err(), "cut {cut}");
        }
        let mut bad = good.clone();
        bad[0] ^= 1; // size word not a multiple of 0x3f1
        assert!(parse(&bad).is_err());
        let mut bad = good.clone();
        bad[12] = 5; // version
        assert!(parse(&bad).is_err());
        let mut long = good;
        long.push(0);
        assert!(parse(&long).is_err());
        assert!(parse(&encode(me(), &[spell(0xCF0A, &[(2, 0), (0x25, 0)])], true).unwrap()).is_err(), "IsValid");
    }
}
