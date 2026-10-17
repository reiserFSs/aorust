//! Current-frame bloom: authored emission always, plus (HDR only) highlights above white. No history.
use crate::gpu_stats::{GpuStatsExt, TextureViewStatsExt};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const LEVELS: usize = 7;

pub(super) struct Bloom {
    pub size: (u32, u32),
    /// Non-HDR only: the UNORM-range composite handed to the presentation pass.
    pub present: Option<wgpu::BindGroup>,
    hdr: bool,
    down: Vec<Pass>,
    up: Vec<Pass>,
    composite: Option<Pass>,
    params: wgpu::Buffer,
    strength: f32,
    pipelines: [wgpu::RenderPipeline; 4],
    pub bytes: u64,
}
struct Pass {
    bind: wgpu::BindGroup,
    target: wgpu::TextureView,
}
fn target(device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
    device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: Some("Bloom energy pyramid"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
            | if cfg!(test) { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
        view_formats: &[],
    }).create_view_tracked(&Default::default())
}
fn sizes(mut size: (u32, u32)) -> Vec<(u32, u32)> {
    let mut result = Vec::with_capacity(LEVELS);
    for _ in 0..LEVELS {
        size = (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1));
        result.push(size);
        if size == (1, 1) { break; }
    }
    result
}
/// Size of the final (half-resolution) halo level; the HDR grade owns a texture of this size.
pub(super) fn halo_size(size: (u32, u32)) -> (u32, u32) { sizes(size)[0] }
/// Shared glare helpers used by bloom and HDR presentation.
pub(super) fn glare() -> std::borrow::Cow<'static, str> {
    crate::shader_source::load("bloom_glare.wgsl", include_str!("bloom_glare.wgsl"))
}
impl Pass {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout,
        inputs: [&wgpu::TextureView; 3], params: &wgpu::Buffer, target: wgpu::TextureView) -> Self {
        let entries = std::array::from_fn::<_, 4, _>(|i| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: if i == 3 { params.as_entire_binding() } else { wgpu::BindingResource::TextureView(inputs[i]) },
        });
        let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Bloom pass"), layout, entries: &entries });
        Self { bind, target }
    }
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bounded emissive bloom"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.target, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
impl Bloom {
    /// `beauty` is the native colour (UNORM path) or the exposed HDR scene. With `halo`, the final
    /// half-resolution level is written there (the HDR `present` adds it above white); otherwise a
    /// UNORM-range composite is built and exposed through `present`.
    pub fn new(device: &wgpu::Device, size: (u32, u32), beauty: &wgpu::TextureView,
        emission: &wgpu::TextureView, presentation: &wgpu::BindGroupLayout, halo: Option<&wgpu::TextureView>) -> Self {
        let source = [glare().as_ref(), crate::shader_source::load("bloom.wgsl", include_str!("bloom.wgsl")).as_ref()].concat();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Visible-source bounded bloom"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let entries = std::array::from_fn::<_, 4, _>(|i| wgpu::BindGroupLayoutEntry {
            binding: i as u32, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: if i == 3 {
                wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }
            } else {
                wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }
            }, count: None,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Bloom inputs"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Bloom kernels"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipelines = ["seed_down", "down", "up", "composite"].map(|entry| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("fullscreen"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        }));
        let hdr = halo.is_some();
        let params = device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: Some("Bloom strength"), contents: bytemuck::cast_slice(&[0.0_f32, f32::from(u8::from(hdr)), 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST });
        let levels = sizes(size);
        let mut down: Vec<Pass> = Vec::with_capacity(levels.len());
        for (i, &level) in levels.iter().enumerate() {
            let source = if i == 0 { emission } else { &down[i - 1].target };
            let out = if i == 0 && levels.len() == 1 { halo.cloned() } else { None };
            down.push(Pass::new(device, &layout, [source, emission, beauty], &params, out.unwrap_or_else(|| target(device, level))));
        }
        let mut up: Vec<Pass> = Vec::with_capacity(levels.len().saturating_sub(1));
        for i in (0..levels.len() - 1).rev() {
            let coarse = up.last().map_or(&down[i + 1].target, |pass| &pass.target);
            let out = if i == 0 { halo.cloned() } else { None };
            up.push(Pass::new(device, &layout, [&down[i].target, coarse, emission], &params, out.unwrap_or_else(|| target(device, levels[i]))));
        }
        let pyramid: u64 = levels.iter().enumerate().map(|(i, &(w, h))| u64::from(w) * u64::from(h) * if i + 1 == levels.len() { 1 } else { 2 }).sum();
        let (present, composite, bytes) = if hdr {
            // The halo texture belongs to the grade; the level-0 up target is not counted twice.
            (None, None, 8 * (pyramid - u64::from(levels[0].0) * u64::from(levels[0].1)))
        } else {
            let halo = up.last().map_or(&down[0].target, |pass| &pass.target);
            let output = target(device, size);
            let present = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Bloom presentation"),
                layout: presentation, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&output) }] });
            (Some(present), Some(Pass::new(device, &layout, [beauty, halo, emission], &params, output)),
                8 * (u64::from(size.0) * u64::from(size.1) + pyramid))
        };
        Self { size, present, hdr, down, up, composite, params, strength: 0.0, pipelines, bytes }
    }
    pub fn is_hdr(&self) -> bool { self.hdr }
    pub fn encode(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, strength: f32) {
        let strength = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 0.0 };
        if self.strength != strength {
            queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&[strength, f32::from(u8::from(self.hdr)), 0.0, 0.0]));
            self.strength = strength;
        }
        for (i, pass) in self.down.iter().enumerate() { pass.encode(encoder, &self.pipelines[usize::from(i != 0)], None); }
        for pass in &self.up { pass.encode(encoder, &self.pipelines[2], None); }
        if let Some(composite) = &self.composite { composite.encode(encoder, &self.pipelines[3], None); }
    }
}

