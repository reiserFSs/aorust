//! The vending machine buy window (`TradeView_c` type 2 = `ShopBuy` of `Views/TradeGUI.xml`, GUI.dll). Evidence: docs/zone/interact.md ("Vending machines / shops").
//!
//! Flow [CODE unless marked]:
//! * Using the machine (`GenericCmd_t` 3) is answered with `ShopUpdateIIR_t` (the stock, [`ao_net::n3::shop`]; `Activate` `FUN_100a0f2c` -> `FUN_1009a4b2`) and a
//!   `TradeIIR_t` op 0 pair (`FUN_100663e4` / `FUN_1009a23c`): header = the own character with `a` = the machine and `b` = a non-zero session identity
//!   -> `GlobalSignals +0xd4 (type, own, a, b)` -> `InventoryGUIModule_c::SlotStartTrade` [GUI 0x100c6ff4] -> `TradeView_c` [GUI 0x100e092f].
//! * The window: shop list (`ItemContainerView_c` mode 1, the machine's stock; item identities `{0x6f, index}`), "Bought Items" list (mode 2 =
//!   `N3Msg_TradeGetInventory(machine)`), "Credits" (`PartnerCashView`, the amount due), Accept / Decline.
//! * A double click on a shop item (`FUN_100ca1e7`): with Shift / Ctrl held `N3Msg_TradeAddItem(own, item)` (`TradeIIR_t` op 5), else
//!   `MoveItemToInventory(item)`; a double click on a bought item removes it (`N3Msg_TradeRemoveItem`, op 6). Accept = `N3Msg_TradeAccept` (op 1),
//!   Decline / close / Esc (`esc_shops`) = `N3Msg_TradeAbort(true)` (op 2).

use super::hud::group;
use super::interact::Interact;
use ao_gui::widgets::MultiCell;
use ao_gui::{Event, Gui, WindowId, WindowSize};
use ao_net::msg::Identity;
use ao_net::n3::inventory::{self, ANY_BAG_SLOT};
use ao_net::n3::shop::ShopUpdate;
use ao_net::n3::trade::{self, Trade};
use ao_net::n3::world::{AcgItem, World};
use ao_net::n3::{Message, N3};
use std::collections::HashMap;

/// Prop kind of the vending machine dynels (`VendingMachine_t`, live: `{0xC75B, 0x4b}`).
pub const VENDING_MACHINE: i32 = 0xC75B;
/// Kind of the identities of shop items (`{0x6f, index into the stock}`: `N3Msg_TradeAddItem` tests `*id == 0x6f`).
pub const SHOP_ITEM: i32 = 0x6f;
/// The window is a `TradeView_c`: same client size as the player trade's.
const CLIENT: (u32, u32) = super::interact_ptrade::CLIENT;
/// Option `esc_shops` (`LoginPrefs.xml`: true): Esc declines (`FUN_100df810`).
const ESC_SHOPS: bool = true;
const ZERO: Identity = Identity { kind: 0, instance: 0 };
/// `AddColumn` flags of the item lists: resizable | label | sortable.
const COL_FLAGS: u32 = 0xe;
/// The columns the flags of the lists add (`FUN_100cdb3a`): Name (id 1), Count (2), Price (3), Quality (4). The original widths are 200 / 30 / 100 / 100
/// (`_DAT_101a959c`, `_DAT_101ae5c8`, `_DAT_101ae160`); the dock is 192 px wide, so [GUESS] they are squeezed to fit it (the Icon column, 16 px, is not shown:
/// the engine's `MultiListView` has no picture cells).
const COLUMNS: [(i32, &str, f32); 4] = [(1, "Name", 56.0), (2, "Count", 40.0), (3, "Price", 46.0), (4, "Quality", 44.0)];

