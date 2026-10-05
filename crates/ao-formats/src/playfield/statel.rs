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
    /// Per zone/room lights (same indexing as `zones`).
    pub lights: Vec<Vec<Light>>,
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

/// `FUN_10026e66` (N3 @0x10026e66) -> `VisualMesh_t::SetLight` (DisplaySystem @0x1006b6b0) -> `RLight_t`
/// (randy31 @0x1003fd4a), whose `light_info` block is a `D3DLIGHT7`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    /// Zone/room-local position.
    pub pos: [f32; 3],
    /// Orientation word (`flags >> 7` as for statels); a spot shines along its local +X.
    pub flags: u32,
    /// Type byte: bits 0..4 light type (2 = point, 4 = spot), bit 5 group flag, bit 6 selects the
    /// `att2` divisor, bit 7 = the client creates no light at all.
    pub kind: u8,
    /// Diffuse colour bytes (`/255`).
    pub rgb: [u8; 3],
    /// `D3DLIGHT7.dvRange` (metres).
    pub range: f32,
    /// `D3DLIGHT7.dvAttenuation0..2`.
    pub att: [f32; 3],
    /// Spot half angles in radians (`SetSpotAngles` doubles them into theta/phi).
    pub cone: [f32; 2],
}

fn lights(r: &mut Rd, out: &mut Vec<Light>) -> Result<()> {
    for _ in 0..r.u16()? {
        let pos = r.vec3()?;
        let flags = r.u32()?;
        let kind = r.u8()?;
        let rgb = [r.u8()?, r.u8()?, r.u8()?];
        let mut l = Light { pos, flags, kind, rgb, range: 0.0, att: [0.0; 3], cone: [0.0; 2] };
        if matches!(kind & 0x1f, 2 | 4) {
            l.range = r.u16()? as f32;
            l.att = [r.u16()? as f32 / 100.0, r.u16()? as f32 / 10_000.0, r.u16()? as f32 / if kind & 0x40 != 0 { 10_000.0 } else { 1_000_000.0 }];
            if kind & 0x1f == 4 {
                let deg = std::f32::consts::TAU / 360.0;
                l.cone = [r.u16()? as f32 * deg, r.u16()? as f32 * deg];
            }
        }
        out.push(l);
    }
    Ok(())
}

/// Below this `D3DLIGHT7` attenuation factor a light is treated as dark (calibration guess).
const FAINT: f32 = 0.05;

/// Maps a statel light onto the contract's `ao_scene::Light` (linear falloff to 0 at `range`).
///
/// * `kind & 0x80` lights are never created by the client (`FUN_10026e66` returns null); lights with
///   `range == 0` are culled by `RVisual_t::CullLights` (`0 < dvRange` test) -> `None`.
/// * D3D7 intensity is `1/(a0 + a1 d + a2 d^2)` up to `range`. The effective range is where it drops below
///   [`FAINT`] (at most `range`); the peak `k` is the least-squares fit of `k (1 - d/R)` to `min(1, I(d))`
///   capped at 1 (the framebuffer saturates). The client does its light maths on gamma values, so the colour is
///   `(k * rgb/255)^2.2` like the ambient colour.
/// * Spots (type 4) become point lights: the contract has no cones, so the light is moved along its axis
///   by `R/2 (1 - sin(half angle))` (wide cones stay put, narrow ones light the area in front) and its range
///   shrinks accordingly. A guess, no attempt at the cone edge.
///
/// `frame` = `(rotation, translation)` of the owning dungeon room (AO space); `None` outdoors.
pub fn scene_light(l: &Light, frame: Option<([[f32; 3]; 3], [f32; 3])>) -> Option<ao_scene::Light> {
    if l.kind & 0x80 != 0 || !matches!(l.kind & 0x1f, 2 | 4) || l.range <= 0.0 {
        return None;
    }
    let inv = |d: f32| (l.att[0] + d * (l.att[1] + d * l.att[2])).recip();
    const N: usize = 256;
    let step = l.range / N as f32;
    let reach = (0..=N).map(|i| i as f32 * step).find(|&d| inv(d).partial_cmp(&FAINT).is_none_or(|o| o.is_lt())).unwrap_or(l.range);
    if reach <= 0.0 {
        return None;
    }
    // k = 3 * integral_0^1 (1 - x) min(1, I(xR)) dx  (midpoint rule)
    let k = (3.0 * (0..N).map(|i| { let x = (i as f32 + 0.5) / N as f32; (1.0 - x) * inv(x * reach).min(1.0) }).sum::<f32>() / N as f32).min(1.0);
    let mut pos = l.pos;
    let mut range = reach;
    if l.kind & 0x1f == 4 {
        let r = orientation(l.flags, 90);
        let off = 0.5 * reach * (1.0 - l.cone[1].sin().abs());
        for i in 0..3 {
            pos[i] += r[i][0] * off;
        }
        range -= off;
    }
    if let Some((q, t)) = frame {
        pos = [0, 1, 2].map(|i| q[i][0] * pos[0] + q[i][1] * pos[1] + q[i][2] * pos[2] + t[i]);
    }
    let color = l.rgb.map(|c| (k * c as f32 / 255.0).powf(2.2));
    Some(ao_scene::Light { pos: [pos[0], pos[1], -pos[2]], color, range })
}

