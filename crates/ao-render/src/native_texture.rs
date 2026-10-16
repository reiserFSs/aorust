//! Randy 100193e4 selects RGB565/RGB888, RGBA4444/RGBA8888, or DXT1/DXT3.
//! Modern Metal has no packed 565/4444 sampled formats: expand those exact bits
//! to RGBA8. Compressed textures remain actual BC1/BC2 GPU allocations.
use ao_scene::TextureSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub quality: i32,
    pub compressed: bool,
    /// aomac Enhanced option "Better Mip Filtering" (not retail, default off): gamma-correct downsampling plus
    /// coverage-preserving alpha scaling for alpha-tested textures ([`build_texture_mips`]).
    /// Native ground uses bundled D3DX filter2 on encoded bytes; ordinary resources
    /// retain the earlier box-mip approximation. This option changes load-time filtering,
    /// not the GPU texture format or byte layout.
    pub better: bool,
}

impl Default for Policy {
    fn default() -> Self { Self { quality: 2, compressed: false, better: false } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding { Rgba8, Rgb565, Rgba4444, Bc1, Bc2 }

impl Policy {
    pub fn encoding(self, source: TextureSource, bc_supported: bool, width: u32, height: u32) -> Encoding {
        let alpha = matches!(source, TextureSource::Rgba | TextureSource::PngRgba | TextureSource::GroundRgba);
        if matches!(source, TextureSource::Generated | TextureSource::GroundComposite565 | TextureSource::GroundComposite1555) { return Encoding::Rgba8; }
        // Native .png temporarily disables compression. DXT allocation requires
        // a full block at the base level; unsupported layouts use native RGB(A).
        if self.compressed && bc_supported && matches!(source, TextureSource::Rgb | TextureSource::Rgba) && width.is_multiple_of(4) && height.is_multiple_of(4) {
            if alpha { Encoding::Bc2 } else { Encoding::Bc1 }
        } else if alpha && self.quality == 0 { Encoding::Rgba4444 }
        else if !alpha && self.quality < 2 { Encoding::Rgb565 }
        else { Encoding::Rgba8 }
    }
}

impl Encoding {
    pub fn format(self) -> wgpu::TextureFormat {
        match self { Self::Bc1 => wgpu::TextureFormat::Bc1RgbaUnorm, Self::Bc2 => wgpu::TextureFormat::Bc2RgbaUnorm, _ => wgpu::TextureFormat::Rgba8Unorm }
    }
    pub fn block_bytes(self) -> u32 { match self { Self::Bc1 => 8, Self::Bc2 => 16, _ => 4 } }
    pub fn block_edge(self) -> u32 { match self { Self::Bc1 | Self::Bc2 => 4, _ => 1 } }
    pub fn encode(self, rgba: &[u8], width: u32, height: u32, output: &mut Vec<u8>) {
        match self {
            Self::Bc1 | Self::Bc2 => {
                let format = if self == Self::Bc1 { texpresso::Format::Bc1 } else { texpresso::Format::Bc2 };
                output.resize(format.compressed_size(width as usize, height as usize), 0);
                format.compress(rgba, width as usize, height as usize, texpresso::Params::default(), output);
            }
            Self::Rgba8 => { output.clear(); output.extend_from_slice(rgba); }
            Self::Rgb565 | Self::Rgba4444 => {
                output.clear();
                output.reserve(rgba.len());
                for pixel in rgba.as_chunks::<4>().0 {
                    let expand = |v: u8, bits: u32| {
                        let q = v >> (8 - bits);
                        (q << (8 - bits)) | (q >> (2 * bits - 8))
                    };
                    if self == Self::Rgb565 {
                        output.extend_from_slice(&[expand(pixel[0], 5), expand(pixel[1], 6), expand(pixel[2], 5), 255]);
                    } else {
                        output.extend(pixel.iter().map(|&v| expand(v, 4)));
                    }
                }
            }
        }
    }
}

/// Pre-refactor reference implementation of the box-filter mip chain + encode, kept only in tests to
/// prove [`build_texture_mips`] produces byte-identical output after the ownership/parallelism rework.
#[cfg(test)]
fn reference_mip_chain(policy: Policy, source: TextureSource, bc_supported: bool, rgba: &[u8], w: u32, h: u32) -> Vec<(u32, u32, Vec<u8>)> {
    let encoding = policy.encoding(source, bc_supported, w, h);
    let mips = 32 - w.max(h).leading_zeros();
    let (mut cw, mut ch, mut data) = (w, h, rgba.to_vec());
    let mut encoded = Vec::new();
    let mut out = Vec::new();
    for level in 0..mips {
        let bytes = if encoding == Encoding::Rgba8 { data.clone() } else { encoding.encode(&data, cw, ch, &mut encoded); encoded.clone() };
        out.push((cw, ch, bytes));
        if level + 1 == mips { break; }
        let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut s = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = ((2 * x + dx).min(cw - 1), (2 * y + dy).min(ch - 1));
                        s += data[((sy * cw + sx) * 4 + c) as usize] as u32;
                    }
                    next[((y * nw + x) * 4 + c) as usize] = ((s + 2) / 4) as u8;
                }
            }
        }
        (cw, ch, data) = (nw, nh, next);
    }
    out
}

