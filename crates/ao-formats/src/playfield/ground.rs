//! Heightfield tilemap (rdb 1000009, magic `CHGA`; `RDBTilemap_t` + serialized
//! `AnarchyGroundDataDB_t`, DisplaySystem.dll @ 0x10036f17 / 0x10035df0).
//!
//! ```text
//! "CHGA", u32 size, u32 version (=1), u16 width, u16 height   valid size in cells
//! f32 cell size (m), f32 height scale
//! u16 n; n x u16 tile -> ground texture id (rdb 1010006 / 1010021 / 1010022)
//! u32 (=1), then an object archive (name table, objects) whose first object holds:
//!   map_width, map_height (vertices), map_modulo, tiletexture_count
//!   heightmap_compressed_data / heightmap_small_data / tilemap_compressed_data:
//!     one u32-length-prefixed zlib stream (resp. 4/8 bytes) per patch, row-major patches
//! ```
//! A patch is `m x m` cells with `m = min(2^tz(width-1), 2^tz(height-1), 64)` and `(m+1)^2`
//! height samples. Heights are stored as two nested prefix sums (down columns, then along
//! rows) of bytes (when `heightmap_small_data` has 4 bytes: height = byte << 8, mod 256) or
//! of i16 (8 bytes). The resulting 16 bit value is **unsigned**; metres = value / 256 * height scale.

use std::io::Read;

use anyhow::{anyhow, bail, ensure, Context, Result};
use flate2::read::ZlibDecoder;

use super::record::Rd;

#[derive(Debug)]
pub struct Tilemap {
    /// Valid extent in cells.
    pub cells_x: usize,
    pub cells_z: usize,
    pub cell_size: f32,
    pub height_scale: f32,
    /// Tile index -> ground texture rdb id.
    pub tile_texture: Vec<u16>,
    /// Vertex grid dimensions (`cells + 1` plus padding to a whole number of patches).
    pub verts_x: usize,
    pub verts_z: usize,
    /// Raw unsigned height per vertex, row-major (`z * verts_x + x`).
    pub heights: Vec<u16>,
    /// Raw tile value per cell, row-major (`z * (verts_x - 1) + x`); see [`Tilemap::tile`].
    pub tiles: Vec<u16>,
    /// 0x3fff or 0xff: bits of a raw tile value that form the tile index.
    pub tile_mask: u16,
}

impl Tilemap {
    pub fn height(&self, x: usize, z: usize) -> f32 {
        self.heights[z * self.verts_x + x.min(self.verts_x - 1)] as f32 * (self.height_scale / 256.0)
    }
    pub fn tile(&self, x: usize, z: usize) -> u16 {
        self.tiles[z * (self.verts_x - 1) + x] & self.tile_mask
    }
    /// Ground texture rdb id (1010006/1010021/1010022) under a cell: `tile_texture[tile(x, z)]`
    /// (`n3Tilemap_t::GetMainTileIDs()[idx]`, consumed by [`ground_material`]).
    pub fn ground_texture(&self, x: usize, z: usize) -> Option<u16> {
        self.tile_texture.get(self.tile(x, z) as usize).copied()
    }
}

/// Footstep/landing sound material of a ground texture id (`n3TilemapController_t::GetTileMaterialType` [N3 0x10018420],
/// rdb `1000024`/1 `RDBTileMaterial_t`; loader `RDBTileMaterial_t::ReadBlob` [N3 0x1001c537] string-matches each entry's raw
/// texture-name hash against ~60 known ground texture names and bakes the small material id in place). Transcribed directly
/// from the decoded live blob (format 2, 61 entries; every id/material pair verified by re-hashing the matched name with the
/// same case-insensitive rotating-xor hash as `ao_audio::sbf::sound_id`) rather than the name table, since the resolved ints
/// are smaller and exactly equivalent. An id absent here is 0 (the loader's own unmatched-name default, e.g. rdb tile 937).
const GROUND_MATERIAL: &[(u16, i32)] = &[
    (1, 2), (2, 15), (5, 14), (6, 14), (8, 2), (10, 3), (12, 3), (13, 14), (15, 4), (16, 4), (17, 1), (18, 5), (19, 3), (20, 14),
    (21, 5), (22, 5), (23, 5), (24, 5), (25, 5), (26, 5), (27, 16), (258, 15), (259, 2), (291, 6), (312, 1), (313, 5), (329, 3),
    (330, 14), (364, 14), (395, 2), (397, 3), (409, 1), (410, 1), (411, 14), (437, 17), (463, 2), (473, 14), (476, 10), (548, 3),
    (562, 14), (585, 14), (606, 2), (640, 14), (673, 14), (685, 6), (702, 14), (703, 14), (704, 14), (706, 14), (720, 14), (741, 14),
    (748, 5), (752, 17), (774, 0), (788, 3), (828, 14), (849, 14), (882, 2), (915, 5), (916, 5), (937, 0),
];

