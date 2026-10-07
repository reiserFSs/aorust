//! Original `PetWindow_c` / `PetListView_c` / `PetView_c` (GUI 0x1007e3ee, 0x1007c770, 0x1007d22e).
//! The builder is native code, not a client XML file; see docs/zone/pets.md.

use super::zone::Zone;
use ao_formats::screens::TextDb;
use ao_gui::{CanvasItem, Event, GfxId, Gui, MouseButton, WindowId, WindowSize};
use ao_net::{frame::Frame, msg::Identity, n3::textcmd};
use std::path::Path;
use std::collections::HashMap;

pub(super) struct HudPet {
    window: Option<WindowId>,
    pets: Vec<Identity>,
    texts: Option<TextDb>,
    screen: (u32, u32),
    closed: bool,
    select: Option<i32>,
    nanos: HashMap<i32, super::own_nanos::OwnNanos>,
    db: super::hud_nanodb::NanoDb,
    elapsed: f32,
    pub(super) outbox: Vec<Frame>,
}

/// `PetView_c::UpdateButtons`: MonsterData (455) 96 is a healing pet.
fn primary_code(zone: &Zone, pet: Identity) -> i32 {
    if zone.stat_of(pet.instance, 455) == Some(96) { 12 } else { 7 }
}

/// `FUN_1007c9ac`: Health (27) / Life (1), zero when the pet is not in the tree.
fn health(zone: &Zone, pet: Identity) -> f32 {
    if !zone.dynels.contains_key(&pet.instance) { return 0.0; }
    let max = zone.stat_of(pet.instance, 1).unwrap_or(0);
    if max == 0 { 0.0 } else { zone.stat_of(pet.instance, 27).unwrap_or(0) as f32 / max as f32 }
}

