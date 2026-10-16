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
use ao_formats::character::actor::ActorAssets;
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
use std::cell::RefCell;

/// Length of the jump's ceiling ray (f32 100.0 @ GC 0x10155eb0).
const JUMP_CEILING_RAY: f32 = 100.0;
/// Audit item 8 (aomac Enhanced, not retail: pinch-to-zoom has no retail equivalent to calibrate against). Chosen so
/// a full pinch gesture (magnification roughly doubling, `delta` summing to about 1.0) is comparable to a few mouse
/// wheel notches. ponytail: uncalibrated against a real trackpad; retune here if it feels too fast/slow.
const PINCH_NOTCHES_PER_UNIT: f32 = 10.0;

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
    collision: Option<Rc<RefCell<Collision>>>,
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
    /// aomac Enhanced "Widescreen FOV" option (not retail, audit item 16): keeps the 4:3 vertical angle instead
    /// of the retail fixed 90° horizontal angle ([`camera::lens`]).
    widescreen_fov: bool,
    /// aomac Enhanced "Extended Scenery Distance" (not retail, audit item 15, 1.0-2.0x): multiplies the far plane
    /// that `camera::far_plane` already derives from `view_distance`, so terrain/statel draw distance, fog end and
    /// statel LOD extend together (`Renderer::set_lens`) while `char_view_distance`/name-tag radius are untouched.
    scenery_distance_mult: f32,
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
    /// A one-shot body pose (emote, cast, death): the role and whether it holds its last frame.
    transient: Option<(Role, bool)>,
    cast_loop: Option<bool>,
    cast_restart: bool,
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
    /// F12 screenshot (retail, `player.rs::run`'s `Cmd::Screenshot`): consumed by the flow via [`Player::take_screenshot`].
    screenshot_requested: bool,
}

/// The state of the door at scene position `pos` into its room link (nothing for a door that links no rooms, `0xffff` = leads nowhere).
fn set_door(c: &mut Collision, pos: [f32; 3], open: bool, passable: bool) {
    if let Some((a, b)) = c.door_link_from_pos(pos).filter(|&(_, b)| b != 0xffff) {
        c.set_door_open(a, b, open);
        c.set_door_passable(a, b, passable);
    }
}

impl Player {
    pub fn set_texture_quality(&self, quality: i32) {
        self.store.set_other_texture_quality(quality);
    }

