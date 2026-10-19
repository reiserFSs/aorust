//! G-buffer pass: world, ground, sky and actors drawn with the native draw lists into a thin G-buffer (DESIGN 1.2),
//! plus the blended/additive remainder ("FX") drawn forward into the HDR target after the resolve.
//!
//! The native vertex stages, draw ordering, instancing and material bind groups are reused unchanged; only the
//! fragment stage differs (`gbuffer.wgsl`). Two pipeline sets are built from the native pipeline tables: `g` (opaque
//! and alpha-tested slots plus the sky layers) and `fx` (every blended slot); each table skips the other's slots.
use crate::pipeline_creation::{self, RenderPipeline};
use crate::pipelines::{Descriptor, Pipelines, Spec};
use crate::{Draw, Renderer, DEPTH};

pub(super) const SKY: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(super) const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(super) const G0: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(super) const G1: wgpu::TextureFormat = wgpu::TextureFormat::Rgb10a2Unorm;
pub(super) const G2: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
pub(super) const G3: wgpu::TextureFormat = wgpu::TextureFormat::Rg11b10Ufloat;

/// Native render-list priorities in submission order (the world pass of `Renderer::render`).
const PRIORITIES: [i32; 9] = [0, 1, 2, 3, 4, 5, 6, 10, 7];

/// Which pass a native pipeline slot belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// Opaque / alpha-tested surface: writes the G-buffer.
    G,
    /// Native sky layer: blends into the sky colour target.
    Sky,
    /// Everything blended: forward into the HDR target after the resolve.
    Fx,
    /// Native passes v2 replaces (the terrain shadow multiply).
    Skip,
}

fn class(d: &Descriptor) -> Class {
    match d.fs {
        "fs_opaque" | "fs_test" | "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" if d.state.is_none() => Class::G,
        "fs_ground_shadow" => Class::Skip,
        fs if fs.starts_with("fs_sky_") => Class::Sky,
        _ => Class::Fx,
    }
}

pub(super) struct Tables {
    pub world: Pipelines,
    pub actor: Pipelines,
    pub ground: Pipelines,
}

pub(super) struct Pipes {
    pub g: Tables,
    pub fx: Tables,
}

/// The native shader with its fragment bodies renamed, the v2 G-buffer wrappers and one linearising wrapper per
/// blended entry point.
pub(super) fn shader_source() -> String {
    let mut source = crate::shader_source::load("shader.wgsl", include_str!("../shader.wgsl")).into_owned();
    let names: Vec<String> = source.lines()
        .filter_map(|line| line.strip_prefix("fn fs_").and_then(|rest| rest.split_once('(')).map(|(name, _)| format!("fs_{name}")))
        .collect();
    source = source.replace("@fragment\nfn fs_", "fn orig_fs_");
    source = source.replace(" -> @location(0) vec4<f32>", " -> vec4<f32>");
    source.push('\n');
    source.push_str(&crate::shader_source::load("v2/packing.wgsl", include_str!("packing.wgsl")));
    source.push_str(&crate::shader_source::load("v2/gbuffer.wgsl", include_str!("gbuffer.wgsl")));
    for name in names {
        if matches!(name.as_str(), "fs_opaque" | "fs_test" | "fs_ground_fused" | "fs_ground_base" | "fs_ground_simple" | "fs_ground_shadow") { continue; }
        if name.starts_with("fs_sky_") {
            // Native sky colour is gamma; the sky target is linear HDR. Blend states are the native ones.
            source.push_str(&format!("@fragment\nfn {name}(i: VOut) -> GOut {{\n    var o: GOut = g_pack(vec3<f32>(0.0), 0u, vec3<f32>(0.0, 1.0, 0.0), 0.0, vec3<f32>(0.0));\n    let c = orig_{name}(i);\n    o.sky = vec4<f32>(lin(c.rgb), c.a);\n    return o;\n}}\n"));
        } else {
            source.push_str(&format!("@fragment\nfn {name}(i: VOut) -> @location(0) vec4<f32> {{\n    let c = orig_{name}(i);\n    return vec4<f32>(lin(c.rgb), c.a);\n}}\n"));
        }
    }
    source
}

