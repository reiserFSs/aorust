//! The NPC trade window (`NPCChatTradeBar_c`, `FUN_100575a1` [GUI]) and the trade flow of the NPC dialogue. Evidence: docs/zone/interact.md §2 and §7.
//!
//! * The trade button of the button bar (`FUN_1005852c`) sends `KnubotStartTrade(0, "")`; the server answers `KnubotStartTrade(max items, text)`
//!   (`FUN_10058a4c`): when no trade bar exists the text goes into the answer view as `<font color=CCNPCChatTrade>text</font>` and the trade bar is created in its own
//!   style-2 window titled `Trade` (LDB 10000) docked to the right of the chat window (dock slot 2).
//! * The bar: a label `GIVE ITEM` / `GIVE ITEMS`, an `ItemContainerView_c` (a `MultiListView_c` icon grid of `(max != 1) + 1` columns x `(max + 1) / 2` rows, max
//!   items = the message's value), a label `GIVE CREDITS`, a numeric text input in an inset border, and the accept (`GFX_GUI_BUTTON_V_*` 0x4b/0x4c) and decline
//!   (`GFX_GUI_BUTTON_X_*` 0x4d/0x4e) buttons.
//! * Dropping an inventory item on the container (`FUN_100572f2`): while it holds fewer than max items a `DragObject_c` `inventory/item` (`container_id`, `item_id`,
//!   no `split_count`) -> `N3Msg_NPCChatAddTradeItem(own, npc, item)` and the drop is accepted; otherwise it is cancelled. Removing an item from the container
//!   (`FUN_10057421`) -> `N3Msg_NPCChatRemoveTradeItem`; the other container signal (`FUN_10057473`) shows `itemid://%d/%d`.
//! * Accept (`FUN_100574dc`): the credits text is read with `atoi`, clamped to the Cash stat `0x3d` (the text is rewritten when it was larger),
//!   `N3Msg_NPCChatEndTrade(own, npc, credits, accepted = true)`; the client lowers its Cash by the credits at once. Decline and the trade button of a running trade
//!   (`FUN_10058408`): the chat view's last text type becomes 3, `N3Msg_NPCChatEndTrade(own, npc, 0, accepted = false)`, the trade window is deleted.
//! * The end of a trade is `KnubotRejectedItems` (`FUN_101281c6` -> GlobalSignals `+0x100` -> `LAB_10058362`): the credits of its `value` are paid back to Cash, the
//!   trade window is deleted and the last text type becomes 3.

use super::interact::Interact;
use super::interact_chat::{style2_xml, BarFlags, ChatOut, ChatTexts, NpcChat};
use super::zone::Zone;
use super::Play;
use ao_gui::{tvf, CanvasItem, DrawCmd, DrawList, Event, GfxId, Gui, MouseButton, WindowId, WindowSize};
use ao_net::msg::Identity;
use ao_net::n3::inventory as inv;
use ao_net::n3::knubot::{self, Knubot};

/// Cash (`0x3d`, `N3Msg_GetSkill(0x3d, 2)` in `FUN_100574dc`).
pub const STAT_CASH: u32 = 0x3d;
/// The container's cell: the 48 px item picture on the 54 px slot art `GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED` ([INFERENCE]: the `ItemContainerView_c` cell metrics were
/// not read; it is the same `MultiListView_c` grid as the inventory's).
const CELL: f32 = 54.0;
const ICON: f32 = 48.0;
const SLOT_GFX: &str = "GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED";

/// Columns and rows of the item container (`FUN_100575a1`: `SetViewCellCounts(IPoint((max != 1) + 1, (max + 1) / 2))`).
pub fn grid(max: i32) -> (u32, u32) {
    (if max == 1 { 1 } else { 2 }, ((max + 1) / 2).max(1) as u32)
}

fn trade_xml(max: i32) -> String {
    let (cols, rows) = grid(max);
    let (w, h) = (cols as f32 * CELL, rows as f32 * CELL);
    let label = |name: &str, text: &str, b: &str| {
        format!("<TextView font=\"NORMAL\" layout_borders=\"Rect({b})\" name=\"{name}\" value=\"{text}\"/>")
    };
    let btn = |name: &str, up: &str, down: &str, b: &str| format!("<Button gfxid_raised=\"{up}\" gfxid_pressed=\"{down}\" gfxid_hover=\"{up}\" layout_borders=\"Rect({b})\" name=\"{name}\"/>");
    let children = format!(
        "{}<CanvasView name=\"items\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/>{}<TextInputView layout_borders=\"Rect(5,5,5,0)\" min_size=\"Point(0,-1)\" max_size=\"Point(16000,-1)\" name=\"credits\"/><View view_layout=\"horizontal\">{}{}</View>",
        label("give_items", if max == 1 { "GIVE ITEM" } else { "GIVE ITEMS" }, "5,5,5,5"),
        label("give_credits", "GIVE CREDITS", "5,5,5,0"),
        btn("accept", "GFX_GUI_BUTTON_V_NORMAL", "GFX_GUI_BUTTON_V_PRESSED", "5,5,5,5"),
        btn("decline", "GFX_GUI_BUTTON_X_NORMAL", "GFX_GUI_BUTTON_X_PRESSED", "0,5,5,5"),
    );
    style2_xml("npctrade", "vertical", &children)
}