/// The open window and its model.
struct Shop {
    win: WindowId,
    machine: Identity,
    /// The machine's stock at the time of the last update: item `{0x6f, i}` is `stock[i]`.
    stock: Vec<AcgItem>,
    /// `N3Msg_TradeGetInventory(machine)`: what the server accepted into the trade (identities as the server names them).
    bought: Vec<Identity>,
    /// The amount due the server sent last (`TradeIIR_t` op 7 from the machine), shown until the list changes.
    due: Option<i32>,
    dirty: bool,
    /// The rows as shown, `[shop list, bought list]`, for the live harness.
    rows: [Vec<String>; 2],
}

#[derive(Default)]
pub struct ShopUi {
    shop: Option<Shop>,
    /// The stock of every machine a `ShopUpdateIIR_t` was received for (`VendingMachine_t` +0x1fc).
    stocks: HashMap<(i32, i32), Vec<AcgItem>>,
    /// Names of the machines (`VendingMachineFullUpdateIIR_t`'s name blob).
    names: HashMap<(i32, i32), String>,
    feedback: Vec<&'static str>,
    /// Shift or Ctrl is held (`View::GetQualifiers() & 0xc` of `FUN_100ca1e7`; which bits are which keys is [INFERENCE]).
    pub quick: bool,
    /// Every step, for the live harness.
    pub log: Vec<String>,
}

/// The `ShopBuy` view of `Views/TradeGUI.xml` (the last view of the file; the loader of the engine opens only the first one).
fn shop_xml() -> anyhow::Result<String> {
    let path = ao_gui::client_dir().join("cd_image/gui/Default/Views/TradeGUI.xml");
    let s = std::fs::read_to_string(&path)?;
    let (a, b) = (s.find("<View name=\"ShopBuy\"").ok_or_else(|| anyhow::anyhow!("no ShopBuy view"))?, s.rfind("</root>").unwrap_or(s.len()));
    Ok(format!("<root>{}</root>", &s[a..b]))
}

fn list_xml(name: &str, (w, h): (u32, u32)) -> String {
    format!("<root><MultiListView name=\"{name}\" feature_flags=\"64\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/></root>")
}

impl Shop {
    /// `TradeView_c` ctor type 2: the title is "Trade", the partner name the machine's name, the docks hold the two item lists.
    fn open(gui: &mut Gui, screen: (u32, u32), machine: Identity, name: &str, stock: Vec<AcgItem>) -> anyhow::Result<Shop> {
        let src = shop_xml()?;
        // the fixed rows of the view are measured with empty docks, the free height goes to the two lists (2 : 1)
        let probe = gui.open_window_xml("ShopBuy", &src, (0, 0), WindowSize::Preferred)?;
        let fixed = gui.window_size(probe).1;
        gui.close_window(probe);
        let free = CLIENT.1.saturating_sub(fixed);
        let dw = CLIENT.0 - 12;
        let (shop_h, bought_h) = (free * 2 / 3, free - free * 2 / 3);
        let pos = ((screen.0 as i32 - super::hud_rollup::AREA_W as i32 - CLIENT.0 as i32 - 20).max(0), 40);
        let win = gui.open_tabbed_window_xml("ShopBuy", "Trade", &src, pos, WindowSize::Fixed(CLIENT.0, CLIENT.1))?;
        for (dock, view, h) in [("ShopInventoryDock", "shop_items", shop_h), ("PartnerInventoryDock", "bought_items", bought_h)] {
            gui.add_view_xml(win, dock, view, &list_xml(view, (dw, h)))?;
            for (id, label, w) in COLUMNS {
                gui.multi_add_column(win, view, id, label, w, COL_FLAGS);
            }
        }
        gui.set_text(win, "PartnerName", name);
        // `FUN_100e039a`: the credits field starts as "0" (read-only)
        gui.set_text(win, "PartnerCashView", "0");
        gui.relayout_window(win);
        Ok(Shop { win, machine, stock, bought: vec![], due: None, dirty: true, rows: Default::default() })
    }

    /// The stock entry an identity names.
    fn item(&self, id: Identity) -> Option<AcgItem> {
        (id.kind == SHOP_ITEM).then(|| self.stock.get(id.instance as usize).copied()).flatten()
    }
}

