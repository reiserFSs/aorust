//! Consumers of the options that have no window of their own: `MiscOptionsMonitor_c` (GUI.dll ctor `FUN_100bff11`, created by `OptionPanelModule_c::Initialize` 0xc2a58 and
//! living for the whole session) and `SoundOptionsMonitor_c` (0x100c4cc9). docs/gui.md "Options window".
//!
//! * `ToggleAllEffects` (observer `FUN_100bfa27`): every change sets the 13 effect prefs (`RealisticWater`, `Wildlife`, `IsSpaceShipsShown` (char prefs), `RealisticClouds`,
//!   `SimpleClouds`, `Shadows`, `BuffsFX`, `TracersFX`, `NanoEffectFX`, `MuzzleFlashFX`, `EnvironmentFX`, `OthersFX`, `SmoothAnimations`) to its bool (`SetPrefInt(name, b, type, true)`).
//! * `AutoTargetMOB` / `AutoTargetPvP` / `DisableXPGain` are bits 1 / 3 / 4 of the own stat `0x15d` (read once at construction); a change runs
//!   `N3Msg_EventFeedback(0x3c, {0,0}, {0, stat with the bit set / cleared})` (`FUN_100bfb7e` / `100bfc3a` / `100bfbdc`), Gamecode 0x1001cbb4: a new value sends a
//!   `CharacterActionIIR_t` of action `0xa5` (identity b `{0, value}`) and stores it in the stat.
//! * `VisualFlags` = the whole stat `0x2a1`, `VisiblePVPTitle` = `stat >> 7 & 3`; a change (`FUN_100bfd64` / `FUN_100bfe01`, the title with `stat & !0x180 | (v & 3) << 7`) runs
//!   `N3Msg_EventFeedback(0x46, ..)`: the same message with action `0xa6` (the one `/rp` sends). The branch "bit 5 changes while fighting -> `Feedback_CantDoThisWhileFighting`, the
//!   DValue falls back to the stat" is not ported (the fighting state `+0x1d4 + 0x44` of the control dynel is not tracked here).
//! * `OpenNanoWindow`: while true the slot `FUN_100bfc98` is connected to `GlobalSignals+0x10c` and sets the DValue `nano_window` (opens the Programs window) -- the emitter of that
//!   signal is UNRESOLVED, see docs.

use super::super::dvalue::{DValues, Kind, Variant, CAT_VARIABLES};
use super::super::zone::Zone;
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::action::{character_action, simple};
use ao_net::n3::outgoing::n3_frame;

/// `FUN_100bfa27`: `(name, char prefs)`.
pub const EFFECT_PREFS: [(&str, bool); 13] = [
    ("RealisticWater", false),
    ("Wildlife", false),
    ("IsSpaceShipsShown", true),
    ("RealisticClouds", false),
    ("SimpleClouds", false),
    ("Shadows", false),
    ("BuffsFX", false),
    ("TracersFX", false),
    ("NanoEffectFX", false),
    ("MuzzleFlashFX", false),
    ("EnvironmentFX", false),
    ("OthersFX", false),
    ("SmoothAnimations", false),
];

/// Own stat of the server-side option flags (`N3Msg_GetSkill(0x15d, 2)`).
pub const STAT_OPTION_FLAGS: u32 = 0x15d;
/// Own stat `VisualFlags`.
pub const STAT_VISUAL_FLAGS: u32 = 0x2a1;
/// `(DValue, bit of stat 0x15d)`.
const FLAG_BITS: [(&str, u32); 3] = [("AutoTargetMOB", 1), ("AutoTargetPvP", 3), ("DisableXPGain", 4)];
/// Bits 7 and 8 of `VisualFlags`: `VisiblePVPTitle`.
const TITLE_MASK: i32 = 0x180;

#[derive(Default)]
pub(in crate::play) struct Live {
    effects: Option<bool>,
    flags: Option<i32>,
    visual: Option<i32>,
}

fn set_bool(d: &mut DValues, name: &str, v: bool) {
    if d.exists(name) {
        d.set_i64(name, i64::from(v));
    } else {
        d.add(name, Variant::Bool(v), false, CAT_VARIABLES, None, None, false);
    }
}

