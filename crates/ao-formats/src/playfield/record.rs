//! Playfield resource (rdb 1000001, `RDBPlayfield_t::ReadBlob` in N3.dll @ 0x1001c115) and its
//! dungeon room list (`n3Room_t` reader, N3.dll @ 0x10012803).

use anyhow::{ensure, Result};

use super::water::Water;

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
    /// Baked vertex lighting: vertex count and the zlib stream (`n3Room_t::DepackLightmap`, see `dungeon`).
    pub lightmap: Option<(u32, Vec<u8>)>,
    /// Rooms behind this room's doors, one entry per door (`u16` zone index, `0xffff` = none; `n3Room_t::GetDoorConnectZone`
    /// @0x10010990 reads the first `u16` of each 4 byte entry, the second one is `tile << 2 | orientation`, `RegisterDoorPosition`
    /// @0x1001037e).
    pub door_zones: Vec<u16>,
    /// The second `u16` of each door entry, parallel to `door_zones`: `tile << 2 | orientation`, the tile counted in the room's
    /// rectangle rows (`n3Room_t::GetDoorLinkFromPos` @0x100105f9, `GetDoorPosRot` @0x10010acf).
    pub door_tiles: Vec<u16>,
    /// Liquid polygons in room-local coordinates (world = `pos + v`: the client's `n3Zone_t::AddLiquidCollisionData` multiplies
    /// the rotation angle `rot * pi/2` by the zero constant f64 @0x1003cb08, so the vertices are never rotated), already
    /// triangulated like the client's `n3WaterData_t` array: even kinds with `nv != 3` are a fan around the vertex centroid
    /// (the last vertex), the rest use the file's triangle list.
    pub waters: Vec<Water>,
}

/// A scripted camera position of a zone/room (`PointCameraAttractor_t`, N3 vtable 0x1003e4f4, built by `FUN_10023bbb` from the
/// playfield blob, `RDBPlayfield_t::ReadBlob` @0x1001c115): the camera is steered to `pos` and keeps looking at the player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraAttractor {
    /// World position (`+0x04`).
    pub pos: [f32; 3],
    /// Orientation quaternion x, y, z, w (`+0x10`).
    pub rot: [f32; 4],
    /// Point the original authored view looks at (`+0x20`; version < 6 records have `target = pos`).
    pub target: [f32; 3],
    /// Range (`+0x2c`): a part of the score and, at >= 2.0 ([`Self::DISABLED_AT`]), the "never visible" flag (`+0x30`).
    pub range: f32,
}

impl CameraAttractor {
    /// `fVar4 >= _DAT_1003c968` (2.0) sets the disabled byte `+0x30`; `FUN_10023bfc` then reports the attractor as not visible.
    pub const DISABLED_AT: f32 = 2.0;

    pub fn disabled(&self) -> bool {
        self.range >= Self::DISABLED_AT
    }
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
    /// Camera attractors per zone (outdoor) or room (dungeon), indexed like the statel zones (`n3Zone_t::GetCameraAttractorList`).
    pub attractors: Vec<Vec<CameraAttractor>>,
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
    let mut attractors = Vec::new();
    if tilemap != id {
        ensure!(count < 50_000, "playfield {id}: implausible room count {count}");
        for _ in 0..count {
            rooms.push(room(&mut r, version, &mut attractors)?);
        }
    } else {
        // per zone: u32 n, n x camera attractor (vec3 + quat + vec3 + f32), see `RDBPlayfield_t::ReadBlob` @0x1001c115
        for _ in 0..count {
            attractors.push(camera_attractors(&mut r, version)?);
        }
    }
    Ok(Record { version, id, name, tilemap, zone_size, count, rooms, attractors, tail: r.o })
}

/// `u32 n` (client limit: `n < 1000`), n x { pos, quat, target (version >= 6, else = pos), range }; version <= 4 has none.
fn camera_attractors(r: &mut Rd, version: u32) -> Result<Vec<CameraAttractor>> {
    if version <= 4 {
        return Ok(Vec::new());
    }
    let n = r.u32()? as usize;
    ensure!(n < 1000, "too many camera attractors ({n})");
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let pos = r.vec3()?;
        let rot = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
        let target = if version >= 6 { r.vec3()? } else { pos };
        v.push(CameraAttractor { pos, rot, target, range: r.f32()? });
    }
    Ok(v)
}

