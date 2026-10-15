//! Client fonts.
//!
//! RE evidence (GUI.dll `FontSystem_t::FontSystem_t` 0x1012ee05, table `0x10272df0`, 12 entries of
//! 5 dwords + the chat font):
//!
//! | `FontID_e` | XML name   | source |
//! |-----------:|------------|--------|
//! | 1 | TOOLTIP  | bitmap font `textures/fonts/FontTooltip9.fnt` + skin image `GFX_FONT_TOOLTIP9` |
//! | 2 | SHELL    | bitmap `FontGameShell12.fnt` + `GFX_FONT_GAMESHELL12` |
//! | 3 | CLOCK    | same table row as SHELL |
//! | 4 | CC17     | Verdana bold, `lfHeight = trunc(20 * f32(DisplayHeight/768))` (1012efab..1012efdb) |
//! | 5 | NORMAL   | GDI Verdana, `lfHeight` 13, regular |
//! | 6 | BOLD     | Verdana 13 bold |
//! | 7 | SMALL    | Verdana 12 |
//! | 8 | LARGE    | Verdana 16 |
//! | 9 | HUGE     | Verdana 24 bold |
//! | 10 | TT_MIN12 | Verdana 14 |
//! | 11 | ITALIC   | Verdana 16 italic |
//! | 12 | CHAT     | `ChatFontName`/`ChatFontStyle`/`ChatFontSize/10` from `MainPrefs.xml` (Verdana, Regular, 14) |
//!
//! * Bitmap fonts (`.fnt`): `u32 256, u32 256`, then 255 records `i32[4] = (x0, y0, x1, y1)` indexed by
//!   character code (glyph = that rectangle of the skin image, green = transparent), then 255 `u32`
//!   widths; the loader adds 1 to every width, so the advance is `x1 − x0 + 1`; the line height is
//!   `rect[97].y1 − rect[97].y0` (0x1012ee05: `+0xff0 = [0x61c] − [0x614]`).
//! * TrueType fonts are rendered by GDI: `CreateFontA(lfHeight = size, weight 400/700, charset 1,
//!   OUT_TT_ONLY_PRECIS(7), quality 3, "Verdana")` (`FontSystem_t::CreateTTFont` 0x1012ec5c) and every glyph is
//!   drawn with `TextOutW` into a **1-bpp** bitmap (`FontInfo_t::GetGlyph` 0x1012e73c), i.e. aliased,
//!   hinted glyphs.  The advance is `GetTextExtentPoint32W().cx`, the line height `tmHeight`
//!   (`FontInfo+0xff0`).  `lfHeight > 0` selects the em size whose cell height (tmAscent+tmDescent) equals it.
//!   The port uses `skrifa` mono hinting plus a centre-sampling scan converter with dropout control;
//!   exact Windows GDI dropout behaviour remains unresolved, not byte-equivalent rendering.
//!   Verdana is taken from the host (`/System/Library/Fonts/Supplemental/Verdana*.ttf`), because the
//!   client resolves the face by name through GDI and does not ship the file.

use anyhow::{anyhow, bail, Context, Result};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, Target};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::gfx::{GfxId, GfxSet};

/// `FontID_e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontId {
    Tooltip = 1,
    Shell = 2,
    Clock = 3,
    Cc17 = 4,
    Normal = 5,
    Bold = 6,
    Small = 7,
    Large = 8,
    Huge = 9,
    TtMin12 = 10,
    Italic = 11,
    Chat = 12,
}

impl FontId {
    /// `FUN_101648b6` (GUI.dll 0x101648b6): XML `font=` name → id, default NORMAL.
    pub fn from_name(s: &str) -> FontId {
        match s.to_ascii_uppercase().as_str() {
            "TOOLTIP" => FontId::Tooltip,
            "SHELL" => FontId::Shell,
            "CLOCK" => FontId::Clock,
            "CC17" => FontId::Cc17,
            "NORMAL" => FontId::Normal,
            "BOLD" => FontId::Bold,
            "SMALL" => FontId::Small,
            "LARGE" => FontId::Large,
            "HUGE" => FontId::Huge,
            "TT_MIN12" => FontId::TtMin12,
            "ITALIC" => FontId::Italic,
            "CHAT" => FontId::Chat,
            _ => FontId::Normal,
        }
    }
}

/// One rendered glyph: 1 byte per pixel (0/1) bitmap anchored at the pen position / line top.
#[derive(Clone, Debug)]
pub struct Glyph {
    /// Pen advance in pixels.
    pub advance: i32,
    pub width: i32,
    pub height: i32,
    pub bits: Vec<u8>,
}

