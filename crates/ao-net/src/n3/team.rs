//! Team and perk messages of the zone server and the client's team / perk request senders (docs/gui.md §11.13).
//!
//! Server messages (bodies after the 13-byte N3 header, `N3::Unknown`): `TeamMemberIIR_t`, `TeamMemberInfoIIR_t`, `TeamInviteIIR_t`,
//! `PerkUpdateIIR`, plus the perk-map block of `FullCharacterIIR_t`. Client requests are `CharacterActionIIR_t`s built exactly like
//! [`super::action::reset_skill`] (`FUN_1007253f(hdr, identity_a, param, action, identity_b, "")`, sent with `SendIIRToObservers`).
//! None of the server messages is in a live capture (the test account was never in a team and knows no perk): the tests build the frames
//! with the encoders below from the layouts read in Gamecode.dll.

use super::action::{character_action, simple};
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `TeamMemberIIR_t` (vtable [GC 0x10161670], read `FUN_10079fa1`, apply `FUN_1007a033`).
pub const TEAM_MEMBER: u32 = 0x46312D2E;
/// `TeamMemberInfoIIR_t` (vtable [GC 0x10161698], read `FUN_1007a1fc`, apply `FUN_1007a294`).
pub const TEAM_MEMBER_INFO: u32 = 0x28784248;
/// `TeamInviteIIR_t` (vtable [GC 0x10166f2c], read `FUN_100a4002`, apply `FUN_100a3f26`).
pub const TEAM_INVITE: u32 = 0x4D2A313B;
/// `PerkUpdateIIR` (vtable [GC 0x10155f74], read `FUN_10061f79`, apply `FUN_10063942`).
pub const PERK_UPDATE: u32 = 0x435F7023;

/// `CharacterActionIIR_t` action ids of the team / perk senders and the team relays (docs/zone/actions.md).
pub mod action {
    /// `N3Msg_KickTeamMember` [GC 0x1001b717]: `identity_a` = the member.
    pub const KICK: i32 = 0x16;
    /// `N3Msg_LeaveTeam` [GC 0x1001b7ca]; the relay clears the team of the acted-on dynel (case 4 of `FUN_1005d0d8`).
    pub const LEAVE: i32 = 0x18;
    /// `N3Msg_TransferTeamLeadership` [GC 0x1001b876]: `identity_a` = the new leader.
    pub const TRANSFER: i32 = 0x19;
    /// `N3Msg_TeamJoinRequest` [GC 0x1001b929]: `identity_a` = the invited / accepted character, `identity_b = {0, bool}`.
    pub const JOIN_REQUEST: i32 = 0x1a;
    /// Received: a member leaves (case 8 of `FUN_1005d0d8`: `FUN_10065e1b(identity_a, 0, ..)` removes the entry, then the dynel's team is cleared).
    pub const MEMBER_LEFT: i32 = 0x20;
    /// Received: leader changed (case 9: the dynel's team is cleared, `FUN_10065718` stores the identity as the team leader).
    pub const LEADER: i32 = 0x23;
    /// `N3Msg_RequestReply(who, accept)` [GC 0x1001b9ff]: the answer to a request dialog (team invitation): `a = who`, `b = {0, accept}`.
    pub const REQUEST_REPLY: i32 = 0x1c;
    /// `n3EngineClientAnarchy_t::N3Msg_TrainPerk` [GC 0x1002757f] -> `FUN_10052ede`.
    pub const TRAIN_PERK: i32 = 0xbb;
    /// `N3Msg_UntrainPerk` [GC 0x100275a4] -> `FUN_10052f8e`.
    pub const UNTRAIN_PERK: i32 = 0xbc;
}

/// Identity kind of a team (`FUN_100657d1`: `0xdea9` is mapped to `0xde01`; the own team record is `{0xDEA9, id}`).
pub const TEAM_KIND: i32 = 0xDEA9;

