//! The audio device + housekeeping: owns the mixer, loads sounds from `cd_image/sound`, streams music.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::combat::{char_sample, CharInfo, CombatSample};
use crate::decode::{self, Pcm};
use crate::game::{Library, PlayfieldAudio, Runtime};
use crate::mixer::{Mixer, Source, Stats, VoiceDesc};

/// State shared with the housekeeping thread.
pub(crate) struct Shared {
    pub mixer: Mutex<Mixer>,
    pub root: PathBuf,
    cache: Mutex<HashMap<PathBuf, Option<Arc<Pcm>>>>,
}

impl Shared {
    pub fn new(root: PathBuf, rate: u32) -> Arc<Shared> {
        Arc::new(Shared { mixer: Mutex::new(Mixer::new(rate)), root, cache: Mutex::default() })
    }

    pub fn mixer(&self) -> MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `<root>/<rel>` with `\` as separator; a missing extension tries `.wav` then `.ogg`.
    pub fn resolve(&self, rel: &str) -> Option<PathBuf> {
        let p = self.root.join(rel.replace('\\', "/"));
        if p.is_file() {
            return Some(p);
        }
        ["wav", "ogg"].iter().map(|e| PathBuf::from(format!("{}.{e}", p.display()))).find(|c| c.is_file())
    }

    /// Decoded sound, cached (failures too: a broken file is reported once).
    pub fn load(&self, path: &Path) -> Option<Arc<Pcm>> {
        if let Some(c) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
            return c.clone();
        }
        let pcm = match decode::decode_file(path) {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                eprintln!("audio: {e:#}");
                None
            }
        };
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_owned(), pcm.clone());
        pcm
    }

    /// Streams `path` through a decoder thread into a voice (id 0 on failure).
    pub fn play_stream(&self, path: &Path, gain: f32, fade_in: f32) -> u64 {
        let mut src = match decode::Source::open(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("audio: {e:#}");
                return 0;
            }
        };
        let (rate, channels) = (src.rate, src.channels.max(1));
        let (tx, rx) = sync_channel::<Vec<f32>>(8);
        let name = path.display().to_string();
        std::thread::spawn(move || loop {
            match src.next_chunk() {
                Ok(Some(c)) => {
                    if tx.send(c.to_vec()).is_err() {
                        return; // voice gone
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    eprintln!("audio: {name}: {e:#}");
                    return;
                }
            }
        });
        let mut m = self.mixer();
        let id = m.play(VoiceDesc { gain, priority: None, ..VoiceDesc::new(Source::Stream { rx, rate, channels }) });
        if id != 0 && fade_in > 0.0 {
            m.fade_from_zero(id, fade_in);
        }
        id
    }

    pub fn play_sample(&self, path: &Path, gain: f32, looping: bool, priority: u8) -> u64 {
        let Some(pcm) = self.load(path) else {
            if audio_log() { eprintln!("audio rejection reason=decode sample={}", path.display()); }
            return 0;
        };
        if pcm.frames() == 0 {
            if audio_log() { eprintln!("audio rejection reason=empty-sample sample={}", path.display()); }
            return 0;
        }
        let voice = self.mixer().play(VoiceDesc { gain, looping, priority: Some(priority), ..VoiceDesc::new(Source::Sample(pcm)) });
        if voice == 0 && audio_log() { eprintln!("audio rejection reason=voice-pool sample={} gain={gain} priority={priority}", path.display()); }
        voice
    }
}

fn audio_log() -> bool {
    static LOG: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("AOMAC_AUDIO_LOG").is_some());
    *LOG
}

fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

/// The client's sound prefs (`cd_image/gui/Default/LoginPrefs.xml` defaults: all 1.0 / on).
#[derive(Clone, Copy, Debug)]
pub struct Prefs {
    pub sound_on: bool,
    pub master: f32,
    pub fx_on: bool,
    pub fx: f32,
    pub music_on: bool,
    pub music: f32,
    /// `BattlemusicMode` 0..=3 (0 = no combat music; default 3, `MainPrefs.xml`).
    pub battlemusic_mode: i32,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { sound_on: true, master: 1.0, fx_on: true, fx: 1.0, music_on: true, music: 1.0, battlemusic_mode: 3 }
    }
}

/// Owns the output stream. Not `Send` (cpal streams are not on every platform); keep it on the main thread.
pub struct Audio {
    pub(crate) sh: Arc<Shared>,
    /// Game rules (music, ambience, emitters); `None` when the client's sound data is not there (offline tests).
    rt: Mutex<Option<Runtime>>,
    _stream: Option<cpal::Stream>,
    pub device: String,
}

impl Audio {
    /// Opens the default output device. `client_dir` is the AO client root (the sound files are in `cd_image/sound`).
    pub fn start(client_dir: &Path) -> Result<Audio> {
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or_else(|| anyhow!("no audio output device"))?;
        let name = dev.name().unwrap_or_else(|_| "?".into());
        let cfg = dev.default_output_config().context("output config")?;
        let (rate, ch) = (cfg.sample_rate().0, cfg.channels() as usize);
        let sh = Shared::new(client_dir.join("cd_image/sound"), rate);
        let rt = Runtime::new(&sh, Library::load(&sh.root).context("client sound data")?, seed());
        let m = sh.clone();
        let mut scratch: Vec<f32> = Vec::new();
        let err = |e| eprintln!("audio stream error: {e}");
        let config: cpal::StreamConfig = cfg.clone().into();
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => dev.build_output_stream(
                &config,
                move |out: &mut [f32], _| write_frames(&m, &mut scratch, out, ch, |s| s),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => dev.build_output_stream(
                &config,
                move |out: &mut [i16], _| write_frames(&m, &mut scratch, out, ch, |s| (s * 32767.0) as i16),
                err,
                None,
            ),
            f => return Err(anyhow!("unsupported output sample format {f:?}")),
        }
        .context("build output stream")?;
        stream.play().context("start output stream")?;
        Ok(Audio { sh, rt: Mutex::new(Some(rt)), _stream: Some(stream), device: format!("{name} ({rate} Hz, {ch} ch)") })
    }

    /// Device-less engine that mixes nowhere: drive it with [`Audio::render`] (tests, offline renders).
    pub fn offline(client_dir: &Path, rate: u32) -> Audio {
        let sh = Shared::new(client_dir.join("cd_image/sound"), rate);
        let rt = Library::load(&sh.root).ok().map(|l| Runtime::new(&sh, l, 1));
        Audio { sh, rt: Mutex::new(rt), _stream: None, device: "offline".into() }
    }

    pub fn render(&self, out: &mut [f32]) {
        self.sh.mixer().render(out);
    }

    /// Device output gain after the level statistics (0 = silent run that still reports voices / RMS).
    pub fn set_output_gain(&self, g: f32) {
        self.sh.mixer().output = g;
    }

    pub fn stats(&self) -> Arc<Stats> {
        self.sh.mixer().stats.clone()
    }

    /// One line of mixer activity since start (callbacks, frames, voices, last block RMS, peak).
    pub fn status(&self) -> String {
        let s = self.stats();
        format!(
            "audio[{}]: {} callbacks, {} frames, {} voices, {} emitters, rms {:.4}, peak {:.3}",
            self.device,
            s.callbacks.load(Relaxed),
            s.frames.load(Relaxed),
            s.voices.load(Relaxed),
            self.active_emitters(),
            f32::from_bits(s.rms_bits.load(Relaxed)),
            f32::from_bits(s.peak_bits.load(Relaxed)),
        )
    }

    fn rt(&self) -> MutexGuard<'_, Option<Runtime>> {
        self.rt.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Applies the client's sound prefs (`SoundOptionsMonitor_c`, GUI @0x100c4ace): `Total_FX = master * FX * mutes`,
    /// `Total_Music = master * Music * mutes`.
    pub fn set_prefs(&self, p: &Prefs) {
        if let Some(rt) = self.rt().as_mut() {
            rt.fx = if p.sound_on && p.fx_on { p.master * p.fx } else { 0.0 };
            rt.music.volume = if p.sound_on && p.music_on { p.master * p.music } else { 0.0 };
            rt.combat.set_pref(p.battlemusic_mode);
        }
    }

    /// Weather state s0..s6 (rain, fog, cloud, wind, sand, fallout R/G storms): selects the fog/rain/storm music slot.
    /// Feed it `ao_formats::weather::State::music_state()` every frame (see docs *Weather*); the default is clear sky.
    pub fn set_weather(&self, s: [f32; 7]) {
        if let Some(rt) = self.rt().as_mut() {
            rt.weather = s;
        }
    }

    /// Module flag +0xbc (server driven): land-control districts play `Landcontrol_neutral`.
    pub fn set_land_control(&self, on: bool) {
        if let Some(rt) = self.rt().as_mut() {
            rt.land_control = on;
        }
    }

    /// `PlaySample` of a keep-alive sound such as `SM_Sandy_CC_Ambience`: call every frame while it should sound;
    /// it ends 1 s + 4 s (duration + fade-out of the definition) after the last call. `update` must run every frame.
    pub fn play_ui_keepalive(&self, name: &str) {
        let mut g = self.rt();
        let Some(rt) = g.as_mut() else { return };
        let db = rt.lib.sounds.clone();
        if let Some(d) = db.by_name(name) {
            rt.keepalive(&self.sh, d);
        }
    }

    /// Like [`Audio::play_ui_keepalive`] with a per-call volume `0..=1` (`SM_Sandy_Env_Rain` / `Wind` / `SandWind` /
    /// `FalloutRWind` / `FalloutGWind` / `Quake` levels of `ao_formats::weather::Levels`, set every frame by the client's
    /// sky manager); a level of 0 does nothing.
    pub fn play_keepalive_level(&self, name: &str, level: f32) {
        if level <= 0.0 || level.is_nan() {
            return;
        }
        let mut g = self.rt();
        let Some(rt) = g.as_mut() else { return };
        let db = rt.lib.sounds.clone();
        if let Some(d) = db.by_name(name) {
            rt.keepalive_scaled(&self.sh, d, level.min(1.0));
        }
    }

    /// Enters playfield `pf` (`None` leaves it): district music, ambience and statel emitters follow [`Audio::update`].
    pub fn set_playfield(&self, pf: Option<PlayfieldAudio>) {
        if let Some(rt) = self.rt().as_mut() {
            rt.set_playfield(&self.sh, pf);
        }
    }

    /// Per frame: `cam` = camera = listener position (scene space), `day_time` = the viewer clock (0..6480 s).
    pub fn update(&self, dt: f32, cam: [f32; 3], day_time: f32) {
        if let Some(rt) = self.rt().as_mut() {
            static LOG: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("AOMAC_AUDIO_LOG").is_some());
            let log = *LOG;
            let before = (rt.combat.state(), rt.music.layer());
            rt.update(&self.sh, dt, cam, day_time);
            let after = (rt.combat.state(), rt.music.layer());
            if log && before != after {
                eprintln!("audio combat={} layer={:?} sample={:?}", after.0, after.1.map(|l| &rt.music.project().layers[l].name), rt.music.now_playing);
            }
        }
    }

    /// One `SandyInterface_t::CombatUpdate` call (see [`crate::combat`]): feed every character each game frame
    /// (`ao_audio::char_sample` builds the sample like `Gamecode FUN_10059736`) *before* [`Audio::update`].
    pub fn set_combat_sample(&self, s: &CombatSample) {
        if let Some(rt) = self.rt().as_mut() {
            rt.combat.combat_update(s);
        }
    }

    /// [`char_sample`] + [`Audio::set_combat_sample`] for one character.
    pub fn set_combat_char(&self, c: &CharInfo) {
        if let Some(s) = char_sample(c) {
            self.set_combat_sample(&s);
        }
    }

    /// Combat music state 0..=16 (`SandyInterface_t+0x94`); 1..=16 override the district layer at its next 1 Hz evaluation.
    pub fn combat_state(&self) -> u8 {
        self.rt().as_ref().map_or(0, |r| r.combat.state())
    }

    /// `SetCombatMusicOverride(name)` (tweak value `CombatMusicOverride`): replaces every combat table layer.
    pub fn set_combat_music_override(&self, name: Option<&str>) {
        if let Some(rt) = self.rt().as_mut() {
            rt.combat.set_override(name);
        }
    }

    /// Music layer by name (`forest\day`); `None` fades the music out. Like `PlayMusic`, a combat state 1..=16
    /// replaces the requested layer.
    pub fn set_music_layer(&self, name: Option<&str>) -> bool {
        let mut g = self.rt();
        let Some(rt) = g.as_mut() else { return false };
        let layer = match name {
            Some(n) => match rt.lib.project.find_layer(n) {
                Some(l) => Some(l),
                None => return false,
            },
            None => None,
        };
        rt.music.signal(rt.combat_layer().unwrap_or(layer));
        true
    }

    /// Login/startup music (`SandyInterface_t::PlayStartupMusic` = layer `mountain\night`).
    pub fn play_startup_music(&self) {
        self.set_music_layer(Some("mountain\\night"));
    }

    /// Statel sound emitters that are currently audible (camera inside their radius).
    pub fn active_emitters(&self) -> usize {
        self.rt().as_ref().map_or(0, |r| r.active_emitters())
    }

    /// Name of the current music layer (`desert\\Day`), `None` = silence.
    pub fn music_layer(&self) -> Option<String> {
        let g = self.rt();
        let m = &g.as_ref()?.music;
        m.layer().map(|l| m.project().layers[l].name.clone())
    }

    /// File name of the music sample that is playing (diagnostics).
    pub fn now_playing(&self) -> Option<String> {
        self.rt().as_ref().and_then(|r| r.music.now_playing.clone())
    }

    /// Plays a named sound definition of the client (`SM_Sandy_CC_GUI_Select`, ... in `SM_Sandy_Gui.sbf`;
    /// `SM_Sandy_*` is prepended with `CC_`/`Gui_` when the exact name is unknown) non-positionally.
    /// Returns the voice ids started.
    pub fn play_ui(&self, name: &str) -> Vec<u64> {
        let mut g = self.rt();
        let Some(rt) = g.as_mut() else { return Vec::new() };
        let db = rt.lib.sounds.clone();
        let def = [name.to_string(), format!("SM_Sandy_CC_{name}"), format!("SM_Sandy_Gui_{name}")].iter().find_map(|n| db.by_name(n));
        match def {
            Some(d) => rt.play(&self.sh, d),
            None => {
                eprintln!("audio: unknown sound '{name}'");
                Vec::new()
            }
        }
    }

    /// `SandyInterfaceModule_t::PlayGameSound(sound id, position)`: a one-shot of the sound definition `id` at `pos`, heard from `listener`
    /// (both in scene space; the level only depends on the distance, `game::attenuation`). Returns the voices started.
    pub fn play_game_sound(&self, id: u32, pos: [f32; 3], listener: [f32; 3]) -> Vec<u64> {
        self.play_game_sound_with(id, pos, listener, 0, 1)
    }

    /// [`Audio::play_game_sound`] with the game material and impact size arguments of the fight sounds (`game::variant_of`).
    pub fn play_game_sound_with(&self, id: u32, pos: [f32; 3], listener: [f32; 3], material: i32, size: i32) -> Vec<u64> {
        let mut g = self.rt();
        let Some(rt) = g.as_mut() else { return Vec::new() };
        let db = rt.lib.sounds.clone();
        let Some(def) = db.get(id) else { return Vec::new() };
        let d = if pos == [0.0; 3] { 0.0 } else { (0..3).map(|i| (pos[i] - listener[i]).powi(2)).sum::<f32>().sqrt() };
        rt.play_at(&self.sh, def, d, material, size)
    }

    /// CharCastNano's per-frame positional duration override (SI PlaySample 10002d98).
    /// Positive durations re-arm the definition's looping voice; zero remains a one-shot.
    pub fn play_game_sound_duration(&self, id: u32, pos: [f32; 3], listener: [f32; 3], duration: f32, volume: f32) -> Vec<u64> {
        let mut g = self.rt();
        let Some(rt) = g.as_mut() else {
            if audio_log() { eprintln!("audio rejection id={id:#x} reason=no-runtime source={pos:?} listener={listener:?}"); }
            return Vec::new();
        };
        let db = rt.lib.sounds.clone();
        let Some(def) = db.get(id) else {
            if audio_log() { eprintln!("audio rejection id={id:#x} reason=no-definition source={pos:?} listener={listener:?}"); }
            return Vec::new();
        };
        let d = if pos == [0.0; 3] { 0.0 } else { (0..3).map(|i| (pos[i] - listener[i]).powi(2)).sum::<f32>().sqrt() };
        if d > def.max_dist {
            if audio_log() { eprintln!("audio rejection id={id:#x} reason=distance distance={d} max={}", def.max_dist); }
            return Vec::new();
        } // SI10002d98 returns before re-arming an out-of-range source.
        if duration <= 0.0 {
            return rt.play_effect_at(&self.sh, def, d, volume, 0.0, 100);
        }
        let level = crate::game::attenuation(d, def.min_dist, def.max_dist, None) * volume;
        let voice = rt.keepalive_duration(&self.sh, def, level, duration);
        if voice == 0 {
            if audio_log() { eprintln!("audio rejection id={id:#x} reason=keepalive-sample sample={:?} resolved={:?}", def.file, def.file.as_deref().and_then(|f| self.sh.resolve(f))); }
            Vec::new()
        } else { vec![voice] }
    }

    /// GC4000 / SI100071ed command, including native delay and duration overrides.
    pub fn play_effect_sound(&self, id: u32, pos: [f32; 3], velocity: [f32; 3], listener: [f32; 3], parameters: [f32; 4], probability: i32) -> Result<Vec<u64>> {
        anyhow::ensure!(pos.iter().chain(&velocity).chain(&parameters).all(|v| v.is_finite()), "nonfinite effect sound command");
        let mut g = self.rt();
        let rt = g.as_mut().context("effect audio runtime unavailable")?;
        rt.effect_sound(&self.sh, crate::game::EffectSound { id, pos, velocity, parameters, probability }, listener)
    }

    /// Plays a file below `cd_image/sound` (e.g. `sfx/gui/click`) once, centred. Returns the voice id (0 = not played).
    pub fn play_sfx(&self, rel: &str, gain: f32) -> u64 {
        match self.sh.resolve(rel) {
            Some(p) => self.sh.play_sample(&p, gain, false, 1),
            None => {
                eprintln!("audio: no sound file for '{rel}'");
                0
            }
        }
    }

    /// Streams an absolute file path (music) with an optional fade-in. Returns the voice id (0 = failed).
    pub fn play_stream_file(&self, path: &Path, gain: f32, fade_in: f32) -> u64 {
        self.sh.play_stream(path, gain, fade_in)
    }

    /// Sets a voice's playback rate in percent (`SE_Update2DSoundPitch`).
    pub fn set_pitch(&self, id: u64, percent: f32) {
        self.sh.mixer().set_pitch(id, percent);
    }

    pub fn stop(&self, id: u64) {
        self.sh.mixer().stop(id);
    }
}

fn write_frames<T: Copy + Default>(sh: &Shared, scratch: &mut Vec<f32>, out: &mut [T], ch: usize, conv: impl Fn(f32) -> T) {
    let frames = out.len() / ch.max(1);
    scratch.resize(frames * 2, 0.0);
    sh.mixer().render(scratch);
    for (i, o) in out.chunks_mut(ch.max(1)).enumerate() {
        o.fill(T::default());
        o[0] = conv(scratch[2 * i]);
        if ch > 1 {
            o[1] = conv(scratch[2 * i + 1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_xyz_game_sound_is_non_positional_for_a_distant_listener() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/sound/SourceFiles/SM_Sandy_Game_Dummy.sbf").exists() { return; }
        let audio = Audio::offline(&dir, 44100);
        let listener = [1_000_000.0; 3];
        assert!(!audio.play_game_sound_with(0x94bb7805, [0.0; 3], listener, 0, 1).is_empty());
        assert!(audio.play_game_sound_with(0x94bb7805, [1.0, 0.0, 0.0], listener, 0, 1).is_empty());
        assert!(audio.play_game_sound_duration(0x35a9ce7d, [1.0, 0.0, 0.0], listener, 0.2, 1.0).is_empty());
        assert!(!audio.play_game_sound_duration(0x35a9ce7d, [0.0; 3], listener, 0.2, 1.0).is_empty());
    }
}
