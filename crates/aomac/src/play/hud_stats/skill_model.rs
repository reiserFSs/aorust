//! IP bookkeeping of the skill window without any UI: the `StatRow` arithmetic (`FUN_100fde49`, `FUN_100f90ef`, `FUN_100f908e`, `FUN_100fe478`),
//! the window's IP slot (`FUN_100fa185`) and the "Suggested IP distribution" spend loop (`FUN_100fac49`). Evidence: docs/gui.md §11.8 / §11.10.

use ao_formats::stats::{self, buffs::{self, Modifiers}, skills::{Character, SkillTables}, DistProfession};
use std::collections::{BTreeMap, HashMap};

/// Own stat lookup (`Zone::stat`).
pub type Get<'a> = &'a dyn Fn(u32) -> Option<i32>;

/// The two deprecated skills of group 10 ("disabled"): `StatRow` flag `+0x1cc` (ctor argument `group == 10`), no increase or decrease.
pub fn is_disabled(stat: u32) -> bool {
    stats::SKILL_GROUPS[10].stats.contains(&(stat as u16))
}

/// What a row shows (`StatRow` fields `+0x1a8 value`, `+0x1ac raw`, `+0x1b0 pending`, `+0x1b4 max`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// `GetSkill(stat, 0)`: the stored value.
    pub raw: i32,
    pub pending: i32,
    /// `GetSkill(stat, 1)`: the stat with its percent boost plus the trickle-down of the buffed abilities including pending ability points (`SetSkillTmp`).
    pub base: i32,
    /// `GetSkill(stat, 2)` (`StatRow+0x1a8`): [`Row::base`] plus the bonus of the active spells (docs §11.10 "Buffed values").
    pub value: i32,
    /// `GetSkillMax`.
    pub max: i32,
}

impl Row {
    /// Text of the value view: `FUN_100fde49` prints `pending + value`.
    pub fn shown(&self) -> i32 {
        self.value + self.pending
    }
    /// Colour of the value text (`FUN_100fde49`: `GetSkill(stat, 2)` against `GetSkill(stat, 1)`).
    pub fn color(&self) -> u32 {
        buffs::value_color(self.value, self.base)
    }
    /// `PowerbarView` value `(raw + pending) / max` (float division, the original does not clamp or guard max = 0).
    pub fn fraction(&self) -> f32 {
        let f = (self.raw + self.pending) as f32 / self.max as f32;
        if f.is_nan() { 0.0 } else { f.clamp(0.0, 1.0) }
    }
}

/// Outcome of [`Model::adjust`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Adjusted {
    /// Points actually added (negative: removed).
    pub applied: i32,
    /// An increase of exactly one point was refused for lack of IP (`Skill_LackIP`, cat 502).
    pub lack_ip: bool,
}

#[derive(Default)]
pub struct Model {
    pub tables: SkillTables,
    pub dist: Vec<DistProfession>,
    pending: HashMap<u32, i32>,
    /// `StatRow+0x1b8`: the IP price of the pending points as last reported to the window.
    cost: HashMap<u32, i32>,
    /// The bonus / percent maps of the active spells (`SimpleChar+0x1bc`), see [`Model::refresh_buffs`].
    mods: Modifiers,
    /// AMS raw value rebuilt from the equipped weapon's attack list (`FUN_10068359`).
    ams: i32,
}

impl Model {
    pub fn new(tables: SkillTables, dist: Vec<DistProfession>) -> Self {
        Self { tables, dist, ..Default::default() }
    }

    pub fn pending(&self, stat: u32) -> i32 {
        self.pending.get(&stat).copied().unwrap_or(0)
    }

