//! Opt-in actual-pass GPU costs. Like ao-vk/rvk/profile.cpp, samples are named and
//! read from reusable frame slots; unlike its Vulkan encoder marks, Metal needs
//! beginning/end-of-pass timestamp writes. Busy slots are skipped, never waited on.
//! Rolling costs include zero for previously observed passes absent from a completed
//! matching-domain frame; GUI batches never dilute world costs (or vice versa).
use crate::gpu_stats::GpuStatsExt;
use std::{cell::RefCell, sync::{Arc, atomic::{AtomicU8, Ordering}}};

const SLOTS: usize = 6;
const PASSES: usize = 1024;
const EFFECTS: usize = 64;
const WINDOW: usize = 120;

#[derive(Clone, Debug)]
pub struct EffectGpuCost {
    pub label: &'static str,
    pub average_ms: f64,
    pub samples: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State { #[default] Idle, Recording, Recorded, Submitted, Pending, Failed }

fn reserve(states: &mut [State]) -> Option<usize> {
    let index = states.iter().position(|state| *state == State::Idle)?;
    states[index] = State::Recording;
    Some(index)
}

fn reserve_pass(count: &mut usize) -> Option<usize> {
    let index = *count;
    *count = count.saturating_add(1);
    (index < PASSES).then_some(index)
}

struct Rolling {
    values: [f64; WINDOW],
    next: usize,
    count: usize,
    sum: f64,
    gui: bool,
}
impl Rolling {
    fn new(gui: bool) -> Self { Self { values: [0.0; WINDOW], next: 0, count: 0, sum: 0.0, gui } }
    fn push(&mut self, value: f64) {
        self.sum += value - self.values[self.next];
        self.values[self.next] = value;
        self.next = (self.next + 1) % WINDOW;
        self.count = (self.count + 1).min(WINDOW);
    }
}

struct Slot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    // 0 = awaiting submission/map callback, 1 = completed/mapped, 2 = mapping failed.
    mapped: Arc<AtomicU8>,
    labels: [Option<&'static str>; PASSES],
    count: usize,
    discard: bool,
    gui: bool,
}

struct Recording {
    queries: wgpu::QuerySet,
    labels: [Option<&'static str>; PASSES],
    count: usize,
    gui: bool,
}
thread_local! {
    // The scope only exists while synchronously encoding one renderer's passes.
    // No device, queue, map, allocation, mutex or clock work on the disabled path.
    static ACTIVE: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

pub(crate) struct RecordingGuard { index: usize }
impl Drop for RecordingGuard {
    fn drop(&mut self) { ACTIVE.with(|active| { active.borrow_mut().take(); }); }
}

pub(crate) struct EffectGpuProfiler {
    slots: [Slot; SLOTS],
    states: [State; SLOTS],
    period_ns: f64,
    rolling: Vec<Rolling>,
    pub(crate) costs: Vec<EffectGpuCost>,
}
impl EffectGpuProfiler {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> anyhow::Result<Self> {
        anyhow::ensure!(device.features().contains(wgpu::Features::TIMESTAMP_QUERY),
            "Per-effect GPU profiling unsupported: adapter/device lacks TIMESTAMP_QUERY");
        let period_ns = f64::from(queue.get_timestamp_period());
        anyhow::ensure!(period_ns.is_finite() && period_ns > 0.0,
            "Per-effect GPU profiling unsupported: GPU timestamp period unavailable");
        let slots = std::array::from_fn(|_| Slot {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("live effect GPU timestamps"), ty: wgpu::QueryType::Timestamp, count: (PASSES * 2) as u32,
            }),
            resolve: device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: Some("live effect GPU timestamp resolve"), size: (PASSES * 16) as u64,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
            }),
            readback: device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: Some("live effect GPU timestamp readback"), size: (PASSES * 16) as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }),
            mapped: Arc::new(AtomicU8::new(0)), labels: [None; PASSES], count: 0, discard: false, gui: false,
        });
        Ok(Self { slots, states: [State::Idle; SLOTS], period_ns,
            rolling: Vec::with_capacity(EFFECTS), costs: Vec::with_capacity(EFFECTS) })
    }

    pub(crate) fn reset_averages(&mut self) {
        self.costs.clear();
        self.rolling.clear();
        // Old submissions still own their slot through render completion and
        // readback callbacks. Retire results, not in-flight GPU resources.
        for slot in &mut self.slots { slot.discard = true; }
    }

    pub(crate) fn begin(&mut self, gui: bool) -> Option<RecordingGuard> {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            if active.is_some() { return None; }
            let index = reserve(&mut self.states)?;
            self.slots[index].discard = false;
            *active = Some(Recording { queries: self.slots[index].queries.clone(), labels: [None; PASSES], count: 0, gui });
            Some(RecordingGuard { index })
        })
    }

    pub(crate) fn finish(&mut self, guard: RecordingGuard) {
        let recording = ACTIVE.with(|active| active.borrow_mut().take());
        let index = guard.index;
        drop(guard);
        let Some(recording) = recording else { self.states[index] = State::Idle; return; };
        if recording.count == 0 {
            record_batch(&mut self.costs, &mut self.rolling, recording.gui, &[]);
            self.states[index] = State::Idle;
            return;
        }
        let slot = &mut self.slots[index];
        // ponytail: fixed query budget; discard an over-budget batch rather than
        // report a partial effect cost. Increase PASSES if pass counts grow.
        if recording.count > PASSES { self.states[index] = State::Idle; return; }
        slot.labels = recording.labels;
        slot.count = recording.count;
        slot.gui = recording.gui;
        // Metal counter samples are available only after the timed submission
        // completes. Resolving here in the timed encoder can read stale counters.
        self.states[index] = State::Recorded;
    }

    pub(crate) fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        // Poll (not Wait) only services ready callbacks. A saturated ring drops
        // samples instead of putting backpressure on rendering.
        if device.poll(wgpu::PollType::Poll).is_err() { return; }
        for index in 0..SLOTS {
            let slot = &self.slots[index];
            if self.states[index] == State::Recorded {
                // poll is called after the encoder captured by finish was submitted.
                // Register against that prior queue work, not against this CPU frame.
                slot.mapped.store(0, Ordering::Release);
                let completed = slot.mapped.clone();
                queue.on_submitted_work_done(move || completed.store(1, Ordering::Release));
                self.states[index] = State::Submitted;
                continue;
            }
            if self.states[index] == State::Submitted {
                if slot.mapped.load(Ordering::Acquire) == 0 { continue; }
                if slot.discard {
                    self.states[index] = State::Idle;
                    continue;
                }
                let bytes = (slot.count * 16) as u64;
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("live GPU timestamps after render completion"),
                });
                encoder.resolve_query_set(&slot.queries, 0..(slot.count * 2) as u32, &slot.resolve, 0);
                encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, bytes);
                slot.mapped.store(0, Ordering::Release);
                let mapped = slot.mapped.clone();
                encoder.map_buffer_on_submit(&slot.readback, wgpu::MapMode::Read, 0..bytes, move |result| {
                    mapped.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
                });
                queue.submit([encoder.finish()]);
                self.states[index] = State::Pending;
                continue;
            }
            if self.states[index] != State::Pending { continue; }
            match self.slots[index].mapped.load(Ordering::Acquire) {
                0 => continue,
                1 => {},
                _ => { self.states[index] = State::Failed; continue; },
            }
            if self.slots[index].discard {
                self.slots[index].readback.unmap();
                self.states[index] = State::Idle;
                continue;
            }
            let slot = &self.slots[index];
            let bytes = slot.readback.slice(0..(slot.count * 16) as u64).get_mapped_range();
            let mut totals = [("", 0.0); EFFECTS];
            let mut used = 0;
            let mut valid = true;
            for (label, pair) in slot.labels[..slot.count].iter().zip(bytes.as_chunks::<16>().0) {
                let Some(label) = *label else { valid = false; break; };
                let start = u64::from_le_bytes(pair[..8].try_into().expect("timestamp start"));
                let end = u64::from_le_bytes(pair[8..].try_into().expect("timestamp end"));
                let Some(ticks) = end.checked_sub(start) else { valid = false; break; };
                let effect = totals[..used].iter().position(|(name, _)| *name == label);
                let effect = match effect {
                    Some(effect) => effect,
                    None if used < EFFECTS => { totals[used].0 = label; used += 1; used - 1 },
                    None => { valid = false; break; },
                };
                totals[effect].1 += ticks as f64 * self.period_ns / 1_000_000.0;
            }
            let gui = slot.gui;
            drop(bytes);
            self.slots[index].readback.unmap();
            self.states[index] = State::Idle;
            if valid { record_batch(&mut self.costs, &mut self.rolling, gui, &totals[..used]); }
        }
    }

}

