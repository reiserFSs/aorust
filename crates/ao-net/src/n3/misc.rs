//! Zone N3 messages: follow-target path, combat, buff, generic command, despawn, in-play.
//! Layouts, RE addresses and live evidence: docs/zone/misc.md.
//!
//! Every message is an `n3InfoItemRemote_t` subclass: the wire message type is
//! `n3InfoItemRemote_t::MapToKey(class name)` ([`key`], N3.dll 0x10009826) and the body is the
//! class's `ReadSubClass` (Gamecode.dll vtable slot 7, written by slot 8).

use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `n3InfoItemRemote_t::MapToKey` (N3.dll 0x10009826): XOR of `(i8)c << ((i & 3) * 8)`.
pub fn key(name: &str) -> u32 {
    name.bytes().enumerate().fold(0, |k, (i, c)| k ^ ((c as i8 as i32 as u32) << ((i & 3) * 8)))
}

pub const FOLLOW_TARGET: u32 = 0x260F_3671; // FollowTargetIIR_c
pub const ATTACK: u32 = 0x2849_4070; // AttackIIR_t
pub const ATTACK_INFO: u32 = 0x4600_2F16; // AttackInfoIIR_t
pub const STOP_FIGHT: u32 = 0x4A41_203E; // StopFightIIR_t
pub const TO_CLIENT_QUIT: u32 = 0x3651_0078; // n3ToClientQuitIIR_t
pub const MISSED_ATTACK_INFO: u32 = 0x5C65_4B28; // MissedAttackInfoIIR_t
pub const GENERIC_CMD: u32 = 0x5252_6858; // GenericCmd_t
pub const BUFF: u32 = 0x3934_3C68; // BuffIIR_c
pub const CHAR_SEC_SPEC_ATTACK: u32 = 0x5149_2120; // CharSecSpecAttackIIR_t
pub const SPECIAL_ATTACK_INFO: u32 = 0x754F_1115; // SpecialAttackInfoIIR_t
pub const CHAR_IN_PLAY: u32 = 0x570C_2039; // CharInPlayIIR_t

/// `Vector3_t` as read by `FUN_1000404e` [GC]: three BE `f32`, as on the wire (Y is height).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub(super) fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self { x: r.f32()?, y: r.f32()?, z: r.f32()? })
    }
    pub(super) fn write(&self, w: &mut Writer) {
        w.f32(self.x);
        w.f32(self.y);
        w.f32(self.z);
    }
}

/// `FollowTargetIIR_c` [GC 0x10073130]. The client keeps at most 30 waypoints.
#[derive(Debug, Clone, PartialEq)]
pub struct FollowTarget {
    /// Leading byte: 1 = short form (target = the header dynel itself, speed 0, `pos` = `path[0]`),
    /// anything else = long form (written as 2).
    pub form: u8,
    /// Passed to the vehicle's movement controller (vtable +0x18) by the apply code. Live: 21, 24, 25.
    pub mode: u8,
    /// Dynel to follow; `0:0` = none.
    pub target: Identity,
    /// Long form only; passed through to `Vehicle_t::SetFollow` unused by it. Live: always 0.0.
    pub speed: f32,
    /// Position the dynel is set to (the apply code calls `SetRelPosIgnoreCollision(path[0])`).
    pub pos: Vec3,
    pub path: Vec<Vec3>,
}

/// `AttackIIR_t` [GC 0x1007c590]: header dynel starts attacking `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attack {
    pub target: Identity,
    /// Signed byte at object +0x20; ignored by the apply code. Live: always 0.
    pub flag: i8,
}

