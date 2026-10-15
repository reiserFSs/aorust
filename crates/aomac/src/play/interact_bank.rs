//! Bank inventory uses InventoryView_c type 1 (GUI 0x100cc2ca), 102 cells
//! (0x100cdb3a). BankIIR_t activation (GC 0x10072002) opens DEAD:own;
//! ContainerAddItemIIR_t confirmations move items (GC 0x100492fa/0x10046a3d).
use super::{interact::Interact, Play};
use ao_gui::Gui;
use ao_net::n3::inventory;

impl Interact {
    /// Local `/bank close` (GC 0x10046bdd): closing does not send a command.
    pub fn bank_close(&mut self, gui: &mut Gui) {
        self.use_ui.loot.close_bank(gui);
    }
}

impl Play {
    pub(super) fn interact_bank_frame(&mut self) -> Vec<ao_net::frame::Frame> {
        let Some(i) = self.interact.as_mut() else { return vec![] };
        i.use_ui.loot.refresh_containers(&mut self.gui, &self.zone);
        let Some(h) = self.hud.as_mut() else { return vec![] };
        let mut rest = vec![];
        for (slot, x, y) in h.take_item_drops() {
            if let Some(bank) = i.use_ui.loot.container_drop(&self.gui, x, y) {
                if let Some(error) = self.zone.container_move_error(inventory::item_identity(slot), bank) {
                    i.use_ui.container_feedback.push(error);
                    continue;
                }
                i.send(inventory::container_add_item(self.zone.char_id as i32, bank, inventory::item_identity(slot)));
            } else {
                rest.push((slot, x, y));
            }
        }
        for (item, x, y) in i.use_ui.loot.take_drops() {
            if i.tradeskill.drop_identity(&self.gui, &self.zone, item, x, y) { continue; }
            if h.container_bag_drop(&self.gui, x, y) {
                i.move_to_bag(&mut self.zone, item);
            } else if item.kind != inventory::KIND_BANK {
                if let Some(container) = i.use_ui.loot.container_drop(&self.gui, x, y) {
                    if let Some(error) = self.zone.container_move_error(item, container) {
                        i.use_ui.container_feedback.push(error);
                        continue;
                    }
                    i.send(inventory::container_add_item(self.zone.char_id as i32, container, item));
                }
            }
        }
        h.requeue_item_drops(rest);
        i.take_outbox()
    }
}
