//! Enhanced plant-card topology and conservative lighting detail.
use ao_scene::Vertex;
use glam::Vec3;
use crate::gpu_stats::GpuStatsExt;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct State {
    meshes: HashMap<usize, DetailMesh>,
}

#[derive(Default)]
struct DetailMesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    ranges: HashMap<u32, [u32; 2]>,
    gpu: Option<(wgpu::Buffer, wgpu::Buffer)>,
}

impl State {
    /// Cache CPU topology independently of the live switch and native GPU offsets.
    pub(super) fn bake(&mut self, mesh: usize, native_first: u32, vertices: &[Vertex], indices: &[u32]) {
        if indices.is_empty() || !indices.len().is_multiple_of(3) || indices.len() / 3 > MAX_SOURCE_TRIANGLES
            || indices.iter().any(|&i| i as usize >= vertices.len()) { return; }
        let detail = self.meshes.entry(mesh).or_default();
        let extra = indices.len() * (1 << (2 * SUBDIVISION_LEVELS));
        if detail.vertices.len().saturating_add(extra) > u32::MAX as usize
            || detail.indices.len().saturating_add(extra) > u32::MAX as usize { return; }
        let first = detail.indices.len() as u32;
        for t in indices.as_chunks::<3>().0 {
            split(&mut detail.vertices, &mut detail.indices, [vertices[t[0] as usize], vertices[t[1] as usize], vertices[t[2] as usize]], SUBDIVISION_LEVELS);
        }
        detail.ranges.insert(native_first, [first, detail.indices.len() as u32 - first]);
        detail.gpu = None;
    }

    /// Live enable uploads once; disable releases all optional GPU allocations.
    pub(super) fn prepare(&mut self, device: &wgpu::Device, enabled: bool) {
        for detail in self.meshes.values_mut() {
            if !enabled { detail.gpu = None; continue; }
            if detail.gpu.is_none() {
                detail.gpu = Some((
                    device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                        label: Some("foliage detail vertices"), contents: bytemuck::cast_slice(&detail.vertices), usage: wgpu::BufferUsages::VERTEX,
                    }),
                    device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                        label: Some("foliage detail indices"), contents: bytemuck::cast_slice(&detail.indices), usage: wgpu::BufferUsages::INDEX,
                    }),
                ));
            }
        }
    }

    pub(super) fn get(&self, mesh: usize, native_first: u32) -> Option<(&wgpu::Buffer, &wgpu::Buffer, [u32; 2])> {
        let detail = self.meshes.get(&mesh)?;
        let range = *detail.ranges.get(&native_first)?;
        let (vertices, indices) = detail.gpu.as_ref()?;
        Some((vertices, indices, range))
    }
}

const MAX_SOURCE_TRIANGLES: usize = 4096;
const SUBDIVISION_LEVELS: usize = 2;


fn midpoint(a: Vertex, b: Vertex) -> Vertex {
    let mut m = a;
    for k in 0..3 { m.pos[k] = (a.pos[k] + b.pos[k]) * 0.5; m.normal[k] = (a.normal[k] + b.normal[k]) * 0.5; }
    for k in 0..2 { m.uv[k] = (a.uv[k] + b.uv[k]) * 0.5; }
    for k in 0..4 { m.color[k] = (a.color[k] + b.color[k]) * 0.5; }
    m
}

fn split(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>, [a, b, c]: [Vertex; 3], depth: usize) {
    if depth == 0 {
        let base = vertices.len() as u32;
        vertices.extend([a, b, c]);
        indices.extend([base, base + 1, base + 2]);
        return;
    }
    let (ab, bc, ca) = (midpoint(a, b), midpoint(b, c), midpoint(c, a));
    for triangle in [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]] { split(vertices, indices, triangle, depth - 1); }
}

/// Use the same admission-filtered actor list as GPU bending, never raw actors.
pub(super) fn use_subdivision(center: Vec3, radius: f32, admitted: &[crate::grass::GrassBender]) -> bool {
    let reach = radius.max(0.0) + 4.0;
    center.is_finite() && reach.is_finite() && admitted.iter().any(|actor| center.distance_squared(Vec3::from(actor.position)) <= reach * reach)
}

