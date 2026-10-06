//! The container (loot) window of corpses and chests: `InventoryView_c` of type 1 (`FUN_100cc2ca(1, container)` [GUI 0x100cc2ca], opened through
//! `GlobalSignals +0x84` -> `InventoryGUIModule_c::SlotContainerOpened` [GUI 0x100c71a3]). Evidence and the unresolved parts: docs/zone/interact.md §9.

use super::hud_stats::inv_grid::{cell_origin, BORDER, ICON as CELL, SPACING};
use super::hud_stats::items::{Items, SLOT};
use ao_gui::{CanvasItem, CanvasTip, Event, Gui, WindowId, WindowSize};
use ao_net::msg::Identity;
use ao_net::n3::inventory::InventoryUpdate;
use ao_net::n3::world::InventoryEntry;
use std::path::Path;

/// `SetMaxItemCount(0x15)` of a container view (docs/gui.md §11.5) and the `item_position_map` loop of `FUN_100cc2ca` (`n % 3`, `n / 3`, `n < 0x15`).
const SLOTS: usize = 0x15;
const COLUMNS: usize = 3;
/// `Identity_t::kind` of an item in a corpse's window: `FUN_1004ad44` [GC] sends kind `0x6a` to `FUN_10046b0b` (the corpse -> own bag move) and
/// `N3Msg_UseItem` refuses it with `Feedback_ItemsCantBeUsedFromCorpse`; the instance is the slot in the container. [INFERENCE]: the kind of the
/// items of a chest (0xC749) was not separated from it.
pub const KIND_CORPSE_ITEM: i32 = 0x6a;
const SLOT_GFX: &str = "GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED";

struct Loot {
    window: WindowId,
    container: Identity,
    /// The items in the order of their cells (by container slot).
    entries: Vec<InventoryEntry>,
    /// The cell of the last short click and when (`Interact::now`), for the double click.
    clicked: Option<(usize, f32)>,
}

/// `(container, [(container slot, item low id, name)])` of every open window, for the harness and the tests.
#[cfg(test)]
pub type LootRows = Vec<(Identity, Vec<(u32, i32, String)>)>;

/// The open container windows (one per container).
#[derive(Default)]
pub struct LootUi {
    items: Option<Items>,
    open: Vec<Loot>,
}

fn gap() -> (f32, f32) {
    (SPACING, SPACING)
}

/// The size of the grid canvas: `rows` rows of cells with the default spacing and the view borders (`RecalcCellCount`, docs/gui.md §11.5).
fn canvas_size() -> (u32, u32) {
    let rows = SLOTS.div_ceil(COLUMNS) as f32;
    let (w, h) = (2.0 * BORDER + COLUMNS as f32 * (CELL + 1.0) + (COLUMNS as f32 - 1.0) * gap().0, 2.0 * BORDER + rows * (CELL + 1.0) + (rows - 1.0) * gap().1);
    (w as u32, h as u32)
}

/// Top-left corner of the 54 px slot art of cell `n` (the art is centred on the 48 px cell, as in the inventory window).
fn slot_origin(n: usize) -> (f32, f32) {
    let (x, y) = cell_origin(n % COLUMNS, n / COLUMNS, gap());
    let o = (SLOT - CELL - 1.0) / 2.0;
    (x - o, y - o)
}

/// The cell under the canvas position.
fn cell_at(x: f32, y: f32) -> Option<usize> {
    (0..SLOTS).find(|&n| {
        let (ox, oy) = cell_origin(n % COLUMNS, n / COLUMNS, gap());
        x >= ox && x < ox + CELL + 1.0 && y >= oy && y < oy + CELL + 1.0
    })
}

impl LootUi {
    #[cfg(test)]
    pub fn is_open(&self, container: Identity) -> bool {
        self.open.iter().any(|l| l.container == container)
    }

    #[cfg(test)]
    pub fn window_of(&self, container: Identity) -> Option<WindowId> {
        self.open.iter().find(|l| l.container == container).map(|l| l.window)
    }

    pub fn close_all(&mut self, gui: &mut Gui) {
        for l in self.open.drain(..) {
            gui.close_window(l.window);
        }
    }

