//! Cached static and admitted actor point shadows, after ao-vk pointshadow.cpp (MIT).
//! Source: wannabuh/ao-vk a0dc47ea2c853d26a4491d5a25ef0331f714b6ce.
use super::*;
use crate::sun_shadows::{ShadowUniform, SunShadows};
#[path = "point_cache.rs"]
mod point_cache;
pub(crate) use point_cache::{CacheCaster, capsule_relevant};

const CUBES: usize = 16;
const RESOLUTION: u32 = 256;
const STATE_WORDS: usize = 28;
const STATE_BYTES: u64 = (STATE_WORDS * 4) as u64;
#[cfg(test)]
const NEAR: f32 = 0.1;
const FADE_SECONDS: f32 = 0.25;

pub(crate) fn receiver_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let mut entries = crate::sun_shadows::receiver_entries().to_vec();
    entries.push(crate::sun_shadows::exposure_entry());
    entries.extend([
        wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
        wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
        wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), count: None },
        wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(14336) }, count: None },
        wgpu::BindGroupLayoutEntry { binding: 9, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
    ]);
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("local and directional receivers"), entries: &entries })
}

pub(crate) struct Compiled {
    layout: wgpu::BindGroupLayout,
    pipeline: crate::pipeline_creation::ComputePipeline,
    cull: crate::pipeline_creation::ComputePipeline,
    cull_actors: crate::pipeline_creation::ComputePipeline,
    clear: wgpu::RenderPipeline,
}
impl Compiled {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let entries: Vec<_> = (0..8).map(|binding| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: if binding == 4 { wgpu::BufferBindingType::Uniform } else { wgpu::BufferBindingType::Storage { read_only: binding == 5 || binding == 7 } },
                has_dynamic_offset: false, min_binding_size: None,
            }, count: None,
        }).collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("GPU admitted cube selection"), entries: &entries });
        let mut source = crate::shader_source::load("point_shadow_select.wgsl", include_str!("point_shadow_select.wgsl")).into_owned();
        source.push_str(&crate::shader_source::load("point_cache.wgsl", include_str!("point_cache.wgsl")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point cache scheduling and conservative indirect culling"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("GPU admitted cube selection"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = crate::pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("GPU admitted cube selection"), layout: Some(&pipeline_layout), module: &shader,
            entry_point: Some("select_cubes"), compilation_options: Default::default(), cache: None,
        });
        let cull = crate::pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("point face sphere and admission culling"), layout: Some(&pipeline_layout), module: &shader,
            entry_point: Some("cull_casters"), compilation_options: Default::default(), cache: None,
        });
        let cull_actors = crate::pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("point actor-only face culling"), layout: Some(&pipeline_layout), module: &shader,
            entry_point: Some("cull_actor_casters"), compilation_options: Default::default(), cache: None,
        });
        let clear_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("indirect cache clear"), bind_group_layouts: &[], immediate_size: 0,
        });
        let clear = crate::pipeline_creation::render(device, &wgpu::RenderPipelineDescriptor {
            label: Some("indirect cache clear"), layout: Some(&clear_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("clear_face"), compilation_options: Default::default(), buffers: &[] },
            fragment: None, primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        pipeline.get();
        cull.get();
        cull_actors.get();
        Self { layout, pipeline, cull, cull_actors, clear }
    }
}

// Vulkan cube face order. Wgpu clip Y points up, while cube texture T points down.
#[cfg(test)]
fn face_matrix(face: usize, position: Vec3, far: f32) -> Mat4 {
    let (major, s, t) = [
        (Vec3::X, -Vec3::Z, -Vec3::Y), (-Vec3::X, Vec3::Z, -Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::Z), (-Vec3::Y, Vec3::X, -Vec3::Z),
        (Vec3::Z, Vec3::X, -Vec3::Y), (-Vec3::Z, -Vec3::X, -Vec3::Y),
    ][face];
    let a = far / (far - NEAR);
    let b = -far * NEAR / (far - NEAR);
    Mat4::from_cols(s.extend(-s.dot(position)), (-t).extend(t.dot(position)),
        (major * a).extend(-a * major.dot(position) + b), major.extend(-major.dot(position))).transpose()
}

