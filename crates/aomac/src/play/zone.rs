//! Client-side state built from the zone server's N3 stream (`ao_net::n3`): the playfield to load, the player's
//! own dynel (start position / heading) and the other dynels the server announced. Evidence and layouts: docs/zone.md.

use ao_net::frame::Frame;
use ao_net::n3::{self, dynel::Dynel, misc::Misc, world::World, N3};
use std::collections::{BTreeMap, HashMap};

/// Identity kind of character / NPC dynels (`SimpleChar_t`).
const CHAR_KIND: i32 = 0xC350;

#[derive(Debug, Clone, PartialEq)]
pub struct DynelState {
    pub name: String,
    /// Server coordinates (Y up), see [`scene_pos`].
    pub pos: [f32; 3],
    /// Heading about Y in radians (`2*atan2(y, w)` of the quaternion), if the server sent a rotation.
    pub yaw: Option<f32>,
    pub npc: bool,
}

/// What a frame changed that the flow reacts to.
#[derive(Debug, PartialEq, Eq)]
pub enum ZoneEvent {
    /// `PlayfieldAnarchyFIIR_t`: load RDB playfield `id` (`proxy.exit_door_id.instance`, falling back to the header id).
    Playfield(u32),
    None,
}

#[derive(Default)]
pub struct Zone {
    pub char_id: u32,
    pub playfield: Option<u32>,
    pub dynels: HashMap<i32, DynelState>,
    /// Frames per message id (`u32` key; system messages and pings are not counted), for diagnostics.
    pub counts: BTreeMap<u32, u32>,
    pub frames: u32,
    pub in_play_sent: bool,
}

impl Zone {
    pub fn new(char_id: u32) -> Self {
        Self { char_id, ..Self::default() }
    }

    /// The player's own dynel once its `SimpleCharFullUpdateIIR_t` arrived.
    pub fn own(&self) -> Option<&DynelState> {
        self.dynels.get(&(self.char_id as i32))
    }

    pub fn on_frame(&mut self, f: &Frame) -> ZoneEvent {
        if f.ptype != ao_net::frame::PT_N3 {
            return ZoneEvent::None;
        }
        self.frames += 1;
        let m = match n3::decode(f) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("zone: undecodable N3 frame ({} bytes): {e:#}", f.payload.len());
                return ZoneEvent::None;
            }
        };
        *self.counts.entry(m.header.msg_type).or_default() += 1;
        let who = m.header.target;
        match m.body {
            N3::World(World::Playfield(p)) => {
                let id = p.rdb_playfield().map_or(p.playfield_id, |i| i.instance) as u32;
                self.playfield = Some(id);
                return ZoneEvent::Playfield(id);
            }
            N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if who.kind == CHAR_KIND => {
                self.dynels.insert(
                    who.instance,
                    DynelState { name: u.name.clone(), pos: u.pos, yaw: u.yaw(), npc: u.is_npc() },
                );
            }
            N3::Dynel(Dynel::CharDCMove(mv)) if who.kind == CHAR_KIND => {
                if let Some(d) = self.dynels.get_mut(&who.instance) {
                    d.pos = mv.pos;
                    d.yaw = Some(mv.yaw());
                }
            }
            N3::Misc(Misc::ToClientQuit) if who.kind == CHAR_KIND => {
                self.dynels.remove(&who.instance);
            }
            _ => {}
        }
        ZoneEvent::None
    }
}

/// Server (AO world) coordinates -> scene coordinates: Y stays up, Z is mirrored. Evidence: playfield 4582's scene
/// bounds span z in [-17885, 0] while the server put the player at z = +742.7 (docs/zone.md §3).
pub fn scene_pos(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

/// Scene-space forward vector for a server heading. [GUESS] a rotation `yaw` about +Y takes the model forward axis
/// +Z to `(sin yaw, 0, cos yaw)` in server space (right-handed rotation formula), mirrored like [`scene_pos`];
/// the handedness is unverified against the client's own camera (docs/zone.md §3).
pub fn scene_forward(yaw: f32) -> [f32; 3] {
    [yaw.sin(), 0.0, -yaw.cos()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::frame::Frame;

    fn frames(rec: &str) -> Vec<Frame> {
        rec.lines()
            .filter_map(|l| {
                let mut p = l.split(' ');
                let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                (dir == "<").then(|| Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
            })
            .collect()
    }

    #[test]
    fn live_capture_gives_playfield_and_own_position() {
        let mut z = Zone::new(25988);
        let mut ev = vec![];
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            ev.push(z.on_frame(&f));
        }
        assert_eq!(ev.iter().filter(|e| **e == ZoneEvent::Playfield(4582)).count(), 1);
        assert_eq!(ev[..3].iter().position(|e| *e == ZoneEvent::Playfield(4582)), Some(2), "after the 0x7F control and the 0x43 system frame");
        // 1.0 s later every dynel announced so far is known; the player is the first SimpleCharFullUpdate with its own id
        let own = z.own().unwrap();
        assert_eq!(own.name, "Testy");
        assert!(own.yaw.is_some());
        assert!(z.dynels.len() > 10, "{}", z.dynels.len());
    }

    #[test]
    fn new_character_start() {
        let mut z = Zone::new(33512);
        for f in frames(include_str!("../../../../docs/captures/zone_newchar_ithaca.rec")) {
            z.on_frame(&f);
        }
        assert_eq!(z.playfield, Some(4604));
        let own = z.own().unwrap();
        assert_eq!(own.name, "Aomacvolk");
        assert!((own.pos[0] - 205.2).abs() < 0.5 && (own.pos[2] - 255.9).abs() < 0.5, "{:?}", own.pos);
        assert_eq!(scene_pos(own.pos)[2], -own.pos[2]);
    }
}
