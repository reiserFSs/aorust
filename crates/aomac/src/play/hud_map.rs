//! The Planet Map window (`planetmap_window`, `PlanetMapView_c`) and the playfield Map window (`map_window`,
//! `PlayfieldMapView_c` + `PFMapRenderer_c`). Evidence: docs/gui.md §12, docs/formats.md *Planet map* / *Playfield map image*.
//!
//! * Planet map: tiles of `textures/PlanetMap/<index>` ([`ao_formats::planetmap`]); markers by `Level::locate`
//!   (`FUN_1004b81a`); zoom levels = the index's levels (`FUN_1004bc1f`: ZoomIn enabled below the last level, ZoomOut above 0).
//! * Playfield map: [`ao_formats::topdown`] image of the ground / room shells of the playfield (built on a worker thread),
//!   centred on the own character; dungeon rooms are lit by exploration (`MapData_c::FUN_100427a1`: current room white,
//!   visited 50 % grey, unexplored black).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use ao_formats::map_areas;
use ao_formats::landcontrol::{self, LandControlMap};
use ao_formats::planetmap::PlanetMap;
use ao_formats::screens::TextDb;
use ao_formats::topdown::{self, GroundMap, NO_OWNER};
use ao_gui::{CanvasItem, CanvasTip, Event, GfxId, Gui, MouseButton, WindowId, WindowSize};
use ao_rdb::RecordStore;

use super::hud::WindowKind;
use super::hud_rollup::Rollup;
use super::zone::Zone;

/// Default client size of the Planet Map window (`Rect(0,0,_DAT_101b183c,_DAT_101b1840)` in `PlanetMapView_c` 0x1004d26a).
const PLANET_SIZE: (u32, u32) = (300, 400);
/// First-time frame `Rect(100,100,500,500)` (`_DAT_101ae160`, `_DAT_101b172c`): top-left corner.
const PLANET_POS: (i32, i32) = (100, 100);
/// Default client size of the playfield map (`Rect(0,0,_DAT_101c0bcc,_DAT_101c0bd0)` in `PlayfieldMapView_c` 0x100eb905).
const PF_SIZE: (u32, u32) = (179, 199);
// First-time placement: `DockableView_c::LoadConfig` (`FUN_10038c98`, GUI 0x10038c98, called by the ctor with `"RollupArea"`) docks the view into the named dock via
// `FUN_1003a26f` (dock lookup by name) when it exists; the centred screen rect it computes is only the fallback of a newly created dock. So the map is a rollup page.
/// Two native HLayout rows of 27px buttons (`PlanetMapView_c`0x1004d26a).
const BUTTONS_H: u32 = 54;
const MENU_PLANET_BUTTONS: u32 = 0x7700;
const MENU_PF_BUTTONS: u32 = 0x7701;
/// Longest side of the playfield map image.
const PF_MAX_PX: u32 = 2048;
/// MapControlView_c100ec4e0: ordinary controls, not invented overlay coordinates.
const PF_XML: &str = r#"<root><View view_layout="vertical">
<ViewSelector name="sel"><CanvasView name="map" view_layout="stacked" max_size="Point(16000,16000)">
<TextView name="lc_info" view_flags="0x100" feature_flags="TVF_MULTILINE" h_alignment="left" v_alignment="top" layout_borders="Rect(5,5,0,0)"/>
</CanvasView><TextView name="na" value="&lt;center&gt;Map&amp;nbsp;Not&lt;br&gt;Available&lt;/center&gt;" feature_flags="TVF_MULTILINE" h_alignment="center" v_alignment="center"/></ViewSelector>
<View name="pf_controls" view_flags="0x100" view_layout="vertical" weight="0" layout_borders="Rect(3,3,3,3)">
<View view_layout="horizontal"><Button name="lc_toggle"/><HLayoutSpacer min_size="5" max_size="16000" weight="1"/>
<Button name="lc_build"/><HLayoutSpacer min_size="5" max_size="16000" weight="1"/><Button name="lc_war"/></View>
<View view_layout="horizontal"><TextInputView name="pf_x" layout_borders="Rect(3,3,3,0)"/><TextInputView name="pf_z" layout_borders="Rect(3,3,3,0)"/>
<Button name="pf_add_waypoint" text="Add Waypoint" layout_borders="Rect(3,3,3,0)"/><Button name="pf_remove_waypoint" text="Remove Waypoint" layout_borders="Rect(3,3,3,0)"/>
<VLayoutSpacer min_size="0" max_size="16000" weight="1"/></View></View></View></root>"#;
/// Screen pixels per terrain cell: `MapData_c::FUN_10042996` sets the content rect to `cells * pt - 1` (`pt` = 4 `_DAT_101b0840` when its first argument, `+8`, is set, else
/// 2 `_DAT_101ae17c`) and `PFMapRenderer_c`'s ctor / `FUN_100e9729` scales the view by `_DAT_101b36b0 / pt` = 4 / pt, so one terrain cell is 4 px either way
/// (cell size in metres: `AnarchyGround_t +0x8264`, [`ao_formats::playfield::Report::cell_size`]).
const PF_PX_PER_CELL: f32 = 4.0;
/// UNRESOLVED: the screen scale of a dungeon's room map (no terrain, `MapData_c +0xb0` = 0 so the content rect is never set).
const PF_SCALE_ROOMS: f32 = 1.0;
/// `N3Msg_GetMapCharacters(range 500)` in `FUN_100425eb`: dots are drawn for characters within this distance.
const DOT_RANGE: f32 = 500.0;
/// New planet map tiles decoded per frame.
const TILES_PER_FRAME: usize = 6;

const RUBIKA_INDEX: &str = "Normal/PlanetMapIndexNormal.txt";
const SHADOWLANDS_INDEX: &str = "Shadowlands/ShadowlandsMap.txt";

/// Tab titles: the `title` argument of the `DockableView_c` constructor (`FUN_10038b47`, called with `"Planet Map"` by `PlanetMapView_c` 0x1004d26a and
/// `"PF Map"` by `PlayfieldMapView_c` 0x100eb905) is what `DockWindow_c` puts into its tab (`FUN_100389bd` -> `Window::InsertTab`). The `#WindowMap` /
/// `#WindowPlanetMap` strings of `ControlCenterModule_c::SetupProviders` 0x10068c38 are the labels of the key-binding providers, not window titles.
const PLANET_TITLE: &str = "Planet Map";
const PF_TITLE: &str = "PF Map";

/// The four `Button_c`s of `PlanetMapView_c` (names from 0x1004d26a) with the skin art of the same name, the pressed art
/// (UNRESOLVED: the pairing by name, the art is assigned outside the constructor) and the tooltip key (`View::SetToolTip(GetText(10000, key), "")`, ctor asm
/// 0x1004de3c..0x1004df61: `Click2ZoomEtc` / `Click2ZoomOutEtc` / `CenterOnChar` / `CenterOnMission`).
const BUTTONS: [(&str, &str, &str, &str); 4] = [
    ("ZoomIn", "GFX_GUI_PLANETMAP_ZOOM_IN", "GFX_GUI_PLANETMAP_ZOOM_IN_PRESSED", "Click2ZoomEtc"),
    ("ZoomOut", "GFX_GUI_PLANETMAP_ZOOM_OUT", "GFX_GUI_PLANETMAP_ZOOM_OUT_PRESSED", "Click2ZoomOutEtc"),
    ("Character", "GFX_GUI_PLANETMAP_CENTER_PLAYER", "GFX_GUI_PLANETMAP_CENTER_PLAYER_PRESSED", "CenterOnChar"),
    ("Quest", "GFX_GUI_PLANETMAP_CENTER_MISSION", "GFX_GUI_PLANETMAP_CENTER_MISSION_PRESSED", "CenterOnMission"),
];

struct Planet {
    window: WindowId,
    map: Option<PlanetMap>,
    /// Playfield the map was last picked for (`Some(None)`: before the zone told).
    tried: Option<Option<u32>>,
    level: usize,
    /// Canvas pixel (of `level`) shown in the middle of the view.
    center: [f32; 2],
    /// Marker position the view last followed.
    followed: Option<[f32; 2]>,
    tiles: HashMap<(usize, u32, u32), GfxId>,
    /// Button (index into [`BUTTONS`]) the left mouse button went down on, and whether the pointer is still over it.
    pressed: Option<(usize, bool)>,
    /// A double-click zoom placed the view: the next update keeps it instead of re-centring on the character.
    hold: bool,
}