/// `TeamMemberIIR_t`: one member joined / was described. Fields in read order (`this+0x18 Identity`, `+0x20 Identity`, `+0x28 i32`,
/// `+0x30 i32`, `+0x34 i16`, `+0x2c` = `i32` length (< 101) + bytes). The names follow the consumers (INFERENCE): `FUN_1007a033` applies
/// `team` to the dynel `member` (`FUN_10065745` stores the identity, instance 0 = no team) and, on the header dynel, calls
/// `FUN_10065e1b(member, 1, name, +0x28, +0x30, +0x34)` which creates the `TeamEntry_t` (identity at +0x1c, side byte, u16 at +0x2a, int at +0x2c);
/// `RaidViewModule_c::SlotTeamMemberJoined(Identity, name, int, Profession_e, int)` has the same order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamMember {
    pub member: Identity,
    pub team: Identity,
    /// `+0x28`: the sub-team (raid team) index; for the own identity it becomes the own sub-team (`team+0x38`).
    pub sub_team: i32,
    /// `+0x30`: `Profession_e`.
    pub profession: i32,
    /// `+0x34`.
    pub level: u16,
    pub name: String,
}

/// `TeamMemberInfoIIR_t`: health / nano of a member out of sight; the apply sets stat `0xdd` = `max_nano`, `0xd6` = `nano`, `1` = `max_health`,
/// `0x1b` = `health` on the dynel (`FUN_1007a294`), which the team bars read (`FUN_100775ac`: `GetSkill(0x1b)/GetSkill(1)`, `(0xd6)/(0xdd)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamMemberInfo {
    pub member: Identity,
    pub nano: i32,
    pub max_nano: i32,
    pub max_health: i32,
    pub health: i32,
}

/// `TeamInviteIIR_t`: `Identity`, `u8 flag`, string (`u16` length + bytes), and for `flag != 0` four more fields
/// (`i32`, `u8`, `i16`, `u16`; the "too low / too high" refusals, `TeamViewModule_c::SlotJoinTeamRequestFailed*`).
/// The apply (`FUN_100a3f26`) acts for `flag == 0` only: the invitation dialog (signal `GlobalSignals+0x124`) unless the own character is already in a team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamInvite {
    pub from: Identity,
    pub flag: u8,
    pub name: String,
    /// `(i32, u8, i16, u16)` of the refusal form.
    pub refusal: Option<(i32, u8, i16, u16)>,
}

/// `PerkUpdateIIR`: `i32` perk id (`+0x18`), `i32` (`+0x1c`, stored into stat `0x109`), `i32` remaining (`+0x20`, `FUN_10052842` clamps negatives to 0):
/// the perk entry's timer; the progress is `(total - remaining) / total` with the catalogue's `+0x18` total (`FUN_10063942`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerkUpdate {
    pub perk: i32,
    pub stat_109: i32,
    pub remaining: i32,
}

/// One entry of the own perk map in `FullCharacterIIR_t` (`FUN_100537a5` -> `FUN_10052859`): the perk id and its timer (`entry+0x1c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownPerk {
    pub perk: i32,
    pub timer: i32,
}

fn done(r: &Reader, what: &str) -> Result<()> {
    if r.remaining() != 0 {
        bail!("{} trailing bytes after {what}", r.remaining());
    }
    Ok(())
}

pub fn parse_team_member(body: &[u8]) -> Result<TeamMember> {
    let mut r = Reader::new(body);
    let (member, team) = (Identity::read(&mut r)?, Identity::read(&mut r)?);
    let (sub_team, profession, level) = (r.i32()?, r.i32()?, r.u16()?);
    // `FUN_10079fa1`: a length above 100 ends the read without the string (and reports success); we refuse it
    let name = r.str_i32(100)?;
    done(&r, "TeamMember")?;
    Ok(TeamMember { member, team, sub_team, profession, level, name })
}

pub fn parse_team_member_info(body: &[u8]) -> Result<TeamMemberInfo> {
    let mut r = Reader::new(body);
    let m = TeamMemberInfo { member: Identity::read(&mut r)?, nano: r.i32()?, max_nano: r.i32()?, max_health: r.i32()?, health: r.i32()? };
    done(&r, "TeamMemberInfo")?;
    Ok(m)
}

pub fn parse_team_invite(body: &[u8]) -> Result<TeamInvite> {
    let mut r = Reader::new(body);
    let (from, flag, name) = (Identity::read(&mut r)?, r.u8()?, r.str_i16()?);
    let refusal = if flag != 0 { Some((r.i32()?, r.u8()?, r.i16()?, r.u16()?)) } else { None };
    done(&r, "TeamInvite")?;
    Ok(TeamInvite { from, flag, name, refusal })
}

