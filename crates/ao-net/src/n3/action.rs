//! Character actions, sit/stand/camp and emotes. Layouts, id tables and evidence: docs/zone/actions.md.
//!
//! Two N3 messages carry them:
//! * `CharacterActionIIR_t` (`5E477770`): the generic action carrier, `FUN_1007253f` [GC] builds it for ~60 `N3Msg_*`
//!   senders; the zone server relays some of them (live: `0x62`, `0x63`, `0xA7`, `0xAD`) and the client applies them with
//!   `FUN_1005d0d8` [GC] (a 106-way switch on the action id, byte table [GC 0x1005efdf], jump table [GC 0x1005ee43]).
//! * `SocialActionCmd_t` (`3B290771`, an `n3Command_t`): the `/<emote>` command sent by `N3Msg_DoSocialAction`.

use super::outgoing::{message_key, DYNEL_CHAR};
use super::world::{self, CharacterAction, World};
use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `SocialActionCmd_t` (`n3Command_t` subclass, registered by `FUN_1000e5f2` [GC], ctor `FUN_1007aab7` [GC]).
pub const SOCIAL_ACTION_CMD: u32 = 0x3B29_0771;

/// Action ids with a known meaning (sender or receiver code; docs/zone/actions.md §2).
pub mod id {
    /// Server relay of "sit down": the receiver runs movement transition `0x1e` (`SwitchToSitGroundMode`) [GC 0x1005d723].
    pub const SIT_RELAY: i32 = 0x56;
    /// Client sends this to stand up (from sleep/lounge/sit-on-item/sit-ground); relayed back: the receiver runs the
    /// `Leave*Mode` transition chosen by stat `WaitState` [GC 0x1005d72f].
    pub const STAND_UP: i32 = 0x57;
    /// Client sends this (`identity_a` = the item's identity) to sit on the selected item (`N3Msg_SitToggle` [GC 0x10028e0a]).
    pub const SIT_ON_ITEM: i32 = 0x55;
    /// `N3Msg_StartCamping` [GC 0x1001c93d].
    pub const START_CAMPING: i32 = 0x78;
    /// `N3Msg_StopCamping` [GC 0x1001cada].
    pub const STOP_CAMPING: i32 = 0x79;
}

/// Stat ids read by the sit logic (`docs/zone/actions.md` §3).
pub mod stat {
    /// `Can` (item flags); bit 1 (value 2) is tested to decide whether an item can be sat on.
    pub const CAN: i32 = 0x1e;
    /// `GmLevel`: non-zero skips the "must sit to camp" check in `N3Msg_StartCamping`.
    pub const GM_LEVEL: i32 = 0xd7;
    /// `IsFightingMe`: `> 0` blocks camping ("Feedback_CantLogOutInAFight").
    pub const IS_FIGHTING_ME: i32 = 0x19a;
    /// `WaitState` (stat 430): 1 = sitting on an item, 0xf = sleeping, 0x10 = lounging (as used by SitToggle and action 0x57).
    pub const WAIT_STATE: i32 = 0x1ae;
}

/// `WaitState` values the client distinguishes.
pub mod wait {
    pub const ON_ITEM: i32 = 1;
    pub const SLEEP: i32 = 0xf;
    pub const LOUNGE: i32 = 0x10;
}

/// `Vehicle_t`+0x178 FSM current-state ids (`FSM+4`) compared in the client: 4 swim, 8 sit ground, 9 [INFERENCE: sit on chair],
/// 0xb / 0xc [INFERENCE: sleep / lounge]. docs/zone/actions.md §3.
pub mod mode {
    pub const SWIM: i32 = 4;
    pub const SIT_GROUND: i32 = 8;
    pub const SIT_CHAIR: i32 = 9;
    pub const SLEEP: i32 = 0xb;
    pub const LOUNGE: i32 = 0xc;
}

/// Movement transition ids (`CharDCMove` `move_type`, and the argument of FSM `vtable[6]`): `FUN_1006c60f` [GC] creates one
/// `*TransitionAction_t` per id. Names are the class names.
pub mod mv {
    pub const SWITCH_TO_SIT_GROUND: u8 = 0x1e;
    pub const SWITCH_TO_CRAWL: u8 = 0x1b;
    pub const SWITCH_TO_SLEEP: u8 = 0x21;
    pub const SWITCH_TO_LOUNGE: u8 = 0x22;
    pub const LEAVE_CRAWL: u8 = 0x28;
    pub const LEAVE_SIT: u8 = 0x25;
    pub const LEAVE_SLEEP: u8 = 0x29;
    pub const LEAVE_LOUNGE: u8 = 0x2a;
}

