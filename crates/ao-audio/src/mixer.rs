//! Device independent software mixer: sample/stream voices, per-voice volume, positional emitters, fades.
//! Faithful to the client's SandyInterface (docs/formats.md `## audio`): no panning (a mono source goes to both
//! channels at the voice level); distance attenuation is applied by the caller (`game::attenuation`).
//! [`Mixer::render`] fills an interleaved stereo buffer; the cpal callback and the offline tests both call it.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

use crate::decode::Pcm;

pub const MAX_VOICES: usize = 64;

/// Where a voice's samples come from.
pub enum Source {
    Sample(Arc<Pcm>),
    /// Decoded chunks (interleaved f32) produced by a worker thread; the voice ends when the sender is gone.
    Stream { rx: Receiver<Vec<f32>>, rate: u32, channels: u16 },
}

pub struct VoiceDesc {
    pub source: Source,
    pub gain: f32,
    pub looping: bool,
}

impl VoiceDesc {
    pub fn new(source: Source) -> Self {
        VoiceDesc { source, gain: 1.0, looping: false }
    }
}

struct Voice {
    id: u64,
    src: Source,
    cur: Vec<f32>,
    /// fractional read position in frames of `cur` (streams) or of the sample (samples)
    pos: f64,
    done: bool,
    gain: f32,
    looping: bool,
    fade: f32,
    fade_target: f32,
    fade_step: f32,
    stop_at_fade_end: bool,
    last: f32,
}

#[derive(Default)]
pub struct Stats {
    pub callbacks: std::sync::atomic::AtomicU64,
    pub frames: std::sync::atomic::AtomicU64,
    pub voices: std::sync::atomic::AtomicUsize,
    pub rms_bits: std::sync::atomic::AtomicU32,
    pub peak_bits: std::sync::atomic::AtomicU32,
}

pub struct Mixer {
    pub rate: u32,
    pub master: f32,
    voices: Vec<Voice>,
    next_id: u64,
    pub stats: Arc<Stats>,
}

