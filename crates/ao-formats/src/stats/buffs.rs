//! Buffed skill values: `N3Msg_GetSkill(stat, 1 | 2)` with the modifier containers of `SimpleChar+0x1bc` (docs/gui.md §11.10 "Buffed values").
//!
//! What the server sends and what the client adds (all Gamecode.dll, traced from the decompilation and the asm):
//! * `StatIIR` / `FullCharacterIIR` stats are **stored unmodified**: `SetStat` (stat object vtable `0x1015f9ec`+0x40, `FUN_10059e6a`) writes the raw
//!   value that `GetStat(stat, 2)` returns. `GetStat(stat, mode)` (vtable +0x3c, `FUN_10058e52`) maps mode 0 → `FUN_1006554c(stat,1,0)` (= `GetSkill` 2,
//!   buffed), 1 → `FUN_100654e1` (= `GetSkill` 1), 3 → `FUN_10064800`, 4 → `FUN_10065576`, 2 → the stored raw value.
//! * The bonuses live in two `std::map<int,int>` of the stat modifier object (`SimpleChar+0x1bc`): **bonus map** (`this+4`, readers `FUN_1006460b`,
//!   writers `FUN_100644c8` add / `FUN_1006469b` remove / `FUN_10064916` set) and **percent map** (`this+0x10`, reader `FUN_10063d39`, writers
//!   `FUN_1006497b` add / `FUN_10064999` remove). They are filled by the spell executor `FUN_100026d0` (stat object vtable +0x30) running `SpellData_t`
//!   lists: the `FullCharacterIIR_t` spell list at activation (`FUN_10073a2f`), `ApplySpellsIIR_t`, item events. See `hud_stats/buffs.rs` for the spell → map step.
//! * `GetSkill(stat, 1)` = `FUN_100654e1`: `raw`, times `(percent + 100) / 100` when that float is not 1.0 (truncated), plus the truncated trickle-down of the
//!   **buffed** abilities (`FUN_10064ac2` mode 3: every ability `a + FUN_1006460b(ability, a, flag 1)`).
//! * `GetSkill(stat, 2)` = `FUN_1006554c(stat, 1, 0)` = `GetSkill(stat, 1) + FUN_1006460b(stat, GetSkill(stat, 1), 1)`.
//! * `FUN_1006460b(stat, value, flag)` = `bonus_map[stat] - extra`, `extra = trunc(lock * value / 10000.0)` when `flag`, `lock` (own stat `0xf7`,
//!   `FUN_10062526`) > 0 and the stat is an ability (16..=21) or a skill (100..=168); the map read creates a 0 entry like `operator[]`.
//!
//! The skill window colours the value `+0x1a8 = GetSkill(stat, 2)` against `GetSkill(stat, 1)` (`FUN_100fde49`, GUI.dll): equal white `0xFFFFFF`,
//! `base < value` green `0x00FF00`, `value < base` red `0xFF0000` ([`value_color`]).

use super::skills::{Character, SkillTables};
use std::collections::HashMap;

/// Own stat read by `FUN_10062526` (`GetStat(0xf7, 2)`): a lock percentage (in 1/100 %: `lock * value / 10000`) that lowers abilities and skills.
/// The client's stat-name table has no entry for 0xf7.
pub const LOCK_STAT: u32 = 0xf7;

/// The two maps of the stat modifier object (`SimpleChar+0x1bc`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// `this+4`: per-stat bonus added by `GetSkill(stat, 2)` (and to the abilities of the trickle-down).
    pub bonus: HashMap<u32, i32>,
    /// `this+0x10`: per-stat percent boost of `GetSkill(stat, 1)`.
    pub percent: HashMap<u32, i32>,
}

impl Modifiers {
    /// `FUN_1006460b(stat, value, 1)`: the bonus of `stat` for a base `value` under the lock percentage `lock`.
    pub fn bonus_of(&self, stat: u32, value: i32, lock: i32) -> i32 {
        let extra = if lock > 0 && ((100..=168).contains(&stat) || (16..=21).contains(&stat)) {
            (f64::from(lock) * f64::from(value) / 10000.0) as i32
        } else {
            0
        };
        self.bonus.get(&stat).copied().unwrap_or(0) - extra
    }

    /// `FUN_10063d39`: `(percent + 100.0) / 100.0` as a float, 1.0 without an entry (or when it computes to 0).
    pub fn multiplier(&self, stat: u32) -> f32 {
        match self.percent.get(&stat) {
            Some(&p) => match ((f64::from(p) + 100.0) / 100.0) as f32 {
                0.0 => 1.0,
                f => f,
            },
            None => 1.0,
        }
    }
}