/// One list row: Name, Count, Price, Quality (`FUN_100404a9`: count = the template's count (stat 0x19c, 0 -> 1), price = `N3Msg_GetShopItemStat(item, 0x4a)`,
/// shown empty when 0; quality = the item's level).
fn cells(info: &Option<(String, i32, i32)>, level: i32) -> Vec<MultiCell> {
    let (name, count, price) = info.clone().unwrap_or_default();
    vec![MultiCell::text(&name), MultiCell::num(count as i64), MultiCell { text: if price == 0 { String::new() } else { group(price) }, key: ao_gui::widgets::MultiKey::Num(price as i64) }, MultiCell::num(level as i64)]
}

type Info<'a> = &'a mut dyn FnMut(&mut Gui, i32) -> Option<(String, i32, i32)>;

impl Interact {
    /// Every decoded zone frame: the machine's name and stock (`ShopUpdateIIR_t::Activate`, `FUN_100a0f2c`: only for a `VendingMachine_t` header).
    pub(super) fn shop_watch(&mut self, m: &Message) {
        let t = m.header.target;
        let ui = &mut self.shop;
        match &m.body {
            N3::World(World::VendingMachine(v)) => {
                let name = v.base.blob.split(|&b| b == 0).next().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                ui.names.insert((t.kind, t.instance), name);
            }
            N3::Shop(ShopUpdate { items }) if t.kind == VENDING_MACHINE => {
                // `FUN_1009a4b2`: the stock is rebuilt; an empty one says `Feedback_ShopContainsNoEntries`
                if items.is_empty() {
                    ui.feedback.push("Feedback_ShopContainsNoEntries");
                }
                ui.stocks.insert((t.kind, t.instance), items.clone());
                if let Some(s) = ui.shop.as_mut().filter(|s| s.machine == t) {
                    s.stock = items.clone();
                    s.dirty = true;
                }
            }
            _ => {}
        }
    }

    /// A `TradeIIR_t` of the shop trade; `true` when consumed (everything else is the player trade's).
    pub(super) fn shop_trade(&mut self, gui: &mut Gui, t: &Trade, who: Identity, ptrade_open: bool) -> bool {
        let me = self.own_id();
        let Some(s) = self.shop.shop.as_mut() else {
            // `FUN_100663e4` -> type 2: the trade with a vending machine starts (`b` = the session identity); the machine's own copy (header = the machine) opens nothing
            if t.op == trade::START && t.a.kind == VENDING_MACHINE && t.b != ZERO {
                self.shop.log.push(format!("{who:?} {t:?}"));
                if who == me {
                    if ptrade_open {
                        self.shop.feedback.push("Feedback_YouAreAlreadyInATrade");
                    } else {
                        let key = (t.a.kind, t.a.instance);
                        let name = self.shop.names.get(&key).cloned().unwrap_or_default();
                        let stock = self.shop.stocks.get(&key).cloned().unwrap_or_default();
                        match Shop::open(gui, self.screen, t.a, &name, stock) {
                            Ok(s) => self.shop.shop = Some(s),
                            Err(e) => eprintln!("interact: shop window: {e:#}"),
                        }
                    }
                }
                return true;
            }
            return false;
        };
        // header = the own character or the machine: both are the shop trade's state
        if who != me && who != s.machine {
            return false;
        }
        self.shop.log.push(format!("{who:?} {t:?}"));
        match t.op {
            trade::START => {
                if who == me {
                    self.shop.feedback.push("Feedback_YouAreAlreadyInATrade");
                }
            }
            // `FUN_100666a3`: the machine's abort (`FUN_1009a23c` case 2) cancels with the notice; ours only when `a.instance` is set
            trade::ABORT => {
                if who == s.machine || t.a.instance != 0 {
                    self.shop.feedback.push("Feedback_TradeCancelled");
                }
                self.shop_close(gui);
            }
            // `FUN_100668d1` + `SlotTradeCompleted`: the window is deleted
            trade::COMPLETE if who == me => self.shop_close(gui),
            // the server confirms an add / remove (`FUN_10066cf7` / `FUN_10066faf`, vending `FUN_10067109` / `FUN_100671e8`); `b` = the item
            trade::ADD_ITEM | trade::VENDING_ADD if who == me => {
                s.bought.push(t.b);
                s.due = None;
                s.dirty = true;
            }
            trade::REMOVE_ITEM | trade::VENDING_REMOVE if who == me => {
                if let Some(i) = s.bought.iter().position(|&b| b == t.b) {
                    s.bought.remove(i);
                }
                s.due = None;
                s.dirty = true;
            }
            // `FUN_100672c7` -> `+0xdc` -> `FUN_100dfe5e`: `PartnerCashView` = the number
            trade::SET_CASH if who == s.machine && t.a.instance >= 0 => {
                s.due = Some(t.a.instance);
                s.dirty = true;
            }
            _ => {}
        }
        true
    }

