//! Server messages that place or push the own character without being a `CharDCMove` (which the client drops for its own
//! dynel, docs/zone/movement.md §3.1): `SetPosIIR_c`, `ImpulseIIR_c`, `ResurrectIIR_t`. Layouts and addresses: docs/zone/movement.md §10.
//! None of them is in the live captures; the tests build frames with the encoders below.
//!
//! Like [`super::action::parse_social_body`] the parsers take the bytes after the 13-byte N3 header (`N3::Unknown`).

use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `SetPosIIR_c` (vtable [GC 0x101612e0], read `FUN_10076df2`, apply `FUN_10076e5a`).
pub const SET_POS: u32 = 0x195E496E;
/// `ImpulseIIR_c` (vtable [GC 0x10160fe0], read `FUN_10074731`, apply `FUN_100745bb`).
pub const IMPULSE: u32 = 0x5F4A4C6C;
/// `ResurrectIIR_t` (vtable [GC 0x10161290], read [GC 0x1007696a], apply `FUN_100769bd`).
pub const RESURRECT: u32 = 0x445F2A0B;
/// `RelocateDynelsIIR_t` (vtable [GC 0x1015d484], read slot 7 `FUN_1003a40a`, apply `FUN_1003a364`).
pub const RELOCATE: u32 = 0x264B514B;
/// `FightModeUpdate_t` (vtable [GC 0x1016ffdc], read slot 7 `FUN_10124b02`, validity `FUN_101248ef`, apply `FUN_10124b70`).
pub const FIGHT_MODE_UPDATE: u32 = 0x371D0542;

/// `SetPosIIR_c`: `Vec3 pos` (object `+0x18`), `u8` (`+0x24`: `UpdateLastAllowedPosition` bookkeeping), `i32` (`+0x28`: for the client
/// char non-zero prints `Feedback_CrowdLimiting`), `u8` (`+0x2c`: full stop if the dynel is moving, `FUN_10059ae5(1)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetPos {
    pub pos: [f32; 3],
    pub last_allowed: bool,
    pub crowd_limit: i32,
    pub stop: bool,
}

/// One element of `ImpulseIIR_c` (24 bytes: `Identity`, `Vec3`, `f32`): `Vehicle_t::Impulse(delta, time)` on that dynel's vehicle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Push {
    pub target: Identity,
    /// Horizontal displacement of the landing point (`x`, `z`; the `y` part is overwritten by the vehicle's own height, Vehicle.dll 0x1000cd61).
    pub delta: [f32; 3],
    /// Flight time in seconds (`BallisticPath_t` duration).
    pub time: f32,
}

/// `ResurrectIIR_t`: `SetStat(Health 0x1b, health)`, `SetStat(CurrentNano 0xd6, nano)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resurrect {
    pub health: i32,
    pub nano: i32,
}

/// `RelocateDynelsIIR_t`: the `Identity` at `+0x18` is the new parent (`n3Dynel_t::RelocateDynel(parent, child, NullPos, NullRot)` per child),
/// followed by a `(n+1)*0x3f1` size word and `n` child `Identity`s (`FUN_1002b8b6`, n < 0x7531).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocate {
    pub parent: Identity,
    pub children: Vec<Identity>,
}

/// `FightModeChange_t` (`FUN_1011f5a3`): `u32 id` (non-zero), `u16 len + name` of the district (non-empty), `u8 flags`, `u8 value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FightModeChange {
    /// Change id (`+0`), the key of a later removal.
    pub id: u32,
    /// `PlayfieldDistrictInfo_t::GetDistrictData(name)`.
    pub district: String,
    /// Flags bit 0 (`+0x24`): the level is set to `value` (otherwise `value` is added).
    pub set: bool,
    /// Flags bit 1 (`+0x20`): "fixed" - once a fixed change applied, later non-fixed changes are skipped (`FUN_1011f88b`).
    pub fixed: bool,
    /// Flags bit 2 (`+0x2c`): remove the change with this id instead of adding one.
    pub remove: bool,
    /// `0..=4` (add: the stream check also allows `-4..-1` but the `& 0xf8 == 0` test rejects them).
    pub value: i8,
}

/// `FightModeUpdate_t`: playfield `Identity` (`+0x18`, `.instance` selects the `PlayfieldAnarchy_t`) and a `(n+1)*0x3f1`-counted change list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FightModeUpdate {
    pub playfield: Identity,
    pub changes: Vec<FightModeChange>,
}

