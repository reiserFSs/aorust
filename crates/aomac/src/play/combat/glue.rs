//! `impl Play` hooks of the combat layer (`module.rs`): player commands in, frames out, floating numbers on the draw list.

use super::actions::Event as ActionEvent;
use super::actions::Pose;
use super::anim::{anim_name, draw_clip, holster_clip, list, npc_sound, special_swing, UNARMED_RSWING};
use super::log::{Space, HUD_Y_FROM_REF};
use super::state::CombatEvent;
use super::module::{Command, Module};
use super::notes::hit_of;
use crate::play::chat::GameAction;
use crate::play::dynels::{Dynels, NAME_TAG_RADIUS};
use crate::play::player::Player;
use crate::play::controls::Cmd;
use crate::play::Play;
use ao_formats::character::Role;
use ao_gui::{DrawList, FontId};
use ao_net::n3::action;
use ao_render::Host;

impl Play {
    /// (Re)creates the combat layer for the current zone connection.
    pub(in crate::play) fn fight_reset(&mut self) {
        self.fight = Some(Module::new(&self.dir, self.zone.char_id));
    }

    /// `N3Msg_SitToggle` [GC 0x10028e0a]: stop the attack, then the movement layer sits (a `CharDCMove` 0x1e) or asks to stand up
    /// (`CharacterActionIIR_t` 0x57).
    fn fight_sit(&mut self) {
        let (Some(m), Some(p)) = (self.fight.as_mut(), self.player.as_mut()) else { return };
        m.before_sit();
        for o in p.sit() {
            if let action::Outgoing::Action(a) = o {
                m.push(action::character_action(self.zone.char_id as i32, &a));
            }
        }
    }

    /// A hotbar / action-menu special action (`FUN_1004256c`); true if the combat layer handled `id`.
    pub(in crate::play) fn fight_special(&mut self, id: u32) -> bool {
        if matches!(id, 0x4c | 0x4d) {
            self.fight_sit();
            return true;
        }
        let mode = self.player.as_ref().map_or(0, |p| p.mode());
        self.fight.as_mut().is_some_and(|m| m.special_action(id as i32, &self.zone, mode))
    }