pub fn ground_material(texture: u16) -> i32 {
    GROUND_MATERIAL.iter().find(|&&(t, _)| t == texture).map_or(0, |&(_, m)| m)
}

/// Minimal object-archive walker: name table + objects, returns the members of object 1.
fn archive_members<'a>(r: &mut Rd<'a>) -> Result<Vec<(String, &'a [u8])>> {
    let names = r.u32()? as usize;
    ensure!(names < 256, "implausible name table");
    let mut table = Vec::new();
    for _ in 0..names {
        for k in 0..2 {
            let s = take_cstr(r)?;
            if k == 1 {
                table.push(s);
            }
        }
    }
    let objects = r.u32()? as usize;
    ensure!(objects >= 1 && objects < r.d.len(), "implausible object count");
    let mut first = Vec::new();
    for k in 0..=objects {
        r.u32()?; // class id
        r.u32()?; // version
        let n = r.u32()?;
        for _ in 0..n {
            let idx = r.u8()? as usize;
            r.u32()?; // type
            r.u32()?; // element size
            let total = r.u32()? as usize;
            let data = r.take(total)?;
            if k == 1 {
                first.push((table.get(idx).cloned().ok_or_else(|| anyhow!("bad member index"))?, data));
            }
        }
        if k == 1 {
            break;
        }
    }
    Ok(first)
}

fn take_cstr(r: &mut Rd) -> Result<String> {
    let rest = &r.d[r.o..];
    let n = rest.iter().position(|&b| b == 0).ok_or_else(|| anyhow!("unterminated string"))?;
    r.o += n + 1;
    Ok(String::from_utf8_lossy(&rest[..n]).into_owned())
}

fn chunks(mut b: &[u8]) -> Result<Vec<&[u8]>> {
    let mut v = Vec::new();
    while !b.is_empty() {
        ensure!(b.len() >= 4, "truncated chunk list");
        let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
        ensure!(b.len() >= 4 + n, "truncated chunk");
        v.push(&b[4..4 + n]);
        b = &b[4 + n..];
    }
    Ok(v)
}

fn inflate(b: &[u8], expect: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expect);
    ZlibDecoder::new(b).read_to_end(&mut out).context("zlib")?;
    ensure!(out.len() >= expect, "patch stream too short ({} < {expect})", out.len());
    Ok(out)
}

fn trailing_zeros_pow2(v: usize) -> usize {
    v.trailing_zeros() as usize
}

