//! Retail TradeSkillView_c (GUI 100de9b8), TradeskillPanel.xml and analyzer actions.
//! Wire: Gamecode 10029892/10029933/100299d4; replies 1005ec5a..1005ece3.
use super::chat::log::{ldb_format, Arg};
use super::dvalue::{element_xml, DValues};
use super::hud_stats::ItemInfo;
use super::hud_wincfg::{Store, WinCfgs};
use super::zone::Zone;
use ao_formats::screens::TextDb;
use ao_gui::{CanvasItem, CanvasTip, Event, Gui, WindowId, WindowSize};
use ao_gui::xml::{self, Element};
use ao_net::msg::Identity;
use ao_net::n3::{action, world::{AcgItem, CharacterAction}};

const SLOTS: [&str; 3] = ["SrcIconSlot", "DstIconSlot", "ResultIconSlot"];
const FEEDBACK: [&str; 3] = ["SrcFeedback", "DstFeedback", "ResultFeedback"];

#[derive(Default)]
struct Analyzer {
    inputs: [Option<(Identity, AcgItem)>; 2],
    result: Option<AcgItem>,
    counts: [Option<i32>; 2],
    min: i32,
    max: i32,
    percent: i32,
    quality: i32,
    feedback: Option<Feedback>,
}

#[derive(Clone, Copy)]
enum Feedback { SourceOnly, Quality(i32), Skill(i32, i32) }

impl Analyzer {
    fn fresh() -> Self { Self { min: 1, max: 300, percent: 100, quality: 300, ..Self::default() } }
    fn select_quality(&mut self) {
        // GUI 100de63e: inclusive QL span, integer division, clamp to max.
        self.quality = (self.min as i64 + ((self.max as i64 - self.min as i64 + 1) * self.percent as i64) / 100).min(self.max as i64) as i32;
        if let Some(r) = self.result.as_mut() { r.level = self.quality; }
    }
    fn insert(&mut self, index: usize, item: Option<(Identity, AcgItem)>) -> CharacterAction {
        self.inputs[index] = item;
        self.counts[index] = None;
        self.result = None;
        self.quality = 0;
        self.feedback = None;
        action::simple(0xdc + index as i32, Identity::default(), item.map_or(Identity::default(), |v| v.0))
    }
    fn clear(&mut self) -> Vec<CharacterAction> {
        let mut out = vec![];
        for index in 0..2 { if self.inputs[index].is_some() { out.push(self.insert(index, None)); } }
        self.counts = [None; 2];
        self.result = None;
        self.quality = 0;
        self.feedback = None;
        out
    }
    fn build(&mut self) -> CharacterAction {
        // BuildAnalyzerItem embeds QL in identity_a.instance, not param.
        let out = action::simple(0xde, Identity { kind: 0, instance: self.quality }, Identity::default());
        self.inputs = [None; 2];
        self.counts = [None; 2];
        self.result = None;
        self.quality = 0;
        self.feedback = None;
        out
    }
    fn reply(&mut self, a: &CharacterAction) -> bool {
        match a.action {
            0xdf => self.counts[0] = Some(a.identity_b.instance),
            0xe0 => self.counts[1] = Some(a.identity_b.instance),
            0xe1 => self.feedback = Some(Feedback::SourceOnly),
            0xe2 => self.feedback = Some(Feedback::Quality(a.identity_b.instance)),
            0xe3 => self.feedback = Some(Feedback::Skill(a.identity_b.kind, a.identity_b.instance)),
            0xe4 => {
                self.min = a.identity_b.kind;
                self.max = a.identity_b.instance;
                self.result = Some(AcgItem { low_id: a.identity_a.kind, high_id: a.identity_a.instance, level: 0 });
                self.select_quality();
            }
            _ => return false,
        }
        true
    }
}

pub(super) struct TradeskillUi {
    win: Option<WindowId>,
    escape: bool,
    closed: bool,
    dirty: bool,
    state: Analyzer,
    requests: Vec<CharacterAction>,
    urls: Vec<String>,
    config: WinCfgs,
}

impl Default for TradeskillUi {
    fn default() -> Self {
        Self { win: None, escape: false, closed: false, dirty: false, state: Analyzer::fresh(), requests: vec![], urls: vec![], config: WinCfgs::new(&ao_gui::client_dir()) }
    }
}

fn slot_canvases(e: &mut Element) {
    if e.name == "ItemSlotView" {
        e.name = "CanvasView".into();
        // ItemSlotView_c 100420f5 loads gfx 0x11d: a 54x54 frame around a 48x48 icon.
        // `Element::attr` returns the first match, so the template's own size attrs must go first.
        e.attrs.retain(|(k, _)| k != "min_size" && k != "max_size");
        e.attrs.extend([("min_size".into(), "Point(54,54)".into()), ("max_size".into(), "Point(54,54)".into())]);
    }
    for c in &mut e.children { slot_canvases(c); }
}

