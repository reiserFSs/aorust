//! Statel file (rdb 1000003; `n3StatelController_t` / `n3StatelLoader_t` in N3.dll).
//!
//! ```text
//! u32 version (=1)
//! u32 offset[count]                 zone/room i = bytes [offset[i], offset[i+1]) (last: to end)
//! global data: u32 v; if v != 4 { u16 n; n x statel; 2 x { u16 n; n x 18 byte fog/sound entry } }
//! zone, outdoor layout (heightfield playfields):
//!   u16 k; k x u16; u16 n; n x statel; u16 n; n x statel;   (4 statel lists in total)
//!   u16 n; n x statel; u16 n; n x statel; u16 n; n x light
//! zone, dungeon layout:
//!   u32 size; size bytes; 2 x { u16 n; n x 18 byte entry }; then the same last three lists
//! statel := f32 x,y,z; u32 flags; u32 mesh_id; u8 scale; u8 nattr; attrs; [u32 colour if flags & 4]
//! ```

use anyhow::{bail, ensure, Result};

use super::record::Rd;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Statel {
    pub pos: [f32; 3],
    pub flags: u32,
    /// Static mesh record (rdb 1010001).
    pub mesh: u32,
    /// `scale/100 + 0.1` is the uniform scale.
    pub scale: u8,
}

#[derive(Debug, Default)]
pub struct StatelFile {
    pub global: Vec<Statel>,
    pub zones: Vec<Vec<Statel>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    Outdoor,
    Dungeon,
}

fn statel(r: &mut Rd) -> Result<Statel> {
    let pos = r.vec3()?;
    let flags = r.u32()?;
    let mesh = r.u32()?;
    let scale = r.u8()?;
    let mut n = r.u8()?;
    while n != 0 {
        let mask = r.u32()?;
        for _ in 0..mask.count_ones() {
            r.u32()?;
            n = n.wrapping_sub(1);
        }
    }
    if flags & 4 != 0 {
        r.u32()?; // colour
    }
    Ok(Statel { pos, flags, mesh, scale })
}

fn statels(r: &mut Rd, out: &mut Vec<Statel>) -> Result<()> {
    for _ in 0..r.u16()? {
        out.push(statel(r)?);
    }
    Ok(())
}

fn fog_lists(r: &mut Rd) -> Result<()> {
    for _ in 0..2 {
        let n = r.u16()? as usize;
        r.skip(n * 18)?;
    }
    Ok(())
}

fn lights(r: &mut Rd) -> Result<()> {
    for _ in 0..r.u16()? {
        r.skip(16)?;
        let kind = r.u8()?;
        r.skip(3)?;
        match kind & 0x1f {
            2 => r.skip(8)?,
            4 => r.skip(12)?,
            _ => {}
        }
    }
    Ok(())
}

fn zone(d: &[u8], a: usize, b: usize, layout: Layout) -> Result<Vec<Statel>> {
    ensure!(a <= b && b <= d.len(), "bad zone range {a}..{b}");
    let mut r = Rd::new(&d[..b], a);
    let mut out = Vec::new();
    match layout {
        Layout::Outdoor => {
            let k = r.u16()? as usize;
            r.skip(2 * k)?;
            statels(&mut r, &mut out)?;
            statels(&mut r, &mut out)?;
        }
        Layout::Dungeon => {
            let size = r.u32()? as usize;
            r.skip(size)?;
            fog_lists(&mut r)?;
        }
    }
    statels(&mut r, &mut out)?;
    statels(&mut r, &mut out)?;
    lights(&mut r)?;
    ensure!(r.o == b, "zone {a}..{b}: {} unparsed bytes", b - r.o);
    Ok(out)
}

fn parse_layout(d: &[u8], count: usize, layout: Layout) -> Result<StatelFile> {
    let mut r = Rd::new(d, 0);
    ensure!(r.u32()? == 1, "unsupported statel file version");
    let offs: Vec<usize> = (0..count).map(|_| r.u32().map(|v| v as usize)).collect::<Result<_>>()?;
    let mut global = Vec::new();
    // The global section only exists when zone 0 does not start right after the offset table.
    if offs.first().is_some_and(|&o| o > r.o) && r.u32()? != 4 {
        statels(&mut r, &mut global)?;
        fog_lists(&mut r)?;
    }
    let mut zones = Vec::with_capacity(count);
    for (i, &a) in offs.iter().enumerate() {
        let b = offs.get(i + 1).copied().unwrap_or(d.len());
        zones.push(zone(d, a, b, layout)?);
    }
    Ok(StatelFile { global, zones })
}

