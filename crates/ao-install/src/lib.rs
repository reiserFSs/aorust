//! PRK client installer/patcher, ported from the PRK launcher (see docs/formats.md `## installer`).
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

const BASE: &str = "https://patches.project-rk.com/patches";

/// Semver-ish version as parsed by the launcher's `VersionIdentifier` (equality is field-wise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    pre: Option<String>,
    build: Option<String>,
}

impl Version {
    /// `M.m.p[-pre][+build]`; pre-release parts after the first `-` are concatenated (launcher quirk).
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            bail!("empty version");
        }
        let plus: Vec<&str> = s.split('+').collect();
        if plus.len() > 2 {
            bail!("invalid version string: {s}");
        }
        let dash: Vec<&str> = plus[0].split('-').collect();
        let pre = (dash.len() >= 2).then(|| dash[1..].concat());
        let nums: Vec<&str> = dash[0].split('.').collect();
        let n = |i: usize| -> Result<u32> {
            nums.get(i).ok_or_else(|| anyhow!("invalid version string: {s}"))?.parse().with_context(|| format!("invalid version string: {s}"))
        };
        Ok(Version { major: n(0)?, minor: n(1)?, patch: n(2)?, pre, build: plus.get(1).map(|b| b.to_string()) })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = self.pre.as_deref().filter(|p| !p.trim().is_empty()) {
            write!(f, "-{p}")?;
        }
        if let Some(b) = self.build.as_deref().filter(|b| !b.trim().is_empty()) {
            write!(f, "+{b}")?;
        }
        Ok(())
    }
}

/// Parse `patches.map`: lines `src = dst`; lines not splitting into exactly two `=` parts are skipped.
pub fn parse_map(text: &str) -> Result<Vec<(Version, Version)>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('=').collect();
        if parts.len() == 2 {
            out.push((Version::parse(parts[0].trim())?, Version::parse(parts[1].trim())?));
        }
    }
    Ok(out)
}

/// The chain of destination versions starting at `current` (stops at first version with no patch).
pub fn resolve_chain(map: &[(Version, Version)], current: &Version) -> Vec<Version> {
    let mut chain = Vec::new();
    let mut cur = current.clone();
    while let Some((_, dst)) = map.iter().find(|(s, _)| *s == cur) {
        if chain.contains(dst) {
            break; // cycle guard
        }
        chain.push(dst.clone());
        cur = dst.clone();
    }
    chain
}

