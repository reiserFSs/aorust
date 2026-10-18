//! wgpu renderer for `ao_scene::Scene`: windowed free-fly viewer (`viewer`) and offscreen PNG path.

mod actors;
mod bloom;
mod contact_shadows;
mod atmosphere;
mod atmosphere_stack;
#[cfg(test)]
mod atmosphere_stack_tests;
mod volumetric_clouds;
mod cloud_shadows;
mod sky_discs;
mod grade;
mod temporal;
mod spatial_post;
mod cube_lut;
#[cfg(test)]
mod post_temporal_tests;
mod grass;
pub use grass::{GrassBender, GrassExposure};
mod gui;
mod viewer;
mod screen;
#[cfg(test)]
mod depth_clear_tests;
#[cfg(test)]
mod first_use_tests;
mod pipeline_creation;
pub use screen::{ScreenLayer,ViewportDepthClear};
mod native_fog;
pub use native_fog::{NativeFog,SharedNativeFog,NativeFogControl};
mod native_ground;
mod native_texture;
mod native_liquid;
mod water;
mod water_waves;
mod water_dynamics;
mod water_optics;
mod water_spray;
mod water_bottom;
#[cfg(test)]
mod water_tests;
mod pipelines;
mod foliage_edges;
mod foliage;
mod foliage_detail;
mod foliage_leaves;
use pipelines::{Pipelines, Slot as PipelineSlot, Spec as PipelineSpec};
mod heat_haze;
mod gpu_stats;
pub use gpu_stats::FrameGpuTimer;
mod enhanced_settings;
pub use enhanced_settings::{EnhancedSettings, NightMode};
mod enhanced;
mod clustered_lighting;
mod authored_specular;
mod enhanced_shader;
mod moon_shadows;
mod moon_exposure;
mod variant;
mod shader_source;
mod ambient_occlusion;
mod sky_probe;
mod sun_shadows;
mod point_shadows;
mod shadow_pass;
mod shadow_coverage;
#[cfg(test)]
mod shadow_tests;
#[cfg(test)]
mod shadow_temporal_tests;
use gpu_stats::{GpuStatsExt, TextureViewStatsExt};
pub use native_ground::GroundPath;

use anyhow::{anyhow, Context, Result};
use ao_scene::{Blend, Environment, Scene, TextureKey};
pub use glam::{Vec3, Vec4};
pub use winit::keyboard::KeyCode;
use glam::Mat4;
use std::collections::HashMap;
use std::path::Path;

pub use gui::GuiRenderer;
pub use viewer::{run_frontend, GameInput, Offscreen, run_viewer, run_viewer_hooked, run_viewer_live, FrameHook, Frontend, Host, LiveSky};
pub use actors::{frustum_culls_sphere, POSE_MARGIN};

/// Downward-clamps `requested` to the closest level present in `supported` (1 is always valid even when `supported` omits it; a pure
/// function so it's testable without a GPU).
fn clamp_msaa_samples(requested: u32, supported: &[u32]) -> u32 {
    supported.iter().copied().filter(|&c| c <= requested).max().unwrap_or(1)
}
const INST_RING: usize = 3;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SKY_SRGB: [f32; 3] = [0.53, 0.72, 0.92];

fn srgb_to_linear(c: f32) -> f32 {
    c.powf(2.2)
}

/// Native camera-relative sky units: scale geometry/offset by far/1000, never the camera translation.
fn sky_instance_matrix(matrix: Mat4, pivot_offset: Vec3, camera: Vec3, scale: f32) -> Mat4 {
    Mat4::from_cols(matrix.x_axis * scale, matrix.y_axis * scale, matrix.z_axis * scale,
        (camera + (pivot_offset + matrix.w_axis.truncate()) * scale).extend(1.0))
}

/// Inverse of [`srgb_to_linear`]: the scene contract's `c^2.2` colours back to the client's framebuffer-space values.
fn gamma(c: f32) -> f32 {
    c.max(0.0).powf(1.0 / 2.2)
}

fn framebuffer_clear(linear: [f32; 3]) -> wgpu::Color {
    // Match the former sRGB render-target encoding, without letting blending decode the destination.
    let encode = |c: f32| if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    let [r, g, b] = linear.map(encode);
    wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }
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
    /// xyz = camera position, w = projection far (not the local fog-volume end).
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
    water_dir: [f32; 4],
    water_g: [f32; 4],
    /// x = moving light count, y = extra directional count (sun uses its existing lanes).
    dyn_info: [i32; 4],
    /// x = DotStars per-FXS-frame Counter (not elapsed seconds).
    sky_counter: [f32; 4],
    /// Pairs of toward-light direction / gamma diffuse+specular colour.
    directional: [[f32; 4]; 16],
    /// Shared Enhanced authored-moon direction and gamma-space illumination.
    moon_direction: [f32; 4],
    moon_g: [f32; 4],
    foliage_wind: [f32; 4],
    foliage_flags: [f32; 4],
    foliage_pushers: [[f32; 4]; 16],
}

/// One submesh draw. Scene pipes use `material_pipe`; sky draws use `sky_material_pipe`.
#[derive(Clone, Copy)]
struct Draw {
    mesh: usize,
    first_index: u32,
    count: u32,
    mat: usize,
    pipe: usize,
    priority: Option<i32>,
    node: Option<(u32, i32)>,
    origin: [f32; 3],
    projection_pass: bool,
    vegetation: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum VisualKey {
    World(usize, u32),
    Actor(u32, u64, usize, u32),
}

fn visual_priority(draw: &Draw, key: Option<VisualKey>, states: &HashMap<VisualKey, i32>) -> Option<i32> {
    key.and_then(|key| states.get(&key).copied()).or(draw.priority)
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
const SPRITE_PIPE: usize = 20;
const NATIVE_BLEND_PIPE: usize = 22;
/// Original two-sided sky `SourceColorOne` slot (`fs_sky_blend`, native `(Src, One)`).
/// It is separate from the four contiguous base sky blends at `SKY_PIPE`.
const SKY_GLOW_PIPE: usize = NATIVE_BLEND_PIPE + 8;
const MATERIAL_STATE_PIPE: usize = SKY_GLOW_PIPE + 1;
const MATERIAL_FADE_PIPE: usize = MATERIAL_STATE_PIPE + 18;
const MATERIAL_ENV_WRITE_PIPE: usize = MATERIAL_FADE_PIPE + 4;
const MATERIAL_ONE_PIPE: usize = MATERIAL_ENV_WRITE_PIPE + 4;
const MATERIAL_COMPARE_PIPE: usize = MATERIAL_ONE_PIPE + 2;
const MATERIAL_COMPARE_FADE_PIPE: usize = MATERIAL_COMPARE_PIPE + 108;
const MATERIAL_ENV_COMPARE_PIPE: usize = MATERIAL_COMPARE_FADE_PIPE + 24;
const MESH_EFFECT_STRIDE: usize = MATERIAL_ENV_COMPARE_PIPE + 16;
const MESH_EFFECT_PIPE: usize = MESH_EFFECT_STRIDE;
/// Append sky-only cull variants without moving world/actor pipeline slots.
const SKY_CULL_PIPE: usize = SKY_GLOW_PIPE + 1;
const SKY_NATIVE_PIPE: usize = SKY_CULL_PIPE + 8;
const SKY_POINT_PIPE: usize = SKY_NATIVE_PIPE + 3;
const SKY_LAST_PIPE: usize = SKY_POINT_PIPE;
const WORLD_MATERIAL_STATE_PIPE: usize = native_liquid::PIPE + 5;
const WORLD_COMPARE_PIPE: usize = WORLD_MATERIAL_STATE_PIPE + 18;
const WORLD_ONE_PIPE: usize = WORLD_COMPARE_PIPE + 108;
const SKY_STATE_PIPE: usize = WORLD_ONE_PIPE + 2;

fn material_depth_write(s: &ao_scene::Submesh) -> bool {
    s.depth_write.unwrap_or(matches!(s.blend, Blend::Opaque | Blend::AlphaTest)
        || (s.blend == Blend::AlphaBlend && s.sprite_alpha_test))
}

fn comparison_slot(s: &ao_scene::Submesh) -> Option<usize> {
    let mode = match s.depth_func { Some(2) => 0, Some(4) => 1, Some(8) => 2, _ => return None };
    Some(mode * 36 + material_depth_write(s) as usize * 18 + s.blend as usize * 2 + s.two_sided as usize)
}

fn world_material_pipe(s: &ao_scene::Submesh) -> usize {
    if let Some(slot) = comparison_slot(s) { return WORLD_COMPARE_PIPE + slot; }
    let pipe = material_pipe(s);
    if (MATERIAL_STATE_PIPE..MATERIAL_FADE_PIPE).contains(&pipe) {
        WORLD_MATERIAL_STATE_PIPE + pipe - MATERIAL_STATE_PIPE
    } else if (MATERIAL_ONE_PIPE..MATERIAL_COMPARE_PIPE).contains(&pipe) {
        WORLD_ONE_PIPE + pipe - MATERIAL_ONE_PIPE
    } else { pipe }
}

fn sky_material_pipe(s: &ao_scene::Submesh) -> usize {
    if s.sky_points { return SKY_POINT_PIPE; }
    if s.depth_func.is_some() || s.depth_write.is_some() || s.blend == Blend::AdditiveOne {
        let mode = match s.depth_func.unwrap_or(8) { 2 => 0, 4 => 1, _ => 2 };
        return SKY_STATE_PIPE + mode * 36 + s.depth_write.unwrap_or(false) as usize * 18
            + s.blend as usize * 2 + s.two_sided as usize;
    }
    if !s.two_sided {
        SKY_CULL_PIPE + s.blend as usize
    } else {
        match s.blend {
            Blend::Opaque | Blend::AlphaTest | Blend::AlphaBlend | Blend::Additive => SKY_PIPE + s.blend as usize,
            Blend::SourceColorOne => SKY_GLOW_PIPE,
            _ => SKY_NATIVE_PIPE + s.blend as usize - Blend::ZeroSourceColor as usize,
        }
    }
}

/// Drop invalid whole primitives without truncating a point list to a multiple of three.
fn append_primitive_indices(out: &mut Vec<u32>, s: &ao_scene::Submesh, vertex_count: u32) {
    if s.sky_points {
        // D3DPT_POINTLIST consumes one vertex per primitive (Randy100227b6).
        out.extend(s.indices.iter().copied().filter(|&i| i < vertex_count));
    } else {
        for triangle in s.indices.as_chunks::<3>().0 {
            if triangle.iter().all(|&i| i < vertex_count) {
                out.extend_from_slice(triangle.as_slice());
            }
        }
    }
}

fn native_blend_state(blend: Blend) -> wgpu::BlendState {
    use wgpu::BlendFactor::*;
    // D3D7 has no separate alpha state: SRCCOLOR/DSTCOLOR use the alpha channels for alpha too.
    let (src_factor, dst_factor) = match blend {
        Blend::ZeroSourceColor => (Zero, Src),
        Blend::DestinationColorSourceColor => (Dst, Src),
        Blend::PremultipliedAlpha => (One, OneMinusSrcAlpha),
        Blend::SourceColorOne => (Src, One),
        _ => unreachable!("not a native effect blend"),
    };
    let component = wgpu::BlendComponent { src_factor, dst_factor, operation: wgpu::BlendOperation::Add };
    wgpu::BlendState { color: component, alpha: component }
}

fn blended_material(blend: Blend) -> bool {
    !matches!(blend, Blend::Opaque | Blend::AlphaTest)
}

fn material_pipe(s: &ao_scene::Submesh) -> usize {
    if let Some(slot) = comparison_slot(s) { return MATERIAL_COMPARE_PIPE + slot; }
    let default_write = matches!(s.blend, Blend::Opaque | Blend::AlphaTest);
    if s.depth_write.is_some_and(|write| write != default_write) {
        return MATERIAL_STATE_PIPE + s.blend as usize * 2 + s.two_sided as usize;
    }
    if s.blend == Blend::AdditiveOne { return MATERIAL_ONE_PIPE + s.two_sided as usize; }
    if s.blend == Blend::AlphaBlend && s.sprite_alpha_test {
        SPRITE_PIPE + s.two_sided as usize
    } else if s.blend as usize >= Blend::ZeroSourceColor as usize {
        NATIVE_BLEND_PIPE + (s.blend as usize - Blend::ZeroSourceColor as usize) * 2 + s.two_sided as usize
    } else {
        s.blend as usize * 2 + s.two_sided as usize
    }
}
/// Lights kept per grid cell (strongest first) and the minimum cell edge in metres.
const CELL_LIGHTS: usize = 16;
const MIN_CELL: f32 = 8.0;
const MAX_CELLS: usize = 1 << 20;
/// Fixed capacity of the per-frame moving-light buffer ([`Renderer::set_dynamic_lights`]): worn/attached item lights
/// (the Light Bar and similar), rewritten every frame; D3D7's own 8-light-per-vertex cap (`vertex_lights` in the shader)
/// already bounds how many actually contribute, so this is a generous ceiling, not a faithfulness constant.
const MAX_DYN_LIGHTS: usize = 16;

/// Static light grid: dense 3D cells, each listing up to `CELL_LIGHTS` lights whose sphere touches it.
struct LightGrid {
    zones: Vec<Option<u32>>, // statel zone per light (`Light::zone`), same order as `lights`
    owners: Vec<Option<u32>>,
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
            return Self { zones: vec![], owners: vec![], lights: vec![[0.0; 4]; 4], cells: vec![[0, 0]], idx: vec![0], origin: Vec3::ZERO, cell: 1.0, dims: [0; 3] };
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
        let gl = lights.iter().flat_map(|l| pack_light(l)).collect();
        Self { zones: lights.iter().map(|l| l.zone).collect(), owners: lights.iter().map(|l| l.owner).collect(), lights: gl, cells, idx, origin: lo, cell, dims }
    }
}

/// Packs one light into the shader's 4-`vec4` layout: `(pos, range), (colour, cos(phi/2) | 2 = no cone), (atten0..2, cos(theta/2)), (axis, 0)`.
/// Shared by the static [`LightGrid`] and [`Renderer::set_dynamic_lights`] (moving, per-frame lights).
fn pack_light(l: &ao_scene::Light) -> [[f32; 4]; 4] {
    let (cos_phi, cos_theta, axis) = l.spot.map_or((2.0, 0.0, [0.0; 3]), |s| ((s.phi * 0.5).cos(), (s.theta * 0.5).cos(), s.dir));
    let a = if l.atten == [0.0; 3] { [-1.0, 0.0, 0.0] } else { l.atten }; // a0 < 0: linear
    // colours go to the shader in D3D (gamma) space, where the fixed function lighting sums them
    [[l.pos[0], l.pos[1], l.pos[2], l.range], [gamma(l.color[0]), gamma(l.color[1]), gamma(l.color[2]), cos_phi], [a[0], a[1], a[2], cos_theta], [axis[0], axis[1], axis[2], 0.0]]
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
    sound_time: f32,
    sound_trigger: f32,
    sounds: [Option<ao_scene::mover::SoundEvent>; 3],
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
    opaque: Vec<Draw>,          // Opaque/alpha-test storage; submission follows authored native lists.
    blended: Vec<Draw>,         // Blended storage; actual list and distance bucket are per visual.
    liquid: Vec<Draw>,          // `Submesh::liquid` (render list 4): opaque kinds first, then blended; after the opaque phase + actors
    insts: Vec<Inst>,           // grouped by mesh
    mesh_range: Vec<std::ops::Range<usize>>,
    radius: f32,
    grid: ([f32; 4], [i32; 4]), // light grid origin+cell, dims
}

struct GroundRun {
    detail: ao_scene::ground_detail::GroundDetail,
    indices: Vec<Vec<Vec<u32>>>,
    draws: Vec<Vec<usize>>,
    scratch: Vec<u32>,
    materials: Vec<Option<wgpu::BindGroup>>,
    has_shadow: Vec<bool>,
    texture_materials: Vec<(usize, TextureKey, [f32; 32])>,
    composites: Vec<native_ground::Composite>,
    /// Which texels of each ground tile picture are green (Enhanced grass placement).
    green: grass::GreenMasks,
    /// Collision-baked terrain sky openness per fine cell; never grass floor occupancy.
    sky_open: Vec<Vec<f32>>,
}

type SkyMaterialKey = (Option<TextureKey>, Option<TextureKey>, bool, [u32; 32]);

/// Sky backdrop with its own mesh/material tables, so [`Renderer::set_sky`] can swap it while the world stays.
///
/// `meshes`/`mats`/`mat_of`/`topology` persist across `set_sky` calls (the fast path writes new vertex/index/material
/// data into them in place instead of recreating buffers and bind groups): `LiveSky` calls `set_sky` about twice a
/// second for the whole time a zone is outdoors, so recreating every GPU object on every tick (the previous
/// behaviour) was the dominant confirmed GPU-object leak (`AOMAC_GPU_STATS=1`, live at ICC: a steady, never-zero
/// per-second churn of sky vertex/index/uniform buffers and bind groups, independent of actor/combat/movement).
#[derive(Default)]
struct SkyGpu {
    meshes: Vec<Option<(wgpu::Buffer, wgpu::Buffer)>>, // indexed like the uploaded scene's meshes
    /// Current GPU buffer sizes in bytes of `meshes[i]`, next power of two of the largest vertex/index byte count
    /// seen since the last full rebuild (mirrors `ModelMesh::vertex_capacity`/`index_capacity` in `actors.rs`).
    vertex_capacity: Vec<u64>,
    index_capacity: Vec<u64>,
    /// `(vertex count, submeshes)` of `meshes[i]`'s last upload: the fast path's eligibility check (same counts
    /// and `actors::same_material` submeshes) against the new scene's meshes.
    topology: Vec<(usize, Vec<ao_scene::Submesh>)>,
    /// Bind group index by `(texture, clamp, uniform)`, content-addressed and persisted: a submesh whose material
    /// is byte-identical to one already uploaded (the common case -- `Submesh::uv_scroll` is a rate, not a baked
    /// offset, so a given sky submesh's uniform is stable tick to tick) reuses the existing uniform buffer and
    /// bind group instead of creating new ones. Bounded by the sky's own small, fairly static material count
    /// (sun, moon, stars, a handful of cloud/overlay layers); never pruned, same as the rest of this cache.
    mat_of: HashMap<SkyMaterialKey, usize>,
    mats: Vec<wgpu::BindGroup>,
    draws: Vec<(Draw, u32)>, // (draw, index into `xf`) in scene order
    xf: Vec<Mat4>,
    spin: Vec<Option<ao_scene::SkySpin>>, // per `xf` entry
    wave: Vec<ao_scene::SkyWaveSpin>, // rotations driven by `GameWaveCurve*`
    bufs: Vec<wgpu::Buffer>, // ring of per-frame instance transforms (translation = camera)
    colors: Vec<ColorAnim>,
    clouds: Vec<CloudAnim>,
    directional_epoch: f32,
    /// Pose-only resource snapshot: no textures or world geometry; sun fans retain their tiny alpha buffers.
    pose: Scene,
    fan_alpha: Vec<(usize, f32)>,
    /// `set_sky` calls that fell through to a full rebuild (every GPU buffer/bind group recreated), since the
    /// renderer started: a regression counter, not drained. In steady state (a stable sky mesh/material set) this
    /// must stop climbing even as `LiveSky` keeps calling `set_sky` with new per-tick vertex data.
    pub(crate) rebuilds: u64,
    /// Sky mesh vertex/index GPU buffers actually (re)created (new or capacity-grown) since the renderer started,
    /// for leak regressions: converges once the sky's largest mesh has been seen and its material set is stable.
    pub(crate) buffer_creates: u64,
}

struct CloudAnim {
    layer: ao_scene::cloud::CloudLayer,
    sim: ao_scene::cloud::Clouds,
    vertices: Vec<ao_scene::Vertex>,
    indices: [Vec<u32>; 2],
    grid: usize,
    time: f32,
}

// Authored FXS objects survive preceding layers becoming invisible; mesh slots do not.
fn cloud_object_key(scene: &Scene, mesh: usize) -> (bool, usize) {
    scene.sky_pose_objects.iter().find(|&&(slot, _)| scene.sky.get(slot).is_some_and(|instance| instance.mesh == mesh))
        .map_or((false, mesh), |&(_, object)| (true, object))
}

/// A sky mesh whose vertex colours are re-simulated every frame ([`ao_scene::SkyColors`]).
struct ColorAnim {
    mesh: usize,
    sim: ao_scene::aurora::GloomySky,
    gain: f32,
    vertices: Vec<ao_scene::Vertex>,
}

/// Material uniform of a submesh (see `shader.wgsl` `Mat`).
fn mat_uniform(s: &ao_scene::Submesh) -> [f32; 32] {
    let [wu, au, wv, av] = s.uv_wave;
    let [fr, fg, fb] = s.sky_fog_color.unwrap_or([0.0; 3]);
    let fog_mode = if !s.sky_fog { 0.0 } else if s.sky_fog_color.is_some() { 2.0 } else { 1.0 };
    let native_flags = u32::from(s.alpha_test.is_some() || s.sprite_alpha_test)
        | (u32::from(!s.fog_enabled) << 1) | (u32::from(!s.lighting_enabled) << 2)
        // Authored SRCCOLOR/ONE effects use fs_blend, not the additive entry.
        | (u32::from(s.blend == Blend::SourceColorOne && !s.liquid && !s.sky_points && !s.sky_fog) << 3)
        | (u32::from(s.liquid) << 4) | (u32::from(s.overlay) << 5) | (u32::from(s.vegetation) << 6);
    [s.base_color[0], s.base_color[1], s.base_color[2], s.base_color[3], s.emissive[0], s.emissive[1], s.emissive[2], if s.prelit { 2.0 } else { s.glow_mask as u32 as f32 }, s.uv_scroll[0], s.uv_scroll[1], fog_mode, s.sun_flicker as u32 as f32, wu, au, wv, av, s.specular[0], s.specular[1], s.specular[2], s.shininess, s.texture_clamp as u32 as f32, fr, fg, fb, s.uv_camera[0], s.uv_camera[1], s.uv_camera[2], s.uv_camera[3],
        s.stage1_env_texture.is_some() as u32 as f32, f32::from(s.alpha_test.unwrap_or(30)) / 255.0, native_flags as f32, f32::from(s.alpha_func)]
}

/// Last-frame counters (after frustum culling).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub instances: usize,
    pub draw_calls: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CloudPrecisionProbe {
    Grouped,
    NativeDraws,
    #[default]
    BeautySky,
}

/// Colour + depth attachments sized to the output.
pub struct Targets {
    /// `None` when MSAA is disabled (`Renderer::msaa_samples` == 1, the retail default): the colour pass writes straight into the
    /// resolve view instead of a separate multisampled target (audit item 2 / MSAA-option decision: no unnecessary resolve target).
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    /// Backed depth/MSAA is required only for the isolated native sky pass.
    backed: bool,
    pub size: (u32, u32),
    /// MSAA sample count. State/Offscreen also use `matches_renderer` to rebuild
    /// when native sky backing requirements change after scene/settings updates.
    pub sample_count: u32,
}

impl Targets {
    pub fn new(r: &Renderer, w: u32, h: u32) -> Self {
        let samples = r.msaa_samples();
        let backed = r.isolate_sky() && !r.enhanced_settings.any_enabled();
        let mk = |format, label| {
            r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | if backed { wgpu::TextureUsages::empty() } else { wgpu::TextureUsages::TRANSIENT },
                view_formats: &[],
            }).create_view_tracked(&Default::default())
        };
        Self { msaa: (samples > 1).then(|| mk(r.format, "msaa")), depth: mk(DEPTH, "depth"), backed, size: (w, h), sample_count: samples }
    }

    pub(crate) fn matches_renderer(&self, r: &Renderer) -> bool {
        self.sample_count == r.msaa_samples() && self.backed == (r.isolate_sky() && !r.enhanced_settings.any_enabled())
    }
}

/// Opt-in CPU wall time for the last render, in milliseconds (not GPU execution time).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameCpuTimings {
    pub prepare_ms: f64,
    pub encode_ms: f64,
    pub finish_ms: f64,
    pub queue_submit_ms: f64,
    pub shadow_prepare_encode_ms: f64,
    pub grass_prepare_ms: f64,
    pub grass_encode_ms: f64,
    pub ao_ms: f64,
    pub bloom_ms: f64,
}

