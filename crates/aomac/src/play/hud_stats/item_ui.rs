//! Pointer handling of the wear / inventory item cells: press, drag with the cursor-carried icon, drop and double click (rules: [`super::item_dnd`]).

use super::item_dnd::{self as dnd, Action, Drag, Place, DOUBLE_CLICK};
use super::{inv_grid, items, HudStats};
use crate::play::zone::Zone;
use ao_gui::{CanvasItem, Gui, InputEvent, MouseButton, WindowSize};
use ao_net::msg::Identity;
use ao_net::n3::inventory::{self as inv, Page};
use ao_net::n3::misc::{GenericArgs, GenericCmd, Misc};
use ao_net::n3::outgoing::{n3_frame, DYNEL_CHAR};

/// Pixels the pointer has to move after the press before the drag starts (`hud_bar::DRAG_THRESHOLD`).
const DRAG_THRESHOLD: f32 = 3.0;
/// The carried icon (`FUN_100cbab4`: `View::BeginDrag(.., hotspot Point(16, 16), itemview)`; the item view's size is UNRESOLVED, the 32 px of the shortcut bar icons is used).
const GHOST: f32 = 32.0;
/// `GenericCmd_t` command of `N3Msg_UseItem` (GC 0x100286f8).
const CMD_USE_ITEM: i32 = 3;
/// `ComputerLiteracy` (stat 0x2d): the deck weapons need it (`FUN_10046cbf`).
const STAT_DECK: u32 = 0x2d;

impl HudStats {
    /// Inventory slot of a place (`None` for an empty grid cell).
    pub(super) fn slot_of(&self, p: Place) -> Option<u32> {
        match p {
            Place::Wear { slot, .. } => Some(slot),
            Place::Bag { cell } => self.inventory.as_ref()?.positions.at(cell),
        }
    }

    /// The cell under the screen position (wear window first, then the inventory grid).
    pub(super) fn place_at(&self, gui: &Gui, x: f32, y: f32) -> Option<Place> {
        if let Some(t) = &self.wear {
            if let Some(r) = gui.view_rect(t.window, "items") {
                if x >= r.l && x < r.r && y >= r.t && y < r.b {
                    return self.wear_cell_at(t.tab, x - r.l, y - r.t);
                }
            }
        }
        if let Some(i) = &self.inventory {
            if !i.list {
                // only the visible part of the scrolled grid counts
                let (g, vp) = (gui.view_rect(i.window, "grid"), gui.view_rect(i.window, "scrollview"));
                if let (Some(g), Some(vp)) = (g, vp) {
                    if x >= g.l.max(vp.l) && x < g.r.min(vp.r) && y >= g.t.max(vp.t) && y < g.b.min(vp.b) {
                        return self.bag_cell_at(x - g.l, y - g.t);
                    }
                }
            }
        }
        None
    }

    /// The wear cell of the tab at the position inside the `items` canvas.
    pub(super) fn wear_cell_at(&self, tab: usize, lx: f32, ly: f32) -> Option<Place> {
        (0..15).find_map(|cell| {
            let slot = items::wear_slot(tab, cell)?;
            let (c, r) = items::wear_cell(tab, cell);
            let (ox, oy) = items::wear_origin(c, r);
            (lx >= ox && lx < ox + items::SLOT && ly >= oy && ly < oy + items::SLOT).then_some(Place::Wear { tab, id: slot - Page::of_slot(slot).base(), slot })
        })
    }

    /// The grid cell at the position inside the `grid` canvas (cells of the first [`inv_grid::MAX_ITEMS`] positions).
    pub(super) fn bag_cell_at(&self, lx: f32, ly: f32) -> Option<Place> {
        let i = self.inventory.as_ref()?;
        (0..inv_grid::MAX_ITEMS).find_map(|n| {
            let cell = (n % i.cols, n / i.cols);
            let (ox, oy) = inv_grid::cell_origin(cell.0, cell.1, i.gap);
            (lx >= ox && lx < ox + inv_grid::ICON + 1.0 && ly >= oy && ly < oy + inv_grid::ICON + 1.0).then_some(Place::Bag { cell })
        })
    }

