//! The own character's movement: a headless port of the original client's movement state machine
//! (`Movement_n::MovementStatus_t` / `CharMovementStatus_t`), `PlayerVehicle_t` inputs, `Vehicle_t` integrator
//! and the `CharDCMoveIIR_t` producers (`n3EngineClientAnarchy_t::N3Msg_MovementChanged` & co.).
//! Evidence, addresses and the unresolved parts: `docs/zone/movement.md`.
//!
//! Coordinates are SERVER coordinates (Y up). The heading `yaw` is the server quaternion heading
//! `2*atan2(q.y, q.w)` (`q = (0, sin(yaw/2), 0, cos(yaw/2))`); *forward* in the server XZ plane is `(sin yaw, cos yaw)`
//! (`Vehicle_t::s_cReferenceForward = (0,0,1)` rotated by the body quaternion [Vehicle.dll 0x10019398], confirmed against
//! the captured relayed moves: displacement after a ForwardStart points along `atan2(dx,dz) == yaw`). A positive yaw
//! step turns right (clockwise seen from above): ForwardStart at yaw 0 moves +Z, TurnRight raises yaw. In scene
//! coordinates (`scene_pos(p) = (x, y, -z)`) the same heading is the `ao_render::Camera` yaw: `(sin yaw, 0, -cos yaw)`.

use ao_formats::character::Role;
use ao_formats::playfield::collision::{Aligned, Body, LiquidEvent, SurfaceState, FOOT_CLEARANCE};
use ao_net::n3::action::SitInput;
use ao_net::n3::outgoing::CharMove;

mod fx;

/// Terrain / collision queries in SERVER coordinates.
pub trait World {
    /// Ground height under the FEET position `p` (`Surface_i::CalculateClosestPoint`: terrain / floor, raised by a short KD
    /// ray), `None` outside the playfield.
    fn ground(&self, p: [f32; 3]) -> Option<f32>;
    /// `Surface_i::GetLineIntersection(p, p + (0, 100, 0))` [vtbl +0xc]: height of the first surface hit straight above the FEET position `p`
    /// (the jump's ceiling raycast, `FUN_1006f9e9` [GC]); `None` when nothing is within 100 m.
    fn ceiling(&self, p: [f32; 3]) -> Option<f32>;
    /// `Vehicle_t::EnsureSurfaceAlignment` for one integration step `old -> new` (feet positions): wall sweep, ground
    /// following, support test (`docs/zone/collision.md` §3.4).
    fn align(&self, old: [f32; 3], new: [f32; 3], body: &Body, st: &mut SurfaceState) -> Aligned;
}

/// `Movement_n::Mode_e` values (`CharMovementStatus_t + 4`).
pub mod mode {
    pub const FROZEN: u8 = 1;
    pub const WALK: u8 = 2;
    pub const RUN: u8 = 3;
    pub const SWIM: u8 = 4;
    pub const CRAWL: u8 = 5;
    pub const SNEAK: u8 = 6;
    pub const FLY: u8 = 7;
    pub const SIT_GROUND: u8 = 8;
    pub const SLEEP: u8 = 0xB;
    pub const LOUNGE: u8 = 0xC;
}

/// `Movement_n::MovementAction_e` ids (the `CharDCMove` move type) with the transition class each one builds
/// in `FUN_1006c60f`. Ids without a constant (0x13, 0x14, 0x1f, 0x20, 0x2b..) build no transition.
pub mod id {
    pub const FORWARD_START: u8 = 1;
    pub const FORWARD_STOP: u8 = 2;
    pub const REVERSE_START: u8 = 3;
    pub const REVERSE_STOP: u8 = 4;
    pub const STRAFE_RIGHT_START: u8 = 5;
    pub const STRAFE_RIGHT_STOP: u8 = 6;
    pub const STRAFE_LEFT_START: u8 = 7;
    pub const STRAFE_LEFT_STOP: u8 = 8;
    pub const TURN_RIGHT_START: u8 = 9;
    pub const MOUSE_TURN_RIGHT_START: u8 = 0xA;
    pub const TURN_RIGHT_STOP: u8 = 0xB;
    pub const TURN_LEFT_START: u8 = 0xC;
    pub const MOUSE_TURN_LEFT_START: u8 = 0xD;
    pub const TURN_LEFT_STOP: u8 = 0xE;
    pub const JUMP_START: u8 = 0xF;
    pub const JUMP_STOP: u8 = 0x10;
    pub const ELEVATE_UP_START: u8 = 0x11;
    pub const ELEVATE_UP_STOP: u8 = 0x12;
    pub const FULL_STOP: u8 = 0x15;
    /// Position/rotation sync: no FSM transition.
    pub const SYNC: u8 = 0x16;
    pub const SWITCH_FROZEN: u8 = 0x17;
    pub const SWITCH_WALK: u8 = 0x18;
    pub const SWITCH_RUN: u8 = 0x19;
    pub const SWITCH_SWIM: u8 = 0x1A;
    pub const SWITCH_CRAWL: u8 = 0x1B;
    pub const SWITCH_SNEAK: u8 = 0x1C;
    pub const SWITCH_FLY: u8 = 0x1D;
    pub const SWITCH_SIT_GROUND: u8 = 0x1E;
    pub const SWITCH_SLEEP: u8 = 0x21;
    pub const SWITCH_LOUNGE: u8 = 0x22;
    pub const LEAVE_SWIM: u8 = 0x23;
    pub const LEAVE_SNEAK: u8 = 0x24;
    pub const LEAVE_SIT: u8 = 0x25;
    pub const LEAVE_FROZEN: u8 = 0x26;
    pub const LEAVE_FLY: u8 = 0x27;
    pub const LEAVE_CRAWL: u8 = 0x28;
    pub const LEAVE_SLEEP: u8 = 0x29;
    pub const LEAVE_LOUNGE: u8 = 0x2A;
    /// `N3Msg_MouseMovement` pseudo action: local look rotation, rewritten to [`SYNC`] and never sent.
    pub const MOUSE_LOOK: u8 = 0x2B;
}

/// The `TransitionAction_i` classes (vftables Gamecode 0x101606dc..0x10160918) a transition runs when applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    ForwardStart,
    ForwardStop,
    ReverseStart,
    ReverseStop,
    StrafeRightStart,
    StrafeLeftStart,
    StrafeStop,
    TurnRightStart,
    MouseTurnRightStart,
    TurnLeftStart,
    MouseTurnLeftStart,
    TurnStop,
    JumpStart,
    JumpStop,
    ElevateUpStart,
    ElevateUpStop,
    FullStop,
    ToFrozen,
    ToWalk,
    ToRun,
    ToSwim,
    ToCrawl,
    ToSneak,
    ToFly,
    ToSitGround,
    ToSleep,
    ToLounge,
    LeaveSwim,
    LeaveSneak,
    LeaveSit,
    LeaveFrozen,
    LeaveFly,
    LeaveCrawl,
    LeaveSleep,
    LeaveLounge,
}

/// `Movement_n::MovementStatus_t` (ctor `FUN_1007038f`, Gamecode 0x1007038f): the state-machine variables.
/// Axis fields are 1 = idle / 2 = moving (turn: 4 = turning, jump: 3 = jumping); direction fields name the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fsm {
    /// `+4` mode ([`mode`]).
    pub mode: u8,
    /// `+8` forward/back axis.
    pub fwd: u8,
    /// `+0xC` 0 none, 1 forward, 2 reverse.
    pub fwd_dir: u8,
    /// `+0x10` strafe axis.
    pub strafe: u8,
    /// `+0x14` 0 none, 3 left, 4 right.
    pub strafe_dir: u8,
    /// `+0x18` elevate axis.
    pub elev: u8,
    /// `+0x1C` 0 none, 5 up.
    pub elev_dir: u8,
    /// `+0x20` turn axis.
    pub turn: u8,
    /// `+0x24` 0 none, 3 left, 4 right.
    pub turn_dir: u8,
    /// `+0x28` jump (1 idle, 3 jumping).
    pub jump: u8,
    /// `CharMovementStatus_t + 0x30`: last walk/run mode (ctor `FUN_1006c16d`: 2); the `Leave*` transitions return to it.
    pub last_speed_mode: u8,
}

impl Default for Fsm {
    fn default() -> Self {
        Self::new()
    }
}

impl Fsm {
    /// Initial state: run mode (`FUN_1007038f` stores 3), everything idle, last speed mode walk (`FUN_1006c16d`).
    pub const fn new() -> Fsm {
        Fsm { mode: mode::RUN, fwd: 1, fwd_dir: 0, strafe: 1, strafe_dir: 0, elev: 1, elev_dir: 0, turn: 1, turn_dir: 0, jump: 1, last_speed_mode: mode::WALK }
    }

    /// `CharMovementStatus_t` vtable slot 7 (`FUN_1006c469`).
    pub fn is_moving(&self) -> bool {
        self.fwd == 2 || self.strafe == 2 || self.jump == 3 || self.mode == mode::FLY
    }

    /// `CharMovementStatus_t` vtable slot 3 (`FUN_1006c1ab`): may action `id` be started in the current mode?
    pub fn allowed(&self, id: u8) -> bool {
        use mode::*;
        let p = id as u32;
        match self.mode {
            FROZEN => matches!(id, 9 | 0xB | 0xC | 0xE | 0x10 | 0x26),
            // 0x1e (sit) additionally refuses while moving forward or jumping.
            WALK => !(matches!(id, 0x18 | 0x21 | 0x22 | 0x25) || (id == 0x1E && (self.fwd == 2 || self.jump == 3))),
            RUN => !(matches!(id, 0x19 | 0x21 | 0x22) || (id == 0x1E && (self.fwd == 2 || self.jump == 3))),
            SWIM => !matches!(id, 5 | 7 | 0xF | 0x18 | 0x19 | 0x1B | 0x1C | 0x1D | 0x1E | 0x1F | 0x24 | 0x27 | 0x28),
            CRAWL => {
                if p < 0x1A {
                    !(p > 0x17 || matches!(id, 5 | 7 | 0xF))
                } else if p < 0x1C {
                    true
                } else {
                    !(p < 0x1F || id == 0x27)
                }
            }
            SNEAK => !matches!(id, 0x21 | 0x2A),
            FLY => !matches!(id, 0x1B | 0x1C | 0x1E | 0x21 | 0x22 | 0x25 | 0x28 | 0x29 | 0x2A),
            SIT_GROUND => {
                if p > 0x19 {
                    match id {
                        0x1A => true,
                        0x21 | 0x22 => !self.is_moving(),
                        0x25 => true,
                        _ => false,
                    }
                } else {
                    matches!(id, 9..=0xE)
                }
            }
            SLEEP => matches!(id, 0x1A | 0x22 | 0x29),
            LOUNGE => matches!(id, 0x1A | 0x21 | 0x2A),
            _ => true,
        }
    }