impl Mixer {
    pub fn new(rate: u32) -> Self {
        Mixer { rate, master: 1.0, voices: Vec::new(), next_id: 1, stats: Arc::default() }
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn is_playing(&self, id: u64) -> bool {
        self.voices.iter().any(|v| v.id == id && !v.done)
    }

    /// Starts a voice; with `MAX_VOICES` busy the quietest non-looping voice is stolen (or the request dropped, id 0).
    pub fn play(&mut self, d: VoiceDesc) -> u64 {
        if self.voices.len() >= MAX_VOICES {
            let victim = self.voices.iter().enumerate().filter(|(_, v)| !v.looping).min_by(|a, b| (a.1.gain * a.1.fade).total_cmp(&(b.1.gain * b.1.fade))).map(|(i, _)| i);
            match victim {
                Some(i) => {
                    self.voices.swap_remove(i);
                }
                None => return 0,
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.voices.push(Voice { id, src: d.source, cur: Vec::new(), pos: 0.0, done: false, gain: d.gain, looping: d.looping, fade: 1.0, fade_target: 1.0, fade_step: 0.0, stop_at_fade_end: false, last: f32::NAN });
        id
    }

    pub fn set_gain(&mut self, id: u64, gain: f32) {
        if let Some(v) = self.voices.iter_mut().find(|v| v.id == id) {
            v.gain = gain;
        }
    }

    /// Starts the voice silent and ramps it to full over `secs`.
    pub fn fade_from_zero(&mut self, id: u64, secs: f32) {
        if let Some(v) = self.voices.iter_mut().find(|v| v.id == id) {
            v.fade = 0.0;
        }
        self.fade(id, 1.0, secs, false);
    }

    pub fn stop(&mut self, id: u64) {
        self.voices.retain(|v| v.id != id);
    }

    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    /// Ramps the voice's fade level to `to` over `secs`; with `stop` it ends there.
    pub fn fade(&mut self, id: u64, to: f32, secs: f32, stop: bool) {
        let rate = self.rate as f32;
        if let Some(v) = self.voices.iter_mut().find(|v| v.id == id) {
            v.fade_target = to;
            v.fade_step = (to - v.fade) / (secs.max(1e-3) * rate);
            v.stop_at_fade_end = stop;
        }
    }

    /// Mixes `out.len() / 2` stereo frames (overwrites `out`).
    pub fn render(&mut self, out: &mut [f32]) {
        use std::sync::atomic::Ordering::Relaxed;
        out.fill(0.0);
        let n = out.len() / 2;
        let ratio_out = self.rate as f64;
        for v in &mut self.voices {
            let g1 = v.gain;
            let g0 = if v.last.is_nan() { g1 } else { v.last };
            let (rate, ch) = match &v.src {
                Source::Sample(p) => (p.rate, p.channels as usize),
                Source::Stream { rate, channels, .. } => (*rate, *channels as usize),
            };
            let step = rate as f64 / ratio_out;
            for i in 0..n {
                if v.done {
                    break;
                }
                let t = i as f32 / n as f32;
                let g = (g0 + (g1 - g0) * t) * v.fade;
                if v.fade != v.fade_target {
                    v.fade += v.fade_step;
                    if (v.fade_step >= 0.0 && v.fade >= v.fade_target) || (v.fade_step < 0.0 && v.fade <= v.fade_target) {
                        v.fade = v.fade_target;
                        if v.stop_at_fade_end && v.fade == 0.0 {
                            v.done = true;
                        }
                    }
                }
                let (sl, sr) = match fetch(v, ch) {
                    Some(s) => s,
                    None => break,
                };
                out[2 * i] += sl * g;
                out[2 * i + 1] += sr * g;
                v.pos += step;
            }
            v.last = g1;
        }
        self.voices.retain(|v| !v.done);
        let (mut sq, mut peak) = (0f64, 0f32);
        for s in out.iter_mut() {
            *s = (*s * self.master).clamp(-1.0, 1.0);
            sq += (*s as f64) * (*s as f64);
            peak = peak.max(s.abs());
        }
        let rms = if out.is_empty() { 0.0 } else { (sq / out.len() as f64).sqrt() as f32 };
        self.stats.callbacks.fetch_add(1, Relaxed);
        self.stats.frames.fetch_add(n as u64, Relaxed);
        self.stats.voices.store(self.voices.len(), Relaxed);
        self.stats.rms_bits.store(rms.to_bits(), Relaxed);
        self.stats.peak_bits.fetch_max(peak.to_bits(), Relaxed); // non-negative floats order like their bits
    }
}

/// One linearly interpolated source frame at `v.pos`, folded to mono-for-pan (stereo sources keep their channels).
/// Returns `None` when the voice ran dry (`done` is set for finished voices; a starved stream yields silence).
fn fetch(v: &mut Voice, ch: usize) -> Option<(f32, f32)> {
    let ch = ch.max(1);
    if let Source::Sample(p) = &v.src {
        let frames = p.frames();
        if v.pos as usize >= frames {
            if v.looping && frames > 0 {
                v.pos -= frames as f64;
            } else {
                v.done = true;
                return None;
            }
        }
        let i = v.pos as usize;
        let f = (v.pos - i as f64) as f32;
        let j = if i + 1 < frames { i + 1 } else if v.looping { 0 } else { i };
        return Some(frame(&p.samples, ch, i, j, f));
    }
    // stream: refill `cur` when the read position passes its end
    loop {
        let frames = v.cur.len() / ch;
        let i = v.pos as usize;
        if i + 1 < frames {
            let f = (v.pos - i as f64) as f32;
            return Some(frame(&v.cur, ch, i, i + 1, f));
        }
        let Source::Stream { rx, .. } = &v.src else { unreachable!() };
        match rx.try_recv() {
            Ok(mut next) => {
                // keep the unread tail so interpolation across chunk borders is continuous
                let mut joined = if i < frames { v.cur[i * ch..].to_vec() } else { Vec::new() };
                v.pos -= i as f64;
                joined.append(&mut next);
                v.cur = joined;
            }
            Err(TryRecvError::Empty) => return Some((0.0, 0.0)),
            Err(TryRecvError::Disconnected) => {
                v.done = true;
                return None;
            }
        }
    }
}

fn frame(s: &[f32], ch: usize, i: usize, j: usize, f: f32) -> (f32, f32) {
    let g = |k: usize, c: usize| s[k * ch + c.min(ch - 1)];
    let lerp = |c: usize| g(i, c) + (g(j, c) - g(i, c)) * f;
    if ch == 1 {
        let m = lerp(0);
        (m, m)
    } else {
        (lerp(0), lerp(1))
    }
}