pub(crate) struct PointShadows {
    pub receiver: Option<wgpu::BindGroup>,
    pub array: wgpu::TextureView,
    pub actor_array: wgpu::TextureView,
    _actor_maps: wgpu::Texture,
    actor_faces: Vec<wgpu::TextureView>,
    casters: Vec<CacheCaster>,
    dynamic_world: Vec<CacheCaster>,
    static_caster_count: usize,
    static_input_signature: u64,
    caster_buffer: wgpu::Buffer,
    caster_capacity: usize,
    indirect: wgpu::Buffer,
    indirect_stride: u64,
    pub sampler: wgpu::Sampler,
    pub metadata: wgpu::Buffer,
    _maps: wgpu::Texture,
    faces: Vec<wgpu::TextureView>,
    face_data: wgpu::Buffer,
    face_stride: u64,
    state: wgpu::Buffer,
    params: wgpu::Buffer,
    groups: Vec<wgpu::BindGroup>,
    selection: Option<wgpu::BindGroup>,
    input_buffer: Option<wgpu::Buffer>,
    admission: Option<wgpu::Buffer>,
    size: (u32, u32),
    count: usize,
    local_resources: bool,
    clock: Option<f32>,
    dispatch: point_cache::Dispatch,
    static_face_mask: u32,
    actor_face_mask: u32,
    had_dynamic_casters: bool,
}
impl PointShadows {
    fn new(device: &wgpu::Device, size: (u32, u32), local_resources: bool, count: usize) -> Self {
        let resolution = if local_resources { RESOLUTION } else { 1 };
        let face_count = if local_resources { count * 6 } else { 0 };
        let descriptor = wgpu::TextureDescriptor {
            label: Some("cached static point shadow faces"),
            size: wgpu::Extent3d { width: resolution, height: resolution, depth_or_array_layers: face_count.max(6) as u32 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        };
        let maps = device.create_texture_tracked(&descriptor);
        let actor_maps = device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("cached admitted actor point faces"), ..descriptor });
        let view = wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() };
        let array = maps.create_view_tracked(&view);
        let actor_array = actor_maps.create_view_tracked(&view);
        let face_views = |texture: &wgpu::Texture| (0..face_count).map(|layer| texture.create_view_tracked(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: layer as u32, array_layer_count: Some(1), ..Default::default()
        })).collect();
        let faces = face_views(&maps);
        let actor_faces = face_views(&actor_maps);
        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let face_stride = (std::mem::size_of::<ShadowUniform>() as u64).div_ceil(alignment) * alignment;
        let buffer = |label, size, usage| device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some(label), size, usage, mapped_at_creation: false,
        });
        let face_data = buffer("GPU selected cube face matrices", face_stride * (face_count * 2).max(1) as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::STORAGE);
        let metadata = buffer("cached point face receiver metadata", 14336,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let state = buffer("persistent cube cache state", CUBES as u64 * STATE_BYTES,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let params = buffer("point shadow simulation clock and encoded schedule", 64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let caster_buffer = buffer("point caster descriptors", 64, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let indirect = buffer("point cache indirect commands", (face_count * 2).max(1) as u64 * 40,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST);
        let sampler = device.create_sampler_tracked(&wgpu::SamplerDescriptor { label: Some("point shadow comparison PCF"),
            compare: Some(wgpu::CompareFunction::LessEqual), mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        Self { receiver: None, array, actor_array, _actor_maps: actor_maps, actor_faces,
            casters: Vec::new(), dynamic_world: Vec::new(), static_caster_count: 0,
            static_input_signature: 0,
            caster_buffer, caster_capacity: 1, indirect, indirect_stride: 40,
            dispatch: point_cache::Dispatch::default(), static_face_mask: 0,
            actor_face_mask: 0, had_dynamic_casters: false,
            sampler, metadata, _maps: maps, faces, face_data, face_stride, state, params,
            groups: Vec::new(), selection: None, input_buffer: None, admission: None, size, count, local_resources, clock: None }
    }
}

impl Renderer {
    pub(super) fn shadow_receiver(&self) -> &wgpu::BindGroup {
        if self.enhanced_settings.needs_admitted_lights() {
            self.point_shadows.as_ref().and_then(|points| points.receiver.as_ref()).expect("local receiver prepared before world")
        } else { &self.sun_shadows.as_ref().expect("directional receiver prepared before world").receiver }
    }

    /// Explicit fixture diagnostic; never consulted by rendering or admission.
    pub fn point_shadow_owner_is_selected(&self, owner: u32) -> anyhow::Result<bool> {
        let Some(points) = self.point_shadows.as_ref().filter(|points| points.local_resources) else { return Ok(false); };
        let size = CUBES as u64 * STATE_BYTES;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("explicit selected point-owner proof"), size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&points.state, 0, &readback, 0, size);
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _ = sender.send(result); });
        self.device.poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow::anyhow!("point-owner diagnostic poll: {error}"))?;
        receiver.recv()??;
        let mapped = readback.slice(..).get_mapped_range();
        // select_cubes first resolves every incumbent against THIS frame's GPU
        // primary-owner-admitted pool, clearing identity immediately if absent.
        // Thus an owned selected identity also proves current primary admission.
        let selected = bytemuck::cast_slice::<u8, u32>(&mapped).as_chunks::<STATE_WORDS>().0.iter().take(points.count)
            .any(|state| state[0] != 0 && state[1] == u32::MAX && state[2] == owner);
        drop(mapped);
        readback.unmap();
        Ok(selected)
    }

    /// Explicit CPU dispatch diagnostics; these are the actual submitted masks,
    /// conservative descriptor counts and the GPU's static-input memoization key.
    pub fn point_shadow_dispatch_work(&self) -> Option<(usize, usize, u32, u32, u64)> {
        self.point_shadows.as_ref().map(|points| (
            points.static_caster_count, points.casters.len() - points.static_caster_count,
            points.static_face_mask, points.actor_face_mask, points.static_input_signature,
        ))
    }
    pub(crate) fn encode_point_shadows(&mut self, encoder: &mut wgpu::CommandEncoder, cam: &Camera, shadows: &mut SunShadows, admission_planes: [f32; 3]) {
        if !self.enhanced_settings.needs_admitted_lights() { self.point_shadows = None; return; }
        let local = self.enhanced_settings.local_light_shadows;
        let count = self.enhanced_settings.point_shadow_count.clamp(1, CUBES as u32) as usize;
        let mut points = self.point_shadows.take().filter(|p| p.size == shadows.size && p.local_resources == local && p.count == count)
            .unwrap_or_else(|| PointShadows::new(&self.device, shadows.size, local, count));
        let elapsed = shadow_elapsed(points.clock, self.time);
        points.clock = Some(self.time);
        if points.admission.as_ref() != Some(&shadows.actor_visibility) {
            points.admission = Some(shadows.actor_visibility.clone());
            points.selection = None;
            points.groups = (0..points.faces.len() * 2).map(|layer| self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("GPU current point face and actor admission"), layout: &shadows.compiled.caster_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &points.face_data, offset: layer as u64 * points.face_stride,
                        size: wgpu::BufferSize::new(std::mem::size_of::<ShadowUniform>() as u64),
                    }) },
                    wgpu::BindGroupEntry { binding: 3, resource: shadows.actor_visibility.as_entire_binding() },
                ],
            })).collect();
            points.receiver = Some(shadows.point_receiver(&self.device, &points));
        }
        for (index, light) in self.light_base.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            light[3][3] = encode_owner(self.static_owners.get(index).copied().flatten(), &shadows.actor_groups);
        }
        for (index, light) in self.dynamic_packed.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            light[3][3] = encode_owner(self.dynamic_owners.get(index).copied().flatten(), &shadows.actor_groups);
        }
        // Owner proof is reduced on GPU before either the 64-light or cube caps.
        self.encode_clustered_lights(encoder, cam.pos, &shadows.actor_visibility);
        if local {
            self.prepare_point_static(shadows);
            points.casters.clear();
            let selection_inputs = self.point_dispatch_inputs(cam, &mut points, admission_planes);
            self.point_static_casters(shadows, &mut points.casters, &points.dispatch.possible_lights);
            points.dynamic_world.clear();
            if self.enhanced_settings.foliage_push {
                let dynamic_world = &mut points.dynamic_world;
                points.casters.retain(|caster| {
                    if !self.gpu.opaque[caster.key[1] as usize].vegetation { return true; }
                    let mut dynamic = *caster;
                    dynamic.key[3] = 2;
                    dynamic_world.push(dynamic);
                    false
                });
            }
            points.static_input_signature = point_cache::static_input_signature(&points.casters);
            let fingerprint = point_cache::world_fingerprint(selection_inputs, points.static_input_signature, self.time);
            points.static_face_mask = points.dispatch.mask(fingerprint, self.time);
            points.static_caster_count = points.casters.len();
            self.point_actor_casters(shadows, &mut points.casters);
            let static_count = points.static_caster_count;
            let mut index = 0;
            points.casters.retain(|caster| {
                let keep = index < static_count || points.dispatch.possible_lights.iter().any(|light| {
                    !(light.owned && caster.key[2] != 0) && point_cache::sphere_possible(caster.sphere, light, false)
                });
                index += 1;
                keep
            });
            points.casters.append(&mut points.dynamic_world);
            let potential_dynamic = points.casters.len() > points.static_caster_count;
            points.actor_face_mask = if potential_dynamic || points.had_dynamic_casters { 63 } else { 0 };
            points.had_dynamic_casters = potential_dynamic;
            if points.casters.len() > points.caster_capacity {
                points.caster_capacity = points.casters.len().next_power_of_two();
                points.caster_buffer = self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                    label: Some("point caster descriptors"), size: (points.caster_capacity * 64) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
                });
                points.indirect = self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                    label: Some("point cache indirect commands"), size: ((points.caster_capacity + 1) * 20 * count * 12) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
                });
                points.selection = None;
            }
            points.indirect_stride = (points.casters.len() as u64 + 1) * 20;
            if !points.casters.is_empty() {
                self.queue.write_buffer(&points.caster_buffer, 0, bytemuck::cast_slice(&points.casters));
            }
            let params = point_cache::Params {
                eye: cam.pos.extend(points.static_face_mask as f32).to_array(),
                timing: [elapsed / FADE_SECONDS, count as f32, (points.face_stride / 16) as f32, points.static_caster_count as f32],
                cache: [points.casters.len() as u32, points.static_input_signature as u32,
                    (points.static_input_signature >> 32) as u32, (self.time.max(0.0) / 120.0).floor() as u32],
                schedule: [points.dispatch.normal_mask as f32, u8::from(points.dispatch.restoration_possible) as f32,
                    u8::from(points.dispatch.allocation_cold) as f32, points.actor_face_mask as f32],
            };
            self.queue.write_buffer(&points.params, 0, bytemuck::bytes_of(&params));
            if points.selection.is_none() || points.input_buffer.as_ref() != Some(&self.dyn_light_buf) {
                points.input_buffer = Some(self.dyn_light_buf.clone());
                points.selection = Some(self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                    label: Some("admitted pool and persistent cube selection"), layout: &self.point_program.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: self.dyn_light_buf.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: points.face_data.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: points.state.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 3, resource: points.metadata.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 4, resource: points.params.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 5, resource: points.caster_buffer.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 6, resource: points.indirect.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 7, resource: shadows.actor_visibility.as_entire_binding() },
                    ],
                }));
            }
            encoder.clear_buffer(&points.indirect, 0, None);
            {
                let mut pass = encoder.begin_effect_compute_pass(&wgpu::ComputePassDescriptor { label: Some("GPU admitted cube ranking"), timestamp_writes: None });
                pass.set_pipeline(&self.point_program.pipeline);
                pass.set_bind_group(0, points.selection.as_ref().expect("cube selection bindings"), &[]);
                pass.dispatch_workgroups(1, 1, 1);
                if points.static_face_mask != 0 {
                    pass.set_pipeline(&self.point_program.cull);
                    pass.dispatch_workgroups((points.static_caster_count as u32 + 1).div_ceil(64), (count * 6) as u32, 1);
                }
                if points.actor_face_mask != 0 {
                    pass.set_pipeline(&self.point_program.cull_actors);
                    pass.dispatch_workgroups(((points.casters.len() - points.static_caster_count) as u32 + 1).div_ceil(64), (count * 6) as u32, 1);
                }
            }
            // Loaded-world broadphase is conservative; GPU proof and per-face culling own actual draws.
            for layer in 0..points.faces.len() * 2 {
                let actor = layer >= points.faces.len();
                let face_mask = if actor { points.actor_face_mask } else { points.static_face_mask };
                if (face_mask & (1 << (layer % 6))) == 0 { continue; }
                let view = if actor { &points.actor_faces[layer - points.faces.len()] } else { &points.faces[layer] };
                let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("cached admitted local shadow face"), color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }),
                    timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
                let base = layer as u64 * points.indirect_stride;
                pass.set_pipeline(&self.point_program.clear);
                pass.draw_indirect(&points.indirect, base);
                pass.set_bind_group(0, &self.globals_bg, &[]);
                pass.set_bind_group(3, &points.groups[layer], &[]);
                if actor {
                    self.draw_point_actors(&mut pass, shadows, &points.indirect, base + 20, &points.casters);
                    self.draw_point_static(&mut pass, shadows, &points.indirect, base + 20, &points.casters,
                        point_cache::FaceDraw { kind: 2, face: layer % 6, lights: &points.dispatch.possible_lights });
                } else {
                    self.draw_point_static(&mut pass, shadows, &points.indirect, base + 20, &points.casters,
                        point_cache::FaceDraw { kind: 0, face: layer % 6, lights: &points.dispatch.possible_lights });
                }
            }
        }
        self.point_shadows = Some(points);
    }

    fn prepare_point_static(&self, shadows: &mut SunShadows) {
        shadows.static_commands.clear();
        let mut changed = shadows.static_transforms.len() != self.gpu.insts.len();
        if changed { shadows.static_transforms = self.gpu.insts.iter().map(|instance| instance.m).collect(); }
        for (index, draw) in self.gpu.opaque.iter().enumerate() {
            if draw.count == 0 || self.ground.as_ref().is_some_and(|ground| ground.materials[draw.mesh].is_some()) { continue; }
            for slot in self.gpu.mesh_range[draw.mesh].clone() {
                let instance = &self.gpu.insts[slot];
                if instance.hidden || (!self.spaceships_shown && self.spaceship_slots[slot])
                    || self.far_away.iter().any(|run| run.slot == slot) || self.movers.iter().any(|run| run.slot == slot) { continue; }
                let key = draw.node.map(|(node, _)| VisualKey::World(slot, node));
                if visual_priority(draw, key, &self.drawn_visuals).is_some_and(|priority| priority < 0) { continue; }
                if shadows.static_transforms[slot] != instance.m { shadows.static_transforms[slot] = instance.m; changed = true; }
                crate::shadow_pass::push_static_shadow_command(&mut shadows.static_commands, index, slot as u32);
            }
        }
        if shadows.static_transforms.len() > shadows.static_capacity {
            shadows.static_capacity = shadows.static_transforms.len().next_power_of_two();
            shadows.static_instances = Some(self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: Some("bounded static shadow caster transforms"), size: (shadows.static_capacity * 64) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }));
            changed = true;
        }
        if changed { if let Some(buffer) = &shadows.static_instances { self.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&shadows.static_transforms)); } }
    }
}

