//! The player-to-player trade: `N3Msg_TradeStart` and the `TradeView_c` window (GUI.dll, `PlayerTrade` of `Views/TradeGUI.xml`). Evidence: docs/zone/interact.md,
//! "Player trade". The wire codec is [`ao_net::n3::trade`].
//!
//! The client's model (Gamecode `Trade_c`, `FUN_100674ab` dispatcher) is driven by what the server sends; the window only asks:
//! * default action on a character (not talkable, own character not fighting, target in range): `TradeIIR_t` op 0 ([`Interact::trade_action`]);
//! * the server's op 0 opens the window (`GlobalSignals +0xd4` -> `InventoryGUIModule_c::SlotStartTrade` [GUI 0x100c6ff4]), an ignored partner is answered with
//!   `TradeAbort(false)`;
//! * an item dragged from the inventory onto the own items -> `TradeAddItem`, double click on an own item -> `TradeRemoveItem`; the lists change when the
//!   server's op 5 / 6 arrive;
//! * the own cash field sends `TradeSetCash` one second after the last edit (`EventTimer_c` `FUN_100df942`);
//! * Accept sends `TradeConfirm` (op 3), the server's op 3 raises the "Are you sure you want to complete this trade?" box (`FUN_100e066c`), Yes sends
//!   `TradeAccept`, No `TradeAbort(true)`; Decline sends `TradeAbort(true)`; op 2 / 4 close the window.

use super::hud_dialog::Dialogs;
use super::hud_stats::ItemInfo;
use super::hud::group;
use super::interact::{Action, Interact};
use super::zone::Zone;
use super::Play;
use ao_gui::{CanvasItem, CanvasTip, Event, GfxId, Gui, MouseButton, WindowId, WindowSize};
use ao_net::msg::Identity;
use ao_net::n3::inventory::BAG_FIRST;
use ao_net::n3::outgoing::DYNEL_CHAR;
use ao_net::n3::trade::{self, Trade};
use ao_net::n3::world::AcgItem;

/// Client rectangle of the `TradeView_c`: its preferred `Rect(0, 0, _DAT_101bfa98 = 119.0, _DAT_101bfa9c = 452.0)` [GUI 0x100e092f] is 120 x 453, but the view
/// docks into the 192 px `RollupArea` (the buttons row alone is wider than 120 px): the free window takes the dock's width ([INFERENCE]).
pub const CLIENT: (u32, u32) = (super::hud_rollup::AREA_W, 453);
/// `FUN_10059ca0` [GC]: the surface distance between both characters must be `<= _DAT_101574fc` = 5.0 m.
pub const RANGE: f32 = 5.0;
/// The bounding sphere radius of a character (`n3VisualDynel_t::GetBoundingSphereRadius`) is not known per model: [GUESS] the body collision default 0.5 m for both.
const RADIUS: f32 = 0.5;
/// The own cash field is sent this long after the last edit: `EventTimer_c::Start(0xf4240, 0, 1)` [GUI 0x100df82d] ([INFERENCE]: microseconds).
pub const CASH_DELAY: f32 = 1.0;
/// Option `esc_trades` (`LoginPrefs.xml`: true): Esc declines the trade (`FUN_100df810`).
const ESC_TRADES: bool = true;
/// Cell art of the item lists (`MultiListView_c` 48 px icons in 54 px slots, hud_stats/items.rs).
const SLOT: f32 = 54.0;
const ICON: f32 = 48.0;
const SLOT_GFX: &str = "GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED";
/// Columns of the item lists: three 54 px slots fit the 192 px page ([INFERENCE]: the list views are `SetViewCellCounts((1, 1), (1000, ..))` grids that fill the dock's width).
const COLS: usize = 3;
/// `BitmapView_c::AddBitmap(0x15b)`, `AddBitmap(0xd9)` of `PartnerStatusDock` [GUI 0x100e039a]: index 0 / 1 = not accepted / accepted.
const STATUS_GFX: [u32; 2] = [0x15b, 0xd9];

/// `FUN_100e066c` [GUI]: the literal English text of the confirmation box (a `DialogBox_c` with the title "Trade").
const CONFIRM_TEXT: &str = "Are you sure you want to complete this trade?";

const ZERO: Identity = Identity { kind: 0, instance: 0 };

/// One item of a trade list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// `{0x68, bag slot}` for the own items; whatever the server names for the partner's.
    pub item: Identity,
    /// The template; the own items are read from the inventory when the server adds them, the partner's only when the server tells us (UNRESOLVED).
    pub acg: Option<AcgItem>,
}

/// The open window and its model (`TradeView_c`: `+0x1c8` partner, `+0x1d0` own).
pub struct PTrade {
    pub win: WindowId,
    pub own: Identity,
    pub partner: Identity,
    pub ours: Vec<Entry>,
    pub theirs: Vec<Entry>,
    /// The status bitmap of `PartnerStatusDock` (`+0x1f8`).
    pub accepted: bool,
    pub partner_cash: i32,
    pub cash: i32,
    /// `[ours, theirs]` first visible row.
    scroll: [usize; 2],
    dock: (u32, u32),
    /// When the own cash field is sent (the 1 s `EventTimer_c` of `+0x200`).
    cash_due: Option<f32>,
    dirty: bool,
}

#[derive(Default)]
pub struct PTradeUi {
    pub(super) trade: Option<PTrade>,
    /// A server op 0 that the frame hook still has to open (the ignore list lives in the chat).
    pending: Option<Identity>,
    confirm: Dialogs<()>,
    buttons: [String; 2],
    feedback: Vec<&'static str>,
    now: f32,
    /// Every step, for the live harness.
    pub log: Vec<String>,
}

/// The one-row-at-a-time geometry of a list: slots `(cell index) -> top-left`.
fn cell(n: usize) -> (f32, f32) {
    ((n % COLS) as f32 * SLOT, (n / COLS) as f32 * SLOT)
}

