//! wgpu renderer for `ao_scene::Scene`: windowed free-fly viewer (`viewer`) and offscreen PNG path.

mod viewer;

use anyhow::{anyhow, Context, Result};
use ao_scene::{Blend, Environment, Scene, TextureKey};
pub use glam::Vec3;
use glam::{Mat4, Vec4};
use std::collections::HashMap;
use std::path::Path;
use wgpu::util::DeviceExt;

pub use egui;
pub use viewer::{run_frontend, run_viewer, Frontend, Host};

const MSAA: u32 = 4;
const INST_RING: usize = 3;
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

/// Default eye/target: `spawn` (+ `spawn_look_at`) if set, else a 3/4 view framing the bounds (distance 2.4 r: the bounding sphere needs r/sin(30°) = 2 r for the 60° vertical FOV, plus margin; 16:10 or wider).
pub fn default_view(scene: &Scene) -> (Vec3, Vec3) {
    let Some((lo, hi)) = scene_bounds(scene) else { return (scene.spawn.map_or(Vec3::new(0.0, 2.0, 5.0), Vec3::from), scene.spawn_look_at.map_or(Vec3::ZERO, Vec3::from)) };
    let center = (lo + hi) * 0.5;
    let r = ((hi - lo).length() * 0.5).max(0.5);
    match scene.spawn {
        Some(s) => {
            let s = Vec3::from(s);
            (s, scene.spawn_look_at.map_or(Vec3::new(center.x, s.y, center.z), Vec3::from))
        }
        None => (center + Vec3::new(0.6, 0.5, 1.0).normalize() * r * 2.4, center),
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    ambient: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
    grid: [f32; 4],
    dims: [i32; 4],
}

/// One submesh draw. `pipe` = `blend as usize * 2 + two_sided as usize`; sky draws use `SKY_PIPE + blend`.
#[derive(Clone, Copy)]
struct Draw {
    mesh: usize,
    first_index: u32,
    count: u32,
    mat: usize,
    pipe: usize,
}

/// Pipelines 0..8 are scene (blend x cull); 8..12 are sky (per blend, two-sided, no depth).
const SKY_PIPE: usize = 8;
/// Lights kept per grid cell (strongest first) and the minimum cell edge in metres.
const CELL_LIGHTS: usize = 16;
const MIN_CELL: f32 = 8.0;
const MAX_CELLS: usize = 1 << 20;

/// Static light grid: dense 3D cells, each listing up to `CELL_LIGHTS` lights whose sphere touches it.
struct LightGrid {
    lights: Vec<[f32; 4]>, // 4 per light: (pos, range), (colour, cos(phi/2) | 2 = no cone), (atten0..2, cos(theta/2)), (axis, 0)
    cells: Vec<[u32; 2]>,  // (first index, count)
    idx: Vec<u32>,
    origin: Vec3,
    cell: f32,
    dims: [i32; 3],
}

impl LightGrid {
    fn new(lights: &[ao_scene::Light]) -> Self {
        let lights: Vec<_> = lights.iter().filter(|l| l.range > 0.0 && l.pos.iter().all(|v| v.is_finite())).collect();
        if lights.is_empty() {
            return Self { lights: vec![[0.0; 4]; 4], cells: vec![[0, 0]], idx: vec![0], origin: Vec3::ZERO, cell: 1.0, dims: [0; 3] };
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for l in &lights {
            lo = lo.min(Vec3::from(l.pos) - l.range);
            hi = hi.max(Vec3::from(l.pos) + l.range);
        }
        let mut cell = MIN_CELL;
        let dims_for = |c: f32| ((hi - lo) / c).ceil().max(Vec3::ONE);
        while { let d = dims_for(cell); (d.x * d.y * d.z) as usize > MAX_CELLS } {
            cell *= 2.0;
        }
        let d = dims_for(cell);
        let dims = [d.x as i32, d.y as i32, d.z as i32];
        let cell_of = |x: i32, y: i32, z: i32| ((z * dims[1] + y) * dims[0] + x) as u32;
        // (cell, score, light): score ranks lights inside a cell, brightest and nearest to the cell centre first.
        let mut pairs: Vec<(u32, f32, u32)> = vec![];
        for (li, l) in lights.iter().enumerate() {
            let p = Vec3::from(l.pos);
            let lum = l.color[0] + l.color[1] + l.color[2];
            let d3d = l.atten != [0.0; 3];
            let a = ((p - l.range - lo) / cell).floor();
            let b = ((p + l.range - lo) / cell).floor();
            for z in (a.z.max(0.0) as i32)..=(b.z as i32).min(dims[2] - 1) {
                for y in (a.y.max(0.0) as i32)..=(b.y as i32).min(dims[1] - 1) {
                    for x in (a.x.max(0.0) as i32)..=(b.x as i32).min(dims[0] - 1) {
                        let cmin = lo + Vec3::new(x as f32, y as f32, z as f32) * cell;
                        let near = p.clamp(cmin, cmin + cell);
                        if (near - p).length_squared() >= l.range * l.range {
                            continue;
                        }
                        let centre = cmin + cell * 0.5;
                        let dist = (centre - p).length();
                        // intensity at the cell centre (capped at 1: the framebuffer saturates) ranks the lights of a cell
                        let i = if d3d { (l.atten[0] + dist * (l.atten[1] + dist * l.atten[2])).recip().min(1.0) } else { (1.0 - dist / l.range).max(0.0) };
                        pairs.push((cell_of(x, y, z), lum * i / (1.0 + dist / cell), li as u32));
                    }
                }
            }
        }
        pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        let mut cells = vec![[0u32, 0u32]; (dims[0] * dims[1] * dims[2]) as usize];
        let mut idx = Vec::with_capacity(pairs.len());
        for (c, _, li) in pairs {
            let e = &mut cells[c as usize];
            if e[1] == 0 {
                e[0] = idx.len() as u32;
            }
            if (e[1] as usize) < CELL_LIGHTS {
                idx.push(li);
                e[1] += 1;
            }
        }
        if idx.is_empty() {
            idx.push(0);
        }
        let gl = lights
            .iter()
            .flat_map(|l| {
                // kind: 0 = linear ramp, 1 = D3D attenuation (atten x of the third vec4 is then the divisor sum)
                let (cos_phi, cos_theta, axis) = l.spot.map_or((2.0, 0.0, [0.0; 3]), |s| ((s.phi * 0.5).cos(), (s.theta * 0.5).cos(), s.dir));
                let a = if l.atten == [0.0; 3] { [-1.0, 0.0, 0.0] } else { l.atten }; // a0 < 0: linear
                [[l.pos[0], l.pos[1], l.pos[2], l.range], [l.color[0], l.color[1], l.color[2], cos_phi], [a[0], a[1], a[2], cos_theta], [axis[0], axis[1], axis[2], 0.0]]
            })
            .collect();
        Self { lights: gl, cells, idx, origin: lo, cell, dims }
    }
}

/// Static instance data: transform plus world-space bounding sphere.
struct Inst {
    m: [[f32; 4]; 4],
    center: Vec3,
    radius: f32,
}

struct Gpu {
    meshes: Vec<Option<(wgpu::Buffer, wgpu::Buffer)>>,
    inst_bufs: Vec<wgpu::Buffer>, // ring: a buffer is rewritten only after earlier frames using it were submitted
    sky_bufs: Vec<wgpu::Buffer>,  // ring of per-frame sky instance transforms
    mats: Vec<wgpu::BindGroup>, // [0] = untextured white
    opaque: Vec<Draw>,          // Opaque + AlphaTest, sorted by pipeline/mesh/material
    blended: Vec<Draw>,         // AlphaBlend + Additive, drawn per visible instance, far to near
    sky: Vec<(Draw, u32)>,      // (draw, index into `sky_xf`) in scene order
    sky_xf: Vec<Mat4>,
    insts: Vec<Inst>,           // grouped by mesh
    mesh_range: Vec<std::ops::Range<usize>>,
    radius: f32,
    grid: ([f32; 4], [i32; 4]), // light grid origin+cell, dims
}

/// Last-frame counters (after frustum culling).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub instances: usize,
    pub draw_calls: usize,
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
    pub stats: FrameStats,
    /// Seconds driving `Submesh::uv_scroll` (the viewer advances it; screenshots leave it at 0).
    pub time: f32,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    g_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipes: Vec<wgpu::RenderPipeline>, // indexed by Draw::pipe
    gpu: Gpu,
    env: Environment,
    /// Camera dependent fog (statel fog volumes), see `Scene::fog_model`.
    fog: Option<ao_scene::FogModel>,
    // per-frame scratch
    frame: usize,
    vis: Vec<[[f32; 4]; 4]>,
    vis_src: Vec<u32>,
    vis_range: Vec<std::ops::Range<u32>>,
    sorted: Vec<(f32, u32, u32)>, // (distance, blended draw, visible instance)
}

fn default_environment(radius: f32) -> Environment {
    let fog_end = (radius * 2.0).max(300.0);
    let sky = SKY_SRGB.map(srgb_to_linear);
    Environment {
        sky_color: sky,
        fog_color: sky,
        fog_start: fog_end * 0.5,
        fog_end,
        ambient: [0.35, 0.38, 0.45],
        sun_color: [0.75, 0.71, 0.64],
        sun_dir: Vec3::new(0.4, 0.8, 0.3).normalize().to_array(),
    }
}

/// Left, right, bottom, top, near, far planes (normalised, normals point inside).
fn frustum_planes(vp: &Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (vp.row(0), vp.row(1), vp.row(2), vp.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| p / p.truncate().length())
}

impl Renderer {
    /// Picks the Metal adapter (compatible with `surface` when windowed) and builds pipelines.
    pub fn new(instance: &wgpu::Instance, surface: Option<&wgpu::Surface>) -> Result<Self> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
        }))
        .map_err(|e| anyhow!("no suitable GPU adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            },
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
        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let storage_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let g_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT), storage_entry(1), storage_entry(2), storage_entry(3)],
        });
        let globals_bg = globals_bind(&device, &g_layout, &globals, &LightGrid::new(&[]));
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
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&g_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let f4 = |o| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: o, shader_location: 4 + (o / 16) as u32 };
        let inst_attrs = [f4(0), f4(16), f4(32), f4(48)];
        let vert_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let mk = |blend: Blend, two_sided: bool, sky: bool| {
            let (fs, state, depth_write) = match blend {
                Blend::Opaque => ("fs_opaque", None, true),
                Blend::AlphaTest => ("fs_test", None, true),
                Blend::AlphaBlend => ("fs_blend", Some(wgpu::BlendState::ALPHA_BLENDING), false),
                Blend::Additive => ("fs_add", Some(wgpu::BlendState { color: add, alpha: wgpu::BlendComponent::OVER }), false),
            };
            let fs = if sky { ["fs_sky_opaque", "fs_sky_test", "fs_sky_blend", "fs_sky_add"][blend as usize] } else { fs };
            let depth_write = depth_write && !sky;
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(if sky { "vs_sky" } else { "vs" }),
                    compilation_options: Default::default(),
                    buffers: &[
                        wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vert_attrs },
                        wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance, attributes: &inst_attrs },
                    ],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: state, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState {
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: (!two_sided && !sky).then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(if sky { wgpu::CompareFunction::Always } else { wgpu::CompareFunction::Less }),
                    stencil: Default::default(),
                    // Blended overlays are often coplanar with the opaque surface below them.
                    bias: if depth_write { Default::default() } else { wgpu::DepthBiasState { constant: -2, slope_scale: -2.0, clamp: 0.0 } },
                }),
                multisample: wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false },
                multiview_mask: None,
                cache: None,
            })
        };
        let pipes = [Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive]
            .into_iter()
            .flat_map(|b| [false, true].map(|two| (b, two, false)))
            .chain([Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive].map(|b| (b, true, true)))
            .map(|(b, two, sky)| mk(b, two, sky))
            .collect();
        let mut r = Self {
            device,
            queue,
            format,
            stats: FrameStats::default(),
            time: 0.0,
            globals,
            globals_bg,
            g_layout,
            tex_layout,
            sampler,
            pipes,
            gpu: Gpu { meshes: vec![], inst_bufs: vec![], sky_bufs: vec![], mats: vec![], opaque: vec![], blended: vec![], sky: vec![], sky_xf: vec![], insts: vec![], mesh_range: vec![], radius: 100.0, grid: ([0.0; 4], [0; 4]) },
            env: default_environment(100.0),
            fog: None,
            frame: 0,
            vis: vec![],
            vis_src: vec![],
            vis_range: vec![],
            sorted: vec![],
        };
        r.upload(&Scene::default());
        Ok(r)
    }

    fn texture_view(&self, rgba: &[u8], w: u32, h: u32) -> wgpu::TextureView {
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
        tex.create_view(&Default::default())
    }

    /// Replaces the GPU-side scene.
    pub fn upload(&mut self, scene: &Scene) {
        let mut views = vec![self.texture_view(&[255; 4], 1, 1)];
        let mut view_of: HashMap<TextureKey, usize> = HashMap::new();
        for (k, t) in &scene.textures {
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize {
                view_of.insert(*k, views.len());
                views.push(self.texture_view(&t.rgba, t.width, t.height));
            }
        }
        // One bind group per distinct (texture, base colour, emissive, glow mask).
        let mut mat_of: HashMap<(usize, [u32; 12]), usize> = HashMap::new();
        let mut mats: Vec<wgpu::BindGroup> = vec![];
        let mut material = |dev: &Renderer, view: usize, s: &ao_scene::Submesh| {
            let u: [f32; 12] = [s.base_color[0], s.base_color[1], s.base_color[2], s.base_color[3], s.emissive[0], s.emissive[1], s.emissive[2], if s.prelit { 2.0 } else { s.glow_mask as u32 as f32 }, s.uv_scroll[0], s.uv_scroll[1], 0.0, 0.0];
            *mat_of.entry((view, u.map(f32::to_bits))).or_insert_with(|| {
                let ub = dev.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::bytes_of(&u),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                mats.push(dev.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &dev.tex_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[view]) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&dev.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: ub.as_entire_binding() },
                    ],
                }));
                mats.len() - 1
            })
        };

        let mut by_mesh: Vec<Vec<Inst>> = scene.meshes.iter().map(|_| vec![]).collect();
        let sphere: Vec<(Vec3, f32)> = scene
            .meshes
            .iter()
            .map(|m| {
                let (lo, hi) = m.vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), v| {
                    (l.min(Vec3::from(v.pos)), h.max(Vec3::from(v.pos)))
                });
                let c = (lo + hi) * 0.5;
                let r = m.vertices.iter().map(|v| (Vec3::from(v.pos) - c).length()).fold(0.0, f32::max);
                (c, r)
            })
            .collect();
        for i in &scene.instances {
            if let Some(v) = by_mesh.get_mut(i.mesh) {
                let m = Mat4::from_cols_array_2d(&i.transform);
                let scale = m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length());
                v.push(Inst { m: i.transform, center: m.transform_point3(sphere[i.mesh].0), radius: sphere[i.mesh].1 * scale });
            }
        }

        let (mut insts, mut mesh_range, mut meshes) = (vec![], vec![], vec![]);
        let (mut opaque, mut blended) = (vec![], vec![]);
        let mut sky_draws: HashMap<usize, Vec<Draw>> = HashMap::new();
        let sky_used = |mi: usize| scene.sky.iter().any(|s| s.mesh == mi);
        for (mi, mesh) in scene.meshes.iter().enumerate() {
            let list = std::mem::take(&mut by_mesh[mi]);
            let list_len = list.len();
            let range = insts.len()..insts.len() + list.len();
            let idx_total: usize = mesh.submeshes.iter().map(|s| s.indices.len()).sum();
            if mesh.vertices.is_empty() || idx_total == 0 || (list.is_empty() && !sky_used(mi)) {
                mesh_range.push(insts.len()..insts.len());
                meshes.push(None);
                continue;
            }
            mesh_range.push(range);
            insts.extend(list);
            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&vertex_bytes(&mesh.vertices)),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let mut indices = Vec::with_capacity(idx_total);
            for s in &mesh.submeshes {
                let first = indices.len() as u32;
                let nv = mesh.vertices.len() as u32;
                // Drop whole triangles that index out of range rather than crash the GPU.
                for t in s.indices.as_chunks::<3>().0 {
                    if t.iter().all(|&i| i < nv) {
                        indices.extend_from_slice(t.as_slice());
                    }
                }
                let count = indices.len() as u32 - first;
                if count > 0 {
                    let view = s.texture.and_then(|k| view_of.get(&k).copied()).unwrap_or(0);
                    let mat = material(self, view, s);
                    let d = Draw { mesh: mi, first_index: first, count, mat, pipe: s.blend as usize * 2 + s.two_sided as usize };
                    if sky_used(mi) {
                        sky_draws.entry(mi).or_default().push(Draw { pipe: SKY_PIPE + s.blend as usize, ..d });
                    }
                    if list_len > 0 {
                        if matches!(s.blend, Blend::AlphaBlend | Blend::Additive) { blended.push(d) } else { opaque.push(d) }
                    }
                }
            }
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            meshes.push(Some((vb, ib)));
        }
        opaque.sort_by_key(|d| (d.pipe, d.mesh, d.mat));
        let inst_bufs = if insts.is_empty() {
            vec![]
        } else {
            (0..INST_RING)
                .map(|_| {
                    self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("instances"),
                        size: (insts.len() * 64) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                })
                .collect()
        };
        let radius = scene_bounds(scene).map_or(100.0, |(l, h)| ((h - l).length() * 0.5).max(1.0));
        self.env = scene.environment.unwrap_or_else(|| default_environment(radius));
        self.fog = scene.fog_model.clone();
        let mut sky = vec![];
        let mut sky_xf = vec![];
        for s in &scene.sky {
            for d in sky_draws.get(&s.mesh).into_iter().flatten() {
                sky.push((*d, sky_xf.len() as u32));
            }
            sky_xf.push(Mat4::from_cols_array_2d(&s.transform));
        }
        let sky_bufs = if sky_xf.is_empty() {
            vec![]
        } else {
            (0..INST_RING)
                .map(|_| {
                    self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("sky"),
                        size: (sky_xf.len() * 64) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                })
                .collect()
        };
        let grid = LightGrid::new(&scene.lights);
        self.globals_bg = globals_bind(&self.device, &self.g_layout, &self.globals, &grid);
        let grid = ([grid.origin.x, grid.origin.y, grid.origin.z, grid.cell], [grid.dims[0], grid.dims[1], grid.dims[2], 0]);
        self.gpu = Gpu { meshes, inst_bufs, sky_bufs, mats, opaque, blended, sky, sky_xf, insts, mesh_range, radius, grid };
    }

    /// Re-poses the uploaded scene in place: `scene` must be the same scene with only vertex positions/normals and
    /// instance transforms changed (an animated character); textures, materials and indices are kept.
    pub fn repose(&mut self, scene: &Scene) {
        let mut seen = vec![0; self.gpu.mesh_range.len()];
        for i in &scene.instances {
            let Some(r) = self.gpu.mesh_range.get(i.mesh) else { continue };
            if let Some(inst) = self.gpu.insts.get_mut(r.start + seen[i.mesh]).filter(|_| seen[i.mesh] < r.len()) {
                inst.m = i.transform;
            }
            seen[i.mesh] += 1;
        }
        for (m, gpu) in scene.meshes.iter().zip(&self.gpu.meshes) {
            if let Some((vb, _)) = gpu {
                self.queue.write_buffer(vb, 0, bytemuck::cast_slice(&vertex_bytes(&m.vertices)));
            }
        }
    }

    /// Scene radius; viewer uses it for speed.
    pub fn radius(&self) -> f32 {
        self.gpu.radius
    }

    /// Draws one frame into `resolve` (a view of the output texture).
    pub fn render(&mut self, resolve: &wgpu::TextureView, t: &Targets, cam: &Camera) {
        let mut env = self.env;
        if let Some((color, end)) = self.fog.as_ref().map(|m| m.at(cam.pos.to_array())) {
            if env.sky_color == env.fog_color {
                env.sky_color = color;
            }
            (env.fog_color, env.fog_end) = (color, end);
        }
        let far = (env.fog_end * 1.1).max(50.0);
        let aspect = t.size.0 as f32 / t.size.1.max(1) as f32;
        let vp = Mat4::perspective_rh(60f32.to_radians(), aspect, 0.2, far) * Mat4::look_to_rh(cam.pos, cam.forward(), Vec3::Y);
        let v4 = |c: [f32; 3], w| Vec4::new(c[0], c[1], c[2], w).to_array();
        let g = Globals {
            view_proj: vp.to_cols_array_2d(),
            eye: cam.pos.extend(1.0).to_array(),
            sun_dir: v4(Vec3::from(env.sun_dir).normalize_or_zero().to_array(), 0.0),
            sun_color: v4(env.sun_color, 0.0),
            ambient: v4(env.ambient, 0.0),
            fog_color: v4(env.fog_color, 1.0),
            fog: [env.fog_start, env.fog_end, self.time, 0.0],
            grid: self.gpu.grid.0,
            dims: self.gpu.grid.1,
        };
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));

        // Frustum cull per instance (bounding spheres), compacting survivors per mesh.
        let planes = frustum_planes(&vp);
        self.vis.clear();
        self.vis_src.clear();
        self.vis_range.clear();
        for r in &self.gpu.mesh_range {
            let start = self.vis.len() as u32;
            for (i, inst) in self.gpu.insts[r.clone()].iter().enumerate() {
                let c = inst.center.extend(1.0);
                if planes.iter().all(|p| p.dot(c) >= -inst.radius) {
                    self.vis.push(inst.m);
                    self.vis_src.push((r.start + i) as u32);
                }
            }
            self.vis_range.push(start..self.vis.len() as u32);
        }
        self.frame = (self.frame + 1) % INST_RING;
        let inst_buf = self.gpu.inst_bufs.get(self.frame);
        if let Some(buf) = inst_buf {
            if !self.vis.is_empty() {
                self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.vis));
            }
        }
        let sky_buf = self.gpu.sky_bufs.get(self.frame);
        if let Some(buf) = sky_buf {
            let xf: Vec<[[f32; 4]; 4]> = self.gpu.sky_xf.iter().map(|m| Mat4::from_cols(m.x_axis, m.y_axis, m.z_axis, cam.pos.extend(1.0))).map(|m| m.to_cols_array_2d()).collect();
            self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&xf));
        }
        // Blended: one draw per (visible instance, submesh), far to near by instance centre.
        self.sorted.clear();
        for (di, d) in self.gpu.blended.iter().enumerate() {
            for vi in self.vis_range[d.mesh].clone() {
                let dist = (self.gpu.insts[self.vis_src[vi as usize] as usize].center - cam.pos).length();
                self.sorted.push((dist, di as u32, vi));
            }
        }
        self.sorted.sort_by(|a, b| b.0.total_cmp(&a.0)); // stable: ties keep submesh order

        let mut calls = 0;
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    resolve_target: Some(resolve),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: env.sky_color[0] as f64, g: env.sky_color[1] as f64, b: env.sky_color[2] as f64, a: 1.0 }),
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
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            let (mut pipe, mut mesh, mut mat) = (usize::MAX, usize::MAX, usize::MAX);
            let mut draw = |pass: &mut wgpu::RenderPass, d: &Draw, insts: std::ops::Range<u32>| {
                if pipe != d.pipe {
                    pass.set_pipeline(&self.pipes[d.pipe]);
                    pipe = d.pipe;
                }
                if mesh != d.mesh {
                    let (vb, ib) = self.gpu.meshes[d.mesh].as_ref().unwrap();
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    mesh = d.mesh;
                }
                if mat != d.mat {
                    pass.set_bind_group(1, &self.gpu.mats[d.mat], &[]);
                    mat = d.mat;
                }
                pass.draw_indexed(d.first_index..d.first_index + d.count, 0, insts);
                calls += 1;
            };
            if let Some(sky) = sky_buf {
                pass.set_vertex_buffer(1, sky.slice(..));
                for (d, k) in &self.gpu.sky {
                    draw(&mut pass, d, *k..*k + 1);
                }
            }
            if let Some(inst) = inst_buf {
                pass.set_vertex_buffer(1, inst.slice(..));
                for d in &self.gpu.opaque {
                    let r = self.vis_range[d.mesh].clone();
                    if !r.is_empty() {
                        draw(&mut pass, d, r);
                    }
                }
                for &(_, di, vi) in &self.sorted {
                    draw(&mut pass, &self.gpu.blended[di as usize], vi..vi + 1);
                }
            }
        }
        self.stats = FrameStats { instances: self.vis.len(), draw_calls: calls };
        self.queue.submit([enc.finish()]);
    }
}

fn vertex_bytes(v: &[ao_scene::Vertex]) -> Vec<f32> {
    v.iter()
        .flat_map(|v| {
            let [x, y, z] = v.pos;
            let [nx, ny, nz] = v.normal;
            let [r, g, b, a] = v.color;
            [x, y, z, nx, ny, nz, v.uv[0], v.uv[1], r, g, b, a]
        })
        .collect()
}

/// Renders `scene` offscreen and writes an sRGB PNG.
pub fn render_to_png(scene: &Scene, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path) -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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
    r.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("poll: {e}"))?;
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

fn storage<T: bytemuck::Pod>(device: &wgpu::Device, data: &[T]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(data), usage: wgpu::BufferUsages::STORAGE })
}

/// Group 0: globals uniform + the static light grid (lights, cells, per-cell index lists).
fn globals_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, globals: &wgpu::Buffer, g: &LightGrid) -> wgpu::BindGroup {
    let (l, c, i) = (storage(device, &g.lights), storage(device, &g.cells), storage(device, &g.idx));
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: l.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: c.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: i.as_entire_binding() },
        ],
    })
}