impl crate::Renderer {
    pub(super) fn prepare_enhanced_bloom(&mut self) {
        if !self.enhanced_settings.emissive_bloom || self.enhanced_settings.bloom_strength <= 0.0 {
            self.bloom = None;
            return;
        }
        let Some(frame) = self.enhanced_frame.as_ref() else { self.bloom = None; return; };
        // With HDR the pyramid reads the exposed scene and writes the grade's halo; the grade is prepared first.
        let grade = self.grade.as_ref();
        if self.bloom.as_ref().is_none_or(|bloom| bloom.size != frame.size || bloom.is_hdr() != grade.is_some()) {
            let emission = frame.emissive.as_ref().expect("enabled bloom owns its world emission mask");
            self.bloom = Some(Bloom::new(&self.device, frame.size, grade.map_or(&frame.colour, |grade| grade.hdr_view()), emission,
                &frame.composite.get_bind_group_layout(0), grade.map(|grade| grade.halo_view())));
        }
    }
    /// Explicit offline diagnostic; no resources or passes exist until requested.
    pub fn emissive_seed_rgba(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(frame) = self.enhanced_frame.as_ref() else { return Ok(None); };
        let Some(emission) = frame.emissive.as_ref() else { return Ok(None); };
        let output = self.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Authored emission mask diagnostic"),
            size: wgpu::Extent3d { width: frame.size.0, height: frame.size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let bind = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("Authored emission mask diagnostic"), layout: &frame.composite.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(emission) }],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        frame.composite(&mut encoder, &output.create_view_tracked(&Default::default()), Some(&bind), None);
        self.queue.submit([encoder.finish()]);
        let mut pixels = crate::texture_to_rgba(self, &output, frame.size.0, frame.size.1)?;
        if self.format == wgpu::TextureFormat::Bgra8Unorm {
            for pixel in pixels.as_chunks_mut::<4>().0 { pixel.swap(0, 2); }
        }
        Ok(Some(pixels))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn half_resolution_pyramid_handles_odd_and_tiny_frames() {
        assert_eq!(sizes((1920, 1200)), [(960,600),(480,300),(240,150),(120,75),(60,38),(30,19),(15,10)]);
        assert_eq!(sizes((3,5)), [(2,3),(1,2),(1,1)]);
        assert_eq!(sizes((1,1)), [(1,1)]);
    }
    #[test]
    fn authored_source_color_addition_is_not_inferred_from_brightness() {
        let mut material = ao_scene::Submesh::new(Vec::new(), None);
        material.blend = ao_scene::Blend::SourceColorOne;
        let marked = |s: &ao_scene::Submesh| crate::mat_uniform(s)[30] as u32 & 8 != 0;
        assert!(marked(&material));
        material.liquid = true;
        assert!(!marked(&material));
        assert_ne!(crate::mat_uniform(&material)[30] as u32 & 16, 0, "liquid authoring emission is explicitly excluded");
        material.liquid = false;
        material.sky_points = true;
        assert!(!marked(&material));
        material.sky_points = false;
        material.sky_fog = true;
        assert!(!marked(&material));
        material.sky_fog = false;
        material.blend = ao_scene::Blend::Opaque;
        material.base_color = [1.0; 4];
        material.prelit = true;
        material.lighting_enabled = false;
        assert!(!marked(&material), "brightness/prelit cannot become additive provenance; unlit uses its own native bit");
    }
    #[test]
    fn installed_light_bar_carries_authored_emission() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            eprintln!("SKIP Light Bar bloom: installed client absent at {}", dir.display());
            return;
        }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let mut scene = ao_scene::Scene::default();
        // ActorRig's attachment loader, not GenericMesh's first-material reset.
        let index = ao_formats::mesh::decode_statel_mesh(&store, ao_formats::mesh::MESH_TYPE,
            26163, &[], None, &mut scene).unwrap().unwrap();
        let materials = &scene.meshes[index].submeshes;
        assert!(materials.iter().any(|s| s.glow_mask || s.emissive.iter().any(|&v| v > 0.0)
            || matches!(s.blend, ao_scene::Blend::Additive | ao_scene::Blend::AdditiveOne | ao_scene::Blend::SourceColorOne)),
            "Light Bar 26163 needs authored emission, never a brightness exception: {:?}",
            materials.iter().map(|s| (s.blend, s.emissive, s.glow_mask)).collect::<Vec<_>>());
    }
    pub(crate) fn f16_bits(v: f32) -> u16 {
        if v == 0.0 { return 0; }
        let bits = v.to_bits();
        let exponent = ((bits >> 23) & 255) as i32 - 127 + 15;
        assert!(v > 0.0 && (1..31).contains(&exponent), "test values are small positive normals: {v}");
        ((exponent as u16) << 10) | ((bits >> 13) & 1023) as u16
    }
    pub(crate) fn f16_value(bits: u16) -> f32 {
        let exponent = i32::from((bits >> 10) & 31);
        let mantissa = f32::from(bits & 1023);
        if exponent == 0 { mantissa * 2.0_f32.powi(-24) } else { (1.0 + mantissa / 1024.0) * 2.0_f32.powi(exponent - 15) }
    }
    /// The real kernels over a 31x29 frame. `hdr` reads back the halo level the grade would add; otherwise the UNORM composite.
    struct Rig {
        r: crate::Renderer,
        size: (u32, u32),
        beauty: wgpu::Texture,
        emission: wgpu::Texture,
        out: wgpu::Texture,
        out_size: (u32, u32),
        bloom: Bloom,
    }
    impl Rig {
        fn new(hdr: bool) -> Self {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let r = crate::Renderer::new(&instance, None).expect("offscreen bloom GPU");
            let size = (31, 29);
            let texture = |size: (u32, u32), usage| r.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Bloom regression"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: FORMAT, usage, view_formats: &[],
            });
            let input_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            let beauty = texture(size, input_usage);
            let emission = texture(size, input_usage);
            let out_size = if hdr { halo_size(size) } else { size };
            let out = texture(out_size, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
            let presentation = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None,
                entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }] });
            let (bv, ev, ov) = (beauty.create_view(&Default::default()), emission.create_view(&Default::default()), out.create_view(&Default::default()));
            let bloom = Bloom::new(&r.device, size, &bv, &ev, &presentation, hdr.then_some(&ov));
            let out = bloom.composite.as_ref().map_or(out, |composite| composite.target.texture().clone());
            Self { r, size, beauty, emission, out, out_size, bloom }
        }
        fn count(&self) -> usize { (self.size.0 * self.size.1) as usize }
        fn at(&self, x: u32, y: u32) -> usize { (y * self.size.0 + x) as usize }
        fn centre(&self) -> usize { self.at(self.size.0 / 2, self.size.1 / 2) }
        /// `beauty` and `emission` are per-pixel RGBA (non-negative); result is the readback RGBA.
        fn run(&mut self, beauty: &[[f32; 4]], emission: &[[f32; 4]], strength: f32) -> Vec<[f32; 4]> {
            let r = &self.r;
            let upload = |texture: &wgpu::Texture, pixels: &[[f32; 4]]| {
                let bits: Vec<[u16; 4]> = pixels.iter().map(|p| p.map(f16_bits)).collect();
                r.queue.write_texture(texture.as_image_copy(), bytemuck::cast_slice(&bits),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.size.0 * 8), rows_per_image: Some(self.size.1) },
                    wgpu::Extent3d { width: self.size.0, height: self.size.1, depth_or_array_layers: 1 });
            };
            upload(&self.beauty, beauty);
            upload(&self.emission, emission);
            let mut encoder = r.device.create_command_encoder(&Default::default());
            self.bloom.encode(&r.queue, &mut encoder, strength);
            let (w, h) = self.out_size;
            let row = (w * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let buffer = r.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(row * h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            encoder.copy_texture_to_buffer(self.out.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
            r.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            r.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let mapped = buffer.slice(..).get_mapped_range();
            let mut result = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    let offset = (y * row + x * 8) as usize;
                    result.push(std::array::from_fn(|c| f16_value(u16::from_le_bytes([mapped[offset + 2 * c], mapped[offset + 2 * c + 1]]))));
                }
            }
            result
        }
    }
    const WHITE: [f32; 4] = [1.0; 4];
    // Shader behaviour, not CPU approximations of it: UNORM-range path (hdr off).
    #[test]
    fn gpu_bloom_is_emissive_only_occluded_and_energy_bounded() {
        let mut rig = Rig::new(false);
        let (count, center) = (rig.count(), rig.centre());
        assert_eq!(rig.bloom.bytes, 12_384, "odd-size texture storage and one output, no history");
        let none = vec![[0.0; 4]; count];
        let red = |v: &[[f32; 4]]| v.iter().map(|p| p[0]).collect::<Vec<_>>();
        assert!(red(&rig.run(&vec![WHITE; count], &none, 0.5)).iter().all(|&v| v == 1.0),
            "white walls, snow and sky without marked emission never bloom");
        let dark = vec![[0.03125, 0.03125, 0.03125, 1.0]; count];
        assert!(red(&rig.run(&dark, &none, 0.5)).iter().all(|&v| v == 0.03125), "a non-emissive dark scene remains exactly unchanged");
        let mut pixels = vec![[0.0, 0.0, 0.0, 1.0]; count];
        let mut sources = none.clone();
        pixels[center] = WHITE;
        sources[center] = WHITE;
        let glare = red(&rig.run(&pixels, &sources, 0.5));
        assert_eq!(glare[center], 1.0, "bloom must never dim the native emitter core");
        // Falloff: strictly smooth along the row and the column (no repeated nearest-neighbour 2x2 blocks).
        let (cx, cy) = (rig.size.0 / 2, rig.size.1 / 2);
        for dx in 1..9 {
            assert!(glare[rig.at(cx + dx, cy)] > glare[rig.at(cx + dx + 1, cy)], "row halo falls off at +{dx}");
            assert!(glare[rig.at(cx, cy + dx)] > glare[rig.at(cx, cy + dx + 1)], "column halo falls off at +{dx}");
            assert!(glare[rig.at(cx - dx, cy)] > glare[rig.at(cx - dx - 1, cy)], "left halo falls off at -{dx}");
        }
        // Round: the same radius reads the same along an axis and a diagonal (4,4 is 5.66 px; 6,0 is 6 px).
        let axis = glare[rig.at(cx + 6, cy)];
        let diagonal = glare[rig.at(cx + 4, cy + 4)];
        assert!((axis / diagonal - 1.0).abs() < 0.3, "halo is round, not a square: axis {axis} diagonal {diagonal}");
        assert!(glare.iter().enumerate().filter(|&(i, _)| i != center).any(|(_, &v)| v > 0.01), "visible emission produces a halo");
        assert!(glare.iter().sum::<f32>() <= 1.0 + 2.01, "halo energy stays within strength*gain of the source");
        assert!(red(&rig.run(&pixels, &none, 0.5)).iter().enumerate().all(|(i, &v)| v == if i == center { 1.0 } else { 0.0 }),
            "a source fully removed by world depth leaves no bloom signal");
        pixels[center] = [0.0, 0.0, 0.0, 1.0];
        sources[center] = [0.0; 4];
        pixels[count - 1] = WHITE;
        sources[count - 1] = WHITE;
        assert!(red(&rig.run(&pixels, &sources, 0.5)).iter().sum::<f32>() <= 1.0 + 2.01,
            "reconstruction at the odd-size corner cannot mint halo energy");
        pixels[count - 1] = [0.0, 0.0, 0.0, 1.0];
        sources[count - 1] = [0.0; 4];
        pixels[center] = [2.0; 4]; // Synthetic over-range input; the UNORM path clips to one.
        sources[center] = [2.0; 4];
        assert!(red(&rig.run(&pixels, &sources, 0.5)).iter().sum::<f32>() <= 1.0 + 2.01,
            "display clipping cannot mint additional halo budget");
        pixels.fill([0.25, 0.25, 0.25, 1.0]);
        pixels[center] = WHITE;
        sources[center] = WHITE;
        let lens = red(&rig.run(&pixels, &sources, 0.5));
        assert!(lens.iter().enumerate().any(|(i, &v)| i != center && v > 0.25), "already-visible emission can produce lens glare over geometry");
        assert!(lens.iter().enumerate().all(|(i, &v)| i == center || v <= 0.25 + 64.0 / 255.0),
            "net lens glare is capped at 64 display-code luminance levels");
        pixels.fill([0.03125, 0.03125, 0.03125, 1.0]);
        pixels[center] = WHITE;
        assert!(red(&rig.run(&pixels, &sources, 0.5)).iter().enumerate().all(|(i, &v)| i == center || v <= 0.03125 + 64.0 / 255.0),
            "dark geometry uses the same receiver-independent glare cap");
        pixels[center] = [1.0, 0.0, 0.0, 1.0];
        sources[center] = pixels[center];
        assert!(red(&rig.run(&pixels, &sources, 0.5)).iter().enumerate().all(|(i, &v)|
            i == center || (v - 0.03125) * 0.2126 <= 64.0 / 255.0), "saturated red glare obeys the luminance cap, not a white-source shortcut");
        pixels.fill(WHITE);
        sources.fill(WHITE);
        pixels[center] = [0.03125, 0.03125, 0.03125, 1.0];
        sources[center] = [0.0; 4];
        let capped = red(&rig.run(&pixels, &sources, 0.5))[center];
        assert!(capped > 0.03125 + 1.0 / 255.0, "glare is visibly stronger than a one-level cap");
        assert!(capped <= 0.03125 + 64.0 / 255.0, "dense sources cannot exceed the net luma cap");
        assert!(!include_str!("bloom.wgsl").contains("texture_depth"), "lens reconstruction must not expose receiver depth silhouettes");
    }
    #[test]
    fn gpu_hdr_highlight_seeds_exist_only_above_white() {
        let mut rig = Rig::new(true);
        assert_eq!(rig.out_size, halo_size(rig.size));
        let (count, center) = (rig.count(), rig.centre());
        let none = vec![[0.0; 4]; count];
        let total = |halo: &[[f32; 4]]| halo.iter().map(|p| p[0] + p[1] + p[2]).sum::<f32>();
        let a_total = |halo: &[[f32; 4]]| halo.iter().map(|p| p[3]).sum::<f32>();
        let field = |v: f32| vec![[v, v, v, 1.0]; count];
        // Native white and everything below it is no highlight: snow, white walls, lit sky stay unbloomed.
        for v in [0.2, 0.9, 1.0] {
            assert_eq!(total(&rig.run(&field(v), &none, 0.6)), 0.0, "uniform {v} seeds nothing without authored emission");
        }
        let mut pixels = field(0.0);
        pixels[center] = [1.4, 1.4, 1.4, 1.0];
        let lit = rig.run(&pixels, &none, 0.6);
        assert!(total(&lit) > 0.1, "a sun-boosted pixel (above white) seeds a halo");
        assert!(a_total(&lit) > 0.03, "highlight energy is tagged in alpha so the authored cap is not applied to it");
        pixels[center] = [1.0, 1.0, 1.0, 1.0];
        assert_eq!(total(&rig.run(&pixels, &none, 0.6)), 0.0, "exactly white is below threshold");
        pixels[center] = [3.0, 3.0, 3.0, 1.0];
        let brighter = a_total(&rig.run(&pixels, &none, 0.6));
        assert!(brighter > a_total(&lit), "more excess seeds more energy");
        // Authored seeds stay untagged (capped) and are clipped to the displayed colour, never inferred from brightness.
        pixels[center] = [0.8, 0.8, 0.8, 1.0];
        let mut sources = none.clone();
        sources[center] = WHITE;
        let authored = rig.run(&pixels, &sources, 0.6);
        assert!(total(&authored) > 0.0 && a_total(&authored) == 0.0, "authored emission carries no highlight tag");
        assert_eq!(total(&rig.run(&pixels, &none, 0.6)), 0.0, "no emission mask and no highlight: no halo (no history, no inference)");
        // Hidden-source safety: highlight seeds come from the current visible colour only.
        pixels[center] = [0.0, 0.0, 0.0, 1.0];
        assert_eq!(total(&rig.run(&pixels, &sources, 0.6)), 0.0, "an emission mark under a fully dark visible pixel leaks nothing");
    }
    #[test]
    fn bloom_switch_resize_and_msaa_retire_cached_views() {
        use crate::{Camera, EnhancedSettings, Renderer, Targets, Vec3, texture_to_rgba};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("bloom lifecycle GPU");
        r.set_msaa_samples(1);
        let model = ao_scene::Scene { meshes: vec![ao_scene::Mesh {
            vertices: [(-0.5,-0.5),(0.5,-0.5),(0.5,0.5),(-0.5,0.5)].into_iter().map(|(x,y)|
                ao_scene::Vertex { pos: [x,y,0.0], normal: [0.0,0.0,1.0], ..Default::default() }).collect(),
            submeshes: vec![ao_scene::Submesh { emissive: [1.0; 3], two_sided: true,
                ..ao_scene::Submesh::new(vec![0,1,2,0,2,3], None) }],
        }], ..Default::default() };
        r.add_actor_model(1, &model);
        let mut actor = ao_scene::ActorFrame { id: 1, model: 1, ..Default::default() };
        actor.transform[3][2] = -3.0;
        r.set_actors(vec![actor.clone()]);
        let capture = |r: &mut Renderer, width, height| {
            let output = r.device.create_texture(&wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
            });
            r.render(&output.create_view(&Default::default()), &Targets::new(r,width,height),
                &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
            texture_to_rgba(r, &output, width, height).unwrap()
        };
        let faithful = capture(&mut r, 32, 32);
        assert!(r.bloom.is_none());
        r.set_enhanced_settings(EnhancedSettings { ground_grass: true, grass_density: 0.0, ..Default::default() });
        let neutral = capture(&mut r, 32, 32);
        assert!(r.bloom.is_none() && r.enhanced_frame.as_ref().unwrap().emissive.is_none());
        let settings = EnhancedSettings { emissive_bloom: true, ..Default::default() };
        r.set_enhanced_settings(settings);
        assert_ne!(neutral, capture(&mut r, 32, 32), "actual world emission must reach lens composition");
        assert!(r.bloom.is_some());
        capture(&mut r, 47, 33);
        assert_eq!(r.bloom.as_ref().unwrap().size, (47,33));
        if let Some(samples) = r.msaa_supported.iter().copied().find(|&n| n > 1) {
            r.set_msaa_samples(samples);
            assert!(r.bloom.is_none(), "MSAA change cannot retain old colour input views");
            capture(&mut r, 32, 32);
            assert!(r.bloom.is_some());
        }
        capture(&mut r, 32, 32);
        r.set_actors(Vec::new());
        let absent = capture(&mut r, 32, 32);
        let mut invisible = actor.clone();
        invisible.alpha = 0.0;
        r.set_actors(vec![invisible]);
        assert_eq!(absent, capture(&mut r, 32, 32), "invisible emissive actors leave no lens signal");
        r.set_actors(Vec::new());
        r.set_enhanced_settings(EnhancedSettings { ground_grass: true, grass_density: 0.0, ..Default::default() });
        assert_eq!(absent, capture(&mut r, 32, 32), "a removed emitter leaves no bloom history");
        r.set_actors(vec![actor]);
        r.set_enhanced_settings(EnhancedSettings { bloom_strength: 0.0, ..settings });
        capture(&mut r, 32, 32);
        assert!(r.bloom.is_none(), "zero strength bypasses pyramid and presentation");
        r.set_enhanced_settings(EnhancedSettings::default());
        r.set_msaa_samples(1);
        assert!(r.bloom.is_none());
        assert_eq!(r.enhanced_target_bytes(), 0);
        assert_eq!(faithful, capture(&mut r, 32, 32), "all-off roundtrip is byte exact");
    }
    #[test]
    fn lightning_actor_emission_respects_fan_and_texture_alpha_on_dark_sky() {
        use crate::{Camera, EnhancedSettings, Renderer, Targets, Vec3, texture_to_rgba};
        use ao_scene::{ActorFrame, Blend, Environment, Lens, Mesh, Scene, Submesh, Texture, TextureKey, Vertex};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("lightning coverage GPU");
        r.set_msaa_samples(1);
        r.upload(&Scene { environment: Some(Environment {
            sky_color: [0.002; 3], fog_color: [0.002; 3], fog_start: 100.0, fog_end: 200.0,
            ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0,
        }), ..Default::default() });
        r.set_lens(Lens { fov: std::f32::consts::FRAC_PI_2, ..Default::default() });
        let capture = |r: &mut Renderer, settings| {
            r.set_enhanced_settings(settings);
            let output = r.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Lightning alpha coverage regression"),
                size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
            });
            r.render(&output.create_view(&Default::default()), &Targets::new(r, 64, 64),
                &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
            texture_to_rgba(r, &output, 64, 64).unwrap()
        };
        let off = EnhancedSettings::default();
        let ao = EnhancedSettings { ambient_occlusion: true, ..off };
        let shadows = EnhancedSettings { sun_shadows: true, ..off };
        let bloom = EnhancedSettings { emissive_bloom: true, ..off };
        let empty = capture(&mut r, off);
        assert_eq!(empty, capture(&mut r, ao), "AO cannot mark an empty dark sky");
        assert_eq!(empty, capture(&mut r, shadows), "shadows cannot mark an empty dark sky");
        assert_eq!(empty, capture(&mut r, bloom), "dark sky is not a bloom emitter");
        assert!(r.emissive_seed_rgba().unwrap().unwrap().as_chunks::<4>().0.iter()
            .all(|pixel| pixel[..3] == [0, 0, 0]));

        for textured in [false, true] {
            let key = TextureKey { rdb_type: 0, id: 1 };
            let mut vertices: Vec<_> = [(-1.0,-1.0),(1.0,-1.0),(1.0,1.0),(-1.0,1.0)]
                .into_iter().map(|(x,y)| Vertex {
                    pos: [x,y,-2.0], normal: [0.0,0.0,1.0], uv: [(x+1.0)*0.5,(1.0-y)*0.5],
                    color: if textured { [0.6,0.6,0.6,1.0] } else { [0.0; 4] },
                }).collect();
            let indices = if textured { vec![0,1,2,0,2,3] } else {
                vertices.push(Vertex { pos: [0.0,0.0,-2.0], color: [0.6,0.6,0.6,1.0], ..Default::default() });
                vec![4,0,1,4,1,2,4,2,3,4,3,0]
            };
            // Native LightningFx uses an unlit, unfogged SRCALPHA/ONE actor,
            // with a bright fan centre and transparent black corner vertices.
            let mut model = Scene { meshes: vec![Mesh { vertices, submeshes: vec![Submesh {
                blend: Blend::Additive, lighting_enabled: false, fog_enabled: false,
                depth_write: Some(false), two_sided: true,
                ..Submesh::new(indices, textured.then_some(key))
            }] }], ..Default::default() };
            if textured {
                // RGB deliberately stays white even where alpha is zero: a
                // brightness-only emission mask would expose the entire quad.
                let rgba = (0..16).flat_map(|y| (0..16).flat_map(move |x|
                    [255,255,255,if (6..10).contains(&x) && (6..10).contains(&y) { 255 } else { 0 }])).collect();
                model.textures.insert(key, Texture { width: 16, height: 16, rgba, source: Default::default() });
            }
            let model_id = if textured { 2 } else { 1 };
            r.add_actor_model(model_id, &model);
            r.set_actors(vec![ActorFrame {
                id: 1, model: model_id, always: true,
                skin: Some(std::sync::Arc::new(model.meshes[0].vertices.clone())),
                ..Default::default()
            }]);
            let faithful = capture(&mut r, off);
            assert_ne!(empty, faithful, "lightning fixture must actually render: textured={textured}");
            assert_eq!(faithful, capture(&mut r, ao), "AO must not stamp additive lightning: textured={textured}");
            assert_eq!(faithful, capture(&mut r, shadows), "shadows must not stamp additive lightning: textured={textured}");
            let glowing = capture(&mut r, bloom);
            let emission = r.emissive_seed_rgba().unwrap().unwrap();
            let center = (32 * 64 + 32) * 4;
            assert!(emission[center..center+3].iter().any(|&value| value > 0),
                "lightning centre must emit: textured={textured}");
            assert_ne!(faithful, glowing, "lightning emission must reach bloom: textured={textured}");
            for y in 0..64 {
                for x in 0..64 {
                    // At 90 degrees FOV the fan occupies [16,48]².
                    // The quad's opaque texels occupy [28,36]²; [24,40]²
                    // conservatively includes their bilinear-filter footprint.
                    let support = if textured { 24..40 } else { 16..48 };
                    if !support.contains(&x) || !support.contains(&y) {
                        let pixel = (y * 64 + x) * 4;
                        assert_eq!(&emission[pixel..pixel+3], &[0,0,0],
                            "transparent lightning must emit exactly zero at ({x},{y}), textured={textured}");
                        assert_eq!(&faithful[pixel..pixel+4], &empty[pixel..pixel+4],
                            "transparent lightning must preserve dark sky at ({x},{y}), textured={textured}");
                    }
                }
            }
            if !textured {
                let corner = (17 * 64 + 17) * 4;
                assert!(emission[corner] < emission[center],
                    "native fan emission must fade towards transparent corner vertices");
            }
            r.set_actors(Vec::new());
        }
    }
    #[test]
    #[ignore = "explicit headless 120-warmup/600-sample GPU benchmark"]
    fn bloom_gpu_timing_1920x1200() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("offscreen bloom benchmark GPU");
        let size = (1920, 1200);
        let beauty = target(&r.device, size);
        let emission = target(&r.device, size);
        let presentation = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None,
            entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }] });
        let mut bloom = Bloom::new(&r.device, size, &beauty, &emission, &presentation, None);
        let mut encoder = r.device.create_command_encoder(&Default::default());
        {
            // Synthetic worst-coverage marked source, not a claim about a zone.
            let attachments = [&beauty, &emission].map(|view| Some(wgpu::RenderPassColorAttachment {
                view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
            }));
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None, color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
        }
        r.queue.submit([encoder.finish()]);
        for _ in 0..120 {
            let mut encoder = r.device.create_command_encoder(&Default::default());
            bloom.encode(&r.queue, &mut encoder, 0.25);
            r.queue.submit([encoder.finish()]);
        }
        r.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mut timer = crate::gpu_stats::FrameGpuTimer::new(&r.device, &r.queue).unwrap();
        let mut samples = Vec::with_capacity(600);
        for _ in 0..600 {
            let mut encoder = r.device.create_command_encoder(&Default::default());
            for (i, pass) in bloom.down.iter().enumerate() {
                let timestamps = if i == 0 { Some(timer.begin_pass(false)) } else { None };
                pass.encode(&mut encoder, &bloom.pipelines[usize::from(i != 0)], timestamps);
            }
            for pass in &bloom.up { pass.encode(&mut encoder, &bloom.pipelines[2], None); }
            bloom.composite.as_ref().unwrap().encode(&mut encoder, &bloom.pipelines[3], Some(timer.end_pass()));
            r.queue.submit([encoder.finish()]);
            samples.push(timer.read_ms(&r.device, &r.queue).unwrap());
        }
        samples.sort_by(f64::total_cmp);
        eprintln!("BLOOM_GPU_TIMING synthetic_full_coverage=1 size=1920x1200 samples=600 warmup=120 p50_ms={:.6} p95_ms={:.6} p99_ms={:.6} private_texture_bytes={} uniform_bytes=16 emission_mask_bytes={}",
            samples[299], samples[569], samples[593], bloom.bytes, 1920_u64 * 1200 * 8);
    }
}
