//! MissionSelectionView_c, constructed in code (GUI 0x100d06af; no retail XML).
//! List 3x2, difficulty 50, six signed dimensions 0, advanced toggle, request/accept/cancel.
use super::{hud_bar::icon_image, hud_dialog::Dialogs, hud_listview::{self as lv, Hit, ListView, Mode, Row}, zone::Zone};
use ao_formats::screens::TextDb;
use ao_gui::{view::CanvasItem, Event, FontId, GfxId, Gui, MenuItem, MouseButton, WindowId, WindowSize};
use ao_net::{msg::Identity, n3::{inventory::{BAG_FIRST, BAG_SLOTS}, mission_selection::{self, Alternatives, GenerateInfo}}};
use ao_rdb::RecordStore;
use super::dvalue::{element_xml, DValues, Variant};

const LABELS: [(&str, &str); 7] = [("QuestSel_Easy", "QuestSel_Hard"), ("QuestSel_Good", "QuestSel_Bad"), ("QuestSel_Order", "QuestSel_Chaos"), ("QuestSel_Open", "QuestSel_Hidden"), ("QuestSel_Phys", "QuestSel_Myst"), ("QuestSel_HeadOn", "QuestSel_Stealth"), ("QuestSel_Credits", "QuestSel_XP")];
const BG: &str = "GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER_BACKGROUND";
const KNOB: &str = "GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER";
const LOOK: u32 = 0x4d53_0001;
const MAP: u32 = 0x4d53_0002;
const CONFIG: &str = "MissionSelectionViewConfig";
const FIELDS: [&str; 7] = ["difficulty", "dim_good", "dim_ctrl", "dim_secret", "dim_mystery", "dim_stealth", "dim_reward"];