/// Class name of a movement transition id (`FUN_1006c60f` [GC]; ids 6, 11, 19, 20, 22, 31, 32 have no class of their own).
pub fn transition_name(id: u8) -> Option<&'static str> {
    Some(match id {
        1 => "ForwardStart",
        2 => "ForwardStop",
        3 => "ReverseStart",
        4 => "ReverseStop",
        5 => "StrafeRightStart",
        7 => "StrafeLeftStart",
        8 => "StrafeStop",
        9 => "TurnRightStart",
        10 => "MouseTurnRightStart",
        12 => "TurnLeftStart",
        13 => "MouseTurnLeftStart",
        14 => "TurnStop",
        15 => "JumpStart",
        16 => "JumpStop",
        17 => "ElevateUpStart",
        18 => "ElevateUpStop",
        21 => "FullStop",
        23 => "SwitchToFrozenMode",
        24 => "SwitchToWalkMode",
        25 => "SwitchToRunMode",
        26 => "SwitchToSwimMode",
        27 => "SwitchToCrawlMode",
        28 => "SwitchToSneakMode",
        29 => "SwitchToFlyMode",
        30 => "SwitchToSitGroundMode",
        33 => "SwitchToSleepMode",
        34 => "SwitchToLoungeMode",
        35 => "LeaveSwimMode",
        36 => "LeaveSneakMode",
        37 => "LeaveSitMode",
        38 => "LeaveFrozenMode",
        39 => "LeaveFlyMode",
        40 => "LeaveCrawlMode",
        41 => "LeaveSleepMode",
        42 => "LeaveLoungeMode",
        _ => return None,
    })
}

/// Client-sent action ids: `(id, N3Msg_* sender or text command)`; `FUN_` names are unnamed helpers (docs/zone/actions.md §2).
pub static SENT: &[(i32, &str)] = &[
    (0x013, "CastNanoSpell"),
    (0x016, "KickTeamMember"),
    (0x018, "LeaveTeam"),
    (0x019, "TransferTeamLeadership"),
    (0x01a, "TeamJoinRequest"),
    (0x01c, "RequestReply"),
    (0x024, "text cmd (?)"),
    (0x034, "SplitItem"),
    (0x035, "JoinItems"),
    (0x036, "HideAgainstOpponent"),
    (0x039, "UseItem"),
    (0x041, "RemoveBuff"),
    (0x042, "FUN_1003fa2c"),
    (0x046, "UseSkill"),
    (0x051, "TradeskillCombine"),
    (0x055, "SitOnItem"),
    (0x057, "StandUp"),
    (0x05c, "text cmd (?)"),
    (0x067, "text cmd damagemult"),
    (0x069, "FUN_1003f5fc"),
    (0x06a, "UseItem(2)"),
    (0x06c, "FUN_1004fcf9"),
    (0x06d, "ToggleReclaim(on)"),
    (0x06e, "ToggleReclaim(off)"),
    (0x070, "DeleteItem"),
    (0x076, "FUN_10068d6b"),
    (0x078, "StartCamping"),
    (0x079, "StopCamping"),
    (0x07f, "text cmd (?)"),
    (0x080, "FUN_10065bc5"),
    (0x085, "text cmd played"),
    (0x086, "text cmd (?)"),
    (0x08e, "FUN_1003f692"),
    (0x091, "text cmd (team loot)"),
    (0x096, "text cmd clone"),
    (0x098, "FUN_1007b58c/FUN_1007b88a"),
    (0x09a, "ResetSkill"),
    (0x09d, "text cmd version"),
    (0x0a3, "TryEnterSneakMode"),
    (0x0a5, "EventFeedback(a)"),
    (0x0a6, "EventFeedback(b)"),
    (0x0ae, "text cmd clearunique"),
    (0x0af, "RequestChecklist"),
    (0x0b3, "FUN_1004256c"),
    (0x0b8, "SetPlayerOption"),
    (0x0bb, "FUN_10052ede"),
    (0x0bc, "FUN_10052f8e"),
    (0x0c6, "StartAltState"),
    (0x0c7, "StopAltState"),
    (0x0ca, "Forage"),
    (0x0d2, "FUN_1006949a"),
    (0x0d3, "DeleteNano"),
    (0x0dc, "InsertSourceAnalyzerItem"),
    (0x0dd, "InsertTargetAnalyzerItem"),
    (0x0de, "BuildAnalyzerItem"),
    (0x0ef, "PetDuel_Challenge"),
    (0x0f0, "PetDuel_Accept/Refuse"),
    (0x0f1, "PetDuel_Stop"),
    (0x0f4, "FUN_1004b1ab"),
    (0x0f5, "FUN_1004b254"),
    (0x0fd, "AddToQueue"),
    (0x0fe, "GetInfo"),
    (0x0ff, "LeaveQueue"),
    (0x100, "RefreshLaserTags"),
    (0x101, "ArtilleryAttack"),
    (0x102, "OrbitalAttack"),
    (0x103, "Airstrike"),
    (0x104, "GetPointLocations"),
    (0x105, "Inspect"),
    (0x106, "Duel_*"),
    (0x107, "RequestClaims"),
];

