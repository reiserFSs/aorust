//! In-game interactions a new player needs: talking to NPCs (KnuBot dialogue), using world objects. Layouts and evidence: docs/zone/interact.md.
//!
//! * [`Interact::default_action_on`] is `N3Msg_DefaultActionOnDynel` [GC 0x100291da] (right click, double click): a character with the dialogue flag
//!   (stat `0x300`, bit 0) gets `KnubotOpenChatWindowIIR_c`; the server answers with `KnubotOpenChatWindow` / `AppendText` / `AnswerList`, which
//!   [`Interact::on_frame`] turns into the NPC chat window ([`interact_chat`](super::interact_chat)).
//! * Everything the player does in a dialogue goes out through [`Interact::take_outbox`].

use super::interact_chat::NpcChat;
use super::interact_grid::GridUi;
use super::interact_mission::MissionUi;
use super::interact_ptrade::PTradeUi;
use super::interact_shop::ShopUi;
use super::interact_trade::TradeUi;
use super::interact_use::UseUi;
use super::zone::Zone;
use ao_gui::{Event, Gui};
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::grid::Grid;
use ao_net::n3::knubot::{self, Knubot};
use ao_net::n3::misc::{GenericArgs, GenericCmd, Misc};
use ao_net::n3::outgoing::{n3_frame, DYNEL_CHAR};
use ao_net::n3::{self, N3};

/// Stat `0x300` (no name in the client's table): bit 0 marks a character the player can talk to (`N3Msg_DefaultActionOnDynel` tests
/// `HasStat(0x300)` and `GetStat(0x300, 2) & 1`). The server sends it as a `StatIIR_t` pair per NPC (docs/zone/dynel.md §3).
pub const STAT_TALK: i32 = 0x300;
/// `GenericCmd_t` command of `N3Msg_UseItem` for a world object: `FUN_1007c95c(actor, ItemActionData, 3)` [GC 0x100286f8].
const CMD_USE_ITEM: i32 = 3;

#[derive(Default)]
pub struct Interact {
    pub(super) own: u32,
    /// The last plain left click (dynel, `Play::time`), for double-click detection.
    last_click: Option<(Identity, f32)>,
    pub(super) chat: Option<NpcChat>,
    /// The NPC trade window and the button bar's state (`interact_trade.rs`).
    pub(super) trade: TradeUi,
    /// Grid / whompah / shuttle destination window (`interact_grid.rs`).
    grid: GridUi,
    /// Object use: the confirmation dialog, the loot windows, refusals the chat shows (`interact_use.rs`).
    pub(super) use_ui: UseUi,
    /// The player-to-player trade (`interact_ptrade.rs`).
    pub(super) ptrade: PTradeUi,
    /// The vending machine buy window (`interact_shop.rs`).
    pub(super) shop: ShopUi,
    /// Mission terminal selection (`MissionSelectionView_c`).
    pub(super) mission: MissionUi,
    outbox: Vec<Frame>,
    /// Text of `KnubotCloseChatWindow` for the chat window ([INFERENCE]: the `+0xf0` slot's consumer was not located; the live server sends the reason, e.g. "You are too far away from <npc> to continue this conversation.").
    notices: Vec<String>,
    /// `n3Command_t` sequence numbers of the commands we sent.
    seq: i32,
    use_actions: super::combat::use_actions::Uses,
    /// Screen size for centring the window.
    pub(super) screen: (u32, u32),
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

    pub(super) fn own_id(&self) -> Identity {
        Identity { kind: DYNEL_CHAR, instance: self.own as i32 }
    }

    fn own_name(zone: &Zone) -> String {
        zone.own().map(|d| d.name.clone()).unwrap_or_default()
    }


    /// Records a left click on `id` at `now`; true when it is the second click of a double click.
    pub fn double_click(&mut self, id: Identity, now: f32) -> bool {
        let double = self.last_click.is_some_and(|(i, t)| i == id && now - t <= ao_gui::DOUBLE_CLICK_TIME);
        self.last_click = if double { None } else { Some((id, now)) };
        double
    }

