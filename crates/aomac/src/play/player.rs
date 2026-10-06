//! The own character in the zone: glue between [`Controls`] (keys/mouse, `controls.rs`), [`Movement`] (the client's
//! movement state machine, `movement.rs`), the playfield [`Collision`], the [`Avatar`] model (`avatar.rs`) and the
//! third-person [`Camera3p`] (`camera.rs`). One `Player` per world (rebuilt when the own dynel is announced again).

use super::avatar::{self, Avatar, AvatarPose};
use super::camera::{self, Camera3p};
use super::camera_views::{door_closed, Sight, Views};
use super::fightmode::{FightLevels, DEFAULT_LEVEL};
use super::controls::{CamCmd, Cmd, ControlPrefs, Controls};
use super::movement::{id as mv, mode, Movement, World};
use super::zone::{scene_pos, scene_yaw, OwnEvent, Zone, IN_PLAY_STAT};
use ao_formats::character::Role;
use ao_formats::playfield::{camera_views, zone_locator};
use ao_formats::playfield::collision::{Aligned, Body, Collision, SurfaceState, FOOT_CLEARANCE};
use ao_gui::{InputEvent, MouseButton};
use ao_net::frame::Frame;
use ao_net::n3::action::{sit_toggle, Outgoing};
use ao_net::n3::outgoing::{char_dc_move, n3_frame};
use ao_rdb::RecordStore;
use ao_render::{GameInput, Host};
use std::path::Path;

/// Length of the jump's ceiling ray (f32 100.0 @ GC 0x10155eb0).
const JUMP_CEILING_RAY: f32 = 100.0;

/// Server-coordinate view of the scene-coordinate [`Collision`] for [`Movement`].
struct Ground<'a>(Option<&'a Collision>);

fn flip(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

impl World for Ground<'_> {
    fn ground(&self, p: [f32; 3]) -> Option<f32> {
        self.0?.ground(flip(p))
    }
    fn ceiling(&self, p: [f32; 3]) -> Option<f32> {
        self.0?.line(flip(p), flip([p[0], p[1] + JUMP_CEILING_RAY, p[2]])).map(|h| h.p[1])
    }
    fn align(&self, old: [f32; 3], new: [f32; 3], body: &Body, st: &mut SurfaceState) -> Aligned {
        match self.0 {
            Some(c) => {
                let mut r = c.align(flip(old), flip(new), body, st);
                r.pos = flip(r.pos);
                r
            }
            // no collision records: a free walk at the old height (nothing to land on)
            None => Aligned { pos: [new[0], old[1], new[2]], airborne: false, normal: [0.0, 1.0, 0.0], liquid: -9999.0 },
        }
    }
}

pub(super) struct Player {
    store: RecordStore,
    collision: Option<Collision>,
    movement: Movement,
    avatar: Avatar,
    controls: Controls,
    camera: Camera3p,
    char_id: u32,
    /// `Zone::own_serial` this player was built from.
    serial: u32,
    clock: f32,
    model_sent: bool,
    lens_set: bool,
    /// `ViewDistance` pref (0..1, docs/chat/dvalue.md): the lens' far plane is [`camera::far_plane`] of it.
    view_distance: f32,
    /// The left/right press that went to the GUI: its release must not reach the controls.
    gui_press: [bool; 2],
    /// The zone's doors were handed to this collision world once ([`Player::door_rooms`]).
    doors_synced: bool,
    game: Vec<Cmd>,
    /// World clicks `Controls` accepted (movement <= 0.02), for the interaction layer (`interact_play.rs`).
    clicks: Vec<ao_gui::MouseButton>,
    /// A one-shot clip over the movement pose (emote, attack swing, death): the role and whether it holds its last frame.
    transient: Option<(Role, bool)>,
    /// The character is in a fight (`idle-unarmed` stance while standing).
    pub fighting: bool,
    /// District fight-mode data of the playfield (`fightmode.rs`).
    fight: Option<FightLevels>,
}

/// The state of the door at scene position `pos` into its room link (nothing for a door that links no rooms, `0xffff` = leads nowhere).
fn set_door(c: &mut Collision, pos: [f32; 3], open: bool, passable: bool) {
    if let Some((a, b)) = c.door_link_from_pos(pos).filter(|&(_, b)| b != 0xffff) {
        c.set_door_open(a, b, open);
        c.set_door_passable(a, b, passable);
    }
}