pub struct BuiltLevel<'a> {
    pub cw: u32,
    pub ch: u32,
    pub bytes: std::borrow::Cow<'a, [u8]>,
}

pub struct BuiltTexture<'a> {
    pub w: u32,
    pub h: u32,
    pub mips: u32,
    pub encoding: Encoding,
    pub levels: Vec<BuiltLevel<'a>>,
}

impl BuiltTexture<'_> {
    pub fn ground_tile_mip(&self) -> Option<&[[u8; 4]]> {
        if self.encoding.block_edge() != 1 { return None; }
        self.levels.iter().find(|level| level.cw == 8 && level.ch == 8)
            .map(|level| level.bytes.as_chunks::<4>().0)
    }
}

/// Coverage-preserving alpha test threshold: `shade`/`fs_test`/`fs_sky_test` in shader.wgsl discard below `0.5`.
const ALPHA_TEST_THRESHOLD: u8 = 128;

fn alpha_coverage(alpha: &[u8]) -> f32 {
    if alpha.is_empty() { return 0.0; }
    alpha.iter().filter(|&&a| a >= ALPHA_TEST_THRESHOLD).count() as f32 / alpha.len() as f32
}

/// Shifts `alpha` by a flat additive bias so its coverage (fraction `>= ALPHA_TEST_THRESHOLD`) matches `target`,
/// found by binary search on the bias ("Computing Alpha Mipmaps", Castano 2007): preserves how much of a mip is
/// "solid" after a box-filtered downsample softens hard alpha-tested edges (foliage, fences, hair), which
/// otherwise thins out and vanishes at a distance despite being just as dense up close. An additive bias (not a
/// scale around the threshold) is required: a scale leaves any texel already sitting exactly on the threshold
/// unmoved and can only push values further from it, so it cannot recover coverage once every texel in a run
/// of a uniformly-spaced dot pattern has softened to the same below-threshold grey; a flat shift lifts them all.
fn preserve_alpha_coverage(alpha: &mut [u8], target: f32) {
    if alpha.is_empty() || target <= 0.0 || target >= 1.0 {
        return;
    }
    let shift = |a: u8, bias: f32| (a as f32 + bias).round().clamp(0.0, 255.0) as u8;
    let coverage_at = |bias: f32| alpha_coverage(&alpha.iter().map(|&a| shift(a, bias)).collect::<Vec<u8>>());
    let (mut lo, mut hi) = (-255.0f32, 255.0f32);
    for _ in 0..16 {
        let mid = (lo + hi) * 0.5;
        if coverage_at(mid) < target { lo = mid; } else { hi = mid; }
    }
    let bias = (lo + hi) * 0.5;
    for a in alpha.iter_mut() {
        *a = shift(*a, bias);
    }
}

