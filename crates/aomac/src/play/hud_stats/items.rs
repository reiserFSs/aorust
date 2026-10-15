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
    /// The template's stats (rdb 1000020): `Can` (30), `ItemClass` (76), `DefaultPos` (88), `Placement` (298) decide how an item may be worn.
    pub stats: Vec<(u32, i32)>,
}

impl Info {
    pub fn stat(&self, id: u32) -> Option<i32> {
        self.stats.iter().find(|s| s.0 == id).map(|s| s.1)
    }
}

/// A shop item's name, count, Value and icon.
pub type ShopInfo = (String, i32, i32, Option<GfxId>);

struct ShopItem {
    template: ao_formats::dynel_visual::ItemTemplate,
    icon: Option<Option<GfxId>>,
}

pub struct Items {
    store: Option<RecordStore>,
    cache: HashMap<i32, Option<Info>>,
    shop_cache: HashMap<(i32, i32, i32), Option<ShopItem>>,
}

impl Items {
    pub fn new(dir: &Path) -> Self {
        Self { store: RecordStore::open(dir).ok(), cache: HashMap::new(), shop_cache: HashMap::new() }
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
                Some(Info { name: t.name.clone().unwrap_or_default(), icon, stats: t.stats.clone() })
            });
            self.cache.insert(low_id, info);
        }
        self.cache[&low_id].as_ref()
    }

    fn cache_acg(&mut self, item: ao_net::n3::world::AcgItem) -> anyhow::Result<()> {
        let key = (item.low_id, item.high_id, item.level);
        if self.shop_cache.contains_key(&key) { return Ok(()); }
        let template = (|| -> anyhow::Result<_> {
            let Some(store) = self.store.as_ref() else { return Ok(None) };
            let Some(low) = item_template(store, u32::try_from(item.low_id)?)? else { return Ok(None) };
            let high = if item.high_id == item.low_id { None } else { item_template(store, u32::try_from(item.high_id)?)? };
            if item.high_id != item.low_id && high.is_none() { return Ok(None); }
            let template = ao_formats::dynel_visual::interpolate_item_templates(&low, high.as_ref().unwrap_or(&low), item.level)?;
            Ok(Some(ShopItem { template, icon: None }))
        })()?;
        self.shop_cache.insert(key, template);
        Ok(())
    }

    pub(super) fn acg_stat(&mut self, item: ao_net::n3::world::AcgItem, stat: u32) -> anyhow::Result<Option<i32>> {
        self.cache_acg(item)?;
        Ok(self.shop_cache.get(&(item.low_id, item.high_id, item.level)).and_then(|v| v.as_ref()).and_then(|v| v.template.stat(stat)))
    }

    /// ACG-resolved shop name/count/Value, sharing the owned-item stat cache.
    pub fn shop_info(&mut self, gui: &mut Gui, item: ao_net::n3::world::AcgItem) -> Option<ShopInfo> {
        let key = (item.low_id, item.high_id, item.level);
        if let Err(error) = self.cache_acg(item) {
            eprintln!("shop: invalid ACG {item:?}: {error:#}");
            self.shop_cache.insert(key, None);
            return None;
        }
        let cached = self.shop_cache.get_mut(&key)?.as_mut()?;
        let t = &cached.template;
        let icon = *cached.icon.get_or_insert_with(|| {
            t.stat(STAT_ICON).filter(|&id| id > 0).and_then(|id| icon_image(gui, self.store.as_ref()?, id as u32)).map(|(id, _, _)| id)
        });
        Some((t.name.clone().unwrap_or_default(), t.stat(STAT_COUNT).filter(|&c| c > 0).unwrap_or(1), t.stat(0x4a).unwrap_or(0), icon))
    }

    /// The record of an item that [`Items::info`] already loaded.
    pub fn cached(&self, low_id: i32) -> Option<&Info> {
        self.cache.get(&low_id)?.as_ref()
    }

    #[cfg(test)]
    pub(super) fn insert_test_info(&mut self, id: i32, info: Info) {
        self.cache.insert(id, Some(info));
    }

    #[cfg(test)]
    pub(super) fn insert_test_acg(&mut self, item: ao_net::n3::world::AcgItem, stats: &[(u32, i32)]) {
        self.shop_cache.insert((item.low_id, item.high_id, item.level), Some(ShopItem {
            template: ao_formats::dynel_visual::ItemTemplate { kind: 0, name: None, sounds: vec![], stats: stats.to_vec() },
            icon: Some(None),
        }));
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
    fn acg_stat_cache_keeps_quality_and_high_endpoint_separate() {
        let mut items = Items { store: None, cache: HashMap::new(), shop_cache: HashMap::new() };
        let low = ao_net::n3::world::AcgItem { low_id: 1, high_id: 2, level: 4 };
        let high = ao_net::n3::world::AcgItem { level: 5, ..low };
        items.insert_test_info(1, Info { name: String::new(), icon: None, stats: vec![(30, 0)] });
        items.insert_test_acg(low, &[(30, 0x200), (0, 0)]);
        items.insert_test_acg(high, &[(30, 0x1000200), (0, 0x4000000)]);
        assert_eq!(items.acg_stat(low, 30).unwrap(), Some(0x200));
        assert_eq!(items.acg_stat(high, 30).unwrap(), Some(0x1000200));
        assert_eq!(items.acg_stat(high, 0).unwrap(), Some(0x4000000));
        assert_eq!(items.acg_stat(ao_net::n3::world::AcgItem { high_id: 3, ..high }, 30).unwrap(), None);
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
