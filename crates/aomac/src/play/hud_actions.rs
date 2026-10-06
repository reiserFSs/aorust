//! Owner of the own character's special-action list in the HUD: derives it from the equipment and the own state each frame
//! ([`SpecialList`]), docs/gui.md §10.5.

use super::hud_special::{OwnState, SpecialList, EQUIP_SLOTS};
use super::zone::Zone;
use ao_formats::dynel_visual::item_template;
use std::collections::HashMap;

/// Item stats read by `FUN_1009e301`: the `Can` flags (0x1e) and the reload stat (0x1a, UNRESOLVED GUESS: an equipped item that carries it with
/// a value other than -1 offers Reload; the DLL tests `GetStat(0x1a) != -1`, whose default for an item without the stat is not known).
const STAT_CAN: u32 = 0x1e;
const STAT_RELOAD: u32 = 0x1a;
/// `Flags` (stat 0) of the own character, the word the Backstab test reads.
const STAT_FLAGS: u32 = 0;

pub(super) struct HudActions {
    pub(super) list: SpecialList,
    store: Option<ao_rdb::RecordStore>,
    /// The equipped item ids / flags the list was last derived from.
    equipped: Vec<(u32, i32)>,
    char_flags: i32,
    menu_metadata: HashMap<u32, Option<(i32, String)>>,
}

impl HudActions {
    pub(super) fn new(dir: &std::path::Path) -> Self {
        HudActions { list: SpecialList::new(), store: ao_rdb::RecordStore::open(dir).ok(), equipped: vec![], char_flags: 0, menu_metadata: HashMap::new() }
    }

    /// ActionMenu reads the provider item's category (stat 588) and raw template name (GUI `FUN_10070c04`).
    pub(super) fn menu_items(&mut self) -> Vec<(u32, i32, String, u32)> {
        let mut items = Vec::new();
        for entry in self.list.entries() {
            let Some(instance) = entry.template() else { continue };
            let metadata = self.menu_metadata.entry(instance).or_insert_with(|| {
                let template = item_template(self.store.as_ref()?, instance).ok()??;
                Some((template.stat(588).unwrap_or(0), template.name.unwrap_or_default()))
            });
            if let Some((category, name)) = metadata {
                items.push((instance, *category, name.clone(), entry.shown));
            }
        }
        items
    }

    /// Per frame: the weapon entries follow the equipment (`FUN_1009e301` on every item put into a slot below 0x30), the recharge records count down.
    pub(super) fn update(&mut self, zone: &mut Zone, dt: f32) {
        for (action, template) in std::mem::take(&mut zone.special_registration_feed) {
            self.list.register(action, template);
        }
        let mut eq: Vec<(u32, i32)> = zone.inventory.values().filter(|e| e.slot < EQUIP_SLOTS).map(|e| (e.slot, e.item.low_id)).collect();
        eq.sort_unstable();
        let flags = zone.stat(STAT_FLAGS).unwrap_or(0);
        if eq != self.equipped || flags != self.char_flags {
            if let Some(store) = &self.store {
                let items: Vec<(i32, i32)> = eq
                    .iter()
                    .filter_map(|&(_, low)| item_template(store, low as u32).ok().flatten())
                    .map(|t| (t.stat(STAT_CAN).unwrap_or(0), t.stat(STAT_RELOAD).unwrap_or(-1)))
                    .collect();
                self.list.sync_equipment(&items, flags);
            }
            self.equipped = eq;
            self.char_flags = flags;
        }
        self.list.tick(dt);
    }

    pub(super) fn set_state(&mut self, s: OwnState) {
        self.list.apply_state(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_perk_feed_reaches_menu_and_identity_lookup() {
        let mut actions = HudActions::new(std::path::Path::new("/nonexistent-aomac-client"));
        // Synthetic registration: the wire supplies this pairing, not the perk catalogue.
        actions.menu_metadata.insert(213010, Some((1, "Registered perk".into())));
        let mut zone = Zone::default();
        zone.special_registration_feed.push((10111, 213010));
        actions.update(&mut zone, 0.0);
        assert_eq!(actions.list.find(213010).unwrap().shown, 10111);
        assert!(actions.menu_items().contains(&(213010, 1, "Registered perk".into(), 10111)));
        assert!(zone.special_registration_feed.is_empty());
    }

    #[test]
    fn menu_uses_registered_actions_and_cached_template_metadata() {
        let mut actions = HudActions::new(std::path::Path::new("/nonexistent-aomac-client"));
        actions.menu_metadata.insert(0xc1a3, Some((2, "Use".into())));
        actions.menu_metadata.insert(0xc1aa, Some((2, "Pick-Up".into())));
        assert_eq!(actions.menu_items(), [(0xc1a3, 2, "Use".into(), 3), (0xc1aa, 2, "Pick-Up".into(), 1)]);
        assert!(!actions.menu_items().iter().any(|item| item.0 == 82216));
    }
}
