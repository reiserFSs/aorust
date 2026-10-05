//! Interactive music player = the client's `SIMPlayer` (`CProject` layers, see docs/formats.md `## audio`):
//! entry sample, weighted random transitions with a cross-fade at `fot`/`ftime`, pause tracks (play N beats, silence
//! M beats, per layer). Driven by [`MusicPlayer::tick`] (deterministic: time only advances by the `dt` given).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::Shared;
use crate::sws::{Project, Transition};

/// Fade of a layer change (`SandyInterface_t::PlayMusic` -> `SIMPlayer::SignalEvent(layer, force, 2000)`).
const LAYER_FADE: f32 = 2.0;
/// The client starts the next stream `AIL_digital_latency` before `fot`; we use a fixed decoder start-up allowance.
const LOOKAHEAD: f32 = 0.05;

/// xorshift64*
pub(crate) struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    /// uniform in [0, 1)
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }
}

struct Playing {
    sample: usize,
    voice: u64,
    t: f32,
    next: Option<Transition>,
}

enum State {
    Idle,
    Playing(Playing),
    /// Silence between two play spans of a pause track.
    Paused { left: f32 },
}

pub struct MusicPlayer {
    sh: Arc<Shared>,
    proj: Arc<Project>,
    dir: PathBuf,
    /// `Total_Music` (master x music volume x mutes).
    pub volume: f32,
    layer: Option<usize>,
    state: State,
    /// Seconds played in the current pause track node, and the node index.
    played: f32,
    node: usize,
    /// Anti-repeat transition weights (`CTransition+0x34`).
    weights: HashMap<(usize, usize), u32>,
    rng: Rng,
    /// Name of the file that is playing (diagnostics).
    pub now_playing: Option<String>,
}

impl MusicPlayer {
    pub(crate) fn new(sh: Arc<Shared>, proj: Arc<Project>, seed: u64) -> Self {
        let dir = sh.root.join("music/env");
        MusicPlayer { sh, proj, dir, volume: 1.0, layer: None, state: State::Idle, played: 0.0, node: 0, weights: HashMap::new(), rng: Rng(seed | 1), now_playing: None }
    }

    pub fn layer(&self) -> Option<usize> {
        self.layer
    }

    pub fn project(&self) -> &Project {
        &self.proj
    }

    /// `SandyInterface_t::PlayMusic`: switch to `layer` (`None` = silence); the same layer is a no-op.
    pub fn signal(&mut self, layer: Option<usize>) {
        if layer == self.layer {
            return;
        }
        self.stop_current(LAYER_FADE);
        self.layer = layer;
        self.played = 0.0;
        self.node = 0;
        self.state = State::Idle;
        if let Some(l) = layer {
            self.start_entry(l, LAYER_FADE);
        }
    }

    fn stop_current(&mut self, fade: f32) {
        if let State::Playing(p) = std::mem::replace(&mut self.state, State::Idle) {
            self.sh.mixer().fade(p.voice, 0.0, fade, true);
        }
        self.now_playing = None;
    }

    /// Path of a sample: `<music/env>/<layer name with \ -> />/<name>.{wav,mp3,ogg}`, matched case-insensitively.
    fn sample_path(&self, s: usize) -> Option<PathBuf> {
        let sample = self.proj.samples.get(s)?;
        let layer = self.proj.layers.get(sample.layer)?;
        let dir = find_ci(&self.dir, &layer.name.replace('\\', "/"))?;
        ["wav", "mp3", "ogg"].iter().find_map(|e| find_ci(&dir, &format!("{}.{e}", sample.name)))
    }

    fn gain(&self, s: usize) -> f32 {
        let sample = &self.proj.samples[s];
        self.volume * self.proj.layers[sample.layer].vol * sample.vol
    }

    fn start(&mut self, s: usize, fade_in: f32) -> bool {
        let Some(path) = self.sample_path(s) else { return false };
        let voice = self.sh.play_stream(&path, self.gain(s), fade_in);
        if voice == 0 {
            return false;
        }
        self.now_playing = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let next = self.pick_next(s);
        self.state = State::Playing(Playing { sample: s, voice, t: 0.0, next });
        true
    }

    /// `CProject::GetEntrySample`: uniform among the entry samples (any sample when none is flagged).
    fn start_entry(&mut self, layer: usize, fade_in: f32) {
        let all = &self.proj.layers[layer].samples;
        let entries: Vec<usize> = all.iter().copied().filter(|&s| self.proj.samples[s].entry).collect();
        let mut pool = if entries.is_empty() { all.clone() } else { entries };
        while !pool.is_empty() {
            let i = self.rng.next() as usize % pool.len();
            let s = pool.swap_remove(i);
            if self.start(s, fade_in) {
                return;
            }
        }
    }

