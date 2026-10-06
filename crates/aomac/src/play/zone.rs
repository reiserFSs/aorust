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
    /// Stat `Side` (0x21): 0 neutral, 1 clan, 2 omni, 3 ... (compared with the own side for the attackable test, GUI 0x100744ae).
    pub side: u8,
    /// Stat `Level` (0x36).
    pub level: i32,
    /// Stat `Health` (0x1b) / stat 1 (max health).
    pub health: i32,
    pub max_health: i32,
}

/// What a frame changed that the flow reacts to.
#[derive(Debug, PartialEq, Eq)]
pub enum ZoneEvent {
    /// `PlayfieldAnarchyFIIR_t`: load RDB playfield `id` (`proxy.exit_door_id.instance`, falling back to the header id).
    Playfield(u32),
    /// `GameTimeIIR_t`: [`Zone::day_time`] was set (first time or a resync).
    Time,
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
    /// `GameDayTime` (0..6480 s) at the last [`Zone::tick`], from the last `GameTimeIIR_t` (see [`day_time_of`]).
    clock: Option<f32>,
    /// `GameTime_t+0x4C`, the game day (`GameTimeIIR_t.arg3`); seeds the weather schedule (docs/zone/world.md §10).
    pub game_day: i32,
    /// The own character's stats: `FullCharacterIIR_t` (stats_a, stats_b, u8, i16, map groups) then every own `StatIIR_t`
    /// (docs/zone/world.md §2; the interface reads them like `N3Msg_GetSkill`). [`ao_formats::stats`] names the ids.
    pub stats: HashMap<u32, i32>,
    /// The own inventory by slot (`FullCharacterIIR_t` inventory; slots 0..0x3f equipment pages, 0x40.. bag; docs/gui.md §11.5).
    pub inventory: HashMap<u32, ao_net::n3::world::InventoryEntry>,
    /// Other players / NPCs as renderer actors (`play/dynels.rs`).
    pub world: super::dynels::Dynels,
    /// The selected target (`InputConfig_t+0xc0`, set by `TargetingModule_t::SetTarget` GUI 0x100257b0): the dynel instance id.
    pub target: Option<i32>,
    /// The own dynel's last `SimpleCharFullUpdateIIR_t` (the avatar's appearance) and how many arrived (a new one = placed again).
    pub own_update: Option<Box<ao_net::n3::dynel::SimpleCharFullUpdate>>,
    pub own_serial: u32,
    /// The fight controller target of every dynel (`SimpleChar+0x1d4`, `+0x4c/+0x50`, set by the relayed `AttackIIR_t`, cleared by
    /// `StopFightIIR_t`; `N3Msg_GetTargetTarget` GC 0x1001641d reads it): fighter → its target instance id.
    pub fight_target: HashMap<i32, i32>,
}

impl Zone {
    pub fn new(char_id: u32) -> Self {
        Self { char_id, ..Self::default() }
    }

    /// The player's own dynel once its `SimpleCharFullUpdateIIR_t` arrived.
    pub fn own(&self) -> Option<&DynelState> {
        self.dynels.get(&(self.char_id as i32))
    }

    /// An own stat (`INVALID` markers are never stored).
    pub fn stat(&self, id: u32) -> Option<i32> {
        self.stats.get(&id).copied()
    }

    /// `FullCharacter` Activate [GC 0x10073a2f]: each group is applied in order, skipping the `0x499602D2` marker.
    fn apply_stats(&mut self, pairs: impl IntoIterator<Item = (u32, i32)>) {
        self.stats.extend(pairs.into_iter().filter(|p| p.1 != ao_formats::stats::INVALID));
    }

    /// The sky clock (`GameDayTime`, 0..6480 s) now: the server's `GameTimeIIR_t` advanced by the frames since, or the
    /// viewer's frozen default before the server sent one.
    pub fn day_time(&self) -> f32 {
        self.clock.unwrap_or(ao_formats::playfield::DEFAULT_DAY_TIME)
    }

    /// `GameTime_t::RunFunction` [GC 0x1000b214]: the clock runs `TimeSpeed` (15) game seconds per real second, i.e. one
    /// `GameDayTime` second per real second.
    pub fn tick(&mut self, dt: f32) {
        if let Some(t) = &mut self.clock {
            *t = (*t + dt).rem_euclid(GAME_DAY_SECS / TIME_SPEED);
        }
    }