    /// Per frame while in the world: the player's combat keys, the layer's timers / music, outgoing frames, chat feedback.
    pub(in crate::play) fn fight_frame(&mut self, dt: f32) {
        let mode = self.player.as_ref().map_or(0, |p| p.mode());
        let cmds = self.player.as_mut().map(|p| p.take_game()).unwrap_or_default();
        for c in cmds {
            match c {
                Cmd::Sit => self.fight_sit(),
                Cmd::Attack => self.fight_cmd(Command::Attack, mode),
                Cmd::SwitchTarget => {
                    if let Some(t) = self.zone.target {
                        self.fight_cmd(Command::SwitchTarget(t), mode);
                    }
                }
                Cmd::Special(s) => self.fight_cmd(Command::Special(s), mode),
                _ => {}
            }
        }
        let acts = self.chat.as_mut().map(|c| c.take_game()).unwrap_or_default();
        for a in acts {
            match a {
                GameAction::Social(id) => {
                    if let Some(m) = self.fight.as_mut() {
                        m.social(id as i32, mode);
                    }
                }
                GameAction::Camp => self.camp(),
                GameAction::SelectSelf => self.zone.set_target(Some(ao_net::msg::Identity { kind: 50000, instance: self.zone.char_id as i32 })),
                GameAction::BankClose => {
                    if let Some(interact) = self.interact.as_mut() {
                        interact.bank_close(&mut self.gui);
                    }
                }
                GameAction::Assist => match self.fight.as_ref().map(|m| m.assist(&self.zone)) {
                    Some(Ok(t)) => self.zone.set_target(Some(ao_net::msg::Identity { kind: 50000, instance: t })),
                    Some(Err(key)) if !key.is_empty() => {
                        if let Some(c) = self.chat.as_mut() {
                            c.feedback(&mut self.gui, key, &self.text);
                        }
                    }
                    _ => {}
                },
                // `/duel`, `/petduel` and the answers of the challenge dialogs (docs/zone/combat-duel.md)
                GameAction::Duel { pet, op } => {
                    if let Some(Err(text)) = self.fight.as_mut().map(|m| m.duel_command(&self.zone, pet, op)) {
                        if let Some(c) = self.chat.as_mut() {
                            c.cmd_error(&mut self.gui, text);
                        }
                    }
                }
                GameAction::StartPvp(target) => {
                    if let Some(m) = self.fight.as_mut() {
                        m.start_pvp(target);
                    }
                }
            }
        }
        self.fight_duel();
        let Some(m) = self.fight.as_mut() else { return };
        m.update(dt, &self.zone, self.audio.as_ref());
        let own = self.zone.char_id as i32;
        // Every health consumer (target, team, nametag and NPC info) reads the zone stores.
        let events = m.take_events();
        for e in &events {
            sync_stats(&mut self.zone, e);
        }
        if std::env::var_os("AOMAC_COMBAT_LOG").is_some() {
            for e in &events {
                match e {
                    CombatEvent::Hit { .. } | CombatEvent::Miss { .. } | CombatEvent::Died { .. } | CombatEvent::FightStarted { .. } | CombatEvent::FightStopped { .. } | CombatEvent::CombatMusic(_) | CombatEvent::DeathMusic(_) => eprintln!("combat: {e:?}"),
                    CombatEvent::Health { dynel, .. } if *dynel == own => eprintln!("combat: {e:?}"),
                    _ => {}
                }
            }
        }
        // what the attack notes of a swing read from the attacker's slot object (`FUN_1006a8f3`); a miss runs the same routine with damage 0 and
        // hit kind 1 (`FUN_1006ae50`: `FUN_1006a8f3(slot, 0, value_1c, 0, 1, 0)`)
        for (attacker, ctx) in events.iter().filter_map(hit_of) {
            self.zone.world.hit_seen(attacker, ctx);
        }
        // swings (`FUN_1006a239` [GC 0x1006a239], docs/zone/combat-anim.md §3): every hit, miss and special attack of every character
        for e in &events {
            match e {
                CombatEvent::Hit { attacker, .. } | CombatEvent::Miss { attacker, .. } => swing(&mut self.zone.world, self.player.as_mut(), *attacker, list::ATTACK, false, own),
                CombatEvent::SpecialAttack { who, special, .. } => {
                    let s = special_swing(*special);
                    swing(&mut self.zone.world, self.player.as_mut(), *who, s.map_or(list::ATTACK, |s| s.list), s.is_some_and(|s| s.own_item), own)
                }
                _ => {}
            }
        }
        // the struck character's reaction clip (`FUN_1009b4ac`, docs/zone/combat-anim.md §4): hit kind > 1, not while swimming (mode 4 / 8)
        for e in &events {
            let CombatEvent::Hit { victim, flags, .. } = e else { continue };
            if *flags <= 1 {
                continue;
            }
            if *victim != own {
                self.zone.world.react_to_hit(*victim, *flags);
            } else if let Some(p) = self.player.as_mut().filter(|p| !matches!(p.mode(), 4 | 8)) {
                let anim = self.zone.world.impact_anim();
                if let Some((name, _)) = anim_name(anim) {
                    p.react(Role::Clip(name.into()), if *flags == 4 { 1.0 } else { 0.5 });
                }
            }
        }
        // fight sounds (docs/zone/combat-anim.md §6): played by the app next to the door sounds (`Dynels::take_sounds`, listener = camera).
        // The weapon / swish / impact sounds belong to the animation notes of the swing clips (`FUN_1003c036` -> `FUN_10045069`, `notes`)
        let own_notes = self.player.as_mut().map(|p| p.take_notes()).unwrap_or_default();
        let notes: Vec<(i32, u32)> = own_notes.into_iter().map(|n| (own, n)).chain(self.zone.world.take_notes()).collect();
        for &(who, n) in &notes {
            if std::env::var_os("AOMAC_COMBAT_LOG").is_some() {
                eprintln!("combat: note {n:#x} of {who}");
            }
            self.zone.world.note_sounds(who, n);
        }
        for id in m.take_struck() {
            self.zone.world.char_sound(id, npc_sound::HIT);
        }
        for e in &events {
            match e {
                // `CharDie_t` is the state of the server's action 99 (cause 0); a death the client computed itself does not start it
                CombatEvent::Died { dynel, cause: 0 } => self.zone.world.char_sound(*dynel, npc_sound::DEATH),
                CombatEvent::SpecialAttack { who, special, .. } => {
                    if let Some(name) = special_swing(*special).and_then(|s| s.sound) {
                        self.zone.world.sound_at(*who, name);
                    }
                }
                _ => {}
            }
        }
        // AnimHolder stance clips (docs/zone/combat-anim.md §4): the fight idle, the draw and the holster follow the fight controller of every character
        stance(&mut self.zone.world, self.player.as_mut(), own, &events, |id| m.is_fighting(id));
        if let Some(p) = self.player.as_mut() {
            for e in events {
                match e {
                    CombatEvent::Died { dynel, .. } if dynel == own => {
                        let name = anim_name(m.death_anim()).map_or("die-knees", |a| a.0);
                        p.play(Role::Clip(name.into()), true);
                    }
                    CombatEvent::Health { dynel, health, .. } if dynel == own && health > 0 && p.holding() => p.stand(),
                    _ => {}
                }
            }
        }
        for e in m.take_pose_events() {
            match e {
                ActionEvent::Emote { dynel, id, clip, .. } => match (dynel == own, self.player.as_mut()) {
                    (true, Some(p)) => p.play(Role::Emote(clip.trim_start_matches("social-").to_string()), false),
                    (true, None) => {}
                    _ => self.zone.world.play_once(dynel, id as u32),
                },
                // the own avatar's enter / stop clips follow its movement role (`Player::update`); the others' pose changes come from the zone stream
                ActionEvent::Pose { dynel, from, to } if dynel != own => {
                    if let Some(id) = Pose::transition_anim(from, to) {
                        self.zone.world.play_once(dynel, id as u32);
                    }
                }
                ActionEvent::Pose { .. } => {}
            }
        }
        // `CharacterAction` 0x64: the server asks a character's animation holder to play an animation id (`FUN_1003c47c`); the unwield (0x61) queues its gesture 0x6d here too
        for (dynel, id) in m.take_anims() {
            match (dynel == own, self.player.as_mut()) {
                (true, Some(p)) => {
                    if let Some((name, _)) = anim_name(id) {
                        p.play(Role::Clip(name.into()), false);
                    }
                }
                (true, None) => {}
                _ => self.zone.world.play_once(dynel, u32::from(id)),
            }
        }
        for key in m.take_feedback() {
            if let Some(c) = self.chat.as_mut() {
                c.feedback(&mut self.gui, key, &self.text);
            }
        }
        let out = m.take_outbox();
        if let Some(s) = &self.session {
            for f in out {
                s.send_zone(f);
            }
        }
    }

