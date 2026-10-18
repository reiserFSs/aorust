//! Bounded GPU spell particles and stable-fluid smoke, drawn with the source's native material.
//! Algorithm port: ao-vk rvk/shaders/{particles,smoke_sim}.comp, MIT (c) 2026 wannabuh.
use super::*;

pub(super) const SMOKE_CELLS: u32 = 24;
pub(super) fn enabled(kind: i32, spell: bool, smoke: bool) -> bool {
    match kind { 1018 => spell, 1009 | 3017 => smoke, _ => false }
}
const SPELL_PARTICLES: u32 = 128;
pub(super) const MAX_SOURCES: usize = 8;

/// Supplied only after native emitter identity and visibility admission. Positions are world-space.
#[derive(Clone, Copy)]
pub(super) struct Source {
    pub position: [f32; 3],
    pub radius: f32,
    pub color: [f32; 4],
    pub uv: [f32; 4],
    pub wind: [f32; 3],
    pub lifetime: f32,
    pub rise: f32,
}
/// Only current-frame admitted, walking characters may enter this list.
#[derive(Clone, Copy)]
pub(super) struct Body { pub feet: [f32; 3], pub height: f32, pub velocity: [f32; 3], pub radius: f32 }
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    source: [f32; 4], color: [f32; 4], uv: [f32; 4],
    wind_life: [f32; 4], clock: [f32; 4], right: [f32; 4], up: [f32; 4],
    corner_size: [f32; 4], bodies: [[f32; 4]; 16],
}