    /// `CharMovementStatus_t` vtable slot 8 (`FUN_1006c60f`, 42-way jump table Gamecode 0x1006d0ee): the state after
    /// action `id` and the transition class to run, or `None` when the case refuses (jump-table default).
    /// `can_move` = Features (stat 0xE0) bit 4.
    pub fn build(&self, id: u8, can_move: bool) -> Option<(Fsm, Act)> {
        use mode::*;
        let mut n = *self;
        // Original quirk: the new elevate axis is initialised from the *strafe* axis (`FUN_100704f2` called twice).
        n.elev = self.strafe;
        let last = self.last_speed_mode;
        // Leave* with the "no move feature" path: the original falls into the SwitchToFrozen case.
        let frozen = |mut n: Fsm| {
            n.mode = FROZEN;
            Some((n, Act::ToFrozen))
        };
        let act = match id {
            id::FORWARD_START => {
                // idle -> forward, or reverse -> forward (two separate branches in the original, same result)
                if !(self.fwd == 1 || (self.fwd == 2 && self.fwd_dir == 2)) {
                    return None;
                }
                n.fwd = 2;
                n.fwd_dir = 1;
                Act::ForwardStart
            }
            id::FORWARD_STOP if self.fwd == 2 && self.fwd_dir == 1 => {
                n.fwd = 1;
                n.fwd_dir = 0;
                Act::ForwardStop
            }
            id::REVERSE_START if self.fwd == 1 || (self.fwd == 2 && self.fwd_dir == 1) => {
                n.fwd = 2;
                n.fwd_dir = 2;
                Act::ReverseStart
            }
            id::REVERSE_STOP if self.fwd == 2 && self.fwd_dir == 2 => {
                n.fwd = 1;
                n.fwd_dir = 0;
                Act::ReverseStop
            }
            id::STRAFE_RIGHT_START if self.strafe == 1 || (self.strafe == 2 && self.strafe_dir == 3) => {
                n.strafe = 2;
                n.strafe_dir = 4;
                Act::StrafeRightStart
            }
            id::STRAFE_RIGHT_STOP if self.strafe == 2 && self.strafe_dir == 4 => {
                n.strafe = 1;
                n.strafe_dir = 0;
                Act::StrafeStop
            }
            id::STRAFE_LEFT_START if self.strafe == 1 || (self.strafe == 2 && self.strafe_dir == 4) => {
                n.strafe = 2;
                n.strafe_dir = 3;
                Act::StrafeLeftStart
            }
            id::STRAFE_LEFT_STOP if self.strafe == 2 && self.strafe_dir == 3 => {
                n.strafe = 1;
                n.strafe_dir = 0;
                Act::StrafeStop
            }
            id::TURN_RIGHT_START | id::MOUSE_TURN_RIGHT_START => {
                n.turn = 4;
                n.turn_dir = 4;
                if id == id::TURN_RIGHT_START { Act::TurnRightStart } else { Act::MouseTurnRightStart }
            }
            id::TURN_LEFT_START | id::MOUSE_TURN_LEFT_START => {
                n.turn = 4;
                n.turn_dir = 3;
                if id == id::TURN_LEFT_START { Act::TurnLeftStart } else { Act::MouseTurnLeftStart }
            }
            id::TURN_RIGHT_STOP if self.turn == 4 && self.turn_dir == 4 => {
                n.turn = 1;
                n.turn_dir = 0;
                Act::TurnStop
            }
            id::TURN_LEFT_STOP if self.turn == 4 && self.turn_dir == 3 => {
                n.turn = 1;
                n.turn_dir = 0;
                Act::TurnStop
            }
            id::JUMP_START if !(self.jump == 3 && self.mode != FLY) => {
                n.jump = 3;
                Act::JumpStart
            }
            id::JUMP_STOP if self.jump == 3 => {
                if self.mode != FLY {
                    n.jump = 1;
                }
                Act::JumpStop
            }
            id::ELEVATE_UP_START if self.mode == FLY => {
                n.elev = 2;
                n.elev_dir = 5;
                Act::ElevateUpStart
            }
            id::ELEVATE_UP_STOP if self.elev == 2 && self.elev_dir == 5 => {
                n.elev_dir = 0;
                n.elev = 1;
                Act::ElevateUpStop
            }
            id::FULL_STOP => {
                n.fwd = 1;
                n.fwd_dir = 0;
                n.strafe = 1;
                n.strafe_dir = 0;
                n.turn = 1;
                n.turn_dir = 0;
                n.jump = 1;
                n.elev = 1;
                n.elev_dir = 0;
                Act::FullStop
            }
            id::SWITCH_FROZEN => {
                n.mode = FROZEN;
                Act::ToFrozen
            }
            id::SWITCH_WALK => {
                n.mode = WALK;
                Act::ToWalk
            }
            id::SWITCH_RUN => {
                n.mode = RUN;
                n.last_speed_mode = RUN;
                Act::ToRun
            }
            id::SWITCH_SWIM => {
                n.mode = SWIM;
                Act::ToSwim
            }
            id::SWITCH_CRAWL if self.jump == 1 && self.fwd == 1 => {
                n.mode = CRAWL;
                Act::ToCrawl
            }
            id::SWITCH_SNEAK => {
                n.mode = SNEAK;
                Act::ToSneak
            }
            id::SWITCH_FLY => {
                n.mode = FLY;
                Act::ToFly
            }
            id::SWITCH_SIT_GROUND if !self.is_moving() && can_move => {
                n.mode = SIT_GROUND;
                Act::ToSitGround
            }
            id::SWITCH_SLEEP if self.jump == 1 && self.fwd == 1 => {
                n.mode = SLEEP;
                Act::ToSleep
            }
            id::SWITCH_LOUNGE if !self.is_moving() && self.jump == 1 && self.fwd == 1 => {
                n.mode = LOUNGE;
                Act::ToLounge
            }
            id::LEAVE_SWIM => {
                n.mode = last;
                Act::LeaveSwim
            }
            id::LEAVE_SNEAK => {
                n.mode = last;
                Act::LeaveSneak
            }
            id::LEAVE_FROZEN => {
                n.mode = last;
                Act::LeaveFrozen
            }
            id::LEAVE_FLY => {
                n.mode = last;
                Act::LeaveFly
            }
            id::LEAVE_SIT => {
                if !can_move {
                    return frozen(n);
                }
                if self.mode == SIT_GROUND {
                    n.mode = last;
                }
                Act::LeaveSit
            }
            id::LEAVE_CRAWL if !self.is_moving() => {
                if !can_move {
                    return frozen(n);
                }
                if self.mode == CRAWL {
                    n.mode = last;
                }
                Act::LeaveCrawl
            }
            id::LEAVE_SLEEP if !self.is_moving() => {
                if !can_move {
                    return frozen(n);
                }
                if self.mode == SLEEP {
                    n.mode = SIT_GROUND;
                }
                Act::LeaveSleep
            }
            id::LEAVE_LOUNGE if !self.is_moving() => {
                if !can_move {
                    return frozen(n);
                }
                if self.mode == LOUNGE {
                    n.mode = SIT_GROUND;
                }
                Act::LeaveLounge
            }
            _ => return None,
        };
        Some((n, act))
    }
}

/// The stats the movement code reads (`GetStat(id, flag)` on the dynel's stat block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    /// 0x9C RunSpeed.
    pub run_speed: i32,
    /// 0x1B Health.
    pub health: i32,
    /// 0x01 Life (max health).
    pub max_health: i32,
    /// 0x10B TurnSpeed (0 = default rates).
    pub turn_speed: i32,
    /// 0x10 Strength / 0x11 Agility (jump height `FUN_1005844d`).
    pub strength: i32,
    pub agility: i32,
    /// 0xE0 Features: bit 2 = may turn, bit 4 = may move (`FUN_1006b84b`, `FUN_10044b6e(4)`).
    pub features: i32,
    /// 0x296 MechData (non-zero: driving a vehicle; jump refused by `N3Msg_MovementChanged`).
    pub mech_data: i32,
    /// 0xD7 GmLevel (bit 0 enables the fly switch, `FUN_1006b84b`).
    pub gm_level: i32,
    /// 0x168 MonsterScale (animation time scale `100/scale`, `FUN_1006fb56`; 0 = none).
    pub monster_scale: i32,
    /// 0x1AE WaitState (0 none, 2 sit, 0xE crawl, 0xF sleep, 0x10 lounge) used by the crawl/sit toggles.
    pub wait_state: i32,
    /// 0x0 Flags: bit 0x20000000 = `DisableFalling`, 0x80000000 = `DisableSurfaceCollision` (stat hook `FUN_10059e6a` [GC] stat 0).
    pub flags: i32,
}

impl Stats {
    pub fn new(run_speed: i32) -> Self {
        Stats { run_speed, health: 1, max_health: 1, turn_speed: 0, strength: 0, agility: 0, features: 4, mech_data: 0, gm_level: 0, monster_scale: 0, wait_state: 0, flags: 0 }
    }

    /// `FUN_1006edb3`: RunSpeed, reduced linearly once health drops below 15 % of Life
    /// (`ratio = health / (life * 0.15)`; `ratio < 1 -> ratio * (rs + 1000) - 1000`).
    fn eff_run_speed(&self) -> f32 {
        let ratio = self.health as f32 / (self.max_health as f32 * 0.15);
        if ratio < 1.0 {
            ratio * (self.run_speed as f32 + 1000.0) - 1000.0
        } else {
            self.run_speed as f32
        }
    }

    /// `FUN_1005844d`: jump height in metres, `(Agility + Strength) / 200 + 1`, at least 0.5 (sums above 800 are
    /// clamped to 800 unless GmLevel != 0).
    fn jump_height(&self) -> f32 {
        let (mut s, mut a) = (self.strength as f32, self.agility as f32);
        if s + a > 800.0 && self.gm_level == 0 {
            s = 800.0;
            a = 0.0;
        }
        ((a + s) / 200.0 + 1.0).max(0.5)
    }
}

/// `Vehicle +0x108` path installed by `Vehicle_t::Impulse` (Vehicle.dll 0x1000cd61): a `BallisticPath_t` (ctor 0x100011fb, eval 0x10001294).
struct Ballistic {
    start: [f32; 3],
    /// Initial velocity `(dest - start) / T - g T / 2`.
    v0: [f32; 3],
    dur: f32,
    /// `Vehicle +0xac`: time since the impulse.
    t: f32,
    /// `Vehicle +0x150`: the "hugging" (falling enabled) flag saved by `FUN_1000c41a` and restored at the end.
    restore_falling: bool,
}

/// Stat ids written by the transition `Apply` functions (`SetStat`, `dynel+0xe8 -> vtbl[0x10]`).
pub const STAT_REST_MODIFIER: u32 = 0x1A9;
pub const STAT_WAIT_STATE: u32 = 0x1AE;
/// MechData stat (0x296): cleared by the liquid-enter callback.
pub const STAT_MECH_DATA: u32 = 0x296;
/// `Flags` stat bits the stat hook turns into vehicle switches.
const FLAG_NO_FALL: i32 = 0x2000_0000;
const FLAG_NO_SURFACE: i32 = i32::MIN;

const GRAVITY: f32 = -20.0; // Vehicle_t::s_vGravityAccel [Vehicle.dll 0x1001938c]
const VY_LIMIT: f32 = 50.0; // f64 @ Vehicle.dll 0x10012748
const MASS: f32 = 10.0; // default when Vehicle +0x34 == 0 [GC 0x1015f168]; the real mass source is unresolved
const SPEED_EPS: f32 = 0.001; // f32 @ Vehicle.dll 0x1001270c
/// Smallest jump height under a low ceiling (f32 0.1 @ GC 0x10160810).
const JUMP_MIN_HEADROOM: f32 = 0.1;
const MAX_DT: f32 = 4.0; // f32 @ Vehicle.dll 0x10012804: longer frames skip the integration
const MAX_SUBSTEP: f32 = 0.05; // [GUESS] Vehicle +0x104 is not initialised in the code read
/// `CheckMotionUpdate`: idle timeout while moving and the rotation-change check [GC 0x101574fc, 0x101574f8, 0x101574f0].
const SYNC_PERIOD: f32 = 5.0;
const ROT_SYNC_MIN_AGE: f32 = 0.25;
const ROT_SYNC_ANGLE: f32 = 0.17;
/// `Ballistic` gravity: f32 -9.81 @ Vehicle.dll 0x10012798 (the steering integrator's `s_vGravityAccel` is -20, a different value).
const BALLISTIC_G: f32 = -9.81;
/// A ballistic flight ends when `EnsureSurfaceAlignment` moved the body at least this far from the path point (f32 0.5 @ VH 0x10012134).
const BALLISTIC_ABORT: f32 = 0.5;
/// `FollowTargetIIR_c` keeps at most 30 waypoints (`Vehicle +0x190`, 30 x 12 bytes).
const FOLLOW_MAX: usize = 30;

/// The own character's kinematic state and the movement message producers.
pub struct Movement {
    pos: [f32; 3],
    /// Horizontal velocity (`Vehicle +0x64 / +0x6c`).
    vel: [f32; 2],
    /// Vertical speed (`Vehicle +0x54`).
    vy: f32,
    yaw: f32,
    /// `Vehicle +0x90` direction (1 forward, -1 while reversing).
    dir: i32,
    airborne: bool,
    falling_enabled: bool,
    launch_y: f32,
    /// `PlayerVehicle +0x164 == 0`: a new jump may start.
    jump_ready: bool,
    /// Jump height of a `JumpStart` whose launch waits for the next [`Movement::update`] (ceiling raycast).
    jump_pending: Option<f32>,
    fsm: Fsm,
    stats: Stats,
    /// `PlayerVehicle` inputs: +0x360 forward (1/-1), +0x364 strafe speed, +0x368 turn rate, +0x36C elevate speed.
    in_fwd: f32,
    in_strafe: f32,
    in_turn: f32,
    in_elev: f32,
    max_vel: f32,
    ref_speed: f32,
    force: f32,
    /// `Vehicle +0x10 vtbl[0x24]` (`FUN_10070fd0`): no path following and the dynel is not flagged.
    controllable: bool,
    /// `dynel + 0x2c8 != 0 && FUN_1002e347() != 0`: refuses the walk switch (unresolved condition).
    walk_locked: bool,
    /// `n3EngineClientAnarchy_t + 0x68` factor of the TurnSpeed mouse clamp (unresolved, 1.0).
    mouse_scale: f32,
    // --- n3EngineClientAnarchy_t motion-message bookkeeping ---
    clock: f64,
    /// `+0xD8` time of the last motion message, `+0xE0` its rotation, `+0xF0` its position.
    last_msg_time: f64,
    last_rot: [f32; 4],
    last_msg_pos: [f32; 3],
    /// `+0xFC`: the mouse rotated the character since the last sync.
    look_dirty: bool,
    /// `DAT_102e2588`: mouse-look turning is active.
    mouse_look: bool,
    /// `DAT_102e32d8`: time of the last `CharDCMove` write (`elapsed_ms` base).
    last_write: f64,
    /// `DAT_101bef94` / `dynel + 0x220` (`FUN_1005a5d6`).
    zone_counter: i32,
    zone_inst: u32,
    snap_pending: bool,
    /// `EnsureSurfaceAlignment`'s memory (hover counter, last allowed dungeon room and position).
    surface: SurfaceState,
    /// Active `Impulse` flight (`Vehicle +0x108`): suspends all steering and refuses movement actions.
    ballistic: Option<Ballistic>,
    /// `FollowTargetIIR_c` waypoints (`Vehicle +0x190`, zero-terminated): the vehicle steers to the first one (`FUN_10070fee`).
    follow: Vec<[f32; 3]>,
    /// The follow-target dynel (`Vehicle +0x180`, a char instance) and its last known position (`n3Dynel_t::GetRelPos`, fed by the caller each
    /// frame through [`Movement::set_chase_pos`]): with a target set the vehicle steers to a point 4 m short of it (`FUN_10070185`).
    chase: Option<i32>,
    chase_pos: Option<[f32; 3]>,
    /// Position at the start of the previous sub-step (`Vehicle +0xd0 / +0xd8`), for the crossing test of `SteeringDirArrive`.
    prev_pos: [f32; 3],
    /// `DummyVehicle_t::Enable/DisableSurfaceCollision` (stat `Flags` bit 0x80000000).
    surface_collision: bool,
    /// Stats written locally by transition `Apply`s, handed to the stat holder by [`Movement::take_stat_writes`].
    stat_writes: Vec<(u32, i32)>,
    /// Nano-effect state of the own character (Features counters, crowd-control state, vehicle class): `movement/fx.rs`.
    fx: fx::Fx,
    outbox: Vec<CharMove>,
}

