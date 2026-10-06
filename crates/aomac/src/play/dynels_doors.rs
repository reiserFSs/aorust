//! Door state and the item animation clock (docs/zone/doors.md). Pure rules, no rendering.
//!
//! * [`ItemAnim`] is `SimpleItem_t`'s animation player: stats `AnimPos` (500), `AnimPlay` (501) and the mesh's animation time, advanced by
//!   `FUN_10088426` [GC 0x10088426] each frame and positioned by `FUN_100872a2` [GC 0x100872a2].
//! * [`Door`] is `Door_t`'s open / lock state: the `Flags` bits 0x80 (open) and 0x40 (locked) and the message handlers
//!   `DoorStatusUpdateIIR_t::Apply` `FUN_1009fc10`, `FUN_1007fdcf` (open), `FUN_1007fe6a` (close), `FUN_1007ff80` / `FUN_1007ef1e` (lock /
//!   unlock). Each returns the [`Effect`]s the original performs besides the animation: sounds and the room monitor calls.
//! * [`PropAnim`] is the glue `dynels.rs` uses: it owns the state of one prop and the pose the renderer holds.

use ao_formats::character::CrtRand;
use ao_formats::dynel_visual::get;
use ao_formats::mesh::NodeRig;
use ao_rdb::RecordStore;
use ao_scene::{Scene, Vertex};
use std::sync::Arc;

/// `AnimPos` (stat 500) values `FUN_100872a2` knows: 1 = animation start (time 0), 2 = animation end (the tree's total time).
pub const POS_START: i32 = 1;
pub const POS_END: i32 = 2;

/// `AnimPlay` (stat 501) values `FUN_10088426` acts on. 1 = idle (also the state after a once-play finished).
pub const PLAY_IDLE: i32 = 1;
/// Forward, never ends (vending machines, templates with `AnimPlay` 2: the clock just keeps running).
pub const PLAY_FORWARD: i32 = 2;
/// Forward until the animation's total time, then park at the end.
pub const PLAY_FORWARD_ONCE: i32 = 3;
/// Backward, never ends.
pub const PLAY_BACKWARD: i32 = 4;
/// Backward until time 0, then park at the start.
pub const PLAY_BACKWARD_ONCE: i32 = 5;

/// The item animation clock of one dynel.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemAnim {
    /// `AnimPos` stat.
    pub pos: i32,
    /// `AnimPlay` stat.
    pub play: i32,
    /// `VisualMesh_t::GetAnimationTime`.
    pub time: f32,
    /// `VisualMesh_t::GetAnimationTreeTotalTime`: the longest node animation of the mesh.
    pub total: f32,
    /// `this+0x194`: set the frame a once-play ended.
    pub done: bool,
}

impl ItemAnim {
    /// A freshly loaded mesh: animation time 0 and the item's `AnimPos` / `AnimPlay` stats (0 when absent).
    pub fn new(total: f32, pos: i32, play: i32) -> Self {
        Self { pos, play, time: 0.0, total, done: false }
    }

    /// `FUN_100872a2(pos)`: stores `AnimPos` and positions the clock (1 = start, 2 = end; other values leave the time alone).
    pub fn set_pos(&mut self, pos: i32) {
        self.pos = pos;
        match pos {
            POS_START => self.time = 0.0,
            POS_END => self.time = self.total,
            _ => {}
        }
    }

    /// `FUN_10088426`: one frame of `dt` seconds. A once-play that reaches its end parks the clock there (`FUN_100872a2(2)` / time 0) and
    /// returns to [`PLAY_IDLE`].
    pub fn step(&mut self, dt: f32) {
        self.done = false;
        match self.play {
            PLAY_FORWARD => self.time += dt,
            PLAY_FORWARD_ONCE => {
                if self.total <= self.time + dt {
                    self.set_pos(POS_END);
                    self.done = true;
                    self.play = PLAY_IDLE;
                } else {
                    self.time += dt;
                }
            }
            PLAY_BACKWARD => self.time -= dt,
            PLAY_BACKWARD_ONCE => {
                if self.time - dt <= 0.0 {
                    self.time = 0.0;
                    self.done = true;
                    self.play = PLAY_IDLE;
                } else {
                    self.time -= dt;
                }
            }
            _ => {}
        }
    }
}