struct Pf {
    window: WindowId,
    playfield: u32,
    ground: Option<Ground>,
    /// World position (server `X`, `Z`) in the middle of the view.
    center: [f32; 2],
    followed: Option<[f32; 2]>,
    arrows: HashMap<u8, GfxId>,
    /// The own character owns the map of the playfield ([`map_areas::owns_map`]): `MapData_c +0x80` = `N3Msg_GetMapCharacters` succeeded.
    owns: bool,
    /// GUI100eacda: ownership or enabled land-control bitmap.
    available: bool,
    land: Option<LandControlMap>,
    land_image: Option<(bool, GfxId)>,
    district: Option<(String, (u16, u16))>,
    lc_poll: f32,
}

struct Ground {
    map: GroundMap,
    /// Dungeon: rooms are lit by exploration.
    rooms: bool,
    /// Screen pixels per metre.
    scale: f32,
    visited: HashSet<u16>,
    current: u16,
    /// The image of the current (`current`, `visited.len()`) state.
    shown: Option<(u16, usize, GfxId)>,
}

pub(super) struct HudMap {
    client: PathBuf,
    texts: Option<TextDb>,
    store: Option<RecordStore>,
    planet: Option<Planet>,
    pf: Option<Pf>,
    planet_buttons: bool,
    pf_buttons: bool,
    land_control: bool,
    config_loaded: bool,
    config_dirty: bool,
    waypoint: Option<Option<(u32, [f32; 2])>>,
    lines: Vec<(String, u32)>,
    closed: Vec<WindowKind>,
    /// The ground image of the loaded playfield, built by the world loader from the scene it just decoded (`Report::ground`, [`ground_map`]); the
    /// `Option` is the terrain cell size in metres (`None`: a dungeon, rooms lit by exploration).
    ground_in: Option<(u32, GroundMap, Option<f32>)>,
    /// Native mission and manual waypoint producers share this planet-map consumer.
    mission: Option<(u32, [f32; 2])>,
}

/// The playfield map's ground image: the `Report::ground` instances of the just-built `scene`, rendered from above, and the terrain cell size (`None`: dungeon rooms).
pub(in crate::play) fn ground_map(scene: &ao_scene::Scene, report: &ao_formats::playfield::Report) -> Option<(GroundMap, Option<f32>)> {
    topdown::render(scene, report.ground.clone(), PF_MAX_PX).map(|m| (m, (report.terrain_cells != 0).then_some(report.cell_size)))
}

fn canvas_xml(name: &str, size: (u32, u32)) -> String {
    let min = if name == "buttons" { (size.0.min(54).saturating_sub(1) as i32, size.1.saturating_sub(1) as i32) } else { (-1, -1) };
    let flags = if name == "buttons" { " view_flags=\"0x100\"" } else { "" };
    format!("<CanvasView name=\"{name}\"{flags} min_size=\"Point({},{})\" max_size=\"Point(16000,{})\"/>", min.0, min.1, if name == "buttons" { BUTTONS_H - 1 } else { 16000 })
}

impl HudMap {
    pub(super) fn new(client: &std::path::Path) -> Self {
        HudMap { client: client.to_path_buf(), texts: TextDb::load(client).ok(), store: RecordStore::open(client).ok(), planet: None, pf: None, planet_buttons: false, pf_buttons: false, land_control: false, config_loaded: false, config_dirty: false, waypoint: None, lines: vec![], closed: vec![], ground_in: None, mission: None }
    }

    pub(super) fn take_waypoint(&mut self) -> Option<Option<(u32, [f32; 2])>> {
        self.waypoint.take()
    }

    pub(super) fn take_lines(&mut self) -> Vec<(String, u32)> {
        std::mem::take(&mut self.lines)
    }

    /// Native map configs retain window geometry and unrelated message fields.
    pub(super) fn sync_config(&mut self, gui: &mut Gui, d: &mut super::dvalue::DValues) {
        use super::dvalue::{element_xml, Variant, CAT_CHAR};
        use ao_gui::xml::{self, Element};
        if self.config_loaded && !self.config_dirty { return; }
        for (name, planet) in [("PlanetMapViewConfig", true), ("PFMapWindowConfig", false)] {
            let mut root = match d.get(name) {
                Some(Variant::Archive(source)) => xml::parse(source).ok(),
                _ => None,
            }.unwrap_or_else(|| Element { name: "Archive".into(), attrs: vec![("name".into(), name.into()), ("code".into(), "0".into())], children: vec![] });
            let flag = |key: &str| root.children.iter().find(|e| e.attr("name") == Some(key)).and_then(|e| e.attr("value")).is_some_and(|v| v == "true" || v == "1");
            if !self.config_loaded && !self.config_dirty {
                if planet { self.planet_buttons = flag("ShowButtons"); }
                else { self.pf_buttons = flag("ShowButtons"); self.land_control = flag("RenderLandControlData"); }
            }
            if self.config_dirty {
                let fields = [("ShowButtons", if planet { self.planet_buttons } else { self.pf_buttons }), ("RenderLandControlData", self.land_control)];
                for (key, value) in fields.into_iter().take(if planet { 1 } else { 2 }) {
                    root.children.retain(|e| e.attr("name") != Some(key));
                    root.children.push(Element { name: "Bool".into(), attrs: vec![("name".into(), key.into()), ("value".into(), value.to_string())], children: vec![] });
                }
                let value = Variant::Archive(element_xml(&root));
                if d.exists(name) { d.set(name, value); } else { d.add(name, value, true, CAT_CHAR, None, None, false); }
            }
        }
        self.config_loaded = true;
        self.config_dirty = false;
        if let Some(p) = &self.planet { gui.set_visible(p.window, "buttons", self.planet_buttons); }
        if let Some(p) = &self.pf { gui.set_visible(p.window, "pf_controls", self.pf_buttons); }
    }

    /// The loader's ground image of playfield `pf` (replaces the one of an earlier playfield).
    pub(super) fn provide_ground(&mut self, pf: u32, map: GroundMap, cell: Option<f32>) {
        self.ground_in = Some((pf, map, cell));
        if let Some(p) = self.pf.as_mut() {
            p.playfield = 0; // taken again by the next update
        }
    }

    /// The mission marker (`FUN_1004b9c9`): a world position in a playfield, `None` when the mission ends (`FUN_1004bfbf`).
    pub(super) fn set_mission(&mut self, marker: Option<(u32, [f32; 2])>) {
        self.mission = marker;
    }

    pub(super) fn is_open(&self, kind: WindowKind) -> bool {
        match kind {
            WindowKind::Map => self.pf.is_some(),
            WindowKind::PlanetMap => self.planet.is_some(),
            _ => false,
        }
    }

    /// Windows the user closed with their frame button since the last call.
    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn open(&mut self, gui: &mut Gui, rollup: &mut Rollup, kind: WindowKind) {
        if self.is_open(kind) {
            return;
        }
        match kind {
            WindowKind::PlanetMap => {
                let xml = format!(
                    "<root><View view_layout=\"vertical\">{}{}</View></root>",
                    canvas_xml("map", (PLANET_SIZE.0, PLANET_SIZE.1 - BUTTONS_H)),
                    canvas_xml("buttons", (PLANET_SIZE.0, BUTTONS_H))
                );
                match gui.open_tabbed_window_xml("PlanetMapView", PLANET_TITLE, &xml, PLANET_POS, WindowSize::Fixed(PLANET_SIZE.0, PLANET_SIZE.1)) {
                    Ok(window) => {
                        gui.set_window_context(window, true);
                        gui.set_visible(window, "buttons", self.planet_buttons);
                        set_button_tips(gui, window, self.texts.as_ref());
                        let (w, h) = gui.canvas_size(window, "map");
                        self.planet = Some(Planet { window, map: None, tried: None, level: 0, center: [w as f32 / 2.0, h as f32 / 2.0], followed: None, tiles: HashMap::new(), pressed: None, hold: false })
                    }
                    Err(e) => eprintln!("planet map: {e:#}"),
                }
            }
            WindowKind::Map => {
                let xml = PF_XML;
                // `FUN_10038c98` (`DockableViewDockName` default "RollupArea"): a page of the rollup dock, not a free window
                match rollup.open_page(gui, kind.dvalue(), PF_TITLE, xml, PF_SIZE.1 as f32) {
                    Ok(window) => {
                        gui.select_child(window, "sel", Some(0));
                        gui.set_window_context(window, true);
                        gui.set_visible(window, "pf_controls", self.pf_buttons);
                        gui.set_visible(window, "lc_info", false);
                        gui.set_text_pref_size(window, "lc_info", (0.0, 0.0), (16000.0, 16000.0));
                        for (name, key) in [("lc_toggle", "Info"), ("lc_build", "Build"), ("lc_war", "Plafieldmap_War")] {
                            if let Some(text) = self.texts.as_ref().and_then(|t| t.by_key(10000, key)) { gui.set_text(window, name, &super::hud_listview::esc(&text)); }
                        }
                        for (name, key) in [("lc_toggle", "ShowLConMap"), ("lc_build", "OpenLCbuild"), ("lc_war", "OpenLCwar")] {
                            if let Some(text) = self.texts.as_ref().and_then(|t| t.by_key(10000, key)) { gui.set_tooltip(window, name, &text, ""); }
                        }
                        self.pf = Some(Pf { window, playfield: 0, ground: None, center: [0.0; 2], followed: None, arrows: HashMap::new(), owns: true, available: true, land: None, land_image: None, district: None, lc_poll: 0.0 })
                    }
                    Err(e) => eprintln!("playfield map: {e:#}"),
                }
            }
            _ => {}
        }
    }

