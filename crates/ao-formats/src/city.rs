//! City.dll 1000f1a7 / 10013f65: city-house info prefix of RDB 1000040.
//! Visuals, doors, guards, advantages and the KD-tree follow the layout; not decoded here.
use anyhow::{bail, Context, Result};

pub const RDB_TYPE: u32 = 1_000_040;

#[derive(Debug, PartialEq)]
pub struct HouseInfo {
    pub upkeep: i32,
    pub tiles_x: u32,
    pub tiles_z: u32,
}

/// Decode the fields used by GUI 100322b8 (registry lookup rotation is zero).
pub fn house_info(data: &[u8]) -> Result<HouseInfo> {
    let mut pos: usize = 0;
    let mut take = |n: usize| -> Result<&[u8]> {
        let end = pos.checked_add(n).context("city house offset overflow")?;
        let bytes = data.get(pos..end).context("truncated city house info")?;
        pos = end;
        Ok(bytes)
    };
    let version = u32::from_le_bytes(take(4)?.try_into()?);
    if !(1..=4).contains(&version) { bail!("unsupported city house version {version}"); }
    take(8)?; // Identity; v4 adds inside-template instance and fixture limit.
    if version == 4 { take(8)?; }
    let name_len = u16::from_le_bytes(take(2)?.try_into()?) as usize;
    if name_len >= 0x8000 { bail!("invalid city house name length {name_len}"); }
    take(name_len)?;
    take(8)?; // House type and stealth factor.
    let upkeep = i32::from_le_bytes(take(4)?.try_into()?);
    take(if version < 3 { 1 } else { 4 })?; // Legacy boolean / house flags.
    let tiles = u32::from_le_bytes(take(4)?.try_into()?);
    let tiles_x = u32::from_le_bytes(take(4)?.try_into()?);
    let tiles_z = u32::from_le_bytes(take(4)?.try_into()?);
    take(tiles.div_ceil(8) as usize)?; // HouseLayout::ReadBlobFixedLen: packed bits.
    Ok(HouseInfo { upkeep, tiles_x, tiles_z })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_city_house_headers() {
        // RDB1000040:393217 v1 and :6946816 v4, through their packed layouts.
        let legacy = "01000000000000000000000000000000000000000000e803000000230000000700000005000000ffffffff07";
        let current = "040000000000000000000000f61000006400000000000000000000000000e803000001000000640000000a0000000a0000001f7cf0c1071ffcffffffffff0f";
        for (hex, x, z) in [(legacy, 7, 5), (current, 10, 10)] {
            let bytes: Vec<u8> = hex.as_bytes().as_chunks::<2>().0.iter().map(|s| u8::from_str_radix(std::str::from_utf8(s).unwrap(), 16).unwrap()).collect();
            assert_eq!(house_info(&bytes).unwrap(), HouseInfo { upkeep: 1000, tiles_x: x, tiles_z: z });
            assert!(house_info(&bytes[..bytes.len()-1]).is_err());
        }
    }
}
