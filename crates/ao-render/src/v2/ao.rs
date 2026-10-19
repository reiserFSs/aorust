//! Ambient occlusion (slot 3b): half-res GTAO on the shared HiZ + G1 normals (2 slices x 6 steps per side), a 3x3 bilateral +
//! temporal denoise, and a depth-aware upsample into the full-res `R8Unorm` texture the resolve reads at binding 6. The AO texture
//! only ever multiplies the ambient term. Fairness: inputs are the G-buffer depth/normals (admitted actors only) and nothing else;
//! fully fogged and sky texels are 1 and never act as occluders; history is dropped when the admitted-actor hash changes.
use super::gbuffer::Targets;
use super::pyramid::{entry, storage, texture, uniform, Pyramid};
use super::timers::PassTimers;
use std::cell::Cell;

pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const RAW: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// rg = denoised AO, view depth of the texel (the bilateral/temporal weights compare against it).
const HIST: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;

pub(super) struct Pipes {
    gtao: wgpu::ComputePipeline,
    denoise: wgpu::ComputePipeline,
    up: wgpu::RenderPipeline,
    l_gtao: wgpu::BindGroupLayout,
    l_denoise: wgpu::BindGroupLayout,
    l_up: wgpu::BindGroupLayout,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let (cs, fs) = (wgpu::ShaderStages::COMPUTE, wgpu::ShaderStages::FRAGMENT);
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries });
        let l_gtao = layout("v2 gtao", &[uniform(0, cs), texture(1, cs, false), texture(2, cs, false), storage(3, RAW)]);
        let l_denoise = layout("v2 ao denoise", &[uniform(0, cs), texture(1, cs, false), texture(4, cs, false), texture(5, cs, false), storage(6, HIST), uniform(7, cs)]);
        let l_up = layout("v2 ao upsample", &[uniform(0, fs), texture(1, fs, false), texture(8, fs, false)]);
        let source = format!("{}\n{}\n{}\n", crate::shader_source::load("v2/common.wgsl", include_str!("common.wgsl")),
            crate::shader_source::load("v2/packing.wgsl", include_str!("packing.wgsl")), crate::shader_source::load("v2/ao.wgsl", include_str!("ao.wgsl")));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("v2 ao"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline_layout = |label, layout: &wgpu::BindGroupLayout| device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
        let compute = |entry_point, layout: &wgpu::BindGroupLayout| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry_point), layout: Some(&pipeline_layout(entry_point, layout)), module: &module, entry_point: Some(entry_point),
            compilation_options: Default::default(), cache: None });
        let up = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("v2 ao upsample"), layout: Some(&pipeline_layout("v2 ao upsample", &l_up)),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("up_vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("up_fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        Self { gtao: compute("gtao", &l_gtao), denoise: compute("denoise", &l_denoise), up, l_gtao, l_denoise, l_up }
    }
}

/// True when the denoise must not read the history: nothing valid yet (first frame / resize), the admitted-actor set changed
/// (DESIGN section 8: history must not carry a hidden actor's signal across an admission change), or TAA jitter was toggled.
fn history_reset(valid: bool, last_hash: u32, hash: u32, last_jitter: bool, jitter: bool) -> bool {
    !valid || last_hash != hash || last_jitter != jitter
}

/// Size-dependent AO resources and the temporal state that belongs to them.
pub(super) struct Ao {
    view: wgpu::TextureView,
    half: (u32, u32),
    gtao_bind: wgpu::BindGroup,
    /// Indexed by the parity of the frame: reads `hist[p ^ 1]`, writes `hist[p]`.
    denoise_bind: [wgpu::BindGroup; 2],
    /// Indexed by the parity: reads `hist[p]`.
    up_bind: [wgpu::BindGroup; 2],
    params: wgpu::Buffer,
    valid: Cell<bool>,
    hash: Cell<u32>,
    jitter: Cell<bool>,
    parity: Cell<usize>,
}

