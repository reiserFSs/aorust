//! Project-original leaf silhouettes and subtractive root-tip/fold colors.
//! Array layers keep filtering inside each species, without atlas-edge bleed.
use crate::gpu_stats::{GpuStatsExt, TextureViewStatsExt};

pub(super) const WIDTH: u32 = 64;
pub(super) const HEIGHT: u32 = 128;
pub(super) const SPECIES: u32 = 5;

// Curved tapered leaves share a root but not a silhouette; returns fold position.
fn leaf(x: f32, y: f32, root: f32, lean: f32, height: f32, width: f32) -> Option<f32> {
    let t = y / height;
    if !(0.0..1.0).contains(&t) { return None; }
    let center = root + lean * t * t;
    let half = width * (1.0 - t).sqrt() * (0.35 + 0.65 * (t * std::f32::consts::PI).sin());
    let across = (x - center).abs() / half.max(0.001);
    (across < 1.0).then_some(across)
}

fn surface(species: u32, x: f32, y: f32) -> Option<[f32; 3]> {
    let fold = match species {
        0 => leaf(x, y, -0.08, 0.34, 0.97, 0.11),
        1 => [(-0.09, -0.50, 0.72, 0.095), (0.0, 0.08, 0.98, 0.10), (0.09, 0.48, 0.82, 0.085)]
            .into_iter().filter_map(|(root, lean, height, width)| leaf(x, y, root, lean, height, width)).reduce(f32::min),
        2 => [(-0.04, -0.24, 0.91, 0.23), (0.08, 0.35, 0.75, 0.19)]
            .into_iter().filter_map(|(root, lean, height, width)| leaf(x, y, root, lean, height, width)).reduce(f32::min),
        _ => {
            let stem = (x - 0.08 * y * y).abs() < 0.022 && y < 0.83;
            let leaves = [(0.26, -1.0), (0.43, 1.0), (0.59, -1.0)].into_iter().any(|(level, side)| {
                let u = (x - 0.08 * level * level) * side;
                let v = y - level - u * 0.42;
                u > 0.0 && u < 0.32 && (u / 0.32 * std::f32::consts::PI).sin() * 0.047 > v.abs()
            });
            let dx = x - 0.08 * 0.83 * 0.83;
            let dy = (y - 0.83) * 2.0;
            if species == 3 {
                // Five small petals, deliberately tiny relative to the card.
                let angle = dy.atan2(dx);
                let radius = 0.085 + 0.025 * (angle * 5.0).cos();
                if dx * dx + dy * dy < radius * radius {
                    return Some(if dx * dx + dy * dy < 0.001 { [0.88, 0.74, 0.40] } else { [0.93, 0.86, 0.78] });
                }
            } else if dx.abs() < 0.065 * (1.0 - ((y - 0.82) / 0.13).abs()).max(0.0) {
                return Some([0.69, 0.66, 0.43]);
            }
            (stem || leaves).then_some(0.5)
        }
    }?;
    // Surface coloration is independent of the shader's directional transmission.
    let root_tip = 0.53 + 0.42 * y.sqrt();
    let ridge = 1.0 - 0.12 * fold;
    let species_tint = match species { 0 => [0.94, 1.0, 0.85], 1 => [0.91, 0.98, 0.82], 2 => [0.87, 0.96, 0.91], _ => [0.94, 0.98, 0.84] };
    Some(species_tint.map(|v| v * root_tip * ridge))
}

pub(super) fn generate() -> Vec<u8> {
    let mut pixels = vec![0; (WIDTH * HEIGHT * SPECIES * 4) as usize];
    for species in 0..SPECIES {
        for row in 0..HEIGHT {
            for column in 0..WIDTH {
                let mut rgb = [0.0; 3];
                let mut covered = 0;
                for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let x = ((column as f32 + dx) / WIDTH as f32 - 0.5) * 2.0;
                    let y = 1.0 - (row as f32 + dy) / HEIGHT as f32;
                    if let Some(color) = surface(species, x, y) {
                        for channel in 0..3 { rgb[channel] += color[channel]; }
                        covered += 1;
                    }
                }
                let offset = (((species * HEIGHT + row) * WIDTH + column) * 4) as usize;
                if covered > 0 {
                    for channel in 0..3 { pixels[offset + channel] = (rgb[channel] / covered as f32 * 255.0).round() as u8; }
                    pixels[offset + 3] = (covered * 255 / 4) as u8;
                }
            }
        }
    }
    pixels
}

pub(super) fn upload(device: &wgpu::Device, queue: &wgpu::Queue) -> (wgpu::TextureView, wgpu::Sampler) {
    let size = wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: SPECIES };
    let texture = device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: Some("original procedural grass species"), size, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    });
    queue.write_texture(texture.as_image_copy(), &generate(),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(WIDTH * 4), rows_per_image: Some(HEIGHT) }, size);
    let view = texture.create_view_tracked(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default()
    });
    let sampler = device.create_sampler_tracked(&wgpu::SamplerDescriptor {
        label: Some("grass atlas linear clamp"), mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear, ..Default::default()
    });
    (view, sampler)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_atlas_is_deterministic_diverse_and_shaded() {
        let pixels = generate();
        assert_eq!(pixels, generate());
        assert_eq!(pixels.len(), (WIDTH * HEIGHT * SPECIES * 4) as usize);
        let layers = pixels.as_chunks::<{ (WIDTH * HEIGHT * 4) as usize }>().0;
        for (species, layer) in layers.iter().enumerate() {
            let texels = layer.as_chunks::<4>().0;
            let solid = texels.iter().filter(|p| p[3] >= 128).count();
            assert!(solid > 150 && solid < (WIDTH * HEIGHT / 2) as usize);
            assert!(texels.iter().any(|p| p[3] == 0));
            assert!(texels.iter().any(|p| p[3] > 0 && p[3] < 255));
            assert!(texels.iter().filter(|p| p[3] > 0).all(|p| p[..3].iter().all(|&v| v > 0 && v <= 242)));
            for other in &layers[..species] {
                assert_ne!(texels.iter().map(|p| p[3]).collect::<Vec<_>>(), other.as_chunks::<4>().0.iter().map(|p| p[3]).collect::<Vec<_>>());
            }
        }
        let root = surface(0, -0.08, 0.02).unwrap();
        let tip = surface(0, -0.08 + 0.34 * (0.8_f32 / 0.97).powi(2), 0.8).unwrap();
        assert!(tip[1] > root[1]);
        let center = surface(0, -0.08 + 0.34 * (0.5_f32 / 0.97).powi(2), 0.5).unwrap();
        let edge = surface(0, -0.08 + 0.34 * (0.5_f32 / 0.97).powi(2) + 0.04, 0.5).unwrap();
        assert_ne!(center, edge, "fold shading must survive rasterization");
        assert_ne!(surface(3, 0.055, 0.83), surface(4, 0.055, 0.83));
    }
}
