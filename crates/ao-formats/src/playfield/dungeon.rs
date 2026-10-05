//! Dungeon room shells: floors, walls and ceilings of indoor playfields.
//!
//! A dungeon tilemap (rdb 1000009, magic `GNDA`, `RDBTilemap_t::ReadBlob`, DisplaySystem.dll
//! @ 0x10078391) is one atlas of all room templates; a room uses the window `rect` of it. The
//! client (`n3GroundRenderer_t::CreateDungeonRoom`, N3.dll @ 0x10008563) builds a room from
//! prefabricated cell meshes (rdb 1010013, [`Piece`]) chosen by the autotiling
//! `n3Tilemap_t::FUN_100169ab` (N3.dll) and bent by the height layers (`DungeonMesh_t`, N3.dll
//! @ 0x10001a91 / @ 0x10001c3b). See `docs/formats.md` § playfields / Dungeon rooms.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};
use std::rc::Rc;

use anyhow::{anyhow, ensure, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Instance, Mesh, Scene, Submesh, Texture, TextureKey, Vertex, IDENTITY};

use super::record::{Rd, Record, Room};
use super::spawn::Spot;
use super::{statel, TILEMAP};

/// rdb type of the cell meshes.
const PIECES: u32 = 1_010_013;
/// Dungeon wall/floor textures (material id without the high bit); the high bit selects the
/// common object texture family instead.
const DUNGEON_TEXTURES: u32 = 1_010_009;
const OBJECT_TEXTURES: u32 = 1_010_004;

/// Autotile keys (`DAT_1005bce0`, N3.dll): canonical wall mask (+ door flags `0x70`) -> slot
/// `index + 1` of the per tile mesh row. Rotation extras of the table are all 0.
const KEYS: [u8; 14] = [0x0f, 0x07, 0x05, 0x03, 0x01, 0x00, 0x31, 0x51, 0x71, 0x1d, 0x33, 0x11, 0x0f, 0x1f];

/// Parsed `GNDA` tilemap.
pub(super) struct Gnda {
    pub w: usize,
    pub h: usize,
    pub cell: f32,
    pub height_scale: f32,
    /// Tile ids; the grid stores indices into this list.
    pub ids: Vec<u16>,
    /// `[tile][neighbour][16]` mesh ids, flattened (`(tile * 32 + nb) * 16 + idx`).
    table: Vec<u16>,
    /// Tile index per cell (bit 7 = door/special flag).
    pub ty: Vec<u8>,
    /// Height of each tile template in metres (`HSTA` chunk, `u16 / 10.0`; ReadBlob @0x10078391), indexed like `ids`.
    pub tile_height: Vec<f32>,
    /// Floor height layer (`DHGA`), units of `height_scale`.
    pub floor: Vec<u8>,
    /// Ceiling stretch layer (`HCDA`), units of `height_scale`.
    pub ceil: Vec<u8>,
    /// Per tile mesh overrides selected by `taha >> 2` (1 based).
    taha_lists: Vec<Vec<u16>>,
    taha: Vec<u8>,
    /// Material ids (`XTHA`).
    pub mats: Vec<u32>,
    pub layers: usize,
    /// `layers` material indices per cell.
    layer: Vec<u8>,
    /// Corner post flags per cell, see [`Gnda::compute_cells`].
    corners: Vec<u8>,
    /// Final autotile state per cell, see [`Gnda::compute_cells`].
    state: Vec<u8>,
    /// Door edges (`IRHA`): `cell * 2 + orientation` (0: with x+1, 1: with z+1).
    doors: Vec<u32>,
}

fn png(b: &[u8], w: usize, h: usize, channels: usize) -> Result<Vec<u8>> {
    let img = image::load_from_memory_with_format(b, image::ImageFormat::Png).context("tilemap png")?;
    ensure!(img.width() as usize == w && img.height() as usize == h, "layer size {}x{} != {w}x{h}", img.width(), img.height());
    Ok(if channels == 1 { img.into_luma8().into_raw() } else { img.into_rgb8().into_raw() })
}

pub(super) fn parse_gnda(d: &[u8]) -> Result<Gnda> {
    let mut r = Rd::new(d, 0);
    ensure!(r.take(4)? == b"GNDA", "not a GNDA tilemap");
    r.u32()?;
    ensure!(r.u32()? == 1, "unsupported GNDA version");
    let (w, h) = (r.u16()? as usize, r.u16()? as usize);
    let (cell, height_scale) = (r.f32()?, r.f32()?);
    let n = r.u16()? as usize;
    ensure!(n <= 32 && w * h > 0 && w * h < 1 << 24, "implausible GNDA header ({n} tiles, {w}x{h})");
    let ids = (0..n).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
    let index = |id: u16| ids.iter().position(|&i| i == id).unwrap_or(0);
    let mut table = vec![0u16; n * 32 * 16];
    for t in 0..n {
        let (a, b) = (r.u8()?, r.u8()?);
        for _ in 0..a {
            let nb = index(r.u16()?);
            // 16 words: stream positions 0..4 -> slots 0..4, 5..14 -> 6..15 (slot 5 is the tile id)
            for k in 0..16 {
                let v = r.u16()?;
                let idx = if k < 5 { k } else { k + 1 };
                if idx < 16 {
                    table[(t * 32 + nb) * 16 + idx] = v;
                }
            }
        }
        r.skip(b as usize * 2)?; // random floor variants; never present in the shipped data
        table[(t * 32 + t) * 16 + 5] = ids[t];
    }
    let mut g = Gnda {
        w,
        h,
        cell,
        height_scale,
        ids,
        table,
        ty: vec![0; w * h],
        tile_height: Vec::new(),
        floor: vec![0; w * h],
        ceil: vec![0; w * h],
        taha_lists: vec![Vec::new(); 32],
        taha: vec![0; w * h],
        mats: Vec::new(),
        layers: 0,
        layer: Vec::new(),
        corners: Vec::new(),
        state: Vec::new(),
        doors: Vec::new(),
    };
    while r.d.len() - r.o >= 12 {
        let start = r.o;
        let tag: [u8; 4] = r.take(4)?.try_into().unwrap();
        let size = r.u32()? as usize;
        let version = r.u32()?;
        if &tag == b"HSTA" {
            // unlike the other sections the size excludes the 12 byte header (24 = 12 tile heights)
            let end = (start + 12 + size).min(d.len());
            g.tile_height = d[(start + 12).min(end)..end].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c) as f32 / 10.0).collect();
            r.o = (end + 3) & !3;
            continue;
        }
        if !tag.iter().all(u8::is_ascii_uppercase) || size < 12 || start + size > d.len() {
            break; // trailing bytes after the last section
        }
        let body = &d[start + 12..start + size];
        match &tag {
            b"DCGA" => g.ty = png(body, w, h, 1)?,
            b"DHGA" => g.floor = png(body, w, h, 1)?,
            b"HCDA" => g.ceil = png(body, w, h, 1)?, // stretch is a signed byte: the loader (FUN_10077e6f) adds 0x20 to each byte, the warp subtracts 0x20 again
            b"TAHA" => {
                let mut s = Rd::new(body, 0);
                for _ in 0..s.u16()? {
                    let (t, c) = (s.u8()? as usize, s.u8()? as usize);
                    let list = (0..c).map(|_| s.u16()).collect::<Result<Vec<_>>>()?;
                    if t < 32 {
                        g.taha_lists[t] = list;
                    }
                }
                g.taha = png(&body[s.o..], w, h, 1)?;
            }
            b"XTHA" => {
                let mut s = Rd::new(body, 0);
                let count = s.u16()? as usize;
                for _ in 0..count {
                    g.mats.push(if version == 1 { s.u16()? as u32 } else { s.u32()? });
                }
                g.layers = if body.get(s.o + 1..s.o + 4) == Some(b"PNG") { 3 } else { s.u16()? as usize };
                g.layer = vec![0; w * h * g.layers];
                let mut rest = &body[s.o..];
                for base in (0..g.layers).step_by(3) {
                    let end = rest.windows(8).position(|x| x == b"IEND\xae\x42\x60\x82").ok_or_else(|| anyhow!("unterminated layer png"))? + 8;
                    let take = (g.layers - base).min(3);
                    let px = png(&rest[..end], w, h, 3)?;
                    for (i, c) in px.as_chunks::<3>().0.iter().enumerate() {
                        g.layer[i * g.layers + base..i * g.layers + base + take].copy_from_slice(&c[..take]);
                    }
                    rest = &rest[end..];
                }
            }
            b"IRHA" => g.doors = body.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect(),
            _ => {}
        }
        r.o = (start + size + (4 - (size & 3)) % 4).min(d.len()); // sections are padded to 4 bytes (by size)
    }
    ensure!(g.layers > 0 && !g.mats.is_empty(), "GNDA without material layers");
    g.compute_cells();
    Ok(g)
}

