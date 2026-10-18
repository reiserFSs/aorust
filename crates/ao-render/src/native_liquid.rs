//! DisplaySystem water draw1003b5e9, acid1003bebf, shallow1003b411.
//! The option changes camera-cell geometry, not the presence of a liquid.
use ao_scene::{liquid::Surface, Vertex};
use super::{Renderer, Vec3};
use crate::gpu_stats::GpuStatsExt;

// The two native passes follow the final ordinary/sky pipeline, not the native blend block.
pub(super) const PIPE: usize = super::SKY_LAST_PIPE + 1;

fn passes(style: u32) -> usize { if style == 1 { 3 } else if style == 4 { 1 } else { 2 } }
fn pass_pipe(style: u32, pass: usize) -> usize {
    PIPE + match style { 1 => 2 + pass, 3 => 5 + pass, _ => pass }
}

// DS1003c509 writes byte26 (red), not byte27 (alpha), of FVF0x152.
fn lava_color(vertex: &Vertex, wave: &[f32; 16]) -> [f32; 4] {
    let selector = (f64::from(vertex.pos[0] - vertex.pos[2]) - 0.49999).round_ties_even() as i64 as usize & 7;
    let red = 255 - ((wave[selector] * 255.0) as u32 >> 2);
    [(red as f32 / 255.0).powf(2.2), vertex.color[1], vertex.color[2], vertex.color[3]]
}

pub(super) struct LiquidRun {
    pub source: Surface,
    pub(super) heat_spheres: Vec<[f32; 4]>,
    cell: Option<([i32; 2], bool)>,
    pub(super) vertices: Vec<Vertex>,
    pub(super) indices: Vec<u32>,
    colors: Vec<u32>,
    bytes: Vec<f32>,
    gpu_indices: Vec<u32>,
    vertex_capacity: u64,
    index_capacity: u64,
}
impl LiquidRun {
    pub fn new(source: Surface) -> Self {
        Self { source, heat_spheres: vec![], cell: None, vertices: vec![], indices: vec![], colors: vec![], bytes: vec![], gpu_indices: vec![], vertex_capacity: 0, index_capacity: 0 }
    }
}

// The two native accumulators are draw-count based (not dt based). Keep them
// independent of texture-scroll time, including when screenshots seek the clock.
fn next_edge_phase(phase: &mut f32, style: u32) {
    *phase += if matches!(style, 1 | 2) { 0.03 } else { 0.003 };
    if *phase > 1000.0 { *phase = 0.0; }
}

fn layer_uv(v: &Vertex, index: usize, style: u32, pass: usize, time: f32, edge: f32, wave: &[f32; 16]) -> [f32; 2] {
    let [x, _, z] = v.pos;
    let [nx, ny, nz] = v.normal;
    let scale = if style == 1 { 8.0 } else if style == 2 { 12.0 } else if style == 4 { 160.0 } else { 40.0 };
    let base = [x / scale, -z / scale];
    if style == 1 {
        if pass == 0 {
            return [base[0] + 0.36 * (wave[index & 7] + wave[0]), base[1] + 0.36 * (wave[(index + 1) & 7] + wave[1])];
        }
        if pass == 2 { return base; } // crust uses camera-position texgen in the shader
        if ny < 0.9999 {
            let n = Vec3::new(nx * 1000.0, ny, -nz * 1000.0).normalize();
            return [base[0] - edge * n.x + 0.36 * wave[0], base[1] - edge * n.z + 0.36 * wave[1]];
        }
        return [base[0] + 0.36 * wave[0], base[1] + 0.36 * wave[1]];
    }
    if style == 3 { return base; }
    if style == 2 {
        // Native integer oscillator table +0x50, divided by4096; identity
        // matrix diagonal100aec48/4c, off-diagonals1015017c/80 start0.
        if pass == 0 {
            return [base[0] + ((wave[index & 7] * 255.0) as u32 + (wave[0] * 255.0) as u32) as f32 / 4096.0,
                base[1] + ((wave[(index + 1) & 7] * 255.0) as u32 + (wave[1] * 255.0) as u32) as f32 / 4096.0];
        }
        let direction = |n: f32| if n > 0.2 { 1.0 } else if n < -0.2 { -1.0 } else { 0.0 };
        return if ny < 0.9 { [base[0] - edge * direction(nx), base[1] - edge * direction(-nz)] } else { base };
    }
    let common = [0.036 * wave[0], 0.036 * wave[1]];
    if style == 4 {
        return [base[0] + common[0] + 0.036 * wave[index & 7] * 0.25,
            base[1] + common[1] + 0.036 * wave[(index + 1) & 7] * 0.25];
    }
    if pass == 0 {
        let scroll = (-time * 0.01).rem_euclid(1.0);
        return [base[0] + common[0] + 0.036 * wave[index & 7] + scroll,
            base[1] + common[1] + 0.036 * wave[(index + 1) & 7] - scroll];
    }
    let mut uv = [base[0] + common[0], base[1] + common[1]];
    if ny < 0.9999 {
        //10005229→100051f8→100048e8 normalizes(nx*1000,ny,nz*1000).
        let n = Vec3::new(nx * 1000.0, ny, -nz * 1000.0).normalize();
        uv[0] -= edge * n.x;
        uv[1] -= edge * n.z;
    }
    uv
}