    /// `None` until the zone knows the own `SimpleCharFullUpdate`. `zone` is `&mut` only to reclaim the warmed
    /// `ActorAssets` parked by the previous `Player` ([`Dynels::take_avatar_assets`](super::dynels::Dynels::take_avatar_assets));
    /// nothing about the own dynel's data is mutated.
    pub fn new(dir: &Path, zone: &mut Zone, playfield: u32) -> Option<Self> {
        zone.own_update.as_ref()?;
        let assets_hint = zone.world.take_avatar_assets();
        let u = zone.own_update.as_deref()?;
        let built = (|| -> anyhow::Result<Self> {
            let store = RecordStore::open(dir)?;
            store.set_other_texture_quality(zone.world.texture_quality());
            let assets = match assets_hint {
                Some(a) => a,
                None => ActorAssets::new(&store)?,
            };
            let mut avatar = Avatar::new(&store, dir, zone.char_id, u, assets)?;
            avatar.set_body_stats(&store, u.mode, zone.stat(0x2f).map_or(u.fatness, |v| v as u8), zone.stat(0x378).unwrap_or(0))?;
            let collision = Collision::load(&store, playfield).map_err(|e| eprintln!("collision {playfield}: {e:#}")).ok();
            let mut movement = Movement::new(u.pos, u.yaw().unwrap_or(0.0), u.run_speed);
            movement.set_input_lock(u.flag_32f); // GC10077e13 copies the full-update byte to +21d.
            movement.set_body_radius(avatar.body_radius());
            if let Some(c) = &collision {
                if let Some(g) = c.ground(flip(u.pos)) {
                    movement.teleport([u.pos[0], g + FOOT_CLEARANCE, u.pos[2]], u.yaw().unwrap_or(0.0));
                }
            }
            movement.restore_blob(&u.blob);
            let prefs_xml = std::fs::read_to_string(dir.join("cd_image/gui/Default/CharPrefs.xml")).unwrap_or_default();
            let prefs = ControlPrefs::from_xml(&prefs_xml);
            let mut camera = Camera3p::new(&prefs, None);
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
                collision: collision.map(|collision| Rc::new(RefCell::new(collision))),
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
                widescreen_fov: false,
                scenery_distance_mult: 1.0,
                fade: avatar::Fade::default(),
                fader: avatar::Fader::default(),
                gui_press: [false; 2],
                doors_synced: false,
                game: Vec::new(),
                clicks: Vec::new(),
                transient: None,
                cast_loop: None,
                cast_restart: false,
                clip_scale: None,
                pose: None,
                fighting: false,
                fight,
                zones,
                screenshot_requested: false,
            })
        })();
        built.map(|mut player| {
            player.sync_travel_appearance(zone);
            player
        }).map_err(|e| eprintln!("player: {e:#}")).ok()
    }

    /// Reclaims the own avatar's `ActorAssets`, to hand to [`Dynels::stash_avatar_assets`](super::dynels::Dynels::stash_avatar_assets)
    /// before this `Player` is dropped (a zone change or a re-placed own dynel).
    pub fn into_avatar_assets(self) -> ActorAssets {
        self.avatar.into_assets()
    }

    /// Loaded playfield surface used by authored effect collision queries (scene coordinates).
    pub fn effect_surface(&self) -> Option<Rc<RefCell<Collision>>> {
        self.collision.clone()
    }

    /// TargetSelector (GC 0x10025b96): world LOS between both feet raised by 1.6 m (`10157454`).
    pub(super) fn target_line_of_sight(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        self.collision.as_ref().is_none_or(|c| {
            let mut a = scene_pos(a);
            let mut b = scene_pos(b);
            a[1] += 1.6;
            b[1] += 1.6;
            segment_clear(&c.borrow(), a, b)
        })
    }

    /// The control options (`ControlPrefs::from_dvalues`) changed: mouse look, zoom, wheel, inversion, own avatar in first person.
    pub fn set_control_prefs(&mut self, p: &ControlPrefs) {
        self.controls.set_prefs(p.clone());
        self.camera.set_prefs(p);
    }

    #[cfg(test)]
    pub(super) fn assert_option_prefs(&self, d: &super::dvalue::DValues) {
        let expected = ControlPrefs::from_dvalues(d);
        assert_eq!(self.controls.option_prefs(), &expected, "actual mouse controls");
        let camera = self.camera.option_prefs();
        let expected_camera = ControlPrefs { third_person: camera.third_person, preferred_camera_mode: camera.preferred_camera_mode, ..expected };
        assert_eq!(camera, &expected_camera, "actual camera preference consumer");
        assert_eq!(self.fade, avatar::Fade::from_prefs(&d.prefs), "actual avatar fade consumer");
        eprintln!("option runtime mouse={:?} avatar_fade={:?}", self.controls.option_prefs(), self.fade);
    }

    pub fn select_camera_mode(&mut self, mode: u8) {
        self.camera.select_mode(mode);
    }

    pub fn camera_mode(&self) -> u8 {
        self.camera.selected_mode()
    }

    /// N3 0x10020a3c excludes the client character only in first person.
    pub fn pick_hit(&self, ray: &super::hud_target::Ray, bounding_box: bool) -> Option<f32> {
        (self.camera_mode() != 0).then(|| self.avatar.pick_hit(ray, bounding_box)).flatten()
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

    /// aomac Enhanced "Widescreen FOV" changed: the next frame sends the lens again.
    pub fn set_widescreen_fov(&mut self, on: bool) {
        if on != self.widescreen_fov {
            self.widescreen_fov = on;
            self.lens_set = false;
        }
    }

    /// aomac Enhanced "Extended Scenery Distance" changed: the next frame sends the lens again.
    pub fn set_scenery_distance_mult(&mut self, mult: f32) {
        let mult = mult.clamp(1.0, 2.0);
        if mult != self.scenery_distance_mult {
            self.scenery_distance_mult = mult;
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

    /// Own Vehicle_t +0x50, read by N3Msg_ActivateMech (GC 0x10079707).
    pub fn falling_enabled(&self) -> bool { self.movement.falling_enabled() }

    /// Movement FSM mode (`FUN_100704e6`; 4 = swimming).
    pub fn mode(&self) -> u32 {
        u32::from(self.movement.fsm().mode)
    }

    /// Vehicle vtable+0x9c (`GC1006c469`), including jumping and flight.
    pub fn movement_active(&self) -> bool {
        let state = self.movement.fsm();
        state.fwd == 2 || state.strafe == 2 || state.jump == 3 || state.mode == mode::FLY
    }

    pub fn play_stance(&mut self, id: u16, idle: Option<u16>, set: Option<i32>, delay_idle: bool) {
        self.avatar.set_stance(set);
        let result = if delay_idle {
            self.avatar.set_holder_idle(idle);
            let role = if self.movement.fsm().mode == mode::CRAWL { Role::Crawl } else if self.fighting { Role::IdleCombat } else { Role::Idle };
            self.avatar.play_posture(&self.store, id, AvatarPose::still(role), avatar::STANCE_IDLE_LEAD_MS)
        } else {
            self.avatar.play_intro_action(&self.store, id)
        };
        if let Err(error) = result { eprintln!("avatar stance: {error:#}"); }
    }

    /// Plays `role` once over the movement pose (`hold`: keep the last frame, for the death clip).
    pub fn play(&mut self, role: Role, hold: bool) {
        self.avatar.set_body_priority(if hold { -3 } else { 0 });
        self.transient = Some((role, hold));
        self.cast_loop = None;
        self.cast_restart = false;
        self.clip_scale = None;
    }
    /// Plays an own use/equipment abstract id without replacing the movement pose.
    pub fn play_action(&mut self, id: u16, layer: Option<u8>, priority: i32) -> bool {
        match self.avatar.play_action(&self.store, id, layer, priority) {
            Ok(started) => {
                let body = layer.map_or_else(
                    || anim_name(id).is_some_and(|(_, layer)| layer == super::combat::anim::Layer::Body),
                    |layer| layer == 0,
                );
                if started && body {
                    self.stand();
                    self.clip_scale = None;
                    self.avatar.set_clip_scale(None);
                    self.avatar.set_cast_loop(false);
                }
                started
            }
            Err(e) => {
                eprintln!("avatar action {id}: {e:#}");
                false
            }
        }
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


    /// Retail suppresses a list key already playing, not a different key resolving to the same clip.
    pub fn swing_list(&mut self, role: Role, item_delay: Option<i32>, key: u16, slot: i32) {
        if let Err(e) = self.avatar.play_swing(&self.store, role, item_delay, key, slot) {
            eprintln!("avatar swing: {e:#}");
        }
    }

    #[cfg(test)]
    pub(super) fn swing_role(&self) -> Option<&Role> {
        self.avatar.swing_role()
    }

    /// The notes the own swing clip reached since the last call (`combat::notes` ids).
    pub fn take_notes(&mut self) -> Vec<super::combat::notes::FiredNote> {
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

    /// Death may be queued while the previous clip is already finished.
    pub fn death_animation_finished(&self) -> bool {
        self.transient.as_ref().is_some_and(|(role, hold)| *hold && self.avatar.finished_role(role))
    }

    /// `Door_t` open / close and lock changes of the zone's doors into the collision world: the door's room link
    /// (`n3Room_t::GetDoorLinkFromPos` through `Door_t::LinkDoorToRooms`) gets the open flag (`n3RoomMonitor_t::DoorOpened/Closed`, read by the
    /// camera's attractor test) and the pass flag (`Door_t::CanPass`, read by `VetoRoomTransition`).
    fn door_rooms(&mut self, zone: &mut Zone) {
        if !std::mem::replace(&mut self.doors_synced, true) {
            zone.world.resync_doors(); // doors that changed before this collision world existed
        }
        let Some(c) = self.collision.as_ref() else { return };
        let mut c = c.borrow_mut();
        for (pos, open, passable) in zone.world.take_door_rooms() {
            set_door(&mut c, pos, open, passable);
        }
    }

    /// `FUN_100585ee` [GC], run every frame by the control dynel's `Run` (`FUN_1005b016`): the feet position inside the teleportal of its zone
    /// (`n3Zone_t::IsPosInTeleportal` [N3 0x1001a86a], [`Collision::in_teleportal`]) and the gate `FUN_1005859d` open ->
    /// `n3EngineClientAnarchy_t::StartTeleportTry` [GC 0x10018f9a]: `N3Msg_MovementChanged(0x16)` (a position sync, no FSM transition), the
    /// `TeleportTrier_t` (`Zone::start_teleport_try`, which the flow turns into `TeleportStartedMessage`) and `StartTryingTeleport`'s
    /// `FUN_10059ae5(1)` (full stop while moving). The trier then runs for 30 s waiting for the server's `n3TeleportIIR_t`.
    fn teleport_try(&mut self, dt: f32, zone: &mut Zone) {
        if zone.trier.is_none() && self.teleport_gate(zone) && self.collision.as_ref().is_some_and(|c| c.borrow().in_teleportal(flip(self.movement.pos()))) {
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
        self.avatar.set_body_priority(0);
        self.transient = None;
        self.cast_loop = None;
        self.cast_restart = false;
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

    /// F12 screenshot, since the last call (retail GUI.dll `FlowControlModule_t::SlotScreenshot`; the flow has the
    /// renderer access this layer doesn't).
    pub fn take_screenshot(&mut self) -> bool {
        std::mem::take(&mut self.screenshot_requested)
    }

    fn run(&mut self, cmds: Vec<Cmd>) {
        for c in cmds {
            match c {
                Cmd::Move(a) => self.movement.action(a, self.clock),
                Cmd::MouseTurn { dx, dy } => self.movement.mouse_turn(dx, dy, self.clock),
                Cmd::EndMouseLook => self.movement.end_mouse_look(self.clock),
                Cmd::Camera(CamCmd::Pitch { dy }) => self.camera.mouse_pitch(dy, self.movement.fsm().mode),
                Cmd::ToggleWalk => self.movement.toggle_run(self.clock),
                Cmd::Camera(cc) => {
                    if let Some(turn) = self.camera.apply(&cc) {
                        self.movement.sync_camera_look(turn, self.clock);
                        self.movement.end_camera_look(self.clock);
                    } else if cc == CamCmd::EndLook {
                        self.movement.end_camera_look(self.clock);
                    }
                }
                // Controls-classified world clicks are consumed by `Play::interact_mouse`; item pick-up has no consumer yet.
                // attack / sit / special attacks belong to the combat layer (`combat::Module`, run by the flow)
                Cmd::Attack | Cmd::SwitchTarget | Cmd::Sit | Cmd::Special(_) => self.game.push(c),
                Cmd::Click(b) => self.clicks.push(b),
                Cmd::Screenshot => self.screenshot_requested = true,
                Cmd::PickupItem => {}
            }
        }
    }

    /// Keyboard and mouse-look deltas (`Frontend::game_input`).
    pub fn game_input(&mut self, ev: GameInput) {
        let cmds = match ev {
            GameInput::Key { code, pressed, .. } => self.controls.on_key(code, pressed),
            GameInput::MouseMotion { dx, dy, dt } => {
                self.movement.set_mouse_frame_dt(dt);
                self.controls.on_mouse_motion(dx, dy)
            }
            GameInput::FocusLost => {
                self.gui_press = [false; 2];
                self.controls.on_focus_lost()
            }
            // Audit item 8 (aomac Enhanced, opt-in, not retail): same wheel-notch zoom path as the mouse wheel
            // (`Controls::on_wheel`), so it keeps the 0.78-25 m range and the `MouseWheel` pref's "zoom vs nothing"
            // choice. ponytail: `delta` -> notches has no retail reference to calibrate against; tune by feel.
            GameInput::Pinch(delta) => self.controls.on_wheel(delta * PINCH_NOTCHES_PER_UNIT),
        };
        self.run(cmds);
    }

    /// Mouse buttons and the wheel (`Frontend::input`); `over_gui` = the cursor is over a GUI window. Updates
    /// `host.look` in the same event the button transitions `mouse_capture` (not deferred to the next `frame()`):
    /// a press/drag/release that arrives between frames must not lose motion to a stale cursor-capture state.
    pub fn mouse(&mut self, ev: &InputEvent, over_gui: bool, host: &mut Host) {
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
        host.look = self.controls.mouse_capture();
    }

    /// One frame: advance the movement, return the `CharDCMove` frames to send, place avatar + camera.
    pub fn frame(&mut self, dt: f32, host: &mut Host, zone: &mut Zone, text_input: bool) -> Vec<Frame> {
        self.clock += dt;
        self.controls.set_text_input(text_input);
        host.look = self.controls.mouse_capture();
        host.effect_dungeon = self.zones.as_ref().map(|zones| zones.is_dungeon());
        let s = |id| zone.stat(id);
        self.movement.set_stats(|st| {
            // GC 1006edb3 reads GetStat(RunSpeed, 2), including authored equipment/nano modifiers.
            if let Some(speed) = zone.skill_value(0x9c) { st.run_speed = speed; }
            // stat ids: RunSpeed 0x9C, Health 0x1B, Life 1, TurnSpeed 0x10B, Strength 0x10, Agility 0x11, Features 0xE0, ...
            for (field, id) in [
                (&mut st.health, 0x1B),
                (&mut st.max_health, 1),
                (&mut st.turn_speed, 0x10B),
                (&mut st.strength, 0x10),
                (&mut st.agility, 0x11),
                (&mut st.features, 0xE0),
                (&mut st.mech_data, 0x296),
                (&mut st.gm_level, 0xD7),
                (&mut st.wait_state, 0x1AE),
                (&mut st.flags, 0),
            ] {
                if let Some(v) = s(id) {
                    *field = v;
                }
            }
            if let Some(scale) = s(0x168) { st.monster_scale = scale + zone.monster_scale_bonus; }
        });
        let was_jumping = self.movement.fsm().jump == 3;
        self.apply_own_events(zone);
        if let Some(update) = zone.own_update.as_ref() {
            match self.avatar.set_body_stats(&self.store, update.mode, zone.stat(0x2f).map_or(update.fatness, |v| v as u8), zone.stat(0x378).unwrap_or(0)) {
                Ok(changed) => {
                    self.model_sent &= !changed;
                    if changed { zone.world.effect_source_model_changed((super::dynels::CHAR_KIND as u32, self.char_id)); }
                }
                Err(e) => eprintln!("avatar body: {e:#}"),
            }
        }
        // `FUN_10070fee`: the target branch re-runs the Features / district gate every frame and reads the target's `GetRelPos`
        if let Some(t) = self.movement.chase_target() {
            if self.follow_gated() {
                self.movement.drop_chase();
            } else {
                let p = if t == self.char_id as i32 { Some(self.movement.pos()) } else { zone.world.character_position(t) };
                self.movement.set_chase_pos(p);
            }
        }
        // `CharFight_t::Update` [GC 0x1007bb81]: the own character auto-faces its live fight target while standing
        // (`Movement::set_face_target`, docs/zone/movement.md).
        self.movement.set_face_target(fight_target_position(zone, self.char_id as i32, self.movement.pos()));
        for (id, v) in self.movement.take_stat_writes() {
            zone.stats.insert(id, v);
            if id == 0x1ae { zone.world.set_wait_state(self.char_id as i32, v); }
        }
        self.sync_travel_appearance(zone);
        if let Some(z) = self.zones.as_ref().and_then(|l| l.zone_at(flip(self.movement.pos()))) {
            self.movement.zone_instance(z as u32);
        }
        self.door_rooms(zone);
        self.teleport_try(dt, zone);
        let out: Vec<Frame> = {
            let collision = self.collision.as_ref().map(|c| c.borrow());
            let world = Ground(collision.as_deref());
            self.movement.update(dt, &world).iter().map(|m| n3_frame(0, self.char_id, char_dc_move(self.char_id as i32, m))).collect()
        };
        for (id, v) in self.movement.take_stat_writes() {
            zone.stats.insert(id, v);
            if id == 0x1ae { zone.world.set_wait_state(self.char_id as i32, v); }
        }
        let (pos, yaw) = (self.movement.pos(), self.movement.yaw());
        zone.nano_motion = Some(super::hud_nano::NanoMotion {
            pos, radius: self.avatar.body_radius(), mode: self.mode(),
            moving: self.movement.fsm().is_moving(), falling: !self.movement.grounded(),
            input_locked: self.movement.input_locked(),
        });
        if let Some(d) = zone.dynels.get_mut(&(self.char_id as i32)) {
            d.pos = pos;
            d.yaw = Some(yaw);
        }
        zone.fight_level = Some(self.fight_level(self.movement.pos()));
        zone.crowd_state = Some(self.movement.crowd_state());
        zone.character_fight_levels.clear();
        for (&id, d) in &zone.dynels {
            let pos = if id == self.char_id as i32 { self.movement.pos() } else { zone.world.character_position(id).unwrap_or(d.pos) };
            zone.character_fight_levels.insert(id, self.fight_level(pos));
        }
        let district = self.fight.as_ref().and_then(|f| f.lc_district(self.movement.pos()));
        zone.own_lca = district.map(|(_, (min, max))| min != 0 && max != 0);
        if zone.lc_district.as_ref().map(|(name, range)| (name.as_str(), *range)) != district {
            zone.lc_district = district.map(|(name, range)| (name.to_owned(), range));
        }

        let role = self.movement.role();
        let moving = self.movement.speed() > 0.01 && self.movement.grounded();
        self.avatar.set_stance(zone.world.wielded_set(self.char_id as i32));
        self.avatar.set_holder_idle(if self.fighting && !moving && matches!(role, Role::Idle | Role::Crawl) {
            zone.world.fight_idle_animation(self.char_id as i32)
        } else { None });
        // the new pose's enter / stop clip plays once over its idle clip (`Pose::transition_anim`, GC 0x1006d330)
        let now = Pose::from_role(&role);
        if let Some(from) = self.pose.replace(now).filter(|&b| b != now && self.cast_loop.is_none()) {
            if let Some(id) = Pose::transition_anim(from, now) {
                self.transient = None;
                self.cast_loop = None;
                self.avatar.set_body_priority(0);
                self.avatar.set_clip_scale(None);
                self.avatar.set_cast_loop(false);
                let idle = if self.fighting && role == Role::Idle { Role::IdleCombat } else { role.clone() };
                zone.world.apply_crawl_idle(self.char_id as i32, id);
                if self.fighting && matches!(id, 0x68 | 0x69) {
                    self.avatar.set_holder_idle(zone.world.fight_idle_animation(self.char_id as i32));
                }
                if let Err(e) = self.avatar.play_posture(&self.store, id, AvatarPose::still(idle), avatar::POSTURE_IDLE_LEAD_MS) {
                    eprintln!("avatar posture: {e:#}");
                }
            }
        }
        // the airborne->grounded edge plays the jump-landing clip once over the resuming movement pose (`jump-land-*`);
        // the clip's own `land` note then plays the ground-material footstep thud (`FUN_10045069`).
        let grounded = self.movement.grounded();
        if was_jumping && self.movement.fsm().jump != 3 && grounded && self.cast_loop.is_none() {
            let fsm = self.movement.fsm();
            if let Some(id) = super::combat::anim::landing_anim(u32::from(fsm.mode), fsm.fwd == 2, fsm.strafe == 2) {
                if let Err(error) = self.avatar.play_intro_action(&self.store, id) { eprintln!("avatar landing: {error:#}"); }
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
        self.avatar.set_clip_scale(self.transient.as_ref().and(self.clip_scale));
        self.avatar.set_cast_loop(self.cast_loop == Some(true));
        self.avatar.set_airborne(!grounded);
        if std::mem::take(&mut self.cast_restart) { self.avatar.restart_cast_clip(); }
        if let Err(e) = self.avatar.set_pose(&self.store, pose) {
            eprintln!("avatar pose: {e:#}");
        }
        self.avatar.set_transform(scene_pos(pos), scene_yaw(yaw));
        self.avatar.update(dt);

        let head_local = self.avatar.head_local();
        if let Some(h) = head_local {
            self.camera.set_head(h);
        }
        let collision = self.collision.as_ref().map(|c| c.borrow());
        let col = collision.as_deref();
        let clear = |a: [f32; 3], b: [f32; 3]| col.is_none_or(|c| segment_clear(c, a, b));
        let closed = |a: [f32; 3], b: [f32; 3]| col.is_some_and(|c| door_closed(c, a, b));
        let ground = |p: [f32; 3]| col.and_then(|c| c.ground(p));
        host.camera = self.camera.update_with(scene_pos(pos), yaw, dt, &Sight { clear: &clear, door_closed: &closed, ground: &ground });
        if !self.lens_set {
            // FOV 90 degrees horizontal, near 0.2 (`VisualCamera_t`, docs/zone/camera.md); far = `ViewDistance` * 1000 m
            // (`FUN_1001fc91`) * the aomac Enhanced "Extended Scenery Distance" multiplier (audit item 15, not retail,
            // 1.0x default); widescreen_fov is the aomac Enhanced FOV option (audit item 16, not retail).
            let mut lens = camera::lens(host.lens.unwrap_or(zone.world.lens), self.widescreen_fov);
            lens.far = Some(camera::far_plane(self.view_distance) * self.scenery_distance_mult);
            host.lens = Some(lens);
            self.lens_set = true;
        }
        if !self.model_sent {
            host.actor_models.push((avatar::MODEL_KEY, self.avatar.model_arc()));
            self.model_sent = true;
        }
        // `VisualCATMesh_t::RunFunction` -> `RefreshAlpha`: opacity from the camera <-> head attractor distance (own character only)
        let head_position = glam::Vec3::from(scene_pos(pos)) + glam::Quat::from_rotation_y(scene_yaw(yaw)) * head_local.unwrap_or(glam::Vec3::ZERO);
        let head_to_camera = head_position - host.camera.pos;
        let alpha = self.fader.step(&self.fade, head_to_camera.length_squared());
        self.avatar.set_animation_refresh(host.smooth_animations, host.camera.pos, host.previous_visible_actors.contains(&self.char_id));
        zone.world.set_own_pick(None);
        if self.camera.show_avatar() {
            // N3 Run 0x100196bd -> DisableVisibility 0x10073693 detaches the CAT frame,
            // so its mounted lights no longer reach Randy ProcessAttractorChilds 0x100577d3.
            host.dynamic_lights.extend(self.avatar.lights());
            let mut frame = if zone.world.needs_effect_source_mesh((super::dynels::CHAR_KIND as u32, self.char_id)) {
                self.avatar.source_frame()
            } else {
                self.avatar.frame()
            };
            frame.alpha = alpha;
            zone.world.set_own_pick(self.avatar.pick_bounds()
                .map(|bounds| super::hud_pick::PickBody { id: self.char_id as i32, bounds, pos: scene_pos(pos), yaw: scene_yaw(yaw), scale: self.avatar.body_scale() }));
            host.actors.push(frame);
        }
        out
    }

    /// Native CAT-bound effects use the same posed geometry as the actual avatar submission.
    pub fn prepare_effect_source(&self, world: &mut super::dynels::Dynels, host: &Host, breed: Option<i32>, in_play:bool) {
        let identity = (super::dynels::CHAR_KIND as u32, self.char_id);
        world.set_effect_source_torso_factor(identity, self.avatar.effect_torso_factor());
        if let Some(breed) = breed {
            world.set_effect_source_runtime(identity, self.avatar.body_scale(), breed, Some(self.movement.speed()), self.movement.vehicle_direction(), self.camera.show_avatar());
        }
        world.set_effect_source_font_locator(identity,in_play,self.fader.opacity());
        world.set_effect_source_head_height(identity, self.avatar.head_local().map(|head| head.y));
        world.set_effect_source_liquid(identity, self.movement.effect_liquid().map(|(depth, flags, direction)| (depth, flags, direction.into())));
        if !world.needs_effect_source_mesh(identity) { return; }
        // First-person hides the actor, not the CAT pose needed by its native effects.
        let mut source = self.avatar.source_frame();
        if let Some(actor) = host.actors.iter().find(|actor| actor.id == self.char_id && actor.model == avatar::MODEL_KEY) {
            source.alpha = actor.alpha;
        }
        world.prepare_effect_source_mesh(identity, self.avatar.model(), &source);
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

    /// `MovementChanged(0x1e)` of `N3Msg_StartCamping`: drain immediately, before the caller queues stop-attack / camping.
    pub fn sit_ground(&mut self) -> Vec<ao_net::n3::outgoing::CharMove> {
        self.movement.action(mv::SWITCH_SIT_GROUND, self.clock);
        self.movement.take_outgoing()
    }

    /// GC 10059e6a MonsterData hook and 1005bea6 body-scale refresh.
    fn sync_travel_appearance(&mut self, zone: &mut Zone) {
        if zone.stat(0x296).unwrap_or(0) == 0 {
            let shape = zone.stat(0x167).or_else(|| zone.own_update.as_ref().map(|u| u.monster_data)).unwrap_or(0);
            match self.avatar.set_monster_data(&self.store, shape) {
                Ok(true) => {
                    self.model_sent = false;
                    self.movement.set_body_radius(self.avatar.body_radius());
                    zone.world.effect_source_model_changed((super::dynels::CHAR_KIND as u32, self.char_id));
                }
                Ok(false) => {}
                Err(e) => eprintln!("avatar morph {shape}: {e:#}"),
            }
        }
        let scale = zone.stat(0x168).or_else(|| zone.own_update.as_ref().map(|u| i32::from(u.monster_scale))).unwrap_or(100) + zone.monster_scale_bonus;
        self.avatar.set_body_scale(if scale == 0 { 1.0 } else { scale as f32 / 100.0 });
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
                OwnEvent::Resurrected => {
                    // GC 0x1003ea0d clears held CAT animation, then refreshes the current vehicle mode.
                    self.stand();
                    self.movement.resurrect();
                    if zone.stat(0x36).is_some_and(|level| level >= 15) {
                        zone.stats.insert(0xf7, 5000);
                    }
                }
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
                    Ok(changed) => {
                        self.model_sent &= !changed;
                        if changed {
                            zone.world.effect_source_model_changed((super::dynels::CHAR_KIND as u32, self.char_id));
                        }
                    }
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

    pub fn control_action_flags(&self) -> (u32, bool) {
        self.movement.control_action_flags()
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


fn fight_target_position(zone: &Zone, own: i32, own_pos: [f32; 3]) -> Option<[f32; 3]> {
    let target = *zone.fight_target.get(&own)?;
    if target == own { Some(own_pos) } else { zone.world.character_position(target) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Authored offline replay: installed RDB Wear spells, acknowledged inventory move, real own CAT rebuild and undo.
    #[test]
    fn rdb_yalmaha_equipment_ack_enters_flight_morph_and_undoes() {
        use ao_net::{msg::Identity, n3::{inventory as inv, spells, world::{AcgItem, InventoryEntry}}, Writer};
        use super::super::hud_stats::equipment::Equipment;
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut zone = Zone::new(7);
        zone.own_update = Some(Box::new(avatar::tests::own_update()));
        zone.stats.extend([(0x167, 0), (0x168, 100), (0x296, 0), (0xd7, 0)]);
        let mut player = Player::new(&dir, &mut zone, 0).expect("real own character");
        let original_meshes = player.avatar.model().meshes.len();
        let store = RecordStore::open(&dir).unwrap();
        let template = store.get(1000020, 96088).unwrap().unwrap();
        let level = ao_formats::dynel_visual::parse_item_template(&template).unwrap().stat(54).unwrap();
        let item = AcgItem { low_id: 96088, high_id: 117322, level };
        zone.inventory.insert(0x40, InventoryEntry { slot: 0x40, a: 0, b: 1, id: inv::item_identity(0x40), item });
        let mut equipment = Equipment::new(&dir);
        equipment.refresh_travel(&mut zone);
        assert!(zone.own_events.is_empty(), "a bag item does not run Wear");
        let ack = |source, destination| {
            let mut w = Writer::default();
            w.u32(inv::CONTAINER_ADD_ITEM);
            Identity { kind: 0xc350, instance: 7 }.write(&mut w);
            w.u8(0);
            inv::item_identity(source).write(&mut w);
            Identity { kind: 0xc350, instance: 7 }.write(&mut w);
            w.i32(destination);
            n3_frame(1, 0, w.0)
        };
        zone.on_frame(&ack(0x40, 1));
        equipment.refresh_travel(&mut zone);
        assert_eq!(equipment.effects().iter().map(|s| s.function).collect::<Vec<_>>(),
            [0xcf35, 0xcf4c, 0xcf76, 0xcf92, 0xcf44], "functions come from both real item templates");
        let authored = equipment.effects().to_vec();
        assert_eq!(authored[0].stat(spells::stat::VALUE), 750);
        assert_eq!((zone.stat(359), zone.stat(360)), (Some(99470), Some(20)));
        player.apply_own_events(&mut zone);
        player.sync_travel_appearance(&mut zone);
        assert_eq!(player.movement.stats().gm_level, 0);
        assert_eq!(player.movement.fsm().mode, mode::FLY);
        assert!(!player.movement.falling_enabled());
        assert_eq!(player.avatar.body_scale(), 0.2);
        assert_eq!(player.avatar.monster_visual(), (99470, 42370, 99429));
        assert!(zone.active_spells.is_empty(), "wear modifiers are not duplicated as nano buffs");
        equipment.refresh_travel(&mut zone);
        assert!(zone.own_events.is_empty(), "unchanged equipment never reapplies Features counters");
        zone.on_frame(&ack(1, 0x40));
        equipment.refresh_travel(&mut zone);
        player.apply_own_events(&mut zone);
        player.sync_travel_appearance(&mut zone);
        assert_ne!(player.movement.fsm().mode, mode::FLY);
        assert!(player.movement.falling_enabled());
        assert_eq!((zone.stat(359), zone.stat(360)), (Some(0), Some(100)));
        assert_eq!(player.avatar.monster_visual().0, 0);
        assert_eq!(player.avatar.body_scale(), 1.0);
        assert_eq!(player.avatar.model().meshes.len(), original_meshes);
        assert!(zone.temporary_stats.is_empty());
    }

    /// Same hand-built ResurrectIIR as `own_server_moves_are_queued`, through the player consumer.
    #[test]
    fn own_server_moves_queued_resurrection_refreshes_without_stopping() {
        use ao_net::msg::Identity;
        use ao_net::n3::{outgoing::n3_frame, server_move as sm};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut zone = Zone::new(7);
        zone.own_update = Some(Box::new(avatar::tests::own_update()));
        let mut player = Player::new(&dir, &mut zone, 0).expect("captured own avatar");
        zone.stats.insert(0x36, 15);
        let frame = n3_frame(1, 0, sm::resurrect(Identity { kind: 0xC350, instance: 7 }, &sm::Resurrect { health: 77, nano: 33 }));
        let mut combat = super::super::combat::state::Combat::new(Box::new(super::super::combat::log::fake::Fixed::new()));
        combat.add_test_char(7, "Own", false, 100);
        let death = n3_frame(1, 0, ao_net::n3::action::character_action(7, &ao_net::n3::action::simple(99, Identity::default(), Identity::default())));
        combat.on_frame(&death, 7);
        assert!(combat.char(7).unwrap().dead);
        combat.on_frame(&frame, 7);
        let restored = combat.char(7).unwrap();
        assert!(!restored.dead);
        assert_eq!((restored.health(), restored.stat(0xd6), restored.stat(0xf7)), (77, 33, 0), "below level 15 there is no shock");
        for mode in [mode::RUN, mode::SIT_GROUND, mode::FLY] {
            player.movement = Movement::new([0.0; 3], 0.0, 0);
            let action = match mode {
                mode::RUN => mv::SWITCH_RUN,
                mode::SIT_GROUND => mv::SWITCH_SIT_GROUND,
                _ => mv::SWITCH_FLY,
            };
            player.movement.transition(action);
            if mode == mode::RUN { player.movement.action(mv::FORWARD_START, 0.0); }
            player.movement.take_outgoing();
            player.movement.take_stat_writes();
            let before = *player.movement.fsm();
            zone.on_frame(&frame);
            player.apply_own_events(&mut zone);
            assert_eq!(*player.movement.fsm(), before, "resurrection retains the movement FSM");
            assert_eq!((zone.stat(0x1b), zone.stat(0xd6), zone.stat(0xf7)), (Some(77), Some(33), Some(5000)));
            assert!(player.movement.take_outgoing().is_empty(), "no movement message or FullStop");
            let writes = player.movement.take_stat_writes();
            if mode == mode::SIT_GROUND { assert!(writes.contains(&(0x1a9, 25)) && writes.contains(&(0x1ae, 2))); }
            if mode == mode::FLY { assert!(!player.movement.falling_enabled()); }
        }
    }

    #[test]
    fn own_deformer_submits_cpu_skin_only_while_needed() {
        use super::super::own_nanos::VisualEvent;
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut zone = Zone::new(7);
        zone.world.start(dir.clone(), 7);
        zone.own_update = Some(Box::new(avatar::tests::own_update()));
        let mut player = Player::new(&dir, &mut zone, 0).expect("captured own avatar");
        player.collision = None;
        player.zones = None;
        player.camera.select_mode(3);
        let mut host = Host::headless();
        player.frame(0.0, &mut host, &mut zone, false);
        let actor = host.actors.iter().find(|a| a.id == 7).unwrap();
        assert!(actor.skin.is_none() && actor.skin_pose.is_some(), "ordinary own submission is palette-only");
        zone.world.apply_buff_visuals(7, [VisualEvent::Add { nano: 1, effect: 31201, duration_cs: 1000 }]);
        zone.world.buff_visual_frame(0.0, |id| player.effect_anchor(id), |_, _| None);
        assert!(zone.world.needs_effect_source_mesh((super::super::dynels::CHAR_KIND as u32, 7)));
        for _ in 0..4 {
            host.actors.clear();
            player.frame(0.0, &mut host, &mut zone, false);
            player.prepare_effect_source(&mut zone.world, &host, None, true);
            let eye = host.camera.pos.to_array();
            zone.world.update_with_collision(0.025, eye, [0.0, 0.0, -1.0], &mut host, None,
                |id| player.effect_anchor(id).map(|m| glam::Mat4::from_cols_array_2d(&m)));
        }
        let actor = host.actors.iter().find(|a| a.id == 7).unwrap();
        let skin = actor.skin.as_ref().expect("renderer must receive the CPU body that Deformer edits");
        assert!(actor.skin_pose.is_some(), "CPU skin overrides the retained render palette");
        let original = player.avatar.source_frame().skin.unwrap();
        assert_eq!(skin.len(), original.len());
        assert!(!skin.is_empty());
        assert!(skin.iter().zip(original.iter()).any(|(a, b)| a.pos != b.pos), "Deformer must edit the submitted own body");
        assert!(player.avatar.frame().skin.is_none(), "ordinary frames stay palette-only even after source geometry is cached");
    }

    #[test]
    fn own_death_completion_reads_the_actual_held_clip() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut zone = Zone::new(7);
        zone.own_update = Some(Box::new(avatar::tests::own_update()));
        let mut player = Player::new(&dir, &mut zone, 0).expect("captured own avatar");
        player.collision = None;
        player.zones = None;
        player.avatar.set_pose(&player.store, AvatarPose::still(Role::Clip("ground-stop".into()))).unwrap();
        player.avatar.update(100.0);
        assert!(player.avatar.finished(), "the preceding clip is already complete");
        player.play(Role::Clip("die-shot".into()), true);
        assert!(!player.death_animation_finished(), "queued death must not inherit previous clip completion");
        let mut host = Host::headless();
        player.frame(0.0, &mut host, &mut zone, false);
        assert!(!player.death_animation_finished());
        player.frame(100.0, &mut host, &mut zone, false);
        assert!(player.holding() && player.death_animation_finished());
        player.stand();
        assert!(!player.death_animation_finished(), "resurrection ends the held death");
    }

    /// Offline, real-CAT Player::frame regression, including the movement FSM and transient dispatch.
    /// AOMAC_POSTURE_TRACE writes both CSVs; AOMAC_POSTURE_PNG also renders every sampled frame.
    #[test]
    fn own_sit_stand_player_frame_pose_continuity() {
        use std::fmt::Write as _;
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let output = std::env::var_os("AOMAC_POSTURE_TRACE").map(std::path::PathBuf::from);
        if let Some(output) = &output { std::fs::create_dir_all(output).unwrap(); }
        let pngs = std::env::var_os("AOMAC_POSTURE_PNG").is_some();
        let store = RecordStore::open(&dir).unwrap();
        let scene = if pngs { Some(ao_formats::playfield::load_playfield(&store, &dir, 4604).unwrap()) } else { None };
        let distance = |a: &[ao_scene::Vertex], b: &[ao_scene::Vertex]| {
            assert_eq!(a.len(), b.len());
            a.iter().zip(b).map(|(a, b)| glam::Vec3::from(a.pos).distance(glam::Vec3::from(b.pos))).fold(0.0_f32, f32::max)
        };
        let mut baseline_error = 0.0_f32;
        for before in [true, false] {
            let mut update = avatar::tests::own_update();
            update.pos = [0.0; 3];
            let mut zone = Zone::new(7);
            zone.own_update = Some(Box::new(update));
            let mut player = Player::new(&dir, &mut zone, 0).expect("captured own avatar");
            player.collision = None;
            player.zones = None;
            player.movement.teleport([0.0; 3], 0.0);
            if player.movement.fsm().mode != mode::RUN { assert!(player.movement.transition(mv::SWITCH_RUN)); }
            let mut host = Host::headless();
            let version = if before { "before" } else { "after" };
            for (label, action, mode, transition, idle) in [
                ("sit", mv::SWITCH_SIT_GROUND, mode::SIT_GROUND, "ground-start", Role::SitGround),
                ("stand", mv::LEAVE_SIT, mode::RUN, "ground-stop", Role::Idle),
            ] {
                let id = if label == "sit" { 0xd5 } else { 0xd6 };
                let duration = avatar::tests::posture_duration(&mut player.avatar, &player.store, id, before);
                let mut csv = String::from("frame,fsm,transient,clip,clip_ms,duration,prev_clip,prev_ms,prev_once,fade_elapsed,fade_ms,posture_clip,posture_ms,posture_duration,posture_blend,idle_delay,max_vertex_step,reference_step,pose_error\n");
                let mut previous = None;
                let mut previous_reference = None;
                let mut previous_clock = None;
                let mut started = false;
                let mut ended = false;
                let mut after = 0;
                let mut max_step = (0.0_f32, -60);
                let mut max_error = (0.0_f32, -60);
                let mut queue_reference = None;
                let mut authored_bound = 0.0_f32;
                for frame_no in -60..600 {
                    if frame_no == 0 {
                        assert!(player.movement.transition(action));
                        assert_eq!(player.movement.fsm().mode, mode);
                        if !before {
                            queue_reference = Some(avatar::tests::posture_reference(&mut player.avatar, &player.store, id, &idle));
                        }
                        if before {
                            // Reproduce the original scheduling, not merely its sampler: suppress
                            // queue dispatch and play a transient until the whole clip finishes.
                            player.pose = Some(Pose::from_role(&idle));
                            player.play(Role::Clip(transition.into()), false);
                        }
                    }
                    let had_transient = player.transient.is_some();
                    host.actors.clear();
                    host.dynamic_lights.clear();
                    player.frame(1.0 / 60.0, &mut host, &mut zone, false);
                    if before && had_transient && player.transient.is_none() {
                        avatar::tests::posture_baseline_idle(&mut player.avatar);
                    }
                    let clock = avatar::tests::posture_clock(&player.avatar);
                    if let Some((id, ms)) = previous_clock {
                        if id == clock.0 { assert!(clock.1 >= ms, "{label}: clip restarted at {frame_no}"); }
                    }
                    previous_clock = Some(clock);
                    if frame_no >= 0 {
                        assert_eq!(player.movement.fsm().mode, mode, "{label}: FSM flicker at {frame_no}");
                        let active = if before {
                            match &player.transient {
                                Some((Role::Clip(name), false)) if name == transition => true,
                                None => false,
                                other => panic!("{label}: unexpected transient {other:?} at {frame_no}"),
                            }
                        } else {
                            assert!(player.transient.is_none(), "{label}: queue fell back to transient");
                            player.avatar.posture_active()
                        };
                        let elapsed = (frame_no + i32::from(!before)) as f32 * (1000.0 / 60.0);
                        if (elapsed - duration).abs() > 0.02 {
                            assert_eq!(active, elapsed < duration, "{label}: finite enter lifetime at {frame_no}");
                        }
                        if active {
                            assert!(!ended, "{label}: restarted transition at {frame_no}");
                            started = true;
                        } else {
                            assert!(started, "{label}: skipped enter/exit clip");
                            ended = true;
                            assert_eq!(player.movement.role(), idle);
                            after += 1;
                        }
                    }
                    let (actor, reference, details) = avatar::tests::posture_sample(&player.avatar, before, frame_no, queue_reference.as_ref());
                    let skin = actor.skin.as_ref().expect("real CAT skin").clone();
                    let step = previous.as_ref().map_or(0.0, |old: &Vec<ao_scene::Vertex>| distance(old, &skin));
                    let reference_step = previous_reference.as_ref().map_or(0.0, |old: &Vec<ao_scene::Vertex>| distance(old, &reference));
                    authored_bound = authored_bound.max(reference_step);
                    let error = distance(&skin, &reference);
                    if step > max_step.0 { max_step = (step, frame_no); }
                    if error > max_error.0 { max_error = (error, frame_no); }
                    if before {
                        baseline_error = baseline_error.max(error);
                    } else {
                        assert!(error < 1e-4, "{label} frame {frame_no}: queue pose diverged ({error}); {details}");
                        assert!(step <= reference_step + 2e-4,
                            "{label} frame {frame_no}: anomalous single-frame displacement {step}, authored/blended reference {reference_step}");
                    }
                    let transient = match &player.transient { Some((Role::Clip(name), _)) => name.as_str(), _ => "-" };
                    writeln!(csv, "{frame_no},{},{transient},{details},{step:.7},{reference_step:.7},{error:.7}",
                        player.movement.fsm().mode).unwrap();
                    if let (Some(output), Some(scene)) = (&output, &scene) {
                        let eye = [3.0, 1.4, 3.0];
                        let png = output.join(format!("own-{label}-{version}-{:04}.png", frame_no + 60));
                        ao_render::render_to_png_actors(scene, &[(avatar::MODEL_KEY, player.avatar.model().clone())],
                            vec![actor], eye, [0.0, 0.9, 0.0], 640, 480, &png, 0.0).unwrap();
                    }
                    previous = Some(skin.as_ref().clone());
                    previous_reference = Some(reference);
                    if ended && after >= 60 { break; }
                }
                assert!(started && ended && after == 60, "{label}: transition did not finish");
                if !before { assert!(max_step.0 <= authored_bound + 2e-4, "{label}: frame motion exceeds the CAT/retail authored envelope {authored_bound}"); }
                eprintln!("own-{label}-{version}: interval={duration:.3}ms max_vertex_step={:.7}@frame{} authored_bound={authored_bound:.7} max_pose_error={:.7}@frame{}; frames start -60, include full enter and 60 after",
                    max_step.0, max_step.1, max_error.0, max_error.1);
                if let Some(output) = &output {
                    std::fs::write(output.join(format!("own-{label}-{version}.csv")), csv).unwrap();
                }
            }
        }
        assert!(baseline_error > 0.01, "real CAT baseline must reproduce the wrapped-outgoing pose defect, got {baseline_error}");
    }

    #[test]
    fn fight_facing_uses_live_pose_and_current_fight_not_selection() {
        let own = 25988;
        let mut zone = Zone::new(own as u32);
        for line in include_str!("../../../../docs/captures/zone_ithaca.rec").lines() {
            let mut fields = line.split(' ');
            let (Some(_), Some("<"), Some(hex)) = (fields.next(), fields.next(), fields.next()) else { continue };
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
            if let Some((frame, _)) = Frame::decode_with(&bytes, false).unwrap() { zone.on_frame(&frame); }
            if zone.world.chars.keys().filter(|id| **id != own).count() >= 2 { break; }
        }
        let ids: Vec<_> = zone.world.chars.keys().copied().filter(|id| *id != own).take(2).collect();
        assert_eq!(ids.len(), 2);
        let (target, replacement) = (ids[0], ids[1]);
        let snapshot = zone.dynels[&target].pos;
        let live = [snapshot[0] + 20.0, snapshot[1], snapshot[2] + 30.0];
        let mut mover = ao_net::n3::motion::Mover::new(live, 0.0);
        mover.set_features(4);
        assert!(mover.on_move_type(1));
        zone.world.chars.get_mut(&target).unwrap().mover = mover;
        zone.world.advance(0.0);
        zone.fight_target.insert(own, target);
        zone.set_target(Some(ao_net::msg::Identity { kind: 50000, instance: replacement }));
        let own_pos = [10.0, 0.0, 20.0];
        assert_eq!(fight_target_position(&zone, own, own_pos), Some(live), "no render-space Z flip or snapshot fallback");
        zone.world.advance(0.5);
        let moved = zone.world.character_position(target).unwrap();
        assert_ne!(moved, live);
        assert_eq!(zone.dynels[&target].pos, snapshot, "wire snapshot stays stale between packets");
        assert_eq!(fight_target_position(&zone, own, own_pos), Some(moved));
        zone.fight_target.insert(own, replacement);
        assert_eq!(fight_target_position(&zone, own, own_pos), zone.world.character_position(replacement));
        zone.fight_target.remove(&own);
        assert_eq!(fight_target_position(&zone, own, own_pos), None);
        zone.fight_target.insert(own, target);
        zone.world.chars.remove(&target);
        assert_eq!(fight_target_position(&zone, own, own_pos), None, "vanished target must not revive its stale snapshot");
        zone.fight_target.insert(own, own);
        assert_eq!(fight_target_position(&zone, own, own_pos), Some(own_pos));
    }

    fn light_bar_appearance(on: bool) -> ao_net::n3::world::AppearanceUpdate {
        let head = avatar::tests::own_update().head_mesh.expect("captured own head");
        let mut attractors = vec![ao_net::n3::world::Attractor { a: 0, b: head, c: 0, d: 4 }];
        if on {
            attractors.push(ao_net::n3::world::Attractor { a: 5, b: 26163, c: 0, d: 0 });
        }
        ao_net::n3::world::AppearanceUpdate {
            cloth: vec![],
            attractors,
            visual_flags: 31, extra: 0,
        }
    }

    fn light_bar_player(dir: &Path, scale: i16) -> (Player, Zone) {
        let mut update = avatar::tests::own_update();
        update.pos = [0.0; 3];
        update.monster_scale = scale;
        let mut zone = Zone::new(7);
        zone.own_update = Some(Box::new(update));
        let mut player = Player::new(dir, &mut zone, 0).expect("own captured avatar");
        player.collision = None;
        player.zones = None;
        player.movement.teleport([0.0; 3], 0.0);
        zone.own_events.push(OwnEvent::Appearance(light_bar_appearance(true)));
        (player, zone)
    }

    #[test]
    fn own_light_bar_tracks_visible_player_frame_and_unequips() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let (mut player, mut zone) = light_bar_player(&dir, 175);
        let mut host = Host::headless();
        player.fade = avatar::Fade { on: true, start: 100.0, end: 99.0, end_alpha: 1.0 };
        player.camera.select_mode(3);
        player.frame(0.0, &mut host, &mut zone, false);
        assert_eq!(host.actors[0].alpha, 0.0, "body fade must not fade the worn light");
        assert_eq!(host.dynamic_lights.len(), 1);
        let authored = ao_formats::mesh::decode_mesh_lights(&player.store, 1010001, 26163, [0.0; 3]).unwrap()[0];
        let actor = player.avatar.frame();
        let mount = glam::Mat4::from_cols_array_2d(actor.parts.last().expect("back mount follows body"));
        let local = mount.transform_point3(glam::Vec3::from(authored.pos));
        let expected = glam::Mat4::from_cols_array_2d(&actor.transform).transform_point3(local);
        assert!((glam::Vec3::from(host.dynamic_lights[0].pos) - expected).length() < 1e-5);
        assert_eq!(host.dynamic_lights[0].color, authored.color);
        assert_eq!(host.dynamic_lights[0].range, authored.range);
        assert_eq!(host.dynamic_lights[0].atten, authored.atten);
        assert_eq!(player.avatar.body_scale(), 1.75);
        player.camera.set_prefs(&ControlPrefs { show_my_character: false, ..Default::default() });
        player.camera.select_mode(0);
        assert!(!player.camera.show_avatar());
        for (pos, yaw) in [([3.0, 2.0, -4.0], 0.7), ([-2.0, 1.0, 5.0], -1.2)] {
            zone.own_events.push(OwnEvent::Place { pos, yaw, full_reset: true });
            host.actors.clear();
            host.dynamic_lights.clear();
            player.frame(0.0, &mut host, &mut zone, false);
            assert!(host.actors.is_empty(), "first-person must really hide the body");
            assert!(host.dynamic_lights.is_empty(), "disabled CAT visibility must also stop mounted light processing");
            player.camera.set_prefs(&ControlPrefs { show_my_character: true, ..Default::default() });
            player.frame(0.0, &mut host, &mut zone, false);
            assert_eq!(host.actors.len(), 1, "ShowMyCharacter must restore the first-person body");
            assert_eq!(host.dynamic_lights.len(), 1, "ShowMyCharacter must restore its mounted light");
            let world = glam::Mat4::from_translation(glam::Vec3::from(scene_pos(pos)))
                * glam::Mat4::from_rotation_y(scene_yaw(yaw)) * glam::Mat4::from_scale(glam::Vec3::splat(1.75));
            assert!((glam::Vec3::from(host.dynamic_lights[0].pos) - world.transform_point3(local)).length() < 1e-4);
            player.camera.set_prefs(&ControlPrefs { show_my_character: false, ..Default::default() });
        }
        player.camera.select_mode(3);
        zone.own_events.push(OwnEvent::Appearance(light_bar_appearance(false)));
        host.dynamic_lights.clear();
        player.frame(0.0, &mut host, &mut zone, false);
        assert!(host.dynamic_lights.is_empty(), "unequipped light must disappear on the same frame");
    }

    /// Opt-in, offline own-avatar evidence: the real mesh 26163 on a synthetic back mount.
    #[test]
    fn own_light_bar_lights_the_ground_at_night() {
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(std::path::PathBuf::from) else { return };
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        struct Shot { gui: ao_gui::Gui, player: Player, zone: Zone, scene: Option<ao_scene::Scene> }
        impl ao_render::Frontend for Shot {
            fn gui(&self) -> &ao_gui::Gui { &self.gui }
            fn input(&mut self, _: InputEvent, _: &mut Host) {}
            fn frame(&mut self, dt: f32, size: (u32, u32), host: &mut Host) -> ao_gui::DrawList {
                if let Some(scene) = self.scene.take() { host.set_scene(scene); }
                self.player.frame(0.0, host, &mut self.zone, false);
                let eye = if size.0 > 2000 { ao_render::Vec3::new(8.0, 4.0, 12.0) } else { ao_render::Vec3::new(3.0, 2.0, 5.0) };
                host.camera = ao_render::Camera::look_at(eye, ao_render::Vec3::new(0.0, 1.0, 0.0));
                self.gui.frame(dt)
            }
        }
        std::fs::create_dir_all(&out).unwrap();
        for (view, size) in [("close", (640, 480)), ("wide", (5120, 1440))] {
            let (player, zone) = light_bar_player(&dir, 100);
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            // Vertex-lit terrain needs subdivisions to show the authored six-metre light pool.
            for z in 0..=40 {
                for x in 0..=40 {
                    vertices.push(ao_scene::Vertex { pos: [x as f32 * 0.25 - 5.0, 0.0, z as f32 * 0.25 - 5.0],
                        normal: [0.0, 1.0, 0.0], color: ao_scene::WHITE, ..Default::default() });
                    if x < 40 && z < 40 {
                        let i = z * 41 + x;
                        indices.extend_from_slice(&[i, i + 41, i + 42, i, i + 42, i + 1]);
                    }
                }
            }
            let scene = ao_scene::Scene {
                meshes: vec![ao_scene::Mesh { vertices, submeshes: vec![ao_scene::Submesh::new(indices, None)] }],
                instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
                environment: Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 400.0,
                    fog_end: 800.0, ambient: [0.01; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0 }),
                ..Default::default()
            };
            let mut shot = Shot { gui: ao_gui::Gui::new(&dir, None).unwrap(), player, zone, scene: Some(scene) };
            let mut renderer = ao_render::Offscreen::new(&shot, size).unwrap();
            for (name, on, hidden) in [("off", false, false), ("on", true, false), ("hidden", true, true)] {
                shot.zone.own_events.push(OwnEvent::Appearance(light_bar_appearance(on)));
                shot.player.camera.set_prefs(&ControlPrefs { show_my_character: false, ..Default::default() });
                shot.player.camera.select_mode(if hidden { 0 } else { 3 });
                let list = renderer.frame(&mut shot, 0.016);
                renderer.png(&shot, &list, &out.join(format!("own-light-bar-{view}-{name}.png"))).unwrap();
            }
        }
    }

    #[test]
    fn jump_landing_uses_native_intent_not_weapon_variants() {
        use super::super::combat::anim::landing_anim;
        assert_eq!(landing_anim(2, true, false), Some(0xb9));
        assert_eq!(landing_anim(3, true, false), Some(0xba));
        assert_eq!(landing_anim(3, false, false), Some(0xbb));
        assert_eq!(landing_anim(3, false, true), Some(0xb9));
        assert_eq!(landing_anim(4, true, false), Some(0x85));
        assert_eq!(landing_anim(5, true, false), None);
    }

    /// The own avatar's model/rig does not change on a zone change (`zone-preload-proposal.md` §2.1: resolving the
    /// model's rest pose dominated the measured ~120-140 ms `Player::new` stall on every zone change, cold or
    /// warm). `drop_player`/`into_avatar_assets` (`flow.rs`) hand the warmed `ActorAssets` to the next `Player::new`
    /// instead of discarding it; a second build through the same `(dir, zone, playfield)` path must land far below
    /// the first build's cost.
    #[test]
    fn player_new_reuses_avatar_assets_across_zone_changes() {
        use super::super::zone::Zone;
        use std::time::Instant;
        fn frames(rec: &str) -> Vec<Frame> {
            rec.lines().filter_map(|l| {
                let mut p = l.split(' ');
                let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                (dir == "<").then(|| Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
            }).collect()
        }
        fn client() -> Option<std::path::PathBuf> {
            let d = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
            d.join("cd_image/rdb.db").exists().then_some(d)
        }
        let Some(dir) = client() else { return };
        let own = 25988;
        let mut zone = Zone::new(own as u32);
        zone.world.start(dir.clone(), own);
        for frame in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            zone.on_frame(&frame);
        }
        let playfield = zone.playfield.unwrap_or(800);
        let t0 = Instant::now();
        let player = Player::new(&dir, &mut zone, playfield).expect("capture has own character");
        let cold = t0.elapsed();
        // the production zone-change path (`Play::drop_player`): stash the warmed assets, then rebuild
        zone.world.stash_avatar_assets(player.into_avatar_assets());
        let t1 = Instant::now();
        let player = Player::new(&dir, &mut zone, playfield).expect("second build with the same own character");
        let warm = t1.elapsed();
        drop(player);
        eprintln!("player_new: cold={:.1}ms warm={:.1}ms", cold.as_secs_f64() * 1000.0, warm.as_secs_f64() * 1000.0);
        assert!(warm.as_secs_f64() * 1000.0 < cold.as_secs_f64() * 1000.0 / 2.0, "warm build (reused ActorAssets) must be well under half the cold build: cold={cold:?} warm={warm:?}");
    }
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
        let mut camera = camera::Camera3p::new(&Default::default(), Some(glam::Vec3::Y * 1.5));
        let feet = [0.0; 3];
        let pivot = glam::Vec3::new(0.0, 1.5, 0.0);
        let open = camera.update_with(feet, std::f32::consts::PI, 0.016, &Sight::with_clear(&clear));
        assert!((open.pos - pivot).length() > 4.99);
        let mut previous_radius = (open.pos - pivot).length();
        for yaw in [-0.2, 0.0, 0.2] {
            let view = camera.update_with(feet, yaw, 0.016, &Sight::with_clear(&clear));
            assert!((view.pos - pivot).length() < 2.0, "{:?}", view.pos);
            previous_radius = (view.pos - pivot).length();
            let dir = (view.pos - pivot).normalize();
            assert!(clear(pivot.to_array(), (view.pos + dir * (camera::COLLISION_RADIUS - camera::DEFAULT_DISTANCE * 0.001)).to_array()));
        }
        let restored = camera.update_with(feet, std::f32::consts::PI, 0.016, &Sight::with_clear(&clear));
        let expected_radius = 0.9 * previous_radius + 0.1 * camera::DEFAULT_DISTANCE;
        assert!(((restored.pos - pivot).length() - expected_radius).abs() < 1e-3,
            "turning away must blend outward, not immediately restore the full boom");
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
        let clear = |a, b| segment_clear(&collision, a, b);
        let mut animated = idle_camera_avatar(&store, &dir, std::env::var_os("AOMAC_CAMERA_ROCK_ANIMATED").is_some());
        if let Some(avatar) = &mut animated {
            avatar.update(1.0 / 60.0);
        }
        let initial_head = animated.as_ref().and_then(Avatar::head_local).unwrap_or(glam::Vec3::Y * 1.5);
        let mut pivot_height = initial_head.y;
        let mut camera = camera::Camera3p::new(&Default::default(), None);
        camera.set_head(initial_head);
        // Optional fixed server yaw (radians); absent keeps the 36-heading route.
        let idle_yaw = std::env::var("AOMAC_CAMERA_ROCK_IDLE_YAW").ok().map(|v| {
            let yaw = v.parse::<f32>().expect("fixed server yaw in radians");
            assert!(yaw.is_finite(), "fixed server yaw must be finite");
            yaw
        });
        if let (Some(avatar), Some(yaw)) = (&mut animated, idle_yaw) {
            for _ in 0..1800 {
                avatar.update(1.0 / 60.0);
                let head = avatar.head_local().unwrap();
                camera.set_head(head);
                pivot_height += (head.y - pivot_height) * 0.2;
                camera.update_with(feet, yaw, 1.0 / 60.0, &Sight::with_clear(&clear));
            }
        }
        let mut pulled = 0;
        for i in 0..if idle_yaw.is_some() { 120 } else { 36 } {
            let yaw = idle_yaw.unwrap_or(i as f32 * std::f32::consts::TAU / 36.0);
            let head = if let Some(avatar) = &mut animated {
                avatar.update(1.0 / 60.0);
                avatar.head_local().expect("captured player head attractor")
            } else { initial_head };
            let head_height = head.y;
            camera.set_head(head);
            // N3 FUN_10020bdb, default third-person blend; diagnostics only.
            pivot_height += (head_height - pivot_height) * 0.2;
            let pivot = glam::Vec3::from(feet) + glam::Quat::from_rotation_y(scene_yaw(yaw))
                * glam::Vec3::new(initial_head.x, pivot_height, initial_head.z);
            let view = camera.update_with(feet, yaw, 1.0 / 60.0, &Sight::with_clear(&clear));
            let distance = (view.pos - pivot).length();
            pulled += usize::from(distance < camera::DEFAULT_DISTANCE - 0.01);
            let dir = (view.pos - pivot).normalize();
            let final_boom_blocked = !clear(pivot.to_array(),
                (view.pos + dir * (camera::COLLISION_RADIUS - camera::DEFAULT_DISTANCE * 0.001)).to_array());
            let full_boom = pivot + dir * (camera::DEFAULT_DISTANCE + camera::COLLISION_RADIUS);
            let full_boom_blocked = !clear(pivot.to_array(), full_boom.to_array());
            eprintln!("ICC camera frame {i}: heading {yaw:.6}, head_height {head_height:.6}, pivot {pivot:?}, eye {:?}, eye_distance {distance:.6}, full_boom_blocked {full_boom_blocked}, final_boom_blocked {final_boom_blocked}", view.pos);
            if idle_yaw.is_none() {
                assert!(!final_boom_blocked, "heading {i}: pivot or boom is blocked beyond the original bisection tolerance");
            }
            if idle_yaw.is_none() || matches!(i, 0 | 50 | 102 | 119) {
                ao_render::render_to_png(&scene, view.pos.to_array(), pivot.to_array(), 1200, 700, &out.join(format!("icc-rock-{i:02}.png"))).unwrap();
            }
        }
        if idle_yaw.is_none() {
            assert!(pulled > 0, "the supplied ICC position never exercises collision pull-in");
        }
    }

    /// Opt-in captured player at animation time zero, before camera initialization.
    fn idle_camera_avatar(store: &RecordStore, dir: &std::path::Path, animated: bool) -> Option<Avatar> {
        if !animated {
            return None;
        }
        let mut avatar = Avatar::new(store, dir, 7, &avatar::tests::own_update(), ActorAssets::new(store).unwrap()).unwrap();
        avatar.set_pose(store, AvatarPose::still(Role::Idle)).unwrap();
        Some(avatar)
    }

    /// Exact captured ICC idle route with the real player's native idle clip.
    #[test]
    fn icc_beach_animated_idle_camera_is_stable() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let store = RecordStore::open(&dir).unwrap();
        let collision = Collision::load(&store, 4582).unwrap();
        let clear = |a, b| segment_clear(&collision, a, b);
        let feet = [930.0051, 24.21451, -759.66864];
        let yaw = 0.98020107;
        let mut avatar = idle_camera_avatar(&store, &dir, true).unwrap();
        avatar.update(1.0 / 60.0);
        let initial_head = avatar.head_local().unwrap();
        let mut height = initial_head.y;
        let mut camera = camera::Camera3p::new(&Default::default(), None);
        camera.set_head(initial_head);
        let mut previous = None;
        let mut max_displacement = 0.0_f32;
        for frame in 0..1920 {
            avatar.update(1.0 / 60.0);
            let head_local = avatar.head_local().unwrap();
            let head = head_local.y;
            camera.set_head(head_local);
            height += (head - height) * 0.2; // N3 default head blend, diagnostic pivot.
            let pivot = glam::Vec3::from(feet) + glam::Quat::from_rotation_y(scene_yaw(yaw))
                * glam::Vec3::new(initial_head.x, height, initial_head.z);
            let view = camera.update_with(feet, yaw, 1.0 / 60.0, &Sight::with_clear(&clear));
            let direction = (view.pos - pivot).normalize();
            let blocked = !clear(pivot.to_array(),
                (pivot + direction * (camera::DEFAULT_DISTANCE + camera::COLLISION_RADIUS)).to_array());
            let displacement = previous.map_or(0.0, |eye: glam::Vec3| (view.pos - eye).length());
            max_displacement = max_displacement.max(displacement);
            if frame >= 1800 {
                let measured_frame = frame - 1800;
                eprintln!("ICC animated idle frame {measured_frame}: heading {yaw:.6}, head_height {head:.6}, pivot {pivot:?}, eye {:?}, eye_distance {:.6}, full_boom_blocked {blocked}, displacement {displacement:.6}", view.pos, (view.pos - pivot).length());
            }
            assert!(!blocked, "full boom unexpectedly blocked at frame {frame}");
            assert!(((view.pos - pivot).length() - camera::DEFAULT_DISTANCE).abs() < 1e-3,
                "idle boom pulled in at frame {frame}");
            previous = Some(view.pos);
        }
        eprintln!("ICC animated idle maximum displacement {max_displacement:.6}");
        assert!(max_displacement < 1e-3, "idle eye displacement exceeds float tolerance: {max_displacement}");
    }

    /// Regression for the `fs_blend` pipeline crash on zoning into ICC Shuttleport (4582): `Renderer::update_liquids`
    /// (`native_liquid.rs`) retargets `self.gpu.liquid` draws onto the `effect` 12/13 pipes the moment a liquid's camera
    /// cell is built, and since the ground-detail commit those pipes used the 3-group `holo_pipeline_layout` for every
    /// `effect >= 8`; the liquid draw loop in `Renderer::render` never binds group 2, so wgpu validation rejects the draw.
    /// Runs the real offscreen path (`Renderer::render`, not just `update_liquids`) with a wgpu validation error scope so
    /// a missing bind group fails the test instead of only surfacing as a panic in the user's windowed `aomac play`.
    #[test]
    fn icc_shuttleport_liquid_draw_has_no_missing_bind_group_validation_error() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let store = RecordStore::open(&dir).unwrap();
        let scene = ao_formats::playfield::load_playfield_at(&store, &dir, 4582, ao_formats::playfield::DEFAULT_DAY_TIME).unwrap();
        assert!(!scene.liquids.is_empty(), "ICC Shuttleport (4582) must carry an authored liquid surface for this regression");
        let center = scene.liquids[0].initial_camera().expect("liquid surface has a source triangle");
        let eye = [center[0], center[1] + 3.0, center[2]];
        let look_at = [center[0] + 1.0, center[1], center[2]];
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = ao_render::Renderer::new(&instance, None).unwrap();
        r.upload(&scene);
        let targets = ao_render::Targets::new(&r, 64, 64);
        let out = r.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let scope = r.device.push_error_scope(wgpu::ErrorFilter::Validation);
        r.render(&out.create_view(&Default::default()), &targets, &ao_render::Camera::look_at(eye.into(), look_at.into()));
        r.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("liquid draw raised a wgpu validation error (missing bind group 2 for the `fs_blend` pipeline is the known crash): {error}");
        }
    }

    #[test]
    fn armed_camera_caches_first_animated_head_sample() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = idle_camera_avatar(&store, &dir, true).unwrap();
        let mut camera = camera::Camera3p::new(&Default::default(), None);
        avatar.set_stance(Some(3));
        avatar.set_pose(&store, AvatarPose::still(Role::Idle)).unwrap();
        avatar.update(1.0 / 60.0);
        let head = avatar.head_local().unwrap();
        camera.set_head(head);
        let feet = [930.0051, 24.21451, -759.66864];
        let yaw = 0.98020107;
        let pivot = glam::Vec3::from(feet) + glam::Quat::from_rotation_y(scene_yaw(yaw)) * head;
        let view = camera.update_with(feet, yaw, 1.0 / 60.0, &Sight::OPEN);
        assert!(((view.pos - pivot).length() - camera::DEFAULT_DISTANCE).abs() < 1e-3,
            "camera must initialize from the first armed animated sample");
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
