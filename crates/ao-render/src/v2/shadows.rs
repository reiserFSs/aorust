//! Sun cascades + Moon A (frame slot 2) and their resolve plugin (`shadows.wgsl`, group 1).
//!
//! * Sun: v1's `CascadeFrame` (3 texel-snapped cascades, 20/60/180 m style splits). Moon A: one 1024^2 cascade fitted like a sun
//!   cascade over the whole range.
//! * Every cascade has two depth layers. The STATIC layer holds world casters only (never an actor: it is drawn from the static
//!   instance list, which has no actor code path) and is cached: it is re-rendered only when its content signature (cascade matrix,
//!   the casters inside it, their transforms and mesh revisions) changes, checked on a per-cascade cadence (near every frame, the
//!   others time-sliced; the moon at most 4 Hz). The receiver samples a static layer with the matrix it was rendered with, so a
//!   late refresh never moves a shadow. The DYNAMIC layer is cleared every frame and holds admitted actors only: the actor shadow
//!   fragment shader discards every fragment whose actor slot is 0 in `Admission::visibility()` (current-frame GPU revocation).
//! * Receiver: `min` of the two layers, 3x3 PCF, bounded slope bias, 10% cascade crossfade, fade-out toward the last split and
//!   `horizon_fade` of the light elevation.
//! * Timer labels: `shadows` (all passes of this module, first begin to last end).
use bytemuck::Zeroable;
use super::admission::Admission;
use super::resolve::Plugin;

use crate::sun_shadows::{CascadeFrame, ShadowUniform};
use crate::Renderer;
use glam::{Mat4, Vec3, Vec4};
use std::ops::Range;

const SUN_CASCADES: usize = 3;
const LAYERS: usize = SUN_CASCADES + 1; // + moon A
const SUN_RES: u32 = 2048;
/// Dynamic (actor) layers are cleared and stored every frame, so they are a quarter of the cached world layers' size (4 cm texels in cascade 0).
const SUN_DYN_RES: u32 = 1024;
const MOON_RES: u32 = 1024;
const MOON_DYN_RES: u32 = 1024;
/// The sun cascades never reach beyond this, whatever the lens far plane (as v1).
const SHADOW_FAR: f32 = 180.0;
/// Extra metres of slack baked into each STATIC layer's fit (sun cascades 0..2, moon): the layer stays valid while the camera's
/// frustum slice remains inside it, so walking re-renders a cascade only after about this much travel (the slack costs ~7% texel size).
const GUARD: [f32; LAYERS] = [1.5, 4.0, 12.0, 6.0];
/// Shortest time between two re-renders of a layer whose frustum slice left it (moon A: at most 4 Hz).
const MIN_INTERVAL: [f64; LAYERS] = [0.0, 0.0, 0.0, 0.25];
/// Layers holding time-animated casters (scrolling alpha, wind) are refreshed at this rate.
const ANIMATED_INTERVAL: f64 = 0.25;
/// A stored layer is aligned with the light while it is within 0.05 degrees of it (the sun moves about that per 10 s).
const ALIGNED_COS: f32 = 0.999_999_6;

/// Cached-layer bookkeeping: what the layer was last rendered with.
#[derive(Default, Debug)]
pub(super) struct StaticCache {
    /// Content signature of the rendered casters; `None` = never rendered.
    signature: Option<u64>,
    /// Hash of the whole static world when it was last compared.
    world: u64,
    animated: bool,
    rendered: f64,
}

impl StaticCache {
    /// Whether the layer must be re-rendered now. `covered`: the stored matrix still contains the camera's frustum slice and is
    /// aligned with the light. `world`: cheap hash of the whole static world (instances, visibility, revisions). The expensive
    /// per-layer `signature_now` (casters inside the stored frustum) is only evaluated when `world` changed.
    pub(super) fn needs_render(&mut self, covered: bool, world: u64, time: f64, min_interval: f64, signature_now: impl FnOnce() -> u64) -> bool {
        let Some(signature) = self.signature else { return true };
        let elapsed = time - self.rendered;
        if !covered { return elapsed >= min_interval || elapsed < 0.0; }
        if self.animated && (elapsed >= ANIMATED_INTERVAL || elapsed < 0.0) { return true; }
        if self.world == world { return false; }
        if signature_now() == signature { self.world = world; return false; }
        true
    }

    pub(super) fn rendered(&mut self, signature: u64, world: u64, animated: bool, time: f64) {
        *self = Self { signature: Some(signature), world, animated, rendered: time };
    }
}

/// One world caster instance as the static layer sees it. There is deliberately no actor variant: actors cannot enter the cache.
pub(super) struct Caster {
    pub draw: u32,
    pub slot: u32,
    pub revision: u64,
    pub material: u32,
    pub pipe: u32,
    pub transform: [[f32; 4]; 4],
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self { Self(0xcbf2_9ce4_8422_2325) }
    fn word(&mut self, w: u32) { self.0 = (self.0 ^ u64::from(w)).wrapping_mul(0x0000_0100_0000_01b3); }
    fn mat(&mut self, m: &[[f32; 4]; 4]) { for v in m.as_flattened() { self.word(v.to_bits()); } }
}

