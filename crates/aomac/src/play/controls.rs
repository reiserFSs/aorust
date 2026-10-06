//! Player input of the zone (`ActionViewMouseHandler_c` + the `FlowControlModule_t` movement/camera slots): default key
//! bindings, mouse-look and wheel rules, translated into [`Cmd`]s. Pure state machine -- no window, no network; the lead
//! feeds it winit keys / ao_gui mouse events and routes the commands to `Movement` and `Camera3p`. Evidence and
//! addresses: docs/zone/camera.md.
//!
//! * Key ids are the client's own (`InputConfig_t` key table, GUI.dll 0x10262e20: 127 `{name*, id}` pairs); a binding is
//!   `id | modifier bits` (SHIFT 0x20000, CTRL 0x40000, ALT 0x80000) -> provider hash -> slot. The defaults in
//!   [`DEFAULT_BINDINGS`] are the `KeyBindings` archive of `cd_image/gui/Default/CharPrefs.xml` (provider = boost
//!   `hash_combine` of the provider name, see [`provider_hash`]); [`Controls::from_char_prefs`] reads a saved one.
//! * Slots call `N3Msg_MovementChanged(action)` with `b` = *released*: press = `Forward(false)` = action 1, release = 2.
//! * While a mouse-look is active the turn actions are remapped to strafes (`N3Msg_MovementChanged` @GC 0x10018b5c).
#![allow(dead_code)] // wired into play/flow.rs by the integration owner

use ao_gui::MouseButton;
use ao_render::KeyCode;

use super::hud::WindowKind;

/// Client key-table ids (GUI.dll 0x10262e20) of the keys we map.
pub mod id {
    pub const MIDDLE_PRESS: u32 = 9;
    pub const SPACE: u32 = 23;
    pub const BACKSPACE: u32 = 25;
    pub const CURSOR_LEFT: u32 = 32;
    pub const CURSOR_RIGHT: u32 = 33;
    pub const CURSOR_UP: u32 = 34;
    pub const CURSOR_DOWN: u32 = 35;
    pub const F1: u32 = 39;
    pub const F8: u32 = 46;
    pub const F12: u32 = 50;
    pub const NUMPAD_0: u32 = 70;
    pub const A: u32 = 82;
    pub const SHIFT: u32 = 0x20000;
    pub const CTRL: u32 = 0x40000;
    pub const ALT: u32 = 0x80000;
}

/// The client key id of a physical key (`None`: the client has no such key).
pub fn key_id(code: KeyCode) -> Option<u32> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const DIGITS: [KeyCode; 9] = [Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    const FKEYS: [KeyCode; 24] = [
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
    ];
    const PAD: [KeyCode; 10] = [Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9];
    if let Some(i) = LETTERS.iter().position(|&k| k == code) {
        return Some(82 + i as u32);
    }
    if let Some(i) = DIGITS.iter().position(|&k| k == code) {
        return Some(108 + i as u32);
    }
    if let Some(i) = FKEYS.iter().position(|&k| k == code) {
        return Some(39 + i as u32);
    }
    if let Some(i) = PAD.iter().position(|&k| k == code) {
        return Some(70 + i as u32);
    }
    Some(match code {
        Escape => 14,
        Tab => 15,
        CapsLock => 16,
        ShiftLeft => 17,
        ControlLeft => 18,
        AltLeft => 19,
        ShiftRight => 20,
        ControlRight => 21,
        AltRight => 22,
        Space => 23,
        Enter | NumpadEnter => 24, // NumpadEnter: [INFERENCE] the table has one ENTER
        Backspace => 25,
        Insert => 26,
        Delete => 27,
        Home => 28,
        End => 29,
        PageUp => 30,
        PageDown => 31,
        ArrowLeft => 32,
        ArrowRight => 33,
        ArrowUp => 34,
        ArrowDown => 35,
        Backslash | IntlBackslash => 36, // PIPE
        Period => 37,
        Comma => 38,
        ScrollLock => 63,
        Pause => 64,
        NumLock => 65,
        NumpadDivide => 66,
        NumpadMultiply => 67,
        NumpadSubtract => 68,
        NumpadAdd => 69,
        NumpadDecimal => 80,
        Digit0 => 117,
        Slash => 118,
        _ => return None,
    })
}

/// Key id of a character key (`Key::Letter`): `A`..`Z` = 82.., `1`..`9` = 108.., `0` = 117 (the same table as [`key_id`]).
pub fn char_key_id(c: char) -> Option<u32> {
    match c.to_ascii_lowercase() {
        l @ 'a'..='z' => Some(82 + (l as u32 - 'a' as u32)),
        d @ '1'..='9' => Some(108 + (d as u32 - '1' as u32)),
        '0' => Some(117),
        _ => None,
    }
}

/// Window hotkeys: the `WINDOW_*` providers of `ControlCenterModule_c::SetupProviders` (GUI 0x10068c38) with their `KeyBindings`
/// defaults of CharPrefs.xml (provider hashes matched with [`provider_hash`]: `WINDOW_PLANETMAP` = 97 (P), `WINDOW_INVENTORY` = 90 (I),
/// `WINDOW_SKILLS` = 102 (U), `WINDOW_WEAR` 262252 = CTRL+1, `WINDOW_MISSION` CTRL+4, `WINDOW_TEAM` CTRL+5, `WINDOW_MAP` 262257 = CTRL+6,
/// `WINDOW_FRIENDS` CTRL+7, `WINDOW_NANO` CTRL+8, `WINDOW_NCU` CTRL+0), plus the fixed `KEY_OPEN_PERK_WINDOW` = SHIFT+P (commands table in
/// GUI.dll). They agree with the help texts (`text/help/The * Window.html`). Providers with no window of ours (`WINDOW_SPECIALACTION` CTRL+2,
/// `WINDOW_KNOWLEDGE` CTRL+3, `WINDOW_RAID` SHIFT+CTRL+R, ...) are not listed. All of them are blocked by `TextInputMode`.
pub const WINDOW_BINDINGS: &[(u32, WindowKind)] = &[
    (90, WindowKind::Inventory),
    (102, WindowKind::Skills),
    (97, WindowKind::PlanetMap),
    (97 | id::SHIFT, WindowKind::Perks),
    (108 | id::CTRL, WindowKind::Character),
    (111 | id::CTRL, WindowKind::Mission),
    (112 | id::CTRL, WindowKind::Team),
    (113 | id::CTRL, WindowKind::Map),
    (114 | id::CTRL, WindowKind::Friends),
    (116 | id::CTRL, WindowKind::Stat),
    (115 | id::CTRL, WindowKind::Nano),
    (117 | id::CTRL, WindowKind::Ncu),
];

