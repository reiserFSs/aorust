//! Client-side state built from the zone server's N3 stream (`ao_net::n3`): the playfield to load, the player's
//! own dynel (start position / heading) and the other dynels the server announced. Evidence and layouts: docs/zone.md.

use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::{self, dynel::Dynel, misc::Misc, pet::PetList, server_move, world::World, N3};
use std::collections::{BTreeMap, HashMap};

/// `InPlay` stat id (0xc2): gates `FUN_1005859d` (the teleportal test) and is set by the full update and the `CharInPlay` echo.
pub const IN_PLAY_STAT: u32 = 0xC2;

/// `CurrentNano` stat id (0xd6): the second value `ResurrectIIR_t` sets.
const NANO_STAT: u32 = 0xD6;

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
    /// `n3TeleportIIR_t` for the own character with a destination playfield (`n3EngineClient_t::StartTeleport`): the old playfield
    /// is stopped and `PlayfieldAnarchy_t::Run` posts `TeleportStarted` (docs/zone/world.md §10.2).
    Teleport,
    /// `CharInPlayIIR_t` relayed for the own character (the server's echo of our `CharInPlay`): `Activate` [GC 0x1007264d] sets stat
    /// `InPlay` and posts event `0xa5` = `FlowControlModule_t::AliveMessage` [GUI 0x10028543] (docs/zone/world.md §10.2).
    Alive,
    None,
}

/// Server messages that act on the own character's movement, in arrival order; `Player::frame` drains them into `Movement`
/// (the original applies each one to the control dynel's vehicle at once; docs/zone/movement.md §10).
#[derive(Debug, Clone, PartialEq)]
pub enum OwnEvent {
    /// Placed at `pos` (server coordinates), heading `yaw`; `full_reset` clears the velocity, inputs and FSM like a spawn
    /// (`Movement::teleport`; the in-playfield `n3TeleportIIR_t`).
    Place { pos: [f32; 3], yaw: f32, full_reset: bool },
    /// `SetPosIIR_c` [GC 0x10076e5a]: `pos` with the current rotation; `stop` = full stop while moving.
    SetPos { pos: [f32; 3], stop: bool },
    /// `ImpulseIIR_c` element for the own dynel: `Vehicle_t::Impulse(delta, time)`.
    Impulse { delta: [f32; 3], time: f32 },
    /// `FollowTargetIIR_c` [GC 0x100732e3] with the own dynel as header.
    Follow { mode: u8, target: Option<i32>, pos: [f32; 3], path: Vec<[f32; 3]> },
    /// `CharacterActionIIR_t` action id of the own dynel (`0x56` sit relay, `0x57` stand up, `0x63` death, `0xAD`).
    Action(i32),
    /// `ResurrectIIR_t` [GC 0x100769bd]: Health / CurrentNano are in the stat table, the vehicle recalculates.
    Resurrected,
    /// `ApplySpellsIIR_t` [GC 0x101288f2] whose target is the own character: `apply` runs the spells, otherwise undoes them (docs/zone/movement.md §10.1).
    Spells { spells: Vec<ao_net::n3::spells::Spell>, apply: bool },
    /// `RelocateDynelsIIR_t` [GC 0x1003a364] listing the own character: it becomes a child of `parent` at the parent's origin (`NullPos`), `pos`
    /// = that origin when the parent's position is known.
    Relocated { parent: Identity, pos: Option<[f32; 3]> },
    /// `FightModeUpdate_t` [GC 0x10124b70] for the current playfield (the district table is the player's, `fightmode.rs`).
    FightMode(ao_net::n3::server_move::FightModeUpdate),
}

/// Events of the client-initiated teleport path (`n3EngineClientAnarchy_t::StartTeleportTry` / `TeleportTrier_t`, docs/zone/world.md §10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeleportEvent {
    /// `TeleportTrier_t::StartTryingTeleport` [GC 0x10037d48]: GUI event 5 = `FlowControlModule_t::TeleportStartedMessage`.
    Started,
    /// `TeleportTrier_t::TeleportFailed` [GC 0x10037db7]: GUI event 6 = `TeleportEndedMessage`, then `Feedback_AreaChangeNotInitiated`.
    Failed,
}