// Missing cached passes are zero-cost matching-domain frames, not stale active-pass averages.
fn record_batch(costs: &mut Vec<EffectGpuCost>, rolling: &mut Vec<Rolling>, gui: bool, totals: &[(&'static str, f64)]) {
    for &(label, _) in totals {
        if !costs.iter().zip(rolling.iter()).any(|(cost, values)| cost.label == label && values.gui == gui) && costs.len() < EFFECTS {
            costs.push(EffectGpuCost { label, average_ms: 0.0, samples: 0 });
            rolling.push(Rolling::new(gui));
        }
    }
    for (cost, values) in costs.iter_mut().zip(rolling.iter_mut()).filter(|(_, values)| values.gui == gui) {
        let ms = totals.iter().find(|(label, _)| *label == cost.label).map_or(0.0, |(_, ms)| *ms);
        values.push(ms);
        cost.average_ms = values.sum / values.count as f64;
        cost.samples = values.count;
    }
}

fn effect_label(file: &'static str, pass: Option<&str>) -> &'static str {
    match pass {
        Some("Enhanced grass") => return "Grass",
        Some("Enhanced canopy leaves") => return "Canopy leaves",
        Some("profiled world continuation") => return "World",
        Some("motion blur") => return "Motion blur",
        Some("depth of field") => return "Depth of field",
        Some("FXAA") => return "FXAA",
        Some("CAS") => return "CAS",
        Some("Fluid smoke") => return "Fluid smoke",
        Some("Spell particles") => return "Spell particles",
        Some("Material reflections") => return "Material reflections",
        Some("Environment probe") => return "Environment probe",
        _ => {},
    }
    let file = file.rsplit('/').next().unwrap_or(file).strip_suffix(".rs").unwrap_or(file);
    match file {
        "gui" => "GUI", "lib" => "World",
        "grass" => "Grass",
        "shadow_pass" | "sun_shadows" => "Directional shadows",
        "point_shadows" => "Point shadows", "ambient_occlusion" => "Ambient occlusion",
        "contact_shadows" => "Contact shadows", "bloom" => "Bloom",
        "indirect_light" => "Indirect light",
        "grade" => "HDR / grading", "volumetric_clouds" => "Volumetric clouds",
        "cloud_shadows" => "Cloud shadows", "sky_discs" => "Sky discs",
        "rain_wet" => "Rain / wetness", "weather_particles" => "Weather particles",
        "temporal" => "TAA",
        "water_bottom" | "water_optics" => "Water optics", "water_spray" => "Water spray",
        "motion_vectors" | "motion_visibility" => "Motion vectors",
        "enhanced" => "Resolve / presentation", "clustered_lighting" => "Clustered lighting",
        "heat_haze" => "Heat haze", "screen" => "Viewport effects", "skinning" => "Skinning",
        "atmosphere" => "Atmosphere", _ => file,
    }
}

/// Profiling-only continuation of an existing world pass. All attachments were
/// stored by the previous segment; preserve every sample and native draw order.
pub(crate) fn resume_world_pass<'a>(encoder: &'a mut wgpu::CommandEncoder, label: &'static str,
    attachments: &[Option<wgpu::RenderPassColorAttachment<'_>>], depth: &wgpu::TextureView) -> wgpu::RenderPass<'a> {
    let loaded: [Option<wgpu::RenderPassColorAttachment<'_>>; 8] = std::array::from_fn(|index| {
        attachments.get(index).and_then(|attachment| attachment.as_ref()).map(|attachment| {
            let mut attachment = attachment.clone();
            attachment.ops.load = wgpu::LoadOp::Load;
            attachment
        })
    });
    encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label), color_attachments: &loaded[..attachments.len()],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
    })
}

