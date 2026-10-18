//! Once-per-frame blade roots and geometry, shared by beauty and every shadow view.
//! A single 64-lane workgroup walks each active cell's bounded blade span.
use crate::{GpuStatsExt, grass::{BANDS, MAX_CELLS}};

pub(crate) const STRIDE: u64 = 144 + 3 * 48;
const HEADER: u64 = (MAX_CELLS * 16) as u64;

#[derive(Clone)]
pub(super) struct Compiled {
    layout: wgpu::BindGroupLayout,
    pipeline: crate::pipeline_creation::ComputePipeline,
}
impl Compiled {
    pub(super) fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, entries: &[wgpu::BindGroupLayoutEntry], shader: &wgpu::ShaderModule) -> Self {
        let mut entries: Vec<_> = entries.iter().filter(|entry| matches!(entry.binding, 0 | 1 | 5 | 6 | 7 | 8 | 9))
            .cloned().map(|mut entry| { entry.visibility = wgpu::ShaderStages::COMPUTE; entry }).collect();
        entries.push(wgpu::BindGroupLayoutEntry { binding: 15, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false, min_binding_size: None }, count: None });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("shared grass root computation"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass root cache"), bind_group_layouts: &[Some(globals), None, None, Some(&layout)], immediate_size: 0,
        });
        let pipeline = crate::pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("once per blade native root lighting and shape"), layout: Some(&pipeline_layout), module: shader,
            entry_point: Some("cs_grass_blades"), compilation_options: Default::default(), cache: None,
        });
        Self { layout, pipeline }
    }
    pub(super) fn is_ready(&self) -> bool { self.pipeline.is_ready() }
    pub(super) fn wait(&self) { self.pipeline.get(); }
    pub(super) fn warm(&self) { self.pipeline.warm(); }
}

