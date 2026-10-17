//! Optional UNORM boundary. Keep native per-draw saturation and quantization;
//! auxiliary float masks never change the faithful colour target's blend semantics.
use crate::{gpu_stats::{GpuStatsExt, TextureViewStatsExt}, DEPTH};
pub(super) const AUX: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(super) struct EnhancedFrame {
    pub size: (u32, u32),
    pub sample_count: u32,
    pub colour: wgpu::TextureView,
    pub ambient: Option<wgpu::TextureView>,
    pub emissive: Option<wgpu::TextureView>,
    pub depth: wgpu::TextureView,
    pub resolved_depth: wgpu::TextureView,
    pub msaa: Option<[Option<wgpu::TextureView>; 3]>,
    pub composite: wgpu::RenderPipeline,
    composite_bg: wgpu::BindGroup,
    depth_resolve: Option<(wgpu::RenderPipeline, wgpu::BindGroup)>,
}
impl EnhancedFrame {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32), samples: u32, settings: crate::EnhancedSettings) -> Self {
        let make = |format, count, label| device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: count, dimension: wgpu::TextureDimension::D2, format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view_tracked(&Default::default());
        let colour = make(format, 1, "Enhanced native UNORM colour");
        let ambient = settings.ambient_occlusion.then(|| make(AUX, 1, "Enhanced ambient share"));
        let emissive = settings.emissive_bloom.then(|| make(AUX, 1, "Enhanced authored emission"));
        let depth = make(DEPTH, samples, "Enhanced sampleable depth");
        let resolved_depth = if samples == 1 { depth.clone() } else { make(DEPTH, 1, "Enhanced resolved depth") };
        let msaa = (samples > 1).then(|| [Some(make(format, samples, "Enhanced MSAA native colour")), settings.ambient_occlusion.then(|| make(AUX, samples, "Enhanced MSAA ambient")), settings.emissive_bloom.then(|| make(AUX, samples, "Enhanced MSAA emission"))]);
        let (composite, composite_bg) = pipeline(device, format, &colour, false);
        let depth_resolve = (samples > 1).then(|| pipeline(device, DEPTH, &depth, true));
        Self { size, sample_count: samples, colour, ambient, emissive, depth, resolved_depth, msaa, composite, composite_bg, depth_resolve }
    }
    pub fn attachment(&self, index: usize) -> Option<wgpu::RenderPassColorAttachment<'_>> {
        let resolved = [Some(&self.colour), self.ambient.as_ref(), self.emissive.as_ref()][index]?;
        let msaa = self.msaa.as_ref().and_then(|views| views[index].as_ref());
        Some(wgpu::RenderPassColorAttachment { view: msaa.unwrap_or(resolved), resolve_target: msaa.map(|_| resolved), depth_slice: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: if msaa.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store } } })
    }
    pub fn resolve_depth(&self, enc: &mut wgpu::CommandEncoder) {
        let Some((pipe, bg)) = &self.depth_resolve else { return; };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Enhanced depth resolve"), color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.resolved_depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None,
            }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(pipe); pass.set_bind_group(0, bg, &[]); pass.draw(0..3, 0..1);
    }
    pub fn composite(&self, enc: &mut wgpu::CommandEncoder, output: &wgpu::TextureView, input: Option<&wgpu::BindGroup>, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Enhanced native colour presentation"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: timestamps, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&self.composite); pass.set_bind_group(0, input.unwrap_or(&self.composite_bg), &[]); pass.draw(0..3, 0..1);
    }
}
fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat, input: &wgpu::TextureView, depth: bool) -> (wgpu::RenderPipeline, wgpu::BindGroup) {
    let source = if depth {
        "@group(0) @binding(0) var image: texture_depth_multisampled_2d;\n@fragment fn fs(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 { var z=1.0; for(var s=0u;s<textureNumSamples(image);s+=1u){z=min(z,textureLoad(image,vec2<i32>(p.xy),i32(s)));} return z; }"
    } else {
        "@group(0) @binding(0) var image: texture_2d<f32>;\n@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return clamp(textureLoad(image,vec2<i32>(p.xy),0),vec4<f32>(0.0),vec4<f32>(1.0)); }"
    };
    let source = format!("{source}\n@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{{let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));return vec4<f32>(p[i],0.0,1.0);}}");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Enhanced boundary"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Enhanced boundary"), entries: &[wgpu::BindGroupLayoutEntry {
        binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: if depth { wgpu::TextureSampleType::Depth } else { wgpu::TextureSampleType::Float { filterable: false } }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: depth }, count: None,
    }] });
    let bg = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("Enhanced boundary"), layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) }] });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Enhanced boundary"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    let targets = [Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })];
    let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Enhanced boundary"), layout: Some(&pl),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: if depth { &[] } else { &targets } }),
        primitive: Default::default(), depth_stencil: depth.then_some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
        multisample: Default::default(), multiview_mask: None, cache: None,
    });
    (pipe, bg)
}

