//! Big-endian primitives matching `BinaryStream` (BinaryStream.dll, stream endian = big,
//! machine endian = little => byte swap on every int/short; see docs/protocol.md).

use anyhow::{bail, Result};

#[derive(Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            bail!("short read: need {n}, have {}", self.remaining());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.bytes(8)?.try_into()?))
    }
    /// IEEE-754 single, big-endian on the wire.
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    /// Fixed-width NUL-padded ASCII field (UserLogin name/version).
    pub fn fixed_str(&mut self, n: usize) -> Result<String> {
        let b = self.bytes(n)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
    /// `i32` length + bytes (CharacterInfo_c name/area, UserCredentials response). Trailing NULs trimmed.
    pub fn str_i32(&mut self, max: usize) -> Result<String> {
        let n = self.i32()?;
        if n < 0 || n as usize > max {
            bail!("string length {n} out of range (max {max})");
        }
        let b = self.bytes(n as usize)?;
        Ok(String::from_utf8_lossy(b).trim_end_matches('\0').to_owned())
    }
    /// `i16` length + bytes (`FUN_10005d97` `[IF]`): 0 -> "", >= 0x8000 -> "" and the stream is flagged bad (we error).
    pub fn str_i16(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        if n >= 0x8000 {
            bail!("i16 string length {n:#x} invalid");
        }
        Ok(String::from_utf8_lossy(self.bytes(n)?).trim_end_matches('\0').to_owned())
    }
}

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    pub fn i16(&mut self, v: i16) {
        self.u16(v as u16);
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    /// `strncpy(dst, s, n)` semantics into a zeroed `n`-byte field.
    pub fn fixed_str(&mut self, s: &str, n: usize) {
        let b = s.as_bytes();
        let k = b.len().min(n);
        self.0.extend_from_slice(&b[..k]);
        self.0.resize(self.0.len() + (n - k), 0);
    }
    pub fn str_i32(&mut self, s: &str) {
        self.i32(s.len() as i32);
        self.bytes(s.as_bytes());
    }
    /// `i16` length + bytes (`FUN_10004a34`), length clamped to 0xFFFF.
    pub fn str_i16(&mut self, s: &str) {
        let b = &s.as_bytes()[..s.len().min(0xFFFF)];
        self.u16(b.len() as u16);
        self.bytes(b);
    }
}
