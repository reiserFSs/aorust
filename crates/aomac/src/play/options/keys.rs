//! The key binding model of the options window's "Fixed keys" / "Key bindings" pages: the client's input ids, the `KeyBindings` archive, the provider
//! registry and the fixed hot keys. GUI.dll addresses: `InputConfig_t` (`SetDefaultHotkeys` 0x1001ad88, `ParseFile` 0x1001ac19, `CreateHotkey` 0x1001a80a,
//! `GetInputModeName` 0x10019c77, `ChangeHotkeyKeystroke` 0x1001a6ba), the binding manager singleton `FUN_10018b7c` (0x28 bytes: provider map +4, input map +0x14,
//! `Version` +0x20, capture flag +0x24) and `Preferences_t::LoadKeyboard` 0x1012488b. docs/gui.md "Hot key pages".

use super::super::controls::id;
use std::collections::BTreeMap;

/// `Hotkey_t` / binding input = key id (low 17 bits, `GetInputModeName`: `input & 0x1ffff`) | SHIFT 0x20000 | CTRL 0x40000 | ALT 0x80000; bit 0x100000 = key
/// *release* (`~ KEY` in the table, `FUN_100180fe`: the release ids 4 / 8 / 0xc / 0x7b / 0x7c are folded onto the press id 1 / 5 / 9 / 0x79 / 0x7a with this flag).
#[cfg(test)]
pub const RELEASE: u32 = 0x10_0000;
pub const KEY_MASK: u32 = 0x1_ffff;

/// `InputConfig_t::m_asInputMethodIDTable` (GUI.dll 0x10262e20 region: 127 `{char* name, int id}` pairs, **indexed by position**: `GetInputModeName` reads
/// `table[(input & 0x1ffff) * 2]`, so the mouse entries 2/3, 6/7 and 10/11 show the names of their neighbours exactly like the original).
const NAMES: [&str; 127] = [
    "Inactive",
    "LeftPress",
    "LeftDoubleClick",
    "LeftPressHold",
    "LeftRelease",
    "RightPress",
    "RightDoubleClick",
    "RightPressHold",
    "RightRelease",
    "MiddlePress",
    "MiddleDoubleClick",
    "MiddlePressHold",
    "MiddleRelease",
    "MouseWheelMovement",
    "ESC",
    "TAB",
    "CAPS_LOCK",
    "SHIFT",
    "CTRL",
    "ALT",
    "SHIFT_RIGHT",
    "CTRL_RIGHT",
    "ALT_RIGHT",
    "SPACE",
    "ENTER",
    "BACKSPACE",
    "INSERT",
    "DELETE",
    "HOME",
    "END",
    "PGUP",
    "PGDN",
    "CursorLeft",
    "CursorRight",
    "CursorUp",
    "CursorDown",
    "PIPE",
    "PERIOD",
    "COMMA",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "F13",
    "F14",
    "F15",
    "F16",
    "F17",
    "F18",
    "F19",
    "F20",
    "F21",
    "F22",
    "F23",
    "F24",
    "SCROLL",
    "PAUSE",
    "NUMPAD_NUMLOCK",
    "NUMPAD_DIV",
    "NUMPAD_MUL",
    "NUMPAD_SUB",
    "NUMPAD_ADD",
    "NUMPAD_0",
    "NUMPAD_1",
    "NUMPAD_2",
    "NUMPAD_3",
    "NUMPAD_4",
    "NUMPAD_5",
    "NUMPAD_6",
    "NUMPAD_7",
    "NUMPAD_8",
    "NUMPAD_9",
    "NUMPAD_DECIMAL",
    "AnyTextKey",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "0",
    "SLASH",
    "MouseMove",
    "MouseMoveDelta",
    "XButton1Press",
    "XButton2Press",
    "XButton1Release",
    "XButton2Release",
    "XButton1DoubleClick",
    "XButton2DoubleClick",
];