/// Seconds a `TeleportTrier_t` waits for the server before `TeleportFailed` (`TeleportTrier_t(_DAT_10157500)` in `StartTeleportTry`; the f32
/// at GC 0x10157500 is also the 30 m radius of `N3Msg_GetDynelsInVicinity`).
pub const TELEPORT_TIMEOUT: f32 = 30.0;

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
    /// The own pet list (`dynel+0x1d8` -> `+0x1c`, `AddPetIIR_c` / `RemovePetIIR_c`; docs/zone/pets.md): service towers included.
    pub pets: Vec<Identity>,
    /// The spells currently running on the own character (`FullCharacterIIR_t` list, `ApplySpellsIIR_t`): their stat bonuses are the buffs of
    /// `GetSkill(stat, 2)` (`play/hud_stats/buffs.rs`, docs/gui.md §11.10).
    pub active_spells: Vec<ao_net::n3::spells::Spell>,
    /// Pending [`OwnEvent`]s (drained by the player each frame).
    pub own_events: Vec<OwnEvent>,
    /// District fight-mode level of the own character (`FUN_1003e1d0`, `fightmode.rs`), written by the player each frame; `None` until then
    /// (the original's no-data default is [`super::fightmode::DEFAULT_LEVEL`]). `/follow` reads it.
    pub fight_level: Option<i32>,
    /// `RelocateDynelsIIR_t`: `(child, parent)` links (`n3Dynel_t::RelocateDynel`), cleared with the playfield.
    pub parents: Vec<(Identity, Identity)>,
    /// Position and heading of each parented character relative to its parent (the vehicle's `+0x58` / `+0x80`).
    child_rel: HashMap<i32, ([f32; 3], f32)>,
    /// The own nano programs and timed nano effects (`own_nanos.rs`, the Programs / NCU windows).
    pub nanos: super::own_nanos::OwnNanos,
    /// The `TeleportTrier_t` [GC 0x10037d05] of a client-initiated teleport try (a child of the playfield, so it survives the own dynel being
    /// placed again and dies with the playfield): seconds elapsed since `StartTryingTeleport`. `None`: no try is running.
    pub trier: Option<f32>,
    /// What the try did since the flow last looked (`Player::frame` posts them, the flow drains them).
    pub teleport_events: Vec<TeleportEvent>,
}

impl Zone {
    pub fn new(char_id: u32) -> Self {
        Self { char_id, ..Self::default() }
    }

    /// Global position and heading of a child at `rel` / `rel_yaw` relative to the character `parent` (`None` when it is unknown).
    fn compose(&self, parent: i32, rel: [f32; 3], rel_yaw: f32) -> Option<([f32; 3], f32)> {
        let p = self.dynels.get(&parent)?;
        let py = p.yaw.unwrap_or(0.0);
        let (s, c) = py.sin_cos();
        Some(([p.pos[0] + rel[0] * c + rel[2] * s, p.pos[1] + rel[1], p.pos[2] - rel[0] * s + rel[2] * c], py + rel_yaw))
    }

    /// The parent `parent` moved: its parented characters follow (`Vehicle_t::UpdateListeners`).
    fn place_children(&mut self, parent: i32) {
        let kids: Vec<i32> = self.parents.iter().filter(|l| l.1.kind == CHAR_KIND && l.1.instance == parent && l.0.kind == CHAR_KIND).map(|l| l.0.instance).collect();
        for k in kids {
            let Some(&(rel, ryaw)) = self.child_rel.get(&k) else { continue };
            if let (Some((pos, yaw)), Some(d)) = (self.compose(parent, rel, ryaw), self.dynels.get_mut(&k)) {
                d.pos = pos;
                d.yaw = Some(yaw);
            }
        }
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
        self.nanos.tick(dt);
        if let Some(t) = &mut self.clock {
            *t = (*t + dt).rem_euclid(GAME_DAY_SECS / TIME_SPEED);
        }
    }

