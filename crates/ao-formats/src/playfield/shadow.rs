//! Baked ground shadow maps: rdb 1000007 (keyed by playfield id; `data/Shadows/<id>.sdw` are older copies).
//!
//! ```text
//! u32 end[6]                 end offsets of the six blobs (the last one is the record length)
//! blob 0 at 24, blob i at end[i-1]: 8 bit grey PNG, all six the same size
//! ```
//! The six greyscale maps are the ground lighting for different sun positions (`DayTimeForGroundShadows`,
//! `UseGroundShadows` in DisplaySystem.dll; layers 0..4 are dawn..dusk, layer 5 is a dark map with a few bright spots).
//! They contain hill shading and cast shadows (trees, buildings). Resolution: one texel per 2 map cells along x and
//! one per cell along z, padded to a 32/64 texel multiple (the padding is repeated edge data).

use anyhow::{ensure, Context, Result};

pub const LAYERS: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Layer {
    /// `p`-th quantile (0..1) of the texel values, 0..1.
    pub fn percentile(&self, p: f32) -> f32 {
        let mut hist = [0usize; 256];
        self.px.iter().for_each(|&v| hist[v as usize] += 1);
        let target = (self.px.len() as f32 * p) as usize;
        let mut acc = 0;
        hist.iter().position(|&n| { acc += n; acc > target }).unwrap_or(255) as f32 / 255.0
    }

    /// Bilinear lookup at texel coordinates (texel centres at +0.5), clamped to the image.
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let (fx, fy) = ((x - 0.5).clamp(0.0, (self.w - 1) as f32), (y - 0.5).clamp(0.0, (self.h - 1) as f32));
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.px[y * self.w + x] as f32;
        let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
        let bot = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
        (top * (1.0 - ty) + bot * ty) / 255.0
    }
}

pub fn parse(d: &[u8]) -> Result<Vec<Layer>> {
    ensure!(d.len() > 24, "short shadow record");
    let end: Vec<usize> = (0..LAYERS).map(|i| u32::from_le_bytes(d[i * 4..i * 4 + 4].try_into().unwrap()) as usize).collect();
    let mut out = Vec::with_capacity(LAYERS);
    let mut start = 24;
    for (i, &e) in end.iter().enumerate() {
        // end[i] is the end of blob i (== start of blob i+1); the last entry is the record length
        let blob_end = if i + 1 < LAYERS { end[i] } else { d.len() };
        ensure!(start < blob_end && blob_end <= d.len(), "bad shadow blob {i} range {start}..{blob_end}");
        let img = image::load_from_memory_with_format(&d[start..blob_end], image::ImageFormat::Png).with_context(|| format!("shadow layer {i}"))?.to_luma8();
        out.push(Layer { w: img.width() as usize, h: img.height() as usize, px: img.into_raw() });
        start = e;
    }
    ensure!(out.iter().all(|l| (l.w, l.h) == (out[0].w, out[0].h)), "shadow layers differ in size");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, v: u8) -> Vec<u8> {
        let mut b = Vec::new();
        let img = image::GrayImage::from_pixel(w, h, image::Luma([v]));
        img.write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png).unwrap();
        b
    }

    #[test]
    fn splits_six_blobs_and_samples() {
        let blobs: Vec<Vec<u8>> = (0..LAYERS).map(|i| png(4, 2, (i * 50) as u8)).collect();
        let mut d = vec![0u8; 24];
        let mut pos = 24;
        for (i, b) in blobs.iter().enumerate() {
            d.extend(b);
            pos += b.len();
            if i < LAYERS - 1 {
                d[i * 4..i * 4 + 4].copy_from_slice(&(pos as u32).to_le_bytes());
            }
        }
        d[20..24].copy_from_slice(&(pos as u32).to_le_bytes());
        let l = parse(&d).unwrap();
        assert_eq!(l.len(), 6);
        assert_eq!((l[2].w, l[2].h), (4, 2));
        assert!((l[2].sample(2.0, 1.0) - 100.0 / 255.0).abs() < 1e-6);
        assert!((l[5].sample(100.0, 100.0) - 250.0 / 255.0).abs() < 1e-6); // clamped
    }

    #[test]
    fn bilinear_between_texels() {
        let l = Layer { w: 2, h: 1, px: vec![0, 255] };
        assert!((l.sample(1.0, 0.5) - 0.5).abs() < 1e-6);
    }
}
