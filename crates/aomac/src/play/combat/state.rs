//! The fight controller of every character, health bookkeeping, floating numbers and combat-log lines, driven by the zone
//! server's N3 messages exactly as the original client applies them (docs/zone/combat-log.md).
//!
//! `SimpleChar_t+0x1d4` is the fight controller ([`Fight`]): `+0x44` state (1 idle, 2 fighting), `+0x4c/+0x50` target,
//! `+0x70/+0x74` last target, `+0x7c` pending death cause. The apply routines ported here:
//! * `AttackIIR_t` [GC 0x1007c545] -> `FUN_10069c68` [GC 0x10069c68] start fight ([`Combat::start_fight`]),
//! * `StopFightIIR_t` [GC 0x10079d75] / `CharDie_t` -> `FUN_10068b7f` [GC 0x10068b7f] ([`Combat::stop_fight`]),
//! * `AttackInfoIIR_t` [GC 0x1009ed0d] -> `FUN_1006a8f3` [GC 0x1006a8f3] -> `FUN_1009b170` [GC 0x1009b170] hit ([`Combat::hit`]),
//! * `SpecialAttackInfoIIR_t` [GC 0x100a19b9] -> `FUN_1006a9c5` [GC 0x1006a9c5] ([`Combat::special_hit`]),
//! * `MissedAttackInfoIIR_t` [GC 0x100a0b20] -> `FUN_1006ae50` [GC 0x1006ae50] ([`Combat::miss`]),
//! * `CharSecSpecAttackIIR_t` [GC 0x100727c8] -> `FUN_10068790` [GC 0x10068790],
//! * `StatIIR_t` [GC 0x100a1aaf] ([`Combat::stats`]), `FUN_1005ae91` [GC 0x1005ae91] death ([`Combat::die`]).

use super::anim::{death_anim_from_action, STAT_DEATH_ANIM};
use super::log::{self, render, Feedback, FloatingNumber, Space, Texts, Who};
use super::arms::{valid_slot, Armory, DEFAULT_DAMAGE_TYPE};
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::{self, dynel::Dynel, misc::{AttackInfo, Misc}, world::World, N3};
use std::collections::{BTreeMap, HashMap};

/// Identity kind of character / NPC dynels (`SimpleChar_t`).
pub const CHAR_KIND: i32 = 0xC350;
/// `CharacterActionIIR_t` action 0xd0: a drain set a stat (Health / Nano) of the receiving character: `identity_b = {stat, new value}`, `identity_a.kind` = the amount.
pub const ACTION_DRAIN: i32 = 0xd0;
/// `CharacterActionIIR_t` action 0x64: the server sets the animation id of a character's animation holder (`identity_b.instance`).
pub const ACTION_PLAY_ANIM: i32 = 0x64;
/// `CharacterActionIIR_t` action 0x99: the server announces the death cause (`identity_b.instance`, 1..=7) -> [`Combat::die`].
pub const ACTION_DEATH_CAUSE: i32 = 0x99;
/// `Stat_e`s used here (`fStatToString` table, [`super::stat_names`]).
pub const STAT_MAX_HEALTH: i32 = 1;
pub const STAT_HEALTH: i32 = 27;
pub const STAT_LEVEL: i32 = 0x36;
pub const STAT_XP: i32 = 0x34;
pub const STAT_ALIEN_XP: i32 = 0x28;
pub const STAT_ALIEN_LEVEL: i32 = 0xa9;
pub const STAT_SHADOW_KNOWLEDGE: i32 = 0x23d;
/// `GetStat(0xb2)` added to the alien-xp delta when the same message raises `AlienLevel` (`FUN_100a1aaf` @ 0x100a1d0b).
const STAT_ALIEN_XP_BASE: i32 = 0xb2;
/// Fight states of `controller+0x44`.
pub const FIGHT_IDLE: u8 = 1;
pub const FIGHT_FIGHTING: u8 = 2;

/// Fight controller (`SimpleChar_t+0x1d4`).
#[derive(Clone, Debug, PartialEq)]
pub struct Fight {
    /// `+0x44`: 1 idle, 2 fighting.
    pub state: u8,
    /// `+0x4c/+0x50`: the current target.
    pub target: Option<Identity>,
    /// `+0x70/+0x74`: target of the last start-fight call (never cleared).
    pub last_target: Option<Identity>,
    /// `+0x7c`: death cause handed to `FUN_1005ae91` after the *next* hit on this char (see [`Combat::hit`]).
    pub death_cause: i32,
    /// Special attacks queued by `CharSecSpecAttack` (`FUN_10068790`).
    pub pending_specials: Vec<i32>,
}

impl Default for Fight {
    fn default() -> Self {
        Self { state: FIGHT_IDLE, target: None, last_target: None, death_cause: 0, pending_specials: Vec::new() }
    }
}

/// What the combat layer tracks per character dynel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Char {
    pub name: String,
    /// `SimpleChar_t+0x21c`.
    pub npc: bool,
    pub stats: HashMap<i32, i32>,
    pub fight: Fight,
    /// dynel stat flag 0x10 (`+0x138` bit 4), set by `CharacterAction` 99.
    pub dead: bool,
}

impl Char {
    pub fn stat(&self, id: i32) -> i32 {
        self.stats.get(&id).copied().unwrap_or(0)
    }
    pub fn health(&self) -> i32 {
        self.stat(STAT_HEALTH)
    }
    pub fn max_health(&self) -> i32 {
        self.stat(STAT_MAX_HEALTH)
    }
}

/// A line for the chat window's combat category: `FUN_10012b05(style, text, category)` [GC 0x10012b05] (signal
/// `GlobalSignals+0x17c`).
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Chat category = `ColorCode_e` ([`log::gui_color`]).
    pub category: u32,
    /// First signal argument (`0x42000001..` for the feedback formatter, 0 for the plain texts).
    pub style: u32,
    pub text: String,
}