pub fn parse_perk_update(body: &[u8]) -> Result<PerkUpdate> {
    let mut r = Reader::new(body);
    let m = PerkUpdate { perk: r.i32()?, stat_109: r.i32()?, remaining: r.i32()? };
    done(&r, "PerkUpdate")?;
    Ok(m)
}

/// The perk-map block of `FullCharacterIIR_t` (`FUN_100537a5`): a size word `(n + 1) * 0x3f1`, then `n` entries of `i32 key` and the entry
/// (`FUN_10052859`: `u32 a`; when `a & 0xffffff00 == 0xffffff00` its low byte is the entry version, followed by `i32 id` and `i32 timer`
/// (+0x1c), and only version 1 adds three more ints `FUN_1002ba8c`; otherwise `a` is the id and the three ints follow). `rest` starts at that
/// size word (`FullCharacter::rest` when the spell list before it is empty). Refuses anything that does not fit (spell lists start the same way).
pub fn parse_perk_map(rest: &[u8]) -> Result<Vec<KnownPerk>> {
    let mut r = Reader::new(rest);
    let size = r.i32()?;
    if size <= 0 || size % 0x3f1 != 0 {
        bail!("inconsistent PerkMap size word {size:#x}");
    }
    let n = size / 0x3f1 - 1;
    if !(0..1000).contains(&n) {
        bail!("PerkMap entry count {n} out of range");
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let key = r.i32()?;
        let a = r.u32()?;
        let (id, timer) = if a & 0xffff_ff00 == 0xffff_ff00 {
            let id = r.i32()?;
            let timer = r.i32()?;
            if a & 0xff == 1 {
                r.bytes(12)?;
            }
            (id, timer)
        } else {
            r.bytes(12)?;
            (a as i32, 0)
        };
        // `FUN_100537a5` inserts the entry only when the id it carries equals the key
        if id == key {
            out.push(KnownPerk { perk: id, timer });
        }
    }
    Ok(out)
}

fn header(msg: u32, target: Identity) -> Writer {
    let mut w = Writer::default();
    w.u32(msg);
    target.write(&mut w);
    w.u8(1);
    w
}

/// Full N3 payload of a `TeamMemberIIR_t` for header dynel `target` (tests, server emulation).
pub fn team_member(target: Identity, m: &TeamMember) -> Vec<u8> {
    let mut w = header(TEAM_MEMBER, target);
    m.member.write(&mut w);
    m.team.write(&mut w);
    w.i32(m.sub_team);
    w.i32(m.profession);
    w.u16(m.level);
    w.str_i32(&m.name);
    w.0
}

pub fn team_member_info(target: Identity, m: &TeamMemberInfo) -> Vec<u8> {
    let mut w = header(TEAM_MEMBER_INFO, target);
    m.member.write(&mut w);
    for v in [m.nano, m.max_nano, m.max_health, m.health] {
        w.i32(v);
    }
    w.0
}

pub fn team_invite(target: Identity, m: &TeamInvite) -> Vec<u8> {
    let mut w = header(TEAM_INVITE, target);
    m.from.write(&mut w);
    w.u8(m.flag);
    w.str_i16(&m.name);
    if let Some((a, b, c, d)) = m.refusal {
        w.i32(a);
        w.u8(b);
        w.i16(c);
        w.u16(d);
    }
    w.0
}

pub fn perk_update(target: Identity, m: &PerkUpdate) -> Vec<u8> {
    let mut w = header(PERK_UPDATE, target);
    for v in [m.perk, m.stat_109, m.remaining] {
        w.i32(v);
    }
    w.0
}

/// The perk-map block bytes for `perks` in the version-1 form (what the server's own encoder is assumed to emit; tests only).
pub fn perk_map(perks: &[KnownPerk]) -> Vec<u8> {
    let mut w = Writer::default();
    w.i32((perks.len() as i32 + 1) * 0x3f1);
    for p in perks {
        w.i32(p.perk);
        w.u32(0xffff_ff01);
        w.i32(p.perk);
        w.i32(p.timer);
        w.bytes(&[0; 12]);
    }
    w.0
}