/// `InputConfig_t::GetInputModeName`: `None` for ids >= 0x80 (and for the table's id -1 terminator).
pub fn key_name(input: u32) -> Option<&'static str> {
    NAMES.get((input & KEY_MASK) as usize).copied()
}

/// Modifier prefix `FUN_1001888d`: `CTRL+`, `ALT+`, `SHIFT+` in this order, none for the modifier keys themselves (ids 0x11..=0x16).
fn prefix(input: u32) -> String {
    let key = input & KEY_MASK;
    let mut s = String::new();
    if !(0x11..=0x16).contains(&key) {
        for (bit, t) in [(id::CTRL, "CTRL+"), (id::ALT, "ALT+"), (id::SHIFT, "SHIFT+")] {
            if input & bit != 0 {
                s += t;
            }
        }
    }
    s
}

/// The Key column / the bind dialog's key text (`FUN_100bbf5d`, `FUN_100bd441`): `<font color=aqua>CTRL+ALT+SHIFT+NAME</font>`.
pub fn bind_text(input: u32) -> String {
    format!("<font color=aqua>{}{}</font>", prefix(input), key_name(input).unwrap_or(""))
}

/// The key of a "Fixed keys" row (`FUN_100bdf77`): the modifiers always, `?` for an unnamed id, the "NoKey" text for 0.
pub fn fixed_text(input: u32, no_key: &str) -> String {
    if input == 0 {
        return no_key.to_string();
    }
    let mut s = String::new();
    for (bit, t) in [(id::CTRL, "CTRL+"), (id::ALT, "ALT+"), (id::SHIFT, "SHIFT+")] {
        if input & bit != 0 {
            s += t;
        }
    }
    s + key_name(input).unwrap_or("?")
}

/// The built-in hot key text of GUI.dll (`SetDefaultHotkeys` -> `ParseFile`, 0x101aab18..) restricted to the lines with a `KEY_*` id (the only ones
/// `SetDefaultLoginPrefs` registers an `IndependentPrefs` int for, name = the id, default = this input): `Login.cfg` entries of the same name override
/// them at login (`Preferences_t::LoadKeyboard`). The consumers read [`FixedKeys`].
pub const FIXED: &[(&str, u32)] = &[
    ("KEY_TOGGLE_CONTROL_CENTER", 0x20024),
    ("KEY_NEXT_FRIENDLY_TARGET", 0x4000f),
    ("KEY_PREV_FRIENDLY_TARGET", 0x6000f),
    ("KEY_NEXT_HOSTILE_TARGET", 0xf),
    ("KEY_PREV_HOSTILE_TARGET", 0x2000f),
    ("KEY_COMMAND_ROTATE_CAMERA_LEFT", 0x4a),
    ("KEY_COMMAND_ROTATE_CAMERA_LEFT_RELEASE", 0x10004a),
    ("KEY_COMMAND_ROTATE_CAMERA_RIGHT", 0x4c),
    ("KEY_COMMAND_ROTATE_CAMERA_RIGHT_RELEASE", 0x10004c),
    ("KEY_COMMAND_ROTATE_CAMERA_UP", 0x48),
    ("KEY_COMMAND_ROTATE_CAMERA_UP_RELEASE", 0x100048),
    ("KEY_COMMAND_ROTATE_CAMERA_DOWN", 0x4e),
    ("KEY_COMMAND_ROTATE_CAMERA_DOWN_RELEASE", 0x10004e),
    ("KEY_COMMAND_ZOOM_CAMERA_IN", 0x45),
    ("KEY_COMMAND_ZOOM_CAMERA_IN_RELEASE", 0x100045),
    ("KEY_COMMAND_ZOOM_CAMERA_OUT", 0x44),
    ("KEY_COMMAND_ZOOM_CAMERA_OUT_RELEASE", 0x100044),
    ("KEY_COMMAND_CAMERA_SET_PREFERRED_POS", 0x4d),
    ("KEY_COMMAND_CAMERA_RESET", 0x4b),
    ("KEY_COMMAND_CHAT_HISTORY_PAGE_UP", 0x1e),
    ("KEY_COMMAND_CHAT_HISTORY_PAGE_DOWN", 0x1f),
    ("KEY_COMMAND_START_CHAT_INPUT", 0x18),
    ("KEY_COMMAND_START_CHAT_CMD_INPUT", 0x76),
    ("KEY_COMMAND_START_CHAT_REPLY_INPUT", 0x20063),
    ("KEY_COMMAND_DEBUG_EXT_POSTOCHAT", 0x2002f),
    ("KEY_COMMAND_DEBUG_POSTOCHAT", 0x2f),
    ("KEY_OPEN_PERK_WINDOW", 0x20061),
    ("KEY_OPEN_VEHICLE_WINDOW", 0x20067),
];

