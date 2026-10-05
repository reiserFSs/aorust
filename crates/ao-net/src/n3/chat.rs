//! Zone N3 messages that carry text for the chat windows: `ChatTextIIR_t`, `FeedbackIIR_t`, `FormatFeedbackIIR_t`,
//! plus the `RemoteFormat` blob (`~&` + base-85 header + typed args) their text may contain, and the
//! client -> zone chat buffer built by `FUN_100891a4` [GUI]. RE, addresses and open questions: docs/chat/zone.md.
//!
//! None of the three message classes occurs in `docs/captures/*.rec` (no chat in the recordings); the layouts come from
//! the class `ReadSubClass`/`WriteSubClass` code [GC 0x1003871a/0x10038608, 0x10072e1d/..., 0x100392ff/0x10039289].

use super::N3Header;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

pub const CHAT_TEXT: u32 = 0x5F4B_442A; // ChatTextIIR_t
pub const FEEDBACK: u32 = 0x5054_4D19; // FeedbackIIR_t
pub const FORMAT_FEEDBACK: u32 = 0x206B_4B73; // FormatFeedbackIIR_t

/// `ChatTextIIR_t` [GC ctor 0x1003864f, read 0x1003871a, write 0x10038608, apply 0x10038677].
/// Wire: `u16 len` + text, `u8 color`, `u8 screen`, `u32 channel`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatText {
    /// Raw text bytes (may contain a `RemoteFormat` blob, see [`parse_remote`]).
    pub text: Vec<u8>,
    /// Object `+0x38`: the `ColorCode_e` handed to the chat window (`ChatGUIModule_c::ColorCodeToHTMLColor` [GUI 0x10087860]).
    pub color: u8,
    /// Object `+0x3c`: 1 = show as on-screen text (AFCM `0x19`, `RenderTextModule_t`) instead of in a chat window.
    /// The read function only stores `color`/`screen` when this byte is `< 2`; the apply code then sees them as 0 (not modelled, [`decode`] errors).
    pub screen: bool,
    /// Object `+0x18`: chat-window id (`< 0x40000000`) or group id, first argument of the `GlobalSignals_c` `+0x17c` signal.
    pub channel: u32,
}

/// `FeedbackIIR_t` [GC ctor 0x10072dff, read 0x10072e1d, apply 0x10072e81]: text-db line `(category, id)` shown with colour 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feedback {
    /// Object `+0x18` (signal channel).
    pub channel: u32,
    /// Object `+0x1c`: `LDBface::GetText` category.
    pub category: u32,
    /// Object `+0x20`: `LDBface::GetText` id.
    pub id: u32,
}

/// `FormatFeedbackIIR_t` [GC ctor 0x100391ff, read 0x100392ff, write 0x10039289, apply 0x10039341].
/// Wire: `u32 channel`, `u16 len` + `RemoteFormat` dump, `u32 mode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatFeedback {
    /// Object `+0x18`.
    pub channel: u32,
    /// Object `+0x20`: the `RemoteFormat::Dump` text (`~&...~`).
    pub format: Vec<u8>,
    /// Object `+0x3c`, `FUN_1005aaa9` [GC]: 1 = signal `+0x1d8` (on-screen), 2 = AFCM `0x19` on-screen text, anything else = chat line (colour 16).
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum N3Chat {
    Text(ChatText),
    Feedback(Feedback),
    Format(FormatFeedback),
}

/// `FUN_100388e2` [GC]: `u16` length + bytes; `>= 0x8000` flags the stream bad (error here).
fn lstr(r: &mut Reader) -> Result<Vec<u8>> {
    let n = r.u16()? as usize;
    if n >= 0x8000 {
        bail!("chat string length {n:#x} invalid");
    }
    Ok(r.bytes(n)?.to_vec())
}

/// `FUN_1003889d` [GC]: `u16` length (clamped to 0xFFFF) + bytes.
fn put_lstr(w: &mut Writer, s: &[u8]) {
    let s = &s[..s.len().min(0xFFFF)];
    w.u16(s.len() as u16);
    w.bytes(s);
}

pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<N3Chat>> {
    Ok(Some(match h.msg_type {
        CHAT_TEXT => {
            let text = lstr(r)?;
            let (color, screen) = (r.u8()?, r.u8()?);
            if screen > 1 {
                bail!("ChatTextIIR_t screen byte {screen}");
            }
            N3Chat::Text(ChatText { text, color, screen: screen == 1, channel: r.u32()? })
        }
        FEEDBACK => N3Chat::Feedback(Feedback { channel: r.u32()?, category: r.u32()?, id: r.u32()? }),
        FORMAT_FEEDBACK => N3Chat::Format(FormatFeedback { channel: r.u32()?, format: lstr(r)?, mode: r.u32()? }),
        _ => return Ok(None),
    }))
}

/// N3 payload (type, header identity, flag 1, body) as the server sends it; used for tests and a loopback.
pub fn encode(target: crate::msg::Identity, m: &N3Chat) -> Vec<u8> {
    let mut w = Writer::default();
    w.u32(match m {
        N3Chat::Text(_) => CHAT_TEXT,
        N3Chat::Feedback(_) => FEEDBACK,
        N3Chat::Format(_) => FORMAT_FEEDBACK,
    });
    target.write(&mut w);
    w.u8(0);
    match m {
        N3Chat::Text(t) => {
            put_lstr(&mut w, &t.text);
            w.u8(t.color);
            w.u8(t.screen as u8);
            w.u32(t.channel);
        }
        N3Chat::Feedback(f) => {
            w.u32(f.channel);
            w.u32(f.category);
            w.u32(f.id);
        }
        N3Chat::Format(f) => {
            w.u32(f.channel);
            put_lstr(&mut w, &f.format);
            w.u32(f.mode);
        }
    }
    w.0
}

// ---------------------------------------------------------------------------------------------------------------
// RemoteFormat (ldb.dll)
// ---------------------------------------------------------------------------------------------------------------

/// One typed argument of a `RemoteFormat` blob (`RemoteFormat::Init` [ldb 0x10005565]).
#[derive(Debug, Clone, PartialEq)]
pub enum RfArg {
    /// `'i'` + 5 base-85 chars.
    Int(i32),
    /// `'u'`.
    Uint(u32),
    /// `'f'` (IEEE bits).
    Float(f32),
    /// `'s'` + length-code + bytes; the string itself may be a nested blob (the client expands it, see the doc).
    Str(Vec<u8>),
    /// `'F'`: like `'s'` but the bytes are always a nested `RemoteFormat` (fed as its expansion, empty when it is not one).
    Nested(Vec<u8>),
    /// `'R'` + two base-85 words: another text-db entry `(category, id)` fed as its text.
    Entry(u32, u32),
}

/// A parsed `~&` blob: `GetText(cat, id)` is the template, `args` replace its `%`/`#N` tokens (`LDBformat`).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteFormat {
    pub cat: u32,
    pub id: u32,
    pub args: Vec<RfArg>,
    /// Bytes of the input the blob covers (index after the terminating `~`).
    pub consumed: usize,
}

/// `RemoteFormat::ToBase85` [ldb 0x1000410e]: 5 chars, most significant first, `'!' + digit`.
pub fn to_base85(mut v: u32) -> [u8; 5] {
    let mut o = [0u8; 5];
    for c in o.iter_mut().rev() {
        *c = b'!' + (v % 85) as u8;
        v /= 85;
    }
    o
}

/// `RemoteFormat::FromBase85` [ldb 0x1000415d]: 5 chars `'!'..='u'`; the accumulation wraps in `u32` like the original.
pub fn from_base85(s: &[u8]) -> Option<u32> {
    s.get(..5)?.iter().try_fold(0u32, |a, &c| (0x21..=0x75).contains(&c).then(|| a.wrapping_mul(85).wrapping_add((c - 0x21) as u32)))
}