/// An item in the container: the inventory slot it came from, its identity and the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeItem {
    pub id: Identity,
    pub name: String,
    pub icon: Option<(GfxId, u32, u32)>,
}

/// What the window asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum TradeOut {
    /// The accept button with the (clamped) credits.
    Accept(i32),
    Decline,
    /// An item was taken out of the container (`FUN_10057421`).
    Remove(Identity),
    /// The info of an item (`FUN_10057473`: `ShowURL("itemid://%d/%d")`).
    Info(Identity),
}

pub struct TradeWin {
    pub win: WindowId,
    /// The `value` of `KnubotStartTrade`: how many items the NPC takes (`this+0x1bc`).
    pub max: i32,
    pub items: Vec<TradeItem>,
}

impl TradeWin {
    pub fn open(gui: &mut Gui, max: i32, title_pos: (i32, i32)) -> anyhow::Result<Self> {
        let win = gui.open_window_xml("NPCChatTradeBar", &trade_xml(max), title_pos, WindowSize::Preferred)?;
        gui.set_feature_flags(win, "credits", tvf::NUMERIC);
        let t = TradeWin { win, max, items: vec![] };
        t.paint(gui);
        Ok(t)
    }

    /// Room for another item (`FUN_100572f2`: the container holds fewer than `max`).
    pub fn has_room(&self) -> bool {
        (self.items.len() as i32) < self.max
    }

    /// Is the screen point over the item container (the drop target)?
    pub fn over_items(&self, gui: &Gui, x: f32, y: f32) -> bool {
        gui.view_rect(self.win, "items").is_some_and(|r| x >= r.l && x <= r.r + 1.0 && y >= r.t && y <= r.b + 1.0)
    }

    fn paint(&self, gui: &mut Gui) {
        let (cols, rows) = grid(self.max);
        let slot = gui.gfx().id(SLOT_GFX);
        let mut v = vec![];
        for i in 0..(cols * rows) as usize {
            let (x, y) = ((i as u32 % cols) as f32 * CELL, (i as u32 / cols) as f32 * CELL);
            if let Some(id) = slot {
                let (w, h) = gui.gfx().size(id);
                v.push(CanvasItem::Image { id, src: [0.0, 0.0, w as f32, h as f32], dst: [x, y, x + CELL, y + CELL], alpha: 1.0 });
            }
            if let Some((id, w, h)) = self.items.get(i).and_then(|t| t.icon) {
                let o = (CELL - ICON) / 2.0;
                v.push(CanvasItem::Image { id, src: [0.0, 0.0, w as f32, h as f32], dst: [x + o, y + o, x + o + ICON, y + o + ICON], alpha: 1.0 });
            }
        }
        gui.set_canvas(self.win, "items", v);
    }

    /// The item is in the container.
    pub fn add(&mut self, gui: &mut Gui, item: TradeItem) {
        self.items.push(item);
        self.paint(gui);
    }

    pub fn remove(&mut self, gui: &mut Gui, id: Identity) -> bool {
        let Some(i) = self.items.iter().position(|t| t.id == id) else { return false };
        self.items.remove(i);
        self.paint(gui);
        true
    }

    /// `FUN_100574dc`: the credits field (`atoi`) clamped to `cash`; a larger number is rewritten.
    pub fn credits(&self, gui: &mut Gui, cash: i32) -> i32 {
        let txt = gui.text(self.win, "credits");
        let digits: String = txt.trim().chars().take_while(char::is_ascii_digit).collect();
        let n = digits.parse::<i64>().unwrap_or(0).min(i32::MAX as i64) as i32;
        if n > cash {
            gui.set_text(self.win, "credits", &cash.max(0).to_string());
            return cash.max(0);
        }
        n
    }

