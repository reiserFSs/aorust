//! The container (loot) window of corpses and chests: `InventoryView_c` of type 1 (`FUN_100cc2ca(1, container)` [GUI 0x100cc2ca], opened through
//! `GlobalSignals +0x84` -> `InventoryGUIModule_c::SlotContainerOpened` [GUI 0x100c71a3]). Evidence and the unresolved parts: docs/zone/interact.md §9.

use super::hud_stats::inv_grid::{cell_origin, BORDER, ICON as CELL, SPACING};
use super::hud_stats::items::{Items, SLOT};
use ao_gui::{CanvasItem, CanvasTip, Event, Gui, WindowId, WindowSize};
use ao_gui::widgets::MultiCell;
use ao_net::msg::Identity;
use ao_net::n3::inventory::{container_item_identity, InventoryUpdate};
use ao_net::n3::world::InventoryEntry;
use std::path::Path;

/// `SetMaxItemCount(0x15)` of a container view (docs/gui.md §11.5) and the `item_position_map` loop of `FUN_100cc2ca` (`n % 3`, `n / 3`, `n < 0x15`).
const SLOTS: usize = 0x15;
const COLUMNS: usize = 3;
const SLOT_GFX: &str = "GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED";

struct Loot {
    window: WindowId,
    escape: bool,
    /// ItemContainerView +0x14c, captured by the native constructor.
    owned: bool,
    container: Identity,
    /// `word` of the last `InventoryUpdateIIR_t` (chest `+0x1dc`): the key of the items' identities (`container_item_identity`).
    word: i32,
    /// The items in the order of their cells (by container slot).
    entries: Vec<InventoryEntry>,
    /// GUI 0x100cdb3a: bank 0x66; ordinary chest 0x15.
    slots: usize,
    /// The cell of the last short click and when (`Interact::now`), for the double click.
    clicked: Option<(usize, f32)>,
    tip: Option<(usize, bool, String)>,
}

/// `(container, [(container slot, item low id, name)])` of every open window, for the harness and the tests.
#[cfg(test)]
pub type LootRows = Vec<(Identity, Vec<(u32, i32, String)>)>;

#[derive(Clone, Copy)]
struct ContainerDrag {
    source: WindowId,
    item: Identity,
    px: f32,
    py: f32,
    ghost: Option<WindowId>,
}

/// The open container windows (one per container).
#[derive(Default)]
pub struct LootUi {
    items: Option<Items>,
    open: Vec<Loot>,
    escape_options: Option<[bool; 3]>,
    pub(super) info_click: bool,
    pub(super) info_urls: Vec<String>,
    drag: Option<ContainerDrag>,
    drops: Vec<(Identity, f32, f32)>,
}

fn gap() -> (f32, f32) {
    (SPACING, SPACING)
}

/// The size of the grid canvas: `rows` rows of cells with the default spacing and the view borders (`RecalcCellCount`, docs/gui.md §11.5).
fn canvas_size_for(slots: usize) -> (u32, u32) {
    let rows = slots.div_ceil(COLUMNS) as f32;
    let (w, h) = (2.0 * BORDER + COLUMNS as f32 * (CELL + 1.0) + (COLUMNS as f32 - 1.0) * gap().0, 2.0 * BORDER + rows * (CELL + 1.0) + (rows - 1.0) * gap().1);
    (w as u32, h as u32)
}

#[cfg(test)]
fn canvas_size() -> (u32, u32) { canvas_size_for(SLOTS) }

/// Top-left corner of the 54 px slot art of cell `n` (the art is centred on the 48 px cell, as in the inventory window).
fn slot_origin(n: usize) -> (f32, f32) {
    let (x, y) = cell_origin(n % COLUMNS, n / COLUMNS, gap());
    let o = (SLOT - CELL - 1.0) / 2.0;
    (x - o, y - o)
}

/// The cell under the canvas position.
fn cell_at_for(x: f32, y: f32, slots: usize) -> Option<usize> {
    (0..slots).find(|&n| {
        let (ox, oy) = cell_origin(n % COLUMNS, n / COLUMNS, gap());
        x >= ox && x < ox + CELL + 1.0 && y >= oy && y < oy + CELL + 1.0
    })
}

