//! `TextMacroSystem_t` (GUI.dll): the text macros of the shortcut bar. Evidence: docs/chat/dialogs.md §6 (`/macro`).
//!
//! * `TextMacro_t` (0x44 bytes): `+0` class id `0xC789`, `+4` id, `+8` name (<= 100 chars), `+0x24` command text (<= 100 chars), `+0x40` saved flag.
//! * `CreateMacro(name, command, id, save)` (0x100228d0): null text -> 0; `id == 0` takes the lowest unused id from 1; an existing macro with
//!   that id is replaced; `save` writes `TextMacro.bin`.
//! * `TextMacro.bin` (`Load` 0x10022a4c / `Save` 0x10022742, big-endian (verified on the template file) `File_t::ReadS32`): `s32 count`, per macro `s32 id, s32 len, name,
//!   s32 len, command` (lengths <= 99 on load; a malformed file ends the load, macros read so far stay). Only macros with the saved flag are written.
//! * `/macro <name> <command>` (0x100b8693) emits `GlobalSignals+0x1a0(name, command)`; the shortcut bar window's slot `FUN_100d82d4`
//!   creates the macro (`save = true`) and starts dragging a `DragObject_c("inventory/textmacro", {item_id = Identity{0xC789, id}})`.

use std::path::PathBuf;

/// Longest name / command (`CreateMacro` clamps to 100 bytes; `Load` refuses lengths above 99).
const MAX: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    pub id: i32,
    pub name: String,
    pub command: String,
}

#[derive(Debug, Default)]
pub struct TextMacros {
    /// Ordered by id (`std::map<int, TextMacro_t*>`).
    macros: std::collections::BTreeMap<i32, Macro>,
    file: Option<PathBuf>,
}

/// Cuts at 100 bytes on a character boundary.
fn clamp(s: &str) -> String {
    let mut n = s.len().min(MAX);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].to_owned()
}

impl TextMacros {
    /// `<prefs dir>/TextMacro.bin` (the original's `<inventory config path>\TextMacro.bin`), loaded when present.
    pub fn open() -> Self {
        let file = super::super::prefs::dir().map(|d| d.join("TextMacro.bin"));
        let mut m = Self { file: file.clone(), ..Default::default() };
        if let Some(b) = file.and_then(|f| std::fs::read(f).ok()) {
            m.load(&b);
        }
        m
    }

    fn load(&mut self, b: &[u8]) {
        let mut at = 0usize;
        let s32 = |at: &mut usize| -> Option<i32> {
            let v = i32::from_be_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
            *at += 4;
            Some(v)
        };
        let Some(count) = s32(&mut at) else { return };
        for _ in 0..count {
            let text = |at: &mut usize| -> Option<String> {
                let n = usize::try_from(s32(at)?).ok().filter(|&n| n <= 99)?;
                let t = b.get(*at..*at + n)?;
                *at += n;
                Some(t.iter().map(|&c| c as char).collect())
            };
            let Some(id) = s32(&mut at) else { return };
            let (Some(name), Some(command)) = (text(&mut at), text(&mut at)) else { return };
            self.create(&name, &command, id, false);
        }
    }

    /// `TextMacroSystem_t::GetMacro(id)`.
    pub fn get(&self, id: i32) -> Option<&Macro> {
        self.macros.get(&id)
    }

    /// `CreateMacro`: returns the id used.
    pub fn create(&mut self, name: &str, command: &str, id: i32, save: bool) -> i32 {
        let id = if id == 0 { (1..).find(|i| !self.macros.contains_key(i)).unwrap_or(i32::MAX) } else { id };
        self.macros.insert(id, Macro { id, name: clamp(name), command: clamp(command) });
        if save {
            self.save();
        }
        id
    }

    /// `Save`: the file image.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = (self.macros.len() as i32).to_be_bytes().to_vec();
        for m in self.macros.values() {
            o.extend(m.id.to_be_bytes());
            for s in [&m.name, &m.command] {
                let b = super::zone::text_bytes(s);
                o.extend((b.len() as i32).to_be_bytes());
                o.extend(b);
            }
        }
        o
    }

    fn save(&self) {
        if let Some(f) = &self.file {
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Err(e) = std::fs::write(f, self.to_bytes()) {
                eprintln!("chat: TextMacro.bin: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_replace_and_roundtrip() {
        let mut m = TextMacros::default();
        assert_eq!(m.create("Follow", "/follow", 0, false), 1);
        assert_eq!(m.create("Wave", "/wave", 0, false), 2);
        assert_eq!(m.create("Hi", "/me hi", 5, false), 5);
        // the lowest free id is reused, an explicit id replaces
        m.macros.remove(&1);
        assert_eq!(m.create("Again", "/follow", 0, false), 1);
        assert_eq!(m.create("Wave2", "/wave 2", 2, false), 2);
        assert_eq!(m.get(2).unwrap().name, "Wave2");
        let mut back = TextMacros::default();
        back.load(&m.to_bytes());
        assert_eq!(back.macros, m.macros);
    }

    #[test]
    fn limits_and_truncation() {
        let mut m = TextMacros::default();
        let id = m.create(&"n".repeat(150), "c", 0, false);
        assert_eq!(m.get(id).unwrap().name.len(), 100);
        // a length above 99 ends the load; the macro before it stays
        let mut img = 2i32.to_be_bytes().to_vec();
        img.extend(1i32.to_be_bytes());
        img.extend(1i32.to_be_bytes());
        img.push(b'a');
        img.extend(1i32.to_be_bytes());
        img.push(b'b');
        img.extend(2i32.to_be_bytes());
        img.extend(100i32.to_be_bytes());
        let mut l = TextMacros::default();
        l.load(&img);
        assert_eq!(l.macros.len(), 1);
        assert_eq!(l.get(1).unwrap().command, "b");
        // an empty / short file loads nothing
        l.load(&[1, 0]);
        assert_eq!(l.macros.len(), 1);
    }

    /// The new-character template's `TextMacro.bin` holds exactly the Follow macro as id 1 (docs: `hud_bar::FIRST_LOGIN`).
    #[test]
    fn template_file_if_installed() {
        let Some(h) = std::env::var_os("HOME") else { return };
        let p = PathBuf::from(h).join("Games/ProjectRubiKa/client/prefs/NewChar/TextMacro.bin");
        let Ok(b) = std::fs::read(p) else { return };
        let mut m = TextMacros::default();
        m.load(&b);
        assert_eq!(m.get(1).map(|x| (x.name.as_str(), x.command.as_str())), Some(("Follow", "/follow")));
    }
}
