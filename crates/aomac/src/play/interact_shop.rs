//! Vending machine windows (`TradeView_c` type 1 `ShopTrade`, type 2 `ShopBuy` of `Views/TradeGUI.xml`, GUI.dll). Evidence: docs/zone/interact.md.
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
use super::dvalue::{DValues, Variant, element_xml};
use super::hud_wincfg::ListCfg;
use super::hud_stats::inv_grid;
use ao_gui::{CanvasItem, CanvasTip, GfxId, MenuItem};
use ao_gui::xml::{self, Element};
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
/// Original list widths (`FUN_100cdb3a`, `_DAT_101a959c`, `_DAT_101ae5c8`, `_DAT_101ae160`).
const COLUMNS: [(i32, &str, f32); 4] = [(1, "Name", 200.0), (2, "Count", 30.0), (3, "Price", 100.0), (4, "Quality", 100.0)];
const LISTS: [&str; 3] = ["shop_items", "bought_items", "sold_items"];
const MODE_MENU: u32 = 0x5348_0000;

/// The open window and its model.
struct Shop {
    win: WindowId,
    machine: Identity,
    cash_shop: bool,
    sold: Vec<super::interact_ptrade::Entry>,
    /// The machine's stock at the time of the last update: item `{0x6f, i}` is `stock[i]`.
    stock: Vec<AcgItem>,
    /// `N3Msg_TradeGetInventory(machine)`: what the server accepted into the trade (identities as the server names them).
    bought: Vec<Identity>,
    /// The amount due the server sent last (`TradeIIR_t` op 7 from the machine), shown until the list changes.
    due: Option<i32>,
    dirty: bool,
    /// The rows as shown, `[shop list, bought list]`, for the live harness.
    rows: [Vec<String>; 2],
    literacy: i32,
    configs: [ListCfg; 3],
    grid_keys: [Vec<i64>; 3],
    grid_width: f32,
    dock_registered: bool,
}

#[derive(Default)]
pub struct ShopUi {
    shop: Option<Shop>,
    config_loaded: bool,
    configs: [ListCfg; 3],
    pending_config: Option<Element>,
    list_mode_text: String,
    /// The stock of every machine a `ShopUpdateIIR_t` was received for (`VendingMachine_t` +0x1fc).
    stocks: HashMap<(i32, i32), Vec<AcgItem>>,
    /// ShopType (0x9c) and BuyPrice (0x1ab), read by GC 0x1009959d/0x10099954.
    pricing: HashMap<(i32, i32), (i32, i32)>,
    sell_factors: HashMap<(i32, i32), i32>,
    pub(super) literacy: i32,
    /// Names of the machines (`VendingMachineFullUpdateIIR_t`'s name blob).
    names: HashMap<(i32, i32), String>,
    feedback: Vec<&'static str>,
    /// Shift or Ctrl is held (`View::GetQualifiers() & 0xc` of `FUN_100ca1e7`; which bits are which keys is [INFERENCE]).
    pub quick: bool,
    /// Every step, for the live harness.
    pub log: Vec<String>,
}

impl ShopUi {
    fn load_config(&mut self, d: &DValues) {
        if self.config_loaded { return; }
        self.config_loaded = true;
        if let Some(Variant::Archive(s)) = d.get("ShopViewConfig") {
            if let Ok(root) = xml::parse(s) {
                for (i, name) in ["shop_listview_config", "partner_listview_config"].iter().enumerate() {
                    if let Some(e) = root.children.iter().find(|e| e.attr("name") == Some(*name)) {
                        self.configs[i] = ListCfg::parse(e);
                    }
                }
            }
        }
    }

    fn save_config(&mut self, d: &mut DValues) {
        let Some(config) = self.pending_config.take() else { return };
        let mut root = match d.get("ShopViewConfig") {
            Some(Variant::Archive(s)) => xml::parse(s).unwrap_or_else(|_| Element { name: "Archive".into(), attrs: vec![("name".into(), "ShopViewConfig".into())], children: vec![] }),
            _ => Element { name: "Archive".into(), attrs: vec![("name".into(), "ShopViewConfig".into())], children: vec![] },
        };
        root.children.retain(|e| e.attr("name") != Some("shop_listview_config"));
        root.children.push(config);
        d.set("ShopViewConfig", Variant::Archive(element_xml(&root)));
    }
}

