//! Additive native-texture canopy sprigs. Original meshes are never rewritten.
use super::*;
use std::ops::Range;

const MAX_SPRIGS: usize = 32768;
const MAX_PER_SUBMESH: usize = 128;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LeafVertex {
    native: ao_scene::Vertex,
    center_phase: [f32; 4],
    falling: f32,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params { control: [f32; 4], wind: [f32; 4] }
#[derive(Default)]
pub(super) struct Leaves {
    vertices: Option<wgpu::Buffer>,
    baked: Vec<LeafVertex>,
    ranges: HashMap<(usize, u32), Range<u32>>,
    compiled: Option<Compiled>,
    enabled: bool,
}
struct Compiled {
    key: (u32, bool, bool, bool, NightMode, bool),
    beauty: pipeline_creation::RenderPipeline,
    shadow: pipeline_creation::RenderPipeline,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}
fn bake(mesh: &ao_scene::Mesh, submesh: &ao_scene::Submesh, limit: usize, output: &mut Vec<LeafVertex>) {
    if !submesh.vegetation || submesh.texture.is_none() || !submesh.lighting_enabled
        || !submesh.fog_enabled || !material_depth_write(submesh)
        || !matches!(submesh.blend, Blend::AlphaTest | Blend::AlphaBlend) { return; }
    let triangles = submesh.indices.as_chunks::<3>().0;
    let count = triangles.len().min(MAX_PER_SUBMESH).min(limit);
    for sample in 0..count {
        let triangle = triangles[sample * triangles.len() / count];
        let [Some(a), Some(b), Some(c)] = triangle.map(|index| mesh.vertices.get(index as usize).copied()) else { continue; };
        let vertices = [a, b, c];
        let p = vertices.map(|v| Vec3::from(v.pos));
        if p.iter().any(|p| !p.is_finite()) { continue; }
        let cross = (p[1] - p[0]).cross(p[2] - p[0]);
        let Some(normal) = cross.try_normalize() else { continue; };
        let seed = p.iter().flat_map(|p| p.to_array()).fold(0x632be59bu32, |seed, v| hash(seed ^ v.to_bits()));
        let a = 0.2 + (seed & 255) as f32 / 255.0 * 0.2;
        let b = 0.2 + ((seed >> 8) & 255) as f32 / 255.0 * 0.2;
        let weights = [a, b, 1.0 - a - b];
        let center = p[0] * weights[0] + p[1] * weights[1] + p[2] * weights[2];
        let extent = p.iter().map(|p| p.distance(center)).fold(0.0f32, f32::max);
        let scale = (0.24 / extent.max(0.001)).min(0.32);
        let lift = normal * (extent * 0.025).clamp(0.008, 0.08);
        let uv_center = std::array::from_fn::<_, 2, _>(|i| (0..3).map(|j| vertices[j].uv[i] * weights[j]).sum::<f32>());
        let phase = seed as f32 / u32::MAX as f32 * std::f32::consts::TAU;
        let tilt = glam::Quat::from_axis_angle((p[1] - p[0]).normalize(), 0.25 + 0.3 * phase.sin());
        for falling in [0.0, 1.0] {
            for (i, mut vertex) in vertices.into_iter().enumerate() {
                vertex.pos = (center + tilt * ((p[i] - center) * scale) + lift).to_array();
                vertex.normal = (tilt * Vec3::from_array(vertex.normal)).to_array();
                vertex.uv = std::array::from_fn(|axis| uv_center[axis] + (vertex.uv[axis] - uv_center[axis]) * scale);
                output.push(LeafVertex { native: vertex, center_phase: (center + lift).extend(phase).to_array(), falling });
            }
        }
    }
}
impl Leaves {
    /// Explicit loading/capture boundary only; ordinary frames remain nonblocking.
    pub(super) fn wait(&self) {
        if let Some(compiled) = &self.compiled {
            compiled.beauty.get();
            compiled.shadow.get();
        }
    }
    pub(super) fn upload(&mut self, _device: &wgpu::Device, scene: &Scene, _gpu: &Gpu) {
        self.ranges.clear();
        let mut vertices = Vec::new();
        for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
            let mut first_index = 0;
            for submesh in &mesh.submeshes {
                let start = vertices.len() as u32;
                bake(mesh, submesh, MAX_SPRIGS.saturating_sub(vertices.len() / 6), &mut vertices);
                if vertices.len() as u32 > start {
                    self.ranges.insert((mesh_index, first_index), start..vertices.len() as u32);
                }
                // Match native invalid-primitive filtering, not raw archive index count.
                first_index += submesh.indices.as_chunks::<3>().0.iter()
                    .filter(|triangle| triangle.iter().all(|&i| (i as usize) < mesh.vertices.len())).count() as u32 * 3;
            }
        }
        self.baked = vertices;
        self.vertices = None;
    }
}
impl Compiled {
    fn new(renderer: &Renderer, key: (u32, bool, bool, bool, NightMode, bool)) -> Self {
        let device = &renderer.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canopy wind"), entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64) }, count: None,
            }],
        });
        let source = shader_source((key.1, key.2, key.3), key.4, key.5);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native canopy sprigs"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let receiver_layout = (key.4 == NightMode::Moonlit)
            .then(|| sun_shadows::receiver_layout(device));
        let mut beauty_groups = vec![Some(&renderer.g_layout), Some(&renderer.tex_layout), Some(&layout)];
        if let Some(receiver) = &receiver_layout { beauty_groups.push(Some(receiver)); }
        let beauty_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native canopy beauty"), bind_group_layouts: &beauty_groups, immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native canopy caster"), bind_group_layouts: &[Some(&renderer.g_layout), Some(&renderer.tex_layout), Some(&layout), Some(&renderer.shadow_pipelines.caster_layout)], immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4, 8 => Float32x4, 9 => Float32];
        let instances = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let buffers = [
            wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<LeafVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes },
            wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance, attributes: &instances },
        ];
        let auxiliary = || Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL });
        let targets = [Some(wgpu::ColorTargetState { format: renderer.format, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            key.1.then(auxiliary).flatten(), key.2.then(auxiliary).flatten(), key.3.then(auxiliary).flatten()];
        let target_count = if key.3 { 4 } else if key.1 || key.2 { 3 } else { 1 };
        let make = |shadow: bool| pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some(if shadow { "canopy sun caster" } else { "canopy native lighting" }), layout: Some(if shadow { &shadow_layout } else { &beauty_layout }),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some(if shadow { "vs_canopy_shadow" } else { "vs_canopy" }), compilation_options: Default::default(), buffers: &buffers },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(if shadow { "fs_canopy_shadow" } else { "fs_canopy" }), compilation_options: Default::default(), targets: if shadow { &[] } else { &targets[..target_count] } }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: if shadow { 1 } else { key.0 }, alpha_to_coverage_enabled: !shadow && key.0 > 1, ..Default::default() }, multiview_mask: None, cache: None,
        });
        let uniform = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("canopy wind switches"), size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("canopy wind"), layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }] });
        Self { key, beauty: make(false), shadow: make(true), uniform, bind }
    }
}
fn shader_source(channels: (bool, bool, bool), night: NightMode, moon_shadows: bool) -> String {
    let leaf = crate::shader_source::load("foliage_leaves.wgsl", include_str!("foliage_leaves.wgsl"))
        .replace("//__AMBIENT_FIELD__", if channels.0 { "@location(1) ambient: vec4<f32>," } else { "" })
        .replace("//__EMISSIVE_FIELD__", if channels.1 { "@location(2) emissive: vec4<f32>," } else { "" })
        .replace("//__SUN_FIELD__", if channels.2 { "@location(3) sun: vec4<f32>," } else { "" })
        .replace("//__AMBIENT_WRITE__", if channels.0 { "output.ambient = vec4<f32>(0.0);" } else { "" })
        .replace("//__EMISSIVE_WRITE__", if channels.1 { "output.emissive = vec4<f32>(0.0);" } else { "" })
        .replace("//__SUN_WRITE__", if channels.2 { "output.sun = vec4<f32>(0.0);" } else { "" })
        .replace("//__MOON_RECEIVER__", if night != NightMode::AsGame {
            "let receiver = authored_moon_receiver(i, i.n, false);"
        } else { "let receiver = i;" });
    let mut native = crate::shader_source::load("shader.wgsl", include_str!("shader.wgsl")).into_owned();
    crate::foliage::append_shader(&mut native);
    let native = native.replace("@vertex\nfn vs(", "fn canopy_native_vs(");
    let source = format!("{native}\n{}\n{leaf}",
        crate::shader_source::load("sun_shadows.wgsl", include_str!("sun_shadows.wgsl")));
    if night != NightMode::AsGame {
        crate::moon_shadows::shader(source, moon_shadows, night == NightMode::Moonlit)
    } else { source }
}
impl Renderer {
    pub(super) fn prepare_canopy_leaves(&mut self) {
        let s = self.enhanced_settings;
        self.canopy_leaves.enabled = s.foliage_leaves || s.foliage_flutter || s.foliage_falling;
        if !self.canopy_leaves.enabled {
            self.canopy_leaves.vertices = None;
            self.canopy_leaves.compiled = None;
            return;
        }
        if self.canopy_leaves.baked.is_empty() { return; }
        if self.canopy_leaves.vertices.is_none() {
            self.canopy_leaves.vertices = Some(self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
                label: Some("native canopy sprigs"), contents: bytemuck::cast_slice(&self.canopy_leaves.baked), usage: wgpu::BufferUsages::VERTEX,
            }));
        }
        let key = (self.world.msaa, s.ambient_occlusion, s.emissive_bloom, s.needs_sun_mask(), s.night_mode, s.moon_shadows);
        if self.canopy_leaves.compiled.as_ref().is_none_or(|compiled| compiled.key != key) {
            self.canopy_leaves.compiled = Some(Compiled::new(self, key));
        }
        let (phase, wind) = self.foliage.wind();
        let params = Params { control: [phase, u8::from(s.foliage_leaves || s.foliage_flutter) as f32, u8::from(s.foliage_flutter) as f32, u8::from(s.foliage_falling) as f32],
            wind: [wind[0], wind[1], wind[2], 0.0] };
        self.queue.write_buffer(&self.canopy_leaves.compiled.as_ref().unwrap().uniform, 0, bytemuck::bytes_of(&params));
    }
    pub(super) fn draw_canopy_leaves(&self, pass: &mut wgpu::RenderPass) -> u32 {
        self.draw_canopy(pass, None)
    }
    pub(super) fn draw_canopy_leaf_shadows(&self, pass: &mut wgpu::RenderPass, shadows: &sun_shadows::SunShadows) -> u32 {
        self.draw_canopy(pass, Some(shadows))
    }
    fn draw_canopy(&self, pass: &mut wgpu::RenderPass, shadows: Option<&sun_shadows::SunShadows>) -> u32 {
        let leaves = &self.canopy_leaves;
        if !leaves.enabled { return 0; }
        let (Some(compiled), Some(vertices), Some(instances)) = (&leaves.compiled, &leaves.vertices, self.gpu.inst_bufs.get(self.frame)) else { return 0; };
        // Deferred pipelines compile on the existing startup worker. Keep native
        // rendering until both programs are ready; never block the first frame.
        if !compiled.beauty.is_ready() || !compiled.shadow.is_ready() { return 0; }
        pass.set_pipeline(if shadows.is_some() { &compiled.shadow } else { &compiled.beauty });
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_bind_group(2, &compiled.bind, &[]);
        if shadows.is_none() && compiled.key.4 == NightMode::Moonlit {
            pass.set_bind_group(3, &self.sun_shadows.as_ref()
                .expect("Moonlit world receiver prepared before canopy").receiver, &[]);
        }
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        let mut calls = 0;
        // Exactly this frame's admitted world instances; no actor-layer emitters,
        // retained visibility or off-screen native animation records.
        for &(_, _, list, index, instance) in &self.sorted {
            let draw = &[&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid][list as usize][index];
            let Some(range) = leaves.ranges.get(&(draw.mesh, draw.first_index)) else { continue; };
            pass.set_bind_group(1, &self.gpu.mats[draw.mat], &[]);
            pass.draw(range.clone(), instance..instance + 1);
            calls += 1;
        }
        calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sprigs_are_bounded_deterministic_and_never_modify_native_geometry() {
        let vertex = |pos| ao_scene::Vertex { pos, normal: [0.0, 0.0, 1.0], uv: [0.5, 0.5], color: [1.0; 4] };
        let submesh = ao_scene::Submesh { vegetation: true, blend: Blend::AlphaTest,
            ..ao_scene::Submesh::new([0, 1, 2].repeat(MAX_PER_SUBMESH * 3),
                Some(TextureKey { rdb_type: 1010004, id: 1 })) };
        let mesh = ao_scene::Mesh { vertices: vec![vertex([0.0, 0.0, 0.0]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])], submeshes: vec![submesh] };
        let original_vertices = mesh.vertices.clone();
        let original_submeshes = mesh.submeshes.clone();
        let (mut first, mut second) = (Vec::new(), Vec::new());
        bake(&mesh, &mesh.submeshes[0], MAX_SPRIGS, &mut first);
        bake(&mesh, &mesh.submeshes[0], MAX_SPRIGS, &mut second);
        assert_eq!(first, second);
        assert_eq!(first.len(), MAX_PER_SUBMESH * 6);
        assert_eq!(mesh.vertices, original_vertices);
        assert_eq!(mesh.submeshes, original_submeshes);
        first.clear();
        bake(&mesh, &mesh.submeshes[0], 2, &mut first);
        assert_eq!(first.len(), 12);
        let mut unmarked = mesh.submeshes[0].clone();
        unmarked.vegetation = false;
        first.clear();
        bake(&mesh, &unmarked, MAX_SPRIGS, &mut first);
        assert!(first.is_empty());
    }

    #[test]
    fn optional_leaf_switches_submit_real_world_and_shadow_draws() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(mut renderer) = Renderer::new(&instance, None) else { return };
        let key = TextureKey { rdb_type: 1010004, id: 1 };
        let vertex = |pos, uv| ao_scene::Vertex { pos, uv, normal: [0.0, 0.0, 1.0], color: [1.0; 4] };
        let mut scene = Scene {
            meshes: vec![ao_scene::Mesh {
                vertices: vec![
                    vertex([-1.0, 0.0, 0.0], [0.0, 0.0]),
                    vertex([1.0, 0.0, 0.0], [1.0, 0.0]),
                    vertex([0.0, 2.0, 0.0], [0.5, 1.0]),
                ],
                submeshes: vec![ao_scene::Submesh {
                    vegetation: true, blend: Blend::AlphaTest, two_sided: true,
                    ..ao_scene::Submesh::new(vec![0, 1, 2], Some(key))
                }],
            }],
            instances: vec![ao_scene::Instance { mesh: 0, transform: Mat4::IDENTITY.to_cols_array_2d() }],
            environment: Some(Environment {
                sky_color: [0.1; 3], fog_color: [0.1; 3], fog_start: 20.0, fog_end: 100.0,
                ambient: [0.2; 3], sun_color: [0.8; 3], sun_dir: [0.3, 0.9, 0.1], sun_specular: 0.0,
            }),
            ground_light: Some(([0.8; 3], [0.3, 0.9, 0.1])), sun_authored: true,
            ..Default::default()
        };
        scene.textures.insert(key, ao_scene::Texture {
            source: Default::default(), width: 2, height: 2,
            rgba: [30, 100, 20, 255].repeat(4),
        });
        renderer.upload(&scene);
        assert!(!renderer.canopy_leaves.baked.is_empty());
        assert!(renderer.canopy_leaves.vertices.is_none(), "upload must remain CPU-only");
        assert!(renderer.canopy_leaves.compiled.is_none());
        let texture = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("leaf draw validation"), size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let view = texture.create_view_tracked(&Default::default());
        let camera = Camera::look_at(Vec3::new(0.0, 1.0, 5.0), Vec3::new(0.0, 1.0, 0.0));
        renderer.render(&view, &Targets::new(&renderer, 64, 64), &camera);
        let native_calls = renderer.stats.draw_calls;
        assert!(native_calls > 0);
        assert!(renderer.canopy_leaves.vertices.is_none());
        assert!(renderer.canopy_leaves.compiled.is_none());
        for (leaves, flutter, falling, shadows) in [
            (true, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
            (true, true, true, true),
        ] {
            renderer.set_enhanced_settings(EnhancedSettings {
                foliage_leaves: leaves, foliage_flutter: flutter, foliage_falling: falling,
                foliage_sway: true, sun_shadows: shadows, ..Default::default()
            });
            renderer.prepare_canopy_leaves();
            renderer.wait_for_pipeline_warmup();
            let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
            renderer.render(&view, &Targets::new(&renderer, 64, 64), &camera);
            renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            assert!(pollster::block_on(errors.pop()).is_none(),
                "leaf draw validation: leaves={leaves}, flutter={flutter}, falling={falling}, shadows={shadows}");
            assert!(renderer.stats.draw_calls > native_calls, "must actually submit the leaf draw");
            assert!(!renderer.sorted.is_empty(), "native plant must be admitted");
            if shadows {
                assert!(renderer.sun_shadows.is_some(), "must encode the leaf caster pipeline too");
            }
        }
        renderer.set_enhanced_settings(EnhancedSettings::default());
        renderer.prepare_canopy_leaves();
        assert!(renderer.canopy_leaves.vertices.is_none(), "switching all off releases optional buffers");
        assert!(renderer.canopy_leaves.compiled.is_none(), "switching all off releases optional programs");
    }

    #[test]
    fn authored_moon_lights_only_admitted_exposed_unfogged_sprigs() {
        use ao_scene::ground_detail::GroundPatch;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("headless leaf moon GPU");
        let key = TextureKey { rdb_type: 1010004, id: 1 };
        // Native triangles remain invisible so positive pixels can only be sprigs.
        let vertex = |pos| ao_scene::Vertex {
            pos, uv: [0.5; 2], normal: [0.0, 0.0, 1.0], color: [1.0, 1.0, 1.0, 0.0],
        };
        let mut scene = Scene {
            meshes: vec![
                ao_scene::Mesh {
                    vertices: vec![vertex([1.0, 0.0, -2.0]), vertex([3.0, 0.0, -2.0]), vertex([2.0, 2.0, -2.0])],
                    submeshes: vec![ao_scene::Submesh {
                        vegetation: true, blend: Blend::AlphaTest, two_sided: true,
                        ..ao_scene::Submesh::new(vec![0, 1, 2], Some(key))
                    }],
                },
                ao_scene::Mesh {
                    vertices: vec![vertex([0.0, 0.0, 0.0]), vertex([4.0, 0.0, 0.0]),
                        vertex([0.0, 0.0, -4.0]), vertex([4.0, 0.0, -4.0])],
                    submeshes: Vec::new(),
                },
            ],
            instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
            ground_detail: vec![GroundPatch { mesh: 1, cells: 1, cell_size: 4.0, ..Default::default() }],
            environment: Some(Environment {
                sky_color: [0.01; 3], fog_color: [0.02; 3], fog_start: 100.0, fog_end: 120.0,
                ambient: [0.05; 3], sun_color: [0.0; 3], sun_dir: [0.0, -1.0, 0.0], sun_specular: 0.0,
            }),
            daylight: Some(0.0),
            ..Default::default()
        };
        scene.moons[0] = Some(ao_scene::MoonSource { toward_light: [0.0, 0.8, 0.6], ..Default::default() });
        scene.textures.insert(key, ao_scene::Texture {
            source: Default::default(), width: 2, height: 2, rgba: [200, 200, 200, 255].repeat(4),
        });
        let upload = |renderer: &mut Renderer, scene: &Scene, visible: bool| {
            renderer.upload(scene);
            for leaf in &mut renderer.canopy_leaves.baked {
                leaf.native.color[3] = if visible { 1.0 } else { 0.0 };
            }
        };
        upload(&mut renderer, &scene, true);
        let (width, height) = (128, 128);
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("isolated moon sprigs"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let camera = Camera::look_at(Vec3::new(2.0, 1.0, 3.0), Vec3::new(2.0, 1.0, -2.0));
        let capture = |renderer: &mut Renderer, night_mode, moon_shadows| {
            renderer.set_enhanced_settings(EnhancedSettings {
                foliage_leaves: true, night_mode, moon_shadows, shadow_map_size: 256, ..Default::default()
            });
            renderer.prepare_canopy_leaves();
            renderer.wait_for_pipeline_warmup();
            renderer.render(&output.create_view(&Default::default()), &Targets::new(renderer, width, height), &camera);
            texture_to_rgba(renderer, &output, width, height).unwrap()
        };
        let native = capture(&mut renderer, NightMode::AsGame, false);
        let lit = capture(&mut renderer, NightMode::Moonlit, false);
        let brighter = lit.as_chunks::<4>().0.iter().zip(native.as_chunks::<4>().0)
            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| i16::from(*x) - i16::from(*y) > 4)).count();
        assert!(brighter > 4, "sprigs alone must receive positive authored Moon RGB: {brighter}");
        assert_eq!(renderer.sun_shadows.as_ref().unwrap().resolution, 1, "Moonlit needs no sun cascades");
        capture(&mut renderer, NightMode::Moonlit, true);
        capture(&mut renderer, NightMode::Dark, false);
        assert_eq!(native, capture(&mut renderer, NightMode::AsGame, false), "night toggles rebuild and roundtrip");
        for vertex in &mut scene.meshes[1].vertices { vertex.color[3] = 1.0; }
        upload(&mut renderer, &scene, true);
        let roofed = capture(&mut renderer, NightMode::AsGame, false);
        assert_eq!(roofed, capture(&mut renderer, NightMode::Moonlit, false), "native lattice closes roofed sprigs");
        for vertex in &mut scene.meshes[1].vertices { vertex.color[3] = 0.0; }
        upload(&mut renderer, &scene, true);
        renderer.env.fog_start = 0.0;
        renderer.env.fog_end = 1.0;
        let fogged = capture(&mut renderer, NightMode::AsGame, false);
        assert_eq!(fogged, capture(&mut renderer, NightMode::Moonlit, false), "native fog extinguishes added illumination");
        upload(&mut renderer, &scene, false);
        let hidden = capture(&mut renderer, NightMode::AsGame, false);
        assert_eq!(hidden, capture(&mut renderer, NightMode::Moonlit, true), "hidden sprigs emit no lighting or shadow signal");
        scene.instances[0].transform = Mat4::from_translation(Vec3::new(10000.0, 0.0, 0.0)).to_cols_array_2d();
        upload(&mut renderer, &scene, true);
        let absent = capture(&mut renderer, NightMode::AsGame, false);
        assert_eq!(absent, capture(&mut renderer, NightMode::Moonlit, true), "nonadmitted sprigs emit no signal");
        assert!(renderer.sorted.is_empty());
    }
}