fn shadow_elapsed(previous: Option<f32>, now: f32) -> f32 {
    previous.map_or(FADE_SECONDS, |previous| (now - previous).max(0.0))
}

fn encode_owner(owner: Option<u32>, groups: &HashMap<u32, u32>) -> f32 {
    owner.map_or(0.0, |owner| -32.0 * (groups.get(&owner).copied().unwrap_or(0) + 1) as f32)
}

pub(crate) fn apply_shader(source: &mut String, local: bool, clustered: bool) {
    if local && !source.contains("@location(4) light: vec4<f32>") {
        crate::authored_specular::pack_material(source);
    }
    // GPU admission precedes the 64-source cap. Keeping the per-receiver native
    // eight-light limit here avoids hidden sources consuming grid/prefix budgets.
    for name in ["vertex_lights", "authored_local_spec"] {
        admitted_native_pool(source, name);
    }
    // One existing flat bit identifies native character receivers without another varying.
    // Mask every pre-existing read so native effect dispatch remains unchanged.
    *source = source.replace("i.effect", "(i.effect & 0x7fffffffu)");
    // effect_time contains an effect clock, not the effect tag.
    *source = source.replace("(i.effect & 0x7fffffffu)_time", "i.effect_time");
    let start = source.find("fn vs_actor(").expect("native actor vertex");
    let end = start + source[start..].find("\n}").expect("actor vertex end");
    let actor = source[start..end].replacen("uv: vec4<f32>)", "uv: vec4<f32>, @location(13) character: u32)", 1)
        .replace("    return o;", "    if character != 0u { o.effect |= 0x80000000u; }\n    return o;");
    source.replace_range(start..end, &actor);
    *source = source.replace("    let beauty = enhanced_original_", "    point_receiver_character = (i.effect & 0x80000000u) != 0u;\n    let beauty = enhanced_original_");
    for name in ["clustered_pixel_lights", "clustered_native_spec", "authored_cluster_spec"] {
        let Some(start) = source.find(&format!("fn {name}(")) else { continue; };
        let end = start + source[start..].find("\n}\n").expect("clustered light end") + 3;
        let mut body = source[start..end].to_owned();
        if !clustered {
            body = body.replace("    var diffuse = vec3<f32>(0.0);",
                "    var diffuse = vec3<f32>(0.0);\n    let directional_count = u32(g.dyn_info.y);\n    var used = directional_count + select(0u, 1u, directional_count > 0u);")
                .replace("k < min(list.y, 64u)", "k < min(list.y, 64u) && used < 8u");
        }
        let increment = if clustered { "" } else { "        used += 1u;\n" };
        body = body.replace("        let col = dyn_lights[li + 1u];",
            &format!("        if !owner_light_admitted(dyn_lights[li + 3u].w) {{ continue; }}\n{increment}        let col = dyn_lights[li + 1u];"));
        if local {
            body = body.replace("        diffuse += ", "        intensity *= point_shadow_visibility(-d, a.w, dyn_lights[li + 3u].w, point_receiver_character);\n        diffuse += ");
        }
        source.replace_range(start..end, &body);
    }
    source.push_str(&crate::shader_source::load("point_shadows.wgsl", include_str!("point_shadows.wgsl")));
    source.push_str(&crate::shader_source::load("point_sampling.wgsl", include_str!("point_sampling.wgsl")));
}