#[cfg(test)]
fn cell_at(x: f32, y: f32) -> Option<usize> { cell_at_for(x, y, SLOTS) }

impl LootUi {
    pub(super) fn configure_escape(&mut self, values: &super::dvalue::DValues) {
        self.escape_options = Some(["esc_backpacks", "esc_chests", "esc_corpses"].map(|key| values.get_i64(key).unwrap_or(1) != 0));
    }
    pub(super) fn uses_in_place(&self, item: Identity) -> bool {
        item.kind == ao_net::n3::inventory::KIND_IN_CONTAINER &&
            self.open.iter().any(|l| l.owned && i32::from(l.word as i16) == item.instance >> 16)
    }

    pub(super) fn item_title(&mut self, gui: &mut Gui, dir: Option<&Path>, low_id: i32) -> Option<String> {
        if self.items.is_none() { self.items = dir.map(Items::new); }
        self.items.as_mut()?.info(gui, low_id).map(|info| info.name.clone())
    }
    pub fn container_drop(&self, gui: &Gui, x: f32, y: f32) -> Option<Identity> {
        self.open.iter().find(|l| (l.container.kind == 0xdead || l.owned)
            && gui.pointer_owner(x, y) == Some(l.window)
            && gui.view_rect(l.window, if l.container.kind == 0xdead { "scrollview" } else { "grid" })
                .is_some_and(|r| x >= r.l && x < r.r && y >= r.t && y < r.b)).map(|l| l.container)
    }

    pub fn take_drops(&mut self) -> Vec<(Identity, f32, f32)> { std::mem::take(&mut self.drops) }

    pub fn refresh_containers(&mut self, gui: &mut Gui, zone: &super::zone::Zone) {
        for at in 0..self.open.len() {
            let c = self.open[at].container;
            let Some((_, entries)) = zone.containers.get(&(c.kind, c.instance)) else { continue };
            if self.open[at].entries != *entries {
                self.open[at].entries = entries.clone();
                self.open[at].entries.sort_by_key(|e| e.slot);
                self.open[at].tip = None;
                self.draw(gui, at);
            }
        }
    }

    pub fn close_bank(&mut self, gui: &mut Gui) {
        if let Some(at) = self.open.iter().position(|l| l.container.kind == 0xdead) {
            if self.drag.is_some_and(|d| d.source == self.open[at].window) {
                if let Some(ghost) = self.drag.take().and_then(|d| d.ghost) { gui.close_window(ghost); }
            }
            gui.close_window(self.open.remove(at).window);
        }
    }
    #[cfg(test)]
    pub fn is_open(&self, container: Identity) -> bool {
        self.open.iter().any(|l| l.container == container)
    }

    #[cfg(test)]
    pub fn window_of(&self, container: Identity) -> Option<WindowId> {
        self.open.iter().find(|l| l.container == container).map(|l| l.window)
    }

    pub fn close_all(&mut self, gui: &mut Gui) {
        if let Some(ghost) = self.drag.take().and_then(|d| d.ghost) { gui.close_window(ghost); }
        self.drops.clear();
        for l in self.open.drain(..) {
            gui.close_window(l.window);
        }
    }