fn compile(device: &wgpu::Device, shader: &wgpu::ShaderModule, layouts: &[wgpu::PipelineLayout; 2], spec: Spec, class: Class) -> RenderPipeline {
    let Descriptor { fs, vs, state, depth_write, compare, actor, holo, points, cull, bias } = spec.descriptor();
    let f4 = |o| wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: o, shader_location: 4 + (o / 16) as u32 };
    let inst_attrs = [f4(0), f4(16), f4(32), f4(48)];
    let actor_attrs = [f4(0), f4(16), f4(32), f4(48), f4(64), f4(80), f4(96), f4(112),
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 128, shader_location: 13 }];
    let vert_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
    let off = |format| wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::empty() };
    let on = |format| wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL };
    let targets: Vec<Option<wgpu::ColorTargetState>> = match class {
        Class::G => vec![Some(off(SKY)), Some(on(G0)), Some(on(G1)), Some(off(G2)), Some(on(G3))],
        Class::Sky => vec![Some(wgpu::ColorTargetState { format: SKY, blend: state, write_mask: wgpu::ColorWrites::ALL }),
            Some(off(G0)), Some(off(G1)), Some(off(G2)), Some(off(G3))],
        Class::Fx => vec![Some(wgpu::ColorTargetState { format: HDR, blend: state, write_mask: wgpu::ColorWrites::ALL })],
        Class::Skip => unreachable!("skipped slots are never compiled"),
    };
    pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
        label: Some(fs),
        layout: Some(&layouts[usize::from(holo)]),
        vertex: wgpu::VertexState {
            module: shader, entry_point: Some(vs), compilation_options: Default::default(),
            buffers: &[
                wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vert_attrs },
                wgpu::VertexBufferLayout {
                    array_stride: if actor { std::mem::size_of::<crate::actors::ActorInstance>() as u64 } else { 64 },
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: if actor { &actor_attrs } else { &inst_attrs },
                },
            ],
        },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &targets }),
        primitive: wgpu::PrimitiveState {
            topology: if points { wgpu::PrimitiveTopology::PointList } else { wgpu::PrimitiveTopology::TriangleList },
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: cull.then_some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            // The sky never writes depth: depth == 1 is how every consumer recognises "no geometry".
            format: DEPTH, depth_write_enabled: Some(depth_write && class != Class::Sky), depth_compare: Some(compare), stencil: Default::default(),
            bias: if bias { wgpu::DepthBiasState { constant: -1, slope_scale: 0.0, clamp: 0.0 } } else { Default::default() },
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl Pipes {
    pub(super) fn new(r: &Renderer) -> Self {
        let device = &r.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Enhanced v2 G-buffer"), source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let layout = |label, holo: bool| {
            let mut layouts = vec![Some(&r.g_layout), Some(&r.tex_layout)];
            if holo { layouts.push(Some(&r.holo_layout)); }
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &layouts, immediate_size: 0 })
        };
        let layouts = [layout("v2 world", false), layout("v2 holo", true)];
        let (mut world_g, mut actor_g, mut ground_g) = crate::build_pipelines();
        let (mut world_fx, mut actor_fx, mut ground_fx) = crate::build_pipelines();
        let prepare = |table: &mut Pipelines, wanted: &dyn Fn(Class) -> bool| {
            for index in 0..table.len() {
                let class = class(&table.spec(index).descriptor());
                if wanted(class) {
                    let (device, shader, layouts) = (device.clone(), shader.clone(), layouts.clone());
                    table.ensure(index, &move |spec| compile(&device, &shader, &layouts, spec, class));
                }
            }
        };
        let g = |c: Class| matches!(c, Class::G | Class::Sky);
        let fx = |c: Class| c == Class::Fx;
        prepare(&mut world_g, &g);
        prepare(&mut actor_g, &g);
        prepare(&mut ground_g, &g);
        prepare(&mut world_fx, &fx);
        prepare(&mut actor_fx, &fx);
        // Terrain has no blended pass.
        let _ = &mut ground_fx;
        let pipes = Self {
            g: Tables { world: world_g, actor: actor_g, ground: ground_g },
            fx: Tables { world: world_fx, actor: actor_fx, ground: ground_fx },
        };
        pipes.wait();
        pipes
    }

    fn wait(&self) {
        for t in [&self.g, &self.fx] {
            t.world.wait();
            t.actor.wait();
            t.ground.wait();
        }
    }
}

