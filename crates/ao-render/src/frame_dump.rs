//! Opt-in real-window diagnostics, read once: `AOMAC_FRAME_DUMP=<abs dir>,<N>` writes N consecutive presented frames
//! (final swapchain colour, after GUI) as `frame_000000.png`..., starting `AOMAC_FRAME_DUMP_DELAY` seconds (default 0) after
//! Play reports the world playing; `AOMAC_FREEZE_CAMERA=1` freezes the camera once that happens. Both cost nothing when unset.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static WORLD_PLAYING: AtomicBool = AtomicBool::new(false);

/// Readiness hook: Play calls this every frame with whether the character is in the world and no loading screen is up.
pub fn set_world_playing(on: bool) { WORLD_PLAYING.store(on, Ordering::Relaxed); }

pub(super) fn world_playing() -> bool { WORLD_PLAYING.load(Ordering::Relaxed) }

pub(super) fn freeze_camera_requested() -> bool { std::env::var_os("AOMAC_FREEZE_CAMERA").is_some_and(|v| v != "0") }

pub(super) struct FrameDump {
    dir: PathBuf,
    total: u32,
    delay: Duration,
    written: u32,
    ready_since: Option<Instant>,
}

/// `<dir>,<N>`; the directory must be an explicit absolute path (e.g. under /tmp), N positive.
fn parse(spec: &str) -> Option<(PathBuf, u32)> {
    let (dir, n) = spec.rsplit_once(',')?;
    let n: u32 = n.trim().parse().ok().filter(|n| *n > 0)?;
    let dir = PathBuf::from(dir);
    dir.is_absolute().then_some((dir, n))
}

impl FrameDump {
    pub(super) fn from_env() -> Option<Self> {
        let spec = std::env::var("AOMAC_FRAME_DUMP").ok()?;
        let Some((dir, total)) = parse(&spec) else {
            eprintln!("AOMAC_FRAME_DUMP={spec:?} ignored: expected <absolute dir>,<positive frame count>");
            return None;
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("AOMAC_FRAME_DUMP: cannot create {}: {e}", dir.display());
            return None;
        }
        let delay = std::env::var("AOMAC_FRAME_DUMP_DELAY").ok().and_then(|v| v.parse::<f32>().ok())
            .filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(0.0);
        eprintln!("AOMAC_FRAME_DUMP: {total} frames to {} after {delay}s of playing", dir.display());
        Some(Self { dir, total, delay: Duration::from_secs_f32(delay), written: 0, ready_since: None })
    }

    /// Path for this presented frame once capturing is due (counts it), else `None`.
    pub(super) fn next_path(&mut self, playing: bool, now: Instant) -> Option<PathBuf> {
        if self.written >= self.total { return None; }
        if !playing && self.written == 0 { self.ready_since = None; return None; }
        let since = *self.ready_since.get_or_insert(now);
        if now.saturating_duration_since(since) < self.delay { return None; }
        let path = self.dir.join(format!("frame_{:06}.png", self.written));
        self.written += 1;
        if self.written == self.total { eprintln!("AOMAC_FRAME_DUMP: wrote {} frames to {}", self.total, self.dir.display()); }
        Some(path)
    }
}

/// `(average, p95, worst)` of per-frame milliseconds; sorts `ms` in place. Empty = zeros.
pub(super) fn frame_stats(ms: &mut [f32]) -> (f32, f32, f32) {
    if ms.is_empty() { return (0.0, 0.0, 0.0); }
    ms.sort_by(f32::total_cmp);
    let avg = ms.iter().sum::<f32>() / ms.len() as f32;
    (avg, ms[((ms.len() * 95).div_ceil(100)).saturating_sub(1)], ms[ms.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_needs_absolute_dir_and_positive_count() {
        assert_eq!(parse("/tmp/a,30"), Some((PathBuf::from("/tmp/a"), 30)));
        assert_eq!(parse("/tmp/a,b,5"), Some((PathBuf::from("/tmp/a,b"), 5)));
        assert_eq!(parse("rel,3"), None);
        assert_eq!(parse("/tmp/a,0"), None);
        assert_eq!(parse("/tmp/a"), None);
    }

    #[test]
    fn dump_waits_for_delay_then_numbers_consecutive_frames() {
        let t0 = Instant::now();
        let mut d = FrameDump { dir: "/tmp/x".into(), total: 2, delay: Duration::from_secs(2), written: 0, ready_since: None };
        assert!(d.next_path(false, t0).is_none());
        assert!(d.next_path(true, t0).is_none());
        assert!(d.next_path(true, t0 + Duration::from_secs(1)).is_none());
        assert!(d.next_path(false, t0 + Duration::from_secs(3)).is_none(), "leaving play before start restarts the delay");
        let t1 = t0 + Duration::from_secs(4);
        assert!(d.next_path(true, t1).is_none());
        assert_eq!(d.next_path(true, t1 + Duration::from_secs(2)), Some(PathBuf::from("/tmp/x/frame_000000.png")));
        assert_eq!(d.next_path(true, t1 + Duration::from_secs(3)), Some(PathBuf::from("/tmp/x/frame_000001.png")));
        assert!(d.next_path(true, t1 + Duration::from_secs(4)).is_none());
    }

    #[test]
    fn stats_report_average_p95_and_worst() {
        let mut v: Vec<f32> = (1..=100).map(|n| n as f32).collect();
        assert_eq!(frame_stats(&mut v), (50.5, 95.0, 100.0));
        assert_eq!(frame_stats(&mut []), (0.0, 0.0, 0.0));
    }
}
