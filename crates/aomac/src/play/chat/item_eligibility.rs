//! Catalogue item wear eligibility: GC 10052904 -> item vtable 10162a24+5c
//! (10081815), criteria 100095a5/100065d8, final status 100805b5.
use anyhow::{bail, ensure, Result};
use crate::play::zone::Zone;
use ao_formats::dynel_visual::{parse_item_template, ItemTemplate};

#[derive(Default)]
pub(crate) struct PerkEquipment {
    worn: [Option<ao_net::n3::world::AcgItem>; 16],
    armor: [[i32; 3]; 16],
    len: usize,
}
impl PerkEquipment {
    pub(crate) fn armor(&self) -> &[[i32; 3]] { &self.armor[..self.len] }
    pub(crate) fn refresh(&mut self, store: &ao_rdb::RecordStore, zone: &Zone) -> Result<bool> {
        let worn = std::array::from_fn(|i| zone.inventory.get(&(i as u32 + 16)).map(|e| e.item));
        if self.worn == worn { return Ok(false); }
        let mut armor = [[0; 3]; 16];
        let mut len = 0;
        for item in worn.iter().flatten() {
            let low = ao_formats::dynel_visual::item_template(store, item.low_id as u32)?.ok_or_else(|| anyhow::anyhow!("missing equipped item {}", item.low_id))?;
            let high_id = if item.high_id == 0 { item.low_id } else { item.high_id };
            let template = if high_id == item.low_id { low } else {
                let high = ao_formats::dynel_visual::item_template(store, high_id as u32)?.ok_or_else(|| anyhow::anyhow!("missing equipped item {high_id}"))?;
                ao_formats::dynel_visual::interpolate_item_templates(&low, &high, item.level)?
            };
            armor[len] = [template.stat(0).unwrap_or(0), template.stat(30).unwrap_or(0), template.stat(702).unwrap_or(0)];
            len += 1;
        }
        self.worn = worn;
        self.armor = armor;
        self.len = len;
        Ok(true)
    }
}

pub(crate) struct PerkCriteria {
    item: ItemTemplate,
    wear: Vec<[i32; 3]>,
}
impl PerkCriteria {
    pub(crate) fn from_record(record: &[u8]) -> Result<Self> {
        let item = parse_item_template(record)?;
        let mut data = super::template_spells::data(record, u32::MAX)?;
        let wear = data.criteria.iter().rposition(|e| e.1 == 6).map(|i| data.criteria.swap_remove(i).2).unwrap_or_default();
        Ok(Self { item, wear })
    }
    /// GC 10052a24: profession enumeration is an any-match scan, not evaluation
    /// of the complete wear expression. No authored profession equality accepts all.
    pub(crate) fn profession_matches(&self, profession: i32) -> bool {
        let mut found = false;
        for &[stat, value, op] in &self.wear {
            if stat == 60 && op == 0 {
                found = true;
                if value == profession { return true; }
            }
        }
        !found
    }
    /// `run_flags` is SimpleChar+138, NOT catalogue Perk_t+0xc flags.
    /// Armor triples are cached equipped slots 16..32: [class stat0, Flags30, stat702].
    pub(crate) fn eligible_with_state(&self, run_flags: u32, zone: &Zone, armor: &[[i32; 3]], completed: &[u32]) -> Result<bool> {
        let flags = self.item.stat(30).unwrap_or(0) as u32;
        if flags & 4 == 0 { return Ok(false); }
        if run_flags & 2 != 0 && flags & 0x10000000 == 0 && !matches!(self.item.stat(76), Some(3 | 5)) { return Ok(false); }
        if !evaluate(&self.wear, zone, completed)? { return Ok(false); }
        // Native virtual+14 checks the class mask (stat0), not Flags30.
        if self.item.stat(0).unwrap_or(0) & 0x100 != 0 {
            let social = self.item.stat(702).unwrap_or(0);
            if armor.iter().any(|&[_, flags, group]| flags as u32 & 0x10000000 == 0 && group != social) { return Ok(false); }
        }
        // This is specifically the native status == 2 branch, never nonzero.
        Ok(true)
    }
    /// GC10080abc sets run bit2 for equipped item class0x100 on slots below0x30.
    pub(crate) fn eligible(&self, zone: &Zone, armor: &[[i32; 3]], completed: &[u32]) -> Result<bool> {
        completed_for_criteria(&self.wear, zone)?;
        let run_flags = if armor.iter().any(|c| c[0] & 0x100 != 0) { 2 } else { 0 };
        self.eligible_with_state(run_flags, zone, armor, completed)
    }
}