fn atol(s: &str) -> i32 {
    let d: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    d.parse::<i64>().map_or(0, |v| v.min(i32::MAX as i64) as i32)
}

/// Height of one dock so that the whole view is [`CLIENT`] high: the view's fixed rows are measured with empty docks.
fn dock_height(gui: &mut Gui) -> anyhow::Result<u32> {
    let w = gui.open_window("TradeGUI", (0, 0), WindowSize::Preferred)?;
    let h = gui.window_size(w).1;
    gui.close_window(w);
    Ok(CLIENT.1.saturating_sub(h) / 2)
}

impl PTrade {
    /// `InventoryGUIModule_c::SlotStartTrade` -> `TradeView_c` ctor [GUI 0x100e092f] for mode 0 (`PlayerTrade`). The view is a `DockableView_c` titled "Trade"
    /// that docks into `RollupArea`; [UNRESOLVED] it is shown here as a free window left of the rollup column.
    pub fn open(gui: &mut Gui, screen: (u32, u32), own: Identity, partner: Identity, name: &str) -> anyhow::Result<PTrade> {
        let dh = dock_height(gui)?;
        let dw = (COLS as f32 * SLOT) as u32;
        let pos = ((screen.0 as i32 - super::hud_rollup::AREA_W as i32 - CLIENT.0 as i32 - 20).max(0), 40);
        let win = gui.open_tabbed_window("TradeGUI", "Trade", pos, WindowSize::Fixed(CLIENT.0, CLIENT.1))?;
        let dock = |c: &str| format!("<root><CanvasView name=\"{c}\" min_size=\"Point({dw},{dh})\" max_size=\"Point({dw},{dh})\"/></root>");
        gui.add_view_xml(win, "PartnerInventoryDock", "partner_items", &dock("partner_items"))?;
        gui.add_view_xml(win, "OurInventoryDock", "our_items", &dock("our_items"))?;
        let (sw, sh) = STATUS_GFX.iter().map(|&g| gui.gfx().size(GfxId(g))).fold((0, 0), |a, s| (a.0.max(s.0), a.1.max(s.1)));
        let status = format!("<root><CanvasView name=\"status\" min_size=\"Point({sw},{sh})\" max_size=\"Point({sw},{sh})\"/></root>");
        gui.add_view_xml(win, "PartnerStatusDock", "status", &status)?;
        // `FUN_100e039a`: the partner's cash starts as "0" (read-only, green 0x44dd44), the own field as the empty text of the XML
        gui.set_text(win, "PartnerName", name);
        gui.set_text(win, "PartnerCashView", "0");
        gui.relayout_window(win);
        Ok(PTrade { win, own, partner, ours: vec![], theirs: vec![], accepted: false, partner_cash: 0, cash: 0, scroll: [0; 2], dock: (dw, dh), cash_due: None, dirty: true })
    }

    /// Paints both lists and the status bitmap when something changed. `info(gui, low_id)` = name and icon of a template.
    pub fn render(&mut self, gui: &mut Gui, info: &mut dyn FnMut(&mut Gui, i32) -> Option<ItemInfo>) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let slot = gui.gfx_id(SLOT_GFX).map(GfxId);
        let rows = (self.dock.1 as f32 / SLOT) as usize;
        for (i, (view, list)) in [("our_items", &self.ours), ("partner_items", &self.theirs)].into_iter().enumerate() {
            let (mut cmds, mut tips) = (vec![], vec![]);
            let first = self.scroll[i] * COLS;
            for n in 0..rows * COLS {
                let (x, y) = cell(n);
                if let Some(g) = slot {
                    cmds.push(CanvasItem::Image { id: g, src: [0.0, 0.0, SLOT, SLOT], dst: [x, y, x + SLOT, y + SLOT], alpha: 1.0 });
                }
                let Some(e) = list.get(first + n) else { continue };
                let Some((name, icon)) = e.acg.and_then(|a| info(gui, a.low_id)) else { continue };
                if let Some((id, w, h)) = icon {
                    let o = (SLOT - ICON) / 2.0;
                    cmds.push(CanvasItem::Image { id, src: [0.0, 0.0, w as f32, h as f32], dst: [x + o, y + o, x + o + ICON, y + o + ICON], alpha: 1.0 });
                }
                tips.push(CanvasTip { rect: [x, y, x + SLOT, y + SLOT], title: name, body: String::new() });
            }
            gui.set_canvas(self.win, view, cmds);
            gui.set_canvas_tips(self.win, view, tips);
        }
        let s = STATUS_GFX[usize::from(self.accepted)];
        let (w, h) = gui.gfx().size(GfxId(s));
        gui.set_canvas(self.win, "status", vec![CanvasItem::Image { id: GfxId(s), src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, w as f32, h as f32], alpha: 1.0 }]);
    }

    /// The list view a canvas name stands for: 0 = own items, 1 = the partner's.
    fn list_of(view: &str) -> Option<usize> {
        match view {
            "our_items" => Some(0),
            "partner_items" => Some(1),
            _ => None,
        }
    }

    fn entries(&self, list: usize) -> &Vec<Entry> {
        if list == 0 { &self.ours } else { &self.theirs }
    }

    /// The entry under a click at `(x, y)` of a list canvas.
    fn entry_at(&self, list: usize, x: f32, y: f32) -> Option<usize> {
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (c, r) = ((x / SLOT) as usize, (y / SLOT) as usize);
        let i = (self.scroll[list] + r) * COLS + c;
        (c < COLS && i < self.entries(list).len()).then_some(i)
    }

    /// `GetStat` of the bag items the own list shows: the inventory slot of an own entry.
    fn slot_of(e: &Entry) -> u32 {
        e.item.instance as u32
    }

    /// The own bag has a free slot once the own trade items are counted as gone (`FUN_1002a1b0(0x40)`: the original moves a traded item out of the bag).
    fn bag_has_room(&self, zone: &Zone) -> bool {
        (BAG_FIRST..BAG_FIRST + ao_net::n3::inventory::BAG_SLOTS).any(|s| !zone.inventory.contains_key(&s) || self.ours.iter().any(|e| Self::slot_of(e) == s))
    }
}

