//! GUI 100322b8: HouseTemplate lookup, upkeep then X-by-Z layout size.
use anyhow::Result;
use ao_formats::{city, dynel_visual::ItemTemplate, screens::TextDb, stats::INVALID};
use ao_rdb::RecordStore;

pub(super) fn html(template: &ItemTemplate, store: &RecordStore, texts: &TextDb) -> Result<String> {
    let Some(id) = template.stat(620).filter(|&id| id != 0 && id != INVALID) else { return Ok(String::new()); };
    let Some(record) = store.get(city::RDB_TYPE, id as u32)? else { return Ok(String::new()); };
    let info = city::house_info(&record)?;
    let mut out = String::new();
    super::character::row(&mut out, &texts.by_key(506, "BuildingUpkeep").unwrap_or_default(), &crate::play::hud::group(info.upkeep), 0);
    super::character::row(&mut out, &texts.by_key(506, "BuildingSize").unwrap_or_default(), &format!("{}x{}", info.tiles_x, info.tiles_z), 0);
    Ok(out)
}