    /// `FUN_100f91d7` + the rows' reset: forget every pending point.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.cost.clear();
    }

    /// The character the formulas read; pending ability points are `N3Msg_SetSkillTmp` (`FUN_100fde49`, stats 0x10..=0x15 while the row is visible).
    fn character(&self, get: Get) -> Character {
        let mut ch = Character::from_stats(get);
        for (a, id) in ch.abilities.iter_mut().zip(stats::skills::ABILITIES) {
            *a += self.pending(id);
        }
        ch
    }

    /// IP the window offers: the server's IP (stat 0x35) minus the price of every pending row (`FUN_100fa7ba` keeps `remaining = new - (old - remaining)`).
    pub fn remaining(&self, get: Get) -> i32 {
        get(stats::IP).unwrap_or(0) - self.cost.values().sum::<i32>()
    }

    /// Rebuilds the modifier maps from the spells running on the own character (`hud_stats/buffs.rs`); the window calls it every update.
    pub fn refresh_buffs(&mut self, get: Get, active: &[ao_net::n3::spells::Spell], wear: &[ao_net::n3::spells::Spell], attack: Option<&[(u32, i32)]>) {
        let (ch, lock) = (Character::from_stats(get), get(buffs::LOCK_STAT).unwrap_or(0));
        let tables = &self.tables;
        let current = |stat: u32, m: &Modifiers| buffs::skill_value(tables, stat, get(stat).unwrap_or(0), &ch, m, lock);
        self.mods = super::buffs::modifiers_with_equipment(active, wear, get(stats::LEVEL).unwrap_or(0), &current);
        // `FUN_10081c67` truncates each weighted term, then adds AMSModifier only for a positive armed total.
        self.ams = match attack {
            None => self.value(get, 100) + self.value(get, 276),
            Some(weights) => {
                let sum: i32 = weights.iter().map(|&(skill, weight)| self.value(get, skill) * weight / 100).sum();
                if sum > 0 { sum + self.value(get, 276) } else { sum }
            }
        };
    }

    /// `GetSkill(stat, 2)`, without the Skills window's pending ability points (which belong to mode 4).
    pub fn value(&self, get: Get, stat: u32) -> i32 {
        let raw = if stat == 22 { self.ams } else { get(stat).unwrap_or(0) };
        buffs::skill_value(&self.tables, stat, raw, &Character::from_stats(get), &self.mods, get(buffs::LOCK_STAT).unwrap_or(0))
    }

    /// Reuses the map allocation; raw values remain untouched so trickle-down is never added twice.
    pub fn publish(&self, zone: &mut super::super::zone::Zone) {
        let mut values = std::mem::take(&mut zone.skill_values);
        values.clear();
        let get = |id| zone.stat(id);
        let ch = Character::from_stats(get);
        let lock = get(buffs::LOCK_STAT).unwrap_or(0);
        for id in zone.stats.keys().chain(self.mods.bonus.keys()).chain(self.mods.percent.keys()).copied().chain([22]) {
            if values.contains_key(&id) { continue; }
            let raw = if id == 22 { self.ams } else { get(id).unwrap_or(0) };
            values.insert(id, buffs::skill_value(&self.tables, id, raw, &ch, &self.mods, lock));
        }
        zone.skill_values = values;
    }

    pub fn row(&self, get: Get, stat: u32) -> Row {
        let ch = self.character(get);
        let (raw, lock) = (get(stat).unwrap_or(0), get(buffs::LOCK_STAT).unwrap_or(0));
        Row {
            raw,
            pending: self.pending(stat),
            base: buffs::skill_base(&self.tables, stat, raw, &ch, &self.mods, lock),
            value: buffs::skill_value(&self.tables, stat, raw, &ch, &self.mods, lock),
            max: self.tables.skill_max(stat, &ch),
        }
    }

    /// `FUN_100f908e`: one more point is allowed (not disabled, `raw + pending + 1 <= max`).
    pub fn can_increase(&self, get: Get, stat: u32) -> bool {
        let r = self.row(get, stat);
        !is_disabled(stat) && r.pending + 1 + r.raw <= r.max
    }

    /// `FUN_100fd5b9` is the refusal at the maximum (feedback text UNRESOLVED, docs §11.10); `true` when [`Model::adjust`] would be called.
    pub fn at_max(&self, get: Get, stat: u32) -> bool {
        let r = self.row(get, stat);
        r.max < r.pending + 1 + r.raw
    }

    /// The IP slot `FUN_100fa185`: `remaining - price` must stay above zero (`0 < remaining`), otherwise nothing changes.
    fn spend(&self, get: Get, price: i32) -> bool {
        self.remaining(get) - price > 0
    }

    /// `FUN_100fde49(delta)`: moves the pending points of `stat` and re-prices them (`Σ trunc(GetSkillCost(raw + i))`). An increase the IP do not
    /// cover is cut back point by point (one point: refused with the `Skill_LackIP` feedback).
    pub fn adjust(&mut self, get: Get, stat: u32, delta: i32) -> Adjusted {
        let ch = self.character(get);
        let raw = get(stat).unwrap_or(0);
        let last = self.cost.get(&stat).copied().unwrap_or(0);
        let start = self.pending(stat);
        let mut pending = start + delta;
        let mut cost = self.tables.cost_of_points(stat, raw, pending, &ch);
        let mut lack_ip = false;
        if delta != 0 && !self.spend(get, cost - last) {
            if delta == 1 {
                pending -= 1;
                lack_ip = true;
            } else {
                let mut d = delta;
                loop {
                    d -= 1;
                    pending -= 1;
                    cost = self.tables.cost_of_points(stat, raw, pending, &ch);
                    if self.spend(get, cost - last) || d == 0 {
                        break;
                    }
                }
            }
            cost = self.tables.cost_of_points(stat, raw, pending, &ch);
        }
        self.pending.insert(stat, pending);
        self.cost.insert(stat, cost);
        Adjusted { applied: pending - start, lack_ip }
    }

    /// `FUN_100fe478`: after the maximum shrank (an ability was lowered), take the surplus pending points back.
    pub fn clamp(&mut self, get: Get, stat: u32) {
        let r = self.row(get, stat);
        if r.raw + r.pending > r.max {
            self.adjust(get, stat, r.max - r.raw - r.pending);
        }
    }

    /// `FUN_100fa5dd`: `stat -> pending + raw` of every changed row (the payload of `SkillIIR_t`).
    pub fn save_map(&self, get: Get) -> BTreeMap<i32, i32> {
        self.pending.iter().filter(|(_, &p)| p != 0).map(|(&s, &p)| (s as i32, p + get(s).unwrap_or(0))).collect()
    }

    /// `FUN_100fac49` (button `suggest_ip`): rows grouped by their `ipdist.xml` priority for (level, profession), ascending, pri 0 never; each
    /// bucket gets one point per row round-robin (rows that hit their maximum drop out) until a one-point increase is refused for lack of IP,
    /// which ends the whole distribution. Returns the stats raised, in the order they were first raised.
    pub fn suggest(&mut self, get: Get, profession: &str, name_of: impl Fn(u32) -> String) -> Vec<u32> {
        let level = get(stats::LEVEL).unwrap_or(0);
        let all: Vec<u32> = stats::SKILL_GROUPS.iter().flat_map(|g| g.stats.iter().map(|&s| s as u32)).collect();
        let pri = stats::skills::ip_priorities(&self.dist, profession, level, name_of, &all);
        let mut buckets: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (s, p) in pri {
            buckets.entry(p).or_default().push(s);
        }
        let mut raised = vec![];
        'all: for rows in buckets.values_mut() {
            let mut i = 0;
            while !rows.is_empty() {
                if i >= rows.len() {
                    i = 0;
                }
                let s = rows[i];
                self.clamp(get, s);
                if !self.can_increase(get, s) {
                    rows.remove(i);
                    continue;
                }
                if self.adjust(get, s, 1).applied == 0 {
                    break 'all;
                }
                if !raised.contains(&s) {
                    raised.push(s);
                }
                i += 1;
            }
        }
        raised
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats_of(pairs: &[(u32, i32)]) -> impl Fn(u32) -> Option<i32> {
        let m: HashMap<u32, i32> = pairs.iter().copied().collect();
        move |s| m.get(&s).copied()
    }

    /// Hand-made tables: a single skill (stat 152) costing `factor / 10 * n` IP, maximum 3 raw points at level 1.
    fn tiny() -> Model {
        Model::new(SkillTables::default(), vec![])
    }

    #[test]
    fn attack_rating_rebuilds_from_buffed_skills_and_weapon_weights() {
        let get = stats_of(&[(22, 999), (100, 11), (112, 17), (276, 3)]);
        let mut m = tiny();
        let wear = [ao_net::n3::spells::spell(0xcf35, &[(0, 100), (0x27, 4)])];
        m.refresh_buffs(&get, &[], &wear, None);
        assert_eq!(m.value(&get, 22), 18, "unarmed: Martial Arts + wear bonus + AMSModifier");
        m.refresh_buffs(&get, &[], &wear, Some(&[(100, 33), (112, 67)]));
        assert_eq!(m.value(&get, 22), 18, "truncate each weighted term: 4 + 11 + 3");
        m.refresh_buffs(&get, &[], &[], Some(&[]));
        assert_eq!(m.value(&get, 22), 0, "an armed empty attack list is not Martial Arts");
    }

    #[test]
    fn published_values_refresh_without_overwriting_raw_stats() {
        let mut z = super::super::super::zone::Zone::new(42);
        z.stats.insert(91, 10);
        let active = [ao_net::n3::spells::spell(0xcf35, &[(0, 91), (0x27, 123)])];
        let mut m = tiny();
        m.refresh_buffs(&|id| z.stat(id), &active, &[], None);
        m.publish(&mut z);
        assert_eq!((z.stat(91), z.skill_value(91)), (Some(10), Some(133)));
        z.stats.insert(91, 20);
        m.refresh_buffs(&|id| z.stat(id), &active, &[], None);
        m.publish(&mut z);
        assert_eq!((z.stat(91), z.skill_value(91)), (Some(20), Some(143)));
    }

    #[test]
    fn increase_is_cut_by_the_ip_slot_and_refused_for_one_point() {
        // an empty table set: profession -1 -> every price is 1 IP (`GetSkillCost` without profession), max 0 -> use abilities with no profession
        let get = stats_of(&[(stats::IP, 3), (stats::PROFESSION, -1), (16, 5)]);
        let mut m = tiny();
        // 3 IP: points 1 and 2 cost 1 each (remaining 3 -> 1), the third would leave 0 (the slot needs `> 0`)
        assert_eq!(m.adjust(&get, 16, 3), Adjusted { applied: 2, lack_ip: false });
        assert_eq!(m.remaining(&get), 1);
        assert_eq!(m.adjust(&get, 16, 1), Adjusted { applied: 0, lack_ip: true });
        assert_eq!(m.pending(16), 2);
        // giving points back
        assert_eq!(m.adjust(&get, 16, -1).applied, -1);
        assert_eq!(m.remaining(&get), 2);
        assert_eq!(m.save_map(&get), BTreeMap::from([(16, 5 + 1)]));
        m.clear();
        assert_eq!((m.remaining(&get), m.pending(16)), (3, 0));
    }
}
