//! Client -> zone-server combat messages and the client-side attack preconditions. RE evidence, addresses and
//! the key/mouse chain: docs/zone/combat-net.md.
//!
//! The three senders go through `n3Dynel_t::SendIIRToObservers` [N3 0x10004e43], which for a dynel that is in the
//! tree is exactly `n3EngineClient_t::SendIIRToServer` [N3 0x10007762] (`Write` [N3 0x100098f4] + ptype-10 frame).
//! Nothing is applied locally: the client waits for the server's relay (`AttackIIR_t` / `StopFightIIR_t`).

use crate::msg::Identity;
use crate::wire::Writer;

use super::outgoing::DYNEL_CHAR;

/// `AttackIIR_t` [GC 0x1007c641 ctor, 0x1007c5c0 writer]. Same key as the relayed message.
pub const ATTACK: u32 = super::misc::ATTACK;
/// `StopFightIIR_t` [GC 0x10079dd8 ctor, 0x10079d95 writer].
pub const STOP_FIGHT: u32 = super::misc::STOP_FIGHT;
/// `CharSecSpecAttackIIR_t` [GC 0x1007280c ctor, 0x100727a3 writer].
pub const CHAR_SEC_SPEC_ATTACK: u32 = super::misc::CHAR_SEC_SPEC_ATTACK;

/// `Feedback_PleaseWaitUntilPreviousAction` text key printed by `FUN_10067c34` [GC] when the attack guard is set.
pub const FB_PLEASE_WAIT: &str = "Feedback_PleaseWaitUntilPreviousAction";

/// The header every one of these messages shares: key, `Identity_t` of the sending character, and the
/// "to be passed on" byte. All three constructors end in `n3InfoItemRemote_t::ClearToBePassedOn` [N3 0x100097fe]
/// (`this+0xc = 0`) and `Write` emits `(this+0xc == 1)`, so the byte is **0** (unlike `CharInPlay`/`CharDCMove`,
/// whose constructors leave it 1, see `outgoing::iir`). Server relays of the same classes carry 0 too.
fn header(key: u32, char_id: i32, body: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.u32(key);
    Identity { kind: DYNEL_CHAR, instance: char_id }.write(&mut w);
    w.u8(0);
    body(&mut w);
    w.0
}

/// `FUN_10067c34` [GC] -> `AttackIIR_t(char identity +0x14, target, flag)`. `flag` is 2 when the character was
/// already fighting (target switch) and 0 otherwise (`N3Msg_DefaultAttack` [GC 0x10027fbc]). 22 bytes.
pub fn attack(char_id: i32, target: Identity, flag: i8) -> Vec<u8> {
    header(ATTACK, char_id, |w| {
        target.write(w);
        w.u8(flag as u8);
    })
}

/// `LookAtIIR_t` 0x2252445F: the client's target selection (`FUN_1003b73e` [GC 0x1003b73e] for a character, mode 1;
/// `FUN_1003b0db` for anything else / no target, mode 0), sent by `FUN_1003fb35` [GC] whenever the selection changes.
/// Ctor `FUN_10074f69` [GC] (`this[0xc] = 0`), write `FUN_10074f41`: `Identity target`, `i32 mode`. 25 bytes.
pub const LOOK_AT: u32 = 0x2252_445F;

pub fn look_at(char_id: i32, target: Identity, mode: i32) -> Vec<u8> {
    header(LOOK_AT, char_id, |w| {
        target.write(w);
        w.i32(mode);
    })
}

/// `N3Msg_StopAttack` [GC 0x10027f55] -> `StopFightIIR_t(char identity, 1)`. The body is written with
/// `BinaryStream::operator<<(uint)` of the stored byte, i.e. a big-endian `i32 1` (captured relays: `00 00 00 01`). 17 bytes.
pub fn stop_fight(char_id: i32) -> Vec<u8> {
    header(STOP_FIGHT, char_id, |w| w.i32(1))
}

