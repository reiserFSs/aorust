//! Item contents of the wear and inventory windows: names and icons of the own `FullCharacterIIR_t` inventory (docs/gui.md §11.9 / §11.11).
//!
//! An item (`ACGItem_t{low_id, high_id, level}`) is the rdb 1000020 record `low_id`; its name is `N3Msg_GetName`, its picture the stat `Icon` (0x4f) →
//! the PNG of rdb 1010008 (`FUN_1003eb30`, the same path as the hotbar icons, [`super::super::hud_bar::icon_image`]).

use super::super::hud_bar::icon_image;
use ao_formats::dynel_visual::item_template;
use ao_gui::{CanvasItem, GfxId, Gui};
use ao_net::n3::world::InventoryEntry;
use ao_rdb::RecordStore;
use std::collections::HashMap;
use std::path::Path;
use crate::play::{chat::info::{item_eligibility::EquipCriteria, PerkEquipment}, zone::Zone};
use ao_net::n3::world::AcgItem;

/// Stat `Icon`.
const STAT_ICON: u32 = 0x4f;
/// Stat `MultipleCount` (212): template count for shop stock; owned inventory count is `InventoryEntry::b` (GC 0x1002a7d2).
const STAT_COUNT: u32 = 212;

/// Size of an item picture (`MultiListView_c` grid icon, 48 px) and of its slot art `GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED`.
pub const ICON: f32 = 48.0;
pub const SLOT: f32 = 54.0;

pub struct Info {
    pub name: String,
    pub icon: Option<(GfxId, u32, u32)>,
}


/// A shop item's name, count, Value and icon.
pub type ShopInfo = (String, i32, i32, Option<GfxId>);
type WearDestination = Option<(usize, u32)>;


pub struct Items {
    store: Option<RecordStore>,
    cache: HashMap<i32, Option<Info>>,
    shop_cache: HashMap<(i32, i32, i32), Option<ShopInfo>>,
    equip: HashMap<(i32, i32, i32), Result<EquipCriteria, String>>,
    destinations: HashMap<(i32, i32, i32), Result<WearDestination, String>>,
    equipment: PerkEquipment,
    pub(super) nano_cast_busy: bool,
}

impl Items {
    pub fn new(dir: &Path) -> Self {
        Self { store: RecordStore::open(dir).ok(), cache: HashMap::new(), shop_cache: HashMap::new(), equip: HashMap::new(), destinations: HashMap::new(), equipment: PerkEquipment::default(), nano_cast_busy: false }
    }

    fn equip_criteria(&mut self, item: AcgItem) -> anyhow::Result<&EquipCriteria> {
        let key = (item.low_id, item.high_id, item.level);
        if !self.equip.contains_key(&key) {
            let result = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("item database unavailable"))
                .and_then(|store| EquipCriteria::load(store, item)).map_err(|e| format!("{e:#}"));
            self.equip.insert(key, result);
        }
        self.equip[&key].as_ref().map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub(super) fn acg_stat(&mut self, item: AcgItem, stat: u32) -> anyhow::Result<Option<i32>> {
        Ok(self.equip_criteria(item)?.stat(stat))
    }

    #[cfg(test)]
    pub(super) fn insert_test_acg(&mut self, item: AcgItem, stats: &[(u32, i32)]) {
        self.equip.insert((item.low_id, item.high_id, item.level), Ok(EquipCriteria::test_stats(stats)));
    }

    pub(super) fn wear_destination(&mut self, item: AcgItem) -> anyhow::Result<Option<(usize, u32)>> {
        let key = (item.low_id, item.high_id, item.level);
        if !self.destinations.contains_key(&key) {
            let result = (|| -> anyhow::Result<_> {
                let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("item database unavailable"))?;
                let low = item_template(store, u32::try_from(item.low_id)?)?.ok_or_else(|| anyhow::anyhow!("missing item {}", item.low_id))?;
                let high_id = if item.high_id == 0 { item.low_id } else { item.high_id };
                let high = if high_id == item.low_id { None } else { Some(item_template(store, u32::try_from(high_id)?)?.ok_or_else(|| anyhow::anyhow!("missing item {high_id}"))?) };
                let template = ao_formats::dynel_visual::interpolate_item_templates(&low, high.as_ref().unwrap_or(&low), item.level)?;
                Ok(super::item_dnd::default_wear_stats(&template.stats))
            })().map_err(|e| format!("{e:#}"));
            self.destinations.insert(key, result);
        }
        self.destinations[&key].as_ref().copied().map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub(super) fn can_unwear(&mut self, zone: &Zone, slot: u32) -> anyhow::Result<bool> {
        let item = zone.inventory.get(&slot).ok_or_else(|| anyhow::anyhow!("missing worn slot {slot}"))?.item;
        if slot == 7 && (9..=14).any(|s| zone.inventory.contains_key(&s)) { return Ok(false); }
        self.equip_criteria(item)?;
        let check = self.equip[&(item.low_id, item.high_id, item.level)].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        check.removable(zone, (0x30..0x40).contains(&slot), check.completion_state(zone, true)?)
    }

