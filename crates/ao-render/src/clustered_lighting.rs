//! Opt-in local lighting: 64 admitted sources in bounded world-space clusters.
use glam::Vec3;
use crate::gpu_stats::GpuStatsExt;

const SOURCES: usize = 64;
const PREFIX: usize = crate::MAX_DYN_LIGHTS * 4;
const HEADER: usize = PREFIX + 2;
const MAX_CELLS: usize = 512;
const COUNT_ROW: usize = HEADER + SOURCES * 4;
const MAP_ROW: usize = COUNT_ROW + 1;
const CELL_ROW: usize = MAP_ROW + SOURCES;
const OUTPUT_ROWS: usize = CELL_ROW + MAX_CELLS + MAX_CELLS * SOURCES;

pub(super) struct Programs {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl Programs {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let entries: Vec<_> = (0..3).map(|binding| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: binding != 0 }, has_dynamic_offset: false, min_binding_size: None }, count: None,
        }).collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("admitted clustered light build"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("admitted clustered light build"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("admission-before-selection clustered light build"),
            source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("clustered_build.wgsl", include_str!("clustered_build.wgsl"))),
        });
        let pipeline = crate::pipeline_creation::compute(device, &wgpu::ComputePipelineDescriptor {
            label: Some("admitted 64-source spatial clusters"), layout: Some(&pipeline_layout), module: &shader,
            entry_point: Some("build"), compilation_options: Default::default(), cache: None,
        });
        Self { layout, pipeline }
    }
}

