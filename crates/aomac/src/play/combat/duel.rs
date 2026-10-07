//! `/duel` and `/petduel`: the received duel / pet-duel messages (`CharacterActionIIR_t` 0x106, 0xef, 0xf0, 0xf3, 0xf8) as the lines and dialogs the
//! original shows. The client keeps no duel state of its own: the apply code only prints and signals the GUI, and `CanAttack` (`FUN_10069556`) does not
//! look at duels (the server answers an attack with action 0x76). Evidence and addresses: docs/zone/combat-duel.md.

use ao_net::msg::Identity;
use ao_net::n3::action::duel as net;
use ao_net::n3::world::CharacterAction;

pub use net::Op;

/// `DYNEL_CHAR` identity kind (`SimpleChar_t`): `FUN_1005d0d8` only resolves `identity_a` to a character when its kind is 50000.
const CHAR: i32 = 0xC350;

/// A line of the System chat window (`FUN_10012a1e` + `FUN_10012b05(0, text, 0)`); the texts are `Feedback_*` / pet-duel keys of LDB category 110.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// `FUN_1005aa3e(110, key, 0)` / `LDBface::GetText(110, key)`: the text as it is.
    Plain(&'static str),
    /// `LDBformat(GetText(110, key)).Feed(name of who).Dump()`. When `who` is not a known character the original takes the other branch:
    /// `otherwise` (a text without a name) or nothing.
    Named { key: &'static str, who: i32, otherwise: Option<&'static str> },
}

/// What a received duel message makes the GUI do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Line(Line),
    /// `FUN_1005b821` sub-op 0 / instance 0: `identity_a` challenges the own character. If `AutoRejectDuel` is set the answer is `net::auto_refuse`
    /// and nothing else happens, otherwise `Feedback_DuelChallenge` is printed and `GuiSystem_c::DuelChallengeReceived` opens the dialog.
    Challenged(i32),
    /// Sub-op 0 / instance 1: the server confirms the own challenge of `identity_a`: `GuiSystem_c::DuelChallengeSent` opens the cancel dialog.
    ChallengeSent(i32),
    /// Sub-ops 1 and 2: `GlobalSignals+0x268 / +0x264` = `GuiSystem_c::CloseDuelWindows`.
    Close,
    /// Action 0x7b (`FUN_1005d0d8` case 0x26 [GC 0x1005db15]): the server wants a confirmation before a PvP attack on `target` (`identity_a`): text
    /// `Combat_PvPTargetLvl` (`team` false) / `Combat_PvPTargetLvlTeam` (LDB category 101), `GlobalSignals+0x54` = `GuiSystem_c::StartPvPFightDialogue`
    /// (GUI 0x1002fa8e): Yes (0) -> `N3Msg_StartPvP(target)`.
    PvpPrompt { team: bool, target: Identity },
}

/// `FUN_1005db15`: `identity_b.instance == 0` -> `Combat_PvPTargetLvl`, else `Combat_PvPTargetLvlTeam`.
pub const PVP_ACTION: i32 = 0x7b;

fn plain(key: &'static str) -> Vec<Event> {
    vec![Event::Line(Line::Plain(key))]
}