    pub(super) fn wear_delay(&mut self, zone: &Zone, source: u32, destination: i32) -> anyhow::Result<Option<f32>> {
        if source >= 0x30 && !(0..0x30).contains(&destination) { return Ok(None); }
        let moved = zone.inventory.get(&source).ok_or_else(|| anyhow::anyhow!("missing source slot {source}"))?.item;
        let delay = self.acg_stat(moved, super::item_dnd::STAT_EQUIP_DELAY)?.unwrap_or(0);
        let occupant = if (0..0x30).contains(&destination) {
            if let Some(entry) = zone.inventory.get(&(destination as u32)) {
                self.acg_stat(entry.item, super::item_dnd::STAT_EQUIP_DELAY)?.unwrap_or(0)
            } else { 0 }
        } else { 0 };
        Ok(super::item_dnd::wear_delay_values(delay, source as i32, destination, occupant))
    }

    pub(super) fn can_equip(&mut self, zone: &Zone, source: u32, tab: usize, id: u32) -> anyhow::Result<bool> {
        let entry = zone.inventory.get(&source).ok_or_else(|| anyhow::anyhow!("missing source slot {source}"))?;
        if entry.b > 1 { return Ok(false); }
        // 1004c570: the temporary/overflow inventory (10048829 kind0x6e)
        // must have its first cell free before an equip.
        if zone.containers.get(&(ao_net::n3::inventory::KIND_TRADE, zone.char_id as i32)).is_some_and(|(_, entries)| entries.iter().any(|e| e.slot == 0)) { return Ok(false); }
        let item = entry.item;
        let destination = tab as u32 * 16 + id;
        // 1004c534: already worn items cannot move into/out of Social.
        if source < 0x40 && (tab == 3 || source >= 0x30) { return Ok(false); }
        self.equip_criteria(item)?;
        let key = (item.low_id, item.high_id, item.level);
        let class = self.equip[&key].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?.stat(76);
        if matches!(class, Some(3 | 5)) {
            if zone.inventory.contains_key(&destination) { return Ok(false); }
            if (class == Some(5)) != (zone.skill_value(60) == Some(15)) { return Ok(false); }
        }
        if class != Some(1) && zone.stat(224).unwrap_or(0) & 0x800 != 0 { return Ok(false); }
        if class == Some(1) && destination == 7 && (9..=14).any(|s| zone.inventory.contains_key(&s)) { return Ok(false); }
        // Replacement checks the displaced item's own Unwear/Unwield stream.
        if destination != source {
            if let Some(occupant) = zone.inventory.get(&destination) {
                if !self.can_unwear(zone, destination)? { return Ok(false); }
                if source < 0x40 {
                    let check = self.equip_criteria(occupant.item)?;
                    if check.stat(76) == Some(1) && !super::item_dnd::can_wear_stats(check.stats(), (source / 16) as usize, source % 16, zone.stat(0x2d).unwrap_or(0)) { return Ok(false); }
                }
            }
        }
        // 1004c6e1 passes the opposite *occupied* slot to 10068e28.
        if class == Some(1) {
            let opposite = match destination { 6 => Some(8), 8 => Some(6), 13 => Some(15), 15 => Some(13), _ => None };
            if let Some(opposite) = opposite.filter(|&slot| slot != source).and_then(|slot| zone.inventory.get(&slot)) {
                self.equip_criteria(opposite.item)?;
                let check = self.equip[&key].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
                let other = self.equip[&(opposite.item.low_id, opposite.item.high_id, opposite.item.level)].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
                if !check.dual_wield(other, zone)? { return Ok(false); }
            }
        }
        if !matches!(tab, 0 | 3) {
            let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("item database unavailable"))?;
            self.equipment.refresh(store, zone)?;
        }
        let check = self.equip[&key].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        if !super::item_dnd::can_wear_stats(check.stats(), tab, id, zone.stat(0x2d).unwrap_or(0)) { return Ok(false); }
        check.eligible(zone, self.equipment.armor(), tab == 3, check.completion_state(zone, false)?)
    }

    /// Interfaces 1000a707: tower type or use-event criterion action 0x53.
    pub fn is_tower_item(&self, low_id: i32) -> anyhow::Result<bool> {
        let Some(store) = self.store.as_ref() else { return Ok(false) };
        let Some(template) = item_template(store, u32::try_from(low_id)?)? else { return Ok(false) };
        if template.stat(0x184).is_some_and(|value| value != 0 && value != 0x499602d2) { return Ok(true); }
        let Some(record) = store.get(ao_formats::dynel_visual::ITEM_TEMPLATE_TYPE, low_id as u32)? else { return Ok(false) };
        crate::play::chat::item_is_shield_disabler(&record)
    }

    /// The record of an item (`None` without rdb or record). Icon textures are created once per item.
    pub fn info(&mut self, gui: &mut Gui, low_id: i32) -> Option<&Info> {
        if !self.cache.contains_key(&low_id) {
            let info = self.store.as_ref().and_then(|s| {
                let t = item_template(s, u32::try_from(low_id).ok()?).ok()??;
                let icon = t.stat(STAT_ICON).filter(|&i| i > 0).and_then(|i| icon_image(gui, s, i as u32));
                Some(Info { name: t.name.unwrap_or_default(), icon })
            });
            self.cache.insert(low_id, info);
        }
        self.cache[&low_id].as_ref()
    }

    /// ACG-resolved shop name/count/Value and icon. Owned item predicates use
    /// `acg_stat`'s canonical EquipCriteria cache, not a second shop template.
    pub fn shop_info(&mut self, gui: &mut Gui, item: ao_net::n3::world::AcgItem) -> Option<ShopInfo> {
        let key = (item.low_id, item.high_id, item.level);
        if !self.shop_cache.contains_key(&key) {
            let result = (|| -> anyhow::Result<Option<ShopInfo>> {
                let Some(store) = self.store.as_ref() else { return Ok(None) };
                let Some(low) = item_template(store, u32::try_from(item.low_id)?)? else { return Ok(None) };
                let high = if item.high_id == item.low_id { None } else {
                    let Some(high) = item_template(store, u32::try_from(item.high_id)?)? else { return Ok(None) };
                    Some(high)
                };
                let t = ao_formats::dynel_visual::interpolate_item_templates(&low, high.as_ref().unwrap_or(&low), item.level)?;
                let icon = t.stat(STAT_ICON).filter(|&id| id > 0).and_then(|id| icon_image(gui, store, id as u32)).map(|(id, _, _)| id);
                Ok(Some((t.name.clone().unwrap_or_default(), t.stat(STAT_COUNT).filter(|&c| c > 0).unwrap_or(1), t.stat(0x4a).unwrap_or(0), icon)))
            })();
            let info = match result {
                Ok(info) => info,
                Err(e) => { eprintln!("shop: invalid ACG {item:?}: {e:#}"); None }
            };
            self.shop_cache.insert(key, info);
        }
        self.shop_cache[&key].clone()
    }


    #[cfg(test)]
    pub(super) fn insert_test_info(&mut self, id: i32, info: Info) {
        self.cache.insert(id, Some(info));
    }


    /// Canvas items of one item picture whose slot cell has its top-left corner at `(x, y)` (the 48 px picture sits 3 px inside the 54 px slot).
    pub fn picture(&mut self, gui: &mut Gui, e: &InventoryEntry, x: f32, y: f32) -> Option<CanvasItem> {
        let (id, w, h) = self.info(gui, e.item.low_id)?.icon?;
        let o = (SLOT - ICON) / 2.0;
        Some(CanvasItem::Image { id, src: [0.0, 0.0, w as f32, h as f32], dst: [x + o, y + o, x + o + ICON, y + o + ICON], alpha: 1.0 })
    }
}