pub(super) struct Programs {
    layout: wgpu::BindGroupLayout,
    stages: Vec<pipeline_creation::ComputePipeline>,
    smoke: bool,
}
impl Programs {
    pub fn new(device: &wgpu::Device, smoke: bool) -> Self {
        let entries = std::array::from_fn::<_, 4, _>(|i| wgpu::BindGroupLayoutEntry {
            binding: i as u32, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: if i == 0 { wgpu::BufferBindingType::Uniform }
                else { wgpu::BufferBindingType::Storage { read_only: i == 1 } }, has_dynamic_offset: false, min_binding_size: None }, count: None,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("native effect simulation"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("native effect simulation"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let common = crate::shader_source::load("spell_smoke_common.wgsl", include_str!("spell_smoke_common.wgsl"));
        let simulation = if smoke { crate::shader_source::load("fluid_smoke.wgsl", include_str!("fluid_smoke.wgsl")) }
            else { crate::shader_source::load("spell_particles.wgsl", include_str!("spell_particles.wgsl")) };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("native effect simulation"), source: wgpu::ShaderSource::Wgsl(format!("{common}\n{simulation}").into()) });
        let names: &[&str] = if smoke { &["advect", "curl_field", "confine", "divergence", "jacobi", "project", "vertices"] } else { &["simulate", "vertices"] };
        let stages = names.iter().map(|entry| pipeline_creation::compute_deferred(device, &wgpu::ComputePipelineDescriptor {
            label: Some("native effect simulation"), layout: Some(&pipeline_layout), module: &shader, entry_point: Some(entry), compilation_options: Default::default(), cache: None,
        })).collect();
        Self { layout, stages, smoke }
    }
    pub fn ready(&self) -> bool { self.stages.iter().all(|s| s.is_ready()) }
    pub fn wait(&self) { for stage in &self.stages { stage.get(); } }
}

pub(super) struct Simulation {
    state: [wgpu::Buffer; 2],
    params: wgpu::Buffer,
    pub vertices: wgpu::Buffer,
    groups: [wgpu::BindGroup; 2],
    pub vertex_count: u32,
    smoke: bool,
    corner: Option<Vec3>,
    size: Option<f32>,
    stepping: bool,
}
impl Simulation {
    pub fn new(device: &wgpu::Device, programs: &Programs) -> Self {
        let count = if programs.smoke { SMOKE_CELLS.pow(3) } else { SPELL_PARTICLES };
        let buffer = |label, size, usage| device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        let state = std::array::from_fn(|_| buffer("native effect state", count as u64 * 48, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST));
        let params = buffer("native effect inputs", std::mem::size_of::<Params>() as u64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let vertex_count = count * 6;
        let vertices = buffer("native effect computed vertices", vertex_count as u64 * std::mem::size_of::<ao_scene::Vertex>() as u64, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX);
        let groups = std::array::from_fn(|i| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("native effect simulation"), layout: &programs.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: state[i].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: state[1-i].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: vertices.as_entire_binding() }] }));
        Self { state, params, vertices, groups, vertex_count, smoke: programs.smoke, corner: None, size: None, stepping: false }
    }
    /// Revoke the whole simulation immediately when source admission is lost; never draw orphan history.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, queue: &wgpu::Queue, source: Source, bodies: &[Body], dt: f32, time: f32, right: Vec3, up: Vec3) {
        let radius = source.radius.clamp(0.03, 4.0);
        let size = *self.size.get_or_insert((radius * 12.0).clamp(4.0, 32.0));
        let corner = Vec3::from_array(source.position) - Vec3::new(size * 0.5, radius, size * 0.5);
        let shift = self.corner.map_or(Vec3::ZERO, |previous| corner - previous);
        self.corner = Some(corner);
        let mut params = Params { source: Vec3::from_array(source.position).extend(radius).to_array(), color: source.color, uv: source.uv,
            wind_life: [source.wind[0], source.wind[1], source.wind[2], source.lifetime.clamp(0.2, 12.0)],
            clock: [if dt.is_finite() { dt.clamp(0.0, 1.0 / 30.0) } else { 0.0 }, time, source.rise.clamp(0.0, 8.0), 0.0],
            right: right.extend(shift.x).to_array(), up: up.extend(shift.y).to_array(), corner_size: corner.extend(size).to_array(), bodies: [[0.0; 4]; 16] };
        // Corner z is carried separately from camera vectors; clock.w is not a simulation clock.
        params.clock[3] = shift.z;
        for (i, body) in bodies.iter().filter(|b| Vec3::from_array(b.velocity).is_finite() && Vec3::from_array(b.velocity).length_squared() > 0.0025
            && Vec3::from_array(b.feet).is_finite() && b.radius.is_finite() && b.height.is_finite()).take(8).enumerate() {
            params.bodies[i] = [body.feet[0], body.feet[1], body.feet[2], body.height.clamp(0.1, 6.0)];
            params.bodies[i+8] = [body.velocity[0].clamp(-12.0,12.0), body.velocity[1].clamp(-12.0,12.0), body.velocity[2].clamp(-12.0,12.0), body.radius.clamp(0.1,1.0)];
        }
        self.stepping = params.clock[0] > 0.0;
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
    }
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, programs: &Programs) -> bool {
        if !programs.ready() { return false; }
        let count = self.vertex_count / 6;
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("bounded native effect GPU simulation"), timestamp_writes: None });
            let mut dispatch = |stage: usize, side: usize| { pass.set_pipeline(&programs.stages[stage]); pass.set_bind_group(0, &self.groups[side], &[]); pass.dispatch_workgroups(count.div_ceil(64), 1, 1); };
            if !self.stepping {
                dispatch(if self.smoke { 6 } else { 1 }, 0);
            } else if self.smoke {
                dispatch(0, 0); dispatch(1, 1); dispatch(2, 0); dispatch(3, 1);
                for iteration in 0..24 { dispatch(4, iteration % 2); }
                dispatch(5, 0); dispatch(6, 1);
            } else { dispatch(0, 0); dispatch(1, 1); }
        }
        if self.stepping { encoder.copy_buffer_to_buffer(&self.state[1], 0, &self.state[0], 0, count as u64 * 48); }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_native_vertex_contract() {
        assert_eq!(std::mem::size_of::<ao_scene::Vertex>(), 48);
        assert_eq!(std::mem::size_of::<Params>() % 16, 0);
        assert_eq!(SMOKE_CELLS.pow(3) * 6, 82944);
        assert_eq!(MAX_SOURCES, 8);
    }
    #[test]
    fn only_exact_native_classes_and_independent_switches() {
        for kind in 0..=6000 {
            assert!(!enabled(kind, false, false));
            assert_eq!(enabled(kind, true, false), kind == 1018);
            assert_eq!(enabled(kind, false, true), matches!(kind, 1009 | 3017));
        }
    }
    #[test]
    fn spell_and_fluid_dispatch_real_gpu_stages() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = Renderer::new(&instance, None).expect("GPU effect regression requires adapter");
        for smoke in [false, true] {
            let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let programs = Programs::new(&renderer.device, smoke);
            programs.wait();
            let mut sim = Simulation::new(&renderer.device, &programs);
            sim.update(&renderer.queue, Source { position: [0.0;3], radius: 0.3, color: [1.0;4], uv: [0.0,0.0,1.0,1.0],
                wind: [0.4,0.0,0.0], lifetime: 2.0, rise: 1.0 },
                &[Body { feet: [0.0;3], height: 1.8, velocity: [1.0,0.0,0.0], radius: 0.35 }], 1.0/60.0, 1.0, Vec3::X, Vec3::Y);
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            assert!(sim.encode(&mut encoder, &programs));
            renderer.queue.submit([encoder.finish()]);
            renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            assert!(pollster::block_on(errors.pop()).is_none(), "native GPU effect validation, smoke={smoke}");
        }
    }
}