fn room(r: &mut Rd, version: u32, attractors: &mut Vec<Vec<CameraAttractor>>) -> Result<Room> {
    let flags = r.u8()?;
    r.u8()?;
    let rect = [r.u16()?, r.u16()?, r.u16()?, r.u16()?];
    ensure!(rect[0] < rect[2] && rect[1] < rect[3], "invalid room tile rectangle {rect:?}");
    let pos = r.vec3()?;
    let doors = r.u16()? as usize;
    let mut door_zones = Vec::with_capacity(doors);
    let mut door_tiles = Vec::with_capacity(doors);
    for _ in 0..doors {
        door_zones.push(r.u16()?);
        door_tiles.push(r.u16()?);
    }
    let name = if flags & 0x80 != 0 { Some(cstr(r.take(32)?)) } else { None };
    // lightmap: i32 zlib size + 4, i32 vertex count, zlib bytes
    let lm = r.u32()?;
    let count = r.u32()?;
    let lightmap = if count != 0 { Some((count, r.take(lm.saturating_sub(4) as usize)?.to_vec())) } else { None };
    // liquid data (`n3Room_t` reader N3 @0x10012803): u32 (n+1)*0x3f1 (anything else: "Room contains broken water data"),
    // n x { u32 kind, u32 nv, nv x vec3 (y snapped to 1 cm), u32 nt, nt x 3 u16 }
    let water = r.u32()?;
    ensure!(water != 0 && water % 0x3f1 == 0, "room contains broken water data ({water:#x})");
    let mut waters = Vec::new();
    for _ in 0..water / 0x3f1 - 1 {
        let kind = r.u32()?;
        let nv = r.u32()? as usize;
        ensure!(nv < 60_000, "implausible room liquid vertex count {nv}");
        let mut verts = Vec::with_capacity(nv + 1);
        for _ in 0..nv {
            let [x, y, z] = r.vec3()?;
            verts.push([x, (y * 100.0 + 0.5).floor() / 100.0, z]);
        }
        let nt = r.u32()? as usize;
        let mut tris = Vec::with_capacity(nt.max(nv));
        for _ in 0..nt {
            let t = [r.u16()?, r.u16()?, r.u16()?];
            ensure!(t.iter().all(|&i| (i as usize) < nv), "room liquid triangle index out of range");
            tris.push(t);
        }
        if kind & 1 == 0 && nv != 3 {
            // a flat polygon is a fan around the vertex centroid (the triangle list of the file is not used)
            let c = verts.iter().fold([0.0f32; 3], |a, v| [a[0] + v[0], a[1] + v[1], a[2] + v[2]]).map(|s| s / nv.max(1) as f32);
            verts.push(c);
            tris = (0..nv).map(|i| [nv as u16, ((i + nv - 1) % nv) as u16, i as u16]).collect();
        }
        waters.push(Water { kind, verts, tris });
    }
    attractors.push(camera_attractors(r, version)?);
    Ok(Room { rot: flags & 3, rect, pos, name, lightmap, door_zones, door_tiles, waters })
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
        d.extend([0u8; 8]); // lightmap size, present
        d.extend((2 * 0x3f1u32).to_le_bytes()); // one liquid polygon: kind 0, a 2 x 2 quad with the y values 1.234 / 1.0
        d.extend([0u8; 4]);
        d.extend(4u32.to_le_bytes());
        for (x, y, z) in [(0.0f32, 1.234, 0.0), (2.0, 1.0, 0.0), (2.0, 1.0, 2.0), (0.0, 1.0, 2.0)] {
            for v in [x, y, z] {
                d.extend(v.to_le_bytes());
            }
        }
        d.extend(2u32.to_le_bytes());
        d.extend([0u8; 12]); // the file's triangle list is ignored for a flat polygon
        d.extend(0u32.to_le_bytes()); // camera attractors
        let r = parse(&d).unwrap();
        assert_eq!(r.rooms.len(), 1);
        let room = &r.rooms[0];
        assert_eq!((room.rot, room.rect, room.pos), (1, [145, 117, 154, 122], [184.0, 107.5, 224.0]));
        assert_eq!(room.name.as_deref(), Some("Ladies' Room"));
        let w = &room.waters[0];
        assert_eq!((w.verts.len(), w.tris.len(), w.verts[0][1]), (5, 4, 1.23));
        assert!(w.verts[4].iter().zip([1.0, 1.0575, 1.0]).all(|(a, b)| (a - b).abs() < 1e-5), "{:?}", w.verts[4]); // centroid; the fan closes on vertex 0
        assert_eq!(w.tris[0], [4, 3, 0]);
    }
}
