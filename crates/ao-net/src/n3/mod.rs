//! Zone-server N3 messages (`ptype == 0xA`). The frame payload is the N3 message handed to
//! `n3EngineClient_t::ToClientN3Message(Identity_t const&, ACE_Data_Block*)`: `u32 msg_type`,
//! an [`Identity`], one flag byte, then the type-specific body (layouts: docs/zone.md).
//! Fixtures come from the sanitized live capture `docs/captures/zone_ithaca.rec`.

pub mod action;
pub mod combat;
pub mod chat;
pub mod dynel;
pub mod misc;
pub mod motion;
pub mod nametag;
pub mod outgoing;
pub mod world;

use crate::msg::Identity;
use crate::wire::Reader;
use anyhow::{bail, Result};

/// Common start of every N3 payload (seen live on all 1000+ zone messages; docs/zone.md §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct N3Header {
    pub msg_type: u32,
    /// Identity the message is about (`0xC350` = character/NPC dynel, `0x9C50` = playfield, ...).
    pub target: Identity,
    /// Byte after the identity (0 or 1 live; meaning in docs/zone.md).
    pub flag: u8,
}

impl N3Header {
    /// Split an N3 frame payload into the header and the body reader.
    pub fn parse(payload: &[u8]) -> Result<(N3Header, Reader<'_>)> {
        let mut r = Reader::new(payload);
        if r.remaining() < 13 {
            bail!("N3 payload of {} bytes is shorter than its header", payload.len());
        }
        let h = N3Header { msg_type: r.u32()?, target: Identity::read(&mut r)?, flag: r.u8()? };
        Ok((h, r))
    }
}

/// A decoded zone N3 message.
#[derive(Debug, Clone, PartialEq)]
pub enum N3 {
    World(world::World),
    Dynel(dynel::Dynel),
    Misc(misc::Misc),
    /// `ChatTextIIR_t` / `FeedbackIIR_t` / `FormatFeedbackIIR_t`.
    Chat(chat::N3Chat),
    /// Id not decoded by any module (name in [`outgoing::REGISTRY`]); the raw body is kept.
    Unknown(Vec<u8>),
}

/// One received N3 frame: header, frame `sender` (the acting dynel, 1 = server) and the decoded body.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub header: N3Header,
    pub sender: u32,
    pub body: N3,
}

/// Decode a `ptype` 0xA frame. `Err` for a malformed/truncated body of a known id.
pub fn decode(f: &crate::frame::Frame) -> Result<Message> {
    if f.ptype != crate::frame::PT_N3 {
        bail!("not an N3 frame (ptype {:#x})", f.ptype);
    }
    let (h, mut r) = N3Header::parse(&f.payload)?;
    let body = if let Some(m) = world::decode(&h, &mut r)? {
        N3::World(m)
    } else if let Some(m) = dynel::decode(&h, &mut r)? {
        N3::Dynel(m)
    } else if let Some(m) = misc::decode(&h, &mut r)? {
        N3::Misc(m)
    } else if let Some(m) = chat::decode(&h, &mut r)? {
        N3::Chat(m)
    } else {
        N3::Unknown(f.payload[13..].to_vec())
    };
    Ok(Message { header: h, sender: f.sender, body })
}

/// Test helper: the live capture as `(ms since probe start, sent?, raw decoded frame bytes)`.
#[cfg(test)]
pub(crate) fn capture() -> Vec<(u32, bool, Vec<u8>)> {
    include_str!("../../../../docs/captures/zone_ithaca.rec")
        .lines()
        .map(|l| {
            let mut p = l.split(' ');
            let ms = p.next().unwrap().parse().unwrap();
            let sent = p.next().unwrap() == ">";
            let hex = p.next().unwrap();
            let b = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            (ms, sent, b)
        })
        .collect()
}

/// Test helper: received N3 frames `(frame, payload)` of the capture.
#[cfg(test)]
pub(crate) fn capture_n3() -> Vec<crate::frame::Frame> {
    capture()
        .into_iter()
        .filter(|(_, sent, _)| !sent)
        .filter_map(|(_, _, b)| crate::frame::Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f))
        .filter(|f| f.ptype == 0xA)
        .collect()
}

/// Test helper: received N3 frames of `docs/captures/zone_newchar_ithaca.rec` (first seconds of a freshly created character).
#[cfg(test)]
pub(crate) fn capture_new_char() -> Vec<crate::frame::Frame> {
    include_str!("../../../../docs/captures/zone_newchar_ithaca.rec")
        .lines()
        .filter_map(|l| {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            (dir == "<").then(|| crate::frame::Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
        })
        .filter(|f| f.ptype == crate::frame::PT_N3)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_captured_n3_message_has_a_header() {
        let frames = capture_n3();
        assert!(frames.len() > 900);
        for f in &frames {
            let (h, _) = N3Header::parse(&f.payload).unwrap();
            assert!(h.target.kind > 0, "{h:?}");
        }
    }

    #[test]
    fn every_captured_message_decodes() {
        for f in capture_n3().iter().chain(capture_new_char().iter()) {
            let m = decode(f).unwrap_or_else(|e| panic!("{:08X}: {e:#}", u32::from_be_bytes(f.payload[..4].try_into().unwrap())));
            assert!(!matches!(m.body, N3::Unknown(_)), "undecoded {:08X}", m.header.msg_type);
        }
    }

    #[test]
    fn new_character_starts_in_playfield_4604() {
        let first = capture_new_char().into_iter().map(|f| decode(&f).unwrap()).find_map(|m| match m.body {
            N3::World(world::World::Playfield(p)) => Some(p),
            _ => None,
        });
        let p = first.unwrap();
        assert_eq!((p.playfield_id, p.rdb_playfield().unwrap().instance), (4604, 4604));
        assert!((p.position[0] - 205.2).abs() < 0.1 && (p.position[2] - 255.9).abs() < 0.2, "{:?}", p.position);
    }

    /// The CharInPlay frame the app sent in a live session (sequence 2, after ZoneLogin) is what `outgoing` builds.
    #[test]
    fn char_in_play_matches_the_live_session() {
        let rec = include_str!("../../../../docs/captures/zone_enter_ithaca.rec");
        let sent: Vec<Vec<u8>> = rec
            .lines()
            .filter_map(|l| l.split_once(' ')?.1.strip_prefix("> "))
            .map(|h| (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect())
            .collect();
        let live = sent.iter().find(|b| b[2..4] == [0, 0xA]).unwrap();
        let mut f = outgoing::n3_frame(0, 0x82e8, outgoing::char_in_play(0x82e8));
        f.seq = 2;
        assert_eq!(&f.encode().unwrap(), live);
    }

    #[test]
    fn playfield_message_header() {
        let f = capture_n3().into_iter().find(|f| f.payload[..4] == 0x5F4B1A39u32.to_be_bytes()).unwrap();
        let (h, _) = N3Header::parse(&f.payload).unwrap();
        assert_eq!((h.target.kind, h.target.instance, h.flag), (0x9C50, 4582, 0));
    }
}
