//! `CheckBox_c` state (`GFX_GUI_CHECKBOX_{UN,}CHECKED`): a click toggles it ([`Gui::mouse_up`]) and raises [`Event::Clicked`].

use super::*;

impl Gui {
    /// State of the named `CheckBox` (false if it is none).
    pub fn checked(&self, w: WindowId, name: &str) -> bool {
        matches!(self.find(w, name).map(|v| &self.tree.views[v].kind), Some(Kind::CheckBox { checked: true, .. }))
    }

    /// `CheckBox_c::SetValue`.
    pub fn set_checked(&mut self, w: WindowId, name: &str, on: bool) {
        if let Some(v) = self.find(w, name) {
            if let Kind::CheckBox { checked, .. } = &mut self.tree.views[v].kind {
                *checked = on;
            }
        }
    }
}
