//! rdb type 1000014 — `GameData::PlayfieldDistrictInfo_t::ReadBlob` (GameData.dll @10009def) with
//! `DistrictData_t` stream reader (`GameData::operator>>` @100049be).
//!
//! ```text
//! u16 version (5|6|7)  u32 nZones  u8 nDistricts
//! nDistricts x district { f32 centre[3];
//!                         name: v>=6 u16 len + bytes, v5 char[32];
//!                         u16 music_ids[9];      // [0] = district ambience sound id, [1..9] = music layer ids
//!                         v5: u8[4];  npc level: v>=6 u16 min,max | v5 u8 min,max;
//!                         v>=6: land-control level u16 min,max;
//!                         u8 respawn%, i32 respawn time, u8 fight mode, u8 nHashInfo, (v5: u8),
//!                         u8 nA, u8 nSpawnPts, u8 nHash, ... spawn lists (not parsed here) }
//! nZones x u8 district index                      // always the last nZones bytes of the blob
//! ```
//! The spawn lists after the level ranges are variable length and not needed for audio; the next district
//! header is located by scanning for a plausible header (3 floats, printable name, music ids <200 or
//! 0xffff), and the result is validated (exactly `nDistricts` found, every zone byte < `nDistricts`).
//! All 600 records of the shipped rdb parse this way.
use anyhow::{bail, ensure, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct District {
    pub name: String,
    /// `music_ids[0]`: selects the `SM_Sandy_Env_Background{Phase}_<id>` ambience set.
    pub sound_id: u16,
    /// `music_ids[1..9]` = slots 1..8: dawn, day, dusk, night, fog, rain, storm, (unused). 0xffff = none.
    /// Value = anarchy.sws layer id.
    pub music: [u16; 8],
    pub npc_lvl: (u16, u16),
    pub lc_lvl: (u16, u16),
    /// Centre of the district (`f32[3]`, server coordinates).
    pub centre: [f32; 3],
    /// `u8 fight mode` after `u8 respawn %, i32 respawn time` (GD `operator>>` @0x100049be); 0 when the record ends early.
    pub fight_mode: u8,
}

#[derive(Debug, Clone)]
pub struct Districts {
    pub zone_to_district: Vec<u8>,
    pub districts: Vec<District>,
}

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?))
}

/// Try to read a district header at `p`; returns the district and the offset just after the level ranges.
fn header_at(d: &[u8], p: usize, ver: u16) -> Option<(District, usize)> {
    let f = |o: usize| Some(f32::from_le_bytes(d.get(p + o..p + o + 4)?.try_into().ok()?));
    for o in [0, 4, 8] {
        let v = f(o)?;
        if !(0.0..30000.0).contains(&v) {
            return None;
        }
    }
    let printable = |s: &[u8]| !s.is_empty() && s.iter().all(|&c| (0x20..0x7f).contains(&c));
    let (name, ids_at) = if ver >= 6 {
        let len = u16_at(d, p + 12)? as usize;
        if !(1..=64).contains(&len) {
            return None;
        }
        let s = d.get(p + 14..p + 14 + len)?;
        if !printable(s) {
            return None;
        }
        (String::from_utf8_lossy(s).into_owned(), p + 14 + len)
    } else {
        let raw = d.get(p + 12..p + 44)?;
        let n = raw.iter().position(|&c| c == 0).unwrap_or(32);
        if !printable(&raw[..n]) || raw[n..].iter().any(|&c| c != 0) {
            return None;
        }
        (String::from_utf8_lossy(&raw[..n]).into_owned(), p + 44)
    };
    let mut ids = [0u16; 9];
    for (i, id) in ids.iter_mut().enumerate() {
        *id = u16_at(d, ids_at + 2 * i)?;
    }
    if ids[1..].iter().any(|&v| v >= 200 && v != 0xffff) {
        return None;
    }
    let q = ids_at + 18;
    let (npc, lc, end) = if ver >= 6 {
        ((u16_at(d, q)?, u16_at(d, q + 2)?), (u16_at(d, q + 4)?, u16_at(d, q + 6)?), q + 8)
    } else {
        ((*d.get(q + 4)? as u16, *d.get(q + 5)? as u16), (0, 0), q + 6)
    };
    let mut music = [0u16; 8];
    music.copy_from_slice(&ids[1..]);
    let fight_mode = d.get(end + 5).copied().unwrap_or(0);
    Some((District { name, sound_id: ids[0], music, npc_lvl: npc, lc_lvl: lc, centre: [f(0)?, f(4)?, f(8)?], fight_mode }, end))
}

