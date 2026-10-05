//! Planet map data (`cd_image/textures/PlanetMap`), the content of the client's Planet Map window
//! (`PlanetMapView_c`, GUI.dll; index parser `FUN_1004cb0c` @0x1004cb0c, coordinates parser
//! `FUN_1004c008` @0x1004c008, see `docs/formats.md` *Planet map*).
//!
//! * Index text (`PlanetMapIndexFile` option, default `Normal/PlanetMapIndexNormal.txt`,
//!   `ShadowlandMapIndexFile` default `Shadowlands/ShadowlandsMap.txt`): `Name "x"`, `Type Rubika|Shadowlands`,
//!   `CoordsFile <rel>`, then one block per zoom level starting at `File <rel .bin>`, `TextureSize n`,
//!   `Size w h` (canvas), `Tiles tx ty`, `MapRect x y w h` (used part of the canvas) and `tx*ty` `FilePos n`
//!   lines (row-major tile offsets into the `.bin`).
//! * `.bin`: concatenated PNG/JPEG tiles of `TextureSize` square pixels; tile `i` ends where tile `i+1` starts.
//! * Coordinates XML: `<Playfield id name x xscale z zscale/>` placing each playfield on the map.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use ao_scene::Texture;

/// One zoom level of a planet map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Level {
    pub file: String,
    pub texture_size: u32,
    /// Canvas size in pixels.
    pub size: [u32; 2],
    /// Tile grid.
    pub tiles: [u32; 2],
    /// `l t r b` of the used part of the canvas (`MapRect`).
    pub rect: [u32; 4],
    pub pos: Vec<u64>,
}

/// World extent the planet map covers (`FUN_1004b81a` @0x1004b81a, doubles at 0x101b1258 / 0x101b1250 / 0x101b1248 / 0x101b1240).
const WORLD_X0: f32 = 31022.0;
const WORLD_Z0: f32 = 24880.0;
const WORLD_W: f32 = 18751.0;
const WORLD_H: f32 = 24115.0;

impl Level {
    /// Canvas pixel of the position `(x, z)` inside playfield `c` (`FUN_1004b81a`): global = `c.x + x`, `c.z + z`;
    /// `px = (global_x - 31022) * (r - l) / 18751 * xscale + l`, `py = (b - t) - (t + (global_z - 24880) * (b - t) / 24115 * zscale)`.
    pub fn locate(&self, c: &Coord, x: f32, z: f32) -> [f32; 2] {
        let [l, t, r, b] = self.rect.map(|v| v as f32);
        let px = (c.x + x - WORLD_X0) * ((r - l) / WORLD_W) * c.xscale + l;
        let sz = t + (c.z + z - WORLD_Z0) * ((b - t) / WORLD_H) * c.zscale;
        [px, (b - t) - sz]
    }
}

/// Where a playfield sits on the planet map (`coords.xml` entry).
#[derive(Debug, Clone, PartialEq)]
pub struct Coord {
    pub name: String,
    pub x: f32,
    pub xscale: f32,
    pub z: f32,
    pub zscale: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Index {
    pub name: String,
    pub kind: String,
    pub coords_file: String,
    pub levels: Vec<Level>,
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// Parses an index text file; unknown keys are an error like the client's "Error in index file around: ".
pub fn parse_index(text: &str) -> Result<Index> {
    let mut idx = Index::default();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, val)) = line.split_once(char::is_whitespace) else { continue };
        let nums = || val.split_whitespace().map(|v| v.parse::<u64>().map_err(|e| anyhow!("{key}: {e}"))).collect::<Result<Vec<_>>>();
        match key {
            "Name" => idx.name = unquote(val).to_string(),
            "Type" => idx.kind = val.trim().to_string(),
            "CoordsFile" => idx.coords_file = val.trim().to_string(),
            "File" => idx.levels.push(Level { file: val.trim().replace('\\', "/"), ..Level::default() }),
            _ => {
                let Some(l) = idx.levels.last_mut() else { bail!("index line before File: {line}") };
                let n = nums()?;
                match (key, n.as_slice()) {
                    ("TextureSize", [a]) => l.texture_size = *a as u32,
                    ("Size", [a, b]) => l.size = [*a as u32, *b as u32],
                    ("Tiles", [a, b]) => l.tiles = [*a as u32, *b as u32],
                    ("MapRect", [a, b, c, d]) => l.rect = [*a as u32, *b as u32, *c as u32, *d as u32],
                    ("FilePos", [a]) => l.pos.push(*a),
                    _ => bail!("Error in index file around: {line}"),
                }
            }
        }
    }
    for l in &idx.levels {
        if l.pos.len() != (l.tiles[0] * l.tiles[1]) as usize {
            bail!("{}: {} FilePos for {}x{} tiles", l.file, l.pos.len(), l.tiles[0], l.tiles[1]);
        }
    }
    if idx.levels.is_empty() {
        bail!("planet map index without levels");
    }
    Ok(idx)
}

