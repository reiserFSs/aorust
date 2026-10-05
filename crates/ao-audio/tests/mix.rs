//! Offline mixing checks: known sequences rendered to a buffer, RMS/peak asserted. No audio device needed.
use std::sync::Arc;

use ao_audio::decode::{decode_file, Pcm};
use ao_audio::mixer::{Falloff, Mixer, Source, VoiceDesc};

const RATE: u32 = 48_000;

fn sine(freq: f32, secs: f32, amp: f32, rate: u32) -> Arc<Pcm> {
    let n = (secs * rate as f32) as usize;
    Arc::new(Pcm { rate, channels: 1, samples: (0..n).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin()).collect() })
}

/// (rms left, rms right, peak) of `secs` of output.
fn render(m: &mut Mixer, secs: f32) -> (f32, f32, f32) {
    let mut buf = vec![0f32; (secs * RATE as f32) as usize * 2];
    m.render(&mut buf);
    let rms = |c: usize| (buf.iter().skip(c).step_by(2).map(|s| (*s as f64).powi(2)).sum::<f64>() / (buf.len() / 2) as f64).sqrt() as f32;
    (rms(0), rms(1), buf.iter().fold(0f32, |a, s| a.max(s.abs())))
}

fn near(a: f32, b: f32, tol: f32) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

#[test]
fn centre_pan_and_resample() {
    let mut m = Mixer::new(RATE);
    // 22.05 kHz source must be resampled to 48 kHz and keep its amplitude: sine rms = amp / sqrt 2, centre pan = x0.7071
    m.play(VoiceDesc { gain: 1.0, looping: true, ..VoiceDesc::new(Source::Sample(sine(441.0, 1.0, 0.8, 22_050))) });
    let (l, r, peak) = render(&mut m, 0.5);
    let want = 0.8 / 2f32.sqrt() * std::f32::consts::FRAC_1_SQRT_2;
    near(l, want, 0.01);
    near(r, want, 0.01);
    near(peak, 0.8 * std::f32::consts::FRAC_1_SQRT_2, 0.02);
}

#[test]
fn hard_pan_and_silence_after_end() {
    let mut m = Mixer::new(RATE);
    m.play(VoiceDesc { pan: -1.0, ..VoiceDesc::new(Source::Sample(sine(440.0, 0.1, 0.5, RATE))) });
    let (l, r, _) = render(&mut m, 0.11);
    assert!(l > 0.2 && r < 1e-3, "{l} {r}");
    assert_eq!(m.voice_count(), 0, "finished one-shot is removed");
    let (l, r, p) = render(&mut m, 0.1);
    assert_eq!((l, r, p), (0.0, 0.0, 0.0));
}

#[test]
fn emitter_distance_and_side() {
    let fall = Falloff { min: 4.0, max: 100.0, rolloff: 1.0 };
    let run = |pos: [f32; 3]| {
        let mut m = Mixer::new(RATE);
        // listener at origin looking down -z, +x is to the right (right-handed scene space)
        m.play(VoiceDesc { looping: true, emitter: Some((pos, fall)), ..VoiceDesc::new(Source::Sample(sine(440.0, 1.0, 1.0, RATE))) });
        render(&mut m, 0.5)
    };
    let (near_l, near_r, _) = run([4.0, 0.0, 0.0]);
    let (far_l, far_r, _) = run([8.0, 0.0, 0.0]);
    assert!(near_r > 0.6 && near_l < 1e-3, "emitter on the right: {near_l} {near_r}");
    near(far_r / near_r, 0.5, 0.01); // inverse distance beyond min
    assert!(far_l < 1e-3);
    let (l, r, _) = run([0.0, 0.0, -2.0]);
    near(l, r, 1e-4);
    near(l, 0.5, 0.01); // inside min distance: full volume x centre pan
    let (l, r, _) = run([0.0, 0.0, -150.0]);
    assert_eq!((l, r), (0.0, 0.0), "silent beyond max distance");
}

#[test]
fn fades_and_voice_cap() {
    let mut m = Mixer::new(RATE);
    let id = m.play(VoiceDesc { looping: true, pan: -1.0, ..VoiceDesc::new(Source::Sample(sine(440.0, 1.0, 1.0, RATE))) });
    m.fade(id, 0.0, 0.25, true);
    let (l1, _, _) = render(&mut m, 0.125); // first half of the ramp: louder than the second
    let (l2, _, _) = render(&mut m, 0.125);
    assert!(l1 > l2 && l2 > 0.0, "{l1} {l2}");
    render(&mut m, 0.1);
    assert_eq!(m.voice_count(), 0, "fade-out with stop ends the voice");
    for _ in 0..100 {
        m.play(VoiceDesc { looping: true, ..VoiceDesc::new(Source::Sample(sine(440.0, 1.0, 0.01, RATE))) });
    }
    assert_eq!(m.voice_count(), ao_audio::mixer::MAX_VOICES, "voices are capped (looping voices are never stolen)");
}

fn wav16(rate: u32, samples: &[i16]) -> Vec<u8> {
    let mut v = b"RIFF".to_vec();
    v.extend((36 + 2 * samples.len() as u32).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend([16, 0, 0, 0, 1, 0, 1, 0]);
    v.extend(rate.to_le_bytes());
    v.extend((rate * 2).to_le_bytes());
    v.extend([2, 0, 16, 0]);
    v.extend(b"data");
    v.extend((2 * samples.len() as u32).to_le_bytes());
    for s in samples {
        v.extend(s.to_le_bytes());
    }
    v
}

#[test]
fn wav_decode_and_stream_voice() {
    let dir = std::env::temp_dir().join(format!("ao-audio-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    let tone: Vec<i16> = (0..RATE).map(|i| (16384.0 * (std::f32::consts::TAU * 440.0 * i as f32 / RATE as f32).sin()) as i16).collect();
    std::fs::write(&path, wav16(RATE, &tone)).unwrap();
    let pcm = decode_file(&path).unwrap();
    assert_eq!((pcm.rate, pcm.channels, pcm.frames()), (RATE, 1, RATE as usize));
    near(pcm.samples.iter().fold(0f32, |a, s| a.max(s.abs())), 0.5, 0.01);

    // streamed through the decoder thread
    let sh = ao_audio::Audio::offline(&dir, RATE);
    let id = sh.play_stream_file(&path, 1.0, 0.0);
    assert_ne!(id, 0);
    let mut buf = vec![0f32; RATE as usize / 2 * 2];
    let mut rms = 0f64;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        sh.render(&mut buf);
        rms = rms.max((buf.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / buf.len() as f64).sqrt());
        if rms > 0.1 {
            break;
        }
    }
    near(rms as f32, 0.5 / 2f32.sqrt() * std::f32::consts::FRAC_1_SQRT_2, 0.05);
    std::fs::remove_dir_all(&dir).unwrap();
}
