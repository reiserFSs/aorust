//! In-world interface (`ControlCenterModule_c`, GUI.dll 0x1006c64d): the control-centre overlay `Views/ControlCenter.xml`
//! with its dock views, the health / nano / XP bar windows, the window-opening menus (`ActionMenu/*.xml`) and the sub-windows
//! the other Hud files own. Evidence and layout rules: docs/gui.md §10.

use super::hud_rollup::{Rollup, RollupEvent};
use super::hud_stats::HudStats;
use super::hud_winb::HudWinB;
use super::hud_nano::HudNano;
use super::hud_ncu::HudNcu;
use super::options::HudOptions;
use super::hud_mission::HudMission;
use super::hud_map::HudMap;
pub(super) use super::hud_map::ground_map;
use super::hud_aggdef::{self, AggDef};
use super::hud_bar::{self, ShortcutBar, SlotUse};
use super::hud_actions::HudActions;
use super::hud_special::SpecialList;
use super::hud_actionwin::HudActionWin;
use super::hud_keys::{HudKey, KeyMap};
use super::options::keys::{Bindings, FixedKeys};
use super::hud_pools;
use super::hud_compass::Compass;
use super::hud_target::HudTarget;
use super::zone::Zone;
use ao_formats::stats;
use ao_net::frame::Frame;
use ao_gui::xml::{self, Element};
use ao_gui::{Event, Gui, InputEvent, MouseButton, WindowId, WindowSize};
use super::dvalue::DValues;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Windows the control centre opens (`name=` of the `ActionMenu/*.xml` entries, which are also the `dvalue` names in `CharPrefs.xml`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowKind {
    Skills,
    Inventory,
    /// `wear_window`
    Character,
    Target,
    /// `map_window` (playfield map); the planet map is `planetmap_window`.
    Map,
    PlanetMap,
    Nano,
    Ncu,
    Mission,
    Friends,
    Team,
    Perks,
    Faction,
    Pet,
    /// `stat_window` (`StatView_c`)
    Stat,
    /// `specialaction_window` (`SpecialActionView_c`, tab "Actions", Ctrl+2)
    Actions,
    /// `optionpanel_window` (`OptionPanelModule_c`, `options.rs`; the Windows menu entry "Settings")
    Options,
}

impl WindowKind {
    pub const ALL: [WindowKind; 17] = [
        WindowKind::Skills,
        WindowKind::Inventory,
        WindowKind::Character,
        WindowKind::Target,
        WindowKind::Map,
        WindowKind::PlanetMap,
        WindowKind::Nano,
        WindowKind::Ncu,
        WindowKind::Mission,
        WindowKind::Friends,
        WindowKind::Team,
        WindowKind::Perks,
        WindowKind::Faction,
        WindowKind::Pet,
        WindowKind::Actions,
        WindowKind::Stat,
        WindowKind::Options,
    ];

    /// The dvalue / menu entry name (`ActionMenu/*.xml` `name=`).
    pub fn dvalue(self) -> &'static str {
        match self {
            WindowKind::Skills => "skill_window",
            WindowKind::Inventory => "inventory_window",
            WindowKind::Character => "wear_window",
            WindowKind::Target => "target_window",
            WindowKind::Map => "map_window",
            WindowKind::PlanetMap => "planetmap_window",
            WindowKind::Nano => "nano_window",
            WindowKind::Ncu => "ncu_window",
            WindowKind::Mission => "mission_window",
            WindowKind::Friends => "friends_window",
            WindowKind::Team => "team_view",
            WindowKind::Perks => "perk_window",
            WindowKind::Faction => "faction_window",
            WindowKind::Pet => "pet_window",
            WindowKind::Actions => "specialaction_window",
            WindowKind::Stat => "stat_window",
            WindowKind::Options => "optionpanel_window",
        }
    }

    pub fn from_dvalue(name: &str) -> Option<WindowKind> {
        WindowKind::ALL.into_iter().find(|k| k.dvalue() == name)
    }
}

/// One `Menu` / `MenuEntry` of `ActionMenu/*.xml` (`FUN_100660f2` 0x100660f2 attributes).
#[derive(Debug, Clone, Default)]
struct MenuNode {
    name: String,
    /// Stable GUI identity, separate from the optional DValue name.
    id: String,
    label: String,
    bgicon: String,
    tooltip: String,
    tooltip_body: String,
    criteria: String,
    golden: bool,
    button: bool,
    children: Vec<MenuNode>,
    actions: Vec<Element>,
    invoke: String,
    active_value: String,
    category: Option<i32>,
    templates: Vec<Element>,
    /// Generated provider identity and the currently shown Action_e.
    special: Option<(u32, u32)>,
}

impl MenuNode {
    /// `FUN_10071215` (0x10071215): children are the `Menu` / `MenuEntry` elements (`sub_script` replaces them with the file's root).
    fn load(e: &Element, dir: &Path, depth: u8) -> MenuNode {
        let a = |k: &str| e.attr(k).unwrap_or("").to_string();
        let mut n = MenuNode {
            name: a("name"),
            id: String::new(),
            label: a("label"),
            bgicon: a("bgicon"),
            tooltip: a("tooltip"),
            tooltip_body: a("tooltip_body"),
            criteria: a("criteria"),
            golden: e.attr("golden_button").is_some(),
            button: e.attr("button_mode").is_some_and(|m| m.eq_ignore_ascii_case("button")),
            children: vec![],
            actions: e.children.iter().filter(|c| c.name.eq_ignore_ascii_case("actions")).cloned().collect(),
            invoke: a("invoke"),
            active_value: a("active_value"),
            category: e.attr("special_action_category").and_then(menu_constant).map(|v| v as i32),
            templates: e.children.iter().filter(|c| c.name.eq_ignore_ascii_case("itemtemplate")).cloned().collect(),
            special: None,
        };
        let mut kids: &[Element] = &e.children;
        let sub;
        if let Some(s) = e.attr("sub_script").filter(|_| depth < 4) {
            if let Some(r) = std::fs::read_to_string(dir.join(s)).ok().and_then(|t| xml::parse(&t).ok()) {
                sub = r;
                kids = &sub.children;
                // the sub file's root `Menu` supplies the label of the entry when the parent has none (FUN_10065dec)
            }
        }
        for c in kids.iter().filter(|c| c.name.eq_ignore_ascii_case("menu") || c.name.eq_ignore_ascii_case("menuentry")) {
            n.children.push(MenuNode::load(c, dir, depth + 1));
        }
        n
    }

    fn identify(&mut self, path: &str) {
        self.id = if self.name.is_empty() || !self.invoke.is_empty() || !self.active_value.is_empty() { format!("__menu_{path}") } else { self.name.clone() };
        for (i, child) in self.children.iter_mut().enumerate() {
            child.identify(&format!("{path}_{i}"));
        }
    }

    /// GUI 0x10070c04: category is template stat 588; overrides match GetName case-insensitively, then "default".
    fn populate(&mut self, items: &[(u32, i32, String, u32)], res: &impl Fn(&str, &str) -> Option<i64>) {
        if let Some(category) = self.category {
            self.children.clear();
            for (instance, item_category, name, action) in items {
                if *item_category != category { continue; }
                let template = self.templates.iter().find(|t| t.attr("action_name").is_some_and(|n| n.eq_ignore_ascii_case(name)))
                    .or_else(|| self.templates.iter().find(|t| t.attr("action_name").unwrap_or("default").eq_ignore_ascii_case("default")));
                let mut entry = template.map(|t| MenuNode::load(t, Path::new("."), 4)).unwrap_or_default();
                if template.is_none_or(|t| t.attr("label").is_none()) { entry.label = name.clone(); }
                if template.is_none_or(|t| t.attr("button_mode").is_none()) { entry.button = true; }
                entry.special = Some((*instance, *action));
                entry.identify(&format!("{}_sa_{instance}", self.id));
                self.children.push(entry);
            }
        }
        for child in &mut self.children { child.populate(items, res); }
        // GUI 0x1006564e / 0x100706d1: active means criteria-enabled, not cooldown-free.
        let active = self.children.iter().filter(|c| c.criteria.is_empty() || ao_gui::expr::truthy(&c.criteria, res)).count();
        self.criteria = self.criteria.replace("active_item_count", &active.to_string()).replace("item_count", &self.children.len().to_string());
    }

    fn has_categories(&self) -> bool {
        self.category.is_some() || self.children.iter().any(MenuNode::has_categories)
    }

    fn walk<'a>(&'a self, out: &mut HashMap<String, &'a MenuNode>) {
        if !self.id.is_empty() {
            out.insert(self.id.clone(), self);
        }
        for c in &self.children {
            c.walk(out);
        }
    }

    /// `CCMenuEntry` element for the engine (`view.rs` "CCMenuEntry"); entries failing their criteria collapse out of the stack.
    fn xml(&self) -> String {
        let mut s = String::from("<CCMenuEntry layout_borders=\"Rect(0,0,0,4)\" view_flags=\"0x100\"");
        let mut attr = |k: &str, v: &str| {
            if !v.is_empty() {
                s += &format!(" {k}=\"{}\"", esc(v));
            }
        };
        attr("name", &self.id);
        attr("label", &self.label);
        attr("bgicon", &self.bgicon);
        attr("tooltip", &self.tooltip);
        attr("tooltip_body", &self.tooltip_body);
        attr("criteria", &self.criteria);
        if self.golden {
            attr("golden_button", "true");
        }
        if self.button {
            attr("button_mode", "button");
        }
        if !self.children.is_empty() || self.category.is_some() {
            attr("submenu", "1");
        }
        s + "/>"
    }
}

/// GUI 0x1006a968 and CameraCoordinator 0x10064631 register these original expression constants.
fn menu_constant(name: &str) -> Option<i64> {
    match name {
        "AM_CATEGORY_PERKS" => Some(1),
        "AM_CATEGORY_NORMAL" => Some(2),
        "AM_CATEGORY_ATTACK" => Some(3),
        "CAMERA_1ST" => Some(0),
        "CAMERA_3RD_FREE" => Some(1),
        "CAMERA_3RD_RUBBER" => Some(2),
        "CAMERA_3RD_LOCK" => Some(3),
        _ => None,
    }
}

