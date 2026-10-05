//! Renderer-neutral draw list.
//!
//! Coordinates are window pixels (the client draws its GUI 1:1, there is no GUI scale: the only
//! resolution-dependent value in `FontSystem_t` is the CC17 font size).  Pixel `(x, y)` covers
//! `[x, x+1) × [y, y+1)`.

use crate::gfx::GfxId;

#[derive(Clone, Copy, Debug)]
pub enum DrawCmd {
    /// Skin image (`src` = x, y, w, h in image pixels) stretched to `dst` (x0, y0, x1, y1 exclusive),
    /// modulated by `tint` (RGB) and `alpha`.
    Gfx { id: GfxId, src: [f32; 4], dst: [f32; 4], tint: [u8; 3], alpha: f32 },
    /// Glyph coverage from the glyph atlas (`src` = x, y, w, h in atlas pixels), drawn 1:1 at `dst`.
    Glyph { src: [u16; 4], dst: [i32; 2], tint: [u8; 3], alpha: f32 },
    /// Solid rectangle (x0, y0, x1, y1 exclusive).
    Solid { dst: [f32; 4], color: [u8; 3], alpha: f32 },
    /// Scissor rectangle (x0, y0, x1, y1 exclusive); `None` clears it.
    Clip(Option<[i32; 4]>),
}

#[derive(Default, Clone, Debug)]
pub struct DrawList {
    pub cmds: Vec<DrawCmd>,
}

/// Single-page 8-bit coverage atlas for rasterised glyphs (grown on demand by the engine).
pub struct GlyphAtlas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Increments whenever `data` changed.
    pub version: u64,
    x: u32,
    y: u32,
    row_h: u32,
}

impl GlyphAtlas {
    pub fn new() -> Self {
        let (w, h) = (1024, 1024);
        Self { width: w, height: h, data: vec![0; (w * h) as usize], version: 1, x: 1, y: 1, row_h: 0 }
    }
    /// Stores a `w × h` coverage bitmap and returns its origin; `None` when the page is full.
    pub fn alloc(&mut self, w: u32, h: u32, bits: &[u8]) -> Option<(u32, u32)> {
        if w + 2 > self.width || h + 2 > self.height {
            return None;
        }
        if self.x + w + 1 > self.width {
            self.x = 1;
            self.y += self.row_h + 1;
            self.row_h = 0;
        }
        if self.y + h + 1 > self.height {
            return None;
        }
        for r in 0..h {
            let d = ((self.y + r) * self.width + self.x) as usize;
            self.data[d..d + w as usize].copy_from_slice(&bits[(r * w) as usize..((r + 1) * w) as usize]);
        }
        let o = (self.x, self.y);
        self.x += w + 1;
        self.row_h = self.row_h.max(h);
        self.version += 1;
        Some(o)
    }
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new()
    }
}
