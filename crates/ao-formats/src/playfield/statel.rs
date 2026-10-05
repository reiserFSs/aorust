//! Statel file (rdb 1000003; `n3StatelController_t` / `n3StatelLoader_t` in N3.dll).
//!
//! ```text
//! u32 version (=1)
//! u32 offset[count]                 zone/room i = bytes [offset[i], offset[i+1]) (last: to end)
//! global data: u32 v; if v != 4 { u16 n; n x statel; { u16 n; n x fog entry }; { u16 n; n x sound entry } }
//! zone, outdoor layout (heightfield playfields):
//!   u16 k; k x u16; u16 n; n x statel; u16 n; n x statel;   (4 statel lists in total)
//!   u16 n; n x statel; u16 n; n x statel; u16 n; n x light
//! zone, dungeon layout:
//!   u32 size; size bytes; { u16 n; n x fog entry }; { u16 n; n x sound entry }; then the same last three lists
//! statel := f32 x,y,z; u32 flags; u32 mesh_id; u8 scale; u8 nattr; attrs; [u32 colour if flags & 4]
//! fog / sound entry := f32 x,y,z; u32 value; u16 radius      (`FUN_10027d90`, N3 @0x10027d90)
//! ```

use anyhow::{bail, ensure, Result};

use super::record::Rd;

#[derive(Debug, Clone, PartialEq)]
pub struct Statel {
    pub pos: [f32; 3],
    pub flags: u32,
    /// Static mesh record (rdb 1010001).
    pub mesh: u32,
    /// `scale/100 + 0.1` is the uniform scale.
    pub scale: u8,
    /// `flags & 4`: per-instance colour word (the client keeps it with the statel; 9 212 of 2.36 M statels carry one).
    pub colour: Option<u32>,
    /// `(bit index, value)` attribute pairs: texture overrides `(SimpleMesh slot, rdb 1010004 id)` (`NewTextureData_t`).
    pub attrs: Vec<(u8, u32)>,
    /// Which of the four statel lists of the zone it came from (0..3; the lists are the zone's distance classes, see docs).
    pub list: u8,
}

/// 18 byte fog or sound entry of the statel file (`n3StatelFog_t` / `n3StatelSound_t`, 0x14 bytes in memory with an
/// active flag at +0x12). Position is zone-local for dungeon rooms, world space otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    pub pos: [f32; 3],
    /// Fog: little endian `B, G, R, intensity %` (`StatelFogRun` N3 @0x10024dbc reads bytes +0xc/+0xd/+0xe as blue/green/red
    /// and +0xf as intensity; `FUN_10027d90` clamps the intensity byte to 100). Sound: game sound id (`PlayGameSound`).
    pub value: u32,
    /// Influence radius in metres.
    pub radius: u16,
}

/// Everything the statel file holds for one zone (outdoor tile block or dungeon room); `global` is zone-independent.
#[derive(Debug, Default)]
pub struct Zone {
    pub statels: Vec<Statel>,
    pub lights: Vec<Light>,
    pub fogs: Vec<Emitter>,
    pub sounds: Vec<Emitter>,
}

#[derive(Debug, Default)]
pub struct StatelFile {
    pub global: Zone,
    pub zones: Vec<Zone>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    Outdoor,
    Dungeon,
}

fn statel(r: &mut Rd, list: u8) -> Result<Statel> {
    let pos = r.vec3()?;
    let flags = r.u32()?;
    let mesh = r.u32()?;
    let scale = r.u8()?;
    let mut n = r.u8()?;
    let mut attrs = Vec::new();
    while n != 0 {
        let mask = r.u32()?;
        for bit in (0..32u8).filter(|b| mask >> b & 1 != 0) {
            attrs.push((bit, r.u32()?));
            n = n.wrapping_sub(1);
        }
    }
    let colour = if flags & 4 != 0 { Some(r.u32()?) } else { None };
    Ok(Statel { pos, flags, mesh, scale, colour, attrs, list })
}

fn statels(r: &mut Rd, out: &mut Vec<Statel>, list: u8) -> Result<()> {
    for _ in 0..r.u16()? {
        out.push(statel(r, list)?);
    }
    Ok(())
}