enum Kind {
    Bitmap(BitmapFont),
    Tt(TtFont),
}

pub struct Font {
    kind: Kind,
    cache: HashMap<char, Glyph>,
    /// aomac Enhanced "Anti-aliased Text" cache (not retail, audit item 13): keyed by the backing scale a
    /// glyph was rasterized at, since a bigger window scale needs a bigger, re-rasterized atlas bitmap.
    aa_cache: HashMap<(char, u8), Glyph>,
    /// Line height in pixels (`FontInfo+0xff0`).
    pub height: i32,
    /// Advance of the space character (`FontInfo+0xff4`).
    pub space: i32,
}

impl Font {
    pub fn glyph(&mut self, ch: char) -> &Glyph {
        if !self.cache.contains_key(&ch) {
            let g = match &self.kind {
                Kind::Bitmap(b) => b.glyph(ch),
                Kind::Tt(t) => t.glyph(ch, 1, false),
            };
            self.cache.insert(ch, g);
        }
        &self.cache[&ch]
    }
    /// aomac Enhanced "Anti-aliased Text" (not retail, opt-in): when `smooth`, rasterize real coverage at
    /// `scale` physical pixels per GUI pixel, including at 1x.
    /// Bitmap fonts (tooltips, shell and clock) always stay pixel art at scale 1 regardless of the
    /// option. Returns the *effective* scale used, so the caller
    /// can size the destination box back down from the bigger atlas bitmap (physical source, logical
    /// destination) -- the retail-hinted advance (`Font::advance`) never depends on `scale`, so text layout
    /// is unchanged.
    pub fn glyph_scaled(&mut self, ch: char, scale: u8, smooth: bool) -> (&Glyph, u8) {
        if !smooth || matches!(&self.kind, Kind::Bitmap(_)) {
            return (self.glyph(ch), 1);
        }
        let scale = scale.max(1);
        if !self.aa_cache.contains_key(&(ch, scale)) {
            let Kind::Tt(t) = &self.kind else { unreachable!("bitmap fonts return above") };
            let g = t.glyph(ch, scale, true);
            self.aa_cache.insert((ch, scale), g);
        }
        (&self.aa_cache[&(ch, scale)], scale)
    }
    pub fn advance(&mut self, ch: char) -> i32 {
        self.glyph(ch).advance
    }
    pub fn text_width(&mut self, s: &str) -> i32 {
        s.chars().map(|c| self.advance(c)).sum()
    }
}

pub struct FontSystem {
    fonts: HashMap<FontId, Font>,
    dir: PathBuf,
    cc17_height: i32,
}

fn cc17_lf_height(display_height: u32) -> i32 {
    (20.0 * (display_height as f32 / 768.0)) as i32
}

impl FontSystem {
    /// `verdana_dir` holds `Verdana.ttf`, `Verdana Bold.ttf`, `Verdana Italic.ttf`.
    pub fn new(client_cd: &Path, gfx: &GfxSet, verdana_dir: Option<&Path>) -> Result<Self> {
        let dir = verdana_dir.map(Path::to_path_buf).or_else(find_verdana_dir).ok_or_else(|| anyhow!("Verdana.ttf not found"))?;
        let mut fonts = HashMap::new();
        for (id, fnt, tex) in [
            (FontId::Tooltip, "FontTooltip9.fnt", "GFX_FONT_TOOLTIP9"),
            (FontId::Shell, "FontGameShell12.fnt", "GFX_FONT_GAMESHELL12"),
            (FontId::Clock, "FontGameShell12.fnt", "GFX_FONT_GAMESHELL12"),
        ] {
            let path = client_cd.join("textures/fonts").join(fnt);
            let gid = gfx.id(tex).ok_or_else(|| anyhow!("{tex} missing"))?;
            let bf = BitmapFont::load(&path, gfx, gid)?;
            fonts.insert(id, Font { height: bf.height, space: bf.advance(' '), kind: Kind::Bitmap(bf), cache: HashMap::new(), aa_cache: HashMap::new() });
        }
        let tt = |name: &str, h: i32, bold: bool, italic: bool| -> Result<Font> {
            let f = TtFont::new(&dir, name, h, bold, italic)?;
            let mut font = Font { height: f.cell_height, space: 0, kind: Kind::Tt(f), cache: HashMap::new(), aa_cache: HashMap::new() };
            font.space = font.advance(' ');
            Ok(font)
        };
        // 0x10272df0 table rows (name, lfHeight, bold, italic)
        for (id, h, b, i) in [
            (FontId::Cc17, 20, true, false),
            (FontId::Normal, 13, false, false),
            (FontId::Bold, 13, true, false),
            (FontId::Small, 12, false, false),
            (FontId::Large, 16, false, false),
            (FontId::Huge, 24, true, false),
            (FontId::TtMin12, 14, false, false),
            (FontId::Italic, 16, false, true),
            (FontId::Chat, 14, false, false),
        ] {
            fonts.insert(id, tt("Verdana", h, b, i)?);
        }
        Ok(Self { fonts, dir, cc17_height: 20 })
    }
    pub fn font(&mut self, id: FontId) -> &mut Font {
        self.fonts.get_mut(&id).expect("all FontIds are loaded")
    }

