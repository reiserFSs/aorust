//! Point-local caster descriptors and indirect replay of the original material draws.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CacheCaster {
    pub sphere: [f32; 4],
    pub draw: [u32; 4],
    pub key: [u32; 4],
    pub spare: [u32; 4],
}

#[repr(C, align(16))]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Params {
    pub(super) eye: [f32; 4],
    pub(super) timing: [f32; 4],
    pub(super) cache: [u32; 4],
    pub(super) schedule: [f32; 4],
}

/// The view cone to an owner sphere lies inside this eye-to-centre capsule.
/// This is only a conservative dependency test, never actor admission.
pub(crate) fn capsule_relevant(eye: Vec3, sphere: [f32; 4], capsules: &[[f32; 4]]) -> bool {
    if capsules.is_empty() { return false; }
    if !sphere.iter().all(|v| v.is_finite()) || sphere[3] < 0.0 { return true; }
    let center = Vec3::from_slice(&sphere[..3]);
    capsules.iter().any(|owner| {
        if !owner.iter().all(|v| v.is_finite()) || owner[3] < 0.0 { return true; }
        let segment = Vec3::from_slice(&owner[..3]) - eye;
        let length = segment.length_squared();
        let t = if length > 0.0 { ((center - eye).dot(segment) / length).clamp(0.0, 1.0) } else { 0.0 };
        let radius = sphere[3] + owner[3] + 0.001;
        center.distance_squared(eye + segment * t) <= radius * radius
    })
}

pub(super) struct PossibleLight {
    pub(super) sphere: [f32; 4],
    pub(super) owned: bool,
}


pub(super) fn sphere_possible(sphere: [f32; 4], light: &PossibleLight, whole_cube: bool) -> bool {
    let range = if whole_cube { light.sphere[3].max(0.1) * 3.0f32.sqrt() + 0.5 } else { light.sphere[3] + 0.001 };
    let radius = sphere[3] + range;
    Vec3::from_slice(&sphere[..3]).distance_squared(Vec3::from_slice(&light.sphere[..3])) <= radius * radius
}

pub(super) struct FaceDraw<'a> {
    pub(super) kind: u32,
    pub(super) face: usize,
    pub(super) lights: &'a [PossibleLight],
}

fn face_possible(sphere: [f32; 4], face: usize, lights: &[PossibleLight]) -> bool {
    let major = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z][face];
    let side = if face < 2 { Vec3::Z } else { Vec3::X };
    let up = if face == 2 || face == 3 { Vec3::Z } else { Vec3::Y };
    let radius = sphere[3] + 0.001;
    lights.iter().any(|light| {
        let delta = Vec3::from_slice(&sphere[..3]) - Vec3::from_slice(&light.sphere[..3]);
        let depth = major.dot(delta);
        if depth < 0.1 - radius || depth > light.sphere[3] + radius { return false; }
        let diagonal_radius = radius * std::f32::consts::SQRT_2;
        depth - side.dot(delta).abs() >= -diagonal_radius
            && depth - up.dot(delta).abs() >= -diagonal_radius
            && delta.length_squared() <= (light.sphere[3] + radius).powi(2)
    })
}

pub(super) fn static_input_signature(casters: &[CacheCaster]) -> u64 {
    let mut hash = 14695981039346656037u64;
    hash_word(&mut hash, casters.len() as u32);
    for word in bytemuck::cast_slice::<CacheCaster, u32>(casters) { hash_word(&mut hash, *word); }
    hash
}

pub(super) fn world_fingerprint(mut hash: u64, signature: u64, time: f32) -> u64 {
    hash_word(&mut hash, signature as u32);
    hash_word(&mut hash, (signature >> 32) as u32);
    hash_word(&mut hash, (time.max(0.0) / 120.0).floor() as u32);
    hash
}
struct RetainedSource {
    identity: [u32; 2],
    last_possible_time: f32,
    last_possible_frame: u64,
    seen: u64,
}