/// The fixed hot keys in effect: the `Login.cfg` ints over the [`FIXED`] defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct FixedKeys(Vec<(&'static str, u32)>);

impl Default for FixedKeys {
    fn default() -> Self {
        FixedKeys(FIXED.to_vec())
    }
}

impl FixedKeys {
    /// `LoadKeyboard`: every `KEY_*` login int replaces the default key.
    pub fn from_prefs(p: &super::super::dvalue::IndepPrefs) -> Self {
        use super::super::dvalue::Kind;
        FixedKeys(FIXED.iter().map(|&(n, d)| (n, p.get_int(n, Kind::Login).map_or(d, |v| v as u32))).collect())
    }

    /// `InputConfig_t::GetHotkey(id)+8`; 0 for an unknown name.
    pub fn get(&self, name: &str) -> u32 {
        self.0.iter().find(|e| e.0 == name).map_or(0, |e| e.1)
    }

    /// The names bound to `input` (`KEY_*` lines are unique per name, not per key).
    #[cfg(test)]
    pub fn names(&self, input: u32) -> impl Iterator<Item = &'static str> + '_ {
        self.0.iter().filter(move |e| e.1 == input).map(|e| e.0)
    }
}

/// `provider_hash` of `FUN_10019132`: the `KeyBindings` archive stores the hash of the provider's name.
pub use super::super::controls::provider_hash;

/// One registered provider (`FUN_10018788(name, label, group, mask, variant, flag, flag)`): `label` / `group` are `#Key`s of the text categories 2011 / 2012 (`FUN_100bd48b`) or literals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub name: String,
    pub label: String,
    pub group: String,
    pub hash: u32,
}

