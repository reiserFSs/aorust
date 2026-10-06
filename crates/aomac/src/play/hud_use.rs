//! Consumers of the HUD's use queue (`Hud::take_uses`): hotbar slots (`FUN_100d79c9`), the Attack button of the hostile target dock.

use super::hud_bar::SlotUse;
use crate::play::Play;
use ao_net::n3::action::{self, CampInput, Outgoing};

impl Play {
    /// Runs the uses the HUD queued this frame.
    pub(in crate::play) fn hud_uses(&mut self) {
        let uses = self.hud.as_mut().map(|h| h.take_uses()).unwrap_or_default();
        for u in uses {
            match u {
                // type 7: the macro text runs as if typed into the chat input
                SlotUse::Macro(line) => {
                    if let Some(c) = self.chat.as_mut() {
                        c.run_line(&mut self.gui, &line, &self.zone, &self.text);
                    }
                }
                // type 6: `N3Msg_PerformSpecialAction(Action_e)` -> `FUN_1004256c` (docs/zone/combat-net.md §5.3)
                SlotUse::SpecialAction(a) => self.special_action(a),
            }
        }
    }

    /// `FUN_1004256c` for the own character: combat (attack 0xb / 0x4e, specials) and sit (0x4c / 0x4d) belong to the combat layer;
    /// walk / run 0x11 / 0x12 and camping 0x51 / 0x52 are here.
    fn special_action(&mut self, a: u32) {
        if self.fight_special(a) {
            return;
        }
        match a {
            // `MovementChanged(0x18 / 0x19)`: one toggle (`SlotMovementWalkToggle` picks 0x11 or 0x12 from the current speed mode)
            0x11 | 0x12 => {
                if let Some(p) = self.player.as_mut() {
                    p.toggle_walk();
                }
            }
            0x51 => self.start_camping(),
            0x52 => self.send_outgoing(vec![action::stop_camping()]),
            _ => eprintln!("hud: special action {a:#x} has no consumer yet"),
        }
    }

    /// `N3Msg_StartCamping` [GC 0x1001c93d] (docs/zone/actions.md §3): refusals are chat feedback, otherwise sit, stop attacking, action 0x78.
    fn start_camping(&mut self) {
        let (Some(p), Some(m)) = (self.player.as_ref(), self.fight.as_ref()) else { return };
        let stat = |id: u32| self.zone.stats.get(&id).copied().unwrap_or(0);
        let c = CampInput {
            gm_level: stat(action::stat::GM_LEVEL as u32),
            is_fighting_me: stat(action::stat::IS_FIGHTING_ME as u32),
            fsm_mode: p.mode() as i32,
            can_sit: p.can_sit(),
            attacking: m.attacking(),
        };
        match action::start_camping(&c) {
            Ok(out) => self.send_outgoing(out),
            Err(r) => {
                if let Some(c) = self.chat.as_mut() {
                    c.feedback(&mut self.gui, r.key(), &self.text);
                }
            }
        }
    }

    fn send_outgoing(&mut self, out: Vec<Outgoing>) {
        for o in out {
            match o {
                Outgoing::Move(action::mv::SWITCH_TO_SIT_GROUND) => {
                    if let Some(p) = self.player.as_mut() {
                        p.sit();
                    }
                }
                Outgoing::Move(m) => eprintln!("hud: movement {m:#x} has no sender here"),
                Outgoing::StopAttack => {
                    if let Some(m) = self.fight.as_mut() {
                        m.before_sit();
                    }
                }
                Outgoing::Action(a) => {
                    if let Some(m) = self.fight.as_mut() {
                        m.push(action::character_action(self.zone.char_id as i32, &a));
                    }
                }
            }
        }
    }
}
