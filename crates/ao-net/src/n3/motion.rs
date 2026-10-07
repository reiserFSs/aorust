//! Movement of *other* dynels: the SimpleChar movement FSM, the per-mode speeds and a deterministic dead-reckoning
//! [`Mover`] that turns `CharDCMoveIIR_t` / `FollowTargetIIR_c` / `SetWantedDirectionIIR_t` / `StatIIR_t` into a pose
//! and an animation state. Pure logic, no rendering. Every rule is documented with its client address in
//! `docs/zone/motion.md` (GC = Gamecode.dll, N3 = N3.dll, VH = Vehicle.dll).
//!
//! Conventions: Y up; heading `yaw = 2·atan2(qy, qw)` (see [`super::dynel::yaw`]), facing vector `(sin yaw, 0, cos yaw)`
//! (checked against the captured walks of players 33491/33402), yaw grows towards +X (a right turn).

use super::dynel::{yaw as quat_yaw, CharDCMove};
use super::misc::FollowTarget;
use ao_formats::playfield::collision::{Aligned, Body, SurfaceState};

/// Stat ids (decimal ids of the client's `fStatToString` table).
pub const STAT_MAX_HEALTH: i32 = 1;
pub const STAT_HEALTH: i32 = 27;
pub const STAT_RUN_SPEED: i32 = 156;
pub const STAT_CURRENT_MOVEMENT_MODE: i32 = 173;
pub const STAT_FEATURES: i32 = 224;
pub const STAT_TURN_SPEED: i32 = 267;

/// `CharDCMoveIIR_t` move type = id of a transition class of the movement FSM (switch in `FUN_1006c60f` [GC], verified
/// case by case, not through a jump-table guess). Ids 19, 20, 31, 32 and > 42 hit `default:` (no transition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveType {
    ForwardStart = 1,
    ForwardStop = 2,
    ReverseStart = 3,
    ReverseStop = 4,
    StrafeRightStart = 5,
    /// Stops a right strafe (needs strafe state "right").
    StrafeRightStop = 6,
    StrafeLeftStart = 7,
    /// Stops a left strafe.
    StrafeLeftStop = 8,
    TurnRightStart = 9,
    MouseTurnRightStart = 10,
    TurnRightStop = 11,
    TurnLeftStart = 12,
    MouseTurnLeftStart = 13,
    TurnLeftStop = 14,
    JumpStart = 15,
    JumpStop = 16,
    ElevateUpStart = 17,
    ElevateUpStop = 18,
    FullStop = 21,
    /// No FSM transition: `FUN_1006b84b` calls `vtable+0x74(relpos, rot, extra0, extra1)` of the dynel (a no-op stub for
    /// `SimpleChar_t`, N3 `n3Dynel_t` slot 29 = `OnCellChanged`-class stub); only the placement of every DC move happens.
    PlaceRot = 22,
    SwitchToFrozenMode = 23,
    SwitchToWalkMode = 24,
    SwitchToRunMode = 25,
    SwitchToSwimMode = 26,
    SwitchToCrawlMode = 27,
    SwitchToSneakMode = 28,
    SwitchToFlyMode = 29,
    SwitchToSitGroundMode = 30,
    SwitchToSleepMode = 33,
    SwitchToLoungeMode = 34,
    LeaveSwimMode = 35,
    LeaveSneakMode = 36,
    LeaveSitMode = 37,
    LeaveFrozenMode = 38,
    LeaveFlyMode = 39,
    LeaveCrawlMode = 40,
    LeaveSleepMode = 41,
    LeaveLoungeMode = 42,
}

impl MoveType {
    pub const ALL: [MoveType; 38] = {
        use MoveType::*;
        [
            ForwardStart, ForwardStop, ReverseStart, ReverseStop, StrafeRightStart, StrafeRightStop, StrafeLeftStart,
            StrafeLeftStop, TurnRightStart, MouseTurnRightStart, TurnRightStop, TurnLeftStart, MouseTurnLeftStart,
            TurnLeftStop, JumpStart, JumpStop, ElevateUpStart, ElevateUpStop, FullStop, PlaceRot, SwitchToFrozenMode,
            SwitchToWalkMode, SwitchToRunMode, SwitchToSwimMode, SwitchToCrawlMode, SwitchToSneakMode, SwitchToFlyMode,
            SwitchToSitGroundMode, SwitchToSleepMode, SwitchToLoungeMode, LeaveSwimMode, LeaveSneakMode, LeaveSitMode,
            LeaveFrozenMode, LeaveFlyMode, LeaveCrawlMode, LeaveSleepMode, LeaveLoungeMode,
        ]
    };

    /// `None` for the ids without a transition class.
    pub fn from_id(id: u8) -> Option<MoveType> {
        Self::ALL.iter().copied().find(|m| *m as u8 == id)
    }

    /// Class name of the client (`<name>TransitionAction_t`, RTTI) where one exists.
    pub fn name(self) -> &'static str {
        use MoveType::*;
        match self {
            ForwardStart => "ForwardStart",
            ForwardStop => "ForwardStop",
            ReverseStart => "ReverseStart",
            ReverseStop => "ReverseStop",
            StrafeRightStart => "StrafeRightStart",
            StrafeRightStop => "StrafeStop(right)",
            StrafeLeftStart => "StrafeLeftStart",
            StrafeLeftStop => "StrafeStop(left)",
            TurnRightStart => "TurnRightStart",
            MouseTurnRightStart => "MouseTurnRightStart",
            TurnRightStop => "TurnStop(right)",
            TurnLeftStart => "TurnLeftStart",
            MouseTurnLeftStart => "MouseTurnLeftStart",
            TurnLeftStop => "TurnStop(left)",
            JumpStart => "JumpStart",
            JumpStop => "JumpStop",
            ElevateUpStart => "ElevateUpStart",
            ElevateUpStop => "ElevateUpStop",
            FullStop => "FullStop",
            PlaceRot => "(no transition; placement hook)",
            SwitchToFrozenMode => "SwitchToFrozenMode",
            SwitchToWalkMode => "SwitchToWalkMode",
            SwitchToRunMode => "SwitchToRunMode",
            SwitchToSwimMode => "SwitchToSwimMode",
            SwitchToCrawlMode => "SwitchToCrawlMode",
            SwitchToSneakMode => "SwitchToSneakMode",
            SwitchToFlyMode => "SwitchToFlyMode",
            SwitchToSitGroundMode => "SwitchToSitGroundMode",
            SwitchToSleepMode => "SwitchToSleepMode",
            SwitchToLoungeMode => "SwitchToLoungeMode",
            LeaveSwimMode => "LeaveSwimMode",
            LeaveSneakMode => "LeaveSneakMode",
            LeaveSitMode => "LeaveSitMode",
            LeaveFrozenMode => "LeaveFrozenMode",
            LeaveFlyMode => "LeaveFlyMode",
            LeaveCrawlMode => "LeaveCrawlMode",
            LeaveSleepMode => "LeaveSleepMode",
            LeaveLoungeMode => "LeaveLoungeMode",
        }
    }
}

/// Movement mode (`CharMovementStatus_t+4`; the number of the SwitchTo* transition's mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Frozen = 1,
    Walk = 2,
    Run = 3,
    Swim = 4,
    Crawl = 5,
    Sneak = 6,
    Fly = 7,
    SitGround = 8,
    Sleep = 0xb,
    Lounge = 0xc,
}

impl Mode {
    pub fn from_u8(v: u8) -> Option<Mode> {
        use Mode::*;
        [Frozen, Walk, Run, Swim, Crawl, Sneak, Fly, SitGround, Sleep, Lounge].into_iter().find(|m| *m as u8 == v)
    }
}

/// `CharMovementStatus_t` [GC vftable 0x101606b4]: the FSM state the transitions test and rewrite.
/// Defaults = base constructor `FUN_1007038f` (mode 3 = run, every axis stopped) + `FUN_1006c16d` (`prev_mode` = 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub mode: Mode,
    /// `+0x30`: mode restored by the Leave* transitions; set to 2 by SwitchToWalk and 3 by SwitchToRun.
    pub prev_mode: Mode,
    /// 0 stopped, 1 forward, -1 backward (`+8` state 1/2, `+0xc` dir 1/2).
    pub forward: i8,
    /// 0 stopped, -1 left (dir 3), +1 right (dir 4) (`+0x10`, `+0x14`).
    pub strafe: i8,
    /// 0 none, -1 left (dir 3), +1 right (dir 4) (`+0x20`, `+0x24`).
    pub turn: i8,
    /// `+0x28 == 3`.
    pub jumping: bool,
    /// `+0x18 == 2 && +0x1c == 5`.
    pub elevating: bool,
}

impl Default for Status {
    fn default() -> Self {
        Status { mode: Mode::Run, prev_mode: Mode::Walk, forward: 0, strafe: 0, turn: 0, jumping: false, elevating: false }
    }
}

impl Status {
    /// The status serialised in the `SimpleCharFullUpdate` blob (`FUN_10070438` [GC] reads 10 bytes mode, fwd state, fwd dir,
    /// strafe state, strafe dir, elevate state, elevate dir, turn state, turn dir, jump state; after 12 leading zero bytes
    /// and followed by a big-endian `i32` = `prev_mode` in the captured blobs). `None` if the blob is shorter than 26 bytes
    /// or holds an unknown mode.
    pub fn from_blob(b: &[u8]) -> Option<Status> {
        let s = b.get(12..26)?;
        let dir = |state: u8, d: u8, neg: u8, pos: u8| -> i8 {
            if state == 1 {
                0
            } else if d == neg {
                -1
            } else if d == pos {
                1
            } else {
                0
            }
        };
        Some(Status {
            mode: Mode::from_u8(s[0])?,
            prev_mode: Mode::from_u8(s[13]).unwrap_or(Mode::Walk),
            forward: dir(s[1], s[2], 2, 1),
            strafe: dir(s[3], s[4], 3, 4),
            turn: dir(s[7], s[8], 3, 4),
            jumping: s[9] == 3,
            elevating: s[5] == 2 && s[6] == 5,
        })
    }