impl Renderer {
    /// DS1003b270 stores the inverse bool and invalidates the camera cell.
    pub fn set_realistic_water(&mut self, enabled: bool) { self.realistic_water = enabled; }

    pub(super) fn update_liquids(&mut self, camera: Vec3) {
        let wave = ao_scene::wave_curves(self.time);
        for run in &mut self.liquids {
            let cell = (Surface::cell(camera.to_array()), self.realistic_water);
            let changed = run.cell.is_none_or(|previous| previous.1 != cell.1 || (self.realistic_water && previous.0 != cell.0));
            if changed {
                run.source.heat_spheres(camera.to_array(), self.realistic_water, &mut run.heat_spheres);
                run.source.geometry(camera.to_array(), self.realistic_water, &mut run.vertices, &mut run.indices, &mut run.colors);
                if !run.vertices.is_empty() {
                    let (low, high) = run.vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(low, high), vertex| {
                        let position = Vec3::from_array(vertex.pos);
                        (low.min(position), high.max(position))
                    });
                    let center = (low + high) * 0.5;
                    let radius = run.vertices.iter().map(|vertex| (Vec3::from_array(vertex.pos) - center).length()).fold(0.0, f32::max);
                    for instance in &mut self.gpu.insts[self.gpu.mesh_range[run.source.mesh].clone()] {
                        let transform = glam::Mat4::from_cols_array_2d(&instance.m);
                        instance.center = transform.transform_point3(center);
                        instance.radius = radius * transform.x_axis.truncate().length().max(transform.y_axis.truncate().length()).max(transform.z_axis.truncate().length());
                    }
                }
                run.cell = Some(cell);
                run.gpu_indices.clear();
                let base = run.vertices.len() as u32;
                for pass in 0..passes(run.source.style) {
                    run.gpu_indices.extend(run.indices.iter().map(|i| i + base * pass as u32));
                }
                for (pass, draw) in self.gpu.liquid.iter_mut().filter(|draw| draw.mesh == run.source.mesh).enumerate() {
                    draw.first_index = (pass * run.indices.len()) as u32;
                    draw.count = run.indices.len() as u32;
                    draw.pipe = pass_pipe(run.source.style, pass);
                }
            }
            if run.indices.is_empty() { continue; }
            let edge = &mut self.liquid_edge[match run.source.style { 1 => 2, 2 => 1, _ => 0 }];
            if !matches!(run.source.style, 3 | 4) { next_edge_phase(edge, run.source.style); }
            // Visibility is from the *previous* frame's frustum cull (this frame's isn't computed yet when
            // `update_liquids` runs, before `render`'s cull pass): close enough to skip the per-vertex UV pass
            // (the actual per-frame cost here, `layer_uv`'s wave/trig math times every vertex times up to 2
            // passes) for liquids that stayed off-screen. Always regenerate on a cell change regardless, so the
            // GPU buffer and `run.bytes`/`run.gpu_indices` never fall out of sync with each other.
            let visible = self.vis_range.get(run.source.mesh).is_none_or(|r| !r.is_empty());
            if !changed && !visible { continue; }
            run.bytes.clear();
            for pass in 0..passes(run.source.style) {
                for (i, vertex) in run.vertices.iter().enumerate() {
                    let uv = layer_uv(vertex, i, run.source.style, pass, self.time, *edge, &wave);
                    run.bytes.extend_from_slice(&vertex.pos);
                    run.bytes.extend_from_slice(&vertex.normal);
                    run.bytes.extend_from_slice(&uv);
                    let color = if run.source.style == 1 { lava_color(vertex, &wave) } else { vertex.color };
                    run.bytes.extend_from_slice(&color);
                }
            }
            let vertex_size = (run.bytes.len() * 4) as u64;
            let index_size = (run.gpu_indices.len() * 4) as u64;
            if vertex_size > run.vertex_capacity || index_size > run.index_capacity {
                run.vertex_capacity = vertex_size.next_power_of_two();
                run.index_capacity = index_size.next_power_of_two();
                let buffer = |label, size, usage| self.device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
                self.gpu.meshes[run.source.mesh] = Some((
                    buffer("native liquid vertices", run.vertex_capacity, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST),
                    buffer("native liquid indices", run.index_capacity, wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST),
                ));
            }
            if let Some((vertices, indices)) = &self.gpu.meshes[run.source.mesh] {
                self.queue.write_buffer(vertices, 0, bytemuck::cast_slice(&run.bytes));
                if changed { self.queue.write_buffer(indices, 0, bytemuck::cast_slice(&run.gpu_indices)); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lava_three_passes_preserve_native_color_channel_and_uvs() {
        let v = Vertex { pos: [8.0, 0.0, -16.0], color: [1.0, 0.0, 0.0, 0.25], ..Default::default() };
        let wave = [1.0; 16];
        assert_eq!(passes(1), 3);
        assert_eq!((0..3).map(|p| pass_pipe(1, p)).collect::<Vec<_>>(), vec![PIPE + 2, PIPE + 3, PIPE + 4]);
        let color = lava_color(&v, &wave);
        assert!((color[0] - (192.0f32 / 255.0).powf(2.2)).abs() < 1e-6);
        assert_eq!(color[3], 0.25, "draw animation changes red, never geometry alpha");
        let close = |a: [f32; 2], b: [f32; 2]| assert!(a.into_iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6));
        close(layer_uv(&v, 0, 1, 0, 0.0, 0.03, &wave), [1.72, 2.72]);
        close(layer_uv(&v, 0, 1, 1, 0.0, 0.03, &wave), [1.36, 2.36]);
        let mut phase = 0.0;
        next_edge_phase(&mut phase, 1);
        assert_eq!(phase, 0.03);
    }

    #[test]
    fn native_lava_crust_multiplies_both_texture_alphas_offscreen() {
        use ao_scene::{liquid::SourceTriangle, Blend, Instance, Mesh, Scene, Submesh, Texture, TextureKey, TextureSource, IDENTITY};
        let positions = [[100.0, 0.0, -100.0], [140.0, 0.0, -100.0], [140.0, 0.0, -140.0], [100.0, 0.0, -140.0]];
        let source = Surface::new(0, 1, [[0, 1, 2], [0, 2, 3]].map(|triangle| SourceTriangle {
            kind: 2, vertices: triangle.map(|index| Vertex { pos: positions[index], ..Default::default() }),
        }).into()).unwrap();
        let (mut vertices, mut indices, mut colors) = (vec![], vec![], vec![]);
        source.geometry([120.0, 10.0, -120.0], false, &mut vertices, &mut indices, &mut colors);
        let count = vertices.len() as u32;
        let water = TextureKey { rdb_type: 0, id: 1 };
        let crust = TextureKey { rdb_type: 0, id: 2 };
        let mut mesh = Mesh { vertices, submeshes: vec![] };
        for (pass, texture) in [water, water, crust].into_iter().enumerate() {
            if pass != 0 { mesh.vertices.extend_from_within(..count as usize); }
            let mut sub = Submesh::new(indices.iter().map(|index| index + count * pass as u32).collect(), Some(texture));
            sub.liquid = true;
            sub.two_sided = true;
            sub.blend = Blend::AlphaBlend;
            mesh.submeshes.push(sub);
        }
        let mut scene = Scene { meshes: vec![mesh], instances: vec![Instance { mesh: 0, transform: IDENTITY }], liquids: vec![source], ..Default::default() };
        for (key, rgba) in [(water, vec![255, 255, 255, 255]), (crust, vec![0, 255, 0, 128])] {
            scene.textures.insert(key, Texture { width: 1, height: 1, rgba, source: TextureSource::PngRgba });
        }
        let path = std::env::temp_dir().join(format!("aomac-native-lava-{}.png", std::process::id()));
        super::super::render_to_png(&scene, [120.0, 10.0, -120.0], [120.0, 0.0, -120.1], 64, 64, &path).unwrap();
        let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap())).read_info().unwrap();
        let mut pixels = vec![0; decoder.output_buffer_size()];
        decoder.next_frame(&mut pixels).unwrap();
        std::fs::remove_file(path).unwrap();
        let pixel = &pixels[(32 * 64 + 32) * 4..][..3];
        assert!((55..75).contains(&pixel[1]) && pixel[0] > 100 && pixel[2] < 5, "{pixel:?}: crust alpha must be (128/255)^2, not a mean .84");
    }