/// One cell mesh (rdb 1010013): `u16 count`, `count` x { `u16 n`, n x (pos, normal, uv) f32,
/// `u16 index_count`, indices }. Block `i` is drawn with the material of layer `i`.
#[derive(Default)]
pub(super) struct Piece {
    blocks: Vec<(Vec<[f32; 8]>, Vec<u16>)>,
}

pub(super) fn parse_piece(d: &[u8]) -> Result<Piece> {
    let mut r = Rd::new(d, 0);
    let mut blocks = Vec::new();
    for _ in 0..r.u16()? {
        let n = r.u16()? as usize;
        let verts = (0..n)
            .map(|_| -> Result<[f32; 8]> {
                let mut v = [0.0; 8];
                for x in &mut v {
                    *x = r.f32()?;
                }
                Ok(v)
            })
            .collect::<Result<Vec<_>>>()?;
        let m = r.u16()? as usize;
        let idx = (0..m).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
        ensure!(m.is_multiple_of(3) && idx.iter().all(|&i| (i as usize) < n), "bad cell mesh block");
        blocks.push((verts, idx));
    }
    Ok(Piece { blocks })
}

/// Canonical (minimal) 4 bit wall mask under right rotation, and the number of rotations.
fn canon(mask: u8) -> (u8, u8) {
    let (mut best, mut rot, mut u) = (0xff, 0, mask & 0xf);
    for r in 0..4 {
        if u < best {
            (best, rot) = (u, r);
        }
        u = (u >> 1) + (u & 1) * 8;
    }
    (best, rot)
}

impl Gnda {
    fn at(&self, x: i32, z: i32) -> usize {
        z.clamp(0, self.h as i32 - 1) as usize * self.w + x.clamp(0, self.w as i32 - 1) as usize
    }

    /// Tile index (without flag bit) of a cell; outside the map = 0 (empty).
    fn tile(&self, x: i32, z: i32) -> u8 {
        if x < 0 || z < 0 || x >= self.w as i32 || z >= self.h as i32 {
            0
        } else {
            self.ty[z as usize * self.w + x as usize] & 0x7f
        }
    }

    fn raw(&self, x: i32, z: i32) -> u8 {
        if x < 0 || z < 0 || x >= self.w as i32 || z >= self.h as i32 {
            0
        } else {
            self.ty[z as usize * self.w + x as usize]
        }
    }

    fn taha_at(&self, x: i32, z: i32) -> u8 {
        if x < 0 || z < 0 || x >= self.w as i32 || z >= self.h as i32 {
            0
        } else {
            self.taha[z as usize * self.w + x as usize]
        }
    }

    /// Mesh id and rotation of cell `(x, z)` from the autotile state of [`Gnda::compute_cells`].
    fn cell_mesh(&self, x: i32, z: i32) -> Option<(u16, u8)> {
        let t = self.tile(x, z) as usize;
        if t == 0 || t >= self.ids.len() {
            return None;
        }
        let state = self.state[z as usize * self.w + x as usize];
        let (canonical, rot) = canon(state & 0xf);
        let key = canonical | (state & 0x70);
        let slot = KEYS.iter().position(|&k| k == key).map_or(0, |i| i + 1);
        let nb = if state == 0 { t } else { 0 };
        // `FUN_10016967` reads word `byte` of the row; a key that is not in `KEYS` has byte 0, i.e. the word right
        // before the row (slot 15 of the previous row; the client's table is `u16[tile][32][16]` starting at +0x23c)
        let row = (t * 32 + nb) * 16;
        let mut mesh = if slot > 0 { self.table[row + slot - 1] } else { self.table[row - 1] };
        let over = self.taha_at(x, z) >> 2;
        if over != 0 {
            if let Some(&m) = self.taha_lists[t].get(over as usize - 1) {
                mesh = m;
            }
        }
        Some((mesh, rot))
    }

    /// The flat floor piece of a tile (the tile id doubles as its mesh id).
    fn floor_mesh(&self, t: usize) -> u16 {
        self.table[(t * 32 + t) * 16 + 5]
    }

    /// Corner post piece of a tile (`GetTileTypeNoWalls`, neighbour 0, key `0x1f`).
    fn corner_mesh(&self, t: usize) -> u16 {
        self.table[(t * 32) * 16 + 13]
    }

