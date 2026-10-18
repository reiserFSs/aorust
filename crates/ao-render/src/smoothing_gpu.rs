//! Cached CAT topology; midpoint positions are evaluated only after the original pose.
use super::*;
#[cfg(test)]
#[path = "smoothing_gpu_tests.rs"]
mod tests;

pub(super) struct MeshGpu {
    pub(super) vertices: wgpu::Buffer,
    pub(super) indices: wgpu::Buffer,
    pub(super) draws: Vec<Draw>,
    samples: wgpu::Buffer,
    counts: wgpu::Buffer,
    pub(super) vertex_count: u32,
}

pub(super) struct ActorGpu {
    pub(super) vertices: wgpu::Buffer,
    bind: wgpu::BindGroup,
    source_is_skin: bool,
}

impl MeshGpu {
    pub(super) fn new(device: &wgpu::Device, mesh: &ao_scene::Mesh, draws: &[Draw]) -> Option<Self> {
        let subdiv = character_smoothing::subdivide(mesh);
        if subdiv.samples.is_empty() { return None; }
        let vertices = device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
            label: Some("CAT smooth bind vertices"), contents: bytemuck::cast_slice(&subdiv.vertices), usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
        });
        let mut indices = Vec::new();
        let mut ranges = Vec::with_capacity(subdiv.submeshes.len());
        for submesh in &subdiv.submeshes {
            let first = indices.len() as u32;
            indices.extend_from_slice(&submesh.indices);
            ranges.push((first, indices.len() as u32 - first));
        }
        let mut smooth_draws = Vec::with_capacity(draws.len());
        // Preserve each native draw and its env redraw; original ranges identify the submesh.
        for draw in draws {
            let mut first = 0;
            for (submesh, &(smooth_first, count)) in mesh.submeshes.iter().zip(&ranges) {
                let valid_count = submesh.indices.as_chunks::<3>().0.iter()
                    .filter(|triangle| triangle.iter().all(|&index| (index as usize) < mesh.vertices.len())).count() as u32 * 3;
                if draw.first_index == first && draw.count == valid_count {
                    smooth_draws.push(Draw { first_index: smooth_first, count, ..*draw });
                    break;
                }
                first += valid_count;
            }
        }
        Some(Self {
            vertices,
            indices: device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: Some("CAT smooth indices"), contents: bytemuck::cast_slice(&indices), usage: wgpu::BufferUsages::INDEX,
            }),
            draws: smooth_draws,
            samples: device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: Some("CAT midpoint endpoints"), contents: bytemuck::cast_slice(&subdiv.samples), usage: wgpu::BufferUsages::STORAGE,
            }),
            counts: device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: Some("CAT smoothing counts"), contents: bytemuck::cast_slice(&[mesh.vertices.len() as u32, subdiv.samples.len() as u32, 0, 0]), usage: wgpu::BufferUsages::UNIFORM,
            }),
            vertex_count: subdiv.vertices.len() as u32,
        })
    }
}

impl Renderer {
    pub(super) fn prepare_actor_smoothing(&mut self) {
        if !self.act.items.iter().any(|item| item.smoothing && item.mesh == 0) { return; }
        let (pipeline, layout) = self.act.smoothing_pipeline.get_or_insert_with(|| {
            let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("CAT smoothing bindings"),
                entries: &std::array::from_fn::<_, 4, _>(|binding| wgpu::BindGroupLayoutEntry {
                    binding: binding as u32, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: if binding == 3 { wgpu::BufferBindingType::Uniform } else { wgpu::BufferBindingType::Storage { read_only: binding != 2 } },
                        has_dynamic_offset: false, min_binding_size: None,
                    }, count: None,
                }),
            });
            let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("CAT smoothing layout"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
            });
            let device = self.device.clone();
            let pipeline = crate::pipeline_creation::ComputePipeline::new(move || {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("CAT posed Phong midpoints"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("character_smoothing.wgsl", include_str!("character_smoothing.wgsl"))),
                });
                crate::pipeline_creation::compute(&device, &wgpu::ComputePipelineDescriptor {
                    label: Some("CAT smoothing"), layout: Some(&pipeline_layout), module: &shader, entry_point: Some("subdivide_pose"), compilation_options: Default::default(), cache: None,
                })
            });
            (pipeline, layout)
        });
        // Geometry must exist before the first enhanced draw, including frozen-pose toggles.
        pipeline.get();
        for item in self.act.items.iter().filter(|item| item.smoothing && item.mesh == 0) {
            let body = self.act.models[&item.model].meshes[0].as_ref().unwrap();
            let smooth = body.smoothing.as_ref().unwrap();
            let actor = self.act.actors.get_mut(&item.actor).unwrap();
            let source_is_skin = actor.vb.is_some();
            if actor.smoothing.as_ref().is_some_and(|s| s.source_is_skin == source_is_skin) { continue; }
            let vertices = device_vertices(&self.device, smooth.vertex_count);
            let source = actor.vb.as_ref().unwrap_or(&body.vb);
            let bind = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                label: Some("CAT posed smoothing resources"), layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: source.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: smooth.samples.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: vertices.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: smooth.counts.as_entire_binding() },
                ],
            });
            actor.smoothing = Some(ActorGpu { vertices, bind, source_is_skin });
        }
    }

    pub(super) fn encode_actor_smoothing(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.enhanced_settings().character_smoothing { return; }
        let Some((pipeline, _)) = &self.act.smoothing_pipeline else { return; };
        if !self.act.items.iter().any(|item| item.smoothing && item.mesh == 0) { return; }
        let timestamps = self.gpu_timer.as_mut().map(crate::FrameGpuTimer::begin_compute_pass);
        let mut pass = encoder.begin_effect_compute_pass(&wgpu::ComputePassDescriptor { label: Some("CAT character smoothing"), timestamp_writes: timestamps });
        pass.set_pipeline(pipeline);
        for item in self.act.items.iter().filter(|item| item.smoothing && item.mesh == 0) {
            let actor = self.act.actors[&item.actor].smoothing.as_ref().unwrap();
            let mesh = self.act.models[&item.model].meshes[0].as_ref().unwrap().smoothing.as_ref().unwrap();
            pass.set_bind_group(0, &actor.bind, &[]);
            pass.dispatch_workgroups(mesh.vertex_count.div_ceil(64), 1, 1);
        }
    }
}

fn device_vertices(device: &wgpu::Device, count: u32) -> wgpu::Buffer {
    device.create_buffer_tracked(&wgpu::BufferDescriptor {
        label: Some("CAT smooth posed vertices"), size: u64::from(count) * 48,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
    })
}
