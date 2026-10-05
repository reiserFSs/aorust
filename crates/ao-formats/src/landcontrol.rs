//! rdb 1000008: the playfield's land-control bitmap (`GameData::LandControlMap_t`, Gamecode `ReadBlob`; the playfield map's
//! `MapData_c` reads it through `N3Msg_GetLandControlBitmap`, GUI.dll `FUN_100428xx` neighbourhood).
//!
//! ```text
//! u32 version (=1)  u16 width  u16 height  12 reserved bytes   (20 byte header)
//! height rows of ceil(width / 8) bytes, bit 0 of each byte is the leftmost cell
//! ```
//! All 53 records satisfy `len == 20 + ceil(width/8) * height` (survey in the tests). Type identification is an
//! INFERENCE: the GUI's `MapData_c` asks `GetLandControlBitmap` for exactly this per-playfield cell mask and the table
//! holds the 53 playfields with tower fields.

use anyhow::{bail, Result};

pub const RDB_TYPE: u32 = 1_000_008;

#[derive(Debug, Clone, PartialEq)]
pub struct LandControlMap {
    pub width: u32,
    pub height: u32,
    bits: Vec<u8>,
}

impl LandControlMap {
    pub fn get(&self, x: u32, y: u32) -> bool {
        x < self.width && y < self.height && self.bits[(y * self.width.div_ceil(8) + x / 8) as usize] >> (x % 8) & 1 != 0
    }
}

pub fn parse(d: &[u8]) -> Result<LandControlMap> {
    if d.len() < 20 {
        bail!("land control map: {} bytes", d.len());
    }
    let (width, height) = (u16::from_le_bytes([d[4], d[5]]) as u32, u16::from_le_bytes([d[6], d[7]]) as u32);
    let stride = width.div_ceil(8) as usize;
    if d.len() != 20 + stride * height as usize {
        bail!("land control map {width}x{height}: {} bytes", d.len());
    }
    Ok(LandControlMap { width, height, bits: d[20..].to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_and_errors() {
        let mut d = vec![1, 0, 0, 0, 10, 0, 2, 0];
        d.resize(20, 0);
        d.extend([0b0000_0101, 0b10, 0, 0b01]);
        let m = parse(&d).unwrap();
        assert!(m.get(0, 0) && !m.get(1, 0) && m.get(2, 0) && m.get(9, 0) && !m.get(8, 0) && !m.get(0, 1) && m.get(8, 1) && !m.get(10, 0));
        assert!(parse(&d[..23]).is_err());
        assert!(parse(&[0; 4]).is_err());
    }

    #[test]
    fn real_records_parse() {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Games/ProjectRubiKa/client");
        let Ok(store) = ao_rdb::RecordStore::open(&dir) else { return };
        let ids = store.ids(RDB_TYPE).unwrap();
        assert_eq!(ids.len(), 53);
        for id in ids {
            let m = parse(&store.get(RDB_TYPE, id).unwrap().unwrap()).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(m.width > 0 && m.height > 0, "{id}");
        }
        let m = parse(&store.get(RDB_TYPE, 505).unwrap().unwrap()).unwrap();
        assert_eq!((m.width, m.height), (860, 1160));
    }
}
