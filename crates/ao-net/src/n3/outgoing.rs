//! Client -> zone-server messages and the registry of all N3 message ids. Layouts and addresses: docs/zone/outgoing.md.
//!
//! Every N3 message is an `n3InfoItemRemote_t` ("IIR"): `n3InfoItemRemote_t::Write` [N3 0x100098f4] emits
//! `u32 key = MapToKey(class name)`, the target [`Identity`], one "to be passed on" byte and the class body.
//! `n3EngineClient_t::SendIIRToServer` [N3 0x10007762] hands that to `Client_t::SendACEDataBlock`
//! [Interfaces 0x10001692] which wraps it in a ptype-10 frame `sender = s_nCharID, receiver = 2`.

use crate::frame::Frame;
use crate::msg::Identity;
use crate::wire::Writer;
use super::N3Header;
use anyhow::{bail, Result};

/// Frame `ptype` of N3 messages (`N3Message_t` = `Message_t(10, ..)` [MP 0x10001fa7]).
pub const PT_N3: u16 = 0xA;
/// Frame `ptype` of text messages (`TextMessage_t` = `Message_t(5, ..)` [MP 0x10003095]).
pub const PT_TEXT: u16 = 5;
/// `receiver` of every client frame (`N3Message_t(s_nCharID, 2, ..)`, `TextMessage_t(type, s_nCharID, 2, ..)`,
/// `SystemMessage_t(0x1b, s_nCharID, 2, ..)`): the zone server.
pub const ZONE_RECEIVER: u32 = 2;
/// `Identity_t` kind of a character/NPC dynel.
pub const DYNEL_CHAR: i32 = 0xC350;

/// `n3InfoItemRemote_t::MapToKey` [N3 0x10009826]: `key ^= (i8)c << ((i & 3) * 8)` over the class name.
pub fn message_key(class: &str) -> u32 {
    class.bytes().enumerate().fold(0u32, |k, (i, c)| k ^ (((c as i8) as i32 as u32) << ((i & 3) * 8)))
}

/// `CharInPlayIIR_t`: "the client finished loading, send me the world" [GC 0x100726ce].
pub const CHAR_IN_PLAY: u32 = 0x570C2039;
/// `CharDCMoveIIR_t`: movement/position update [GC 0x1006ba23].
pub const CHAR_DC_MOVE: u32 = 0x54111123;

/// N3 payload header common to every IIR the client writes. `to_be_passed_on` is `this+0xc == 1`; both client
/// messages below set it (`CharInPlayIIR_t` ctor [GC 0x100726ce], `CharDCMoveIIRBase_t` ctor [GC 0x1006bb28]).
fn iir(key: u32, target: Identity, body: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.u32(key);
    target.write(&mut w);
    w.u8(1);
    body(&mut w);
    w.0
}

/// `N3Msg_SendInPlayMessage` [GC 0x10018090]: payload of the in-play message of character `char_id`
/// (`Identity_t` = control dynel `+0x14` = `{0xC350, char_id}`). 13 bytes, no body.
pub fn char_in_play(char_id: i32) -> Vec<u8> {
    iir(CHAR_IN_PLAY, Identity { kind: DYNEL_CHAR, instance: char_id }, |_| {})
}

/// Body of `CharDCMoveIIR_t` (`FUN_1006b9d6` [GC 0x1006b9d6] + `FUN_1006bc55` [GC 0x1006bc55]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharMove {
    /// `Movement_n::MovementAction_e` (one byte; the receiver masks `& 0x7f`).
    pub action: u8,
    /// Dynel `GetRelPos()` as sent: x, y (up), z.
    pub pos: [f32; 3],
    /// Dynel `GetRelRot()` quaternion in wire order x, y, z, w.
    pub rot: [f32; 4],
    /// Milliseconds since the previous `CharDCMove` write (`round((GameTime - last) * 1000)`, first = game time).
    pub elapsed_ms: i32,
    /// The two floats of `N3Msg_MovementChanged(.., f1, f2, ..)`: mouse-look deltas for action `0x2b`, else 0.
    pub look: [f32; 2],
}

