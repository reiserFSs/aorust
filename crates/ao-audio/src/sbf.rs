//! Sandy sound-bank files `sound/SourceFiles/SM_Sandy_Game_Dummy.sbf` and `SM_Sandy_Gui.sbf`
//! (SandyInterface.dll `SandyInterface_t::ParseSourceFile` @10003520, sound object setter `FUN_100011b0`).
//!
//! Layout: `u32 version (=0x0b)`, then repeated `u32 count; count x record`, to EOF.
//! Record = 0xe4 byte block, then (if `flags & 0x10`) `u32 len + len bytes` file path (every byte stored
//! rotated right by 3: decode with `rotate_left(3)`), then `(rec[7] & 0x1f)` x `u32` child sound ids.
//!
//! | off | field |
//! |---|---|
//! | 0 | flags: b0 in "all sounds" list, b1 prefetch, b2 play all children, b3 sequential children, b4 has file, b5, b6 random child (no immediate repeat), b7 children by volume |
//! | 1 | bits0-6 play probability (100 = always, else `rand()%200 < p`), bit7 stop-group flag |
//! | 2,3 | min / max volume = byte/255 |
//! | 6 | bits0-4 noise level/10, bits5-6, bit7 |
//! | 7 | bits0-4 child count |
//! | 8,10,12 (u16) | /100 s: child interval min, max; fade-out |
//! | 0x10 | u32 sound id (`sound_id(name)`) |
//! | 0x14 | 46 x u32 material variant sound ids |
//! | 0xcc, 0xd0 | f32 duration max, min (s; 0 = play to end) |
//! | 0xd4, 0xd8, 0xdc, 0xe0 | f32 min dist, max dist, noise radius, fade-in (s) |
use anyhow::{anyhow, ensure, Result};
use std::collections::HashMap;

