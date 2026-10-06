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
    pub(super) fn interact_bank_frame(&mut self) {
        let Some(i) = self.interact.as_mut() else { return };
        if let Some((_, entries)) = self.zone.containers.get(&(0xdead, self.zone.char_id as i32)) {
            i.use_ui.loot.refresh_bank(&mut self.gui, entries);
        }
        let Some(h) = self.hud.as_mut() else { return };
        let mut rest = vec![];
        for (slot, x, y) in h.take_item_drops() {
            if let Some(bank) = i.use_ui.loot.bank_drop(&self.gui, x, y) {
                i.send(inventory::container_add_item(self.zone.char_id as i32, bank, inventory::item_identity(slot)));
            } else {
                rest.push((slot, x, y));
            }
        }
        for f in i.take_outbox() {
            if let Some(s) = &self.session { s.send_zone(f); }
        }
        h.requeue_item_drops(rest);
    }
}