/// Central pass interception keeps all existing helper signatures and offline
/// timer descriptors intact. Explicit timestamps always take priority.
pub(crate) trait EffectPassEncoder {
    #[track_caller]
    fn begin_effect_render_pass<'a>(&'a mut self, descriptor: &wgpu::RenderPassDescriptor<'_>) -> wgpu::RenderPass<'a>;
    #[track_caller]
    fn begin_effect_compute_pass<'a>(&'a mut self, descriptor: &wgpu::ComputePassDescriptor<'_>) -> wgpu::ComputePass<'a>;
}
impl EffectPassEncoder for wgpu::CommandEncoder {
    #[track_caller]
    fn begin_effect_render_pass<'a>(&'a mut self, descriptor: &wgpu::RenderPassDescriptor<'_>) -> wgpu::RenderPass<'a> {
        let file = std::panic::Location::caller().file();
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(recording) = active.as_mut().filter(|_| descriptor.timestamp_writes.is_none()) else {
                return self.begin_render_pass(descriptor);
            };
            let Some(index) = reserve_pass(&mut recording.count) else { return self.begin_render_pass(descriptor); };
            recording.labels[index] = Some(effect_label(file, descriptor.label));
            self.begin_render_pass(&wgpu::RenderPassDescriptor {
                timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                    query_set: &recording.queries, beginning_of_pass_write_index: Some((index * 2) as u32),
                    end_of_pass_write_index: Some((index * 2 + 1) as u32),
                }), ..descriptor.clone()
            })
        })
    }
    #[track_caller]
    fn begin_effect_compute_pass<'a>(&'a mut self, descriptor: &wgpu::ComputePassDescriptor<'_>) -> wgpu::ComputePass<'a> {
        let file = std::panic::Location::caller().file();
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(recording) = active.as_mut().filter(|_| descriptor.timestamp_writes.is_none()) else {
                return self.begin_compute_pass(descriptor);
            };
            let Some(index) = reserve_pass(&mut recording.count) else { return self.begin_compute_pass(descriptor); };
            recording.labels[index] = Some(effect_label(file, descriptor.label));
            self.begin_compute_pass(&wgpu::ComputePassDescriptor {
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: &recording.queries, beginning_of_pass_write_index: Some((index * 2) as u32),
                    end_of_pass_write_index: Some((index * 2 + 1) as u32),
                }), ..descriptor.clone()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rolling_window_evicts_old_samples() {
        let mut rolling = Rolling::new(false);
        for value in 1..=WINDOW + 1 { rolling.push(value as f64); }
        assert_eq!(rolling.count, WINDOW);
        assert_eq!(rolling.next, 1);
        assert_eq!(rolling.sum / rolling.count as f64, (WINDOW as f64 + 3.0) / 2.0);
    }
    #[test]
    fn cached_passes_amortize_zero_only_in_their_own_domain() {
        let (mut costs, mut rolling) = (Vec::with_capacity(EFFECTS), Vec::with_capacity(EFFECTS));
        record_batch(&mut costs, &mut rolling, false, &[("Directional shadows", 6.0), ("Point shadows", 3.0)]);
        record_batch(&mut costs, &mut rolling, true, &[("GUI", 1.0)]);
        assert_eq!(costs[0].average_ms, 6.0);
        assert_eq!(costs[0].samples, 1);
        record_batch(&mut costs, &mut rolling, false, &[("Point shadows", 1.0)]);
        assert_eq!(costs[0].average_ms, 3.0);
        assert_eq!(costs[0].samples, 2);
        assert_eq!(costs[1].average_ms, 2.0);
        assert_eq!(costs[2].average_ms, 1.0);
        assert_eq!(costs[2].samples, 1);
        record_batch(&mut costs, &mut rolling, false, &[]);
        assert_eq!(costs[0].average_ms, 2.0);
        assert_eq!(costs[0].samples, 3);
        assert_eq!(costs[2].samples, 1);
        record_batch(&mut costs, &mut rolling, true, &[]);
        assert_eq!(costs[2].average_ms, 0.5);
        assert_eq!(costs[2].samples, 2);
        assert_eq!(costs[0].samples, 3);
    }
    #[test]
    fn recorded_submitted_pending_and_failed_slots_are_never_reused() {
        let mut states = [State::Idle; SLOTS];
        for state in [State::Recording, State::Recorded, State::Submitted, State::Pending, State::Failed] {
            let mut busy = [state; SLOTS];
            assert_eq!(reserve(&mut busy), None, "{state:?} slots are still owned");
        }
        for index in 0..SLOTS {
            assert_eq!(reserve(&mut states), Some(index));
            assert_eq!(states[index], State::Recording);
            states[index] = State::Pending;
        }
        assert_eq!(reserve(&mut states), None);
        states[2] = State::Failed;
        assert_eq!(reserve(&mut states), None);
        states[4] = State::Idle; // Only a consumed/unmapped callback releases a slot.
        assert_eq!(reserve(&mut states), Some(4));
        assert_eq!(reserve(&mut states), None);
    }
    #[test]
    fn each_pass_reserves_a_distinct_pair_and_overflow_invalidates_batch() {
        let mut count = 0;
        for index in 0..PASSES {
            assert_eq!(reserve_pass(&mut count), Some(index));
        }
        assert_eq!(count, PASSES);
        assert_eq!(reserve_pass(&mut count), None);
        assert!(count > PASSES, "finish must discard the entire overflowed batch");
        assert_eq!(reserve_pass(&mut count), None);
    }
}
