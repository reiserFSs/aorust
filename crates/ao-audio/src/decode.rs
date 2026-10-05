//! File decoding: RIFF WAV (PCM, IMA/MS ADPCM, MPEG layer 3 in format 0x55) and Ogg Vorbis via symphonia.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// A fully decoded sound: interleaved f32 frames.
#[derive(Clone, Debug, Default)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

/// Packet-wise decoder; yields interleaved f32 chunks. Used both for one-shot decoding and music streaming.
pub struct Source {
    reader: Box<dyn FormatReader>,
    dec: Box<dyn Decoder>,
    track: u32,
    buf: Option<SampleBuffer<f32>>,
    pub rate: u32,
    pub channels: u16,
}

impl Source {
    pub fn open(path: &Path) -> Result<Source> {
        let mut hint = Hint::new();
        let mss = match riff_mp3(path)? {
            // symphonia's WAV demuxer rejects format 0x55: feed the `data` chunk to the MPEG demuxer instead.
            Some(mp3) => {
                hint.with_extension("mp3");
                MediaSourceStream::new(Box::new(Cursor::new(mp3)), Default::default())
            }
            None => {
                if let Some(e) = path.extension().and_then(|e| e.to_str()) {
                    hint.with_extension(&e.to_ascii_lowercase());
                }
                let file = File::open(path).with_context(|| path.display().to_string())?;
                MediaSourceStream::new(Box::new(file), Default::default())
            }
        };
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| anyhow!("{}: probe: {e}", path.display()))?;
        let reader = probed.format;
        let t = reader.default_track().ok_or_else(|| anyhow!("{}: no audio track", path.display()))?;
        let dec = symphonia::default::get_codecs()
            .make(&t.codec_params, &DecoderOptions::default())
            .map_err(|e| anyhow!("{}: codec: {e}", path.display()))?;
        let rate = t.codec_params.sample_rate.ok_or_else(|| anyhow!("{}: no sample rate", path.display()))?;
        let channels = t.codec_params.channels.map(|c| c.count() as u16).unwrap_or(0);
        let track = t.id;
        Ok(Source { reader, dec, track, buf: None, rate, channels })
    }

    /// Next decoded chunk of interleaved samples (`None` at end of stream). A corrupt packet is skipped.
    pub fn next_chunk(&mut self) -> Result<Option<&[f32]>> {
        loop {
            let pkt = match self.reader.next_packet() {
                Ok(p) => p,
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(Error::ResetRequired) => return Ok(None),
                Err(e) => return Err(anyhow!("packet: {e}")),
            };
            if pkt.track_id() != self.track {
                continue;
            }
            match self.dec.decode(&pkt) {
                Ok(b) => {
                    let spec = *b.spec();
                    self.channels = spec.channels.count() as u16;
                    self.rate = spec.rate;
                    let sb = self.buf.get_or_insert_with(|| SampleBuffer::new(b.capacity() as u64, spec));
                    if sb.capacity() < b.capacity() * spec.channels.count() {
                        *sb = SampleBuffer::new(b.capacity() as u64, spec);
                    }
                    sb.copy_interleaved_ref(b);
                    return Ok(Some(self.buf.as_ref().unwrap().samples()));
                }
                Err(Error::DecodeError(_)) => continue,
                Err(e) => return Err(anyhow!("decode: {e}")),
            }
        }
    }

    /// Restarts the stream from the beginning (music loops).
    pub fn rewind(&mut self) -> Result<()> {
        use symphonia::core::formats::{SeekMode, SeekTo};
        use symphonia::core::units::Time;
        self.reader
            .seek(SeekMode::Accurate, SeekTo::Time { time: Time::new(0, 0.0), track_id: Some(self.track) })
            .map_err(|e| anyhow!("seek: {e}"))?;
        self.dec.reset();
        Ok(())
    }
}

/// For a RIFF/WAVE file whose `fmt ` tag is 0x55 (MPEG layer 3), the bytes of its `data` chunk.
fn riff_mp3(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut f = File::open(path).with_context(|| path.display().to_string())?;
    let mut head = [0u8; 12];
    if f.read_exact(&mut head).is_err() || &head[..4] != b"RIFF" || &head[8..] != b"WAVE" {
        return Ok(None);
    }
    let mut d = Vec::new();
    f.read_to_end(&mut d)?;
    let (mut pos, mut tag) = (0usize, 0u16);
    while pos + 8 <= d.len() {
        let size = u32::from_le_bytes(d[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        match &d[pos..pos + 4] {
            b"fmt " if size >= 2 && body + 2 <= d.len() => tag = u16::from_le_bytes([d[body], d[body + 1]]),
            b"data" => {
                return Ok((tag == 0x55).then(|| d[body..(body + size).min(d.len())].to_vec()));
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    Ok(None)
}

/// Decodes the whole file. A file with an empty `data` chunk (`sfx/env/nosound.wav`) is valid silence: 0 frames. Errors if it yields no samples.
pub fn decode_file(path: &Path) -> Result<Pcm> {
    let mut s = Source::open(path)?;
    let mut pcm = Pcm { rate: s.rate, channels: 0, samples: Vec::new() };
    while let Some(c) = s.next_chunk()? {
        pcm.samples.extend_from_slice(c);
    }
    pcm.rate = s.rate;
    pcm.channels = s.channels;
    pcm.channels = pcm.channels.max(1);
    Ok(pcm)
}