    /// `FUN_10008c7e`: weighted random over the sample's transitions to samples that exist on disk; the chosen one
    /// keeps its weight, every other candidate gains 1 (reset to 1 when one passes 999).
    fn pick_next(&mut self, s: usize) -> Option<Transition> {
        let cands: Vec<Transition> = self.proj.samples[s].trans.iter().filter(|t| self.sample_path(t.to).is_some()).cloned().collect();
        if cands.is_empty() {
            return None;
        }
        // [INFERENCE] a pri of 0 (all sampled transitions) still has to be selectable: initial weight max(pri, 1)
        let w: Vec<u32> = cands.iter().map(|t| *self.weights.entry((s, t.to)).or_insert(t.pri.max(1) as u32)).collect();
        let total: u32 = w.iter().sum();
        let mut r = self.rng.next() % total;
        let mut pick = 0;
        for (i, w) in w.iter().enumerate() {
            if r < *w {
                pick = i;
                break;
            }
            r -= w;
        }
        let mut reset = false;
        for (i, t) in cands.iter().enumerate() {
            if i != pick {
                let e = self.weights.get_mut(&(s, t.to)).unwrap();
                *e += 1;
                reset |= *e > 999;
            }
        }
        if reset {
            self.weights.retain(|_, v| {
                *v = 1;
                true
            });
        }
        Some(cands[pick].clone())
    }

    /// Advances the music clock by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let Some(layer) = self.layer else { return };
        match std::mem::replace(&mut self.state, State::Idle) {
            // nothing started yet (a missing file at signal time): retry
            State::Idle => self.start_entry(layer, 0.0),
            State::Paused { left } if left - dt <= 0.0 => {
                let fade = self.node_fade_in(layer);
                self.start_entry(layer, fade);
            }
            State::Paused { left } => self.state = State::Paused { left: left - dt },
            State::Playing(p) => self.tick_playing(layer, p, dt),
        }
    }

    fn tick_playing(&mut self, layer: usize, mut p: Playing, dt: f32) {
        p.t += dt;
        self.played += dt;
        let Some(next) = p.next.clone() else {
            // no successor: the sample plays out, then the layer restarts from an entry sample
            let end = self.proj.samples[p.sample].end_ms as f32 / 1000.0;
            if !(p.t > end + 0.5 && !self.sh.mixer().is_playing(p.voice)) {
                self.state = State::Playing(p);
            }
            return;
        };
        if p.t + LOOKAHEAD < next.fot_ms as f32 / 1000.0 {
            self.state = State::Playing(p);
            return;
        }
        if let Some((secs, fade)) = self.due_pause(layer) {
            // pause track: fade the span out (ptype 0 = hard cut) and stay silent for the pause length
            self.sh.mixer().fade(p.voice, 0.0, fade, true);
            self.state = State::Paused { left: secs };
            self.now_playing = None;
            return;
        }
        let fade = next.ftime_ms as f32 / 1000.0;
        self.sh.mixer().fade(p.voice, 0.0, fade, true);
        self.start(next.to, fade); // on failure the state stays Idle and the layer restarts
    }

    /// `Some((pause seconds, fade-out seconds))` when the current play span of the layer's pause track is over;
    /// advances to the next node.
    fn due_pause(&mut self, layer: usize) -> Option<(f32, f32)> {
        let pt = &self.proj.pauses[self.proj.layers[layer].pause?];
        let n = pt.nodes.get(self.node)?;
        let beat = 60.0 / pt.bpm.max(1.0);
        if self.played < n.play_beats as f32 * beat {
            return None;
        }
        let fade = if n.ptype == 0 { 0.0 } else { n.fade_out_ms as f32 / 1000.0 };
        let secs = n.pause_beats as f32 * beat;
        self.node += 1;
        if self.node >= pt.nodes.len() {
            self.node = pt.loopto.min(pt.nodes.len() - 1);
        }
        self.played = 0.0;
        Some((secs, fade))
    }

    fn node_fade_in(&self, layer: usize) -> f32 {
        self.proj.layers[layer].pause.and_then(|p| self.proj.pauses[p].nodes.get(self.node)).map_or(0.0, |n| n.fade_in_ms as f32 / 1000.0)
    }
}

/// `<dir>/<rel>` where every path component matches case-insensitively (the client ran on Windows).
pub(crate) fn find_ci(dir: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = dir.to_owned();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        let direct = cur.join(part);
        if direct.exists() {
            cur = direct;
            continue;
        }
        let lower = part.to_ascii_lowercase();
        cur = std::fs::read_dir(&cur).ok()?.flatten().find(|e| e.file_name().to_string_lossy().to_ascii_lowercase() == lower)?.path();
    }
    Some(cur)
}
