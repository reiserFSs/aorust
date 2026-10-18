//! Previous-frame selection, with current-frame GPU revocation at every interaction receiver.
use crate::gpu_stats::GpuStatsExt;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver};

struct Pending {
    buffer: wgpu::Buffer,
    result: Option<Receiver<Result<(), wgpu::BufferAsyncError>>>,
    actors: HashMap<u32, u32>,
    frame: u64,
}
pub(super) struct Admission {
    pub buffer: wgpu::Buffer,
    pub admitted: HashSet<u32>,
    pending: Option<Pending>,
    reusable: Option<Pending>,
    frame: u64,
}
impl Admission {
    pub fn new(device: &wgpu::Device) -> Self {
        Self { buffer: Self::buffer(device, 4), admitted: HashSet::new(), pending: None, reusable: None, frame: 0 }
    }
    pub fn invalidate(&mut self) {
        self.admitted.clear();
        self.pending = None;
    }
    fn buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer_tracked(&wgpu::BufferDescriptor {
            label: Some("current interaction visibility"), size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    pub fn resize(&mut self, device: &wgpu::Device, size: u64) -> bool {
        if size <= self.buffer.size() { return false; }
        self.buffer = Self::buffer(device, size.next_power_of_two());
        true
    }
    pub fn begin_frame(&mut self, device: &wgpu::Device) {
        self.frame = self.frame.wrapping_add(1);
        self.admitted.clear();
        // Never wait for graphics. A late result is discarded, not reused as stale admission.
        let _ = device.poll(wgpu::PollType::Poll);
        let ready = self.pending.as_ref().and_then(|p| p.result.as_ref()).and_then(|r| r.try_recv().ok());
        if let Some(result) = ready {
            let mut pending = self.pending.take().unwrap();
            if result.is_ok() {
                if pending.frame.wrapping_add(1) == self.frame {
                    let bytes = pending.buffer.slice(..).get_mapped_range();
                    let visible: &[u32] = bytemuck::cast_slice(&bytes);
                    self.admitted.extend(pending.actors.iter().filter_map(|(&actor, &slot)|
                        visible.get(slot as usize).is_some_and(|&word| word != 0).then_some(actor)));
                }
                pending.buffer.unmap();
            }
            pending.result = None;
            self.reusable = Some(pending);
        }
    }
    pub fn encode(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Buffer, actors: &HashMap<u32, u32>) {
        encoder.clear_buffer(&self.buffer, 0, None);
        if actors.is_empty() { return; }
        encoder.copy_buffer_to_buffer(source, 0, &self.buffer, 0, source.size());
        if self.pending.is_some() { return; }
        let mut pending = self.reusable.take().filter(|p| p.buffer.size() >= source.size()).unwrap_or_else(|| Pending {
            buffer: device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: Some("previous interaction admission readback"), size: source.size(),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }),
            result: None, actors: HashMap::new(), frame: self.frame,
        });
        encoder.copy_buffer_to_buffer(source, 0, &pending.buffer, 0, source.size());
        pending.actors.clone_from(actors);
        pending.frame = self.frame;
        self.pending = Some(pending);
    }
    pub fn submitted(&mut self) {
        if let Some(pending) = self.pending.as_mut().filter(|p| p.result.is_none()) {
            let (send, result) = mpsc::channel();
            pending.buffer.slice(..).map_async(wgpu::MapMode::Read, move |value| { let _ = send.send(value); });
            pending.result = Some(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_previous_readback_is_nonblocking_and_late_result_is_discarded() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("headless admission GPU");
        let mut admission = Admission::new(&renderer.device);
        let (send, result) = mpsc::channel();
        let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("controlled delayed callback"), size: 8,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true,
        });
        admission.pending = Some(Pending { buffer, result: Some(result), actors: [(7, 1)].into_iter().collect(), frame: 0 });
        admission.admitted.insert(7);
        // Keep the callback unresolved while running the real frame poll path.
        let start = std::time::Instant::now();
        admission.begin_frame(&renderer.device);
        let poll_us = start.elapsed().as_micros();
        assert!(admission.admitted.is_empty(), "incomplete previous admission must fail closed");
        assert!(admission.pending.is_some());
        send.send(Ok(())).unwrap();
        admission.begin_frame(&renderer.device);
        assert!(admission.admitted.is_empty(), "two-frame-old admission must never reserve a capped slot");
        assert!(admission.pending.is_none());
        assert!(admission.reusable.is_some(), "completed staging allocation must be reused");
        println!("interaction_admission incomplete_poll_us={poll_us} callback_unresolved=true frame_path_waits=0 late_result_discarded=true");
    }
}