    /// The zone changes or the connection ends: the dialogue window goes away without a word to the server.
    pub fn close_all(&mut self, gui: &mut Gui) {
        self.use_actions.clear();
        self.use_ui.close_all(gui);
        self.ptrade.close_all(gui);
        self.shop_close_all(gui);
        self.grid.close_all(gui);
        self.mission.close_all(gui);
        if let Some(c) = self.chat.take() {
            c.close(gui);
        }
    }

    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    pub fn take_outbox(&mut self) -> Vec<Frame> {
        for frame in &self.outbox {
            if let Ok(message) = n3::decode(frame) { self.use_actions.request(&message); }
        }
        std::mem::take(&mut self.outbox)
    }

    /// Actual outgoing use requests, after their inventory/world gates succeeded.
    pub fn observe_use_request(&mut self, frame: &Frame, _zone: &Zone) {
        if let Ok(message) = n3::decode(frame) { self.use_actions.request(&message); }
    }

    pub fn advance_use_actions(&mut self, dt: f32, zone: &mut Zone) {
        self.use_actions.update(zone, dt);
    }

    pub fn take_use_actions(&mut self) -> Vec<super::combat::use_actions::Playback> {
        self.use_actions.take()
    }

    pub(super) fn send(&mut self, payload: Vec<u8>) {
        self.outbox.push(n3_frame(0, self.own, payload));
    }

