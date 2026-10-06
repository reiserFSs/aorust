//! Collision surface records (rdb 1000013, `n3SurfaceResource_t`, N3.dll): the triangle volumes of a statel zone / dungeon room.
//!
//! `n3Zone_t::LoadSurface` (N3 @0x1001a947) acquires rdb `0xf424d` = 1000013 with identity `(playfield << 16) | zone`
//! (`RDBPlayfield_t::ReadBlob` @0x1001c115 sets `zone.id = playfield << 16 | index`, `n3Room_t` reader @0x10012803 the
//! same for rooms). `n3SurfaceResource_t::ReadBlob` (@0x10016179) dispatches on the record version column:
//!
//! * **4** (`ReadVersion4` @0x10015e17): `u32` inflated size + zlib stream; inside `u32 a; [KD tree if a]; u32 b; ...` with the
//!   plain KD file reader `FUN_1002e93e` (`"KDTreeFile"` header, float / u16 quantised vertices, volumes, nodes).
//! * **5** (`ReadVersion5` @0x10015fc1): one range-coded stream (`FUN_1002f1b0` main tree, optional portal, marker `0x79`).
//!
//! A tree is a set of *volumes* (indexed triangle meshes, max 999 triangles each, `FUN_1002ffcd`) plus a binary space
//! partition (split plane per node, leaves list volumes) that only accelerates queries; the volumes are what we keep.

use anyhow::{bail, ensure, Context, Result};

/// rdb type of the collision surface records (`n3Zone_t::LoadSurface` N3 @0x1001a947: `Identity_t{0xf424d, zone id}`).
pub const SURFACE_TYPE: u32 = 1_000_013;

/// One triangle mesh volume (`0x38` byte face record of the client).
#[derive(Debug, Clone, Default)]
pub struct Volume {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub verts: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
}

/// Decoded record: volumes plus the optional teleport portal polygon (`PortalArea_t`).
#[derive(Debug, Clone, Default)]
pub struct Surface {
    pub volumes: Vec<Volume>,
    /// Number of BSP nodes (validated against the stream, the tree itself is not kept).
    pub nodes: usize,
    /// `n3SurfaceResource_t::GetTeleportalArea` polygon (metres), if the record has one.
    pub portal: Vec<[f32; 3]>,
    /// Teleport destination bits at `+0x40` (playfield id in the low 16 bits, see `GetTeleportDestinationPlayfield`).
    pub portal_dest: Option<u32>,
}

/// Depth cap of the BSP walk (real trees are far shallower; protects the stack against garbage).
const MAX_DEPTH: usize = 128;

/// Triangles per volume (the client rejects volumes outside `[1, 999]`, `FUN_1002e93e`).
const MAX_VOLUME_TRIS: usize = 999;

/// Quantisation step of the range-coded tree (`FUN_1002f1b0`: `*(float*)(kd+4) = _DAT_1003d618`).
const STEP: f32 = 0.01;

/// Range decoder (`FUN_1002f610` init, `FUN_1002f952` frequency decode, `FUN_1002f7b6` binary decode).
struct Rc<'a> {
    d: &'a [u8],
    p: usize,
    range: u32,
    code: u32,
    /// Set when the stream violated the decoder invariants (only corrupt data does).
    bad: bool,
}

impl<'a> Rc<'a> {
    fn new(d: &'a [u8]) -> Self {
        Rc { d, p: 0, range: 1, code: 0, bad: false }
    }
    fn norm(&mut self) {
        if self.range == 0 {
            self.bad = true;
            self.range = 1 << 24;
        }
        while self.range < 1 << 24 {
            self.range <<= 8;
            self.code <<= 8;
            if self.p < self.d.len() {
                self.code |= self.d[self.p] as u32;
                self.p += 1;
            }
        }
        // valid streams keep `code < range`; garbage breaks it (and could drive `range` to 0)
        if self.code >= self.range {
            self.bad = true;
            self.code = 0;
        }
    }
    /// Uniform symbol in `0..total`.
    fn freq(&mut self, total: u32) -> Result<u32> {
        ensure!(total > 0, "range decoder: empty alphabet");
        self.norm();
        let (r, t) = (self.range as u64, total as u64);
        let mut q = (self.code as u64 * t / r) as u32;
        let mut up = ((q as u64 + 1) * r / t) as u32;
        if up <= self.code {
            up = ((q as u64 + 2) * r / t) as u32;
            q += 1;
        }
        let lo = (q as u64 * r / t) as u32;
        self.code = self.code.wrapping_sub(lo);
        self.range = up.wrapping_sub(lo);
        Ok(q)
    }
    /// Binary decision, `0` with probability `a / (a + b)`.
    fn bit(&mut self, a: u32, b: u32) -> u32 {
        self.norm();
        let thr = (a as u64 * self.range as u64 / (a + b) as u64) as u32;
        if self.code < thr {
            self.range = thr;
            0
        } else {
            self.range -= thr;
            self.code -= thr;
            1
        }
    }
}