fn rot_of(yaw: f32) -> [f32; 4] {
    [0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()]
}

/// Heading of a server quaternion (`2*atan2(y, w)`, in `(-pi, pi]` for `w >= 0`).
#[cfg(test)]
pub fn yaw_of(q: [f32; 4]) -> f32 {
    2.0 * q[1].atan2(q[3])
}

fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let a = a % t;
    if a > std::f32::consts::PI {
        a - t
    } else if a <= -std::f32::consts::PI {
        a + t
    } else {
        a
    }
}

fn truncate(v: [f32; 2], max: f32) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if l != 0.0 && max < l {
        let k = max / l;
        [v[0] * k, v[1] * k]
    } else {
        v
    }
}

impl Movement {
    /// Character standing at `pos` heading `yaw` (server quaternion heading), RunSpeed stat `run_speed`.
    pub fn new(pos: [f32; 3], yaw: f32, run_speed: i16) -> Self {
        let mut m = Movement {
            pos,
            vel: [0.0; 2],
            vy: 0.0,
            yaw: wrap(yaw),
            dir: 1,
            airborne: false,
            falling_enabled: true,
            launch_y: pos[1],
            jump_ready: true,
            jump_pending: None,
            fsm: Fsm::new(),
            stats: Stats::new(run_speed as i32),
            in_fwd: 0.0,
            in_strafe: 0.0,
            in_turn: 0.0,
            in_elev: 0.0,
            max_vel: 0.0,
            ref_speed: 1.0,
            force: 0.0,
            controllable: true,
            walk_locked: false,
            mouse_scale: 1.0,
            clock: 0.0,
            last_msg_time: 0.0,
            last_rot: rot_of(yaw),
            last_msg_pos: pos,
            look_dirty: false,
            mouse_look: false,
            last_write: 0.0,
            zone_counter: -1,
            zone_inst: 0,
            snap_pending: false,
            surface: SurfaceState::default(),
            ballistic: None,
            follow: Vec::new(),
            chase: None,
            chase_pos: None,
            prev_pos: pos,
            surface_collision: true,
            stat_writes: Vec::new(),
            fx: fx::Fx::default(),
            outbox: Vec::new(),
        };
        m.recalc();
        m
    }

    /// Restore the FullUpdate vehicle state without replaying movement transitions.
    /// PlayerVehicle read hook [GC 0x1007135d], CharVehicle [GC 0x1006f03e],
    /// FSM [GC 0x10070438 / 0x1006c4dc]; the four player inputs follow at byte 26.
    pub fn restore_blob(&mut self, blob: &[u8]) {
        let Some(s) = ao_net::n3::motion::Status::from_blob(blob) else { return };
        let float = |offset: usize| f32::from_be_bytes(blob[offset..offset + 4].try_into().unwrap());
        if ![0, 4, 8].into_iter().all(|offset| float(offset).is_finite())
            || (blob.len() >= 42 && ![26, 30, 34, 38].into_iter().all(|offset| float(offset).is_finite()))
        {
            return;
        }
        self.fsm = Fsm {
            mode: s.mode as u8,
            last_speed_mode: s.prev_mode as u8,
            fwd: if s.forward == 0 { 1 } else { 2 },
            fwd_dir: match s.forward { -1 => 2, 1 => 1, _ => 0 },
            strafe: if s.strafe == 0 { 1 } else { 2 },
            strafe_dir: match s.strafe { -1 => 3, 1 => 4, _ => 0 },
            elev: if s.elevating { 2 } else { 1 },
            elev_dir: if s.elevating { 5 } else { 0 },
            turn: if s.turn == 0 { 1 } else { 4 },
            turn_dir: match s.turn { -1 => 3, 1 => 4, _ => 0 },
            jump: if s.jumping { 3 } else { 1 },
        };
        self.recalc();
        self.vel = [float(0), float(8)];
        self.vy = float(4);
        self.dir = if s.forward < 0 { -1 } else { 1 };
        if blob.len() >= 42 {
            self.in_fwd = float(26);
            self.in_strafe = float(30);
            self.in_turn = float(34);
            self.in_elev = float(38);
        }
    }

    // ---- getters -------------------------------------------------------------------------------------------------

    /// Position in server coordinates (feet).
    pub fn pos(&self) -> [f32; 3] {
        self.pos
    }
    /// Heading: the server quaternion heading, forward is `(sin yaw, cos yaw)` in server XZ.
    pub fn yaw(&self) -> f32 {
        self.yaw
    }
    /// The `GetRelRot()` quaternion `(x, y, z, w)`.
    pub fn rot(&self) -> [f32; 4] {
        rot_of(self.yaw)
    }
    pub fn grounded(&self) -> bool {
        !self.airborne
    }
    pub fn fsm(&self) -> &Fsm {
        &self.fsm
    }
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
    /// Update stats (RunSpeed, health, Features, ...) from the stat stream; speeds are recomputed.
    pub fn set_stats(&mut self, f: impl FnOnce(&mut Stats)) {
        let old = self.stats.flags;
        f(&mut self.stats);
        self.recalc();
        // stat hook `FUN_10059e6a`, stat 0 (Flags): falling / surface collision switches
        let changed = old ^ self.stats.flags;
        if changed & FLAG_NO_FALL != 0 {
            if self.stats.flags & FLAG_NO_FALL != 0 {
                self.disable_falling();
            } else if self.fsm.mode != mode::FLY {
                self.enable_falling();
            }
        }
        if changed & FLAG_NO_SURFACE != 0 {
            self.surface_collision = self.stats.flags & FLAG_NO_SURFACE == 0;
        }
    }
    /// Current horizontal speed in m/s.
    pub fn speed(&self) -> f32 {
        (self.vel[0] * self.vel[0] + self.vel[1] * self.vel[1]).sqrt()
    }
    /// `Vehicle_t::GetDir`: the retained native direction, including while stopped.
    pub fn vehicle_direction(&self) -> i32 { self.dir }
    /// Native liquid state from the last surface-alignment query, not a render-time feet-depth estimate.
    pub fn effect_liquid(&self) -> Option<(f32, u32, [f32; 3])> {
        self.surface.liquid_info.map(|liquid| (self.surface.submersion, liquid.kind, liquid.normal))
    }

    /// `Vehicle +0x170` reference speed of the current mode (m/s), the clip rate divisor.
    pub fn ref_speed(&self) -> f32 {
        self.ref_speed
    }
    /// `Vehicle +0x3C` maximum speed of the current mode (m/s).
    pub fn max_speed(&self) -> f32 {
        self.max_vel
    }
    /// Clip role for the current state. The original also plays turn-in-place clips (anim ids 0xC4/0xC5), which have no
    /// [`Role`]; those states map to idle. Strafing plays WalkLeft/WalkRight in every mode (anim ids 0x86/0x87).
    pub fn role(&self) -> Role {
        use mode::*;
        let f = &self.fsm;
        let moving_h = f.fwd == 2 || f.strafe == 2;
        if self.airborne || f.jump == 3 {
            return if f.mode == FLY { Role::Hover } else if f.fwd == 2 { Role::JumpForward } else { Role::JumpStand };
        }
        match f.mode {
            SIT_GROUND => Role::SitGround,
            CRAWL => Role::Crawl,
            SLEEP => Role::SleepGround,
            LOUNGE => Role::Lounge,
            FLY => Role::Hover,
            SWIM => {
                if moving_h {
                    Role::Swim
                } else {
                    Role::IdleSwim
                }
            }
            _ => {
                let fast = f.mode == RUN;
                if f.fwd == 2 {
                    match (f.fwd_dir, f.mode) {
                        (2, _) => {
                            if fast {
                                Role::RunBack
                            } else {
                                Role::WalkBack
                            }
                        }
                        (_, SNEAK) => Role::Sneak,
                        _ => {
                            if fast {
                                Role::Run
                            } else {
                                Role::Walk
                            }
                        }
                    }
                } else if f.strafe == 2 {
                    if f.strafe_dir == 3 {
                        Role::WalkLeft
                    } else {
                        Role::WalkRight
                    }
                } else {
                    Role::Idle
                }
            }
        }
    }

    // ---- inputs --------------------------------------------------------------------------------------------------

    /// `N3Msg_MovementChanged(action, 0, 0, true)`: a key slot fired `action` (GUI slots always pass the 4th argument
    /// `true`). `now` is the game time in seconds. The resulting `CharDCMove` (if any) is returned by the next
    /// [`Movement::update`] / [`Movement::take_outgoing`].
    pub fn action(&mut self, action: u8, now: f32) {
        self.sync_clock(now);
        self.movement_changed(action, 0.0, 0.0);
    }

    /// `N3Msg_MouseMovement(dx, dy)`: mouse-look yaw delta `dx` in radians (positive = right); `dy` only matters in
    /// first person and is not applied (the pitch half of `VehicleForwardUpdate` is not ported).
    pub fn mouse_turn(&mut self, dx: f32, dy: f32, now: f32) {
        self.sync_clock(now);
        if !self.mouse_look {
            // First event: keyboard turning becomes strafing while the mouse steers.
            if self.fsm.turn_dir == 3 && self.in_turn != 0.0 {
                self.movement_changed(id::TURN_LEFT_STOP, 0.0, 0.0);
                self.movement_changed(id::STRAFE_LEFT_START, 0.0, 0.0);
            } else if self.fsm.turn_dir == 4 && self.in_turn != 0.0 {
                self.movement_changed(id::TURN_RIGHT_STOP, 0.0, 0.0);
                self.movement_changed(id::STRAFE_RIGHT_START, 0.0, 0.0);
            }
        }
        self.mouse_look = true;
        if self.stats.features & 6 == 0 {
            return;
        }
        let mut dx = dx;
        if self.stats.turn_speed != 0 {
            // GetStat(0x10B): |dx| <= TurnSpeed / 100000 * (n3EngineClientAnarchy + 0x68, unresolved -> mouse_scale).
            let lim = self.stats.turn_speed as f32 / 100000.0 * self.mouse_scale;
            dx = dx.clamp(-lim, lim);
        }
        let td = self.fsm.turn_dir;
        if dx >= 0.0 {
            if td != 0 && td != 3 {
                self.movement_changed(id::MOUSE_LOOK, dx, dy);
            } else {
                self.movement_changed(id::MOUSE_TURN_RIGHT_START, 0.0, 0.0);
            }
        } else if td == 0 || td == 4 {
            self.movement_changed(id::MOUSE_TURN_LEFT_START, 0.0, 0.0);
        } else {
            self.movement_changed(id::MOUSE_LOOK, dx, dy);
        }
    }

    /// `N3Msg_EndMouseMovement` (mouse-look button released): stop the mouse turn and the mouse-look strafes.
    pub fn end_mouse_look(&mut self, now: f32) {
        self.sync_clock(now);
        self.mouse_look = false;
        if self.fsm.turn == 4 {
            let a = if self.fsm.turn_dir == 4 { id::TURN_RIGHT_STOP } else { id::TURN_LEFT_STOP };
            self.movement_changed(a, 0.0, 0.0);
        }
        if self.fsm.strafe == 2 {
            self.movement_changed(id::STRAFE_LEFT_STOP, 0.0, 0.0);
            self.movement_changed(id::STRAFE_RIGHT_STOP, 0.0, 0.0);
        }
    }

    /// Walk/run toggle: switch to the other speed mode (0x18 walk / 0x19 run; the permission table refuses the one
    /// that is already active).
    pub fn toggle_run(&mut self, now: f32) {
        let a = if self.fsm.mode == mode::RUN { id::SWITCH_WALK } else { id::SWITCH_RUN };
        self.action(a, now);
    }

    /// The movement facts `N3Msg_SitToggle` (Gamecode 0x10028e0a) reads: `char+0x50` virtual `+0x9c` = FSM `IsMoving` (vehicle vtable slot 39 =
    /// `FUN_1006efe1` -> `fsm.vtable[7]`) refuses the toggle, the WaitState stat and the FSM mode pick sit or stand-up.
    /// `fighting` and the selected item are the caller's.
    pub fn sit_input(&self, fighting: bool) -> SitInput {
        SitInput { fighting, blocked: self.fsm.is_moving(), wait_state: self.stats.wait_state, fsm_mode: i32::from(self.fsm.mode), item: None }
    }

    /// `N3Msg_CrawlToggle` (Gamecode 0x278c9): no-op while swimming.
    pub fn crawl_toggle(&mut self, now: f32) {
        if self.fsm.mode == mode::SWIM {
            return;
        }
        let a = if self.stats.wait_state == 0xE { id::LEAVE_CRAWL } else { id::SWITCH_CRAWL };
        self.action(a, now);
    }

    /// Server-driven state change (stat WaitState changes, `CharacterActionIIR_t`): runs the FSM transition without
    /// sending anything. Returns whether the transition was taken.
    pub fn transition(&mut self, id: u8) -> bool {
        if !self.fsm.allowed(id) {
            return false;
        }
        let Some((n, act)) = self.fsm.build(id, self.stats.features & 4 != 0) else { return false };
        let old = std::mem::replace(&mut self.fsm, n);
        self.apply_act(act, &old);
        true
    }