/// Ground's bundled D3DX path: packed conversion between recursive filter-2
/// levels; patch composites instead have a fixed packed format and one level.
fn build_ground_mips(policy: Policy, source: TextureSource, rgba: &[u8], w: u32, h: u32) -> BuiltTexture<'_> {
    use ao_scene::ground_texture::{Format, downsample};
    let composite = matches!(source, TextureSource::GroundComposite565 | TextureSource::GroundComposite1555);
    let format = match source {
        TextureSource::GroundComposite565 => Format::Rgb565,
        TextureSource::GroundComposite1555 => Format::Rgba1555,
        _ => Format::tile(source, policy.quality),
    };
    let mut data: Vec<_> = rgba.as_chunks::<4>().0.iter().flat_map(|&p| format.pixel(p)).collect();
    let mips = if composite { 1 } else { 32 - w.max(h).leading_zeros() };
    let mut levels = Vec::with_capacity(mips as usize);
    let (mut cw, mut ch) = (w, h);
    for level in 0..mips {
        let next = (level + 1 < mips).then(|| downsample(&data, cw, ch, format));
        levels.push(BuiltLevel { cw, ch, bytes: std::borrow::Cow::Owned(data) });
        let Some(next) = next else { break; };
        data = next; cw = (cw / 2).max(1); ch = (ch / 2).max(1);
    }
    BuiltTexture { w, h, mips, encoding: Encoding::Rgba8, levels }
}

