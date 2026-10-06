//! `Button_c` as a toggle: `ButtonBase_c::SetToggleButton(true)` + `SetValue` keep the pressed art after the release (the Programs window's
//! school selector, `FUN_100d6478` GUI 0x100d6478).

use super::*;

impl Gui {
    /// Shows the named `Button` with its pressed art (`on`) or its raised art. A mouse release resets it like any press: the application
    /// sets the state again after handling the click.
    pub fn set_button_pressed(&mut self, w: WindowId, name: &str, on: bool) {
        if let Some(v) = self.find(w, name) {
            if let Kind::Button(b) = &mut self.tree.views[v].kind {
                b.pressed = on;
            }
        }
    }
}
