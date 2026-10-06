//! Worn item templates: Wear (2,14), SkillPst (0x13,0xc), and QL interpolation.
//! GC FUN_100cba71 merges matching function/stat spells; FUN_100cb3b2 rounds
//! from the smaller value's endpoint, adding 0.5 before integer truncation.
use super::super::zone::Zone;
use ao_formats::dynel_visual::parse_item_template;
use ao_net::{n3::{spells::{read_spell, stat, Spell}, world::AcgItem}, Reader};
use ao_rdb::RecordStore;
use anyhow::{ensure, Result};
use std::{collections::HashMap, path::Path};

#[derive(Default)]
struct Template {
    ql: i32,
    weapon: bool,
    wear: Vec<Spell>,
    attack: Vec<(u32, i32)>,
}

fn count(r: &mut Reader, stride: usize) -> Result<usize> {
    let w = r.u32()?;
    ensure!(w >= 0x3f1 && w % 0x3f1 == 0, "invalid item list size {w:#x}");
    let n = (w / 0x3f1 - 1) as usize;
    ensure!(n <= r.remaining() / stride, "item list exceeds record");
    Ok(n)
}

fn parse(rec: &[u8]) -> Result<Template> {
    let item = parse_item_template(rec)?;
    let mut t = Template { ql: item.stat(54).unwrap_or(0), weapon: item.stat(76) == Some(1), ..Default::default() };
    // Like dynel_visual::sound_map: preceding element readers are not all ported.
    // Accept only complete, validated payloads, and reject ambiguous matches.
    let mut found_wear = false;
    let mut found_attack = false;
    for at in 8..rec.len().saturating_sub(12) {
        let header = &rec[at..at + 8];
        let ty = u32::from_le_bytes(header[..4].try_into().unwrap());
        let sub = u32::from_le_bytes(header[4..].try_into().unwrap());
        if (ty, sub) == (2, 14) {
            let mut r = Reader::little_endian(&rec[at + 8..]);
            let candidate = count(&mut r, 16).and_then(|n| (0..n).map(|_| read_spell(&mut r)).collect::<Result<Vec<_>>>());
            if let Ok(v) = candidate {
                ensure!(!found_wear, "ambiguous Wear element");
                found_wear = true;
                t.wear = v.into_iter().filter(|s| !matches!(s.stat(stat::TARGET), 0xe | 0x17)).collect();
            }
        } else if (ty, sub) == (0x13, 0xc) {
            let mut r = Reader::little_endian(&rec[at + 8..]);
            let candidate = count(&mut r, 8).and_then(|n| (0..n).map(|_| Ok((r.u32()?, r.i32()?))).collect::<Result<Vec<_>>>());
            if let Ok(v) = candidate {
                ensure!(!found_attack, "ambiguous SkillPst element");
                found_attack = true;
                t.attack = v;
            }
        }
    }
    Ok(t)
}

fn interpolate(ql: i32, low_ql: i32, high_ql: i32, low: i32, high: i32) -> i32 {
    if low == high { return low; }
    let (q0, q1, v0, v1) = if low < high { (low_ql, high_ql, low, high) } else { (high_ql, low_ql, high, low) };
    let delta = ((f64::from(ql) - f64::from(q0)) * (f64::from(v1) - f64::from(v0)) / (f64::from(q1) - f64::from(q0)) + 0.5) as i32;
    v0.wrapping_add(delta)
}

fn merge(low: &Template, high: &Template, ql: i32) -> Result<Template> {
    ensure!((1..=511).contains(&low.ql) && (1..=511).contains(&high.ql) && low.ql != high.ql, "invalid template QL endpoints");
    ensure!(low.wear.len() == high.wear.len() && low.attack.len() == high.attack.len(), "item endpoint list sizes differ");
    let mut wear = low.wear.clone();
    for s in &mut wear {
        if matches!(s.function, 0xcf14 | 0xcf35 | 0xcfb7 | 0xcff5) {
            // FUN_100cba71 searches by function and GetStat(0), not list index.
            let h = high.wear.iter().find(|h| h.function == s.function && h.stat(stat::STAT) == s.stat(stat::STAT)).unwrap_or(&*s);
            let value = interpolate(ql, low.ql, high.ql, s.stat(stat::VALUE), h.stat(stat::VALUE));
            s.stats.insert(stat::VALUE, value);
        }
    }
    let mut attack = low.attack.clone();
    // FUN_100cb801 requires equal pair keys in file order.
    for ((id, value), (high_id, h)) in attack.iter_mut().zip(&high.attack) {
        ensure!(*id == *high_id, "item endpoint SkillPst keys differ");
        *value = interpolate(ql, low.ql, high.ql, *value, *h);
    }
    Ok(Template { ql, weapon: low.weapon, wear, attack })
}