fn menu_eval(value: &str, res: &impl Fn(&str, &str) -> Option<i64>) -> i64 {
    menu_constant(value).unwrap_or_else(|| ao_gui::expr::eval(value, res))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The four character bar windows of `ControlCenterModule_c::SlotPlayerCharacterAlive` (GUI 0x1006afed): `CharacterBar_c` 0x10066af6
/// (`PowerbarView_c(rect, bg, full, left, right, Direction_e 3 = up, …)` inside a style-3 `CharBarWindow_c` 0x1006d7c7).
struct BarSpec {
    cfg: &'static str,
    criteria: &'static str,
    /// bg, full, left cap, right cap
    gfx: [&'static str; 4],
}

const BARS: [BarSpec; 4] = [
    BarSpec {
        cfg: "CCHealthBarConfig",
        criteria: "dvalue:cc_section1 && dvalue:cc_health_bar",
        gfx: ["GFX_GUI_ACTIONVIEW_HEALTHBAR_BACKGROUND", "GFX_GUI_ACTIONVIEW_HEALTHBAR", "GFX_GUI_ACTIONVIEW_HEALTHBAR_LEFT", "GFX_GUI_ACTIONVIEW_HEALTHBAR_RIGHT"],
    },
    BarSpec {
        cfg: "CCNanoBarConfig",
        criteria: "dvalue:cc_section1 && dvalue:cc_nano_bar",
        gfx: ["GFX_GUI_ACTIONVIEW_NANOBAR_BACKGROUND", "GFX_GUI_ACTIONVIEW_NANOBAR", "GFX_GUI_ACTIONVIEW_NANOBAR_LEFT", "GFX_GUI_ACTIONVIEW_NANOBAR_RIGHT"],
    },
    BarSpec {
        cfg: "CCXPBarConfig",
        criteria: "dvalue:cc_section1 && dvalue:cc_xp_bar",
        gfx: ["GFX_GUI_ACTIONVIEW_XPBAR_BACKGROUND", "GFX_GUI_ACTIONVIEW_XPBAR", "GFX_GUI_ACTIONVIEW_XPBAR_LEFT", "GFX_GUI_ACTIONVIEW_XPBAR_RIGHT"],
    },
    BarSpec {
        cfg: "CCAlienXPBarConfig",
        criteria: "dvalue:cc_section1 && dvalue:cc_alienxp_bar",
        gfx: ["GFX_GUI_ACTIONVIEW_ALIENXPBAR_BACKGROUND", "GFX_GUI_ACTIONVIEW_ALIENXPBAR", "GFX_GUI_ACTIONVIEW_ALIENXPBAR_LEFT", "GFX_GUI_ACTIONVIEW_ALIENXPBAR_RIGHT"],
    },
];

/// Stat ids the bars read (`FUN_100666b6` 0x100666b6 health, `FUN_100667a8` nano, `FUN_100668aa` XP, `FUN_100669f7` alien XP).
mod sid {
    pub const MAX_HEALTH: u32 = 1;
    pub const HEALTH: u32 = 27;
    pub const MAX_NANO: u32 = 221;
    pub const NANO: u32 = 214;
    pub const ALIEN_XP: u32 = 40;
    pub const ALIEN_XP_NEXT: u32 = 178;
    pub const NCU_USED: u32 = 180;
    pub const NCU_MAX: u32 = 181;
    pub const CASH: u32 = 61;
    pub const EXPANSION: u32 = 389;
}


struct Bar {
    window: WindowId,
    spec: &'static BarSpec,
}

struct Popup {
    window: WindowId,
    /// menu entry name that opened it
    owner: String,
    anchor: ao_gui::Rect,
    nodes: Vec<MenuNode>,
    source: Option<MenuNode>,
    action_snapshot: Vec<super::hud_special::Entry>,
}

pub(super) struct Hud {
    /// The full-screen `ControlCenter.xml` window.
    cc: WindowId,
    size: (u32, u32),
    /// The distributed-value store (`play/dvalue.rs`): window flags, prefs, `/option` `/dvalue` (docs/chat/dvalue.md).
    pub(super) dvalues: DValues,
    /// Tooltip titles of the health / nano / XP / alien XP bars (text.mdb category 0x2710).
    bar_titles: [String; 4],
    bars: Vec<Bar>,
    menu_roots: Vec<MenuNode>,
    popup: Option<Popup>,
    menu_scripts: Vec<String>,
    menu_camera: Option<u8>,
    open: Vec<WindowKind>,
    /// Skills / inventory / wear windows (`hud_stats.rs`).
    stats: HudStats,
    /// The `RollupArea` dock (docs/gui.md §11.13): the wear and stat windows are its pages; other windows dock with `Rollup::open_page`.
    pub(super) rollup: Rollup,
    /// Programs and NCU windows (`hud_nano.rs`, `hud_ncu.rs`).
    nano: HudNano,
    ncu: HudNcu,
    /// The options window (`options.rs`).
    options: HudOptions,
    /// `file://` pages the `?` buttons of window frames asked for (`WndBorder::SlotHelpButton`; the flow shows them in the InfoView).
    help_urls: Vec<String>,
    /// The Mission window (`hud_mission.rs`).
    mission: HudMission,
    /// System-window lines the NCU window produced (the flow prints them).
    system_lines: Vec<String>,
    /// Playfield map and planet map windows (`hud_map.rs`).
    map: HudMap,
    /// Team, Perks and Faction windows (`hud_winb.rs`).
    winb: HudWinB,
    /// Target docks, health-bar windows and click-to-select (`hud_target.rs`).
    target: HudTarget,
    /// Shortcut bars (`hud_bar.rs`).
    shortcuts: Vec<ShortcutBar>,
    /// The special-action list the hotbar slots and the Actions window show (`hud_actions.rs`).
    pub(super) actions: HudActions,
    /// The Actions window (`hud_actionwin.rs`).
    actwin: HudActionWin,
    /// Hot keys from the `KeyBindings` archive (`hud_keys.rs`) and the archive text they were built from.
    keys: KeyMap,
    keys_src: String,
    /// The shared key binding table and the fixed keys the HUD and the game's controls read (`options/keys.rs`).
    bindings: Bindings,
    fixed: FixedKeys,
    /// The compass window (`hud_compass.rs`).
    compass: Option<Compass>,
    /// The AGG/DEF slider of the right control-centre bar (`hud_aggdef.rs`).
    aggdef: AggDef,
    /// Zone frames produced by the HUD (the slider's `SetStatIIR_t`); the flow drains them.
    outbox: Vec<Frame>,
    /// Activated hotbar slots the game has to carry out (the flow drains them).
    uses: Vec<SlotUse>,
    /// The character a plain world click selected this frame ([`Hud::take_click`]).
    click: Option<i32>,
    /// Saved places of the movable windows (`hud_wincfg.rs`).
    wincfg: super::hud_wincfg::WinCfgs,
}

impl Hud {
    pub(super) fn new(gui: &mut Gui, dir: &Path, size: (u32, u32)) -> anyhow::Result<Self> {
        let cc = gui.open_window("ControlCenter", (0, 0), WindowSize::Fixed(size.0, size.1))?;
        let menus_dir: PathBuf = dir.join("cd_image/gui/Default/ActionMenu");
        let menu_roots: Vec<MenuNode> = ["LeftMenu.xml", "RightMenu.xml"]
            .iter()
            .enumerate().filter_map(|(i, f)| {
                let root = xml::parse(&std::fs::read_to_string(menus_dir.join(f)).ok()?).ok()?;
                let mut node = MenuNode::load(&root, &menus_dir, 0);
                node.identify(&i.to_string());
                Some(node)
            })
            .collect();
        let target = HudTarget::new(gui, cc, size)?;
        let texts = ao_formats::screens::TextDb::load(dir)?;
        let bar_titles = ["Health", "Nano", "Experience", "AlienExperience"].map(|k| texts.by_key(ao_formats::screens::CAT_GUI, k).unwrap_or_else(|| k.to_string()));
        let compass = Compass::new(gui, size).map_err(|e| eprintln!("hud: compass: {e:#}")).ok();
        let mut hud = Hud { cc, size, dvalues: DValues::new(dir), bars: vec![], bar_titles, menu_roots, popup: None, menu_scripts: vec![], menu_camera: None, open: vec![], stats: HudStats::new(dir, size)?, rollup: Rollup::new(dir, size), nano: HudNano::new(dir, size)?, ncu: HudNcu::new(dir, size)?, options: HudOptions::new(dir, size)?, help_urls: vec![], mission: HudMission::new(dir, size)?, system_lines: vec![], map: HudMap::new(dir), winb: HudWinB::new(dir, size), target, shortcuts: vec![], actions: HudActions::new(dir), actwin: HudActionWin::new(dir, size), keys: KeyMap::default(), keys_src: String::new(), bindings: Bindings::default(), fixed: FixedKeys::default(), compass, aggdef: AggDef::default(), outbox: vec![], uses: vec![], click: None, wincfg: super::hud_wincfg::WinCfgs::new(dir) };
        hud.target.targets_target = hud.dvalues.flag("Targetstarget");
        hud.rollup.restore_docks(gui, &hud.wincfg.load_docks(&hud.dvalues));
        hud.fill_docks(gui);
        hud.create_bars(gui);
        for n in 0..hud_bar::default_count(&hud.dvalues) {
            match ShortcutBar::new(gui, dir, n, size) {
                Ok(b) => hud.shortcuts.push(b),
                Err(e) => eprintln!("hud: shortcut bar {n}: {e:#}"),
            }
        }
        hud.refresh(gui, &Zone::default());
        // the windows the NewChar template opens at the first login (`<Value name="wear_window" value="true">`, `stat_window`; the wear window first, the stat
        // window is placed below it); the other windows of the template (nano, ncu, team) are not implemented by this HUD
        let template = std::fs::read_to_string(dir.join("prefs/NewChar/Prefs.xml")).ok().and_then(|t| xml::parse(&t).ok());
        for k in [WindowKind::Character, WindowKind::Stat, WindowKind::Team] {
            if template.as_ref().is_some_and(|r| r.children.iter().any(|c| c.name == "Value" && c.attr("name") == Some(k.dvalue()) && c.attr("value") == Some("true"))) {
                hud.open(gui, k);
            }
        }
        Ok(hud)
    }

    /// `FUN_1006f098` (0x1006f098): wings, bottom bars and menus go into the dock views of `ControlCenter.xml`.
    fn fill_docks(&mut self, gui: &mut Gui) {
        let wing = |id: &str| format!("<root><BitmapView bitmap_id=\"{id}\" color=\"0x1000000\"/></root>");
        // LeftBarView_c / RightBarView_c are BitmapViews with an HLayoutNode whose child View (borders) holds the labels
        let bar = |id: &str, inner: &str, borders: &str| {
            format!(
                "<root><BitmapView bitmap_id=\"{id}\" color=\"0x1000000\" view_layout=\"horizontal\">\
                 <View view_layout=\"horizontal\" layout_borders=\"{borders}\">{inner}</View></BitmapView></root>"
            )
        };
        // LeftBarView_c 0x1006f7c5: [NCU][0/0] <spacer> [CRED][0]; RightBarView_c 0x1006ff09: [DEF] slider [AGG]
        let left = bar(
            "GFX_GUI_CONTROLCENTER_BOTTOM_LEFT",
            "<TextView value=\"NCU\" layout_borders=\"Rect(0,0,5,0)\"/><TextView name=\"ncu\" value=\"0/0\"/>\
             <HLayoutSpacer min_size=\"10\" max_size=\"16000\"/>\
             <TextView value=\"CRED\" layout_borders=\"Rect(0,0,5,0)\"/><TextView name=\"cash\" value=\"0\"/>",
            "Rect(6,2,23,2)",
        );
        let right = bar(
            "GFX_GUI_CONTROLCENTER_BOTTOM_RIGHT",
            &format!(
                "<TextView value=\"DEF\" color=\"0x1000000\" layout_borders=\"Rect(0,0,5,0)\"/>{}\
                 <TextView value=\"AGG\" color=\"0x1000000\" layout_borders=\"Rect(5,0,0,0)\"/>",
                hud_aggdef::view_xml()
            ),
            "Rect(23,2,6,2)",
        );
        let docks = [
            ("LeftWingDock", wing("GFX_GUI_CONTROLCENTER_WING_LEFT")),
            ("LeftBarDock", left),
            ("RightWingDock", wing("GFX_GUI_CONTROLCENTER_WING_RIGHT")),
            ("RightBarDock", right),
        ];
        for (dock, src) in docks {
            if let Err(e) = gui.add_view_xml(self.cc, dock, dock, &src) {
                eprintln!("hud: {dock}: {e:#}");
            }
        }
        self.fit_rollup_dock(gui);
        for (script, root) in ["LeftMenu.xml", "RightMenu.xml"].into_iter().zip(&self.menu_roots) {
            for c in &root.children {
                if let Err(e) = gui.add_view_xml(self.cc, script, script, &format!("<root>{}</root>", c.xml())) {
                    eprintln!("hud: {script}: {e:#}");
                }
            }
        }
    }

    /// `RollupArea` (`DockArea_c`, `ControlCenterModule_c::InitialiseMessage` 0x1006a968): the area is `Rect(W-191, 20, W, H-225)` (asm 0x1006ab32..0x1006ab75: the
    /// doubles 191 / 225 / the float 20 at 0x101b4e00 / 0x101b4e10 / 0x101b4e08), a 192 px wide column (earlier notes had the constants swapped); the rollup dock view
    /// carries that width, which pushes the right wing and bar to the bottom of the right column. The dock's pages (friends, wear, nano, stat) are
    /// `hud_rollup.rs` windows.
    fn fit_rollup_dock(&mut self, gui: &mut Gui) {
        let wing = gui.gfx_id("GFX_GUI_CONTROLCENTER_WING_RIGHT").map_or(194, |g| gui.gfx().size(ao_gui::GfxId(g)).1);
        let h = (self.size.1 as i32 - 5 - (wing as i32 + 5) - (20 + 5)).max(0);
        gui.remove_children(self.cc, "RollupControllerDock");
        let w = super::hud_rollup::AREA_W;
        let src = format!("<root><View name=\"RollupArea\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/></root>");
        if let Err(e) = gui.add_view_xml(self.cc, "RollupControllerDock", "RollupArea", &src) {
            eprintln!("hud: RollupControllerDock: {e:#}");
        }
    }

    /// The fade dvalues, applied live: `CCFadeLow` / `CCFadeHigh` / `CCFadeDelay` drive the `FadeGroupController_c` (gui/fade.rs, docs/gui.md
    /// §10.8), `cc_rollup_controller_fade_level` the alpha of the black surface behind the rollup column (`RollupController_c` 0x10048346).
    fn apply_fades(&self, gui: &mut Gui) {
        let f = |n: &str, d: f32| self.dvalues.get_f32(n).unwrap_or(d);
        gui.set_fade_params(f("CCFadeLow", 0.33), f("CCFadeHigh", 0.85), f("CCFadeDelay", 2.0));
        gui.set_backdrop(self.cc, "RollupArea", f("cc_rollup_controller_fade_level", 0.0));
        // `cc_rollup_panel` (CharPrefs, default true; `RollupController_c` sets it when a page is added) = the rollup column is shown
        gui.set_visible(self.cc, "RollupArea", !self.dvalues.exists("cc_rollup_panel") || self.dvalues.flag("cc_rollup_panel"));
    }

    /// `CharBarWindow_c` windows (0x1006d7c7) created by `SlotPlayerCharacterAlive` (GUI 0x1006afed) at the points it passes in: health (0, 0), nano
    /// (0, Height(health frame) + 20.0), XP (Width(health frame) + 3.0, 0), alien XP (Width(nano frame) + 3.0, Height(XP frame) + 20.0)
    /// (`_DAT_101ae300` = 20.0, `_DAT_101a8a20` = 3.0, doubles); then `LoadWndConfig` (nothing saved on a first login) and `MoveInsideScreen`.
    /// The install's `prefs/NewChar` frames are NOT used: they were authored on a 2304 x 1440 screen, no DLL reads that directory, and retail
    /// screenshots show the pool bars at the left screen edge (docs/gui.md 10.4).
    fn create_bars(&mut self, gui: &mut Gui) {
        for spec in BARS.iter() {
            let [bg, full, l, r] = spec.gfx;
            let src = format!("<root><PowerBar name=\"bar\" bg_gfx=\"{bg}\" full_gfx=\"{full}\" left_gfx=\"{l}\" right_gfx=\"{r}\" direction=\"up\"/></root>");
            match gui.open_window_xml(spec.cfg, &src, (0, 0), WindowSize::Preferred) {
                Ok(window) => {
                    let (w, h) = gui.window_size(window);
                    let size = |cfg: &str| self.bars.iter().find(|b| b.spec.cfg == cfg).map(|b| gui.window_size(b.window)).unwrap_or((w, h));
                    let (x, y) = match spec.cfg {
                        "CCNanoBarConfig" => (0, size("CCHealthBarConfig").1 as i32 + 20),
                        "CCXPBarConfig" => (size("CCHealthBarConfig").0 as i32 + 3, 0),
                        "CCAlienXPBarConfig" => (size("CCNanoBarConfig").0 as i32 + 3, size("CCXPBarConfig").1 as i32 + 20),
                        _ => (0, 0),
                    };
                    let x = x.min(self.size.0.saturating_sub(w) as i32).max(0);
                    let y = y.min(self.size.1.saturating_sub(h) as i32).max(0);
                    gui.set_window_pos(window, (x, y));
                    self.bars.push(Bar { window, spec });
                }
                Err(e) => eprintln!("hud: {}: {e:#}", spec.cfg),
            }
        }
    }

    pub(super) fn resize(&mut self, gui: &mut Gui, size: (u32, u32)) {
        if size != self.size {
            self.wincfg.resize_screen(gui, self.size, size);
            self.size = size;
            self.stats.set_screen(size);
            self.rollup.set_screen(gui, size);
            self.winb.set_screen(size);
            self.nano.set_screen(size);
            self.ncu.set_screen(size);
            self.options.set_screen(size);
            self.actwin.set_screen(size);
            self.mission.set_screen(size);
            gui.resize_window(self.cc, WindowSize::Fixed(size.0, size.1));
            for s in &mut self.shortcuts {
                s.resize(gui, size);
            }
            self.fit_rollup_dock(gui);
            if let Some(c) = &mut self.compass {
                c.resize(gui, size);
            }
            self.target.resize(gui, size);
            for b in &self.bars {
                let (w, h) = gui.window_size(b.window);
                let p = gui.window_pos(b.window);
                let x = p.0.min(size.0.saturating_sub(w) as i32).max(0);
                let y = p.1.min(size.1.saturating_sub(h) as i32).max(0);
                gui.set_window_pos(b.window, (x, y));
            }
        }
    }

    /// System-window lines to print (the NCU window's remove feedback, `GlobalSignals+0x17c`).
    pub(super) fn take_system_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.system_lines)
    }

    /// Programs the nano window cast (`N3Msg_CastNanoSpell`): the chat prints their "Executing Nano Program" lines.
    pub(super) fn take_casts(&mut self) -> Vec<String> {
        self.nano.take_casts()
    }

    /// The zone frames [`Hud::on_zone_frame`] reads (the flow keeps them until the HUD exists).
    pub(super) fn wants_zone_frame(f: &Frame) -> bool {
        super::hud_winb::wants(f) || super::hud_mission::wants(f)
    }

    /// Every zone frame: team members / invitations, the own perk map and perk updates (`hud_winb.rs`).
    pub(super) fn on_zone_frame(&mut self, f: &Frame, own: i32) {
        self.winb.on_zone_frame(f, own);
        self.mission.on_frame(f, own);
    }

    /// Zone frames produced by the HUD since the last call.
    pub(super) fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    #[cfg(test)]
    pub(super) fn use_item_on(&mut self, zone: &Zone, slot: u32, target: ao_net::msg::Identity) {
        self.stats.use_on(zone, slot, target);
        self.outbox.extend(self.stats.take_outbox());
    }

    #[cfg(test)]
    pub(super) fn live_item_line(&mut self, gui: &mut Gui, low_id: i32) -> String {
        self.stats.live_item_line(gui, low_id)
    }

    #[cfg(test)]
    pub(super) fn live_double_click(&mut self, zone: &Zone, slot: u32) {
        self.stats.live_double_click(zone, slot);
        self.outbox.extend(self.stats.take_outbox());
    }

    /// True while the pointer carries an inventory item.
    pub(super) fn item_dragging(&self) -> bool {
        self.stats.dragging_item()
    }

    /// See [`HudStats::set_world_under`].
    pub(super) fn set_world_under(&mut self, id: Option<ao_net::msg::Identity>) {
        self.stats.set_world_under(id);
    }

    /// Inventory items released over a foreign window since the last call (`HudStats::take_drops`).
    pub(super) fn take_item_drops(&mut self) -> Vec<(u32, f32, f32)> {
        self.stats.take_drops()
    }

    /// Puts back the drops no window claimed.
    pub(super) fn requeue_item_drops(&mut self, drops: Vec<(u32, f32, f32)>) {
        self.stats.requeue_drops(drops);
    }

    /// Name and icon of an item template (`HudStats::item_info`).
    pub(super) fn item_info(&mut self, gui: &mut Gui, low_id: i32) -> Option<super::hud_stats::ItemInfo> {
        self.stats.item_info(gui, low_id)
    }

    /// Name, count and price of an item template (`HudStats::shop_info`).
    pub(super) fn shop_info(&mut self, gui: &mut Gui, item: ao_net::n3::world::AcgItem) -> Option<(String, i32, i32, Option<ao_gui::GfxId>)> {
        self.stats.shop_info(gui, item)
    }

    /// Hotbar slots activated since the last call (`FUN_100d79c9`).
    /// The character a world click selected since the last call (CTRL/ALT + click also attacks it: `FUN_1002c469`).
    pub(super) fn take_click(&mut self) -> Option<i32> {
        self.click.take()
    }

    /// The character a Shift + click asked the info page of since the last call (`ShowURL("charid://50000/<id>")`, hud_target.rs).
    pub(super) fn take_info(&mut self) -> Option<i32> {
        self.target.info.take()
    }

    /// InfoView pages the Mission window asked for (`hud_mission.rs`).
    pub(super) fn take_info_urls(&mut self) -> Vec<String> {
        let mut v = self.mission.take_urls();
        v.append(&mut self.help_urls);
        v
    }

    /// The game's own mouse pointer over the world (`MousePointerModule_t`, hud_cursor.rs); appended to the frame's draw list.
    pub(super) fn draw_cursor(&mut self, gui: &Gui, zone: &Zone, host: &mut ao_render::Host, list: &mut ao_gui::DrawList) {
        let mode = self.dvalues.get_i64("MouseCursorMode").unwrap_or(2);
        let dblclick = self.dvalues.get_i64("DoubleclickAction").is_none_or(|v| v != 0);
        self.target.draw_cursor(gui, zone, host, list, self.size, mode, dblclick);
    }

    pub(super) fn take_uses(&mut self) -> Vec<SlotUse> {
        std::mem::take(&mut self.uses)
    }

    fn res<'a>(&'a self, zone: &'a Zone) -> impl Fn(&str, &str) -> Option<i64> + 'a {
        resolver(&self.dvalues, zone)
    }

    /// `/quit` / `/camp` the options window's `Quit2Windows` / `Quit2Login` buttons asked for (the flow runs the chat commands' game actions).
    pub(super) fn take_option_actions(&mut self) -> Vec<super::options::Action> {
        self.options.take_actions()
    }

    /// Options the HUD reads live (docs/gui.md "Options window"): `Targetstarget`, the target bars' criteria (`cc_section1 && cc_friendly_health_bar` /
    /// `cc_hostile_health_bar`) and `NumHotbars` (bars are created / closed to match, 1..=10).
    fn sync_options(&mut self, gui: &mut Gui) {
        let d = &self.dvalues;
        self.target.targets_target = d.flag("Targetstarget");
        let cc = d.flag("cc_section1");
        self.target.bars_enabled = [cc && d.flag("cc_friendly_health_bar"), cc && d.flag("cc_hostile_health_bar")];
        let want = hud_bar::default_count(d);
        while self.shortcuts.len() > want {
            if let Some(s) = self.shortcuts.pop() {
                s.close(gui);
            }
        }
        while self.shortcuts.len() < want {
            let n = self.shortcuts.len();
            match ShortcutBar::new(gui, self.options.dir(), n, self.size) {
                Ok(b) => self.shortcuts.push(b),
                Err(e) => {
                    eprintln!("hud: shortcut bar {n}: {e:#}");
                    break;
                }
            }
        }
    }

    /// Re-evaluates criteria and refreshes every value-driven widget.
    fn refresh(&mut self, gui: &mut Gui, zone: &Zone) {
        let res = self.res(zone);
        gui.apply_criteria(self.cc, &res);
        for b in &self.bars {
            let show = ao_gui::expr::truthy(b.spec.criteria, &res);
            // the alien bar exists only for characters with expansion bits 0x18 (GUI 0x1006afed: `GetSkill(0x185) & 0x18`)
            let show = show && (b.spec.cfg != "CCAlienXPBarConfig" || zone.skill_value(sid::EXPANSION).unwrap_or(0) & 0x18 != 0);
            gui.set_window_visible(b.window, show);
        }
        // `CompassWindow_c` criteria (`FUN_1006d433`)
        let show = ao_gui::expr::truthy("dvalue:cc_section1 && dvalue:cc_compass", &res);
        drop(res);
        if let Some(c) = &mut self.compass {
            c.set_visible(gui, show);
        }
        let active: Vec<(String, bool)> = self.dvalues.iter_i64().map(|(k, v)| (k.to_string(), v != 0)).collect();
        for (n, a) in active {
            gui.set_cc_active(self.cc, &n, a);
            if let Some(p) = &self.popup {
                gui.set_cc_active(p.window, &n, a);
            }
        }
        if let Some(popup) = &self.popup {
            for entry in &popup.nodes {
                if !entry.active_value.is_empty() {
                    let active = self.dvalues.get_i64(&entry.name).unwrap_or(0) == menu_eval(&entry.active_value, &self.res(zone));
                    gui.set_cc_active(popup.window, &entry.id, active);
                }
            }
        }
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, _dt: f32) {
        self.refresh(gui, zone);
        self.options.apply(&mut self.dvalues, zone, &mut self.outbox);
        self.options.update(gui, &self.dvalues, &resolver(&self.dvalues, zone));
        self.sync_options(gui);
        self.apply_fades(gui);
        self.stats.configure_stat(&self.dvalues);
        self.stats.update(gui, zone, _dt);
        let st = |id: u32| zone.skill_value(id).unwrap_or(0);
        let xp = hud_pools::xp(st);
        let values = [
            hud_pools::ratio(st(sid::HEALTH), st(sid::MAX_HEALTH)),
            hud_pools::ratio(st(sid::NANO), st(sid::MAX_NANO)),
            hud_pools::ratio(xp.0, xp.1),
            hud_pools::ratio(st(sid::ALIEN_XP), st(sid::ALIEN_XP_NEXT)),
        ];
        for (b, v) in self.bars.iter().zip(values) {
            gui.set_progress(b.window, "bar", v);
        }
        // `View::SetToolTip(LDBface::GetText(0x2710, key), "%d / %d")` (`FUN_100666b6` / `FUN_100667a8` / `FUN_100668aa` /
        // `FUN_100669f7`); the keys are the strings at GUI 0x101b4300 "Health", 0x101b4334 "Nano", 0x101af930 "Experience" and
        // 0x101b433c "AlienExperience" (resolved once in `new`).
        let tips = [
            (&self.bar_titles[0], st(sid::HEALTH), st(sid::MAX_HEALTH)),
            (&self.bar_titles[1], st(sid::NANO), st(sid::MAX_NANO)),
            (&self.bar_titles[2], xp.0, xp.1),
            (&self.bar_titles[3], st(sid::ALIEN_XP), st(sid::ALIEN_XP_NEXT)),
        ];
        for (b, (t, cur, max)) in self.bars.iter().zip(tips) {
            gui.set_tooltip(b.window, "bar", t, &format!("<tvoptions wordwrap=\"no\">{cur}&nbsp;/&nbsp;{max}"));
        }
        gui.set_text(self.cc, "cash", &group(st(sid::CASH)));
        gui.set_text(self.cc, "ncu", &format!("{}/{}", st(sid::NCU_USED), st(sid::NCU_MAX)));
        self.nano.update(gui, zone, _dt);
        self.ncu.update(gui, zone, _dt);
        self.outbox.extend(self.nano.take_outbox());
        self.outbox.extend(self.ncu.take_outbox());
        self.system_lines.extend(self.ncu.take_lines());
        self.mission.update(gui, _dt);
        self.outbox.extend(self.mission.take_outbox());
        if let Some(m) = self.mission.take_marker() {
            self.set_mission(Some(m));
        }
        self.map.update(gui, zone, _dt);
        self.winb.update(gui, zone, _dt);
        self.outbox.extend(self.winb.take_outbox());
        self.system_lines.extend(self.winb.take_lines());
        self.target.update(gui, zone, _dt);
        self.aggdef.update(gui, self.cc, zone.stat(ao_net::n3::outgoing::STAT_AGG_DEF as u32));
        if let Some(c) = &mut self.compass {
            c.update(gui, zone);
        }
        // the special-action list (equipment, recharge) and the hotbar slots following it (`hud_actions.rs`, `hud_bar.rs`)
        self.actions.update(zone, _dt);
        if self.popup.as_ref().is_some_and(|p| p.source.is_some() && p.action_snapshot != self.actions.list.entries()) {
            let popup = self.popup.as_ref().unwrap();
            let source = popup.source.as_ref().unwrap().clone();
            let anchor = popup.anchor;
            self.close_popup(gui);
            self.open_popup(gui, &source, anchor, zone);
        }
        // the recharge feed: relayed `CharacterActionIIR_t` 0x14 (`hud_special::SpecialList::feed_recharge`)
        let pct = zone.skill_value(SpecialList::STAT_RECHARGE_PCT).unwrap_or(0);
        for (action, duration) in std::mem::take(&mut zone.recharge_feed) {
            self.actions.list.feed_recharge(action as u32, duration, pct);
        }
        let changes = self.actions.list.take_changes();
        let locked = self.dvalues.flag("LockHotbars");
        let timers = (self.dvalues.flag("IconTimers"), self.dvalues.flag("IconTimerText"));
        for s in &mut self.shortcuts {
            s.update(gui, _dt, &self.actions.list, &changes, locked, timers);
        }
        self.actwin.update(gui, &self.actions.list, _dt, timers.0);
        self.wincfg.update(gui, &mut self.dvalues);
        self.rollup.refresh_groups(gui);
        self.save_rollup(gui);
    }

    /// The rollup column's `DockAreas/RollupArea.xml` (a page expanded / collapsed / docked, the column scrolled).
    fn save_rollup(&mut self, gui: &Gui) {
        if !gui.interacting() && self.rollup.take_dock_dirty() {
            self.wincfg.save_docks(&mut self.dvalues, &self.rollup.dock_states(gui));
        }
    }

    /// Mouse-down outside an open sub menu closes it (the original's popup menus lose focus).
    pub(super) fn input(&mut self, gui: &mut Gui, zone: &mut Zone, ev: &InputEvent, cam: &ao_render::Camera, lens: &ao_scene::Lens, mods: ao_gui::Modifiers) {
        for s in &mut self.shortcuts {
            s.input(gui, ev, &self.actions.list, mods);
            self.uses.extend(s.take_uses());
        }
        self.actwin.input(gui, ev, &self.actions.list, mods, &mut self.shortcuts);
        self.uses.extend(self.actwin.take_uses());
        // item drag and drop between the wear window and the inventory (hud_stats/item_ui.rs)
        self.stats.input(gui, zone, ev);
        self.rollup.input(gui, ev);
        self.options.input(gui, &mut self.dvalues, ev);
        // Esc closes the windows whose `esc_*` option was set when they opened (`esc_inventory`, `esc_wear`, `esc_nano`, `esc_perkwindow`, `esc_planetmap`, `esc_optionpanel`)
        if matches!(ev, InputEvent::Key { key: ao_gui::Key::Escape, pressed: true, .. }) && !gui.text_focused() {
            for k in self.options.take_esc() {
                self.close_kind(gui, k);
            }
        }
        self.outbox.extend(self.stats.take_outbox());
        // the slider's release is `N3Msg_SetAggDef` -> `SetStat(0x33)`: applied to the own stats at once, then sent
        if let Some(v) = self.aggdef.input(gui, self.cc, ev) {
            zone.stats.insert(ao_net::n3::outgoing::STAT_AGG_DEF as u32, v);
            let payload = ao_net::n3::outgoing::set_stat(zone.char_id as i32, ao_net::n3::outgoing::STAT_AGG_DEF, v);
            self.outbox.push(ao_net::n3::outgoing::n3_frame(0, zone.char_id, payload));
        }
        // target docks, world click-to-select (hud_target.rs) and the Tab target keys
        if let Some(pos) = self.target.input(gui, zone, ev) {
            match self.target.world_click(zone, cam, lens, self.size, pos, mods) {
                Some(super::hud_target::WorldClick::Select(id)) => self.click = Some(id),
                Some(super::hud_target::WorldClick::Info(id)) => self.target.info = Some(id),
                Some(super::hud_target::WorldClick::ObjectInfo(id)) => self.help_urls.push(format!("itemid://{}/{}", id.kind, id.instance)),
                None => {}
            }
        }
        if std::mem::take(&mut self.target.attack) {
            self.uses.push(SlotUse::SpecialAction(0xb));
        }
        if let InputEvent::Key { key, pressed: true, mods } = ev {
            if gui.focused_view().is_none() {
                self.target.key(zone, *key, *mods, &self.fixed);
            }
        }
        // the window / hotbar hot keys are driven by `hot_input` from the physical key stream (`Play::game_input`): any key can be bound
        if let (Some(p), InputEvent::MouseDown { x, y, button: MouseButton::Left }) = (&self.popup, ev) {
            let pos = gui.window_pos(p.window);
            let (w, h) = gui.window_size(p.window);
            let inside = *x >= pos.0 as f32 && *x < (pos.0 + w as i32) as f32 && *y >= pos.1 as f32 && *y < (pos.1 + h as i32) as f32;
            let r = p.anchor;
            let on_owner = *x >= r.l && *x <= r.r && *y >= r.t && *y <= r.b;
            if !inside && !on_owner {
                self.close_popup(gui);
            }
        }
    }

    /// Reloads the shared key tables when the `KeyBindings` archive (character prefs) or a `KEY_*` login pref changed (the saved prefs are loaded after the
    /// HUD is created; the options window's "Key bindings" page writes the archive).
    pub(super) fn sync_keys(&mut self) {
        let stale = match self.dvalues.get("KeyBindings") {
            Some(super::dvalue::Variant::Archive(t)) => *t != self.keys_src,
            _ => false,
        };
        let fixed = FixedKeys::from_prefs(&self.dvalues.prefs);
        if stale || fixed != self.fixed {
            if let Some(super::dvalue::Variant::Archive(t)) = self.dvalues.get("KeyBindings") {
                self.keys_src = t.clone();
                self.bindings = Bindings::from_archive(t);
            }
            self.fixed = fixed;
            self.keys = KeyMap::new(&self.bindings, &self.fixed);
        }
    }

    /// A physical key press: captured by the options window's "Bind Key" dialog when it is open (true: the key does nothing else), else the HUD hot keys.
    pub(super) fn key_press(&mut self, gui: &mut Gui, key: u32, mods: ao_gui::Modifiers) -> bool {
        let input = super::options::keys::key_input(key, mods);
        if self.options.capturing() {
            self.options.capture(gui, &mut self.dvalues, input);
            return true;
        }
        self.hot_input(gui, input);
        false
    }

    /// The tables every key consumer reads (`options/keys.rs`).
    pub(super) fn key_tables(&self) -> (&Bindings, &FixedKeys) {
        (&self.bindings, &self.fixed)
    }

    /// A key press (`key id | modifier bits`): every HUD provider the binding table binds to it (`hud_keys.rs`), unless a text field has the keyboard
    /// (`! TextInputMode`) or the options window is capturing a key for a binding.
    pub(super) fn hot_input(&mut self, gui: &mut Gui, input: u32) {
        self.sync_keys();
        if gui.text_focused() || self.options.capturing() {
            return;
        }
        // `KEY_TOGGLE_CONTROL_CENTER` (Shift + `|`, `! TextInputMode`): `SwitchLayoutMessage` 0x10067e7e flips `cc_rollup_panel` -- the command -> message link is
        // [INFERENCE] (names; the message table holds no readable reference to the handler)
        if input == self.fixed.get("KEY_TOGGLE_CONTROL_CENTER") {
            let on = self.dvalues.flag("cc_rollup_panel");
            self.dvalues.set("cc_rollup_panel", super::dvalue::Variant::Bool(!on));
        }
        for k in self.keys.lookup(input) {
            match k {
                HudKey::Window(kind) => self.toggle(gui, kind),
                HudKey::BarActive(n) => {
                    if let Some(s) = self.shortcuts.iter_mut().find(|s| s.is_active()) {
                        s.use_slot(n, &self.actions.list);
                        self.uses.extend(s.take_uses());
                    }
                }
                // `SHORTUCT_BAR_ROW_n` scrolls the bar's list view to row `n` (`FUN_100d85fb`)
                HudKey::BarRow(n) => {
                    if let Some(s) = self.shortcuts.iter_mut().find(|s| s.is_active()) {
                        s.set_row(gui, n);
                    }
                }
                HudKey::BarSelect(n) => {
                    if n < self.shortcuts.len() {
                        for (i, s) in self.shortcuts.iter_mut().enumerate() {
                            s.set_primary(gui, i == n);
                        }
                    }
                }
                HudKey::BarSlot(b, slot) => {
                    if let Some(s) = self.shortcuts.get_mut(b) {
                        s.use_slot(slot, &self.actions.list);
                        self.uses.extend(s.take_uses());
                    }
                }
            }
        }
    }

    /// `/macro` (`GlobalSignals+0x1a0` -> `ShortcutBarWindow_c` `FUN_100d82d4`): the created macro `{0xc789, id}` is dragged by the first shortcut bar.
    pub(super) fn begin_macro_drag(&mut self, gui: &mut Gui, id: i32, name: &str, command: &str) {
        if let Some(s) = self.shortcuts.first_mut() {
            s.begin_macro_drag(gui, id, name, command);
        }
    }

    /// `true` when the event was consumed by the HUD.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        if self.rollup.dock_event(gui, ev) {
            return true;
        }
        if let Event::FrameHelp { url, .. } = ev {
            self.help_urls.push(url.clone());
            return true;
        }
        if self.map.event(gui, ev, zone) {
            for k in self.map.take_closed() {
                self.close_kind(gui, k);
            }
            return true;
        }
        match self.rollup.event(gui, ev) {
            Some(RollupEvent::Closed(key)) => {
                if key.is_empty() { return false; } // transient view's owner handles close/abort
                if let Some(k) = WindowKind::from_dvalue(&key) {
                    self.close_kind(gui, k);
                }
                return true;
            }
            Some(RollupEvent::Handled) => return true,
            None => {}
        }
        if self.stats.stat_event(gui, ev, zone, &mut self.dvalues) {
            return true;
        }
        if self.stats.event(gui, ev, zone) {
            for k in self.stats.take_closed() {
                self.close_kind(gui, k);
            }
            self.outbox.extend(self.stats.take_outbox());
            return true;
        }
        if self.nano.event(gui, ev, zone) {
            for k in self.nano.take_closed() {
                self.close_kind(gui, k);
            }
            return true;
        }
        if self.ncu.event(gui, ev, zone) {
            for k in self.ncu.take_closed() {
                self.close_kind(gui, k);
            }
            return true;
        }
        if self.options.event(gui, ev, &mut self.dvalues) {
            if self.options.take_closed() {
                self.close_kind(gui, WindowKind::Options);
            }
            return true;
        }
        if self.mission.event(gui, ev, zone) {
            for k in self.mission.take_closed() {
                self.close_kind(gui, k);
            }
            return true;
        }
        let mut closed = vec![];
        if self.winb.event(gui, ev, zone, &mut closed) || !closed.is_empty() {
            for k in closed {
                self.close_kind(gui, k);
            }
            return true;
        }
        let Event::Clicked { window, view, .. } = ev else { return false };
        let popup = self.popup.as_ref().map(|p| p.window);
        if *window != self.cc && Some(*window) != popup {
            return false;
        }
        let mut all = HashMap::new();
        if let Some(popup) = &self.popup {
            for node in &popup.nodes { node.walk(&mut all); }
        }
        for r in &self.menu_roots {
            r.walk(&mut all);
        }
        let Some(node) = all.get(view.as_str()).copied().cloned() else { return false };
        if node.children.is_empty() && node.category.is_none() {
            if let Some((instance, _)) = node.special {
                let use_action = self.actions.list.find(instance).map_or(SlotUse::Unavailable, |e| SlotUse::SpecialAction(e.shown));
                self.uses.push(use_action);
            }
            if !node.button { self.close_popup(gui); }
            if (!node.button || !node.invoke.is_empty()) && !node.name.is_empty() && !node.actions.iter().any(|a| a.attr("event").unwrap_or("").split(',').any(|e| e.trim() == "on_invoke")) {
                if node.invoke.is_empty() || node.invoke.eq_ignore_ascii_case("toggle") {
                    self.toggle_dvalue(gui, &node.name);
                } else {
                    let value = menu_eval(&node.invoke, &self.res(zone));
                    self.set_menu_value(gui, &node.name, value);
                }
            }
            if !node.button { self.menu_actions(gui, zone, &node, "on_activate"); }
            self.menu_actions(gui, zone, &node, "on_invoke");
        } else {
            let was = self.popup.as_ref().is_some_and(|p| p.owner == node.id);
            let anchor = gui.view_rect(*window, &node.id);
            self.close_popup(gui);
            if !was {
                if let Some(anchor) = anchor {
                    self.open_popup(gui, &node, anchor, zone);
                    self.menu_actions(gui, zone, &node, "on_activate");
                }
            }
        }
        true
    }

    pub(super) fn take_menu_camera(&mut self) -> Option<u8> {
        self.menu_camera.take()
    }

    /// CameraCoordinator's preference callback (GUI 0x10064283), including key-driven mode changes.
    pub(super) fn sync_camera_mode(&mut self, mode: u8) {
        self.dvalues.set_i64("camera_mode", i64::from(mode));
        self.dvalues.set_i64("3rdPersonCamera", i64::from(mode != 0));
        if mode != 0 { self.dvalues.set_i64("PreferredCameraMode", i64::from(mode)); }
    }

    fn set_menu_value(&mut self, gui: &mut Gui, name: &str, value: i64) {
        self.dvalues.set_i64(name, value);
        gui.set_cc_active(self.cc, name, value != 0);
        if name == "camera_mode" {
            // GUI 0x10064387: zero disables third person without discarding its preferred mode.
            if let Ok(mode @ 0..=3) = u8::try_from(value) {
                self.sync_camera_mode(mode);
                self.menu_camera = Some(mode);
            }
        } else if let Some(kind) = WindowKind::from_dvalue(name) {
            if value != 0 { self.open(gui, kind); } else { self.close_kind(gui, kind); }
        } else if value == 0 && self.popup.as_ref().is_some_and(|p| p.owner == name) {
            self.close_popup(gui);
        }
    }

    /// Original ActionMenu RunChatScript values; the flow submits them through the chat command dispatcher.
    pub(super) fn take_menu_scripts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.menu_scripts)
    }

    fn menu_actions(&mut self, gui: &mut Gui, zone: &Zone, node: &MenuNode, event: &str) {
        for group in &node.actions {
            if !group.attr("event").unwrap_or("").split(',').any(|e| e.trim() == event) {
                continue;
            }
            if let Some(criteria) = group.attr("criteria") {
                if !ao_gui::expr::truthy(criteria, &self.res(zone)) {
                    continue;
                }
            }
            for action in &group.children {
                if action.name.eq_ignore_ascii_case("RunChatScript") {
                    if let Some(value) = action.attr("value") {
                        self.menu_scripts.push(value.to_string());
                    }
                } else if action.name.eq_ignore_ascii_case("CloseMenu") {
                    self.close_popup(gui);
                } else if action.name.eq_ignore_ascii_case("SetValue") {
                    if let (Some(name), Some(value)) = (action.attr("name"), action.attr("value")) {
                        let v = menu_eval(value, &self.res(zone));
                        self.set_menu_value(gui, name, v);
                    }
                }
            }
        }
    }

    fn toggle_dvalue(&mut self, gui: &mut Gui, name: &str) {
        let v = self.dvalues.get_i64(name).unwrap_or(0) == 0;
        self.dvalues.set_i64(name, v as i64);
        gui.set_cc_active(self.cc, name, v);
        if let Some(k) = WindowKind::from_dvalue(name) {
            if v {
                self.open(gui, k);
            } else {
                self.close_kind(gui, k);
            }
        }
    }

    /// `ControlMenu_c` popup (GUI 0x10071524 with a parent entry: own style-3 `Window`, flags 0xd3c); placed by `FUN_100653b6`:
    /// right of the parent button unless its centre is in the right half of the screen, bottom edge 7 px above the button bottom.
    fn open_popup(&mut self, gui: &mut Gui, node: &MenuNode, r: ao_gui::Rect, zone: &Zone) {
        let source = node.has_categories().then(|| node.clone());
        let action_snapshot = if source.is_some() { self.actions.list.entries().to_vec() } else { vec![] };
        let mut node = node.clone();
        let items = if source.is_some() { self.actions.menu_items() } else { vec![] };
        node.populate(&items, &self.res(zone));
        let mut src = String::from("<root><View view_layout=\"vertical\" h_alignment=\"left\">");
        for c in &node.children {
            src += &c.xml();
        }
        src += "</View></root>";
        let Ok(window) = gui.open_window_xml(&node.id, &src, (0, 0), WindowSize::Preferred) else { return };
        gui.apply_criteria(window, &self.res(zone));
        for entry in &node.children {
            if !entry.active_value.is_empty() {
                let active = self.dvalues.get_i64(&entry.name).unwrap_or(0) == menu_eval(&entry.active_value, &self.res(zone));
                gui.set_cc_active(window, &entry.id, active);
            }
        }
        let (w, h) = gui.window_size(window);
        let (sw, sh) = (self.size.0 as f32, self.size.1 as f32);
        let x = if (r.l + (r.r - r.l) * 0.5) >= sw * 0.5 { r.l - 5.0 - w as f32 } else { r.r + 5.0 };
        let y = r.b - h as f32 - 7.0;
        let x = x.clamp(0.0, (sw - w as f32).max(0.0));
        let y = y.clamp(0.0, (sh - h as f32).max(0.0));
        gui.set_window_pos(window, (x as i32, y as i32));
        self.popup = Some(Popup { window, owner: node.id.clone(), anchor: r, nodes: node.children, source, action_snapshot });
        gui.set_cc_active(self.cc, &node.id, true);
    }

    fn close_popup(&mut self, gui: &mut Gui) {
        if let Some(p) = self.popup.take() {
            gui.set_cc_active(self.cc, &p.owner, false);
            gui.close_window(p.window);
        }
    }

    /// Dockable views owned outside Hud (the chat FriendsView) share this
    /// controller and the character's DockAreas persistence.
    pub(super) fn register_dock(&mut self, gui: &mut Gui, key: &str, window: WindowId) -> anyhow::Result<()> {
        if self.rollup.contains(window) {
            return Ok(());
        }
        self.rollup.register_window(gui, key, window)?;
        self.rollup.restore_docks(gui, &[]);
        Ok(())
    }

    /// TradeView's empty identity is docked at runtime but never archived as a view.
    pub(super) fn register_transient_dock(&mut self, gui: &mut Gui, window: WindowId) -> anyhow::Result<()> {
        self.rollup.register_transient_window(gui, window)
    }

    pub(super) fn open(&mut self, gui: &mut Gui, kind: WindowKind) {
        let before = gui.window_ids();
        self.dvalues.set_i64(kind.dvalue(), 1);
        gui.set_cc_active(self.cc, kind.dvalue(), true);
        self.stats.open(gui, &mut self.rollup, kind);
        if HudNano::handles(kind) {
            self.nano.open(gui, &mut self.rollup);
        }
        if HudNcu::handles(kind) {
            self.ncu.open(gui);
        }
        if HudOptions::handles(kind) {
            self.options.open(gui, &self.dvalues);
        }
        if HudActionWin::handles(kind) {
            self.actwin.open(gui, &mut self.rollup);
        }
        if HudMission::handles(kind) {
            self.mission.open(gui);
        }
        self.map.open(gui, &mut self.rollup, kind);
        self.winb.open(gui, kind);
        if !self.open.contains(&kind) {
            self.open.push(kind);
            self.options.arm_esc(kind, &self.dvalues);
        }
        // `Window::LoadWndConfig` of the window the module just made (hud_wincfg.rs)
        let made: Vec<WindowId> = gui.window_ids().into_iter().filter(|id| !before.contains(id) && gui.window_tabbed(*id)).collect();
        for id in made {
            self.wincfg.attach(gui, &self.dvalues, kind, id, self.size);
            if matches!(super::hud_wincfg::store_of(kind), Some(super::hud_wincfg::Store::Dock(_))) {
                if let Err(e) = self.rollup.register_window(gui, kind.dvalue(), id) {
                    eprintln!("hud: dock {}: {e:#}", kind.dvalue());
                }
            }
        }
        self.rollup.restore_docks(gui, &[]);
        if kind == WindowKind::Inventory {
            self.stats.reflow_inventory(gui);
        }
    }

    pub(super) fn close_kind(&mut self, gui: &mut Gui, kind: WindowKind) {
        self.dvalues.set_i64(kind.dvalue(), 0);
        self.wincfg.detach(gui, &mut self.dvalues, kind);
        gui.set_cc_active(self.cc, kind.dvalue(), false);
        self.stats.close(gui, &mut self.rollup, kind);
        if HudNano::handles(kind) {
            self.nano.close(gui, &mut self.rollup);
        }
        if HudNcu::handles(kind) {
            self.ncu.close(gui);
        }
        if HudOptions::handles(kind) {
            self.options.close(gui, &mut self.dvalues);
        }
        if HudActionWin::handles(kind) {
            self.actwin.close(gui, &mut self.rollup);
        }
        if HudMission::handles(kind) {
            self.mission.close(gui);
        }
        self.map.close(gui, &mut self.rollup, kind);
        self.winb.close(gui, kind);
        self.open.retain(|k| *k != kind);
        self.options.disarm_esc(kind);
    }

    pub(super) fn toggle(&mut self, gui: &mut Gui, kind: WindowKind) {
        self.toggle_dvalue(gui, kind.dvalue());
    }

    /// The Map window's ground image from the world loader (`hud_map::ground_map`).
    pub(super) fn provide_ground(&mut self, playfield: u32, map: ao_formats::topdown::GroundMap, cell_size: Option<f32>) {
        self.map.provide_ground(playfield, map, cell_size);
    }

    /// The `/waypoint` marker (`GlobalSignals+0x158`): the Planet Map's mission button (`hud_map::HudMap::set_mission`) and the compass marker
    /// (`FUN_100670f8`) read the same value.
    pub(super) fn set_mission(&mut self, marker: Option<(u32, [f32; 2])>) {
        self.map.set_mission(marker);
        if let Some(c) = &mut self.compass {
            c.set_waypoint(marker.map(|(playfield, [x, z])| super::hud_compass::Waypoint { playfield, pos: [x, 0.0, z] }));
        }
    }

    #[cfg(test)]
    pub(super) fn stats_window(&self, kind: WindowKind) -> Option<WindowId> {
        self.stats.window(kind)
    }

    #[cfg(test)]
    pub(super) fn is_open(&self, kind: WindowKind) -> bool {
        self.open.contains(&kind)
    }

    /// A dvalue of the control centre menus as a flag (`friends_window`, `lft_window`: windows the chat module owns).
    pub(super) fn dvalue(&self, name: &str) -> bool {
        self.dvalues.flag(name)
    }

    /// Screen rect of the AGG/DEF slider (live harness `agg` step).
    #[cfg(test)]
    pub(super) fn aggdef_rect(&self, gui: &Gui) -> Option<ao_gui::Rect> {
        gui.view_rect(self.cc, hud_aggdef::VIEW)
    }

    pub(super) fn set_dvalue(&mut self, gui: &mut Gui, name: &str, on: bool) {
        self.dvalues.set_i64(name, on as i64);
        gui.set_cc_active(self.cc, name, on);
        if let Some(k) = WindowKind::from_dvalue(name) {
            if !on {
                self.open.retain(|o| *o != k);
            }
        }
    }

    /// `LoadUserConfig` ran (the HUD is built before the character's prefs are read): the windows take their saved places (`Window::LoadWndConfig`), the
    /// rollup column its saved order / page state / scroll (`DockAreas/RollupArea.xml`), and a returning character gets the windows its dvalues say are open
    /// (`wear_window` ... = 1) opened and the others closed; a first login keeps the NewChar template's set.
    pub(super) fn prefs_loaded(&mut self, gui: &mut Gui) {
        let Some(chr) = self.dvalues.char_dir().map(Path::to_path_buf) else { return };
        if let Ok(src) = std::fs::read_to_string(chr.join("DockAreas/RollupArea.xml")) {
            self.rollup.load_user(gui, &src);
        }
        if chr.join("Prefs.xml").exists() {
            for k in WindowKind::ALL.into_iter().filter(|k| !matches!(k, WindowKind::Target | WindowKind::Friends | WindowKind::Options)) {
                match (self.dvalues.flag(k.dvalue()), self.open.contains(&k)) {
                    (true, false) => self.open(gui, k),
                    (false, true) => self.close_kind(gui, k),
                    _ => {}
                }
            }
        }
        self.wincfg.reload(gui, &self.dvalues, self.size);
        self.rollup.restore_docks(gui, &self.wincfg.load_docks(&self.dvalues));
        self.stats.reflow_inventory(gui);
    }

    /// Closes the HUD windows (leaving the world).
    pub(super) fn close(mut self, gui: &mut Gui) {
        self.wincfg.detach_all(gui, &mut self.dvalues);
        self.wincfg.save_docks(&mut self.dvalues, &self.rollup.dock_states(gui));
        if let Some(p) = &self.popup {
            gui.close_window(p.window);
        }
        for b in &self.bars {
            gui.close_window(b.window);
        }
        self.stats.close_all(gui, &mut self.rollup);
        self.rollup.close_all(gui);
        self.nano.close(gui, &mut self.rollup);
        self.ncu.close(gui);
        self.options.close(gui, &mut self.dvalues);
        // the windows' final states (`SaveWndConfig` of the dtors) reach the character's prefs files
        if !self.dvalues.take_changed().is_empty() {
            self.dvalues.save_user();
        }
        self.actwin.close(gui, &mut self.rollup);
        self.mission.close(gui);
        self.map.close_all(gui, &mut self.rollup);
        self.winb.close_all(gui);
        for s in self.shortcuts {
            s.close(gui);
        }
        if let Some(c) = self.compass {
            c.close(gui);
        self.target.close(gui);
        }
        gui.close_window(self.cc);
    }
}

