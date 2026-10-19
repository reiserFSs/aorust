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
use super::timers::PassTimers;
use crate::sun_shadows::{CascadeFrame, ShadowUniform};
use crate::Renderer;
use glam::{Mat4, Vec3, Vec4};
use std::ops::Range;

const SUN_CASCADES: usize = 3;
const LAYERS: usize = SUN_CASCADES + 1; // + moon A
const SUN_RES: u32 = 2048;
/// Dynamic (actor) layers are cleared every frame, so they are cheaper than the cached world layers; texels stay under 3 cm in cascade 0.
const SUN_DYN_RES: u32 = 2048;
const MOON_RES: u32 = 1024;
const MOON_DYN_RES: u32 = 1024;
/// The sun cascades never reach beyond this, whatever the lens far plane (as v1).
const SHADOW_FAR: f32 = 180.0;
/// Extra metres around the moon fit: its static layer may be up to a quarter second (a few metres of walking) stale.
const MOON_GUARD: f32 = 6.0;

/// How often a static layer re-checks its content signature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Cadence {
    /// Every `period` frames, offset by `phase` so that the layers never refresh in the same frame.
    Frames { period: u64, phase: u64 },
    /// At most this often (seconds).
    Seconds(f64),
}

const CADENCE: [Cadence; LAYERS] = [
    Cadence::Frames { period: 1, phase: 0 },
    Cadence::Frames { period: 2, phase: 1 },
    Cadence::Frames { period: 4, phase: 2 },
    Cadence::Seconds(0.25),
];

/// Cached-layer bookkeeping. The layer is valid for exactly the signature it was last rendered with.
#[derive(Default, Debug)]
pub(super) struct StaticCache {
    signature: Option<u64>,
    checked: f64,
}

impl StaticCache {
    /// Whether the signature has to be (re)computed now: never rendered, or its cadence slot came up.
    pub(super) fn due(&self, cadence: Cadence, frame: u64, time: f64) -> bool {
        self.signature.is_none() || match cadence {
            Cadence::Frames { period, phase } => (frame + phase) % period == 0,
            Cadence::Seconds(interval) => time - self.checked >= interval || time < self.checked,
        }
    }

