//! Load-time Enhanced material maps. No image work is performed by frame rendering.
//! Normal and GGX filtering follows ao-vk material_maps.h (MIT; see THIRD_PARTY).
use std::{path::{Path, PathBuf}, io::BufReader};
use ao_scene::{TextureKey, TextureSource};

pub(super) struct Chain { pub w: u32, pub h: u32, pub levels: Vec<Vec<u8>> }
#[derive(Clone, Copy, Default)]
pub(super) struct Selection { pub generated: bool, pub maps: bool }
pub(super) struct BuiltMaterial {
    pub generated_normal: Option<Chain>,
    pub side_normal: Option<Chain>,
    pub orm: Option<Chain>,
    pub emissive: Option<Chain>,
    pub mask: u8,
}
struct Image { w: u32, h: u32, data: Vec<u8> }
const MAX_EDGE: u32 = 2048;
const MAX_BYTES: u64 = 32 * 1024 * 1024;
#[cfg(test)]
thread_local! { static FILE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

pub(super) fn default_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .map(|p| p.join("Library/Application Support/aomac/materials"))
}
pub(super) fn canonical(key: TextureKey) -> TextureKey {
    TextureKey { rdb_type: match key.rdb_type {
        1_010_016 | 1_010_017 => 1_010_004,
        1_010_021 | 1_010_022 => 1_010_006,
        1_010_019 | 1_010_020 => 1_010_011,
        value => value,
    }, id: key.id }
}
fn decode(path: &Path) -> Result<Image, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_BYTES { return Err("PNG exceeds 32 MiB".into()); }
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_limits(png::Limits { bytes: MAX_BYTES as usize });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > MAX_EDGE || h > MAX_EDGE { return Err("PNG dimensions outside 1..2048".into()); }
    let mut bytes = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1, png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3, png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("unexpanded indexed PNG".into()),
    };
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for p in bytes[..info.buffer_size()].chunks_exact(channels) {
        let rgba = match channels { 1 => [p[0], p[0], p[0], 255], 2 => [p[0], p[0], p[0], p[1]], 3 => [p[0], p[1], p[2], 255], _ => [p[0], p[1], p[2], p[3]] };
        data.extend_from_slice(&rgba);
    }
    Ok(Image { w, h, data })
}
fn load(root: Option<&Path>, key: TextureKey, suffix: &str) -> Option<Image> {
    let root = root?;
    let key = canonical(key);
    for name in [format!("{}_{}_{suffix}.png", key.rdb_type, key.id), format!("{}_{suffix}.png", key.id)] {
        let path = root.join(name);
        #[cfg(test)]
        FILE_PROBES.with(|count| count.set(count.get() + 1));
        if !path.is_file() { continue; }
        match decode(&path) {
            Ok(image) => return Some(image),
            Err(error) => eprintln!("material map {}: {error}", path.display()),
        }
    }
    None
}
fn encode(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 }
fn normal_pixel(v: [f32; 3], coherence: f32) -> [u8; 4] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 { return [128, 128, 255, 0]; }
    [encode(v[0] / len * 0.5 + 0.5), encode(v[1] / len * 0.5 + 0.5), encode(v[2] / len * 0.5 + 0.5), encode(coherence)]
}
fn unpack(p: &[u8]) -> [f32; 3] {
    // Exact representable flat normal avoids a bias at every level.
    [(f32::from(p[0]) - 128.0) / 127.0, (f32::from(p[1]) - 128.0) / 127.0, (f32::from(p[2]) - 128.0) / 127.0]
}
#[derive(Clone, Copy)]
enum Filter { Normal, Orm(u8), Emissive }
fn chain(mut image: Image, filter: Filter) -> Chain {
    if matches!(filter, Filter::Normal) {
        for p in image.data.as_chunks_mut::<4>().0 { let v = unpack(p); p.copy_from_slice(&normal_pixel(v, 1.0)); }
    }
    let (w, h) = (image.w, image.h);
    let mut levels = Vec::new();
    loop {
        let (cw, ch) = (image.w, image.h);
        if cw == 1 && ch == 1 { levels.push(image.data); break; }
        let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
        let mut next = vec![0; (nw * nh * 4) as usize];
        for y in 0..nh { for x in 0..nw {
            let mut sum = [0.0f32; 4];
            let mut count = 0.0;
            // Area bins retain odd-edge texels instead of discarding the last row.
            for sy in y * ch / nh..(y + 1) * ch / nh { for sx in x * cw / nw..(x + 1) * cw / nw {
                let p = &image.data[((sy * cw + sx) * 4) as usize..][..4];
                match filter {
                    Filter::Normal => { let n = unpack(p); for c in 0..3 { sum[c] += n[c] * f32::from(p[3]) / 255.0; } },
                    Filter::Orm(_) => { sum[0] += f32::from(p[0]) / 255.0; sum[1] += (f32::from(p[1]) / 255.0).powi(4); sum[2] += f32::from(p[2]) / 255.0; },
                    Filter::Emissive => { for c in 0..3 { sum[c] += (f32::from(p[c]) / 255.0).powf(2.2); } },
                }
                count += 1.0;
            } }
            for value in &mut sum { *value /= count; }
            let pixel = match filter {
                Filter::Normal => { let len = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt(); normal_pixel([sum[0], sum[1], sum[2]], len) },
                Filter::Orm(mask) => [encode(sum[0]), encode(sum[1].sqrt().sqrt()), encode(sum[2]), mask],
                Filter::Emissive => [encode(sum[0].powf(1.0 / 2.2)), encode(sum[1].powf(1.0 / 2.2)), encode(sum[2].powf(1.0 / 2.2)), 255],
            };
            next[((y * nw + x) * 4) as usize..][..4].copy_from_slice(&pixel);
        } }
        levels.push(image.data);
        image = Image { w: nw, h: nh, data: next };
    }
    Chain { w, h, levels }
}
fn generated(rgba: &[u8], w: u32, h: u32) -> Chain {
    if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
        return chain(Image { w: 1, h: 1, data: vec![128, 128, 255, 255] }, Filter::Normal);
    }
    let luma = |x: u32, y: u32| { let p = &rgba[((y * w + x) * 4) as usize..]; (0.3 * f32::from(p[0]) + 0.59 * f32::from(p[1]) + 0.11 * f32::from(p[2])) / 255.0 };
    let mut data = Vec::with_capacity(rgba.len());
    for y in 0..h { for x in 0..w {
        let dx = (luma((x + 1) % w, y) - luma((x + w - 1) % w, y)) * 0.5;
        let dy = (luma(x, (y + 1) % h) - luma(x, (y + h - 1) % h)) * 0.5;
        data.extend_from_slice(&normal_pixel([-dx, dy, 1.0], 1.0));
    } }
    chain(Image { w, h, data }, Filter::Normal)
}
fn channel(image: &Option<Image>, x: u32, y: u32, w: u32, h: u32, default: u8) -> u8 {
    image.as_ref().map_or(default, |i| i.data[(((y * i.h / h) * i.w + x * i.w / w) * 4) as usize])
}
pub(super) fn build(key: TextureKey, rgba: &[u8], w: u32, h: u32, _source: TextureSource, root: Option<&Path>, selection: Selection) -> BuiltMaterial {
    let generated_normal = selection.generated.then(|| generated(rgba, w, h));
    if !selection.maps {
        return BuiltMaterial { generated_normal, side_normal: None, orm: None, emissive: None, mask: 0 };
    }
    let side_normal = load(root, key, "n").map(|i| chain(i, Filter::Normal));
    let emissive = load(root, key, "e").map(|i| chain(i, Filter::Emissive));
    let mut packed = load(root, key, "orm");
    if packed.is_none() {
        let (r, m, ao) = (load(root, key, "r"), load(root, key, "m"), load(root, key, "ao"));
        if let Some(size) = r.as_ref().or(m.as_ref()).or(ao.as_ref()) {
            let (mw, mh) = (size.w, size.h);
            let mut data = Vec::with_capacity((mw * mh * 4) as usize);
            for y in 0..mh { for x in 0..mw { data.extend_from_slice(&[channel(&ao, x, y, mw, mh, 255), channel(&r, x, y, mw, mh, 255), channel(&m, x, y, mw, mh, 0), 0]); } }
            packed = Some(Image { w: mw, h: mh, data });
        }
    }
    let mask = u8::from(side_normal.is_some()) | (u8::from(packed.is_some()) << 1) | (u8::from(emissive.is_some()) << 2);
    if mask == 0 {
        return BuiltMaterial { generated_normal, side_normal, orm: None, emissive, mask };
    }
    // Always retain a mask carrier, even when only a normal/emissive file exists.
    let mut packed = packed.unwrap_or(Image { w: 1, h: 1, data: vec![255, 255, 0, mask] });
    for p in packed.data.as_chunks_mut::<4>().0 { p[3] = mask; }
    BuiltMaterial { generated_normal, side_normal, orm: Some(chain(packed, Filter::Orm(mask))), emissive, mask }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(t: u32) -> TextureKey { TextureKey { rdb_type: t, id: 42 } }
    #[test]
    fn generated_normals_flat_and_ramp_and_ggx_mips() {
        let flat = generated(&[120; 4 * 15], 5, 3);
        assert!(flat.levels.iter().all(|m| m.as_chunks::<4>().0.iter().all(|p| *p == [128, 128, 255, 255])));
        let pixels: Vec<u8> = (0..16).flat_map(|x| { let v = (x % 4 * 64) as u8; [v, v, v, 255] }).collect();
        let ramp = generated(&pixels, 4, 4);
        assert!(ramp.levels[0].as_chunks::<4>().0.iter().any(|p| p[0] != 128));
        let orm = chain(Image { w: 2, h: 1, data: vec![255, 0, 0, 7, 255, 255, 255, 7] }, Filter::Orm(7));
        assert_eq!(orm.levels[1][1], encode(0.5f32.sqrt().sqrt()));
        assert_eq!(orm.levels[1][3], 7);
    }
    #[test]
    fn canonical_texture_families() {
        for (low, full) in [(1010016,1010004),(1010017,1010004),(1010021,1010006),(1010022,1010006),(1010019,1010011),(1010020,1010011)] { assert_eq!(canonical(key(low)), key(full)); }
    }
    #[test]
    fn maps_packed_precedence_defaults_and_malformed_bounds() {
        let root = std::env::temp_dir().join(format!("aomac-materials-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&root).unwrap();
        let save = |name: &str, pixel: [u8; 4]| {
            let file = std::fs::File::create(root.join(name)).unwrap();
            let mut enc = png::Encoder::new(file, 1, 1); enc.set_color(png::ColorType::Rgba); enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&pixel).unwrap();
        };
        save("42_m.png", [180,180,180,255]);
        let build_maps = || build(key(1010016), &[255;4], 1, 1, TextureSource::Generated, Some(&root), Selection { generated: false, maps: true });
        assert_eq!(&build_maps().orm.unwrap().levels[0][..3], &[255,255,180]);
        save("1010004_42_orm.png", [100,80,60,255]);
        assert_eq!(&build_maps().orm.unwrap().levels[0][..3], &[100,80,60]);
        std::fs::write(root.join("42_n.png"), b"\x89PNG\r\n\x1a\ntruncated").unwrap();
        assert!(build_maps().side_normal.is_none());
        assert!(FILE_PROBES.with(|count| count.get()) > 0, "enabled maps exercise file lookup");
        FILE_PROBES.with(|count| count.set(0));
        let disabled = build(key(1010016), &[], 0, 0, TextureSource::Generated, Some(&root), Selection::default());
        assert!(disabled.generated_normal.is_none());
        assert!(disabled.side_normal.is_none() && disabled.orm.is_none() && disabled.emissive.is_none());
        assert_eq!(FILE_PROBES.with(|count| count.get()), 0, "all-off must not probe the material directory");
        let generated_only = build(key(1010016), &[255;4], 1, 1, TextureSource::Generated, Some(&root), Selection { generated: true, maps: false });
        assert!(generated_only.generated_normal.is_some());
        assert!(generated_only.orm.is_none() && generated_only.mask == 0);
        assert_eq!(FILE_PROBES.with(|count| count.get()), 0, "generated normals must not probe PNG files");
        std::fs::remove_dir_all(root).unwrap();
    }
}
