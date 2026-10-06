//! Skill rules of `Gamecode.dll` (`N3Msg_GetSkill` modes 1/2, `GetSkillMax`, `GetSkillCost`, `GetSkillCostLevel`) over the cost tables the client loads
//! from the resource database. Evidence and address list: `docs/gui.md` §11.8.
//!
//! Tables (all little-endian `i32` streams, `DbObject` identity → rdb type from the loaders `FUN_100c5ef7`, `FUN_100c5fe3`, `FUN_100c604d`,
//! `FUN_100c5f61`, `FUN_100c5e78`, `FUN_100c62af` [GC 0x100c646d dispatcher]):
//!
//! | rdb | id | content | loader |
//! |---|---|---|---|
//! | 1000206 (`0xf430e`) | profession 1..15 | 77 cost factors in the stat order of `FUN_100c4afd` ([`COST_ORDER`]) | `FUN_100c5201` → `FUN_100c4afd` |
//! | 1000209 (`0xf4311`) | cost class 1..5 | `T1..T7` (maximum skill at title level n), `B` (points per level) | `FUN_100c595c` → `FUN_100c57c3` |
//! | 1000200 (`0xf4308`) | profession 1..15 | 7 rows of 4 ints, `row[0]` = first character level of title level n | `FUN_100c49ce` → `FUN_100c48f3` |
//! | 1000204 (`0xf430c`) | stat 100..168 | 6 trickle-down percentages (Str, Agi, Sta, Int, Sen, Psy) | `FUN_100c56c1` → `FUN_100c564e` |
//! | 1000027 (`0xf425b`) | breed 1..4 | breed, 6 base abilities, 6 ability caps | `FUN_100c5c6a` → `FUN_100c5afd` |
//! | 1000210 (`0xf4312`) | breed 1..5 | ability cost class, stream order Str, Int, Agi, Sen, Sta, Psy | `FUN_100bfddc` |
//!
//! The modifier containers (`FUN_1006460b`'s per-stat bonus map, `FUN_10063d39`'s percent boost map) are in [`super::buffs`] (`GetSkill` modes 1 and 2);
//! the `FUN_10064ac2` mask-4 term (`FUN_1008a3e8` on a third map) is not modelled (only `GetSkillMax` mode 4 reads it).

use super::{BREED, LEVEL, PROFESSION};
use anyhow::Result;
use ao_rdb::RecordStore;
use std::collections::HashMap;

/// `TitleLevel` (stat 37, `GetSkill(0x25)`).
pub const TITLE_LEVEL: u32 = 37;
/// Abilities in the order of the stat ids 0x10..=0x15 (Strength, Agility, Stamina, Intelligence, Sense, Psychic).
pub const ABILITIES: [u32; 6] = [16, 17, 18, 19, 20, 21];

/// Stat order of the 77 cost factors of one rdb 1000206 record (`FUN_100c4afd` calls `FUN_100c4adf(stat, value)` in this sequence).
pub const COST_ORDER: [u8; 77] = [
    152, 137, 154, 153, 155, 156, 138, 142, 144, 100, 143, 93, 95, 139, 166, 117, 140, 92, 97, 168, 123, 124, 118, 119, 149, 120, 151, 167, 111, 148, 133, 134, 150, 114,
    112, 116, 115, 113, 121, 107, 106, 102, 104, 147, 105, 145, 146, 103, 101, 91, 128, 132, 130, 127, 129, 122, 131, 90, 126, 125, 96, 94, 165, 164, 135, 136, 163, 161,
    159, 160, 162, 157, 158, 141, 110, 109, 108,
];

/// The two stats whose maximum is always 0 (`DAT_101600dc` in `GetSkillMax`, read from the DLL: 0x8a, 0x8c).
const NO_MAX: [u32; 2] = [0x8a, 0x8c];

/// The own character values the formulas read (`GetSkill(stat, 2)` of 4, 0x25, 0x36, 0x3c and the abilities).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Character {
    pub breed: i32,
    pub profession: i32,
    pub level: i32,
    pub title_level: i32,
    /// Raw ability values, Strength .. Psychic.
    pub abilities: [i32; 6],
}

impl Character {
    /// From a stat lookup (`None` → the stat is absent; the client's missing stat reads as 0, a missing profession as -1).
    pub fn from_stats(get: impl Fn(u32) -> Option<i32>) -> Self {
        let mut abilities = [0; 6];
        for (a, id) in abilities.iter_mut().zip(ABILITIES) {
            *a = get(id).unwrap_or(0);
        }
        Self {
            breed: get(BREED).unwrap_or(0),
            profession: get(PROFESSION).unwrap_or(-1),
            level: get(LEVEL).unwrap_or(0),
            title_level: get(TITLE_LEVEL).unwrap_or(0),
            abilities,
        }
    }
}

