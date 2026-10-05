//! Blocking framed TCP connection: sequence numbers, buffering, receive validation.

use crate::frame::{Frame, RecvSeq, PT_SYSTEM};
use crate::msg::{Message, USER_CREDENTIALS, ZONE_INFO, ZONE_LOGIN};
use anyhow::{anyhow, bail, Result};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Wire tap: `(sent, exact bytes)`. UserCredentials bodies are redacted before the tap sees them.
pub type Tap = Box<dyn FnMut(bool, &[u8]) + Send>;

pub struct Conn {
    stream: TcpStream,
    rx: Vec<u8>,
    tx_seq: u16,
    rx_seq: RecvSeq,
    pub tap: Option<Tap>,
}

impl Conn {
    pub fn connect(addr: SocketAddr) -> Result<Conn> {
        let s = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
            .map_err(|e| anyhow!("connect {addr}: {e}"))?;
        Ok(Conn::from_stream(s))
    }

    pub fn from_stream(stream: TcpStream) -> Conn {
        let _ = stream.set_nodelay(true);
        Conn { stream, rx: Vec::new(), tx_seq: 0, rx_seq: RecvSeq::default(), tap: None }
    }

    /// Assign the next sequence number (pre-increment, first frame = 1) and send.
    pub fn send(&mut self, mut f: Frame) -> Result<()> {
        self.tx_seq = self.tx_seq.wrapping_add(1);
        f.seq = self.tx_seq;
        let bytes = f.encode()?;
        if let Some(tap) = &mut self.tap {
            let shown = redact(&f, true).and_then(|r| r.encode().ok()).unwrap_or_else(|| bytes.clone());
            tap(true, &shown);
        }
        self.stream.write_all(&bytes)?;
        Ok(())
    }

    pub fn send_message(&mut self, m: &Message) -> Result<()> {
        self.send(m.to_frame(0))
    }

    /// Wait up to `wait` for one frame. `Ok(None)` = timeout; `Err` = closed / protocol violation.
    pub fn recv(&mut self, wait: Duration) -> Result<Option<Frame>> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some((f, used)) = Frame::decode(&self.rx)? {
                if let Some(tap) = &mut self.tap {
                    let shown = redact(&f, false).and_then(|r| r.encode().ok());
                    tap(false, shown.as_deref().unwrap_or(&self.rx[..used]));
                }
                self.rx.drain(..used);
                if !self.rx_seq.accept(f.ptype, f.seq) {
                    bail!("frame sequence violation (seq {})", f.seq);
                }
                return Ok(Some(f));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.stream.set_read_timeout(Some(left))?;
            let mut buf = [0u8; 4096];
            match self.stream.read(&mut buf) {
                Ok(0) => bail!("connection closed by peer"),
                Ok(n) => self.rx.extend_from_slice(&buf[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Ok(None)
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// Copy of a frame with secrets replaced by `*` (same length): the UserCredentials response, the ZoneInfo
/// cookies (received) and the ZoneLogin cookies (sent).
fn redact(f: &Frame, sent: bool) -> Option<Frame> {
    if f.ptype != PT_SYSTEM || f.payload.len() < 4 {
        return None;
    }
    let id = u32::from_be_bytes(f.payload[..4].try_into().ok()?);
    let range = match (sent, id) {
        (true, USER_CREDENTIALS) if f.payload.len() >= 4 + 44 => 48..f.payload.len(),
        (true, ZONE_LOGIN) if f.payload.len() >= 16 => 8..16,
        (false, ZONE_INFO) if f.payload.len() >= 22 => 14..22,
        _ => return None,
    };
    let mut r = f.clone();
    r.payload[range].fill(b'*');
    Some(r)
}