/// `N3Msg_SecondarySpecialAttack` [GC 0x10028071] -> `CharSecSpecAttackIIR_t(char identity, target, stat)`;
/// `special` is the special-attack skill stat (142 Brawl, 144 Dimach, ...). 25 bytes.
pub fn sec_spec_attack(char_id: i32, target: Identity, special: i32) -> Vec<u8> {
    header(CHAR_SEC_SPEC_ATTACK, char_id, |w| {
        target.write(w);
        w.i32(special);
    })
}

/// Special-action ids (`ACTION_*` providers of `ControlCenterModule_c::SetupProviders` [GUI 0x10068c38], the argument of
/// `N3Msg_PerformSpecialAction` [GC 0x1004256c]).
pub mod action {
    pub const ATTACK: i32 = 0xb;
    pub const USE: i32 = 3;
    pub const SNEAK: i32 = 0x13;
    pub const SIT: i32 = 0x4c;
    pub const RELOAD: i32 = 0x6e;
    pub const BOW_SPECIAL_ATTACK: i32 = 0x79;
    pub const BRAWL: i32 = 0x8e;
    pub const DIMACH: i32 = 0x90;
    pub const SNEAK_ATTACK: i32 = 0x92;
    pub const FAST_ATTACK: i32 = 0x93;
    pub const BURST: i32 = 0x94;
    pub const FLING_SHOT: i32 = 0x96;
    pub const AIMED_SHOT: i32 = 0x97;
    pub const FULL_AUTO: i32 = 0xa7;
}

/// Movement mode values read by `can_attack` (`FUN_100704e6(*(vehicle+0x178))` = `*(controller+4)`).
pub const MODE_SWIMMING: u32 = 4;
/// Mode 7: `FUN_1006dd0c` [GC] calls `Vehicle_t::EnableFalling` for it ([INFERENCE]: falling).
pub const MODE_FALLING: u32 = 7;

/// `Features` stat 224 (`0xe0`) bits tested by `FUN_10044b6e` [GC] on the dynel behind `char+0x1ec`.
pub mod features {
    /// Target cannot be attacked here -> `Feedback_CantbeAttacked`.
    pub const CANT_BE_ATTACKED: u32 = 0x2000_0000;
    /// With `GmLevel` (215) != 0: attacking always allowed.
    pub const GM_OVERRIDE: u32 = 0x8000_0000;
    /// Attacker's area forbids attacking -> `Feedback_YoureUnableToAttack`.
    pub const NO_ATTACK: u32 = 0x400;
    /// Read (result unused) by the PvP tail of `FUN_10069556`.
    pub const UNUSED_PVP: u32 = 0x400_0000;
}

/// `NPCIsSurrendering` (449) values.
pub const SURRENDER_ASKING: i32 = 1;
pub const SURRENDER_DONE: i32 = 2;

/// What `FUN_10069556` needs to know about the attacker (`param_1+8`, the client character).
#[derive(Debug, Clone, Default)]
pub struct Attacker {
    pub id: i32,
    /// `char+0x140`: `IsClientChar` — only the local character gets feedback texts.
    pub is_client: bool,
    /// `char+0x21c`.
    pub flag_21c: bool,
    /// Movement mode of the character's vehicle ([`MODE_SWIMMING`], [`MODE_FALLING`]).
    pub mode: u32,
    /// `GmLevel` stat 215.
    pub gm_level: i32,
    /// `Features` (224) of the area object at `char+0x1ec`.
    pub area_features: u32,
    /// `Side` stat 33 (3 = neutral).
    pub side: i32,
    /// Opening gate of `FUN_10069556`: controller `+0x38 == 0` and its condition map holds key 0x3d or 0x3f
    /// ([INFERENCE] stun/root style conditions) *and* `VisualFlags` (673) bit 5 is set -> no attack, silently.
    pub status_blocked: bool,
    /// District fight-mode level (`FUN_1003e1d0` [GC], default 2; overridden by `char+0xf4` of the state object when
    /// `char+0x13a & 1`).
    pub fight_level: i32,
    /// `char+0x1e0 != 0`: the character has a team record.
    pub in_team: bool,
}

