//! Character deletion: `CharSelectWindow_c::SlotDeletePressed` → `CharDeleteWindow_c` (`Views/CharacterDeleteWindow.xml`,
//! GUI 0x1000de47): the typed name must equal the character's name (`SlotOkayButtonActivated` 0x1000d985, else the
//! `MatchError` box), then `LoginModule_c::SlotDeleteCharacter` → `Client_t::DeleteCharacter`; `CharacterDeleted`
//! removes the row (`SlotCharacterDeleted` 0x1000ed5c) without re-requesting the list.

use super::*;

impl Play {
    pub(super) fn delete_pressed(&mut self) {
        let Some(i) = self.selected.filter(|&i| i < self.chars.len()) else { return };
        if self.dialog_w.is_some() {
            return;
        }
        let Some(w) = self.open_centered("CharacterDeleteWindow") else { return };
        let fmt = self.text.by_key(10000, "DeleteCharacter").unwrap_or_default();
        self.gui.set_text(w, "confirmation_text", &fmt.replace("%s", &self.chars[i].info.name));
        self.gui.set_enabled(w, "ok_btn", false); // enabled once the name field is not empty
        self.gui.resize_window(w, WindowSize::Preferred);
        self.recenter(w);
        self.gui.focus(w, "name_input");
        self.dialog_w = Some((w, DialogKind::Delete));
    }

    pub(super) fn delete_ok(&mut self, host: &mut Host) {
        let Some((w, _)) = self.dialog_w else { return };
        let Some(c) = self.selected.and_then(|i| self.chars.get(i)) else { return };
        let (id, name) = (c.id, c.info.name.clone());
        if self.gui.text(w, "name_input") == name {
            self.close_dialog();
            if self.fake {
                // `--fake-charlist` has no server: the reply is simulated
                return self.character_deleted(id, host);
            }
            if let Some(s) = &self.session {
                s.delete_character(id as u32);
            }
        } else {
            let t = self.text.by_key(10000, "NamesDontMatch").unwrap_or_default();
            self.message_box(&t); // the `MatchError` DialogBox
        }
    }

    /// `SlotCharacterDeleted`: the row disappears, the list is rebuilt with nothing selected.
    pub(super) fn character_deleted(&mut self, id: i32, host: &mut Host) {
        let mut list = self.char_list.clone();
        list.characters.retain(|c| c.id != id);
        self.prefs.selected_character = -1;
        self.show_characters(list, host);
    }
}
