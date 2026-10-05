//! `sound/music/env/anarchy.sws` — the Sandy interactive-music project (SIMPlayer.dll `CProject`).
//!
//! Container = serialize.dll `ObjectArchive` (serialize.dll @10004d29, `Message_c::UnFlatten` @10003784):
//! ```text
//! u32 version (=1)   u32 nSymbols   nSymbols x { cstr tag, cstr name }
//! u32 nObjects       u32 blobLen    blob[blobLen]
//! nObjects x { u32 len, Message[len] }
//! Message = u32 1, u32 nEntries, entries { varint symbol, u32 type, u32 elemSize, u32 totalBytes, data }
//! varint  = 1 byte (<0x80) | 2 bytes (10xxxxxx xxxxxxxx) | 4 bytes (11xxxxxx ...), big endian
//! ```
//! `elemSize == 0` means the items are `u32 len + bytes` (strings). The class of an object is the symbol
//! named by its `__class_id__` entry. Object 0 is the `CProject`; object ids are indices into the list.
//! Sample ids inside transitions (`sampid`) are indices into the project's `samp` list.
use anyhow::{anyhow, bail, ensure, Result};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// Index into [`Project::samples`] of the sample that follows.
    pub to: usize,
    pub pri: i32,
    /// Position (ms) in the current sample where the next stream starts.
    pub fot_ms: u32,
    /// Fade-in offset (ms) into the next sample.
    pub fit_ms: u32,
    /// Cross-fade duration (ms); `fot + ftime == end_ms` for end-of-sample transitions.
    pub ftime_ms: u32,
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub layer: usize,
    /// File stem; file = `<music/env>/<layer dir>/<name>.{wav,mp3,ogg}`.
    pub name: String,
    pub end_ms: u32,
    /// `entflg`: may be chosen as the first sample of a layer.
    pub entry: bool,
    pub vol: f32,
    pub trans: Vec<Transition>,
    /// Transitions used when entering a pause (`ptrans`).
    pub ptrans: Vec<Transition>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PauseNode {
    pub play_beats: u32,
    pub pause_beats: u32,
    pub ptype: u32,
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
}

#[derive(Debug, Clone)]
pub struct PauseTrack {
    pub bpm: f32,
    pub loopto: usize,
    pub nodes: Vec<PauseNode>,
}

#[derive(Debug, Clone)]
pub struct Layer {
    /// As stored (backslashes), e.g. `forest\Day`. Layer id == index in [`Project::layers`].
    pub name: String,
    pub vol: f32,
    /// Index into [`Project::pauses`].
    pub pause: Option<usize>,
    /// Indices into [`Project::samples`].
    pub samples: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub layers: Vec<Layer>,
    pub samples: Vec<Sample>,
    pub pauses: Vec<PauseTrack>,
}

struct Entry {
    ty: u32,
    elem: u32,
    data: Vec<u8>,
}
struct Msg(HashMap<usize, Entry>);

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Rd<'a> {
    fn u32(&mut self) -> Result<u32> {
        let s = self.b.get(self.p..self.p + 4).ok_or_else(|| anyhow!("sws: truncated"))?;
        self.p += 4;
        Ok(u32::from_le_bytes(s.try_into().unwrap()))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n).ok_or_else(|| anyhow!("sws: overflow"))?).ok_or_else(|| anyhow!("sws: truncated"))?;
        self.p += n;
        Ok(s)
    }
    fn cstr(&mut self) -> Result<String> {
        let rest = self.b.get(self.p..).ok_or_else(|| anyhow!("sws: truncated"))?;
        let n = rest.iter().position(|&c| c == 0).ok_or_else(|| anyhow!("sws: unterminated string"))?;
        self.p += n + 1;
        Ok(String::from_utf8_lossy(&rest[..n]).into_owned())
    }
}

fn parse_msg(m: &[u8], nsym: usize) -> Result<Msg> {
    let mut r = Rd { b: m, p: 0 };
    let _ver = r.u32()?;
    let cnt = r.u32()? as usize;
    let mut ents = HashMap::new();
    for _ in 0..cnt {
        let b0 = *r.take(1)?.first().unwrap() as u32;
        let sym = if b0 < 0x80 {
            b0
        } else if b0 & 0xc0 == 0x80 {
            ((b0 & 0x3f) << 8) | r.take(1)?[0] as u32
        } else {
            let t = r.take(3)?;
            ((b0 & 0x3f) << 24) | (t[0] as u32) << 16 | (t[1] as u32) << 8 | t[2] as u32
        } as usize;
        ensure!(sym < nsym, "sws: bad symbol index {sym}");
        let ty = r.u32()?;
        let elem = r.u32()?;
        let total = r.u32()? as usize;
        ents.insert(sym, Entry { ty, elem, data: r.take(total)?.to_vec() });
    }
    Ok(Msg(ents))
}