#[derive(Default)]
pub(super) struct Dispatch {
    fingerprint: Option<u64>,
    pending: u32,
    phase: u32,
    settle_until: f32,
    previous_time: f32,
    selection_fingerprint: Option<u64>,
    selection_changed: bool,
    pub(super) normal_mask: u32,
    pub(super) restoration_possible: bool,
    pub(super) allocation_cold: bool,
    pub(super) possible_lights: Vec<PossibleLight>,
    owners: Vec<u32>,
    owner_capsules: Vec<[f32; 4]>,
    retained_sources: Vec<RetainedSource>,
    frame: u64,
}

fn hash_word(hash: &mut u64, word: u32) {
    *hash = (*hash ^ u64::from(word)).wrapping_mul(1099511628211);
}

impl Dispatch {
    pub(super) fn mask(&mut self, fingerprint: u64, time: f32) -> u32 {
        self.allocation_cold = self.fingerprint.is_none();
        let settling = time != self.previous_time && self.previous_time < self.settle_until;
        self.restoration_possible = self.selection_changed || settling;
        if self.fingerprint != Some(fingerprint) || settling { self.pending = 3; }
        self.previous_time = time;
        self.fingerprint = Some(fingerprint);
        self.normal_mask = 3 << (self.phase * 2);
        self.phase = (self.phase + 1) % 3;
        if self.allocation_cold { self.pending = 0; return 63; }
        if self.pending == 0 { return 0; }
        self.pending -= 1;
        // Encode a conservative superset only for potential source restoration.
        // The GPU still limits existing dirty cubes to normal_mask.
        if self.restoration_possible { 63 } else { self.normal_mask }
    }
}

impl Renderer {
    pub(super) fn point_dispatch_inputs(&self, cam: &Camera, points: &mut PointShadows, admission_planes: [f32; 3]) -> u64 {
        let mut hash = 14695981039346656037u64;
        for value in [cam.pos.x, cam.pos.y, cam.pos.z, cam.yaw, cam.pitch, cam.roll,
            self.lens.fov, self.lens.near, self.lens.far.unwrap_or(self.gpu.radius)] {
            hash_word(&mut hash, value.to_bits());
        }
        for word in [u32::from(self.lens.horizontal), points.size.0, points.size.1] { hash_word(&mut hash, word); }
        let candidates = self.cluster_frame.data[2..].as_chunks::<5>().0;
        let mut nearest = [f32::INFINITY; CUBES];
        let dispatch = &mut points.dispatch;
        dispatch.frame = dispatch.frame.wrapping_add(1);
        dispatch.owners.clear();
        for source in candidates {
            if source[0][3] <= 0.1 || !source[1][..3].iter().any(|&c| c > 0.0) { continue; }
            let owner = source[4][2].to_bits();
            if source[3][3] < 0.0 { dispatch.owners.push(owner); }
            else {
                let score = (Vec3::from_slice(&source[0][..3]) - cam.pos).length_squared();
                if let Some(at) = nearest.iter().position(|&old| score < old) {
                    nearest.copy_within(at..CUBES - 1, at + 1);
                    nearest[at] = score;
                }
            }
        }
        dispatch.owners.sort_unstable();
        dispatch.owners.dedup();
        self.point_owner_capsules(&dispatch.owners, &mut dispatch.owner_capsules);
        if !dispatch.owners.is_empty() {
            let occluders = self.point_owner_occluder_hash(cam.pos, &dispatch.owner_capsules);
            hash_word(&mut hash, occluders as u32);
            hash_word(&mut hash, (occluders >> 32) as u32);
            for capsule in &dispatch.owner_capsules {
                for value in capsule { hash_word(&mut hash, value.to_bits()); }
            }
            self.point_primary_world_hash(cam.pos, &dispatch.owner_capsules, &mut hash);
            hash_word(&mut hash, self.fog_density_scale.to_bits());
            hash_word(&mut hash, u32::from(self.screen.fog_disabled));
            for value in admission_planes { hash_word(&mut hash, value.to_bits()); }
            let grass = self.point_grass_admission_hash();
            hash_word(&mut hash, grass as u32);
            hash_word(&mut hash, (grass >> 32) as u32);
        }
        // N nearer unowned sources beat every unowned source beyond D_N/.8,
        // even with incumbent bias. Closer admitted owned sources only tighten it.
        let cutoff = nearest[points.count - 1] / 0.64 * 1.000001 + 0.001;
        dispatch.possible_lights.clear();
        for source in candidates {
            if source[0][3] <= 0.1 || !source[1][..3].iter().any(|&c| c > 0.0) { continue; }
            let owned = source[3][3] < 0.0;
            let sphere = source[0];
            if !owned {
                let identity = [source[4][0].to_bits() & 1, source[4][1].to_bits()];
                let near = (Vec3::from_slice(&sphere[..3]) - cam.pos).length_squared() <= cutoff;
                let old = dispatch.retained_sources.iter_mut().find(|old| old.identity == identity);
                let retained = if let Some(old) = old {
                    old.seen = dispatch.frame;
                    if near { old.last_possible_time = self.time; old.last_possible_frame = dispatch.frame; }
                    self.time <= old.last_possible_time + FADE_SECONDS
                        || dispatch.frame.wrapping_sub(old.last_possible_frame) <= 3
                } else if near {
                    dispatch.retained_sources.push(RetainedSource {
                        identity, last_possible_time: self.time, last_possible_frame: dispatch.frame, seen: dispatch.frame,
                    });
                    true
                } else { false };
                if !retained { continue; }
            }
            for value in source[0] { hash_word(&mut hash, value.to_bits()); }
            hash_word(&mut hash, source[4][0].to_bits() & 1);
            hash_word(&mut hash, u32::from(owned));
            hash_word(&mut hash, source[4][2].to_bits());
            hash_word(&mut hash, if owned { source[4][3].to_bits() } else { source[4][1].to_bits() });
            dispatch.possible_lights.push(PossibleLight { sphere, owned });
        }
        dispatch.retained_sources.retain(|old| old.seen == dispatch.frame
            && (self.time <= old.last_possible_time + FADE_SECONDS || dispatch.frame.wrapping_sub(old.last_possible_frame) <= 3));
        dispatch.selection_changed = dispatch.selection_fingerprint != Some(hash);
        if dispatch.selection_changed && dispatch.selection_fingerprint.is_some() { dispatch.settle_until = self.time + FADE_SECONDS; }
        dispatch.selection_fingerprint = Some(hash);
        hash
    }

