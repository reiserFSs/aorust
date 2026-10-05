//! Sitting / standing / sleeping / lounging / crawling and emotes of other dynels, from the zone stream. Pure state, no
//! rendering: the renderer reads [`Pose`] / [`Event`]. Rules and addresses: docs/zone/actions.md §4-§5.
//!
//! The movement FSM is the one of `ao_net::n3::motion` ([`Status`] / [`MoveType`]); this tracker only adds what the
//! `CharacterActionIIR_t` and `SocialActionCmd_t` messages and the `WaitState` stat do to it.

#![allow(dead_code)] // pure API; the Combat controller / renderer wire it in (nothing in the app calls it yet)

use ao_net::frame::Frame;
use ao_net::n3::action::{self, id, mv, stat, wait, Emote};
use ao_net::n3::dynel::Dynel;
use ao_net::n3::motion::{Mode, MoveType, Status};
use ao_net::n3::world::World;
use ao_net::n3::{self, N3};
use std::collections::HashMap;

/// `Features` (stat 224) mask the sit/leave transitions test (`FUN_10044b6e(4)` [GC 0x1006cd51]).
const FEATURES_SIT: i32 = 4;
/// Default `Features` of a dynel we know nothing about (`Mover::new`: NPC-like, 0x8003 / 0x8007 in the NPC records).
const FEATURES_NPC: i32 = 2;
const STAT_FEATURES: i32 = 224;

/// What the dynel looks like it is doing with its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Standing,
    /// FSM mode 8 (`SwitchToSitGroundMode`).
    SitGround,
    /// FSM mode 8 while `WaitState == 1` (sat down on an item with `SitToggle` action 0x55).
    SitItem,
    Sleeping,
    Lounging,
    Crawling,
}

impl Pose {
    /// Looping clip while in the pose (names from the client's id -> clip table `FUN_100c01c9` [GC]; which transition plays which
    /// start/stop clip is **[INFERENCE]** from the names, nothing was traced).
    pub fn idle_clip(self) -> Option<&'static str> {
        Some(match self {
            Pose::Standing => return None,
            Pose::SitGround => "idle-ground",
            Pose::SitItem => "idle-chair",
            Pose::Sleeping => "idle-sleep-ground",
            Pose::Lounging => "idle-lounging",
            Pose::Crawling => "idle-crawl",
        })
    }
    /// One-shot clip when the pose is entered (**[INFERENCE]**, see [`Pose::idle_clip`]).
    pub fn enter_clip(self) -> Option<&'static str> {
        Some(match self {
            Pose::Standing => return None,
            Pose::SitGround => "ground-start",
            Pose::SitItem => "chair-start",
            Pose::Sleeping => "sleep-ground",
            Pose::Lounging => "lounging",
            Pose::Crawling => "crawl_start",
        })
    }
    /// One-shot clip when the pose is left (**[INFERENCE]**); sleep and lounge have none in the table.
    pub fn exit_clip(self) -> Option<&'static str> {
        Some(match self {
            Pose::SitGround => "ground-stop",
            Pose::SitItem => "chair-stop",
            Pose::Crawling => "crawl_stop",
            Pose::Standing | Pose::Sleeping | Pose::Lounging => return None,
        })
    }
}

/// Something the renderer / HUD should react to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The pose of dynel `dynel` (instance of its `{0xC350, id}` identity) changed.
    Pose { dynel: i32, from: Pose, to: Pose },
    /// `/<emote>` seen: play `clip` (`social-<name>`) once on the dynel.
    Emote { dynel: i32, id: i32, name: &'static str, clip: String },
}

#[derive(Clone, Copy, Debug)]
struct Tracked {
    status: Status,
    features: i32,
    wait_state: i32,
}

impl Default for Tracked {
    fn default() -> Self {
        Tracked { status: Status::default(), features: FEATURES_NPC, wait_state: 0 }
    }
}

impl Tracked {
    fn pose(&self) -> Pose {
        match self.status.mode {
            Mode::SitGround if self.wait_state == wait::ON_ITEM => Pose::SitItem,
            Mode::SitGround => Pose::SitGround,
            Mode::Sleep => Pose::Sleeping,
            Mode::Lounge => Pose::Lounging,
            Mode::Crawl => Pose::Crawling,
            _ => Pose::Standing,
        }
    }
    fn sit_ok(&self) -> bool {
        self.features & FEATURES_SIT != 0
    }
}

