//! Tracks actual creation and queues owned pipeline factories off the frame thread.
use std::{
    cell::Cell,
    ops::Deref,
    sync::{Arc, LazyLock, OnceLock, atomic::{AtomicBool, Ordering}, mpsc},
};
use parking_lot::Mutex;

type Job = Box<dyn FnOnce() + Send>;
type Factory<T> = Box<dyn FnOnce() -> T + Send>;

fn startup_queue() -> &'static mpsc::Sender<Job> {
    static QUEUE: LazyLock<mpsc::Sender<Job>> = LazyLock::new(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        super::variant::spawn_compile_worker("startup pipeline warmup", move || {
            for job in receiver { job(); }
        });
        sender
    });
    &QUEUE
}

struct CellState<T> {
    result: OnceLock<Result<T, String>>,
    claimed: AtomicBool,
    factory: Mutex<Option<Factory<T>>>,
}

impl<T> CellState<T> {
    fn claim(&self) -> bool {
        self.claimed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    fn compile(&self) {
        let factory = self.factory.lock().take().expect("claimed pipeline factory");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(factory))
            .map_err(|panic| {
                if let Some(message) = panic.downcast_ref::<String>() { message.clone() }
                else if let Some(message) = panic.downcast_ref::<&str>() { (*message).to_owned() }
                else { "pipeline factory panicked".to_owned() }
            });
        let _ = self.result.set(result);
    }
}

/// Shared queued work. Demand only waits for this cell, never the startup tail.
pub(super) struct Deferred<T>(Arc<CellState<T>>);

impl<T> Clone for Deferred<T> {
    fn clone(&self) -> Self { Self(self.0.clone()) }
}

impl<T: Send + Sync + 'static> Deferred<T> {
    pub fn new(factory: impl FnOnce() -> T + Send + 'static) -> Self {
        Self::enqueue(factory, startup_queue())
    }

    fn enqueue(factory: impl FnOnce() -> T + Send + 'static, queue: &mpsc::Sender<Job>) -> Self {
        let cell = Self(Arc::new(CellState {
            result: OnceLock::new(), claimed: AtomicBool::new(false),
            factory: Mutex::new(Some(Box::new(factory))),
        }));
        let weak = Arc::downgrade(&cell.0);
        queue.send(Box::new(move || {
            if let Some(cell) = weak.upgrade() {
                if cell.claim() { cell.compile(); }
            }
        })).expect("startup pipeline queue remains alive");
        cell
    }

    pub fn is_ready(&self) -> bool { matches!(self.0.result.get(), Some(Ok(_))) }

    pub fn get(&self) -> &T {
        if let Some(Ok(value)) = self.0.result.get() { return value; }
        if self.0.result.get().is_none() {
            if FRAME_DEPTH.with(|depth| depth.get() != 0) {
                FRAME_PIPELINE_WAITS.with(|count| count.set(count.get() + 1));
            }
            if self.0.claim() {
                let cell = self.0.clone();
                // A demanded pipeline bypasses even a blocked, unrelated queue head.
                super::variant::spawn_compile_worker("demanded pipeline", move || cell.compile());
            }
        }
        self.wait()
    }

    /// Only compiler workers may claim and execute factories inline.
    pub fn warm(&self) -> &T {
        if let Some(Ok(value)) = self.0.result.get() { return value; }
        if self.0.result.get().is_none() && self.0.claim() { self.0.compile(); }
        self.wait()
    }

    fn wait(&self) -> &T {
        match self.0.result.wait() {
            Ok(value) => value,
            Err(message) => panic!("pipeline compilation failed: {message}"),
        }
    }
}

impl<T: Send + Sync + 'static> Deref for Deferred<T> {
    type Target = T;
    fn deref(&self) -> &T { self.get() }
}

pub(super) type RenderPipeline = Deferred<wgpu::RenderPipeline>;
pub(super) type ComputePipeline = Deferred<wgpu::ComputePipeline>;