struct Archive {
    names: Vec<String>,
    index: HashMap<String, usize>,
    objs: Vec<Msg>,
}
impl Archive {
    fn ent<'a>(&self, m: &'a Msg, key: &str) -> Option<&'a Entry> {
        m.0.get(self.index.get(key)?)
    }
    fn u32s(&self, m: &Msg, key: &str) -> Vec<u32> {
        self.ent(m, key).map_or(vec![], |e| e.data.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect())
    }
    fn f32s(&self, m: &Msg, key: &str) -> Vec<f32> {
        self.ent(m, key).map_or(vec![], |e| e.data.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
    }
    fn bytes(&self, m: &Msg, key: &str) -> Vec<u8> {
        self.ent(m, key).map_or(vec![], |e| e.data.clone())
    }
    fn strs(&self, m: &Msg, key: &str) -> Vec<String> {
        let Some(e) = self.ent(m, key) else { return vec![] };
        debug_assert_eq!(e.ty, 6);
        let _ = e.elem;
        let (mut p, mut out) = (0usize, vec![]);
        while p + 4 <= e.data.len() {
            let n = u32::from_le_bytes(e.data[p..p + 4].try_into().unwrap()) as usize;
            let s = e.data.get(p + 4..p + 4 + n).unwrap_or(&[]);
            out.push(String::from_utf8_lossy(s.split(|&c| c == 0).next().unwrap_or(&[])).into_owned());
            p += 4 + n;
        }
        out
    }
    fn class(&self, m: &Msg) -> &str {
        self.u32s(m, "__class_id__").first().and_then(|&i| self.names.get(i as usize)).map_or("", |s| s.as_str())
    }
}

impl Project {
    pub fn parse(data: &[u8]) -> Result<Project> {
        let mut r = Rd { b: data, p: 0 };
        ensure!(r.u32()? == 1, "sws: wrong archive version");
        let nsym = r.u32()? as usize;
        let (mut names, mut index) = (Vec::with_capacity(nsym), HashMap::new());
        for i in 0..nsym {
            let _tag = r.cstr()?;
            let name = r.cstr()?;
            index.insert(name.clone(), i);
            names.push(name);
        }
        let nobj = r.u32()? as usize;
        let blob = r.u32()? as usize;
        r.take(blob)?;
        let mut objs = Vec::with_capacity(nobj);
        for _ in 0..nobj {
            let len = r.u32()? as usize;
            objs.push(parse_msg(r.take(len)?, nsym)?);
        }
        ensure!(r.p == data.len(), "sws: {} trailing bytes", data.len() - r.p);
        let a = Archive { names, index, objs };

        // pause tracks, in object order
        let mut pause_of_obj = HashMap::new();
        let mut pauses = vec![];
        for (oid, m) in a.objs.iter().enumerate() {
            if a.class(m) != "PauseTrack_c" {
                continue;
            }
            let (pl, pa, pt, fi, fo) = (a.u32s(m, "pn_playbeats"), a.u32s(m, "pn_pausebeats"), a.u32s(m, "pn_ptype"), a.u32s(m, "pn_fadein"), a.u32s(m, "pn_fadeout"));
            let n = a.u32s(m, "numnodes").first().copied().unwrap_or(0) as usize;
            ensure!([pl.len(), pa.len(), pt.len(), fi.len(), fo.len()].iter().all(|&l| l >= n), "sws: short pause node arrays");
            let nodes = (0..n).map(|i| PauseNode { play_beats: pl[i], pause_beats: pa[i], ptype: pt[i], fade_in_ms: fi[i], fade_out_ms: fo[i] }).collect();
            pause_of_obj.insert(oid as u32, pauses.len());
            pauses.push(PauseTrack {
                bpm: a.u32s(m, "bpm").first().copied().unwrap_or(0) as f32,
                loopto: a.u32s(m, "loopto").first().copied().unwrap_or(0) as usize,
                nodes,
            });
        }

        let proj = a.objs.first().ok_or_else(|| anyhow!("sws: no objects"))?;
        if a.class(proj) != "CProject" {
            bail!("sws: object 0 is {:?}, not CProject", a.class(proj));
        }
        let names_l = a.strs(proj, "lname");
        let vols = a.f32s(proj, "lvol");
        let lpause = a.u32s(proj, "lpause");
        ensure!(vols.len() >= names_l.len() && lpause.len() >= names_l.len(), "sws: short layer arrays");
        let samp_objs = a.u32s(proj, "samp");

        let trans = |ids: Vec<u32>| -> Result<Vec<Transition>> {
            ids.iter()
                .map(|&t| {
                    let m = a.objs.get(t as usize).ok_or_else(|| anyhow!("sws: bad transition object"))?;
                    let sid = a.u32s(m, "sampid");
                    ensure!(sid.len() == 2, "sws: transition sampid");
                    let g = |k| a.u32s(m, k).first().copied().unwrap_or(0);
                    Ok(Transition { to: sid[1] as usize, pri: g("pri") as i32, fot_ms: g("fot"), fit_ms: g("fit"), ftime_ms: g("ftime") })
                })
                .collect()
        };
        let mut samples = Vec::with_capacity(samp_objs.len());
        for &oid in &samp_objs {
            let m = a.objs.get(oid as usize).ok_or_else(|| anyhow!("sws: bad sample object"))?;
            let g = |k| a.u32s(m, k).first().copied().unwrap_or(0);
            samples.push(Sample {
                layer: g("lid") as usize,
                name: a.strs(m, "name").into_iter().next().unwrap_or_default(),
                end_ms: g("endmtime"),
                entry: a.bytes(m, "entflg").first().is_some_and(|&b| b != 0),
                vol: a.f32s(m, "vol").first().copied().unwrap_or(1.0),
                trans: trans(a.u32s(m, "trans"))?,
                ptrans: trans(a.u32s(m, "ptrans"))?,
            });
        }
        let mut layers: Vec<Layer> = names_l
            .into_iter()
            .enumerate()
            .map(|(i, name)| Layer { name, vol: vols[i], pause: pause_of_obj.get(&lpause[i]).copied(), samples: vec![] })
            .collect();
        for (i, s) in samples.iter().enumerate() {
            layers.get_mut(s.layer).ok_or_else(|| anyhow!("sws: sample layer out of range"))?.samples.push(i);
        }
        for s in &samples {
            for t in s.trans.iter().chain(&s.ptrans) {
                ensure!(t.to < samples.len(), "sws: transition target out of range");
            }
        }
        Ok(Project { layers, samples, pauses })
    }