/// `N3Msg_MovementChanged` [GC 0x18b5c] -> `SendIIRToServer(CharDCMoveIIR_t)`. 54 bytes.
pub fn char_dc_move(char_id: i32, m: &CharMove) -> Vec<u8> {
    iir(CHAR_DC_MOVE, Identity { kind: DYNEL_CHAR, instance: char_id }, |w| {
        w.u8(m.action);
        m.rot.iter().chain(&m.pos).for_each(|&f| w.f32(f));
        w.i32(m.elapsed_ms);
        m.look.iter().for_each(|&f| w.f32(f));
    })
}

/// A ptype-10 frame carrying an N3 payload from character `char_id`.
pub fn n3_frame(seq: u16, char_id: u32, payload: Vec<u8>) -> Frame {
    Frame { seq, ptype: PT_N3, sender: char_id, receiver: ZONE_RECEIVER, payload }
}

/// `TextMessage_t` type word (`Client_t::SendWhisper/Vicinity/ShoutMessage` [Interfaces 0x100015d8/0x10001464/0x1000151e]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    Whisper = 2,
    Vicinity = 3,
    Shout = 4,
}

/// Text payload: `u32 kind`, `Identity`, `i32 len`, `len` raw bytes (`TextMessage_t::CreateDataBlock` [MP 0x10002f7a]
/// puts the kind at frame offset 16, the `BinaryStream` body at 20). `target` is the `Identity_t` the GUI passes
/// through `AFCM` (unresolved who it names, docs/zone/outgoing.md); `text` is sent without a terminator.
pub fn text_payload(kind: TextKind, target: Identity, text: &[u8]) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.u32(kind as u32);
    target.write(&mut w);
    w.i32(text.len() as i32);
    w.bytes(text);
    w.0
}

/// A ptype-5 frame carrying a text payload from character `char_id`.
pub fn text_frame(seq: u16, char_id: u32, payload: Vec<u8>) -> Frame {
    Frame { seq, ptype: PT_TEXT, sender: char_id, receiver: ZONE_RECEIVER, payload }
}