pub struct Equipment {
    store: Option<RecordStore>,
    templates: HashMap<i32, Option<Template>>,
    items: HashMap<(i32, i32, i32), Option<Template>>,
    worn: Vec<(u32, AcgItem)>,
    effects: Vec<Spell>,
    attack: Option<Vec<(u32, i32)>>,
}

impl Equipment {
    pub fn new(dir: &Path) -> Self {
        Self { store: RecordStore::open(dir).ok(), templates: HashMap::new(), items: HashMap::new(), worn: Vec::new(), effects: Vec::new(), attack: None }
    }

    fn load(&mut self, item: AcgItem) {
        let high_id = if item.high_id == 0 { item.low_id } else { item.high_id };
        let key = (item.low_id, high_id, item.level);
        if self.items.contains_key(&key) { return; }
        for id in [item.low_id, high_id] {
            self.templates.entry(id).or_insert_with(|| {
                let rec = self.store.as_ref()?.get(1000020, u32::try_from(id).ok()?).ok()??;
                parse(&rec).ok()
            });
        }
        let result = self.templates[&item.low_id].as_ref().and_then(|lo| {
            if item.low_id == high_id {
                Some(Template { ql: item.level, weapon: lo.weapon, wear: lo.wear.clone(), attack: lo.attack.clone() })
            } else {
                merge(lo, self.templates[&high_id].as_ref()?, item.level).ok()
            }
        });
        self.items.insert(key, result);
    }

    pub fn refresh(&mut self, zone: &Zone) {
        let changed = self.worn.len() != zone.inventory.keys().filter(|&&s| s < 0x30).count()
            || self.worn.iter().any(|(slot, item)| zone.inventory.get(slot).map(|e| e.item) != Some(*item));
        if !changed { return; }
        self.worn.clear();
        self.worn.extend(zone.inventory.iter().filter(|(s, _)| **s < 0x30).map(|(&s, e)| (s, e.item)));
        self.worn.sort_unstable_by_key(|e| e.0);
        for i in 0..self.worn.len() { self.load(self.worn[i].1); }
        self.effects.clear();
        self.attack = None;
        for &(slot, item) in &self.worn {
            let high = if item.high_id == 0 { item.low_id } else { item.high_id };
            if let Some(t) = self.items[&(item.low_id, high, item.level)].as_ref() {
                self.effects.extend(t.wear.iter().cloned());
                if matches!(slot, 6 | 8) && self.attack.is_none() && t.weapon { self.attack = Some(t.attack.clone()); }
            }
        }
    }

    pub fn effects(&self) -> &[Spell] { &self.effects }
    pub fn attack_weights(&self) -> Option<&[(u32, i32)]> { self.attack.as_deref() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interpolation_rounds_from_the_minimum_endpoint_and_matches_stats() {
        assert_eq!(interpolate(2, 1, 3, 0, 1), 1);
        assert_eq!(interpolate(2, 1, 3, 1, 0), 1);
        assert_eq!(interpolate(2, 1, 3, -10, -9), -9);
        let spell = |id, value| ao_net::n3::spells::spell(0xcf35, &[(stat::STAT, id), (stat::VALUE, value)]);
        let low = Template { ql: 1, wear: vec![spell(108, 2), spell(113, 4)], attack: vec![(108, 100)], ..Default::default() };
        let high = Template { ql: 3, wear: vec![spell(113, 8), spell(108, 6)], attack: vec![(108, 100)], ..Default::default() };
        let t = merge(&low, &high, 2).unwrap();
        assert_eq!((t.wear[0].stat(stat::VALUE), t.wear[1].stat(stat::VALUE)), (4, 6));
        assert_eq!(t.attack, [(108, 100)]);
        let mut lo = low;
        let mut hi = high;
        lo.wear.push(ao_net::n3::spells::spell(0xcfc0, &[(stat::STAT, 108), (stat::VALUE, 10)]));
        hi.wear.push(ao_net::n3::spells::spell(0xcfc0, &[(stat::STAT, 108), (stat::VALUE, 90)]));
        assert_eq!(merge(&lo, &hi, 2).unwrap().wear[2].stat(stat::VALUE), 10, "CFC0 has no interpolated arguments");
        assert!(merge(&lo, &lo, 2).is_err());
        assert!(count(&mut Reader::little_endian(&0u32.to_le_bytes()), 8).is_err());
        let words: [u32; 23] = [0xc74a, 3, 15, 23, 2 * 0x3f1, 54, 1, 2, 14, 2 * 0x3f1, 0xcf35, 0, 4, 0, 1, 0, 2, 9, 108, 2, 0x13, 0xc, 2 * 0x3f1];
        let mut bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        bytes.extend([108u32, 100].into_iter().flat_map(u32::to_le_bytes));
        let t = parse(&bytes).unwrap();
        assert_eq!(t.wear[0].stat(stat::VALUE), 2);
        assert_eq!(t.attack, [(108, 100)]);
    }
}