impl Player {
    /// `None` until the zone knows the own `SimpleCharFullUpdate`.
    pub fn new(dir: &Path, zone: &Zone, playfield: u32) -> Option<Self> {
        let u = zone.own_update.as_deref()?;
        let built = (|| -> anyhow::Result<Self> {
            let store = RecordStore::open(dir)?;
            let avatar = Avatar::new(&store, dir, zone.char_id, u)?;
            let collision = Collision::load(&store, playfield).map_err(|e| eprintln!("collision {playfield}: {e:#}")).ok();
            let mut movement = Movement::new(u.pos, u.yaw().unwrap_or(0.0), u.run_speed);
            movement.set_body_radius(avatar.body_radius());
            if let Some(c) = &collision {
                if let Some(g) = c.ground(flip(u.pos)) {
                    movement.teleport([u.pos[0], g + FOOT_CLEARANCE, u.pos[2]], u.yaw().unwrap_or(0.0));
                }
            }
            let prefs_xml = std::fs::read_to_string(dir.join("cd_image/gui/Default/CharPrefs.xml")).unwrap_or_default();
            let prefs = ControlPrefs::from_xml(&prefs_xml);
            let mut camera = Camera3p::new(&prefs, avatar.head_height().unwrap_or(camera::MIN_PIVOT_HEIGHT));
            // scripted views (Shift/Ctrl+F8): only playfields that have camera attractors need the zone locator
            match (camera_views(&store, playfield), zone_locator(&store, playfield)) {
                (Ok(v), Ok(l)) if !v.is_empty() => camera.set_views(Views::new(v, Box::new(move |p| l.zone_at(p).unwrap_or(0)))),
                (Err(e), _) | (_, Err(e)) => eprintln!("camera views {playfield}: {e:#}"),
                _ => {}
            }
            let fight = FightLevels::load(&store, playfield);
            let controls = Controls::from_char_prefs(prefs, &prefs_xml);
            Ok(Self {
                store,
                collision,
                movement,
                avatar,
                controls,
                camera,
                char_id: zone.char_id,
                serial: zone.own_serial,
                clock: 0.0,
                model_sent: false,
                lens_set: false,
                view_distance: camera::VIEW_DISTANCE,
                gui_press: [false; 2],
                doors_synced: false,
                game: Vec::new(),
                clicks: Vec::new(),
                transient: None,
                fighting: false,
                fight,
            })
        })();
        built.map_err(|e| eprintln!("player: {e:#}")).ok()
    }

    /// The `ViewDistance` pref changed (`FUN_1001fc91` replaces the camera's far plane at once): the next frame sends the lens again.
    pub fn set_view_distance(&mut self, vd: f32) {
        if vd != self.view_distance {
            self.view_distance = vd;
            self.lens_set = false;
        }
    }

    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// Movement FSM mode (`FUN_100704e6`; 4 = swimming).
    pub fn mode(&self) -> u32 {
        u32::from(self.movement.fsm().mode)
    }

    /// Plays `role` once over the movement pose (`hold`: keep the last frame, for the death clip).
    pub fn play(&mut self, role: Role, hold: bool) {
        self.transient = Some((role, hold));
    }

    /// A held clip (death) is playing.
    pub fn holding(&self) -> bool {
        self.transient.as_ref().is_some_and(|t| t.1)
    }

    /// `Door_t` open / close and lock changes of the zone's doors into the collision world: the door's room link
    /// (`n3Room_t::GetDoorLinkFromPos` through `Door_t::LinkDoorToRooms`) gets the open flag (`n3RoomMonitor_t::DoorOpened/Closed`, read by the
    /// camera's attractor test) and the pass flag (`Door_t::CanPass`, read by `VetoRoomTransition`).
    fn door_rooms(&mut self, zone: &mut Zone) {
        if !std::mem::replace(&mut self.doors_synced, true) {
            zone.world.resync_doors(); // doors that changed before this collision world existed
        }
        let Some(c) = self.collision.as_mut() else { return };
        for (pos, open, passable) in zone.world.take_door_rooms() {
            set_door(c, pos, open, passable);
        }
    }