    /// Raw pointer events (`Hud::input`): press on an item, drag, drop.
    pub(in crate::play) fn input(&mut self, gui: &mut Gui, zone: &Zone, ev: &InputEvent) {
        match *ev {
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                self.dnd.press = self.place_at(gui, x, y).filter(|&p| self.slot_of(p).is_some_and(|s| zone.inventory.contains_key(&s))).map(|p| (p, x, y));
            }
            InputEvent::MouseMove { x, y } => {
                if let Some((ghost, slot)) = self.dnd.drag.as_ref().map(|d| (d.ghost, d.slot)) {
                    gui.set_window_pos(ghost, (x as i32 - GHOST as i32 / 2, y as i32 - GHOST as i32 / 2));
                    self.dnd.hover = self.place_at(gui, x, y).map(|p| (p, self.drop_action(zone, slot, p).is_some()));
                } else if let Some((p, px, py)) = self.dnd.press {
                    if (x - px).hypot(y - py) > DRAG_THRESHOLD {
                        self.begin_drag(gui, zone, p, x, y);
                    }
                }
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                self.dnd.press = None;
                if let Some(d) = self.dnd.drag.take() {
                    gui.close_window(d.ghost);
                    self.dnd.hover = None;
                    let dest = self.place_at(gui, x, y);
                    let action = match dest {
                        Some(p) => self.drop_action(zone, d.slot, p),
                        // released over the world: `N3Msg_DropItem` (the position under the cursor is picked by the 3D view: UNRESOLVED, we drop at our feet)
                        None if !gui.wants_mouse(x, y) => Some(Action::Drop { item: inv::item_identity(d.slot) }),
                        None => None,
                    };
                    if let Some(a) = action {
                        self.run(zone, a, dest);
                    }
                }
            }
            _ => {}
        }
    }

    fn begin_drag(&mut self, gui: &mut Gui, zone: &Zone, from: Place, x: f32, y: f32) {
        let Some(slot) = self.slot_of(from) else { return };
        let Some(e) = zone.inventory.get(&slot).copied() else { return };
        let src = format!("<root><CanvasView name=\"icon\" min_size=\"Point({0},{0})\" max_size=\"Point({0},{0})\"/></root>", GHOST as u32);
        let Ok(ghost) = gui.open_window_xml("ItemDrag", &src, (x as i32 - GHOST as i32 / 2, y as i32 - GHOST as i32 / 2), WindowSize::Preferred) else { return };
        if let Some((g, w, h)) = self.items.info(gui, e.item.low_id).and_then(|i| i.icon) {
            gui.set_canvas(ghost, "icon", vec![CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, GHOST, GHOST], alpha: 1.0 }]);
        }
        self.dnd.press = None;
        self.dnd.drag = Some(Drag { slot, ghost });
    }

    /// What dropping the item of inventory slot `slot` on `dest` asks for; `None` = refused (the drop is cancelled).
    pub(super) fn drop_action(&self, zone: &Zone, slot: u32, dest: Place) -> Option<Action> {
        let item = inv::item_identity(slot);
        let e = zone.inventory.get(&slot)?;
        match dest {
            Place::Wear { tab, id, slot: dslot } => {
                if dslot == slot {
                    return None;
                }
                let info = self.items.cached(e.item.low_id)?;
                let (page, _) = dnd::tab_page(tab)?;
                dnd::can_wear(info, tab, id, zone.stat(STAT_DECK).unwrap_or(0)).then(|| Action::Move { item, slot: (page.base() + id) as i32 })
            }
            Place::Bag { cell } => {
                if slot < inv::BAG_FIRST {
                    // unequip: `N3Msg_MoveItemToInventory(item, 1, 0x6f)`; refused with `Feedback_InventoryFull` when the bag is full
                    return zone.free_bag_slot().map(|_| Action::Move { item, slot: inv::ANY_BAG_SLOT });
                }
                let inventory = self.inventory.as_ref()?;
                let from = inventory.positions.get(slot)?;
                if from == cell {
                    return None;
                }
                match inventory.positions.at(cell) {
                    Some(t) => {
                        let te = zone.inventory.get(&t)?;
                        let (a, b) = (self.items.cached(te.item.low_id)?, self.items.cached(e.item.low_id)?);
                        let same = te.item.low_id == e.item.low_id && te.item.high_id == e.item.high_id;
                        if dnd::can_join(a, b, same, te.item.level, e.item.level) {
                            Some(Action::Join { from: item, onto: inv::item_identity(t) })
                        } else {
                            Some(Action::MoveCell { from, to: cell })
                        }
                    }
                    None => Some(Action::MoveCell { from, to: cell }),
                }
            }
        }
    }

    /// Performs an accepted drop / double click.
    fn run(&mut self, zone: &Zone, a: Action, dest: Option<Place>) {
        let char_id = zone.char_id as i32;
        let payload = match a {
            Action::Move { item, slot } => {
                // `FUN_100cbc21`: the cell the item was dropped on is where it appears once the server has moved it into the bag
                if let (Some(Place::Bag { cell }), Some(i)) = (dest, self.inventory.as_mut()) {
                    i.pending_drop = Some(cell);
                }
                inv::move_item_to_inventory(char_id, item, slot)
            }
            Action::Join { from, onto } => inv::join_items(char_id, onto, from),
            Action::MoveCell { from, to } => {
                if let Some(i) = self.inventory.as_mut() {
                    i.positions.move_cell(from, to);
                }
                return;
            }
            Action::Drop { item } => {
                let p = zone.own().map_or([0.0; 3], |d| d.pos);
                inv::drop_item(char_id, item, p)
            }
            Action::Use { item } => {
                self.use_seq += 1;
                let own = Identity { kind: DYNEL_CHAR, instance: char_id };
                let cmd = GenericCmd { state: 0, seq: self.use_seq, cmd: CMD_USE_ITEM, args: GenericArgs::Item { flag: 0, actor: own, item } };
                Misc::GenericCmd(cmd).encode(own, 1)
            }
        };
        self.outbox.push(n3_frame(0, zone.char_id, payload));
    }

    /// A short click on an item cell (`Event::CanvasClick`): the second click on the same cell within [`DOUBLE_CLICK`] seconds uses the item
    /// (`FUN_100ca1e7` -> `N3Msg_UseItem`): a worn item goes to the bag, a bag item is worn at its `DefaultPos` or used.
    pub(super) fn item_click(&mut self, zone: &Zone, place: Place) {
        let now = self.dnd.clock;
        let double = self.dnd.last_click.is_some_and(|(p, t)| p == place && now - t <= DOUBLE_CLICK);
        self.dnd.last_click = if double { None } else { Some((place, now)) };
        if !double {
            return;
        }
        let Some(slot) = self.slot_of(place) else { return };
        let Some(e) = zone.inventory.get(&slot) else { return };
        let item = inv::item_identity(slot);
        let action = if slot < inv::BAG_FIRST {
            zone.free_bag_slot().map(|_| Action::Move { item, slot: inv::ANY_BAG_SLOT })
        } else {
            let Some(info) = self.items.cached(e.item.low_id) else { return };
            match dnd::default_wear(info) {
                Some((tab, id)) => {
                    let Some((page, _)) = dnd::tab_page(tab) else { return };
                    dnd::can_wear(info, tab, id, zone.stat(STAT_DECK).unwrap_or(0)).then(|| Action::Move { item, slot: (page.base() + id) as i32 })
                }
                None => Some(Action::Use { item }),
            }
        };
        if let Some(a) = action {
            self.run(zone, a, None);
        }
    }
}