#[cfg(test)]
mod tests {
    use crate::*;
    #[test]
    fn neutral_unorm_pixels_and_resource_retirement() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("Enhanced regression needs offscreen GPU");
        let scene = Scene { meshes: vec![ao_scene::Mesh {
            vertices: [(-0.5,-0.5),(0.5,-0.5),(0.5,0.5),(-0.5,0.5)].into_iter().map(|(x,y)|
                ao_scene::Vertex { pos: [x,y,0.0], normal: [0.0,0.0,1.0], ..Default::default() }).collect(),
            submeshes: vec![ao_scene::Submesh { two_sided: true, emissive: [0.5,0.2,0.1],
                ..ao_scene::Submesh::new(vec![0,1,2,0,2,3], None) }],
        }], ..Default::default() };
        r.add_actor_model(1, &scene);
        let mut actor = ao_scene::ActorFrame { id: 1, model: 1, ..Default::default() };
        actor.transform[3][2] = -3.0;
        r.set_actors(vec![actor]);
        let capture = |r: &mut Renderer| {
            let output = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: Some("Enhanced pixel oracle"), size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
            });
            let targets = Targets::new(r,32,32);
            r.render(&output.create_view_tracked(&Default::default()), &targets, &Camera::look_at(Vec3::ZERO,Vec3::NEG_Z));
            texture_to_rgba(r,&output,32,32).unwrap()
        };
        for samples in r.msaa_supported.clone() {
            r.set_msaa_samples(samples);
            let faithful = capture(&mut r);
            assert!(r.enhanced_frame.is_none() && r.enhanced_shader.is_none());
            assert_eq!(r.enhanced_target_bytes(), 0);
            r.set_enhanced_settings(EnhancedSettings { ground_grass: true, grass_density: 0.0, ..Default::default() });
            let neutral = capture(&mut r);
            let sample_count = u64::from(r.msaa_samples());
            let expected = 32 * 32 * (4 + 4 * sample_count + if sample_count > 1 { 4 * sample_count + 4 } else { 0 });
            assert_eq!(r.enhanced_target_bytes(), expected, "neutral allocates no ambient/emission masks");
            assert_eq!(faithful, neutral, "neutral UNORM must retain native blend/quantization at {samples}x");
            r.set_enhanced_settings(EnhancedSettings::default());
            assert!(r.enhanced_frame.is_none() && r.enhanced_shader.is_none());
            assert_eq!(r.enhanced_target_bytes(), 0);
            assert_eq!(faithful,capture(&mut r),"all-off roundtrip must be byte identical");
        }
    }

    /// Native-world neutral-boundary evidence, not a retail visual-acceptance oracle.
    /// Run alone with `AOMAC_GFX_CORE_OUT=$HOME/aomac-handoff/gfx/<new-run>`.
    #[test]
    #[ignore = "installed ICC pixel/GPU benchmark; writes external evidence; run alone"]
    fn native_icc_neutral_pixels_and_gpu_timing() -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        use std::{fmt::Write, path::PathBuf, time::Instant};
        const WIDTH: u32 = 1920;
        const HEIGHT: u32 = 1200;
        const WARMUP: usize = 120;
        const SAMPLES: usize = 600;
        let out = PathBuf::from(std::env::var_os("AOMAC_GFX_CORE_OUT").context("set AOMAC_GFX_CORE_OUT to a new external directory")?);
        let evidence_root = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?).join("aomac-handoff/gfx");
        ensure!(out.starts_with(&evidence_root) && !out.components().any(|part| part == std::path::Component::ParentDir),
            "output must be under {}", evidence_root.display());
        ensure!(!out.exists(), "refusing to overwrite {}", out.display());
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("status.txt"), "status=running\nvisual_acceptance=unexercised\n")?;
        let result = (|| -> anyhow::Result<()> {
            let dir = ao_gui::client_dir();
            let store = ao_rdb::RecordStore::open(&dir)?;
            ao_scene::crt_srand(1);
            let weather = ao_formats::weather::State::clear();
            let (scene, report) = ao_formats::playfield::load_playfield_report_with_weather(
                &store, &dir, 4582, 2648.69, 0, Some(weather))?;
            ensure!(!scene.meshes.is_empty() && !scene.instances.is_empty(), "ICC snapshot must contain native world geometry");
            let camera = Camera { pos: Vec3::new(933.5, 26.0, -748.0), yaw: 3.765, pitch: 0.046, roll: 0.0 };
            let lens = ao_scene::Lens { fov: std::f32::consts::FRAC_PI_2, horizontal: true, near: 0.2, far: Some(800.0) };
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let mut renderers = [Renderer::new(&instance, None)?, Renderer::new(&instance, None)?];
            for (index, r) in renderers.iter_mut().enumerate() {
                ao_scene::crt_srand(1);
                r.upload(&scene);
                r.set_lens(lens);
                r.set_msaa_samples(1);
                r.time = 0.0;
                if index == 1 {
                    r.set_enhanced_settings(EnhancedSettings { ground_grass: true, grass_density: 0.0, ..Default::default() });
                }
                r.set_gpu_timing(false)?;
            }
            let outputs: Vec<_> = renderers.iter().map(|r| {
                let texture = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                    label: Some("native ICC neutral benchmark"), size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
                });
                let view = texture.create_view_tracked(&Default::default());
                (texture, view, Targets::new(r, WIDTH, HEIGHT))
            }).collect();
            // Capture the identical first frame before any timing work or counter advancement.
            for (index, r) in renderers.iter_mut().enumerate() {
                r.render(&outputs[index].1, &outputs[index].2, &camera);
            }
            let mut pixels = Vec::with_capacity(2);
            for (index, r) in renderers.iter().enumerate() {
                let name = ["off", "neutral"][index];
                let raw = texture_to_rgba(r, &outputs[index].0, WIDTH, HEIGHT)?;
                std::fs::write(out.join(format!("{name}.rgba")), &raw)?;
                texture_to_png(r, &outputs[index].0, WIDTH, HEIGHT, &out.join(format!("{name}.png")))?;
                pixels.push(raw);
            }
            let max_error = pixels[0].iter().zip(&pixels[1]).map(|(a,b)| a.abs_diff(*b)).max().unwrap_or(0);
            let total_error: u64 = pixels[0].iter().zip(&pixels[1]).map(|(a,b)| u64::from(a.abs_diff(*b))).sum();
            let differing = pixels[0].iter().zip(&pixels[1]).filter(|(a,b)| a != b).count();
            let mean_error = total_error as f64 / pixels[0].len() as f64;
            let mut manifest = format!("fixture=native_icc_neutral\npf=4582\ngame_time=2648.69\ngame_day=0\nrenderer_time=0\ncrt_seed=1\nweather={weather:?}\nsize={WIDTH}x{HEIGHT}\ncamera={camera:?}\nlens={lens:?}\nmsaa=1\nwarmup_per_profile={WARMUP}\nsamples_per_profile={SAMPLES}\norder=off,neutral,neutral,off\ncpu_interval=render_encode_submit_excludes_gpu_wait\nvisual_acceptance=unexercised\nload_report={report:?}\npixel_max_byte_error={max_error}\npixel_mean_byte_error={mean_error:.9}\npixel_differing_bytes={differing}\n");
            let revision = std::process::Command::new("git")
                .current_dir(env!("CARGO_MANIFEST_DIR")).args(["rev-parse", "HEAD"]).output()
                .context("query fixture source revision")?;
            ensure!(revision.status.success(), "fixture revision query failed: {}", String::from_utf8_lossy(&revision.stderr));
            writeln!(manifest, "revision={}", String::from_utf8(revision.stdout)?.trim())?;
            writeln!(manifest, "platform_os={}\nplatform_arch={}", std::env::consts::OS, std::env::consts::ARCH)?;
            writeln!(manifest, "device_features={:?}", renderers[0].device.features())?;
            writeln!(manifest, "pixel_frame=first\npixel_timing_enabled=false\npixel_exact_match={}", max_error == 0)?;
            for (index, r) in renderers.iter().enumerate() {
                writeln!(manifest, "{}_optional_attachment_bytes={}", ["off", "neutral"][index], r.enhanced_target_bytes())?;
            }
            // Preserve pixel acceptance independently of timestamp support/readback failures.
            std::fs::write(out.join("manifest.txt"), format!("{manifest}timing_status=pending\n"))?;
            ensure!(max_error == 0, "neutral native UNORM ICC differs by {max_error} byte levels");
            for r in &mut renderers {
                r.set_gpu_timing(true)?;
            }
            let mut measurements = [Vec::with_capacity(SAMPLES), Vec::with_capacity(SAMPLES)];
            let mut samples = String::from("pair,profile,cpu_render_encode_submit_ms,gpu_frame_ms\n");
            for pair in 0..WARMUP + SAMPLES {
                // Alternating pairs produce off/neutral/neutral/off, keeping each renderer persistent.
                for index in if pair % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let r = &mut renderers[index];
                    let (_, view, targets) = &outputs[index];
                    let start = Instant::now();
                    r.render(view, targets, &camera);
                    let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
                    let gpu_ms = r.read_frame_gpu_ms()?.context("GPU timing unexpectedly disabled")?;
                    if pair >= WARMUP {
                        measurements[index].push((cpu_ms, gpu_ms));
                        writeln!(samples, "{},{},{cpu_ms:.9},{gpu_ms:.9}", pair - WARMUP, ["off", "neutral"][index])?;
                    }
                }
            }
            for (index, values) in measurements.iter().enumerate() {
                for (metric, column) in [("cpu_render_encode_submit_ms", 0), ("gpu_frame_ms", 1)] {
                    let mut sorted: Vec<_> = values.iter().map(|&(cpu, gpu)| if column == 0 { cpu } else { gpu }).collect();
                    sorted.sort_by(f64::total_cmp);
                    for percentile in [50usize, 95, 99] {
                        let value = sorted[(sorted.len() * percentile).div_ceil(100) - 1];
                        writeln!(manifest, "{}_{metric}_p{percentile}={value:.9}", ["off", "neutral"][index])?;
                    }
                }
            }
            std::fs::write(out.join("samples.csv"), samples)?;
            writeln!(manifest, "timing_status=complete")?;
            std::fs::write(out.join("manifest.txt"), &manifest)?;
            eprintln!("{manifest}");
            Ok(())
        })();
        std::fs::write(out.join("status.txt"), match &result {
            Ok(()) => "status=complete\nvisual_acceptance=unexercised\n".to_owned(),
            Err(error) => format!("status=failed\nerror={error:#}\nvisual_acceptance=unexercised\n"),
        })?;
        result
    }
}
