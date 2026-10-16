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

/// Effective mode0 for non-skill nano stats. Zone supplies the actual
/// SimpleChar constructor presets; only unlisted slots read INVALID.
pub(super) fn effective_nano_stat(zone: &super::zone::Zone, id: u32) -> i32 {
    if zone.stat(id).is_some() { return zone.skill_value(id).expect("character raw stat"); }
    let raw = f64::from(ao_formats::stats::INVALID);
    let base = (raw * f64::from(zone.skill_modifiers.multiplier(id))) as i64 as i32;
    base.wrapping_add(zone.skill_modifiers.bonus_of(id, base, 0))
}

/// GC10026dd7 -> 1004e3e1, shared by cast admission and InfoView.
pub(super) fn nano_cost_multiplier(zone: &super::zone::Zone) -> i32 {
    if zone.char_id == 0 { return 0; }
    let value = effective_nano_stat(zone, 318);
    // GC100583eb/10058415 mirror actual Flags bit into actor+21c.
    if zone.stat(0).is_some_and(|flags| flags & 0x10000000 != 0) { return value; }
    let minimum = match zone.stat(4) { Some(1 | 2) => 50, Some(3) => 45, Some(4) => 55, _ => 0 };
    value.max(minimum)
}

#[derive(Clone, Debug)]
pub struct NanoInfo {
    pub id: i32,
    pub name: String,
    pub stats: Vec<(u32, i32)>,
    /// Use/action3, selected by GC1004f935 -> 100811ac -> 10081149.
    pub criteria: Vec<[i32; 3]>,
    /// Event 0: N3Msg_GetModifierString's list and N3Msg_IsTeamNano's search.
    pub spells: Vec<ao_net::n3::spells::Spell>,
    /// rdb 1010008 icon.
    pub icon: Option<(GfxId, u32, u32)>,
}

impl NanoInfo {
    pub fn stat(&self, id: u32) -> Option<i32> {
        self.stats.iter().find(|s| s.0 == id).map(|s| s.1)
            .or_else(|| (id == 75).then_some(self.id)) // GC10085bf8 primary family default.
    }

    /// The school page (0..=4) or `None` for the favourites page.
    pub fn school(&self) -> Option<usize> {
        self.stat(stat::SCHOOL).filter(|s| (1..=5).contains(s)).map(|s| (s - 1) as usize)
    }

    /// Template `TimeExist`, used by Programs; active effects use the lifecycle's server-provided runtime duration.
    pub fn total_time(&self) -> i32 {
        self.stat(stat::TIME_EXIST).unwrap_or(0).max(0)
    }

