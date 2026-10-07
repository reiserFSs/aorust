//! The combat / action layer of the own client: feeds the received N3 frames to the fight controller
//! ([`super::state::Combat`]) and the pose tracker ([`super::actions::Actions`]), turns the player's commands (attack
//! key, hotbar special action, sit) into the original's outgoing messages (`ao_net::n3::combat` / `action`), keeps the
//! floating damage numbers and feeds the combat-music state machine. Message flow and addresses: docs/zone/combat-net.md,
//! combat-log.md, combat-anim.md, actions.md.

use super::actions::{Actions, Event as ActionEvent};
use super::anim::{plays_hit_sound, special_swing, DieEvent, Dying, ACTION_DEATH_DONE, ACTION_HIT, DEFAULT_DEATH_ANIM, STAT_DEATH_ANIM, WIELD_GESTURE};
use super::arms::ACTION_UNWIELD;
use super::log::{floating_number, FloatingNumber, Space, HUD_X, HUD_X_JITTER};
use super::state::{Combat, CombatEvent, ACTION_PLAY_ANIM, FIGHT_IDLE};
use crate::play::zone::{DynelState, Zone};
use ao_audio::combat::CharInfo;
use ao_audio::Audio;
use ao_formats::screens::TextDb;
use ao_net::frame::Frame;
use ao_net::n3::action::{self, simple};
use ao_net::n3::combat::{self as net, action as act, Attacker, AttackGate, CharTarget, DefaultOutcome, Fight, Target};
use ao_net::n3::outgoing::{n3_frame, DYNEL_CHAR};
use ao_net::msg::Identity;
use std::path::Path;

/// `ColorCode_e` of the special-attack text above a character (`FUN_10011108(char, text, 0xd)` [GC 0x1003c594]).
const SPECIAL_TEXT_CATEGORY: u32 = 0xd;

/// What the player asked for (key bindings `ACTION_*`, hotbar special actions, mouse).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// `ACTION_ATTACK` (Q): `FUN_1004256c` case 0xb -> `N3Msg_DefaultAttack(selected target, false)`; toggles when fighting.
    Attack,
    /// `ACTION_SWITCHTARGET` (CTRL+Q) / CTRL-click: `N3Msg_SwitchTarget(id)` = `DefaultAttack(id, true)`.
    SwitchTarget(i32),
    /// A special attack of `FUN_1004256c` / `ACTION_BRAWL` ...: `N3Msg_SecondarySpecialAttack(target, stat)`.
    Special(i32),
}

/// A floating number on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Number {
    pub dynel: i32,
    pub text: String,
    pub spec: FloatingNumber,
    /// Seconds since creation.
    pub age: f32,
    /// HUD path: horizontal jitter in pixels (`50 + r`, `r` in -20..=20).
    pub jitter: f32,
}

impl Number {
    /// Rise since creation: pixels (HUD) or metres (world).
    pub fn rise(&self) -> f32 {
        self.spec.rise_speed * self.age
    }
}

/// Everything of the combat layer that lives as long as the zone connection.
pub struct Module {
    combat: Combat,
    actions: Actions,
    gate: AttackGate,
    own: i32,
    outbox: Vec<Frame>,
    feedback: Vec<&'static str>,
    numbers: Vec<Number>,
    dying: Option<Dying>,
    rng: u32,
    /// `s_nCommandRefCntr` [GC]: counter of the `n3Command_t`s the client sent (`SocialActionCmd_t.counter`).
    counter: i32,
    /// The selection last announced to the server with `LookAtIIR_t`.
    announced: Option<Identity>,
    /// Characters hit by an `0xd1` `CharacterAction` (sound cue, [`Module::take_struck`]).
    struck: Vec<i32>,
    /// `CharacterAction` 0x64: `(dynel, AbstractAnimID)` the server asks to play ([`Module::take_anims`]).
    anims: Vec<(i32, u16)>,
    /// Events of the last received frames for the HUD / sounds (taken by [`Module::take_events`]).
    events: Vec<CombatEvent>,
    pose_events: Vec<ActionEvent>,
    /// Duel / pet-duel reactions of the last received frames ([`Module::take_duel`]).
    duel: Vec<super::duel::Event>,
}

/// `Feedback_*` texts of the server's attack refusal (`CharacterAction` 0x76, `FUN_1005d0d8` case 0x23 @ 0x1005d92b): the jump table at
/// 0x1005f0e7 maps `identity_a.instance` 1..=14 to these strings (read from the table, in this order).
const ATTACK_REFUSED: [&str; 14] = [
    "Feedback_CombatIsNotPossibleInThisDistrict",
    "Feedback_AttackNotAllowedSinceYouAreOnSameSide",
    "Feedback_YouCannotAttackThisPlayerTooFarAwayInLevel",
    "Feedback_YouCannotAttackYourPet",
    "Feedback_NotAllowedToAttackTeamMembers",
    "Feedback_PvpNotAllowedInThisDistrict",
    "Feedback_PvpNotAllowedSinceYouAreNeutral",
    "Feedback_PvpNotAllowedSinceYourTeamIsNeutral",
    "Feedback_CantAttackTargetIsInPvpGrace",
    "Feedback_DefenseShieldEnabled",
    "Feedback_YouCannotAttackThisTowerTooFarAwayInLevel",
    "Feedback_CantAttackTargetYouAreInMixedTeam",
    "Feedback_TowersCanOnlyBeAttackedWhenGaslevelBelow75",
    "Feedback_CantAttackTargetYouAreInMixedTeamBattlestation",
];

struct NoTexts;
impl super::log::Texts for NoTexts {
    fn feedback(&self, _: &str) -> Option<String> {
        None
    }
    fn stat_name(&self, _: u32) -> Option<String> {
        None
    }
}

