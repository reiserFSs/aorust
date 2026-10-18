//! Current-admitted-depth-only, quarter-resolution diffuse bounce (ao-vk horizon gather).
use crate::effect_gpu::EffectPassEncoder as _;
use crate::gpu_stats::GpuStatsExt;
use crate::pipeline_creation::RenderPipeline;
use glam::{Mat4, Vec3};

const TARGET: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse_vp: [[f32; 4]; 4],
    vp: [[f32; 4]; 4],
    eye: [f32; 4],
    control: [f32; 4], // strength, radius, room BVH node count, indoors
}
#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Room { min: [f32; 4], max: [f32; 4] }
fn packed_room(room: &ao_scene::RoomVolume) -> Room {
    Room { min: [room.min[0], room.min[1], room.min[2], room.room as f32], max: [room.max[0], room.max[1], room.max[2], 0.0] }
}

fn room_upload_size(count: usize, limit: u64) -> Option<u64> {
    let nodes = if count == 0 { 0 } else { count.checked_mul(2)?.checked_sub(1)? };
    // Escape indices and original-input priorities are stored as exact f32s.
    if nodes > (1 << 24) { return None; }
    let bytes = u64::try_from(nodes).ok()?.checked_mul(std::mem::size_of::<Room>() as u64)?.max(32);
    if bytes > limit { return None; }
    Some(bytes.checked_next_power_of_two().unwrap_or(limit).min(limit))
}

