//! Which texels of the terrain are green. Enhanced grass grows wherever the ground's own tile picture is green -
//! including the green part of a grass/soil transition tile - and nowhere else, so roads, sand, soil and rock stay
//! bare without a material allow-list. One 64 x 64 coverage mask per distinct ground tile texture, classified from
//! the scene's decoded tile images at load, then uploaded as a texture array the blade shader reads at each root.
use std::collections::HashMap;

pub(super) const SIZE: usize = 64;
/// A tile with less green than this (share of its mask) grows nothing: its cells are not even drawn.
pub(super) const MIN_COVERAGE: f32 = 0.04;

#[derive(Default)]
pub(crate) struct GreenMasks {
    /// `SIZE * SIZE` coverage (0 = bare, 255 = fully green) per layer.
    pub layers: Vec<u8>,
    /// Mean coverage of each layer, 0..1.
    pub coverage: Vec<f32>,
    /// Layer of each ground patch's submesh; `u32::MAX` for a submesh with no usable tile picture.
    pub layer_of: Vec<Vec<u32>>,
}

impl GreenMasks {
    pub fn layer_count(&self) -> u32 { self.coverage.len() as u32 }
    /// The tile layer of a fine cell's submesh when enough of it is green.
    pub fn green_layer(&self, patch: usize, submesh: usize) -> Option<u32> {
        let layer = *self.layer_of.get(patch)?.get(submesh)?;
        (self.coverage.get(layer as usize).copied()? >= MIN_COVERAGE).then_some(layer)
    }
}

/// How green one texel is, 0..1: green leads both red (with a little allowance for olive and dry grass) and blue.
fn greenness(rgb: [u8; 3]) -> f32 {
    let [r, g, b] = rgb.map(f32::from);
    if g < 24.0 { return 0.0; }
    ((g - (r * 0.95).max(b * 1.1)) / 10.0).clamp(0.0, 1.0)
}

fn classify(texture: &ao_scene::Texture) -> [u8; SIZE * SIZE] {
    let (w, h) = (texture.width as usize, texture.height as usize);
    let mut mask = [0u8; SIZE * SIZE];
    if w == 0 || h == 0 || texture.rgba.len() < w * h * 4 { return mask; }
    for (cell, out) in mask.iter_mut().enumerate() {
        let (cx, cy) = (cell % SIZE, cell / SIZE);
        // The texel box of this mask cell (at least one texel, for tiles smaller than the mask).
        let (x0, x1) = (cx * w / SIZE, ((cx + 1) * w / SIZE).max(cx * w / SIZE + 1).min(w));
        let (y0, y1) = (cy * h / SIZE, ((cy + 1) * h / SIZE).max(cy * h / SIZE + 1).min(h));
        let mut sum = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) * 4;
                sum += greenness([texture.rgba[i], texture.rgba[i + 1], texture.rgba[i + 2]]);
            }
        }
        *out = (sum / ((x1 - x0) * (y1 - y0)) as f32 * 255.0 + 0.5) as u8;
    }
    mask
}

/// Masks for every tile texture the scene's fine ground cells use.
pub(crate) fn build(scene: &ao_scene::Scene) -> GreenMasks {
    let mut masks = GreenMasks::default();
    let mut by_key = HashMap::new();
    for patch in &scene.ground_detail {
        let submeshes = scene.meshes.get(patch.mesh).map_or(&[][..], |mesh| &mesh.submeshes[..]);
        let layers = submeshes.iter().map(|submesh| {
            let key = submesh.texture?;
            let texture = scene.textures.get(&key)?;
            Some(*by_key.entry(key).or_insert_with(|| {
                let mask = classify(texture);
                masks.coverage.push(mask.iter().map(|&v| f32::from(v)).sum::<f32>() / (255.0 * mask.len() as f32));
                masks.layers.extend_from_slice(&mask);
                masks.coverage.len() as u32 - 1
            }))
        });
        masks.layer_of.push(layers.map(|layer| layer.unwrap_or(u32::MAX)).collect());
    }
    masks
}

pub(super) fn upload(device: &wgpu::Device, queue: &wgpu::Queue, masks: &GreenMasks) -> wgpu::TextureView {
    use crate::gpu_stats::{GpuStatsExt, TextureViewStatsExt};
    let layers = masks.layer_count().max(1);
    let size = wgpu::Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: layers };
    let texture = device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: Some("grass green masks"), size, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    });
    let empty = vec![0u8; SIZE * SIZE];
    queue.write_texture(texture.as_image_copy(), if masks.layers.is_empty() { &empty } else { &masks.layers },
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE as u32), rows_per_image: Some(SIZE as u32) }, size);
    texture.create_view_tracked(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_scene::{ground_detail::{GroundCell, GroundPatch}, Mesh, Scene, Submesh, Texture, TextureKey, TextureSource};

    fn picture(colour: impl Fn(usize, usize) -> [u8; 3]) -> Texture {
        let rgba = (0..64 * 64).flat_map(|i| { let [r, g, b] = colour(i % 64, i / 64); [r, g, b, 255] }).collect();
        Texture { width: 64, height: 64, rgba, source: TextureSource::default() }
    }

    #[test]
    fn green_texels_are_classified_per_tile_including_transition_tiles() {
        let key = |id| TextureKey { rdb_type: 1_010_006, id };
        let mut scene = Scene::default();
        scene.textures.insert(key(1), picture(|_, _| [60, 104, 40]));            // meadow
        scene.textures.insert(key(2), picture(|_, _| [150, 110, 70]));           // soil
        scene.textures.insert(key(3), picture(|x, _| if x < 32 { [58, 100, 38] } else { [190, 170, 120] })); // grass to sand
        scene.textures.insert(key(4), picture(|_, _| [110, 112, 108]));          // rock
        let submesh = |id| Submesh::new(vec![], Some(key(id)));
        scene.meshes.push(Mesh { vertices: vec![], submeshes: vec![submesh(1), submesh(2), submesh(3), submesh(4), Submesh::new(vec![], None)] });
        scene.ground_detail.push(GroundPatch { mesh: 0, fine_cells: vec![GroundCell::default()], ..Default::default() });
        let masks = build(&scene);
        assert_eq!(masks.layers.len(), 4 * SIZE * SIZE);
        let layer = |submesh| masks.green_layer(0, submesh);
        assert_eq!(layer(0), Some(0));
        assert_eq!((layer(1), layer(3)), (None, None), "soil and rock stay bare");
        assert_eq!(layer(4), None, "a submesh without a picture grows nothing");
        let transition = &masks.layers[SIZE * SIZE * layer(2).unwrap() as usize..][..SIZE * SIZE];
        assert!(transition[10 * SIZE + 5] > 240 && transition[10 * SIZE + SIZE - 5] < 10, "only the green half of a transition tile is masked");
        assert!((masks.coverage[layer(2).unwrap() as usize] - 0.5).abs() < 0.02);
    }
}