/// `Flags` bit of an open door (`vtable+0x14(0x80)`).
pub const FLAG_OPEN: u32 = 0x80;
/// `Flags` bit of a locked door (`FUN_100850c1` sets it, `FUN_100850c9` clears it).
pub const FLAG_LOCKED: u32 = 0x40;
/// `StateAction` values `FUN_100850f3` / `FUN_1008513a` store (stat 98): the door's state machine (rdb 1000015) leaves its idle state on 100
/// and returns on 102.
pub const ACTION_OPENED: i32 = 100;
pub const ACTION_CLOSED: i32 = 102;
/// Sound lists of the item's template (`{0x14, 5}` element, key -> Sandy sound ids): opening and closing, each with a generic fallback
/// key (`FUN_1007fdcf` @0x1007fdcf: `0x83`, else `0x64`; `FUN_1007fe6a` @0x1007fe6a: `0x84`, else `0x66`).
pub const SOUND_OPEN: [i32; 2] = [0x83, 0x64];
pub const SOUND_CLOSE: [i32; 2] = [0x84, 0x66];
/// Stat 259 (`0x103`, unnamed) bit that stops `FUN_1007fe6a` / `FUN_1007ff50` from closing the door at all.
pub const STAT_103_NO_CLOSE: i32 = 0x40;

/// What the original does besides moving the mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// `PlayGameSound(id, door position)`; the keys are tried in order, the first with a sound wins.
    Sound([i32; 2]),
    /// `n3RoomMonitor_t::DoorOpened` (`FUN_1007ef56`).
    RoomOpened,
    /// `n3RoomMonitor_t::DoorClosed` (`FUN_1007ef90`).
    RoomClosed,
}

/// `Door_t`: the lock / open state, and its animation when the mesh has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Door {
    /// `Flags` (stat 0), of which only [`FLAG_OPEN`] and [`FLAG_LOCKED`] are interpreted.
    pub flags: u32,
    /// Stat 259 (`0x103`).
    pub stat_103: i32,
    /// Stat 98 `StateAction`.
    pub action: i32,
    /// Stat 195 (`0xC3`), set by every status update.
    pub stat_c3: i32,
    /// `+0x1d5`, set by a status update's flag.
    pub flag_1d5: bool,
    /// The mesh animation (`HasMesh` and keyframes); `None` = nothing to animate, the state still follows the messages.
    pub anim: Option<ItemAnim>,
}

impl Door {
    pub fn new(flags: u32, stat_103: i32, anim: Option<ItemAnim>) -> Self {
        Self { flags, stat_103, action: 0, stat_c3: 0, flag_1d5: false, anim }
    }

    pub fn is_open(&self) -> bool {
        self.flags & FLAG_OPEN != 0
    }

    /// `FUN_1007fdcf`: opens (sound, room monitor, then `FUN_100850f3`: flag 0x80, `StateAction` 100, animation 0 -> total).
    pub fn open(&mut self, fx: &mut Vec<Effect>) {
        fx.push(Effect::Sound(SOUND_OPEN));
        fx.push(Effect::RoomOpened);
        self.flags |= FLAG_OPEN;
        self.action = ACTION_OPENED;
        if let Some(a) = &mut self.anim {
            a.set_pos(POS_START);
            a.play = PLAY_FORWARD_ONCE;
        }
    }

    /// `FUN_1007fe6a`: closes unless stat 259 bit 0x40 is set (sound, room monitor, then `FUN_1008513a`: an open door plays its animation
    /// total -> 0; flag 0x80 off, `StateAction` 102).
    pub fn close(&mut self, fx: &mut Vec<Effect>) {
        if self.stat_103 & STAT_103_NO_CLOSE != 0 {
            return;
        }
        fx.push(Effect::Sound(SOUND_CLOSE));
        fx.push(Effect::RoomClosed);
        let open = self.is_open();
        if let (Some(a), true) = (&mut self.anim, open) {
            a.set_pos(POS_END);
            a.play = PLAY_BACKWARD_ONCE;
        }
        self.flags &= !FLAG_OPEN;
        self.action = ACTION_CLOSED;
    }

