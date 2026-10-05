//! `Graphics.uvgi` / `Graphics.uvga` skin archive and the `GFX_*` id table.
//!
//! RE evidence
//! * Interfaces.dll `GuiResourceManager_t::ParseFile` 0x1000b86d: `.uvgi` is text, first line the
//!   entry count, then `"%s %i %i\n"` = name, offset, length; `.uvga` (same name, extension swapped)
//!   is the concatenation of PNG files.  Each entry is registered under
//!   `DynamicID_t::GetID(name)`.
//! * AFCM.dll `DynamicID_t::DynamicID_t` 0x10006200 fills the id lists from the static table
//!   `DynamicID_t::m_azDynamicIDGlobalTabelle` (0x10016060); the id of a `GFX_*` name is its index in
//!   that table's GFX list (`DynamicID_t::GetID` 0x10005fe6, appends unknown names at the end).  The
//!   487 built-in names are `data/gfx_ids.txt`; GUI.dll hard-codes these numeric ids (e.g.
//!   `Button_c::Initialize` 0x10128994 uses 0x1e..0x26 = `GFX_GUI_BORDER01_*`).
//! * DisplaySystem.dll `SpriteInfo_t::ConvertImage` 0x1007b8e2: GUI textures are loaded as
//!   `Format_e 2` (keyed): a pixel equal to pure green `0x00FF00` becomes transparent, every other
//!   pixel is opaque.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BUILTIN_IDS: &str = include_str!("../data/gfx_ids.txt");

/// Index into the `GFX_*` id list (what GUI.dll calls a gfx id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GfxId(pub u32);

pub struct GfxImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// RGBA8; alpha is 0 for colour-keyed pixels, 255 otherwise.
    pub rgba: Vec<u8>,
}

pub struct GfxSet {
    names: Vec<String>,
    by_name: HashMap<String, GfxId>,
    images: HashMap<GfxId, GfxImage>,
}

pub const COLOR_KEY: [u8; 3] = [0, 255, 0];

impl GfxSet {
    /// Loads `<gui_dir>/Graphics.uvgi` + `.uvga` (`gui_dir` = `cd_image/gui/Default`).
    pub fn load(gui_dir: &Path) -> Result<Self> {
        let idx_path = gui_dir.join("Graphics.uvgi");
        let idx = std::fs::read_to_string(&idx_path).with_context(|| format!("read {}", idx_path.display()))?;
        let blob_path: PathBuf = gui_dir.join("Graphics.uvga");
        let blob = std::fs::read(&blob_path).with_context(|| format!("read {}", blob_path.display()))?;
        let mut names: Vec<String> = BUILTIN_IDS.lines().map(str::to_string).collect();
        let mut by_name: HashMap<String, GfxId> = names.iter().enumerate().map(|(i, n)| (n.clone(), GfxId(i as u32))).collect();
        let mut lines = idx.lines();
        let count: usize = lines.next().ok_or_else(|| anyhow!("empty uvgi"))?.trim().parse()?;
        let mut images = HashMap::new();
        for line in lines.take(count) {
            let mut it = line.split_whitespace();
            let (Some(name), Some(off), Some(len)) = (it.next(), it.next(), it.next()) else { bail!("bad uvgi line {line:?}") };
            let (off, len): (usize, usize) = (off.parse()?, len.parse()?);
            let id = *by_name.entry(name.to_string()).or_insert_with(|| {
                names.push(name.to_string());
                GfxId(names.len() as u32 - 1)
            });
            let data = blob.get(off..off + len).ok_or_else(|| anyhow!("{name}: range outside uvga"))?;
            images.insert(id, decode_png(name, data)?);
        }
        Ok(Self { names, by_name, images })
    }

    pub fn id(&self, name: &str) -> Option<GfxId> {
        self.by_name.get(name).copied()
    }
    pub fn name(&self, id: GfxId) -> Option<&str> {
        self.names.get(id.0 as usize).map(String::as_str)
    }
    pub fn image(&self, id: GfxId) -> Option<&GfxImage> {
        self.images.get(&id)
    }
    pub fn len(&self) -> usize {
        self.images.len()
    }
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (GfxId, &GfxImage)> {
        self.images.iter().map(|(k, v)| (*k, v))
    }
    /// Pixel size of an image as `(w, h)`; `(0, 0)` when absent.
    pub fn size(&self, id: GfxId) -> (u32, u32) {
        self.images.get(&id).map_or((0, 0), |i| (i.width, i.height))
    }
}