    /// The autotiling sweep of `n3Tilemap_t::FUN_100169ab` (dungeon branch, N3.dll @ 0x100169ab)
    /// over the whole map. Fills
    /// * `state`: per cell wall mask (bit 0: z+1, 1: x+1, 2: z-1, 3: x-1 closed) plus door frame
    ///   flags `0x10/0x20/0x40`,
    /// * `corners`: bit `c` = corner post at corner `c` (0: -x-z, 1: -x+z, 2: +x+z, 3: +x-z). A
    ///   cell claims the corners away from its walls; two diagonal claims on one grid vertex cancel,
    ///   so posts remain at concave room corners.
    ///
    /// Rows are swept from the last image row up (the client works in flipped rows). Walls that a
    /// cell adds towards a flagged (`0x80`) partition neighbour are passed on to the neighbour
    /// through the sweep's carries (`up`, `left`), as in the client.
    fn compute_cells(&mut self) {
        const CLAIM: [u8; 16] = [0x0f, 0x09, 0x03, 0x01, 0x06, 0x00, 0x02, 0x00, 0x0c, 0x08, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00];
        let (w, h) = (self.w as i32, self.h as i32);
        let at = |x: i32, z: i32| z as usize * self.w + x as usize;
        let mut door = vec![false; self.w * self.h];
        for &v in &self.doors {
            let i = (v >> 1) as usize;
            let j = i + if v & 1 == 0 { 1 } else { self.w };
            for k in [i, j] {
                if k < door.len() {
                    door[k] = true;
                }
            }
        }
        let is_door = |x: i32, z: i32| x >= 0 && z >= 0 && x < w && z < h && door[at(x, z)];
        let mut n = vec![0u8; self.w * self.h];
        let mut state = vec![0u8; self.w * self.h];
        let mut up = vec![0u8; self.w];
        for z in (0..h).rev() {
            let mut left = 0u8;
            for x in 0..w {
                let val = self.raw(x, z);
                let mut m = 0u8;
                if val & 0x7f != 0 {
                    let closed = |empty: bool, bit: u8| if empty { bit } else { 0 };
                    m = closed(up[x as usize] & 0x7f == 0, 1) | closed(self.tile(x + 1, z) == 0, 2) | closed(self.tile(x, z - 1) == 0, 4) | closed(left & 0x7f == 0, 8);
                    let flagged = val & 0x80 != 0;
                    if flagged && self.raw(x, z + 1) & 0x80 != 0 {
                        m |= 2;
                    }
                    if flagged && self.raw(x - 1, z) & 0x80 != 0 {
                        m |= 4;
                    }
                    if is_door(x, z) {
                        let mut side = 0;
                        for (bit, (dx, dz)) in [(1, (0, 1)), (2, (1, 0)), (4, (0, -1)), (8, (-1, 0))] {
                            if m & bit != 0 && is_door(x + dx, z + dz) {
                                side = bit;
                            }
                        }
                        if side != 0 {
                            m |= 0x10;
                            if self.taha_at(x, z) & 1 != 0 {
                                let (zp, xp, zm, xm) = (self.taha_at(x, z + 1) & 1, self.taha_at(x + 1, z) & 1, self.taha_at(x, z - 1) & 1, self.taha_at(x - 1, z) & 1);
                                let mut v = 7u8;
                                match side {
                                    1 => {
                                        if xm == 0 {
                                            v = 1;
                                        }
                                        if xp == 0 {
                                            v &= 0xfc;
                                        }
                                    }
                                    2 => {
                                        if zp == 0 {
                                            v &= 0xf9;
                                        }
                                        if zm == 0 {
                                            v &= 0xfc;
                                        }
                                    }
                                    4 => {
                                        if xp == 0 {
                                            v &= 0xf9;
                                        }
                                        if xm == 0 {
                                            v &= 0xfc;
                                        }
                                    }
                                    _ => {
                                        if zm == 0 {
                                            v &= 0xf9;
                                        }
                                        if zp == 0 {
                                            v &= 0xfc;
                                        }
                                    }
                                }
                                match v {
                                    7 => m = (m & !0x20) | 0x50,
                                    1 => m = (m & !0x40) | 0x30,
                                    4 => m |= 0x70,
                                    _ => {}
                                }
                            }
                        }
                    }
                    let mut c = CLAIM[(m & 0xf) as usize];
                    if c & 2 != 0 && x > 0 && z + 1 < h && n[at(x - 1, z + 1)] & 8 != 0 {
                        n[at(x - 1, z + 1)] &= !8;
                        c &= !2;
                    }
                    if c & 4 != 0 && x + 1 < w && z + 1 < h && n[at(x + 1, z + 1)] & 1 != 0 {
                        n[at(x + 1, z + 1)] &= !1;
                        c &= !4;
                    }
                    n[at(x, z)] = c;
                    if self.taha_at(x, z) & 3 == 2 {
                        m = 0;
                    }
                }
                state[at(x, z)] = m;
                left = if m & 2 != 0 { 0 } else { val };
                up[x as usize] = if m & 4 != 0 { 0 } else { val };
            }
        }
        self.state = state;
        self.corners = n;
    }
}

/// Ceiling stretch of an `HCDA` byte in height-scale units: a signed value. The loader (`FUN_10077e6f`, DisplaySystem
/// @0x10077e6f) adds 0x20 to every byte (wrapping), `FUN_10001c3b` subtracts 0x20 again from the unsigned byte, so
/// raw 242..255 mean -14..-1 and the range is -32..223.
pub(super) fn stretch(raw: u8) -> f64 {
    raw.wrapping_add(0x20) as f64 - 32.0
}

/// Catmull-Rom style slope polynomial (`FUN_100023ad` / `FUN_10002479`): `c = [c0..c5]`.
fn slope(p: [f64; 4], da: f64, d3: f64, swap: bool) -> [f64; 6] {
    let f = 0.25;
    // x version pairs (p1, p2) / (p3, p4); z version pairs (p1, p3) / (p2, p4)
    let (e0, e1, e2, e3) = if swap { (p[0], p[2], p[1], p[3]) } else { (p[0], p[1], p[2], p[3]) };
    let t5 = f * e0;
    let t4 = f * e2;
    let v7 = f * e1 - t5;
    let c1 = (2.0 * f * da - t5) * 4.0 - v7;
    let c0 = v7 - c1;
    let v8 = f * e3 - t4;
    let dv2 = ((d3 + da) * 2.0 * f - t4) * 4.0 - v8;
    [c0, c1, t5, (v8 - dv2) - c0, dv2 - c1, t4 - t5]
}

fn eval_u(c: &[f64; 6], u: f64, v: f64) -> f64 {
    v * c[5] + ((v * c[4] + (v * c[3] + c[0]) * u + c[1]) * u + c[2])
}

fn eval_v(c: &[f64; 6], u: f64, v: f64) -> f64 {
    u * c[5] + ((u * c[4] + (u * c[3] + c[0]) * v + c[1]) * v + c[2])
}

/// Bilinear patch over the corner values of one height layer plus its slope polynomials.
struct Surface {
    d: [f64; 4],
    sx: [f64; 6],
    sz: [f64; 6],
}

impl Surface {
    /// `g(x, z)` = layer value at tile `(x, z)`; the patch of cell `(x, z)` spans corners `x-1..x`, `z-1..z`.
    fn new(g: impl Fn(i32, i32) -> f64, x: i32, z: i32) -> Surface {
        let a00 = g(x - 1, z - 1);
        let d0 = g(x, z - 1) - a00;
        let d1 = g(x - 1, z) - a00;
        let d3 = (g(x, z) - a00) - d0 - d1;
        let px = [g(x, z - 1) - g(x - 2, z - 1), g(x + 1, z - 1) - g(x - 1, z - 1), g(x, z) - g(x - 2, z), g(x + 1, z) - g(x - 1, z)];
        let pz = [g(x - 1, z) - g(x - 1, z - 2), g(x, z) - g(x, z - 2), g(x - 1, z + 1) - g(x - 1, z - 1), g(x, z + 1) - g(x, z - 1)];
        Surface { d: [d0, d1, a00, d3], sx: slope(px, d0, d3, false), sz: slope(pz, d1, d3, true) }
    }

    fn height(&self, u: f64, v: f64) -> f64 {
        u * self.d[0] + (u * self.d[3] + self.d[1]) * v + self.d[2]
    }
}

/// Output of one room in scene space.
struct RoomBuf {
    vertices: Vec<Vertex>,
    by_tex: HashMap<Option<TextureKey>, Vec<u32>>,
    /// Vertex count at the start of every swept cell (the `A[]` ring of `CreateDungeonRoom`, N3 @0x10008563).
    cell_start: Vec<usize>,
    /// Room width in cells (ring length).
    width: usize,
}

/// Room placement (`Placer::place` convention) and the AO -> scene mirror.
struct Frame {
    rot: [[f32; 3]; 3],
    pos: [f32; 3],
}