    /// FontSystem ctor 1012efab..1012efdb: scale the table's 20px cell by display height.
    pub fn set_display_height(&mut self, height: u32) -> Result<bool> {
        let lf = cc17_lf_height(height);
        if lf == self.cc17_height {
            return Ok(false);
        }
        let f = TtFont::new(&self.dir, "Verdana", lf, true, false)?;
        let mut font = Font { height: f.cell_height, space: 0, kind: Kind::Tt(f), cache: HashMap::new(), aa_cache: HashMap::new() };
        font.space = font.advance(' ');
        self.fonts.insert(FontId::Cc17, font);
        self.cc17_height = lf;
        Ok(true)
    }

    /// Re-creates the CHAT font from the `ChatFontName` / `ChatFontStyle` / `ChatFontSize` prefs (`size` in tenths of a point = `lfHeight * 10`, so
    /// 140 = the default 14 px). The face is looked up in the host font dir like Verdana; an unknown face or style leaves the font unchanged
    /// (returns false). Returns whether the metrics changed.
    pub fn set_chat(&mut self, family: &str, style: &str, size: i32) -> bool {
        let (bold, italic) = match style.to_ascii_lowercase().as_str() {
            "bold" => (true, false),
            "italic" => (false, true),
            "bold italic" | "bolditalic" => (true, true),
            _ => (false, false),
        };
        let lf = (size / 10).clamp(5, 48);
        let Ok(f) = TtFont::new(&self.dir, family, lf, bold, italic) else { return false };
        let mut font = Font { height: f.cell_height, space: 0, kind: Kind::Tt(f), cache: HashMap::new(), aa_cache: HashMap::new() };
        font.space = font.advance(' ');
        self.fonts.insert(FontId::Chat, font);
        true
    }
}

fn find_verdana_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut c = vec![PathBuf::from("/System/Library/Fonts/Supplemental"), PathBuf::from("/Library/Fonts")];
    if let Some(h) = home {
        c.push(h.join("Library/Fonts"));
    }
    c.into_iter().find(|d| d.join("Verdana.ttf").exists())
}

// ---------------------------------------------------------------- bitmap fonts

pub struct BitmapFont {
    rects: Vec<[i32; 4]>,
    widths: Vec<i32>,
    pub height: i32,
    /// Glyph sheet (alpha 0 where colour-keyed): intensity is the red channel.
    sheet_w: u32,
    sheet: Vec<u8>,
}

impl BitmapFont {
    pub fn load(path: &Path, gfx: &GfxSet, tex: GfxId) -> Result<Self> {
        let d = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        if d.len() < 8 + 0xff0 + 0x3fc {
            bail!("{}: short .fnt", path.display());
        }
        let rd = |o: usize| i32::from_le_bytes(d[o..o + 4].try_into().unwrap());
        let rects: Vec<[i32; 4]> = (0..255).map(|i| [rd(8 + i * 16), rd(12 + i * 16), rd(16 + i * 16), rd(20 + i * 16)]).collect();
        let widths: Vec<i32> = (0..255).map(|i| rd(8 + 0xff0 + i * 4) + 1).collect();
        let height = rects[97][3] - rects[97][1];
        let img = gfx.image(tex).ok_or_else(|| anyhow!("font sheet missing"))?;
        let sheet = img.rgba.clone();
        Ok(Self { rects, widths, height, sheet_w: img.width, sheet })
    }
    pub fn advance(&self, ch: char) -> i32 {
        let c = ch as u32;
        if c < 255 {
            self.widths[c as usize]
        } else {
            -1
        }
    }
    fn glyph(&self, ch: char) -> Glyph {
        let c = ch as usize;
        if c >= 255 {
            return Glyph { advance: 0, width: 0, height: 0, bits: vec![] };
        }
        let [x0, y0, x1, y1] = self.rects[c];
        let (w, h) = ((x1 - x0).max(0), (y1 - y0).max(0));
        let mut bits = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = (((y0 + y) as u32 * self.sheet_w + (x0 + x) as u32) * 4) as usize;
                // keyed pixels transparent; any other intensity is drawn (antialias greys are tinted)
                bits[(y * w + x) as usize] = if self.sheet[p + 3] == 0 { 0 } else { self.sheet[p] };
            }
        }
        Glyph { advance: self.widths[c], width: w, height: h, bits }
    }
}