/// Action ids the client's apply switch handles: `(id, case index, handler code address in Gamecode.dll)`.
pub static RECEIVED: &[(i32, u8, u32)] = &[
    (0x001, 0x00, 0x1005d187),
    (0x012, 0x01, 0x1005d1e4),
    (0x014, 0x02, 0x1005e2cd),
    (0x015, 0x03, 0x1005d284),
    (0x018, 0x04, 0x1005d2a2),
    (0x01a, 0x05, 0x1005d2ff),
    (0x01b, 0x06, 0x1005d2b5),
    (0x01e, 0x07, 0x1005d235),
    (0x020, 0x08, 0x1005d348),
    (0x023, 0x09, 0x1005d384),
    (0x02b, 0x0a, 0x1005d45c),
    (0x02e, 0x0b, 0x1005d4c3),
    (0x02f, 0x0c, 0x1005d4e5),
    (0x034, 0x0d, 0x1005d4f6),
    (0x035, 0x0e, 0x1005d50e),
    (0x03b, 0x0f, 0x1005d522),
    (0x03c, 0x10, 0x1005d65c),
    (0x049, 0x11, 0x1005d674),
    (0x04f, 0x12, 0x1005d695),
    (0x050, 0x13, 0x1005ebe7),
    (0x054, 0x14, 0x1005d70f),
    (0x056, 0x15, 0x1005d723),
    (0x057, 0x16, 0x1005d72f),
    (0x061, 0x17, 0x1005d790),
    (0x062, 0x18, 0x1005d7de),
    (0x063, 0x19, 0x1005d827),
    (0x064, 0x1a, 0x1005d873),
    (0x066, 0x1b, 0x1005d258),
    (0x06a, 0x1c, 0x1005d887),
    (0x06b, 0x1d, 0x1005d8bd),
    (0x06c, 0x1e, 0x1005d8d7),
    (0x06d, 0x1f, 0x1005d897),
    (0x06e, 0x20, 0x1005d8ad),
    (0x070, 0x21, 0x1005d8f1),
    (0x075, 0x22, 0x1005d913),
    (0x076, 0x23, 0x1005d92b),
    (0x077, 0x24, 0x1005da11),
    (0x07a, 0x25, 0x1005dae8),
    (0x07b, 0x26, 0x1005db15),
    (0x081, 0x27, 0x1005dc31),
    (0x082, 0x28, 0x1005dc12),
    (0x083, 0x29, 0x1005d7b7),
    (0x084, 0x2a, 0x1005ddbc),
    (0x089, 0x2b, 0x1005d248),
    (0x08a, 0x2c, 0x1005dc77),
    (0x08b, 0x2d, 0x1005dc50),
    (0x08c, 0x2e, 0x1005dc96),
    (0x091, 0x2f, 0x1005ddda),
    (0x092, 0x30, 0x1005ddf3),
    (0x093, 0x31, 0x1005de04),
    (0x099, 0x32, 0x1005d861),
    (0x09b, 0x33, 0x1005de1a),
    (0x09c, 0x34, 0x1005dd18),
    (0x09d, 0x35, 0x1005deef),
    (0x09e, 0x36, 0x1005dfe0),
    (0x0a2, 0x37, 0x1005e006),
    (0x0a4, 0x38, 0x1005e016),
    (0x0a7, 0x39, 0x1005e3d2),
    (0x0a8, 0x3a, 0x1005d320),
    (0x0a9, 0x3b, 0x1005d334),
    (0x0aa, 0x3c, 0x1005e350),
    (0x0ab, 0x3d, 0x1005e41e),
    (0x0ac, 0x3e, 0x1005e479),
    (0x0ad, 0x3f, 0x1005e489),
    (0x0b0, 0x40, 0x1005d213),
    (0x0b1, 0x41, 0x1005d7f5),
    (0x0b2, 0x42, 0x1005e2b4),
    (0x0b4, 0x43, 0x1005e49b),
    (0x0b5, 0x44, 0x1005e4dd),
    (0x0b6, 0x45, 0x1005e4bc),
    (0x0b9, 0x46, 0x1005e69c),
    (0x0ba, 0x47, 0x1005e6b9),
    (0x0bb, 0x48, 0x1005e4f9),
    (0x0bc, 0x49, 0x1005e512),
    (0x0bd, 0x4a, 0x1005e6cb),
    (0x0be, 0x4b, 0x1005e6e5),
    (0x0c0, 0x4c, 0x1005e738),
    (0x0c1, 0x4d, 0x1005e752),
    (0x0c2, 0x4e, 0x1005e76c),
    (0x0c3, 0x4f, 0x1005e786),
    (0x0c4, 0x50, 0x1005e52b),
    (0x0c5, 0x51, 0x1005e5db),
    (0x0c6, 0x52, 0x1005e3fe),
    (0x0c7, 0x53, 0x1005e40e),
    (0x0c9, 0x54, 0x1005e7a0),
    (0x0cb, 0x55, 0x1005e7c6),
    (0x0cc, 0x56, 0x1005e7e6),
    (0x0cd, 0x57, 0x1005e846),
    (0x0ce, 0x58, 0x1005e18a),
    (0x0cf, 0x59, 0x1005e297),
    (0x0d0, 0x5a, 0x1005e8b5),
    (0x0d1, 0x5b, 0x1005ea42),
    (0x0df, 0x5c, 0x1005ec5a),
    (0x0e0, 0x5d, 0x1005ec71),
    (0x0e1, 0x5e, 0x1005ec88),
    (0x0e2, 0x5f, 0x1005ec99),
    (0x0e3, 0x60, 0x1005ecb0),
    (0x0e4, 0x61, 0x1005eccb),
    (0x0ef, 0x62, 0x1005eced),
    (0x0f0, 0x62, 0x1005eced),
    (0x0f1, 0x62, 0x1005eced),
    (0x0f3, 0x62, 0x1005eced),
    (0x0f8, 0x62, 0x1005eced),
    (0x0fc, 0x63, 0x1005e7b3),
    (0x105, 0x64, 0x1005ed1a),
    (0x106, 0x65, 0x1005ed04),
];