    pub fn event(&mut self, gui: &mut Gui, ev: &Event, cash: i32) -> Option<TradeOut> {
        match ev {
            Event::Clicked { window, view, .. } if *window == self.win && view == "accept" => Some(TradeOut::Accept(self.credits(gui, cash))),
            Event::Clicked { window, view, .. } if *window == self.win && view == "decline" => Some(TradeOut::Decline),
            Event::CanvasPress { window, view, x, y, button, clicks } if *window == self.win && view == "items" => {
                let (cols, _) = grid(self.max);
                let idx = (*y / CELL) as usize * cols as usize + (*x / CELL) as usize;
                let item = self.items.get(idx)?.id;
                // [UNRESOLVED] which gesture raises which container signal: `+0x2e0` (remove) and `+0x2e4` (info) are plain `MultiListView_c` signals; a left
                // double click removes, the right button asks for the info
                match (button, clicks) {
                    (MouseButton::Left, 2..) => Some(TradeOut::Remove(item)),
                    (MouseButton::Right, _) => Some(TradeOut::Info(item)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The window as the harness prints it.
    #[cfg(test)]
    pub fn dump(&self, gui: &Gui) -> String {
        format!("max {} items {:?} credits {:?} label {:?}", self.max, self.items.iter().map(|t| (t.id.kind, t.id.instance, &t.name)).collect::<Vec<_>>(), gui.text(self.win, "credits"), gui.text(self.win, "give_items"))
    }
}

/// State of the trade part of the dialogue (a field of [`Interact`]).
pub struct TradeUi {
    pub win: Option<TradeWin>,
    pub texts: ChatTexts,
    /// Pref `ShowNPCQuestions` (default 1).
    pub show_questions: bool,
    /// Change of the own Cash stat to apply (accept: minus the credits, rejected items: plus the value).
    pub cash_delta: i32,
    /// `ShowURL` requests of the bar's info button / the container.
    pub info_urls: Vec<String>,
    /// Pointer position, for the splitter's mouse pointer.
    pub mouse: (f32, f32),
    pub rejected: Vec<(Identity, i32, i32)>,
}

impl Default for TradeUi {
    fn default() -> Self {
        TradeUi { win: None, texts: ChatTexts::default(), show_questions: true, cash_delta: 0, info_urls: vec![], mouse: (-1.0, -1.0), rejected: vec![] }
    }
}

impl TradeUi {
    fn close(&mut self, gui: &mut Gui) {
        if let Some(t) = self.win.take() {
            gui.close_window(t.win);
        }
    }
}

impl Interact {
    /// `KnubotOpenChatWindow` -> `NPCChatView_c` (`FUN_10058ed8`): the window with the bar whose buttons are enabled by `b20` (description), `b21` (trade) and stat 0
    /// bit 21 of the NPC (use).
    pub(super) fn open_chat(&mut self, gui: &mut Gui, npc: Identity, name: &str, b20: bool, b21: bool, use_npc: bool) {
        self.trade.close(gui);
        match NpcChat::open(gui, self.screen, npc, name, BarFlags { description: b20, trade: b21, use_npc }, &self.trade.texts) {
            Ok(mut c) => {
                c.show_questions = self.trade.show_questions;
                self.chat = Some(c);
            }
            Err(e) => eprintln!("interact: NPC chat window: {e:#}"),
        }
    }

    /// `StartTrade` / `RejectedItems` of the dialogue.
    pub(super) fn on_trade_knubot(&mut self, gui: &mut Gui, k: &Knubot) {
        match k {
            // `FUN_10058a4c`: only when no trade bar exists
            Knubot::StartTrade { npc, value, text } if self.trade.win.is_none() => {
                let Some(c) = self.chat.as_mut().filter(|c| c.npc == *npc) else { return };
                self.log.push(format!("trade: {text}"));
                match TradeWin::open(gui, *value, (0, 0)) {
                    Ok(t) => {
                        c.set_trade_text(gui, text);
                        c.sync_docks(gui, Some(t.win));
                        self.trade.win = Some(t);
                    }
                    Err(e) => eprintln!("interact: NPC trade window: {e:#}"),
                }
            }
            // `FUN_101281c6`: the credits of `value` are paid back, the trade window goes away
            Knubot::RejectedItems { items, value, .. } => {
                self.trade.cash_delta += *value;
                self.trade.rejected = items.clone();
                self.log.push(format!("rejected {} items, {value} credits back", items.len()));
                self.trade.close(gui);
                if let Some(c) = self.chat.as_mut() {
                    c.trade_ended();
                }
            }
            _ => {}
        }
    }

    /// `FUN_10058408`: the trade ends by the player's decision.
    fn trade_cancel(&mut self, gui: &mut Gui) {
        let me = self.own_id();
        let Some(c) = self.chat.as_mut() else { return };
        c.trade_ended();
        let npc = c.npc;
        self.trade.close(gui);
        self.send(knubot::end_trade(me, npc, 0, false));
    }

    /// What a click on the button bar asks for.
    pub(super) fn chat_out(&mut self, gui: &mut Gui, out: ChatOut) {
        let Some(npc) = self.chat.as_ref().map(|c| c.npc) else { return };
        let me = self.own_id();
        match out {
            ChatOut::Answer(i) => self.send(knubot::answer(me, npc, i)),
            ChatOut::Link(url) => self.trade.info_urls.push(url),
            ChatOut::Closed => {
                if let Some(c) = self.chat.take() {
                    c.close(gui);
                }
                self.trade.close(gui);
                self.send(knubot::close_window(me, npc));
            }
            ChatOut::Description => self.send(knubot::request_description(me, npc)),
            ChatOut::Info => self.trade.info_urls.push(format!("charid://{}/{}", npc.kind, npc.instance)),
            ChatOut::Trade => {
                if self.trade.win.is_none() {
                    self.send(knubot::start_trade(me, npc));
                } else {
                    self.trade_cancel(gui);
                }
            }
            ChatOut::Use => self.use_object(npc),
        }
    }

    /// GUI events of the trade window; `true` when consumed.
    pub(super) fn trade_event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let Some(t) = self.trade.win.as_mut() else { return false };
        let Some(out) = t.event(gui, ev, zone.stat(STAT_CASH).unwrap_or(0)) else { return false };
        let (me, npc) = (self.own_id(), self.chat.as_ref().map(|c| c.npc));
        let Some(npc) = npc else { return true };
        match out {
            TradeOut::Accept(credits) => {
                self.trade.cash_delta -= credits;
                self.send(knubot::end_trade(me, npc, credits, true));
            }
            TradeOut::Decline => self.trade_cancel(gui),
            TradeOut::Remove(item) => {
                if let Some(t) = self.trade.win.as_mut() {
                    t.remove(gui, item);
                }
                self.send(knubot::trade_item(me, npc, 1, item));
            }
            TradeOut::Info(item) => self.trade.info_urls.push(format!("itemid://{}/{}", item.kind, item.instance)),
        }
        true
    }

    /// `FUN_100572f2`: an inventory item (`slot`) was released at the screen point; true when the container took it (the caller keeps the others).
    pub fn trade_drop(&mut self, gui: &mut Gui, slot: u32, x: f32, y: f32, (name, icon): (String, Option<(GfxId, u32, u32)>)) -> bool {
        let (me, npc) = (self.own_id(), self.chat.as_ref().map(|c| c.npc));
        let Some(t) = self.trade.win.as_mut().filter(|t| t.over_items(gui, x, y)) else { return false };
        let Some(npc) = npc else { return false };
        let id = inv::item_identity(slot);
        if !t.has_room() || t.items.iter().any(|i| i.id == id) {
            return true; // `DragObject_c::Cancel`: the drop is consumed, nothing moves
        }
        t.add(gui, TradeItem { id, name, icon });
        self.send(knubot::trade_item(me, npc, 0, id));
        true
    }

    /// Is the screen point over the trade container (a drop there is this window's)?
    pub fn trade_wants(&self, gui: &Gui, x: f32, y: f32) -> bool {
        self.trade.win.as_ref().is_some_and(|t| t.over_items(gui, x, y))
    }

    /// The dialogue is gone: so is its trade window (`NPCChatView_c`'s destructor deletes the bar).
    pub(super) fn trade_sync(&mut self, gui: &mut Gui) {
        if self.chat.is_none() {
            self.trade.close(gui);
        }
    }

    pub fn take_cash_delta(&mut self) -> i32 {
        std::mem::take(&mut self.trade.cash_delta)
    }

    pub fn take_info_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.trade.info_urls)
    }

    /// The pointer is over the splitter of the dialogue (or dragging it).
    pub fn splitter_pointer(&self, gui: &Gui) -> bool {
        self.chat.as_ref().is_some_and(|c| c.pointer_over(gui, self.trade.mouse.0, self.trade.mouse.1))
    }
}

impl Play {
    /// Per frame: the prefs the dialogue reads, the docked windows, the items dropped on the trade window, the Cash change and the info pages.
    pub(super) fn interact_trade_frame(&mut self) {
        let Some(i) = self.interact.as_mut() else { return };
        if let Some(h) = self.hud.as_mut() {
            i.trade.show_questions = h.dvalues.prefs.int_any("ShowNPCQuestions").unwrap_or(1) != 0;
            if let Some(c) = i.chat.as_mut() {
                c.show_questions = i.trade.show_questions;
            }
            let (mut rest, drops) = (vec![], h.take_item_drops());
            for (slot, x, y) in drops {
                let low = self.zone.inventory.get(&slot).map(|e| e.item.low_id);
                let taken = i.trade_wants(&self.gui, x, y) && low.is_some_and(|low| {
                    let info = h.item_info(&mut self.gui, low).unwrap_or_default();
                    i.trade_drop(&mut self.gui, slot, x, y, info)
                });
                if !taken {
                    rest.push((slot, x, y));
                }
            }
            h.requeue_item_drops(rest);
        }
        i.trade_sync(&mut self.gui);
        let t = i.trade.win.as_ref().map(|t| t.win);
        if let Some(c) = i.chat.as_mut() {
            c.sync_docks(&mut self.gui, t);
        }
        let d = i.take_cash_delta();
        if d != 0 {
            *self.zone.stats.entry(STAT_CASH).or_insert(0) += d;
        }
        for u in i.take_info_urls() {
            if let Some(c) = self.chat.as_mut() {
                c.show_url(&mut self.gui, &self.zone, &self.text, &u);
            }
        }
        if let Some(c) = self.chat.as_mut() {
            for (url, id, container) in c.take_shop_info_urls() {
                if let Some((item, price)) = i.shop_info(id, container) {
                    c.shop_item_page(&mut self.gui, &self.zone, &self.text, &url, item, price);
                }
            }
        }
    }

    /// The mouse pointer 10 (`GFX_GUI_POINTER_VER_DRAG`, hotspot (6, 16)) over the splitter of the dialogue, drawn like the game's own pointer sprites.
    pub(super) fn interact_pointer(&mut self, host: &mut ao_render::Host, list: &mut DrawList) {
        let Some(i) = self.interact.as_ref() else { return };
        if !i.splitter_pointer(&self.gui) {
            return;
        }
        let Some(id) = self.gui.gfx().id(super::interact_chat::POINTER_GFX) else { return };
        let (w, h) = self.gui.gfx().size(id);
        let (x0, y0) = (i.trade.mouse.0 - super::interact_chat::POINTER_HOTSPOT.0, i.trade.mouse.1 - super::interact_chat::POINTER_HOTSPOT.1);
        list.cmds.push(DrawCmd::Gfx { id, src: [0.0, 0.0, w as f32, h as f32], dst: [x0, y0, x0 + w as f32, y0 + h as f32], tint: [255; 3], alpha: 1.0 });
        host.hide_cursor = true;
    }
}

/// Live-harness helpers (`flow/live.rs`).
#[cfg(test)]
impl Interact {
    /// The trade as the harness prints it.
    pub fn trade_dump(&self, gui: &Gui) -> String {
        match &self.trade.win {
            None => "npc trade: no window".into(),
            Some(t) => format!("npc trade: {}\n  log: {:?}", t.dump(gui), self.log),
        }
    }

    /// Press button `i` of the bar (0 description, 1 info, 2 trade, 3 use); false when it is disabled or there is no dialogue.
    pub fn press_button(&mut self, gui: &mut Gui, i: usize) -> bool {
        let Some(out) = self.chat.as_ref().and_then(|c| c.press(i)) else { return false };
        self.chat_out(gui, out);
        true
    }

    /// Put a resolved inventory slot into the container through the same drop path as the HUD.
    pub fn trade_add(&mut self, gui: &mut Gui, slot: u32, info: super::hud_stats::ItemInfo) -> bool {
        let Some(t) = self.trade.win.as_ref().filter(|t| t.has_room()) else { return false };
        let Some(r) = gui.view_rect(t.win, "items") else { return false };
        self.trade_drop(gui, slot, (r.l + r.r) / 2.0, (r.t + r.b) / 2.0, info)
    }

    /// Press the accept button with `credits` in the credits field.
    pub fn trade_accept(&mut self, gui: &mut Gui, zone: &Zone, credits: i32) -> bool {
        let Some(t) = self.trade.win.as_ref() else { return false };
        gui.set_text(t.win, "credits", &credits.to_string());
        let ev = Event::Clicked { window: t.win, view: "accept".into(), item: None };
        self.trade_event(gui, &ev, zone)
    }

    /// Press the decline button.
    pub fn trade_decline(&mut self, gui: &mut Gui, zone: &Zone) -> bool {
        let Some(t) = self.trade.win.as_ref() else { return false };
        let ev = Event::Clicked { window: t.win, view: "decline".into(), item: None };
        self.trade_event(gui, &ev, zone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::interact_chat::{dock_below, dock_right};
    use crate::play::interact_chat::STAT0_USE_BIT;
    use crate::play::zone::DynelState;
    use ao_gui::InputEvent;
    use ao_net::n3::misc::Misc;
    use ao_net::n3::outgoing::n3_frame;
    use ao_net::n3::{self, N3};

    const OWN: u32 = 0x6584;
    const ME: Identity = Identity { kind: 50000, instance: OWN as i32 };
    const NPC: Identity = Identity { kind: 50000, instance: 4711 };

    fn rig() -> Option<Gui> {
        let client = ao_gui::client_dir();
        client.join("cd_image/gui").exists().then(|| Gui::new(&client, None).unwrap())
    }

    fn zone() -> Zone {
        let mut z = Zone::new(OWN);
        z.dynels.insert(4711, DynelState { name: "Guard".into(), pos: [0.0; 3], yaw: None, npc: true, side: 0, level: 1, health: 5, max_health: 5 });
        z.stats.insert(STAT_CASH, 500);
        z
    }

    fn feed(i: &mut Interact, gui: &mut Gui, z: &Zone, k: Knubot) {
        i.on_frame(gui, &n3_frame(0, OWN, k.encode(ME)), z);
    }

    /// The Knubot messages of the outbox (what the server would receive).
    fn sent(i: &mut Interact) -> Vec<Knubot> {
        i.take_outbox()
            .iter()
            .filter_map(|f| match n3::decode(f).unwrap().body {
                N3::Knubot(k) => Some(k),
                _ => None,
            })
            .collect()
    }

    fn open(gui: &mut Gui, z: &Zone, b20: bool, b21: bool) -> Interact {
        let mut i = Interact::new(OWN, (1280, 800));
        feed(&mut i, gui, z, Knubot::Open { npc: NPC, b20, b21 });
        i
    }

    #[test]
    fn container_grid_follows_the_item_count() {
        assert_eq!(grid(1), (1, 1));
        assert_eq!(grid(2), (2, 1));
        assert_eq!(grid(5), (2, 3));
        assert_eq!(grid(0), (2, 1));
    }

    #[test]
    fn bar_buttons_send_their_messages() {
        let Some(mut gui) = rig() else { return };
        let mut z = zone();
        let mut i = Interact::new(OWN, (1280, 800));
        // stat 0 bit 21 of the NPC enables the fourth button (read when the window is built)
        z.character_stats.entry(4711).or_default().insert(0, STAT0_USE_BIT);
        feed(&mut i, &mut gui, &z, Knubot::Open { npc: NPC, b20: true, b21: false });
        assert!(i.press_button(&mut gui, 0));
        assert!(!i.press_button(&mut gui, 2), "trade needs b21");
        assert!(i.press_button(&mut gui, 1));
        assert!(i.press_button(&mut gui, 3));
        let out = sent(&mut i);
        assert_eq!(out, vec![Knubot::Description { npc: NPC }]);
        assert_eq!(i.take_info_urls(), vec!["charid://50000/4711".to_string()]);
        // the use button: GenericCmd 3 on the NPC (`N3Msg_UseItem`)
        let f = i.take_outbox();
        assert!(f.is_empty(), "use_object was already flushed with the first take: {f:?}");
    }

    #[test]
    fn transcript_links_use_the_existing_info_queue_without_sending_an_answer() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z, true, false);
        i.take_outbox();
        i.chat_out(&mut gui, ChatOut::Link("itemref://53019/53020/1".into()));
        assert_eq!(i.take_info_urls(), ["itemref://53019/53020/1"]);
        assert!(i.take_outbox().is_empty());
    }

    #[test]
    fn use_button_sends_the_use_command() {
        let Some(mut gui) = rig() else { return };
        let mut z = zone();
        let mut i = Interact::new(OWN, (1280, 800));
        z.character_stats.entry(4711).or_default().insert(0, STAT0_USE_BIT);
        feed(&mut i, &mut gui, &z, Knubot::Open { npc: NPC, b20: false, b21: false });
        assert!(i.press_button(&mut gui, 3));
        let f = i.take_outbox();
        assert_eq!(f.len(), 1);
        let N3::Misc(Misc::GenericCmd(c)) = n3::decode(&f[0]).unwrap().body else { panic!("not a GenericCmd") };
        assert_eq!(c.cmd, 3);
    }

    #[test]
    fn trade_round_trip_accept() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z, true, true);
        // the trade button asks the server; the server's StartTrade builds the window
        assert!(i.press_button(&mut gui, 2));
        // (the empty string is what the client writes; the read side of the class rejects it, so compare the frame)
        assert_eq!(i.take_outbox(), vec![n3_frame(0, OWN, knubot::start_trade(ME, NPC))]);
        assert!(i.trade.win.is_none());
        feed(&mut i, &mut gui, &z, Knubot::StartTrade { npc: NPC, value: 3, text: "Sell me <b>stuff</b>".into() });
        let t = i.trade.win.as_ref().expect("trade window");
        assert_eq!(gui.text(t.win, "give_items"), "GIVE ITEMS");
        assert_eq!(gui.text(i.chat.as_ref().unwrap().win, "npc_answers"), "<font color=CCNPCChatTrade>Sell me <b>stuff</b></font>");
        // docked to the right of the chat window (slot 2), the bar below it (slot 5)
        gui.frame(0.0);
        i.chat.as_mut().unwrap().sync_docks(&mut gui, Some(t.win));
        let chat = gui.window_outer_frame(i.chat.as_ref().unwrap().win).unwrap();
        let tw = gui.window_outer_frame(t.win).unwrap();
        assert_eq!((tw.0, tw.1), dock_right(chat));
        assert_eq!(gui.window_outer_frame(i.chat.as_ref().unwrap().bar).map(|b| (b.0, b.1)), Some(dock_below(chat)));
        // a second StartTrade while a bar exists changes nothing (`FUN_10058a4c` tests `this+0x140`)
        feed(&mut i, &mut gui, &z, Knubot::StartTrade { npc: NPC, value: 9, text: "again".into() });
        assert_eq!(i.trade.win.as_ref().unwrap().max, 3);
        // items: three fit, the fourth is refused
        let item = |n| Identity { kind: 0x68, instance: 0x40 + n };
        for n in 0..3 {
            assert!(i.trade_add(&mut gui, (0x40 + n) as u32, Default::default()));
        }
        assert!(!i.trade_add(&mut gui, 0x43, Default::default()));
        assert_eq!(sent(&mut i).len(), 3);
        assert!(i.trade_dump(&gui).contains("max 3"));
        // accept with more credits than Cash: clamped to Cash, the field is rewritten, Cash drops at once
        assert!(i.trade_accept(&mut gui, &z, 900));
        assert_eq!(sent(&mut i), vec![Knubot::FinishTrade { npc: NPC, flag: false, value: 500 }]);
        assert_eq!(gui.text(i.trade.win.as_ref().unwrap().win, "credits"), "500");
        assert_eq!(i.take_cash_delta(), -500);
        // the server ends the trade: credits back, window gone, last text type 3
        feed(&mut i, &mut gui, &z, Knubot::RejectedItems { npc: NPC, items: vec![(item(0), 1, 2)], value: 120 });
        assert!(i.trade.win.is_none());
        assert_eq!(i.take_cash_delta(), 120);
        assert_eq!(i.trade.rejected.len(), 1);
        assert_eq!(i.chat.as_ref().unwrap().text.last, 3);
        assert_eq!(i.trade_dump(&gui).lines().next(), Some("npc trade: no window"));
    }

