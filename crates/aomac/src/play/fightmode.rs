//! District fight-mode level of the own character (`FUN_1003e1d0` [GC], reached through `FUN_1003e228`): the level the `FollowTargetIIR_c` gate
//! (`FUN_100732e3`), `/follow` and the attack permission code compare against 1 / 2. Evidence: docs/zone/movement.md §10.2.
//!
//! * `PlayfieldAnarchy_t::Get(playfield)` + `+0xb8` (`PlayfieldDistrictInfo_t`, rdb 1000014 = [`Districts`]) +
//!   `GetDistrictData(GetZoneInstanceID())` (`zone_to_district[zone]`, the zone is [`ZoneLocator::zone_at`]) + `PlayfieldAnarchy_t::
//!   GetFightModeHandler` (created on demand whenever the district info exists): without any of them the level is **2**.
//! * Otherwise `FUN_1011f88b(handler, district)`: start from `DistrictData+0x54` (the `fight mode` byte), walk the district's
//!   `FightModeChange_t` list in order: `if !fixed_seen || change.fixed { fixed_seen |= change.fixed; level = change.set ? value : level + value }`,
//!   finally `clamp(level, 0, 4)` (`FUN_1002d366`).
//! * The changes come from `FightModeUpdate_t` ([`ao_net::n3::server_move::FightModeUpdate`]): a change with the remove flag deletes the change of
//!   that id (`FUN_1011f913`), any other adds it (`FUN_1011f980`); a message naming an unknown district is dropped as a whole
//!   (`FUN_101248ef` returns 3).
//!
//! Not ported: `FUN_1003e228` returns the controller's cached `+0xf4` instead while the character's state flags `+0x138` have bit 2 (dead) or
//! 16 set; the cache is written by `FUN_1003f239` (which also prints the district text LDB 0x6e and ends a fight when the level changes).

use ao_audio::district::Districts;
use ao_formats::playfield::{zone_locator, ZoneLocator};
use ao_net::n3::server_move::{FightModeChange, FightModeUpdate};
use ao_rdb::RecordStore;

/// rdb type of `PlayfieldDistrictInfo_t` (`GameData::PlayfieldDistrictInfo_t::ReadBlob`).
const DISTRICT_INFO: u32 = 1_000_014;
/// The level `FUN_1003e1d0` returns when any piece of the district data is missing.
pub const DEFAULT_LEVEL: i32 = 2;
/// Bounds of `FUN_1002d366(&level, &0, &4)`.
const MAX_LEVEL: i32 = 4;

pub struct FightLevels {
    districts: Districts,
    zones: ZoneLocator,
    /// The playfield's `FightModeHandler_t` map, flattened: the changes in arrival order (the per-district vectors keep this order).
    changes: Vec<FightModeChange>,
}

impl FightLevels {
    /// The district data of `playfield`; `None` when it has none (the level is then [`DEFAULT_LEVEL`]).
    pub fn load(store: &RecordStore, playfield: u32) -> Option<Self> {
        let blob = store.get(DISTRICT_INFO, playfield).ok().flatten()?;
        let districts = Districts::parse(&blob).map_err(|e| eprintln!("fight mode: district table of playfield {playfield}: {e:#}")).ok()?;
        let zones = zone_locator(store, playfield).map_err(|e| eprintln!("fight mode: zone locator of playfield {playfield}: {e:#}")).ok()?;
        Some(Self { districts, zones, changes: Vec::new() })
    }

    /// `FUN_1003e1d0` for a character standing at `pos` (server coordinates).
    pub fn level(&self, pos: [f32; 3]) -> i32 {
        let Some(zone) = self.zones.zone_at([pos[0], pos[1], -pos[2]]) else { return DEFAULT_LEVEL };
        let Some(&idx) = self.districts.zone_to_district.get(zone) else { return DEFAULT_LEVEL };
        let Some(d) = self.districts.districts.get(idx as usize) else { return DEFAULT_LEVEL };
        let mut level = i32::from(d.fight_mode);
        let mut fixed_seen = false;
        for c in self.changes.iter().filter(|c| c.district == d.name) {
            if !fixed_seen || c.fixed {
                fixed_seen |= c.fixed;
                level = if c.set { i32::from(c.value) } else { level + i32::from(c.value) };
            }
        }
        level.clamp(0, MAX_LEVEL)
    }

