//! Exact GPU-visible actor-pose IDs, independent of hidden CPU membership churn.
use super::*;

pub(super) struct VisibilityPrograms {
    layout: wgpu::BindGroupLayout,
    collect: crate::pipeline_creation::ComputePipeline,
    compare: crate::pipeline_creation::ComputePipeline,
}
impl VisibilityPrograms {
    pub fn new(device: &wgpu::Device, samples: u32) -> Self {
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("exact visible actor IDs"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: samples > 1 }, count: None },
                storage(1, false), storage(2, true), storage(3, false), storage(4, true),
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let source = crate::shader_source::load("motion_visibility.wgsl", include_str!("motion_visibility.wgsl"));
        let source = if samples > 1 { source.into_owned() } else {
            source.replace("texture_multisampled_2d<u32>", "texture_2d<u32>")
                .replace("i32(textureNumSamples(identities))", "1")
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("exact visible actor IDs"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("exact visible actor IDs"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = |entry| crate::pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("exact visible actor IDs"), layout: Some(&pipeline_layout), module: &shader, entry_point: Some(entry), compilation_options: Default::default(), cache: None,
        });
        Self { layout, collect: pipeline("collect"), compare: pipeline("compare") }
    }
    pub fn is_ready(&self) -> bool { self.collect.is_ready() && self.compare.is_ready() }
    pub fn wait(&self) { self.collect.get(); self.compare.get(); }
}

struct Slots { capacity: u32, free: Vec<u32> }
impl Slots {
    fn new() -> Self { Self { capacity: 1, free: vec![0] } }
    fn allocate(&mut self) -> u32 {
        if self.free.is_empty() {
            let old = self.capacity;
            self.capacity = self.capacity.checked_mul(2).expect("actor visibility slot capacity exhausted");
            self.free.extend((old..self.capacity).rev());
        }
        self.free.pop().unwrap()
    }
    fn release(&mut self, slot: u32) { self.free.push(slot); }
}

pub(super) struct Visibility {
    slots: Slots,
    capacity: u32,
    current: wgpu::Buffer,
    previous: wgpu::Buffer,
    pending_previous: Option<(wgpu::Buffer, u64)>,
    changed: wgpu::Buffer,
    table: wgpu::Buffer,
    params: wgpu::Buffer,
    group: wgpu::BindGroup,
    identity: wgpu::TextureView,
    rows: Vec<[u32; 2]>,
    table_dirty: bool,
}
fn words(device: &wgpu::Device, size: u64, label: &'static str) -> wgpu::Buffer {
    device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some(label), size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false })
}
impl Visibility {
    pub fn new(device: &wgpu::Device, programs: &VisibilityPrograms, identity: &wgpu::TextureView) -> Self {
        let current = words(device, 4, "current visible actor IDs");
        let previous = words(device, 4, "previous visible actor IDs");
        let changed = words(device, 4, "visible actor IDs changed");
        let table = words(device, 8, "exact actor ID slot table");
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("visible actor IDs parameters"), size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = Self::group(device, programs, identity, [&current, &previous, &changed, &table, &params]);
        Self { slots: Slots::new(), capacity: 1, current, previous, pending_previous: None, changed, table, params, group,
            identity: identity.clone(), rows: Vec::new(), table_dirty: true }
    }
    fn group(device: &wgpu::Device, programs: &VisibilityPrograms, identity: &wgpu::TextureView, buffers: [&wgpu::Buffer; 5]) -> wgpu::BindGroup {
        let entries = std::array::from_fn::<_, 6, _>(|binding| wgpu::BindGroupEntry {
            binding: binding as u32, resource: if binding == 0 { wgpu::BindingResource::TextureView(identity) } else { buffers[binding - 1].as_entire_binding() },
        });
        device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("exact visible actor IDs"), layout: &programs.layout, entries: &entries })
    }
    pub fn allocate(&mut self, device: &wgpu::Device, programs: &VisibilityPrograms) -> u32 {
        let slot = self.slots.allocate();
        if self.slots.capacity > self.capacity {
            let capacity = self.slots.capacity;
            let old = std::mem::replace(&mut self.previous, words(device, capacity as u64 * 4, "previous visible actor IDs"));
            // Several CPU updates may grow again before submission. Keep the oldest
            // GPU-populated prefix, not an intervening never-used zero allocation.
            if self.pending_previous.is_none() { self.pending_previous = Some((old, self.capacity as u64 * 4)); }
            self.capacity = capacity;
            self.current = words(device, capacity as u64 * 4, "current visible actor IDs");
            self.table = words(device, capacity as u64 * 8, "exact actor ID slot table");
            self.group = Self::group(device, programs, &self.identity, [&self.current, &self.previous, &self.changed, &self.table, &self.params]);
        }
        self.table_dirty = true;
        slot
    }
    pub fn release(&mut self, slot: u32) { self.slots.release(slot); self.table_dirty = true; }
    pub fn changed(&self) -> &wgpu::Buffer { &self.changed }
    pub fn bytes(&self) -> u64 { self.capacity as u64 * 16 + 20 + self.pending_previous.as_ref().map_or(0, |(_, bytes)| *bytes) }
    pub fn prepare(&mut self, queue: &wgpu::Queue, rows: impl Iterator<Item = [u32; 2]>) {
        if !self.table_dirty { return; }
        self.rows.clear(); self.rows.extend(rows); self.rows.sort_unstable_by_key(|entry| entry[0]);
        if !self.rows.is_empty() { queue.write_buffer(&self.table, 0, bytemuck::cast_slice(&self.rows)); }
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&[self.rows.len() as u32, self.capacity, 0, 0]));
        self.table_dirty = false;
    }
    pub fn begin_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some((previous, bytes)) = self.pending_previous.take() {
            encoder.clear_buffer(&self.previous, 0, None);
            encoder.copy_buffer_to_buffer(&previous, 0, &self.previous, 0, bytes);
        }
        encoder.clear_buffer(&self.current, 0, None);
        encoder.clear_buffer(&self.changed, 0, None);
    }
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, programs: &VisibilityPrograms, size: (u32, u32)) {
        if !self.rows.is_empty() {
            let mut pass = encoder.begin_effect_compute_pass(&wgpu::ComputePassDescriptor { label: Some("collect exact visible actor IDs"), timestamp_writes: None });
            pass.set_pipeline(&programs.collect); pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        }
        {
            let mut pass = encoder.begin_effect_compute_pass(&wgpu::ComputePassDescriptor { label: Some("compare exact visible actor IDs"), timestamp_writes: None });
            pass.set_pipeline(&programs.compare); pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(self.capacity.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.current, 0, &self.previous, 0, self.capacity as u64 * 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slots_reuse_retired_words_without_reindexing_survivors() {
        let mut slots = Slots::new();
        let first = slots.allocate(); let survivor = slots.allocate();
        slots.release(first);
        assert_eq!(slots.allocate(), first);
        assert_ne!(survivor, first);
        assert_eq!(slots.capacity, 2);
        let next = slots.allocate();
        assert_eq!(next, 2);
        assert_eq!(slots.capacity, 4);
    }
    #[test]
    fn visibility_compares_exact_semantic_ids_not_membership_or_hashes() {
        let shader = include_str!("motion_visibility.wgsl");
        assert!(shader.contains("atomicStore(&current_visible[actor_slots[first].y], id)"));
        assert!(shader.contains("atomicLoad(&current_visible[slot]) != previous_visible[slot]"));
        assert!(!shader.contains("force_reset"));
    }
}