impl Live {
    /// Per frame: runs the observers whose value changed since the last call. Frames go to `out`.
    pub fn apply(&mut self, d: &mut DValues, zone: &mut Zone, out: &mut Vec<Frame>) {
        // ToggleAllEffects
        let all = d.flag("ToggleAllEffects");
        if self.effects.is_some_and(|e| e != all) {
            for (n, chr) in EFFECT_PREFS {
                d.prefs.set_int(n, i32::from(all), if chr { Kind::Char } else { Kind::Login });
            }
        }
        self.effects = Some(all);

        let id = zone.char_id as i32;
        // stat 0x15d <-> AutoTargetMOB / AutoTargetPvP / DisableXPGain
        if let Some(stat) = zone.stat(STAT_OPTION_FLAGS) {
            if self.flags != Some(stat) {
                // the server (or the first sight) set the stat: the check boxes follow it (`SetDValue` at construction)
                for (n, bit) in FLAG_BITS {
                    set_bool(d, n, stat >> bit & 1 != 0);
                }
                self.flags = Some(stat);
            } else {
                let mut want = stat;
                for (n, bit) in FLAG_BITS {
                    want = if d.flag(n) { want | 1 << bit } else { want & !(1 << bit) };
                }
                if want != stat {
                    out.push(event_feedback(id, 0xa5, want));
                    zone.stats.insert(STAT_OPTION_FLAGS, want);
                    self.flags = Some(want);
                }
            }
        }
        // stat 0x2a1 <-> VisualFlags / VisiblePVPTitle
        if let Some(stat) = zone.stat(STAT_VISUAL_FLAGS) {
            if self.visual != Some(stat) {
                d.set_i64("VisualFlags", i64::from(stat));
                d.set_i64("VisiblePVPTitle", i64::from(stat >> 7 & 3));
                self.visual = Some(stat);
            } else {
                let vf = d.get_i64("VisualFlags").unwrap_or(i64::from(stat)) as i32;
                let title = d.get_i64("VisiblePVPTitle").unwrap_or(i64::from(stat >> 7 & 3)) as i32;
                let want = if vf != stat { vf } else { stat & !TITLE_MASK | (title & 3) << 7 };
                if want != stat {
                    out.push(event_feedback(id, 0xa6, want));
                    zone.stats.insert(STAT_VISUAL_FLAGS, want);
                    // the DValues follow the stored stat again
                    d.set_i64("VisualFlags", i64::from(want));
                    d.set_i64("VisiblePVPTitle", i64::from(want >> 7 & 3));
                    self.visual = Some(want);
                }
            }
        }
    }
}

/// `N3Msg_EventFeedback` of a changed value: `CharacterActionIIR_t(action, a = {0,0}, b = {0, value})` (Gamecode 0x1001cbb4, `FUN_1007253f`).
pub fn event_feedback(char_id: i32, action: i32, value: i32) -> Frame {
    n3_frame(0, char_id as u32, character_action(char_id, &simple(action, Identity::default(), Identity { kind: 0, instance: value })))
}

/// `SoundOptionsMonitor_c` (`SlotPrefMasterVolumeChanged` 0x100c47cf, `SlotPrefSoundFXOnChanged` ..., `SlotPrefBattlemusicModeChanged`): the sound prefs of the client's audio engine.
pub fn audio_prefs(d: &DValues) -> ao_audio::Prefs {
    let f = |n: &str, def: f32| match d.get(n) {
        Some(Variant::Float(v)) => *v,
        Some(Variant::Int(v)) => *v as f32,
        _ => def,
    };
    ao_audio::Prefs {
        sound_on: d.get("SoundOnOff").is_none_or(|_| d.flag("SoundOnOff")),
        master: f("MasterVolume", 1.0),
        fx_on: d.get("SoundFXOn").is_none_or(|_| d.flag("SoundFXOn")),
        fx: f("FXVolume", 1.0),
        music_on: d.get("MusicOn").is_none_or(|_| d.flag("MusicOn")),
        music: f("MusicVolume", 1.0),
        battlemusic_mode: d.get_i64("BattlemusicMode").unwrap_or(3) as i32,
    }
}

