//! Exact displayed-pose history. Nested under actors to reuse admitted native draws.
use super::*;
#[path = "motion_visibility.rs"]
mod motion_visibility;
use motion_visibility::{Visibility, VisibilityPrograms};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PoseKey { World(u32), Actor(u32, u64, usize) }
fn retired_actor_pose(key: &PoseKey, ranges: &[(u32, u32)]) -> bool {
    matches!(key, PoseKey::Actor(id, ..)
        if ranges.iter().any(|&(first, count)| id.wrapping_sub(first) < count))
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform { current_vp: [[f32; 4]; 4], previous_vp: [[f32; 4]; 4], previous_model: [[f32; 4]; 4], control: [u32; 4] }
fn transform_cut(previous: [[f32; 4]; 4], current: [[f32; 4]; 4]) -> bool {
    let a = Vec3::from_slice(&previous[3][..3]);
    let b = Vec3::from_slice(&current[3][..3]);
    a.distance_squared(b) > 64.0
}
struct Pose {
    id: u32,
    transform: [[f32; 4]; 4],
    vertices: Option<wgpu::Buffer>,
    bytes: u64,
    room: Option<usize>,
    ready: bool,
    cut: bool,
    visibility_slot: Option<u32>,
    uniform: wgpu::Buffer,
    group: Option<wgpu::BindGroup>,
}
pub(crate) struct Programs {
    layout: wgpu::BindGroupLayout,
    pipelines: HashMap<(bool, usize), crate::pipeline_creation::RenderPipeline>,
    pub samples: u32,
    resolve: Option<(wgpu::BindGroupLayout, crate::pipeline_creation::RenderPipeline)>,
    visibility: VisibilityPrograms,
}
pub(crate) struct MotionVectors {
    pub size: (u32, u32),
    pub motion: wgpu::TextureView,
    pub identity: wgpu::TextureView,
    programs: Arc<Programs>,
    admitted: std::collections::HashSet<PoseKey>,
    depth_input: Option<wgpu::TextureView>,
    multisampled: Option<(wgpu::TextureView, wgpu::TextureView)>,
    resolve_input: Option<wgpu::BindGroup>,
    poses: HashMap<PoseKey, Pose>,
    next_id: u32,
    previous_vp: Mat4,
    current_vp: Mat4,
    reset: bool,
    ordered_clear: bool,
    visibility: Visibility,
}
impl MotionVectors {
    pub fn motion_view(&self) -> &wgpu::TextureView { &self.motion }
    pub fn visibility_changed(&self) -> &wgpu::Buffer { self.visibility.changed() }
    pub fn identity_view(&self) -> &wgpu::TextureView { &self.identity }
    pub fn bytes(&self) -> u64 {
        let samples = if self.programs.samples > 1 { 1 + self.programs.samples } else { 1 };
        self.size.0 as u64 * self.size.1 as u64 * 12 * samples as u64
            + self.visibility.bytes()
            + self.poses.values().map(|p| p.bytes + std::mem::size_of::<Uniform>() as u64).sum::<u64>()
    }
    fn new(r: &Renderer, size: (u32, u32), programs: Arc<Programs>) -> Self {
        let target = |format, label, sample_count| {
            let texture = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: Some(label), size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count, dimension: wgpu::TextureDimension::D2, format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
            });
            texture.create_view_tracked(&Default::default())
        };
        let motion = target(wgpu::TextureFormat::Rgba16Float, "exact pose motion", 1);
        let identity = target(wgpu::TextureFormat::R32Uint, "exact pose identities", 1);
        let multisampled = (programs.samples > 1).then(|| (
            target(wgpu::TextureFormat::Rgba16Float, "per-sample exact motion", programs.samples),
            target(wgpu::TextureFormat::R32Uint, "per-sample exact identity", programs.samples),
        ));
        let visibility = Visibility::new(&r.device, &programs.visibility, multisampled.as_ref().map_or(&identity, |views| &views.1));
        Self { size, motion, identity, multisampled, resolve_input: None, programs, admitted: Default::default(), depth_input: None,
            poses: HashMap::new(), next_id: 1, current_vp: Mat4::IDENTITY, previous_vp: Mat4::IDENTITY, reset: true, ordered_clear: false, visibility }
    }
}
impl Programs {
    fn resolve_program(r: &Renderer) -> (wgpu::BindGroupLayout, crate::pipeline_creation::RenderPipeline) {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: true }, count: None,
        };
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nearest native sample motion resolve"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Depth),
                texture(1, wgpu::TextureSampleType::Float { filterable: false }),
                texture(2, wgpu::TextureSampleType::Uint),
            ],
        });
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nearest native sample motion resolve"),
            source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("motion_resolve.wgsl", include_str!("motion_resolve.wgsl"))),
        });
        let pipeline_layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nearest native sample motion resolve"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let targets = [
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Uint, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        let pipeline = crate::pipeline_creation::render_deferred(&r.device, &wgpu::RenderPipelineDescriptor {
            label: Some("nearest native sample motion resolve"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_resolve"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_resolve"), targets: &targets, compilation_options: Default::default() }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        (layout, pipeline)
    }
    pub(crate) fn is_ready(&self) -> bool {
        self.pipelines.values().all(crate::pipeline_creation::RenderPipeline::is_ready)
            && self.resolve.as_ref().is_none_or(|(_, pipeline)| pipeline.is_ready())
            && self.visibility.is_ready()
    }
    pub(crate) fn wait(&self) {
        for pipeline in self.pipelines.values() { pipeline.get(); }
        if let Some((_, pipeline)) = &self.resolve { pipeline.get(); }
        self.visibility.wait();
    }
    fn new(r: &Renderer) -> Self {
        let samples = r.world.msaa;
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("previous displayed pose"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let motion_source = crate::shader_source::load("motion_vectors.wgsl", include_str!("motion_vectors.wgsl"));
        let motion_source = if samples > 1 { motion_source.into_owned() } else {
            motion_source.replace("@interpolate(perspective, sample) ", "")
                .replace(", @builtin(sample_index) sample: u32", "")
                .replace("motion_pixel(i, sample,", "motion_pixel(i, 0u,")
        };
        let source = format!("{}\n{}", crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl")), motion_source);
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("exact displayed-pose motion"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native motion raster"), bind_group_layouts: &[Some(&r.g_layout), Some(&r.tex_layout), Some(&layout)], immediate_size: 0,
        });
        let (world, actors, _) = build_pipelines();
        let mut pipelines = HashMap::new();
        let mut unique: HashMap<(bool, bool, bool, &'static str), crate::pipeline_creation::RenderPipeline> = HashMap::new();
        for (actor, slots) in [(false, &world), (true, &actors)] {
            for slot in 0..slots.len() {
                let spec = slots.spec(slot);
                let descriptor = spec.descriptor();
                let reactive = descriptor.state.is_some() || spec.effect != 0 || spec.env || spec.fade;
                let fs = if reactive { "fs_motion_reactive" } else if matches!(spec.blend, Blend::AlphaTest) { "fs_motion_test" } else { "fs_motion_opaque" };
                let key = (actor, descriptor.cull, descriptor.bias, fs);
                if let Some(pipeline) = unique.get(&key) {
                    pipelines.insert((actor, slot), pipeline.clone());
                    continue;
                }
                let vertices = wgpu::vertex_attr_array![0=>Float32x3, 1=>Float32x3, 2=>Float32x2, 3=>Float32x4];
                let f4 = |offset| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset, shader_location: 4 + (offset / 16) as u32 };
                let instances = [f4(0), f4(16), f4(32), f4(48), f4(64), f4(80), f4(96), f4(112)];
                let previous = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 12 }];
                let buffers = [
                    wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertices },
                    wgpu::VertexBufferLayout { array_stride: if actor { std::mem::size_of::<ActorInstance>() as u64 } else { 64 }, step_mode: wgpu::VertexStepMode::Instance, attributes: if actor { &instances } else { &instances[..4] } },
                    wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &previous },
                ];
                let targets = [Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                    Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Uint, blend: None, write_mask: wgpu::ColorWrites::ALL })];
                let pipeline = crate::pipeline_creation::render_deferred(&r.device, &wgpu::RenderPipelineDescriptor {
                    label: Some("native exact pose motion"), layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState { module: &shader, entry_point: Some(if actor { "vs_motion_actor" } else { "vs_motion_world" }), compilation_options: Default::default(), buffers: &buffers[..if actor { 3 } else { 2 }] },
                    fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &targets }),
                    primitive: wgpu::PrimitiveState { cull_mode: descriptor.cull.then_some(wgpu::Face::Back), ..Default::default() },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH, depth_write_enabled: Some(false),
                        depth_compare: Some(if reactive { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Equal }),
                        stencil: Default::default(),
                        bias: if descriptor.bias { wgpu::DepthBiasState { constant: -1, slope_scale: 0.0, clamp: 0.0 } } else { Default::default() },
                    }),
                    multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
                });
                unique.insert(key, pipeline.clone());
                pipelines.insert((actor, slot), pipeline);
            }
        }
        let resolve = (samples > 1).then(|| Self::resolve_program(r));
        let visibility = VisibilityPrograms::new(&r.device, samples);
        Self { layout, pipelines, samples, resolve, visibility }
    }
}
impl MotionVectors {
    fn admit(&mut self, device: &wgpu::Device, key: PoseKey, bytes: u64, room: Option<usize>) {
        if self.poses.get(&key).is_some_and(|pose| pose.bytes == bytes && pose.room == room) { return; }
        if let Some(previous) = self.poses.remove(&key) {
            if let Some(slot) = previous.visibility_slot { self.visibility.release(slot); }
        }
        let visibility_slot = matches!(key, PoseKey::Actor(..)).then(|| self.visibility.allocate(device, &self.programs.visibility));
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("motion semantic identity exhausted");
        let uniform = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("previous displayed pose"), size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("exact motion pose"), layout: &self.programs.layout,
            entries: &[wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() }] });
        self.poses.insert(key, Pose { id, transform: Mat4::IDENTITY.to_cols_array_2d(), bytes, room, ready: false, cut: false, visibility_slot, uniform, group: Some(group),
            vertices: (bytes > 0).then(|| device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("previous displayed vertices"), size: bytes,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })) });
    }
    fn retain_poses(&mut self, mut keep: impl FnMut(&PoseKey) -> bool) {
        self.poses.retain(|key, pose| {
            if keep(key) { return true; }
            if let Some(slot) = pose.visibility_slot { self.visibility.release(slot); }
            false
        });
    }
}