impl Districts {
    pub fn parse(d: &[u8]) -> Result<Districts> {
        ensure!(d.len() >= 7, "district: truncated");
        let ver = u16_at(d, 0).unwrap();
        if !matches!(ver, 5..=7) {
            bail!("district: unsupported version {ver}");
        }
        let nz = u32::from_le_bytes(d[2..6].try_into().unwrap()) as usize;
        let nd = d[6] as usize;
        if nd == 0 {
            return Ok(Districts { zone_to_district: vec![], districts: vec![] });
        }
        ensure!(nz > 0 && nz <= d.len() - 7, "district: bad counts nz={nz} nd={nd}");
        let zones = &d[d.len() - nz..];
        ensure!(zones.iter().all(|&z| (z as usize) < nd), "district: zone index out of range");
        let body_end = d.len() - nz;
        let (mut districts, mut p) = (Vec::with_capacity(nd), 7usize);
        while districts.len() < nd {
            let mut found = None;
            while p + 14 < body_end {
                if let Some(h) = header_at(&d[..body_end], p, ver) {
                    found = Some(h);
                    break;
                }
                p += 1;
            }
            let Some((dist, end)) = found else { bail!("district: only {} of {nd} districts found", districts.len()) };
            districts.push(dist);
            p = end;
        }
        Ok(Districts { zone_to_district: zones.to_vec(), districts })
    }

    /// District of a playfield zone (`GetDistrictData(zone)`), `None` when the zone is out of range.
    pub fn district(&self, zone: usize) -> Option<&District> {
        self.districts.get(*self.zone_to_district.get(zone)? as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn district_v7(name: &str, ids: [u16; 9]) -> Vec<u8> {
        let mut v = vec![];
        for x in [300f32, 34.8, 260.0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        for i in ids {
            v.extend_from_slice(&i.to_le_bytes());
        }
        for x in [1u16, 45, 0, 0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&[100, 100, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0]); // respawn/fight/counts etc.
        v
    }

    #[test]
    fn synthetic() {
        let mut b = 7u16.to_le_bytes().to_vec();
        b.extend_from_slice(&3u32.to_le_bytes());
        b.push(2);
        b.extend(district_v7("Area A", [37, 0, 0, 0, 2, 2, 2, 2, 14]));
        b.extend(district_v7("Area B", [0, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff]));
        b.extend_from_slice(&[0, 1, 1]);
        let d = Districts::parse(&b).unwrap();
        assert_eq!(d.districts.len(), 2);
        assert_eq!(d.districts[0].name, "Area A");
        assert_eq!((d.districts[0].sound_id, d.districts[0].music), (37, [0, 0, 0, 2, 2, 2, 2, 14]));
        assert_eq!(d.districts[0].npc_lvl, (1, 45));
        assert_eq!(d.district(2).unwrap().name, "Area B");
        assert!(d.district(3).is_none());
        let mut bad = b.clone();
        *bad.last_mut().unwrap() = 9;
        assert!(Districts::parse(&bad).is_err());
    }

    #[test]
    fn real_districts() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let ids = store.ids(1000014).unwrap();
        assert_eq!(ids.len(), 600);
        let mut total = 0;
        for id in &ids {
            let blob = store.get(1000014, *id).unwrap().unwrap();
            total += Districts::parse(&blob).unwrap_or_else(|e| panic!("pf {id}: {e}")).districts.len();
        }
        assert_eq!(total, 4241);
        let d = Districts::parse(&store.get(1000014, 566).unwrap().unwrap()).unwrap();
        assert_eq!((d.zone_to_district.len(), d.districts.len()), (225, 2));
        // dawn, day, dusk = desert\Day (0), night = desert\Night (2)
        assert_eq!(d.district(0).unwrap().music[..4], [0, 0, 0, 2]);
        assert_eq!(d.district(0).unwrap().sound_id, 37);
        let d = Districts::parse(&store.get(1000014, 127).unwrap().unwrap()).unwrap();
        assert_eq!((d.zone_to_district.len(), d.districts.len()), (46, 4));
        assert_eq!(d.districts[1].name, "Condemned Subway");
        assert_eq!(d.districts[1].music, [16; 8]);
        let g = Districts::parse(&store.get(1000014, 152).unwrap().unwrap()).unwrap();
        assert_eq!(g.districts[0].music, [0xffff; 8]);
    }
}