/// Content signature of one static layer: the world revision plus every caster inside its frustum (not its matrix: whether the layer
/// still covers the view is a separate question).
pub(super) fn static_signature(world_revision: u64, casters: impl Iterator<Item = Caster>) -> u64 {
    let mut hash = Fnv::new();
    hash.word(world_revision as u32);
    hash.word((world_revision >> 32) as u32);
    for caster in casters {
        for w in [caster.draw, caster.slot, caster.revision as u32, (caster.revision >> 32) as u32, caster.material, caster.pipe] { hash.word(w); }
        hash.mat(&caster.transform);
    }
    hash.0
}

/// The eight corners of the camera frustum slice `start..end` (view depth).
fn slice_corners(view: &View, start: f32, end: f32) -> [Vec3; 8] {
    let forward = view.forward.normalize_or_zero();
    let right = forward.cross(view.up).normalize_or_zero();
    let up = right.cross(forward);
    let tan = (view.fovy * 0.5).tan();
    std::array::from_fn(|i| {
        let depth = if i < 4 { start } else { end };
        let (sx, sy) = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)][i % 4];
        view.eye + forward * depth + right * (sx * tan * depth * view.aspect) + up * (sy * tan * depth)
    })
}

/// Whether every corner lies inside the layer's depth map with a 3-texel border (PCF) and inside its depth range.
fn contains(matrix: &Mat4, corners: &[Vec3], resolution: u32) -> bool {
    let border = 6.0 / resolution as f32;
    corners.iter().all(|&c| {
        let p = matrix.project_point3(c);
        p.x.abs() <= 1.0 - border && p.y.abs() <= 1.0 - border && p.z > 0.0 && p.z < 1.0
    })
}

/// Texel-snapped orthographic fit of the camera frustum slice `start..end` seen from `toward_light` (same construction as
/// `CascadeFrame::new`, for a single slice). `guard` adds metres around the bounding sphere.
#[allow(clippy::too_many_arguments)]
pub(super) fn fit_slice(eye: Vec3, forward: Vec3, fovy: f32, aspect: f32, start: f32, end: f32, reach: f32, toward_light: Vec3, resolution: u32, guard: f32) -> Option<Mat4> {
    if !(eye.is_finite() && forward.is_finite() && toward_light.is_finite() && fovy > 0.0 && fovy < std::f32::consts::PI
        && aspect > 0.0 && start >= 0.0 && end > start && resolution > 0) || toward_light.length_squared() < 1e-8 { return None; }
    let light = toward_light.normalize();
    let step = std::f32::consts::TAU / 16384.0;
    let azimuth = (light.x.atan2(light.z) / step).round() * step;
    let elevation = (light.y.clamp(-1.0, 1.0).asin() / step).round() * step;
    let (sin_az, cos_az) = azimuth.sin_cos();
    let (sin_el, cos_el) = elevation.sin_cos();
    let dir = Vec3::new(sin_az * cos_el, sin_el, cos_az * cos_el);
    let forward = forward.try_normalize()?;
    let right = Vec3::new(cos_az, 0.0, -sin_az);
    let basis = Mat4::from_cols(right.extend(0.0), dir.cross(right).extend(0.0), dir.extend(0.0), Vec4::W).transpose();
    let center = eye + forward * ((start + end) * 0.5);
    let half_height = (fovy * 0.5).tan() * end;
    let half_width = half_height * aspect;
    let radius = ((end - start).powi(2) * 0.25 + half_height.powi(2) + half_width.powi(2)).sqrt() + guard;
    let radius = ((radius * 16.0).ceil() / 16.0) * (1.0 + 4.0 / resolution as f32);
    let texel = 2.0 * radius / resolution as f32;
    let light_center = basis.transform_point3(center);
    let x = (light_center.x / texel).round() * texel;
    let y = (light_center.y / texel).round() * texel;
    let depth_center = (light_center.z / texel).round() * texel;
    let z = reach + radius + texel;
    Some(Mat4::orthographic_rh(x - radius, x + radius, y - radius, y + radius, -depth_center - z, -depth_center + z) * basis)
}

/// What the camera and the lights are doing this frame.
pub(super) struct View {
    pub eye: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    pub fovy: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
    pub fog_end: f32,
    pub time: f32,
}

/// A directional light that casts shadows: toward the light, and its `horizon_fade` strength (> 0).
#[derive(Clone, Copy)]
pub(super) struct Light {
    pub dir: Vec3,
    pub strength: f32,
}

