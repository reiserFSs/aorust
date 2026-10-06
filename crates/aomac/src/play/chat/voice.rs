//! Voice chat: `/voice <sound>` and the playback of heard voice messages (GUI.dll). Evidence: docs/chat/dialogs.md §6 (`/voice`).
//!
//! * `/voice` (`FUN_100b82c2`): needs `N3Msg_GetSkill(0x185) & 1`. The sender builds `TextMacro_t {breed (stat 4), sex (stat 0x3b),
//!   fx type, sound}`; `FUN_1008c762` checks that a voice file exists (`<cd>/sound/sfx/player/<candidate>.wav` **and** `.txt`), else
//!   "You do not have that voice effect installed."; `FUN_1008cb83` reads the first line of the `.txt` (the spoken text, e.g.
//!   `[T]Dude! Juice man!`); `FUN_1008c17b` strips a leading `[flags]` (`M`/`m`: `GlobalSignals+0x150`, `T`/`t`:
//!   `TeamViewModule_c::FlashMemberName`); the remaining text goes out through `Group::Send(.., mode 0, macro)` of the window's output group
//!   (vicinity: ptype-5 message with the extras block `BBBSS(1, breed, sex, fx, sound)`, docs/chat/zone.md §4).
//! * Hearing (`HandleVicinityMessage` 0x10086728 -> `FUN_1008c953`): DValue `VoiceSndFxHearVicinityOn`, a non-empty sound, stat 0x185 bit 0 and
//!   a throttle (`FUN_1008c360`: the same sender is played again only 3 s after its last voice), then the first existing candidate wav is
//!   played with `SandyInterfaceModule_t::PlayPlayerFX(path, 1.0)`.
//! * Candidate file names (`FUN_1008c3e7`, tried in this order): `<name>_<sound>_01`, `<breed>_<sex>_<fx>_<sound>_01`,
//!   `<sex>_<fx>_<sound>_01`, `<sex>_<sound>_01`, `<sound>_01`; `..` is removed from each (`String::Remove`). The port also refuses
//!   candidates with a path separator (the original joins them unchecked; the sound name of a heard message is remote input).

use std::path::{Path, PathBuf};

/// Directory below the client's `cd_image` ("sound/sfx/player/").
const DIR: &str = "sound/sfx/player";

/// Extras block of a chat message: the `TextMacro_t` that a voice message carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Voice {
    pub breed: i32,
    pub sex: i32,
    pub fx: String,
    pub sound: String,
}

/// `FUN_1008c0e8`.
pub fn breed_name(breed: i32) -> &'static str {
    match breed {
        2 => "opifex",
        3 => "nano",
        4 => "atrox",
        _ => "solitus",
    }
}

/// `FUN_1008c13c`.
pub fn sex_name(sex: i32) -> &'static str {
    if sex == 3 {
        "female"
    } else {
        "male"
    }
}

/// `FUN_1008c012(.., true)` for the DValue `VoiceSndFxType` (the file names carry the original's spelling "distunguished").
pub fn fx_name(fx_type: i32) -> &'static str {
    match fx_type {
        1 => "distunguished",
        2 => "cool",
        3 => "military",
        _ => "simple",
    }
}

/// `FUN_1008c3e7`: candidate base names, most specific first.
pub fn candidates(name: &str, v: &Voice) -> Vec<String> {
    let (b, s, f, n) = (breed_name(v.breed), sex_name(v.sex), v.fx.as_str(), v.sound.as_str());
    [format!("{name}_{n}_01"), format!("{b}_{s}_{f}_{n}_01"), format!("{s}_{f}_{n}_01"), format!("{s}_{n}_01"), format!("{n}_01")]
        .into_iter()
        .map(|c| c.replace("..", ""))
        .filter(|c| !c.contains(['/', '\\']))
        .collect()
}

fn base(cd: &Path, cand: &str, ext: &str) -> PathBuf {
    cd.join(DIR).join(format!("{cand}{ext}"))
}

/// `FUN_1008c762`: the first candidate whose `.wav` and `.txt` both exist.
pub fn installed(cd: &Path, cands: &[String]) -> bool {
    cands.iter().any(|c| base(cd, c, ".wav").is_file() && base(cd, c, ".txt").is_file())
}

/// `FUN_1008cb83`: first line (at most 1023 bytes, `fgets`) of the first candidate `.txt` that opens; "" when none does.
pub fn spoken_text(cd: &Path, cands: &[String]) -> String {
    for c in cands {
        if let Ok(b) = std::fs::read(base(cd, c, ".txt")) {
            let mut line: Vec<u8> = b.iter().copied().take(1023).take_while(|&x| x != b'\n').collect();
            if b.get(line.len()) == Some(&b'\n') {
                line.push(b'\n'); // `fgets` keeps the newline
            }
            return line.iter().map(|&x| x as char).collect();
        }
    }
    String::new()
}

/// `FUN_1008c953`'s file test: the sound path below `cd_image/sound` (as [`ao_audio::Audio::play_sfx`] takes it) of the first candidate with a `.wav`.
pub fn wav_rel(cd: &Path, cands: &[String]) -> Option<String> {
    cands.iter().find(|c| base(cd, c, ".wav").is_file()).map(|c| format!("sfx/player/{c}.wav"))
}

/// What `FUN_1008c17b` does with the spoken text: a leading `[flags]` is removed; flags are returned in order.
pub fn strip_flags(text: &str) -> (String, Vec<char>) {
    if let (true, Some(end)) = (text.starts_with('['), text.find(']')) {
        (text[end + 1..].to_owned(), text[1..end].chars().collect())
    } else {
        (text.to_owned(), vec![])
    }
}

