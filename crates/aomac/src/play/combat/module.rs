//! The combat / action layer of the own client: feeds the received N3 frames to the fight controller
//! ([`super::state::Combat`]) and the pose tracker ([`super::actions::Actions`]), turns the player's commands (attack
//! key, hotbar special action, sit) into the original's outgoing messages (`ao_net::n3::combat` / `action`), keeps the
//! floating damage numbers and feeds the combat-music state machine. Message flow and addresses: docs/zone/combat-net.md,
//! combat-log.md, combat-anim.md, actions.md.

use super::actions::{Actions, Event as ActionEvent};
use super::anim::{DieEvent, Dying, ACTION_DEATH_DONE};
use super::log::{FloatingNumber, Space, HUD_X, HUD_X_JITTER};
use super::state::{Combat, CombatEvent, FIGHT_IDLE};
use crate::play::zone::Zone;
use ao_audio::combat::CharInfo;
use ao_audio::Audio;
use ao_formats::screens::TextDb;
use ao_net::frame::Frame;
use ao_net::n3::action::{self, simple};
use ao_net::n3::combat::{self as net, action as act, Attacker, AttackGate, CharTarget, DefaultOutcome, Fight, Target};
use ao_net::n3::outgoing::{n3_frame, DYNEL_CHAR};
use ao_net::msg::Identity;
use std::path::Path;

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
    /// `Dynels` hooks for the next `drain`: (dynel that swung, dynel that died).
    swings: Vec<i32>,
    rng: u32,
    /// Events of the last received frames for the HUD / sounds (taken by [`Module::take_events`]).
    events: Vec<CombatEvent>,
    pose_events: Vec<ActionEvent>,
}

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
        Self::with_texts(texts, own)
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
            swings: Vec::new(),
            rng: 0x2545_F491 ^ own,
            events: Vec::new(),
            pose_events: Vec::new(),
        }
    }

    pub fn combat(&self) -> &Combat {
        &self.combat
    }

    pub fn actions(&self) -> &Actions {
        &self.actions
    }

    /// The own character is in a fight (`N3Msg_IsAttacking`).
    pub fn attacking(&self) -> bool {
        self.combat.is_fighting(self.own)
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

    /// Dynels whose attack clip should play (a hit of theirs landed or a special attack started).
    pub fn take_swings(&mut self) -> Vec<i32> {
        std::mem::take(&mut self.swings)
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
        let ev = self.combat.on_frame(f, self.own as u32);
        for e in ev {
            match &e {
                CombatEvent::Hit { attacker, .. } => self.swings.push(*attacker),
                CombatEvent::SpecialAttack { who, .. } => self.swings.push(*who),
                CombatEvent::Floating { dynel, amount, number, .. } => {
                    let jitter = if number.space == Space::Hud { HUD_X as f32 + (self.rand01() * 2.0 - 1.0) * HUD_X_JITTER as f32 } else { 0.0 };
                    self.numbers.push(Number { dynel: *dynel, text: amount.to_string(), spec: *number, age: 0.0, jitter });
                }
                CombatEvent::FightStarted { who, .. } if *who == self.own => {
                    // FUN_10069c68 cleared the controller's +0x79 guard after CanAttack(report = 0) passed
                    self.gate.on_attack_applied(true);
                }
                CombatEvent::Died { dynel, .. } if *dynel == self.own => {
                    if self.dying.is_none() {
                        self.dying = Some(Dying::new(0, true));
                    }
                }
                _ => {}
            }
            self.events.push(e);
        }
        self.pose_events.extend(self.actions.on_frame(f));
    }

    /// Per frame: ages the numbers, feeds the combat music (`FUN_10059736` for every character), runs the own death timer.
    pub fn update(&mut self, dt: f32, zone: &Zone, audio: Option<&Audio>) {
        for n in &mut self.numbers {
            n.age += dt;
        }
        self.numbers.retain(|n| n.age < n.spec.life);
        if let Some(d) = self.dying.as_mut() {
            if d.tick(dt) == Some(DieEvent::SendDeathDone) {
                let none = Identity::default();
                let a = simple(ACTION_DEATH_DONE, none, none);
                self.outbox.push(n3_frame(0, self.own as u32, action::character_action(self.own, &a)));
            }
        }
        if self.dying.is_some() && self.combat.char(self.own).is_some_and(|c| !c.dead && c.health() > 0) {
            self.dying = None; // resurrected (ResurrectIIR sets Health)
        }
        let Some(a) = audio else { return };
        for (&id, d) in &zone.dynels {
            let fight = self.combat.fight(id);
            let info = CharInfo {
                is_local: id == self.own,
                life: d.health,
                max_health: d.max_health,
                fighting: fight.is_some_and(|f| f.state != FIGHT_IDLE),
                target_is_local: fight.and_then(|f| f.target).is_some_and(|t| t.kind == DYNEL_CHAR && t.instance == self.own),
                side: i32::from(d.side),
                id,
                flag: false, // dynel+0x21c: writer unknown (docs/formats.md Combat music)
                level: d.level,
                metric: 0, // stat 421: stored, never read by a decision
            };
            a.set_combat_char(&info);
        }
    }

    fn attacker(&self, zone: &Zone, mode: u32) -> Attacker {
        let s = |id| zone.stat(id).unwrap_or(0);
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

    /// `N3Msg_SitToggle` pre-step: sitting stops the attack first (the rest is the movement layer's `sit_toggle`).
    pub fn before_sit(&mut self) {
        self.stop_attack();
    }

    /// Queue a raw frame built by another part of the layer (`CharacterActionIIR_t` for stand-up, emotes ...).
    pub fn push(&mut self, payload: Vec<u8>) {
        self.send(payload);
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

    fn sent(m: &mut Module) -> Vec<Misc> {
        m.take_outbox().iter().filter_map(misc).collect()
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