    /// `n3Dynel_t::GetBodyCollSphereRadi` of the own dynel (`SurfaceState::radius`), fed by the avatar model's torso sphere.
    pub fn set_body_radius(&mut self, r: f32) {
        self.surface.radius = r;
    }

    /// Place the character (spawn, teleport, playfield change): velocity and inputs are cleared.
    pub fn teleport(&mut self, pos: [f32; 3], yaw: f32) {
        // any running Impulse flight / FollowTarget path ends with the placement
        self.follow.clear();
        if self.ballistic.take().is_some_and(|b| b.restore_falling) {
            self.enable_falling();
        }
        self.pos = pos;
        self.yaw = wrap(yaw);
        self.vel = [0.0; 2];
        self.vy = 0.0;
        self.airborne = false;
        self.launch_y = pos[1];
        // `Vehicle +0x120` (the liquid callback flag) survives a placement: the FSM mode (swimming) does too
        self.surface = SurfaceState { in_liquid: self.surface.in_liquid, radius: self.surface.radius, ..SurfaceState::default() };
        self.jump_ready = true;
        self.fsm = Fsm { last_speed_mode: self.fsm.last_speed_mode, mode: self.fsm.mode, ..Fsm::new() };
        self.in_fwd = 0.0;
        self.in_strafe = 0.0;
        self.in_turn = 0.0;
        self.in_elev = 0.0;
        self.dir = 1;
        self.look_dirty = false;
        self.last_rot = rot_of(self.yaw);
        self.last_msg_pos = pos;
        self.recalc();
    }

    /// `SetPosIIR_c` apply [GC 0x10076e5a] for the own dynel: `stop` (flag `+0x2c`) runs `FUN_10059ae5(1)` = FullStop while moving, then vehicle
    /// vtable `+0x60` (`FUN_1006eed0`: `+0x174 = +0xd4 = y`, `LandNow(y)`: airborne ends, landing callback) and `SetRelPosRot(pos, current rot)`;
    /// the velocity is kept. The alignment against the ground is the next step's.
    pub fn set_pos(&mut self, pos: [f32; 3], stop: bool) {
        if stop {
            self.stop_if_moving();
        }
        self.pos = pos;
        self.airborne = false;
        self.vy = 0.0;
        self.launch_y = pos[1];
        self.on_land(pos[1]);
    }

    /// `FUN_10059ae5(1)`: while the vehicle `IsMoving` (vtable `+0x9c`) its vtable `+0x98` (`FUN_1006f008`) runs FSM `Transition(0x15)` FullStop.
    /// Used by SetPos (flag), the death action `0x63`, `FUN_10044a07` (Features bit 4 cleared) and Resurrect.
    pub fn stop_if_moving(&mut self) -> bool {
        self.fsm.is_moving() && self.transition(id::FULL_STOP)
    }

    /// `FollowTargetIIR_c` apply [GC 0x100732e3] part 1: `pos` (non-zero) is set with the current rotation (`SetRelPosRot`, then again by
    /// `FUN_1006fe02` as `SetRelPosIgnoreCollision`), before the gate and the mode check run.
    pub fn follow_place(&mut self, pos: [f32; 3]) {
        if pos != [0.0; 3] {
            self.pos = pos;
        }
    }

    /// `FollowTargetIIR_c` apply part 2 (after the caller's Features / district gate): dropped in FSM modes 1, 8, 9, 0xB, 0xC; else the FSM runs
    /// `Transition(mode)` (live ids 21 FullStop, 24 Walk, 25 Run) and `FUN_1006fe02` stores the target dynel (`chase`, a char instance) and
    /// the waypoints (at most 30, up to the first all-zero entry). The vehicle then steers to the target, or without one to the first
    /// waypoint; a manual movement action cancels both.
    pub fn follow_target(&mut self, mv: u8, path: &[[f32; 3]], target: Option<i32>) -> bool {
        if matches!(self.fsm.mode, 1 | 8 | 9 | 0xB | 0xC) {
            return false;
        }
        self.transition(mv);
        self.chase = target;
        self.chase_pos = None;
        self.follow = path.iter().take(FOLLOW_MAX).take_while(|p| **p != [0.0; 3]).copied().collect();
        if self.following() && self.fsm.fwd == 1 {
            self.transition(id::FORWARD_START);
        }
        true
    }

    /// The follow-target dynel the caller has to keep fed with [`Movement::set_chase_pos`].
    pub fn chase_target(&self) -> Option<i32> {
        self.chase
    }

    /// `n3Dynel_t::GetRelPos` of the follow target; `None` (the dynel is gone) drops the target (`FUN_1006fe02(0, ..)`).
    pub fn set_chase_pos(&mut self, p: Option<[f32; 3]>) {
        self.chase_pos = p;
        if p.is_none() {
            self.chase = None;
        }
    }

    /// `FUN_1006fe02(0, 2.0, 0)` without a path: the target is cleared (the per-frame Features / district gate of `FUN_10070fee`).
    pub fn drop_chase(&mut self) {
        self.chase = None;
        self.chase_pos = None;
    }

    /// A follow target or waypoint path steers the vehicle (`FUN_1006ef82` or the waypoint list).
    fn following(&self) -> bool {
        self.chase.is_some() || !self.follow.is_empty()
    }