impl Ao {
    pub(super) fn new(device: &wgpu::Device, pipes: &Pipes, consts: &wgpu::Buffer, pyramid: &Pyramid, targets: &Targets) -> Self {
        let size = targets.size;
        let half = (size.0.div_ceil(2), size.1.div_ceil(2));
        let make = |label, size: (u32, u32), format, usage| device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
        }).create_view(&Default::default());
        let rw = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let raw = make("v2 AO raw", half, RAW, rw);
        let hist = [make("v2 AO history 0", half, HIST, rw), make("v2 AO history 1", half, HIST, rw)];
        let view = make("v2 AO", size, FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 AO params"), size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let fc = || wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() };
        let gtao_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 gtao"), layout: &pipes.l_gtao,
            entries: &[fc(), entry(1, pyramid.view()), entry(2, &targets.g1), entry(3, &raw)] });
        let denoise = |p: usize| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 ao denoise"), layout: &pipes.l_denoise,
            entries: &[fc(), entry(1, pyramid.view()), entry(4, &raw), entry(5, &hist[p ^ 1]), entry(6, &hist[p]),
                wgpu::BindGroupEntry { binding: 7, resource: params.as_entire_binding() }] });
        let up = |p: usize| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("v2 ao upsample"), layout: &pipes.l_up,
            entries: &[fc(), entry(1, pyramid.view()), entry(8, &hist[p])] });
        Self { view, half, gtao_bind, denoise_bind: [denoise(0), denoise(1)], up_bind: [up(0), up(1)], params,
            valid: Cell::new(false), hash: Cell::new(0), jitter: Cell::new(false), parity: Cell::new(0) }
    }

    /// Full-resolution `R8Unorm` AO, 1 = open; bound to the resolve at binding 6 and valid after [`Ao::encode`].
    pub(super) fn view(&self) -> &wgpu::TextureView { &self.view }

    /// Encodes `ao`, `ao_denoise` and `ao_up` (after the pyramid of the same frame). `admission` = `FrameConsts.flags.z`.
    pub(super) fn encode(&self, pipes: &Pipes, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, timer: Option<&PassTimers>, admission: u32, jitter: bool) {
        let reset = history_reset(self.valid.get(), self.hash.get(), admission, self.jitter.get(), jitter);
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&[u32::from(reset), 0, 0, 0]));
        let p = self.parity.get();
        self.valid.set(true);
        self.hash.set(admission);
        self.jitter.set(jitter);
        self.parity.set(p ^ 1);
        let groups = (self.half.0.div_ceil(8), self.half.1.div_ceil(8));
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 gtao"), timestamp_writes: timer.and_then(|t| t.compute("ao")) });
            pass.set_pipeline(&pipes.gtao);
            pass.set_bind_group(0, &self.gtao_bind, &[]);
            pass.dispatch_workgroups(groups.0, groups.1, 1);
        }
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 ao denoise"), timestamp_writes: timer.and_then(|t| t.compute("ao_denoise")) });
            pass.set_pipeline(&pipes.denoise);
            pass.set_bind_group(0, &self.denoise_bind[p], &[]);
            pass.dispatch_workgroups(groups.0, groups.1, 1);
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("v2 ao upsample"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: timer.and_then(|t| t.render("ao_up")), occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&pipes.up);
        pass.set_bind_group(0, &self.up_bind[p], &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::pyramid;

    #[test]
    fn history_resets_on_first_frame_admission_change_and_jitter_toggle() {
        assert!(history_reset(false, 7, 7, true, true), "nothing valid yet (first frame / resize)");
        assert!(!history_reset(true, 7, 7, true, true), "identical frames keep the history");
        assert!(history_reset(true, 7, 8, true, true), "a (dis)appearing actor drops it");
        assert!(history_reset(true, 7, 7, false, true), "TAA switched on");
        assert!(history_reset(true, 7, 7, true, false), "TAA switched off");
    }

    #[test]
    fn half_resolution_covers_odd_viewports() {
        assert_eq!((1921u32.div_ceil(2), 1199u32.div_ceil(2)), (961, 600));
        // The shader reads full-res texel min(2 * hp, size - 1): always inside the viewport.
        let (half, size) = (1921u32.div_ceil(2), 1921u32);
        assert!((2 * (half - 1)).min(size - 1) < size);
        assert_eq!(pyramid::mip_size((1921, 1199), 1), (960, 599));
    }

    #[test]
    fn fog_bound_matches_the_native_fog_depth_rule() {
        // visible(z) in ao.wgsl is native_transmittance(z) > 0 and is the view-depth form of ambient_occlusion::fog_depth_limit.
        let (near, far, end) = (0.1_f32, 100.0_f32, 3.25_f32);
        let proj = glam::Mat4::perspective_rh(1.0, 1.0, near, far);
        let clip = proj * glam::Vec4::new(0.0, 0.0, -end, 1.0);
        let limit = crate::ambient_occlusion::fog_depth_limit(near, far, end);
        assert!((limit - clip.z / clip.w).abs() < 1e-5);
    }
}