/// This frame's fit of one layer: the camera frustum slice it must cover (view depth), the light direction and the tight matrix the
/// per-frame actor layer is rendered with.
#[derive(Clone, Copy)]
struct Fit {
    start: f32,
    end: f32,
    dir: Vec3,
    tight: Mat4,
}

/// Mirror of `struct Receive` in `shadows.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Receive {
    sun_static: [[[f32; 4]; 4]; SUN_CASCADES],
    sun_dyn: [[[f32; 4]; 4]; SUN_CASCADES],
    moon_static: [[f32; 4]; 4],
    moon_dyn: [[f32; 4]; 4],
    splits: [f32; 4],
    fade: [f32; 4],
    flags: [u32; 4],
}

struct DepthArray {
    array: wgpu::TextureView,
    layers: Vec<wgpu::TextureView>,
}

fn depth_array(device: &wgpu::Device, label: &str, resolution: u32, layers: u32) -> DepthArray {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label), size: wgpu::Extent3d { width: resolution, height: resolution, depth_or_array_layers: layers },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: crate::DEPTH,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
    });
    DepthArray {
        array: texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() }),
        layers: (0..layers).map(|i| texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: i, array_layer_count: Some(1), ..Default::default() })).collect(),
    }
}

/// A static depth layer: its cache state, the matrix it was rendered with (sampled with this one) and the light it was lit from.
struct StaticLayer {
    cache: StaticCache,
    matrix: Mat4,
    dir: Vec3,
}

/// A caster pass target: matrices uniform + the bind group the v1 caster pipelines expect at group 3.
struct Target {
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

pub(super) struct Shadows {
    sun_static: DepthArray,
    sun_dyn: DepthArray,
    moon_static: DepthArray,
    moon_dyn: DepthArray,
    static_targets: [Target; LAYERS],
    dyn_targets: [Target; LAYERS],
    /// What each static layer was rendered with.
    layers: [StaticLayer; LAYERS],
    /// Dynamic layers that may hold actor depth and so must be cleared.
    dyn_dirty: [bool; LAYERS],
    commands: [Vec<(usize, Range<u32>)>; LAYERS],
    transforms: Vec<[[f32; 4]; 4]>,
    transform_buffer: Option<wgpu::Buffer>,
    receive: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pub(super) bind: wgpu::BindGroup,
    /// Diagnostic for the offline harness (`AOMAC_V2_SHADOW_RERENDER=<bit mask of layers>`, 0 = off): re-renders those static layers
    /// every frame to measure the walking worst case per layer.
    always_rerender: u32,
    /// Diagnostic (`AOMAC_V2_SHADOWS=off`): no shadow work at all, to measure what shadows add to the frame.
    off: bool,
}

fn layout_entry(binding: u32, ty: wgpu::BindingType, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None }
}

