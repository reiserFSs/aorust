//! Maximum health (stat 1 `Life`) and maximum nano (stat 221 `MaxNanoEnergy`) of the own character: the client computes them, the server's values
//! for these two stats are not used (PRK's `FullCharacterIIR_t` carries `Life` = 1 and `MaxNanoEnergy` = 1, docs/captures/zone_*.rec).
//!
//! `FUN_1006208d(this = SimpleChar stat controller, heal_nano: bool, full_heal: bool)` [Gamecode.dll 0x1006208d], runs for the client character
//! (`dynel+0x140 != 0`, `dynel+0x21c == 0`) from the dynel set-up `FUN_1005bea6`, `FUN_100588cc`, the deferred-flag handlers `FUN_10057d5d` /
//! `FUN_10080a79` (flag `dynel+0x138` bit 6, set by `SetStat` `FUN_10059e6a` for stats `0x84` NanoPool and `0x98` BodyDevelopment) and the ability reroll
//! `FUN_10062411`:
//! ```text
//! rec    = FUN_100c499b(profession(0x3c), title_level(0x25))     // rdb 1000200 row, 0 when outside 1..=7 / unknown profession
//! hp_lvl = level(0x36) * rec[+8]   nano_lvl = level * rec[+4]     // row[2] / row[1] of the profession's title-level table
//! b      = FUN_100c43d4(breed(4), 600..=0x25d)                    // rdb 1000203[breed] = six ints stored as stats 600, 0x259, 0x25a, 0x25b, 0x25c, 0x25d
//! Life          = level * b[0x259] + GetStat(0x98, 0) * b[0x25a] + b[600] + hp_lvl     // SetStat(1, ..)
//! MaxNanoEnergy = level * b[0x25c] + GetStat(0x84, 0) * b[0x25d] + b[0x25b] + nano_lvl // SetStat(0xdd, ..)
//! ```
//! `GetStat(stat, 0)` is `GetSkill(stat, 2)`: the raw stat plus its trickle-down ([`SkillTables::trickle`]); the bonus / percent maps of the stat
//! modifier object (`SimpleChar+0x1bc`: buffs, items) are not wired into the own stats yet, so the value is unbuffed (UNRESOLVED, docs/gui.md 11.10). A breed without a record yields the error code 2 for each of
//! the three breed values (`FUN_100c43d4`). Check: a level 1 Solitus Soldier with all abilities 6 (BodyDev / NanoPool 5 + trickle 1 = 6) gets
//! `6·1 + 6·3 + 10 = 34` health and `4·1 + 6·3 + 10 = 32` nano, exactly the dynel header's `max_health` 34 and the `CurrentNano` 32 of the captures.

use super::skills::{Character, SkillTables};
use anyhow::Result;
use ao_rdb::RecordStore;
use std::collections::HashMap;

/// Stat `Life` (max health) written by `FUN_1006208d`.
pub const LIFE: u32 = 1;
/// Stat `MaxNanoEnergy` (0xdd) written by `FUN_1006208d`.
pub const MAX_NANO: u32 = 221;
/// `BodyDevelopment` (0x98) and `NanoPool` (0x84), the two skills the maxima scale with.
pub const BODY_DEV: u32 = 152;
pub const NANO_POOL: u32 = 132;

/// The two tables of `FUN_1006208d`.
#[derive(Debug, Default)]
pub struct PoolTables {
    /// rdb 1000203, record = breed 1..=5: the six ints `[600, 0x259, 0x25a, 0x25b, 0x25c, 0x25d]` (`FUN_100c443d`).
    breed: HashMap<i32, [i32; 6]>,
    /// rdb 1000200, record = profession 1..=15: 7 rows of 4 ints (`FUN_100c48f3` keeps them at row stride 0x14).
    title: HashMap<i32, [[i32; 4]; 7]>,
}

impl PoolTables {
    pub fn load(store: &RecordStore) -> Result<Self> {
        let mut t = Self::default();
        let ints = |b: Vec<u8>| -> Vec<i32> { b.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect() };
        for breed in 1..6 {
            if let Some(v) = store.get(1_000_203, breed)?.map(ints).filter(|v| v.len() >= 6) {
                t.breed.insert(breed as i32, std::array::from_fn(|i| v[i]));
            }
        }
        for prof in 1..16 {
            if let Some(v) = store.get(1_000_200, prof)?.map(ints).filter(|v| v.len() >= 28) {
                t.title.insert(prof as i32, std::array::from_fn(|r| std::array::from_fn(|c| v[4 * r + c])));
            }
        }
        Ok(t)
    }