    fn fight_cmd(&mut self, c: Command, mode: u32) {
        if let Some(m) = self.fight.as_mut() {
            m.command(c, &self.zone, mode);
        }
    }

    /// Received duel / pet-duel messages (`FUN_1005b821` / `FUN_1005c514`): System-window lines, the challenge dialogs, the `AutoRejectDuel` answer.
    fn fight_duel(&mut self) {
        use super::duel::{Event, Line};
        let Some(events) = self.fight.as_mut().map(|m| m.take_duel()) else { return };
        let name = |z: &crate::play::zone::Zone, who: i32| z.dynels.get(&who).map(|d| d.name.clone());
        for e in events {
            let Some(c) = self.chat.as_mut() else { return };
            match e {
                Event::Line(Line::Plain(key)) => c.feedback(&mut self.gui, key, &self.text),
                Event::Line(Line::Named { key, who, otherwise }) => match (name(&self.zone, who), otherwise) {
                    (Some(n), _) => c.feedback_named(&mut self.gui, key, &n, &self.text),
                    (None, Some(k)) => c.feedback(&mut self.gui, k, &self.text),
                    (None, None) => {}
                },
                // the challenger has to be a known character (`FUN_10058e36`); `AutoRejectDuel` answers before anything is shown
                Event::Challenged(who) => {
                    let Some(n) = name(&self.zone, who) else { continue };
                    if self.hud.as_ref().is_some_and(|h| h.dvalues.flag("AutoRejectDuel")) {
                        if let Some(m) = self.fight.as_mut() {
                            m.duel_auto_refuse();
                        }
                    } else {
                        c.feedback_named(&mut self.gui, "Feedback_DuelChallenge", &n, &self.text);
                        c.duel_dialog(&mut self.gui, false, &n, &self.text);
                    }
                }
                Event::ChallengeSent(who) => {
                    if let Some(n) = name(&self.zone, who) {
                        c.duel_dialog(&mut self.gui, true, &n, &self.text);
                    }
                }
                Event::Close => c.close_duel_dialog(&mut self.gui),
                Event::PvpPrompt { team, target } => c.pvp_dialog(&mut self.gui, team, target, &self.text),
            }
        }
    }