#[derive(Debug, Default)]
pub struct SkillTables {
    cost: HashMap<i32, HashMap<u32, i32>>,
    class: HashMap<i32, ([i32; 7], i32)>,
    starts: HashMap<i32, [i32; 7]>,
    trickle: HashMap<u32, [i32; 6]>,
    breed_base: HashMap<i32, [i32; 6]>,
    breed_cap: HashMap<i32, [i32; 6]>,
    ability_class: HashMap<i32, [i32; 6]>,
}

#[cfg(test)]
impl SkillTables {
    /// A table whose only trickle row is `stat` (tests of [`super::buffs`]).
    pub(crate) fn test_trickle(stat: u32, pct: [i32; 6]) -> Self {
        let mut t = Self::default();
        t.trickle.insert(stat, pct);
        t
    }
}

fn ints(b: &[u8]) -> Vec<i32> {
    b.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect()
}

/// Truncation toward zero, `FUN_1013ecf0` (rounds to nearest, then corrects by the sign of the remainder = `(int)x`).
fn ftol(x: f32) -> i32 {
    x as i32
}

impl SkillTables {
    pub fn load(store: &RecordStore) -> Result<Self> {
        let mut t = Self::default();
        let rec = |ty: u32, id: u32| -> Result<Vec<i32>> { Ok(store.get(ty, id)?.map(|b| ints(&b)).unwrap_or_default()) };
        for prof in 1..16 {
            let v = rec(1_000_206, prof)?;
            if v.len() >= COST_ORDER.len() {
                t.cost.insert(prof as i32, COST_ORDER.iter().zip(&v).map(|(&s, &f)| (s as u32, f)).collect());
            }
            let v = rec(1_000_200, prof)?;
            if v.len() >= 28 {
                t.starts.insert(prof as i32, std::array::from_fn(|i| v[4 * i]));
            }
        }
        for c in 1..6 {
            let v = rec(1_000_209, c)?;
            if v.len() >= 8 {
                t.class.insert(c as i32, (std::array::from_fn(|i| v[i]), v[7]));
            }
        }
        for stat in 100..169 {
            let v = rec(1_000_204, stat)?;
            if v.len() >= 6 {
                t.trickle.insert(stat, std::array::from_fn(|i| v[i]));
            }
        }
        for breed in 1..5 {
            let v = rec(1_000_027, breed)?;
            if v.len() >= 13 {
                t.breed_base.insert(breed as i32, std::array::from_fn(|i| v[1 + i]));
                t.breed_cap.insert(breed as i32, std::array::from_fn(|i| v[7 + i]));
            }
        }
        for breed in 1..6 {
            let v = rec(1_000_210, breed)?;
            if v.len() >= 6 {
                // stream order Str, Int, Agi, Sen, Sta, Psy -> stat order Str, Agi, Sta, Int, Sen, Psy
                t.ability_class.insert(breed as i32, [v[0], v[2], v[4], v[1], v[3], v[5]]);
            }
        }
        Ok(t)
    }

    /// `FUN_100c4ab1(profession, stat)`: the stat's cost factor of the profession (0 without table, a stored 0 counts as 1).
    pub fn cost_factor(&self, profession: i32, stat: u32) -> i32 {
        match self.cost.get(&profession) {
            Some(m) => m.get(&stat).copied().filter(|&f| f != 0).unwrap_or(1),
            None => 0,
        }
    }

    /// `FUN_100c5782(class, title_level)`: `T[title_level]` of cost class `class` (0 without table).
    fn class_t(&self, class: i32, tl: i32) -> i32 {
        match (self.class.get(&class), tl) {
            (Some((t, _)), 1..=7) => t[tl as usize - 1],
            _ => 0,
        }
    }

    /// `FUN_100c573e`: points per level `B` of a cost class (6 without table or out of 1..=10).
    fn class_b(&self, class: i32) -> i32 {
        let b = self.class.get(&class).map_or(6, |c| c.1);
        if (1..=10).contains(&b) {
            b
        } else {
            6
        }
    }

    /// `FUN_100c499b(profession, tl)` → `[+0xc]`: first level of title level `tl` (1..=7).
    fn title_start(&self, profession: i32, tl: i32) -> Option<i32> {
        let s = self.starts.get(&profession)?;
        (1..=7).contains(&tl).then(|| s[tl as usize - 1])
    }