/// Extract the server-selected original shop view; the XML loader otherwise opens PlayerTrade.
fn shop_xml(cash_shop: bool) -> anyhow::Result<String> {
    let path = ao_gui::client_dir().join("cd_image/gui/Default/Views/TradeGUI.xml");
    let s = std::fs::read_to_string(&path)?;
    let name = if cash_shop { "ShopTrade" } else { "ShopBuy" };
    let a = s.find(&format!("<View name=\"{name}\"")).ok_or_else(|| anyhow::anyhow!("no {name} view"))?;
    let b = if cash_shop { s[a..].find("<View name=\"ShopBuy\"").map(|b| a + b).unwrap_or(s.len()) } else { s.rfind("</root>").unwrap_or(s.len()) };
    Ok(format!("<root>{}</root>", &s[a..b]))
}

fn list_xml(name: &str, (w, h): (u32, u32)) -> String {
    format!("<root><View view_layout=\"stacked\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"><MultiListView name=\"{name}\" feature_flags=\"64\" h_scrollbar_mode=\"none\" v_scrollbar_mode=\"auto\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/><ScrollView name=\"{name}_grid_scroll\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"none\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"><ScrollViewChild view_layout=\"vertical\"><View name=\"{name}_grid_content\" view_layout=\"vertical\"/></ScrollViewChild></ScrollView></View></root>")
}

impl Shop {
    /// Original type 1 (Cash) or type 2 (other currency) shop view.
    fn open(gui: &mut Gui, screen: (u32, u32), machine: Identity, name: &str, stock: Vec<AcgItem>, cash_shop: bool) -> anyhow::Result<Shop> {
        let src = shop_xml(cash_shop)?;
        let view_name = if cash_shop { "ShopTrade" } else { "ShopBuy" };
        // Measure the original view with empty docks, then distribute its free height.
        let probe = gui.open_window_xml(view_name, &src, (0, 0), WindowSize::Preferred)?;
        let fixed = gui.window_size(probe).1;
        gui.close_window(probe);
        let free = CLIENT.1.saturating_sub(fixed);
        let dw = CLIENT.0 - 12;
        let shop_h = free * 2 / if cash_shop { 4 } else { 3 };
        let bought_h = if cash_shop { (free - shop_h) / 2 } else { free - shop_h };
        let pos = ((screen.0 as i32 - super::hud_rollup::AREA_W as i32 - CLIENT.0 as i32 - 20).max(0), 40);
        let win = gui.open_tabbed_window_xml(view_name, "Trade", &src, pos, WindowSize::Fixed(CLIENT.0, CLIENT.1))?;
        let mut docks = vec![("ShopInventoryDock", "shop_items", shop_h), ("PartnerInventoryDock", "bought_items", bought_h)];
        if cash_shop { docks.push(("OurInventoryDock", "sold_items", free - shop_h - bought_h)); }
        for (dock, view, h) in docks {
            gui.add_view_xml(win, dock, view, &list_xml(view, (dw, h)))?;
            gui.multi_add_column(win, view, 0, "", 16.0, 0);
            for (id, label, w) in COLUMNS {
                if view == "sold_items" && id == 3 { continue; } // Own list flags 0xb omit Price.
                gui.multi_add_column(win, view, id, label, w, COL_FLAGS);
            }
            gui.multi_sort(win, view, 1, false);
        }
        gui.set_text(win, "PartnerName", name);
        // `FUN_100e039a`: the credits field starts as "0" (read-only)
        gui.set_text(win, "PartnerCashView", "0");
        gui.clear_feature_flags(win, "PartnerCashView", 0xd);
        gui.set_text_color(win, "PartnerCashView", 0x44dd44);
        if cash_shop {
            gui.clear_feature_flags(win, "OurCashView", 0xd);
            gui.set_text_color(win, "OurCashView", 0x44dd44);
        }
        gui.relayout_window(win);
        gui.set_window_context(win, true);
        Ok(Shop { win, machine, stock, cash_shop, sold: vec![], bought: vec![], due: None, dirty: true, rows: Default::default(), literacy: 0, configs: Default::default(), grid_keys: Default::default(), grid_width: dw as f32 - 13.0, dock_registered: false })
    }

    fn apply_modes(&mut self, gui: &mut Gui) {
        for (i, view) in LISTS.iter().enumerate().take(if self.cash_shop { 3 } else { 2 }) {
            let list = self.configs[i].list.unwrap_or(false);
            gui.set_visible(self.win, view, list);
            gui.set_visible(self.win, &format!("{view}_grid_scroll"), !list);
        }
    }