    /// `FUN_100585ee` [GC], run every frame by the control dynel's `Run` (`FUN_1005b016`): the feet position inside the teleportal of its zone
    /// (`n3Zone_t::IsPosInTeleportal` [N3 0x1001a86a], [`Collision::in_teleportal`]) and the gate `FUN_1005859d` open ->
    /// `n3EngineClientAnarchy_t::StartTeleportTry` [GC 0x10018f9a]: `N3Msg_MovementChanged(0x16)` (a position sync, no FSM transition), the
    /// `TeleportTrier_t` (`Zone::start_teleport_try`, which the flow turns into `TeleportStartedMessage`) and `StartTryingTeleport`'s
    /// `FUN_10059ae5(1)` (full stop while moving). The trier then runs for 30 s waiting for the server's `n3TeleportIIR_t`.
    fn teleport_try(&mut self, dt: f32, zone: &mut Zone) {
        if zone.trier.is_none() && self.teleport_gate(zone) && self.collision.as_ref().is_some_and(|c| c.in_teleportal(flip(self.movement.pos()))) {
            self.movement.action(mv::SYNC, self.clock);
            zone.start_teleport_try();
            self.movement.stop_if_moving();
        }
        zone.run_trier(dt);
    }

    /// `FUN_1005859d` [GC]: a player character (`+0x21c` clear) that is not dying (`+0x80`), has stat `InPlay` (0xC2) set and not the dead
    /// flag 0x20 (set by `CharDie_t`, docs/zone/combat-anim.md). [UNRESOLVED] The test of dynel flag bit 17 (0x20000, `+0x138`) has no writer
    /// in Gamecode.dll (a `PUSH 0x20000` scan finds two readers that mask `Features` bit 2 and one clear in the NPC initialiser), so it is
    /// taken as clear.
    fn teleport_gate(&self, zone: &Zone) -> bool {
        zone.stat(IN_PLAY_STAT).is_some_and(|v| v != 0) && !self.holding()
    }

    /// Ends a held clip (resurrection).
    pub fn stand(&mut self) {
        self.transient = None;
    }

    /// The point `to` is visible from `from` (no collision geometry in between): effects of occluded dynels are hidden by the depth test.
    pub fn line_clear(&self, from: [f32; 3], to: [f32; 3]) -> bool {
        self.collision.as_ref().is_none_or(|c| segment_clear(c, from, to))
    }

    /// CTRL / ALT held (mouse clicks do not carry modifiers).
    pub fn attack_modifier(&self) -> bool {
        self.controls.attack_modifier()
    }

    /// Mouse clicks (no look / drag) since the last call.
    pub fn take_clicks(&mut self) -> Vec<ao_gui::MouseButton> {
        std::mem::take(&mut self.clicks)
    }

    /// Commands for the combat layer collected since the last call.
    pub fn take_game(&mut self) -> Vec<Cmd> {
        std::mem::take(&mut self.game)
    }

    fn run(&mut self, cmds: Vec<Cmd>) {
        for c in cmds {
            match c {
                Cmd::Move(a) => self.movement.action(a, self.clock),
                Cmd::MouseTurn { dx, dy } => self.movement.mouse_turn(dx, dy, self.clock),
                Cmd::ToggleWalk => self.movement.toggle_run(self.clock),
                Cmd::Camera(cc) => {
                    if let Some(turn) = self.camera.apply(&cc) {
                        self.movement.mouse_turn(turn, 0.0, self.clock);
                        self.movement.end_mouse_look(self.clock);
                    } else if cc == CamCmd::EndLook {
                        self.movement.end_mouse_look(self.clock);
                    }
                }
                // selection clicks are the HUD's (`Hud::input`); item pick-up / screenshot have no consumer yet
                // attack / sit / special attacks belong to the combat layer (`combat::Module`, run by the flow)
                Cmd::Attack | Cmd::SwitchTarget | Cmd::Sit | Cmd::Special(_) => self.game.push(c),
                Cmd::Click(b) => self.clicks.push(b),
                Cmd::PickupItem | Cmd::Screenshot => {}
            }
        }
    }

    /// Keyboard and mouse-look deltas (`Frontend::game_input`).
    pub fn game_input(&mut self, ev: GameInput) {
        let cmds = match ev {
            GameInput::Key { code, pressed, .. } => self.controls.on_key(code, pressed),
            GameInput::MouseMotion { dx, dy } => self.controls.on_mouse_motion(dx, dy),
        };
        self.run(cmds);
    }