type Q = [i32; 3];

/// Triangle mesh decoder state (`M`, 0x5c bytes at `ebp-0x5c` of `FUN_1003517f`).
struct MeshDec {
    idx: Vec<u16>,
    verts: Vec<Q>,
    org: Q,
    size: Q,
}

impl MeshDec {
    /// `FUN_100363b5`: position of the vertex used by index slot `slot`, each axis skewed to the box faces.
    fn read_pos(&mut self, rc: &mut Rc, slot: usize) -> Result<()> {
        let v = self.idx[slot] as usize;
        let mut p = [0i32; 3];
        for (a, pa) in p.iter_mut().enumerate() {
            let n = self.size[a];
            let off = if rc.bit(0x44, 0x11) == 0 {
                ensure!(n >= 3, "mesh cell too small");
                rc.freq(n as u32 - 2)? as i32 + 1
            } else if rc.bit(3, 3) == 0 {
                n - 1
            } else {
                0
            };
            *pa = self.org[a] + off;
        }
        if self.verts.len() <= v {
            self.verts.resize(v + 1, [0; 3]);
        }
        self.verts[v] = p;
        Ok(())
    }
    fn nverts(&self) -> usize {
        self.verts.len()
    }
}

/// Decoded volume: indices, vertices, cell box min and max.
type Decoded = (Vec<u16>, Vec<[f32; 3]>, [f32; 3], [f32; 3]);

