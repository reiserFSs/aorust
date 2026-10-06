//! The HUD's hot keys from the `KeyBindings` archive of the character prefs (`CharPrefs.xml` defaults, then the user's saved
//! `prefs/<account>/Char<id>/Prefs.xml`, both merged by the DValue store `play/dvalue.rs`). `Input` = client key id | SHIFT 0x20000 /
//! CTRL 0x40000 / ALT 0x80000, `Provider` = [`provider_hash`] of the provider name; the providers the HUD registers are
//! `ControlCenterModule_c::SetupProviders` (GUI 0x10068c38, the `WINDOW_*` ones) and `ShortcutBarContainer` (GUI 0x100d9a81,
//! `SHORTCUT_BAR_%d_%d`, `SHORTUCT_BAR_{SELECT,ROW,ACTIVE}_%d` -- the DLL spells it `SHORTUCT`). docs/gui.md §10.5 / §12.1.

use super::controls::{char_key_id, id, provider_hash};
use super::hud::WindowKind;
use ao_gui::xml;

/// A provider the HUD answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HudKey {
    /// `WINDOW_*`: toggles the window.
    Window(WindowKind),
    /// `SHORTUCT_BAR_ACTIVE_n` (default: the digit keys): use slot `n` of the active bar (`FUN_100d7cef`).
    BarActive(usize),
    /// `SHORTUCT_BAR_ROW_n` (default Shift + digit): show row `n` of the active bar (`FUN_100d85fb`).
    BarRow(usize),
    /// `SHORTUCT_BAR_SELECT_n` (default Alt + digit): bar `n` becomes the active one (`FUN_100d7cac`).
    BarSelect(usize),
    /// `SHORTCUT_BAR_<bar>_<slot>` (no default key): use that slot of that bar.
    BarSlot(usize, usize),
}

/// The window providers of `SetupProviders` that have a window here; `WINDOW_KNOWLEDGE`, `WINDOW_RAID`, ... have none (their keys are ignored).
const WINDOW_PROVIDERS: &[(&str, WindowKind)] = &[
    ("WINDOW_INVENTORY", WindowKind::Inventory),
    ("WINDOW_SKILLS", WindowKind::Skills),
    ("WINDOW_PLANETMAP", WindowKind::PlanetMap),
    ("WINDOW_WEAR", WindowKind::Character),
    ("WINDOW_SPECIALACTION", WindowKind::Actions),
    ("WINDOW_MISSION", WindowKind::Mission),
    ("WINDOW_TEAM", WindowKind::Team),
    ("WINDOW_MAP", WindowKind::Map),
    ("WINDOW_FRIENDS", WindowKind::Friends),
    ("WINDOW_NANO", WindowKind::Nano),
    ("WINDOW_STAT", WindowKind::Stat),
    ("WINDOW_NCU", WindowKind::Ncu),
];
/// Bars and slots the container registers providers for (`NumHotbars` max 10, 10 slots).
const BARS: usize = 10;
const SLOTS: usize = 10;

/// Provider hash -> what the HUD does.
fn providers() -> Vec<(u32, HudKey)> {
    let mut v: Vec<(u32, HudKey)> = WINDOW_PROVIDERS.iter().map(|(n, k)| (provider_hash(n), HudKey::Window(*k))).collect();
    for b in 0..BARS {
        v.push((provider_hash(&format!("SHORTUCT_BAR_ACTIVE_{b}")), HudKey::BarActive(b)));
        v.push((provider_hash(&format!("SHORTUCT_BAR_ROW_{b}")), HudKey::BarRow(b)));
        v.push((provider_hash(&format!("SHORTUCT_BAR_SELECT_{b}")), HudKey::BarSelect(b)));
        for s in 0..SLOTS {
            v.push((provider_hash(&format!("SHORTCUT_BAR_{b}_{s}")), HudKey::BarSlot(b, s)));
        }
    }
    v
}

#[derive(Default)]
pub(super) struct KeyMap {
    binds: Vec<(u32, HudKey)>,
}

impl KeyMap {
    /// The bindings of a `KeyBindings` archive (`<Array name="Bind">` of `<Archive>`s with `Int32 Input` and `Int64 Provider`) plus the fixed
    /// `KEY_OPEN_PERK_WINDOW` (Shift + P, the GUI.dll commands table).
    pub(super) fn from_archive(text: &str) -> Self {
        let table = providers();
        let mut binds = vec![(97 | id::SHIFT, HudKey::Window(WindowKind::Perks))];
        if let Ok(root) = xml::parse(text) {
            let archive = if root.attr("name") == Some("KeyBindings") { Some(&root) } else { root.children.iter().find(|c| c.name == "Archive" && c.attr("name") == Some("KeyBindings")) };
            if let Some(bind) = archive.and_then(|a| a.children.iter().find(|c| c.name == "Array" && c.attr("name") == Some("Bind"))) {
                for a in &bind.children {
                    let num = |n: &str| a.children.iter().find(|c| c.attr("name") == Some(n)).and_then(|c| c.attr("value")).and_then(|v| v.parse::<u64>().ok());
                    let (Some(input), Some(provider)) = (num("Input"), num("Provider")) else { continue };
                    if let Some((_, k)) = table.iter().find(|(h, _)| u64::from(*h) == provider) {
                        binds.push((input as u32, *k));
                    }
                }
            }
        }
        KeyMap { binds }
    }

