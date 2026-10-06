//! The options window (`OptionPanelModule_c`, GUI.dll ctor 0xc2c03, module name `optionpanel_window`; docs/gui.md "Options window").
//!
//! Opened by the Windows menu entry `optionpanel_window` (`ActionMenu/Windows.xml`, `MenuEntry label="#Setting"`) / `/open Settings` (the dvalue toggles it like
//! every other window, [`super::hud::WindowKind::Options`]). `ModuleActivated` 0xc2b7a builds `OptionWindow_c` (`FUN_100c3c44`): a style-0 `Window(Rect(200,200,600,400), "", "", 0,
//! 0x1000)` with the tabs "Preferences" (this file), "Fixed keys" and "Key bindings" (`HotKeys.xml`, not ported: hot key rebinding), configured by the `OptionWindowConfig`
//! archive (`LoadWndConfig`). The Preferences tab is an `OptionCategoryPanel_c` (`FUN_100c1053`: the category tree and the two buttons `Quit2Windows` / `Quit2Login`) next to a
//! `ViewSelector_c` holding one page per `ScrollView` of `OptionPanel/Root.xml` (`FUN_100c3691`); every control is bound to its variable ([`model`]).

mod keypage;
pub(super) mod keys;
mod live;
mod model;

pub(super) use live::{audio_prefs, voice_gain};

use super::dvalue::{DValues, Variant};
use ao_formats::screens::{TextDb, CAT_GUI};
use ao_gui::view::{CanvasItem, ListItem};
use ao_gui::{xml, Event, GfxId, Gui, InputEvent, MouseButton, WindowId, WindowSize};
use model::{Ctl, Item, Opt, Page};
use std::path::Path;

/// `Window(Rect(200,200,600,400), ..)` of `FUN_100c3c44` (floats `_DAT_101a959c` / `_DAT_101b3db8` / `_DAT_101b1840`), used when the config has no `WindowFrame`.
const DEFAULT_FRAME: [f32; 4] = [200.0, 200.0, 600.0, 400.0];
const CONFIG: &str = "OptionWindowConfig";
const TREE: &str = "tree";
const PAGES: &str = "pages";
/// The `ViewSelector` holding the three tab views (`Window::AppendTab`).
const TABS: &str = "tabs";
/// Style-0 frame insets (docs/gui.md §6.1): client = outer - (10, 31).
const CHROME: (u32, u32) = (10, 31);
/// `Slider_c` art: `GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER_BACKGROUND` (id 0x92) and the knob `..._SLIDER` (0x91, 11 x 18, tinted DEFAULT) -- docs/gui.md §10.
const SLIDER_BG: &str = "GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER_BACKGROUND";
const SLIDER_KNOB: &str = "GFX_GUI_CONTROLCENTER_AGGDEF_SLIDER";
const KNOB_W: f32 = 11.0;
const KNOB_H: f32 = 18.0;
const DEFAULT_COLOR: u32 = 0x1000000;

/// What a click on `Quit2Windows` / `Quit2Login` asks of the game: the slots `LAB_100c1027` / `LAB_100c103d` send `AFCM::Send(10, 0x133)` / `Send(10, 0x134)`,
/// the same messages as the chat commands `/quit` (`StartQuitToSystemMessage` 0x10029a0d) and `/camp` (`StartQuitToLoginMessage` 0x10027c74).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Quit,
    Camp,
}