    fn restore_config(&mut self, gui: &mut Gui) {
        for (i, view) in LISTS.iter().enumerate().take(if self.cash_shop { 3 } else { 2 }) {
            let columns: Vec<_> = self.configs[i].columns.iter().enumerate().map(|(n, &(id, width))| (id, width, Some(self.configs[i].col_flags.get(n).copied().unwrap_or(0) & 1 != 0))).collect();
            gui.multi_restore_columns(self.win, view, &columns);
            if let Some((col, order)) = self.configs[i].list_sort { gui.multi_sort(self.win, view, col, order != 0); }
        }
        self.apply_modes(gui);
    }

    /// Original auto-arranged 48-pixel inventory grid; tray one-row counts cap preferred height, not wrapping.
    fn paint_grid(&mut self, gui: &mut Gui, view: &str, rows: &[(i64, Option<GfxId>, String)]) {
        let Some(i) = LISTS.iter().position(|v| *v == view) else { return };
        let (cols, gap) = inv_grid::columns(self.grid_width - 1.0);
        let nrows = rows.len().max(1).div_ceil(cols);
        let height = 2.0 * inv_grid::BORDER + nrows as f32 * 48.0 + (nrows - 1) as f32 * inv_grid::SPACING;
        let canvas = format!("{view}_grid");
        gui.remove_children(self.win, &format!("{view}_grid_content"));
        let src = format!("<root><CanvasView name=\"{canvas}\" min_size=\"Point({},{height})\" max_size=\"Point({},{height})\"/></root>", self.grid_width, self.grid_width);
        if let Err(e) = gui.add_view_xml(self.win, &format!("{view}_grid_content"), &canvas, &src) { eprintln!("shop grid: {e:#}"); return; }
        let mut pictures = vec![];
        let mut tips = vec![];
        for (n, (_, icon, title)) in rows.iter().enumerate() {
            let (x, y) = inv_grid::cell_origin(n % cols, n / cols, (gap, inv_grid::SPACING));
            if let Some(id) = gui.gfx_id("GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED").map(GfxId) {
                let (w, h) = gui.gfx().size(id);
                pictures.push(CanvasItem::Image { id, src: [0.0, 0.0, w as f32, h as f32], dst: [x - 3.0, y - 3.0, x + 51.0, y + 51.0], alpha: 1.0 });
            }
            if let Some(id) = icon {
                let (w, h) = if id.0 >= ao_gui::EXTRA_BASE {
                    gui.extra_images().get((id.0 - ao_gui::EXTRA_BASE) as usize).map_or((0, 0), |image| (image.w, image.h))
                } else { gui.gfx().size(*id) };
                pictures.push(CanvasItem::Image { id: *id, src: [0.0, 0.0, w as f32, h as f32], dst: [x, y, x + 48.0, y + 48.0], alpha: 1.0 });
            }
            tips.push(CanvasTip { rect: [x, y, x + 48.0, y + 48.0], title: title.clone(), body: String::new() });
        }
        self.grid_keys[i] = rows.iter().map(|r| r.0).collect();
        gui.set_canvas(self.win, &canvas, pictures);
        gui.set_canvas_tips(self.win, &canvas, tips);
        self.apply_modes(gui);
    }

    fn grid_hit(&self, view: &str, x: f32, y: f32) -> Option<(&'static str, i64)> {
        let i = LISTS.iter().position(|v| view == format!("{v}_grid"))?;
        let (cols, gap) = inv_grid::columns(self.grid_width - 1.0);
        self.grid_keys[i].iter().enumerate().find_map(|(n, key)| {
            let (cx, cy) = inv_grid::cell_origin(n % cols, n / cols, (gap, inv_grid::SPACING));
            (x >= cx && x < cx + 48.0 && y >= cy && y < cy + 48.0).then_some((LISTS[i], *key))
        })
    }

    /// The stock entry an identity names.
    fn item(&self, id: Identity) -> Option<AcgItem> {
        (id.kind == SHOP_ITEM).then(|| self.stock.get(id.instance as usize).copied()).flatten()
    }
}

/// One list row: Name, Count, Price, Quality (`FUN_100404a9`: count = the template's count (stat 0x19c, 0 -> 1), price = `N3Msg_GetShopItemStat(item, 0x4a)`,
/// shown empty when 0; quality = the item's level).
fn cells(info: &Option<(String, i32, i32, Option<GfxId>)>, level: i32) -> Vec<MultiCell> {
    let (name, count, price, icon) = info.clone().unwrap_or_default();
    vec![icon.map(MultiCell::image).unwrap_or_else(|| MultiCell::unsorted("")), MultiCell::text(&name), MultiCell::num(count as i64), MultiCell { text: if price == 0 { String::new() } else { group(price) }, key: ao_gui::widgets::MultiKey::Num(price as i64), image: None }, MultiCell::num(level as i64)]
}

