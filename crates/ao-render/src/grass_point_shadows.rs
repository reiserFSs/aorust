//! GPU-selected, range-bounded native grass cell lists for point shadow cubes.
use super::{GpuStatsExt, Renderer};
use crate::grass::{Grass, BANDS, MAX_CELLS};
use crate::sun_shadows::{GrassCaster, ShadowUniform};

pub(crate) struct GrassPointShadows {
    cells: wgpu::Buffer,
    count: usize,
    params: wgpu::Buffer,
    visible: wgpu::Buffer,
    indirect: wgpu::Buffer,
    select: crate::pipeline_creation::ComputePipeline,
    select_group: wgpu::BindGroup,
    caster: GrassCaster,
    faces: Vec<wgpu::BindGroup>,
}
impl GrassPointShadows {
    pub(crate) fn matches(&self, grass: &Grass, count: usize) -> bool {
        self.cells == grass.cells_buffer && self.count == count
    }
    pub(crate) fn new(renderer: &Renderer, grass: &Grass, face_data: &wgpu::Buffer, face_stride: u64, count: usize) -> Self {
        let device = &renderer.device;
        let per_light = (BANDS * MAX_CELLS * 4) as u64;
        let buffer = |label, size, usage| device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some(label), size, usage, mapped_at_creation: false,
        });
        let visible = buffer("range-bounded grass point cells", per_light * count as u64, wgpu::BufferUsages::STORAGE);
        let indirect = buffer("grass point band indirect draws", (count * BANDS * 16) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC);
        let params = buffer("grass point counts and strides", 80, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let entries: Vec<_> = (0..6).map(|binding| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: if binding == 2 { wgpu::BufferBindingType::Uniform }
                else { wgpu::BufferBindingType::Storage { read_only: binding < 4 } },
                has_dynamic_offset: false, min_binding_size: None }, count: None,
        }).collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("selected point lights and native grass cells"), entries: &entries,
        });
        let select_group = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("point grass cell compaction"), layout: &layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: grass.cells_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: grass.visible_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: face_data.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: visible.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: indirect.as_entire_binding() },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bounded grass point cell selection"),
            source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("grass_point_select.wgsl", include_str!("grass_point_select.wgsl"))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point grass cell compaction"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let select = crate::pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("point grass cell compaction"), layout: Some(&pipeline_layout), module: &shader,
            entry_point: Some("select_grass_cells"), compilation_options: Default::default(), cache: None,
        });
        let caster = GrassCaster::new(renderer);
        let faces = (0..count * 6).map(|face| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("selected grass point face"), layout: &caster.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: grass.cells_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: grass.params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: face_data, offset: (count * 6 + face) as u64 * face_stride, size: wgpu::BufferSize::new(std::mem::size_of::<ShadowUniform>() as u64),
                }) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &visible, offset: (face / 6) as u64 * per_light, size: wgpu::BufferSize::new(per_light),
                }) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&grass.noise_view) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&grass.noise_sampler) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&grass.mask_view) },
            ],
        })).collect();
        Self { cells: grass.cells_buffer.clone(), count, params, visible, indirect, select, select_group, caster, faces }
    }
    pub(crate) fn encode(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, grass: &Grass, face_stride: u64) {
        let counts = grass.point_shadow_counts();
        let strides = grass.point_shadow_strides();
        let parameters = [counts[0], counts[1], strides[0], strides[1], [(face_stride / 16) as u32, self.count as u32, (self.count * 6) as u32, 0]];
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&parameters));
        let commands: [[u32; 4]; 16 * BANDS] = std::array::from_fn(|index| {
            let band = (index % BANDS) as u32;
            // Sparse lamp bands may contain near-camera blades: preserve both
            // triangles until even their original camera band has collapsed.
            [if band >= 4 { 3 } else { 6 }, 0, band * 6, 0]
        });
        queue.write_buffer(&self.indirect, 0, bytemuck::cast_slice(&commands[..self.count * BANDS]));
        let cells = counts.into_iter().flatten().max().unwrap_or(0);
        if cells == 0 { return; }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("range-bounded admitted grass point cells"), timestamp_writes: None,
        });
        pass.set_pipeline(&self.select);
        pass.set_bind_group(0, &self.select_group, &[]);
        pass.dispatch_workgroups(cells.div_ceil(64), BANDS as u32, self.count as u32);
    }
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, globals: &wgpu::BindGroup, face: usize) {
        pass.set_pipeline(&self.caster.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(3, &self.faces[face], &[]);
        for band in 0..BANDS {
            pass.draw_indirect(&self.indirect, ((face / 6 * BANDS + band) * 16) as u64);
        }
    }
    pub(crate) fn resource_bytes(&self) -> usize {
        self.visible.size() as usize + self.indirect.size() as usize + self.params.size() as usize
    }
}