    /// The window is gone: `+0xe4(1)` (`SlotTradeCompleted` deletes the view).
    fn shop_close(&mut self, gui: &mut Gui) {
        if let Some(s) = self.shop.shop.take() {
            gui.close_window(s.win);
        }
    }

    pub(super) fn shop_close_all(&mut self, gui: &mut Gui) {
        self.shop_close(gui);
    }

    /// GUI events of the window; `true` when consumed.
    pub(super) fn shop_event(&mut self, gui: &mut Gui, ev: &Event) -> bool {
        let me = self.own_id();
        let quick = self.shop.quick;
        let Some(s) = self.shop.shop.as_mut() else { return false };
        let mut out: Vec<Vec<u8>> = vec![];
        match ev {
            // `FUN_100dfd33`: Accept is disabled, then `N3Msg_TradeAccept` (type != 0)
            Event::Clicked { window, view, .. } if *window == s.win && view == "AcceptButton" => {
                gui.set_enabled(s.win, "AcceptButton", false);
                out.push(trade::accept(me));
            }
            // `FUN_100df810`
            Event::Clicked { window, view, .. } if *window == s.win && view == "DeclineButton" => out.push(trade::abort(me, true)),
            Event::CloseRequested { window } if *window == s.win => out.push(trade::abort(me, true)),
            Event::Escape { window } if *window == s.win && ESC_SHOPS => out.push(trade::abort(me, true)),
            Event::Escape { window } if *window == s.win => {}
            Event::MultiMouse { window, view, id, button, clicks, .. } if *window == s.win => {
                let Some(id) = *id else { return true };
                if *button == 1 && *clicks >= 2 {
                    match view.as_str() {
                        // `FUN_100ca1e7`: Shift / Ctrl + double click = `N3Msg_TradeAddItem(own, item)`, a plain double click = `MoveItemToInventory(item)`
                        "shop_items" if (id as usize) < s.stock.len() => {
                            let item = Identity { kind: SHOP_ITEM, instance: id as i32 };
                            out.push(if quick { trade::add_item(me, item) } else { inventory::move_item_to_inventory(me.instance, item, ANY_BAG_SLOT) });
                        }
                        // `FUN_100df67e` (double click on a bought item): `N3Msg_TradeRemoveItem(own, item)` [INFERENCE: wired for this list]
                        "bought_items" => {
                            if let Some(&b) = s.bought.get(id as usize) {
                                out.push(trade::remove_item(me, me, b));
                            }
                        }
                        _ => {}
                    }
                } else if *button == 1 {
                    gui.multi_select(s.win, view, id, true, true);
                }
            }
            Event::MultiMouse { window, .. } | Event::MultiSelected { window, .. } if *window == s.win => {}
            _ => return false,
        }
        out.into_iter().for_each(|p| self.send(p));
        true
    }

