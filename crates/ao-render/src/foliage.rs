//! Native-identified foliage motion. No observed draws or concealed actor history.
use super::{GrassBender, GrassExposure, Mat4, Vec3};

const PUSHERS: usize = 16;
const HISTORY: usize = 64;
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
        self.contacts.retain(|p| p.age < 1.6 && self.selected.iter().any(|b| b.actor == p.source));
        for bender in &self.selected {
            let last = self.contacts.iter_mut().rev().find(|p| p.source == bender.actor);
            if let Some(last) = last.filter(|p| Vec3::from_array(p.position).distance_squared(Vec3::from_array(bender.position)) < 0.35 * 0.35) {
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
            let t = (point.age - 0.08).max(0.0);
            *slot = [point.position[0], point.position[1], point.position[2], (-6.0 * t).exp() * (7.0 * t).cos()];
        }
        out
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
    source.insert_str(end, "\n    foliage_wind: vec4<f32>,\n    foliage_flags: vec4<f32>,\n    foliage_pushers: array<vec4<f32>, 16>,");
    let start = source.find("fn vs(v: VIn)").expect("native static vertex");
    let end = start + source[start..].find("\n}").expect("native static vertex end");
    let body = source[start..end].replace("    var o = vtx(v);", "    var o = vtx(v);\n    if (u32(mat.native.z) & 64u) != 0u { o = foliage_vertex(v, o); }");
    source.replace_range(start..end, &body);
    let grass = crate::shader_source::load("grass.wgsl", include_str!("grass.wgsl"));
    let start = grass.find("fn grass_gust(").expect("stabilized grass gust");
    let end = grass[start..].find("// A sideways bend").expect("stabilized grass wind end") + start;
    source.push_str(&grass[start..end]);
    source.push_str(&crate::shader_source::load("foliage.wgsl", include_str!("foliage.wgsl")));
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
        for _ in 0..8 { state.advance(0.1); }
        state.update_contacts();
        assert!(state.contacts[0].age > 0.7);
        assert!(state.pushers()[1][3].abs() < 0.03);
        state.selected.clear(); state.update_contacts();
        assert!(state.contacts.is_empty());
        assert_eq!(state.pushers(), [[0.0; 4]; PUSHERS]);
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
    fn generated_foliage_shader_contract() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("foliage regression requires GPU");
        for shadows in [false, true] {
            let mut source = crate::enhanced_shader::source(shadows, true, true, true);
            append_shader(&mut source);
            crate::foliage_detail::append_shader(&mut source, true, true);
            assert!(source.contains("max(vertex.fog, vertex_fog("), "motion must not reduce native fog extinction");
            let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("complete foliage shader contract"), source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            assert!(pollster::block_on(errors.pop()).is_none());
        }
    }
}
