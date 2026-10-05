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

/// Day time ranges (`DayTimeForGroundShadows = GameDayTime * 15`) and the layer pair blended in each
/// (`FUN_100b7a72` Gamecode @0x100b7a72: table @0x101c14d0 (9 bounds), @0x101c14f4 (from layer), @0x101c1518 (to layer);
/// `BlendShadowMaps(out, from, to, w, h, 1 - (t - lo) / (hi - lo))`): night (5) until 15300, then 5->0 (dawn), 0->1, 1->2 (noon
/// at 50400), 2->3, 3->4 (dusk), 4->5, night again from 85500.
const BOUNDS: [f32; 9] = [0.0, 15300.0, 27000.0, 38700.0, 50400.0, 62100.0, 73800.0, 85500.0, 97700.0];
const FROM: [usize; 8] = [5, 5, 0, 1, 2, 3, 4, 5];
const TO: [usize; 8] = [5, 0, 1, 2, 3, 4, 5, 5];

/// The ground shadow map at `t = DayTimeForGroundShadows`: the client's blend of two layers (`BlendShadowMaps`).
pub fn at_time(layers: &[Layer], t: f32) -> Layer {
    let i = (0..8).find(|&i| t >= BOUNDS[i] && t < BOUNDS[i + 1]).unwrap_or(0);
    let w = 1.0 - (t - BOUNDS[i]) / (BOUNDS[i + 1] - BOUNDS[i]);
    let (a, b) = (&layers[FROM[i]], &layers[TO[i]]);
    let px = a.px.iter().zip(&b.px).map(|(&x, &y)| (x as f32 * w + y as f32 * (1.0 - w)).round() as u8).collect();
    Layer { w: a.w, h: a.h, px }
}

/// Ground brightness of a shadow map sample `s` (0..1 = level / 15): `AnarchyGround_t::SetShadowMap` (DisplaySystem
/// @0x10032570) builds a 16 entry palette `(level / 15 * 188 + 4) * (1 - fade)` (bytes; fade = 0 here), normalised to the lit
/// level 15 (192) [guess: the engine's final gain of the lightmap stage is not decoded].
pub fn brightness(s: f32) -> f32 {
    (4.0 + 188.0 * s.clamp(0.0, 1.0)) / 192.0
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
    fn day_time_blends_the_client_layer_pairs() {
        let l: Vec<Layer> = (0..LAYERS).map(|i| Layer { w: 1, h: 1, px: vec![(i * 40) as u8] }).collect();
        assert_eq!(at_time(&l, 50400.0).px[0], 80); // noon: layer 2 only
        assert_eq!(at_time(&l, 3000.0).px[0], 200); // night: layer 5
        assert_eq!(at_time(&l, 38700.0 + 11700.0 * 0.5).px[0], 60); // halfway 1 -> 2
        assert!((brightness(1.0) - 1.0).abs() < 1e-6 && (brightness(0.0) - 4.0 / 192.0).abs() < 1e-6);
    }

    #[test]
    fn bilinear_between_texels() {
        let l = Layer { w: 2, h: 1, px: vec![0, 255] };
        assert!((l.sample(1.0, 0.5) - 0.5).abs() < 1e-6);
    }
}