/// `N3Msg_KickTeamMember(member)`: `FUN_1007253f(hdr, a = member, 0, 0x16, b = {0,0}, "")`; the client skips it when `member` is its own identity.
pub fn kick_team_member(char_id: i32, member: Identity) -> Vec<u8> {
    character_action(char_id, &simple(action::KICK, member, Identity::default()))
}

/// `N3Msg_LeaveTeam()`: action `0x18`, both identities zero.
pub fn leave_team(char_id: i32) -> Vec<u8> {
    character_action(char_id, &simple(action::LEAVE, Identity::default(), Identity::default()))
}

/// `N3Msg_TransferTeamLeadership(member)`: action `0x19`, `a = member`; skipped for the own identity.
pub fn transfer_team_leadership(char_id: i32, member: Identity) -> Vec<u8> {
    character_action(char_id, &simple(action::TRANSFER, member, Identity::default()))
}

/// `N3Msg_TeamJoinRequest(who, accept)`: action `0x1a`, `a = who`, `b = {0, accept}`.
pub fn team_join_request(char_id: i32, who: Identity, accept: bool) -> Vec<u8> {
    character_action(char_id, &simple(action::JOIN_REQUEST, who, Identity { kind: 0, instance: accept as i32 }))
}

/// `N3Msg_RequestReply(who, accept)`: action `0x1c`.
pub fn request_reply(char_id: i32, who: Identity, accept: bool) -> Vec<u8> {
    character_action(char_id, &simple(action::REQUEST_REPLY, who, Identity { kind: 0, instance: accept as i32 }))
}

/// `N3Msg_TrainPerk(perk)`: action `0xbb`, `a = {0,0}`, `b = {0, perk}` (`FUN_10052ede`).
pub fn train_perk(char_id: i32, perk: i32) -> Vec<u8> {
    character_action(char_id, &simple(action::TRAIN_PERK, Identity::default(), Identity { kind: 0, instance: perk }))
}

/// `N3Msg_UntrainPerk(perk)`: action `0xbc` (`FUN_10052f8e`), same shape.
pub fn untrain_perk(char_id: i32, perk: i32) -> Vec<u8> {
    character_action(char_id, &simple(action::UNTRAIN_PERK, Identity::default(), Identity { kind: 0, instance: perk }))
}

