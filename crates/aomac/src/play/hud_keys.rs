//! The HUD's hot keys from the shared key binding table (`options/keys.rs`: the `KeyBindings` archive of the character prefs, `CharPrefs.xml` defaults,
//! then the user's saved `prefs/<account>/Char<id>/Prefs.xml`, both merged by the DValue store `play/dvalue.rs`, edited by the options window's "Key bindings" page). `Input` = client key id | SHIFT 0x20000 /
//! CTRL 0x40000 / ALT 0x80000, `Provider` = [`provider_hash`] of the provider name; the providers the HUD registers are
//! `ControlCenterModule_c::SetupProviders` (GUI 0x10068c38, the `WINDOW_*` ones) and `ShortcutBarContainer` (GUI 0x100d9a81,
//! `SHORTCUT_BAR_%d_%d`, `SHORTUCT_BAR_{SELECT,ROW,ACTIVE}_%d` -- the DLL spells it `SHORTUCT`). docs/gui.md §10.5 / §12.1.

use super::controls::provider_hash;
use super::hud::WindowKind;
use super::options::keys::{Bindings, FixedKeys};

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
    ("WINDOW_OPTIONS", WindowKind::Options),
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
    /// The HUD's providers of the shared table plus the fixed `KEY_OPEN_PERK_WINDOW` (`Login.cfg` key, default Shift + P, the GUI.dll commands table).
    pub(super) fn new(b: &Bindings, f: &FixedKeys) -> Self {
        let table = providers();
        let mut binds = vec![(f.get("KEY_OPEN_PERK_WINDOW"), HudKey::Window(WindowKind::Perks))];
        binds.extend(b.pairs().filter_map(|(i, p)| Some((i, table.iter().find(|(h, _)| *h == p)?.1))));
        KeyMap { binds }
    }

    /// Everything the input (`key id | modifier bits`) is bound to (several providers may share a key).
    pub(super) fn lookup(&self, input: u32) -> Vec<HudKey> {
        self.binds.iter().filter(|b| b.0 == input).map(|b| b.1).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::controls::{char_key_id, id};

    fn shipped() -> Option<String> {
        std::fs::read_to_string(ao_gui::client_dir().join("cd_image/gui/Default/CharPrefs.xml")).ok()
    }

    fn map(text: &str) -> KeyMap {
        KeyMap::new(&Bindings::from_archive(text), &FixedKeys::default())
    }

    fn k(c: char, mods: u32) -> u32 {
        char_key_id(c).unwrap() | mods
    }

    /// The shipped `KeyBindings`: windows (I, U, P, Ctrl+1..0, the Ctrl+2 Actions window, F10 options), the digit keys use slots of the active bar, Shift+digit
    /// scrolls the row, Alt+digit selects the bar (provider hashes of `SHORTUCT_BAR_{ACTIVE,ROW,SELECT}_n` found in CharPrefs.xml).
    #[test]
    fn shipped_bindings_resolve_windows_and_bar_keys() {
        let Some(text) = shipped() else { return };
        let m = map(&text);
        assert_eq!(m.lookup(k('i', 0)), [HudKey::Window(WindowKind::Inventory)]);
        assert_eq!(m.lookup(k('u', 0)), [HudKey::Window(WindowKind::Skills)]);
        assert_eq!(m.lookup(k('p', 0)), [HudKey::Window(WindowKind::PlanetMap)]);
        assert_eq!(m.lookup(k('p', id::SHIFT)), [HudKey::Window(WindowKind::Perks)]);
        assert_eq!(m.lookup(k('1', id::CTRL)), [HudKey::Window(WindowKind::Character)]);
        assert_eq!(m.lookup(k('2', id::CTRL)), [HudKey::Window(WindowKind::Actions)]);
        assert_eq!(m.lookup(k('6', id::CTRL)), [HudKey::Window(WindowKind::Map)]);
        assert_eq!(m.lookup(k('0', id::CTRL)), [HudKey::Window(WindowKind::Ncu)]);
        assert_eq!(m.lookup(k('3', id::CTRL)), [], "Knowledge has no window");
        assert_eq!(m.lookup(48), [HudKey::Window(WindowKind::Options)], "F10");
        assert_eq!(m.lookup(k('1', 0)), [HudKey::BarActive(0)]);
        assert_eq!(m.lookup(k('0', 0)), [HudKey::BarActive(9)]);
        assert_eq!(m.lookup(k('4', id::SHIFT)), [HudKey::BarRow(3)]);
        assert_eq!(m.lookup(k('5', id::ALT)), [HudKey::BarSelect(4)]);
        assert_eq!(m.lookup(k('q', 0)), [], "movement / combat keys belong to controls.rs");
    }

    /// A saved archive rebinding the Inventory window to J and a hotbar slot to F5 replaces the defaults it names (any key, not only letters); unknown providers are ignored.
    #[test]
    fn saved_bindings_override() {
        let j = u64::from(provider_hash("WINDOW_INVENTORY"));
        let slot = u64::from(provider_hash("SHORTCUT_BAR_0_3"));
        let xml = format!(
            "<Archive name=\"KeyBindings\" code=\"0\"><Int32 name=\"Version\" value=\"7\"/><Array name=\"Bind\">\
             <Archive code=\"0\"><Int32 name=\"Input\" value=\"91\"/><Int64 name=\"Provider\" value=\"{j}\"/></Archive>\
             <Archive code=\"0\"><Int32 name=\"Input\" value=\"43\"/><Int64 name=\"Provider\" value=\"{slot}\"/></Archive>\
             <Archive code=\"0\"><Int32 name=\"Input\" value=\"93\"/><Int64 name=\"Provider\" value=\"12345\"/></Archive></Array></Archive>"
        );
        let m = map(&xml);
        assert_eq!(m.lookup(k('j', 0)), [HudKey::Window(WindowKind::Inventory)]);
        assert_eq!(m.lookup(43), [HudKey::BarSlot(0, 3)]);
        assert_eq!(m.lookup(k('i', 0)), [], "the default I binding is gone");
        assert_eq!(m.lookup(k('l', 0)), []);
    }
}
