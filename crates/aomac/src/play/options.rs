//! The options window (`OptionPanelModule_c`, GUI.dll ctor 0xc2c03, module name `optionpanel_window`; docs/gui.md "Options window").
//!
//! Opened by the Windows menu entry `optionpanel_window` (`ActionMenu/Windows.xml`, `MenuEntry label="#Setting"`) / `/open Settings` (the dvalue toggles it like
//! every other window, [`super::hud::WindowKind::Options`]). `ModuleActivated` 0xc2b7a builds `OptionWindow_c` (`FUN_100c3c44`): a style-0 `Window(Rect(200,200,600,400), "", "", 0,
//! 0x1000)` with the tabs "Preferences" (this file), "Fixed keys" and "Key bindings" ([`keypage`]), configured by the `OptionWindowConfig`
//! archive (`LoadWndConfig`). The Preferences tab is an `OptionCategoryPanel_c` (`FUN_100c1053`: the category tree and the two buttons `Quit2Windows` / `Quit2Login`) next to a
//! `ViewSelector_c` holding one page per `ScrollView` of `OptionPanel/Root.xml` (`FUN_100c3691`); every control is bound to its variable ([`model`]).

mod keypage;
pub(super) mod keys;
mod live;
mod model;
#[cfg(test)]
mod probe;

pub(super) use live::{audio_prefs, login_audio_prefs};

use super::dvalue::{DValues, Kind, Variant};
use ao_formats::screens::{TextDb, CAT_GUI};
use ao_gui::view::ListItem;
use ao_gui::{xml, Event, Gui, InputEvent, MouseButton, WindowId, WindowSize};
use model::{Ctl, Item, Opt, Page, Ty};
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
    #[cfg(test)]
    page: usize,
    /// `c<i>` check box, `r<i>` radio group, `s<i>` native slider (value text `v<i>`).
    name: String,
    shown: Option<f64>,
    enabled: Option<bool>,
}

struct Win {
    id: WindowId,
    controls: Vec<Bound>,
    /// Leaf id of the page at each selector index.
    leaves: Vec<String>,
    folders: Vec<String>,
    selected: usize,
    tab: i32,
}

pub(super) struct HudOptions {
    pages: Vec<Page>,
    texts: TextDb,
    template: Option<Config>,
    screen: (u32, u32),
    screen_changed: bool,
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
            Item::Text { value, borders: b } => {
                *out += &format!("<TextView value=\"{}\"{}/>", esc(value), borders(b));
            }
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
                             <Slider name=\"s{i}\" min_size=\"Point(11,17)\" max_size=\"Point(16000,17)\"/></View>",
                            borders(&o.borders),
                            esc(&o.label)
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
    let enhanced = page.label == "Enhanced";
    let mut s = if enhanced { "<View view_layout=\"vertical\">".to_string() } else { String::new() };
    // Reserve the scrollbar width in the Enhanced minimum, not just after the viewport shrinks.
    let (mode, height) = if enhanced { ("auto_reserve", 16000) } else { ("auto", -1) };
    s += &format!("<ScrollView name=\"pg{idx}\" label=\"{}\" v_scrollbar_mode=\"{mode}\" max_size=\"Point(16000,{height})\"><ScrollViewChild><View h_alignment=\"left\" view_layout=\"vertical\" max_size=\"Point(16000,-1)\" layout_borders=\"Rect(0,0,10,0)\">", esc(&page.label));
    for it in &page.items {
        item(it, n, &mut s);
    }
    s += "</View></ScrollViewChild></ScrollView>";
    if enhanced {
        s += "<View view_layout=\"horizontal\" layout_borders=\"Rect(0,5,10,0)\"><Button name=\"restore_original_graphics\" label=\"Restore Original\" tooltip=\"Disable sun shadows, ambient occlusion, emissive bloom and ground grass; other preferences are unchanged.\"/><HLayoutSpacer/></View></View>";
    }
    s
}

