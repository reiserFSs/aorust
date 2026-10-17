//! N3 AnarchyGround_t preference callback 100320b2 and draw selection 10034dab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundPath { Fused, ShadowsTwoPass, Simple }
#[derive(Clone, Copy, Debug)]
pub struct NativeGround { mode: i32, enabled: bool, shadows: bool }
impl Default for NativeGround {
    fn default() -> Self { Self { mode: 3, enabled: true, shadows: false } }
}
impl NativeGround {
    pub fn set(&mut self, mode: i32) {
        let mode = mode.clamp(0, 3);
        if self.mode == mode { return; }
        self.mode = mode;
        match mode {
            0 => self.enabled = false,
            1 => { self.enabled = true; self.shadows = true; }
            _ => { self.enabled = true; self.shadows = false; }
        }
    }
    pub fn recreate(&mut self) {
        let mode = self.mode;
        *self = Self::default();
        self.set(mode);
    }
    pub fn path(self) -> GroundPath {
        if !self.shadows { GroundPath::Fused }
        else if self.enabled { GroundPath::ShadowsTwoPass }
        else { GroundPath::Simple }
    }
}

/// Terrain-only lanes: native palette scale and world-to-patch sky openness UVs.
pub(super) fn material_uniform(s: &ao_scene::Submesh, patch: &ao_scene::ground_detail::GroundPatch) -> [f32; 32] {
    let mut u = crate::mat_uniform(s);
    u[12] = patch.shadow_uv_scale[0];
    u[14] = patch.shadow_uv_scale[1];
    let scale = 1.0 / (patch.cells as f32 * patch.cell_size);
    u[24..28].copy_from_slice(&[scale, scale, -patch.origin[0] as f32 / patch.cells as f32, -patch.origin[1] as f32 / patch.cells as f32]);
    u
}

/// Baked sky openness at the fine lattice nodes, independent of selected ROAM triangles.
/// Alpha stores occlusion; invert it after the bake closes every roofed-cell corner.
pub(super) fn sky_openness(patch: &ao_scene::ground_detail::GroundPatch, vertices: &[ao_scene::Vertex]) -> Vec<u8> {
    let start = patch.lattice_start as usize;
    vertices[start..start + (patch.cells + 1).pow(2)].iter()
        .map(|v| ((1.0 - v.color[3].clamp(0.0, 1.0)) * 255.0).round() as u8).collect()
}
/// CPU recipe only; geometry/actor buffers do not change on a ground-family refresh.
pub(super) struct Composite {
    pub(super) key: ao_scene::TextureKey,
    source: ao_scene::TextureSource,
    cells: usize,
    tiles: Vec<Option<(ao_scene::TextureKey, u16)>>,
}

pub(super) fn composites(scene: &ao_scene::Scene) -> Vec<Composite> {
    use ao_scene::TextureSource;
    scene.ground_detail.iter().filter_map(|patch| {
        let mesh = scene.meshes.get(patch.mesh)?;
        let key = mesh.submeshes.get(patch.coarse_submesh)?.texture?;
        let source = scene.textures.get(&key)?.source;
        if !matches!(source, TextureSource::GroundComposite565 | TextureSource::GroundComposite1555) { return None; }
        let tiles = patch.fine_cells.iter().map(|cell| {
            let key = mesh.submeshes.get(cell.submesh)?.texture?;
            let uv = mesh.vertices.get(cell.vertices[0] as usize)?.uv;
            let orientation = match (uv[0] >= 0.5, uv[1] >= 0.5) {
                (false, true) => 0, (true, true) => 1, (true, false) => 2, (false, false) => 3,
            };
            Some((key, orientation))
        }).collect();
        Some(Composite { key, source, cells: patch.cells, tiles })
    }).collect()
}