fn admitted_native_pool(source: &mut String, name: &str) {
    let Some(start) = source.find(&format!("fn {name}(")) else { return; };
    let end = start + source[start..].find("\n}\n").expect("native lighting end") + 3;
    let body = &source[start..end];
    let statics = body.find("    if !(any(c").or_else(|| body.find("    if false && !(any(c")).expect("native static grid");
    let moving = body.find("    // Per-frame moving lights").expect("native moving lights");
    let dynamic = body[moving..].replace("let dyn_count = u32(g.dyn_info.x);", "let dyn_count = min(bitcast<u32>(dyn_lights[322u].x), 64u);")
        .replace("for (var k = 0u; k < dyn_count && used < 8u; k = k + 1u) {",
            "for (var candidate = 0u; candidate < min(dyn_count, local_list.y) && used < 8u; candidate += 1u) {\n        let k = bitcast<u32>(dyn_lights[local_list.x + candidate].x);")
        .replace("let li = k * 4u;", "let li = 66u + k * 4u;")
        .replace("        let a = dyn_lights[li];", "        let a = dyn_lights[li];\n        if !owner_light_admitted(dyn_lights[li + 3u].w) { continue; }")
        .replace("        let d = a.xyz - p;\n        let dist = length(d);",
            "        let d = a.xyz - p;\n        if dot(d, d) > a.w * a.w * 1.000001 { continue; }\n        let dist = length(d);");
    let replacement = format!("{}    let local_list = admitted_light_list(p);\n{}", &body[..statics], dynamic);
    source.replace_range(start..end, &replacement);
}