/// Owned inventory follows 1004959a: weapons use virtual+64/action8,
/// other classes virtual+5c/action6. Social skips the wear-status guards.
pub(crate) struct EquipCriteria {
    check: PerkCriteria,
    wield: Vec<[i32; 3]>,
    unwear: Vec<[i32; 3]>,
    unwield: Vec<[i32; 3]>,
}
impl EquipCriteria {
    pub(crate) fn stats(&self) -> &[(u32, i32)] { &self.check.item.stats }
    pub(crate) fn stat(&self, id: u32) -> Option<i32> { self.check.item.stat(id) }
    #[cfg(test)]
    pub(crate) fn test_stats(stats: &[(u32, i32)]) -> Self {
        Self { check: PerkCriteria { item: ItemTemplate { kind: 0xc73d, stats: stats.to_vec(), name: None, sounds: vec![] }, wear: vec![] }, wield: vec![], unwear: vec![], unwield: vec![] }
    }
    pub(crate) fn implant_access(&self, zone: &Zone) -> bool {
        self.check.item.stat(76) != Some(3) || zone.stat(224).unwrap_or(0) & 0x10 != 0
    }
    pub(crate) fn completion_state<'a>(&self, zone: &'a Zone, removing: bool) -> Result<&'a [u32]> {
        let weapon = self.check.item.stat(76) == Some(1);
        let criteria = match (removing, weapon) {
            (false, false) => &self.check.wear,
            (false, true) => &self.wield,
            (true, false) => &self.unwear,
            (true, true) => &self.unwield,
        };
        completed_for_criteria(criteria, zone)
    }
    pub(crate) fn load(store: &ao_rdb::RecordStore, acg: ao_net::n3::world::AcgItem) -> Result<Self> {
        let low = store.get(1_000_020, u32::try_from(acg.low_id)?)?.ok_or_else(|| anyhow::anyhow!("missing item {}", acg.low_id))?;
        let high_id = if acg.high_id == 0 { acg.low_id } else { acg.high_id };
        let high = if high_id == acg.low_id { None } else { Some(store.get(1_000_020, u32::try_from(high_id)?)?.ok_or_else(|| anyhow::anyhow!("missing item {high_id}"))?) };
        let high = high.as_deref().unwrap_or(&low);
        let a = parse_item_template(&low)?;
        let b = if std::ptr::eq(low.as_slice(), high) { None } else { Some(parse_item_template(high)?) };
        let b = b.as_ref().unwrap_or(&a);
        let item = ao_formats::dynel_visual::interpolate_item_templates(&a, b, acg.level)?;
        let mut data = super::interpolated_data(&low, high, acg.level, a.stat(54).unwrap_or(1), b.stat(54).unwrap_or(1))?;
        let mut list = |key| data.criteria.iter().rposition(|e| e.1 == key).map(|i| data.criteria.swap_remove(i).2).unwrap_or_default();
        Ok(Self { check: PerkCriteria { item, wear: list(6) }, wield: list(8), unwear: list(7), unwield: list(9) })
    }
    pub(crate) fn eligible(&self, zone: &Zone, armor: &[[i32; 3]], social: bool, completed: &[u32]) -> Result<bool> {
        let weapon = self.check.item.stat(76) == Some(1);
        if !weapon && self.check.item.stat(30).unwrap_or(0) & 4 == 0 { return Ok(false); }
        // FUN_10044b6e reads the own Features stat, not proximity to a clinic.
        if !self.implant_access(zone) { return Ok(false); }
        let criteria = if weapon { &self.wield } else { &self.check.wear };
        if weapon && social && !social_weapon_idle(zone) { return Ok(false); }
        if weapon || social { return evaluate(criteria, zone, completed); }
        self.check.eligible(zone, armor, completed)
    }

    /// GC 1004969a -> virtual+60 (100818ea) or +68 (10081a4a).
    pub(crate) fn removable(&self, zone: &Zone, social: bool, completed: &[u32]) -> Result<bool> {
        if !self.implant_access(zone) { return Ok(false); }
        if self.check.item.stat(76) == Some(1) {
            if social && !social_weapon_idle(zone) { return Ok(false); }
            evaluate(&self.unwield, zone, completed)
        } else {
            if self.check.item.stat(30).unwrap_or(0) & 4 == 0 || zone.stat(224).unwrap_or(0) & 0x800 != 0 { return Ok(false); }
            evaluate(&self.unwear, zone, completed)
        }
    }

    /// GC10068e28/10080541/10080568: both weapons must allow multiwield;
    /// select the existing opposite hand's melee/ranged requirement and use
    /// the smaller nonzero/non-invalid requirement against mode0/raw skill.
    pub(crate) fn dual_wield(&self, other: &Self, zone: &Zone) -> Result<bool> {
        let item = &self.check.item;
        let other = &other.check.item;
        if [item, other].iter().any(|t| matches!(t.stat(353), Some(2 | 3 | 6 | 8))) { return Ok(false); }
        let skill = if other.stat(420).unwrap_or(0) < 1 { 101 } else { 134 };
        let a = item.stat(skill).unwrap_or(1_234_567_890);
        let b = other.stat(skill).unwrap_or(1_234_567_890);
        if a == 0 || b == 0 || a == 1_234_567_890 || b == 1_234_567_890 { return Ok(false); }
        let actual = zone.stat(skill).ok_or_else(|| anyhow::anyhow!("missing raw multiwield skill {skill}"))?;
        Ok(actual >= a.min(b))
    }
}