impl PTradeUi {
    pub fn set_texts(&mut self, yes: String, no: String) {
        self.buttons = [yes, no];
    }

    pub fn close_all(&mut self, gui: &mut Gui) {
        self.confirm.close_all(gui);
        if let Some(t) = self.trade.take() {
            gui.close_window(t.win);
        }
        self.pending = None;
    }

    pub fn take_feedback(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.feedback)
    }
}

impl Interact {
    /// `N3Msg_TradeStart(target)` [GC 0x100190e0], the character branch of `N3Msg_DefaultActionOnDynel` [GC 0x100291da] when the target is not talkable:
    /// outside a fight, not the own character, a `SimpleChar_t` without `TowerType` (stat 0x184, not tracked: assumed 0) and within [`RANGE`]
    /// (`FUN_10059ca0`; the dungeon door test of its tail is not ported), else `Feedback_TargetOutsideRangeForTrade`. Sends `TradeIIR_t` op 0 (target, 0).
    pub fn trade_action(&mut self, zone: &Zone, id: i32) -> Action {
        let own = self.own as i32;
        if id == own || zone.fight_target.contains_key(&own) {
            return Action::None;
        }
        let (Some(me), Some(them)) = (zone.dynels.get(&own), zone.dynels.get(&id)) else { return Action::None };
        let d = me.pos.iter().zip(them.pos.iter()).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt();
        if d - 2.0 * RADIUS > RANGE {
            self.ptrade.feedback.push("Feedback_TargetOutsideRangeForTrade");
            return Action::Refused("Feedback_TargetOutsideRangeForTrade");
        }
        let (own_id, target) = (self.own_id(), Identity { kind: DYNEL_CHAR, instance: id });
        self.send(trade::start(own_id, target));
        Action::Trade
    }

    /// The `TradeIIR_t` the server sent; `who` is its header identity (the character the state belongs to). Handlers: `FUN_100674ab` [GC] and what it calls.
    pub(super) fn on_trade(&mut self, gui: &mut Gui, t: Trade, who: Identity, zone: &Zone) {
        let me = self.own_id();
        let ui = &mut self.ptrade;
        ui.log.push(format!("{who:?} {t:?}"));
        match t.op {
            // `FUN_100663e4`: only the client character's own state; an open trade -> `Feedback_YouAreAlreadyInATrade`; `b != 0` is a shop / vending machine (not ported)
            trade::START if who == me && t.b == ZERO => {
                if ui.trade.is_some() || ui.pending.is_some() {
                    ui.feedback.push("Feedback_YouAreAlreadyInATrade");
                } else {
                    ui.pending = Some(t.a);
                }
            }
            trade::ACCEPT | trade::RESET if who == me => {
                if let Some(p) = ui.trade.as_mut() {
                    // `FUN_10066598` -> `+0xe0(1)`; `FUN_1006741b` -> `+0xe0(0)`: `FUN_100df971`
                    p.accepted = t.op == trade::ACCEPT;
                    p.cash_due = None;
                    p.dirty = true;
                    if t.op == trade::RESET {
                        gui.set_enabled(p.win, "AcceptButton", true);
                        ui.confirm.close_all(gui);
                    } else {
                        let c = group(atol(&gui.text(p.win, "OurCashView")));
                        gui.set_text(p.win, "OurCashView", &c);
                    }
                }
            }
            // `FUN_100673a0` -> `+0xe8` -> `FUN_100e066c`
            trade::CONFIRM if who == me => {
                if ui.trade.is_some() && !ui.confirm.is_open() {
                    let [yes, no] = ui.buttons.clone();
                    ui.confirm.go(gui, self.screen, (), CONFIRM_TEXT, &[yes, no]);
                }
            }
            // `FUN_100666a3`: either side's abort ends the trade; the cancel notice is shown when the own state had `a.instance != 0` (or the partner aborted)
            trade::ABORT => {
                let Some(p) = ui.trade.as_ref().filter(|p| who == me || who == p.partner) else { return };
                let flag = t.a.instance != 0;
                if who == p.partner || flag {
                    ui.feedback.push("Feedback_TradeCancelled");
                }
                self.ptrade_end(gui);
            }
            // `FUN_100668d1`: the items change hands, `+0xe4(1)` closes the window
            trade::COMPLETE => {
                if ui.trade.as_ref().is_some_and(|p| who == me || who == p.partner) {
                    self.ptrade_end(gui);
                }
            }
            trade::ADD_ITEM | trade::REMOVE_ITEM => {
                let Some(p) = ui.trade.as_mut() else { return };
                let (mine, theirs) = (t.a == p.own, t.a == p.partner);
                if !(mine || theirs) {
                    return;
                }
                let list = if mine { &mut p.ours } else { &mut p.theirs };
                if t.op == trade::ADD_ITEM {
                    let acg = mine.then(|| zone.inventory.get(&(t.b.instance as u32)).map(|e| e.item)).flatten();
                    list.push(Entry { item: t.b, acg });
                } else if let Some(i) = list.iter().position(|e| e.item == t.b) {
                    list.remove(i);
                }
                p.dirty = true;
            }
            // `FUN_100672c7`: `cash >= 0`; only the partner's value is shown (`+0xdc` -> `FUN_100dfe5e`)
            trade::SET_CASH if t.a.instance >= 0 => {
                let Some(p) = ui.trade.as_mut() else { return };
                if who == p.partner {
                    p.partner_cash = t.a.instance;
                    gui.set_text(p.win, "PartnerCashView", &group(t.a.instance));
                } else if who == p.own {
                    p.cash = t.a.instance;
                }
            }
            _ => {}
        }
    }

    /// The window is gone (`+0xe4`): also the confirmation box.
    fn ptrade_end(&mut self, gui: &mut Gui) {
        self.ptrade.confirm.close_all(gui);
        if let Some(p) = self.ptrade.trade.take() {
            gui.close_window(p.win);
        }
    }