/// `FUN_10004013` + `FUN_1000403f` [ldb]: the length prefix of `'s'`/`'F'` args is a UTF-8 code point holding `len + 1`.
/// Returns `(value, bytes used)`; a malformed sequence yields the lead byte itself (1 byte), `0x92` is mapped to `0x27`.
fn code_point(s: &[u8]) -> Option<(u32, usize)> {
    let b = *s.first()?;
    if b == 0x92 {
        return Some((0x27, 1));
    }
    let n = match b {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xFF => 4,
        _ => return Some((b as u32, 1)),
    };
    let tail = s.get(1..n);
    match tail {
        Some(t) if t.iter().all(|&c| c & 0xC0 == 0x80) => {
            let lead = (b as u32) & (0x7F >> n);
            Some((t.iter().fold(lead, |a, &c| a << 6 | (c & 0x3F) as u32), n))
        }
        _ => Some((b as u32, 1)),
    }
}

/// Parse a `RemoteFormat` blob at the start of `s` (`RemoteFormat::Init`): `~&` + `b85(cat)` + `b85(id)`, then args
/// (`i u f s F R`), terminated by `~`. `None` when `s` is not a complete blob (the caller keeps the `~` as plain text).
pub fn parse_remote(s: &[u8]) -> Option<RemoteFormat> {
    if s.len() < 12 || &s[..2] != b"~&" {
        return None;
    }
    let (cat, id) = (from_base85(&s[2..])?, from_base85(&s[7..])?);
    let mut args = Vec::new();
    let mut i = 12;
    while i < s.len() {
        match s[i] {
            c @ (b'i' | b'u' | b'f') => {
                let v = from_base85(s.get(i + 1..)?)?;
                args.push(match c {
                    b'i' => RfArg::Int(v as i32),
                    b'u' => RfArg::Uint(v),
                    _ => RfArg::Float(f32::from_bits(v)),
                });
                i += 6;
            }
            b'R' => {
                args.push(RfArg::Entry(from_base85(s.get(i + 1..)?)?, from_base85(s.get(i + 6..)?)?));
                i += 11;
            }
            c @ (b's' | b'F') => {
                let (v, n) = code_point(s.get(i + 1..)?)?;
                if v == 0 {
                    return None;
                }
                let start = i + 1 + n;
                let end = start.checked_add(v as usize - 1)?;
                let body = s.get(start..end)?.to_vec();
                args.push(if c == b's' { RfArg::Str(body) } else { RfArg::Nested(body) });
                i = end;
            }
            b'~' => return Some(RemoteFormat { cat, id, args, consumed: i + 1 }),
            _ => return None,
        }
    }
    None
}

/// Build a blob like `RemoteFormat(cat, id)` + `Feed(..)`* + `MarkEnd()` do.
pub fn encode_remote(cat: u32, id: u32, args: &[RfArg]) -> Vec<u8> {
    let mut o = b"~&".to_vec();
    o.extend(to_base85(cat));
    o.extend(to_base85(id));
    for a in args {
        match a {
            RfArg::Int(v) => {
                o.push(b'i');
                o.extend(to_base85(*v as u32));
            }
            RfArg::Uint(v) => {
                o.push(b'u');
                o.extend(to_base85(*v));
            }
            RfArg::Float(v) => {
                o.push(b'f');
                o.extend(to_base85(v.to_bits()));
            }
            RfArg::Entry(c, i) => {
                o.push(b'R');
                o.extend(to_base85(*c));
                o.extend(to_base85(*i));
            }
            RfArg::Str(b) | RfArg::Nested(b) => {
                o.push(if matches!(a, RfArg::Str(_)) { b's' } else { b'F' });
                let mut cp = [0u8; 4];
                o.extend(char::from_u32(b.len() as u32 + 1).unwrap_or('\u{FFFD}').encode_utf8(&mut cp).as_bytes());
                o.extend(b);
            }
        }
    }
    o.push(b'~');
    o
}