/// Sender side of the extras: `pack("BBBSS", 1, breed, sex, fx, sound)` (`FUN_10089163`, S = u16 BE length + bytes).
pub fn extras(v: &Voice) -> Vec<u8> {
    let mut o = vec![1, v.breed as u8, v.sex as u8];
    for s in [&v.fx, &v.sound] {
        let b = super::zone::text_bytes(s);
        o.extend((b.len() as u16).to_be_bytes());
        o.extend(b);
    }
    o
}

/// Receiver side (`FUN_10085b4a`): the TLV list after the message-kind byte. Tag 0 `IIISS` (item link, skipped as a block), tag 1 `BBSS`
/// ([`Voice`] with `fx` = S1, `sound` = S2), tag 2 `B` (flags, bit 0 = bypass the ignore list). Returns the voice block and the flags;
/// a truncated list ends the parse like the original's `-1`.
pub fn parse_block(mut d: &[u8]) -> (Option<Voice>, u8) {
    let (mut voice, mut flags) = (None, 0);
    let s16 = |d: &mut &[u8]| -> Option<String> {
        let n = u16::from_be_bytes([*d.first()?, *d.get(1)?]) as usize;
        let b = d.get(2..2 + n)?;
        *d = &d[2 + n..];
        Some(b.iter().map(|&x| x as char).collect())
    };
    while let Some((&tag, rest)) = d.split_first() {
        d = rest;
        match tag {
            0 => {
                if d.len() < 12 {
                    break;
                }
                d = &d[12..];
                if s16(&mut d).is_none() || s16(&mut d).is_none() {
                    break;
                }
            }
            1 => {
                let Some(&[breed, sex]) = d.get(..2).map(|x| <&[u8; 2]>::try_from(x).unwrap()) else { break };
                d = &d[2..];
                let (Some(fx), Some(sound)) = (s16(&mut d), s16(&mut d)) else { break };
                voice = Some(Voice { breed: breed as i32, sex: sex as i32, fx, sound });
            }
            2 => {
                let Some((&f, rest)) = d.split_first() else { break };
                flags = f;
                d = rest;
            }
            _ => break,
        }
    }
    (voice, flags)
}

/// `FUN_1008c360`: a sender's voice is played when its previous one is at least 3 s old (the original compares `time()` seconds).
#[derive(Default)]
pub struct Throttle(std::collections::HashMap<String, i64>);

impl Throttle {
    pub fn allow(&mut self, who: &str, now: i64) -> bool {
        let ok = self.0.get(who).is_none_or(|&t| now - t >= 3);
        if ok {
            self.0.insert(who.to_owned(), now);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Option<PathBuf> {
        let d = PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client/cd_image");
        d.is_dir().then_some(d)
    }

    #[test]
    fn candidate_order_and_names() {
        let v = Voice { breed: 4, sex: 2, fx: "cool".into(), sound: "heal".into() };
        assert_eq!(candidates("Bob", &v), ["Bob_heal_01", "atrox_male_cool_heal_01", "male_cool_heal_01", "male_heal_01", "heal_01"]);
        assert_eq!(fx_name(0), "simple");
        assert_eq!(fx_name(1), "distunguished");
        let v = Voice { breed: 3, sex: 3, fx: "simple".into(), sound: "../x/..y".into() };
        assert!(candidates("a", &v).iter().all(|c| !c.contains(['/', '.'])), "{:?}", candidates("a", &v));
    }

    #[test]
    fn flags_and_wire_block() {
        assert_eq!(strip_flags("[T]Dude! Juice man!\n"), ("Dude! Juice man!\n".into(), vec!['T']));
        assert_eq!(strip_flags("hi"), ("hi".into(), vec![]));
        assert_eq!(strip_flags("[Mt]x"), ("x".into(), vec!['M', 't']));
        let v = Voice { breed: 2, sex: 3, fx: "cool".into(), sound: "no".into() };
        let b = extras(&v);
        assert_eq!(b, [1, 2, 3, 0, 4, b'c', b'o', b'o', b'l', 0, 2, b'n', b'o']);
        assert_eq!(parse_block(&b), (Some(v.clone()), 0));
        let mut with_flags = b.clone();
        with_flags.extend([2, 1]);
        assert_eq!(parse_block(&with_flags), (Some(v), 1));
        assert_eq!(parse_block(&[1, 2]).0, None, "truncated");
    }

    #[test]
    fn throttle_is_per_sender() {
        let mut t = Throttle::default();
        assert!(t.allow("a", 10) && !t.allow("a", 12) && t.allow("b", 12) && t.allow("a", 13));
    }

    /// The shipped voice files: every `<breed>_<sex>_<fx>_<sound>_01` pair resolves and its text starts with a flag block or text.
    #[test]
    fn real_voice_files() {
        let Some(cd) = client() else { return };
        let v = Voice { breed: 4, sex: 2, fx: "cool".into(), sound: "heal".into() };
        let c = candidates("Nobody", &v);
        assert!(installed(&cd, &c));
        assert_eq!(spoken_text(&cd, &c).trim_end(), "[T]Dude! Juice man!");
        assert_eq!(wav_rel(&cd, &c).as_deref(), Some("sfx/player/atrox_male_cool_heal_01.wav"));
        let none = Voice { sound: "nonexistent".into(), ..v };
        assert!(!installed(&cd, &candidates("Nobody", &none)));
    }
}