    /// Case-insensitive layer lookup, `/` == `\` (client: `CProject::FindLayerID`, which only maps `/`→`\`;
    /// the macOS port has to be case-insensitive because the disk directories differ in case).
    pub fn find_layer(&self, name: &str) -> Option<usize> {
        let n = name.replace('/', "\\");
        self.layers.iter().position(|l| l.name.eq_ignore_ascii_case(&n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cstr(v: &mut Vec<u8>, s: &str) {
        v.extend_from_slice(s.as_bytes());
        v.push(0);
    }
    fn ent(v: &mut Vec<u8>, sym: u8, ty: u32, elem: u32, data: &[u8]) {
        v.push(sym);
        for x in [ty, elem, data.len() as u32] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(data);
    }
    fn msg(ents: Vec<Vec<u8>>) -> Vec<u8> {
        let mut m = vec![];
        m.extend_from_slice(&1u32.to_le_bytes());
        m.extend_from_slice(&(ents.len() as u32).to_le_bytes());
        for e in ents {
            m.extend(e);
        }
        m
    }
    fn e(sym: u8, ty: u32, elem: u32, data: &[u8]) -> Vec<u8> {
        let mut v = vec![];
        ent(&mut v, sym, ty, elem, data);
        v
    }
    fn i32s(x: &[i32]) -> Vec<u8> {
        x.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn s(x: &str) -> Vec<u8> {
        let mut v = (x.len() as u32 + 1).to_le_bytes().to_vec();
        v.extend_from_slice(x.as_bytes());
        v.push(0);
        v
    }

    /// Tiny archive: 1 layer "a\B" with one pause track, 2 samples, one transition 0 -> 1.
    #[test]
    fn synthetic() {
        let syms = ["CProject", "__class_id__", "lname", "lpause", "lvol", "samp", "PauseTrack_c", "bpm", "loopto", "numnodes", "pn_playbeats", "pn_pausebeats", "pn_ptype", "pn_fadein", "pn_fadeout", "CSampleExtras", "lid", "name", "endmtime", "entflg", "vol", "trans", "CTransition", "sampid", "pri", "fot", "fit", "ftime"];
        let id = |n: &str| syms.iter().position(|&x| x == n).unwrap() as u8;
        let mut f = vec![];
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&(syms.len() as u32).to_le_bytes());
        for n in syms {
            cstr(&mut f, "");
            cstr(&mut f, n);
        }
        f.extend_from_slice(&5u32.to_le_bytes()); // objects: proj, pause, samp0, samp1, trans
        f.extend_from_slice(&0u32.to_le_bytes());
        let objs = vec![
            msg(vec![
                e(id("__class_id__"), 3, 4, &i32s(&[id("CProject") as i32])),
                e(id("lname"), 6, 0, &s("a\\B")),
                e(id("lpause"), 17, 4, &i32s(&[1])),
                e(id("lvol"), 10, 4, &0.5f32.to_le_bytes()),
                e(id("samp"), 17, 4, &i32s(&[2, 3])),
            ]),
            msg(vec![
                e(id("__class_id__"), 3, 4, &i32s(&[id("PauseTrack_c") as i32])),
                e(id("bpm"), 3, 4, &i32s(&[95])),
                e(id("loopto"), 3, 4, &i32s(&[1])),
                e(id("numnodes"), 3, 4, &i32s(&[2])),
                e(id("pn_playbeats"), 3, 4, &i32s(&[190, 94])),
                e(id("pn_pausebeats"), 3, 4, &i32s(&[950, 940])),
                e(id("pn_ptype"), 3, 4, &i32s(&[1, 1])),
                e(id("pn_fadein"), 3, 4, &i32s(&[0, 10000])),
                e(id("pn_fadeout"), 3, 4, &i32s(&[15000, 15000])),
            ]),
            msg(vec![
                e(id("__class_id__"), 3, 4, &i32s(&[id("CSampleExtras") as i32])),
                e(id("lid"), 3, 4, &i32s(&[0])),
                e(id("name"), 6, 0, &s("S0")),
                e(id("endmtime"), 3, 4, &i32s(&[5048])),
                e(id("entflg"), 0, 1, &[1]),
                e(id("vol"), 10, 4, &1.0f32.to_le_bytes()),
                e(id("trans"), 17, 4, &i32s(&[4])),
            ]),
            msg(vec![
                e(id("__class_id__"), 3, 4, &i32s(&[id("CSampleExtras") as i32])),
                e(id("lid"), 3, 4, &i32s(&[0])),
                e(id("name"), 6, 0, &s("S1")),
                e(id("endmtime"), 3, 4, &i32s(&[100])),
                e(id("entflg"), 0, 1, &[0]),
            ]),
            msg(vec![
                e(id("__class_id__"), 3, 4, &i32s(&[id("CTransition") as i32])),
                e(id("sampid"), 3, 4, &i32s(&[0, 1])),
                e(id("pri"), 3, 4, &i32s(&[2])),
                e(id("fot"), 3, 4, &i32s(&[4417])),
                e(id("fit"), 3, 4, &i32s(&[0])),
                e(id("ftime"), 3, 4, &i32s(&[631])),
            ]),
        ];
        for o in objs {
            f.extend_from_slice(&(o.len() as u32).to_le_bytes());
            f.extend(o);
        }
        let p = Project::parse(&f).unwrap();
        assert_eq!(p.layers.len(), 1);
        assert_eq!(p.layers[0].name, "a\\B");
        assert_eq!(p.layers[0].pause, Some(0));
        assert_eq!(p.layers[0].samples, vec![0, 1]);
        assert_eq!(p.pauses[0].nodes[1], PauseNode { play_beats: 94, pause_beats: 940, ptype: 1, fade_in_ms: 10000, fade_out_ms: 15000 });
        assert_eq!(p.samples[0].trans, vec![Transition { to: 1, pri: 2, fot_ms: 4417, fit_ms: 0, ftime_ms: 631 }]);
        assert!(p.samples[0].entry && !p.samples[1].entry);
        assert_eq!(p.find_layer("A/b"), Some(0));
        assert_eq!(p.find_layer("zzz"), None);
        assert!(Project::parse(&f[..f.len() - 3]).is_err());
    }

    #[test]
    fn real_anarchy_sws() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let path = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client/cd_image/sound/music/env/anarchy.sws");
        let Ok(data) = std::fs::read(&path) else { return };
        let p = Project::parse(&data).unwrap();
        assert_eq!(p.layers.len(), 91);
        assert_eq!(p.samples.len(), 1257);
        assert_eq!(p.pauses.len(), 26);
        assert_eq!(p.layers[5].name, "forest\\Day");
        assert_eq!(p.find_layer("forest/day"), Some(5));
        assert_eq!(p.layers[5].samples.len(), 26);
        let pt = &p.pauses[p.layers[5].pause.unwrap()];
        assert_eq!((pt.bpm, pt.loopto, pt.nodes.len()), (95.0, 1, 2));
        assert_eq!(pt.nodes[0].play_beats, 190);
        assert_eq!(p.layers[43].pause, None); // battle\Neutral
        let s0 = &p.samples[0];
        assert_eq!((s0.name.as_str(), s0.layer, s0.end_ms, s0.entry), ("FDay01", 5, 5048, true));
        assert_eq!(s0.trans.len(), 6);
        assert_eq!(s0.trans[1], Transition { to: 6, pri: 0, fot_ms: 4417, fit_ms: 0, ftime_ms: 631 });
        assert_eq!(p.samples.iter().filter(|s| s.layer == 5 && s.entry).count(), 3);
    }
}