fn ordered_cells(mut cells: Vec<MultiCell>, columns: &[(i32, f32, bool)]) -> Vec<MultiCell> {
    let mut ids = [0, 1, 2, 3, 4];
    for (to, &(id, _, _)) in columns.iter().enumerate() {
        if let Some(from) = ids.iter().position(|&current| current == id) {
            ids.swap(to, from);
            cells.swap(to, from);
        }
    }
    cells.truncate(columns.len());
    cells
}

/// GC 0x10099954, verified against x87 instructions: truncation at each conversion;
/// constants GC 10158670=100, 101600f0=10, 1015df60=.25, 10158b98=-100.
fn buy_price(value: i32, factor: i32, shop_type: i32, literacy: i32) -> i32 {
    let base = (value as f64 * factor as f64 / 100.0) as i32;
    if shop_type != 0x3d { return base; }
    let discount = ((literacy.min(3000) as f64 / 10.0 * 0.25) as i32).min(90);
    base + (discount as f64 * base as f64 / -100.0) as i32
}

/// GC 10099d56: SellPrice 0x1aa, Value sentinel -> zero, truncated CL bonus (no buy-discount cap).
fn sell_price(value: i32, factor: i32, literacy: i32) -> i32 {
    let value = if value == 0x499602d2 { 0 } else { value };
    let base = (value as f64 * factor as f64 / 100.0) as i32;
    let bonus = (literacy.min(3000) as f64 / 10.0 * 0.25) as i32;
    base + (bonus as f64 * base as f64 / 100.0) as i32
}

type Info<'a> = &'a mut dyn FnMut(&mut Gui, AcgItem) -> Option<(String, i32, i32, Option<GfxId>)>;