/// The window a character key press with these modifiers toggles.
pub fn window_for_key(c: char, mods: ao_gui::Modifiers) -> Option<WindowKind> {
    let key = char_key_id(c)? | if mods.shift { id::SHIFT } else { 0 } | if mods.ctrl { id::CTRL } else { 0 } | if mods.alt { id::ALT } else { 0 };
    WINDOW_BINDINGS.iter().find(|b| b.0 == key).map(|b| b.1)
}

/// What a binding triggers. The `*Global*` slots are the arrow keys (`SlotMovementGlobal*`), which keep working while
/// typing when Ctrl/Alt is held (`FUN_10027e46` @GUI 0x10027e46).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Forward,
    Back,
    TurnLeft,
    TurnRight,
    StrafeLeft,
    StrafeRight,
    AutoRun,
    Jump,
    WalkToggle,
    GlobalForward,
    GlobalBack,
    GlobalTurnLeft,
    GlobalTurnRight,
    ToggleCameraView,
    NextCameraView,
    PrevCameraView,
    Screenshot,
    PickupItem,
    /// `ACTION_ATTACK`, `ACTION_SWITCHTARGET`, `ACTION_SIT` and the special attacks (`ACTION_BRAWL` ...): `PerformSpecialAction` ids.
    Attack,
    SwitchTarget,
    Sit,
    Special(i32),
    CameraRotateLeft,
    CameraRotateRight,
    CameraRotateUp,
    CameraRotateDown,
    CameraZoomIn,
    CameraZoomOut,
    CameraSetPreferred,
    CameraReset,
}

impl Slot {
    fn is_global(self) -> bool {
        matches!(self, Slot::GlobalForward | Slot::GlobalBack | Slot::GlobalTurnLeft | Slot::GlobalTurnRight)
    }
    /// Fixed-key camera commands (`commands.txt` + the GUI.dll key table): they do not go through the `KeyBindings` archive.
    fn is_fixed_camera(self) -> bool {
        matches!(
            self,
            Slot::CameraRotateLeft
                | Slot::CameraRotateRight
                | Slot::CameraRotateUp
                | Slot::CameraRotateDown
                | Slot::CameraZoomIn
                | Slot::CameraZoomOut
                | Slot::CameraSetPreferred
                | Slot::CameraReset
        )
    }
}

/// Provider names registered by `FlowControlModule_t` (GUI.dll 0x1002abd6) -> slot. The `KeyBindings` archive stores
/// [`provider_hash`] of the name.
pub const PROVIDERS: &[(&str, Slot)] = &[
    ("MOVEMENT_FORWARD_GLOBAL_V2", Slot::GlobalForward),
    ("MOVEMENT_BACK_GLOBAL_V2", Slot::GlobalBack),
    ("MOVEMENT_LEFT_GLOBAL_V2", Slot::GlobalTurnLeft),
    ("MOVEMENT_RIGHT_GLOBAL_V2", Slot::GlobalTurnRight),
    ("MOVEMENT_FORWARD_V3", Slot::Forward),
    ("MOVEMENT_BACK_V2", Slot::Back),
    ("MOVEMENT_LEFT_V2", Slot::TurnLeft),
    ("MOVEMENT_RIGHT_V2", Slot::TurnRight),
    ("MOVEMENT_AUTORUN", Slot::AutoRun),
    ("MOVEMENT_STRAFELEFT", Slot::StrafeLeft),
    ("MOVEMENT_STRAFERIGHT", Slot::StrafeRight),
    ("MOVEMENT_JUMP", Slot::Jump),
    ("MOVEMENT_TOGGLEWALK", Slot::WalkToggle),
    ("CAMERA_TOGGLE3RD", Slot::ToggleCameraView),
    ("CAMERA_NEXTVIEW", Slot::NextCameraView),
    ("CAMERA_PREVVIEW", Slot::PrevCameraView),
    ("CAMERA_SCREENSHOT", Slot::Screenshot),
    ("ACTION_PICKUPITEM", Slot::PickupItem),
    ("ACTION_ATTACK", Slot::Attack),
    ("ACTION_SWITCHTARGET", Slot::SwitchTarget),
    ("ACTION_SIT", Slot::Sit),
    ("ACTION_BRAWL", Slot::Special(0x8e)),
    ("ACTION_DIMACH", Slot::Special(0x90)),
    ("ACTION_SNEAKATTACK", Slot::Special(0x92)),
    ("ACTION_FASTATTACK", Slot::Special(0x93)),
    ("ACTION_BURST", Slot::Special(0x94)),
    ("ACTION_FLINGSHOT", Slot::Special(0x96)),
    ("ACTION_AIMEDSHOT", Slot::Special(0x97)),
    ("ACTION_FULLAUTO", Slot::Special(0xa7)),
    ("ACTION_BOWSPECIALATTACK", Slot::Special(0x79)),
];

/// The provider key of the `KeyBindings` archive: `std::hash_combine` over the signed chars (GUI.dll 0x1001900a):
/// `h ^= (h << 6) + (h >> 2) + c + 0x9e3779b9`.
pub fn provider_hash(name: &str) -> u32 {
    name.bytes().fold(0u32, |h, c| h ^ (h << 6).wrapping_add(h >> 2).wrapping_add(c as i8 as i32 as u32).wrapping_add(0x9e37_79b9))
}