/// Parses with the layout expected for the playfield type, falling back to the other one
/// (playfield 4622 has its own heightfield but dungeon-style zones).
pub fn parse(d: &[u8], count: usize, preferred: Layout) -> Result<StatelFile> {
    let other = if preferred == Layout::Outdoor { Layout::Dungeon } else { Layout::Outdoor };
    match parse_layout(d, count, preferred) {
        Ok(f) => Ok(f),
        Err(e1) => match parse_layout(d, count, other) {
            Ok(f) => Ok(f),
            Err(e2) => bail!("statel file: {e1:#} / {e2:#}"),
        },
    }
}

/// 3x3 rotation (row-major, column-vector convention, AO's left-handed space, right-hand-rule
/// numerics) and per-axis scale of a statel (`FUN_1002777a`). `R = Rz(roll) Rx(pitch) Ry(heading)`.
pub fn orientation(flags: u32, scale: u8) -> ([[f32; 3]; 3], [f32; 3]) {
    let u = flags >> 7;
    let s = scale as f32 / 100.0 + 0.1;
    let deg = std::f32::consts::PI / 180.0;
    if flags & 1 == 0 {
        let (nine, rem) = if u < 0x163f500 { (u / 180, u % 180) } else { (u.wrapping_add(0xfe9c0b00), 180) };
        let r = mul(rz((nine / 360) as f32 * deg), mul(rx((rem as f32 - 90.0) * deg), ry((nine % 360) as f32 * deg)));
        (r, [s; 3])
    } else {
        // Heading in 1/630 turns plus a stretch factor; the trailing translation term of the
        // original (`FUN_10026d5c`) is not applied.
        let k = ((u / 630) % 0xd3) as f32 / 100.0 + 0.5;
        (ry((u % 630) as f32 * std::f32::consts::TAU / 630.0), [s * k, s, s])
    }
}

pub fn ry(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}
fn rx(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}
fn rz(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}
pub fn mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statel_bytes(x: f32, flags: u32, mesh: u32, nattr: u8) -> Vec<u8> {
        let mut d = Vec::new();
        for v in [x, 1.0, 2.0] {
            d.extend(v.to_le_bytes());
        }
        d.extend(flags.to_le_bytes());
        d.extend(mesh.to_le_bytes());
        d.extend([90, nattr]);
        if nattr == 3 {
            d.extend(7u32.to_le_bytes());
            d.extend([0u8; 12]);
        }
        d
    }

    #[test]
    fn zone_layouts() {
        // outdoor: k=0, list A empty, list B with one statel, then C, D, lights empty
        let mut o = Vec::new();
        o.extend(0u16.to_le_bytes());
        o.extend(0u16.to_le_bytes());
        o.extend(1u16.to_le_bytes());
        o.extend(statel_bytes(5.0, 0x2d00, 201717, 3));
        o.extend([0u8; 6]);
        let s = zone(&o, 0, o.len(), Layout::Outdoor).unwrap();
        assert_eq!(s, vec![Statel { pos: [5.0, 1.0, 2.0], flags: 0x2d00, mesh: 201717, scale: 90 }]);
        // dungeon: size=0, 2 empty fog lists, C=1 statel, D, lights
        let mut dg = vec![0u8; 4 + 4];
        dg.extend(1u16.to_le_bytes());
        dg.extend(statel_bytes(-3.0, 0x2d00, 6255, 0));
        dg.extend([0u8; 4]);
        assert_eq!(zone(&dg, 0, dg.len(), Layout::Dungeon).unwrap()[0].mesh, 6255);
        // the same bytes are not a valid outdoor zone
        assert!(zone(&dg, 0, dg.len(), Layout::Outdoor).is_err());
    }

    #[test]
    fn identity_and_heading() {
        // flags 0x2d00: u = 90 -> heading 0, pitch 0, roll 0; scale byte 90 -> 1.0
        let (r, s) = orientation(0x2d00, 90);
        assert_eq!(r, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!((s[0] - 1.0).abs() < 1e-6);
        // heading 90: u = 90 + 180*90
        let (r, _) = orientation((90 + 180 * 90) << 7, 90);
        assert!((r[0][2] - 1.0).abs() < 1e-5 && r[0][0].abs() < 1e-5); // x' = z
    }
}