struct Options {
    constants: Vec<(String, f64)>,
    zero_initialize_workgroup_memory: bool,
}
impl Options {
    fn new(options: &wgpu::PipelineCompilationOptions<'_>) -> Self {
        Self {
            constants: options.constants.iter().map(|&(name, value)| (name.to_owned(), value)).collect(),
            zero_initialize_workgroup_memory: options.zero_initialize_workgroup_memory,
        }
    }
    fn constants(&self) -> Vec<(&str, f64)> {
        self.constants.iter().map(|(name, value)| (name.as_str(), *value)).collect()
    }
}

pub(super) fn render_deferred(device: &wgpu::Device, descriptor: &wgpu::RenderPipelineDescriptor<'_>) -> RenderPipeline {
    let device = device.clone();
    let label = descriptor.label.map(str::to_owned);
    let layout = descriptor.layout.cloned();
    let vertex_module = descriptor.vertex.module.clone();
    let vertex_entry = descriptor.vertex.entry_point.map(str::to_owned);
    let vertex_options = Options::new(&descriptor.vertex.compilation_options);
    let buffers: Vec<_> = descriptor.vertex.buffers.iter()
        .map(|buffer| (buffer.array_stride, buffer.step_mode, buffer.attributes.to_vec())).collect();
    let fragment = descriptor.fragment.as_ref().map(|fragment| (
        fragment.module.clone(), fragment.entry_point.map(str::to_owned),
        Options::new(&fragment.compilation_options), fragment.targets.to_vec(),
    ));
    let primitive = descriptor.primitive;
    let depth_stencil = descriptor.depth_stencil.clone();
    let multisample = descriptor.multisample;
    let multiview_mask = descriptor.multiview_mask;
    let cache = descriptor.cache.cloned();
    Deferred::new(move || {
        let vertex_constants = vertex_options.constants();
        let fragment_constants = fragment.as_ref().map(|(_, _, options, _)| options.constants());
        let buffers: Vec<_> = buffers.iter().map(|(array_stride, step_mode, attributes)| wgpu::VertexBufferLayout {
            array_stride: *array_stride, step_mode: *step_mode, attributes,
        }).collect();
        render(&device, &wgpu::RenderPipelineDescriptor {
            label: label.as_deref(), layout: layout.as_ref(),
            vertex: wgpu::VertexState {
                module: &vertex_module, entry_point: vertex_entry.as_deref(), buffers: &buffers,
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &vertex_constants,
                    zero_initialize_workgroup_memory: vertex_options.zero_initialize_workgroup_memory,
                },
            },
            primitive, depth_stencil, multisample, multiview_mask, cache: cache.as_ref(),
            fragment: fragment.as_ref().map(|(module, entry, options, targets)| wgpu::FragmentState {
                module, entry_point: entry.as_deref(), targets,
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: fragment_constants.as_deref().unwrap(),
                    zero_initialize_workgroup_memory: options.zero_initialize_workgroup_memory,
                },
            }),
        })
    })
}