    /// `Vehicle_t::Impulse(delta, time)` [VH 0x1000cd61]: a ballistic flight from the current position to `pos + (dx, 0, dz)` lasting `time`
    /// seconds replaces any path (`FUN_1000c41a`: the old one is deleted, `+0xac = 0`; the first path saves the falling flag and disables
    /// falling). Movement actions are refused until it ends. A non-positive `time` is ignored (the original divides by it).
    pub fn impulse(&mut self, delta: [f32; 3], time: f32) {
        if time.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) || !time.is_finite() || delta.iter().any(|d| !d.is_finite()) {
            return;
        }
        let restore_falling = self.ballistic.as_ref().map_or(self.falling_enabled, |b| b.restore_falling);
        if self.ballistic.is_none() {
            self.disable_falling();
        }
        let v0 = [delta[0] / time, -BALLISTIC_G * time * 0.5, delta[2] / time];
        self.ballistic = Some(Ballistic { start: self.pos, v0, dur: time, t: 0.0, restore_falling });
    }

    /// An `Impulse` flight is running (`Vehicle +0x108 != 0`).
    #[cfg(test)]
    pub fn pushed(&self) -> bool {
        self.ballistic.is_some()
    }

    /// Stats the transition `Apply`s wrote since the last call (WaitState, RestModifier): the caller stores them in the own stat table.
    pub fn take_stat_writes(&mut self) -> Vec<(u32, i32)> {
        std::mem::take(&mut self.stat_writes)
    }

    /// `SetStat` from an `Apply`: also mirrored into [`Stats`] where the FSM reads it back.
    fn write_stat(&mut self, id: u32, v: i32) {
        if id == STAT_MECH_DATA {
            self.stats.mech_data = v;
        }
        if id == STAT_WAIT_STATE {
            self.stats.wait_state = v;
        }
        self.stat_writes.push((id, v));
    }

    /// `Apply` of the sit family: `RestModifier` (when the function writes it) and `WaitState`.
    fn write_rest(&mut self, rest: Option<i32>, wait: i32) {
        if let Some(r) = rest {
            self.write_stat(STAT_REST_MODIFIER, r);
        }
        self.write_stat(STAT_WAIT_STATE, wait);
    }

    /// One `Vehicle_t::Run` while a `BallisticPath_t` is installed [VH 0x1000e849]: the body is set to the path position of `t`; while the flight
    /// goes on `EnsureSurfaceAlignment` (reduced: wall slide, never below the support) runs and a deviation of 0.5 m or more from the path point
    /// aborts it at the old position; at the end the path is deleted, the falling flag restored and the body lands.
    fn run_ballistic(&mut self, dt: f32, world: &dyn World) {
        let Some(b) = self.ballistic.as_mut() else { return };
        b.t += dt;
        let running = b.t <= b.dur;
        let t = b.t.min(b.dur);
        let g = [0.0, BALLISTIC_G, 0.0];
        let p: [f32; 3] = std::array::from_fn(|i| b.start[i] + b.v0[i] * t + 0.5 * g[i] * t * t);
        let old = self.pos;
        self.pos = p;
        let mut alive = running;
        if running {
            let body = Body { falling_enabled: self.falling_enabled, airborne: self.airborne, vy: self.vy, teleport: false };
            self.pos = world.align(old, p, &body, &mut self.surface).pos;
            self.liquid_callbacks();
            let d = (0..3).map(|i| (self.pos[i] - p[i]).powi(2)).sum::<f32>().sqrt();
            if d >= BALLISTIC_ABORT {
                self.pos = old;
                alive = false;
            }
        }
        if !alive {
            let restore = self.ballistic.take().is_some_and(|b| b.restore_falling);
            if restore {
                self.enable_falling();
            }
        }
    }

    /// `dynel + 0x220 / GetZoneInstanceID` changed (zone border crossed): arms the sync of `FUN_1005a5d6`.
    pub fn zone_instance(&mut self, zone: u32) {
        if zone != self.zone_inst {
            if self.zone_inst != 0 && self.zone_counter == -1 {
                self.zone_counter = 2;
            }
            self.zone_inst = zone;
        }
    }

    /// Messages produced since the last call.
    pub fn take_outgoing(&mut self) -> Vec<CharMove> {
        std::mem::take(&mut self.outbox)
    }

    // ---- per-frame -----------------------------------------------------------------------------------------------

    /// Advance by `dt` seconds (`Vehicle_t::Run`), then `CheckMotionUpdate`; returns every `CharDCMove` to send.
    pub fn update(&mut self, dt: f32, world: &dyn World) -> Vec<CharMove> {
        self.clock += dt.max(0.0) as f64;
        if self.snap_pending {
            self.snap_pending = false;
            if let Some(g) = world.ground(self.pos) {
                self.pos[1] = g + FOOT_CLEARANCE;
            }
        }
        // `Vehicle_t::Run` (Vehicle.dll @0x1000e849) reads the speed `+0xcc` before the frame (an `Impact` does not touch it). `+0xcc`
        // is the length of the horizontal-plane velocity `+0x64..+0x6c` only (FUN_1000e3d3); the vertical speed `+0x54` (jump/fall) is separate
        let still = self.vel == [0.0, 0.0];
        if let Some(h) = self.jump_pending.take() {
            self.launch_jump(h, world);
        }
        if self.ballistic.is_some() {
            if dt > 0.0 && dt <= MAX_DT {
                self.run_ballistic(dt, world);
            }
        } else if dt > 0.0 && dt <= MAX_DT {
            let free = !self.following();
            let mut left = dt;
            while left > 0.0 {
                let h = left.min(MAX_SUBSTEP);
                self.step(h, world);
                left -= h;
            }
            // ... and, in the free-roam branch (`+0x108 == 0`), runs the vtable `+0x70` callback when it went from 0 to > 0
            if still && free && self.vel != [0.0, 0.0] {
                self.start_moving_callback();
            }
        }
        // FUN_1005a5d6: two frames after a zone change a sync is sent.
        if self.zone_counter > 0 {
            self.zone_counter -= 1;
        }
        if self.zone_counter == 0 {
            self.emit(id::SYNC, 0.0, 0.0);
            self.zone_counter = -1;
        }
        self.check_motion_update();
        self.take_outgoing()
    }

    // ---- internals -----------------------------------------------------------------------------------------------

    fn sync_clock(&mut self, now: f32) {
        if now as f64 > self.clock {
            self.clock = now as f64;
        }
    }

    /// `n3EngineClientAnarchy_t::N3Msg_MovementChanged` [GC 0x18b5c].
    fn movement_changed(&mut self, mut action: u8, f1: f32, f2: f32) {
        let mut local = false;
        if action == id::MOUSE_LOOK {
            action = id::SYNC;
            local = true;
        }
        if !self.controllable || self.ballistic.is_some() || !self.fsm.allowed(action) {
            return;
        }
        if self.fsm.mode == mode::FLY && action == id::JUMP_START {
            return;
        }
        if action == id::JUMP_START && self.stats.mech_data != 0 {
            return;
        }
        if action == id::SWITCH_WALK && self.walk_locked {
            return;
        }
        if self.fsm.mode == mode::SIT_GROUND && action == id::SWITCH_SNEAK {
            return;
        }
        if self.mouse_look {
            action = match action {
                id::TURN_LEFT_START => id::STRAFE_LEFT_START,
                id::TURN_RIGHT_START => id::STRAFE_RIGHT_START,
                id::TURN_LEFT_STOP => id::STRAFE_LEFT_STOP,
                id::TURN_RIGHT_STOP => id::STRAFE_RIGHT_STOP,
                a => a,
            };
        }
        if local {
            // dynel->vtbl[0x74] = n3Dynel_t::VehicleForwardUpdate(zero, rot, dx, dy): the yaw part.
            self.yaw = wrap(self.yaw + f1);
            self.look_dirty = true;
        } else {
            self.emit(action, f1, f2);
        }
    }

    /// `FUN_1006ba23` (build from the current pose) + `FUN_1006b84b` (apply) + `SendIIRToServer` (write).
    fn emit(&mut self, action: u8, f1: f32, f2: f32) {
        let mut msg = CharMove { action, pos: self.pos, rot: self.rot(), elapsed_ms: 0, look: [f1, f2] };
        // FUN_1006bcc6 -> UpdateLastMotionMessageData
        self.last_msg_time = self.clock;
        self.last_rot = msg.rot;
        self.last_msg_pos = msg.pos;
        let feat = self.stats.features;
        if action != id::JUMP_START && action != id::SYNC {
            // FUN_1006b84b: a movement action cancels the FollowTarget path (the sync exemption is a [GUESS]: CheckMotionUpdate would cancel it every 5 s)
            if self.following() {
                // [GUESS] the ForwardStart the path began with is ended with it
                self.follow.clear();
                self.drop_chase();
                self.transition(id::FORWARD_STOP);
            }
        }
        if feat & 6 != 0 && (feat & 4 != 0 || (8 < action && action < 15)) {
            // action 0x16: VehicleForwardUpdate(pos, rot, 0, 0) changes nothing for the own, already placed dynel.
            let gm_fly_ok = self.stats.gm_level & 1 != 0;
            if !(action == id::SWITCH_FLY && !gm_fly_ok) {
                self.transition(action);
            }
        }
        // FUN_1006bc55: elapsed = (int)((now - last_write) * 1000), truncated (CVTTSD2SI).
        msg.elapsed_ms = ((self.clock - self.last_write) * 1000.0) as i32;
        self.last_write = self.clock;
        self.outbox.push(msg);
    }

    /// `n3EngineClientAnarchy_t::CheckMotionUpdate` [GC 0x18eb8].
    fn check_motion_update(&mut self) {
        let age = (self.clock - self.last_msg_time) as f32;
        if SYNC_PERIOD < age && self.fsm.is_moving() {
            self.movement_changed(id::SYNC, 0.0, 0.0);
            return;
        }
        if ROT_SYNC_MIN_AGE < age && self.look_dirty {
            let (a, b) = (self.rot(), self.last_rot);
            let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
            if dot.clamp(-1.0, 1.0).acos() > ROT_SYNC_ANGLE {
                self.movement_changed(id::SYNC, 0.0, 0.0);
                self.look_dirty = false;
            }
        }
    }

    fn forward(&self) -> [f32; 2] {
        [self.yaw.sin(), self.yaw.cos()]
    }

    /// `FUN_1006f4a2` (the speed part): per-mode maximum speed, reference speed (animation) and steering force.
    fn recalc(&mut self) {
        use mode::*;
        let rs = self.stats.eff_run_speed();
        let (reference, v) = match self.fsm.mode {
            RUN if self.fsm.fwd_dir == 2 => (3.0, (rs * 0.002_545_454_5 + 3.0).clamp(1.05, 9.1)),
            RUN => (5.0, (rs / 275.0 + 5.0).clamp(1.5, 13.0)),
            SWIM => (3.0, (rs / 440.0 + 3.0).clamp(1.5, 8.0)),
            CRAWL => (1.0, 1.0),
            FLY => (7.0, (rs / 275.0 + 7.0).clamp(1.5, 15.0)),
            _ => (1.5, 1.5),
        };
        self.ref_speed = reference;
        self.max_vel = v;
        self.force = (2.0 * MASS * v).min(100_000.0).min(10_000.0);
        if self.fsm.mode == FLY {
            self.falling_enabled = false;
            self.airborne = false;
            self.vy = 0.0;
        }
    }

    /// `FUN_1006f894`: strafe speed for the speed class `m` (2 walk/sneak, 3 run, 7 fly): half the forward speed,
    /// except walking (the full 1.5).
    fn strafe_speed(&self, m: u8) -> f32 {
        let rs = self.stats.eff_run_speed();
        let (base, div, max) = match m {
            2 => return 1.5,
            3 => (5.0, 275.0, 13.0),
            4 => (3.0, 440.0, 8.0),
            _ => (7.0, 275.0, 15.0),
        };
        (base * 0.5 + rs * (0.5 / div)).min(0.5 * max).max(0.75)
    }

    fn strafe_class(&self) -> u8 {
        match self.fsm.mode {
            mode::WALK | mode::SNEAK => 2,
            mode::FLY => 7,
            _ => 3,
        }
    }

    /// Turn rate (rad/s, negative = left) for a key turn: `FUN_1006d3fe` / `FUN_1006d4e9` / `FUN_1006c506`.
    fn turn_rate(&self, sign: f32) -> f32 {
        let st = self.stats.turn_speed as f32;
        let idle = self.fsm.fwd == 1;
        match (idle, self.stats.turn_speed == 0) {
            (true, true) => sign * 3.5,
            (true, false) => sign * st / 11000.0,
            (false, true) => sign * 1.5,
            (false, false) => sign * st / 11000.0 * 0.5,
        }
    }

    /// `FUN_1006c506`: after forward/back start/stop an active key turn switches between the standing and moving rate.
    fn retune_turn(&mut self) {
        if self.in_turn != 0.0 && self.fsm.turn != 1 {
            let sign = if self.fsm.turn_dir == 3 { -1.0 } else { 1.0 };
            self.in_turn = self.turn_rate(sign);
        }
    }

    /// `Vehicle_t::Halt`.
    fn halt(&mut self) {
        self.vel = [0.0; 2];
    }

    /// Player vehicle vtable `+0x70` = `FUN_1006ef34` [GC]: the vehicle began to move (walked off an edge, jumped from a standstill): FSM
    /// transition ForwardStart (1), ReverseStart (3) when `Vehicle_t::GetDir` is negative, through the permission table.
    fn start_moving_callback(&mut self) {
        self.transition(if self.dir < 0 { id::REVERSE_START } else { id::FORWARD_START });
    }

    fn enable_falling(&mut self) {
        if !self.falling_enabled {
            self.falling_enabled = true;
            self.airborne = true; // LandNow + FUN_1000a1a7: fly -> walk starts a fall
            self.vy = 0.0;
            self.launch_y = self.pos[1];
        }
    }

    /// `Vehicle_t::DisableFalling`.
    fn disable_falling(&mut self) {
        self.falling_enabled = false;
        self.airborne = false;
        self.vy = 0.0;
    }

    /// `FUN_1006f9e9` (vtable +0x2c), first half: a jump in progress (`PlayerVehicle +0x164 != 0`) ignores the request; otherwise it is
    /// remembered and [`Movement::launch_jump`] runs it at the next [`Movement::update`], the first point with a collision query for the
    /// ceiling raycast.
    fn jump_impulse(&mut self, h: f32) {
        if !self.jump_ready {
            return;
        }
        self.jump_ready = false;
        self.jump_pending = Some(h);
    }

    /// `FUN_1006f9e9` second half: `h` is shortened to the free height under the ceiling (`Surface_i::GetLineIntersection` from the position
    /// straight up 100 m; `hit.y - pos.y - 2 * BodyScale`, at least 0.1, f64 @GC 0x1015def8 / f32 0x10160810), then the launch speed is
    /// `sqrt(2 h |g|)` (`Vehicle_t::Impact((0, v * mass, 0))` adds `v` to the vertical speed while on the ground). The own dynel is a
    /// player, so the `dynel + 0x21c` NPC minimum of 1.5 m does not apply.
    fn launch_jump(&mut self, h: f32, world: &dyn World) {
        let scale = if self.stats.monster_scale != 0 { self.stats.monster_scale as f32 / 100.0 } else { 1.0 };
        let h = match world.ceiling(self.pos) {
            Some(y) => h.min((y - self.pos[1] - 2.0 * scale).max(JUMP_MIN_HEADROOM)),
            None => h,
        };
        self.launch_y = self.pos[1];
        if !self.airborne {
            self.vy += (2.0 * h * GRAVITY.abs()).sqrt();
            self.airborne = true;
        }
    }

    /// `Vehicle` landing callback `FUN_1006eef9`.
    fn on_land(&mut self, y: f32) {
        if self.fsm.jump == 3 {
            self.transition(id::JUMP_STOP);
        }
        self.jump_ready = true;
        self.launch_y = y;
    }

    /// The `Apply` virtual of the transition classes (`old` = the state before, `self.fsm` = the new state).
    fn apply_act(&mut self, act: Act, old: &Fsm) {
        match act {
            Act::ForwardStart => {
                self.set_direction(1);
                self.in_fwd = 1.0;
                self.recalc();
                self.retune_turn();
            }
            Act::ForwardStop => {
                self.in_fwd = 0.0;
                self.halt();
                self.recalc();
                self.retune_turn();
            }
            Act::ReverseStart => {
                self.in_fwd = -1.0;
                self.set_direction(-1);
                self.recalc();
                self.retune_turn();
            }
            Act::ReverseStop => {
                self.in_fwd = 0.0;
                self.halt();
                self.set_direction(1);
                self.recalc();
                self.retune_turn();
            }
            Act::TurnLeftStart => self.in_turn = self.turn_rate(-1.0),
            Act::TurnRightStart => self.in_turn = self.turn_rate(1.0),
            Act::MouseTurnLeftStart | Act::MouseTurnRightStart | Act::TurnStop => self.in_turn = 0.0,
            Act::StrafeLeftStart | Act::StrafeRightStart => {
                let sign = if act == Act::StrafeLeftStart { -1.0 } else { 1.0 };
                self.in_strafe = sign * self.strafe_speed(self.strafe_class());
                self.recalc();
            }
            Act::StrafeStop => self.in_strafe = 0.0,
            Act::JumpStart => {
                self.recalc();
                let h = self.stats.jump_height();
                self.jump_impulse(h);
            }
            Act::JumpStop => {}
            Act::ElevateUpStart => {
                self.recalc();
                self.in_elev = 3.0;
            }
            Act::ElevateUpStop => self.in_elev = -0.8,
            Act::FullStop => {
                self.halt();
                self.set_direction(1);
                self.in_fwd = 0.0;
                self.in_strafe = 0.0;
                self.in_turn = 0.0;
            }
            Act::ToFrozen => {
                self.halt();
                self.set_direction(1);
                self.in_fwd = 0.0;
                self.in_strafe = 0.0;
                self.in_turn = 0.0;
                self.leave_fly(old);
            }
            Act::ToWalk | Act::ToRun => {
                self.recalc();
                self.fsm.last_speed_mode = if act == Act::ToRun { mode::RUN } else { mode::WALK };
                self.leave_fly(old);
            }
            Act::ToSwim => {
                self.recalc();
                if old.strafe != 1 {
                    self.in_strafe = 0.0;
                }
                self.write_rest(None, 0); // SetStat(WaitState, 0) [FUN_1006dd0c]
                self.leave_fly(old);
                self.in_elev = 0.0;
            }
            Act::LeaveSwim | Act::LeaveFrozen => self.recalc(),
            Act::LeaveSit => {
                self.recalc();
                self.write_rest(Some(100), 0);
            }
            Act::ToSneak => {
                if old.strafe == 2 {
                    let sign = if old.strafe_dir == 3 { -1.0 } else { 1.0 };
                    self.in_strafe = sign * self.strafe_speed(2);
                }
                self.recalc();
            }
            Act::LeaveSneak => self.recalc(),
            Act::ToFly => {
                self.recalc();
                self.disable_falling();
                self.pos[1] += 0.5;
            }
            Act::LeaveFly => {
                self.recalc();
                self.enable_falling();
                self.pos[1] += 0.1;
                self.in_elev = 0.0;
            }
            Act::ToSitGround => {
                self.recalc();
                self.halt();
                self.write_rest(Some(25), 2);
            }
            Act::ToCrawl | Act::ToSleep | Act::ToLounge | Act::LeaveCrawl | Act::LeaveSleep | Act::LeaveLounge => {
                self.recalc();
                // WaitState / RestModifier writes of FUN_1006e646 / 1006e6ff / 1006e8a9 / 1006e3ff / 1006e79f / 1006e949
                match act {
                    Act::ToCrawl => self.write_rest(None, 0xE),
                    Act::ToSleep => self.write_rest(None, 0xF),
                    Act::ToLounge => self.write_rest(None, 0x10),
                    Act::LeaveLounge => self.write_rest(Some(100), 2),
                    _ => self.write_rest(Some(100), 0),
                }
                // CalculateGroundPoint + SetRelPos
                self.snap_pending = true;
            }
        }
    }

    fn leave_fly(&mut self, old: &Fsm) {
        if old.mode == mode::FLY {
            self.enable_falling();
            self.pos[1] += 0.1;
            self.in_elev = 0.0;
        }
    }

    /// `FUN_10070fee` target branch: `FUN_1007022d(1)` picks the steering point, `Vehicle_t::SteeringDirArrive` turns it into a force.
    /// * Target dynel (`FUN_10070185`): the point is `pos + n * (|d| - 4)` along `d = target - pos` (`DAT_10160a90` = 4.0), `pos` itself when
    ///   `|d|^2 <= 16` (`DAT_10160a98`).
    /// * Waypoints (`FUN_10070019`): a first waypoint closer than 1.0 m horizontally (`FUN_1006fd91`) is popped, repeatedly; an emptied list
    ///   runs ForwardStop (`FUN_10070c00`).
    ///
    /// `SteeringDirArrive` [VH 0x1000ac8c]: halt when the point was crossed since the previous step (`(p - prev) . (p - cur) < 0`, xz),
    /// else `SteeringArrive(radius 0.2)` [VH 0x1000ab28]: halt below 0.2 m (or d^2 < 0.01), else the desired velocity `d / |d| *
    /// min(|d| / (maxVel / 4) * maxVel, maxVel)` and the force `(desired - v) * mass * 4`, truncated to the maximum force and integrated
    /// as `v += F / mass * dt` (`FUN_1000e3d3`). [GUESS] the brake distance `this+0x40` is `maxVel / 4` (`FUN_1006f36f`'s caller was not
    /// traced); the vertical force component is dropped (`vy` is owned by gravity / ground following).
    fn steer_follow(&mut self, h: f32) {
        /// f32 0.2 @ Vehicle.dll 0x10012298, passed by `SteeringDirArrive`.
        const ARRIVE_RADIUS: f32 = 0.2;
        let pos = self.pos;
        let point = if self.chase.is_some() {
            match self.chase_pos {
                Some(t) => {
                    let d = [t[0] - pos[0], t[1] - pos[1], t[2] - pos[2]];
                    let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    if d2 > 16.0 {
                        let k = (d2.sqrt() - 4.0) / d2.sqrt();
                        [pos[0] + d[0] * k, pos[1] + d[1] * k, pos[2] + d[2] * k]
                    } else {
                        pos
                    }
                }
                None => pos,
            }
        } else {
            while let Some(&w) = self.follow.first() {
                let (dx, dz) = (w[0] - pos[0], w[2] - pos[2]);
                if dx * dx + dz * dz >= 1.0 {
                    break;
                }
                self.follow.remove(0);
            }
            match self.follow.first() {
                Some(&w) => w,
                None => {
                    self.transition(id::FORWARD_STOP);
                    return;
                }
            }
        };
        let prev = self.prev_pos;
        let crossed = (point[0] - prev[0]) * (point[0] - pos[0]) + (point[2] - prev[2]) * (point[2] - pos[2]) < 0.0;
        let to = [point[0] - pos[0], point[1] - pos[1], point[2] - pos[2]];
        let d2 = to[0] * to[0] + to[1] * to[1] + to[2] * to[2];
        if crossed || d2 < ARRIVE_RADIUS * ARRIVE_RADIUS || d2 < 0.01 {
            self.halt();
            return;
        }
        let d = d2.sqrt();
        let brake = self.max_vel * 0.25;
        let k = (d / brake * self.max_vel).min(self.max_vel) / d;
        let steer = [(to[0] * k - self.vel[0]) * MASS * 4.0, (to[2] * k - self.vel[1]) * MASS * 4.0];
        let f = truncate(steer, self.force);
        self.vel = truncate([self.vel[0] + f[0] / MASS * h, self.vel[1] + f[1] / MASS * h], self.max_vel);
    }

    /// `Vehicle_t::SetDirection`: a direction change halts.
    fn set_direction(&mut self, d: i32) {
        if d != self.dir {
            self.halt();
        }
        self.dir = d;
    }

    /// The liquid medium callbacks of the player vehicle that `EnsureSurfaceAlignment` fires through its vtable (`Vehicle +0x80` / `+0x84`,
    /// `SurfaceState::event`): entering deep water (`FUN_1006f99e` [GC]) clears MechData (the dynel is a player: `dynel+0x21c == 0`) and
    /// runs SwitchToSwimMode (0x1a); leaving it (`FUN_1006ef74`) runs LeaveSwimMode (0x23). Both go through the permission table.
    fn liquid_callbacks(&mut self) {
        match self.surface.event.take() {
            Some(LiquidEvent::Enter) => {
                if self.stats.mech_data != 0 {
                    self.write_stat(STAT_MECH_DATA, 0);
                }
                self.transition(id::SWITCH_SWIM);
            }
            Some(LiquidEvent::Leave) => {
                self.transition(id::LEAVE_SWIM);
            }
            None => {}
        }
    }

    /// One `Vehicle_t` sub-step (`FUN_1000e3d3` loop body) followed by `EnsureSurfaceAlignment`.
    fn step(&mut self, h: f32, world: &dyn World) {
        let old = self.pos;
        let mode = self.fsm.mode;
        // `FUN_10070fee` target / path branch (not in modes 1, 8, 9)
        if self.following() && !matches!(mode, 1 | 8 | 9) {
            self.steer_follow(h);
        }
        // gravity
        if self.airborne {
            self.vy = (self.vy + GRAVITY * h).clamp(-VY_LIMIT, VY_LIMIT);
        }
        // CalcSteering (PlayerVehicle vtbl[0x13]): forward / reverse thrust along the body forward.
        let steer = !matches!(mode, 1 | 8 | 9) && self.in_fwd != 0.0 && !self.following();
        if steer || self.airborne {
            if steer {
                let f = self.forward();
                let k = self.force * self.in_fwd.signum() / MASS * h;
                self.vel[0] += f[0] * k;
                self.vel[1] += f[1] * k;
            }
            self.vel = truncate(self.vel, self.max_vel);
        }
        let speed = self.speed();
        let moving = speed > SPEED_EPS;
        let mut v = self.vel;
        // vtbl[0x14]: strafe (+ elevate) as a direct velocity.
        if self.in_strafe != 0.0 || self.in_elev != 0.0 {
            let f = self.forward();
            let right = [f[1], -f[0]];
            let mut s = [right[0] * self.in_strafe, right[1] * self.in_strafe];
            if !moving {
                s = truncate(s, self.max_vel);
            } else {
                let sum = [v[0] + s[0], v[1] + s[1]];
                let l = (sum[0] * sum[0] + sum[1] * sum[1]).sqrt();
                if l > 0.0 {
                    let k = speed / l;
                    v = [v[0] * k, v[1] * k];
                    s = [s[0] * k, s[1] * k];
                }
            }
            self.pos[0] += s[0] * h;
            self.pos[2] += s[1] * h;
            self.pos[1] += self.in_elev * h;
        }
        if moving || self.vy != 0.0 {
            self.pos[0] += v[0] * h;
            self.pos[2] += v[1] * h;
            self.pos[1] += self.vy * h;
        }
        // vtbl[0x15]: turning. Standing: the body turns; moving: the velocity vector turns and the body follows it.
        if self.in_turn.abs() > 1e-4 {
            let a = self.in_turn * h;
            if self.vel == [0.0, 0.0] {
                self.yaw = wrap(self.yaw + a);
            } else {
                let (s, c) = a.sin_cos();
                self.vel = [self.vel[0] * c + self.vel[1] * s, -self.vel[0] * s + self.vel[1] * c];
            }
        }
        self.align(old, world);
        self.prev_pos = old;
        // OrientationMode 0 (FUN_1000c616): the body faces the velocity (away from it while reversing).
        if self.vel != [0.0, 0.0] {
            let sg = if self.dir < 0 { -1.0 } else { 1.0 };
            self.yaw = (sg * self.vel[0]).atan2(sg * self.vel[1]);
        }
    }

    /// `Vehicle_t::EnsureSurfaceAlignment` through [`World::align`] (wall sweep, ground following, support test with the step
    /// tolerance `0.48 + 1.1547 * step length`), then the fall bookkeeping of the original: nothing carries a walking body ->
    /// `FUN_1000a1a7` starts a fall, ground under a falling body -> `LandNow`. The liquid medium (`Vehicle +0xfc`, wading / swimming)
    /// runs inside the world's `align`; its callbacks come back as [`Movement::liquid_callbacks`].
    fn align(&mut self, old: [f32; 3], world: &dyn World) {
        if !self.surface_collision {
            self.airborne = false;
            self.vy = 0.0;
            return;
        }
        let body = Body { falling_enabled: self.falling_enabled, airborne: self.airborne, vy: self.vy, teleport: false };
        let r = world.align(old, self.pos, &body, &mut self.surface);
        self.pos = r.pos;
        self.liquid_callbacks();
        if !self.falling_enabled {
            return;
        }
        if r.airborne {
            if !self.airborne {
                // walked off an edge
                self.airborne = true;
                self.vy = 0.0;
                self.launch_y = old[1];
            }
        } else if self.airborne {
            self.vy = 0.0;
            self.airborne = false;
            self.on_land(self.pos[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::outgoing::{char_dc_move, parse_char_dc_move};

    /// The support test of `EnsureSurfaceAlignment` on a floor at height `ground` (0.48 m tolerance, no sweep).
    fn carried(ground: f32, new: [f32; 3], body: &Body) -> Aligned {
        let on = new[1] - 0.48 <= ground && body.vy <= 0.1;
        let y = if body.falling_enabled && on { ground } else { new[1].max(ground) };
        Aligned { pos: [new[0], y, new[2]], airborne: !on, normal: [0.0, 1.0, 0.0], liquid: -9999.0 }
    }

    struct Flat(f32);
    impl World for Flat {
        fn ground(&self, _p: [f32; 3]) -> Option<f32> {
            Some(self.0)
        }
        fn ceiling(&self, _p: [f32; 3]) -> Option<f32> {
            None
        }
        fn align(&self, _old: [f32; 3], new: [f32; 3], body: &Body, _st: &mut SurfaceState) -> Aligned {
            carried(self.0, new, body)
        }
    }
    /// Wall at x >= 10.
    struct Wall;
    impl World for Wall {
        fn ground(&self, _p: [f32; 3]) -> Option<f32> {
            Some(0.0)
        }
        fn ceiling(&self, _p: [f32; 3]) -> Option<f32> {
            None
        }
        fn align(&self, _old: [f32; 3], mut new: [f32; 3], body: &Body, _st: &mut SurfaceState) -> Aligned {
            new[0] = new[0].min(10.0);
            carried(0.0, new, body)
        }
    }

    fn run(m: &mut Movement, w: &dyn World, secs: f32) -> Vec<CharMove> {
        let mut out = Vec::new();
        let n = (secs * 60.0).round() as usize;
        for _ in 0..n {
            out.extend(m.update(1.0 / 60.0, w));
        }
        out
    }

    #[test]
    fn full_update_restores_speed_axes_and_player_inputs() {
        let mut blob = [0u8; 42];
        blob[12..22].copy_from_slice(&[mode::RUN, 2, 1, 1, 0, 1, 0, 1, 0, 1]);
        blob[22..26].copy_from_slice(&(mode::RUN as i32).to_be_bytes());
        blob[26..30].copy_from_slice(&1.0f32.to_be_bytes());
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.teleport([0.0; 3], 0.0);
        m.restore_blob(&blob);
        assert_eq!((m.fsm.mode, m.fsm.last_speed_mode, m.fsm.fwd, m.in_fwd), (mode::RUN, mode::RUN, 2, 1.0));
        assert_eq!(m.max_speed(), 5.0);
        m.surface.event = Some(LiquidEvent::Enter);
        m.liquid_callbacks();
        assert_eq!(m.fsm.mode, mode::SWIM);
        m.surface.event = Some(LiquidEvent::Leave);
        m.liquid_callbacks();
        assert_eq!(m.fsm.mode, mode::RUN);
        blob[12] = mode::WALK;
        blob[25] = mode::WALK;
        m.restore_blob(&blob);
        m.surface.event = Some(LiquidEvent::Enter);
        m.liquid_callbacks();
        m.surface.event = Some(LiquidEvent::Leave);
        m.liquid_callbacks();
        assert_eq!((m.fsm.mode, m.fsm.last_speed_mode, m.max_speed()), (mode::WALK, mode::WALK, 1.5));
        let before = (m.fsm, m.vel, m.vy, m.in_fwd, m.max_speed());
        m.restore_blob(&blob[..25]);
        assert_eq!((m.fsm, m.vel, m.vy, m.in_fwd, m.max_speed()), before);
        blob[12] = 0xff;
        m.restore_blob(&blob);
        assert_eq!((m.fsm, m.vel, m.vy, m.in_fwd, m.max_speed()), before);
        blob[12] = mode::WALK;
        blob[0..4].copy_from_slice(&f32::NAN.to_be_bytes());
        m.restore_blob(&blob);
        assert_eq!((m.fsm, m.vel, m.vy, m.in_fwd, m.max_speed()), before);
        blob[0..4].copy_from_slice(&0.0f32.to_be_bytes());
        blob[26..30].copy_from_slice(&f32::INFINITY.to_be_bytes());
        m.restore_blob(&blob);
        assert_eq!((m.fsm, m.vel, m.vy, m.in_fwd, m.max_speed()), before);
        assert!(m.take_outgoing().is_empty());
    }

    #[test]
    fn walk_straight_stop_and_bytes() {
        let w = Flat(0.0);
        let mut m = Movement::new([100.0, 0.0, 200.0], 0.0, 0);
        m.action(id::FORWARD_START, 10.0);
        let out = run(&mut m, &w, 0.0);
        assert!(out.is_empty());
        let out = m.take_outgoing();
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].action, out[0].pos, out[0].rot), (1, [100.0, 0.0, 200.0], [0.0, 0.0, 0.0, 1.0]));
        assert_eq!(out[0].elapsed_ms, 10_000); // first write: now * 1000
        run(&mut m, &w, 2.0);
        assert!((m.speed() - 5.0).abs() < 1e-3, "run speed {}", m.speed());
        // accel 2*v for 0.5 s (1.25 m) + 1.5 s at 5 m/s
        let d = m.pos()[2] - 200.0;
        assert!((d - 8.75).abs() < 0.15, "dist {d}");
        assert!((m.pos()[0] - 100.0).abs() < 1e-3);
        assert_eq!(m.role(), Role::Run);
        m.action(id::FORWARD_STOP, 12.0);
        let out = m.take_outgoing();
        assert_eq!((out[0].action, out[0].elapsed_ms), (2, 2000));
        assert!((out[0].pos[2] - m.pos()[2]).abs() < 1e-5);
        assert_eq!(m.speed(), 0.0);
        assert_eq!(m.role(), Role::Idle);
        // wire round trip
        let bytes = char_dc_move(25988, &out[0]);
        let (t, flag, back) = parse_char_dc_move(&bytes).unwrap();
        assert_eq!((t.instance, flag, back), (25988, 1, out[0]));
    }

    #[test]
    fn heading_convention_matches_capture() {
        // Captured relayed move of char 33402 (zone_ithaca.rec, t=70451): quaternion y/w and the position it ended at.
        let q = [0.0, -0.781_885_74, 0.0, 0.623_421_8];
        let yaw = yaw_of(q);
        let mut m = Movement::new([864.926_33, 40.004_997, 692.192_14], yaw, 6);
        m.action(id::FORWARD_START, 1.0);
        run(&mut m, &Flat(40.004_997), 1.0);
        let d = [m.pos()[0] - 864.926_33, m.pos()[2] - 692.192_14];
        // capture: (860.4958, 691.1804) after 4.4 m: direction (-0.975, -0.222)
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
        assert!((d[0] / l + 0.975).abs() < 0.01 && (d[1] / l + 0.222).abs() < 0.01, "{d:?}");
        assert!((rot_of(yaw)[1] - q[1]).abs() < 1e-4);
    }

    #[test]
    fn turning_standing_and_moving() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::TURN_RIGHT_START, 0.0);
        run(&mut m, &w, 0.5);
        assert!((m.yaw() - 1.75).abs() < 0.02, "3.5 rad/s standing: {}", m.yaw());
        m.action(id::TURN_RIGHT_STOP, 1.0);
        let y = m.yaw();
        run(&mut m, &w, 0.5);
        assert_eq!(m.yaw(), y);
        // turning left while walking is slower (1.5 rad/s) and the velocity follows the heading
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::FORWARD_START, 0.0);
        run(&mut m, &w, 1.0);
        m.action(id::TURN_LEFT_START, 1.0);
        run(&mut m, &w, 1.0);
        assert!((m.yaw() + 1.5).abs() < 0.05, "{}", m.yaw());
        assert!((m.speed() - 5.0).abs() < 0.01);
    }

    #[test]
    fn jump_arc() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::JUMP_START, 0.0);
        let out = m.take_outgoing();
        assert_eq!(out[0].action, 0x0F);
        m.update(0.0, &w); // the launch runs in the next update (ceiling raycast)
        assert!(!m.grounded());
        assert_eq!(m.role(), Role::JumpStand);
        // default stats: h = 1 m, v0 = sqrt(2*1*20)
        let mut peak = 0.0f32;
        let mut t = 0.0;
        let mut landed = None;
        while t < 2.0 {
            m.update(1.0 / 120.0, &w);
            t += 1.0 / 120.0;
            peak = peak.max(m.pos()[1]);
            if m.grounded() && landed.is_none() {
                landed = Some(t);
            }
        }
        assert!((peak - 1.0).abs() < 0.05, "apex {peak}");
        let l = landed.unwrap();
        // the body lands once its feet are within the 0.48 m step tolerance of the ground (`EnsureSurfaceAlignment`)
        assert!((l - (40f32.sqrt() + (40.0 - 40.0 * 0.48f32).sqrt()) / 20.0).abs() < 0.02, "air time {l}");
        assert_eq!(m.fsm().jump, 1, "landing runs JumpStop");
        assert_eq!(m.pos()[1], 0.0);
        // can jump again
        m.action(id::JUMP_START, 3.0);
        assert_eq!(m.take_outgoing().len(), 1);
    }

    /// A standing jump stays in place: Vehicle `+0xcc` is the horizontal speed only (RE Vehicle.dll 0x1000e849/0x1000e3d3), so no
    /// ForwardStart callback fires and nothing moves forward after the landing.
    #[test]
    fn standing_jump_stays_in_place() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::JUMP_START, 0.0);
        m.take_outgoing();
        for _ in 0..360 {
            m.update(1.0 / 120.0, &w);
            assert_eq!(m.speed(), 0.0);
        }
        assert!(m.grounded());
        assert_eq!(m.fsm().fwd_dir, 0, "no ForwardStart");
        assert_eq!((m.pos()[0], m.pos()[2]), (0.0, 0.0));
    }

    #[test]
    fn jump_height_from_stats() {
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.set_stats(|s| {
            s.strength = 100;
            s.agility = 100;
        });
        let w = Flat(0.0);
        m.action(id::JUMP_START, 0.0);
        m.update(0.0, &w);
        let mut peak = 0.0f32;
        for _ in 0..240 {
            m.update(1.0 / 120.0, &w);
            peak = peak.max(m.pos()[1]);
        }
        assert!((peak - 2.0).abs() < 0.06, "(100+100)/200+1 = 2 m: {peak}");
    }

    /// `FUN_1006f9e9` numbers: `v0 = sqrt(2 h 20)`, apex `h`, and the ceiling clamp `hit.y - y - 2 * scale` (at least 0.1).
    #[test]
    fn jump_numbers_and_ceiling_clamp() {
        struct Ceil(f32);
        impl World for Ceil {
            fn ground(&self, p: [f32; 3]) -> Option<f32> {
                Flat(0.0).ground(p)
            }
            fn ceiling(&self, _p: [f32; 3]) -> Option<f32> {
                Some(self.0)
            }
            fn align(&self, o: [f32; 3], n: [f32; 3], b: &Body, st: &mut SurfaceState) -> Aligned {
                Flat(0.0).align(o, n, b, st)
            }
        }
        let apex = |w: &dyn World, scale: i32| {
            let mut m = Movement::new([0.0; 3], 0.0, 0);
            m.set_stats(|s| {
                s.strength = 100;
                s.agility = 100; // h = 2 m
                s.monster_scale = scale;
            });
            m.action(id::JUMP_START, 0.0);
            m.update(0.0, w);
            let v0 = m.vy;
            let mut peak = 0.0f32;
            for _ in 0..240 {
                m.update(1.0 / 240.0, w);
                peak = peak.max(m.pos()[1]);
            }
            (v0, peak)
        };
        let (v0, peak) = apex(&Flat(0.0), 0);
        assert!((v0 - 80f32.sqrt()).abs() < 1e-4 && (peak - 2.0).abs() < 0.06, "{v0} {peak}");
        // ceiling 3.5 m over the feet, body scale 1: free height 1.5 m
        let (v0, peak) = apex(&Ceil(3.5), 100);
        assert!((v0 - 60f32.sqrt()).abs() < 1e-4 && (peak - 1.5).abs() < 0.05, "{v0} {peak}");
        // a body scaled 1.5x needs 3 m of headroom: 0.5 m left
        let (v0, _) = apex(&Ceil(3.5), 150);
        assert!((v0 - 20f32.sqrt()).abs() < 1e-4, "{v0}");
        // a ceiling lower than the head: the 0.1 m floor
        let (v0, _) = apex(&Ceil(1.0), 100);
        assert!((v0 - 4f32.sqrt()).abs() < 1e-4, "{v0}");
        // a high ceiling does not matter
        assert!((apex(&Ceil(50.0), 100).0 - 80f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn speeds_from_stats() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 550);
        m.action(id::FORWARD_START, 0.0);
        run(&mut m, &w, 1.0);
        assert!((m.speed() - 7.0).abs() < 1e-3, "5 + 550/275");
        m.action(id::FORWARD_STOP, 1.0);
        m.action(id::SWITCH_WALK, 1.0);
        m.action(id::FORWARD_START, 1.0);
        run(&mut m, &w, 1.0);
        assert!((m.speed() - 1.5).abs() < 1e-3);
        assert_eq!(m.role(), Role::Walk);
        // hurt below 15 % life: run speed shrinks
        m.set_stats(|s| {
            s.health = 1;
            s.max_health = 100;
        });
        m.action(id::FORWARD_STOP, 2.0);
        m.action(id::SWITCH_RUN, 2.0);
        m.action(id::FORWARD_START, 2.0);
        run(&mut m, &w, 1.0);
        let rs = (1.0 / 15.0) * (550.0 + 1000.0) - 1000.0;
        assert!((m.speed() - (5.0f32 + rs / 275.0).max(1.5)).abs() < 1e-3, "{}", m.speed());
    }

    #[test]
    fn strafe_reverse_and_permissions() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::STRAFE_RIGHT_START, 0.0);
        run(&mut m, &w, 1.0);
        assert!((m.pos()[0] - 2.5).abs() < 1e-3, "run strafe = 0.5 * 5 m/s: {:?}", m.pos());
        assert_eq!(m.role(), Role::WalkRight);
        m.action(id::STRAFE_LEFT_STOP, 1.0); // wrong direction: refused
        assert_eq!(m.fsm().strafe, 2);
        m.action(id::STRAFE_RIGHT_STOP, 1.0);
        assert_eq!(m.fsm().strafe, 1);
        // reverse: 3 m/s, body keeps facing +Z, role RunBack
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::REVERSE_START, 0.0);
        run(&mut m, &w, 1.5);
        assert!((m.speed() - 3.0).abs() < 1e-3);
        assert!(m.pos()[2] < -3.0 && m.yaw().abs() < 1e-3, "{:?} {}", m.pos(), m.yaw());
        assert_eq!(m.role(), Role::RunBack);
        // table: no jump while swimming, no run switch while running, sit refused while moving
        let f = Fsm::new();
        assert!(!f.allowed(id::SWITCH_RUN) && f.allowed(id::SWITCH_WALK) && f.allowed(id::JUMP_START));
        let swim = Fsm { mode: mode::SWIM, ..f };
        assert!(!swim.allowed(id::JUMP_START) && swim.allowed(id::FORWARD_START));
        assert!(Fsm { fwd: 2, fwd_dir: 1, ..f }.build(id::SWITCH_SIT_GROUND, true).is_none());
        // unresolved/non-transition ids build nothing
        for i in [0x13, 0x14, 0x16, 0x1f, 0x20, 0x2b, 0x2c] {
            assert!(f.build(i, true).is_none(), "{i:#x}");
        }
        // elevate axis quirk: untouched axis copies the strafe axis
        let st = Fsm { strafe: 2, strafe_dir: 3, ..f };
        assert_eq!(st.build(id::TURN_LEFT_START, true).unwrap().0.elev, 2);
    }

    /// `N3Msg_SitToggle` through the shared decision (`action::sit_toggle`) and the movement facts of [`Movement::sit_input`].
    fn toggle_sit(m: &mut Movement, now: f32) -> Vec<ao_net::n3::action::Outgoing> {
        let out = ao_net::n3::action::sit_toggle(&m.sit_input(false));
        for o in &out {
            if let ao_net::n3::action::Outgoing::Move(a) = o {
                m.action(*a, now);
            }
        }
        out
    }

    #[test]
    fn sit_and_modes() {
        use ao_net::n3::action::{id as act, Outgoing};
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        assert_eq!(toggle_sit(&mut m, 0.0), [Outgoing::Move(id::SWITCH_SIT_GROUND)]);
        assert_eq!(m.fsm().mode, mode::SIT_GROUND);
        assert_eq!(m.role(), Role::SitGround);
        // Apply of SwitchToSitGround writes RestModifier 25 and WaitState 2 (FUN_1006e2be)
        assert_eq!(m.take_stat_writes(), [(STAT_REST_MODIFIER, 25), (STAT_WAIT_STATE, 2)]);
        assert_eq!(m.stats().wait_state, 2);
        m.action(id::FORWARD_START, 1.0); // refused while sitting
        assert_eq!(m.take_outgoing().len(), 1);
        assert_eq!(m.fsm().fwd, 1);
        // sitting: the toggle asks the server to stand up (CharacterActionIIR_t 0x57), no move
        let stand = toggle_sit(&mut m, 2.0);
        assert!(matches!(&stand[..], [Outgoing::Action(a)] if a.action == act::STAND_UP));
        assert!(m.take_outgoing().is_empty());
        // the server's 0x57 echo: WaitState 2 -> LeaveSit; RestModifier 100, WaitState 0
        assert!(m.transition(id::LEAVE_SIT));
        assert_eq!(m.take_stat_writes(), [(STAT_REST_MODIFIER, 100), (STAT_WAIT_STATE, 0)]);
        assert_eq!(m.fsm().mode, mode::WALK, "returns to the last speed mode (ctor: walk)");
        run(&mut m, &w, 0.1);
        // moving: the toggle is refused (vehicle vtable +0x9c = IsMoving) and sends nothing
        m.action(id::FORWARD_START, 3.0);
        assert!(toggle_sit(&mut m, 3.0).is_empty());
    }

    #[test]
    fn sleep_and_lounge_write_wait_state() {
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        assert!(m.transition(id::SWITCH_SIT_GROUND)); // sleep / lounge are entered from sitting (walk refuses 0x21 / 0x22)
        m.take_stat_writes();
        assert!(m.transition(id::SWITCH_SLEEP));
        assert_eq!(m.take_stat_writes(), [(STAT_WAIT_STATE, 0xF)]);
        // sleep -> stand up request (WaitState 0xF), the echo leaves sleep for sitting and writes WaitState 0
        assert!(matches!(&ao_net::n3::action::sit_toggle(&m.sit_input(false))[..], [ao_net::n3::action::Outgoing::Action(_)]));
        assert!(m.transition(id::LEAVE_SLEEP));
        assert_eq!((m.fsm().mode, m.take_stat_writes()), (mode::SIT_GROUND, vec![(STAT_REST_MODIFIER, 100), (STAT_WAIT_STATE, 0)]));
        assert!(m.transition(id::SWITCH_LOUNGE));
        assert_eq!(m.take_stat_writes().last(), Some(&(STAT_WAIT_STATE, 0x10)));
        assert!(m.transition(id::LEAVE_LOUNGE));
        assert_eq!(m.take_stat_writes(), [(STAT_REST_MODIFIER, 100), (STAT_WAIT_STATE, 2)]);
    }

    #[test]
    fn set_pos_places_keeps_rotation_and_stops() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 1.0, 0);
        m.action(id::FORWARD_START, 0.0);
        run(&mut m, &w, 0.5);
        assert!(m.fsm().is_moving());
        m.set_pos([50.0, 0.0, 60.0], true);
        assert_eq!(m.pos(), [50.0, 0.0, 60.0]);
        assert!((m.yaw() - 1.0).abs() < 1e-3, "SetPos keeps the rotation");
        assert!(!m.fsm().is_moving(), "flag +0x2c: FullStop while moving");
        // without the flag the movement goes on
        m.action(id::FORWARD_START, 1.0);
        m.set_pos([0.0; 3], false);
        assert!(m.fsm().is_moving());
    }

    #[test]
    fn impulse_flies_the_ballistic_arc_and_refuses_actions() {
        let w = Flat(0.0);
        let mut m = Movement::new([10.0, 0.0, 10.0], 0.0, 0);
        m.impulse([6.0, 5.0, -3.0], 1.0); // y of the delta is ignored
        assert!(m.pushed());
        m.action(id::FORWARD_START, 0.0);
        assert_eq!(m.fsm().fwd, 1, "no movement action during the flight (Vehicle +0x108 != 0)");
        // half way: x/z linear, y = v0 t + g t^2 / 2 with v0 = -g T / 2: the apex 9.81 / 8
        run(&mut m, &w, 0.5);
        let p = m.pos();
        assert!((p[0] - 13.0).abs() < 0.2 && (p[2] - 8.5).abs() < 0.2, "{p:?}");
        assert!((p[1] - 9.81 / 8.0).abs() < 0.05, "{p:?}");
        run(&mut m, &w, 0.7);
        assert!(!m.pushed(), "path deleted after T");
        let p = m.pos();
        assert!((p[0] - 16.0).abs() < 0.3 && (p[2] - 7.0).abs() < 0.3 && p[1].abs() < 0.2, "{p:?}");
        m.action(id::FORWARD_START, 5.0);
        assert_eq!(m.fsm().fwd, 2, "controllable again");
        // a bad duration is ignored
        m.impulse([1.0, 0.0, 1.0], 0.0);
        assert!(!m.pushed());
    }

    #[test]
    fn impulse_aborts_on_a_wall() {
        let mut m = Movement::new([8.0, 0.0, 0.0], 0.0, 0);
        m.impulse([10.0, 0.0, 0.0], 1.0);
        run(&mut m, &Wall, 1.5);
        assert!(!m.pushed());
        assert!(m.pos()[0] <= 10.0 + 1e-3, "{:?}", m.pos());
    }

    #[test]
    fn follow_target_modes_path_and_cancel() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        // mode 25 = run, path to (0, 0, 20): the vehicle steers to it without keys and returns to idle
        assert!(m.follow_target(25, &[[0.0, 0.0, 20.0]], None));
        assert_eq!(m.fsm().mode, mode::RUN);
        run(&mut m, &w, 6.0);
        assert!((m.pos()[2] - 20.0).abs() < 1.5 && m.pos()[0].abs() < 0.2, "{:?}", m.pos());
        assert_eq!(m.fsm().fwd, 1, "ForwardStop at the end of the path");
        // the first waypoint is zero-terminated, a key press cancels the path
        assert!(m.follow_target(24, &[[0.0, 0.0, 40.0], [0.0; 3], [9.0, 9.0, 9.0]], None));
        m.action(id::STRAFE_LEFT_START, 7.0);
        let z = m.pos()[2];
        run(&mut m, &w, 1.0);
        assert!((m.pos()[2] - z).abs() < 0.1, "path cancelled");
        // dropped in sit / sleep / lounge / frozen
        m.transition(id::FULL_STOP);
        assert!(m.transition(id::SWITCH_SIT_GROUND));
        assert!(!m.follow_target(25, &[[1.0, 0.0, 1.0]], None));
        // the placement part runs before any gate
        m.follow_place([5.0, 0.0, 5.0]);
        assert_eq!(m.pos(), [5.0, 0.0, 5.0]);
        m.follow_place([0.0; 3]);
        assert_eq!(m.pos(), [5.0, 0.0, 5.0], "a zero position places nothing");
    }

    /// `FUN_10070185` + `SteeringDirArrive`: a follow target is approached to 4 m short of it and the vehicle halts there; a target that moves
    /// away is followed again; a vanished target drops the chase.
    #[test]
    fn follow_target_dynel_stops_four_metres_short() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        assert!(m.follow_target(25, &[], Some(7)));
        assert_eq!(m.chase_target(), Some(7));
        m.set_chase_pos(Some([0.0, 0.0, 20.0]));
        run(&mut m, &w, 8.0);
        let z = m.pos()[2];
        assert!((z - 16.0).abs() < 0.5 && m.pos()[0].abs() < 0.2, "{:?}", m.pos());
        assert!(m.speed() < 0.1, "halted: {}", m.speed());
        // inside 4 m (3-D distance) nothing moves
        m.set_chase_pos(Some([0.0, 0.0, 18.0]));
        run(&mut m, &w, 1.0);
        assert!((m.pos()[2] - z).abs() < 0.05);
        // the target walks off: followed again
        m.set_chase_pos(Some([0.0, 0.0, 40.0]));
        run(&mut m, &w, 3.0);
        assert!(m.pos()[2] > z + 5.0, "{:?}", m.pos());
        m.set_chase_pos(None);
        assert_eq!(m.chase_target(), None);
    }

    #[test]
    fn flags_stat_switches_falling_and_surface_collision() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0, 5.0, 0.0], 0.0, 0);
        m.set_stats(|s| s.flags = FLAG_NO_FALL);
        run(&mut m, &w, 1.0);
        assert_eq!(m.pos()[1], 5.0, "DisableFalling: no gravity");
        m.set_stats(|s| s.flags = 0);
        run(&mut m, &w, 2.0);
        assert!(m.pos()[1] < 0.5, "falling again: {:?}", m.pos());
        m.set_stats(|s| s.flags = FLAG_NO_SURFACE);
        m.action(id::FORWARD_START, 0.0);
        run(&mut m, &Wall, 8.0);
        assert!(m.pos()[2] > 10.0 || m.pos()[0].abs() < 1e-3, "no surface collision: {:?}", m.pos());
    }

    #[test]
    fn sync_cadence() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::FORWARD_START, 100.0);
        m.take_outgoing();
        let out = run(&mut m, &w, 5.2);
        assert_eq!(out.iter().map(|o| o.action).collect::<Vec<_>>(), vec![0x16], "sync after 5 s of motion");
        assert!((5000..5100).contains(&out[0].elapsed_ms), "{}", out[0].elapsed_ms);
        // standing still: nothing
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        assert!(run(&mut m, &w, 8.0).is_empty());
    }

    #[test]
    fn mouse_look_rotation_sync() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.mouse_turn(0.05, 0.0, 1.0); // first event sends MouseTurnRightStart (dx dropped)
        let out = m.take_outgoing();
        assert_eq!(out.iter().map(|o| o.action).collect::<Vec<_>>(), vec![id::MOUSE_TURN_RIGHT_START]);
        assert_eq!(m.yaw(), 0.0);
        for i in 0..10 {
            m.mouse_turn(0.05, 0.0, 1.0 + i as f32 * 0.01); // local rotation, no message
        }
        assert!(m.take_outgoing().is_empty());
        assert!((m.yaw() - 0.5).abs() < 1e-5);
        let out = run(&mut m, &w, 0.5);
        assert_eq!(out.iter().map(|o| o.action).collect::<Vec<_>>(), vec![0x16], "0.5 rad > 0.17 -> sync after 0.25 s");
        assert!((yaw_of(out[0].rot) - 0.5).abs() < 1e-4);
        // opposite direction starts a left mouse turn; button release stops it
        m.mouse_turn(-0.05, 0.0, 3.0);
        m.end_mouse_look(3.1);
        let acts: Vec<u8> = m.take_outgoing().iter().map(|o| o.action).collect();
        assert_eq!(acts, vec![id::MOUSE_TURN_LEFT_START, id::TURN_LEFT_STOP]);
        assert_eq!(m.fsm().turn, 1);
        // small drifts stay silent
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.mouse_turn(0.0, 0.0, 0.0);
        m.take_outgoing();
        m.mouse_turn(0.1, 0.0, 0.1);
        assert!(run(&mut m, &w, 1.0).is_empty(), "0.1 rad < 0.17");
    }

    #[test]
    fn keys_become_strafes_under_mouse_look() {
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::TURN_LEFT_START, 0.0);
        assert_eq!(m.fsm().turn, 4);
        m.mouse_turn(0.01, 0.0, 0.1); // stops the key turn, starts a strafe, then a mouse turn
        let acts: Vec<u8> = m.take_outgoing().iter().map(|o| o.action).collect();
        assert_eq!(acts, vec![id::TURN_LEFT_START, id::TURN_LEFT_STOP, id::STRAFE_LEFT_START, id::MOUSE_TURN_RIGHT_START]);
        m.action(id::TURN_LEFT_START, 0.2); // while mouse-looking: strafe left (already) -> refused as allowed? re-start ok
        m.action(id::TURN_RIGHT_START, 0.2);
        let acts: Vec<u8> = m.take_outgoing().iter().map(|o| o.action).collect();
        assert_eq!(acts, vec![id::STRAFE_LEFT_START, id::STRAFE_RIGHT_START]);
        assert_eq!(m.fsm().strafe_dir, 4);
    }

    #[test]
    fn wall_slide_and_ground_following() {
        let mut m = Movement::new([8.0, 0.0, 0.0], std::f32::consts::FRAC_PI_2, 0);
        m.action(id::FORWARD_START, 0.0);
        run(&mut m, &Wall, 1.0);
        assert!((m.pos()[0] - 10.0).abs() < 1e-4);
        // stepping off an edge starts a fall, stepping onto a stair snaps up
        struct Stair;
        impl World for Stair {
            fn ground(&self, p: [f32; 3]) -> Option<f32> {
                Some(if p[2] > 2.0 { 0.3 } else { 0.0 })
            }
            fn align(&self, _o: [f32; 3], t: [f32; 3], body: &Body, _st: &mut SurfaceState) -> Aligned {
                carried(if t[2] > 2.0 { 0.3 } else { 0.0 }, t, body)
            }
            fn ceiling(&self, _p: [f32; 3]) -> Option<f32> {
                None
            }
        }
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.action(id::FORWARD_START, 0.0);
        run(&mut m, &Stair, 1.0);
        assert!((m.pos()[1] - 0.3).abs() < 1e-5 && m.grounded());
    }

    #[test]
    fn zone_change_sync() {
        let w = Flat(0.0);
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        m.zone_instance(7);
        m.zone_instance(9); // armed: two frames later a sync
        assert!(m.update(0.016, &w).is_empty());
        let out = m.update(0.016, &w);
        assert_eq!(out.iter().map(|o| o.action).collect::<Vec<_>>(), vec![0x16]);
        assert!(m.update(0.016, &w).is_empty());
    }

    /// `FUN_1006ef34` (vehicle vtable `+0x70`): a body whose horizontal speed (`+0xcc`) goes from 0 to > 0 without a key runs ForwardStart,
    /// or ReverseStart while `GetDir` is negative; one that stands on the ground, falls or jumps (vertical speed only) does not.
    #[test]
    fn starting_to_move_from_rest_runs_forward_or_reverse_start() {
        let mut m = Movement::new([0.0, 0.0, 0.0], 0.0, 0);
        run(&mut m, &Flat(0.0), 0.5);
        assert_eq!((m.fsm().fwd, m.fsm().fwd_dir), (1, 0), "standing on the ground: nothing starts");
        let mut m = Movement::new([0.0, 5.0, 0.0], 0.0, 0);
        run(&mut m, &Flat(0.0), 0.1);
        assert_eq!((m.fsm().fwd, m.fsm().fwd_dir), (1, 0), "falling from rest: vertical speed only, nothing starts");
        let mut m = Movement::new([0.0, 0.0, 0.0], 0.0, 0);
        m.in_fwd = 1.0; // thrust without the FSM having started a move
        run(&mut m, &Flat(0.0), 0.1);
        assert_eq!((m.fsm().fwd, m.fsm().fwd_dir), (2, 1), "speed 0 -> > 0 without a key: ForwardStart");
        let mut m = Movement::new([0.0, 0.0, 0.0], 0.0, 0);
        m.dir = -1;
        m.in_fwd = -1.0;
        run(&mut m, &Flat(0.0), 0.1);
        assert_eq!((m.fsm().fwd, m.fsm().fwd_dir), (2, 2), "while reversing: ReverseStart");
        let mut m = Movement::new([0.0, 0.0, 0.0], 0.0, 0);
        m.jump_impulse(2.0);
        run(&mut m, &Flat(0.0), 0.1);
        assert_eq!(m.fsm().fwd, 1, "a jump from a standstill does not fire the callback");
    }

    #[test]
    fn the_body_radius_survives_a_placement() {
        let mut m = Movement::new([0.0; 3], 0.0, 0);
        assert_eq!(m.surface.radius, 0.5);
        m.set_body_radius(0.8);
        m.teleport([1.0, 0.0, 1.0], 0.0);
        assert_eq!(m.surface.radius, 0.8);
    }
}
