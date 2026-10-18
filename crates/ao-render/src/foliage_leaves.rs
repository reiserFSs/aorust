//! Additive native-texture canopy sprigs. Original meshes are never rewritten.
use super::*;
use std::ops::Range;

const MAX_SPRIGS: usize = 32768;
const MAX_PER_SUBMESH: usize = 4096;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LeafVertex {
    native: ao_scene::Vertex,
    center_phase: [f32; 4],
    falling: f32,
    corner: [f32; 2],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params { control: [f32; 4], wind: [f32; 4] }
#[derive(Default)]
pub(super) struct Leaves {
    vertices: Option<wgpu::Buffer>,
    baked: Vec<LeafVertex>,
    ground: Vec<[f32; 4]>,
    ranges: HashMap<(usize, u32), Range<u32>>,
    compiled: Option<Compiled>,
    enabled: bool,
}
struct Compiled {
    key: (u32, bool, bool, bool, NightMode, bool, bool),
    beauty: [pipeline_creation::RenderPipeline; 2],
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
fn alpha(texture: &ao_scene::Texture, uv: glam::Vec2) -> f32 {
    if texture.width == 0 || texture.height == 0 { return 0.0; }
    let p = glam::Vec2::new(uv.x.rem_euclid(1.0) * texture.width as f32 - 0.5,
        uv.y.rem_euclid(1.0) * texture.height as f32 - 0.5);
    let base = p.floor();
    let f = p - base;
    let at = |dx: i64, dy: i64| {
        let x = (base.x as i64 + dx).rem_euclid(i64::from(texture.width)) as usize;
        let y = (base.y as i64 + dy).rem_euclid(i64::from(texture.height)) as usize;
        texture.rgba.get((y * texture.width as usize + x) * 4 + 3).copied().unwrap_or(0) as f32 / 255.0
    };
    let a = at(0, 0) * (1.0 - f.x) + at(1, 0) * f.x;
    let b = at(0, 1) * (1.0 - f.x) + at(1, 1) * f.x;
    a * (1.0 - f.y) + b * f.y
}

fn bake(mesh: &ao_scene::Mesh, submesh: &ao_scene::Submesh, texture: Option<&ao_scene::Texture>, limit: usize, output: &mut Vec<LeafVertex>) {
    if !submesh.vegetation || submesh.texture.is_none() || !submesh.lighting_enabled
        || !submesh.fog_enabled || !material_depth_write(submesh)
        || !matches!(submesh.blend, Blend::AlphaTest | Blend::AlphaBlend) { return; }
    let start = output.len();
    let limit = limit.min(MAX_PER_SUBMESH);
    let attached_limit = limit.saturating_sub(5.min(limit / 2));
    let uv_area = |triangle: &[u32; 3]| {
        let [Some(a), Some(b), Some(c)] = triangle.map(|index| mesh.vertices.get(index as usize)) else { return 0.0; };
        let a = glam::Vec2::from_array(a.uv);
        let b = glam::Vec2::from_array(b.uv);
        let c = glam::Vec2::from_array(c.uv);
        let area = (b - a).perp_dot(c - a).abs() * 0.5;
        if area.is_finite() { area } else { 0.0 }
    };
    let total_area: f64 = submesh.indices.as_chunks::<3>().0.iter().map(|triangle| f64::from(uv_area(triangle))).sum();
    let density = (attached_limit as f64 / total_area.max(0.000001)).min(2.2 / (4.0 * 0.0625 * 0.0625));
    let mut cumulative = 0.0f64;
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for &index in &submesh.indices {
        if let Some(vertex) = mesh.vertices.get(index as usize) {
            let p = Vec3::from_array(vertex.pos);
            if p.is_finite() { low = low.min(p); high = high.max(p); }
        }
    }
    let crown = (low + high) * 0.5;
    // ao-vk BakeLeaves: an eighth-texture window, density from UV area rather
    // than one tiny, usually occluded copy of each source triangle.
    let win = 0.0625;
    'triangles: for (triangle_index, triangle) in submesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let [Some(a), Some(b), Some(c)] = triangle.map(|index| mesh.vertices.get(index as usize).copied()) else { continue; };
        let vertices = [a, b, c];
        let p = vertices.map(|v| Vec3::from_array(v.pos));
        let uv = vertices.map(|v| glam::Vec2::from_array(v.uv));
        if p.iter().any(|p| !p.is_finite()) || uv.iter().any(|uv| !uv.is_finite()) { continue; }
        let e1 = p[1] - p[0];
        let e2 = p[2] - p[0];
        let d1 = uv[1] - uv[0];
        let d2 = uv[2] - uv[0];
        let det = d1.perp_dot(d2);
        let Some(normal) = e1.cross(e2).try_normalize() else { continue; };
        if det.abs() < 0.000001 { continue; }
        let du = (e1 * d2.y - e2 * d1.y) / det;
        let dv = (e2 * d1.x - e1 * d2.x) / det;
        let before = (cumulative + 0.0000001).floor();
        cumulative += f64::from(uv_area(triangle)) * density;
        let tries = ((cumulative + 0.0000001).floor() - before).min(4096.0) as usize;
        let seed = p.iter().flat_map(|p| p.to_array()).fold(hash(triangle_index as u32), |seed, v| hash(seed ^ v.to_bits()));
        for sample in 0..tries {
            if (output.len() - start) / 6 >= attached_limit { break 'triangles; }
            let mut state = hash(seed ^ sample as u32);
            let mut random = || { state = hash(state.wrapping_add(0x9e3779b9)); (state >> 8) as f32 / 16777216.0 };
            let (mut r1, mut r2) = (random(), random());
            if r1 + r2 > 1.0 { r1 = 1.0 - r1; r2 = 1.0 - r2; }
            let tex = uv[0] + d1 * r1 + d2 * r2;
            if let Some(texture) = texture {
                if alpha(texture, tex) < 0.5 || [
                    glam::Vec2::new(-0.6 * win, 0.0), glam::Vec2::new(0.6 * win, 0.0),
                    glam::Vec2::new(0.0, -0.6 * win), glam::Vec2::new(0.0, 0.6 * win),
                ].iter().map(|off| alpha(texture, tex + *off)).sum::<f32>() < 1.4 { continue; }
            }
            let turn = glam::Quat::from_axis_angle(normal, random() * std::f32::consts::TAU);
            let tilt = glam::Quat::from_axis_angle(turn * du.normalize(), (random() - 0.5) * 1.4);
            let hu = tilt * turn * du * win;
            let hv = tilt * turn * dv * win;
            let on_card = p[0] + e1 * r1 + e2 * r2;
            let outward = (on_card - crown).try_normalize().unwrap_or(normal);
            let center = on_card + normal * ((random() - 0.5) * 1.2 * hv.length())
                + outward * (random() * 0.5 * hv.length());
            let mut n = hu.cross(hv).normalize();
            if n.dot(outward) < 0.0 { n = -n; }
            n = (n + outward * 1.2).normalize();
            let phase = random() * std::f32::consts::TAU;
            let color = std::array::from_fn(|axis| a.color[axis] * (1.0 - r1 - r2) + b.color[axis] * r1 + c.color[axis] * r2);
            for corner in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
                let xy = glam::Vec2::from_array(corner);
                let native = ao_scene::Vertex { pos: (center + hu * xy.x + hv * xy.y).to_array(),
                    normal: n.to_array(), uv: (tex + xy * win).to_array(), color };
                output.push(LeafVertex { native, center_phase: center.extend(phase).to_array(), falling: 0.0, corner });
            }
        }
    }
    let count = (output.len() - start) / 6;
    let detached = count.min(5).min(limit.saturating_sub(count));
    for slot in 0..detached {
        let index = start + slot * count / detached * 6;
        for corner in 0..6 {
            let mut vertex = output[index + corner];
            vertex.falling = 1.0;
            output.push(vertex);
        }
    }
}

fn ground_data(scene: &Scene) -> Vec<[f32; 4]> {
    let mut data = vec![[scene.ground_detail.len() as f32, 0.0, 0.0, 0.0]; 1 + scene.ground_detail.len() * 2];
    for (index, patch) in scene.ground_detail.iter().enumerate() {
        data[1 + index * 2] = [patch.origin[0] as f32, patch.origin[1] as f32, patch.cell_size, patch.cells as f32];
        data[2 + index * 2] = [(data.len() * 4) as f32, 0.0, 0.0, 0.0];
        data.extend(patch.heights.chunks(4).map(|heights| std::array::from_fn(|i|
            heights.get(i).copied().unwrap_or(0) as f32 * patch.height_scale / 256.0)));
    }
    data
}
impl Leaves {
    /// Explicit loading/capture boundary only; ordinary frames remain nonblocking.
    pub(super) fn wait(&self) {
        if let Some(compiled) = &self.compiled {
            for beauty in &compiled.beauty { beauty.get(); }
            compiled.shadow.get();
        }
    }
    pub(super) fn upload(&mut self, _device: &wgpu::Device, scene: &Scene, _gpu: &Gpu) {
        self.ranges.clear();
        self.ground = ground_data(scene);
        let eligible = scene.meshes.iter().flat_map(|mesh| &mesh.submeshes)
            .filter(|submesh| submesh.vegetation && submesh.texture.is_some()).count();
        let per_submesh = MAX_SPRIGS / eligible.max(1);
        let mut vertices = Vec::new();
        for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
            let mut first_index = 0;
            for submesh in &mesh.submeshes {
                let start = vertices.len() as u32;
                bake(mesh, submesh, submesh.texture.and_then(|key| scene.textures.get(&key)), per_submesh, &mut vertices);
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
        // The bind group owns this scene's immutable landing heights, even
        // when the next zone has exactly the same graphics pipeline key.
        self.compiled = None;
    }
}
impl Compiled {
    fn new(renderer: &Renderer, key: (u32, bool, bool, bool, NightMode, bool, bool)) -> Self {
        let device = &renderer.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canopy wind"), entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64) }, count: None,
            }, wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None }],
        });
        // Water visibility changes only the beauty attachment shape. Warm both
        // shapes together and retain one caster, wind binding and leaf buffer.
        let shaders = [false, true].map(|water| {
            let source = shader_source((key.1, key.2, key.3, water), key.4, key.5, key.6);
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native canopy sprigs"), source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        });
        let receiver_layout = (key.4 == NightMode::Moonlit || key.6)
            .then(|| sun_shadows::receiver_layout(device));
        let mut beauty_groups = vec![Some(&renderer.g_layout), Some(&renderer.tex_layout), Some(&layout)];
        if let Some(receiver) = &receiver_layout { beauty_groups.push(Some(receiver)); }
        let beauty_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native canopy beauty"), bind_group_layouts: &beauty_groups, immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native canopy caster"), bind_group_layouts: &[Some(&renderer.g_layout), Some(&renderer.tex_layout), Some(&layout), Some(&renderer.shadow_pipelines.caster_layout)], immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4, 8 => Float32x4, 9 => Float32, 10 => Float32x2];
        let instances = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let buffers = [
            wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<LeafVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes },
            wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance, attributes: &instances },
        ];
        let auxiliary = || Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL });
        let make = |shadow: bool, water: bool| {
            let shader = &shaders[usize::from(water)];
            let targets = [Some(wgpu::ColorTargetState { format: renderer.format, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                key.1.then(auxiliary).flatten(), key.2.then(auxiliary).flatten(), key.3.then(auxiliary).flatten(),
                water.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend: None, write_mask: wgpu::ColorWrites::RED })];
            let target_count = if water { 5 } else if key.3 { 4 } else if key.1 || key.2 { 3 } else { 1 };
            pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some(if shadow { "canopy sun caster" } else { "canopy native lighting" }), layout: Some(if shadow { &shadow_layout } else { &beauty_layout }),
            vertex: wgpu::VertexState { module: shader, entry_point: Some(if shadow { "vs_canopy_shadow" } else { "vs_canopy" }), compilation_options: Default::default(), buffers: &buffers },
            fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(if shadow { "fs_canopy_shadow" } else { "fs_canopy" }), compilation_options: Default::default(), targets: if shadow { &[] } else { &targets[..target_count] } }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: if shadow { 1 } else { key.0 }, alpha_to_coverage_enabled: !shadow && key.0 > 1, ..Default::default() }, multiview_mask: None, cache: None,
            })
        };
        let uniform = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("canopy wind switches"), size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let ground = device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
            label: Some("native leaf landing heights"), contents: bytemuck::cast_slice(&renderer.canopy_leaves.ground), usage: wgpu::BufferUsages::STORAGE,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("canopy wind"), layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: ground.as_entire_binding() }] });
        Self { key, beauty: [make(false, false), make(false, true)], shadow: make(true, false), uniform, bind }
    }
}
fn shader_source(channels: (bool, bool, bool, bool), night: NightMode, moon_shadows: bool, sun_shadows: bool) -> String {
    let leaf = crate::shader_source::load("foliage_leaves.wgsl", include_str!("foliage_leaves.wgsl"))
        .replace("//__AMBIENT_FIELD__", if channels.0 { "@location(1) ambient: vec4<f32>," } else { "" })
        .replace("//__EMISSIVE_FIELD__", if channels.1 { "@location(2) emissive: vec4<f32>," } else { "" })
        .replace("//__SUN_FIELD__", if channels.2 { "@location(3) sun: vec4<f32>," } else { "" })
        .replace("//__WATER_FIELD__", if channels.3 { "@location(4) water_cloud: f32," } else { "" })
        .replace("//__AMBIENT_WRITE__", if channels.0 { "output.ambient = vec4<f32>(0.0);" } else { "" })
        .replace("//__EMISSIVE_WRITE__", if channels.1 { "output.emissive = vec4<f32>(0.0);" } else { "" })
        .replace("//__SUN_WRITE__", if channels.2 { "output.sun = enhanced_sun(receiver, output.beauty, false, 0.0);" } else { "" })
        .replace("//__WATER_WRITE__", if channels.3 { "output.water_cloud = 0.0;" } else { "" })
        .replace("//__MOON_RECEIVER__", if night != NightMode::AsGame {
            "let receiver = authored_moon_receiver(rounded, i.n, false);"
        } else { "let receiver = rounded;" });
    // Reuse native directional budgets and the exact world sun receiver rather
    // than estimating a second sun contribution for sprigs.
    let mut native = crate::enhanced_shader::source(EnhancedSettings {
        hdr: channels.2, sun_shadows: sun_shadows || (night == NightMode::Moonlit && moon_shadows), night_mode: night, ..Default::default()
    });
    native = native.replace("    @location(12) env_uv: vec2<f32>,",
        "    @location(12) env_uv: vec2<f32>,\n    @location(13) canopy_corner: vec2<f32>,");
    native = native.replace("    var o: VOut;", "    var o: VOut;\n    o.canopy_corner = vec2<f32>(0.0);");
    native = native.replace("    o.n = (model * vec4<f32>(v.normal, 0.0)).xyz;",
        "    o.n = normalize((cross(v.m1.xyz, v.m2.xyz) * v.normal.x + cross(v.m2.xyz, v.m0.xyz) * v.normal.y + cross(v.m0.xyz, v.m1.xyz) * v.normal.z) / dot(v.m0.xyz, cross(v.m1.xyz, v.m2.xyz)));");
    crate::foliage::append_shader(&mut native);
    // Sprigs apply branch motion at their shared anchor in canopy_vertex, once.
    native = native.replace("    if (u32(mat.native.z) & 64u) != 0u { o = foliage_vertex(v, o); }", "");
    let native = native.replace("@vertex\nfn vs(", "fn canopy_native_vs(");
    let shadow = crate::shader_source::load("sun_shadows.wgsl", include_str!("sun_shadows.wgsl"));
    let caster = &shadow[shadow.find("@group(3) @binding(3)").expect("caster declarations")..shadow.find("// Vogel (golden-angle) disk").expect("caster end")];
    let declarations = if sun_shadows || (night == NightMode::Moonlit && moon_shadows) { String::new() } else {
        shadow[..shadow.find("@group(3) @binding(3)").expect("receiver end")].to_owned()
    };
    let source = format!("{native}\n{declarations}\n{caster}\n{leaf}");
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
        let key = (self.world.msaa, s.ambient_occlusion, s.emissive_bloom, s.needs_sun_mask(), s.night_mode, s.moon_shadows, s.sun_shadows);
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
        // rendering until both attachment shapes and the shared caster are ready.
        if compiled.beauty.iter().any(|beauty| !beauty.is_ready()) || !compiled.shadow.is_ready() { return 0; }
        pass.set_pipeline(if shadows.is_some() { &compiled.shadow } else { &compiled.beauty[usize::from(self.world_key.water_cloud)] });
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_bind_group(2, &compiled.bind, &[]);
        if shadows.is_none() && (compiled.key.4 == NightMode::Moonlit || compiled.key.6) {
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
    fn canopy_post_mask_reuses_native_directional_budget() {
        for water in [false, true] {
            let source = shader_source((false, false, true, water), NightMode::AsGame, false, true);
            assert!(source.contains("output.sun = enhanced_sun(receiver, output.beauty, false, 0.0);"));
            assert!(source.contains("enhanced_sun_visibility = visible;"));
            assert_eq!(source.contains("@location(4) water_cloud: f32,"), water);
            assert_eq!(source.contains("output.water_cloud = 0.0;"), water);
        }
    }
    #[test]
    fn canopy_night_shader_contract_keeps_shared_receiver_and_native_shading() {
        for night in [NightMode::AsGame, NightMode::Dark, NightMode::Moonlit] {
            for shadows in [false, true] {
                let source = shader_source((true, true, true, false), night, shadows, true);
                let authored = night != NightMode::AsGame;
                assert!(source.contains("@location(14) sun_share: vec4<f32>,"));
                assert_eq!(source.contains("let receiver = authored_moon_receiver(rounded, i.n, false);"), authored);
                assert_eq!(source.matches("fn authored_moon_receiver(").count(), usize::from(authored));
                assert_eq!(source.matches("struct ShadowFrame {").count(), 1);
                assert_eq!(source.contains("fn authored_moon_visibility("), night == NightMode::Moonlit && shadows);
                assert_eq!(source.contains("fn enhanced_sky_open("), night == NightMode::Moonlit);
                assert!(source.contains("output.beauty = shade(receiver, 1u);"));
                assert!(source.contains("if i.color.a <= 0.0 { discard; }"));
                assert!(source.contains("output.fog = max(output.fog, vtx(v).fog);"));
            }
        }
    }
    #[test]
    fn sprigs_are_bounded_deterministic_and_never_modify_native_geometry() {
        let vertex = |pos: [f32; 3]| ao_scene::Vertex { pos, normal: [0.0, 0.0, 1.0], uv: [pos[0], pos[1]], color: [1.0; 4] };
        let submesh = ao_scene::Submesh { vegetation: true, blend: Blend::AlphaTest,
            ..ao_scene::Submesh::new([0, 1, 2].repeat(MAX_PER_SUBMESH * 3),
                Some(TextureKey { rdb_type: 1010004, id: 1 })) };
        let mesh = ao_scene::Mesh { vertices: vec![vertex([0.0, 0.0, 0.0]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])], submeshes: vec![submesh] };
        let original_vertices = mesh.vertices.clone();
        let original_submeshes = mesh.submeshes.clone();
        let (mut first, mut second) = (Vec::new(), Vec::new());
        bake(&mesh, &mesh.submeshes[0], None, MAX_SPRIGS, &mut first);
        bake(&mesh, &mesh.submeshes[0], None, MAX_SPRIGS, &mut second);
        assert_eq!(first, second);
        assert_eq!(first.len(), MAX_PER_SUBMESH * 6);
        assert_eq!(mesh.vertices, original_vertices);
        assert_eq!(mesh.submeshes, original_submeshes);
        first.clear();
        bake(&mesh, &mesh.submeshes[0], None, 2, &mut first);
        assert_eq!(first.len(), 12);
        let mut unmarked = mesh.submeshes[0].clone();
        unmarked.vegetation = false;
        first.clear();
        bake(&mesh, &unmarked, None, MAX_SPRIGS, &mut first);
        assert!(first.is_empty());
    }

    #[test]
    fn native_texture_windows_are_visible_leafy_and_ground_heights_are_exact() {
        let vertex = |pos: [f32; 3], uv| ao_scene::Vertex { pos, uv, normal: [0.0, 0.0, 1.0], color: [1.0; 4] };
        let submesh = ao_scene::Submesh { vegetation: true, blend: Blend::AlphaTest,
            ..ao_scene::Submesh::new(vec![0, 1, 2], Some(TextureKey { rdb_type: 1010004, id: 1 })) };
        let mesh = ao_scene::Mesh {
            vertices: vec![vertex([0.0, 0.0, 0.0], [0.0, 0.0]), vertex([16.0, 0.0, 0.0], [1.0, 0.0]),
                vertex([0.0, 16.0, 0.0], [0.0, 1.0])], submeshes: vec![submesh],
        };
        let mut texture = ao_scene::Texture { source: Default::default(), width: 2, height: 2, rgba: [60, 100, 20, 255].repeat(4) };
        let mut sprigs = Vec::new();
        bake(&mesh, &mesh.submeshes[0], Some(&texture), 128, &mut sprigs);
        let attached = sprigs.iter().filter(|v| v.falling == 0.0).count() / 6;
        assert!(attached > 32, "UV area must create many sprigs, not one per triangle");
        assert_eq!(sprigs.iter().filter(|v| v.falling == 1.0).count(), 30, "five fall slots only");
        let first = &sprigs[..6];
        let center = Vec3::from_array(first[0].center_phase[..3].try_into().unwrap());
        assert!(Vec3::from_array(first[0].native.pos).distance(center) > 1.0, "size must track painted texture proportions");
        assert!((first[1].native.uv[0] - first[0].native.uv[0] - 0.125).abs() < 0.000001);
        assert!(sprigs.iter().all(|v| (Vec3::from_array(v.native.normal).length() - 1.0).abs() < 0.00001));
        for pixel in texture.rgba.chunks_mut(4) { pixel[3] = 0; }
        sprigs.clear();
        bake(&mesh, &mesh.submeshes[0], Some(&texture), 128, &mut sprigs);
        assert!(sprigs.is_empty(), "no detached geometry can originate in a texture hole");
        let scene = Scene { ground_detail: vec![ao_scene::ground_detail::GroundPatch {
            origin: [3, -7], cell_size: 2.0, cells: 1, height_scale: 128.0,
            heights: vec![2, 4, 6, 8], ..Default::default()
        }], ..Default::default() };
        let ground = ground_data(&scene);
        assert_eq!(ground[0][0], 1.0);
        assert_eq!(ground[1], [3.0, -7.0, 2.0, 1.0]);
        assert_eq!(ground[2][0], 12.0);
        assert_eq!(ground[3], [1.0, 2.0, 3.0, 4.0]);
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
            let compiled = renderer.canopy_leaves.compiled.as_ref().unwrap();
            assert!(compiled.beauty.iter().all(|beauty| beauty.is_ready()),
                "warmup must prepare both water attachment shapes");
            let beauty = compiled.beauty.each_ref().map(|pipeline| pipeline.get().clone());
            let caster = compiled.shadow.get().clone();
            let uniform = compiled.uniform.clone();
            let vertices = renderer.canopy_leaves.vertices.clone();
            let baked = renderer.canopy_leaves.baked.clone();
            let water = renderer.world_key.water_cloud;
            for water_cloud in [!water, water] {
                renderer.world_key.water_cloud = water_cloud;
                renderer.prepare_canopy_leaves();
                let compiled = renderer.canopy_leaves.compiled.as_ref().unwrap();
                assert!(compiled.beauty.iter().all(|beauty| beauty.is_ready()));
                assert!(compiled.shadow.is_ready());
                assert_eq!(compiled.beauty[usize::from(water_cloud)].get(), &beauty[usize::from(water_cloud)]);
                assert_eq!(compiled.shadow.get(), &caster, "water flips retain the caster");
                assert_eq!(compiled.uniform, uniform, "water flips retain wind resources");
                assert_eq!(renderer.canopy_leaves.vertices, vertices, "water flips retain leaf buffers");
                assert_eq!(renderer.canopy_leaves.baked, baked, "water flips never regenerate leaves");
            }
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
        let previous_key = renderer.canopy_leaves.compiled.as_ref().unwrap().key;
        let previous_ground = renderer.canopy_leaves.ground.clone();
        scene.ground_detail = vec![ao_scene::ground_detail::GroundPatch {
            origin: [7, -3], cells: 1, cell_size: 4.0, height_scale: 256.0,
            heights: vec![9, 10, 11, 12], ..Default::default()
        }];
        // Exercise the same loading boundary without introducing unrelated
        // terrain draw geometry into this isolated leaf-pipeline fixture.
        renderer.canopy_leaves.upload(&renderer.device, &scene, &renderer.gpu);
        assert_ne!(renderer.canopy_leaves.ground, previous_ground);
        assert!(renderer.canopy_leaves.compiled.is_none(), "zone upload must revoke the old terrain binding");
        renderer.prepare_canopy_leaves();
        assert_eq!(renderer.canopy_leaves.compiled.as_ref().unwrap().key, previous_key,
            "terrain binding must rebuild even though the graphics key is unchanged");
        renderer.set_enhanced_settings(EnhancedSettings::default());
        renderer.prepare_canopy_leaves();
        assert!(renderer.canopy_leaves.vertices.is_none(), "switching all off releases optional buffers");
        assert!(renderer.canopy_leaves.compiled.is_none(), "switching all off releases optional programs");
    }

    #[test]
    fn authored_moon_lights_only_admitted_exposed_unfogged_sprigs() {
        use ao_scene::ground_detail::{GroundCell, GroundPatch};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("headless leaf moon GPU");
        let key = TextureKey { rdb_type: 1010004, id: 1 };
        // Native triangles remain invisible so positive pixels can only be sprigs.
        let vertex = |pos: [f32; 3]| ao_scene::Vertex {
            pos, uv: [pos[0] * 0.25, pos[1] * 0.25], normal: [0.0, 0.0, 1.0], color: [1.0, 1.0, 1.0, 0.0],
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
                    submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2, 1, 3, 2], None)],
                },
            ],
            instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
            ground_detail: vec![GroundPatch {
                mesh: 1, cells: 1, cell_size: 4.0, height_scale: 1.0, heights: vec![0; 4],
                fine_cells: vec![GroundCell { submesh: 0, vertices: [0, 1, 2, 3], ..Default::default() }],
                ..Default::default()
            }],
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
        capture(&mut renderer, NightMode::Dark, true);
        assert_eq!(native, capture(&mut renderer, NightMode::AsGame, true), "moon shadows alone leave native sprigs unchanged");
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
