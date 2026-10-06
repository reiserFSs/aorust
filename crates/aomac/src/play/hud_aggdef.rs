//! The AGG/DEF slider of the right control-centre bar: `Slider_c` (GUI.dll ctor 0x1014448a, range -100 ..= 100) inside
//! `RightBarView_c` (0x1006ff09). Evidence and the drag rules: docs/gui.md §10 "AGG/DEF slider".

use ao_gui::view::CanvasItem;
use ao_gui::{Gui, GfxId, InputEvent, MouseButton, WindowId};

/// `Slider_c::Slider_c(rect, "", -1, -100.0, 100.0, 0, 0)` (`_DAT_101b5358` / `_DAT_101adfa8`).
const MIN: f32 = -100.0;
const MAX: f32 = 100.0;
/// `GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER_BACKGROUND` (0x92, 128 x 18) and the knob `GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER` (0x91, 11 x 18).
const BACKGROUND: &str = "GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER_BACKGROUND";
const KNOB: &str = "GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER";
/// Knob travel: `(Width(slider) + 1 - borders) - (Width(knob) + 1)` on inclusive extents = 128 - 11 (`Slider_c::_Layout` 0x10144141).
const TRAVEL: f32 = 128.0 - 11.0;
const KNOB_W: f32 = 11.0;
const KNOB_H: f32 = 18.0;
/// `ColorID_e` DEFAULT: `View::SetColor(knob, 0x1000000)` (ctor).
const DEFAULT: u32 = 0x1000000;
/// The view name inside the right bar dock.
pub(super) const VIEW: &str = "aggdef";

/// XML of the slider view (inclusive extents of the 128 x 18 background).
pub(super) fn view_xml() -> String {
    format!("<CanvasView name=\"{VIEW}\" min_size=\"Point(127,17)\" max_size=\"Point(127,17)\"/>")
}

/// `floor((value - min) * travel / (max - min))`: the knob's left edge inside the slider (`_Layout`).
fn knob_x(v: f32) -> f32 {
    ((v - MIN) * TRAVEL / (MAX - MIN)).floor()
}

#[derive(Default)]
pub(super) struct AggDef {
    value: f32,
    /// Grab offset (mouse x - knob left) while the knob is dragged (`Slider_c` +0x170 / +0x174).
    grab: Option<f32>,
    painted: Option<f32>,
}

impl AggDef {
    /// Follows the character's stat 0x33 (`FUN_1006e968`: `SetValue(N3Msg_GetAggDef(), false)` on every change of the stat; the
    /// slider is 0 until the character exists, `GetAggDef` returns 0 without a client char).
    pub(super) fn update(&mut self, gui: &mut Gui, cc: WindowId, stat: Option<i32>) {
        if self.grab.is_none() {
            self.value = (stat.unwrap_or(0) as f32).clamp(MIN, MAX);
        }
        if self.painted != Some(self.value) {
            self.painted = Some(self.value);
            let mut items = vec![];
            let mut put = |name: &str, x: f32, tint: Option<u32>| {
                let Some(g) = gui.gfx_id(name).map(GfxId) else { return };
                let (w, h) = gui.gfx().size(g);
                let (src, dst) = ([0.0, 0.0, w as f32, h as f32], [x, 0.0, x + w as f32, h as f32]);
                items.push(match tint {
                    Some(color) => CanvasItem::ImageTint { id: g, src, dst, color, alpha: 1.0 },
                    None => CanvasItem::Image { id: g, src, dst, alpha: 1.0 },
                });
            };
            put(BACKGROUND, 0.0, None);
            put(KNOB, knob_x(self.value), Some(DEFAULT));
            gui.set_canvas(cc, VIEW, items);
        }
    }

    /// `Slider_c::MouseDown` / `MouseMove` / `MouseUp`. Pressing the knob grabs it; moving sets the value from the pointer
    /// (`(max - min) * (x - grab - border) / travel + min`, clamped by `SetValue`) and the slot `FUN_1006e9c5` floors it while it is
    /// dragged; the release emits the final value, `N3Msg_SetAggDef(v)`. Returns that value on release.
    pub(super) fn input(&mut self, gui: &Gui, cc: WindowId, ev: &InputEvent) -> Option<i32> {
        let r = gui.view_rect(cc, VIEW)?;
        match *ev {
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                let left = r.l + knob_x(self.value);
                if x >= left && x < left + KNOB_W && y >= r.t && y < r.t + KNOB_H {
                    self.grab = Some(x - left);
                }
                None
            }
            InputEvent::MouseMove { x, .. } => {
                if let Some(g) = self.grab {
                    self.value = ((MAX - MIN) * (x - g - r.l) / TRAVEL + MIN).clamp(MIN, MAX).floor();
                }
                None
            }
            InputEvent::MouseUp { button: MouseButton::Left, .. } => self.grab.take().map(|_| self.value as i32),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_positions() {
        // 0 sits at 58 (the static layout's `Rect(58,0,0,0)` knob), the ends at 0 and 117
        assert_eq!((knob_x(-100.0), knob_x(0.0), knob_x(100.0)), (0.0, 58.0, 117.0));
    }
}