/// `AttackInfoIIR_t` [GC 0x1009ec5d]. Field names after the offsets in the client object; meanings in the doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttackInfo {
    /// +0x1c: hit points taken off Health (stat 27) by `FUN_100693a3`.
    pub damage: i32,
    /// +0x20: stored into stat 26 when >= 0 by `FUN_1006a8f3`. Live: always -1.
    pub value_20: i32,
    /// +0x18: index (0..15, 0x3d, 0x3f) for `FUN_10068072`. Live: 0..6.
    pub slot: i32,
    /// +0x24: the other combatant (the attacker when the header is the one hit).
    pub other: Identity,
    /// +0x2c: id handed to `FUN_1005ae91` when non-zero. Live: 0 or 4.
    pub unk_2c: i32,
    /// +0x30. Live: 3 or 4.
    pub unk_30: i32,
    /// +0x34: when non-zero selects the object via `FUN_100686d0`. Live: 0.
    pub unk_34: i32,
}

/// `StopFightIIR_t` [GC 0x10079daa]: header dynel stops fighting. Wire is an `i32` (non-zero = true).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopFight {
    pub flag: bool,
}

/// `MissedAttackInfoIIR_t` [GC 0x100a0a87].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissedAttackInfo {
    /// +0x1c: passed as the stat-26 value like `AttackInfo::value_20`. Live: always -1.
    pub value_1c: i32,
    /// +0x18: slot index like `AttackInfo::slot`. Live: 0..6.
    pub slot: i32,
    /// +0x20: `[GUESS]` the one that missed (equals the header dynel in all 11 live messages).
    pub source: Identity,
    /// +0x28: `[GUESS]` the one missed.
    pub target: Identity,
    /// +0x30: `Stat_e`; 0 = none, otherwise its name (`fStatToString`) goes into the combat-log text.
    pub stat: i32,
}

/// Arguments of a [`GenericCmd`] (`FUN_1003aaf4` [GC]; `ActionData_t` subclasses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericArgs {
    /// cmd 3, `ItemActionData_t`.
    Item { flag: i32, actor: Identity, item: Identity },
    /// cmd 5 / 0x20, `UseItemOnItemActionData_t`: the item action plus the item it is used on.
    ItemOnItem { flag: i32, actor: Identity, item: Identity, target: Identity },
    /// Any other cmd: the client's reader rejects it; bytes kept.
    Raw(Vec<u8>),
}

/// `GenericCmd_t` [GC 0x1007c850] = `n3Command_t::ReadSubClass` (state, seq) + cmd + args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericCmd {
    /// `n3Command_t` +0x18: 0 = request (client -> server), 1..5 are confirmations/aborts (doc). Live: 1.
    pub state: i32,
    /// `n3Command_t` +0x1c: sequence number looked up in `n3Command_t::m_cRefList`. Live: 238, 239, 240.
    pub seq: i32,
    /// +0x24 command type (`FUN_1003ab80`: 3, 5, 0x20 have arguments).
    pub cmd: i32,
    pub args: GenericArgs,
}

/// `BuffIIR_c` [GC 0x1007213b].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buff {
    /// +0x18; the apply code only acts when 0.
    pub kind: i16,
    /// Present when `kind == 0`: kind `0xCF1B` = nano program, instance = nano id (RDB record 0xFDE85).
    pub nano: Option<Identity>,
    /// Bytes after the part the client reads (non-zero `kind`: everything).
    pub rest: Vec<u8>,
}

/// `CharSecSpecAttackIIR_t` [GC 0x10072773].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharSecSpecAttack {
    pub target: Identity,
    /// `Stat_e` of the special attack (142 = "Brawl" in the client's stat table).
    pub special: i32,
}