    /// Floating damage numbers (`DamageTextMessage` for the own character, the effect `0x2f5a` billboard for everybody else).
    pub(in crate::play) fn fight_draw(&mut self, host: &Host, list: &mut DrawList) {
        let Some(m) = self.fight.as_ref() else { return };
        let w = self.size.0 as f32;
        for n in m.numbers() {
            let rgb = n.spec.color & 0x00ff_ffff;
            let (x, y) = match n.spec.space {
                // `DamageTextMessage` [GUI 0x1004ae2f]: centre (50 + r, DAT_102761c0 - 20), text top = centre - font height / 2. `DAT_102761c0` is
                // a .bss int with a single reader (code scan of GUI.dll: no writer, and its neighbour is the one-only text pointer), i.e.
                // 0: the number starts above the screen and rises (y0 - 70 * t / 2.3), so the original never shows it on screen.
                Space::Hud => (n.jitter, HUD_Y_FROM_REF as f32 - self.gui.font_height(FontId::Shell) as f32 / 2.0 - n.rise()),
                // A billboard effect of the 3D scene: only what the camera sees (on screen, within the locality radius of the tags,
                // not behind terrain / walls) is drawn. [GUESS] the radius: the effect's own range was not traced.
                Space::World => match self.zone.world.head_point(n.dynel, &host.camera, self.size, n.rise()) {
                    Some((x, y, at))
                        if (0.0..w).contains(&x)
                            && (0.0..self.size.1 as f32).contains(&y)
                            && (at - host.camera.pos).length() <= NAME_TAG_RADIUS
                            && self.player.as_ref().is_none_or(|p| p.line_clear(host.camera.pos.to_array(), at.to_array())) =>
                    {
                        (x, y)
                    }
                    _ => continue,
                },
            };
            let tw = self.gui.text_width(FontId::Shell, &n.text);
            // neither path fades: the HUD text has flag 1 (move only), the effect keeps its constant colour (combat-log.md §6)
            self.gui.text_cmds(FontId::Shell, &n.text, x as i32 - tw / 2, y as i32, rgb, 1.0, list);
        }
    }
}

/// One swing of `who` (`FUN_1006a239` [GC 0x1006a239]): the weapon's list `key` ([`Dynels::pick_swing`]) played as that character's clip, sped up
/// for the weapon's `ItemDelay`. `own_item` specials (Brawl, Dimach, Backstab, bow special) look the key up on the special's own item
/// (`FUN_100686d0(stat) + 0xe4`), whose record layout is not decoded: they play the bare-handed swing like a character without a weapon.
/// **[UNRESOLVED]** that swing for bare hands is the creature path `0x40a` `unarmed-rswing` (the player's unarmed-template item list is not
/// decoded, combat-anim.md §3.1); creatures (no weapon attractors) always use it.
fn swing(world: &mut Dynels, player: Option<&mut Player>, who: i32, key: u16, own_item: bool, own: i32) {
    let picked = if own_item { None } else { world.pick_swing(who, key) };
    // the clip's notes (`attack`, `swish_*`) start the sounds: the own avatar and the dynels watch them while the clip plays
    if who != own {
        world.swing_mark(who);
    }
    match (who == own, picked) {
        (true, Some((anim, delay))) => {
            if let (Some(p), Some((name, _))) = (player, anim_name(anim)) {
                p.swing(Role::Clip(name.into()), Some(delay));
            }
        }
        (true, None) => {
            if let (Some(p), Some((name, _))) = (player, anim_name(UNARMED_RSWING)) {
                p.swing(Role::Clip(name.into()), None);
            }
        }
        (false, Some((anim, _))) => world.play_once(who, anim as u32),
        (false, None) => world.attack(who),
    }
}

/// Plays AbstractAnimID `id` once on `who`: the own avatar through [`Player::play`], every other character through [`Dynels::play_once`].
fn play_anim(world: &mut Dynels, player: &mut Option<&mut Player>, own: i32, who: i32, id: u16) {
    if who != own {
        world.play_once(who, u32::from(id));
    } else if let (Some(p), Some((name, _))) = (player.as_deref_mut(), anim_name(id)) {
        p.play(Role::Clip(name.into()), false);
    }
}