/// `CreateSoundID` (SandyInterface.dll @10005ad2 -> `FUN_10008213`): case-insensitive rotating xor hash
/// of the first 255 characters; 4-char groups big-endian `c0<<24|c1<<16|c2<<8|c3`, rotl by group index
/// (`i & 31`); a trailing partial group is packed `c0<<24 | c1<<16 | c2<<8` the same way.
pub fn sound_id(name: &str) -> u32 {
    let b: Vec<u8> = name.bytes().take(255).map(|c| c.to_ascii_lowercase()).collect();
    let n = b.len();
    let mut h = 0u32;
    for i in 0..n / 4 {
        let v = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        h ^= v.rotate_left((i & 31) as u32);
    }
    if n & 3 != 0 {
        let i = n / 4;
        let at = |k: usize| b.get(4 * i + k).copied().unwrap_or(0) as u32;
        let v = (at(0) << 24) | (at(1) << 16) | (at(2) << 8);
        h ^= v.rotate_left((i & 31) as u32);
    }
    if n == 0 {
        0
    } else {
        h
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoundDef {
    pub id: u32,
    /// Raw flag byte (see module table).
    pub flags: u8,
    /// Decoded path as stored, `/` separated, e.g. `sfx/general/rain_medium.wav`. The client opens
    /// `<cd_image>/sound/<file>` (`sprintf("%s/sound/%s")`, SandyInterface.dll `PrefetchSample` @1000227e);
    /// extension as stored, name case may differ from disk (Windows client).
    pub file: Option<String>,
    pub vol_min: f32,
    pub vol_max: f32,
    pub min_dist: f32,
    pub max_dist: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub duration_min: f32,
    pub duration_max: f32,
    /// Play probability (0..=100).
    pub prob: u8,
    pub children: Vec<u32>,
    /// Child interval (s), also the re-arm range of child timers.
    pub interval_min: f32,
    pub interval_max: f32,
    pub play_all: bool,
    pub sequential: bool,
    /// Flag b6: one child per trigger, random, never the same twice in a row.
    pub random_child: bool,
    /// Voice-pool priority (byte 6 bits 5-6): 0 = highest (503 records: env/ambience), 1 = default (5320), 2 (17).
    pub priority: u8,
    /// 46 material variant ids (0 = none).
    pub variants: Vec<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct SoundDb(HashMap<u32, SoundDef>);

impl SoundDb {
    pub fn parse(data: &[u8]) -> Result<SoundDb> {
        ensure!(data.len() >= 4 && u32::from_le_bytes(data[..4].try_into().unwrap()) == 0x0b, "sbf: bad version");
        let (mut p, mut db) = (4usize, HashMap::new());
        let rd = |p: &mut usize, n: usize| -> Result<&[u8]> {
            let s = data.get(*p..p.checked_add(n).ok_or_else(|| anyhow!("sbf: overflow"))?).ok_or_else(|| anyhow!("sbf: truncated"))?;
            *p += n;
            Ok(s)
        };
        while p < data.len() {
            let count = u32::from_le_bytes(rd(&mut p, 4)?.try_into().unwrap());
            for _ in 0..count {
                let r = rd(&mut p, 0xe4)?;
                let u16_ = |o: usize| u16::from_le_bytes([r[o], r[o + 1]]) as f32 / 100.0;
                let f = |o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
                let u = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
                let flags = r[0];
                let file = if flags & 0x10 != 0 {
                    let n = u32::from_le_bytes(rd(&mut p, 4)?.try_into().unwrap()) as usize;
                    let raw: Vec<u8> = rd(&mut p, n)?.iter().map(|b| b.rotate_left(3)).collect();
                    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                    Some(String::from_utf8_lossy(&raw[..end]).into_owned())
                } else {
                    None
                };
                let nch = (r[7] & 0x1f) as usize;
                let children = (0..nch).map(|_| Ok(u32::from_le_bytes(rd(&mut p, 4)?.try_into().unwrap()))).collect::<Result<Vec<_>>>()?;
                let d = SoundDef {
                    id: u(0x10),
                    flags,
                    file: file.filter(|s| !s.is_empty()),
                    vol_min: r[2] as f32 / 255.0,
                    vol_max: r[3] as f32 / 255.0,
                    min_dist: f(0xd4),
                    max_dist: f(0xd8),
                    fade_in: f(0xe0),
                    fade_out: u16_(12),
                    duration_min: f(0xd0),
                    duration_max: f(0xcc),
                    prob: r[1] & 0x7f,
                    children,
                    interval_min: u16_(8),
                    interval_max: u16_(10),
                    play_all: flags & 0x04 != 0,
                    sequential: flags & 0x08 != 0,
                    random_child: flags & 0x40 != 0,
                    priority: (r[6] >> 5) & 3,
                    variants: (0..46).map(|i| u(0x14 + 4 * i)).collect(),
                };
                // The client updates an existing object in place when an id repeats (later wins).
                db.insert(d.id, d);
            }
        }
        Ok(SoundDb(db))
    }

    pub fn get(&self, id: u32) -> Option<&SoundDef> {
        self.0.get(&id)
    }

    /// Later banks overwrite earlier ones, like loading a second .sbf into the same sound table.
    pub fn merge(&mut self, other: SoundDb) {
        self.0.extend(other.0);
    }

    pub fn by_name(&self, name: &str) -> Option<&SoundDef> {
        self.get(sound_id(name))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SoundDef> {
        self.0.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u32, flags: u8, file: Option<&str>, children: &[u32]) -> Vec<u8> {
        let mut r = vec![0u8; 0xe4];
        r[0] = flags | if file.is_some() { 0x10 } else { 0 };
        r[1] = 100;
        r[2] = 127;
        r[3] = 127;
        r[7] = children.len() as u8;
        r[12..14].copy_from_slice(&400u16.to_le_bytes());
        r[0x10..0x14].copy_from_slice(&id.to_le_bytes());
        r[0xd8..0xdc].copy_from_slice(&15f32.to_le_bytes());
        let mut v = r;
        if let Some(f) = file {
            v.extend_from_slice(&(f.len() as u32 + 1).to_le_bytes());
            v.extend(f.bytes().chain(Some(0)).map(|b| b.rotate_right(3)));
        }
        for c in children {
            v.extend_from_slice(&c.to_le_bytes());
        }
        v
    }

    #[test]
    fn synthetic_and_hash() {
        let mut f = 0x0bu32.to_le_bytes().to_vec();
        f.extend_from_slice(&2u32.to_le_bytes());
        f.extend(rec(sound_id("SM_Sandy_Test"), 0x01, Some("sfx/x/a.wav"), &[7, 8]));
        f.extend(rec(7, 0x04, None, &[]));
        let db = SoundDb::parse(&f).unwrap();
        let d = db.by_name("sm_sandy_test").unwrap();
        assert_eq!(d.file.as_deref(), Some("sfx/x/a.wav"));
        assert_eq!(d.children, vec![7, 8]);
        assert_eq!((d.prob, d.max_dist, d.fade_out), (100, 15.0, 4.0));
        assert!(db.get(7).unwrap().play_all);
        assert_eq!(db.len(), 2);
        assert!(SoundDb::parse(&f[..f.len() - 1]).is_err());
        assert_eq!(sound_id(""), 0);
    }

    #[test]
    fn real_banks() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client/cd_image/sound/SourceFiles");
        let (Ok(g), Ok(game)) = (std::fs::read(dir.join("SM_Sandy_Gui.sbf")), std::fs::read(dir.join("SM_Sandy_Game_Dummy.sbf"))) else { return };
        let gui = SoundDb::parse(&g).unwrap();
        let mut db = SoundDb::parse(&game).unwrap();
        assert_eq!(gui.len(), 85);
        // 6810 records (some ids repeat -> fewer unique)
        assert!(db.len() > 6000 && db.len() <= 6810, "{}", db.len());
        db.merge(gui);
        let rain = db.by_name("SM_Sandy_Env_Rain").unwrap();
        assert_eq!(rain.file.as_deref(), Some("sfx/general/rain_medium.wav"));
        assert_eq!((rain.min_dist, rain.max_dist, rain.fade_out, rain.duration_max), (0.0, 15.0, 4.0, 1.0));
        assert!((rain.vol_max - 127.0 / 255.0).abs() < 1e-6);
        let bg = db.by_name("SM_Sandy_Env_BackgroundDay_1").unwrap();
        assert_eq!(bg.file.as_deref(), Some("sfx/env/forest_bg_day.wav"));
        assert_eq!(bg.children.len(), 5);
        let c = db.get(bg.children[0]).unwrap();
        assert_eq!(c.file.as_deref(), Some("sfx/env/far_bird_nutty.wav"));
        assert_eq!((c.interval_min, c.interval_max), (15.0, 32.0));
        let l = db.by_name("SM_Sandy_Env_NearLightning").unwrap();
        assert!(l.random_child && l.prob == 50 && l.children.len() == 3);
        assert_eq!(db.by_name("SM_Sandy_CC_GUI_Select").unwrap().file.as_deref(), Some("sfx/cc/select.wav"));
    }
}
