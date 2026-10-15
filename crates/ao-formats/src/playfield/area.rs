//! GameData PlayfieldAreaInfo (1000029): ReadBlob 10008ed2, AreaData 100032e4.
//! Positions retain native AO coordinates (Y up, no scene-space Z negation).

use anyhow::{ensure, Context, Result};
use ao_rdb::RecordStore;
use super::{environment, record::{cstr, Rd}};

pub const RECORD: u32 = 1_000_029;

#[derive(Debug, Clone, PartialEq)]
pub struct Areas {
    pub areas: Vec<Area>,
    pub zone_to_area: Vec<u8>,
}

impl Areas {
    /// GetAreaData(zone), GD 10008cb8: the argument is a zone, not an area index.
    pub fn area_for_zone(&self, zone: usize) -> Option<&Area> {
        self.areas.get(usize::from(*self.zone_to_area.get(zone)?))
    }
    /// GC 10121d84: zone zero's resolved name equals the case-sensitive string `Grid`.
    pub fn is_grid(&self) -> bool {
        self.area_for_zone(0).is_some_and(|area| area.name == "Grid")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub name: String,
    pub owner: String,
    pub description: String,
    pub knowledge: String,
    /// EnvironmentData stream order, including bytes 10/11 before native struct swapping.
    pub environment: [u8; 26],
    pub control: [u8; 2],
    pub levels: [u16; 2],
    /// Native +2c..+44; final two words are default/allowed building style masks.
    pub words: [u32; 7],
    pub center: [f32; 3],
    pub points: [Vec<Point>; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub position: [f32; 3],
    pub flags: u32,
    pub data: PointData,
    pub zone: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PointData {
    Base,
    Acg { name: String, styles: u32 },
    Fence { rotation: u16 },
    Placement { angles: [u16; 2] },
}

pub fn load(store: &RecordStore, playfield: u32) -> Result<Option<Areas>> {
    store.get(RECORD, playfield)?.map(|raw| parse(&raw).with_context(|| format!("area info of playfield {playfield}"))).transpose()
}

pub fn parse(raw: &[u8]) -> Result<Areas> {
    let mut r = Rd::new(raw, 0);
    ensure!(r.u16()? == 6, "unsupported AreaInfo version");
    let zones = r.u32()? as usize;
    let count = usize::from(r.u8()?);
    ensure!((1..=1_000_000).contains(&zones) && (1..=zones).contains(&count), "invalid AreaInfo counts");
    let mut areas = Vec::with_capacity(count);
    for _ in 0..count {
        ensure!(r.u16()? == 4, "unsupported AreaData version");
        let len = usize::from(r.u16()?);
        ensure!(len >= 5, "short AreaData packed strings");
        let mut packed = r.take(len)?;
        let mut string = || -> Result<String> {
            let end = packed.iter().position(|&b| b == 0).context("unterminated AreaData string")?;
            let value = cstr(&packed[..end]);
            packed = &packed[end + 1..];
            Ok(value)
        };
        let (name, owner, description, knowledge) = (string()?, string()?, string()?, string()?);
        ensure!(packed.is_empty(), "excess AreaData string bytes");
        let environment = environment::parse_raw(&mut r)?.raw;
        let control = [r.u8()?, r.u8()?];
        ensure!(u16::from(control[0]) + u16::from(control[1]) <= 100, "invalid area control percentages");
        let levels = [r.u16()?, r.u16()?];
        ensure!(levels[0] >= 1 && levels[0] <= levels[1] && levels[1] <= 511, "invalid area levels");
        let mut words = [0; 7];
        for word in &mut words { *word = r.u32()?; }
        let center = r.vec3()?;
        ensure!(center.iter().all(|v| v.is_finite()) && center[0] > 0.0 && center[1] >= 0.0 && center[2] > 0.0, "invalid area center");
        let points = [points(&mut r)?, points(&mut r)?];
        areas.push(Area { name, owner, description, knowledge, environment, control, levels, words, center, points });
    }
    let zone_to_area = r.take(zones)?.to_vec();
    ensure!(zone_to_area.iter().all(|&index| usize::from(index) < count), "invalid zone-to-area index");
    // Native stops at the mapping and does not require EOF or inspect trailing bytes.
    Ok(Areas { areas, zone_to_area })
}

fn points(r: &mut Rd<'_>) -> Result<Vec<Point>> {
    let count = usize::from(r.u8()?);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let position = r.vec3()?;
        ensure!(position.iter().all(|v| v.is_finite()), "nonfinite area point");
        let flags = r.u32()?;
        let kind = match flags >> 24 {
            0 => flags & 3,
            1 => flags & 0x00ff_ffff,
            version => anyhow::bail!("unsupported area point version {version}"),
        };
        let data = match kind {
            0 | 4..=6 => PointData::Base,
            1..=3 => {
                let len = usize::from(r.u16()?);
                ensure!((2..=128).contains(&len), "invalid ACG point name length");
                let name = r.take(len)?;
                // Native allocates len+1 and appends its own terminator (GD 10001295).
                PointData::Acg { name: cstr(name), styles: r.u32()? }
            }
            7 => {
                let rotation = r.u16()?;
                ensure!(rotation < 360, "invalid fence rotation");
                PointData::Fence { rotation }
            }
            8 => {
                let angles = [r.u16()?, r.u16()?];
                ensure!(angles[0] < 360 && angles[1] <= 360, "invalid placement angles");
                PointData::Placement { angles }
            }
            _ => anyhow::bail!("unsupported area point type {kind}"),
        };
        out.push(Point { position, flags, data, zone: r.u16()? });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut raw = 6u16.to_le_bytes().to_vec();
        raw.extend(2u32.to_le_bytes());
        raw.push(2);
        for name in ["Area", "Grid"] {
            raw.extend(4u16.to_le_bytes());
            raw.extend(((name.len() + 4) as u16).to_le_bytes());
            raw.extend(name.as_bytes());
            raw.extend([0; 4]);
            raw.extend([0; 26]);
            raw.extend([0; 2]);
            raw.extend(1u16.to_le_bytes());
            raw.extend(511u16.to_le_bytes());
            raw.extend([0; 28]);
            for coordinate in [1.0f32, 0.0, 1.0] { raw.extend(coordinate.to_le_bytes()); }
            raw.extend([0; 2]);
        }
        raw.extend([1, 0]);
        raw
    }

    #[test]
    fn zone_zero_resolves_mapping_not_first_area() {
        let mut raw = fixture();
        let areas = parse(&raw).unwrap();
        assert_eq!(areas.areas[0].name, "Area");
        assert!(areas.is_grid());
        assert_eq!(areas.area_for_zone(1).unwrap().name, "Area");
        assert!(areas.area_for_zone(2).is_none());
        let end = raw.len();
        raw[end - 2] = 0;
        assert!(!parse(&raw).unwrap().is_grid());
        raw.extend([0; 12]);
        assert!(parse(&raw).is_ok());
    }

    #[test]
    fn malformed_and_native_rejected_records_return_errors() {
        let raw = fixture();
        for end in 0..raw.len() { assert!(parse(&raw[..end]).is_err(), "truncation {end}"); }
        for (offset, value) in [(0, 5), (6, 3), (7, 3), (9, 4), (raw.len() - 1, 2)] {
            let mut bad = raw.clone(); bad[offset] = value;
            assert!(parse(&bad).is_err(), "offset {offset}");
        }
        let mut bad = raw.clone(); bad[15..19].fill(b'x');
        assert!(parse(&bad).is_err());
        let mut empty = 6u16.to_le_bytes().to_vec();
        empty.extend([0; 17]);
        assert!(parse(&empty).is_err());
        let mut trailing = raw; trailing.extend([1, 2, 3]);
        assert!(parse(&trailing).is_ok(), "native does not inspect trailing bytes");
    }

    #[test]
    fn point_variants_are_bounded_and_preserve_native_fields() {
        for kind in 0u32..=8 {
            let mut raw = vec![1];
            for value in [1.0f32, 2.0, 3.0] { raw.extend(value.to_le_bytes()); }
            raw.extend((0x0100_0000 | kind).to_le_bytes());
            match kind {
                1..=3 => {
                    raw.extend(2u16.to_le_bytes());
                    raw.extend(b"AB"); // Native supplies the NUL after these length-delimited bytes.
                    raw.extend(0x8000_0000u32.to_le_bytes());
                }
                7 => raw.extend(359u16.to_le_bytes()),
                8 => { raw.extend(359u16.to_le_bytes()); raw.extend(360u16.to_le_bytes()); }
                _ => {}
            }
            raw.extend(42u16.to_le_bytes());
            let decoded = points(&mut Rd::new(&raw, 0)).unwrap();
            assert_eq!(decoded[0].position, [1.0, 2.0, 3.0]);
            assert_eq!(decoded[0].zone, 42);
            if (1..=3).contains(&kind) {
                assert_eq!(decoded[0].data, PointData::Acg { name: "AB".into(), styles: 0x8000_0000 });
            }
            for end in 0..raw.len() { assert!(points(&mut Rd::new(&raw[..end], 0)).is_err()); }
        }
        for flags in [0x0200_0000u32, 0x0100_0009] {
            let mut raw = vec![1];
            raw.extend([0; 12]);
            raw.extend(flags.to_le_bytes());
            assert!(points(&mut Rd::new(&raw, 0)).is_err());
        }
    }
}