/// What the target resolves to (`GetDynel`, then `SimpleChar_t` / `SimpleItem_t` dynamic casts).
#[derive(Debug, Clone)]
pub enum Target {
    /// Neither a `SimpleChar` nor a `SimpleItem`, or not in the dynel tree: silent `false`.
    Unattackable,
    /// A `SimpleItem`; `usable` is `vtable+0x14(0x10000000)` ? `FUN_100811ac(0xb, ..)` : false.
    Item { usable_as_target: bool, flag_10000000: bool },
    Char(CharTarget),
}

#[derive(Debug, Clone, Default)]
pub struct CharTarget {
    pub id: i32,
    /// `n3Dynel_t::IsInTree`.
    pub in_tree: bool,
    /// `char+0x21c`.
    pub flag_21c: bool,
    /// `dynel+0x138` bit 4.
    pub dead_flag: bool,
    /// `Health` stat 27.
    pub health: i32,
    pub gm_level: i32,
    pub area_features: u32,
    /// `NPCIsSurrendering` (449) and `NPCSurrenderInstance` (451).
    pub surrendering: i32,
    pub surrender_instance: i32,
    /// The target's own fight target (`FUN_100676bd(target+0x1d4)`) is the attacker: retaliation is always allowed.
    pub fighting_attacker: bool,
    pub fight_level: i32,
    /// Target is in the attacker's team (`FUN_1006581f` / `FUN_10065865`).
    pub teammate: bool,
}

/// Result of [`can_attack`]: `feedback` lists the `Feedback_*` text keys the client prints (in order); only the local
/// client character gets them and only when `report` is set (`FUN_10058b00` is skipped otherwise).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Verdict {
    pub allowed: bool,
    pub feedback: Vec<&'static str>,
}

impl Verdict {
    fn yes() -> Self {
        Verdict { allowed: true, feedback: vec![] }
    }
    fn no(me: &Attacker, report: bool, text: &'static str) -> Self {
        Verdict { allowed: false, feedback: if me.is_client && report { vec![text] } else { vec![] } }
    }
    fn silent() -> Self {
        Verdict::default()
    }
}

/// Port of `FUN_10069556(controller, target, report)` [GC 0x10069556] ("CanAttack"). `report` = the second stack
/// argument: 1 from `N3Msg_DefaultAttack`/`SecondarySpecialAttack`, 0 from `FUN_10069c68` (applying the echo).
/// Inputs that the original reads out of live dynels are passed in [`Attacker`]/[`Target`]; see the doc for which
/// callee each field replaces.
pub fn can_attack(me: &Attacker, target: &Target, report: bool) -> Verdict {
    if me.status_blocked {
        return Verdict::silent();
    }
    let t = match target {
        Target::Item { usable_as_target, flag_10000000 } => {
            return if *flag_10000000 {
                Verdict { allowed: *usable_as_target, feedback: vec![] }
            } else {
                Verdict::no(me, report, "Feedback_YouCantAttackThisItem")
            };
        }
        Target::Unattackable => return Verdict::silent(),
        Target::Char(t) => t,
    };
    if !t.in_tree {
        return Verdict::silent();
    }
    if t.id == me.id {
        return Verdict::no(me, report, "Feedback_YouCantAttackYourself");
    }
    if t.area_features & features::CANT_BE_ATTACKED != 0 {
        // Not gated on `report`/`is_client`: `Feedback_CantbeAttacked` is printed whenever `char+0x21c == 0`.
        return if me.flag_21c {
            Verdict::silent()
        } else {
            Verdict { allowed: false, feedback: vec!["Feedback_CantbeAttacked"] }
        };
    }
    if (me.area_features & features::GM_OVERRIDE != 0 && me.gm_level != 0)
        || (t.area_features & features::GM_OVERRIDE != 0 && t.gm_level != 0)
    {
        return Verdict::yes();
    }
    if !me.flag_21c && me.mode == MODE_SWIMMING {
        return Verdict::no(me, report, "Feedback_YouCantAttackWhileSwimming");
    }
    if me.area_features & features::NO_ATTACK != 0 || me.mode == MODE_FALLING {
        return Verdict::no(me, report, "Feedback_YoureUnableToAttack");
    }
    if t.dead_flag {
        return if t.health != 0 { Verdict::silent() } else { Verdict::no(me, report, "Feedback_TargetIsAlreadyDead") };
    }
    if t.flag_21c && (t.surrendering == SURRENDER_ASKING || t.surrendering == SURRENDER_DONE) {
        if t.surrender_instance == me.id {
            if t.surrendering == SURRENDER_DONE {
                return Verdict::no(me, report, "Feedback_YouAcceptedSurrender");
            }
        } else if t.surrendering == SURRENDER_ASKING {
            return Verdict::no(me, report, "Feedback_TargetIsSurrendering");
        } else {
            return Verdict::no(me, report, "Feedback_TargetHasSurrendered");
        }
    }
    // Tail at 0x1006991e.
    if t.fighting_attacker {
        return Verdict::yes();
    }
    let mut v = Verdict::yes();
    if t.fight_level < me.fight_level && me.is_client && report {
        v.feedback.push("Feedback_TargetIsInADistrictWithHigherSuppression");
    }
    if me.flag_21c && me.side == 3 {
        v.allowed = me.fight_level != 0 && t.fight_level != 0;
        return v;
    }
    if me.in_team && t.teammate {
        // `FUN_10068d6b(5)` (a CharacterAction request, id 0x76) is sent when `report`; not modelled.
        v.allowed = false;
    }
    v
}

