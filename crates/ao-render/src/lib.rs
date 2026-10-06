//! wgpu renderer for `ao_scene::Scene`: windowed free-fly viewer (`viewer`) and offscreen PNG path.

mod actors;
mod gui;
mod viewer;

use anyhow::{anyhow, Context, Result};
use ao_scene::{Blend, Environment, Scene, TextureKey};
pub use glam::Vec3;
pub use winit::keyboard::KeyCode;
use glam::{Mat4, Vec4};
use std::collections::HashMap;
use std::path::Path;
use wgpu::util::DeviceExt;

pub use gui::GuiRenderer;
pub use viewer::{run_frontend, GameInput, Offscreen, run_viewer, run_viewer_hooked, run_viewer_live, FrameHook, Frontend, Host, LiveSky};

const MSAA: u32 = 4;
const INST_RING: usize = 3;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SKY_SRGB: [f32; 3] = [0.53, 0.72, 0.92];

fn srgb_to_linear(c: f32) -> f32 {
    c.powf(2.2)
}

/// Inverse of [`srgb_to_linear`]: the scene contract's `c^2.2` colours back to the client's framebuffer-space values.
fn gamma(c: f32) -> f32 {
    c.max(0.0).powf(1.0 / 2.2)
}

/// Free-fly camera. Forward = (sin yaw·cos pitch, sin pitch, -cos yaw·cos pitch).
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Rotation about the view axis, radians; positive turns the up vector towards [`Camera::right`] (0 = horizon level).
    pub roll: f32,
}

impl Camera {
    pub fn look_at(eye: Vec3, at: Vec3) -> Self {
        let d = (at - eye).normalize_or_zero();
        let d = if d == Vec3::ZERO { -Vec3::Z } else { d };
        Self { pos: eye, yaw: d.x.atan2(-d.z), pitch: d.y.asin(), roll: 0.0 }
    }
    /// Camera at `eye` looking along `forward` with the given `up` (full orientation; `up` need not be orthogonal).
    pub fn look_to_up(eye: Vec3, forward: Vec3, up: Vec3) -> Self {
        let mut c = Self::look_at(eye, eye + forward);
        let level = c.right().cross(c.forward());
        c.roll = up.dot(c.right()).atan2(up.dot(level));
        c
    }
    /// View-space up: the horizon-level up turned by `roll` about the view axis.
    pub fn up(&self) -> Vec3 {
        let (r, f) = (self.right(), self.forward());
        r.cross(f) * self.roll.cos() + r * self.roll.sin()
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
    wave: [[f32; 4]; 4],
    sun_g: [f32; 4],
    ambient_g: [f32; 4],
    fog_g: [f32; 4],
    view: [[f32; 4]; 4],
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
/// Actor environment-map pipelines (`ao_scene::Submesh::env_texture`), one/two-sided each: 12..14 draw in the opaque actor phase,
/// 14..16 (the same pipelines) in the blended actor phase, so an env layer follows the phase of its submesh.
const ENV_PIPE: usize = 12;
const ENV_BLEND_PIPE: usize = 14;
/// Actors drawn with `ActorFrame::alpha < 1`: the opaque / alpha-tested submeshes (blend x cull = 16..20) blend with the frame's alpha but keep
/// the depth write (`RVisual_t::RenderWithTransparency` randy31 0x1004d2d8 only switches ALPHABLENDENABLE on); they stay in the opaque phase.
const FADE_PIPE: usize = 16;
/// Lights kept per grid cell (strongest first) and the minimum cell edge in metres.
const CELL_LIGHTS: usize = 16;
const MIN_CELL: f32 = 8.0;
const MAX_CELLS: usize = 1 << 20;

/// Static light grid: dense 3D cells, each listing up to `CELL_LIGHTS` lights whose sphere touches it.
struct LightGrid {
    zones: Vec<Option<u32>>, // statel zone per light (`Light::zone`), same order as `lights`
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
            return Self { zones: vec![], lights: vec![[0.0; 4]; 4], cells: vec![[0, 0]], idx: vec![0], origin: Vec3::ZERO, cell: 1.0, dims: [0; 3] };
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
                // colours go to the shader in D3D (gamma) space, where the fixed function lighting sums them
                [[l.pos[0], l.pos[1], l.pos[2], l.range], [gamma(l.color[0]), gamma(l.color[1]), gamma(l.color[2]), cos_phi], [a[0], a[1], a[2], cos_theta], [axis[0], axis[1], axis[2], 0.0]]
            })
            .collect();
        Self { zones: lights.iter().map(|l| l.zone).collect(), lights: gl, cells, idx, origin: lo, cell, dims }
    }
}

/// Static instance data: transform plus world-space bounding sphere.
struct Inst {
    /// Hidden by the statel distance LOD (`Scene::statel_lod`).
    hidden: bool,
    m: [[f32; 4]; 4],
    center: Vec3,
    radius: f32,
}

/// A traffic ship with its GPU instance slot and accumulators.
struct MoverRun {
    mover: ao_scene::Mover,
    state: ao_scene::MoverState,
    slot: usize,
    /// Centre of the mesh's bounding sphere in object space.
    local_center: Vec3,
}

/// An instance the client re-poses every frame (`Scene::far_away`, [`ao_scene::far_away_pose`]).
struct FarAwayRun {
    slot: usize,
    /// Pose of a static instance; ships take the pose their mover just computed.
    base: Option<Mat4>,
    local_center: Vec3,
    local_radius: f32,
}

/// `Scene::statel_lod` plus the GPU instance slot of every scene instance and the last zone levels.
struct LodState {
    lod: ao_scene::StatelLod,
    /// Scene instance index -> index into `Gpu::insts` (`usize::MAX` = not uploaded).
    slot: Vec<usize>,
    levels: Vec<u8>,
    /// Per item: the controller's current identity is the reduced mesh (`FUN_100241ab` keeps it between modes).
    reduced: Vec<bool>,
}

struct Gpu {
    meshes: Vec<Option<(wgpu::Buffer, wgpu::Buffer)>>,
    inst_bufs: Vec<wgpu::Buffer>, // ring: a buffer is rewritten only after earlier frames using it were submitted
    mats: Vec<wgpu::BindGroup>, // [0] = untextured white
    opaque: Vec<Draw>,          // Opaque + AlphaTest, sorted by pipeline/mesh/material
    blended: Vec<Draw>,         // AlphaBlend + Additive, drawn per visible instance, far to near
    liquid: Vec<Draw>,          // `Submesh::liquid` (render list 4): opaque kinds first, then blended; after the opaque phase + actors
    insts: Vec<Inst>,           // grouped by mesh
    mesh_range: Vec<std::ops::Range<usize>>,
    radius: f32,
    grid: ([f32; 4], [i32; 4]), // light grid origin+cell, dims
}