/// Every `n3InfoItemRemote_t::Register` call found in Gamecode.dll and N3.dll, sorted by id
/// (scan: docs/zone/outgoing.md "Registry").
pub static REGISTRY: &[(u32, &str)] = &[
    (0x000A0C5A, "KnubotNPCDescriptionIIR_c"),
    (0x052E2F0C, "AddTemplateIIR_t"),
    (0x0639474D, "GridDestinationSelectIIR_t"),
    (0x08536F65, "CentralControllerStateIIR_t"),
    (0x0C5A5D6D, "WeatherControlIIR_t"),
    (0x0D381F02, "PetToMasterIIR_c"),
    (0x1078735A, "FlushRDBCachesIIR_c"),
    (0x15253307, "CentralControllerFullUpdateIIR_t"),
    (0x166A435E, "AcceptBSInviteIIR_t"),
    (0x194E4F76, "AddPetIIR_c"),
    (0x195E496E, "SetPosIIR_c"),
    (0x1C3A4F77, "ReflectAttackIIR_t"),
    (0x1D3C0F1C, "SpecialAttackWeaponIIR_t"),
    (0x2001377E, "MentorInviteIIR_c"),
    (0x2049527C, "ActionIIR_t"),
    (0x204F4871, "ScriptIIR_t"),
    (0x206B4B73, "FormatFeedbackIIR_t"),
    (0x2103247D, "KnubotAnswerIIR_c"),
    (0x212C487A, "QuestIIR_t"),
    (0x215B5678, "MineFullUpdateIIR_t"),
    (0x2252445F, "LookAtIIR_t"),
    (0x25192476, "ShieldAttackIIR_t"),
    (0x25314D6D, "CastNanoSpellIIR_t"),
    (0x253D0240, "ResearchUpdateIIR"),
    (0x260F3671, "FollowTargetIIR_c"),
    (0x264B514B, "RelocateDynelsIIR_t"),
    (0x264E5F61, "AbsorbIIR_t"),
    (0x26515E61, "ReloadIIR_t"),
    (0x270A4C62, "KnubotCloseChatWindowIIR_c"),
    (0x271B3A6B, "SimpleCharFullUpdateIIR_t"),
    (0x28251F01, "StartLogoutIIR_t"),
    (0x28494070, "AttackIIR_t"),
    (0x28784248, "TeamMemberInfoIIR_t"),
    (0x29304349, "FullCharacterIIR_t"),
    (0x2933154F, "LaserTagListIIR_t"),
    (0x2A253F5F, "TrapDisarmedIIR_t"),
    (0x2A293D0F, "FovIIR_c"),
    (0x2B333D6E, "StatIIR_t"),
    (0x2C2F061C, "QueueUpdateIIR_t"),
    (0x2D212407, "KnubotRejectedItemsIIR_c"),
    (0x2E2A4A6B, "OrgInfoPacketIIR_t"),
    (0x30161355, "n3PlayfieldFullUpdateIIR_t"),
    (0x3129233B, "AreaFormulaIIR_t"),
    (0x3301337A, "InfromPlayerIIR_t"),
    (0x33312042, "WaypointPathIIR_c"),
    (0x333B2867, "MailIIR_c"),
    (0x342C1D1D, "ApplySpellsIIR_t"),
    (0x343C287F, "BankIIR_t"),
    (0x35505644, "TemplateActionIIR_t"),
    (0x36284F6E, "TradeIIR_t"),
    (0x36510078, "n3ToClientQuitIIR_t"),
    (0x365A5071, "DoorFullUpdateIIR_t"),
    (0x365E555B, "CityAdvantagesIIR_t"),
    (0x3710256C, "HealthDamageIIR_t"),
    (0x371D0542, "FightModeUpdate_t"),
    (0x39343C68, "BuffIIR_c"),
    (0x3A1B2C0C, "KnubotTradeIIR_c"),
    (0x3A223B50, "ItemReplacedIIR_c"),
    (0x3A243F41, "DropTemplateIIR_t"),
    (0x3A322A4A, "GridSelectedIIR_t"),
    (0x3B11256F, "SimpleItemFullUpdateIIR_t"),
    (0x3B132D64, "KnubotOpenChatWindowIIR_c"),
    (0x3B1D2268, "WeaponItemFullUpdateIIR_t"),
    (0x3B290771, "SocialActionCmd_t"),
    (0x3B3B2878, "RaidIIR_c"),
    (0x3C1E2803, "ShadowLevelIIR_t"),
    (0x3C265179, "CloneIIR_t"),
    (0x3D746C70, "ServerPathPosDebugInfoIIR_c"),
    (0x3E205660, "SkillIIR_t"),
    (0x3F3A1914, "LeaveBattleIIR_t"),
    (0x41624F0D, "AppearanceUpdateIIR_c"),
    (0x43197D22, "n3TeleportIIR_t"),
    (0x435F7023, "PerkUpdateIIR"),
    (0x44483B3A, "SendScoreIIR_t"),
    (0x445F2A0B, "ResurrectIIR_t"),
    (0x45072A2D, "UpdateClientVisualIIR_t"),
    (0x455D2938, "PlaySoundIIR_c"),
    (0x46002F16, "AttackInfoIIR_t"),
    (0x46312D2E, "TeamMemberIIR_t"),
    (0x464D000A, "SpawnMechIIR_t"),
    (0x465A4061, "QuestFullUpdateIIR_t"),
    (0x465A5D73, "ChestFullUpdateIIR_t"),
    (0x470B2E14, "MarketSendIIR_c"),
    (0x47483633, "DropDynelIIR_t"),
    (0x47537A24, "ContainerAddItemIIR_t"),
    (0x485E7202, "InventoryUpdatedIIR_t"),
    (0x49222612, "VisibilityIIR_t"),
    (0x4A41203E, "StopFightIIR_t"),
    (0x4B062919, "BattleOverIIR_t"),
    (0x4C7D403B, "DoorStatusUpdateIIR_t"),
    (0x4D2A313B, "TeamInviteIIR_t"),
    (0x4D38242E, "InfoPacketIIR_t"),
    (0x4D450114, "SpellListIIR_t"),
    (0x4E536976, "InventoryUpdateIIR_t"),
    (0x4F474E05, "CorpseFullUpdateIIR_t"),
    (0x50544D19, "FeedbackIIR_t"),
    (0x51492120, "CharSecSpecAttackIIR_t"),
    (0x52213420, "BankCorpseIIR_t"),
    (0x52526858, "GenericCmd_t"),
    (0x540E3B27, "ArriveAtBsIIR_t"),
    (0x54111123, "CharDCMoveIIR_t"),
    (0x55220726, "PlayfieldAllTowersIIR_t"),
    (0x55682B24, "KnubotFinishTradeIIR_c"),
    (0x55704D31, "KnubotAnswerListIIR_c"),
    (0x56353038, "StopLogoutIIR_t"),
    (0x570C2039, "CharInPlayIIR_t"),
    (0x58362220, "ShopUpdateIIR_t"),
    (0x58574239, "MechInfoIIR_t"),
    (0x58742A0F, "RemovePetIIR_c"),
    (0x59210126, "PlayfieldAllCitiesIIR_t"),
    (0x59313928, "TrapItemFullUpdateIIR_t"),
    (0x5A585F65, "InspectIIR_c"),
    (0x5B1E052C, "PlayfieldTowerUpdateClientIIR_t"),
    (0x5C240404, "ServerPosDebugInfoIIR_c"),
    (0x5C436609, "QuestAlternativeIIR_t"),
    (0x5C4A493A, "FullAutoIIR_t"),
    (0x5C654B28, "MissedAttackInfoIIR_t"),
    (0x5D70532A, "KnubotAppendTextIIR_c"),
    (0x5E477770, "CharacterActionIIR_t"),
    (0x5F4A4C6C, "ImpulseIIR_c"),
    (0x5F4B1A39, "PlayfieldAnarchyFIIR_t"),
    (0x5F4B442A, "ChatTextIIR_t"),
    (0x5F52412E, "GameTimeIIR_t"),
    (0x60201D0E, "SetWantedDirectionIIR_t"),
    (0x62741E15, "AOTransportSignalIIR_c"),
    (0x64582A07, "OrgServerIIR_c"),
    (0x6B333303, "PetCommandIIR_c"),
    (0x6C6C756E, "null"),
    (0x6E5F566E, "SetStatIIR_t"),
    (0x734E5A7B, "SetNameIIR_t"),
    (0x742E2314, "StopMovingCmd_t"),
    (0x754F1115, "SpecialAttackInfoIIR_t"),
    (0x77230927, "GiveQuestToMembersIIR_t"),
    (0x7864401D, "KnubotStartTradeIIR_c"),
    (0x7A222202, "GfxTriggerIIR_t"),
    (0x7F405A16, "NewLevelIIR_t"),
    (0x7F4B3108, "OrgClientIIR_c"),
    (0x7F544905, "VendingMachineFullUpdateIIR_t"),
];

