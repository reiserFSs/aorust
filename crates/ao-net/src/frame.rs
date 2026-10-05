//! Transport framing: `Connection_t::Send` (Connection.dll 0x100016f3) and
//! `Connection_t::Receive` (Connection.dll 0x100019ba). All ints big-endian.
//!
//! ```text
//! off 0  u16 seq        per-direction counter, first frame = 1
//! off 2  u16 ptype      1 system, 5 text, 10 N3, 0xB ping, 0xE operator, 0x7F compression
//! off 4  u16 version    always 1 on send, ignored on receive
//! off 6  u16 size       header + payload (unpadded)
//! off 8  u32 sender
//! off 12 u32 receiver
//! off 16 payload        zero-padded on the wire to a multiple of 4
//! ```

use anyhow::{bail, Result};

pub const HEADER_LEN: usize = 16;
pub const PT_SYSTEM: u16 = 1;
pub const PT_COMPRESSION: u16 = 0x7F;
/// Receive-side hard limit in `Connection_t::Receive`; the size field itself is u16.
pub const MAX_SIZE: usize = 0xFFFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u16,
    pub ptype: u16,
    pub sender: u32,
    pub receiver: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let size = HEADER_LEN + self.payload.len();
        if size > MAX_SIZE {
            bail!("frame too large: {size}");
        }
        let mut v = Vec::with_capacity((size + 3) & !3);
        v.extend_from_slice(&self.seq.to_be_bytes());
        v.extend_from_slice(&self.ptype.to_be_bytes());
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&(size as u16).to_be_bytes());
        v.extend_from_slice(&self.sender.to_be_bytes());
        v.extend_from_slice(&self.receiver.to_be_bytes());
        v.extend_from_slice(&self.payload);
        v.resize((size + 3) & !3, 0);
        Ok(v)
    }

    /// Parse one frame from the front of `buf`. `Ok(None)` = need more bytes.
    /// Returns the frame and the number of bytes consumed (including padding).
    pub fn decode(buf: &[u8]) -> Result<Option<(Frame, usize)>> {
        if buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let u16at = |o: usize| u16::from_be_bytes([buf[o], buf[o + 1]]);
        let u32at = |o: usize| u32::from_be_bytes(buf[o..o + 4].try_into().unwrap());
        let size = u16at(6) as usize;
        if size < HEADER_LEN {
            bail!("frame size {size} < header");
        }
        let padded = (size + 3) & !3;
        if buf.len() < padded {
            return Ok(None);
        }
        let f = Frame {
            seq: u16at(0),
            ptype: u16at(2),
            sender: u32at(8),
            receiver: u32at(12),
            payload: buf[HEADER_LEN..size].to_vec(),
        };
        Ok(Some((f, padded)))
    }

    /// System-message payload = `u32 msg_type` + body (Message_t::HeaderSize(1) == 0x14).
    pub fn system(seq: u16, sender: u32, receiver: u32, msg_type: u32, body: &[u8]) -> Frame {
        let mut payload = Vec::with_capacity(4 + body.len());
        payload.extend_from_slice(&msg_type.to_be_bytes());
        payload.extend_from_slice(body);
        Frame { seq, ptype: PT_SYSTEM, sender, receiver, payload }
    }

    /// Split a system frame into `(msg_type, body)`.
    pub fn system_parts(&self) -> Result<(u32, &[u8])> {
        if self.ptype != PT_SYSTEM || self.payload.len() < 4 {
            bail!("not a system frame");
        }
        Ok((u32::from_be_bytes(self.payload[..4].try_into()?), &self.payload[4..]))
    }
}

/// Receive-side sequence check, exactly as `Connection_t::Receive` (Connection.dll 0x100019ba).
/// Sequence numbers must strictly increase from 0; after 0xFFFF any non-zero value is accepted.
#[derive(Default)]
pub struct RecvSeq(u16);

impl RecvSeq {
    pub fn accept(&mut self, ptype: u16, seq: u16) -> bool {
        let compression_magic = ptype == PT_COMPRESSION && seq == 0xDFDF;
        if !compression_magic && seq <= self.0 && (self.0 != 0xFFFF || seq == 0) {
            return false;
        }
        if ptype != PT_COMPRESSION {
            self.0 = seq;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hand-derived from the layout above: UserLogin-sized body padded to 4.
    #[test]
    fn encode_known_bytes() {
        let f = Frame::system(1, 0, 1, 0x22, &[0xAA, 0xBB, 0xCC]);
        assert_eq!(
            f.encode().unwrap(),
            [
                0, 1, 0, 1, 0, 1, 0, 23, // seq, ptype, version, size=16+4+3
                0, 0, 0, 0, 0, 0, 0, 1, // sender, receiver
                0, 0, 0, 0x22, 0xAA, 0xBB, 0xCC, 0 // msg type, body, pad
            ]
        );
    }

    #[test]
    fn roundtrip_and_partial() {
        let f = Frame::system(7, 1, 0x2B3F, 0x24, &[9; 32]);
        let b = f.encode().unwrap();
        assert_eq!(b.len(), 52);
        assert!(Frame::decode(&b[..51]).unwrap().is_none());
        let (g, n) = Frame::decode(&b).unwrap().unwrap();
        assert_eq!(n, 52);
        assert_eq!(g, f);
        assert_eq!(g.system_parts().unwrap(), (0x24, &[9u8; 32][..]));
    }

    #[test]
    fn seq_rules() {
        let mut r = RecvSeq::default();
        assert!(!r.accept(1, 0));
        assert!(r.accept(1, 1));
        assert!(!r.accept(1, 1));
        assert!(r.accept(1, 0xFFFF));
        assert!(!r.accept(1, 0));
        assert!(r.accept(1, 5));
    }

    #[test]
    fn rejects_small_size() {
        let mut b = vec![0u8; 16];
        b[7] = 8;
        assert!(Frame::decode(&b).is_err());
    }
}