/// `FUN_1005b821` [GC 0x1005b821] (case 0x65 of `FUN_1005d0d8`): `identity_b.kind` is the sub-op, `identity_b.instance` its argument.
/// Only for the own character (the receiving dynel; the lines of sub-ops 1..4 are gated on `dynel+0x140`).
fn duel(a: &CharacterAction) -> Vec<Event> {
    let other = (a.identity_a.kind == CHAR).then_some(a.identity_a.instance);
    let inst = a.identity_b.instance;
    match (a.identity_b.kind, inst) {
        // the outer `if (other != 0)` of sub-op 0: both signals need the character the identity resolves to
        (0, 0) => other.map(Event::Challenged).into_iter().collect(),
        (0, 1) => other.map(Event::ChallengeSent).into_iter().collect(),
        (1, _) => vec![Event::Close, Event::Line(Line::Plain("Feedback_DuelAccepted"))],
        (2, i) => {
            let key = match i {
                0 => "Feedback_DuelRefused",
                1 => "Feedback_DuelRetracted",
                2 => "Feedback_DuelAutoRefused",
                _ => return vec![Event::Close],
            };
            vec![Event::Close, Event::Line(Line::Plain(key))]
        }
        (3, 2) => plain("Feedback_DuelDraw"),
        (3, 1) => plain("Feedback_DuelWon"),
        (3, 0) => plain("Feedback_DuelLost"),
        (4, 1) => plain("Feedback_DuelDrawProposed"),
        (4, 0) => plain("Feedback_DuelProposeDraw"),
        _ => vec![],
    }
}

/// `FUN_1005c514` [GC 0x1005c514] (case 0x62 of `FUN_1005d0d8`, the pet-duel ids): the System-window lines. `0xf1` (PetDuel_Stop) falls through
/// the id dispatch without an effect.
fn pet_duel(a: &CharacterAction) -> Vec<Event> {
    let other = (a.identity_a.kind == CHAR).then_some(a.identity_a.instance);
    let named_or = |key, otherwise| match other {
        Some(who) => vec![Event::Line(Line::Named { key, who, otherwise: Some(otherwise) })],
        None => vec![Event::Line(Line::Plain(otherwise))],
    };
    match a.action {
        // the original feeds the name of `other` without a null test; an unknown character prints nothing here
        net::PET_CHALLENGE => other.map(|who| Event::Line(Line::Named { key: "PetDuelChallenge", who, otherwise: None })).into_iter().collect(),
        net::PET_ANSWER => match a.identity_b.kind {
            0 => named_or("XRejectedDuel", "OpponentRejectedDuel"),
            1 => named_or("XAcceptedDuel", "OpponentAcceptedDuel"),
            2 => named_or("XinvalidOpponentDuel", "TargetNotValidOpponent"),
            3 => named_or("XbusyInDuel", "TargetBusy"),
            4 => plain("NotChallenged"),
            5 => plain("NeedDuelPet"),
            6 => plain("OpponentNeedsDuelPet"),
            _ => vec![],
        },
        net::PET_RESULT => match a.identity_b.kind {
            0 => plain("UwonPetDuel"),
            1 => plain("UlostPetDuel"),
            2 => plain("OpponentWithdrew"),
            _ => vec![],
        },
        net::PET_ANNOUNCE => named_or("ChallengedX2duel", "ChallengedSomeone"),
        _ => vec![],
    }
}

/// Reaction of the client to a received `CharacterActionIIR_t` that belongs to duels; empty for every other action.
pub fn on_action(a: &CharacterAction) -> Vec<Event> {
    match a.action {
        net::DUEL => duel(a),
        PVP_ACTION => vec![Event::PvpPrompt { team: a.identity_b.instance != 0, target: a.identity_a }],
        net::PET_CHALLENGE | net::PET_ANSWER | net::PET_RESULT | net::PET_ANNOUNCE => pet_duel(a),
        _ => vec![],
    }
}

