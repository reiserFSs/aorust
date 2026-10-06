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

use std::collections::hash_map::Entry;

use anyhow::{ensure, Result};
use ao_rdb::RecordStore;
use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};

use super::record::Rd;
use crate::character::NameTable;
use crate::texture::decode_texture;

const TEXTURES: u32 = 1_010_004;

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

/// How one liquid type is drawn: `VisualWater_t::Create` (DisplaySystem @0x1003ad24) → `FUN_1003a0d6` @0x1003a0d6 picks the
/// vertex colour (ARGB, `switch (kind)`, odd kinds are the sloped twin of the even one) and a render style, the style's
/// draw routine (`FUN_1003f3db` @0x1003f3db calls `FUN_1003b5e9` water, `FUN_1003c356` lava, `FUN_1003bebf` acid,
/// `FUN_1003d1b9` star water, `FUN_1003b411` shallow water) blends textured triangles with `uv = (x, z) / scale`.
struct Liquid {
    /// Vertex colour ARGB bytes (sRGB-ish, as D3D: the alpha is the vertex alpha, the water textures are JPEG).
    argb: u32,
    texture: &'static str,
    /// Metres per texture repeat (`DAT_100aebe4` 40 water, `DAT_100aebec` 8 lava, `DAT_100aebe8` 12 acid, 4 x 40 shallow).
    scale: f32,
    /// uv units per second (`FUN_1003b5e9`: `_DAT_10150174 -= dt * 0.01`, added to u, subtracted from v).
    scroll: [f32; 2],
}

const WATER_TEX: &str = "water5.png";
const SCROLL: [f32; 2] = [-0.01, 0.01];

/// Liquid of `kind` (`None` = the client draws nothing: kinds 16/17). Kinds above 19 (`switch` default: the colour is
/// whatever the stack holds) are drawn like plain water.
fn liquid(kind: u32) -> Option<Liquid> {
    let (argb, texture, scale, scroll) = match kind >> 1 {
        1 => (0xffff_0000, WATER_TEX, 8.0, [0.0; 2]), // lava (+ crust layer, see `build_mesh`)
        2 => (0xf21f_3a21, WATER_TEX, 40.0, SCROLL),  // slime
        3 => (0xf727_fc05, "toxicwater.png", 12.0, [0.0; 2]), // acid
        4 => (0xed66_3b1b, WATER_TEX, 40.0, SCROLL),  // mud
        5 => (0x9933_997f, WATER_TEX, 40.0, SCROLL),
        6 => (0xccc8_e6ff, WATER_TEX, 40.0, SCROLL),
        7 => (0xffff_ffff, "stars01.png", 40.0, [0.0; 2]), // star water
        8 => return None,
        9 => (0x3334_9dfa, WATER_TEX, 160.0, [0.0; 2]), // shallow water
        _ => (0xcc00_186a, WATER_TEX, 40.0, SCROLL),   // water
    };
    Some(Liquid { argb, texture, scale, scroll })
}

/// Crust of the lava: `env_lava2.png` (RGBA, alpha 60..255) blended over the lava with its own alpha; the client multiplies the
/// alphas of two copies at 75 m and 250 m (`FUN_1003c356`, blob 0x7e8), we draw one at 75 m with the mean of the product.
const CRUST_TEX: &str = "env_lava2.png";
const CRUST_SCALE: f32 = 75.0;
const CRUST_ALPHA: f32 = 0.84;

fn argb_to_linear(argb: u32) -> [f32; 4] {
    let b = |s: u32| (argb >> s & 0xff) as f32 / 255.0;
    let c = |s| super::environment::srgb_to_linear(b(s));
    [c(16), c(8), c(0), b(24)]
}

fn texture(store: &RecordStore, names: &NameTable, scene: &mut Scene, name: &str) -> Option<TextureKey> {
    let key = TextureKey { rdb_type: TEXTURES, id: names.id(TEXTURES, name)? };
    if let Entry::Vacant(e) = scene.textures.entry(key) {
        e.insert(decode_texture(&store.get(TEXTURES, key.id).ok()??).ok()?);
    }
    Some(key)
}

/// Adds one translucent mesh holding every liquid polygon (scene space, z negated) to `scene`: one submesh per liquid
/// kind, textured as the client does (uv in metres / `Liquid::scale`, AO x/z); lava gets a second, crust submesh.
pub fn emit(store: &RecordStore, waters: &[Water], scene: &mut Scene) {
    let Ok(names) = NameTable::load(store) else { return };
    let Some(mesh) = build_mesh(waters, &mut |n| texture(store, &names, scene, n)) else { return };
    scene.meshes.push(mesh);
    scene.instances.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
}