    /// `FUN_1006c469` (vtable slot 7): true while forward/strafe motion, a jump, or fly mode is active.
    pub fn is_moving(&self) -> bool {
        self.forward != 0 || self.strafe != 0 || self.jumping || self.mode == Mode::Fly
    }

    /// Run the transition guard of `FUN_1006c60f` for `t` and rewrite the status when it holds. `leave_ok` = Features bit 4
    /// (`FUN_10044b6e(4)`) of the object at `dynel+0x1ec` (the Leave*/SitGround guards read it; unresolved which object).
    /// Returns whether the transition was valid.
    pub fn transition(&mut self, t: MoveType, leave_ok: bool) -> bool {
        use MoveType::*;
        let s = *self;
        match t {
            ForwardStart if s.forward == 0 || s.forward == -1 => self.forward = 1,
            ForwardStop if s.forward == 1 => self.forward = 0,
            ReverseStart if s.forward == 0 || s.forward == 1 => self.forward = -1,
            ReverseStop if s.forward == -1 => self.forward = 0,
            StrafeRightStart if s.strafe == 0 || s.strafe == -1 => self.strafe = 1,
            StrafeRightStop if s.strafe == 1 => self.strafe = 0,
            StrafeLeftStart if s.strafe == 0 || s.strafe == 1 => self.strafe = -1,
            StrafeLeftStop if s.strafe == -1 => self.strafe = 0,
            TurnRightStart | MouseTurnRightStart => self.turn = 1,
            TurnLeftStart | MouseTurnLeftStart => self.turn = -1,
            TurnRightStop if s.turn == 1 => self.turn = 0,
            TurnLeftStop if s.turn == -1 => self.turn = 0,
            JumpStart if !(s.jumping && s.mode != Mode::Fly) => self.jumping = true,
            JumpStop if s.jumping => {
                if s.mode != Mode::Fly {
                    self.jumping = false;
                }
            }
            ElevateUpStart if s.mode == Mode::Fly => self.elevating = true,
            ElevateUpStop if s.elevating => self.elevating = false,
            FullStop => {
                self.forward = 0;
                self.strafe = 0;
                self.turn = 0;
                self.jumping = false;
                self.elevating = false;
            }
            SwitchToFrozenMode => self.mode = Mode::Frozen,
            SwitchToWalkMode => {
                self.mode = Mode::Walk;
                self.prev_mode = Mode::Walk;
            }
            SwitchToRunMode => {
                self.mode = Mode::Run;
                self.prev_mode = Mode::Run;
            }
            SwitchToSwimMode => self.mode = Mode::Swim,
            SwitchToCrawlMode if !s.jumping && s.forward == 0 => self.mode = Mode::Crawl,
            SwitchToSneakMode => self.mode = Mode::Sneak,
            SwitchToFlyMode => self.mode = Mode::Fly,
            SwitchToSitGroundMode if !s.is_moving() && leave_ok => self.mode = Mode::SitGround,
            SwitchToSleepMode if !s.jumping && s.forward == 0 => self.mode = Mode::Sleep,
            SwitchToLoungeMode if !s.is_moving() && !s.jumping && s.forward == 0 => self.mode = Mode::Lounge,
            LeaveSwimMode | LeaveSneakMode | LeaveFrozenMode | LeaveFlyMode => self.mode = s.prev_mode,
            LeaveSitMode => {
                if !leave_ok {
                    self.mode = Mode::Frozen;
                } else if s.mode == Mode::SitGround {
                    self.mode = s.prev_mode;
                }
            }
            LeaveCrawlMode if !s.is_moving() => {
                if !leave_ok {
                    self.mode = Mode::Frozen;
                } else if s.mode == Mode::Crawl {
                    self.mode = s.prev_mode;
                }
            }
            LeaveSleepMode if !s.is_moving() => {
                if !leave_ok {
                    self.mode = Mode::Frozen;
                } else if s.mode == Mode::Sleep {
                    self.mode = Mode::SitGround;
                }
            }
            LeaveLoungeMode if !s.is_moving() => {
                if !leave_ok {
                    self.mode = Mode::Frozen;
                } else if s.mode == Mode::Lounge {
                    self.mode = Mode::SitGround;
                }
            }
            PlaceRot => return false,
            _ => return false,
        }
        true
    }
}

/// Animation to play, named by the client's animation id (`FUN_100c01c9` [GC] fills the id → clip name table, 197 entries).
/// `Attack`/`Die` are for the application (combat messages); their clip depends on the weapon/NPC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimState {
    Idle,
    Walk,
    Run,
    WalkBack,
    RunBack,
    WalkLeft,
    WalkRight,
    TurnLeft,
    TurnRight,
    JumpStand,
    JumpForward,
    Swim,
    IdleSwim,
    Sneak,
    Crawl,
    IdleCrawl,
    Hover,
    Fly,
    SitGround,
    Sleep,
    Lounge,
    Attack,
    Die,
}

impl AnimState {
    /// Client animation id (key of the NPC record's animation table and of the client's id → clip name table).
    /// `None` for `Attack`/`Die`.
    pub fn anim_id(self) -> Option<u32> {
        use AnimState::*;
        Some(match self {
            Idle => 0x78,
            Walk => 0x64,
            Run => 0x65,
            WalkBack => 0x88,
            RunBack => 0xde,
            WalkLeft => 0x86,
            WalkRight => 0x87,
            TurnLeft => 0xc4,
            TurnRight => 0xc5,
            JumpStand => 0x9c,
            JumpForward => 0x9d,
            Swim => 0x85,
            IdleSwim => 0xc3,
            Sneak => 0x66,
            Crawl => 0x67,
            IdleCrawl => 0x9a,
            Hover => 0xb2,
            Fly => 0xb3,
            SitGround => 0xd7,
            Sleep => 0xee,
            Lounge => 0xf0,
            Attack | Die => return None,
        })
    }

    /// Clip name part of `<set>_<name>_01_01.ani` (`None` for `Attack`/`Die`).
    pub fn clip_name(self) -> Option<&'static str> {
        use AnimState::*;
        Some(match self {
            Idle => "idle-stand",
            Walk => "walk",
            Run => "run",
            WalkBack => "walk-back",
            RunBack => "run-back",
            WalkLeft => "walk-left",
            WalkRight => "walk-right",
            TurnLeft => "turn-left",
            TurnRight => "turn-right",
            JumpStand => "jump-stand",
            JumpForward => "jump-forward",
            Swim => "swim",
            IdleSwim => "idle-swim",
            Sneak => "sneakcool",
            Crawl => "crawl",
            IdleCrawl => "idle-crawl",
            Hover => "idle-hover",
            Fly => "hover-norm",
            SitGround => "idle-ground",
            Sleep => "idle-sleep-ground",
            Lounge => "idle-lounging",
            Attack | Die => return None,
        })
    }

    /// For NPCs whose record has no animation for this id the client falls back (`FUN_1006c065`: sit → idle-stand when the
    /// record has no 0xd7; `FUN_1006be27`: swim idle 0xc3 → 0x85 → 0x78). Returns the next id to try.
    pub fn fallback(self) -> Option<AnimState> {
        use AnimState::*;
        match self {
            SitGround | Sleep | Lounge | IdleCrawl | Hover | TurnLeft | TurnRight | WalkLeft | WalkRight => Some(Idle),
            IdleSwim => Some(Swim),
            Swim | Crawl | Fly | JumpStand | JumpForward | RunBack | WalkBack | Sneak => Some(Idle),
            _ => None,
        }
    }
}

/// `FUN_1006be27` (stationary/moving animation of a status) and `FUN_1006c065` (idle of a mode).
pub fn anim_of(s: &Status) -> AnimState {
    use AnimState::*;
    if s.jumping {
        return if s.forward == 0 { JumpStand } else { JumpForward };
    }
    if s.forward == 0 {
        return match s.mode {
            Mode::Walk | Mode::Run => match (s.strafe, s.turn) {
                (-1, _) => WalkLeft,
                (1, _) => WalkRight,
                (_, -1) => TurnLeft,
                (_, 1) => TurnRight,
                _ => Idle,
            },
            Mode::Sneak => Sneak,
            Mode::Swim => IdleSwim,
            Mode::Fly => Hover,
            Mode::Crawl => IdleCrawl,
            Mode::SitGround => SitGround,
            Mode::Sleep => Sleep,
            Mode::Lounge => Lounge,
            Mode::Frozen => Idle,
        };
    }
    let back = s.forward < 0;
    match s.mode {
        Mode::Walk if back => WalkBack,
        Mode::Walk => Walk,
        Mode::Run if back => RunBack,
        Mode::Run => Run,
        Mode::Swim => Swim,
        Mode::Sneak => Sneak,
        Mode::Crawl => Crawl,
        Mode::Fly => Fly,
        Mode::Frozen | Mode::SitGround | Mode::Sleep | Mode::Lounge => anim_of(&Status { forward: 0, ..*s }),
    }
}