impl Module {
    /// `dir` = client directory (the text database gives the formatter its templates).
    pub fn new(dir: &Path, own: u32) -> Self {
        let texts: Box<dyn super::log::Texts> = match TextDb::load(dir) {
            Ok(t) => Box::new(t),
            Err(e) => {
                eprintln!("combat: text database unavailable ({e:#}); no floating numbers");
                Box::new(NoTexts)
            }
        };
        let mut m = Self::with_texts(texts, own);
        m.combat.arms = super::arms::Armory::open(dir);
        m
    }

    pub fn with_texts(texts: Box<dyn super::log::Texts>, own: u32) -> Self {
        Self {
            combat: Combat::new(texts),
            actions: Actions::new(),
            gate: AttackGate::default(),
            own: own as i32,
            outbox: Vec::new(),
            feedback: Vec::new(),
            numbers: Vec::new(),
            dying: None,
            rng: 0x2545_F491 ^ own,
            counter: 0,
            announced: None,
            struck: Vec::new(),
            anims: Vec::new(),
            events: Vec::new(),
            pose_events: Vec::new(),
            duel: Vec::new(),
        }
    }

    #[cfg(test)]
    fn combat(&self) -> &Combat {
        &self.combat
    }

    /// The own character is in a fight (`N3Msg_IsAttacking`).
    pub fn attacking(&self) -> bool {
        self.combat.is_fighting(self.own)
    }

    /// Character `id`'s fight controller is in state 2 (`SimpleChar+0x1d4 +0x44`).
    pub fn is_fighting(&self, id: i32) -> bool {
        self.combat.is_fighting(id)
    }

    /// `FUN_100688f9`: resolve the fight target when the animation note fires.
    pub fn note_target(&self, id: i32) -> Option<i32> {
        let fight = self.combat.fight(id)?;
        fight.target.or(fight.last_target).map(|target| target.instance)
    }

