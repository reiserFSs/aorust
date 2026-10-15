//! The `Play` side of the interaction layer: the hooks `flow.rs` calls (zone frames, GUI events, mouse clicks, the per-frame outbox).

use super::hud_target::pick_ray;
use super::interact::Interact;
use super::interact_use::pick_objects;
use super::Play;
use ao_gui::{Event, InputEvent, MouseButton};
use ao_net::msg::Identity;
use ao_net::n3::outgoing::DYNEL_CHAR;
use ao_net::frame::Frame;
use ao_render::Host;

impl Play {
    /// A new zone connection / playfield: the old dialogue window is gone (`StopPlayfield`), the state starts over.
    pub(super) fn interact_reset(&mut self) {
        if let Some(i) = self.interact.as_mut() {
            i.close_all(&mut self.gui);
        }
        let mut i = Interact::new(self.zone.char_id, self.size);
        if let Some(hud) = self.hud.as_ref() { i.configure_escape(&hud.dvalues); }
        i.set_client_dir(self.dir.clone());
        i.trade.texts = super::interact_chat::ChatTexts::load(&self.text);
        i.ptrade.set_texts(self.text.by_key(10000, "MsgBox_Yes").unwrap_or_default(), self.text.by_key(10000, "MsgBox_No").unwrap_or_default());
        self.interact = Some(i);
    }

    pub(super) fn interact_zone_frame(&mut self, f: &Frame) {
        if let Some(i) = self.interact.as_mut() {
            if let Some(hud) = self.hud.as_ref() {
                i.configure_escape(&hud.dvalues);
                i.mission.load_config(&hud.dvalues);
            }
            i.on_frame(&mut self.gui, f, &mut self.zone);
        }
    }

    pub(super) fn interact_event(&mut self, e: &Event) -> bool {
        if let (Some(i), Some(hud)) = (self.interact.as_mut(), self.hud.as_ref()) { i.configure_escape(&hud.dvalues); }
        self.interact.as_mut().is_some_and(|i| i.event(&mut self.gui, e, &mut self.zone))
    }

    /// Per frame: the frames the player's actions produced go to the zone server.
    pub(super) fn interact_frame(&mut self) {
        let Some(i) = self.interact.as_mut() else { return };
        let mut check_ready = true;
        if let Some(hud) = self.hud.as_mut() {
            i.configure_escape(&hud.dvalues);
            check_ready = hud.actions.list.progress(0xa1).is_none();
            for item in hud.take_tower_uses() { i.queue_tower_use(&self.zone, item); }
        }
        for item in std::mem::take(&mut i.tower_uses) {
            match i.tower.open(&mut self.gui, self.size, &self.zone, item, &self.text) {
                Ok(Some(payload)) => i.send(payload),
                Ok(None) => {}
                Err(error) => eprintln!("tower build menu: {error:#}"),
            }
        }
        if let Some(reply) = i.tower_reply.take() { i.tower.result(&mut self.gui, &reply, &self.text, &self.zone); }
        i.tower.update(&mut self.gui, &self.zone, check_ready);
        i.resize(self.size);
        i.tick(self.time);
        if let Some(hud) = self.hud.as_mut() {
            i.mission.load_config(&hud.dvalues);
            i.mission.save_config(&mut hud.dvalues);
            i.tower.sync_config(&mut self.gui, &mut hud.dvalues, self.size);
            i.use_ui.loot.update_tooltips(&mut self.gui, &self.zone, &self.text, hud.dvalues.flag("ItemDescriptionTooltips"));
        }
        for t in i.take_notices().into_iter().chain(i.tower.take_notices()) {
            if let Some(c) = self.chat.as_mut() {
                c.system_line(&mut self.gui, &t, 12);
            }
        }
        for key in i.take_feedback() {
            if let Some(c) = self.chat.as_mut() {
                c.feedback(&mut self.gui, key, &self.text);
            }
        }
        i.show_confirms(&mut self.gui, &self.text, &self.zone);
        for key in i.ptrade.take_feedback() {
            if let Some(c) = self.chat.as_mut() {
                c.feedback(&mut self.gui, key, &self.text);
            }
        }
        for url in i.mission.take_urls() {
            if let Some(chat) = self.chat.as_mut() {
                chat.show_url(&mut self.gui, &self.zone, &self.text, &url);
            }
        }
        for url in i.tower.take_urls() {
            if let Some(chat) = self.chat.as_mut() {
                chat.show_url(&mut self.gui, &self.zone, &self.text, &url);
            }
        }
        if let Some(marker) = i.mission.take_marker() {
            if let Some(hud) = self.hud.as_mut() {
                hud.set_mission(Some(marker));
            }
        }
        for f in i.take_outbox() {
            if let Some(s) = &self.session {
                s.send_zone(f);
            }
        }
        self.interact_trade_frame();
        // Containers claim their destinations before player trade drains the remaining foreign drops.
        for frame in self.interact_bank_frame() {
            if let Some(session) = &self.session { session.send_zone(frame); }
        }
        self.interact_ptrade_frame();
        self.interact_shop_frame();
        let notices = self.interact.as_mut().map(|i| std::mem::take(&mut i.use_ui.container_feedback)).unwrap_or_default();
        for (category, key) in notices {
            if let (Some(chat), Some(text)) = (self.chat.as_mut(), self.text.by_key(category, key)) {
                // GUI100304df ->10030636 connects GlobalSignals+184 to1002f852.
                let ok = self.text.by_key(10000, "MsgBox_OK").unwrap_or_default();
                chat.dialog(&mut self.gui, super::chat::dialog::Kind::Information, text, vec![ok], None);
            }
        }
    }