    /// Fills the lists and the amount due when something changed; `info(gui, low_id)` = (name, count, price) of a template.
    pub(super) fn shop_render(&mut self, gui: &mut Gui, info: Info) {
        let Some(s) = self.shop.shop.as_mut().filter(|s| s.dirty) else { return };
        s.dirty = false;
        let lists: [Vec<(i64, AcgItem)>; 2] = [
            s.stock.iter().enumerate().map(|(i, a)| (i as i64, *a)).collect(),
            s.bought.iter().enumerate().map(|(i, b)| (i as i64, s.item(*b).unwrap_or_default())).collect(),
        ];
        let mut due = 0i64;
        for (n, (view, list)) in ["shop_items", "bought_items"].into_iter().zip(lists).enumerate() {
            gui.multi_clear(s.win, view);
            s.rows[n].clear();
            for (id, a) in list {
                let inf = info(gui, a.low_id);
                if n == 1 {
                    due += inf.as_ref().map_or(0, |i| i.2 as i64);
                }
                let c = cells(&inf, a.level);
                s.rows[n].push(format!("[{id}] {}", c.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" | ")));
                gui.multi_add_row(s.win, view, id, c, true);
            }
        }
        // `FUN_100dfbf3` (`+0xd8`): the credits field shows the cost of the bought items (`FUN_10099d56`: price x count of every one); the machine's own figure wins
        gui.set_text(s.win, "PartnerCashView", &group(s.due.unwrap_or(due.min(i32::MAX as i64) as i32)));
    }
}