/// Contents of `patch.version`, `None` if the client has none yet.
pub fn installed_version(client_dir: &Path) -> Result<Option<String>> {
    match fs::read_to_string(client_dir.join("patch.version")) {
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Install or update the client in `client_dir` to the latest version in patches.map.
pub fn run(client_dir: &Path) -> Result<()> {
    fs::create_dir_all(client_dir)?;
    let current = match installed_version(client_dir)? {
        Some(v) if !v.is_empty() => Version::parse(&v)?,
        _ => Version::parse("0.0.0")?,
    };
    eprintln!("current version: {current}, client dir: {}", client_dir.display());
    let map = parse_map(&ureq::get(&format!("{BASE}/patches.map")).call()?.into_string()?)?;
    let chain = resolve_chain(&map, &current);
    if chain.is_empty() {
        eprintln!("already up to date ({current})");
        return Ok(());
    }
    let dl_dir = client_dir.parent().unwrap_or(client_dir).join("downloads");
    fs::create_dir_all(&dl_dir)?;
    for dst in chain {
        eprintln!("patching to {dst}");
        let zip_path = dl_dir.join(format!("{dst}.zip"));
        download(&format!("{BASE}/{dst}.zip"), &zip_path)?;
        apply_patch(client_dir, &dst, &zip_path)?;
        eprintln!("now at {dst}");
    }
    Ok(())
}

/// Extract `zip_path` over the client, apply `<dst>.rdbpatch`, drop the zip, process `<dst>.df`, and only
/// then advance `patch.version` (atomically), so any failure leaves the old version and a rerun redoes
/// the step (extraction overwrites, rdbpatch is one transaction of `INSERT OR REPLACE`).
fn apply_patch(client_dir: &Path, dst: &Version, zip_path: &Path) -> Result<()> {
    eprintln!("extracting {dst}");
    if let Err(e) = extract_zip(zip_path, client_dir) {
        let _ = fs::remove_file(zip_path); // unreadable archive: force a fresh download next run
        return Err(e);
    }
    let rdb = client_dir.join(format!("{dst}.rdbpatch"));
    if rdb.exists() {
        eprintln!("applying {}", rdb.display());
        let n = apply_rdbpatch(&client_dir.join("cd_image/rdb.db"), &rdb)?;
        eprintln!("  {n} records");
    }
    fs::remove_file(zip_path)?;
    // the launcher leaves the .df in place after processing; so do we
    let df = client_dir.join(format!("{dst}.df"));
    if df.exists() {
        let n = process_df(&df, client_dir)?;
        eprintln!("  deleted {n} files");
    }
    let tmp = client_dir.join("patch.version.tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(dst.to_string().as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, client_dir.join("patch.version"))?;
    File::open(client_dir)?.sync_all()?; // persist the rename
    Ok(())
}

/// Download with HTTP Range resume into `dest` (kept on failure so the next run resumes).
/// A partial file is only continued when the server confirms the offset; a 416 is only trusted when
/// the local size equals the server's size, otherwise the stale file is discarded and fetched afresh.
fn download(url: &str, dest: &Path) -> Result<()> {
    let mut have = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    loop {
        let mut req = ureq::get(url);
        if have > 0 {
            req = req.set("Range", &format!("bytes={have}-"));
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(416, _)) if have > 0 => {
                let server = ureq::head(url).call().with_context(|| format!("HEAD {url}"))?;
                if server.header("Content-Length").and_then(|t| t.parse::<u64>().ok()) == Some(have) {
                    eprintln!("  HTTP 416, local file already complete ({have} B)");
                    return Ok(());
                }
                eprintln!("  HTTP 416 but local size {have} != server size, restarting");
                fs::remove_file(dest)?;
                have = 0;
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("GET {url}")),
        };
        eprintln!("  HTTP {}{}", resp.status(), if have > 0 { format!(" (Range: bytes={have}-)") } else { String::new() });
        match resp_plan(resp.status(), resp.header("Content-Range"), resp.header("Content-Length"), have) {
            Plan::Restart => {
                eprintln!("  Content-Range does not continue at {have}, restarting");
                fs::remove_file(dest)?;
                have = 0;
            }
            Plan::Append(total) => return stream(resp, dest, have, total, true),
            Plan::Fresh(total) => return stream(resp, dest, 0, total, false),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Plan {
    /// Continue the partial file; value = total size from `Content-Range`.
    Append(Option<u64>),
    /// Overwrite from byte 0; value = `Content-Length`.
    Fresh(Option<u64>),
    /// 206 for a different offset than requested: drop the partial file and ask again without Range.
    Restart,
}

/// Decide what to do with a successful response given `have` bytes already on disk.
fn resp_plan(status: u16, content_range: Option<&str>, content_length: Option<&str>, have: u64) -> Plan {
    if status != 206 {
        return Plan::Fresh(content_length.and_then(|t| t.parse().ok()));
    }
    // Content-Range: bytes a-b/total
    let parsed = content_range.and_then(|r| {
        let (range, total) = r.trim().strip_prefix("bytes ")?.split_once('/')?;
        let start: u64 = range.split_once('-')?.0.trim().parse().ok()?;
        Some((start, total.trim().parse::<u64>().ok()))
    });
    match parsed {
        Some((start, total)) if start == have => Plan::Append(total),
        _ => Plan::Restart,
    }
}

fn stream(resp: ureq::Response, dest: &Path, start: u64, total: Option<u64>, append: bool) -> Result<()> {
    let mut done = start;
    let mut file = if append { OpenOptions::new().append(true).create(true).open(dest)? } else { File::create(dest)? };
    let mut r = resp.into_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut last = u64::MAX;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if let Some(t) = total {
            let pct = done * 100 / t.max(1);
            if pct != last {
                eprint!("\r  downloading {}: {pct}%", dest.file_name().unwrap().to_string_lossy());
                last = pct;
            }
        }
    }
    eprintln!();
    if let Some(t) = total {
        if done != t {
            bail!("download truncated: {done}/{t} bytes (rerun to resume)");
        }
    }
    Ok(())
}

/// Extract with overwrite; rejects entries that would escape `dest` (zip-slip).
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let mut ar = zip::ZipArchive::new(File::open(zip_path)?).context("opening zip (delete it to re-download)")?;
    for i in 0..ar.len() {
        let mut e = ar.by_index(i)?;
        let rel = e.enclosed_name().ok_or_else(|| anyhow!("zip entry escapes target dir: {:?}", e.name()))?;
        let out = dest.join(rel);
        if e.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            io::copy(&mut e, &mut File::create(&out)?)?;
        }
    }
    Ok(())
}

/// Apply an `.rdbpatch` to rdb.db in one transaction, then delete the patch file. Returns record count.
pub fn apply_rdbpatch(db: &Path, patch: &Path) -> Result<usize> {
    let bytes = fs::read(patch)?;
    let mut pos = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let s = bytes.get(pos..pos.checked_add(n).ok_or_else(|| anyhow!("bad length"))?).ok_or_else(|| anyhow!("truncated rdbpatch"))?;
        pos += n;
        Ok(s)
    };
    let u32le = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let count = u32le(take(4)?) as i32;
    if count < 0 {
        bail!("negative record count");
    }
    // never create an empty rdb.db: a missing database means a broken client
    let mut conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX).with_context(|| format!("opening {}", db.display()))?;
    let tx = conn.transaction()?;
    for _ in 0..count {
        let ty = u32le(take(4)?);
        let id = u32le(take(4)?);
        let _version = u32le(take(4)?); // launcher ignores it and writes version 1
        let len = u32le(take(4)?) as i32;
        if len < 0 {
            bail!("negative record length");
        }
        let data = take(len as usize)?;
        // table name is a formatted u32, no injection possible
        tx.execute(&format!("CREATE TABLE IF NOT EXISTS [rdb_{ty}] (id INTEGER PRIMARY KEY, version INTEGER, data BLOB)"), [])?;
        tx.execute(&format!("INSERT OR REPLACE INTO [rdb_{ty}] (id, version, data) VALUES (?1, 1, ?2)"), rusqlite::params![id, data])?;
    }
    tx.commit()?;
    fs::remove_file(patch)?;
    Ok(count as usize)
}