    /// Frames to send to the zone server.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// `Feedback_*` keys the original prints through `FUN_10058b00` (system chat line).
    pub fn take_feedback(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.feedback)
    }

    pub fn take_events(&mut self) -> Vec<CombatEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn take_pose_events(&mut self) -> Vec<ActionEvent> {
        std::mem::take(&mut self.pose_events)
    }

    /// Characters struck by an `0xd1` `CharacterAction` since the last call (their hit sound plays).
    pub fn take_struck(&mut self) -> Vec<i32> {
        std::mem::take(&mut self.struck)
    }

    /// `FUN_1005d0d8` case 0x1a (action 0x64, handler 0x1005d873): `FUN_1003c47c(identity_b.instance)` = the animation holder (`char+0x1dc`) gets a new
    /// animation id, which its idle update plays (the same setter as the emote path). 0 = nothing to play.
    pub fn take_anims(&mut self) -> Vec<(i32, u16)> {
        std::mem::take(&mut self.anims)
    }

    /// The own character is dead (`CharacterAction` 99 seen, not resurrected yet): the live harness ends a `goto` with it.
    #[cfg(test)]
    pub fn is_dying(&self) -> bool {
        self.dying.is_some()
    }

    /// Animation id of the own death: `CharacterAction` 99's `identity_b.instance` (stat 0x183), [`DEFAULT_DEATH_ANIM`] when the server sent none.
    pub fn death_anim(&self) -> u16 {
        self.dying.as_ref().map_or(DEFAULT_DEATH_ANIM, |d| d.anim)
    }

    pub fn numbers(&self) -> &[Number] {
        &self.numbers
    }

    fn rand01(&mut self) -> f32 {
        // xorshift32; the original uses the C `rand()` (`RenderText_t` ctor), the stream is unseeded and shared
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// One received frame (call before `Zone::on_frame` so the zone's stats are still the old ones, like the chat log).
    pub fn on_frame(&mut self, f: &Frame) {
        if let Ok(m) = ao_net::n3::decode(f) {
            if let ao_net::n3::N3::World(ao_net::n3::world::World::CharacterAction(a)) = &m.body {
                if m.header.target.instance == self.own {
                    match a.action {
                        // `FUN_1005d0d8` case 0x31 (action 0x93): "starting the attack failed", clears the `+0x79` guard
                        0x93 => self.feedback.extend(self.gate.on_format_feedback(0x31)),
                        // action 0x76: the server refused the attack; `identity_a.instance` selects the text (jump table 0x1005f0e7)
                        0x76 => {
                            self.gate.pending = false;
                            if let Some(k) = usize::try_from(a.identity_a.instance - 1).ok().and_then(|i| ATTACK_REFUSED.get(i)) {
                                self.feedback.push(k);
                            }
                        }
                        // action 0x7b (`FUN_1005d0d8` case 0x26): clears the `+0x79` guard of the char `identity_a` names, then asks the player (below)
                        super::duel::PVP_ACTION => {
                            if a.identity_a.kind == DYNEL_CHAR && a.identity_a.instance == self.own {
                                self.gate.pending = false;
                            }
                            self.duel.extend(super::duel::on_action(a));
                        }
                        // duel / pet-duel messages (0x106, 0xef, 0xf0, 0xf3, 0xf8): lines and dialogs for the GUI
                        _ => self.duel.extend(super::duel::on_action(a)),
                    }
                }
                if a.action == ACTION_HIT && plays_hit_sound(a.identity_a.instance, a.identity_b.instance) {
                    self.struck.push(m.header.target.instance);
                }
                if a.action == ACTION_PLAY_ANIM && m.header.target.kind == DYNEL_CHAR && a.identity_b.instance > 0 {
                    self.anims.push((m.header.target.instance, a.identity_b.instance as u16));
                }
                // `FUN_1006a857` (action 0x61): after the unwield `FUN_10081e74(char, 3)` plays the wield gesture 0x6d once; a fight (`+0x44` != 1) then runs
                // the idle update `FUN_1006a772` -> `FUN_1003cad0`, which starts the idle clip over it in the same frame (docs/zone/combat-anim.md §4)
                if a.action == ACTION_UNWIELD && m.header.target.kind == DYNEL_CHAR {
                    let who = m.header.target.instance;
                    if self.combat.arms.slot_item(who, a.identity_b.instance).is_some() && !self.combat.is_fighting(who) {
                        self.anims.push((who, WIELD_GESTURE));
                    }
                }
            }
        }
        let ev = self.combat.on_frame(f, self.own as u32);
        for e in ev {
            match &e {
                // `FUN_1003c594` [GC 0x1003c594]: the special's name floats above the character (`FUN_10011108(char, text, 0xd)`, the world
                // effect path; the client's "\n" ends the text)
                CombatEvent::SpecialAttack { who, special, .. } => {
                    if let Some(text) = special_swing(*special).and_then(|s| s.text) {
                        self.numbers.push(Number { dynel: *who, text: text.trim_end().to_string(), spec: floating_number(Space::World, SPECIAL_TEXT_CATEGORY), age: 0.0, jitter: 0.0 });
                    }
                }
                CombatEvent::Floating { dynel, amount, number, .. } => {
                    let jitter = if number.space == Space::Hud { HUD_X as f32 + (self.rand01() * 2.0 - 1.0) * HUD_X_JITTER as f32 } else { 0.0 };
                    self.numbers.push(Number { dynel: *dynel, text: amount.to_string(), spec: *number, age: 0.0, jitter });
                }
                CombatEvent::FightStarted { who, .. } if *who == self.own => {
                    // FUN_10069c68 cleared the controller's +0x79 guard after CanAttack(report = 0) passed
                    self.gate.on_attack_applied(true);
                }
                CombatEvent::Died { dynel, .. } if *dynel == self.own && self.dying.is_none() => {
                    // `CharDie_t` ctor: the animation is `GetSkill(0x183)` (set by the server's action 99; 0 for a death the client computed itself)
                    let anim = self.combat.char(self.own).map_or(0, |c| c.stat(STAT_DEATH_ANIM as i32));
                    self.dying = Some(Dying::new(if anim > 0 { anim as u16 } else { DEFAULT_DEATH_ANIM }, true));
                }
                _ => {}
            }
            self.events.push(e);
        }
        self.pose_events.extend(self.actions.on_frame(f));
    }

    /// Per frame: ages the numbers, feeds the combat music (`FUN_10059736` for every character), runs the own death timer.
    pub fn update(&mut self, dt: f32, zone: &Zone, audio: Option<&Audio>) {
        self.announce(zone);
        for n in &mut self.numbers {
            n.age += dt;
        }
        self.numbers.retain(|n| n.age < n.spec.life);
        if let Some(d) = self.dying.as_mut() {
            if d.tick(dt) == Some(DieEvent::SendDeathDone) {
                let none = Identity::default();
                let a = simple(ACTION_DEATH_DONE, none, none);
                if std::env::var_os("AOMAC_COMBAT_LOG").is_some() {
                    eprintln!("combat: CharDie_t wait over, sending CharacterAction {ACTION_DEATH_DONE:#x}");
                }
                self.outbox.push(n3_frame(0, self.own as u32, action::character_action(self.own, &a)));
            }
        }
        if self.dying.is_some() && self.combat.char(self.own).is_some_and(|c| !c.dead && c.health() > 0) {
            self.dying = None; // resurrected (ResurrectIIR sets Health)
        }
        let Some(a) = audio else { return };
        for (&id, d) in &zone.dynels {
            a.set_combat_char(&self.music_char(id, d));
        }
    }

    /// `FUN_10059736` reads the same character stats as the hit handlers, not the zone's announcement snapshot.
    fn music_char(&self, id: i32, d: &DynelState) -> CharInfo {
        let c = self.combat.char(id);
        let fight = self.combat.fight(id);
        CharInfo {
            is_local: id == self.own,
            life: c.map_or(d.health, |c| c.health()),
            max_health: c.map_or(d.max_health, |c| c.max_health()),
            fighting: fight.is_some_and(|f| f.state != FIGHT_IDLE),
            target_is_local: fight.and_then(|f| f.target).is_some_and(|t| t.kind == DYNEL_CHAR && t.instance == self.own),
            side: i32::from(d.side),
            id,
            flag: false, // dynel+0x21c: writer unknown (docs/formats.md Combat music)
            level: d.level,
            metric: 0, // stat 421: stored, never read by a decision
        }
    }

    /// `FUN_1003fb35` -> `FUN_1003b73e` / `FUN_1003b0db`: every change of the selection is announced with `LookAtIIR_t`
    /// (before any attack that uses it).
    fn announce(&mut self, zone: &Zone) {
        let selected = zone.selected_target();
        if self.announced != selected && self.own_known(zone) {
            let t = selected.unwrap_or_default();
            self.send(net::look_at(self.own, t, i32::from(t.kind == DYNEL_CHAR)));
            self.announced = selected;
        }
    }

    fn own_known(&self, zone: &Zone) -> bool {
        zone.dynels.contains_key(&self.own)
    }

    fn attacker(&self, zone: &Zone, mode: u32) -> Attacker {
        let s = |id| zone.skill_value(id).unwrap_or(0);
        Attacker {
            id: self.own,
            is_client: true,
            flag_21c: false,
            mode,
            gm_level: s(0xd7),
            area_features: 0,
            side: s(0x21),
            status_blocked: false,
            fight_level: 2,
            in_team: false,
        }
    }

    fn target(&self, zone: &Zone, id: i32) -> Target {
        let Some(d) = zone.dynels.get(&id) else { return Target::Unattackable };
        let c = self.combat.char(id);
        Target::Char(CharTarget {
            id,
            in_tree: true,
            flag_21c: d.npc,
            dead_flag: c.is_some_and(|c| c.dead),
            health: c.map_or(d.health, |c| c.health()),
            fighting_attacker: self.combat.fight(id).and_then(|f| f.target).is_some_and(|t| t.instance == self.own),
            fight_level: 2,
            ..CharTarget::default()
        })
    }

    fn send(&mut self, payload: Vec<u8>) {
        self.outbox.push(n3_frame(0, self.own as u32, payload));
    }

    /// `N3Msg_StopAttack` [GC 0x10027f55].
    pub fn stop_attack(&mut self) {
        if self.attacking() {
            self.send(net::stop_fight(self.own));
        }
    }

    /// `N3Msg_DefaultAttack(target, switch)` [GC 0x10027fbc]. `mode` = movement FSM mode of the own character.
    fn default_attack(&mut self, zone: &Zone, mode: u32, target: Option<i32>, switch: bool) {
        let tid = target.map_or(Identity::default(), |t| Identity { kind: DYNEL_CHAR, instance: t });
        let f = self.combat.fight(self.own);
        let fight = Fight { fighting: self.attacking(), target: f.and_then(|f| f.target).unwrap_or_default() };
        let me = self.attacker(zone, mode);
        let tgt = target.map_or(Target::Unattackable, |t| self.target(zone, t));
        let r = net::default_attack(fight, tid, switch, mode, || net::can_attack(&me, &tgt, true));
        if r.stop_first {
            self.stop_attack();
        }
        match r.outcome {
            DefaultOutcome::Stopped => {}
            DefaultOutcome::Refused(fb) => self.feedback.extend(fb),
            DefaultOutcome::Send { flag } => match self.gate.begin(false) {
                Ok(()) => self.send(net::attack(self.own, tid, flag)),
                Err(fb) => self.feedback.push(fb),
            },
        }
    }

    /// `N3Msg_SecondarySpecialAttack(target, stat)` [GC 0x10028071]: the target is the selection (idle) or the fight target.
    /// [UNRESOLVED port] The weapon availability / recharge / range / line-of-sight checks (`FUN_10063be4`, `FUN_100686fb`,
    /// `FUN_100679c1`, `FUN_10058908`) are left to the server, which refuses the attack itself.
    fn special(&mut self, zone: &Zone, mode: u32, stat: i32) {
        let tid = match self.combat.fight(self.own).filter(|f| f.state != FIGHT_IDLE).and_then(|f| f.target) {
            Some(t) => t,
            None => zone.target.map_or(Identity::default(), |t| Identity { kind: DYNEL_CHAR, instance: t }),
        };
        let me = self.attacker(zone, mode);
        let tgt = if tid.kind == DYNEL_CHAR { self.target(zone, tid.instance) } else { Target::Unattackable };
        let v = net::can_attack(&me, &tgt, true);
        self.feedback.extend(v.feedback);
        if v.allowed {
            self.send(net::sec_spec_attack(self.own, tid, stat));
        }
    }

    /// Run a player command. `mode` = movement FSM mode of the own character (4 = swimming).
    pub fn command(&mut self, cmd: Command, zone: &Zone, mode: u32) {
        self.announce(zone);
        match cmd {
            Command::Attack => self.default_attack(zone, mode, zone.target, false),
            Command::SwitchTarget(id) => self.default_attack(zone, mode, Some(id), true),
            Command::Special(stat) => self.special(zone, mode, stat),
        }
    }

    /// `N3Msg_PerformSpecialAction(id)` / hotbar slot type 6 (`FUN_1004256c` [GC 0x1004256c]) for the actions of the combat
    /// layer. Returns false for ids that belong to other modules (sneak, crawl, camping, ...).
    pub fn special_action(&mut self, id: i32, zone: &Zone, mode: u32) -> bool {
        match id {
            act::ATTACK | 0x4e => self.command(Command::Attack, zone, mode),
            act::BRAWL | act::DIMACH | 0x79 | 0x92..=0x94 | 0x96 | 0x97 | act::FULL_AUTO | 0x1e9 => self.command(Command::Special(id), zone, mode),
            _ => return false,
        }
        true
    }

    /// `N3Msg_DoSocialAction(anim)` [GC 0x100269d3] (`/wave`, `/emote bow`, ...): `mode` = movement FSM mode.
    pub fn social(&mut self, anim: i32, mode: u32) {
        match action::social_allowed(anim, mode as i32, false) {
            Ok(()) => {
                self.counter += 1;
                self.send(action::social_action(self.own, self.counter, anim));
            }
            Err(key) => self.feedback.push(key),
        }
    }

    /// `N3Msg_AssistFight` [GC 0x10027374] (`/assist`): selects the fight target of the selected character.
    /// Returns the new selection or the refusal text. [UNRESOLVED] the PvP/side checks of the original (stat 0xc4 / 0x184 tests).
    pub fn assist(&self, zone: &Zone) -> Result<i32, &'static str> {
        let Some(t) = zone.target else { return Err("Feedback_NoTargetToAssist") };
        if t == self.own {
            return Err("Feedback_CantAssistYourself");
        }
        match self.combat.fight(t) {
            Some(f) if f.state != FIGHT_IDLE => f.target.filter(|x| x.kind == DYNEL_CHAR && zone.dynels.contains_key(&x.instance)).map(|x| x.instance).ok_or(""),
            _ => Err("Feedback_TargetIsNotInFight"),
        }
    }

    /// `N3Msg_SitToggle` pre-step: sitting stops the attack first (the rest is the movement layer's `sit_toggle`).
    pub fn before_sit(&mut self) {
        self.stop_attack();
    }

    /// Queue a raw frame built by another part of the layer (`CharacterActionIIR_t` for stand-up, emotes ...).
    pub fn push(&mut self, payload: Vec<u8>) {
        self.send(payload);
    }

    /// Duel / pet-duel reactions of the received frames (lines, dialogs; docs/zone/combat-duel.md).
    pub fn take_duel(&mut self) -> Vec<super::duel::Event> {
        std::mem::take(&mut self.duel)
    }

    /// `/duel [accept|reject|stop|draw]` (GUI 0x100b8a10) and `/petduel [accept|reject|stop]` (0x100b8789) after the chat parser: `Err(text)` is the red
    /// chat line of the original. The challenge needs a target: `/duel` a character that is neither the own one nor an NPC (`N3Msg_IsNpc`), `/petduel`
    /// any character; accepting / refusing a duel also emits the signals that close the challenge dialog (`Event::Close`).
    pub fn duel_command(&mut self, zone: &Zone, pet: bool, op: super::duel::Op) -> Result<(), &'static str> {
        use super::duel::Op;
        use ao_net::n3::action::duel as d;
        let own = self.own;
        let payload = match (pet, op) {
            (false, Op::Challenge) => match zone.target.filter(|&t| t != own && !zone.dynels.get(&t).is_some_and(|d| d.npc)) {
                Some(t) => d::challenge(own, t),
                None => return Err(super::duel::NEED_TARGET),
            },
            (true, Op::Challenge) => match zone.target {
                Some(t) => d::pet_challenge(own, t),
                None => return Err(super::duel::NEED_TARGET),
            },
            (false, op) => {
                if matches!(op, Op::Accept | Op::Refuse) {
                    self.duel.push(super::duel::Event::Close);
                }
                d::op(own, op)
            }
            (true, Op::Accept) => d::pet_answer(own, true),
            (true, Op::Refuse) => d::pet_answer(own, false),
            // `/petduel draw` is a usage error in the parser; `stop` is `N3Msg_PetDuel_Stop`
            (true, _) => d::pet_stop(own),
        };
        self.send(payload);
        Ok(())
    }

    /// `AutoRejectDuel` DValue set: `FUN_1005b821` answers a challenge with a refusal ({2, 1}) instead of asking the player.
    pub fn duel_auto_refuse(&mut self) {
        self.send(ao_net::n3::action::duel::auto_refuse(self.own));
    }

    /// `N3Msg_StartPvP(target)` [GC 0x10018276] (Yes in the PvP confirmation): `FUN_10067c34(target, 0)`, i.e. an `AttackIIR_t` with flag 0 behind the
    /// `+0x79` guard, no `CanAttack` test.
    pub fn start_pvp(&mut self, target: Identity) {
        match self.gate.begin(false) {
            Ok(()) => self.send(net::attack(self.own, target, 0)),
            Err(fb) => self.feedback.push(fb),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::combat::log::fake::Fixed;
    use crate::play::zone::DynelState;
    use ao_net::n3::misc::Misc;
    use ao_net::n3::{self, N3};

    const OWN: u32 = 25988;

    fn frames() -> Vec<Frame> {
        include_str!("../../../../../docs/captures/zone_ithaca.rec")
            .lines()
            .filter_map(|l| {
                let mut p = l.split(' ');
                let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                (dir == "<").then(|| Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
            })
            .filter(|f| f.ptype == ao_net::frame::PT_N3)
            .collect()
    }

    fn misc(f: &Frame) -> Option<Misc> {
        match n3::decode(f).ok()?.body {
            N3::Misc(m) => Some(m),
            _ => None,
        }
    }

    /// Module that has seen the capture up to (excluding) the first AttackIIR on an announced, living target, and that target.
    fn primed() -> (Module, Zone, i32) {
        let mut m = Module::with_texts(Box::new(Fixed::new()), OWN);
        let mut target = None;
        for f in frames() {
            if let Some(Misc::Attack(a)) = misc(&f) {
                if m.combat().char(a.target.instance).is_some_and(|c| c.health() > 0) && m.combat().char(OWN as i32).is_some() {
                    target = Some(a.target.instance);
                    break;
                }
            }
            m.on_frame(&f);
        }
        let t = target.expect("capture has an attack");
        let mut z = Zone::new(OWN);
        for id in [OWN as i32, t] {
            z.dynels.insert(id, DynelState { name: format!("c{id}"), pos: [0.0; 3], yaw: None, npc: id != OWN as i32, side: 0, level: 1, health: 50, max_health: 50 });
        }
        z.target = Some(t);
        (m, z, t)
    }

    #[test]
    fn kill_and_stop_fight_restore_district_music_from_live_stats() {
        use ao_audio::combat::{char_sample, CombatMusic};
        use ao_net::n3::misc::AttackInfo;
        let (mut m, z, target) = primed();
        let own = OWN as i32;
        m.combat.stats(own, &[(27, 50), (1, 50)], &mut Vec::new());
        m.combat.stats(target, &[(27, 50), (1, 50)], &mut Vec::new());
        m.on_frame(&n3_frame(0, OWN, net::attack(own, Identity { kind: DYNEL_CHAR, instance: target }, 0)));
        m.on_frame(&n3_frame(0, target as u32, net::attack(target, Identity { kind: DYNEL_CHAR, instance: own }, 0)));
        let mut music = CombatMusic::new(3);
        let feed = |m: &Module, music: &mut CombatMusic, dt| {
            for (&id, d) in &z.dynels {
                if let Some(sample) = char_sample(&m.music_char(id, d)) {
                    music.combat_update(&sample);
                }
            }
            music.update(dt);
        };
        feed(&m, &mut music, 0.016);
        assert_ne!(music.state(), 0, "the opponent starts battle music");
        m.combat.hit(own, &AttackInfo { slot: 0, damage: 50, value_20: -1, other: Identity { kind: DYNEL_CHAR, instance: target }, unk_2c: 0, unk_30: 3, unk_34: 0 }, &mut Vec::new());
        assert_eq!(z.dynels[&target].health, 50, "the zone announcement remains stale");
        assert_eq!(m.music_char(target, &z.dynels[&target]).life, 0, "audio reads the hit-updated stat");
        feed(&m, &mut music, 0.016);
        m.on_frame(&n3_frame(0, OWN, net::stop_fight(own)));
        m.on_frame(&n3_frame(0, target as u32, net::stop_fight(target)));
        assert!(!m.music_char(target, &z.dynels[&target]).fighting);
        assert!(char_sample(&m.music_char(target, &z.dynels[&target])).is_none());
        for _ in 0..20 {
            feed(&m, &mut music, 1.0);
        }
        assert_eq!(music.state(), 0, "district music resumes after the retail victory lock");
        assert_eq!(music.layer_name(), None);
    }

    fn sent(m: &mut Module) -> Vec<Misc> {
        m.take_outbox().iter().filter_map(misc).collect()
    }


    #[test]
    fn object_selection_announces_complete_identity_with_mode_zero() {
        let (mut m, mut z, _) = primed();
        m.take_outbox();
        let object = Identity { kind: 0xc76a, instance: 2 };
        z.set_target(Some(object));
        m.announce(&z);
        let out = m.take_outbox();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, net::look_at(OWN as i32, object, 0));
        m.announce(&z);
        assert!(m.take_outbox().is_empty(), "unchanged object selection is not retransmitted");
        z.set_target(None);
        m.announce(&z);
        assert_eq!(m.take_outbox()[0].payload, net::look_at(OWN as i32, Identity::default(), 0));
    }
    /// Attack key: AttackIIR out, second press while the server has not answered is refused (`+0x79`), the server's echo starts the
    /// fight, the next press sends StopFight, its echo ends the fight.
    #[test]
    fn attack_key_round_trip() {
        let (mut m, z, t) = primed();
        m.command(Command::Attack, &z, 2);
        let out = sent(&mut m);
        assert!(matches!(&out[..], [Misc::Attack(a)] if a.target.instance == t && a.flag == 0), "{out:?}");
        m.command(Command::Attack, &z, 2);
        assert!(sent(&mut m).is_empty());
        assert_eq!(m.take_feedback(), vec![net::FB_PLEASE_WAIT]);
        let echo = n3_frame(0, OWN, net::attack(OWN as i32, Identity { kind: DYNEL_CHAR, instance: t }, 0));
        m.on_frame(&echo);
        assert!(m.attacking());
        m.command(Command::Attack, &z, 2);
        let out = sent(&mut m);
        assert!(matches!(&out[..], [Misc::StopFight(s)] if s.flag), "{out:?}");
        assert!(m.attacking(), "the fight state changes only when the server echoes");
        m.on_frame(&n3_frame(0, OWN, net::stop_fight(OWN as i32)));
        assert!(!m.attacking());
    }

    /// The unwield action (0x61, `FUN_1006a857`) of an occupied hand slot plays the wield gesture 0x6d (`FUN_10081e74(char, 3)`) unless the holder fights
    /// (the idle update of `FUN_1006a772` then starts the idle clip over it); an empty slot does nothing.
    #[test]
    fn unwield_queues_the_wield_gesture_out_of_a_fight() {
        let (mut m, _z, t) = primed();
        let own = OWN as i32;
        let unwield = |m: &mut Module| m.on_frame(&n3_frame(0, OWN, action::character_action(own, &simple(ACTION_UNWIELD, Identity::default(), Identity { kind: 0, instance: 6 }))));
        assert!(!m.attacking());
        unwield(&mut m);
        assert!(m.take_anims().is_empty(), "nothing in the hand");
        m.combat.arms.wield(own, 901, 6, None, &[(0x1b4, 0x5a)]);
        unwield(&mut m);
        assert_eq!(m.take_anims(), vec![(own, WIELD_GESTURE)]);
        m.combat.arms.wield(own, 901, 6, None, &[(0x1b4, 0x5a)]);
        m.on_frame(&n3_frame(0, OWN, net::attack(own, Identity { kind: DYNEL_CHAR, instance: t }, 0)));
        assert!(m.is_fighting(own));
        unwield(&mut m);
        assert!(m.take_anims().is_empty(), "in a fight the idle update replaces the gesture");
    }

    #[test]
    fn social_action_and_assist() {
        let (mut m, mut z, t) = primed();
        m.social(62, 2); // /wave
        let f = m.take_outbox();
        let s = action::parse_social_action(&f[0].payload).unwrap();
        assert_eq!((s.anim, s.counter, s.state), (62, 1, 0));
        m.social(62, 4); // swimming
        assert_eq!(m.take_feedback(), vec!["Feedback_CantDoSocialActionsWhileSwimming"]);
        // /assist: the selected NPC is not fighting -> refused; once it fights `other`, `other` becomes the selection
        assert_eq!(m.assist(&z), Err("Feedback_TargetIsNotInFight"));
        z.dynels.insert(t + 1, z.dynels[&t].clone());
        m.on_frame(&n3_frame(0, 0, net::attack(t, Identity { kind: DYNEL_CHAR, instance: t + 1 }, 0)));
        let _ = m.assist(&z);
        z.target = None;
        assert_eq!(m.assist(&z), Err("Feedback_NoTargetToAssist"));
        z.target = Some(OWN as i32);
        assert_eq!(m.assist(&z), Err("Feedback_CantAssistYourself"));
    }

    /// The live refusal of an attack on a Surf Lizard (`CharacterAction` 0x93 then 0x76 with instance 6): texts, gate cleared.
    #[test]
    fn server_refusal_prints_text_and_clears_the_gate() {
        let (mut m, z, _) = primed();
        m.command(Command::Attack, &z, 2);
        m.take_outbox();
        let own = Identity { kind: DYNEL_CHAR, instance: OWN as i32 };
        let mk = |act: i32, a: Identity| n3_frame(0, OWN, action::character_action_for(own, &ao_net::n3::world::CharacterAction { action: act, param: 0, identity_a: a, identity_b: Identity::default(), text: String::new() }));
        m.on_frame(&mk(0x93, Identity::default()));
        m.on_frame(&mk(0x76, Identity { kind: 0, instance: 6 }));
        assert_eq!(m.take_feedback(), vec!["Feedback_StartingAttackFailed", "Feedback_PvpNotAllowedInThisDistrict"]);
        m.command(Command::Attack, &z, 2);
        assert_eq!(sent(&mut m).len(), 1, "the guard is clear again");
    }

    /// The first live attack (docs/captures/zone_attack_refused_ithaca.rec, Testy on a Surf Lizard): our encoder reproduces the
    /// client frame byte for byte and the server's 0x93 / 0x76 answer yields the original's two texts.
    #[test]
    fn live_refusal_capture() {
        let rec = include_str!("../../../../../docs/captures/zone_attack_refused_ithaca.rec");
        let (mut sent_n, mut m) = (0, Module::with_texts(Box::new(Fixed::new()), 25988));
        for l in rec.lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            let f = Frame::decode_with(&b, false).unwrap().unwrap().0;
            if dir == ">" {
                sent_n += 1;
                assert_eq!(f.payload, net::attack(25988, Identity { kind: DYNEL_CHAR, instance: 0xfaac5 }, 0));
            } else {
                m.on_frame(&f);
            }
        }
        assert_eq!(sent_n, 1);
        assert_eq!(m.take_feedback(), vec!["Feedback_StartingAttackFailed", "Feedback_PvpNotAllowedInThisDistrict"]);
    }

    /// The first fight that worked live (docs/captures/zone_fight_ithaca.rec, Aomacvolk on a Beach Leet, after the `LookAtIIR_t`
    /// selection announce was added): the frames we sent are reproduced, the replies drive the whole fight.
    #[test]
    fn live_fight_capture() {
        let own = 0x82e8;
        let mut m = Module::with_texts(Box::new(Fixed::new()), own);
        // the capture's own `FullCharacter` registers the player; the leet's `SimpleCharFullUpdate` is not in the excerpt
        m.combat.add_test_char(0xfd6a9, "Beach Leet", true, 12);
        let (mut sent, mut hits, mut stopped, mut started) = (vec![], 0, false, false);
        for l in include_str!("../../../../../docs/captures/zone_fight_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            let f = Frame::decode_with(&b, false).unwrap().unwrap().0;
            if dir == ">" {
                sent.push(f.payload);
                continue;
            }
            m.on_frame(&f);
            for e in m.take_events() {
                match e {
                    CombatEvent::Hit { attacker, .. } if attacker == own as i32 => hits += 1,
                    CombatEvent::FightStarted { who, .. } if who == own as i32 => started = true,
                    CombatEvent::FightStopped { who } if who == own as i32 => stopped = true,
                    _ => {}
                }
            }
        }
        let t = Identity { kind: DYNEL_CHAR, instance: 0xfd6a9 };
        assert_eq!(sent[0], net::look_at(own as i32, t, 1));
        assert_eq!(sent[1], net::attack(own as i32, t, 0));
        assert!(started && stopped && hits >= 3, "{started} {stopped} {hits}");
        assert!(m.numbers().iter().any(|n| n.text == "145"), "the XP number");
    }

    #[test]
    fn attack_without_target_is_refused() {
        let (mut m, mut z, _) = primed();
        z.target = None;
        m.command(Command::Attack, &z, 2);
        assert!(sent(&mut m).is_empty());
        assert_eq!(m.take_feedback(), vec!["Feedback_UnableToAttackTarget"]);
        m.command(Command::Attack, &z, 4);
        assert_eq!(m.take_feedback(), vec!["Feedback_CantAttackInThisState"], "swimming");
    }

    #[test]
    fn switching_target_while_fighting_sets_flag_2() {
        let (mut m, mut z, t) = primed();
        let other = t + 1000;
        z.dynels.insert(other, DynelState { name: "b".into(), pos: [0.0; 3], yaw: None, npc: true, side: 0, level: 1, health: 5, max_health: 5 });
        m.on_frame(&n3_frame(0, OWN, net::attack(OWN as i32, Identity { kind: DYNEL_CHAR, instance: t }, 0)));
        m.command(Command::SwitchTarget(other), &z, 2);
        let out = sent(&mut m);
        assert!(matches!(&out[..], [Misc::StopFight(_), Misc::Attack(a)] if a.target.instance == other && a.flag == 2), "{out:?}");
    }

    #[test]
    fn special_attack_and_hotbar_ids() {
        let (mut m, z, t) = primed();
        assert!(m.special_action(act::BRAWL, &z, 2));
        let out = sent(&mut m);
        assert!(matches!(&out[..], [Misc::CharSecSpecAttack(s)] if s.target.instance == t && s.special == 0x8e), "{out:?}");
        assert!(!m.special_action(0x13, &z, 2), "sneak is not the combat layer's");
    }

    /// Replaying the capture as the fighting player 33402 yields floating numbers that age out.
    #[test]
    fn numbers_age_out() {
        let mut m = Module::with_texts(Box::new(Fixed::new()), 33402);
        for f in frames() {
            m.on_frame(&f);
        }
        assert!(!m.numbers().is_empty());
        assert!(m.numbers().iter().any(|n| n.spec.space == Space::Hud));
        let z = Zone::new(33402);
        for _ in 0..30 {
            m.update(0.1, &z, None);
        }
        assert!(m.numbers().is_empty());
    }
}