/// Default `KeyBindings` (CharPrefs.xml `Version` 7) for the slots above; the mouse middle button is input 9.
/// Only the numpad `AutoRun` key exists on a full keyboard -- laptops need [`Controls::from_char_prefs`].
pub const DEFAULT_BINDINGS: &[(u32, Slot)] = &[
    (id::MIDDLE_PRESS, Slot::Forward),
    (104, Slot::Forward), // W
    (82, Slot::TurnLeft), // A
    (85, Slot::TurnRight), // D
    (100, Slot::Back),    // S
    (107, Slot::StrafeLeft), // Z
    (84, Slot::StrafeRight), // C
    (id::SPACE, Slot::Jump),
    (id::BACKSPACE, Slot::WalkToggle),
    (id::NUMPAD_0, Slot::AutoRun),
    (id::CURSOR_LEFT, Slot::GlobalTurnLeft),
    (id::CURSOR_RIGHT, Slot::GlobalTurnRight),
    (id::CURSOR_UP, Slot::GlobalForward),
    (id::CURSOR_DOWN, Slot::GlobalBack),
    (id::F8, Slot::ToggleCameraView),
    (id::F8 | id::SHIFT, Slot::PrevCameraView),
    (id::F8 | id::CTRL, Slot::NextCameraView),
    (id::F12, Slot::Screenshot),
    (99, Slot::PickupItem), // R
    // CharPrefs.xml `KeyBindings`: the ACTION_* providers (hash of the provider name, `provider_hash`)
    (98, Slot::Attack),                   // Q
    (98 | id::SHIFT, Slot::SwitchTarget), // SHIFT+Q
    (105, Slot::Sit),                     // X
    (83, Slot::Special(0x8e)),            // B  Brawl
    (92, Slot::Special(0x90)),            // K  Dimach
    (91, Slot::Special(0x92)),            // J  Sneak attack
    (95, Slot::Special(0x93)),            // N  Fast attack
    (94, Slot::Special(0x94)),            // M  Burst
    (93, Slot::Special(0x96)),            // L  Fling shot
    (96, Slot::Special(0x97)),            // O  Aimed shot
    (38, Slot::Special(0xa7)),            // , Full auto
    (37, Slot::Special(0x79)),            // . Bow special attack
];

/// Fixed camera keys (GUI.dll input table text, `KEY_COMMAND_*`): numpad 4/6/2/8 rotate, + / - zoom, 7 save, 5 reset.
const FIXED_BINDINGS: &[(u32, Slot)] = &[
    (74, Slot::CameraRotateLeft),
    (76, Slot::CameraRotateRight),
    (72, Slot::CameraRotateUp),
    (78, Slot::CameraRotateDown),
    (69, Slot::CameraZoomIn),
    (68, Slot::CameraZoomOut),
    (77, Slot::CameraSetPreferred),
    (75, Slot::CameraReset),
];

/// The client's control options (`LoginPrefs.xml` "Control" block + `IndependentPrefs`).
#[derive(Clone, Debug, PartialEq)]
pub struct ControlPrefs {
    /// `MouseTurnSensitivity` (1..20): mouse counts / 1000 * this = radians.
    pub mouse_turn_sensitivity: f32,
    /// `ZoomSpeed` (1..40): wheel notch = `/ 10` metres, zoom keys = this metres per second.
    pub zoom_speed: f32,
    /// `LMBMouseLook`: dragging with the left button orbits the camera.
    pub lmb_mouse_look: bool,
    /// `RMBMouseLook1st` / `RMBMouseLook3rd`: dragging with the right button also pitches the camera.
    pub rmb_mouse_look_1st: bool,
    pub rmb_mouse_look_3rd: bool,
    /// `ZoomTo1stPerson`: zooming in/out past the limit switches first/third person.
    pub zoom_to_1st_person: bool,
    /// `MouseWheel`: 0 zoom, 1 scroll sidebar, 2 nothing.
    pub mouse_wheel: u8,
    /// `MouseLookInverted`.
    pub mouse_look_inverted: bool,
    /// `3rdPersonCamera`: start in third person.
    pub third_person: bool,
    /// `PreferredCameraMode` (1..3): the camera vehicle of third person, 3 = `CameraVehicleFixedThird_t(rigid)`, 2 = the
    /// same damped, 1 = the plain `CameraVehicle_t` (`FUN_10020290` @N3 0x10020290). Ctrl+F8 cycles 3 -> 2 -> 1 -> 3.
    pub preferred_camera_mode: u8,
    /// `ShowMyCharacter`: draw the own avatar in first person.
    pub show_my_character: bool,
}

impl Default for ControlPrefs {
    /// `cd_image/gui/Default/LoginPrefs.xml` and `SetDefaultLoginPrefs` (GUI @0x10124b33).
    fn default() -> Self {
        Self {
            mouse_turn_sensitivity: 10.0,
            zoom_speed: 20.0,
            lmb_mouse_look: true,
            rmb_mouse_look_1st: true,
            rmb_mouse_look_3rd: true,
            zoom_to_1st_person: true,
            mouse_wheel: 0,
            mouse_look_inverted: false,
            third_person: true,
            preferred_camera_mode: 3,
            show_my_character: false,
        }
    }
}

impl ControlPrefs {
    /// Overrides the defaults with the `<Value name=".." value=".."/>` entries of a prefs XML.
    pub fn from_xml(xml: &str) -> Self {
        let mut p = Self::default();
        let mut rest = xml;
        while let Some(i) = rest.find("<Value ") {
            let tag_end = rest[i..].find('>').map_or(rest.len(), |e| i + e);
            let tag = &rest[i..tag_end];
            rest = &rest[tag_end.min(rest.len())..];
            let (Some(name), Some(value)) = (attr(tag, "name"), attr(tag, "value")) else { continue };
            let b = || value == "true" || value == "1";
            match name {
                "MouseTurnSensitivity" => p.mouse_turn_sensitivity = value.parse().unwrap_or(p.mouse_turn_sensitivity),
                "ZoomSpeed" => p.zoom_speed = value.parse().unwrap_or(p.zoom_speed),
                "LMBMouseLook" => p.lmb_mouse_look = b(),
                "RMBMouseLook1st" => p.rmb_mouse_look_1st = b(),
                "RMBMouseLook3rd" => p.rmb_mouse_look_3rd = b(),
                "ZoomTo1stPerson" => p.zoom_to_1st_person = b(),
                "MouseWheel" => p.mouse_wheel = value.parse().unwrap_or(0),
                "MouseLookInverted" => p.mouse_look_inverted = b(),
                "ShowMyCharacter" => p.show_my_character = b(),
                // 0 would mean first person; entering third person maps it to 1 (`FUN_10021859` @N3 0x10021859)
                "PreferredCameraMode" => p.preferred_camera_mode = value.parse::<u8>().ok().filter(|m| *m <= 3).map_or(p.preferred_camera_mode, |m| m.max(1)),
                _ => {}
            }
        }
        p
    }
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let i = tag.find(&key)? + key.len();
    let e = tag[i..].find('"')?;
    Some(&tag[i..i + e])
}

/// Camera keys that act while held (`n3Camera_t::Start/Stop{Rotate*,Zoom*}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CamKey {
    RotateLeft,
    RotateRight,
    RotateUp,
    RotateDown,
    ZoomIn,
    ZoomOut,
}