pub(super) fn compute_deferred(device: &wgpu::Device, descriptor: &wgpu::ComputePipelineDescriptor<'_>) -> ComputePipeline {
    let device = device.clone();
    let label = descriptor.label.map(str::to_owned);
    let layout = descriptor.layout.cloned();
    let module = descriptor.module.clone();
    let entry_point = descriptor.entry_point.map(str::to_owned);
    let options = Options::new(&descriptor.compilation_options);
    let cache = descriptor.cache.cloned();
    Deferred::new(move || {
        let constants = options.constants();
        compute(&device, &wgpu::ComputePipelineDescriptor {
            label: label.as_deref(), layout: layout.as_ref(), module: &module,
            entry_point: entry_point.as_deref(), cache: cache.as_ref(),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                zero_initialize_workgroup_memory: options.zero_initialize_workgroup_memory,
            },
        })
    })
}
thread_local! {
    static FRAME_DEPTH: Cell<u32> = const { Cell::new(0) };
    static FRAME_COMPILES: Cell<u64> = const { Cell::new(0) };
    static FRAME_PIPELINE_WAITS: Cell<u64> = const { Cell::new(0) };
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
/// Cold pipeline demand during frame encoding, distinct from actual compilation.
pub(super) fn frame_pipeline_waits() -> u64 { FRAME_PIPELINE_WAITS.with(Cell::get) }
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
    use std::time::Duration;

    fn isolated_queue() -> (mpsc::Sender<Job>, std::thread::JoinHandle<()>) {
        let (sender, receiver) = mpsc::channel::<Job>();
        let worker = super::super::variant::spawn_compile_worker("deferred regression queue", move || {
            for job in receiver { job(); }
        });
        (sender, worker)
    }

    #[test]
    fn demanded_get_bypasses_blocked_unused_tail_and_cancels_dropped_cells() {
        let (queue, worker) = isolated_queue();
        let (started, start) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let blocker = Deferred::enqueue(move || {
            started.send(()).unwrap();
            released.recv().unwrap();
        }, &queue);
        start.recv_timeout(Duration::from_secs(5)).unwrap();
        let cancelled_ran = Arc::new(AtomicBool::new(false));
        let ran = cancelled_ran.clone();
        let unused = Deferred::enqueue(move || ran.store(true, Ordering::Release), &queue);
        drop(unused);
        let frame_thread = std::thread::current().id();
        let target = Deferred::enqueue(move || {
            assert_ne!(std::thread::current().id(), frame_thread);
            assert_eq!(FRAME_DEPTH.with(Cell::get), 0, "factory ran on the frame thread");
            record();
            42
        }, &queue);
        let target_clone = target.clone();
        let (done, result) = mpsc::channel();
        let demand = super::super::variant::spawn_compile_worker("deferred demand regression", move || {
            let waits = frame_pipeline_waits();
            let compiles = frame_thread_compiles();
            let value = {
                let _frame = FrameGuard::enter();
                let value = *target_clone.get();
                assert_eq!(frame_pipeline_waits() - waits, 1, "cold demand must count one frame wait");
                assert_eq!(*target_clone.get(), value);
                assert_eq!(frame_pipeline_waits() - waits, 1, "ready demand must not count another wait");
                value
            };
            assert_eq!(*target_clone.get(), value);
            assert_eq!(frame_pipeline_waits() - waits, 1, "outside-frame demand must not count waits");
            done.send((value, frame_thread_compiles() - compiles, frame_pipeline_waits() - waits)).unwrap();
        });
        let demanded = result.recv_timeout(Duration::from_secs(5));
        // Always unblock and join before asserting so a failed regression cannot hang.
        release.send(()).unwrap();
        demand.join().unwrap();
        blocker.get();
        drop(queue);
        worker.join().unwrap();
        assert_eq!(demanded.unwrap(), (42, 0, 1));
        assert!(target.is_ready());
        assert!(!cancelled_ran.load(Ordering::Acquire));
    }

    #[test]
    fn failed_factory_wakes_waiters_and_does_not_kill_queue() {
        let (queue, worker) = isolated_queue();
        let (started, start) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let failed = Deferred::enqueue(move || -> u32 {
            started.send(()).unwrap();
            released.recv().unwrap();
            panic!("deferred regression panic");
        }, &queue);
        start.recv_timeout(Duration::from_secs(5)).unwrap();
        let (waiting, waiters) = mpsc::channel();
        let (failed_result, failures) = mpsc::channel();
        let requesters: [_; 2] = std::array::from_fn(|_| {
            let failed = failed.clone();
            let waiting = waiting.clone();
            let failed_result = failed_result.clone();
            super::super::variant::spawn_compile_worker("deferred panic requester", move || {
                waiting.send(()).unwrap();
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| failed.get()));
                let panic = panic.expect_err("factory failure must propagate to each requester");
                failed_result.send(panic.downcast_ref::<String>().unwrap().clone()).unwrap();
            })
        });
        for _ in 0..2 { waiters.recv_timeout(Duration::from_secs(5)).unwrap(); }
        let (done, completed) = mpsc::channel();
        let healthy = Deferred::enqueue(move || { done.send(()).unwrap(); 7 }, &queue);
        release.send(()).unwrap();
        let completion = completed.recv_timeout(Duration::from_secs(5));
        drop(queue);
        worker.join().unwrap();
        completion.unwrap();
        assert_eq!(*healthy.get(), 7);
        assert!(!failed.is_ready());
        for _ in 0..2 {
            let message = failures.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(message.contains("deferred regression panic"));
        }
        for requester in requesters { requester.join().unwrap(); }
    }

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
