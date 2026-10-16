//! Ground texture bytes shared by the format producer and policy-aware renderer.
//! DS 1007690b disables BC; Randy 1006bfc1 is bundled D3DX filter 2, not a driver box filter.
use crate::{Texture, TextureSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format { Rgb888, Rgba8888, Rgb565, Rgba4444, Rgba1555 }

impl Format {
    pub fn tile(source: TextureSource, quality: i32) -> Self {
        let alpha = matches!(source, TextureSource::Rgba | TextureSource::PngRgba | TextureSource::GroundRgba | TextureSource::Generated);
        match (alpha, quality) {
            (true, 0) => Self::Rgba4444,
            (true, _) => Self::Rgba8888,
            (false, q) if q < 2 => Self::Rgb565,
            _ => Self::Rgb888,
        }
    }

    /// Native packed writers truncate high bits; readers replicate them (Randy
    /// 1006c5d7/1006c62b, 1006ce4c/1006cead, 1006cb80).
    pub fn pixel(self, pixel: [u8; 4]) -> [u8; 4] {
        let bits = match self {
            Self::Rgb888 => [8, 8, 8, 0], Self::Rgba8888 => [8; 4],
            Self::Rgb565 => [5, 6, 5, 0], Self::Rgba4444 => [4; 4],
            Self::Rgba1555 => [5, 5, 5, 1],
        };
        std::array::from_fn(|c| match bits[c] {
            0 => 255, 1 => if pixel[c] >= 128 { 255 } else { 0 }, 8 => pixel[c],
            n => { let q = pixel[c] >> (8 - n); (q << (8 - n)) | (q >> (2 * n - 8)) }
        })
    }
}

/// One D3DX filter-2 level, including source and destination packed-format
/// conversion. Half-size coordinates have fractions 0x8001 after subtracting
/// 0x7fff; their quantized weights are [63,64,64,64], summed with >>8 (no bias).
pub fn downsample(rgba: &[u8], width: u32, height: u32, format: Format) -> Vec<u8> {
    let (nw, nh) = ((width / 2).max(1), (height / 2).max(1));
    let (dx, dy) = ((width << 16) / nw, (height << 16) / nh);
    let mut result = vec![0; (nw * nh * 4) as usize];
    for y in 0..nh { for x in 0..nw {
        let (fx, fy) = ((dx / 2 + x * dx).saturating_sub(0x7fff), (dy / 2 + y * dy).saturating_sub(0x7fff));
        let (u, v) = (fx & 0xffff, fy & 0xffff);
        let uv = (u * v) >> 16;
        let weights = [(0x10000 - (u - uv) - (v - uv) - uv) >> 8, (u - uv) >> 8, (v - uv) >> 8, uv >> 8];
        let mut sum = [0u32; 4];
        for (i, (ox, oy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            let sx = ((fx >> 16) + ox).min(width - 1);
            let sy = ((fy >> 16) + oy).min(height - 1);
            let offset = ((sy * width + sx) * 4) as usize;
            let pixel = format.pixel(rgba[offset..offset + 4].try_into().expect("RGBA pixel"));
            for c in 0..4 { sum[c] += pixel[c] as u32 * weights[i]; }
        }
        let pixel = format.pixel(sum.map(|s| (s >> 8) as u8));
        let offset = ((y * nw + x) * 4) as usize;
        result[offset..offset + 4].copy_from_slice(&pixel);
    }}
    result
}

/// LockRTexture's requested 8x8 level (DS 1003598a). Default parser output uses
/// quality 2; the renderer repeats this with the actual resource policy.
pub fn tile_mip(texture: &Texture, quality: i32) -> [[u8; 4]; 64] {
    let format = Format::tile(texture.source, quality);
    let (mut w, mut h) = (texture.width, texture.height);
    let mut pixels = std::borrow::Cow::Borrowed(texture.rgba.as_slice());
    while w > 8 || h > 8 {
        pixels = std::borrow::Cow::Owned(downsample(&pixels, w, h, format));
        w = (w / 2).max(1); h = (h / 2).max(1);
    }
    std::array::from_fn(|i| {
        let offset = (((i / 8) as u32 * h / 8 * w + (i % 8) as u32 * w / 8) * 4) as usize;
        format.pixel(pixels[offset..offset + 4].try_into().expect("RGBA pixel"))
    })
}

pub fn oriented_pixel(orientation: u16, x: usize, y: usize) -> usize {
    match orientation & 3 {
        0 => (7 - y) * 8 + x, 1 => (7 - x) * 8 + 7 - y,
        2 => y * 8 + 7 - x, _ => x * 8 + y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_filter_two_has_recursive_truncation_not_box_rounding() {
        let image = Texture { width: 32, height: 32, rgba: vec![255; 32 * 32 * 4], source: TextureSource::GroundRgba };
        assert_eq!(tile_mip(&image, 2), [[253; 4]; 64]);
        // Packed white254 truncates to nibble15 and expands back to255 between levels.
        assert_eq!(tile_mip(&image, 0), [[255; 4]; 64]);
        let rgb = Texture { source: TextureSource::GroundRgb, ..image };
        assert_eq!(tile_mip(&rgb, 2), [[253, 253, 253, 255]; 64]);
        let pixels = [0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0];
        assert_eq!(downsample(&pixels, 2, 2, Format::Rgba8888), [127; 4]);
    }
    #[test]
    fn native_composite_formats_truncate_and_replicate_bits() {
        assert_eq!(Format::Rgb565.pixel([123, 127, 251, 0]), [123, 125, 255, 255]);
        assert_eq!(Format::Rgba1555.pixel([123, 127, 251, 127]), [123, 123, 255, 0]);
        assert_eq!(Format::Rgba1555.pixel([123, 127, 251, 128]), [123, 123, 255, 255]);
    }
}