/// Sender name of an action id, if the client sends it.
pub fn sent_name(action: i32) -> Option<&'static str> {
    SENT.iter().find(|e| e.0 == action).map(|e| e.1)
}

/// `(case index, handler address)` of the client's apply switch for `action`, `None` if it falls to the default (ignored).
pub fn received_case(action: i32) -> Option<(u8, u32)> {
    RECEIVED.iter().find(|e| e.0 == action).map(|e| (e.1, e.2))
}

/// Header of `CharacterActionIIR_t` / `SocialActionCmd_t` payloads: `u32 key`, target, `to_be_passed_on` byte.
fn header(w: &mut Writer, class: &str, target: Identity, passed_on: u8) {
    w.u32(message_key(class));
    target.write(w);
    w.u8(passed_on);
}

/// `CharacterActionIIR_t` of character `char_id` (`FUN_1007253f` [GC 0x1007253f]): header `{0xC350, char_id}` with the
/// to-be-passed-on byte **0** (`this[0xc] = 0`), then `i32 action, i32 param, Identity a, Identity b, i16 len + text`.
pub fn character_action(char_id: i32, a: &CharacterAction) -> Vec<u8> {
    character_action_for(Identity { kind: DYNEL_CHAR, instance: char_id }, a)
}

/// Same for an arbitrary header identity (relayed copies carry the acting dynel).
pub fn character_action_for(target: Identity, a: &CharacterAction) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, "CharacterActionIIR_t", target, 0);
    w.i32(a.action);
    w.i32(a.param);
    a.identity_a.write(&mut w);
    a.identity_b.write(&mut w);
    w.str_i16(&a.text);
    w.0
}

/// Parse a `CharacterActionIIR_t` payload: `(header, action)`.
pub fn parse_character_action(payload: &[u8]) -> Result<(N3Header, CharacterAction)> {
    let (h, mut r) = N3Header::parse(payload)?;
    if h.msg_type != world::CHARACTER_ACTION {
        bail!("not a CharacterAction: {:08X}", h.msg_type);
    }
    let Some(World::CharacterAction(a)) = world::decode(&h, &mut r)? else { bail!("CharacterAction not decoded") };
    if r.remaining() != 0 {
        bail!("{} trailing bytes after CharacterAction", r.remaining());
    }
    Ok((h, a))
}

/// An action with `param = 0`, empty text and zero identities unless given (what every `N3Msg_*` sender above builds).
pub fn simple(action: i32, a: Identity, b: Identity) -> CharacterAction {
    CharacterAction { action, param: 0, identity_a: a, identity_b: b, text: String::new() }
}

/// `SocialActionCmd_t` body: `n3Command_t` `state` (`+0x18`, 0 from the client), command `counter` (`+0x1c`,
/// `s_nCommandRefCntr++`), then the `AbstractAnimID_e` (`+0x20`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocialAction {
    pub target: Identity,
    pub state: i32,
    pub counter: i32,
    /// `1..=0x47` (the reader `FUN_1007aa23` [GC] rejects everything else).
    pub anim: i32,
}

/// Write side (`FUN_1007aa03` [GC] over `n3Command_t::WriteSubClass` [N3 0x100037c5]); header flag byte 1 (`n3Command_t` ctor).
pub fn social_action(char_id: i32, counter: i32, anim: i32) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, "SocialActionCmd_t", Identity { kind: DYNEL_CHAR, instance: char_id }, 1);
    w.i32(0);
    w.i32(counter);
    w.i32(anim);
    w.0
}

/// Read side (`FUN_1007aa23` [GC] over `n3Command_t::ReadSubClass` [N3 0x1000378f]).
pub fn parse_social_action(payload: &[u8]) -> Result<SocialAction> {
    let (h, mut r) = N3Header::parse(payload)?;
    if h.msg_type != SOCIAL_ACTION_CMD {
        bail!("not a SocialActionCmd: {:08X}", h.msg_type);
    }
    let s = read_social(&mut r, h.target)?;
    if r.remaining() != 0 {
        bail!("{} trailing bytes after SocialActionCmd", r.remaining());
    }
    Ok(s)
}

