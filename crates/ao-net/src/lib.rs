//! Pure (socket-free) encode/decode of the Anarchy Online client <-> login-server protocol:
//! transport framing, login key exchange (DH + TEA-CBC) and the system messages from
//! connect through character list / select / zone hand-off. See docs/protocol.md.

pub mod crypto;
pub mod frame;
pub mod msg;
mod wire;
