//! Decodes every audio file below `<client>/cd_image/sound` and prints a survey (`audio_survey <client_dir>`).
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

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("client dir")).join("cd_image/sound");
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
                let ext = f.extension().unwrap().to_string_lossy().to_ascii_lowercase();
                *fmts.entry(format!("{ext} {} Hz {} ch", p.rate, p.channels)).or_default() += 1;
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
