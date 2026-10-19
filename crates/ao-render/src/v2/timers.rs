//! Per-pass GPU timers: one begin/end timestamp pair on every v2 pass, resolved into a small ring so the offline harness can
//! keep several frames in flight (a GPU that waits for the CPU every frame downclocks and inflates every number).
//! Disabled (no queries, no readback) unless `Renderer::set_v2_pass_timing(true)`.
//!
//! Metal tile GPUs sample these at encoder boundaries and overlap the vertex work of one pass with the fragment work of the
//! previous one: per-pass spans are non-exclusive and do not sum to the frame (`frame` = first begin to last end).
use std::cell::{Cell, RefCell};

const MAX_PASSES: u32 = 32;
const SLOTS: usize = 4;

pub(super) struct PassTimers {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: Vec<wgpu::Buffer>,
    period_ns: f32,
    names: RefCell<[Vec<&'static str>; SLOTS]>,
    slot: Cell<usize>,
    submissions: RefCell<[Option<wgpu::SubmissionIndex>; SLOTS]>,
    marker: Marker,
}

const SLOT_BYTES: u64 = MAX_PASSES as u64 * 16;

impl PassTimers {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) { return None; }
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("v2 pass timers"), ty: wgpu::QueryType::Timestamp, count: MAX_PASSES * 2 * SLOTS as u32 }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 timer resolve"), size: SLOT_BYTES * SLOTS as u64,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false }),
            readback: (0..SLOTS).map(|_| device.create_buffer(&wgpu::BufferDescriptor { label: Some("v2 timer readback"), size: SLOT_BYTES,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })).collect(),
            period_ns: queue.get_timestamp_period(),
            names: RefCell::new(Default::default()),
            slot: Cell::new(0),
            submissions: RefCell::new(Default::default()),
            marker: Marker::new(device),
        })
    }

    /// Starts a new frame in the next ring slot.
    pub(super) fn begin_frame(&self) {
        let slot = (self.slot.get() + 1) % SLOTS;
        self.slot.set(slot);
        self.names.borrow_mut()[slot].clear();
    }

    fn slot(&self, name: &'static str) -> Option<(u32, u32)> {
        let slot = self.slot.get();
        let mut names = self.names.borrow_mut();
        let i = names[slot].len() as u32;
        (i < MAX_PASSES).then(|| { names[slot].push(name); let base = (slot as u32 * MAX_PASSES + i) * 2; (base, base + 1) })
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

    /// One label over several passes (work encoded by code that opens its own passes): put the first writes on a tiny
    /// pass before that work and the second on a tiny pass after it.
    pub(super) fn compute_span(&self, name: &'static str) -> Option<(wgpu::ComputePassTimestampWrites<'_>, wgpu::ComputePassTimestampWrites<'_>)> {
        let (begin, end) = self.slot(name)?;
        Some((
            wgpu::ComputePassTimestampWrites { query_set: &self.queries, beginning_of_pass_write_index: Some(begin), end_of_pass_write_index: None },
            wgpu::ComputePassTimestampWrites { query_set: &self.queries, beginning_of_pass_write_index: None, end_of_pass_write_index: Some(end) },
        ))
    }

    /// Records the resolve + copy; call once, after every timed pass of the frame, before `finish()`.
    pub(super) fn resolve(&self, enc: &mut wgpu::CommandEncoder) {
        let slot = self.slot.get();
        let n = self.names.borrow()[slot].len() as u32;
        if n == 0 { return; }
        let first = slot as u32 * MAX_PASSES * 2;
        enc.resolve_query_set(&self.queries, first..first + n * 2, &self.resolve, slot as u64 * SLOT_BYTES);
        enc.copy_buffer_to_buffer(&self.resolve, slot as u64 * SLOT_BYTES, &self.readback[slot], 0, u64::from(n) * 16);
    }

    /// Remembers the submission that carries this frame's slot (call right after `queue.submit`).
    pub(super) fn submitted(&self, submission: &wgpu::SubmissionIndex) {
        self.submissions.borrow_mut()[self.slot.get()] = Some(submission.clone());
    }

    /// A 1x1 render pass that marks the end of the frame's GPU work (name `tail`; an empty compute pass records no timestamp on
    /// Metal); call after the last timed pass.
    pub(super) fn tail(&self, enc: &mut wgpu::CommandEncoder) {
        self.marker.pass(enc, self.render("tail"));
    }

    /// Blocks until the frame submitted `lag` frames ago (0 = the latest, at most `SLOTS - 1`) completes and returns
    /// `(pass, milliseconds)` in encode order, led by `frame` (first begin to last end).
    pub(super) fn read(&self, device: &wgpu::Device, lag: usize) -> anyhow::Result<Vec<(&'static str, f64)>> {
        anyhow::ensure!(lag < SLOTS, "v2 timer lag {lag} exceeds the ring");
        let slot = (self.slot.get() + SLOTS - lag) % SLOTS;
        let names = self.names.borrow()[slot].clone();
        if names.is_empty() { return Ok(Vec::new()); }
        let slice = self.readback[slot].slice(..u64::try_from(names.len() * 16)?);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| { let _ = tx.send(result); });
        // Wait for exactly this frame's submission (later frames stay in flight), then let the map callback run.
        let submission = self.submissions.borrow()[slot].clone();
        device.poll(wgpu::PollType::Wait { submission_index: submission, timeout: None }).map_err(|e| anyhow::anyhow!("v2 timer wait: {e}"))?;
        let result = loop {
            if let Ok(result) = rx.try_recv() { break result; }
            device.poll(wgpu::PollType::Poll).map_err(|e| anyhow::anyhow!("v2 timer poll: {e}"))?;
        };
        result?;
        let out = {
            let data = slice.get_mapped_range();
            let ticks: &[u64] = bytemuck::cast_slice(&data);
            let n = names.len();
            // Metal leaves the end sample of a render pass without a depth attachment (post) stale; such a pass is taken to end
            // when the next encoder (the `tail` marker) begins.
            let end_of = |i: usize| if ticks[2 * i + 1] >= ticks[2 * i] || i + 1 >= n { ticks[2 * i + 1] } else { ticks[2 * i + 2] };
            let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * f64::from(self.period_ns) / 1.0e6;
            let begin = (0..n).map(|i| ticks[2 * i]).min().unwrap_or(0);
            let end = (0..n).map(end_of).max().unwrap_or(0);
            std::iter::once(("frame", ms(begin, end)))
                .chain(names.iter().enumerate().filter(|(_, name)| **name != "tail").map(|(i, name)| (*name, ms(ticks[2 * i], end_of(i)))))
                .collect()
        };
        self.readback[slot].unmap();
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