pub(super) fn composite_images(recipes: &[Composite], textures: &std::collections::HashMap<ao_scene::TextureKey, ao_scene::Texture>, mips: &std::collections::HashMap<ao_scene::TextureKey, &[[u8; 4]]>) -> std::collections::HashMap<ao_scene::TextureKey, ao_scene::Texture> {
    use ao_scene::{ground_texture, Texture};
    let mut images = std::collections::HashMap::new();
    for recipe in recipes {
        let source = textures.get(&recipe.key).map_or(recipe.source, |texture| texture.source);
        let side = recipe.cells * 8;
        let mut rgba = vec![255; side * side * 4];
        for (cell, tile) in recipe.tiles.iter().enumerate() {
            let Some((key, orientation)) = tile else { continue; };
            let Some(mip) = mips.get(key).or_else(|| mips.iter().find(|(k, _)| k.id == key.id && matches!(k.rdb_type, 1_010_006 | 1_010_021 | 1_010_022)).map(|(_, mip)| mip)) else { continue; };
            for y in 0..8 { for x in 0..8 {
                let dst = (((cell / recipe.cells * 8 + y) * side) + cell % recipe.cells * 8 + x) * 4;
                rgba[dst..dst + 4].copy_from_slice(&mip[ground_texture::oriented_pixel(*orientation, x, y)]);
            }}
        }
        images.insert(recipe.key, Texture { source, width: side as u32, height: side as u32, rgba });
    }
    images
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sky_openness_uses_baked_lattice_and_arbitrary_patch_mapping() {
        use ao_scene::{ground_detail::GroundPatch, Submesh, Vertex};
        let patch = GroundPatch {
            origin: [-7, 11], cells: 2, cell_size: 2.5, lattice_start: 4,
            shadow_uv_scale: [0.125, 0.25], ..Default::default()
        };
        // Fine tile duplicates intentionally disagree: only the shared lattice is authoritative.
        let mut vertices = vec![Vertex { color: [1.0, 1.0, 1.0, 0.0], ..Default::default() }; 4];
        let alpha = [1.0, 1.0, 0.5, 1.0, 1.0, 0.0, 0.75, 0.25, 0.0];
        vertices.extend(alpha.map(|a| Vertex { color: [1.0, 1.0, 1.0, a], ..Default::default() }));
        let mask = sky_openness(&patch, &vertices);
        assert_eq!(mask, [0, 0, 128, 0, 0, 255, 64, 191, 255]);
        assert!([mask[0], mask[1], mask[3], mask[4]].iter().all(|&a| a == 0),
            "all bilinear contributions to a closed cell remain closed");
        let u = material_uniform(&Submesh::new(vec![], None), &patch);
        assert_eq!([u[12], u[14]], patch.shadow_uv_scale);
        for (world, expected) in [([-17.5, 27.5], [0.0, 0.0]), ([-12.5, 32.5], [1.0, 1.0])] {
            let uv = [world[0] * u[24] + u[26], world[1] * u[25] + u[27]];
            assert!(uv.into_iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-6));
        }
    }

    #[test]
    fn ground_composites_reuse_policy_mips_across_orientations_and_family_refresh() {
        use ao_scene::{TextureKey, TextureSource};
        use crate::native_texture::{Policy, build_texture_mips};
        let tile = TextureKey { rdb_type: 1_010_006, id: 17 };
        let refreshed = TextureKey { rdb_type: 1_010_022, ..tile };
        let key = TextureKey { rdb_type: 0, id: 4 };
        let recipes = [Composite { key, source: TextureSource::GroundComposite1555, cells: 2,
            tiles: (0..4).map(|orientation| Some((tile, orientation))).collect() }];
        let rgba: Vec<u8> = (0..32 * 32).flat_map(|_| [128, 128, 128, 255]).collect();
        for (quality, expected) in [(0, 140), (2, 123)] {
            let policy = Policy { quality, compressed: true, better: false };
            let built = build_texture_mips(policy, TextureSource::GroundRgba, true, &rgba, 32, 32, false);
            let mips = [(refreshed, built.ground_tile_mip().unwrap())].into_iter().collect();
            let images = composite_images(&recipes, &Default::default(), &mips);
            let image = &images[&key];
            let coarse = build_texture_mips(policy, image.source, true, &image.rgba, image.width, image.height, false);
            assert_eq!(coarse.mips, 1);
            assert!(coarse.levels[0].bytes.as_chunks::<4>().0.iter().all(|pixel| *pixel == [expected, expected, expected, 255]));
        }
        let pixels: Vec<[u8; 4]> = (0..64).map(|i| [i, i, i, 255]).collect();
        let mips = [(tile, pixels.as_slice())].into_iter().collect();
        let image = &composite_images(&recipes, &Default::default(), &mips)[&key];
        for orientation in 0..4 { for y in 0..8 { for x in 0..8 {
            let dst = (((orientation / 2 * 8 + y) * 16) + orientation % 2 * 8 + x) * 4;
            assert_eq!(image.rgba[dst], ao_scene::ground_texture::oriented_pixel(orientation as u16, x, y) as u8);
        }}}
        assert_eq!(image.source, TextureSource::GroundComposite1555);
    }

    #[test]
    fn native_mode_histories() {
        for initial in 0..=3 {
            let mut s = NativeGround::default(); s.set(initial);
            assert_eq!(s.path(), if initial == 1 { GroundPath::ShadowsTwoPass } else { GroundPath::Fused });
        }
        let mut s = NativeGround::default();
        for (mode, path) in [(1,GroundPath::ShadowsTwoPass),(0,GroundPath::Simple),(0,GroundPath::Simple),(2,GroundPath::Fused),(0,GroundPath::Fused),(1,GroundPath::ShadowsTwoPass),(3,GroundPath::Fused)] {
            s.set(mode); assert_eq!(s.path(),path);
        }
        s.set(1); s.set(0); s.recreate();
        assert_eq!(s.path(), GroundPath::Fused, "new native ground object fires the saved mode from clean flags");
    }
    #[test]
    #[ignore = "Requires a GPU; native ground stage output regression"]
    fn native_ground_gpu_mode_history() {
        use ao_scene::{ground_detail::{GroundCell, GroundPatch}, Environment, Instance, Mesh, Scene, Submesh, Texture, TextureKey, Vertex, IDENTITY};
        use crate::{Camera, Renderer, Targets, Vec3};
        let tile = TextureKey { rdb_type: 1_010_006, id: 1 };
        let palette = TextureKey { rdb_type: 1_000_007, id: 2 };
        let composite = TextureKey { rdb_type: 0, id: 3 };
        let mut scene = Scene { environment: Some(Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 100.0, fog_end: 200.0, ambient: [0.2f32.powf(2.2); 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0 }), ..Default::default() };
        for (key, value) in [(tile, 204), (palette, 64), (composite, 204)] {
            scene.textures.insert(key, Texture { width: 1, height: 1, rgba: vec![value, value, value, 255], source: Default::default() });
        }
        let vertices = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, -1.0]].into_iter().map(|pos| Vertex { pos, normal: [0.0, 1.0, 0.0], uv: [0.5; 2], color: [1.0; 4] }).collect();
        let mut fine = Submesh::new(vec![0, 1, 2, 1, 3, 2], Some(tile));
        fine.texture_clamp = true;
        let mut coarse = Submesh::new(vec![0; 6], Some(composite));
        coarse.texture_clamp = true;
        scene.meshes.push(Mesh { vertices, submeshes: vec![fine, coarse] });
        scene.instances.push(Instance { mesh: 0, transform: IDENTITY });
        scene.ground_detail.push(GroundPatch { mesh: 0, cells: 1, cell_size: 1.0, heights: vec![0; 4], coarse_submesh: 1, fine_cells: vec![GroundCell { submesh: 0, vertices: [0, 1, 2, 3], ..Default::default() }], shadow_texture: Some(palette), shadow_uv_scale: [1.0; 2], ..Default::default() });
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).unwrap();
        renderer.upload(&scene);
        let targets = Targets::new(&renderer, 32, 32);
        let out = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let camera = Camera::look_at(Vec3::new(0.5, 1.0, -0.5), Vec3::new(0.5, 0.0, -0.5));
        let path = std::env::temp_dir().join(format!("ao-ground-history-{}.png", std::process::id()));
        for fog in [false, true] {
            renderer.set_ground_rendering(3);
            renderer.env.fog_start = if fog { 0.0 } else { 100.0 };
            renderer.env.fog_end = if fog { 2.0 } else { 200.0 };
            renderer.env.fog_color = [0.6f32.powf(2.2); 3];
        for (mode, expected_path, expected_pixel, calls) in [(0, GroundPath::Fused, 51, 1), (1, GroundPath::ShadowsTwoPass, 51, 2), (0, GroundPath::Simple, 41, 1), (2, GroundPath::Fused, 51, 1), (3, GroundPath::Fused, 51, 1)] {
            renderer.set_ground_rendering(mode);
            let mut expected_pixel = if fog { match expected_path { GroundPath::Fused => 102, GroundPath::ShadowsTwoPass => 112, GroundPath::Simple => 97 } } else { expected_pixel };
            if mode == 2 {
                let textures = [(TextureKey { rdb_type: 1_010_022, id: 1 }, Texture { width: 1, height: 1, rgba: vec![102,102,102,255], source: Default::default() })].into_iter().collect();
                renderer.refresh_ground_textures(&textures);
                expected_pixel = if fog { 89 } else { 26 };
            } else if mode == 3 { renderer.refresh_ground_textures(&scene.textures); }
            renderer.render(&out.create_view(&Default::default()), &targets, &camera);
            assert_eq!(renderer.ground_rendering_path(), expected_path);
            assert_eq!(renderer.stats.draw_calls, calls);
            crate::texture_to_png(&renderer, &out, 32, 32, &path).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
            let mut pixels = vec![0; reader.output_buffer_size()];
            let info = reader.next_frame(&mut pixels).unwrap();
            let pixel = pixels[(16 * 32 + 16) * (info.color_type.samples())] as i32;
            assert!((pixel - expected_pixel).abs() <= 2, "mode {mode}: {pixel} != {expected_pixel}");
        }
        }
    }
}