/// Pose / emote tracker for every dynel of a playfield.
#[derive(Default)]
pub struct Actions {
    dynels: HashMap<i32, Tracked>,
}

impl Actions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current pose (`Standing` for unknown dynels).
    pub fn pose(&self, dynel: i32) -> Pose {
        self.dynels.get(&dynel).map_or(Pose::Standing, Tracked::pose)
    }

    pub fn forget(&mut self, dynel: i32) {
        self.dynels.remove(&dynel);
    }

    /// Feed one received `ptype` 0xA frame; the events it causes (empty for everything not about actions).
    pub fn on_frame(&mut self, f: &Frame) -> Vec<Event> {
        let Ok(msg) = n3::decode(f) else { return vec![] };
        let dynel = msg.header.target.instance;
        let is_char = msg.header.target.kind == ao_net::n3::outgoing::DYNEL_CHAR;
        match msg.body {
            N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if is_char => {
                let mut t = Tracked::default();
                if let Some(s) = Status::from_blob(&u.blob) {
                    t.status = s;
                }
                t.features = u.pairs.iter().find(|p| p.0 == STAT_FEATURES).map_or(if u.is_npc() { FEATURES_NPC } else { FEATURES_SIT }, |p| p.1);
                self.dynels.insert(dynel, t);
                vec![]
            }
            N3::Dynel(Dynel::CharDCMove(m)) if is_char => self.on_move(dynel, m.move_type),
            N3::Dynel(Dynel::Stat(s)) if is_char => s.stats.iter().flat_map(|&(k, v)| self.on_stat(dynel, k, v)).collect(),
            N3::World(World::CharacterAction(a)) if is_char => self.on_action(dynel, a.action),
            N3::Unknown(body) if msg.header.msg_type == action::SOCIAL_ACTION_CMD => {
                match action::parse_social_body(msg.header.target, &body) {
                    Ok(s) if is_char && matches!(s.state, 0 | 1) => self.on_emote(dynel, s.anim, s.state == 1),
                    _ => vec![],
                }
            }
            _ => vec![],
        }
    }

    /// Stat pair (`StatIIR_t`, or the stats of a full update): `Features` (224) gates the FSM, `WaitState` (430) tells sit-on-item.
    pub fn on_stat(&mut self, dynel: i32, stat_id: i32, value: i32) -> Vec<Event> {
        let before = self.pose(dynel);
        let t = self.dynels.entry(dynel).or_default();
        match stat_id {
            STAT_FEATURES => t.features = value,
            stat::WAIT_STATE => t.wait_state = value,
            _ => return vec![],
        }
        self.changed(dynel, before)
    }

    /// `CharDCMoveIIR_t` move type (`FUN_1006bcc6` / `FUN_1006b84b` [GC]): Features mask 4 lets every type drive the FSM, mask 2 only
    /// the turn types 9..=14; the SitGround / Leave* guards additionally need mask 4 (`Status::transition`'s `leave_ok`).
    pub fn on_move(&mut self, dynel: i32, move_type: u8) -> Vec<Event> {
        let before = self.pose(dynel);
        let t = self.dynels.entry(dynel).or_default();
        let drives = t.features & 4 != 0 || (t.features & 2 != 0 && (9..=14).contains(&move_type));
        if drives {
            if let Some(m) = MoveType::from_id(move_type) {
                let ok = t.sit_ok();
                t.status.transition(m, ok);
            }
        }
        self.changed(dynel, before)
    }

    /// `CharacterActionIIR_t` apply (`FUN_1005d0d8` [GC]) for the actions that move the FSM: `0x56` = transition `0x1e`
    /// (SwitchToSitGround) [GC 0x1005d723], `0x57` = LeaveSleep (`0x29`) / LeaveLounge (`0x2a`) / LeaveSit (`0x25`) by `WaitState`
    /// `0xf` / `0x10` / else [GC 0x1005d72f]. No Features gate on this path itself, the transitions keep theirs.
    pub fn on_action(&mut self, dynel: i32, action_id: i32) -> Vec<Event> {
        let before = self.pose(dynel);
        let t = self.dynels.entry(dynel).or_default();
        let step = match action_id {
            id::SIT_RELAY => Some(mv::SWITCH_TO_SIT_GROUND),
            id::STAND_UP => Some(match t.wait_state {
                wait::SLEEP => mv::LEAVE_SLEEP,
                wait::LOUNGE => mv::LEAVE_LOUNGE,
                _ => mv::LEAVE_SIT,
            }),
            _ => None,
        };
        if let Some(m) = step.and_then(MoveType::from_id) {
            let ok = t.sit_ok();
            t.status.transition(m, ok);
        }
        self.changed(dynel, before)
    }

    /// `SocialActionCmd_t` for emote `anim`: the clip always plays; sleep (0x44) / lounge (0x45) also run their transition when the
    /// message is the server's accepted copy (`state == 1`, `FUN_1007a977` [GC 0x1007a977] -> `FUN_1007aa66`). Call it with
    /// `accepted = false` for our own `/emote` right after sending (`FUN_1007a91e` plays the clip locally).
    pub fn on_emote(&mut self, dynel: i32, anim: i32, accepted: bool) -> Vec<Event> {
        let Some(e) = action::emote(anim) else { return vec![] };
        let before = self.pose(dynel);
        let t = self.dynels.entry(dynel).or_default();
        if let (true, Some(m)) = (accepted, e.transition().and_then(MoveType::from_id)) {
            t.status.transition(m, true);
        }
        let mut out = vec![Event::Emote { dynel, id: e.id, name: e.command, clip: e.clip() }];
        out.extend(self.changed(dynel, before));
        out
    }

    fn changed(&self, dynel: i32, before: Pose) -> Vec<Event> {
        let to = self.pose(dynel);
        if to == before {
            vec![]
        } else {
            vec![Event::Pose { dynel, from: before, to }]
        }
    }
}

