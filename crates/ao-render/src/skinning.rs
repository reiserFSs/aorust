//! Native CAT skinning writes the unchanged 48-byte render vertex layout.
use super::*;
use std::sync::Arc;

pub(super) struct PoseGpu {
    bones: wgpu::Buffer,
    capacity: u64,
    bind: wgpu::BindGroup,
    palette: Arc<Vec<ao_scene::SkinBone>>,
    pending: bool,
    initialize: bool,
}

impl Renderer {
    pub(super) fn prepare_actor_skins(&mut self, frames: &[ActorFrame]) {
        if !frames.iter().any(|f| f.skin_pose.is_some() && self.act.models.get(&f.model).is_some_and(|m| m.native_skin.is_some())) { return; }
        let native_metal = self.act.native_metal;
        let (_, layout) = self.act.skin_pipeline.get_or_insert_with(|| {
            let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("CAT compute bindings"),
                entries: &std::array::from_fn::<_, 4, _>(|binding| wgpu::BindGroupLayoutEntry {
                    binding: binding as u32, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: if binding == 3 { wgpu::BufferBindingType::Uniform } else { wgpu::BufferBindingType::Storage { read_only: binding != 2 } },
                        has_dynamic_offset: false, min_binding_size: None,
                    }, count: None,
                }),
            });
            let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("CAT compute layout"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
            });
            let shader = if native_metal {
                // SAFETY: fixed native layouts match the four bindings above. Model upload checks attribute
                // count against mesh 0; set_actors checks every active bone index against nonempty palettes.
                unsafe { self.device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                    label: Some("precise native CAT Metal"), msl: Some(include_str!("skinning.metal").into()),
                    num_workgroups: (64, 1, 1), ..Default::default()
                }) }
            } else {
                self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("native CAT scalar skin"), source: wgpu::ShaderSource::Wgsl(include_str!("skinning.wgsl").into()),
                })
            };
            let pipeline = self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("native CAT skin"), layout: Some(&pipeline_layout), module: &shader, entry_point: Some("skin"), compilation_options: Default::default(), cache: None,
            });
            (pipeline, layout)
        });
        for frame in frames {
            let Some(palette) = &frame.skin_pose else { continue };
            let Some(model) = self.act.models.get(&frame.model) else { continue };
            let Some(native) = &model.native_skin else { continue };
            let Some(body) = model.meshes.first().and_then(Option::as_ref) else { continue };
            let actor = self.act.actors.get_mut(&frame.id).unwrap();
            if actor.pose.as_ref().is_some_and(|p| Arc::ptr_eq(&p.palette, palette)) { continue; }
            let initialize = actor.vb.is_none() || actor.pose.as_ref().is_none_or(|p| p.initialize);
            if actor.vb.is_none() {
                actor.vb = Some(self.act.vb_pool.get_mut(&frame.model).and_then(Vec::pop).unwrap_or_else(|| {
                    self.act.vb_created += 1;
                    self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                        label: Some("CAT skinned vertices"), size: (body.nverts * 48) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
                    })
                }));
            }
            let bytes = bytemuck::cast_slice(palette.as_slice());
            let size = (bytes.len() as u64).max(48);
            if actor.pose.as_ref().is_none_or(|p| p.capacity < size) {
                let bones = self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                    label: Some("CAT bone palette"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
                });
                let bind = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                    label: Some("CAT skin resources"), layout, entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: native.0.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: bones.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: actor.vb.as_ref().unwrap().as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 3, resource: native.1.as_entire_binding() },
                    ],
                });
                actor.pose = Some(PoseGpu { bones, capacity: size, bind, palette: Arc::clone(palette), pending: true, initialize });
            }
            let pose = actor.pose.as_mut().unwrap();
            pose.initialize |= initialize;
            pose.palette = Arc::clone(palette);
            pose.pending = true;
            if !bytes.is_empty() { self.queue.write_buffer(&pose.bones, 0, bytes); }
            actor.skin = None;
        }
    }

    /// Encoded before the world render pass; unchanged render shaders consume the output as vertices.
    pub(crate) fn encode_actor_skins(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some((pipeline, _)) = &self.act.skin_pipeline else { return };
        for actor in self.act.actors.values_mut() {
            let Some(pose) = actor.pose.as_mut().filter(|p| p.pending) else { continue };
            let model = &self.act.models[&actor.model];
            let body = model.meshes[0].as_ref().unwrap();
            let vb = actor.vb.as_ref().unwrap();
            if pose.initialize || pose.palette.is_empty() {
                encoder.copy_buffer_to_buffer(&body.vb, 0, vb, 0, (body.nverts * 48) as u64);
            }
        }
        let has_dispatch = self.act.actors.values().any(|actor| actor.pose.as_ref().is_some_and(|p| p.pending && !p.palette.is_empty()));
        let mut pass = has_dispatch.then(|| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("CAT crowd skin"), timestamp_writes: None });
            pass.set_pipeline(pipeline);
            pass
        });
        for actor in self.act.actors.values_mut() {
            let Some(pose) = actor.pose.as_mut().filter(|p| p.pending) else { continue };
            if !pose.palette.is_empty() {
                let pass = pass.as_mut().unwrap();
                pass.set_bind_group(0, &pose.bind, &[]);
                let body = self.act.models[&actor.model].meshes[0].as_ref().unwrap();
                pass.dispatch_workgroups((body.nverts as u32).div_ceil(64), 1, 1);
            }
            pose.pending = false;
            pose.initialize = false;
        }
    }
}
