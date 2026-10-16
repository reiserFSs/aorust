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
/// `GenericCmd_t` command of `N3Msg_UseItemOnItem` [GC 0x100267e0].
const CMD_USE_ITEM_ON_ITEM: i32 = 5;
/// `GenericCmd_t` command of `N3Msg_UseItemOnCharacter` [GC 0x100268af].
const CMD_USE_ITEM_ON_CHARACTER: i32 = 0x20;

impl HudStats {
    /// Native count selector `100412a0`: logical window coordinates, same units as MouseMove.
    pub(in crate::play) fn take_pointer_warp(&mut self) -> Option<(f32, f32)> {
        self.dnd.pointer_warp.take()
    }

    pub(in crate::play) fn set_nano_cast_busy(&mut self, busy: bool) {
        self.items.nano_cast_busy = busy;
    }

    pub(in crate::play) fn take_info_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.info_urls)
    }

    fn inspect_slot(&mut self, zone: &Zone, slot: u32) {
        if zone.inventory.contains_key(&slot) {
            let id = inv::item_identity(slot);
            self.info_urls.push(format!("itemid://{}/{}", id.kind, id.instance));
            self.dnd.last_click = None;
        }
    }

    pub(in crate::play) fn info_slot_at(&self, gui: &Gui, zone: &Zone, x: f32, y: f32) -> Option<u32> {
        let owner = gui.pointer_owner(x, y)?;
        if let Some(slot) = self.place_at_window(gui, x, y, Some(owner)).and_then(|p| self.slot_of(p)) {
            return Some(slot);
        }
        let i = self.inventory.as_ref().filter(|i| i.list && i.window == owner)?;
        let vp = gui.view_rect(i.window, "scrollview")?;
        if x < vp.l || x >= vp.r || y < vp.t || y >= vp.b { return None; }
        zone.inventory.keys().copied().find(|slot| {
            gui.view_rect(i.window, &format!("item{slot}")).is_some_and(|r| x >= r.l && x < r.r && y >= r.t && y < r.b)
        })
    }

    /// Inventory slot of a place (`None` for an empty grid cell).
    pub(super) fn slot_of(&self, p: Place) -> Option<u32> {
        match p {
            Place::Wear { slot, .. } => Some(slot),
            Place::Bag { cell } => self.inventory.as_ref()?.positions.at(cell),
        }
    }

    /// The cell under the screen position (wear window first, then the inventory grid).
    pub(super) fn place_at(&self, gui: &Gui, x: f32, y: f32) -> Option<Place> {
        self.place_at_window(gui, x, y, None)
    }

    fn place_at_window(&self, gui: &Gui, x: f32, y: f32, owner: Option<ao_gui::WindowId>) -> Option<Place> {
        if let Some(t) = self.wear.as_ref().filter(|t| owner.is_none_or(|w| w == t.window)) {
            if let Some(r) = gui.view_rect(t.window, "items") {
                if x >= r.l && x < r.r && y >= r.t && y < r.b {
                    return self.wear_cell_at(t.tab, x - r.l, y - r.t);
                }
            }
        }
        if let Some(i) = self.inventory.as_ref().filter(|i| owner.is_none_or(|w| w == i.window)) {
            if !i.list {
                // only the visible part of the scrolled grid counts
                let (g, vp) = (gui.view_rect(i.window, "grid"), gui.view_rect(i.window, "scrollview"));
                if let (Some(g), Some(vp)) = (g, vp) {
                    if x >= g.l.max(vp.l) && x < g.r.min(vp.r) && y >= g.t.max(vp.t) && y < g.b.min(vp.b) {
                        return self.bag_cell_at(x - g.l, y - g.t);
                    }
                }
            } else if let Some(vp) = gui.view_rect(i.window, "scrollview") {
                if x >= vp.l && x < vp.r && y >= vp.t && y < vp.b {
                    return (inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS).find_map(|slot| {
                        let row = gui.view_rect(i.window, &format!("item{slot}"))?;
                        let cell = i.positions.get(slot)?;
                        (x >= row.l && x < row.r && y >= row.t && y < row.b).then_some(Place::Bag { cell })
                    });
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

    /// Container withdrawals use the inventory view, never an obscured window or wear page.
    pub(in crate::play) fn container_bag_drop(&self, gui: &Gui, x: f32, y: f32) -> bool {
        matches!(self.place_at_window(gui, x, y, gui.pointer_owner(x, y)), Some(Place::Bag { .. }))
            && self.inventory.as_ref().is_some_and(|i| gui.pointer_owner(x, y) == Some(i.window))
    }

    /// Raw pointer events (`Hud::input`): press on an item, drag, drop.
    pub(super) fn cancel_item_drag(&mut self, gui: &mut Gui) {
        if let Some(split) = self.dnd.split.take() {
            if let Some(w) = split.popup { gui.close_window(w); }
        }
        if let Some(drag) = self.dnd.drag.take() { gui.close_window(drag.ghost); }
        self.dnd.press = None;
        self.dnd.hover = None;
        self.item_hold = None;
        self.dnd.pointer_warp = None;
    }

    pub(in crate::play) fn input(&mut self, gui: &mut Gui, zone: &Zone, ev: &InputEvent, mods: ao_gui::Modifiers) {
        self.skills_input(ev, mods);
        match *ev {
            InputEvent::Key { key: ao_gui::Key::Escape, pressed: true, .. } => self.cancel_item_drag(gui),
            InputEvent::MouseDown { x, y, .. } if !gui.pointer_owner(x, y).is_some_and(|owner|
                self.wear.as_ref().is_some_and(|w| w.window == owner)
                    || self.inventory.as_ref().is_some_and(|i| i.window == owner)) => {}
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                if mods.ctrl && self.dnd.drag.is_none() {
                    if let Some(place) = self.place_at_window(gui, x, y, gui.pointer_owner(x, y)) {
                        if self.slot_of(place).and_then(|s| zone.inventory.get(&s)).is_some_and(|e| (e.b as u16) > 1) {
                            self.dnd.press = None;
                            self.item_hold = None;
                            self.dnd.last_click = None;
                            self.info_click = true;
                            self.dnd.split = Some(dnd::Split { place, anchor: (x, y), count: 1, popup: None });
                            return;
                        }
                    }
                }
                // GUI 10040a17: information uses qualifier mask 3 (Shift), not Ctrl.
                self.info_click = mods.shift;
                if self.info_click {
                    self.dnd.press = None;
                    self.item_hold = None;
                    self.dnd.last_click = None;
                    if let Some(slot) = self.info_slot_at(gui, zone, x, y) {
                        self.inspect_slot(zone, slot);
                    }
                    return;
                }
                self.item_button = MouseButton::Left;
                self.dnd.press = self.new_mouse.then(|| self.place_at_window(gui, x, y, gui.pointer_owner(x, y))).flatten().filter(|&p| self.slot_of(p).is_some_and(|s| zone.inventory.contains_key(&s))).map(|p| (p, x, y));
                self.item_hold = self.dnd.press.map(|_| (0.0, self.item_hold_delay));
            }
            InputEvent::MouseDown { x, y, button: MouseButton::Right } => {
                if !self.new_mouse {
                    self.item_button = MouseButton::Right;
                    self.dnd.press = self.place_at_window(gui, x, y, gui.pointer_owner(x, y)).filter(|&p| self.slot_of(p).is_some_and(|s| zone.inventory.contains_key(&s))).map(|p| (p, x, y));
                    self.item_hold = self.dnd.press.map(|_| (0.0, self.item_hold_delay));
                    return;
                }
                self.dnd.press = None;
                self.dnd.last_click = None;
                if let Some(slot) = self.info_slot_at(gui, zone, x, y) {
                    self.use_slot(zone, slot);
                }
            }
            InputEvent::MouseMove { x, y } => {
                self.item_mouse = (x, y);
                if let Some(split) = &mut self.dnd.split {
                    let total = match split.place {
                        Place::Bag { cell } => self.inventory.as_ref().and_then(|i| i.positions.at(cell)),
                        Place::Wear { slot, .. } => Some(slot),
                    }.and_then(|s| zone.inventory.get(&s)).map_or(1, |e| i32::from(e.b as u16));
                    split.count = (split.count + (x - split.anchor.0).trunc() as i32).clamp(1, (total - 1).max(1));
                    if (x, y) != split.anchor { self.dnd.pointer_warp = Some(split.anchor); }
                    if split.popup.is_none() {
                        split.popup = gui.open_window_xml("ItemSplitCount", "<root><TextView name=\"count\"/></root>", (split.anchor.0 as i32, split.anchor.1 as i32), WindowSize::Preferred).ok();
                    }
                    if let Some(w) = split.popup {
                        gui.set_text(w, "count", &format!("<font color=yellow>{}</font>", split.count));
                        gui.relayout_window(w);
                    }
                    return;
                }
                if let Some((ghost, slot)) = self.dnd.drag.as_ref().map(|d| (d.ghost, d.slot)) {
                    gui.set_window_pos(ghost, (x as i32 - GHOST as i32 / 2, y as i32 - GHOST as i32 / 2));
                    self.dnd.hover = self.place_at(gui, x, y).map(|p| (p, self.drop_action(zone, slot, p).is_some()));
                } else if let Some((p, px, py)) = self.dnd.press {
                    if (x - px).hypot(y - py) > DRAG_THRESHOLD {
                        self.begin_drag(gui, zone, p, x, y);
                    }
                }
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } if self.dnd.split.is_some() => {
                let split = self.dnd.split.take().expect("matched above");
                self.dnd.pointer_warp = None;
                if let Some(w) = split.popup { gui.close_window(w); }
                self.begin_drag(gui, zone, split.place, x, y);
                if let Some(drag) = &mut self.dnd.drag { drag.count = split.count; }
                self.item_button = MouseButton::Left;
            }
            InputEvent::MouseUp { x, y, button } if button == self.item_button => {
                let pressed = self.dnd.press.take();
                self.item_hold = None;
                if button == MouseButton::Right && self.dnd.drag.is_none() {
                    if let Some((place, _, _)) = pressed.filter(|(place, _, _)| self.place_at(gui, x, y) == Some(*place)) {
                        if let Some(slot) = self.slot_of(place) { self.use_slot(zone, slot); }
                    }
                }
                if let Some(d) = self.dnd.drag.take() {
                    gui.close_window(d.ghost);
                    self.dnd.hover = None;
                    let dest = self.place_at(gui, x, y);
                    if d.count > 0 {
                        if let Some(Place::Bag { cell }) = dest {
                            let Some(inventory) = self.inventory.as_ref() else { return };
                            let empty = inventory.list || inventory.positions.at(cell).is_none();
                            let dest = (!inventory.list).then_some(Place::Bag { cell });
                            if empty && zone.free_bag_slot().is_some() {
                                if let Some(e) = zone.inventory.get(&d.slot) {
                                    let can = match self.items.acg_stat(e.item, dnd::STAT_CAN) {
                                        Ok(Some(can)) => can,
                                        Ok(None) => return,
                                        Err(error) => { eprintln!("split item: {error:#}"); return; }
                                    };
                                    if dnd::can_split(can, i32::from(e.b as u16), d.count) {
                                        self.run(zone, Action::Split { item: inv::item_identity(d.slot), count: d.count }, dest);
                                    }
                                }
                            }
                        }
                        return;
                    }
                    if self.equipment_locked && (d.slot < inv::BAG_FIRST || matches!(dest, Some(Place::Wear { .. }))) {
                        self.equipment_feedback = true;
                        return;
                    }
                    let action = match dest {
                        Some(p) => self.drop_action(zone, d.slot, p),
                        // released over the world: `N3Msg_DropItem` (the position under the cursor is picked by the 3D view: UNRESOLVED, we drop at our feet)
                        // `FUN_100cb081`: over a character / another world object the item is used on it, over the ground it is dropped
                        None if !gui.wants_mouse(x, y) => {
                            let item = inv::item_identity(d.slot);
                            Some(match self.dnd.world_under.take() {
                                Some(target) => Action::UseOn { item, target },
                                None => Action::Drop { item },
                            })
                        }
                        // released over another window (the NPC trade window, `play/interact_trade.rs`): the drop is reported to whoever owns that window
                        None => {
                            self.dnd.dropped.push((d.slot, x, y));
                            None
                        }
                    };
                    if let Some(a) = action {
                        if let Action::Drop { item } = a {
                            self.request_drop(gui, zone, item);
                            return;
                        }
                        self.run(zone, a, dest);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn advance_item_hold(&mut self, gui: &mut Gui, zone: &Zone, dt: f32) {
        let Some((held, delay)) = &mut self.item_hold else { return };
        *held += dt;
        if *held < *delay { return; }
        self.item_hold = None;
        if let Some((place, x, y)) = self.dnd.press {
            self.begin_drag(gui, zone, place, x, y);
        }
    }

    fn begin_drag(&mut self, gui: &mut Gui, zone: &Zone, from: Place, x: f32, y: f32) {
        let Some(slot) = self.slot_of(from) else { return };
        let Some(e) = zone.inventory.get(&slot).copied() else { return };
        if self.equipment_locked && slot < inv::BAG_FIRST {
            self.dnd.press = None;
            self.item_hold = None;
            self.equipment_feedback = true;
            return;
        }
        let src = format!("<root><CanvasView name=\"icon\" min_size=\"Point({0},{0})\" max_size=\"Point({0},{0})\"/></root>", GHOST as u32);
        let Ok(ghost) = gui.open_window_xml("ItemDrag", &src, (x as i32 - GHOST as i32 / 2, y as i32 - GHOST as i32 / 2), WindowSize::Preferred) else { return };
        if let Some((g, w, h)) = self.items.info(gui, e.item).and_then(|i| i.icon) {
            gui.set_canvas(ghost, "icon", vec![CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, GHOST, GHOST], alpha: 1.0 }]);
        }
        self.dnd.press = None;
        self.item_hold = None;
        self.dnd.drag = Some(Drag { slot, ghost, count: 0 });
    }

    /// What dropping the item of inventory slot `slot` on `dest` asks for; `None` = refused (the drop is cancelled).
    pub(super) fn drop_action(&mut self, zone: &Zone, slot: u32, dest: Place) -> Option<Action> {
        if self.equipment_locked && (slot < inv::BAG_FIRST || matches!(dest, Place::Wear { .. })) {
            return None;
        }
        if (slot < inv::BAG_FIRST || matches!(dest, Place::Wear { .. }))
            && (self.items.nano_cast_busy || self.wear_timers.active().is_some() || (slot < inv::BAG_FIRST && !self.can_unwear(zone, slot))) {
            return None;
        }
        let item = inv::item_identity(slot);
        let e = zone.inventory.get(&slot)?;
        match dest {
            Place::Wear { tab, id, slot: dslot } => {
                if dslot == slot {
                    return None;
                }
                let (page, _) = dnd::tab_page(tab)?;
                self.can_equip(zone, slot, tab, id).then(|| Action::Move { item, slot: (page.base() + id) as i32 })
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
                        let a = self.items.acg_stat(te.item, dnd::STAT_CAN).map_err(|error| eprintln!("join item: {error:#}")).ok().flatten()?;
                        let b = self.items.acg_stat(e.item, dnd::STAT_CAN).map_err(|error| eprintln!("join item: {error:#}")).ok().flatten()?;
                        let same = te.item.low_id == e.item.low_id && te.item.high_id == e.item.high_id;
                        if dnd::can_join(a, b, same, te.item.level, e.item.level) && u32::from(te.b as u16) + u32::from(e.b as u16) <= 50_000 {
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

    /// The drop of bag slot `slot` on world object `target` (live harness `useon`; the pointer release does the same).
    #[cfg(test)]
    pub(in crate::play) fn use_on(&mut self, zone: &Zone, slot: u32, target: Identity) {
        self.run(zone, Action::UseOn { item: inv::item_identity(slot), target }, None);
    }

    /// Performs an accepted drop / double click.
    fn run(&mut self, zone: &Zone, a: Action, dest: Option<Place>) {
        let char_id = zone.char_id as i32;
        let payload = match a {
            Action::Move { item, slot } => {
                // Guard the shared dispatch as well: double-clicks do not pass through drop_action.
                if self.equipment_locked && (item.instance < inv::BAG_FIRST as i32 || slot < inv::BAG_FIRST as i32) {
                    self.equipment_feedback = true;
                    return;
                }
                if self.items.nano_cast_busy || self.wear_timers.active().is_some() { return; }
                if (0..inv::BAG_FIRST as i32).contains(&item.instance) && !self.can_unwear(zone, item.instance as u32) { return; }
                if (1..0x40).contains(&slot) {
                    let tab = (slot / 0x10) as usize;
                    if !self.can_equip(zone, item.instance as u32, tab, slot as u32 % 0x10) { return; }
                }
                // `FUN_100cbc21`: the cell the item was dropped on is where it appears once the server has moved it into the bag
                if let (Some(Place::Bag { cell }), Some(i)) = (dest, self.inventory.as_mut()) {
                    i.pending_drop = Some(cell);
                }
                // GC1004aed0 sums ACG-resolved moved/displaced EquipDelay.
                // Kind6 gates later requests, but never fabricates the server move.
                match self.items.wear_delay(zone, item.instance as u32, slot) {
                    Ok(Some(seconds)) => self.wear_timers.start(seconds),
                    Ok(None) => {}
                    Err(error) => { eprintln!("item wear delay: {error:#}"); return; }
                }
                inv::move_item_to_inventory(char_id, item, slot)
            }
            Action::Join { from, onto } => inv::join_items(char_id, onto, from),
            Action::Split { item, count } => {
                if let (Some(Place::Bag { cell }), Some(i)) = (dest, self.inventory.as_mut()) {
                    i.pending_drop = Some(cell);
                }
                inv::split_item(char_id, item, count)
            }
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
                if let Some(entry) = zone.inventory.get(&(item.instance as u32)) {
                    match self.items.is_tower_item(entry.item.low_id) {
                        Ok(true) => { self.tower_uses.push(item); return; }
                        Ok(false) => {}
                        Err(error) => { eprintln!("tower item: {error:#}"); return; }
                    }
                }
                self.use_seq += 1;
                let own = Identity { kind: DYNEL_CHAR, instance: char_id };
                let cmd = GenericCmd { state: 0, seq: self.use_seq, cmd: CMD_USE_ITEM, args: GenericArgs::Item { flag: 0, actor: own, item } };
                Misc::GenericCmd(cmd).encode(own, 1)
            }
            Action::UseOn { item, target } => {
                self.use_seq += 1;
                let own = Identity { kind: DYNEL_CHAR, instance: char_id };
                let cmd = if target.kind == DYNEL_CHAR { CMD_USE_ITEM_ON_CHARACTER } else { CMD_USE_ITEM_ON_ITEM };
                let cmd = GenericCmd { state: 0, seq: self.use_seq, cmd, args: GenericArgs::ItemOnItem { flag: 0, actor: own, item, target } };
                Misc::GenericCmd(cmd).encode(own, 1)
            }
        };
        self.outbox.push(n3_frame(0, zone.char_id, payload));
    }

    /// GUI `FUN_100cb081`: the Char int chooses direct drop or the Yes/No confirmation.
    fn request_drop(&mut self, gui: &mut Gui, zone: &Zone, item: Identity) {
        if !self.confirm_drop {
            self.run(zone, Action::Drop { item }, None);
            return;
        }
        let Some(e) = zone.inventory.get(&(item.instance as u32)) else { return };
        let Some(info) = self.items.info(gui, e.item) else { return };
        let body = super::super::chat::log::ldb_format(
            &self.db.by_key(10000, "Want2DropItemXonGround").unwrap_or_default(),
            &[super::super::chat::log::Arg::S(info.name.clone())],
        );
        let buttons = ["MsgBox_Yes", "MsgBox_No"].map(|key| self.db.by_key(10000, key).unwrap_or_default());
        let position = zone.own().map_or([0.0; 3], |d| d.pos);
        self.drop_dialogs.go(gui, self.screen, (item, position), &body, &buttons);
    }

    pub(super) fn drop_dialog_event(&mut self, gui: &mut Gui, ev: &ao_gui::Event, zone: &Zone) -> bool {
        let (handled, choice) = self.drop_dialogs.event(gui, ev);
        if let Some(((item, position), 0)) = choice {
            if zone.inventory.contains_key(&(item.instance as u32)) {
                self.outbox.push(n3_frame(0, zone.char_id, inv::drop_item(zone.char_id as i32, item, position)));
            }
        }
        handled
    }

    pub(crate) fn tooltip_body(&self, zone: &Zone, id: Identity) -> String {
        match super::super::chat::item_html(zone, &super::super::chat::ItemRequest::Identity { id, shop: false, container: Identity::default() }, &self.db) {
            Ok(body) => body.unwrap_or_default(),
            Err(error) => { eprintln!("item tooltip: {error:#}"); String::new() }
        }
    }

    pub(super) fn update_item_tooltip(&mut self, gui: &mut Gui, zone: &Zone) {
        let (x, y) = self.item_mouse;
        let slot = self.info_slot_at(gui, zone, x, y);
        let key = slot.and_then(|slot| zone.inventory.get(&slot).copied().map(|e| (slot, e, self.item_description_tips)));
        if key == self.item_tip { return; }
        self.item_tip = key;
        let body = if self.item_description_tips { slot.map(|s| self.tooltip_body(zone, inv::item_identity(s))).unwrap_or_default() } else { String::new() };
        let title = key.and_then(|(_, e, _)| self.items.info(gui, e.item)).map(|i| i.name.as_str()).unwrap_or("");
        if let Some(w) = self.wear.as_ref() {
            gui.set_tooltip(w.window, "items", if slot.is_some_and(|s| s < inv::BAG_FIRST) { title } else { "" }, if slot.is_some_and(|s| s < inv::BAG_FIRST) { &body } else { "" });
        }
        if let Some(i) = self.inventory.as_ref() {
            if i.list {
                if let Some(slot) = slot.filter(|s| *s >= inv::BAG_FIRST) {
                    gui.set_tooltip(i.window, &format!("item{slot}"), title, &body);
                }
            } else {
                gui.set_tooltip(i.window, "grid", if slot.is_some_and(|s| s >= inv::BAG_FIRST) { title } else { "" }, if slot.is_some_and(|s| s >= inv::BAG_FIRST) { &body } else { "" });
            }
        }
    }

    /// A short click on an item cell (`Event::CanvasClick`): the second click on the same cell within [`DOUBLE_CLICK`] seconds uses the item
    /// (`FUN_100ca1e7` -> `N3Msg_UseItem`): a worn item goes to the bag, a bag item is worn at its `DefaultPos` or used.
    pub(super) fn item_click(&mut self, zone: &Zone, place: Place) {
        if self.info_click {
            return;
        }
        let now = self.dnd.clock;
        let double = self.dnd.last_click.is_some_and(|(p, t)| p == place && now - t <= DOUBLE_CLICK);
        self.dnd.last_click = if double { None } else { Some((place, now)) };
        if !double {
            return;
        }
        let Some(slot) = self.slot_of(place) else { return };
        self.use_slot(zone, slot);
    }

    /// Shortcuts use the same native UseItem wear/default-position dispatch as clicks.
    pub(in crate::play) fn activate_item(&mut self, zone: &Zone, slot: u32) {
        self.use_slot(zone, slot);
    }

    /// The double click on the item in inventory slot `slot` (worn: to the bag; bag: worn at its `DefaultPos` or used).
    fn use_slot(&mut self, zone: &Zone, slot: u32) {
        let Some(e) = zone.inventory.get(&slot) else { return };
        let item = inv::item_identity(slot);
        let action = if slot < inv::BAG_FIRST {
            zone.free_bag_slot().map(|_| Action::Move { item, slot: inv::ANY_BAG_SLOT })
        } else {
            let destination = match self.items.wear_destination(e.item) {
                Ok(destination) => destination,
                Err(error) => { eprintln!("item wear: {error:#}"); return; }
            };
            match destination {
                Some((tab, id)) => {
                    let Some((page, _)) = dnd::tab_page(tab) else { return };
                    Some(Action::Move { item, slot: (page.base() + id) as i32 })
                }
                None => Some(Action::Use { item }),
            }
        };
        if let Some(a) = action {
            self.run(zone, a, None);
        }
    }

    fn can_equip(&mut self, zone: &Zone, source: u32, tab: usize, id: u32) -> bool {
        match self.items.can_equip(zone, source, tab, id) {
            Ok(allowed) => allowed,
            Err(error) => { eprintln!("item wear: {error:#}"); false }
        }
    }

    fn can_unwear(&mut self, zone: &Zone, slot: u32) -> bool {
        match self.items.can_unwear(zone, slot) {
            Ok(allowed) => allowed,
            Err(error) => { eprintln!("item unwear: {error:#}"); false }
        }
    }
}

#[cfg(test)]
impl HudStats {
    /// Live harness: double click on the item of inventory slot `slot` (wear / unwear / use).
    pub(in crate::play) fn live_double_click(&mut self, zone: &Zone, slot: u32) {
        self.use_slot(zone, slot);
    }

    /// Live harness: screen position of the centre of the cell showing `slot` (bag grid cell or the
    /// currently selected wear tab), for driving a real mouse click through `Gui`/`Hud::input`.
    pub(in crate::play) fn item_screen_pos(&self, gui: &Gui, slot: u32) -> Option<(f32, f32)> {
        if let Some(w) = &self.wear {
            if let Some(cell) = (0..15).find(|&cell| items::wear_slot(w.tab, cell) == Some(slot)) {
                let r = gui.view_rect(w.window, "items")?;
                let (c, row) = items::wear_cell(w.tab, cell);
                let (ox, oy) = items::wear_origin(c, row);
                return Some((r.l + ox + items::SLOT / 2.0, r.t + oy + items::SLOT / 2.0));
            }
        }
        let i = self.inventory.as_ref()?;
        let cell = i.positions.get(slot)?;
        let r = gui.view_rect(i.window, "grid")?;
        let (ox, oy) = inv_grid::cell_origin(cell.0, cell.1, i.gap);
        Some((r.l + ox + inv_grid::ICON / 2.0, r.t + oy + inv_grid::ICON / 2.0))
    }
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    #[test]
    fn native_implant_activation_and_removal_require_clinic_without_replacement() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut stats = HudStats::new(&dir, (800, 600)).unwrap();
        let mut zone = Zone::new(7);
        zone.stats.extend([(60, 1), (224, 0)]);
        // Native RDB25806: class3, DefaultPos13, Placement0x2000,
        // EquipDelay50cs. Its criteria key is Use3, not Wear6.
        let item = ao_net::n3::world::AcgItem { low_id: 25806, high_id: 25806, level: 1 };
        zone.inventory.insert(0x40, ao_net::n3::world::InventoryEntry { slot: 0x40, a: 0, b: 1, id: inv::item_identity(0x40), item });
        let destination = Place::Wear { tab: 2, id: 13, slot: 0x2d };
        assert!(stats.drop_action(&zone, 0x40, destination).is_none());
        stats.activate_item(&zone, 0x40);
        assert!(stats.take_outbox().is_empty());
        zone.stats.insert(224, 0x10);
        assert!(stats.drop_action(&zone, 0x40, destination).is_some());
        stats.activate_item(&zone, 0x40);
        assert_eq!(stats.take_outbox().into_iter().map(|f| f.payload).collect::<Vec<_>>(), [inv::move_item_to_inventory(7, inv::item_identity(0x40), 0x2d)]);
        assert!(zone.inventory.contains_key(&0x40), "request never fabricates the server move");
        zone.apply_inventory(&inv::InventoryMsg::ContainerAdd { item: inv::item_identity(0x40), container: Identity { kind: DYNEL_CHAR, instance: 7 }, slot: 0x2d });
        stats.wear_timers.tick(0.5);
        zone.inventory.insert(0x41, ao_net::n3::world::InventoryEntry { slot: 0x41, a: 0, b: 1, id: inv::item_identity(0x41), item });
        stats.activate_item(&zone, 0x41);
        assert!(stats.take_outbox().is_empty(), "native implants cannot replace an occupied slot");
        zone.stats.insert(224, 0);
        stats.activate_item(&zone, 0x2d);
        assert!(stats.take_outbox().is_empty(), "removal requires the clinic too");
        zone.stats.insert(224, 0x10);
        stats.activate_item(&zone, 0x2d);
        assert_eq!(stats.take_outbox().into_iter().map(|f| f.payload).collect::<Vec<_>>(), [inv::move_item_to_inventory(7, inv::item_identity(0x2d), inv::ANY_BAG_SLOT)]);
    }

    #[test]
    fn activation_sends_use_and_waits_for_server_upload() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/text").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let mut stats = HudStats::new(&dir, (800, 600)).unwrap();
        let mut zone = Zone::new(7);
        zone.inventory.insert(0x40, ao_net::n3::world::InventoryEntry {
            slot: 0x40, a: 0, b: 0, id: Identity::default(),
            item: ao_net::n3::world::AcgItem { low_id: 23314, high_id: 23314, level: 1 },
        });
        stats.activate_item(&zone, 0x40);
        let out = stats.take_outbox();
        assert_eq!(out.len(), 1);
        let decoded = ao_net::n3::decode(&out[0]).unwrap();
        let own = Identity { kind: DYNEL_CHAR, instance: 7 };
        assert_eq!(decoded.header.target, own);
        let ao_net::n3::N3::Misc(Misc::GenericCmd(cmd)) = decoded.body else { panic!("expected item use") };
        assert_eq!(cmd.cmd, CMD_USE_ITEM);
        assert_eq!(cmd.args, GenericArgs::Item { flag: 0, actor: own, item: inv::item_identity(0x40) });
        assert!(zone.nanos.programs.is_empty(), "sending use must not fake a successful upload");
        stats.activate_item(&zone, 0x41);
        assert!(stats.take_outbox().is_empty(), "missing item cannot be activated");
        let movement = Action::Move { item: inv::item_identity(0x40), slot: inv::ANY_BAG_SLOT };
        stats.set_nano_cast_busy(true);
        stats.run(&zone, Action::Move { item: inv::item_identity(0x40), slot: 6 }, None);
        assert!(stats.take_outbox().is_empty(), "native kind3 also blocks equip dispatch");
        assert!(stats.drop_action(&zone, 0x40, Place::Wear { tab: 0, id: 6, slot: 6 }).is_none(), "busy preview refuses equip");
        stats.run(&zone, movement.clone(), None);
        assert!(stats.take_outbox().is_empty(), "native kind3 blocks packet-backed moves");
        stats.set_nano_cast_busy(false);
        stats.wear_timers.start(2.0);
        stats.run(&zone, movement.clone(), None);
        assert!(stats.take_outbox().is_empty(), "native kind6 blocks packet-backed moves");
        stats.wear_timers.tick(2.0);
        stats.run(&zone, movement, None);
        assert_eq!(stats.take_outbox().len(), 1, "move resumes only after timer expires");
    }
}

#[cfg(test)]
mod stack_tests {
    use super::*;
    use ao_net::n3::world::{AcgItem, InventoryEntry};

    #[test]
    fn ctrl_horizontal_selection_splits_then_drag_rejoins_on_server_reply() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut hud = HudStats::new(&dir, (800, 600)).unwrap();
        let mut zone = Zone::new(7);
        hud.items.insert_test_info(AcgItem { low_id: 1, high_id: 1, level: 5 }, items::Info { name: "Stack".into(), icon: None });
        zone.inventory.insert(0x40, InventoryEntry { slot: 0x40, a: 1, b: 10, id: inv::item_identity(0x40), item: AcgItem { low_id: 1, high_id: 1, level: 5 } });
        hud.items.insert_test_acg(zone.inventory[&0x40].item, &[(dnd::STAT_CAN, 0x200)]);
        hud.open_inventory(&mut gui).unwrap();
        hud.update_inventory(&mut gui, &zone);
        let (x, y) = hud.item_screen_pos(&gui, 0x40).unwrap();
        let ctrl = ao_gui::Modifiers { ctrl: true, ..Default::default() };
        hud.input(&mut gui, &zone, &InputEvent::MouseDown { x, y, button: MouseButton::Left }, ctrl);
        hud.input(&mut gui, &zone, &InputEvent::MouseMove { x: x + 3.0, y }, ctrl);
        let selection = hud.dnd.split.as_ref().unwrap();
        assert_eq!(selection.count, 4);
        assert!(gui.text(selection.popup.unwrap(), "count").contains('4'));
        let mut host = ao_render::Host::headless();
        host.pointer_warp = hud.take_pointer_warp();
        assert_eq!(host.pointer_warp.take(), Some((x, y)));
        assert_eq!(host.pointer_warp.take(), None, "Host consumes the warp exactly once");
        hud.input(&mut gui, &zone, &InputEvent::MouseMove { x, y }, ctrl);
        assert_eq!(hud.dnd.split.as_ref().unwrap().count, 4, "the native warp event contributes no count");
        assert_eq!(hud.take_pointer_warp(), None, "a synthesized warp must not queue another warp");
        hud.input(&mut gui, &zone, &InputEvent::MouseUp { x: x + 3.0, y, button: MouseButton::Left }, ctrl);
        assert_eq!(hud.dnd.drag.as_ref().unwrap().count, 4);
        assert!(hud.take_outbox().is_empty(), "selecting a count only starts the carried drag");
        let inventory = hud.inventory.as_ref().unwrap();
        let grid = gui.view_rect(inventory.window, "grid").unwrap();
        let (dx, dy) = inv_grid::cell_origin(1, 0, inventory.gap);
        let (tx, ty) = (grid.l + dx + 10.0, grid.t + dy + 10.0);
        hud.input(&mut gui, &zone, &InputEvent::MouseUp { x: tx, y: ty, button: MouseButton::Left }, Default::default());
        assert_eq!(hud.take_outbox(), vec![n3_frame(0, 7, inv::split_item(7, inv::item_identity(0x40), 4))]);
        assert_eq!(zone.inventory[&0x40].b, 10, "UI never fabricates the reply");
        zone.on_frame(&n3_frame(0, 7, inv::split_item(7, inv::item_identity(0x40), 4)));
        assert_eq!((zone.inventory[&0x40].b, zone.inventory[&0x41].b), (6, 4));
        hud.update_inventory(&mut gui, &zone);
        let cell = hud.inventory.as_ref().unwrap().positions.get(0x40).unwrap();
        let action = hud.drop_action(&zone, 0x41, Place::Bag { cell }).unwrap();
        assert_eq!(action, Action::Join { from: inv::item_identity(0x41), onto: inv::item_identity(0x40) });
        hud.run(&zone, action, Some(Place::Bag { cell }));
        assert_eq!(hud.take_outbox(), vec![n3_frame(0, 7, inv::join_items(7, inv::item_identity(0x40), inv::item_identity(0x41)))]);
        assert_eq!(zone.inventory.len(), 2);
        zone.on_frame(&n3_frame(0, 7, inv::join_items(7, inv::item_identity(0x40), inv::item_identity(0x41))));
        assert_eq!(zone.inventory[&0x40].b, 10);
        assert!(!zone.inventory.contains_key(&0x41));
    }
}