    #[test]
    fn decline_and_the_trade_button_cancel() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z, true, true);
        feed(&mut i, &mut gui, &z, Knubot::StartTrade { npc: NPC, value: 1, text: "x".into() });
        assert_eq!(gui.text(i.trade.win.as_ref().unwrap().win, "give_items"), "GIVE ITEM");
        assert!(i.trade_decline(&mut gui, &z));
        assert_eq!(sent(&mut i), vec![Knubot::FinishTrade { npc: NPC, flag: true, value: 0 }]);
        assert!(i.trade.win.is_none());
        assert_eq!(i.take_cash_delta(), 0);
        // the same through the bar's trade button while the trade runs
        feed(&mut i, &mut gui, &z, Knubot::StartTrade { npc: NPC, value: 1, text: "x".into() });
        assert!(i.press_button(&mut gui, 2));
        assert_eq!(sent(&mut i), vec![Knubot::FinishTrade { npc: NPC, flag: true, value: 0 }]);
        assert!(i.trade.win.is_none());
    }

    #[test]
    fn drops_on_the_container_add_and_remove_items() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z, true, true);
        feed(&mut i, &mut gui, &z, Knubot::StartTrade { npc: NPC, value: 2, text: "x".into() });
        gui.frame(0.0);
        let r = gui.view_rect(i.trade.win.as_ref().unwrap().win, "items").unwrap();
        let (x, y) = (r.l + 10.0, r.t + 10.0);
        assert!(i.trade_wants(&gui, x, y));
        assert!(!i.trade_wants(&gui, r.l - 100.0, r.t));
        let client = ao_gui::client_dir();
        if !client.join("cd_image/rdb.db").exists() { return; }
        let mut items = super::super::hud_stats::items::Items::new(&client);
        let info = items.info(&mut gui, 248323).map(|i| (i.name.clone(), i.icon)).unwrap();
        assert_eq!(info.0, "Spinal Section");
        let icon = info.1.expect("retail Spinal Section icon");
        assert!(i.trade_add(&mut gui, 0x40, info.clone()));
        assert_eq!(i.trade.win.as_ref().unwrap().items[0].name, "Spinal Section");
        assert!(gui.frame(0.0).cmds.iter().any(|cmd| matches!(cmd, DrawCmd::Gfx { id, .. } if *id == icon.0)));
        assert_eq!(sent(&mut i), vec![Knubot::Trade { npc: NPC, op: 0, a: Identity::default(), b: Identity { kind: 0x68, instance: 0x40 } }]);
        // the same item twice is not added again; the drop is still this window's
        assert!(i.trade_drop(&mut gui, 0x40, x, y, info));
        assert!(sent(&mut i).is_empty());
        // a double click on the first cell takes it out again
        let win = i.trade.win.as_ref().unwrap().win;
        for button in [MouseButton::Left, MouseButton::Left] {
            gui.input(InputEvent::MouseMove { x: r.l + 5.0, y: r.t + 5.0 });
            let mut evs = gui.input(InputEvent::MouseDown { x: r.l + 5.0, y: r.t + 5.0, button });
            evs.extend(gui.input(InputEvent::MouseUp { x: r.l + 5.0, y: r.t + 5.0, button }));
            for e in evs {
                i.event(&mut gui, &e, &z);
            }
            let _ = win;
        }
        assert_eq!(sent(&mut i), vec![Knubot::Trade { npc: NPC, op: 1, a: Identity::default(), b: Identity { kind: 0x68, instance: 0x40 } }]);
        assert!(i.trade.win.as_ref().unwrap().items.is_empty());
    }

    #[test]
    fn closing_the_dialogue_closes_the_trade_window() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z, true, true);
        feed(&mut i, &mut gui, &z, Knubot::StartTrade { npc: NPC, value: 1, text: "x".into() });
        let (bar, tw) = (i.chat.as_ref().unwrap().bar, i.trade.win.as_ref().unwrap().win);
        assert!(gui.window_visible(bar) && gui.window_visible(tw));
        feed(&mut i, &mut gui, &z, Knubot::Close { npc: NPC, value: 0, text: String::new() });
        assert!(i.chat.is_none() && i.trade.win.is_none());
        assert!(!gui.window_visible(bar) && !gui.window_visible(tw), "both windows are closed");
    }

    #[test]
    fn window_close_button_sends_close_and_removes_the_trade_window() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z, true, true);
        feed(&mut i, &mut gui, &z, Knubot::StartTrade { npc: NPC, value: 1, text: "x".into() });
        let win = i.chat.as_ref().unwrap().win;
        assert!(i.event(&mut gui, &Event::CloseRequested { window: win }, &z));
        assert_eq!(sent(&mut i), vec![Knubot::Close { npc: NPC, value: 0, text: String::new() }]);
        assert!(i.chat.is_none() && i.trade.win.is_none());
    }

    /// Offscreen render of the dialogue (answer bullets, button bar, trade window): `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac npc_dialogue_render`.
    #[test]
    fn npc_dialogue_render() {
        let Some(dir) = std::env::var_os("AOMAC_SHOT_DIR") else { return };
        let Some(mut gui) = rig() else { return };
        let z = zone();
        gui.set_screen_size(1000, 760);
        let texts = ChatTexts { tips: ["Request description".into(), "Request info".into(), "Give items".into(), "Shop".into()] };
        let mut i = Interact::new(OWN, (1000, 760));
        i.trade.texts = texts;
        feed(&mut i, &mut gui, &z, Knubot::Open { npc: NPC, b20: true, b21: true });
        feed(&mut i, &mut gui, &z, Knubot::AppendText { npc: NPC, kind: 0, text: "Welcome, stranger. What can I do for you today?".into() });
        feed(&mut i, &mut gui, &z, Knubot::AnswerList { npc: NPC, answers: vec!["Where am I?".into(), "I would like to trade a very long answer that has to wrap over several lines of the answer list.".into(), "Goodbye".into()] });
        i.chat.as_mut().unwrap().sync_docks(&mut gui, None);
        struct Fe(Gui);
        impl ao_render::Frontend for Fe {
            fn gui(&self) -> &Gui {
                &self.0
            }
            fn input(&mut self, ev: InputEvent, _: &mut ao_render::Host) {
                self.0.input(ev);
            }
            fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut ao_render::Host) -> ao_gui::DrawList {
                self.0.frame(dt)
            }
        }
        let mut fe = Fe(gui);
        let mut o = ao_render::Offscreen::new(&fe, (1000, 760)).unwrap();
        let list = o.frame(&mut fe, 0.016);
        std::fs::create_dir_all(&dir).unwrap();
        o.png(&fe, &list, &std::path::Path::new(&dir).join("npc_answers.png")).unwrap();
        // the trade: the text replaces the answers, the trade window docks to the right
        feed(&mut i, &mut fe.0, &z, Knubot::StartTrade { npc: NPC, value: 3, text: "Give me 3 items and some credits".into() });
        let mut items = super::super::hud_stats::items::Items::new(&ao_gui::client_dir());
        let info = items.info(&mut fe.0, 248323).map(|i| (i.name.clone(), i.icon)).unwrap();
        assert_eq!(info.0, "Spinal Section");
        assert!(info.1.is_some());
        assert!(i.trade_add(&mut fe.0, 0x40, info));
        i.chat.as_mut().unwrap().sync_docks(&mut fe.0, i.trade.win.as_ref().map(|t| t.win));
        let list = o.frame(&mut fe, 0.016);
        o.png(&fe, &list, &std::path::Path::new(&dir).join("npc_trade.png")).unwrap();
    }
}