    /// Mouse buttons and the wheel (`Frontend::input`); `over_gui` = the cursor is over a GUI window.
    pub fn mouse(&mut self, ev: &InputEvent, over_gui: bool) {
        let btn = |b: MouseButton| match b {
            MouseButton::Left => Some(0),
            MouseButton::Right => Some(1),
            _ => None,
        };
        let cmds = match *ev {
            InputEvent::MouseDown { button, .. } => {
                if let Some(i) = btn(button) {
                    self.gui_press[i] = over_gui;
                }
                if over_gui && btn(button).is_some() {
                    return;
                }
                self.controls.on_mouse_button(button, true)
            }
            InputEvent::MouseUp { button, .. } => {
                if btn(button).is_some_and(|i| std::mem::take(&mut self.gui_press[i])) {
                    return;
                }
                self.controls.on_mouse_button(button, false)
            }
            InputEvent::Wheel { dy, .. } if !over_gui => self.controls.on_wheel(dy),
            _ => return,
        };
        self.run(cmds);
    }

    /// One frame: advance the movement, return the `CharDCMove` frames to send, place avatar + camera.
    pub fn frame(&mut self, dt: f32, host: &mut Host, zone: &mut Zone, text_input: bool) -> Vec<Frame> {
        self.clock += dt;
        self.controls.set_text_input(text_input);
        host.look = self.controls.mouse_capture();
        let s = |id| zone.stat(id);
        self.movement.set_stats(|st| {
            // stat ids: RunSpeed 0x9C, Health 0x1B, Life 1, TurnSpeed 0x10B, Strength 0x10, Agility 0x11, Features 0xE0, ...
            for (field, id) in [
                (&mut st.run_speed, 0x9C),
                (&mut st.health, 0x1B),
                (&mut st.max_health, 1),
                (&mut st.turn_speed, 0x10B),
                (&mut st.strength, 0x10),
                (&mut st.agility, 0x11),
                (&mut st.features, 0xE0),
                (&mut st.mech_data, 0x296),
                (&mut st.gm_level, 0xD7),
                (&mut st.monster_scale, 0x168),
                (&mut st.wait_state, 0x1AE),
                (&mut st.flags, 0),
            ] {
                if let Some(v) = s(id) {
                    *field = v;
                }
            }
        });
        self.apply_own_events(zone);
        zone.fight_level = Some(self.fight_level(self.movement.pos()));
        // `FUN_10070fee`: the target branch re-runs the Features / district gate every frame and reads the target's `GetRelPos`
        if let Some(t) = self.movement.chase_target() {
            if self.follow_gated() {
                self.movement.drop_chase();
            } else {
                let p = if t == self.char_id as i32 { Some(self.movement.pos()) } else { zone.dynels.get(&t).map(|d| d.pos) };
                self.movement.set_chase_pos(p);
            }
        }
        for (id, v) in self.movement.take_stat_writes() {
            zone.stats.insert(id, v);
        }
        self.door_rooms(zone);
        self.teleport_try(dt, zone);
        let world = Ground(self.collision.as_ref());
        let out: Vec<Frame> = self.movement.update(dt, &world).iter().map(|m| n3_frame(0, self.char_id, char_dc_move(self.char_id as i32, m))).collect();
        for (id, v) in self.movement.take_stat_writes() {
            zone.stats.insert(id, v);
        }
        let (pos, yaw) = (self.movement.pos(), self.movement.yaw());
        if let Some(d) = zone.dynels.get_mut(&(self.char_id as i32)) {
            d.pos = pos;
            d.yaw = Some(yaw);
        }

        let role = self.movement.role();
        let moving = self.movement.speed() > 0.01 && self.movement.grounded();
        // emotes and swings end with their clip or when the character moves; a death clip holds until `stand`
        if self.transient.as_ref().is_some_and(|(_, hold)| !hold && (moving || self.avatar.finished())) {
            self.transient = None;
        }
        let pose = match &self.transient {
            Some((r, _)) => AvatarPose::still(r.clone()),
            None if moving => AvatarPose { role, speed: self.movement.max_speed(), ref_speed: self.movement.ref_speed() },
            None if self.fighting && role == Role::Idle => AvatarPose::still(Role::IdleCombat),
            None => AvatarPose::still(role),
        };
        if let Err(e) = self.avatar.set_pose(&self.store, pose) {
            eprintln!("avatar pose: {e:#}");
        }
        self.avatar.set_transform(scene_pos(pos), scene_yaw(yaw));
        self.avatar.update(dt);

        if let Some(h) = self.avatar.head_height() {
            self.camera.set_head(h);
        }
        let col = self.collision.as_ref();
        let clear = |a: [f32; 3], b: [f32; 3]| col.is_none_or(|c| segment_clear(c, a, b));
        let closed = |a: [f32; 3], b: [f32; 3]| col.is_some_and(|c| door_closed(c, a, b));
        let ground = |p: [f32; 3]| col.and_then(|c| c.ground(p));
        host.camera = self.camera.update_with(scene_pos(pos), yaw, dt, &Sight { clear: &clear, door_closed: &closed, ground: &ground });
        if !self.lens_set {
            // FOV 90 degrees horizontal, near 0.2 (`VisualCamera_t`, docs/zone/camera.md); far = `ViewDistance` * 1000 m (`FUN_1001fc91`)
            let mut lens = camera::lens(host.lens.unwrap_or(zone.world.lens));
            lens.far = Some(camera::far_plane(self.view_distance));
            host.lens = Some(lens);
            self.lens_set = true;
        }
        if !self.model_sent {
            host.actor_models.push((avatar::MODEL_KEY, self.avatar.model().clone()));
            self.model_sent = true;
        }
        if self.camera.show_avatar() {
            host.actors.push(self.avatar.frame());
        }
        out
    }