/// Size-dependent attachments.
pub(super) struct Targets {
    pub size: (u32, u32),
    pub sky: wgpu::TextureView,
    pub g0: wgpu::TextureView,
    pub g1: wgpu::TextureView,
    pub g2: wgpu::TextureView,
    pub g3: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    /// Resolve output (storage write), FX target (render attachment), then post input.
    pub hdr: wgpu::TextureView,
}

impl Targets {
    pub(super) fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let texture = |label, format, usage| device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
        }).create_view(&Default::default());
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        Self {
            size,
            sky: texture("v2 sky colour", SKY, rt),
            g0: texture("v2 G0 albedo+flags", G0, rt),
            g1: texture("v2 G1 normal+aux", G1, rt),
            g2: texture("v2 G2 motion", G2, rt),
            g3: texture("v2 G3 emission", G3, rt),
            depth: texture("v2 depth", DEPTH, rt),
            hdr: texture("v2 HDR", HDR, rt | wgpu::TextureUsages::STORAGE_BINDING),
        }
    }
}

/// Draw-state cache shared by the sky and world draw helpers of one pass.
#[derive(Default)]
pub(super) struct DrawState {
    pipe: std::cell::Cell<usize>,
    mesh: std::cell::Cell<(bool, usize)>,
    mat: std::cell::Cell<(bool, usize)>,
}

impl DrawState {
    pub(super) fn new() -> Self {
        Self { pipe: usize::MAX.into(), mesh: (false, usize::MAX).into(), mat: (false, usize::MAX).into() }
    }
    fn reset(&self) {
        self.pipe.set(usize::MAX);
        self.mesh.set((false, usize::MAX));
        self.mat.set((false, usize::MAX));
    }
}

impl Renderer {
    /// One native draw through `tables`; slots the table skipped are not drawn.
    fn v2_draw(&self, pass: &mut wgpu::RenderPass<'_>, tables: &Tables, st: &DrawState, d: &Draw, insts: std::ops::Range<u32>, sky: bool) -> usize {
        let slot = if d.pipe >= 1000 { tables.ground.get(d.pipe - 1000) } else { tables.world.get(d.pipe) };
        let Some(pipeline) = slot else { return 0 };
        if st.pipe.get() != d.pipe {
            pass.set_pipeline(pipeline);
            st.pipe.set(d.pipe);
        }
        if st.mesh.get() != (sky, d.mesh) {
            let (vb, ib) = if sky { &self.sky.meshes } else { &self.gpu.meshes }[d.mesh].as_ref().unwrap();
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            st.mesh.set((sky, d.mesh));
        }
        if st.mat.get() != (sky, d.mat) {
            pass.set_bind_group(1, &if sky { &self.sky.mats } else { &self.gpu.mats }[d.mat], &[]);
            st.mat.set((sky, d.mat));
        }
        pass.draw_indexed(d.first_index..d.first_index + d.count, 0, insts.clone());
        if !sky && d.count != 0 {
            if let Some((node, next)) = d.node {
                for vi in insts {
                    self.pending_visuals.borrow_mut().insert(crate::VisualKey::World(self.vis_src[vi as usize] as usize, node), next);
                }
            }
        }
        1
    }