#[cfg(test)]
mod death_tests {
    use super::*;
    use crate::play::combat::log::fake::Fixed;
    use ao_net::n3::{self, world::CharacterAction, N3};

    fn action(who: i32, a: i32, b: Identity, act: i32) -> Frame {
        let id = Identity { kind: DYNEL_CHAR, instance: who };
        n3_frame(0, who as u32, action::character_action_for(id, &CharacterAction { action: act, param: 0, identity_a: a_id(a), identity_b: b, text: String::new() }))
    }

    fn a_id(instance: i32) -> Identity {
        Identity { kind: 0, instance }
    }

    /// The own death (`CharacterAction` 99, `identity_b` = animation): stat 0x183 holds the animation (`CharDie_t` ctor), the death timer runs
    /// 3 s (`FUN_1007b58c`) and then sends action 0x98 once.
    #[test]
    fn own_death_holds_the_animation_and_reports_after_three_seconds() {
        let own = 0x82e8;
        let mut m = Module::with_texts(Box::new(Fixed::new()), own);
        m.combat.add_test_char(own as i32, "Aomacvolk", false, 100);
        assert_eq!(m.death_anim(), DEFAULT_DEATH_ANIM);
        m.on_frame(&action(own as i32, 0, a_id(500), 99));
        assert_eq!(m.death_anim(), 500);
        assert!(m.take_events().iter().any(|e| matches!(e, CombatEvent::Died { dynel, cause: 0 } if *dynel == own as i32)));
        let z = Zone::new(own);
        m.update(2.9, &z, None);
        assert!(m.take_outbox().is_empty(), "the wait is 3 s");
        m.update(0.2, &z, None);
        m.update(1.0, &z, None);
        let out = m.take_outbox();
        let done: Vec<i32> = out.iter().filter_map(|f| match n3::decode(f).ok()?.body {
            N3::World(ao_net::n3::world::World::CharacterAction(a)) => Some(a.action),
            _ => None,
        }).collect();
        assert_eq!(done, [ACTION_DEATH_DONE], "sent exactly once");
    }