    /// `InventoryUpdateIIR_t` for `container` [GC 0x100a040e]: the contents are replaced; with `u.flag` the container is opened (a window appears
    /// unless one is open already, `SlotContainerOpened`), without it only an open window is refreshed (`+0x8c`, container changed).
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, gui: &mut Gui, dir: Option<&Path>, screen: (u32, u32), container: Identity, title: &str, u: &InventoryUpdate, owned: bool) {
        let slots = match container.kind { 0xdead => 0x66, 0xdeaa => 100, _ => SLOTS };
        if self.items.is_none() {
            self.items = dir.map(Items::new);
        }
        let mut entries = u.entries.clone();
        entries.sort_by_key(|e| e.slot);
        entries.truncate(slots);
        let at = match self.open.iter().position(|l| l.container == container) {
            Some(i) => i,
            None if u.flag => {
                let (w, h) = canvas_size_for(slots);
                let xml = if container.kind == 0xdeaa {
                    // GUI100cdb3a: contracts force name-sorted list mode, max100.
                    format!("<root><MultiListView name=\"contracts\" feature_flags=\"64\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"auto\" min_size=\"Point({w},{})\" max_size=\"Point(16000,16000)\"/></root>", canvas_size_for(SLOTS).1)
                } else if container.kind == 0xdead {
                    format!("<root><ScrollView name=\"scrollview\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"auto\" min_size=\"Point({w},1)\" max_size=\"Point(16000,16000)\"><ScrollViewChild view_layout=\"vertical\"><CanvasView name=\"grid\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/></ScrollViewChild></ScrollView></root>")
                } else {
                    format!("<root><View view_layout=\"vertical\"><CanvasView name=\"grid\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/></View></root>")
                };
                // [UNRESOLVED guess] bank's saved InventoryView frame is not loaded here:
                // reuse the existing three-column viewport until bank config persistence is ported.
                // Searched GUI 0x100cc2ca/0x100cdb3a and shipped CharPrefs.xml; capacity, not initial viewport, is fixed by code.
                let size = if container.kind == 0xdead { WindowSize::Fixed(w, canvas_size_for(SLOTS).1) } else { WindowSize::Preferred };
                let Ok(window) = gui.open_tabbed_window_xml("LootView", title, &xml, (0, 0), size) else { return };
                let (ow, oh) = gui.outer_size(window);
                // `Window::MoveToCenter` is the default of a window without a saved frame (`container_position` is not stored)
                gui.set_window_pos(window, ((screen.0 as i32 - ow as i32) / 2, (screen.1 as i32 - oh as i32) / 2));
                if container.kind == 0xdeaa {
                    for (id, name, width, flags) in [(0, "Icon", 16.0, 8), (1, "Name", 200.0, 0xe), (2, "Count", 30.0, 0xe), (3, "Price", 100.0, 0xe), (4, "Quality", 100.0, 0xe)] {
                        gui.multi_add_column(window, "contracts", id, name, width, flags);
                    }
                    gui.multi_sort(window, "contracts", 1, false);
                }
                // GUI 100cc2ca: owned Chest_t uses esc_backpacks, world Chest_t esc_chests, Corpse_t esc_corpses.
                let policy = self.escape_options.unwrap_or([true; 3]);
                let escape = match container.kind { 0xc749 => policy[usize::from(!owned)], 0xc76a => policy[2], _ => false };
                self.open.push(Loot { window, escape, owned, container, word: u.word, slots, entries: vec![], clicked: None, tip: None });
                self.open.len() - 1
            }
            None => return,
        };
        self.open[at].entries = entries;
        self.open[at].word = u.word;
        self.open[at].tip = None;
        self.draw(gui, at);
    }

    fn draw(&mut self, gui: &mut Gui, at: usize) {
        if self.open[at].container.kind == 0xdeaa {
            let window = self.open[at].window;
            gui.multi_clear(window, "contracts");
            for entry in &self.open[at].entries {
                let info = self.items.as_mut().and_then(|items| items.info(gui, entry.item.low_id));
                let name = info.as_ref().map(|i| i.name.as_str()).unwrap_or("");
                let icon = info.as_ref().and_then(|i| i.icon).map(|(id, _, _)| MultiCell::image(id)).unwrap_or_else(|| MultiCell::unsorted(""));
                gui.multi_add_row(window, "contracts", entry.slot as i64, vec![icon, MultiCell::text(name), MultiCell::num(i32::from(entry.b as u16).max(1) as i64), MultiCell::unsorted(""), MultiCell::num(entry.item.level as i64)], true);
            }
            return;
        }
        let slot_gfx = gui.gfx_id(SLOT_GFX).map(ao_gui::GfxId);
        let (mut cmds, mut tips) = (vec![], vec![]);
        for n in 0..self.open[at].slots {
            let (x, y) = slot_origin(n);
            if let Some(g) = slot_gfx {
                cmds.push(CanvasItem::Image { id: g, src: [0.0, 0.0, SLOT, SLOT], dst: [x, y, x + SLOT, y + SLOT], alpha: 1.0 });
            }
            let Some(e) = self.open[at].entries.get(n).copied() else { continue };
            let Some(items) = self.items.as_mut() else { continue };
            cmds.extend(items.picture(gui, &e, x, y));
            if let Some(info) = items.info(gui, e.item.low_id) {
                let body = self.open[at].tip.as_ref().filter(|(cell, _, _)| *cell == n).map_or("", |(_, _, body)| body.as_str());
                tips.push(CanvasTip { rect: [x, y, x + SLOT, y + SLOT], title: info.name.clone(), body: body.to_owned() });
            }
        }
        let w = self.open[at].window;
        gui.set_canvas(w, "grid", cmds);
        gui.set_canvas_tips(w, "grid", tips);
    }

    /// The native InventoryListView asks `100384f3` for long tooltip HTML only for the hovered item.
    pub(super) fn update_tooltips(&mut self, gui: &mut Gui, zone: &super::zone::Zone, text: &ao_formats::screens::TextDb, descriptions: bool) {
        let (x, y) = gui.mouse_position();
        for at in 0..self.open.len() {
            let loot = &self.open[at];
            if gui.view_rect(loot.window, "scrollview").is_some_and(|r| x < r.l || x >= r.r || y < r.t || y >= r.b) {
                self.open[at].tip = None;
                continue;
            }
            let cell = gui.view_rect(loot.window, "grid").filter(|r| gui.window_visible(loot.window) && x >= r.l && x < r.r && y >= r.t && y < r.b)
                .and_then(|r| cell_at_for(x - r.l, y - r.t, loot.slots));
            let Some((cell, entry)) = cell.and_then(|cell| loot.entries.get(cell).copied().map(|e| (cell, e))) else {
                self.open[at].tip = None;
                continue;
            };
            if loot.tip.as_ref().is_some_and(|(old, enabled, _)| *old == cell && *enabled == descriptions) { continue; }
            let body = if descriptions {
                let request = super::chat::ItemRequest::Identity { id: container_item_identity(loot.word, entry.slot), shop: false, container: loot.container };
                match super::chat::item_html(zone, &request, text) {
                    Ok(body) => body.unwrap_or_default(),
                    Err(error) => { eprintln!("container tooltip: {error:#}"); String::new() }
                }
            } else { String::new() };
            let origin = slot_origin(cell);
            let tip = gui.canvas_tips(loot.window, "grid").iter().position(|t| t.rect[0] == origin.0 && t.rect[1] == origin.1);
            gui.set_canvas_tip_body(loot.window, "grid", tip, &body);
            self.open[at].tip = Some((cell, descriptions, body));
        }
    }

    /// `ContainerAddItemIIR_t` for `{0x6b, key << 16 | slot}` into our bag (`FUN_1004a7b3`: the container cell is emptied, `FUN_100490cd` refreshes the
    /// view): the item leaves the open window whose last update carried `word == key` (as a signed short).
    pub fn taken(&mut self, gui: &mut Gui, item: Identity) {
        let (key, slot) = (item.instance >> 16, (item.instance & 0xffff) as u32);
        let Some(at) = self.open.iter().position(|l| i32::from(l.word as i16) == key && l.entries.iter().any(|e| e.slot == slot)) else { return };
        self.open[at].entries.retain(|e| e.slot != slot);
        self.open[at].tip = None;
        self.draw(gui, at);
    }

    /// `(container, entry.id)` of cell `n` of the first window (live harness).
    #[cfg(test)]
    pub fn entry_id(&self, n: usize) -> Option<Identity> {
        self.open.first()?.entries.get(n).map(|e| e.id)
    }

    /// The double click on cell `n` of the first window (live harness): the item to take.
    #[cfg(test)]
    pub fn take(&mut self, gui: &mut Gui, n: usize, now: f32) -> Option<Identity> {
        let window = self.open.first()?.window;
        let (x, y) = cell_origin(n % 3, n / 3, gap());
        let click = Event::CanvasClick { window, view: "grid".into(), x: x + 2.0, y: y + 2.0 };
        self.event(gui, &click, now);
        self.event(gui, &click, now + 0.1).flatten()
    }

    /// GUI events: `None` = not a loot window's, `Some(None)` = handled, `Some(Some(item))` = the second click on an item cell within the double-click
    /// time (`FUN_100ca1e7` -> `MoveItemToInventory(item)`): the item to take.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event, now: f32) -> Option<Option<Identity>> {
        match ev {
            Event::MultiMouse { window, view, id, button, clicks, .. } if view == "contracts" => {
                let Some(id) = *id else { return Some(None) };
                let loot = self.open.iter().find(|l| l.window == *window)?;
                let entry = loot.entries.iter().find(|e| e.slot as i64 == id)?;
                let item = Identity { kind: ao_net::n3::inventory::KIND_CONTRACT, instance: entry.slot as i32 };
                if self.info_click && *button == 1 {
                    self.info_urls.push(format!("itemid://{}/{}", item.kind, item.instance));
                    Some(None)
                } else {
                    Some(((*button == 1 && *clicks >= 2) || *button == 2).then_some(item))
                }
            }
            Event::Escape { window } if self.open.iter().any(|l| l.window == *window && !l.escape) => Some(None),
            Event::CanvasPress { window, view, x, y, button, .. } if view == "grid" => {
                let l = self.open.iter_mut().find(|l| l.window == *window)?;
                let Some(n) = cell_at_for(*x, *y, l.slots).filter(|&n| n < l.entries.len()) else { return Some(None) };
                let item = if l.container.kind == 0xdead { Identity { kind: 0x69, instance: l.entries[n].slot as i32 } } else { container_item_identity(l.word, l.entries[n].slot) };
                if *button == ao_gui::MouseButton::Left && self.info_click {
                    l.clicked = None;
                    self.info_urls.push(format!("itemid://{}/{}", item.kind, item.instance));
                    Some(None)
                } else if *button == ao_gui::MouseButton::Right {
                    l.clicked = None;
                    Some(Some(item))
                } else {
                    if *button == ao_gui::MouseButton::Left {
                        if let Some(r) = gui.view_rect(l.window, "grid") {
                            self.drag = Some(ContainerDrag { source: *window, item, px: r.l + *x, py: r.t + *y, ghost: None });
                        }
                    }
                    Some(None)
                }
            }
            Event::CanvasDrag { window, view, x, y, .. } if view == "grid" => {
                let ContainerDrag { source, item, px, py, ghost } = self.drag?;
                if source != *window { return None; }
                let r = gui.view_rect(source, "grid")?;
                let (sx, sy) = (r.l + *x, r.t + *y);
                let ghost = if let Some(ghost) = ghost {
                    gui.set_window_pos(ghost, (sx as i32 - 16, sy as i32 - 16));
                    Some(ghost)
                } else if (sx - px).hypot(sy - py) > 3.0 {
                    // Match the carried inventory icon: native drag hotspot (16,16),
                    // with the existing inventory's unresolved 32px ghost-size convention.
                    let l = self.open.iter_mut().find(|l| l.window == source)?;
                    let entry = l.entries.iter().find(|e| e.slot == (item.instance & 0xffff) as u32)?;
                    l.clicked = None;
                    let xml = "<root><CanvasView name=\"icon\" min_size=\"Point(32,32)\" max_size=\"Point(32,32)\"/></root>";
                    let ghost = gui.open_window_xml("ItemDrag", xml, (sx as i32 - 16, sy as i32 - 16), WindowSize::Preferred).ok()?;
                    if let Some((g, w, h)) = self.items.as_mut().and_then(|items| items.info(gui, entry.item.low_id)).and_then(|info| info.icon) {
                        gui.set_canvas(ghost, "icon", vec![CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, 32.0, 32.0], alpha: 1.0 }]);
                    }
                    Some(ghost)
                } else { None };
                self.drag = Some(ContainerDrag { source, item, px, py, ghost });
                Some(None)
            }
            Event::CanvasRelease { window, view } if view == "grid" && self.drag.is_some_and(|d| d.source == *window) => {
                if let Some(ContainerDrag { item, ghost: Some(ghost), .. }) = self.drag.take() {
                    gui.close_window(ghost);
                    let (x, y) = gui.mouse_position();
                    self.drops.push((item, x, y));
                }
                Some(None)
            }
            Event::CanvasClick { window, view, x, y } if view == "grid" => {
                let l = self.open.iter_mut().find(|l| l.window == *window)?;
                if self.info_click { return Some(None); }
                let Some(n) = cell_at_for(*x, *y, l.slots).filter(|&n| n < l.entries.len()) else { return Some(None) };
                let double = l.clicked.is_some_and(|(c, t)| c == n && now - t <= ao_gui::DOUBLE_CLICK_TIME);
                l.clicked = if double { None } else { Some((n, now)) };
                Some(double.then(|| if l.container.kind == 0xdead { Identity { kind: 0x69, instance: l.entries[n].slot as i32 } } else { container_item_identity(l.word, l.entries[n].slot) }))
            }
            Event::CloseRequested { window } | Event::Escape { window } if self.open.iter().any(|l| l.window == *window) => {
                let i = self.open.iter().position(|l| l.window == *window)?;
                if self.drag.is_some_and(|d| d.source == *window) {
                    if let Some(ghost) = self.drag.take().and_then(|d| d.ghost) { gui.close_window(ghost); }
                }
                gui.close_window(self.open.remove(i).window);
                Some(None)
            }
            _ => None,
        }
    }

    /// The window's items as the harness prints them: `(container, [(slot, low id, name)])`.
    #[cfg(test)]
    pub fn dump(&mut self, gui: &mut Gui) -> LootRows {
        let mut out = vec![];
        for l in &self.open {
            let rows = l.entries.iter().map(|e| (e.slot, e.item.low_id, self.items.as_mut().and_then(|i| i.info(gui, e.item.low_id).map(|i| i.name.clone())).unwrap_or_default())).collect();
            out.push((l.container, rows));
        }
        out
    }
}