pub struct Renderer {
    grass: Option<grass::Grass>,
    foliage: foliage::Foliage,
    foliage_detail_state: foliage_detail::State,
    canopy_leaves: foliage_leaves::Leaves,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    enhanced_settings: EnhancedSettings,
    enhanced_frame: Option<enhanced::EnhancedFrame>,
    bloom: Option<bloom::Bloom>,
    grade: Option<grade::Grade>,
    temporal: Option<temporal::Temporal>,
    temporal_programs: Option<temporal::TemporalPrograms>,
    post_target: Option<temporal::PostTarget>,
    spatial_post: Option<spatial_post::SpatialPost>,
    motion_vectors: Option<actors::motion_vectors::MotionVectors>,
    motion_programs: Option<std::sync::Arc<actors::motion_vectors::Programs>>,
    post_focus: Option<[f32; 3]>,
    temporal_room: Option<usize>,
    post_previous_view: Option<(Mat4, Vec3)>,
    post_fog_color: Option<[f32;3]>,
    enhanced_pipelines: enhanced::EnhancedPipelines,
    bloom_pipelines: bloom::BloomPipelines,
    grade_pipelines: grade::GradePipelines,
    shadow_pipelines: std::sync::Arc<sun_shadows::Compiled>,
    point_program: point_shadows::Compiled,
    ambient_occlusion: Option<ambient_occlusion::AmbientOcclusion>,
    contact_shadows: Option<contact_shadows::ContactShadows>,
    atmosphere: Option<atmosphere::Atmosphere>,
    volumetric_clouds: Option<volumetric_clouds::VolumetricClouds>,
    sky_discs: Option<sky_discs::SkyDiscs>,
    atmosphere_layers: Vec<volumetric_clouds::CloudDensityLayer>,
    atmosphere_base_height: f32,
    atmosphere_native_layers: Vec<ao_scene::cloud::CloudLayer>,
    sky_disc_transforms: Vec<[[f32; 4]; 4]>,
    sky_disc_sources: sky_discs::DiscSources,
    gpu_timer: Option<FrameGpuTimer>,
    cpu_timings: Option<FrameCpuTimings>,
    pub stats: FrameStats,
    sun_shadows: Option<sun_shadows::SunShadows>,
    point_shadows: Option<point_shadows::PointShadows>,
    shadow_sun: Option<[f32; 3]>,
    shadow_moon: Option<ao_scene::MoonSource>,
    moon_exposure: moon_exposure::MoonExposure,
    shadow_timer: Option<FrameGpuTimer>,
    shadow_timer_pending: bool,
    /// Seconds driving `Submesh::uv_scroll` (the viewer advances it; screenshots leave it at 0).
    pub time: f32,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    g_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    holo_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    world: variant::Pipes,
    world_variants: Vec<(variant::Key, variant::Pipes)>,
    world_key: variant::Key,
    pending_world: Option<variant::Pending>,
    world_compiles: std::sync::Arc<std::sync::atomic::AtomicU64>,
    world_frame_compiles: std::sync::Arc<std::sync::atomic::AtomicU64>,
    sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    /// Texture anisotropic filtering clamp ([`Renderer::set_anisotropy`]; audit item 2, default 1x = retail `FUN_10041ede`,
    /// `docs/screens.md` "max anisotropy 1"). The aomac Enhanced options page may raise it.
    anisotropy: u16,
    /// Unclamped preference, retained while auxiliary attachments temporarily limit MSAA.
    requested_msaa: u32,
    /// Sample counts the adapter actually supports for both the surface colour format and [`DEPTH`] (always includes 1).
    msaa_supported: Vec<u32>,
    auxiliary_msaa: wgpu::TextureFormatFeatureFlags,
    water_cloud_msaa: wgpu::TextureFormatFeatureFlags,
    texture_policy: native_texture::Policy,
    drawn_visuals: HashMap<VisualKey, i32>,
    pending_visuals: std::cell::RefCell<HashMap<VisualKey, i32>>,
    actor_visual_retirements: Vec<(u32, u32)>,
    gpu: Gpu,
    liquids: Vec<native_liquid::LiquidRun>,
    water: water::Water,
    heat_haze: heat_haze::HeatHaze,
    realistic_water: bool,
    liquid_edge: [f32; 3],
    water_light: Option<([f32; 3], [f32; 3])>,
    sky: SkyGpu,
    cloud_preferences: (bool, bool, bool),
    sky_beauty_only: bool,
    #[cfg(test)]
    cloud_precision_probe: CloudPrecisionProbe,
    /// Sky textures by key; kept across [`Renderer::set_sky`] so a live sky only ships new textures.
    sky_views: HashMap<TextureKey, wgpu::TextureView>,
    /// The untextured-submesh placeholder view, lazily created once and reused (`set_sky`'s fast path runs about
    /// twice a second for as long as a zone is outdoors; a fresh 1x1 texture every tick was a small but real
    /// leftover per-tick GPU object churn after the rest of the sky rebuild was fixed).
    sky_white_view: std::sync::OnceLock<wgpu::TextureView>,
    env: Environment,
    /// Lens of the loaded scene (`Scene::lens`).
    lens: ao_scene::Lens,
    /// Camera dependent fog (statel fog volumes), see `Scene::fog_model`.
    fog: Option<ao_scene::FogModel>,
    /// Multiplier of the accumulated fog density, see [`ao_scene::fog_mode_density_scale`] (`FogMode` pref).
    fog_density_scale: f32,
    /// Statel distance LOD state, see `Scene::statel_lod`.
    lod: Option<LodState>,
    ground: Option<GroundRun>,
    ground_quality: (i32, i32),
    ground_state: native_ground::NativeGround,
    /// Light storage buffer plus its pristine contents and the statel zone of each light (distance gating). `cells`/`idx`
    /// are the light-grid index buffers; all three are kept so [`Renderer::set_anisotropy`] can rebuild `globals_bg` with
    /// the current lighting data instead of recomputing the light grid just to change a sampler.
    light_buf: wgpu::Buffer,
    light_cells_buf: wgpu::Buffer,
    light_idx_buf: wgpu::Buffer,
    light_base: Vec<[f32; 4]>,
    light_zones: Vec<Option<u32>>,
    static_owners: Vec<Option<u32>>,
    cluster_frame: clustered_lighting::Frame,
    cluster_programs: clustered_lighting::Programs,
    dynamic_owners: Vec<Option<u32>>,
    dynamic_packed: Vec<[f32; 4]>,
    /// Per-frame moving lights (worn/attached item lights such as the Light Bar), see [`Renderer::set_dynamic_lights`]:
    /// a small fixed-capacity storage buffer rewritten every frame, separate from the static `light_buf`/grid so an
    /// actor's own light moving every frame never rebuilds the whole scene's light grid.
    dyn_light_buf: wgpu::Buffer,
    dyn_light_count: i32,
    /// Traffic ships of the uploaded scene, see `Scene::movers`.
    movers: Vec<MoverRun>,
    spaceship_slots: Vec<bool>,
    spaceships_shown: bool,
    far_away: Vec<FarAwayRun>,
    /// `Scene::day_time` of the uploaded scene.
    day_time: f32,
    /// Game seconds per [`Renderer::time`] second for the traffic ships (1 = the game clock; the viewer sets the live
    /// sky's scale).
    pub day_time_rate: f32,
    // per-frame scratch
    frame: usize,
    sky_dot_counter: f32,
    vis: Vec<[[f32; 4]; 4]>,
    vis_src: Vec<u32>,
    vis_range: Vec<std::ops::Range<u32>>,
    sorted: Vec<(i32, i32, u8, usize, u32)>, // (native list, distance bucket, source list, draw, visible instance)
    /// Dynamic actors drawn after the world, see `actors.rs`.
    act: actors::ActorLayer,
    screen: screen::ScreenPass,
}

/// Inactive combinations retained for switching back without recompiling.
const WORLD_VARIANT_CACHE: usize = 3;

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

/// GC100b7273/100b749f feed the active camera planes to VisualFog::AddClipPlanes.
/// Reapply every frame: live sky snapshots can replace the model between lens updates.
fn apply_camera_fog(env: &mut Environment, model: &mut ao_scene::FogModel, lens: ao_scene::Lens, far: f32, eye: [f32; 3], density_scale: f32) {
    model.near = lens.near;
    model.far = far;
    let (color, end) = model.at_scaled(eye, density_scale);
    if env.sky_color == env.fog_color {
        env.sky_color = color;
    }
    (env.fog_color, env.fog_start, env.fog_end) = (color, lens.near, end);
}

/// Left, right, bottom, top, near, far planes (normalised, normals point inside).
/// Builds every opaque/blend/sky/env/ground/liquid pipeline variant at a given colour+depth sample count. wgpu bakes
/// `MultisampleState` into the pipeline (no live toggle), so [`Renderer::set_msaa_samples`] calls this again at the new count;
/// `Renderer::new` calls it once at the default (1 = MSAA off, matching retail: the MSAA-option decision found no AA option
/// anywhere in the client's GUI/text/twk data).
/// The world/material repeat sampler at a given anisotropic filtering clamp (`Renderer::new` default 1 = retail;
/// [`Renderer::set_anisotropy`] rebuilds it, along with [`make_clamp_sampler`]'s clamp-address variant).
fn make_world_sampler(device: &wgpu::Device, anisotropy: u16) -> wgpu::Sampler {
    let descriptor = wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: anisotropy,
        ..Default::default()
    };
    device.create_sampler_tracked(&descriptor)
}

/// Clamp-to-edge counterpart of [`make_world_sampler`], same filtering/anisotropy. Both samplers live in the shared
/// `globals_bg` bind group (group 0); `shader.wgsl`'s `sample_clamped` picks between them per material, so
/// [`Renderer::set_anisotropy`] only has to rebuild `globals_bg`, not every material's bind group.
fn make_clamp_sampler(device: &wgpu::Device, anisotropy: u16) -> wgpu::Sampler {
    let descriptor = wgpu::SamplerDescriptor {
        label: Some("native D3D texture clamp"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: anisotropy,
        ..Default::default()
    };
    device.create_sampler_tracked(&descriptor)
}

/// Ambient alpha tags the depth owner, independently of material RGB blending.
fn ambient_target(state: Option<wgpu::BlendState>, depth_write: bool) -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format: enhanced::AUX,
        blend: state.map(|state| wgpu::BlendState { alpha: wgpu::BlendComponent::REPLACE, ..state }),
        write_mask: if depth_write { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::COLOR },
    }
}

fn compile_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    layouts: (&wgpu::PipelineLayout, &wgpu::PipelineLayout),
    settings: EnhancedSettings,
    msaa: u32,
    spec: PipelineSpec,
) -> wgpu::RenderPipeline {
    let (layout, holo_pipeline_layout) = layouts;
    let pipelines::Descriptor { fs, vs, state, depth_write, compare, actor, holo, points, cull, bias } = spec.descriptor();
    let f4 = |o| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: o, shader_location: 4 + (o / 16) as u32 };
    let inst_attrs = [f4(0), f4(16), f4(32), f4(48)];
    let actor_attrs = [f4(0), f4(16), f4(32), f4(48), f4(64), f4(80), f4(96), f4(112),
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 128, shader_location: 13 }];
    let vert_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
        let target = wgpu::ColorTargetState { format, blend: state, write_mask: wgpu::ColorWrites::ALL };
        let auxiliary = wgpu::ColorTargetState { format: enhanced::AUX, blend: state, write_mask: wgpu::ColorWrites::ALL };
        let sun_mask = settings.needs_sun_mask().then(|| auxiliary.clone());
        let water_cloud = settings.cloud_shadows.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend: state, write_mask: wgpu::ColorWrites::RED });
        let targets = [Some(target), settings.ambient_occlusion.then(|| ambient_target(state, depth_write)), settings.emissive_bloom.then_some(auxiliary), sun_mask, water_cloud];
        pipeline_creation::render(device, &wgpu::RenderPipelineDescriptor {
            label: Some(fs),
            // Ground effects 8-11 sample `ground_palette` (holo0) at bind group 2; liquids (12/13) use `fs_blend`,
            // which never samples group 2, so they must stay on the 2-group layout (no draw path for `self.gpu.liquid`
            // binds a group 2 - `DisplaySystem_t::Render`'s liquid list carries no palette texture).
            layout: Some(if holo { holo_pipeline_layout } else { layout }),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vert_attrs },
                    wgpu::VertexBufferLayout {
                        array_stride: if actor { std::mem::size_of::<actors::ActorInstance>() as u64 } else { 64 },
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: if actor { &actor_attrs } else { &inst_attrs },
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &targets[..enhanced::target_count(&settings)],
            }),
            primitive: wgpu::PrimitiveState {
                topology: if points { wgpu::PrimitiveTopology::PointList } else { wgpu::PrimitiveTopology::TriangleList },
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: cull.then_some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                // Only the native mesh-effect mode 7 authors a depth bias.
                bias: if bias { wgpu::DepthBiasState { constant: -1, slope_scale: 0.0, clamp: 0.0 } } else { Default::default() },
            }),
            // Audit item 4 (aomac Enhanced, opt-in): only the hard-cutout alpha-test shaders (`shade`/`shade_sky`'s
            // `mode == 1u`, no blend state of their own) benefit; blended/additive pipelines already have real alpha
            // blending. Off at MSAA 1 (retail-faithful default): there is nothing for per-sample coverage to smooth.
            multisample: wgpu::MultisampleState { count: msaa, mask: !0, alpha_to_coverage_enabled: msaa > 1 && matches!(fs, "fs_test" | "fs_sky_test") },
            multiview_mask: None,
            cache: None,
        })
}

fn build_pipelines() -> (Pipelines, Pipelines, Pipelines) {
    let mk = |blend, two_sided, sky, env, fade, sprite, actor, flip_depth, effect, compare| {
        PipelineSlot::new(PipelineSpec { blend, two_sided, sky, env, fade, sprite, actor, flip_depth, effect, compare })
    };
    let pipe_specs = [Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive]
        .into_iter()
        .flat_map(|b| [false, true].map(|two| (b, two, false, false)))
        .chain([Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive].map(|b| (b, true, true, false)))
        .chain([false, true, false, true].map(|two| (Blend::Opaque, two, false, true)))
        .map(|(b, two, sky, env)| (b, two, sky, env, false))
        .chain([Blend::Opaque, Blend::AlphaTest].into_iter().flat_map(|b| [false, true].map(move |two| (b, two, false, false, true))))
        .map(|(b, two, sky, env, fade)| (b, two, sky, env, fade, false))
        .chain([false, true].map(|two| (Blend::AlphaBlend, two, false, false, false, true)))
        .chain([Blend::ZeroSourceColor, Blend::DestinationColorSourceColor, Blend::PremultipliedAlpha, Blend::SourceColorOne].into_iter().flat_map(|b| [false, true].map(move |two| (b, two, false, false, false, false))))
        // Keep the original 31 entries as the actor table's stable prefix.
        .chain(std::iter::once((Blend::SourceColorOne, true, true, false, false, false)))
        .chain([Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive, Blend::ZeroSourceColor, Blend::DestinationColorSourceColor, Blend::PremultipliedAlpha, Blend::SourceColorOne]
            .map(|b| (b, false, true, false, false, false)))
        .chain([Blend::ZeroSourceColor, Blend::DestinationColorSourceColor, Blend::PremultipliedAlpha]
            .map(|b| (b, true, true, false, false, false)));
    assert_eq!(pipe_specs.clone().count(), SKY_POINT_PIPE, "point list follows triangle sky pipelines");
    let blends = [Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive,
        Blend::ZeroSourceColor, Blend::DestinationColorSourceColor, Blend::PremultipliedAlpha,
        Blend::SourceColorOne, Blend::AdditiveOne];
    let depth_specs = blends.into_iter().flat_map(|b| [false, true].map(move |two| (b, two, false, false, false, false)));
    let comparison_specs = [wgpu::CompareFunction::Less, wgpu::CompareFunction::LessEqual, wgpu::CompareFunction::Always].into_iter()
        .flat_map(|compare| [false, true].into_iter().flat_map(move |write| blends.into_iter()
            .flat_map(move |blend| [false, true].map(move |two|
                ((blend, two, false, false, false, false), write != matches!(blend, Blend::Opaque | Blend::AlphaTest), Some(compare))))));
    let mut pipes: Vec<_> = pipe_specs.clone().map(|(b, two, sky, env, fade, sprite)| mk(b, two, sky, env, fade, sprite, false, false, 0, None))
        .chain(std::iter::once(mk(Blend::Additive, true, true, false, false, false, false, false, 14, None)))
        .chain([12, 13, 15, 16, 17].into_iter().map(|effect| mk(Blend::AlphaBlend, true, false, false, false, false, false, false, effect, None)))
        .chain(depth_specs.clone().map(|(b, two, sky, env, fade, sprite)| mk(b, two, sky, env, fade, sprite, false, true, 0, None)))
        .collect();
    assert_eq!(pipes.len(), WORLD_COMPARE_PIPE);
    for ((b, two, sky, env, fade, sprite), flip, compare) in comparison_specs.clone() {
        pipes.push(mk(b, two, sky, env, fade, sprite, false, flip, 0, compare));
    }
    pipes.extend([false, true].map(|two| mk(Blend::AdditiveOne, two, false, false, false, false, false, false, 0, None)));
    assert_eq!(pipes.len(), SKY_STATE_PIPE);
    for compare in [wgpu::CompareFunction::Less, wgpu::CompareFunction::LessEqual, wgpu::CompareFunction::Always] {
        for write in [false, true] {
            for b in blends {
                for two in [false, true] {
                    pipes.push(mk(b, two, true, false, false, false, false, write, 0, Some(compare)));
                }
            }
        }
    }
    let comparison_fade_specs = [wgpu::CompareFunction::Less, wgpu::CompareFunction::LessEqual, wgpu::CompareFunction::Always].into_iter()
        .flat_map(|compare| [false, true].into_iter().flat_map(move |write| [Blend::Opaque, Blend::AlphaTest].into_iter()
            .flat_map(move |blend| [false, true].map(move |two|
                ((blend, two, false, false, true, false), !write, Some(compare))))));
    let actor_specs = pipe_specs.take(MATERIAL_STATE_PIPE).map(|spec| (spec, false, None))
        .chain(depth_specs.map(|spec| (spec, true, None)))
        .chain([Blend::Opaque, Blend::AlphaTest].into_iter().flat_map(|b| [false, true].map(move |two| ((b, two, false, false, true, false), true, None))))
        .chain([false, true, false, true].map(|two| ((Blend::Opaque, two, false, true, false, false), true, None)))
        .chain([false, true].map(|two| ((Blend::AdditiveOne, two, false, false, false, false), false, None)))
        .chain(comparison_specs).chain(comparison_fade_specs)
        .chain([wgpu::CompareFunction::Less, wgpu::CompareFunction::Always].into_iter()
            .flat_map(|compare| [false, true].into_iter().flat_map(move |_blended|
                [false, true].into_iter().flat_map(move |write| [false, true].map(move |two|
                    ((Blend::Opaque, two, false, true, false, false), write, Some(compare)))))));
    assert_eq!(actor_specs.clone().count(), MESH_EFFECT_STRIDE);
    let mut actor_pipes = Vec::with_capacity(8 * MESH_EFFECT_STRIDE);
    for effect in 0..=7 {
        for (index, ((b, two, sky, env, fade, sprite), flip, compare)) in actor_specs.clone().enumerate() {
            let env_alias = if (ENV_BLEND_PIPE..ENV_BLEND_PIPE + 2).contains(&index)
                || (MATERIAL_ENV_WRITE_PIPE + 2..MATERIAL_ENV_WRITE_PIPE + 4).contains(&index) {
                Some(index - 2)
            } else if index >= MATERIAL_ENV_COMPARE_PIPE && (index - MATERIAL_ENV_COMPARE_PIPE) % 8 >= 4 {
                Some(index - 4)
            } else { None };
            actor_pipes.push(if effect == 3 { PipelineSlot::alias(index) }
                else if let Some(source) = env_alias { PipelineSlot::alias(effect as usize * MESH_EFFECT_STRIDE + source) }
                else { mk(b, two, sky, env, fade, sprite, true, flip, effect, compare) });
        }
    }
    let ground_pipes: Vec<_> = (8..=11).map(|effect| mk(Blend::Opaque, false, false, false, false, false, false, false, effect, None)).collect();
    (pipes.into(), actor_pipes.into(), ground_pipes.into())
}

/// Left/right/bottom/top/near/far planes of `vp` (view-projection), normalized, `>= 0` inside. Public so an app can
/// build a conservative pre-skin visibility test (perf audit item 5) using the same math as the renderer's own
/// per-mesh cull (see [`crate::frustum_culls_sphere`]).
pub fn frustum_planes(vp: &Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (vp.row(0), vp.row(1), vp.row(2), vp.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| p / p.truncate().length())
}