/// The AnimHolder's fight idle of `who` goes on / off ([`Dynels::set_fighting`], the own [`Player::fighting`]) and plays its one-shot clip:
/// on = idle update `FUN_1003cad0` [GC 0x1003cad0] (the weapon's list 0x1a, then the weapon idle), off = `FUN_1003cc15` (list 0x1b, then the idle of
/// the equip routine); `play` = false skips the clip (a dying character goes straight to its death clip).
fn fight_idle(world: &mut Dynels, player: &mut Option<&mut Player>, own: i32, who: i32, on: bool, play: bool) {
    world.set_fighting(who, on);
    if who == own {
        if let Some(p) = player.as_deref_mut() {
            p.fighting = on;
        }
    }
    let set = world.wielded_set(who);
    if let Some(id) = if on { draw_clip(set) } else { holster_clip(set) }.filter(|_| play) {
        play_anim(world, player, own, who, id);
    }
}

/// What the fight controller does to the AnimHolder (docs/zone/combat-anim.md §4): `CharFight_t` ctor `FUN_1007b816` runs the idle update when a
/// character starts fighting (not when it only switches target: `FUN_10069c68` stops the old fight first, `FUN_10068b7f` -> `FUN_1003cc15` plays the
/// holster, and the fight state stays 2 without a new draw), `FUN_10068b7f` the holster when a fight stops, `FUN_1006a700` the idle update when a weapon
/// is wielded while the holder fights (`fighting`).
fn stance(world: &mut Dynels, mut player: Option<&mut Player>, own: i32, events: &[CombatEvent], fighting: impl Fn(i32) -> bool) {
    let died = |who: i32| events.iter().any(|e| matches!(e, CombatEvent::Died { dynel, .. } if *dynel == who));
    for e in events {
        match e {
            CombatEvent::FightStarted { who, switched: false, .. } => fight_idle(world, &mut player, own, *who, true, true),
            CombatEvent::FightStopped { who } => fight_idle(world, &mut player, own, *who, false, !died(*who)),
            _ => {}
        }
    }
    for who in world.take_wielded() {
        if fighting(who) {
            fight_idle(world, &mut player, own, who, true, true);
        }
    }
}

fn sync_stats(zone: &mut crate::play::zone::Zone, event: &CombatEvent) {
    if let CombatEvent::StatChanged { dynel, stat, value } = event {
        zone.character_stats.entry(*dynel).or_default().insert(*stat, *value);
        if *dynel == zone.char_id as i32 {
            zone.stats.insert(*stat, *value);
        }
        if *stat == ao_formats::stats::LEVEL {
            if let Some(d) = zone.dynels.get_mut(dynel) {
                d.level = *value;
            }
        }
        return;
    }
    let CombatEvent::Health { dynel, health, max_health, .. } = event else { return };
    let stats = zone.character_stats.entry(*dynel).or_default();
    stats.insert(ao_formats::stats::HEALTH, *health);
    if *dynel == zone.char_id as i32 {
        // Own Life is computed by the pool layer, not the FullCharacter wire value.
        zone.stats.insert(ao_formats::stats::HEALTH, *health);
    } else {
        stats.insert(ao_formats::stats::pools::LIFE, *max_health);
    }
    if let Some(d) = zone.dynels.get_mut(dynel) {
        d.health = *health;
        if *dynel != zone.char_id as i32 {
            d.max_health = *max_health;
        }
    }
}

#[cfg(test)]
mod health_tests {
    use super::*;
    use crate::play::combat::state::{Combat, CHAR_KIND};
    use crate::play::combat::log::fake::Fixed;

