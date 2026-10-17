//! Counts actual pipeline creation on threads currently encoding a frame.
use std::cell::Cell;
thread_local! {
    static FRAME_DEPTH: Cell<u32> = const { Cell::new(0) };
    static FRAME_COMPILES: Cell<u64> = const { Cell::new(0) };
}
pub(super) struct FrameGuard;
impl FrameGuard {
    pub fn enter() -> Self {
        FRAME_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}
impl Drop for FrameGuard {
    fn drop(&mut self) { FRAME_DEPTH.with(|depth| depth.set(depth.get() - 1)); }
}
pub(super) fn frame_thread_compiles() -> u64 { FRAME_COMPILES.with(Cell::get) }
fn record() {
    if FRAME_DEPTH.with(|depth| depth.get() != 0) {
        FRAME_COMPILES.with(|count| count.set(count.get() + 1));
    }
}
pub(super) fn render(device: &wgpu::Device, descriptor: &wgpu::RenderPipelineDescriptor<'_>) -> wgpu::RenderPipeline {
    record();
    device.create_render_pipeline(descriptor)
}
pub(super) fn compute(device: &wgpu::Device, descriptor: &wgpu::ComputePipelineDescriptor<'_>) -> wgpu::ComputePipeline {
    record();
    device.create_compute_pipeline(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_only_inside_nested_frame_scopes() {
        let before = frame_thread_compiles();
        record();
        assert_eq!(frame_thread_compiles(), before);
        {
            let _frame = FrameGuard::enter();
            record();
            { let _nested = FrameGuard::enter(); record(); }
            record();
        }
        record();
        assert_eq!(frame_thread_compiles(), before + 3);
    }
}
