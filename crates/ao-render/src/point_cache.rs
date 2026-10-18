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
    pub(super) possible_lights: Vec<[f32; 4]>,
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
    pub(super) fn point_dispatch_inputs(&self, cam: &Camera, points: &mut PointShadows, world_hash: u32, admission_planes: [f32; 3]) -> u64 {
        let mut hash = 14695981039346656037u64;
        for value in [cam.pos.x, cam.pos.y, cam.pos.z, cam.yaw, cam.pitch, cam.roll,
            self.lens.fov, self.lens.near, self.lens.far.unwrap_or(self.gpu.radius),
            self.env.fog_start, self.env.fog_end] {
            hash_word(&mut hash, value.to_bits());
        }
        for word in [u32::from(self.lens.horizontal), points.size.0, points.size.1] { hash_word(&mut hash, word); }
        let candidates = &self.cluster_frame.data[2..];
        let mut owned = false;
        let mut nearest = [f32::INFINITY; 64];
        for source in candidates.as_chunks::<5>().0 {
            if !source[1][..3].iter().any(|&c| c > 0.0) { continue; }
            let owner = source[4][2].to_bits();
            owned |= owner != 0;
            for value in source[0] { hash_word(&mut hash, value.to_bits()); }
            hash_word(&mut hash, source[4][0].to_bits() & 1);
            hash_word(&mut hash, owner);
            hash_word(&mut hash, if owner == 0 { source[4][1].to_bits() } else { source[4][3].to_bits() });
            if owner == 0 {
                let score = (Vec3::from_slice(&source[0][..3]) - cam.pos).length_squared();
                if let Some(at) = nearest.iter().position(|&old| score < old) {
                    nearest.copy_within(at..63, at + 1);
                    nearest[at] = score;
                }
            }
        }
        if owned {
            hash_word(&mut hash, world_hash);
            let occluders = self.point_owner_occluder_hash();
            hash_word(&mut hash, occluders as u32);
            hash_word(&mut hash, (occluders >> 32) as u32);
            for instance in &self.gpu.insts {
                hash_word(&mut hash, u32::from(instance.hidden));
                for word in bytemuck::cast_slice::<_, u32>(&instance.m) { hash_word(&mut hash, *word); }
            }
            for &(priority, bucket, list, draw, instance) in &self.sorted {
                for word in [priority as u32, bucket as u32, u32::from(list), draw as u32, instance] {
                    hash_word(&mut hash, word);
                }
            }
            hash_word(&mut hash, self.fog_density_scale.to_bits());
            hash_word(&mut hash, u32::from(self.screen.fog_disabled));
            for value in admission_planes { hash_word(&mut hash, value.to_bits()); }
            if self.point_primary_time_dependent { hash_word(&mut hash, self.time.to_bits()); }
            if [&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid].into_iter().flatten().any(|draw| draw.vegetation) {
                if self.enhanced_settings.foliage_sway {
                    let (phase, wind) = self.foliage.wind();
                    for value in [phase, wind[0], wind[1], wind[2]] { hash_word(&mut hash, value.to_bits()); }
                }
                if self.enhanced_settings.foliage_push {
                    for value in self.foliage.pushers().as_flattened() { hash_word(&mut hash, value.to_bits()); }
                }
            }
        }
        points.dispatch.possible_lights.clear();
        for source in candidates.as_chunks::<5>().0 {
            if !source[1][..3].iter().any(|&c| c > 0.0) { continue; }
            let score = (Vec3::from_slice(&source[0][..3]) - cam.pos).length_squared();
            if source[4][2].to_bits() == 0 && score > nearest[63] { continue; }
            let mut sphere = source[0];
            sphere[3] = sphere[3].max(0.1) * 3.0f32.sqrt() + 0.1;
            points.dispatch.possible_lights.push(sphere);
        }
        points.dispatch.selection_changed = points.dispatch.selection_fingerprint != Some(hash);
        if points.dispatch.selection_changed && points.dispatch.selection_fingerprint.is_some() {
            points.dispatch.settle_until = self.time + FADE_SECONDS;
        }
        points.dispatch.selection_fingerprint = Some(hash);
        hash_word(&mut hash, world_hash);
        hash_word(&mut hash, (self.time.max(0.0) / 120.0).floor() as u32);
        hash
    }
}

impl Renderer {
    pub(super) fn point_static_casters(&self, shadows: &SunShadows, casters: &mut Vec<CacheCaster>) -> u32 {
        let mut hash = 2166136261u32;
        for word in [self.point_world_revision as u32, (self.point_world_revision >> 32) as u32] {
            hash = (hash ^ word).wrapping_mul(16777619);
        }
        if self.point_world_time_dependent {
            hash = (hash ^ self.time.to_bits()).wrapping_mul(16777619);
        }
        if shadows.static_commands.iter().any(|(index, _)| self.gpu.opaque[*index].vegetation) {
            hash = (hash ^ u32::from(self.enhanced_settings.foliage_sway)).wrapping_mul(16777619);
            hash = (hash ^ u32::from(self.enhanced_settings.foliage_push)).wrapping_mul(16777619);
            if self.enhanced_settings.foliage_sway && !self.enhanced_settings.foliage_push {
                let (phase, wind) = self.foliage.wind();
                for value in [phase, wind[0], wind[1], wind[2]] {
                    hash = (hash ^ value.to_bits()).wrapping_mul(16777619);
                }
            }
        }
        for (draw_index, range) in &shadows.static_commands {
            let draw = &self.gpu.opaque[*draw_index];
            for instance_index in range.clone() {
                let instance = &self.gpu.insts[instance_index as usize];
                for word in bytemuck::cast_slice::<_, u32>(&instance.m) {
                    hash = (hash ^ word).wrapping_mul(16777619);
                }
                for word in [*draw_index as u32, instance_index, draw.first_index, draw.count, draw.mat as u32, draw.pipe as u32] {
                    hash = (hash ^ word).wrapping_mul(16777619);
                }
                // Length-preserving foliage caps bend at .8*height, so displacement
                // is <=.895*height and height<=2*bind sphere radius: 3r encloses it.
                let radius = if draw.vegetation && (self.enhanced_settings.foliage_sway || self.enhanced_settings.foliage_push) {
                    instance.radius * 3.0 + 0.001
                } else { instance.radius };
                casters.push(CacheCaster {
                    sphere: instance.center.extend(radius).to_array(),
                    draw: [draw.count, draw.first_index, instance_index, 0],
                    key: [0, *draw_index as u32, 0, 0], spare: [0; 4],
                });
            }
        }
        hash
    }

    pub(super) fn draw_point_static(&self, pass: &mut wgpu::RenderPass, shadows: &SunShadows,
        indirect: &wgpu::Buffer, base: u64, casters: &[CacheCaster], kind: u32) {
        let Some(instances) = &shadows.static_instances else { return; };
        let (mut pipe, mut mesh, mut material) = (None, None, None);
        for (index, caster) in casters.iter().enumerate() {
            if caster.key[3] != kind { continue; }
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