    /// `(Life, MaxNanoEnergy)` for `ch` with the buffed `BodyDevelopment` / `NanoPool` values.
    pub fn max_pools(&self, ch: &Character, body_dev: i32, nano_pool: i32) -> (i32, i32) {
        let (hp_title, nano_title) = match (ch.title_level, self.title.get(&ch.profession)) {
            (tl @ 1..=7, Some(rows)) => (rows[tl as usize - 1][2], rows[tl as usize - 1][1]),
            _ => (0, 0),
        };
        let b = self.breed.get(&ch.breed).copied().unwrap_or([2; 6]);
        let life = ch.level * b[1] + body_dev * b[2] + b[0] + ch.level * hp_title;
        let nano = ch.level * b[4] + nano_pool * b[5] + b[3] + ch.level * nano_title;
        (life, nano)
    }

    /// [`Self::max_pools`] from the own stats: `get` reads `GetStat(stat, 2)`; BodyDevelopment / NanoPool are `GetSkill(stat, 2)` = `trunc(raw) + trunc(trickle)`
    /// (`FUN_100654e1` with no percent / bonus node).
    pub fn own(&self, skills: &SkillTables, get: impl Fn(u32) -> Option<i32>) -> (i32, i32) {
        let ch = Character::from_stats(&get);
        let skill = |stat: u32| get(stat).unwrap_or(0) + skills.trickle(stat, ch.abilities) as i32;
        self.max_pools(&ch, skill(BODY_DEV), skill(NANO_POOL))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> PoolTables {
        let mut t = PoolTables::default();
        t.breed.insert(1, [10, 6, 3, 10, 4, 3]);
        t.title.insert(3, std::array::from_fn(|r| [[1, 0, 0, 0], [15, 1, 0, 0], [50, 2, 0, 0], [0; 4], [0; 4], [0; 4], [0; 4]][r]));
        t
    }

    #[test]
    fn level_one_solitus_gets_the_header_values() {
        let ch = Character { breed: 1, profession: 1, level: 1, title_level: 1, abilities: [6; 6] };
        assert_eq!(tables().max_pools(&ch, 6, 6), (34, 32));
    }

    #[test]
    fn title_row_adds_per_level_health_and_nano() {
        let t = tables();
        // profession 3 title level 2: row [15, 1, 0]: +1 nano per level; title level 3: +2
        let ch = Character { breed: 1, profession: 3, level: 20, title_level: 2, abilities: [6; 6] };
        assert_eq!(t.max_pools(&ch, 10, 10), (20 * 6 + 10 * 3 + 10, 20 * 4 + 10 * 3 + 10 + 20));
        // title level 0 or 8, or an unknown profession: no title term; unknown breed: the error code 2 for each breed value
        let ch = Character { title_level: 0, ..ch };
        assert_eq!(t.max_pools(&ch, 10, 10).1, 20 * 4 + 10 * 3 + 10);
        let ch = Character { breed: 9, ..ch };
        assert_eq!(t.max_pools(&ch, 10, 10), (20 * 2 + 10 * 2 + 2, 20 * 2 + 10 * 2 + 2));
    }

    /// The real client tables reproduce the 34 / 32 of the captured level 1 characters (breed 1, profession 1, all abilities 6, skills 5).
    #[test]
    fn real_tables_match_the_captures() {
        let dir = std::env::var_os("AO_CLIENT_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Games/ProjectRubiKa/client")
        });
        let Ok(store) = RecordStore::open(&dir) else { return };
        let (pools, skills) = (PoolTables::load(&store).unwrap(), SkillTables::load(&store).unwrap());
        let stats: HashMap<u32, i32> = [(4, 1), (0x3c, 1), (0x36, 1), (0x25, 1), (152, 5), (132, 5)].into_iter().chain((16..22).map(|a| (a, 6))).collect();
        assert_eq!(pools.own(&skills, |s| stats.get(&s).copied()), (34, 32));
    }
}
