//! Anarchy Online client <-> login-server protocol: transport framing, login key exchange
//! (DH + TEA-CBC), the system messages from connect through character list / select / zone
//! hand-off, and a threaded login/zone client ([`client`]). See docs/protocol.md.

pub mod client;
pub mod conn;
pub mod crypto;
pub mod frame;
pub mod msg;
pub mod n3;
mod wire;
