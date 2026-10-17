//! Opt-in pixel evidence from the last rendered sky, not a second cloud simulation.
use super::*;
use std::fmt::Write;

const FRAGMENT: &str = r#"
@group(2) @binding(2) var<uniform> probe_triangle: vec4<u32>;
// Only the diagnostic vertex entry consumes these padding words. Ordinary sky
// vertex code and its uploaded geometry/projection are left untouched.
@vertex
fn vs_probe(v: VIn) -> VOut {
    var out = sky_vertex(v);
    if probe_triangle.z != 0u {
        let near = bitcast<f32>(probe_triangle.y);
        let q = bitcast<f32>(probe_triangle.z);
        out.clip.z = q * (out.clip.w - near);
    }
    return out;
}
struct ProbeOut {
    @location(0) texture_data: vec4<f32>,
    @location(1) geometry_data: vec4<f32>,
    @location(2) source_data: vec4<f32>,
}
@fragment
fn fs_probe(i: VOut) -> ProbeOut {
    let uv = sky_uv(i);
    let sampled = sample_clamped(tex, uv);
    var base: vec4<f32>;
    if mat.flags.x > 0.5 {
        base = textureSampleLevel(tex, samp_clamp, uv, 0.0);
    } else {
        base = textureSampleLevel(tex, samp, uv, 0.0);
    }
    let size = vec2<f32>(textureDimensions(tex));
    let dx = dpdx(uv) * size;
    let dy = dpdy(uv) * size;
    var out: ProbeOut;
    out.texture_data = vec4<f32>(uv, sampled.a, base.a);
    out.geometry_data = vec4<f32>(f32(probe_triangle.x), i.color.a * mat.color.a, i.clip.z,
        clamp(log2(max(length(dx), length(dy))), 0.0, f32(textureNumLevels(tex) - 1u)));
    out.source_data = shade_sky(i, 2u);
    return out;
}
"#;

