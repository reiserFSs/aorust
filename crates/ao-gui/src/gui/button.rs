//! `Button_c` as a toggle: `ButtonBase_c::SetToggleButton(true)` + `SetValue` keep the pressed art after the release (the Programs window's
//! school selector, `FUN_100d6478` GUI 0x100d6478).

use super::*;

impl Gui {
    /// Label drawn by the named `Button` (not editable text returned by [`Gui::text`]).
    pub fn button_label(&self, w: WindowId, name: &str) -> Option<&str> {
        match &self.tree.views[self.find(w, name)?].kind {
            Kind::Button(b) => Some(&b.label),
            _ => None,
        }
    }

    /// `Button_c::SetBackgroundIcon` 0x10127d08. The caller loads either skin art or an RDB bitmap.
    pub fn set_button_background_icon(&mut self, w: WindowId, name: &str, icon: Option<GfxId>) {
        let Some(v) = self.find(w, name) else { return };
        match &mut self.tree.views[v].kind {
            Kind::Button(b) => {
                b.background_icon = icon;
                b.background_icon_timer = 0.0;
                b.background_icon_fade = if b.hover { 1.0 } else { 0.0 };
            }
            Kind::CcEntry(c) => {
                c.icon = icon;
                c.fade_timer = 0.0;
                c.fade = if c.hover { 1.0 } else { 0.0 };
            }
            _ => return,
        }
        self.relayout_window(w);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_background_icon_bounds_and_states() {
        let dir = crate::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let w = gui.open_window_xml("icon", r#"<root><Button name="button" label="Vehicle"/></root>"#, (0, 0), WindowSize::Fixed(160, 80)).unwrap();
        let icon = gui.add_image("vehicle", vec![255; 48 * 48 * 4], 48, 48, false);
        gui.set_button_background_icon(w, "button", Some(icon));
        let id = gui.find(w, "button").unwrap();
        let mut env = layout::Env { gfx: &gui.gfx, fonts: &mut gui.fonts, colors: &gui.colors, groups: Default::default(), preferred: Default::default(), group_depth: 0 };
        let preferred = layout::pref(&mut env, &gui.tree, id, false);
        assert!(preferred.x >= 73.0);
        assert_eq!(preferred.y, 56.0);
        let r = Rect::new(0.0, 0.0, 159.0, 79.0);
        for (pressed, enabled, fade, grey) in [(false, true, 0.0, 255), (true, true, 0.0, 255), (true, false, 1.0, 18)] {
            let mut v = gui.tree.views[id].clone();
            v.enabled = enabled;
            let Kind::Button(b) = &mut v.kind else { panic!("button") };
            b.pressed = pressed;
            b.background_icon_fade = fade;
            let b = b.clone();
            let mut out = Vec::new();
            gui.draw_button(&mut out, &v, &b, r, [255; 3], 1.0);
            let (iw, ih) = gui.gfx.size(icon);
            let l = ((160.0 - iw as f32) * 0.5).floor();
            let t = ((80.0 - ih as f32) * 0.5).floor();
            assert!(out.iter().any(|c| matches!(c, DrawCmd::Gfx { id, dst, tint, alpha, .. } if *id == icon && *dst == [l, t, l + iw as f32, t + ih as f32] && *tint == [grey; 3] && *alpha == BUTTON_ALPHA)));
        }
        gui.set_button_background_icon(w, "button", None);
        assert!(matches!(&gui.tree.views[id].kind, Kind::Button(b) if b.background_icon.is_none()));
    }
}