/// CPU-side mip construction plus BC/packed-format encoding. Ground follows the
/// recovered native policy; ordinary resources retain the earlier box approximation.
/// Factored out of `Renderer::texture_view` as a pure function (no GPU access) so a batch
/// of textures can be built across threads; the caller still issues the actual `queue.write_texture`
/// calls serially afterward (audit item 10). Base-level bytes are borrowed from `rgba` when the
/// upload format is raw RGBA8 (no copy); every other buffer is produced once by the box filter or the
/// BC/565/4444 encoder and moved, never duplicated.
///
/// `policy.better` (aomac Enhanced option, not retail, default off) downsamples the RGB channels in linear
/// light instead of averaging sRGB bytes directly, and, when `alpha_test` is set (the texture is used by at
/// least one `Blend::AlphaTest` submesh), rescales every mip's alpha channel to keep the base level's
/// alpha-test coverage ([`preserve_alpha_coverage`]).
pub fn build_texture_mips<'a>(policy: Policy, source: TextureSource, bc_supported: bool, rgba: &'a [u8], w: u32, h: u32, alpha_test: bool) -> BuiltTexture<'a> {
    if matches!(source, TextureSource::GroundComposite565 | TextureSource::GroundComposite1555)
        || (!policy.better && matches!(source, TextureSource::GroundRgb | TextureSource::GroundRgba)) {
        return build_ground_mips(policy, source, rgba, w, h);
    }
    let encoding = policy.encoding(source, bc_supported, w, h);
    let mips = 32 - w.max(h).leading_zeros();
    let (mut cw, mut ch) = (w, h);
    let mut raw: std::borrow::Cow<'a, [u8]> = std::borrow::Cow::Borrowed(rgba);
    let mut levels = Vec::with_capacity(mips as usize);
    let base_coverage = (policy.better && alpha_test).then(|| alpha_coverage(&rgba.iter().skip(3).step_by(4).copied().collect::<Vec<u8>>()));
    for level in 0..mips {
        let last = level + 1 == mips;
        // Downsample before consuming `raw` into this level's output bytes below.
        let next = if last {
            None
        } else {
            let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
            let mut next = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut s = 0u32;
                        let mut lin = 0.0f32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (sx, sy) = ((2 * x + dx).min(cw - 1), (2 * y + dy).min(ch - 1));
                            let v = raw[((sy * cw + sx) * 4 + c) as usize];
                            s += v as u32;
                            if policy.better && c < 3 { lin += (v as f32 / 255.0).powf(2.2); }
                        }
                        next[((y * nw + x) * 4 + c) as usize] = if policy.better && c < 3 {
                            ((lin / 4.0).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8
                        } else {
                            ((s + 2) / 4) as u8
                        };
                    }
                }
            }
            if let Some(target) = base_coverage {
                let mut alpha: Vec<u8> = next.iter().skip(3).step_by(4).copied().collect();
                preserve_alpha_coverage(&mut alpha, target);
                for (i, a) in alpha.into_iter().enumerate() { next[i * 4 + 3] = a; }
            }
            Some((nw, nh, next))
        };
        let bytes: std::borrow::Cow<'a, [u8]> = if encoding == Encoding::Rgba8 {
            raw
        } else {
            let mut encoded = Vec::new();
            encoding.encode(&raw, cw, ch, &mut encoded);
            std::borrow::Cow::Owned(encoded)
        };
        levels.push(BuiltLevel { cw, ch, bytes });
        match next {
            Some((nw, nh, n)) => { cw = nw; ch = nh; raw = std::borrow::Cow::Owned(n); }
            None => break,
        }
    }
    BuiltTexture { w, h, mips, encoding, levels }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_ground_upload_uses_recursive_packed_mips_and_single_level_composites() {
        let rgba = vec![255; 32 * 32 * 4];
        let policy = Policy { quality: 0, compressed: true, better: false };
        let fine = build_texture_mips(policy, TextureSource::GroundRgba, true, &rgba, 32, 32, false);
        assert_eq!(fine.ground_tile_mip().unwrap(), &[[255; 4]; 64]);
        let coarse = build_texture_mips(policy, TextureSource::GroundComposite1555, true, &rgba, 32, 32, false);
        assert_eq!(coarse.mips, 1);
        assert_eq!(coarse.levels.len(), 1);
        assert_eq!(coarse.encoding.format(), wgpu::TextureFormat::Rgba8Unorm);
    }

    #[test]
    fn native_format_selection_and_upload_bytes() {
        let low = Policy { quality: 0, compressed: true, better: false };
        assert_eq!(low.encoding(TextureSource::Rgb, true, 4, 4), Encoding::Bc1);
        assert_eq!(low.encoding(TextureSource::Rgba, true, 4, 4), Encoding::Bc2);
        assert_eq!(low.encoding(TextureSource::PngRgb, true, 4, 4), Encoding::Rgb565);
        assert_eq!(low.encoding(TextureSource::PngRgba, true, 4, 4), Encoding::Rgba4444);
        assert_eq!(low.encoding(TextureSource::Generated, true, 4, 4), Encoding::Rgba8);
        assert_eq!(low.encoding(TextureSource::Rgb, false, 4, 4), Encoding::Rgb565);
        assert_eq!(low.encoding(TextureSource::GroundRgb, true, 4, 4), Encoding::Rgb565, "ground disables BC even for JPEG");
        assert_eq!(low.encoding(TextureSource::GroundRgba, true, 4, 4), Encoding::Rgba4444);
        let mut encoded = Vec::new();
        Encoding::Rgb565.encode(&[123, 127, 251, 17], 1, 1, &mut encoded);
        assert_eq!(encoded, [123, 125, 255, 255]);
        Encoding::Rgba4444.encode(&[123, 127, 251, 17], 1, 1, &mut encoded);
        assert_eq!(encoded, [119, 119, 255, 17]);
        let pixels: Vec<_> = (0..16).flat_map(|_| [255, 0, 0, 136]).collect();
        for format in [Encoding::Bc1, Encoding::Bc2] {
            format.encode(&pixels, 4, 4, &mut encoded);
            assert_eq!(encoded.len(), format.block_bytes() as usize);
            let decoder = if format == Encoding::Bc1 { texpresso::Format::Bc1 } else { texpresso::Format::Bc2 };
            let mut decoded = vec![0; 64];
            decoder.decompress(&encoded, 4, 4, &mut decoded);
            assert_eq!(&decoded[..3], &[255, 0, 0]);
            if format == Encoding::Bc2 { assert_eq!(decoded[3], 136); }
        }
    }

    fn synth_rgba(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| [(i * 37 % 251) as u8, (i * 53 % 241) as u8, (i * 97 % 239) as u8, (i * 131 % 233) as u8]).collect()
    }

    type MipLevels = Vec<(u32, u32, Vec<u8>)>;
    fn built_levels(b: &BuiltTexture) -> MipLevels {
        b.levels.iter().map(|l| (l.cw, l.ch, l.bytes.to_vec())).collect()
    }

    #[test]
    fn mip_chain_byte_identical_to_pre_refactor_reference() {
        // Square power-of-two, non-square (clamp actually matters: smaller axis hits 1px first),
        // and a BC1-compressed case all have to match the old inline per-level implementation exactly.
        let cases: &[(Policy, TextureSource, bool, u32, u32)] = &[
            (Policy { quality: 2, compressed: false, better: false }, TextureSource::Rgb, false, 16, 16),
            (Policy { quality: 2, compressed: false, better: false }, TextureSource::Rgb, false, 1, 8),
            (Policy { quality: 2, compressed: false, better: false }, TextureSource::Rgb, false, 8, 1),
            (Policy { quality: 0, compressed: false, better: false }, TextureSource::PngRgb, false, 12, 6),
            (Policy { quality: 0, compressed: false, better: false }, TextureSource::PngRgba, false, 6, 12),
            (Policy { quality: 2, compressed: true, better: false }, TextureSource::Rgb, true, 8, 8),
            (Policy { quality: 2, compressed: true, better: false }, TextureSource::Rgba, true, 16, 8),
        ];
        for &(policy, source, bc_supported, w, h) in cases {
            let rgba = synth_rgba(w, h);
            let expected = reference_mip_chain(policy, source, bc_supported, &rgba, w, h);
            let built = build_texture_mips(policy, source, bc_supported, &rgba, w, h, false);
            assert_eq!(built_levels(&built), expected, "mismatch for {w}x{h} source={source:?} compressed={}", policy.compressed);
        }
    }

    #[test]
    fn parallel_batch_build_matches_sequential_per_texture() {
        // Simulates Renderer::upload's thread::scope batch: build several distinct textures across
        // worker threads, then assert every result matches building that same texture alone,
        // sequentially -- i.e. no texture's bytes depend on what else ran concurrently.
        let policy = Policy { quality: 2, compressed: true, better: false };
        let sizes = [(8, 8), (16, 16), (4, 4), (8, 4), (32, 8), (4, 8)];
        let rgbas: Vec<Vec<u8>> = sizes.iter().map(|&(w, h)| synth_rgba(w, h)).collect();
        let sequential: Vec<_> = sizes.iter().zip(&rgbas).map(|(&(w, h), rgba)| {
            built_levels(&build_texture_mips(policy, TextureSource::Rgb, true, rgba, w, h, false))
        }).collect();

        let mut parallel: Vec<Option<MipLevels>> = (0..sizes.len()).map(|_| None).collect();
        std::thread::scope(|scope| {
            for ((&(w, h), rgba), slot) in sizes.iter().zip(&rgbas).zip(parallel.iter_mut()) {
                scope.spawn(move || {
                    *slot = Some(built_levels(&build_texture_mips(policy, TextureSource::Rgb, true, rgba, w, h, false)));
                });
            }
        });
        for (i, (seq, par)) in sequential.iter().zip(parallel).enumerate() {
            assert_eq!(*seq, par.unwrap(), "texture {i} diverged between sequential and parallel builds");
        }
    }

    #[test]
    fn better_filtering_is_byte_identical_to_the_box_filter_when_flat() {
        // A uniform-colour texture downsamples to the same colour either way: gamma-correct averaging and
        // plain averaging only disagree on non-uniform input, so this is a cheap identity check.
        let flat: Vec<u8> = (0..16 * 16).flat_map(|_| [200u8, 100, 50, 255]).collect();
        let off = build_texture_mips(Policy { quality: 2, compressed: false, better: false }, TextureSource::Rgba, false, &flat, 16, 16, false);
        let on = build_texture_mips(Policy { quality: 2, compressed: false, better: true }, TextureSource::Rgba, false, &flat, 16, 16, true);
        assert_eq!(built_levels(&off), built_levels(&on));
    }

    #[test]
    fn gamma_correct_downsample_differs_from_srgb_box_filter_on_contrasty_input() {
        // Alternating black/white checkerboard: averaging in linear light brightens the result relative to
        // averaging the sRGB bytes directly (the whole point of gamma-correct downsampling).
        let checker: Vec<u8> = (0..16 * 16).flat_map(|i| { let v = if (i % 16 + i / 16) % 2 == 0 { 255 } else { 0 }; [v, v, v, 255] }).collect();
        let plain = build_texture_mips(Policy { quality: 2, compressed: false, better: false }, TextureSource::Rgba, false, &checker, 16, 16, false);
        let better = build_texture_mips(Policy { quality: 2, compressed: false, better: true }, TextureSource::Rgba, false, &checker, 16, 16, false);
        assert_eq!(plain.levels[1].bytes[0], 128, "plain box filter of a 50/50 checker averages to mid-grey");
        assert!(better.levels[1].bytes[0] > 180, "gamma-correct averaging of a 50/50 checker should look much brighter than mid-grey, got {}", better.levels[1].bytes[0]);
    }

    #[test]
    fn coverage_preserving_alpha_keeps_the_base_levels_alpha_test_coverage() {
        // A 16x16 texture built from 64 2x2 source blocks, each with a different "density" (0..4 solid-alpha
        // pixels out of its 4, cycling through densities so mip texels end up with varied alpha, not all
        // identical) -- a box filter alone drifts the mip's coverage away from the base level's; coverage
        // preservation must pull it back close, within a tolerance for this test's own small 8x8 mip sample.
        let mut rgba = vec![0u8; 16 * 16 * 4];
        for by in 0..8 {
            for bx in 0..8 {
                let density = (by * 8 + bx) % 5; // 0..4 solid pixels out of this block's 4
                for (i, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                    let (x, y) = (bx * 2 + dx, by * 2 + dy);
                    rgba[(y * 16 + x) * 4 + 3] = if i < density { 255 } else { 0 };
                }
            }
        }
        let base_coverage = alpha_coverage(&rgba.iter().skip(3).step_by(4).copied().collect::<Vec<u8>>());
        let plain = build_texture_mips(Policy { quality: 2, compressed: false, better: false }, TextureSource::Rgba, false, &rgba, 16, 16, false);
        let plain_alpha: Vec<u8> = plain.levels[1].bytes.iter().skip(3).step_by(4).copied().collect();
        let plain_drift = (alpha_coverage(&plain_alpha) - base_coverage).abs();
        let built = build_texture_mips(Policy { quality: 2, compressed: false, better: true }, TextureSource::Rgba, false, &rgba, 16, 16, true);
        let alpha: Vec<u8> = built.levels[1].bytes.iter().skip(3).step_by(4).copied().collect();
        let corrected_drift = (alpha_coverage(&alpha) - base_coverage).abs();
        assert!(corrected_drift <= plain_drift, "coverage preservation must not drift further from the base level than the plain box filter: corrected {corrected_drift} vs plain {plain_drift}");
        assert!(corrected_drift < 0.15, "mip level 1 coverage drifted too far from base {base_coverage}: {corrected_drift}");
    }
}
