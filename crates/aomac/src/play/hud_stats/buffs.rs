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
//! Timed nano template events 4/0 are supplied by `TimedEffects` from the zone's authoritative active entries; removal drops their contribution.
//! Timed Life-bonus undo clamps stored Health between received messages (`HudStats::nano_stats_changed`, native `1006469b`), including refresh's intermediate undo.
//! **UNRESOLVED / not modelled**:
//! * Timed effects: `FUN_100a59f5` ends spells with a duration (`GetStat(0x19)`, function `0xcf16`) by itself; the server's undo `ApplySpellsIIR_t` is what removes
//!   a spell here.
//! * The delta of `0xcfb7` / `0xcff5` is fixed at apply time in the client (stored in the spell object for the undo); it is recomputed from the current
//!   level / value on every refresh here.
//! * `FUN_100644c8`'s minimum-Life bonus adjustment and `FUN_1006253c` ability stat-change signals are not ported. Health clamping on timed Life undo is implemented.

use super::super::zone::Zone;
use ao_formats::stats::buffs::Modifiers;
use ao_net::n3::spells::{stat, Spell};

/// Timed nano entries execute template events 4 and 0 (`GC 100512af`,
/// 10051487..100514ff); 1005064c undoes both on removal. These are independent
/// of FullCharacter/ApplySpells spell lists, just as in the native executor.
pub(in crate::play) struct TimedEffects {
    store: Option<ao_rdb::RecordStore>,
    templates: std::collections::HashMap<i32, Vec<Spell>>,
    signature: Vec<i32>,
    active: Vec<Spell>,
    effects: Vec<Spell>,
}

impl TimedEffects {
    pub(in crate::play) fn new(dir: &std::path::Path) -> Self {
        Self { store: ao_rdb::RecordStore::open(dir).ok(), templates: Default::default(),
            signature: Vec::new(), active: Vec::new(), effects: Vec::new() }
    }