/// Camera commands for [`super::camera::Camera3p::apply`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CamCmd {
    /// `N3Msg_CameraMouseLookMovement(dx, dy)`: left-drag orbit, radians, +dx = look right, +dy = camera up / look down.
    Orbit { dx: f32, dy: f32 },
    /// Right-drag pitch (`n3Camera_t::MouseCameraControl(0, dy)` when `RMBMouseLook1st/3rd`).
    Pitch { dy: f32 },
    /// Wheel notches, positive = zoom in.
    Zoom(f32),
    Key { key: CamKey, down: bool },
    /// NUMPAD_5.
    Reset,
    /// NUMPAD_7.
    SetPreferred,
    /// F8: toggles `3rdPersonCamera`.
    ToggleView,
    /// Ctrl+F8: drop the attractor and cycle the camera vehicle (`GetNextVisibleAttractor`); Shift+F8: previous attractor.
    NextView,
    PrevView,
    /// A left-button look ended (`N3Msg_EndCameraMouseLook`).
    EndLook,
}

/// What the input layer asks the game to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// `N3Msg_MovementChanged(MovementAction_e)`, after the mouse-look remap. Jump (0xF) and auto-run (1) are plain moves.
    Move(u8),
    /// `N3Msg_MouseMovement(dx, dy)` (action 0x2B): right-drag, turn the character by `dx` radians and pitch by `dy`.
    MouseTurn { dx: f32, dy: f32 },
    /// `SlotMovementWalkToggle`: `N3Msg_PerformSpecialAction(0x12 - (GetLastSpeedMode() != 2))`.
    ToggleWalk,
    Camera(CamCmd),
    /// `SlotPickupItem`: `N3Msg_GetItem(object under the mouse)`.
    PickupItem,
    /// `ACTION_ATTACK`: `FUN_1004256c(0xb)` on the selected target.
    Attack,
    /// `ACTION_SWITCHTARGET`: attack the selected target even when fighting another one.
    SwitchTarget,
    /// `ACTION_SIT` (`N3Msg_SitToggle`).
    Sit,
    /// Special attack `Stat_e` (`N3Msg_SecondarySpecialAttack`).
    Special(i32),
    Screenshot,
    /// A mouse button was released without dragging (`ActionViewMouseHandler_c` release): left = select the object under
    /// the cursor, right = its default action.
    Click(MouseButton),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    Key(KeyCode),
    Middle,
}

struct Held {
    input: Input,
    slot: Slot,
    /// Movement action sent on press (the stop action is derived from it).
    started: Option<u8>,
    /// `started` is a strafe produced by the mouse-look remap.
    remapped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    /// Camera orbit (`N3Msg_CameraMouseLookMovement`).
    Camera,
    /// Character turn (`N3Msg_MouseMovement`).
    Turn,
}

/// Pixel-count scale of raw mouse deltas (`InputConfig_t::FrameProcess` @GUI 0x1001ae14: `counts / 1000.0`).
const MOUSE_COUNT_SCALE: f32 = 1000.0;
/// Accumulated movement (in scaled units) up to which a press/release still counts as a click (GUI 0x101aeaf4).
const CLICK_THRESHOLD: f32 = 0.02;

pub struct Controls {
    prefs: ControlPrefs,
    bindings: Vec<(u32, Slot)>,
    held: Vec<Held>,
    shift: bool,
    ctrl: bool,
    alt: bool,
    text_input: bool,
    pending: Option<Look>,
    active: Option<Look>,
    /// Mouse button that started the look (left/right).
    look_button: Option<MouseButton>,
    /// `DAT_102e2588`: a mouse-look ran since the last `End*MouseLook`; turns are remapped to strafes.
    look_flag: bool,
    acc: f32,
}

impl Controls {
    /// Client defaults ([`DEFAULT_BINDINGS`] + fixed camera keys).
    pub fn new(prefs: ControlPrefs) -> Self {
        let mut bindings = DEFAULT_BINDINGS.to_vec();
        bindings.extend_from_slice(FIXED_BINDINGS);
        Self {
            prefs,
            bindings,
            held: vec![],
            shift: false,
            ctrl: false,
            alt: false,
            text_input: false,
            pending: None,
            active: None,
            look_button: None,
            look_flag: false,
            acc: 0.0,
        }
    }

    /// The `KeyBindings` archive of a (saved) CharPrefs.xml replaces the movement/camera bindings. Entries with unknown
    /// providers are ignored; the fixed camera keys stay.
    pub fn from_char_prefs(prefs: ControlPrefs, xml: &str) -> Self {
        let mut c = Self::new(prefs);
        let table: Vec<(u32, Slot)> = parse_key_bindings(xml);
        if !table.is_empty() {
            c.bindings = table;
            c.bindings.extend_from_slice(FIXED_BINDINGS);
        }
        c
    }

    /// CTRL or ALT is down (the `& 0xc` qualifier of `ActionViewMouseHandler_c`'s release slot; which bit is which is unresolved).
    pub fn attack_modifier(&self) -> bool {
        self.ctrl || self.alt
    }

    pub fn prefs(&self) -> &ControlPrefs {
        &self.prefs
    }

    /// A text field has the keyboard (chat input): only the arrow keys with Ctrl/Alt still move.
    pub fn set_text_input(&mut self, on: bool) {
        self.text_input = on;
    }

    /// The cursor must be captured: a look button is down (the first mouse movement turns it into a look).
    pub fn mouse_capture(&self) -> bool {
        self.pending.is_some() || self.active.is_some()
    }

    /// The camera is being dragged with the mouse.
    pub fn looking(&self) -> bool {
        self.active.is_some()
    }

    fn mods(&self) -> u32 {
        (if self.shift { id::SHIFT } else { 0 }) | (if self.ctrl { id::CTRL } else { 0 }) | (if self.alt { id::ALT } else { 0 })
    }

    fn lookup(&self, input: u32) -> Option<Slot> {
        self.bindings.iter().find(|b| b.0 == input).map(|b| b.1)
    }