/// Sky backdrop with its own mesh/material tables, so [`Renderer::set_sky`] can swap it while the world stays.
#[derive(Default)]
struct SkyGpu {
    meshes: Vec<Option<(wgpu::Buffer, wgpu::Buffer)>>, // indexed like the uploaded scene's meshes
    mats: Vec<wgpu::BindGroup>,
    draws: Vec<(Draw, u32)>, // (draw, index into `xf`) in scene order
    xf: Vec<Mat4>,
    spin: Vec<Option<ao_scene::SkySpin>>, // per `xf` entry
    wave: Vec<ao_scene::SkyWaveSpin>, // rotations driven by `GameWaveCurve*`
    bufs: Vec<wgpu::Buffer>, // ring of per-frame instance transforms (translation = camera)
    colors: Vec<ColorAnim>,
}

/// A sky mesh whose vertex colours are re-simulated every frame ([`ao_scene::SkyColors`]).
struct ColorAnim {
    mesh: usize,
    sim: ao_scene::aurora::GloomySky,
    gain: f32,
    vertices: Vec<ao_scene::Vertex>,
}

/// Material uniform of a submesh (see `shader.wgsl` `Mat`).
fn mat_uniform(s: &ao_scene::Submesh) -> [f32; 20] {
    let [wu, au, wv, av] = s.uv_wave;
    [s.base_color[0], s.base_color[1], s.base_color[2], s.base_color[3], s.emissive[0], s.emissive[1], s.emissive[2], if s.prelit { 2.0 } else { s.glow_mask as u32 as f32 }, s.uv_scroll[0], s.uv_scroll[1], s.sky_fog as u32 as f32, s.sun_flicker as u32 as f32, wu, au, wv, av, s.specular[0], s.specular[1], s.specular[2], s.shininess]
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
    sky: SkyGpu,
    /// Sky textures by key; kept across [`Renderer::set_sky`] so a live sky only ships new textures.
    sky_views: HashMap<TextureKey, wgpu::TextureView>,
    env: Environment,
    /// Lens of the loaded scene (`Scene::lens`).
    lens: ao_scene::Lens,
    /// Camera dependent fog (statel fog volumes), see `Scene::fog_model`.
    fog: Option<ao_scene::FogModel>,
    /// Multiplier of the accumulated fog density, see [`ao_scene::fog_mode_density_scale`] (`FogMode` pref).
    fog_density_scale: f32,
    /// Statel distance LOD state, see `Scene::statel_lod`.
    lod: Option<LodState>,
    /// Light storage buffer plus its pristine contents and the statel zone of each light (distance gating).
    light_buf: wgpu::Buffer,
    light_base: Vec<[f32; 4]>,
    light_zones: Vec<Option<u32>>,
    /// Traffic ships of the uploaded scene, see `Scene::movers`.
    movers: Vec<MoverRun>,
    far_away: Vec<FarAwayRun>,
    /// `Scene::day_time` of the uploaded scene.
    day_time: f32,
    /// Game seconds per [`Renderer::time`] second for the traffic ships (1 = the game clock; the viewer sets the live
    /// sky's scale).
    pub day_time_rate: f32,
    // per-frame scratch
    frame: usize,
    vis: Vec<[[f32; 4]; 4]>,
    vis_src: Vec<u32>,
    vis_range: Vec<std::ops::Range<u32>>,
    sorted: Vec<(f32, u32, u32)>, // (distance, blended draw, visible instance)
    /// Dynamic actors drawn after the world, see `actors.rs`.
    act: actors::ActorLayer,
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
        sun_specular: 1.0,
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
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let g_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT), storage_entry(1), storage_entry(2), storage_entry(3)],
        });
        let (globals_bg, light_buf) = globals_bind(&device, &g_layout, &globals, &LightGrid::new(&[]));
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
                uniform_entry(2, wgpu::ShaderStages::VERTEX_FRAGMENT),
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
        let one_one = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let mk = |blend: Blend, two_sided: bool, sky: bool, env: bool, fade: bool| {
            let (fs, state, depth_write) = match blend {
                _ if env => ("fs_env", Some(wgpu::BlendState { color: one_one, alpha: one_one }), false),
                Blend::Opaque if fade => ("fs_fade_opaque", Some(wgpu::BlendState::ALPHA_BLENDING), true),
                Blend::AlphaTest if fade => ("fs_fade_test", Some(wgpu::BlendState::ALPHA_BLENDING), true),
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
                    entry_point: Some(if sky { "vs_sky" } else if env { "vs_env" } else { "vs" }),
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
                    // the env pass redraws the very triangles of the base pass: D3D's default ZFUNC LESSEQUAL
                    depth_compare: Some(if sky { wgpu::CompareFunction::Always } else if env { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Less }),
                    stencil: Default::default(),
                    // Blended overlays are often coplanar with the opaque surface below them.
                    bias: if depth_write || env { Default::default() } else { wgpu::DepthBiasState { constant: -2, slope_scale: -2.0, clamp: 0.0 } },
                }),
                multisample: wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false },
                multiview_mask: None,
                cache: None,
            })
        };
        let pipes = [Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive]
            .into_iter()
            .flat_map(|b| [false, true].map(|two| (b, two, false, false)))
            .chain([Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive].map(|b| (b, true, true, false)))
            .chain([false, true, false, true].map(|two| (Blend::Opaque, two, false, true)))
            .map(|(b, two, sky, env)| (b, two, sky, env, false))
            .chain([Blend::Opaque, Blend::AlphaTest].into_iter().flat_map(|b| [false, true].map(move |two| (b, two, false, false, true))))
            .map(|(b, two, sky, env, fade)| mk(b, two, sky, env, fade))
            .collect();
        let mut r = Self {
            device,
            queue,
            format,
            stats: FrameStats::default(),
            time: 0.0,
            movers: vec![],
            far_away: vec![],
            day_time: 0.0,
            day_time_rate: 1.0,
            globals,
            globals_bg,
            g_layout,
            tex_layout,
            sampler,
            pipes,
            sky: SkyGpu::default(),
            sky_views: HashMap::new(),
            gpu: Gpu { meshes: vec![], inst_bufs: vec![], mats: vec![], opaque: vec![], blended: vec![], liquid: vec![], insts: vec![], mesh_range: vec![], radius: 100.0, grid: ([0.0; 4], [0; 4]) },
            env: default_environment(100.0),
            lens: ao_scene::Lens::default(),
            fog: None,
            fog_density_scale: 1.0,
            lod: None,
            light_buf,
            light_base: vec![],
            light_zones: vec![],
            frame: 0,
            vis: vec![],
            vis_src: vec![],
            vis_range: vec![],
            sorted: vec![],
            act: Default::default(),
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
        let mut mat_of: HashMap<(usize, [u32; 20]), usize> = HashMap::new();
        let mut mats: Vec<wgpu::BindGroup> = vec![];
        let mut material = |dev: &Renderer, view: usize, s: &ao_scene::Submesh| {
            let u = mat_uniform(s);
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
                v.push(Inst { hidden: false, m: i.transform, center: m.transform_point3(sphere[i.mesh].0), radius: sphere[i.mesh].1 * scale });
            }
        }

        let (mut insts, mut mesh_range, mut meshes) = (vec![], vec![], vec![]);
        let (mut opaque, mut blended, mut liquid) = (vec![], vec![], vec![]);
        for (mi, mesh) in scene.meshes.iter().enumerate() {
            let list = std::mem::take(&mut by_mesh[mi]);
            let range = insts.len()..insts.len() + list.len();
            let idx_total: usize = mesh.submeshes.iter().map(|s| s.indices.len()).sum();
            if mesh.vertices.is_empty() || idx_total == 0 || list.is_empty() {
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
                    if s.liquid {
                        liquid.push(d)
                    } else if matches!(s.blend, Blend::AlphaBlend | Blend::Additive) {
                        blended.push(d)
                    } else {
                        opaque.push(d)
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
        self.lens = scene.lens.unwrap_or_default();
        self.fog = scene.fog_model.clone();
        let slots = instance_slots(scene, &mesh_range);
        self.lod = scene.statel_lod.clone().map(|lod| LodState { levels: vec![u8::MAX; lod.zones.len()], reduced: vec![false; lod.items.len()], lod, slot: slots.clone() });
        let rate = self.day_time_rate;
        self.day_time = scene.day_time;
        self.movers = scene
            .movers
            .iter()
            .filter_map(|m| {
                let slot = *slots.get(m.instance).filter(|&&s| s != usize::MAX)?;
                let local_center = sphere[scene.instances[m.instance].mesh].0;
                Some(MoverRun { mover: m.clone(), state: ao_scene::MoverState::settled(m, scene.day_time, rate), slot, local_center })
            })
            .collect();
        self.far_away = scene
            .far_away
            .iter()
            .filter_map(|&i| {
                let slot = *slots.get(i).filter(|&&s| s != usize::MAX)?;
                let (local_center, local_radius) = sphere[scene.instances[i].mesh];
                let is_mover = scene.movers.iter().any(|m| m.instance == i);
                Some(FarAwayRun { slot, base: (!is_mover).then(|| Mat4::from_cols_array_2d(&scene.instances[i].transform)), local_center, local_radius })
            })
            .collect();
        self.set_sky(scene);
        let grid = LightGrid::new(&scene.lights);
        (self.globals_bg, self.light_buf) = globals_bind(&self.device, &self.g_layout, &self.globals, &grid);
        self.light_zones = grid.zones.clone();
        self.light_base = grid.lights.clone();
        let grid = ([grid.origin.x, grid.origin.y, grid.origin.z, grid.cell], [grid.dims[0], grid.dims[1], grid.dims[2], 0]);
        liquid.sort_by_key(|d| d.pipe / 2 >= 2); // stable: opaque kinds (lava) before the blended ones
        self.gpu = Gpu { meshes, inst_bufs, mats, opaque, blended, liquid, insts, mesh_range, radius, grid };
    }

    /// Replaces only the sky backdrop (`scene.sky` instances of `scene.meshes`) and, when given, the environment (sun, ambient,
    /// fog base, volumes kept); the world stays. Textures are cached by key, so `scene.textures` need only hold keys not sent before.
    pub fn set_sky(&mut self, scene: &Scene) {
        for (k, t) in &scene.textures {
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize && !self.sky_views.contains_key(k) {
                let v = self.texture_view(&t.rgba, t.width, t.height);
                self.sky_views.insert(*k, v);
            }
        }
        let white = self.texture_view(&[255; 4], 1, 1);
        let mut sky = SkyGpu::default();
        let mut mat_of: HashMap<(Option<TextureKey>, [u32; 20]), usize> = HashMap::new();
        for (mi, mesh) in scene.meshes.iter().enumerate() {
            let idx_total: usize = mesh.submeshes.iter().map(|s| s.indices.len()).sum();
            if mesh.vertices.is_empty() || idx_total == 0 || !scene.sky.iter().any(|s| s.mesh == mi) {
                sky.meshes.push(None);
                continue;
            }
            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&vertex_bytes(&mesh.vertices)),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let nv = mesh.vertices.len() as u32;
            let mut indices = Vec::with_capacity(idx_total);
            let mut ranges = vec![];
            for s in &mesh.submeshes {
                let first = indices.len() as u32;
                for t in s.indices.as_chunks::<3>().0 {
                    if t.iter().all(|&i| i < nv) {
                        indices.extend_from_slice(t.as_slice());
                    }
                }
                let count = indices.len() as u32 - first;
                if count == 0 {
                    continue;
                }
                let key = s.texture.filter(|k| self.sky_views.contains_key(k));
                let u = mat_uniform(s);
                let mat = *mat_of.entry((key, u.map(f32::to_bits))).or_insert_with(|| {
                    let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&u), usage: wgpu::BufferUsages::UNIFORM });
                    sky.mats.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &self.tex_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(key.map_or(&white, |k| &self.sky_views[&k])) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            wgpu::BindGroupEntry { binding: 2, resource: ub.as_entire_binding() },
                        ],
                    }));
                    sky.mats.len() - 1
                });
                ranges.push(Draw { mesh: mi, first_index: first, count, mat, pipe: SKY_PIPE + s.blend as usize });
            }
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&indices), usage: wgpu::BufferUsages::INDEX });
            sky.meshes.push(Some((vb, ib)));
            // draws are attached per instance below
            for d in ranges {
                sky.draws.push((d, u32::MAX));
            }
        }
        // one draw per (instance, submesh), in instance order
        let per_mesh = std::mem::take(&mut sky.draws);
        for inst in &scene.sky {
            for (d, _) in per_mesh.iter().filter(|(d, _)| d.mesh == inst.mesh) {
                sky.draws.push((*d, sky.xf.len() as u32));
            }
            sky.xf.push(Mat4::from_cols_array_2d(&inst.transform));
        }
        sky.spin = (0..sky.xf.len()).map(|i| scene.sky_spin.iter().find(|s| s.instance == i).copied()).collect();
        sky.wave = scene.sky_wave_spin.clone();
        sky.bufs = if sky.xf.is_empty() {
            vec![]
        } else {
            (0..INST_RING)
                .map(|_| self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("sky"), size: (sky.xf.len() * 64) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }))
                .collect()
        };
        // a live update keeps the running simulation of an identical aurora
        sky.colors = scene
            .sky_colors
            .iter()
            .filter(|c| matches!(sky.meshes.get(c.mesh), Some(Some(_))))
            .map(|c| ColorAnim {
                mesh: c.mesh,
                sim: self.sky.colors.iter().find(|o| o.sim.params == c.sim.params).map_or_else(|| c.sim.clone(), |o| o.sim.clone()),
                gain: c.gain,
                vertices: scene.meshes[c.mesh].vertices.clone(),
            })
            .collect();
        self.sky = sky;
        if let Some(env) = scene.environment {
            self.env = env;
        }
        // a live update brings the base fog of the new time; the playfield's fog volumes stay
        if let Some(mut model) = scene.fog_model.clone() {
            if let Some(old) = self.fog.take().filter(|_| model.volumes.is_empty()) {
                model.volumes = old.volumes;
            }
            self.fog = Some(model);
        }
    }

    /// Applies the client's statel zone LOD for a camera at scene `(x, z)`: re-evaluates the zone levels and, when one
    /// changed, which statels (and which of their meshes) are shown.
    fn update_lod(&mut self, x: f32, z: f32) {
        let Some(st) = self.lod.as_mut() else { return };
        let levels: Vec<u8> = (0..st.lod.zones.len()).map(|i| st.lod.level(i, [x, z])).collect();
        if levels == st.levels {
            return;
        }
        if st.levels.iter().all(|&l| l == u8::MAX) {
            // statels no zone shows (unreferenced global statels) are never created by the client
            for item in st.lod.items.iter().filter(|i| i.zones.is_empty()) {
                for i in [Some(item.full), item.reduced].into_iter().flatten() {
                    if let Some(g) = st.slot.get(i).and_then(|&g| self.gpu.insts.get_mut(g)) {
                        g.hidden = true;
                    }
                }
            }
        }
        for (zone, (&new, old)) in levels.iter().zip(&st.levels).enumerate() {
            if new == *old {
                continue;
            }
            for &it in &st.lod.zone_items[zone] {
                let item = &st.lod.items[it as usize];
                let (pick, ident) = st.lod.pick(item, &levels, st.reduced[it as usize]);
                st.reduced[it as usize] = ident;
                let mut set = |inst: Option<usize>, show: bool| {
                    if let Some(g) = inst.and_then(|i| st.slot.get(i)).and_then(|&g| self.gpu.insts.get_mut(g)) {
                        g.hidden = !show;
                    }
                };
                set(Some(item.full), pick == ao_scene::LodPick::Full);
                set(item.reduced, pick == ao_scene::LodPick::Reduced);
            }
        }
        // lights follow the zone's list 5 state: `range = 0` switches a light off (the shader's hard cut at the range)
        if self.light_zones.iter().any(Option::is_some) {
            let data = gate_lights(&self.light_base, &self.light_zones, &levels);
            self.queue.write_buffer(&self.light_buf, 0, bytemuck::cast_slice(&data));
        }
        st.levels = levels;
    }

    /// The `FogMode` pref's density multiplier (`VisualFog_t::SetFogMode`, [`ao_scene::fog_mode_density_scale`]).
    pub fn set_fog_density_scale(&mut self, scale: f32) {
        self.fog_density_scale = scale;
    }

    /// Replaces the lens of the loaded scene (a camera path that changes the field of view every frame, the player camera's `ViewDistance`).
    /// A far plane also moves the fog end and the statel LOD's view length: the client's `VisualFog_t::AddClipPlanes(near, far)` and
    /// `VisualCamera_t::GetLengthOfViewcone` (far - near) both come from the camera's planes (docs/chat/dvalue.md).
    pub fn set_lens(&mut self, lens: ao_scene::Lens) {
        if let Some(far) = lens.far {
            let near = self.fog.as_ref().map_or(lens.near, |f| f.near);
            if let Some(f) = &mut self.fog {
                f.far = far;
            }
            if let Some(l) = &mut self.lod {
                l.lod.view_length = far - near;
            }
        }
        self.lens = lens;
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

    /// One FXS frame for every traffic ship ([`ao_scene::Mover`]): the waypoint target of the game clock
    /// (`Scene::day_time + time * day_time_rate`) feeds the per-frame accumulators, the instance follows.
    fn step_movers(&mut self) {
        let day = self.day_time + self.time * self.day_time_rate;
        for r in &mut self.movers {
            r.state.step(&r.mover, r.mover.target(day));
            if let Some(g) = self.gpu.insts.get_mut(r.slot) {
                g.m = r.mover.transform(&r.state);
                g.center = Mat4::from_cols_array_2d(&g.m).transform_point3(r.local_center);
            }
        }
    }

    /// `e_ScaleVisibleFarAway` objects: pulled in towards the camera like the client's `GenericMeshObject::Process`.
    fn pose_far_away(&mut self, cam: Vec3) {
        for r in &self.far_away {
            let Some(g) = self.gpu.insts.get_mut(r.slot) else { continue };
            let base = r.base.unwrap_or_else(|| Mat4::from_cols_array_2d(&g.m));
            let (pos, s) = ao_scene::far_away_pose(base.w_axis.truncate().to_array(), cam.to_array());
            let m = Mat4::from_cols(base.x_axis * s, base.y_axis * s, base.z_axis * s, Vec3::from(pos).extend(1.0));
            let scale = m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length());
            g.m = m.to_cols_array_2d();
            g.center = m.transform_point3(r.local_center);
            g.radius = r.local_radius * scale;
        }
    }

    /// Sets [`Renderer::time`] and lets the traffic ships settle at that game time (the accumulators are per frame, a
    /// jump needs the frames in between): screenshots at `--anim-time`.
    pub fn seek(&mut self, time: f32) {
        self.time = time;
        let day = self.day_time + time * self.day_time_rate;
        for r in &mut self.movers {
            r.state = ao_scene::MoverState::settled(&r.mover, day, self.day_time_rate);
        }
    }

    /// Scene radius; viewer uses it for speed.
    pub fn radius(&self) -> f32 {
        self.gpu.radius
    }

    /// Draws one frame into `resolve` (a view of the output texture).
    pub fn render(&mut self, resolve: &wgpu::TextureView, t: &Targets, cam: &Camera) {
        self.update_lod(cam.pos.x, cam.pos.z);
        self.step_movers();
        self.pose_far_away(cam.pos);
        let mut env = self.env;
        if let Some((color, end)) = self.fog.as_ref().map(|m| m.at_scaled(cam.pos.to_array(), self.fog_density_scale)) {
            if env.sky_color == env.fog_color {
                env.sky_color = color;
            }
            (env.fog_color, env.fog_end) = (color, end);
        }
        let far = self.lens.far.unwrap_or_else(|| (env.fog_end * 1.1).max(50.0));
        let aspect = t.size.0 as f32 / t.size.1.max(1) as f32;
        let vp = Mat4::perspective_rh(self.lens.vertical_fov(aspect), aspect, self.lens.near, far) * Mat4::look_to_rh(cam.pos, cam.forward(), cam.up());
        let v4 = |c: [f32; 3], w| Vec4::new(c[0], c[1], c[2], w).to_array();
        let g4 = |c: [f32; 3], w| v4(c.map(gamma), w);
        let w = ao_scene::wave_curves(self.time);
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
            wave: std::array::from_fn(|i| std::array::from_fn(|j| w[i * 4 + j])),
            sun_g: g4(env.sun_color, env.sun_specular),
            ambient_g: g4(env.ambient, 0.0),
            fog_g: g4(env.fog_color, 1.0),
            view: Mat4::look_to_rh(cam.pos, cam.forward(), cam.up()).to_cols_array_2d(),
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
                if !inst.hidden && planes.iter().all(|p| p.dot(c) >= -inst.radius) {
                    self.vis.push(inst.m);
                    self.vis_src.push((r.start + i) as u32);
                }
            }
            self.vis_range.push(start..self.vis.len() as u32);
        }
        self.frame = (self.frame + 1) % INST_RING;
        self.prepare_actors(cam.pos, &planes);
        let inst_buf = self.gpu.inst_bufs.get(self.frame);
        if let Some(buf) = inst_buf {
            if !self.vis.is_empty() {
                self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.vis));
            }
        }
        for a in &mut self.sky.colors {
            a.sim.advance_to(self.time);
            let lin = |c: u32, shift: u32| srgb_to_linear(((c >> shift & 255) as f32 / 255.0 * a.gain).min(1.0));
            for (v, &c) in a.vertices.iter_mut().zip(a.sim.vertex_colors()) {
                v.color = [lin(c, 16), lin(c, 8), lin(c, 0), (c >> 24) as f32 / 255.0];
            }
            if let Some(Some((vb, _))) = self.sky.meshes.get(a.mesh) {
                self.queue.write_buffer(vb, 0, bytemuck::cast_slice(&vertex_bytes(&a.vertices)));
            }
        }
        let sky_buf = self.sky.bufs.get(self.frame);
        if let Some(buf) = sky_buf {
            // translation = camera; a spinning instance turns about its pivot (relative to the camera) first
            let time = self.time;
            let xf: Vec<[[f32; 4]; 4]> = self
                .sky
                .xf
                .iter()
                .zip(&self.sky.spin)
                .enumerate()
                .map(|(i, (m, spin))| {
                    let (mut m, mut off) = match spin {
                        Some(s) => {
                            let r = Mat4::from_axis_angle(Vec3::from(s.axis).normalize_or_zero(), (s.degrees_per_second * time).to_radians());
                            let p = Vec3::from(s.pivot);
                            (r * *m, p - r.transform_vector3(p))
                        }
                        None => (*m, Vec3::ZERO),
                    };
                    // `GameWaveCurve*` driven turns (`SkyWaveSpin`) compose after the constant spin
                    for ws in self.sky.wave.iter().filter(|ws| ws.instance == i) {
                        let r = Mat4::from_axis_angle(Vec3::from(ws.axis).normalize_or_zero(), (ws.degrees * w[ws.curve & 15]).to_radians());
                        let p = Vec3::from(ws.pivot);
                        m = r * m;
                        off = r.transform_vector3(off) + p - r.transform_vector3(p);
                    }
                    Mat4::from_cols(m.x_axis, m.y_axis, m.z_axis, (cam.pos + off).extend(1.0)).to_cols_array_2d()
                })
                .collect();
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

        let calls = std::cell::Cell::new(0usize);
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
            let (pipe, mesh, mat) = (std::cell::Cell::new(usize::MAX), std::cell::Cell::new((false, usize::MAX)), std::cell::Cell::new((false, usize::MAX)));
            let draw = |pass: &mut wgpu::RenderPass, d: &Draw, insts: std::ops::Range<u32>, sky: bool| {
                if pipe.get() != d.pipe {
                    pass.set_pipeline(&self.pipes[d.pipe]);
                    pipe.set(d.pipe);
                }
                if mesh.get() != (sky, d.mesh) {
                    let (vb, ib) = if sky { &self.sky.meshes } else { &self.gpu.meshes }[d.mesh].as_ref().unwrap();
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    mesh.set((sky, d.mesh));
                }
                if mat.get() != (sky, d.mat) {
                    pass.set_bind_group(1, &if sky { &self.sky.mats } else { &self.gpu.mats }[d.mat], &[]);
                    mat.set((sky, d.mat));
                }
                pass.draw_indexed(d.first_index..d.first_index + d.count, 0, insts);
                calls.set(calls.get() + 1);
            };
            if let Some(sky) = sky_buf {
                pass.set_vertex_buffer(1, sky.slice(..));
                for (d, k) in &self.sky.draws {
                    draw(&mut pass, d, *k..*k + 1, true);
                }
            }
            // Order of `DisplaySystem_t::Render` @0x100793b8: sky, the opaque lists 3 + 4 (list 3 = world meshes and the actors'
            // opaque parts; list 4 = liquids, `VisualLiquid_t` ctor @0x10067385 `SetRenderPriority(4)`), then the blended lists
            // 5 (blended meshes) and 6 (actors' blended parts, effects), each far to near.
            if let Some(inst) = inst_buf {
                pass.set_vertex_buffer(1, inst.slice(..));
                for d in &self.gpu.opaque {
                    let r = self.vis_range[d.mesh].clone();
                    if !r.is_empty() {
                        draw(&mut pass, d, r, false);
                    }
                }
            }
            calls.set(calls.get() + self.draw_actors(&mut pass, false));
            // the actor pass rebinds pipeline, buffers and material: forget the cached state
            pipe.set(usize::MAX);
            mesh.set((false, usize::MAX));
            mat.set((false, usize::MAX));
            if let Some(inst) = inst_buf {
                pass.set_vertex_buffer(1, inst.slice(..));
                for d in &self.gpu.liquid {
                    let r = self.vis_range[d.mesh].clone();
                    if !r.is_empty() {
                        draw(&mut pass, d, r, false);
                    }
                }
                for &(_, di, vi) in &self.sorted {
                    draw(&mut pass, &self.gpu.blended[di as usize], vi..vi + 1, false);
                }
            }
            calls.set(calls.get() + self.draw_actors(&mut pass, true));
        }
        self.stats = FrameStats { instances: self.vis.len(), draw_calls: calls.get() };
        self.queue.submit([enc.finish()]);
    }
}