    pub(in crate::play) fn refresh(&mut self, zone: &Zone) {
        if self.signature.iter().copied().eq(zone.nanos.buffs.iter().map(|b| b.nano))
            && self.active == zone.active_spells { return; }
        self.signature.clear();
        self.signature.extend(zone.nanos.buffs.iter().map(|b| b.nano));
        self.active.clone_from(&zone.active_spells);
        self.effects.clone_from(&self.active);
        for &nano in &self.signature {
            let spells = self.templates.entry(nano).or_insert_with(|| {
                let load = || -> anyhow::Result<Vec<Spell>> {
                    let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("nano record store unavailable"))?;
                    let record = store.get(super::super::hud_nanodb::NANO_RDB_TYPE, u32::try_from(nano)?)?
                        .ok_or_else(|| anyhow::anyhow!("nano template {nano} missing"))?;
                    let mut spells = super::super::chat::item_template_spells(&record, 4)?;
                    spells.extend(super::super::chat::item_template_spells(&record, 0)?);
                    spells.retain(|s| runs_on_character(s) && matches!(s.function, 0xcf14 | 0xcf35 | 0xcfb7 | 0xcff5 | 0xcfc0));
                    Ok(spells)
                };
                load().unwrap_or_else(|error| {
                    eprintln!("nano {nano}: timed modifiers unavailable: {error:#}");
                    Vec::new()
                })
            });
            self.effects.extend_from_slice(spells);
        }
    }

    /// Undo the old entry before a refresh/conflicting replacement adds its new one.
    /// Returns whether an undone bonus-map spell targeted Life (`1006469b`).
    pub(super) fn remove(&mut self, nano: i32) -> bool {
        let Some(index) = self.signature.iter().position(|&id| id == nano) else { return false };
        self.signature.remove(index);
        let life = self.templates.get(&nano).is_some_and(|spells| spells.iter().any(|s|
            s.stat(stat::STAT) == 1 && matches!(s.function, 0xcf14 | 0xcf35 | 0xcfb7 | 0xcff5)));
        self.effects.clone_from(&self.active);
        for id in &self.signature {
            if let Some(spells) = self.templates.get(id) { self.effects.extend_from_slice(spells); }
        }
        life
    }

    pub(in crate::play) fn effects(&self) -> &[Spell] { &self.effects }
}

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
    fn timed_ability_and_percent_modifiers_keep_info_base_cost_and_total_consistent() {
        use ao_formats::stats::{buffs, skills::{Character, SkillTables}};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let tables = SkillTables::load(&store).unwrap();
        let mut zone = Zone::new(7);
        zone.stats.extend([(4, 1), (54, 10), (60, 1), (37, 1), (152, 20)]);
        zone.stats.extend((16..=21).map(|id| (id, 20)));
        zone.nanos.buffs.push(super::super::super::own_nanos::Buff { nano: 1, ..Default::default() });
        let mut effects = TimedEffects::new(&dir);
        effects.templates.insert(1, vec![modify(0xcf35, 18, 12), modify(0xcfc0, 152, 50)]);
        effects.refresh(&zone);
        let character = Character::from_stats(|id| zone.stat(id));
        let current = |id, modifiers: &Modifiers| buffs::skill_value(&tables, id, zone.stat(id).unwrap_or(0), &character, modifiers, 0);
        let modifiers = modifiers_with_equipment(effects.effects(), &[], 10, &current);
        let info_base = buffs::skill_base(&tables, 152, 20, &character, &modifiers, 0);
        let unbuffed_base = buffs::skill_base(&tables, 152, 20, &character, &Modifiers::default(), 0);
        assert!(info_base > unbuffed_base, "timed stamina and percent buffs affect Base, not just Total");
        assert_ne!(tables.cost(152, info_base, &character), tables.cost(152, unbuffed_base, &character));
        let mut model = super::super::skill_model::Model::new(tables, Vec::new());
        model.refresh_buffs(&|id| zone.stat(id), effects.effects(), &[], None);
        let row = model.row(&|id| zone.stat(id), 152);
        model.publish(&mut zone);
        assert_eq!(info_base, row.base);
        assert_eq!(zone.skill_value(152), Some(row.value));
    }

    #[test]
    fn body_boost_timed_template_health_lifecycle() {
        use ao_net::{msg::Identity, n3::{action::simple, nano, world::World, N3}};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let own = Identity { kind: 0xc350, instance: 7 };
        let mut zone = Zone::new(7);
        zone.stats.extend([(1, 40), (27, 60)]);
        let mut timed = TimedEffects::new(&dir);
        let mut model = super::super::skill_model::Model::new(Default::default(), Vec::new());
        let add = N3::World(World::CharacterAction(simple(0x62, nano::nano(29091), Identity { kind: 7, instance: 180000 })));
        let publish = |timed: &mut TimedEffects, model: &mut super::super::skill_model::Model, zone: &mut Zone| {
            timed.refresh(zone);
            model.refresh_buffs(&|id| zone.stat(id), timed.effects(), &[], None);
            model.publish(zone);
        };
        zone.nanos.on_message(Identity { instance: 8, ..own }, own, &add, 100);
        publish(&mut timed, &mut model, &mut zone);
        assert_eq!(zone.skill_value(1), Some(40));
        for _ in 0..2 {
            zone.nanos.on_message(own, own, &add, 100);
            publish(&mut timed, &mut model, &mut zone);
            assert_eq!((zone.stat(1), zone.skill_value(1), zone.skill_value(27)), (Some(40), Some(60), Some(60)));
        }
        zone.nanos.tick(1801.0);
        publish(&mut timed, &mut model, &mut zone);
        assert_eq!(zone.skill_value(1), Some(60), "timer zero does not invent server removal");
        // The projection also consumes entries restored at login, without an immediate spell list.
        let mut restored = Zone::new(7);
        restored.stats.extend([(1, 40), (27, 60)]);
        restored.nanos.buffs.clone_from(&zone.nanos.buffs);
        publish(&mut timed, &mut model, &mut restored);
        assert_eq!(restored.skill_value(1), Some(60));
        // Native immediate and timed sources are independent, not value-deduplicated.
        restored.active_spells.push(modify(0xcf35, 1, 20));
        publish(&mut timed, &mut model, &mut restored);
        assert_eq!(restored.skill_value(1), Some(80));
        restored.active_spells.clear();
        let remove = N3::Misc(ao_net::n3::misc::Misc::Buff(ao_net::n3::misc::Buff {
            kind: 0, nano: Some(nano::nano(29091)), rest: Vec::new(),
        }));
        restored.nanos.on_message(own, own, &remove, 100);
        publish(&mut timed, &mut model, &mut restored);
        assert_eq!((restored.stat(1), restored.skill_value(1)), (Some(40), Some(40)));
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