/// Saved state of the window (`OptionWindowConfig`, `Window::SaveWndConfig` + `OptionWindow_c` dtor): frame, open category folders, selected leaf.
#[derive(Clone, Debug, PartialEq)]
struct Config {
    frame: [f32; 4],
    folders: Vec<String>,
    selected: String,
    tab: i32,
    scroll: f32,
    /// The `hotkey_config` sub-archive (`FUN_100bd921`): the Fixed keys list.
    hotkeys: keypage::HotkeyConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config { frame: DEFAULT_FRAME, folders: vec![], selected: String::new(), tab: 0, scroll: 0.0, hotkeys: Default::default() }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// TinyXML attribute quoting of a string value `"x"`: single quotes with `&quot;` inside (as the shipped templates write it).
fn string_attr(s: &str) -> String {
    format!("'&quot;{}&quot;'", s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"))
}

impl Config {
    /// Reads an `OptionWindowConfig` archive (`Window::LoadWndConfig` + `FUN_100c3691` / `FUN_100c3c44` `Message::Find*`).
    fn parse(archive: &str) -> Option<Config> {
        let root = xml::parse(archive).ok()?;
        let a = if root.name == "Archive" { &root } else { root.children.iter().find(|c| c.name == "Archive")? };
        let mut c = Config::default();
        let unq = |v: &str| v.trim_matches('"').to_string();
        for e in &a.children {
            let (Some(name), Some(v)) = (e.attr("name"), e.attr("value")) else { continue };
            match (e.name.as_str(), name) {
                ("Rect", "WindowFrame") => {
                    let n: Vec<f32> = v.trim_start_matches("Rect(").trim_end_matches(')').split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    if let [l, t, r, b] = n[..] {
                        c.frame = [l, t, r, b];
                    }
                }
                ("String", "open_panel_folders") => c.folders.push(unq(v)),
                ("String", "selected_panel") => c.selected = unq(v),
                ("Int32", "selected_tab") => c.tab = v.trim().parse().unwrap_or(0),
                ("Float", "panel_list_scroll_offset") => c.scroll = v.trim().parse().unwrap_or(0.0),
                _ => {}
            }
        }
        if let Some(h) = a.children.iter().find(|e| e.name == "Archive" && e.attr("name") == Some("hotkey_config")) {
            for e in &h.children {
                let (Some(name), Some(v)) = (e.attr("name"), e.attr("value")) else { continue };
                match (e.name.as_str(), name) {
                    ("String", "open_hotkey_folders") => c.hotkeys.folders.push(unq(v)),
                    ("String", "selected_hotkey") => c.hotkeys.selected = unq(v),
                    ("Float", "list_scroll_offset") => c.hotkeys.scroll = v.trim().parse().unwrap_or(0.0),
                    _ => {}
                }
            }
        }
        Some(c)
    }

    fn archive(&self) -> String {
        let mut s = String::from("<Archive name=\"OptionWindowConfig\" code=\"0\">");
        let [l, t, r, b] = self.frame;
        s += &format!("<Rect name=\"WindowFrame\" value=\"Rect({l:.6},{t:.6},{r:.6},{b:.6})\" />");
        s += "<Bool name=\"WindowPinButtonState\" value=\"false\" />";
        for f in &self.folders {
            s += &format!("<String name=\"open_panel_folders\" value={} />", string_attr(f));
        }
        s += &format!("<String name=\"selected_panel\" value={} />", string_attr(&self.selected));
        s += &format!("<Int32 name=\"selected_tab\" value=\"{}\" />", self.tab);
        s += &format!("<Float name=\"panel_list_scroll_offset\" value=\"{:.6}\" />", self.scroll);
        s += "<Archive code=\"0\" name=\"hotkey_config\">";
        for f in &self.hotkeys.folders {
            s += &format!("<String name=\"open_hotkey_folders\" value={} />", string_attr(f));
        }
        if !self.hotkeys.selected.is_empty() {
            s += &format!("<String name=\"selected_hotkey\" value={} />", string_attr(&self.hotkeys.selected));
        }
        s += &format!("<Float name=\"list_scroll_offset\" value=\"{:.6}\" /></Archive></Archive>", self.hotkeys.scroll);
        s
    }
}

/// Tree entries of a page label (`FUN_100c3691`): the text split at `/`; the id of a level is the concatenation of the segments up to it (no separator:
/// the shipped template's `selected_panel` is `"GUIControl Center"`, `open_panel_folders` `"GUI"`). Returns `(id, text, is_leaf)` per level.
fn levels(label: &str) -> Vec<(String, String, bool)> {
    let segs: Vec<&str> = label.split('/').collect();
    let mut id = String::new();
    segs.iter()
        .enumerate()
        .map(|(i, s)| {
            id += s;
            (id.clone(), s.to_string(), i + 1 == segs.len())
        })
        .collect()
}

/// One bound control: the option and the names of its views in the window.
struct Bound {
    opt: Opt,
    page: usize,
    /// `c<i>` check box, `r<i>` radio group, `s<i>` slider canvas (value text `v<i>`).
    name: String,
    shown: Option<f64>,
    shown_w: u32,
    enabled: Option<bool>,
}

struct Win {
    id: WindowId,
    controls: Vec<Bound>,
    /// Leaf id of the page at each selector index.
    leaves: Vec<String>,
    folders: Vec<String>,
    selected: usize,
    /// The slider being dragged: control index and the grab offset inside the knob (`Slider_c` +0x170 / +0x174).
    grab: Option<(usize, f32)>,
    tab: i32,
}

pub(super) struct HudOptions {
    pages: Vec<Page>,
    texts: TextDb,
    template: Option<Config>,
    screen: (u32, u32),
    win: Option<Win>,
    closed: bool,
    actions: Vec<Action>,
    live: live::Live,
    dir: std::path::PathBuf,
    esc: Vec<super::hud::WindowKind>,
    /// The "Fixed keys" and "Key bindings" tabs ([`keypage`]).
    keypages: keypage::KeyPages,
}

fn borders(b: &str) -> String {
    if b.is_empty() { String::new() } else { format!(" layout_borders=\"{}\"", esc(b)) }
}

/// The view XML of a page (`ScrollView` of `Root.xml`); `n` numbers the controls over all pages. Controls follow the structure of `OptionCheckBox_c`
/// (`FUN_100c1be9`), `OptionSlider_c` (`FUN_100c0aa9`: `[label, spacer, value text]` above the slider) and `OptionRadioButtonGroup_c` (`FUN_100c2fd5`: label above the
/// group, group borders (10, 3, 0, 0)).
fn page_xml(page: &Page, idx: usize, n: &mut usize) -> String {
    fn item(it: &Item, n: &mut usize, out: &mut String) {
        match it {
            Item::Text { value, borders: b } => *out += &format!("<TextView value=\"{}\"{}/>", esc(value), borders(b)),
            Item::Spacer { min, max } => *out += &format!("<VLayoutSpacer min_size=\"{min}\" max_size=\"{max}\"/>"),
            Item::Group { borders: b, items } => {
                *out += &format!("<View{}>", borders(b));
                for i in items {
                    item(i, n, out);
                }
                *out += "</View>";
            }
            Item::Opt(o) => {
                let i = *n;
                *n += 1;
                let tip = o.tooltip.as_ref().map_or(String::new(), |(t, b)| format!(" tooltip=\"{}\" tooltip_body=\"{}\"", esc(t), esc(b)));
                match &o.ctl {
                    // `OptionCheckBox_c` is an HLayout `[toggle button, label, spacer]`: the row spans the page, the box sits at its left
                    Ctl::Check => *out += &format!("<View view_layout=\"horizontal\"{}{tip}><CheckBox name=\"c{i}\" label=\"{}\"/><HLayoutSpacer/></View>", borders(&o.borders), esc(&o.label)),
                    Ctl::Slider => {
                        *out += &format!(
                            "<View view_layout=\"vertical\"{}{tip}><View view_layout=\"horizontal\"><TextView value=\"{}\"/><HLayoutSpacer/><TextView name=\"v{i}\" value=\"\"/></View>\
                             <CanvasView name=\"s{i}\" min_size=\"Point({},{})\" max_size=\"Point(16000,{})\"/></View>",
                            borders(&o.borders),
                            esc(&o.label),
                            KNOB_W as i32,
                            KNOB_H as i32 - 1,
                            KNOB_H as i32 - 1
                        )
                    }
                    Ctl::Radio(rs) => {
                        *out += &format!("<View view_layout=\"vertical\" h_alignment=\"left\"{}{tip}><TextView value=\"{}\"/><RadioButtonGroup name=\"r{i}\" layout_borders=\"Rect(10,3,0,0)\" h_alignment=\"left\">", borders(&o.borders), esc(&o.label));
                        for (l, v) in rs {
                            *out += &format!("<RadioButton name=\"r{i}_{v}\" label=\"{}\" value=\"{v}\"/>", esc(l));
                        }
                        *out += "</RadioButtonGroup></View>";
                    }
                }
            }
        }
    }
    let mut s = format!("<ScrollView name=\"pg{idx}\" label=\"{}\" v_scrollbar_mode=\"auto\" max_size=\"Point(16000,-1)\"><ScrollViewChild><View h_alignment=\"left\" view_layout=\"vertical\" max_size=\"Point(16000,-1)\" layout_borders=\"Rect(0,0,10,0)\">", esc(&page.label));
    for it in &page.items {
        item(it, n, &mut s);
    }
    s + "</View></ScrollViewChild></ScrollView>"
}

impl HudOptions {
    pub(super) fn handles(kind: super::hud::WindowKind) -> bool {
        kind == super::hud::WindowKind::Options
    }

    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> anyhow::Result<Self> {
        let root = std::fs::read_to_string(dir.join("cd_image/gui/Default/OptionPanel/Root.xml")).unwrap_or_default();
        // the shipped `prefs/NewChar/Prefs.xml` carries the window the installer's first login shows (frame, `GUIControl Center` selected)
        let template = std::fs::read_to_string(dir.join("prefs/NewChar/Prefs.xml")).ok().and_then(|t| xml::parse(&t).ok()).and_then(|r| {
            let a = r.children.iter().find(|c| c.name == "Archive" && c.attr("name") == Some(CONFIG))?;
            Config::parse(&xml_text(a))
        });
        let texts = TextDb::load(dir)?;
        let keypages = keypage::KeyPages::new(dir, &texts, screen);
        Ok(Self { pages: model::parse(&root), texts, template, screen, win: None, closed: false, actions: vec![], live: live::Live::default(), dir: dir.to_path_buf(), esc: vec![], keypages })
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
        self.keypages.set_screen(screen);
    }

    /// The "Bind Key" dialog is open: key presses are captured for the binding ([`Self::capture`]).
    pub(super) fn capturing(&self) -> bool {
        self.keypages.capturing()
    }

    /// A key (`key id | modifiers`) pressed while the "Bind Key" dialog is open.
    pub(super) fn capture(&mut self, gui: &mut Gui, _d: &mut DValues, input: u32) {
        self.keypages.capture(gui, input);
    }

    pub(super) fn take_closed(&mut self) -> bool {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    /// A window was opened: when its `esc_*` option is set *now* ("changes to already open windows will not take effect until the next time the window is
    /// opened", Root.xml note; the window constructors read the DValue, e.g. `FUN_100cc2ca` `esc_inventory`), Esc will close it.
    pub(super) fn arm_esc(&mut self, kind: super::hud::WindowKind, d: &DValues) {
        use super::hud::WindowKind as K;
        let var = match kind {
            K::Inventory => "esc_inventory",
            K::Character => "esc_wear",
            K::Nano => "esc_nano",
            K::Perks => "esc_perkwindow",
            K::PlanetMap => "esc_planetmap",
            K::Options => "esc_optionpanel",
            _ => return,
        };
        if d.flag(var) && !self.esc.contains(&kind) {
            self.esc.push(kind);
        }
    }

    pub(super) fn disarm_esc(&mut self, kind: super::hud::WindowKind) {
        self.esc.retain(|k| *k != kind);
    }

    /// Esc was pressed: the armed windows to close.
    pub(super) fn take_esc(&mut self) -> Vec<super::hud::WindowKind> {
        std::mem::take(&mut self.esc)
    }

    pub(super) fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    #[cfg(test)]
    fn window(&self) -> Option<WindowId> {
        self.win.as_ref().map(|w| w.id)
    }

    /// The option observers that run for the whole session ([`live`]): `ToggleAllEffects`, the server-side flags, the visual flags.
    pub(super) fn apply(&mut self, d: &mut DValues, zone: &mut super::zone::Zone, out: &mut Vec<ao_net::frame::Frame>) {
        self.live.apply(d, zone, out);
    }

    /// `ModuleActivated(true)`: nothing happens while the window exists.
    pub(super) fn open(&mut self, gui: &mut Gui, d: &DValues) {
        if self.win.is_some() {
            return;
        }
        let cfg = match d.get(CONFIG) {
            Some(Variant::Archive(a)) => Config::parse(a).filter(|_| a.contains("WindowFrame")),
            _ => None,
        }
        .or_else(|| self.template.clone())
        .unwrap_or_default();
        let label = |p: &Page| self.texts.label(&p.label);
        // the category tree: folders for the leading levels of the page labels, one leaf per page
        let mut n = 0;
        let mut pages_xml = String::new();
        for (i, p) in self.pages.iter().enumerate() {
            pages_xml += &page_xml(p, i, &mut n);
        }
        let (quit_win, quit_login) = ["Quit2Windows", "Quit2Login"].map(|k| self.texts.by_key(CAT_GUI, k).unwrap_or_else(|| k.to_string())).into();
        let has_fixed = self.keypages.has_fixed();
        let (fixed_tab, bind_tab) = self.keypages.xml();
        let fixed_tab = if has_fixed { fixed_tab } else { String::new() };
        let src = format!(
            "<root><ViewSelector name=\"{TABS}\"><View view_layout=\"horizontal\" h_alignment=\"left\">\
             <View view_layout=\"vertical\" layout_borders=\"Rect(10,10,0,10)\">\
             <BorderView layout_borders=\"Rect(0,0,5,10)\"><View layout_borders=\"Rect(5,5,5,5)\"><StringListView name=\"{TREE}\" v_scrollbar_mode=\"auto\" max_size=\"Point(16000,16000)\"/></View></BorderView>\
             <View view_layout=\"horizontal\"><HLayoutSpacer/><View view_layout=\"vertical\">\
             <Button name=\"b1\" label=\"{}\" layout_borders=\"Rect(5,10,5,0)\" width_group=\"QuitButtons\"/>\
             <Button name=\"b2\" label=\"{}\" layout_borders=\"Rect(5,10,5,0)\" width_group=\"QuitButtons\"/>\
             </View><HLayoutSpacer/></View></View>\
             <BorderView layout_borders=\"Rect(5,10,10,10)\"><View layout_borders=\"Rect(5,5,5,5)\"><ViewSelector name=\"{PAGES}\">{pages_xml}</ViewSelector></View></BorderView>\
             </View>{fixed_tab}{bind_tab}</ViewSelector></root>",
            esc(&quit_win),
            esc(&quit_login)
        );
        let [l, t, r, b] = cfg.frame;
        let outer = ((r - l + 1.0).max(120.0) as u32, (b - t + 1.0).max(120.0) as u32);
        let client = (outer.0.saturating_sub(CHROME.0).max(60), outer.1.saturating_sub(CHROME.1).max(40));
        let (x, y) = (l as i32, t as i32);
        let id = match gui.open_tabbed_window_xml("OptionWindow", "Preferences", &src, (x, y), WindowSize::Fixed(client.0, client.1)) {
            Ok(w) => w,
            Err(e) => return eprintln!("hud: options window: {e:#}"),
        };
        gui.set_window_frame(id, true, true);
        gui.set_window_size_limits(id, (200, 120), (0, 0));
        // keep the window on the screen (`Window::MoveInsideScreen`)
        let (ow, oh) = gui.outer_size(id);
        let (px, py) = ((x.min(self.screen.0 as i32 - ow as i32)).max(0), (y.min(self.screen.1 as i32 - oh as i32)).max(0));
        gui.set_window_outer_frame(id, (px, py, ow, oh));

        let mut controls = vec![];
        let mut leaves = vec![];
        let mut folders = vec![];
        let mut selected = 0;
        let mut seen: Vec<String> = vec![];
        for (pi, p) in self.pages.iter().enumerate() {
            for o in p.opts() {
                let name = match &o.ctl {
                    Ctl::Check => format!("c{}", controls.len()),
                    Ctl::Slider => format!("s{}", controls.len()),
                    Ctl::Radio(_) => format!("r{}", controls.len()),
                };
                controls.push(Bound { opt: o.clone(), page: pi, name, shown: None, shown_w: 0, enabled: None });
            }
            let lv = levels(&label(p));
            for (lid, text, leaf) in &lv {
                if *leaf {
                    let parent = lv.iter().rev().nth(1).map(|(pid, ..)| pid.as_str());
                    gui.list_add(id, TREE, parent, ListItem::new(lid, text, 0xd8, 0));
                    if *lid == cfg.selected {
                        selected = pi;
                    }
                    leaves.push(lid.clone());
                } else if !seen.contains(lid) {
                    seen.push(lid.clone());
                    let parent = lv.iter().take_while(|(i, ..)| i != lid).last().map(|(pid, ..)| pid.as_str());
                    // `StringListViewItem_c(variant, text, 0xd5, 0xd4)`, `SetIsFolder(true)`, `MakeSelectable(false)`, `OpenFolder(saved)`
                    let mut f = ListItem::new(lid, text, 0xd5, 0xd4);
                    f.folder = true;
                    f.selectable = false;
                    f.open = cfg.folders.contains(lid);
                    gui.list_add(id, TREE, parent, f);
                    folders.push(lid.clone());
                }
            }
        }
        self.win = Some(Win { id, controls, leaves, folders, selected, grab: None, tab: cfg.tab });
        if let Some(leaf) = self.win.as_ref().and_then(|w| w.leaves.get(w.selected).cloned()) {
            gui.list_select(id, TREE, &leaf, true, false);
        }
        gui.select_child(id, PAGES, Some(selected));
        // `AppendTab("Preferences")`, `AppendTab("Fixed keys")` (only when HotKeys.xml loaded), `AppendTab("Key bindings")`
        let mut tabs = vec!["Preferences".to_string()];
        if has_fixed {
            tabs.push("Fixed keys".into());
            self.keypages.populate_fixed(gui, id, &keys::FixedKeys::from_prefs(&d.prefs), &cfg.hotkeys, &self.texts);
        }
        tabs.push("Key bindings".into());
        let table = match d.get("KeyBindings") {
            Some(Variant::Archive(t)) => keys::Bindings::from_archive(t),
            _ => keys::Bindings::default(),
        };
        self.keypages.populate_binds(gui, id, &table, &self.texts);
        let sel = (cfg.tab.max(0) as usize).min(tabs.len() - 1);
        gui.set_window_tabs(id, &tabs, sel);
        gui.select_child(id, TABS, Some(sel));
        if let Some(w) = self.win.as_mut() {
            w.tab = sel as i32;
        }
    }

    /// `OptionWindow_c` dtor: the frame, open folders and selection go into `OptionWindowConfig` (the flow saves the DValue files).
    pub(super) fn close(&mut self, gui: &mut Gui, d: &mut DValues) {
        let Some(w) = self.win.take() else { return };
        let mut cfg = Config { tab: w.tab, ..Config::default() };
        if let Some((x, y, ow, oh)) = gui.window_outer_frame(w.id) {
            cfg.frame = [x as f32, y as f32, (x + ow as i32 - 1) as f32, (y + oh as i32 - 1) as f32];
        }
        cfg.folders = w.folders.iter().filter(|f| gui.list_item(w.id, TREE, f).is_some_and(|i| i.open)).cloned().collect();
        cfg.selected = w.leaves.get(w.selected).cloned().unwrap_or_default();
        cfg.hotkeys = self.keypages.save_fixed(gui, &self.texts);
        self.keypages.close(gui);
        gui.close_window(w.id);
        d.set(CONFIG, Variant::Archive(cfg.archive()));
    }

    /// Mirrors the stored values into the controls every frame (the original's `DistributedValue_c::Observe` / pref changed callbacks, `FUN_100c0934` ...) and
    /// evaluates `view_enable_expression` (`View::SetEnableExpression`, a frame timer).
    pub(super) fn update(&mut self, gui: &mut Gui, d: &DValues, res: &dyn Fn(&str, &str) -> Option<i64>) {
        let Some(w) = self.win.as_mut() else { return };
        let id = w.id;
        self.keypages.sync(gui, d, &self.texts);
        self.keypages.refresh_fixed(gui, &keys::FixedKeys::from_prefs(&d.prefs));
        let dragging = w.grab.map(|g| g.0);
        for (i, c) in w.controls.iter_mut().enumerate() {
            let on = c.opt.enable.is_empty() || ao_gui::expr::truthy(&c.opt.enable, res);
            if c.enabled != Some(on) {
                c.enabled = Some(on);
                gui.set_enabled(id, &c.name, on);
                if let Ctl::Radio(rs) = &c.opt.ctl {
                    for (_, v) in rs {
                        gui.set_enabled(id, &format!("{}_{v}", c.name), on);
                    }
                }
            }
            let v = c.opt.value(d);
            match &c.opt.ctl {
                Ctl::Check => {
                    let on = c.opt.checked(d);
                    if gui.checked(id, &c.name) != on {
                        gui.set_checked(id, &c.name, on);
                    }
                }
                Ctl::Radio(_) => {
                    if gui.radio_value(id, &c.name) != Some(v as i32) {
                        gui.set_radio_value(id, &c.name, v as i32);
                    }
                }
                Ctl::Slider => {
                    let width = gui.canvas_size(id, &c.name).0;
                    if c.shown != Some(v) || c.shown_w != width || dragging == Some(i) {
                        c.shown = Some(v);
                        c.shown_w = width;
                        let (lo, hi) = c.opt.range(d);
                        gui.set_text(id, &format!("v{}", &c.name[1..]), &c.opt.value_text(v));
                        gui.set_canvas(id, &c.name, slider_items(gui, width as f32, v, lo, hi));
                    }
                }
            }
        }
    }

    /// Handles the window's events; true when consumed.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, d: &mut DValues) -> bool {
        let Some(w) = self.win.as_mut() else { return false };
        if self.keypages.event(gui, ev, d, &self.texts) {
            return true;
        }
        match ev {
            Event::TabSelected { window, index } if *window == w.id => {
                w.tab = *index as i32;
                gui.select_child(w.id, TABS, Some(*index));
                true
            }
            Event::CloseRequested { window } if *window == w.id => {
                self.closed = true;
                true
            }
            Event::Clicked { window, view, .. } if *window == w.id => {
                match view.as_str() {
                    "b1" => self.actions.push(Action::Quit),
                    "b2" => self.actions.push(Action::Camp),
                    v => {
                        // `c<i>` check box, `r<i>_<value>` radio button
                        if let Some(i) = v.strip_prefix('c').and_then(|n| n.parse::<usize>().ok()) {
                            if let Some(c) = w.controls.get(i) {
                                c.opt.set_checked(d, gui.checked(w.id, v));
                            }
                        } else if let Some((i, _)) = v.strip_prefix('r').and_then(|r| r.split_once('_')).and_then(|(i, val)| Some((i.parse::<usize>().ok()?, val))) {
                            if let (Some(c), Some(val)) = (w.controls.get(i), gui.radio_value(w.id, &format!("r{i}"))) {
                                c.opt.set_value(d, f64::from(val));
                            }
                        }
                    }
                }
                true
            }
            Event::ListSelected { window, view, id, selected: true } if *window == w.id && view == TREE => {
                if let Some(i) = w.leaves.iter().position(|l| l == id) {
                    w.selected = i;
                    gui.select_child(w.id, PAGES, Some(i));
                }
                true
            }
            _ => false,
        }
    }

    /// `Slider_c::MouseDown` / `MouseMove` / `MouseUp` (docs/gui.md §10 "AGG/DEF slider"): a press inside the knob grabs it, moving sets the value from the
    /// pointer (`(max - min) * (x - grab - left) / travel + min`, clamped) and every step writes the variable (the slot `FUN_100c08c5` -> `SetValue(v, true)`).
    pub(super) fn input(&mut self, gui: &mut Gui, d: &mut DValues, ev: &InputEvent) {
        let Some(w) = self.win.as_mut() else { return };
        match *ev {
            // the bind dialog takes the middle button as a key (`FUN_100bc58e`: input 9)
            InputEvent::MouseDown { button: MouseButton::Middle, .. } if self.keypages.capturing() => self.keypages.capture_mouse(gui, 9),
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                let page = w.selected;
                let viewport = gui.view_rect(w.id, &format!("pg{page}"));
                for (i, c) in w.controls.iter().enumerate().filter(|(_, c)| c.page == page && c.opt.ctl == Ctl::Slider) {
                    let Some(r) = gui.view_rect(w.id, &c.name) else { continue };
                    if !viewport.is_some_and(|p| x >= p.l && x <= p.r && y >= p.t && y <= p.b) {
                        continue;
                    }
                    let (lo, hi) = c.opt.range(d);
                    let left = r.l + knob_x(c.opt.value(d), lo, hi, gui.canvas_size(w.id, &c.name).0 as f32);
                    if x >= left && x < left + KNOB_W && y >= r.t && y < r.t + KNOB_H {
                        w.grab = Some((i, x - left));
                    }
                }
            }
            InputEvent::MouseMove { x, .. } => {
                if let Some((i, g)) = w.grab {
                    let c = &w.controls[i];
                    if let Some(r) = gui.view_rect(w.id, &c.name) {
                        let (lo, hi) = c.opt.range(d);
                        let travel = (gui.canvas_size(w.id, &c.name).0 as f32 - KNOB_W).max(1.0);
                        let v = (f64::from(x - g - r.l) * (hi - lo) / f64::from(travel) + lo).clamp(lo, hi);
                        c.opt.set_value(d, v);
                    }
                }
            }
            InputEvent::MouseUp { button: MouseButton::Left, .. } => w.grab = None,
            _ => {}
        }
    }
}

/// `Slider_c::_Layout` 0x10144141: `floor((v - min) * travel / (max - min))`, `travel` = slider width - knob width.
fn knob_x(v: f64, lo: f64, hi: f64, width: f32) -> f32 {
    let travel = f64::from((width - KNOB_W).max(0.0));
    if hi <= lo { 0.0 } else { ((v - lo) * travel / (hi - lo)).floor() as f32 }
}

/// The paint of a slider: the background art over the whole width and the knob (`BorderView` bg gfx 0x92, `BitmapView` 0x91 tinted DEFAULT).
fn slider_items(gui: &Gui, width: f32, v: f64, lo: f64, hi: f64) -> Vec<CanvasItem> {
    let mut items = vec![];
    if let Some(g) = gui.gfx_id(SLIDER_BG).map(GfxId) {
        let (w, h) = gui.gfx().size(g);
        items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, width, h as f32], alpha: 1.0 });
    }
    if let Some(g) = gui.gfx_id(SLIDER_KNOB).map(GfxId) {
        let (w, h) = gui.gfx().size(g);
        let x = knob_x(v, lo, hi, width);
        items.push(CanvasItem::ImageTint { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x, 0.0, x + w as f32, h as f32], color: DEFAULT_COLOR, alpha: 1.0 });
    }
    items
}

/// Re-serialises a parsed element (the archive of the template) so [`Config::parse`] can read it.
fn xml_text(e: &xml::Element) -> String {
    let attrs: String = e.attrs.iter().map(|(k, v)| format!(" {k}=\"{}\"", esc(v))).collect();
    let kids: String = e.children.iter().map(xml_text).collect();
    format!("<{}{attrs}>{kids}</{}>", e.name, e.name)
}

#[cfg(test)]
mod tests;
