//! Consumers of the HUD's use queue (`Hud::take_uses`): hotbar slots (`FUN_100d79c9`), the Attack button of the hostile target dock.

use super::hud_bar::SlotUse;
use crate::play::Play;
use ao_net::n3::action::{self, CampInput, Outgoing};

/// Logout countdown of the camp timer bar (`LDBformat::Feed(0x1e)` in `CampStartedMessage`).
const CAMP_SECONDS: f32 = 30.0;

impl Play {
    /// Runs the uses the HUD queued this frame.
    pub(in crate::play) fn hud_uses(&mut self) {
        // the toggles of the special-action list follow the own state (`FUN_1006d196`, fight start / stop, camping; hud_special.rs)
        if let (Some(h), Some(p)) = (self.hud.as_mut(), self.player.as_ref()) {
            let fighting = self.fight.as_ref().is_some_and(|m| m.attacking());
            h.actions.set_state(super::hud_special::OwnState { fighting, mode: p.mode() as u8, last_speed_walk: p.last_speed_walk(), camping: self.camp.is_some() });
        }
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
            0x51 => {
                self.start_camping();
            }
            0x52 => {
                self.cancel_camp();
                self.send_outgoing(vec![action::stop_camping()]);
            }
            _ => eprintln!("hud: special action {a:#x} has no consumer yet"),
        }
    }

    /// `/camp` / `StartQuitToLoginMessage` [GUI 0x10027c74]: unless already camping to the login (state 2), `N3Msg_StartCamping` (from state 0;
    /// failure leaves the state), then state 2: the countdown ends back at the login.
    pub(in crate::play) fn camp(&mut self) {
        use super::logout::State;
        if self.logout.state == State::Login || (self.logout.state == State::None && !self.start_camping()) {
            return;
        }
        self.logout.state = State::Login;
    }

    /// `/quit` / `StartQuitToSystemMessage` [GUI 0x10029a0d]: a second `/quit` within 3 s, or one during a timed logout to the system, quits
    /// at once (`QuitGameToSystemMessage`); otherwise text `ClosingClient` (category 200, code 12) and camping, whose countdown then ends in the
    /// quit (state 1). The two GlobalSignals emissions (+0x294, +0x20c) have no listener in the port [UNRESOLVED: listeners not traced].
    pub(in crate::play) fn quit_cmd(&mut self, host: &mut ao_render::Host) {
        use super::logout::State;
        if self.logout.quit_now(super::logout::now_ms()) {
            host.quit = true;
            return;
        }
        if let Some(c) = self.chat.as_mut() {
            c.logout_line(&mut self.gui, "ClosingClient", None, &self.text);
        }
        if self.logout.state == State::None && !self.start_camping() {
            return;
        }
        self.logout.state = State::System;
    }

    /// `CancelCampMessage` [GUI 0x10029c26]: the timer bar is deleted, text `TimedLogoutAborted` (code 12), state 0, quit time 0.
    fn cancel_camp(&mut self) {
        self.camp = None;
        self.logout.cancel();
        if let Some(c) = self.chat.as_mut() {
            c.logout_line(&mut self.gui, "TimedLogoutAborted", None, &self.text);
        }
    }

    /// The camp countdown (`FlowControlModule_t::m_pcCampTimer`, 30 s): its end is the server dropping the connection, and
    /// `ServerLostMessage` [GUI 0x10028d50] runs `ActivateGameClosing(state)`: state 1 quits the game, state 2 returns to the login (config
    /// saved, screen cleared, then login [GUI 0x10028194]). [INFERENCE] the 30 s expiry itself is the server's: `StartLogoutIIR_t` /
    /// `StopLogoutIIR_t` have no client apply [GC 0x10079c53 / 0x10079e6c]; the timer bar widget is not drawn.
    pub(in crate::play) fn camp_frame(&mut self, dt: f32, host: &mut ao_render::Host) {
        let Some(t) = self.camp.as_mut() else { return };
        *t += dt;
        if *t >= CAMP_SECONDS {
            self.camp = None;
            match self.logout.expired() {
                Some(super::logout::Closing::System) => host.quit = true,
                _ => self.show_login(host),
            }
        }
    }

    /// `N3Msg_StartCamping` [GC 0x1001c93d] (docs/zone/actions.md §3): refusals are chat feedback, otherwise sit, stop attacking, action 0x78.
    /// `true` = camping started.
    fn start_camping(&mut self) -> bool {
        let (Some(p), Some(m)) = (self.player.as_ref(), self.fight.as_ref()) else { return false };
        let stat = |id: u32| self.zone.stats.get(&id).copied().unwrap_or(0);
        let c = CampInput {
            gm_level: stat(action::stat::GM_LEVEL as u32),
            is_fighting_me: stat(action::stat::IS_FIGHTING_ME as u32),
            fsm_mode: p.mode() as i32,
            can_sit: p.can_sit(),
            attacking: m.attacking(),
        };
        match action::start_camping(&c) {
            Ok(out) => {
                self.send_outgoing(out);
                // `CampStartedMessage` [GUI 0x10029d38]: timer 40000 "Logout", state 0 -> 1, text `LogoutStarted` fed 30 (code 12)
                self.camp = Some(0.0);
                self.logout.camp_started();
                if let Some(c) = self.chat.as_mut() {
                    c.logout_line(&mut self.gui, "LogoutStarted", Some(CAMP_SECONDS as i32), &self.text);
                }
                true
            }
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
                        p.sit_ground();
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