    fn close_planet(&mut self, gui: &mut Gui) {
        if let Some(p) = self.planet.take() {
            gui.close_window(p.window);
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, rollup: &mut Rollup, kind: WindowKind) {
        match kind {
            WindowKind::PlanetMap => self.close_planet(gui),
            WindowKind::Map if self.pf.take().is_some() => rollup.close_page(gui, kind.dvalue()),
            _ => {}
        }
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui, rollup: &mut Rollup) {
        self.close(gui, rollup, WindowKind::PlanetMap);
        self.close(gui, rollup, WindowKind::Map);
    }

    fn toggle_planet_buttons(&mut self, gui: &mut Gui) {
        self.planet_buttons = !self.planet_buttons;
        self.config_dirty = true;
        if let Some(p) = &self.planet {
            gui.set_visible(p.window, "buttons", self.planet_buttons);
            set_button_tips(gui, p.window, self.texts.as_ref());
        }
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone, screen: (u32, u32)) -> bool {
        if matches!(ev, Event::MenuPicked { id: MENU_PLANET_BUTTONS }) {
            self.toggle_planet_buttons(gui);
            return true;
        }
        if matches!(ev, Event::MenuPicked { id: MENU_PF_BUTTONS }) {
            self.pf_buttons = !self.pf_buttons;
            self.config_dirty = true;
            if let Some(p) = &self.pf { gui.set_visible(p.window, "pf_controls", self.pf_buttons); }
            return true;
        }
        if let Some(p) = self.planet.as_mut() {
            match ev {
                Event::FrameIcon { window, x, y } if *window == p.window => {
                    gui.open_menu((*x, *y), (screen.0 as i32, screen.1 as i32), vec![ao_gui::MenuItem::check(MENU_PLANET_BUTTONS, "Show buttons", self.planet_buttons)]);
                    return true;
                }
                Event::WindowFrame { window } if *window == p.window => {
                    set_button_tips(gui, p.window, self.texts.as_ref());
                }
                Event::CloseRequested { window } if *window == p.window => {
                    self.close_planet(gui);
                    self.closed.push(WindowKind::PlanetMap);
                    return true;
                }
                Event::CanvasDrag { window, view, dx, dy, x, y } if *window == p.window => {
                    match view.as_str() {
                        "map" => p.center = [p.center[0] - dx, p.center[1] - dy],
                        // the pressed `Button_c` shows its pressed art only while the pointer is over it
                        _ => {
                            if let Some((i, _)) = p.pressed {
                                p.pressed = Some((i, button_index_at(*x, *y, gui.canvas_size(p.window, "buttons").0 as f32) == Some(i)));
                            }
                        }
                    }
                    return true;
                }
                Event::CanvasPress { window, view, x, y, button, clicks } if *window == p.window => {
                    match (view.as_str(), button) {
                        ("buttons", MouseButton::Left) => p.pressed = button_index_at(*x, *y, gui.canvas_size(p.window, "buttons").0 as f32).map(|i| (i, true)),
                        // `BitmapTileView_c` signals (ctor of the view, `FUN_1004bc1f`): double click with the left button zooms in (`LAB_1004b447`),
                        // with the right button out (`LAB_1004b468`), around the clicked point (`FUN_1004b422` stores it for `FUN_1004bc1f`)
                        ("map", MouseButton::Left) if *clicks == 2 => p.zoom_at(1, gui.canvas_size(p.window, "map"), [*x, *y]),
                        ("map", MouseButton::Right) if *clicks == 2 => p.zoom_at(-1, gui.canvas_size(p.window, "map"), [*x, *y]),
                        _ => {}
                    }
                    return true;
                }
                Event::CanvasRelease { window, .. } if *window == p.window => {
                    p.pressed = None;
                    return true;
                }
                Event::CanvasWheel { window, view, dy, .. } if *window == p.window && view == "map" => {
                    p.zoom(if *dy > 0.0 { 1 } else { -1 });
                    return true;
                }
                Event::CanvasClick { window, view, x, y } if *window == p.window && view == "buttons" => {
                    if let Some(i) = button_index_at(*x, *y, gui.canvas_size(p.window, "buttons").0 as f32) {
                        let marker = p.mission_marker(self.mission);
                        match BUTTONS[i].0 {
                            "ZoomIn" => p.zoom(1),
                            "ZoomOut" => p.zoom(-1),
                            // `FUN_1004b6d4`: centre on the own character again (the follow state is dropped)
                            "Character" => p.followed = None,
                            // `FUN_1004b72b`: centre on the mission marker, if there is one (the button is enabled by `FUN_1004b9c9`)
                            _ => {
                                if let Some(m) = marker {
                                    p.center = m;
                                    p.hold = true;
                                }
                            }
                        }
                    }
                    return true;
                }
                _ => {}
            }
        }
        if let Some(p) = self.pf.as_mut() {
            match ev {
                Event::FrameIcon { window, x, y } if *window == p.window => {
                    gui.open_menu((*x, *y), (screen.0 as i32, screen.1 as i32), vec![ao_gui::MenuItem::check(MENU_PF_BUTTONS, "Show buttons", self.pf_buttons)]);
                    return true;
                }
                Event::Clicked { window, view, .. } if *window == p.window => match view.as_str() {
                    "lc_toggle" if zone.skill_value(389).unwrap_or(0) & 1 != 0 => {
                        self.land_control = !self.land_control;
                        self.config_dirty = true;
                        p.lc_poll = 0.0;
                        return true;
                    }
                    // Installed native Build/War emit GlobalSignals+170(bool), with no
                    // subscribers in the shipped modules. Do not invent a generic opener.
                    "lc_build" | "lc_war" if zone.skill_value(389).unwrap_or(0) & 1 != 0 => return true,
                    "pf_add_waypoint" => {
                        let x = super::chat::atof(&gui.text(p.window, "pf_x")) as f32;
                        let z = super::chat::atof(&gui.text(p.window, "pf_z")) as f32;
                        if x > 0.0 && z > 0.0 {
                            self.waypoint = Some(Some((zone.playfield.unwrap_or(0), [x, z])));
                        } else {
                            self.lines.push(("Coordinates not in range.".into(), 0x52));
                        }
                        return true;
                    }
                    "pf_remove_waypoint" => {
                        self.waypoint = Some(None);
                        return true;
                    }
                    _ => {}
                },
                _ => {}
            }
            if let Event::CanvasDrag { window, view, dx, dy, .. } = ev {
                if *window == p.window && view == "map" {
                    let scale = p.ground.as_ref().map_or(PF_SCALE_ROOMS, |g| g.scale);
                    p.center = [p.center[0] - dx / scale, p.center[1] + dy / scale];
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, _dt: f32) {
        self.update_planet(gui, zone);
        self.update_pf(gui, zone, _dt);
    }

    // ------------------------------------------------------------------ planet map

    fn update_planet(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(p) = self.planet.as_mut() else { return };
        if p.tried != Some(zone.playfield) {
            p.tried = Some(zone.playfield);
            match pick_planet_map(&self.client, zone.playfield) {
                Ok(m) => {
                    p.level = p.level.min(m.index.levels.len() - 1);
                    p.map = Some(m);
                    p.tiles.clear();
                    p.followed = None;
                }
                Err(e) => eprintln!("planet map: {e:#}"),
            }
        }
        let Some(map) = p.map.as_ref() else { return };
        let lv = &map.index.levels[p.level];
        // the marker: `FUN_1004b81a` hides it when the playfield has no coordinates entry or the position is unknown
        let marker = zone.playfield.and_then(|pf| map.coords.get(&pf)).zip(zone.own()).map(|(c, d)| lv.locate(c, d.pos[0], d.pos[2]));
        if std::mem::take(&mut p.hold) {
            // a double-click zoom or the mission button placed the view (`FUN_1004b422` / `FUN_1004b72b` after `FUN_1004b6d4`)
            p.followed = marker;
        } else if p.followed != marker {
            // moving the character re-centres the view on it (help text *The PlanetMap Window*)
            if let Some(m) = marker {
                p.center = m;
            }
            p.followed = marker;
        }
        let (cw, ch) = gui.canvas_size(p.window, "map");
        let (cw, ch) = (cw as f32, ch as f32);
        let origin = [p.center[0] - cw / 2.0, p.center[1] - ch / 2.0];
        let ts = lv.texture_size as f32;
        let mut items = vec![CanvasItem::Solid { dst: [0.0, 0.0, cw, ch], color: 0x000000, alpha: 1.0 }];
        let range = |o: f32, len: f32, n: u32| (o / ts).floor().max(0.0) as i64..=(((o + len) / ts).floor() as i64).min(n as i64 - 1);
        let mut fresh = 0;
        for ty in range(origin[1], ch, lv.tiles[1]) {
            for tx in range(origin[0], cw, lv.tiles[0]) {
                let (tx, ty) = (tx as u32, ty as u32);
                let key = (p.level, tx, ty);
                let id = match p.tiles.get(&key) {
                    Some(id) => *id,
                    None if fresh < TILES_PER_FRAME => {
                        fresh += 1;
                        let Ok(t) = map.tile(p.level, tx, ty) else { continue };
                        let id = gui.add_image("planetmap tile", t.rgba, t.width, t.height, false);
                        p.tiles.insert(key, id);
                        id
                    }
                    None => continue,
                };
                let (x, y) = (tx as f32 * ts - origin[0], ty as f32 * ts - origin[1]);
                items.push(CanvasItem::Image { id, src: [0.0, 0.0, ts, ts], dst: [x, y, x + ts, y + ts], alpha: 1.0 });
            }
        }
        let mission = p.mission_marker(self.mission);
        if let (Some(m), Some(g)) = (mission, gui.gfx().id("GFX_GUI_PLANETMAP_MISSION_MARKER")) {
            let (w, h) = gui.gfx().size(g);
            let (x, y) = (m[0] - origin[0] - w as f32 / 2.0, m[1] - origin[1] - h as f32 / 2.0);
            items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x.floor(), y.floor(), x.floor() + w as f32, y.floor() + h as f32], alpha: 1.0 });
        }
        if let (Some(m), Some(g)) = (marker, gui.gfx().id("GFX_GUI_PLANETMAP_PLAYER_MARKER")) {
            let (w, h) = gui.gfx().size(g);
            let (x, y) = (m[0] - origin[0] - w as f32 / 2.0, m[1] - origin[1] - h as f32 / 2.0);
            items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x.floor(), y.floor(), x.floor() + w as f32, y.floor() + h as f32], alpha: 1.0 });
        }
        gui.set_canvas(p.window, "map", items);
        // Each pair sits between equal flexible spacers in its own HLayout row.
        let levels = map.index.levels.len();
        let mut row = Vec::new();
        for (i, (name, gfx, gfx_pressed, _)) in BUTTONS.iter().enumerate() {
            let enabled = match *name {
                "ZoomIn" => p.level + 1 < levels,
                "ZoomOut" => p.level > 0,
                "Character" => marker.is_some(),
                _ => mission.is_some(),
            };
            let down = enabled && p.pressed == Some((i, true));
            let Some(g) = gui.gfx().id(if down { gfx_pressed } else { gfx }) else { continue };
            let (w, h) = gui.gfx().size(g);
            let (x, _) = button_rect(i, gui.canvas_size(p.window, "buttons").0 as f32);
            let y = (i / 2) as f32 * 27.0;
            row.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x, y, x + w as f32, y + h as f32], alpha: if enabled { 1.0 } else { 0.4 } });
        }
        gui.set_canvas(p.window, "buttons", row);
    }

    // ------------------------------------------------------------------ playfield map

    fn update_pf(&mut self, gui: &mut Gui, zone: &Zone, dt: f32) {
        let Some(p) = self.pf.as_mut() else { return };
        if let Some(pf) = zone.playfield.filter(|&pf| pf != p.playfield) {
            // a new playfield: the map is rebuilt (exploration starts again, "since you last entered"); the image is the one the world loader
            // made from the scene of this playfield ([`ground_map`] -> `provide_ground`), so nothing is decoded a second time here
            p.playfield = pf;
            p.ground = None;
            p.followed = None;
            p.land = self.store.as_ref().and_then(|s| s.get(landcontrol::RDB_TYPE, pf).ok().flatten()).and_then(|bytes| landcontrol::parse(&bytes).ok());
            p.land_image = None;
            p.district = None;
            p.lc_poll = 0.0;
            gui.set_text(p.window, "lc_info", "");
        }
        if p.ground.is_none() && self.ground_in.as_ref().is_some_and(|g| Some(g.0) == zone.playfield) {
            if let Some((_, map, cell)) = self.ground_in.take() {
                let scale = cell.filter(|&c| c > 0.0).map_or(PF_SCALE_ROOMS, |c| PF_PX_PER_CELL / c);
                p.ground = Some(Ground { map, rooms: cell.is_none(), scale, visited: HashSet::new(), current: NO_OWNER, shown: None });
            }
        }
        // `MapData_c::FUN_100425eb`: `owns` = `N3Msg_GetMapCharacters` result (= `FUN_10057ca5`); `FUN_100eacda` swaps the map for the "Map Not Available" text
        p.owns = map_areas::owns_map(zone.playfield, |s| zone.stat(s).unwrap_or(0));
        let available = p.owns || (self.land_control && p.land.is_some());
        if available != p.available {
            p.available = available;
            gui.select_child(p.window, "sel", Some(if p.available { 0 } else { 1 }));
        }
        let expansion = zone.skill_value(389).unwrap_or(0) & 1 != 0;
        for name in ["lc_toggle", "lc_build", "lc_war"] { gui.set_enabled(p.window, name, expansion); }
        gui.set_button_pressed(p.window, "lc_toggle", self.land_control);
        gui.set_visible(p.window, "lc_info", self.land_control && p.ground.as_ref().is_some_and(|g| !g.rooms));
        p.lc_poll -= dt;
        if self.land_control && p.lc_poll <= 0.0 {
            p.lc_poll = 0.5;
            if p.district != zone.lc_district {
                p.district.clone_from(&zone.lc_district);
                let text = p.district.as_ref().filter(|(_, (min, max))| *min > 0 && *max > 0)
                    .map_or_else(String::new, |(name, (min, max))| format!("{}<br>Lvl Rng: {min} - {max}", super::hud_listview::esc(name)));
                gui.set_text(p.window, "lc_info", &text);
            }
        }
        if !p.available {
            return;
        }
        let own = zone.own().map(|d| (d.pos, d.yaw));
        let here = own.map(|(pos, _)| [pos[0], pos[2]]);
        if p.followed != here {
            if let Some(h) = here {
                p.center = h;
            }
            p.followed = here;
        }
        let (cw, ch) = gui.canvas_size(p.window, "map");
        let (cw, ch) = (cw as f32, ch as f32);
        let center = p.center;
        let scale = p.ground.as_ref().map_or(PF_SCALE_ROOMS, |g| g.scale);
        let to_screen = |x: f32, z: f32| [cw / 2.0 + (x - center[0]) * scale, ch / 2.0 - (z - center[1]) * scale];
        let mut items = vec![CanvasItem::Solid { dst: [0.0, 0.0, cw, ch], color: 0, alpha: 1.0 }];
        if let Some(g) = p.ground.as_mut() {
            if let Some(h) = here {
                let px = g.map.to_px(h[0], h[1]);
                let o = if px[0] >= 0.0 && px[1] >= 0.0 && (px[0] as u32) < g.map.width && (px[1] as u32) < g.map.height { g.map.owner[px[1] as usize * g.map.width as usize + px[0] as usize] } else { NO_OWNER };
                if g.rooms && o != NO_OWNER {
                    // `FUN_100428e1`: GetCurrentRoom changed -> the old room goes grey, the new one white
                    if g.current != o {
                        g.visited.insert(o);
                        g.current = o;
                    }
                }
            }
            let key = (g.current, g.visited.len());
            let id = match g.shown {
                Some((c, n, id)) if (c, n) == key => id,
                _ => {
                    let id = gui.add_image("playfield map", lit(g), g.map.width, g.map.height, true);
                    g.shown = Some((key.0, key.1, id));
                    id
                }
            };
            let a = to_screen(g.map.origin[0], g.map.origin[1]);
            let (w, h) = (g.map.width as f32 * g.map.mpp * scale, g.map.height as f32 * g.map.mpp * scale);
            items.push(CanvasItem::Image { id, src: [0.0, 0.0, g.map.width as f32, g.map.height as f32], dst: [a[0], a[1], a[0] + w, a[1] + h], alpha: 1.0 });
        }
        if self.land_control && p.ground.as_ref().is_some_and(|g| !g.rooms) {
            if let Some(land) = &p.land {
                let image = match p.land_image {
                    Some((owns, image)) if owns == p.owns => image,
                    _ => {
                        let image = gui.add_image("land-control map", land_rgba(land, p.owns), land.width, land.height, false);
                        p.land_image = Some((p.owns, image));
                        image
                    }
                };
                let cell = PF_PX_PER_CELL / scale;
                let top_left = to_screen(0.0, land.height as f32 * cell);
                items.push(CanvasItem::Image { id: image, src: [0.0, 0.0, land.width as f32, land.height as f32], dst: [top_left[0], top_left[1], top_left[0] + land.width as f32 * PF_PX_PER_CELL, top_left[1] + land.height as f32 * PF_PX_PER_CELL], alpha: 1.0 });
                if !p.owns {
                    //100e9ebb's64x64 mask: one opaque green row/column, map-locked.
                    let origin = to_screen(0.0, 0.0);
                    let mut x = origin[0].rem_euclid(64.0);
                    while x < cw { items.push(CanvasItem::Solid { dst: [x, 0.0, x + 1.0, ch], color: 0x00ff00, alpha: 1.0 }); x += 64.0; }
                    let mut y = origin[1].rem_euclid(64.0);
                    while y < ch { items.push(CanvasItem::Solid { dst: [0.0, y, cw, y + 1.0], color: 0x00ff00, alpha: 1.0 }); y += 64.0; }
                }
            }
        }
        if let Some((pos, yaw)) = own {
            // other characters (`FUN_100425eb` list, drawn by `FUN_100ea626` only when `GetMapCharacters` succeeded = `owns`, true here): 4x4 squares (`MapSquare` art), range 500
            for (id, d) in &zone.dynels {
                if !p.owns || *id == zone.char_id as i32 {
                    continue;
                }
                let (dx, dz) = (d.pos[0] - pos[0], d.pos[2] - pos[2]);
                if dx * dx + dz * dz > DOT_RANGE * DOT_RANGE {
                    continue;
                }
                // UNRESOLVED: the original's choice among MAPSQUARE_CLAN/OMNI/NEUTRAL/MONSTER/TEAMMEMBER/SHOP (by art name and `Side`)
                let name = match (d.npc, d.side) {
                    (true, _) => "GFX_GUI_MAPSQUARE_MONSTER",
                    (_, 1) => "GFX_GUI_MAPSQUARE_CLAN",
                    (_, 2) => "GFX_GUI_MAPSQUARE_OMNI",
                    _ => "GFX_GUI_MAPSQUARE_NEUTRAL",
                };
                if let Some(g) = gui.gfx().id(name) {
                    let s = to_screen(d.pos[0], d.pos[2]);
                    items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, 4.0, 4.0], dst: [s[0].floor() - 2.0, s[1].floor() - 2.0, s[0].floor() + 2.0, s[1].floor() + 2.0], alpha: 1.0 });
                }
            }
            let s = to_screen(pos[0], pos[2]);
            // GUI100eb216→100e92ef: MapData+81 selects the arrow, set unconditionally
            // by100425eb after the first map-character poll; MapNavigation is not read.
            if let Some(id) = p.arrow(gui, yaw.unwrap_or(0.0)) {
                items.push(CanvasItem::Image { id, src: [0.0, 0.0, 16.0, 16.0], dst: [s[0].floor() - 8.0, s[1].floor() - 8.0, s[0].floor() + 8.0, s[1].floor() + 8.0], alpha: 1.0 });
            }
        }
        gui.set_canvas(p.window, "map", items);
    }
}

