//! The shared depth pyramid (HiZ, slot 3a): one R32Float texture of min VIEW depth next to G1's normals, built right after the
//! G-buffer. Every later consumer (GTAO, water SSR, froxel inject, TAA dilation) reads this texture instead of the D32F depth.
use super::gbuffer::Targets;

/// Mip count (mips 0..=7).
pub(super) const MIPS: u32 = 8;
pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Mirror of `HIZ_SKY` in `pyramid.wgsl` / `ao.wgsl`: the value of a texel that holds no geometry (depth == 1.0).
#[cfg(test)]
pub(super) const SKY: f32 = 1.0e9;

pub(super) fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }
}
pub(super) fn texture(binding: u32, visibility: wgpu::ShaderStages, depth: bool) -> wgpu::BindGroupLayoutEntry {
    let sample_type = if depth { wgpu::TextureSampleType::Depth } else { wgpu::TextureSampleType::Float { filterable: false } };
    wgpu::BindGroupLayoutEntry { binding, visibility,
        ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }
}
pub(super) fn storage(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format, view_dimension: wgpu::TextureViewDimension::D2 }, count: None }
}
pub(super) fn entry<'a>(binding: u32, view: &'a wgpu::TextureView) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
}

/// Size of mip `mip` of a `size` texture (floor halving, never below 1).
pub(super) fn mip_size(size: (u32, u32), mip: u32) -> (u32, u32) { ((size.0 >> mip).max(1), (size.1 >> mip).max(1)) }

/// Eight mips need a longest side of 128; smaller viewports allocate the minimum (consumers clamp to the viewport).
fn alloc_size(size: (u32, u32)) -> (u32, u32) { (size.0.max(128), size.1.max(128)) }

pub(super) struct Pipes {
    a: wgpu::ComputePipeline,
    b: wgpu::ComputePipeline,
    layout_a: wgpu::BindGroupLayout,
    layout_b: wgpu::BindGroupLayout,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let cs = wgpu::ShaderStages::COMPUTE;
        let layout_a = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 hiz a"),
            entries: &[uniform(0, cs), texture(1, cs, true), storage(2, FORMAT), storage(3, FORMAT), storage(4, FORMAT), storage(5, FORMAT)] });
        let layout_b = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 hiz b"),
            entries: &[texture(6, cs, false), storage(7, FORMAT), storage(8, FORMAT), storage(9, FORMAT), storage(10, FORMAT)] });
        let source = format!("{}\n{}\n", crate::shader_source::load("v2/common.wgsl", include_str!("common.wgsl")),
            crate::shader_source::load("v2/pyramid.wgsl", include_str!("pyramid.wgsl")));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 hiz"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline = |layout: &wgpu::BindGroupLayout, entry_point| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(entry_point), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry_point), layout: Some(&pipeline_layout), module: &module,
                entry_point: Some(entry_point), compilation_options: Default::default(), cache: None })
        };
        Self { a: pipeline(&layout_a, "build_a"), b: pipeline(&layout_b, "build_b"), layout_a, layout_b }
    }
}

pub(super) struct Pyramid {
    /// All mips.
    view: wgpu::TextureView,
    bind_a: wgpu::BindGroup,
    bind_b: wgpu::BindGroup,
    size: (u32, u32),
}