/// Maximum speed in m/s of `FUN_1006f4a2` [GC] (`Vehicle_t::SetMaxVel`). `skill` = [`Mover::skill`] (RunSpeed with the low
/// health penalty), `back` = `Vehicle_t::GetDir() < 0`.
pub fn max_speed(mode: Mode, back: bool, skill: f32) -> f32 {
    let clamp = |v: f32, lo: f32, hi: f32| v.min(hi).max(lo);
    match mode {
        Mode::Run if back => clamp(3.0 + skill * 0.002_545_454_5, 1.05, 9.1),
        Mode::Run => clamp(5.0 + skill / 275.0, 1.5, 13.0),
        Mode::Swim => clamp(3.0 + skill / 440.0, 1.5, 8.0),
        Mode::Fly => clamp(7.0 + skill / 275.0, 1.5, 15.0),
        Mode::Crawl => 1.0,
        Mode::Walk | Mode::Sneak | Mode::Frozen | Mode::SitGround | Mode::Sleep | Mode::Lounge => 1.5,
    }
}

/// Lateral speed of a strafe (`FUN_1006f894` [GC], vtable slot +0xa0; the strafe velocity is `sign · this`): walk/sneak 1.5,
/// run `2.5 + skill/550` (≤ 6.5, ≥ 0.75), fly `3.5 + skill/550` (≤ 7.5).
pub fn strafe_speed(mode: Mode, skill: f32) -> f32 {
    match mode {
        Mode::Run => (2.5 + skill / 550.0).clamp(0.75, 6.5),
        Mode::Fly => (3.5 + skill / 550.0).clamp(0.75, 7.5),
        Mode::Swim => (1.5 + skill / 880.0).clamp(0.75, 4.0),
        _ => 1.5,
    }
}

/// Rendered state of a [`Mover`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Visual position (reconcile-smoothed). Y is the server's height (interpolated along a path), not terrain.
    pub pos: [f32; 3],
    /// Heading about Y, radians (convention in the module docs).
    pub yaw: f32,
    pub anim: AnimState,
    /// Current speed along the ground in m/s (for scaling the walk/run clip: the client uses `speed / base speed`).
    pub speed: f32,
}

const POP_RADIUS_MIN: f32 = 0.5;
/// `n3VisualDynel_t::Run` [N3 0x196bd]: while `0.04 ≤ |recon − pos|² < 1225` the drawn position is
/// `0.8·recon + 0.2·pos` per rendered frame (constants 0x1003d9c4/0x1003ce50/0x1003d9c8/0x1003d9cc).
const RECON_KEEP: f32 = 0.8;
const RECON_MIN_D2: f32 = 0.04;
const RECON_MAX_D2: f32 = 1225.0;
/// Frame rate the per-frame reconcile factor is normalised to (the client applies it once per rendered frame).
const RECON_HZ: f32 = 60.0;
/// `SteeringDirArrive` radius [VH 0x10012298] where the vehicle halts at the final waypoint.
const ARRIVE_RADIUS: f32 = 0.2;
/// `FUN_1006d3fe` [GC]: turn rate in rad/s of a stationary char without TurnSpeed stat, 1.5 while moving.
const TURN_RATE_STILL: f32 = 3.5;
const TURN_RATE_MOVING: f32 = 1.5;
/// Below this heading error (rad) a `SetWantedDirection` turn is done. **[GUESS]**
const TURN_DONE: f32 = 0.05;

fn len2(a: [f32; 2]) -> f32 {
    a[0] * a[0] + a[1] * a[1]
}

fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let r = (a + std::f32::consts::PI).rem_euclid(t) - std::f32::consts::PI;
    if r == -std::f32::consts::PI { std::f32::consts::PI } else { r }
}

/// Dead-reckoning state of one remote character. Feed it the dynel's messages in arrival order, call
/// [`Mover::advance`] once per frame.
#[derive(Clone, Debug)]
pub struct Mover {
    pos: [f32; 3],
    yaw: f32,
    status: Status,
    /// Current ground speed (signed along the facing: negative when reversing).
    speed: f32,
    /// Retained `Vehicle_t::GetDir` sign, independent of the current speed.
    direction: i32,
    /// Drawn position lags the simulated one by the reconcile offset (`n3VisualDynel_t+0xa0`).
    recon: Option<[f32; 3]>,
    path: Vec<[f32; 3]>,
    /// Target heading of the last `SetWantedDirection` (turn in place towards it).
    wanted_yaw: Option<f32>,
    run_speed: i32,
    health: Option<i32>,
    max_health: i32,
    turn_speed: i32,
    features: i32,
    sit_ok: bool,
    reported_mode: Option<i32>,
    /// Turn started by a mouse-turn type (10/13): the vehicle's turn rate is 0, the heading comes with the messages.
    mouse_turn: bool,
    surface: SurfaceState,
    vy: f32,
    airborne: bool,
    placement_pending: bool,
    placement_from: [f32; 3],
    jump_pending: bool,
    strength: i32,
    agility: i32,
    gm_level: i32,
    flags: u32,
    falling: bool,
    body_scale: f32,
    elevate_speed: f32,
}

impl Mover {
    /// A freshly created dynel (`SimpleCharFullUpdate`): position, heading, FSM defaults. Features default to 2 (an NPC-like
    /// dynel: only turn move types drive the FSM, see [`Mover::set_features`]).
    pub fn new(pos: [f32; 3], yaw: f32) -> Mover {
        Mover {
            pos,
            yaw,
            status: Status::default(),
            speed: 0.0,
            direction: 1,
            recon: None,
            path: vec![],
            wanted_yaw: None,
            run_speed: 0,
            health: None,
            max_health: 0,
            turn_speed: 0,
            features: 2,
            sit_ok: false,
            reported_mode: None,
            mouse_turn: false,
            surface: SurfaceState::default(),
            vy: 0.0,
            airborne: false,
            placement_pending: true,
            placement_from: pos,
            jump_pending: false,
            strength: super::world::STAT_UNSET,
            agility: super::world::STAT_UNSET,
            gm_level: super::world::STAT_UNSET,
            flags: 0,
            falling: true,
            body_scale: 1.0,
            elevate_speed: 0.0,
        }
    }

    /// Dynel body scale used by the native jump ceiling clearance.
    pub fn set_body_scale(&mut self, scale: f32) {
        self.body_scale = scale;
    }

    pub fn set_body_radius(&mut self, radius: f32) {
        self.surface.radius = radius;
    }

    /// As [`Mover::new`] with the FSM state read from the `SimpleCharFullUpdate` blob (see [`Status::from_blob`]).
    pub fn with_blob(pos: [f32; 3], yaw: f32, blob: &[u8]) -> Mover {
        let mut m = Mover::new(pos, yaw);
        m.restore_blob(blob);
        m
    }

    /// Restore after full-update stat hooks; GC 0x10077af2 sets Flags before +0xac.
    pub fn restore_blob(&mut self, blob: &[u8]) {
        if let Some(s) = Status::from_blob(blob) {
            self.status = s;
            self.direction = if s.forward < 0 { -1 } else { 1 };
            let velocity = [0, 4, 8].map(|offset| f32::from_be_bytes(blob[offset..offset + 4].try_into().unwrap()));
            if velocity.iter().all(|v| v.is_finite()) {
                self.speed = velocity[0].hypot(velocity[2]) * self.direction as f32;
                self.vy = velocity[1];
                self.airborne = velocity[1] != 0.0 || s.jumping;
            }
            if blob.len() >= 42 {
                let elevate = f32::from_be_bytes(blob[38..42].try_into().unwrap());
                if elevate.is_finite() { self.elevate_speed = elevate; }
            }
        }
    }

    /// Stat `Features` (0xE0, decimal 224): bit 4 lets every move type drive the FSM, bit 2 only the turn types 9..=14
    /// (`FUN_1006b84b` [GC]); with neither bit the message is placement only. NPC records carry it (`0x8003` / `0x8007`);
    /// players are **[INFERENCE]** bit 4 (their DC moves are applied live). Same as `on_stat(STAT_FEATURES, v)`.
    pub fn set_features(&mut self, features: i32) {
        self.features = features;
        if features & 8 != 0 {
            self.vy = 0.0;
            self.airborne = false;
        }
    }

    /// Allow a *DC move* type 30 to sit the dynel down. The client's guard is `Features & 4` of the dynel itself
    /// [GC 0x1006cd49..56: `FUN_10044b6e(0x1ec helper, 4)`; helper+0x40 = the dynel], i.e. true for most captured NPC records
    /// (Features 0x8007), which would then all sit down right after spawning (idle-ground, or idle-stand when the record has
    /// no animation 0xd7). That contradicts what a player sees, so the default is `false` (type 30 = plain placement);
    /// see docs/zone/motion.md §2. [`Mover::on_move_type`] always uses the real guard.
    pub fn set_sit_allowed(&mut self, ok: bool) {
        self.sit_ok = ok;
    }

    /// Run FSM transition `id` directly (what `CharacterActionIIR_t` actions 0x56/0x57 do through `vtable[6]`), with the
    /// guards' Features bit-4 test taken from the dynel's own Features stat (= `Features & 4`, the helper at `dynel+0x1ec`
    /// holds the dynel itself). Returns whether the transition was valid. No placement, no follow cancel.
    pub fn on_move_type(&mut self, id: u8) -> bool {
        MoveType::from_id(id).is_some_and(|t| self.apply_guarded(t, self.features & 4 != 0))
    }

    /// `StatIIR_t` pair. RunSpeed (156) is the speed skill, Health/MaxHealth (27/1) scale it below 15 % health,
    /// TurnSpeed (267) the turn rate, Features (224) the FSM gate. CurrentMovementMode (173) is kept for [`Mover::reported_mode`]
    /// only: the client never reads it (no `PUSH 0xad` in Gamecode/N3), the mode changes through move types.
    pub fn on_stat(&mut self, stat: i32, value: i32) {
        match stat {
            STAT_RUN_SPEED => self.run_speed = value,
            STAT_HEALTH => self.health = Some(value),
            STAT_MAX_HEALTH => self.max_health = value,
            STAT_TURN_SPEED => self.turn_speed = value,
            STAT_FEATURES => self.set_features(value),
            STAT_CURRENT_MOVEMENT_MODE => self.reported_mode = Some(value),
            0 => {
                self.flags = value as u32;
                self.falling = self.flags & 0x20000000 == 0;
                if !self.falling_enabled() || self.flags & 0x80000000 != 0 {
                    self.vy = 0.0;
                    self.airborne = false;
                }
            }
            16 => self.strength = value,
            17 => self.agility = value,
            215 => self.gm_level = value,
            360 => self.body_scale = if value == 0 { 1.0 } else { value as f32 / 100.0 },
            _ => {}
        }
    }