    /// The partner's list as the server sends it as an `InventoryUpdateIIR_t` of the partner's identity ([INFERENCE]: the contents of the partner's trade
    /// container reach the client as `N3Msg_TradeGetInventory(partner)`'s list; which message carries it was not seen live).
    pub(super) fn ptrade_inventory(&mut self, m: &ao_net::n3::inventory::InventoryMsg) {
        let ao_net::n3::inventory::InventoryMsg::Update(u) = m else { return };
        let Some(p) = self.ptrade.trade.as_mut().filter(|p| p.partner == u.container) else { return };
        p.theirs = u.entries.iter().map(|e| Entry { item: e.id, acg: Some(e.item) }).collect();
        p.dirty = true;
    }

    /// GUI events of the trade window and its confirmation box; `true` when consumed.
    pub(super) fn ptrade_event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let me = self.own_id();
        let ui = &mut self.ptrade;
        let (mine, answer) = ui.confirm.event(gui, ev);
        if mine {
            // `FUN_100df843`: 0 = `TradeAccept`, 1 = decline; Esc (-1) does neither
            match answer.map(|(_, b)| b) {
                Some(0) => self.send(trade::accept(me)),
                Some(1) => self.send(trade::abort(me, true)),
                _ => {}
            }
            return true;
        }
        let Some(p) = ui.trade.as_mut() else { return false };
        let now = ui.now;
        let mut out: Vec<Vec<u8>> = vec![];
        match ev {
            // `FUN_100dfd33`: pending cash goes out first, the field is shown formatted, Accept is disabled, the status shows "accepted", `TradeConfirm`
            Event::Clicked { window, view, .. } if *window == p.win && view == "AcceptButton" => {
                let text = gui.text(p.win, "OurCashView");
                if p.cash_due.take().is_some() {
                    p.cash = atol(&text);
                    out.push(trade::set_cash(me, p.cash));
                }
                gui.set_text(p.win, "OurCashView", &group(atol(&text)));
                gui.set_enabled(p.win, "AcceptButton", false);
                p.accepted = true;
                p.dirty = true;
                out.push(trade::confirm(me));
            }
            // `FUN_100df810`
            Event::Clicked { window, view, .. } if *window == p.win && view == "DeclineButton" => {
                p.cash_due = None;
                out.push(trade::abort(me, true));
            }
            Event::Escape { window } | Event::CloseRequested { window } if *window == p.win => {
                if matches!(ev, Event::CloseRequested { .. }) || ESC_TRADES {
                    p.cash_due = None;
                    out.push(trade::abort(me, true));
                }
            }
            // the own cash field: digits only ([GUESS] for the feature flag 0x2000 of `FUN_100e039a`), sent after `CASH_DELAY`
            Event::TextChanged { window, view, text } if *window == p.win && view == "OurCashView" => {
                let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                if digits != *text {
                    gui.set_text(p.win, "OurCashView", &digits);
                }
                p.cash_due = Some(now + CASH_DELAY);
            }
            Event::TextChanged { window, .. } if *window == p.win => {}
            // double click on an own item: `FUN_100df67e` -> `N3Msg_TradeRemoveItem(own, item)`, refused with `Feedback_NoRoomInInventory` when the bag is full
            Event::CanvasPress { window, view, x, y, button: MouseButton::Left, clicks: 2 } if *window == p.win => {
                let Some(list) = PTrade::list_of(view) else { return true };
                if list == 0 {
                    if let Some(e) = p.entry_at(0, *x, *y).map(|i| p.ours[i].clone()) {
                        if p.bag_has_room(zone) {
                            out.push(trade::remove_item(me, me, e.item));
                        } else {
                            ui.feedback.push("Feedback_NoRoomInInventory");
                        }
                    }
                }
            }
            Event::CanvasWheel { window, view, dy, .. } if *window == p.win => {
                if let Some(l) = PTrade::list_of(view) {
                    let max = p.entries(l).len().div_ceil(COLS).saturating_sub((p.dock.1 as f32 / SLOT) as usize);
                    p.scroll[l] = (p.scroll[l] as i64 - *dy as i64).clamp(0, max as i64) as usize;
                    p.dirty = true;
                }
            }
            Event::CanvasPress { window, .. } | Event::CanvasClick { window, .. } | Event::CanvasRelease { window, .. } | Event::CanvasDrag { window, .. } if *window == p.win => {}
            _ => return false,
        }
        out.into_iter().for_each(|o| self.send(o));
        true
    }

    /// An inventory item released at `(x, y)`: when it is over the own item list `FUN_100df870` accepts the drag object (`inventory/item`, no `split_count`)
    /// and sends `N3Msg_TradeAddItem(own, item)` [GC 0x100191f6]. The engine's refusals that need item flags (`Feedback_*` of the `vtable+0x14` test) are UNRESOLVED.
    fn ptrade_drop(&mut self, gui: &Gui, zone: &Zone, slot: u32, x: f32, y: f32) {
        let me = self.own_id();
        let Some(p) = self.ptrade.trade.as_ref() else { return };
        let Some(r) = gui.view_rect(p.win, "our_items") else { return };
        if x < r.l || x > r.r || y < r.t || y > r.b {
            return;
        }
        let item = ao_net::n3::inventory::item_identity(slot);
        if slot < BAG_FIRST || !zone.inventory.contains_key(&slot) || p.ours.iter().any(|e| e.item == item) {
            return;
        }
        self.send(trade::add_item(me, item));
    }
}