impl HudPet {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        Self { window: None, pets: vec![], texts: TextDb::load(dir).ok(), screen, closed: false, select: None, outbox: vec![], nanos: HashMap::new(), db: super::hud_nanodb::NanoDb::new(dir), elapsed: 0.5 }
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) { self.screen = screen; }
    pub(super) fn take_closed(&mut self) -> bool { std::mem::take(&mut self.closed) }
    pub(super) fn on_frame(&mut self, frame: &Frame) {
        let Ok(m) = ao_net::n3::decode(frame) else { return; };
        if matches!(&m.body,
            ao_net::n3::N3::Misc(ao_net::n3::misc::Misc::Buff(_))
            | ao_net::n3::N3::World(ao_net::n3::world::World::CharacterAction(_))
            | ao_net::n3::N3::Dynel(ao_net::n3::dynel::Dynel::SimpleCharFullUpdate(_)))
        {
            self.nanos.entry(m.header.target.instance).or_default()
                .on_message(m.header.target, m.header.target, &m.body, 100);
        }
    }
    fn text(&self, key: &str) -> String {
        self.texts.as_ref().and_then(|t| t.by_key(10000, key)).unwrap_or_else(|| key.to_string())
    }

    pub(super) fn open(&mut self, gui: &mut Gui) {
        if self.window.is_some() { return; }
        // Original Rect(200,300,600,500), MoveToCenter; flags 0x1000, tab title PetWindow.
        let title = self.text("PetWindow");
        match gui.open_tabbed_window_xml(&title, &title, "<root><View name=\"pet_view\" view_layout=\"vertical\"/></root>", (0, 0), WindowSize::Fixed(401, 201)) {
            Ok(w) => {
                gui.set_window_help(w, Some("The Pet Window.html"));
                let (width, height) = gui.outer_size(w);
                gui.set_window_pos(w, ((self.screen.0 as i32 - width as i32) / 2, (self.screen.1 as i32 - height as i32) / 2));
                self.window = Some(w);
                self.pets.clear();
                self.elapsed = 0.5;
            }
            Err(e) => eprintln!("hud: pet_window: {e:#}"),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui) {
        if let Some(w) = self.window.take() { gui.close_window(w); }
        self.pets.clear();
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, dt: f32) {
        for nanos in self.nanos.values_mut() { nanos.tick(dt); }
        if let Some(id) = self.select.take() { zone.set_target(Some(ao_net::msg::Identity { kind: 50000, instance: id })); }
        let Some(w) = self.window else { return; };
        self.elapsed += dt;
        if self.elapsed < 0.5 && self.pets == zone.pets { return; }
        self.elapsed = 0.0;
        if self.pets != zone.pets {
            gui.remove_children(w, "pet_view");
            let bg = gui.gfx().name(GfxId(0xdb)).unwrap_or_default().to_owned();
            let fill = gui.gfx().name(GfxId(0xdd)).unwrap_or_default().to_owned();
            let size = gui.gfx().size(GfxId(0xdb));
            for (i, _) in zone.pets.iter().enumerate() {
                // PetView: vertical; horizontal 4px spacer, labelled health bar, 4px spacer,
                // primary Attack/Heal, Follow, Wait. Art fixes the stacked bar's preferred size.
                let xml = format!("<root><View view_layout=\"vertical\"><View view_layout=\"horizontal\"><HLayoutSpacer min_size=\"Point(4,0)\"/><View view_layout=\"stacked\" min_size=\"Point({},{})\"><PowerBar name=\"hp{i}\" bg_gfx=\"{bg}\" full_gfx=\"{fill}\"/><TextView name=\"name{i}\" h_alignment=\"center\"/><CanvasView name=\"target{i}\"/></View><HLayoutSpacer min_size=\"Point(4,0)\"/><Button name=\"primary{i}\" label=\"{}\"/><Button name=\"follow{i}\" label=\"{}\"/><Button name=\"wait{i}\" label=\"{}\"/></View><View name=\"buffs{i}\" view_layout=\"horizontal\" layout_borders=\"Rect(6,6,6,6)\"/></View></root>", size.0.saturating_sub(1), size.1.saturating_sub(1), self.text("ButtonPetAttack"), self.text("ButtonPetFollow"), self.text("ButtonPetWait"));
                if let Err(e) = gui.add_view_xml(w, "pet_view", "PetView", &xml) { eprintln!("hud: pet row: {e:#}"); }
            }
            self.pets.clone_from(&zone.pets);
            gui.relayout_window(w);
        }
        for (i, pet) in self.pets.iter().copied().enumerate() {
            let known = zone.dynels.get(&pet.instance);
            let name = known.map_or("", |d| d.name.as_str());
            let name: String = name.chars().filter(|c| *c > '\u{1f}').collect();
            let color = if known.is_some() { "bbbbbb" } else { "808080" };
            gui.set_text(w, &format!("name{i}"), &format!("<font color=#{color}>{}</font>", super::hud_listview::esc(&name)));
            gui.set_progress(w, &format!("hp{i}"), health(zone, pet));
            gui.set_text(w, &format!("primary{i}"), &self.text(if primary_code(zone, pet) == 12 { "ButtonPetHeal" } else { "ButtonPetAttack" }));
            for action in ["primary", "follow", "wait"] { gui.set_enabled(w, &format!("{action}{i}"), known.is_some()); }
            gui.remove_children(w, &format!("buffs{i}"));
            gui.set_visible(w, &format!("buffs{i}"), false);
            if let Some(nanos) = self.nanos.get_mut(&pet.instance) {
                for buff in &nanos.buffs {
                    let Some(info) = self.db.info(gui, buff.nano) else { continue; };
                    let remaining = buff.remaining_cs(nanos.time);
                    let Some((icon, width, height)) = info.icon else { continue; };
                    let name = format!("buff{i}_{}", buff.nano);
                    // PetView's NanoTemplateInfoListView: icon size 3 (16px), 1..100 cells.
                    let xml = format!("<root><CanvasView name=\"{name}\" min_size=\"Point(15,15)\" layout_borders=\"Rect(0,0,7,0)\"/></root>");
                    if gui.add_view_xml(w, &format!("buffs{i}"), "PetBuff", &xml).is_ok() {
                        gui.set_visible(w, &format!("buffs{i}"), true);
                        gui.set_canvas(w, &name, vec![CanvasItem::Image { id: icon, src: [0.0, 0.0, width as f32, height as f32], dst: [0.0, 0.0, 16.0, 16.0], alpha: 1.0 }]);
                        gui.set_tooltip(w, &name, &info.name, &super::hud_listview::hms(remaining));
                    }
                }
            }
        }
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let Some(w) = self.window else { return false; };
        match ev {
            Event::CloseRequested { window } if *window == w => { self.close(gui); self.closed = true; true }
            Event::CanvasPress { window, view, button: MouseButton::Left, .. } if *window == w => {
                let Some(i) = view.strip_prefix("target").and_then(|s| s.parse::<usize>().ok()) else { return false; };
                if let Some(pet) = self.pets.get(i) { self.select = Some(pet.instance); }
                true
            }
            Event::Clicked { window, view, .. } if *window == w => {
                let Some((prefix, i)) = ["primary", "follow", "wait"].iter().find_map(|p| Some((*p, view.strip_prefix(p)?.parse::<usize>().ok()?))) else { return false; };
                let Some(&pet) = self.pets.get(i) else { return true; };
                if !zone.pets.contains(&pet) || !zone.dynels.contains_key(&pet.instance) { return true; }
                let code = match prefix { "follow" => 1, "wait" => 4, _ => primary_code(zone, pet) };
                // Original handlers 1007c919/936/953/970 send one identity, window/arg/tower 0.
                self.outbox.push(ao_net::n3::outgoing::n3_frame(0, zone.char_id, textcmd::pet_command(zone.char_id as i32, 0, code, 0, &[pet], false, "")));
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pet_list_count_and_type_commands_use_real_zone_state() {
        let mut zone = Zone::new(7);
        let pet = Identity { kind: 50000, instance: 8 };
        zone.pets.push(pet);
        zone.stats.insert(458, 99);
        assert_eq!(zone.stat(458), Some(1));
        assert_eq!(primary_code(&zone, pet), 7);
        zone.character_stats.entry(8).or_default().insert(455, 96);
        assert_eq!(primary_code(&zone, pet), 12);
        assert_eq!(health(&zone, pet), 0.0);
        zone.pets.clear();
        assert_eq!(zone.stat(458), Some(0));
    }
    #[test]
    fn pet_window_commands_address_one_pet_and_survives_last_removal() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut hud = HudPet::new(&dir, (1024, 768));
        let mut zone = Zone::new(7);
        let pet = Identity { kind: 50000, instance: 8 };
        zone.pets.push(pet);
        zone.dynels.insert(8, super::super::zone::DynelState { name: "Heal Pet".into(), pos: [0.0; 3], yaw: None, npc: true, side: 0, level: 1, health: 50, max_health: 100 });
        zone.character_stats.insert(8, [(455, 96), (1, 100), (27, 50)].into());
        hud.open(&mut gui);
        hud.update(&mut gui, &mut zone, 0.0);
        let window = hud.window.unwrap();
        assert_eq!(health(&zone, pet), 0.5);
        for (button, code) in [("primary0", 12), ("follow0", 1), ("wait0", 4)] {
            assert!(hud.event(&mut gui, &Event::Clicked { window, view: button.into(), item: None }, &zone));
            let frame = hud.outbox.pop().unwrap();
            assert_eq!(frame.payload, textcmd::pet_command(7, 0, code, 0, &[pet], false, ""));
        }
        zone.pets.clear();
        hud.update(&mut gui, &mut zone, 0.0);
        assert_eq!(hud.window, Some(window));
        assert!(hud.pets.is_empty());
        assert_eq!(super::super::hud::WindowKind::from_dvalue("pet_window"), Some(super::super::hud::WindowKind::Pet));
    }
}