/// `/duel` without a player targeted (`FUN_100b8a10`, printed with colour 0x51 = error).
pub const NEED_TARGET: &str = "You need to target a player first.";

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::msg::Identity;
    use ao_net::n3::action::simple;

    fn act(action: i32, a_kind: i32, a: i32, b_kind: i32, b: i32) -> CharacterAction {
        simple(action, Identity { kind: a_kind, instance: a }, Identity { kind: b_kind, instance: b })
    }

    fn lines(ev: Vec<Event>) -> Vec<String> {
        ev.into_iter()
            .map(|e| match e {
                Event::Line(Line::Plain(k)) => k.to_string(),
                Event::Line(Line::Named { key, who, otherwise }) => format!("{key}({who})/{otherwise:?}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn duel_sub_ops() {
        // challenge received: identity_a = challenger, identity_b = {0, 0}; confirmation of an own challenge: instance 1
        assert_eq!(on_action(&act(0x106, 0xC350, 7, 0, 0)), [Event::Challenged(7)]);
        assert_eq!(on_action(&act(0x106, 0xC350, 7, 0, 1)), [Event::ChallengeSent(7)]);
        // `other == 0` (identity_a not a character): nothing at all for sub-op 0
        assert!(on_action(&act(0x106, 0, 0, 0, 0)).is_empty());
        assert_eq!(lines(on_action(&act(0x106, 0, 0, 1, 0))), ["Close", "Feedback_DuelAccepted"]);
        for (i, k) in [(0, "Feedback_DuelRefused"), (1, "Feedback_DuelRetracted"), (2, "Feedback_DuelAutoRefused")] {
            assert_eq!(lines(on_action(&act(0x106, 0, 0, 2, i))), ["Close", k]);
        }
        assert_eq!(lines(on_action(&act(0x106, 0, 0, 3, 2))), ["Feedback_DuelDraw"]);
        assert_eq!(lines(on_action(&act(0x106, 0, 0, 3, 1))), ["Feedback_DuelWon"]);
        assert_eq!(lines(on_action(&act(0x106, 0, 0, 3, 0))), ["Feedback_DuelLost"]);
        assert!(on_action(&act(0x106, 0, 0, 3, 9)).is_empty());
        assert_eq!(lines(on_action(&act(0x106, 0, 0, 4, 1))), ["Feedback_DuelDrawProposed"]);
        assert_eq!(lines(on_action(&act(0x106, 0, 0, 4, 0))), ["Feedback_DuelProposeDraw"]);
        assert!(on_action(&act(0x106, 0, 0, 5, 0)).is_empty());
    }

    #[test]
    fn pvp_prompt() {
        let t = Identity { kind: 0xC350, instance: 5 };
        assert_eq!(on_action(&act(0x7b, 0xC350, 5, 0, 0)), [Event::PvpPrompt { team: false, target: t }]);
        assert_eq!(on_action(&act(0x7b, 0xC350, 5, 0, 3)), [Event::PvpPrompt { team: true, target: t }]);
    }

    #[test]
    fn pet_duel_lines() {
        assert_eq!(lines(on_action(&act(0xef, 0xC350, 9, 0, 0))), ["PetDuelChallenge(9)/None"]);
        assert!(on_action(&act(0xef, 0, 0, 0, 0)).is_empty());
        let keys = [
            (0, "XRejectedDuel", "OpponentRejectedDuel"),
            (1, "XAcceptedDuel", "OpponentAcceptedDuel"),
            (2, "XinvalidOpponentDuel", "TargetNotValidOpponent"),
            (3, "XbusyInDuel", "TargetBusy"),
        ];
        for (k, named, plain) in keys {
            assert_eq!(lines(on_action(&act(0xf0, 0xC350, 9, k, 0))), [format!("{named}(9)/Some({plain:?})")]);
            assert_eq!(lines(on_action(&act(0xf0, 0, 0, k, 0))), [plain]);
        }
        for (k, t) in [(4, "NotChallenged"), (5, "NeedDuelPet"), (6, "OpponentNeedsDuelPet")] {
            assert_eq!(lines(on_action(&act(0xf0, 0, 0, k, 0))), [t]);
        }
        assert!(on_action(&act(0xf0, 0, 0, 7, 0)).is_empty());
        for (k, t) in [(0, "UwonPetDuel"), (1, "UlostPetDuel"), (2, "OpponentWithdrew")] {
            assert_eq!(lines(on_action(&act(0xf3, 0, 0, k, 0))), [t]);
        }
        assert_eq!(lines(on_action(&act(0xf8, 0xC350, 4, 0, 0))), ["ChallengedX2duel(4)/Some(\"ChallengedSomeone\")"]);
        assert_eq!(lines(on_action(&act(0xf8, 0, 0, 0, 0))), ["ChallengedSomeone"]);
        // PetDuel_Stop is only ever sent
        assert!(on_action(&act(0xf1, 0, 0, 0, 0)).is_empty());
    }

    // ---- Module level: commands out, messages in ----

    use crate::play::combat::log::fake::Fixed;
    use crate::play::combat::module::Module;
    use crate::play::zone::{DynelState, Zone};
    use ao_net::frame::Frame;
    use ao_net::n3::action::{self, parse_character_action};
    use ao_net::n3::outgoing::n3_frame;

    const OWN: u32 = 25988;
    const PLAYER: i32 = 77;
    const NPC: i32 = 78;

    fn rig() -> (Module, Zone) {
        let mut z = Zone::new(OWN);
        for (id, npc) in [(OWN as i32, false), (PLAYER, false), (NPC, true)] {
            z.dynels.insert(id, DynelState { name: format!("n{id}"), pos: [0.0; 3], yaw: None, npc, side: 0, level: 1, health: 5, max_health: 5 });
        }
        (Module::with_texts(Box::new(Fixed::new()), OWN), z)
    }

    fn out(m: &mut Module) -> Vec<CharacterAction> {
        m.take_outbox().iter().map(|f: &Frame| parse_character_action(&f.payload).unwrap().1).collect()
    }

    fn recv(m: &mut Module, header: u32, a: CharacterAction) {
        m.on_frame(&n3_frame(0, 1, action::character_action(header as i32, &a)));
    }

    /// `/duel` needs a target that is a character, not the own one, not an NPC (GUI 0x100b8a10); `/petduel` takes any character (0x100b8789).
    #[test]
    fn challenge_needs_a_valid_target() {
        let (mut m, mut z) = rig();
        z.target = None;
        assert_eq!(m.duel_command(&z, false, Op::Challenge), Err(NEED_TARGET));
        assert_eq!(m.duel_command(&z, true, Op::Challenge), Err(NEED_TARGET));
        z.target = Some(NPC);
        assert_eq!(m.duel_command(&z, false, Op::Challenge), Err(NEED_TARGET));
        z.target = Some(OWN as i32);
        assert_eq!(m.duel_command(&z, false, Op::Challenge), Err(NEED_TARGET));
        assert!(out(&mut m).is_empty());
        assert_eq!(m.duel_command(&z, true, Op::Challenge), Ok(()));
        z.target = Some(PLAYER);
        assert_eq!(m.duel_command(&z, false, Op::Challenge), Ok(()));
        let sent = out(&mut m);
        assert_eq!((sent[0].action, sent[0].identity_a.instance), (0xef, OWN as i32));
        assert_eq!((sent[1].action, sent[1].identity_a, sent[1].identity_b), (0x106, Identity { kind: 0xC350, instance: PLAYER }, Identity::default()));
    }

    #[test]
    fn answers_close_the_dialog_and_pet_answers_do_not() {
        let (mut m, z) = rig();
        for (op, kind) in [(Op::Accept, 1), (Op::Refuse, 2), (Op::Stop, 3), (Op::Draw, 4)] {
            assert_eq!(m.duel_command(&z, false, op), Ok(()));
            let s = out(&mut m);
            assert_eq!((s[0].action, s[0].identity_b.kind), (0x106, kind));
            let closes = m.take_duel() == [Event::Close];
            assert_eq!(closes, matches!(op, Op::Accept | Op::Refuse));
        }
        for (op, action, kind) in [(Op::Accept, 0xf0, 1), (Op::Refuse, 0xf0, 0), (Op::Stop, 0xf1, 0)] {
            assert_eq!(m.duel_command(&z, true, op), Ok(()));
            let s = out(&mut m);
            assert_eq!((s[0].action, s[0].identity_b.kind), (action, kind));
            assert!(m.take_duel().is_empty());
        }
        m.duel_auto_refuse();
        assert_eq!(out(&mut m)[0].identity_b, Identity { kind: 2, instance: 1 });
    }

    /// Only messages addressed to the own character are applied.
    #[test]
    fn received_messages_reach_the_gui_layer() {
        let (mut m, _) = rig();
        recv(&mut m, OWN, act(0x106, 0xC350, PLAYER, 0, 0));
        recv(&mut m, 4, act(0x106, 0xC350, PLAYER, 0, 0));
        recv(&mut m, OWN, act(0xf8, 0xC350, PLAYER, 0, 0));
        assert_eq!(
            m.take_duel(),
            [Event::Challenged(PLAYER), Event::Line(Line::Named { key: "ChallengedX2duel", who: PLAYER, otherwise: Some("ChallengedSomeone") })]
        );
    }

    /// `CharacterAction` 0x64 hands an animation id to the header character's animation holder (any character, id 0 = nothing).
    #[test]
    fn action_0x64_queues_an_animation() {
        let (mut m, _) = rig();
        recv(&mut m, 4, act(0x64, 0, 0, 0, 510));
        recv(&mut m, OWN, act(0x64, 0, 0, 0, 0));
        recv(&mut m, OWN, act(0x64, 0, 0, 0, 503));
        assert_eq!(m.take_anims(), [(4, 510, None), (OWN as i32, 503, None)]);
        assert!(m.take_anims().is_empty());
    }

    /// The keys exist in the client's text database (category 110).
    #[test]
    fn keys_are_in_the_text_db() {
        let Some(h) = std::env::var_os("HOME") else { return };
        let Ok(db) = ao_formats::screens::TextDb::load(&std::path::Path::new(&h).join("Games/ProjectRubiKa/client")) else { return };
        let mut keys = vec![];
        for a in [(0x106, 0, 0), (0x106, 1, 0), (0x106, 2, 0), (0x106, 2, 1), (0x106, 2, 2), (0x106, 3, 0), (0x106, 3, 1), (0x106, 3, 2), (0x106, 4, 0), (0x106, 4, 1)] {
            keys.extend(on_action(&act(a.0, 0xC350, 1, a.1, a.2)));
        }
        for k in 0..7 {
            keys.extend(on_action(&act(0xf0, 0xC350, 1, k, 0)));
            keys.extend(on_action(&act(0xf0, 0, 0, k, 0)));
        }
        for k in 0..3 {
            keys.extend(on_action(&act(0xf3, 0, 0, k, 0)));
        }
        keys.extend(on_action(&act(0xef, 0xC350, 1, 0, 0)));
        keys.extend(on_action(&act(0xf8, 0xC350, 1, 0, 0)));
        keys.extend(on_action(&act(0xf8, 0, 0, 0, 0)));
        let mut n = 0;
        for e in keys {
            let Event::Line(l) = e else { continue };
            let ks = match l {
                Line::Plain(k) => vec![k],
                Line::Named { key, otherwise, .. } => std::iter::once(key).chain(otherwise).collect(),
            };
            for k in ks {
                assert!(db.by_key(110, k).is_some(), "{k}");
                n += 1;
            }
        }
        assert!(n > 25);
        assert_eq!(db.by_key(110, "Feedback_DuelChallenge").as_deref(), Some("%s has challenged you to a duel."));
        for (cat, k) in [(101, "Combat_PvPTargetLvl"), (101, "Combat_PvPTargetLvlTeam"), (10000, "MsgBox_Yes"), (10000, "MsgBox_No")] {
            assert!(db.by_key(cat, k).is_some(), "{k}");
        }
        for k in ["Feedback_DuelChallenge", "Feedback_DuelChallengeSent"] {
            assert!(db.by_key(110, k).is_some(), "{k}");
        }
        for k in ["MsgBox_Accept", "MsgBox_Reject", "MsgBox_Cancel"] {
            assert!(db.by_key(10000, k).is_some(), "{k}");
        }
    }
}