/// `FUN_1002b8b6` / `FUN_101251cf` size word: `(n+1)*0x3f1` with `n < 0x7531`.
fn counted(r: &mut Reader) -> Result<usize> {
    let w = r.u32()?;
    if w == 0 || w % 0x3F1 != 0 || w / 0x3F1 > 0x7531 {
        bail!("container size word {w:#x} is not (n+1)*0x3f1 with n < 0x7531");
    }
    Ok((w / 0x3F1 - 1) as usize)
}

pub fn parse_relocate(body: &[u8]) -> Result<Relocate> {
    let mut r = Reader::new(body);
    let parent = Identity::read(&mut r)?;
    let n = counted(&mut r)?;
    if n > r.remaining() / 8 {
        bail!("Relocate child count {n} does not fit {} bytes", r.remaining());
    }
    let children = (0..n).map(|_| Identity::read(&mut r)).collect::<Result<_>>()?;
    done(&r, "Relocate")?;
    Ok(Relocate { parent, children })
}

pub fn parse_fight_mode_update(body: &[u8]) -> Result<FightModeUpdate> {
    let mut r = Reader::new(body);
    let playfield = Identity::read(&mut r)?;
    let n = counted(&mut r)?;
    if n > r.remaining() / 8 {
        bail!("FightModeUpdate change count {n} does not fit {} bytes", r.remaining());
    }
    let mut changes = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.u32()?;
        let district = r.str_i16()?;
        let (flags, value) = (r.u8()?, r.u8()?);
        // `FUN_1011f5a3`: id and name must be non-empty; add: value + 4 <= 8 (u8), set: value <= 4; both: value & 0xf8 == 0
        let ok = id != 0 && !district.is_empty() && (flags & 1 != 0 || value.wrapping_add(4) <= 8) && (flags & 1 == 0 || value <= 4) && value & 0xf8 == 0;
        if !ok {
            bail!("Invalid FightModeChange_t in stream (id {id}, district {district:?}, flags {flags:#x}, value {value})");
        }
        changes.push(FightModeChange { id, district, set: flags & 1 != 0, fixed: flags & 2 != 0, remove: flags & 4 != 0, value: value as i8 });
    }
    done(&r, "FightModeUpdate")?;
    Ok(FightModeUpdate { playfield, changes })
}

fn vec3(r: &mut Reader) -> Result<[f32; 3]> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

fn done(r: &Reader, what: &str) -> Result<()> {
    if r.remaining() != 0 {
        bail!("{} trailing bytes after {what}", r.remaining());
    }
    Ok(())
}

pub fn parse_set_pos(body: &[u8]) -> Result<SetPos> {
    let mut r = Reader::new(body);
    let m = SetPos { pos: vec3(&mut r)?, last_allowed: r.u8()? != 0, crowd_limit: r.i32()?, stop: r.u8()? != 0 };
    done(&r, "SetPos")?;
    Ok(m)
}

pub fn parse_impulse(body: &[u8]) -> Result<Vec<Push>> {
    let mut r = Reader::new(body);
    let n = r.i32()?;
    if n < 0 || n as usize > r.remaining() / 24 {
        bail!("Impulse element count {n} does not fit {} bytes", r.remaining());
    }
    let v = (0..n).map(|_| Ok(Push { target: Identity::read(&mut r)?, delta: vec3(&mut r)?, time: r.f32()? })).collect::<Result<_>>()?;
    done(&r, "Impulse")?;
    Ok(v)
}

pub fn parse_resurrect(body: &[u8]) -> Result<Resurrect> {
    let mut r = Reader::new(body);
    let m = Resurrect { health: r.i32()?, nano: r.i32()? };
    done(&r, "Resurrect")?;
    Ok(m)
}

fn header(msg: u32, target: Identity) -> Writer {
    let mut w = Writer::default();
    w.u32(msg);
    target.write(&mut w);
    w.u8(1);
    w
}

/// Full N3 payload of a `SetPosIIR_c` for `target` (tests, server emulation).
pub fn set_pos(target: Identity, m: &SetPos) -> Vec<u8> {
    let mut w = header(SET_POS, target);
    m.pos.iter().for_each(|&f| w.f32(f));
    w.u8(m.last_allowed as u8);
    w.i32(m.crowd_limit);
    w.u8(m.stop as u8);
    w.0
}

/// Full N3 payload of an `ImpulseIIR_c` whose header dynel is `target`.
pub fn impulse(target: Identity, pushes: &[Push]) -> Vec<u8> {
    let mut w = header(IMPULSE, target);
    w.i32(pushes.len() as i32);
    for p in pushes {
        p.target.write(&mut w);
        p.delta.iter().for_each(|&f| w.f32(f));
        w.f32(p.time);
    }
    w.0
}

/// Full N3 payload of a `ResurrectIIR_t`.
pub fn resurrect(target: Identity, m: &Resurrect) -> Vec<u8> {
    let mut w = header(RESURRECT, target);
    w.i32(m.health);
    w.i32(m.nano);
    w.0
}