    /// `InventoryUpdateIIR_t` for `container` [GC 0x100a040e]: the contents are replaced; with `u.flag` the container is opened (a window appears
    /// unless one is open already, `SlotContainerOpened`), without it only an open window is refreshed (`+0x8c`, container changed).
    pub fn update(&mut self, gui: &mut Gui, dir: Option<&Path>, screen: (u32, u32), container: Identity, title: &str, u: &InventoryUpdate) {
        if self.items.is_none() {
            self.items = dir.map(Items::new);
        }
        let mut entries = u.entries.clone();
        entries.sort_by_key(|e| e.slot);
        entries.truncate(SLOTS);
        let at = match self.open.iter().position(|l| l.container == container) {
            Some(i) => i,
            None if u.flag => {
                let (w, h) = canvas_size();
                let xml = format!("<root><View view_layout=\"vertical\"><CanvasView name=\"grid\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/></View></root>");
                let Ok(window) = gui.open_tabbed_window_xml("LootView", title, &xml, (0, 0), WindowSize::Preferred) else { return };
                let (ow, oh) = gui.outer_size(window);
                // `Window::MoveToCenter` is the default of a window without a saved frame (`container_position` is not stored)
                gui.set_window_pos(window, ((screen.0 as i32 - ow as i32) / 2, (screen.1 as i32 - oh as i32) / 2));
                self.open.push(Loot { window, container, entries: vec![], clicked: None });
                self.open.len() - 1
            }
            None => return,
        };
        self.open[at].entries = entries;
        self.draw(gui, at);
    }

    fn draw(&mut self, gui: &mut Gui, at: usize) {
        let slot_gfx = gui.gfx_id(SLOT_GFX).map(ao_gui::GfxId);
        let (mut cmds, mut tips) = (vec![], vec![]);
        for n in 0..SLOTS {
            let (x, y) = slot_origin(n);
            if let Some(g) = slot_gfx {
                cmds.push(CanvasItem::Image { id: g, src: [0.0, 0.0, SLOT, SLOT], dst: [x, y, x + SLOT, y + SLOT], alpha: 1.0 });
            }
            let Some(e) = self.open[at].entries.get(n).copied() else { continue };
            let Some(items) = self.items.as_mut() else { continue };
            cmds.extend(items.picture(gui, &e, x, y));
            if let Some(info) = items.info(gui, e.item.low_id) {
                tips.push(CanvasTip { rect: [x, y, x + SLOT, y + SLOT], title: info.name.clone(), body: String::new() });
            }
        }
        let w = self.open[at].window;
        gui.set_canvas(w, "grid", cmds);
        gui.set_canvas_tips(w, "grid", tips);
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
            Event::CanvasClick { window, view, x, y } if view == "grid" => {
                let l = self.open.iter_mut().find(|l| l.window == *window)?;
                let Some(n) = cell_at(*x, *y).filter(|&n| n < l.entries.len()) else { return Some(None) };
                let double = l.clicked.is_some_and(|(c, t)| c == n && now - t <= ao_gui::DOUBLE_CLICK_TIME);
                l.clicked = if double { None } else { Some((n, now)) };
                Some(double.then(|| Identity { kind: KIND_CORPSE_ITEM, instance: l.entries[n].slot as i32 }))
            }
            Event::CloseRequested { window } | Event::Escape { window } if self.open.iter().any(|l| l.window == *window) => {
                let i = self.open.iter().position(|l| l.window == *window)?;
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
mod tests {
    use super::*;
    use ao_net::n3::world::AcgItem;

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

    fn update(flag: bool, slots: &[u32]) -> InventoryUpdate {
        let entries = slots.iter().map(|&s| InventoryEntry { slot: s, a: 0, b: 0, id: Identity::default(), item: AcgItem { low_id: 1, high_id: 1, level: 1 } }).collect();
        InventoryUpdate { capacity: 21, kind: 0, entries, container: Identity { kind: 0xC76A, instance: 5 }, word: 0, flag }
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
        ui.update(&mut gui, None, (800, 600), c, "Remains of a Test", &update(false, &[0]));
        assert!(!ui.is_open(c));
        ui.update(&mut gui, None, (800, 600), c, "Remains of a Test", &update(true, &[3, 1]));
        assert!(ui.is_open(c));
        let w = ui.open[0].window;
        assert_eq!(ui.open[0].entries.iter().map(|e| e.slot).collect::<Vec<_>>(), [1, 3], "cells follow the container slots");
        // the second item sits in cell 1
        let (ox, oy) = cell_origin(1, 0, gap());
        let click = Event::CanvasClick { window: w, view: "grid".into(), x: ox + 2.0, y: oy + 2.0 };
        assert_eq!(ui.event(&mut gui, &click, 1.0), Some(None));
        assert_eq!(ui.event(&mut gui, &click, 1.2), Some(Some(Identity { kind: KIND_CORPSE_ITEM, instance: 3 })));
        // too slow: no double click; an empty cell: nothing
        assert_eq!(ui.event(&mut gui, &click, 5.0), Some(None));
        let (ex, ey) = cell_origin(2, 5, gap());
        assert_eq!(ui.event(&mut gui, &Event::CanvasClick { window: w, view: "grid".into(), x: ex, y: ey }, 5.1), Some(None));
        // a refresh with the flag clear keeps the window and replaces the items; closing removes it
        ui.update(&mut gui, None, (800, 600), c, "", &update(false, &[7]));
        assert_eq!(ui.open[0].entries.len(), 1);
        assert_eq!(ui.event(&mut gui, &Event::CloseRequested { window: w }, 6.0), Some(None));
        assert!(!ui.is_open(c));
    }
}