    /// Live kill of Aomacrceg (lvl 1 Solitus Soldier, bare hands) on a Beach Leet (12 HP) on the ICC beach (`docs/captures/zone_kill_ithaca.rec`, redacted
    /// excerpt: only frames addressed to the two characters and the corpse): own hits of 5 / 6 / 5 (bare hands = slot 0), the leet's hits, the
    /// kill (`CharacterAction` 99 -> `Died`), the XP number 145 and the corpse update.
    #[test]
    fn live_kill_capture() {
        let own = 0x830e;
        let leet = 1015682;
        let mut m = Module::with_texts(Box::new(Fixed::new()), own);
        let (mut dealt, mut died, mut stopped, mut started) = (0, false, false, false);
        for l in include_str!("../../../../../docs/captures/zone_kill_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir == ">" {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            m.on_frame(&Frame::decode_with(&b, false).unwrap().unwrap().0);
            for e in m.take_events() {
                match e {
                    CombatEvent::Hit { attacker, victim, damage, slot, .. } if attacker == own as i32 && victim == leet => {
                        assert_eq!(slot, 0, "bare hands");
                        dealt += damage;
                    }
                    CombatEvent::FightStarted { who, .. } if who == own as i32 => started = true,
                    CombatEvent::FightStopped { who } if who == own as i32 => stopped = true,
                    CombatEvent::Died { dynel, cause: 0 } if dynel == leet => died = true,
                    _ => {}
                }
            }
        }
        assert!(started && stopped && died, "{started} {stopped} {died}");
        assert!(dealt >= 12, "the leet has 12 HP: {dealt}");
        assert!(m.numbers().iter().any(|n| n.text == "145"), "the XP number");
    }