/// Mesh decoder with the queue of open edges (`FUN_1003517f`). Returns indices, vertices and the cell box.
fn decode_mesh(rc: &mut Rc, min: Q, max: Q) -> Result<Decoded> {
    // FUN_10035acc: cell origin and size
    let mut org = [0i32; 3];
    for a in 0..3 {
        org[a] = rc.freq((max[a] - min[a] + 1) as u32)? as i32 + min[a];
    }
    let mut size = [0i32; 3];
    for a in 0..3 {
        size[a] = rc.freq((max[a] - org[a] + 1) as u32)? as i32 + 1;
    }
    let to_f = |q: Q, off: i32| [0, 1, 2].map(|a| ((q[a] + off) as f32 + 0.5) * STEP);
    let (bmin, bmax) = (to_f(org, 0), to_f([0, 1, 2].map(|a| org[a] + size[a] - 1), 0));
    let mut m = MeshDec { idx: Vec::new(), verts: Vec::new(), org, size };
    let sparse = rc.bit(3, 2);
    let mut nv = 0usize; // vertices allocated so far (M[10])
    let mut queue: Vec<(u32, u32)> = Vec::new();
    loop {
        // FUN_1003680f: a fresh seed triangle with three new vertices
        let t0 = m.idx.len();
        ensure!(t0 < 3 * MAX_VOLUME_TRIS && !rc.bad, "runaway mesh stream");
        for k in 0..3 {
            m.idx.push(nv as u16);
            nv += 1;
            ensure!(nv <= u16::MAX as usize, "volume with too many vertices");
            m.read_pos(rc, t0 + k)?;
        }
        for k in 0..3 {
            queue.push((m.idx[t0 + k] as u32, m.idx[t0 + (k + 1) % 3] as u32));
        }
        while !queue.is_empty() {
            let e = queue.remove(0);
                    if sparse != 0 && rc.bit(2, 6) == 0 {
                continue;
            }
            let prev = queue.iter().position(|n| n.1 == e.0);
            let next = queue.iter().position(|n| n.0 == e.1);
            if prev.is_some() || next.is_some() {
                let c1 = ctx(&m, prev.map(|i| queue[i]), Some(e));
                let c2 = ctx(&m, Some(e), next.map(|i| queue[i]));
                if rc.bit(200, c2 + c1) != 0 {
                    let closes = match (prev, next) {
                        (Some(p), Some(n)) => queue[p].0 == queue[n].1,
                        _ => false,
                    };
                    if !closes {
                        // the neighbour with the larger weight is likelier; bit 0 picks `next`
                        if rc.bit(c2, c1) == 0 {
                            let n = next.context("edge queue: missing next edge")?;
                            let ni = queue[n];
                            m.idx.extend([e.1 as u16, e.0 as u16, ni.1 as u16]);
                            queue[n] = (e.0, ni.1);
                        } else {
                            let p = prev.context("edge queue: missing previous edge")?;
                            let pi = queue[p];
                            m.idx.extend([pi.1 as u16, pi.0 as u16, e.1 as u16]);
                            queue[p] = (pi.0, e.1);
                        }
                    } else {
                        let (p, n) = (prev.unwrap(), next.unwrap());
                        let pi = queue[p];
                        m.idx.extend([pi.1 as u16, pi.0 as u16, e.1 as u16]);
                        // remove the larger index first
                        let (hi, lo) = if p > n { (p, n) } else { (n, p) };
                        if p == n { bail!("degenerate close"); }
                        queue.remove(hi);
                        queue.remove(lo);
                    }
                    continue;
                }
            }
            // FUN_10036895: new triangle (v, e.b, e.a) over the edge, v existing (1/16) or new
            let slot = m.idx.len();
            if rc.bit(1, 15) == 0 {
                ensure!(nv > 0, "no vertices to reuse");
                let v = rc.freq(nv as u32)?;
                m.idx.push(v as u16);
            } else {
                m.idx.push(nv as u16);
                nv += 1;
                ensure!(nv <= u16::MAX as usize, "volume with too many vertices");
                m.read_pos(rc, slot)?;
            }
            m.idx.extend([e.1 as u16, e.0 as u16]);
            let v = m.idx[slot] as u32;
            queue.push((e.0, v));
            queue.push((v, e.1));
            ensure!(m.idx.len() < 3 * MAX_VOLUME_TRIS && queue.len() < 1 << 16 && !rc.bad, "runaway mesh stream");
        }
        if rc.bit(0x18, 1) == 0 {
            break;
        }
    }
    if rc.bit(1, 999) != 1 {
        return Ok((Vec::new(), Vec::new(), bmin, bmax));
    }
    let verts = (0..m.nverts()).map(|i| to_f(m.verts[i], 0)).collect();
    Ok((m.idx, verts, bmin, bmax))
}

/// `FUN_10036319`: weight of the corner `a -> b` formed with the next edge by the sign of a dot product.
fn ctx(m: &MeshDec, a: Option<(u32, u32)>, b: Option<(u32, u32)>) -> u32 {
    let (Some(a), Some(b)) = (a, b) else { return 0 };
    let v = |i: u32| m.verts.get(i as usize).copied().unwrap_or([0; 3]);
    let (ab, aa, bb) = (v(a.1), v(a.0), v(b.1));
    // 32 bit wrapping arithmetic like the x86 code (cells up to hundreds of metres overflow it)
    let d: i32 = (0..3).fold(0i32, |s, k| s.wrapping_add(bb[k].wrapping_sub(ab[k]).wrapping_mul(aa[k].wrapping_sub(ab[k]))));
    match d {
        0 => 0x61,
        d if d > 0 => 0xa7,
        _ => 0x31,
    }
}