impl Interact {
    /// A server op 0 waits for the frame hook: the partner is checked against the ignore list (`SlotStartTrade`: `IgnoreSystem_t::IsCharacterIgnored` ->
    /// `N3Msg_TradeAbort(false)`), else the window opens.
    pub(super) fn ptrade_start(&mut self, gui: &mut Gui, zone: &Zone, ignored: impl Fn(u32) -> bool) {
        let Some(partner) = self.ptrade.pending.take() else { return };
        let me = self.own_id();
        if ignored(partner.instance as u32) {
            self.send(trade::abort(me, false));
            return;
        }
        let name = zone.dynels.get(&partner.instance).map(|d| d.name.clone()).unwrap_or_default();
        match PTrade::open(gui, self.screen, me, partner, &name) {
            Ok(t) => self.ptrade.trade = Some(t),
            Err(e) => eprintln!("interact: trade window: {e:#}"),
        }
    }

    /// The own cash field goes out [`CASH_DELAY`] after its last edit (`FUN_100df942`: `TradeSetCash(atol(text))`).
    pub(super) fn ptrade_tick(&mut self, gui: &Gui, now: f32) {
        self.ptrade.now = now;
        let me = self.own_id();
        let Some(p) = self.ptrade.trade.as_mut().filter(|p| p.cash_due.is_some_and(|d| d <= now)) else { return };
        p.cash_due = None;
        p.cash = atol(&gui.text(p.win, "OurCashView"));
        let cash = p.cash;
        self.send(trade::set_cash(me, cash));
    }

    /// The inventory items released over a window: those over the own item list are added (see [`Interact::ptrade_drop`]).
    pub(super) fn ptrade_drops(&mut self, gui: &Gui, zone: &Zone, drops: Vec<(u32, f32, f32)>) {
        for (slot, x, y) in drops {
            self.ptrade_drop(gui, zone, slot, x, y);
        }
    }
}

impl Play {
    /// Per frame, after [`Play::interact_frame`]: opens a trade the server started, sends the due cash, takes the items dropped on the window, repaints it.
    pub(super) fn interact_ptrade_frame(&mut self) {
        let Some(i) = self.interact.as_mut() else { return };
        let chat = self.chat.as_ref();
        i.ptrade_start(&mut self.gui, &self.zone, |id| chat.is_some_and(|c| c.is_ignored(id)));
        i.ptrade_tick(&self.gui, self.time);
        if let Some(h) = self.hud.as_mut() {
            i.ptrade_drops(&self.gui, &self.zone, h.take_item_drops());
            if let Some(p) = i.ptrade.trade.as_mut() {
                p.render(&mut self.gui, &mut |g, low| h.item_info(g, low));
            }
        }
        for f in i.take_outbox() {
            if let Some(s) = &self.session {
                s.send_zone(f);
            }
        }
    }
}