    pub fn reported_mode(&self) -> Option<i32> {
        self.reported_mode
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Simulated (not reconcile-smoothed) position.
    pub fn sim_pos(&self) -> [f32; 3] {
        self.pos
    }

    /// `FUN_1006edb3` [GC]: RunSpeed, reduced when `health / (0.15·max_health) < 1`: `f·(rs+1000) − 1000`.
    pub fn skill(&self) -> f32 {
        let rs = self.run_speed as f32;
        match self.health {
            Some(h) if self.max_health > 0 => {
                let f = h as f32 / (self.max_health as f32 * 0.15);
                if f < 1.0 { f * (rs + 1000.0) - 1000.0 } else { rs }
            }
            _ => rs,
        }
    }
    /// Native horizontal speed (`Vehicle +0xcc`), excluding jump/fall velocity.
    pub fn vehicle_speed(&self) -> f32 { self.speed.abs() }
    /// Native retained forward/reverse direction (`Vehicle +0x90`).
    pub fn vehicle_direction(&self) -> i32 { self.direction }


    fn place(&mut self, pos: [f32; 3]) {
        // UpdateReconcilePos stores the simulated position *before* the teleport (N3 0x19414).
        self.recon = Some(self.pos);
        self.placement_from = self.pos;
        self.pos = pos;
        self.placement_pending = true;
    }

    fn stop_follow(&mut self) {
        self.path.clear();
    }

    fn apply(&mut self, t: MoveType) -> bool {
        // Leave* guards test Features bit 4, which the DC gate already requires for these types; SitGround: `set_sit_allowed`
        let ok = if t == MoveType::SwitchToSitGroundMode { self.sit_ok } else { true };
        self.apply_guarded(t, ok)
    }

    fn apply_guarded(&mut self, t: MoveType, ok: bool) -> bool {
        let previous_mode = self.status.mode;
        let applied = self.status.transition(t, ok);
        if applied {
            if t == MoveType::JumpStart {
                self.jump_pending = true;
            }
            if self.status.mode == Mode::Fly {
                self.vy = 0.0;
                self.airborne = false;
            }
            match t {
                MoveType::SwitchToFlyMode => {
                    self.falling = false;
                    self.pos[1] += 0.5;
                }
                MoveType::SwitchToRunMode | MoveType::SwitchToWalkMode | MoveType::LeaveFlyMode
                    | MoveType::SwitchToSwimMode | MoveType::SwitchToFrozenMode
                    if previous_mode == Mode::Fly => {
                        self.falling = true;
                        self.pos[1] += 0.1;
                        self.elevate_speed = 0.0;
                    }
                MoveType::ElevateUpStart => self.elevate_speed = 3.0,
                MoveType::ElevateUpStop => self.elevate_speed = -0.8,
                _ => {}
            }
            match t {
                MoveType::ReverseStart => self.direction = -1,
                MoveType::ForwardStart | MoveType::ReverseStop | MoveType::FullStop => self.direction = 1,
                _ => {}
            }
        }
        applied
    }

    /// `CharDCMoveIIR_t` (`FUN_1006bcc6` + `FUN_1006b84b` [GC]): reconcile, teleport to `pos`/`rot`, then — if Features bit 4,
    /// or bit 2 with a type in 9..=14 — cancel a follow (types other than 15 and 22) and run the FSM transition.
    pub fn on_char_dc_move(&mut self, m: &CharDCMove) {
        self.place(m.pos);
        self.yaw = quat_yaw(&m.rot);
        self.wanted_yaw = None;
        let f = self.features;
        if f & 4 == 0 && f & 2 == 0 {
            return;
        }
        if f & 4 == 0 && !(9..=14).contains(&m.move_type) {
            return;
        }
        if m.move_type != 22 && m.move_type != 15 {
            self.stop_follow();
        }
        if let Some(t) = MoveType::from_id(m.move_type) {
            if self.apply(t) {
                match t {
                    MoveType::MouseTurnRightStart | MoveType::MouseTurnLeftStart => self.mouse_turn = true,
                    MoveType::TurnRightStart | MoveType::TurnLeftStart | MoveType::TurnRightStop | MoveType::TurnLeftStop | MoveType::FullStop => {
                        self.mouse_turn = false
                    }
                    _ => {}
                }
            }
        }
        if self.status.forward == 0 && self.status.strafe == 0 && self.path.is_empty() {
            self.speed = 0.0; // the Stop actions call Vehicle_t::Halt
        }
    }

    /// `n3TeleportIIR_t::Activate` [N3 0x10029f87], in-playfield branch: `SetRelPosRot(pos, rot)` (docs/zone/world.md §10.2). Unlike
    /// `CharDCMove` nothing reconciles, so the drawn position snaps; that call does not touch a running path / follow.
    pub fn on_teleport(&mut self, pos: [f32; 3], rot: &[f32; 4]) {
        self.placement_from = self.pos;
        self.pos = pos;
        self.recon = None;
        self.yaw = quat_yaw(rot);
        self.placement_pending = true;
    }

    /// `SetWantedDirectionIIR_t`: heading the dynel turns to (live: the direction of the travel that the next
    /// FollowTarget starts). **[GUESS]** semantics, see docs/zone/motion.md §4.
    pub fn on_wanted_direction(&mut self, dir: [f32; 3]) {
        if dir[0] != 0.0 || dir[2] != 0.0 {
            self.wanted_yaw = Some(dir[0].atan2(dir[2]));
        }
    }

    /// `FollowTargetIIR_c` apply (`FUN_100732e3` [GC]). Returns false when the client drops the message (FSM mode
    /// frozen/sit/9/sleep/lounge). Otherwise: teleport to `f.pos` (unless zero), run transition `f.mode` (21 FullStop,
    /// 24 walk, 25 run) and — when there is a follow target (short form: the dynel itself) — walk `f.path`.
    pub fn on_follow_target(&mut self, f: &FollowTarget) -> bool {
        use Mode::*;
        if matches!(self.status.mode, Frozen | SitGround | Sleep | Lounge) || self.status.mode as u8 == 9 {
            return false;
        }
        if f.pos.x != 0.0 || f.pos.y != 0.0 || f.pos.z != 0.0 {
            self.place([f.pos.x, f.pos.y, f.pos.z]);
        }
        if let Some(t) = MoveType::from_id(f.mode) {
            self.apply(t);
        }
        let has_target = f.form == 1 || f.target.kind != 0 || f.target.instance != 0;
        self.path = if has_target { f.path.iter().map(|v| [v.x, v.y, v.z]).filter(|v| *v != [0.0; 3]).collect() } else { vec![] };
        if !has_target {
            self.speed = 0.0;
        }
        true
    }

    /// The `path` of a `SimpleCharFullUpdate` (flag bit 16; the apply routine calls the same follow setter `FUN_1006fe02` as
    /// FollowTarget, without an FSM transition): `has_target` = the path identity is non-null.
    pub fn on_path(&mut self, has_target: bool, waypoints: &[[f32; 3]]) {
        self.path = if has_target { waypoints.iter().copied().filter(|v| *v != [0.0; 3]).collect() } else { vec![] };
    }

    /// Follow path currently walked (next waypoint first).
    pub fn path(&self) -> &[[f32; 3]] {
        &self.path
    }

    pub fn pose(&self) -> Pose {
        let moving_path = !self.path.is_empty();
        let mut status = self.status;
        if moving_path && self.speed != 0.0 && status.forward == 0 {
            status.forward = 1; // the vehicle's OnStartMoving calls ForwardStart (`FUN_1006ef34`)
        }
        let mut anim = anim_of(&status);
        if status.forward == 0 && status.strafe == 0 && status.turn == 0 && matches!(status.mode, Mode::Walk | Mode::Run) {
            if let Some(w) = self.wanted_yaw {
                let e = wrap(w - self.yaw);
                if e.abs() > TURN_DONE {
                    anim = if e < 0.0 { AnimState::TurnLeft } else { AnimState::TurnRight };
                }
            }
        }
        Pose { pos: self.visual_pos(), yaw: self.yaw, anim, speed: self.speed.abs() }
    }

    fn visual_pos(&self) -> [f32; 3] {
        match self.recon {
            // the client decides per frame: too far → snap, too near → done (N3 0x196bd)
            Some(r) if (RECON_MIN_D2..RECON_MAX_D2).contains(&(0..3).map(|i| (r[i] - self.pos[i]).powi(2)).sum::<f32>()) => r,
            _ => self.pos,
        }
    }

    /// Integrate `dt` seconds (sub-stepped at 1/60 s so the result does not depend on the frame rate).
    pub fn advance(&mut self, dt: f32) -> Pose {
        let mut left = dt.max(0.0);
        while left > 0.0 {
            let h = left.min(1.0 / 60.0);
            self.step(h);
            left -= h;
        }
        self.pose()
    }

    /// Run the same remote vehicle against the loaded playfield, in wire coordinates.
    pub fn advance_with_surface(
        &mut self,
        dt: f32,
        mut align: impl FnMut([f32; 3], [f32; 3], &Body, &mut SurfaceState) -> Aligned,
        mut ceiling: impl FnMut([f32; 3]) -> Option<f32>,
    ) -> Pose {
        if dt > 4.0 { return self.pose(); }
        if self.placement_pending {
            self.align_surface(self.placement_from, true, &mut align);
            self.placement_pending = false;
        }
        if self.jump_pending {
            self.jump_pending = false;
            if !self.airborne {
                let sum = self.strength as f32 + self.agility as f32;
                let sum = if self.gm_level == 0 { sum.min(800.0) } else { sum };
                let mut height = (sum / 200.0 + 1.0).max(0.5);
                if let Some(y) = ceiling(self.pos) {
                    height = height.min((y - self.pos[1] - 2.0 * self.body_scale).max(0.1));
                }
                self.status.jumping = true;
                self.vy = (40.0 * height).sqrt();
                self.falling = true;
                self.airborne = true;
            }
        }
        let mut left = dt.max(0.0);
        while left > 0.0 {
            let h = left.min(1.0 / 60.0);
            let old = self.pos;
            if self.airborne && self.falling_enabled() {
                self.vy = (self.vy - 20.0 * h).clamp(-50.0, 50.0);
            }
            self.step(h);
            self.pos[1] += self.vy * h;
            if self.status.mode == Mode::Fly {
                self.pos[1] += self.elevate_speed * h;
            }
            self.align_surface(old, false, &mut align);
            left -= h;
        }
        self.pose()
    }

    fn falling_enabled(&self) -> bool {
        self.falling && self.features & 8 == 0
    }

    fn align_surface(
        &mut self, old: [f32; 3], teleport: bool,
        align: &mut impl FnMut([f32; 3], [f32; 3], &Body, &mut SurfaceState) -> Aligned,
    ) {
        if self.flags & 0x80000000 != 0 {
            self.vy = 0.0;
            self.airborne = false;
            return;
        }
        let falling_enabled = self.falling_enabled();
        let body = Body { falling_enabled, airborne: self.airborne, vy: self.vy, teleport };
        self.surface.heading = self.yaw;
        let r = align(old, self.pos, &body, &mut self.surface);
        self.pos = r.pos;
        match self.surface.event.take() {
            Some(ao_formats::playfield::collision::LiquidEvent::Enter) => {
                self.status.transition(MoveType::SwitchToSwimMode, true);
            }
            Some(ao_formats::playfield::collision::LiquidEvent::Leave) => {
                self.status.transition(MoveType::LeaveSwimMode, true);
            }
            None => {}
        }
        if let Some(r) = &mut self.recon { r[1] = self.pos[1]; }
        if falling_enabled {
            if r.airborne && !self.airborne {
                self.airborne = true;
                self.vy = 0.0;
            } else if !r.airborne {
                self.airborne = false;
                self.vy = 0.0;
                self.status.transition(MoveType::JumpStop, true);
            }
        }
    }

    fn turn_rate(&self, dir: i8, moving: bool) -> f32 {
        if self.mouse_turn {
            return 0.0;
        }
        let r = if self.turn_speed == 0 {
            if moving { TURN_RATE_MOVING } else { TURN_RATE_STILL }
        } else {
            let r = self.turn_speed as f32 / 11000.0;
            if moving { r * 0.5 } else { r }
        };
        r * dir as f32
    }

    fn step(&mut self, dt: f32) {
        let mode = self.status.mode;
        let skill = self.skill();
        let halted = matches!(mode, Mode::Frozen | Mode::SitGround | Mode::Sleep | Mode::Lounge);
        // --- desired planar velocity (speed along the facing, lateral) and heading target
        let mut target_speed = 0.0f32; // signed, along the facing
        let mut lateral = 0.0f32;
        let mut heading_to: Option<f32> = None;
        let mut turn_in_place = 0.0f32;
        let on_path = !self.path.is_empty() && !halted;
        if on_path {
            let vmax = max_speed(mode, false, skill);
            // pop reached waypoints (radius = braking distance vmax/4, min 0.5: the client's radius is not recovered)
            let pop = (vmax * 0.25).max(POP_RADIUS_MIN);
            while self.path.len() > 1 {
                let w = self.path[0];
                if len2([w[0] - self.pos[0], w[2] - self.pos[2]]).sqrt() < pop {
                    self.path.remove(0);
                } else {
                    break;
                }
            }
            let w = self.path[0];
            let d = [w[0] - self.pos[0], w[2] - self.pos[2]];
            let dist = len2(d).sqrt();
            if self.path.len() == 1 && dist < ARRIVE_RADIUS {
                self.path.clear();
                self.speed = 0.0;
            } else {
                // SteeringArrive [VH 0x1000ab28]: slow down inside the brake distance vmax/4
                let brake = vmax * 0.25;
                target_speed = vmax.min(vmax * dist / brake);
                if self.path.len() > 1 {
                    target_speed = vmax;
                }
                heading_to = Some(d[0].atan2(d[1]));
            }
        } else if !halted {
            let st = self.status;
            if st.forward != 0 {
                target_speed = max_speed(mode, st.forward < 0, skill) * st.forward as f32;
            }
            if st.strafe != 0 {
                lateral = strafe_speed(mode, skill) * st.strafe as f32;
            }
            if st.turn != 0 {
                turn_in_place = self.turn_rate(st.turn, st.forward != 0);
            }
        }
        // --- heading
        if turn_in_place != 0.0 {
            self.yaw = wrap(self.yaw + turn_in_place * dt);
        } else if let Some(h) = heading_to {
            let step = TURN_RATE_MOVING.max(TURN_RATE_STILL) * dt;
            let e = wrap(h - self.yaw);
            self.yaw = wrap(self.yaw + e.clamp(-step, step));
        } else if let Some(w) = self.wanted_yaw {
            let step = self.turn_rate(1, false).abs() * dt;
            let e = wrap(w - self.yaw);
            if e.abs() <= step {
                self.yaw = w;
                self.wanted_yaw = None;
            } else {
                self.yaw = wrap(self.yaw + e.signum() * step);
            }
        }
        // --- speed: accelerate with maxForce/mass = 2·vmax (FUN_1006f4a2: force = 2·mass·vmax); Halt zeroes it
        let accel = 2.0 * max_speed(mode, target_speed < 0.0, skill);
        if target_speed == 0.0 && self.path.is_empty() {
            self.speed = 0.0;
        } else {
            let d = target_speed - self.speed;
            self.speed += d.clamp(-accel * dt, accel * dt);
        }
        // --- position
        if on_path && !self.path.is_empty() {
            // move along the steering direction (the facing is still turning towards it)
            let w = self.path[0];
            let (ox, oz) = (self.pos[0], self.pos[2]);
            let seg = len2([w[0] - ox, w[2] - oz]).sqrt();
            let adv = (self.speed * dt).min(seg);
            if seg > 1e-6 {
                let k = adv / seg;
                if mode == Mode::Fly {
                    self.pos[1] += (w[1] - self.pos[1]) * k;
                }
                self.pos[0] = ox + (w[0] - ox) * k;
                self.pos[2] = oz + (w[2] - oz) * k;
            }
        } else {
            let (s, c) = self.yaw.sin_cos();
            self.pos[0] += (s * self.speed + c * lateral) * dt;
            self.pos[2] += (c * self.speed - s * lateral) * dt;
        }
        // --- reconcile smoothing of the drawn position
        if let Some(r) = self.recon {
            let d2 = (r[0] - self.pos[0]).powi(2) + (r[1] - self.pos[1]).powi(2) + (r[2] - self.pos[2]).powi(2);
            if !(RECON_MIN_D2..RECON_MAX_D2).contains(&d2) {
                self.recon = None;
            } else {
                let k = RECON_KEEP.powf(dt * RECON_HZ);
                self.recon = Some([0, 1, 2].map(|i| r[i] * k + self.pos[i] * (1.0 - k)));
            }
        }
    }
}

/// Facing vector of a heading.
pub fn facing(yaw: f32) -> [f32; 3] {
    [yaw.sin(), 0.0, yaw.cos()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::{capture, decode, dynel::Dynel, misc::Misc, Message, N3};
    use std::collections::HashMap;

    fn surface_step(m: &mut Mover, dt: f32, floor: impl Fn([f32; 3]) -> f32) -> Pose {
        surface_step_ceiling(m, dt, floor, |_| None)
    }

    #[test]
    fn remote_jump_uses_native_unset_stats_and_ceiling_height() {
        let mut mover = Mover::new([0.0; 3], 0.0);
        mover.on_move_type(MoveType::JumpStart as u8);
        surface_step(&mut mover, 0.0, |_| 0.0);
        let height = (2.0 * super::super::world::STAT_UNSET as f32) / 200.0 + 1.0;
        assert_eq!(mover.vy, (40.0 * height).sqrt(), "missing GM is nonzero, so no 800 cap");
        let mut mover = Mover::new([0.0; 3], 0.0);
        mover.on_stat(16, 0);
        mover.on_stat(17, 0);
        mover.on_stat(215, 0);
        mover.on_stat(0, 0x20000000);
        mover.on_move_type(MoveType::JumpStart as u8);
        surface_step_ceiling(&mut mover, 0.0, |_| 0.0, |_| Some(2.1));
        assert!((mover.vy - 2.0).abs() < 1e-5, "stored NPC minimum does not raise launch impulse");
        assert!(mover.falling_enabled(), "Jump enables falling even after no-fall Flags");
    }

    #[test]
    fn remote_fly_blob_restore_does_not_execute_mode_actions() {
        let mut blob = [0u8; 42];
        blob[12] = Mode::Fly as u8;
        blob[22..26].copy_from_slice(&(Mode::Run as i32).to_be_bytes());
        blob[38..42].copy_from_slice(&(-0.8f32).to_be_bytes());
        let mut mover = Mover::new([0.0, 8.0, 0.0], 0.0);
        mover.on_stat(0, 0x20000000);
        mover.restore_blob(&blob);
        assert_eq!(surface_step(&mut mover, 0.0, |_| 0.0).pos[1], 8.0);
        surface_step(&mut mover, 1.0, |_| 0.0);
        assert!((mover.pos[1] - 7.2).abs() < 1e-4, "restored elevate input, no synthesized lift");
        blob[4..8].copy_from_slice(&(2.0f32).to_be_bytes());
        mover.restore_blob(&blob);
        assert_eq!(mover.vy, 2.0, "Flags hook runs before serialized velocity restore");
        assert!(!mover.falling_enabled());
        let falling = Mover::with_blob([0.0, 8.0, 0.0], 0.0, &blob);
        assert!(falling.falling_enabled(), "restoring Fly does not execute DisableFalling");
    }

    #[test]
    fn remote_fly_exit_applies_swim_and_frozen_lifts() {
        for action in [MoveType::SwitchToSwimMode, MoveType::SwitchToFrozenMode] {
            let mut mover = Mover::new([0.0, 8.0, 0.0], 0.0);
            mover.on_move_type(MoveType::SwitchToFlyMode as u8);
            mover.on_move_type(MoveType::ElevateUpStart as u8);
            mover.on_move_type(action as u8);
            assert!((mover.pos[1] - 8.6).abs() < 1e-5);
            assert_eq!(mover.elevate_speed, 0.0);
            assert!(mover.falling_enabled());
        }
    }

    fn surface_step_ceiling(
        m: &mut Mover, dt: f32, floor: impl Fn([f32; 3]) -> f32,
        ceiling: impl FnMut([f32; 3]) -> Option<f32>,
    ) -> Pose {
        m.advance_with_surface(dt, |_, mut pos, body, _| {
            let ground = floor(pos);
            let supported = pos[1] - 0.48 <= ground && body.vy <= 0.1;
            pos[1] = if body.falling_enabled && supported { ground } else { pos[1].max(ground) };
            Aligned { pos, airborne: !supported, normal: [0.0, 1.0, 0.0], liquid: -9999.0 }
        }, ceiling)
    }

    #[test]
    fn remote_ground_downhill_fall_jump_and_fly() {
        let mut m = Mover::new([0.0, 0.2, 0.0], 0.0);
        m.set_features(4);
        m.on_stat(16, 0);
        m.on_stat(17, 0);
        m.on_stat(215, 0);
        assert_eq!(surface_step(&mut m, 0.0, |_| 0.0).pos[1], 0.0);
        m.on_move_type(1);
        surface_step(&mut m, 1.0, |p| -0.1 * p[2]);
        assert!((m.pos[1] + 0.1 * m.pos[2]).abs() < 1e-5, "downhill follows support");
        assert!(!m.airborne);
        m.on_teleport([0.0, 5.0, 0.0], &[0.0, 0.0, 0.0, 1.0]);
        m.on_move_type(21);
        surface_step(&mut m, 0.25, |_| 0.0);
        assert!(m.pos[1] < 5.0 && m.pos[1] > 0.0 && m.airborne);
        surface_step(&mut m, 1.0, |_| 0.0);
        assert_eq!(m.pos[1], 0.0);
        m.on_move_type(15);
        surface_step(&mut m, 0.2, |_| 0.0);
        assert!(m.pos[1] > 0.5 && m.airborne && m.status.jumping);
        surface_step(&mut m, 1.0, |_| 0.0);
        assert!(!m.airborne && !m.status.jumping);
        m.on_move_type(29);
        assert_eq!(m.pos[1], 0.5, "Fly Apply lifts the body");
        m.on_teleport([0.0, 8.0, 0.0], &[0.0, 0.0, 0.0, 1.0]);
        surface_step(&mut m, 1.0, |_| 0.0);
        assert_eq!(m.pos[1], 8.0, "flight is not ground-clamped");
        m.on_move_type(17);
        surface_step(&mut m, 0.25, |_| 0.0);
        assert!((m.pos[1] - 8.75).abs() < 1e-5, "elevate speed is 3 m/s");
        m.on_move_type(18);
        surface_step(&mut m, 0.25, |_| 0.0);
        assert!((m.pos[1] - 8.55).abs() < 1e-4, "stop-elevate sinks at 0.8 m/s");
        m.on_move_type(39);
        assert!((m.pos[1] - 8.65).abs() < 1e-4, "leaving flight lifts by 0.1 m before falling");
        surface_step(&mut m, 2.0, |_| 0.0);
        assert_eq!(m.pos[1], 0.0);
        m.on_stat(0, 0x20000000);
        m.on_teleport([0.0, 5.0, 0.0], &[0.0, 0.0, 0.0, 1.0]);
        surface_step(&mut m, 1.0, |_| 0.0);
        assert_eq!(m.pos[1], 5.0, "Flags DisableFalling preserves height");
        m.on_stat(0, 0);
        m.set_features(4 | 8);
        surface_step(&mut m, 1.0, |_| 0.0);
        assert_eq!(m.pos[1], 5.0, "Features bit 8 disables falling");
        m.set_features(4);
        m.on_stat(0, i32::MIN);
        m.on_teleport([0.0, -1.0, 0.0], &[0.0, 0.0, 0.0, 1.0]);
        surface_step(&mut m, 1.0, |_| 0.0);
        assert_eq!(m.pos[1], -1.0, "Flags DisableSurfaceCollision bypasses alignment");
        m.on_stat(0, 0);
        surface_step(&mut m, 0.1, |_| 0.0);
        assert_eq!(m.pos[1], 0.0);
        m.on_move_type(15);
        surface_step_ceiling(&mut m, 0.0, |_| 0.0, |_| Some(2.2));
        assert!((m.vy - 8.0f32.sqrt()).abs() < 1e-4, "ceiling shortens the launch height");
    }

    #[test]
    fn captured_remote_movement_runs_surface_on_placements_and_substeps() {
        let mut movers = HashMap::new();
        let mut previous = 0;
        let mut placements = 0;
        let mut steps = 0;
        for (ms, message) in events() {
            for mover in movers.values_mut() {
                surface_step(mover, (ms - previous) as f32 / 1000.0, |_| 0.0);
                steps += 1;
                assert!(mover.pos[1] >= 0.0);
                assert_eq!(mover.pose().pos[1], mover.pos[1], "reconcile must not restore stale Y");
            }
            previous = ms;
            let id = message.header.target.instance;
            match message.body {
                N3::Dynel(Dynel::SimpleCharFullUpdate(update)) => {
                    let mut mover = Mover::with_blob(update.pos, update.yaw().unwrap_or(0.0), &update.blob);
                    mover.set_features(if update.is_npc() { 2 } else { 4 });
                    movers.insert(id, mover);
                }
                N3::Dynel(Dynel::CharDCMove(mv)) => {
                    if let Some(mover) = movers.get_mut(&id) {
                        mover.on_char_dc_move(&mv);
                        surface_step(mover, 0.0, |_| 0.0);
                        assert!(!mover.placement_pending);
                        placements += 1;
                    }
                }
                N3::Misc(Misc::FollowTarget(follow)) => {
                    if let Some(mover) = movers.get_mut(&id) {
                        mover.on_follow_target(&follow);
                        surface_step(mover, 0.0, |_| 0.0);
                    }
                }
                _ => {}
            }
        }
        assert!(placements > 10 && steps > 10);
    }
    /// The captured zone session as `(ms, decoded message)`.
    fn events() -> Vec<(u32, Message)> {
        capture()
            .into_iter()
            .filter(|(_, sent, _)| !sent)
            .filter_map(|(ms, _, b)| {
                let (f, _) = crate::frame::Frame::decode_with(&b, false).ok().flatten()?;
                (f.ptype == 0xA).then(|| decode(&f).ok().map(|m| (ms, m))).flatten()
            })
            .collect()
    }

    /// Replay every dynel's movement messages into a Mover; returns `(id, is_npc, message kind, move type / mode, horizontal
    /// error in metres of the dead-reckoned position against the position the server sent, dt in seconds)`.
    fn replay() -> Vec<(i32, bool, &'static str, u8, f32, f32)> {
        let mut movers: HashMap<i32, (Mover, u32, bool)> = HashMap::new();
        let mut out = vec![];
        let err = |m: &Mover, p: [f32; 3]| ((m.sim_pos()[0] - p[0]).powi(2) + (m.sim_pos()[2] - p[2]).powi(2)).sqrt();
        for (ms, m) in events() {
            let id = m.header.target.instance;
            match m.body {
                N3::Dynel(Dynel::SimpleCharFullUpdate(c)) => {
                    let mut mv = Mover::with_blob(c.pos, c.yaw().unwrap_or(0.0), &c.blob);
                    mv.set_features(if c.is_npc() { 0x8003 } else { 4 });
                    mv.on_stat(STAT_RUN_SPEED, c.run_speed as i32);
                    mv.on_stat(STAT_MAX_HEALTH, c.max_health);
                    mv.on_stat(STAT_HEALTH, c.health);
                    if let Some(p) = &c.path {
                        mv.on_path(p.id.kind != 0 || p.id.instance != 0, &p.waypoints);
                    }
                    movers.insert(id, (mv, ms, c.is_npc()));
                }
                N3::Dynel(Dynel::CharDCMove(d)) => {
                    if let Some((mv, last, npc)) = movers.get_mut(&id) {
                        let dt = (ms - *last) as f32 / 1000.0;
                        mv.advance(dt);
                        out.push((id, *npc, "dc", d.move_type, err(mv, d.pos), dt));
                        mv.on_char_dc_move(&d);
                        *last = ms;
                    }
                }
                N3::Dynel(Dynel::SetWantedDirection(w)) => {
                    if let Some((mv, last, _)) = movers.get_mut(&id) {
                        mv.advance((ms - *last) as f32 / 1000.0);
                        *last = ms;
                        mv.on_wanted_direction(w.dir);
                    }
                }
                N3::Dynel(Dynel::Stat(s)) => {
                    if let Some((mv, _, _)) = movers.get_mut(&id) {
                        for (k, v) in s.stats {
                            mv.on_stat(k, v);
                        }
                    }
                }
                N3::Misc(Misc::FollowTarget(f)) => {
                    if let Some((mv, last, npc)) = movers.get_mut(&id) {
                        let dt = (ms - *last) as f32 / 1000.0;
                        mv.advance(dt);
                        out.push((id, *npc, "ft", f.mode, err(mv, [f.pos.x, f.pos.y, f.pos.z]), dt));
                        mv.on_follow_target(&f);
                        *last = ms;
                    }
                }
                N3::Misc(Misc::ToClientQuit) => {
                    movers.remove(&id);
                }
                _ => {}
            }
        }
        out
    }

    fn median(mut v: Vec<f32>) -> f32 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    }

    #[test]
    fn move_type_table() {
        // ids with a transition class in the switch of FUN_1006c60f; 19, 20, 31, 32 and everything above 42 have none
        let ids: Vec<u8> = (0..=60).filter(|i| MoveType::from_id(*i).is_some()).collect();
        let want: Vec<u8> = (1..=18).chain([21, 22]).chain(23..=30).chain(33..=42).collect();
        assert_eq!(ids, want);
        for i in [0u8, 19, 20, 31, 32, 43, 255] {
            assert_eq!(MoveType::from_id(i), None);
        }
        assert_eq!(MoveType::from_id(30).unwrap().name(), "SwitchToSitGroundMode");
        assert_eq!(MoveType::from_id(6).unwrap().name(), "StrafeStop(right)");
        assert_eq!(MoveType::from_id(11).unwrap().name(), "TurnStop(right)");
    }

    #[test]
    fn captured_blobs_are_the_default_status() {
        let mut n = 0;
        for (_, m) in events() {
            if let N3::Dynel(Dynel::SimpleCharFullUpdate(c)) = m.body {
                let s = Status::from_blob(&c.blob).unwrap();
                assert_eq!(Status { prev_mode: Mode::Walk, ..s }, Status::default(), "{:02x?}", c.blob);
                assert!(matches!(s.prev_mode, Mode::Walk | Mode::Run));
                n += 1;
            }
        }
        assert_eq!(n, 81);
    }
    #[test]
    fn effect_inputs_retain_native_vehicle_direction_and_speed() {
        let mut blob = [0u8; 28];
        blob[0..4].copy_from_slice(&3.0f32.to_be_bytes());
        blob[4..8].copy_from_slice(&100.0f32.to_be_bytes());
        blob[8..12].copy_from_slice(&4.0f32.to_be_bytes());
        blob[12..22].copy_from_slice(&[3, 2, 2, 1, 0, 1, 0, 1, 0, 1]);
        blob[25] = 3;
        let mut mover = Mover::with_blob([0.0; 3], 0.0, &blob);
        assert_eq!((mover.vehicle_speed(), mover.vehicle_direction()), (5.0, -1), "vertical velocity is not Vehicle+cc");
        assert!(mover.apply(MoveType::ReverseStop));
        assert_eq!(mover.vehicle_direction(), 1);
        assert!(mover.apply(MoveType::ForwardStart));
        assert!(mover.apply(MoveType::ForwardStop));
        assert_eq!(mover.vehicle_direction(), 1);
    }


    #[test]
    fn fsm_guards() {
        let mut s = Status::default();
        assert!(s.transition(MoveType::ForwardStart, true) && s.forward == 1);
        assert!(!s.transition(MoveType::ForwardStart, true), "already moving forward");
        assert!(s.transition(MoveType::ReverseStart, true) && s.forward == -1);
        assert!(!s.transition(MoveType::ForwardStop, true), "ForwardStop needs forward motion");
        assert!(s.transition(MoveType::ReverseStop, true) && s.forward == 0);
        assert!(s.transition(MoveType::StrafeLeftStart, true) && s.strafe == -1);
        assert!(!s.transition(MoveType::StrafeRightStop, true), "right stop does not stop a left strafe");
        assert!(s.transition(MoveType::StrafeLeftStop, true) && s.strafe == 0);
        assert!(s.transition(MoveType::TurnRightStart, true) && s.turn == 1);
        assert!(s.transition(MoveType::TurnRightStop, true) && s.turn == 0);
        assert!(s.transition(MoveType::SwitchToWalkMode, true) && (s.mode, s.prev_mode) == (Mode::Walk, Mode::Walk));
        assert!(s.transition(MoveType::SwitchToSwimMode, true) && s.mode == Mode::Swim);
        assert!(s.transition(MoveType::LeaveSwimMode, true) && s.mode == Mode::Walk);
        assert!(!s.transition(MoveType::ElevateUpStop, true));
        assert!(s.transition(MoveType::SwitchToFlyMode, true) && s.mode == Mode::Fly);
        assert!(s.transition(MoveType::ElevateUpStart, true) && s.elevating);
        assert!(s.transition(MoveType::ElevateUpStop, true) && !s.elevating);
        assert!(s.transition(MoveType::LeaveFlyMode, true) && s.mode == Mode::Walk);
        s.transition(MoveType::FullStop, true);
        assert!(s.transition(MoveType::SwitchToSitGroundMode, true) && s.mode == Mode::SitGround);
        assert!(s.transition(MoveType::LeaveSitMode, true) && s.mode == Mode::Walk);
        assert!(s.transition(MoveType::SwitchToSleepMode, true) && s.mode == Mode::Sleep);
        assert!(s.transition(MoveType::LeaveSleepMode, true) && s.mode == Mode::SitGround, "leaving sleep sits up");
        let mut m = Status::default();
        m.transition(MoveType::ForwardStart, true);
        assert!(!m.transition(MoveType::SwitchToSitGroundMode, true), "no sitting while moving");
        assert!(!m.transition(MoveType::SwitchToCrawlMode, true), "no crawling while moving");
    }

    #[test]
    fn animation_roles() {
        let mut s = Status::default();
        assert_eq!(anim_of(&s), AnimState::Idle);
        s.transition(MoveType::ForwardStart, true);
        assert_eq!((anim_of(&s), anim_of(&s).anim_id()), (AnimState::Run, Some(0x65)));
        s.transition(MoveType::SwitchToWalkMode, true);
        assert_eq!(anim_of(&s), AnimState::Walk);
        s.transition(MoveType::ReverseStart, true);
        assert_eq!(anim_of(&s).clip_name(), Some("walk-back"));
        s.transition(MoveType::FullStop, true);
        s.transition(MoveType::StrafeRightStart, true);
        assert_eq!(anim_of(&s), AnimState::WalkRight);
        s.transition(MoveType::FullStop, true);
        s.transition(MoveType::TurnLeftStart, true);
        assert_eq!(anim_of(&s), AnimState::TurnLeft);
        s.transition(MoveType::FullStop, true);
        s.transition(MoveType::SwitchToSwimMode, true);
        assert_eq!(anim_of(&s), AnimState::IdleSwim);
        assert_eq!(AnimState::Attack.anim_id(), None);
    }

    #[test]
    fn speeds_follow_the_client_formulas() {
        assert!((max_speed(Mode::Run, false, 144.0) - 5.5236).abs() < 1e-3);
        assert_eq!(max_speed(Mode::Run, false, 1.0e6), 13.0);
        assert_eq!(max_speed(Mode::Run, true, 0.0), 3.0);
        assert_eq!(max_speed(Mode::Walk, false, 500.0), 1.5);
        assert_eq!(max_speed(Mode::Swim, false, 0.0), 3.0);
        assert_eq!(max_speed(Mode::Fly, false, 0.0), 7.0);
        let mut m = Mover::new([0.0; 3], 0.0);
        m.on_stat(STAT_RUN_SPEED, 144);
        assert_eq!(m.skill(), 144.0);
        m.on_stat(STAT_MAX_HEALTH, 1000);
        m.on_stat(STAT_HEALTH, 75); // half of the 15 % threshold: f = 0.5 → 0.5·(144+1000) − 1000
        assert!((m.skill() + 428.0).abs() < 1e-3);
        m.on_stat(STAT_HEALTH, 1000);
        assert_eq!(m.skill(), 144.0);
    }

    /// Players: the dead-reckoned position at the next DC move is within decimetres of the server's.
    #[test]
    fn dead_reckoning_tracks_players() {
        let r = replay();
        let moving: Vec<f32> = r.iter().filter(|(_, npc, k, t, _, dt)| !*npc && *k == "dc" && *t == 1 && *dt > 0.0 && *dt < 3.0).map(|r| r.4).collect();
        assert!(moving.len() >= 25, "{}", moving.len());
        assert!(median(moving.clone()) < 0.6, "median {}", median(moving));
        // turn/strafe/stop messages arrive at the position the char already stands on
        assert!(r.iter().filter(|(_, npc, k, t, _, _)| !*npc && *k == "dc" && [9, 11, 12, 14].contains(t)).all(|r| r.4 < 0.1));
    }

    /// NPC run segments: the server moves them at `5 + RunSpeed/275` m/s (± message timing), the client's max speed.
    #[test]
    fn npc_run_speed_matches_the_server() {
        let mut start: HashMap<i32, (u32, [f32; 3])> = HashMap::new();
        let mut rs: HashMap<i32, i16> = HashMap::new();
        let mut diffs = vec![];
        for (ms, m) in events() {
            let id = m.header.target.instance;
            match m.body {
                N3::Dynel(Dynel::SimpleCharFullUpdate(c)) => {
                    rs.insert(id, c.run_speed);
                }
                N3::Misc(Misc::FollowTarget(f)) if f.mode == 25 && f.form == 1 => {
                    start.insert(id, (ms, [f.pos.x, f.pos.y, f.pos.z]));
                }
                N3::Misc(Misc::FollowTarget(f)) if f.mode == 21 => {
                    if let Some((t0, p0)) = start.remove(&id) {
                        let dt = (ms - t0) as f32 / 1000.0;
                        if (0.3..0.7).contains(&dt) {
                            let d = ((f.pos.x - p0[0]).powi(2) + (f.pos.z - p0[2]).powi(2)).sqrt();
                            diffs.push((d / dt - max_speed(Mode::Run, false, rs[&id] as f32)).abs());
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(diffs.len() >= 20, "{}", diffs.len());
        assert!(median(diffs.clone()) < 0.8, "{diffs:?}");
    }

    /// First FollowTarget of the capture (NPC 1002060, "ICC Shuttle Guard", RunSpeed 144): wanted direction, then a run
    /// along `[pos, (901.44, 833.6)]`.
    #[test]
    fn follow_target_walks_the_path() {
        let ev = events();
        let (full, wd, ft) = {
            let id = 1002060;
            let full = ev.iter().find_map(|(_, m)| match &m.body {
                N3::Dynel(Dynel::SimpleCharFullUpdate(c)) if m.header.target.instance == id => Some((**c).clone()),
                _ => None,
            });
            let wd = ev.iter().find_map(|(_, m)| match &m.body {
                N3::Dynel(Dynel::SetWantedDirection(w)) if m.header.target.instance == id => Some(w.dir),
                _ => None,
            });
            let ft = ev.iter().find_map(|(_, m)| match &m.body {
                N3::Misc(Misc::FollowTarget(f)) if m.header.target.instance == id => Some(f.clone()),
                _ => None,
            });
            (full.unwrap(), wd.unwrap(), ft.unwrap())
        };
        let mut m = Mover::with_blob(full.pos, full.yaw().unwrap(), &full.blob);
        m.on_stat(STAT_RUN_SPEED, full.run_speed as i32);
        assert_eq!(m.pose().anim, AnimState::Idle);
        m.on_wanted_direction(wd);
        // standing still: turns towards the wanted direction with the turn animation
        let p = m.advance(0.05);
        assert!(matches!(p.anim, AnimState::TurnLeft | AnimState::TurnRight), "{:?}", p.anim);
        m.advance(1.0);
        assert!((wrap(m.pose().yaw - wd[0].atan2(wd[2]))).abs() < 0.01);
        assert_eq!(m.pose().anim, AnimState::Idle);
        // the run
        assert!(m.on_follow_target(&ft));
        let dest = *ft.path.last().unwrap();
        let mut seen_run = false;
        let mut top = 0.0f32;
        for _ in 0..90 {
            let p = m.advance(1.0 / 30.0);
            seen_run |= p.anim == AnimState::Run;
            top = top.max(p.speed);
        }
        let p = m.pose();
        assert!(seen_run && (top - 5.52).abs() < 0.05, "top speed {top}");
        let d = ((p.pos[0] - dest.x).powi(2) + (p.pos[2] - dest.z).powi(2)).sqrt();
        assert!(d < 0.3, "arrived {d} m from the destination");
        assert_eq!((p.anim, p.speed), (AnimState::Idle, 0.0));
        assert!(m.path().is_empty());
        // heading = direction of travel (atan2(dx, dz) of the path)
        let want = (dest.x - ft.pos.x).atan2(dest.z - ft.pos.z);
        assert!(wrap(p.yaw - want).abs() < 0.05);
        // a FullStop FollowTarget (mode 21, no target) interrupts and places
        let stop = FollowTarget { form: 2, mode: 21, target: crate::msg::Identity { kind: 0, instance: 0 }, speed: 0.0, pos: ft.pos, path: vec![ft.pos] };
        assert!(m.on_follow_target(&stop));
        assert_eq!(m.sim_pos(), [ft.pos.x, ft.pos.y, ft.pos.z]);
        assert!(m.path().is_empty());
    }

    #[test]
    fn spawn_placement_type_30_is_not_a_sit_down() {
        let (pos, rot) = ([856.6425, 40.0342, 691.2252], [0.0, 0.0, 0.0, 1.0]);
        let dc = CharDCMove { move_type: 30, type_bit7: false, rot, pos, time: 0, extra: [0.0; 2] };
        let mut npc = Mover::new([0.0; 3], 0.0);
        npc.set_features(0x8007); // Features bit 4 set, as on most captured NPC records
        npc.on_char_dc_move(&dc);
        assert_eq!((npc.status().mode, npc.sim_pos()), (Mode::Run, pos));
        assert_eq!(npc.pose().anim, AnimState::Idle);
        npc.set_sit_allowed(true);
        npc.on_char_dc_move(&dc);
        assert_eq!(npc.pose().anim, AnimState::SitGround);
    }

    #[test]
    fn npc_without_bit4_only_turns_from_dc_moves() {
        let mk = |t| CharDCMove { move_type: t, type_bit7: false, rot: [0.0, 0.0, 0.0, 1.0], pos: [1.0, 2.0, 3.0], time: 0, extra: [0.0; 2] };
        let mut n = Mover::new([0.0; 3], 0.0);
        n.set_features(0x8003);
        n.on_char_dc_move(&mk(1));
        assert_eq!((n.status().forward, n.sim_pos()), (0, [1.0, 2.0, 3.0]));
        n.on_char_dc_move(&mk(9));
        assert_eq!(n.status().turn, 1);
        let mut none = Mover::new([0.0; 3], 0.0);
        none.set_features(0);
        none.on_char_dc_move(&mk(9));
        assert_eq!(none.status().turn, 0);
    }

    #[test]
    fn turning_and_strafing_dead_reckon() {
        let mk = |t| CharDCMove { move_type: t, type_bit7: false, rot: [0.0, 0.0, 0.0, 1.0], pos: [0.0; 3], time: 0, extra: [0.0; 2] };
        let mut m = Mover::new([0.0; 3], 0.0);
        m.set_features(4);
        m.on_char_dc_move(&mk(9)); // TurnRightStart, stationary: 3.5 rad/s
        let p = m.advance(0.2);
        assert!((p.yaw - 0.7).abs() < 0.01 && p.anim == AnimState::TurnRight);
        m.on_char_dc_move(&mk(11));
        m.on_char_dc_move(&mk(1)); // ForwardStart, run: accelerates to 5 m/s along +Z
        let p = m.advance(2.0);
        assert!((p.speed - 5.0).abs() < 1e-3 && p.anim == AnimState::Run);
        let mut s = Mover::new([0.0; 3], 0.0);
        s.set_features(4);
        s.on_char_dc_move(&mk(7)); // StrafeLeftStart: lateral −X at 2.5 m/s (run, skill 0)
        let p = s.advance(1.0);
        assert!((p.pos[0] + 2.5).abs() < 0.01 && p.pos[2].abs() < 0.01 && p.anim == AnimState::WalkLeft, "{p:?}");
    }

    #[test]
    fn mouse_turns_do_not_rotate() {
        let mk = |t| CharDCMove { move_type: t, type_bit7: false, rot: [0.0, 0.0, 0.0, 1.0], pos: [0.0; 3], time: 0, extra: [0.0; 2] };
        let mut m = Mover::new([0.0; 3], 0.0);
        m.set_features(4);
        m.on_char_dc_move(&mk(10));
        let p = m.advance(0.5);
        assert_eq!((p.yaw, m.status().turn, p.anim), (0.0, 1, AnimState::TurnRight));
        m.on_char_dc_move(&mk(11));
        m.on_char_dc_move(&mk(9));
        assert!(m.advance(0.1).yaw > 0.3);
    }

    #[test]
    fn reconcile_smooths_a_placement() {
        let mut m = Mover::new([0.0; 3], 0.0);
        m.set_features(0);
        let mv = |x| CharDCMove { move_type: 22, type_bit7: false, rot: [0.0, 0.0, 0.0, 1.0], pos: [x, 0.0, 0.0], time: 0, extra: [0.0; 2] };
        m.on_char_dc_move(&mv(1.0)); // 1 m jump: drawn at the old position first, then converges
        assert_eq!(m.pose().pos, [0.0; 3]);
        let p = m.advance(1.0 / 60.0);
        assert!((p.pos[0] - 0.2).abs() < 1e-4, "0.8·old + 0.2·new per frame, got {}", p.pos[0]);
        let p = m.advance(1.0);
        assert_eq!(p.pos, [1.0, 0.0, 0.0]);
        m.on_char_dc_move(&mv(1000.0)); // beyond 35 m: snap
        assert_eq!(m.pose().pos, [1000.0, 0.0, 0.0]);
        m.advance(0.016);
        assert_eq!(m.pose().pos[0], 1000.0);
    }

    #[test]
    fn follow_is_dropped_while_sitting() {
        let mut m = Mover::new([0.0; 3], 0.0);
        m.set_features(4);
        m.set_sit_allowed(true);
        m.on_char_dc_move(&CharDCMove { move_type: 30, type_bit7: false, rot: [0.0, 0.0, 0.0, 1.0], pos: [0.0; 3], time: 0, extra: [0.0; 2] });
        let f = FollowTarget {
            form: 1,
            mode: 25,
            target: crate::msg::Identity { kind: 0xC350, instance: 1 },
            speed: 0.0,
            pos: super::super::misc::Vec3 { x: 1.0, y: 0.0, z: 1.0 },
            path: vec![],
        };
        assert!(!m.on_follow_target(&f));
        assert_eq!(m.sim_pos(), [0.0; 3]);
    }
}
