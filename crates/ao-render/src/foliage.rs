//! Native-identified foliage motion. No observed draws or concealed actor history.
use super::{GrassBender, GrassExposure, Mat4, Vec3};

const PUSHERS: usize = 16;
const HISTORY: usize = 64;

fn push_spring(age: f32) -> f32 {
    // Deliberately less damped than ao-vk's exp(-6t) cos(7t): its ~7% rebound
    // disappears at ordinary third-person distance. This returns ~25% once.
    let t = (age - 0.08).max(0.0);
    (-4.0 * t).exp() * (9.0 * t).cos()
}
#[derive(Clone, Copy)]
struct Contact { source: Option<u32>, position: [f32; 3], age: f32 }
#[derive(Default)]
pub(super) struct Foliage {
    phase: f64,
    wind: [f32; 3],
    filtered: [f32; 3],
    seeded: bool,
    inputs: Vec<GrassBender>,
    exposure: Option<GrassExposure>,
    selected: Vec<GrassBender>,
    contacts: Vec<Contact>,
}
impl Foliage {
    pub fn advance(&mut self, dt: f32) {
        self.phase = super::grass::advance_wind_phase(self.phase, dt);
        self.filtered = super::grass::smooth_wind(self.filtered, self.wind, dt);
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        for contact in &mut self.contacts { contact.age += dt; }
    }
    pub fn clear_inputs(&mut self) { self.inputs.clear(); self.selected.clear(); }
    pub fn set_inputs(&mut self, inputs: &[GrassBender], wind: [f32; 3]) {
        self.inputs.clear();
        self.inputs.extend(inputs.iter().copied().filter(|b| b.position.iter().all(|v| v.is_finite())));
        self.wind = wind.map(|v| if v.is_finite() { v } else { 0.0 });
        if !self.seeded { self.filtered = super::grass::wind_target(self.wind); self.seeded = true; }
    }
    pub fn set_exposure(&mut self, exposure: Option<GrassExposure>) {
        if self.exposure.as_ref().map(|e| e.generation) != exposure.as_ref().map(|e| e.generation) { self.contacts.clear(); }
        self.exposure = exposure;
    }
    pub fn wind(&self) -> (f32, [f32; 3]) { (self.phase as f32, self.filtered) }
    pub fn wind_state(&self) -> (f64, [f32; 3]) { (self.phase, self.filtered) }
    pub fn selected(&self) -> &[GrassBender] { &self.selected }
    pub fn prepare(&mut self, eye: Vec3, vp: Mat4, admitted: impl Iterator<Item = u32>, enabled: bool) {
        if !enabled { self.selected.clear(); self.contacts.clear(); return; }
        let (selected, count) = super::grass::select_benders(&mut self.inputs, eye, vp, admitted,
            self.exposure.as_ref().map(|e| e.line_of_sight.as_ref()));
        self.selected.clear(); self.selected.extend_from_slice(&selected[..count]);
        self.update_contacts();
    }
    fn update_contacts(&mut self) {
        // Source revocation precedes recovery: an unseen enemy cannot leave a spring signal.
        self.contacts.retain(|p| p.age < 2.2 && self.selected.iter().any(|b| b.actor == p.source));
        for bender in &self.selected {
            let last = self.contacts.iter_mut().rev().find(|p| p.source == bender.actor);
            if let Some(last) = last.filter(|p| Vec3::from_array(p.position).distance_squared(Vec3::from_array(bender.position)) < 0.2 * 0.2) {
                last.age = 0.0;
            } else {
                if self.contacts.len() == HISTORY { self.contacts.remove(0); }
                self.contacts.push(Contact { source: bender.actor, position: bender.position, age: 0.0 });
            }
        }
    }
    pub fn pushers(&self) -> [[f32; 4]; PUSHERS] {
        let mut out = [[0.0; 4]; PUSHERS];
        for (slot, point) in out.iter_mut().zip(self.contacts.iter().rev()) {
            *slot = [point.position[0], point.position[1], point.position[2], push_spring(point.age)];
        }
        out
    }
    pub fn owners(&self, slot: impl Fn(Option<u32>) -> u32) -> [[u32; 4]; 4] {
        let mut owners = [[0; 4]; 4];
        for (index, contact) in self.contacts.iter().rev().take(PUSHERS).enumerate() {
            owners[index / 4][index % 4] = slot(contact.source);
        }
        owners
    }
}