/// Class name of an N3 message id, if it is one of [`REGISTRY`].
pub fn class_name(id: u32) -> Option<&'static str> {
    REGISTRY.binary_search_by_key(&id, |e| e.0).ok().map(|i| REGISTRY[i].1)
}

/// Parse a `CharDCMoveIIR_t` payload (client-written or server-relayed): `(target, flag, body)`.
pub fn parse_char_dc_move(payload: &[u8]) -> Result<(Identity, u8, CharMove)> {
    let (h, mut r) = N3Header::parse(payload)?;
    if h.msg_type != CHAR_DC_MOVE {
        bail!("not a CharDCMove: {:08X}", h.msg_type);
    }
    let action = r.u8()?;
    let mut f = [0f32; 7];
    for v in &mut f {
        *v = r.f32()?;
    }
    let elapsed_ms = r.i32()?;
    let look = [r.f32()?, r.f32()?];
    if r.remaining() != 0 {
        bail!("{} trailing bytes after CharDCMove", r.remaining());
    }
    Ok((h.target, h.flag, CharMove { action, rot: [f[0], f[1], f[2], f[3]], pos: [f[4], f[5], f[6]], elapsed_ms, look }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::{capture, capture_n3};

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn registry_is_sorted_and_hash_matches() {
        assert_eq!(REGISTRY.len(), 138);
        assert!(REGISTRY.windows(2).all(|w| w[0].0 < w[1].0));
        for &(id, name) in REGISTRY {
            assert_eq!(message_key(name), id, "{name}");
        }
        assert_eq!(message_key("CharDCMoveIIR_t"), CHAR_DC_MOVE);
        assert_eq!(message_key("CharInPlayIIR_t"), CHAR_IN_PLAY);
        assert_eq!(class_name(0x5F4B1A39), Some("PlayfieldAnarchyFIIR_t"));
        assert_eq!(class_name(0x1), None);
    }

    #[test]
    fn every_captured_message_id_is_registered() {
        let mut seen = std::collections::BTreeSet::new();
        for f in capture_n3() {
            let id = u32::from_be_bytes(f.payload[..4].try_into().unwrap());
            assert!(class_name(id).is_some(), "{id:08X}");
            seen.insert(id);
        }
        assert_eq!(seen.len(), 27);
    }

    #[test]
    fn in_play_bytes() {
        // key, {0xC350, 0x6584}, to-be-passed-on = 1
        assert_eq!(char_in_play(0x6584), hex("570c2039 0000c350 00006584 01"));
        // same bytes as the server relays for other characters (captured 4853 ms / 70758 ms)
        let relayed: Vec<_> = capture_n3().into_iter().filter(|f| f.payload[..4] == CHAR_IN_PLAY.to_be_bytes()).collect();
        assert_eq!(relayed.len(), 2);
        for f in relayed {
            let inst = i32::from_be_bytes(f.payload[8..12].try_into().unwrap());
            assert_eq!(f.payload, char_in_play(inst));
            assert!([33402, 33491].contains(&inst));
        }
    }

    #[test]
    fn move_bytes() {
        let m = CharMove { action: 1, pos: [1.0, 2.0, 3.0], rot: [0.0, 0.0, 0.0, 1.0], elapsed_ms: 16, look: [0.0, 0.0] };
        let want = hex(
            "54111123 0000c350 00006584 01  01  00000000 00000000 00000000 3f800000
             3f800000 40000000 40400000  00000010  00000000 00000000",
        );
        assert_eq!(char_dc_move(0x6584, &m), want);
        assert_eq!(want.len(), 54);
        assert_eq!(parse_char_dc_move(&want).unwrap(), (Identity { kind: DYNEL_CHAR, instance: 0x6584 }, 1, m));
    }

    #[test]
    fn captured_moves_roundtrip() {
        let mut n = 0;
        for f in capture_n3().into_iter().filter(|f| f.payload[..4] == CHAR_DC_MOVE.to_be_bytes()) {
            let (id, flag, m) = parse_char_dc_move(&f.payload).unwrap();
            assert_eq!((id.kind, flag, f.payload.len()), (DYNEL_CHAR, 0, 54));
            // the server relays with to-be-passed-on cleared; the client writes 1
            let mut want = f.payload.clone();
            want[12] = 1;
            assert_eq!(char_dc_move(id.instance, &m), want);
            n += 1;
        }
        assert_eq!(n, 144);
    }

    #[test]
    fn move_rejects_malformed() {
        let ok = char_dc_move(1, &CharMove { action: 1, pos: [0.0; 3], rot: [0.0; 4], elapsed_ms: 0, look: [0.0; 2] });
        for cut in [0, 12, 13, 30, 53] {
            assert!(parse_char_dc_move(&ok[..cut]).is_err(), "{cut}");
        }
        let mut long = ok.clone();
        long.push(0);
        assert!(parse_char_dc_move(&long).is_err());
        assert!(parse_char_dc_move(&char_in_play(1)).is_err());
    }

    #[test]
    fn frame_header_like_zone_login() {
        let f = n3_frame(5, 0x6584, char_in_play(0x6584));
        let b = f.encode().unwrap();
        // seq, ptype 0xA, version 1, size 16+13, sender, receiver 2, payload, pad to 4
        assert_eq!(&b[..16], &hex("0005 000a 0001 001d 00006584 00000002")[..]);
        assert_eq!(b.len(), 32);
        // the only client frames in the capture: ZoneLogin (ptype 1, receiver 2) and 3 ping replies (receiver = own id)
        let sent: Vec<_> = capture().into_iter().filter(|c| c.1).collect();
        assert_eq!(sent.len(), 4);
        for (_, _, raw) in sent {
            assert_eq!(u32::from_be_bytes(raw[8..12].try_into().unwrap()), 0x6584);
            let want = if raw[3] == 1 { ZONE_RECEIVER } else { 0x6584 };
            assert_eq!(u32::from_be_bytes(raw[12..16].try_into().unwrap()), want);
        }
    }

    #[test]
    fn text_bytes() {
        let p = text_payload(TextKind::Vicinity, Identity { kind: 0, instance: 0 }, b"hi");
        assert_eq!(p, hex("00000003 00000000 00000000 00000002 6869"));
        let f = text_frame(9, 0x6584, p);
        assert_eq!((f.ptype, f.receiver), (5, 2));
        assert_eq!(f.encode().unwrap().len(), 36);
        assert_eq!(TextKind::Whisper as u32 + TextKind::Shout as u32, 6);
    }
}