/// Scene instance index -> index into `Gpu::insts` (`usize::MAX` = not uploaded): instances of a mesh occupy that mesh's
/// range in scene order.
fn instance_slots(scene: &Scene, mesh_range: &[std::ops::Range<usize>]) -> Vec<usize> {
    let mut seen = vec![0usize; mesh_range.len()];
    scene
        .instances
        .iter()
        .map(|i| match mesh_range.get(i.mesh) {
            Some(r) if seen[i.mesh] < r.len() => {
                seen[i.mesh] += 1;
                r.start + seen[i.mesh] - 1
            }
            _ => usize::MAX,
        })
        .collect()
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
    render_to_png_at(scene, eye, look_at, width, height, path, 0.0)
}

/// [`render_to_png`] with the scrolling textures (`Submesh::uv_scroll`) at `time` seconds.
pub fn render_to_png_at(scene: &Scene, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path, time: f32) -> Result<()> {
    render_to_png_actors(scene, &[], vec![], eye, look_at, width, height, path, time)
}

/// [`render_to_png_at`] with dynamic actors on top of the scene (`models` for [`Renderer::add_actor_model`], `actors` for [`Renderer::set_actors`]).
#[allow(clippy::too_many_arguments)]
pub fn render_to_png_actors(scene: &Scene, models: &[(u64, Scene)], actors: Vec<ao_scene::ActorFrame>, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path, time: f32) -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r = Renderer::new(&instance, None)?;
    r.upload(scene);
    for (k, m) in models {
        r.add_actor_model(*k, m);
    }
    r.set_actors(actors);
    r.seek(time);
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

    texture_to_png(&r, &out, width, height, path)
}