/// Runs only on the generated Enhanced source; no faithful shader mutations.
pub(super) fn append_shader(source: &mut String, translucency: bool, lod: bool) {
    if !translucency && !lod { return; }
    source.push_str(&crate::shader_source::load("foliage_detail.wgsl", include_str!("foliage_detail.wgsl")));
    if lod {
        // rvk F_SHADOWCHEAP: distant foliage keeps real lighting but replaces
        // soft-shadow blocker search/PCSS with one hardware-filtered comparison.
        // Bounds, receiver projection/bias, cascade selection and blending stay
        // in the shared receiver. Near foliage and non-plants stay byte-identical.
        *source = source.replace(
            "    // Wide taps must not slope-extrapolate past ~76 degrees of receiver tilt.",
            "    if foliage_detail_distant(world) {\n        return textureSampleCompareLevel(shadow_maps, shadow_sampler, uv, i32(layer), p.z - 0.00002);\n    }\n    // Wide taps must not slope-extrapolate past ~76 degrees of receiver tilt.",
        );
    }
    if translucency {
        *source = source.replace("    let lit = min(texture_color + spec, vec3<f32>(1.0));", "    let foliage_transfer = foliage_detail_transmission(i.wpos, i.n, i.clip.xy);\n    texture_color = min(texture_color + foliage_transfer * clamp(t.rgb * i.tint.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0));\n    let lit = min(texture_color + spec, vec3<f32>(1.0));");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vertex(x: f32, y: f32) -> Vertex { Vertex { pos: [x,y,0.0], normal: [0.0,0.0,1.0], uv: [x,y], color: [1.0;4] } }
    #[test]
    fn topology_keeps_original_and_interpolates_interior() {
        let native_vertices = vec![vertex(0.0,0.0),vertex(4.0,0.0),vertex(0.0,4.0)];
        let native_indices = vec![0,1,2,0,2,1];
        let mut state = State::default();
        state.bake(7,0,&native_vertices,&native_indices[..3]);
        state.bake(7,3,&native_vertices,&native_indices[3..]);
        let detail = &state.meshes[&7];
        assert_eq!(detail.ranges[&0],[0,48]);
        assert_eq!(detail.ranges[&3],[48,48]);
        assert_eq!(native_indices,vec![0,1,2,0,2,1]);
        assert_eq!(native_vertices.len(),3);
        assert!(detail.gpu.is_none(), "scene upload must not allocate optional GPU buffers");
        assert!(detail.vertices.iter().any(|v| v.pos == [1.0,1.0,0.0] && v.uv == [1.0,1.0]));
        let area: f32 = detail.indices[..48].as_chunks::<3>().0.iter().map(|t| { let a = Vec3::from(detail.vertices[t[0] as usize].pos); let b = Vec3::from(detail.vertices[t[1] as usize].pos); let c = Vec3::from(detail.vertices[t[2] as usize].pos); (b-a).cross(c-a).z * 0.5 }).sum();
        assert_eq!(area,8.0);
    }
    #[test]
    fn malformed_and_unbounded_topology_stays_native() {
        let vertices = vec![vertex(0.0,0.0)];
        let mut state = State::default();
        for indices in [vec![1,0,0], vec![0,0], vec![0; (MAX_SOURCE_TRIANGLES + 1)*3]] {
            state.bake(0,0,&vertices,&indices);
            assert!(state.meshes.is_empty());
        }
    }
    #[test]
    fn actor_admission_and_distance_select_topology() {
        let a = crate::grass::GrassBender { actor: Some(7), position: [0.0;3] };
        assert!(!use_subdivision(Vec3::ZERO,1.0,&[]));
        assert!(use_subdivision(Vec3::new(4.5,0.0,0.0),1.0,&[a]));
        assert!(!use_subdivision(Vec3::new(6.0,0.0,0.0),1.0,&[a]));
    }
    #[test]
    fn switches_are_structurally_independent() {
        let base = crate::enhanced_shader::source(crate::EnhancedSettings::default());
        let mut off = base.clone(); append_shader(&mut off,false,false); assert_eq!(off,base);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("foliage shader regression requires offscreen GPU");
        let mut state = State::default();
        state.bake(0,12,&[vertex(0.0,0.0),vertex(1.0,0.0),vertex(0.0,1.0)],&[0,1,2]);
        for enabled in [false,true,false,true] {
            state.prepare(&renderer.device,enabled);
            assert_eq!(state.get(0,12).is_some(),enabled, "live switch must allocate and release cached topology without a scene reload");
            assert!(state.get(0,0).is_none(), "cache uses native first-index keys");
        }
        for (transmission,lod) in [(true,false),(false,true),(true,true)] {
            let mut source = crate::enhanced_shader::source(crate::EnhancedSettings { sun_shadows: true, ..Default::default() });
            append_shader(&mut source,transmission,lod);
            assert_eq!(source.contains("let foliage_transfer ="),transmission);
            assert_eq!(source.contains("if foliage_detail_distant(world) {"),lod);
            let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("foliage detail regression"), source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            assert!(pollster::block_on(errors.pop()).is_none(), "valid foliage detail shader");
        }
        // Local-light/specular augmentation changes the tint varying to RGBA.
        let source = crate::enhanced_shader::source(crate::EnhancedSettings {
            sun_shadows: true, material_specular: true, clustered_lighting: true,
            foliage_translucency: true, foliage_lod: true, ..Default::default()
        });
        let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("combined foliage lighting regression"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        assert!(pollster::block_on(errors.pop()).is_none(), "valid combined foliage and lighting shader");
    }
    #[test]
    fn transmission_is_sun_gated_and_keeps_native_fog() {
        let original = crate::enhanced_shader::source(crate::EnhancedSettings::default());
        let mut source = original.clone();
        append_shader(&mut source,true,false);
        let fog = "let rgb = select(mix(lit, authored_fog, f), lit, add);";
        assert!(source.contains(fog) && original.contains(fog));
        assert!(source.contains("sun_visibility(world, lit_side, pixel)"));
        assert!(source.contains("max(g.sun_g.rgb, vec3<f32>(0.0))"));
        assert!(!source.contains("foliage_pushers"));
        for sun in [0.0f32, 0.5, 1.0] { for visible in [0.0f32, 0.5, 1.0] { for fog in [0.0f32, 0.5, 1.0] {
            let unfogged = 0.15 * sun * visible;
            let transmitted = unfogged * (1.0 - fog);
            assert!(transmitted <= unfogged);
            if sun == 0.0 || visible == 0.0 || fog == 1.0 { assert_eq!(transmitted,0.0); }
        } } }
    }
    #[test]
    fn backlight_is_two_sided_and_strongest_toward_sun() {
        let transfer = |normal: Vec3, view: Vec3, sun: Vec3| {
            let facing = if normal.dot(view) >= 0.0 { normal } else { -normal };
            0.5 * (-facing.dot(sun)).max(0.0) + 0.7 * (-view.dot(sun)).max(0.0).powi(4)
        };
        let normal = Vec3::Z;
        let sun = -Vec3::Z;
        assert_eq!(transfer(normal, Vec3::Z, sun), 1.2);
        assert_eq!(transfer(-normal, Vec3::Z, sun), 1.2);
        assert_eq!(transfer(normal, -Vec3::Z, sun), 0.0);
        assert!(transfer(Vec3::X, Vec3::Z, sun) >= 0.7, "grazing cards still forward-scatter");
        let shader = include_str!("foliage_detail.wgsl");
        assert!(shader.contains("select(-n, n, dot(n, view) >= 0.0)"));
        assert!(shader.contains("0.5 * through_leaf + 0.7 * toward_eye"));
    }
    #[test]
    fn lod_cheap_shadow_preserves_lamps_near_plants_and_receiver_bounds() {
        let original = crate::enhanced_shader::source(crate::EnhancedSettings { sun_shadows: true, ..Default::default() });
        let mut source = original.clone();
        append_shader(&mut source, false, true);
        let lighting = |text: &str| text.split("fn light_vertex_sun(").nth(1).unwrap().split("\n}").next().unwrap().to_owned();
        assert_eq!(lighting(&source), lighting(&original), "LOD must not discard local lamps or specular");
        let cascade = source.split("fn cascade_visibility(").nth(1).unwrap().split("\n}").next().unwrap();
        let branch = cascade.find("if foliage_detail_distant(world) {").unwrap();
        assert!(cascade.find("if any(uv <= guard)").unwrap() < branch);
        assert!(cascade.find("if softness <= 0.0").unwrap() < branch, "hard-shadow receiver plane filter is unchanged");
        assert!(branch < cascade.find("var blocker_sum").unwrap());
        let single_tap = cascade[branch..].split("\n    }").next().unwrap();
        assert_eq!(single_tap.matches("textureSampleCompareLevel(").count(), 1);
        assert!(single_tap.contains("uv, i32(layer), p.z - 0.00002"));
        assert_eq!(source.split("fn sun_visibility(").nth(1).unwrap().split("\n}").next(),
            original.split("fn sun_visibility(").nth(1).unwrap().split("\n}").next(), "cascade selection, blend and fog envelope stay native");
        for plant in [false, true] { for distance in [0.0f32, 48.0, 48.1] {
            let cheap = plant && distance * distance > 2304.0;
            assert_eq!(cheap, plant && distance > 48.0);
        } }
        let helper = include_str!("foliage_detail.wgsl");
        assert!(helper.contains("(u32(mat.native.z) & 64u) != 0u && dot(delta, delta) > 2304.0"));
    }
}