/// Read the body of a decoded-but-unclaimed message (`N3::Unknown(body)`, i.e. the bytes after the 13-byte header).
pub fn parse_social_body(target: Identity, body: &[u8]) -> Result<SocialAction> {
    let mut r = Reader::new(body);
    let s = read_social(&mut r, target)?;
    if r.remaining() != 0 {
        bail!("{} trailing bytes after SocialActionCmd", r.remaining());
    }
    Ok(s)
}

fn read_social(r: &mut Reader, target: Identity) -> Result<SocialAction> {
    let (state, counter, anim) = (r.i32()?, r.i32()?, r.i32()?);
    if !(1..=0x47).contains(&anim) {
        bail!("social action id {anim} outside 1..=0x47");
    }
    Ok(SocialAction { target, state, counter, anim })
}

/// One row of the client's emote table (`FUN_10053c73` [GC] / `FUN_100b29be` [GUI 0x101badd0], 12-byte rows
/// `{ name, id, second string }`; the id is the row index + 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emote {
    pub id: i32,
    /// The `/<name>` chat command (`_stricmp` against this string only).
    pub command: &'static str,
    /// The row's third field: another name (`allah`, `ass`, `crotch`) or a `*_01_01` animation name; its consumer was not found.
    pub alias: Option<&'static str>,
}

impl Emote {
    /// Clip name in the character animation set (`social-<name>`; docs/formats.md "Clip sets").
    pub fn clip(&self) -> String {
        format!("social-{}", self.command)
    }
    /// Movement transition the client runs for it besides the animation: sleep (`0x44`) and lounge (`0x45`)
    /// (`FUN_1007aa66` [GC]).
    pub fn transition(&self) -> Option<u8> {
        match self.id {
            SLEEP_EMOTE => Some(mv::SWITCH_TO_SLEEP),
            LOUNGE_EMOTE => Some(mv::SWITCH_TO_LOUNGE),
            _ => None,
        }
    }
}

pub const SLEEP_EMOTE: i32 = 0x44;
pub const LOUNGE_EMOTE: i32 = 0x45;

static EMOTE_ROWS: &[(i32, &str, Option<&str>)] = &[
    (0x01, "prostrate", Some("allah")),
    (0x02, "angry", None),
    (0x03, "apachi", None),
    (0x04, "applause", None),
    (0x05, "itch", Some("ass")),
    (0x06, "backflip", None),
    (0x07, "ballet", None),
    (0x08, "blowkiss", None),
    (0x09, "bow", None),
    (0x0a, "bulge", None),
    (0x0b, "chicken", None),
    (0x0c, "cross", None),
    (0x0d, "crossarm", None),
    (0x0e, "adjust", Some("crotch")),
    (0x0f, "curt", None),
    (0x10, "disco", None),
    (0x11, "drink", None),
    (0x12, "eat", None),
    (0x13, "fblock", None),
    (0x14, "fishsize", None),
    (0x15, "flamenco", None),
    (0x16, "flip", None),
    (0x17, "giggle", None),
    (0x18, "gloat", None),
    (0x19, "greet", None),
    (0x1a, "italian", None),
    (0x1b, "kneel", None),
    (0x1c, "laugh-b", None),
    (0x1d, "laugh-s", None),
    (0x1e, "legshake", None),
    (0x1f, "lookout", None),
    (0x20, "moon", None),
    (0x21, "nod", None),
    (0x22, "nono", None),
    (0x23, "pointba", None),
    (0x24, "pointfor", None),
    (0x25, "pointlef", None),
    (0x26, "pointrig", None),
    (0x27, "pointup", None),
    (0x28, "pray", None),
    (0x29, "puke", None),
    (0x2a, "pulp", None),
    (0x2b, "read", None),
    (0x2c, "rocky", None),
    (0x2d, "salute", None),
    (0x2e, "scared", None),
    (0x2f, "scratch", None),
    (0x30, "shake", None),
    (0x31, "shrug", None),
    (0x32, "slap", None),
    (0x33, "speech", None),
    (0x34, "spit", None),
    (0x35, "strong1", None),
    (0x36, "strong2", None),
    (0x37, "strong3", None),
    (0x38, "strong4", None),
    (0x39, "surprised", None),
    (0x3a, "surrender", None),
    (0x3b, "swroyal", None),
    (0x3c, "thinker", None),
    (0x3d, "thumbs", None),
    (0x3e, "wave", None),
    (0x3f, "ymca", None),
    (0x40, "kiss", Some("kiss_01_01")),
    (0x41, "kisslow", Some("kissdown_01_01")),
    (0x42, "kisshigh", Some("kissup_01_01")),
    (0x43, "hug", Some("hug_01_01")),
    (0x44, "sleep", None),
    (0x45, "lounge", None),
    (0x46, "facepalm", None),
];

/// The 70 emotes, ids 1..=0x46.
pub fn emotes() -> impl Iterator<Item = Emote> {
    EMOTE_ROWS.iter().map(|&(id, command, alias)| Emote { id, command, alias })
}