    /// `FUN_1009fc10` (`DoorStatusUpdateIIR_t` apply): lock (an open door closes first) or unlock, the `+0x1d5` flag, open or close (each
    /// followed by the room monitor call again), `SetStat(0xC3, value)`.
    pub fn status(&mut self, locked: bool, open: bool, value_c3: i32, flag_1a: bool) -> Vec<Effect> {
        let mut fx = vec![];
        if locked {
            if self.is_open() {
                self.close(&mut fx);
            }
            self.flags |= FLAG_LOCKED;
        } else {
            self.flags &= !FLAG_LOCKED;
        }
        self.flag_1d5 |= flag_1a;
        if open {
            self.open(&mut fx);
            fx.push(Effect::RoomOpened);
        } else {
            self.close(&mut fx);
            fx.push(Effect::RoomClosed);
        }
        self.stat_c3 = value_c3;
        fx
    }

    /// One frame (`dt` seconds); returns the animation time of a pose that changed.
    pub fn step(&mut self, dt: f32) -> Option<f32> {
        let a = self.anim.as_mut()?;
        let was = (a.time, a.play);
        a.step(dt);
        (a.time != was.0 || a.play != was.1).then_some(a.time)
    }
}

/// A queued or applied message to a door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// `DoorStatusUpdateIIR_t`: `(locked, open, value_c3, flag_1a)`.
    Status(bool, bool, i32, bool),
    /// A `DoorFullUpdateIIR_t` with `Flags` bit 0x80: `FUN_1009faaf` opens the new door (sound, animation from the start).
    Open,
}

impl Door {
    pub fn apply(&mut self, c: Cmd) -> Vec<Effect> {
        match c {
            Cmd::Status(locked, open, value_c3, flag_1a) => self.status(locked, open, value_c3, flag_1a),
            Cmd::Open => {
                let mut fx = vec![];
                self.open(&mut fx);
                fx
            }
        }
    }
}

/// One `PlayGameSound` of a door or a fight: Sandy sound id and the scene position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameSound {
    pub id: u32,
    pub pos: [f32; 3],
    /// The game material argument (`FabricType` of the struck creature, 7 = flesh of a player, 0 = none) and the impact size 0 / 1 / 2 (1 = plain),
    /// which pick the sound's material variant (`ao_audio::game::variant_of`).
    pub material: i32,
    pub size: i32,
}

impl GameSound {
    /// `PlayGameSound(id, pos, 0, 1.0, 0, 0, 100, 1)`: the plain one-shot of doors and weapon swings.
    pub fn at(id: u32, pos: [f32; 3]) -> GameSound {
        GameSound { id, pos, material: 0, size: 1 }
    }
}

/// The animation data of an item model, built next to the model on the worker thread: the node keyframes (`None` when the mesh has none),
/// the item's stats the door code reads, and the template's sound lists.
pub struct ItemRig {
    pub rig: Arc<NodeRig>,
    pub flags: u32,
    pub stat_103: i32,
    pub pos: i32,
    pub play: i32,
    pub sounds: Vec<(u32, Vec<u32>)>,
}

impl ItemRig {
    /// `None` unless mesh record `mesh` has animated nodes and the static decode `model` has the same vertices.
    pub fn new(store: &RecordStore, mesh: u32, model: &Scene, stats: &[(u32, i32)], sounds: Vec<(u32, Vec<u32>)>) -> Option<ItemRig> {
        let rig = NodeRig::load(store, mesh).ok().flatten()?;
        (model.meshes.first()?.vertices.len() == rig.vertex_count()).then(|| {
            let stat = |id| get(stats, id).unwrap_or(0);
            ItemRig { rig: Arc::new(rig), flags: stat(0) as u32, stat_103: stat(259), pos: stat(500), play: stat(501), sounds }
        })
    }
}

/// The animation side of a prop: the door / item state once its model is in, messages that came before, the vertex set the renderer holds.
#[derive(Default)]
pub struct PropAnim {
    door: Option<Door>,
    early: Vec<Cmd>,
    /// Pose the renderer has (`None` = the rest vertices).
    sent: Option<f32>,
    skin: Vec<Vertex>,
    /// The door's room link state changed since [`PropAnim::take_room_state`] (`n3RoomMonitor_t::DoorOpened/DoorClosed`).
    dirty: bool,
}

/// `FUN_1004570c` (@0x1004570c): a random sound of the first key with a list (the client's `rand() % count`).
fn pick(sounds: &[(u32, Vec<u32>)], keys: [i32; 2], rng: &mut CrtRand) -> Option<u32> {
    let list = keys.iter().find_map(|&k| sounds.iter().find(|s| s.0 == k as u32).map(|s| &s.1).filter(|l| !l.is_empty()))?;
    // the client draws only when there is a choice
    Some(if list.len() > 1 { list[rng.rand() as usize % list.len()] } else { list[0] })
}