impl Renderer {
    /// Picks a compatible Metal adapter and queues pipeline warmup without waiting for unused states.
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
                required_features: (adapter.features() & (wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES | wgpu::Features::TIMESTAMP_QUERY))
                    | if adapter.get_info().backend == wgpu::Backend::Metal { wgpu::Features::PASSTHROUGH_SHADERS } else { wgpu::Features::empty() },
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
                *caps.formats.iter().find(|f| !f.is_srgb()).expect("surface requires a gamma-space UNORM format")
            }
            None => wgpu::TextureFormat::Rgba8Unorm,
        };
        // Both the colour target (`format`) and [`DEPTH`] must support a sample count for `Targets` to create matching
        // attachments at it; `set_msaa_samples` clamps requests to this intersection (always includes 1).
        let msaa_supported: Vec<u32> = {
            let color = adapter.get_texture_format_features(format).flags.supported_sample_counts();
            let depth = adapter.get_texture_format_features(DEPTH).flags.supported_sample_counts();
            color.into_iter().filter(|c| depth.contains(c)).collect()
        };
        let auxiliary_msaa = adapter.get_texture_format_features(enhanced::AUX).flags;
        let water_cloud_msaa = adapter.get_texture_format_features(wgpu::TextureFormat::R16Float).flags;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl"))),
        });
        let globals = device.create_buffer_tracked(&wgpu::BufferDescriptor {
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
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let g_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                storage_entry(1),
                storage_entry(2),
                storage_entry(3),
                storage_entry(4),
                // Both material samplers live here (not per-material): `Renderer::set_anisotropy` rebuilds only this one
                // bind group, so every existing world/actor/sky/ground material picks up the change without a bind-group
                // rebuild (`shader.wgsl`'s `sample_clamped` picks repeat vs clamp per material from `Mat::flags.x`).
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let sampler = make_world_sampler(&device, 1);
        let clamp_sampler = make_clamp_sampler(&device, 1);
        let (light_buf, light_cells_buf, light_idx_buf) = light_storage_buffers(&device, &LightGrid::new(&[]));
        let dyn_light_buf = make_dyn_light_buffer(&device);
        let globals_bg = globals_bind(&device, &g_layout, &globals, &light_buf, &light_cells_buf, &light_idx_buf, &dyn_light_buf, &sampler, &clamp_sampler);
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
                uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
            ],
        });
        let holo_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native Holo textures"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            }),
        });
        let compiles = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let frame_compiles = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let world_key = variant::Key::of(EnhancedSettings::default(), 1);
        let world = variant::Request::new(
            &device, &shader, format, (&g_layout, &tex_layout, &holo_layout),
            world_key, EnhancedSettings::default(),
            &compiles, &frame_compiles).prepare();
        let enhanced_pipelines = enhanced::EnhancedPipelines::new(&device, format);
        let bloom_pipelines = bloom::BloomPipelines::new(&device);
        let grade_pipelines = grade::GradePipelines::new(&device, format, enhanced_pipelines.source_layout());
        let shadow_pipelines = std::sync::Arc::new(sun_shadows::Compiled::new(&device, &g_layout, &tex_layout));
        let point_program = point_shadows::Compiled::new(&device);
        let ambient_occlusion = Some(ambient_occlusion::AmbientOcclusion::new(&device, format));
        let contact_shadows = Some(contact_shadows::ContactShadows::new(&device, format));
        let cluster_programs = clustered_lighting::Programs::new(&device);
        let atmosphere = Some(atmosphere::Atmosphere::new(&device, format));
        let volumetric_clouds = Some(volumetric_clouds::VolumetricClouds::new(&device, format, &msaa_supported));
        let sky_discs = Some(sky_discs::SkyDiscs::new(&device, format));
        let mut r = Self {
            device,
            queue,
            format,
            enhanced_settings: EnhancedSettings::default(),
            enhanced_frame: None,
            bloom: None,
            grade: None,
            temporal: None,
            temporal_programs: None,
            post_target: None,
            spatial_post: None,
            motion_vectors: None,
            motion_programs: None,
            post_focus: None,
            temporal_room: None,
            post_previous_view: None,
            post_fog_color: None,
            enhanced_pipelines,
            bloom_pipelines,
            grade_pipelines,
            shadow_pipelines,
            point_program,
            ambient_occlusion,
            contact_shadows,
            atmosphere,
            volumetric_clouds,
            sky_discs,
            atmosphere_layers: Vec::new(),
            atmosphere_base_height: 0.0,
            atmosphere_native_layers: Vec::new(),
            sky_disc_transforms: Vec::new(),
            sky_disc_sources: sky_discs::DiscSources::default(),
            gpu_timer: None,
            cpu_timings: None,
            grass: None,
            foliage: foliage::Foliage::default(),
            foliage_detail_state: foliage_detail::State::default(),
            canopy_leaves: foliage_leaves::Leaves::default(),
            stats: FrameStats::default(),
            time: 0.0,
            movers: vec![],
            spaceship_slots: vec![],
            spaceships_shown: true,
            liquids: vec![],
            water: water::Water::default(),
            drawn_visuals: HashMap::new(),
            pending_visuals: Default::default(),
            actor_visual_retirements: Vec::new(),
            heat_haze: Default::default(),
            sun_shadows: None,
            point_shadows: None,
            shadow_sun: None,
            shadow_moon: None,
            moon_exposure: Default::default(),
            shadow_timer: None,
            shadow_timer_pending: false,
            realistic_water: true,
            liquid_edge: [0.0; 3],
            water_light: None,
            far_away: vec![],
            day_time: 0.0,
            day_time_rate: 1.0,
            globals,
            globals_bg,
            g_layout,
            tex_layout,
            holo_layout,
            shader,
            world,
            world_variants: Vec::new(),
            world_key,
            pending_world: None,
            world_compiles: compiles,
            world_frame_compiles: frame_compiles,
            sampler,
            clamp_sampler,
            anisotropy: 1,
            requested_msaa: 1,
            msaa_supported,
            auxiliary_msaa,
            water_cloud_msaa,
            texture_policy: Default::default(),
            sky: SkyGpu::default(),
            cloud_preferences: (true, true, false),
            sky_beauty_only: false,
            #[cfg(test)]
            cloud_precision_probe: CloudPrecisionProbe::default(),
            sky_views: HashMap::new(),
            sky_white_view: std::sync::OnceLock::new(),
            gpu: Gpu { meshes: vec![], inst_bufs: vec![], mats: vec![], opaque: vec![], blended: vec![], liquid: vec![], insts: vec![], mesh_range: vec![], radius: 100.0, grid: ([0.0; 4], [0; 4]) },
            env: default_environment(100.0),
            lens: ao_scene::Lens::default(),
            fog: None,
            fog_density_scale: 1.0,
            lod: None,
            ground: None,
            ground_quality: (200, 44),
            ground_state: Default::default(),
            light_buf,
            light_cells_buf,
            light_idx_buf,
            light_base: vec![],
            light_zones: vec![],
            static_owners: vec![],
            cluster_frame: Default::default(),
            cluster_programs,
            dynamic_owners: vec![],
            dynamic_packed: vec![],
            dyn_light_buf,
            dyn_light_count: 0,
            frame: 0,
            sky_dot_counter: 0.0,
            vis: vec![],
            vis_src: vec![],
            vis_range: vec![],
            sorted: vec![],
            act: Default::default(),
            screen: Default::default(),
        };
        r.act.native_metal = adapter.get_info().backend == wgpu::Backend::Metal;
        r.warm_skin_pipeline();
        r.screen.warm_pipelines(&r.device, &r.queue, r.format);
        r.heat_haze.warm_pipeline(&r.device, r.format);
        r.upload(&Scene::default());
        Ok(r)
    }

    /// Whether the active startup pipeline set has finished compiling in the background.
    pub fn pipeline_warmup_complete(&self) -> bool {
        self.world.is_ready()
            && self.enhanced_pipelines.is_ready()
            && self.bloom_pipelines.is_ready()
            && self.grade_pipelines.is_ready()
            && self.shadow_pipelines.is_ready()
            && self.ambient_occlusion.as_ref().is_some_and(|ao| ao.is_ready())
            && self.contact_shadows.as_ref().is_some_and(|contact| contact.is_ready())
            && self.atmosphere.as_ref().is_some_and(|pass| pass.is_ready())
            && self.volumetric_clouds.as_ref().is_some_and(|pass| pass.is_ready())
            && self.sky_discs.as_ref().is_some_and(|pass| pass.is_ready())
            && self.screen.is_ready()
            && self.heat_haze.is_ready()
            && self.water.is_ready()
            && self.skin_pipeline_is_ready()
            && self.temporal_programs.as_ref().is_none_or(temporal::TemporalPrograms::is_ready)
            && self.motion_programs.as_ref().is_none_or(|programs| programs.is_ready())
            && self.spatial_post.as_ref().is_none_or(spatial_post::SpatialPost::is_ready)
    }

    /// Explicit readiness barrier for zone preparation and deterministic evidence, never a frame prerequisite.
    pub fn wait_for_pipeline_warmup(&self) {
        self.world.wait();
        self.enhanced_pipelines.wait();
        self.bloom_pipelines.wait();
        self.grade_pipelines.wait();
        self.shadow_pipelines.wait();
        self.ambient_occlusion.as_ref().expect("startup AO pipelines").wait();
        self.contact_shadows.as_ref().expect("startup contact pipelines").wait();
        self.atmosphere.as_ref().expect("startup atmosphere pipelines").wait();
        self.volumetric_clouds.as_ref().expect("startup cloud pipelines").wait();
        self.sky_discs.as_ref().expect("startup sky disc pipelines").wait();
        self.screen.wait();
        self.water.wait();
        self.heat_haze.wait();
        self.canopy_leaves.wait();
        self.wait_skin_pipeline();
        if let Some(temporal) = &self.temporal_programs { temporal.wait(); }
        if let Some(motion) = &self.motion_programs { motion.wait(); }
        if let Some(spatial) = &self.spatial_post { spatial.wait(); }
    }

    /// Native quality is latched at startup; compression callbacks affect future loads. `better` is the aomac
    /// Enhanced "Better Mip Filtering" option (not retail, default off, audit item 12).
    pub fn set_texture_policy(&mut self, quality: i32, compressed: bool, better: bool) {
        self.texture_policy = native_texture::Policy { quality, compressed, better };
    }

    fn texture_view(&self, rgba: &[u8], w: u32, h: u32, source: ao_scene::TextureSource) -> wgpu::TextureView {
        let bc_supported = self.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        // Sky/ground single-texture uploads are never alpha-tested foliage: coverage preservation does not apply.
        let built = native_texture::build_texture_mips(self.texture_policy, source, bc_supported, rgba, w, h, false);
        self.upload_built_texture(&built)
    }

    /// Issues the actual GPU texture creation + `queue.write_texture` calls for a CPU-built mip chain
    /// (see [`native_texture::build_texture_mips`]). Kept separate so a batch of textures can be built
    /// in parallel on worker threads while every wgpu call still happens serially on the caller's thread.
    fn upload_built_texture(&self, built: &native_texture::BuiltTexture) -> wgpu::TextureView {
        let block = built.encoding.block_edge();
        let block_bytes = built.encoding.block_bytes();
        let tex = self.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: built.w, height: built.h, depth_or_array_layers: 1 },
            mip_level_count: built.mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: built.encoding.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, lvl) in built.levels.iter().enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &lvl.bytes,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lvl.cw.div_ceil(block) * block_bytes), rows_per_image: Some(lvl.ch.div_ceil(block)) },
                wgpu::Extent3d { width: lvl.cw.div_ceil(block) * block, height: lvl.ch.div_ceil(block) * block, depth_or_array_layers: 1 },
            );
        }
        let descriptor = wgpu::TextureViewDescriptor::default();
        tex.create_view_tracked(&descriptor)
    }
    /// Anisotropic filtering clamp (audit item 2: retail runs max anisotropy 1, randy31 `FUN_10041ede`, `docs/screens.md`
    /// "max anisotropy 1"; `Renderer::new` already defaults to it). The aomac Enhanced options page may raise it up to 16x.
    /// Rebuilds `globals_bg` (the shared bind group holding both samplers), which every already-uploaded world, actor, sky
    /// and ground material picks up immediately (`shader.wgsl`'s `sample_clamped`) without rebuilding their own bind
    /// groups or waiting for the next zone load.
    pub fn set_anisotropy(&mut self, level: u16) {
        let level = level.clamp(1, 16);
        if level == self.anisotropy { return; }
        self.anisotropy = level;
        self.sampler = make_world_sampler(&self.device, level);
        self.clamp_sampler = make_clamp_sampler(&self.device, level);
        self.globals_bg = globals_bind(&self.device, &self.g_layout, &self.globals, &self.light_buf, &self.light_cells_buf, &self.light_idx_buf, &self.dyn_light_buf, &self.sampler, &self.clamp_sampler);
    }

    /// Changes MSAA synchronously, except that an in-flight live switch stays asynchronous.
    /// Retained targets must be rebuilt when [`Renderer::msaa_samples`] changes.
    pub fn set_msaa_samples(&mut self, requested: u32) {
        self.requested_msaa = requested.max(1);
        let (settings, defer) = self.pending_world.as_ref()
            .map_or((self.enhanced_settings, false), |pending| (pending.settings, true));
        let key = variant::Key::of(settings, self.clamp_msaa(self.requested_msaa, settings));
        self.request_world(key, settings, defer);
    }

    pub fn msaa_samples(&self) -> u32 { self.world.msaa }

    fn clamp_msaa(&self, requested: u32, settings: EnhancedSettings) -> u32 {
        let auxiliary = settings.ambient_occlusion || settings.emissive_bloom || settings.needs_sun_mask();
        if auxiliary {
            self.msaa_supported.iter().copied().filter(|&count| count <= requested
                && self.auxiliary_msaa.sample_count_supported(count)
                && (!settings.cloud_shadows || self.water_cloud_msaa.sample_count_supported(count))).max().unwrap_or(1)
        } else {
            clamp_msaa_samples(requested.max(1), &self.msaa_supported)
        }
    }

    /// Applies one frontend frame's options together, without compiling an intermediate MSAA variant.
    pub(crate) fn set_world_options(&mut self, samples: Option<u32>, settings: Option<EnhancedSettings>, switch: Option<EnhancedSettings>) {
        if let Some(samples) = samples { self.requested_msaa = samples.max(1); }
        if let Some(settings) = switch {
            self.request_enhanced_settings(settings);
        } else if let Some(settings) = settings {
            self.set_enhanced_settings(settings);
        } else if samples.is_some() {
            let settings = self.pending_world.as_ref().map_or(self.enhanced_settings, |pending| pending.settings);
            let key = variant::Key::of(settings, self.clamp_msaa(self.requested_msaa, settings));
            self.request_world(key, settings, true);
        }
    }

    /// Immediate fixture setting: waits for an unseen world combination to compile on a worker.
    pub fn set_enhanced_settings(&mut self, settings: EnhancedSettings) {
        let settings = settings.clamped();
        let key = variant::Key::of(settings, self.clamp_msaa(self.requested_msaa, settings));
        self.request_world(key, settings, false);
    }

    /// Keeps drawing the active variant while an unseen combination compiles on a worker.
    pub fn request_enhanced_settings(&mut self, settings: EnhancedSettings) {
        let settings = settings.clamped();
        let key = variant::Key::of(settings, self.clamp_msaa(self.requested_msaa, settings));
        self.request_world(key, settings, true);
    }

    fn request_world(&mut self, key: variant::Key, settings: EnhancedSettings, defer: bool) {
        // A shader-equivalent switch (e.g. HDR -> HDR + cloud shadows) still
        // needs both attachment shapes warmed before water can enter the view.
        let companion = key.with_water_cloud(!key.water_cloud);
        let water_pair_cached = !settings.cloud_shadows || self.world_key == companion
            || self.world_variants.iter().any(|(cached, _)| *cached == companion);
        if key == self.world_key && water_pair_cached {
            self.pending_world = None;
            if self.enhanced_settings != settings { self.apply_enhanced_settings(settings); }
            return;
        }
        if let Some(index) = self.world_variants.iter().position(|(cached, _)| water_pair_cached && *cached == key) {
            let world = self.world_variants.remove(index).1;
            self.install_world(key, (world, None), settings);
            return;
        }
        if let Some(pending) = self.pending_world.as_mut().filter(|pending| pending.key == key && (!settings.cloud_shadows || pending.settings.cloud_shadows)) {
            pending.settings = settings;
            if !defer { self.await_enhanced_world(); }
            return;
        }
        let request = variant::Request::new(
            &self.device, &self.shader, self.format, (&self.g_layout, &self.tex_layout, &self.holo_layout),
            key, settings,
            &self.world_compiles, &self.world_frame_compiles);
        if defer {
            self.pending_world = Some(request.spawn(key, settings));
        } else {
            let world = request.spawn(key, settings).wait();
            self.install_world(key, world, settings);
        }
    }

    fn install_world(&mut self, key: variant::Key, (world, companion): variant::Built, settings: EnhancedSettings) {
        self.pending_world = None;
        let previous = std::mem::replace(&mut self.world, world);
        let previous_key = std::mem::replace(&mut self.world_key, key);
        if previous.msaa != self.world.msaa {
            self.release_effect_targets();
            self.enhanced_frame = None;
            self.bloom = None;
            self.grade = None;
        }
        if let Some((companion_key, companion)) = companion {
            self.world_variants.retain(|(cached, _)| *cached != companion_key);
            self.world_variants.push((companion_key, companion));
        }
        self.world_variants.push((previous_key, previous));
        while self.world_variants.len() > WORLD_VARIANT_CACHE { self.world_variants.remove(0); }
        self.apply_enhanced_settings(settings);
    }

    fn apply_enhanced_settings(&mut self, settings: EnhancedSettings) {
        let previous = self.enhanced_settings;
        let changed_frame = previous.any_enabled() != settings.any_enabled()
            || previous.ambient_occlusion != settings.ambient_occlusion
            || previous.emissive_bloom != settings.emissive_bloom
            || previous.needs_sun_mask() != settings.needs_sun_mask();
        self.enhanced_settings = settings;
        self.moon_exposure.sync(&self.device, settings.night_mode == NightMode::Moonlit);
        if previous.night_mode != settings.night_mode {
            self.sun_shadows = None;
            self.point_shadows = None;
        }
        if previous.taa != settings.taa || previous.motion_blur != settings.motion_blur {
            self.reset_post_history();
        }
        if settings.taa && self.temporal_programs.is_none() {
            self.temporal_programs = Some(temporal::TemporalPrograms::new(&self.device, self.format));
        } else if !settings.taa {
            self.temporal = None;
            self.temporal_programs = None;
        }
        let spatial_enabled = settings.fxaa && !settings.taa || settings.cas || settings.depth_of_field || settings.motion_blur > 0.0;
        if spatial_enabled && self.spatial_post.is_none() {
            self.spatial_post = Some(spatial_post::SpatialPost::new(&self.device,self.format,self.enhanced_pipelines.source_layout()));
        } else if !spatial_enabled { self.spatial_post = None; }
        if !settings.post_enabled() { self.post_target = None; }
        if settings.taa || settings.motion_blur > 0.0 {
            self.warm_motion_vectors();
        } else {
            self.motion_vectors = None;
            self.motion_programs = None;
        }
        if previous.needs_admitted_lights() && !settings.needs_admitted_lights() {
            self.cluster_frame = Default::default();
            self.dyn_light_buf = make_dyn_light_buffer(&self.device);
            self.dyn_light_count = self.dyn_light_count.min(MAX_DYN_LIGHTS as i32);
            let count = (self.dyn_light_count as usize * 4).min(self.dynamic_packed.len());
            if count > 0 { self.queue.write_buffer(&self.dyn_light_buf, 0, bytemuck::cast_slice(&self.dynamic_packed[..count])); }
            self.globals_bg = globals_bind(&self.device, &self.g_layout, &self.globals, &self.light_buf, &self.light_cells_buf, &self.light_idx_buf, &self.dyn_light_buf, &self.sampler, &self.clamp_sampler);
        }
        self.configure_water();
        if !settings.ground_grass || settings.grass_density <= 0.0 { self.grass = None; }
        if !settings.ambient_occlusion {
            self.ambient_occlusion.as_mut().expect("startup AO pipelines").release_targets();
        }
        if !settings.contact_shadows {
            self.contact_shadows.as_mut().expect("startup contact pipelines").release_targets();
        }
        if !settings.sun_shadows && !settings.moon_shadows && !settings.volumetric_shafts && !settings.needs_admitted_lights()
            && settings.night_mode != NightMode::Moonlit { self.sun_shadows = None; }
        if !settings.needs_admitted_lights() { self.point_shadows = None; }
        self.release_disabled_atmosphere(settings);
        if changed_frame {
            self.reset_post_history();
            self.release_effect_targets();
            self.enhanced_frame = None; self.bloom = None; self.grade = None;
        }
    }

    fn release_effect_targets(&mut self) {
        self.ambient_occlusion.as_mut().expect("startup AO pipelines").release_targets();
        self.contact_shadows.as_mut().expect("startup contact pipelines").release_targets();
        self.release_atmosphere_targets();
    }

    fn await_enhanced_world(&mut self) {
        let Some(mut pending) = self.pending_world.take() else { return };
        let world = pending.wait();
        self.install_world(pending.key, world, pending.settings);
    }

    /// Call before checking targets; never install a variant partway through rendering a frame.
    pub fn settle_enhanced_world(&mut self) {
        let Some(pending) = self.pending_world.as_mut() else { return };
        let Some(world) = pending.poll() else { return };
        let pending = self.pending_world.take().expect("pending variant");
        self.install_world(pending.key, world, pending.settings);
    }

    pub fn world_pipeline_compiles(&self) -> u64 { self.world_compiles.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn world_frame_thread_compiles(&self) -> u64 { self.world_frame_compiles.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn world_variant_cache_len(&self) -> usize { self.world_variants.len() }
    /// All instrumented pipeline creations inside this thread's render scopes.
    pub fn frame_thread_compiles(&self) -> u64 { pipeline_creation::frame_thread_compiles() }
    /// Unready pipeline cells encountered inside this thread's render scopes.
    pub fn frame_pipeline_waits(&self) -> u64 { pipeline_creation::frame_pipeline_waits() }

    pub fn enhanced_settings(&self) -> EnhancedSettings { self.enhanced_settings }
    /// Logical optional attachment bytes, not driver allocation/alignment or total GPU residency.
    pub fn enhanced_target_bytes(&self) -> u64 {
        self.enhanced_frame.as_ref().map_or(0, |frame| {
            let pixels = u64::from(frame.size.0) * u64::from(frame.size.1);
            let colour = 4 + 8 * (u64::from(frame.ambient.is_some()) + u64::from(frame.emissive.is_some()) + u64::from(frame.sun.is_some())) + 2 * u64::from(frame.water_cloud.is_some());
            let samples = u64::from(frame.sample_count);
            pixels * (colour + 4 * samples + if samples > 1 { colour * samples + 4 } else { 0 })
        }) + self.ambient_occlusion.as_ref().map_or(0, ambient_occlusion::AmbientOcclusion::target_bytes)
            + self.contact_shadows.as_ref().map_or(0, contact_shadows::ContactShadows::target_bytes)
            + self.bloom.as_ref().map_or(0, |bloom| bloom.bytes)
            + self.grade.as_ref().map_or(0, grade::Grade::bytes)
            + self.atmosphere_target_bytes()
            + self.temporal.as_ref().map_or(0, temporal::Temporal::bytes)
            + self.motion_vectors.as_ref().map_or(0, actors::motion_vectors::MotionVectors::bytes)
            + self.post_target.as_ref().map_or(0, |target| u64::from(target.size.0) * u64::from(target.size.1) * 4)
            + self.spatial_post.as_ref().map_or(0, spatial_post::SpatialPost::bytes)
            + self.water.target_bytes()
    }
    /// Explicit offline instrumentation; disabled by default and owns no resources then.
    /// After each render, read the result before encoding another timed frame.
    pub fn set_gpu_timing(&mut self, enabled: bool) -> Result<()> {
        if enabled && self.shadow_timer.is_some() { return Err(anyhow!("frame and shadow GPU timing must be sampled separately")); }
        self.gpu_timer = if enabled { Some(FrameGpuTimer::new(&self.device, &self.queue)?) } else { None };
        Ok(())
    }
    pub fn read_frame_gpu_ms(&mut self) -> Result<Option<f64>> {
        self.gpu_timer.as_mut().map(|timer| timer.read_ms(&self.device, &self.queue)).transpose()
    }

    /// No clock reads or timing allocations while disabled (the default).
    pub fn set_cpu_timing(&mut self, enabled: bool) {
        self.cpu_timings = enabled.then(FrameCpuTimings::default);
    }
    pub fn frame_cpu_timings(&self) -> Option<FrameCpuTimings> { self.cpu_timings }

    // The native P8 shadow palette is filtered in framebuffer space. It is
    // neither an sRGB tile nor subject to RGB565/BC resource preferences.
    fn ground_palette_view(&self, t: &ao_scene::Texture) -> wgpu::TextureView {
        let size = wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("native ground palette"), size, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
        });
        self.queue.write_texture(texture.as_image_copy(), &t.rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(t.width * 4), rows_per_image: Some(t.height) }, size);
        texture.create_view_tracked(&Default::default())
    }


    /// Replaces the GPU-side scene.
    pub fn upload(&mut self, scene: &Scene) {
        self.reset_post_history();
        self.foliage_detail_state = foliage_detail::State::default();
        self.sky_dot_counter = 0.0;
        self.sky.directional_epoch = self.time;
        self.water_light = scene.water_light;
        self.moon_exposure = moon_exposure::MoonExposure::from_scene(scene);
        self.moon_exposure.sync(&self.device, self.enhanced_settings.night_mode == NightMode::Moonlit);
        self.sun_shadows = None;
        self.point_shadows = None;
        self.sky.clouds.clear();
        self.release_atmosphere_targets();
        self.atmosphere_base_height = scene.ground_detail.iter()
            .flat_map(|patch| patch.heights.iter().map(move |height| f32::from(*height) * patch.height_scale / 256.0))
            .filter(|height| height.is_finite()).reduce(f32::min).unwrap_or(0.0);
        self.ground_state.recreate();
        let bc_supported = self.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let policy = self.texture_policy;
        let composites = native_ground::composites(scene);
        let composite_keys: std::collections::HashSet<_> = composites.iter().map(|recipe| recipe.key).collect();
        // aomac Enhanced "Better Mip Filtering" coverage preservation only applies to textures actually used by a
        // `Blend::AlphaTest` submesh (foliage, fences, hair cards); every other texture keeps the plain box filter.
        let alpha_tested: std::collections::HashSet<TextureKey> = scene.meshes.iter()
            .flat_map(|m| &m.submeshes)
            .filter(|s| s.blend == ao_scene::Blend::AlphaTest)
            .filter_map(|s| s.texture)
            .collect();
        enum Job<'a> {
            Ground(&'a ao_scene::Texture),
            Mip { rgba: &'a [u8], w: u32, h: u32, source: ao_scene::TextureSource, alpha_test: bool },
        }
        const DEFAULT_WHITE: [u8; 4] = [255; 4];
        let mut jobs: Vec<Job> = vec![Job::Mip { rgba: &DEFAULT_WHITE, w: 1, h: 1, source: ao_scene::TextureSource::Generated, alpha_test: false }];
        let mut view_of: HashMap<TextureKey, usize> = HashMap::new();
        for (k, t) in &scene.textures {
            if composite_keys.contains(k) { continue; }
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize {
                view_of.insert(*k, jobs.len());
                jobs.push(if k.rdb_type == 1_000_007 { Job::Ground(t) } else { Job::Mip { rgba: &t.rgba, w: t.width, h: t.height, source: t.source, alpha_test: alpha_tested.contains(k) } });
            }
        }
        // CPU-side box-filter mip chain + BC/packed encoding runs across threads (audit item 10, the
        // expensive part with BC compression on); GPU texture creation and `queue.write_texture` stay
        // serial on the calling thread below, since wgpu's Queue isn't safe to interleave across
        // threads. Each texture's encoded bytes depend only on its own pixels, never on thread
        // scheduling, so output is byte-identical to the old fully-serial path.
        let mut built: Vec<Option<native_texture::BuiltTexture>> = Vec::with_capacity(jobs.len());
        built.resize_with(jobs.len(), || None);
        let threads = std::thread::available_parallelism().map(std::num::NonZero::get).unwrap_or(1).min(jobs.len().max(1));
        let chunk = jobs.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (job_chunk, built_chunk) in jobs.chunks(chunk).zip(built.chunks_mut(chunk)) {
                scope.spawn(move || {
                    for (job, slot) in job_chunk.iter().zip(built_chunk.iter_mut()) {
                        if let Job::Mip { rgba, w, h, source, alpha_test } = job {
                            *slot = Some(native_texture::build_texture_mips(policy, *source, bc_supported, rgba, *w, *h, *alpha_test));
                        }
                    }
                });
            }
        });
        let mut views: Vec<wgpu::TextureView> = jobs.iter().zip(built.iter()).map(|(job, b)| match job {
            Job::Ground(t) => self.ground_palette_view(t),
            Job::Mip { .. } => self.upload_built_texture(b.as_ref().expect("built in parallel above")),
        }).collect();
        // Reuse the already-built actual tile mip bytes, including low-quality
        // packed conversion, instead of reducing the decoded source a second time.
        let tile_mips: HashMap<_, _> = view_of.iter().filter_map(|(key, &index)|
            built[index].as_ref()?.ground_tile_mip().map(|mip| (*key, mip))).collect();
        for (key, texture) in native_ground::composite_images(&composites, &scene.textures, &tile_mips) {
            view_of.insert(key, views.len());
            views.push(self.texture_view(&texture.rgba, texture.width, texture.height, texture.source));
        }
        // One bind group per distinct (texture, base colour, emissive, glow mask).
        let mut mat_of: HashMap<(usize, usize, bool, [u32; 32]), usize> = HashMap::new();
        let mut mats: Vec<wgpu::BindGroup> = vec![];
        let mut material = |dev: &Renderer, view: usize, s: &ao_scene::Submesh, ground_patch: Option<&ao_scene::ground_detail::GroundPatch>, mesh: &ao_scene::Mesh| {
            let u = ground_patch.map_or_else(|| foliage::material_bounds(mat_uniform(s), s, mesh), |p| native_ground::material_uniform(s, p));
            let env_view = s.stage1_env_texture.and_then(|key| view_of.get(&key).copied()).unwrap_or(0);
            *mat_of.entry((view, env_view, s.texture_clamp, u.map(f32::to_bits))).or_insert_with(|| {
                let ub = dev.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::bytes_of(&u),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                mats.push(dev.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &dev.tex_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[view]) },
                        wgpu::BindGroupEntry { binding: 1, resource: ub.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[env_view]) },
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
        let mut ground_materials: Vec<Vec<usize>> = scene.meshes.iter().map(|m| vec![usize::MAX; m.submeshes.len()]).collect();
        let mut ground_texture_materials = HashMap::new();
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
            let mut indices = Vec::with_capacity(idx_total);
            for (si, s) in mesh.submeshes.iter().enumerate() {
                let first = indices.len() as u32;
                let nv = mesh.vertices.len() as u32;
                append_primitive_indices(&mut indices, s, nv);
                let count = indices.len() as u32 - first;
                let vegetation = s.vegetation && !s.liquid;
                if vegetation {
                    self.foliage_detail_state.bake(mi, first, &mesh.vertices, &indices[first as usize..(first + count) as usize]);
                }
                if count > 0 {
                    let view = s.texture.and_then(|k| view_of.get(&k).copied()).unwrap_or(0);
                    let mat = material(self, view, s, scene.ground_detail.iter().find(|p| p.mesh == mi), mesh);
                    ground_materials[mi][si] = mat;
                    if let (Some(patch), Some(key)) = (scene.ground_detail.iter().find(|p| p.mesh == mi), s.texture) {
                        let uniform = native_ground::material_uniform(s, patch);
                        ground_texture_materials.insert(mat, (key, uniform));
                    }
                    let d = Draw { mesh: mi, first_index: first, count, mat, pipe: world_material_pipe(s),
                        priority: s.render_priority, node: s.render_node, origin: s.render_origin, projection_pass: s.projection_pass,
                        vegetation };
                    if s.liquid {
                        liquid.push(d)
                    } else if blended_material(s.blend) {
                        blended.push(d)
                    } else {
                        opaque.push(d)
                    }
                }
            }
            let vb = self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let ib = self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            });
            meshes.push(Some((vb, ib)));
        }
        opaque.sort_by_key(|d| (d.pipe, d.mesh, d.mat));
        let inst_bufs = if insts.is_empty() {
            vec![]
        } else {
            (0..INST_RING)
                .map(|_| {
                    self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
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
        self.spaceship_slots = vec![false; insts.len()];
        for &instance in &scene.spaceships {
            if let Some(&slot) = slots.get(instance).filter(|&&slot| slot != usize::MAX) {
                self.spaceship_slots[slot] = true;
            }
        }
        self.lod = scene.statel_lod.clone().map(|lod| LodState { levels: vec![u8::MAX; lod.zones.len()], reduced: vec![false; lod.items.len()], lod, slot: slots.clone() });
        let rate = self.day_time_rate;
        self.day_time = scene.day_time;
        self.movers = scene
            .movers
            .iter()
            .filter_map(|m| {
                let slot = *slots.get(m.instance).filter(|&&s| s != usize::MAX)?;
                let local_center = sphere[scene.instances[m.instance].mesh].0;
                Some(MoverRun { mover: m.clone(), state: ao_scene::MoverState::settled(m, scene.day_time, rate), sound_time: self.time, sound_trigger: 0.0, sounds: [None; 3], slot, local_center })
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
        (self.light_buf, self.light_cells_buf, self.light_idx_buf) = light_storage_buffers(&self.device, &grid);
        self.dyn_light_buf = make_dyn_light_buffer(&self.device);
        self.dyn_light_count = 0;
        self.dynamic_packed.clear();
        self.dynamic_owners.clear();
        self.cluster_frame = Default::default();
        self.globals_bg = globals_bind(&self.device, &self.g_layout, &self.globals, &self.light_buf, &self.light_cells_buf, &self.light_idx_buf, &self.dyn_light_buf, &self.sampler, &self.clamp_sampler);
        self.light_zones = grid.zones.clone();
        self.static_owners = grid.owners;
        self.light_base = grid.lights.clone();
        let grid = ([grid.origin.x, grid.origin.y, grid.origin.z, grid.cell], [grid.dims[0], grid.dims[1], grid.dims[2], 0]);
        // Keep VisualWater's style order (water, acid, lava, star, shallow), including both coplanar passes.
        self.gpu = Gpu { meshes, inst_bufs, mats, opaque, blended, liquid, insts, mesh_range, radius, grid };
        self.foliage = foliage::Foliage::default();
        self.canopy_leaves.upload(&self.device, scene, &self.gpu);
        self.drawn_visuals.retain(|key, _| matches!(key, VisualKey::Actor(..)));
        self.liquids = scene.liquids.iter().cloned().map(native_liquid::LiquidRun::new).collect();
        self.water.reset();
        self.water.set_static_lights(&scene.lights);
        self.set_camera_underwater(None);
        self.configure_water();
        self.heat_haze.reset();
        // A new scene's ground has its own green masks: Enhanced grass is rebuilt on its first frame.
        self.grass = None;
        if let Some(shadows) = &mut self.sun_shadows { shadows.grass_receiver = None; }
        self.ground = (!scene.ground_detail.is_empty()).then(|| {
            let indices = scene.ground_detail.iter().map(|p| scene.meshes[p.mesh].submeshes.iter().map(|s| Vec::with_capacity(s.indices.len())).collect()).collect();
            let draws = scene.ground_detail.iter().map(|p| ground_materials[p.mesh].iter().map(|&mat| self.gpu.opaque.iter().position(|d| d.mesh == p.mesh && d.mat == mat).unwrap_or(usize::MAX)).collect()).collect();
            let capacity = scene.ground_detail.iter().map(|p| scene.meshes[p.mesh].submeshes.iter().map(|s| s.indices.len()).sum::<usize>()).max().unwrap_or(0);
            let mut materials: Vec<_> = (0..scene.meshes.len()).map(|_| None).collect();
            let mut has_shadow = vec![false; scene.meshes.len()];
            for p in &scene.ground_detail {
                has_shadow[p.mesh] = p.shadow_texture.is_some();
                let view = &views[p.shadow_texture.and_then(|k| view_of.get(&k).copied()).unwrap_or(0)];
                let size = wgpu::Extent3d { width: p.cells as u32 + 1, height: p.cells as u32 + 1, depth_or_array_layers: 1 };
                let sky = self.device.create_texture_tracked(&wgpu::TextureDescriptor {
                    label: Some("ground baked sky openness"), size, mip_level_count: 1, sample_count: 1,
                    dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
                });
                self.queue.write_texture(sky.as_image_copy(), &native_ground::sky_openness(p, &scene.meshes[p.mesh].vertices),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width), rows_per_image: Some(size.height) }, size);
                let sky_view = sky.create_view_tracked(&Default::default());
                materials[p.mesh] = Some(self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                    label: Some("native ground shadow palette"), layout: &self.holo_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&sky_view) },
                    ],
                }));
            }
            GroundRun { detail: ao_scene::ground_detail::GroundDetail::new(scene.ground_detail.clone()), indices, draws, scratch: Vec::with_capacity(capacity), materials, has_shadow, texture_materials: ground_texture_materials.into_iter().map(|(mat,(key,u))| (mat,key,u)).collect(), composites, green: grass::green_masks(scene), sky_open: grass::baked_sky_open(scene) }
        });
    }

    /// Replaces only the sky backdrop (`scene.sky` instances of `scene.meshes`) and, when given, the environment (sun, ambient,
    /// fog base, volumes kept); the world stays. Textures are cached by key, so `scene.textures` need only hold keys not sent before.
    ///
    /// Fast path: when every sky mesh's vertex count and submeshes (`actors::same_material`, ignoring `indices`) match
    /// the last upload, vertex/index data is written into the existing GPU buffers in place (growing them, capacity-
    /// doubled, only when a frame needs more room) and materials are looked up by content in a persisted cache, reusing
    /// their existing uniform buffer and bind group -- no buffer or bind group is created. `LiveSky` calls this about
    /// twice a second for the whole time a zone is outdoors; recreating every GPU object on every tick (the previous
    /// behaviour) was the dominant confirmed GPU-object leak (measured live at ICC with `AOMAC_GPU_STATS=1`: a steady,
    /// never-zero per-second churn of sky buffers/bind groups, independent of actor/combat/movement). A changed sky
    /// mesh/material set (zone or weather change) still falls through to a full rebuild.
    pub fn set_sky(&mut self, scene: &Scene) {
        self.water_light = scene.water_light;
        self.update_shadow_sun(scene);
        self.sky_disc_sources = sky_discs::DiscSources::from_scene(scene);
        for (k, t) in &scene.textures {
            if t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize && !self.sky_views.contains_key(k) {
                let v = self.texture_view(&t.rgba, t.width, t.height, t.source);
                self.sky_views.insert(*k, v);
            }
        }
        self.sky_white_view.get_or_init(|| self.texture_view(&[255; 4], 1, 1, ao_scene::TextureSource::Generated));
        let is_live = |mi: usize, mesh: &ao_scene::Mesh| {
            !mesh.vertices.is_empty() && mesh.submeshes.iter().map(|s| s.indices.len()).sum::<usize>() > 0 && scene.sky.iter().any(|s| s.mesh == mi)
        };
        let eligible = self.sky.topology.len() == scene.meshes.len()
            && self.sky.topology.iter().zip(scene.meshes.iter().enumerate()).zip(&self.sky.meshes).all(|(((nv, subs), (mi, mesh)), gpu)| {
                *nv == mesh.vertices.len()
                    && subs.len() == mesh.submeshes.len()
                    && subs.iter().zip(&mesh.submeshes).all(|(a, b)| actors::same_material(a, b))
                    && gpu.is_some() == is_live(mi, mesh)
            });
        if !eligible {
            self.sky.rebuilds += 1;
            self.sky.meshes = vec![None; scene.meshes.len()];
            self.sky.vertex_capacity = vec![0; scene.meshes.len()];
            self.sky.index_capacity = vec![0; scene.meshes.len()];
            self.sky.mat_of.clear();
            self.sky.mats.clear();
        }
        let mut topology = Vec::with_capacity(scene.meshes.len());
        let per_mesh: Vec<Vec<Draw>> = scene.meshes.iter().enumerate().map(|(mi, mesh)| {
            topology.push((mesh.vertices.len(), mesh.submeshes.clone()));
            if !is_live(mi, mesh) {
                self.sky.meshes[mi] = None;
                return Vec::new();
            }
            let (old_vb, old_ib) = self.sky.meshes[mi].take().map_or((None, None), |(a, b)| (Some(a), Some(b)));
            let (vb, created) = ensure_buf(&self.device, old_vb, &mut self.sky.vertex_capacity[mi], (mesh.vertices.len() * 48) as u64, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, "sky vertices");
            self.sky.buffer_creates += created as u64;
            self.queue.write_buffer(&vb, 0, bytemuck::cast_slice(&mesh.vertices));
            let nv = mesh.vertices.len() as u32;
            let idx_total: usize = mesh.submeshes.iter().map(|s| s.indices.len()).sum();
            let (mut indices, mut draws) = (Vec::with_capacity(idx_total), vec![]);
            for s in &mesh.submeshes {
                let first = indices.len() as u32;
                append_primitive_indices(&mut indices, s, nv);
                let count = indices.len() as u32 - first;
                if count == 0 {
                    continue;
                }
                let key = s.texture.filter(|k| self.sky_views.contains_key(k));
                let env_key = s.stage1_env_texture.filter(|k| self.sky_views.contains_key(k));
                let u = mat_uniform(s);
                let mat = *self.sky.mat_of.entry((key, env_key, s.texture_clamp, u.map(f32::to_bits))).or_insert_with(|| {
                    let ub = self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&u), usage: wgpu::BufferUsages::UNIFORM });
                    self.sky.mats.push(self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &self.tex_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(key.map_or(self.sky_white_view.get().unwrap(), |k| &self.sky_views[&k])) },
                            wgpu::BindGroupEntry { binding: 1, resource: ub.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(env_key.map_or(self.sky_white_view.get().unwrap(), |k| &self.sky_views[&k])) },
                        ],
                    }));
                    self.sky.mats.len() - 1
                });
                let pipe = sky_material_pipe(s);
                draws.push(Draw { mesh: mi, first_index: first, count, mat, pipe,
                    priority: s.render_priority, node: None, origin: s.render_origin, projection_pass: s.projection_pass,
                    vegetation: false });
            }
            let ibytes = (indices.len() * 4) as u64;
            let (ib, created) = ensure_buf(&self.device, old_ib, &mut self.sky.index_capacity[mi], ibytes, wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, "sky indices");
            self.sky.buffer_creates += created as u64;
            self.queue.write_buffer(&ib, 0, bytemuck::cast_slice(&indices));
            self.sky.meshes[mi] = Some((vb, ib));
            draws
        }).collect();
        self.sky.topology = topology;
        // one draw per (instance, submesh), in instance order
        let mut draws = vec![];
        let mut xf = vec![];
        for inst in &scene.sky {
            for d in per_mesh.get(inst.mesh).into_iter().flatten() {
                draws.push((*d, xf.len() as u32));
            }
            xf.push(Mat4::from_cols_array_2d(&inst.transform));
        }
        self.sky.draws = draws;
        self.sky.spin = (0..xf.len()).map(|i| scene.sky_spin.iter().find(|s| s.instance == i).copied()).collect();
        self.sky.wave = scene.sky_wave_spin.clone();
        if xf.len() != self.sky.xf.len() {
            self.sky.bufs = if xf.is_empty() {
                vec![]
            } else {
                (0..INST_RING)
                    .map(|_| self.device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("sky"), size: (xf.len() * 64) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }))
                    .collect()
            };
        }
        self.sky.xf = xf;
        // a live update keeps the running simulation of an identical aurora
        self.sky.colors = scene
            .sky_colors
            .iter()
            .filter(|c| matches!(self.sky.meshes.get(c.mesh), Some(Some(_))))
            .map(|c| ColorAnim {
                mesh: c.mesh,
                sim: self.sky.colors.iter().find(|o| o.sim.params == c.sim.params).map_or_else(|| c.sim.clone(), |o| o.sim.clone()),
                gain: c.gain,
                vertices: scene.meshes[c.mesh].vertices.clone(),
            })
            .collect();
        let mut old_clouds=std::mem::take(&mut self.sky.clouds);
        self.sky.clouds=scene.single_clouds.iter().map(|layer| {
            let mut run=old_clouds.iter().position(|old|cloud_object_key(&self.sky.pose,old.layer.mesh)==cloud_object_key(scene,layer.mesh))
                .map(|i|old_clouds.swap_remove(i)).unwrap_or_else(||CloudAnim {
                    layer:layer.clone(),sim:Default::default(),vertices:scene.meshes[layer.mesh].vertices.clone(),
                    indices:[ao_scene::cloud::Clouds::indices(2,0,30),ao_scene::cloud::Clouds::indices(10,0,30)],grid:0,time:self.time});
            run.layer=layer.clone();run.grid=0;run
        }).collect();
        self.sky.pose = Scene {
            day_time: scene.day_time,
            sky: scene.sky.clone(),
            sky_pose_objects: scene.sky_pose_objects.clone(),
            sky_yaw: scene.sky_yaw.clone(),
            sky_wave_spin: scene.sky_wave_spin.clone(),
            environment: scene.environment,
            ground_light: scene.ground_light,
            sun_authored: scene.sun_authored,
            daylight: scene.daylight,
            moons: scene.moons,
            directional_lights: scene.directional_lights.clone(),
            meshes: scene.meshes.iter().map(|mesh| ao_scene::Mesh {
                vertices: if mesh.submeshes.iter().any(|sub| sub.sun_flicker) { mesh.vertices.clone() } else { vec![] },
                ..Default::default()
            }).collect(),
            ..Scene::default()
        };
        self.sky.fan_alpha = self.sky.pose.sky.iter().filter_map(|instance| {
            self.sky.pose.meshes.get(instance.mesh)?.vertices.first().map(|vertex| (instance.mesh, vertex.color[3]))
        }).collect();
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

    /// Reuses the loaded sky buffers; the frame callback changes only matrices and the sun fans' byte alpha.
    fn apply_sky_pose(&mut self) {
        for (matrix, instance) in self.sky.xf.iter_mut().zip(&self.sky.pose.sky) {
            *matrix = Mat4::from_cols_array_2d(&instance.transform);
        }
        self.sky.wave.copy_from_slice(&self.sky.pose.sky_wave_spin);
        for (mesh, alpha) in &mut self.sky.fan_alpha {
            let data = &self.sky.pose.meshes[*mesh];
            let next = data.vertices[0].color[3];
            if next != *alpha {
                if let Some(Some((vertices, _))) = self.sky.meshes.get(*mesh) {
                    self.queue.write_buffer(vertices, 0, bytemuck::cast_slice(&data.vertices));
                }
                *alpha = next;
            }
        }
        if let Some(env) = self.sky.pose.environment { self.env.sun_dir = env.sun_dir; }
        self.shadow_sun = shadow_pass::authored_sun(&self.sky.pose);
        self.shadow_moon = moon_shadows::source(&self.sky.pose);
    }

    /// `set_sky` calls since the renderer started that fell through to a full rebuild, for leak regressions: in
    /// steady state (a stable sky mesh/material set) this must stop climbing even as `LiveSky` keeps calling
    /// `set_sky` with new per-tick vertex data.
    pub fn sky_rebuilds(&self) -> u64 { self.sky.rebuilds }

    /// Sky mesh vertex/index GPU buffers actually (re)created since the renderer started, for leak regressions:
    /// converges once the sky's largest mesh has been seen and its material set is stable.
    pub fn sky_buffer_creates(&self) -> u64 { self.sky.buffer_creates }

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

    /// `GAME.GameShowSpaceship`, DisplaySystem game state +0x189 (GC 0x100ad1ea).
    pub fn set_spaceships_shown(&mut self, shown: bool) {
        self.spaceships_shown = shown;
    }

    /// Native N3 ground quality/full-quality-radius preference consumers.
    pub fn set_ground_detail(&mut self, quality: i32, radius: i32) {
        self.ground_quality = (quality.clamp(100, 200), radius.clamp(7, 44));
    }

    /// Native texture-family invalidation (10037b9d): replace current ground
    /// bindings without rebuilding geometry, actors, or preference history.
    pub fn refresh_ground_textures(&mut self, textures: &HashMap<TextureKey, ao_scene::Texture>) {
        let Some(ground) = &self.ground else { return; };
        let same = |a: &TextureKey, b: &TextureKey| a == b || (a.id == b.id && matches!(a.rdb_type, 1_010_006 | 1_010_021 | 1_010_022) && matches!(b.rdb_type, 1_010_006 | 1_010_021 | 1_010_022));
        let composite_keys: std::collections::HashSet<_> = ground.composites.iter().map(|recipe| recipe.key).collect();
        let bc_supported = self.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let built: HashMap<_, _> = textures.iter().filter(|(key, t)| !composite_keys.contains(*key)
            && t.width > 0 && t.height > 0 && t.rgba.len() == (t.width * t.height * 4) as usize)
            .map(|(key, t)| (*key, native_texture::build_texture_mips(self.texture_policy, t.source, bc_supported, &t.rgba, t.width, t.height, false))).collect();
        let tile_mips: HashMap<_, _> = built.iter().filter_map(|(key, texture)| texture.ground_tile_mip().map(|mip| (*key, mip))).collect();
        let mut views: HashMap<_, _> = built.iter().filter(|(key, _)| ground.texture_materials.iter().any(|(_, k, _)| same(k, key)))
            .map(|(key, texture)| (*key, self.upload_built_texture(texture))).collect();
        for (key, texture) in native_ground::composite_images(&ground.composites, textures, &tile_mips) {
            views.insert(key, self.texture_view(&texture.rgba, texture.width, texture.height, texture.source));
        }
        for (mat,key,u) in &ground.texture_materials {
            let Some(view) = views.get(key).or_else(|| views.iter().find(|(k,_)| same(k,key)).map(|(_,v)| v)) else { continue; };
            let uniform = self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: Some("native ground material refresh"), contents: bytemuck::bytes_of(u), usage: wgpu::BufferUsages::UNIFORM,
            });
            self.gpu.mats[*mat] = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("native ground texture family refresh"), layout: &self.tex_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 1, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(view) },
                ],
            });
        }
    }

    /// Native callback preserves the shadows bit when mode zero is selected.
    pub fn set_ground_rendering(&mut self, mode: i32) { self.ground_state.set(mode); }
    pub fn ground_rendering_path(&self) -> GroundPath { self.ground_state.path() }

    fn update_ground_detail(&mut self, eye: Vec3) {
        let Some(ground) = &mut self.ground else { return; };
        if !ground.detail.update(eye.to_array(), self.ground_quality.0, self.ground_quality.1, &mut ground.indices) { return; }
        for (pi, patch) in ground.detail.patches().iter().enumerate() {
            ground.scratch.clear();
            for (si, list) in ground.indices[pi].iter().enumerate() {
                let draw = ground.draws[pi][si];
                if draw == usize::MAX { continue; }
                self.gpu.opaque[draw].first_index = ground.scratch.len() as u32;
                self.gpu.opaque[draw].count = list.len() as u32;
                ground.scratch.extend_from_slice(list);
            }
            if let Some((_, buffer)) = &self.gpu.meshes[patch.mesh] {
                if !ground.scratch.is_empty() { self.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&ground.scratch)); }
            }
        }
    }

    /// Replaces the lens of the loaded scene (a camera path that changes the field of view every frame, the player camera's `ViewDistance`).
    /// A far plane also moves the fog end and the statel LOD's view length: the client's `VisualFog_t::AddClipPlanes(near, far)` and
    /// `VisualCamera_t::GetLengthOfViewcone` (far - near) both come from the camera's planes (docs/chat/dvalue.md).
    pub fn set_lens(&mut self, lens: ao_scene::Lens) {
        if let Some(far) = lens.far {
            if let Some(f) = &mut self.fog {
                f.near = lens.near;
                f.far = far;
            }
            if let Some(l) = &mut self.lod {
                l.lod.view_length = far - lens.near;
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
                self.queue.write_buffer(vb, 0, bytemuck::cast_slice(&m.vertices));
            }
        }
    }

    /// One FXS frame for every traffic ship ([`ao_scene::Mover`]): the waypoint target of the game clock
    /// (`Scene::day_time + time * day_time_rate`) feeds the per-frame accumulators, the instance follows.
    fn step_movers(&mut self) {
        let day = self.day_time + self.time * self.day_time_rate;
        for r in &mut self.movers {
            let previous = r.state;
            r.state.step(&r.mover, r.mover.target(day));
            let dt = self.time - r.sound_time;
            r.sound_time = self.time;
            r.sounds = r.mover.sound.map(|sound| sound.frame(&previous, &r.state, dt, self.spaceships_shown, &mut r.sound_trigger)).unwrap_or([None; 3]);
            if let Some(g) = self.gpu.insts.get_mut(r.slot) {
                g.m = r.mover.transform(&r.state);
                g.center = Mat4::from_cols_array_2d(&g.m).transform_point3(r.local_center);
            }
        }
    }

    /// Drains the actual FXS-frame ship sound requests, before far-away visual placement.
    pub fn take_mover_sounds(&mut self) -> impl Iterator<Item = ao_scene::mover::SoundEvent> + '_ {
        self.movers.iter_mut().flat_map(|mover| mover.sounds.iter_mut().filter_map(Option::take))
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
            r.sound_time = time;
            r.sound_trigger = 0.0;
            r.sounds = [None; 3];
        }
        for a in &mut self.sky.colors {
            a.sim = ao_scene::aurora::GloomySky::new(a.sim.params.clone());
            a.sim.advance_to(time);
        }
    }

    /// Scene radius; viewer uses it for speed.
    pub fn radius(&self) -> f32 {
        self.gpu.radius
    }

    /// Per-frame moving point/spot lights (worn/attached item lights, e.g. the Light Bar's embedded `RLight_t`,
    /// `mesh.rs::decode_mesh_lights`, tracked to its attractor bone every frame): affects terrain, statics and actors
    /// through the native D3D7 formula. Faithful transport retains [`MAX_DYN_LIGHTS`];
    /// Enhanced lighting/shadows upload every candidate for GPU owner admission before selection. Call before [`Renderer::render`];
    /// an empty slice turns dynamic lighting off again (e.g. the item unequipped or its wearer out of view).
    pub fn set_dynamic_lights(&mut self, lights: &[ao_scene::Light]) {
        let n = if self.enhanced_settings.needs_admitted_lights() { lights.len() } else { lights.len().min(MAX_DYN_LIGHTS) };
        self.dyn_light_count = n.min(MAX_DYN_LIGHTS) as i32;
        self.dynamic_packed.clear();
        self.dynamic_packed.extend(lights[..n].iter().flat_map(pack_light));
        self.dynamic_owners.clear();
        self.dynamic_owners.extend(lights[..n].iter().map(|light| light.owner));
        let prefix = self.dyn_light_count as usize * 4;
        if prefix > 0 { self.queue.write_buffer(&self.dyn_light_buf, 0, bytemuck::cast_slice(&self.dynamic_packed[..prefix])); }
    }

    pub fn set_screen_effects(&mut self,layers:Vec<ScreenLayer>,fog_disabled:bool) {
        self.screen.layers=layers;
        self.screen.fog_disabled=fog_disabled;
    }
    pub fn upload_screen_texture(&mut self,key:TextureKey,texture:&ao_scene::Texture) {
        self.screen.upload(&self.device,&self.queue,key,texture,self.texture_policy);
    }
    pub fn set_viewport_depth_clears(&mut self,clears:Vec<ViewportDepthClear>) {self.screen.depth_clears=clears;}

    pub fn set_cloud_preferences(&mut self,realistic:bool,simple:bool,disabled_realistic:bool) {
        self.cloud_preferences=(realistic,simple,disabled_realistic);
    }

    fn isolate_sky(&self) -> bool {
        #[cfg(test)]
        if self.cloud_precision_probe != CloudPrecisionProbe::BeautySky { return false; }
        self.enhanced_settings.volumetric_clouds || (!self.sky.clouds.is_empty() && ao_scene::cloud::resolution(
            self.cloud_preferences.0, self.cloud_preferences.1, self.cloud_preferences.2) != 0)
    }

    /// Draws one frame into `resolve` (a view of the output texture).
    pub fn render(&mut self, resolve: &wgpu::TextureView, t: &Targets, cam: &Camera) -> wgpu::SubmissionIndex {
        let _compile_scope = pipeline_creation::FrameGuard::enter();
        let timed = self.cpu_timings.is_some();
        let prepare_start = timed.then(std::time::Instant::now);
        let mut cpu = FrameCpuTimings::default();
        self.update_liquids(cam.pos);
        self.update_ground_detail(cam.pos);
        self.update_lod(cam.pos.x, cam.pos.z);
        self.step_movers();
        self.pose_far_away(cam.pos);
        let mut env = self.env;
        // Select the projection before local fog evaluation: volumes must never
        // shorten the camera far plane or change normalized sky geometry.
        let far = self.lens.far.unwrap_or_else(|| (env.fog_end * 1.1).max(50.0));
        if self.screen.fog_disabled { env.fog_start=f32::MAX;env.fog_end=f32::MAX; }
        if let Some(model) = self.fog.as_mut().filter(|_| !self.screen.fog_disabled) {
            apply_camera_fog(&mut env, model, self.lens, far, cam.pos.to_array(), self.fog_density_scale);
        }
        let aspect = t.size.0 as f32 / t.size.1.max(1) as f32;
        let native_vp = Mat4::perspective_rh(self.lens.vertical_fov(aspect), aspect, self.lens.near, far) * Mat4::look_to_rh(cam.pos, cam.forward(), cam.up());
        let vp = self.prepare_post_frame(t.size, cam, native_vp, env.fog_color);
        let v4 = |c: [f32; 3], w| Vec4::new(c[0], c[1], c[2], w).to_array();
        let g4 = |c: [f32; 3], w| v4(c.map(gamma), w);
        let w = ao_scene::wave_curves(self.time);
        let (water_color, water_dir) = self.water_light.unwrap_or_default();
        self.sky_dot_counter = (self.sky_dot_counter + 0.01) % 1.0;
        let moon = self.enhanced_moonlight();
        let mut g = Globals {
            view_proj: vp.to_cols_array_2d(),
            eye: cam.pos.extend(far).to_array(),
            sun_dir: v4(Vec3::from(env.sun_dir).normalize_or_zero().to_array(), 0.0),
            sun_color: v4(env.sun_color, 0.0),
            ambient: v4(env.ambient, 0.0),
            fog_color: v4(env.fog_color, 1.0),
            fog: [env.fog_start, env.fog_end, self.time, if self.world.msaa > 1 { 1.0 } else { 0.0 }],
            grid: self.gpu.grid.0,
            dims: self.gpu.grid.1,
            wave: std::array::from_fn(|i| std::array::from_fn(|j| w[i * 4 + j])),
            sun_g: g4(env.sun_color, env.sun_specular),
            ambient_g: g4(env.ambient, grade::daylight_weight(self.sky.pose.daylight)),
            fog_g: g4(env.fog_color, 1.0),
            view: Mat4::look_to_rh(cam.pos, cam.forward(), cam.up()).to_cols_array_2d(),
            water_dir: v4(Vec3::from_array(water_dir).normalize_or_zero().to_array(), 0.0),
            water_g: g4(water_color, env.sun_specular),
            dyn_info: [self.dyn_light_count, self.sky.pose.directional_lights.len().min(7) as i32, 0, 0],
            sky_counter: [self.sky_dot_counter, 0.0, 0.0, 0.0],
            directional: std::array::from_fn(|lane| {
                self.sky.pose.directional_lights.get(lane / 2).map_or([0.0; 4], |light| {
                    if lane % 2 == 0 {
                        v4(Vec3::from_array(light.direction).normalize_or_zero().to_array(), 0.0)
                    } else {
                        v4(light.gamma_at(self.time - self.sky.directional_epoch), 0.0)
                    }
                })
            }),
            moon_direction: moon.map_or([0.0; 4], |(direction, _)| v4(direction, 1.0)),
            moon_g: v4(moon.map_or([0.0; 3], |(_, energy)| energy),
                if self.enhanced_settings.night_mode == NightMode::Dark {
                    self.sky.pose.daylight.filter(|day| day.is_finite()).map_or(0.0, |day|
                        self.enhanced_settings.dark_night_strength * (1.0 - day.clamp(0.0, 1.0)))
                } else { 0.0 }),
            foliage_wind: { let (phase, wind) = self.foliage.wind(); [wind[0], wind[1], wind[2], phase] },
            foliage_flags: [f32::from(u8::from(self.enhanced_settings.foliage_sway)), f32::from(u8::from(self.enhanced_settings.foliage_push)), 0.0, 0.0],
            foliage_pushers: [[0.0; 4]; 16],
        };

        // Frustum cull per instance (bounding spheres), compacting survivors per mesh.
        let planes = frustum_planes(&vp);
        self.vis.clear();
        self.vis_src.clear();
        self.vis_range.clear();
        for r in &self.gpu.mesh_range {
            let start = self.vis.len() as u32;
            for (i, inst) in self.gpu.insts[r.clone()].iter().enumerate() {
                let c = inst.center.extend(1.0);
                if !inst.hidden && (self.spaceships_shown || !self.spaceship_slots[r.start + i]) && planes.iter().all(|p| p.dot(c) >= -inst.radius) {
                    self.vis.push(inst.m);
                    self.vis_src.push((r.start + i) as u32);
                }
            }
            self.vis_range.push(start..self.vis.len() as u32);
        }
        self.frame = (self.frame + 1) % INST_RING;
        self.prepare_actors(cam.pos, cam.forward(), &planes);
        self.foliage.prepare(cam.pos, vp, self.act.enhanced_grass_actor_ids(env.fog_end.max(env.fog_start + 1e-3)), self.enhanced_settings.foliage_enabled());
        self.foliage_detail_state.prepare(&self.device, self.enhanced_settings.foliage_subdivision);
        g.foliage_pushers = self.foliage.pushers();
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        self.sorted.clear();
        for (list, draws) in [&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid].into_iter().enumerate() {
            for (di, d) in draws.iter().enumerate() {
                if d.count == 0 || self.ground.as_ref().is_some_and(|ground| ground.materials[d.mesh].is_some()) { continue; }
                for vi in self.vis_range[d.mesh].clone() {
                    let instance = self.vis_src[vi as usize] as usize;
                    let key = d.node.map(|(node, _)| VisualKey::World(instance, node));
                    let priority = visual_priority(d, key, &self.drawn_visuals).unwrap_or([3, 5, 4][list]);
                    if priority < 0 { continue; }
                    let matrix = Mat4::from_cols_array_2d(&self.gpu.insts[instance].m);
                    let bucket = actors::render_bucket(matrix.transform_point3(Vec3::from(d.origin)).distance(cam.pos));
                    self.sorted.push((priority, bucket, list as u8, di, vi));
                }
            }
        }
        self.sorted.sort_by(|a, b| actors::bucket_order(a.0, a.1, b.0, b.1)
            .then_with(|| b.4.cmp(&a.4)).then_with(|| a.3.cmp(&b.3)));
        let water_cloud = self.enhanced_settings.cloud_shadows && self.sorted.iter().any(|draw| draw.2 == 2);
        let key = variant::Key::of(self.enhanced_settings, self.world.msaa).with_water_cloud(water_cloud);
        if key != self.world_key {
            let pending = self.pending_world.take();
            self.request_world(key, self.enhanced_settings, false);
            self.pending_world = pending;
        }
        let grass_start = timed.then(std::time::Instant::now);
        self.prepare_enhanced_grass(cam, vp, env.fog_end.max(env.fog_start + 1e-3), 0.5 * t.size.1 as f32 / (0.5 * self.lens.vertical_fov(aspect)).tan());
        if let Some(start) = grass_start { cpu.grass_prepare_ms = start.elapsed().as_secs_f64() * 1000.0; }
        let isolate_sky = self.isolate_sky();
        self.sky_beauty_only = isolate_sky;
        if self.enhanced_settings.any_enabled() && self.enhanced_frame.as_ref().is_none_or(|frame| frame.size != t.size || frame.sample_count != self.world.msaa) {
            self.reset_post_history();
            self.release_effect_targets();
            self.bloom = None;
            self.grade = None;
            self.enhanced_frame = Some(enhanced::EnhancedFrame::new(&self.device, self.format, t.size, self.world.msaa, self.enhanced_settings, water_cloud, &self.enhanced_pipelines));
        }
        if let Some(frame) = self.enhanced_frame.as_mut() {
            frame.set_water_cloud(&self.device, water_cloud);
        }
        self.prepare_water(cam, vp, t.size, env);
        self.prepare_enhanced_grade(env.fog_color);
        self.prepare_enhanced_bloom();
        let encode_start = timed.then(std::time::Instant::now);
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode_actor_skins(&mut enc);
        if let Some(buf) = self.gpu.inst_bufs.get(self.frame) {
            if !self.vis.is_empty() {
                self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.vis));
            }
        }
        for a in &mut self.sky.colors {
            a.sim.frame_at(self.time);
            let lin = |c: u32, shift: u32| srgb_to_linear(((c >> shift & 255) as f32 / 255.0 * a.gain).min(1.0));
            for (v, &c) in a.vertices.iter_mut().zip(a.sim.vertex_colors()) {
                v.color = [lin(c, 16), lin(c, 8), lin(c, 0), (c >> 24) as f32 / 255.0];
            }
            if let Some(Some((vb, _))) = self.sky.meshes.get(a.mesh) {
                self.queue.write_buffer(vb, 0, bytemuck::cast_slice(&a.vertices));
            }
        }
        let grid=ao_scene::cloud::resolution(self.cloud_preferences.0,self.cloud_preferences.1,self.cloud_preferences.2);
        for run in &mut self.sky.clouds {
            let dt=(self.time-run.time).max(0.0);run.time=self.time;
            run.layer.ambient=env.ambient.map(gamma);
            run.sim.update(&run.layer,grid,cam.pos.to_array(),dt,far,&mut run.vertices);
            if let Some(Some((vb,ib)))=self.sky.meshes.get(run.layer.mesh) {
                if grid!=0 {
                    self.queue.write_buffer(vb,0,bytemuck::cast_slice(&run.vertices));
                    if run.grid!=grid {
                        self.queue.write_buffer(ib,0,bytemuck::cast_slice(&run.indices[usize::from(grid==10)]));
                        let mut first=0;
                        for ((d,_),(a,b)) in self.sky.draws.iter_mut().filter(|(d,_)|d.mesh==run.layer.mesh).zip([(0,2),(2,4),(4,27),(27,30)]) {
                            d.first_index=first;d.count=((b-a)*(grid-1)*(grid-1)*6) as u32;first+=d.count;
                        }
                    }
                }
            }
            run.grid=grid;
        }
        if let Some(buf) = self.sky.bufs.get(self.frame) {
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
                    if self.sky.pose.sky_yaw.contains(&i) {
                        let forward = cam.forward();
                        let yaw = Mat4::from_rotation_y((-forward.x).atan2(-forward.z));
                        m = yaw * m;
                        off = yaw.transform_vector3(off);
                    }
                    // VisualSingleCloud already generates camera-relative metre positions at the live far.
                    let cloud = self.sky.clouds.iter().any(|cloud| self.sky.pose.sky.get(i).is_some_and(|instance| instance.mesh == cloud.layer.mesh));
                    sky_instance_matrix(m, off, cam.pos, if cloud { 1.0 } else { far / 1000.0 }).to_cols_array_2d()
                })
                .collect();
            self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&xf));
            if self.enhanced_settings.sky_discs { self.sky_disc_transforms.clone_from(&xf); }
        }
        if self.enhanced_settings.taa || self.enhanced_settings.motion_blur > 0.0 {
            let previous = self.post_previous_view.map_or(native_vp, |(matrix, _)| matrix);
            self.prepare_motion_vectors(t.size, native_vp, previous);
        }
        self.prepare_canopy_leaves();
        let shadow_start = timed.then(std::time::Instant::now);
        self.encode_enhanced_shadows(&mut enc, cam, t.size, env, far);
        if let Some(start) = shadow_start { cpu.shadow_prepare_encode_ms = start.elapsed().as_secs_f64() * 1000.0; }
        self.prepare_grass_shadow_receiver();
        self.prepare_atmosphere_stack(&mut enc, vp, cam.pos, far, env);
        self.encode_water_bottom(&mut enc);
        let inst_buf = self.gpu.inst_bufs.get(self.frame);
        let sky_buf = self.sky.bufs.get(self.frame);

        let calls = std::cell::Cell::new(0usize);
        self.screen.prepare(&self.device,&self.queue,self.format,t.size);
        self.heat_haze.update(t.size, vp, cam.pos, cam.forward(), self.liquids.iter().flat_map(|run| run.heat_spheres.iter()));
        self.heat_haze.prepare(&self.device, &self.queue, self.format, t.size);
        let world_resolve = self.heat_haze.source().unwrap_or(resolve);
        let enhanced_frame = self.enhanced_frame.as_ref();
        let world_target = enhanced_frame.map_or(world_resolve, |frame| &frame.colour);
        let msaa_target = enhanced_frame.and_then(|frame| frame.msaa.as_ref().and_then(|views| views[0].as_ref())).or(t.msaa.as_ref().filter(|_| enhanced_frame.is_none()));
        let mut attachments = [
            Some(wgpu::RenderPassColorAttachment { view: msaa_target.unwrap_or(world_target), resolve_target: if isolate_sky { None } else { msaa_target.map(|_| world_target) }, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(framebuffer_clear(env.sky_color)), store: if msaa_target.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store } } }),
            enhanced_frame.and_then(|frame| frame.attachment(1)),
            enhanced_frame.and_then(|frame| frame.attachment(2)),
            enhanced_frame.and_then(|frame| frame.attachment(3)),
            enhanced_frame.and_then(|frame| frame.attachment(4)),
        ];
        if isolate_sky { attachments[0].as_mut().unwrap().ops.store = wgpu::StoreOp::Store; }
        let final_world = enhanced_frame.is_none() && self.heat_haze.source().is_none() && self.screen.layers.is_empty();
        let timestamps = self.gpu_timer.as_mut().and_then(|timer| {
            if timer.is_recording() { final_world.then(|| timer.end_pass()) } else { Some(timer.begin_pass(final_world)) }
        }).map(|writes| (writes.beginning_of_pass_write_index, writes.end_of_pass_write_index));
        let timestamp_queries = self.gpu_timer.as_ref().map(|timer| timer.query_set());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &attachments[..if isolate_sky { 1 } else { enhanced_frame.map_or(1, enhanced::EnhancedFrame::target_count) }],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: enhanced_frame.map_or(&t.depth, |frame| &frame.depth),
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: if isolate_sky || enhanced_frame.is_some() { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } }),
                    stencil_ops: None,
                }),
                timestamp_writes: timestamps.and_then(|(begin, end)| {
                    let end = if isolate_sky { None } else { end };
                    (begin.is_some() || end.is_some()).then(|| wgpu::RenderPassTimestampWrites {
                        query_set: timestamp_queries.unwrap(), beginning_of_pass_write_index: begin, end_of_pass_write_index: end,
                    })
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            // Sky, ground and actors retain this receiver across pipeline changes.
            if self.enhanced_settings.sun_shadows || self.enhanced_settings.moon_shadows || self.enhanced_settings.needs_admitted_lights()
                || self.enhanced_settings.night_mode == NightMode::Moonlit {
                pass.set_bind_group(3, self.shadow_receiver(), &[]);
            }
            let (pipe, mesh, mat) = (std::cell::Cell::new(usize::MAX), std::cell::Cell::new((false, usize::MAX, false)), std::cell::Cell::new((false, usize::MAX)));
            let draw = |pass: &mut wgpu::RenderPass, d: &Draw, insts: std::ops::Range<u32>, sky: bool| {
                if pipe.get() != d.pipe {
                    pass.set_pipeline(if d.pipe >= 1000 { &self.world.ground_pipes[d.pipe - 1000] }
                        else if sky && isolate_sky { &self.world.native_sky_pipes.as_ref().unwrap_or(&self.world.pipes)[d.pipe] }
                        else { &self.world.pipes[d.pipe] });
                    pipe.set(d.pipe);
                }
                let detail = if !sky && d.vegetation && self.enhanced_settings.foliage_subdivision
                    && insts.clone().any(|vi| {
                        let instance = &self.gpu.insts[self.vis_src[vi as usize] as usize];
                        foliage_detail::use_subdivision(instance.center, instance.radius, self.foliage.selected())
                    }) {
                    self.foliage_detail_state.get(d.mesh, d.first_index)
                } else { None };
                let mesh_key = (sky, d.mesh, detail.is_some());
                if mesh.get() != mesh_key {
                    let (vb, ib) = detail.map(|(vb, ib, _)| (vb, ib)).unwrap_or_else(|| {
                        let (vb, ib) = if sky { &self.sky.meshes } else { &self.gpu.meshes }[d.mesh].as_ref().unwrap();
                        (vb, ib)
                    });
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    mesh.set(mesh_key);
                }
                if mat.get() != (sky, d.mat) {
                    pass.set_bind_group(1, &if sky { &self.sky.mats } else { &self.gpu.mats }[d.mat], &[]);
                    mat.set((sky, d.mat));
                }
                let [first, count] = detail.map_or([d.first_index, d.count], |(_, _, range)| range);
                pass.draw_indexed(first..first + count, 0, insts.clone());
                calls.set(calls.get() + 1);
                if !sky && d.count != 0 {
                    if let Some((node, next)) = d.node {
                        for vi in insts.clone() {
                            self.pending_visuals.borrow_mut().insert(VisualKey::World(self.vis_src[vi as usize] as usize, node), next);
                        }
                    }
                }
            };
            // Native viewport clears are ordered actor commands. Keep the entire
            // frame native if any are present rather than replaying soft pixels
            // against a depth buffer from a different native clear interval.
            let split_foliage = self.enhanced_settings.foliage_edges && self.screen.depth_clears.is_empty();
            let world_phase = |pass: &mut wgpu::RenderPass, priority: i32| {
                pass.set_bind_group(0, &self.globals_bg, &[]);
                // Grass replaces group 3; restore the shared receiver at phase entry.
                if self.enhanced_settings.sun_shadows || self.enhanced_settings.moon_shadows || self.enhanced_settings.needs_admitted_lights()
                    || self.enhanced_settings.night_mode == NightMode::Moonlit {
                    pass.set_bind_group(3, self.shadow_receiver(), &[]);
                }
                pipe.set(usize::MAX); mesh.set((false, usize::MAX, false)); mat.set((false, usize::MAX));
                if let Some(inst) = inst_buf {
                    pass.set_vertex_buffer(1, inst.slice(..));
                    let first = self.sorted.partition_point(|command| command.0 < priority);
                    let last = self.sorted.partition_point(|command| command.0 <= priority);
                    for &(_, _, list, di, vi) in &self.sorted[first..last] {
                        let d = &[&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid][list as usize][di];
                        let core = (split_foliage && d.vegetation)
                            .then(|| foliage_edges::pipeline(&self.world.pipes, d.pipe, false)).flatten();
                        if let Some(pipe) = core {
                            draw(pass, &Draw { pipe, ..*d }, vi..vi + 1, false);
                        } else {
                            draw(pass, d, vi..vi + 1, false);
                        }
                    }
                }
                calls.set(calls.get() + self.draw_actors(pass, priority));
                if let Some(inst) = inst_buf.filter(|_| split_foliage) {
                    pass.set_bind_group(0, &self.globals_bg, &[]);
                    if self.enhanced_settings.sun_shadows || self.enhanced_settings.moon_shadows || self.enhanced_settings.night_mode == NightMode::Moonlit {
                        pass.set_bind_group(3, &self.sun_shadows.as_ref().expect("shadow maps prepared before world").receiver, &[]);
                    }
                    pass.set_vertex_buffer(1, inst.slice(..));
                    pipe.set(usize::MAX); mesh.set((false, usize::MAX, false)); mat.set((false, usize::MAX));
                    let first = self.sorted.partition_point(|command| command.0 < priority);
                    let last = self.sorted.partition_point(|command| command.0 <= priority);
                    // Replay only the complementary soft coverage, preserving
                    // native relative order and never crossing a priority.
                    for &(_, _, list, di, vi) in &self.sorted[first..last] {
                        let d = &[&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid][list as usize][di];
                        if !d.vegetation { continue; }
                        if let Some(pipe) = foliage_edges::pipeline(&self.world.pipes, d.pipe, true) {
                            draw(pass, &Draw { pipe, ..*d }, vi..vi + 1, false);
                        }
                    }
                }
            };
            if let Some(sky) = sky_buf {
                pass.set_vertex_buffer(1, sky.slice(..));
                for (d, k) in &self.sky.draws {
                    let cloud = self.sky.clouds.iter().find(|run| run.layer.mesh == d.mesh);
                    if grid == 0 && cloud.is_some() { continue; }
                    if let Some(cloud) = cloud {
                        // DS10054e07 submits each sheet separately; its four ranges only switch materials.
                        let count = ((cloud.grid - 1).pow(2) * 6) as u32;
                        #[cfg(test)]
                        let count = if self.cloud_precision_probe == CloudPrecisionProbe::Grouped { d.count } else { count };
                        for first_index in (d.first_index..d.first_index + d.count).step_by(count as usize) {
                            draw(&mut pass, &Draw { first_index, count, ..*d }, *k..*k + 1, true);
                        }
                    } else {
                        draw(&mut pass, d, *k..*k + 1, true);
                    }
                }
            }
            if self.enhanced_settings.volumetric_clouds {
                self.volumetric_clouds.as_ref().expect("startup cloud pipelines").draw_sky(&mut pass, self.world.msaa);
            }
            if isolate_sky {
                drop(pass);
                let beauty = attachments[0].as_mut().unwrap();
                beauty.ops.load = wgpu::LoadOp::Load;
                beauty.ops.store = if msaa_target.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store };
                beauty.resolve_target = msaa_target.map(|_| world_target);
                pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native world after beauty-only sky"),
                    color_attachments: &attachments[..enhanced_frame.map_or(1, enhanced::EnhancedFrame::target_count)],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: enhanced_frame.map_or(&t.depth, |frame| &frame.depth),
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: if enhanced_frame.is_some() { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: timestamps.and_then(|(_, end)| end.map(|index| wgpu::RenderPassTimestampWrites {
                        query_set: timestamp_queries.unwrap(), beginning_of_pass_write_index: None, end_of_pass_write_index: Some(index),
                    })),
                    occlusion_query_set: None, multiview_mask: None,
                });
            }
            world_phase(&mut pass, 0);
            // AnarchyGround_t ctor 10033b97 sets priority 1. Complete the
            // native base stage for all terrain before the shadow stage.
            if let (Some(ground), Some(inst)) = (&self.ground, inst_buf) {
                for stage in 0..=1 {
                    if stage == 1 && self.ground_state.path() != GroundPath::ShadowsTwoPass { break; }
                    pass.set_bind_group(0,&self.globals_bg,&[]);
                    pass.set_vertex_buffer(1,inst.slice(..));
                    pipe.set(usize::MAX);mesh.set((false,usize::MAX,false));mat.set((false,usize::MAX));
                    for d in &self.gpu.opaque {
                        let Some(palette) = &ground.materials[d.mesh] else { continue; };
                        let r = self.vis_range[d.mesh].clone();
                        if r.is_empty() || d.count == 0 { continue; }
                        let path = if ground.has_shadow[d.mesh] { self.ground_state.path() } else { GroundPath::Simple };
                        if stage == 1 && path != GroundPath::ShadowsTwoPass { continue; }
                        pass.set_bind_group(2,palette,&[]);
                        let mut gd = *d;
                        gd.pipe = 1000 + if stage == 1 { 2 } else { match path { GroundPath::Fused => 0, GroundPath::ShadowsTwoPass => 1, GroundPath::Simple => 3 } };
                        draw(&mut pass,&gd,r,false);
                    }
                }
            }
            let grass_start = timed.then(std::time::Instant::now);
            calls.set(calls.get() + self.draw_enhanced_grass(&mut pass));
            if let Some(start) = grass_start { cpu.grass_encode_ms = start.elapsed().as_secs_f64() * 1000.0; }
            // Existing native list order; list 4 remains the liquid list.
            for priority in [1, 2, 3, 4, 5, 6, 10, 7] {
                world_phase(&mut pass, priority);
            }
            calls.set(calls.get() + self.draw_canopy_leaves(&mut pass) as usize);
        }
        if let Some(frame) = enhanced_frame {
            frame.resolve_depth(&mut enc);
        }
        self.encode_water(&mut enc);
        if self.enhanced_settings.ambient_occlusion {
            let ao_start = timed.then(std::time::Instant::now);
            self.encode_enhanced_ao(&mut enc, vp, ambient_occlusion::fog_depth_limit(self.lens.near, far, env.fog_end));
            if let Some(start) = ao_start { cpu.ao_ms = start.elapsed().as_secs_f64() * 1000.0; }
        }
        if self.enhanced_settings.contact_shadows {
            self.encode_enhanced_contact_shadows(&mut enc, vp, cam.pos, Vec3::from(env.sun_dir), far);
        }
        self.encode_atmosphere_stack(&mut enc, vp, cam.pos, far, env);
        // After AO and contact shadows: expose first so the HDR pyramid reads the lifted scene; non-HDR bloom works on the native colour.
        let bloom_strength = if self.bloom.is_some() { self.enhanced_settings.bloom_strength } else { 0.0 };
        if let (Some(grade), Some(frame)) = (&self.grade, self.enhanced_frame.as_ref()) {
            grade.expose(&self.queue, &mut enc, &frame.composite_bg, if self.enhanced_settings.hdr { self.enhanced_settings.sun_boost } else { 0.0 }, bloom_strength);
        }
        if let Some(bloom) = &mut self.bloom {
            let bloom_start = timed.then(std::time::Instant::now);
            bloom.encode(&self.queue, &mut enc, self.enhanced_settings.bloom_strength);
            if let Some(start) = bloom_start { cpu.bloom_ms = start.elapsed().as_secs_f64() * 1000.0; }
        }
        if let Some(frame) = self.enhanced_frame.as_ref() {
            let post = self.post_target.as_ref();
            let timestamps = if post.is_none() && self.heat_haze.source().is_none() && self.screen.layers.is_empty() { self.gpu_timer.as_mut().map(FrameGpuTimer::end_pass) } else { None };
            let output = post.map_or_else(|| self.heat_haze.source().unwrap_or(resolve), |target| &target.view);
            if let Some(grade) = &self.grade {
                grade.present(&mut enc, output, timestamps);
            } else {
                frame.composite(&mut enc, output, self.bloom.as_ref().and_then(|bloom| bloom.present.as_ref()), timestamps);
            }
        }
        self.encode_post_frame(&mut enc, resolve, cam, native_vp, far);
        let timestamps = if self.heat_haze.source().is_some() && self.screen.layers.is_empty() { self.gpu_timer.as_mut().map(FrameGpuTimer::end_pass) } else { None };
        self.heat_haze.draw(&mut enc, resolve, timestamps);
        let timestamps = if !self.screen.layers.is_empty() { self.gpu_timer.as_mut().map(FrameGpuTimer::end_pass) } else { None };
        self.screen.draw(&mut enc, resolve, timestamps);
        self.drawn_visuals.extend(self.pending_visuals.get_mut().drain());
        self.stats = FrameStats { instances: self.vis.len(), draw_calls: calls.get() };
        let finish_start = timed.then(std::time::Instant::now);
        let commands = enc.finish();
        if let Some(start) = finish_start { cpu.finish_ms = start.elapsed().as_secs_f64() * 1000.0; }
        let submit_start = timed.then(std::time::Instant::now);
        let submission = self.queue.submit([commands]);
        if let (Some(prepare), Some(encode), Some(submit)) = (prepare_start, encode_start, submit_start) {
            cpu.prepare_ms = (encode - prepare).as_secs_f64() * 1000.0;
            cpu.encode_ms = (submit - encode).as_secs_f64() * 1000.0;
            cpu.queue_submit_ms = submit.elapsed().as_secs_f64() * 1000.0;
            self.cpu_timings = Some(cpu);
        }
        submission
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

