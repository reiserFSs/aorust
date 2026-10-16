//! The server's inventory messages applied to [`Zone::inventory`] (`FUN_1004ad44` / `FUN_10047d77` / `FUN_1004cf08` [GC]; docs/gui.md §11.12).

use super::super::zone::Zone;
use ao_net::msg::Identity;
use ao_net::n3::inventory::{self as inv, InventoryMsg};

impl Zone {
    /// `N3Msg_ToggleReclaim` queries registered container kind DEAE (GC 0x1003f03e).
    pub(in crate::play) fn reclaim_registered(&self) -> bool {
        !self.reclaim_registrations.is_empty()
    }

    /// GC1001753e ->10046bb5 ->1012584a: DEAE selects the current manager+180
    /// inventory, regardless of requested instance or registration membership.
    pub(in crate::play) fn container_inventory<'a>(
        &'a self,
        container: Identity,
        mut can30: impl FnMut(ao_net::n3::world::AcgItem) -> Option<i32> + 'a,
    ) -> impl Iterator<Item = ao_net::n3::world::InventoryEntry> + 'a {
        let registered = if container.kind == 0xdeae {
            self.containers.iter().find(|((kind, _), _)| *kind == 0xdeae)
        } else {
            self.containers.get_key_value(&(container.kind, container.instance))
        };
        let all = registered.is_none_or(|(&(kind, instance), _)| kind != 0xdeae || instance == self.char_id as i32);
        registered.into_iter().flat_map(|(_, (_, entries))| entries).filter_map(move |entry| {
            // Occupancy is the presence of a cell (1002a379), not raw.a bit 1.
            if !all && entry.a & 0x20 == 0 && can30(entry.item).unwrap_or(0) & 0x40 == 0 {
                return None;
            }
            let mut entry = *entry;
            if container.kind == 0xdeae {
                entry.id = Identity { kind: 0x6a, instance: entry.slot as i32 };
            }
            Some(entry)
        })
    }

    /// The own character's identity (`{0xC350, char_id}`).
    fn own_identity(&self) -> Identity {
        Identity { kind: 0xC350, instance: self.char_id as i32 }
    }

    /// First free bag slot (`FUN_1002a1b0(0x40)` over the bag range).
    pub fn free_bag_slot(&self) -> Option<u32> {
        (inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS).find(|s| !self.inventory.contains_key(s))
    }

    /// GC1004b80a checks source access first, then destination access.
    /// GC100a0310/1002ae7b/100a040e preserve these server-supplied masks.
    pub(in crate::play) fn container_move_error(&self, item: Identity, destination: Identity) -> Option<(u32, &'static str)> {
        let source = if item.kind == inv::KIND_IN_CONTAINER {
            self.containers.iter().find(|(_, (word, _))| i32::from(*word as i16) == item.instance >> 16).map(|(key, _)| *key)
        } else if item.kind == inv::KIND_BANK {
            Some((0xdead, self.char_id as i32))
        } else { None };
        if source.and_then(|key| self.container_access.get(&key)).is_some_and(|access| access & 2 == 0) {
            return Some((10003, "Inv_SrcCantRemove"));
        }
        if self.container_access.get(&(destination.kind, destination.instance)).is_some_and(|access| access & 1 == 0) {
            return Some((10003, "Inv_DstCantAdd"));
        }
        None
    }

    /// GC 1005d4f6 -> 10049e8f: confirmed split creates a copy in the first free slot.
    pub(in crate::play) fn split_inventory_item(&mut self, item: Identity, count: i32) {
        if count <= 0 { return; }
        if item.kind == inv::KIND_BAG {
            let Some(to) = self.free_bag_slot() else { return };
            let Some(source) = self.inventory.get_mut(&(item.instance as u32)) else { return };
            let total = i32::from(source.b as u16);
            if count >= total { return; }
            source.b = (total - count) as i16;
            let mut entry = *source;
            entry.slot = to;
            entry.id = inv::item_identity(to);
            entry.b = count as i16;
            self.inventory.insert(to, entry);
        } else if item.kind == inv::KIND_IN_CONTAINER {
            let Some((word, entries)) = self.containers.values_mut().find(|(word, _)| i32::from(*word as i16) == item.instance >> 16) else { return };
            let Some(to) = (0..21).find(|s| !entries.iter().any(|e| e.slot == *s)) else { return };
            let Some(source) = entries.iter_mut().find(|e| e.slot == (item.instance & 0xffff) as u32) else { return };
            let total = i32::from(source.b as u16);
            if count >= total { return; }
            source.b = (total - count) as i16;
            let mut entry = *source;
            entry.slot = to;
            entry.id = inv::container_item_identity(*word, to);
            entry.b = count as i16;
            entries.push(entry);
            entries.sort_by_key(|e| e.slot);
        }
    }

    /// GC 1005d50e -> 10049cfe: first identity stays, second disappears.
    pub(in crate::play) fn join_inventory_items(&mut self, onto: Identity, from: Identity) {
        if onto == from || onto.kind != from.kind { return; }
        if onto.kind == inv::KIND_BAG {
            let (Some(a), Some(b)) = (self.inventory.get(&(onto.instance as u32)), self.inventory.get(&(from.instance as u32))) else { return };
            let total = u32::from(a.b as u16) + u32::from(b.b as u16);
            if a.item != b.item || total > 50_000 { return; }
            self.inventory.remove(&(from.instance as u32));
            self.inventory.get_mut(&(onto.instance as u32)).expect("found above").b = total as i16;
        } else if onto.kind == inv::KIND_IN_CONTAINER && onto.instance >> 16 == from.instance >> 16 {
            let Some((_, entries)) = self.containers.values_mut().find(|(word, _)| i32::from(*word as i16) == onto.instance >> 16) else { return };
            let (Some(a), Some(b)) = (entries.iter().position(|e| e.slot == (onto.instance & 0xffff) as u32), entries.iter().position(|e| e.slot == (from.instance & 0xffff) as u32)) else { return };
            let total = u32::from(entries[a].b as u16) + u32::from(entries[b].b as u16);
            if entries[a].item != entries[b].item || total > 50_000 { return; }
            entries[a].b = total as i16;
            entries.remove(b);
        }
    }

    /// GC 10017707 -> 100489be/10087e68: inventory membership, including one
    /// container parent. A world chest/corpse being cached does not imply ownership.
    pub(in crate::play) fn is_item_mine(&self, item: Identity) -> bool {
        if item == Identity::default() { return false; }
        let carried = |id| self.inventory.values().any(|e| e.id == id || inv::item_identity(e.slot) == id);
        if carried(item) { return true; }
        self.containers.iter().any(|(&(kind, instance), (word, entries))| {
            carried(Identity { kind, instance }) && entries.iter().any(|e|
                e.id == item || inv::container_item_identity(*word, e.slot) == item)
        })
    }

    /// Server `CharacterActionIIR_t` 0x70: GC 0x1005d8f1 -> 0x1004a191.
    /// `identity_b.kind > 0` consumes one stack unit; otherwise the whole item is deleted.
    pub(in crate::play) fn delete_inventory_item(&mut self, item: Identity, one: bool) {
        if item.kind == inv::KIND_BAG && item.instance >= inv::BAG_FIRST as i32 {
            let slot = item.instance as u32;
            if let Some(entry) = self.inventory.get_mut(&slot) {
                if one && entry.b as u16 > 1 {
                    entry.b = ((entry.b as u16) - 1) as i16;
                } else {
                    self.inventory.remove(&slot);
                }
            }
            return;
        }
        let slot = if item.kind == inv::KIND_IN_CONTAINER { (item.instance & 0xffff) as u32 } else if item.instance >= 0 { item.instance as u32 } else { return };
        let list = match item.kind {
            inv::KIND_IN_CONTAINER => self.containers.values_mut().find(|(word, _)| i32::from(*word as i16) == item.instance >> 16),
            inv::KIND_BANK => self.containers.get_mut(&(0xdead, self.char_id as i32)),
            inv::KIND_OVERFLOW => self.containers.get_mut(&(inv::KIND_OVERFLOW, self.char_id as i32)),
            _ => None, // worn pages (65/66/67/73) are refused by FUN_10047242
        };
        if let Some((_, entries)) = list {
            if let Some(at) = entries.iter().position(|e| e.slot == slot) {
                if one && entries[at].b as u16 > 1 {
                    entries[at].b = ((entries[at].b as u16) - 1) as i16;
                } else {
                    entries.remove(at);
                }
            }
        }
    }

    /// GC10069274 / 10049036 / 10049073: source count replacement/removal
    /// precedes weapon lookup, including when the weapon is absent or Ammo is negative.
    pub(in crate::play) fn apply_reload(&mut self, source: Identity, weapon: Identity, remaining: i32, clip: i32, remove_source: bool) -> bool {
        let slot = if source.kind == inv::KIND_IN_CONTAINER { (source.instance & 0xffff) as u32 } else { source.instance as u32 };
        // GC1002ad45 stores a u16; out-of-range counts become zero, not saturation.
        let remaining = if (0..=0xffff).contains(&remaining) { remaining as i16 } else { 0 };
        if !(0x65..0xfa).contains(&source.kind) && remove_source {
            // GC1002aa2d: non-page identities remove the first matching inventory dynel.
            if let Some(slot) = self.inventory.iter().filter(|(_, e)| e.id == source).map(|(&slot, _)| slot).min() {
                self.inventory.remove(&slot);
            }
        }
        if matches!(source.kind, 0x65..=0x68 | 0x73) && source.instance >= 0 {
            if remove_source { self.inventory.remove(&slot); }
            else if let Some(e) = self.inventory.get_mut(&slot) { e.b = remaining; }
        } else {
            let list = match source.kind {
                inv::KIND_IN_CONTAINER => self.containers.values_mut().find(|(word, _)| i32::from(*word as i16) == source.instance >> 16),
                inv::KIND_BANK => self.containers.get_mut(&(0xdead, self.char_id as i32)),
                inv::KIND_TRADE => self.containers.get_mut(&(inv::KIND_TRADE, self.char_id as i32)),
                _ => None,
            };
            if let Some((_, entries)) = list {
                if remove_source { entries.retain(|e| e.slot != slot); }
                else if let Some(e) = entries.iter_mut().find(|e| e.slot == slot) { e.b = remaining; }
            }
        }
        if let Some(recharge) = self.world.arms.reload(weapon, clip) {
            self.world.start_reload_bar(recharge);
            true
        } else { false }
    }

    /// Apply one server inventory message addressed to the own character.
    ///
    /// * `ContainerAddItemIIR_t` with the own character as container and an item identity of the own pages (`FUN_10047d77(kind, from = item.instance,
    ///   to = slot)`): the item at `from` and the cell `to` swap places (`FUN_1002a200` swaps two vector cells), `to == 0x6f` means the first
    ///   free bag slot (`FUN_1002a1b0(0x40)`, nothing happens when the bag is full). The "has item" check `FUN_1002a82b(from)` makes a move of
    ///   an empty cell a no-op. An item `{0x6b, word << 16 | slot}` of a corpse / chest list moves into the bag (`take_from_container`).
    ///   A temporary overflow item (`0x6e`, created by `TemplateActionIIR_t` action 0x57) transfers into the first free bag slot (`FUN_100475ae`).
    ///   Picking up a ground item (container `{0, 0}`, `FUN_10047eb2`) still requires the item dynel's stats and is not decoded here.
    /// * `ItemReplacedIIR_c`: a worn slot (`< 0x40`) gets the new item when it differs from the current one (`FUN_1004cf08`).
    /// * `InventoryUpdateIIR_t` of a chest / corpse stores its list in [`Zone::containers`] (`FUN_100a040e`); `InventoryUpdatedIIR_t` is the signal only (`FUN_10074e49`).
    pub fn apply_inventory(&mut self, msg: &InventoryMsg) {
        match msg {
            InventoryMsg::Reload { source, weapon, remaining, clip, remove_source } => {
                self.apply_reload(*source, *weapon, *remaining, *clip, *remove_source);
            }
            // GC 1004d148 action 0x57: create an actual server-described item in the temporary inventory.
            InventoryMsg::TemplateAction { item, count, action: 0x57, .. } => {
                let (_, entries) = self.containers.entry((inv::KIND_OVERFLOW, self.char_id as i32)).or_default();
                let slot = (0..=entries.len() as u32).find(|slot| !entries.iter().any(|e| e.slot == *slot)).unwrap();
                entries.push(ao_net::n3::world::InventoryEntry {
                    slot,
                    a: 0x21,
                    b: *count as i16,
                    id: Identity { kind: inv::KIND_OVERFLOW, instance: slot as i32 },
                    item: *item,
                });
            }
            InventoryMsg::ContainerAdd { item, container, .. }
                if item.kind == inv::KIND_OVERFLOW && container.kind == inv::KIND_OVERFLOW && container.instance == self.char_id as i32 =>
            {
                // GC 100475ae: the wire destination is ignored; transfer the temporary item into the first free bag cell.
                let Some(slot) = self.free_bag_slot() else { return };
                let Some((_, entries)) = self.containers.get_mut(&(inv::KIND_OVERFLOW, self.char_id as i32)) else { return };
                let Some(at) = entries.iter().position(|e| e.slot as i32 == item.instance) else { return };
                let mut entry = entries.remove(at);
                entry.slot = slot;
                entry.id = inv::item_identity(slot);
                self.inventory.insert(slot, entry);
            }
            // GC1004ad44 ->100472d7: contract item0x6d is copied to
            // the first free bag cell and removed from the contract inventory.
            InventoryMsg::ContainerAdd { item, container, .. }
                if item.kind == ao_net::n3::inventory::KIND_CONTRACT && (*container == self.own_identity() || *container == Identity::default()) =>
            {
                let Some(slot) = self.free_bag_slot() else { return };
                let Some((_, (_, entries))) = self.containers.iter_mut().find(|((kind, _), _)| *kind == 0xdeaa) else { return };
                let Some(at) = entries.iter().position(|e| e.slot as i32 == item.instance) else { return };
                let mut entry = entries.remove(at);
                entry.slot = slot;
                entry.id = inv::item_identity(slot);
                entry.b = 1;
                self.inventory.insert(slot, entry);
            }
            // GC10048c32 removes the deposited inventory item; a subsequent
            // OrgServer snapshot supplies the contract inventory descriptor.
            InventoryMsg::ContainerAdd { item, container, .. } if container.kind == 0xdeaa => {
                self.delete_inventory_item(*item, false);
            }
            InventoryMsg::Bank(entries) => {
                self.containers.insert((0xdead, self.char_id as i32), (0, entries.clone()));
            }
            InventoryMsg::Reclaim { entries, instance } => {
                // Native activation first resolves an existing SimpleChar. Its
                // +140 emission flag is set by own SCFU10077af2/SetMainDynel10019f77.
                if self.own().is_none() { return; }
                // GC10071de0 creates DEAE:0; 10071e95 replaces its instance with
                // the trailing wire word and replaces manager+180, not a second list.
                self.containers.retain(|&(kind, _), _| kind != 0xdeae);
                let mut entries = entries.clone();
                entries.sort_by_key(|entry| entry.slot);
                self.containers.insert((0xdeae, *instance), (0, entries));
                if self.reclaim_registrations.insert(*instance) {
                    self.reclaim_events.push((Identity { kind: 0xdeae, instance: *instance }, true));
                }
            }
            // GC1004ad44 ->10046b0b: reclaim always takes the first free bag cell.
            InventoryMsg::ContainerAdd { item, container, .. }
                if item.kind == 0x6a && (*container == self.own_identity() || *container == Identity::default()) =>
            {
                let Some(slot) = self.free_bag_slot() else { return };
                let Some((_, (_, entries))) = self.containers.iter_mut().find(|((kind, _), _)| *kind == 0xdeae) else { return };
                let Some(at) = entries.iter().position(|e| e.slot as i32 == item.instance && e.a & 3 != 0) else { return };
                let mut entry = entries.remove(at);
                entry.slot = slot;
                entry.id = inv::item_identity(slot);
                self.inventory.insert(slot, entry);
            }
            InventoryMsg::ContainerAdd { item, container, .. }
                if container.kind == 0xc749 || (container.kind == 0xdead && container.instance == self.char_id as i32) =>
            {
                self.put_into_container(*item, *container);
            }
            InventoryMsg::ContainerAdd { item, container, .. } if item.kind == inv::KIND_BANK && *container == self.own_identity() => {
                let Some(slot) = self.free_bag_slot() else { return };
                let Some((_, entries)) = self.containers.get_mut(&(0xdead, self.char_id as i32)) else { return };
                let Some(at) = entries.iter().position(|e| e.slot as i32 == item.instance) else { return };
                let mut e = entries.remove(at);
                e.slot = slot;
                self.inventory.insert(slot, e);
            }
            InventoryMsg::ContainerAdd { item, container, slot } if item.kind == inv::KIND_IN_CONTAINER => self.take_from_container(*item, *container, *slot),
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
            // `FUN_100a040e`: the contents of a chest / corpse replace the stored list (the own character's pages are `FullCharacterIIR_t`'s)
            InventoryMsg::Update(u) if u.container.kind != 0xC350 || self.trade_state.as_ref().is_some_and(|s| s.partner == u.container) => {
                let mut entries = u.entries.clone();
                entries.sort_by_key(|e| e.slot);
                self.containers.insert((u.container.kind, u.container.instance), (u.word, entries));
                self.container_access.insert((u.container.kind, u.container.instance), u.access);
            }
            InventoryMsg::Updated(_) | InventoryMsg::Update(_) | InventoryMsg::TemplateAction { .. } => {}
        }
    }
}

