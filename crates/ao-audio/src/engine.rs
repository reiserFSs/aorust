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
    pub voice_on: bool,
    pub voice: f32,
    /// `BattlemusicMode` 0..=3 (0 = no combat music; default 3, `MainPrefs.xml`).
    pub battlemusic_mode: i32,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { sound_on: true, master: 1.0, fx_on: true, fx: 1.0, music_on: true, music: 1.0, voice_on: true, voice: 1.0, battlemusic_mode: 3 }
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
        let restart = {
            let mut m = self.sh.mixer();
            let restart = !m.sound_on && p.sound_on;
            if m.sound_on && !p.sound_on { m.stop_all(); }
            if !m.sound_on && p.sound_on { m.player_prefs = None; }
            m.sound_on = p.sound_on;
            m.fx = if p.sound_on && p.fx_on { p.master * p.fx } else { 0.0 };
            m.music = if p.sound_on && p.music_on { p.master * p.music } else { 0.0 };
            // SI 0x10006dfa/0x10006e4c recompute the player total on voice preference
            // changes. SetMasterVolume 0x10006d37 does not recompute that total.
            if m.player_prefs != Some((p.voice_on, p.voice)) {
                m.player = if p.voice_on { p.master * p.voice } else { 0.0 };
                m.player_prefs = Some((p.voice_on, p.voice));
            }
            restart
        };
        if let Some(rt) = self.rt().as_mut() {
            if restart { rt.restart_device(); }
            rt.combat.set_pref(p.battlemusic_mode);
        }
    }

    /// Effective preference gains of the running mixer, in FX/music/player order.
    /// Player gain retains Sandy's last voice-callback master snapshot.
    pub fn preference_gains(&self) -> [f32; 3] {
        let m = self.sh.mixer();
        if m.sound_on { [m.fx, m.music, m.player] } else { [0.0; 3] }
    }

    /// Weather state s0..s6 (rain, fog, cloud, wind, sand, fallout R/G storms): selects the fog/rain/storm music slot.
    /// Feed it `ao_formats::weather::State::music_state()` every frame (see docs *Weather*); the default is clear sky.
    pub fn set_weather(&self, s: [f32; 7]) {
        if let Some(rt) = self.rt().as_mut() {
            rt.weather = s;
        }
    }

    /// GC100be767/100bfa11 per-frame weather keepalives and GC100b6d67 music inputs.
    pub fn update_weather(&self, state: &ao_formats::weather::State) {
        self.set_weather(state.music_state());
        for (name, level) in [
            ("SM_Sandy_Env_Rain", state.levels.rain),
            ("SM_Sandy_Env_Wind", state.levels.wind),
            ("SM_Sandy_Env_SandWind", state.levels.sand_wind),
            ("SM_Sandy_Env_FalloutRWind", state.levels.fallout_red_wind),
            ("SM_Sandy_Env_FalloutGWind", state.levels.fallout_green_wind),
            ("SM_Sandy_Env_Quake", state.levels.quake),
        ] {
            self.play_keepalive_level(name, level);
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

    /// `StopAllSounds` (SI 0x10007446) + `StopMusic` (0x1000221e): hard lifecycle stop, not a layer fade.
    /// Login itself does not request a music layer; the device and user preferences remain available.
    pub fn reset(&self) {
        if let Some(rt) = self.rt().as_mut() {
            rt.reset(&self.sh);
        } else {
            self.sh.mixer().stop_all();
        }
    }

    /// Per frame: `cam` = camera = listener position (scene space), `day_time` = the viewer clock (0..6480 s).
    pub fn update(&self, dt: f32, cam: [f32; 3], day_time: f32) {
        if !self.sh.mixer().sound_on { return; }
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

    /// Character-login startup music (`SandyInterface_t::PlayStartupMusic` = layer `mountain\night`).
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

    /// `SM_Sandy_Env_NearLightning` (`FUN_100b9850`, `ao_formats::lightning::Strike::audio`): a one-shot at the
    /// caller's own gain, non-positionally — the native handler computes `volume` itself from the strike's distance
    /// and extent, not the generic attenuation curve [`Audio::play_game_sound`] uses. Routed through the same
    /// SI10003f61/SI1000567d delayed-command queue as other authored effect sounds (`Audio::play_effect_sound`
    /// / `game::Runtime::effect_delays`), so `delay` (`AudioCue::delay` = distance / 100 s) is honored exactly like
    /// GC4000 effect delays, including being cleared on logout (`Runtime::reset`); `pos == listener == [0,0,0]`
    /// keeps the generic attenuation curve a no-op (distance 0), leaving only this module's own volume.
    /// `volume <= 0.0` plays nothing (not even queued).
    pub fn play_lightning_cue(&self, volume: f32, delay: f32) -> Result<Vec<u64>> {
        if volume <= 0.0 {
            return Ok(Vec::new());
        }
        let id = crate::sbf::sound_id("SM_Sandy_Env_NearLightning");
        self.play_effect_sound(id, [0.0; 3], [0.0; 3], [0.0; 3], [volume, 0.0, 0.0, delay], 100)
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
        rt.play_at(&self.sh, def, d, material, size, pos)
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
            return rt.play_effect_at(&self.sh, def, d, volume, 0.0, 100, pos);
        }
        let level = crate::game::attenuation(d, def.min_dist, def.max_dist, None) * volume;
        let voice = rt.keepalive_duration(&self.sh, def, level, duration, pos);
        if voice == 0 {
            if audio_log() { eprintln!("audio rejection id={id:#x} reason=keepalive-sample sample={:?} resolved={:?}", def.file, def.file.as_deref().and_then(|f| self.sh.resolve(f))); }
            Vec::new()
        } else { vec![voice] }
    }

    /// Drains the positional sound-danger spheres recorded since the last call (GC100b85a3 reads these once per
    /// `Life::Run` pass). Positions are in the same scene-space coordinates passed to `play_game_sound*`.
    pub fn take_life_dangers(&self) -> Vec<([f32; 3], f32)> {
        let mut g = self.rt();
        let Some(rt) = g.as_mut() else { return Vec::new() };
        std::mem::take(&mut rt.life_dangers)
    }

    /// GC4000 / SI100071ed command, including native delay and duration overrides.
    pub fn play_effect_sound(&self, id: u32, pos: [f32; 3], velocity: [f32; 3], listener: [f32; 3], parameters: [f32; 4], probability: i32) -> Result<Vec<u64>> {
        anyhow::ensure!(pos.iter().chain(&velocity).chain(&parameters).all(|v| v.is_finite()), "nonfinite effect sound command");
        let mut g = self.rt();
        let rt = g.as_mut().context("effect audio runtime unavailable")?;
        rt.effect_sound(&self.sh, crate::game::EffectSound { id, pos, velocity, parameters, probability }, listener)
    }

    /// Plays a player voice file below `cd_image/sound` once, centred. Sandy's player-FX
    /// volume is separate from ordinary sound effects. Returns the voice id (0 = not played).
    pub fn play_sfx(&self, rel: &str, gain: f32) -> u64 {
        match self.sh.resolve(rel) {
            Some(p) => {
                let Some(pcm) = self.sh.load(&p) else { return 0 };
                self.sh.mixer().play(VoiceDesc { gain, player: true, ..VoiceDesc::new(Source::Sample(pcm)) })
            }
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
    fn player_volume_recomputes_only_when_retail_voice_callbacks_fire() {
        let audio = Audio::offline(Path::new(""), 44100);
        let mut p = Prefs { master: 0.5, voice: 0.8, ..Prefs::default() };
        audio.set_prefs(&p);
        assert_eq!(audio.sh.mixer().player, 0.4);
        p.master = 0.25;
        audio.set_prefs(&p);
        assert_eq!(audio.sh.mixer().player, 0.4, "SetMasterVolume does not update the player total");
        p.voice = 0.4;
        audio.set_prefs(&p);
        assert_eq!(audio.sh.mixer().player, 0.1);
    }

    #[test]
    fn sound_preferences_update_existing_voices_without_restarting() {
        let audio = Audio::offline(Path::new(""), 44100);
        let pcm = Arc::new(Pcm { rate: 44100, channels: 1, samples: vec![0.1; 32] });
        let mut ids = Vec::new();
        for (priority, player) in [(Some(1), false), (None, false), (Some(1), true)] {
            ids.push(audio.sh.mixer().play(VoiceDesc {
                looping: true, priority, player, ..VoiceDesc::new(Source::Sample(pcm.clone()))
            }));
        }
        let mut out = [0.0; 32];
        let mut p = Prefs { master: 0.5, fx: 0.4, music: 0.6, voice: 0.8, ..Prefs::default() };
        audio.set_prefs(&p);
        audio.render(&mut out);
        assert!(out.iter().all(|s| (*s - 0.09).abs() < 1e-6));
        p.fx_on = false;
        p.music_on = false;
        audio.set_prefs(&p);
        audio.render(&mut out);
        assert!(out.iter().all(|s| (*s - 0.04).abs() < 1e-6));
        p.voice_on = false;
        audio.set_prefs(&p);
        audio.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
        p = Prefs::default();
        audio.set_prefs(&p);
        audio.render(&mut out);
        assert!(out.iter().all(|s| (*s - 0.3).abs() < 1e-6));
        assert!(ids.iter().all(|id| audio.sh.mixer().is_playing(*id)));
        p.sound_on = false;
        audio.set_prefs(&p);
        audio.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
        assert!(ids.into_iter().all(|id| !audio.sh.mixer().is_playing(id)));
        assert_eq!(audio.sh.mixer().play(VoiceDesc::new(Source::Sample(pcm.clone()))), 0);
        p.sound_on = true;
        audio.set_prefs(&p);
        assert_ne!(audio.sh.mixer().play(VoiceDesc::new(Source::Sample(pcm))), 0);
    }

    #[test]
    fn weather_frame_routes_music_and_each_keepalive() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/sound/SourceFiles/SM_Sandy_Game_Dummy.sbf").exists() { return; }
        let audio = Audio::offline(&dir, 44100);
        for channel in 0..6 {
            audio.reset();
            let mut state = ao_formats::weather::State::clear();
            state.fields[0] = 0.8;
            let levels = &mut state.levels;
            *[&mut levels.rain, &mut levels.wind, &mut levels.sand_wind,
                &mut levels.fallout_red_wind, &mut levels.fallout_green_wind, &mut levels.quake][channel] = 0.5;
            audio.update_weather(&state);
            assert_eq!(audio.rt().as_ref().unwrap().weather, state.music_state());
            let mut samples = vec![0.0; 44100 * 2];
            audio.render(&mut samples);
            assert!(samples.iter().any(|&sample| sample != 0.0), "weather channel {channel}");
        }
    }

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

    #[test]
    fn lightning_cue_plays_at_the_given_gain_and_nothing_at_zero() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/sound/SourceFiles/SM_Sandy_Game_Dummy.sbf").exists() { return; }
        let audio = Audio::offline(&dir, 44100);
        assert!(audio.play_lightning_cue(0.0, 0.0).unwrap().is_empty());
        assert!(audio.play_lightning_cue(-1.0, 0.0).unwrap().is_empty());
        assert!(!audio.play_lightning_cue(0.8, 0.0).unwrap().is_empty(), "delay 0 plays immediately");
    }

    #[test]
    fn lightning_cue_delay_queues_then_plays_and_is_cleared_on_logout() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/sound/SourceFiles/SM_Sandy_Game_Dummy.sbf").exists() { return; }
        let audio = Audio::offline(&dir, 44100);
        assert!(audio.play_lightning_cue(0.8, 0.4).unwrap().is_empty(), "a nonzero delay queues, nothing plays yet");
        audio.update(0.1, [0.0; 3], 0.0);
        let mut samples = vec![0.0; 4410 * 2];
        audio.render(&mut samples);
        assert!(samples.iter().all(|&s| s == 0.0), "still queued before the delay elapses");
        audio.update(0.35, [0.0; 3], 0.0);
        audio.render(&mut samples);
        assert!(samples.iter().any(|&s| s != 0.0), "the delayed cue must play once its delay elapses");
        // logout clears any still-pending delayed cue (`Runtime::reset`, the same queue GC4000
        // effect delays use).
        assert!(audio.play_lightning_cue(0.8, 10.0).unwrap().is_empty());
        audio.reset();
        audio.update(20.0, [0.0; 3], 0.0);
        let mut samples2 = vec![0.0; 4410 * 2];
        audio.render(&mut samples2);
        assert!(samples2.iter().all(|&s| s == 0.0), "reset must drop the pending delayed cue");
    }

    #[test]
    fn mission_completion_sounds_reach_the_mixer_for_a_world_listener() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/sound/SourceFiles/SM_Sandy_Game_Dummy.sbf").exists() { return; }
        let audio = Audio::offline(&dir, 44100);
        for (name, id) in [
            ("SM_Sandy_Gui_Mission_Complete", 0x76a54fdc),
            ("SM_Sandy_Gui_Mission_Complete_Clan", 0x3714f9bd),
            ("SM_Sandy_Gui_Mission_Complete_Omni", 0x3012f932),
            ("SM_Sandy_Gui_Mission_Complete_Neutral", 0x2d601cf1),
        ] {
            assert_eq!(crate::sbf::sound_id(name), id);
            audio.reset();
            assert!(!audio.play_game_sound_with(id, [0.0; 3], [1234.0, 56.0, -789.0], 0, 1).is_empty(), "{name}");
            let mut samples = vec![0.0; 44100 * 2];
            audio.render(&mut samples);
            let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
            assert!(rms > 0.0, "{name}: no audible mixer output");
        }
    }
}