/// `SetMasterPlayerFXMute(!VoiceSndFxOn)` / `SetMasterPlayerFXVolume(VoiceSndFxVolume)` (0x100c487a / 0x100c489b): the gain of the voice sounds
/// (`/voice`, heard voice messages) -- 0 when muted, also by the master `SoundOnOff`.
pub fn voice_gain(d: &DValues) -> f32 {
    let p = audio_prefs(d);
    let on = p.sound_on && d.get("VoiceSndFxOn").is_none_or(|_| d.flag("VoiceSndFxOn"));
    match d.get("VoiceSndFxVolume") {
        _ if !on => 0.0,
        Some(Variant::Float(v)) => v.clamp(0.0, 1.0),
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::action::parse_character_action;

    fn store() -> DValues {
        let mut d = DValues::default();
        d.load_config(
            r#"<Root><Value name="ToggleAllEffects" value="true"/><Value name="VisualFlags" value="31"/><Value name="VisiblePVPTitle" value="0"/><Value name="OpenNanoWindow" value="false"/><Value name="VoiceSndFxVolume" value="0.5" min="0.0" max="1.0"/></Root>"#,
            super::super::super::dvalue::CAT_CHAR,
            true,
        );
        d.prefs = super::super::super::dvalue::IndepPrefs::with_defaults();
        d
    }

    fn zone() -> Zone {
        let mut z = Zone::default();
        z.char_id = 77;
        z.stats.insert(STAT_OPTION_FLAGS, 0b01010);
        z.stats.insert(STAT_VISUAL_FLAGS, 31 | 1 << 7);
        z
    }

    #[test]
    fn server_stats_fill_the_options_and_changes_send_event_feedback() {
        let (mut d, mut z, mut live, mut out) = (store(), zone(), Live::default(), vec![]);
        live.apply(&mut d, &mut z, &mut out);
        // 0x15d = 0b01010: bit 1 (auto target monsters) and bit 3 (players) set, bit 4 (disable XP) clear
        assert!(d.flag("AutoTargetMOB") && d.flag("AutoTargetPvP") && !d.flag("DisableXPGain"));
        assert_eq!((d.get_i64("VisualFlags"), d.get_i64("VisiblePVPTitle")), (Some(31 | 128), Some(1)));
        assert!(out.is_empty(), "reading the stats sends nothing");
        // the user ticks "disable XP gain" and unticks auto target monsters
        d.set_i64("DisableXPGain", 1);
        d.set_i64("AutoTargetMOB", 0);
        live.apply(&mut d, &mut z, &mut out);
        assert_eq!(z.stat(STAT_OPTION_FLAGS), Some(0b11000));
        assert_eq!(out.len(), 1);
        let (_, a) = parse_character_action(&out[0].payload).unwrap();
        assert_eq!((a.action, a.identity_a, a.identity_b), (0xa5, Identity::default(), Identity { kind: 0, instance: 0b11000 }));
        // no further message while nothing changes
        live.apply(&mut d, &mut z, &mut out);
        assert_eq!(out.len(), 1);
        // a visual flag bit goes out with action 0xa6 and the whole stat value
        d.set_i64("VisualFlags", i64::from(31 | 128) & !4);
        live.apply(&mut d, &mut z, &mut out);
        let (_, a) = parse_character_action(&out[1].payload).unwrap();
        assert_eq!((a.action, a.identity_b.instance), (0xa6, (31 | 128) & !4));
        assert_eq!(z.stat(STAT_VISUAL_FLAGS), Some((31 | 128) & !4));
        // the PvP title radio replaces bits 7..8 only
        d.set_i64("VisiblePVPTitle", 3);
        live.apply(&mut d, &mut z, &mut out);
        let (_, a) = parse_character_action(&out[2].payload).unwrap();
        assert_eq!(a.identity_b.instance, 31 & !4 | 3 << 7);
        assert_eq!(d.get_i64("VisualFlags"), Some(i64::from(31 & !4 | 3 << 7)), "the VisualFlags DValue follows the stored stat");
        // a server update wins over the DValues
        z.stats.insert(STAT_OPTION_FLAGS, 0);
        live.apply(&mut d, &mut z, &mut out);
        assert!(!d.flag("DisableXPGain") && !d.flag("AutoTargetPvP"));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn toggle_all_effects_sets_every_effect_pref() {
        let (mut d, mut z, mut live, mut out) = (store(), Zone::default(), Live::default(), vec![]);
        live.apply(&mut d, &mut z, &mut out);
        assert_eq!(d.prefs.get_int("Wildlife", Kind::Login), Some(1), "the first sight changes nothing");
        d.set_i64("ToggleAllEffects", 0);
        live.apply(&mut d, &mut z, &mut out);
        for (n, chr) in EFFECT_PREFS {
            assert_eq!(d.prefs.get_int(n, if chr { Kind::Char } else { Kind::Login }), Some(0), "{n}");
        }
        d.set_i64("ToggleAllEffects", 1);
        live.apply(&mut d, &mut z, &mut out);
        assert_eq!(d.prefs.get_int("IsSpaceShipsShown", Kind::Char), Some(1));
        assert_eq!(d.prefs.get_int("SmoothAnimations", Kind::Login), Some(1));
    }

    #[test]
    fn sound_prefs_follow_the_dvalues() {
        let mut d = store();
        d.load_config(r#"<Root><Value name="SoundOnOff" value="true"/><Value name="MasterVolume" value="0.5"/><Value name="MusicOn" value="false"/><Value name="BattlemusicMode" value="1"/></Root>"#, super::super::super::dvalue::CAT_LOGIN, true);
        let p = audio_prefs(&d);
        assert_eq!((p.sound_on, p.master, p.music_on, p.fx_on, p.battlemusic_mode), (true, 0.5, false, true, 1));
        assert_eq!(voice_gain(&d), 0.5);
        d.set_i64("SoundOnOff", 0);
        assert_eq!(voice_gain(&d), 0.0, "the master switch mutes the voices too");
    }
}
