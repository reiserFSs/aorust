//! Per-pass GPU timers: one begin/end timestamp pair on every v2 pass, resolved and read back by the offline harness.
//! Disabled (no queries, no readback) unless `Renderer::set_v2_pass_timing(true)`.
use std::cell::RefCell;

const MAX_PASSES: u32 = 32;

pub(super) struct PassTimers {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f32,
    names: RefCell<Vec<&'static str>>,
}

impl PassTimers {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) { return None; }
        let bytes = u64::from(MAX_PASSES) * 16;
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("v2 pass timers"), ty: wgpu::QueryType::Timestamp, count: MAX_PASSES * 2 }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 timer resolve"), size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false }),
            readback: device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 timer readback"), size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
            period_ns: queue.get_timestamp_period(),
            names: RefCell::new(Vec::new()),
        })
    }

    pub(super) fn begin_frame(&self) { self.names.borrow_mut().clear(); }

    fn slot(&self, name: &'static str) -> Option<(u32, u32)> {
        let mut names = self.names.borrow_mut();
        let i = names.len() as u32;
        (i < MAX_PASSES).then(|| { names.push(name); (2 * i, 2 * i + 1) })
    }

    pub(super) fn render(&self, name: &'static str) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let (begin, end) = self.slot(name)?;
        Some(wgpu::RenderPassTimestampWrites { query_set: &self.queries, beginning_of_pass_write_index: Some(begin), end_of_pass_write_index: Some(end) })
    }

    pub(super) fn compute(&self, name: &'static str) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let (begin, end) = self.slot(name)?;
        Some(wgpu::ComputePassTimestampWrites { query_set: &self.queries, beginning_of_pass_write_index: Some(begin), end_of_pass_write_index: Some(end) })
    }

    /// One label over several consecutive passes: give the first pass `begin_*` and the last `end_*`.
    pub(super) fn span(&self, name: &'static str) -> Option<Span<'_>> {
        let (begin, end) = self.slot(name)?;
        Some(Span { queries: &self.queries, begin, end })
    }

    /// Records the resolve + copy; call once, after every timed pass of the frame, before `finish()`.
    pub(super) fn resolve(&self, enc: &mut wgpu::CommandEncoder) {
        let n = self.names.borrow().len() as u32;
        if n == 0 { return; }
        enc.resolve_query_set(&self.queries, 0..n * 2, &self.resolve, 0);
        enc.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, u64::from(n) * 16);
    }

    /// Blocks until the submitted frame completes and returns `(pass, milliseconds)` in encode order.
    pub(super) fn read(&self, device: &wgpu::Device) -> anyhow::Result<Vec<(&'static str, f64)>> {
        let names = self.names.borrow().clone();
        if names.is_empty() { return Ok(Vec::new()); }
        let slice = self.readback.slice(..u64::try_from(names.len() * 16)?);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| { let _ = tx.send(result); });
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("v2 timer poll: {e}"))?;
        rx.recv()??;
        let out = {
            let data = slice.get_mapped_range();
            let ticks: &[u64] = bytemuck::cast_slice(&data);
            let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * f64::from(self.period_ns) / 1.0e6;
            let begin = (0..names.len()).map(|i| ticks[2 * i]).min().unwrap_or(0);
            let end = (0..names.len()).map(|i| ticks[2 * i + 1]).max().unwrap_or(0);
            // "frame" = first pass begin to last pass end (includes any idle gaps between passes).
            std::iter::once(("frame", ms(begin, end)))
                .chain(names.iter().enumerate().map(|(i, name)| (*name, ms(ticks[2 * i], ticks[2 * i + 1]))))
                .collect()
        };
        self.readback.unmap();
        Ok(out)
    }
}

/// Timestamp indices of one [`PassTimers::span`]: the label's time is first pass begin -> last pass end.
pub(super) struct Span<'a> {
    queries: &'a wgpu::QuerySet,
    begin: u32,
    end: u32,
}

impl Span<'_> {
    pub(super) fn begin_render(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites { query_set: self.queries, beginning_of_pass_write_index: Some(self.begin), end_of_pass_write_index: None }
    }
    pub(super) fn end_render(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites { query_set: self.queries, beginning_of_pass_write_index: None, end_of_pass_write_index: Some(self.end) }
    }
    pub(super) fn end_compute(&self) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites { query_set: self.queries, beginning_of_pass_write_index: None, end_of_pass_write_index: Some(self.end) }
    }
}

/// An (almost free) empty render pass that only carries a timestamp, so a [`Span`] exists every frame even when its real passes are
/// skipped (an empty compute pass records no timestamp on Metal).
pub(super) struct Marker(wgpu::TextureView);

impl Marker {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("v2 timer marker"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: crate::DEPTH, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        }).create_view(&Default::default()))
    }

    /// Does nothing without timestamps (timing disabled), so production frames carry no marker passes.
    pub(super) fn pass(&self, enc: &mut wgpu::CommandEncoder, timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        if timestamp_writes.is_none() { return; }
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("v2 timer marker"), color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.0,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
            timestamp_writes, occlusion_query_set: None, multiview_mask: None,
        }));
    }
}