/// `ExpressionParser_c` name resolver of the criteria / enable expressions: `dvalue:` reads the DValue store, `stat:` / `s:` (the short form, CommandMenu.xml
/// `s:npcnumpets!=0`) an own stat by name.
fn resolver<'a>(d: &'a DValues, zone: &'a Zone) -> impl Fn(&str, &str) -> Option<i64> + 'a {
    move |kind, name| match kind {
        "dvalue" => d.get_i64(name),
        "stat" | "s" => stats::id_of(name).and_then(|id| zone.stat(id)).map(i64::from),
        _ => None,
    }
}

/// `String::FormatNumeric` (GUI 0x1006e8f2): decimal with thousands separators. [INFERENCE] ',' (the client's locale separator).
pub(super) fn group(n: i32) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        out.insert(0, '-');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_formats::screens::TextDb;
    use ao_gui::{DrawList, InputEvent};
    use ao_render::{Frontend, Host, Offscreen};

    #[test]
    fn numeric_grouping() {
        assert_eq!(group(0), "0");
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(-1000), "-1,000");
    }

    #[test]
    fn repeated_external_dock_registration_preserves_live_frame() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut hud = Hud::new(&mut gui, &dir, (1280, 800)).unwrap();
        let window = gui.open_tabbed_window_xml("external", "External", "<root><View/></root>", (100, 100), WindowSize::Fixed(185, 127)).unwrap();
        hud.register_dock(&mut gui, "external_view", window).unwrap();
        gui.set_window_pos(window, (320, 240));
        gui.resize_window(window, WindowSize::Fixed(260, 210));
        gui.set_window_pinned(window, true);
        let frame = gui.window_outer_frame(window);
        for _ in 0..3 {
            hud.register_dock(&mut gui, "external_view", window).unwrap();
            assert_eq!(gui.window_outer_frame(window), frame);
            assert!(gui.window_pinned(window));
        }
    }

    #[test]
    fn ncu_modifier_updates_real_hud_and_window_title() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut hud = Hud::new(&mut gui, &dir, (1280, 800)).unwrap();
        let mut zone = Zone::default();
        zone.stats.extend([(sid::NCU_USED, 3), (sid::NCU_MAX, 8)]);
        hud.open(&mut gui, WindowKind::Ncu);
        hud.update(&mut gui, &mut zone, 0.0);
        assert_eq!(gui.text(hud.cc, "ncu"), "3/8");
        zone.apply_effects(&[ao_net::n3::spells::spell(0xcf35, &[(0, sid::NCU_MAX as i32), (0x27, 12)])], true);
        hud.update(&mut gui, &mut zone, 0.0);
        assert_eq!(zone.stat(sid::NCU_MAX), Some(8), "stored stat remains raw");
        assert_eq!(gui.text(hud.cc, "ncu"), "3/20");
        assert_eq!(hud.ncu.title(), HudNcu::title_for(&TextDb::load(&dir).unwrap(), 3, 20));
    }

    #[test]
    fn action_categories_override_names_and_count_visible_entries() {
        let root = xml::parse(r##"<Menu special_action_category="AM_CATEGORY_NORMAL" criteria="active_item_count&gt;0"><ItemTemplate action_name="invite person to your team" label="#Invite"/><ItemTemplate action_name="suspended animation" label="#Quit" tooltip="#ExitTheGame"/><ItemTemplate action_name="default" criteria="stat:level&gt;=5"/></Menu>"##).unwrap();
        let mut node = MenuNode::load(&root, Path::new("."), 0);
        node.identify("normal");
        let items = vec![
            (1, 2, "Invite Person To Your Team".into(), 7),
            (2, 2, "Suspended Animation".into(), 0x51),
            (3, 3, "Attack".into(), 0xb),
            (4, 2, "Search".into(), 0x86),
        ];
        node.populate(&items, &|_, _| Some(1));
        assert_eq!(node.children.len(), 3);
        assert_eq!(node.children[0].label, "#Invite");
        assert_eq!(node.children[1].label, "#Quit");
        assert_eq!(node.children[1].tooltip, "#ExitTheGame");
        assert_eq!(node.children[2].label, "Search");
        assert!(node.children.iter().all(|c| c.button));
        assert_eq!(node.criteria, "2>0");
        assert_eq!(node.children[1].special, Some((2, 0x51)));
        let mut empty = MenuNode::load(&root, Path::new("."), 0);
        empty.populate(&[], &|_, _| None);
        assert_eq!(empty.criteria, "0>0");
        assert!(empty.xml().contains("submenu=\"1\""));
        let camera = xml::parse(r#"<Menu><MenuEntry name="camera_mode" invoke="CAMERA_1ST" active_value="CAMERA_1ST"/><MenuEntry name="camera_mode" invoke="CAMERA_3RD_LOCK" active_value="CAMERA_3RD_LOCK"/></Menu>"#).unwrap();
        let mut camera = MenuNode::load(&camera, Path::new("."), 0);
        camera.identify("camera");
        assert_ne!(camera.children[0].id, camera.children[1].id);
        assert_eq!(menu_eval(&camera.children[1].invoke, &|_, _| None), 3);
    }

    #[test]
    fn unnamed_menu_identity_and_actions() {
        let root = xml::parse(r#"<Menu><Menu label="Help"><MenuEntry name="optionpanel_window"/></Menu><MenuEntry button_mode="button"><Actions event="on_invoke,on_begin_drag"><RunChatScript value="/pet follow"/></Actions></MenuEntry></Menu>"#).unwrap();
        let mut node = MenuNode::load(&root, Path::new("."), 0);
        node.identify("0");
        let mut all = HashMap::new();
        node.walk(&mut all);
        assert_eq!(all["__menu_0_0"].children[0].name, "optionpanel_window");
        assert!(all["__menu_0_0"].xml().contains("name=\"__menu_0_0\""));
        assert_eq!(all["__menu_0_1"].actions[0].children[0].attr("value"), Some("/pet follow"));
        assert!(all["__menu_0_1"].name.is_empty());
    }

    #[test]
    fn settings_nested_popup_and_pet_actions() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut hud = Hud::new(&mut gui, &dir, (1280, 800)).unwrap();
        let zone = Zone::default();
        let click = |hud: &mut Hud, gui: &mut Gui, window, view: &str| {
            assert!(hud.event(gui, &Event::Clicked { window, view: view.into(), item: None }, &zone));
        };
        let cc = hud.cc;
        click(&mut hud, &mut gui, cc, "command_menu");
        let command = hud.popup.as_ref().unwrap().window;
        let help = {
            let mut all = HashMap::new();
            for root in &hud.menu_roots { root.walk(&mut all); }
            all.values().find(|n| n.label == "#Help/se").unwrap().id.clone()
        };
        let anchor = gui.view_rect(command, &help).unwrap();
        click(&mut hud, &mut gui, command, &help);
        let windows = hud.popup.as_ref().unwrap();
        assert_eq!(windows.anchor, anchor);
        let window = windows.window;
        click(&mut hud, &mut gui, window, "optionpanel_window");
        assert!(hud.open.contains(&WindowKind::Options));
        assert_eq!(hud.dvalues.get_i64("optionpanel_window"), Some(1));
        hud.close_kind(&mut gui, WindowKind::Options);
        for (constant, mode) in [("CAMERA_1ST", 0), ("CAMERA_3RD_FREE", 1), ("CAMERA_3RD_RUBBER", 2), ("CAMERA_3RD_LOCK", 3)] {
            click(&mut hud, &mut gui, cc, "command_menu");
            let command = hud.popup.as_ref().unwrap().window;
            click(&mut hud, &mut gui, command, "camera_menu");
            let camera = hud.popup.as_ref().unwrap();
            let entry = camera.nodes.iter().find(|n| n.invoke == constant).unwrap().id.clone();
            let window = camera.window;
            click(&mut hud, &mut gui, window, &entry);
            assert_eq!(hud.take_menu_camera(), Some(mode));
            assert_eq!(hud.dvalues.get_i64("camera_mode"), Some(i64::from(mode)));
            assert_eq!(hud.dvalues.get_i64("3rdPersonCamera"), Some(i64::from(mode != 0)));
        }
        click(&mut hud, &mut gui, cc, "command_menu");
        let command = hud.popup.as_ref().unwrap();
        let category = command.nodes.iter().find(|n| n.category == Some(2)).unwrap().id.clone();
        let window = command.window;
        click(&mut hud, &mut gui, window, &category);
        let normal = hud.popup.as_ref().unwrap();
        let entry = &normal.nodes[0];
        let shown = entry.special.unwrap().1;
        let id = entry.id.clone();
        let window = normal.window;
        click(&mut hud, &mut gui, window, &id);
        assert_eq!(hud.take_uses(), [SlotUse::SpecialAction(shown)]);
        let pets = xml::parse(&std::fs::read_to_string(dir.join("cd_image/gui/Default/ActionMenu/PetMenu.xml")).unwrap()).unwrap();
        let pets = MenuNode::load(&pets, Path::new("."), 0);
        for node in &pets.children { hud.menu_actions(&mut gui, &zone, node, "on_invoke"); }
        assert_eq!(hud.take_menu_scripts(), ["/pet follow", "/pet behind", "/pet wait", "/pet guard", "/pet attack", "/pet terminate", "/pet report", "/pet heal"]);
        let toggle = xml::parse(r#"<MenuEntry button_mode="button"><Actions event="on_invoke"><CloseMenu/><SetValue name="test_menu_value" value="dvalue:test_menu_value==false"/></Actions></MenuEntry>"#).unwrap();
        let toggle = MenuNode::load(&toggle, Path::new("."), 0);
        hud.menu_actions(&mut gui, &zone, &toggle, "on_activate");
        assert_eq!(hud.dvalues.get_i64("test_menu_value"), None);
        hud.menu_actions(&mut gui, &zone, &toggle, "on_invoke");
        assert_eq!(hud.dvalues.get_i64("test_menu_value"), Some(1));
        hud.menu_actions(&mut gui, &zone, &toggle, "on_invoke");
        assert_eq!(hud.dvalues.get_i64("test_menu_value"), Some(0));
        hud.close(&mut gui);
    }

    /// A `Frontend` that is just the HUD over an empty world (the headless screenshot harness).
    struct Shot {
        gui: Gui,
        hud: Hud,
        zone: Zone,
    }

    impl Frontend for Shot {
        fn gui(&self) -> &Gui {
            &self.gui
        }
        fn input(&mut self, ev: InputEvent, host: &mut Host) {
            self.hud.input(&mut self.gui, &mut self.zone, &ev, &host.camera, &host.lens.unwrap_or_default(), host.mods);
            for e in self.gui.input(ev) {
                self.hud.event(&mut self.gui, &e, &self.zone);
            }
        }
        fn game_input(&mut self, ev: ao_render::GameInput, host: &mut Host) {
            if let ao_render::GameInput::Key { code, pressed: true, repeat: false } = ev {
                if let Some(key) = super::super::controls::key_id(code) {
                    self.hud.key_press(&mut self.gui, key, host.mods);
                }
            }
        }
        fn frame(&mut self, dt: f32, size: (u32, u32), _host: &mut Host) -> DrawList {
            self.hud.resize(&mut self.gui, size);
            self.hud.update(&mut self.gui, &mut self.zone, dt);
            self.gui.frame(dt)
        }
    }

    fn shot(size: (u32, u32)) -> Option<(Shot, Offscreen)> {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return None;
        }
        let labels = TextDb::load(&dir).unwrap();
        let mut gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let mut zone = Zone::default();
        // Captured level-one Solitus Soldier inputs: the retail pool formula derives 34 Life / 32 Nano from six abilities and five raw skill points.
        zone.stats.extend(ao_formats::stats::skills::ABILITIES.into_iter().map(|id| (id, 6)));
        zone.stats.extend(ao_formats::stats::SKILL_GROUPS.iter().skip(1).flat_map(|g| g.stats).map(|&id| (id as u32, 5)));
        for (id, v) in [(4, 1), (37, 1), (60, 1), (1, 1), (27, 34), (221, 1), (214, 32), (54, 1), (52, 40), (57, 0), (350, 1450), (61, 1234), (180, 0), (181, 150)] {
            zone.stats.insert(id, v);
        }
        let hud = Hud::new(&mut gui, &dir, size).unwrap();
        let shot = Shot { gui, hud, zone };
        let off = Offscreen::new(&shot, size).unwrap();
        Some((shot, off))
    }

    fn png(s: &mut Shot, o: &mut Offscreen, name: &str) {
        let mut list = DrawList::default();
        for _ in 0..3 {
            list = o.frame(s, 0.016);
        }
        if let Some(dir) = std::env::var_os("AOMAC_SHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            o.png(s, &list, &std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    }

    /// The bar tooltip titles are `LDBface::GetText(0x2710, key)` of the keys in `FUN_100666b6`…`FUN_100669f7`, `Targetstarget` is read from `LoginPrefs.xml`.
    #[test]
    fn bar_titles_come_from_the_text_db() {
        let Some((s, _)) = shot((1280, 800)) else { return };
        assert_eq!(s.hud.bar_titles, ["Health", "Nano", "Experience", "Alien Experience"]);
        assert_eq!(s.hud.dvalues.get_i64("Targetstarget"), Some(0));
        assert!(!s.hud.target.targets_target);
    }

    /// `SlotPlayerCharacterAlive` (GUI 0x1006afed) points: health (0,0), nano (0, H+20), XP (W+3, 0), alien XP (W+3, H+20) for every screen size.
    #[test]
    fn bars_sit_at_the_code_default_points() {
        for size in [(1280, 800), (1920, 1080)] {
            let Some((s, _)) = shot(size) else { return };
            let at = |cfg: &str| {
                let b = s.hud.bars.iter().find(|b| b.spec.cfg == cfg).unwrap();
                (s.gui.window_pos(b.window), s.gui.window_size(b.window))
            };
            let (health, nano, xp, alien) = (at("CCHealthBarConfig"), at("CCNanoBarConfig"), at("CCXPBarConfig"), at("CCAlienXPBarConfig"));
            let (w, h) = health.1;
            assert_eq!(health.0, (0, 0), "{size:?}");
            assert_eq!(nano.0, (0, h as i32 + 20), "{size:?}");
            assert_eq!(xp.0, (w as i32 + 3, 0), "{size:?}");
            assert_eq!(alien.0, (w as i32 + 3, h as i32 + 20), "{size:?}");
        }
    }

    /// Ctrl+6 = Map, P = Planet Map, Shift+P = Perks, I = Inventory (help texts / CharPrefs.xml); a focused text field swallows them.
    /// The fixed keys are read from the `KEY_*` login ints: `KEY_TOGGLE_CONTROL_CENTER` flips `cc_rollup_panel` (the rollup column), the target keys cycle the target.
    #[test]
    fn fixed_keys_follow_the_login_prefs() {
        use ao_render::KeyCode as K;
        let Some((mut s, mut o)) = shot((1280, 800)) else { return };
        let shift = ao_gui::Modifiers { shift: true, ..Default::default() };
        let ctrl = ao_gui::Modifiers { ctrl: true, ..Default::default() };
        let mut press = |s: &mut Shot, code: K, mods: ao_gui::Modifiers| {
            o.host.mods = mods;
            s.game_input(ao_render::GameInput::Key { code, pressed: true, repeat: false }, &mut o.host);
            s.hud.update(&mut s.gui, &mut s.zone, 0.016);
        };
        // Shift + `|` (PIPE 0x24)
        assert!(s.gui.is_visible(s.hud.cc, "RollupArea") && s.hud.dvalues.flag("cc_rollup_panel"));
        press(&mut s, K::Backslash, shift);
        assert!(!s.hud.dvalues.flag("cc_rollup_panel") && !s.gui.is_visible(s.hud.cc, "RollupArea"));
        press(&mut s, K::Backslash, shift);
        assert!(s.gui.is_visible(s.hud.cc, "RollupArea"));
        // a `Login.cfg` override moves it to Ctrl + K
        s.hud.dvalues.prefs.set_int("KEY_TOGGLE_CONTROL_CENTER", 92 | 0x40000, super::super::dvalue::Kind::Login);
        press(&mut s, K::Backslash, shift);
        assert!(s.gui.is_visible(s.hud.cc, "RollupArea"), "the old key does nothing");
        press(&mut s, K::KeyK, ctrl);
        assert!(!s.gui.is_visible(s.hud.cc, "RollupArea"));
        // the target keys: Tab by default, Y after the override (`KEY_NEXT_HOSTILE_TARGET`)
        let mods = ao_gui::Modifiers::default();
        let mut zone = Zone::default();
        assert!(s.hud.target.key(&mut zone, ao_gui::Key::Tab, mods, &s.hud.fixed));
        assert!(!s.hud.target.key(&mut zone, ao_gui::Key::Letter('y'), mods, &s.hud.fixed));
        s.hud.dvalues.prefs.set_int("KEY_NEXT_HOSTILE_TARGET", 106, super::super::dvalue::Kind::Login);
        s.hud.sync_keys();
        assert!(!s.hud.target.key(&mut zone, ao_gui::Key::Tab, mods, &s.hud.fixed));
        assert!(s.hud.target.key(&mut zone, ao_gui::Key::Letter('y'), mods, &s.hud.fixed));
        // KEY_OPEN_PERK_WINDOW (Shift + P) follows too
        s.hud.dvalues.prefs.set_int("KEY_OPEN_PERK_WINDOW", 106, super::super::dvalue::Kind::Login);
        press(&mut s, K::KeyP, shift);
        assert!(!s.hud.is_open(WindowKind::Perks));
        press(&mut s, K::KeyY, mods);
        assert!(s.hud.is_open(WindowKind::Perks));
    }

    #[test]
    fn window_hotkeys_toggle_and_respect_text_input() {
        let Some((mut s, mut o)) = shot((1280, 800)) else { return };
        use ao_render::KeyCode as K;
        // the physical key stream (`Play::game_input`): any key can be bound, so the hot keys read key ids, not characters
        let code = |c: char| match c {
            '6' => K::Digit6,
            '9' => K::Digit9,
            'p' => K::KeyP,
            _ => K::KeyI,
        };
        let ctrl = ao_gui::Modifiers { ctrl: true, ..Default::default() };
        let none = ao_gui::Modifiers::default();
        fn press(s: &mut Shot, o: &mut Offscreen, code: ao_render::KeyCode, mods: ao_gui::Modifiers) {
            o.host.mods = mods;
            s.game_input(ao_render::GameInput::Key { code, pressed: true, repeat: false }, &mut o.host);
        }
        press(&mut s, &mut o, code('6'), ctrl);
        assert!(s.hud.is_open(WindowKind::Map));
        press(&mut s, &mut o, code('6'), ctrl);
        assert!(!s.hud.is_open(WindowKind::Map));
        // Ctrl+9 = `WINDOW_STAT`: the stat window is open from the start (NewChar template), so the first press closes it
        assert!(s.hud.is_open(WindowKind::Stat));
        press(&mut s, &mut o, code('9'), ctrl);
        assert!(!s.hud.is_open(WindowKind::Stat));
        press(&mut s, &mut o, code('9'), ctrl);
        assert!(s.hud.is_open(WindowKind::Stat));
        press(&mut s, &mut o, code('p'), none);
        assert!(s.hud.is_open(WindowKind::PlanetMap) && !s.hud.is_open(WindowKind::Perks));
        press(&mut s, &mut o, code('p'), ao_gui::Modifiers { shift: true, ..none });
        assert!(s.hud.is_open(WindowKind::Perks));
        press(&mut s, &mut o, code('i'), none);
        assert!(s.hud.is_open(WindowKind::Inventory));
        // typing a name into a text field must not open windows
        let w = s.gui.open_window("LoginWindow", (0, 0), WindowSize::Preferred).unwrap();
        s.gui.focus(w, "username");
        assert!(s.gui.text_focused());
        press(&mut s, &mut o, code('p'), none);
        press(&mut s, &mut o, code('6'), ctrl);
        assert!(!s.hud.is_open(WindowKind::Map) && s.hud.is_open(WindowKind::PlanetMap));
        s.gui.clear_focus();
        press(&mut s, &mut o, code('p'), none);
        assert!(!s.hud.is_open(WindowKind::PlanetMap));
    }

    #[test]
    fn default_layout_shots() {
        for (size, tag) in [((1280, 800), "1280"), ((1920, 1080), "1920")] {
            let Some((mut s, mut o)) = shot(size) else { return };
            // every dock the XML names is filled and the menus carry their entries
            for dock in ["LeftWingDock", "LeftBarDock", "RightWingDock", "RightBarDock"] {
                assert!(s.gui.has_view(s.hud.cc, dock), "{dock}");
            }
            assert!(s.gui.has_view(s.hud.cc, "inventory_window") && s.gui.has_view(s.hud.cc, "friends_window"));
            png(&mut s, &mut o, &format!("hud-{tag}"));
            assert_eq!((s.zone.stat(1), s.zone.stat(221)), (Some(34), Some(32)));
            assert_eq!((s.zone.skill_value(27), s.zone.skill_value(214)), (Some(34), Some(32)), "synthetic screenshot pools must be full and internally valid");
        }
    }

    /// The control-centre fade groups (`CCFadeLow` 0.33 / `CCFadeHigh` 0.85 / `CCFadeDelay` 2 s of LoginPrefs.xml, docs/gui.md §10.8): every group
    /// starts dim, the wing / menus of the hovered side brighten, dim again 2 s + 1 s after the pointer left, and the dvalues apply live.
    /// `fade-dim.png` / `fade-hover-left.png` / `fade-live.png` in `AOMAC_SHOT_DIR`.
    #[test]
    fn fade_groups_follow_the_pointer_and_the_dvalues() {
        use super::super::dvalue::Variant;
        let size = (1280, 800);
        let Some((mut s, mut o)) = shot(size) else { return };
        let run = |s: &mut Shot, o: &mut Offscreen, secs: f32| {
            for _ in 0..(secs / 0.05).round() as u32 {
                s.frame(0.05, size, &mut o.host);
            }
        };
        let a = |s: &Shot, g: &str| s.gui.fade_alpha(g).unwrap();
        run(&mut s, &mut o, 0.1);
        assert_eq!((a(&s, "cc_left_fade_group"), a(&s, "cc_right_fade_group")), (0.33, 0.33));
        assert_eq!(s.gui.view_alpha(s.hud.cc, "LeftWingDock"), Some(0.33));
        png(&mut s, &mut o, "fade-dim");
        let r = s.gui.view_rect(s.hud.cc, "LeftWingDock").expect("left wing");
        send(&mut s, InputEvent::MouseMove { x: (r.l + r.r) / 2.0, y: (r.t + r.b) / 2.0 });
        run(&mut s, &mut o, 0.3);
        assert_eq!((a(&s, "cc_left_fade_group"), a(&s, "cc_right_fade_group")), (0.85, 0.33));
        assert_eq!(s.gui.view_alpha(s.hud.cc, "LeftWingDock"), Some(0.85));
        png(&mut s, &mut o, "fade-hover-left");
        // the pointer leaves: nothing changes for CCFadeDelay, then the group falls to CCFadeLow in 1 s
        send(&mut s, InputEvent::MouseMove { x: 640.0, y: 300.0 });
        run(&mut s, &mut o, 1.9);
        assert_eq!(a(&s, "cc_left_fade_group"), 0.85);
        run(&mut s, &mut o, 1.3);
        assert_eq!(a(&s, "cc_left_fade_group"), 0.33);
        // live: CCFadeLow / CCFadeHigh changes snap every group, the hot (here none) / other groups fall again after 2 s
        assert!(s.hud.dvalues.set("CCFadeLow", Variant::Float(0.6)));
        run(&mut s, &mut o, 0.1);
        assert_eq!((a(&s, "cc_left_fade_group"), s.gui.view_alpha(s.hud.cc, "RightBarDock")), (0.6, Some(0.6)));
        png(&mut s, &mut o, "fade-live");
        // `cc_rollup_controller_fade_level`: a black surface behind the rollup column, default 0 = nothing
        let area = s.gui.view_rect(s.hud.cc, "RollupArea").expect("rollup area");
        let black = |l: &DrawList| l.cmds.iter().any(|c| matches!(c, ao_gui::DrawCmd::Solid { dst, color: [0, 0, 0], alpha } if *alpha == 0.5 && dst[0] == area.l && dst[1] == area.t));
        assert!(!black(&s.frame(0.016, size, &mut o.host)));
        assert!(s.hud.dvalues.set("cc_rollup_controller_fade_level", Variant::Float(0.5)));
        assert!(black(&s.frame(0.016, size, &mut o.host)));
    }

    fn own(s: &mut Shot, yaw: f32, pos: [f32; 3]) {
        use crate::play::zone::DynelState;
        s.zone.char_id = 25988;
        s.zone.dynels.insert(25988, DynelState { name: "Testy".into(), pos, yaw: Some(yaw), npc: false, side: 0, level: 1, health: 34, max_health: 34 });
    }

    /// One input event through the HUD and the GUI, like `Shot::input`.
    fn send(s: &mut Shot, ev: InputEvent) {
        let cam = ao_render::Camera::look_at(glam::Vec3::ZERO, -glam::Vec3::Z);
        s.hud.input(&mut s.gui, &mut s.zone, &ev, &cam, &ao_scene::Lens::default(), Default::default());
        for e in s.gui.input(ev) {
            s.hud.event(&mut s.gui, &e, &s.zone);
        }
    }

    /// The compass window at four headings (north / east / south / west) and with a waypoint straight ahead, 90 degrees right and
    /// behind: 4x crops of the HUD shots (`compass-*.png` in `AOMAC_SHOT_DIR`).
    #[test]
    fn compass_shots() {
        use super::super::hud_compass::Waypoint;
        let Some((mut s, mut o)) = shot((1280, 800)) else { return };
        let win = s.hud.compass.as_ref().expect("compass").window;
        let (x, y) = s.gui.window_pos(win);
        assert_eq!((x, y), ((1280.0f32 * 0.73 - 68.0).floor() as i32, 5));
        let crop = |s: &mut Shot, o: &mut Offscreen, name: &str| {
            png(s, o, name);
            if let Some(dir) = std::env::var_os("AOMAC_SHOT_DIR") {
                let p = std::path::Path::new(&dir).join(format!("{name}.png"));
                let img = image::open(&p).unwrap().to_rgba8();
                let sub = image::imageops::crop_imm(&img, x as u32 - 4, 0, 145, 36).to_image();
                let big = image::imageops::resize(&sub, 145 * 4, 36 * 4, image::imageops::FilterType::Nearest);
                big.save(p.with_file_name(format!("{}.png", name.replace("hud-", "")))).unwrap();
                std::fs::remove_file(&p).unwrap();
            }
        };
        for (yaw, tag) in [(0.0, "north"), (std::f32::consts::FRAC_PI_2, "east"), (std::f32::consts::PI, "south"), (-std::f32::consts::FRAC_PI_2, "west")] {
            own(&mut s, yaw, [100.0, 0.0, 100.0]);
            crop(&mut s, &mut o, &format!("hud-compass-{tag}"));
        }
        // waypoints of the current playfield: ahead (north), to the east (right), behind (south)
        own(&mut s, 0.0, [100.0, 0.0, 100.0]);
        s.zone.playfield = Some(567);
        for (wp, tag) in [([100.0, 0.0, 200.0], "wp-ahead"), ([200.0, 0.0, 100.0], "wp-right"), ([100.0, 0.0, 0.0], "wp-behind")] {
            s.hud.compass.as_mut().unwrap().set_waypoint(Some(Waypoint { playfield: 567, pos: wp }));
            crop(&mut s, &mut o, &format!("hud-compass-{tag}"));
        }
        // another playfield's waypoint is not shown
        s.hud.compass.as_mut().unwrap().set_waypoint(Some(Waypoint { playfield: 1, pos: [100.0, 0.0, 200.0] }));
        crop(&mut s, &mut o, "hud-compass-wp-other-playfield");
    }

    /// Dragging the AGG/DEF knob sends `SetStatIIR_t(0x33)` on release only, floors the value while dragging and clamps to -100..100.
    #[test]
    fn aggdef_drag_sends_set_stat_on_release() {
        let Some((mut s, mut o)) = shot((1280, 800)) else { return };
        own(&mut s, 0.0, [0.0; 3]);
        s.zone.stats.insert(0x33, 0);
        png(&mut s, &mut o, "hud-aggdef-0");
        let r = s.gui.view_rect(s.hud.cc, "aggdef").expect("slider view");
        let knob = r.l + 58.0 + 5.0;
        send(&mut s, InputEvent::MouseDown { x: knob, y: r.t + 5.0, button: MouseButton::Left });
        send(&mut s, InputEvent::MouseMove { x: knob + 30.0, y: r.t + 5.0 });
        assert!(s.hud.take_outbox().is_empty(), "nothing is sent while dragging");
        send(&mut s, InputEvent::MouseUp { x: knob + 30.0, y: r.t + 5.0, button: MouseButton::Left });
        let out = s.hud.take_outbox();
        assert_eq!(out.len(), 1);
        // 58 + 30 px of the 117 px travel = 50.4 -> floor 50
        assert_eq!(out[0].payload, ao_net::n3::outgoing::set_stat(25988, 0x33, 50));
        assert_eq!(s.zone.stat(0x33), Some(50));
        png(&mut s, &mut o, "hud-aggdef-50");
        // a press beside the knob grabs nothing; a far-right drag clamps to 100
        send(&mut s, InputEvent::MouseDown { x: r.l + 2.0, y: r.t + 5.0, button: MouseButton::Left });
        send(&mut s, InputEvent::MouseUp { x: r.l + 2.0, y: r.t + 5.0, button: MouseButton::Left });
        assert!(s.hud.take_outbox().is_empty());
        let knob = r.l + (50.0f32 * 117.0 / 200.0 + 58.5).floor() + 5.0;
        send(&mut s, InputEvent::MouseDown { x: knob, y: r.t + 5.0, button: MouseButton::Left });
        send(&mut s, InputEvent::MouseMove { x: knob + 500.0, y: r.t + 5.0 });
        send(&mut s, InputEvent::MouseUp { x: knob + 500.0, y: r.t + 5.0, button: MouseButton::Left });
        assert_eq!(s.hud.take_outbox()[0].payload, ao_net::n3::outgoing::set_stat(25988, 0x33, 100));
    }

    /// The Actions window (Ctrl+2): the 9 base entries in list order with their rdb names, a click is the use, a held press drags the action onto a hotbar
    /// slot, the recharge feed (`CharacterActionIIR_t` 0x14) makes the overlay, the hotbar rows page with the up / down buttons and the row keys.
    #[test]
    fn actions_window_uses_drags_and_recharges() {
        use super::super::hud_special::template_of;
        let Some((mut s, mut o)) = shot((1280, 800)) else { return };
        // the template's wear / stat pages first sit above it in the rollup column; the Actions page is moved to the top by closing them
        for k in [WindowKind::Character, WindowKind::Stat, WindowKind::Team] {
            s.hud.close_kind(&mut s.gui, k);
        }
        s.hud.open(&mut s.gui, WindowKind::Actions);
        png(&mut s, &mut o, "hud-actions");
        let want: Vec<i32> = [3, 1, 0xb, 0x4c, 0x11, 0x13, 0x51, 0x14, 0x86].iter().map(|&a| template_of(a).unwrap() as i32).collect();
        assert_eq!(s.hud.actwin.shown(), want);
        let centre = |s: &Shot, i: usize| s.hud.actwin.cell_centre(&s.gui, i).expect("grid");
        // a short press is the use on the release; Start Combat is the third icon
        let (x, y) = centre(&s, 2);
        send(&mut s, InputEvent::MouseDown { x, y, button: MouseButton::Left });
        assert!(s.hud.take_uses().is_empty());
        send(&mut s, InputEvent::MouseUp { x, y, button: MouseButton::Left });
        assert_eq!(s.hud.take_uses(), vec![SlotUse::SpecialAction(0xb)]);
        // Walk held for 0.3 s is dragged and dropped on hotbar slot 7
        let (x, y) = centre(&s, 4);
        send(&mut s, InputEvent::MouseDown { x, y, button: MouseButton::Left });
        s.hud.update(&mut s.gui, &mut s.zone, 0.4);
        let win = s.hud.shortcuts[0].window;
        let (wx, wy) = s.gui.window_pos(win);
        let (sx, sy) = (wx as f32 + 43.0 + 36.0 * 7.0 + 17.0, wy as f32 + 19.0);
        send(&mut s, InputEvent::MouseMove { x: sx, y: sy });
        send(&mut s, InputEvent::MouseUp { x: sx, y: sy, button: MouseButton::Left });
        assert!(s.hud.take_uses().is_empty(), "the release of a drag is no use");
        assert_eq!(s.hud.shortcuts[0].slot_names()[7], "Walk");
        // the recharge feed of action 0x86 (Search) shows its overlay and counts down
        s.zone.recharge_feed.push((0x86, 30));
        png(&mut s, &mut o, "hud-actions-recharge");
        assert!(s.hud.actions.list.progress(0x86).is_some());
        // hotbar rows: down shows the empty row 1, up returns; the row key `Shift+2` goes to row 1 again
        let (bx, by) = (wx as f32 + 11.0 + 11.0 + 4.0, wy as f32 + 30.0);
        send(&mut s, InputEvent::MouseDown { x: bx, y: by, button: MouseButton::Left });
        send(&mut s, InputEvent::MouseUp { x: bx, y: by, button: MouseButton::Left });
        assert_eq!((s.hud.shortcuts[0].row(), s.hud.shortcuts[0].slot_names()[0].as_str()), (1, ""));
        let up = (bx, wy as f32 + 5.0);
        send(&mut s, InputEvent::MouseDown { x: up.0, y: up.1, button: MouseButton::Left });
        send(&mut s, InputEvent::MouseUp { x: up.0, y: up.1, button: MouseButton::Left });
        assert_eq!((s.hud.shortcuts[0].row(), s.hud.shortcuts[0].slot_names()[0].as_str()), (0, "Start Combat"));
        s.hud.shortcuts[0].set_row(&mut s.gui, 3);
        s.hud.shortcuts[0].set_row(&mut s.gui, 0);
        assert_eq!(s.hud.shortcuts[0].slot_names()[7], "Walk", "rows keep their slots");
        // closing removes the page
        s.hud.close_kind(&mut s.gui, WindowKind::Actions);
        assert!(s.hud.actwin.shown().is_empty());
    }

    /// The windows the retail capture shows side by side (`ref.png`, docs/gui.md §6.4): inventory with its scrollbar, the NCU and Programs pages, the Actions page.
    /// `hud-compare-*.png` in `AOMAC_SHOT_DIR`.
    #[test]
    fn retail_comparison_shots() {
        let size = (1667, 916);
        let Some((mut s, mut o)) = shot(size) else { return };
        for k in [WindowKind::Character, WindowKind::Stat, WindowKind::Team] {
            s.hud.close_kind(&mut s.gui, k);
        }
        for k in [WindowKind::Character, WindowKind::Ncu, WindowKind::Nano, WindowKind::Inventory, WindowKind::Skills] {
            s.hud.open(&mut s.gui, k);
        }
        png(&mut s, &mut o, "hud-compare-1667");
        s.hud.close_kind(&mut s.gui, WindowKind::Nano);
        s.hud.open(&mut s.gui, WindowKind::Actions);
        png(&mut s, &mut o, "hud-compare-actions");
    }

    /// Drags the frame of `w` by its title strip (right of the tab, below the top border) by `(dx, dy)`.
    fn drag_strip(s: &mut Shot, w: WindowId, dx: f32, dy: f32) {
        let (x, y, ow, _) = s.gui.window_outer_frame(w).unwrap();
        let (px, py) = (x as f32 + ow as f32 * 0.6, y as f32 + 12.0);
        send(s, InputEvent::MouseDown { x: px, y: py, button: MouseButton::Left });
        send(s, InputEvent::MouseMove { x: px + dx, y: py + dy });
        send(s, InputEvent::MouseUp { x: px + dx, y: py + dy, button: MouseButton::Left });
    }

    /// `Window::SaveWndConfig` / `LoadWndConfig` (docs/gui.md §6.5): the inventory (a dock window: `DockAreas/DockArea0.xml`) and the skills window
    /// (`SkillConfig`) are dragged, the inventory is resized (its grid reflows) and pinned; a second login restores frames and pin, opens the windows its dvalues
    /// say are open, and the NCU window takes the template's pin state.
    #[test]
    fn dragged_windows_are_saved_and_restored() {
        let tmp = std::env::temp_dir().join(format!("aomac-hud-wincfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let size = (1280, 800);
        let Some((mut s, mut o)) = shot(size) else { return };
        s.hud.dvalues.open_user(&tmp, "acct", 7);
        s.hud.prefs_loaded(&mut s.gui);
        for k in [WindowKind::Inventory, WindowKind::Skills, WindowKind::Ncu] {
            s.hud.open(&mut s.gui, k);
        }
        s.frame(0.016, size, &mut o.host);
        let inv = s.hud.stats_window(WindowKind::Inventory).unwrap();
        let skills = s.hud.stats_window(WindowKind::Skills).unwrap();
        let ncu = s.hud.ncu.window().unwrap();
        // the template's DockArea2 pins the NCU window (`WindowPinButtonState` true); nothing is written for an untouched layout
        assert!(s.gui.window_pinned(ncu) && !s.gui.window_pinned(inv));
        assert!(!tmp.join("acct/Char7/DockAreas/DockArea0.xml").exists());
        let (x0, y0, w0, h0) = s.gui.window_outer_frame(inv).unwrap();
        let cols0 = s.hud.stats.inventory_cols().unwrap();
        // drag by the strip, then the right edge, then pin
        drag_strip(&mut s, inv, -150.0, -200.0);
        let (x1, y1, ..) = s.gui.window_outer_frame(inv).unwrap();
        assert_eq!((x1, y1), (x0 - 150, y0 - 200));
        let (px, py) = ((x1 + w0 as i32 - 2) as f32, (y1 + h0 as i32 / 2) as f32);
        send(&mut s, InputEvent::MouseDown { x: px, y: py, button: MouseButton::Left });
        send(&mut s, InputEvent::MouseMove { x: px + 120.0, y: py });
        send(&mut s, InputEvent::MouseUp { x: px + 120.0, y: py, button: MouseButton::Left });
        let frame = s.gui.window_outer_frame(inv).unwrap();
        assert_eq!(frame.2, w0 + 120, "the right edge resizes the inventory");
        let cols1 = s.hud.stats.inventory_cols().unwrap();
        assert!(cols1 > cols0, "the grid reflows to more columns ({cols0} -> {cols1})");
        s.gui.set_window_pinned(inv, true);
        drag_strip(&mut s, skills, 25.0, 10.0);
        let sk = s.gui.window_outer_frame(skills).unwrap();
        s.frame(0.016, size, &mut o.host);
        s.hud.dvalues.save_user();
        let dock = std::fs::read_to_string(tmp.join("acct/Char7/DockAreas/DockArea0.xml")).expect("the dock file of the inventory");
        let cfg = super::super::hud_wincfg::Cfg::parse(&xml::parse(&dock).unwrap());
        let (x, y, w, h) = frame;
        assert_eq!(cfg.frame, Some([x as f32, y as f32, (x + w as i32 - 1) as f32, (y + h as i32 - 1) as f32]));
        // `selected_tab` -1 is the template's value for the inventory dock, kept as read
        assert_eq!((cfg.pin, cfg.tab), (Some(true), Some(-1)));
        assert!(dock.contains("inventory_window") && dock.contains("DockTabbedWindow"));
        let prefs = std::fs::read_to_string(tmp.join("acct/Char7/Prefs.xml")).unwrap();
        assert!(prefs.contains("SkillConfig") && prefs.contains("WindowFrame"));
        // the next login: a fresh HUD reads the files
        drop((s, o));
        let Some((mut s, mut o)) = shot(size) else { return };
        s.hud.dvalues.open_user(&tmp, "acct", 7);
        s.hud.prefs_loaded(&mut s.gui);
        s.frame(0.016, size, &mut o.host);
        assert!(s.hud.is_open(WindowKind::Inventory) && s.hud.is_open(WindowKind::Skills), "the open flags come back");
        let inv = s.hud.stats_window(WindowKind::Inventory).unwrap();
        assert_eq!(s.gui.window_outer_frame(inv), Some(frame));
        assert!(s.gui.window_pinned(inv));
        assert_eq!(s.hud.stats.inventory_cols(), Some(cols1));
        assert_eq!(s.gui.window_outer_frame(s.hud.stats_window(WindowKind::Skills).unwrap()), Some(sk));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The first-login hotbar: Start Combat / Walk / Sit / Suspended Animation icons from rdb 1010008, the Follow macro, use,
    /// drag-and-drop between slots and removal by dropping outside.
    #[test]
    fn hotbar_first_login_and_interaction() {
        let Some((mut s, mut o)) = shot((1280, 800)) else { return };
        let names = s.hud.shortcuts[0].slot_names();
        assert_eq!(names, ["Start Combat", "Walk", "Sit", "Follow", "", "", "", "", "", "Suspended Animation"]);
        assert_eq!(s.hud.shortcuts[0].icon_count(), 5, "four rdb icons and the macro icon");
        png(&mut s, &mut o, "hud-hotbar");
        let win = s.hud.shortcuts[0].window;
        let (wx, wy) = s.gui.window_pos(win);
        let at = |i: usize| (wx as f32 + 43.0 + 36.0 * i as f32 + 17.0, wy as f32 + 19.0);
        // use fires on the release of a short press (`FUN_10040a17`), never on the press; empty slots do nothing
        let mut uses = vec![];
        for i in [0, 3, 5] {
            let (x, y) = at(i);
            send(&mut s, InputEvent::MouseDown { x, y, button: MouseButton::Left });
            assert!(s.hud.take_uses().is_empty(), "no use on the press");
            send(&mut s, InputEvent::MouseUp { x, y, button: MouseButton::Left });
            uses.extend(s.hud.take_uses());
        }
        assert_eq!(uses, vec![SlotUse::SpecialAction(0xb), SlotUse::Macro("/follow".into())]);
        // a slot held for 0.3 s is dragged without moving and its release is no use
        let (x0, y0) = at(0);
        send(&mut s, InputEvent::MouseDown { x: x0, y: y0, button: MouseButton::Left });
        s.hud.update(&mut s.gui, &mut s.zone, 0.4);
        send(&mut s, InputEvent::MouseUp { x: x0, y: y0, button: MouseButton::Left });
        assert!(s.hud.take_uses().is_empty());
        // drag slot 1 onto slot 5, then slot 5 out of the bar
        let ((x1, y), (x5, _)) = (at(1), at(5));
        send(&mut s, InputEvent::MouseDown { x: x1, y, button: MouseButton::Left });
        s.hud.update(&mut s.gui, &mut s.zone, 0.4);
        send(&mut s, InputEvent::MouseMove { x: x1 + 20.0, y });
        send(&mut s, InputEvent::MouseMove { x: x5, y });
        png(&mut s, &mut o, "hud-hotbar-dragging");
        send(&mut s, InputEvent::MouseUp { x: x5, y, button: MouseButton::Left });
        assert_eq!(s.hud.shortcuts[0].slot_names()[1], "");
        assert_eq!(s.hud.shortcuts[0].slot_names()[5], "Walk");
        send(&mut s, InputEvent::MouseDown { x: x5, y, button: MouseButton::Left });
        s.hud.update(&mut s.gui, &mut s.zone, 0.4);
        send(&mut s, InputEvent::MouseMove { x: x5 + 10.0, y: y + 200.0 });
        send(&mut s, InputEvent::MouseUp { x: x5 + 10.0, y: y + 200.0, button: MouseButton::Left });
        assert_eq!(s.hud.shortcuts[0].slot_names()[5], "");
        png(&mut s, &mut o, "hud-hotbar-after");
    }

    /// `/macro`: the new macro is dragged with the macro icon, a drop on a slot stores `{0xc789, id}` (use runs the command), a drop outside the bar discards it.
    #[test]
    fn macro_drag_drops_on_slot_or_is_discarded() {
        let Some((mut s, mut o)) = shot((1280, 800)) else { return };
        let win = s.hud.shortcuts[0].window;
        let (wx, wy) = s.gui.window_pos(win);
        let at = |i: usize| (wx as f32 + 43.0 + 36.0 * i as f32 + 17.0, wy as f32 + 19.0);
        let (x6, y) = at(6);
        send(&mut s, InputEvent::MouseMove { x: x6 - 50.0, y });
        s.hud.begin_macro_drag(&mut s.gui, 7, "Hi", "/say hi");
        send(&mut s, InputEvent::MouseMove { x: x6, y });
        png(&mut s, &mut o, "hud-macro-drag");
        send(&mut s, InputEvent::MouseUp { x: x6, y, button: MouseButton::Left });
        assert_eq!(s.hud.shortcuts[0].slot_names()[6], "Hi");
        assert_eq!(s.hud.shortcuts[0].icon_count(), 6, "the macro shows GFX_GUI_ICON_MACRO");
        // use fires on the release of a short press on the slot (`FUN_10040a17`)
        send(&mut s, InputEvent::MouseDown { x: x6, y, button: MouseButton::Left });
        send(&mut s, InputEvent::MouseUp { x: x6, y, button: MouseButton::Left });
        assert_eq!(s.hud.take_uses(), vec![SlotUse::Macro("/say hi".into())]);
        // dropped outside the bar: discarded, the bar is unchanged
        s.hud.begin_macro_drag(&mut s.gui, 8, "Gone", "/x");
        send(&mut s, InputEvent::MouseUp { x: x6, y: y + 300.0, button: MouseButton::Left });
        assert!(!s.hud.shortcuts[0].slot_names().contains(&"Gone".to_string()));
        assert_eq!(s.hud.shortcuts[0].slot_names()[6], "Hi");
    }
}