impl Renderer {
    /// Settings/upload warmup only: never compile a motion program inside render.
    pub(crate) fn warm_motion_vectors(&mut self) {
        if self.enhanced_settings.taa || self.enhanced_settings.motion_blur > 0.0 {
            if self.motion_programs.as_ref().is_none_or(|programs| programs.samples != self.world.msaa) {
                self.motion_programs = Some(Arc::new(Programs::new(self)));
            }
        } else {
            self.motion_vectors = None;
            self.motion_programs = None;
        }
    }
    pub(super) fn retire_motion_model(&mut self, model: u64) {
        if let Some(motion) = &mut self.motion_vectors {
            motion.retain_poses(|key| !matches!(key, PoseKey::Actor(_, key_model, _) if *key_model == model));
        }
    }
    pub(super) fn retire_motion_actor_visuals(&mut self, ranges: &[(u32, u32)]) {
        if let Some(motion) = &mut self.motion_vectors {
            motion.retain_poses(|key| !retired_actor_pose(key, ranges));
        }
    }
    pub(super) fn retain_motion_actors(&mut self) {
        if let Some(motion) = &mut self.motion_vectors {
            motion.retain_poses(|key| match key {
                PoseKey::Actor(actor, model, _) => self.act.actors.get(actor).is_some_and(|state| state.model == *model),
                PoseKey::World(_) => true,
            });
        }
    }
    /// Call on scene/room admission changes and model topology/material replacement.
    pub(crate) fn reset_motion_vectors(&mut self) {
        if let Some(motion) = &mut self.motion_vectors {
            motion.reset = true;
            motion.retain_poses(|_| false);
        }
    }
    /// After prepare_actors and world admission; previous_vp is the last displayed VP.
    pub(crate) fn prepare_motion_vectors(&mut self, size: (u32, u32), vp: Mat4, previous_vp: Mat4) {
        if !(self.enhanced_settings.taa || self.enhanced_settings.motion_blur > 0.0) {
            self.motion_vectors = None;
            return;
        }
        let programs = self.motion_programs.as_ref().expect("motion programs warmed outside rendering").clone();
        let mut motion = self.motion_vectors.take().filter(|m| m.size == size && m.programs.samples == programs.samples).unwrap_or_else(|| MotionVectors::new(self, size, programs));
        motion.previous_vp = previous_vp;
        motion.current_vp = vp;
        motion.ordered_clear = self.act.commands.iter().any(|command| command.3);
        motion.admitted.clear();
        motion.admitted.extend(self.vis_src.iter().copied().map(PoseKey::World));
        motion.admitted.extend(self.act.items.iter().map(|item| PoseKey::Actor(item.actor, item.model, item.mesh)));
        // Release absent slots before allocating replacements; ordinary churn
        // cannot grow capacity beyond the largest current admitted pose set.
        let admitted = std::mem::take(&mut motion.admitted);
        motion.retain_poses(|key| admitted.contains(key));
        motion.admitted = admitted;
        for (index, &source) in self.vis_src.iter().enumerate() {
            let position = self.vis[index][3];
            let room = self.fog.as_ref().and_then(|fog| fog.camera_room([position[0], position[1], position[2]]));
            let key = PoseKey::World(source); motion.admit(&self.device, key, 0, room);
            let pose = motion.poses.get_mut(&key).unwrap();
            pose.cut = pose.ready && transform_cut(pose.transform, self.vis[index]);
        }
        for (index, item) in self.act.items.iter().enumerate() {
            let Some(mesh) = self.act.models[&item.model].meshes[item.mesh].as_ref() else { continue; };
            let key = PoseKey::Actor(item.actor, item.model, item.mesh);
            let position = self.act.motion_instances[index].transform[3];
            let room = self.fog.as_ref().and_then(|fog| fog.camera_room([position[0], position[1], position[2]]));
            let deforming = item.mesh == 0 && self.act.actors.get(&item.actor).is_some_and(|actor| actor.vb.is_some());
            motion.admit(&self.device, key, if deforming { mesh.nverts as u64 * 48 } else { 0 }, room);
            let pose = motion.poses.get_mut(&key).unwrap();
            pose.cut = pose.ready && transform_cut(pose.transform, self.act.motion_instances[index].transform);
        }
        if self.enhanced_settings.taa {
            motion.visibility.prepare(&self.queue, motion.poses.values().filter_map(|pose| pose.visibility_slot.map(|slot| [pose.id, slot])));
        }
        self.motion_vectors = Some(motion);
    }
    /// After native rendering and native skin compute, using its resolved device depth.
    pub(crate) fn encode_motion_vectors(&mut self, encoder: &mut wgpu::CommandEncoder, native_depth: &wgpu::TextureView) {
        let Some(motion) = &mut self.motion_vectors else { return; };
        if self.enhanced_settings.taa { motion.visibility.begin_frame(encoder); }
        for pose in motion.poses.values_mut() {
            self.queue.write_buffer(&pose.uniform, 0, bytemuck::bytes_of(&Uniform { current_vp: motion.current_vp.to_cols_array_2d(), previous_vp: motion.previous_vp.to_cols_array_2d(), previous_model: pose.transform,
                control: [pose.id, u32::from(!pose.ready || pose.cut || motion.reset || motion.ordered_clear), u32::from(motion.ordered_clear), 0] }));
        }
        if motion.depth_input.as_ref().is_none_or(|view| view != native_depth) {
            motion.resolve_input = motion.multisampled.as_ref().map(|(sample_motion, sample_identity)| self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("nearest native sample motion resolve"), layout: &motion.programs.resolve.as_ref().unwrap().0,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(native_depth) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(sample_motion) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(sample_identity) },
                ],
            }));
            motion.depth_input = Some(native_depth.clone());
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native admitted exact motion raster"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment { view: motion.multisampled.as_ref().map_or(&motion.motion, |views| &views.0), resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }), store: wgpu::StoreOp::Store } }),
                Some(wgpu::RenderPassColorAttachment { view: motion.multisampled.as_ref().map_or(&motion.identity, |views| &views.1), resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } }),
            ], depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: native_depth, depth_ops: None, stencil_ops: None,
            }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_bind_group(0, &self.globals_bg, &[]);
        for priority in [0, 1, 2, 3, 4, 5, 6, 10, 7] {
            // Native ground runs between list 0 and list 1, with the original
            // vertex/index geometry; its lighting-only second pass adds no motion.
            if priority == 1 {
                if let (Some(ground), Some(instances)) = (&self.ground, self.gpu.inst_bufs.get(self.frame)) {
                    pass.set_vertex_buffer(1, instances.slice(..));
                    for draw in &self.gpu.opaque {
                        if ground.materials[draw.mesh].is_none() || draw.count == 0 { continue; }
                        let Some((vb, ib)) = &self.gpu.meshes[draw.mesh] else { continue; };
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.set_pipeline(&motion.programs.pipelines[&(false, draw.pipe)]);
                        pass.set_bind_group(1, &self.gpu.mats[draw.mat], &[]);
                        for vi in self.vis_range[draw.mesh].clone() {
                            let pose = &motion.poses[&PoseKey::World(self.vis_src[vi as usize])];
                            pass.set_bind_group(2, pose.group.as_ref().unwrap(), &[]);
                            pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, vi..vi + 1);
                        }
                    }
                }
            }
            if let Some(instances) = self.gpu.inst_bufs.get(self.frame) {
                pass.set_vertex_buffer(1, instances.slice(..));
                for &(_, _, list, di, vi) in self.sorted.iter().filter(|command| command.0 == priority) {
                    let draw = &[&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid][list as usize][di];
                    let Some((vb, ib)) = &self.gpu.meshes[draw.mesh] else { continue; };
                    let Some(pose) = motion.poses.get(&PoseKey::World(self.vis_src[vi as usize])) else { continue; };
                    pass.set_pipeline(&motion.programs.pipelines[&(false, draw.pipe)]);
                    pass.set_bind_group(1, &self.gpu.mats[draw.mat], &[]);
                    pass.set_bind_group(2, pose.group.as_ref().unwrap(), &[]);
                    pass.set_vertex_buffer(0, vb.slice(..)); pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, vi..vi + 1);
                }
            }
            if let Some(instances) = self.act.bufs.get(self.frame) {
                pass.set_vertex_buffer(1, instances.slice(..));
                for &(_, index, _, _, _) in self.act.commands.iter().filter(|command| command.0 == priority) {
                    let Some(index) = index else { continue; };
                    let item = &self.act.items[index]; let model = &self.act.models[&item.model];
                    let Some(mesh) = &model.meshes[item.mesh] else { continue; };
                    let pose = &motion.poses[&PoseKey::Actor(item.actor, item.model, item.mesh)];
                    let own = (item.mesh == 0).then(|| self.act.actors.get(&item.actor)?.vb.as_ref()).flatten();
                    let vb = own.unwrap_or(&mesh.vb);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_vertex_buffer(2, pose.vertices.as_ref().filter(|_| pose.ready).unwrap_or(vb).slice(..));
                    pass.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.set_bind_group(2, pose.group.as_ref().unwrap(), &[]);
                    for draw in &mesh.draws {
                        if item.rendering_effect == 1 && model.holo.is_none() { continue; }
                        if actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)), item.faded, item.rendering_effect, blended_pipe(draw.pipe)) != priority { continue; }
                        let slot = mesh_effect_pipe(if item.faded { faded_pipe(draw.pipe) } else { draw.pipe }, item.rendering_effect, draw.projection_pass);
                        pass.set_pipeline(&motion.programs.pipelines[&(true, slot)]);
                        pass.set_bind_group(1, &model.mats[draw.mat], &[]);
                        pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, index as u32..index as u32 + 1);
                    }
                }
            }
        }
        drop(pass);
        if let Some((_, pipeline)) = &motion.programs.resolve {
            let mut resolve = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nearest native sample exact motion resolve"), color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment { view: &motion.motion, resolve_target: None, depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } }),
                    Some(wgpu::RenderPassColorAttachment { view: &motion.identity, resolve_target: None, depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } }),
                ], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            resolve.set_pipeline(pipeline);
            resolve.set_bind_group(0, motion.resolve_input.as_ref().unwrap(), &[]);
            resolve.draw(0..3, 0..1);
        }
        if self.enhanced_settings.taa { motion.visibility.encode(encoder, &motion.programs.visibility, motion.size); }
    }
    /// Encode last, after current skin compute and motion raster. Never copy at set_actors:
    /// queue writes there already contain the next pose, not the last displayed one.
    pub(crate) fn finish_motion_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(motion) = &mut self.motion_vectors else { return; };
        for (index, &source) in self.vis_src.iter().enumerate() {
            if let Some(pose) = motion.poses.get_mut(&PoseKey::World(source)) { pose.transform = self.vis[index]; pose.ready = true; }
        }
        for (index, item) in self.act.items.iter().enumerate() {
            let Some(mesh) = &self.act.models[&item.model].meshes[item.mesh] else { continue; };
            let own = (item.mesh == 0).then(|| self.act.actors.get(&item.actor)?.vb.as_ref()).flatten();
            let Some(pose) = motion.poses.get_mut(&PoseKey::Actor(item.actor, item.model, item.mesh)) else { continue; };
            if pose.bytes > 0 { encoder.copy_buffer_to_buffer(own.unwrap_or(&mesh.vb), 0, pose.vertices.as_ref().unwrap(), 0, pose.bytes); }
            pose.transform = self.act.motion_instances[index].transform;
            pose.transform[0][3] = 0.0;
            pose.ready = true;
        }
        motion.reset = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pose_identity_is_mesh_and_model_specific() {
        let keys = [PoseKey::Actor(7, 1, 0), PoseKey::Actor(7, 2, 0), PoseKey::Actor(7, 1, 1), PoseKey::World(7)];
        assert_eq!(keys.into_iter().collect::<std::collections::HashSet<_>>().len(), 4);
    }
    #[test]
    fn native_actor_retirement_covers_all_parts_and_wrapping_ranges() {
        let ranges = [(u32::MAX, 3), (7, 1), (12, 0)];
        for id in [u32::MAX, 0, 1, 7] {
            assert!(retired_actor_pose(&PoseKey::Actor(id, 1, 0), &ranges));
            assert!(retired_actor_pose(&PoseKey::Actor(id, 2, 3), &ranges));
            assert!(!retired_actor_pose(&PoseKey::World(id), &ranges));
        }
        for id in [2, 6, 8, 12] {
            assert!(!retired_actor_pose(&PoseKey::Actor(id, 1, 0), &ranges));
        }
        assert!(!retired_actor_pose(&PoseKey::Actor(7, 1, 0), &[]));
    }
    #[test]
    fn object_teleport_rejects_history_without_changing_exact_pose() {
        let old = Mat4::IDENTITY.to_cols_array_2d();
        let edge = Mat4::from_translation(Vec3::new(8.0, 0.0, 0.0)).to_cols_array_2d();
        let jump = Mat4::from_translation(Vec3::new(8.01, 0.0, 0.0)).to_cols_array_2d();
        assert!(!transform_cut(old, edge));
        assert!(transform_cut(old, jump));
        assert!(!transform_cut(jump, jump));
    }
    #[test]
    fn motion_uses_native_camera_and_exact_native_sample() {
        let shader = include_str!("motion_vectors.wgsl");
        assert!(shader.contains("o.clip = g.vp * wp"));
        assert!(shader.contains("o.native_current = motion_pose.current_vp * wp"));
        assert!(shader.contains("@builtin(sample_index)"));
        assert!(!shader.contains("depth != i.clip.z"));
        let source = include_str!("motion_vectors.rs");
        assert!(source.contains("depth_write_enabled: Some(false)"));
        assert!(source.contains("wgpu::CompareFunction::Equal"));
        assert!(!shader.contains("0.000002"));
        let resolve = include_str!("motion_resolve.wgsl");
        assert!(resolve.contains("if candidate < depth"));
        assert!(resolve.contains("textureLoad(sample_identity, pixel, nearest)"));
    }
    #[test]
    fn previous_pose_is_copied_only_after_raster() {
        let source = include_str!("motion_vectors.rs");
        let finish = source.find("pub(crate) fn finish_motion_frame").unwrap();
        assert!(!source[..finish].contains("encoder.copy_buffer_to_buffer"));
        assert!(source[finish..].contains("pose.ready = true"));
        assert!(source.contains("motion.retain_poses(|key| admitted.contains(key))"));
        assert!(source.contains("motion.retain_poses(|_| false)"));
        assert!(source.contains("pose.bytes == bytes && pose.room == room"));
    }
}