fn zone(d: &[u8], a: usize, b: usize, layout: Layout) -> Result<(Vec<Statel>, Vec<Light>)> {
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
    let mut lit = Vec::new();
    lights(&mut r, &mut lit)?;
    ensure!(r.o == b, "zone {a}..{b}: {} unparsed bytes", b - r.o);
    Ok((out, lit))
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
    let mut lit = Vec::with_capacity(count);
    for (i, &a) in offs.iter().enumerate() {
        let b = offs.get(i + 1).copied().unwrap_or(d.len());
        let (z, l) = zone(d, a, b, layout)?;
        zones.push(z);
        lit.push(l);
    }
    Ok(StatelFile { global, zones, lights: lit })
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

/// Linear part (3x3, row-major, column-vector convention, AO's left-handed space, right-hand-rule
/// numerics) of a statel transform (`FUN_1002777a`): `R * S` with `R = Rz(roll) Rx(pitch) Ry(heading)`.
///
/// `flags & 1 == 0`: uniform scale `scale/100 + 0.1`. `flags & 1`: heading in 1/630 turns, stretch
/// `k` and a shear `t` (`FUN_10026cdc(1, 1/k, 1/k)` then `FUN_10026d5c(t, 0)` build the anim matrix
/// `[[1, t, 0], [0, 1/k, 0], [0, 0, 1/k]]` in row-vector form, applied before the uniform scale
/// `s*k` and the rotation): `x' = s*k*x`, `y' = s*(k*t*x + y)`, `z' = s*z`.
pub fn orientation(flags: u32, scale: u8) -> [[f32; 3]; 3] {
    let u = flags >> 7;
    let s = scale as f32 / 100.0 + 0.1;
    let deg = std::f32::consts::PI / 180.0;
    if flags & 1 == 0 {
        let (nine, rem) = if u < 0x163f500 { (u / 180, u % 180) } else { (u.wrapping_add(0xfe9c0b00), 180) };
        let r = mul(rz((nine / 360) as f32 * deg), mul(rx((rem as f32 - 90.0) * deg), ry((nine % 360) as f32 * deg)));
        mul(r, [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]])
    } else {
        let q = u / 630;
        let k = (q % 211) as f32 / 100.0 + 0.5;
        let t = (q / 211) as f32 / 100.0 - 1.25;
        let r = ry((u % 630) as f32 * std::f32::consts::TAU / 630.0);
        mul(r, [[s * k, 0.0, 0.0], [s * k * t, s, 0.0], [0.0, 0.0, s]])
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
        let (s, l) = zone(&o, 0, o.len(), Layout::Outdoor).unwrap();
        assert!(l.is_empty());
        assert_eq!(s, vec![Statel { pos: [5.0, 1.0, 2.0], flags: 0x2d00, mesh: 201717, scale: 90 }]);
        // dungeon: size=0, 2 empty fog lists, C=1 statel, D, lights
        let mut dg = vec![0u8; 4 + 4];
        dg.extend(1u16.to_le_bytes());
        dg.extend(statel_bytes(-3.0, 0x2d00, 6255, 0));
        dg.extend([0u8; 4]);
        assert_eq!(zone(&dg, 0, dg.len(), Layout::Dungeon).unwrap().0[0].mesh, 6255);
        // the same bytes are not a valid outdoor zone
        assert!(zone(&dg, 0, dg.len(), Layout::Outdoor).is_err());
    }

    fn light_bytes(kind: u8, extra: &[u16]) -> Vec<u8> {
        let mut d = Vec::new();
        for v in [1.0f32, 2.0, 3.0] {
            d.extend(v.to_le_bytes());
        }
        d.extend(0x2d00u32.to_le_bytes());
        d.extend([kind, 255, 128, 0]);
        for v in extra {
            d.extend(v.to_le_bytes());
        }
        d
    }

    #[test]
    fn light_kinds() {
        // point (2), spot (4) with the att2 divisor flag (bit 6), and a type without extra data (1)
        let mut d = 3u16.to_le_bytes().to_vec();
        d.extend(light_bytes(2, &[30, 100, 250, 50]));
        d.extend(light_bytes(4 | 0x40, &[12, 150, 0, 400, 45, 90]));
        d.extend(light_bytes(1, &[]));
        let mut out = Vec::new();
        let mut r = Rd::new(&d, 0);
        lights(&mut r, &mut out).unwrap();
        assert_eq!(r.o, d.len());
        assert_eq!((out[0].pos, out[0].rgb, out[0].range, out[0].att), ([1.0, 2.0, 3.0], [255, 128, 0], 30.0, [1.0, 0.025, 0.00005]));
        assert_eq!(out[1].att, [1.5, 0.0, 0.04]);
        assert!((out[1].cone[0] - 45f32.to_radians()).abs() < 1e-5 && (out[1].cone[1] - 90f32.to_radians()).abs() < 1e-5);
        assert_eq!((out[2].range, out[2].att), (0.0, [0.0; 3]));
    }

    #[test]
    fn scene_light_mapping() {
        let l = |kind, range, att, cone| Light { pos: [1.0, 2.0, 3.0], flags: 0x2d00, kind, rgb: [255, 0, 255], range, att, cone };
        // disabled bit, no range, unsupported type -> no light
        assert!(scene_light(&l(0x82, 10.0, [1.0, 0.0, 0.0], [0.0; 2]), None).is_none());
        assert!(scene_light(&l(2, 0.0, [1.0, 0.0, 0.0], [0.0; 2]), None).is_none());
        assert!(scene_light(&l(1, 10.0, [1.0, 0.0, 0.0], [0.0; 2]), None).is_none());
        // constant intensity 1 inside the range: ramp fit 1.5 capped to 1; z is mirrored
        let p = scene_light(&l(2, 10.0, [1.0, 0.0, 0.0], [0.0; 2]), None).unwrap();
        assert_eq!((p.pos, p.range), ([1.0, 2.0, -3.0], 10.0));
        assert!((p.color[0] - 1.0).abs() < 1e-3 && p.color[1] == 0.0);
        // faint beyond 1/(1+d) < 0.05 -> reach 19 m even though range is 100
        let q = scene_light(&l(2, 100.0, [1.0, 1.0, 0.0], [0.0; 2]), None).unwrap();
        assert!((q.range - 19.0).abs() < 0.5);
        // room frame: rot 90 degrees about Y (x' = z) plus translation
        let f = Some((ry(std::f32::consts::FRAC_PI_2), [10.0, 0.0, 0.0]));
        let r = scene_light(&l(2, 10.0, [1.0, 0.0, 0.0], [0.0; 2]), f).unwrap();
        assert!((r.pos[0] - 13.0).abs() < 1e-4 && (r.pos[2] + -1.0).abs() < 1e-4);
        // narrow spot (half angle 0 deg) shifts along +X by R/2
        let s = scene_light(&l(4, 10.0, [1.0, 0.0, 0.0], [0.0, 0.0]), None).unwrap();
        assert!((s.pos[0] - 6.0).abs() < 1e-4 && (s.range - 5.0).abs() < 1e-4);
    }

    #[test]
    fn identity_and_heading() {
        // flags 0x2d00: u = 90 -> heading 0, pitch 0, roll 0; scale byte 90 -> 1.0
        let r = orientation(0x2d00, 90);
        assert_eq!(r, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        // heading 90: u = 90 + 180*90
        let r = orientation((90 + 180 * 90) << 7, 90);
        assert!((r[0][2] - 1.0).abs() < 1e-5 && r[0][0].abs() < 1e-5); // x' = z
    }

    #[test]
    fn stretch_shear_form() {
        // flags & 1: u = 630*(k100 + 211*t100) + heading; heading 0, k = 1.0 (k100 = 50), t = 0.5 (t100 = 175)
        let u = 630 * (50 + 211 * 175);
        let m = orientation((u << 7) | 1, 90);
        let (k, t) = (1.0f32, 0.5f32);
        assert!((m[0][0] - k).abs() < 1e-5 && (m[1][0] - k * t).abs() < 1e-5);
        assert!((m[1][1] - 1.0).abs() < 1e-5 && (m[2][2] - 1.0).abs() < 1e-5 && m[0][1].abs() < 1e-6);
        // heading 90 degrees (630/4 is not integral: use 315 = half turn): x -> -x, z -> -z, shear follows
        let m = orientation(((u + 315) << 7) | 1, 90);
        assert!((m[0][0] + k).abs() < 1e-4 && (m[1][0] - k * t).abs() < 1e-4 && (m[2][2] + 1.0).abs() < 1e-4);
    }
}