impl PropAnim {
    /// A message for the door: applied now, or queued until the model is in.
    pub fn command(&mut self, c: Cmd, item: Option<&ItemRig>, at: [f32; 3], rng: &mut CrtRand, out: &mut Vec<GameSound>) {
        match (&mut self.door, item) {
            (Some(d), Some(item)) => {
                let fx = d.apply(c);
                self.dirty |= fx.iter().any(|e| matches!(e, Effect::RoomOpened | Effect::RoomClosed));
                Self::sounds(&fx, &item.sounds, at, rng, out);
            }
            _ => {
                if self.early.len() < 16 {
                    self.early.push(c);
                }
            }
        }
    }

    fn sounds(fx: &[Effect], list: &[(u32, Vec<u32>)], at: [f32; 3], rng: &mut CrtRand, out: &mut Vec<GameSound>) {
        for e in fx {
            if let Effect::Sound(keys) = e {
                out.extend(pick(list, *keys, rng).map(|id| GameSound::at(id, at)));
            }
        }
    }

    /// One frame of the clock: creates the state when the model arrives (queued messages replay silently), then advances it by `dt`.
    pub fn step(&mut self, dt: f32, item: &ItemRig) {
        let dirty = &mut self.dirty;
        let door = self.door.get_or_insert_with(|| {
            let mut d = Door::new(item.flags, item.stat_103, Some(ItemAnim::new(item.rig.total_time(), item.pos, item.play)));
            for c in self.early.drain(..) {
                d.apply(c);
            }
            *dirty = true; // `Door_t::LinkDoorToRooms` registers the new door with the state it starts in
            d
        });
        door.step(dt);
    }

    /// `(open, passable)` of the door's room link when it changed: `n3RoomMonitor_t::DoorOpened / DoorClosed` (open flag, `Flags` bit
    /// 0x80) and `Door_t::CanPass` for the own character. `FUN_1007f74d` / `FUN_1007f58e` [GC]: an unlocked door (vtable `+0xfc` =
    /// `IsLocked`, `Flags` bit 0x40) lets the character through, open or not; the locked rest (a lock-difficulty of 0 passes,
    /// stat 0x103 bit 0x10 refuses, keys / owned buildings) is ported as "refused".
    pub fn take_room_state(&mut self) -> Option<(bool, bool)> {
        let d = self.door.as_ref().filter(|_| std::mem::take(&mut self.dirty))?;
        Some((d.is_open(), d.flags & FLAG_LOCKED == 0))
    }

    /// The room link state is handed out again (a new collision world was built).
    pub fn resync(&mut self) {
        self.dirty = self.door.is_some();
    }