/// Full N3 payload of a `RelocateDynelsIIR_t` whose header dynel is `target`.
pub fn relocate(target: Identity, m: &Relocate) -> Vec<u8> {
    let mut w = header(RELOCATE, target);
    m.parent.write(&mut w);
    w.u32((m.children.len() as u32 + 1) * 0x3F1);
    m.children.iter().for_each(|c| c.write(&mut w));
    w.0
}

/// Full N3 payload of a `FightModeUpdate_t` (header target: the playfield).
pub fn fight_mode_update(target: Identity, m: &FightModeUpdate) -> Vec<u8> {
    let mut w = header(FIGHT_MODE_UPDATE, target);
    m.playfield.write(&mut w);
    w.u32((m.changes.len() as u32 + 1) * 0x3F1);
    for c in &m.changes {
        w.u32(c.id);
        w.str_i16(&c.district);
        w.u8(c.set as u8 | (c.fixed as u8) << 1 | (c.remove as u8) << 2);
        w.u8(c.value as u8);
    }
    w.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Identity = Identity { kind: 50000, instance: 7 };

    #[test]
    fn set_pos_round_trip() {
        let m = SetPos { pos: [1.0, 2.0, 3.0], last_allowed: true, crowd_limit: 2, stop: true };
        let p = set_pos(ME, &m);
        assert_eq!(&p[..4], &SET_POS.to_be_bytes());
        assert_eq!(parse_set_pos(&p[13..]).unwrap(), m);
        assert!(parse_set_pos(&p[13..p.len() - 1]).is_err());
        assert!(parse_set_pos(&[p.clone(), vec![0]].concat()[13..]).is_err());
    }

    #[test]
    fn impulse_round_trip_and_bounds() {
        let v = [Push { target: ME, delta: [4.0, 0.0, -2.0], time: 0.8 }, Push { target: Identity { kind: 50000, instance: 9 }, delta: [0.0; 3], time: 1.0 }];
        let p = impulse(ME, &v);
        assert_eq!(parse_impulse(&p[13..]).unwrap(), v);
        // a count larger than the payload is refused before allocating
        let mut bad = p[13..].to_vec();
        bad[..4].copy_from_slice(&0x7fff_ffffi32.to_be_bytes());
        assert!(parse_impulse(&bad).is_err());
        assert!(parse_impulse(&p[13..p.len() - 3]).is_err());
    }

    #[test]
    fn relocate_round_trip_and_bounds() {
        let m = Relocate { parent: Identity { kind: 0xC350, instance: 3 }, children: vec![ME, Identity { kind: 50000, instance: 9 }] };
        let p = relocate(ME, &m);
        assert_eq!(&p[..4], &RELOCATE.to_be_bytes());
        assert_eq!(parse_relocate(&p[13..]).unwrap(), m);
        assert!(parse_relocate(&p[13..p.len() - 1]).is_err());
        let mut bad = p[13..].to_vec();
        bad[8..12].copy_from_slice(&(0x3F1u32 * 0x7000).to_be_bytes());
        assert!(parse_relocate(&bad).is_err(), "a count larger than the payload");
        bad[8..12].copy_from_slice(&5u32.to_be_bytes());
        assert!(parse_relocate(&bad).is_err(), "not a (n+1)*0x3f1 word");
    }

    #[test]
    fn fight_mode_update_round_trip_and_validity() {
        let ch = |id, d: &str, set, fixed, remove, value| FightModeChange { id, district: d.into(), set, fixed, remove, value };
        let m = FightModeUpdate { playfield: Identity { kind: 0x9C50, instance: 4582 }, changes: vec![ch(5, "Tir", true, false, false, 3), ch(6, "Tir", false, true, true, 1)] };
        let p = fight_mode_update(m.playfield, &m);
        assert_eq!(parse_fight_mode_update(&p[13..]).unwrap(), m);
        let bad = |c| fight_mode_update(m.playfield, &FightModeUpdate { playfield: m.playfield, changes: vec![c] });
        for c in [ch(0, "Tir", true, false, false, 1), ch(1, "", true, false, false, 1), ch(1, "Tir", true, false, false, 5), ch(1, "Tir", false, false, false, -1), ch(1, "Tir", false, false, false, 8)] {
            assert!(parse_fight_mode_update(&bad(c.clone())[13..]).is_err(), "{c:?}");
        }
    }

    #[test]
    fn resurrect_round_trip() {
        let m = Resurrect { health: 120, nano: 40 };
        assert_eq!(parse_resurrect(&resurrect(ME, &m)[13..]).unwrap(), m);
    }
}