/// `/<name>` or `/emote <name>` lookup (`_stricmp`; the GUI accepts ids `0 < id < 0x47`).
pub fn emote_by_name(name: &str) -> Option<Emote> {
    emotes().find(|e| e.command.eq_ignore_ascii_case(name))
}

/// Emote by id.
pub fn emote(id: i32) -> Option<Emote> {
    emotes().find(|e| e.id == id)
}

/// Where a produced message goes: `SendIIRToObservers` for actions ([`Outgoing::Action`]), the movement sender for sit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// `N3Msg_StopAttack` [GC 0x10027f55] (StopFightIIR_t, see misc.rs).
    StopAttack,
    /// `CharacterActionIIR_t` sent with `n3Dynel_t::SendIIRToObservers(client dynel, ..)`.
    Action(CharacterAction),
    /// `N3Msg_MovementChanged(action, 0.0, 0.0, true)` [GC 0x18b5c] (CharDCMove `move_type`).
    Move(u8),
}

/// The selected item considered by `N3Msg_SitToggle`: `FUN_1008720e(FUN_10058816()+0x5c)` [GC] turns an identity held by
/// the targeting object into a `SimpleItem_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SitItem {
    /// `item+0xc4`.
    pub identity: Identity,
    /// Item stat `Can` (0x1e, kind 2).
    pub can: i32,
}

/// Inputs of `N3Msg_SitToggle` [GC 0x10028e0a].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SitInput {
    /// Fight state (`char+0x1d4` controller `+0x44 != 1`).
    pub fighting: bool,
    /// `char+0x50` (`Vehicle_t`) virtual `+0x9c` returned true: the client does nothing [meaning unresolved].
    pub blocked: bool,
    /// Own stat `WaitState` (0x1ae).
    pub wait_state: i32,
    /// `char+0x50 -> +0x178 (FSM) -> +4`, see [`mode`].
    pub fsm_mode: i32,
    pub item: Option<SitItem>,
}

/// Decision logic of `N3Msg_SitToggle` [GC 0x10028e0a], in the original order.
pub fn sit_toggle(s: &SitInput) -> Vec<Outgoing> {
    let mut out = Vec::new();
    if s.fighting {
        out.push(Outgoing::StopAttack);
    }
    if s.blocked {
        return out;
    }
    let none = Identity::default();
    let stand = || Outgoing::Action(simple(id::STAND_UP, none, none));
    if matches!(s.wait_state, wait::SLEEP | wait::LOUNGE) {
        out.push(stand());
    } else if let Some(item) = s.item.filter(|i| i.can & 2 != 0) {
        out.push(if s.wait_state == wait::ON_ITEM {
            stand()
        } else {
            Outgoing::Action(simple(id::SIT_ON_ITEM, item.identity, none))
        });
    } else if s.fsm_mode == mode::SIT_GROUND {
        out.push(stand());
    } else {
        out.push(Outgoing::Move(mv::SWITCH_TO_SIT_GROUND));
    }
    out
}

/// Inputs of `N3Msg_StartCamping` [GC 0x1001c93d].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CampInput {
    pub gm_level: i32,
    pub is_fighting_me: i32,
    pub fsm_mode: i32,
    /// FSM `vtable[3](0x1e)`: may the sit transition run.
    pub can_sit: bool,
    pub attacking: bool,
}

/// Refusal texts of `N3Msg_StartCamping` (`FUN_10058b00(key, 0)` feedback keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampRefusal {
    /// "Feedback_CantLogOutInAFight".
    InFight,
    /// "Feedback_YouMustBeSitting".
    MustBeSitting,
}

impl CampRefusal {
    pub fn key(self) -> &'static str {
        match self {
            CampRefusal::InFight => "Feedback_CantLogOutInAFight",
            CampRefusal::MustBeSitting => "Feedback_YouMustBeSitting",
        }
    }
}

/// `N3Msg_StartCamping`: sits down first (unless a GM or already sitting), stops attacking, then sends action `0x78`.
pub fn start_camping(c: &CampInput) -> Result<Vec<Outgoing>, CampRefusal> {
    let mut out = Vec::new();
    if c.gm_level == 0 {
        if c.is_fighting_me > 0 {
            return Err(CampRefusal::InFight);
        }
        if !matches!(c.fsm_mode, mode::SIT_GROUND | mode::SIT_CHAIR) {
            if !c.can_sit {
                return Err(CampRefusal::MustBeSitting);
            }
            out.push(Outgoing::Move(mv::SWITCH_TO_SIT_GROUND));
        }
    }
    if c.attacking {
        out.push(Outgoing::StopAttack);
    }
    let none = Identity::default();
    out.push(Outgoing::Action(simple(id::START_CAMPING, none, none)));
    Ok(out)
}

/// `N3Msg_StopCamping` [GC 0x1001cada].
pub fn stop_camping() -> Outgoing {
    let none = Identity::default();
    Outgoing::Action(simple(id::STOP_CAMPING, none, none))
}