impl Pyramid {
    pub(super) fn new(device: &wgpu::Device, pipes: &Pipes, consts: &wgpu::Buffer, targets: &Targets) -> Self {
        let alloc = alloc_size(targets.size);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("v2 HiZ"), size: wgpu::Extent3d { width: alloc.0, height: alloc.1, depth_or_array_layers: 1 },
            mip_level_count: MIPS, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        });
        let mip = |level| texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: level, mip_level_count: Some(1), ..Default::default() });
        let mips: Vec<_> = (0..MIPS).map(mip).collect();
        let bind_a = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 hiz a"), layout: &pipes.layout_a, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() },
            entry(1, &targets.depth), entry(2, &mips[0]), entry(3, &mips[1]), entry(4, &mips[2]), entry(5, &mips[3])] });
        let bind_b = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 hiz b"), layout: &pipes.layout_b, entries: &[
            entry(6, &mips[3]), entry(7, &mips[4]), entry(8, &mips[5]), entry(9, &mips[6]), entry(10, &mips[7])] });
        Self { view: texture.create_view(&Default::default()), bind_a, bind_b, size: alloc }
    }

    /// The pyramid, all [`MIPS`] mips (`texture_2d<f32>`, read with `textureLoad(.., mip)`; not filterable).
    ///
    /// Layout: R32Float; mip 0 is the full-resolution VIEW depth (metres along the view axis, `-(view * p).z`), mip m > 0 the
    /// minimum of the 2x2 block below it, mip sizes `max(1, w >> m)`. Texels without geometry (G-buffer depth == 1, the sky)
    /// hold [`SKY`] (1e9), so a min-reduction keeps the nearest surface. Valid after [`Pyramid::encode`] in the same frame.
    pub(super) fn view(&self) -> &wgpu::TextureView { &self.view }

    pub(super) fn encode(&self, pipes: &Pipes, enc: &mut wgpu::CommandEncoder, timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 hiz"), timestamp_writes });
        let tiles = |size: (u32, u32)| (size.0.div_ceil(16), size.1.div_ceil(16));
        pass.set_pipeline(&pipes.a);
        pass.set_bind_group(0, &self.bind_a, &[]);
        let a = tiles(self.size);
        pass.dispatch_workgroups(a.0, a.1, 1);
        pass.set_pipeline(&pipes.b);
        pass.set_bind_group(0, &self.bind_b, &[]);
        let b = tiles(mip_size(self.size, 3));
        pass.dispatch_workgroups(b.0, b.1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CPU mirror of one `build_*` step: min over 2x2 blocks, blocks past the edge repeat the last row/column.
    fn reduce(src: &[f32], size: (u32, u32)) -> (Vec<f32>, (u32, u32)) {
        let dst = mip_size(size, 1);
        let at = |x: u32, y: u32| src[(y.min(size.1 - 1) * size.0 + x.min(size.0 - 1)) as usize];
        let out = (0..dst.1).flat_map(|y| (0..dst.0).map(move |x| (x, y)))
            .map(|(x, y)| at(2 * x, 2 * y).min(at(2 * x + 1, 2 * y)).min(at(2 * x, 2 * y + 1)).min(at(2 * x + 1, 2 * y + 1))).collect();
        (out, dst)
    }

    #[test]
    fn mips_halve_down_to_one_texel() {
        let sizes: Vec<_> = (0..MIPS).map(|m| mip_size((1920, 1200), m)).collect();
        assert_eq!(sizes, [(1920, 1200), (960, 600), (480, 300), (240, 150), (120, 75), (60, 37), (30, 18), (15, 9)]);
        assert_eq!(mip_size((128, 128), MIPS - 1), (1, 1));
        assert_eq!(alloc_size((64, 40)), (128, 128));
        // build_a covers mip 0..3 per 16x16 tile; build_b mip 3..7 per 16x16 tile of mip 3.
        assert_eq!((1920u32.div_ceil(16), 1200u32.div_ceil(16)), (120, 75));
        assert_eq!(mip_size((1920, 1200), 3), (240, 150));
    }

    #[test]
    fn min_reduction_keeps_the_nearest_surface_and_sky_never_wins() {
        let (w, h) = (9u32, 5u32);
        let mut depth: Vec<f32> = (0..w * h).map(|i| 10.0 + (i % 7) as f32).collect();
        depth[(3 * w + 6) as usize] = 2.5; // a near pole
        for texel in depth.iter_mut().skip(((h - 1) * w) as usize) { *texel = SKY; } // last row is sky
        let (m1, s1) = reduce(&depth, (w, h));
        assert_eq!(s1, (4, 2));
        assert_eq!(m1[(1 * 4 + 3) as usize], 2.5);
        assert!(m1.iter().all(|&z| z < SKY)); // every block has some geometry
        let (m2, s2) = reduce(&m1, s1);
        assert_eq!(s2, (2, 1));
        assert_eq!(m2[1], 2.5);
        assert_eq!(m2[0], m1[0].min(m1[1]).min(m1[4]).min(m1[5]));
    }

    #[test]
    fn linear_depth_inverts_the_projection() {
        let (near, far) = (0.1_f32, 600.0_f32);
        let proj = glam::Mat4::perspective_rh(1.0, 1.6, near, far);
        for z in [0.1_f32, 1.0, 7.5, 80.0, 599.0] {
            let clip = proj * glam::Vec4::new(0.0, 0.0, -z, 1.0);
            let d = clip.z / clip.w;
            let back = near * far / (far - d * (far - near)); // the pyramid.wgsl formula
            assert!((back - z).abs() < z * 2e-3, "{z} -> {back}");
        }
    }

    #[test]
    fn sky_sentinel_matches_both_shaders() {
        for source in [include_str!("pyramid.wgsl"), include_str!("ao.wgsl")] {
            assert!(source.contains("const HIZ_SKY: f32 = 1.0e9;"));
        }
        assert_eq!(SKY, 1.0e9);
    }
}