    /// A physical key changed. Repeats (a press of a key already down) are ignored.
    pub fn on_key(&mut self, code: KeyCode, pressed: bool) -> Vec<Cmd> {
        match code {
            KeyCode::ShiftLeft | KeyCode::ShiftRight => self.shift = pressed,
            KeyCode::ControlLeft | KeyCode::ControlRight => self.ctrl = pressed,
            KeyCode::AltLeft | KeyCode::AltRight => self.alt = pressed,
            _ => {}
        }
        let input = Input::Key(code);
        if !pressed {
            return self.release(input);
        }
        if self.held.iter().any(|h| h.input == input) {
            return vec![];
        }
        let Some(kid) = key_id(code) else { return vec![] };
        // The arrow slots test Ctrl/Alt themselves (FUN_10027e46), so their bindings must match with those held:
        // [INFERENCE] the bare key id is the fallback for them.
        let slot = self.lookup(kid | self.mods()).or_else(|| self.lookup(kid).filter(|s| s.is_global()));
        let Some(slot) = slot else { return vec![] };
        let typing_ok = slot.is_fixed_camera() || (slot.is_global() && (self.ctrl || self.alt));
        if self.text_input && !typing_ok {
            return vec![];
        }
        self.press(input, slot)
    }

    fn press(&mut self, input: Input, slot: Slot) -> Vec<Cmd> {
        let mut out = vec![];
        let mut started = None;
        match slot {
            Slot::Forward | Slot::GlobalForward => started = Some(1),
            Slot::Back | Slot::GlobalBack => started = Some(3),
            Slot::StrafeRight => started = Some(5),
            Slot::StrafeLeft => started = Some(7),
            Slot::TurnRight | Slot::GlobalTurnRight => started = Some(9),
            Slot::TurnLeft | Slot::GlobalTurnLeft => started = Some(0xC),
            Slot::AutoRun => out.push(Cmd::Move(1)), // SlotMovementAutoRun = SlotMovementForward(false); no stop
            Slot::Jump => out.push(Cmd::Move(0xF)),
            Slot::WalkToggle => out.push(Cmd::ToggleWalk),
            Slot::ToggleCameraView => out.push(Cmd::Camera(CamCmd::ToggleView)),
            Slot::NextCameraView => out.push(Cmd::Camera(CamCmd::NextView)),
            Slot::PrevCameraView => out.push(Cmd::Camera(CamCmd::PrevView)),
            Slot::Screenshot => out.push(Cmd::Screenshot),
            Slot::PickupItem => out.push(Cmd::PickupItem),
            Slot::Attack => out.push(Cmd::Attack),
            Slot::SwitchTarget => out.push(Cmd::SwitchTarget),
            Slot::Sit => out.push(Cmd::Sit),
            Slot::Special(s) => out.push(Cmd::Special(s)),
            Slot::CameraRotateLeft => out.push(cam_key(CamKey::RotateLeft, true)),
            Slot::CameraRotateRight => out.push(cam_key(CamKey::RotateRight, true)),
            Slot::CameraRotateUp => out.push(cam_key(CamKey::RotateUp, true)),
            Slot::CameraRotateDown => out.push(cam_key(CamKey::RotateDown, true)),
            Slot::CameraZoomIn => out.push(cam_key(CamKey::ZoomIn, true)),
            Slot::CameraZoomOut => out.push(cam_key(CamKey::ZoomOut, true)),
            Slot::CameraSetPreferred => out.push(Cmd::Camera(CamCmd::SetPreferred)),
            Slot::CameraReset => out.push(Cmd::Camera(CamCmd::Reset)),
        }
        let mut remapped = false;
        if let Some(a) = started {
            let a = if self.look_flag { remap(a) } else { a };
            remapped = self.look_flag && matches!(a, 5 | 7) && matches!(slot, Slot::TurnLeft | Slot::TurnRight | Slot::GlobalTurnLeft | Slot::GlobalTurnRight);
            started = Some(a);
            out.push(Cmd::Move(a));
        }
        self.held.push(Held { input, slot, started, remapped });
        out
    }

    fn release(&mut self, input: Input) -> Vec<Cmd> {
        let Some(i) = self.held.iter().position(|h| h.input == input) else { return vec![] };
        let h = self.held.remove(i);
        match (h.slot, h.started) {
            (_, Some(a)) => vec![Cmd::Move(stop_of(a))],
            (Slot::CameraRotateLeft, _) => vec![cam_key(CamKey::RotateLeft, false)],
            (Slot::CameraRotateRight, _) => vec![cam_key(CamKey::RotateRight, false)],
            (Slot::CameraRotateUp, _) => vec![cam_key(CamKey::RotateUp, false)],
            (Slot::CameraRotateDown, _) => vec![cam_key(CamKey::RotateDown, false)],
            (Slot::CameraZoomIn, _) => vec![cam_key(CamKey::ZoomIn, false)],
            (Slot::CameraZoomOut, _) => vec![cam_key(CamKey::ZoomOut, false)],
            _ => vec![],
        }
    }

    /// Mouse button press/release. Left/right drive mouse-look and clicks; the middle button is binding input 9.
    pub fn on_mouse_button(&mut self, button: MouseButton, pressed: bool) -> Vec<Cmd> {
        match (button, pressed) {
            (MouseButton::Middle, true) => match self.lookup(id::MIDDLE_PRESS | self.mods()) {
                Some(slot) if !self.held.iter().any(|h| h.input == Input::Middle) => self.press(Input::Middle, slot),
                _ => vec![],
            },
            (MouseButton::Middle, false) => self.release(Input::Middle),
            (MouseButton::Left, true) => {
                if self.prefs.lmb_mouse_look {
                    self.pending = Some(Look::Camera);
                    self.look_button = Some(button);
                }
                vec![]
            }
            (MouseButton::Right, true) => {
                self.pending = Some(Look::Turn);
                self.look_button = Some(button);
                vec![]
            }
            (_, false) => {
                let mut out = vec![];
                // FUN_1002c469: a click needs no look yet or at most CLICK_THRESHOLD of movement.
                if self.active.is_none() || self.acc <= CLICK_THRESHOLD {
                    out.push(Cmd::Click(button));
                }
                out.extend(self.end_look());
                out
            }
        }
    }