    /// `FightModeUpdate_t` apply [GC 0x10124b70] after the validity check `FUN_101248ef`: every named district must exist, else nothing
    /// happens. Returns whether the update was applied.
    pub fn update(&mut self, u: &FightModeUpdate) -> bool {
        if !u.changes.iter().all(|c| self.districts.districts.iter().any(|d| d.name == c.district)) {
            return false;
        }
        for c in &u.changes {
            if c.remove {
                // `FUN_1011f913`: the first change with this id, in any district
                if let Some(i) = self.changes.iter().position(|x| x.id == c.id) {
                    self.changes.remove(i);
                }
            } else {
                self.changes.push(c.clone());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Option<std::path::PathBuf> {
        let d = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        d.join("cd_image/rdb.db").exists().then_some(d)
    }

    fn change(id: u32, set: bool, fixed: bool, remove: bool, value: i8) -> FightModeChange {
        FightModeChange { id, district: String::new(), set, fixed, remove, value }
    }

    /// The level arithmetic of `FUN_1011f88b` on every playfield of the shipped rdb and a hand-made update list.
    #[test]
    fn levels_from_the_district_records_and_updates() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        // every playfield with a district record loads; a playfield without one yields no table (default level)
        let ids = store.ids(DISTRICT_INFO).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for id in ids.iter().take(60) {
            let Some(mut f) = FightLevels::load(&store, *id) else { continue };
            for d in &f.districts.districts {
                seen.insert(d.fight_mode);
                assert!(d.fight_mode <= 7, "playfield {id}: fight mode {}", d.fight_mode);
            }
            // the centre of every district is in some zone; the level there is the base clamped to 0..=4
            let d0 = f.districts.districts[0].clone();
            let at = [d0.centre[0], d0.centre[1], d0.centre[2]];
            let base = f.level(at);
            assert!((0..=4).contains(&base));
            // a missing district name drops the whole update
            let bad = FightModeUpdate { playfield: Default::default(), changes: vec![FightModeChange { district: "nonexistent".into(), ..change(1, true, false, false, 1) }] };
            assert!(!f.update(&bad) && f.changes.is_empty());
        }
        assert!(!seen.is_empty());
        eprintln!("fight modes seen in 60 playfields: {seen:?}");
    }

    #[test]
    fn change_list_order_fixed_and_clamp() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let ids = store.ids(DISTRICT_INFO).unwrap();
        let mut f = ids.iter().find_map(|id| FightLevels::load(&store, *id)).expect("a playfield with districts");
        let d = f.districts.districts[0].clone();
        let at = d.centre;
        let zone_ok = f.zones.zone_at([at[0], at[1], -at[2]]).and_then(|z| f.districts.zone_to_district.get(z)).is_some_and(|i| f.districts.districts[*i as usize].name == d.name);
        if !zone_ok {
            return; // the centre lies in another district of this playfield
        }
        let base = i32::from(d.fight_mode);
        let c = |id, set, fixed, v| FightModeChange { district: d.name.clone(), ..change(id, set, fixed, false, v) };
        let upd = |changes| FightModeUpdate { playfield: Default::default(), changes };
        assert!(f.update(&upd(vec![c(1, false, false, 2)])));
        assert_eq!(f.level(at), (base + 2).clamp(0, 4), "add");
        assert!(f.update(&upd(vec![c(2, true, true, 1)])));
        assert_eq!(f.level(at), 1, "a fixed set wins from here on");
        assert!(f.update(&upd(vec![c(3, false, false, 4)])));
        assert_eq!(f.level(at), 1, "non-fixed changes after a fixed one are skipped");
        assert!(f.update(&upd(vec![FightModeChange { remove: true, ..c(2, true, true, 0) }])));
        assert_eq!(f.level(at), (base + 2 + 4).clamp(0, 4), "removal by id, clamp to 4");
    }
}
