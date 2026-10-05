//! The audio device + housekeeping: owns the mixer, loads sounds from `cd_image/sound`, streams music.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

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
        let id = m.play(VoiceDesc { gain, ..VoiceDesc::new(Source::Stream { rx, rate, channels }) });
        if id != 0 && fade_in > 0.0 {
            m.fade_from_zero(id, fade_in);
        }
        id
    }

    pub fn play_sample(&self, path: &Path, gain: f32, looping: bool) -> u64 {
        let Some(pcm) = self.load(path) else { return 0 };
        if pcm.frames() == 0 {
            return 0;
        }
        self.mixer().play(VoiceDesc { gain, looping, ..VoiceDesc::new(Source::Sample(pcm)) })
    }
}

fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
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

    /// `Total_FX` / `Total_Music` of the client (master x channel volume, 0..1; all default 1.0).
    pub fn set_volumes(&self, fx: f32, music: f32) {
        if let Some(rt) = self.rt().as_mut() {
            rt.fx = fx;
            rt.music.volume = music;
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
            rt.update(&self.sh, dt, cam, day_time);
        }
    }

    /// Music layer by name (`forest\day`); `None` fades the music out.
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
        rt.music.signal(layer);
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

    /// Plays a file below `cd_image/sound` (e.g. `sfx/gui/click`) once, centred. Returns the voice id (0 = not played).
    pub fn play_sfx(&self, rel: &str, gain: f32) -> u64 {
        match self.sh.resolve(rel) {
            Some(p) => self.sh.play_sample(&p, gain, false),
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