    fn point_primary_world_hash(&self, eye: Vec3, capsules: &[[f32; 4]], hash: &mut u64) {
        let mut vegetation = false;
        for (list, draws) in [&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid].into_iter().enumerate() {
            for (draw_index, draw) in draws.iter().enumerate() {
                if draw.count == 0 || !self.shadow_pipelines.pipelines.get(&(false, draw.pipe, true)).is_some_and(Option::is_some) { continue; }
                for slot in self.gpu.mesh_range[draw.mesh].clone() {
                    let instance = &self.gpu.insts[slot];
                    if instance.hidden { continue; }
                    let radius = if draw.vegetation && self.enhanced_settings.foliage_enabled() { instance.radius * 3.0 + 0.001 } else { instance.radius };
                    if !capsule_relevant(eye, instance.center.extend(radius).to_array(), capsules) { continue; }
                    let key = draw.node.map(|(node, _)| VisualKey::World(slot, node));
                    let priority = visual_priority(draw, key, &self.drawn_visuals);
                    if priority.is_some_and(|priority| priority < 0) { continue; }
                    let revision = self.point_mesh_revisions.get(draw.mesh).copied().unwrap_or(0);
                    for word in [list as u32, draw_index as u32, slot as u32, draw.first_index, draw.count,
                        draw.mat as u32, draw.pipe as u32, revision as u32, (revision >> 32) as u32,
                        priority.unwrap_or(0) as u32] { hash_word(hash, word); }
                    for word in bytemuck::cast_slice::<_, u32>(&instance.m) { hash_word(hash, *word); }
                    if draw.time_alpha { hash_word(hash, self.time.to_bits()); }
                    vegetation |= draw.vegetation;
                }
            }
        }
        if vegetation {
            hash_word(hash, u32::from(self.enhanced_settings.foliage_sway));
            hash_word(hash, u32::from(self.enhanced_settings.foliage_push));
            if self.enhanced_settings.foliage_sway {
                let (phase, wind) = self.foliage.wind();
                for value in [phase, wind[0], wind[1], wind[2]] { hash_word(hash, value.to_bits()); }
            }
            if self.enhanced_settings.foliage_push {
                for value in self.foliage.pushers().as_flattened() { hash_word(hash, value.to_bits()); }
            }
        }
    }
}