fn attr<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}=\"");
    let s = tag.find(&pat)? + pat.len();
    Some(&tag[s..s + tag[s..].find('"')?])
}

/// Parses the `<Playfield .../>` entries.
pub fn parse_coords(xml: &str) -> HashMap<u32, Coord> {
    let mut out = HashMap::new();
    for tag in xml.split('<').filter(|t| t.starts_with("Playfield")) {
        let f = |k: &str| attr(tag, k).and_then(|v| v.parse::<f32>().ok());
        let (Some(id), Some(x), Some(z)) = (attr(tag, "id").and_then(|v| v.parse().ok()), f("x"), f("z")) else { continue };
        out.insert(id, Coord { name: attr(tag, "name").unwrap_or("").to_string(), x, xscale: f("xscale").unwrap_or(1.0), z, zscale: f("zscale").unwrap_or(1.0) });
    }
    out
}

/// A loaded planet map: index, coordinates and the tile bytes of every level file.
pub struct PlanetMap {
    pub index: Index,
    pub coords: HashMap<u32, Coord>,
    bins: HashMap<String, Vec<u8>>,
}

impl PlanetMap {
    /// `index_rel` is relative to `cd_image/textures/PlanetMap` (e.g. `Normal/PlanetMapIndexNormal.txt`, matched case-insensitively on the path as on Windows).
    pub fn load(client_dir: &Path, index_rel: &str) -> Result<Self> {
        let root = client_dir.join("cd_image/textures/PlanetMap");
        let read = |rel: &str| -> Result<Vec<u8>> {
            let p = resolve(&root, rel).with_context(|| format!("planet map file {rel}"))?;
            std::fs::read(&p).with_context(|| p.display().to_string())
        };
        let index = parse_index(&String::from_utf8_lossy(&read(index_rel)?))?;
        let coords = parse_coords(&String::from_utf8_lossy(&read(&index.coords_file)?));
        let mut bins = HashMap::new();
        for l in &index.levels {
            if !bins.contains_key(&l.file) {
                bins.insert(l.file.clone(), read(&l.file)?);
            }
        }
        Ok(Self { index, coords, bins })
    }

    /// Decodes tile `(tx, ty)` of `level`; the tile spans from its `FilePos` to the next one (or the file end).
    pub fn tile(&self, level: usize, tx: u32, ty: u32) -> Result<Texture> {
        let l = self.index.levels.get(level).ok_or_else(|| anyhow!("no level {level}"))?;
        if tx >= l.tiles[0] || ty >= l.tiles[1] {
            bail!("tile {tx},{ty} outside {}x{}", l.tiles[0], l.tiles[1]);
        }
        let bin = &self.bins[&l.file];
        let i = (tx * l.tiles[1] + ty) as usize;
        let start = l.pos[i] as usize;
        let end = l.pos.get(i + 1).map_or(bin.len(), |&e| e as usize).min(bin.len());
        crate::texture::decode_texture(bin.get(start..end).ok_or_else(|| anyhow!("tile offset past file"))?)
    }