    /// Raw mouse movement (counts) while a look button is down.
    pub fn on_mouse_motion(&mut self, dx: f32, dy: f32) -> Vec<Cmd> {
        let mut out = vec![];
        if let Some(p) = self.pending.take() {
            self.active = Some(p);
            self.acc = 0.0;
        }
        let Some(mode) = self.active else { return out };
        // Right button + Ctrl drags the camera instead of turning (FUN_1002c17b).
        let mode = match (self.look_button, self.ctrl) {
            (Some(MouseButton::Right), true) if mode == Look::Turn => {
                out.extend(self.stop_remapped());
                self.active = Some(Look::Camera);
                Look::Camera
            }
            (Some(MouseButton::Right), false) if mode == Look::Camera => {
                out.push(Cmd::Camera(CamCmd::EndLook));
                out.extend(self.stop_remapped());
                self.look_flag = false;
                self.active = Some(Look::Turn);
                Look::Turn
            }
            _ => mode,
        };
        let (fx, fy) = (dx / MOUSE_COUNT_SCALE, dy / MOUSE_COUNT_SCALE);
        self.acc += (fx * fx + fy * fy).sqrt();
        let s = self.prefs.mouse_turn_sensitivity;
        let inv = if self.prefs.mouse_look_inverted { -1.0 } else { 1.0 };
        let (sx, sy) = (fx * s, fy * s * inv);
        match mode {
            Look::Camera => {
                self.look_flag = true;
                out.push(Cmd::Camera(CamCmd::Orbit { dx: sx, dy: sy }));
            }
            Look::Turn => {
                if !self.look_flag {
                    // N3Msg_MouseMovement: the first movement converts a running turn into a strafe.
                    out.extend(self.turns_to_strafes());
                    self.look_flag = true;
                }
                out.push(Cmd::MouseTurn { dx: sx, dy: sy });
                out.push(Cmd::Camera(CamCmd::Pitch { dy: sy }));
            }
        }
        out
    }

    /// Wheel over the world, notches (positive = up = zoom in).
    pub fn on_wheel(&mut self, notches: f32) -> Vec<Cmd> {
        if self.prefs.mouse_wheel == 0 && notches != 0.0 {
            vec![Cmd::Camera(CamCmd::Zoom(notches))]
        } else {
            vec![]
        }
    }

    fn turns_to_strafes(&mut self) -> Vec<Cmd> {
        let mut out = vec![];
        for h in &mut self.held {
            if let Some(a @ (9 | 0xC)) = h.started {
                out.push(Cmd::Move(stop_of(a)));
                let s = remap(a);
                out.push(Cmd::Move(s));
                h.started = Some(s);
                h.remapped = true;
            }
        }
        out
    }

    /// Stops the strafes that stand in for turns (`N3Msg_EndCameraMouseLook` sends the stops 8 and 6).
    fn stop_remapped(&mut self) -> Vec<Cmd> {
        let mut out = vec![];
        for h in &mut self.held {
            if h.remapped {
                if let Some(a) = h.started.take() {
                    out.push(Cmd::Move(stop_of(a)));
                }
                h.remapped = false;
            }
        }
        out
    }

    fn end_look(&mut self) -> Vec<Cmd> {
        let mut out = vec![];
        if self.active == Some(Look::Camera) {
            out.push(Cmd::Camera(CamCmd::EndLook));
        }
        if self.active.is_some() {
            out.extend(self.stop_remapped());
        }
        self.pending = None;
        self.active = None;
        self.look_button = None;
        self.look_flag = false;
        self.acc = 0.0;
        out
    }
}

fn cam_key(key: CamKey, down: bool) -> Cmd {
    Cmd::Camera(CamCmd::Key { key, down })
}

/// `N3Msg_MovementChanged` mouse-look remap: turn left/right (and their stops) become strafes.
fn remap(a: u8) -> u8 {
    match a {
        0xC => 7,
        9 => 5,
        0xE => 8,
        0xB => 6,
        a => a,
    }
}

/// The `MovementAction_e` that ends a started movement: forward 1->2, back 3->4, strafe 5->6 / 7->8, turn 9->0xB / 0xC->0xE.
fn stop_of(start: u8) -> u8 {
    match start {
        9 => 0xB,
        0xC => 0xE,
        a => a + 1,
    }
}