/// aomac-only controls, separate from the retail inventory. All effects default off;
/// quality knobs and flags use the existing account-independent DV Main persistence.
fn enhanced_page() -> Page {
    let opt = |var: &str, label: &str, radios: &[(&str, i32)]| Opt {
        ctl: Ctl::Radio(radios.iter().map(|&(l, v)| (l.to_string(), v)).collect()),
        var: var.to_string(),
        ty: Ty::Variant,
        kind: Kind::Login, // unused by Ty::Variant (`Opt::{value,range,set_value}` only read `kind` for Ty::Int/Float)
        label: label.to_string(),
        tooltip: None,
        enable: String::new(),
        bit: None,
        fmt: String::new(),
        scale: 1.0,
        borders: String::new(),
    };
    let check = |var: &str, label: &str| Opt {
        ctl: Ctl::Check,
        var: var.to_string(),
        ty: Ty::Variant,
        kind: Kind::Login,
        label: label.to_string(),
        tooltip: None,
        enable: String::new(),
        bit: None,
        fmt: String::new(),
        scale: 1.0,
        borders: String::new(),
    };
    // `ChatFontSize` is a *retail* DValue (`MainPrefs.xml` template, tenths of a point, range 50-480, read live by
    // `chat/win.rs::WinPrefs::from_dvalues`); retail never gave it an on-screen control (only `/setoption`). The slider
    // here is aomac-only, writing the same retail variable.
    let chat_font = Opt {
        ctl: Ctl::Slider,
        var: "ChatFontSize".to_string(),
        ty: Ty::Variant,
        kind: Kind::Login,
        label: "Chat Font Size".to_string(),
        tooltip: None,
        enable: String::new(),
        bit: None,
        fmt: "%.0f pt".to_string(),
        scale: 0.1,
        borders: String::new(),
    };
    let slider = |var: &str, label: &str, fmt: &str| Opt {
        ctl: Ctl::Slider,
        var: var.to_string(),
        ty: Ty::Variant,
        kind: Kind::Login,
        label: label.to_string(),
        tooltip: None,
        enable: String::new(),
        bit: None,
        fmt: fmt.to_string(),
        scale: 1.0,
        borders: String::new(),
    };
    Page {
        label: "Enhanced".to_string(),
        items: vec![
            Item::Opt(check("AomacSunShadows", "Sun Shadows")),
            Item::Opt(opt("AomacShadowMapSize", "Shadow Map Size", &[("256", 256), ("512", 512), ("1024", 1024), ("2048", 2048), ("4096", 4096)])),
            Item::Opt(check("AomacAmbientOcclusion", "Ambient Occlusion")),
            Item::Opt(slider("AomacAoRadius", "Ambient Occlusion Radius", "%.2f m")),
            Item::Opt(slider("AomacAoStrength", "Ambient Occlusion Strength", "%.2f")),
            Item::Opt(check("AomacEmissiveBloom", "Emissive Bloom")),
            Item::Opt(slider("AomacBloomStrength", "Emissive Bloom Strength", "%.2f")),
            Item::Opt(check("AomacGroundGrass", "Ground Grass")),
            Item::Opt(slider("AomacGrassDensity", "Ground Grass Density", "%.2fx")),
            Item::Opt(slider("AomacGrassDistance", "Ground Grass Distance", "%.1f m")),
            Item::Opt(opt("AomacTextureSharpening", "Texture Sharpening (anisotropic filtering)", &[("Off (1x, retail)", 0), ("2x", 1), ("4x", 2), ("8x", 3), ("16x", 4)])),
            Item::Opt(opt("AomacEdgeSmoothing", "Edge Smoothing (MSAA)", &[("Off (retail)", 0), ("2x", 1), ("4x", 2), ("8x", 3)])),
            Item::Opt(opt("AomacInterfaceScale", "Interface Scale", &[("Auto (retail-equivalent)", 0), ("1x", 1), ("2x", 2), ("3x", 3)])),
            Item::Opt(check("AomacBorderlessFullscreen", "Instant Fullscreen (skip animation, hide menu bar/dock)")),
            Item::Opt(check("AomacPinchZoom", "Pinch to Zoom (trackpad)")),
            Item::Opt(chat_font),
            Item::Opt(check("AomacBetterMipFiltering", "Better Mip Filtering (gamma-correct, coverage-preserving foliage)")),
            Item::Opt(check("AomacWidescreenFOV", "Widescreen FOV (keep the 4:3 vertical angle)")),
            Item::Opt(slider("AomacSceneryDistance", "Extended Scenery Distance (terrain/statels only, 1.0x = retail)", "%.2fx")),
            Item::Opt(check("AomacSmoothText", "Anti-aliased Text (Retina, TrueType fonts only)")),
        ],
    }
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
        let pages = { let mut p = model::parse(&root); p.push(enhanced_page()); p };
        Ok(Self { pages, texts, template, screen, screen_changed: false, win: None, closed: false, actions: vec![], live: live::Live::default(), dir: dir.to_path_buf(), esc: vec![], keypages })
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen_changed |= self.screen != screen;
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
            K::Raid => "esc_raid",
            K::Research => "esc_research",
            K::Vehicle => "esc_vehicle",
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
    pub(super) fn apply(&mut self, d: &mut DValues, zone: &mut super::zone::Zone, out: &mut Vec<ao_net::frame::Frame>) -> Option<String> {
        if self.live.apply(d, zone, out) {
            self.texts.by_key(110, "Feedback_CantDoThisWhileFighting")
        } else {
            None
        }
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
             <View name=\"categories\" view_layout=\"vertical\" layout_borders=\"Rect(10,10,0,10)\">\
             <BorderView layout_borders=\"Rect(0,0,5,10)\"><View layout_borders=\"Rect(5,5,5,5)\"><StringListView name=\"{TREE}\" v_scrollbar_mode=\"auto_reserve\" max_size=\"Point(16000,16000)\"/></View></BorderView>\
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
        let outer = ((r - l + 1.0).max(1.0) as u32, (b - t + 1.0).max(1.0) as u32);
        let client = (outer.0.saturating_sub(CHROME.0).max(1), outer.1.saturating_sub(CHROME.1).max(1));
        let (x, y) = (l as i32, t as i32);
        let id = match gui.open_tabbed_window_xml("OptionWindow", "Preferences", &src, (x, y), WindowSize::Fixed(client.0, client.1)) {
            Ok(w) => w,
            Err(e) => return eprintln!("hud: options window: {e:#}"),
        };
        gui.set_window_frame(id, true, true);

        gui.batch_updates(|gui| {
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
                controls.push(Bound { opt: o.clone(), #[cfg(test)] page: pi, name, shown: None, enabled: None });
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
        self.win = Some(Win { id, controls, leaves, folders, selected, tab: cfg.tab });
        if let Some(leaf) = self.win.as_ref().and_then(|w| w.leaves.get(w.selected).cloned()) {
            gui.list_select(id, TREE, &leaf, true, false);
        }
        // ViewSelector measures every page; restore only the selected page's visibility.
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
        gui.select_child(id, TABS, Some(0));
        gui.select_child(id, TABS, Some(sel));
        if let Some(w) = self.win.as_mut() {
            w.tab = sel as i32;
        }
        });
        // OptionWindow_c 0x100c3c44 caps the category panel to its minimum preferred width.
        gui.fit_view_width(id, "categories");
        gui.relayout_window(id);
        gui.set_window_preferred_size_limits(id, true);
        // Load the requested frame only after final content limits, then MoveInsideScreen.
        gui.set_window_outer_frame(id, (x, y, outer.0.min(self.screen.0), outer.1.min(self.screen.1)));
        if let Some(frame) = gui.window_outer_frame(id) {
            let (px, py) = super::hud_wincfg::inside_screen(frame, self.screen);
            gui.set_window_outer_frame(id, (px.max(0), py.max(0), frame.2, frame.3));
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
        // Options is not tracked by HudWinCfg: re-fit once after a viewport change.
        // Ordinary updates must preserve user drags, especially when the minimum
        // window is wider than a scaled logical viewport.
        if self.screen_changed {
            self.screen_changed = false;
            if let Some((x, y, width, height)) = gui.window_outer_frame(id) {
                gui.set_window_outer_frame(id, (x, y, width.min(self.screen.0), height.min(self.screen.1)));
                if let Some(frame) = gui.window_outer_frame(id) {
                    let (px, py) = super::hud_wincfg::inside_screen(frame, self.screen);
                    gui.set_window_outer_frame(id, (px.max(0), py.max(0), frame.2, frame.3));
                }
            }
        }
        self.keypages.sync(gui, d, &self.texts);
        self.keypages.refresh_fixed(gui, &keys::FixedKeys::from_prefs(&d.prefs));
        for c in &mut w.controls {
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
                    let (lo, hi) = c.opt.range(d);
                    gui.set_slider_range(id, &c.name, lo, hi);
                    gui.set_slider_value(id, &c.name, v);
                    if c.shown != Some(v) {
                        c.shown = Some(v);
                        gui.set_text(id, &format!("v{}", &c.name[1..]), &c.opt.value_text(v));
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
                    "restore_original_graphics" => d.restore_original_graphics(),
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
            Event::SliderChanged { window, view, value, .. } if *window == w.id => {
                if let Some(c) = w.controls.iter().find(|c| c.name == *view && c.opt.ctl == Ctl::Slider) {
                    c.opt.set_value(d, *value);
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

    /// Binding capture remains raw input; native sliders dispatch through their value signal.
    pub(super) fn input(&mut self, gui: &mut Gui, _d: &mut DValues, ev: &InputEvent) {
        if self.win.is_some() && self.keypages.capturing() && matches!(ev, InputEvent::MouseDown { button: MouseButton::Middle, .. }) {
            self.keypages.capture_mouse(gui, 9);
        }
    }
}


/// Re-serialises a parsed element (the archive of the template) so [`Config::parse`] can read it.
fn xml_text(e: &xml::Element) -> String {
    let attrs: String = e.attrs.iter().map(|(k, v)| format!(" {k}=\"{}\"", esc(v))).collect();
    let kids: String = e.children.iter().map(xml_text).collect();
    format!("<{}{attrs}>{kids}</{}>", e.name, e.name)
}

#[cfg(test)]
mod tests;
