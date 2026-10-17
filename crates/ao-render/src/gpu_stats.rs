//! Debug-only accounting of GPU resource creation, enabled by `AOMAC_GPU_STATS=1`. Every `create_buffer`,
//! `create_buffer_init`, `create_texture`, `create_bind_group`, `create_sampler` and `create_view` call in this
//! crate that goes through the `*_tracked` extension methods below is counted per call-site (file:line, via
//! `#[track_caller]`), with cumulative estimated creation bytes for buffers/textures, not live residency.
//! A background thread prints the top creation call sites once a second. Counts never track destruction;
//! sustained creation can identify churn but is not by itself proof of a leak. When unset, accounting does
//! one `LazyLock` read per call, without locking/counting. [`FrameGpuTimer`] is separately opt-in.
use parking_lot::Mutex;
use std::collections::HashMap;
use std::panic::Location;
use std::sync::LazyLock;
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};

static PERF_ENABLED: LazyLock<bool> = LazyLock::new(|| std::env::var_os("AOMAC_PERF").is_some());
static CREATED: AtomicU64 = AtomicU64::new(0);
static CREATED_BYTES: AtomicU64 = AtomicU64::new(0);

/// Cumulative tracked creations, not live resources or physical VRAM residency.
pub(crate) fn perf_snapshot() -> (u64, u64) {
    (CREATED.load(Ordering::Relaxed), CREATED_BYTES.load(Ordering::Relaxed))
}

#[derive(Default, Clone, Copy)]
struct Site {
    count: u64,
    bytes: u64,
}

type Key = (&'static str, u32);

static STATS: Mutex<Option<HashMap<Key, Site>>> = Mutex::new(None);
static GPU_STATS_ENABLED: LazyLock<bool> = LazyLock::new(|| {
    let on = std::env::var_os("AOMAC_GPU_STATS").is_some();
    if on {
        std::thread::spawn(report_loop);
    }
    on
});

fn enabled() -> bool {
    *GPU_STATS_ENABLED
}

fn report_loop() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let mut guard = STATS.lock();
        let Some(map) = guard.as_mut() else { continue };
        let mut rows: Vec<_> = map.iter().filter(|(_, s)| s.count > 0).map(|(k, s)| (*k, *s)).collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|a| std::cmp::Reverse(a.1.count));
        eprintln!("[gpu-stats] creates this second, by call site:");
        for ((file, line), site) in rows.iter().take(25) {
            eprintln!("  {:6}  {:>12} bytes  {file}:{line}", site.count, site.bytes);
        }
        map.clear();
    }
}

#[track_caller]
fn record(bytes: u64) {
    if *PERF_ENABLED {
        CREATED.fetch_add(1, Ordering::Relaxed);
        CREATED_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }
    if !enabled() {
        return;
    }
    let loc = Location::caller();
    let key: Key = (loc.file(), loc.line());
    let mut guard = STATS.lock();
    let entry = guard.get_or_insert_with(HashMap::new).entry(key).or_default();
    entry.count += 1;
    entry.bytes += bytes;
}

fn texture_bytes(desc: &wgpu::TextureDescriptor) -> u64 {
    let texel = u64::from(desc.format.block_copy_size(None).unwrap_or(4));
    let w = u64::from(desc.size.width);
    let h = u64::from(desc.size.height);
    let layers = u64::from(desc.size.depth_or_array_layers);
    w * h * layers * texel * u64::from(desc.mip_level_count.max(1))
}

/// Call-site-tracked `wgpu::Device`/`wgpu::Texture` resource creation; see the module doc.
pub(crate) trait GpuStatsExt {
    fn create_buffer_tracked(&self, desc: &wgpu::BufferDescriptor) -> wgpu::Buffer;
    fn create_buffer_init_tracked(&self, desc: &wgpu::util::BufferInitDescriptor) -> wgpu::Buffer;
    fn create_texture_tracked(&self, desc: &wgpu::TextureDescriptor) -> wgpu::Texture;
    fn create_bind_group_tracked(&self, desc: &wgpu::BindGroupDescriptor) -> wgpu::BindGroup;
    fn create_sampler_tracked(&self, desc: &wgpu::SamplerDescriptor) -> wgpu::Sampler;
}