/// Live-harness helpers (`flow/live.rs`).
#[cfg(test)]
impl Interact {
    /// The trade window as the harness prints it.
    pub fn ptrade_dump(&self, gui: &Gui) -> String {
        let Some(p) = self.ptrade.trade.as_ref() else { return "trade: no window".into() };
        let list = |l: &[Entry]| l.iter().map(|e| format!("{}:{}{}", e.item.kind, e.item.instance, e.acg.map_or(String::new(), |a| format!(" #{}", a.low_id)))).collect::<Vec<_>>().join(", ");
        format!(
            "trade with {:?} ({:?})\n  ours: [{}]\n  theirs: [{}]\n  cash: ours {} (field {:?}) partner {} (field {:?})\n  accepted: {} accept enabled: {}\n  log: {:?}",
            gui.text(p.win, "PartnerName"),
            p.partner,
            list(&p.ours),
            list(&p.theirs),
            p.cash,
            gui.text(p.win, "OurCashView"),
            p.partner_cash,
            gui.text(p.win, "PartnerCashView"),
            p.accepted,
            gui.is_enabled(p.win, "AcceptButton"),
            self.ptrade.log
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::zone::DynelState;
    use ao_net::frame::Frame;
    use ao_net::n3::{self, world::InventoryEntry, N3};

    const OWN: i32 = 0x6584;
    const ME: Identity = Identity { kind: DYNEL_CHAR, instance: OWN };
    const BOB: Identity = Identity { kind: DYNEL_CHAR, instance: 77 };

    fn rig() -> Option<Gui> {
        let client = ao_gui::client_dir();
        client.join("cd_image/gui").exists().then(|| Gui::new(&client, None).unwrap())
    }

    fn dyn_at(name: &str, x: f32) -> DynelState {
        DynelState { name: name.into(), pos: [x, 0.0, 0.0], yaw: None, npc: false, side: 0, level: 1, health: 5, max_health: 5 }
    }

    fn zone() -> Zone {
        let mut z = Zone::new(OWN as u32);
        z.dynels.insert(OWN, dyn_at("Me", 0.0));
        z.dynels.insert(77, dyn_at("Bob", 5.5));
        z.dynels.insert(78, dyn_at("Far", 6.5));
        for (slot, low) in [(0x40, 111), (0x41, 222)] {
            z.inventory.insert(slot, InventoryEntry { slot, a: 0, b: 0, id: Identity { kind: 0x68, instance: slot as i32 }, item: AcgItem { low_id: low, high_id: low, level: 5 } });
        }
        z
    }

    /// The `TradeIIR_t`s of the outbox.
    fn sent(i: &mut Interact) -> Vec<Trade> {
        i.take_outbox()
            .iter()
            .map(|f: &Frame| match n3::decode(f).unwrap().body {
                N3::Trade(t) => t,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn msg(op: i8, a: Identity, b: Identity) -> Trade {
        Trade { op, a, b }
    }

    /// A trade window with Bob, opened through the server's op 0.
    fn open(gui: &mut Gui, z: &Zone) -> Interact {
        let mut i = Interact::new(OWN as u32, (1280, 800));
        i.on_trade(gui, msg(trade::START, BOB, ZERO), ME, z);
        i.ptrade_start(gui, z, |_| false);
        i
    }

    fn click(i: &mut Interact, gui: &mut Gui, z: &Zone, view: &str) {
        let window = i.ptrade.trade.as_ref().unwrap().win;
        assert!(i.event(gui, &Event::Clicked { window, view: view.into(), item: None }, z), "{view}");
    }

    #[test]
    fn default_action_starts_a_trade_only_within_range_and_outside_fights() {
        let mut z = zone();
        let mut i = Interact::new(OWN as u32, (1280, 800));
        // 5.5 m between the centres = 4.5 m between the assumed 0.5 m spheres
        assert_eq!(i.trade_action(&z, 77), Action::Trade);
        assert_eq!(sent(&mut i), [msg(trade::START, BOB, ZERO)]);
        assert_eq!(i.trade_action(&z, 78), Action::Refused("Feedback_TargetOutsideRangeForTrade"));
        assert_eq!(i.ptrade.take_feedback(), ["Feedback_TargetOutsideRangeForTrade"]);
        assert!(i.take_outbox().is_empty());
        assert_eq!(i.trade_action(&z, OWN), Action::None);
        assert_eq!(i.trade_action(&z, 99), Action::None, "unknown dynel");
        z.fight_target.insert(OWN, 77);
        assert_eq!(i.trade_action(&z, 77), Action::None, "the own character is fighting");
        assert!(i.take_outbox().is_empty());
        // the same through the default action of a (not talkable) character
        z.fight_target.clear();
        assert_eq!(i.default_action_on(&z, BOB), Action::Trade);
        assert_eq!(sent(&mut i).len(), 1);
    }

    #[test]
    fn window_is_the_player_trade_view() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z);
        let p = i.ptrade.trade.as_ref().unwrap();
        let win = p.win;
        assert_eq!(gui.window_size(p.win), CLIENT);
        assert_eq!(gui.text(p.win, "PartnerName"), "Bob");
        assert_eq!(gui.text(p.win, "PartnerCashView"), "0");
        for v in ["PartnerInventoryDock", "OurInventoryDock", "OurCashView", "AcceptButton", "DeclineButton", "PartnerStatusDock", "our_items", "partner_items", "status"] {
            assert!(gui.has_view(p.win, v), "{v}");
        }
        i.ptrade.trade.as_mut().unwrap().render(&mut gui, &mut |_, _| None);
        // the red light first, the green one once accepted
        let light = |gui: &Gui, w| match &gui.canvas_items(w, "status")[0] {
            CanvasItem::Image { id, .. } => id.0,
            o => panic!("{o:?}"),
        };
        assert_eq!(light(&gui, win), 0x15b);
        i.on_trade(&mut gui, msg(trade::ACCEPT, ZERO, ZERO), ME, &z);
        i.ptrade.trade.as_mut().unwrap().render(&mut gui, &mut |_, _| None);
        assert_eq!(light(&gui, win), 0xd9);
        gui.frame(0.0);
    }

    #[test]
    fn an_ignored_partner_is_aborted_without_a_flag_and_a_second_start_is_refused() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = Interact::new(OWN as u32, (1280, 800));
        i.on_trade(&mut gui, msg(trade::START, BOB, ZERO), ME, &z);
        i.ptrade_start(&mut gui, &z, |id| id == 77);
        assert_eq!(sent(&mut i), [msg(trade::ABORT, ZERO, ZERO)]);
        assert!(i.ptrade.trade.is_none());
        // a start about somebody else's state, or with a second identity (shops), opens nothing
        i.on_trade(&mut gui, msg(trade::START, BOB, ZERO), BOB, &z);
        i.on_trade(&mut gui, msg(trade::START, BOB, BOB), ME, &z);
        i.ptrade_start(&mut gui, &z, |_| false);
        assert!(i.ptrade.trade.is_none());
        i.on_trade(&mut gui, msg(trade::START, BOB, ZERO), ME, &z);
        i.ptrade_start(&mut gui, &z, |_| false);
        assert!(i.ptrade.trade.is_some());
        i.on_trade(&mut gui, msg(trade::START, BOB, ZERO), ME, &z);
        assert_eq!(i.ptrade.take_feedback(), ["Feedback_YouAreAlreadyInATrade"]);
    }

    #[test]
    fn lists_follow_the_servers_adds_and_removes() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z);
        let item = |s: i32| Identity { kind: 0x68, instance: s };
        i.on_trade(&mut gui, msg(trade::ADD_ITEM, ME, item(0x40)), ME, &z);
        i.on_trade(&mut gui, msg(trade::ADD_ITEM, BOB, item(0x50)), BOB, &z);
        i.on_trade(&mut gui, msg(trade::ADD_ITEM, ME, item(0x41)), ME, &z);
        let p = i.ptrade.trade.as_ref().unwrap();
        assert_eq!(p.ours.iter().map(|e| e.acg.map(|a| a.low_id)).collect::<Vec<_>>(), [Some(111), Some(222)], "the template is read from the inventory");
        assert_eq!(p.theirs, [Entry { item: item(0x50), acg: None }]);
        i.on_trade(&mut gui, msg(trade::REMOVE_ITEM, ME, item(0x40)), ME, &z);
        i.on_trade(&mut gui, msg(trade::REMOVE_ITEM, BOB, item(0x50)), BOB, &z);
        i.on_trade(&mut gui, msg(trade::ADD_ITEM, Identity { kind: DYNEL_CHAR, instance: 5 }, item(1)), ME, &z);
        let p = i.ptrade.trade.as_ref().unwrap();
        assert_eq!((p.ours.len(), p.theirs.len()), (1, 0));
        // the partner's cash is shown formatted, the own value is only remembered
        i.on_trade(&mut gui, msg(trade::SET_CASH, Identity { kind: 0, instance: 1_234_567 }, ZERO), BOB, &z);
        i.on_trade(&mut gui, msg(trade::SET_CASH, Identity { kind: 0, instance: -5 }, BOB), BOB, &z);
        let p = i.ptrade.trade.as_ref().unwrap();
        assert_eq!((p.partner_cash, gui.text(p.win, "PartnerCashView").as_str()), (1_234_567, "1,234,567"));
        let dump = i.ptrade_dump(&gui);
        assert!(dump.contains("ours: [104:65"), "{dump}");
    }