/// The `char+0x79` byte of the fight controller (`CharFight`, the object at `SimpleChar+0x1d4`).
/// Set by `FUN_10067c34` [GC 0x10067c34] when an `AttackIIR_t` is sent; there is no timer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttackGate {
    pub pending: bool,
}

impl AttackGate {
    /// `FUN_10067c34` guard: `+0x79 == 0 && char+0x21d == 0` -> set and send, else `Feedback_PleaseWaitUntilPreviousAction`.
    pub fn begin(&mut self, char_flag_21d: bool) -> Result<(), &'static str> {
        if self.pending || char_flag_21d {
            return Err(FB_PLEASE_WAIT);
        }
        self.pending = true;
        Ok(())
    }

    /// `FUN_10069c68` [GC] (the echo of `AttackIIR_t`, the full-update fight target, `CharFight` ctor) clears it
    /// at 0x10069cdb, after `can_attack(.., report = false)` succeeded; when that fails the flag stays.
    pub fn on_attack_applied(&mut self, can_attack_ok: bool) {
        if can_attack_ok {
            self.pending = false;
        }
    }

    /// `FormatFeedbackIIR` categories 0x23, 0x26 and 0x31 clear it (`FUN_1005d0d8` [GC]); 0x31 also prints
    /// `Feedback_StartingAttackFailed` (`FUN_10067978`). Returns the text to print.
    pub fn on_format_feedback(&mut self, category: i32) -> Option<&'static str> {
        match category {
            0x23 | 0x26 => self.pending = false,
            0x31 => {
                self.pending = false;
                return Some("Feedback_StartingAttackFailed");
            }
            _ => {}
        }
        None
    }

    /// `FUN_10059949` [GC] (SimpleChar flag-event handler) clears it for event bit `0x20000000`.
    pub fn on_flag_event(&mut self, flags: u32) {
        if flags & 0x2000_0000 != 0 {
            self.pending = false;
        }
    }
}

/// Fight controller state read by [`default_attack`] (`SimpleChar+0x1d4`: `+0x44` state, `+0x4c/+0x50` target).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fight {
    pub fighting: bool,
    pub target: Identity,
}

