//! Opt-in CPU-wall frame work, excluding event-loop pacing and capture/readback.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) struct FrameDiagnostics {
    start: Option<Instant>,
    last: Option<Instant>,
    seconds: [f64; 9],
}

impl FrameDiagnostics {
    pub(super) fn new(enabled: bool) -> Self {
        let start = enabled.then(Instant::now);
        Self { start, last: start, seconds: [0.0; 9] }
    }

    pub(super) fn mark(&mut self, stage: usize) {
        if let Some(last) = self.last.as_mut() {
            let now = Instant::now();
            self.seconds[stage] += (now - *last).as_secs_f64();
            *last = now;
        }
    }
    pub(super) fn finish(mut self, enabled: bool, path: &str, costs: &[crate::EffectGpuCost]) -> [f64; 9] {
        if let Some(start) = self.start {
            self.seconds[0] = start.elapsed().as_secs_f64();
            if enabled && is_slow(Duration::from_secs_f64(self.seconds[0])) {
                eprintln!("{}", format_slow_frame(SystemTime::now(), path, &self.seconds, costs));
            }
        }
        self.seconds.map(|seconds| seconds * 1000.0)
    }
}

fn is_slow(elapsed: Duration) -> bool { elapsed > Duration::from_millis(50) }

fn format_slow_frame(time: SystemTime, path: &str, seconds: &[f64; 9], costs: &[crate::EffectGpuCost]) -> String {
    use std::fmt::Write;
    let wall = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let mut out = format!("slow frame unix={}.{:03} path={path} CPU_wall_ms", wall.as_secs(), wall.subsec_millis());
    for (name, seconds) in ["total", "update", "frontend", "upload", "render", "gui", "gpu_wait", "present", "acquire"].into_iter().zip(seconds) {
        let _ = write!(out, " {name}={:.3}", seconds * 1000.0);
    }
    out.push_str("; GPU_rolling_context_ms (prior completed samples, not this frame):");
    if costs.is_empty() { out.push_str(" unavailable"); }
    for cost in costs { let _ = write!(out, " {}={:.3}[n={}]", cost.label, cost.average_ms, cost.samples); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_threshold_and_stage_context_are_explicit() {
        assert!(!is_slow(Duration::from_millis(50)));
        assert!(is_slow(Duration::from_micros(50_001)));
        let line = format_slow_frame(UNIX_EPOCH + Duration::from_millis(123_456), "windowed", &[0.051; 9], &[]);
        assert!(line.contains("unix=123.456 path=windowed CPU_wall_ms"));
        for stage in ["total", "update", "frontend", "upload", "render", "gui", "gpu_wait", "present", "acquire"] {
            assert!(line.contains(&format!(" {stage}=51.000")));
        }
        assert!(line.contains("prior completed samples, not this frame"));
        assert!(line.ends_with("unavailable"));
        let costs = [crate::EffectGpuCost { label: "GUI", average_ms: 0.125, samples: 7 }];
        let line = format_slow_frame(UNIX_EPOCH, "offscreen", &[0.0; 9], &costs);
        assert!(line.ends_with(" GUI=0.125[n=7]"));
        assert_eq!(FrameDiagnostics::new(false).finish(true, "idle", &[]), [0.0; 9]);
    }
}