#[cfg(test)]
#[path = "specular_admission_tests.rs"]
mod specular_admission_tests;

#[cfg(test)]
#[path = "point_cache_tests.rs"]
mod point_cache_tests;


#[cfg(test)]
mod tests {
    use super::*;
    use ao_scene::{Instance, Mesh, Submesh, Vertex};
    fn quad(corners: [[f32; 3]; 4]) -> Mesh {
        Mesh {
            vertices: corners.into_iter().map(|pos| Vertex { pos, normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() }).collect(),
            submeshes: vec![Submesh { two_sided: true, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
        }
    }
    fn cube_states(renderer: &Renderer) -> Vec<[u32; 8]> {
        let size = CUBES as u64 * STATE_BYTES;
        let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-only cube identity readback"), size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
        });
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&renderer.point_shadows.as_ref().unwrap().state, 0, &readback, 0, size);
        renderer.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| sender.send(result).unwrap());
        renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        bytemuck::cast_slice::<u8, u32>(&mapped).as_chunks::<STATE_WORDS>().0.iter()
            .map(|state| state[..8].try_into().unwrap()).collect()
    }
    #[test]
    fn simulation_fade_clock_is_deterministic() {
        assert_eq!(shadow_elapsed(None, 0.0), FADE_SECONDS);
        assert_eq!(shadow_elapsed(Some(0.0), 0.0), 0.0);
        assert_eq!(shadow_elapsed(Some(0.0), FADE_SECONDS * 0.5), FADE_SECONDS * 0.5);
        assert_eq!(shadow_elapsed(Some(2.0), 1.0), 0.0);
    }
    #[test]
    fn cube_projection_matches_major_axis_depth_and_texture_faces() {
        let origin = Vec3::new(13.0, -7.0, 2.0);
        for (face, direction) in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z].into_iter().enumerate() {
            let matrix = face_matrix(face, origin, 20.0);
            for distance in [NEAR, 2.0, 20.0] {
                let clip = matrix * (origin + direction * distance).extend(1.0);
                assert!(clip.x.abs() < 1e-4 && clip.y.abs() < 1e-4);
                let expected = 20.0 / (20.0 - NEAR) - 20.0 * NEAR / ((20.0 - NEAR) * distance);
                assert!((clip.z / clip.w - expected).abs() < 1e-4);
            }
        }
    }
    #[test]
    fn packed_owner_proof_is_exact_and_independent_of_cube_assignment() {
        let groups = HashMap::from([(7, 3), (8, 5)]);
        assert_eq!(encode_owner(None, &groups), 0.0);
        assert_eq!(encode_owner(Some(99), &groups), -32.0);
        for (owner, group) in [(7, 3), (8, 5)] {
            for slot in 0..=CUBES {
                let encoded = encode_owner(Some(owner), &groups) - slot as f32;
                assert_eq!((encoded.abs() as u32) % 32, slot as u32);
                assert_eq!((encoded.abs() / 32.0).floor() as u32 - 1, group);
            }
        }
    }

    #[test]
    fn static_owner_metadata_follows_the_exact_grid_filter_and_upload() {
        let light = |x, owner| ao_scene::Light { pos: [x, 0.0, 0.0], range: 4.0, owner, ..Default::default() };
        let sources = [
            light(1.0, Some(7)),
            ao_scene::Light { range: 0.0, ..light(2.0, Some(99)) },
            light(f32::NAN, Some(98)),
            light(3.0, None),
            light(4.0, Some(8)),
        ];
        let grid = LightGrid::new(&sources);
        assert_eq!(grid.owners, [Some(7), None, Some(8)]);
        assert_eq!(grid.zones.len(), grid.owners.len());
        assert_eq!(grid.lights.as_chunks::<4>().0.iter().map(|light| light[0][0]).collect::<Vec<_>>(), [1.0, 3.0, 4.0]);
        let empty = LightGrid::new(&sources[1..3]);
        assert!(empty.owners.is_empty());
    }

    #[test]
    fn local_shadow_generation_is_independent_and_covers_native_and_override_lights() {
        let off = crate::enhanced_shader::source(crate::EnhancedSettings::default());
        assert!(!off.contains("point_shadow_visibility"));
        for clustered in [false, true] {
            for specular in [false, true] {
                let generated = crate::enhanced_shader::source(crate::EnhancedSettings {
                    material_specular: specular, clustered_lighting: clustered, local_light_shadows: true, ..Default::default()
                });
                assert!(generated.contains("fn clustered_pixel_lights("));
                assert!(!generated.contains("point_occlusion["), "each local BRDF must receive cube visibility exactly once");
                assert!(generated.contains("used < 8u"), "shadow-only must retain native light selection");
                assert!(generated.contains("point_receiver_character = (i.effect & 0x80000000u) != 0u"));
                assert!(generated.contains("owner_light_admitted"));
                assert!(!generated.contains("@location(16)"));
                if clustered {
                    assert!(generated.contains("intensity *= point_shadow_visibility(-d, a.w, dyn_lights[li + 3u].w"));
                } else {
                    assert!(generated.contains("k < min(list.y, 64u) && used < 8u"));
                }
            }
        }
    }

    #[test]
    fn point_shadow_only_darkens_a_real_receiver_and_unknown_owner_is_neutral() {
        let gpu = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&gpu, None).expect("point shadow regression GPU");
        renderer.set_msaa_samples(1);
        let mut scene = Scene {
            meshes: vec![
                quad([[-4.0, -3.0, -6.0], [4.0, -3.0, -6.0], [4.0, 3.0, -6.0], [-4.0, 3.0, -6.0]]),
                quad([[0.6, -1.0, -4.5], [1.4, -1.0, -4.5], [1.4, 1.0, -4.5], [0.6, 1.0, -4.5]]),
            ],
            lights: vec![ao_scene::Light { pos: [2.0, 0.0, -3.0], color: [0.4; 3], range: 20.0, ..Default::default() }],
            ..Default::default()
        };
        scene.instances = (0..2).map(|mesh| Instance { mesh, transform: ao_scene::IDENTITY }).collect();
        renderer.upload(&scene);
        renderer.env.sun_color = [0.0; 3];
        renderer.env.ambient = [0.05; 3];
        renderer.env.fog_start = 100.0;
        renderer.env.fog_end = 120.0;
        let targets = Targets::new(&renderer, 64, 64);
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("point shadow only receiver regression"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let camera = Camera::look_at(Vec3::ZERO, -Vec3::Z);
        let capture = |renderer: &mut Renderer| {
            renderer.render(&view, &targets, &camera);
            texture_to_rgba(renderer, &output, 64, 64).unwrap()
        };
        let native = capture(&mut renderer);
        renderer.set_enhanced_settings(EnhancedSettings { local_light_shadows: true, ..Default::default() });
        capture(&mut renderer);
        let shadowed = capture(&mut renderer);
        assert!(shadowed.iter().zip(&native).any(|(&a, &b)| a.saturating_add(8) < b), "independent point switch did not shadow the lit receiver");
        renderer.set_enhanced_settings(EnhancedSettings::default());
        assert_eq!(native, capture(&mut renderer), "all-off did not restore faithful rendering");

        // Unknown owners must not emit light or shadows, nor consume either cap.
        scene.lights[0].owner = Some(99);
        renderer.upload(&scene);
        renderer.env.sun_color = [0.0; 3];
        renderer.env.ambient = [0.05; 3];
        renderer.env.fog_start = 100.0;
        renderer.env.fog_end = 120.0;
        renderer.set_enhanced_settings(EnhancedSettings { local_light_shadows: true, ..Default::default() });
        let unknown = capture(&mut renderer);
        renderer.light_base[0][3] = 0.0;
        assert_eq!(unknown, capture(&mut renderer), "nonadmitted owner emitted light or a shadow");

        scene.lights[0].owner = None;
        renderer.upload(&scene);
        renderer.env.sun_color = [0.0; 3];
        renderer.env.ambient = [0.05; 3];
        renderer.env.fog_start = 100.0;
        renderer.env.fog_end = 120.0;
        let body = Scene { meshes: vec![quad([[-0.15, -0.15, -2.0], [0.15, -0.15, -2.0],
            [0.15, 0.15, -2.0], [-0.15, 0.15, -2.0]])], ..Default::default() };
        renderer.add_actor_model(7, &body);
        let visible = ao_scene::ActorFrame { id: 7, model: 7, character: true, ..Default::default() };
        renderer.set_actors(vec![visible.clone()]);
        let carried = ao_scene::Light { pos: [0.0, 0.0, -1.5], color: [0.4; 3], range: 20.0, owner: Some(7), ..Default::default() };
        renderer.set_dynamic_lights(&[carried]);
        renderer.set_enhanced_settings(EnhancedSettings { local_light_shadows: true, point_shadow_count: 1, ..Default::default() });
        let admitted = capture(&mut renderer);
        let before = cube_states(&renderer);
        assert_eq!(&before[0][..4], &[2, u32::MAX, 7, 0], "carried cube needs exact owner identity, not producer index");
        assert_eq!(f32::from_bits(before[0][4]), 1.0);
        let hidden = ao_scene::ActorFrame { id: 99, model: 7, character: true,
            transform: Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0)).to_cols_array_2d(), ..Default::default() };
        renderer.set_actors(vec![hidden, visible]);
        let mut inserted = vec![ao_scene::Light { pos: [0.0, 0.0, -0.5], owner: Some(99), ..carried }; 65];
        inserted.push(carried);
        renderer.set_dynamic_lights(&inserted);
        assert_eq!(admitted, capture(&mut renderer), "hidden owners evicted or reset a visible carried source");
        assert_eq!(before, cube_states(&renderer), "dense primary IDs or producer order changed persistent cube identity");

        renderer.set_actors(vec![
            ao_scene::ActorFrame { id: 7, model: 7, character: true, ..Default::default() },
            ao_scene::ActorFrame { id: 8, model: 7, character: true,
                transform: Mat4::from_translation(Vec3::new(-0.5, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() },
        ]);
        renderer.set_dynamic_lights(&[ao_scene::Light { owner: Some(8), pos: [-0.5, 0.0, -1.5], ..carried }]);
        capture(&mut renderer);
        let born = cube_states(&renderer);
        assert_eq!(&born[0][..4], &[2, u32::MAX, 8, 0]);
        assert_eq!(f32::from_bits(born[0][4]), 0.0, "genuinely new source skipped its appearance fade");
        capture(&mut renderer);
        assert_eq!(born, cube_states(&renderer), "frozen captures advanced source age");
        renderer.time = FADE_SECONDS * 0.5;
        capture(&mut renderer);
        assert_eq!(f32::from_bits(cube_states(&renderer)[0][4]), 0.5);
        renderer.time = FADE_SECONDS;
        capture(&mut renderer);
        assert_eq!(f32::from_bits(cube_states(&renderer)[0][4]), 1.0);

        renderer.set_actors(vec![
            ao_scene::ActorFrame { id: 8, model: 7, character: true,
                transform: Mat4::from_translation(Vec3::new(-0.5, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() },
            ao_scene::ActorFrame { id: 9, model: 7, character: true,
                transform: Mat4::from_translation(Vec3::new(0.5, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() },
        ]);
        let incumbent = ao_scene::Light { owner: Some(8), pos: [-0.5, 0.0, -1.5], ..carried };
        let challenger = ao_scene::Light { owner: Some(9), pos: [-0.4, 0.0, -1.3], ..carried };
        renderer.set_dynamic_lights(&[incumbent, challenger]);
        capture(&mut renderer);
        assert_eq!(&cube_states(&renderer)[0][..4], &[2, u32::MAX, 8, 0], "0.8 incumbent preference did not prevent a near-distance swap");
        let challenger = ao_scene::Light { pos: [0.0, 0.0, -0.7], ..challenger };
        renderer.set_dynamic_lights(&[incumbent, challenger]);
        renderer.time += FADE_SECONDS * 0.5;
        capture(&mut renderer);
        let departing = cube_states(&renderer);
        assert_eq!(&departing[0][..4], &[2, u32::MAX, 8, 0]);
        assert_eq!(f32::from_bits(departing[0][4]), 0.5, "beaten incumbent did not fade out");
        renderer.time += FADE_SECONDS * 0.5;
        capture(&mut renderer);
        assert_eq!(&cube_states(&renderer)[0][..4], &[2, u32::MAX, 9, 0], "clearly nearer source did not receive the released cube");

        // Exercise the OUTER cap too: owner 8 remains a native source while 65
        // visible owner-9 lamps displace it from the admitted 64-source pool.
        let mut crowd = vec![challenger; 65];
        crowd.push(incumbent);
        renderer.set_dynamic_lights(&crowd);
        capture(&mut renderer);
        assert_eq!(&cube_states(&renderer)[0][..4], &[2, u32::MAX, 9, 0]);
        renderer.set_actors(vec![ao_scene::ActorFrame { id: 8, model: 7, character: true,
            transform: Mat4::from_translation(Vec3::new(-0.5, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() }]);
        let outer_restored_image = capture(&mut renderer);
        let outer_restored = cube_states(&renderer);
        assert_eq!(&outer_restored[0][..4], &[2, u32::MAX, 8, 0]);
        assert_eq!(f32::from_bits(outer_restored[0][4]), 1.0, "64-source eviction restarted an existing visible source's fade");
        renderer.set_dynamic_lights(&[incumbent]);
        assert_eq!(outer_restored_image, capture(&mut renderer), "hidden owner retained a negative shadow signal after outer-cap eviction");

        renderer.set_actors(Vec::new());
        let no_owner = capture(&mut renderer);
        let restored = cube_states(&renderer);
        assert_eq!(&restored[0][..4], &[1, 0, 0, 0]);
        assert_eq!(f32::from_bits(restored[0][4]), 1.0, "static source retained a deficit from a now-hidden owner");
        renderer.set_dynamic_lights(&[]);
        assert_eq!(no_owner, capture(&mut renderer), "hidden carried source left temporal shadow residue");

        scene.lights = (0..17).map(|index| ao_scene::Light {
            pos: [2.0 + index as f32 * 0.1, 0.0, -3.0], color: [0.04; 3], range: 20.0, ..Default::default()
        }).collect();
        renderer.upload(&scene);
        renderer.set_enhanced_settings(EnhancedSettings { local_light_shadows: true, point_shadow_count: 16, ..Default::default() });
        capture(&mut renderer);
        assert_eq!(renderer.point_shadows.as_ref().unwrap().faces.len(), CUBES * 6);
        assert_eq!(cube_states(&renderer).iter().filter(|state| state[0] != 0).count(), CUBES);
    }

    #[test]
    fn carried_cube_excludes_characters_but_dynamic_doors_cast_and_receive() {
        let gpu = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&gpu, None).expect("character/door shadow regression GPU");
        renderer.set_msaa_samples(1);
        let receiver = quad([[-4.0, -3.0, -6.0], [4.0, -3.0, -6.0], [4.0, 3.0, -6.0], [-4.0, 3.0, -6.0]]);
        let door = quad([[0.6, -1.0, -4.5], [1.4, -1.0, -4.5], [1.4, 1.0, -4.5], [0.6, 1.0, -4.5]]);
        let owner_model = Scene { meshes: vec![quad([[-0.8, -0.15, -2.0], [-0.6, -0.15, -2.0],
            [-0.6, 0.15, -2.0], [-0.8, 0.15, -2.0]])], ..Default::default() };
        let configure = |renderer: &mut Renderer, mesh: Mesh| {
            renderer.upload(&Scene { meshes: vec![mesh], instances: vec![Instance { mesh: 0, transform: ao_scene::IDENTITY }], ..Default::default() });
            renderer.env.sun_color = [0.0; 3];
            renderer.env.ambient = [0.05; 3];
            renderer.env.sky_color = [0.0; 3];
            renderer.env.fog_start = 100.0;
            renderer.env.fog_end = 120.0;
        };
        configure(&mut renderer, receiver.clone());
        renderer.add_actor_model(7, &owner_model);
        renderer.add_actor_model(8, &Scene { meshes: vec![door.clone()], ..Default::default() });
        let owner = ao_scene::ActorFrame { id: 7, model: 7, character: true, ..Default::default() };
        let prop = ao_scene::ActorFrame { id: 8, model: 8, ..Default::default() };
        let lamp = ao_scene::Light { pos: [2.0, 0.0, -3.0], color: [0.4; 3], range: 20.0, owner: Some(7), ..Default::default() };
        renderer.set_dynamic_lights(&[lamp]);
        let targets = Targets::new(&renderer, 64, 64);
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("character/door semantic shadow comparison"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let camera = Camera::look_at(Vec3::ZERO, -Vec3::Z);
        let capture = |renderer: &mut Renderer| {
            renderer.render(&view, &targets, &camera);
            texture_to_rgba(renderer, &output, 64, 64).unwrap()
        };
        let center = |image: &[u8]| -> u32 {
            (30..34).flat_map(|y| (30..34).map(move |x| (y * 64 + x) * 4))
                .map(|pixel| image[pixel..pixel + 3].iter().map(|&v| u32::from(v)).sum::<u32>()).sum()
        };

        // Both instance records change classification; the faithful shader must
        // ignore the new attribute and preserve every byte of the native image.
        renderer.set_actors(vec![ao_scene::ActorFrame { character: false, ..owner.clone() }, prop.clone()]);
        let native_props = capture(&mut renderer);
        renderer.set_actors(vec![owner.clone(), ao_scene::ActorFrame { character: true, ..prop.clone() }]);
        let native_characters = capture(&mut renderer);
        assert_eq!(native_props, native_characters, "character metadata changed the all-off native image");

        renderer.set_enhanced_settings(EnhancedSettings { local_light_shadows: true, ..Default::default() });
        let excluded_character = capture(&mut renderer);
        renderer.set_actors(vec![owner.clone(), prop.clone()]);
        let casting_door = capture(&mut renderer);
        assert!(center(&casting_door) + 16 * 3 * 8 < center(&excluded_character),
            "the same admitted geometry must cast as a dynamic door, but not as a character");
        renderer.set_enhanced_settings(EnhancedSettings::default());
        assert_eq!(native_props, capture(&mut renderer), "all-off restoration changed the actor instance prefix");

        // A dynamic door/prop also remains a receiver of carried-light shadows.
        // A native character occupying identical geometry must not receive them.
        configure(&mut renderer, door);
        renderer.add_actor_model(7, &owner_model);
        renderer.add_actor_model(9, &Scene { meshes: vec![receiver], ..Default::default() });
        renderer.set_dynamic_lights(&[lamp]);
        renderer.set_enhanced_settings(EnhancedSettings { local_light_shadows: true, ..Default::default() });
        let prop_receiver = ao_scene::ActorFrame { id: 8, model: 9, ..Default::default() };
        renderer.set_actors(vec![owner.clone(), prop_receiver.clone()]);
        let shadowed_prop = capture(&mut renderer);
        renderer.set_actors(vec![owner, ao_scene::ActorFrame { character: true, ..prop_receiver }]);
        let unshadowed_character = capture(&mut renderer);
        assert!(center(&shadowed_prop) + 16 * 3 * 8 < center(&unshadowed_character),
            "carried-light receiver exclusion incorrectly included non-character actors");
    }
}