/// Cursor over the node section (`FUN_1002eefc`): the tree is only validated.
fn read_nodes(rc: &mut Rc, nvol: u32, min: Q, max: Q, count: &mut usize, budget: usize, depth: usize) -> Result<()> {
    ensure!(*count < budget && depth < MAX_DEPTH && !rc.bad, "more BSP nodes than announced");
    *count += 1;
    if rc.bit(1, 1) == 0 {
        let axis = rc.freq(3)? as usize;
        ensure!(max[axis] >= min[axis], "inverted BSP bounds");
        let split = rc.freq((max[axis] - min[axis] + 1) as u32)? as i32 + min[axis];
        let mut lmax = max;
        lmax[axis] = split;
        read_nodes(rc, nvol, min, lmax, count, budget, depth + 1)?;
        let mut rmin = min;
        rmin[axis] = split;
        read_nodes(rc, nvol, rmin, max, count, budget, depth + 1)
    } else {
        let n = rc.freq(nvol + 1)? as i32;
        let (mut lo, mut hi) = (0i32, nvol as i32 - 1);
        for k in (1..=n).rev() {
            ensure!(hi >= lo, "BSP leaf list exhausted");
            let v = rc.freq((hi - lo + 1) as u32)? as i32 + lo;
            if k & 1 == 0 {
                hi = v - 1;
            } else {
                lo = v + 1;
            }
        }
        Ok(())
    }
}

/// Range-coded version 5 record (`FUN_1002f1b0` + the tail of `ReadVersion5`).
pub fn parse_v5(d: &[u8]) -> Result<Surface> {
    let mut rc = Rc::new(d);
    rc.freq(0xf)?;
    let count = |rc: &mut Rc| -> Result<u32> { Ok(if rc.bit(1, 1) == 0 { rc.freq(100)? } else { rc.freq(1_000_000)? + 100 }) };
    let nvol = count(&mut rc)?;
    let mut s = Surface::default();
    if nvol != 0 {
        let nnodes = count(&mut rc)?;
        // the largest record of the client has 4165 volumes / 8051 nodes
        ensure!(nvol <= 1 << 16 && nnodes <= 1 << 17, "implausible surface size {nvol}/{nnodes}");
        let mut b = [0i32; 6];
        for v in &mut b {
            *v = rc.freq(0x100_0000)? as i32 - 0x80_0000;
        }
        let (min, max) = ([b[0], b[1], b[2]], [b[3], b[4], b[5]]);
            let mut budget = 2_000_000usize;
        for _ in 0..nvol {
            let (idx, verts, bmin, bmax) = decode_mesh(&mut rc, min, max)?;
            budget = budget.checked_sub(idx.len() / 3).context("implausible triangle count")?;
            let nv = idx.iter().map(|&i| i as usize + 1).max().unwrap_or(0);
            ensure!(nv <= verts.len() && idx.len() % 3 == 0 && idx.len() / 3 < 1000, "bad volume ({} idx, {} verts)", idx.len(), verts.len());
            s.volumes.push(Volume { min: bmin, max: bmax, verts: verts[..nv].to_vec(), tris: idx.as_chunks::<3>().0.iter().map(|t| t.map(u32::from)).collect() });
        }
        ensure!(rc.freq(0xff)? == 0x5a, "KD tree marker 0x5a missing");
        let mut n = 0;
        read_nodes(&mut rc, nvol, min, max, &mut n, nnodes as usize, 0)?;
        ensure!(n == nnodes as usize, "BSP node count {n} != {nnodes}");
        s.nodes = n;
    }
    if rc.bit(7, 1) != 0 {
        rc.freq(10)?;
        let n = rc.freq(1000)? as usize;
        for _ in 0..n {
            let mut p = [0f32; 3];
            for c in &mut p {
                let u = rc.freq(0x100_0000)? as i32 - 0x80_0000;
                *c = u as f32 * 0.01;
            }
            s.portal.push(p);
        }
        let hi = rc.freq(0x10000)?;
        let lo = rc.freq(0x10000)?;
        s.portal_dest = Some(hi << 16 | lo);
    }
    ensure!(rc.freq(0xfe)? == 0x79 && !rc.bad, "KD record end marker 0x79 missing");
    Ok(s)
}

struct Cur<'a> {
    d: &'a [u8],
    o: usize,
}

impl Cur<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        ensure!(self.o + n <= self.d.len(), "truncated KD record");
        self.o += n;
        Ok(&self.d[self.o - n..self.o])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
}

