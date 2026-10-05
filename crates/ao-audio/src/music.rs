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
    /// Layer whose pause track the pause clock follows (`PauseTracker` this+0xc, set by `FUN_10002712`).
    pause_layer: Option<usize>,
    /// Sample the player resumes with after a pause span (`Pause()` returns the pending transition's target, `Play(saved, false)`).
    resume: Option<usize>,
    /// Stream voices this player opened (`SIMPlayer_t+0x30..0x38`, at most [`crate::mixer::MAX_STREAMS`] fit).
    streams: Vec<u64>,
    /// Anti-repeat transition weights (`CTransition+0x34`).
    weights: HashMap<(usize, usize), u32>,
    rng: Rng,
    /// Name of the file that is playing (diagnostics).
    pub now_playing: Option<String>,
}

impl MusicPlayer {
    pub(crate) fn new(sh: Arc<Shared>, proj: Arc<Project>, seed: u64) -> Self {
        let dir = sh.root.join("music/env");
        MusicPlayer { sh, proj, dir, volume: 1.0, layer: None, state: State::Idle, played: 0.0, node: 0, pause_layer: None, resume: None, streams: Vec::new(), weights: HashMap::new(), rng: Rng(seed | 1), now_playing: None }
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
    /// debug toggle, so the 2000 ms synthetic cross-fade of forced transitions never applies); the transition is
    /// re-evaluated once now (`SignalEvent` state 2 -> 1) and again whenever a new sample starts, never per frame. A silent
    /// player (idle, pause track silence, closing sample) starts the new layer's entry sample immediately, and so does
    /// a previously stopped player (`PlayMusic`: `Play(NULL, true)` after the signal, which `Stop(true)`s every stream).
    /// `None` (`Stop(false, true)`) lets the current sample play to its end without a fade. The pause clock is *not*
    /// touched by a request: it restarts when a stream of another layer starts (`FUN_100096b6`) or on `Play(.., true)`
    /// (`FUN_1000289f`).
    pub fn signal(&mut self, layer: Option<usize>) {
        if layer == self.layer {
            return;
        }
        let was_stopped = self.layer.is_none();
        self.layer = layer;
        match (std::mem::replace(&mut self.state, State::Idle), layer) {
            (State::Playing(mut p), Some(_)) if !was_stopped => {
                p.next = self.pick_next(p.sample, p.t);
                self.state = State::Playing(p);
            }
            (State::Playing(mut p), None) => {
                p.next = None;
                self.state = State::Playing(p);
            }
            (_, Some(l)) => {
                self.stop_streams();
                self.resume = None;
                self.start_entry(l, 0.0, true);
            }
            (_, None) => self.now_playing = None,
        }
    }

    /// Opens a stream voice and remembers it (id 0 when all three Miles stream handles are in use, `AIL_open_stream` = 0).
    fn open_stream(&mut self, path: &Path, gain: f32, fade_in: f32) -> u64 {
        let id = self.sh.play_stream(path, gain, fade_in);
        if id != 0 {
            let m = self.sh.mixer();
            self.streams.retain(|v| m.is_playing(*v));
            self.streams.push(id);
        }
        id
    }

    /// `SIMPlayer_t::Stop(true, ..)` + `FUN_10009ffb`: every stream handle is closed at once.
    fn stop_streams(&mut self) {
        let mut m = self.sh.mixer();
        for id in self.streams.drain(..) {
            m.stop(id);
        }
    }

    /// `FUN_10002712` + `FUN_1000289f`: the pause clock follows `layer`'s pause track from its first node.
    fn reset_clock(&mut self, layer: usize) {
        self.pause_layer = Some(layer);
        self.played = 0.0;
        self.node = 0;
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

    /// Starts sample `s`; `reset` restarts the pause clock on the sample's layer.
    fn start(&mut self, s: usize, fade_in: f32, reset: bool) -> bool {
        let Some(path) = self.sample_path(s) else { return false };
        let voice = self.open_stream(&path, self.gain(s), fade_in);
        if voice == 0 {
            return false;
        }
        if reset {
            self.reset_clock(self.proj.samples[s].layer);
        }
        self.now_playing = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let next = self.pick_next(s, 0.0);
        self.state = State::Playing(Playing { sample: s, voice, t: 0.0, next });
        true
    }

    /// `CProject::GetEntrySample`: uniform among the entry samples (any sample when none is flagged).
    fn start_entry(&mut self, layer: usize, fade_in: f32, reset: bool) {
        let all = &self.proj.layers[layer].samples;
        let entries: Vec<usize> = all.iter().copied().filter(|&s| self.proj.samples[s].entry).collect();
        let mut pool = if entries.is_empty() { all.clone() } else { entries };
        while !pool.is_empty() {
            let i = self.rng.next() as usize % pool.len();
            let s = pool.swap_remove(i);
            if self.start(s, fade_in, reset) {
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
        // `FUN_10008c7e`: `while (w < r) { r -= w; next }` => a candidate is chosen while `r <= w` (the last one gets one slot less)
        for (i, w) in w.iter().enumerate() {
            if r <= *w {
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
            // nothing started yet (a missing file at signal time): retry (`FUN_1000a552`)
            State::Idle => self.start_entry(layer, 0.0, true),
            State::Paused { left } if left - dt <= 0.0 => {
                // `Play(saved, false)`: the pending transition's target, else the requested layer's entry; the clock keeps running
                let fade = self.node_fade_in();
                match self.resume.take() {
                    Some(s) if self.start(s, fade, false) => {}
                    _ => self.start_entry(layer, fade, false),
                }
            }
            State::Paused { left } => self.state = State::Paused { left: left - dt },
            State::Ending { left, pause, .. } if left - dt <= 0.0 => self.state = State::Paused { left: pause },
            State::Ending { voice, left, pause } => self.state = State::Ending { voice, left: left - dt, pause },
            State::Playing(p) => self.tick_playing(p, dt),
        }
    }

    fn tick_playing(&mut self, mut p: Playing, dt: f32) {
        p.t += dt;
        self.played += dt;
        // no per-frame transition search: `FUN_10008d2e` ran at the sample start and at the request, and its result stands
        // (a "none" is cached until the sample changes)
        let sample = &self.proj.samples[p.sample];
        let end = sample.end_ms as f32 / 1000.0;
        let Some((pause_secs, fade)) = self.pause_due() else {
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
            let (from, to) = (self.proj.samples[p.sample].layer, self.proj.samples[next.to].layer);
            // the new stream opens before the old one is released: a failed `AIL_open_stream` (3 handles busy) keeps the
            // current sample and searches again (`FUN_1000a143` -> state 1)
            let old = p.voice;
            if self.start(next.to, fade, from != to) {
                self.sh.mixer().fade(old, 0.0, fade, true);
            } else {
                p.next = self.pick_next(p.sample, p.t);
                self.state = State::Playing(p);
            }
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
        self.advance_node();
        self.resume = p.next.as_ref().map(|n| n.to);
        let mut m = self.sh.mixer();
        match closing {
            Some((t, path)) => {
                m.fade(p.voice, 0.0, t.ftime_ms as f32 / 1000.0, true);
                drop(m);
                let ending = self.open_stream(&path, self.gain(t.to), t.ftime_ms as f32 / 1000.0);
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
    fn pause_due(&self) -> Option<(f32, f32)> {
        let pt = &self.proj.pauses[self.proj.layers[self.pause_layer?].pause?];
        let n = pt.nodes.get(self.node)?;
        let beat = 60.0 / pt.bpm.max(1.0);
        (self.played >= n.play_beats as f32 * beat).then(|| (n.pause_beats as f32 * beat, if n.ptype == 0 { 0.0 } else { n.fade_out_ms as f32 / 1000.0 }))
    }

    /// Next pause track node (`loopto` after the last).
    fn advance_node(&mut self) {
        let Some(p) = self.pause_layer.and_then(|l| self.proj.layers[l].pause) else { return };
        let pt = &self.proj.pauses[p];
        self.node += 1;
        if self.node >= pt.nodes.len() {
            self.node = pt.loopto.min(pt.nodes.len() - 1);
        }
        self.played = 0.0;
    }

    fn node_fade_in(&self) -> f32 {
        self.pause_layer.and_then(|l| self.proj.layers[l].pause).and_then(|p| self.proj.pauses[p].nodes.get(self.node)).map_or(0.0, |n| n.fade_in_ms as f32 / 1000.0)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::{Source, VoiceDesc};
    use crate::sws::{Layer, PauseNode, PauseTrack, Sample};

    fn tr(to: usize) -> Transition {
        Transition { to, pri: 0, fot_ms: 4000, fit_ms: 0, ftime_ms: 0 }
    }

    fn sample(layer: usize, name: &str, entry: bool, trans: Vec<Transition>) -> Sample {
        Sample { layer, name: name.into(), end_ms: 4000, entry, vol: 1.0, trans, ptrans: Vec::new() }
    }

    /// Layers `A` (a1, a2; pause track: 30 s of music, 30 s of silence) and `B` (b1). No authored transition A -> B.
    fn player(tag: &str) -> (MusicPlayer, Arc<Shared>) {
        let root = std::env::temp_dir().join(format!("ao-music-{tag}-{}", std::process::id()));
        let wav: Vec<u8> = {
            let n = 22050u32;
            let mut v = b"RIFF".to_vec();
            v.extend_from_slice(&(36 + n * 2).to_le_bytes());
            v.extend_from_slice(b"WAVEfmt ");
            v.extend_from_slice(&16u32.to_le_bytes());
            for x in [1u16, 1] {
                v.extend_from_slice(&x.to_le_bytes());
            }
            v.extend_from_slice(&44100u32.to_le_bytes());
            v.extend_from_slice(&88200u32.to_le_bytes());
            v.extend_from_slice(&2u16.to_le_bytes());
            v.extend_from_slice(&16u16.to_le_bytes());
            v.extend_from_slice(b"data");
            v.extend_from_slice(&(n * 2).to_le_bytes());
            v.resize(v.len() + n as usize * 2, 0);
            v
        };
        for f in ["A/a1", "A/a2", "B/b1"] {
            let p = root.join(format!("music/env/{f}.wav"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, &wav).unwrap();
        }
        let proj = Project {
            layers: vec![Layer { name: "A".into(), vol: 1.0, pause: Some(0), samples: vec![0, 1] }, Layer { name: "B".into(), vol: 1.0, pause: None, samples: vec![2] }],
            samples: vec![sample(0, "a1", true, vec![tr(1), tr(0)]), sample(0, "a2", false, vec![tr(0), tr(1)]), sample(1, "b1", true, vec![])],
            pauses: vec![PauseTrack { bpm: 60.0, loopto: 0, nodes: vec![PauseNode { play_beats: 30, pause_beats: 30, ptype: 1, fade_in_ms: 0, fade_out_ms: 1000 }] }],
        };
        let sh = Shared::new(root, 44100);
        (MusicPlayer::new(sh.clone(), Arc::new(proj), 7), sh)
    }

    /// `secs` of 10 ms ticks, draining the mixer like the audio callback does.
    fn run(mp: &mut MusicPlayer, sh: &Shared, secs: f32) {
        let mut buf = vec![0f32; 441 * 2];
        for _ in 0..(secs * 100.0).round() as usize {
            mp.tick(0.01);
            sh.mixer().render(&mut buf);
        }
    }

    /// `FUN_10008d2e` is cached per sample/request: a pending request is not searched again every frame (which bumped the
    /// anti-repeat weights and probed the disk), and the pause clock is not restarted by a request.
    #[test]
    fn pending_request_is_evaluated_once_and_keeps_the_pause_clock() {
        let (mut mp, sh) = player("once");
        mp.signal(Some(0));
        run(&mut mp, &sh, 0.5);
        mp.signal(Some(1)); // no A -> B transition: the A chain goes on
        let w = mp.weights.clone();
        run(&mut mp, &sh, 3.0);
        assert_eq!(mp.weights, w, "weights only change when a sample starts, not per frame");
        assert!((mp.played - 3.5).abs() < 0.05, "the request did not restart the pause clock: {}", mp.played);
        assert!(mp.now_playing.as_deref().is_some_and(|n| n.starts_with('a')));
        let _ = std::fs::remove_dir_all(&sh.root);
    }

    /// Request while the pause span is still playing: after the silence the player resumes with the pending transition's
    /// target (`Pause()` returns it, `Play(saved, false)`); a request that arrives *during* the silence starts at once.
    #[test]
    fn pause_resumes_pending_sample_but_silence_starts_new_layer() {
        let (mut mp, sh) = player("pause");
        mp.signal(Some(0));
        run(&mut mp, &sh, 1.0);
        mp.signal(Some(1));
        run(&mut mp, &sh, 35.0); // the 30 s A span ends at the next transition point of the sample (<= 4 s later)
        assert!(matches!(mp.state, State::Paused { .. }), "silent after the play span");
        run(&mut mp, &sh, 30.0); // 30 s of silence: resumed
        assert!(mp.now_playing.as_deref().is_some_and(|n| n.starts_with('a')), "resumes the pending A sample: {:?}", mp.now_playing);
        // the next silence: a request starts the new layer's entry sample at once
        run(&mut mp, &sh, 36.0);
        assert!(matches!(mp.state, State::Paused { .. }));
        mp.signal(Some(0));
        assert!(matches!(mp.state, State::Playing(_)));
        assert_eq!(mp.now_playing.as_deref(), Some("a1.wav"));
        let _ = std::fs::remove_dir_all(&sh.root);
    }

    /// Only three stream handles exist: when the next sample's stream cannot open, the current one keeps playing
    /// (`FUN_1000a143` returns false, state 1) instead of being faded into silence.
    #[test]
    fn failed_stream_open_keeps_the_current_sample() {
        let (mut mp, sh) = player("cap");
        mp.signal(Some(0));
        let first = mp.now_playing.clone().unwrap();
        let mut hold = Vec::new();
        for _ in 0..2 {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(1);
            let id = sh.mixer().play(VoiceDesc { priority: None, ..VoiceDesc::new(Source::Stream { rx, rate: 44100, channels: 2 }) });
            assert_ne!(id, 0);
            hold.push(tx);
        }
        for _ in 0..397 {
            mp.tick(0.01); // no rendering: the first stream keeps its handle; the transition is due at 3.95 s
        }
        assert_eq!(mp.now_playing.as_deref(), Some(first.as_str()));
        let State::Playing(p) = &mp.state else { panic!("still playing the first sample") };
        assert!(sh.mixer().is_playing(p.voice), "old stream not faded out");
        drop(hold);
        let _ = std::fs::remove_dir_all(&sh.root);
    }
}
