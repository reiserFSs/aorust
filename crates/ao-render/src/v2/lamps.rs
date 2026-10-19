//! Enhanced v2 lamps (slot 4): every light the faithful renderer already knows, view-space clusters, cube-shadow atlas.
//!
//! * [`gather`]: static lamps (`light_base`, zone-gated like the native statel LOD), worn/attached moving lights
//!   (`dynamic_packed`), owner-gated on the CPU against `Renderer::admitted_actor_ids()`; frustum + range culled, nearest
//!   [`MAX_LAMPS`] kept. Authored positions, colours, ranges, D3D coefficients and spot cones are the only inputs.
//! * `lamps_build.wgsl` (timer `clusters`): GPU owner revocation against the admission visibility buffer, cube-slot lookup,
//!   then 16x9x24 view-space clusters with <= 8 lights each.
//! * Cube atlas (timer `cube_atlas`): v1's point-shadow cache (static faces cached by world signature, at most 2 faces per
//!   cube of admitted-actor depth per frame) driven from a private 1x1 carrier `SunShadows`; the carrier's actor
//!   visibility buffer is the v2 admission buffer, so a hidden actor can neither cast into a cube nor own a lamp.
//! * `lamps.wgsl`: resolve plugin at group 2, `lamps_light(..)`.
use super::{resolve, timers::PassTimers};
use crate::sun_shadows::SunShadows;
use crate::{Camera, Renderer};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

/// Candidate lamps per frame after culling (nearest first).
pub(super) const MAX_LAMPS: usize = 1024;
/// Linear gain on authored lamp colours: the design's pushed look (DESIGN 4.2), a constant rather than a setting.
pub(super) const LAMP_GAIN: f32 = 1.6;
/// Cube-atlas cubes. Each cube costs 6 actor-face render passes (~40 us each on the M4 Pro) every frame an admitted actor can
/// reach a lamp, because v1 schedules the faces CPU-side and only the GPU knows which <= 2 it rebuilds. Measured with an actor
/// holding a Light Bar: 1 cube 0.25 ms, 2 cubes 0.51 ms, 4 cubes 1.51 ms (budget 0.25): one cube fits the High row.
const CUBES: u32 = 1;
const CLUSTERS: u32 = 16 * 9 * 24;
const CLUSTER_STRIDE: u32 = 9;
/// Length of the admission visibility array (V2Shadows' fixed-size buffer; index = actor interaction slot).
const VISIBILITY_LEN: usize = 8193;
/// Lights beyond this distance (+ their range) cannot reach a cluster (`CLUSTER_FAR_CAP` in lamps_cluster.wgsl).
const FAR_CAP: f32 = 400.0;

/// One candidate lamp, 4 x vec4 (layout documented in lamps_build.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct LampRec {
    pub pos_range: [f32; 4],
    /// Linear colour x [`LAMP_GAIN`]; w = cos(phi/2), or 2 for "no cone".
    pub color_cone: [f32; 4],
    /// D3D attenuation 0..2 (a0 < 0: linear authored), w = cos(theta/2).
    pub atten: [f32; 4],
    /// Spot axis; w = owner interaction slot as bits (0 = not carried).
    pub axis_owner: [f32; 4],
}

impl LampRec {
    #[cfg(test)]
    pub(super) fn owner(&self) -> u32 { self.axis_owner[3].to_bits() }
}

/// Frustum planes (normalised, inward) of a wgpu clip-space matrix: left, right, bottom, top, near, far.
fn frustum(view_proj: Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (view_proj.row(0), view_proj.row(1), view_proj.row(2), view_proj.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| p / p.truncate().length().max(1e-9))
}

pub(super) struct Source<'a> {
    /// 4 vec4 per light, v1 `pack_light`: (pos, range), (gamma colour, cos phi/2 | 2), (atten, cos theta/2), (axis, _).
    pub packed: &'a [[f32; 4]],
    /// Statel zone per light (empty = always on).
    pub zones: &'a [Option<u32>],
    /// Carried-light owner actor id per light.
    pub owners: &'a [Option<u32>],
}