/// `actors::ensure_capacity`, but also handles the "no buffer yet" case (a mesh slot just cleared by a full sky
/// rebuild, or newly live): creates a fresh buffer at the first needed capacity instead of requiring one to grow.
/// Returns whether a new buffer was actually created (vs. the existing one reused as-is), for leak regressions.
fn ensure_buf(device: &wgpu::Device, buf: Option<wgpu::Buffer>, capacity: &mut u64, needed: u64, usage: wgpu::BufferUsages, label: &str) -> (wgpu::Buffer, bool) {
    match buf {
        Some(mut b) => {
            let grew = actors::ensure_capacity(device, &mut b, capacity, needed, usage, label);
            (b, grew)
        }
        None => {
            *capacity = needed.next_power_of_two().max(1);
            (device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some(label), size: *capacity, usage, mapped_at_creation: false }), true)
        }
    }
}

/// Renders `scene` offscreen and writes an sRGB PNG.
pub fn render_to_png(scene: &Scene, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path) -> Result<()> {
    render_to_png_at(scene, eye, look_at, width, height, path, 0.0)
}

/// [`render_to_png`] with the scrolling textures (`Submesh::uv_scroll`) at `time` seconds.
pub fn render_to_png_at(scene: &Scene, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path, time: f32) -> Result<()> {
    render_to_png_actors(scene, &[] as &[(u64, Scene)], vec![], eye, look_at, width, height, path, time)
}