    /// The whole canvas of `level` as RGBA (missing tiles stay transparent).
    pub fn canvas(&self, level: usize) -> Result<Texture> {
        let l = self.index.levels.get(level).ok_or_else(|| anyhow!("no level {level}"))?;
        let (w, h) = (l.tiles[0] * l.texture_size, l.tiles[1] * l.texture_size);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for ty in 0..l.tiles[1] {
            for tx in 0..l.tiles[0] {
                let t = self.tile(level, tx, ty)?;
                for y in 0..t.height.min(l.texture_size) {
                    let (dy, dx) = (ty * l.texture_size + y, tx * l.texture_size);
                    let n = t.width.min(l.texture_size) as usize * 4;
                    let d = ((dy * w + dx) * 4) as usize;
                    let s = (y * t.width * 4) as usize;
                    rgba[d..d + n].copy_from_slice(&t.rgba[s..s + n]);
                }
            }
        }
        Ok(Texture { width: w, height: h, rgba })
    }
}

/// Case-insensitive path resolution below `root` (the index files mix `Normal/` and `normal/`, `\` and `/`).
fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let mut p = root.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        let hit = std::fs::read_dir(&p)?.filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part));
        p = hit.ok_or_else(|| anyhow!("{} not found in {}", part, p.display()))?.path();
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_coords() {
        let idx = parse_index("Name \"Default Map\"\nType Rubika\nCoordsFile normal/coords.xml\n\nFile a\\b.bin\nTextureSize 128\nSize 256 128\nTiles 2 1\nMapRect 1 2 3 4\nFilePos 0\nFilePos 9\n").unwrap();
        assert_eq!(idx.name, "Default Map");
        assert_eq!(idx.levels[0].file, "a/b.bin");
        assert_eq!(idx.levels[0].rect, [1, 2, 3, 4]);
        let l = Level { rect: [56, 0, 945, 1145], ..Level::default() };
        let c = Coord { name: String::new(), x: 42862.0, xscale: 1.0, z: 42389.0, zscale: 1.0 };
        let p = l.locate(&c, 0.0, 0.0);
        assert!((p[0] - 617.3).abs() < 0.1 && (p[1] - 313.6).abs() < 0.1, "{p:?}");
        assert_eq!(idx.levels[0].pos, vec![0, 9]);
        assert!(parse_index("File x\nTiles 2 1\nFilePos 0\n").is_err());
        assert!(parse_index("File x\nBogus 1\n").is_err());
        let c = parse_coords("<root><Playfield id=\"566\" name=\"Newland City\" x=\"42862\" xscale=\"1.00000\" z=\"42389\" zscale=\"0.5\"/></root>");
        assert_eq!(c[&566], Coord { name: "Newland City".into(), x: 42862.0, xscale: 1.0, z: 42389.0, zscale: 0.5 });
    }

    fn client() -> Option<PathBuf> {
        let d = dirs_home()?.join("Games/ProjectRubiKa/client");
        d.join("cd_image/textures/PlanetMap").is_dir().then_some(d)
    }

    fn dirs_home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    #[test]
    fn real_default_map() {
        let Some(dir) = client() else { return };
        let m = PlanetMap::load(&dir, "Normal/PlanetMapIndexNormal.txt").unwrap();
        assert_eq!(m.index.kind, "Rubika");
        assert_eq!(m.index.levels.len(), 2);
        assert_eq!(m.index.levels[0].tiles, [10, 11]);
        assert_eq!(m.index.levels[1].tiles, [31, 37]);
        assert_eq!(m.coords[&566].x, 42862.0);
        let t = m.tile(0, 3, 4).unwrap();
        assert_eq!((t.width, t.height), (128, 128));
        let sl = PlanetMap::load(&dir, "Shadowlands/ShadowlandsMap.txt").unwrap();
        assert_eq!(sl.index.kind, "Shadowlands");
        assert_eq!(sl.index.levels.len(), 2);
        assert!(sl.tile(0, 0, 0).is_ok());
    }
}
