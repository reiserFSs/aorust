//! Debug-only accounting of GPU resource creation, enabled by `AOMAC_GPU_STATS=1`. Every `create_buffer`,
//! `create_buffer_init`, `create_texture`, `create_bind_group`, `create_sampler` and `create_view` call in this
//! crate that goes through the `*_tracked` extension methods below is counted per call-site (file:line, via
//! `#[track_caller]`), with live byte totals for buffers/textures. A background thread prints the top call sites
//! by count once a second, so a leak from unbounded per-frame GPU object creation shows up immediately: the call
//! site with a count that never drops to zero in steady state is the leak. Zero cost when the env var is unset
//! (one `LazyLock` read per call, no locking, no counting).
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