/// [`render_to_png_at`] with dynamic actors on top of the scene (`models` for [`Renderer::add_actor_model`], `actors` for [`Renderer::set_actors`]).
#[allow(clippy::too_many_arguments)]
pub fn render_to_png_actors(scene: &Scene, models: &[(u64, impl std::borrow::Borrow<Scene>)], actors: Vec<ao_scene::ActorFrame>, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path, time: f32) -> Result<()> {
    render_to_png_effects(scene,models,actors,eye,look_at,width,height,path,time,vec![],NativeFog::default())
}

/// Offscreen native viewport effects, also used by the installed configuration regression.
pub fn render_to_png_screen(scene:&Scene,layers:Vec<ScreenLayer>,fog_disabled:bool,width:u32,height:u32,path:&Path)->Result<()> {
    render_to_png_screen_fog(scene,layers,NativeFog{enabled:!fog_disabled,..Default::default()},width,height,path)
}
pub fn render_to_png_screen_fog(scene:&Scene,layers:Vec<ScreenLayer>,fog:NativeFog,width:u32,height:u32,path:&Path)->Result<()> {
    render_to_png_effects(scene,&[] as &[(u64, Scene)],vec![],[0.0,0.0,5.0],[0.0;3],width,height,path,0.0,layers,fog)
}

#[allow(clippy::too_many_arguments)]
fn render_to_png_effects(scene: &Scene, models: &[(u64, impl std::borrow::Borrow<Scene>)], actors: Vec<ao_scene::ActorFrame>, eye: [f32; 3], look_at: [f32; 3], width: u32, height: u32, path: &Path, time: f32,layers:Vec<ScreenLayer>,fog:NativeFog) -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r = Renderer::new(&instance, None)?;
    r.upload(scene);
    for (k, m) in models {
        r.add_actor_model(*k, m.borrow());
    }
    r.set_actors(actors);
    r.seek(time);
    r.set_fog_density_scale(fog.weight);
    r.set_screen_effects(layers,!fog.enabled);
    for layer in &r.screen.layers.clone() {
        if let Some(key)=layer.texture {
            let texture=scene.textures.get(&key).context("missing authored screen texture")?;
            r.upload_screen_texture(key,texture);
        }
    }
    let targets = Targets::new(&r, width, height);
    let out = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: r.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    r.render(&out.create_view_tracked(&Default::default()), &targets, &Camera::look_at(eye.into(), look_at.into()));

    texture_to_png(&r, &out, width, height, path)
}

