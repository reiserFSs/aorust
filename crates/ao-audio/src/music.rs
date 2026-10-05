//! Interactive music player = the client's `SIMPlayer` (`CProject` layers, see docs/formats.md `## audio`):
//! entry sample, weighted random transitions with a cross-fade at `fot`/`ftime`, pause tracks (play N beats, silence
//! M beats, per layer). Driven by [`MusicPlayer::tick`] (deterministic: time only advances by the `dt` given).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::Shared;
use crate::sws::{Project, Transition};

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
    /// Closing sample of a pause transition (`ptrans`) fading the layer out; silence follows.
    Ending { voice: u64, left: f32, pause: f32 },
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

    /// `SandyInterface_t::PlayMusic` -> `SIMPlayer::SignalEvent(layer, force = 0, 2000)` (SIM @0x1000a599): the request is
    /// only stored. The playing layer keeps going until its current sample has an authored transition into the new
    /// layer ahead of the playhead (the transition's own `fot`/`ftime` cross-fade is used; `force` is 0 outside the
    /// debug toggle, so the 2000 ms synthetic cross-fade of forced transitions never applies). A silent player (idle,
    /// pause track silence, closing sample) starts the new layer's entry sample immediately. `None` (`Stop(false, true)`)
    /// lets the current sample play to its end without a fade. The pause clock restarts on every layer change
    /// (`FUN_1000289f`).
    pub fn signal(&mut self, layer: Option<usize>) {
        if layer == self.layer {
            return;
        }
        self.layer = layer;
        self.played = 0.0;
        self.node = 0;
        match (std::mem::replace(&mut self.state, State::Idle), layer) {
            (State::Playing(mut p), Some(_)) => {
                p.next = self.pick_next(p.sample, p.t);
                self.state = State::Playing(p);
            }
            (State::Playing(mut p), None) => {
                p.next = None;
                self.state = State::Playing(p);
            }
            (_, Some(l)) => self.start_entry(l, 0.0),
            (_, None) => self.now_playing = None,
        }
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
        let next = self.pick_next(s, 0.0);
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

    /// `FUN_10008d2e` + `FUN_10008c7e`: candidates are the transitions of `s` that start ahead of the playhead `t`
    /// (`fot - 10 ms > pos`) and whose sample exists. With a pending request for another layer the earliest
    /// transitions into it win; otherwise the ones that stay in the sample's own layer.
    fn pick_next(&mut self, s: usize, t: f32) -> Option<Transition> {
        let own = self.proj.samples[s].layer;
        let ahead: Vec<Transition> = self.proj.samples[s].trans.iter().filter(|x| x.fot_ms as f32 / 1000.0 - 0.01 > t && self.sample_path(x.to).is_some()).cloned().collect();
        let proj = &self.proj;
        let into = |l: usize| ahead.iter().filter(|x| proj.samples[x.to].layer == l).cloned().collect::<Vec<_>>();
        let mut cands = match self.layer {
            Some(tl) if tl != own => {
                let a = into(tl);
                let min = a.iter().map(|x| x.fot_ms).min();
                a.into_iter().filter(|x| Some(x.fot_ms) == min).collect()
            }
            _ => Vec::new(),
        };
        if cands.is_empty() {
            cands = into(own);
        }
        self.weighted(s, cands)
    }

    /// Weighted random among `cands`; the chosen one keeps its weight, every other candidate gains 1 (reset to 1 when one
    /// passes 999). [INFERENCE] a `pri` of 0 (all sampled transitions) starts at weight 1.
    fn weighted(&mut self, s: usize, cands: Vec<Transition>) -> Option<Transition> {
        if cands.is_empty() {
            return None;
        }
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
            self.weights.values_mut().for_each(|v| *v = 1);
        }
        Some(cands[pick].clone())
    }

    /// Advances the music clock by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let Some(layer) = self.layer else {
            // stopped: the current sample plays out, then the player is idle
            if let State::Playing(mut p) = std::mem::replace(&mut self.state, State::Idle) {
                p.t += dt;
                if p.t < self.proj.samples[p.sample].end_ms as f32 / 1000.0 {
                    self.state = State::Playing(p);
                } else {
                    self.now_playing = None;
                }
            }
            return;
        };
        match std::mem::replace(&mut self.state, State::Idle) {
            // nothing started yet (a missing file at signal time): retry
            State::Idle => self.start_entry(layer, 0.0),
            State::Paused { left } if left - dt <= 0.0 => {
                let fade = self.node_fade_in(layer);
                self.start_entry(layer, fade);
            }
            State::Paused { left } => self.state = State::Paused { left: left - dt },
            State::Ending { left, pause, .. } if left - dt <= 0.0 => self.state = State::Paused { left: pause },
            State::Ending { voice, left, pause } => self.state = State::Ending { voice, left: left - dt, pause },
            State::Playing(p) => self.tick_playing(layer, p, dt),
        }
    }

    fn tick_playing(&mut self, layer: usize, mut p: Playing, dt: f32) {
        p.t += dt;
        self.played += dt;
        // decision point (every update): a pending layer request needs an authored transition into it
        if self.proj.samples[p.sample].layer != layer && p.next.as_ref().is_none_or(|n| self.proj.samples[n.to].layer != layer) {
            if let Some(n) = self.pick_next(p.sample, p.t) {
                if self.proj.samples[n.to].layer == layer {
                    p.next = Some(n);
                }
            }
        }
        let sample = &self.proj.samples[p.sample];
        let end = sample.end_ms as f32 / 1000.0;
        let Some((pause_secs, fade)) = self.pause_due(layer) else {
            let Some(next) = p.next.clone() else {
                // no successor: the sample plays out, then the layer restarts from an entry sample
                if p.t <= end + 0.5 {
                    self.state = State::Playing(p);
                }
                return;
            };
            if p.t + LOOKAHEAD < next.fot_ms as f32 / 1000.0 {
                self.state = State::Playing(p);
                return;
            }
            let fade = next.ftime_ms as f32 / 1000.0;
            self.sh.mixer().fade(p.voice, 0.0, fade, true);
            self.start(next.to, fade); // on failure the state stays Idle and the layer restarts
            return;
        };
        // pause track: the play span is over. A sample with a pause transition (`ptrans`) crosses into its closing
        // sample at that transition's `fot`; others fade out at their normal transition point (ptype 0 = hard cut).
        let closing = sample.ptrans.iter().find_map(|t| self.sample_path(t.to).map(|path| (t.clone(), path)));
        let fot = match (&closing, &p.next) {
            (Some((t, _)), _) => t.fot_ms as f32 / 1000.0,
            (None, Some(n)) => n.fot_ms as f32 / 1000.0,
            (None, None) => end,
        };
        if p.t + LOOKAHEAD < fot {
            self.state = State::Playing(p);
            return;
        }
        self.advance_node(layer);
        let mut m = self.sh.mixer();
        match closing {
            Some((t, path)) => {
                m.fade(p.voice, 0.0, t.ftime_ms as f32 / 1000.0, true);
                drop(m);
                let ending = self.sh.play_stream(&path, self.gain(t.to), t.ftime_ms as f32 / 1000.0);
                self.now_playing = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.state = State::Ending { voice: ending, left: self.proj.samples[t.to].end_ms as f32 / 1000.0, pause: pause_secs };
            }
            None => {
                m.fade(p.voice, 0.0, fade, true);
                self.now_playing = None;
                self.state = State::Paused { left: pause_secs };
            }
        }
    }

    /// `Some((pause seconds, fade-out seconds))` once the current play span of the layer's pause track is over.
    fn pause_due(&self, layer: usize) -> Option<(f32, f32)> {
        let pt = &self.proj.pauses[self.proj.layers[layer].pause?];
        let n = pt.nodes.get(self.node)?;
        let beat = 60.0 / pt.bpm.max(1.0);
        (self.played >= n.play_beats as f32 * beat).then(|| (n.pause_beats as f32 * beat, if n.ptype == 0 { 0.0 } else { n.fade_out_ms as f32 / 1000.0 }))
    }

    /// Next pause track node (`loopto` after the last).
    fn advance_node(&mut self, layer: usize) {
        let Some(p) = self.proj.layers[layer].pause else { return };
        let pt = &self.proj.pauses[p];
        self.node += 1;
        if self.node >= pt.nodes.len() {
            self.node = pt.loopto.min(pt.nodes.len() - 1);
        }
        self.played = 0.0;
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