pub(super) struct Cache {
    pub buffer: wgpu::Buffer,
    capacity: usize,
    spans: Vec<[u32; 4]>,
    bind: Option<wgpu::BindGroup>,
    owner: Option<wgpu::Buffer>,
}
impl Cache {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self { buffer: allocate(device, 1), capacity: 1, spans: vec![[0; 4]; MAX_CELLS], bind: None, owner: None }
    }
    pub(super) fn invalidate(&mut self) { self.bind = None; }
    pub(super) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, visible: &[Vec<u32>; BANDS], strides: &[u32; BANDS]) -> (bool, u32) {
        self.spans.fill([0; 4]);
        let mut count = 0usize;
        for (cells, &stride) in visible.iter().zip(strides) {
            for &cell in cells {
                self.spans[cell as usize] = [count as u32, stride, 0, 0];
                count += stride as usize;
            }
        }
        let mut active = 0usize;
        for &cell in visible.iter().flatten() {
            self.spans[active][2] = cell;
            active += 1;
        }
        let grown = count > self.capacity;
        if grown {
            self.capacity = count.next_power_of_two();
            self.buffer = allocate(device, self.capacity);
            self.bind = None;
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.spans));
        (grown, active as u32)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind(&mut self, device: &wgpu::Device, compiled: &Compiled, cells: &wgpu::Buffer, params: &wgpu::Buffer,
        visible: &wgpu::Buffer, noise: &wgpu::TextureView, sampler: &wgpu::Sampler, mask: &wgpu::TextureView, owner: Option<&wgpu::Buffer>) {
        if self.bind.is_some() && self.owner.as_ref() == owner { return; }
        self.owner = owner.cloned();
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: cells.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: params.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: visible.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(noise) },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(mask) },
            wgpu::BindGroupEntry { binding: 15, resource: self.buffer.as_entire_binding() },
        ];
        if let Some(owner) = owner { entries.push(wgpu::BindGroupEntry { binding: 9, resource: owner.as_entire_binding() }); }
        self.bind = Some(device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("shared current-frame blade roots"), layout: &compiled.layout, entries: &entries,
        }));
    }
    pub(super) fn encode(&self, pass: &mut wgpu::ComputePass<'_>, globals: &wgpu::BindGroup, compiled: &Compiled,
        cells: u32) {
        pass.set_pipeline(&compiled.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(3, self.bind.as_ref().expect("grass root inputs prepared"), &[]);
        pass.dispatch_workgroups(1, cells, 1);
    }
}
fn allocate(device: &wgpu::Device, records: usize) -> wgpu::Buffer {
    device.create_buffer_tracked(&wgpu::BufferDescriptor {
        label: Some("visible blade root cache, not a field-wide blade allocation"), size: HEADER + records as u64 * STRIDE,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_blade_cache_matches_explicit_wgsl_storage_layout() {
        let source = include_str!("grass.wgsl");
        let record = source.split_once("struct GrassBladeShade {").unwrap().1.split_once('}').unwrap().0;
        let section = source.split_once("struct GrassBladeSection {").unwrap().1.split_once('}').unwrap().0;
        let shade_bytes = record.matches("vec4<f32>").count() as u64 * 16;
        let section_bytes = (section.matches("vec4<f32>").count() + section.matches("vec4<u32>").count()) as u64 * 16;
        assert_eq!(shade_bytes, 144);
        assert_eq!(section_bytes, 48);
        assert_eq!(shade_bytes + 3 * section_bytes, STRIDE);
        assert_eq!(STRIDE, 288);
        assert_eq!(section.matches("vec4<f32>").count(), 2);
        assert_eq!(section.matches("vec4<u32>").count(), 1);
        assert!(section.contains("left: vec4<f32>"));
        assert!(section.contains("right: vec4<f32>"));
        assert!(section.contains("appearance: vec4<u32>"));
        assert!(!source.contains("enable f16"));
        assert!(!source.contains("vec4<f16>"));
        let build = source.split_once("fn grass_build_section(").unwrap().1.split_once("fn grass_vertex(").unwrap().0;
        assert_eq!(build.matches("pack2x16float(").count(), 4);
        for pair in ["shape.xy", "shape.zw", "sunlight.xy", "sunlight.zw"] {
            assert!(build.contains(&format!("pack2x16float({pair})")));
        }
        assert!(build.contains("vec4<f32>(left, max(vertex_fog(-(g.view * vec4<f32>(left, 1.0)).z), root_fog))"));
        assert!(build.contains("vec4<f32>(right_edge, max(vertex_fog(-(g.view * vec4<f32>(right_edge, 1.0)).z), root_fog))"));
        let vertex = source.split_once("fn grass_vertex(").unwrap().1.split_once("@compute").unwrap().0;
        assert_eq!(vertex.matches("unpack2x16float(").count(), 4);
        for component in ["x", "y", "z", "w"] {
            assert!(vertex.contains(&format!("unpack2x16float(section.appearance.{component})")));
        }
        assert!(vertex.contains("let edge = select(section.left, section.right, (vertex & 1u) != 0u);"));
        assert!(vertex.contains("o.world = edge.xyz;"));
        assert!(vertex.contains("o.fog = edge.w;"));
        let invalid = source.split_once("if b.identity.y == 0u {").unwrap().1.split_once('}').unwrap().0;
        assert!(invalid.contains("grass_blade_output.blades[index].base.head = vec4<f32>(0.0);"));
        assert!(invalid.contains("continue;"));
        assert!(!invalid.contains("grass_blade_shade("));
        assert!(source.find("continue;").unwrap() < source.find("grass_blade_output.blades[index].base = grass_blade_shade(b);").unwrap());
        assert!(vertex.find("if grass_blade_cache.blades[index].base.head.w == 0.0 { return o; }").unwrap()
            < vertex.find("let section =").unwrap());
        let caster = include_str!("grass_caster.wgsl");
        assert!(caster.find("if blade.base.head.w == 0.0 { return clipped; }").unwrap()
            < caster.find("var world =").unwrap());
        assert!(source.contains("sections: array<GrassBladeSection, 3>"));
        assert_eq!(HEADER, MAX_CELLS as u64 * std::mem::size_of::<[u32; 4]>() as u64);
        assert!(source.contains("struct GrassBladeSpan { offset: u32, count: u32, active_cell: u32, _pad1: u32 }"),
            "compact active IDs occupy only the third word; per-cell offsets/counts must not move");
        assert!(source.contains("let cell_id = grass_blade_output.spans[id.y].active_cell;"));
    }
    #[test]
    fn blade_compute_uses_one_bounded_workgroup_per_active_cell() {
        let source = include_str!("grass.wgsl");
        let compute = source.split_once("@compute @workgroup_size(64)").unwrap().1
            .split_once("@vertex").unwrap().0;
        assert!(compute.contains("let cell_id = grass_blade_output.spans[id.y].active_cell;"));
        assert!(compute.contains("let span = grass_blade_output.spans[cell_id];"));
        assert!(compute.contains("for (var k = id.x; k < span.count; k += 64u) {"));
        assert!(compute.contains("let index = span.offset + k;"));
        let encode = include_str!("grass_blades.rs").split_once("pub(super) fn encode(").unwrap().1
            .split_once("fn allocate(").unwrap().0;
        assert!(encode.contains("pass.dispatch_workgroups(1, cells, 1);"));
    }
}