    /// Live death of Aomacvolk (lvl 2, 40 HP) against a Fresh Engineer in Borealis (`docs/captures/zone_death_borealis.rec`, redacted excerpt of the
    /// frames addressed to the two characters): the exchange of blows, the health dropping below 0, the music switch and the server's `CharacterAction` 99.
    #[test]
    fn live_death_capture() {
        let own = 0x82e8;
        let mut m = Module::with_texts(Box::new(Fixed::new()), own);
        let (mut hits_on_own, mut min_health, mut music_off, mut death_music, mut died, mut started) = (0, i32::MAX, false, false, false, false);
        for l in include_str!("../../../../../docs/captures/zone_death_borealis.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir == ">" {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            m.on_frame(&Frame::decode_with(&b, false).unwrap().unwrap().0);
            for e in m.take_events() {
                match e {
                    CombatEvent::Hit { victim, .. } if victim == own as i32 => hits_on_own += 1,
                    CombatEvent::Health { dynel, health, .. } if dynel == own as i32 => min_health = min_health.min(health),
                    CombatEvent::FightStarted { who, .. } if who == own as i32 => started = true,
                    CombatEvent::CombatMusic(false) => music_off = true,
                    CombatEvent::DeathMusic(true) => death_music = true,
                    CombatEvent::Died { dynel, cause: 0 } if dynel == own as i32 => died = true,
                    _ => {}
                }
            }
        }
        assert!(started && hits_on_own >= 2 && min_health < 0 && music_off && death_music && died, "{started} {hits_on_own} {min_health} {music_off} {death_music} {died}");
    }