impl Zone {
    /// GC 1004abe5/10048678/100492fa: chest and bank deposits use the first free
    /// destination cell. A full or unknown destination never consumes the source.
    fn put_into_container(&mut self, item: Identity, container: Identity) {
        let target = (container.kind, container.instance);
        let Some((word, entries)) = self.containers.get(&target) else { return };
        let word = *word;
        let capacity = if container.kind == 0xdead { 102 } else { 21 };
        let Some(slot) = (0..capacity).find(|s| !entries.iter().any(|e| e.slot == *s)) else { return };
        let source = if item.kind == inv::KIND_IN_CONTAINER {
            self.containers.iter().find(|(_, (word, entries))|
                i32::from(*word as i16) == item.instance >> 16 && entries.iter().any(|e| e.slot == (item.instance & 0xffff) as u32)).map(|(key, _)| *key)
        } else { None };
        let entry = if let Some(source) = source {
            if source == target { return; }
            let entries = &mut self.containers.get_mut(&source).expect("source found above").1;
            entries.iter().position(|e| e.slot == (item.instance & 0xffff) as u32).map(|at| entries.remove(at))
        } else if item.instance >= 0 && inv::slot_kind(item.instance as u32) == item.kind {
            self.inventory.remove(&(item.instance as u32))
        } else { None };
        if let Some(mut entry) = entry {
            entry.slot = slot;
            entry.id = if container.kind == 0xdead { Identity { kind: inv::KIND_BANK, instance: slot as i32 } } else { inv::container_item_identity(word, slot) };
            let entries = &mut self.containers.get_mut(&target).expect("destination found above").1;
            entries.push(entry);
            entries.sort_by_key(|e| e.slot);
        }
    }