pub fn parse(d: &[u8]) -> Result<Tilemap> {
    let mut r = Rd::new(d, 0);
    ensure!(r.take(4)? == b"CHGA", "not a CHGA tilemap");
    r.u32()?;
    ensure!(r.u32()? == 1, "unsupported CHGA version");
    let (cells_x, cells_z) = (r.u16()? as usize, r.u16()? as usize);
    let cell_size = r.f32()?;
    let height_scale = r.f32()?;
    let n = r.u16()? as usize;
    let tile_texture = (0..n).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
    r.u32()?;
    let members = archive_members(&mut r)?;
    let get = |name: &str| members.iter().find(|m| m.0 == name).map(|m| m.1).ok_or_else(|| anyhow!("missing member {name}"));
    let int = |name: &str| get(name).and_then(|b| b.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).ok_or_else(|| anyhow!("short int {name}")));
    let (vx, vz) = (int("map_width")?, int("map_height")?);
    ensure!(vx > 1 && vz > 1, "empty heightfield");
    let m = 1usize << trailing_zeros_pow2(vx - 1).min(trailing_zeros_pow2(vz - 1)).min(6);
    let (px, pz) = ((vx - 1) / m, (vz - 1) / m);
    let hm = chunks(get("heightmap_compressed_data")?)?;
    let small = chunks(get("heightmap_small_data")?)?;
    let tm = chunks(get("tilemap_compressed_data")?)?;
    ensure!(hm.len() == px * pz && small.len() == hm.len() && tm.len() == hm.len(), "patch count mismatch");
    let mut wide = false;
    let mut heights = vec![0u16; vx * vz];
    let mut tiles = vec![0u16; (vx - 1) * (vz - 1)];
    let s = m + 1;
    for (i, ((h, sm), t)) in hm.iter().zip(&small).zip(&tm).enumerate() {
        let (gx, gz) = (i % px, i / px);
        let mut a = vec![0u16; s * s];
        match sm.len() {
            4 => {
                let raw = inflate(h, s * s)?;
                for (o, &b) in a.iter_mut().zip(&raw) {
                    *o = b as u16;
                }
            }
            8 => {
                wide = true;
                let raw = inflate(h, 2 * s * s)?;
                for (o, c) in a.iter_mut().zip(raw.as_chunks::<2>().0) {
                    *o = u16::from_le_bytes([c[0], c[1]]);
                }
            }
            n => bail!("unexpected heightmap_small_data length {n}"),
        }
        // prefix sums: down every column, then along every row (wrapping at the sample width)
        let byte = sm.len() == 4;
        for x in 0..s {
            let mut acc = 0u16;
            for z in 0..s {
                acc = acc.wrapping_add(a[z * s + x]);
                if byte {
                    acc &= 0xff;
                }
                a[z * s + x] = acc;
            }
        }
        for z in 0..s {
            let mut acc = 0u16;
            for x in 0..s {
                acc = acc.wrapping_add(a[z * s + x]);
                if byte {
                    acc &= 0xff;
                }
                a[z * s + x] = if byte { acc << 8 } else { acc };
            }
        }
        for z in 0..s {
            let dst = (gz * m + z) * vx + gx * m;
            heights[dst..dst + s].copy_from_slice(&a[z * s..z * s + s]);
        }
        let raw = inflate(t, 2 * m * m)?;
        for z in 0..m {
            let dst = (gz * m + z) * (vx - 1) + gx * m;
            for x in 0..m {
                let o = 2 * (z * m + x);
                tiles[dst + x] = u16::from_le_bytes([raw[o], raw[o + 1]]);
            }
        }
    }
    // RDBTilemap: the tile index mask is 0x3fff when `tile_type_data` exists, 0xff otherwise
    // (DisplaySystem.dll @ 0x10036f17 sets AnarchyGroundData+0x60).
    let tile_mask = if members.iter().any(|m| m.0 == "tile_type_data" && m.1.len() > 4) { 0x3fff } else { 0xff };
    // metres = sample * scale for both widths: 8 bit samples are stored `<< 8` (so `/ 256` in `height`); 16 bit maps (22 records:
    // 556, 656, 4364, 4380-4388, 4894, 6001, 6012, 6022 ...) carry scales of 0.2..0.5 / 256 (= 0.00078..0.002) for the raw sample
    let height_scale = if wide { height_scale * 256.0 } else { height_scale };
    Ok(Tilemap { cells_x: cells_x.min(vx - 1), cells_z: cells_z.min(vz - 1), cell_size, height_scale, tile_texture, tile_mask, verts_x: vx, verts_z: vz, heights, tiles })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    fn z(b: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }
    fn blob(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.iter().flat_map(|p| (p.len() as u32).to_le_bytes().into_iter().chain(p.iter().copied())).collect()
    }
    fn member(out: &mut Vec<u8>, idx: u8, ty: u32, data: &[u8]) {
        out.push(idx);
        for v in [ty, 4, data.len() as u32] {
            out.extend(v.to_le_bytes());
        }
        out.extend(data);
    }

    /// Smallest valid map: 3 x 3 vertices, 2 x 2 cells, one 2 x 2 patch, 8 bit heights.
    fn fixture(raw_heights: &[u8]) -> Vec<u8> {
        let names = ["map_width", "map_height", "map_modulo", "tiletexture_count", "heightmap_compressed_data", "heightmap_small_data", "tilemap_compressed_data"];
        let mut d = b"CHGA".to_vec();
        for v in [0u32, 1] {
            d.extend(v.to_le_bytes());
        }
        for v in [2u16, 2] {
            d.extend(v.to_le_bytes());
        }
        for v in [4.0f32, 0.2] {
            d.extend(v.to_le_bytes());
        }
        d.extend(2u16.to_le_bytes());
        d.extend([7, 0, 9, 0]); // tile -> texture ids
        d.extend(1u32.to_le_bytes());
        d.extend((names.len() as u32).to_le_bytes());
        for n in names {
            d.extend(b"0\0".iter().chain(n.as_bytes()).chain(&[0u8]).copied());
        }
        d.extend(1u32.to_le_bytes()); // object count
        for v in [25u32, 1, 0] {
            d.extend(v.to_le_bytes()); // root holder with no members
        }
        for v in [7u32, 1, 6] {
            d.extend(v.to_le_bytes()); // ground object, 7 members
        }
        for i in 0..3 {
            member(&mut d, i, 3, &3u32.to_le_bytes());
        }
        member(&mut d, 4, 9, &blob(&[z(raw_heights)]));
        member(&mut d, 5, 9, &blob(&[vec![0, 0, 0, 0]]));
        let tiles: Vec<u8> = [1u16, 0, 0x4001, 0].iter().flat_map(|t| t.to_le_bytes()).collect();
        member(&mut d, 6, 9, &blob(&[z(&tiles)]));
        d
    }

    #[test]
    fn prefix_sums_and_patch_layout() {
        // first sample 10, rest 0 -> both prefix sums give a constant 10 (<< 8) everywhere
        let mut raw = vec![0u8; 9];
        raw[0] = 10;
        let tm = parse(&fixture(&raw)).unwrap();
        assert_eq!((tm.cells_x, tm.cells_z, tm.verts_x, tm.verts_z), (2, 2, 3, 3));
        assert_eq!(tm.tile_texture, vec![7, 9]);
        assert!(tm.heights.iter().all(|&h| h == 10 << 8));
        assert_eq!((tm.tile(0, 0), tm.tile(0, 1)), (1, 1)); // 0x4001 & 0x3fff
        assert!((tm.height(1, 1) - 10.0 * 0.2).abs() < 1e-4);
        // byte arithmetic wraps: 200 + 100 -> 44 in the column pass
        let mut raw = vec![0u8; 9];
        raw[0] = 200;
        raw[3] = 100;
        let tm = parse(&fixture(&raw)).unwrap();
        assert_eq!(tm.heights[3] >> 8, 44);
    }

    /// N3 `10017c3e` promotes the sample with `&0xffff` and multiplies the stored
    /// height scale. Statel `1002435c` passes raw XYZ to SetPosition; only dungeon
    /// loader `100275f6` applies the room transform (`100271b6`).
    #[test]
    fn native_wide_maps_preserve_geometry_and_authored_statel_origins() {
        use super::super::{list_playfields, load_playfield, record, statel, terrain_height, RECORD, STATELS, TILEMAP};
        use std::collections::HashSet;
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Ok(store) = ao_rdb::RecordStore::open(&dir) else { return };
        let mut audited = Vec::new();
        let mut high_bit_samples = 0;
        for (id, _) in list_playfields(&store).unwrap() {
            let record = record::parse(&store.get(RECORD, id).unwrap().unwrap()).unwrap();
            if !record.is_outdoor() { continue; }
            let raw = store.get(TILEMAP, record.tilemap).unwrap().unwrap();
            let mut reader = Rd::new(&raw, 24);
            let textures = reader.u16().unwrap() as usize;
            reader.skip(textures * 2 + 4).unwrap();
            let members = archive_members(&mut reader).unwrap();
            let small = chunks(members.iter().find(|(name, _)| name == "heightmap_small_data").unwrap().1).unwrap();
            if !small.iter().any(|data| data.len() == 8) { continue; }
            assert!(small.iter().all(|data| data.len() == 8), "native wide-family flag must cover every patch in {id}");
            let native_scale = f32::from_le_bytes(raw[20..24].try_into().unwrap());
            let map = parse(&raw).unwrap();
            high_bit_samples += map.heights.iter().filter(|&&height| height >= 0x8000).count();
            for z in 0..=map.cells_z {
                for x in 0..=map.cells_x {
                    assert_eq!(map.height(x, z).to_bits(), (map.heights[z * map.verts_x + x] as f32 * native_scale).to_bits(), "native unsigned sample formula pf{id} ({x},{z})");
                }
            }
            // Independent authored corner cache, loaded as four ushort words by DS10036f17.
            let patch = 1usize << (map.verts_x - 1).trailing_zeros().min((map.verts_z - 1).trailing_zeros()).min(6);
            let across = (map.verts_x - 1) / patch;
            for (index, corners) in small.iter().enumerate() {
                let (x, z) = (index % across * patch, index / across * patch);
                for (corner, (dx, dz)) in [(0, 0), (patch, 0), (0, patch), (patch, patch)].into_iter().enumerate() {
                    let expected = u16::from_le_bytes(corners[corner * 2..corner * 2 + 2].try_into().unwrap());
                    assert_eq!(map.heights[(z + dz) * map.verts_x + x + dx], expected, "native patch corner pf{id} patch{index} corner{corner}");
                }
            }
            let file = statel::parse(&store.get(STATELS, id).unwrap().unwrap(), record.count as usize, statel::Layout::Outdoor).unwrap();
            assert!(file.outdoor);
            let native_positions: HashSet<_> = std::iter::once(&file.global).chain(&file.zones).flat_map(|zone| &zone.statels).filter(|statel| statel.mesh != 0)
                .map(|statel| [statel.pos[0].to_bits(), statel.pos[1].to_bits(), (-statel.pos[2]).to_bits()]).collect();
            let scene = load_playfield(&store, &dir, id).unwrap();
            let lod = scene.statel_lod.as_ref().expect("outdoor statel instance ownership");
            assert!(!lod.items.is_empty(), "wide map{id} must exercise placed geometry");
            let mut gaps = Vec::new();
            for item in &lod.items {
                for instance in std::iter::once(item.full).chain(item.reduced) {
                    let position = scene.instances[instance].transform[3];
                    assert!(native_positions.contains(&[position[0].to_bits(), position[1].to_bits(), position[2].to_bits()]), "pf{id}: statel position was snapped, offset or room-transformed: {position:?}");
                }
                let p = scene.instances[item.full].transform[3];
                if let Some(height) = terrain_height(&map, p[0], p[2]) { gaps.push((p[1] - height).abs()); }
            }
            gaps.sort_by(f32::total_cmp);
            eprintln!("native wide pf{id}: coordinate mismatches=0, statels={}, diagnostic median origin/terrain gap={:?}", lod.items.len(), gaps.get(gaps.len() / 2));
            audited.push(id);
        }
        assert!(high_bit_samples > 0, "the unsigned conversion regression must exercise bit15");
        audited.sort_unstable();
        assert_eq!(audited, [556, 656, 4364, 4365, 4366, 4367, 4368, 4380, 4381, 4382, 4383, 4384, 4385, 4386, 4387, 4388, 4468, 4894, 6001, 6002, 6012, 6022]);
    }

    #[test]
    fn ground_material_resolves_known_footstep_textures_and_defaults_unknown_ids_to_zero() {
        assert_eq!(ground_material(1), 2, "sand");
        assert_eq!(ground_material(2), 15, "grass");
        assert_eq!(ground_material(291), 6, "drygrass");
        assert_eq!(ground_material(476), 10, "shadow1_lava");
        assert_eq!(ground_material(774), 0, "shadowbrink, its own explicit zero");
        assert_eq!(ground_material(9999), 0, "unlisted texture id: the loader's own unmatched-name default");
    }
}
