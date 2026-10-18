//! Lava FX41 is VisualVolumetricFogSphere, not fire particles.
//! DS10056a6a queues perturbations; 100590fe integrates a persistent60x30 UV grid.
use crate::effect_gpu::EffectPassEncoder as _;
use glam::{Mat4, Vec2, Vec3};
use crate::{frustum_culls_sphere, frustum_planes, gpu_stats::GpuStatsExt};
use crate::pipeline_creation::RenderPipeline;

const WIDTH: usize = 60;
const HEIGHT: usize = 30;
const COUNT: usize = WIDTH * HEIGHT;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex { position: [f32; 2], uv: [f32; 2] }

#[derive(Default)]
pub(super) struct HeatHaze {
    uv: Vec<Vec2>,
    velocity: Vec<Vec2>,
    vertices: Vec<Vertex>,
    active: bool,
    gpu: Option<Gpu>,
}
struct Gpu {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: RenderPipeline,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    target: Option<Target>,
}
struct Target { size: (u32, u32), view: wgpu::TextureView, bind: wgpu::BindGroup }

fn base(index: usize) -> Vec2 { Vec2::new((index % WIDTH) as f32 / 60.0, (index / WIDTH) as f32 / 30.0) }
fn random() -> f32 { (ao_scene::crt_rand() % 1000) as f32 / 1000.0 }
fn perturb(velocity: &mut [Vec2], center: Vec2, radius: f32, strength: f32) {
    for (index, velocity) in velocity.iter_mut().enumerate() {
        let delta = base(index) + *velocity - center;
        let distance = delta.length();
        if distance > 0.0 && distance < radius {
            *velocity += delta / distance * (1.0 - distance / radius).powi(2) * strength;
        }
    }
}
fn integrate(uv: &mut [Vec2], velocity: &mut [Vec2]) {
    for (index, (uv, velocity)) in uv.iter_mut().zip(velocity).enumerate() {
        let previous = *uv;
        *uv += *velocity * 0.1;
        *velocity = *velocity * 0.4 + base(index) - previous;
    }
}
fn indices() -> Vec<u16> {
    let mut indices = Vec::with_capacity((WIDTH - 1) * (HEIGHT - 1) * 6);
    for x in 0..WIDTH - 1 {
        for y in 0..HEIGHT - 1 {
            let a = (y * WIDTH + x) as u16;
            if x % 2 == y % 2 { indices.extend([a, a + 1, a + 60, a + 60, a + 1, a + 61]); }
            else { indices.extend([a, a + 1, a + 61, a, a + 61, a + 60]); }
        }
    }
    indices
}
impl HeatHaze {
    pub fn is_ready(&self) -> bool { self.gpu.as_ref().is_some_and(|gpu| gpu.pipeline.is_ready()) }
    pub fn wait(&self) {
        self.gpu.as_ref().expect("heat haze pipeline warmed at startup").pipeline.get();
    }