/// What the renderer / HUD / audio react to.
#[derive(Clone, Debug, PartialEq)]
pub enum CombatEvent {
    /// `FUN_10069c68` finished: `who` is fighting `target` (`switched` = it was already fighting).
    FightStarted { who: i32, target: Identity, switched: bool },
    /// `FUN_10068b7f` reset the controller of `who` to idle.
    FightStopped { who: i32 },
    /// `SandyInterfaceModule_t::SetCombatMusicMode` (client char only) [GC 0x100111e1].
    CombatMusic(bool),
    /// `SandyInterfaceModule_t::SetDeathMusicMode` [GC 0x100111f2]: the client char stopped fighting at 0 health.
    DeathMusic(bool),
    /// GUI fight notification of the client char (`FUN_100112fb` [GC 0x100112fb] with code 1 = started on `target`, 2 = stopped).
    GuiFight { started: bool, target: Option<Identity> },
    /// A hit landed (`FUN_1009b170`): for animation / sound; `flags` = `AttackInfo::unk_30` (4 critical, 2 glancing).
    Hit { attacker: i32, victim: i32, damage: i32, slot: i32, flags: i32 },
    /// An attack missed (`FUN_1006ae50`).
    Miss { attacker: i32, target: i32, slot: i32 },
    /// `SpecialAttackInfo`: the queued special now performs its swing on `target`.
    SpecialAttack { who: i32, target: Identity, special: i32, slot: i32, damage: i32 },
    /// A floating number above `dynel` (HUD space for the client char, world space otherwise).
    Floating { dynel: i32, amount: i32, category: u32, number: FloatingNumber },
    /// A chat/combat-log line.
    Log(LogLine),
    /// Health (stat 27) / max health (stat 1) of `dynel` changed.
    Health { dynel: i32, health: i32, max_health: i32, delta: i32 },
    /// Absolute stat applied by NewLevelIIR (GC 0x10075a0c), without StatIIR delta feedback.
    StatChanged { dynel: i32, stat: u32, value: i32 },
    /// Retail own-character NewLevel gate: social 6, level-complete sound, and GUI DValues.
    OwnNewLevel { dynel: i32, level: i32, animation: u16, sound: &'static str, got_ip: bool, got_perk: bool, got_tech: bool },
    /// `dynel` died: `cause` = `FUN_1005ae91` mode (1 terminate, 2 reflect, 3 shield, 4 weapon, 5 spell, 6 fall, 7 liquid) or
    /// 0 for the server's `CharacterAction` 99.
    Died { dynel: i32, cause: u32 },
}

/// `Feedback_DeathBy*` key of a death cause (`FUN_1005ae91` jump chain @ 0x1005aeb0).
pub fn death_key(cause: u32) -> Option<&'static str> {
    Some(match cause {
        1 => "Feedback_DeathByTerminate",
        2 => "Feedback_DeathByReflectDamage",
        3 => "Feedback_DeathByShieldDamage",
        4 => "Feedback_DeathByWeaponDamage",
        5 => "Feedback_DeathBySpellDamage",
        6 => "Feedback_DeathByFallDamage",
        7 => "Feedback_DeathByLiquidDamage",
        _ => return None,
    })
}

pub struct Combat {
    texts: Box<dyn Texts>,
    chars: HashMap<i32, Char>,
    own: i32,
    /// Stat `Features` (0xe0) of the playfield area (`FUN_10044b6e` [GC 0x10044b6e]); bit 0x800000 makes the client print the
    /// attacker's name for player hits (type 0x1d instead of 0x1e). [UNRESOLVED] source, 0 until the playfield layer sets it.
    pub area_features: i32,
    /// The weapon-slot tables (stat `DamageType` of the items), filled from the same messages.
    pub arms: Armory,
}

impl Combat {
    pub fn new(texts: Box<dyn Texts>) -> Self {
        Self { texts, chars: HashMap::new(), own: 0, area_features: 0, arms: Armory::default() }
    }

    /// Registers a character the way its `SimpleCharFullUpdate` would (for replays whose capture lacks that frame).
    #[cfg(test)]
    pub fn add_test_char(&mut self, id: i32, name: &str, npc: bool, health: i32) {
        let stats = [(STAT_HEALTH, health), (STAT_MAX_HEALTH, health), (STAT_LEVEL, 1)].into();
        self.chars.insert(id, Char { name: name.into(), npc, stats, ..Char::default() });
    }

    pub fn char(&self, instance: i32) -> Option<&Char> {
        self.chars.get(&instance)
    }

    /// Local stat-holder SetStat, not StatIIR feedback (`1006469b` Life undo).
    pub(super) fn set_health_local(&mut self, id: i32, health: i32) {
        if let Some(character) = self.chars.get_mut(&id) {
            character.stats.insert(STAT_HEALTH, health);
        }
    }

    pub fn fight(&self, instance: i32) -> Option<&Fight> {
        self.chars.get(&instance).map(|c| &c.fight)
    }

    pub fn is_fighting(&self, instance: i32) -> bool {
        self.fight(instance).is_some_and(|f| f.state != FIGHT_IDLE)
    }

    fn known(&self, id: Identity) -> Option<i32> {
        (id.kind == CHAR_KIND && self.chars.contains_key(&id.instance)).then_some(id.instance)
    }