impl GpuStatsExt for wgpu::Device {
    #[track_caller]
    fn create_buffer_tracked(&self, desc: &wgpu::BufferDescriptor) -> wgpu::Buffer {
        record(desc.size);
        self.create_buffer(desc)
    }
    #[track_caller]
    fn create_buffer_init_tracked(&self, desc: &wgpu::util::BufferInitDescriptor) -> wgpu::Buffer {
        record(desc.contents.len() as u64);
        wgpu::util::DeviceExt::create_buffer_init(self, desc)
    }
    #[track_caller]
    fn create_texture_tracked(&self, desc: &wgpu::TextureDescriptor) -> wgpu::Texture {
        record(texture_bytes(desc));
        self.create_texture(desc)
    }
    #[track_caller]
    fn create_bind_group_tracked(&self, desc: &wgpu::BindGroupDescriptor) -> wgpu::BindGroup {
        record(0);
        self.create_bind_group(desc)
    }
    #[track_caller]
    fn create_sampler_tracked(&self, desc: &wgpu::SamplerDescriptor) -> wgpu::Sampler {
        record(0);
        self.create_sampler(desc)
    }
}

/// Call-site-tracked `wgpu::Texture::create_view`.
pub(crate) trait TextureViewStatsExt {
    fn create_view_tracked(&self, desc: &wgpu::TextureViewDescriptor) -> wgpu::TextureView;
}

impl TextureViewStatsExt for wgpu::Texture {
    #[track_caller]
    fn create_view_tracked(&self, desc: &wgpu::TextureViewDescriptor) -> wgpu::TextureView {
        record(0);
        self.create_view(desc)
    }
}

/// Explicit opt-in GPU pass-boundary timing for offline evidence. Construction allocates one query pair
/// and two persistent buffers; an absent timer performs no timestamp, resolve, copy or readback work.
///
/// Attach `begin_compute_pass` to an actual skinning dispatch pass, or `begin_pass` to the first actual
/// shadow/world pass if no compute pass starts the span. Use `is_recording` to avoid overwriting that start.
/// Attach `end_pass` to the final output pass (or use `begin_pass(true)` when one render pass is the entire
/// span). Submit that encoder, then call `read_ms` before reuse. It waits for rendering completion before
/// resolving counters in a separate submission. GUI work outside these boundaries, CPU encoding and
/// presentation are excluded.
pub struct FrameGpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f32,
    state: TimerState,
    mapped: std::sync::Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimerState {
    Idle,
    Recording,
    Ready,
    Failed,
}

fn advance_timer(state: &mut TimerState, expected: TimerState, next: TimerState) -> anyhow::Result<()> {
    anyhow::ensure!(*state == expected, "GPU timer order: expected {expected:?}, found {state:?}");
    *state = next;
    Ok(())
}

fn timestamp_ms(start: u64, end: u64, period_ns: f32) -> anyhow::Result<f64> {
    anyhow::ensure!(period_ns.is_finite() && period_ns > 0.0, "GPU timestamp period unavailable or invalid");
    let ticks = end.checked_sub(start).ok_or_else(|| anyhow::anyhow!(
        "GPU timestamp counters reversed: start={start}, end={end}, period_ns={period_ns}"
    ))?;
    Ok(ticks as f64 * f64::from(period_ns) / 1_000_000.0)
}

