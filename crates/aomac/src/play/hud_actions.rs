//! Owner of the own character's special-action list in the HUD: derives it from the equipment and the own state each frame
//! ([`SpecialList`]), docs/gui.md §10.5.

use super::hud_special::{OwnState, SpecialList, EQUIP_SLOTS};
use super::zone::Zone;
use ao_formats::dynel_visual::item_template;

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
}

impl HudActions {
    pub(super) fn new(dir: &std::path::Path) -> Self {
        HudActions { list: SpecialList::new(), store: ao_rdb::RecordStore::open(dir).ok(), equipped: vec![], char_flags: 0 }
    }

    /// Per frame: the weapon entries follow the equipment (`FUN_1009e301` on every item put into a slot below 0x30), the recharge records count down.
    pub(super) fn update(&mut self, zone: &Zone, dt: f32) {
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
