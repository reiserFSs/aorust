//! Global access to the native collision-baked ground lattice, not receiver geometry.
use crate::gpu_stats::GpuStatsExt;

/// Four native R8 lattice corners per cell; absent cells remain entirely closed.
/// The header is eight words followed by row-major cells, shared with WGSL.
#[derive(Default)]
pub(super) struct MoonExposure {
    words: Vec<u32>,
    gpu: Option<wgpu::Buffer>,
}

impl MoonExposure {
    pub(super) fn from_scene(scene: &ao_scene::Scene) -> Self {
        let mut out = Self::default();
        let Some(first) = scene.ground_detail.first() else { return out; };
        let step = first.cell_size;
        if !step.is_finite() || step <= 0.0 { return out; }
        // Native ground is one lattice. Incompatible/malformed data is unknown,
        // never resampled into a new openness heuristic.
        if scene.ground_detail.iter().any(|p| p.cell_size != step || p.cells == 0) { return out; }
        let mut lo = [i64::MAX; 2];
        let mut hi = [i64::MIN; 2];
        for p in &scene.ground_detail {
            let Ok(cells) = i64::try_from(p.cells) else { return out; };
            for axis in 0..2 {
                lo[axis] = lo[axis].min(i64::from(p.origin[axis]));
                let Some(end) = i64::from(p.origin[axis]).checked_add(cells) else { return out; };
                hi[axis] = hi[axis].max(end);
            }
        }
        let Ok(width) = usize::try_from(hi[0] - lo[0]) else { return out; };
        let Ok(height) = usize::try_from(hi[1] - lo[1]) else { return out; };
        let Some(count) = width.checked_mul(height) else { return out; };
        // Bound retained CPU data to WebGPU's default storage-binding limit.
        // Oversized/invalid playfields fail closed rather than partially exposing roofs.
        if count == 0 || count > (128 * 1024 * 1024 - 32) / 4 { return out; }
        out.words = vec![0; 8 + count];
        out.words[..5].copy_from_slice(&[
            (lo[0] as f32).to_bits(), (lo[1] as f32).to_bits(),
            (1.0 / step).to_bits(), width as u32, height as u32,
        ]);
        for p in &scene.ground_detail {
            let Some(mesh) = scene.meshes.get(p.mesh) else { return Self::default(); };
            let Some(nodes) = p.cells.checked_add(1).and_then(|n| n.checked_mul(n)) else { return Self::default(); };
            let Some(end) = (p.lattice_start as usize).checked_add(nodes) else { return Self::default(); };
            if end > mesh.vertices.len() { return Self::default(); }
            let sky = crate::native_ground::sky_openness(p, &mesh.vertices);
            let x0 = (i64::from(p.origin[0]) - lo[0]) as usize;
            let y0 = (i64::from(p.origin[1]) - lo[1]) as usize;
            for y in 0..p.cells {
                for x in 0..p.cells {
                    let a = y * (p.cells + 1) + x;
                    out.words[8 + (y0 + y) * width + x0 + x] = u32::from_le_bytes([
                        sky[a], sky[a + 1], sky[a + p.cells + 1], sky[a + p.cells + 2],
                    ]);
                }
            }
        }
        out
    }

    pub(super) fn sync(&mut self, device: &wgpu::Device, enabled: bool) {
        if !enabled { self.gpu = None; return; }
        if self.gpu.is_some() { return; }
        let closed = [0u32; 8];
        let words = if self.words.len() * 4 <= device.limits().max_storage_buffer_binding_size as usize && !self.words.is_empty() {
            self.words.as_slice()
        } else { &closed };
        self.gpu = Some(device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
            label: Some("moon native sky openness"), contents: bytemuck::cast_slice(words),
            usage: wgpu::BufferUsages::STORAGE,
        }));
    }

    pub(super) fn buffer(&self) -> Option<&wgpu::Buffer> { self.gpu.as_ref() }
}

pub(super) fn append_shader(source: &mut String) {
    source.push_str(&crate::shader_source::load("moon_exposure.wgsl", include_str!("moon_exposure.wgsl")));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(data: &MoonExposure, world: [f32; 3]) -> f32 {
        if data.words.is_empty() { return 0.0; }
        let h = &data.words;
        let p = [world[0] * f32::from_bits(h[2]) - f32::from_bits(h[0]),
            -world[2] * f32::from_bits(h[2]) - f32::from_bits(h[1])];
        if !(p[0] >= 0.0 && p[1] >= 0.0 && p[0] <= h[3] as f32 && p[1] <= h[4] as f32) { return 0.0; }
        let x = (p[0] as u32).min(h[3] - 1);
        let y = (p[1] as u32).min(h[4] - 1);
        let corners = h[8 + (y * h[3] + x) as usize].to_le_bytes().map(|v| f32::from(v) / 255.0);
        let t = [p[0] - x as f32, p[1] - y as f32];
        let a = corners[0] * (1.0 - t[0]) + corners[1] * t[0];
        let b = corners[2] * (1.0 - t[0]) + corners[3] * t[0];
        a * (1.0 - t[1]) + b * t[1]
    }

    #[test]
    fn exact_native_interpolation_roof_and_unknown_cells() {
        use ao_scene::{ground_detail::GroundPatch, Mesh, Scene, Vertex};
        let patch = GroundPatch { origin: [-7, 11], cells: 2, cell_size: 2.5, lattice_start: 4, ..Default::default() };
        let mut vertices = vec![Vertex::default(); 4];
        vertices.extend([1.0, 1.0, 0.5, 1.0, 1.0, 0.0, 0.75, 0.25, 0.0].map(|a| Vertex { color: [1.0, 1.0, 1.0, a], ..Default::default() }));
        let mut scene = Scene::default();
        scene.meshes.push(Mesh { vertices, ..Default::default() });
        scene.ground_detail.push(patch);
        let exposure = MoonExposure::from_scene(&scene);
        assert_eq!(sample(&exposure, [-16.25, 200.0, -28.75]), 0.0, "roofed cell closes actors at any elevation");
        let expected = (128.0 + 255.0) / (4.0 * 255.0);
        assert!((sample(&exposure, [-13.75, 3.0, -28.75]) - expected).abs() < 1e-6);
        assert_eq!(sample(&exposure, [-17.6, 0.0, -28.0]), 0.0);
        assert_eq!(sample(&exposure, [0.0, 0.0, 0.0]), 0.0);
        assert_eq!(sample(&MoonExposure::from_scene(&Scene::default()), [0.0; 3]), 0.0);
        // Missing patches inside the bounding rectangle must not interpolate neighbours.
        let mut distant = scene.ground_detail[0].clone();
        distant.origin[0] += 4;
        scene.ground_detail.push(distant);
        assert_eq!(sample(&MoonExposure::from_scene(&scene), [-10.0, 0.0, -28.75]), 0.0);
    }
}
