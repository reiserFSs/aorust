//! Zone-server N3 messages (`ptype == 0xA`). The frame payload is the N3 message handed to
//! `n3EngineClient_t::ToClientN3Message(Identity_t const&, ACE_Data_Block*)`: `u32 msg_type`,
//! an [`Identity`], one flag byte, then the type-specific body (layouts: docs/zone.md).
//! Fixtures come from the sanitized live capture `docs/captures/zone_ithaca.rec`.

pub mod dynel;
pub mod misc;
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
    fn playfield_message_header() {
        let f = capture_n3().into_iter().find(|f| f.payload[..4] == 0x5F4B1A39u32.to_be_bytes()).unwrap();
        let (h, _) = N3Header::parse(&f.payload).unwrap();
        assert_eq!((h.target.kind, h.target.instance, h.flag), (0x9C50, 4582, 0));
    }
}