/// Reads back a `r.format` texture and writes it as PNG.
pub(crate) fn texture_to_png(r: &Renderer, out: &wgpu::Texture, width: u32, height: u32, path: &Path) -> Result<()> {
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
/// Light storage with the lights of zones whose level disables list 5 switched off (`range = 0`).
fn gate_lights(base: &[[f32; 4]], zones: &[Option<u32>], levels: &[u8]) -> Vec<[f32; 4]> {
    let mut data = base.to_vec();
    for (i, z) in zones.iter().enumerate() {
        if z.is_some_and(|z| !ao_scene::StatelLod::lights_active(levels[z as usize])) {
            data[i * 4][3] = 0.0;
        }
    }
    data
}

fn globals_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, globals: &wgpu::Buffer, g: &LightGrid) -> (wgpu::BindGroup, wgpu::Buffer) {
    let l = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&g.lights), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST });
    let (c, i) = (storage(device, &g.cells), storage(device, &g.idx));
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: l.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: c.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: i.as_entire_binding() },
        ],
    });
    (bg, l)
}

#[cfg(test)]
mod sky_tests {
    #[test]
    fn look_to_up_reproduces_forward_and_up() {
        for (f, u) in [(Vec3::new(0.3, -0.2, -1.0), Vec3::new(0.5, 1.0, 0.1)), (Vec3::new(-1.0, 0.1, 0.4), Vec3::new(0.0, -1.0, 0.0))] {
            let c = Camera::look_to_up(Vec3::ZERO, f, u);
            let (f, u) = (f.normalize(), (u - f.normalize() * u.dot(f.normalize())).normalize());
            assert!((c.forward() - f).length() < 1e-5 && (c.up() - u).length() < 1e-5, "{c:?}");
        }
        assert_eq!(Camera::look_at(Vec3::ZERO, -Vec3::Z).up(), Vec3::Y);
    }