impl Shadows {
    pub(super) fn new(r: &Renderer, visibility: &wgpu::Buffer) -> Self {
        let device = &r.device;
        let sun_static = depth_array(device, "v2 sun cascades (static)", SUN_RES, SUN_CASCADES as u32);
        let sun_dyn = depth_array(device, "v2 sun cascades (actors)", SUN_DYN_RES, SUN_CASCADES as u32);
        let moon_static = depth_array(device, "v2 moon A (static)", MOON_RES, 1);
        let moon_dyn = depth_array(device, "v2 moon A (actors)", MOON_DYN_RES, 1);
        let target = || {
            let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 shadow caster matrices"), size: std::mem::size_of::<ShadowUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 shadow caster"), layout: &r.shadow_pipelines.caster_layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: visibility.as_entire_binding() },
            ] });
            Target { uniform, bind }
        };
        let compute = wgpu::ShaderStages::COMPUTE;
        let depth_array_entry = |binding| layout_entry(binding, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, compute);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 shadow receiver"), entries: &[
            layout_entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Receive>() as u64) }, compute),
            depth_array_entry(1), depth_array_entry(2), depth_array_entry(3), depth_array_entry(4),
            layout_entry(5, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), compute),
        ] });
        let receive = device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 shadow receiver"), size: std::mem::size_of::<Receive>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("v2 shadow compare"), compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, ..Default::default() });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 shadow receiver"), layout: &layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: receive.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&sun_static.array) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&sun_dyn.array) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&moon_static.array) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&moon_dyn.array) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&sampler) },
        ] });
        Self {
            sun_static, sun_dyn, moon_static, moon_dyn,
            static_targets: std::array::from_fn(|_| target()), dyn_targets: std::array::from_fn(|_| target()),
            layers: std::array::from_fn(|_| StaticLayer { cache: StaticCache::default(), matrix: Mat4::ZERO, dir: Vec3::ZERO }),
            dyn_dirty: [true; LAYERS], commands: Default::default(),
            transforms: Vec::new(), transform_buffer: None, receive, layout, bind,
            always_rerender: std::env::var("AOMAC_V2_SHADOW_RERENDER").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            off: std::env::var("AOMAC_V2_SHADOWS").is_ok_and(|v| v == "off"),
        }
    }

    /// The resolve plugin: defines `sun_visibility` / `moon_visibility` at group 1.
    pub(super) fn plugin(&self) -> Plugin<'_> {
        Plugin { wgsl: crate::shader_source::load("v2/shadows.wgsl", include_str!("shadows.wgsl")).into_owned(), layout: &self.layout }
    }

    fn layer_view(&self, layer: usize, dynamic: bool) -> &wgpu::TextureView {
        match (layer, dynamic) {
            (SUN_CASCADES, false) => &self.moon_static.layers[0],
            (SUN_CASCADES, true) => &self.moon_dyn.layers[0],
            (k, false) => &self.sun_static.layers[k],
            (k, true) => &self.sun_dyn.layers[k],
        }
    }

    /// Mirrors the static instance transforms into the caster instance stream (re-uploaded only when something moved).
    fn sync_transforms(&mut self, r: &Renderer) {
        let same = self.transforms.len() == r.gpu.insts.len() && self.transforms.iter().zip(&r.gpu.insts).all(|(a, b)| *a == b.m);
        if same && self.transform_buffer.is_some() { return; }
        self.transforms.clear();
        self.transforms.extend(r.gpu.insts.iter().map(|inst| inst.m));
        let bytes = (self.transforms.len().max(1) * 64) as u64;
        if self.transform_buffer.as_ref().is_none_or(|b| b.size() < bytes) {
            self.transform_buffer = Some(r.device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 static caster transforms"),
                size: bytes.next_power_of_two(), usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        }
        if let Some(buffer) = &self.transform_buffer { r.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.transforms)); }
    }

    /// Slot 2. `sun` / `moon`: the lights that cast shadows this frame (None = inactive; their layers are left alone).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, r: &Renderer, enc: &mut wgpu::CommandEncoder, view: &View, sun: Option<Light>, moon: Option<Light>,
        admission: &Admission) {
        if self.off { return; }

        let far = r.lens.far.unwrap_or(SHADOW_FAR).min(SHADOW_FAR).max(view.near + 0.01);
        let receiver_far = view.far.min(view.fog_end).min(far);
        let casters = || r.gpu.insts.iter().filter(|inst| !inst.hidden).map(|inst| (inst.center, inst.radius));
        let mut fits: [Option<Fit>; LAYERS] = [None; LAYERS];
        let mut receive = Receive::zeroed();
        let mut sun_frame = None;
        if let Some(light) = sun {
            sun_frame = CascadeFrame::new(view.eye, view.forward, view.up, view.fovy, view.aspect, view.near, far, light.dir, SUN_RES);
            if let Some(frame) = sun_frame.as_mut() {
                frame.include_caster_depth(casters());
                let splits = frame.uniform.splits;
                let mut start = splits[3];
                for k in 0..SUN_CASCADES {
                    fits[k] = Some(Fit { start, end: splits[k], dir: light.dir.normalize(), tight: Mat4::from_cols_array_2d(&frame.uniform.matrices[k]) });
                    start = splits[k] - (splits[k] - start) * 0.1;
                }
                receive.splits = splits;
                receive.fade[0] = light.strength;
                receive.fade[1] = splits[2].min(receiver_far).max(splits[1] + 0.01);
                receive.flags[2] = 1;
            }
        }
        if let Some(light) = moon {
            let range = receiver_far.max(view.near + 1.0);
            if let Some(tight) = fit_slice(view.eye, view.forward, view.fovy, view.aspect, view.near, range, range, light.dir, MOON_RES, 0.0) {
                fits[SUN_CASCADES] = Some(Fit { start: view.near, end: range, dir: light.dir.normalize(),
                    tight: crate::shadow_coverage::include_caster_depth(tight, casters()) });
                receive.fade[2] = light.strength;
                receive.fade[3] = range;
                receive.flags[3] = 1;
            }
        }

        self.refresh_static(r, enc, &fits, view, far, sun_frame.as_ref());
        let items = r.v2_has_actor_items();
        let current = fits.map(|fit| fit.map(|fit| fit.tight));
        self.draw_dynamic(r, enc, &current, sun_frame.as_ref(), admission, items);

        for (k, fit) in current.iter().take(SUN_CASCADES).enumerate() {
            receive.sun_static[k] = self.layers[k].matrix.to_cols_array_2d();
            receive.sun_dyn[k] = fit.unwrap_or(Mat4::ZERO).to_cols_array_2d();
        }
        receive.moon_static = self.layers[SUN_CASCADES].matrix.to_cols_array_2d();
        receive.moon_dyn = current[SUN_CASCADES].unwrap_or(Mat4::ZERO).to_cols_array_2d();
        receive.flags[0] = u32::from(items && receive.flags[2] != 0);
        receive.flags[1] = u32::from(items && receive.flags[3] != 0);
        r.queue.write_buffer(&self.receive, 0, bytemuck::bytes_of(&receive));
    }

    /// Cheap hash of everything that can change a static caster anywhere in the world (instances, visibility, revisions).
    fn world_hash(r: &Renderer) -> u64 {
        let mut hash = Fnv::new();
        for w in [r.point_world_revision as u32, (r.point_world_revision >> 32) as u32, u32::from(r.spaceships_shown), r.far_away.len() as u32,
            r.movers.len() as u32, r.gpu.insts.len() as u32, r.drawn_visuals.len() as u32] { hash.word(w); }
        for instance in &r.gpu.insts {
            hash.word(u32::from(instance.hidden));
            hash.mat(&instance.m);
        }
        // Order-independent: visual node states (doors, lifts) that hide or show a draw.
        let nodes = r.drawn_visuals.iter().fold(0u64, |acc, entry| {
            use std::hash::{Hash, Hasher};
            let mut each = std::hash::DefaultHasher::new();
            entry.hash(&mut each);
            acc ^ each.finish()
        });
        hash.word(nodes as u32);
        hash.word((nodes >> 32) as u32);
        hash.0
    }

    /// Casters of `layer` inside `matrix`' frustum (left in `self.commands[layer]`), their content signature and whether any of them
    /// is time-animated.
    fn gather(&mut self, r: &Renderer, layer: usize, matrix: &Mat4) -> (u64, bool) {
        self.world_commands(r, layer, &crate::frustum_planes(matrix));
        let animated = self.commands[layer].iter().any(|(draw, _)| {
            let draw = &r.gpu.opaque[*draw];
            draw.time_alpha || (draw.vegetation && (r.enhanced_settings.foliage_sway || r.enhanced_settings.foliage_push))
        });
        let signature = static_signature(r.point_world_revision, self.commands[layer].iter().flat_map(|(draw, range)| {
            let draw_state = &r.gpu.opaque[*draw];
            range.clone().map(move |slot| Caster {
                draw: *draw as u32, slot, revision: r.point_mesh_revisions.get(draw_state.mesh).copied().unwrap_or(0),
                material: draw_state.mat as u32, pipe: draw_state.pipe as u32, transform: r.gpu.insts[slot as usize].m,
            })
        }));
        (signature, animated)
    }

    /// Re-renders the static layers whose stored matrix no longer covers the camera's frustum slice (or the light moved), whose casters
    /// changed, or whose animated casters are due.
    fn refresh_static(&mut self, r: &Renderer, enc: &mut wgpu::CommandEncoder, fits: &[Option<Fit>; LAYERS], view: &View, far: f32, sun_frame: Option<&CascadeFrame>) {
        let world = Self::world_hash(r);
        let time = f64::from(view.time);
        let mut render = [false; LAYERS];
        for layer in 0..LAYERS {
            let Some(fit) = fits[layer] else { continue };
            let resolution = if layer < SUN_CASCADES { SUN_RES } else { MOON_RES };
            let stored = self.layers[layer].matrix;
            let covered = self.layers[layer].dir.dot(fit.dir) >= ALIGNED_COS && contains(&stored, &slice_corners(view, fit.start, fit.end), resolution);
            let mut cache = std::mem::take(&mut self.layers[layer].cache);
            let need = self.always_rerender >> layer & 1 != 0 || cache.needs_render(covered, world, time, MIN_INTERVAL[layer], || self.gather(r, layer, &stored).0);
            self.layers[layer].cache = cache;
            if !need { continue; }
            let reach = if layer < SUN_CASCADES { far } else { fit.end };
            let Some(matrix) = fit_slice(view.eye, view.forward, view.fovy, view.aspect, fit.start, fit.end, reach, fit.dir, resolution, GUARD[layer]) else { continue };
            let matrix = crate::shadow_coverage::include_caster_depth(matrix, r.gpu.insts.iter().filter(|inst| !inst.hidden).map(|inst| (inst.center, inst.radius)));
            let (signature, animated) = self.gather(r, layer, &matrix);
            let slot = &mut self.layers[layer];
            slot.matrix = matrix;
            slot.dir = fit.dir;
            slot.cache.rendered(signature, world, animated, time);
            render[layer] = true;
        }
        if !render.contains(&true) { return; }
        self.sync_transforms(r);
        let Some(instances) = self.transform_buffer.as_ref() else { return };
        for layer in (0..LAYERS).filter(|&l| render[l]) {
            let mut uniform = ShadowUniform::zeroed();
            if layer < SUN_CASCADES {
                if let Some(frame) = sun_frame { uniform.matrices = frame.uniform.matrices; }
                uniform.control[0] = layer as u32;
            }
            uniform.matrices[if layer < SUN_CASCADES { layer } else { 0 }] = self.layers[layer].matrix.to_cols_array_2d();
            r.queue.write_buffer(&self.static_targets[layer].uniform, 0, bytemuck::bytes_of(&uniform));
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("v2 static shadow layer"), color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: self.layer_view(layer, false),
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_bind_group(0, &r.globals_bg, &[]);
            pass.set_bind_group(3, &self.static_targets[layer].bind, &[]);
            pass.set_vertex_buffer(1, instances.slice(..));
            let (mut pipe, mut mesh, mut mat) = (usize::MAX, usize::MAX, usize::MAX);
            for (index, range) in &self.commands[layer] {
                let draw = &r.gpu.opaque[*index];
                let Some((vertices, indices)) = &r.gpu.meshes[draw.mesh] else { continue };
                let Some(Some(pipeline)) = r.shadow_pipelines.pipelines.get(&(false, draw.pipe, false)) else { continue };
                if pipe != draw.pipe { pass.set_pipeline(pipeline); pipe = draw.pipe; }
                if mesh != draw.mesh {
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    mesh = draw.mesh;
                }
                if mat != draw.mat { pass.set_bind_group(1, &r.gpu.mats[draw.mat], &[]); mat = draw.mat; }
                pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, range.clone());
            }
        }
    }

    /// Static world casters inside one frustum, in draw/instance order (consecutive instances of one draw coalesce).
    fn world_commands(&mut self, r: &Renderer, layer: usize, planes: &[Vec4; 6]) {
        self.commands[layer].clear();
        for (index, draw) in r.gpu.opaque.iter().enumerate() {
            if draw.count == 0 || !r.shadow_pipelines.pipelines.get(&(false, draw.pipe, false)).is_some_and(Option::is_some) { continue; }
            let vegetation = draw.vegetation && r.enhanced_settings.foliage_enabled();
            for slot in r.gpu.mesh_range[draw.mesh].clone() {
                let instance = &r.gpu.insts[slot];
                if instance.hidden || (!r.spaceships_shown && r.spaceship_slots[slot])
                    || r.far_away.iter().any(|run| run.slot == slot) || r.movers.iter().any(|run| run.slot == slot) { continue; }
                let key = draw.node.map(|(node, _)| crate::VisualKey::World(slot, node));
                if crate::visual_priority(draw, key, &r.drawn_visuals).is_some_and(|priority| priority < 0) { continue; }
                // Length-preserving foliage moves <= .895 of its height (v1 bound).
                let radius = if vegetation { instance.radius * 3.0 + 0.001 } else { instance.radius };
                if crate::shadow_coverage::admits_sphere(&[*planes], instance.center, radius) {
                    crate::shadow_pass::push_static_shadow_command(&mut self.commands[layer], index, slot as u32);
                }
            }
        }
    }

    /// The dynamic layers: cleared every frame (only when they hold something), then admitted-actor casters.
    fn draw_dynamic(&mut self, r: &Renderer, enc: &mut wgpu::CommandEncoder, current: &[Option<Mat4>; LAYERS], sun_frame: Option<&CascadeFrame>,
        admission: &Admission, items: bool) {
        for (layer, fit) in current.iter().enumerate() {
            let Some(matrix) = *fit else { continue };
            if !items && !self.dyn_dirty[layer] { continue; }
            if items {
                let mut uniform = ShadowUniform::zeroed();
                if layer < SUN_CASCADES {
                    if let Some(frame) = sun_frame { uniform.matrices = frame.uniform.matrices; }
                    uniform.control[0] = layer as u32;
                } else {
                    uniform.matrices[0] = matrix.to_cols_array_2d();
                }
                r.queue.write_buffer(&self.dyn_targets[layer].uniform, 0, bytemuck::bytes_of(&uniform));
            }
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("v2 actor shadow layer"), color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: self.layer_view(layer, true),
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            if items {
                pass.set_bind_group(0, &r.globals_bg, &[]);
                pass.set_bind_group(3, &self.dyn_targets[layer].bind, &[]);
                let pipelines = &r.shadow_pipelines.pipelines;
                r.v2_draw_actor_parts(&mut pass, admission.slots(), false, &|slot| pipelines.get(&(true, slot, false)).and_then(Option::as_ref).map(|p| p.get()));
            }
            self.dyn_dirty[layer] = items;
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use super::*;

    const FOVY: f32 = 1.0;
    const ASPECT: f32 = 1.5;

    fn view_at(eye: Vec3, forward: Vec3) -> View {
        View { eye, forward, up: Vec3::Y, fovy: FOVY, aspect: ASPECT, near: 0.1, far: 180.0, fog_end: 180.0, time: 0.0 }
    }

    /// Every corner of the camera frustum slice `start..end` inside the unit light cube.
    fn inside(matrix: &Mat4, eye: Vec3, forward: Vec3, start: f32, end: f32) -> bool {
        slice_corners(&view_at(eye, forward), start, end).iter().all(|&c| {
            let p = matrix.project_point3(c);
            p.x.abs() <= 1.0 && p.y.abs() <= 1.0 && (0.0..=1.0).contains(&p.z)
        })
    }

    #[test]
    fn sun_cascades_contain_their_frustum_slice_whatever_the_view_direction() {
        let sun = Vec3::new(0.3, 0.7, 0.4).normalize();
        for yaw in [0.0f32, 0.9, 2.4, -1.7] {
            let forward = Vec3::new(yaw.sin(), -0.1, -yaw.cos()).normalize();
            let eye = Vec3::new(120.0, 3.0, -40.0);
            let frame = CascadeFrame::new(eye, forward, Vec3::Y, FOVY, ASPECT, 0.1, 180.0, sun, SUN_RES).unwrap();
            let splits = frame.uniform.splits;
            let mut start = splits[3];
            for (k, &end) in splits.iter().take(SUN_CASCADES).enumerate() {
                let matrix = Mat4::from_cols_array_2d(&frame.uniform.matrices[k]);
                assert!(inside(&matrix, eye, forward, start, end), "cascade {k} misses its slice at yaw {yaw}");
                start = end - (end - start) * 0.1;
            }
        }
    }

    #[test]
    fn moon_fit_matches_the_v1_cascade_construction_and_contains_the_whole_range() {
        let moon = Vec3::new(-0.4, 0.5, 0.6).normalize();
        let (eye, forward) = (Vec3::new(10.0, 2.0, 30.0), Vec3::new(0.2, 0.0, -1.0).normalize());
        let frame = CascadeFrame::new(eye, forward, Vec3::Y, FOVY, ASPECT, 0.1, 180.0, moon, MOON_RES).unwrap();
        // Same slice, no guard: identical matrix to v1's first cascade.
        let first = fit_slice(eye, forward, FOVY, ASPECT, 0.1, frame.uniform.splits[0], 180.0, moon, MOON_RES, 0.0).unwrap();
        for (a, b) in first.to_cols_array().iter().zip(frame.uniform.matrices[0].as_flattened()) { assert_eq!(a, b); }
        let whole = fit_slice(eye, forward, FOVY, ASPECT, 0.1, 150.0, 150.0, moon, MOON_RES, GUARD[3]).unwrap();
        assert!(inside(&whole, eye, forward, 0.1, 150.0));
        // Texel snapping: sub-texel camera motion does not move the grid.
        let moved = fit_slice(eye + Vec3::new(1e-4, 0.0, 0.0), forward, FOVY, ASPECT, 0.1, 150.0, 150.0, moon, MOON_RES, GUARD[3]).unwrap();
        assert_eq!(whole, moved);
        assert!(fit_slice(eye, forward, FOVY, ASPECT, 0.1, 150.0, 150.0, Vec3::ZERO, MOON_RES, 0.0).is_none());
    }

    fn caster(slot: u32) -> Caster {
        Caster { draw: 1, slot, revision: 4, material: 2, pipe: 3, transform: Mat4::IDENTITY.to_cols_array_2d() }
    }

    #[test]
    fn signature_tracks_revision_casters_and_transforms_only() {
        let base = static_signature(9, [caster(0), caster(1)].into_iter());
        assert_eq!(base, static_signature(9, [caster(0), caster(1)].into_iter()), "same static world, same signature");
        assert_ne!(base, static_signature(10, [caster(0), caster(1)].into_iter()), "world revision");
        assert_ne!(base, static_signature(9, [caster(0)].into_iter()), "caster set");
        let mut moved = caster(1);
        moved.transform[3][0] = 5.0;
        assert_ne!(base, static_signature(9, [caster(0), moved].into_iter()), "instance transform");
    }

    #[test]
    fn cache_survives_actor_activity_and_rerenders_on_content_or_coverage_change() {
        let signature = |revision| static_signature(revision, [caster(0)].into_iter());
        let mut cache = StaticCache::default();
        assert!(cache.needs_render(false, 7, 0.0, 0.0, || unreachable!()), "a layer that was never rendered renders at once");
        cache.rendered(signature(1), 7, false, 0.0);
        // 100 frames in which actors come, go and move: they are no input of the world hash, the stored matrix still covers the
        // view, so the layer is neither re-rendered nor even re-hashed.
        for frame in 1..100 { assert!(!cache.needs_render(true, 7, f64::from(frame), 0.0, || unreachable!("actors must not trigger a signature check"))); }
        // Something changed elsewhere in the world: same casters inside this layer -> kept (and the world hash remembered).
        assert!(!cache.needs_render(true, 8, 100.0, 0.0, || signature(1)));
        assert!(!cache.needs_render(true, 8, 101.0, 0.0, || unreachable!()));
        // A caster inside this layer changed (revision / transform): re-render.
        assert!(cache.needs_render(true, 9, 102.0, 0.0, || signature(2)));
        // The view left the stored matrix: re-render.
        assert!(cache.needs_render(false, 8, 103.0, 0.0, || unreachable!()));
    }

    #[test]
    fn moon_layer_refreshes_at_most_four_hertz_and_animated_layers_at_the_animated_rate() {
        let mut cache = StaticCache::default();
        cache.rendered(1, 1, false, 10.0);
        assert!(!cache.needs_render(false, 1, 10.2, MIN_INTERVAL[3], || 1), "moon layer, 0.2 s after its last render");
        assert!(cache.needs_render(false, 1, 10.25, MIN_INTERVAL[3], || 1));
        assert!(cache.needs_render(false, 1, 10.0, MIN_INTERVAL[0], || 1), "an uncovered sun layer re-renders at once");
        cache.rendered(1, 1, true, 20.0);
        assert!(!cache.needs_render(true, 1, 20.1, 0.0, || 1));
        assert!(cache.needs_render(true, 1, 20.25, 0.0, || 1), "animated casters refresh at 4 Hz");
        assert!(cache.needs_render(true, 1, 5.0, 0.0, || 1), "a clock that went backwards (scene reload) re-renders");
    }

    #[test]
    fn slack_keeps_a_walked_cascade_valid_and_a_long_walk_does_not() {
        let sun = Vec3::new(0.3, 0.7, 0.4).normalize();
        let frame = CascadeFrame::new(Vec3::ZERO, -Vec3::Z, Vec3::Y, FOVY, ASPECT, 0.1, 180.0, sun, SUN_RES).unwrap();
        let mut start = 0.1;
        for (k, &guard) in GUARD.iter().take(SUN_CASCADES).enumerate() {
            let end = frame.uniform.splits[k];
            let stored = fit_slice(Vec3::ZERO, -Vec3::Z, FOVY, ASPECT, start, end, 180.0, sun, SUN_RES, guard).unwrap();
            let covers = |eye: Vec3| contains(&stored, &slice_corners(&view_at(eye, -Vec3::Z), start, end), SUN_RES);
            assert!(covers(Vec3::ZERO), "cascade {k}");
            assert!(covers(Vec3::new(0.5 * guard, 0.0, 0.0)) && covers(Vec3::new(0.0, 0.0, -0.5 * guard)), "cascade {k}: half the slack");
            assert!(!covers(Vec3::new(end, 0.0, 0.0)), "cascade {k}: a walk of one cascade length must re-render");
            start = end - (end - start) * 0.1;
        }
    }

    #[test]
    fn receive_layout_matches_wgsl() {
        // 8 mat4 (sun static/dynamic, moon static/dynamic) + 3 vec4, 16-byte aligned for a uniform buffer.
        assert_eq!(std::mem::size_of::<Receive>(), 8 * 64 + 3 * 16);
        assert_eq!(std::mem::size_of::<Receive>() % 16, 0);
    }

    /// No shimmer under camera yaw/motion: every cascade matrix maps the world onto one fixed texel lattice, so a world point keeps its
    /// sub-texel position in the depth map whatever the view direction (the static layer rasterises identically after a refresh).
    #[test]
    fn cascade_texel_lattice_is_independent_of_yaw_and_position() {
        let sun = Vec3::new(0.3, 0.7, 0.4).normalize();
        let probes = [Vec3::new(3.0, 0.5, -7.0), Vec3::new(-41.0, 2.0, 12.5), Vec3::new(100.25, 9.0, 80.0)];
        let frame_at = |eye: Vec3, yaw: f32| CascadeFrame::new(eye, Vec3::new(yaw.sin(), -0.1, -yaw.cos()).normalize(), Vec3::Y, FOVY, ASPECT, 0.1, 180.0, sun, SUN_RES).unwrap();
        let texel_phase = |frame: &CascadeFrame, k: usize, p: Vec3| {
            let uv = Mat4::from_cols_array_2d(&frame.uniform.matrices[k]).project_point3(p).truncate() * 0.5 * SUN_RES as f32;
            uv - uv.floor()
        };
        let base = frame_at(Vec3::new(120.0, 3.0, -40.0), 0.3);
        for (eye, yaw) in [(Vec3::new(120.0, 3.0, -40.0), 1.9), (Vec3::new(123.7, 3.0, -38.1), -0.8), (Vec3::new(97.3, 5.0, -52.0), 3.0)] {
            let other = frame_at(eye, yaw);
            for k in 0..SUN_CASCADES {
                for p in probes {
                    let (a, b) = (texel_phase(&base, k, p), texel_phase(&other, k, p));
                    let d = (a - b).abs();
                    let d = d.min(Vec2::ONE - d);
                    assert!(d.max_element() < 0.02, "cascade {k} lattice moved by {d:?} at yaw {yaw}");
                }
            }
        }
    }
}