impl Renderer {
    pub(super) fn point_static_casters(&self, shadows: &SunShadows, casters: &mut Vec<CacheCaster>, lights: &[PossibleLight]) {
        for (draw_index, range) in &shadows.static_commands {
            let draw = &self.gpu.opaque[*draw_index];
            if !shadows.compiled.pipelines.get(&(false, draw.pipe, false)).is_some_and(Option::is_some) { continue; }
            for instance_index in range.clone() {
                let instance = &self.gpu.insts[instance_index as usize];
                // Length-preserving foliage moves <=.895*height, height<=2r.
                let radius = if draw.vegetation && self.enhanced_settings.foliage_enabled() {
                    instance.radius * 3.0 + 0.001
                } else { instance.radius };
                let sphere = instance.center.extend(radius).to_array();
                if !lights.iter().any(|light| sphere_possible(sphere, light, true)) { continue; }
                let revision = self.point_mesh_revisions.get(draw.mesh).copied().unwrap_or(0);
                let mut pose = 2166136261u32;
                for word in [revision as u32, (revision >> 32) as u32, draw.mat as u32, draw.pipe as u32] {
                    pose = (pose ^ word).wrapping_mul(16777619);
                }
                for word in bytemuck::cast_slice::<_, u32>(&instance.m) { pose = (pose ^ word).wrapping_mul(16777619); }
                if draw.time_alpha { pose = (pose ^ self.time.to_bits()).wrapping_mul(16777619); }
                if draw.vegetation {
                    for flag in [self.enhanced_settings.foliage_sway, self.enhanced_settings.foliage_push] {
                        pose = (pose ^ u32::from(flag)).wrapping_mul(16777619);
                    }
                    if self.enhanced_settings.foliage_sway && !self.enhanced_settings.foliage_push {
                        let (phase, wind) = self.foliage.wind();
                        for value in [phase, wind[0], wind[1], wind[2]] { pose = (pose ^ value.to_bits()).wrapping_mul(16777619); }
                    }
                }
                casters.push(CacheCaster {
                    sphere, draw: [draw.count, draw.first_index, instance_index, 0],
                    key: [pose, *draw_index as u32, 0, 0], spare: [0; 4],
                });
            }
        }
    }

    pub(super) fn draw_point_static(&self, pass: &mut wgpu::RenderPass, shadows: &SunShadows,
        indirect: &wgpu::Buffer, base: u64, casters: &[CacheCaster], cull: FaceDraw<'_>) {
        let Some(instances) = &shadows.static_instances else { return; };
        let (mut pipe, mut mesh, mut material) = (None, None, None);
        for (index, caster) in casters.iter().enumerate() {
            if caster.key[3] != cull.kind || !face_possible(caster.sphere, cull.face, cull.lights) { continue; }
            let draw = &self.gpu.opaque[caster.key[1] as usize];
            let Some((vertices, indices)) = &self.gpu.meshes[draw.mesh] else { continue; };
            pass.set_vertex_buffer(1, instances.slice(caster.draw[2] as u64 * 64..));
            if pipe != Some(draw.pipe) {
                let Some(Some(pipeline)) = shadows.compiled.pipelines.get(&(false, draw.pipe, false)) else { continue; };
                pass.set_pipeline(pipeline); pipe = Some(draw.pipe);
            }
            if mesh != Some(draw.mesh) {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32); mesh = Some(draw.mesh);
            }
            if material != Some(draw.mat) { pass.set_bind_group(1, &self.gpu.mats[draw.mat], &[]); material = Some(draw.mat); }
            pass.draw_indexed_indirect(indirect, base + index as u64 * 20);
        }
    }
}