impl super::Play {
    /// Per frame, after [`Play::interact_ptrade_frame`]: fills the shop lists, prints the feedback, sends what the window produced.
    pub(super) fn interact_shop_frame(&mut self) {
        let Some(i) = self.interact.as_mut() else { return };
        if let Some(h) = self.hud.as_mut() {
            i.shop_render(&mut self.gui, &mut |g, low| h.shop_info(g, low));
        }
        for key in std::mem::take(&mut i.shop.feedback) {
            if let Some(c) = self.chat.as_mut() {
                c.feedback(&mut self.gui, key, &self.text);
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
    /// The shop window as the harness prints it: name, the rows of both lists as shown (Name | Count | Price | Quality), the amount due.
    pub fn shop_dump(&self, gui: &Gui) -> String {
        let Some(s) = self.shop.shop.as_ref() else { return "shop: no window".into() };
        format!(
            "shop {:?} ({:?})\n  stock ({}): {:?}\n  bought ({}): {:?}\n  credits: {:?}  accept enabled: {}\n  log: {:?}",
            gui.text(s.win, "PartnerName"),
            s.machine,
            s.stock.len(),
            s.rows[0],
            s.bought.len(),
            s.rows[1],
            gui.text(s.win, "PartnerCashView"),
            gui.is_enabled(s.win, "AcceptButton"),
            self.shop.log
        )
    }

    fn shop_double_click(&mut self, gui: &mut Gui, view: &str, index: usize, quick: bool) -> bool {
        let Some(win) = self.shop.shop.as_ref().map(|s| s.win) else { return false };
        self.shop.quick = quick;
        let ev = Event::MultiMouse { window: win, view: view.into(), id: Some(index as i64), button: 1, clicks: 2, x: 0, y: 0 };
        self.shop_event(gui, &ev)
    }

    /// Double click on stock item `index` (a plain one: `MoveItemToInventory({0x6f, index})`); false without a window or such an item.
    pub fn shop_buy(&mut self, gui: &mut Gui, index: usize) -> bool {
        self.shop.shop.as_ref().is_some_and(|s| index < s.stock.len()) && self.shop_double_click(gui, "shop_items", index, false)
    }

    /// Shift + double click on stock item `index` (`TradeAddItem(own, {0x6f, index})`, the server then confirms it into the bought list).
    pub fn shop_add(&mut self, gui: &mut Gui, index: usize) -> bool {
        self.shop.shop.as_ref().is_some_and(|s| index < s.stock.len()) && self.shop_double_click(gui, "shop_items", index, true)
    }

    /// Double click on bought item `index` (`TradeRemoveItem`).
    pub fn shop_remove(&mut self, gui: &mut Gui, index: usize) -> bool {
        self.shop.shop.as_ref().is_some_and(|s| index < s.bought.len()) && self.shop_double_click(gui, "bought_items", index, false)
    }

    /// Press Accept (`TradeAccept`) / Decline (`TradeAbort(true)`).
    pub fn shop_press(&mut self, gui: &mut Gui, accept: bool) -> bool {
        let Some(win) = self.shop.shop.as_ref().map(|s| s.win) else { return false };
        let view = if accept { "AcceptButton" } else { "DeclineButton" };
        self.shop_event(gui, &Event::Clicked { window: win, view: view.into(), item: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::zone::Zone;
    use ao_net::frame::Frame;

    const OWN: u32 = 0x82e8;
    const ME: Identity = Identity { kind: 50000, instance: OWN as i32 };
    const MACHINE: Identity = Identity { kind: VENDING_MACHINE, instance: 0x4b };
    const SESSION: Identity = Identity { kind: 0xC767, instance: 0x116f_7753 };

    fn rig() -> Option<Gui> {
        let client = ao_gui::client_dir();
        client.join("cd_image/gui").exists().then(|| Gui::new(&client, None).unwrap())
    }

    fn server(payload: Vec<u8>) -> Frame {
        Frame { seq: 0, ptype: ao_net::frame::PT_N3, sender: 1, receiver: 0, payload }
    }

    fn items() -> Vec<AcgItem> {
        [(111, 1), (222, 150), (333, 200)].map(|(low_id, level)| AcgItem { low_id, high_id: low_id, level }).to_vec()
    }

    /// `(name, count, price)` of the fake templates.
    fn info(_: &mut Gui, low: i32) -> Option<(String, i32, i32)> {
        Some((format!("Item{low}"), 1, low * 10))
    }

    fn sent(i: &mut Interact) -> Vec<Vec<u8>> {
        i.take_outbox().into_iter().map(|f| f.payload).collect()
    }

    /// The machine's stock and the trade start as the live server sent them, then the window.
    fn open(gui: &mut Gui, z: &Zone) -> Interact {
        let mut i = Interact::new(OWN, (1280, 800));
        i.on_frame(gui, &server(ShopUpdate { items: items() }.encode(MACHINE)), z);
        i.on_frame(gui, &server(Trade { op: trade::START, a: MACHINE, b: SESSION }.encode(ME)), z);
        i.on_frame(gui, &server(Trade { op: trade::START, a: ME, b: SESSION }.encode(MACHINE)), z);
        i.shop_render(gui, &mut info);
        i
    }

    #[test]
    fn the_trade_start_opens_the_buy_window_with_the_stock() {
        let Some(mut gui) = rig() else { return };
        let z = Zone::new(OWN);
        let i = open(&mut gui, &z);
        let s = i.shop.shop.as_ref().expect("window");
        assert_eq!(gui.window_size(s.win), CLIENT);
        for v in ["ShopInventoryDock", "PartnerInventoryDock", "PartnerCashView", "AcceptButton", "DeclineButton", "shop_items", "bought_items"] {
            assert!(gui.has_view(s.win, v), "{v}");
        }
        assert!(!gui.has_view(s.win, "OurCashView"), "ShopBuy has no own cash field");
        // sorted by the Name column (`SetListSortColumn(1, 0)`)
        assert_eq!(gui.multi_row_ids(s.win, "shop_items"), [0, 1, 2]);
        let d = i.shop_dump(&gui);
        assert!(d.contains("[1] Item222 | 1 | 2,220 | 150"), "{d}");
        assert!(d.contains("credits: \"0\""), "{d}");
        assert!(i.shop.feedback.is_empty());
    }

    #[test]
    fn start_without_a_machine_or_while_in_a_trade_opens_nothing() {
        let Some(mut gui) = rig() else { return };
        let z = Zone::new(OWN);
        let mut i = Interact::new(OWN, (1280, 800));
        // a player trade start (b == 0), a character partner, and the machine's own copy never open the shop window
        let bob = Identity { kind: 50000, instance: 5 };
        i.on_frame(&mut gui, &server(Trade { op: trade::START, a: bob, b: SESSION }.encode(ME)), &z);
        i.on_frame(&mut gui, &server(Trade { op: trade::START, a: ME, b: SESSION }.encode(MACHINE)), &z);
        assert!(i.shop.shop.is_none());
        // an open player trade refuses (`Feedback_YouAreAlreadyInATrade`)
        i.on_frame(&mut gui, &server(Trade { op: trade::START, a: bob, b: ZERO }.encode(ME)), &z);
        i.ptrade_start(&mut gui, &z, |_| false);
        i.on_frame(&mut gui, &server(Trade { op: trade::START, a: MACHINE, b: SESSION }.encode(ME)), &z);
        assert!(i.shop.shop.is_none());
        assert_eq!(std::mem::take(&mut i.shop.feedback), ["Feedback_YouAreAlreadyInATrade"]);
        // an empty stock says so
        i.on_frame(&mut gui, &server(ShopUpdate { items: vec![] }.encode(MACHINE)), &z);
        assert_eq!(std::mem::take(&mut i.shop.feedback), ["Feedback_ShopContainsNoEntries"]);
    }

    #[test]
    fn double_clicks_buttons_and_server_confirmations() {
        let Some(mut gui) = rig() else { return };
        let z = Zone::new(OWN);
        let mut i = open(&mut gui, &z);
        // plain double click = MoveItemToInventory({0x6f, 1}, any bag), Shift = TradeAddItem(own, {0x6f, 1})
        assert!(i.shop_buy(&mut gui, 1));
        assert_eq!(sent(&mut i), [inventory::move_item_to_inventory(ME.instance, Identity { kind: SHOP_ITEM, instance: 1 }, ANY_BAG_SLOT)]);
        assert!(i.shop_add(&mut gui, 1));
        let item = Identity { kind: SHOP_ITEM, instance: 1 };
        assert_eq!(sent(&mut i), [trade::add_item(ME, item)]);
        assert!(!i.shop_buy(&mut gui, 3), "no such stock item");
        // the server's echo fills the bought list and the credits
        i.on_frame(&mut gui, &server(Trade { op: trade::ADD_ITEM, a: ME, b: item }.encode(ME)), &z);
        i.on_frame(&mut gui, &server(Trade { op: trade::VENDING_ADD, a: ME, b: Identity { kind: SHOP_ITEM, instance: 2 } }.encode(ME)), &z);
        i.shop_render(&mut gui, &mut info);
        let d = i.shop_dump(&gui);
        assert!(d.contains("bought (2)") && d.contains("credits: \"5,550\""), "{d}");
        // the amount the machine sends wins until the list changes
        i.on_frame(&mut gui, &server(Trade { op: trade::SET_CASH, a: Identity { kind: 0, instance: 99 }, b: ZERO }.encode(MACHINE)), &z);
        i.shop_render(&mut gui, &mut info);
        assert!(i.shop_dump(&gui).contains("credits: \"99\""));
        // removing a bought item
        assert!(i.shop_remove(&mut gui, 0));
        assert_eq!(sent(&mut i), [trade::remove_item(ME, ME, item)]);
        i.on_frame(&mut gui, &server(Trade { op: trade::REMOVE_ITEM, a: ME, b: item }.encode(ME)), &z);
        i.shop_render(&mut gui, &mut info);
        assert!(i.shop_dump(&gui).contains("bought (1)"));
        // Accept = TradeAccept (and the button is disabled), Decline / Esc / close = TradeAbort(true)
        assert!(i.shop_press(&mut gui, true));
        assert_eq!(sent(&mut i), [trade::accept(ME)]);
        assert!(i.shop_dump(&gui).contains("accept enabled: false"));
        assert!(i.shop_press(&mut gui, false));
        let win = i.shop.shop.as_ref().unwrap().win;
        assert!(i.event(&mut gui, &Event::Escape { window: win }, &z));
        assert!(i.event(&mut gui, &Event::CloseRequested { window: win }, &z));
        assert_eq!(sent(&mut i), vec![trade::abort(ME, true); 3]);
        assert!(i.shop.shop.is_some(), "the window waits for the server's abort");
    }

    #[test]
    fn window_ends_with_the_servers_abort_or_complete() {
        let Some(mut gui) = rig() else { return };
        let z = Zone::new(OWN);
        for (op, who, a, feedback) in [
            (trade::COMPLETE, ME, ZERO, vec![]),
            (trade::ABORT, ME, Identity { kind: 0, instance: 1 }, vec!["Feedback_TradeCancelled"]),
            (trade::ABORT, ME, ZERO, vec![]),
            (trade::ABORT, MACHINE, ME, vec!["Feedback_TradeCancelled"]),
        ] {
            let mut i = open(&mut gui, &z);
            i.on_frame(&mut gui, &server(Trade { op, a, b: ZERO }.encode(who)), &z);
            assert!(i.shop.shop.is_none(), "op {op}");
            assert_eq!(std::mem::take(&mut i.shop.feedback), feedback, "op {op}");
        }
        // the zone change closes it without a word
        let mut i = open(&mut gui, &z);
        i.close_all(&mut gui);
        assert!(i.shop.shop.is_none() && i.take_outbox().is_empty());
        // a new stock refreshes an open window
        let mut i = open(&mut gui, &z);
        i.on_frame(&mut gui, &server(ShopUpdate { items: items()[..1].to_vec() }.encode(MACHINE)), &z);
        i.shop_render(&mut gui, &mut info);
        assert!(i.shop_dump(&gui).contains("stock (1)"));
    }

    /// The replay of the live use of the vending machine (docs/captures/zone_use_object_ithaca.rec, frames 100825..100830): the window opens with the 36 items.
    #[test]
    fn live_capture_opens_the_window() {
        let Some(mut gui) = rig() else { return };
        let z = Zone::new(OWN);
        let mut i = Interact::new(OWN, (1280, 800));
        for l in include_str!("../../../../docs/captures/zone_use_object_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (idx, dir, hex) = (p.next().unwrap().parse::<u32>().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir != "<" || !(100825..=100830).contains(&idx) {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|k| u8::from_str_radix(&hex[2 * k..2 * k + 2], 16).unwrap()).collect();
            i.on_frame(&mut gui, &Frame::decode_with(&b, false).unwrap().unwrap().0, &z);
        }
        let s = i.shop.shop.as_ref().expect("window");
        assert_eq!((s.machine, s.stock.len()), (MACHINE, 36));
        assert_eq!(s.stock[2], AcgItem { low_id: 0x419f9, high_id: 0x419f9, level: 150 });
        assert!(i.shop_buy(&mut gui, 35));
        assert_eq!(sent(&mut i), [inventory::move_item_to_inventory(ME.instance, Identity { kind: SHOP_ITEM, instance: 35 }, ANY_BAG_SLOT)]);
    }

    /// `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac shop_window_screenshot`: the live machine's 36 items (names are the fake templates').
    #[test]
    fn shop_window_screenshot() {
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
        let z = Zone::new(OWN);
        let mut i = open(&mut gui, &z);
        let item = Identity { kind: SHOP_ITEM, instance: 1 };
        i.on_frame(&mut gui, &server(Trade { op: trade::ADD_ITEM, a: ME, b: item }.encode(ME)), &z);
        i.shop_render(&mut gui, &mut info);
        let mut s = Shot(gui);
        let mut o = Offscreen::new(&s, (1280, 560)).unwrap();
        let mut list = DrawList::default();
        for _ in 0..3 {
            list = o.frame(&mut s, 0.016);
        }
        let dir = std::path::PathBuf::from(std::env::var_os("AOMAC_SHOT_DIR").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        o.png(&s, &list, &dir.join("shop.png")).unwrap();
    }
}