/// `SpecialAttackInfoIIR_t` [GC 0x100a191e].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecialAttackInfo {
    /// +0x20. Live: 0.
    pub slot: i32,
    /// +0x24: subtracted from the Health (stat 27) of `target` by `FUN_1006a9c5`.
    pub damage: i32,
    /// +0x28. Live: -1.
    pub value_28: i32,
    /// +0x18.
    pub target: Identity,
    /// +0x2c: special attack `Stat_e` (142 live).
    pub special: i32,
    /// +0x30: id handed to `FUN_1005ae91` when non-zero. Live: 0.
    pub unk_30: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Misc {
    FollowTarget(FollowTarget),
    Attack(Attack),
    AttackInfo(AttackInfo),
    StopFight(StopFight),
    /// `n3ToClientQuitIIR_t` (N3.dll): no body; the client calls `n3Dynel_t::Die` on the header dynel.
    ToClientQuit,
    MissedAttackInfo(MissedAttackInfo),
    GenericCmd(GenericCmd),
    Buff(Buff),
    CharSecSpecAttack(CharSecSpecAttack),
    SpecialAttackInfo(SpecialAttackInfo),
    /// `CharInPlayIIR_t`: empty body (`n3ToServerUnBlockIIR_t::ReadSubClass`).
    CharInPlay,
}

impl Misc {
    pub fn msg_type(&self) -> u32 {
        match self {
            Misc::FollowTarget(_) => FOLLOW_TARGET,
            Misc::Attack(_) => ATTACK,
            Misc::AttackInfo(_) => ATTACK_INFO,
            Misc::StopFight(_) => STOP_FIGHT,
            Misc::ToClientQuit => TO_CLIENT_QUIT,
            Misc::MissedAttackInfo(_) => MISSED_ATTACK_INFO,
            Misc::GenericCmd(_) => GENERIC_CMD,
            Misc::Buff(_) => BUFF,
            Misc::CharSecSpecAttack(_) => CHAR_SEC_SPEC_ATTACK,
            Misc::SpecialAttackInfo(_) => SPECIAL_ATTACK_INFO,
            Misc::CharInPlay => CHAR_IN_PLAY,
        }
    }

    /// Full N3 payload (`n3InfoItemRemote_t::Write` [N3 0x100098f4]: key, identity, flag byte, body).
    pub fn encode(&self, target: Identity, flag: u8) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(self.msg_type());
        target.write(&mut w);
        w.u8(flag);
        match self {
            Misc::FollowTarget(m) => {
                if m.form == 1 {
                    w.u8(1);
                    w.u8(m.mode);
                } else {
                    w.u8(2);
                    w.u8(m.mode);
                    m.target.write(&mut w);
                    w.f32(m.speed);
                    m.pos.write(&mut w);
                }
                w.u8(m.path.len() as u8);
                m.path.iter().for_each(|p| p.write(&mut w));
            }
            Misc::Attack(m) => {
                m.target.write(&mut w);
                w.u8(m.flag as u8);
            }
            Misc::AttackInfo(m) => {
                w.i32(m.damage);
                w.i32(m.value_20);
                w.i32(m.slot);
                m.other.write(&mut w);
                w.i32(m.unk_2c);
                w.i32(m.unk_30);
                w.i32(m.unk_34);
            }
            Misc::StopFight(m) => w.i32(m.flag as i32),
            Misc::ToClientQuit | Misc::CharInPlay => {}
            Misc::MissedAttackInfo(m) => {
                w.i32(m.value_1c);
                w.i32(m.slot);
                m.source.write(&mut w);
                m.target.write(&mut w);
                w.i32(m.stat);
            }
            Misc::GenericCmd(m) => {
                w.i32(m.state);
                w.i32(m.seq);
                w.i32(m.cmd);
                match &m.args {
                    GenericArgs::Item { flag, actor, item } => {
                        w.i32(*flag);
                        actor.write(&mut w);
                        item.write(&mut w);
                    }
                    GenericArgs::ItemOnItem { flag, actor, item, target } => {
                        w.i32(*flag);
                        actor.write(&mut w);
                        item.write(&mut w);
                        target.write(&mut w);
                    }
                    GenericArgs::Raw(b) => w.bytes(b),
                }
            }
            Misc::Buff(m) => {
                w.i16(m.kind);
                if let Some(n) = &m.nano {
                    n.write(&mut w);
                }
                w.bytes(&m.rest);
            }
            Misc::CharSecSpecAttack(m) => {
                m.target.write(&mut w);
                w.i32(m.special);
            }
            Misc::SpecialAttackInfo(m) => {
                w.i32(m.slot);
                w.i32(m.damage);
                w.i32(m.value_28);
                m.target.write(&mut w);
                w.i32(m.special);
                w.i32(m.unk_30);
            }
        }
        w.0
    }
}