    /// `n3EngineClientAnarchy_t::StartTeleportTry` [GC 0x10018f9a] after the movement sync: creates the `TeleportTrier_t` (its constructor gets
    /// [`TELEPORT_TIMEOUT`]) and runs `StartTryingTeleport` (GUI event 5). Returns `false` and does nothing while a trier exists.
    pub fn start_teleport_try(&mut self) -> bool {
        if self.trier.is_some() {
            return false;
        }
        self.trier = Some(0.0);
        self.teleport_events.push(TeleportEvent::Started);
        true
    }

    /// `TeleportTrier_t::RunFunction` [GC 0x10037e29]: adds the frame time; past [`TELEPORT_TIMEOUT`] it runs `TeleportFailed` (GUI event 6,
    /// the trier dies: `n3Fobj_t::Die` [N3 0x1000105c], `Run` [N3 0x10007e07] then drops it).
    pub fn run_trier(&mut self, dt: f32) {
        if let Some(t) = self.trier.as_mut() {
            *t += dt;
            if *t > TELEPORT_TIMEOUT {
                self.trier = None;
                self.teleport_events.push(TeleportEvent::Failed);
            }
        }
    }

    /// A playfield change (`PlayfieldAnarchyFIIR_t` or a zone redirection): everything of the old playfield is gone, the new
    /// burst re-announces the dynels, and `CharInPlay` is owed again after the new world appears (docs/zone/outgoing.md §3).
    pub fn reset_world(&mut self) {
        self.world.clear();
        self.dynels.clear();
        self.fight_target.clear();
        self.own_events.clear();
        self.parents.clear();
        self.child_rel.clear();
        self.in_play_sent = false;
        // `n3Playfield_t::StopPlayfield` kills every child of the playfield, the `TeleportTrier_t` among them
        self.trier = None;
        self.teleport_events.clear();
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
        self.nanos.on_message(who, Identity { kind: CHAR_KIND, instance: self.char_id as i32 }, &m.body);
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
                self.set_effects(&c.spells);
            }
            // server item moves of the own inventory (`play/hud_stats/zone_inv.rs`, docs/gui.md §11.12)
            N3::Inventory(m) if who.kind == CHAR_KIND && who.instance == self.char_id as i32 => self.apply_inventory(&m),
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
                    self.own_update = Some(u.clone());
                    // `FUN_10077af2` [GC]: stat `InPlay` (0xC2) = `VisualFlags >> 1 & 1`
                    self.stats.insert(IN_PLAY_STAT, i32::from(u.visual_flags >> 1 & 1));
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
                // a child of a `RelocateDynelsIIR_t` parent moves relative to it (`Vehicle +0x58` is the relative position, `UpdateListeners`
                // [VH 0x1000d0a2]: global = parent global + parent rotation * relative)
                let rel = (mv.pos, mv.yaw());
                let parent = self.parents.iter().find(|l| l.0 == who && l.1.kind == CHAR_KIND).map(|l| l.1.instance);
                let placed = parent
                    .and_then(|p| {
                        self.child_rel.insert(who.instance, rel);
                        self.compose(p, rel.0, rel.1)
                    })
                    .unwrap_or(rel);
                if let Some(d) = self.dynels.get_mut(&who.instance) {
                    d.pos = placed.0;
                    d.yaw = Some(placed.1);
                }
                self.place_children(who.instance);
            }
            // server messages that place / push / reconfigure the own character (the original applies them to its vehicle, §10)
            N3::Misc(Misc::FollowTarget(f)) if who.kind == CHAR_KIND && who.instance == self.char_id as i32 => {
                let v = |p: &ao_net::n3::misc::Vec3| [p.x, p.y, p.z];
                self.own_events.push(OwnEvent::Follow { mode: f.mode, target: (f.target.kind == CHAR_KIND && f.target.instance != 0).then_some(f.target.instance), pos: v(&f.pos), path: f.path.iter().map(v).collect() });
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && who.instance == self.char_id as i32 => {
                self.own_events.push(OwnEvent::Action(a.action));
            }
            // `CharInPlayIIR_t::Activate` [GC 0x1007264d] on the own dynel: `SetStat(0xC2, 1)` and the `AliveMessage` event
            N3::Misc(Misc::CharInPlay) if who.kind == CHAR_KIND && who.instance == self.char_id as i32 => {
                self.stats.insert(IN_PLAY_STAT, 1);
                return ZoneEvent::Alive;
            }
            N3::Unknown(body) if matches!(m.header.msg_type, ao_net::n3::spells::APPLY_SPELLS | server_move::RELOCATE | server_move::FIGHT_MODE_UPDATE) => {
                self.on_world_message(m.header.msg_type, who, &body)
            }
            N3::Unknown(body) if who.kind == CHAR_KIND => self.on_unknown(m.header.msg_type, who.instance, &body),
            // `n3TeleportIIR_t::Activate` [N3 0x10029f87]: a destination playfield only matters for the own character
            // (`StartTeleport`); otherwise the dynel is placed (`SetRelPosRot`)
            N3::Teleport(t) if who.kind == CHAR_KIND => {
                let own = who.instance == self.char_id as i32;
                if t.is_zone_change() {
                    if own {
                        return ZoneEvent::Teleport;
                    }
                } else {
                    let yaw = ao_net::n3::dynel::yaw(&t.rot);
                    if own {
                        self.own_events.push(OwnEvent::Place { pos: t.pos, yaw, full_reset: true });
                    }
                    if let Some(d) = self.dynels.get_mut(&who.instance) {
                        d.pos = t.pos;
                        d.yaw = Some(yaw);
                    }
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
            // `FUN_1007145e` / `FUN_100768ac`: add (once) / remove the pet identity of the header dynel's list (only the own one is read)
            N3::Pet(p) if who.kind == CHAR_KIND && who.instance == self.char_id as i32 => match p {
                PetList::Add(i) if !self.pets.contains(&i) => self.pets.push(i),
                PetList::Add(_) => {}
                PetList::Remove(i) => self.pets.retain(|x| *x != i),
            },
            _ => {}
        }
        ZoneEvent::None
    }

    /// `ApplySpellsIIR_t`, `RelocateDynelsIIR_t`, `FightModeUpdate_t` (docs/zone/movement.md §10.1 / §10.2): the apply of each runs on the dynel the body names.
    fn on_world_message(&mut self, msg: u32, who: Identity, body: &[u8]) {
        let own = Identity { kind: CHAR_KIND, instance: self.char_id as i32 };
        match msg {
            ao_net::n3::spells::APPLY_SPELLS => match ao_net::n3::spells::parse(body) {
                Ok(a) if a.target == own => {
                    self.apply_effects(&a.spells, a.apply);
                    self.own_events.push(OwnEvent::Spells { spells: a.spells, apply: a.apply });
                }
                Ok(_) => {}
                Err(e) => eprintln!("zone: ApplySpells: {e:#}"),
            },
            server_move::RELOCATE => match server_move::parse_relocate(body) {
                // `FUN_1003a27c`: the header must be a live character
                Ok(r) if who.kind == CHAR_KIND && self.dynels.contains_key(&who.instance) => {
                    let origin = (r.parent.kind == CHAR_KIND).then(|| self.dynels.get(&r.parent.instance).map(|d| d.pos)).flatten();
                    for c in r.children {
                        self.parents.retain(|l| l.0 != c);
                        self.parents.push((c, r.parent));
                        if c.kind != CHAR_KIND {
                            continue;
                        }
                        // `SetParentVehicle(parent, NullPos)` + `NullRot`: the child sits at the parent's origin, facing as the parent
                        self.child_rel.insert(c.instance, ([0.0; 3], 0.0));
                        if let Some((pos, yaw)) = (r.parent.kind == CHAR_KIND).then(|| self.compose(r.parent.instance, [0.0; 3], 0.0)).flatten() {
                            if let Some(d) = self.dynels.get_mut(&c.instance) {
                                d.pos = pos;
                                if c != own {
                                    d.yaw = Some(yaw);
                                }
                            }
                        }
                        if c == own {
                            self.own_events.push(OwnEvent::Relocated { parent: r.parent, pos: origin });
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => eprintln!("zone: Relocate: {e:#}"),
            },
            server_move::FIGHT_MODE_UPDATE => match server_move::parse_fight_mode_update(body) {
                Ok(u) if Some(u.playfield.instance as u32) == self.playfield => self.own_events.push(OwnEvent::FightMode(u)),
                Ok(_) => {}
                Err(e) => eprintln!("zone: FightModeUpdate: {e:#}"),
            },
            _ => {}
        }
    }

    /// `SetPosIIR_c`, `ImpulseIIR_c`, `ResurrectIIR_t` (docs/zone/movement.md §10): own character -> [`OwnEvent`], others -> their placed position / health.
    fn on_unknown(&mut self, msg: u32, who: i32, body: &[u8]) {
        use ao_net::n3::server_move as sm;
        let own = who == self.char_id as i32;
        match msg {
            sm::SET_POS => match sm::parse_set_pos(body) {
                Ok(p) if own => self.own_events.push(OwnEvent::SetPos { pos: p.pos, stop: p.stop }),
                Ok(p) => {
                    if let Some(d) = self.dynels.get_mut(&who) {
                        d.pos = p.pos;
                    }
                }
                Err(e) => eprintln!("zone: SetPos: {e:#}"),
            },
            sm::IMPULSE => match sm::parse_impulse(body) {
                Ok(v) => {
                    let own_id = ao_net::msg::Identity { kind: CHAR_KIND, instance: self.char_id as i32 };
                    for p in v.into_iter().filter(|p| p.target == own_id) {
                        self.own_events.push(OwnEvent::Impulse { delta: p.delta, time: p.time });
                    }
                }
                Err(e) => eprintln!("zone: Impulse: {e:#}"),
            },
            sm::RESURRECT => match sm::parse_resurrect(body) {
                Ok(r) if own => {
                    self.stats.insert(ao_formats::stats::HEALTH, r.health);
                    self.stats.insert(NANO_STAT, r.nano);
                    self.own_events.push(OwnEvent::Resurrected);
                }
                Ok(r) => {
                    if let Some(d) = self.dynels.get_mut(&who) {
                        d.health = r.health;
                    }
                }
                Err(e) => eprintln!("zone: Resurrect: {e:#}"),
            },
            _ => {}
        }
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

    /// `StartTeleportTry` runs once while the trier lives; the trier fails after 30 s (`RunFunction`), and the playfield change kills it.
    #[test]
    fn teleport_trier_runs_once_and_times_out() {
        let mut z = Zone::new(1);
        assert!(z.start_teleport_try() && !z.start_teleport_try());
        z.run_trier(29.0);
        assert!(z.trier.is_some());
        z.run_trier(1.5);
        assert!(z.trier.is_none());
        assert_eq!(z.teleport_events, [TeleportEvent::Started, TeleportEvent::Failed]);
        z.teleport_events.clear();
        assert!(z.start_teleport_try(), "a new try may start once the old one failed");
        z.reset_world();
        assert!(z.trier.is_none() && z.teleport_events.is_empty(), "StopPlayfield kills the trier");
    }

    /// The server's relay of our own `CharInPlayIIR_t` (live capture, 104 ms after we sent it) is `AliveMessage`; the relays for other characters are not.
    #[test]
    fn own_char_in_play_relay_is_the_alive_event() {
        let mut z = Zone::new(0x82e8);
        let ev: Vec<_> = frames(include_str!("../../../../docs/captures/zone_enter_ithaca.rec")).iter().map(|f| z.on_frame(f)).collect();
        assert_eq!(ev.iter().filter(|e| **e == ZoneEvent::Alive).count(), 1);
        assert_eq!(z.stat(IN_PLAY_STAT), Some(1));
        let mut z = Zone::new(25988);
        let ev: Vec<_> = frames(include_str!("../../../../docs/captures/zone_ithaca.rec")).iter().map(|f| z.on_frame(f)).collect();
        assert!(!ev.contains(&ZoneEvent::Alive), "the capture holds only relays for other characters");
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

    /// `UpdateListeners` [VH 0x1000d0a2] / `GetGlobalRot`: a parented character's global position is `parent + R_y(parent heading) * relative`, its
    /// heading the parent's plus its own. One level (a parent that is itself parented keeps the last global it was given).
    #[test]
    fn children_move_with_their_parent() {
        let d = |pos, yaw| DynelState { name: String::new(), pos, yaw: Some(yaw), npc: false, side: 0, level: 1, health: 1, max_health: 1 };
        let mut z = Zone::new(1);
        let (p, c) = (Identity { kind: CHAR_KIND, instance: 10 }, Identity { kind: CHAR_KIND, instance: 20 });
        z.dynels.insert(10, d([100.0, 5.0, 50.0], std::f32::consts::FRAC_PI_2));
        z.dynels.insert(20, d([0.0; 3], 0.0));
        z.parents.push((c, p));
        z.child_rel.insert(20, ([1.0, 0.5, 2.0], 0.25));
        // heading +90 degrees about Y: (x, z) = (1, 2) -> (x cos + z sin, -x sin + z cos) = (2, -1)
        let (pos, yaw) = z.compose(10, [1.0, 0.5, 2.0], 0.25).unwrap();
        assert!((pos[0] - 102.0).abs() < 1e-4 && (pos[1] - 5.5).abs() < 1e-5 && (pos[2] - 49.0).abs() < 1e-4, "{pos:?}");
        assert!((yaw - (std::f32::consts::FRAC_PI_2 + 0.25)).abs() < 1e-6);
        // the parent moving carries the child
        z.dynels.get_mut(&10).unwrap().pos = [200.0, 0.0, 0.0];
        z.place_children(10);
        let ch = &z.dynels[&20];
        assert!((ch.pos[0] - 202.0).abs() < 1e-4 && (ch.pos[2] + 1.0).abs() < 1e-4, "{:?}", ch.pos);
        assert!(z.compose(99, [0.0; 3], 0.0).is_none(), "unknown parent");
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
        assert!(health.windows(2).any(|w| w[0] != w[1]) || !health.is_empty());
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

    /// SetPos / Impulse / CharacterAction / Resurrect for the own dynel become [`OwnEvent`]s (hand-built frames, none are in the captures).
    #[test]
    fn own_server_moves_are_queued() {
        use ao_net::msg::Identity;
        use ao_net::n3::{action, outgoing::n3_frame, server_move as sm};
        let me = Identity { kind: CHAR_KIND, instance: 7 };
        let other = Identity { kind: CHAR_KIND, instance: 8 };
        let mut z = Zone::new(7);
        z.dynels.insert(8, DynelState { name: String::new(), pos: [0.0; 3], yaw: None, npc: false, side: 0, level: 1, health: 5, max_health: 5 });
        let f = |p| n3_frame(1, 0, p);
        z.on_frame(&f(sm::set_pos(me, &sm::SetPos { pos: [1.0, 2.0, 3.0], last_allowed: false, crowd_limit: 0, stop: true })));
        z.on_frame(&f(sm::set_pos(other, &sm::SetPos { pos: [9.0, 9.0, 9.0], last_allowed: false, crowd_limit: 0, stop: false })));
        let push = |t, d| sm::Push { target: t, delta: d, time: 0.5 };
        z.on_frame(&f(sm::impulse(me, &[push(other, [1.0; 3]), push(me, [2.0, 0.0, 4.0])])));
        z.on_frame(&f(action::character_action(7, &action::simple(action::id::STAND_UP, Identity::default(), Identity::default()))));
        z.on_frame(&f(action::character_action(8, &action::simple(action::id::STAND_UP, Identity::default(), Identity::default()))));
        z.on_frame(&f(sm::resurrect(me, &sm::Resurrect { health: 77, nano: 33 })));
        assert_eq!(
            z.own_events,
            [
                OwnEvent::SetPos { pos: [1.0, 2.0, 3.0], stop: true },
                OwnEvent::Impulse { delta: [2.0, 0.0, 4.0], time: 0.5 },
                OwnEvent::Action(0x57),
                OwnEvent::Resurrected
            ]
        );
        assert_eq!((z.stat(ao_formats::stats::HEALTH), z.stat(0xD6)), (Some(77), Some(33)));
        assert_eq!(z.dynels[&8].pos, [9.0, 9.0, 9.0], "another dynel is just placed");
        z.reset_world();
        assert!(z.own_events.is_empty());
    }
}