/// Bounds occupy otherwise sky-only material lanes, and are read only by the
/// Enhanced static vegetation vertex path. Native shader evaluation is unchanged.
pub(super) fn material_bounds(mut uniform: [f32; 32], material: &ao_scene::Submesh, mesh: &ao_scene::Mesh) -> [f32; 32] {
    if !material.vegetation { return uniform; }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for &index in &material.indices {
        if let Some(vertex) = mesh.vertices.get(index as usize) {
            lo = lo.min(Vec3::from_array(vertex.pos)); hi = hi.max(Vec3::from_array(vertex.pos));
        }
    }
    if lo.is_finite() && hi.is_finite() {
        uniform[12..16].copy_from_slice(&lo.extend(0.0).to_array());
        uniform[24..28].copy_from_slice(&hi.extend(0.0).to_array());
    }
    uniform
}

pub(super) fn append_shader(source: &mut String) {
    let globals = source.find("struct G {").expect("native globals");
    let end = globals + source[globals..].find("\n}").expect("native globals end");
    source.insert_str(end, "\n    foliage_wind: vec4<f32>,\n    foliage_flags: vec4<f32>,\n    foliage_pushers: array<vec4<f32>, 16>,\n    foliage_owners: array<vec4<u32>, 4>,");
    let start = source.find("fn vs(v: VIn)").expect("native static vertex");
    let end = start + source[start..].find("\n}").expect("native static vertex end");
    let body = source[start..end].replace("    var o = vtx(v);", "    var o = vtx(v);\n    if (u32(mat.native.z) & 64u) != 0u { o = foliage_vertex(v, o); }");
    source.replace_range(start..end, &body);
    let grass = crate::shader_source::load("grass.wgsl", include_str!("grass.wgsl"));
    let start = grass.find("fn grass_gust(").expect("stabilized grass gust");
    let end = grass[start..].find("// A sideways bend").expect("stabilized grass wind end") + start;
    source.push_str(&grass[start..end]);
    source.push_str(&crate::shader_source::load("foliage.wgsl", include_str!("foliage.wgsl")));
    source.push_str("\n@group(0) @binding(8) var<storage, read> interaction_visibility: array<u32>;\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revoked_sources_erase_contacts_and_springs_recover() {
        let mut state = Foliage::default();
        state.selected.push(GrassBender { actor: Some(7), position: [1.0, 0.0, 1.0] });
        state.update_contacts();
        assert_eq!(state.pushers()[0][3], 1.0);
        state.selected[0].position[0] += 1.0;
        for _ in 0..4 { state.advance(0.1); }
        state.update_contacts();
        assert!(state.contacts[0].age > 0.39);
        assert!(state.pushers()[1][3] < -0.2, "departed contact must visibly overshoot upright");
        for _ in 0..19 { state.advance(0.1); }
        state.update_contacts();
        assert_eq!(state.contacts.len(), 1, "old spring must expire, not accumulate");
        state.set_inputs(&[GrassBender { actor: Some(7), position: [2.0, 0.0, 1.0] }], [0.0; 3]);
        state.prepare(Vec3::ZERO, Mat4::IDENTITY, std::iter::empty(), true);
        assert!(state.contacts.is_empty());
        assert_eq!(state.pushers(), [[0.0; 4]; PUSHERS]);
    }
    #[test]
    fn hidden_crowds_cannot_evict_visible_benders_or_contact_history() {
        let eye = Vec3::new(0.0, 1.0, 0.0);
        let vp = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.2, 100.0)
            * Mat4::look_at_rh(eye, -Vec3::Z, Vec3::Y);
        let mut baseline = Foliage::default();
        let mut crowded = Foliage::default();
        for state in [&mut baseline, &mut crowded] {
            state.set_exposure(Some(GrassExposure {
                generation: 1, query: std::rc::Rc::new(|_| true),
                line_of_sight: std::rc::Rc::new(|_, target| target[0] <= 0.0),
            }));
        }
        for step in 0..6 {
            let visible: Vec<_> = (0..PUSHERS as u32).map(|id| GrassBender {
                actor: Some(id), position: [-1.0, 0.0, -5.0 - step as f32 * 0.3],
            }).collect();
            // More occluded and non-admitted candidates than either capacity,
            // all closer than the visible actors and ordered before them.
            let mut inputs: Vec<_> = (100..260).map(|id| GrassBender {
                actor: Some(id), position: [if id < 180 { 0.1 } else { -0.1 }, 0.0, -1.0],
            }).collect();
            inputs.extend_from_slice(&visible);
            baseline.set_inputs(&visible, [0.0; 3]);
            crowded.set_inputs(&inputs, [0.0; 3]);
            baseline.prepare(eye, vp, 0..PUSHERS as u32, true);
            crowded.prepare(eye, vp, (100..180).chain(0..PUSHERS as u32), true);
            assert_eq!(baseline.selected(), crowded.selected());
            assert_eq!(baseline.pushers(), crowded.pushers());
            let contacts = |state: &Foliage| state.contacts.iter()
                .map(|p| (p.source, p.position, p.age)).collect::<Vec<_>>();
            assert_eq!(contacts(&baseline), contacts(&crowded));
            baseline.advance(0.1);
            crowded.advance(0.1);
        }
        assert_eq!(baseline.selected.len(), PUSHERS);
        assert_eq!(baseline.contacts.len(), HISTORY);
        crowded.prepare(eye, vp, std::iter::empty(), true);
        assert!(crowded.contacts.is_empty(), "admission revocation erases the full history");
    }
    #[test]
    fn gpu_occluded_crowd_cannot_consume_bender_or_history_capacity() {
        use ao_scene::{ActorFrame, Instance, Mesh, Scene, Submesh, Vertex, IDENTITY};
        let quad = |lo: [f32; 3], hi: [f32; 3]| Mesh {
            vertices: [[lo[0], lo[1], lo[2]], [hi[0], lo[1], lo[2]],
                [hi[0], hi[1], hi[2]], [lo[0], hi[1], hi[2]]].map(|pos|
                Vertex { pos, normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() }).to_vec(),
            submeshes: vec![Submesh { two_sided: true, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = crate::Renderer::new(&instance, None).expect("headless foliage admission GPU");
        renderer.upload(&Scene {
            meshes: vec![quad([-1.0, 0.0, -2.0], [1.0, 4.0, -2.0])],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }], ..Default::default()
        });
        renderer.add_actor_model(1, &Scene { meshes: vec![quad([-0.15, 0.0, 0.0], [0.15, 1.8, 0.0])], ..Default::default() });
        renderer.env.fog_start = 100.0;
        renderer.env.fog_end = 120.0;
        renderer.set_enhanced_settings(crate::EnhancedSettings { foliage_push: true, ..Default::default() });
        // Deliberately admit every CPU LOS: only actual primary GPU occlusion can
        // distinguish the hidden crowd, including its nearer feet, from the control.
        renderer.set_enhanced_grass_exposure(Some(GrassExposure {
            generation: 1, query: std::rc::Rc::new(|_| true), line_of_sight: std::rc::Rc::new(|_, _| true),
        }));
        let targets = crate::Targets::new(&renderer, 128, 96);
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("foliage admission regression"), size: wgpu::Extent3d { width: 128, height: 96, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let camera = crate::Camera::look_at(Vec3::new(0.0, 1.0, 4.0), Vec3::new(0.0, 1.0, -2.0));
        let mut run = |crowd: bool| {
            renderer.foliage.contacts.clear();
            // Both scenarios must start without a previous-frame visibility result.
            // Otherwise the second run inherits the first run's visible control
            // and creates one extra contact before its own first GPU admission.
            renderer.interaction_admission.invalidate();
            let mut result = Vec::new();
            for step in 0..=HISTORY + 2 {
                let position = [-2.0 - (step % 2) as f32 * 0.3, 0.0, -4.0];
                let mut actors = Vec::new();
                let mut inputs = Vec::new();
                if crowd {
                    for id in 100..180 {
                        let feet = [0.0, 0.0, -2.5];
                        actors.push(ActorFrame { id, model: 1, transform: Mat4::from_translation(feet.into()).to_cols_array_2d(), ..Default::default() });
                        inputs.push(GrassBender { actor: Some(id), position: feet });
                    }
                }
                actors.push(ActorFrame { id: 1, model: 1, transform: Mat4::from_translation(position.into()).to_cols_array_2d(), ..Default::default() });
                inputs.push(GrassBender { actor: Some(1), position });
                renderer.set_actors(actors);
                renderer.set_enhanced_grass_inputs(&mut inputs, [0.0; 3]);
                renderer.render(&view, &targets, &camera);
                // Test-only synchronization: production Admission uses Poll, never Wait.
                renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                if step > 1 {
                    assert_eq!(renderer.foliage.selected(), &[GrassBender { actor: Some(1), position }]);
                    assert!(renderer.foliage.contacts.iter().all(|p| p.source == Some(1)));
                    result.push(renderer.foliage.pushers());
                }
            }
            assert_eq!(renderer.foliage.contacts.len(), HISTORY);
            result
        };
        let baseline = run(false);
        assert_eq!(baseline, run(true), "more than both caps of GPU-hidden actors must match their absence");
        let shader = include_str!("foliage.wgsl");
        assert!(shader.find("if !foliage_owner_admitted(owner)").unwrap() < shader.find("let source = g.foliage_pushers[k]").unwrap(),
            "current admission must revoke previous-frame contacts before their displacement");
        let helper = shader.split("fn foliage_vertex(").next().unwrap();
        let source = format!("@group(0) @binding(0) var<storage,read> interaction_visibility:array<u32>;
@group(0) @binding(1) var<storage,read_write> output:array<u32>;
{helper}
@compute @workgroup_size(1) fn main() {{
 output[0]=select(0u,1u,foliage_owner_admitted(0u));
 output[1]=select(0u,1u,foliage_owner_admitted(1u));
 output[2]=select(0u,1u,foliage_owner_admitted(2u));
 output[3]=select(0u,1u,foliage_owner_admitted(0xffffffffu));
}}");
        let device = &renderer.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("actual foliage current owner gate"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None, layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None,
        });
        use wgpu::util::DeviceExt;
        let visible = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: bytemuck::cast_slice(&[0u32, 1, 0]), usage: wgpu::BufferUsages::STORAGE,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let staging = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: visible.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
        ] });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline); pass.set_bind_group(0, &group, &[]); pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, 16);
        renderer.queue.submit([encoder.finish()]);
        let (send, receive) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |value| send.send(value).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receive.recv().unwrap().unwrap();
        assert_eq!(bytemuck::cast_slice::<u8, u32>(&staging.slice(..).get_mapped_range()), [1, 1, 0, 0],
            "current GPU rejection must override previous admission, while own control remains admitted");
        staging.unmap();
        println!("foliage_admission hidden_actors=80 bender_cap=16 contact_cap=64 hidden_vs_absent=equal current_hidden_rejected=true");
    }
    #[test]
    fn motion_reuses_stabilized_grass_wind() {
        let mut state = Foliage::default();
        state.set_inputs(&[], [2.0, 0.0, 1.0]);
        state.advance(0.1);
        assert_eq!(state.wind().0, super::super::grass::advance_wind_phase(0.0, 0.1) as f32);
        assert_eq!(state.wind().1, super::super::grass::wind_target([2.0, 0.0, 1.0]));
    }
    #[test]
    fn spring_has_readable_rebound_and_no_stationary_pileup() {
        assert_eq!(push_spring(0.0), 1.0);
        assert_eq!(push_spring(0.08), 1.0);
        assert!(push_spring(0.4) < -0.2);
        assert!(push_spring(1.6).abs() < 0.003);
        let mut state = Foliage::default();
        state.selected.push(GrassBender { actor: None, position: [0.0; 3] });
        for _ in 0..30 { state.advance(0.1); state.update_contacts(); }
        assert_eq!(state.contacts.len(), 1);
        assert_eq!(state.pushers()[0][3], 1.0);
        state.prepare(Vec3::ZERO, Mat4::IDENTITY, std::iter::empty(), false);
        assert_eq!(state.pushers(), [[0.0; 4]; PUSHERS]);
    }
    #[test]
    fn generated_foliage_shader_contract() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("foliage regression requires GPU");
        for shadows in [false, true] {
            let mut source = crate::enhanced_shader::source(crate::EnhancedSettings {
                sun_shadows: shadows, ambient_occlusion: true, emissive_bloom: true, hdr: true,
                ..Default::default()
            });
            append_shader(&mut source);
            crate::foliage_detail::append_shader(&mut source, true, true);
            crate::foliage_edges::append(&mut source);
            assert!(source.contains("max(vertex.fog, vertex_fog("), "motion must not reduce native fog extinction");
            let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("complete foliage shader contract"), source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            assert!(pollster::block_on(errors.pop()).is_none());
        }
    }
}
