//! wgpu renderer for `ao_scene::Scene`: windowed free-fly viewer (`viewer`) and offscreen PNG path.

mod viewer;

use anyhow::{anyhow, Context, Result};
use ao_scene::{Scene, TextureKey};
pub use glam::Vec3;
use glam::{Mat4, Vec4};
use std::collections::HashMap;
use std::path::Path;
use wgpu::util::DeviceExt;

pub use viewer::run_viewer;

const MSAA: u32 = 4;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SKY_SRGB: [f32; 3] = [0.53, 0.72, 0.92];

fn srgb_to_linear(c: f32) -> f32 {
    c.powf(2.2)
}

/// Free-fly camera. Forward = (sin yaw·cos pitch, sin pitch, -cos yaw·cos pitch).
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl Camera {
    pub fn look_at(eye: Vec3, at: Vec3) -> Self {
        let d = (at - eye).normalize_or_zero();
        let d = if d == Vec3::ZERO { -Vec3::Z } else { d };
        Self { pos: eye, yaw: d.x.atan2(-d.z), pitch: d.y.asin() }
    }
    pub fn forward(&self) -> Vec3 {
        Vec3::new(self.yaw.sin() * self.pitch.cos(), self.pitch.sin(), -self.yaw.cos() * self.pitch.cos())
    }
    pub fn right(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin())
    }
}

/// World-space AABB over all instances (None when the scene draws nothing).
pub fn scene_bounds(scene: &Scene) -> Option<(Vec3, Vec3)> {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    // Per-mesh local AABB first, then transform its 8 corners per instance.
    let local: Vec<Option<(Vec3, Vec3)>> = scene
        .meshes
        .iter()
        .map(|m| {
            m.vertices.iter().fold(None, |a, v| {
                let p = Vec3::from(v.pos);
                Some(a.map_or((p, p), |(l, h): (Vec3, Vec3)| (l.min(p), h.max(p))))
            })
        })
        .collect();
    for inst in &scene.instances {
        let Some(Some((l, h))) = local.get(inst.mesh) else { continue };
        let m = Mat4::from_cols_array_2d(&inst.transform);
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 == 0 { l.x } else { h.x },
                if i & 2 == 0 { l.y } else { h.y },
                if i & 4 == 0 { l.z } else { h.z },
            );
            let w = m.transform_point3(c);
            lo = lo.min(w);
            hi = hi.max(w);
        }
    }
    (lo.x <= hi.x).then_some((lo, hi))
}