impl FrameGpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> anyhow::Result<Self> {
        let required = wgpu::Features::TIMESTAMP_QUERY;
        anyhow::ensure!(device.features().contains(required), "GPU frame timestamps unsupported: device needs {required:?}");
        let period_ns = queue.get_timestamp_period();
        timestamp_ms(0, 0, period_ns)?;
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("offline GPU frame timestamps"), ty: wgpu::QueryType::Timestamp, count: 2,
        });
        record(0);
        let resolve = device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some("offline GPU timestamp resolve"), size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some("offline GPU timestamp readback"), size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self { queries, resolve, readback, period_ns, state: TimerState::Idle, mapped: Default::default() })
    }

    /// Whether an earlier compute/render pass has already supplied this frame's starting boundary.
    pub fn is_recording(&self) -> bool {
        self.state == TimerState::Recording
    }

    pub(crate) fn query_set(&self) -> &wgpu::QuerySet { &self.queries }

    /// Attach to the first compute pass only when it actually dispatches work.
    pub fn begin_compute_pass(&mut self) -> wgpu::ComputePassTimestampWrites<'_> {
        advance_timer(&mut self.state, TimerState::Idle, TimerState::Recording).expect("GPU timer begin requires previous readback");
        wgpu::ComputePassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: None,
        }
    }

    /// Beginning-of-pass writes avoid encoder timestamp reordering. `last_pass` also writes the end
    /// timestamp when the world pass is the final output pass, without adding a dummy render pass.
    pub fn begin_pass(&mut self, last_pass: bool) -> wgpu::RenderPassTimestampWrites<'_> {
        let next = if last_pass { TimerState::Ready } else { TimerState::Recording };
        advance_timer(&mut self.state, TimerState::Idle, next).expect("GPU timer begin requires previous readback");
        wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: last_pass.then_some(1),
        }
    }

    /// Attach only to the final render pass that actually executes (world, composite, haze or screen).
    pub fn end_pass(&mut self) -> wgpu::RenderPassTimestampWrites<'_> {
        advance_timer(&mut self.state, TimerState::Recording, TimerState::Ready).expect("GPU timer end requires begin");
        wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: None,
            end_of_pass_write_index: Some(1),
        }
    }

    /// Blocking offline-only read, after render submission. Wait for the final pass's counter samples
    /// before resolving them, then submit and wait for the separate resolve/copy/readback work.
    /// Any failure retires this timer; no CPU timing substitute or reversed-counter repair is used.
    pub fn read_ms(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> anyhow::Result<f64> {
        advance_timer(&mut self.state, TimerState::Ready, TimerState::Failed)?;
        device.poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow::anyhow!("GPU timed render completion poll: {error}"))?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offline GPU timestamps after render completion"),
        });
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, 16);
        queue.submit([encoder.finish()]);
        let mapped = self.mapped.clone();
        self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            *mapped.lock() = Some(result);
        });
        if let Err(error) = device.poll(wgpu::PollType::wait_indefinitely()) {
            self.readback.unmap();
            return Err(anyhow::anyhow!("GPU timestamp readback poll: {error}"));
        }
        let Some(result) = self.mapped.lock().take() else {
            self.readback.unmap();
            return Err(anyhow::anyhow!("GPU timestamp mapping callback missing"));
        };
        if let Err(error) = result {
            self.readback.unmap();
            return Err(error.into());
        }
        let bytes = self.readback.slice(..).get_mapped_range();
        let start = u64::from_le_bytes(bytes[0..8].try_into().expect("timestamp buffer start"));
        let end = u64::from_le_bytes(bytes[8..16].try_into().expect("timestamp buffer end"));
        drop(bytes);
        self.readback.unmap();
        let ms = timestamp_ms(start, end, self.period_ns)?;
        self.state = TimerState::Idle;
        Ok(ms)
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    #[test]
    fn timestamp_conversion_rejects_reversal_and_enforces_frame_order() {
        assert_eq!(timestamp_ms(10, 1_000_010, 2.0).unwrap(), 2.0);
        assert_eq!(timestamp_ms(42, 42, 1.0).unwrap(), 0.0);
        for (start, end) in [(1_000_010, 10), (u64::MAX - 2, 2)] {
            let error = timestamp_ms(start, end, 1.0).unwrap_err().to_string();
            assert!(error.contains(&format!("start={start}")));
            assert!(error.contains(&format!("end={end}")));
        }
        for period in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(timestamp_ms(0, 1, period).is_err());
        }
        let mut state = TimerState::Idle;
        assert!(advance_timer(&mut state, TimerState::Ready, TimerState::Failed).is_err());
        advance_timer(&mut state, TimerState::Idle, TimerState::Recording).unwrap();
        assert!(advance_timer(&mut state, TimerState::Idle, TimerState::Recording).is_err());
        advance_timer(&mut state, TimerState::Recording, TimerState::Ready).unwrap();
        assert!(advance_timer(&mut state, TimerState::Recording, TimerState::Ready).is_err());
        advance_timer(&mut state, TimerState::Ready, TimerState::Failed).unwrap();
        assert!(advance_timer(&mut state, TimerState::Idle, TimerState::Recording).is_err());
        // Successful readback is the only production path back to Idle.
        state = TimerState::Idle;
        advance_timer(&mut state, TimerState::Idle, TimerState::Ready).unwrap();
        advance_timer(&mut state, TimerState::Ready, TimerState::Failed).unwrap();
    }
}