    /// `FUN_10064ac2(stat, mask)` trickle-down: `0.25 * Σ pct[k] * ability[k] / 100` over the abilities the caller passes (buffed ones for `GetSkill`, [`super::buffs`]).
    pub fn trickle(&self, stat: u32, ab: [i32; 6]) -> f32 {
        let Some(p) = self.trickle.get(&stat) else { return 0.0 };
        let d = |k: usize| f64::from(p[k]) * f64::from(ab[k]) / 100.0;
        // the original sums five terms in double, the sixth in float, scales by 0.25f
        let five = d(0) + d(1) + d(2) + d(3) + d(4);
        let last = (p[5] as f32 * ab[5] as f32) / 100.0;
        (last + five as f32) * 0.25
    }

    /// `FUN_100626d8`: maximum of ability `stat` (16..=21) – `min(base + 3·level, cap)` below level 200, `(level-200)·k + cap` from 200
    /// (`k` = 20/15/10 for ability cost class 1/2/3), capped by `cap` (+400/300/200 above level 200).
    pub fn ability_max(&self, stat: u32, ch: &Character) -> i32 {
        let i = (stat - 16) as usize;
        let (Some(base), Some(cap), Some(class)) = (self.breed_base.get(&ch.breed), self.breed_cap.get(&ch.breed), self.ability_class.get(&ch.breed)) else {
            return 0;
        };
        let class = class[i];
        let mut cap_abs = cap[i];
        if ch.level > 200 {
            cap_abs += match class {
                1 => 400,
                2 => 300,
                3 => 200,
                _ => 0,
            };
        }
        let x = if ch.level < 200 {
            base[i] + 3 * ch.level
        } else {
            let k = match class {
                1 => 20,
                2 => 15,
                3 => 10,
                _ => 0,
            };
            (ch.level - 200) * k + cap[i]
        };
        x.min(cap_abs)
    }

    /// `N3Msg_GetSkillMax` = `FUN_100651b5` (skills 100..=168; `mode4` of the trickle is the plain trickle here).
    pub fn skill_max(&self, stat: u32, ch: &Character) -> i32 {
        if NO_MAX.contains(&stat) {
            return 0;
        }
        if (16..=21).contains(&stat) {
            return self.ability_max(stat, ch);
        }
        let f = self.cost_factor(ch.profession, stat);
        let class = ftol(f as f32 / 10.0 + 0.5);
        let mut max = 1000.0f32;
        if (100..=168).contains(&stat) && ch.profession != -1 {
            let tl = ch.title_level;
            let a = self.class_t(class, tl) as f32;
            if a <= 1000.0 {
                max = a;
            }
            let b = self.class_b(class);
            if tl > 1 {
                let prev = self.class_t(class, tl - 1) as f32;
                if let Some(start) = self.title_start(ch.profession, tl) {
                    let v = if tl == 7 { ftol(prev) } else { ftol(((ch.level.min(0xcc) - start + 1) * b) as f32 + prev) };
                    max = max.min(v as f32);
                }
            } else if tl == 1 {
                let v = ftol((ch.level * b) as f32 + 5.0);
                max = max.min(v as f32);
            }
        }
        let tr = ftol(self.trickle(stat, ch.abilities) * 8.0);
        if tr > 0 {
            max = max.min(tr as f32);
        }
        if ch.title_level > 1 && ch.profession != -1 {
            if let Some(start) = self.title_start(ch.profession, ch.title_level) {
                let v = ((ch.level + 1 - start) * 6) as f32 + self.class_t(class, ch.title_level - 1) as f32;
                max = max.min(v);
            }
        }
        if ch.level > 200 {
            max += ((6 - class) * (ch.level - 200) * 5) as f32;
        }
        ftol(max)
    }

    /// `N3Msg_GetSkillCostLevel` = `FUN_10062398`: the cost factor (skills), ten times the ability cost class, 1 without profession.
    pub fn cost_level(&self, stat: u32, ch: &Character) -> i32 {
        if ch.profession == -1 {
            1
        } else if (16..=21).contains(&stat) {
            self.ability_class.get(&ch.breed).map_or(999, |c| c[(stat - 16) as usize]) * 10
        } else if (100..=168).contains(&stat) {
            self.cost_factor(ch.profession, stat)
        } else {
            0
        }
    }

    /// `N3Msg_GetSkillCost(stat, n)` = `FUN_10061fdb`: IP price of raising the skill from `n` by one (`n` < 1 counts as 1).
    pub fn cost(&self, stat: u32, n: i32, ch: &Character) -> f32 {
        if ch.profession == -1 {
            return 1.0;
        }
        let n = n.max(1);
        if (16..=21).contains(&stat) {
            let class = self.ability_class.get(&ch.breed).map_or(999, |c| c[(stat - 16) as usize]);
            (class * n) as f32
        } else if (100..=168).contains(&stat) {
            (self.cost_factor(ch.profession, stat) * n) as f32 / 10.0
        } else {
            n as f32 * 0.5
        }
    }