impl Frame {
    fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let q = self.vec(p);
        [q[0] + self.pos[0], q[1] + self.pos[1], -(q[2] + self.pos[2])]
    }

    fn vec(&self, p: [f32; 3]) -> [f32; 3] {
        let r = &self.rot;
        [r[0][0] * p[0] + r[0][1] * p[1] + r[0][2] * p[2], r[1][0] * p[0] + r[1][1] * p[1] + r[1][2] * p[2], r[2][0] * p[0] + r[2][1] * p[1] + r[2][2] * p[2]]
    }
}

struct Builder<'a> {
    g: &'a Gnda,
    frame: Frame,
    /// Texture of material index `i` (loaded on first use).
    textures: &'a mut dyn FnMut(u8) -> Option<TextureKey>,
    out: RoomBuf,
}

impl Builder<'_> {
    /// Adds the blocks of `piece` for cell `(x, z)` (only block 0, the floor, if `only_floor`),
    /// rotated by `rot` quarter turns.
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, piece: &Piece, rot: u8, x: i32, z: i32, origin: [f32; 2], yoff: f64, only_floor: bool) {
        let g = self.g;
        let hs = g.height_scale as f64;
        let floor = Surface::new(|px, pz| g.floor[g.at(px, pz)] as f64 * hs + yoff, x, z);
        let ceil = Surface::new(|px, pz| stretch(g.ceil[g.at(px, pz)]) * hs, x, z);
        let theta = rot as f64 * FRAC_PI_2 + PI;
        let (s, c) = theta.sin_cos();
        let cell = g.cell as f64;
        let layer = &g.layer[(z as usize * g.w + x as usize) * g.layers..][..g.layers];
        for (bi, (verts, tris)) in piece.blocks.iter().enumerate() {
            if verts.is_empty() || (only_floor && bi > 0) {
                continue;
            }
            let mat = layer.get(bi).and_then(|&m| (self.textures)(m));
            let mut block = Vec::with_capacity(verts.len());
            for v in verts {
                let (px, pz) = (v[0] as f64 * c + v[2] as f64 * s, -(v[0] as f64) * s + v[2] as f64 * c);
                let (nx, nz) = (v[3] as f64 * c + v[5] as f64 * s, -(v[3] as f64) * s + v[5] as f64 * c);
                let ny = v[4] as f64;
                let y = v[1] as f64;
                let (u, w) = (px / cell + 0.5, pz / cell + 0.5);
                let gx = eval_u(&floor.sx, u, w) + y * 0.25 * eval_u(&ceil.sx, u, w);
                let gz = eval_v(&floor.sz, u, w) + y * 0.25 * eval_v(&ceil.sz, u, w);
                let (nx, nz) = (nx - ny * gx, nz - ny * gz);
                let l = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-9);
                let y2 = y + floor.height(u, w) + y * 0.25 * ceil.height(u, w);
                let p = self.frame.point([px as f32 + origin[0], y2 as f32, pz as f32 + origin[1]]);
                let n = self.frame.vec([(nx / l) as f32, (ny / l) as f32, (nz / l) as f32]);
                block.push(Vertex { pos: p, normal: [n[0], n[1], -n[2]], uv: [v[6], v[7]], ..Default::default() });
            }
            let remap = weld_block(&mut self.out, &mut block);
            // z is mirrored: reverse the winding
            let idx = self.out.by_tex.entry(mat).or_default();
            for t in tris.as_chunks::<3>().0 {
                let [a, b, c] = [remap[t[0] as usize], remap[t[2] as usize], remap[t[1] as usize]];
                if a != b && b != c && a != c {
                    idx.extend([a, b, c]);
                }
            }
        }
    }
}

/// Lowest floor of the room becomes local y = 0 (`RDBPlayfield_t::CalculateRoomHeights`).
pub(super) fn floor_min(g: &Gnda, room: &Room) -> Option<f32> {
    let [x1, z1, x2, z2] = room.rect.map(|v| v as i32);
    let m = (z1..z2).flat_map(|z| (x1..x2).map(move |x| (x, z))).filter(|&(x, z)| g.tile(x, z) != 0).map(|(x, z)| g.floor[g.at(x, z)] as f32 * g.height_scale).fold(f32::MAX, f32::min);
    (m != f32::MAX).then_some(m)
}

/// Local position of the centre of the room's first cell.
fn room_origin(g: &Gnda, room: &Room) -> (f32, f32) {
    let half = |n: i32| (((n - 1) & !1) + 1) as f32 * 0.5 * g.cell;
    (-half(room.rect[2] as i32 - room.rect[0] as i32), -half(room.rect[3] as i32 - room.rect[1] as i32))
}

/// Camera inside `room`: eye height `EYE_H` on an open floor cell. Small rooms and corridors use the
/// cell with the longest open run along an axis, looking down it; open halls (over 400 cells) use
/// the cell nearest the centre, looking at the room's props (`props` = local statel positions).
fn room_spot(g: &Gnda, room: &Room, props: &[[f32; 3]]) -> Option<Spot> {
    const EYE_H: f32 = 1.7;
    let [x1, z1, x2, z2] = room.rect.map(|v| v as i32);
    if x2 as usize > g.w || z2 as usize > g.h {
        return None;
    }
    let fmin = floor_min(g, room)?;
    let (ox, oz) = room_origin(g, room);
    let open = |x: i32, z: i32| (x1..x2).contains(&x) && (z1..z2).contains(&z) && g.tile(x, z) != 0;
    let (mx, mz) = ((x1 + x2) as f32 * 0.5, (z1 + z2) as f32 * 0.5);
    let hall = (z1..z2).flat_map(|z| (x1..x2).map(move |x| (x, z))).filter(|&(x, z)| open(x, z)).count() > 400;
    let mut best: Option<(i32, f32, i32, i32, i32, i32)> = None; // run, -distance to centre, cell, dir
    for z in z1..z2 {
        for x in x1..x2 {
            if !open(x, z) {
                continue;
            }
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let run = (1..).take_while(|&k| open(x + dx * k, z + dz * k)).count() as i32;
                let near = -((x as f32 + 0.5 - mx).powi(2) + (z as f32 + 0.5 - mz).powi(2));
                let score = if hall { (0, near) } else { (run, near) };
                if best.is_none_or(|b| score > (b.0, b.1)) {
                    best = Some((score.0, near, x, z, dx, dz));
                }
            }
        }
    }
    let (_, _, x, z, dx, dz) = best?;
    let hs = g.height_scale;
    let floor_at = |cx: i32, cz: i32| -> f32 {
        let h = |px: i32, pz: i32| g.floor[g.at(px, pz)] as f32 * hs;
        (h(cx - 1, cz - 1) + h(cx, cz - 1) + h(cx - 1, cz) + h(cx, cz)) * 0.25 - fmin
    };
    let local = |cx: f32, cz: f32, up: f32| [(cx - x1 as f32) * g.cell + ox, up, (cz - z1 as f32) * g.cell + oz];
    let frame = Frame { rot: statel::ry(room.rot as f32 * std::f32::consts::FRAC_PI_2), pos: room.pos };
    let y = floor_at(x, z);
    let eye = local(x as f32, z as f32, y + EYE_H);
    let mut at = local((x + dx * 8) as f32, (z + dz * 8) as f32, y + EYE_H * 0.8);
    if hall && !props.is_empty() {
        // statel positions are room-local too (origin = room centre): aim at their centroid
        let n = props.len() as f32;
        let c = [props.iter().map(|p| p[0]).sum::<f32>() / n, props.iter().map(|p| p[2]).sum::<f32>() / n];
        if (c[0] - eye[0]).hypot(c[1] - eye[2]) > 5.0 {
            at = [c[0], y + EYE_H * 0.8, c[1]];
        }
    }
    Some(Spot { eye: frame.point(eye), at: frame.point(at) })
}