struct Gpu {
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    owners: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SourceKey {
    Static(usize),
    Dynamic(usize),
    Owned(u32, u32),
}

struct Birth {
    at: Option<f32>, // None: source was present in the first enabled snapshot.
    seen: u64,
}

fn source_fade(births: &mut std::collections::HashMap<SourceKey, Birth>, key: SourceKey, time: f32, generation: u64, initial: bool) -> f32 {
    let birth = births.entry(key).or_insert(Birth { at: (!initial).then_some(time), seen: generation });
    birth.seen = generation;
    birth.at.map_or(1.0, |at| ((time - at) / 0.25).clamp(0.0, 1.0))
}

#[derive(Default)]
pub(super) struct Frame {
    pub data: Vec<[f32; 4]>,
    gpu: Option<Gpu>,
    owner_ordinals: std::collections::HashMap<u32, u32>,
    births: std::collections::HashMap<SourceKey, Birth>,
    generation: u64,
    #[cfg(test)]
    pairs: Vec<(usize, usize)>,
}

impl Frame {
    #[cfg(test)]
    fn build(&mut self, selected: &[[[f32; 4]; 4]], prefix: &[[f32; 4]]) {
        self.data.clear();
        self.data.resize(HEADER, [0.0; 4]);
        self.data[..prefix.len()].copy_from_slice(prefix);
        self.pairs.clear();
        if selected.is_empty() { return; }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for light in selected {
            let p = Vec3::from_slice(&light[0][..3]);
            lo = lo.min(p - light[0][3]);
            hi = hi.max(p + light[0][3]);
        }
        let mut cell = crate::MIN_CELL;
        let dims = loop {
            let d = ((hi - lo) / cell).ceil().max(Vec3::ONE);
            if d.x * d.y * d.z <= MAX_CELLS as f32 { break d.as_uvec3(); }
            cell *= 2.0;
        };
        self.data[PREFIX] = lo.extend(cell).to_array();
        let cells_start = HEADER + selected.len() * 4;
        self.data[PREFIX + 1] = [f32::from_bits(dims.x), f32::from_bits(dims.y), f32::from_bits(dims.z), f32::from_bits(cells_start as u32)];
        self.data.extend(selected.iter().flat_map(|light| *light));
        let count = (dims.x * dims.y * dims.z) as usize;
        self.data.resize(cells_start + count, [0.0; 4]);
        for (index, light) in selected.iter().enumerate() {
            let p = Vec3::from_slice(&light[0][..3]);
            let range = light[0][3];
            let a = ((p - range - lo) / cell).floor().max(Vec3::ZERO).as_uvec3();
            let b = ((p + range - lo) / cell).floor().as_uvec3().min(dims - 1);
            for z in a.z..=b.z { for y in a.y..=b.y { for x in a.x..=b.x {
                let cmin = lo + Vec3::new(x as f32, y as f32, z as f32) * cell;
                if p.distance_squared(p.clamp(cmin, cmin + cell)) <= range * range {
                    self.pairs.push((((z * dims.y + y) * dims.x + x) as usize, index));
                }
            } } }
        }
        self.pairs.sort_unstable();
        for &(cell_index, light) in &self.pairs {
            let row = cells_start + cell_index;
            if self.data[row][1].to_bits() == 0 { self.data[row][0] = f32::from_bits(self.data.len() as u32); }
            self.data[row][1] = f32::from_bits(self.data[row][1].to_bits() + 1);
            self.data.push([f32::from_bits(light as u32), 0.0, 0.0, 0.0]);
        }
    }
}

impl crate::Renderer {
    pub(super) fn encode_clustered_lights(&mut self, encoder: &mut wgpu::CommandEncoder, eye: Vec3, owners: &wgpu::Buffer) {
        if !self.enhanced_settings.needs_admitted_lights() { return; }
        let data = &mut self.cluster_frame.data;
        data.clear();
        data.extend([[0.0; 4], eye.extend(0.0).to_array()]);
        let owner_ordinals = &mut self.cluster_frame.owner_ordinals;
        owner_ordinals.clear();
        let initial = self.cluster_frame.generation == 0;
        self.cluster_frame.generation = self.cluster_frame.generation.wrapping_add(1);
        let generation = self.cluster_frame.generation;
        let births = &mut self.cluster_frame.births;
        let time = self.time;
        let settings = self.enhanced_settings;
        let mut admit = |packed: &[[f32; 4]], intensity: f32, dynamic: bool, index: usize, owner: Option<u32>, active: bool| {
            let ordinal = owner.map_or(0, |owner| {
                let ordinal = owner_ordinals.entry(owner).or_default();
                let result = *ordinal;
                *ordinal += 1;
                result
            });
            let key = owner.map_or_else(|| if dynamic { SourceKey::Dynamic(index) } else { SourceKey::Static(index) }, |owner| SourceKey::Owned(owner, ordinal));
            // Source age is native lifetime, not selected-pool or GPU-visibility
            // lifetime. Caps, LOD and slider changes must not restart its fade.
            let fade = source_fade(births, key, time, generation, initial);
            let p = Vec3::from_slice(&packed[0][..3]);
            let range = packed[0][3];
            if !active || !p.is_finite() || !range.is_finite() || range <= 0.0 || intensity <= 0.0 || !(p - range).is_finite() || !(p + range).is_finite() { return; }
            let first = data.len();
            data.extend_from_slice(packed);
            for channel in &mut data[first + 1][..3] { *channel *= intensity; }
            data.push([f32::from_bits((fade.to_bits() & !1) | u32::from(dynamic)), f32::from_bits(index as u32), f32::from_bits(owner.unwrap_or(0)), f32::from_bits(ordinal)]);
        };
        for (index, packed) in self.light_base.as_chunks::<4>().0.iter().enumerate() {
            let active = self.light_zones.get(index).copied().flatten().is_none_or(|zone| {
                self.lod.as_ref().is_none_or(|state| state.levels.get(zone as usize).is_some_and(|&level| ao_scene::StatelLod::lights_active(level)))
            });
            let carried = self.static_owners.get(index).is_some_and(Option::is_some);
            let intensity = if !settings.clustered_lighting { 1.0 } else if carried { settings.carried_light_intensity } else { settings.lamp_intensity };
            admit(packed, intensity, false, index, self.static_owners.get(index).copied().flatten(), active);
        }
        for (index, packed) in self.dynamic_packed.as_chunks::<4>().0.iter().enumerate() {
            let carried = self.dynamic_owners.get(index).is_some_and(Option::is_some);
            let intensity = if !settings.clustered_lighting { 1.0 } else if carried { settings.carried_light_intensity } else { settings.lamp_intensity };
            admit(packed, intensity, true, index, self.dynamic_owners.get(index).copied().flatten(), true);
        }
        births.retain(|_, birth| birth.seen == generation);
        data[0][0] = f32::from_bits(((data.len() - 2) / 5) as u32);
        let output_bytes = (OUTPUT_ROWS * 16) as u64;
        if self.dyn_light_buf.size() < output_bytes {
            self.dyn_light_buf = self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: Some("admitted clustered light output"), size: output_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            });
            self.globals_bg = crate::globals_bind(&self.device, &self.g_layout, &self.globals, &self.light_buf, &self.light_cells_buf, &self.light_idx_buf, &self.dyn_light_buf, &self.sampler, &self.clamp_sampler);
        }
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let recreate = self.cluster_frame.gpu.as_ref().is_none_or(|gpu| gpu.input.size() < bytes.len() as u64 || gpu.output != self.dyn_light_buf || gpu.owners != *owners);
        if recreate {
            let input = self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: Some("all native light candidates"), size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            });
            let bind = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("current admitted light candidates"), layout: &self.cluster_programs.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.dyn_light_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: input.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: owners.as_entire_binding() }],
            });
            self.cluster_frame.gpu = Some(Gpu { input, output: self.dyn_light_buf.clone(), owners: owners.clone(), bind });
        }
        let gpu = self.cluster_frame.gpu.as_ref().expect("admitted light resources");
        self.queue.write_buffer(&gpu.input, 0, bytes);
        let timestamp_writes = self.gpu_timer.as_mut().filter(|timer| !timer.is_recording()).map(crate::FrameGpuTimer::begin_compute_pass);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("admitted local light selection and spatial clusters"), timestamp_writes });
        pass.set_pipeline(&self.cluster_programs.pipeline);
        pass.set_bind_group(0, &gpu.bind, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

pub(super) fn apply(source: &mut String) {
    for (name, emissive) in [("vs", "mat.emissive.rgb"), ("vs_actor", "eff_emissive")] {
        let start = source.find(&format!("fn {name}(")).expect("native material vertex");
        let end = start + source[start..].find("\n}").expect("material vertex end");
        let body = source[start..end].replace("    o.dlight = l.dlight;", &format!("    o.dlight = l.dlight;\n    o.color = vec4<f32>(select(clustered_base(o.n, {emissive}), v.color.rgb + 0.8 * g.ambient_g.rgb, prelit), o.color.a);"));
        source.replace_range(start..end, &body);
    }
    // Shadow shares must not inherit saturation from a hidden local source.
    *source = source.replace("max(sun_diffuse * max(sn, 0.0) - excess, vec3<f32>(0.0))", "sun_diffuse * max(sn, 0.0)");
    // A dedicated sentinel keeps authored WaterLight out of local redistribution.
    let start = source.find("fn vs_liquid(").expect("native water vertex");
    let end = start + source[start..].find("\n}").expect("water vertex end");
    let body = source[start..end].replace("    var o = vtx(v);", "    var o = vtx(v);\n    o.effect = 8u;");
    source.replace_range(start..end, &body);
    let ground = source.find("fn vs_ground(").expect("native ground vertex");
    let ground_end = ground + source[ground..].find("\n}").expect("ground vertex end");
    let body = source[ground..ground_end].replace("    o.spec = vec3<f32>(0.0);", "    o.color = vec4<f32>(clustered_directional(n), o.color.a);\n    o.spec = vec3<f32>(0.0);");
    source.replace_range(ground..ground_end, &body);
    // Compute derivatives before ground_tile's alpha discard. Both plain source
    // tests and the generated attachment wrappers use these same native bodies.
    for name in ["fs_ground_fused", "fs_ground_shadow", "fs_ground_simple"] {
        let prefix = format!("fn enhanced_original_{name}(i: VOut) -> vec4<f32> {{");
        let fallback = format!("fn {name}(i: VOut) -> @location(0) vec4<f32> {{");
        let start = source.find(&prefix).or_else(|| source.find(&fallback)).expect("ground fragment");
        let end = start + source[start..].find("\n}").expect("ground fragment end");
        let body = &source[start..end];
        let insertion = body.find('\n').expect("ground fragment body");
        let mut body = body.to_owned();
        for field in ["sun_share", "sun_spec_share"] {
            body = body.replace(&format!(" - i.{field}.xyz * (1.0 - sun_visibility(i.wpos, i.n, i.clip.xy))"), "");
        }
        let channel = if name == "fs_ground_simple" { "dlight" } else { "light" };
        body = body.replace(&format!("i.{channel}"), "cluster_light");
        body.insert_str(insertion, &format!("\n    let cluster_light = clustered_ground(i, {});", name == "fs_ground_simple"));
        source.replace_range(start..end, &body);
    }
    let mut shader = crate::shader_source::load("clustered_lighting.wgsl", include_str!("clustered_lighting.wgsl")).into_owned();
    let shadow = if source.contains("@location(14) sun_share:") { "i.sun_share.xyz * (1.0 - sun_visibility(i.wpos, i.n, i.clip.xy))" } else { "vec3<f32>(0.0)" };
    shader = shader.replace("CLUSTER_SUN_SHADOW", shadow);
    let start = shader.find("fn clustered_pixel_lights(").expect("cluster evaluator");
    let end = start + shader[start..].find("\n}\n").expect("cluster evaluator end") + 3;
    let specular = shader[start..end].replace("fn clustered_pixel_lights(", "fn clustered_native_spec(")
        .replace("footprint: f32)", "footprint: f32, power: f32)")
        .replace("max(dot(n, l), 0.0)", "select(0.0, pow(max(dot(n, normalize(l + normalize(g.eye.xyz - p))), 0.0), power), dot(n, l) > 0.0 && power > 0.0)");
    source.push_str(&specular);
    source.push_str(&shader);
    // Shadow wrappers use a facet normal for depth-gradient bias, not shading.
    // Retain the original smoothly interpolated normal for the local BRDF.
    source.push_str("\nvar<private> clustered_normal: vec3<f32>;\n");
    *source = source.replace("    let beauty = enhanced_original_", "    clustered_normal = i.n;\n    let beauty = enhanced_original_");
    let anchor = "    if !native_unlit() && mat.emissive.w > 1.5";
    *source = source.replace(anchor, &format!("    light = clustered_receiver(i, light);\n    spec = clustered_receiver_spec(i, spec);\n{anchor}"));
    crate::authored_specular::pack_material(source);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_birth_fade_is_native_lifetime_not_pool_or_visibility_lifetime() {
        let mut births = std::collections::HashMap::new();
        let existing = SourceKey::Owned(7, 0);
        let later = SourceKey::Owned(99, 0);
        assert_eq!(source_fade(&mut births, existing, 0.0, 1, true), 1.0);
        assert_eq!(source_fade(&mut births, later, 10.0, 2, false), 0.0);
        // A source ages while hidden/outside the GPU caps: neither fact is
        // an input to native source lifetime tracking.
        assert_eq!(source_fade(&mut births, later, 10.125, 3, false), 0.5);
        assert_eq!(source_fade(&mut births, existing, 10.125, 3, false), 1.0);
        let packed = (0.5f32.to_bits() & !1) | 1;
        assert_eq!(packed & 1, 1);
        assert_eq!(f32::from_bits(packed & !1), 0.5);
        assert_eq!(source_fade(&mut births, later, 10.25, 4, false), 1.0);
        births.retain(|_, birth| birth.seen == 4);
        assert!(!births.contains_key(&existing), "retired native sources do not retain history");
        assert_eq!(source_fade(&mut births, existing, 11.0, 5, false), 0.0, "a newly born source with a reused owner identity fades in again");
    }

    #[test]
    fn clusters_keep_more_than_eight_and_sixteen_sources_with_bounded_indices() {
        let light = crate::pack_light(&ao_scene::Light { pos: [0.0; 3], color: [1.0; 3], range: 10.0, atten: [1.0, 0.0, 0.0], spot: None, zone: None, owner: None });
        let mut frame = Frame::default();
        frame.build(&[light; SOURCES], &[]);
        let meta = frame.data[PREFIX + 1].map(f32::to_bits);
        let cells = (meta[0] * meta[1] * meta[2]) as usize;
        assert!(cells <= MAX_CELLS);
        assert!(frame.data[meta[3] as usize..meta[3] as usize + cells].iter().any(|row| row[1].to_bits() == 64));
        for row in &frame.data[meta[3] as usize..meta[3] as usize + cells] {
            let start = row[0].to_bits() as usize;
            let count = row[1].to_bits() as usize;
            if count > 0 {
                assert!(start + count <= frame.data.len());
                assert!(frame.data[start..start + count].iter().all(|id| id[0].to_bits() < 64));
            }
        }
        let capacity = (frame.data.capacity(), frame.pairs.capacity());
        frame.build(&[light; SOURCES], &[]);
        assert_eq!((frame.data.capacity(), frame.pairs.capacity()), capacity);
        frame.build(&[], &[]);
        assert_eq!(frame.data[PREFIX + 1], [0.0; 4]);
    }

    #[test]
    fn spatial_query_culls_disjoint_source_spheres() {
        let mut near = [[0.0; 4]; 4];
        near[0] = [0.0, 0.0, 0.0, 2.0];
        let mut far = near;
        far[0][0] = 100.0;
        let mut frame = Frame::default();
        frame.build(&[near, far], &[]);
        let grid = frame.data[PREFIX];
        let meta = frame.data[PREFIX + 1].map(f32::to_bits);
        let c = ((Vec3::ZERO - Vec3::from_slice(&grid[..3])) / grid[3]).floor().as_uvec3();
        let cell = ((c.z * meta[1] + c.y) * meta[0] + c.x) as usize;
        let row = frame.data[meta[3] as usize + cell];
        let first = row[0].to_bits() as usize;
        let count = row[1].to_bits() as usize;
        assert!(count > 0);
        assert!(frame.data[first..first + count].iter().all(|id| id[0].to_bits() == 0));
    }

    #[test]
    fn shader_preserves_native_fog_and_excludes_authored_water() {
        let mut source = include_str!("shader.wgsl").to_owned();
        apply(&mut source);
        assert!(source.contains("o.effect = 8u"));
        assert!(source.contains("return min(candidate, vec3<f32>(1.0))"));
        assert!(source.contains("mix(lit, authored_fog, f)"));
        assert!(source.contains("used < 8u"));
        assert!(source.contains("fwidth(i.wpos)"));
    }

    #[test]
    fn prelit_override_keeps_baked_base_and_uses_full_cluster_list() {
        let mut source = include_str!("shader.wgsl").to_owned();
        apply(&mut source);
        assert!(source.contains("v.color.rgb + 0.8 * g.ambient_g.rgb, prelit"));
        let receiver = source.split("fn clustered_receiver(").nth(1).unwrap().split("fn clustered_ground(").next().unwrap();
        assert!(!receiver.contains("mat.emissive.w > 1.5"));
        assert!(receiver.contains("clustered_pixel_lights"));
        let evaluator = source.split("fn clustered_pixel_lights(").nth(1).unwrap().split("fn clustered_directional(").next().unwrap();
        assert!(evaluator.contains("k < min(list.y, 64u)"));
        assert!(!evaluator.contains("used < 8u"));
    }
}

#[cfg(test)]
#[path = "clustered_tests.rs"]
mod gpu_tests;