/// What `N3Msg_DefaultAttack(target, switch)` [GC 0x10027fbc] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultAttack {
    /// `N3Msg_StopAttack` ran first (only if the character has `char+0x21d == 0`; the fight state stays "fighting"
    /// until the server's `StopFightIIR_t` arrives).
    pub stop_first: bool,
    pub outcome: DefaultOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultOutcome {
    /// Return without further action (attack key while fighting just stops; same target with `switch`).
    Stopped,
    /// `Feedback_CantAttackInThisState` (swimming) or `Feedback_UnableToAttackTarget` (+ whatever `can_attack` printed).
    Refused(Vec<&'static str>),
    /// Send `AttackIIR_t(target, flag)` through the [`AttackGate`].
    Send { flag: i8 },
}

/// Port of `N3Msg_DefaultAttack`. `can` is `can_attack(.., report = true)`, evaluated by the caller only when needed
/// (pass `None` before it is known; it is only called in the `Send` path).
pub fn default_attack(
    fight: Fight,
    target: Identity,
    switch: bool,
    mode: u32,
    can: impl FnOnce() -> Verdict,
) -> DefaultAttack {
    let same = fight.target == target;
    let was_fighting = fight.fighting;
    let stop_first = was_fighting;
    // N3Msg_StopAttack does not change the local state, so the second state test still sees "fighting".
    if mode == MODE_SWIMMING {
        return DefaultAttack { stop_first, outcome: DefaultOutcome::Refused(vec!["Feedback_CantAttackInThisState"]) };
    }
    if fight.fighting && (!switch || same) {
        return DefaultAttack { stop_first, outcome: DefaultOutcome::Stopped };
    }
    let v = can();
    if !v.allowed {
        let mut fb = v.feedback;
        fb.push("Feedback_UnableToAttackTarget");
        return DefaultAttack { stop_first, outcome: DefaultOutcome::Refused(fb) };
    }
    DefaultAttack { stop_first, outcome: DefaultOutcome::Send { flag: if was_fighting { 2 } else { 0 } } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::misc::{Misc, StopFight};
    use crate::n3::{capture_n3, decode, N3};

    fn id(instance: i32) -> Identity {
        Identity { kind: DYNEL_CHAR, instance }
    }

    /// The server relays the client's messages with the same class layout and `to_be_passed_on = 0`, so the
    /// encoders must reproduce every captured relay byte-for-byte.
    #[test]
    fn look_at_layout() {
        let t = Identity { kind: DYNEL_CHAR, instance: 0x1234 };
        let b = look_at(7, t, 1);
        assert_eq!(b.len(), 25);
        assert_eq!(&b[..4], &[0x22, 0x52, 0x44, 0x5f]);
        assert_eq!(&b[b.len() - 12..], &[0, 0, 0xc3, 0x50, 0, 0, 0x12, 0x34, 0, 0, 0, 1]);
    }

    #[test]
    fn encoders_reproduce_captured_relays() {
        let (mut a, mut s, mut c) = (0, 0, 0);
        for f in capture_n3() {
            let m = decode(&f).unwrap();
            let N3::Misc(misc) = m.body else { continue };
            let from = m.header.target;
            if from.kind != DYNEL_CHAR {
                continue;
            }
            let want = match &misc {
                Misc::Attack(x) => {
                    a += 1;
                    attack(from.instance, x.target, x.flag)
                }
                Misc::StopFight(StopFight { flag: true }) => {
                    s += 1;
                    stop_fight(from.instance)
                }
                Misc::CharSecSpecAttack(x) => {
                    c += 1;
                    sec_spec_attack(from.instance, x.target, x.special)
                }
                _ => continue,
            };
            assert_eq!(want, f.payload);
        }
        assert_eq!((s, c), (128, 3));
        assert!(a > 0);
    }

    #[test]
    fn lengths_and_flag_byte() {
        let t = id(0xFA8C9);
        assert_eq!(attack(0x827A, t, 2).len(), 22);
        assert_eq!(attack(0x827A, t, 2)[12], 0);
        assert_eq!(*attack(0x827A, t, 2).last().unwrap(), 2);
        assert_eq!(stop_fight(0x827A).len(), 17);
        assert_eq!(stop_fight(0x827A)[13..], [0, 0, 0, 1]);
        assert_eq!(sec_spec_attack(0x827A, t, 142).len(), 25);
        assert_eq!(&attack(1, t, 0)[..4], &0x2849_4070u32.to_be_bytes());
    }

    fn me() -> Attacker {
        Attacker { id: 1, is_client: true, fight_level: 2, ..Default::default() }
    }
    fn mob() -> CharTarget {
        CharTarget { id: 2, in_tree: true, health: 10, fight_level: 2, ..Default::default() }
    }

    #[test]
    fn can_attack_rules() {
        let ok = can_attack(&me(), &Target::Char(mob()), true);
        assert!(ok.allowed && ok.feedback.is_empty());
        let selfie = CharTarget { id: 1, ..mob() };
        assert_eq!(can_attack(&me(), &Target::Char(selfie.clone()), true).feedback, ["Feedback_YouCantAttackYourself"]);
        assert!(can_attack(&me(), &Target::Char(selfie), false).feedback.is_empty());
        let swim = Attacker { mode: MODE_SWIMMING, ..me() };
        assert_eq!(can_attack(&swim, &Target::Char(mob()), true).feedback, ["Feedback_YouCantAttackWhileSwimming"]);
        let dead = CharTarget { dead_flag: true, health: 0, ..mob() };
        assert_eq!(can_attack(&me(), &Target::Char(dead), true).feedback, ["Feedback_TargetIsAlreadyDead"]);
        let gm = Attacker { area_features: features::GM_OVERRIDE, gm_level: 1, ..me() };
        let protected = CharTarget { area_features: 0, ..mob() };
        assert!(can_attack(&gm, &Target::Char(protected), true).allowed);
        let cant = CharTarget { area_features: features::CANT_BE_ATTACKED, ..mob() };
        assert_eq!(can_attack(&me(), &Target::Char(cant), false).feedback, ["Feedback_CantbeAttacked"]);
        let item = Target::Item { usable_as_target: false, flag_10000000: false };
        assert_eq!(can_attack(&me(), &item, true).feedback, ["Feedback_YouCantAttackThisItem"]);
        assert!(!can_attack(&me(), &Target::Unattackable, true).allowed);
    }

    #[test]
    fn gate_follows_the_echo() {
        let mut g = AttackGate::default();
        assert_eq!(g.begin(false), Ok(()));
        assert_eq!(g.begin(false), Err(FB_PLEASE_WAIT));
        g.on_attack_applied(false);
        assert!(g.pending);
        g.on_attack_applied(true);
        assert_eq!(g.begin(false), Ok(()));
        assert_eq!(g.on_format_feedback(0x31), Some("Feedback_StartingAttackFailed"));
        assert!(!g.pending);
        assert_eq!(g.begin(true), Err(FB_PLEASE_WAIT));
    }

    #[test]
    fn default_attack_toggle_and_switch() {
        let t = id(7);
        let idle = Fight { fighting: false, target: id(0) };
        let go = || Verdict::yes();
        assert_eq!(default_attack(idle, t, false, 0, go).outcome, DefaultOutcome::Send { flag: 0 });
        let busy = Fight { fighting: true, target: t };
        let r = default_attack(busy, t, false, 0, go);
        assert!(r.stop_first && r.outcome == DefaultOutcome::Stopped);
        assert_eq!(default_attack(busy, t, true, 0, go).outcome, DefaultOutcome::Stopped);
        let other = Fight { fighting: true, target: id(8) };
        let r = default_attack(other, t, true, 0, go);
        assert!(r.stop_first && r.outcome == DefaultOutcome::Send { flag: 2 });
        assert_eq!(
            default_attack(idle, t, false, MODE_SWIMMING, go).outcome,
            DefaultOutcome::Refused(vec!["Feedback_CantAttackInThisState"])
        );
        let no = || Verdict { allowed: false, feedback: vec!["Feedback_TargetIsAlreadyDead"] };
        assert_eq!(
            default_attack(idle, t, false, 0, no).outcome,
            DefaultOutcome::Refused(vec!["Feedback_TargetIsAlreadyDead", "Feedback_UnableToAttackTarget"])
        );
    }
}