    pub fn reset(&mut self) {
        // GC100b1f20 activates offscreen compositor FX3e at environment setup.
        // ActivateFX10063cc1 invokes its init hook10058baf on each activation:
        // three CRT draws per grid node, even though first-viewport UVs overwrite
        // the random startup jitter. Do not defer these draws until lava appears.
        for _ in 0..COUNT * 3 { ao_scene::crt_rand(); }
        self.uv.iter_mut().enumerate().for_each(|(index, uv)| *uv = base(index));
        self.velocity.fill(Vec2::ZERO);
        self.active = false;
    }
    pub fn update<'a>(&mut self, size: (u32, u32), vp: Mat4, eye: Vec3, forward: Vec3, spheres: impl Iterator<Item = &'a [f32; 4]>) {
        self.active = false;
        let mut spheres = spheres.peekable();
        if spheres.peek().is_none() { return; }
        if self.gpu.as_ref().and_then(|gpu| gpu.target.as_ref()).is_some_and(|target| target.size != size) {
            self.uv.iter_mut().enumerate().for_each(|(index, uv)| *uv = base(index));
        }
        let planes = frustum_planes(&vp);
        for sphere in spheres {
            let center = Vec3::new(sphere[0], sphere[1], sphere[2]);
            let depth = (center - eye).dot(forward);
            if depth <= 0.0 || depth >= 100.0 || frustum_culls_sphere(&planes, center, sphere[3]) { continue; }
            if self.uv.is_empty() {
                self.uv.extend((0..COUNT).map(base));
                self.velocity.resize(COUNT, Vec2::ZERO);
                self.vertices.extend((0..COUNT).map(|index| Vertex {
                    position: [(index % WIDTH) as f32 / 59.0 * 2.0 - 1.0, 1.0 - (index / WIDTH) as f32 / 29.0 * 2.0],
                    uv: [0.0; 2],
                }));
            }
            // Native uses the camera-space sphere depth only. The perturbation
            // centre is random in screen UV space, not the projected sphere centre.
            let center = Vec2::new(random(), random());
            let radius = random() * 1.7;
            perturb(&mut self.velocity, center, radius, 0.005 / depth.max(1.0));
            self.active = true;
        }
        if !self.active { return; }
        integrate(&mut self.uv, &mut self.velocity);
    }
    pub fn warm_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.gpu.get_or_insert_with(|| Gpu::new(device, format));
    }
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, size: (u32, u32)) {
        if !self.active { return; }
        for (index, vertex) in self.vertices.iter_mut().enumerate() {
            vertex.uv = self.uv[index].to_array();
        }
        let gpu = self.gpu.as_mut().expect("heat haze pipeline warmed at startup");
        if gpu.target.as_ref().is_none_or(|target| target.size != size) { gpu.resize(device, format, size); }
        queue.write_buffer(&gpu.vertices, 0, bytemuck::cast_slice(&self.vertices));
    }
    pub fn source(&self) -> Option<&wgpu::TextureView> {
        if !self.active { return None; }
        self.gpu.as_ref()?.target.as_ref().map(|target| &target.view)
    }
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        if !self.active { return; }
        let Some(gpu) = &self.gpu else { return; };
        let Some(target) = &gpu.target else { return; };
        let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native FX41 resolved-color distortion"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: timestamps, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &target.bind, &[]);
        pass.set_vertex_buffer(0, gpu.vertices.slice(..));
        pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..((WIDTH - 1) * (HEIGHT - 1) * 6) as u32, 0, 0..1);
    }
}
impl Gpu {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("heat haze source"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ] });
        let sampler = device.create_sampler_tracked(&wgpu::SamplerDescriptor { label: Some("native heat haze linear clamp"), address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, min_filter: wgpu::FilterMode::Linear, mag_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native heat haze"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("heat_haze.wgsl", include_str!("heat_haze.wgsl"))) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some("native heat haze"), layout: Some(&pipeline_layout), vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[wgpu::VertexBufferLayout { array_stride: 16, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() }, depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let vertices = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("native heat haze grid"), size: (COUNT * 16) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let indices = device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: Some("native checkerboard grid indices"), contents: bytemuck::cast_slice(&indices()), usage: wgpu::BufferUsages::INDEX });
        Self { layout, sampler, pipeline, vertices, indices, target: None }
    }
    fn resize(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) {
        let texture = device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some("native heat haze resolved world"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] });
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("native heat haze source"), layout: &self.layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ] });
        self.target = Some(Target { size, view, bind });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_activation_consumes_grid_initialization_draws_once() {
        ao_scene::crt_srand(42);
        let mut expected = ao_scene::CrtRand::new(42);
        for _ in 0..5400 { expected.rand(); }
        let mut heat = HeatHaze::default();
        heat.reset();
        assert_eq!(ao_scene::crt_rand(), expected.rand());
        heat.update((64, 64), Mat4::IDENTITY, Vec3::ZERO, Vec3::NEG_Z, std::iter::empty());
        assert_eq!(ao_scene::crt_rand(), expected.rand(), "no visible sphere must not repeat environment initialization");
        heat.reset();
        for _ in 0..5400 { expected.rand(); }
        assert_eq!(ao_scene::crt_rand(), expected.rand(), "a new environment activation repeats native init, regardless of lava");
    }

    #[test]
    fn native_grid_perturbation_spring_and_checkerboard_topology() {
        let mut velocity = vec![Vec2::ZERO; COUNT];
        let mut uv: Vec<_> = (0..COUNT).map(base).collect();
        // First node: distance.25, radius.5 -> strength×(1-.5)^2.
        perturb(&mut velocity, Vec2::new(0.25, 0.0), 0.5, 0.005);
        assert!((velocity[0].x + 0.00125).abs() < 1e-8);
        assert_eq!(velocity[0].y, 0.0);
        integrate(&mut uv, &mut velocity);
        assert!((uv[0].x + 0.000125).abs() < 1e-8);
        assert!((velocity[0].x + 0.0005).abs() < 1e-8);
        let indices = indices();
        assert_eq!(indices.len(), 0x281a);
        assert_eq!(&indices[..6], &[0, 1, 60, 60, 1, 61]);
        assert_eq!(&indices[6..12], &[60, 61, 121, 60, 121, 120]);
        assert!(indices.iter().all(|&index| usize::from(index) < COUNT));
    }

    #[test]
    fn native_sphere_visibility_stops_and_restarts_queue_without_allocating_offscreen() {
        let vp = Mat4::perspective_rh(1.0, 1.0, 0.1, 500.0);
        let mut heat = HeatHaze::default();
        let distant = [[0.0, 0.0, -101.0, 10.0]];
        heat.update((64, 64), vp, Vec3::ZERO, Vec3::NEG_Z, distant.iter());
        assert!(!heat.active && heat.uv.is_empty());
        let near = [[0.0, 0.0, -10.0, 10.0]];
        heat.update((64, 64), vp, Vec3::ZERO, Vec3::NEG_Z, near.iter());
        assert!(heat.active);
        assert_eq!(heat.uv.len(), COUNT);
        assert!(heat.velocity.iter().any(|v| v.length_squared() > 0.0));
        let previous = heat.uv.clone();
        heat.update((64, 64), vp, Vec3::ZERO, Vec3::NEG_Z, std::iter::empty());
        assert!(!heat.active);
        assert_eq!(heat.uv, previous, "native empty queue uses the copy branch without advancing its grid");
        heat.reset();
        assert!(heat.velocity.iter().all(|&velocity| velocity == Vec2::ZERO));
        assert!(heat.uv.iter().enumerate().all(|(index, &uv)| uv == base(index)));
    }
}