/// Camera for dungeon `rec`: entrance-like rooms first (`Entrance`, `Lobby`, `start…`), then in file
/// order; `props[i]` = local statel positions of room `i`.
pub(super) fn entry_spot(g: &Gnda, rec: &Record, props: &[Vec<[f32; 3]>]) -> Option<Spot> {
    let named = |r: &Room| r.name.as_ref().is_some_and(|n| ["entr", "lobby", "start", "enter"].iter().any(|k| n.to_lowercase().contains(k)));
    let order = (0..rec.rooms.len()).filter(|&i| named(&rec.rooms[i])).chain((0..rec.rooms.len()).filter(|&i| !named(&rec.rooms[i])));
    order.into_iter().find_map(|i| room_spot(g, &rec.rooms[i], props.get(i).map_or(&[], |p| p)))
}

/// Vertex welding of one cell block (`FUN_10001645`, N3 @0x10001645), against the vertices already
/// built with the same material. A vertex matches an earlier one when the squared distance is
/// <= 0.001 (`DAT_1003c8a0`), the squared normal difference <= 0.2 (`DAT_1003c898`) and the uv
/// difference is an integer within +-0.1 (`DAT_1003c890`). The client hashes positions at 30 cells
/// per metre (`DAT_1003c8a8`) and compares the 3x3x3 neighbourhood. The *whole block* is first shifted by
/// one integer uv offset (the first offset found for two different vertices, else the last one) so its
/// texture coordinates continue those of its neighbours; vertices that then coincide are dropped.
/// Returns the output index of every vertex of `block` (kept ones are appended to `out`).
fn weld_block(out: &mut RoomBuf, block: &mut [Vertex]) -> Vec<u32> {
    // The comparisons are x87 code with float operands: differences of two floats are exact, the constants are
    // doubles that hold the floats 0.1 / 0.2 (bytes at 0x1003c890 / 0x1003c898) and every result stored to a float
    // local is rounded to f32. Ties (uv 0.95 / 0.05 / 1.05 ...) decide merges, so the order of operations matters:
    // `(c - v) + 0.1` is rounded once, `off - (c - v)` once.
    const K1: f64 = 0.1f32 as f64;
    const K2: f64 = 0.2f32 as f64;
    let len2 = |a: [f32; 3], b: [f32; 3]| {
        let d = |i: usize| (b[i] - a[i]) as f64; // `FUN_10001340` stores float differences
        d(2) * d(2) + d(0) * d(0) + d(1) * d(1) // `FUN_100013a9` sums in this order
    };
    // candidates (index order): the three cells above the current one, then the previous cell up to this block.
    // `FUN_10001645` compares positions, normals and uvs only: the material plays no part. Its position hash
    // prefilter (bytes 0x7f & round(30 p), accepts a difference of -2..1) never rejects a pair within 0.0316 m.
    let (cur, w, start) = (out.cell_start.len() as isize - 1, out.width as isize, out.vertices.len());
    let cs = |k: isize| if k < 0 { 0 } else { out.cell_start[(k as usize).min(cur as usize)].min(start) };
    let cands: Vec<usize> = (cs(cur - w - 1)..cs(cur - w + 2)).chain(cs(cur - 1)..start).collect();
    let find = |out: &RoomBuf, v: &Vertex, f: &mut dyn FnMut(u32, &Vertex)| {
        for &j in &cands {
            let o = &out.vertices[j];
            if len2(o.pos, v.pos) <= 0.001f32 as f64 && len2(o.normal, v.normal) <= K2 {
                f(j as u32, o);
            }
        }
    };
    // integer part (as i16) of `(cand - v) + 0.1` when its fraction is <= 0.2
    let step = |c: f32, v: f32| {
        let t = (c as f64 - v as f64 + K1) as f32;
        let fl = (t as f64).floor();
        (t as f64 - fl <= K2).then_some(fl as i32 as i16)
    };
    // pass 1: the piece-wide uv offset
    let mut seen: Vec<([i16; 2], usize)> = Vec::new();
    let mut offset = None;
    'scan: for (i, v) in block.iter().enumerate() {
        let mut hits: Vec<[i16; 2]> = Vec::new();
        find(out, v, &mut |_, o| {
            if let (Some(u), Some(w)) = (step(o.uv[0], v.uv[0]), step(o.uv[1], v.uv[1])) {
                hits.push([u, w]);
            }
        });
        for off in hits {
            match seen.iter().position(|s| s.0 == off) {
                Some(k) if seen[k].1 != i => {
                    offset = Some(off);
                    break 'scan;
                }
                Some(_) => {}
                // the client's table holds 20 entries; a new one overwrites the last
                None if seen.len() == 20 => seen[19] = (off, i),
                None => seen.push((off, i)),
            }
        }
        offset = seen.last().map(|s| s.0);
    }
    let off = offset.unwrap_or([0, 0]).map(|v| v as f32);
    // pass 2: a vertex that coincides with an earlier one (uv compared against the unshifted uv: |off - (c - v)| <= 0.1)
    // is dropped, a kept one gets the offset added
    let near = |off: f32, c: f32, v: f32| ((off as f64 - (c as f64 - v as f64)) as f32).abs() as f64 <= K1;
    let mut remap = Vec::with_capacity(block.len());
    for v in block.iter_mut() {
        let mut hit = None;
        find(out, v, &mut |j, o| {
            if hit.is_none() && near(off[0], o.uv[0], v.uv[0]) && near(off[1], o.uv[1], v.uv[1]) {
                hit = Some(j);
            }
        });
        remap.push(hit.unwrap_or_else(|| {
            v.uv = [v.uv[0] + off[0], v.uv[1] + off[1]];
            out.vertices.push(*v);
            out.vertices.len() as u32 - 1
        }));
    }
    remap
}

/// `n3Room_t::DepackLightmap` (N3 @0x10010eb7) + `FUN_100082a7` (N3 @0x100082a7): the room record stores
/// `count` (= vertex count of the room shell) and a zlib stream (`FUN_1002a180`, zlib 1.2.5) that inflates to
/// `count` little-endian u16 (`count * 6` bytes in room format version < 8 is not present in the data). Every
/// u16 is the vertex diffuse colour of the shell vertex with the same index: `R = (v & 0x1f) << 3`,
/// `G = (v >> 5 & 0x3f) << 2`, `B = (v >> 8) & 0xf8`, alpha 255. Returns sRGB bytes; `None` when the stream is
/// shorter than `count * 2` (the client then paints all vertices white, `FUN_100082a7` first branch).
fn depack_lightmap(count: u32, z: &[u8]) -> Option<Vec<[u8; 3]>> {
    use std::io::Read;
    let mut raw = Vec::with_capacity(count as usize * 2);
    flate2::read::ZlibDecoder::new(z).read_to_end(&mut raw).ok()?;
    if raw.len() < count as usize * 2 {
        return None;
    }
    Some(raw.as_chunks::<2>().0.iter().take(count as usize).map(|c| {
        let v = u16::from_le_bytes([c[0], c[1]]);
        [((v & 0x1f) << 3) as u8, ((v >> 5 & 0x3f) << 2) as u8, (v >> 8 & 0xf8) as u8]
    }).collect())
}