/// `(name, label, group)` of every provider the GUI registers (call sites of 0x10018788: `ControlCenterModule_c::SetupProviders` 0x10068c38,
/// `FlowControlModule_t` 0x1002abd6, `TargetingModule_t` 0x10026322, `TeamViewModule_c` 0x1007b9be, `TextInputModule_t` 0x100220aa, `BrowserModule_c`
/// 0x1010bcbf, `ShortcutBarContainer` 0x100d9a81). The shortcut-bar ones depend on `NumHotbars` ([`registry`]).
const REGISTRY: &[(&str, &str, &str)] = &[
    ("TEXT_COPYTOCLIPBOARD", "#CopyToClip", "#Text"),
    ("TARGET_SELECTSELF", "#TargetSelectSelf", "#Target"),
    ("TARGET_SELECTPET0", "#TargetSelectPet0", "#Target"),
    ("TARGET_SELECTPET1", "#TargetSelectPet1", "#Target"),
    ("TARGET_SELECTPET2", "#TargetSelectPet2", "#Target"),
    ("TARGET_SELECTPET3", "#TargetSelectPet3", "#Target"),
    ("TARGET_SELECTPET4", "#TargetSelectPet4", "#Target"),
    ("TARGET_SELECTPET5", "#TargetSelectPet5", "#Target"),
    ("MOVEMENT_FORWARD_GLOBAL_V2", "#MoveForwardGlobal", "#Movement"),
    ("MOVEMENT_BACK_GLOBAL_V2", "#MoveBackGlobal", "#Movement"),
    ("MOVEMENT_LEFT_GLOBAL_V2", "#MoveLeftGlobal", "#Movement"),
    ("MOVEMENT_RIGHT_GLOBAL_V2", "#MoveRightGlobal", "#Movement"),
    ("MOVEMENT_FORWARD_V3", "#MoveForward", "#Movement"),
    ("MOVEMENT_BACK_V2", "#MoveBack", "#Movement"),
    ("MOVEMENT_LEFT_V2", "#MoveLeft", "#Movement"),
    ("MOVEMENT_RIGHT_V2", "#MoveRight", "#Movement"),
    ("MOVEMENT_AUTORUN", "#MoveAutoRun", "#Movement"),
    ("MOVEMENT_STRAFELEFT", "#MoveStrafeLeft", "#Movement"),
    ("MOVEMENT_STRAFERIGHT", "#MoveStrafeRight", "#Movement"),
    ("MOVEMENT_JUMP", "#MoveJump", "#Movement"),
    ("MOVEMENT_TOGGLEWALK", "#MoveToggleWalk", "#Movement"),
    ("CAMERA_TOGGLE3RD", "#CameraToggle3rdPerson", "#Camera"),
    ("CAMERA_NEXTVIEW", "#CameraNextView", "#Camera"),
    ("CAMERA_PREVVIEW", "#CameraPrevView", "#Camera"),
    ("CAMERA_SCREENSHOT", "#CameraScreenshot", "#Camera"),
    ("ACTION_PICKUPITEM", "#ActionPickupItem", "#Action"),
    ("WINDOW_INVENTORY", "#WindowInventory", "#Window"),
    ("WINDOW_SKILLS", "#WindowSkills", "#Window"),
    ("WINDOW_PLANETMAP", "#WindowPlanetMap", "#Window"),
    ("WINDOW_SHORTCUTBAR", "#WindowShortcutBar", "#Window"),
    ("WINDOW_LFT", "#WindowLFT", "#Window"),
    ("WINDOW_RESEARCH", "#WindowResearch", "#Window"),
    ("WINDOW_RAID", "#WindowRaid", "#Window"),
    ("WINDOW_OPTIONS", "#WindowOptions", "#Window"),
    ("WINDOW_WEAR", "#WindowWear", "#Window"),
    ("WINDOW_SPECIALACTION", "#WindowSpecialAction", "#Window"),
    ("WINDOW_KNOWLEDGE", "#WindowKnowledge", "#Window"),
    ("WINDOW_MISSION", "#WindowMission", "#Window"),
    ("WINDOW_TEAM", "#WindowTeam", "#Window"),
    ("WINDOW_MAP", "#WindowMap", "#Window"),
    ("WINDOW_FRIENDS", "#WindowFriends", "#Window"),
    ("WINDOW_NANO", "#WindowNano", "#Window"),
    ("WINDOW_NCU", "#WindowNCU", "#Window"),
    ("WINDOW_STAT", "#WindowStat", "#Window"),
    ("WINDOW_TS", "#WindowTS", "#Window"),
    ("WINDOW_DL", "Daily Login", "#Window"),
    ("ACTION_ATTACK", "#ActionAttack", "#Action"),
    ("ACTION_USE", "#ActionUse", "#Action"),
    ("ACTION_SNEAK", "#ActionSneak", "#Action"),
    ("ACTION_SIT", "#ActionSit", "#Action"),
    ("ACTION_SNEAKATTACK", "#ActionSneakAttack", "#Action"),
    ("ACTION_FULLAUTO", "#ActionFullAuto", "#Action"),
    ("ACTION_BURST", "#ActionBurst", "#Action"),
    ("ACTION_BRAWL", "#ActionBrawl", "#Action"),
    ("ACTION_RELOAD", "#ActionReload", "#Action"),
    ("ACTION_FASTATTACK", "#ActionFastAttack", "#Action"),
    ("ACTION_DIMACH", "#ActionDimach", "#Action"),
    ("ACTION_FLINGSHOT", "#ActionFlingShot", "#Action"),
    ("ACTION_AIMEDSHOT", "#ActionAimedShot", "#Action"),
    ("ACTION_BOWSPECIALATTACK", "#ActionBowSpecialAttack", "#Action"),
    ("ACTION_CREATEREF", "#ActionCreateRef", "#Action"),
    ("ACTION_LOOKAT", "#ActionLookAt", "#Action"),
    ("ACTION_SWITCHTARGET", "#ActionSwitchTarget", "#Action"),
    ("TARGET_SELECTTEAMMEMBER1", "#TargetSelectTeamMember1", "#Target"),
    ("TARGET_SELECTTEAMMEMBER2", "#TargetSelectTeamMember2", "#Target"),
    ("TARGET_SELECTTEAMMEMBER3", "#TargetSelectTeamMember3", "#Target"),
    ("TARGET_SELECTTEAMMEMBER4", "#TargetSelectTeamMember4", "#Target"),
    ("TARGET_SELECTTEAMMEMBER5", "#TargetSelectTeamMember5", "#Target"),
    ("WINDOW_BROWSER", "#WindowBrowser", "#Window"),
    ("WINDOW_ITEMSHOP", "#WindowItemShop", "#Window"),
];