    /// Records a check with the layer's current content `signature`; true when the layer must be re-rendered.
    pub(super) fn update(&mut self, signature: u64, time: f64) -> bool {
        self.checked = time;
        self.signature.replace(signature) != Some(signature)
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
    /// Seconds bits for draws whose alpha coverage animates; other draws never depend on time.
    pub time: Option<u32>,
}

/// Content signature of one static layer: the cascade matrix plus every caster inside it.
pub(super) fn static_signature(matrix: &Mat4, world_revision: u64, casters: impl Iterator<Item = Caster>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut word = |w: u32| hash = (hash ^ u64::from(w)).wrapping_mul(0x0000_0100_0000_01b3);
    for value in matrix.to_cols_array() { word(value.to_bits()); }
    word(world_revision as u32);
    word((world_revision >> 32) as u32);
    for caster in casters {
        for w in [caster.draw, caster.slot, caster.revision as u32, (caster.revision >> 32) as u32, caster.material, caster.pipe] { word(w); }
        for value in caster.transform.as_flattened() { word(value.to_bits()); }
        word(caster.time.map_or(0, |t| t | 1));
    }
    hash
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
    /// Cache state + the matrix the layer was rendered with.
    cache: [(StaticCache, Mat4); LAYERS],
    /// Dynamic layers that may hold actor depth and so must be cleared.
    dyn_dirty: [bool; LAYERS],
    commands: [Vec<(usize, Range<u32>)>; LAYERS],
    transforms: Vec<[[f32; 4]; 4]>,
    transform_buffer: Option<wgpu::Buffer>,
    receive: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pub(super) bind: wgpu::BindGroup,
    frame: u64,
    /// Diagnostic for the offline harness: forces every static layer to re-render every frame (measures the walking worst case).
    always_rerender: bool,
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
            cache: std::array::from_fn(|_| (StaticCache::default(), Mat4::ZERO)),
            dyn_dirty: [true; LAYERS], commands: Default::default(),
            transforms: Vec::new(), transform_buffer: None, receive, layout, bind, frame: 0,
            always_rerender: std::env::var_os("AOMAC_V2_SHADOW_RERENDER").is_some(),
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
        admission: &Admission, timers: Option<&PassTimers>) {
        self.frame += 1;
        let span = timers.and_then(|t| t.span("shadows"));
        drop(enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 shadows begin"), timestamp_writes: span.as_ref().map(|s| s.begin_compute()) }));

        let far = r.lens.far.unwrap_or(SHADOW_FAR).min(SHADOW_FAR).max(view.near + 0.01);
        let receiver_far = view.far.min(view.fog_end).min(far);
        let casters = || r.gpu.insts.iter().filter(|inst| !inst.hidden).map(|inst| (inst.center, inst.radius));
        // Current-frame cascade matrices (static fit; the dynamic layers use them as they are, the static layers when they refresh).
        let mut current: [Option<Mat4>; LAYERS] = [None; LAYERS];
        let mut receive = Receive::zeroed();
        let mut sun_frame = None;
        if let Some(light) = sun {
            sun_frame = CascadeFrame::new(view.eye, view.forward, view.up, view.fovy, view.aspect, view.near, far, light.dir, SUN_RES);
            if let Some(frame) = sun_frame.as_mut() {
                frame.include_caster_depth(casters());
                for k in 0..SUN_CASCADES { current[k] = Some(Mat4::from_cols_array_2d(&frame.uniform.matrices[k])); }
                receive.splits = frame.uniform.splits;
                receive.fade[0] = light.strength;
                receive.fade[1] = frame.uniform.splits[2].min(receiver_far).max(frame.uniform.splits[1] + 0.01);
                receive.flags[2] = 1;
            }
        }
        if let Some(light) = moon {
            let matrix = fit_slice(view.eye, view.forward, view.fovy, view.aspect, view.near, receiver_far.max(view.near + 1.0), receiver_far.max(view.near + 1.0), light.dir, MOON_RES, MOON_GUARD)
                .map(|m| crate::shadow_coverage::include_caster_depth(m, casters()));
            if let Some(matrix) = matrix {
                current[SUN_CASCADES] = Some(matrix);
                receive.fade[2] = light.strength;
                receive.fade[3] = receiver_far.max(view.near + 1.0);
                receive.flags[3] = 1;
            }
        }

        self.refresh_static(r, enc, &current, sun_frame.as_ref(), view);
        let items = r.v2_has_actor_items();
        self.draw_dynamic(r, enc, &current, sun_frame.as_ref(), admission, items);

        for k in 0..SUN_CASCADES {
            receive.sun_static[k] = self.cache[k].1.to_cols_array_2d();
            receive.sun_dyn[k] = current[k].unwrap_or(Mat4::ZERO).to_cols_array_2d();
        }
        receive.moon_static = self.cache[SUN_CASCADES].1.to_cols_array_2d();
        receive.moon_dyn = current[SUN_CASCADES].unwrap_or(Mat4::ZERO).to_cols_array_2d();
        receive.flags[0] = u32::from(items && receive.flags[2] != 0);
        receive.flags[1] = u32::from(items && receive.flags[3] != 0);
        r.queue.write_buffer(&self.receive, 0, bytemuck::bytes_of(&receive));
        drop(enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 shadows end"), timestamp_writes: span.as_ref().map(|s| s.end_compute()) }));
    }

    /// Re-renders the static layers whose cadence is due and whose content signature changed.
    fn refresh_static(&mut self, r: &Renderer, enc: &mut wgpu::CommandEncoder, current: &[Option<Mat4>; LAYERS], sun_frame: Option<&CascadeFrame>, view: &View) {
        let time = f64::from(view.time);
        let mut due = [false; LAYERS];
        let mut planes: [[Vec4; 6]; LAYERS] = [[Vec4::ZERO; 6]; LAYERS];
        for layer in 0..LAYERS {
            let Some(matrix) = current[layer] else { continue };
            due[layer] = self.always_rerender || self.cache[layer].0.due(CADENCE[layer], self.frame, time);
            if due[layer] { planes[layer] = crate::frustum_planes(&matrix); }
        }
        if !due.contains(&true) { return; }
        for list in &mut self.commands { list.clear(); }
        self.world_commands(r, &due, &planes);
        let mut render = [false; LAYERS];
        for layer in 0..LAYERS {
            let Some(matrix) = current[layer].filter(|_| due[layer]) else { continue };
            let signature = static_signature(&matrix, r.point_world_revision, self.commands[layer].iter().flat_map(|(draw, range)| {
                let draw_state = &r.gpu.opaque[*draw];
                range.clone().map(move |slot| Caster {
                    draw: *draw as u32, slot, revision: r.point_mesh_revisions.get(draw_state.mesh).copied().unwrap_or(0),
                    material: draw_state.mat as u32, pipe: draw_state.pipe as u32, transform: r.gpu.insts[slot as usize].m,
                    time: draw_state.time_alpha.then(|| r.time.to_bits()),
                })
            }));
            render[layer] = self.cache[layer].0.update(signature, time) || self.always_rerender;
            if render[layer] { self.cache[layer].1 = matrix; }
        }
        if !render.contains(&true) { return; }
        self.sync_transforms(r);
        let Some(instances) = self.transform_buffer.as_ref() else { return };
        for layer in (0..LAYERS).filter(|&l| render[l]) {
            let mut uniform = ShadowUniform::zeroed();
            if layer < SUN_CASCADES {
                if let Some(frame) = sun_frame { uniform.matrices = frame.uniform.matrices; }
                uniform.matrices[layer] = self.cache[layer].1.to_cols_array_2d();
                uniform.control[0] = layer as u32;
            } else {
                uniform.matrices[0] = self.cache[layer].1.to_cols_array_2d();
            }
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

    /// Static world casters per due layer, in draw/instance order (consecutive instances of one draw coalesce).
    fn world_commands(&mut self, r: &Renderer, due: &[bool; LAYERS], planes: &[[Vec4; 6]; LAYERS]) {
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
                for layer in (0..LAYERS).filter(|&l| due[l]) {
                    if crate::shadow_coverage::admits_sphere(&[planes[layer]], instance.center, radius) {
                        crate::shadow_pass::push_static_shadow_command(&mut self.commands[layer], index, slot as u32);
                    }
                }
            }
        }
    }

    /// The dynamic layers: cleared every frame (only when they hold something), then admitted-actor casters.
    fn draw_dynamic(&mut self, r: &Renderer, enc: &mut wgpu::CommandEncoder, current: &[Option<Mat4>; LAYERS], sun_frame: Option<&CascadeFrame>,
        admission: &Admission, items: bool) {
        for layer in 0..LAYERS {
            let Some(matrix) = current[layer] else { continue };
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
    use super::*;

    const FOVY: f32 = 1.0;
    const ASPECT: f32 = 1.5;

    /// The eight corners of the camera frustum slice `start..end`.
    fn slice_corners(eye: Vec3, forward: Vec3, up: Vec3, start: f32, end: f32) -> Vec<Vec3> {
        let right = forward.cross(up).normalize();
        let up = right.cross(forward);
        let mut corners = Vec::new();
        for depth in [start, end] {
            let h = (FOVY * 0.5).tan() * depth;
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                corners.push(eye + forward * depth + right * (sx * h * ASPECT) + up * (sy * h));
            }
        }
        corners
    }

    fn contains(matrix: &Mat4, corners: &[Vec3]) -> bool {
        corners.iter().all(|&c| {
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
            for k in 0..SUN_CASCADES {
                let matrix = Mat4::from_cols_array_2d(&frame.uniform.matrices[k]);
                assert!(contains(&matrix, &slice_corners(eye, forward, Vec3::Y, start, splits[k])), "cascade {k} misses its slice at yaw {yaw}");
                start = splits[k] - (splits[k] - start) * 0.1;
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
        let whole = fit_slice(eye, forward, FOVY, ASPECT, 0.1, 150.0, 150.0, moon, MOON_RES, MOON_GUARD).unwrap();
        assert!(contains(&whole, &slice_corners(eye, forward, Vec3::Y, 0.1, 150.0)));
        // Texel snapping: sub-texel camera motion does not move the grid.
        let moved = fit_slice(eye + Vec3::new(1e-4, 0.0, 0.0), forward, FOVY, ASPECT, 0.1, 150.0, 150.0, moon, MOON_RES, MOON_GUARD).unwrap();
        assert_eq!(whole, moved);
        assert!(fit_slice(eye, forward, FOVY, ASPECT, 0.1, 150.0, 150.0, Vec3::ZERO, MOON_RES, 0.0).is_none());
    }

    fn caster(slot: u32) -> Caster {
        Caster { draw: 1, slot, revision: 4, material: 2, pipe: 3, transform: Mat4::IDENTITY.to_cols_array_2d(), time: None }
    }

    #[test]
    fn signature_tracks_matrix_revision_and_transforms_only() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let base = static_signature(&m, 9, [caster(0), caster(1)].into_iter());
        assert_eq!(base, static_signature(&m, 9, [caster(0), caster(1)].into_iter()), "same static world, same signature");
        assert_ne!(base, static_signature(&(m * Mat4::from_translation(Vec3::X)), 9, [caster(0), caster(1)].into_iter()), "cascade matrix");
        assert_ne!(base, static_signature(&m, 10, [caster(0), caster(1)].into_iter()), "world revision");
        assert_ne!(base, static_signature(&m, 9, [caster(0)].into_iter()), "caster set");
        let mut moved = caster(1);
        moved.transform[3][0] = 5.0;
        assert_ne!(base, static_signature(&m, 9, [caster(0), moved].into_iter()), "instance transform");
        let mut animated = caster(1);
        animated.time = Some(1.0f32.to_bits());
        assert_ne!(base, static_signature(&m, 9, [caster(0), animated].into_iter()), "animated alpha");
    }

    #[test]
    fn cache_survives_actor_changes_and_rerenders_on_matrix_or_revision_change() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let mut cache = StaticCache::default();
        let cadence = Cadence::Frames { period: 1, phase: 0 };
        assert!(cache.due(cadence, 0, 0.0));
        assert!(cache.update(static_signature(&m, 1, [caster(0)].into_iter()), 0.0), "first frame renders");
        // Actors come, go and move for 100 frames: the signature function has no actor input at all, so the layer never re-renders.
        for frame in 1..100u64 {
            assert!(cache.due(cadence, frame, frame as f64));
            assert!(!cache.update(static_signature(&m, 1, [caster(0)].into_iter()), frame as f64), "actor activity must not touch the static layer");
        }
        assert!(cache.update(static_signature(&Mat4::from_translation(Vec3::new(1.0, 2.0, 3.1)), 1, [caster(0)].into_iter()), 100.0), "matrix change");
        assert!(!cache.update(static_signature(&Mat4::from_translation(Vec3::new(1.0, 2.0, 3.1)), 1, [caster(0)].into_iter()), 101.0));
        assert!(cache.update(static_signature(&Mat4::from_translation(Vec3::new(1.0, 2.0, 3.1)), 2, [caster(0)].into_iter()), 102.0), "revision change");
    }

    #[test]
    fn sun_layers_refresh_in_different_frames_and_the_moon_at_most_four_hertz() {
        let mut counts = [0; SUN_CASCADES];
        for frame in 0..64u64 {
            let due: Vec<usize> = (0..SUN_CASCADES).filter(|&k| StaticCache { signature: Some(1), checked: 0.0 }.due(CADENCE[k], frame, 0.0)).collect();
            assert!(due.len() <= 2 && !(due.contains(&1) && due.contains(&2)), "frame {frame}: {due:?}");
            for k in due { counts[k] += 1; }
        }
        assert_eq!(counts, [64, 32, 16]);
        let cache = StaticCache { signature: Some(1), checked: 10.0 };
        assert!(!cache.due(CADENCE[3], 0, 10.2));
        assert!(cache.due(CADENCE[3], 0, 10.25));
        assert!(StaticCache::default().due(CADENCE[3], 0, 0.0), "never-rendered moon layer renders at once");
    }

    #[test]
    fn receive_layout_matches_wgsl() {
        // 8 mat4 (sun static/dynamic, moon static/dynamic) + 3 vec4, 16-byte aligned for a uniform buffer.
        assert_eq!(std::mem::size_of::<Receive>(), 8 * 64 + 3 * 16);
        assert_eq!(std::mem::size_of::<Receive>() % 16, 0);
    }
}
