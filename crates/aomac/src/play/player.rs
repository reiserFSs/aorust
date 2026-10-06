//! The own character in the zone: glue between [`Controls`] (keys/mouse, `controls.rs`), [`Movement`] (the client's
//! movement state machine, `movement.rs`), the playfield [`Collision`], the [`Avatar`] model (`avatar.rs`) and the
//! third-person [`Camera3p`] (`camera.rs`). One `Player` per world (rebuilt when the own dynel is announced again).

use super::avatar::{self, Avatar, AvatarPose};
use super::camera::{self, Camera3p};
use super::controls::{CamCmd, Cmd, ControlPrefs, Controls};
use super::movement::{Movement, SitToggle, World};
use super::zone::{scene_pos, scene_yaw, Zone};
use ao_formats::playfield::collision::Collision;
use ao_gui::{InputEvent, MouseButton};
use ao_net::frame::Frame;
use ao_net::n3::outgoing::{char_dc_move, n3_frame};
use ao_rdb::RecordStore;
use ao_render::{GameInput, Host};
use ao_scene::Lens;
use std::path::Path;

/// Character capsule for wall sliding: [GUESS] the client's radius is per dynel (Vehicle sphere), docs/zone/collision.md.
const RADIUS: f32 = 0.35;

/// Server-coordinate view of the scene-coordinate [`Collision`] for [`Movement`].
struct Ground<'a>(Option<&'a Collision>, f32);

fn flip(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

impl World for Ground<'_> {
    fn ground(&self, p: [f32; 3]) -> Option<f32> {
        self.0?.ground(flip(p))
    }
    fn slide(&self, from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
        match self.0 {
            Some(c) => flip(c.slide(flip(from), flip(to), RADIUS, self.1)),
            None => to,
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
    /// The left/right press that went to the GUI: its release must not reach the controls.
    gui_press: [bool; 2],
    game: Vec<Cmd>,
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
            if let Some(c) = &collision {
                if let Some(g) = c.ground(flip([u.pos[0], u.pos[1] + 0.4, u.pos[2]])) {
                    movement.teleport([u.pos[0], g, u.pos[2]], u.yaw().unwrap_or(0.0));
                }
            }
            let prefs_xml = std::fs::read_to_string(dir.join("cd_image/gui/Default/CharPrefs.xml")).unwrap_or_default();
            let prefs = ControlPrefs::from_xml(&prefs_xml);
            let camera = Camera3p::new(&prefs, camera::DEFAULT_PIVOT_HEIGHT);
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
                gui_press: [false; 2],
                game: Vec::new(),
            })
        })();
        built.map_err(|e| eprintln!("player: {e:#}")).ok()
    }

    pub fn serial(&self) -> u32 {
        self.serial
    }

    pub fn pos(&self) -> [f32; 3] {
        self.movement.pos()
    }

    pub fn yaw(&self) -> f32 {
        self.movement.yaw()
    }

    /// Movement FSM mode (`FUN_100704e6`; 4 = swimming).
    pub fn mode(&self) -> u32 {
        u32::from(self.movement.fsm().mode)
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
                Cmd::Click(_) | Cmd::PickupItem | Cmd::Screenshot => {}
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
        let s = |id| zone.stat(id).map(|v| v as i32);
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
            ] {
                if let Some(v) = s(id) {
                    *field = v;
                }
            }
        });
        let world = Ground(self.collision.as_ref(), self.avatar.height());
        let out: Vec<Frame> = self.movement.update(dt, &world).iter().map(|m| n3_frame(0, self.char_id, char_dc_move(self.char_id as i32, m))).collect();
        let (pos, yaw) = (self.movement.pos(), self.movement.yaw());
        if let Some(d) = zone.dynels.get_mut(&(self.char_id as i32)) {
            d.pos = pos;
            d.yaw = Some(yaw);
        }

        let role = self.movement.role();
        let pose = if self.movement.speed() > 0.01 && self.movement.grounded() {
            AvatarPose { role, speed: self.movement.max_speed(), ref_speed: self.movement.ref_speed() }
        } else {
            AvatarPose::still(role)
        };
        if let Err(e) = self.avatar.set_pose(&self.store, pose) {
            eprintln!("avatar pose: {e:#}");
        }
        self.avatar.set_transform(scene_pos(pos), scene_yaw(yaw));
        self.avatar.update(dt);

        let col = self.collision.as_ref();
        let clear = |a: [f32; 3], b: [f32; 3]| col.is_none_or(|c| segment_clear(c, a, b));
        host.camera = self.camera.update_with(scene_pos(pos), yaw, dt, &clear);
        if !self.lens_set {
            // FOV 90 degrees horizontal (`VisualCamera_t`, docs/zone/camera.md); near/far stay the playfield's
            let l = host.lens.unwrap_or(zone.world.lens);
            host.lens = Some(Lens { fov: camera::FOV_HORIZONTAL, horizontal: true, ..l });
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

    /// `N3Msg_SitToggle` result for the caller (a stand-up request needs `CharacterActionIIR_t` op 0x57).
    pub fn sit(&mut self) -> SitToggle {
        self.movement.sit_toggle(self.clock)
    }

    /// `SlotMovementWalkToggle` / special actions 0x11 / 0x12: `MovementChanged(0x18 / 0x19)`.
    pub fn toggle_walk(&mut self) {
        self.movement.toggle_run(self.clock);
    }

    /// FSM `vtable[3](0x1e)`: the sit transition is allowed (`N3Msg_StartCamping`).
    pub fn can_sit(&self) -> bool {
        self.movement.fsm().allowed(0x1e)
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