/// Appends the lamps of `src` that can light the view. `zone_active` is the native statel gate; `owner_slot` returns the
/// actor's interaction slot only for an admitted actor (`None` drops the light: a hidden actor emits nothing).
pub(super) fn gather(out: &mut Vec<LampRec>, src: &Source<'_>, zone_active: impl Fn(u32) -> bool, owner_slot: impl Fn(u32) -> Option<u32>, view_proj: Mat4, eye: Vec3) {
    let planes = frustum(view_proj);
    for (i, light) in src.packed.as_chunks::<4>().0.iter().enumerate() {
        let pos = Vec3::from_slice(&light[0][..3]);
        let range = light[0][3];
        let color = [light[1][0], light[1][1], light[1][2]].map(|c| c.max(0.0).powf(2.2) * LAMP_GAIN);
        if !(range > 0.0 && range.is_finite() && pos.is_finite() && color.iter().all(|c| c.is_finite()) && color.iter().any(|&c| c > 0.0)) { continue; }
        if src.zones.get(i).copied().flatten().is_some_and(|zone| !zone_active(zone)) { continue; }
        let owner = match src.owners.get(i).copied().flatten() {
            None => 0,
            Some(id) => match owner_slot(id) { Some(slot) => slot, None => continue },
        };
        if pos.distance(eye) - range > FAR_CAP || planes.iter().any(|plane| plane.truncate().dot(pos) + plane.w < -range) { continue; }
        out.push(LampRec {
            pos_range: light[0],
            color_cone: [color[0], color[1], color[2], light[1][3]],
            atten: light[2],
            axis_owner: [light[3][0], light[3][1], light[3][2], f32::from_bits(owner)],
        });
    }
}

/// Nearest [`MAX_LAMPS`] by distance to the eye.
pub(super) fn keep_nearest(lamps: &mut Vec<LampRec>, eye: Vec3) {
    if lamps.len() > MAX_LAMPS {
        let key = |l: &LampRec| Vec3::from_slice(&l.pos_range[..3]).distance_squared(eye);
        lamps.select_nth_unstable_by(MAX_LAMPS, |a, b| key(a).total_cmp(&key(b)));
        lamps.truncate(MAX_LAMPS);
    }
}

/// Build-pass pipelines: owner revocation + cube lookup (`prep`), cluster lists (`build`), timer carrier (`tick`).
pub(super) struct Build {
    pub layout: wgpu::BindGroupLayout,
    prep: wgpu::ComputePipeline,
    build: wgpu::ComputePipeline,
    tick: wgpu::ComputePipeline,
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }, count: None }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }
}