    fn who(&self, instance: i32) -> Who<'_> {
        Who { id: instance, name: self.chars.get(&instance).map_or("", |c| c.name.as_str()), is_client: instance == self.own }
    }

    /// Apply one received frame. `own_id` = the player's character id (the `IsClientChar` dynel).
    pub fn on_frame(&mut self, f: &Frame, own_id: u32) -> Vec<CombatEvent> {
        let mut ev = Vec::new();
        self.own = own_id as i32;
        let Ok(m) = n3::decode(f) else { return ev };
        let h = m.header.target;
        self.arms.on_message(&m, self.chars.get(&h.instance).is_some_and(|c| c.npc));
        // Reuse the retail ReadSubClass decoder shared with chat; its delta is feedback,
        // not another subtraction after AttackInfo has already applied a hit.
        if matches!(m.body, N3::Unknown(_)) {
            match crate::play::chat::log::from_n3(&m) {
                Some(crate::play::chat::log::LogEvent::HealthDamage { who, health, death_cause, .. }) if who.kind == CHAR_KIND => {
                    self.set_health(who.instance, health, &mut ev);
                    if death_cause != 0 && self.chars.contains_key(&who.instance) {
                        self.die(who.instance, death_cause, &mut ev);
                    }
                }
                Some(crate::play::chat::log::LogEvent::NewLevel { who, f }) if who.kind == CHAR_KIND => {
                    if let Some(c) = self.chars.get_mut(&who.instance) {
                        for (stat, value) in [(0x36, f[0]), (0x34, f[2]), (0x35, f[1]), (0x39, f[3]), (0x15e, f[4]), (0x113, f[6])] {
                            c.stats.insert(stat as i32, value);
                            ev.push(CombatEvent::StatChanged { dynel: who.instance, stat, value });
                        }
                        if who.instance == self.own && f[5] > 0 {
                            c.stats.insert(0x25, f[5]);
                            ev.push(CombatEvent::StatChanged { dynel: who.instance, stat: 0x25, value: f[5] });
                        }
                        if who.instance == self.own {
                            let expansion = c.stat(0x185);
                            ev.push(CombatEvent::OwnNewLevel {
                                dynel: who.instance, level: f[0], animation: 6,
                                sound: "SM_Sandy_Game_Level_Complete", got_ip: true,
                                got_perk: f[0] % 10 == 0 && expansion & 2 != 0,
                                got_tech: f[0] == 5 && expansion & 0x20 != 0,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        match m.body {
            N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if h.kind == CHAR_KIND => {
                let c = self.chars.entry(h.instance).or_default();
                c.name.clone_from(&u.name);
                c.npc = u.is_npc();
                for (s, v) in [
                    (STAT_HEALTH, u.health),
                    (STAT_MAX_HEALTH, u.max_health),
                    (STAT_LEVEL, i32::from(u.level)),
                    (0x1a7, i32::from(u.stat_1a7)),
                ] {
                    c.stats.insert(s, v);
                }
                ev.push(CombatEvent::Health { dynel: h.instance, health: u.health, max_health: u.max_health, delta: 0 });
                if let Some(t) = u.target {
                    self.start_fight(h.instance, t, &mut ev);
                }
            }
            N3::World(World::FullCharacter(fc)) if h.kind == CHAR_KIND => {
                let c = self.chars.entry(h.instance).or_default();
                // PRK's `FullCharacter` carries `Life` (1) = 1: the client never uses it as the maximum (it computes the own one,
                // `ao_formats::stats::pools`, and every other character's comes from the `SimpleCharFullUpdate` header), so the
                // stat must not replace the header's `max_health`.
                let mut put = |s: i32, v: i32| {
                    if s != STAT_MAX_HEALTH {
                        c.stats.insert(s, v);
                    }
                };
                for &(s, v) in fc.stats_a.iter().chain(&fc.stats_b) {
                    put(s as i32, v);
                }
                for &(s, v) in &fc.stats_u8 {
                    put(i32::from(s), i32::from(v));
                }
                for &(s, v) in &fc.stats_i16 {
                    put(i32::from(s), i32::from(v));
                }
                for &(s, v) in &fc.stat_map {
                    put(s, v);
                }
                ev.push(CombatEvent::Health { dynel: h.instance, health: c.health(), max_health: c.max_health(), delta: 0 });
            }
            N3::World(World::CharacterAction(a)) if h.kind == CHAR_KIND && death_anim_from_action(a.action, a.identity_b.instance as u32).is_some() => {
                if let Some(c) = self.chars.get_mut(&h.instance) {
                    c.dead = true;
                    // `FUN_1005d0d8` case 0x19: stat 0x183 := `identity_b.instance`, read by the `CharDie_t` ctor (`GetSkill(0x183)`)
                    c.stats.insert(STAT_DEATH_ANIM as i32, a.identity_b.instance);
                    self.stop_fight(h.instance, &mut ev);
                    ev.push(CombatEvent::Died { dynel: h.instance, cause: 0 });
                }
            }
            // `FUN_1005d0d8` case 0x32 (action 0x99, handler 0x1005d861): `FUN_1005ae91(identity_b.instance)` on the receiving character = the same
            // death routine as the `AttackInfo` death cause (message for the client char, `Health = 0`, "items will be reclaimed")
            N3::World(World::CharacterAction(a)) if h.kind == CHAR_KIND && a.action == ACTION_DEATH_CAUSE && self.chars.contains_key(&h.instance) => {
                self.die(h.instance, a.identity_b.instance, &mut ev);
            }
            // `FUN_1005d0d8` case 0x5a (action 0xd0, handler 0x1005e8b5): `SetStat(identity_b.kind, identity_b.instance)` on the receiving (drained)
            // character; the "You drained .." line is the chat log's (`chat/log.rs`)
            N3::World(World::CharacterAction(a)) if h.kind == CHAR_KIND && a.action == ACTION_DRAIN && self.chars.contains_key(&h.instance) => {
                if a.identity_b.kind == STAT_HEALTH {
                    self.set_health(h.instance, a.identity_b.instance, &mut ev);
                } else if let Some(c) = self.chars.get_mut(&h.instance) {
                    c.stats.insert(a.identity_b.kind, a.identity_b.instance);
                }
            }
            N3::Dynel(Dynel::Stat(s)) if h.kind == CHAR_KIND => self.stats(h.instance, &s.stats, &mut ev),
            N3::Misc(Misc::Attack(a)) if h.kind == CHAR_KIND => {
                if self.chars.contains_key(&h.instance) {
                    self.start_fight(h.instance, a.target, &mut ev);
                }
            }
            N3::Misc(Misc::StopFight(_)) if h.kind == CHAR_KIND => self.stop_fight(h.instance, &mut ev),
            N3::Misc(Misc::AttackInfo(a)) => {
                if h.kind == CHAR_KIND && self.chars.contains_key(&h.instance) {
                    self.hit(h.instance, &a, &mut ev);
                } else if self.known(a.other) == Some(self.own) {
                    self.unattributed_hit(a.damage, a.unk_2c, &mut ev);
                }
            }
            N3::Misc(Misc::SpecialAttackInfo(a)) if h.kind == CHAR_KIND && self.chars.contains_key(&h.instance) => {
                self.special_hit(h.instance, &a, &mut ev);
            }
            N3::Misc(Misc::MissedAttackInfo(a)) if h.kind == CHAR_KIND && self.chars.contains_key(&h.instance) => {
                self.miss(a.slot, a.source, a.target, a.stat, &mut ev);
            }
            N3::Misc(Misc::CharSecSpecAttack(a)) if h.kind == CHAR_KIND => {
                if let Some(c) = self.chars.get_mut(&h.instance) {
                    // FUN_10068790: only an empty deque accepts a special; otherwise retail activates its GUI action.
                    if c.fight.pending_specials.is_empty() {
                        c.fight.pending_specials.push(a.special);
                    }
                }
            }
            N3::Misc(Misc::ToClientQuit) if h.kind == CHAR_KIND => {
                self.chars.remove(&h.instance);
            }
            _ => {}
        }
        ev
    }

    // ---- fight controller ------------------------------------------------------------------------------------

    /// `FUN_10069556(target, report = 0)` [GC 0x10069556], the part that does not depend on district / PvP data (see doc):
    /// the target must be a known character other than the attacker and not dead. The remaining rules (swimming,
    /// surrender, district fight-mode levels, team members) need movement/team state this layer does not own.
    fn can_attack(&self, attacker: i32, target: i32) -> bool {
        attacker != target && self.chars.get(&target).is_some_and(|t| !t.dead)
    }

    /// `FUN_10069c68(target, 0)` start fight of `att`.
    pub fn start_fight(&mut self, att: i32, target: Identity, ev: &mut Vec<CombatEvent>) {
        let Some(t) = self.known(target) else { return };
        if !self.can_attack(att, t) {
            return;
        }
        let was_fighting = self.is_fighting(att);
        if was_fighting {
            self.stop_fight(att, ev); // FUN_10068b7f
        }
        let client = att == self.own;
        if client {
            ev.push(CombatEvent::CombatMusic(true));
            ev.push(CombatEvent::GuiFight { started: true, target: Some(target) });
        }
        if let Some(c) = self.chars.get_mut(&att) {
            c.fight.state = FIGHT_FIGHTING;
            c.fight.target = Some(target);
            c.fight.last_target = Some(target);
        }
        // log lines (target resolved): "Attacking %s..." (client attacker) / "Attacked by %s!" (client target)
        let level = self.chars.get(&att).map_or(0, |c| c.stat(STAT_LEVEL));
        if client {
            let line = log::LdbFormat::new(&self.texts.feedback("Feedback_Attacking").unwrap_or_default())
                .feed(log::Arg::Str(&self.chars[&t].name))
                .dump();
            self.push_log(ev, 0xc, 0, line);
            if level < 2 {
                self.plain(ev, "Feedback_UseAggDefSlider");
            }
        } else if t == self.own {
            let line = log::LdbFormat::new(&self.texts.feedback("Feedback_AttackedBy").unwrap_or_default())
                .feed(log::Arg::Str(&self.chars[&att].name))
                .dump();
            self.push_log(ev, 0xc, 0, line);
        }
        ev.push(CombatEvent::FightStarted { who: att, target, switched: was_fighting });
    }

    /// `FUN_10068b7f(1, 0)` stop fight.
    pub fn stop_fight(&mut self, att: i32, ev: &mut Vec<CombatEvent>) {
        let Some(c) = self.chars.get_mut(&att) else { return };
        c.fight.pending_specials.clear(); // FUN_1005548b at 10068c93.
        if att == self.own {
            ev.push(CombatEvent::GuiFight { started: false, target: None });
            ev.push(CombatEvent::CombatMusic(false));
            if c.health() <= 0 {
                ev.push(CombatEvent::DeathMusic(true));
            }
        }
        if c.fight.state != FIGHT_IDLE {
            c.fight.state = FIGHT_IDLE;
            c.fight.target = None;
            ev.push(CombatEvent::FightStopped { who: att });
        }
    }

    // ---- hits ------------------------------------------------------------------------------------------------

    fn push_log(&self, ev: &mut Vec<CombatEvent>, category: u32, style: u32, text: String) {
        if !text.is_empty() {
            ev.push(CombatEvent::Log(LogLine { category, style, text }));
        }
    }

    /// `FUN_10058b00(key, 0)` / `FUN_10012b05(0, text, 0)`: a plain feedback text (style 0, category 0).
    fn plain(&self, ev: &mut Vec<CombatEvent>, key: &str) {
        if let Some(t) = self.texts.feedback(key) {
            self.push_log(ev, 0, 0, log::LdbFormat::new(&t).dump());
        }
    }

    /// Run the feedback formatter and turn its result into events (`FUN_10012bd5` tail: chat line + floating number).
    fn feedback(&self, ev: &mut Vec<CombatEvent>, f: &Feedback, subject: i32) {
        let Some(r) = render(self.texts.as_ref(), f) else { return };
        self.push_log(ev, r.category, r.style, r.text);
        if let Some(n) = r.number {
            // `FUN_100044c2` (HUD) for the client char unless `DAT_102e0640`, else `FUN_10011108` (world effect on A)
            let space = if f.a.is_client { Space::Hud } else { Space::World };
            ev.push(CombatEvent::Floating {
                dynel: subject,
                amount: n,
                category: r.category,
                number: log::floating_number(space, r.category),
            });
        }
    }

    fn set_health(&mut self, id: i32, health: i32, ev: &mut Vec<CombatEvent>) {
        if let Some(c) = self.chars.get_mut(&id) {
            let old = c.health();
            c.stats.insert(STAT_HEALTH, health);
            ev.push(CombatEvent::Health { dynel: id, health, max_health: c.max_health(), delta: health - old });
        }
    }

    /// `FUN_1005ae91(cause)` on `id`: death message for the client char, `Health = 0`, "items will be reclaimed" for a player.
    pub fn die(&mut self, id: i32, cause: i32, ev: &mut Vec<CombatEvent>) {
        let client = id == self.own;
        if client {
            if let Some(k) = death_key(cause as u32) {
                self.plain(ev, k);
            }
        }
        self.set_health(id, 0, ev);
        ev.push(CombatEvent::Died { dynel: id, cause: cause as u32 });
        let Some(c) = self.chars.get(&id) else { return };
        if !c.npc && client && c.stat(STAT_LEVEL) >= 1000 {
            let n = match c.stat(0x22) {
                n if n > 0 => n,
                _ => 0x4b,
            };
            if let Some(t) = self.texts.feedback("Feedback_ItemsWillBeReclaimed") {
                let line = log::LdbFormat::new(&t).feed(log::Arg::Int(n)).dump();
                self.push_log(ev, 0, 0, line);
            }
        }
    }

    /// `AttackInfoIIR_t` with a known header `att`: `FUN_1006a8f3(slot, damage, _, death cause, hit flags, special key)`.
    /// The victim is the controller target of `att` (`FUN_100676bd` = `GetDynel(ctrl+0x4c)`), NOT the message's `other`
    /// identity. The damage type is stat `0x1b4` of the item behind the weapon slot ([`Armory::damage_type`]); a slot the table does
    /// not know (the original skips the hit then) is printed with the item default ([`DEFAULT_DAMAGE_TYPE`], [GUESS]). Quirk kept from the
    /// client: the death cause of this message is stored at the victim controller `+0x7c` *after* `FUN_1009b170` ran, so a cause is
    /// acted on after the next hit on that victim.
    pub fn hit(&mut self, att: i32, a: &AttackInfo, ev: &mut Vec<CombatEvent>) {
        let (slot, damage, death_cause, flags) = (a.slot, a.damage, a.unk_2c, a.unk_30);
        if !valid_slot(slot) {
            return;
        }
        let Some(v) = self.chars.get(&att).and_then(|c| c.fight.target).and_then(|t| self.known(t)) else { return };
        // type selection of FUN_1009b170
        let attacker_player = !self.chars[&att].npc;
        let ty = if v == self.own {
            // FUN_10058a05: player attacker && area feature bit 0x800000 -> named hit
            if attacker_player && self.area_features & 0x80_0000 == 0 {
                log::ty::PLAYER_HIT_YOU
            } else {
                log::ty::HIT_YOU
            }
        } else if att == self.own {
            log::ty::YOU_HIT
        } else {
            log::ty::OTHER_HIT_OTHER
        };
        let mut f = Feedback::new(ty, self.who(v), damage);
        f.b = Some(self.who(att));
        // `FUN_1009b170`: a valid stat `0x153` of the attacker (nano) overrides the weapon's `FUN_1009afde`
        let over = Some(self.chars[&att].stat(0x153)).filter(|&v| v != 0 && log::weapon_damage_type(v) == v);
        let weapon = self.arms.damage_type(att, slot, a.unk_34).unwrap_or(DEFAULT_DAMAGE_TYPE);
        f.stat = over.unwrap_or_else(|| log::weapon_damage_type(weapon));
        f.hit = flags;
        self.feedback(ev, &f, v);
        ev.push(CombatEvent::Hit { attacker: att, victim: v, damage, slot, flags });
        let h = self.chars[&v].health();
        self.set_health(v, h - damage, ev);
        let pending = self.chars[&v].fight.death_cause;
        if pending != 0 {
            self.die(v, pending, ev);
        }
        if let Some(c) = self.chars.get_mut(&v) {
            c.fight.death_cause = death_cause;
        }
    }

    /// `AttackInfoIIR_t` whose header dynel is unknown but `other` is the client char: `FUN_100693a3(damage)`.
    fn unattributed_hit(&mut self, damage: i32, death_cause: i32, ev: &mut Vec<CombatEvent>) {
        if let Some(t) = self.texts.feedback("Feedback_YouWereHitForPointsOfDamage") {
            let line = log::LdbFormat::new(&t).feed(log::Arg::UInt(damage as u32)).dump();
            self.push_log(ev, 0x17, 0, line);
        }
        let own = self.own;
        if let Some(h) = self.chars.get(&own).map(Char::health) {
            self.set_health(own, h - damage, ev);
            if death_cause != 0 {
                self.die(own, death_cause, ev);
            }
        }
    }

    /// `SpecialAttackInfoIIR_t`: `FUN_1006a9c5`.
    pub fn special_hit(&mut self, att: i32, a: &n3::misc::SpecialAttackInfo, ev: &mut Vec<CombatEvent>) {
        let (slot, damage, target, special, death_cause) = (a.slot, a.damage, a.target, a.special, a.unk_30);
        let Some(t) = self.known(target) else { return };
        // FUN_1006a9c5 starts the swing before feedback; FUN_1006855a reads the deque's front, not the result's stat.
        if let Some(queued) = self.chars.get(&att).and_then(|c| c.fight.pending_specials.first()).copied() {
            ev.push(CombatEvent::SpecialAttack { who: att, target, special: queued, slot, damage });
        } else {
            ev.push(CombatEvent::SpecialAttack { who: att, target, special: 0, slot, damage });
        }
        let name = self.texts.stat_name(special as u32).unwrap_or_default();
        let ty = if att == self.own {
            log::ty::SPECIAL_YOU_HIT
        } else if t == self.own {
            log::ty::SPECIAL_HIT_YOU
        } else {
            log::ty::SPECIAL_OTHER // the PvP flag variant 0x33 needs `FUN_100523c3`, not ported
        };
        let mut f = Feedback::new(ty, self.who(t), damage);
        f.b = Some(self.who(att));
        f.extra = Some(&name);
        self.feedback(ev, &f, t);
        let h = self.chars[&t].health();
        self.set_health(t, h - damage, ev);
        if death_cause != 0 {
            self.die(t, death_cause, ev);
        }
        // FUN_1005548b at 1006abed clears the entire deque after applying the result.
        if let Some(c) = self.chars.get_mut(&att) {
            c.fight.pending_specials.clear();
        }
    }

    /// `MissedAttackInfoIIR_t`: `FUN_1006ae50(slot, _, source, target, stat)`.
    pub fn miss(&mut self, slot: i32, source: Identity, target: Identity, stat: i32, ev: &mut Vec<CombatEvent>) {
        let (Some(b), Some(a)) = (self.known(source), self.known(target)) else { return };
        let name = (stat != 0).then(|| log::stat_to_string(stat as u32));
        let mut f = Feedback::new(log::ty::MISS, self.who(a), 1);
        f.b = Some(self.who(b));
        f.extra = name.as_deref();
        self.feedback(ev, &f, a);
        ev.push(CombatEvent::Miss { attacker: b, target: a, slot });
    }

    // ---- stats -----------------------------------------------------------------------------------------------

    /// `StatIIR_t` apply [GC 0x100a1aaf]: for every pair (ascending stat id, duplicates overwritten) show the combat text of the
    /// delta against the current value, then store the value.
    pub fn stats(&mut self, id: i32, pairs: &[(i32, i32)], ev: &mut Vec<CombatEvent>) {
        let Some(c) = self.chars.get(&id) else { return };
        let map: BTreeMap<i32, i32> = pairs.iter().copied().collect();
        let client = id == self.own;
        // alien level raised by this very message?
        let level_up = map.get(&STAT_ALIEN_LEVEL).is_some_and(|&v| c.stat(STAT_ALIEN_LEVEL) < v);
        for (&stat, &val) in &map {
            let Some(c) = self.chars.get(&id) else { return };
            let cur = c.stat(stat);
            let delta = val - cur;
            let alien_base = c.stat(STAT_ALIEN_XP_BASE);
            let mut fb: Option<(u32, i32)> = None;
            match stat {
                STAT_HEALTH => {
                    if delta < 0 {
                        // 0x1a client char; 0x1c a pet of the local player (FUN_100523c3, not ported); else 0x1b
                        let ty = if client { log::ty::SELF_ATTACKED } else { log::ty::OTHER_ATTACKED };
                        fb = Some((ty, delta.abs()));
                    } else if delta >= 1 && client {
                        fb = Some((log::ty::HEALED, delta));
                    }
                }
                STAT_XP if client => fb = Some((log::ty::XP, delta)),
                STAT_ALIEN_XP if client => fb = Some((log::ty::ALIEN_XP, if level_up { alien_base + delta } else { delta })),
                STAT_SHADOW_KNOWLEDGE if client => fb = Some((log::ty::SHADOW_KNOWLEDGE, delta)),
                _ => {}
            }
            if let Some((ty, value)) = fb {
                let f = Feedback::new(ty, self.who(id), value);
                self.feedback(ev, &f, id);
            }
            if let Some(c) = self.chars.get_mut(&id) {
                c.stats.insert(stat, val);
                if stat == STAT_HEALTH || stat == STAT_MAX_HEALTH {
                    ev.push(CombatEvent::Health { dynel: id, health: c.health(), max_health: c.max_health(), delta: if stat == STAT_HEALTH { delta } else { 0 } });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::log::fake::Fixed;
    use super::*;
    use ao_net::msg::Identity;
    use ao_net::n3::misc::{Attack, AttackInfo, MissedAttackInfo, SpecialAttackInfo};

    /// Codec-layout fixtures, not a live level-up capture.
    #[test]
    fn authoritative_level_wire_preserves_retail_own_gate() {
        let packet = |kind: u32, id: i32, words: &[i32]| {
            let mut payload = kind.to_be_bytes().to_vec();
            payload.extend_from_slice(&CHAR_KIND.to_be_bytes());
            payload.extend_from_slice(&id.to_be_bytes());
            payload.push(0);
            for word in words { payload.extend_from_slice(&word.to_be_bytes()); }
            Frame { seq: 1, ptype: ao_net::frame::PT_N3, sender: 1, receiver: 1, payload }
        };
        let mut c = setup();
        let notices = |events: &[CombatEvent]| events.iter().filter(|event| matches!(event, CombatEvent::OwnNewLevel { .. })).count();
        for id in [1, 3] {
            let new_level = packet(0x7f40_5a16, id, &[2, 100, 200, 150, 300, 2, 8, 50]);
            let events = c.on_frame(&new_level, 1);
            assert_eq!(notices(&events), usize::from(id == 1));
            assert_eq!(c.char(id).unwrap().stat(STAT_LEVEL), 2);
            // Retail NewLevel's own gate is unconditional; repeated packets repeat its notice.
            assert_eq!(notices(&c.on_frame(&new_level, 1)), usize::from(id == 1));
            let stat = packet(ao_net::n3::dynel::STAT, id, &[1, STAT_LEVEL, 2]);
            assert_eq!(notices(&c.on_frame(&stat, 1)), 0);
            let stat = packet(ao_net::n3::dynel::STAT, id, &[1, STAT_LEVEL, 3]);
            assert_eq!(notices(&c.on_frame(&stat, 1)), 0);
            let new_level = packet(0x7f40_5a16, id, &[3, 100, 200, 150, 300, 0, 8, 50]);
            assert_eq!(notices(&c.on_frame(&new_level, 1)), usize::from(id == 1));
            let lower = packet(ao_net::n3::dynel::STAT, id, &[1, STAT_LEVEL, 2]);
            assert_eq!(notices(&c.on_frame(&lower, 1)), 0);
        }
        c.chars.get_mut(&3).unwrap().stats.remove(&STAT_LEVEL);
        assert_eq!(notices(&c.on_frame(&packet(ao_net::n3::dynel::STAT, 3, &[1, STAT_LEVEL, 10]), 1)), 0);
        c.chars.get_mut(&1).unwrap().stats.remove(&STAT_LEVEL);
        c.chars.get_mut(&1).unwrap().stats.insert(0x185, 0x22);
        let events = c.on_frame(&packet(0x7f40_5a16, 1, &[10, 0, 0, 0, 0, 0, 0, 0]), 1);
        assert!(events.iter().any(|event| matches!(event, CombatEvent::OwnNewLevel {
            animation: 6, sound: "SM_Sandy_Game_Level_Complete", got_ip: true, got_perk: true, got_tech: false, ..
        })));
        let events = c.on_frame(&packet(0x7f40_5a16, 1, &[5, 0, 0, 0, 0, 0, 0, 0]), 1);
        assert!(events.iter().any(|event| matches!(event, CombatEvent::OwnNewLevel { got_perk: false, got_tech: true, .. })));
        assert_eq!(notices(&c.on_frame(&packet(0x7f40_5a16, 99, &[10, 0, 0, 0, 0, 0, 0, 0]), 1)), 0);
    }

    fn ch(i: i32) -> Identity {
        Identity { kind: CHAR_KIND, instance: i }
    }

    fn setup() -> Combat {
        let mut c = Combat::new(Box::new(Fixed::new()));
        c.own = 1;
        for (i, n, npc, hp) in [(1, "Testy", false, 100), (2, "Junkbot", true, 50), (3, "Bystander", false, 30)] {
            c.chars.insert(
                i,
                Char { name: n.into(), npc, stats: [(STAT_HEALTH, hp), (STAT_MAX_HEALTH, hp), (STAT_LEVEL, 1)].into(), ..Char::default() },
            );
        }
        c
    }

    #[test]
    fn start_stop_fight() {
        let mut c = setup();
        let mut ev = Vec::new();
        c.start_fight(1, ch(2), &mut ev);
        assert!(c.is_fighting(1));
        assert_eq!(c.fight(1).unwrap().target, Some(ch(2)));
        assert!(ev.contains(&CombatEvent::CombatMusic(true)));
        let lines: Vec<_> = ev.iter().filter_map(|e| if let CombatEvent::Log(l) = e { Some(l.text.as_str()) } else { None }).collect();
        assert_eq!(lines[0], "Attacking Junkbot...");
        assert!(lines[1].starts_with("Use the Def-Agg slider"), "level < 2: {lines:?}");
        // switching target stops first
        ev.clear();
        c.start_fight(1, ch(3), &mut ev);
        assert!(matches!(ev[0], CombatEvent::GuiFight { started: false, .. }), "{ev:?}");
        assert!(ev.contains(&CombatEvent::FightStarted { who: 1, target: ch(3), switched: true }));
        ev.clear();
        c.stop_fight(1, &mut ev);
        assert!(!c.is_fighting(1) && c.fight(1).unwrap().target.is_none());
        assert_eq!(ev, vec![
            CombatEvent::GuiFight { started: false, target: None },
            CombatEvent::CombatMusic(false),
            CombatEvent::FightStopped { who: 1 },
        ]);
        // self / dead / unknown targets are refused silently (can_attack)
        ev.clear();
        c.start_fight(1, ch(1), &mut ev);
        c.start_fight(1, ch(99), &mut ev);
        c.chars.get_mut(&2).unwrap().dead = true;
        c.start_fight(1, ch(2), &mut ev);
        assert!(ev.is_empty() && !c.is_fighting(1));
    }

    fn atk(slot: i32, damage: i32, death: i32, flags: i32) -> AttackInfo {
        AttackInfo { damage, value_20: -1, slot, other: ch(0), unk_2c: death, unk_30: flags, unk_34: 0 }
    }

    /// The damage type of a hit line is stat `0x1b4` of the item behind the `AttackInfo` slot (`FUN_1009afde`), the nano stat `0x153` of the
    /// attacker overriding it; the old hard-coded 0x5a ("projectile") for every unarmed / melee hit is gone.
    #[test]
    fn hit_line_damage_type_comes_from_the_slot_item() {
        let line = |c: &mut Combat, att: i32, a: AttackInfo| {
            let mut ev = Vec::new();
            c.hit(att, &a, &mut ev);
            match &ev[0] {
                CombatEvent::Log(l) => l.text.clone(),
                e => panic!("{e:?}"),
            }
        };
        let mut c = setup();
        c.start_fight(1, ch(2), &mut Vec::new());
        c.arms.list(1, false, &[(43712, 100)]);
        assert_eq!(line(&mut c, 1, atk(0, 3, 0, 3)), "You hit Junkbot for 3 points of melee damage.");
        // a pistol in the right hand, an energy weapon in the left
        c.arms.wield(1, 901, 6, None, &[(0x1b4, 0x5a)]);
        c.arms.wield(1, 902, 8, None, &[(0x1b4, 0x5c)]);
        assert_eq!(line(&mut c, 1, atk(6, 3, 0, 3)), "You hit Junkbot for 3 points of projectile damage.");
        assert_eq!(line(&mut c, 1, atk(8, 3, 0, 3)), "You hit Junkbot for 3 points of energy damage.");
        // a stat 0x1b4 outside 0x5a..=0x61 / 0xa8 is the 0x5a default of `FUN_1009afde`
        c.arms.wield(1, 903, 5, None, &[(0x1b4, 0x1234)]);
        assert_eq!(line(&mut c, 1, atk(5, 3, 0, 3)), "You hit Junkbot for 3 points of projectile damage.");
        // the attacker's nano stat 0x153 overrides the weapon
        c.chars.get_mut(&1).unwrap().stats.insert(0x153, 0x5d);
        assert_eq!(line(&mut c, 1, atk(6, 3, 0, 3)), "You hit Junkbot for 3 points of chemical damage.");
    }

    #[test]
    fn npc_attacks_you_text_number_health() {
        let mut c = setup();
        let mut ev = Vec::new();
        c.start_fight(2, ch(1), &mut ev);
        assert!(matches!(&ev[0], CombatEvent::Log(l) if l.text == "Attacked by Junkbot!" && l.category == 0xc));
        ev.clear();
        c.arms.wield(2, 900, 2, None, &[(0x1b4, 0x5a)]);
        c.hit(2, &atk(2, 17, 0, 3), &mut ev);
        let CombatEvent::Log(l) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!((l.text.as_str(), l.category, l.style), ("Junkbot hit you for 17 points of projectile damage.", 0x17, 0x4200_0006));
        let CombatEvent::Floating { dynel, amount, category, number } = &ev[1] else { panic!() };
        assert_eq!((*dynel, *amount, *category, number.space), (1, 17, 0x17, Space::Hud));
        assert_eq!(number.color, 0xff0000);
        assert_eq!(c.char(1).map(|c| (c.health(), c.max_health())), Some((83, 100)));
        assert!(ev.contains(&CombatEvent::Health { dynel: 1, health: 83, max_health: 100, delta: -17 }));
    }

    #[test]
    fn you_hit_world_number_and_crit() {
        let mut c = setup();
        let mut ev = Vec::new();
        c.start_fight(1, ch(2), &mut ev);
        ev.clear();
        c.arms.list(1, false, &[(43712, 100), (43713, 144)]);
        c.hit(1, &atk(0, 8, 0, 4), &mut ev);
        let CombatEvent::Log(l) = &ev[0] else { panic!() };
        // bare hands: slot 0 = the martial-arts item, whose record has no stat 0x1b4 -> the item default 0x5b
        assert_eq!(l.text, "You hit Junkbot for 8 points of melee damage. Critical hit!");
        let CombatEvent::Floating { dynel, number, .. } = &ev[1] else { panic!() };
        assert_eq!((*dynel, number.space, number.life), (2, Space::World, 1.3));
        assert_eq!(c.char(2).map(|c| (c.health(), c.max_health())), Some((42, 50)));
        // invalid slot: ignored
        ev.clear();
        c.hit(1, &atk(20, 8, 0, 3), &mut ev);
        assert!(ev.is_empty());
    }

    #[test]
    fn death_cause_is_applied_after_the_next_hit() {
        let mut c = setup();
        let mut ev = Vec::new();
        c.start_fight(1, ch(2), &mut ev);
        c.hit(1, &atk(0, 50, 4, 3), &mut ev);
        assert_eq!(c.fight(2).unwrap().death_cause, 4);
        assert_eq!(c.char(2).map(|c| (c.health(), c.max_health())), Some((0, 50)));
        ev.clear();
        c.hit(1, &atk(0, 1, 0, 3), &mut ev);
        assert!(ev.iter().any(|e| matches!(e, CombatEvent::Died { dynel: 2, cause: 4 })), "{ev:?}");
        assert_eq!(c.char(2).map(|c| (c.health(), c.max_health())), Some((0, 50)));
    }

    /// `CharacterActionIIR_t` 0x99 (handler 0x1005d861) runs `FUN_1005ae91(identity_b.instance)` on the receiving character.
    #[test]
    fn action_0x99_is_the_death_routine() {
        use ao_net::n3::action::{character_action_for, simple};
        let mut c = setup();
        let frame = |who: i32, cause: i32| {
            let a = simple(ACTION_DEATH_CAUSE, Identity::default(), Identity { kind: 0, instance: cause });
            ao_net::n3::outgoing::n3_frame(0, 1, character_action_for(ch(who), &a))
        };
        let ev = c.on_frame(&frame(2, 4), 1);
        assert!(ev.iter().any(|e| matches!(e, CombatEvent::Died { dynel: 2, cause: 4 })), "{ev:?}");
        assert_eq!(c.char(2).map(|c| c.health()), Some(0));
        // unknown characters are ignored
        assert!(c.on_frame(&frame(99, 4), 1).is_empty());
        let ev = c.on_frame(&frame(1, 6), 1);
        assert!(ev.iter().any(|e| matches!(e, CombatEvent::Died { dynel: 1, cause: 6 })), "{ev:?}");
        assert_eq!(c.char(1).map(|c| c.health()), Some(0));
    }

    #[test]
    fn miss_and_special() {
        let mut c = setup();
        let mut ev = Vec::new();
        c.miss(2, ch(2), ch(1), 0, &mut ev);
        assert!(matches!(&ev[0], CombatEvent::Log(l) if l.text == "Junkbot tried to hit you, but missed!"));
        assert!(matches!(ev[1], CombatEvent::Miss { attacker: 2, target: 1, slot: 2 }));
        ev.clear();
        c.miss(0, ch(1), ch(2), 142, &mut ev);
        assert!(matches!(&ev[0], CombatEvent::Log(l) if l.text == "You try to attack Junkbot with Brawl, but you miss!"));
        ev.clear();
        c.special_hit(1, &SpecialAttackInfo { slot: 0, damage: 20, target: ch(2), special: 142, value_28: -1, unk_30: 0 }, &mut ev);
        assert!(ev.iter().any(|e| matches!(e, CombatEvent::Log(l) if l.text == "You hit Junkbot for 20 points of Brawling damage.")));
        assert_eq!(c.char(2).map(|c| (c.health(), c.max_health())), Some((30, 50)));
        assert_eq!(ev[0], CombatEvent::SpecialAttack { who: 1, target: ch(2), special: 0, slot: 0, damage: 20 });
    }

    #[test]
    fn special_result_uses_queue_front_then_clears_the_deque() {
        let mut c = setup();
        c.chars.get_mut(&1).unwrap().fight.pending_specials = vec![142, 144];
        let mut ev = Vec::new();
        c.special_hit(1, &SpecialAttackInfo { slot: 6, damage: 1, target: ch(2), special: 144, value_28: -1, unk_30: 0 }, &mut ev);
        assert_eq!(ev[0], CombatEvent::SpecialAttack { who: 1, target: ch(2), special: 142, slot: 6, damage: 1 });
        assert!(c.fight(1).unwrap().pending_specials.is_empty());
    }

    #[test]
    fn stat_updates() {
        let mut c = setup();
        let mut ev = Vec::new();
        // own health drops by 10 via StatIIR: "attacked with nanobots" line, HUD number
        c.stats(1, &[(STAT_HEALTH, 90)], &mut ev);
        let CombatEvent::Log(l) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(l.text, "You were attacked with nanobots for 10 points of unknown damage.");
        assert!(matches!(&ev[1], CombatEvent::Floating { amount: 10, number, .. } if number.space == Space::Hud));
        // heal
        ev.clear();
        c.stats(1, &[(STAT_HEALTH, 95)], &mut ev);
        assert!(matches!(&ev[0], CombatEvent::Log(l) if l.text == "You were healed for 5 points."), "{ev:?}");
        // an equal value prints nothing; another char losing health prints the third-person line
        ev.clear();
        c.stats(1, &[(STAT_HEALTH, 95)], &mut ev);
        assert!(matches!(ev[..], [CombatEvent::Health { delta: 0, .. }]));
        ev.clear();
        c.stats(3, &[(STAT_HEALTH, 20)], &mut ev);
        assert!(matches!(&ev[1], CombatEvent::Floating { dynel: 3, amount: 10, number, .. } if number.space == Space::World), "{ev:?}");
        // xp
        ev.clear();
        c.stats(1, &[(STAT_XP, 500)], &mut ev);
        assert!(matches!(&ev[0], CombatEvent::Log(l) if l.text == "You received 500 xp."));
    }

    #[test]
    fn unattributed_hit_on_the_client_char() {
        let mut c = setup();
        let mut ev = Vec::new();
        c.unattributed_hit(12, 0, &mut ev);
        assert!(matches!(&ev[0], CombatEvent::Log(l) if l.text == "You were hit for 12 points of damage." && l.category == 0x17 && l.style == 0));
        assert_eq!(c.char(1).map(|c| (c.health(), c.max_health())), Some((88, 100)));
    }

    // ---- replay of the live capture (docs/captures/zone_ithaca.rec, own character 25988 "Testy") --------------

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

    fn replay(texts: Box<dyn Texts>) -> Vec<CombatEvent> {
        let mut c = Combat::new(texts);
        frames().iter().flat_map(|f| c.on_frame(f, OWN)).collect()
    }

    #[test]
    fn captured_specials_queue_until_the_result_for_own_and_observer() {
        for own in [OWN, 33402] {
            let mut c = Combat::new(Box::new(Fixed::new()));
            let mut results = 0;
            for f in frames() {
                let Ok(m) = n3::decode(&f) else { continue };
                let who = m.header.target.instance;
                let queued = matches!(&m.body, N3::Misc(Misc::CharSecSpecAttack(_)));
                let result = match &m.body {
                    N3::Misc(Misc::SpecialAttackInfo(a)) => Some(a.special),
                    _ => None,
                };
                let ev = c.on_frame(&f, own);
                if queued {
                    assert!(!ev.iter().any(|e| matches!(e, CombatEvent::SpecialAttack { .. })));
                }
                if let Some(special) = result {
                    assert!(ev.iter().any(|e| matches!(e, CombatEvent::SpecialAttack { who: w, special: s, .. } if *w == who && *s == special)));
                    assert!(!c.fight(who).unwrap().pending_specials.contains(&special));
                    results += 1;
                }
            }
            assert_eq!(results, 3);
        }
    }

    fn real_texts() -> Option<ao_formats::screens::TextDb> {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        ao_formats::screens::TextDb::load(&dir).ok()
    }

    /// The own `FullCharacter` carries `Life` (1) = 1: the `SimpleCharFullUpdate` header's maximum health stays.
    #[test]
    fn full_character_life_does_not_replace_the_header_max_health() {
        let mut c = Combat::new(Box::new(super::log::fake::Fixed::new()));
        let (mut header, mut full) = (None, false);
        for f in frames() {
            if let Ok(m) = n3::decode(&f) {
                match &m.body {
                    N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if m.header.target.instance == OWN as i32 => header = Some(u.max_health),
                    N3::World(World::FullCharacter(_)) if m.header.target.instance == OWN as i32 => full = true,
                    _ => {}
                }
            }
            c.on_frame(&f, OWN);
        }
        assert!(full && header.is_some(), "the capture has both frames of the own character");
        assert_eq!(c.char(OWN as i32).map(Char::max_health), header);
    }

    fn count(ev: &[CombatEvent], p: fn(&CombatEvent) -> bool) -> usize {
        ev.iter().filter(|e| p(e)).count()
    }

    fn lines(ev: &[CombatEvent]) -> Vec<&str> {
        ev.iter().filter_map(|e| if let CombatEvent::Log(l) = e { Some(l.text.as_str()) } else { None }).collect()
    }

    /// Live capture, observer view (own char 25988 is not fighting): 134 AttackInfo, 128 Attack, 128 StopFight, 11 Missed,
    /// 3 SpecialAttackInfo (+ 3 CharSecSpecAttack, 15 deaths).
    #[test]
    fn replay_counts() {
        let ev = replay(Box::new(Fixed::new()));
        // 12 of the 134 AttackInfo have a header whose controller has no target (its Attack was refused because the target
        // dynel was not announced yet, or never sent): the client prints nothing for them either
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::Hit { .. })), 122);
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::Miss { .. })), 11);
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::SpecialAttack { .. })), 3);
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::Died { cause: 0, .. })), 15);
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::FightStarted { .. })), 127);
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::FightStopped { .. })), 118);
        // 122 hits + 3 specials, all on other chars: one world-space number and one line each; misses between bystanders: none
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::Floating { number, .. } if number.space == Space::World)), 125);
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::Floating { number, .. } if number.space == Space::Hud)), 0);
        assert_eq!(lines(&ev).len(), 125);
        // no combat music for the observer
        assert_eq!(count(&ev, |e| matches!(e, CombatEvent::CombatMusic(_))), 0);
        // damage numbers are the AttackInfo damages (live range 8..99)
        for e in &ev {
            if let CombatEvent::Floating { amount, .. } = e {
                assert!((1..=99).contains(amount), "{amount}");
            }
        }
    }

    /// The same capture seen as player 33402 (`Bergdoktor`, whose special attacks and hits are in the stream).
    #[test]
    fn replay_as_the_fighting_player() {
        let mut c = Combat::new(Box::new(Fixed::new()));
        let ev: Vec<_> = frames().iter().flat_map(|f| c.on_frame(f, 33402)).collect();
        assert!(count(&ev, |e| matches!(e, CombatEvent::CombatMusic(true))) >= 1);
        assert!(count(&ev, |e| matches!(e, CombatEvent::Floating { number, .. } if number.space == Space::Hud)) >= 1);
        let l = lines(&ev);
        assert!(l.iter().any(|t| t.starts_with("You hit ") && t.contains("points of Brawling damage.")), "{l:?}");
        assert!(l.iter().any(|t| t.starts_with("Attacking ")), "{l:?}");
    }

    /// Strings straight from `text.mdb` (skipped without the client).
    #[test]
    fn replay_with_the_real_texts() {
        let Some(db) = real_texts() else { return };
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("Games/ProjectRubiKa/client");
        let mut c = Combat::new(Box::new(db));
        c.arms = Armory::open(&dir);
        let ev: Vec<_> = frames().iter().flat_map(|f| c.on_frame(f, OWN)).collect();
        let l = lines(&ev);
        // the first AttackInfo of the capture (slot 2 of a guard) arrives before the guard's `SpecialAttackWeapon` list: the original finds no
        // slot object and drops the hit, the port prints it with the item default (melee, see `Combat::hit`)
        assert_eq!(l[0], "Scout - Jaax'Sinuh hit ICC Shuttle Guard for 17 points of melee damage.");
        // the damage types come from the items behind the slots (rdb stat 436): the guards' rifle attacks are projectile, their innate melee melee
        let kind = |w: &str| l.iter().filter(|t| t.contains(&format!(" points of {w} damage."))).count();
        assert!(kind("projectile") > 0 && kind("melee") > 0, "{l:?}");
        assert!(l.iter().all(|t| !t.contains('%')), "every conversion of the captured hits is fed: {:?}", l.iter().find(|t| t.contains('%')));
        // AttackInfo unk_30 == 4 occurs 3x on the wire, one of them in a dropped message (header without target)
        let crits = ev.iter().filter(|e| matches!(e, CombatEvent::Hit { flags: 4, .. })).count();
        assert_eq!(crits, 2);
        assert_eq!(l.iter().filter(|t| t.ends_with("Critical hit!")).count(), crits);
        let special: Vec<_> = l.iter().filter(|t| t.contains("Brawling")).collect();
        assert_eq!(special.len(), 3, "{special:?}");
        assert!(special.iter().all(|t| t.starts_with("Bergdoktor hit ") && t.contains(" points of Brawling damage.")), "{special:?}");
    }

    #[test]
    fn attack_info_struct_fields_are_unused_in_unit_tests() {
        // keep the wire types in the test imports honest
        let _ = (
            Attack { target: ch(1), flag: 0 },
            AttackInfo { damage: 1, value_20: -1, slot: 0, other: ch(1), unk_2c: 0, unk_30: 3, unk_34: 0 },
            MissedAttackInfo { value_1c: -1, slot: 0, source: ch(1), target: ch(2), stat: 0 },
            SpecialAttackInfo { slot: 0, damage: 1, value_28: -1, target: ch(1), special: 142, unk_30: 0 },
        );
    }
}
