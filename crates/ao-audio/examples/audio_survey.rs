//! Decodes every audio file below `<client>/cd_image/sound` and prints a survey (`audio_survey [client_dir]`, default `~/Games/ProjectRubiKa/client`).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn walk(d: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out)
        } else if matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("wav" | "ogg")) {
            out.push(p)
        }
    }
}

/// Container + codec from the file header (RIFF `fmt ` tag).
fn codec(f: &Path) -> String {
    let h = std::fs::read(f).map(|d| d[..d.len().min(24)].to_vec()).unwrap_or_default();
    if h.len() >= 22 && &h[..4] == b"RIFF" {
        let tag = u16::from_le_bytes([h[20], h[21]]);
        return match tag {
            1 => "wav PCM".into(),
            2 => "wav MS-ADPCM".into(),
            0x11 => "wav IMA-ADPCM".into(),
            0x55 => "wav MPEG-L3".into(),
            t => format!("wav tag {t:#x}"),
        };
    }
    "ogg Vorbis".into()
}

fn main() {
    let client = std::env::args().nth(1).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Games/ProjectRubiKa/client")));
    let root = client.unwrap_or_default().join("cd_image/sound");
    if !root.is_dir() {
        eprintln!("audio_survey: {} not found; usage: audio_survey [client_dir] (default ~/Games/ProjectRubiKa/client)", root.display());
        std::process::exit(2);
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let (mut ok, mut secs) = (0usize, 0f64);
    let mut fmts: BTreeMap<String, usize> = BTreeMap::new();
    let mut fails: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in &files {
        match ao_audio::decode::decode_file(f) {
            Ok(p) => {
                ok += 1;
                secs += p.frames() as f64 / p.rate as f64;
                let peak = p.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
                assert!(peak.is_finite());
                *fmts.entry(format!("{} {} Hz {} ch", codec(f), p.rate, p.channels)).or_default() += 1;
                if p.frames() == 0 {
                    *fmts.entry("(empty data chunk)".into()).or_default() += 1;
                }
            }
            Err(e) => fails.entry(e.to_string().split(": ").last().unwrap_or("").to_string()).or_default().push(format!("{}: {e}", f.strip_prefix(&root).unwrap().display())),
        }
    }
    println!("{} files, {} decoded, {} failed, {:.1} h of audio", files.len(), ok, files.len() - ok, secs / 3600.0);
    for (k, v) in &fmts {
        println!("  {k}: {v}");
    }
    for (k, v) in &fails {
        println!("FAIL [{k}] x{}: {}", v.len(), v.iter().take(3).cloned().collect::<Vec<_>>().join(" | "));
    }
}
