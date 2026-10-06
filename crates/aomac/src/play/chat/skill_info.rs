//! GUI 1003343f: skill name, optional next-point IP cost, base/buffed points, description.
use ao_formats::{screens::TextDb, stats::{self, buffs, skills::{Character, SkillTables}}};
use crate::play::{hud_stats::{buffs::modifiers_with_equipment, equipment::Equipment}, zone::Zone};

pub(super) fn html(zone: &Zone, stat: u32, texts: &TextDb) -> anyhow::Result<String> {
    let dir = ao_gui::client_dir();
    let store = ao_rdb::RecordStore::open(&dir)?;
    let tables = SkillTables::load(&store)?;
    let get = |id| zone.stat(id);
    let character = Character::from_stats(get);
    let lock = zone.stat(buffs::LOCK_STAT).unwrap_or(0);
    let mut equipment = Equipment::new(&dir);
    equipment.refresh(zone);
    let current = |id, modifiers: &buffs::Modifiers| buffs::skill_value(&tables, id, zone.stat(id).unwrap_or(0), &character, modifiers, lock);
    let modifiers = modifiers_with_equipment(&zone.active_spells, equipment.effects(), zone.stat(stats::LEVEL).unwrap_or(0), &current);
    let base = buffs::skill_base(&tables, stat, zone.stat(stat).unwrap_or(0), &character, &modifiers, lock);
    let value = zone.skill_value(stat).unwrap_or(0);
    let cost = tables.cost(stat, base, &character);
    Ok(render(texts, stat, base, value, (tables.cost_level(stat, &character) != 0 && cost != 0.0).then_some(cost)))
}

fn render(texts: &TextDb, stat: u32, base: i32, value: i32, cost: Option<f32>) -> String {
    let mut out = format!("<font color=CCInfoHeadline>{}</font><br>", texts.by_id(2002, stat).unwrap_or_default());
    if let Some(cost) = cost {
        super::character::row(&mut out, &texts.by_key(506, "Cost").unwrap_or_default(), &format!("{cost:.1} ip"), 0);
    }
    for (label, points) in [("Base", base), ("Total", value)] {
        super::character::row(&mut out, &texts.by_key(506, label).unwrap_or_default(), &format!("{points} points"), 0);
    }
    out.push_str("<font color=CCInfoText>");
    out.push_str(&stats::description(texts, stat).unwrap_or_default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_page_keeps_original_point_units_and_optional_cost() {
        let texts = TextDb::parse(b"MMDB\0\0\0\0".to_vec()).unwrap();
        let page = render(&texts, 100, 12, 34, Some(2.5));
        assert!(page.contains("2.5 ip"));
        assert!(page.find("12 points").unwrap() < page.find("34 points").unwrap());
        assert!(!render(&texts, 100, 12, 34, None).contains(" ip"));
    }
}
