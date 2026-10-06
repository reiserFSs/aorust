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
                SlotUse::Unavailable => {
                    if let Some(c) = self.chat.as_mut() {
                        c.feedback(&mut self.gui, "Feedback_ActionIsNotAvailable", &self.text);
                    }
                }
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
            // Use / Pick-Up on the selected target (`FUN_1004256c`: identity = the targeting object's `+0x5c`, `Zone::target`)
            3 => self.use_target(),
            1 => self.pick_up(),
            // `N3Msg_TryEnterSneakMode` [GC 0x293c0]: refused with a vehicle (stat MechData 0x296), else `CharacterActionIIR_t` 0xa3. How the server's
            // answer puts the movement FSM into sneak mode is not traced here (the apply table ignores 0xa3: docs/zone/actions.md).
            0x13 => {
                if self.zone.stat(0x296).unwrap_or(0) != 0 {
                    self.feedback("Feedback_YouCantSneakInVehicle");
                } else {
                    self.send_outgoing(vec![action::plain_action(action::id::SNEAK)]);
                }
            }
            // `MovementChanged(0x24)`: leave sneak
            0x4f => {
                if let Some(p) = self.player.as_mut() {
                    p.leave_sneak();
                }
            }
            // `N3Msg_CrawlToggle` [GC 0x278c9]: entering crawl is refused while polymorphed (stat 0x167)
            0x14 | 0x8d => {
                let leaving = self.zone.stat(0x1ae) == Some(0xe);
                if !leaving && self.zone.stat(0x167).unwrap_or(0) != 0 {
                    self.feedback("Feedback_YouCantBePolymorphed");
                } else if let Some(p) = self.player.as_mut() {
                    p.toggle_crawl();
                }
            }
            0x89 => self.send_outgoing(vec![action::plain_action(action::id::FORAGE)]),
            0x86 => self.search(),
            // Reload `FUN_1006949a` [GC]: action 0xd2. UNRESOLVED: its gate `FUN_10029e66` (a field of the manager's `+4` object being 0).
            0x6e => self.send_outgoing(vec![action::plain_action(action::id::RELOAD)]),
            _ => eprintln!("hud: special action {a:#x} has no consumer yet"),
        }
    }

    fn feedback(&mut self, key: &str) {
        if let Some(c) = self.chat.as_mut() {
            c.feedback(&mut self.gui, key, &self.text);
        }
    }

    /// `N3Msg_UseItem(target, false)` [GC 0x286f8]: selected world objects use the same confirmation and use path as right clicks.
    fn use_target(&mut self) {
        let Some(id) = self.zone.selected_target() else { return };
        if let Some(i) = self.interact.as_mut() {
            i.use_item(&self.zone, id, false);
        }
    }

    /// `N3Msg_GetItem(target)` [GC 0x27beb]: `Feedback_InventoryFull` when no bag slot (`0x40..0x5e`) is free, else `ClientGetItemIIR_t`.
    fn pick_up(&mut self) {
        let Some(item) = self.zone.selected_target() else { return };
        if (0x40..0x5e).all(|s| self.zone.inventory.contains_key(&s)) {
            return self.feedback("Feedback_InventoryFull");
        }
        let payload = ao_net::n3::inventory::get_item(self.zone.char_id as i32, item);
        if let Some(m) = self.fight.as_mut() {
            m.push(payload);
        }
    }

    /// Search `FUN_1003fa2c` [GC]: nothing while the Search recharge (action 0x88) runs (the original prints its remaining time through
    /// `FUN_10064301`; the text keys are UNRESOLVED), else `Feedback_SearchingForHiddenObjects` and `CharacterActionIIR_t` 0x42 (the effect 0xb01c on
    /// the own dynel is not created).
    fn search(&mut self) {
        if self.hud.as_ref().is_some_and(|h| h.actions.list.progress(0x88).is_some()) {
            return;
        }
        self.feedback("Feedback_SearchingForHiddenObjects");
        self.send_outgoing(vec![action::plain_action(action::id::SEARCH)]);
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

    /// `CancelCampMessage` [GUI 0x10029c26]: the timer bar is deleted (`TimerSystemModule_t::DeleteTimer`), text `TimedLogoutAborted`
    /// (code 12), state 0, quit time 0.
    fn cancel_camp(&mut self) {
        self.camp_bar_close();
        self.camp = None;
        self.logout.cancel();
        if let Some(c) = self.chat.as_mut() {
            c.logout_line(&mut self.gui, "TimedLogoutAborted", None, &self.text);
        }
    }

    /// `TimerSystemModule_t::CreateTimer(40000, 0:0, "Logout", 0xffffff)` [GUI 0x100518f0] -> `TimerBar_c` (ctor 0x100512ae): a `RenderWindow_t`
    /// holding a `PowerBar_t(gfx 0x1a8 `GFX_GUI_TIMERBAR_EMPTY` / 0x1a9 `GFX_GUI_TIMERBAR_FULL`)` sized to the art and a `TextLine_t` with the
    /// name; `CampStartedMessage` fixes the time at 30.0 s, direction 1 (`FUN_1002ba93`: the bar drains) and centres it,
    /// `((display - size) / 2)` per axis. 40000 is the `RenderWindow_t` / `PowerBar_t` id argument, not a duration. [UNRESOLVED] the text
    /// colour (`TextLine_t::SetDefaultColor(0)`: palette entry 0) and font; the frame-less window stands in for the `RenderWindow_t` layer 7.
    fn camp_bar_open(&mut self) {
        self.camp_bar_close();
        let (bw, bh) = self.gui.gfx_id("GFX_GUI_TIMERBAR_EMPTY").map(ao_gui::GfxId).map_or((128, 16), |g| self.gui.gfx().size(g));
        let xml = format!(
            "<root><View view_layout=\"stacked\" name=\"camp_timer\" min_size=\"Point({bw},{bh})\" max_size=\"Point({bw},{bh})\">\
             <PowerBar name=\"camp_bar\" bg_gfx=\"GFX_GUI_TIMERBAR_EMPTY\" full_gfx=\"GFX_GUI_TIMERBAR_FULL\" direction=\"right\"/>\
             <TextView name=\"camp_label\" h_alignment=\"center\" v_alignment=\"center\"/></View></root>"
        );
        match self.gui.open_window_xml("TimerBar", &xml, (0, 0), ao_gui::WindowSize::Fixed(bw, bh)) {
            Ok(w) => {
                self.gui.set_text(w, "camp_label", "Logout");
                let (ow, oh) = self.gui.outer_size(w);
                self.gui.set_window_pos(w, ((self.size.0 as i32 - ow as i32) / 2, (self.size.1 as i32 - oh as i32) / 2));
                self.camp_bar = Some(w);
            }
            Err(e) => eprintln!("hud: logout timer bar: {e:#}"),
        }
    }

    fn camp_bar_close(&mut self) {
        if let Some(w) = self.camp_bar.take() {
            self.gui.close_window(w);
        }
    }

    /// The camp countdown (`FlowControlModule_t::m_pcCampTimer`, 30 s): its end is the server dropping the connection, and
    /// `ServerLostMessage` [GUI 0x10028d50] runs `ActivateGameClosing(state)`: state 1 quits the game, state 2 returns to the login (config
    /// saved, screen cleared, then login [GUI 0x10028194]). [INFERENCE] the 30 s expiry itself is the server's: `StartLogoutIIR_t` /
    /// `StopLogoutIIR_t` have no client apply [GC 0x10079c53 / 0x10079e6c]. The timer bar's level is `remaining / total`
    /// (`TimerBar_c` vtable slot 0 [GUI 0x100514d5], direction 1).
    pub(in crate::play) fn camp_frame(&mut self, dt: f32, host: &mut ao_render::Host) {
        let Some(t) = self.camp.as_mut() else { return };
        *t += dt;
        let t = *t;
        if let Some(w) = self.camp_bar {
            self.gui.set_progress(w, "camp_bar", (1.0 - t / CAMP_SECONDS).max(0.0));
        }
        if t >= CAMP_SECONDS {
            self.camp = None;
            self.camp_bar_close();
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
                // `CampStartedMessage` [GUI 0x10029d38]: timer 40000 "Logout", state 0 -> 1, text `LogoutStartedXseconds` fed 30 (code 12)
                self.camp = Some(0.0);
                self.camp_bar_open();
                self.logout.camp_started();
                if let Some(c) = self.chat.as_mut() {
                    c.logout_line(&mut self.gui, "LogoutStartedXseconds", Some(CAMP_SECONDS as i32), &self.text);
                }
                true
            }
            Err(r) => {
                if let Some(c) = self.chat.as_mut() {
                    c.feedback(&mut self.gui, r.key(), &self.text);
                }
                false
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
