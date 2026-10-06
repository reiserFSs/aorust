//! Item-info combat cells: GUI 100318ac/10033395 -> GC 1001a954/10018d47.
use anyhow::Result;
use ao_formats::{dynel_visual::ItemTemplate, screens::TextDb, stats::INVALID};
use crate::play::zone::Zone;
use ao_net::n3::spells::Spell;

#[derive(Default)]
pub(super) struct Fields {
    pub damage: String,
    pub dps: String,
    pub radius: i32,
    pub duration: i32,
    pub nano_cost: i32,
    pub crystal: Option<u32>,
}

/// GC 1006772b/10067849: hundredths, current raw initiative and AggDef,
/// the post-1200 initiative slope, individual item caps, then the one-second floor.
fn delay(template: &ItemTemplate, stat: u32, initiative: i32, agg_def: i32, recharge: bool) -> f32 {
    let initiative = if initiative <= 1200 { initiative as f32 * 0.5 }
        else { (initiative - 1200) as f32 / 6.0 + 600.0 };
    let mut time = (template.stat(stat).unwrap_or(INVALID) as f32 + 75.0)
        - initiative * if recharge { 2.0 } else { 1.0 } / 3.0 - agg_def as f32;
    if let Some(cap) = template.stat(if recharge { 524 } else { 523 }).filter(|&v| v != INVALID) {
        time = time.max(cap as f32);
    }
    time.max(100.0)
}

pub(super) fn fields(template: &ItemTemplate, spells: &[Spell], zone: &Zone, _texts: &TextDb) -> Result<Fields> {
    let mut out = Fields::default();
    // GC 10026dd7 / 1004e3e1: own player's raw cost multiplier, breed floor.
    let multiplier = if zone.char_id == 0 { 0 } else {
        let value = zone.stats.get(&318).copied().unwrap_or(INVALID);
        // GC 100583eb/10058415: Flags bit 0x10000000 sets/clears actor+21c.
        if zone.stats.get(&0).is_some_and(|v| v & 0x10000000 != 0) { value }
        else { match zone.stat(4) { Some(1 | 2) => value.max(50), Some(3) => value.max(45), Some(4) => value.max(55), _ => value } }
    };
    // GUI 10035dc5 FILD/FIMUL/FDIV 100.0 -> _ftol (truncate, not ceil).
    out.nano_cost = (template.stat(407).unwrap_or(1) as f64 * multiplier as f64 / 100.0) as i32;
    if template.kind == 0xc76b {
        // GC 1001a954: first non-positive Health ModifyStat in the formula.
        if let Some(s) = spells.iter().find(|s| s.function == 0xcf0a && s.stat(0) == 27 && s.stat(2) <= 0 && s.stat(37) <= 0) {
            out.damage = format!("{}-{}", s.stat(2).wrapping_abs(), s.stat(37).wrapping_abs());
        }
        // GC 10085fd4: static programs have no runtime total-time override.
        out.duration = template.stat(8).unwrap_or(1000);
        // GC 100164f2 reads NanoItem+b8, initialized to zero by 10085bf8.
        // Static template deserialization (1008622a/1008070e) leaves it unchanged.
    } else {
        // GC 1001a15f mutates the inspected identity to the first cast-nano spell.
        out.crystal = spells.iter().find(|s| s.function == 0xcf1b).map(|s| s.stat(0) as u32);
        if template.stat(76) == Some(1) {
            if let Some(min) = template.stat(286).filter(|&v| v != INVALID) {
                let max = template.stat(285).unwrap_or(INVALID);
                let bonus = template.stat(284).unwrap_or(INVALID);
                out.damage = format!("{min}-{max}({bonus})");
                let init = template.stat(440).unwrap_or(INVALID);
                let attack_init = zone.stats.get(&(init as u32)).copied().unwrap_or(INVALID);
                let recharge_init = zone.stats.get(&(if (0..=1200).contains(&init) { init } else { 118 } as u32)).copied().unwrap_or(INVALID);
                let agg_def = zone.stats.get(&51).copied().unwrap_or(INVALID);
                let seconds = (delay(template, 294, attack_init, agg_def, false)
                    + delay(template, 210, recharge_init, agg_def, true)) * 0.01;
                out.dps = format!("{:.1}-{:.1}({:.1})", min as f32 / seconds, max as f32 / seconds, bonus as f32 / seconds);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[test]
fn initiative_slope_and_weapon_caps() {
    let mut t = ItemTemplate { kind: 0xc73d, stats: vec![(294, 500), (210, 500)], name: None, sounds: vec![] };
    assert_eq!(delay(&t, 294, 1200, 100, false), 275.0);
    assert!((delay(&t, 294, 1800, 100, false) - (475.0 - 700.0 / 3.0)).abs() < 0.0001);
    assert_eq!(delay(&t, 210, 1200, 100, true), 100.0);
    t.stats.push((524, 250));
    assert_eq!(delay(&t, 210, 1200, 100, true), 250.0);
}

#[cfg(test)]
#[test]
fn nano_damage_record_fixture() {
    let words: [i32; 19] = [0xc76b, 2, 15, 23, 1009, 2, 0, 2018,
        0xcf0a, 0, 4, 0, 1, 0, 3, -1, 27, -10, -20];
    let mut record: Vec<u8> = words.into_iter().flat_map(i32::to_le_bytes).collect();
    record.extend(90i32.to_le_bytes());
    let template = ItemTemplate { kind: 0xc76b, stats: vec![(8, 45000), (407, 11)], name: None, sounds: vec![] };
    let texts = TextDb::parse(b"MMDB\0\0\0\0".to_vec()).unwrap();
    let spells = super::template_spells::spells(&record, 0).unwrap();
    let mut zone = Zone::default();
    zone.char_id = 1;
    zone.stats.extend([(4, 3), (318, 10)]);
    let out = fields(&template, &spells, &zone, &texts).unwrap();
    assert_eq!(out.damage, "10-20");
    assert_eq!((out.duration, out.radius, out.crystal), (45000, 0, None));
    assert!(out.dps.is_empty());
    assert_eq!(out.nano_cost, 4, "nanomage 45 percent floor, truncated 4.95");
    zone.stats.insert(0, 0x10000000);
    assert_eq!(fields(&template, &spells, &zone, &texts).unwrap().nano_cost, 1, "NPC flag skips breed floor");
    assert!(super::template_spells::spells(&record[..record.len() - 1], 0).is_err());
}