// ---------------------------------------------------------------- TrueType (GDI emulation)

pub struct TtFont {
    data: Vec<u8>,
    ppem: f32,
    hint: HintingInstance,
    pub ascent: i32,
    pub descent: i32,
    pub cell_height: i32,
    hdmx: Option<(u16, Vec<u8>)>,
}

impl TtFont {
    pub fn new(dir: &Path, family: &str, lf_height: i32, bold: bool, italic: bool) -> Result<Self> {
        let file = match (bold, italic) {
            (false, false) => format!("{family}.ttf"),
            (true, false) => format!("{family} Bold.ttf"),
            (false, true) => format!("{family} Italic.ttf"),
            (true, true) => format!("{family} Bold Italic.ttf"),
        };
        let path = dir.join(&file);
        let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let font = FontRef::new(&data).map_err(|e| anyhow!("{file}: {e:?}"))?;
        let os2 = font.os2().map_err(|e| anyhow!("{file}: os2 {e:?}"))?;
        let upem = font.head().map_err(|e| anyhow!("{file}: head {e:?}"))?.units_per_em() as f32;
        let (wa, wd) = (os2.us_win_ascent() as f32, os2.us_win_descent() as f32);
        // lfHeight > 0: largest em whose rounded cell (ascent + descent) does not exceed lf_height.
        let cell = |p: i32| ((wa * p as f32 / upem).round() as i32, (wd * p as f32 / upem).round() as i32);
        let mut ppem = 1;
        for p in 1..200 {
            let (a, d) = cell(p);
            if a + d <= lf_height {
                ppem = p;
            }
        }
        let (ascent, descent) = cell(ppem);
        let outlines = font.outline_glyphs();
        let hint = HintingInstance::new(&outlines, Size::new(ppem as f32), LocationRef::default(), HintingOptions { engine: Engine::Interpreter, target: Target::Mono })
            .map_err(|e| anyhow!("{file}: hinting {e:?}"))?;
        let hdmx = font.hdmx().ok().and_then(|h| {
            h.records().iter().filter_map(|r| r.ok()).find(|r| r.pixel_size() as i32 == ppem).map(|r| (r.pixel_size() as u16, r.widths().to_vec()))
        });
        Ok(Self { data, ppem: ppem as f32, hint, ascent, descent, cell_height: ascent + descent, hdmx })
    }

