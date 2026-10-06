//! In-game interactions a new player needs: talking to NPCs (KnuBot dialogue), using world objects. Layouts and evidence: docs/zone/interact.md.
//!
//! * [`Interact::default_action`] is `N3Msg_DefaultActionOnDynel` [GC 0x100291da] (right click, double click): a character with the dialogue flag
//!   (stat `0x300`, bit 0) gets `KnubotOpenChatWindowIIR_c`; the server answers with `KnubotOpenChatWindow` / `AppendText` / `AnswerList`, which
//!   [`Interact::on_frame`] turns into the NPC chat window ([`interact_chat`](super::interact_chat)).
//! * Everything the player does in a dialogue goes out through [`Interact::take_outbox`].

use super::interact_chat::{ChatOut, NpcChat};
use super::interact_grid::GridUi;
use super::zone::Zone;
use ao_gui::{Event, Gui};
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::grid::Grid;
use ao_net::n3::knubot::{self, Knubot};
use ao_net::n3::misc::{GenericArgs, GenericCmd, Misc};
use ao_net::n3::outgoing::{n3_frame, DYNEL_CHAR};
use ao_net::n3::{self, dynel::Dynel, N3};
use std::collections::HashMap;

/// Stat `0x300` (no name in the client's table): bit 0 marks a character the player can talk to (`N3Msg_DefaultActionOnDynel` tests
/// `HasStat(0x300)` and `GetStat(0x300, 2) & 1`). The server sends it as a `StatIIR_t` pair per NPC (docs/zone/dynel.md §3).
pub const STAT_TALK: i32 = 0x300;
/// `GenericCmd_t` command of `N3Msg_UseItem` for a world object: `FUN_1007c95c(actor, ItemActionData, 3)` [GC 0x100286f8].
const CMD_USE_ITEM: i32 = 3;

#[derive(Default)]
pub struct Interact {
    own: u32,
    /// The last plain left click (dynel, `Play::time`), for double-click detection.
    last_click: Option<(i32, f32)>,
    /// Last value of stat `0x300` per dynel.
    talk: HashMap<i32, i32>,
    chat: Option<NpcChat>,
    /// Grid / whompah / shuttle destination window (`interact_grid.rs`).
    grid: GridUi,
    outbox: Vec<Frame>,
    /// Text of `KnubotCloseChatWindow` for the chat window ([INFERENCE]: the `+0xf0` slot's consumer was not located; the live server sends the reason, e.g. "You are too far away from <npc> to continue this conversation.").
    notices: Vec<String>,
    /// `n3Command_t` sequence numbers of the commands we sent.
    seq: i32,
    /// Screen size for centring the window.
    screen: (u32, u32),
    /// Every NPC dialogue line seen, in order, for the live harness (`HTML` as shown).
    pub log: Vec<String>,
}

impl Interact {
    pub fn new(own: u32, screen: (u32, u32)) -> Self {
        Interact { own, screen, ..Default::default() }
    }

    pub fn resize(&mut self, screen: (u32, u32)) {
        self.screen = screen;
    }

    fn own_id(&self) -> Identity {
        Identity { kind: DYNEL_CHAR, instance: self.own as i32 }
    }

    fn own_name(zone: &Zone) -> String {
        zone.own().map(|d| d.name.clone()).unwrap_or_default()
    }

    /// The dialogue flag of `id` (`HasStat(0x300) && GetStat(0x300) & 1`).
    pub fn talkable(&self, id: i32) -> bool {
        self.talk.get(&id).is_some_and(|v| v & 1 != 0)
    }

    /// Records a left click on `id` at `now`; true when it is the second click of a double click.
    pub fn double_click(&mut self, id: i32, now: f32) -> bool {
        let double = self.last_click.is_some_and(|(i, t)| i == id && now - t <= ao_gui::DOUBLE_CLICK_TIME);
        self.last_click = if double { None } else { Some((id, now)) };
        double
    }

    /// The zone changes or the connection ends: the dialogue window goes away without a word to the server.
    pub fn close_all(&mut self, gui: &mut Gui) {
        self.grid.close_all(gui);
        if let Some(c) = self.chat.take() {
            c.close(gui);
        }
    }

    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    fn send(&mut self, payload: Vec<u8>) {
        self.outbox.push(n3_frame(0, self.own, payload));
    }

    /// Every received zone frame: `StatIIR_t` pairs of stat `0x300`, and the Knubot messages.
    pub fn on_frame(&mut self, gui: &mut Gui, f: &Frame, zone: &Zone) {
        let Ok(m) = n3::decode(f) else { return };
        let who = m.header.target;
        match m.body {
            N3::Dynel(Dynel::Stat(s)) if who.kind == DYNEL_CHAR => {
                for (stat, v) in s.stats {
                    if stat == STAT_TALK {
                        self.talk.insert(who.instance, v);
                    }
                }
            }
            N3::Knubot(k) => self.on_knubot(gui, k, zone),
            N3::Grid(Grid::DestinationSelect { destinations, token }) => self.grid.activate(gui, self.screen, zone, who, destinations, token),
            _ => {}
        }
    }

