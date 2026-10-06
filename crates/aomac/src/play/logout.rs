//! `FlowControlModule_t::m_eLoggingOutTimed` (GUI 0x102760c8) and `m_nQuitToSystemTime` (0x102760d0): the `/camp` / `/quit` state.
//!
//! State 0 = none, 1 = a timed logout to the system (`/quit`, or camping started by the hotkey: `CampStartedMessage` 0x10029d38
//! sets 0 -> 1), 2 = a timed logout to the login (`/camp`, `StartQuitToLoginMessage` 0x10027c74). The camp timer ends with the
//! server dropping the connection: `ServerLostMessage` 0x10028d50 calls `ActivateGameClosing(state)` when the state is not 0
//! (1 = quit to the system, 2 = back to the login). docs/chat/dialogs.md §3.

/// `/quit` pressed twice within this many ms quits at once (`StartQuitToSystemMessage` 0x10029a0d: `now < time + 3000`).
pub const DOUBLE_QUIT_MS: u64 = 3000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    None,
    /// `/quit` timed logout.
    System,
    /// `/camp` timed logout.
    Login,
}

/// What the end of the camp timer does (`ActivateGameClosing(1 | 2)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    System,
    Login,
}

#[derive(Default)]
pub struct Logout {
    pub state: State,
    /// `m_nQuitToSystemTime`: the time of the last `/quit` (ms, game timer), 0 = none.
    quit_time: u64,
}

impl Logout {
    /// `StartQuitToSystemMessage` first test: quit now when the state is 1 or the previous `/quit` was less than 3 s ago.
    /// Otherwise the time is stored (`now_ms` > 0) and the caller prints text `ClosingClient` and starts camping.
    pub fn quit_now(&mut self, now_ms: u64) -> bool {
        if self.state == State::System || (self.quit_time != 0 && now_ms < self.quit_time + DOUBLE_QUIT_MS) {
            return true;
        }
        self.quit_time = now_ms.max(1);
        false
    }

    /// `CampStartedMessage`: state 0 -> 1 (the hotkey path: nothing set it before).
    pub fn camp_started(&mut self) {
        if self.state == State::None {
            self.state = State::System;
        }
    }

    /// `CancelCampMessage` 0x10029c26: the camp timer is deleted, `m_nQuitToSystemTime = 0`, state 0.
    pub fn cancel(&mut self) {
        *self = Self::default();
    }

    /// `ServerLostMessage` after the camp countdown: `ActivateGameClosing(state)` resets the state to 0. `None` at state 0 (the original
    /// then closes with status 3, "connection lost": not a logout).
    pub fn expired(&mut self) -> Option<Closing> {
        let c = match self.state {
            State::None => return None,
            State::System => Closing::System,
            State::Login => Closing::Login,
        };
        *self = Self::default();
        Some(c)
    }
}

/// `Timer_t+0x24`: milliseconds of the game timer (here: since the first call, never 0).
pub fn now_ms() -> u64 {
    static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
    START.elapsed().as_millis() as u64 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_quit_within_three_seconds_quits_at_once() {
        let mut l = Logout::default();
        assert!(!l.quit_now(10_000));
        assert!(l.quit_now(12_999));
        let mut l = Logout::default();
        assert!(!l.quit_now(10_000));
        assert!(!l.quit_now(13_000), "3000 ms is outside the window (`now < time + 3000`) and restarts it");
        assert!(l.quit_now(14_000));
    }

    #[test]
    fn timed_logout_state_quits_at_once_and_cancel_resets() {
        let mut l = Logout { state: State::System, ..Default::default() };
        assert!(l.quit_now(5));
        l.cancel();
        assert_eq!(l.state, State::None);
        assert!(!l.quit_now(5), "the stored time was cleared by the cancel");
        let mut c = Logout { state: State::Login, ..Default::default() };
        assert!(!c.quit_now(5), "/quit during a /camp countdown only switches the target (state 2 -> 1)");
    }

    #[test]
    fn hotkey_camp_ends_in_a_system_quit_and_camp_command_in_the_login() {
        let mut l = Logout::default();
        l.camp_started();
        assert_eq!(l.expired(), Some(Closing::System));
        assert_eq!((l.state, l.expired()), (State::None, None));
        l.state = State::Login;
        l.camp_started();
        assert_eq!(l.expired(), Some(Closing::Login));
    }
}