    /// Another character's action 0xd1 is a hit-sound cue when both identity instances are positive, nothing otherwise.
    #[test]
    fn action_d1_cues_the_hit_sound() {
        let mut m = Module::with_texts(Box::new(Fixed::new()), 1);
        m.on_frame(&action(77, 5, a_id(9), ACTION_HIT));
        m.on_frame(&action(78, 0, a_id(9), ACTION_HIT));
        m.on_frame(&action(79, 5, a_id(0), ACTION_HIT));
        assert_eq!(m.take_struck(), [77]);
    }

    /// The Beach Leet kill of `zone_fight_ithaca.rec`: the fight controller marks it dead and stores the animation 503 (stat 0x183).
    #[test]
    fn captured_kill_stores_the_death_animation() {
        let mut m = Module::with_texts(Box::new(Fixed::new()), 0x82e8);
        m.combat.add_test_char(0xfd6a9, "Beach Leet", true, 12);
        let mut died = false;
        for l in include_str!("../../../../../docs/captures/zone_fight_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir == ">" {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            m.on_frame(&Frame::decode_with(&b, false).unwrap().unwrap().0);
            died |= m.take_events().iter().any(|e| matches!(e, CombatEvent::Died { dynel: 0xfd6a9, cause: 0 }));
        }
        let c = m.combat().char(0xfd6a9).expect("the capture holds no quit of the leet");
        assert!(died && c.dead);
        assert_eq!(c.stat(STAT_DEATH_ANIM as i32), 503);
    }
}