    /// Every received zone frame: Knubot, trade, inventory and grid messages.
    pub fn on_frame(&mut self, gui: &mut Gui, f: &Frame, zone: &mut Zone) {
        let Ok(m) = n3::decode(f) else { return };
        let who = m.header.target;
        self.shop.literacy = zone.skill_value(0xa1).unwrap_or(0).min(3000);
        self.watch_objects(gui, &m);
        self.shop_watch(&m);
        self.use_actions.on_message(&m, zone);
        match m.body {
            N3::Knubot(k) => self.on_knubot(gui, k, zone),
            N3::Trade(t) => self.on_trade(gui, t, who, zone),
            N3::Inventory(m) => self.ptrade_inventory(&m),
            N3::Grid(Grid::DestinationSelect { destinations, token }) => self.grid.activate(gui, self.screen, zone, who, destinations, token),
            N3::MissionSelection(alternatives) if who == self.own_id() => {
                if let Some(notice) = self.mission.alternatives(gui, alternatives) {
                    self.notices.push(notice);
                }
            }
            N3::Misc(Misc::GenericCmd(cmd)) if cmd.state == 1 && cmd.cmd == CMD_USE_ITEM => {
                if let GenericArgs::Item { actor, item, .. } = cmd.args {
                    if actor == self.own_id() && zone.world.item_class_of(item.kind, item.instance) == Some(0xdac1) {
                        // QuestBooth use callback (`FUN_10086571`) emits GlobalSignals +0x164.
                        let origin_type = zone.world.stat_of(item.kind, item.instance, 0x1ea).filter(|v| (1..=8).contains(v)).unwrap_or(1) as u8;
                        if let Err(error) = self.mission.open(gui, self.screen, origin_type, item) {
                            eprintln!("mission selection: {error:#}");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The signal slots of GlobalSignals `+0xec` .. `+0x100` ([GUI] `NPCChatModule`, `FUN_1002d36x`; `NPCChatView_c` `FUN_100586fd` / `FUN_10058dc4`).
    fn on_knubot(&mut self, gui: &mut Gui, k: Knubot, zone: &Zone) {
        eprintln!("interact: {k:?}");
        let own = Self::own_name(zone);
        match k {
            // `FUN_1002d36x`: an open window is deleted first (its destructor sends `NPCChatCloseWindow`), then a new one is built
            Knubot::Open { npc, b20, b21 } => {
                if let Some(old) = self.chat.take() {
                    let (n, id) = (old.npc, self.own_id());
                    old.close(gui);
                    self.send(knubot::close_window(id, n));
                }
                let name = zone.dynels.get(&npc.instance).map(|d| d.name.clone()).unwrap_or_default();
                let use_npc = zone.stat_of(npc.instance, 0).is_some_and(|flags| flags & super::interact_chat::STAT0_USE_BIT != 0);
                self.open_chat(gui, npc, &name, b20, b21, use_npc);
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
            Knubot::StartTrade { .. } | Knubot::RejectedItems { .. } => self.on_trade_knubot(gui, &k),
            _ => {}
        }
        self.trade_sync(gui);
    }

    /// GUI events of the NPC chat window; `true` when consumed.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let me = self.own_id();
        if let Some(out) = self.use_ui.event(gui, ev) {
            self.use_out(zone, out);
            return true;
        }
        if self.ptrade_event(gui, ev, zone) || self.shop_event(gui, ev, zone) {
            return true;
        }
        if let Some(out) = self.grid.event(gui, ev, me) {
            out.into_iter().for_each(|p| self.send(p));
            return true;
        }
        if let Some(out) = self.mission.event(gui, ev, me, zone) {
            out.into_iter().for_each(|p| self.send(p));
            return true;
        }
        if self.trade_event(gui, ev, zone) {
            return true;
        }
        let own = Self::own_name(zone);
        let Some(c) = self.chat.as_mut() else { return false };
        let Some(out) = c.event(gui, ev, &own) else { return false };
        if let Some(out) = out {
            self.chat_out(gui, out);
        }
        self.trade_sync(gui);
        true
    }

    /// Click an answer link of the open dialogue by its index (the live harness / tests; the window's link event does the same).
    #[cfg(test)]
    pub fn answer(&mut self, gui: &mut Gui, zone: &Zone, index: usize) -> bool {
        let own = Self::own_name(zone);
        let Some(c) = self.chat.as_mut() else { return false };
        let npc = c.npc;
        match c.link(gui, &index.to_string(), &own) {
            Some(super::interact_chat::ChatOut::Answer(i)) => {
                let id = self.own_id();
                self.send(knubot::answer(id, npc, i));
                true
            }
            _ => false,
        }
    }

    /// `N3Msg_UseItem` [GC 0x100286f8] on a world object: `GenericCmd_t(state 0, seq, cmd 3, ItemActionData{actor = own, item})`.
    pub fn use_object(&mut self, item: Identity) {
        self.seq += 1;
        let cmd = GenericCmd { state: 0, seq: self.seq, cmd: CMD_USE_ITEM, args: GenericArgs::Item { flag: 0, actor: self.own_id(), item } };
        let p = Misc::GenericCmd(cmd).encode(self.own_id(), 1);
        self.send(p);
    }
}

/// What [`Interact::default_action_on`] / [`Interact::use_item`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// `KnubotOpenChatWindow` sent.
    Talk,
    /// `ClientGetItemIIR_t` sent (`N3Msg_GetItem`): `Can` bit 0.
    Get,
    /// `GenericCmd_t` 3 sent (`N3Msg_UseItem`): `Can` bit 3, or the confirmation was answered Yes.
    Use,
    /// `TradeIIR_t` op 0 sent (`N3Msg_TradeStart`): a character that is not talkable.
    Trade,
    /// `TradeIIR_t` op 2 sent (`N3Msg_TradeAbort(true)`).
    Abort,
    /// `Can` bit 4: the "UseItem" confirmation dialog is asked for (`GuiSystem_c::ConfirmUseItemDialogue`).
    Confirm,
    /// Refused with the `Feedback_*` text of the key (chat category 110).
    Refused(&'static str),
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
    pub fn flagged(&self, zone: &Zone) -> Vec<(i32, i32)> {
        let mut v: Vec<_> = zone.dynels.keys().filter_map(|id| zone.stat_of(*id, STAT_TALK as u32).map(|value| (*id, value))).collect();
        v.sort();
        v
    }
}