    /// The native sky layers (colour fans, discs, clouds) into the sky target.
    pub(super) fn v2_draw_sky(&self, pass: &mut wgpu::RenderPass<'_>, tables: &Tables, st: &DrawState, grid: usize) -> usize {
        let Some(sky_buf) = self.sky.bufs.get(self.frame) else { return 0 };
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_vertex_buffer(1, sky_buf.slice(..));
        st.reset();
        let mut calls = 0;
        for (d, k) in &self.sky.draws {
            let cloud = self.sky.clouds.iter().find(|run| run.layer.mesh == d.mesh);
            if grid == 0 && cloud.is_some() { continue; }
            if let Some(cloud) = cloud {
                let count = ((cloud.grid - 1).pow(2) * 6) as u32;
                for first_index in (d.first_index..d.first_index + d.count).step_by(count as usize) {
                    calls += self.v2_draw(pass, tables, st, &Draw { first_index, count, ..*d }, *k..*k + 1, true);
                }
            } else {
                calls += self.v2_draw(pass, tables, st, d, *k..*k + 1, true);
            }
        }
        calls
    }

    /// Every visible world draw and actor of one native priority.
    fn v2_draw_priority(&self, pass: &mut wgpu::RenderPass<'_>, tables: &Tables, st: &DrawState, inst: &wgpu::Buffer, priority: i32) -> usize {
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_vertex_buffer(1, inst.slice(..));
        st.reset();
        let first = self.sorted.partition_point(|command| command.0 < priority);
        let last = self.sorted.partition_point(|command| command.0 <= priority);
        let mut calls = 0;
        for &(_, _, list, di, vi) in &self.sorted[first..last] {
            let d = &[&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid][list as usize][di];
            calls += self.v2_draw(pass, tables, st, d, vi..vi + 1, false);
        }
        calls + self.draw_actors_with(pass, &|pipe| tables.actor.get(pipe), None, priority, 0..=i32::MAX)
    }

    /// Native terrain: the base stage only (the lightmap multiply stage is replaced by v2 lighting).
    fn v2_draw_ground(&self, pass: &mut wgpu::RenderPass<'_>, tables: &Tables, st: &DrawState, inst: &wgpu::Buffer) -> usize {
        let Some(ground) = &self.ground else { return 0 };
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_vertex_buffer(1, inst.slice(..));
        st.reset();
        let mut calls = 0;
        for d in &self.gpu.opaque {
            let Some(palette) = &ground.materials[d.mesh] else { continue };
            let range = self.vis_range[d.mesh].clone();
            if range.is_empty() || d.count == 0 { continue; }
            pass.set_bind_group(2, palette, &[]);
            let mut gd = *d;
            gd.pipe = 1000 + if ground.has_shadow[d.mesh] { 0 } else { 3 };
            calls += self.v2_draw(pass, tables, st, &gd, range, false);
        }
        calls
    }

    /// The whole G-buffer world pass body: sky, then the native priorities with terrain after priority 0.
    pub(super) fn v2_draw_g(&self, pass: &mut wgpu::RenderPass<'_>, tables: &Tables, grid: usize) -> usize {
        let st = DrawState::new();
        let mut calls = self.v2_draw_sky(pass, tables, &st, grid);
        let Some(inst) = self.gpu.inst_bufs.get(self.frame) else { return calls };
        for priority in PRIORITIES {
            calls += self.v2_draw_priority(pass, tables, &st, inst, priority);
            if priority == 0 { calls += self.v2_draw_ground(pass, tables, &st, inst); }
        }
        calls
    }

    /// The blended remainder, forward into the HDR target.
    pub(super) fn v2_draw_fx(&self, pass: &mut wgpu::RenderPass<'_>, tables: &Tables) -> usize {
        let Some(inst) = self.gpu.inst_bufs.get(self.frame) else { return 0 };
        let st = DrawState::new();
        PRIORITIES.into_iter().map(|priority| self.v2_draw_priority(pass, tables, &st, inst, priority)).sum()
    }
}
