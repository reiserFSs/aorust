//! Optional celestial detail over the native sky. Admission is metadata-only; poses
//! are the very matrices uploaded for the current native sky frame.
use crate::effect_gpu::EffectPassEncoder as _;
use std::collections::HashMap;
use ao_scene::{Scene, TextureKey};
use glam::{Mat4, Vec3};
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};

#[derive(Clone)]
struct Source {
    instance: usize,
    texture: TextureKey,
    vertices: Vec<ao_scene::Vertex>,
    indices: Vec<u32>,
    center: Vec3,
    material: ao_scene::Submesh,
    sun: bool,
}
#[derive(Default)]
pub(super) struct DiscSources(Vec<Source>);
impl DiscSources {
    pub(super) fn from_scene(scene: &Scene) -> Self { Self(sources(scene)) }
}


fn sources(scene: &Scene) -> Vec<Source> {
    let mut result = Vec::new();
    for (instance, sky) in scene.sky.iter().enumerate() {
        let Some(mesh) = scene.meshes.get(sky.mesh) else { continue; };
        let moon = scene.moons.iter().flatten().find(|moon| moon.sky_instance == instance);
        for material in &mesh.submeshes {
            if !material.sun_flicker && moon.is_none() { continue; }
            let Some(texture) = material.texture else { continue; };
            if material.indices.is_empty() || material.indices.iter().any(|&i| i as usize >= mesh.vertices.len()) { continue; }
            let center = if material.sun_flicker {
                let Some(vertex) = material.indices.iter().map(|&i| &mesh.vertices[i as usize]).find(|v| v.normal[2] < 0.0) else { continue; };
                Vec3::from(vertex.pos)
            } else {
                Vec3::from(moon.expect("moon admission checked").resource_center)
            };
            result.push(Source { instance, texture, vertices: mesh.vertices.clone(), indices: material.indices.clone(), center, material: material.clone(), sun: material.sun_flicker });
        }
    }
    result
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    clip: [f32; 4],
    uv: [f32; 2],
    color: [f32; 4],
    radial: [f32; 2],
    ray: [f32; 3],
    sun: f32,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params { control: [f32; 4] }

fn low_sun_scale(sun: bool, direction: Vec3) -> glam::Vec2 {
    if !sun { return glam::Vec2::ONE; }
    let height = direction.normalize_or_zero().y.abs();
    let t = (height / 0.3).clamp(0.0, 1.0);
    let low = 1.0 - t * t * (3.0 - 2.0 * t);
    glam::Vec2::new(1.0 + 0.5 * low, 1.0 + 0.2 * low)
}

fn disc_channel(vertex: f32, emissive: f32, base: f32) -> f32 {
    (crate::gamma(vertex) + crate::gamma(emissive)).min(1.0) * crate::gamma(base)
}

/// No history and no owned colour targets. Disabling releases cached frame bindings.
pub(super) struct SkyDiscs {
    pipeline: RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    scratch: Vec<Vertex>,
    vertices: Option<wgpu::Buffer>,
    capacity: u64,
    layers: Option<wgpu::Buffer>,
    layer_capacity: u64,
    draws: Vec<(std::ops::Range<u32>, TextureKey)>,
    bindings: HashMap<TextureKey, (wgpu::TextureView, wgpu::TextureView, wgpu::TextureView, wgpu::BindGroup)>,
}

impl SkyDiscs {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("authored celestial inputs"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            texture(1, wgpu::TextureSampleType::Depth), texture(2, wgpu::TextureSampleType::Float { filterable: true }),
            wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            texture(5, wgpu::TextureSampleType::Float { filterable: false }),
        ] });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("authored sky discs"), source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", crate::shader_source::load("cloud_density.wgsl", include_str!("cloud_density.wgsl")), crate::shader_source::load("sky_discs.wgsl", include_str!("sky_discs.wgsl"))).into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Float32x4, 3 => Float32x2, 4 => Float32x3, 5 => Float32];
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some("bounded authored celestial detail"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::SrcAlpha, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE }), write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN | wgpu::ColorWrites::BLUE })] }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() }, depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("celestial parameters"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let sampler = device.create_sampler_tracked(&wgpu::SamplerDescriptor { label: Some("authored celestial texture"), min_filter: wgpu::FilterMode::Linear, mag_filter: wgpu::FilterMode::Linear, ..Default::default() });
        Self { pipeline, layout, sampler, params, scratch: vec![], vertices: None, capacity: 0, layers: None, layer_capacity: 0, draws: vec![], bindings: HashMap::new() }
    }
    pub(super) fn is_ready(&self) -> bool { self.pipeline.is_ready() }
    pub(super) fn wait(&self) { self.pipeline.get(); }
    pub(super) fn release_targets(&mut self) {
        self.vertices = None; self.layers = None; self.capacity = 0; self.layer_capacity = 0;
        self.bindings.clear();
    }
    pub(super) fn target_bytes(&self) -> u64 { self.capacity + self.layer_capacity }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView, directional_mask: &wgpu::TextureView, colour: &wgpu::TextureView, size: (u32, u32), vp: Mat4, eye: Vec3,
        current_sky: &[[[f32; 4]; 4]], textures: &HashMap<TextureKey, wgpu::TextureView>,
        time: f32, daylight: Option<f32>, layers: &[crate::volumetric_clouds::CloudDensityLayer], sources: &DiscSources) {
        if size.0 == 0 || size.1 == 0 || !time.is_finite() { return; }
        self.scratch.clear();
        self.draws.clear();
        let inverse_vp = vp.inverse();
        for source in &sources.0 {
            let Some(matrix) = current_sky.get(source.instance) else { continue; };
            if !textures.contains_key(&source.texture) { continue; }
            let matrix = Mat4::from_cols_array_2d(matrix);
            let center = matrix.transform_point3(source.center);
            let center_clip = vp * center.extend(1.0);
            if center_clip.w <= 0.0 { continue; }
            let center_ndc = center_clip.truncate() / center_clip.w;
            let scale = low_sun_scale(source.sun, center - eye);
            let aspect = size.0 as f32 / size.1 as f32;
            let mut radius = 0.0f32;
            for &index in &source.indices {
                let clip = vp * matrix.transform_point3(Vec3::from(source.vertices[index as usize].pos)).extend(1.0);
                if clip.w > 0.0 {
                    let delta = clip.truncate() / clip.w - center_ndc;
                    radius = radius.max(glam::Vec2::new(delta.x * aspect, delta.y).length());
                }
            }
            if radius <= f32::EPSILON || !radius.is_finite() { continue; }
            let first = self.scratch.len() as u32;
            let flicker = ao_scene::sun_flicker_table(time);
            for &index in &source.indices {
                let vertex = source.vertices[index as usize];
                let world = matrix.transform_point3(Vec3::from(vertex.pos));
                let mut clip = vp * world.extend(1.0);
                let ndc = clip.truncate() / clip.w;
                // Enlarge the authored sun fan about its unchanged current centre,
                // not a fixed angular Earth sun. Moon geometry is unchanged.
                clip.x = (center_ndc.x + (ndc.x - center_ndc.x) * scale.x) * clip.w;
                clip.y = (center_ndc.y + (ndc.y - center_ndc.y) * scale.y) * clip.w;
                let enlarged_world = inverse_vp * clip;
                let ray = enlarged_world.truncate() / enlarged_world.w - eye;
                let mut color = [0.0; 4];
                for (channel, value) in color.iter_mut().take(3).enumerate() {
                    *value = disc_channel(vertex.color[channel], source.material.emissive[channel], source.material.base_color[channel]);
                }
                color[3] = vertex.color[3] * source.material.base_color[3];
                if source.sun && vertex.normal[2] >= 0.0 { color[3] *= 1.0 - flicker[(vertex.normal[2] + 0.5) as usize & 7] as f32 / 255.0; }
                self.scratch.push(Vertex { clip: clip.to_array(), uv: vertex.uv, color, radial: [(ndc.x - center_ndc.x) * aspect / radius, (ndc.y - center_ndc.y) / radius], ray: ray.to_array(), sun: u32::from(source.sun) as f32 });
            }
            self.draws.push((first..self.scratch.len() as u32, source.texture));
        }
        if self.draws.is_empty() { return; }
        let bytes = bytemuck::cast_slice(&self.scratch);
        if bytes.len() as u64 > self.capacity {
            self.capacity = (bytes.len() as u64).next_power_of_two();
            self.vertices = Some(device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("current authored discs"), size: self.capacity, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        }
        let layer_bytes = (std::mem::size_of_val(layers) as u64).max(std::mem::size_of::<crate::volumetric_clouds::CloudDensityLayer>() as u64);
        if layer_bytes > self.layer_capacity {
            self.layer_capacity = layer_bytes.next_power_of_two();
            self.layers = Some(device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("celestial cloud attenuation"), size: self.layer_capacity, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
            self.bindings.clear();
        }
        if !layers.is_empty() { queue.write_buffer(self.layers.as_ref().unwrap(), 0, bytemuck::cast_slice(layers)); }
        queue.write_buffer(self.vertices.as_ref().unwrap(), 0, bytes);
        let daylight = daylight.filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params { control: [time, daylight, layers.len() as f32, 1_000_000.0] }));
        for (_, key) in &self.draws {
            let texture = &textures[key];
            if self.bindings.get(key).is_some_and(|(old_depth, old_texture, old_mask, _)| old_depth == depth && old_texture == texture && old_mask == directional_mask) { continue; }
            let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("current celestial source"), layout: &self.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(texture) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: self.layers.as_ref().unwrap().as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(directional_mask) },
            ] });
            self.bindings.insert(*key, (depth.clone(), texture.clone(), directional_mask.clone(), bind));
        }
        let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor { label: Some("authored celestial detail"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: colour, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertices.as_ref().unwrap().slice(..));
        for (range, key) in &self.draws { pass.set_bind_group(0, &self.bindings[key].3, &[]); pass.draw(range.clone(), 0..1); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disc_tint_is_encoded_once_for_the_native_gamma_target() {
        let linear = |v: f32| v.powf(2.2);
        assert!((disc_channel(linear(0.25), linear(0.125), linear(0.5)) - 0.1875).abs() < 1e-6);
        assert!((disc_channel(linear(0.8), linear(0.8), linear(0.5)) - 0.5).abs() < 1e-6);
        assert_eq!(disc_channel(-1.0, 0.0, 1.0), 0.0);
    }
    #[test]
    fn low_sun_growth_is_bounded_and_never_changes_moon_radius() {
        assert_eq!(low_sun_scale(false, Vec3::X), glam::Vec2::ONE);
        assert_eq!(low_sun_scale(true, Vec3::Y), glam::Vec2::ONE);
        assert_eq!(low_sun_scale(true, Vec3::X), glam::Vec2::new(1.5, 1.2));
        let middle = low_sun_scale(true, Vec3::new(1.0, 0.15, 0.0));
        assert!(middle.x > 1.0 && middle.x < 1.5);
        assert!(middle.y > 1.0 && middle.y < 1.2);
    }

    #[test]
    fn metadata_admission_keeps_both_suns_and_current_instance_identity() {
        let key = TextureKey { rdb_type: 0, id: 7 };
        let mut material = ao_scene::Submesh::new(vec![0, 1, 2], Some(key));
        let mesh = ao_scene::Mesh { vertices: vec![ao_scene::Vertex { normal: [0.0, 0.0, -1.0], ..Default::default() }; 3], submeshes: vec![material.clone()] };
        let mut scene = Scene { meshes: vec![mesh], sky: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }; 2], ..Default::default() };
        assert!(sources(&scene).is_empty(), "no metadata must not guess suns from colour");
        material.sun_flicker = true; scene.meshes[0].submeshes[0] = material;
        let admitted = sources(&scene);
        assert_eq!(admitted.len(), 2); assert_eq!(admitted[1].instance, 1);
        let current = Mat4::from_translation(Vec3::new(17.0, 3.0, -8.0));
        assert_eq!(current.transform_point3(admitted[1].center), Vec3::new(17.0, 3.0, -8.0));
        scene.meshes[0].submeshes[0].sun_flicker = false;
        scene.moons[0] = Some(ao_scene::MoonSource { sky_instance: 1, resource_center: [2.0, 0.0, 0.0], ..Default::default() });
        let admitted = sources(&scene); assert_eq!(admitted.len(), 1); assert!(!admitted[0].sun);
        assert_eq!(current.transform_point3(admitted[0].center), Vec3::new(19.0, 3.0, -8.0));
    }
}