/// The chat buffer behind AFCM `0x16d/0x129/0x170` (`FUN_100891a4` [GUI 0x100891a4]): `u16 len` + text, `u8 kind`,
/// then, only when the message carries a macro/extras block, `BBBSS` = `1, a, b, S, S`.
/// `None` when the text is longer than 0x400 bytes (the original drops it silently).
pub fn chat_buffer(text: &[u8], kind: u8) -> Option<Vec<u8>> {
    if text.len() > 0x400 {
        return None;
    }
    let mut w = Writer::default();
    w.u16(text.len() as u16);
    w.bytes(text);
    w.u8(kind);
    Some(w.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::Identity;

    #[test]
    fn keys_match_the_registry() {
        assert_eq!(super::super::misc::key("ChatTextIIR_t"), CHAT_TEXT);
        assert_eq!(super::super::misc::key("FeedbackIIR_t"), FEEDBACK);
        assert_eq!(super::super::misc::key("FormatFeedbackIIR_t"), FORMAT_FEEDBACK);
    }

    fn roundtrip(m: N3Chat) {
        let p = encode(Identity { kind: 0xC350, instance: 7 }, &m);
        let (h, mut r) = N3Header::parse(&p).unwrap();
        assert_eq!(decode(&h, &mut r).unwrap().unwrap(), m);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn wire_layouts() {
        let t = N3Chat::Text(ChatText { text: b"hi".to_vec(), color: 5, screen: false, channel: 0x40000002 });
        let p = encode(Identity { kind: 0xC350, instance: 7 }, &t);
        // type, identity, flag, u16 len + "hi", color, screen, channel
        assert_eq!(hex(&p), "5f4b442a0000c350000000070000026869050040000002");
        roundtrip(t);
        roundtrip(N3Chat::Feedback(Feedback { channel: 1, category: 2, id: 3 }));
        roundtrip(N3Chat::Format(FormatFeedback { channel: 9, format: encode_remote(20000, 5, &[]), mode: 0 }));
    }

    #[test]
    fn malformed_is_an_error() {
        let mut p = encode(Identity::default(), &N3Chat::Text(ChatText { text: b"abc".to_vec(), color: 1, screen: false, channel: 1 }));
        p.truncate(p.len() - 1);
        let (h, mut r) = N3Header::parse(&p).unwrap();
        assert!(decode(&h, &mut r).is_err());
        let mut p = encode(Identity::default(), &N3Chat::Text(ChatText { text: vec![], color: 1, screen: false, channel: 1 }));
        p[13 + 3] = 2; // screen byte
        let (h, mut r) = N3Header::parse(&p).unwrap();
        assert!(decode(&h, &mut r).is_err());
    }

    #[test]
    fn base85() {
        assert_eq!(&to_base85(0), b"!!!!!");
        assert_eq!(from_base85(&to_base85(0xDEADBEEF)), Some(0xDEADBEEF));
        assert_eq!(from_base85(b"!!!!\x7f"), None);
        assert_eq!(from_base85(b"!!!!"), None);
    }

    #[test]
    fn remote_blob() {
        let args = [
            RfArg::Int(-5),
            RfArg::Uint(7),
            RfArg::Float(1.5),
            RfArg::Str(b"Bob".to_vec()),
            RfArg::Entry(1005, 600),
            RfArg::Nested(encode_remote(1, 2, &[])),
            RfArg::Str(vec![b'x'; 200]), // length code needs a 2-byte code point
        ];
        let blob = encode_remote(20000, 77, &args);
        let mut s = blob.clone();
        s.extend(b" trailing");
        let p = parse_remote(&s).unwrap();
        assert_eq!((p.cat, p.id, p.consumed), (20000, 77, blob.len()));
        assert_eq!(p.args, args);
        // unterminated / not a blob
        assert!(parse_remote(&blob[..blob.len() - 1]).is_none());
        assert!(parse_remote(b"~&short").is_none());
        assert!(parse_remote(b"hello world, plain").is_none());
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