/// `tex` resolves a texture name to a scene texture.
fn build_mesh(waters: &[Water], tex: &mut dyn FnMut(&str) -> Option<TextureKey>) -> Option<Mesh> {
    let mut mesh = Mesh::default();
    let mut crust: Vec<u32> = Vec::new();
    let mut by_kind: Vec<(u32, Vec<u32>)> = Vec::new();
    for w in waters.iter().filter(|w| !w.tris.is_empty()) {
        let key = (w.kind >> 1).min(10);
        let Some(l) = liquid(w.kind) else { continue };
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(w.verts.iter().map(|v| Vertex { pos: [v[0], v[1], -v[2]], uv: [v[0] / l.scale, v[2] / l.scale], ..Default::default() }));
        let idx = by_kind.iter().position(|(k, _)| *k == key).unwrap_or_else(|| {
            by_kind.push((key, Vec::new()));
            by_kind.len() - 1
        });
        by_kind[idx].1.extend(w.tris.iter().flatten().map(|&i| base + i as u32));
        if key == 1 {
            // the crust layer has its own uv scale: a second copy of the vertices
            let cbase = mesh.vertices.len() as u32;
            mesh.vertices.extend(w.verts.iter().map(|v| Vertex { pos: [v[0], v[1], -v[2]], uv: [v[0] / CRUST_SCALE, v[2] / CRUST_SCALE], ..Default::default() }));
            crust.extend(w.tris.iter().flatten().map(|&i| cbase + i as u32));
        }
    }
    if mesh.vertices.is_empty() {
        return None;
    }
    for (key, indices) in by_kind {
        let l = liquid(key << 1).expect("kind drawn above");
        let mut s = Submesh::new(indices, tex(l.texture));
        s.two_sided = true;
        s.liquid = true; // `VisualLiquid_t` render list 4
        s.uv_scroll = l.scroll;
        let c = argb_to_linear(l.argb);
        match key {
            // lava: opaque, unlit red water5 + the crust; `prelit` = unlit in the renderer (vertex colour is white)
            1 => {
                s.prelit = true;
                s.base_color = [c[0], c[1], c[2], 1.0];
                mesh.submeshes.push(s);
                let mut crust = Submesh::new(std::mem::take(&mut crust), tex(CRUST_TEX));
                crust.blend = Blend::AlphaBlend;
                crust.two_sided = true;
                crust.liquid = true;
                crust.prelit = true;
                crust.base_color = [1.0, 1.0, 1.0, CRUST_ALPHA];
                mesh.submeshes.push(crust);
                continue;
            }
            // star water: the client projects a star field and mist layers with `ONE, INVSRCCOLOR` [not decoded in detail]
            7 => s.blend = Blend::Additive,
            _ => {
                s.blend = Blend::AlphaBlend;
                s.base_color = c;
            }
        }
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
        let m = build_mesh(&w, &mut |n| Some(TextureKey { rdb_type: TEXTURES, id: n.len() as u32 })).unwrap();
        assert_eq!(m.vertices.len(), 6); // 3 lava vertices + 3 crust copies
        assert_eq!(m.vertices[2].pos, [0.0, 5.0, -10.0]); // z negated
        assert_eq!(m.vertices[2].uv, [0.0, 10.0 / 8.0]); // z metres / lava scale
        assert_eq!(m.vertices[5].uv, [0.0, 10.0 / 75.0]); // crust scale
        assert_eq!(m.submeshes.len(), 2);
        assert_eq!(m.submeshes[0].blend, Blend::Opaque);
        assert_eq!(m.submeshes[0].base_color, [1.0, 0.0, 0.0, 1.0]); // lava = red
        assert_eq!(m.submeshes[1].blend, Blend::AlphaBlend);
        assert!(m.submeshes.iter().all(|s| s.liquid), "every liquid submesh is in render list 4");
        assert_eq!(m.submeshes[1].indices, vec![3, 4, 5]);
    }

    #[test]
    fn kinds_follow_create_switch() {
        assert_eq!(argb_to_linear(0xcc00_186a)[3], 0.8); // water: alpha 0xcc
        assert!(liquid(16).is_none() && liquid(17).is_none()); // not drawn
        assert_eq!(liquid(1).unwrap().argb, liquid(0).unwrap().argb); // sloped twin
        assert_eq!(liquid(338).unwrap().argb, liquid(0).unwrap().argb); // switch default
        assert_eq!(liquid(7).unwrap().texture, "toxicwater.png");
    }

    #[test]
    fn rejects_bad_triangle_index() {
        let mut d = fixture();
        let n = d.len();
        d[n - 2] = 9; // last index 9 >= 3 vertices
        assert!(parse(&mut Rd::new(&d, 0)).is_err());
    }
}