/// The hotbars the shortcut container registers providers for (`NumHotbars` max 10) and their slots.
pub const BARS: usize = 10;
pub const SLOTS: usize = 10;

/// Every provider in the order of the original's `std::map<hash, Provider>` (ascending hash = the order of the "Key bindings" list).
/// The shortcut-bar entries (`FUN_100d9a81`): `SHORTCUT_BAR_%d_%d` (label text `ShortcutBarKey`), `SHORTUCT_BAR_{SELECT,ROW,ACTIVE}_%d` (the DLL spells it
/// `SHORTUCT`), group `#ShortcutBar`; the label is the text formatted with the bar (and slot) number.
pub fn registry(texts: &dyn Fn(&str) -> String) -> Vec<Provider> {
    let mut v: Vec<Provider> = REGISTRY.iter().map(|&(n, l, g)| Provider { name: n.into(), label: l.into(), group: g.into(), hash: provider_hash(n) }).collect();
    let group = "#ShortcutBar";
    for b in 0..BARS {
        for s in 0..SLOTS {
            let n = format!("SHORTCUT_BAR_{b}_{s}");
            v.push(Provider { hash: provider_hash(&n), name: n, label: texts(&format!("ShortcutBarKey {} {}", b + 1, s + 1)), group: group.into() });
        }
        for (kind, key) in [("SELECT", "ShortcutBarSelect"), ("ROW", "ShortcutBarRow"), ("ACTIVE", "ShortcutBarActive")] {
            let n = format!("SHORTUCT_BAR_{kind}_{b}");
            v.push(Provider { hash: provider_hash(&n), name: n, label: texts(&format!("{key} {}", b + 1)), group: group.into() });
        }
    }
    v.sort_by_key(|p| p.hash);
    v
}

/// `Version` of the shipped `KeyBindings` archive (`CharPrefs.xml`, the manager's +0x20).
pub const VERSION: i32 = 7;

/// The binding table (manager +0x14: `std::map<input, vector<provider hash>>`) and `Version`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bindings {
    pub version: i32,
    binds: BTreeMap<u32, Vec<u32>>,
}

impl Default for Bindings {
    fn default() -> Self {
        Bindings { version: VERSION, binds: BTreeMap::new() }
    }
}

