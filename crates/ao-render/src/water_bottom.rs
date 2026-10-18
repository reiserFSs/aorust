//! Actor-free current-camera bottom depth, using native world buffers and materials.
use super::{pipeline_creation::{self, RenderPipeline}, pipelines::{Pipelines, Spec}, Blend, Gpu, GroundRun};
use std::{collections::HashMap, ops::Range};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct BottomSpec {
    cutout: bool,
    cull: bool,
    compare: wgpu::CompareFunction,
    bias: bool,
}
impl BottomSpec {
    const GROUND: Self = Self { cutout: false, cull: true, compare: wgpu::CompareFunction::Less, bias: false };

    fn native(spec: Spec) -> Option<Self> {
        let descriptor = spec.descriptor();
        if spec.actor || spec.sky || !descriptor.depth_write || (12..=17).contains(&spec.effect) {
            return None;
        }
        Some(Self {
            cutout: matches!(spec.blend, Blend::AlphaTest),
            cull: descriptor.cull,
            compare: descriptor.compare,
            bias: descriptor.bias,
        })
    }
}

pub(super) struct WaterBottom {
    pipelines: HashMap<BottomSpec, RenderPipeline>,
}
impl WaterBottom {
    pub(super) fn new(device: &wgpu::Device, g_layout: &wgpu::BindGroupLayout, tex_layout: &wgpu::BindGroupLayout) -> Self {
        let mut source = super::shader_source::load("shader.wgsl", include_str!("shader.wgsl")).into_owned();
        // Calling the native helper retains its texture stages, alpha comparison,
        // UV animation and discard, without writing any colour attachment.
        source.push_str("\n@fragment fn fs_water_bottom_test(i: VOut) { let unused = shade(i, 1u); }\n");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native static water bottom"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native static water bottom"), bind_group_layouts: &[Some(g_layout), Some(tex_layout)], immediate_size: 0,
        });
        // Native layouts are currently local to compile_pipeline, not reusable
        // helpers: these are the exact 48-byte world vertex / 64-byte matrix slots.
        let vertices = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
        let instances = wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
        let buffers = [
            wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertices },
            wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance, attributes: &instances },
        ];
        let native = super::build_pipelines().0;
        let mut pipelines = HashMap::new();
        for key in std::iter::once(BottomSpec::GROUND).chain((0..native.len()).filter_map(|index| BottomSpec::native(native.spec(index)))) {
            pipelines.entry(key).or_insert_with(|| pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
                label: Some("native static water bottom"), layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &buffers },
                fragment: key.cutout.then_some(wgpu::FragmentState {
                    module: &shader, entry_point: Some("fs_water_bottom_test"), compilation_options: Default::default(), targets: &[],
                }),
                primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Ccw, cull_mode: key.cull.then_some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true), depth_compare: Some(key.compare), stencil: Default::default(),
                    bias: if key.bias { wgpu::DepthBiasState { constant: -1, slope_scale: 0.0, clamp: 0.0 } } else { Default::default() },
                }),
                multisample: wgpu::MultisampleState::default(), multiview_mask: None, cache: None,
            }));
        }
        Self { pipelines }
    }

    pub(super) fn is_ready(&self) -> bool { self.pipelines.values().all(RenderPipeline::is_ready) }
    pub(super) fn wait(&self) { for pipeline in self.pipelines.values() { pipeline.get(); } }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(&self, encoder: &mut wgpu::CommandEncoder, depth_target: &wgpu::TextureView,
        globals_bg: &wgpu::BindGroup, gpu: &Gpu, vis_range: &[Range<u32>], frame: usize,
        pipes: &Pipelines, ground: Option<&GroundRun>, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("actor-free static water bottom"), color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_target,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }), timestamp_writes: timestamps, occlusion_query_set: None, multiview_mask: None,
        });
        let Some(instances) = gpu.inst_bufs.get(frame) else { return; };
        pass.set_bind_group(0, globals_bg, &[]);
        pass.set_vertex_buffer(1, instances.slice(..));
        // Deliberately never visits liquid, blended, sky, grass or ActorLayer.
        // Ground detail indices have already been rebuilt in these same buffers.
        let mut previous = None;
        let mut mesh = usize::MAX;
        let mut material = usize::MAX;
        for draw in &gpu.opaque {
            let visible = vis_range[draw.mesh].clone();
            if draw.count == 0 || visible.is_empty() { continue; }
            let is_ground = ground.is_some_and(|ground| ground.materials[draw.mesh].is_some());
            let key = if is_ground { BottomSpec::GROUND } else {
                let Some(key) = BottomSpec::native(pipes.spec(draw.pipe)) else { continue; };
                key
            };
            if previous != Some(key) {
                pass.set_pipeline(self.pipelines[&key].get());
                previous = Some(key);
            }
            if mesh != draw.mesh {
                let (vertices, indices) = gpu.meshes[draw.mesh].as_ref().expect("uploaded static water bottom mesh");
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                mesh = draw.mesh;
            }
            if material != draw.mat {
                pass.set_bind_group(1, &gpu.mats[draw.mat], &[]);
                material = draw.mat;
            }
            pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, visible);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_selection_respects_native_depth_ownership() {
        let base = Spec { blend: Blend::Opaque, two_sided: false, sky: false, env: false,
            fade: false, sprite: false, actor: false, flip_depth: false, effect: 0, compare: None };
        assert!(!BottomSpec::native(base).unwrap().cutout);
        assert!(BottomSpec::native(Spec { blend: Blend::AlphaTest, ..base }).unwrap().cutout);
        assert!(BottomSpec::native(Spec { flip_depth: true, ..base }).is_none());
        assert!(BottomSpec::native(Spec { actor: true, ..base }).is_none());
        assert!(BottomSpec::native(Spec { sky: true, ..base }).is_none());
        assert!(BottomSpec::native(Spec { blend: Blend::AlphaBlend, ..base }).is_none());
        assert!(BottomSpec::native(Spec { effect: 12, ..base }).is_none());
        let forced = BottomSpec::native(Spec { blend: Blend::AlphaBlend, flip_depth: true,
            two_sided: true, compare: Some(wgpu::CompareFunction::Always), ..base }).unwrap();
        assert!(!forced.cull);
        assert_eq!(forced.compare, wgpu::CompareFunction::Always);
        const { assert!(!BottomSpec::GROUND.cutout); }
    }
}
