//! TAA (slot 5b): Halton-8 jitter (`render_v2`), the jittered input reconstructed on the unjittered output grid (12x12 tile, 2-pixel
//! halo), catmull-rom history, 3x3 variance clip of the reconstructed neighbours, depth-dilated motion (actors from G2, the rest
//! reprojected from depth), history weight 1/n with the sample count n (<= 32) in the history alpha. History is a full-resolution
//! RGBA16F ping-pong; CAS runs in post on its output.
//!
//! Fairness (DESIGN section 8): the history is discarded outright whenever the drawn-actor set changes ([`reset_reason`]), so
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

/// Why the history must not be blended (first frame / resize (`prev == None`), admission set change, camera cut) and the
/// `AOMAC_V2_TEMPORAL_TRACE` log text of it; `None` keeps the history.
pub(super) fn reset_reason(prev: Option<&HistoryKey>, now: &HistoryKey) -> Option<&'static str> {
    let Some(prev) = prev else { return Some("first-frame/resize") };
    if prev.admission != now.admission { Some("admission-change") }
    else if prev.eye.distance_squared(now.eye) > CUT_DISTANCE * CUT_DISTANCE { Some("camera-teleport") }
    else if prev.forward.dot(now.forward) < CUT_COS { Some("camera-turn") }
    else { None }
}

static TRACE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("AOMAC_V2_TEMPORAL_TRACE").is_some_and(|v| v != "0"));
static TRACE_LAST_TIME: std::sync::Mutex<Option<f32>> = std::sync::Mutex::new(None);

/// One frame of the opt-in `AOMAC_V2_TEMPORAL_TRACE=1` log on stderr (nothing is computed or locked when it is off): frame index,
/// scene time and dt, camera, jitter, drawn-actor hash + item count, and which temporal histories were reset and why.
pub(super) struct TraceFrame<'a> {
    pub frame: u32,
    pub time: f32,
    pub eye: Vec3,
    pub forward: Vec3,
    pub jitter: [f32; 2],
    pub admission: u32,
    pub drawn_items: usize,
    pub taa_reset: Option<&'a str>,
    pub ao_reset: Option<&'a str>,
}

pub(super) fn trace_enabled() -> bool { *TRACE }

pub(super) fn trace(f: &TraceFrame) {
    if !*TRACE { return; }
    let dt = TRACE_LAST_TIME.lock().ok().and_then(|mut last| last.replace(f.time)).map(|prev| f.time - prev);
    eprintln!("[v2-temporal] frame={} t={:.4} dt={} eye=({:.3},{:.3},{:.3}) fwd=({:.4},{:.4},{:.4}) jitter=({:.3},{:.3}) admitted={:08x} items={} taa_reset={} ao_reset={}",
        f.frame, f.time, dt.map_or_else(|| "-".to_string(), |d| format!("{d:.4}")), f.eye.x, f.eye.y, f.eye.z, f.forward.x, f.forward.y, f.forward.z,
        f.jitter[0], f.jitter[1], f.admission, f.drawn_items, f.taa_reset.unwrap_or("no"), f.ao_reset.unwrap_or("no"));
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
            post::sampler_entry(5), post::storage_entry(6), post::tex_entry(7, C, true), post::uniform_entry(8, C), post::tex_entry(9, C, false),
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
                post::view_bind(1, &t.hdr), post::view_bind(2, &t.depth), post::view_bind(3, read), post::view_bind(4, &t.g2), post::view_bind(7, halo), post::view_bind(9, &t.g3),
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
        let history_reset = |prev: Option<&HistoryKey>, now: &HistoryKey| reset_reason(prev, now).is_some();
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

#[cfg(test)]
mod jitter_tests {
    /// CPU mirror of `recon` (1-D, pixels): sample position relative to the output pixel's own render pixel centre.
    fn recon_1d(row: &[f32], t: usize, j: f32) -> f32 {
        let d: isize = if j >= 0.0 { 1 } else { -1 };
        let w = j.abs();
        row[t] * (1.0 - w) + row[(t as isize + d) as usize] * w
    }

    /// CPU mirror of the count update in `main`.
    fn next_count(prev: f32, speed: f32, reject: f32) -> f32 {
        let cap = 32.0 + (2.9 - 32.0) * (speed * 0.25).clamp(0.0, 1.0);
        let grown = (prev + 1.0).min(cap);
        (grown + (grown.min(8.0) - grown) * reject).max(1.0)
    }

    #[test]
    fn reconstruction_recovers_the_unjittered_grid_of_a_linear_ramp() {
        // Render image of the ramp f(s) = s sampled at p + 0.5 - j: pixel p holds p + 0.5 - j. Output pixel o must read o + 0.5.
        for j in [-0.43_f32, -0.1, 0.0, 0.27, 0.49] {
            let row: Vec<f32> = (0..16).map(|p| p as f32 + 0.5 - j).collect();
            for o in 2..14 {
                assert!((recon_1d(&row, o, j) - (o as f32 + 0.5)).abs() < 1e-5, "jitter {j} output pixel {o}");
            }
        }
    }

    #[test]
    fn count_caps_at_32_resets_on_rejection_and_stays_short_while_moving() {
        let mut n = 0.0;
        for _ in 0..100 { n = next_count(n, 0.0, 0.0); }
        assert_eq!(n, 32.0, "static history saturates at the cap (weight 1/32)");
        assert_eq!(next_count(0.0, 0.0, 0.0), 1.0, "first frame after a reset takes the current colour (weight 1)");
        assert_eq!(next_count(32.0, 0.0, 1.0), 8.0, "a fully rejected history falls back to 8 samples, not 1 (thin-feature flicker)");
        assert_eq!(next_count(2.0, 0.0, 1.0), 3.0, "a young history is not reset by a rejection");
        assert!(next_count(32.0, 8.0, 0.0) < 3.0, "fast motion keeps the weight near 0.35");
    }

    #[test]
    fn shader_uses_the_tile_halo_count_alpha_and_no_constant_alpha() {
        let wgsl = include_str!("taa.wgsl");
        for needle in ["array<vec4<f32>, 144>", "- vec2<i32>(2)", "const MAX_COUNT: f32 = 32.0", "1.0 / count", "vec4<f32>(min(out_lin, vec3<f32>(OUT_MAX)), count)", "o + 0.5 + jitter"] {
            assert!(wgsl.contains(needle), "taa.wgsl lost `{needle}`");
        }
        assert!(!wgsl.contains("ALPHA_STATIC") && !wgsl.contains("ALPHA_MOVING"), "the constant-alpha EMA is gone");
    }
}
