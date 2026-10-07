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
use super::combat::actions::Pose;
use super::combat::anim::anim_name;
use ao_formats::character::Role;
use ao_formats::playfield::{camera_views, zone_locator, ZoneLocator};
use ao_formats::playfield::collision::{Aligned, Body, Collision, SurfaceState, FOOT_CLEARANCE};
use ao_gui::{InputEvent, MouseButton};
use ao_net::frame::Frame;
use ao_net::n3::action::{sit_toggle, Outgoing};
use ao_net::n3::outgoing::{char_dc_move, n3_frame};
use ao_rdb::RecordStore;
use ao_render::{GameInput, Host};
use std::path::Path;
use std::rc::Rc;

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
    /// `FadeCharacter*` prefs and the cached factor of `VisualCATMesh_t::RefreshAlpha` ([`avatar::Fade`]).
    fade: avatar::Fade,
    fader: avatar::Fader,
    /// The left/right press that went to the GUI: its release must not reach the controls.
    gui_press: [bool; 2],
    /// The zone's doors were handed to this collision world once ([`Player::door_rooms`]).
    doors_synced: bool,
    game: Vec<Cmd>,
    /// World clicks `Controls` accepted (movement <= 0.02), for the interaction layer (`interact_play.rs`).
    clicks: Vec<ao_gui::MouseButton>,
    /// A one-shot clip over the movement pose (emote, attack swing, death): the role and whether it holds its last frame.
    transient: Option<(Role, bool)>,
    cast_loop: Option<bool>,
    cast_restart: bool,
    /// `ItemDelay` of the weapon of the swing in `transient`.
    swing_delay: Option<i32>,
    /// `transient` is a weapon swing (its notes start the attack sounds, `combat::notes`).
    swinging: bool,
    swing_key: Option<u16>,
    /// Playback rate factor of the hit reaction in `transient`.
    clip_scale: Option<f32>,
    /// The pose the movement role showed last frame (`None` before the first update).
    pose: Option<Pose>,
    /// The AnimHolder's fight idle is on (`combat/glue.rs::stance`: set by the draw of a fight start, cleared by the holster of a fight stop); the
    /// character then stands in the weapon's fight idle, bare-handed in the fighting stance.
    pub fighting: bool,
    /// District fight-mode data of the playfield (`fightmode.rs`).
    fight: Option<FightLevels>,
    /// `n3Playfield_t::GetZoneInstance` of the playfield: the own dynel's zone instance feeds `Movement::zone_instance`.
    zones: Option<Rc<ZoneLocator>>,
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
            movement.restore_blob(&u.blob);
            let prefs_xml = std::fs::read_to_string(dir.join("cd_image/gui/Default/CharPrefs.xml")).unwrap_or_default();
            let prefs = ControlPrefs::from_xml(&prefs_xml);
            let mut camera = Camera3p::new(&prefs, avatar.head_height().unwrap_or(camera::MIN_PIVOT_HEIGHT));
            // scripted views (Shift/Ctrl+F8): only playfields that have camera attractors need the zone locator
            let zones = zone_locator(&store, playfield).map_err(|e| eprintln!("zone locator {playfield}: {e:#}")).ok().map(Rc::new);
            match (camera_views(&store, playfield), &zones) {
                (Ok(v), Some(l)) if !v.is_empty() => {
                    let l = l.clone();
                    camera.set_views(Views::new(v, Box::new(move |p| l.zone_at(p).unwrap_or(0))));
                }
                (Err(e), _) => eprintln!("camera views {playfield}: {e:#}"),
                _ => {}
            }
            let fight = FightLevels::load(&store, playfield);
            let controls = Controls::new(prefs);
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
                fade: avatar::Fade::default(),
                fader: avatar::Fader::default(),
                gui_press: [false; 2],
                doors_synced: false,
                game: Vec::new(),
                clicks: Vec::new(),
                transient: None,
                cast_loop: None,
                cast_restart: false,
                swing_delay: None,
                swinging: false,
                swing_key: None,
                clip_scale: None,
                pose: None,
                fighting: false,
                fight,
                zones,
            })
        })();
        built.map_err(|e| eprintln!("player: {e:#}")).ok()
    }

    /// Loaded playfield surface used by authored effect collision queries (scene coordinates).
    pub fn effect_surface(&self) -> Option<&Collision> {
        self.collision.as_ref()
    }

    /// The control options (`ControlPrefs::from_dvalues`) changed: mouse look, zoom, wheel, inversion, own avatar in first person.
    pub fn set_control_prefs(&mut self, p: &ControlPrefs) {
        self.controls.set_prefs(p.clone());
        self.camera.set_prefs(p);
    }

    pub fn select_camera_mode(&mut self, mode: u8) {
        self.camera.select_mode(mode);
    }

    pub fn camera_mode(&self) -> u8 {
        self.camera.selected_mode()
    }

    /// The shared key binding table (`options/keys.rs`) changed: movement, camera, combat and pick-up keys follow at once.
    pub fn set_keys(&mut self, b: &super::options::keys::Bindings, f: &super::options::keys::FixedKeys) {
        self.controls.set_keys(b, f);
    }

    /// The `ViewDistance` pref changed (`FUN_1001fc91` replaces the camera's far plane at once): the next frame sends the lens again.
    pub fn set_view_distance(&mut self, vd: f32) {
        if vd != self.view_distance {
            self.view_distance = vd;
            self.lens_set = false;
        }
    }

    /// The `FadeCharacter*` Char prefs (read each frame, applied by the next [`Player::frame`] like the original's changed callbacks).
    pub fn set_fade(&mut self, fade: avatar::Fade) {
        self.fade = fade;
    }

    pub fn effect_anchor(&self, id: i32) -> Option<[[f32; 4]; 4]> {
        self.avatar.effect_anchor(id)
    }

    pub fn weapon_effect_anchor(&self, place: u8) -> Option<[[f32; 4]; 4]> {
        self.avatar.weapon_effect_anchor(place)
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
        self.cast_loop = None;
        self.cast_restart = false;
        self.swing_delay = None;
        self.swinging = false;
        self.swing_key = None;
        self.clip_scale = None;
    }
    pub fn cast_animation(&mut self, animation: Option<(Role, bool)>) {
        match animation {
            Some((role, looping)) => {
                self.play(role, false);
                self.cast_loop = Some(looping);
                self.cast_restart = true;
            }
            None if self.cast_loop.is_some() => {
                self.transient = None;
                self.cast_loop = None;
                self.cast_restart = false;
            }
            None => {}
        }
    }


    /// Plays the hit reaction `role` once at `rate` times its speed (`FUN_1009b4ac`, `Dynels::react_to_hit`) unless one is playing.
    pub fn react(&mut self, role: Role, rate: f32) {
        if self.transient.is_none() {
            self.play(role, false);
            self.clip_scale = Some(rate);
        }
    }

    /// Plays the swing `role` once (`FUN_1006a239`), sped up for the weapon's `ItemDelay` (centiseconds) when there is a weapon; its animation notes
    /// start the attack sounds ([`Player::take_notes`]).
    pub fn swing(&mut self, role: Role, item_delay: Option<i32>) {
        self.play(role, false);
        self.swing_delay = item_delay;
        self.swinging = true;
    }

    /// Retail suppresses a list key already playing, not a different key resolving to the same clip.
    pub fn swing_list(&mut self, role: Role, item_delay: Option<i32>, key: u16) {
        if self.transient.is_some() && !self.avatar.finished() && self.swing_key == Some(key) {
            return;
        }
        self.swing(role, item_delay);
        self.swing_key = Some(key);
        self.avatar.restart_clip();
    }

    #[cfg(test)]
    pub(super) fn transient_role(&self) -> Option<&Role> {
        self.transient.as_ref().map(|(role, _)| role)
    }

    /// The notes the own swing clip reached since the last call (`combat::notes` ids).
    pub fn take_notes(&mut self) -> Vec<u32> {
        self.avatar.take_notes()
    }

    /// Animation-holder completion used by the nano cast release state.
    pub fn animation_finished(&self) -> bool {
        self.transient.is_none() || self.avatar.finished()
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
        self.cast_loop = None;
        self.cast_restart = false;
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
        if let Some(z) = self.zones.as_ref().and_then(|l| l.zone_at(flip(self.movement.pos()))) {
            self.movement.zone_instance(z as u32);
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
        // the new pose's enter / stop clip plays once over its idle clip (`Pose::transition_anim`, GC 0x1006d330)
        let now = Pose::from_role(&role);
        if let Some(from) = self.pose.replace(now).filter(|&b| b != now && self.cast_loop.is_none()) {
            if let Some((name, _)) = Pose::transition_anim(from, now).and_then(anim_name) {
                self.play(Role::Clip(name.into()), false);
            }
        }
        // emotes and swings end with their clip or when the character moves; a death clip holds until `stand`
        if self.cast_loop != Some(true) && !self.cast_restart && self.transient.as_ref().is_some_and(|(_, hold)| !hold && (self.avatar.finished() || (self.cast_loop.is_none() && moving))) {
            self.transient = None;
            self.cast_loop = None;
        }
        let pose = match &self.transient {
            Some((r, _)) => AvatarPose::still(r.clone()),
            None if moving => AvatarPose { role, speed: self.movement.max_speed(), ref_speed: self.movement.ref_speed() },
            None if self.fighting && role == Role::Idle => AvatarPose::still(Role::IdleCombat),
            None => AvatarPose::still(role),
        };
        self.avatar.set_stance(zone.world.wielded_set(self.char_id as i32));
        self.avatar.set_swing_delay(self.transient.as_ref().and(self.swing_delay));
        self.avatar.set_swinging(self.transient.is_some() && self.swinging);
        self.avatar.set_clip_scale(self.transient.as_ref().and(self.clip_scale));
        self.avatar.set_cast_loop(self.cast_loop == Some(true));
        if std::mem::take(&mut self.cast_restart) { self.avatar.restart_cast_clip(); }
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
        // `VisualCATMesh_t::RunFunction` -> `RefreshAlpha`: opacity from the camera <-> head attractor distance (own character only)
        let head_to_camera = self.avatar.head_position() - host.camera.pos;
        let alpha = self.fader.step(&self.fade, head_to_camera.length_squared());
        if self.camera.show_avatar() {
            let mut frame = self.avatar.frame();
            frame.alpha = alpha;
            host.actors.push(frame);
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
                OwnEvent::Appearance(appearance) => match self.avatar.set_appearance(&self.store, &appearance) {
                    Ok(changed) => self.model_sent &= !changed, // the next frame uploads the rebuilt model again
                    Err(e) => eprintln!("avatar appearance: {e:#}"),
                },
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

/// `n3Playfield_t::LineOfSight` (N3 0x1000d34c) tests both directions against
/// front-face surface lines, including sloping rock faces, floors and ceilings.
fn segment_clear(c: &Collision, a: [f32; 3], b: [f32; 3]) -> bool {
    c.inside(a) && c.inside(b) && !door_closed(c, a, b) && c.line(a, b).is_none() && c.line(b, a).is_none()
}

/// Gamecode `FUN_100ad4bd`: ask the playfield surface for its closest point and
/// normal, then raise only the supplied point's y. The native surface query
/// includes terrain / room tiles, their KD collision volumes and liquid depth.
pub(super) fn effect_collision(c: &Collision, p: ao_render::Vec3) -> Option<(ao_render::Vec3, ao_render::Vec3)> {
    let mut point = p.to_array();
    // A fresh room=-1 state reproduces the null-source veto: accept an existing
    // room, or use GetSafePos outside rooms; no remembered dynel/door transition.
    c.veto(&mut point, &mut SurfaceState::default());
    let hit = c.closest(point, -1)?;
    point[1] = point[1].max(hit.pos[1]);
    Some((ao_render::Vec3::from_array(point), ao_render::Vec3::from_array(hit.normal)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_collision_uses_surface_normal_and_never_lowers_the_rock() {
        use ao_scene::{Instance, Mesh, Scene, Submesh, Vertex, IDENTITY};
        let vertices = [[0.0, 1.0, 0.0], [0.0, 1.0, -4.0], [4.0, 3.0, -4.0], [4.0, 3.0, 0.0]]
            .map(|pos| Vertex { pos, ..Default::default() }).to_vec();
        let collision = Collision::from_scene(&Scene {
            meshes: vec![Mesh { vertices, submeshes: vec![Submesh::new(vec![0, 2, 1, 0, 3, 2], None)] }],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }],
            ..Default::default()
        });
        let point = ao_render::Vec3::new(2.0, 5.0, -2.0);
        let (corrected, normal) = effect_collision(&collision, point).unwrap();
        assert_eq!(corrected, point, "the native helper raises y only");
        assert!((normal - ao_render::Vec3::new(-0.5, 1.0, 0.0).normalize()).length() < 1e-5);
        assert!(effect_collision(&collision, ao_render::Vec3::new(10.0, 5.0, -2.0)).is_none());
    }

    #[test]
    fn effect_collision_routes_loaded_outdoor_and_dungeon_surfaces() {
        let Ok(store) = RecordStore::open(&ao_gui::client_dir()) else { return };
        for playfield in [4582, 6131] {
            let collision = Collision::load(&store, playfield).unwrap();
            let point = ao_render::Vec3::new(-1.0, -1.0, 1.0);
            let mut vetoed = point.to_array();
            collision.veto(&mut vetoed, &mut SurfaceState::default());
            let closest = collision.closest(vetoed, -1).unwrap();
            let (corrected, normal) = effect_collision(&collision, point).unwrap();
            assert_ne!(corrected, point, "loaded surface {playfield} must correct the invalid point");
            assert_eq!(corrected.x, vetoed[0]);
            assert_eq!(corrected.z, vetoed[2]);
            assert_eq!(corrected.y, vetoed[1].max(closest.pos[1]));
            assert_eq!(normal.to_array(), closest.normal);
            assert!((normal.length() - 1.0).abs() < 1e-4, "{playfield}: {normal:?}");
        }
    }

    #[test]
    fn turning_camera_pulls_in_at_a_sloping_rock_face() {
        use ao_scene::{Instance, Mesh, Scene, Submesh, Vertex, IDENTITY};
        // An overhanging rock face has |normal.y| > 0.5: the old sampled wall
        // spheres skip it, and the endpoint ground check never considers ceilings.
        let vertices = [[-3.0, 0.0, 2.8], [3.0, 0.0, 2.8], [0.0, 6.0, -3.2]]
            .map(|pos| Vertex { pos, ..Default::default() }).to_vec();
        let scene = Scene {
            meshes: vec![Mesh { vertices, submeshes: vec![Submesh::new(vec![0, 2, 1], None)] }],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }],
            ..Default::default()
        };
        let collision = Collision::from_scene(&scene);
        let clear = |a, b| segment_clear(&collision, a, b);
        assert!(!clear([0.0, 1.5, 0.0], [0.0, 3.1, 4.8]));
        assert!(!clear([0.0, 3.1, 4.8], [0.0, 1.5, 0.0]), "visibility must test the reverse ray too");
        let mut camera = camera::Camera3p::new(&Default::default(), 1.5);
        let feet = [0.0; 3];
        let pivot = glam::Vec3::new(0.0, 1.5, 0.0);
        let open = camera.update_with(feet, std::f32::consts::PI, 0.016, &Sight::with_clear(&clear));
        assert!((open.pos - pivot).length() > 4.99);
        for yaw in [-0.2, 0.0, 0.2] {
            let view = camera.update_with(feet, yaw, 0.016, &Sight::with_clear(&clear));
            assert!((view.pos - pivot).length() < 2.0, "{:?}", view.pos);
            let dir = (view.pos - pivot).normalize();
            assert!(clear(pivot.to_array(), (view.pos + dir * (camera::COLLISION_RADIUS - camera::DEFAULT_DISTANCE * 0.001)).to_array()));
        }
        let restored = camera.update_with(feet, std::f32::consts::PI, 0.016, &Sight::with_clear(&clear));
        assert!((restored.pos - pivot).length() > 4.99);
    }

    /// Offline ICC geometry/camera frame route; no login, live session or window.
    #[test]
    #[ignore = "requires a measured ICC rock position and an output directory"]
    fn icc_rock_camera_turn_frames() {
        let pos: Vec<f32> = std::env::var("AOMAC_CAMERA_ROCK_POS").expect("server x,y,z position")
            .split(',').map(|v| v.parse::<f32>().unwrap()).collect();
        assert!(pos.len() == 3 && pos.iter().all(|v| v.is_finite()), "server x,y,z position");
        let out = std::path::PathBuf::from(std::env::var("AOMAC_CAMERA_ROCK_SHOTS").expect("frame output directory"));
        std::fs::create_dir_all(&out).unwrap();
        let dir = ao_gui::client_dir();
        let store = RecordStore::open(&dir).unwrap();
        let scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, ao_formats::playfield::DEFAULT_DAY_TIME).unwrap();
        let collision = Collision::load(&store, 4582).unwrap();
        let feet = [pos[0], pos[1], -pos[2]];
        let pivot = glam::Vec3::from(feet) + glam::Vec3::Y * 1.5;
        let clear = |a, b| segment_clear(&collision, a, b);
        let mut camera = camera::Camera3p::new(&Default::default(), 1.5);
        let mut pulled = 0;
        for i in 0..36 {
            let yaw = i as f32 * std::f32::consts::TAU / 36.0;
            let view = camera.update_with(feet, yaw, 1.0 / 60.0, &Sight::with_clear(&clear));
            let distance = (view.pos - pivot).length();
            pulled += usize::from(distance < camera::DEFAULT_DISTANCE - 0.01);
            let dir = (view.pos - pivot).normalize();
            assert!(clear(pivot.to_array(), (view.pos + dir * (camera::COLLISION_RADIUS - camera::DEFAULT_DISTANCE * 0.001)).to_array()), "heading {i}: pivot or boom is blocked beyond the original bisection tolerance");
            eprintln!("ICC camera heading {i}: eye {:?}, boom {distance:.4}", view.pos);
            ao_render::render_to_png(&scene, view.pos.to_array(), pivot.to_array(), 1200, 700, &out.join(format!("icc-rock-{i:02}.png"))).unwrap();
        }
        assert!(pulled > 0, "the supplied ICC position never exercises collision pull-in");
    }

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