/// The class names behind the ids (registry check in the tests).
pub fn class_keys() -> [(&'static str, u32); 4] {
    [("TeamMemberIIR_t", TEAM_MEMBER), ("TeamMemberInfoIIR_t", TEAM_MEMBER_INFO), ("TeamInviteIIR_t", TEAM_INVITE), ("PerkUpdateIIR", PERK_UPDATE)]
}

#[cfg(test)]
mod tests {
    use super::super::action::parse_character_action;
    use super::super::outgoing::message_key;
    use super::*;

    const ME: Identity = Identity { kind: 50000, instance: 25988 };
    const OTHER: Identity = Identity { kind: 50000, instance: 4242 };

    #[test]
    fn ids_are_the_class_name_keys() {
        for (name, id) in class_keys() {
            assert_eq!(message_key(name), id, "{name}");
        }
    }

    #[test]
    fn team_member_round_trip_and_bounds() {
        let m = TeamMember { member: OTHER, team: Identity { kind: TEAM_KIND, instance: 99 }, sub_team: 0, profession: 6, level: 37, name: "Aomactest".into() };
        let p = team_member(ME, &m);
        assert_eq!(&p[..4], &TEAM_MEMBER.to_be_bytes());
        assert_eq!(parse_team_member(&p[13..]).unwrap(), m);
        assert!(parse_team_member(&p[13..p.len() - 1]).is_err());
        assert!(parse_team_member(&[p.clone(), vec![0]].concat()[13..]).is_err());
        // an over-long name is refused (the original stops reading there)
        let long = TeamMember { name: "x".repeat(101), ..m };
        assert!(parse_team_member(&team_member(ME, &long)[13..]).is_err());
    }

    #[test]
    fn team_member_info_round_trip() {
        let m = TeamMemberInfo { member: OTHER, nano: 120, max_nano: 400, max_health: 900, health: 450 };
        let p = team_member_info(ME, &m);
        assert_eq!(parse_team_member_info(&p[13..]).unwrap(), m);
        assert!(parse_team_member_info(&p[13..p.len() - 2]).is_err());
    }

    #[test]
    fn team_invite_both_forms() {
        let ask = TeamInvite { from: OTHER, flag: 0, name: "Inviter".into(), refusal: None };
        assert_eq!(parse_team_invite(&team_invite(ME, &ask)[13..]).unwrap(), ask);
        let no = TeamInvite { from: OTHER, flag: 1, name: "Inviter".into(), refusal: Some((12, 3, -4, 55)) };
        assert_eq!(parse_team_invite(&team_invite(ME, &no)[13..]).unwrap(), no);
        // a refusal form without its tail is truncated
        let p = team_invite(ME, &ask);
        let mut cut = p[13..].to_vec();
        cut[8] = 1;
        assert!(parse_team_invite(&cut).is_err());
    }

    #[test]
    fn perk_update_and_map() {
        let u = PerkUpdate { perk: 211, stat_109: 3, remaining: 7200 };
        assert_eq!(parse_perk_update(&perk_update(ME, &u)[13..]).unwrap(), u);
        let known = [KnownPerk { perk: 100, timer: 0 }, KnownPerk { perk: 101, timer: 3600 }];
        assert_eq!(parse_perk_map(&perk_map(&known)).unwrap(), known);
        assert_eq!(parse_perk_map(&perk_map(&[])).unwrap(), []);
        // old entry form: the first word is the id, three ints follow, no timer
        let mut w = Writer::default();
        w.i32(2 * 0x3f1);
        w.i32(7);
        w.i32(7);
        w.bytes(&[0; 12]);
        assert_eq!(parse_perk_map(&w.0).unwrap(), [KnownPerk { perk: 7, timer: 0 }]);
        // a wrong size word, a huge count and a truncated entry are errors, an entry whose id differs from its key is dropped
        assert!(parse_perk_map(&[0, 0, 0, 5]).is_err());
        assert!(parse_perk_map(&(0x7fff_f000i32 - 0x7fff_f000 % 0x3f1).to_be_bytes()).is_err());
        assert!(parse_perk_map(&perk_map(&known)[..20]).is_err());
        let mut w = Writer::default();
        w.i32(2 * 0x3f1);
        w.i32(8);
        w.i32(7);
        w.bytes(&[0; 12]);
        assert_eq!(parse_perk_map(&w.0).unwrap(), []);
    }

    #[test]
    fn request_encoders_are_character_actions() {
        let check = |p: Vec<u8>, action: i32, a: Identity, b: Identity| {
            let (h, c) = parse_character_action(&p).unwrap();
            assert_eq!(h.target, Identity { kind: 0xC350, instance: 25988 });
            assert_eq!((c.action, c.param, c.identity_a, c.identity_b, c.text.as_str()), (action, 0, a, b, ""));
        };
        let zero = Identity::default();
        check(kick_team_member(25988, OTHER), 0x16, OTHER, zero);
        check(leave_team(25988), 0x18, zero, zero);
        check(transfer_team_leadership(25988, OTHER), 0x19, OTHER, zero);
        check(team_join_request(25988, OTHER, false), 0x1a, OTHER, zero);
        check(team_join_request(25988, OTHER, true), 0x1a, OTHER, Identity { kind: 0, instance: 1 });
        check(request_reply(25988, OTHER, true), 0x1c, OTHER, Identity { kind: 0, instance: 1 });
        check(train_perk(25988, 211), 0xbb, zero, Identity { kind: 0, instance: 211 });
        check(untrain_perk(25988, 211), 0xbc, zero, Identity { kind: 0, instance: 211 });
        // byte level: key of CharacterActionIIR_t, header {0xC350, id}, flag 0, action, param, identities, empty text
        let p = leave_team(25988);
        assert_eq!(&p[..4], &message_key("CharacterActionIIR_t").to_be_bytes());
        assert_eq!(p.len(), 13 + 4 + 4 + 8 + 8 + 2);
        assert_eq!(&p[13..17], &0x18i32.to_be_bytes());
    }
}