/// Delete files listed in `.df` (relative, one per line) inside `client_dir`, then prune emptied
/// parent dirs (never `client_dir` itself). Entries escaping `client_dir` are errors. Returns files deleted.
pub fn process_df(df: &Path, client_dir: &Path) -> Result<usize> {
    let mut n = 0;
    for line in fs::read_to_string(df)?.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rel = PathBuf::from(line.replace('\\', "/"));
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            bail!("deletion entry escapes client dir: {line:?}");
        }
        let path = client_dir.join(&rel);
        if !path.is_file() {
            continue;
        }
        fs::remove_file(&path)?;
        n += 1;
        let mut dir = path.parent();
        while let Some(d) = dir.filter(|d| *d != client_dir && d.starts_with(client_dir)) {
            if fs::remove_dir(d).is_err() {
                break; // not empty
            }
            dir = d.parent();
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn map_and_chain() {
        let m = parse_map("0.0.0 = 0.6.1\r\n0.6.1 = 0.6.2\n0.6.2 = 0.7.0\ngarbage\n0.9.0 = 1.0.0\n").unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!(resolve_chain(&m, &v("0.0.0")), vec![v("0.6.1"), v("0.6.2"), v("0.7.0")]);
        assert_eq!(resolve_chain(&m, &v("0.6.2")), vec![v("0.7.0")]);
        assert!(resolve_chain(&m, &v("0.7.0")).is_empty());
        assert_eq!(v("1.2.3-rc-1+x").to_string(), "1.2.3-rc1+x");
        assert!(Version::parse("1.2").is_err());
    }

    fn patch_bytes(recs: &[(u32, u32, &[u8])]) -> Vec<u8> {
        let mut b = (recs.len() as i32).to_le_bytes().to_vec();
        for (t, id, d) in recs {
            b.extend(t.to_le_bytes());
            b.extend(id.to_le_bytes());
            b.extend(7u32.to_le_bytes());
            b.extend((d.len() as i32).to_le_bytes());
            b.extend(*d);
        }
        b
    }

    #[test]
    fn rdbpatch_applies() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rdb.db");
        let c = Connection::open(&db).unwrap();
        c.execute_batch("CREATE TABLE rdb_5 (id INTEGER PRIMARY KEY, version INTEGER, data BLOB); INSERT INTO rdb_5 VALUES (1, 1, x'00');").unwrap();
        let p = dir.path().join("x.rdbpatch");
        fs::write(&p, patch_bytes(&[(5, 1, b"new"), (5, 2, b"two"), (9, 3, b"t9")])).unwrap();
        assert_eq!(apply_rdbpatch(&db, &p).unwrap(), 3);
        assert!(!p.exists());
        let got: Vec<u8> = c.query_row("SELECT data FROM rdb_5 WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(got, b"new");
        let n: i64 = c.query_row("SELECT count(*) FROM rdb_5", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        let got: Vec<u8> = c.query_row("SELECT data FROM rdb_9 WHERE id=3", [], |r| r.get(0)).unwrap();
        assert_eq!(got, b"t9");
        // truncated patch leaves db untouched and file in place
        let mut bad = patch_bytes(&[(5, 8, b"zzzz")]);
        bad.truncate(bad.len() - 2);
        fs::write(&p, bad).unwrap();
        assert!(apply_rdbpatch(&db, &p).is_err());
        let n: i64 = c.query_row("SELECT count(*) FROM rdb_5", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn df_stays_inside() {
        let root = tempfile::tempdir().unwrap();
        let client = root.path().join("client");
        fs::create_dir_all(client.join("a/b")).unwrap();
        fs::write(client.join("a/b/f"), "x").unwrap();
        fs::write(client.join("keep"), "x").unwrap();
        fs::write(root.path().join("outside"), "x").unwrap();
        let df = client.join("p.df");
        fs::write(&df, "a/b/f\n\nmissing\n").unwrap();
        assert_eq!(process_df(&df, &client).unwrap(), 1);
        assert!(!client.join("a").exists() && client.join("keep").exists() && client.exists());
        fs::write(&df, "../outside\n").unwrap();
        assert!(process_df(&df, &client).is_err());
        fs::write(&df, format!("{}\n", root.path().join("outside").display())).unwrap();
        assert!(process_df(&df, &client).is_err());
        assert!(root.path().join("outside").exists());
    }

    #[test]
    fn zip_slip_rejected() {
        let root = tempfile::tempdir().unwrap();
        let client = root.path().join("client");
        fs::create_dir_all(&client).unwrap();
        let mk = |name: &str| {
            let p = root.path().join("t.zip");
            let mut w = zip::ZipWriter::new(File::create(&p).unwrap());
            w.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
            p
        };
        assert!(extract_zip(&mk("../evil"), &client).is_err());
        assert!(!root.path().join("evil").exists());
        extract_zip(&mk("d/ok.txt"), &client).unwrap();
        assert_eq!(fs::read(client.join("d/ok.txt")).unwrap(), b"hi");
    }

    fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(File::create(path).unwrap());
        for (n, d) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn failed_patch_keeps_version() {
        let root = tempfile::tempdir().unwrap();
        let client = root.path().join("client");
        fs::create_dir_all(client.join("cd_image")).unwrap();
        fs::write(client.join("patch.version"), "0.1.0").unwrap();
        Connection::open(client.join("cd_image/rdb.db")).unwrap().execute_batch("CREATE TABLE rdb_5 (id INTEGER PRIMARY KEY, version INTEGER, data BLOB);").unwrap();
        let (zip, dst) = (root.path().join("0.1.1.zip"), v("0.1.1"));
        let version = || fs::read_to_string(client.join("patch.version")).unwrap();

        // 1. corrupt archive: version stays, bad zip discarded so the next run re-downloads
        fs::write(&zip, b"not a zip").unwrap();
        assert!(apply_patch(&client, &dst, &zip).is_err());
        assert_eq!(version(), "0.1.0");
        assert!(!zip.exists());

        // 2. extraction hits an escaping entry after writing a good one: version stays, zip kept
        zip_with(&zip, &[("ok.txt", b"1"), ("../evil", b"2")]);
        assert!(apply_patch(&client, &dst, &zip).is_err());
        assert_eq!(version(), "0.1.0");

        // 3. truncated rdbpatch: version stays, zip + rdbpatch kept, db untouched
        let mut bad = patch_bytes(&[(5, 1, b"new")]);
        bad.truncate(bad.len() - 1);
        zip_with(&zip, &[("0.1.1.rdbpatch", &bad)]);
        assert!(apply_patch(&client, &dst, &zip).is_err());
        assert_eq!(version(), "0.1.0");
        assert!(zip.exists() && client.join("0.1.1.rdbpatch").exists());
        let c = Connection::open(client.join("cd_image/rdb.db")).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM rdb_5", [], |r| r.get::<_, i64>(0)).unwrap(), 0);

        // 4. a good patch afterwards succeeds: rdbpatch applied+removed, .df kept and processed, version advances
        fs::write(client.join("old.txt"), "x").unwrap();
        zip_with(&zip, &[("0.1.1.rdbpatch", &patch_bytes(&[(5, 1, b"new")])), ("0.1.1.df", b"old.txt")]);
        apply_patch(&client, &dst, &zip).unwrap();
        assert_eq!(version(), "0.1.1");
        assert!(!zip.exists() && !client.join("0.1.1.rdbpatch").exists() && client.join("0.1.1.df").exists() && !client.join("old.txt").exists());
        assert_eq!(c.query_row("SELECT count(*) FROM rdb_5", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    /// Minimal HTTP server over `data`: GET/HEAD, honours `Range: bytes=N-` unless `lie` (then answers
    /// 206 starting at 0). Serves until the process ends; returns the URL and a log of "METHOD status".
    fn serve(data: Vec<u8>, lie: bool) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/p.zip", l.local_addr().unwrap());
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let lg = log.clone();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut rd = BufReader::new(c.try_clone().unwrap());
                let (mut method, mut range) = (String::new(), None::<u64>);
                let mut line = String::new();
                while rd.read_line(&mut line).unwrap_or(0) > 2 {
                    if method.is_empty() {
                        method = line.split(' ').next().unwrap().to_string();
                    }
                    if let Some(r) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = r.trim().trim_end_matches('-').parse().ok();
                    }
                    line.clear();
                }
                let n = data.len() as u64;
                let (status, head, body): (u16, String, &[u8]) = match range {
                    Some(a) if a >= n => (416, format!("Content-Range: bytes */{n}\r\n"), &[]),
                    Some(_) if lie => (206, format!("Content-Range: bytes 0-{}/{n}\r\n", n - 1), &data[..]),
                    Some(a) => (206, format!("Content-Range: bytes {a}-{}/{n}\r\n", n - 1), &data[a as usize..]),
                    None => (200, String::new(), &data[..]),
                };
                lg.lock().unwrap().push(format!("{method} {status}"));
                let _ = write!(c, "HTTP/1.1 {status} X\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n", if status == 416 && method == "HEAD" { 0 } else { body.len() });
                if method != "HEAD" {
                    let _ = c.write_all(body);
                }
            }
        });
        (url, log)
    }

    #[test]
    fn download_resume_checks() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("p.zip");
        let (url, log) = serve(data.clone(), false);

        // valid partial: 206 and appended
        fs::write(&dest, &data[..30_000]).unwrap();
        download(&url, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(log.lock().unwrap().as_slice(), ["GET 206"]);

        // complete file: 416 confirmed by HEAD size, file untouched
        log.lock().unwrap().clear();
        download(&url, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(log.lock().unwrap().as_slice(), ["GET 416", "HEAD 200"]);

        // oversized stale file: 416 but size differs -> discarded and refetched in full
        log.lock().unwrap().clear();
        let mut big = data.clone();
        big.extend([9u8; 500]);
        fs::write(&dest, big).unwrap();
        download(&url, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(log.lock().unwrap().as_slice(), ["GET 416", "HEAD 200", "GET 200"]);

        // server answers 206 for the wrong offset: partial discarded, refetched from 0
        let (url, log) = serve(data.clone(), true);
        fs::write(&dest, &data[..30_000]).unwrap();
        download(&url, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert_eq!(log.lock().unwrap().first().unwrap(), "GET 206");
    }

    #[test]
    fn resp_plan_cases() {
        assert_eq!(resp_plan(200, None, Some("10"), 5), Plan::Fresh(Some(10)));
        assert_eq!(resp_plan(206, Some("bytes 5-9/10"), None, 5), Plan::Append(Some(10)));
        assert_eq!(resp_plan(206, Some("bytes 0-9/10"), None, 5), Plan::Restart);
        assert_eq!(resp_plan(206, None, None, 5), Plan::Restart);
    }

    #[test]
    fn rdbpatch_missing_db_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let (db, p) = (dir.path().join("rdb.db"), dir.path().join("x.rdbpatch"));
        fs::write(&p, patch_bytes(&[(5, 1, b"a")])).unwrap();
        assert!(apply_rdbpatch(&db, &p).is_err());
        assert!(!db.exists() && p.exists());
    }
}
