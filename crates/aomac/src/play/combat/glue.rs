//! `impl Play` hooks of the combat layer (`module.rs`): player commands in, frames out, floating numbers on the draw list.

use super::log::Space;
use super::module::{Command, Module};
use crate::play::controls::Cmd;
use crate::play::movement::SitToggle;
use crate::play::Play;
use ao_gui::{DrawList, FontId};
use ao_net::msg::Identity;
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
        if p.sit() == SitToggle::StandRequest {
            let none = Identity::default();
            let a = action::simple(action::id::STAND_UP, none, none);
            m.push(action::character_action(self.zone.char_id as i32, &a));
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

    /// `N3Msg_SwitchTarget(id)` (CTRL / ALT + left click on a character).
    pub(in crate::play) fn fight_switch(&mut self, id: i32) {
        let mode = self.player.as_ref().map_or(0, |p| p.mode());
        if let Some(m) = self.fight.as_mut() {
            m.command(Command::SwitchTarget(id), &self.zone, mode);
        }
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
        let Some(m) = self.fight.as_mut() else { return };
        m.update(dt, &self.zone, self.audio.as_ref());
        for id in m.take_swings() {
            self.zone.world.attack(id);
        }
        // the HUD / sound / pose consumers take their events elsewhere; drop what nobody reads so the queues stay bounded
        m.take_events();
        m.take_pose_events();
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