    /// GUI1003eacb/1003cbcb. Static NanoItem+bc is false (GC100861a4).
    pub fn type_text(&self) -> &'static str {
        if self.stat(stat::FLAGS).unwrap_or(1) & 0x8000 != 0 {
            "<font color=\"red\">Hostile</font>"
        } else if self.spells.iter().any(|s| s.function == 0xcf4a) {
            "<font color=\"green\">Team</font>"
        } else {
            "<font color=\"white\">Friendly</font>"
        }
    }
    pub fn cast_target(&self, own: u32, target: ao_net::msg::Identity) -> ao_net::msg::Identity {
        if target == ao_net::msg::Identity::default() && self.stat(stat::FLAGS).unwrap_or(1) & 0x8000 == 0 {
            ao_net::msg::Identity { kind: ao_net::n3::outgoing::DYNEL_CHAR, instance: own as i32 }
        } else { target }
    }


    /// Template, criteria and stat-holder portion of GC1004f6a0, combined
    /// with HudNano's one shared runtime admission guard before encoding.
    pub fn can_cast(&self, zone: &super::zone::Zone, target: ao_net::msg::Identity) -> anyhow::Result<bool> {
        // GC10058e52 replaces raw Features224 bit0x800 with IsGrid, even
        // when no Features slot was received. Vehicle662 is ctor-seeded0.
        let features = (zone.stat(224).unwrap_or(ao_formats::stats::INVALID) & !0x800) | if zone.is_grid { 0x800 } else { 0 };
        if zone.stat(662).is_some_and(|v| v != 0) || features & 0x800 != 0 || features & 2 == 0 { return Ok(false); }
        // 1004e3e1 returns the cost multiplier, NOT the percentage removed.
        let multiplier = nano_cost_multiplier(zone);
        // FILD/FIMUL/FDIV -> native ftol low32 (1004f827..1004f833).
        let cost = (i64::from(self.stat(stat::NANO_POINTS).unwrap_or(1)) * i64::from(multiplier) / 100) as i32;
        // 1004f846 reads current NanoEnergy214; NanoPoints407 is the template cost.
        if zone.stat(214).unwrap_or(ao_formats::stats::INVALID) < cost { return Ok(false); }
        anyhow::ensure!(zone.completed_perks.is_some() || !self.criteria.iter().any(|c| matches!(c[2], 93 | 103)),
            "nano criterion needs the authoritative completed-perk snapshot");
        if !super::chat::info::item_eligibility::evaluate(&self.criteria, zone, zone.completed_perks.as_deref().unwrap_or(&[]))? { return Ok(false); }
        // NanoItem ctor100861a4 initializes +bc to zero; 10085bf8 and
        // template loader1008622a do not change it. No template-derived self flag.
        let local = self.cast_target(zone.char_id, target);
        // 10058e36 requires a resolved character, not an arbitrary selected object.
        Ok(local.kind == ao_net::n3::outgoing::DYNEL_CHAR
            && (local.instance == zone.char_id as i32 || zone.dynels.contains_key(&local.instance)))
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
                let mut data = super::chat::info::template_spells::data(&rec, 0).ok()?;
                let criteria = data.criteria.iter().rposition(|e| e.1 == 3).map(|i| data.criteria.swap_remove(i).2).unwrap_or_default();
                Some(NanoInfo { id, name: t.name.clone().unwrap_or_default(), stats: t.stats, criteria, spells: data.spells, icon })
            });
            self.cache.insert(id, info);
        }
        self.cache[&id].as_ref()
    }

    /// GC100236fa -> ConvertSpellList, sharing the InfoView opcode formatter.
    pub fn modifiers(&self, info: &NanoInfo, zone: &super::zone::Zone, texts: &ao_formats::screens::TextDb) -> anyhow::Result<String> {
        let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("nano record store unavailable"))?;
        super::chat::info::item_effects::event_text(&info.spells, true, zone, store, texts)
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
        let info = NanoInfo { id: 163449, name: "Shadow Touch".into(), stats: t.stats, criteria: vec![], spells: vec![], icon: None };
        assert_eq!(info.school(), Some(4));
        assert_eq!(info.total_time(), 0);
        // a timed nano
        let rec = store.get(NANO_RDB_TYPE, 25982).unwrap().expect("record");
        let t = parse_item_template(&rec).unwrap();
        assert_eq!((t.stat(stat::TIME_EXIST), t.stat(stat::NCU)), (Some(45000), Some(14)));
    }

    #[test]
    fn cost_multiplier_breed_floor_and_unknown_criteria() {
        let mut zone = super::super::zone::Zone::new(7);
        let mut info = NanoInfo { id: 1, name: String::new(), stats: vec![(stat::NANO_POINTS, 101)], criteria: vec![], spells: vec![], icon: None };
        zone.stats.extend([(4, 3), (318, 10), (214, 44)]);
        assert!(!info.can_cast(&zone, Default::default()).unwrap());
        zone.stats.insert(214, 45);
        assert!(info.can_cast(&zone, Default::default()).unwrap(), "45.45 truncates; breed3 floor45");
        for (breed, cost) in [(1, 50), (2, 50), (4, 55)] {
            zone.stats.extend([(4, breed), (214, cost - 1)]);
            assert!(!info.can_cast(&zone, Default::default()).unwrap());
            zone.stats.insert(214, cost);
            assert!(info.can_cast(&zone, Default::default()).unwrap());
        }
        zone.stats.extend([(0, 0x10000000), (214, 10)]);
        assert_eq!(nano_cost_multiplier(&zone), 10, "actual NPC Flags skip the breed floor");
        assert!(info.can_cast(&zone, Default::default()).unwrap());
        zone.stats.remove(&0);
        zone.stats.remove(&318);
        zone.stats.insert(214, 100);
        assert_eq!(effective_nano_stat(&zone, 318), 100);
        assert_eq!(effective_nano_stat(&zone, 381), 0);
        assert_eq!(effective_nano_stat(&zone, 1200), ao_formats::stats::INVALID);
        assert!(!info.can_cast(&zone, Default::default()).unwrap(), "constructor multiplier100 costs101");
        zone.stats.insert(214, 101);
        assert!(info.can_cast(&zone, Default::default()).unwrap());
        zone.stats.remove(&214);
        assert!(!info.can_cast(&zone, Default::default()).unwrap(), "constructor NanoEnergy is zero");
        zone.stats.insert(214, 101);
        zone.is_grid = true;
        assert!(!info.can_cast(&zone, Default::default()).unwrap(), "Grid classification does not depend on a received Features bit");
        zone.is_grid = false;
        zone.stats.insert(224, 0x802);
        assert!(info.can_cast(&zone, Default::default()).unwrap(), "non-Grid clears stale raw Grid bit");
        info.criteria = vec![[0, 0, 144]];
        assert!(info.can_cast(&zone, Default::default()).is_err());
        info.criteria = vec![[0, 1, 103]];
        assert!(info.can_cast(&zone, Default::default()).is_err(), "missing perk snapshot is not an empty snapshot");
        zone.completed_perks = Some(vec![]);
        assert!(info.can_cast(&zone, Default::default()).unwrap(), "known-empty snapshot satisfies NotCompleted1");
        zone.completed_perks = Some(vec![1]);
        assert!(!info.can_cast(&zone, Default::default()).unwrap());
        info.criteria[0][2] = 93;
        assert!(info.can_cast(&zone, Default::default()).unwrap(), "completed predicate uses the same received snapshot as inventory/perks");
    }
}