    #[test]
    fn cash_edits_are_sent_once_after_the_delay() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z);
        let win = i.ptrade.trade.as_ref().unwrap().win;
        i.ptrade_tick(&gui, 10.0);
        gui.set_text(win, "OurCashView", "12x3");
        assert!(i.event(&mut gui, &Event::TextChanged { window: win, view: "OurCashView".into(), text: "12x3".into() }, &z));
        assert_eq!(gui.text(win, "OurCashView"), "123", "digits only");
        i.ptrade_tick(&gui, 10.5);
        assert!(i.take_outbox().is_empty());
        i.ptrade_tick(&gui, 11.1);
        assert_eq!(sent(&mut i), [msg(trade::SET_CASH, Identity { kind: 0, instance: 123 }, ZERO)]);
        i.ptrade_tick(&gui, 20.0);
        assert!(i.take_outbox().is_empty());
    }

    #[test]
    fn accept_confirm_and_the_box() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = open(&mut gui, &z);
        let win = i.ptrade.trade.as_ref().unwrap().win;
        i.ptrade.set_texts("Yes".into(), "No".into());
        i.ptrade_tick(&gui, 1.0);
        gui.set_text(win, "OurCashView", "2500");
        i.event(&mut gui, &Event::TextChanged { window: win, view: "OurCashView".into(), text: "2500".into() }, &z);
        // Accept: the pending cash first, the field formatted, the button disabled, the light green, then `TradeConfirm`
        click(&mut i, &mut gui, &z, "AcceptButton");
        assert_eq!(sent(&mut i), [msg(trade::SET_CASH, Identity { kind: 0, instance: 2500 }, ZERO), msg(trade::CONFIRM, ZERO, ZERO)]);
        assert_eq!(gui.text(win, "OurCashView"), "2,500");
        assert!(!gui.is_enabled(win, "AcceptButton"));
        assert!(i.ptrade.trade.as_ref().unwrap().accepted);
        // the server's op 3 asks "Are you sure ...": Yes -> TradeAccept
        i.on_trade(&mut gui, msg(trade::CONFIRM, ZERO, ZERO), ME, &z);
        let dlg = i.ptrade.confirm.windows();
        assert_eq!(dlg.len(), 1);
        assert!(i.event(&mut gui, &Event::Clicked { window: dlg[0], view: "btn0".into(), item: None }, &z));
        assert_eq!(sent(&mut i), [msg(trade::ACCEPT, ZERO, ZERO)]);
        // No -> TradeAbort(true)
        i.on_trade(&mut gui, msg(trade::CONFIRM, ZERO, ZERO), ME, &z);
        let dlg = i.ptrade.confirm.windows();
        i.event(&mut gui, &Event::Clicked { window: dlg[0], view: "btn1".into(), item: None }, &z);
        assert_eq!(sent(&mut i), [msg(trade::ABORT, Identity { kind: 0, instance: 1 }, ZERO)]);
        // op 10 resets the accepted state, enables Accept and closes the box
        i.on_trade(&mut gui, msg(trade::CONFIRM, ZERO, ZERO), ME, &z);
        i.on_trade(&mut gui, msg(trade::RESET, ZERO, ZERO), ME, &z);
        assert!(i.ptrade.confirm.windows().is_empty());
        assert!(gui.is_enabled(win, "AcceptButton") && !i.ptrade.trade.as_ref().unwrap().accepted);
        // a confirm about the partner's state asks nothing
        i.on_trade(&mut gui, msg(trade::CONFIRM, ZERO, ZERO), BOB, &z);
        assert!(i.ptrade.confirm.windows().is_empty());
        click(&mut i, &mut gui, &z, "DeclineButton");
        assert_eq!(sent(&mut i), [msg(trade::ABORT, Identity { kind: 0, instance: 1 }, ZERO)]);
        assert!(i.ptrade.trade.is_some(), "the window waits for the server's answer");
    }

    #[test]
    fn abort_and_completion_close_the_window() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let flag = |v| Identity { kind: 0, instance: v };
        for (who, a, op, notice) in [
            (ME, flag(1), trade::ABORT, true),
            (ME, flag(0), trade::ABORT, false),
            (BOB, flag(0), trade::ABORT, true),
            (ME, ZERO, trade::COMPLETE, false),
        ] {
            let mut i = open(&mut gui, &z);
            let win = i.ptrade.trade.as_ref().unwrap().win;
            i.on_trade(&mut gui, msg(trade::CONFIRM, ZERO, ZERO), ME, &z);
            i.on_trade(&mut gui, msg(op, a, ZERO), who, &z);
            assert!(i.ptrade.trade.is_none() && i.ptrade.confirm.windows().is_empty() && !gui.has_view(win, "AcceptButton"));
            assert_eq!(i.ptrade.take_feedback(), if notice { vec!["Feedback_TradeCancelled"] } else { vec![] }, "{who:?} {op}");
        }
        // an abort of a stranger does not touch the trade
        let mut i = open(&mut gui, &z);
        i.on_trade(&mut gui, msg(trade::ABORT, flag(1), ZERO), Identity { kind: DYNEL_CHAR, instance: 5 }, &z);
        assert!(i.ptrade.trade.is_some());
        // Esc declines (`esc_trades`), as does the window's close button
        let win = i.ptrade.trade.as_ref().unwrap().win;
        assert!(i.event(&mut gui, &Event::Escape { window: win }, &z));
        assert!(i.event(&mut gui, &Event::CloseRequested { window: win }, &z));
        assert_eq!(sent(&mut i), [msg(trade::ABORT, flag(1), ZERO), msg(trade::ABORT, flag(1), ZERO)]);
        // leaving the zone closes without a word
        i.close_all(&mut gui);
        assert!(i.ptrade.trade.is_none() && i.take_outbox().is_empty());
    }

    #[test]
    fn dropping_an_inventory_item_on_the_own_list_adds_it_and_a_double_click_removes_it() {
        let Some(mut gui) = rig() else { return };
        let mut z = zone();
        let mut i = open(&mut gui, &z);
        let win = i.ptrade.trade.as_ref().unwrap().win;
        let r = gui.view_rect(win, "our_items").unwrap();
        let (x, y) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        let bag = |s: i32| Identity { kind: 0x68, instance: s };
        // over the partner's list, over nothing, an empty slot, a worn slot: nothing
        let p = gui.view_rect(win, "partner_items").unwrap();
        i.ptrade_drops(&gui, &z, vec![(0x40, p.l + 5.0, p.t + 5.0), (0x40, 5.0, 5.0), (0x55, x, y), (0x10, x, y)]);
        assert!(i.take_outbox().is_empty());
        i.ptrade_drops(&gui, &z, vec![(0x40, x, y)]);
        assert_eq!(sent(&mut i), [msg(trade::ADD_ITEM, ME, bag(0x40))]);
        i.on_trade(&mut gui, msg(trade::ADD_ITEM, ME, bag(0x40)), ME, &z);
        i.ptrade_drops(&gui, &z, vec![(0x40, x, y)]);
        assert!(i.take_outbox().is_empty(), "already in the trade");
        // the item is drawn in the first cell (the template is unknown without rdb, the slot art is there)
        i.ptrade.trade.as_mut().unwrap().render(&mut gui, &mut |_, _| Some(("Thing".into(), None)));
        assert!(!gui.canvas_items(win, "our_items").is_empty());
        let press = |x: f32, y: f32| Event::CanvasPress { window: win, view: "our_items".into(), x, y, button: MouseButton::Left, clicks: 2 };
        assert!(i.event(&mut gui, &press(SLOT * 2.5, 3.0), &z), "empty cell");
        assert!(i.take_outbox().is_empty());
        i.event(&mut gui, &press(3.0, 3.0), &z);
        assert_eq!(sent(&mut i), [msg(trade::REMOVE_ITEM, ME, bag(0x40))]);
        // the traded item's own slot counts as free: with a full bag the removal is still sent; with nothing traded it is refused
        for sl in 0x40..0x40 + 30 {
            z.inventory.insert(sl, z.inventory[&0x41]);
        }
        assert!(i.ptrade.trade.as_ref().unwrap().bag_has_room(&z));
        i.event(&mut gui, &press(3.0, 3.0), &z);
        assert_eq!(sent(&mut i), [msg(trade::REMOVE_ITEM, ME, bag(0x40))]);
        i.on_trade(&mut gui, msg(trade::REMOVE_ITEM, ME, bag(0x40)), ME, &z);
        assert!(!i.ptrade.trade.as_ref().unwrap().bag_has_room(&z));
    }

    /// Frame of the trade window through the zone-frame hook: the bytes the server sends decode into the N3 enum and drive the window.
    #[test]
    fn server_frames_reach_the_window() {
        let Some(mut gui) = rig() else { return };
        let z = zone();
        let mut i = Interact::new(OWN as u32, (1280, 800));
        let frame = |t: Trade, who: Identity| ao_net::n3::outgoing::n3_frame(1, OWN as u32, t.encode(who));
        i.on_frame(&mut gui, &frame(msg(trade::START, BOB, ZERO), ME), &z);
        i.ptrade_start(&mut gui, &z, |_| false);
        i.on_frame(&mut gui, &frame(msg(trade::SET_CASH, Identity { kind: 0, instance: 900 }, ZERO), BOB), &z);
        let p = i.ptrade.trade.as_ref().unwrap();
        assert_eq!(gui.text(p.win, "PartnerCashView"), "900");
        i.on_frame(&mut gui, &frame(msg(trade::ABORT, ZERO, ZERO), BOB), &z);
        assert!(i.ptrade.trade.is_none());
        assert_eq!(i.ptrade.take_feedback(), ["Feedback_TradeCancelled"]);
    }

    /// `AOMAC_SHOT_DIR=/tmp/x cargo test ... trade_window_screenshot`: the window with labels from the text db, two own items and one of the partner's.
    #[test]
    fn trade_window_screenshot() {
        use ao_gui::{DrawList, InputEvent};
        use ao_render::{Frontend, Host, Offscreen};
        struct Shot(Gui);
        impl Frontend for Shot {
            fn gui(&self) -> &Gui {
                &self.0
            }
            fn input(&mut self, _: InputEvent, _: &mut Host) {}
            fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut Host) -> DrawList {
                self.0.frame(dt)
            }
        }
        let dir = ao_gui::client_dir();
        let (Ok(labels), true) = (ao_formats::screens::TextDb::load(&dir), std::env::var_os("AOMAC_SHOT_DIR").is_some()) else { return };
        let mut gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let z = zone();
        let mut i = open(&mut gui, &z);
        let bag = |s: i32| Identity { kind: 0x68, instance: s };
        i.on_trade(&mut gui, msg(trade::ADD_ITEM, ME, bag(0x40)), ME, &z);
        i.on_trade(&mut gui, msg(trade::ADD_ITEM, BOB, bag(0x50)), BOB, &z);
        i.on_trade(&mut gui, msg(trade::SET_CASH, Identity { kind: 0, instance: 25_000 }, ZERO), BOB, &z);
        gui.set_text(i.ptrade.trade.as_ref().unwrap().win, "OurCashView", "1,500");
        i.ptrade.trade.as_mut().unwrap().render(&mut gui, &mut |_, _| Some(("Thing".into(), None)));
        let mut s = Shot(gui);
        let mut o = Offscreen::new(&s, (1280, 560)).unwrap();
        let mut list = DrawList::default();
        for _ in 0..3 {
            list = o.frame(&mut s, 0.016);
        }
        let dir = std::path::PathBuf::from(std::env::var_os("AOMAC_SHOT_DIR").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        o.png(&s, &list, &dir.join("ptrade.png")).unwrap();
    }
}
