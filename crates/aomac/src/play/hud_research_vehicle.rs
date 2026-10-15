//! ResearchWindow_c (GUI 100f6bf4) and MechSelect (100f3566), using their shipped Views XML.
//! Dynamic rows: GUI 100f4d9c/100f5de8; native actions: 100f6880/100f6a3d/100f4719/100f4a68/100f3017.
use super::{chat::info, dvalue::element_xml, hud::WindowKind, hud_perks::{HudPerks, PerkDef}, zone::Zone};
use ao_formats::dynel_visual;
use ao_gui::{xml, Event, Gui, WindowId, WindowSize};
use ao_net::{frame::Frame, n3::{outgoing, research}};
use std::{collections::{BTreeMap, HashMap}, path::Path};

const PAGES: &str = "research_pages";
const TITLES: [&str; 3] = ["Allocate Research", "Available Research", "Finished Research"];

fn quote(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

struct Item {
    description: String,
    icon: Option<ao_gui::GfxId>,
    can: i32,
    criteria: info::PerkCriteria,
}

#[derive(Default)]
pub(super) struct HudResearchVehicle {
    research: Option<WindowId>,
    vehicle: Option<WindowId>,
    research_xml: String,
    items: HashMap<u32, Item>,
    known: BTreeMap<i32, i32>,
    global: BTreeMap<i32, [i32; 3]>,
    stats: HashMap<u32, i32>,
    equipment: info::PerkEquipment,
    dirty: bool,
    request: bool,
    selected_vehicle: Option<u32>,
    selected_line: [Option<u32>; 2],
    closed: Vec<WindowKind>,
    pub(super) outbox: Vec<Frame>,
    pub(super) lines: Vec<String>,
    pub(super) guard: super::vehicle_guard::Context,
    vehicle_playfield: Option<u32>,
    texts: Option<ao_formats::screens::TextDb>,
}

impl HudResearchVehicle {
    pub(super) fn new(dir: &Path) -> anyhow::Result<Self> {
        let mut src = format!("<root><ViewSelector name=\"{PAGES}\">");
        for (index, name) in ["Research", "ResearchDetails", "ResearchFinished"].iter().enumerate() {
            let path = dir.join("cd_image/gui/Default/Views").join(format!("{name}.xml"));
            let mut root = xml::parse(&std::fs::read_to_string(path)?)?;
            let page = root.children.first_mut().ok_or_else(|| anyhow::anyhow!("{name}: missing View"))?;
            page.attrs.retain(|(key, _)| key != "name");
            page.attrs.push(("name".into(), format!("research_page{index}")));
            // Native pages have separate info/finished-list instances. Keep their names unambiguous in the shared tree.
            fn rename(e: &mut xml::Element, index: usize) {
                if e.name == "TextView" {
                    let heading = match e.attr("value") {
                        Some("Personal Research") => Some("personal_heading"),
                        Some("Global Research") => Some("global_heading"),
                        Some("Started Personal Research") => Some("personal_started_heading"),
                        Some("Current Global Research Goal") => Some("global_started_heading"),
                        Some("Personal Research Progress" | "Completed Personal Research Projects") => Some("personal_heading"),
                        Some("Global Research Progress" | "Completed Global Research Projects") => Some("global_heading"),
                        Some("Selected Research Line") => Some("selected_heading"),
                        _ => None,
                    };
                    if let Some(heading) = heading { e.attrs.push(("name".into(), format!("page{index}_{heading}"))); }
                }
                for (key, value) in &mut e.attrs {
                    if key == "name" && matches!(value.as_str(), "info" | "PerkLine" | "finishedPersonal" | "finishedGlobal") {
                        *value = format!("page{index}_{value}");
                    }
                    if key == "width_group_owner" && value == "parent" {
                        *value = format!("research_page{index}");
                    }
                }
                for child in &mut e.children { rename(child, index); }
            }
            rename(page, index);
            src += &element_xml(page);
        }
        src += "</ViewSelector></root>";
        Ok(Self { research_xml: src, texts: Some(ao_formats::screens::TextDb::load(dir)?), ..Self::default() })
    }

    pub(super) fn open(&mut self, gui: &mut Gui, kind: WindowKind, screen: (u32, u32)) {
        let slot = match kind { WindowKind::Research => &mut self.research, WindowKind::Vehicle => &mut self.vehicle, _ => return };
        if slot.is_some() { return; }
        // Both ctors use Rect(200,180,850,700), then MoveToCenter. Insets (5,26,5,5).
        let pos = (((screen.0 as i32 - 651) / 2).max(0), ((screen.1 as i32 - 521) / 2).max(0));
        let result = if kind == WindowKind::Research {
            gui.open_tabbed_window_xml("ResearchWindow", TITLES[0], &self.research_xml, pos, WindowSize::Fixed(641, 490))
        } else { gui.open_tabbed_window("MechSelect", "Vehicle", pos, WindowSize::Fixed(641, 490)) };
        match result {
            Ok(w) => {
                *slot = Some(w);
                if kind == WindowKind::Research {
                    gui.set_window_tabs(w, &TITLES.map(str::to_owned), 0);
                    gui.select_child(w, PAGES, Some(0));
                    gui.set_window_help(w, Some("The Research window.html"));
                    // GUI 100f6bf4 overrides the finished info renderer's authored 80px minimum.
                    gui.set_text_pref_size(w, "page2_info", (0.0, 20.0), (16000.0, 16000.0));
                } else {
                    gui.set_window_help(w, Some("The Mech Select window.html"));
                    // The XML's HelloKittyMech is a designer sample. Retail deletes it before enumerating completed global goals.
                    gui.remove_children(w, "MechButtons");
                    gui.set_enabled(w, "OrderVehicle", false);
                    self.selected_vehicle = None;
                }
                self.dirty = true;
                self.request = true;
            }
            Err(e) => eprintln!("hud: {}: {e:#}", kind.dvalue()),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, kind: WindowKind) {
        let slot = match kind { WindowKind::Research => &mut self.research, WindowKind::Vehicle => &mut self.vehicle, _ => return };
        if let Some(w) = slot.take() { gui.close_window(w); }
    }

    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> { std::mem::take(&mut self.closed) }

    fn load_items(&mut self, gui: &mut Gui, perks: &HudPerks) -> anyhow::Result<()> {
        let store = perks.store.as_ref().ok_or_else(|| anyhow::anyhow!("research requires client RDB"))?;
        for def in perks.db.defs.iter().filter(|d| d.flags & 0xc0 != 0) {
            if self.items.contains_key(&def.id) { continue; }
            let record = store.get(dynel_visual::ITEM_TEMPLATE_TYPE, def.item)?.ok_or_else(|| anyhow::anyhow!("missing research item {}", def.item))?;
            let template = dynel_visual::parse_item_template(&record)?;
            self.items.insert(def.id, Item {
                description: info::item_description(&record)?.unwrap_or_default(),
                icon: template.stat(79).filter(|id| *id != 1_234_567_890).and_then(|id| super::hud_bar::icon_image(gui, store, id as u32).map(|v| v.0)), can: template.stat(177).unwrap_or(0),
                criteria: info::PerkCriteria::from_record(&record)?,
            });
        }
        Ok(())
    }


    fn eligible(&self, item: &Item, zone: &Zone) -> anyhow::Result<bool> {
        item.criteria.eligible(zone, self.equipment.armor(), zone.completed_perks.as_deref().unwrap_or_default())
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, perks: &mut HudPerks) {
        if self.research.is_none() && self.vehicle.is_none() { return; }
        if self.vehicle_playfield != zone.playfield {
            self.vehicle_playfield = zone.playfield;
            self.guard.vehicles_allowed = zone.playfield.and_then(|id| perks.store.as_ref().and_then(|store| match ao_formats::playfield::vehicles_allowed(store, id) {
                Ok(allowed) => Some(allowed),
                Err(e) => { eprintln!("vehicle playfield: {e:#}"); None }
            }));
        }
        if self.request {
            self.outbox.push(outgoing::n3_frame(0, zone.char_id, research::request(zone.char_id as i32)));
            self.request = false;
        }
        let armor_changed = match perks.store.as_ref().ok_or_else(|| anyhow::anyhow!("research requires client RDB")).and_then(|store| self.equipment.refresh(store, zone)) { Ok(changed) => changed, Err(e) => { eprintln!("research equipment: {e:#}"); return; } };
        let stats_changed = self.stats.len() != zone.stats.len() + zone.skill_values.keys().filter(|key| !zone.stats.contains_key(key)).count()
            || zone.stats.iter().chain(zone.skill_values.iter()).any(|(key, _)| self.stats.get(key).copied() != zone.skill_value(*key));
        let changed = armor_changed || self.dirty || self.known != perks.known.0 || self.global != zone.research || stats_changed;
        if !changed { return; }
        if let Err(e) = self.load_items(gui, perks) { eprintln!("research: {e:#}"); return; }
        self.known.clone_from(&perks.known.0);
        self.global.clone_from(&zone.research);
        self.stats.clone_from(&zone.stats);
        self.stats.extend(zone.skill_values.iter().map(|(&k, &v)| (k, v)));
        self.dirty = false;
        if let Some(w) = self.research {
            for (name, stat) in [("PersonalResearch", 263), ("GlobalResearch", 264)] {
                let value = zone.skill_value(stat).unwrap_or(0);
                gui.set_slider_value(w, &format!("{name}Slider"), f64::from(value));
                gui.set_text(w, &format!("{name}Lbl"), &format!("{value}%"));
            }
            if let Err(e) = self.rebuild_research(gui, w, zone, perks) { eprintln!("research rows: {e:#}"); }
        }
        if let Some(w) = self.vehicle {
            if let Err(e) = self.rebuild_vehicle(gui, w, zone, perks) { eprintln!("vehicle rows: {e:#}"); }
        }
    }

    fn total(def: &PerkDef) -> i32 {
        def.research_total.wrapping_mul(if def.flags & 0x80 != 0 { 1000 } else { 1 }) as i32
    }

    fn remaining(&self, def: &PerkDef, side: usize) -> Option<i32> {
        if def.flags & 0x80 != 0 {
            self.global.get(&(def.id as i32)).and_then(|v| v.get(side)).map(|&remaining| if remaining < 0 { Self::total(def) } else { remaining })
        }
        else { self.known.get(&(def.id as i32)).copied() }
    }

    fn progress(&self, def: &PerkDef, side: usize) -> f32 {
        let Some(remaining) = self.remaining(def, side) else { return 0.0 };
        let total = f64::from(Self::total(def));
        ((total - f64::from(remaining)) / total) as f32
    }

    fn available(&self, def: &PerkDef, side: usize) -> bool {
        let unfinished = if def.flags & 0x80 != 0 { self.remaining(def, side).is_some_and(|r| r > 0) }
            else { self.remaining(def, side) != Some(0) };
        unfinished && (def.precursor == 0 || if def.flags & 0x80 != 0 {
            self.global.get(&(def.precursor as i32)).and_then(|v| v.get(side)) == Some(&0)
        } else { self.known.get(&(def.precursor as i32)) == Some(&0) })
    }

    /// GUI 100f4d9c: PowerbarView_c(0xe0,0xe1), then label and HLayoutSpacer, row borders(8,2,0,2).
    fn row_xml(name: &str, label: &str) -> String {
        format!("<View name=\"{name}\" view_layout=\"horizontal\" layout_borders=\"Rect(8,2,0,2)\"><PowerBar name=\"{name}_bar\" bg_gfx=\"GFX_GUI_HOR_BAR_SMALL_EMPTY\" full_gfx=\"GFX_GUI_HOR_BAR_SMALL_BLUE\" layout_borders=\"Rect(5,0,0,0)\"/><TextView name=\"{name}_label\" value=\"{}\" layout_borders=\"Rect(5,1,1,1)\"/><HLayoutSpacer max_size=\"Point(16000,-1)\"/></View>", quote(label))
    }

    #[allow(clippy::too_many_arguments)]
    fn add_row(&self, gui: &mut Gui, w: WindowId, parent: &str, def: &PerkDef, side: usize, name: &str, label: &str) -> anyhow::Result<()> {
        gui.add_view_xml(w, parent, name, &format!("<root>{}</root>", Self::row_xml(name, label)))?;
        gui.set_progress(w, &format!("{name}_bar"), self.progress(def, side));
        gui.set_view_mouse(w, name, true);
        Ok(())
    }

    fn rebuild_research(&mut self, gui: &mut Gui, w: WindowId, zone: &Zone, perks: &mut HudPerks) -> anyhow::Result<()> {
        for parent in ["StartedPersonal", "StartedGlobal", "personalRadio", "globalRadio", "page2_finishedPersonal", "page2_finishedGlobal"] { gui.remove_children(w, parent); }
        let side = zone.skill_value(33).unwrap_or(0) as usize;
        let profession = zone.skill_value(60).unwrap_or(0);
        for index in 0..perks.db.defs.len() {
            let def = perks.db.defs[index];
            let personal = def.flags & 0x40 != 0;
            if !personal && def.flags & 0x80 == 0 { continue; }
            let Some(item) = self.items.get(&def.id) else { continue };
            if personal && !item.criteria.profession_matches(profession) { continue; }
            let label = perks.name(def.id);
            if self.available(&def, side) {
                let group = if personal { "personalRadio" } else { "globalRadio" };
                let name = format!("goal_{}_{}", if personal { "personal" } else { "global" }, def.id);
                let row_name = format!("hover_{}", def.id);
                let row = Self::row_xml(&row_name, &label);
                gui.add_view_xml(w, group, &name, &format!("<root><RadioButton name=\"{name}\" value=\"{}\">{row}</RadioButton></root>", def.id))?;
                gui.set_progress(w, &format!("{row_name}_bar"), self.progress(&def, side));
                let enabled = if personal { self.eligible(item, zone)? } else { true };
                gui.set_enabled(w, &name, enabled);
                gui.set_view_mouse(w, &format!("hover_{}", def.id), true);
                let progress = self.progress(&def, side);
                if personal && progress != 0.0 || !personal && zone.skill_value(266) == Some(def.id as i32) {
                    self.add_row(gui, w, if personal { "StartedPersonal" } else { "StartedGlobal" }, &def, side, &format!("started_{}", def.id), &label)?;
                }
            }
            if self.remaining(&def, side) == Some(0) {
                // GetCompletedPersonalResearchGoals retains the highest completed project of each line.
                let later_complete = personal && perks.db.defs.iter().any(|next| next.precursor == def.id && next.flags & 0x40 != 0 && self.remaining(next, side) == Some(0));
                if !later_complete { self.add_row(gui, w, if personal { "page2_finishedPersonal" } else { "page2_finishedGlobal" }, &def, side, &format!("finished_{}", def.id), &label)?; }
            }
        }
        gui.set_radio_value(w, "personalRadio", zone.skill_value(265).unwrap_or(0));
        gui.set_radio_value(w, "globalRadio", zone.skill_value(266).unwrap_or(0));
        for page in 0..2 { if let Some(id) = self.selected_line[page] { self.show_line(gui, w, page + 1, id, side, perks)?; } }
        Ok(())
    }

    fn rebuild_vehicle(&mut self, gui: &mut Gui, w: WindowId, zone: &Zone, perks: &mut HudPerks) -> anyhow::Result<()> {
        gui.remove_children(w, "MechButtons");
        let side = zone.skill_value(33).unwrap_or(0) as usize;
        for index in 0..perks.db.defs.len() {
            let def = perks.db.defs[index];
            if def.flags & 0x80 == 0 || self.remaining(&def, side) != Some(0) { continue; }
            let Some(item) = self.items.get(&def.id) else { continue };
            if item.can & 0x200 == 0 { continue; }
            let enabled = self.eligible(item, zone)?;
            let icon = item.icon;
            let label = perks.name(def.id);
            gui.add_view_xml(w, "MechButtons", "VehicleButton", &format!("<root><Button name=\"vehicle_{}\" label=\"{}\" layout_borders=\"Rect(5,5,5,5)\" width_group=\"Buttons\" width_group_owner=\"MechButtons\"/></root>", def.id, quote(&label)))?;
            gui.set_button_background_icon(w, &format!("vehicle_{}", def.id), icon);
            gui.set_enabled(w, &format!("vehicle_{}", def.id), enabled);
        }
        gui.select_child(w, "mode", Some(usize::from(zone.skill_value(662).unwrap_or(0) != 0)));
        gui.set_enabled(w, "OrderVehicle", self.selected_vehicle.is_some());
        Ok(())
    }

    fn show_line(&mut self, gui: &mut Gui, w: WindowId, page: usize, id: u32, side: usize, perks: &mut HudPerks) -> anyhow::Result<()> {
        let parent = format!("page{page}_PerkLine");
        gui.remove_children(w, &parent);
        let Some(mut root) = perks.db.get(id).copied() else { return Ok(()) };
        let mut seen = vec![root.id];
        while root.precursor != 0 {
            let Some(previous) = perks.db.get(root.precursor).copied() else { break };
            anyhow::ensure!(!seen.contains(&previous.id), "cyclic research line");
            seen.push(previous.id);
            root = previous;
        }
        seen.clear();
        loop {
            anyhow::ensure!(!seen.contains(&root.id), "cyclic research line");
            seen.push(root.id);
            let label = perks.name(root.id);
            self.add_row(gui, w, &parent, &root, side, &format!("line{page}_{}", root.id), &label)?;
            let Some(next) = perks.db.defs.iter().find(|next| next.precursor == root.id).copied() else { break };
            root = next;
        }
        gui.add_view_xml(w, &parent, "ResearchLineSpacer", "<root><VLayoutSpacer max_size=\"Point(-1,16000)\"/></root>")?;
        self.selected_line[page - 1] = Some(id);
        Ok(())
    }

    fn set_stat(&mut self, zone: &Zone, stat: i32, value: i32) { self.outbox.push(outgoing::n3_frame(0, zone.char_id, outgoing::set_stat(zone.char_id as i32, stat, value))); }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone, perks: &mut HudPerks, dvalues: &mut super::dvalue::DValues) -> bool {
        let w = match ev {
            Event::Clicked { window, .. } | Event::CloseRequested { window } | Event::TabSelected { window, .. } | Event::SliderChanged { window, .. } | Event::ViewMouse { window, .. } | Event::ViewHover { window, .. } => *window,
            _ => return false,
        };
        if Some(w) != self.research && Some(w) != self.vehicle { return false; }
        let kind = if Some(w) == self.research { WindowKind::Research } else { WindowKind::Vehicle };
        match ev {
            Event::CloseRequested { .. } => self.closed.push(kind),
            Event::Clicked { view, .. } if view == "Quit" => self.closed.push(kind),
            Event::TabSelected { index, .. } if kind == WindowKind::Research => gui.select_child(w, PAGES, Some((*index).min(2))),
            Event::SliderChanged { view, value, .. } if kind == WindowKind::Research => {
                let (stat, other, other_name) = match view.as_str() {
                    "PersonalResearchSlider" => (263, 264, "GlobalResearch"),
                    "GlobalResearchSlider" => (264, 263, "PersonalResearch"),
                    _ => return false,
                };
                let value = value.clamp(0.0, 100.0) as i32;
                let opposite = gui.slider_value(w, &format!("{other_name}Slider")).unwrap_or(0.0) as i32;
                if value + opposite > 100 {
                    gui.set_slider_value(w, &format!("{other_name}Slider"), f64::from(100 - value));
                    gui.set_text(w, &format!("{other_name}Lbl"), &format!("{}%", 100 - value));
                    self.set_stat(zone, other, 100 - value);
                }
                gui.set_text(w, if stat == 263 { "PersonalResearchLbl" } else { "GlobalResearchLbl" }, &format!("{value}%"));
                self.set_stat(zone, stat, value);
            }
            Event::Clicked { view, .. } if kind == WindowKind::Research && view.starts_with("goal_") => {
                let personal = view.starts_with("goal_personal_");
                let group = if personal { "personalRadio" } else { "globalRadio" };
                if let Some(id) = gui.radio_value(w, group) { self.set_stat(zone, if personal { 265 } else { 266 }, id); }
            }
            Event::Clicked { view, .. } if kind == WindowKind::Vehicle && view.starts_with("vehicle_") => {
                if let Ok(id) = view[8..].parse::<u32>() {
                    self.selected_vehicle = Some(id);
                    gui.set_text(w, "VehicleInfo", self.items.get(&id).map_or("", |item| item.description.as_str()));
                    gui.set_enabled(w, "OrderVehicle", true);
                }
            }
            Event::Clicked { view, .. } if kind == WindowKind::Vehicle && matches!(view.as_str(), "OrderVehicle" | "LeaveVehicle") => {
                let id = if view == "LeaveVehicle" { Some(0) } else { self.selected_vehicle };
                if let Some(id) = id {
                    self.guard.skill_locked = zone.skill_locked(662);
                    self.guard.resurrection_shock = Some(zone.skill_value(247).unwrap_or(0));
                    self.guard.battlestation = zone.playfield.map(|id| (0x111c..0x1125).contains(&id));
                    self.guard.own_lca = zone.own_lca;
                    let flags = perks.db.get(id).map_or(0, |def| def.flags);
                    match super::vehicle_guard::check(id as i32, flags, &self.guard) {
                        Ok(super::vehicle_guard::Verdict::Allowed) => {
                            let eligible = if id == 0 { Ok(true) } else {
                                self.items.get(&id).ok_or_else(|| anyhow::anyhow!("missing selected vehicle catalogue item {id}")).and_then(|item| self.eligible(item, zone))
                            };
                            match eligible {
                                Ok(true) => self.outbox.push(outgoing::n3_frame(0, zone.char_id, research::spawn_mech(zone.char_id as i32, id as i32))),
                                Ok(false) => {}
                                Err(e) => eprintln!("vehicle eligibility: {e:#}"),
                            }
                        }
                        Ok(super::vehicle_guard::Verdict::Feedback(key)) => {
                            if let Some(text) = self.texts.as_ref().and_then(|texts| texts.by_key(110, key)) { self.lines.push(text); }
                        }
                        Ok(super::vehicle_guard::Verdict::Silent) => {}
                        Err(e) => eprintln!("vehicle activation: {e:#}"),
                    }
                    if view == "OrderVehicle" {
                        if let Some(time) = zone.server_now() { dvalues.set_i64("mech_ordered", i64::from(time)); }
                    }
                    self.closed.push(kind);
                }
            }
            Event::ViewHover { view, entered, .. } if kind == WindowKind::Research => {
                let id = view.rsplit('_').next().and_then(|s| s.parse::<u32>().ok());
                let description = if *entered { id.and_then(|id| self.items.get(&id)).map_or("", |item| item.description.as_str()) } else { "" };
                for name in ["page1_info", "page2_info"] { gui.set_text(w, name, description); }
            }
            Event::ViewMouse { view, .. } if kind == WindowKind::Research => {
                if let Some(id) = view.rsplit('_').next().and_then(|s| s.parse::<u32>().ok()) {
                    let page = gui.window_tabs(w).1;
                    if page != 0 { if let Err(e) = self.show_line(gui, w, page, id, zone.skill_value(33).unwrap_or(0) as usize, perks) { eprintln!("research line: {e:#}"); } }
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn def(id: u32, precursor: u32, flags: u32, total: u32) -> PerkDef { PerkDef { id, item: 0, precursor, flags, mask: 0, action: 0, research_total: total } }
    #[test]
    fn native_goal_availability_progress_and_global_sides() {
        let mut state = HudResearchVehicle::default();
        let personal = def(2, 1, 0x40, 100);
        assert!(!state.available(&personal, 0));
        state.known.insert(1, 0);
        assert!(state.available(&personal, 0));
        state.known.insert(2, 75);
        assert_eq!(state.progress(&personal, 0), 0.25);
        state.known.insert(2, 0);
        assert!(!state.available(&personal, 0));
        let global = def(4, 3, 0x80, 100);
        state.global.insert(3, [0, 1, 0]);
        state.global.insert(4, [25_000, 50_000, 0]);
        assert!(state.available(&global, 0));
        assert!(!state.available(&global, 1));
        assert!(!state.available(&global, 2));
        assert_eq!(state.progress(&global, 0), 0.75);
        assert_eq!(state.progress(&global, 1), 0.5);
        state.global.insert(4, [-1, -1, -1]);
        assert!(state.available(&global, 0), "1002bc59 initializes an existing negative timer to its native total");
        assert_eq!(state.progress(&global, 0), 0.0);
        state.global.remove(&4);
        assert!(!state.available(&global, 0), "absent server data never manufactures a goal");
    }
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    #[test]
    fn native_research_vehicle_assets_and_allocation_dispatch() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Views/Research.xml").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut window = HudResearchVehicle::new(&dir).unwrap();
        let mut perks = HudPerks::new(&dir, (1280, 800));
        let zone = Zone::new(42);
        let mut values = super::super::dvalue::DValues::new(&dir);
        window.open(&mut gui, WindowKind::Research, (1280, 800));
        let research = window.research.unwrap();
        assert_eq!(gui.window_tabs(research).0, TITLES.map(str::to_owned));
        assert!(gui.view_names(research).iter().any(|name| name == "PersonalResearchSlider"));
        gui.set_slider_value(research, "GlobalResearchSlider", 70.0);
        gui.set_slider_value(research, "PersonalResearchSlider", 80.0);
        assert!(window.event(&mut gui, &Event::SliderChanged { window: research, view: "PersonalResearchSlider".into(), value: 80.0, final_change: true }, &zone, &mut perks, &mut values));
        assert_eq!(gui.slider_value(research, "GlobalResearchSlider"), Some(20.0));
        assert_eq!(window.outbox.len(), 2, "native sum guard dispatches both changed allocation stats");
        window.open(&mut gui, WindowKind::Vehicle, (1280, 800));
        let vehicle = window.vehicle.unwrap();
        assert!(!gui.view_names(vehicle).iter().any(|name| name == "Mech 1"), "retail removes the authored designer sample");
        assert!(window.items.is_empty(), "opening an asset never manufactures server-completed research");
    }
}