#[cfg(test)]
mod escape_tests {
    use super::*;

    #[test]
    fn container_escape_policy_is_captured_at_open() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut values = super::super::dvalue::DValues::new(&dir);
        for (kind, owned, key) in [(0xc749, true, "esc_backpacks"), (0xc749, false, "esc_chests"), (0xc76a, false, "esc_corpses")] {
            let mut ui = LootUi::default();
            let container = Identity { kind, instance: 7 };
            let update = InventoryUpdate { capacity: 21, access: 2, entries: vec![], container, word: 0, flag: true };
            values.set_i64(key, 0);
            ui.configure_escape(&values);
            ui.update(&mut gui, None, (800, 600), container, "Container", &update, owned);
            let win = ui.window_of(container).unwrap();
            values.set_i64(key, 1);
            ui.configure_escape(&values);
            ui.event(&mut gui, &Event::Escape { window: win }, 0.0);
            assert!(ui.is_open(container));
            ui.event(&mut gui, &Event::CloseRequested { window: win }, 0.0);
            ui.update(&mut gui, None, (800, 600), container, "Container", &update, owned);
            let win = ui.window_of(container).unwrap();
            values.set_i64(key, 0);
            ui.configure_escape(&values);
            ui.event(&mut gui, &Event::Escape { window: win }, 0.0);
            assert!(!ui.is_open(container));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::world::AcgItem;

    #[test]
    fn organisation_contracts_use_sorted_list_and_native_slot_identities() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut ui = LootUi::default();
        let container = Identity { kind: 0xdeaa, instance: 9 };
        let mut update = update(true, &[29]);
        update.container = container;
        update.access = 3;
        ui.update(&mut gui, None, (1280, 800), container, "", &update, false);
        let window = ui.window_of(container).unwrap();
        assert_eq!(ui.open[0].slots, 100);
        assert_eq!(gui.multi_sort_state(window, "contracts"), Some((1, false)));
        assert!(gui.has_view(window, "contracts") && !gui.has_view(window, "grid"));
        assert_eq!(ui.event(&mut gui, &Event::MultiMouse { window, view: "contracts".into(), id: Some(29), button: 1, clicks: 2, x: 0, y: 0 }, 0.0), Some(Some(Identity { kind: ao_net::n3::inventory::KIND_CONTRACT, instance: 29 })));
        assert_eq!(ui.container_drop(&gui, 0.0, 0.0), None);
    }