impl Renderer {
    /// Samples each actual GPU draw independently, after `render`, preserving its material,
    /// generated vertices, instance matrix and projection. Uncovered pixels have triangle=-1.
    /// LOD is the isotropic derivative LOD; the alpha is the actual hardware-filtered result
    /// (including anisotropy). No blend/depth rejection is applied to this diagnostic pass.
    pub(crate) fn sky_pixel_probe(&self, layer: usize, pixels: &[(u32, u32)], size: (u32, u32), decoded: &HashMap<TextureKey, ao_scene::Texture>, projection_far: Option<f32>) -> Result<String> {
        if let Some(far) = projection_far {
            anyhow::ensure!(far.is_finite() && far > self.lens.near, "probe projection far must be finite and exceed near");
        }
        anyhow::ensure!(pixels.iter().all(|&(x,y)| x < size.0 && y < size.1), "sky probe pixel outside render surface");
        let instances = self.sky.bufs.get(self.frame).context("render a sky frame before probing")?;
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("actual sky pixel probe"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{FRAGMENT}", crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl"))).into()),
        });
        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
        let instance_attrs = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let targets: [_; 3] = std::array::from_fn(|_| Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba32Float, blend: None, write_mask: wgpu::ColorWrites::ALL }));
        let tag_layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("portable triangle probe tag"),
            entries: &[wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true,
                    min_binding_size: std::num::NonZeroU64::new(16) }, count: None }],
        });
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("portable sky probe"),
            bind_group_layouts: &[Some(&self.g_layout), Some(&self.tex_layout), Some(&tag_layout)], immediate_size: 0,
        });
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("actual sky pixel probe"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_probe"), compilation_options: Default::default(), buffers: &[
                wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertex_attrs },
                wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance, attributes: &instance_attrs },
            ] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_probe"), compilation_options: Default::default(), targets: &targets }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let textures: Vec<_> = (0..3).map(|_| self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky probe float attachment"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        })).collect();
        let views: Vec<_> = textures.iter().map(|t| t.create_view(&Default::default())).collect();
        // Split only index ranges, not geometry: retain every overlapping cloud sheet,
        // including transparent ones which must not hide an earlier sheet in the report.
        let mut draws = Vec::new();
        for (submesh, &(d, k)) in self.sky.draws.iter().filter(|(_, k)| *k as usize == layer).enumerate() {
            if let Some(run) = self.sky.clouds.iter().find(|run| run.layer.mesh == d.mesh) {
                anyhow::ensure!(run.grid > 1, "cloud rendering is disabled");
                let count = ((run.grid - 1).pow(2) * 6) as u32;
                for first_index in (d.first_index..d.first_index + d.count).step_by(count as usize) {
                    draws.push((Draw { first_index, count, ..d }, k, submesh));
                }
            } else {
                draws.push((d, k, submesh));
            }
        }
        anyhow::ensure!(!draws.is_empty(), "no uploaded sky draws for layer {layer}");
        let slots = draws.len() * pixels.len() * 3;
        anyhow::ensure!(slots > 0, "at least one sky probe pixel required");
        // Tag each real indexed triangle with a portable dynamic uniform. No
        // primitive-index builtin/extension or modified geometry is needed.
        let stride = u64::from(self.device.limits().min_uniform_buffer_offset_alignment).max(16);
        let triangles = draws.iter().map(|(d, _, _)| d.count / 3).max().unwrap_or(0);
        let mut tags = vec![0u8; triangles as usize * stride as usize];
        for triangle in 0..triangles {
            let offset = triangle as usize * stride as usize;
            tags[offset..offset + 4].copy_from_slice(&triangle.to_le_bytes());
            tags[offset + 4..offset + 8].copy_from_slice(&self.lens.near.to_le_bytes());
            let q = projection_far.map_or(0.0, |far| far / (far - self.lens.near));
            tags[offset + 8..offset + 12].copy_from_slice(&q.to_le_bytes());
        }
        let tag_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("portable triangle IDs"), size: tags.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        self.queue.write_buffer(&tag_buffer, 0, &tags);
        let tag_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("portable triangle IDs"), layout: &tag_layout,
            entries: &[wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &tag_buffer, offset: 0, size: std::num::NonZeroU64::new(16),
            }) }],
        });
        let min_x = pixels.iter().map(|p| p.0).min().unwrap();
        let max_x = pixels.iter().map(|p| p.0).max().unwrap();
        let min_y = pixels.iter().map(|p| p.1).min().unwrap();
        let max_y = pixels.iter().map(|p| p.1).max().unwrap();
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("sky pixel probe readback"), size: slots as u64 * 256, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for (ordinal, (d, k, _)) in draws.iter().enumerate() {
            {
                let attachments: Vec<_> = views.iter().map(|view| Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: -1.0, g: -1.0, b: -1.0, a: -1.0 }), store: wgpu::StoreOp::Store } })).collect();
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("sky probe draw"), color_attachments: &attachments,
                    depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &self.globals_bg, &[]);
                pass.set_bind_group(1, &self.sky.mats[d.mat], &[]);
                let (vb, ib) = self.sky.meshes[d.mesh].as_ref().context("missing sky mesh")?;
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_vertex_buffer(1, instances.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                // Scissor does not change the viewport or interpolation/derivatives.
                pass.set_scissor_rect(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1);
                for triangle in 0..d.count / 3 {
                    pass.set_bind_group(2, &tag_group, &[(u64::from(triangle) * stride) as u32]);
                    let first = d.first_index + triangle * 3;
                    pass.draw_indexed(first..first + 3, 0, *k..*k + 1);
                }
            }
            for (p, &(x,y)) in pixels.iter().enumerate() {
                for (attachment, texture) in textures.iter().enumerate() {
                    encoder.copy_texture_to_buffer(wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: ((ordinal * pixels.len() + p) * 3 + attachment) as u64 * 256, bytes_per_row: Some(256), rows_per_image: Some(1) } },
                        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _ = tx.send(result); });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let bytes = readback.slice(..).get_mapped_range();
        let mut report = format!("\nGPU sky pixel probe: layer={layer} policy={:?} anisotropy={} msaa={} time={}\nsource=last-render GPU buffers, material bind groups and globals; independent unblended draws; triangle=-1 means no raster coverage\nfields=x,y,draw,mesh,pipe,triangle,global_triangle,sheet,uv,sampled_alpha,uploaded_base_alpha,diffuse_alpha,source_alpha,depth,isotropic_lod\n", self.texture_policy, self.anisotropy, self.msaa_samples, self.time);
        writeln!(report, "projection_only_far_override={projection_far:?} near={} geometry=unchanged,last-render-GPU-buffers viewport_xyw=unchanged", self.lens.near)?;
        for (ordinal, (d, _, submesh)) in draws.iter().enumerate() {
            let cloud = self.sky.clouds.iter().find(|run| run.layer.mesh == d.mesh && run.grid > 1);
            let material = self.sky.topology.get(d.mesh).and_then(|(_, materials)| materials.get(*submesh));
            if let Some(m) = material {
                writeln!(report, "actual_pipeline: blend={:?} alpha_test={} depth_write=false depth_compare=Always cull={} clamp={} fog={} source_alpha=texture.a*diffuse.a",
                    m.blend, m.blend == Blend::AlphaTest, if m.two_sided { "None" } else { "Back" }, m.texture_clamp, m.sky_fog)?;
            }
            let texture = material.and_then(|m| m.texture).and_then(|key| decoded.get(&key));
            if let Some(t) = texture {
                writeln!(report, "decoded_texture={}x{} source={:?} upload_encoding={:?}", t.width, t.height, t.source,
                    self.texture_policy.encoding(t.source, self.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC), t.width, t.height))?;
            }
            writeln!(report, "draw{ordinal}: first_index={} count={} material={} texture={:?}", d.first_index, d.count, d.mat, material.and_then(|m| m.texture))?;
            for (p, &(x,y)) in pixels.iter().enumerate() {
                let values: [[f32;4];3] = std::array::from_fn(|a| std::array::from_fn(|c| {
                    let offset = ((ordinal * pixels.len() + p) * 3 + a) * 256 + c * 4;
                    f32::from_le_bytes(bytes[offset..offset+4].try_into().unwrap())
                }));
                let [u,v,alpha,base] = values[0];
                let [triangle,diffuse,depth,lod] = values[1];
                let global = (triangle >= 0.0).then(|| d.first_index as usize / 3 + triangle as usize);
                let sheet = cloud.zip(global).map(|(run,t)| t / ((run.grid - 1).pow(2) * 2));
                let source_alpha = if triangle >= 0.0 { alpha * diffuse } else { -1.0 };
                writeln!(report, "{x},{y},draw{ordinal},mesh{},pipe{},triangle={triangle},global_triangle={global:?},sheet={sheet:?},uv=({u:.9},{v:.9}),sampled_alpha={alpha:.9},uploaded_base_alpha={base:.9},diffuse_alpha={diffuse:.9},source_alpha={source_alpha:.9},depth={depth:.9},isotropic_lod={lod:.9}", d.mesh, d.pipe)?;
                writeln!(report, "  source_rgba={:?} source_bits={:?}", values[2], values[2].map(f32::to_bits))?;
                if triangle >= 0.0 {
                    if let Some(t) = texture {
                        let clamp = material.is_some_and(|m| m.texture_clamp);
                        let axis = |uv: f32, n: u32| {
                            let address = if clamp { uv.clamp(0.0, 1.0) } else { uv.rem_euclid(1.0) };
                            (address * n as f32).floor().min((n - 1) as f32) as usize
                        };
                        let tx = axis(u, t.width);
                        let ty = axis(v, t.height);
                        let offset = (ty * t.width as usize + tx) * 4;
                        writeln!(report, "  decoded_nearest_texel=({tx},{ty}) rgba={:?}", &t.rgba[offset..offset+4])?;
                    }
                }
                if let Some((run,t)) = cloud.zip(global) {
                    let indices = &run.indices[usize::from(run.grid == 10)][t*3..t*3+3];
                    for &index in indices { writeln!(report, "  runtime_vertex{index}={:?}", run.vertices[index as usize])?; }
                }
            }
        }
        drop(bytes);
        readback.unmap();
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_pixel_probe_reads_uploaded_alpha_and_uncovered_pixels() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("pixel probe regression requires a GPU");
        let key = TextureKey { rdb_type: 0, id: 1 };
        let scene = Scene {
            textures: HashMap::from([(key, ao_scene::Texture { width: 4, height: 4,
                rgba: [20, 40, 60, 64].repeat(16), source: ao_scene::TextureSource::Rgba })]),
            meshes: vec![ao_scene::Mesh {
                vertices: [[-1.0, -1.0, -5.0], [1.0, -1.0, -5.0], [0.0, 1.0, -5.0]]
                    .map(|pos| ao_scene::Vertex { pos, uv: [0.5; 2], color: [1.0; 4], ..Default::default() }).into(),
                submeshes: vec![ao_scene::Submesh { blend: Blend::AlphaBlend, ..ao_scene::Submesh::new(vec![0, 1, 2], Some(key)) }],
            }],
            sky: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
            ..Default::default()
        };
        renderer.upload(&scene);
        renderer.set_lens(ao_scene::Lens { far: Some(1000.0), ..Default::default() });
        let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        renderer.render(&target.create_view(&Default::default()), &Targets::new(&renderer, 64, 64),
            &Camera::look_at(Vec3::ZERO, -Vec3::Z));
        let report = renderer.sky_pixel_probe(0, &[(32, 32), (0, 0)], (64, 64), &scene.textures, None).unwrap();
        let center = report.lines().find(|line| line.starts_with("32,32,")).unwrap();
        for field in ["sampled_alpha=", "uploaded_base_alpha=", "source_alpha="] {
            let alpha = center.split_once(field).unwrap().1.split(',').next().unwrap().parse::<f32>().unwrap();
            assert!((alpha - 64.0 / 255.0).abs() < 1e-5, "{field}: {center}");
        }
        assert!(center.contains("triangle=0,"));
        let source = report.lines().skip_while(|line| !line.starts_with("32,32,")).nth(1).unwrap();
        let rgba = source.strip_prefix("  source_rgba=[").unwrap().split_once(']').unwrap().0;
        for (actual, expected) in rgba.split(", ").zip([20.0, 40.0, 60.0, 64.0]) {
            assert!((actual.parse::<f32>().unwrap() - expected / 255.0).abs() < 1e-5, "{source}");
        }
        assert!(source.contains("source_bits="), "{source}");
        assert!(report.lines().any(|line| line.starts_with("0,0,") && line.contains("triangle=-1,") && line.contains("source_alpha=-1.000000000,")));
    }

    #[test]
    fn sky_pixel_probe_projection_far_recovers_fixed_straddling_geometry() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("pixel probe regression requires a GPU");
        // One sloped quad crosses far1000. All positions are uploaded once;
        // extending only diagnostic projection to1400 must recover its upper half.
        let scene = Scene {
            meshes: vec![ao_scene::Mesh {
                vertices: [[-800.0, -800.0, -800.0], [800.0, -800.0, -800.0],
                    [1200.0, 1200.0, -1200.0], [-1200.0, 1200.0, -1200.0]]
                    .map(|pos| ao_scene::Vertex { pos, uv: [0.5; 2], color: [1.0; 4], ..Default::default() }).into(),
                submeshes: vec![ao_scene::Submesh { blend: Blend::AlphaBlend,
                    ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
            }],
            sky: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
            ..Default::default()
        };
        renderer.upload(&scene);
        renderer.set_lens(ao_scene::Lens { fov: std::f32::consts::FRAC_PI_2, horizontal: true,
            near: 0.25, far: Some(1000.0) });
        let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        renderer.render(&target.create_view(&Default::default()), &Targets::new(&renderer, 64, 64),
            &Camera::look_at(Vec3::ZERO, -Vec3::Z));
        let pixels = [(32, 16), (32, 48)];
        let original = renderer.sky_pixel_probe(0, &pixels, (64, 64), &scene.textures, None).unwrap();
        let extended = renderer.sky_pixel_probe(0, &pixels, (64, 64), &scene.textures, Some(1400.0)).unwrap();
        let restored = renderer.sky_pixel_probe(0, &pixels, (64, 64), &scene.textures, None).unwrap();
        let row = |report: &str, prefix: &str| report.lines().find(|line| line.starts_with(prefix)).unwrap().to_owned();
        assert!(row(&original, "32,16,").contains("triangle=-1,"));
        assert!(!row(&extended, "32,16,").contains("triangle=-1,"));
        assert!(row(&extended, "32,16,").contains("source_alpha=1.000000000,"));
        assert!(!row(&original, "32,48,").contains("triangle=-1,"));
        // Neither probe may mutate the live render projection or uploaded geometry.
        assert_eq!(row(&original, "32,16,"), row(&restored, "32,16,"));
        assert_eq!(row(&original, "32,48,"), row(&restored, "32,48,"));
    }
}