/// GUI100eb328, TextureFormat10=A4R4G4B4 (Randy100262dc), flipped bitmap rows.
fn land_rgba(map: &LandControlMap, owns: bool) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(map.width as usize * map.height as usize * 4);
    for y in (0..map.height).rev() {
        for x in 0..map.width {
            let pixel = if map.get(x, y) { [0x33, 0x99, 0x22, 0xff] } else if owns { [0, 0, 0, 0x44] } else { [0x55, 0x55, 0x55, 0xff] };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}

fn button_rect(i: usize, width: f32) -> (f32, f32) {
    (width / 2.0 - 27.0 + (i % 2) as f32 * 27.0, 27.0)
}
fn set_button_tips(gui: &mut Gui, window: WindowId, texts: Option<&TextDb>) {
    let width = gui.canvas_size(window, "buttons").0 as f32;
    let tips = BUTTONS.iter().enumerate().filter_map(|(i, b)| {
        let title = texts?.by_key(ao_formats::screens::CAT_GUI, b.3)?;
        let (left, width) = button_rect(i, width);
        let top = (i / 2) as f32 * 27.0;
        Some(CanvasTip { rect: [left, top, left + width, top + 27.0], title, body: String::new() })
    }).collect();
    gui.set_canvas_tips(window, "buttons", tips);
}


fn button_index_at(x: f32, y: f32, width: f32) -> Option<usize> {
    (0..BUTTONS.len()).find(|&i| {
        let (l, w) = button_rect(i, width);
        let top = (i / 2) as f32 * 27.0;
        x >= l && x < l + w && y >= top && y < top + 27.0
    })
}

impl Planet {
    /// The mission marker on the current level (`None`: no mission, or its playfield has no coordinates entry).
    fn mission_marker(&self, mission: Option<(u32, [f32; 2])>) -> Option<[f32; 2]> {
        let (pf, pos) = mission?;
        let map = self.map.as_ref()?;
        Some(map.index.levels[self.level].locate(map.coords.get(&pf)?, pos[0], pos[1]))
    }

    /// Double-click zoom: like [`Planet::zoom`], but the clicked point (`click` in canvas pixels of a view `size`) becomes the middle of the new level
    /// (`FUN_1004b422` stores the point, `FUN_1004bc1f` scrolls to it after the zoom).
    fn zoom_at(&mut self, by: i32, size: (u32, u32), click: [f32; 2]) {
        let before = self.level;
        let anchor = [self.center[0] + click[0] - size.0 as f32 / 2.0, self.center[1] + click[1] - size.1 as f32 / 2.0];
        self.center = anchor;
        self.zoom(by);
        if self.level != before {
            self.hold = true;
        } else {
            self.center = [anchor[0] - click[0] + size.0 as f32 / 2.0, anchor[1] - click[1] + size.1 as f32 / 2.0];
        }
    }

    /// ZoomIn / ZoomOut (`FUN_1004bc1f`): the level is clamped to `0..levels`; the shown map point stays in the middle.
    fn zoom(&mut self, by: i32) {
        let Some(map) = self.map.as_ref() else { return };
        let to = (self.level as i32 + by).clamp(0, map.index.levels.len() as i32 - 1) as usize;
        if to == self.level {
            return;
        }
        let (a, b) = (&map.index.levels[self.level], &map.index.levels[to]);
        let f = |c: f32, la: u32, ra: u32, lb: u32, rb: u32| lb as f32 + (c - la as f32) / (ra - la).max(1) as f32 * (rb - lb) as f32;
        self.center = [f(self.center[0], a.rect[0], a.rect[2], b.rect[0], b.rect[2]), f(self.center[1], a.rect[1], a.rect[3], b.rect[1], b.rect[3])];
        self.level = to;
        self.followed = None;
    }
}

impl Pf {
    /// `GFX_GUI_MAP_BIG_ARROW` turned to the heading, 16 steps (cached).
    fn arrow(&mut self, gui: &mut Gui, yaw: f32) -> Option<GfxId> {
        let step = ((yaw.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 16.0).round() as u32 % 16) as u8;
        if let Some(id) = self.arrows.get(&step) {
            return Some(*id);
        }
        let base = gui.gfx().id("GFX_GUI_MAP_BIG_ARROW")?;
        let img = gui.gfx().image(base)?;
        let (w, h) = (img.width as i32, img.height as i32);
        let a = step as f32 / 16.0 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let mut out = vec![0u8; img.rgba.len()];
        for y in 0..h {
            for x in 0..w {
                // inverse rotation of the destination pixel centre about the image centre (art points up at heading 0)
                let (dx, dy) = (x as f32 + 0.5 - w as f32 / 2.0, y as f32 + 0.5 - h as f32 / 2.0);
                let (sx, sy) = (dx * c + dy * s + w as f32 / 2.0, -dx * s + dy * c + h as f32 / 2.0);
                let (sx, sy) = (sx.floor() as i32, sy.floor() as i32);
                if (0..w).contains(&sx) && (0..h).contains(&sy) {
                    let (si, di) = (((sy * w + sx) * 4) as usize, ((y * w + x) * 4) as usize);
                    out[di..di + 4].copy_from_slice(&img.rgba[si..si + 4]);
                }
            }
        }
        let id = gui.add_image("map arrow", out, w as u32, h as u32, false);
        self.arrows.insert(step, id);
        Some(id)
    }
}

/// The playfield image with the rooms' exploration lighting applied.
fn lit(g: &Ground) -> Vec<u8> {
    let mut rgba = g.map.rgba.clone();
    if !g.rooms {
        return rgba;
    }
    for (px, o) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(&g.map.owner) {
        if *o == NO_OWNER {
            continue;
        }
        let k = if *o == g.current {
            256
        } else if g.visited.contains(o) {
            128
        } else {
            0
        };
        for c in &mut px[..3] {
            *c = (*c as u32 * k / 256) as u8;
        }
    }
    rgba
}

/// Rubi-Ka map unless the playfield is only known to the Shadowlands coordinates (`Type Rubika|Shadowlands` of the index files;
/// UNRESOLVED: the client's own switch between `PlanetMapIndexFile` and `ShadowlandMapIndexFile`).
fn pick_planet_map(client: &std::path::Path, pf: Option<u32>) -> anyhow::Result<PlanetMap> {
    let rubika = PlanetMap::load(client, RUBIKA_INDEX)?;
    if let Some(pf) = pf.filter(|pf| !rubika.coords.contains_key(pf)) {
        if let Ok(sl) = PlanetMap::load(client, SHADOWLANDS_INDEX) {
            if sl.coords.contains_key(&pf) {
                return Ok(sl);
            }
        }
    }
    Ok(rubika)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn land_fixture() -> LandControlMap {
        let mut bytes = vec![1, 0, 0, 0, 2, 0, 2, 0];
        bytes.resize(20, 0);
        bytes.extend([0b01, 0b10]);
        landcontrol::parse(&bytes).unwrap()
    }

    #[test]
    fn native_land_control_texels_and_flipped_rows() {
        let map = land_fixture();
        let green = [0x33, 0x99, 0x22, 0xff];
        let dim = [0, 0, 0, 0x44];
        let grey = [0x55, 0x55, 0x55, 0xff];
        assert_eq!(land_rgba(&map, true), [dim, green, green, dim].concat());
        assert_eq!(land_rgba(&map, false), [grey, green, green, grey].concat());
    }

    #[test]
    fn land_control_controls_availability_district_poll_and_waypoint_signals() {
        let dir = ao_gui::client_dir();
        let Ok(mut gui) = Gui::new(&dir, None) else { return };
        let mut rollup = Rollup::new(&dir, (1024, 768));
        let mut maps = HudMap::new(&dir);
        let mut zone = Zone::new(7);
        zone.playfield = Some(4001); // native map ownership requires area bit79
        zone.lc_district = Some(("Authored Field".into(), (10, 20)));
        maps.open(&mut gui, &mut rollup, WindowKind::Map);
        maps.provide_ground(4001, GroundMap { width: 2, height: 2, rgba: vec![255; 16], origin: [0.0, 2.0], mpp: 1.0, owner: vec![0; 4] }, Some(1.0));
        maps.update(&mut gui, &zone, 0.0);
        let window = maps.pf.as_ref().unwrap().window;
        assert!(!maps.pf_buttons && !maps.land_control);
        assert!(!maps.pf.as_ref().unwrap().available);
        maps.pf.as_mut().unwrap().land = Some(land_fixture());
        let click = |name: &str| Event::Clicked { window, view: name.into(), item: None };
        assert!(!maps.event(&mut gui, &click("lc_toggle"), &zone, (1024, 768)));
        assert!(!maps.land_control, "NotumWars bit gates nativeInfo/Build/War controls");
        zone.stats.insert(389, 1);
        assert!(maps.event(&mut gui, &Event::MenuPicked { id: MENU_PF_BUTTONS }, &zone, (1024, 768)));
        assert!(maps.event(&mut gui, &click("lc_toggle"), &zone, (1024, 768)));
        maps.update(&mut gui, &zone, 0.0);
        let p = maps.pf.as_ref().unwrap();
        assert!(p.available && !p.owns, "LC bitmap makes an unowned map available");
        let image = p.land_image.unwrap().1;
        assert!(gui.canvas_items(window, "map").iter().any(|item| matches!(item, CanvasItem::Image { id, .. } if *id == image)));
        assert_eq!(gui.text(window, "lc_info"), "Authored Field<br>Lvl Rng: 10 - 20");
        zone.lc_district = Some(("Next Field".into(), (20, 30)));
        maps.update(&mut gui, &zone, 0.1);
        assert!(gui.text(window, "lc_info").starts_with("Authored Field"));
        maps.update(&mut gui, &zone, 0.5);
        assert!(gui.text(window, "lc_info").starts_with("Next Field"));
        assert_eq!(maps.pf.as_ref().unwrap().land_image.unwrap().1, image, "static bitmap upload is cached");
        for name in ["lc_build", "lc_war"] {
            assert!(maps.event(&mut gui, &click(name), &zone, (1024, 768)));
            assert!(maps.land_control, "native unobserved170 signal does not toggleInfo");
        }
        gui.set_text(window, "pf_x", " 5.5suffix");
        gui.set_text(window, "pf_z", "7.25");
        assert!(maps.event(&mut gui, &click("pf_add_waypoint"), &zone, (1024, 768)));
        assert_eq!(maps.take_waypoint(), Some(Some((4001, [5.5, 7.25]))));
        gui.set_text(window, "pf_x", "0");
        maps.event(&mut gui, &click("pf_add_waypoint"), &zone, (1024, 768));
        assert_eq!(maps.take_lines(), [("Coordinates not in range.".into(), 0x52)]);
        maps.event(&mut gui, &click("pf_remove_waypoint"), &zone, (1024, 768));
        assert_eq!(maps.take_waypoint(), Some(None));
        maps.event(&mut gui, &click("lc_toggle"), &zone, (1024, 768));
        maps.update(&mut gui, &zone, 0.0);
        assert!(!maps.pf.as_ref().unwrap().available);
    }

    #[test]
    fn native_map_flags_roundtrip_without_losing_window_fields() {
        use super::super::dvalue::{DValues, Variant, CAT_CHAR};
        let dir = ao_gui::client_dir();
        let Ok(mut gui) = Gui::new(&dir, None) else { return };
        let mut d = DValues::default();
        d.add("PlanetMapViewConfig", Variant::Archive("<Archive name=\"PlanetMapViewConfig\"><Bool name=\"ShowButtons\" value=\"true\"/><Int32 name=\"ZoomLev\" value=\"2\"/></Archive>".into()), true, CAT_CHAR, None, None, false);
        d.add("PFMapWindowConfig", Variant::Archive("<Archive name=\"PFMapWindowConfig\"><Bool name=\"ShowButtons\" value=\"true\"/><Bool name=\"RenderLandControlData\" value=\"true\"/><Rect name=\"WindowFrame\" value=\"Rect(1,2,300,400)\"/></Archive>".into()), true, CAT_CHAR, None, None, false);
        let mut maps = HudMap::new(&dir);
        maps.sync_config(&mut gui, &mut d);
        assert!(maps.planet_buttons && maps.pf_buttons && maps.land_control);
        maps.toggle_planet_buttons(&mut gui);
        maps.sync_config(&mut gui, &mut d);
        let mut restored = HudMap::new(&dir);
        restored.sync_config(&mut gui, &mut d);
        assert!(!restored.planet_buttons && restored.pf_buttons && restored.land_control);
        assert!(matches!(d.get("PFMapWindowConfig"), Some(Variant::Archive(xml)) if xml.contains("WindowFrame")));
        assert!(matches!(d.get("PlanetMapViewConfig"), Some(Variant::Archive(xml)) if xml.contains("ZoomLev")));
    }

    #[test]
    fn map_viewports_grow_in_both_axes_and_buttons_follow_width() {
        let dir = ao_gui::client_dir();
        let Ok(mut gui) = Gui::new(&dir, None) else { return };
        let mut rollup = Rollup::new(&dir, (1280, 800));
        let mut maps = HudMap::new(&dir);
        maps.open(&mut gui, &mut rollup, WindowKind::PlanetMap);
        assert!(!maps.planet_buttons, "native ShowButtons defaults false");
        maps.toggle_planet_buttons(&mut gui);
        let window = maps.planet.as_ref().unwrap().window;
        gui.set_window_preferred_size_limits(window, true);
        let before = gui.canvas_size(window, "map");
        gui.resize_window(window, WindowSize::Fixed(500, 600));
        let after = gui.canvas_size(window, "map");
        assert_eq!((after.0 - before.0, after.1 - before.1), (200, 200));
        assert_eq!(gui.canvas_size(window, "buttons"), (500, BUTTONS_H));
        for width in [300.0, 500.0] {
            for i in 0..4 {
                let (left, size) = button_rect(i, width);
                assert_eq!(button_index_at(left + size / 2.0, (i / 2) as f32 * 27.0 + 13.0, width), Some(i));
            }
        }
        gui.resize_window(window, WindowSize::Fixed(100, 100));
        let small = gui.canvas_size(window, "map");
        assert!(small.0 <= 100 && small.1 <= 100, "constructor dimensions are not native viewport minima: {small:?}");
    }

    use crate::play::zone::DynelState;
    use ao_formats::screens::TextDb;
    use ao_gui::{DrawList, InputEvent};
    use ao_render::{Frontend, Host, Offscreen};
    
    struct Shot {
        gui: Gui,
        map: HudMap,
        rollup: Rollup,
        zone: Zone,
    }

    impl Frontend for Shot {
        fn gui(&self) -> &Gui {
            &self.gui
        }
        fn input(&mut self, ev: InputEvent, _host: &mut Host) {
            for e in self.gui.input(ev) {
                self.map.event(&mut self.gui, &e, &self.zone, (640, 600));
            }
        }
        fn frame(&mut self, dt: f32, _size: (u32, u32), _host: &mut Host) -> DrawList {
            self.map.update(&mut self.gui, &self.zone, dt);
            self.gui.frame(dt)
        }
    }

    fn shot() -> Option<(Shot, Offscreen)> {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/textures/PlanetMap").exists() {
            eprintln!("skipping: no client");
            return None;
        }
        let labels = TextDb::load(&dir).unwrap();
        let gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let s = Shot { gui, map: HudMap::new(&dir), rollup: Rollup::new(&dir, (640, 600)), zone: Zone::default() };
        let o = Offscreen::new(&s, (640, 600)).unwrap();
        Some((s, o))
    }

    fn png(s: &mut Shot, o: &mut Offscreen, name: &str) {
        let mut list = DrawList::default();
        for _ in 0..4 {
            list = o.frame(s, 0.016);
        }
        if let Some(dir) = std::env::var_os("AOMAC_SHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            o.png(s, &list, &std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    }

    fn own_at(s: &mut Shot, playfield: u32, pos: [f32; 3], yaw: f32) {
        s.zone.char_id = 7;
        s.zone.playfield = Some(playfield);
        s.zone.dynels.insert(7, DynelState { name: "Testy".into(), pos, yaw: Some(yaw), npc: false, side: 0, level: 1, health: 1, max_health: 1 });
    }

    /// What the world loader does (`Play::start_world_load`): decode the playfield once, render the ground image from that scene and hand it over.
    fn provide_ground(s: &mut Shot, pf: u32) {
        let dir = ao_gui::client_dir();
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let (scene, report) = ao_formats::playfield::load_playfield_report(&store, &dir, pf).unwrap();
        let (map, cell) = ground_map(&scene, &report).expect("no ground map");
        s.map.provide_ground(pf, map, cell);
    }

    /// Plays frames until the map window took the ground image.
    fn wait_ground(s: &mut Shot, o: &mut Offscreen) {
        for _ in 0..3 {
            o.frame(s, 0.016);
        }
        assert!(s.map.pf.as_ref().is_some_and(|p| p.ground.is_some()), "ground image not taken");
    }

    /// World position of an opaque ground pixel near the middle of the opaque area.
    fn some_floor(s: &Shot) -> [f32; 3] {
        let g = &s.map.pf.as_ref().unwrap().ground.as_ref().expect("no ground map").map;
        let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
        for (i, a) in g.rgba.as_chunks::<4>().0.iter().enumerate() {
            if a[3] != 0 {
                sx += i as u64 % g.width as u64;
                sy += i as u64 / g.width as u64;
                n += 1;
            }
        }
        // the opaque pixel closest to the centroid
        let (cx, cy) = ((sx / n) as i64, (sy / n) as i64);
        let best = g.rgba.as_chunks::<4>().0.iter().enumerate().filter(|(_, a)| a[3] != 0).map(|(i, _)| (i as i64 % g.width as i64, i as i64 / g.width as i64)).min_by_key(|&(x, y)| (x - cx).pow(2) + (y - cy).pow(2)).unwrap();
        [g.origin[0] + (best.0 as f32 + 0.5) * g.mpp, 0.0, g.origin[1] - (best.1 as f32 + 0.5) * g.mpp]
    }

    #[test]
    fn playfield_maps() {
        for (pf, tag) in [(4604u32, "arrival-hall"), (4582, "icc-shuttleport"), (566, "newland-city")] {
            let Some((mut s, mut o)) = shot() else { return };
            s.map.open(&mut s.gui, &mut s.rollup, WindowKind::Map);
            provide_ground(&mut s, pf);
            own_at(&mut s, pf, [0.0; 3], 0.0);
            wait_ground(&mut s, &mut o);
            let g = s.map.pf.as_ref().unwrap().ground.as_ref().unwrap();
            // 4 px per 4 m terrain cell of Newland City; the dungeons keep the unresolved room scale
            assert_eq!(g.scale, if pf == 566 { 1.0 } else { PF_SCALE_ROOMS }, "{tag}");
            let pos = some_floor(&s);
            own_at(&mut s, pf, pos, 0.6);
            png(&mut s, &mut o, &format!("map-{tag}"));
            // Both map-navigation values use the native heading marker after polling.
            s.zone.stats.insert(140, 1);
            png(&mut s, &mut o, &format!("map-{tag}-upgraded"));
        }
    }

    /// `FUN_100eacda`: a map the character does not own is replaced by the "Map Not Available" text (area 79 = bit 15 of `MapAreaPart3`).
    #[test]
    fn unowned_map_shows_not_available() {
        let Some((mut s, mut o)) = shot() else { return };
        s.map.open(&mut s.gui, &mut s.rollup, WindowKind::Map);
        own_at(&mut s, 4001, [10.0, 0.0, 10.0], 0.0);
        png(&mut s, &mut o, "map-not-available");
        assert!(!s.map.pf.as_ref().unwrap().available);
        s.zone.stats.insert(585, 1 << 15);
        png(&mut s, &mut o, "map-owned-by-area-bit");
        assert!(s.map.pf.as_ref().unwrap().available);
    }

    #[test]
    fn own_heading_arrow_does_not_require_map_navigation() {
        let Some((mut s, _)) = shot() else { return };
        own_at(&mut s, 4582, [10.0, 0.0, 10.0], 0.0);
        s.map.open(&mut s.gui, &mut s.rollup, WindowKind::Map);
        for navigation in [0, 1, 100] {
            s.zone.stats.insert(140, navigation);
            s.map.update(&mut s.gui, &s.zone, 0.0);
            let p = s.map.pf.as_ref().unwrap();
            assert!(!p.arrows.is_empty(), "MapData+81 is set even at MapNavigation={navigation}");
            assert!(s.gui.canvas_items(p.window, "map").iter().any(|item| matches!(item, CanvasItem::Image { id, .. } if p.arrows.values().any(|arrow| arrow == id))));
        }
    }

    /// An unmapped playfield leaves the freshly constructed BitmapTileView at scroll zero.
    #[test]
    fn planet_map_unmapped_playfield_starts_at_zero_scroll() {
        let Some((mut s, mut o)) = shot() else { return };
        s.map.open(&mut s.gui, &mut s.rollup, WindowKind::PlanetMap);
        own_at(&mut s, 4582, [10.0, 0.0, 10.0], 0.0);
        png(&mut s, &mut o, "planet-4582-zero-scroll");
        let p = s.map.planet.as_ref().unwrap();
        let (w, h) = s.gui.canvas_size(p.window, "map");
        assert_eq!(p.center, [w as f32 / 2.0, h as f32 / 2.0]);
    }

    #[test]
    fn planet_map_follows_the_character_and_zooms() {
        let Some((mut s, mut o)) = shot() else { return };
        s.map.open(&mut s.gui, &mut s.rollup, WindowKind::PlanetMap);
        s.map.toggle_planet_buttons(&mut s.gui);
        // Newland City, 100 m east of its origin
        own_at(&mut s, 566, [100.0, 0.0, 200.0], 0.0);
        png(&mut s, &mut o, "planet-566-level0");
        let p = s.map.planet.as_ref().unwrap();
        let lv = &p.map.as_ref().unwrap().index.levels[0];
        let m = lv.locate(&p.map.as_ref().unwrap().coords[&566], 100.0, 200.0);
        assert_eq!(p.center, m);
        // the zoom-in button (first of the row), then drag the map: the view moves, the follow state is kept until the character moves
        let w = s.map.planet.as_ref().unwrap().window;
        let r = s.gui.view_rect(w, "buttons").unwrap();
        let (x, y) = (r.l + button_rect(0, r.width() + 1.0).0 + 13.0, r.t + 13.0);
        s.input(InputEvent::MouseMove { x, y }, &mut o.host);
        s.input(InputEvent::MouseDown { x, y, button: MouseButton::Left }, &mut o.host);
        s.input(InputEvent::MouseUp { x, y, button: MouseButton::Left }, &mut o.host);
        assert_eq!(s.map.planet.as_ref().unwrap().level, 1);
        png(&mut s, &mut o, "planet-566-level1");
        let c = s.map.planet.as_ref().unwrap().center;
        let mr = s.gui.view_rect(w, "map").unwrap();
        let (x, y) = (mr.l + 100.0, mr.t + 100.0);
        s.input(InputEvent::MouseMove { x, y }, &mut o.host);
        s.input(InputEvent::MouseDown { x, y, button: MouseButton::Left }, &mut o.host);
        s.input(InputEvent::MouseMove { x: x + 30.0, y: y + 10.0 }, &mut o.host);
        s.input(InputEvent::MouseUp { x: x + 30.0, y: y + 10.0, button: MouseButton::Left }, &mut o.host);
        let d = s.map.planet.as_ref().unwrap().center;
        assert_eq!([d[0] - c[0], d[1] - c[1]], [-30.0, -10.0]);
        // an indoor playfield has no entry in the coordinates: no marker, the view keeps its place
        own_at(&mut s, 4604, [10.0, 0.0, 10.0], 0.0);
        png(&mut s, &mut o, "planet-4604-no-marker");
        // Shadowlands playfield: the Shadowlands map is picked
        let some_sl = ao_formats::planetmap::PlanetMap::load(&ao_gui::client_dir(), SHADOWLANDS_INDEX).unwrap().coords.keys().copied().find(|k| !ao_formats::planetmap::PlanetMap::load(&ao_gui::client_dir(), RUBIKA_INDEX).unwrap().coords.contains_key(k));
        if let Some(id) = some_sl {
            own_at(&mut s, id, [100.0, 0.0, 100.0], 0.0);
            png(&mut s, &mut o, "planet-shadowlands");
            assert_eq!(s.map.planet.as_ref().unwrap().map.as_ref().unwrap().index.kind, "Shadowlands");
        }
    }

    fn press(s: &mut Shot, o: &mut Offscreen, x: f32, y: f32, b: MouseButton) {
        s.input(InputEvent::MouseMove { x, y }, &mut o.host);
        s.input(InputEvent::MouseDown { x, y, button: b }, &mut o.host);
    }

    fn release(s: &mut Shot, o: &mut Offscreen, x: f32, y: f32) {
        s.input(InputEvent::MouseUp { x, y, button: MouseButton::Left }, &mut o.host);
    }

    /// Tooltips, pressed art, double-click zoom and the mission button of the planet map window.
    #[test]
    fn planet_map_buttons_tooltips_pressed_art_double_click_and_mission() {
        let Some((mut s, mut o)) = shot() else { return };
        s.map.open(&mut s.gui, &mut s.rollup, WindowKind::PlanetMap);
        s.map.toggle_planet_buttons(&mut s.gui);
        s.gui.set_screen_size(640, 600);
        own_at(&mut s, 566, [100.0, 0.0, 200.0], 0.0);
        png(&mut s, &mut o, "planet-window-title");
        let w = s.map.planet.as_ref().unwrap().window;
        let br = s.gui.view_rect(w, "buttons").unwrap();
        let on_button = |i: usize| {
            let (l, bw) = button_rect(i, br.width() + 1.0);
            (br.l + l + bw / 2.0, br.t + (i / 2) as f32 * 27.0 + 13.0)
        };
        // tooltips: texts.mdb 10000 `Click2ZoomEtc` .. `CenterOnMission`, shown after the pointer rests 500 ms
        for (i, text) in [(0, "Click to zoom in or double click left button directly on map."), (1, "Click to zoom out or double click right button directly on map."), (2, "Center view on your character."), (3, "Center view on mission marker.")] {
            let (x, y) = on_button(i);
            s.input(InputEvent::MouseMove { x, y }, &mut o.host);
            s.gui.frame(0.6);
            assert_eq!(s.gui.tooltip_shown().map(|t| t.0.to_string()), Some(text.to_string()), "button {i}");
        }
        png(&mut s, &mut o, "planet-tooltip-mission");
        // pressed art while the button is held (and the pointer over it), the normal art again after the release
        let (zx, zy) = on_button(0);
        let art = |s: &Shot, name: &str| s.gui.gfx().id(name).unwrap();
        let has = |s: &mut Shot, o: &mut Offscreen, gfx: GfxId| {
            o.frame(s, 0.016);
            s.gui.canvas_items(w, "buttons").iter().any(|i| matches!(i, CanvasItem::Image { id, .. } if *id == gfx))
        };
        let (normal, down) = (art(&s, "GFX_GUI_PLANETMAP_ZOOM_IN"), art(&s, "GFX_GUI_PLANETMAP_ZOOM_IN_PRESSED"));
        assert!(has(&mut s, &mut o, normal) && !has(&mut s, &mut o, down));
        press(&mut s, &mut o, zx, zy, MouseButton::Left);
        assert!(has(&mut s, &mut o, down) && !has(&mut s, &mut o, normal));
        png(&mut s, &mut o, "planet-zoom-in-pressed");
        // dragging off the button lifts it, the release ends the press
        s.input(InputEvent::MouseMove { x: zx + 200.0, y: zy }, &mut o.host);
        assert!(has(&mut s, &mut o, normal));
        release(&mut s, &mut o, zx + 200.0, zy);
        assert!(has(&mut s, &mut o, normal) && s.map.planet.as_ref().unwrap().pressed.is_none());
        // double click on the map: left zooms in (around the clicked point), right zooms out
        let mr = s.gui.view_rect(w, "map").unwrap();
        let (x, y) = (mr.l + 60.0, mr.t + 50.0);
        let level = |s: &Shot| s.map.planet.as_ref().unwrap().level;
        let before = level(&s);
        press(&mut s, &mut o, x, y, MouseButton::Left);
        release(&mut s, &mut o, x, y);
        assert_eq!(level(&s), before, "one click does not zoom");
        press(&mut s, &mut o, x, y, MouseButton::Left);
        release(&mut s, &mut o, x, y);
        assert_eq!(level(&s), before + 1);
        // the clicked map point is now in the middle of the view and the view stays there (no re-centring on the character)
        let c = s.map.planet.as_ref().unwrap().center;
        png(&mut s, &mut o, "planet-double-click-zoomed");
        assert_eq!(s.map.planet.as_ref().unwrap().center, c);
        s.gui.frame(1.0);
        s.input(InputEvent::MouseDown { x, y, button: MouseButton::Right }, &mut o.host);
        s.input(InputEvent::MouseDown { x, y, button: MouseButton::Right }, &mut o.host);
        assert_eq!(level(&s), before);
        // the mission button: disabled without a marker, centres on it once a mission is set
        let mission = (566u32, [150.0f32, 260.0f32]);
        s.map.set_mission(Some(mission));
        o.frame(&mut s, 0.016);
        let want = {
            let p = s.map.planet.as_ref().unwrap();
            let m = p.map.as_ref().unwrap();
            m.index.levels[p.level].locate(&m.coords[&566], 150.0, 260.0)
        };
        let (qx, qy) = on_button(3);
        press(&mut s, &mut o, qx, qy, MouseButton::Left);
        release(&mut s, &mut o, qx, qy);
        o.frame(&mut s, 0.016);
        assert_eq!(s.map.planet.as_ref().unwrap().center, want);
        png(&mut s, &mut o, "planet-mission-centred");
    }
}