    /// Retail-style 1-bit hinted bitmap; exact Windows GDI dropout equivalence is unresolved.
    /// Enhanced AA uses real coverage while retaining the port's logical advances and cell dimensions.
    /// Callers divide the physical bitmap dimensions by `scale`, so layout is unaffected.
    fn glyph(&self, ch: char, scale: u8, smooth: bool) -> Glyph {
        let font = FontRef::new(&self.data).expect("validated in new");
        let ch = if ch == '\t' { ' ' } else { ch };
        let gid = font.charmap().map(ch).unwrap_or_default();
        // advance: hdmx device width if the font has one for this size (what GDI reports), else rounded linear width
        let adv = match &self.hdmx {
            Some((_, w)) => w.get(gid.to_u32() as usize).map(|v| *v as i32),
            None => None,
        }
        .unwrap_or_else(|| {
            font.glyph_metrics(Size::new(self.ppem), LocationRef::default()).advance_width(gid).unwrap_or(0.0).round() as i32
        });
        let outlines = font.outline_glyphs();
        if !smooth {
            let mut pen = Flatten::default();
            if let Some(g) = outlines.get(gid) {
                let _ = g.draw(DrawSettings::hinted(&self.hint, false), &mut pen);
            }
            let h = self.cell_height;
            let w = (adv.max(0) as usize).max(pen.max_x().ceil().max(0.0) as usize) + 1;
            let mut bits = vec![0u8; w * h as usize];
            pen.fill(w, h as usize, self.ascent as f32, &mut bits);
            return Glyph { advance: adv, width: w as i32, height: h, bits };
        }
        // Fixed box-filtered supersample factor for the AA coverage fill: fine enough that diagonal/curved
        // strokes look smooth, cheap enough to run per-glyph at load time (glyphs are cached after the first draw).
        const AA_SUPERSAMPLE: usize = 4;
        let scale_f = f32::from(scale);
        let hint = HintingInstance::new(&outlines, Size::new(self.ppem * scale_f), LocationRef::default(), HintingOptions { engine: Engine::Interpreter, target: Target::Mono }).ok();
        let mut pen = Flatten::default();
        if let (Some(g), Some(hint)) = (outlines.get(gid), &hint) {
            let _ = g.draw(DrawSettings::hinted(hint, false), &mut pen);
        }
        let h_phys = self.cell_height as usize * scale as usize;
        let ascent_phys = self.ascent as f32 * scale_f;
        let w_phys = ((adv.max(0) as f32 * scale_f) as usize).max(pen.max_x().ceil().max(0.0) as usize) + scale as usize;
        let mut bits = vec![0u8; w_phys * h_phys];
        pen.fill_aa(w_phys, h_phys, ascent_phys, AA_SUPERSAMPLE, &mut bits);
        Glyph { advance: adv, width: w_phys as i32, height: h_phys as i32, bits }
    }
}

#[derive(Default)]
struct Flatten {
    segs: Vec<[f32; 4]>,
    start: (f32, f32),
    cur: (f32, f32),
}