impl Build {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 lamp clusters"), entries: &[
            uniform_entry(0), uniform_entry(1), storage_entry(2, true), storage_entry(3, false), storage_entry(4, false),
            storage_entry(5, true), storage_entry(6, true), storage_entry(7, false),
        ] });
        let source = format!("{}\n{}\n{}", crate::shader_source::load("v2/common.wgsl", include_str!("common.wgsl")),
            crate::shader_source::load("v2/lamps_cluster.wgsl", include_str!("lamps_cluster.wgsl")),
            crate::shader_source::load("v2/lamps_build.wgsl", include_str!("lamps_build.wgsl")));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 lamp clusters"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("v2 lamp clusters"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = |entry| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry), layout: Some(&pipeline_layout), module: &module, entry_point: Some(entry), compilation_options: Default::default(), cache: None,
        });
        Self { prep: pipeline("prep"), build: pipeline("build"), tick: pipeline("tick"), layout }
    }

    /// Both dispatches in one pass (one timer label). `count` lights; the cluster lists are always rewritten.
    pub(super) fn encode(&self, enc: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup, count: u32, timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 lamp clusters"), timestamp_writes });
        pass.set_bind_group(0, bind, &[]);
        if count > 0 {
            pass.set_pipeline(&self.prep);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        pass.set_pipeline(&self.build);
        pass.dispatch_workgroups(CLUSTERS.div_ceil(64), 1, 1);
    }

    fn tick(&self, enc: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup, timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 timer tick"), timestamp_writes });
        pass.set_pipeline(&self.tick);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

pub(super) struct Lamps {
    build: Build,
    eval_layout: wgpu::BindGroupLayout,
    lights: wgpu::Buffer,
    view_lights: wgpu::Buffer,
    status: wgpu::Buffer,
    clusters: wgpu::Buffer,
    params: wgpu::Buffer,
    /// Owner admission, `array<u32>` of [`VISIBILITY_LEN`], index = actor interaction slot, nonzero = admitted this frame.
    /// ponytail: CPU-written from `admitted_actor_ids()` (alpha > 0) until V2Shadows' GPU depth-tested buffer
    /// (`V2.admission.visibility()`) replaces this one field; every consumer takes the buffer through `owner_visibility()`.
    visibility: wgpu::Buffer,
    visibility_cpu: Vec<u32>,
    carrier: Option<SunShadows>,
    recs: Vec<LampRec>,
}

impl Lamps {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let buffer = |label, size: usize, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage, mapped_at_creation: false });
        let ssbo = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let eval_entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let read = |binding| eval_entry(binding, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None });
        let cube_maps = |binding| eval_entry(binding, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false });
        let eval_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 lamps"), entries: &[
            read(0), read(1), read(2),
            eval_entry(3, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(14336) }),
            cube_maps(4), cube_maps(5), eval_entry(6, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)), read(7),
        ] });
        Self {
            build: Build::new(device),
            eval_layout,
            lights: buffer("v2 lamp records", MAX_LAMPS * 64, ssbo),
            view_lights: buffer("v2 lamp view-space centres", MAX_LAMPS * 16, wgpu::BufferUsages::STORAGE),
            status: buffer("v2 lamp cube slots", MAX_LAMPS * 4, wgpu::BufferUsages::STORAGE),
            clusters: buffer("v2 lamp clusters", CLUSTERS as usize * CLUSTER_STRIDE as usize * 4, wgpu::BufferUsages::STORAGE),
            params: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("v2 lamp params"), contents: &[0u8; 16], usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST }),
            visibility: buffer("v2 owner visibility", VISIBILITY_LEN * 4, ssbo),
            visibility_cpu: vec![0; VISIBILITY_LEN],
            carrier: None,
            recs: Vec::new(),
        }
    }

    /// Resolve plugin (group 2): `wgsl` is the shared cluster geometry followed by the lamp evaluation.
    pub(super) fn plugin(&self) -> resolve::Plugin<'_> {
        resolve::Plugin {
            wgsl: format!("{}\n{}", crate::shader_source::load("v2/lamps_cluster.wgsl", include_str!("lamps_cluster.wgsl")),
                crate::shader_source::load("v2/lamps.wgsl", include_str!("lamps.wgsl"))),
            layout: &self.eval_layout,
        }
    }

    /// The one place the owner admission buffer is chosen.
    fn owner_visibility(&self) -> &wgpu::Buffer { &self.visibility }
}