/// The emote a `/<name>` or `/emote <name>` chat line names (`_stricmp` against the command column).
pub fn parse_emote_command(line: &str) -> Option<Emote> {
    let mut words = line.trim().strip_prefix('/')?.split_whitespace();
    let first = words.next()?;
    let name = if first.eq_ignore_ascii_case("emote") { words.next()? } else { first };
    action::emote_by_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::msg::Identity;
    use ao_net::n3::outgoing::{char_dc_move, n3_frame, CharMove};

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

    fn mv_frame(id: i32, t: u8) -> Frame {
        let m = CharMove { action: t, pos: [1.0, 2.0, 3.0], rot: [0.0, 0.0, 0.0, 1.0], elapsed_ms: 0, look: [0.0, 0.0] };
        n3_frame(1, 1, char_dc_move(id, &m))
    }
    fn act_frame(id: i32, a: i32) -> Frame {
        let p = action::character_action(id, &action::simple(a, Identity::default(), Identity::default()));
        n3_frame(1, 1, p)
    }

    /// The live capture: NPCs get a type-30 placement right after spawning and stand (Features 0x8003 / 0x8007 lack mask 4),
    /// the 22 relayed actions (0x62/0x63/0xa7/0xad) are none of the FSM ones.
    #[test]
    fn live_capture_never_sits_anyone() {
        let mut a = Actions::new();
        let mut total = 0;
        for f in frames(include_str!("../../../../../docs/captures/zone_ithaca.rec")) {
            total += a.on_frame(&f).len();
        }
        assert_eq!(total, 0);
        assert!(a.dynels.len() > 10, "{}", a.dynels.len());
        assert!(a.dynels.keys().all(|&d| a.pose(d) == Pose::Standing));
    }

    #[test]
    fn sit_and_stand_from_moves_and_actions() {
        let mut a = Actions::new();
        a.on_stat(7, STAT_FEATURES, 4); // a player: mask 4
        // CharDCMove 0x1e sits (what the client sends and the server relays), 0x25 = LeaveSit stands
        assert_eq!(a.on_frame(&mv_frame(7, 0x1e)), [Event::Pose { dynel: 7, from: Pose::Standing, to: Pose::SitGround }]);
        assert_eq!(a.pose(7), Pose::SitGround);
        assert_eq!(a.on_frame(&mv_frame(7, 0x25)), [Event::Pose { dynel: 7, from: Pose::SitGround, to: Pose::Standing }]);
        // CharacterAction 0x56 sits, 0x57 stands
        assert_eq!(a.on_frame(&act_frame(7, 0x56)), [Event::Pose { dynel: 7, from: Pose::Standing, to: Pose::SitGround }]);
        assert_eq!(a.on_frame(&act_frame(7, 0x57)), [Event::Pose { dynel: 7, from: Pose::SitGround, to: Pose::Standing }]);
        // an action that does not touch the FSM changes nothing
        assert!(a.on_frame(&act_frame(7, 0x63)).is_empty());
    }

    #[test]
    fn npc_features_block_the_sit_placement() {
        let mut a = Actions::new();
        assert!(a.on_frame(&mv_frame(9, 0x1e)).is_empty(), "default Features 2: placement only");
        a.on_stat(9, STAT_FEATURES, 0x8003);
        assert!(a.on_frame(&mv_frame(9, 0x1e)).is_empty());
        assert_eq!(a.pose(9), Pose::Standing);
        assert!(a.on_frame(&act_frame(9, 0x56)).is_empty(), "the transition guard needs mask 4 as well");
    }

    #[test]
    fn wait_state_picks_the_leave_transition() {
        let mut a = Actions::new();
        a.on_stat(3, STAT_FEATURES, 4);
        // sleep via the emote 0x44 (accepted copy), then 0x57 with WaitState 0xf leaves sleep -> sitting on the ground
        let ev = a.on_emote(3, action::SLEEP_EMOTE, true);
        assert_eq!(ev[0], Event::Emote { dynel: 3, id: 0x44, name: "sleep", clip: "social-sleep".into() });
        assert_eq!(a.pose(3), Pose::Sleeping);
        a.on_stat(3, stat::WAIT_STATE, wait::SLEEP);
        assert_eq!(a.on_action(3, id::STAND_UP), [Event::Pose { dynel: 3, from: Pose::Sleeping, to: Pose::SitGround }]);
        // WaitState 1 while sitting = on an item
        assert_eq!(a.on_stat(3, stat::WAIT_STATE, wait::ON_ITEM), [Event::Pose { dynel: 3, from: Pose::SitGround, to: Pose::SitItem }]);
        a.on_stat(3, stat::WAIT_STATE, 0);
        assert_eq!(a.pose(3), Pose::SitGround);
        // lounge
        a.on_stat(3, stat::WAIT_STATE, wait::LOUNGE);
        a.on_emote(3, action::LOUNGE_EMOTE, true);
        assert_eq!(a.pose(3), Pose::Lounging);
        a.on_action(3, id::STAND_UP);
        assert_eq!(a.pose(3), Pose::SitGround);
        a.on_stat(3, stat::WAIT_STATE, 0);
        a.on_action(3, id::STAND_UP);
        assert_eq!(a.pose(3), Pose::Standing);
    }

    #[test]
    fn emote_frames_and_chat_commands() {
        let mut a = Actions::new();
        let p = action::social_action(5, 1, 0x3e);
        let mut p = p;
        p[16] = 1; // the server's accepted copy: state 1 (client-built messages carry 0)
        let ev = a.on_frame(&n3_frame(1, 5, p));
        assert_eq!(ev, [Event::Emote { dynel: 5, id: 0x3e, name: "wave", clip: "social-wave".into() }]);
        // a bad id is dropped
        assert!(a.on_frame(&n3_frame(1, 5, action::social_action(5, 1, 0x48))).is_empty());
        assert_eq!(parse_emote_command("/wave").unwrap().id, 0x3e);
        assert_eq!(parse_emote_command("  /EMOTE Bow ").unwrap().id, 9);
        assert!(parse_emote_command("/emote").is_none());
        assert!(parse_emote_command("wave").is_none());
        assert!(parse_emote_command("/nosuch").is_none());
    }

    #[test]
    fn pose_clips_exist_for_every_pose() {
        for p in [Pose::SitGround, Pose::SitItem, Pose::Sleeping, Pose::Lounging, Pose::Crawling] {
            assert!(p.idle_clip().is_some() && p.enter_clip().is_some());
        }
        assert!(Pose::Standing.idle_clip().is_none());
        assert_eq!(Pose::SitGround.exit_clip(), Some("ground-stop"));
    }
}
