//! The buffs of the own character: the spells that run on it and the bonus / percent maps they fill (docs/gui.md §11.10 "Buffed values").
//!
//! The server sends the skill values unmodified (`StatIIR`); the client adds the bonuses itself (`ao_formats::stats::buffs`). The maps are filled by the spell
//! executor `FUN_100026d0` (stat object vtable +0x30) through the per-function handlers of `FUN_100a59f5` [GC]:
//!
//! | function | handler | effect on the maps (stat = `GetStat(0)`, amount = `GetStat(0x27)`) |
//! |---|---|---|
//! | `0xcf14`, `0xcf35` | `FUN_100a4c24` | bonus `+= amount` (`FUN_100644c8`; undo `FUN_1006469b` `-= amount`) |
//! | `0xcfb7` | `FUN_100a87b5` | bonus `+= trunc(level * amount / 200.0)`, level = `GetStat(0x36, 2)` |
//! | `0xcff5` | `FUN_100a88ee` | bonus `+= trunc(f32(amount / 100.0) * GetStat(stat, 0))` (percent of the current buffed value) |
//! | `0xcfc0` | `FUN_100a8a96` | percent `+= amount` (`FUN_1006497b`; undo `FUN_10064999`) |
//!
//! Every other function id has other effects (movement, damage, ...) and no entry in the two maps. Sources of the spells:
//! `FullCharacterIIR_t`'s spell list (run by `FUN_10073a2f` after the stats) and `ApplySpellsIIR_t` (apply flag 0 = undo). Spells whose `Target` (standard
//! argument 0x20) is 0xe or 0x17 run on the owner / pet relation of the character, not on it (`FUN_100026d0`), so they are not kept.
//!
//! Item Wear effects are supplied by `equipment.rs`: `FUN_100cba71` merges the low/high templates by QL; modifier functions interpolate argument 39 only.
//! **UNRESOLVED / not modelled**:
//! * Timed effects: `FUN_100a59f5` ends spells with a duration (`GetStat(0x19)`, function `0xcf16`) by itself; the server's undo `ApplySpellsIIR_t` is what removes
//!   a spell here.
//! * The delta of `0xcfb7` / `0xcff5` is fixed at apply time in the client (stored in the spell object for the undo); it is recomputed from the current
//!   level / value on every refresh here.
//! * `FUN_100644c8`'s side effects (a `Life` clamp for stat 1, `FUN_1006253c` stat-change events for ability changes) are not ported: they do not change the maps' values.

use super::super::zone::Zone;
use ao_formats::stats::buffs::Modifiers;
use ao_net::n3::spells::{stat, Spell};

/// Function ids of the spells that write the maps (see the module table).
const MODIFY_STAT: [u32; 2] = [0xCF14, 0xCF35];
const MODIFY_PER_LEVEL: u32 = 0xCFB7;
const MODIFY_PERCENT_OF_VALUE: u32 = 0xCFF5;
const MODIFY_PERCENT_MAP: u32 = 0xCFC0;

/// `Target` values that name the owner / pet relation of the character instead of the character (`FUN_100026d0`, target 0xe and 0x17).
fn runs_on_character(s: &Spell) -> bool {
    !matches!(s.stat(stat::TARGET), 0xe | 0x17)
}

impl Zone {
    /// `FullCharacterIIR_t`: the spell list of the message replaces what ran before.
    pub fn set_effects(&mut self, spells: &[Spell]) {
        self.active_spells = spells.iter().filter(|s| runs_on_character(s)).cloned().collect();
    }

    /// `ApplySpellsIIR_t` on the own character: apply adds the spells, undo removes the first equal spell of each.
    pub fn apply_effects(&mut self, spells: &[Spell], apply: bool) {
        for s in spells.iter().filter(|s| runs_on_character(s)) {
            if apply {
                self.active_spells.push(s.clone());
            } else if let Some(i) = self.active_spells.iter().position(|a| a == s) {
                self.active_spells.remove(i);
            }
        }
    }
}