fn decode_png(name: &str, data: &[u8]) -> Result<GfxImage> {
    let mut dec = png::Decoder::new(data);
    dec.set_transformations(png::Transformations::EXPAND);
    let mut rd = dec.read_info().with_context(|| format!("{name}: png header"))?;
    let mut buf = vec![0; rd.output_buffer_size()];
    let info = rd.next_frame(&mut buf).with_context(|| format!("{name}: png data"))?;
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    match info.color_type {
        png::ColorType::Rgb => {
            for p in buf[..n * 3].chunks_exact(3) {
                let key = p == COLOR_KEY;
                rgba.extend_from_slice(&[p[0], p[1], p[2], if key { 0 } else { 255 }]);
            }
        }
        png::ColorType::Rgba => {
            for p in buf[..n * 4].chunks_exact(4) {
                let key = p[..3] == COLOR_KEY;
                rgba.extend_from_slice(&[p[0], p[1], p[2], if key { 0 } else { p[3] }]);
            }
        }
        t => bail!("{name}: unsupported png colour type {t:?}"),
    }
    Ok(GfxImage { name: name.to_string(), width: w, height: h, rgba })
}

/// Position of an image inside an atlas page.
#[derive(Clone, Copy, Debug)]
pub struct AtlasEntry {
    pub page: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub struct AtlasPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Shelf-packed atlas of every skin image (1px transparent gutter so bilinear sampling never bleeds).
pub struct Atlas {
    pub pages: Vec<AtlasPage>,
    entries: HashMap<GfxId, AtlasEntry>,
}

impl Atlas {
    pub fn pack(set: &GfxSet, page_size: u32) -> Result<Atlas> {
        let mut items: Vec<(GfxId, &GfxImage)> = set.iter().collect();
        items.sort_by(|a, b| b.1.height.cmp(&a.1.height).then(a.0.cmp(&b.0)));
        let mut pages: Vec<AtlasPage> = Vec::new();
        let mut entries = HashMap::new();
        // current shelf cursor per page
        let (mut x, mut y, mut row_h) = (0u32, 0u32, 0u32);
        for (id, img) in items {
            let (w, h) = (img.width, img.height);
            if w + 1 > page_size || h + 1 > page_size {
                bail!("{}: {}x{} larger than atlas page {}", img.name, w, h, page_size);
            }
            if pages.is_empty() {
                pages.push(AtlasPage { width: page_size, height: page_size, rgba: vec![0; (page_size * page_size * 4) as usize] });
            }
            if x + w + 1 > page_size {
                x = 0;
                y += row_h + 1;
                row_h = 0;
            }
            if y + h + 1 > page_size {
                pages.push(AtlasPage { width: page_size, height: page_size, rgba: vec![0; (page_size * page_size * 4) as usize] });
                (x, y, row_h) = (0, 0, 0);
            }
            let pg = pages.last_mut().unwrap();
            for r in 0..h {
                let s = (r * w * 4) as usize;
                let d = (((y + r) * page_size + x) * 4) as usize;
                pg.rgba[d..d + (w * 4) as usize].copy_from_slice(&img.rgba[s..s + (w * 4) as usize]);
            }
            entries.insert(id, AtlasEntry { page: pages.len() as u32 - 1, x, y, w, h });
            x += w + 1;
            row_h = row_h.max(h);
        }
        Ok(Atlas { pages, entries })
    }
    pub fn entry(&self, id: GfxId) -> Option<AtlasEntry> {
        self.entries.get(&id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_table_matches_client_constants() {
        let names: Vec<&str> = BUILTIN_IDS.lines().collect();
        assert_eq!(names.len(), 487);
        // Button_c::Initialize (GUI.dll 0x10128994): bg 0x1e, tl 0x24, tr 0x26.
        assert_eq!(names[0x1e], "GFX_GUI_BORDER01_BACKGROUND");
        assert_eq!(names[0x24], "GFX_GUI_BORDER01_TL");
        assert_eq!(names[0x26], "GFX_GUI_BORDER01_TR");
        // FontSystem_t ctor (GUI.dll 0x1012ee05): texture ids 2/3 = GAMESHELL12/TOOLTIP9.
        assert_eq!(names[2], "GFX_FONT_GAMESHELL12");
        assert_eq!(names[3], "GFX_FONT_TOOLTIP9");
    }

    #[test]
    fn real_skin_loads_790_images() {
        let dir = crate::client_dir().join("cd_image/gui/Default");
        if !dir.join("Graphics.uvgi").exists() {
            return;
        }
        let set = GfxSet::load(&dir).unwrap();
        assert_eq!(set.len(), 790);
        let tl = set.image(set.id("GFX_GUI_TAB_BORDER_TL").unwrap()).unwrap();
        assert!(tl.width > 0 && tl.height > 0);
        let atlas = Atlas::pack(&set, 2048).unwrap();
        assert!(atlas.entry(set.id("GFX_GUI_TAB_BORDER_TL").unwrap()).is_some());
    }
}