/// `FUN_10027d90`: fog list then sound list. Dungeon entries are room-local: the client turns them by the room rotation
/// (`FUN_1003727d`, a Y rotation of an identity matrix) and adds the room position, exactly like statels.
fn emitters(r: &mut Rd, zone: &mut Zone) -> Result<()> {
    for list in 0..2 {
        for _ in 0..r.u16()? {
            let pos = r.vec3()?;
            let (value, radius) = (r.u32()?, r.u16()?);
            let value = if list == 0 && value >> 24 > 100 { value & 0xff_ffff | 100 << 24 } else { value };
            [&mut zone.fogs, &mut zone.sounds][list].push(Emitter { pos, value, radius });
        }
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

/// Maps a statel light onto the contract's `ao_scene::Light`, which carries the `D3DLIGHT7` fields the client sets
/// (`RLight_t`, randy31 @0x1003fd4a): colour, `dvRange`, `dvAttenuation0..2` and, for spots, axis / `dvTheta` / `dvPhi`
/// (`dvFalloff` = 1). The renderer evaluates the D3D7 fixed function formula, nothing is fitted.
///
/// * `kind & 0x80` lights are never created by the client (`FUN_10026e66` returns null); lights with
///   `range == 0` are culled by `RVisual_t::CullLights` (`0 < dvRange` test) -> `None`.
/// * The client does its light maths on gamma values, so the colour is `(rgb/255)^2.2` like the ambient colour.
/// * A spot shines along its local +X; its half angles are doubled into `theta` / `phi` (`SetSpotAngles`).
///
/// `frame` = `(rotation, translation)` of the owning dungeon room (AO space); `None` outdoors.
pub fn scene_light(l: &Light, frame: Option<([[f32; 3]; 3], [f32; 3])>) -> Option<ao_scene::Light> {
    if l.kind & 0x80 != 0 || !matches!(l.kind & 0x1f, 2 | 4) || l.range <= 0.0 {
        return None;
    }
    let to_world = |v: [f32; 3], translate: bool| match frame {
        Some((q, t)) => [0, 1, 2].map(|i| q[i][0] * v[0] + q[i][1] * v[1] + q[i][2] * v[2] + if translate { t[i] } else { 0.0 }),
        None => v,
    };
    let scene = |v: [f32; 3]| [v[0], v[1], -v[2]];
    let spot = (l.kind & 0x1f == 4).then(|| {
        let r = orientation(l.flags, 90);
        ao_scene::Spot { dir: scene(to_world([r[0][0], r[1][0], r[2][0]], false)), theta: 2.0 * l.cone[0], phi: 2.0 * l.cone[1] }
    });
    // all zero attenuation would be 1/0 in D3D (full intensity): keep it distinct from the contract's "linear" marker
    let atten = if l.att == [0.0; 3] { [f32::MIN_POSITIVE, 0.0, 0.0] } else { l.att };
    Some(ao_scene::Light { pos: scene(to_world(l.pos, true)), color: l.rgb.map(|c| (c as f32 / 255.0).powf(2.2)), range: l.range, atten, spot })
}

fn zone(d: &[u8], a: usize, b: usize, layout: Layout) -> Result<Zone> {
    ensure!(a <= b && b <= d.len(), "bad zone range {a}..{b}");
    let mut r = Rd::new(&d[..b], a);
    let mut z = Zone::default();
    match layout {
        Layout::Outdoor => {
            let k = r.u16()? as usize;
            r.skip(2 * k)?;
            statels(&mut r, &mut z.statels, 0)?;
            statels(&mut r, &mut z.statels, 1)?;
        }
        Layout::Dungeon => {
            let size = r.u32()? as usize;
            r.skip(size)?;
            emitters(&mut r, &mut z)?;
        }
    }
    statels(&mut r, &mut z.statels, 2)?;
    statels(&mut r, &mut z.statels, 3)?;
    lights(&mut r, &mut z.lights)?;
    ensure!(r.o == b, "zone {a}..{b}: {} unparsed bytes", b - r.o);
    Ok(z)
}

fn parse_layout(d: &[u8], count: usize, layout: Layout) -> Result<StatelFile> {
    let mut r = Rd::new(d, 0);
    ensure!(r.u32()? == 1, "unsupported statel file version");
    let offs: Vec<usize> = (0..count).map(|_| r.u32().map(|v| v as usize)).collect::<Result<_>>()?;
    let mut global = Zone::default();
    // The global section only exists when zone 0 does not start right after the offset table.
    if offs.first().is_some_and(|&o| o > r.o) && r.u32()? != 4 {
        statels(&mut r, &mut global.statels, 0)?;
        emitters(&mut r, &mut global)?;
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
            d.extend(0b10101u32.to_le_bytes()); // bits 0, 2, 4
            for v in [40759u32, 0, 6315] {
                d.extend(v.to_le_bytes());
            }
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
        let z = zone(&o, 0, o.len(), Layout::Outdoor).unwrap();
        assert!(z.lights.is_empty());
        assert_eq!(z.statels, vec![Statel { pos: [5.0, 1.0, 2.0], flags: 0x2d00, mesh: 201717, scale: 90, colour: None, attrs: vec![(0, 40759), (2, 0), (4, 6315)], list: 1 }]);
        // dungeon: size=0, 2 empty fog lists, C=1 statel, D, lights
        let mut dg = vec![0u8; 4 + 4];
        dg.extend(1u16.to_le_bytes());
        dg.extend(statel_bytes(-3.0, 0x2d00, 6255, 0));
        dg.extend([0u8; 4]);
        assert_eq!(zone(&dg, 0, dg.len(), Layout::Dungeon).unwrap().statels[0].mesh, 6255);
        // the same bytes are not a valid outdoor zone
        assert!(zone(&dg, 0, dg.len(), Layout::Outdoor).is_err());
    }

    #[test]
    fn fog_and_sound_entries() {
        let entry = |x: f32, value: u32, radius: u16| {
            let mut d = Vec::new();
            for v in [x, 0.0, 2.0] {
                d.extend(v.to_le_bytes());
            }
            d.extend(value.to_le_bytes());
            d.extend(radius.to_le_bytes());
            d
        };
        // dungeon zone: empty blob, 2 fogs (the second with an intensity byte of 200 -> clamped to 100), 1 sound
        let mut z = 0u32.to_le_bytes().to_vec();
        z.extend(2u16.to_le_bytes());
        z.extend(entry(1.0, 0x32_40_30_20, 15));
        z.extend(entry(2.0, 0xc8_00_00_ff, 9));
        z.extend(1u16.to_le_bytes());
        z.extend(entry(-4.0, 1234, 60));
        z.extend([0u8; 6]);
        let z = zone(&z, 0, z.len(), Layout::Dungeon).unwrap();
        assert_eq!(z.fogs.len(), 2);
        assert_eq!((z.fogs[0].pos, z.fogs[0].value, z.fogs[0].radius), ([1.0, 0.0, 2.0], 0x32_40_30_20, 15));
        assert_eq!(z.fogs[1].value, 0x64_00_00_ff);
        assert_eq!((z.sounds[0].pos, z.sounds[0].value, z.sounds[0].radius), ([-4.0, 0.0, 2.0], 1234, 60));
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
        // D3D fields are passed through; z is mirrored; colour is gamma-converted
        let p = scene_light(&l(2, 10.0, [1.0, 0.5, 0.25], [0.0; 2]), None).unwrap();
        assert_eq!((p.pos, p.range, p.atten, p.spot), ([1.0, 2.0, -3.0], 10.0, [1.0, 0.5, 0.25], None));
        assert!((p.color[0] - 1.0).abs() < 1e-6 && p.color[1] == 0.0);
        // room frame: rot 90 degrees about Y (x' = z) plus translation
        let f = Some((ry(std::f32::consts::FRAC_PI_2), [10.0, 0.0, 0.0]));
        let r = scene_light(&l(2, 10.0, [1.0, 0.0, 0.0], [0.0; 2]), f).unwrap();
        assert!((r.pos[0] - 13.0).abs() < 1e-4 && (r.pos[2] + -1.0).abs() < 1e-4);
        // spot: axis = local +X (scene z mirrored), angles doubled
        let s = scene_light(&l(4, 10.0, [1.0, 0.0, 0.0], [0.2, 0.4]), None).unwrap().spot.unwrap();
        assert!((s.dir[0] - 1.0).abs() < 1e-5 && (s.theta - 0.4).abs() < 1e-6 && (s.phi - 0.8).abs() < 1e-6);
        // all-zero attenuation stays distinct from the contract's linear marker
        assert_ne!(scene_light(&l(2, 10.0, [0.0; 3], [0.0; 2]), None).unwrap().atten, [0.0; 3]);
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
