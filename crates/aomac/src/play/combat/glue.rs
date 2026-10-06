//! `impl Play` hooks of the combat layer (`module.rs`): player commands in, frames out, floating numbers on the draw list.

use super::actions::Event as ActionEvent;
use super::actions::Pose;
use super::anim::{anim_name, list, npc_sound, special_swing, UNARMED_RSWING};
use super::log::{Space, HUD_Y_FROM_REF};
use super::state::CombatEvent;
use super::module::{Command, Module};
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
                GameAction::SelectSelf => self.zone.target = Some(self.zone.char_id as i32),
                GameAction::Assist => match self.fight.as_ref().map(|m| m.assist(&self.zone)) {
                    Some(Ok(t)) => self.zone.target = Some(t),
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
        // `AttackInfo` / `StatIIR` / death change the own `Health` (27) stat in place (docs/zone/combat-log.md §3): the interface reads that one store
        let events = m.take_events();
        for e in &events {
            if let CombatEvent::Health { dynel, health, .. } = e {
                if *dynel == own {
                    self.zone.stats.insert(ao_formats::stats::HEALTH, *health);
                }
            }
        }
        // swings (`FUN_1006a239` [GC 0x1006a239], docs/zone/combat-anim.md §3): every hit and special attack of every character
        for e in &events {
            match e {
                CombatEvent::Hit { attacker, .. } => swing(&mut self.zone.world, self.player.as_mut(), *attacker, list::ATTACK, false, own),
                CombatEvent::SpecialAttack { who, special, .. } => {
                    let s = special_swing(*special);
                    swing(&mut self.zone.world, self.player.as_mut(), *who, s.map_or(list::ATTACK, |s| s.list), s.is_some_and(|s| s.own_item), own)
                }
                _ => {}
            }
        }
        // fight sounds (docs/zone/combat-anim.md §6): played by the app next to the door sounds (`Dynels::take_sounds`, listener = camera)
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
        if let Some(p) = self.player.as_mut() {
            p.fighting = m.attacking();
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
        // `CharacterAction` 0x64: the server asks a character's animation holder to play an animation id (`FUN_1003c47c`)
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
    match (who == own, picked) {
        (true, Some((anim, delay))) => {
            if let (Some(p), Some((name, _))) = (player, anim_name(anim)) {
                p.swing(Role::Clip(name.into()), delay);
            }
        }
        (true, None) => {
            if let (Some(p), Some((name, _))) = (player, anim_name(UNARMED_RSWING)) {
                p.play(Role::Clip(name.into()), false);
            }
        }
        (false, Some((anim, _))) => world.play_once(who, anim as u32),
        (false, None) => world.attack(who),
    }
}