/// `N3Msg_DoSocialAction` [GC 0x100269d3] checks. `Ok(())` = send a [`social_action`].
pub fn social_allowed(anim: i32, fsm_mode: i32, vehicle_equipped: bool) -> Result<(), &'static str> {
    if fsm_mode == mode::SWIM {
        return Err("Feedback_CantDoSocialActionsWhileSwimming");
    }
    let sleep_or_lounge = anim == SLEEP_EMOTE || anim == LOUNGE_EMOTE;
    if vehicle_equipped && sleep_or_lounge {
        return Err("Feedback_YouCanNotDoThisWithAVehicleEquipped");
    }
    if sleep_or_lounge && !matches!(fsm_mode, mode::SIT_GROUND | mode::SLEEP | mode::LOUNGE) {
        return Err("Feedback_MustSitToLoungeOrSleep");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::capture_n3;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn keys_are_the_class_name_hash() {
        assert_eq!(message_key("SocialActionCmd_t"), SOCIAL_ACTION_CMD);
        assert_eq!(message_key("CharacterActionIIR_t"), world::CHARACTER_ACTION);
    }

    #[test]
    fn captured_relays_reencode_byte_exact() {
        let mut n = 0;
        for f in capture_n3() {
            if f.payload[..4] != world::CHARACTER_ACTION.to_be_bytes() {
                continue;
            }
            let (h, a) = parse_character_action(&f.payload).unwrap();
            assert_eq!(h.flag, 0);
            assert_eq!(character_action_for(h.target, &a), f.payload);
            n += 1;
        }
        assert_eq!(n, 22);
    }

    #[test]
    fn client_bytes() {
        // stand up (0x57) as N3Msg_SitToggle builds it: flag 0, zero identities, empty text
        let a = simple(id::STAND_UP, Identity::default(), Identity::default());
        assert_eq!(
            character_action(0x6584, &a),
            hex("5e477770 0000c350 00006584 00  00000057 00000000 00000000 00000000 00000000 00000000 0000")
        );
        // sit on an item: identity_a = the item
        let a = simple(id::SIT_ON_ITEM, Identity { kind: 0xCF1B, instance: 0x27E79 }, Identity::default());
        assert_eq!(
            character_action(0x6584, &a),
            hex("5e477770 0000c350 00006584 00  00000055 00000000 0000cf1b 00027e79 00000000 00000000 0000")
        );
        let (h, back) = parse_character_action(&character_action(0x6584, &a)).unwrap();
        assert_eq!((h.target.instance, back), (0x6584, a));
    }

    #[test]
    fn truncated_actions_error() {
        let p = character_action(1, &simple(1, Identity::default(), Identity::default()));
        for n in 0..p.len() {
            assert!(parse_character_action(&p[..n]).is_err(), "{n}");
        }
        assert!(parse_character_action(&[p.clone(), vec![0]].concat()).is_err());
    }

    #[test]
    fn social_action_bytes() {
        let p = social_action(0x6584, 7, 0x3e);
        assert_eq!(p, hex("3b290771 0000c350 00006584 01  00000000 00000007 0000003e"));
        let s = parse_social_action(&p).unwrap();
        assert_eq!((s.state, s.counter, s.anim, s.target.instance), (0, 7, 0x3e, 0x6584));
        for bad in [0, 0x48, -1] {
            assert!(parse_social_action(&social_action(1, 0, bad)).is_err());
        }
        assert!(parse_social_action(&p[..p.len() - 1]).is_err());
        assert_eq!(parse_social_body(s.target, &p[13..]).unwrap(), s);
        assert!(parse_social_body(s.target, &p[13..p.len() - 1]).is_err());
        assert!(parse_social_action(&character_action(1, &simple(1, Identity::default(), Identity::default()))).is_err());
    }

    #[test]
    fn emote_table() {
        let all: Vec<_> = emotes().collect();
        assert_eq!(all.len(), 70);
        assert!(all.iter().enumerate().all(|(i, e)| e.id == i as i32 + 1));
        assert_eq!(emote_by_name("WAVE").unwrap().id, 0x3e);
        assert_eq!(emote_by_name("wave").unwrap().clip(), "social-wave");
        assert_eq!(emote_by_name("prostrate").unwrap().alias, Some("allah"));
        assert!(emote_by_name("allah").is_none());
        assert_eq!(emote(SLEEP_EMOTE).unwrap().command, "sleep");
        assert_eq!(emote(LOUNGE_EMOTE).unwrap().transition(), Some(mv::SWITCH_TO_LOUNGE));
        assert_eq!(emote(0x47), None);
        // every command of the Action menu's social page (docs/zone/actions.md) exists
        for n in ["bow", "curt", "kneel", "ymca", "disco", "thumbs", "backflip", "pointup", "lounge", "scratch", "laugh-b"] {
            assert!(emote_by_name(n).is_some(), "{n}");
        }
    }

    #[test]
    fn tables_are_sorted_and_consistent() {
        assert_eq!(RECEIVED.len(), 106);
        assert!(RECEIVED.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(SENT.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(received_case(id::STAND_UP), Some((0x16, 0x1005d72f)));
        assert_eq!(received_case(id::SIT_RELAY), Some((0x15, 0x1005d723)));
        assert_eq!(received_case(id::SIT_ON_ITEM), None);
        assert_eq!(received_case(0x107), None);
        assert_eq!(sent_name(id::START_CAMPING), Some("StartCamping"));
        // the captured relays are all handled by the client
        for f in capture_n3() {
            if f.payload[..4] == world::CHARACTER_ACTION.to_be_bytes() {
                let (_, a) = parse_character_action(&f.payload).unwrap();
                assert!(received_case(a.action).is_some(), "{:#x}", a.action);
            }
        }
        assert_eq!(transition_name(mv::SWITCH_TO_SIT_GROUND), Some("SwitchToSitGroundMode"));
        assert_eq!(transition_name(mv::LEAVE_LOUNGE), Some("LeaveLoungeMode"));
    }

    fn base() -> SitInput {
        SitInput { fsm_mode: 1, ..Default::default() }
    }
    fn stand() -> Outgoing {
        Outgoing::Action(simple(id::STAND_UP, Identity::default(), Identity::default()))
    }

    #[test]
    fn sit_toggle_decisions() {
        // plain standing char sits on the ground; sitting char stands up with action 0x57
        assert_eq!(sit_toggle(&base()), [Outgoing::Move(0x1e)]);
        assert_eq!(sit_toggle(&SitInput { fsm_mode: mode::SIT_GROUND, ..base() }), [stand()]);
        // sleeping / lounging (WaitState 0xf / 0x10) -> 0x57 regardless of the FSM mode or an item
        let sit_item = SitItem { identity: Identity { kind: 5, instance: 9 }, can: 2 };
        let item = Some(sit_item);
        for w in [wait::SLEEP, wait::LOUNGE] {
            assert_eq!(sit_toggle(&SitInput { wait_state: w, item, ..base() }), [stand()]);
        }
        // a sittable selected item: sit on it (0x55 with its identity), or leave it when WaitState = 1
        assert_eq!(
            sit_toggle(&SitInput { item, ..base() }),
            [Outgoing::Action(simple(id::SIT_ON_ITEM, Identity { kind: 5, instance: 9 }, Identity::default()))]
        );
        assert_eq!(sit_toggle(&SitInput { item, wait_state: wait::ON_ITEM, ..base() }), [stand()]);
        // an item without Can&2 is ignored
        assert_eq!(sit_toggle(&SitInput { item: Some(SitItem { can: 1, ..sit_item }), ..base() }), [Outgoing::Move(0x1e)]);
        // fighting stops the attack first; a blocked controller sends nothing else
        assert_eq!(sit_toggle(&SitInput { fighting: true, ..base() }), [Outgoing::StopAttack, Outgoing::Move(0x1e)]);
        assert_eq!(sit_toggle(&SitInput { fighting: true, blocked: true, ..base() }), [Outgoing::StopAttack]);
        assert!(sit_toggle(&SitInput { blocked: true, ..base() }).is_empty());
    }

    #[test]
    fn camping_and_social_rules() {
        let c = CampInput { fsm_mode: 1, can_sit: true, ..Default::default() };
        let acts = start_camping(&c).unwrap();
        assert_eq!(acts[0], Outgoing::Move(0x1e));
        assert!(matches!(&acts[1], Outgoing::Action(a) if a.action == 0x78));
        // already sitting: just the action; GMs skip the checks; fighting refuses
        assert_eq!(start_camping(&CampInput { fsm_mode: mode::SIT_CHAIR, ..c }).unwrap().len(), 1);
        assert_eq!(start_camping(&CampInput { gm_level: 1, is_fighting_me: 3, can_sit: false, ..c }).unwrap().len(), 1);
        assert_eq!(start_camping(&CampInput { is_fighting_me: 1, ..c }), Err(CampRefusal::InFight));
        assert_eq!(start_camping(&CampInput { can_sit: false, ..c }), Err(CampRefusal::MustBeSitting));
        assert_eq!(
            start_camping(&CampInput { attacking: true, fsm_mode: 8, ..c }).unwrap()[0],
            Outgoing::StopAttack
        );
        assert!(matches!(stop_camping(), Outgoing::Action(a) if a.action == 0x79));

        assert_eq!(social_allowed(9, mode::SWIM, false), Err("Feedback_CantDoSocialActionsWhileSwimming"));
        assert_eq!(social_allowed(9, 1, false), Ok(()));
        assert_eq!(social_allowed(SLEEP_EMOTE, 1, false), Err("Feedback_MustSitToLoungeOrSleep"));
        assert_eq!(social_allowed(LOUNGE_EMOTE, mode::SIT_GROUND, false), Ok(()));
        assert_eq!(social_allowed(LOUNGE_EMOTE, mode::SIT_GROUND, true), Err("Feedback_YouCanNotDoThisWithAVehicleEquipped"));
    }
}