/// `FUN_1002e93e`: plain `KDTreeFile` (version 3 body). Returns the volumes.
fn kd_file(c: &mut Cur) -> Result<Vec<Volume>> {
    // `BinaryStream >> char*`: NUL terminated
    let start = c.o;
    let name_len = c.d[start..].iter().position(|&b| b == 0).context("KD name")?;
    ensure!(&c.d[start..start + name_len] == b"KDTreeFile", "not a KDTreeFile");
    c.o += name_len + 1;
    let (nvol, nnodes, version, nverts) = (c.u32()? as usize, c.u32()? as usize, c.u32()?, c.u32()? as usize);
    ensure!(version == 3, "KDTreeFile version {version}");
    ensure!(nvol < 1 << 20 && nverts < 1 << 24 && nnodes < 1 << 22, "implausible KD sizes");
    let first = c.f32()?;
    let mut v = vec![[0f32; 3]; nverts];
    // `_DAT_100413d4` (-99999.0) as the first float marks raw float vertices (axis-major arrays), otherwise it is the
    // x base of u16 vertices in steps of 0.01 (`_DAT_1003d628`), followed by the y and z bases
    if first == RAW_MARK {
        for a in 0..3 {
            for p in v.iter_mut() {
                p[a] = c.f32()?;
            }
        }
    } else {
        let base = [first, c.f32()?, c.f32()?];
        for a in 0..3 {
            for p in v.iter_mut() {
                p[a] = c.u16()? as f32 * 0.01 + base[a];
            }
        }
    }
    let mut vols = Vec::with_capacity(nvol);
    for _ in 0..nvol {
        let ntri = c.u32()? as usize;
        ensure!((1..=999).contains(&ntri), "KD volume with {ntri} triangles");
        let min = [c.f32()?, c.f32()?, c.f32()?];
        let max = [c.f32()?, c.f32()?, c.f32()?];
        let mut tris = Vec::with_capacity(ntri);
        let mut idx = Vec::with_capacity(ntri * 3);
        for _ in 0..ntri * 3 {
            let i = if nverts < 0x100 {
                c.take(1)?[0] as u32
            } else if nverts < 0x10000 {
                c.u16()? as u32
            } else {
                c.u32()?
            };
            ensure!((i as usize) < nverts, "KD index out of range");
            idx.push(i);
        }
        tris.extend(idx.as_chunks::<3>().0.iter().copied());
        vols.push(Volume { min, max, verts: Vec::new(), tris });
    }
    // volumes address the shared vertex array: localise
    for vol in &mut vols {
        let mut map = std::collections::HashMap::new();
        for t in &mut vol.tris {
            for i in t.iter_mut() {
                let n = map.len() as u32;
                let l = *map.entry(*i).or_insert_with(|| {
                    vol.verts.push(v[*i as usize]);
                    n
                });
                *i = l;
            }
        }
    }
    // nodes: `FUN_1002d524`: f32 split, i32 axis, u32 count (0 = inner node), leaf lists of volume indices
    let mut nodes = 0usize;
    let nvolu = nvol;
    fn walk(c: &mut Cur, nvol: usize, nodes: &mut usize, depth: usize) -> Result<()> {
        *nodes += 1;
        ensure!(*nodes < 1 << 22 && depth < MAX_DEPTH, "KD node runaway");
        let (_split, _axis, cnt) = (c.f32()?, c.u32()?, c.u32()? as usize);
        if cnt == 0 {
            walk(c, nvol, nodes, depth + 1)?;
            walk(c, nvol, nodes, depth + 1)
        } else {
            let w = if nvol < 0x100 { 1 } else if nvol < 0x10000 { 2 } else { 4 };
            c.take(cnt * w).map(|_| ())
        }
    }
    walk(c, nvolu, &mut nodes, 0)?;
    ensure!(nodes == nnodes, "KD node count {nodes} != {nnodes}");
    Ok(vols)
}

/// Marker of raw float vertices in the version 3 body: `_DAT_100413d4` (N3).
const RAW_MARK: f32 = -99999.0;