    fn container_stack_slot(&mut self, item: Identity) -> Option<u32> {
        if item.kind != inv::KIND_IN_CONTAINER { return None; }
        let source = self.containers.values().find(|(word, _)| i32::from(*word as i16) == item.instance >> 16)?
            .1.iter().find(|e| e.slot == (item.instance & 0xffff) as u32).copied()?;
        if source.a & 1 == 0 { return None; }
        let can = self.world.item_template_stat(source.item, 30)?;
        if can & 0x200 == 0 || can & 0x1000000 != 0 { return None; }
        (inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS).find(|s| self.inventory.get(s).is_some_and(|e| e.a & 1 != 0 && e.item == source.item))
    }

    pub(in crate::play) fn can_take_from_container(&mut self, item: Identity) -> bool {
        self.free_bag_slot().is_some() || self.container_stack_slot(item).is_some()
    }

    /// `ContainerAddItemIIR_t` for an item `{0x6b, word << 16 | slot}` into the own character (`FUN_1004ad44` -> `FUN_1004a7b3(item, slot)` [GC]): the item
    /// is looked up in the container registered under `word` (`FUN_10048644`, the list of its last `InventoryUpdateIIR_t`), the cell `slot` of that
    /// list is emptied (`FUN_1002a64f(slot, 0)`) and the item goes to the bag slot `slot_to` (`0x6f` / `-1` = the first free bag slot
    /// `FUN_1002a1b0(0x40)`, nothing happens when the bag is full). `FUN_1002a5ff` first finds an identical
    /// stack for automatic destinations; `FUN_1002a2bc` merges up to 50000, otherwise the native move swaps the cells.
    fn take_from_container(&mut self, item: Identity, container: Identity, slot_to: i32) {
        if container != self.own_identity() {
            return;
        }
        let (key, from) = (item.instance >> 16, (item.instance & 0xffff) as u32);
        let Some(&at) = self.containers.iter().find(|(_, (word, l))| i32::from(*word as i16) == key && l.iter().any(|e| e.slot == from)).map(|(k, _)| k) else { return };
        let stack = if matches!(slot_to, inv::ANY_BAG_SLOT | -1) { self.container_stack_slot(item) } else { None };
        let to = match slot_to {
            inv::ANY_BAG_SLOT | -1 => match stack.or_else(|| self.free_bag_slot()) {
                Some(s) => s,
                None => return,
            },
            s if (inv::BAG_FIRST as i32..(inv::BAG_FIRST + inv::BAG_SLOTS) as i32).contains(&s) => s as u32,
            _ => return,
        };
        let list = &mut self.containers.get_mut(&at).expect("found above").1;
        let mut e = list.remove(list.iter().position(|e| e.slot == from).expect("found above"));
        if let Some(target) = self.inventory.get_mut(&to) {
            let total = u32::from(target.b as u16) + u32::from(e.b as u16);
            if stack.is_some() && total <= 50_000 {
                target.b = total as i16;
                return;
            }
        }
        e.slot = to;
        e.a &= !0x40;
        e.id = inv::item_identity(to);
        if let Some(mut displaced) = self.inventory.insert(to, e) {
            displaced.slot = from;
            displaced.id = item;
            list.push(displaced);
            list.sort_by_key(|e| e.slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::world::{AcgItem, InventoryEntry};

    #[test]
    fn native_10071e95_10046bb5_1012584a_reclaim_projection_and_context() {
        use ao_net::{wire::Writer, n3::{action, outgoing::n3_frame}};
        let mut z = Zone::new(7);
        let mut items = super::super::items::Items::new(std::path::Path::new("/nonexistent/reclaim-fixture"));
        let acg = |id| AcgItem { low_id: id, high_id: id, level: 1 };
        items.insert_test_acg(acg(10), &[(30, 0), (0, 0x40)]);
        items.insert_test_acg(acg(20), &[(30, 0x40), (0, 0)]);
        let query = Identity { kind: 0xdeae, instance: -123 };
        let registration = |instance| {
            let mut w = Writer::default();
            w.u32(inv::BANK_CORPSE);
            w.i32(0xc350); w.i32(7); w.u8(1);
            w.i32(0x3f1 * 5);
            // Sparse occupied cells: native1002a379 checks cell pointers, not a&1.
            for (slot, a, id) in [(9, 0, 10), (2, 0x20, 999), (6, 0, 20), (4, 0, 999)] {
                w.i32(slot); w.i16(a); w.i16(1);
                w.i32(0); w.i32(0);
                w.i32(id); w.i32(id); w.i32(1);
                w.i32(0);
            }
            w.i32(instance);
            n3_frame(0, 0, w.0)
        };
        assert_eq!(z.container_inventory(query, |_| None).count(), 0);
        z.on_frame(&registration(7));
        assert!(!z.reclaim_registered(), "10071e95 ignores an absent target SimpleChar");
        z.dynels.insert(7, super::super::super::zone::DynelState {
            name: String::new(), pos: [0.0; 3], yaw: None, npc: false, side: 0, level: 1, health: 10, max_health: 10,
        });
        z.on_frame(&registration(7));
        assert_eq!(z.reclaim_events, [(Identity { kind: 0xdeae, instance: 7 }, true)]);
        z.reclaim_events.clear();
        z.on_frame(&registration(7));
        assert!(z.reclaim_events.is_empty(), "registered identity is not another open");
        let rows = z.container_inventory(query, |item| items.acg_stat(item, 30).ok().flatten()).collect::<Vec<_>>();
        assert_eq!(rows.iter().map(|e| e.slot).collect::<Vec<_>>(), [2, 4, 6, 9]);
        assert!(rows.iter().all(|e| e.id == Identity { kind: 0x6a, instance: e.slot as i32 }));
        z.on_frame(&registration(8));
        assert!(!z.containers.contains_key(&(0xdeae, 7)), "10071e95 replaces the active context");
        let rows = z.container_inventory(query, |item| items.acg_stat(item, 30).ok().flatten()).collect::<Vec<_>>();
        assert_eq!(rows.iter().map(|e| e.slot).collect::<Vec<_>>(), [2, 6], "1002ace0: raw0x20 OR resolved Can30&0x40; Flags0 is not Can");
        assert_eq!(z.containers[&(0xdeae, 8)].1.len(), 4, "projection never filters raw storage");
        let off = action::simple(0x6e, Identity::default(), Identity::default());
        z.on_frame(&n3_frame(0, 0, action::character_action(7, &off)));
        assert_eq!(z.reclaim_events.last(), Some(&(Identity { kind: 0xdeae, instance: 8 }, false)));
        assert!(z.reclaim_registrations.contains(&7), "off removes only current identity");
        assert!(!z.reclaim_registrations.contains(&8));
        assert_eq!(z.container_inventory(query, |_| None).map(|e| e.slot).collect::<Vec<_>>(), [2], "off preserves manager+180 raw data; absent Can still rejects non0x20 rows");
        z.reclaim_events.clear();
        z.on_frame(&n3_frame(0, 0, action::character_action(7, &off)));
        assert!(z.reclaim_events.is_empty(), "duplicate off has no native close");
        z.on_frame(&registration(7));
        z.reset_world();
        assert_eq!(z.container_inventory(query, |_| None).count(), 0);
        assert!(z.reclaim_registrations.is_empty());
        assert!(z.reclaim_events.is_empty());
    }

    #[test]
    fn acg_endpoint_can_and_flags_follow_quality_and_tie_high_for_stack_eligibility() {
        let mut z = Zone::new(7);
        z.world.test_inventory_endpoint(10, 1, 0x200, 0);
        z.world.test_inventory_endpoint(20, 9, 0x1000200, 0x4000000);
        for slot in inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS { put(&mut z, slot, 99); }
        let container = (0xc76a, 22);
        let id = inv::container_item_identity(0x77, 0);
        for (ql, eligible, flags) in [(4, true, 0), (5, false, 0x4000000), (8, false, 0x4000000)] {
            let item = AcgItem { low_id: 10, high_id: 20, level: ql };
            z.inventory.insert(0x40, InventoryEntry { slot: 0x40, a: 1, b: 10, id: inv::item_identity(0x40), item });
            z.containers.insert(container, (0x77, vec![InventoryEntry { slot: 0, a: 1, b: 2, id, item }]));
            assert_eq!(z.world.item_template_stat(item, 0), Some(flags));
            assert_eq!(z.can_take_from_container(id), eligible, "QL {ql}: Can must not always come from low endpoint");
        }
        let missing = AcgItem { low_id: 10, high_id: 999, level: 1 };
        assert_eq!(z.world.item_template_stat(missing, 30), None, "missing high endpoint must not become a low fallback");
    }

    #[test]
    fn contract_withdrawal_retains_full_bag_source_and_deposit_waits_for_snapshot() {
        let mut z = Zone::new(7);
        let container = Identity { kind: 0xdeaa, instance: 9 };
        let entry = InventoryEntry { slot: 29, a: 1, b: 1, id: Identity::default(), item: AcgItem { low_id: 5, high_id: 5, level: 1 } };
        z.apply_inventory(&InventoryMsg::Update(inv::InventoryUpdate { capacity: 30, access: 3, entries: vec![entry], container, word: 0, flag: true }));
        for slot in inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS { put(&mut z, slot, 100); }
        let take = InventoryMsg::ContainerAdd { item: Identity { kind: inv::KIND_CONTRACT, instance: 29 }, container: z.own_identity(), slot: inv::ANY_BAG_SLOT };
        z.apply_inventory(&take);
        assert_eq!(z.containers[&(container.kind, container.instance)].1.len(), 1);
        z.inventory.remove(&0x45);
        z.apply_inventory(&take);
        assert_eq!(z.inventory[&0x45].item, entry.item);
        assert!(z.containers[&(container.kind, container.instance)].1.is_empty());
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: inv::item_identity(0x45), container, slot: 0 });
        assert!(!z.inventory.contains_key(&0x45));
        assert!(z.containers[&(container.kind, container.instance)].1.is_empty());
    }

    #[test]
    fn synthetic_loot_access_update_preserves_native_source_then_destination_refusals() {
        use ao_net::n3::{self, outgoing};
        use ao_net::wire::Writer;
        let mut z = Zone::new(7);
        let source = Identity { kind: 0xc76a, instance: 8 };
        let target = Identity { kind: 0xc749, instance: 9 };
        let item = inv::container_item_identity(0x8001, 3);
        let update = |container: Identity, access, word| {
            let mut w = Writer::default();
            w.u32(inv::INVENTORY_UPDATE);
            Identity { kind: 0xc350, instance: 7 }.write(&mut w);
            w.u8(0);
            w.i32(21);
            w.i32(access);
            w.i32(0x3f1); // empty inventory, native count encoding
            container.write(&mut w);
            w.i32(word);
            w.i32(0); // refresh rights without reopening the view
            outgoing::n3_frame(1, 7, w.0)
        };
        for (container, access, word) in [(source, 0, 0x8001), (target, 0, 0x71)] {
            let frame = update(container, access, word);
            assert!(matches!(n3::decode(&frame).unwrap().body, n3::N3::Inventory(_)));
            z.on_frame(&frame);
        }
        assert_eq!(z.container_move_error(item, target), Some((10003, "Inv_SrcCantRemove")));
        z.on_frame(&update(source, 2, 0x8001));
        assert_eq!(z.container_move_error(item, target), Some((10003, "Inv_DstCantAdd")));
        z.on_frame(&update(target, 1, 0x71));
        assert_eq!(z.container_move_error(item, target), None);
        assert_eq!(z.container_move_error(item, z.own_identity()), None);
        z.on_frame(&update(source, 1, 0x8001));
        assert_eq!(z.container_move_error(item, z.own_identity()), Some((10003, "Inv_SrcCantRemove")));
    }

    #[test]
    fn confirmed_loot_joins_full_bag_only_with_native_can_and_matching_acg() {
        let mut z = Zone::new(7);
        for slot in inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS { put(&mut z, slot, 10); }
        z.world.test_inventory_can(10, 0x200);
        let bag = z.inventory.get_mut(&0x40).unwrap();
        bag.a = 1;
        bag.b = 49_998u16 as i16;
        let mut loot = *bag;
        loot.slot = 0;
        loot.b = 2;
        let source = Identity { kind: 0xc76a, instance: 22 };
        let item = inv::container_item_identity(0x77, 0);
        z.containers.insert((source.kind, source.instance), (0x77, vec![loot]));
        let reply = InventoryMsg::ContainerAdd { item, container: z.own_identity(), slot: inv::ANY_BAG_SLOT };
        assert!(z.can_take_from_container(item), "the production request guard must admit full-bag merging");
        z.apply_inventory(&reply);
        assert_eq!(z.inventory[&0x40].b as u16, 50_000);
        assert!(z.containers[&(source.kind, source.instance)].1.is_empty());
        z.world.test_inventory_can(10, 0x1000200);
        z.containers.get_mut(&(source.kind, source.instance)).unwrap().1.push(loot);
        assert!(!z.can_take_from_container(item));
        z.apply_inventory(&reply);
        assert_eq!(z.containers[&(source.kind, source.instance)].1.len(), 1, "cannot-split forbids automatic merging and the bag is full");
        assert_eq!(z.inventory[&0x40].b as u16, 50_000);
    }

    #[test]
    fn split_join_confirmations_preserve_counts_and_reject_invalid_or_full_moves() {
        let mut z = Zone::new(7);
        put(&mut z, 0x40, 10);
        z.inventory.get_mut(&0x40).unwrap().b = 10;
        for count in [-1, 0, 10, 11] { z.split_inventory_item(inv::item_identity(0x40), count); }
        assert_eq!(z.inventory.len(), 1);
        z.split_inventory_item(inv::item_identity(0x40), 4);
        assert_eq!((z.inventory[&0x40].b, z.inventory[&0x41].b), (6, 4));
        z.join_inventory_items(inv::item_identity(0x40), inv::item_identity(0x41));
        assert_eq!(z.inventory[&0x40].b, 10);
        assert_eq!(z.inventory.len(), 1);
        for slot in inv::BAG_FIRST + 1..inv::BAG_FIRST + inv::BAG_SLOTS { put(&mut z, slot, 10); }
        z.split_inventory_item(inv::item_identity(0x40), 4);
        assert_eq!(z.inventory[&0x40].b, 10);
        z.inventory.get_mut(&0x40).unwrap().b = 50_000u16 as i16;
        z.join_inventory_items(inv::item_identity(0x40), inv::item_identity(0x41));
        assert!(z.inventory.contains_key(&0x41), "50001 must not overflow a stack");
    }

    fn put(z: &mut Zone, slot: u32, low: i32) {
        let id = inv::item_identity(slot);
        z.inventory.insert(slot, InventoryEntry { slot, a: 0x40, b: 1, id, item: AcgItem { low_id: low, high_id: low, level: 1 } });
    }

    #[test]
    fn reload_ack_updates_source_and_exact_weapon_before_feedback() {
        use ao_net::{n3::{dynel::KIND_WEAPON_ITEM, outgoing::n3_frame}, wire::Writer};
        let mut z = Zone::new(7);
        put(&mut z, 0x40, 100);
        let weapon = Identity { kind: KIND_WEAPON_ITEM, instance: 900 };
        z.world.arms.wield(7, 899, 8, None, &[(26, 0), (210, 700)]);
        z.world.arms.wield(7, 900, 6, None, &[(26, 0), (210, 250)]);
        let ack = |source: Identity, remaining: i32, clip: i32, remove: i32| {
            let mut w = Writer::default();
            w.u32(0x26515e61); Identity { kind: 50000, instance: 7 }.write(&mut w); w.u8(0);
            source.write(&mut w); weapon.write(&mut w);
            w.i32(remaining); w.i32(clip); w.i32(remove);
            n3_frame(1, 1, w.0)
        };
        assert!(z.world.reload_bar_progress().is_none());
        assert_eq!(z.on_frame(&ack(inv::item_identity(0x40), 123, 25, 0)), crate::play::zone::ZoneEvent::Reloading);
        assert_eq!(z.inventory[&0x40].b, 123);
        assert_eq!(z.world.arms.weapon_item(900).unwrap().clip, 25);
        assert_eq!(z.world.stat_of(KIND_WEAPON_ITEM, 900, 26), Some(25));
        // A generic authoritative stat delta must update that same retained Ammo state,
        // including crossing from negative to nonnegative (unlike the reload guard).
        {
            use ao_net::n3::{Message, N3, N3Header, dynel::{Dynel, StatUpdate}};
            for value in [-1, 25] {
                z.world.on_message(&Message {
                    header: N3Header { msg_type: ao_net::n3::dynel::STAT, target: weapon, flag: 0 }, sender: 1,
                    body: N3::Dynel(Dynel::Stat(StatUpdate { stats: vec![(26, value)], rest: vec![] })),
                });
                assert_eq!(z.world.stat_of(KIND_WEAPON_ITEM, 900, 26), Some(value));
            }
        }
        assert_eq!(z.world.arms.weapon_item(899).unwrap().clip, 0);
        assert_eq!(z.world.reload_bar_progress(), Some((0.0, 2.5)));
        z.on_frame(&ack(inv::item_identity(0x40), 0, 30, 2));
        assert!(!z.inventory.contains_key(&0x40));
        // Container lookup uses signed high-word registration, and count bounds are native u16.
        put(&mut z, 0x41, 101);
        let mut entry = z.inventory.remove(&0x41).unwrap(); entry.slot = 3;
        z.containers.insert((0xc750, 55), (0xfffe, vec![entry]));
        let source = inv::container_item_identity(-2, 3);
        z.on_frame(&ack(source, 65535, 5, 0));
        assert_eq!(z.containers[&(0xc750, 55)].1[0].b as u16, 65535);
        z.on_frame(&ack(source, 65536, 5, 0));
        assert_eq!(z.containers[&(0xc750, 55)].1[0].b, 0);
        // Source removal still happens when the resolved weapon's Ammo rejects the reload.
        z.world.arms.wield(7, 900, 6, None, &[(26, -1), (210, 900)]);
        assert_eq!(z.on_frame(&ack(source, 0, 9, 1)), crate::play::zone::ZoneEvent::None);
        assert!(z.containers[&(0xc750, 55)].1.is_empty());
        assert_eq!(z.world.arms.weapon_item(900).unwrap().clip, -1);
        assert_eq!(z.world.reload_bar_progress(), Some((0.0, 2.5)));
        put(&mut z, 0x42, 102);
        let dynel_source = Identity { kind: KIND_WEAPON_ITEM, instance: 12345 };
        z.inventory.get_mut(&0x42).unwrap().id = dynel_source;
        z.on_frame(&ack(dynel_source, 0, 9, 1));
        assert!(!z.inventory.contains_key(&0x42));
    }

    fn add(from: u32, to: i32) -> InventoryMsg {
        InventoryMsg::ContainerAdd { item: inv::item_identity(from), container: Identity { kind: 0xC350, instance: 7 }, slot: to }
    }

    #[test]
    fn confirmed_backpack_moves_use_first_free_slot_and_preserve_full_sources() {
        let mut z = Zone::new(7);
        let a = Identity { kind: 0xc749, instance: 101 };
        let b = Identity { kind: 0xc749, instance: 102 };
        z.containers.insert((a.kind, a.instance), (0x70, vec![]));
        z.containers.insert((b.kind, b.instance), (0x71, vec![]));
        z.containers.insert((0xdead, 7), (0, vec![]));
        put(&mut z, 0x40, 10);
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: inv::item_identity(0x40), container: a, slot: 19 });
        assert!(!z.inventory.contains_key(&0x40));
        assert_eq!(z.containers[&(a.kind, a.instance)].1[0].slot, 0);
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: inv::container_item_identity(0x70, 0), container: b, slot: 19 });
        assert!(z.containers[&(a.kind, a.instance)].1.is_empty());
        assert_eq!(z.containers[&(b.kind, b.instance)].1[0].id, inv::container_item_identity(0x71, 0));
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: inv::container_item_identity(0x71, 0), container: Identity { kind: 0xdead, instance: 7 }, slot: 99 });
        assert!(z.containers[&(b.kind, b.instance)].1.is_empty());
        assert_eq!(z.containers[&(0xdead, 7)].1[0].slot, 0);
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: Identity { kind: inv::KIND_BANK, instance: 0 }, container: z.own_identity(), slot: 19 });
        assert_eq!(z.inventory[&0x40].item.low_id, 10);
        let entry = z.inventory[&0x40];
        z.containers.get_mut(&(a.kind, a.instance)).unwrap().1 = (0..21).map(|slot| InventoryEntry { slot, ..entry }).collect();
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: inv::item_identity(0x40), container: a, slot: 0 });
        assert!(z.inventory.contains_key(&0x40), "full destination cannot discard the source");
    }

    #[test]
    fn server_delete_consumes_stack_or_removes_item_without_learning_a_nano() {
        let mut z = Zone::new(7);
        put(&mut z, 0x40, 100);
        z.inventory.get_mut(&0x40).unwrap().b = 2;
        put(&mut z, 0x11, 101);
        let delete = |who, slot, one| ao_net::n3::outgoing::n3_frame(0, who as u32,
            inv::delete_item(who, inv::item_identity(slot), Identity { kind: i32::from(one), instance: 0 }));
        z.on_frame(&delete(8, 0x40, true));
        assert_eq!(z.inventory[&0x40].b, 2, "another character's deletion is not ours");
        z.on_frame(&delete(7, 0x11, false));
        assert!(z.inventory.contains_key(&0x11), "worn items must be unequipped first");
        z.on_frame(&delete(7, 0x40, true));
        assert_eq!(z.inventory[&0x40].b, 1);
        z.on_frame(&delete(7, 0x40, true));
        assert!(!z.inventory.contains_key(&0x40));
        put(&mut z, 0x40, 100);
        z.inventory.get_mut(&0x40).unwrap().b = 3;
        z.on_frame(&delete(7, 0x40, false));
        assert!(!z.inventory.contains_key(&0x40), "zero extra kind deletes the entire stack");
        assert!(z.nanos.programs.is_empty(), "consumption alone must not invent a learned nano");
    }

    #[test]
    fn temporary_server_items_preserve_count_and_wait_for_bag_space() {
        let mut z = Zone::new(7);
        let item = AcgItem { low_id: 218395, high_id: 218395, level: 1 };
        let create = InventoryMsg::TemplateAction { item, count: 3, action: 0x57, identity_a: Identity { kind: inv::KIND_OVERFLOW, instance: 0 }, identity_b: Identity::default() };
        z.apply_inventory(&create);
        z.apply_inventory(&create);
        let key = (inv::KIND_OVERFLOW, 7);
        assert_eq!(z.containers[&key].1.iter().map(|e| (e.slot, e.b)).collect::<Vec<_>>(), [(0, 3), (1, 3)]);
        for slot in inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS { put(&mut z, slot, 100); }
        let transfer = InventoryMsg::ContainerAdd { item: Identity { kind: inv::KIND_OVERFLOW, instance: 0 }, container: Identity { kind: inv::KIND_OVERFLOW, instance: 7 }, slot: inv::ANY_BAG_SLOT };
        z.apply_inventory(&transfer);
        assert_eq!(z.containers[&key].1.len(), 2, "full bag must retain the server item");
        z.inventory.remove(&0x45);
        z.apply_inventory(&transfer);
        assert_eq!(z.inventory[&0x45], InventoryEntry { slot: 0x45, a: 0x21, b: 3, id: inv::item_identity(0x45), item });
        assert_eq!(z.containers[&key].1[0].slot, 1);
        z.apply_inventory(&transfer);
        assert_eq!(z.inventory.len(), inv::BAG_SLOTS as usize, "repeating a transfer cannot duplicate an item");
    }

    #[test]
    fn captured_cash_purchase_reconciles_item_before_trade_completion() {
        use ao_net::n3::{self, N3};
        let mut z = Zone::new(0x830e);
        z.stats.insert(61, 1005);
        for slot in 0x40..0x45 { put(&mut z, slot, 100); }
        let mut transferred = false;
        for line in include_str!("../../../../../docs/captures/zone_shop_cash_purchase_ithaca.rec").lines() {
            let mut parts = line.split(' ');
            let (_, dir, hex) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
            if dir != "<" { continue; }
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
            let frame = ao_net::frame::Frame::decode_with(&bytes, false).unwrap().unwrap().0;
            let message = n3::decode(&frame).unwrap();
            z.on_frame(&frame);
            match message.body {
                N3::Inventory(InventoryMsg::TemplateAction { action: 0x57, .. }) => {
                    assert_eq!(z.inventory.len(), 5, "template creation alone does not place a bag item");
                    assert_eq!(z.containers[&(inv::KIND_OVERFLOW, 0x830e)].1.len(), 1);
                }
                N3::Inventory(InventoryMsg::ContainerAdd { item, .. }) if item.kind == inv::KIND_OVERFLOW => {
                    assert_eq!(z.inventory[&0x45].item, AcgItem { low_id: 218395, high_id: 218395, level: 1 });
                    assert_eq!(z.inventory[&0x45].b, 1);
                    assert!(z.containers[&(inv::KIND_OVERFLOW, 0x830e)].1.is_empty());
                    transferred = true;
                }
                N3::Trade(t) if t.op == n3::trade::COMPLETE => {
                    assert!(transferred, "actual server item transfer precedes completion");
                    assert_eq!(z.inventory.len(), 6);
                }
                _ => {}
            }
        }
        assert!(transferred);
        assert_eq!(z.stat(61), Some(743));
    }

    #[test]
    fn bank_confirmation_moves_to_first_free_slots() {
        let mut z = Zone::new(7);
        put(&mut z, 0x40, 100);
        z.apply_inventory(&InventoryMsg::Bank(vec![]));
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: inv::item_identity(0x40), container: Identity { kind: 0xdead, instance: 7 }, slot: 99 });
        assert!(z.inventory.is_empty());
        assert_eq!(z.containers[&(0xdead, 7)].1[0].slot, 0);
        z.apply_inventory(&InventoryMsg::ContainerAdd { item: Identity { kind: inv::KIND_BANK, instance: 0 }, container: Identity { kind: 0xc350, instance: 7 }, slot: 99 });
        assert_eq!(z.inventory[&0x40].item.low_id, 100);
        assert!(z.containers[&(0xdead, 7)].1.is_empty());
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

    /// Own-account kill (`docs/captures/zone_loot_take_ithaca.rec`, own id 0x830e): `InventoryUpdateIIR_t` (corpse `{0xC76A, 0xfd8}`, word 0x70, one
    /// item) -> our take `{0x6b, 0x00700000}` -> the server's `ContainerAddItemIIR_t` (container = own, slot 0x6f): the item lands in bag slot 0x40
    /// and leaves the corpse list.
    #[test]
    fn captured_corpse_take_moves_the_item_into_the_bag() {
        let mut z = Zone::new(0x830e);
        let mut seen_update = false;
        for l in include_str!("../../../../../docs/captures/zone_loot_take_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir != "<" {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            let f = ao_net::frame::Frame::decode_with(&b, false).unwrap().unwrap().0;
            z.on_frame(&f);
            if !seen_update && !z.containers.is_empty() {
                seen_update = true;
                let (word, list) = &z.containers[&(0xc76a, 0xfd8)];
                assert_eq!((*word, list.len(), list[0].slot, list[0].item.low_id), (0x70, 1, 0, 248_323));
                assert!(z.inventory.is_empty());
            }
        }
        assert!(seen_update);
        assert_eq!(z.inventory.len(), 1);
        assert_eq!((z.inventory[&0x40].slot, z.inventory[&0x40].item.low_id), (0x40, 248_323));
        assert!(z.containers[&(0xc76a, 0xfd8)].1.is_empty());
    }

    #[test]
    fn corpse_take_with_a_full_bag_or_unknown_item_changes_nothing() {
        let mut z = Zone::new(7);
        let corpse = Identity { kind: 0xc76a, instance: 1 };
        let entry = InventoryEntry { slot: 3, a: 0, b: 0, id: Identity::default(), item: AcgItem { low_id: 5, high_id: 5, level: 1 } };
        z.apply_inventory(&InventoryMsg::Update(inv::InventoryUpdate { capacity: 21, access: 2, entries: vec![entry], container: corpse, word: 0x70, flag: true }));
        let own = Identity { kind: 0xC350, instance: 7 };
        let take = |slot: u32, key: i32| InventoryMsg::ContainerAdd { item: inv::container_item_identity(key, slot), container: own, slot: inv::ANY_BAG_SLOT };
        z.apply_inventory(&take(4, 0x70));
        z.apply_inventory(&take(3, 0x71));
        assert!(z.inventory.is_empty() && z.containers[&(0xc76a, 1)].1.len() == 1);
        for s in inv::BAG_FIRST..inv::BAG_FIRST + inv::BAG_SLOTS {
            put(&mut z, s, s as i32);
        }
        let before = z.inventory.clone();
        z.apply_inventory(&take(3, 0x70));
        assert_eq!(z.inventory, before);
        z.inventory.remove(&0x50);
        z.apply_inventory(&take(3, 0x70));
        assert_eq!(z.inventory[&0x50].item.low_id, 5);
    }
}