/// Default eye/target: the spawn point if set, else a 3/4 view framing the bounds.
pub fn default_view(scene: &Scene) -> (Vec3, Vec3) {
    let Some((lo, hi)) = scene_bounds(scene) else { return (Vec3::new(0.0, 2.0, 5.0), Vec3::ZERO) };
    let center = (lo + hi) * 0.5;
    let r = ((hi - lo).length() * 0.5).max(0.5);
    match scene.spawn {
        Some(s) => {
            let s = Vec3::from(s);
            (s, Vec3::new(center.x, s.y, center.z))
        }
        None => (center + Vec3::new(0.6, 0.5, 1.0).normalize() * r * 1.8, center),
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    sun: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
}

struct Draw {
    mesh: usize,
    first_index: u32,
    count: u32,
    bind: usize,
    alpha: bool,
    inst: std::ops::Range<u32>,
}

struct Gpu {
    meshes: Vec<Option<(wgpu::Buffer, wgpu::Buffer)>>,
    inst_buf: Option<wgpu::Buffer>,
    binds: Vec<wgpu::BindGroup>, // [0] = untextured fallback
    draws: Vec<Draw>,
    radius: f32,
}

/// Colour + depth attachments sized to the output.
pub struct Targets {
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    pub size: (u32, u32),
}

impl Targets {
    pub fn new(r: &Renderer, w: u32, h: u32) -> Self {
        let mk = |format, label| {
            r.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: MSAA,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Self { msaa: mk(r.format, "msaa"), depth: mk(DEPTH, "depth"), size: (w, h) }
    }
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipe_opaque: wgpu::RenderPipeline,
    pipe_alpha: wgpu::RenderPipeline,
    gpu: Gpu,
}

impl Renderer {
    /// Picks the Metal adapter (compatible with `surface` when windowed) and builds pipelines.
    pub fn new(instance: &wgpu::Instance, surface: Option<&wgpu::Surface>) -> Result<Self> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| anyhow!("no suitable GPU adapter"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .context("request_device")?;
        let format = match surface {
            Some(s) => {
                let caps = s.get_capabilities(&adapter);
                *caps.formats.iter().find(|f| f.is_srgb()).unwrap_or(&caps.formats[0])
            }
            None => wgpu::TextureFormat::Rgba8UnormSrgb,
        };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let g_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &g_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&g_layout, &tex_layout],
            push_constant_ranges: &[],
        });
        let f4 = |o| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: o, shader_location: 3 + (o / 16) as u32 };
        let inst_attrs = [f4(0), f4(16), f4(32), f4(48)];
        let vert_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let mk = |fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[
                        wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<ao_scene::Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &vert_attrs,
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: 64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &inst_attrs,
                        },
                    ],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: wgpu::PrimitiveState::default(), // two-sided: no culling
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false },
                multiview: None,
                cache: None,
            })
        };
        let (pipe_opaque, pipe_alpha) = (mk("fs_opaque"), mk("fs_alpha"));
        let mut r = Self {
            device,
            queue,
            format,
            globals,
            globals_bg,
            tex_layout,
            sampler,
            pipe_opaque,
            pipe_alpha,
            gpu: Gpu { meshes: vec![], inst_buf: None, binds: vec![], draws: vec![], radius: 100.0 },
        };
        r.upload(&Scene::default());
        Ok(r)
    }

    fn texture_bind(&self, rgba: &[u8], w: u32, h: u32) -> wgpu::BindGroup {
        let mips = 32 - w.max(h).leading_zeros();
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // CPU box-filter mip chain (sRGB-space average; fine for diffuse art).
        let (mut cw, mut ch, mut data) = (w, h, rgba.to_vec());
        for level in 0..mips {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(cw * 4), rows_per_image: Some(ch) },
                wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
            );
            let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
            let mut next = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut s = 0u32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (sx, sy) = ((2 * x + dx).min(cw - 1), (2 * y + dy).min(ch - 1));
                            s += data[((sy * cw + sx) * 4 + c) as usize] as u32;
                        }
                        next[((y * nw + x) * 4 + c) as usize] = ((s + 2) / 4) as u8;
                    }
                }
            }
            (cw, ch, data) = (nw, nh, next);
        }
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tex.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Replaces the GPU-side scene.
    pub fn upload(&mut self, scene: &Scene) {
        let mut binds = vec![self.texture_bind(&[190, 190, 190, 255], 1, 1)];
        let mut bind_of: HashMap<TextureKey, usize> = HashMap::new();
        for (k, t) in &scene.textures {
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize {
                bind_of.insert(*k, binds.len());
                binds.push(self.texture_bind(&t.rgba, t.width, t.height));
            }
        }

        // Instances grouped by mesh -> contiguous ranges in one instance buffer.
        let mut by_mesh: Vec<Vec<[[f32; 4]; 4]>> = vec![vec![]; scene.meshes.len()];
        for i in &scene.instances {
            if let Some(v) = by_mesh.get_mut(i.mesh) {
                v.push(i.transform);
            }
        }
        let mut all: Vec<[[f32; 4]; 4]> = vec![];
        let mut meshes = vec![];
        let mut draws = vec![];
        for (mi, mesh) in scene.meshes.iter().enumerate() {
            let range = all.len() as u32..(all.len() + by_mesh[mi].len()) as u32;
            all.extend_from_slice(&by_mesh[mi]);
            let idx_total: usize = mesh.submeshes.iter().map(|s| s.indices.len()).sum();
            if mesh.vertices.is_empty() || idx_total == 0 || range.is_empty() {
                meshes.push(None);
                continue;
            }
            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&vertex_bytes(&mesh.vertices)),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let mut indices = Vec::with_capacity(idx_total);
            for s in &mesh.submeshes {
                let first = indices.len() as u32;
                let nv = mesh.vertices.len() as u32;
                // Drop whole triangles that index out of range rather than crash the GPU.
                for t in s.indices.chunks_exact(3) {
                    if t.iter().all(|&i| i < nv) {
                        indices.extend_from_slice(t);
                    }
                }
                let count = indices.len() as u32 - first;
                if count > 0 {
                    draws.push(Draw {
                        mesh: mi,
                        first_index: first,
                        count,
                        bind: s.texture.and_then(|k| bind_of.get(&k).copied()).unwrap_or(0),
                        alpha: s.alpha_test,
                        inst: range.clone(),
                    });
                }
            }
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            meshes.push(Some((vb, ib)));
        }
        draws.sort_by_key(|d| d.alpha); // opaque first, then cutouts
        let inst_buf = (!all.is_empty()).then(|| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&all),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let radius = scene_bounds(scene).map_or(100.0, |(l, h)| ((h - l).length() * 0.5).max(1.0));
        self.gpu = Gpu { meshes, inst_buf, binds, draws, radius };
    }

    /// Scene radius; viewer uses it for speed and far plane.
    pub fn radius(&self) -> f32 {
        self.gpu.radius
    }

    /// Draws one frame into `resolve` (a view of the output texture).
    pub fn render(&self, resolve: &wgpu::TextureView, t: &Targets, cam: &Camera) {
        let fog_end = (self.gpu.radius * 2.0).max(300.0);
        let far = fog_end * 1.5;
        let aspect = t.size.0 as f32 / t.size.1.max(1) as f32;
        let vp = Mat4::perspective_rh(60f32.to_radians(), aspect, 0.2, far) * Mat4::look_to_rh(cam.pos, cam.forward(), Vec3::Y);
        let sky = SKY_SRGB.map(srgb_to_linear);
        let sun = Vec3::new(0.4, 0.8, 0.3).normalize();
        let g = Globals {
            view_proj: vp.to_cols_array_2d(),
            eye: cam.pos.extend(1.0).to_array(),
            sun: sun.extend(0.0).to_array(),
            fog_color: Vec4::new(sky[0], sky[1], sky[2], 1.0).to_array(),
            fog: [fog_end * 0.35, fog_end, 0.0, 0.0],
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    resolve_target: Some(resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: sky[0] as f64, g: sky[1] as f64, b: sky[2] as f64, a: 1.0 }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(inst) = &self.gpu.inst_buf {
                pass.set_bind_group(0, &self.globals_bg, &[]);
                pass.set_vertex_buffer(1, inst.slice(..));
                let (mut cur_mesh, mut cur_alpha, mut cur_bind) = (usize::MAX, None, usize::MAX);
                for d in &self.gpu.draws {
                    if cur_alpha != Some(d.alpha) {
                        pass.set_pipeline(if d.alpha { &self.pipe_alpha } else { &self.pipe_opaque });
                        cur_alpha = Some(d.alpha);
                    }
                    if cur_mesh != d.mesh {
                        let (vb, ib) = self.gpu.meshes[d.mesh].as_ref().unwrap();
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                        cur_mesh = d.mesh;
                    }
                    if cur_bind != d.bind {
                        pass.set_bind_group(1, &self.gpu.binds[d.bind], &[]);
                        cur_bind = d.bind;
                    }
                    pass.draw_indexed(d.first_index..d.first_index + d.count, 0, d.inst.clone());
                }
            }
        }
        self.queue.submit([enc.finish()]);
    }
}

fn vertex_bytes(v: &[ao_scene::Vertex]) -> Vec<f32> {
    v.iter().flat_map(|v| [v.pos[0], v.pos[1], v.pos[2], v.normal[0], v.normal[1], v.normal[2], v.uv[0], v.uv[1]]).collect()
}

/// Renders `scene` offscreen and writes an sRGB PNG.
pub fn render_to_png(scene: &Scene, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path) -> Result<()> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let mut r = Renderer::new(&instance, None)?;
    r.upload(scene);
    let targets = Targets::new(&r, width, height);
    let out = r.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: r.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    r.render(&out.create_view(&Default::default()), &targets, &Camera::look_at(eye.into(), look_at.into()));

    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = r.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = r.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        out.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    r.queue.submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |res| tx.send(res).unwrap());
    r.device.poll(wgpu::Maintain::Wait);
    rx.recv()??;
    let mapped = buf.slice(..).get_mapped_range();
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        pixels.extend_from_slice(&mapped[(y * row) as usize..(y * row + width * 4) as usize]);
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&pixels)?;
    Ok(())
}