    /// Everything the character key `c` with `mods` is bound to (several providers may share a key).
    pub(super) fn lookup(&self, c: char, mods: ao_gui::Modifiers) -> Vec<HudKey> {
        let Some(base) = char_key_id(c) else { return vec![] };
        let key = base | if mods.shift { id::SHIFT } else { 0 } | if mods.ctrl { id::CTRL } else { 0 } | if mods.alt { id::ALT } else { 0 };
        self.binds.iter().filter(|b| b.0 == key).map(|b| b.1).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_gui::Modifiers;

    fn shipped() -> Option<String> {
        std::fs::read_to_string(ao_gui::client_dir().join("cd_image/gui/Default/CharPrefs.xml")).ok()
    }

    /// The shipped `KeyBindings`: windows (I, U, P, Ctrl+1..0, the Ctrl+2 Actions window), the digit keys use slots of the active bar, Shift+digit
    /// scrolls the row, Alt+digit selects the bar (provider hashes of `SHORTUCT_BAR_{ACTIVE,ROW,SELECT}_n` found in CharPrefs.xml).
    #[test]
    fn shipped_bindings_resolve_windows_and_bar_keys() {
        let Some(text) = shipped() else { return };
        let m = KeyMap::from_archive(&text);
        let none = Modifiers::default();
        let ctrl = Modifiers { ctrl: true, ..none };
        assert_eq!(m.lookup('i', none), [HudKey::Window(WindowKind::Inventory)]);
        assert_eq!(m.lookup('u', none), [HudKey::Window(WindowKind::Skills)]);
        assert_eq!(m.lookup('p', none), [HudKey::Window(WindowKind::PlanetMap)]);
        assert_eq!(m.lookup('p', Modifiers { shift: true, ..none }), [HudKey::Window(WindowKind::Perks)]);
        assert_eq!(m.lookup('1', ctrl), [HudKey::Window(WindowKind::Character)]);
        assert_eq!(m.lookup('2', ctrl), [HudKey::Window(WindowKind::Actions)]);
        assert_eq!(m.lookup('6', ctrl), [HudKey::Window(WindowKind::Map)]);
        assert_eq!(m.lookup('0', ctrl), [HudKey::Window(WindowKind::Ncu)]);
        assert_eq!(m.lookup('3', ctrl), [], "Knowledge has no window");
        assert_eq!(m.lookup('1', none), [HudKey::BarActive(0)]);
        assert_eq!(m.lookup('0', none), [HudKey::BarActive(9)]);
        assert_eq!(m.lookup('4', Modifiers { shift: true, ..none }), [HudKey::BarRow(3)]);
        assert_eq!(m.lookup('5', Modifiers { alt: true, ..none }), [HudKey::BarSelect(4)]);
        assert_eq!(m.lookup('q', none), [], "movement / combat keys belong to controls.rs");
    }

    /// A saved archive rebinding the Inventory window to J and a hotbar slot to K replaces the defaults it names; unknown providers are ignored.
    #[test]
    fn saved_bindings_override() {
        let j = u64::from(provider_hash("WINDOW_INVENTORY"));
        let k = u64::from(provider_hash("SHORTCUT_BAR_0_3"));
        let xml = format!(
            "<Archive name=\"KeyBindings\" code=\"0\"><Int32 name=\"Version\" value=\"7\"/><Array name=\"Bind\">\
             <Archive code=\"0\"><Int32 name=\"Input\" value=\"91\"/><Int64 name=\"Provider\" value=\"{j}\"/></Archive>\
             <Archive code=\"0\"><Int32 name=\"Input\" value=\"92\"/><Int64 name=\"Provider\" value=\"{k}\"/></Archive>\
             <Archive code=\"0\"><Int32 name=\"Input\" value=\"93\"/><Int64 name=\"Provider\" value=\"12345\"/></Archive></Array></Archive>"
        );
        let m = KeyMap::from_archive(&xml);
        let none = Modifiers::default();
        assert_eq!(m.lookup('j', none), [HudKey::Window(WindowKind::Inventory)]);
        assert_eq!(m.lookup('k', none), [HudKey::BarSlot(0, 3)]);
        assert_eq!(m.lookup('i', none), [], "the default I binding is gone");
        assert_eq!(m.lookup('l', none), []);
    }
}