/// Builds the shell of `room` in scene space. `piece(id)` returns cell meshes, `textures` maps
/// material indices to textures.
fn build_room(g: &Gnda, room: &Room, textures: &mut dyn FnMut(u8) -> Option<TextureKey>, piece: &mut dyn FnMut(u16) -> Option<Rc<Piece>>) -> Option<Mesh> {
    let [x1, z1, x2, z2] = room.rect.map(|v| v as i32);
    if x2 as usize > g.w || z2 as usize > g.h {
        return None;
    }
    let floor_min = floor_min(g, room)?;
    let (ox, oz) = room_origin(g, room);
    let mut b = Builder {
        g,
        frame: Frame { rot: statel::ry(room.rot as f32 * std::f32::consts::FRAC_PI_2), pos: room.pos },
        textures,
        out: RoomBuf { vertices: Vec::new(), by_tex: HashMap::new(), cell_start: Vec::new(), width: (x2 - x1) as usize },
    };
    for z in z1..z2 {
        for x in x1..x2 {
            b.out.cell_start.push(b.out.vertices.len());
            let Some((id, rot)) = g.cell_mesh(x, z) else { continue };
            let origin = [(x - x1) as f32 * g.cell + ox, (z - z1) as f32 * g.cell + oz];
            // piece order of `FUN_10001c3b` (N3 @0x10001c3b): corner posts, floor piece, main piece
            let corners = g.corners[z as usize * g.w + x as usize];
            for c in (0..4).filter(|c| corners & (1 << c) != 0) {
                if let Some(p) = piece(g.corner_mesh(g.tile(x, z) as usize)) {
                    b.add(&p, c, x, z, origin, -floor_min as f64, false);
                }
            }
            let main = (id != 0).then(|| piece(id)).flatten();
            // walls carry no floor: the client adds the flat floor piece of the tile
            if main.as_ref().is_none_or(|p| p.blocks.first().is_none_or(|bl| bl.1.is_empty())) {
                if let Some(f) = piece(g.floor_mesh(g.tile(x, z) as usize)) {
                    b.add(&f, 0, x, z, origin, -floor_min as f64, true);
                }
            }
            if let Some(p) = &main {
                b.add(p, rot, x, z, origin, -floor_min as f64, false);
            }
        }
    }
    let RoomBuf { mut vertices, by_tex, .. } = b.out;
    if vertices.is_empty() {
        return None;
    }
    // baked lighting only when the stream matches the shell vertex for vertex (1991 of 2000 rooms; the rest keep ambient light)
    let prelit = match room.lightmap.as_ref().and_then(|l| depack_lightmap(l.0, &l.1)).filter(|c| c.len() == vertices.len()) {
        Some(c) => {
            for (v, c) in vertices.iter_mut().zip(c) {
                let l = c.map(|b| super::environment::srgb_to_linear(b as f32 / 255.0));
                v.color = [l[0], l[1], l[2], 1.0];
            }
            true
        }
        None => false,
    };
    let mut keys: Vec<_> = by_tex.keys().copied().collect();
    keys.sort_by_key(|k| k.map(|k| (k.rdb_type, k.id)));
    let submeshes = keys.into_iter().map(|k| Submesh { two_sided: true, prelit, ..Submesh::new(by_tex[&k].clone(), k) }).collect();
    Some(Mesh { vertices, submeshes })
}

/// `VisualRoom_t::AddMaterial` (DisplaySystem @0x1006e916) pairs every material with the fallback texture
/// `darkrock_tileable.png` (rdb 1010004). Dungeon material 1 (slot 0 of most tilemaps: unpainted cells) is the
/// "Error" placeholder image in the PRK database, i.e. the texture that failed to resolve: draw the fallback.
fn material_texture(store: &RecordStore, textures: &mut HashMap<TextureKey, Texture>, mut m: u32) -> Option<TextureKey> {
    if m == 1 {
        m = crate::character::NameTable::load(store).ok()?.id(OBJECT_TEXTURES, "darkrock_tileable.png")? | 0x8000_0000;
    }
    let (ty, id) = if m & 0x8000_0000 != 0 { (OBJECT_TEXTURES, m & 0x7fff_ffff) } else { (DUNGEON_TEXTURES, m) };
    let key = TextureKey { rdb_type: ty, id };
    if let std::collections::hash_map::Entry::Vacant(e) = textures.entry(key) {
        let tex = store.get(ty, id).ok()??;
        e.insert(crate::texture::decode_texture(&tex).ok()?);
    }
    Some(key)
}