fn social_weapon_idle(zone: &Zone) -> bool {
    !zone.fight_target.contains_key(&(zone.char_id as i32)) && zone.skill_value(410).unwrap_or(0) == 0
}

/// Unknown completion state is not an empty completed set for predicates93/103.
pub(crate) fn completed_for_criteria<'a>(criteria: &[[i32; 3]], zone: &'a Zone) -> Result<&'a [u32]> {
    if let Some(completed) = zone.completed_perks.as_deref() { return Ok(completed); }
    ensure!(!criteria.iter().any(|c| matches!(c[2], 93 | 103)), "completed-perk state unavailable for item criteria");
    Ok(&[])
}

pub(crate) fn evaluate(criteria: &[[i32; 3]], zone: &Zone, completed: &[u32]) -> Result<bool> {
    // GC uses a 100-byte postfix stack. Check before writing; do not reproduce
    // its unchecked overflow on malformed item records.
    let mut stack = [false; 100];
    let mut len = 0;
    for &[stat, value, op] in criteria {
        match op {
            19 => continue,
            3 | 4 => {
                ensure!(len >= 2, "criterion binary operator without operands");
                len -= 1;
                stack[len - 1] = if op == 3 { stack[len - 1] || stack[len] } else { stack[len - 1] && stack[len] };
                continue;
            }
            42 => {
                ensure!(len != 0, "criterion not without operand");
                stack[len - 1] = !stack[len - 1];
                continue;
            }
            _ => {}
        }
        let actual = zone.skill_value(stat as u32).unwrap_or(1_234_567_890);
        let result = match op {
            0 => actual == value,
            1 => actual < value,
            2 => actual > value,
            22 => actual & value == value,
            24 => actual != value,
            // GC100065d8 case0x42: signed packed-nibble comparison (pet count).
            66 => {
                let operand = value.unsigned_abs();
                let count = (actual as u32 >> (((operand / 1000) & 7) * 4)) & 15;
                (count < operand % 1000) != (value < 0)
            }
            107 => actual & value == 0,
            93 => completed.contains(&(value as u32)),
            103 => !completed.contains(&(value as u32)),
            // Native switch cases deliberately produce true, not unknown.
            45 | 73 | 74 | 98 | 104 | 108 | 109 | 115..=118 | 127 | 128 | 134..=136 => true,
            130 => actual == zone.skill_value(value as u32).unwrap_or(1_234_567_890),
            131 => actual < zone.skill_value(value as u32).unwrap_or(1_234_567_890),
            132 => actual > zone.skill_value(value as u32).unwrap_or(1_234_567_890),
            133 => actual != zone.skill_value(value as u32).unwrap_or(1_234_567_890),
            _ => bail!("unsupported native wear criterion [{stat}, {value}, {op}]"),
        };
        ensure!(len < stack.len(), "criterion stack exceeds native capacity");
        stack[len] = result;
        len += 1;
    }
    Ok(stack[..len].iter().all(|&v| v))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_completed_state_only_errors_for_completion_predicates() {
        let mut zone = Zone::new(1);
        assert!(completed_for_criteria(&[[54, 1, 2]], &zone).unwrap().is_empty());
        assert!(completed_for_criteria(&[[0, 42, 93]], &zone).is_err());
        assert!(completed_for_criteria(&[[0, 42, 103]], &zone).is_err());
        zone.completed_perks = Some(vec![]);
        assert!(!evaluate(&[[0, 42, 93]], &zone, completed_for_criteria(&[[0, 42, 93]], &zone).unwrap()).unwrap());
        assert!(evaluate(&[[0, 42, 103]], &zone, completed_for_criteria(&[[0, 42, 103]], &zone).unwrap()).unwrap());
        zone.completed_perks = Some(vec![42]);
        assert!(evaluate(&[[0, 42, 93]], &zone, completed_for_criteria(&[[0, 42, 93]], &zone).unwrap()).unwrap());
        assert!(!evaluate(&[[0, 42, 103]], &zone, completed_for_criteria(&[[0, 42, 103]], &zone).unwrap()).unwrap());
    }
    #[test]
    fn inventory_wear_checks_clinic_and_correct_action() {
        let mut zone = Zone::new(1);
        zone.stats.insert(124, 100);
        let item = ItemTemplate { kind: 0xc73d, stats: vec![(30, 4), (76, 3)], name: None, sounds: vec![] };
        let mut check = EquipCriteria { check: PerkCriteria { item, wear: vec![[124, 99, 2]] }, wield: vec![[124, 199, 2]], unwear: vec![], unwield: vec![] };
        assert!(!check.eligible(&zone, &[], false, &[]).unwrap(), "no clinic access");
        zone.stats.insert(224, 0x10);
        assert!(check.eligible(&zone, &[], false, &[]).unwrap());
        zone.stats.insert(124, 99);
        assert!(!check.eligible(&zone, &[], false, &[]).unwrap(), "strict greater requirement");
        check.check.item.stats[1].1 = 5;
        zone.stats.insert(224, 0);
        zone.stats.insert(124, 100);
        assert!(check.implant_access(&zone), "class5 does not require clinic");
        assert!(check.eligible(&zone, &[], false, &[]).unwrap());
        check.check.item.stats[1].1 = 1;
        assert!(!check.eligible(&zone, &[], false, &[]).unwrap(), "weapon uses Wield8, not Wear6");
        zone.stats.insert(124, 200);
        assert!(check.eligible(&zone, &[], false, &[]).unwrap());
        check.wield = vec![[0, 42, 93]];
        assert!(!check.eligible(&zone, &[], false, &[]).unwrap());
        assert!(check.eligible(&zone, &[], false, &[42]).unwrap());
        check.wield = vec![[0, 0, 144]];
        assert!(check.eligible(&zone, &[], false, &[]).is_err(), "unknown operators remain errors");
    }
    #[test]
    fn inventory_unwear_and_social_combat_use_native_streams() {
        let mut zone = Zone::new(1);
        zone.stats.extend([(124, 100), (224, 0x10)]);
        let item = ItemTemplate { kind: 0xc73d, stats: vec![(30, 4), (76, 3)], name: None, sounds: vec![] };
        let mut check = EquipCriteria { check: PerkCriteria { item, wear: vec![] }, wield: vec![], unwear: vec![[124, 100, 2]], unwield: vec![[124, 99, 2]] };
        assert!(!check.removable(&zone, false, &[]).unwrap(), "Unwear7, not Unwield9");
        zone.stats.insert(124, 101);
        assert!(check.removable(&zone, false, &[]).unwrap());
        zone.stats.insert(224, 0x810);
        assert!(!check.removable(&zone, false, &[]).unwrap(), "Features0x800 forbids unwear");
        check.check.item.stats[1].1 = 1;
        assert!(check.removable(&zone, false, &[]).unwrap(), "weapon uses Unwield9 and does not read Features0x800");
        zone.fight_target.insert(1, 2);
        assert!(!check.removable(&zone, true, &[]).unwrap());
        assert!(!check.eligible(&zone, &[], true, &[]).unwrap());
        assert!(check.eligible(&zone, &[], false, &[]).unwrap(), "ordinary weapons may equip while fighting");
        zone.fight_target.clear();
        zone.stats.insert(410, 1);
        assert!(!check.removable(&zone, true, &[]).unwrap());
        zone.stats.insert(410, 0);
        assert!(check.removable(&zone, true, &[]).unwrap());
    }

    #[test]
    fn inventory_dual_wield_uses_smaller_raw_requirement_and_animation_set() {
        let weapon = |requirement, mesh| EquipCriteria {
            check: PerkCriteria { item: ItemTemplate { kind: 0xc73d, stats: vec![(76, 1), (353, 1), (101, requirement), (134, requirement), (420, mesh)], name: None, sounds: vec![] }, wear: vec![] },
            wield: vec![], unwear: vec![], unwield: vec![],
        };
        let mut zone = Zone::new(1);
        zone.stats.extend([(101, 199), (134, 300)]);
        let moved = weapon(200, 0);
        let mut other = weapon(400, 0);
        assert!(!moved.dual_wield(&other, &zone).unwrap());
        zone.stats.insert(101, 200);
        assert!(moved.dual_wield(&other, &zone).unwrap(), "min(pair), inclusive raw threshold");
        other.check.item.stats[4].1 = 1;
        zone.stats.insert(101, 0);
        assert!(moved.dual_wield(&other, &zone).unwrap(), "opposite hand mesh selects MultiRanged134");
        other.check.item.stats[1].1 = 2;
        assert!(!moved.dual_wield(&other, &zone).unwrap(), "two-handed animation rejects pair");
        other.check.item.stats[1].1 = 1;
        other.check.item.stats[3].1 = 0;
        assert!(!moved.dual_wield(&other, &zone).unwrap(), "zero requirement is not a free pass");
        other.check.item.stats[3].1 = 400;
        zone.stats.remove(&134);
        assert!(moved.dual_wield(&other, &zone).is_err(), "missing raw skill is not an invented pass");
    }
    #[test]
    fn social_armor_still_requires_native_wear_capability() {
        let mut check = EquipCriteria::test_stats(&[(76, 2), (30, 0)]);
        let zone = Zone::new(1);
        assert!(!check.eligible(&zone, &[], true, &[]).unwrap());
        check.check.item.stats[1].1 = 4;
        assert!(check.eligible(&zone, &[], true, &[]).unwrap());
    }
    #[test]
    fn native_postfix_and_status_guards() {
        let mut zone = Zone::new(1);
        zone.stats.extend([(54, 20), (60, 1), (389, 24)]);
        let item = ItemTemplate { kind: 0xc73d, stats: vec![(30, 4), (76, 1)], name: None, sounds: vec![] };
        let mut check = PerkCriteria { item, wear: vec![[54, 19, 2], [60, 2, 0], [60, 1, 0], [0, 0, 3], [0, 0, 4], [389, 16, 22]] };
        assert!(check.profession_matches(1));
        assert!(!check.profession_matches(3));
        assert!(check.eligible(&zone, &[], &[]).unwrap());
        assert!(!check.eligible(&zone, &[[0x100, 4, 1]], &[]).unwrap());
        check.item.stats.push((0, 0x100));
        assert!(!check.eligible_with_state(0, &zone, &[[0x100, 4, 1]], &[]).unwrap());
        assert!(check.eligible_with_state(0, &zone, &[[0x100, 0x10000004, 1]], &[]).unwrap());
        check.wear = vec![[0, 0, 3]];
        assert!(check.eligible(&zone, &[], &[]).is_err());
    }
    #[test]
    fn native_packed_pet_count_criterion() {
        let mut zone = Zone::new(1);
        zone.stats.insert(251, 0x21);
        assert!(!evaluate(&[[251, 1, 66]], &zone, &[]).unwrap());
        assert!(evaluate(&[[251, -1, 66]], &zone, &[]).unwrap());
        assert!(evaluate(&[[251, 1003, 66]], &zone, &[]).unwrap());
        assert!(!evaluate(&[[251, 1002, 66]], &zone, &[]).unwrap());
    }
    #[test]
    fn native_catalogue_wear_criteria() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        // Catalogue perk161 -> item212164: profession14, level>29, completed160.
        let raw = store.get(1_000_020, 212164).unwrap().unwrap();
        let check = PerkCriteria::from_record(&raw).unwrap();
        let mut zone = Zone::new(1);
        zone.stats.extend([(60, 14), (54, 29)]);
        assert!(check.profession_matches(14));
        assert!(!check.profession_matches(1));
        assert!(!check.eligible_with_state(0, &zone, &[], &[160]).unwrap());
        zone.stats.insert(54, 30);
        assert!(!check.eligible_with_state(0, &zone, &[], &[]).unwrap());
        assert!(check.eligible_with_state(0, &zone, &[], &[160]).unwrap());
        // Every catalogue wear stream must be supported, including dormant rows.
        for id in store.ids(1_000_036).unwrap() {
            let def = store.get(1_000_036, id).unwrap().unwrap();
            let item = u32::from_le_bytes(def[..4].try_into().unwrap());
            if let Some(raw) = store.get(1_000_020, item).unwrap() {
                let check = PerkCriteria::from_record(&raw).unwrap_or_else(|e| panic!("perk{id} item{item}: {e}"));
                evaluate(&check.wear, &zone, &[]).unwrap_or_else(|e| panic!("perk{id} item{item}: {e}"));
            }
        }
    }
}
