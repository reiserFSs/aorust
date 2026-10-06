//! Gamecode requirement text: 1001a03d/1001a20a/1001a3eb/1001a4fe/10022f27.
//! RDB type 4 feeds DummyItemBase +0x58 (1002b3ef -> 1002e21e), keyed by
//! its inner key, NOT the element subtype. Type 19 is discarded (1002b663).
//! Type 22 similarly feeds +0x6c (1002b6ad -> 1008a007 -> 10081149).
use anyhow::Result;
use ao_formats::{dynel_visual::ItemTemplate, screens::TextDb};
use ao_net::msg::Identity;
use ao_rdb::RecordStore;
use crate::play::zone::Zone;
use super::template_spells::Data;
use std::fmt::Write;

#[derive(Default)]
pub(super) struct Sections {
    pub useby: String,
    pub location: String,
    pub attack: String,
    pub defend: String,
    pub use_skill: String,
    pub overequip: i32,
}

fn pairs(data: &Data, key: u32) -> &[(u32, i32)] {
    data.pairs.iter().rev().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.as_slice()).unwrap_or_default()
}
fn criteria(data: &Data, key: u32) -> &[[i32; 3]] {
    data.criteria.iter().rev().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.as_slice()).unwrap_or_default()
}
fn skills(data: &Data, key: u32, percent: bool, texts: &TextDb) -> String {
    let mut out = String::new();
    for &(stat, value) in pairs(data, key) {
        out.push_str(&texts.by_id(2003, stat).unwrap_or_default());
        if percent { let _ = write!(out, " {value}% "); }
        else {
            out.push(' '); // DAT_10154e50 = 0x20, including value == 1.
            if value < 0 { let _ = write!(out, " ({value}) "); }
            else if value != 1 { let _ = write!(out, " {value} "); }
        }
    }
    out
}
fn location(t: &ItemTemplate, texts: &TextDb) -> String {
    let base = match t.stat(76) { Some(1) => 300, Some(2) => 200, Some(3 | 5) => 400, _ => return String::new() };
    let Some(mask) = t.stat(298).filter(|v| *v != 1_234_567_890) else { return String::new() };
    let mut out = String::new();
    for bit in 1..16 {
        if mask & (1 << bit) != 0 {
            out.push_str(&texts.by_id(1005, base + bit).unwrap_or_default());
            out.push(' ');
        }
    }
    out
}

// 10017c5d -> 10047822 -> 10081ae9: worn armor uses Wear (6), hands
// 6/8 use Wield (8). Only strict-greater (2) criteria contribute. The retail
// FSTP rounds the ratio to f32 before double (10 - ratio*10)*0.5 and _ftol.
fn overequip(data: &Data, id: Identity, zone: &Zone) -> i32 {
    if !matches!(id.kind, 101..=103) || zone.char_id == 0 { return 0; }
    let action = match id.instance { 6 | 8 => 8, 16..=31 => 6, _ => return 0 };
    criteria(data, action).iter().filter(|c| c[2] == 2).map(|c| {
        let value = zone.stats.get(&(c[0] as u32)).copied().unwrap_or(1_234_567_890);
        let ratio = (value as f64 / c[1] as f64) as f32;
        if ratio < 1.0 { ((10.0 - ratio as f64 * 10.0) * 0.5) as i32 } else { 0 }
    }).max().unwrap_or(0).clamp(0, 4)
}

pub(super) fn sections(data: &Data, t: &ItemTemplate, id: Identity, zone: &Zone, store: &RecordStore, texts: &TextDb) -> Result<Sections> {
    let mut out = Sections {
        location: location(t, texts),
        attack: skills(data, 12, true, texts),
        defend: skills(data, 13, true, texts),
        use_skill: if id.kind == 0xcf1b { String::new() } else { skills(data, 3, false, texts) },
        overequip: overequip(data, id, zone),
        ..Sections::default()
    };
    // 100211e1 hides criteria when Can (177) has bit 4 set.
    if t.stat(177).is_some_and(|v| v & 4 != 0) { return Ok(out); }
    for (index, key) in ["Get:", "Drop:", "Use:", "Repair:", "UseOn:", "Wear:", "Unwear:", "Wield:", "Unwield:", "Split:"].into_iter().enumerate() {
        let list = criteria(data, index as u32 + 1);
        if list.is_empty() { continue; }
        let value = super::item_effects::criteria_text(list, zone, store, texts)?;
        if !value.is_empty() {
            out.useby.push_str(&texts.by_key(1005, key).unwrap_or_default());
            out.useby.push_str(&value);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[test]
fn retail_inner_keys_and_overequip() {
    let data = Data { pairs: vec![(12, 3, vec![(100, 1), (101, -2)]), (3, 12, vec![(102, 75)]), (99, 12, vec![(103, 25)])], criteria: vec![(6, 8, vec![[100, 100, 2]]), (8, 6, vec![[100, 200, 2]])], ..Data::default() };
    let texts = TextDb::parse(b"MMDB\0\0\0\0".to_vec()).unwrap();
    assert_eq!(skills(&data, 3, false, &texts), "   (-2) ");
    assert_eq!(skills(&data, 12, true, &texts), " 25% ");
    let mut zone = Zone::default();
    zone.char_id = 1;
    zone.stats.insert(100, 75);
    assert_eq!(overequip(&data, Identity { kind: 101, instance: 6 }, &zone), 1);
    assert_eq!(overequip(&data, Identity { kind: 102, instance: 16 }, &zone), 3);
    assert_eq!(overequip(&data, Identity { kind: 0xc788, instance: 16 }, &zone), 0);
}
