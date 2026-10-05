//! Playfield resource (rdb 1000001, `RDBPlayfield_t::ReadBlob` in N3.dll @ 0x1001c115) and its
//! dungeon room list (`n3Room_t` reader, N3.dll @ 0x10012803).

use anyhow::{ensure, Result};

/// Little-endian cursor over a record.
pub(super) struct Rd<'a> {
    pub d: &'a [u8],
    pub o: usize,
}

impl<'a> Rd<'a> {
    pub fn new(d: &'a [u8], o: usize) -> Self {
        Rd { d, o }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(n <= self.d.len() - self.o.min(self.d.len()), "truncated record at {:#x} (+{n})", self.o);
        let s = &self.d[self.o..self.o + n];
        self.o += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
}

/// NUL-terminated name in a fixed buffer.
pub(super) fn cstr(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..n].iter().map(|&c| c as char).collect()
}

/// One dungeon room: the zone with the same index in the statel file is placed relative to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    /// Quarter turns about +Y (`flags & 3`).
    pub rot: u8,
    /// Tile rectangle `[x1, z1, x2, z2]` in the dungeon tilemap (`GNDA` record `tilemap`).
    pub rect: [u16; 4],
    /// World position of the room centre; statels in the room are relative to it.
    pub pos: [f32; 3],
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub version: u32,
    pub id: u32,
    pub name: String,
    /// Tilemap record (rdb 1000009) id. Equal to `id` for outdoor playfields (own heightfield).
    pub tilemap: u32,
    /// Zone edge length in tiles.
    pub zone_size: u32,
    /// Number of zones (outdoor) or rooms (dungeon) = number of offsets in the statel file.
    pub count: u32,
    pub rooms: Vec<Room>,
    /// Byte offset of the `RDBPlayfieldAnarchy_t` tail (liquid polygons, environment; see `water`/`environment`).
    pub tail: usize,
}

impl Record {
    pub fn is_outdoor(&self) -> bool {
        self.tilemap == self.id
    }
}

pub fn parse(d: &[u8]) -> Result<Record> {
    let mut r = Rd::new(d, 0);
    let version = r.u32()?;
    let id = r.u32()?;
    let name = cstr(r.take(32)?);
    let tilemap = r.u32()?;
    let zone_size = r.u32()?;
    let count = r.u32()?;
    ensure!(version >= 7, "playfield {id}: unsupported data format version {version}");
    if version > 8 {
        r.skip(5 * 4 + 0x18)?;
    }
    let mut rooms = Vec::new();
    if tilemap != id {
        ensure!(count < 50_000, "playfield {id}: implausible room count {count}");
        for _ in 0..count {
            rooms.push(room(&mut r, version)?);
        }
    } else {
        // per zone: u32 n, n x camera attractor (vec3 + quat + vec3 + f32), see `RDBPlayfield_t::ReadBlob` @0x1001c115
        for _ in 0..count {
            let n = r.u32()? as usize;
            r.skip(n * (12 + 16 + 12 + 4))?;
        }
    }
    Ok(Record { version, id, name, tilemap, zone_size, count, rooms, tail: r.o })
}

fn room(r: &mut Rd, version: u32) -> Result<Room> {
    let flags = r.u8()?;
    r.u8()?;
    let rect = [r.u16()?, r.u16()?, r.u16()?, r.u16()?];
    ensure!(rect[0] < rect[2] && rect[1] < rect[3], "invalid room tile rectangle {rect:?}");
    let pos = r.vec3()?;
    let doors = r.u16()? as usize;
    r.skip(doors * 4)?; // (u16 door tile x, u16 door tile z) pairs, not needed for geometry
    let name = if flags & 0x80 != 0 { Some(cstr(r.take(32)?)) } else { None };
    // lightmap: i32 size+4, i32 present, bytes
    let lm = r.u32()?;
    let present = r.u32()?;
    if present != 0 {
        r.skip(lm.saturating_sub(4) as usize)?;
    }
    // liquid data: u32 (n+1)*0x3f1, n x { u32, u32 n1, n1 x vec3, u32 n2, n2 x 3 u16 }
    let water = r.u32()?;
    if water != 0 && water % 0x3f1 == 0 {
        for _ in 0..water / 0x3f1 - 1 {
            r.u32()?;
            let n1 = r.u32()? as usize;
            r.skip(n1 * 12)?;
            let n2 = r.u32()? as usize;
            r.skip(n2 * 6)?;
        }
    }
    if version > 4 {
        let n = r.u32()? as usize;
        r.skip(n * (12 + 16 + if version >= 6 { 12 } else { 0 } + 4))?; // camera attractors
    }
    Ok(Room { rot: flags & 3, rect, pos, name })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(version: u32, id: u32, tilemap: u32, count: u32) -> Vec<u8> {
        let mut d = Vec::new();
        for v in [version, id] {
            d.extend(v.to_le_bytes());
        }
        let mut name = b"Test PF".to_vec();
        name.resize(32, 0);
        d.extend(name);
        for v in [tilemap, 10, count] {
            d.extend(v.to_le_bytes());
        }
        d
    }

    #[test]
    fn outdoor_header() {
        let mut d = header(8, 566, 566, 2);
        d.extend(1u32.to_le_bytes()); // zone 0: one camera attractor
        d.extend([0u8; 12 + 16 + 12 + 4]);
        d.extend(0u32.to_le_bytes()); // zone 1: none
        let tail = d.len();
        d.extend([7u8; 10]); // anarchy tail (water / environment)
        let r = parse(&d).unwrap();
        assert_eq!((r.name.as_str(), r.count, r.zone_size, r.is_outdoor()), ("Test PF", 2, 10, true));
        assert!(r.rooms.is_empty());
        assert_eq!(r.tail, tail);
    }

    #[test]
    fn dungeon_room() {
        let mut d = header(8, 127, 126, 1);
        d.extend([0x81, 5]); // rot 1, has name
        for v in [145u16, 117, 154, 122] {
            d.extend(v.to_le_bytes());
        }
        for v in [184.0f32, 107.5, 224.0] {
            d.extend(v.to_le_bytes());
        }
        d.extend(1u16.to_le_bytes()); // one door
        d.extend([0, 0, 78, 0]);
        let mut name = b"Ladies' Room".to_vec();
        name.resize(32, 0);
        d.extend(name);
        d.extend([0u8; 16]); // lightmap size, present, liquid, camera attractor count
        let r = parse(&d).unwrap();
        assert_eq!(r.rooms.len(), 1);
        let room = &r.rooms[0];
        assert_eq!((room.rot, room.rect, room.pos), (1, [145, 117, 154, 122], [184.0, 107.5, 224.0]));
        assert_eq!(room.name.as_deref(), Some("Ladies' Room"));
    }
}