/// `Input`/`Provider` pairs of a CharPrefs.xml `KeyBindings` archive, restricted to the providers we know.
fn parse_key_bindings(xml: &str) -> Vec<(u32, Slot)> {
    let mut out = vec![];
    let Some(start) = xml.find("name=\"KeyBindings\"") else { return out };
    let mut input: Option<u32> = None;
    for tag in xml[start..].split('<').skip(1) {
        if tag.starts_with("Int32 name=\"Input\"") {
            input = attr(tag, "value").and_then(|v| v.parse().ok());
        } else if tag.starts_with("Int64 name=\"Provider\"") {
            let (Some(i), Some(p)) = (input, attr(tag, "value").and_then(|v| v.parse::<u64>().ok())) else { continue };
            if let Some(&(_, slot)) = PROVIDERS.iter().find(|(n, _)| u64::from(provider_hash(n)) == p) {
                out.push((i, slot));
            }
        } else if tag.starts_with("/Array") {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyCode::*;

    /// Rounds the float fields to 1e-4 so expectations can be written as literals.
    fn norm(cmds: Vec<Cmd>) -> Vec<Cmd> {
        let r = |x: f32| (x * 1e4).round() / 1e4;
        cmds.into_iter()
            .map(|c| match c {
                Cmd::MouseTurn { dx, dy } => Cmd::MouseTurn { dx: r(dx), dy: r(dy) },
                Cmd::Camera(CamCmd::Orbit { dx, dy }) => Cmd::Camera(CamCmd::Orbit { dx: r(dx), dy: r(dy) }),
                Cmd::Camera(CamCmd::Pitch { dy }) => Cmd::Camera(CamCmd::Pitch { dy: r(dy) }),
                c => c,
            })
            .collect()
    }

    fn ctl() -> Controls {
        Controls::new(ControlPrefs::default())
    }

    fn client_file(rel: &str) -> Option<String> {
        let p = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client").join(rel);
        std::fs::read_to_string(p).ok()
    }

    #[test]
    fn provider_hash_matches_char_prefs() {
        // Providers of CharPrefs.xml inputs 34 / 35 / 32.
        assert_eq!(provider_hash("MOVEMENT_FORWARD_GLOBAL_V2"), 1539102526);
        assert_eq!(provider_hash("MOVEMENT_BACK_GLOBAL_V2"), 4062287708);
        assert_eq!(provider_hash("MOVEMENT_LEFT_GLOBAL_V2"), 2860728372);
    }

    #[test]
    fn default_table_equals_client_char_prefs() {
        let Some(xml) = client_file("cd_image/gui/Default/CharPrefs.xml") else { return };
        let mut parsed = parse_key_bindings(&xml);
        let mut ours = DEFAULT_BINDINGS.to_vec();
        parsed.sort_by_key(|b| b.0);
        ours.sort_by_key(|b| b.0);
        assert_eq!(parsed, ours);
        // ... and the prefs XML gives the documented option defaults.
        let login = client_file("cd_image/gui/Default/LoginPrefs.xml").unwrap();
        assert_eq!(ControlPrefs::from_xml(&login), ControlPrefs::default());
    }

    #[test]
    fn window_hotkeys_equal_client_char_prefs() {
        let Some(xml) = client_file("cd_image/gui/Default/CharPrefs.xml") else { return };
        // (provider, window): the default binding of the provider in CharPrefs.xml is the table entry of the window
        let providers = [
            ("WINDOW_INVENTORY", WindowKind::Inventory),
            ("WINDOW_SKILLS", WindowKind::Skills),
            ("WINDOW_PLANETMAP", WindowKind::PlanetMap),
            ("WINDOW_WEAR", WindowKind::Character),
            ("WINDOW_MISSION", WindowKind::Mission),
            ("WINDOW_TEAM", WindowKind::Team),
            ("WINDOW_MAP", WindowKind::Map),
            ("WINDOW_FRIENDS", WindowKind::Friends),
            ("WINDOW_NANO", WindowKind::Nano),
            ("WINDOW_NCU", WindowKind::Ncu),
            ("WINDOW_STAT", WindowKind::Stat),
        ];
        let mut input = 0;
        let mut found = 0;
        for tag in xml.split('<') {
            if tag.starts_with("Int32 name=\"Input\"") {
                input = attr(tag, "value").and_then(|v| v.parse().ok()).unwrap_or(0);
            } else if tag.starts_with("Int64 name=\"Provider\"") {
                let p: u64 = attr(tag, "value").and_then(|v| v.parse().ok()).unwrap_or(0);
                if let Some((n, w)) = providers.iter().find(|(n, _)| u64::from(provider_hash(n)) == p) {
                    assert!(WINDOW_BINDINGS.contains(&(input, *w)), "{n}: {input}");
                    found += 1;
                }
            }
        }
        assert_eq!(found, providers.len());
        let ctrl = ao_gui::Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(window_for_key('6', ctrl), Some(WindowKind::Map));
        assert_eq!(window_for_key('p', Default::default()), Some(WindowKind::PlanetMap));
        assert_eq!(window_for_key('p', ao_gui::Modifiers { shift: true, ..Default::default() }), Some(WindowKind::Perks));
        assert_eq!(window_for_key('6', Default::default()), None);
    }

    #[test]
    fn movement_keys_press_and_release() {
        let mut c = ctl();
        for (key, down, up) in [(KeyW, 1, 2), (KeyS, 3, 4), (KeyC, 5, 6), (KeyZ, 7, 8), (KeyD, 9, 0xB), (KeyA, 0xC, 0xE)] {
            assert_eq!(c.on_key(key, true), vec![Cmd::Move(down)], "{key:?}");
            assert_eq!(c.on_key(key, true), vec![], "repeat ignored");
            assert_eq!(c.on_key(key, false), vec![Cmd::Move(up)], "{key:?}");
        }
        // jump only on press, auto-run = forward start without stop
        assert_eq!(c.on_key(Space, true), vec![Cmd::Move(0xF)]);
        assert_eq!(c.on_key(Space, false), vec![]);
        assert_eq!(c.on_key(Numpad0, true), vec![Cmd::Move(1)]);
        assert_eq!(c.on_key(Numpad0, false), vec![]);
        assert_eq!(c.on_key(Backspace, true), vec![Cmd::ToggleWalk]);
        assert_eq!(c.on_key(KeyR, true), vec![Cmd::PickupItem]);
        assert_eq!(c.on_key(F12, true), vec![Cmd::Screenshot]);
    }

    #[test]
    fn arrows_and_modifiers() {
        let mut c = ctl();
        assert_eq!(c.on_key(ArrowUp, true), vec![Cmd::Move(1)]);
        assert_eq!(c.on_key(ArrowUp, false), vec![Cmd::Move(2)]);
        // Shift+W is not bound; the stop of a key pressed before the modifier still fires.
        c.on_key(ShiftLeft, true);
        assert_eq!(c.on_key(KeyW, true), vec![]);
        c.on_key(ShiftLeft, false);
        assert_eq!(c.on_key(KeyW, true), vec![Cmd::Move(1)]);
        c.on_key(ShiftLeft, true);
        assert_eq!(c.on_key(KeyW, false), vec![Cmd::Move(2)]);
        // F8 family
        assert_eq!(c.on_key(F8, true), vec![Cmd::Camera(CamCmd::PrevView)]); // Shift held
        c.on_key(F8, false);
        c.on_key(ShiftLeft, false);
        assert_eq!(c.on_key(F8, true), vec![Cmd::Camera(CamCmd::ToggleView)]);
        c.on_key(F8, false);
        c.on_key(ControlLeft, true);
        assert_eq!(c.on_key(F8, true), vec![Cmd::Camera(CamCmd::NextView)]);
    }

    #[test]
    fn typing_blocks_all_but_ctrl_arrows() {
        let mut c = ctl();
        c.set_text_input(true);
        assert_eq!(c.on_key(KeyW, true), vec![]);
        assert_eq!(c.on_key(ArrowUp, true), vec![]);
        c.on_key(ControlLeft, true);
        assert_eq!(c.on_key(ArrowLeft, true), vec![Cmd::Move(0xC)]);
    }

    #[test]
    fn fixed_camera_keys_hold_and_release() {
        let mut c = ctl();
        assert_eq!(c.on_key(Numpad4, true), vec![cam_key(CamKey::RotateLeft, true)]);
        assert_eq!(c.on_key(Numpad4, false), vec![cam_key(CamKey::RotateLeft, false)]);
        assert_eq!(c.on_key(NumpadAdd, true), vec![cam_key(CamKey::ZoomIn, true)]);
        assert_eq!(c.on_key(Numpad5, true), vec![Cmd::Camera(CamCmd::Reset)]);
        assert_eq!(c.on_key(Numpad7, true), vec![Cmd::Camera(CamCmd::SetPreferred)]);
    }

    #[test]
    fn middle_button_is_forward() {
        let mut c = ctl();
        assert_eq!(c.on_mouse_button(MouseButton::Middle, true), vec![Cmd::Move(1)]);
        assert_eq!(c.on_mouse_button(MouseButton::Middle, false), vec![Cmd::Move(2)]);
    }

    /// The client has no left+right chord: `AnarchyOnline.exe` maps `WM_*BUTTON*` 1:1 to the input ids 1/5/9, no GUI/Gamecode code
    /// tests two buttons together (docs/zone/camera.md §5), so holding both starts the two looks and moves nothing.
    #[test]
    fn both_buttons_do_not_move_forward() {
        let mut c = ctl();
        assert_eq!(c.on_mouse_button(MouseButton::Left, true), vec![]);
        assert_eq!(c.on_mouse_button(MouseButton::Right, true), vec![]);
        for cmd in c.on_mouse_motion(30.0, 0.0) {
            assert!(!matches!(cmd, Cmd::Move(1)), "{cmd:?}");
        }
    }

    #[test]
    fn click_without_drag_and_sensitivity() {
        let mut c = ctl();
        assert_eq!(c.on_mouse_button(MouseButton::Right, true), vec![]);
        assert!(c.mouse_capture());
        assert_eq!(c.on_mouse_button(MouseButton::Right, false), vec![Cmd::Click(MouseButton::Right)]);
        assert!(!c.mouse_capture());
        // 1 count = 0.001 * 10 = 0.01 rad, below the click threshold of 0.02
        c.on_mouse_button(MouseButton::Left, true);
        let out = norm(c.on_mouse_motion(1.0, 0.0));
        assert_eq!(out, vec![Cmd::Camera(CamCmd::Orbit { dx: 0.01, dy: 0.0 })]);
        assert_eq!(c.on_mouse_button(MouseButton::Left, false), vec![Cmd::Click(MouseButton::Left), Cmd::Camera(CamCmd::EndLook)]);
        // a real drag is no click: 100 counts -> 1 rad
        c.on_mouse_button(MouseButton::Left, true);
        let out = norm(c.on_mouse_motion(100.0, -50.0));
        assert_eq!(out, vec![Cmd::Camera(CamCmd::Orbit { dx: 1.0, dy: -0.5 })]);
        assert_eq!(c.on_mouse_button(MouseButton::Left, false), vec![Cmd::Camera(CamCmd::EndLook)]);
    }

    #[test]
    fn lmb_look_pref_off_and_inversion() {
        let mut c = Controls::new(ControlPrefs { lmb_mouse_look: false, mouse_look_inverted: true, ..Default::default() });
        c.on_mouse_button(MouseButton::Left, true);
        assert!(!c.mouse_capture());
        c.on_mouse_button(MouseButton::Right, true);
        let out = norm(c.on_mouse_motion(10.0, 10.0));
        assert_eq!(out[..2], [Cmd::MouseTurn { dx: 0.1, dy: -0.1 }, Cmd::Camera(CamCmd::Pitch { dy: -0.1 })]);
    }

    #[test]
    fn right_drag_turns_character_and_remaps_turn_keys() {
        let mut c = ctl();
        // A (turn left) is running when the right-button drag starts: stop turn, start strafe left
        assert_eq!(c.on_key(KeyA, true), vec![Cmd::Move(0xC)]);
        c.on_mouse_button(MouseButton::Right, true);
        let out = norm(c.on_mouse_motion(100.0, 0.0));
        assert_eq!(out, vec![Cmd::Move(0xE), Cmd::Move(7), Cmd::MouseTurn { dx: 1.0, dy: 0.0 }, Cmd::Camera(CamCmd::Pitch { dy: 0.0 })]);
        // D pressed during the drag is a strafe right (9 -> 5) and stops with 6
        assert_eq!(c.on_key(KeyD, true), vec![Cmd::Move(5)]);
        assert_eq!(c.on_key(KeyD, false), vec![Cmd::Move(6)]);
        // releasing A during the drag stops the strafe it became
        assert_eq!(c.on_key(KeyA, false), vec![Cmd::Move(8)]);
        c.on_key(KeyD, true);
        c.on_key(KeyD, false);
        assert_eq!(c.on_mouse_button(MouseButton::Right, false), vec![]);
        // after the drag turning is back to normal
        assert_eq!(c.on_key(KeyD, true), vec![Cmd::Move(9)]);
    }

    #[test]
    fn drag_end_stops_remapped_strafes_and_ctrl_switches_to_camera() {
        let mut c = ctl();
        c.on_mouse_button(MouseButton::Right, true);
        c.on_mouse_motion(50.0, 0.0);
        assert_eq!(c.on_key(KeyD, true), vec![Cmd::Move(5)]);
        assert_eq!(c.on_mouse_button(MouseButton::Right, false), vec![Cmd::Move(6)]);

        c.on_mouse_button(MouseButton::Right, true);
        c.on_key(ControlLeft, true);
        let out = norm(c.on_mouse_motion(10.0, 0.0));
        assert_eq!(out, vec![Cmd::Camera(CamCmd::Orbit { dx: 0.1, dy: 0.0 })]);
    }

    #[test]
    fn wheel_follows_pref() {
        let mut c = ctl();
        assert_eq!(c.on_wheel(2.0), vec![Cmd::Camera(CamCmd::Zoom(2.0))]);
        let mut c = Controls::new(ControlPrefs { mouse_wheel: 1, ..Default::default() });
        assert_eq!(c.on_wheel(2.0), vec![]);
    }

    #[test]
    fn saved_bindings_replace_defaults() {
        let xml = format!(
            r#"<Archive name="KeyBindings"><Array name="Bind"><Archive code="0"><Int32 name="Input" value="87" /><Int64 name="Provider" value="{}" /></Archive></Array></Archive>"#,
            provider_hash("MOVEMENT_JUMP")
        );
        let mut c = Controls::from_char_prefs(ControlPrefs::default(), &xml);
        assert_eq!(c.on_key(KeyW, true), vec![]); // 87 is 'F' in the client table, W is no longer bound
        assert_eq!(c.on_key(KeyF, true), vec![Cmd::Move(0xF)]);
        assert_eq!(c.on_key(Numpad5, true), vec![Cmd::Camera(CamCmd::Reset)]);
    }

    #[test]
    fn key_ids_follow_the_client_table() {
        assert_eq!(key_id(KeyA), Some(82));
        assert_eq!(key_id(KeyZ), Some(107));
        assert_eq!(key_id(Digit1), Some(108));
        assert_eq!(key_id(Digit0), Some(117));
        assert_eq!(key_id(F8), Some(46));
        assert_eq!(key_id(Numpad7), Some(77));
        assert_eq!(key_id(NumpadAdd), Some(69));
        assert_eq!(key_id(ArrowDown), Some(35));
        assert_eq!(key_id(Slash), Some(118));
        assert_eq!(key_id(F13), Some(51));
    }
}