pub(super) struct MissionUi {
    win: Option<WindowId>,
    values: [i32; 7],
    advanced: bool,
    origin: Identity,
    origin_type: u8,
    list: ListView,
    alternatives: Option<Alternatives>,
    grab: Option<(usize, f32)>,
    urls: Vec<String>,
    marker: Option<(u32, [f32; 2])>,
    popup: Option<i32>,
    dialogs: Dialogs<()>,
    screen: (u32, u32),
    config_loaded: bool,
    dirty: bool,
    escape: bool,
}
impl Default for MissionUi {
    fn default() -> Self {
        Self { win: None, values: [50, 0, 0, 0, 0, 0, 0], advanced: false, origin: Identity::default(), origin_type: 0, list: ListView::new(Mode::Grid, vec![], 3, 2, (0, false)), alternatives: None, grab: None, urls: vec![], marker: None, popup: None, dialogs: Dialogs::default(), screen: (0, 0), config_loaded: false, dirty: false, escape: true }
    }
}
fn escape(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }
fn knob(value: i32, index: usize, width: f32) -> f32 {
    let min = if index == 0 { 0 } else { -100 };
    ((value - min) as f32 * (width - 11.0).max(0.0) / (100 - min) as f32).floor()
}
impl MissionUi {
    pub(super) fn load_config(&mut self, d: &DValues) {
        self.escape = d.get_i64("esc_missionselection").unwrap_or(1) != 0;
        if self.config_loaded || self.win.is_some() { return; }
        self.config_loaded = true;
        let Some(Variant::Archive(src)) = d.get(CONFIG) else { return };
        let Ok(e) = ao_gui::xml::parse(src) else { return };
        for (i, key) in FIELDS.iter().enumerate() {
            if let Some(v) = e.children.iter().find(|e| e.attr("name") == Some(*key)).and_then(|e| e.attr("value")).and_then(|s| s.parse::<i32>().ok()) {
                self.values[i] = v.clamp(if i == 0 { 0 } else { -100 }, 100);
            }
        }
        self.advanced = e.children.iter().find(|e| e.attr("name") == Some("show_advanced_options")).and_then(|e| e.attr("value")).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    }
    pub(super) fn save_config(&mut self, d: &mut DValues) {
        if !self.dirty { return; }
        self.dirty = false;
        let mut e = match d.get(CONFIG) {
            Some(Variant::Archive(src)) => ao_gui::xml::parse(src).ok(),
            _ => None,
        }.unwrap_or_else(|| ao_gui::xml::Element { name: "Archive".into(), attrs: vec![("name".into(), CONFIG.into())], children: vec![] });
        for (tag, key, value) in FIELDS.iter().enumerate().map(|(i, k)| ("Int32", *k, self.values[i].to_string())).chain(std::iter::once(("Bool", "show_advanced_options", self.advanced.to_string()))) {
            let child = ao_gui::xml::Element { name: tag.into(), attrs: vec![("name".into(), key.into()), ("value".into(), value)], children: vec![] };
            if let Some(old) = e.children.iter_mut().find(|e| e.attr("name") == Some(key)) { *old = child; }
            else { e.children.push(child); }
        }
        d.set(CONFIG, Variant::Archive(element_xml(&e)));
    }
    fn text(key: &str) -> String {
        static TEXTS: std::sync::LazyLock<Option<TextDb>> = std::sync::LazyLock::new(|| TextDb::load(&ao_gui::client_dir()).ok());
        TEXTS.as_ref().and_then(|t| t.by_key(10000, key)).unwrap_or_else(|| key.into())
    }
    pub(super) fn open(&mut self, gui: &mut Gui, screen: (u32, u32), origin_type: u8, origin: Identity) -> anyhow::Result<()> {
        if self.win.is_some() { return Ok(()); }
        self.screen = screen;
        self.origin = origin;
        self.origin_type = origin_type;
        self.alternatives = None;
        self.list.selected = None;
        self.list.set_rows(vec![]);
        let labels: Vec<_> = LABELS.iter().map(|(a, b)| (Self::text(a), Self::text(b))).collect();
        // MSSlider::GetLabelWidth 0x100d16de: widest of all endpoint labels + 5.
        let label_width = labels.iter().flat_map(|(a, b)| [a, b]).map(|s| s.chars().map(|c| gui.fonts().font(FontId::Normal).advance(c)).sum::<i32>()).max().unwrap_or(0) + 5;
        let slider = |i: usize| format!("<View view_layout=\"horizontal\"><TextView value=\"{}\" min_size=\"Point({label_width},17)\" max_size=\"Point({label_width},17)\"/><CanvasView name=\"slider{i}\" min_size=\"Point(127,17)\"/><TextView value=\"{}\" min_size=\"Point({label_width},17)\" max_size=\"Point({label_width},17)\"/></View>", escape(&labels[i].0), escape(&labels[i].1));
        let mut advanced = String::from("<View name=\"advanced\" view_layout=\"vertical\">");
        for i in 1..7 { advanced += &slider(i); }
        advanced += "</View>";
        let (ew, eh) = gui.gfx().size(GfxId(gui.gfx_id("GFX_GUI_BUTTON_MISSION_MORE").unwrap_or(0x47)));
        let xml = format!("<root><View name=\"missionselection_window\" view_layout=\"vertical\" min_size=\"Point(167,112)\">{}{}<View view_layout=\"horizontal\"><Button name=\"request\" label=\"#RequestMissions\" layout_borders=\"Rect(5,5,5,5)\"/><HLayoutSpacer/><CanvasView name=\"expand\" min_size=\"Point({},{})\" max_size=\"Point({},{})\" layout_borders=\"Rect(5,5,5,5)\"/></View>{advanced}<View view_layout=\"horizontal\"><Button name=\"accept\" label=\"#AcceptMission\" layout_borders=\"Rect(5,5,5,5)\"/><HLayoutSpacer/><Button name=\"cancel\" label=\"#MsgBox_Cancel\" layout_borders=\"Rect(5,5,5,5)\"/></View></View></root>", lv::scroll_xml(73.0, lv::grid_width(3, lv::ICON_32)), slider(0), ew - 1, eh - 1, ew - 1, eh - 1);
        let win = gui.open_window_xml("MissionSelectionView", &xml, (0, 0), WindowSize::Preferred)?;
        gui.set_window_dock_frame(win, Some("Missions"));
        gui.show_collapsing(win, "advanced", self.advanced);
        gui.set_enabled(win, "accept", false);
        self.win = Some(win);
        self.list.paint(gui, win, lv::ICON_32);
        gui.resize_window(win, WindowSize::Preferred);
        let (w, h) = gui.outer_size(win);
        gui.set_window_pos(win, ((screen.0 as i32 - w as i32) / 2, (screen.1 as i32 - h as i32) / 2));
        self.paint_sliders(gui);
        Ok(())
    }
    fn paint_sliders(&self, gui: &mut Gui) {
        let Some(win) = self.win else { return };
        let art = if self.advanced { "GFX_GUI_BUTTON_MISSION_LESS" } else { "GFX_GUI_BUTTON_MISSION_MORE" };
        if let Some(id) = gui.gfx_id(art).map(GfxId) {
            let (w, h) = gui.gfx().size(id);
            gui.set_canvas(win, "expand", vec![CanvasItem::ImageTint { id, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, w as f32, h as f32], color: 0x1000000, alpha: 1.0 }]);
        }
        for i in 0..7 {
            let name = format!("slider{i}");
            let Some(r) = gui.view_rect(win, &name) else { continue };
            let width = r.width() + 1.0;
            let mut items = vec![];
            for (art, x, w, tint) in [(BG, 0.0, width, false), (KNOB, knob(self.values[i], i, width), 11.0, true)] {
                if let Some(id) = gui.gfx_id(art).map(GfxId) {
                    let (sw, sh) = gui.gfx().size(id);
                    let src = [0.0, 0.0, sw as f32, sh as f32];
                    let dst = [x, 0.0, x + w, 18.0];
                    items.push(if tint { CanvasItem::ImageTint { id, src, dst, color: 0x1000000, alpha: 1.0 } } else { CanvasItem::Image { id, src, dst, alpha: 1.0 } });
                }
            }
            gui.set_canvas(win, &name, items);
        }
    }
    pub(super) fn alternatives(&mut self, gui: &mut Gui, alternatives: Alternatives) -> Option<String> {
        let win = self.win?;
        let store = RecordStore::open(&ao_gui::client_dir()).ok();
        let rows = alternatives.missions.iter().map(|a| {
            let q = &a.quest;
            let icon = store.as_ref().and_then(|s| u32::try_from(q.icon).ok().filter(|i| *i > 0).and_then(|i| icon_image(gui, s, i)));
            Row { key: q.id.instance, icon, tip_title: q.name.clone(), ..Default::default() }
        }).collect();
        let empty = alternatives.missions.is_empty();
        self.alternatives = Some(alternatives);
        self.list.selected = None;
        self.list.set_rows(rows);
        self.list.paint(gui, win, lv::ICON_32);
        gui.set_enabled(win, "accept", false);
        empty.then(|| "No missions available, please try again later!".into())
    }
    fn enough_room(zone: &Zone) -> bool { (BAG_FIRST..BAG_FIRST + BAG_SLOTS).filter(|s| !zone.inventory.contains_key(s)).take(2).count() == 2 }
    fn refusal(&mut self, gui: &mut Gui, key: &str) {
        self.dialogs.go(gui, self.screen, (), &Self::text(key), &[Self::text("MsgBox_OK")]);
    }
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, own: Identity, zone: &Zone) -> Option<Vec<Vec<u8>>> {
        if self.dialogs.event(gui, ev).0 { return Some(vec![]); }
        let win = self.win?;
        if let Event::MenuPicked { id } = ev {
            if matches!(*id, LOOK | MAP) {
                if let Some(q) = self.popup.take().and_then(|k| self.alternatives.as_ref()?.missions.iter().find(|a| a.quest.id.instance == k)).map(|a| &a.quest) {
                    if *id == LOOK { self.urls.push(format!("itemid://{}/{}", q.id.kind, q.id.instance)); }
                    else if let Some(p) = q.world_pos() { self.marker = Some((p.playfield.instance as u32, [p.x as f32, p.z as f32])); }
                }
                return Some(vec![]);
            }
        }
        if let Some(hit) = self.list.event(gui, win, ev, 0.0, lv::ICON_32) {
            let key = match hit { Hit::Click(k) | Hit::Double(k) | Hit::Context(k, _) => k };
            gui.set_enabled(win, "accept", true);
            self.list.paint(gui, win, lv::ICON_32);
            if let Some(q) = self.alternatives.as_ref().and_then(|a| a.missions.iter().find(|a| a.quest.id.instance == key)).map(|a| &a.quest) {
                if let Some(p) = q.world_pos() { self.marker = Some((p.playfield.instance as u32, [p.x as f32, p.z as f32])); }
                match hit {
                    Hit::Double(_) => self.urls.push(format!("itemid://{}/{}", q.id.kind, q.id.instance)),
                    Hit::Context(_, at) => {
                        let mut items = vec![MenuItem::entry(LOOK, &Self::text("Look@Mission"))];
                        if q.world_pos().is_some() { items.push(MenuItem::entry(MAP, &Self::text("Upload2Map"))); }
                        self.popup = Some(key);
                        gui.open_menu(at, (self.screen.0 as i32, self.screen.1 as i32), items);
                    }
                    Hit::Click(_) => {},
                }
            }
            return Some(vec![]);
        }
        match ev {
            Event::CanvasPress { window, view, x, y, button: MouseButton::Left, .. } if *window == win => {
                if view == "expand" {
                    self.advanced = !self.advanced;
                    self.dirty = true;
                    gui.show_collapsing(win, "advanced", self.advanced);
                    gui.resize_window(win, WindowSize::Preferred);
                    self.paint_sliders(gui);
                }
                if let Some(i) = view.strip_prefix("slider").and_then(|s| s.parse::<usize>().ok()).filter(|i| *i < 7) {
                    if let Some(r) = gui.view_rect(win, view) {
                        let left = knob(self.values[i], i, r.width() + 1.0);
                        if *x >= left && *x < left + 11.0 && *y >= 0.0 && *y < 18.0 { self.grab = Some((i, x - left)); }
                    }
                }
            }
            Event::CanvasDrag { window, view, x, .. } if *window == win => {
                if let Some((i, grab)) = self.grab {
                    if let Some(r) = gui.view_rect(win, view) {
                        let min = if i == 0 { 0.0 } else { -100.0 };
                        self.values[i] = ((100.0 - min) * (x - grab) / (r.width() + 1.0 - 11.0).max(1.0) + min).clamp(min, 100.0) as i32;
                        self.dirty = true;
                        self.paint_sliders(gui);
                    }
                }
            }
            Event::CanvasRelease { window, .. } if *window == win => { self.grab = None; }
            Event::Clicked { window, view, .. } if *window == win => match view.as_str() {
                "request" => {
                    // Generate handler 100cff7e compares mission charge (stat54) with credits (61), then requires two free bag slots.
                    if zone.stat(0x3d).unwrap_or(0) < zone.stat(0x36).unwrap_or(0) {
                        let body = super::chat::log::ldb_format(&Self::text("NotEnoughCredits_NeedX"), &[super::chat::log::Arg::N(zone.stat(0x36).unwrap_or(0))]);
                        self.dialogs.go(gui, self.screen, (), &body, &[Self::text("MsgBox_OK")]);
                    }
                    else if !Self::enough_room(zone) { self.refusal(gui, "MustHave2FreeSlots"); }
                    else {
                        let info = GenerateInfo { difficulty: self.values[0] as u8, dimensions: std::array::from_fn(|i| self.values[i + 1] as i8), origin_type: self.origin_type, origin: self.origin };
                        match mission_selection::generate(own, &info) {
                            Ok(payload) => {
                                self.alternatives = None;
                                self.list.selected = None;
                                self.list.set_rows(vec![]);
                                self.list.paint(gui, win, lv::ICON_32);
                                gui.set_enabled(win, "accept", false);
                                return Some(vec![payload]);
                            }
                            Err(e) => eprintln!("mission generation: {e:#}"),
                        }
                    }
                }
                "accept" => {
                    if let Some(q) = self.list.selected.and_then(|k| self.alternatives.as_ref()?.missions.iter().find(|a| a.quest.id.instance == k)).map(|a| a.quest.id) {
                        if !Self::enough_room(zone) { self.refusal(gui, "MustHave2FreeSlots"); }
                        else { let payload = mission_selection::select(own, q); self.close_all(gui); return Some(vec![payload]); }
                    }
                }
                "cancel" => self.close_all(gui),
                _ => {},
            },
            Event::Escape { window } if *window == win && self.escape => self.close_all(gui),
            Event::CloseRequested { window } if *window == win => self.close_all(gui),
            _ => return None,
        }
        Some(vec![])
    }
    pub(super) fn close_all(&mut self, gui: &mut Gui) {
        if let Some(win) = self.win.take() { gui.close_window(win); }
        self.dialogs.close_all(gui);
        self.alternatives = None;
        self.grab = None;
        self.popup = None;
    }
    pub(super) fn take_urls(&mut self) -> Vec<String> { std::mem::take(&mut self.urls) }
    #[cfg(test)]
    pub(super) fn window(&self) -> Option<WindowId> { self.win }
    pub(super) fn take_marker(&mut self) -> Option<(u32, [f32; 2])> { self.marker.take() }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_defaults_and_slider_extents() {
        let ui = MissionUi::default();
        assert_eq!(ui.values, [50, 0, 0, 0, 0, 0, 0]);
        assert!(!ui.advanced);
        assert_eq!((knob(0, 0, 128.0), knob(50, 0, 128.0), knob(100, 0, 128.0)), (0.0, 58.0, 117.0));
        assert_eq!((knob(-100, 1, 128.0), knob(0, 1, 128.0), knob(100, 1, 128.0)), (0.0, 58.0, 117.0));
    }
    #[test]
    fn request_alternatives_accept_and_cancel() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        gui.set_screen_size(1280, 800);
        let mut ui = MissionUi::default();
        let own = Identity { kind: 50000, instance: 1 };
        let origin = Identity { kind: 51000, instance: 2 };
        let zone = Zone::new(1);
        ui.open(&mut gui, (1280, 800), 1, origin).unwrap();
        let win = ui.win.unwrap();
        let press = Event::Clicked { window: win, view: "request".into(), item: None };
        let request = ui.event(&mut gui, &press, own, &zone).unwrap();
        assert_eq!(request, vec![mission_selection::generate(own, &GenerateInfo { difficulty: 50, dimensions: [0; 6], origin_type: 1, origin }).unwrap()]);
        let quest = ao_net::n3::quest::Quest { id: Identity { kind: 56000, instance: 3 }, name: "mission".into(), ..Default::default() };
        assert!(ui.alternatives(&mut gui, Alternatives { difficulty: 6, dimensions: [0; 6], seed: 4, origin_type: 1, origin, missions: vec![mission_selection::Alternative { quest: quest.clone(), kind: 1 }] }).is_none());
        assert_eq!(ui.list.rows().len(), 1);
        ui.list.selected = Some(3);
        let accept = Event::Clicked { window: win, view: "accept".into(), item: None };
        assert_eq!(ui.event(&mut gui, &accept, own, &zone).unwrap(), vec![mission_selection::select(own, quest.id)]);
        assert!(ui.win.is_none());
        ui.open(&mut gui, (1280, 800), 1, origin).unwrap();
        let cancel = Event::Clicked { window: ui.win.unwrap(), view: "cancel".into(), item: None };
        assert!(ui.event(&mut gui, &cancel, own, &zone).unwrap().is_empty());
        assert!(ui.win.is_none());
    }
    #[test]
    fn only_own_confirmed_booth_use_opens() {
        use ao_net::n3::{misc::{GenericArgs, GenericCmd, Misc}, outgoing::n3_frame};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() || !dir.join("cd_image/rdb.db").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        gui.set_screen_size(1280, 800);
        let mut interact = super::super::interact::Interact::new(1, (1280, 800));
        let mut zone = Zone::new(1);
        let own = Identity { kind: 50000, instance: 1 };
        // Wire identity is not the runtime class. Retail 1000020:41568 is a QuestBooth.
        let booth = Identity { kind: 0xc748, instance: 2 };
        let confirmed = Misc::GenericCmd(GenericCmd { state: 1, seq: 1, cmd: 3, args: GenericArgs::Item { flag: 0, actor: own, item: booth } }).encode(own, 0);
        zone.world.test_prop(booth, vec![]);
        interact.on_frame(&mut gui, &n3_frame(0, 1, confirmed), &zone);
        assert!(interact.mission.window().is_none(), "unbuilt props cannot invoke a runtime use callback");
        let store = RecordStore::open(&dir).unwrap();
        zone.world.test_template_prop(booth, 41568, &store).unwrap();
        assert_eq!(zone.world.item_class_of(booth.kind, booth.instance), Some(0xdac1));
        assert_eq!(zone.world.item_class_of(0xdac1, booth.instance), None, "template class must not remap the wire identity");
        for (state, actor) in [(0, own), (1, Identity { instance: 3, ..own }), (1, own)] {
            let payload = Misc::GenericCmd(GenericCmd { state, seq: 1, cmd: 3, args: GenericArgs::Item { flag: 0, actor, item: booth } }).encode(own, 0);
            interact.on_frame(&mut gui, &n3_frame(0, 1, payload), &zone);
            assert_eq!(interact.mission.window().is_some(), state == 1 && actor == own);
        }
        interact.close_all(&mut gui);
        assert!(interact.mission.window().is_none());
    }
}