    /// Total IP of `pending` further points from raw value `raw` (`FUN_100f90ef`: Σ trunc(cost(raw + i)) for i in 0..pending).
    pub fn cost_of_points(&self, stat: u32, raw: i32, pending: i32, ch: &Character) -> i32 {
        (0..pending.max(0)).map(|i| ftol(self.cost(stat, raw + i, ch))).sum()
    }
}

/// Pending-IP distribution: stat ids by `ipdist.xml` priority for (level, profession name): `pri` 1 first … 3 last, 0 = never raised
/// (`FUN_100fac49` skips the bucket with key 0, walks the other buckets in ascending key order).
pub fn ip_priorities(dist: &[super::DistProfession], profession: &str, level: i32, name_of: impl Fn(u32) -> String, skills: &[u32]) -> Vec<(u32, u32)> {
    let Some(p) = dist.iter().find(|p| p.name == profession) else { return vec![] };
    let mut out = vec![];
    for &s in skills {
        let n = name_of(s);
        // "Parry" is the pre-rename name of 145 "Deflect" (docs/gui.md §11.3)
        let n = if s == 145 { "Parry".to_string() } else { n };
        if let Some(d) = p.stats.iter().find(|d| d.name == n) {
            if (d.min as i32..=d.max as i32).contains(&level) && d.pri > 0 {
                out.push((d.pri, s));
            }
        }
    }
    out.sort_by_key(|&(pri, _)| pri);
    out.into_iter().map(|(p, s)| (s, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Option<std::path::PathBuf> {
        let dir = std::env::var_os("AO_CLIENT_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Games/ProjectRubiKa/client")
        });
        dir.join("cd_image/rdb.db").exists().then_some(dir)
    }

    #[test]
    fn cost_order_is_the_77_skill_and_ac_stats_once() {
        let mut v = COST_ORDER.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), 77);
        assert_eq!((v[0], v[76]), (90, 168));
    }

    #[test]
    fn ability_cap_and_trickle_follow_the_client_formulas() {
        let mut t = SkillTables::default();
        t.breed_base.insert(1, [6; 6]);
        t.breed_cap.insert(1, [472, 480, 480, 480, 480, 480]);
        t.ability_class.insert(1, [2; 6]);
        t.trickle.insert(152, [0, 0, 50, 0, 0, 50]);
        let ch = Character { breed: 1, profession: 1, level: 1, title_level: 1, abilities: [6; 6] };
        // base 6 + 3 * level 1
        assert_eq!(t.ability_max(16, &ch), 9);
        // level 205: (205 - 200) * 15 + cap 472 (class 2), below the raised cap 472 + 300
        assert_eq!(t.ability_max(16, &Character { level: 205, ..ch }), 5 * 15 + 472);
        // trickle: 0.25 * (50*6/100 + 50*6/100) = 1.5 -> truncated 1
        assert!((t.trickle(152, ch.abilities) - 1.5).abs() < 1e-6);
        assert_eq!(super::super::buffs::skill_base(&t, 152, 5, &ch, &Default::default(), 0), 6);
        assert_eq!(t.cost(16, 6, &ch), 12.0);
    }

    #[test]
    fn real_tables_load_and_new_character_values_are_sane() {
        let Some(dir) = client() else { return };
        let t = SkillTables::load(&RecordStore::open(&dir).unwrap()).unwrap();
        assert_eq!((t.cost.len(), t.class.len(), t.starts.len(), t.trickle.len()), (15, 5, 15, 69));
        assert_eq!((t.breed_base.len(), t.ability_class.len()), (4, 5));
        // profession 1, rdb 1000200 id 1: title levels start at 1, 15, 50, 100, 150, 190, 205
        assert_eq!(t.starts[&1], [1, 15, 50, 100, 150, 190, 205]);
        // the captured new character (zone_newchar_ithaca.rec): breed 1, profession 1, level 1, title level 1, abilities 6
        let ch = Character { breed: 1, profession: 1, level: 1, title_level: 1, abilities: [6; 6] };
        assert_eq!(t.ability_max(16, &ch), 9);
        for stat in 100..169u32 {
            let m = t.skill_max(stat, &ch);
            assert!((0..=1000).contains(&m), "stat {stat} max {m}");
        }
        // 10000000 marks "cannot be raised" (stats 90..=92 of every profession but 2)
        assert_eq!(t.cost_factor(1, 92), 10_000_000);
        assert_eq!(t.cost_factor(2, 92), 50);
        for s in [152u32, 111, 100, 130] {
            eprintln!("stat {s}: factor {} max {} cost@5 {}", t.cost_factor(1, s), t.skill_max(s, &ch), t.cost(s, 5, &ch));
        }
    }
}
