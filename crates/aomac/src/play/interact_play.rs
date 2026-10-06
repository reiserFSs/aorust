//! The `Play` side of the interaction layer: the hooks `flow.rs` calls (zone frames, GUI events, mouse clicks, the per-frame outbox).

use super::hud_target::{pick_all, pick_ray};
use super::interact::Interact;
use super::Play;
use ao_gui::{Event, InputEvent, MouseButton};
use ao_net::frame::Frame;
use ao_render::Host;

impl Play {
    /// A new zone connection / playfield: the old dialogue window is gone (`StopPlayfield`), the state starts over.
    pub(super) fn interact_reset(&mut self) {
        if let Some(i) = self.interact.as_mut() {
            i.close_all(&mut self.gui);
        }
        self.interact = Some(Interact::new(self.zone.char_id, self.size));
    }

    pub(super) fn interact_zone_frame(&mut self, f: &Frame) {
        if let Some(i) = self.interact.as_mut() {
            i.on_frame(&mut self.gui, f, &self.zone);
        }
    }

    pub(super) fn interact_event(&mut self, e: &Event) -> bool {
        self.interact.as_mut().is_some_and(|i| i.event(&mut self.gui, e, &self.zone))
    }

    /// Per frame: the frames the player's actions produced go to the zone server.
    pub(super) fn interact_frame(&mut self) {
        let Some(i) = self.interact.as_mut() else { return };
        i.resize(self.size);
        for t in i.take_notices() {
            if let Some(c) = self.chat.as_mut() {
                c.system_line(&mut self.gui, &t, 12);
            }
        }
        for f in i.take_outbox() {
            if let Some(s) = &self.session {
                s.send_zone(f);
            }
        }
    }

    /// A plain left click that selected `id`: the second one on the same dynel within [`ao_gui::DOUBLE_CLICK_TIME`] is a double click,
    /// which runs the default action when the option `DoubleclickAction` is on (`LoginPrefs.xml`: true; `FUN_1002c2ee`).
    pub(super) fn interact_left_click(&mut self, id: i32) {
        let now = self.time;
        if let Some(i) = self.interact.as_mut() {
            if i.double_click(id, now) {
                i.default_action(id);
            }
        }
    }

    /// The right button released without a look (`Controls` emits `Click(Right)`): `N3Msg_DefaultActionOnDynel` on the dynel under the pointer.
    pub(super) fn interact_mouse(&mut self, ev: &InputEvent, host: &Host) {
        let InputEvent::MouseUp { x, y, button: MouseButton::Right } = *ev else { return };
        let clicked = self.player.as_mut().is_some_and(|p| p.take_clicks().contains(&MouseButton::Right));
        if !clicked || self.gui.wants_mouse(x, y) {
            return;
        }
        let ray = pick_ray(&host.camera, &host.lens.unwrap_or_default(), (self.size.0 as f32, self.size.1 as f32), (x, y));
        // `GetObjectUnderColLine`: the current target when it is under the pointer, else the nearest hit
        let list = pick_all(&ray, &self.zone);
        let id = self.zone.target.filter(|t| list.contains(t)).or_else(|| list.first().copied());
        if let (Some(id), Some(i)) = (id, self.interact.as_mut()) {
            i.default_action(id);
        }
    }
}