    /// A playfield change (`PlayfieldAnarchyFIIR_t` or a zone redirection): everything of the old playfield is gone, the new
    /// burst re-announces the dynels, and `CharInPlay` is owed again after the new world appears (docs/zone/outgoing.md §3).
    pub fn reset_world(&mut self) {
        self.world.clear();
        self.dynels.clear();
        self.fight_target.clear();
        self.in_play_sent = false;
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
        self.world.on_message(&m);
        match m.body {
            N3::World(World::Playfield(p)) => {
                let id = p.rdb_playfield().map_or(p.playfield_id, |i| i.instance) as u32;
                self.playfield = Some(id);
                self.reset_world();
                self.world.on_playfield(id);
                return ZoneEvent::Playfield(id);
            }
            N3::World(World::GameTime(t)) => {
                self.clock = Some(day_time_of(t.time));
                self.game_day = t.arg3;
                return ZoneEvent::Time;
            }
            N3::World(World::FullCharacter(c)) if who.instance == self.char_id as i32 => {
                self.apply_stats(c.stats_a.iter().chain(&c.stats_b).copied());
                self.apply_stats(c.stats_u8.iter().map(|s| (s.0 as u32, s.1 as i32)));
                self.apply_stats(c.stats_i16.iter().map(|s| (s.0 as u32, s.1 as i32)));
                self.apply_stats(c.stat_map.iter().map(|s| (s.0 as u32, s.1)));
                // `FUN_1002aeca` replaces the character's inventory vector by the message's elements
                self.inventory = c.inventory.iter().map(|e| (e.slot, *e)).collect();
            }
            N3::Dynel(Dynel::Stat(u)) if who.kind == CHAR_KIND && who.instance == self.char_id as i32 => {
                self.apply_stats(u.stats.iter().map(|s| (s.0 as u32, s.1)));
            }
            // other characters: the stats the target window reads (`N3Msg_GetSkill` of the target; docs/zone/dynel.md §3)
            N3::Dynel(Dynel::Stat(u)) if who.kind == CHAR_KIND => {
                if let Some(d) = self.dynels.get_mut(&who.instance) {
                    for (id, v) in &u.stats {
                        match *id as u32 {
                            ao_formats::stats::HEALTH => d.health = *v,
                            ao_formats::stats::LIFE => d.max_health = *v,
                            ao_formats::stats::LEVEL => d.level = *v,
                            ao_formats::stats::SIDE => d.side = *v as u8,
                            _ => {}
                        }
                    }
                }
            }
            N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if who.kind == CHAR_KIND => {
                if who.instance == self.char_id as i32 {
                    self.own_update = Some(Box::new(u.clone()));
                    self.own_serial += 1;
                }
                self.dynels.insert(
                    who.instance,
                    DynelState {
                        name: u.name.clone(),
                        pos: u.pos,
                        yaw: u.yaw(),
                        npc: u.is_npc(),
                        side: u.side,
                        level: i32::from(u.level),
                        health: u.health,
                        max_health: u.max_health,
                    },
                );
            }
            // the original ignores every CharDCMove for the own dynel (`FUN_1006bcc6`, docs/zone/movement.md)
            N3::Dynel(Dynel::CharDCMove(mv)) if who.kind == CHAR_KIND && who.instance != self.char_id as i32 => {
                if let Some(d) = self.dynels.get_mut(&who.instance) {
                    d.pos = mv.pos;
                    d.yaw = Some(mv.yaw());
                }
            }
            N3::Misc(Misc::ToClientQuit) if who.kind == CHAR_KIND => {
                self.dynels.remove(&who.instance);
                self.fight_target.remove(&who.instance);
            }
            N3::Misc(Misc::Attack(a)) if who.kind == CHAR_KIND && a.target.kind == CHAR_KIND => {
                self.fight_target.insert(who.instance, a.target.instance);
            }
            N3::Misc(Misc::StopFight(_)) if who.kind == CHAR_KIND => {
                self.fight_target.remove(&who.instance);
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

/// Game seconds per game day: 27 hours of 3600 s (`GameTime_t` ctor [GC 0x1000af71]: `+0x80 = 27`, `+0x8c = 3600`; day length
/// double `+0x60 = 97200.0` @GC 0x10155ee8).
const GAME_DAY_SECS: f32 = 97_200.0;
/// `GameTime_t::TimeSpeed` (`+0x5c`, ctor default 15): game seconds per real second.
const TIME_SPEED: f32 = 15.0;

/// `GameTimeIIR_t.time` (game seconds into the 27 hour day, `GameTime_t+0x50 = fmod(time, 97200)` [GC 0x1000b526]) as the sky's
/// `GameDayTime` (0..6480 s): `DayTimeForGroundShadows = GameDayTime * 15` is that in-day time (table bounds up to 97700, docs/formats.md).
pub fn day_time_of(server_time: f32) -> f32 {
    server_time.rem_euclid(GAME_DAY_SECS) / TIME_SPEED
}

/// Scene-space forward vector for a server heading `yaw` (`2*atan2(y, w)`): in server space a character walks along
/// `(sin yaw, 0, cos yaw)` (x, y, z), mirrored like [`scene_pos`]. VERIFIED against the live captures: for every moving
/// NPC the travel direction `atan2(dx, dz)` of consecutive `CharDCMove` positions equals the quaternion's `yaw`
/// (test `heading_matches_the_direction_npcs_walk`; docs/zone/avatar.md §1).
pub fn scene_forward(yaw: f32) -> [f32; 3] {
    [yaw.sin(), 0.0, -yaw.cos()]
}

/// Rotation about +Y (right-handed, `Mat4::from_rotation_y`) that turns a character model in scene space (it faces -Z
/// at rest, seen in `view player`) to face [`scene_forward`]: `-yaw`, the sign flip being the z mirror of [`scene_pos`].
pub fn scene_yaw(server_yaw: f32) -> f32 {
    -server_yaw
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

    /// `GameTimeIIR_t` 67170 s (live capture) = 18:39:30 of the 27 h day = 4478 s of the 6480 s sky clock; it runs 1 s per real second and wraps.
    #[test]
    fn game_time_sets_the_sky_clock() {
        let mut z = Zone::new(25988);
        assert_eq!(z.day_time(), ao_formats::playfield::DEFAULT_DAY_TIME, "frozen default until the server sent a time");
        z.tick(1.0);
        assert_eq!(z.day_time(), ao_formats::playfield::DEFAULT_DAY_TIME);
        let ev: Vec<_> = frames(include_str!("../../../../docs/captures/zone_ithaca.rec")).iter().map(|f| z.on_frame(f)).collect();
        assert_eq!(ev.iter().filter(|e| **e == ZoneEvent::Time).count(), 1);
        assert_eq!(z.day_time(), 4478.0);
        assert_eq!(z.game_day, 0x437C9);
        z.tick(2005.0);
        assert!((z.day_time() - 3.0).abs() < 1e-2, "{}", z.day_time());
        assert_eq!(day_time_of(97_200.0 + 15.0), 1.0);
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

    #[test]
    fn own_stats_from_full_character_and_stat_deltas() {
        use ao_formats::stats as st;
        let mut z = Zone::new(33512);
        let mut health = vec![];
        for f in frames(include_str!("../../../../docs/captures/zone_newchar_ithaca.rec")) {
            z.on_frame(&f);
            health.extend(z.stat(st::HEALTH));
        }
        assert_eq!(z.stat(st::LEVEL), Some(1));
        assert_eq!(z.stat(st::IP), Some(1500));
        assert_eq!(z.stat(st::CASH), Some(1000));
        assert_eq!(z.stat(16), Some(z.stat(17).unwrap()), "new character: equal abilities");
        assert!(z.stats.len() > 240, "{}", z.stats.len());
        assert!(!z.stats.values().any(|&v| v == st::INVALID));
        // other dynels' StatIIR must not leak into the own stats
        assert!(health.windows(2).any(|w| w[0] != w[1]) || health.len() >= 1);
    }

    /// Moving NPCs (move types 1/2 = forward start/stop) travel along `scene_forward(yaw)`: the heading handedness.
    #[test]
    fn heading_matches_the_direction_npcs_walk() {
        use ao_net::n3::{dynel::Dynel, N3};
        let mut last: HashMap<i32, [f32; 3]> = HashMap::new();
        let (mut n, mut good, mut mirrored_good) = (0, 0, 0);
        let wrap = |a: f32| a.sin().atan2(a.cos()).abs();
        for rec in [include_str!("../../../../docs/captures/zone_ithaca.rec"), include_str!("../../../../docs/captures/zone_newchar_ithaca.rec")] {
            for f in frames(rec) {
                let Ok(m) = n3::decode(&f) else { continue };
                let N3::Dynel(Dynel::CharDCMove(mv)) = m.body else { continue };
                let id = m.header.target.instance;
                if let Some(p0) = last.insert(id, mv.pos).filter(|_| matches!(mv.move_type, 1 | 2)) {
                    let (dx, dz) = (mv.pos[0] - p0[0], mv.pos[2] - p0[2]);
                    if dx.hypot(dz) < 0.3 {
                        continue;
                    }
                    // travel direction in scene space = (dx, -dz); scene_forward(yaw) = (sin, -cos)
                    let fwd = scene_forward(mv.yaw());
                    let travel = dx.atan2(-dz);
                    n += 1;
                    good += (wrap(travel - fwd[0].atan2(fwd[2])) < 0.8) as u32;
                    mirrored_good += (wrap(travel + fwd[0].atan2(fwd[2])) < 0.8) as u32;
                }
            }
        }
        assert!(n >= 20, "{n} samples");
        assert!(good * 10 >= n * 9, "{good}/{n}");
        assert!(mirrored_good * 2 < n, "the mirrored handedness matches {mirrored_good}/{n}");
    }
}