    /// `N3Msg_SitToggle` [GC 0x10028e0a] without the attack stop (the combat layer's): the frames the original sends. A `Move(0x1e)` is applied
    /// here and goes out with the movement frames; the rest (`CharacterActionIIR_t` 0x57 / 0x55) is for the caller.
    pub fn sit(&mut self) -> Vec<Outgoing> {
        let out = sit_toggle(&self.movement.sit_input(false));
        for o in &out {
            if let Outgoing::Move(a) = o {
                self.movement.action(*a, self.clock);
            }
        }
        out
    }

    /// `MovementChanged(0x1e)` of `N3Msg_StartCamping`.
    pub fn sit_ground(&mut self) {
        self.movement.action(mv::SWITCH_SIT_GROUND, self.clock);
    }

    /// Server messages for the own dynel (docs/zone/movement.md §10), in arrival order.
    fn apply_own_events(&mut self, zone: &mut Zone) {
        for e in std::mem::take(&mut zone.own_events) {
            match e {
                OwnEvent::Place { pos, yaw, full_reset } => {
                    if full_reset {
                        self.movement.teleport(pos, yaw);
                    } else {
                        self.movement.set_pos(pos, false);
                    }
                }
                OwnEvent::SetPos { pos, stop } => self.movement.set_pos(pos, stop),
                OwnEvent::Impulse { delta, time } => self.movement.impulse(delta, time),
                OwnEvent::Follow { mode: m, target, pos, path } => {
                    self.movement.follow_place(pos);
                    if !self.follow_gated() {
                        self.movement.follow_target(m, &path, target);
                    }
                }
                OwnEvent::Action(a) => match a {
                    // sit relay / stand up: `FUN_1005d0d8` [GC 0x1005d723 / 0x1005d72f]
                    0x56 => {
                        self.movement.transition(mv::SWITCH_SIT_GROUND);
                    }
                    0x57 => {
                        let t = match self.movement.stats().wait_state {
                            0xF => mv::LEAVE_SLEEP,
                            0x10 => mv::LEAVE_LOUNGE,
                            _ => mv::LEAVE_SIT,
                        };
                        self.movement.transition(t);
                    }
                    // death: `FUN_10059ae5(1)`; 0xAD: leave sneak [GC 0x1005e489]
                    0x63 => {
                        self.movement.stop_if_moving();
                    }
                    0xAD if self.movement.fsm().mode == mode::SNEAK => {
                        self.movement.transition(mv::LEAVE_SNEAK);
                    }
                    _ => {}
                },
                OwnEvent::Resurrected => {}
                OwnEvent::Spells { spells, apply } => {
                    for sp in &spells {
                        self.movement.apply_spell(sp, apply);
                    }
                }
                OwnEvent::Relocated { pos, .. } => {
                    if let Some(p) = pos {
                        self.movement.set_pos(p, false);
                    }
                }
                OwnEvent::FightMode(u) => {
                    if let Some(f) = &mut self.fight {
                        f.update(&u);
                    }
                }
            }
        }
    }