    #[test]
    fn npc_hit_and_authoritative_health_reach_all_zone_views() {
        let mut combat = Combat::new(Box::new(Fixed::new()));
        let mut zone = crate::play::zone::Zone::default();
        zone.char_id = 1;
        combat.add_test_char(1, "Player", false, 100);
        combat.add_test_char(2, "NPC", true, 50);
        zone.dynels.insert(2, crate::play::zone::DynelState {
            name: "NPC".into(), pos: [0.0; 3], yaw: None, npc: true,
            side: 0, level: 1, health: 50, max_health: 50,
        });
        // Compact N3 packets using the retail Attack, AttackInfo and HealthDamage layouts.
        let packet = |kind: u32, id: i32, words: &[i32]| {
            let mut payload = kind.to_be_bytes().to_vec();
            payload.extend_from_slice(&CHAR_KIND.to_be_bytes());
            payload.extend_from_slice(&id.to_be_bytes());
            payload.push(0);
            for word in words { payload.extend_from_slice(&word.to_be_bytes()); }
            ao_net::frame::Frame { seq: 1, ptype: ao_net::frame::PT_N3, sender: 1, receiver: 1, payload }
        };
        let apply = |combat: &mut Combat, zone: &mut crate::play::zone::Zone, frame| {
            let events = combat.on_frame(&frame, 1);
            for event in &events { sync_stats(zone, event); }
            events
        };
        let ch = |instance| ao_net::msg::Identity { kind: CHAR_KIND, instance };
        let frame = |body: ao_net::n3::misc::Misc| ao_net::frame::Frame {
            seq: 1, ptype: ao_net::frame::PT_N3, sender: 1, receiver: 1,
            payload: body.encode(ch(1), 0),
        };
        apply(&mut combat, &mut zone, frame(ao_net::n3::misc::Misc::Attack(ao_net::n3::misc::Attack { target: ch(2), flag: 0 })));
        let hit = apply(&mut combat, &mut zone, frame(ao_net::n3::misc::Misc::AttackInfo(ao_net::n3::misc::AttackInfo {
            damage: 10, value_20: -1, slot: 0, other: ch(2), unk_2c: 0, unk_30: 3, unk_34: 0,
        })));
        assert!(hit.iter().any(|e| matches!(e, CombatEvent::Hit { victim: 2, .. })));
        assert_eq!(zone.dynels[&2].health, 40);
        assert_eq!(zone.stat_of(2, ao_formats::stats::HEALTH), Some(40));
        let update = apply(&mut combat, &mut zone, packet(0x3710_256C, 2, &[37, -3, 95, 0, CHAR_KIND, 1, 0]));
        assert_eq!(combat.char(2).unwrap().health(), 37);
        assert_eq!(zone.dynels[&2].health, 37);
        assert_eq!(zone.stat_of(2, ao_formats::stats::HEALTH), Some(37));
        assert!(matches!(update.as_slice(), [CombatEvent::Health { health: 37, delta: -3, .. }]));
        // A repeated authoritative value must not subtract its feedback delta a second time.
        apply(&mut combat, &mut zone, packet(0x3710_256C, 2, &[37, -3, 95, 0, CHAR_KIND, 1, 0]));
        assert_eq!(zone.dynels[&2].health, 37);
        apply(&mut combat, &mut zone, packet(0x3710_256C, 1, &[91, -9, 95, 0, CHAR_KIND, 2, 0]));
        assert_eq!(zone.stats[&ao_formats::stats::HEALTH], 91);
        assert_eq!(zone.character_stats[&1][&ao_formats::stats::HEALTH], 91);
        apply(&mut combat, &mut zone, packet(0x7F40_5A16, 2, &[12, 100, 200, 150, 300, 2, 8, 50]));
        assert_eq!(zone.dynels[&2].level, 12);
        assert_eq!(zone.stat_of(2, 0x113), Some(8));
        assert_eq!(combat.char(2).unwrap().stat(0x113), 8);
        assert_eq!(zone.stat_of(2, 0x25), None); // TitleLevel is own-char gated.
        apply(&mut combat, &mut zone, packet(0x7F40_5A16, 1, &[13, 101, 201, 151, 301, 2, 9, 51]));
        assert_eq!(zone.stat_of(1, 0x36), Some(13));
        assert_eq!(zone.stat_of(1, 0x113), Some(9));
        assert_eq!(zone.stat_of(1, 0x25), Some(2));
        // HealthDamage sets the absolute value before calling the death routine.
        let death = apply(&mut combat, &mut zone, packet(0x3710_256C, 2, &[5, -32, 95, 4, CHAR_KIND, 1, 0]));
        assert!(matches!(death.first(), Some(CombatEvent::Health { health: 5, .. })));
        assert_eq!(zone.dynels[&2].health, 0);
        assert!(death.iter().any(|e| matches!(e, CombatEvent::Died { dynel: 2, cause: 4 })));
    }
}