impl Flatten {
    fn line(&mut self, x: f32, y: f32) {
        self.segs.push([self.cur.0, self.cur.1, x, y]);
        self.cur = (x, y);
    }
    fn max_x(&self) -> f32 {
        self.segs.iter().flat_map(|s| [s[0], s[2]]).fold(0.0, f32::max)
    }
    /// Scan-converts with pixel-centre sampling, non-zero winding, and dropout control
    /// (a span that covers no pixel centre lights the pixel containing its midpoint).
    fn fill(&self, w: usize, h: usize, baseline: f32, out: &mut [u8]) {
        for row in 0..h {
            // outline y is up; bitmap row r has its centre at y = baseline - (r + 0.5)
            let y = baseline - (row as f32 + 0.5);
            let mut xs: Vec<(f32, i32)> = Vec::new();
            for s in &self.segs {
                let (x0, y0, x1, y1) = (s[0], s[1], s[2], s[3]);
                if y0 == y1 {
                    continue;
                }
                let (lo, hi, dir) = if y0 < y1 { (y0, y1, 1) } else { (y1, y0, -1) };
                if y >= lo && y < hi {
                    let t = (y - y0) / (y1 - y0);
                    xs.push((x0 + t * (x1 - x0), dir));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            let mut enter = 0.0;
            for (x, d) in xs {
                let was = wind;
                wind += d;
                if was == 0 && wind != 0 {
                    enter = x;
                } else if was != 0 && wind == 0 {
                    let a = (enter - 0.5).ceil() as i32;
                    let b = (x - 0.5).ceil() as i32; // exclusive
                    let (a, b) = if b <= a && x > enter { let m = ((enter + x) * 0.5).floor() as i32; (m, m + 1) } else { (a, b) };
                    for px in a.max(0)..b.min(w as i32) {
                        out[row * w + px as usize] = 1;
                    }
                }
            }
        }
    }

    /// Coverage antialiasing of the same non-zero-winding scanline fill as [`Flatten::fill`], used only by the
    /// aomac Enhanced "Anti-aliased Text" option (not retail, audit item 13): `ss` x `ss` box-filtered
    /// supersampling (`ss` sub-rows per pixel row, `ss` sub-columns per pixel column) producing real 0..255
    /// coverage instead of `fill`'s hard 0/1.
    fn fill_aa(&self, w: usize, h: usize, baseline: f32, ss: usize, out: &mut [u8]) {
        let ss = ss.max(1);
        let hits_per_px = (ss * ss) as u32;
        let mut counts = vec![0u32; w];
        for row in 0..h {
            counts.fill(0);
            for sy in 0..ss {
                let y = baseline - (row as f32 + (sy as f32 + 0.5) / ss as f32);
                let mut xs: Vec<(f32, i32)> = Vec::new();
                for s in &self.segs {
                    let (x0, y0, x1, y1) = (s[0], s[1], s[2], s[3]);
                    if y0 == y1 {
                        continue;
                    }
                    let (lo, hi, dir) = if y0 < y1 { (y0, y1, 1) } else { (y1, y0, -1) };
                    if y >= lo && y < hi {
                        let t = (y - y0) / (y1 - y0);
                        xs.push((x0 + t * (x1 - x0), dir));
                    }
                }
                xs.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut wind = 0;
                let mut enter = 0.0;
                for (x, d) in xs {
                    let was = wind;
                    wind += d;
                    if was == 0 && wind != 0 {
                        enter = x;
                    } else if was != 0 && wind == 0 {
                        let a = ((enter * ss as f32) - 0.5).ceil() as i64;
                        let b = ((x * ss as f32) - 0.5).ceil() as i64; // exclusive
                        let (a, b) = if b <= a && x > enter { let m = (((enter + x) * 0.5) * ss as f32).floor() as i64; (m, m + 1) } else { (a, b) };
                        for subx in a.max(0)..b.min((w * ss) as i64) {
                            counts[subx as usize / ss] += 1;
                        }
                    }
                }
            }
            for x in 0..w {
                out[row * w + x] = ((counts[x] * 255 + hits_per_px / 2) / hits_per_px) as u8;
            }
        }
    }
}

impl OutlinePen for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = (x, y);
        self.cur = (x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.line(x, y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (px, py) = self.cur;
        const N: usize = 8;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let u = 1.0 - t;
            self.line(u * u * px + 2.0 * u * t * cx + t * t * x, u * u * py + 2.0 * u * t * cy + t * t * y);
        }
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        let (px, py) = self.cur;
        const N: usize = 12;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let u = 1.0 - t;
            self.line(
                u * u * u * px + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * x,
                u * u * u * py + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * y,
            );
        }
    }
    fn close(&mut self) {
        let s = self.start;
        if self.cur != s {
            self.line(s.0, s.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cc17_requested_height_uses_native_display_ratio_and_truncation() {
        assert_eq!(cc17_lf_height(768), 20);
        assert_eq!(cc17_lf_height(800), 20);
        assert_eq!(cc17_lf_height(1536), 40);
    }

    fn dump(g: &Glyph) -> String {
        let mut s = String::new();
        for r in 0..g.height {
            for c in 0..g.width {
                s.push(if g.bits[(r * g.width + c) as usize] != 0 { '#' } else { '.' });
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn verdana_13_cell_matches_gdi() {
        let Some(dir) = find_verdana_dir() else { return };
        let f = TtFont::new(&dir, "Verdana", 13, false, false).unwrap();
        // Windows: Verdana at lfHeight 13 is the classic 8pt (11 ppem) face, tmHeight 13.
        assert_eq!(f.ppem as i32, 11);
        assert_eq!(f.cell_height, 13);
        let g = f.glyph('L', 1, false);
        println!("{}\nadv {}", dump(&g), g.advance);
        assert!(g.bits.iter().any(|b| *b != 0));
        let l = f.glyph('l', 1, false);
        println!("{}", dump(&l));
    }

    #[test]
    fn anti_aliased_text_scales_the_bitmap_not_the_advance() {
        // AA is independent of GUI scale; all modes preserve the retail layout.
        let Some(dir) = find_verdana_dir() else { return };
        let mut font = Font { kind: Kind::Tt(TtFont::new(&dir, "Verdana", 13, false, false).unwrap()), cache: HashMap::new(), aa_cache: HashMap::new(), height: 13, space: 0 };
        let retail = font.glyph_scaled('A', 1, false).0.clone();
        assert!(retail.bits.iter().all(|&b| b == 0 || b == 1), "AA off must stay the retail 1-bit bitmap");
        for scale in [1u8, 2, 3] {
            let (aa, effective) = font.glyph_scaled('A', scale, true);
            assert_eq!(effective, scale);
            assert_eq!(aa.advance, retail.advance, "scale must never change the retail-hinted advance (layout)");
            assert_eq!(aa.height, retail.height * i32::from(scale), "the AA bitmap must be `scale`x the logical cell height");
            assert!(aa.width >= retail.width * i32::from(scale) - i32::from(scale), "the AA bitmap must be about `scale`x the logical width");
            assert!(aa.bits.iter().any(|&b| b > 1 && b < 255), "a real coverage mask must have in-between (antialiased) values, not just 0/255");
        }
    }
}