impl Interact {
    /// Every decoded zone frame: the machine's name and stock (`ShopUpdateIIR_t::Activate`, `FUN_100a0f2c`: only for a `VendingMachine_t` header).
    pub(super) fn shop_watch(&mut self, m: &Message) {
        let t = m.header.target;
        let ui = &mut self.shop;
        match &m.body {
            N3::World(World::VendingMachine(v)) => {
                let name = v.base.blob.split(|&b| b == 0).next().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                let stat = |id| v.base.stats.iter().rev().find(|(s, _)| *s == id).map_or(0, |(_, v)| *v);
                let shop_type = stat(0x9c);
                ui.pricing.insert((t.kind, t.instance), (if shop_type == 0 { 0x3d } else { shop_type }, stat(0x1ab)));
                ui.sell_factors.insert((t.kind, t.instance), stat(0x1aa));
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
    pub(super) fn shop_trade(&mut self, gui: &mut Gui, t: &Trade, who: Identity, ptrade_open: bool, zone: &super::zone::Zone) -> bool {
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
                        let cash_shop = self.shop.pricing.get(&key).is_none_or(|p| p.0 == 0x3d);
                        match Shop::open(gui, self.screen, t.a, &name, stock, cash_shop) {
                            Ok(mut s) => { s.literacy = self.shop.literacy; s.configs = self.shop.configs.clone(); s.restore_config(gui); self.shop.shop = Some(s); }
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
            trade::ACCEPT | trade::RESET if who == me => {
                gui.set_enabled(s.win, "AcceptButton", t.op == trade::RESET);
                if t.op == trade::RESET { self.ptrade.shop_confirmation(gui, self.screen, false); }
            }
            trade::CONFIRM if who == me => self.ptrade.shop_confirmation(gui, self.screen, true),
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
            trade::ADD_ITEM | trade::VENDING_ADD if who == me && t.b.kind != SHOP_ITEM && s.cash_shop => {
                s.sold.push(super::interact_ptrade::Entry { item: t.b, acg: zone.inventory.get(&(t.b.instance as u32)).map(|e| e.item) });
                s.dirty = true;
            }
            trade::REMOVE_ITEM | trade::VENDING_REMOVE if who == me && t.b.kind != SHOP_ITEM && s.cash_shop => {
                s.sold.retain(|e| e.item != t.b);
                s.dirty = true;
            }
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
            trade::SET_CASH if who == me && s.cash_shop && t.a.instance >= 0 => {
                gui.set_text(s.win, "OurCashView", &group(t.a.instance));
            }
            _ => {}
        }
        true
    }

    /// The window is gone: `+0xe4(1)` (`SlotTradeCompleted` deletes the view).
    fn shop_close(&mut self, gui: &mut Gui) {
        self.ptrade.shop_confirmation(gui, self.screen, false);
        if let Some(s) = self.shop.shop.take() {
            let mut config = Element { name: "Archive".into(), attrs: vec![("name".into(), "shop_listview_config".into()), ("code".into(), "0".into())], children: vec![] };
            let mut list = s.configs[0].clone();
            let columns = gui.multi_columns(s.win, "shop_items");
            list.columns = columns.iter().map(|&(id, width, _)| (id, width)).collect();
            list.col_flags = columns.iter().map(|&(_, _, hidden)| u32::from(hidden)).collect();
            list.list_sort = gui.multi_sort_state(s.win, "shop_items").map(|(col, desc)| (col, i32::from(desc)));
            list.write_into(&mut config);
            self.shop.configs[0] = list;
            self.shop.pending_config = Some(config);
            gui.close_window(s.win);
        }
    }

    pub(super) fn shop_close_all(&mut self, gui: &mut Gui) {
        self.shop_close(gui);
    }

    /// GUI events of the window; `true` when consumed.
    pub(super) fn shop_event(&mut self, gui: &mut Gui, ev: &Event, zone: &super::zone::Zone) -> bool {
        let me = self.own_id();
        let quick = self.shop.quick;
        if let Event::CanvasPress { window, view, x, y, button, clicks } = ev {
            if let Some(s) = self.shop.shop.as_ref().filter(|s| s.win == *window) {
                if let Some((list, id)) = s.grid_hit(view, *x, *y) {
                    let event = Event::MultiMouse { window: *window, view: list.into(), id: Some(id), button: if *button == ao_gui::MouseButton::Left { 1 } else { 2 }, clicks: *clicks, x: *x as i32, y: *y as i32 };
                    return self.shop_event(gui, &event, zone);
                }
            }
        }
        let Some(s) = self.shop.shop.as_mut() else { return false };
        let mut out: Vec<Vec<u8>> = vec![];
        match ev {
            Event::CanvasClick { window, view, .. } if *window == s.win && view == "close" => out.push(trade::abort(me, true)),
            Event::ContextMenu { window, x, y, .. } | Event::FrameIcon { window, x, y } if *window == s.win => {
                // GUI 100dfad8: the trade window's ListMode entry controls the shop stock, not the bought/sold trays.
                gui.open_menu((*x, *y), (self.screen.0 as i32, self.screen.1 as i32), vec![MenuItem::check(MODE_MENU, &self.shop.list_mode_text, s.configs[0].list.unwrap_or(false))]);
            }
            Event::MenuPicked { id } if *id == MODE_MENU => {
                let mode = &mut s.configs[0].list;
                *mode = Some(!mode.unwrap_or(false));
                s.apply_modes(gui);
                s.dirty = true;
            }
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
            Event::TextChanged { window, .. } if *window == s.win => {}
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
                        "sold_items" if s.cash_shop => {
                            if let Some(e) = s.sold.get(id as usize) {
                                if (inventory::BAG_FIRST..inventory::BAG_FIRST + inventory::BAG_SLOTS).any(|slot| !zone.inventory.contains_key(&slot) || s.sold.iter().any(|e| e.item.instance as u32 == slot)) {
                                    out.push(trade::remove_item(me, me, e.item));
                                } else {
                                    self.shop.feedback.push("Feedback_NoRoomInInventory");
                                }
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


    pub(super) fn shop_drop(&mut self, gui: &Gui, zone: &super::zone::Zone, slot: u32, x: f32, y: f32) {
        let Some(s) = self.shop.shop.as_ref().filter(|s| s.cash_shop) else { return };
        if !LISTS.iter().enumerate().any(|(n, view)| {
            let view = if s.configs[n].list.unwrap_or(false) { (*view).to_owned() } else { format!("{view}_grid_scroll") };
            gui.view_rect(s.win, &view).is_some_and(|r| x >= r.l && x <= r.r && y >= r.t && y <= r.b)
        }) { return; }
        let item = inventory::item_identity(slot);
        if slot < inventory::BAG_FIRST || !zone.inventory.contains_key(&slot) || s.sold.iter().any(|e| e.item == item) { return; }
        self.send(trade::add_item(self.own_id(), item));
    }

    /// Fills lists and totals using the complete ACG item, including actual quality level.
    pub(super) fn shop_render(&mut self, gui: &mut Gui, info: Info) {
        let Some(s) = self.shop.shop.as_mut().filter(|s| s.dirty) else { return };
        let (shop_type, factor) = self.shop.pricing.get(&(s.machine.kind, s.machine.instance)).copied().unwrap_or((0x3d, 0));
        let literacy = s.literacy;
        s.dirty = false;
        let lists: [Vec<(i64, AcgItem)>; 2] = [
            s.stock.iter().enumerate().map(|(i, a)| (i as i64, *a)).collect(),
            s.bought.iter().enumerate().map(|(i, b)| (i as i64, s.item(*b).unwrap_or_default())).collect(),
        ];
        let mut due = 0i64;
        for (n, (view, list)) in ["shop_items", "bought_items"].into_iter().zip(lists).enumerate() {
            gui.multi_clear(s.win, view);
            s.rows[n].clear();
            let mut grid = vec![];
            let columns = gui.multi_columns(s.win, view);
            for (id, a) in list {
                let inf = info(gui, a).map(|(name, count, value, icon)| (name, count, buy_price(value, factor, shop_type, literacy), icon));
                if n == 1 {
                    due += inf.as_ref().map_or(0, |i| i.2 as i64 * i.1.max(1) as i64);
                }
                if let Some((name, _, _, icon)) = &inf { grid.push((id, *icon, name.clone())); }
                let c = cells(&inf, a.level);
                s.rows[n].push(format!("[{id}] {}", c.iter().skip(1).map(|c| c.text.as_str()).collect::<Vec<_>>().join(" | ")));
                gui.multi_add_row(s.win, view, id, ordered_cells(c, &columns), true);
            }
            if let Some((1, order)) = s.configs[n].grid_sort {
                grid.sort_by(|a, b| if order == 0 { a.2.cmp(&b.2) } else { b.2.cmp(&a.2) });
            }
            s.paint_grid(gui, view, &grid);
        }
        if s.cash_shop {
            let sell_factor = self.shop.sell_factors.get(&(s.machine.kind, s.machine.instance)).copied().unwrap_or(0);
            let mut revenue = 0i64;
            gui.multi_clear(s.win, "sold_items");
            let mut grid = vec![];
            let columns = gui.multi_columns(s.win, "sold_items");
            for (id, e) in s.sold.iter().enumerate() {
                if let Some(a) = e.acg {
                    let inf = info(gui, a).map(|(name, count, value, icon)| {
                        revenue += sell_price(value, sell_factor, literacy) as i64;
                        (name, count, 0, icon)
                    });
                    if let Some((name, _, _, icon)) = &inf { grid.push((id as i64, *icon, name.clone())); }
                    let c = cells(&inf, a.level);
                    gui.multi_add_row(s.win, "sold_items", id as i64, ordered_cells(c, &columns), true);
                }
            }
            if let Some((1, order)) = s.configs[2].grid_sort {
                grid.sort_by(|a, b| if order == 0 { a.2.cmp(&b.2) } else { b.2.cmp(&a.2) });
            }
            s.paint_grid(gui, "sold_items", &grid);
            let balance = (revenue - due).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            gui.set_text(s.win, "OurCashView", &group(balance));
            gui.set_text_color(s.win, "OurCashView", if balance < 0 { 0xdd4444 } else { 0x44dd44 });
        }
        // `FUN_100dfbf3` (`+0xd8`): the credits field shows the cost of the bought items (`FUN_10099d56`: price x count of every one); the machine's own figure wins
        gui.set_text(s.win, "PartnerCashView", &group(s.due.unwrap_or(due.min(i32::MAX as i64) as i32)));
    }
}

impl super::Play {
    /// Per frame, after [`Play::interact_ptrade_frame`]: fills the shop lists, prints the feedback, sends what the window produced.
    pub(super) fn interact_shop_frame(&mut self) {
        let Some(i) = self.interact.as_mut() else { return };
        i.shop.literacy = self.zone.skill_value(0xa1).unwrap_or(0).min(3000);
        if let Some(h) = self.hud.as_mut() {
            let first_config = !i.shop.config_loaded;
            i.shop.load_config(&h.dvalues);
            if first_config {
                if let Some(s) = i.shop.shop.as_mut() {
                    s.configs = i.shop.configs.clone();
                    s.restore_config(&mut self.gui);
                    s.dirty = true;
                }
            }
            i.shop.save_config(&mut h.dvalues);
            i.shop.list_mode_text = self.text.by_key(10000, "ListMode").unwrap_or_default();
            if let Some(s) = i.shop.shop.as_mut().filter(|s| !s.dock_registered) {
                if let Err(e) = h.register_transient_dock(&mut self.gui, s.win) { eprintln!("shop docking: {e:#}"); }
                s.dock_registered = true;
            }
            i.shop_render(&mut self.gui, &mut |gui, item| h.shop_info(gui, item));
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
            "shop {:?} ({:?})\n  stock ({}): {:?}\n  bought ({}): {:?}\n  sold: {:?}  balance: {:?}\n  credits: {:?}  accept enabled: {}\n  log: {:?}",
            gui.text(s.win, "PartnerName"),
            s.machine,
            s.stock.len(),
            s.rows[0],
            s.bought.len(),
            s.rows[1],
            s.sold,
            gui.text(s.win, "OurCashView"),
            gui.text(s.win, "PartnerCashView"),
            gui.is_enabled(s.win, "AcceptButton"),
            self.shop.log
        )
    }

    fn shop_double_click(&mut self, gui: &mut Gui, view: &str, index: usize, quick: bool) -> bool {
        let Some(win) = self.shop.shop.as_ref().map(|s| s.win) else { return false };
        self.shop.quick = quick;
        let ev = Event::MultiMouse { window: win, view: view.into(), id: Some(index as i64), button: 1, clicks: 2, x: 0, y: 0 };
        self.shop_event(gui, &ev, &super::zone::Zone::new(self.own))
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
        self.shop_event(gui, &Event::Clicked { window: win, view: view.into(), item: None }, &super::zone::Zone::new(self.own))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::zone::Zone;
    use ao_net::frame::Frame;

    #[test]
    fn shop_config_saves_stock_mode_and_retains_partner_archive() {
        let mut d = DValues::default();
        d.add("ShopViewConfig", Variant::Archive("<Archive name=\"ShopViewConfig\"><Archive name=\"partner_listview_config\"><Bool name=\"listview_mode\" value=\"true\"/></Archive><Archive name=\"shop_listview_config\"><Bool name=\"listview_mode\" value=\"false\"/></Archive></Archive>".into()), true, super::super::dvalue::CAT_CHAR, None, None, false);
        let mut ui = ShopUi::default();
        ui.load_config(&d);
        assert_eq!(ui.configs[0].list, Some(false));
        assert_eq!(ui.configs[1].list, Some(true));
        let mut config = Element { name: "Archive".into(), attrs: vec![("name".into(), "shop_listview_config".into())], children: vec![] };
        ListCfg { list: Some(true), list_sort: Some((1, 0)), ..Default::default() }.write_into(&mut config);
        ui.pending_config = Some(config);
        ui.save_config(&mut d);
        let mut restored = ShopUi::default();
        restored.load_config(&d);
        assert_eq!(restored.configs[0].list, Some(true));
        assert_eq!(restored.configs[0].list_sort, Some((1, 0)));
        assert_eq!(restored.configs[1].list, Some(true));
    }

    #[test]
    fn original_buy_price_applies_factor_then_truncated_literacy_discount() {
        assert_eq!(buy_price(101, 150, 0x3c, 3000), 151);
        assert_eq!(buy_price(101, 150, 0x3d, 39), 151);
        assert_eq!(buy_price(101, 150, 0x3d, 40), 150);
        assert_eq!(buy_price(100, 100, 0x3d, 3000), 25);
        assert_eq!(buy_price(100, 100, 0x3d, 9000), 25);
        assert_eq!(sell_price(101, 150, 40), 152);
        assert_eq!(sell_price(100, 100, 3000), 175);
        assert_eq!(sell_price(0x499602d2, 100, 3000), 0);
    }
    #[test]
    fn original_icon_column_precedes_text_columns() {
        let icon = GfxId(ao_gui::EXTRA_BASE);
        let c = cells(&Some(("Item".into(), 2, 123, Some(icon))), 73);
        assert_eq!(c[0].image, Some(icon));
        assert_eq!(c.iter().skip(1).map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Item", "2", "123", "73"]);
        assert_eq!(COLUMNS.map(|c| c.2), [200.0, 30.0, 100.0, 100.0]);
        let reordered = ordered_cells(c, &[(4, 100.0, false), (1, 200.0, false), (0, 16.0, false), (2, 30.0, true)]);
        assert_eq!(reordered[0].text, "73");
        assert_eq!(reordered[1].text, "Item");
        assert_eq!(reordered[2].image, Some(icon));
        assert_eq!(reordered[3].text, "2");
    }

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
    fn info(_: &mut Gui, item: AcgItem) -> Option<(String, i32, i32, Option<GfxId>)> {
        Some((format!("Item{}", item.low_id), 1, item.low_id * 10, None))
    }

    fn sent(i: &mut Interact) -> Vec<Vec<u8>> {
        i.take_outbox().into_iter().map(|f| f.payload).collect()
    }

    /// The machine's stock and the trade start as the live server sent them, then the window.
    fn open(gui: &mut Gui, z: &Zone) -> Interact {
        let mut i = Interact::new(OWN, (1280, 800));
        i.shop.pricing.insert((MACHINE.kind, MACHINE.instance), (0x3c, 100));
        i.on_frame(gui, &server(ShopUpdate { items: items() }.encode(MACHINE)), z);
        i.on_frame(gui, &server(Trade { op: trade::START, a: MACHINE, b: SESSION }.encode(ME)), z);
        i.on_frame(gui, &server(Trade { op: trade::START, a: ME, b: SESSION }.encode(MACHINE)), z);
        i.shop_render(gui, &mut info);
        i
    }

    #[test]
    fn shop_resolves_both_endpoints_at_actual_quality() {
        let Some(mut gui) = rig() else { return };
        let z = Zone::new(OWN);
        let mut i = open(&mut gui, &z);
        let item = AcgItem { low_id: 111, high_id: 222, level: 73 };
        let s = i.shop.shop.as_mut().unwrap();
        s.stock = vec![item];
        s.bought = vec![Identity { kind: SHOP_ITEM, instance: 0 }];
        s.dirty = true;
        let mut calls = 0;
        i.shop_render(&mut gui, &mut |_, acg| {
            assert_eq!(acg, item);
            calls += 1;
            Some(("Interpolated".into(), 1, acg.level * 10, None))
        });
        assert_eq!(calls, 2);
        assert!(i.shop_dump(&gui).contains("credits: \"730\""));
    }

    #[test]
    fn cash_shop_has_sold_items_and_cash_controls() {
        let Some(mut gui) = rig() else { return };
        let mut z = Zone::new(OWN);
        let slot = inventory::BAG_FIRST;
        let item = inventory::item_identity(slot);
        z.inventory.insert(slot, ao_net::n3::world::InventoryEntry { slot, a: 0, b: 0, id: item, item: items()[0] });
        let mut i = Interact::new(OWN, (1280, 800));
        i.on_trade(&mut gui, Trade { op: trade::START, a: MACHINE, b: SESSION }, ME, &z);
        let win = i.shop.shop.as_ref().unwrap().win;
        for view in ["OurInventoryDock", "OurCashView", "sold_items"] { assert!(gui.has_view(win, view)); }
        assert!(gui.is_enabled(win, "OurCashView"), "readonly shop cash retains normal visual styling");
        i.shop.sell_factors.insert((MACHINE.kind, MACHINE.instance), 100);
        i.shop_render(&mut gui, &mut info);
        // Original default is grid mode: the hidden native list has no live drop geometry.
        let r = gui.view_rect(win, "sold_items_grid_scroll").unwrap();
        assert!(r.r > r.l && r.b > r.t, "the active shop tray must retain its dock area: {r:?}");
        i.ptrade_drops(&gui, &z, vec![(slot, (r.l + r.r) / 2.0, (r.t + r.b) / 2.0)]);
        assert_eq!(sent(&mut i), [trade::add_item(ME, item)]);
        i.on_trade(&mut gui, Trade { op: trade::ADD_ITEM, a: ME, b: item }, ME, &z);
        assert_eq!(i.shop.shop.as_ref().unwrap().sold[0].acg, Some(items()[0]));
        i.shop_render(&mut gui, &mut info);
        assert_eq!(gui.text(win, "OurCashView"), "1,110");
        i.shop_event(&mut gui, &Event::MultiMouse { window: win, view: "sold_items".into(), id: Some(0), button: 1, clicks: 2, x: 0, y: 0 }, &z);
        assert_eq!(sent(&mut i), [trade::remove_item(ME, ME, item)]);
        i.on_trade(&mut gui, Trade { op: trade::REMOVE_ITEM, a: ME, b: item }, ME, &z);
        assert!(i.shop.shop.as_ref().unwrap().sold.is_empty());
        i.on_trade(&mut gui, Trade { op: trade::RESET, a: ZERO, b: ZERO }, ME, &z);
        assert!(gui.is_enabled(win, "AcceptButton"));
        i.shop_press(&mut gui, true);
        assert_eq!(sent(&mut i), [trade::accept(ME)]);
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