/// Reads back a `r.format` texture and writes it as PNG.
pub(crate) fn texture_to_png(r: &Renderer, out: &wgpu::Texture, width: u32, height: u32, path: &Path) -> Result<()> {
    let pixels = texture_to_rgba(r, out, width, height)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

/// Reads back a `r.format` texture as raw RGBA8 bytes (row-major, no padding): shared by [`texture_to_png`] and
/// visual-regression tests that diff pixels directly instead of round-tripping through a PNG file.
pub(crate) fn texture_to_rgba(r: &Renderer, out: &wgpu::Texture, width: u32, height: u32) -> Result<Vec<u8>> {
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = r.device.create_buffer_tracked(&wgpu::BufferDescriptor {
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
    Ok(pixels)
}

/// Reads back `texture` (any `Renderer::format`, RGBA or BGRA) and writes it as a PNG to `path` on a background
/// thread: the GPU copy+map here is the fast, already-synchronous part; PNG's zlib encode is the slow CPU part
/// (aomac's F12 screenshot), so it must not stall the render thread. Swaps R/B if the surface format is BGRA
/// (the swapchain is typically BGRA on macOS Metal; the headless `--screenshot` CLI path is always RGBA).
pub fn capture_frame_to_png(r: &Renderer, texture: &wgpu::Texture, width: u32, height: u32, path: std::path::PathBuf) -> Result<()> {
    let mut pixels = texture_to_rgba(r, texture, width, height)?;
    if matches!(r.format, wgpu::TextureFormat::Bgra8Unorm) {
        for px in pixels.as_chunks_mut::<4>().0 { px.swap(0, 2); }
    }
    std::thread::Builder::new().name("screenshot-png".into()).spawn(move || {
        let write = (|| -> Result<()> {
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&path)?), width, height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(&pixels)?;
            Ok(())
        })();
        if let Err(e) = write { eprintln!("screenshot: {}: {e}", path.display()); }
    })?;
    Ok(())
}

fn storage<T: bytemuck::Pod>(device: &wgpu::Device, data: &[T]) -> wgpu::Buffer {
    device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(data), usage: wgpu::BufferUsages::STORAGE })
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

/// Light-grid storage buffers ([`globals_bind`]'s binding 1-3). Kept as `Renderer` fields so a sampler-only change
/// (`Renderer::set_anisotropy`) can rebuild `globals_bg` without recomputing the light grid.
fn light_storage_buffers(device: &wgpu::Device, g: &LightGrid) -> (wgpu::Buffer, wgpu::Buffer, wgpu::Buffer) {
    let l = device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&g.lights), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST });
    let (c, i) = (storage(device, &g.cells), storage(device, &g.idx));
    (l, c, i)
}

/// Zero-initialized (wgpu buffers are zero-filled on creation) per-frame moving-light buffer; `Renderer::set_dynamic_lights`
/// rewrites the live prefix every frame. Recreated (losing any live dynamic lights) on `Renderer::new`/`upload`, kept as-is
/// by a sampler-only `set_anisotropy` change.
fn make_dyn_light_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer_tracked(&wgpu::BufferDescriptor {
        label: None,
        size: (MAX_DYN_LIGHTS * 4 * 16) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Builds `globals_bg` (group 0: globals uniform, light-grid storage buffers, the per-frame dynamic-light buffer, both
/// material samplers). Samplers live here rather than per-material so [`Renderer::set_anisotropy`] only rebuilds this one
/// bind group.
#[allow(clippy::too_many_arguments)]
fn globals_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, globals: &wgpu::Buffer, light_buf: &wgpu::Buffer, cells_buf: &wgpu::Buffer, idx_buf: &wgpu::Buffer, dyn_light_buf: &wgpu::Buffer, sampler: &wgpu::Sampler, clamp_sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: light_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: cells_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: idx_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: dyn_light_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(clamp_sampler) },
        ],
    })
}

#[cfg(test)]
mod sky_tests {
    /// Pure, GPU-free regression for the MSAA-option decision's downward clamp: unsupported levels fall back to the next lower
    /// one the adapter format features actually report, and a missing/empty list never panics (1 = off is always valid).
    #[test]
    fn clamp_msaa_samples_downward_clamps_to_nearest_supported() {
        assert_eq!(super::clamp_msaa_samples(1, &[1, 2, 4]), 1);
        assert_eq!(super::clamp_msaa_samples(4, &[1, 2, 4]), 4);
        assert_eq!(super::clamp_msaa_samples(8, &[1, 2, 4]), 4, "no 8x support: fall back to the nearest lower level, not panic");
        assert_eq!(super::clamp_msaa_samples(2, &[1, 4]), 1, "no exact match: the nearest level below the request, not above");
        assert_eq!(super::clamp_msaa_samples(4, &[]), 1, "missing format-feature data never panics: 1 (off) is always valid");
        assert_eq!(super::clamp_msaa_samples(16, &[1, 2, 4, 8, 16]), 16);
    }

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

    #[test]
    fn ambient_alpha_follows_depth_owner_without_changing_rgb_blending() {
        for state in [None, Some(wgpu::BlendState::ALPHA_BLENDING), Some(native_blend_state(Blend::SourceColorOne))] {
            for depth_write in [false, true] {
                let target = ambient_target(state, depth_write);
                assert_eq!(target.format, enhanced::AUX);
                assert_eq!(target.blend.map(|blend| blend.color), state.map(|blend| blend.color));
                assert_eq!(target.blend.map(|blend| blend.alpha), state.map(|_| wgpu::BlendComponent::REPLACE));
                assert_eq!(target.write_mask, if depth_write { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::COLOR });
            }
        }
    }

    #[test]
    fn native_material_state_slots_keep_depth_alpha_and_blend_independent() {
        let _ = build_pipelines(); // Slot-table layout assertions are GPU-free.
        for blend in [Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::AdditiveOne] {
            for two_sided in [false, true] {
                for write in [false, true] {
                    for compare in [2, 4, 8] {
                        let sub = Submesh { blend, two_sided, depth_write: Some(write), depth_func: Some(compare),
                            alpha_test: Some(30), alpha_func: 5, ..Submesh::new(vec![], None) };
                        let mode = match compare { 2 => 0, 4 => 1, _ => 2 };
                        let slot = mode * 36 + write as usize * 18 + blend as usize * 2 + two_sided as usize;
                        assert_eq!(material_pipe(&sub), MATERIAL_COMPARE_PIPE + slot);
                        assert_eq!(world_material_pipe(&sub), WORLD_COMPARE_PIPE + slot);
                        let uniform = mat_uniform(&sub);
                        assert_eq!(uniform[29], 30.0 / 255.0);
                        assert_eq!((uniform[30], uniform[31]), (1.0, 5.0));
                    }
                }
            }
        }
        let no_light_or_fog = Submesh { fog_enabled: false, lighting_enabled: false, ..Submesh::new(vec![], None) };
        assert_eq!(mat_uniform(&no_light_or_fog)[30], 6.0);
    }

