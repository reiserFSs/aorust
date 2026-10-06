//! Dummy-item tower interpolation: GC 10082a3d -> 100cc270 -> 100cba71.
//! PTR_Interpolate_10154580 is Color_t interpolation, unrelated to ACG items.
use anyhow::{ensure, Result};
use ao_formats::dynel_visual::{self, ItemTemplate};
use ao_net::n3::{spells::Spell, world::AcgItem};
use ao_rdb::RecordStore;

pub(super) struct TowerTemplate {
    pub template: ItemTemplate,
    pub lists: [Vec<Spell>; 3],
}

pub(super) fn interpolated(store: &RecordStore, item: &AcgItem) -> Result<Option<TowerTemplate>> {
    let Some(low) = store.get(dynel_visual::ITEM_TEMPLATE_TYPE, item.low_id as u32)? else { return Ok(None) };
    let high = if item.low_id == item.high_id { None } else { store.get(dynel_visual::ITEM_TEMPLATE_TYPE, item.high_id as u32)? };
    if item.low_id != item.high_id && high.is_none() { return Ok(None); }
    let high = high.as_deref().unwrap_or(&low);
    let a = dynel_visual::parse_item_template(&low)?;
    let b = dynel_visual::parse_item_template(high)?;
    let q = if (1..=511).contains(&(item.level as u16 as i32)) { item.level as u16 as i32 } else { 1 };
    let aq = a.stat(54).unwrap_or(0) as u16 as i32;
    let bq = b.stat(54).unwrap_or(0) as u16 as i32;
    let template = dynel_visual::interpolate_item_templates(&a, &b, q)?;
    let mut lists = [Vec::new(), Vec::new(), Vec::new()];
    for (index, list) in lists.iter_mut().enumerate() {
        let a = super::template_spells::spells(&low, index as u32 + 24)?;
        let b = super::template_spells::spells(high, index as u32 + 24)?;
        *list = if (q - aq).abs() < (q - bq).abs() { a.clone() } else { b.clone() };
        if aq == bq || item.low_id == item.high_id { continue; }
        ensure!(a.len() == b.len(), "different tower spell counts");
        for (index, spell) in list.iter_mut().enumerate() {
            let by_stat = matches!(spell.function, 0xcf0a | 0xcf0e | 0xcf14 | 0xcf16 | 0xcf20 | 0xcf22..=0xcf24 | 0xcf29 | 0xcf35 | 0xcf51 | 0xcf76 | 0xcf77 | 0xcf93..=0xcf95 | 0xcfaa | 0xcfb7 | 0xcfb9 | 0xcff5);
            let low = if by_stat { a.iter().find(|s| s.function == spell.function && s.stat(0) == spell.stat(0)).unwrap_or(spell) } else { &a[index] };
            let high = if by_stat { b.iter().find(|s| s.function == spell.function && s.stat(0) == spell.stat(0)).unwrap_or(spell) } else { &b[index] };
            let mut result = spell.clone();
            interpolate_spell(&mut result, low, high, aq, bq, q)?;
            *spell = result;
        }
    }
    Ok(Some(TowerTemplate { template, lists }))
}

// GC 100cb4ba, tables 1016b168..1016b1f8. Only these arguments interpolate.
fn fields(function: u32) -> &'static [u16] {
    match function {
        0xcfc1 => &[2, 37, 39],
        0xcf0a | 0xcfcc => &[2, 37],
        0xcf47 | 0xcf48 | 0xcfe4 => &[84, 72],
        0xcf14 | 0xcf22 | 0xcf25 | 0xcf29 | 0xcf35 | 0xcf61 | 0xcf7d | 0xcf83 | 0xcfaa | 0xcfb7 | 0xcff5 | 0xd001 => &[39],
        0xcfb9 => &[0, 39],
        0xcf84 | 0xcf85 => &[39, 84, 72],
        0xcf5f | 0xcfa0 => &[11],
        0xcfaf => &[84, 72, 131, 132, 133],
        0xcf51 => &[2, 37, 11],
        0xcf71 => &[84],
        0xcf16 => &[11, 25, 39],
        _ => &[],
    }
}

fn interpolate_spell(out: &mut Spell, low: &Spell, high: &Spell, aq: i32, bq: i32, q: i32) -> Result<()> {
    // GC 100cb934: same criterion keys/count, interpolate values, retain selected operators.
    ensure!(low.criteria.len() == high.criteria.len() && out.criteria.len() == low.criteria.len(), "different spell criteria counts");
    for ((out, a), b) in out.criteria.iter_mut().zip(&low.criteria).zip(&high.criteria) {
        ensure!(out[0] == a[0] && out[0] == b[0], "different spell criterion keys");
        out[1] = dynel_visual::interpolate_acg_value(a[1], b[1], aq, bq, q);
    }
    for &id in fields(out.function) {
        let (Some(&a), Some(&b)) = (low.stats.get(&id), high.stats.get(&id)) else { continue };
        out.stats.insert(id, dynel_visual::interpolate_acg_value(a, b, aq, bq, q));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tower_amount_interpolates_but_stat_and_target_do_not() {
        let mut a = Spell { function: 0xcf35, ..Spell::default() };
        a.stats.extend([(0, 16), (32, 2), (39, -10)]);
        let mut b = a.clone();
        b.stats.insert(39, -1);
        let mut out = b.clone();
        interpolate_spell(&mut out, &a, &b, 1, 3, 2).unwrap();
        assert_eq!((out.stat(0), out.stat(32), out.stat(39)), (16, 2, -5));
        assert!(fields(0xcf76).is_empty());
    }
}