/// Order of the weapon grid's cells (docs/gui.md §11.9): row-major `1, 15, 2, 3 .. 14` (Hud 1, Hud 2, Hud 3 = slot 15, Utils 1-3, Right Hand 6, Deck 7,
/// Left Hand 8, Deck 1-6).
pub const WEAPON_IDS: [u32; 15] = [1, 15, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];

/// Tabs of the wear window: Weapons, Clothes, Implants, Social. The inventory slot of cell `i` (row-major, 3 columns): weapons `id`, clothes
/// `0x10 + id`, implants `0x20 + id` (13 cells), social `0x30 + id` (`FUN_10046e35`, `FUN_10047873`).
pub fn wear_slot(tab: usize, cell: usize) -> Option<u32> {
    let id = cell as u32 + 1;
    match tab {
        0 => WEAPON_IDS.get(cell).copied(),
        1 if cell < 15 => Some(0x10 + id),
        2 if cell < 13 => Some(0x20 + id),
        3 if cell < 15 => Some(0x30 + id),
        _ => None,
    }
}

/// Grid position (column, row) of a cell: 3 columns; the 13th implant ("Feet") stands alone in the middle column of row 4 (art `GFX_GUI_WEARVIEW_IMPLANTS`).
pub fn wear_cell(tab: usize, cell: usize) -> (usize, usize) {
    if tab == 2 && cell == 12 { (1, 4) } else { (cell % 3, cell / 3) }
}