    #[test]
    fn ordinary_environment_add_precedes_blend_while_cat_env_is_another_one_one_pass() {
        let world = Scene {
            environment: Some(Environment { sky_color: [0.0; 3], fog_color: [0.0; 3],
                fog_start: 100.0, fog_end: 200.0, ambient: [0.0; 3], sun_color: [0.0; 3],
                sun_dir: [0.0, 0.0, 1.0], sun_specular: 0.0 }),
            ..Default::default()
        };
        let env = TextureKey { rdb_type: 1010004, id: 1 };
        let model = |cat: bool| {
            let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
                blend: Blend::AlphaBlend, base_color: [1.0, 1.0, 1.0, 0.25],
                stage1_env_texture: (!cat).then_some(env), env_texture: cat.then_some(env),
                ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
            });
            model.textures.insert(env, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![0, 255, 0, 0] });
            model
        };
        let Some(ordinary) = actor_shot(&world, model(false), [0.0, 0.0, -3.0], "ordinary-env-stage") else { return };
        let Some(cat) = actor_shot(&world, model(true), [0.0, 0.0, -3.0], "cat-env-pass") else { return };
        assert!((60..=68).contains(&ordinary[1]), "{ordinary:?}: stage1 ADD still uses the base blend alpha");
        assert!(cat[1] >= 250, "{cat:?}: CAT ONE/ONE env RGB is independent of its alpha");
    }

    #[test]
    fn native_material_comparisons_and_unlit_fog_reach_fragments() {
        let key = TextureKey { rdb_type: 1010004, id: 1 };
        let world = Scene {
            environment: Some(Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 100.0,
                fog_end: 200.0, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 0.0 }),
            ..Default::default()
        };
        for (function, passes) in [(1, false), (2, false), (3, true), (4, true), (5, false), (6, false), (7, true), (8, true)] {
            let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
                blend: Blend::AlphaBlend, emissive: [1.0; 3], alpha_test: Some(128), alpha_func: function,
                ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key))
            });
            model.textures.insert(key, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![255, 0, 0, 128] });
            let Some(pixel) = actor_shot(&world, model, [0.0, 0.0, -3.0], &format!("native-alpha-cmp-{function}")) else { return };
            assert_eq!(pixel[0] > 120, passes, "native cmp{function}: {pixel:?}");
        }
        for fog_enabled in [false, true] {
            let mut fog_world = world.clone();
            let env = fog_world.environment.as_mut().unwrap();
            env.fog_color = [0.0, 1.0, 0.0];
            env.fog_start = 0.0;
            env.fog_end = 1.0;
            let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
                lighting_enabled: false, fog_enabled,
                ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key))
            });
            model.textures.insert(key, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![255, 0, 0, 255] });
            let Some(pixel) = actor_shot(&fog_world, model, [0.0, 0.0, -3.0], &format!("native-unlit-fog-{fog_enabled}")) else { return };
            assert!(if fog_enabled { pixel[1] > 250 && pixel[0] < 2 } else { pixel[0] > 250 && pixel[1] < 2 }, "{pixel:?}");
        }
    }

    #[test]
    fn native_node_priority_advances_only_after_a_visible_submission() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(mut renderer) = Renderer::new(&instance, None) else { return };
        let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
            emissive: [1.0; 3], render_priority: Some(6), render_node: Some((7, 3)),
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        });
        renderer.add_actor_model(1, &model);
        let target = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let targets = Targets::new(&renderer, 32, 32);
        let mut transform = IDENTITY;
        transform[3][2] = -3.0;
        renderer.set_actors(vec![ao_scene::ActorFrame { id: 1, model: 1, transform, ..Default::default() }]);
        let key = VisualKey::Actor(1, 1, 0, 7);
        renderer.render(&target.create_view_tracked(&Default::default()), &targets, &Camera::look_at(Vec3::ZERO, Vec3::Z));
        assert!(!renderer.drawn_visuals.contains_key(&key), "culled submissions do not mutate native priority");
        renderer.render(&target.create_view_tracked(&Default::default()), &targets, &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
        assert_eq!(renderer.drawn_visuals.get(&key), Some(&3));
        model.meshes[0].vertices[0].pos[0] += 0.01;
        renderer.update_actor_model(1, &model);
        assert_eq!(renderer.drawn_visuals.get(&key), Some(&3), "pose updates do not recreate a native visual");
        renderer.add_actor_model(1, &model);
        assert!(!renderer.drawn_visuals.contains_key(&key), "replacement starts at authored priority again");
    }

    #[test]
    fn destroyed_visual_churn_is_bounded_without_resetting_culled_priorities_or_pixels() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("visual retirement regression requires GPU");
        let model = actor_quad([0.0, 0.0, 1.0], Submesh {
            emissive: [1.0; 3], base_color: [1.0, 0.0, 0.0, 1.0],
            lighting_enabled: false, fog_enabled: false,
            render_priority: Some(6), render_node: Some((7, 7)),
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        });
        renderer.add_actor_model(1, &model);
        let mut blocker = model.clone();
        let material = &mut blocker.meshes[0].submeshes[0];
        material.base_color = [0.0, 0.0, 1.0, 1.0];
        material.render_priority = Some(10);
        material.render_node = None;
        renderer.add_actor_model(2, &blocker);
        let target = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let targets = Targets::new(&renderer, 32, 32);
        let camera = Camera::look_at(Vec3::ZERO, Vec3::NEG_Z);
        let view = target.create_view_tracked(&Default::default());
        let mut transform = IDENTITY;
        transform[3][2] = -3.0;
        let frame = |id| ao_scene::ActorFrame { id, model: 1, transform, ..Default::default() };
        let blocker_frame = ao_scene::ActorFrame { id: 2, model: 2, transform, ..Default::default() };
        let survivor = VisualKey::Actor(1, 1, 0, 7);
        let center = (16 * 32 + 16) * 4;
        renderer.set_actors(vec![frame(1), blocker_frame.clone()]);
        renderer.render(&view, &targets, &camera);
        assert_eq!(renderer.drawn_visuals.get(&survivor), Some(&7));
        for cycle in 0..64 {
            let first = 0xe000_0000 + cycle * 32;
            renderer.set_actors(vec![frame(first), frame(first + 31), blocker_frame.clone()]);
            renderer.render(&view, &targets, &camera);
            let authored = texture_to_rgba(&renderer, &target, 32, 32).unwrap();
            assert_eq!(&authored[center..center + 4], &[0, 0, 255, 255],
                "native order is 6,10,7: the fixed list-10 blocker covers authored list 6");
            assert_eq!(renderer.drawn_visuals.len(), 3);
            assert_eq!(renderer.drawn_visuals.get(&survivor), Some(&7),
                "non-submission must not reset the native 6 -> 7 transition");
            renderer.render(&view, &targets, &camera);
            let unretired_reference = texture_to_rgba(&renderer, &target, 32, 32).unwrap();
            assert_eq!(&unretired_reference[center..center + 4], &[255, 0, 0, 255],
                "advanced list 7 must cover the blue blocker; the priority oracle is non-vacuous");
            assert_ne!(authored, unretired_reference);
            // Nested destruction can follow emission of the final ActorFrames.
            // F12/capture can render those frames repeatedly before the next host apply.
            renderer.queue_actor_visual_retirements(&mut vec![(first, 32)]);
            for _ in 0..2 {
                renderer.render(&view, &targets, &camera);
                assert_eq!(unretired_reference, texture_to_rgba(&renderer, &target, 32, 32).unwrap(),
                    "final-frame and capture renders must match the equivalent unretired reference");
                assert_eq!(renderer.drawn_visuals.len(), 3);
                assert_eq!(renderer.drawn_visuals.get(&VisualKey::Actor(first, 1, 0, 7)), Some(&7));
                assert_eq!(renderer.actor_visual_retirements.len(), 1);
            }
            // The next host apply retires old ranges before replacing their frames.
            renderer.pending_visuals.get_mut().insert(VisualKey::Actor(first + 17, 2, 4, 8), 9);
            renderer.pending_visuals.get_mut().insert(VisualKey::World(0, 19), 2);
            renderer.queue_actor_visual_retirements(&mut vec![]);
            renderer.set_actors(vec![frame(1), blocker_frame.clone()]);
            assert_eq!(renderer.drawn_visuals.len(), 1, "no retained-state slope under monotonic IDs");
            assert_eq!(renderer.pending_visuals.borrow().len(), 1);
            assert_eq!(renderer.pending_visuals.get_mut().remove(&VisualKey::World(0, 19)), Some(2));
            renderer.render(&view, &targets, &camera);
            assert_eq!(unretired_reference, texture_to_rgba(&renderer, &target, 32, 32).unwrap(),
                "the returning survivor retains advanced priority and identical RGBA");
            assert_eq!(renderer.drawn_visuals.get(&survivor), Some(&7));
            assert!(renderer.pending_visuals.borrow().is_empty());
            assert!(renderer.actor_visual_retirements.is_empty());
            assert_eq!(renderer.actor_cache_sizes().0, 2, "shared models are not retired");
        }
        // Logic-only Offscreen applies frames without render calls.
        for cycle in 0..256 {
            let first = 0xe200_0000 + cycle * 32;
            let mut ranges = vec![(first, 32)];
            renderer.queue_actor_visual_retirements(&mut ranges);
            renderer.set_actors(vec![]);
            assert!(ranges.is_empty());
            assert_eq!(renderer.actor_visual_retirements.len(), 1);
            assert_eq!(renderer.drawn_visuals.len(), 1);
        }
        renderer.queue_actor_visual_retirements(&mut vec![]);
        assert!(renderer.actor_visual_retirements.is_empty());
        renderer.pending_visuals.get_mut().insert(survivor, 7);
        renderer.clear_actors();
        assert!(renderer.drawn_visuals.is_empty() && renderer.pending_visuals.borrow().is_empty());
        assert!(renderer.actor_visual_retirements.is_empty());
    }

    #[test]
    fn native_fog_start_and_end_follow_camera_planes_after_snapshot_refresh() {
        let original = default_environment(400.0);
        for far in [800.0, 1600.0] {
            for near in [0.2, 0.5] {
                // A newly delivered snapshot still has its offline placeholder planes.
                let mut model = ao_scene::FogModel { near: 0.5, far: 800.0, base_density: 1.0, ..Default::default() };
                let mut env = original;
                let lens = ao_scene::Lens { near, far: Some(far), ..Default::default() };
                apply_camera_fog(&mut env, &mut model, lens, far, [0.0; 3], 1.0);
                assert_eq!(env.fog_start, near);
                assert_eq!((model.near, model.far), (near, far));
                assert!((env.fog_end - (near + 5.0)).abs() < 0.0001);
                model.base_density = 0.025;
                apply_camera_fog(&mut env, &mut model, lens, far, [0.0; 3], 1.0);
                assert!((env.fog_end - (far - (far - near - 5.0) * 0.025)).abs() < 0.0001);
            }
        }
    }

    #[test]
    fn native_sky_scale_keeps_angles_and_real_projection_depth() {
        let camera = Vec3::new(150.0, 80.0, -40.0);
        let local = Vec3::new(120.0, 90.0, -300.0);
        let base = Mat4::from_translation(Vec3::new(10.0, 0.0, -20.0));
        let mut previous_angle: Option<Vec3> = None;
        for far in [800.0, 1600.0] {
            let matrix = sky_instance_matrix(base, Vec3::ZERO, camera, far / 1000.0);
            let relative = matrix.transform_point3(local) - camera;
            assert!((relative - (local + base.w_axis.truncate()) * (far / 1000.0)).length() < 0.001);
            let angle = relative.normalize();
            if let Some(previous) = previous_angle { assert!((angle - previous).length() < 1e-6); }
            previous_angle = Some(angle);
            let projection = Mat4::perspective_rh(1.0, 1.6, 0.2, far);
            let clip = projection * relative.extend(1.0);
            assert!(clip.z > 0.0 && clip.z < clip.w, "sky has its real depth, not z=w");
            let beyond = projection * Vec3::new(0.0, 0.0, -far * 1.01).extend(1.0);
            assert!(beyond.z > beyond.w, "native far clipping remains enabled");
            // Source fog uses eye depth, which scales along with the projection far.
            assert!((-relative.z / far - 0.32).abs() < 1e-6);
        }
    }

    #[test]
    fn cloud_identity_survives_preceding_sky_mesh_changes() {
        let mut old=Scene {sky:vec![Instance {mesh:3,transform:IDENTITY}],sky_pose_objects:vec![(0,42)],..Scene::default()};
        let new=Scene {sky:vec![Instance {mesh:7,transform:IDENTITY}],sky_pose_objects:vec![(0,42)],..Scene::default()};
        assert_eq!(cloud_object_key(&old,3),cloud_object_key(&new,7));
        old.sky_pose_objects[0].1=43;
        assert_ne!(cloud_object_key(&old,3),cloud_object_key(&new,7));
        assert_ne!(cloud_object_key(&Scene::default(),3),cloud_object_key(&Scene::default(),7));
    }

    #[test]
    fn native_cloud_submits_thirty_sheets_not_four_material_ranges() {
        use ao_scene::cloud::{CloudLayer, Clouds};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("cloud draw regression requires a GPU");
        let mut mesh = Clouds::mesh();
        mesh.submeshes = [(0, 2), (2, 4), (4, 27), (27, 30)].map(|(first, end)|
            Submesh { blend: Blend::AlphaBlend, two_sided: true,
                ..Submesh::new(Clouds::indices(10, first, end), None) }).into();
        r.upload(&Scene {
            meshes: vec![mesh], sky: vec![Instance { mesh: 0, transform: IDENTITY }],
            single_clouds: vec![CloudLayer { mesh: 0, altitude: 100.0, wind: [0.0; 3],
                storm: 0.0, thick: 0.0, sun_y: 1.0, light: [1.0; 3],
                light_direction: [0.0, 1.0, 0.0], ambient: [0.0; 3] }],
            ..Default::default()
        });
        let output = r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native cloud draw count"), size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        for (realistic, simple, expected) in [(true, true, 30), (false, true, 30), (false, false, 0)] {
            r.set_cloud_preferences(realistic, simple, false);
            let targets = Targets::new(&r, 64, 64);
            r.render(&output.create_view(&Default::default()), &targets,
                &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
            assert_eq!(r.stats.draw_calls, expected, "native sheet submissions, realistic={realistic} simple={simple}");
            assert_eq!(targets.backed, expected != 0, "only active faithful clouds require backed depth");
        }
        r.set_cloud_preferences(true, true, false);
        r.set_enhanced_settings(EnhancedSettings { ambient_occlusion: true, ..Default::default() });
        r.render(&output.create_view(&Default::default()), &Targets::new(&r, 64, 64),
            &Camera::look_at(Vec3::ZERO, Vec3::NEG_Z));
        assert_eq!(r.stats.draw_calls, 30, "native sky shader must remain compatible with MRT world attachments");
    }

    #[test]
    #[ignore = "exclusive GPU ICC 4x cloud precision benchmark; writes handoff evidence"]
    fn native_cloud_precision_four_sample_gpu_timing() -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        use std::{fmt::Write, path::PathBuf, time::Instant};
        const SIZE: (u32, u32) = (1920, 1200);
        const WARMUP: usize = 120;
        const SAMPLES: usize = 600;
        let out = PathBuf::from(std::env::var_os("AOMAC_GFX_SKY_TIMING_OUT")
            .context("set AOMAC_GFX_SKY_TIMING_OUT to a new handoff directory")?);
        let root = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?).join("aomac-handoff/gfx");
        ensure!(out.starts_with(&root) && !out.components().any(|part| part == std::path::Component::ParentDir),
            "output must be under {}", root.display());
        ensure!(!out.exists(), "refusing to overwrite {}", out.display());
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("status.txt"), "status=running\nvisual_acceptance=unexercised\n")?;
        let result = (|| -> anyhow::Result<()> {
            let dir = ao_gui::client_dir();
            let store = ao_rdb::RecordStore::open(&dir)?;
            let weather = ao_formats::weather::State::clear();
            ao_scene::crt_srand(1);
            let (scene, _) = ao_formats::playfield::load_playfield_report_with_weather(
                &store, &dir, 4582, 2648.69, 0, Some(weather))?;
            ensure!(!scene.single_clouds.is_empty(), "ICC snapshot must contain native cloud sheets");
            let camera = Camera { pos: Vec3::new(940.0, 48.72, -874.78), yaw: 3.765, pitch: -0.2, roll: 0.0 };
            let lens = ao_scene::Lens { fov: std::f32::consts::FRAC_PI_2, horizontal: true, near: 0.2, far: Some(800.0) };
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let modes = [CloudPrecisionProbe::Grouped, CloudPrecisionProbe::NativeDraws, CloudPrecisionProbe::BeautySky];
            let names = ["grouped4", "native30", "native30-beauty-sky"];
            let mut renderers = Vec::with_capacity(3);
            let mut outputs = Vec::with_capacity(3);
            let mut images = Vec::with_capacity(3);
            let mut manifest = format!("fixture=native_cloud_precision_four_sample\nsize={}x{}\nmsaa=4\nprofile=faithful\npf=4582\ngame_time=2648.69\ngame_day=0\nrenderer_time=0\ncrt_seed=1\ncloud_seed=1\nweather={weather:?}\ncamera={camera:?}\nlens={lens:?}\norder=ABC,CBA\nwarmup_per_variant={WARMUP}\nsamples_per_variant={SAMPLES}\ncpu_interval=render_encode_submit_excludes_gpu_wait\nvisual_acceptance=unexercised\n", SIZE.0, SIZE.1);
            for (index, mode) in modes.into_iter().enumerate() {
                let mut r = Renderer::new(&instance, None)?;
                r.cloud_precision_probe = mode;
                ao_scene::crt_srand(1);
                r.upload(&scene);
                r.set_lens(lens);
                r.set_msaa_samples(4);
                ensure!(r.msaa_samples() == 4, "true 4x MSAA unavailable; refusing a clamped benchmark");
                ensure!(!r.enhanced_settings.any_enabled(), "benchmark must remain faithful");
                r.time = 0.0;
                let target = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                    label: Some("faithful 4x cloud precision benchmark"),
                    size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
                });
                let view = target.create_view_tracked(&Default::default());
                let targets = Targets::new(&r, SIZE.0, SIZE.1);
                ensure!(targets.sample_count == 4, "render attachments must be true 4x");
                r.render(&view, &targets, &camera);
                images.push(texture_to_rgba(&r, &target, SIZE.0, SIZE.1)?);
                texture_to_png(&r, &target, SIZE.0, SIZE.1, &out.join(format!("{}-first.png", names[index])))?;
                writeln!(manifest, "{}_target_samples={}\n{}_draw_calls={}",
                    names[index], targets.sample_count, names[index], r.stats.draw_calls)?;
                r.set_gpu_timing(true)?;
                renderers.push(r);
                outputs.push((target, view, targets));
            }
            for index in [1, 2] {
                let count = images[0].iter().zip(&images[index]).filter(|(a, b)| a != b).count();
                let maximum = images[0].iter().zip(&images[index]).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
                writeln!(manifest, "{}_baseline_differing_bytes={count}\n{}_baseline_max_byte_error={maximum}", names[index], names[index])?;
            }
            std::fs::write(out.join("manifest.txt"), &manifest)?;
            let mut values: [Vec<(f64, f64)>; 3] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
            let mut csv = String::from("pair,variant,cpu_render_encode_submit_ms,gpu_frame_ms\n");
            for pair in 0..WARMUP + SAMPLES {
                for index in if pair % 2 == 0 { [0, 1, 2] } else { [2, 1, 0] } {
                    let r = &mut renderers[index];
                    let (_, view, targets) = &outputs[index];
                    let start = Instant::now();
                    r.render(view, targets, &camera);
                    let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
                    let gpu_ms = r.read_frame_gpu_ms()?.context("GPU timing unexpectedly disabled")?;
                    if pair >= WARMUP {
                        values[index].push((cpu_ms, gpu_ms));
                        writeln!(csv, "{},{},{cpu_ms:.9},{gpu_ms:.9}", pair - WARMUP, names[index])?;
                    }
                }
            }
            let mut gpu_p50 = [0.0; 3];
            for (index, measurements) in values.iter().enumerate() {
                for gpu in [false, true] {
                    let mut sorted: Vec<_> = measurements.iter().map(|&(cpu, frame)| if gpu { frame } else { cpu }).collect();
                    sorted.sort_by(f64::total_cmp);
                    for percentile in [50usize, 95, 99] {
                        let value = sorted[(sorted.len() * percentile).div_ceil(100) - 1];
                        writeln!(manifest, "{}_{}_p{percentile}={value:.9}", names[index], if gpu { "gpu_frame_ms" } else { "cpu_render_encode_submit_ms" })?;
                        if gpu && percentile == 50 { gpu_p50[index] = value; }
                    }
                }
            }
            writeln!(manifest, "beauty_sky_vs_grouped_gpu_p50_delta_ms={:.9}\nbeauty_sky_vs_native30_gpu_p50_delta_ms={:.9}\nwithin_0_1_ms_budget={}",
                gpu_p50[2] - gpu_p50[0], gpu_p50[2] - gpu_p50[1], gpu_p50[2] - gpu_p50[0] <= 0.1)?;
            std::fs::write(out.join("samples.csv"), csv)?;
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

    #[test]
    fn framebuffer_clear_preserves_encoded_bytes() {
        let c = framebuffer_clear([0.0, 0.5, 1.0]);
        assert_eq!((c.r, c.a), (0.0, 1.0));
        assert!((c.b - 1.0).abs() < 1e-6);
        assert!((c.g - 0.73535698).abs() < 1e-6);
    }

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
        s.textures.insert(key, Texture { source: Default::default(), width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 255] });
        let v = |x: f32, y: f32, uv: [f32; 2], color: [f32; 4]| Vertex { pos: [x, y, -10.0], uv, color, ..Default::default() };
        let c = [[1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0]];
        let vertices = vec![v(-9.0, -9.0, [0.0, 0.0], c[0]), v(9.0, -9.0, [1.0, 0.0], c[1]), v(9.0, 9.0, [1.0, 1.0], c[2]), v(-9.0, 9.0, [0.0, 1.0], c[3])];
        let sub = Submesh { two_sided: true, uv_scroll: scroll, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key)) };
        s.meshes.push(Mesh { vertices, submeshes: vec![sub] });
        s.sky.push(Instance { mesh: 0, transform: IDENTITY });
        s.sky_spin.extend(spin.map(|d| SkySpin { instance: 0, axis: [0.0, 0.0, 1.0], pivot: [0.0; 3], degrees_per_second: d }));
        s
    }

    #[test]
    fn horizon_camera_uv_tracks_live_eye_x_and_mirrored_z() {
        let mut sky = scene([0.0; 2], None);
        let texture = sky.textures.values_mut().next().unwrap();
        texture.height = 2;
        texture.rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        for vertex in &mut sky.meshes[0].vertices { vertex.uv = [0.25, 0.25]; vertex.color = [1.0; 4]; }
        let material = &mut sky.meshes[0].submeshes[0];
        material.uv_camera = [1.0 / 51200.0, 0.0, 0.0, -1.0 / 51200.0];
        assert_eq!(&mat_uniform(material)[24..28], &material.uv_camera);
        for (name, eye, expected) in [
            ("origin", [0.0, 0.0, 0.0], [255, 0, 0]),
            ("east", [25600.0, 0.0, 0.0], [0, 255, 0]),
            ("scene_south", [0.0, 0.0, 25600.0], [0, 0, 255]),
        ] {
            let path = std::env::temp_dir().join(format!("ao-horizon-camera-{}-{name}.png", std::process::id()));
            render_to_png_at(&sky, eye, [eye[0], eye[1], eye[2] - 1.0], 64, 64, &path, 0.0)
                .expect("horizon camera UV regression requires an offscreen GPU adapter");
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
            let mut pixels = vec![0; decoder.output_buffer_size()];
            decoder.next_frame(&mut pixels).unwrap();
            let center = &pixels[(32 * 64 + 32) * 4..][..3];
            assert!((0..3).all(|channel| (i32::from(center[channel]) - expected[channel]).abs() < 3),
                "{name}: live camera UV expected {expected:?}, got {center:?}");
        }
    }

    #[test]
    fn sky_material_pipes_preserve_existing_slots_and_cover_every_blend() {
        assert_eq!((ENV_PIPE, ENV_BLEND_PIPE, FADE_PIPE, SPRITE_PIPE, NATIVE_BLEND_PIPE), (12, 14, 16, 20, 22));
        assert_eq!(MESH_EFFECT_PIPE, MESH_EFFECT_STRIDE, "effect families include the independent material-state slots");
        assert_eq!(native_liquid::PIPE, SKY_LAST_PIPE + 1);
        let mut slots = std::collections::HashSet::new();
        for blend in [Blend::Opaque, Blend::AlphaTest, Blend::AlphaBlend, Blend::Additive, Blend::ZeroSourceColor, Blend::DestinationColorSourceColor, Blend::PremultipliedAlpha, Blend::SourceColorOne] {
            for two_sided in [false, true] {
                let sub = Submesh { blend, two_sided, ..Submesh::new(vec![], None) };
                let pipe = sky_material_pipe(&sub);
                assert!(pipe < native_liquid::PIPE, "{blend:?}: sky must not select a liquid pipeline");
                assert!(slots.insert(pipe), "{blend:?}: each sky blend/cull combination needs its own pipeline");
                if two_sided && (blend as usize) < 4 {
                    assert_eq!(pipe, SKY_PIPE + blend as usize);
                } else if two_sided && blend == Blend::SourceColorOne {
                    assert_eq!(pipe, SKY_GLOW_PIPE);
                }
            }
        }
        let (native, actors, _) = build_pipelines();
        for index in 0..=SKY_LAST_PIPE {
            let spec = native.spec(index);
            if spec.sky {
                assert!(!spec.actor && matches!(spec.effect, 0 | 14), "native sky must not select an emitting actor effect");
            }
        }
        for effect in [1, 4, 5, 6] {
            let spec = actors.spec(effect * MESH_EFFECT_STRIDE + SKY_PIPE);
            assert!(spec.sky && spec.actor, "actor sky aliases must retain Enhanced emission handling");
        }
    }

    #[test]
    fn native_sky_points_are_single_pixels_with_one_one_blend_and_source_fog() {
        let mut s = Scene {
            lens: Some(ao_scene::Lens { far: Some(800.0), ..Default::default() }),
            environment: Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3],
                fog_start: 0.0, fog_end: 800.0, ..default_environment(400.0) }),
            ..Scene::default()
        };
        s.meshes.push(Mesh {
            vertices: vec![Vertex { pos: [12.0, 17.0, -500.0], color: [1.0, 1.0, 1.0, 0.0], ..Default::default() }],
            submeshes: vec![Submesh { sky_points: true, sky_fog: true, sky_fog_color: Some([0.0; 3]),
                two_sided: true, blend: Blend::Additive, ..Submesh::new(vec![0], None) }],
        });
        s.sky.push(Instance { mesh: 0, transform: IDENTITY });
        assert_eq!(sky_material_pipe(&s.meshes[0].submeshes[0]), SKY_POINT_PIPE);
        for width in [64, 128] {
            for far in [800.0, 1600.0] {
                s.lens.as_mut().unwrap().far = Some(far);
                s.environment.as_mut().unwrap().fog_end = far;
                let path = std::env::temp_dir().join(format!("ao-render-{}-native-dot.png", std::process::id()));
                render_to_png_at(&s, [0.0; 3], [0.0, 0.0, -1.0], width, width, &path, 0.0)
                    .expect("native point regression requires an offscreen GPU adapter");
                let bytes = std::fs::read(&path).unwrap();
                let _ = std::fs::remove_file(path);
                let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
                let mut rgba = vec![0; decoder.output_buffer_size()];
                let info = decoder.next_frame(&mut rgba).unwrap();
                let colored: Vec<_> = rgba[..info.buffer_size()].as_chunks::<4>().0.iter().filter(|p| p[..3] != [0, 0, 0]).collect();
                assert_eq!(colored.len(), 1, "point size is one framebuffer pixel, independent of viewport/far");
                assert!(colored[0][..3].iter().all(|&c| (127..=129).contains(&c)),
                    "half-depth black source fog leaves half RGB, even with alpha zero (ONE/ONE): {:?}", colored[0]);
            }
        }
    }

    #[test]
    fn primitive_index_validation_preserves_points_and_whole_triangles() {
        let mut sub = Submesh { sky_points: true, ..Submesh::new((0..500).collect(), None) };
        let mut indices = Vec::new();
        append_primitive_indices(&mut indices, &sub, 500);
        assert_eq!(indices, sub.indices, "all 500 native points survive in source order");
        sub.indices = vec![0, 500, 499];
        indices.clear();
        append_primitive_indices(&mut indices, &sub, 500);
        assert_eq!(indices, [0, 499], "invalid points do not discard adjacent valid points");
        sub.sky_points = false;
        sub.indices = vec![0, 1, 2, 0, 500, 2, 3];
        indices.clear();
        append_primitive_indices(&mut indices, &sub, 500);
        assert_eq!(indices, [0, 1, 2], "triangle validation still drops whole invalid/incomplete triangles");
    }

    #[test]
    fn native_point_population_survives_initial_upload_and_sky_replacement() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("native population regression requires GPU");
        let points = |count: u32| Scene {
            meshes: vec![Mesh {
                vertices: vec![Vertex { pos: [0.0, 0.0, -500.0], ..Default::default() }; count as usize],
                submeshes: vec![Submesh { sky_points: true, two_sided: true, blend: Blend::Additive,
                    ..Submesh::new((0..count).collect(), None) }],
            }],
            sky: vec![Instance { mesh: 0, transform: IDENTITY }],
            ..Scene::default()
        };
        let all = points(500);
        let assert_population = |r: &Renderer, expected| {
            assert_eq!(r.sky.draws.len(), 1);
            assert_eq!(r.sky.draws[0].0.count, expected);
            assert_eq!(r.sky.draws[0].0.pipe, SKY_POINT_PIPE);
        };
        r.upload(&all);
        assert_population(&r, 500);
        let rebuilds = r.sky_rebuilds();
        r.set_sky(&all);
        assert_population(&r, 500);
        assert_eq!(r.sky_rebuilds(), rebuilds, "same-topology replacement uses the live fast path");
        r.set_sky(&points(1));
        assert_population(&r, 1);
        r.set_sky(&all);
        assert_population(&r, 500);
        r.upload(&all);
        assert_population(&r, 500);
    }

    #[test]
    fn dot_counter_steps_per_frame_not_elapsed_time_or_sky_snapshot() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("native counter regression requires GPU");
        let s = Scene::default();
        r.upload(&s);
        let targets = Targets::new(&r, 8, 8);
        let output = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let view = output.create_view_tracked(&Default::default());
        let camera = Camera::look_at(Vec3::ZERO, -Vec3::Z);
        let mut expected = 0.0_f32;
        for time in [0.0, 500.0, 500.0] {
            r.seek(time);
            assert_eq!(r.sky_dot_counter, expected, "seeking time is not an FXS frame");
            r.set_sky(&s);
            assert_eq!(r.sky_dot_counter, expected, "resource refresh retains counter history");
            r.render(&view, &targets, &camera);
            expected = (expected + 0.01) % 1.0;
            assert_eq!(r.sky_dot_counter, expected);
        }
        r.sky_dot_counter = 0.995;
        r.render(&view, &targets, &camera);
        assert!((r.sky_dot_counter - 0.005).abs() < 0.000001);
        r.upload(&s);
        assert_eq!(r.sky_dot_counter, 0.0, "new scene resets the FXS object's counter");
    }

    /// Randy 0x10041ede defaults to CULL_CW (2); only explicit CULL_NONE (1) disables it.
    #[test]
    fn sky_backfaces_cull_unless_material_explicitly_disables_culling() {
        let mut sky = scene([0.0; 2], None);
        sky.meshes[0].submeshes[0].texture = None;
        for vertex in &mut sky.meshes[0].vertices {
            vertex.color = [1.0, 0.0, 0.0, 1.0];
        }
        let mut empty = sky.clone();
        empty.sky.clear();
        let shot = |s: &Scene| pixels(s, 0.0, "sky-culling").expect("sky culling regression requires an offscreen GPU adapter");
        let background = shot(&empty);
        // HorizonClouds 227651 inherits culling; rim 227745 explicitly sets CULL_NONE.
        for blend in [Blend::AlphaBlend, Blend::SourceColorOne] {
            sky.meshes[0].submeshes[0].blend = blend;
            sky.meshes[0].submeshes[0].two_sided = false;
            let front = shot(&sky);
            assert_ne!(front, background, "{blend:?}: front faces must remain visible");
            for triangle in sky.meshes[0].submeshes[0].indices.as_chunks_mut::<3>().0 {
                triangle.swap(1, 2);
            }
            assert_eq!(shot(&sky), background, "{blend:?}: default sky material must cull back faces");
            sky.meshes[0].submeshes[0].two_sided = true;
            assert_eq!(shot(&sky), front, "{blend:?}: CULL_NONE must retain back faces");
            for triangle in sky.meshes[0].submeshes[0].indices.as_chunks_mut::<3>().0 {
                triangle.swap(1, 2);
            }
        }
    }

    #[test]
    fn sky_fog_scales_with_scenery_distance_but_not_local_fog_end() {
        let mut s = scene([0.0; 2], None);
        s.meshes[0].submeshes[0].texture = None;
        s.meshes[0].submeshes[0].sky_fog = true;
        for v in &mut s.meshes[0].vertices {
            v.pos = v.pos.map(|c| c * 75.0); // eye depth = 75% of normalized far; lateral range differs
            v.color = [1.0, 0.0, 0.0, 1.0];
        }
        let mut previous = None;
        for scenery in [1.0, 2.0] {
            let far = 800.0 * scenery;
            s.lens = Some(ao_scene::Lens { far: Some(far), ..Default::default() });
            s.environment = Some(ao_scene::Environment {
                sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 0.0, fog_end: far,
                ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0,
            });
            s.fog_model = None;
            let Some(clear) = pixels(&s, 0.0, "sky_fog_scenery") else { return };
            let clear = centre(&clear);
            assert!((63..=65).contains(&clear), "75% native gamma-space fog leaves 25% red: {clear}");
            if let Some(previous) = previous {
                assert_eq!(clear, previous, "scenery 1x/2x must retain the same relative sky fog");
            }
            previous = Some(clear);
            s.fog_model = Some(ao_scene::FogModel {
                far, volumes: vec![ao_scene::FogVolume {
                    pos: [0.0; 3], color: [0.0; 3], density: 0.25, radius: 100.0, room: None,
                }], ..Default::default()
            });
            let Some(dense) = pixels(&s, 0.0, "sky_fog_volume") else { return };
            assert!(centre(&dense) + 50 < clear, "a local fog volume must shorten fog end, not sky distance");
        }
    }

    #[test]
    fn sky_source_color_one_fogs_source_without_darkening_black_texels() {
        let mut s = scene([0.0; 2], None);
        let key = s.meshes[0].submeshes[0].texture.unwrap();
        s.textures.get_mut(&key).unwrap().rgba = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let sub = &mut s.meshes[0].submeshes[0];
        sub.blend = Blend::SourceColorOne;
        sub.texture_clamp = true;
        sub.sky_fog = true;
        sub.sky_fog_color = Some([0.0; 3]);
        s.lens = Some(ao_scene::Lens { far: Some(800.0), ..Default::default() });
        s.environment = Some(ao_scene::Environment {
            sky_color: [0.02; 3], fog_color: [0.8; 3], fog_start: 0.0, fog_end: 800.0,
            ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0,
        });
        for v in &mut s.meshes[0].vertices {
            v.pos = v.pos.map(|c| c * 50.0); // eye depth = 50% of normalized far
            v.color = [1.0; 4];
        }
        let shot = |s: &Scene, name: &str| pixels(s, 0.0, name).expect("source fog regression requires an offscreen GPU adapter");
        let mut background = s.clone();
        background.sky.clear();
        let destination = centre(&shot(&background, "sky_source_fog_background"));
        assert!(destination > 0, "nonblack destination must expose a darkening overlay");
        for (u, white, name) in [(-1.0, false, "sky_fog_border"), (0.25, false, "sky_fog_black"), (0.75, true, "sky_fog_white")] {
            for v in &mut s.meshes[0].vertices {
                v.uv = [u, 0.5];
            }
            let actual = centre(&shot(&s, name));
            if white {
                // Fog white to 0.5 in gamma space, then SRCCOLOR/ONE adds 0.5² to destination.
                let expected = f32::from(destination) + 0.25 * 255.0;
                assert!((f32::from(actual) - expected).abs() <= 2.0, "source fog must precede self-multiply: got {actual}, expected {expected}");
            } else {
                assert_eq!(actual, destination, "{name}: fogged black source must leave destination untouched");
            }
        }
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

    /// Leak regression: `LiveSky` calls `set_sky` about twice a second for the whole time a zone is outdoors, every
    /// time with a scene holding the same sky mesh/material topology but new per-tick vertex data (sun/stars/clouds
    /// moving) -- confirmed live at ICC (`AOMAC_GPU_STATS=1`) to be the dominant source of a steady, never-zero
    /// per-second GPU buffer/bind-group churn even while otherwise idle. After warm-up, further ticks must stop
    /// creating buffers or falling through to a full rebuild.
    #[test]
    fn live_sky_ticks_reuse_buffers_after_warmup() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("resource regression requires GPU");
        let tick = |renderer: &mut Renderer, n: i32| {
            let mut s = scene([0.5, 0.0], None);
            for v in &mut s.meshes[0].vertices {
                v.pos[2] -= f32::from(n as i16) * 0.01; // a moving sky: same topology/materials, new positions
            }
            renderer.set_sky(&s);
        };
        for n in 0..30 {
            tick(&mut renderer, n);
        }
        let (rebuilds, creates) = (renderer.sky_rebuilds(), renderer.sky_buffer_creates());
        assert!(rebuilds <= 1, "set_sky fell through to a full rebuild {rebuilds} times for an unchanged mesh/material set");
        for n in 30..60 {
            tick(&mut renderer, n);
        }
        assert_eq!(renderer.sky_rebuilds(), rebuilds, "steady-state live sky ticks triggered further full rebuilds");
        assert_eq!(renderer.sky_buffer_creates(), creates, "steady-state live sky ticks created {} more GPU buffers instead of reusing them", renderer.sky_buffer_creates() - creates);
    }

    /// Audit item 2: `Renderer::new` defaults to anisotropy 1x (retail; randy31 `FUN_10041ede` sets max anisotropy 1,
    /// `docs/screens.md` "max anisotropy 1"). The aomac Enhanced options page can raise it. Regression for the bug where
    /// Enhanced-page Texture Sharpening only took effect after the next zone load: both samplers live in the shared
    /// `globals_bg` bind group (group 0), not per-material, so `set_anisotropy` must change rendering immediately for
    /// materials already uploaded, without touching their bind groups (`renderer.sky.mats` must not grow) or panicking
    /// from a stale bind group referencing a dropped sampler.
    #[test]
    fn anisotropy_defaults_to_retail_1x_and_applies_live_without_rebuilding_materials() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("anisotropy regression requires GPU");
        assert_eq!(renderer.anisotropy, 1, "retail default must be 1x, not the former hardcoded 16x");
        renderer.upload(&scene([0.5, 0.0], None));
        let mats_before = renderer.sky.mats.len();
        assert!(mats_before > 0, "the test scene must upload at least one sky material for this regression to be meaningful");
        let render_frame = |r: &mut Renderer| {
            let targets = Targets::new(r, 64, 64);
            let out = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: r.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            r.render(&out.create_view_tracked(&Default::default()), &targets, &Camera::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO));
        };
        render_frame(&mut renderer);
        renderer.set_anisotropy(16); // no upload() in between: the materials above must pick this up live
        assert_eq!(renderer.anisotropy, 16);
        assert_eq!(renderer.sky.mats.len(), mats_before, "live anisotropy must not rebuild any material bind group");
        render_frame(&mut renderer); // would panic if a material still referenced the dropped 1x sampler
        renderer.set_anisotropy(1);
        assert_eq!(renderer.anisotropy, 1);
        assert_eq!(renderer.sky.mats.len(), mats_before);
        render_frame(&mut renderer);
    }

    /// MSAA-option decision: `Renderer::new` defaults to 1 sample (off, matching retail -- no AA option exists anywhere in the
    /// client's GUI/text/twk data). `set_msaa_samples` rebuilds every pipeline at the new `MultisampleState` and the renderer
    /// must still draw a full frame both with MSAA off (the `Targets::msaa == None` fast path, no resolve target) and with it on.
    #[test]
    fn msaa_samples_defaults_off_and_set_msaa_samples_rebuilds_pipelines() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("MSAA regression requires GPU");
        assert_eq!(renderer.msaa_samples(), 1, "retail default must be 1 sample (off), matching the MSAA-option decision");
        renderer.upload(&scene([0.5, 0.0], None));
        let render_frame = |r: &mut Renderer, w: u32, h: u32| {
            let targets = Targets::new(r, w, h);
            let out = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: r.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            r.render(&out.create_view_tracked(&Default::default()), &targets, &Camera::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO));
        };
        render_frame(&mut renderer, 64, 64); // MSAA off: Targets::msaa is None, no resolve target
        renderer.set_msaa_samples(4);
        assert!(renderer.msaa_samples() > 1, "the adapter must support at least 2x/4x MSAA for this regression to be meaningful");
        render_frame(&mut renderer, 64, 64); // the caller rebuilt Targets at the new sample count
        renderer.set_msaa_samples(1);
        assert_eq!(renderer.msaa_samples(), 1);
        render_frame(&mut renderer, 64, 64);
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
        let frame = |transform, parts: Vec<[[f32; 4]; 4]>, skin: Option<Vec<Vertex>>, always| ao_scene::ActorFrame { id: 1, model: 7, transform, parts, skin: skin.map(std::sync::Arc::new), always, alpha: 1.0, ..Default::default() };
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
        actor_shot_frame(world, model, at, ao_scene::ActorFrame { alpha, ..Default::default() }, name)
    }

    fn actor_shot_frame(world: &Scene, model: Scene, at: [f32; 3], frame: ao_scene::ActorFrame, name: &str) -> Option<[u8; 3]> {
        let bytes = actor_frame_png(world, model, at, frame, name)?;
        let mut r = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        let o = (info.width as usize * (info.height as usize / 2) + info.width as usize / 2) * 4;
        Some([buf[o], buf[o + 1], buf[o + 2]])
    }

    fn actor_frame_png(world: &Scene, model: Scene, at: [f32; 3], frame: ao_scene::ActorFrame, name: &str) -> Option<Vec<u8>> {
        let path = std::env::temp_dir().join(format!("ao-render-env-{}-{name}.png", std::process::id()));
        let t = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [at[0], at[1], at[2], 1.0]];
        let f = ao_scene::ActorFrame { id: 1, model: 7, transform: t, ..frame };
        render_to_png_actors(world, &[(7, model)], vec![f], [0.0; 3], [0.0, 0.0, -1.0], 64, 64, &path, 0.0).ok()?;
        let bytes = std::fs::read(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        Some(bytes)
    }
    #[test]
    fn native_texture_interpolates_encoded_bytes_before_modulation() {
        let key = TextureKey { rdb_type: 1, id: 1 };
        let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
            emissive: [1.0; 3], texture_clamp: true,
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key))
        });
        model.textures.insert(key, Texture {
            source: ao_scene::TextureSource::Generated, width: 2, height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
        });
        // Constant midpoint UV keeps LOD at zero: this tests GPU interpolation,
        // not the CPU mip builder. sRGB decode/filter/re-encode gives ~188.
        for vertex in &mut model.meshes[0].vertices { vertex.uv = [0.5, 0.5]; }
        let pixel = actor_shot(&Scene::default(), model, [0.0, 0.0, -5.0], "native-gamma-filter")
            .expect("native filtering regression requires an offscreen GPU adapter");
        assert!(pixel.iter().all(|&c| (c as i16 - 128).abs() <= 1),
            "D3D7 interpolates encoded black/white to 128, not linear-light 188: {pixel:?}");
    }

    #[test]
    fn native_ground_composite_samples_packed_gamma_bytes() {
        let key = TextureKey { rdb_type: 0, id: 73 };
        let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
            emissive: [1.0; 3], texture_clamp: true,
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key))
        });
        model.textures.insert(key, Texture {
            source: ao_scene::TextureSource::GroundComposite565, width: 8, height: 8,
            rgba: (0..64).flat_map(|_| [123, 127, 251, 255]).collect(),
        });
        let pixel = actor_shot(&Scene::default(), model, [0.0, 0.0, -5.0], "native-ground-packed")
            .expect("native ground pixel regression requires an offscreen GPU adapter");
        assert!(pixel.into_iter().zip([123i16, 125, 255]).all(|(actual, expected)|
            (actual as i16 - expected).abs() <= 1), "native RGB565 expansion: {pixel:?}");
    }

    #[test]
    fn native_texture_clamp_uses_distinct_cached_sampler_materials() {
        let key=TextureKey {rdb_type:1,id:1};
        // Both coplanar draws must execute: opaque depth writes would hide the second sampler.
        let sub=Submesh {blend:Blend::AlphaBlend,emissive:[1.0;3],..Submesh::new(vec![0,1,2,0,2,3],Some(key))};
        assert!(!sub.texture_clamp);
        let mut model=actor_quad([0.0,0.0,1.0],sub);
        model.textures.insert(key,Texture { source: Default::default(), width:2,height:1,rgba:vec![255,0,0,255,0,255,0,255] });
        for v in &mut model.meshes[0].vertices {v.uv=[1.25,0.5];}
        let world=Scene::default();
        let wrap=actor_shot(&world,model.clone(),[0.0,0.0,-5.0],"sampler-wrap").expect("sampler regression requires GPU");
        model.meshes[0].submeshes[0].texture_clamp=true;
        let clamp=actor_shot(&world,model.clone(),[0.0,0.0,-5.0],"sampler-clamp").expect("sampler regression requires GPU");
        assert!(wrap[0]>wrap[1],"WRAP samples the repeated red texel: {wrap:?}");
        assert!(clamp[1]>clamp[0],"CLAMP samples the edge green texel: {clamp:?}");
        let mut repeat=model.meshes[0].submeshes[0].clone();repeat.texture_clamp=false;
        model.meshes[0].submeshes.insert(0,repeat);
        let mixed=actor_shot(&world,model,[0.0,0.0,-5.0],"sampler-mixed").expect("sampler regression requires GPU");
        assert_eq!(mixed,clamp,"same texture/uniform but distinct native address states must not share a bind group");
    }


    #[test]
    fn actor_part_uv_transform_matches_authored_vertex_uvs() {
        let key = TextureKey { rdb_type: 1, id: 1 };
        let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
            emissive: [1.0; 3], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key))
        });
        model.textures.insert(key, Texture { source: Default::default(), width: 2, height: 2,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255] });
        for (v, uv) in model.meshes[0].vertices.iter_mut().zip([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]) {
            v.uv = uv;
        }
        let world = Scene::default();
        let shot = |model, uvs, name| actor_frame_png(&world, model, [0.0, 0.0, -5.0],
            ao_scene::ActorFrame { part_uvs: uvs, ..Default::default() }, name)
            .expect("authored UV regression requires an offscreen GPU adapter");
        let base = shot(model.clone(), vec![], "part-uv-default");
        assert_eq!(base, shot(model.clone(), vec![[1.0, 1.0, 0.0, 0.0]], "part-uv-identity"));
        let transform = [0.5, 0.75, 0.2, 0.1];
        let actual = shot(model.clone(), vec![transform], "part-uv-instance");
        let mut reference = model;
        for v in &mut reference.meshes[0].vertices {
            v.uv = [v.uv[0] * transform[0] + transform[2], v.uv[1] * transform[1] + transform[3]];
        }
        assert_ne!(base, actual, "authored UV animation changes the texture mapping");
        assert_eq!(actual, shot(reference, vec![], "part-uv-reference"));
    }

    #[test]
    fn native_holo_mesh_uses_two_textures_and_effect_local_timer() {
        let world = Scene {
            environment: Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 100.0, fog_end: 200.0, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 0.0 }),
            ..Default::default()
        };
        let mut model = actor_quad([0.0, 0.0, 1.0], Submesh::new(vec![0, 1, 2, 0, 2, 3], None));
        model.textures.insert(TextureKey { rdb_type: 1, id: 0x28022 }, Texture { source: Default::default(), width: 1, height: 2, rgba: vec![32, 0, 0, 255, 128, 0, 0, 255], });
        model.textures.insert(TextureKey { rdb_type: 1, id: 0x28023 }, Texture { source: Default::default(), width: 2, height: 1, rgba: vec![0, 32, 0, 255, 0, 128, 0, 255], });
        let shot = |elapsed| actor_shot_frame(&world, model.clone(), [0.0, 0.0, -5.0],
            ao_scene::ActorFrame { rendering_effect: 1, rendering_effect_time: elapsed, ..Default::default() },
            &format!("native-holo-{elapsed}")).expect("Holo regression requires an offscreen GPU adapter");
        let a = shot(0.0);
        let b = shot(2.5);
        assert!(a[0] > 0 && a[1] > 0, "both native textures contribute: {a:?}");
        assert_ne!(a, b, "effect-local timer scrolls the two texture stages");
        assert_eq!(a, shot(0.0), "timer restart is deterministic");
    }

    #[test]
    fn native_mesh_delta_states_render_lit_unlit_fog_and_material_preservation() {
        let world = Scene {
            environment: Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 0.0, fog_end: 1.0, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 0.0 }),
            ..Default::default()
        };
        let model = actor_quad([0.0, 0.0, 1.0], Submesh {
            emissive: [0.25_f32.powf(2.2); 3], two_sided: false,
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        });
        let shot = |effect| actor_shot_frame(&world, model.clone(), [0.0, 0.0, -5.0],
            ao_scene::ActorFrame { rendering_effect: effect, alpha: 0.5, ..Default::default() },
            &format!("native-mesh-{effect}")).expect("mesh mode regression requires an offscreen GPU adapter");
        let base = shot(0);
        assert_eq!(base, shot(3), "mode 3 clears depth, not the isolated mesh's material");
        assert_eq!(base, shot(7), "ZBIAS must not change an isolated mesh's colour");
        let lit = shot(4);
        let unlit = shot(5);
        assert!(lit[0] > base[0], "mode 4 disables fog: {base:?}, {lit:?}");
        assert!(unlit[0] > lit[0] + 50, "mode 5 disables lighting: {lit:?}, {unlit:?}");
        assert_eq!(unlit, shot(6), "native modes 5 and 6 own identical delta states");
        assert!(shot(2)[0] > base[0], "Space retains lighting and disables fog");
        let mut reversed = model.clone();
        for triangle in reversed.meshes[0].submeshes[0].indices.as_chunks_mut::<3>().0 {
            triangle.swap(1, 2);
        }
        let back = actor_shot_frame(&world, reversed, [0.0, 0.0, -5.0],
            ao_scene::ActorFrame { rendering_effect: 5, alpha: 0.5, ..Default::default() },
            "native-mesh-backface").unwrap();
        assert_eq!(unlit, back, "native transparent modes disable face culling");
    }

    #[test]
    fn native_mesh_zbias_and_clear_draw_over_existing_world_depth() {
        let model = actor_quad([0.0, 0.0, 1.0], Submesh {
            base_color: [0.0, 1.0, 0.0, 1.0], emissive: [1.0; 3],
            // Exercise ZBIAS against explicit LESS; the native default is
            // LESSEQUAL and already accepts coplanar geometry without bias.
            depth_func: Some(2),
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        });
        let mut world = model.clone();
        world.meshes[0].submeshes[0].base_color = [1.0, 0.0, 0.0, 1.0];
        world.environment = Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 100.0, fog_end: 200.0, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 0.0 });
        let mut transform = IDENTITY;
        transform[3][2] = -5.0;
        world.instances.push(Instance { mesh: 0, transform });
        let shot = |effect, z| actor_shot_frame(&world, model.clone(), [0.0, 0.0, z],
            ao_scene::ActorFrame { rendering_effect: effect, ..Default::default() },
            &format!("native-mesh-depth-{effect}")).expect("mesh depth regression requires an offscreen GPU adapter");
        assert!(shot(0, -5.0)[0] > 240, "authored LESS rejects the ordinary coplanar actor");
        assert!(shot(7, -5.0)[1] > 240, "native ZBIAS1 brings coplanar actor forward");
        assert!(shot(3, -6.0)[1] > 240, "native mode 3 clears the preceding world depth");
    }

    #[test]
    fn actor_material_overrides_preserve_base_and_emissive_gamma_space() {
        let linear = |rgb: [f32; 3]| rgb.map(|c| c.powf(2.2));
        let sub = Submesh {
            emissive: linear([0.2, 0.3, 0.4]), specular: linear([0.1, 0.2, 0.3]), shininess: 8.0,
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        };
        let model = actor_quad([0.0, 0.0, 1.0], sub.clone());
        let world = Scene {
            environment: Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 100.0, fog_end: 200.0, ambient: [0.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 0.0, 1.0], sun_specular: 0.0 }),
            ..Default::default()
        };
        let shot = |frame, name| actor_shot_frame(&world, model.clone(), [0.0, 0.0, -5.0], frame, name);
        let base = actor_frame_png(&world, model.clone(), [0.0, 0.0, -5.0], ao_scene::ActorFrame::default(), "material-none").expect("material regression requires an offscreen GPU adapter");
        let explicit = ao_scene::ActorFrame {
            emissive: Some(sub.emissive), specular: Some(sub.specular), specular_power: Some(sub.shininess),
            ..Default::default()
        };
        assert_eq!(base, actor_frame_png(&world, model.clone(), [0.0, 0.0, -5.0], explicit, "material-base").unwrap(), "None must retain every pixel of the model material");
        let mut static_world = world.clone();
        static_world.meshes = model.meshes.clone();
        let mut transform = IDENTITY;
        transform[3][2] = -5.0;
        static_world.instances.push(Instance { mesh: 0, transform });
        let path = std::env::temp_dir().join(format!("ao-render-material-world-{}.png", std::process::id()));
        render_to_png(&static_world, [0.0; 3], [0.0, 0.0, -1.0], 64, 64, &path).unwrap();
        let static_png = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(base, static_png, "None must match every pixel of the unchanged world material shader");
        let gamma = [0.25, 0.5, 0.75];
        let override_frame = ao_scene::ActorFrame { emissive: Some(linear(gamma)), ..Default::default() };
        let pixel = shot(override_frame, "material-emissive").unwrap();
        for (actual, expected) in pixel.into_iter().zip(gamma) {
            assert!((f32::from(actual) - expected * 255.0).abs() <= 2.0, "D3D7 gamma-space emissive: {pixel:?}");
        }
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

    #[test]
    fn sprite_cutout_retains_edge_alpha_at_near_and_far_distances() {
        let model = |alpha: u8| {
            let key = TextureKey { rdb_type: 0, id: 1 };
            let sub = Submesh { blend: Blend::AlphaBlend, sprite_alpha_test: true, emissive: [1.0; 3], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key)) };
            assert_eq!(material_pipe(&sub), SPRITE_PIPE);
            let mut scene = actor_quad([0.0, 0.0, 1.0], sub);
            scene.textures.insert(key, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![255, 0, 0, alpha] });
            scene
        };
        for distance in [2.0, 12.0] {
            let shot = |alpha| actor_shot(&Scene::default(), model(alpha), [0.0, 0.0, -distance], &format!("sprite-{distance}-{alpha}"));
            let Some(keyed) = shot(0) else { return };
            assert_eq!(shot(30).unwrap(), keyed, "ALPHAFUNC greater rejects the boundary");
            let edge = shot(128).unwrap();
            let solid = shot(255).unwrap();
            assert!(edge[0] > keyed[0] && edge[0] < solid[0], "filtered alpha must blend, not become opaque: {keyed:?}, {edge:?}, {solid:?}");
            assert_ne!(shot(31).unwrap(), keyed, "coverage above ALPHAREF survives");
        }
    }
    #[test]
    fn native_default_less_equal_preserves_coplanar_world_and_actor_draws() {
        // Randy 10041f98 sets ZFUNC=LESSEQUAL. Ordinary RTriMesh and CAT
        // base draws inherit it unless a material explicitly authors ZFUNC.
        for (depth_func, expected) in [(None, [0, 0, 255]), (Some(2), [255, 0, 0]), (Some(4), [0, 0, 255])] {
            let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
                depth_func, lighting_enabled: false, base_color: [1.0, 0.0, 0.0, 1.0],
                ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
            });
            let mut second = model.meshes[0].submeshes[0].clone();
            second.base_color = [0.0, 0.0, 1.0, 1.0];
            model.meshes[0].submeshes.push(second);
            let mut world = model.clone();
            let mut transform = IDENTITY;
            transform[3][2] = -3.0;
            world.instances.push(ao_scene::Instance { mesh: 0, transform });
            let world_pixel = actor_shot(&world, Scene::default(), [0.0; 3], &format!("native-depth-world-{depth_func:?}"))
                .expect("native inherited depth regression requires an offscreen GPU adapter");
            let actor_pixel = actor_shot(&Scene::default(), model, [0.0, 0.0, -3.0], &format!("native-depth-actor-{depth_func:?}"))
                .expect("native inherited depth regression requires an offscreen GPU adapter");
            assert_eq!(world_pixel, expected, "ordinary world inherited/authored ZFUNC {depth_func:?}");
            assert_eq!(actor_pixel, expected, "CAT base inherited/authored ZFUNC {depth_func:?}");
        }
    }
    #[test]
    fn native_rtti_mode_two_depth_uses_visual_provenance_not_skinning() {
        let red = |projection_pass| actor_quad([0.0, 0.0, 1.0], Submesh {
            projection_pass, depth_func: Some(2), depth_write: Some(false),
            lighting_enabled: false, base_color: [1.0, 0.0, 0.0, 1.0],
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        });
        let blue = || actor_quad([0.0, 0.0, 1.0], Submesh {
            lighting_enabled: false, base_color: [0.0, 0.0, 1.0, 1.0],
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        });
        for projection_pass in [false, true] {
            let mut world = blue();
            let mut transform = IDENTITY;
            transform[3][2] = -3.0;
            world.instances.push(ao_scene::Instance { mesh: 0, transform });
            let frame = ao_scene::ActorFrame { rendering_effect: 2, ..Default::default() };
            let pixel = actor_shot_frame(&world, red(projection_pass), [0.0, 0.0, -3.0], frame,
                &format!("rtti-mode-two-compare-{projection_pass}")).unwrap();
            assert_eq!(pixel, if projection_pass { [255, 0, 0] } else { [0, 0, 255] },
                "only RTriMesh mode 2 replaces authored LESS with LESSEQUAL");

            let mut model = red(projection_pass);
            model.meshes[0].submeshes[0].render_priority = Some(1);
            let mut behind = blue().meshes.remove(0);
            for vertex in &mut behind.vertices { vertex.pos[2] -= 0.2; }
            // Native queues prepend separate visuals within one list: explicit
            // lists make the farther quad draw after the tested depth writer.
            behind.submeshes[0].render_priority = Some(2);
            model.meshes.push(behind);
            // CPU-skinned and rigid models have no native skin buffer; the
            // archived visual class still determines the mode-2 branch.
            assert!(model.native_skin.is_none());
            let cpu_vertices = std::sync::Arc::new(model.meshes[0].vertices.clone());
            for skin in [None, Some(cpu_vertices)] {
                let frame = ao_scene::ActorFrame { rendering_effect: 2, skin, ..Default::default() };
                let pixel = actor_shot_frame(&Scene::default(), model.clone(), [0.0, 0.0, -3.0], frame,
                    &format!("rtti-mode-two-write-{projection_pass}")).unwrap();
                assert_eq!(pixel, if projection_pass { [255, 0, 0] } else { [0, 0, 255] },
                    "only RTriMesh mode 2 overrides disabled ZWRITE, including CPU fallback");
            }
        }
    }



    #[test]
    fn native_environment_redraw_inherits_material_depth() {
        let base = TextureKey { rdb_type: 0, id: 88 };
        let env = TextureKey { rdb_type: 0, id: 89 };
        // ONE/ONE adds to the framebuffer: give the alpha-discard case an
        // explicit black background rather than adding to the default sky.
        let world = Scene { environment: Some(Environment { sky_color: [0.0; 3], ..default_environment(1000.0) }), ..Default::default() };
        let reflective = |depth_func, write, discard| {
            let mut scene = actor_quad([0.0, 0.0, 1.0], Submesh {
                depth_func, depth_write: Some(write), lighting_enabled: false,
                alpha_test: Some(128), env_texture: Some(env),
                base_color: [0.0, 0.0, 0.0, 1.0],
                ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(base))
            });
            scene.textures.insert(base, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![0, 0, 0, if discard { 0 } else { 255 }] });
            scene.textures.insert(env, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![64, 0, 0, 255] });
            scene
        };
        let blue_quad = |scene: &mut Scene, z: f32| {
            let mesh = &mut scene.meshes[0];
            let first = mesh.vertices.len() as u32;
            let vertices: Vec<_> = mesh.vertices[..4].iter().map(|vertex| {
                let mut vertex = *vertex;
                vertex.pos[2] += z;
                vertex
            }).collect();
            mesh.vertices.extend(vertices);
            mesh.submeshes.push(Submesh {
                lighting_enabled: false, base_color: [0.0, 0.0, 1.0, 1.0],
                ..Submesh::new([0, 1, 2, 0, 2, 3].map(|i| first + i).to_vec(), None)
            });
        };
        for depth_func in [None, Some(2), Some(4), Some(8)] {
            for write in [false, true] {
                let name = format!("{depth_func:?}-{write}");
                for effect in [0, 2] {
                    let frame = ao_scene::ActorFrame { rendering_effect: effect, ..Default::default() };
                    let pixel = actor_shot_frame(&world, reflective(depth_func, write, false), [0.0, 0.0, -3.0], frame, &format!("env-coplanar-{name}-{effect}"))
                        .expect("native env depth regression requires an offscreen GPU adapter");
                    assert_eq!(pixel, if depth_func == Some(2) && write { [0; 3] } else { [64, 0, 0] }, "coplanar base/env inherit ZFUNC/ZWRITE {name}/effect{effect}");
                    // Base ALPHATEST discards, while the env texture survives.
                    // Only the env pass can occlude this later, farther blue quad.
                    let mut scene = reflective(depth_func, write, true);
                    blue_quad(&mut scene, -0.2);
                    let frame = ao_scene::ActorFrame { rendering_effect: effect, ..Default::default() };
                    let pixel = actor_shot_frame(&world, scene, [0.0, 0.0, -3.0], frame, &format!("env-write-{name}-{effect}")).unwrap();
                    assert_eq!(pixel, if write { [64, 0, 0] } else { [0, 0, 255] }, "env alone inherits ZWRITE {name}/effect{effect}");
                }
                let mut scene = reflective(depth_func, write, true);
                blue_quad(&mut scene, 0.2);
                scene.meshes[0].submeshes.swap(0, 1);
                let pixel = actor_shot(&world, scene, [0.0, 0.0, -3.0], &format!("env-occluded-{name}")).unwrap();
                assert_eq!(pixel, if depth_func == Some(8) { [64, 0, 255] } else { [0, 0, 255] }, "env retains authored ALWAYS rather than replacing it with LESSEQUAL {name}");
            }
        }
    }

    #[test]
    fn native_alpha_edge_keeps_environment_stage_derivatives() {
        // Fixed-function stages precede ALPHATEST, including samples used only
        // by a later stage. The pixel oracle also covers divergent derivative quads.
        let shader = include_str!("shader.wgsl");
        assert!(shader.find("sample_clamped(env_tex, i.env_uv)").unwrap()
            < shader.find("if native_alpha_test() && !native_alpha_pass(alpha)").unwrap());
        let base = TextureKey { rdb_type: 0, id: 80 };
        let env = TextureKey { rdb_type: 0, id: 81 };
        let mut model = actor_quad([0.0, 0.0, 1.0], Submesh {
            texture_clamp: true, lighting_enabled: false,
            alpha_test: Some(128), stage1_env_texture: Some(env),
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(base))
        });
        for vertex in &mut model.meshes[0].vertices {
            let [x, y, _] = vertex.pos;
            vertex.uv = [0.53 + (x + y) * 0.4, 0.5];
            vertex.normal = [x * 2.0, y * 2.0, 0.1];
        }
        model.textures.insert(base, Texture { source: Default::default(), width: 2, height: 1,
            rgba: vec![0, 0, 0, 0, 0, 0, 0, 255] });
        model.textures.insert(env, Texture { source: Default::default(), width: 64, height: 64,
            rgba: (0..64).flat_map(|y| (0..64).flat_map(move |x| {
                let value = if (x + y) % 2 == 0 { 0 } else { 255 };
                [value, value, value, 255]
            })).collect() });
        let shot = |model, name| {
            let png = actor_frame_png(&Scene::default(), model, [0.0, 0.0, -4.0], Default::default(), name)
                .expect("native alpha/environment regression requires an offscreen GPU adapter");
            let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
            let mut rgba = vec![0; reader.output_buffer_size()];
            let info = reader.next_frame(&mut rgba).unwrap();
            rgba.truncate(info.buffer_size());
            rgba
        };
        let mut full = model.clone();
        full.meshes[0].submeshes[0].alpha_test = None;
        let full = shot(full, "env-before-alpha-full");
        let mut mask = model.clone();
        mask.meshes[0].submeshes[0].stage1_env_texture = None;
        for pixel in mask.textures.get_mut(&base).unwrap().rgba.as_chunks_mut::<4>().0 { pixel[..3].fill(255); }
        let mask = shot(mask, "env-before-alpha-mask");
        let actual = shot(model, "env-before-alpha-edge");
        let background = &mask[..3];
        let mut surviving = 0;
        let mut rejected = 0;
        for ((actual, full), mask) in actual.as_chunks::<4>().0.iter().zip(full.as_chunks::<4>().0).zip(mask.as_chunks::<4>().0) {
            if mask[..3] == [255; 3] {
                surviving += 1;
                assert_eq!(&actual[..3], &full[..3], "discarded neighbours must not change surviving env samples");
            } else {
                rejected += usize::from(&full[..3] != background);
                assert_eq!(&actual[..3], background, "ALPHATEST must reject the whole material, not only its base stage");
            }
        }
        assert!(surviving > 0 && rejected > 0, "the alpha edge must split visible environment geometry");
    }

    #[test]
    fn native_cat_coplanar_environment_pass_keeps_invariant_positions() {
        assert!(include_str!("shader.wgsl").contains("@builtin(position) @invariant clip"));
        let base = TextureKey { rdb_type: 0, id: 82 };
        let env = TextureKey { rdb_type: 0, id: 83 };
        let mut model = actor_quad([0.2, 0.4, 1.0], Submesh {
            emissive: [1.0; 3], env_texture: Some(env),
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(base))
        });
        for vertex in &mut model.meshes[0].vertices {
            let [x, y, _] = vertex.pos;
            vertex.pos = [x * 1.17 + 0.023, y * 0.91 - 0.017, x * 0.31 + y * 0.43];
        }
        model.textures.insert(base, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![0, 0, 0, 255] });
        model.textures.insert(env, Texture { source: Default::default(), width: 1, height: 1, rgba: vec![64, 32, 16, 255] });
        let mut reference = model.clone();
        reference.meshes[0].submeshes[0].env_texture = None;
        reference.textures.get_mut(&base).unwrap().rgba = vec![64, 32, 16, 255];
        let shot = |model, name| actor_frame_png(&Scene::default(), model, [0.0, 0.0, -4.0], Default::default(), name)
            .expect("native coplanar environment regression requires an offscreen GPU adapter");
        assert_eq!(shot(model, "cat-coplanar-env"), shot(reference, "cat-coplanar-reference"),
            "the LEQUAL environment redraw must cover every base-pass sample, including tilted depth gradients");
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
        model_tex.textures.insert(env, Texture { source: Default::default(), width: 2, height: 2, rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255] });
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
        // D3D7 blends stored gamma bytes: half red + half blue is 128, not linear-space 188.
        assert!((120..136).contains(&behind[0]) && (120..136).contains(&behind[2]), "red actor under half-transparent blue water: {behind:?}");
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

    #[test]
    fn prelit_light_saturates_per_vertex_before_interpolation() {
        // Pixel (32,32) samples x = 10*tan(30°)/64 on this constant-depth quad.
        let t = (2.0 + 10.0 * (std::f32::consts::PI / 6.0).tan() / 64.0) / 4.0;
        for lamps in [false, true] {
            let mut s = lit_quad(2.0, [0.0; 3], 1.0, 0.0);
            if lamps {
                s.lights[0].color = [0.5_f32.powf(2.2); 3];
            } else {
                s.lights.clear();
                s.environment.as_mut().unwrap().ambient = [0.5_f32.powf(2.2); 3];
            }
            for v in &mut s.meshes[0].vertices {
                let c = if v.pos[0] < 0.0 { 1.0 } else { 0.0 };
                v.color = [c, c, c, 1.0];
            }
            for prelit in [false, true] {
                s.meshes[0].submeshes[0].prelit = prelit;
                let added = if lamps { 0.5 * 10.0 / 108.0_f32.sqrt() } else if prelit { 0.8 * 0.5 } else { 0.5 };
                let expected = 255.0 * if prelit { (1.0 - t) + t * added } else { (1.0 - t) * added };
                let name = format!("prelit-gouraud-{lamps}-{prelit}");
                let Some(png) = pixels(&s, 0.0, &name) else { return };
                // Exact pixel-centre barycentrics above; allow two UNORM levels for raster/rounding differences.
                assert!((f32::from(centre(&png)) - expected).abs() <= 2.0, "{name}: {} expected {expected}", centre(&png));
                if prelit {
                    let mut world = s.clone();
                    world.instances.clear();
                    let Some(actor) = actor_shot_frame(&world, s.clone(), [0.0; 3], ao_scene::ActorFrame::default(), &name) else { return };
                    assert!((f32::from(actor[0]) - expected).abs() <= 2.0, "actor {name}: {actor:?} expected {expected}");
                }
            }
        }
    }
    /// `SPECULARENABLE` materials add `specular * (N.H)^power` after the texture stage, also on a black diffuse material.
    #[test]
    fn specular_term_is_added_after_the_texture_stage() {
        let (Some(on), Some(off)) = (pixels(&lit_quad(2.0, [0.0; 3], 0.0, 1.0), 0.0, "spec_on"), pixels(&lit_quad(2.0, [0.0; 3], 0.0, 0.0), 0.0, "spec_off")) else { return };
        // light at the eye: H = L, N.H = N.L = 10 / sqrt(108) = 0.962 at the corners -> 0.962^10 = 0.68
        assert_eq!(centre(&off), 0);
        assert!(centre(&on) > 150, "specular {}", centre(&on));
    }

    #[test]
    fn horizon_directionals_light_world_diffuse_and_specular_with_a_live_palette() {
        let mut scene = lit_quad(2.0, [0.0; 3], 1.0, 0.0);
        scene.lights.clear();
        let light = ao_scene::DirectionalLight {
            direction: [0.0, 0.3, 1.0],
            cycle: Some(ao_scene::LightCycle {
                palette: (0..10).map(|i| [1.0, 0.5, 0.04].map(|c| c * if i % 2 == 0 { 1.0 } else { 0.8 })).collect(),
                phase: 0.1, rate: 0.4, intensity: 0.45,
            }),
            ..Default::default()
        };
        scene.directional_lights.push(light);
        let (Some(low), Some(high)) = (pixels(&scene, 0.0, "horizon_low"), pixels(&scene, 0.25, "horizon_high")) else { return };
        // +Z-facing normal: N.L=1/sqrt(1+.3²); native palette red .36 -> .45.
        assert!((86..=90).contains(&centre(&low)), "gamma diffuse {}", centre(&low));
        assert!((108..=112).contains(&centre(&high)), "animated gamma diffuse {}", centre(&high));
        scene.meshes[0].submeshes[0].base_color = [0.0, 0.0, 0.0, 1.0];
        scene.meshes[0].submeshes[0].specular = [1.0; 3];
        let specular = pixels(&scene, 0.25, "horizon_specular").unwrap();
        assert!(centre(&specular) > 45, "directional specular must survive a black diffuse material");
        scene.directional_lights.clear();
        let off = pixels(&scene, 0.25, "horizon_removed").unwrap();
        assert_eq!(centre(&off), 0, "no stale directional light after scene replacement");
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

    #[test]
    fn world_and_water_use_vertex_eye_depth_not_radial_fragment_fog() {
        for liquid in [false, true] {
            let mut s = lit_quad(20.0, [0.0; 3], 1.0, 0.0);
            s.lights.clear();
            let sub = &mut s.meshes[0].submeshes[0];
            sub.emissive = [1.0; 3];
            sub.liquid = liquid;
            let env = s.environment.as_mut().unwrap();
            env.fog_start = 0.0;
            env.fog_end = 20.0;
            // Corners exceed range20 but eye-depth10: native vertex fog is half.
            let Some(png) = pixels(&s, 0.0, if liquid { "water_eye_fog" } else { "world_eye_fog" }) else { return };
            assert!((126..=129).contains(&centre(&png)), "linear vertex eye fog (liquid={liquid}): {}", centre(&png));
        }
    }

    #[test]
    fn sky_emissive_saturates_before_texture_gain_and_clips_at_native_far() {
        let mut s = scene([0.0; 2], None);
        let key = s.meshes[0].submeshes[0].texture.unwrap();
        s.textures.get_mut(&key).unwrap().rgba = vec![64, 64, 64, 255, 64, 64, 64, 255];
        for v in &mut s.meshes[0].vertices { v.color = [0.0, 0.0, 0.0, 1.0]; }
        let sub = &mut s.meshes[0].submeshes[0];
        sub.emissive = [1.0; 3];
        sub.base_color = [2.0_f32.powf(2.2), 2.0_f32.powf(2.2), 2.0_f32.powf(2.2), 1.0];
        let Some(png) = pixels(&s, 0.0, "sky_emissive_gain") else { return };
        assert!((126..=130).contains(&centre(&png)), "black diffuse plus emissive1, then texture64*2: {}", centre(&png));
        s.environment = Some(ao_scene::Environment { sky_color: [0.0; 3], ..default_environment(100.0) });
        for v in &mut s.meshes[0].vertices { v.pos = v.pos.map(|c| c * 101.0); }
        let Some(clipped) = pixels(&s, 0.0, "sky_far_clip") else { return };
        assert_eq!(centre(&clipped), 0, "normalized depth1010 must be clipped by the native far plane");
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