impl Renderer {
    /// Slot 4: cube atlas, light gather, cluster build. Returns the resolve plugin bind group (group 2).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_lamps(&mut self, lamps: &mut Lamps, enc: &mut wgpu::CommandEncoder, timer: Option<&PassTimers>, consts: &wgpu::Buffer,
        cam: &Camera, view_proj: Mat4, fog_planes: [f32; 3]) -> wgpu::BindGroup {
        // Owner admission for this frame (CPU stand-in, see `Lamps::visibility`): alpha > 0 actors that have a drawn item.
        let admitted: Vec<u32> = self.admitted_actor_ids().collect();
        lamps.visibility_cpu.fill(0);
        for &id in &admitted {
            let slot = self.act.interaction_slot(Some(id));
            if slot != u32::MAX { if let Some(word) = lamps.visibility_cpu.get_mut(slot as usize) { *word = 1; } }
        }
        lamps.visibility_cpu[0] = 0;
        self.queue.write_buffer(&lamps.visibility, 0, bytemuck::cast_slice(&lamps.visibility_cpu));

        // Cube atlas: v1's cache needs the admitted-lights/local-shadow switches; v2 owns its own settings, so the v1
        // switches are forced on for exactly this call.
        let saved = self.enhanced_settings;
        self.enhanced_settings = crate::EnhancedSettings { clustered_lighting: true, local_light_shadows: true, point_shadow_count: CUBES, ..saved };
        let mut carrier = lamps.carrier.take().unwrap_or_else(|| SunShadows::new(self, (1, 1), 1));
        self.prepare_actor_shadows(&mut carrier);
        carrier.actor_visibility = lamps.owner_visibility().clone();
        let span = timer.and_then(|t| t.compute_span("cube_atlas"));
        let (open, close) = span.map_or((None, None), |(a, b)| (Some(a), Some(b)));
        let bind = self.lamps_bind(lamps, consts, None);
        lamps.build.tick(enc, &bind, open);
        self.encode_point_shadows(enc, cam, &mut carrier, fog_planes, false);
        lamps.build.tick(enc, &bind, close);
        self.enhanced_settings = saved;
        lamps.carrier = Some(carrier);

        // Gather every light the faithful renderer knows (statics by statel zone, then moving/carried), nearest first.
        let owner_slot = |id: u32| admitted.contains(&id).then(|| self.act.interaction_slot(Some(id))).filter(|&slot| slot != 0 && slot != u32::MAX && (slot as usize) < VISIBILITY_LEN);
        let zone_active = |zone: u32| self.lod.as_ref().is_none_or(|state| state.levels.get(zone as usize).is_some_and(|&level| ao_scene::StatelLod::lights_active(level)));
        lamps.recs.clear();
        gather(&mut lamps.recs, &Source { packed: &self.light_base, zones: &self.light_zones, owners: &self.static_owners }, &zone_active, owner_slot, view_proj, cam.pos);
        gather(&mut lamps.recs, &Source { packed: &self.dynamic_packed, zones: &[], owners: &self.dynamic_owners }, &zone_active, owner_slot, view_proj, cam.pos);
        keep_nearest(&mut lamps.recs, cam.pos);
        let count = lamps.recs.len() as u32;
        if count > 0 { self.queue.write_buffer(&lamps.lights, 0, bytemuck::cast_slice(&lamps.recs)); }
        self.queue.write_buffer(&lamps.params, 0, bytemuck::cast_slice(&[count, 0, 0, 0]));

        let points = self.point_shadows.as_ref().expect("encode_point_shadows keeps the atlas resident");
        let build_bind = self.lamps_bind(lamps, consts, Some(&points.metadata));
        lamps.build.encode(enc, &build_bind, count, timer.and_then(|t| t.compute("clusters")));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 lamps"), layout: &lamps.eval_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: lamps.lights.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: lamps.status.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: lamps.clusters.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: points.metadata.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&points.array) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&points.actor_array) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&points.sampler) },
            wgpu::BindGroupEntry { binding: 7, resource: lamps.owner_visibility().as_entire_binding() },
        ] })
    }

    /// Build-pass bind group; before the atlas has run this frame only the timer carrier needs a valid one, so `cube_meta`
    /// falls back to the lamp records (any storage buffer; `tick` reads nothing).
    fn lamps_bind(&self, lamps: &Lamps, consts: &wgpu::Buffer, cube_meta: Option<&wgpu::Buffer>) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 lamp clusters"), layout: &lamps.build.layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: lamps.params.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: lamps.lights.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: lamps.view_lights.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: lamps.status.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: lamps.owner_visibility().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: cube_meta.unwrap_or(&lamps.lights).as_entire_binding() },
            wgpu::BindGroupEntry { binding: 7, resource: lamps.clusters.as_entire_binding() },
        ] })
    }
}

/// Resolve group 1 belongs to V2Shadows' plugin. Until it merges, this empty plugin keeps lamps at group 2 (the lead
/// replaces the two `reserved` lines in `V2::new` / `render_v2` with the shadow plugin and deletes this type).
pub(super) struct Reserved { layout: wgpu::BindGroupLayout, pub bind: wgpu::BindGroup }

impl Reserved {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 resolve group 1 reserved"), entries: &[] });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 resolve group 1 reserved"), layout: &layout, entries: &[] });
        Self { layout, bind }
    }

    pub(super) fn plugin(&self) -> resolve::Plugin<'_> { resolve::Plugin { wgsl: String::new(), layout: &self.layout } }
}

#[cfg(test)]
#[path = "lamps_tests.rs"]
mod tests;