    #[test]
    fn backpack_deposit_targets_owned_view_and_drag_keeps_identity() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&client, None).unwrap();
        gui.set_screen_size(1280, 800);
        let mut ui = LootUi::default();
        let c = Identity { kind: 0xc749, instance: 7 };
        let mut u = update(true, &[3]);
        u.container = c;
        ui.update(&mut gui, None, (1280, 800), c, "Backpack", &u, true);
        let w = ui.window_of(c).unwrap();
        let r = gui.view_rect(w, "grid").unwrap();
        let (ox, oy) = cell_origin(0, 0, gap());
        let (x, y) = (r.l + ox + 2.0, r.t + oy + 2.0);
        assert_eq!(ui.container_drop(&gui, x, y), Some(c));
        let press = Event::CanvasPress { window: w, view: "grid".into(), x: ox + 2.0, y: oy + 2.0, button: ao_gui::MouseButton::Left, clicks: 1 };
        ui.event(&mut gui, &press, 1.0);
        ui.event(&mut gui, &Event::CanvasDrag { window: w, view: "grid".into(), x: ox + 20.0, y: oy + 2.0, dx: 18.0, dy: 0.0 }, 1.1);
        gui.input(ao_gui::InputEvent::MouseMove { x: 10.0, y: 20.0 });
        ui.event(&mut gui, &Event::CanvasRelease { window: w, view: "grid".into() }, 1.2);
        assert_eq!(ui.take_drops(), [(container_item_identity(u.word, 3), 10.0, 20.0)]);
        ui.open[0].owned = false;
        assert_eq!(ui.container_drop(&gui, x, y), None, "world chest cache grants no deposit ownership");
        ui.close_all(&mut gui);
    }

    #[test]
    fn right_click_takes_and_shift_left_only_inspects_bank_and_loot() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&client, None).unwrap();
        for kind in [0xdead, 0xc76a] {
            let mut ui = LootUi::default();
            let c = Identity { kind, instance: 7 };
            let mut u = update(true, &[3]);
            u.container = c;
            ui.update(&mut gui, None, (800, 600), c, "Items", &u, false);
            let window = ui.window_of(c).unwrap();
            let (x, y) = cell_origin(0, 0, gap());
            let item = if kind == 0xdead { Identity { kind: 0x69, instance: 3 } } else { container_item_identity(u.word, 3) };
            let press = |button| Event::CanvasPress { window, view: "grid".into(), x: x + 2.0, y: y + 2.0, button, clicks: 1 };
            assert_eq!(ui.event(&mut gui, &press(ao_gui::MouseButton::Right), 1.0), Some(Some(item)));
            assert!(ui.info_urls.is_empty());
            ui.info_click = true;
            assert_eq!(ui.event(&mut gui, &press(ao_gui::MouseButton::Left), 1.1), Some(None));
            let click = Event::CanvasClick { window, view: "grid".into(), x: x + 2.0, y: y + 2.0 };
            assert_eq!(ui.event(&mut gui, &click, 1.2), Some(None));
            assert_eq!(ui.event(&mut gui, &click, 1.3), Some(None));
            assert_eq!(ui.info_urls, [format!("itemid://{}/{}", item.kind, item.instance)]);
            ui.close_all(&mut gui);
        }
    }

    #[test]
    fn grid_is_three_columns_of_cells_and_hit_tests_them() {
        let (w, h) = canvas_size();
        assert!(w > 150 && h > 380, "{w}x{h}");
        for n in 0..SLOTS {
            let (ox, oy) = cell_origin(n % COLUMNS, n / COLUMNS, gap());
            assert_eq!(cell_at(ox + 1.0, oy + 1.0), Some(n));
        }
        // the border and the spacing between cells hit nothing
        assert_eq!(cell_at(0.0, 0.0), None);
        let (ox, oy) = cell_origin(0, 0, gap());
        assert_eq!(cell_at(ox + CELL + 3.0, oy + 1.0), None);
    }

    #[test]
    fn bank_has_102_cells_and_uses_bank_item_identities() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&client, None).unwrap();
        let mut ui = LootUi::default();
        let c = Identity { kind: 0xdead, instance: 7 };
        let mut u = update(true, &[101]);
        u.container = c;
        ui.update(&mut gui, None, (1280, 800), c, "Bank", &u, false);
        assert_eq!(ui.open[0].slots, 102);
        assert_eq!(ui.take(&mut gui, 0, 1.0), Some(Identity { kind: 0x69, instance: 101 }));
        let w = ui.open[0].window;
        ui.event(&mut gui, &Event::Escape { window: w }, 2.0);
        assert!(ui.is_open(c), "bank is not registered with the Esc handler");
        ui.close_bank(&mut gui);
        assert!(!ui.is_open(c));
    }

    fn update(flag: bool, slots: &[u32]) -> InventoryUpdate {
        let entries = slots.iter().map(|&s| InventoryEntry { slot: s, a: 0, b: 0, id: Identity::default(), item: AcgItem { low_id: 1, high_id: 1, level: 1 } }).collect();
        InventoryUpdate { capacity: 21, access: 0, entries, container: Identity { kind: 0xC76A, instance: 5 }, word: 0x70, flag }
    }

    #[test]
    fn flag_opens_the_window_and_double_click_takes_the_item() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&client, None).unwrap();
        gui.set_screen_size(1280, 800);
        let (mut ui, c) = (LootUi::default(), Identity { kind: 0xC76A, instance: 5 });
        // an update without the open flag does not open anything
        ui.update(&mut gui, None, (800, 600), c, "Remains of a Test", &update(false, &[0]), false);
        assert!(!ui.is_open(c));
        ui.update(&mut gui, None, (800, 600), c, "Remains of a Test", &update(true, &[3, 1]), false);
        assert!(ui.is_open(c));
        let w = ui.open[0].window;
        assert_eq!(ui.open[0].entries.iter().map(|e| e.slot).collect::<Vec<_>>(), [1, 3], "cells follow the container slots");
        // the second item sits in cell 1
        let (ox, oy) = cell_origin(1, 0, gap());
        let click = Event::CanvasClick { window: w, view: "grid".into(), x: ox + 2.0, y: oy + 2.0 };
        assert_eq!(ui.event(&mut gui, &click, 1.0), Some(None));
        assert_eq!(ui.event(&mut gui, &click, 1.2), Some(Some(Identity { kind: 0x6b, instance: 0x0070_0003 })));
        // too slow: no double click; an empty cell: nothing
        assert_eq!(ui.event(&mut gui, &click, 5.0), Some(None));
        let (ex, ey) = cell_origin(2, 5, gap());
        assert_eq!(ui.event(&mut gui, &Event::CanvasClick { window: w, view: "grid".into(), x: ex, y: ey }, 5.1), Some(None));
        ui.open[0].tip = Some((0, true, "old item body".into()));
        ui.taken(&mut gui, container_item_identity(ui.open[0].word, 1));
        assert!(ui.open[0].tip.is_none(), "taken items invalidate cached tooltip bodies");
        ui.open[0].tip = Some((0, true, "old item body".into()));
        // a refresh with the flag clear keeps the window and replaces the items; closing removes it
        ui.update(&mut gui, None, (800, 600), c, "", &update(false, &[7]), false);
        assert_eq!(ui.open[0].entries.len(), 1);
        assert!(ui.open[0].tip.is_none(), "a container refresh invalidates cached tooltip bodies");
        assert_eq!(ui.event(&mut gui, &Event::CloseRequested { window: w }, 6.0), Some(None));
        assert!(!ui.is_open(c));
    }
}