/// The two maps after the spells ran in order. `level` is `GetStat(0x36, 2)`; `current(stat, maps)` is `GetStat(stat, 0)` (the buffed value) under the maps
/// built so far (`0xcff5` takes a percentage of it).
/// Wear and running spells share the same maps, so percent-of-current effects see preceding equipment bonuses.
pub fn modifiers_with_equipment(active: &[Spell], wear: &[Spell], level: i32, current: &dyn Fn(u32, &Modifiers) -> i32) -> Modifiers {
    let mut m = Modifiers::default();
    for s in wear.iter().chain(active) {
        let Ok(target) = u32::try_from(s.stat(stat::STAT)) else { continue };
        let amount = s.stat(stat::VALUE);
        match s.function {
            f if MODIFY_STAT.contains(&f) => *m.bonus.entry(target).or_default() += amount,
            MODIFY_PER_LEVEL => *m.bonus.entry(target).or_default() += (f64::from(amount) * f64::from(level) / 200.0) as i32,
            MODIFY_PERCENT_OF_VALUE => {
                let pct = (f64::from(amount) / 100.0) as f32;
                let delta = (f64::from(pct) * f64::from(current(target, &m))) as i32;
                *m.bonus.entry(target).or_default() += delta;
            }
            MODIFY_PERCENT_MAP => *m.percent.entry(target).or_default() += amount,
            _ => {}
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::spells::spell;

    fn modify(function: u32, stat_id: i32, amount: i32) -> Spell {
        spell(function, &[(stat::STAT, stat_id), (stat::VALUE, amount), (stat::TARGET, 2)])
    }

    #[test]
    fn writers_fill_the_bonus_and_percent_maps() {
        let spells = [modify(0xCF35, 108, 2), modify(0xCF14, 108, 40), modify(0xCF35, 101, 3), modify(0xCFC0, 110, 25), modify(0xCFB7, 109, 100), modify(0xCF22, 5, 5)];
        let m = modifiers_with_equipment(&spells, &[], 50, &|_, _| 0);
        assert_eq!(m.bonus.get(&108), Some(&42));
        assert_eq!(m.bonus.get(&101), Some(&3));
        assert_eq!(m.bonus.get(&109), Some(&25), "100 * 50 / 200");
        assert_eq!(m.percent.get(&110), Some(&25));
        assert_eq!(m.bonus.len() + m.percent.len(), 4, "0xcf22 is no writer");
    }

    #[test]
    fn percent_of_value_reads_the_buffed_value_so_far() {
        let spells = [modify(0xCF35, 108, 20), modify(0xCFF5, 108, 10)];
        // GetStat(108, 0) = 100 + the bonus built so far (20) = 120: 10 % -> +12
        let m = modifiers_with_equipment(&spells, &[], 1, &|stat, m| 100 + m.bonus.get(&stat).copied().unwrap_or(0));
        assert_eq!(m.bonus.get(&108), Some(&32));
        let m = modifiers_with_equipment(&[modify(0xCFF5, 108, 10)], &[modify(0xCF35, 108, 20)], 1, &|_, m| 100 + m.bonus.get(&108).copied().unwrap_or(0));
        assert_eq!(m.bonus[&108], 32);
    }

    #[test]
    fn apply_and_undo_track_the_running_spells() {
        let mut z = Zone::new(1);
        let a = modify(0xCF35, 108, 2);
        let owner = spell(0xCF35, &[(stat::STAT, 108), (stat::VALUE, 9), (stat::TARGET, 0xe)]);
        z.set_effects(&[a.clone(), owner.clone()]);
        assert_eq!(z.active_spells, std::slice::from_ref(&a), "an owner-relation spell does not run on the character");
        z.apply_effects(std::slice::from_ref(&a), true);
        assert_eq!(z.active_spells.len(), 2);
        z.apply_effects(std::slice::from_ref(&a), false);
        assert_eq!(z.active_spells, std::slice::from_ref(&a));
        z.apply_effects(&[a], false);
        z.apply_effects(&[modify(0xCF35, 1, 1)], false);
        assert!(z.active_spells.is_empty());
    }

    /// The spell words of two real Eye Implant records (rdb 1000020:101103 / :101110) as the body of an `ApplySpellsIIR_t` (size word, spells, target, apply flag),
    /// then the maps.
    #[test]
    fn real_item_record_spells_become_bonuses() {
        let words: [i32; 20] = [53045, 0, 4, 0, 1, 0, 2, 9, 108, 2, 53045, 0, 4, 0, 1, 0, 2, 9, 113, 105];
        let mut body = (3 * 0x3F1_i32).to_be_bytes().to_vec();
        body.extend(words.iter().flat_map(|w| w.to_be_bytes()));
        body.extend([0, 0, 0xC3, 0x50, 0, 0, 0, 7, 1]);
        let msg = ao_net::n3::spells::parse(&body).unwrap();
        assert_eq!((msg.target.instance, msg.apply), (7, true));
        let m = modifiers_with_equipment(&msg.spells, &[], 1, &|_, _| 0);
        assert_eq!((m.bonus[&108], m.bonus[&113]), (2, 105));
    }
}
