//! `impl Play` hooks of the combat layer (`module.rs`): player commands in, frames out, floating numbers on the draw list.

use super::actions::Event as ActionEvent;
use super::anim::anim_name;
use super::log::Space;
use super::state::CombatEvent;
use super::module::{Command, Module};
use crate::play::chat::GameAction;
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
                GameAction::Assist => match self.fight.as_ref().map(|m| m.assist(&self.zone)) {
                    Some(Ok(t)) => self.zone.target = Some(t),
                    Some(Err(key)) if !key.is_empty() => {
                        if let Some(c) = self.chat.as_mut() {
                            c.feedback(&mut self.gui, key, &self.text);
                        }
                    }
                    _ => {}
                },
            }
        }
        let Some(m) = self.fight.as_mut() else { return };
        m.update(dt, &self.zone, self.audio.as_ref());
        for id in m.take_swings() {
            self.zone.world.attack(id);
        }
        let own = self.zone.char_id as i32;
        if let Some(p) = self.player.as_mut() {
            p.fighting = m.attacking();
            for e in m.take_events() {
                match e {
                    // `FUN_1006a8f3` swing of the own character: [GUESS] the unarmed swing (weapon swing lists: combat-anim.md §3)
                    CombatEvent::Hit { attacker, .. } | CombatEvent::SpecialAttack { who: attacker, .. } if attacker == own => {
                        p.play(Role::Clip("unarmed-rswing".into()), false)
                    }
                    CombatEvent::Died { dynel, .. } if dynel == own => {
                        let name = anim_name(m.death_anim()).map_or("die-knees", |a| a.0);
                        p.play(Role::Clip(name.into()), true);
                    }
                    CombatEvent::Health { dynel, health, .. } if dynel == own && health > 0 && p.holding() => p.stand(),
                    _ => {}
                }
            }
        } else {
            m.take_events();
        }
        for e in m.take_pose_events() {
            if let ActionEvent::Emote { dynel, id, clip, .. } = e {
                match (dynel == own, self.player.as_mut()) {
                    (true, Some(p)) => p.play(Role::Emote(clip.trim_start_matches("social-").to_string()), false),
                    (true, None) => {}
                    _ => self.zone.world.play_once(dynel, id as u32),
                }
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

    /// Floating damage numbers (`DamageTextMessage` for the own character, the effect `0x2f5a` billboard for everybody else).
    pub(in crate::play) fn fight_draw(&mut self, host: &Host, list: &mut DrawList) {
        let Some(m) = self.fight.as_ref() else { return };
        let h = self.size.1 as f32;
        for n in m.numbers() {
            let rgb = n.spec.color & 0x00ff_ffff;
            let (x, y) = match n.spec.space {
                // [UNRESOLVED] `DAT_102761c0` (the HUD number's reference y) is not identified: the lower third of the screen.
                Space::Hud => (n.jitter, h * 0.66 - 20.0 - n.rise()),
                Space::World => match self.zone.world.head_point(n.dynel, &host.camera, self.size, n.rise()) {
                    Some(p) => p,
                    None => continue,
                },
            };
            let tw = self.gui.text_width(FontId::Shell, &n.text);
            // neither path fades: the HUD text has flag 1 (move only), the effect keeps its constant colour (combat-log.md §6)
            self.gui.text_cmds(FontId::Shell, &n.text, x as i32 - tw / 2, y as i32, rgb, 1.0, list);
        }
    }
}