/// Top-left corner of a cell's 54 px slot in the 192 x 320 wear art: the labelled frames of `GFX_GUI_WEARVIEW_*` measured from the PNG are 52 px wide, 52 px high,
/// columns at x = 9, 70, 131 (pitch 61), rows at y = 22 + 58 r; the slot art has a 1 px margin around its frame.
pub fn wear_origin(col: usize, row: usize) -> (f32, f32) {
    (8.0 + 61.0 * col as f32, 21.0 + 58.0 * row as f32)
}

/// Bag slots of the own inventory: `0x40 + i`, `SetMaxItemCount(0x1e)`.
pub const BAG_FIRST: u32 = 0x40;
pub const BAG_SLOTS: u32 = 30;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_item_stats_are_cached_by_both_endpoints_and_quality() {
        let mut items = Items::new(Path::new("/no-item-database"));
        let low = AcgItem { low_id: 1, high_id: 2, level: 1 };
        let quality = AcgItem { level: 2, ..low };
        let endpoint = AcgItem { high_id: 3, ..low };
        items.insert_test_info(1, Info { name: String::new(), icon: None });
        items.insert_test_acg(low, &[(30, 0x200), (0, 0)]);
        items.insert_test_acg(quality, &[(30, 0x1000200), (0, 0x4000000)]);
        items.insert_test_acg(endpoint, &[(30, 8)]);
        assert_eq!(items.acg_stat(low, 30).unwrap(), Some(0x200));
        assert_eq!(items.acg_stat(quality, 30).unwrap(), Some(0x1000200));
        assert_eq!(items.acg_stat(quality, 0).unwrap(), Some(0x4000000));
        assert_eq!(items.acg_stat(endpoint, 30).unwrap(), Some(8));
        assert!(items.acg_stat(AcgItem { high_id: 4, ..low }, 30).is_err(), "missing ACG never falls back to the low-info cache");
    }

    #[test]
    fn wear_slots_follow_the_cell_tables() {
        // weapons: Right Hand / Deck / Left Hand are row 2, slots 6 / 7 / 8; Hud 2 = slot 15
        assert_eq!((0..15).map(|c| wear_slot(0, c).unwrap()).collect::<Vec<_>>(), [1, 15, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
        assert_eq!((wear_slot(1, 0), wear_slot(1, 14), wear_slot(2, 12), wear_slot(2, 13), wear_slot(3, 0), wear_slot(3, 14)), (Some(0x11), Some(0x1f), Some(0x2d), None, Some(0x31), Some(0x3f)));
        assert_eq!((wear_cell(2, 12), wear_cell(2, 11), wear_cell(1, 14)), ((1, 4), (2, 3), (2, 4)));
        // every slot is unique over the four tabs and below the bag
        let mut all: Vec<u32> = (0..4).flat_map(|t| (0..15).filter_map(move |c| wear_slot(t, c))).collect();
        all.sort();
        all.dedup();
        assert_eq!((all.len(), *all.last().unwrap() < BAG_FIRST), (58, true));
    }
}