// Built only when native volumes change. Preorder internal nodes escape past
// their entire subtree; leaves retain original input priority for overlaps.
fn build_room_bvh(leaves: &mut [Room], nodes: &mut Vec<Room>) {
    if leaves.is_empty() { return; }
    if leaves.len() == 1 { nodes.push(leaves[0]); return; }
    let start = nodes.len();
    let mut bounds = Room { min: [f32::INFINITY; 4], max: [f32::NEG_INFINITY; 4] };
    for leaf in leaves.iter() {
        for axis in 0..3 {
            bounds.min[axis] = bounds.min[axis].min(leaf.min[axis]);
            bounds.max[axis] = bounds.max[axis].max(leaf.max[axis]);
        }
    }
    bounds.min[3] = -1.0;
    bounds.max[3] = 0.0;
    nodes.push(bounds);
    let axis = (0..3).max_by(|&a, &b| (bounds.max[a] - bounds.min[a]).total_cmp(&(bounds.max[b] - bounds.min[b]))).unwrap_or(0);
    let middle = leaves.len() / 2;
    leaves.select_nth_unstable_by(middle, |a, b| (a.min[axis] * 0.5 + a.max[axis] * 0.5).total_cmp(&(b.min[axis] * 0.5 + b.max[axis] * 0.5)));
    let (left, right) = leaves.split_at_mut(middle);
    build_room_bvh(left, nodes);
    build_room_bvh(right, nodes);
    nodes[start].max[3] = nodes.len() as f32;
}
pub(super) struct IndirectLight {
    layout: wgpu::BindGroupLayout,
    source_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    rooms: wgpu::Buffer,
    room_values: Vec<Room>,
    room_nodes: Vec<Room>,
    rooms_dirty: bool,
    pipelines: [RenderPipeline; 5],
    targets: Option<Targets>,
}
struct Targets {
    size: (u32, u32),
    inputs: [wgpu::TextureView; 3],
    sky: wgpu::Buffer,
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    source: wgpu::TextureView,
    source_bind: wgpu::BindGroup,
    from_a: wgpu::BindGroup,
    from_b: wgpu::BindGroup,
}
impl IndirectLight {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let mut entries = vec![
            buffer(0, wgpu::BufferBindingType::Uniform), texture(1, wgpu::TextureSampleType::Depth),
            texture(2, wgpu::TextureSampleType::Float { filterable: false }), texture(3, wgpu::TextureSampleType::Float { filterable: false }),
            buffer(6, wgpu::BufferBindingType::Storage { read_only: true }), buffer(7, wgpu::BufferBindingType::Storage { read_only: true }),
        ];
        // The source prepass never binds its own output, even as an unused input.
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("indirect light source inputs"), entries: &entries });
        entries.push(texture(5, wgpu::TextureSampleType::Float { filterable: false }));
        entries.push(texture(8, wgpu::TextureSampleType::Uint));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("indirect light inputs"), entries: &entries });
        let mut source = crate::shader_source::load("indirect_light.wgsl", include_str!("indirect_light.wgsl")).into_owned();
        source.push_str(&crate::shader_source::load("moon_exposure.wgsl", include_str!("moon_exposure.wgsl")).replace("@group(3) @binding(7)", "@group(0) @binding(6)"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("indirect light"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("indirect light"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let source_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("indirect light source cache"), bind_group_layouts: &[Some(&source_layout)], immediate_size: 0 });
        let pipelines = ["source_cache", "gather", "blur_x1", "blur_y1", "composite"].map(|entry| {
            let composite = entry == "composite";
            crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
                label: Some(entry), layout: Some(if entry == "source_cache" { &source_pipeline_layout } else { &pipeline_layout }),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState {
                    format: if composite { format } else if entry == "source_cache" { wgpu::TextureFormat::Rgba32Uint } else { TARGET },
                    blend: composite.then_some(wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE }),
                    write_mask: if composite { wgpu::ColorWrites::COLOR } else { wgpu::ColorWrites::ALL },
                })] }), primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
            })
        });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("indirect light parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let rooms = Self::room_buffer(device, 32);
        Self { layout, source_layout, params, rooms, room_values: Vec::new(), room_nodes: Vec::new(), rooms_dirty: true, pipelines, targets: None }
    }
    fn room_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("indirect light room volumes"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }
    pub(super) fn is_ready(&self) -> bool { self.pipelines.iter().all(RenderPipeline::is_ready) }
    pub(super) fn wait(&self) { for pipeline in &self.pipelines { pipeline.get(); } }
    pub(super) fn release_targets(&mut self) { self.targets = None; }
    /// Call after replacing the renderer's scene room volumes.
    pub(super) fn invalidate_rooms(&mut self) { self.rooms_dirty = true; }
    pub(super) fn target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |t| Self::incremental_bytes(t.size) + self.rooms.size() - 32)
    }
    pub(super) fn incremental_bytes(size: (u32, u32)) -> u64 {
        u64::from(size.0.div_ceil(4)) * u64::from(size.1.div_ceil(4)) * 32 + std::mem::size_of::<Params>() as u64 + 32
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, diffuse: &wgpu::TextureView, albedo: &wgpu::TextureView, sky: &wgpu::Buffer, rooms: &[ao_scene::RoomVolume], indoors: bool, colour: &wgpu::TextureView, size: (u32, u32), vp: Mat4, eye: Vec3, strength: f32) {
        if !strength.is_finite() || strength <= 0.0 || size.0 == 0 || size.1 == 0 { return; }
        let limit = device.limits().max_storage_buffer_binding_size.min(device.limits().max_buffer_size);
        let room_size = room_upload_size(rooms.len(), limit);
        // Oversized native room data is unknown, never a partially admitted set.
        // Close the domain even outdoors so omitted room walls cannot leak light.
        let indoors = indoors || room_size.is_none();
        let rooms = if room_size.is_some() { rooms } else { &[] };
        if self.rooms_dirty {
            self.room_values.clear();
            self.room_values.extend(rooms.iter().map(packed_room));
            for (priority, leaf) in self.room_values.iter_mut().enumerate() { leaf.max[3] = priority as f32; }
            self.room_nodes.clear();
            build_room_bvh(&mut self.room_values, &mut self.room_nodes);
            let bytes = bytemuck::cast_slice(&self.room_nodes);
            if bytes.len() as u64 > self.rooms.size() {
                self.rooms = Self::room_buffer(device, room_size.expect("bounded nonempty room upload"));
                self.targets = None;
            }
            if !bytes.is_empty() { queue.write_buffer(&self.rooms, 0, bytes); }
            self.rooms_dirty = false;
        }
        let inputs = [depth, diffuse, albedo];
        if self.targets.as_ref().is_none_or(|t| t.size != size || t.sky != *sky || t.inputs.iter().zip(inputs).any(|(a, b)| a != b)) {
            let create = |label, format| device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width: size.0.div_ceil(4), height: size.1.div_ceil(4), depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
            let a = create("indirect light quarter A", TARGET);
            let b = create("indirect light quarter B", TARGET);
            let source = create("indirect light admitted donor cache", wgpu::TextureFormat::Rgba32Uint);
            let source_bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("indirect light source inputs"), layout: &self.source_layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(diffuse) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(albedo) },
                wgpu::BindGroupEntry { binding: 6, resource: sky.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: self.rooms.as_entire_binding() },
            ] });
            let bind = |input| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("indirect light frame inputs"), layout: &self.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(diffuse) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(albedo) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(input) },
                wgpu::BindGroupEntry { binding: 6, resource: sky.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: self.rooms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&source) },
            ] });
            self.targets = Some(Targets { size, inputs: inputs.map(Clone::clone), sky: sky.clone(), from_a: bind(&a), from_b: bind(&b), a, b, source, source_bind });
        }
        let reference = eye.as_dvec3();
        let matrix = vp.as_dmat4();
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params {
            inverse_vp: (glam::DMat4::from_translation(-reference) * matrix.inverse()).as_mat4().to_cols_array_2d(),
            vp: (matrix * glam::DMat4::from_translation(reference)).as_mat4().to_cols_array_2d(),
            eye: eye.extend(0.0).to_array(), control: [strength.clamp(0.0, 3.0), 3.0, self.room_nodes.len() as f32, if indoors { 1.0 } else { 0.0 }],
        }));
        let t = self.targets.as_ref().expect("indirect light targets allocated");
        for (index, label, input, output) in [
            (0, "indirect light donor cache", &t.source_bind, &t.source),
            (1, "indirect light horizon gather", &t.from_b, &t.a),
            (2, "indirect light bilateral x1", &t.from_a, &t.b),
            (3, "indirect light bilateral y1", &t.from_b, &t.a),
            (4, "indirect light albedo composite", &t.from_a, colour),
        ] {
            let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor { label: Some(label), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: if index == 4 { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) }, store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(&self.pipelines[index]);
            pass.set_bind_group(0, input, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quarter_targets_and_shader_transfer_contract() {
        assert_eq!(IndirectLight::incremental_bytes((5, 9)), 2 * 3 * 32 + std::mem::size_of::<Params>() as u64 + 32);
        assert_eq!(room_upload_size(2, 100), Some(100));
        assert_eq!(room_upload_size(3, 100), None);
        assert_eq!(room_upload_size(usize::MAX, 100), None);
        assert_eq!(room_upload_size(0, 100), Some(32));
        let room = ao_scene::RoomVolume { min: [1.0, 2.0, 3.0], max: [4.0, 5.0, 6.0], room: 7 };
        assert_eq!(packed_room(&room).min, [1.0, 2.0, 3.0, 7.0]);
        let shader = include_str!("indirect_light.wgsl");
        for contract in ["const FADE_NEAR = 50.0", "const FADE_FAR = 80.0", "FADE_FAR + params.control.y", "smoothstep(FADE_NEAR, FADE_FAR", "fn source_cache(", "fn transfer(", "fn room_at(", "enhanced_sky_open", "enhanced_sky_known", "fn gather(", "fn blur_x1(", "fn blur_y1(", "fn composite(", "textureLoad(diffuse", "textureLoad(albedo", "pack2x16snorm"] { assert!(shader.contains(contract), "missing {contract}"); }
        assert_eq!(shader.matches("@fragment fn blur_").count(), 2);
        assert!(shader.contains("return blur(p, vec2<i32>(1, 0))"));
        assert!(shader.contains("return blur(p, vec2<i32>(0, 1))"));
        assert!(shader.contains("for (var i = -2; i <= 2; i++)"));
        assert!(!shader.contains("history"));
        assert!(shader.contains("fn transfer(p: vec3<f32>, q: vec3<f32>, room: u32)"));
        assert!(shader.contains("room_at(mid) != room"));
        assert!(shader.contains("clip.w - surface_clip.w > 0.08"));
    }

    #[test]
    fn stackless_rooms_match_first_input_and_remain_balanced() {
        fn contains(node: &Room, p: [f32; 3]) -> bool {
            (0..3).all(|axis| p[axis] >= node.min[axis] && p[axis] <= node.max[axis])
        }
        fn query(nodes: &[Room], p: [f32; 3]) -> u32 {
            let (mut index, mut priority, mut room) = (0, f32::INFINITY, 0);
            while index < nodes.len() {
                let node = &nodes[index];
                let inside = contains(node, p);
                if node.min[3] < 0.0 {
                    index = if inside { index + 1 } else { node.max[3] as usize };
                } else {
                    if inside && node.max[3] < priority {
                        priority = node.max[3];
                        room = node.min[3] as u32;
                        if priority == 0.0 { break; }
                    }
                    index += 1;
                }
            }
            room
        }
        fn height(nodes: &[Room], index: usize) -> usize {
            if nodes[index].min[3] >= 0.0 { return 1; }
            let left = index + 1;
            let right = if nodes[left].min[3] < 0.0 { nodes[left].max[3] as usize } else { left + 1 };
            1 + height(nodes, left).max(height(nodes, right))
        }
        let mut leaves: Vec<_> = (0..65).map(|i| Room {
            min: [i as f32 * 2.0, -1.0, -1.0, (100 - i) as f32],
            max: [i as f32 * 2.0 + 3.0, 1.0, 1.0, i as f32],
        }).collect();
        leaves[2].min[3] = 0.0; // Explicit unknown volumes still win by input order.
        let original = leaves.clone();
        let mut nodes = Vec::new();
        build_room_bvh(&mut leaves, &mut nodes);
        assert_eq!(nodes.len(), 129);
        assert!(height(&nodes, 0) <= 8);
        for x in -6..270 {
            for y in [0.0, 2.0] {
                let p = [x as f32 * 0.5, y, 0.0];
                let linear = original.iter().find(|room| contains(room, p)).map_or(0, |room| room.min[3] as u32);
                assert_eq!(query(&nodes, p), linear, "at {p:?}");
            }
        }
        let mut empty = Vec::new();
        build_room_bvh(&mut [], &mut empty);
        assert_eq!(query(&empty, [0.0; 3]), 0);
    }
}
