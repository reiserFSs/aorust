//! Hardware mouse reports, summed once per render frame; no pixel/DPI conversion.
use std::sync::Arc;
use parking_lot::Mutex;
use std::time::{Duration, Instant};

// Platform backend liveness, not retail sensitivity or motion scaling.
const NATIVE_GRACE: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Counts {
    active: bool,
    native_report: bool,
    delta: (f32, f32),
    appkit_delta: (f32, f32),
    last_native: Option<Instant>,
}
impl Counts {
    fn add(&mut self, dx: f32, dy: f32) {
        if self.active {
            self.native_report = true;
            self.delta.0 += dx;
            self.delta.1 += dy;
        }
    }
    fn winit(&mut self, dx: f32, dy: f32) {
        if self.active { self.appkit_delta.0 += dx; self.appkit_delta.1 += dy; }
    }
    fn set_active(&mut self, active: bool) {
        if self.active != active || !active {
            self.delta = (0.0, 0.0);
            self.appkit_delta = (0.0, 0.0);
            self.native_report = false;
            self.last_native = None;
        }
        self.active = active;
    }
    fn take(&mut self, now: Instant) -> ((f32, f32), bool) {
        if std::mem::take(&mut self.native_report) { self.last_native = Some(now); }
        let native = self.last_native.is_some_and(|last| now.duration_since(last) < NATIVE_GRACE);
        if !native { self.last_native = None; }
        let raw = std::mem::take(&mut self.delta);
        // Discard duplicates during the grace interval, rather than replay them on handoff.
        let appkit = std::mem::take(&mut self.appkit_delta);
        (if native { raw } else { appkit }, native)
    }
}

pub(crate) struct RawMouse {
    counts: Arc<Mutex<Counts>>,
    backend: Option<bool>,
    perf: bool,
    #[cfg(target_os = "macos")]
    mice: Vec<native::Mouse>,
}
impl RawMouse {
    pub(crate) fn new() -> Self {
        let mut result = Self {
            counts: Arc::new(Mutex::new(Counts::default())),
            backend: None,
            perf: std::env::var_os("AOMAC_PERF").is_some(),
            #[cfg(target_os = "macos")]
            mice: Vec::new(),
        };
        result.refresh();
        result
    }
    fn refresh(&mut self) {
        #[cfg(target_os = "macos")]
        native::refresh(&mut self.mice, &self.counts);
    }
    pub(crate) fn set_active(&mut self, active: bool) {
        self.counts.lock().set_active(active);
    }
    pub(crate) fn push_winit(&mut self, dx: f32, dy: f32) {
        self.counts.lock().winit(dx, dy);
    }
    pub(crate) fn take(&mut self) -> (f32, f32) {
        self.refresh();
        let (delta, native) = self.counts.lock().take(Instant::now());
        if delta != (0.0, 0.0) && self.backend != Some(native) {
            self.backend = Some(native);
            if self.perf { eprintln!("mouse look backend: {}", if native { "GCMouse raw" } else { "winit fallback" }); }
        }
        delta
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use block2::{Block, RcBlock};
    use objc2::{msg_send, runtime::{AnyClass, AnyObject}, rc::Retained};

    #[link(name = "GameController", kind = "framework")]
    unsafe extern "C" {}
    type MouseMoved = Block<dyn Fn(*mut AnyObject, f32, f32)>;

    pub(super) struct Mouse {
        object: Retained<AnyObject>,
        input: Retained<AnyObject>,
    }
    impl Drop for Mouse {
        fn drop(&mut self) {
            let handler: Option<&MouseMoved> = None;
            unsafe { let _: () = msg_send![&*self.input, setMouseMovedHandler: handler]; }
        }
    }
    // Called on the window thread. Enumerating the current set also handles hotplug without
    // notification observers or global input monitoring. GCMouse is available on macOS 11+.
    pub(super) fn refresh(mice: &mut Vec<Mouse>, counts: &Arc<Mutex<Counts>>) {
        unsafe {
            let Some(class) = AnyClass::get(c"GCMouse") else { return };
            let array: Retained<AnyObject> = msg_send![class, mice];
            let count: usize = msg_send![&*array, count];
            mice.retain(|mouse| {
                (0..count).any(|i| {
                    let object: *mut AnyObject = msg_send![&*array, objectAtIndex: i];
                    object == Retained::as_ptr(&mouse.object).cast_mut()
                })
            });
            for i in 0..count {
                let object: Retained<AnyObject> = msg_send![&*array, objectAtIndex: i];
                if mice.iter().any(|m| Retained::as_ptr(&m.object) == Retained::as_ptr(&object)) { continue; }
                // Apple's GCMouse.h declares this profile nullable; keep AppKit usable without it.
                let input: Option<Retained<AnyObject>> = msg_send![&*object, mouseInput];
                let Some(input) = input else { continue };
                let shared = counts.clone();
                let handler = RcBlock::new(move |_input: *mut AnyObject, dx: f32, dy: f32| {
                    // GameController uses positive-up Y; the frontend uses positive-down.
                    shared.lock().add(dx, -dy);
                });
                let _: () = msg_send![&*input, setMouseMovedHandler: &*handler];
                mice.push(Mouse { object, input });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_sum_without_acceleration_and_clear_at_capture_boundaries() {
        let mut c = Counts { active: true, ..Default::default() };
        let now = Instant::now();
        // A connected-but-silent native source must not swallow initial trackpad motion.
        c.winit(99.0, 101.0);
        assert_eq!(c.take(now), ((99.0, 101.0), false));
        c.add(2.0, -3.0);
        c.add(200.0, -300.0);
        c.winit(99.0, 99.0);
        assert_eq!(c.take(now), ((202.0, -303.0), true));
        // A delayed AppKit duplicate on the next frame is discarded, not replayed.
        c.winit(202.0, -303.0);
        assert_eq!(c.take(now + Duration::from_millis(16)), ((0.0, 0.0), true));
        c.winit(7.0, 11.0);
        assert_eq!(c.take(now + NATIVE_GRACE), ((7.0, 11.0), false));
        c.add(4.0, 5.0);
        c.winit(99.0, 99.0);
        assert_eq!(c.take(now + NATIVE_GRACE), ((4.0, 5.0), true));
        c.add(6.0, 7.0);
        c.winit(8.0, 9.0);
        c.set_active(false);
        assert_eq!(c.take(now + NATIVE_GRACE), ((0.0, 0.0), false));
        c.add(8.0, 9.0);
        c.winit(10.0, 11.0);
        assert_eq!(c.take(now + NATIVE_GRACE), ((0.0, 0.0), false));
        c.set_active(true);
        c.winit(7.0, 11.0);
        assert_eq!(c.take(now + NATIVE_GRACE), ((7.0, 11.0), false));
    }
}