    /// `FUN_1003e1d0`: the district fight-mode level at `pos` (2 without district data).
    fn fight_level(&self, pos: [f32; 3]) -> i32 {
        self.fight.as_ref().map_or(DEFAULT_LEVEL, |f| f.level(pos))
    }

    /// `FUN_100732e3` gate for a `FollowTargetIIR_c` on the own dynel: dropped when Features bit 0 or `0x4000000` is set or the district
    /// fight-mode level (`FUN_1003e228`) is above 1. The level comes from `PlayfieldDistrictInfo` / `FightModeHandler`, which are not read
    /// here: [UNRESOLVED] the original's no-data default 2 is used, i.e. the follow part is always dropped (the placement is not).
    fn follow_gated(&self) -> bool {
        let f = self.movement.stats().features;
        f & 1 != 0 || f & 0x400_0000 != 0 || self.fight_level(self.movement.pos()) > 1
    }

    /// `SlotMovementWalkToggle` / special actions 0x11 / 0x12: `MovementChanged(0x18 / 0x19)`.
    pub fn toggle_walk(&mut self) {
        self.movement.toggle_run(self.clock);
    }

    /// Special actions 0x14 / 0x8d, `N3Msg_CrawlToggle` [GC 0x278c9] (movement part).
    pub fn toggle_crawl(&mut self) {
        self.movement.crawl_toggle(self.clock);
    }

    /// Special action 0x4f: `N3Msg_MovementChanged(0x24)` = leave sneak mode.
    pub fn leave_sneak(&mut self) {
        self.movement.action(mv::LEAVE_SNEAK, self.clock);
    }

    /// FSM `vtable[3](0x1e)`: the sit transition is allowed (`N3Msg_StartCamping`).
    pub fn can_sit(&self) -> bool {
        self.movement.fsm().allowed(0x1e)
    }

    /// `Fsm::last_speed_mode == WALK` (the `+0x30 == 2` test of `FUN_1006d196`, hud_special.rs).
    pub fn last_speed_walk(&self) -> bool {
        self.movement.fsm().last_speed_mode == mode::WALK
    }
}

/// Free line of sight for the camera: no wall within 0.2 m of the segment and the end not below the terrain/floor.
fn segment_clear(c: &Collision, a: [f32; 3], b: [f32; 3]) -> bool {
    const STEPS: usize = 8;
    (1..=STEPS).all(|i| {
        let t = i as f32 / STEPS as f32;
        let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        c.sphere_hit(p, 0.2).is_none()
    }) && c.ground([b[0], b[1] + 3.0, b[2]]).is_none_or(|g| g < b[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real data (skips without the client): the door of ICC Holodeck Alien Training (6131) found by its position opens / locks its room link.
    #[test]
    fn a_door_position_sets_its_room_link() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let store = RecordStore::open(&dir).unwrap();
        let mut c = Collision::load(&store, 6131).unwrap();
        let s = ao_formats::playfield::load_playfield(&store, &dir, 6131).unwrap().spawn.unwrap();
        let (pos, (a, b)) = (-300..=300)
            .flat_map(|x| (-300..=300).map(move |z| [s[0] + x as f32, s[1], s[2] + z as f32]))
            .find_map(|p| c.door_link_from_pos(p).filter(|l| l.1 != 0xffff).map(|l| (p, l)))
            .expect("the playfield has a door");
        let (au, bu) = (a as usize, b as usize);
        assert!(!c.door_open_between(au, bu) && c.room_transition_allowed(a as i32, b as i32));
        set_door(&mut c, pos, true, false); // an open door the character may not pass (locked)
        assert!(c.door_open_between(au, bu) && !c.room_transition_allowed(a as i32, b as i32));
        set_door(&mut c, pos, false, true);
        assert!(!c.door_open_between(au, bu) && c.room_transition_allowed(b as i32, a as i32));
    }
}