    /// The signal slots of GlobalSignals `+0xec` .. `+0x100` ([GUI] `NPCChatModule`, `FUN_1002d36x`; `NPCChatView_c` `FUN_100586fd` / `FUN_10058dc4`).
    fn on_knubot(&mut self, gui: &mut Gui, k: Knubot, zone: &Zone) {
        eprintln!("interact: {k:?}");
        let own = Self::own_name(zone);
        match k {
            // `FUN_1002d36x`: an open window is deleted first (its destructor sends `NPCChatCloseWindow`), then a new one is built
            Knubot::Open { npc, .. } => {
                if let Some(old) = self.chat.take() {
                    let (n, id) = (old.npc, self.own_id());
                    old.close(gui);
                    self.send(knubot::close_window(id, n));
                }
                let name = zone.dynels.get(&npc.instance).map(|d| d.name.clone()).unwrap_or_default();
                match NpcChat::open(gui, self.screen, npc, &name) {
                    Ok(c) => self.chat = Some(c),
                    Err(e) => eprintln!("interact: NPC chat window: {e:#}"),
                }
            }
            Knubot::AppendText { npc, kind, text } => {
                if let Some(c) = self.chat.as_mut().filter(|c| c.npc == npc) {
                    self.log.push(format!("[{kind}] {text}"));
                    c.append(gui, &text, kind, &own);
                }
            }
            Knubot::AnswerList { npc, answers } => {
                if let Some(c) = self.chat.as_mut().filter(|c| c.npc == npc) {
                    self.log.push(format!("answers: {}", answers.join(" | ")));
                    c.set_answers(gui, answers);
                }
            }
            // the server closed the window: the destructor does not answer (`this[0x98]` set)
            Knubot::Close { npc, text, .. } => {
                if !text.is_empty() {
                    self.notices.push(text);
                }
                if let Some(c) = self.chat.take_if(|c| c.npc == npc) {
                    c.close(gui);
                }
            }
            _ => {}
        }
    }

    /// GUI events of the NPC chat window; `true` when consumed.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let me = self.own_id();
        if let Some(out) = self.grid.event(gui, ev, me) {
            out.into_iter().for_each(|p| self.send(p));
            return true;
        }
        let own = Self::own_name(zone);
        let Some(c) = self.chat.as_mut() else { return false };
        let npc = c.npc;
        let Some(out) = c.event(gui, ev, &own) else { return false };
        let id = self.own_id();
        match out {
            Some(ChatOut::Answer(i)) => self.send(knubot::answer(id, npc, i)),
            Some(ChatOut::Closed) => {
                if let Some(c) = self.chat.take() {
                    c.close(gui);
                }
                self.send(knubot::close_window(id, npc));
            }
            None => {}
        }
        true
    }

    /// Click an answer link of the open dialogue by its index (the live harness / tests; the window's link event does the same).
    #[cfg(test)]
    pub fn answer(&mut self, gui: &mut Gui, zone: &Zone, index: usize) -> bool {
        let own = Self::own_name(zone);
        let Some(c) = self.chat.as_mut() else { return false };
        let npc = c.npc;
        match c.link(gui, &index.to_string(), &own) {
            Some(ChatOut::Answer(i)) => {
                let id = self.own_id();
                self.send(knubot::answer(id, npc, i));
                true
            }
            _ => false,
        }
    }

    /// `N3Msg_DefaultActionOnDynel` [GC 0x100291da] on a character. The dialogue branch: `HasStat(0x300)` with bit 0 -> `KnubotOpenChatWindowIIR_c(own,
    /// npc, 0, 0)` (`FUN_10127f4d`). The player-trade branch (`N3Msg_TradeStart`) is not ported: a new player has nobody to trade with.
    /// Returns what the action was.
    pub fn default_action(&mut self, id: i32) -> Action {
        if id == self.own as i32 {
            return Action::None;
        }
        if self.talkable(id) {
            let (own, npc) = (self.own_id(), Identity { kind: DYNEL_CHAR, instance: id });
            self.send(knubot::open_chat_window(own, npc));
            return Action::Talk;
        }
        Action::None
    }

    /// `N3Msg_UseItem` [GC 0x100286f8] on a world object: `GenericCmd_t(state 0, seq, cmd 3, ItemActionData{actor = own, item})`.
    pub fn use_object(&mut self, item: Identity) {
        self.seq += 1;
        let cmd = GenericCmd { state: 0, seq: self.seq, cmd: CMD_USE_ITEM, args: GenericArgs::Item { flag: 0, actor: self.own_id(), item } };
        let p = Misc::GenericCmd(cmd).encode(self.own_id(), 1);
        self.send(p);
    }
}

/// What [`Interact::default_action`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// `KnubotOpenChatWindow` sent.
    Talk,
}

/// Live-harness helpers (`flow/live.rs` steps `talk`, `dlg`, `answer`, `useobj`, `npcs`).
#[cfg(test)]
impl Interact {
    /// The dialogue as the harness prints it: window state, the lines shown, the answers pending.
    pub fn dump(&self, gui: &Gui) -> String {
        match &self.chat {
            None => "dialogue: no window".into(),
            Some(c) => format!(
                "dialogue with {:?} ({:?})\n  text: {}\n  answers: {:?}\n  shown: {:?}",
                c.name,
                c.npc,
                gui.text(c.win, "npc_text"),
                c.answers,
                self.log
            ),
        }
    }

    /// The grid window's rows as shown (`interact_grid.rs`).
    pub fn grid_dump(&self, gui: &Gui) -> String {
        self.grid.dump(gui)
    }

    /// Select list entry `index` in the grid window and press Go; false when there is no such entry.
    pub fn grid_select(&mut self, gui: &mut Gui, index: usize) -> bool {
        let me = self.own_id();
        let Some(out) = self.grid.select(gui, me, index) else { return false };
        out.into_iter().for_each(|p| self.send(p));
        true
    }

    /// `(instance, stat 0x300)` of every dynel the server flagged.
    pub fn flagged(&self) -> Vec<(i32, i32)> {
        let mut v: Vec<_> = self.talk.iter().map(|(k, v)| (*k, *v)).collect();
        v.sort();
        v
    }
}