    #[test]
    fn native_authored_water_layers_and_draw_clock() {
        let close = |actual: [f32; 2], expected: [f32; 2]| assert!(actual.into_iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-6), "{actual:?} != {expected:?}");
        let vertex = Vertex { pos: [40.0, 5.0, -80.0], normal: [0.6, 0.8, 0.0], ..Default::default() };
        let wave = ao_scene::wave_curves(0.0);
        let mut edge = 0.0;
        next_edge_phase(&mut edge, 0);
        assert_eq!(edge, 0.003);
        let first = layer_uv(&vertex, 0, 0, 0, 0.0, edge, &wave);
        close(first, [1.0 + 0.072 * wave[0], 2.0 + 0.072 * wave[1]]);
        let second = layer_uv(&vertex, 0, 0, 1, 0.0, edge, &wave);
        let normal = Vec3::new(600.0, 0.8, 0.0).normalize();
        close(second, [1.0 + 0.036 * wave[0] - 0.003 * normal.x, 2.0 + 0.036 * wave[1]]);
        assert_ne!(first, second);
        assert_ne!(first, layer_uv(&vertex, 0, 0, 0, 1.0, edge, &wave));
        let flat = Vertex { normal: [0.0, 1.0, 0.0], ..vertex };
        assert_eq!(layer_uv(&flat, 0, 0, 1, 0.0, edge, &wave), [1.0 + 0.036 * wave[0], 2.0 + 0.036 * wave[1]]);
        edge = 1000.0;
        next_edge_phase(&mut edge, 0);
        assert_eq!(edge, 0.0);
        next_edge_phase(&mut edge, 2);
        assert_eq!(edge, 0.03);
        let shallow = layer_uv(&flat, 0, 4, 0, 5.0, edge, &wave);
        close(shallow, [0.25 + 0.045 * wave[0], 0.5 + 0.045 * wave[1]]);
    }

    #[test]
    fn native_water_passes_blend_over_sky_write_depth_and_redraw_coplanar() {
        use ao_scene::{liquid::SourceTriangle, Instance, Mesh, Scene, Submesh, IDENTITY};
        let vertices: Vec<_> = [[-20.0, 0.0, -20.0], [20.0, 0.0, -20.0], [20.0, 0.0, 20.0], [-20.0, 0.0, 20.0]]
            .map(|pos| Vertex { pos, normal: [0.0, 1.0, 0.0], ..Default::default() }).into();
        let indices = vec![0, 1, 2, 0, 2, 3];
        let source = Surface::new(0, 0, indices.as_chunks::<3>().0.iter().map(|triangle| SourceTriangle {
            kind: 0x80ff_ffff, vertices: std::array::from_fn(|i| vertices[triangle[i] as usize]),
        }).collect()).unwrap();
        let first = Submesh { liquid: true, two_sided: true, blend: ao_scene::Blend::AlphaBlend,
            base_color: [0.0, 0.0, 1.0, 1.0], emissive: [1.0; 3], ..Submesh::new(indices.clone(), None) };
        let mut second = first.clone();
        second.indices.iter_mut().for_each(|index| *index += 4);
        let mut water_vertices = vertices.clone();
        water_vertices.extend_from_within(..);
        let floor = Submesh { two_sided: true, base_color: [1.0, 0.0, 0.0, 1.0],
            emissive: [1.0; 3], ..Submesh::new(indices, None) };
        let scene = Scene {
            meshes: vec![
                Mesh { vertices: water_vertices, submeshes: vec![first, second] },
                Mesh { vertices: vertices.into_iter().map(|mut vertex| { vertex.pos[1] = -1.0; vertex }).collect(), submeshes: vec![floor] },
            ],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }, Instance { mesh: 1, transform: IDENTITY }],
            liquids: vec![source], ..Default::default()
        };
        let shot = |scene: &Scene| {
            let path = std::env::temp_dir().join(format!("aomac-native-water-{}.png", std::process::id()));
            super::super::render_to_png(scene, [0.0, 10.0, 0.0], [0.0, 0.0, -0.1], 64, 64, &path).unwrap();
            let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap())).read_info().unwrap();
            let mut pixels = vec![0; decoder.output_buffer_size()];
            decoder.next_frame(&mut pixels).unwrap();
            std::fs::remove_file(path).unwrap();
            pixels
        };
        let pixels = shot(&scene);
        let pixel = &pixels[(32 * 64 + 32) * 4..][..3];
        // Two half-alpha passes: red -> (128,0,128) -> (64,0,192), in D3D7 gamma bytes.
        // The stale slot uses SRCCOLOR/ONE, leaving red at 255 instead.
        assert!((55..75).contains(&pixel[0]) && pixel[1] < 5 && (180..205).contains(&pixel[2]), "{pixel:?}");
        // The second blue pass is coplanar: the 192 blue above also proves LESSEQUAL,
        // rather than LESS rejecting it after the first pass writes depth.
        let mut sky = scene.clone();
        sky.instances.pop();
        sky.sky.push(Instance { mesh: 1, transform: IDENTITY });
        assert_eq!(&shot(&sky)[(32 * 64 + 32) * 4..][..3], pixel, "water must blend over the earlier sky without sky depth occlusion");
        let mut behind = scene.clone();
        let mut overlay = behind.meshes[1].clone();
        overlay.submeshes[0].blend = ao_scene::Blend::AlphaBlend;
        overlay.submeshes[0].base_color = [0.0, 1.0, 0.0, 1.0];
        behind.meshes.push(overlay);
        behind.instances.push(Instance { mesh: 2, transform: IDENTITY });
        assert_eq!(shot(&behind), pixels, "native water writes depth even while blending, occluding later below-water blends");
        for vertex in &mut behind.meshes[2].vertices { vertex.pos[1] = 1.0; }
        assert_ne!(shot(&behind), pixels, "the same later blend above water must remain visible");
    }

    #[test]
    fn native_liquid_runtime_rebuilds_real_geometry_and_preserves_disabled_cell() {
        use ao_scene::{liquid::SourceTriangle, Instance, Mesh, Scene, Submesh, IDENTITY};
        let source = Surface::new(0, 0, vec![SourceTriangle { kind: 0, vertices: [
            Vertex { pos: [200.0, 5.0, -200.0], ..Default::default() },
            Vertex { pos: [260.0, 5.0, -200.0], ..Default::default() },
            Vertex { pos: [200.0, 5.0, -260.0], ..Default::default() },
        ] }]).unwrap();
        let camera = source.initial_camera().unwrap();
        let (mut vertices, mut indices, mut colors) = (vec![], vec![], vec![]);
        source.geometry(camera, false, &mut vertices, &mut indices, &mut colors);
        let count = vertices.len() as u32;
        vertices.extend_from_within(..);
        let mut first = Submesh::new(indices.clone(), None);
        first.liquid = true;
        first.two_sided = true;
        first.blend = ao_scene::Blend::AlphaBlend;
        let mut second = first.clone();
        second.indices.iter_mut().for_each(|index| *index += count);
        let mut scene = Scene { water_light: Some(([0.1, 0.2, 0.3], [0.0, 1.0, 0.0])), ..Default::default() };
        scene.meshes.push(Mesh { vertices, submeshes: vec![first, second] });
        scene.instances.push(Instance { mesh: 0, transform: IDENTITY });
        scene.liquids.push(source);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).unwrap();
        renderer.upload(&scene);
        assert_eq!(renderer.water_light, scene.water_light);
        let changed_sky = Scene { water_light: Some(([0.0; 3], [1.0, 0.0, 0.0])), ..Default::default() };
        renderer.set_sky(&changed_sky);
        assert_eq!(renderer.water_light, changed_sky.water_light, "live authored water light must replace the previous sky's light");
        renderer.update_liquids(Vec3::from_array(camera));
        let fine = renderer.gpu.liquid[0].count;
        assert!(fine > indices.len() as u32);
        assert_eq!(renderer.gpu.liquid.len(), 2);
        assert_eq!((renderer.gpu.liquid[0].pipe, renderer.gpu.liquid[1].pipe), (PIPE, PIPE + 1));
        assert!(renderer.liquids[0].vertex_capacity > 0 && renderer.liquids[0].index_capacity > 0);
        renderer.set_realistic_water(false);
        renderer.update_liquids(Vec3::from_array(camera));
        assert_eq!(renderer.gpu.liquid[0].count, indices.len() as u32);
        renderer.update_liquids(Vec3::new(5000.0, 5.0, -5000.0));
        assert_eq!(renderer.gpu.liquid[0].count, indices.len() as u32, "native inverse preference freezes camera cell, not liquid visibility");
        renderer.set_realistic_water(true);
        renderer.update_liquids(Vec3::new(5000.0, 5.0, -5000.0));
        assert_eq!(renderer.gpu.liquid[0].count, 0);
        renderer.update_liquids(Vec3::from_array(camera));
        assert_eq!(renderer.gpu.liquid[0].count, fine);
        assert_eq!(renderer.gpu.liquid[1].first_index, fine);
    }

    #[test]
    fn native_liquid_skips_uv_regen_off_screen_but_keeps_advancing_edge_phase() {
        use ao_scene::{liquid::SourceTriangle, Instance, Mesh, Scene, Submesh, IDENTITY};
        let source = Surface::new(0, 0, vec![SourceTriangle { kind: 0, vertices: [
            Vertex { pos: [200.0, 5.0, -200.0], ..Default::default() },
            Vertex { pos: [260.0, 5.0, -200.0], ..Default::default() },
            Vertex { pos: [200.0, 5.0, -260.0], ..Default::default() },
        ] }]).unwrap();
        let camera = source.initial_camera().unwrap();
        let (mut vertices, indices) = {
            let mut vertices = vec![];
            let mut indices = vec![];
            let mut colors = vec![];
            source.geometry(camera, false, &mut vertices, &mut indices, &mut colors);
            (vertices, indices)
        };
        vertices.extend_from_within(..);
        let mut submesh = Submesh::new(indices, None);
        submesh.liquid = true;
        let mut scene = Scene::default();
        scene.meshes.push(Mesh { vertices, submeshes: vec![submesh] });
        scene.instances.push(Instance { mesh: 0, transform: IDENTITY });
        scene.liquids.push(source);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).unwrap();
        renderer.upload(&scene);
        // First frame: no prior cull result (`vis_range` empty), so treated as visible -- establishes the buffer
        // and a first set of UVs.
        renderer.update_liquids(Vec3::from_array(camera));
        let bytes_before = renderer.liquids[0].bytes.clone();
        let edge_before = renderer.liquid_edge[0];
        assert!(!bytes_before.is_empty());
        // Mark every mesh invisible, like a frustum cull that found nothing on screen last frame, then call
        // again with the same camera cell (so geometry/indices don't change) but a different clock time.
        renderer.vis_range = vec![0..0; renderer.gpu.mesh_range.len()];
        renderer.seek(1.0);
        renderer.update_liquids(Vec3::from_array(camera));
        assert_eq!(renderer.liquids[0].bytes, bytes_before, "UVs must not regenerate for an off-screen liquid");
        assert!(renderer.liquid_edge[0] > edge_before, "the edge/draw-count animation phase must keep advancing while off-screen");
        // Coming back on screen regenerates with the new clock time.
        renderer.vis_range = vec![0..1; renderer.gpu.mesh_range.len()];
        renderer.update_liquids(Vec3::from_array(camera));
        assert_ne!(renderer.liquids[0].bytes, bytes_before, "UVs must regenerate once the liquid is visible again");
    }
}
