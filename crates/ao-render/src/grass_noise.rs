//! The slow fields grass is modulated by - clumps, dry and tall patches, lean, wildflower patches - baked once into a
//! tileable texture, so a blade reads them with two fetches instead of evaluating a dozen noises per vertex.
//! World-anchored: the texture repeats every 64 world units (grass.wgsl `NOISE_PERIOD`). Value noise with a smoothstep blend, on lattices
//! that wrap with the texture; ao-vk's field frequencies (see THIRD_PARTY), rounded to whole cycles per period.

pub(super) const SIZE: u32 = 256;
/// Per field, up to two octaves of (cycles across the texture, weight). Layer 0: clump, dry, tall, meadow; layer 1:
/// lean x, lean z, lean strength, spare.
const FIELDS: [[(u32, f32); 2]; 8] = [
    [(22, 0.55), (70, 0.45)], // clump: 0.35 and 1.1 per unit
    [(6, 0.65), (20, 0.35)],  // dry patches: 0.09 and 0.31
    [(8, 1.0), (0, 0.0)],     // tall patches: 0.12
    [(4, 1.0), (0, 0.0)],     // wildflower meadows: 0.07
    [(3, 1.0), (0, 0.0)],     // lean x: 0.05
    [(3, 1.0), (0, 0.0)],     // lean z: 0.05
    [(3, 1.0), (0, 0.0)],     // lean strength: 0.028 (three cycles: two would leave it nearly flat)
    [(0, 0.0), (0, 0.0)],     // spare
];

fn hash(mut v: u32) -> u32 { v ^= v >> 16; v = v.wrapping_mul(0x7feb352d); v ^= v >> 15; v = v.wrapping_mul(0x846ca68b); v ^ (v >> 16) }

fn lattice(x: u32, z: u32, period: u32, salt: u32) -> f32 {
    (hash(hash(x % period).wrapping_add(z % period).wrapping_mul(0x9e3779b1) ^ salt) & 0xffffff) as f32 / 16777216.0
}

/// Tileable value noise at texel (x, z): `cycles` lattice cells across the texture.
fn value_noise(x: u32, z: u32, cycles: u32, salt: u32) -> f32 {
    let scale = cycles as f32 / SIZE as f32;
    let (fx, fz) = (x as f32 * scale, z as f32 * scale);
    let (ix, iz) = (fx.floor(), fz.floor());
    let (tx, tz) = (fx - ix, fz - iz);
    let (sx, sz) = (tx * tx * (3.0 - 2.0 * tx), tz * tz * (3.0 - 2.0 * tz));
    let at = |dx: u32, dz: u32| lattice(ix as u32 + dx, iz as u32 + dz, cycles, salt);
    let (a, b) = (at(0, 0) + (at(1, 0) - at(0, 0)) * sx, at(0, 1) + (at(1, 1) - at(0, 1)) * sx);
    a + (b - a) * sz
}

/// Two RGBA8 layers, `SIZE` x `SIZE`.
pub(super) fn generate() -> Vec<u8> {
    let mut pixels = vec![0u8; (SIZE * SIZE * 8) as usize];
    for (field, octaves) in FIELDS.iter().enumerate() {
        let (layer, channel) = (field / 4, field % 4);
        for z in 0..SIZE {
            for x in 0..SIZE {
                let value: f32 = octaves.iter().enumerate().filter(|(_, (cycles, _))| *cycles > 0)
                    .map(|(octave, &(cycles, weight))| weight * value_noise(x, z, cycles, (field * 2 + octave) as u32 * 0x1b873593)).sum();
                pixels[(((layer as u32 * SIZE + z) * SIZE + x) * 4) as usize + channel] = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }
    pixels
}

pub(super) fn upload(device: &wgpu::Device, queue: &wgpu::Queue) -> (wgpu::TextureView, wgpu::Sampler) {
    use crate::gpu_stats::{GpuStatsExt, TextureViewStatsExt};
    let size = wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 2 };
    let texture = device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: Some("grass noise fields"), size, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    });
    queue.write_texture(texture.as_image_copy(), &generate(),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: Some(SIZE) }, size);
    let view = texture.create_view_tracked(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default()
    });
    let sampler = device.create_sampler_tracked(&wgpu::SamplerDescriptor {
        label: Some("grass noise repeat"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat, ..Default::default()
    });
    (view, sampler)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_fields_are_deterministic_tileable_and_varied() {
        let pixels = generate();
        assert_eq!(pixels, generate());
        let at = |layer: u32, channel: usize, x: u32, z: u32| i32::from(pixels[(((layer * SIZE + z) * SIZE + x) * 4) as usize + channel]);
        for (layer, channel) in [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1), (1, 2)] {
            let values = (0..SIZE).flat_map(|z| (0..SIZE).map(move |x| (x, z))).map(|(x, z)| at(layer, channel, x, z)).collect::<Vec<_>>();
            assert!(values.iter().max().unwrap() - values.iter().min().unwrap() > 100, "field {layer}/{channel} must vary");
            // The wrap is as smooth as any neighbouring texels: no seam where the texture repeats.
            for i in 0..SIZE {
                assert!((at(layer, channel, 0, i) - at(layer, channel, SIZE - 1, i)).abs() < 60);
                assert!((at(layer, channel, i, 0) - at(layer, channel, i, SIZE - 1)).abs() < 60);
            }
        }
        assert!((0..SIZE).all(|i| at(1, 3, i, i) == 0), "the spare channel stays empty");
    }
}