/// `FUN_10064ac2` mode 3: the abilities the trickle-down reads (`a + FUN_1006460b(ability, a, 1)`).
pub fn buffed_abilities(ab: [i32; 6], m: &Modifiers, lock: i32) -> [i32; 6] {
    let mut out = ab;
    for ((o, a), id) in out.iter_mut().zip(ab).zip(super::skills::ABILITIES) {
        *o = a + m.bonus_of(id, a, lock);
    }
    out
}

/// `N3Msg_GetSkill(stat, 1)` = `FUN_100654e1(stat, 0)`: `raw` is `GetStat(stat, 2)`, `ch.abilities` the unmodified abilities.
pub fn skill_base(t: &SkillTables, stat: u32, raw: i32, ch: &Character, m: &Modifiers, lock: i32) -> i32 {
    let mult = m.multiplier(stat);
    let own = if mult != 1.0 { (f64::from(raw) * f64::from(mult)) as i32 } else { raw };
    own + t.trickle(stat, buffed_abilities(ch.abilities, m, lock)) as i32
}

/// `N3Msg_GetSkill(stat, 2)` = `FUN_1006554c(stat, 1, 0)` = `GetStat(stat, 0)`: [`skill_base`] plus its bonus.
pub fn skill_value(t: &SkillTables, stat: u32, raw: i32, ch: &Character, m: &Modifiers, lock: i32) -> i32 {
    let base = skill_base(t, stat, raw, ch, m, lock);
    base + m.bonus_of(stat, base, lock)
}

/// The colour `FUN_100fde49` gives the value text: white when `value == base`, green (`0x00FF00`) when `base < value`, red (`0xFF0000`) when `value < base`.
pub fn value_color(value: i32, base: i32) -> u32 {
    match value.cmp(&base) {
        std::cmp::Ordering::Equal => 0xFFFFFF,
        std::cmp::Ordering::Greater => 0x00FF00,
        std::cmp::Ordering::Less => 0xFF0000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(bonus: &[(u32, i32)], percent: &[(u32, i32)]) -> Modifiers {
        Modifiers { bonus: bonus.iter().copied().collect(), percent: percent.iter().copied().collect() }
    }

    #[test]
    fn bonus_map_and_lock_follow_fun_1006460b() {
        let m = mods(&[(108, 42), (16, 5)], &[]);
        assert_eq!(m.bonus_of(108, 100, 0), 42);
        assert_eq!(m.bonus_of(109, 100, 0), 0, "no node reads as 0");
        // lock 500 = 5 %: trunc(500 * 100 / 10000) = 5 off a skill and an ability, nothing off other stats
        assert_eq!(m.bonus_of(108, 100, 500), 42 - 5);
        assert_eq!(m.bonus_of(16, 100, 500), 5 - 5);
        assert_eq!(m.bonus_of(109, 100, 500), -5);
        assert_eq!(m.bonus_of(27, 100, 500), 0);
        assert_eq!(m.bonus_of(108, 100, -3), 42, "a non-positive lock is ignored");
    }

    #[test]
    fn percent_map_multiplier_is_one_without_node() {
        let m = mods(&[], &[(110, 20), (111, -100)]);
        assert_eq!(m.multiplier(109), 1.0);
        assert_eq!(m.multiplier(110), 1.2);
        assert_eq!(m.multiplier(111), 1.0, "0.0 falls back to 1.0");
    }

    #[test]
    fn colour_rule_of_fun_100fde49() {
        assert_eq!(value_color(7, 7), 0xFFFFFF);
        assert_eq!(value_color(9, 7), 0x00FF00);
        assert_eq!(value_color(5, 7), 0xFF0000);
    }

    #[test]
    fn buffed_value_adds_ability_trickle_percent_and_bonus() {
        let t = SkillTables::test_trickle(152, [0, 0, 50, 0, 0, 50]);
        let ch = Character { breed: 1, profession: 1, level: 1, title_level: 1, abilities: [6; 6] };
        let none = Modifiers::default();
        assert_eq!(skill_base(&t, 152, 5, &ch, &none, 0), 6);
        assert_eq!(skill_value(&t, 152, 5, &ch, &none, 0), 6);
        // +8 Stamina and +8 Psychic raise the trickle to 0.25 * (50 * 14 / 100 * 2) = 3.5 -> 3; +4 direct bonus; +50 % of the stat
        let m = mods(&[(18, 8), (21, 8), (152, 4)], &[(152, 50)]);
        assert_eq!(buffed_abilities(ch.abilities, &m, 0), [6, 6, 14, 6, 6, 14]);
        assert_eq!(skill_base(&t, 152, 5, &ch, &m, 0), 7 + 3);
        assert_eq!(skill_value(&t, 152, 5, &ch, &m, 0), 7 + 3 + 4);
    }
}
