//! TAA (slot 5b): Halton-8 jitter (`render_v2`), catmull-rom history, 3x3 variance clip, depth-dilated motion (actors from G2,
//! the rest reprojected from depth). History is a full-resolution RGBA16F ping-pong; CAS runs in post on its output.
//!
//! Fairness (DESIGN section 8): the history is discarded outright whenever the admitted-actor set changes ([`history_reset`]), so
//! a hidden actor can neither ghost nor leave residue; it is also discarded on frame 0, on a size change and on camera cuts.
use super::{gbuffer::{Targets, HDR}, post::{self, compute_pipeline}};
use glam::Vec3;

const C: wgpu::ShaderStages = wgpu::ShaderStages::COMPUTE;
/// A camera that moved further than this (metres) in one frame, or turned by more than 25 degrees, is a cut.
const CUT_DISTANCE: f32 = 5.0;
const CUT_COS: f32 = 0.906;

/// What the history depends on besides pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HistoryKey {
    /// `FrameConsts::flags.z`: hash of the admitted actor ids.
    pub admission: u32,
    pub eye: Vec3,
    pub forward: Vec3,
}

/// True when the history must not be blended: first frame / resize (`prev == None`), admission set change, camera cut.
pub(super) fn history_reset(prev: Option<&HistoryKey>, now: &HistoryKey) -> bool {
    let Some(prev) = prev else { return true };
    prev.admission != now.admission || prev.eye.distance_squared(now.eye) > CUT_DISTANCE * CUT_DISTANCE || prev.forward.dot(now.forward) < CUT_COS
}

pub(super) struct Taa {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// Size-dependent: the ping-pong history. `binds[k]` reads `hist[k]` and writes `hist[1 - k]`.
pub(super) struct TaaTargets {
    pub hist: [wgpu::TextureView; 2],
    binds: [wgpu::BindGroup; 2],
}

impl Taa {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 taa"), entries: &[
            post::uniform_entry(0, C), post::tex_entry(1, C, false), post::depth_entry(2, C), post::tex_entry(3, C, true), post::tex_entry(4, C, false),
            post::sampler_entry(5), post::storage_entry(6), post::tex_entry(7, C, true), post::uniform_entry(8, C),
        ] });
        let module = post::module(device, "v2 taa", crate::shader_source::load("v2/taa.wgsl", include_str!("taa.wgsl")));
        Self {
            pipeline: compute_pipeline(device, "v2 taa", &module, "main", &layout),
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() }),
        }
    }

    /// `halo` is the half-resolution bloom (added to the resolved HDR image while the TAA input tile is assembled).
    pub(super) fn bind(&self, device: &wgpu::Device, consts: &wgpu::Buffer, ctl: &wgpu::Buffer, halo: &wgpu::TextureView, t: &Targets) -> TaaTargets {
        let hist = [0, 1].map(|_| device.create_texture(&wgpu::TextureDescriptor {
            label: Some("v2 taa history"), size: wgpu::Extent3d { width: t.size.0, height: t.size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HDR,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view(&Default::default()));
        let group = |read: &wgpu::TextureView, write: &wgpu::TextureView| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("v2 taa"), layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() },
                post::view_bind(1, &t.hdr), post::view_bind(2, &t.depth), post::view_bind(3, read), post::view_bind(4, &t.g2), post::view_bind(7, halo),
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                post::view_bind(6, write),
                wgpu::BindGroupEntry { binding: 8, resource: ctl.as_entire_binding() },
            ],
        });
        let binds = [group(&hist[0], &hist[1]), group(&hist[1], &hist[0])];
        TaaTargets { hist, binds }
    }

    /// Resolves frame `index` (alternating 0/1): reads `hist[index]`, writes `hist[1 - index]`.
    pub(super) fn encode(&self, enc: &mut wgpu::CommandEncoder, targets: &TaaTargets, index: usize, size: (u32, u32), timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 taa"), timestamp_writes });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &targets.binds[index], &[]);
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(admission: u32, eye: Vec3, forward: Vec3) -> HistoryKey { HistoryKey { admission, eye, forward } }

    #[test]
    fn history_survives_ordinary_motion_and_resets_on_cuts() {
        let base = key(7, Vec3::ZERO, Vec3::NEG_Z);
        assert!(history_reset(None, &base), "first frame / resize");
        assert!(!history_reset(Some(&base), &key(7, Vec3::new(0.1, 0.0, -0.2), Vec3::new(0.05, 0.0, -1.0).normalize())), "walking, small turn");
        assert!(history_reset(Some(&base), &key(8, Vec3::ZERO, Vec3::NEG_Z)), "admission change (actor shown / hidden)");
        assert!(history_reset(Some(&base), &key(7, Vec3::new(5.1, 0.0, 0.0), Vec3::NEG_Z)), "teleport");
        assert!(!history_reset(Some(&base), &key(7, Vec3::new(4.9, 0.0, 0.0), Vec3::NEG_Z)));
        assert!(history_reset(Some(&base), &key(7, Vec3::ZERO, Vec3::new(1.0, 0.0, -1.0).normalize())), "45 degree flick");
    }

    #[test]
    fn motion_sentinel_and_static_reprojection_are_in_the_shader() {
        let wgsl = include_str!("taa.wgsl");
        assert!(wgsl.contains("abs(mv.x) < 32.0") && wgsl.contains("ctl.taa.x == 0u"));
        assert!(include_str!("motion.wgsl").contains("vec2<f32>(64.0)"));
    }
}
