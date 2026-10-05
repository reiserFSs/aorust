//! Camera position -> zone index (`n3Playfield_t::GetZoneInstance`), used to look up the audio district.
//!
//! Outdoor playfields tile the map with square zones of `zone_size` cells, row-major (`zone = row * columns + col`,
//! the statel file order). Dungeon zones are rooms; [INFERENCE] the room whose centre is nearest in the XZ plane
//! (the exact `GetZoneInstance` room test was not reverse engineered).

use anyhow::{anyhow, Context, Result};
use ao_rdb::RecordStore;

use super::{ground, record, RECORD, TILEMAP};

pub struct ZoneLocator {
    kind: Kind,
}

enum Kind {
    /// zone edge in metres, columns, rows
    Grid(f32, usize, usize),
    /// room centres (AO world x, z)
    Rooms(Vec<[f32; 2]>),
}

impl ZoneLocator {
    /// Zone containing the scene-space point (scene z is the negated AO world z). Points outside the map clamp to
    /// its border zone. `None` for a playfield without zones.
    pub fn zone_at(&self, scene_pos: [f32; 3]) -> Option<usize> {
        let (x, z) = (scene_pos[0], -scene_pos[2]);
        match &self.kind {
            Kind::Grid(m, w, h) => {
                let col = ((x / m).floor().max(0.0) as usize).min(w.saturating_sub(1));
                let row = ((z / m).floor().max(0.0) as usize).min(h.saturating_sub(1));
                (*w > 0 && *h > 0).then_some(row * w + col)
            }
            Kind::Rooms(c) => c
                .iter()
                .enumerate()
                .min_by(|a, b| d2(a.1, x, z).total_cmp(&d2(b.1, x, z)))
                .map(|(i, _)| i),
        }
    }
}

fn d2(c: &[f32; 2], x: f32, z: f32) -> f32 {
    (c[0] - x).powi(2) + (c[1] - z).powi(2)
}

pub fn zone_locator(store: &RecordStore, id: u32) -> Result<ZoneLocator> {
    let raw = store.get(RECORD, id)?.ok_or_else(|| anyhow!("no playfield {id}"))?;
    let rec = record::parse(&raw)?;
    let kind = if rec.is_outdoor() {
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        let tm = ground::parse(&d).with_context(|| format!("tilemap {}", rec.tilemap))?;
        let zs = rec.zone_size.max(1) as usize;
        Kind::Grid(zs as f32 * tm.cell_size, tm.cells_x.div_ceil(zs), tm.cells_z.div_ceil(zs))
    } else {
        Kind::Rooms(rec.rooms.iter().map(|r| [r.pos[0], r.pos[2]]).collect())
    };
    Ok(ZoneLocator { kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_and_rooms() {
        let g = ZoneLocator { kind: Kind::Grid(40.0, 15, 15) };
        assert_eq!(g.zone_at([1.0, 0.0, -1.0]), Some(0));
        assert_eq!(g.zone_at([85.0, 0.0, -41.0]), Some(15 + 2));
        assert_eq!(g.zone_at([-50.0, 0.0, 5000.0]), Some(0)); // clamped
        assert_eq!(g.zone_at([1e6, 0.0, -1e6]), Some(224));
        let r = ZoneLocator { kind: Kind::Rooms(vec![[0.0, 0.0], [100.0, 50.0]]) };
        assert_eq!(r.zone_at([90.0, 0.0, -40.0]), Some(1));
        assert_eq!(r.zone_at([1.0, 0.0, 0.0]), Some(0));
    }
}