/// Version 4 record (`ReadVersion4`): inflated payload `u32 a; [tree if a]; u32 b; ...`.
pub fn parse_v4(d: &[u8]) -> Result<Surface> {
    ensure!(d.len() > 4, "short record");
    let size = u32::from_le_bytes(d[..4].try_into().unwrap()) as usize;
    ensure!(size <= 4_000_000, "KD record too large");
    let mut raw = Vec::with_capacity(size);
    std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&d[4..]), &mut raw).context("inflating KD record")?;
    ensure!(raw.len() == size, "inflated size {} != {size}", raw.len());
    let mut c = Cur { d: &raw, o: 0 };
    let a = c.u32()?;
    let mut s = Surface::default();
    if a != 0 {
        s.volumes = kd_file(&mut c)?;
        let b = c.u32()?;
        if b != 0 {
            // the second tree is the invisible blocker surface (+0x2c): its volumes block movement like the main ones
            s.volumes.extend(kd_file(&mut c)?);
        }
    } else {
        let b = c.u32()?;
        if b != 0 {
            s.volumes = kd_file(&mut c)?;
        }
    }
    Ok(s)
}

/// Decodes a record of rdb 1000013 given its `version` column.
pub fn parse(version: u32, d: &[u8]) -> Result<Surface> {
    match version {
        4 => parse_v4(d),
        5 => parse_v5(d),
        v => bail!("unsupported surface record version {v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    /// Real version 4 record 296486101 (playfield 4524 zone 1237): one volume, 5 vertices, 6 triangles.
    #[test]
    fn kd_version4_record() {
        let d = hex("8C00000078DA636060606800626F9790A2D454B7CC9C5406462017849981981588231C5FBBECFBECEE7427C0C8459EC1862191C19401018E33DA33A832180159820C6C68AA7B125EBB70684439E5561AB93030323133323031323330B13033B03031B38074FF0702469851001D721A3B000000000000000000000000");
        let s = parse(4, &d).unwrap();
        assert_eq!(s.volumes.len(), 1);
        let v = &s.volumes[0];
        assert_eq!((v.verts.len(), v.tris.len()), (5, 6));
        assert_eq!(v.tris[0], [0, 1, 2]);
        assert!((v.verts[0][0] - 1882.352).abs() < 1e-2 && (v.verts[4][1] - 54.538).abs() < 1e-2);
        assert!(v.verts.iter().all(|p| (0..3).all(|k| p[k] >= v.min[k] - 1e-2 && p[k] <= v.max[k] + 1e-2)));
        assert!(parse(4, &d[..d.len() - 20]).is_err(), "a truncated record is an error");
    }

    /// Real version 5 record 33095760 (playfield 505 zone 80): one volume, 6 triangles; the end markers 0x5a / 0x79 and the
    /// node count validate the whole range-coded stream.
    #[test]
    fn kd_version5_record() {
        let d = hex("112713E196C800B58200A29689627600BD2A00A88C000719AC52A2C79FE250D5710F95F217DF000000000000000000000000");
        let s = parse(5, &d).unwrap();
        assert_eq!((s.volumes.len(), s.nodes), (1, 1));
        let v = &s.volumes[0];
        assert_eq!(v.tris.len(), 6);
        assert!(v.tris.iter().flatten().all(|&i| (i as usize) < v.verts.len()));
        assert!(v.verts.iter().all(|p| (0..3).all(|k| p[k] >= v.min[k] - 1e-2 && p[k] <= v.max[k] + 1e-2)));
        // a flipped byte desynchronises the stream: an error or a different mesh, never the same one
        let mut bad = d.clone();
        bad[3] ^= 0x40;
        assert!(parse(5, &bad).map_or(true, |b| b.volumes.first().is_none_or(|w| w.verts != v.verts)));
    }

    /// Arbitrary bytes never panic (malformed records are `Err`).
    #[test]
    fn kd_garbage_never_panics() {
        // arbitrary bytes must never panic: either an Err or a (meaningless) Ok
        for seed in 0u32..200 {
            let d: Vec<u8> = (0..64).map(|i| (seed.wrapping_mul(2654435761).wrapping_add(i * 40503) >> 7) as u8).collect();
            let _ = parse_v5(&d);
            let _ = parse_v4(&d);
        }
        assert!(parse(7, &[]).is_err());
    }
}