/// Adds the shells of all rooms of dungeon playfield `rec` to `scene`.
pub(super) fn build(store: &RecordStore, rec: &Record, scene: &mut Scene) -> Result<Gnda> {
    let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {}: no tilemap {}", rec.id, rec.tilemap))?;
    let g = parse_gnda(&d).with_context(|| format!("dungeon tilemap {}", rec.tilemap))?;
    let mut cached: Vec<Option<Option<TextureKey>>> = vec![None; g.mats.len()];
    let mut texture = |i: u8| -> Option<TextureKey> {
        let slot = cached.get_mut(i as usize)?;
        *slot.get_or_insert_with(|| material_texture(store, &mut scene.textures, g.mats[i as usize]))
    };
    let mut pieces: HashMap<u16, Option<Rc<Piece>>> = HashMap::new();
    let mut piece = |id: u16| -> Option<Rc<Piece>> {
        pieces
            .entry(id)
            .or_insert_with(|| store.get(PIECES, id as u32).ok().flatten().and_then(|b| parse_piece(&b).ok()).map(Rc::new))
            .clone()
    };
    let meshes: Vec<Mesh> = rec.rooms.iter().filter_map(|room| build_room(&g, room, &mut texture, &mut piece)).collect();
    for mesh in meshes {
        scene.meshes.push(mesh);
        scene.instances.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const W: usize = 6;

    fn png_bytes(img: image::DynamicImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn gray(px: &[u8]) -> Vec<u8> {
        png_bytes(image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(W as u32, W as u32, px.to_vec()).unwrap()))
    }

    fn section(out: &mut Vec<u8>, tag: &[u8; 4], version: u32, body: &[u8]) {
        let size = 12 + body.len();
        out.extend(tag);
        out.extend((size as u32).to_le_bytes());
        out.extend(version.to_le_bytes());
        out.extend(body);
        out.resize(out.len() + (4 - size % 4) % 4, 0); // padding is derived from the section size
    }

    /// 6x6 map, tile index 1 (id 7) on a 3x3 block at (1,1) minus its (3,3) corner; stream word
    /// `k` of the only neighbour row (tile 1 next to tile 0) is `101 + k`.
    fn fixture(doors: &[u32]) -> Vec<u8> {
        let mut d = b"GNDA".to_vec();
        d.extend([0; 4]);
        d.extend(1u32.to_le_bytes());
        d.extend([W as u8, 0, W as u8, 0]);
        d.extend(2.0f32.to_le_bytes());
        d.extend(0.2f32.to_le_bytes());
        d.extend(2u16.to_le_bytes());
        for id in [0u16, 7] {
            d.extend(id.to_le_bytes());
        }
        d.extend([0, 0]); // tile 0: no neighbour rows, no variants
        d.extend([1, 0]);
        d.extend(0u16.to_le_bytes());
        for k in 0..16u16 {
            d.extend((101 + k).to_le_bytes());
        }
        let mut ty = vec![0u8; W * W];
        for (x, z) in [(1, 1), (2, 1), (3, 1), (1, 2), (2, 2), (3, 2), (1, 3), (2, 3)] {
            ty[z * W + x] = 1;
        }
        section(&mut d, b"DCGA", 1, &gray(&ty));
        section(&mut d, b"DHGA", 1, &gray(&[0; W * W]));
        section(&mut d, b"HCDA", 1, &gray(&[0; W * W]));
        section(&mut d, b"IRHA", 1, &doors.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
        let mut taha = vec![0, 0];
        taha.extend(gray(&[0; W * W]));
        section(&mut d, b"TAHA", 1, &taha);
        let mut xtha = 2u16.to_le_bytes().to_vec();
        for m in [1u32, 0x8000_0005] {
            xtha.extend(m.to_le_bytes());
        }
        xtha.extend(3u16.to_le_bytes());
        xtha.extend(png_bytes(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(W as u32, W as u32, |_, _| image::Rgb([1, 0, 1])))));
        section(&mut d, b"XTHA", 2, &xtha);
        d.extend(b"tail!"); // trailing bytes after the last section are ignored
        d
    }

    fn floor_piece() -> Piece {
        let quad = |y: f32| -> (Vec<[f32; 8]>, Vec<u16>) {
            let v = |x: f32, z: f32, u: f32, w: f32| [x, y, z, 0.0, 1.0, 0.0, u, w];
            (vec![v(-1.0, 1.0, 1.0, 1.0), v(1.0, 1.0, 0.0, 1.0), v(-1.0, -1.0, 1.0, 0.0), v(1.0, -1.0, 0.0, 0.0)], vec![0, 1, 2, 3, 2, 1])
        };
        Piece { blocks: vec![quad(0.0), quad(4.0)] }
    }

    #[test]
    fn canonical_masks() {
        assert_eq!(canon(0), (0, 0));
        assert_eq!(canon(12), (3, 2));
        assert_eq!(canon(10), (5, 1));
        assert_eq!(canon(8), (1, 3));
        assert_eq!(canon(15), (15, 0));
    }

    #[test]
    fn gnda_layout() {
        let g = parse_gnda(&fixture(&[])).unwrap();
        assert_eq!((g.w, g.h, g.cell, g.ids.clone()), (W, W, 2.0, vec![0, 7]));
        // stream words 0..4 -> slots 0..4, 5.. -> 6.. ; slot 5 of the self row is the tile id
        let row = |idx: usize| g.table[32 * 16 + idx];
        assert_eq!((row(0), row(4), row(5), row(6), row(15)), (101, 105, 0, 106, 115));
        assert_eq!((g.floor_mesh(1), g.corner_mesh(1)), (7, 113));
        assert_eq!((g.mats.clone(), g.layers, g.layer[0..3].to_vec()), (vec![1, 0x8000_0005], 3, vec![1, 0, 1]));
        assert_eq!(g.ty[W + 1], 1);
    }

    #[test]
    fn autotile_state_and_mesh() {
        let g = parse_gnda(&fixture(&[])).unwrap();
        let state = |x: usize, z: usize| g.state[z * W + x];
        assert_eq!((state(2, 2), state(1, 1), state(2, 1), state(3, 2)), (0, 12, 4, 3));
        assert_eq!(g.cell_mesh(2, 2), Some((7, 0))); // interior: flat floor/ceiling piece
        assert_eq!(g.cell_mesh(2, 1), Some((105, 2))); // single wall, canonical key 1 -> stream word 4
        assert_eq!(g.cell_mesh(1, 1), Some((104, 2))); // two adjacent walls -> key 3
        assert_eq!(g.cell_mesh(0, 0), None);
        // the concave room corner (cell (3,3) missing) gets a post from (2,2) only: corner 2 (+x,+z)
        assert_eq!(g.corners[2 * W + 2], 0b0100);
        assert_eq!(g.corners[2 * W + 1], 0, "interior vertices cancel");
    }

    #[test]
    fn door_edge_adds_frame_flag() {
        // edge between cells (0,2) and (1,2): the wall at x-1 of cell (1,2) becomes a door frame (key 0x11)
        let g = parse_gnda(&fixture(&[(2 * W as u32) << 1])).unwrap();
        assert_eq!(g.state[2 * W + 1], 0x18);
        assert_eq!(g.cell_mesh(1, 2), Some((111, 3)));
    }

    #[test]
    fn piece_format() {
        let mut d = 2u16.to_le_bytes().to_vec();
        d.extend([0, 0, 0, 0]); // empty block 0
        d.extend(3u16.to_le_bytes());
        for v in [0.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 0.25, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 0.25, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.5, 0.25] {
            d.extend(v.to_le_bytes());
        }
        d.extend(3u16.to_le_bytes());
        for i in [0u16, 1, 2] {
            d.extend(i.to_le_bytes());
        }
        d.extend([0; 12]);
        let p = parse_piece(&d).unwrap();
        assert_eq!(p.blocks.len(), 2);
        assert!(p.blocks[0].1.is_empty());
        assert_eq!((p.blocks[1].0[1], p.blocks[1].1.clone()), ([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 0.25], vec![0, 1, 2]));
        assert!(parse_piece(&[1, 0, 1, 0]).is_err());
    }

    #[test]
    fn room_is_placed_like_statels_and_mirrored() {
        let g = parse_gnda(&fixture(&[])).unwrap();
        let room = Room { rot: 1, rect: [1, 1, 4, 4], pos: [100.0, 5.0, 200.0], name: None, lightmap: None };
        let floor = Rc::new(floor_piece());
        let mut piece = |id: u16| (id == 7).then(|| floor.clone());
        let mut texture = |i: u8| Some(TextureKey { rdb_type: 1, id: i as u32 });
        let mesh = build_room(&g, &room, &mut texture, &mut piece).unwrap();
        // 8 floors (walls get the flat floor added) + one ceiling (interior cell only)
        // welded: the floor corners of the 3x3 window (4x4 grid minus the corner without a floor) are shared
        assert_eq!(mesh.vertices.len(), 15 + 4);
        let low: Vec<_> = mesh.vertices.iter().filter(|v| (v.pos[1] - 5.0).abs() < 1e-4).collect();
        let range = |f: &dyn Fn(&Vertex) -> f32| low.iter().map(|v| f(v)).fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
        // local x,z in [-4, 2]; rotated by one quarter turn about +Y (x' = z, z' = -x), scene z = -z'
        assert_eq!((range(&|v| v.pos[0]), range(&|v| v.pos[2])), ((96.0, 102.0), (-204.0, -198.0)));
        assert!(mesh.vertices.iter().any(|v| (v.pos[1] - 9.0).abs() < 1e-4), "ceiling at y = 4");
        // right-handed CCW normal of every floor triangle points up
        for s in &mesh.submeshes {
            for t in s.indices.as_chunks::<3>().0 {
                let p = |i: u32| mesh.vertices[i as usize].pos;
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                if (a[1] - 5.0).abs() < 1e-4 && (b[1] - 5.0).abs() < 1e-4 && (c[1] - 5.0).abs() < 1e-4 {
                    let (u, v) = ([b[0] - a[0], b[2] - a[2]], [c[0] - a[0], c[2] - a[2]]);
                    assert!(u[1] * v[0] - u[0] * v[1] > 0.0, "floor triangle faces down: {a:?} {b:?} {c:?}");
                }
            }
        }
        // material of block 0 is layer 0 -> index 1 -> texture id 1
        assert!(mesh.submeshes.iter().any(|s| s.texture == Some(TextureKey { rdb_type: 1, id: 1 })));
    }

    #[test]
    fn real_dungeon_builds_when_game_data_exists() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let Ok(store) = RecordStore::open(&std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client")) else { return };
        let Some(rec) = store.get(super::super::RECORD, 127).ok().flatten() else { return };
        let rec = super::super::record::parse(&rec).unwrap();
        let mut scene = Scene::default();
        build(&store, &rec, &mut scene).unwrap();
        assert_eq!(scene.instances.len(), rec.rooms.len());
        assert!(scene.meshes.iter().all(|m| !m.vertices.is_empty()));
        assert!(!scene.textures.is_empty());
        // welding: 184 561 shell vertices unwelded, 111 643 welded
        assert!(scene.meshes.iter().map(|m| m.vertices.len()).sum::<usize>() < 150_000);
    }

    #[test]
    fn lightmap_depack_matches_fun_100082a7() {
        use std::io::Write;
        // R = low 5 bits, G = middle 6, B = high 5 (client: `(v&0x1f)<<3`, `(v>>5&0x3f)<<2`, `v>>8&0xf8`)
        let words = [0x0000u16, 0xffff, 0x001f, 0x07e0, 0xf800, 0x1922];
        let raw: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&raw).unwrap();
        let z = z.finish().unwrap();
        let c = depack_lightmap(6, &z).unwrap();
        assert_eq!(c[..5], [[0, 0, 0], [248, 252, 248], [248, 0, 0], [0, 252, 0], [0, 0, 248]]);
        assert_eq!(c[5], [16, 36, 24]);
        // a stream shorter than count*2 bytes is rejected (the client paints white)
        assert!(depack_lightmap(7, &z).is_none());
    }

    #[test]
    fn weld_shifts_the_block_uv_and_merges_shared_corners() {
        let v = |x: f32, n: [f32; 3], u: f32| Vertex { pos: [x, 0.0, 0.0], normal: n, uv: [u, 0.5], ..Default::default() };
        let up = [0.0, 1.0, 0.0];
        let mut out = RoomBuf { vertices: Vec::new(), by_tex: HashMap::new(), cell_start: vec![0], width: 4 };
        // first block: x = 0 (u 0), x = 1 (u 1)
        let first = weld_block(&mut out, &mut [v(0.0, up, 0.0), v(1.0, up, 1.0)]);
        assert_eq!(first, vec![0, 1]);
        // neighbour block starts at x = 1 with u = 0 (tiled texture): shifted by +1 so its corner
        // continues u = 1 and merges; the far vertex becomes u = 2. A vertex with another normal is kept.
        let mut b = [v(1.0, up, 0.0), v(2.0, up, 1.0), v(1.0, [1.0, 0.0, 0.0], 0.0)];
        assert_eq!(weld_block(&mut out, &mut b), vec![1, 2, 3]);
        assert_eq!(out.vertices.len(), 4);
        assert_eq!(out.vertices[2].uv[0], 2.0);
        // a cell that is not adjacent (its vertices are outside the candidate ranges) never merges
        out.cell_start.extend([4, 4, 4, 4, 4, 4]);
        assert_eq!(weld_block(&mut out, &mut [v(1.0, up, 1.0)]), vec![4]);
    }

    #[test]
    fn ceiling_stretch_is_a_signed_byte() {
        assert_eq!([stretch(0), stretch(37), stretch(223), stretch(224), stretch(242), stretch(255)], [0.0, 37.0, 223.0, -32.0, -14.0, -1.0]);
    }

    #[test]
    fn weld_decides_uv_ties_like_the_x87_code() {
        let v = |u: f32| Vertex { pos: [1.0, 2.0, 3.0], normal: [0.0, 1.0, 0.0], uv: [u, 0.0], ..Default::default() };
        let mut out = RoomBuf { vertices: Vec::new(), by_tex: HashMap::new(), cell_start: vec![0], width: 4 };
        weld_block(&mut out, &mut [v(1.05)]);
        // 0.95 -> 1.05 is an offset of 0.1 + 0.1 - 2e-8 in float arithmetic: (1.05 - 0.95) + 0.1 = 0.19999996 <= 0.2, so
        // the uv offset is 0 and |0 - 0.0999999642| <= 0.1 merges the vertices
        assert_eq!(weld_block(&mut out, &mut [v(0.95)]), vec![0]);
        // 0.05 -> 0.95: (0.050000012 - 0.95) + 0.1 = -0.79999995, fraction 0.2000000477 > 0.2, no offset, |0 - 0.9| > 0.1: kept
        let mut out = RoomBuf { vertices: Vec::new(), by_tex: HashMap::new(), cell_start: vec![0], width: 4 };
        weld_block(&mut out, &mut [v(0.050000012)]);
        assert_eq!(weld_block(&mut out, &mut [v(0.95)]), vec![1]);
    }

    #[test]
    fn height_layers_clamp_at_the_map_border() {
        // N3 @0x10001c3b replicates the nearest valid row/column for the 4x4 corner window
        let g = parse_gnda(&fixture(&[])).unwrap();
        assert_eq!(g.at(-5, -3), 0);
        assert_eq!(g.at(W as i32 + 4, W as i32 + 4), W * W - 1);
        assert_eq!(g.at(2, -1), 2);
    }

    /// Over every dungeon room of the client data: the shell vertex count must equal the room's baked lightmap
    /// count (`Room::lightmap.0`, the vertex count `CreateDungeonRoom` produced when the lightmap was baked).
    /// `cargo test --release -p ao-formats dungeon_lightmap_survey -- --ignored --nocapture` prints the tally.
    #[test]
    #[ignore]
    fn dungeon_lightmap_survey() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let Ok(store) = RecordStore::open(&std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client")) else { return };
        let (mut rooms, mut ok) = (0, 0);
        let mut bad = Vec::new();
        for (id, _) in super::super::list_playfields(&store).unwrap() {
            let Some(rec) = store.get(super::super::RECORD, id).ok().flatten() else { continue };
            let rec = super::super::record::parse(&rec).unwrap();
            let Some(d) = store.get(TILEMAP, rec.tilemap).ok().flatten().filter(|_| rec.tilemap != rec.id) else { continue };
            let g = parse_gnda(&d).unwrap();
            let mut pieces: HashMap<u16, Option<Rc<Piece>>> = HashMap::new();
            let mut piece = |id: u16| -> Option<Rc<Piece>> {
                pieces.entry(id).or_insert_with(|| store.get(PIECES, id as u32).ok().flatten().and_then(|b| parse_piece(&b).ok()).map(Rc::new)).clone()
            };
            for (ri, room) in rec.rooms.iter().enumerate() {
                let Some(lm) = &room.lightmap else { continue };
                let n = build_room(&g, room, &mut |_| None, &mut piece).map_or(0, |m| m.vertices.len());
                rooms += 1;
                if n == lm.0 as usize {
                    ok += 1;
                } else {
                    bad.push((id, ri, n as i64 - lm.0 as i64));
                }
            }
        }
        println!("rooms with lightmap {rooms}, shell vertex count == lightmap count: {ok}; mismatching (playfield, room, delta): {bad:?}");
        assert!(ok * 100 >= rooms * 99, "lightmap match rate regressed: {ok}/{rooms}");
    }
}