impl TradeskillUi {
    pub fn sync(&mut self, gui: &mut Gui, d: &mut DValues, screen: (u32, u32)) {
        if self.closed { d.set_i64("tradeskill_window", 0); self.closed = false; }
        if !d.flag("tradeskill_window") {
            if let Some(w) = self.win.take() {
                self.requests.extend(self.state.clear());
                self.config.detach_hud(gui, d, w);
                gui.close_window(w);
            }
            return;
        }
        if self.win.is_none() {
            let open = || -> anyhow::Result<String> {
                let s = std::fs::read_to_string(ao_gui::client_dir().join("cd_image/gui/Default/Views/TradeskillPanel.xml"))?;
                let mut root = xml::parse(&s)?;
                slot_canvases(&mut root);
                Ok(element_xml(&root))
            };
            // GUI 100de9b8 calls ResizeTo(FUN_100ddb44()) after attaching the XML:
            // native default client size is 300x400, before TradeSkillViewConfig restoration.
            match open().and_then(|s| gui.open_tabbed_window_xml("TradeskillPanel", "Tradeskill Kit", &s, (0, 0), WindowSize::Fixed(300, 400))) {
                Ok(w) => {
                    self.win = Some(w);
                    self.escape = d.flag("esc_tradeskill");
                    self.state = Analyzer::fresh();
                    self.dirty = true;
                    self.config.attach_hud(gui, d, w, Store::Value("TradeSkillViewConfig"), screen);
                    gui.set_slider_value(w, "TradeskillSlider", 100.0);
                }
                Err(e) => eprintln!("tradeskill: {e:#}"),
            }
        }
        self.config.update(gui, d);
    }
    pub fn close_all(&mut self, gui: &mut Gui) {
        if let Some(w) = self.win.take() { gui.close_window(w); }
        self.state = Analyzer::fresh();
        self.requests.clear();
        self.urls.clear();
        self.closed = false;
        self.dirty = false;
        self.config = WinCfgs::new(&ao_gui::client_dir());
    }
    pub fn take_requests(&mut self) -> Vec<CharacterAction> { std::mem::take(&mut self.requests) }
    pub fn window(&self) -> Option<WindowId> { self.win }
    pub fn take_urls(&mut self) -> Vec<String> { std::mem::take(&mut self.urls) }
    pub fn reply(&mut self, a: &CharacterAction) {
        if self.win.is_some() && self.state.reply(a) { self.dirty = true; }
    }
    pub fn drop_item(&mut self, gui: &Gui, zone: &Zone, slot: u32, x: f32, y: f32) {
        self.drop_identity(gui, zone, ao_net::n3::inventory::item_identity(slot), x, y);
    }
    pub fn drop_identity(&mut self, gui: &Gui, zone: &Zone, item: Identity, x: f32, y: f32) -> bool {
        let Some(w) = self.win else { return false };
        if gui.pointer_owner(x, y) != Some(w) { return false; }
        let entry = if item.kind == ao_net::n3::inventory::KIND_IN_CONTAINER {
            zone.containers.values().find(|(word, _)| i32::from(*word as i16) == item.instance >> 16)
                .and_then(|(_, entries)| entries.iter().find(|e| e.slot == (item.instance & 0xffff) as u32))
        } else {
            zone.inventory.get(&(item.instance as u32)).filter(|e| ao_net::n3::inventory::item_identity(e.slot) == item)
        };
        let Some(entry) = entry else { return false };
        for (index, name) in SLOTS[..2].iter().enumerate() {
            if gui.view_rect(w, name).is_some_and(|r| x >= r.l && x <= r.r && y >= r.t && y <= r.b) {
                self.requests.push(self.state.insert(index, Some((item, entry.item))));
                self.dirty = true;
                return true;
            }
        }
        false
    }
    pub fn event(&mut self, ev: &Event) -> bool {
        let Some(w) = self.win else { return false };
        match ev {
            Event::CloseRequested { window } if *window == w => self.closed = true,
            Event::Escape { window } if *window == w => { if self.escape { self.closed = true; } }
            Event::Clicked { window, view, .. } if *window == w => match view.as_str() {
                "ClearButton" => { self.requests.extend(self.state.clear()); self.dirty = true; }
                "BuildButton" if self.state.inputs.iter().all(Option::is_some) => { self.requests.push(self.state.build()); self.dirty = true; }
                _ => {}
            },
            Event::SliderChanged { window, view, value, .. } if *window == w && view == "TradeskillSlider" => {
                self.state.percent = (*value as i32).clamp(0, 100);
                self.state.select_quality();
                self.dirty = true;
            }
            // TradeSkillView_c::MouseDown, vtable +0x3c (100de34a): click an input to clear, result to inspect.
            Event::CanvasPress { window, view, .. } if *window == w => {
                if let Some(index) = SLOTS[..2].iter().position(|n| *n == view) {
                    self.requests.push(self.state.insert(index, None));
                    self.dirty = true;
                } else if view == SLOTS[2] {
                    if let Some(r) = self.state.result {
                        self.urls.push(format!("itemref://{}/{}/{}", r.low_id, r.high_id, r.level));
                    }
                }
            }
            Event::WindowFrame { window } if *window == w => {}
            _ => return false,
        }
        true
    }
    pub fn render(&mut self, gui: &mut Gui, zone: &Zone, texts: &TextDb, mut info: impl FnMut(&mut Gui, i32) -> Option<ItemInfo>) {
        if !self.dirty { return; }
        let Some(w) = self.win else { return };
        self.dirty = false;
        let mut names = [String::new(), String::new(), String::new()];
        let items = [self.state.inputs[0].map(|v| v.1), self.state.inputs[1].map(|v| v.1), self.state.result];
        for (index, item) in items.iter().enumerate() {
            let mut pictures = vec![];
            let mut tips = vec![];
            if let Some(g) = gui.gfx().id("GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED") {
                let (width, height) = gui.gfx().size(g);
                pictures.push(CanvasItem::Image { id: g, src: [0.0, 0.0, width as f32, height as f32], dst: [0.0, 0.0, 54.0, 54.0], alpha: 1.0 });
            }
            if let Some((name, icon)) = item.and_then(|a| info(gui, a.low_id)) {
                names[index] = name.clone();
                tips.push(CanvasTip { rect: [0.0, 0.0, 54.0, 54.0], title: name, body: String::new() });
                if let Some((g, width, height)) = icon {
                    pictures.push(CanvasItem::Image { id: g, src: [0.0, 0.0, width as f32, height as f32], dst: [3.0, 3.0, 51.0, 51.0], alpha: 1.0 });
                }
            }
            gui.set_canvas(w, SLOTS[index], pictures);
            gui.set_canvas_tips(w, SLOTS[index], tips);
        }
        let fmt = |key: &str, args: &[Arg]| ldb_format(&texts.by_key(10000, key).unwrap_or_default(), args);
        for (index, view) in FEEDBACK[..2].iter().enumerate() {
            let text = self.state.counts[index].map(|count| fmt("X_Lvl_Y_used_in_Z", &[Arg::S(names[index].clone()), Arg::N(items[index].map_or(0, |a| a.level)), Arg::N(count)])).unwrap_or_default();
            gui.set_text(w, view, &text);
        }
        let result_text = self.state.result.map(|r| fmt("ResultWillBeXY", &[Arg::N(r.level), Arg::S(names[2].clone())])).unwrap_or_default();
        gui.set_text(w, FEEDBACK[2], &result_text);
        let feedback = match self.state.feedback {
            Some(Feedback::SourceOnly) => fmt("MaybyUsedAsSource", &[]),
            Some(Feedback::Quality(q)) => fmt("X_MustBeLvl_Y_ToCombineWithZZ", &[Arg::S(names[0].clone()), Arg::N(q), Arg::S(names[1].clone()), Arg::N(items[1].map_or(0, |a| a.level))]),
            Some(Feedback::Skill(stat, required)) => {
                let skill = ao_formats::stats::name(stat as u32).unwrap_or_default().to_string();
                fmt("YouNeedAtLeastSkill", &[Arg::N(required), Arg::S(skill.clone()), Arg::S(skill), Arg::N(zone.skill_value(stat as u32).unwrap_or(0))])
            }
            None => String::new(),
        };
        gui.set_text(w, "BuildFeedback", &feedback);
        for (view, quality) in [("TradeskillLblMin", self.state.min), ("TradeskillLbl", self.state.quality), ("TradeskillLblMax", self.state.max)] { gui.set_text(w, view, &format!("QL {quality}")); }
        gui.set_enabled(w, "BuildButton", self.state.inputs.iter().all(Option::is_some));
        gui.set_enabled(w, "ClearButton", self.state.inputs.iter().any(Option::is_some));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn analyzer_requests_replies_and_inclusive_quality() {
        let mut a = Analyzer::fresh();
        let item = AcgItem { low_id: 1, high_id: 2, level: 50 };
        let id = Identity { kind: 0x68, instance: 0x40 };
        let source = a.insert(0, Some((id, item)));
        assert_eq!(source, action::simple(0xdc, Identity::default(), id));
        let wire = action::character_action(123, &source);
        assert_eq!(action::parse_character_action(&wire).unwrap().1, source);
        assert_eq!(a.insert(1, Some((id, item))).action, 0xdd);
        let reply = action::simple(0xe4, Identity { kind: 10, instance: 11 }, Identity { kind: 100, instance: 200 });
        assert!(a.reply(&reply));
        assert_eq!(a.quality, 200);
        a.percent = 0; a.select_quality(); assert_eq!(a.quality, 100);
        a.percent = 50; a.select_quality(); assert_eq!(a.quality, 150);
        let build = a.build();
        assert_eq!(build, action::simple(0xde, Identity { kind: 0, instance: 150 }, Identity::default()));
        assert!(a.inputs.iter().all(Option::is_none));
        assert!(a.result.is_none());
        a.insert(0, Some((id, item)));
        assert_eq!(a.clear(), [action::simple(0xdc, Identity::default(), Identity::default())]);
    }
    #[test]
    fn native_panel_escape_capture_and_inventory_drop() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&client, None).unwrap();
        let mut d = DValues::new(&client);
        let mut ui = TradeskillUi::default();
        d.set_i64("tradeskill_window", 1);
        d.set_i64("esc_tradeskill", 0);
        ui.sync(&mut gui, &mut d, (1280, 800));
        let w = ui.win.unwrap();
        assert_eq!(gui.window_size(w), (300, 400), "native constructor explicitly sizes the XML panel");
        for view in SLOTS.into_iter().chain(["ClearButton", "BuildButton", "TradeskillSlider", "BuildFeedback"]) {
            assert!(gui.has_view(w, view), "{view}");
        }
        let mut zone = Zone::new(123);
        let item = AcgItem { low_id: 1, high_id: 2, level: 50 };
        zone.inventory.insert(0x40, ao_net::n3::world::InventoryEntry { slot: 0x40, a: 0, b: 0, id: Identity::default(), item });
        let rect = gui.view_rect(w, SLOTS[0]).unwrap();
        // The XML's bordered columns, operators and buttons share a 300-wide row.
        // 54x54 is the slot art/preferred size, not a fixed layout allocation:
        // native View::SpaceOut (1014a238) also distributes a space deficit.
        // view_rect reports the allocated inclusive extents, not the icon size.
        assert_eq!((rect.r - rect.l, rect.b - rect.t), (51.0, 54.0));
        ui.drop_item(&gui, &zone, 0x40, rect.l + 4.0, rect.t + 4.0);
        assert_eq!(ui.take_requests(), [action::simple(0xdc, Identity::default(), ao_net::n3::inventory::item_identity(0x40))]);
        let id = ao_net::n3::inventory::container_item_identity(0x70, 3);
        zone.containers.insert((0xc749, 7), (0x70, vec![ao_net::n3::world::InventoryEntry { slot: 3, a: 0, b: 0, id, item }]));
        assert!(ui.drop_identity(&gui, &zone, id, rect.l + 4.0, rect.t + 4.0));
        assert_eq!(ui.take_requests(), [action::simple(0xdc, Identity::default(), id)]);
        ui.reply(&action::simple(0xe4, Identity { kind: 10, instance: 11 }, Identity { kind: 100, instance: 200 }));
        ui.event(&Event::CanvasPress { window: w, view: SLOTS[2].into(), x: 4.0, y: 4.0, button: ao_gui::MouseButton::Left, clicks: 1 });
        assert_eq!(ui.take_urls(), ["itemref://10/11/200"]);
        d.set_i64("esc_tradeskill", 1);
        ui.event(&Event::Escape { window: w });
        ui.sync(&mut gui, &mut d, (1280, 800));
        assert_eq!(ui.win, Some(w));
        ui.event(&Event::CloseRequested { window: w });
        ui.sync(&mut gui, &mut d, (1280, 800));
        assert!(ui.win.is_none() && !d.flag("tradeskill_window"));
        assert_eq!(ui.take_requests(), [action::simple(0xdc, Identity::default(), Identity::default())]);
        d.set_i64("tradeskill_window", 1);
        ui.sync(&mut gui, &mut d, (1280, 800));
        let w = ui.win.unwrap();
        d.set_i64("esc_tradeskill", 0);
        ui.event(&Event::Escape { window: w });
        ui.sync(&mut gui, &mut d, (1280, 800));
        assert!(ui.win.is_none() && !d.flag("tradeskill_window"));
    }
}
