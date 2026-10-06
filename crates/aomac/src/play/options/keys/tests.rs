use super::*;
use crate::play::dvalue::{IndepPrefs, Kind};

fn shipped(rel: &str) -> Option<String> {
    std::fs::read_to_string(ao_gui::client_dir().join(rel)).ok()
}

fn labels(_: &str) -> String {
    String::new()
}

#[test]
fn archive_round_trips_in_the_original_format() {
    let mut b = Bindings::default();
    assert!(b.add(104, provider_hash("MOVEMENT_FORWARD_V3")));
    assert!(b.add(9, provider_hash("MOVEMENT_FORWARD_V3")));
    assert!(b.add(9, provider_hash("ACTION_SIT")));
    let text = b.archive();
    // inputs ascending, providers in binding order, `Int64 Provider` repeated inside one `Input` archive
    assert!(text.starts_with("<Archive name=\"KeyBindings\" code=\"0\"><Int32 name=\"Version\" value=\"7\" /><Array name=\"Bind\"><Archive code=\"0\"><Int32 name=\"Input\" value=\"9\" />"), "{text}");
    assert_eq!(text.matches("name=\"Provider\"").count(), 3);
    assert_eq!(Bindings::from_archive(&text), b);
}

#[test]
fn modifier_bits_and_release_flag_encode_like_the_hotkey_table() {
    // `CTRL + SHIFT + TAB`, `~ NUMPAD_4`
    assert_eq!(FIXED.iter().find(|e| e.0 == "KEY_PREV_FRIENDLY_TARGET").unwrap().1, 15 | id::CTRL | id::SHIFT);
    assert_eq!(FIXED.iter().find(|e| e.0 == "KEY_COMMAND_ROTATE_CAMERA_LEFT_RELEASE").unwrap().1, 74 | RELEASE);
    assert_eq!(key_name(74), Some("NUMPAD_4"));
    assert_eq!(key_name(0x20000 | 82), Some("A"));
    assert_eq!(key_name(0x80), None);
    assert_eq!(bind_text(82 | id::CTRL | id::ALT | id::SHIFT), "<font color=aqua>CTRL+ALT+SHIFT+A</font>");
    // the modifier keys themselves carry no prefix
    assert_eq!(bind_text(17 | id::SHIFT), "<font color=aqua>SHIFT</font>");
    assert_eq!(fixed_text(15 | id::CTRL, "none"), "CTRL+TAB");
    assert_eq!(fixed_text(0, "none"), "none");
    assert_eq!(fixed_text(126, "none"), "XButton2DoubleClick");
    assert_eq!(key_input(82, ao_gui::Modifiers { shift: true, ctrl: false, alt: true }), 82 | id::SHIFT | id::ALT);
    assert_eq!(key_input(18, ao_gui::Modifiers { ctrl: true, ..Default::default() }), 18);
}

/// A key may carry several providers and a provider several keys; the same pair twice is refused (`FUN_10018af9`); there is no conflict rule.
#[test]
fn conflicts_are_allowed_but_duplicates_are_not() {
    let (a, b) = (provider_hash("ACTION_SIT"), provider_hash("MOVEMENT_JUMP"));
    let mut t = Bindings::default();
    assert!(t.add(23, a));
    assert!(!t.add(23, a));
    assert!(t.add(23, b));
    assert_eq!(t.providers(23), [a, b]);
    assert!(t.add(105, a));
    assert_eq!(t.inputs(a), [23, 105]);
    assert!(t.has(b));
    assert!(t.remove(23, b));
    assert!(!t.remove(23, b));
    assert!(!t.has(b));
    // a key without providers is not saved
    assert!(Bindings::from_archive(&t.archive()).inputs(b).is_empty());
}

/// Every provider hash of the shipped `KeyBindings` is one of the registered providers (99 entries, none unknown), and the registry is ordered like the
/// original's `std::map<hash, Provider>`.
#[test]
fn shipped_archive_resolves_against_the_registry() {
    let Some(text) = shipped("cd_image/gui/Default/CharPrefs.xml") else { return };
    let b = Bindings::from_archive(&text);
    assert_eq!(b.version, VERSION);
    let reg = registry(&labels);
    assert_eq!(b.pairs().count(), 99);
    for (i, p) in b.pairs() {
        assert!(reg.iter().any(|r| r.hash == p), "input {i}: unknown provider {p}");
    }
    assert!(reg.windows(2).all(|w| w[0].hash < w[1].hash));
    assert_eq!(reg.len(), 70 + 10 * 13);
}

/// The fixed table equals the text GUI.dll parses (`ParseFile`: the `KEY_*` lines of the built-in hot key text).
#[test]
fn fixed_table_equals_the_gui_dll_text() {
    let Ok(dll) = std::fs::read(ao_gui::client_dir().join("GUI.dll")) else { return };
    let needle = b"  COMMAND_CAMP_GAME_TO_SYSTEM_ALTF4";
    let start = dll.windows(needle.len()).position(|w| w == needle).expect("hot key text");
    let end = start + dll[start..].iter().position(|&b| b == 0).unwrap();
    let text = String::from_utf8_lossy(&dll[start..end]).to_string();
    let mut parsed = vec![];
    for l in text.lines() {
        let Some(name) = l.split_whitespace().next().filter(|n| n.starts_with("KEY_")) else { continue };
        let (_, expr) = l.split_once('=').unwrap();
        let expr = expr.trim();
        let (rel, expr) = expr.strip_prefix('~').map_or((0, expr), |e| (RELEASE, e.trim()));
        let mut v = rel;
        for t in expr.split('+').map(str::trim) {
            v |= match t {
                "SHIFT" => id::SHIFT,
                "CTRL" => id::CTRL,
                "ALT" => id::ALT,
                k => (0..127u32).find(|i| NAMES[*i as usize] == k).unwrap_or_else(|| panic!("key {k}")),
            };
        }
        parsed.push((name.to_string(), v));
    }
    let ours: Vec<(String, u32)> = FIXED.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    assert_eq!(parsed, ours);
}

/// `KEY_*` ints of `Login.cfg` (`Preferences_t::LoadKeyboard`) override the fixed keys; the defaults are registered as `InitDefaultInt(.., 0, 999999999)`.
#[test]
fn login_cfg_overrides_the_fixed_keys() {
    let mut p = IndepPrefs::with_defaults();
    assert_eq!(FixedKeys::from_prefs(&p), FixedKeys::default());
    assert_eq!(p.int_range("KEY_NEXT_HOSTILE_TARGET", Kind::Login), Some((0, 999_999_999)));
    assert_eq!(FixedKeys::default().get("KEY_NEXT_HOSTILE_TARGET"), 15);
    p.load("KEY_NEXT_HOSTILE_TARGET 262159\r\n", Kind::Login);
    let f = FixedKeys::from_prefs(&p);
    assert_eq!(f.get("KEY_NEXT_HOSTILE_TARGET"), 15 | id::CTRL);
    assert_eq!(f.names(15 | id::CTRL).collect::<Vec<_>>(), ["KEY_NEXT_FRIENDLY_TARGET", "KEY_NEXT_HOSTILE_TARGET"]);
    assert!(p.save(Kind::Login).contains("KEY_NEXT_HOSTILE_TARGET 262159"));
}