    /// The vertex set to hand to the renderer when the pose differs from the one it holds (`resubmit`: the renderer forgot the actor, it
    /// holds the rest vertices again).
    pub fn pose(&mut self, item: &ItemRig, rest: &[Vertex], resubmit: bool) -> Option<Vec<Vertex>> {
        let t = self.door.as_ref()?.anim.as_ref().map_or(0.0, |a| a.time);
        let have = if resubmit { None } else { self.sent };
        if (t == 0.0 && have.is_none()) || have == Some(t) {
            self.sent = have;
            return None;
        }
        self.sent = (t != 0.0).then_some(t);
        if t == 0.0 {
            self.skin = Vec::new();
            return Some(rest.to_vec());
        }
        if self.skin.len() != rest.len() {
            self.skin = rest.to_vec();
        }
        item.rig.pose(t, &mut self.skin);
        Some(self.skin.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door(total: f32) -> Door {
        Door::new(0x01, 0, Some(ItemAnim::new(total, 0, 0)))
    }

    fn run(d: &mut Door, secs: f32) {
        let mut t = 0.0;
        while t < secs {
            d.step(1.0 / 60.0);
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn opening_plays_forward_once_and_parks_at_the_end() {
        let mut d = door(0.6);
        let fx = d.status(false, true, 7, false);
        assert_eq!(fx, [Effect::Sound(SOUND_OPEN), Effect::RoomOpened, Effect::RoomOpened]);
        assert!(d.is_open() && d.action == ACTION_OPENED && d.stat_c3 == 7);
        let a = d.anim.clone().unwrap();
        assert_eq!((a.pos, a.play, a.time), (POS_START, PLAY_FORWARD_ONCE, 0.0));
        run(&mut d, 0.3);
        let t = d.anim.as_ref().unwrap().time;
        assert!((t - 0.3).abs() < 0.02, "{t}");
        run(&mut d, 1.0);
        let a = d.anim.as_ref().unwrap();
        assert_eq!((a.time, a.play, a.pos), (0.6, PLAY_IDLE, POS_END));
        assert_eq!(d.step(0.1), None); // parked: nothing changes any more
    }

    #[test]
    fn closing_plays_backward_once_to_time_zero() {
        let mut d = door(0.6);
        d.status(false, true, 0, false);
        run(&mut d, 1.0);
        let fx = d.status(false, false, 0, false);
        assert_eq!(fx, [Effect::Sound(SOUND_CLOSE), Effect::RoomClosed, Effect::RoomClosed]);
        assert!(!d.is_open() && d.action == ACTION_CLOSED);
        assert_eq!((d.anim.as_ref().unwrap().time, d.anim.as_ref().unwrap().play), (0.6, PLAY_BACKWARD_ONCE));
        run(&mut d, 0.3);
        assert!((d.anim.as_ref().unwrap().time - 0.3).abs() < 0.02);
        run(&mut d, 1.0);
        let a = d.anim.as_ref().unwrap();
        assert_eq!((a.time, a.play), (0.0, PLAY_IDLE));
    }

    #[test]
    fn closing_a_closed_door_only_makes_noise_and_moves_nothing() {
        let mut d = door(0.6);
        d.close(&mut vec![]);
        let a = d.anim.as_ref().unwrap();
        assert_eq!((a.time, a.play), (0.0, 0)); // `FUN_1008513a` animates only an open door
    }

    #[test]
    fn locking_closes_an_open_door_first_and_unlocking_clears_the_bit() {
        let mut d = door(0.6);
        d.status(false, true, 0, false);
        let fx = d.status(true, false, 0, false);
        assert_eq!(fx[0], Effect::Sound(SOUND_CLOSE));
        assert!(d.flags & FLAG_LOCKED != 0 && !d.is_open());
        d.status(false, false, 0, true);
        assert!(d.flags & FLAG_LOCKED == 0 && d.flag_1d5);
    }

    #[test]
    fn room_state_follows_open_and_lock_and_is_taken_once() {
        let mut p = PropAnim::default();
        assert_eq!(p.take_room_state(), None);
        p.door = Some(door(0.6));
        p.dirty = true;
        assert_eq!(p.take_room_state(), Some((false, true)));
        assert_eq!(p.take_room_state(), None);
        p.door.as_mut().unwrap().status(false, true, 0, false);
        p.dirty = true;
        assert_eq!(p.take_room_state(), Some((true, true)));
        p.door.as_mut().unwrap().status(true, false, 0, false); // locked: closed, and CanPass refuses
        p.dirty = true;
        assert_eq!(p.take_room_state(), Some((false, false)));
        assert_eq!(p.take_room_state(), None);
        p.resync();
        assert_eq!(p.take_room_state(), Some((false, false)));
    }

    #[test]
    fn stat_103_bit_40_keeps_a_door_from_closing() {
        let mut d = Door::new(0x81, 0x40, Some(ItemAnim::new(0.6, 0, 0)));
        let mut fx = vec![];
        d.close(&mut fx);
        assert!(fx.is_empty() && d.is_open());
    }

    #[test]
    fn a_door_without_animation_still_tracks_its_flags() {
        let mut d = Door::new(0x01, 0, None);
        d.status(false, true, 0, false);
        assert!(d.is_open());
        assert_eq!(d.step(0.1), None);
        d.status(false, false, 0, false);
        assert!(!d.is_open());
    }

    #[test]
    fn looping_item_clock_keeps_running_and_other_states_follow_the_original() {
        let mut a = ItemAnim::new(2.0, 0, PLAY_FORWARD); // a vending machine: AnimPlay 2
        for _ in 0..10 {
            a.step(0.5);
        }
        assert_eq!((a.time, a.play), (5.0, PLAY_FORWARD));
        a.play = PLAY_BACKWARD;
        a.step(1.0);
        assert_eq!(a.time, 4.0);
        a.play = PLAY_IDLE;
        a.step(1.0);
        assert_eq!(a.time, 4.0);
        // AnimPos values other than 1 / 2 leave the clock alone
        a.set_pos(0);
        assert_eq!(a.time, 4.0);
    }
}