    /// A plain left click that selected character `id`: the second one on the same dynel within [`ao_gui::DOUBLE_CLICK_TIME`] is a double click,
    /// which runs the default action when the option `DoubleclickAction` is on (`LoginPrefs.xml`: true; `FUN_1002c2ee`).
    pub(super) fn interact_left_click(&mut self, id: i32) {
        let now = self.time;
        let id = Identity { kind: DYNEL_CHAR, instance: id };
        if let Some(i) = self.interact.as_mut() {
            if i.double_click(id, now) {
                i.default_action_on(&self.zone, id);
            }
        }
    }

    /// A mouse button released without a look (`Controls` emits `Click`) on the world. The object under the pointer is `GetObjectUnderColLine`: the
    /// current target when it is under the pointer, else the nearest hit (characters and objects, [`pick_objects`]).
    /// * Right button (`ActionViewMouseHandler_c` release `FUN_1002c469` [GUI]): a character -> `N3Msg_DefaultActionOnDynel`, anything else ->
    ///   `N3Msg_UseItem(id, false)`.
    /// * Left button: the HUD selects from the same merged hit list; a second click on a non-character runs its default action.
    pub(super) fn interact_mouse(&mut self, ev: &InputEvent, host: &Host) {
        if let Some(i) = self.interact.as_mut() {
            // GUI 100ca1e7: quick trade uses qualifier mask 0xc (Ctrl).
            i.shop.quick = host.mods.ctrl;
            if matches!(ev, InputEvent::MouseDown { button: MouseButton::Left, .. }) {
                // GUI 10040a17 -> 100c9afc: information uses mask 3 (Shift).
                i.use_ui.loot.info_click = host.mods.shift;
                i.mission.info_click = host.mods.shift;
            }
            if let InputEvent::MouseMove { x, y } = ev {
                i.trade.mouse = (*x, *y);
            }
        }
        // an inventory item released over the world (`FUN_100cb081` [GUI], docs/zone/interact.md §8.6): the object under the pointer is what it is used on
        if let InputEvent::MouseUp { x, y, button: MouseButton::Left } = *ev {
            if self.hud.as_ref().is_some_and(|h| h.item_dragging()) {
                let under = (!self.gui.wants_mouse(x, y)).then(|| {
                    let ray = pick_ray(&host.camera, &host.lens.unwrap_or_default(), (self.size.0 as f32, self.size.1 as f32), (x, y));
                    pick_objects(&ray, &self.zone, self.player.as_ref()).first().copied()
                });
                if let Some(h) = self.hud.as_mut() {
                    h.set_world_under(under.flatten());
                }
            }
        }
        let InputEvent::MouseUp { x, y, button: button @ (MouseButton::Left | MouseButton::Right) } = *ev else { return };
        let clicked = self.player.as_mut().is_some_and(|p| p.take_clicks().contains(&button));
        if !clicked || self.gui.wants_mouse(x, y) {
            return;
        }
        let ray = pick_ray(&host.camera, &host.lens.unwrap_or_default(), (self.size.0 as f32, self.size.1 as f32), (x, y));
        let list = pick_objects(&ray, &self.zone, self.player.as_ref());
        let target = self.zone.selected_target();
        let Some(id) = target.filter(|t| list.contains(t)).or_else(|| list.first().copied()) else { return };
        if button == MouseButton::Left {
            if let Some(h) = self.hud.as_mut() {
                h.world_click(&mut self.zone, &list, host.mods);
            }
        }
        let (now, zone) = (self.time, &self.zone);
        let Some(i) = self.interact.as_mut() else { return };
        match button {
            MouseButton::Right if id.kind == DYNEL_CHAR => {
                i.default_action_on(zone, id);
            }
            MouseButton::Right => {
                i.use_item(zone, id, false);
            }
            _ if !host.mods.shift && id.kind != DYNEL_CHAR && i.double_click(id, now) => {
                i.default_action_on(zone, id);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::inventory::{container_item_identity, InventoryUpdate};
    use ao_net::n3::world::{AcgItem, InventoryEntry};

    #[test]
    fn play_loot_access_refusal_opens_information_dialog_and_ok_sends_nothing() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut play = Play::new(dir, None, None, None).unwrap();
        play.size = (1280, 800);
        play.gui.set_screen_size(1280, 800);
        let mut chat = super::super::chat::Chat::new(false);
        chat.open(&mut play.gui, play.size, None).unwrap();
        play.chat = Some(chat);
        play.interact = Some(Interact::new(7, play.size));
        let source = Identity { kind: 0xc76a, instance: 8 };
        play.zone.apply_inventory(&ao_net::n3::inventory::InventoryMsg::Update(InventoryUpdate {
            capacity: 21, access: 1, entries: vec![], container: source, word: 0x70, flag: false,
        }));
        let before = play.gui.window_ids();
        play.interact.as_mut().unwrap().move_to_bag(&mut play.zone, container_item_identity(0x70, 0));
        assert!(play.interact.as_mut().unwrap().take_outbox().is_empty());
        play.interact_frame();
        let window = play.gui.window_ids().into_iter().find(|w| !before.contains(w)).expect("native Information dialog");
        assert_eq!(play.gui.text(window, "text"), "It is not possible to remove items from this inventory.");
        assert_eq!(play.gui.button_label(window, "btn0"), Some(play.text.by_key(10000, "MsgBox_OK").unwrap().as_str()));
        assert!(!play.gui.has_view(window, "progress_bar"), "not the login ProgressDialog");
        assert!(play.chat.as_mut().unwrap().event(&mut play.gui,
            &Event::Clicked { window, view: "btn0".into(), item: None }, &play.zone, &play.text));
        assert!(!play.gui.window_ids().contains(&window));
        assert!(play.interact.as_mut().unwrap().take_outbox().is_empty());
    }
    #[test]
    fn mission_alternative_raw_shift_click_inspects_once_and_preserves_plain_double() {
        use ao_net::n3::{mission_selection::{Alternative, Alternatives}, quest::Quest};
        use super::super::hud_listview::{BORDER, ICON_32, SPACING};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        for (shift, ctrl) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut play = Play::new(dir.clone(), None, None, None).unwrap();
            play.gui.set_screen_size(1280, 800);
            play.interact = Some(Interact::new(7, (1280, 800)));
            let origin = Identity { kind: 51000, instance: 2 };
            let quest = Quest { id: Identity { kind: 56000, instance: 3 }, name: "mission".into(), ..Default::default() };
            let i = play.interact.as_mut().unwrap();
            i.mission.open(&mut play.gui, (1280, 800), 1, origin).unwrap();
            i.mission.alternatives(&mut play.gui, Alternatives {
                difficulty: 50, dimensions: [0; 6], seed: 4, origin_type: 1, origin,
                missions: vec![Alternative { quest, kind: 1 }],
            });
            let win = i.mission.window().unwrap();
            let grid = play.gui.view_rect(win, "grid").unwrap();
            let (x, y) = (grid.l + BORDER + 2.0, grid.t + BORDER + 2.0);
            assert!(i.icon_hover_at(&play.gui, x, y));
            assert!(!i.icon_hover_at(&play.gui, x + ICON_32 + SPACING, y), "empty alternative slots are not quest icons");
            let mut host = Host::headless();
            host.mods = ao_gui::Modifiers { shift, ctrl, ..Default::default() };
            for click in 1..=2 {
                // Same ordering as Play::input: capture qualifiers, then dispatch GUI events.
                for ev in [
                    InputEvent::MouseDown { x, y, button: MouseButton::Left },
                    InputEvent::MouseUp { x, y, button: MouseButton::Left },
                ] {
                    play.interact_mouse(&ev, &host);
                    for event in play.gui.input(ev) {
                        play.interact.as_mut().unwrap().event(&mut play.gui, &event, &mut play.zone);
                    }
                }
                let i = play.interact.as_mut().unwrap();
                let expected = if shift || click == 2 { vec!["itemid://56000/3".to_owned()] } else { vec![] };
                assert_eq!(i.mission.take_urls(), expected, "Shift={shift}, Ctrl={ctrl}, click={click}");
                assert!(play.gui.is_enabled(win, "accept"), "inspection preserves native selection");
                assert!(i.take_outbox().is_empty(), "inspection never requests or accepts a mission");
            }
        }
    }


    #[test]
    fn play_container_frame_claims_deposits_and_preserves_other_window_drops() {
        use ao_net::n3::inventory;
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut play = Play::new(dir.clone(), None, None, None).unwrap();
        play.size = (1280, 800);
        play.gui.set_screen_size(1280, 800);
        play.hud = Some(super::super::hud::Hud::new(&mut play.gui, &dir, play.size).unwrap());
        play.interact = Some(Interact::new(play.zone.char_id, play.size));
        for kind in [0xdead, 0xc749] {
            let container = Identity { kind, instance: play.zone.char_id as i32 };
            let update = InventoryUpdate { capacity: 21, access: 0, container, word: 0x70, flag: true, entries: vec![] };
            let loot = &mut play.interact.as_mut().unwrap().use_ui.loot;
            loot.update(&mut play.gui, None, play.size, container, "Container", &update, kind == 0xc749);
            let window = loot.window_of(container).unwrap();
            let rect = play.gui.view_rect(window, if kind == 0xdead { "scrollview" } else { "grid" }).unwrap();
            let foreign = (0x41, -100.0, -100.0);
            play.hud.as_mut().unwrap().requeue_item_drops(vec![(0x40, rect.l + 10.0, rect.t + 10.0), foreign]);
            let requests = play.interact_bank_frame();
            assert_eq!(requests.len(), 1, "production Play frame must claim bank/backpack drops before player trade drains them");
            assert_eq!(requests[0].payload, inventory::container_add_item(play.zone.char_id as i32, container, inventory::item_identity(0x40)));
            assert_eq!(play.hud.as_mut().unwrap().take_item_drops(), [foreign]);
            play.interact.as_mut().unwrap().use_ui.loot.close_all(&mut play.gui);
        }
    }

    #[test]
    fn shift_inspects_ctrl_keeps_container_activation_and_shop_quick() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut play = Play::new(dir, None, None, None).unwrap();
        play.interact = Some(Interact::new(7, play.size));
        let mut host = Host::headless();
        for kind in [0xdead, 0xc76a] {
            let container = Identity { kind, instance: 7 };
            let update = InventoryUpdate {
                capacity: 21, access: 0, container, word: 0x70, flag: true,
                entries: vec![InventoryEntry {
                    slot: 3, a: 0, b: 0, id: Identity::default(),
                    item: AcgItem { low_id: 1, high_id: 1, level: 1 },
                }],
            };
            play.interact.as_mut().unwrap().use_ui.loot.update(&mut play.gui, None, play.size, container, "Items", &update, false);
            let window = play.interact.as_ref().unwrap().use_ui.loot.window_of(container).unwrap();
            let item = if kind == 0xdead { Identity { kind: 0x69, instance: 3 } } else { container_item_identity(update.word, 3) };
            for shift in [true, false] {
                host.mods = ao_gui::Modifiers { shift, ctrl: !shift, ..Default::default() };
                play.interact_mouse(&InputEvent::MouseDown { x: 0.0, y: 0.0, button: MouseButton::Left }, &host);
                let i = play.interact.as_mut().unwrap();
                assert_eq!(i.shop.quick, !shift, "only Ctrl requests quick trade");
                let gap = super::super::hud_stats::inv_grid::SPACING;
                let (x, y) = super::super::hud_stats::inv_grid::cell_origin(0, 0, (gap, gap));
                let press = Event::CanvasPress { window, view: "grid".into(), x: x + 2.0, y: y + 2.0, button: MouseButton::Left, clicks: 1 };
                assert_eq!(i.use_ui.loot.event(&mut play.gui, &press, 1.0), Some(None));
                if shift {
                    assert_eq!(i.use_ui.loot.info_urls, [format!("itemid://{}/{}", item.kind, item.instance)]);
                    assert_eq!(i.use_ui.loot.take(&mut play.gui, 0, 1.1), None);
                } else {
                    assert!(i.use_ui.loot.info_urls.is_empty(), "Ctrl must not inspect");
                    assert_eq!(i.use_ui.loot.take(&mut play.gui, 0, 2.0), Some(item), "Ctrl must not suppress ordinary double-click activation");
                }
                i.use_ui.loot.info_urls.clear();
            }
            play.interact.as_mut().unwrap().use_ui.loot.close_all(&mut play.gui);
        }
    }
}