impl Bindings {
    /// `FUN_10018bbf` (load): the `Bind` array of a `KeyBindings` archive (`<Archive name="KeyBindings">` itself or a `<Root>` holding it): per entry
    /// `Int32 Input` and any number of `Int64 Provider` (a key may carry several providers). Unreadable text gives the empty table.
    pub fn from_archive(text: &str) -> Self {
        let mut b = Bindings::default();
        let Ok(root) = ao_gui::xml::parse(text) else { return b };
        let a = if root.attr("name") == Some("KeyBindings") { Some(&root) } else { root.children.iter().find(|c| c.name == "Archive" && c.attr("name") == Some("KeyBindings")) };
        let Some(a) = a else { return b };
        let num = |e: &ao_gui::xml::Element| e.attr("value").and_then(|v| v.trim().parse::<i64>().ok());
        if let Some(v) = a.children.iter().find(|c| c.attr("name") == Some("Version")).and_then(num) {
            b.version = v as i32;
        }
        for bind in a.children.iter().filter(|c| c.name == "Array" && c.attr("name") == Some("Bind")).flat_map(|c| &c.children) {
            let Some(input) = bind.children.iter().find(|c| c.attr("name") == Some("Input")).and_then(num) else { continue };
            for p in bind.children.iter().filter(|c| c.attr("name") == Some("Provider")).filter_map(num) {
                b.add(input as u32, p as u32);
            }
        }
        b
    }

    /// `FUN_10018498`: `Version`, then per input (ascending) with at least one provider an `<Archive>` with `Input` and the `Provider`s in order.
    pub fn archive(&self) -> String {
        let mut s = format!("<Archive name=\"KeyBindings\" code=\"0\"><Int32 name=\"Version\" value=\"{}\" /><Array name=\"Bind\">", self.version);
        for (input, ps) in self.binds.iter().filter(|(_, p)| !p.is_empty()) {
            s += &format!("<Archive code=\"0\"><Int32 name=\"Input\" value=\"{input}\" />");
            for p in ps {
                s += &format!("<Int64 name=\"Provider\" value=\"{p}\" />");
            }
            s += "</Archive>";
        }
        s + "</Array></Archive>"
    }

    /// `FUN_10018af9`: adds `provider` to `input` unless it is already there (a key may drive several providers: the original has no conflict check). True when added.
    pub fn add(&mut self, input: u32, provider: u32) -> bool {
        let v = self.binds.entry(input).or_default();
        if v.contains(&provider) {
            return false;
        }
        v.push(provider);
        true
    }

    /// `FUN_10018b3a`: removes `provider` from `input`. True when it was bound.
    pub fn remove(&mut self, input: u32, provider: u32) -> bool {
        let Some(v) = self.binds.get_mut(&input) else { return false };
        let n = v.len();
        v.retain(|p| *p != provider);
        let removed = v.len() != n;
        if v.is_empty() {
            self.binds.remove(&input); // an input without providers is not saved (`FUN_10018498`), so it does not exist
        }
        removed
    }

    /// `FUN_10018708`: the inputs bound to `provider`, ascending.
    pub fn inputs(&self, provider: u32) -> Vec<u32> {
        self.binds.iter().filter(|(_, p)| p.contains(&provider)).map(|(i, _)| *i).collect()
    }

    /// `FUN_10018074`: the provider has at least one key.
    pub fn has(&self, provider: u32) -> bool {
        self.binds.values().any(|p| p.contains(&provider))
    }

    /// The providers `input` triggers, in binding order (`FUN_100180fe` calls them all).
    #[cfg(test)]
    pub fn providers(&self, input: u32) -> &[u32] {
        self.binds.get(&input).map_or(&[], Vec::as_slice)
    }

    /// Every `(input, provider)` pair.
    pub fn pairs(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.binds.iter().flat_map(|(i, ps)| ps.iter().map(move |p| (*i, *p)))
    }
}

/// The input a physical key makes (`FUN_100bc5cf`, the bind dialog's key slot): the key id with ALT / CTRL / SHIFT from the modifier state; the modifier
/// keys themselves (ids 0x11..=0x16) stay bare.
pub fn key_input(key: u32, mods: ao_gui::Modifiers) -> u32 {
    if (0x11..=0x16).contains(&key) {
        return key;
    }
    key | if mods.alt { id::ALT } else { 0 } | if mods.ctrl { id::CTRL } else { 0 } | if mods.shift { id::SHIFT } else { 0 }
}

#[cfg(test)]
mod tests;
