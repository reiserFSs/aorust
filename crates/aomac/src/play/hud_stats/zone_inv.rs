//! The server's inventory messages applied to [`Zone::inventory`] (`FUN_1004ad44` / `FUN_10047d77` / `FUN_1004cf08` [GC]; docs/gui.md §11.12).

use super::super::zone::Zone;
use ao_net::msg::Identity;
use ao_net::n3::inventory::{self as inv, InventoryMsg};

impl Zone {
    /// The own character's identity (`{0xC350, char_id}`).
    fn own_identity(&self) -> Identity {
        Identity { kind: 0xC350, instance: self.char_id as i32 }
    }

    /// First free bag slot (`FUN_1002a1b0(0x40)` over the bag range).
    pub fn free_bag_slot(&self) -> Option<u32> {
        (inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS).find(|s| !self.inventory.contains_key(s))
    }

    /// Apply one server inventory message addressed to the own character.
    ///
    /// * `ContainerAddItemIIR_t` with the own character as container and an item identity of the own pages (`FUN_10047d77(kind, from = item.instance,
    ///   to = slot)`): the item at `from` and the cell `to` swap places (`FUN_1002a200` swaps two vector cells), `to == 0x6f` means the first
    ///   free bag slot (`FUN_1002a1b0(0x40)`, nothing happens when the bag is full). The "has item" check `FUN_1002a82b(from)` makes a move of
    ///   an empty cell a no-op. Bank (`0x69`), backpack (`0x6b`), trade (`0x6e`) and the other special kinds of `FUN_1004ad44` and the pick-up
    ///   of a ground item (container `{0, 0}`, `FUN_10047eb2`: needs the item dynel's stats) are **not decoded** (UNRESOLVED): they leave the inventory unchanged.
    /// * `ItemReplacedIIR_c`: a worn slot (`< 0x40`) gets the new item when it differs from the current one (`FUN_1004cf08`).
    /// * `InventoryUpdate(d)IIR_t` concern chests / the signal only (`FUN_100a040e`, `FUN_10074e49`): the own inventory is not touched.
    pub fn apply_inventory(&mut self, msg: &InventoryMsg) {
        match msg {
            InventoryMsg::ContainerAdd { item, container, slot } => {
                if *container != self.own_identity() || inv::slot_kind(item.instance.max(0) as u32) != item.kind || item.instance < 0 {
                    return;
                }
                let from = item.instance as u32;
                let to = if *slot == inv::ANY_BAG_SLOT {
                    match self.free_bag_slot() {
                        Some(s) => s,
                        None => return,
                    }
                } else if *slot < 0 {
                    return;
                } else {
                    *slot as u32
                };
                if from == to || !self.inventory.contains_key(&from) {
                    return;
                }
                let a = self.inventory.remove(&from);
                let b = self.inventory.remove(&to);
                if let Some(mut e) = a {
                    e.slot = to;
                    self.inventory.insert(to, e);
                }
                if let Some(mut e) = b {
                    e.slot = from;
                    self.inventory.insert(from, e);
                }
                // `FUN_1002a64f(slot, 0)`: clears flag 0x40 of the cell's first word on the bag side (meaning UNRESOLVED)
                for s in [from, to] {
                    if s >= inv::BAG_FIRST {
                        if let Some(e) = self.inventory.get_mut(&s) {
                            e.a &= !0x40;
                        }
                    }
                }
            }
            InventoryMsg::ItemReplaced { slot, new, .. } => {
                if *slot >= 0 && (*slot as u32) < inv::BAG_FIRST {
                    if let Some(e) = self.inventory.get_mut(&(*slot as u32)) {
                        if e.item != *new {
                            e.item = *new;
                        }
                    }
                }
            }
            InventoryMsg::Updated(_) | InventoryMsg::Update(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::world::{AcgItem, InventoryEntry};

    fn put(z: &mut Zone, slot: u32, low: i32) {
        let id = inv::item_identity(slot);
        z.inventory.insert(slot, InventoryEntry { slot, a: 0x40, b: 1, id, item: AcgItem { low_id: low, high_id: low, level: 1 } });
    }

    fn add(from: u32, to: i32) -> InventoryMsg {
        InventoryMsg::ContainerAdd { item: inv::item_identity(from), container: Identity { kind: 0xC350, instance: 7 }, slot: to }
    }

    #[test]
    fn container_add_moves_and_swaps() {
        let mut z = Zone::new(7);
        put(&mut z, 0x40, 100);
        put(&mut z, 0x41, 200);
        // bag -> empty worn slot
        z.apply_inventory(&add(0x40, 0x13));
        assert!(!z.inventory.contains_key(&0x40));
        assert_eq!((z.inventory[&0x13].slot, z.inventory[&0x13].item.low_id), (0x13, 100));
        // worn -> occupied bag slot swaps, the displaced item goes to the worn slot
        z.apply_inventory(&add(0x13, 0x41));
        assert_eq!((z.inventory[&0x41].item.low_id, z.inventory[&0x13].item.low_id), (100, 200));
        assert_eq!(z.inventory[&0x13].slot, 0x13);
        // 0x6f = first free bag slot; unequip item 0x13
        z.apply_inventory(&add(0x13, inv::ANY_BAG_SLOT));
        assert_eq!(z.inventory[&0x40].item.low_id, 200);
        assert!(!z.inventory.contains_key(&0x13));
    }

    #[test]
    fn container_add_ignores_what_it_cannot_apply() {
        let mut z = Zone::new(7);
        put(&mut z, 0x40, 100);
        let before = z.inventory.clone();
        // wrong container, wrong identity kind for the slot, empty source cell, full bag for 0x6f, negative slots
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: inv::item_identity(0x40), container: Identity { kind: 0xC350, instance: 8 }, slot: 0x41 });
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: Identity { kind: 0x66, instance: 0x40 }, container: Identity { kind: 0xC350, instance: 7 }, slot: 0x41 });
        z.apply_inventory(&add(0x42, 0x41));
        z.apply_inventory(&add(0x40, -1));
        assert_eq!(z.inventory, before);
        for s in inv::BAG_FIRST + 1..inv::BAG_FIRST + inv::BAG_SLOTS {
            put(&mut z, s, s as i32);
        }
        put(&mut z, 0x12, 5);
        let before = z.inventory.clone();
        z.apply_inventory(&add(0x12, inv::ANY_BAG_SLOT));
        assert_eq!(z.inventory, before);
    }

    #[test]
    fn item_replaced_only_changes_worn_slots() {
        let mut z = Zone::new(7);
        put(&mut z, 0x12, 5);
        put(&mut z, 0x40, 6);
        let new = AcgItem { low_id: 9, high_id: 9, level: 3 };
        z.apply_inventory(&InventoryMsg::ItemReplaced { slot: 0x12, old: z.inventory[&0x12].item, new });
        z.apply_inventory(&InventoryMsg::ItemReplaced { slot: 0x40, old: z.inventory[&0x40].item, new });
        assert_eq!((z.inventory[&0x12].item, z.inventory[&0x40].item.low_id), (new, 6));
    }
}