fn path(r: &mut Reader) -> Result<Vec<Vec3>> {
    let n = r.u8()? as usize;
    if n > 30 {
        bail!("FollowTarget path of {n} points (client keeps 30)");
    }
    (0..n).map(|_| Vec3::read(r)).collect()
}

/// Decode one of this module's messages; `Ok(None)` for other message types. `r` is the body
/// reader from [`N3Header::parse`]. Bytes the client's reader would not consume are left in `r`.
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<Misc>> {
    Ok(Some(match h.msg_type {
        FOLLOW_TARGET => {
            let form = r.u8()?;
            let mode = r.u8()?;
            if form == 1 {
                let path = path(r)?;
                let pos = path.first().copied().unwrap_or_default();
                Misc::FollowTarget(FollowTarget { form, mode, target: h.target, speed: 0.0, pos, path })
            } else {
                let target = Identity::read(r)?;
                let speed = r.f32()?;
                let pos = Vec3::read(r)?;
                Misc::FollowTarget(FollowTarget { form, mode, target, speed, pos, path: path(r)? })
            }
        }
        ATTACK => Misc::Attack(Attack { target: Identity::read(r)?, flag: r.u8()? as i8 }),
        ATTACK_INFO => {
            let damage = r.i32()?;
            let value_20 = r.i32()?;
            let slot = r.i32()?;
            Misc::AttackInfo(AttackInfo {
                damage,
                value_20,
                slot,
                other: Identity::read(r)?,
                unk_2c: r.i32()?,
                unk_30: r.i32()?,
                unk_34: r.i32()?,
            })
        }
        STOP_FIGHT => Misc::StopFight(StopFight { flag: r.i32()? != 0 }),
        TO_CLIENT_QUIT => Misc::ToClientQuit,
        MISSED_ATTACK_INFO => {
            let value_1c = r.i32()?;
            let slot = r.i32()?;
            Misc::MissedAttackInfo(MissedAttackInfo {
                value_1c,
                slot,
                source: Identity::read(r)?,
                target: Identity::read(r)?,
                stat: r.i32()?,
            })
        }
        GENERIC_CMD => {
            let (state, seq, cmd) = (r.i32()?, r.i32()?, r.i32()?);
            let args = match cmd {
                3 => GenericArgs::Item { flag: r.i32()?, actor: Identity::read(r)?, item: Identity::read(r)? },
                5 | 0x20 => GenericArgs::ItemOnItem {
                    flag: r.i32()?,
                    actor: Identity::read(r)?,
                    item: Identity::read(r)?,
                    target: Identity::read(r)?,
                },
                _ => GenericArgs::Raw(r.bytes(r.remaining())?.to_vec()),
            };
            Misc::GenericCmd(GenericCmd { state, seq, cmd, args })
        }
        BUFF => {
            let kind = r.i16()?;
            let nano = if kind == 0 { Some(Identity::read(r)?) } else { None };
            Misc::Buff(Buff { kind, nano, rest: r.bytes(r.remaining())?.to_vec() })
        }
        CHAR_SEC_SPEC_ATTACK => {
            Misc::CharSecSpecAttack(CharSecSpecAttack { target: Identity::read(r)?, special: r.i32()? })
        }
        SPECIAL_ATTACK_INFO => {
            let (slot, damage, value_28) = (r.i32()?, r.i32()?, r.i32()?);
            Misc::SpecialAttackInfo(SpecialAttackInfo {
                slot,
                damage,
                value_28,
                target: Identity::read(r)?,
                special: r.i32()?,
                unk_30: r.i32()?,
            })
        }
        CHAR_IN_PLAY => Misc::CharInPlay,
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::capture_n3;
    use std::collections::HashMap;

    fn id(kind: i32, instance: i32) -> Identity {
        Identity { kind, instance }
    }

    /// Every captured message of this module: `(sender, header, message)`; asserts the body is consumed exactly.
    fn all() -> Vec<(u32, N3Header, Misc)> {
        capture_n3()
            .iter()
            .filter_map(|f| {
                let (h, mut r) = N3Header::parse(&f.payload).unwrap();
                let m = decode(&h, &mut r).unwrap()?;
                assert_eq!(r.remaining(), 0, "{m:?}");
                assert_eq!(m.encode(h.target, h.flag), f.payload, "re-encode {m:?}");
                Some((f.sender, h, m))
            })
            .collect()
    }

    #[test]
    fn keys_are_class_name_hashes() {
        for (n, k) in [
            ("FollowTargetIIR_c", FOLLOW_TARGET),
            ("AttackIIR_t", ATTACK),
            ("AttackInfoIIR_t", ATTACK_INFO),
            ("StopFightIIR_t", STOP_FIGHT),
            ("n3ToClientQuitIIR_t", TO_CLIENT_QUIT),
            ("MissedAttackInfoIIR_t", MISSED_ATTACK_INFO),
            ("GenericCmd_t", GENERIC_CMD),
            ("BuffIIR_c", BUFF),
            ("CharSecSpecAttackIIR_t", CHAR_SEC_SPEC_ATTACK),
            ("SpecialAttackInfoIIR_t", SPECIAL_ATTACK_INFO),
            ("CharInPlayIIR_t", CHAR_IN_PLAY),
            ("PlayfieldAnarchyFIIR_t", 0x5F4B_1A39),
            ("CharDCMoveIIR_t", 0x5411_1123),
        ] {
            assert_eq!(key(n), k, "{n}");
        }
    }

    #[test]
    fn captured_counts() {
        let mut c: HashMap<u32, usize> = HashMap::new();
        all().iter().for_each(|(_, _, m)| *c.entry(m.msg_type()).or_default() += 1);
        let want = [
            (FOLLOW_TARGET, 173),
            (ATTACK, 128),
            (ATTACK_INFO, 134),
            (STOP_FIGHT, 128),
            (TO_CLIENT_QUIT, 27),
            (MISSED_ATTACK_INFO, 11),
            (GENERIC_CMD, 3),
            (BUFF, 3),
            (CHAR_SEC_SPEC_ATTACK, 3),
            (SPECIAL_ATTACK_INFO, 3),
            (CHAR_IN_PLAY, 2),
        ];
        assert_eq!(c.len(), want.len());
        for (k, n) in want {
            assert_eq!(c[&k], n, "{k:08X}");
        }
    }

    #[test]
    fn follow_target() {
        let v: Vec<_> = all()
            .into_iter()
            .filter_map(|(_, h, m)| if let Misc::FollowTarget(f) = m { Some((h, f)) } else { None })
            .collect();
        let (h, f) = &v[0];
        assert_eq!((h.target, h.flag), (id(0xC350, 0xFA8DD), 1));
        assert_eq!((f.form, f.mode, f.path.len(), f.target, f.speed), (1, 25, 2, h.target, 0.0));
        assert_eq!(f.pos, Vec3 { x: 860.8, y: f32::from_bits(0x4211_daf6), z: 829.1 });
        assert_eq!(f.path[1], Vec3 { x: 873.1, y: f32::from_bits(0x4214_f8e1), z: 823.2 });
        // 107 short forms (modes 25 x79, 24 x28; 2..9 points), 66 long forms (mode 21, no target, speed 0, one point)
        assert_eq!(v.iter().filter(|(_, f)| f.form == 1).count(), 107);
        assert_eq!(v.iter().filter(|(_, f)| f.form == 1 && f.mode == 25).count(), 79);
        assert_eq!(v.iter().filter(|(_, f)| f.form == 1 && f.mode == 24).count(), 28);
        assert_eq!(v.iter().map(|(_, f)| f.path.len()).max(), Some(9));
        let long: Vec<_> = v.iter().filter(|(_, f)| f.form == 2).collect();
        assert_eq!(long.len(), 66);
        assert!(long.iter().all(|(_, f)| f.mode == 21 && f.target == id(0, 0) && f.speed == 0.0 && f.path == [f.pos]));
        let (_, l) = long[0];
        assert_eq!(l.pos, Vec3 { x: 896.4, y: f32::from_bits(0x4215_00e6), z: 829.0 });
        assert!(v.iter().all(|(h, _)| h.target.kind == 0xC350));
    }

    #[test]
    fn combat() {
        let a = all();
        let attacks: Vec<_> = a.iter().filter_map(|(s, h, m)| if let Misc::Attack(x) = m { Some((*s, h, x)) } else { None }).collect();
        let (s, h, x) = attacks[0];
        assert_eq!((s, h.target, h.flag, x.target, x.flag), (3503, id(0xC350, 0xF4A4B), 0, id(0xC350, 0xFA8C9), 0));
        assert!(attacks.iter().all(|(_, _, x)| x.flag == 0));

        let infos: Vec<_> = a.iter().filter_map(|(s, h, m)| if let Misc::AttackInfo(x) = m { Some((*s, h, x)) } else { None }).collect();
        let (s, h, x) = infos[0];
        assert_eq!(s, 1026263);
        assert_eq!(h.target, id(0xC350, 0xFA8D7));
        assert_eq!(
            *x,
            AttackInfo { damage: 17, value_20: -1, slot: 2, other: id(0xC350, 0xF4A4C), unk_2c: 0, unk_30: 3, unk_34: 0 }
        );
        assert_eq!(infos.iter().map(|(_, _, x)| x.damage).max(), Some(99));
        assert_eq!(infos.iter().map(|(_, _, x)| x.damage).min(), Some(8));
        assert!(infos.iter().all(|(_, _, x)| x.value_20 == -1 && x.unk_34 == 0 && x.slot <= 6 && x.other.kind == 0xC350));

        let stops: Vec<_> = a.iter().filter(|(_, _, m)| matches!(m, Misc::StopFight(StopFight { flag: true }))).collect();
        assert_eq!(stops.len(), 128);

        let miss: Vec<_> = a.iter().filter_map(|(s, h, m)| if let Misc::MissedAttackInfo(x) = m { Some((*s, h, x)) } else { None }).collect();
        let (s, h, x) = miss[0];
        assert_eq!(s, 3502);
        assert_eq!(h.target, id(0xC350, 0xFA885));
        assert_eq!(
            *x,
            MissedAttackInfo { value_1c: -1, slot: 2, source: id(0xC350, 0xFA885), target: id(0xC350, 0xFA143), stat: 0 }
        );
        assert!(miss.iter().all(|(_, h, x)| x.source == h.target && x.value_1c == -1 && x.stat == 0));
    }

    #[test]
    fn special_attacks() {
        let a = all();
        let sec: Vec<_> = a.iter().filter_map(|(_, h, m)| if let Misc::CharSecSpecAttack(x) = m { Some((h, x)) } else { None }).collect();
        assert_eq!(sec.len(), 3);
        assert!(sec.iter().all(|(h, x)| h.target == id(0xC350, 0x827A) && x.special == 142));
        assert_eq!(sec[0].1.target, id(0xC350, 0xFA8EA));
        assert_eq!(sec[2].1.target, id(0xC350, 0xFA513));
        let info: Vec<_> = a.iter().filter_map(|(_, _, m)| if let Misc::SpecialAttackInfo(x) = m { Some(x.clone()) } else { None }).collect();
        assert_eq!(
            info[0],
            SpecialAttackInfo { slot: 0, damage: 5, value_28: -1, target: id(0xC350, 0xFA8EA), special: 142, unk_30: 0 }
        );
        assert_eq!(info.iter().map(|x| x.damage).collect::<Vec<_>>(), [5, 20, 11]);
    }

    #[test]
    fn generic_cmd_buff_quit_in_play() {
        let a = all();
        let cmds: Vec<_> = a.iter().filter_map(|(s, h, m)| if let Misc::GenericCmd(x) = m { Some((*s, h, x)) } else { None }).collect();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds.iter().map(|(_, _, x)| x.seq).collect::<Vec<_>>(), [238, 239, 240]);
        let (s, h, x) = cmds[0];
        assert_eq!((s, h.target), (3504, id(0xC350, 0x827A)));
        assert_eq!(
            *x,
            GenericCmd {
                state: 1,
                seq: 238,
                cmd: 3,
                args: GenericArgs::Item { flag: 1, actor: id(0xC350, 0x827A), item: id(0xC76A, 0x161F) },
            }
        );
        assert_eq!(
            cmds[2].2.args,
            GenericArgs::Item { flag: 1, actor: id(0xC350, 0x827A), item: id(0xC76A, 0x163B) }
        );

        let buffs: Vec<_> = a.iter().filter_map(|(s, h, m)| if let Misc::Buff(x) = m { Some((*s, h, x)) } else { None }).collect();
        assert_eq!(buffs.len(), 3);
        for (s, h, x) in buffs {
            assert_eq!((s, h.target), (33402, id(0xC350, 0x827A)));
            assert_eq!(*x, Buff { kind: 0, nano: Some(id(0xCF1B, 163449)), rest: vec![] });
        }

        let quits: Vec<_> = a.iter().filter(|(_, _, m)| *m == Misc::ToClientQuit).map(|(_, h, _)| h.target.kind).collect();
        assert_eq!((quits.iter().filter(|&&k| k == 0xC350).count(), quits.iter().filter(|&&k| k == 0xC76A).count()), (18, 9));

        let play: Vec<_> = a.iter().filter(|(_, _, m)| *m == Misc::CharInPlay).map(|(s, h, _)| (*s, h.target, h.flag)).collect();
        assert_eq!(play, [(33402, id(0xC350, 0x827A), 1), (33491, id(0xC350, 0x82D3), 1)]);
    }

    #[test]
    fn other_types_and_malformed() {
        let other = [&0x5F4B1A39u32.to_be_bytes()[..], &[0; 9]].concat();
        let (h, mut r) = N3Header::parse(&other).unwrap();
        assert_eq!(decode(&h, &mut r).unwrap(), None);
        // every truncation of every captured message errors instead of panicking
        for f in capture_n3() {
            let (h, _) = N3Header::parse(&f.payload).unwrap();
            if !matches!(h.msg_type, FOLLOW_TARGET | ATTACK | ATTACK_INFO | STOP_FIGHT | MISSED_ATTACK_INFO | GENERIC_CMD | BUFF | CHAR_SEC_SPEC_ATTACK | SPECIAL_ATTACK_INFO) {
                continue;
            }
            for cut in 13..f.payload.len() {
                let (h, mut r) = N3Header::parse(&f.payload[..cut]).unwrap();
                assert!(decode(&h, &mut r).is_err(), "{:08X} cut {cut}", h.msg_type);
            }
        }
        // path longer than the client's 30 slots
        let mut bad = Misc::FollowTarget(FollowTarget {
            form: 1,
            mode: 25,
            target: id(0xC350, 1),
            speed: 0.0,
            pos: Vec3::default(),
            path: vec![Vec3::default(); 31],
        })
        .encode(id(0xC350, 1), 0);
        let (h, mut r) = N3Header::parse(&bad).unwrap();
        assert!(decode(&h, &mut r).is_err());
        bad.truncate(14);
        let (h, mut r) = N3Header::parse(&bad).unwrap();
        assert!(decode(&h, &mut r).is_err());
    }
}
