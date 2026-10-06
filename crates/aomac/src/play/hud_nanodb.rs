//! The nano program records the Programs / NCU windows list: rdb 1040005 (`0xFDE85`, `StaticInstance` = the nano id of `Identity{0xCF1B, id}`). A nano record
//! has the same layout as an item template (`dynel_visual::parse_item_template`: kind 0xC76B, the stat list, the name), so its columns are stats read the way
//! `FUN_1003d50c` [GUI 0x1003d50c] does (`N3Msg_GetSkill(nano, stat, 2)`): docs/gui.md §11.12.

use super::hud_bar::icon_image;
use ao_formats::dynel_visual::parse_item_template;
use ao_gui::{GfxId, Gui};
use ao_rdb::RecordStore;
use std::collections::HashMap;
use std::path::Path;

/// rdb record type of the nano programs.
pub const NANO_RDB_TYPE: u32 = 1_040_005;

/// Stat ids the windows read (the stat table: `ao_formats::stats`).
pub mod stat {
    /// `Flags`: bit 0x8000 / 0xc100 classify the program (`FUN_1003eacb`, `N3Msg_RemoveBuff`).
    pub const FLAGS: u32 = 0;
    /// `TimeExist` (8): the effect's duration in 1/100 s (`FUN_10085fd4`, `N3Msg_GetBuffTotalTime`).
    pub const TIME_EXIST: u32 = 8;
    /// `Level` (0x36): the NCU the program occupies (column 10, "NCUcost"; `FUN_100512af` adds it to `CurrentNCU`).
    pub const NCU: u32 = 0x36;
    /// `Icon` (0x4f): rdb 1010008 image id.
    pub const ICON: u32 = 0x4f;
    /// `RechargeDelay` (0xd2): column 7.
    pub const RECHARGE_DELAY: u32 = 0xd2;
    /// `AttackRange` (0x11f): column 5.
    pub const ATTACK_RANGE: u32 = 0x11f;
    /// `ItemDelay` (0x126): column 6 ("AtkDelay").
    pub const ATTACK_DELAY: u32 = 0x126;
    /// `School` (0x195): 1..=5 = Combat, Medical, Protection, Psi, Space; the tab is `school - 1`, anything else the "Favorites" page (`FUN_100d3eed`).
    pub const SCHOOL: u32 = 0x195;
    /// `NanoPoints` (0x197): column 9 ("NanoCost").
    pub const NANO_POINTS: u32 = 0x197;
    /// Column 16 ("StackingLine"): `stat 0x227 & 0xffff | stat 0x4b << 16` (`FUN_1003d50c`).
    pub const STACKING_LINE: u32 = 0x227;
}

#[derive(Clone, Debug)]
pub struct NanoInfo {
    pub name: String,
    pub stats: Vec<(u32, i32)>,
    /// rdb 1010008 icon.
    pub icon: Option<(GfxId, u32, u32)>,
}

impl NanoInfo {
    pub fn stat(&self, id: u32) -> Option<i32> {
        self.stats.iter().find(|s| s.0 == id).map(|s| s.1)
    }

    /// The school page (0..=4) or `None` for the favourites page.
    pub fn school(&self) -> Option<usize> {
        self.stat(stat::SCHOOL).filter(|s| (1..=5).contains(s)).map(|s| (s - 1) as usize)
    }

    /// `FUN_10085fd4`: the effect's total time in 1/100 s (the float at `+0xb0` of the dynel is a runtime value of a casting nano; a program in the list has none).
    pub fn total_time(&self) -> i32 {
        self.stat(stat::TIME_EXIST).unwrap_or(0).max(0)
    }
}

pub struct NanoDb {
    store: Option<RecordStore>,
    cache: HashMap<i32, Option<NanoInfo>>,
}

impl NanoDb {
    pub fn new(dir: &Path) -> Self {
        Self { store: RecordStore::open(dir).ok(), cache: HashMap::new() }
    }

    /// The record of nano `id`; `None` without rdb or record (`FUN_100a45ba` finds no template: the window lists no row).
    pub fn info(&mut self, gui: &mut Gui, id: i32) -> Option<&NanoInfo> {
        if !self.cache.contains_key(&id) {
            let info = self.store.as_ref().and_then(|s| {
                let rec = s.get(NANO_RDB_TYPE, u32::try_from(id).ok()?).ok()??;
                let t = parse_item_template(&rec).ok()?;
                let icon = t.stat(stat::ICON).filter(|&i| i > 0).and_then(|i| icon_image(gui, s, i as u32));
                Some(NanoInfo { name: t.name.clone().unwrap_or_default(), stats: t.stats, icon })
            });
            self.cache.insert(id, info);
        }
        self.cache[&id].as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Option<std::path::PathBuf> {
        let d = dirs_home()?.join("Games/ProjectRubiKa/client");
        d.join("cd_image/rdb.db").exists().then_some(d)
    }

    fn dirs_home() -> Option<std::path::PathBuf> {
        std::env::var_os("HOME").map(Into::into)
    }

    /// Shadow Touch (163449: the nano the live capture's NPCs cast): a record of the install, read through the item-template layout.
    #[test]
    fn real_nano_record() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let rec = store.get(NANO_RDB_TYPE, 163449).unwrap().expect("record");
        let t = parse_item_template(&rec).unwrap();
        assert_eq!(t.name.as_deref(), Some("Shadow Touch"));
        assert_eq!((t.stat(stat::SCHOOL), t.stat(stat::NCU), t.stat(stat::ICON)), (Some(5), Some(999), Some(16202)));
        let info = NanoInfo { name: "Shadow Touch".into(), stats: t.stats, icon: None };
        assert_eq!(info.school(), Some(4));
        assert_eq!(info.total_time(), 0);
        // a timed nano
        let rec = store.get(NANO_RDB_TYPE, 25982).unwrap().expect("record");
        let t = parse_item_template(&rec).unwrap();
        assert_eq!((t.stat(stat::TIME_EXIST), t.stat(stat::NCU)), (Some(45000), Some(14)));
    }
}
