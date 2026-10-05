//! Liquid polygons: the first part of the `RDBPlayfieldAnarchy_t` tail that follows the zone / room
//! list of a playfield record (rdb 1000001), see `record::Record::tail`.
//!
//! ```text
//! u32 n; n x { u32 kind, u32 nv, nv x f32[3] vertex (AO world x,y,z), u32 nt, nt x u16[3] triangle }
//! ```
//! This is the `n3WaterData_t` array that `PlayfieldAnarchy_t::GetWaters` returns (count @ +0x98, data @ +0x9c;
//! `n3Zone_t::SetWater` N3 @0x1001abf2 keeps the highest water level per zone). The record parses with this
//! layout in all 601 playfields (outdoor and dungeon); the data after it is `environment`.
//!
//! `kind` is the `VisualWater_t::Create` liquid type (DisplaySystem @0x1003ad24): bit 0 = sloped/flowing
//! polygon (river, fall), `kind >> 1` selects the liquid. The liquid colours are the under-water fog colours of
//! `FUN_100b7273` (Gamecode, the camera-in-liquid check): `AddFog(r,g,b,density)` per `kind >> 1`.

use anyhow::{ensure, Result};
use ao_scene::{Blend, Mesh, Submesh, Vertex};

use super::record::Rd;

#[derive(Debug, Clone, PartialEq)]
pub struct Water {
    pub kind: u32,
    pub verts: Vec<[f32; 3]>,
    pub tris: Vec<[u16; 3]>,
}

pub fn parse(r: &mut Rd) -> Result<Vec<Water>> {
    let n = r.u32()? as usize;
    ensure!(n < 100_000, "implausible liquid count {n}");
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = r.u32()?;
        let nv = r.u32()? as usize;
        ensure!(nv < 100_000, "implausible liquid vertex count {nv}");
        let verts = (0..nv).map(|_| r.vec3()).collect::<Result<Vec<_>>>()?;
        let nt = r.u32()? as usize;
        ensure!(nt < 100_000, "implausible liquid triangle count {nt}");
        let mut tris = Vec::with_capacity(nt);
        for _ in 0..nt {
            let t = [r.u16()?, r.u16()?, r.u16()?];
            ensure!(t.iter().all(|&i| (i as usize) < nv), "liquid triangle index out of range");
            tris.push(t);
        }
        out.push(Water { kind, verts, tris });
    }
    Ok(out)
}

/// Linear RGB of a liquid and the opacity it is drawn with. Colours are the under-water fog colours of
/// Gamecode `FUN_100b7273` (sRGB-ish floats from the data section), `kind >> 1`:
/// 0/5/6/9 water, 1 lava, 2 slime, 3 acid, 4 mud; other kinds fall back to water.
fn tint(kind: u32) -> [f32; 4] {
    let (rgb, a): ([f32; 3], f32) = match (kind >> 1) & 0xff {
        1 => ([1.0, 0.0, 0.0], 0.9),
        2 => ([0.062, 0.114, 0.065], 0.85),
        3 => ([0.154, 0.991, 0.02], 0.8),
        4 => ([0.2, 0.1175, 0.0545], 0.9),
        _ => ([0.1, 0.3, 0.25], 0.6),
    };
    let c = rgb.map(super::environment::srgb_to_linear);
    [c[0], c[1], c[2], a]
}

/// One translucent mesh holding every liquid polygon (scene space, z negated), one submesh per tint.
pub fn build_mesh(waters: &[Water]) -> Option<Mesh> {
    let mut mesh = Mesh::default();
    let mut by_tint: Vec<(u32, Vec<u32>)> = Vec::new();
    for w in waters.iter().filter(|w| !w.tris.is_empty()) {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(w.verts.iter().map(|v| Vertex { pos: [v[0], v[1], -v[2]], ..Default::default() }));
        let key = (w.kind >> 1) & 0xff;
        let idx = match by_tint.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                by_tint.push((key, Vec::new()));
                by_tint.len() - 1
            }
        };
        by_tint[idx].1.extend(w.tris.iter().flatten().map(|&i| base + i as u32));
    }
    if mesh.vertices.is_empty() {
        return None;
    }
    for (key, indices) in by_tint {
        let mut s = Submesh::new(indices, None);
        s.blend = Blend::AlphaBlend;
        s.two_sided = true;
        s.base_color = tint(key << 1);
        mesh.submeshes.push(s);
    }
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(1u32.to_le_bytes()); // one liquid
        b.extend(2u32.to_le_bytes()); // kind 2: lava
        b.extend(3u32.to_le_bytes());
        for v in [[0.0f32, 5.0, 0.0], [10.0, 5.0, 0.0], [0.0, 5.0, 10.0]] {
            for c in v {
                b.extend(c.to_le_bytes());
            }
        }
        b.extend(1u32.to_le_bytes());
        for i in [0u16, 1, 2] {
            b.extend(i.to_le_bytes());
        }
        b
    }

    #[test]
    fn parses_liquid_and_builds_mesh() {
        let d = fixture();
        let mut r = Rd::new(&d, 0);
        let w = parse(&mut r).unwrap();
        assert_eq!(r.o, d.len());
        assert_eq!(w, vec![Water { kind: 2, verts: vec![[0.0, 5.0, 0.0], [10.0, 5.0, 0.0], [0.0, 5.0, 10.0]], tris: vec![[0, 1, 2]] }]);
        let m = build_mesh(&w).unwrap();
        assert_eq!(m.vertices[2].pos, [0.0, 5.0, -10.0]); // z negated
        assert_eq!(m.submeshes.len(), 1);
        assert_eq!(m.submeshes[0].blend, Blend::AlphaBlend);
        assert_eq!(m.submeshes[0].base_color[..3], [1.0, 0.0, 0.0]); // lava = red
    }

    #[test]
    fn rejects_bad_triangle_index() {
        let mut d = fixture();
        let n = d.len();
        d[n - 2] = 9; // last index 9 >= 3 vertices
        assert!(parse(&mut Rd::new(&d, 0)).is_err());
    }
}