    use super::*;
    use ao_scene::{Instance, Mesh, SkySpin, Submesh, Texture, TextureKey, Vertex, IDENTITY};

    fn pixels(scene: &Scene, time: f32, name: &str) -> Option<Vec<u8>> {
        let path = std::env::temp_dir().join(format!("ao-render-{}-{name}.png", std::process::id()));
        // no GPU adapter: skip
        render_to_png_at(scene, [0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 64, 64, &path, time).ok()?;
        let bytes = std::fs::read(&path).ok();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    /// A sky quad straight ahead: a 2x1 checker texture (scrolled by `uv_scroll`) or a coloured corner (turned by `sky_spin`).
    fn scene(scroll: [f32; 2], spin: Option<f32>) -> Scene {
        let key = TextureKey { rdb_type: 1, id: 1 };
        let mut s = Scene::default();
        s.textures.insert(key, Texture { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 255] });
        let v = |x: f32, y: f32, uv: [f32; 2], color: [f32; 4]| Vertex { pos: [x, y, -10.0], uv, color, ..Default::default() };
        let c = [[1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0]];
        let vertices = vec![v(-9.0, -9.0, [0.0, 0.0], c[0]), v(9.0, -9.0, [1.0, 0.0], c[1]), v(9.0, 9.0, [1.0, 1.0], c[2]), v(-9.0, 9.0, [0.0, 1.0], c[3])];
        let sub = Submesh { two_sided: true, uv_scroll: scroll, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key)) };
        s.meshes.push(Mesh { vertices, submeshes: vec![sub] });
        s.sky.push(Instance { mesh: 0, transform: IDENTITY });
        s.sky_spin.extend(spin.map(|d| SkySpin { instance: 0, axis: [0.0, 0.0, 1.0], pivot: [0.0; 3], degrees_per_second: d }));
        s
    }

    /// `GameWaveCurve2` is 1 at t = 0 and 0 at t = 3.6 s (phase 0, 50 deg/s): the wave driven uv offset, rotation and the sun fan
    /// rim alpha must change the picture between the two instants.
    #[test]
    fn game_wave_curves_drive_uv_rotation_and_sun_flicker() {
        let mut uv = scene([0.0; 2], None);
        uv.meshes[0].submeshes[0].uv_wave = [2.0, 0.5, 0.0, 0.0];
        let mut rot = scene([0.0; 2], None);
        rot.sky_wave_spin.push(ao_scene::SkyWaveSpin { instance: 0, axis: [0.0, 0.0, 1.0], pivot: [0.0; 3], curve: 2, degrees: 90.0 });
        let mut sun = scene([0.0; 2], None);
        for (i, v) in sun.meshes[0].vertices.iter_mut().enumerate() {
            v.normal = [0.0, -1.0, (i + 2) as f32]; // rim index 2..5: table entry 2 (= curve 2) fades the first corner
        }
        sun.meshes[0].submeshes[0].sun_flicker = true;
        sun.meshes[0].submeshes[0].blend = ao_scene::Blend::AlphaBlend;
        for (name, s) in [("uv_wave", uv), ("wave_spin", rot), ("sun_flicker", sun)] {
            let (Some(a), Some(b)) = (pixels(&s, 0.0, name), pixels(&s, 3.6, name)) else { return };
            assert_ne!(a, b, "{name}: curve 2 changes between t = 0 and 3.6 s");
        }
    }

    #[test]
    fn uv_scroll_and_sky_spin_change_the_picture_with_time() {
        for (name, scroll, spin) in [("scroll", [0.5, 0.0], None), ("spin", [0.0, 0.0], Some(90.0))] {
            let s = scene(scroll, spin);
            let (Some(a), Some(b), Some(c)) = (pixels(&s, 0.0, name), pixels(&s, 1.0, name), pixels(&s, 0.0, name)) else { return };
            assert_ne!(a, b, "{name}: time 1 s differs from time 0");
            assert_eq!(a, c, "{name}: deterministic at the same time");
        }
        // a still sky does not depend on time
        let s = scene([0.0; 2], None);
        let (Some(a), Some(b)) = (pixels(&s, 0.0, "still"), pixels(&s, 5.0, "still")) else { return };
        assert_eq!(a, b);
    }

    /// Red channel of the centre pixel of a PNG.
    fn centre(png_bytes: &[u8]) -> u8 {
        let mut r = png::Decoder::new(std::io::Cursor::new(png_bytes)).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        buf[(info.width as usize * (info.height as usize / 2) + info.width as usize / 2) * 4]
    }

    /// Actor layer: a 1 m quad model (mesh 0 = body, mesh 1 = rigid mount) drawn at the actor transform, moved by new skin vertices
    /// and part transforms, not visible behind the camera.
    #[test]
    fn actor_layer_draws_skins_mounts_and_culls() {
        let quad = |x0: f32, col: [f32; 4]| Mesh {
            vertices: [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].iter().map(|&(x, y)| Vertex { pos: [x0 + x, y, 0.0], normal: [0.0, 0.0, 1.0], color: col, ..Default::default() }).collect(),
            submeshes: vec![Submesh { two_sided: true, emissive: [1.0; 3], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
        };
        let model = Scene { meshes: vec![quad(0.0, [1.0, 0.0, 0.0, 1.0]), quad(0.0, [1.0, 0.0, 0.0, 1.0])], ..Default::default() };
        let at = |x: f32, z: f32| [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, 0.0, z, 1.0]];
        let frame = |transform, parts: Vec<[[f32; 4]; 4]>, skin: Option<Vec<Vertex>>, always| ao_scene::ActorFrame { id: 1, model: 7, transform, parts, skin, always, alpha: 1.0 };
        let shot = |f: ao_scene::ActorFrame, name: &str| -> Option<u8> {
            let path = std::env::temp_dir().join(format!("ao-render-actor-{}-{name}.png", std::process::id()));
            render_to_png_actors(&Scene::default(), &[(7, model.clone())], vec![f], [0.0; 3], [0.0, 0.0, -1.0], 64, 64, &path, 0.0).ok()?;
            let bytes = std::fs::read(&path).ok()?;
            let _ = std::fs::remove_file(&path);
            Some(centre(&bytes))
        };
        // the body quad (mesh 1 is pushed away by its part transform) straight ahead
        let away = at(100.0, 0.0);
        let Some(on) = shot(frame(at(0.0, -5.0), vec![IDENTITY, away], None, false), "on") else { return };
        assert!(on > 240, "actor drawn: {on}");
        // new skin vertices move the body out of the centre
        let moved: Vec<Vertex> = model.meshes[0].vertices.iter().map(|v| Vertex { pos: [v.pos[0] + 50.0, v.pos[1], v.pos[2]], ..*v }).collect();
        assert!(shot(frame(at(0.0, -5.0), vec![IDENTITY, away], Some(moved), false), "skin").unwrap() < 240);
        // the rigid mount follows its part transform into the centre while the body is off to the side
        assert!(shot(frame(at(50.0, -5.0), vec![IDENTITY, at(-50.0, 0.0)], None, false), "mount").unwrap() > 240);
        // behind the camera: not visible
        assert!(shot(frame(at(0.0, 5.0), vec![IDENTITY, away], None, false), "behind").unwrap() < 240);
    }

    /// RGB of the centre pixel of an actors-only render (`None` without a GPU adapter).
    fn actor_shot(world: &Scene, model: Scene, at: [f32; 3], name: &str) -> Option<[u8; 3]> {
        actor_shot_alpha(world, model, at, 1.0, name)
    }

    /// [`actor_shot`] with `ActorFrame::alpha`.
    fn actor_shot_alpha(world: &Scene, model: Scene, at: [f32; 3], alpha: f32, name: &str) -> Option<[u8; 3]> {
        let path = std::env::temp_dir().join(format!("ao-render-env-{}-{name}.png", std::process::id()));
        let t = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [at[0], at[1], at[2], 1.0]];
        let f = ao_scene::ActorFrame { id: 1, model: 7, transform: t, parts: vec![], skin: None, always: false, alpha };
        render_to_png_actors(world, &[(7, model)], vec![f], [0.0; 3], [0.0, 0.0, -1.0], 64, 64, &path, 0.0).ok()?;
        let bytes = std::fs::read(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        let mut r = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        let o = (info.width as usize * (info.height as usize / 2) + info.width as usize / 2) * 4;
        Some([buf[o], buf[o + 1], buf[o + 2]])
    }

    /// `ActorFrame::alpha` (`RRefFrame_t::SetTransparency`): an opaque emissive quad blends with the frame's alpha over the world behind it
    /// (and still depth-writes), alpha 0 draws nothing; alpha-blended and alpha-tested submeshes follow the same factor.
    #[test]
    fn actor_alpha_blends_opaque_and_tested_submeshes() {
        let red = |blend| actor_quad([0.0, 0.0, 1.0], Submesh { blend, emissive: [1.0; 3], base_color: [1.0, 0.0, 0.0, 1.0], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) });
        // a white emissive wall behind the actor
        let wall = {
            let v = |x: f32, y: f32| Vertex { pos: [x, y, -9.0], normal: [0.0, 0.0, 1.0], ..Default::default() };
            let sub = Submesh { two_sided: true, emissive: [1.0; 3], base_color: [1.0; 4], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) };
            let mut s = Scene::default();
            s.meshes.push(Mesh { vertices: vec![v(-9.0, -9.0), v(9.0, -9.0), v(9.0, 9.0), v(-9.0, 9.0)], submeshes: vec![sub] });
            s.instances.push(Instance { mesh: 0, transform: IDENTITY });
            s
        };
        for blend in [ao_scene::Blend::Opaque, ao_scene::Blend::AlphaTest, ao_scene::Blend::AlphaBlend] {
            let shot = |alpha, name: &str| actor_shot_alpha(&wall, red(blend), [0.0, 0.0, -5.0], alpha, &format!("{name}{}", blend as u32));
            let Some(solid) = shot(1.0, "a1") else { return };
            let (half, gone) = (shot(0.5, "a05").unwrap(), shot(0.0, "a0").unwrap());
            assert!(solid[0] > 240 && solid[1] < 20, "{blend:?} solid red {solid:?}");
            assert!(half[0] > 200 && (100..200).contains(&half[1]), "{blend:?} half red over the white wall {half:?}");
            assert!(gone[0] > 240 && gone[1] > 240 && gone[2] > 240, "{blend:?} alpha 0 draws nothing {gone:?}");
        }
    }

    /// A 1 m quad at the origin of model space facing +Z (one-sided = false) with per-vertex `normal`.
    fn actor_quad(normal: [f32; 3], sub: Submesh) -> Scene {
        let vertices = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].iter().map(|&(x, y)| Vertex { pos: [x, y, 0.0], normal, ..Default::default() }).collect();
        Scene { meshes: vec![Mesh { vertices, submeshes: vec![Submesh { two_sided: true, ..sub }] }], ..Default::default() }
    }

    /// `Submesh::env_texture`: a black (unlit) CAT material gets the env texture added in a second pass (`SRC = DEST = ONE`, no
    /// lighting), its uv generated from the camera-space normal with no v flip (`ao_scene::env_uv`).
    #[test]
    fn env_layer_is_an_additive_camera_normal_sphere_map() {
        let env = TextureKey { rdb_type: 2, id: 2 };
        let mut model_tex = Scene::default();
        // 2x2: top-left red, top-right green, bottom-left blue, bottom-right white
        model_tex.textures.insert(env, Texture { width: 2, height: 2, rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255] });
        let black = || Submesh { base_color: [0.0, 0.0, 0.0, 1.0], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) };
        let model = |normal: [f32; 3], with_env: bool| {
            let mut m = actor_quad(normal, Submesh { env_texture: with_env.then_some(env), ..black() });
            m.textures = model_tex.textures.clone();
            m
        };
        let n = [0.5, -0.5, 0.7]; // view-space normal pointing right and down: uv = (0.75, 0.25) = the top-right texel
        let Some(plain) = actor_shot(&Scene::default(), model(n, false), [0.0, 0.0, -5.0], "plain") else { return };
        assert_eq!(plain, [0, 0, 0], "black material, no light: nothing to see");
        let with = actor_shot(&Scene::default(), model(n, true), [0.0, 0.0, -5.0], "env").unwrap();
        assert!(with[1] > 200 && with[0] < 40 && with[2] < 40, "top-right (green) texel: {with:?}");
        // up-left normal -> bottom row would be v > 0.5: a +y normal samples the *bottom* texels (no flip), here bottom-left blue
        let up_left = actor_shot(&Scene::default(), model([-0.5, 0.5, 0.7], true), [0.0, 0.0, -5.0], "up_left").unwrap();
        assert!(up_left[2] > 200 && up_left[0] < 40 && up_left[1] < 40, "bottom-left (blue) texel: {up_left:?}");
    }

    /// Render list order (`DisplaySystem_t::Render` @0x100793b8): a liquid (list 4) is drawn after an opaque actor (list 3) even though
    /// it is part of the world, so water in front of an actor tints it; an actor in front of the water stays opaque.
    #[test]
    fn liquid_is_drawn_after_opaque_actors() {
        let water = |z: f32| {
            let v = |x: f32, y: f32| Vertex { pos: [x, y, z], normal: [0.0, 0.0, 1.0], ..Default::default() };
            let sub = Submesh { two_sided: true, blend: ao_scene::Blend::AlphaBlend, base_color: [0.0, 0.0, 1.0, 0.5], emissive: [1.0; 3], liquid: true, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) };
            let mut s = Scene::default();
            s.meshes.push(Mesh { vertices: vec![v(-5.0, -5.0), v(5.0, -5.0), v(5.0, 5.0), v(-5.0, 5.0)], submeshes: vec![sub] });
            s.instances.push(Instance { mesh: 0, transform: IDENTITY });
            s
        };
        let red = || actor_quad([0.0, 0.0, 1.0], Submesh { emissive: [1.0; 3], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) });
        let mut red = red();
        red.meshes[0].vertices.iter_mut().for_each(|v| v.color = [1.0, 0.0, 0.0, 1.0]);
        // water at z = -3 in front of the actor at z = -5: the actor shows through half transparent blue
        let Some(behind) = actor_shot(&water(-3.0), red.clone(), [0.0, 0.0, -5.0], "water_front") else { return };
        // half red + half blue blended in the sRGB target (linear 0.5 = 188 per channel)
        assert!((150..230).contains(&behind[0]) && (150..230).contains(&behind[2]), "red actor under half-transparent blue water: {behind:?}");
        // water at z = -8 behind the actor: depth test hides it, the actor is pure red
        let front = actor_shot(&water(-8.0), red, [0.0, 0.0, -5.0], "water_back").unwrap();
        assert!(front[0] > 240 && front[2] < 20, "actor in front of the water: {front:?}");
    }

    /// A `half`-metre wide quad facing the camera at z = -10 (normal +Z), lit by one white D3D light and nothing else.
    fn lit_quad(half: f32, light: [f32; 3], base: f32, specular: f32) -> Scene {
        let mut s = Scene::default();
        let v = |x: f32, y: f32| Vertex { pos: [x, y, -10.0], normal: [0.0, 0.0, 1.0], ..Default::default() };
        let vertices = vec![v(-half, -half), v(half, -half), v(half, half), v(-half, half)];
        let sub = Submesh { two_sided: true, base_color: [base, base, base, 1.0], specular: [specular; 3], shininess: 10.0, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) };
        s.meshes.push(Mesh { vertices, submeshes: vec![sub] });
        s.instances.push(Instance { mesh: 0, transform: IDENTITY });
        s.environment = Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 1e4, fog_end: 2e4, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 1.0 });
        s.lights.push(ao_scene::Light { pos: light, color: [1.0; 3], range: 1000.0, atten: [1.0, 0.0, 0.0], ..Default::default() });
        s
    }

    /// D3D7 lights per vertex: the middle of a big quad whose corners the light barely grazes stays dark (a per-pixel light
    /// would be at full N.L right under the light), and a point light closer to the quad lights its corners more.
    #[test]
    fn diffuse_light_is_evaluated_per_vertex_and_interpolated() {
        let Some(far) = pixels(&lit_quad(20.0, [0.0, 0.0, -8.0], 1.0, 0.0), 0.0, "gouraud") else { return };
        // corners: N.L = 2 / sqrt(800 + 4) = 0.07; the lighting sum is gamma space (the client's), so the interpolated centre is
        // 0.07 * 255 = 18, not the per-pixel 255
        assert!((14..24).contains(&centre(&far)), "centre {}", centre(&far));
    }

    /// `SPECULARENABLE` materials add `specular * (N.H)^power` after the texture stage, also on a black diffuse material.
    #[test]
    fn specular_term_is_added_after_the_texture_stage() {
        let (Some(on), Some(off)) = (pixels(&lit_quad(2.0, [0.0; 3], 0.0, 1.0), 0.0, "spec_on"), pixels(&lit_quad(2.0, [0.0; 3], 0.0, 0.0), 0.0, "spec_off")) else { return };
        // light at the eye: H = L, N.H = N.L = 10 / sqrt(108) = 0.962 at the corners -> 0.962^10 = 0.68
        assert_eq!(centre(&off), 0);
        assert!(centre(&on) > 150, "specular {}", centre(&on));
    }

    /// D3D sums ambient + sun in framebuffer (gamma) space and saturates there: 0.5 + 0.5 = 1.0 (255), where a linear-space sum of
    /// the same colours would only reach 176.
    #[test]
    fn lighting_sum_is_formed_in_gamma_space() {
        let mut s = lit_quad(2.0, [0.0; 3], 1.0, 0.0);
        s.lights.clear();
        let e = s.environment.as_mut().unwrap();
        (e.ambient, e.sun_color) = ([0.5_f32.powf(2.2); 3], [0.5_f32.powf(2.2); 3]);
        let Some(png) = pixels(&s, 0.0, "gamma_sum") else { return };
        assert!(centre(&png) >= 250, "sum {}", centre(&png));
    }

    /// Emissive is part of the saturated vertex colour: a fully emissive texel is the texture, not twice the texture.
    #[test]
    fn emissive_is_saturated_with_the_lighting() {
        let mut s = lit_quad(2.0, [0.0; 3], 0.5_f32.powf(2.2), 0.0);
        s.lights.clear();
        s.meshes[0].submeshes[0].emissive = [1.0; 3];
        s.environment.as_mut().unwrap().ambient = [1.0; 3];
        let Some(png) = pixels(&s, 0.0, "emissive_sat") else { return };
        assert!((120..136).contains(&centre(&png)), "emissive {}", centre(&png));
    }

    /// The sun is a directional light with specular colour `sun_specular * sun_color`.
    #[test]
    fn sun_specular_follows_the_light_intensity() {
        let build = |intensity: f32| {
            let mut s = lit_quad(2.0, [0.0; 3], 0.0, 1.0);
            s.lights.clear();
            let e = s.environment.as_mut().unwrap();
            (e.sun_color, e.sun_specular) = ([1.0; 3], intensity);
            s
        };
        let (Some(on), Some(off)) = (pixels(&build(1.0), 0.0, "sun_spec_on"), pixels(&build(0.0), 0.0, "sun_spec_off")) else { return };
        // sun along +Z towards the eye: N.H = 1 on the whole quad, so the highlight is the full material specular
        assert!(centre(&on) > 200, "specular {}", centre(&on));
        assert_eq!(centre(&off), 0);
    }
}

#[cfg(test)]
mod lod_tests {
    use super::gate_lights;

    #[test]
    fn lights_follow_their_zone_level() {
        let base: Vec<[f32; 4]> = (0..3).flat_map(|i| [[0.0, 0.0, 0.0, 10.0 + i as f32], [1.0; 4], [0.0; 4], [0.0; 4]]).collect();
        // light 0: zone 0 (level 2: on), light 1: zone 1 (level 0: off), light 2: no zone (always on)
        let d = gate_lights(&base, &[Some(0), Some(1), None], &[2, 0]);
        assert_eq!([d[0][3], d[4][3], d[8][3]], [10.0, 0.0, 12.0]);
        assert_eq!(d[5], [1.0; 4]);
    }
}
