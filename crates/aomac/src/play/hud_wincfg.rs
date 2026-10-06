//! Persistence of the HUD windows' place: `Window::SaveWndConfig` / `LoadWndConfig` (GUI.dll 0x1548a7 / 0x10154d6e, docs/gui.md §6.5).
//!
//! `SaveWndConfig(msg)` adds `WindowFrame` (`GetFrame`, a `Rect` of inclusive screen coordinates) and, unless the window flag 0x800 is set,
//! `WindowPinButtonState` (`WndBorder::GetPinButtonState`); `LoadWndConfig(msg)` is `FindRect("WindowFrame")` -> `WndBorder::SetClientFrame`,
//! `MoveInsideScreen(false, true, true)` and `FindBool("WindowPinButtonState")` -> `SetPinButtonState`. Where the message lives depends on the window:
//!
//! * a plain `Window` with its own DValue archive: `SkillConfig` (`FUN_100fc18e`), `PerkWindowConfig` (`FUN_1006372d`) -> [`Store::Value`];
//! * a `DockableView_c` (inventory, team, NCU, missions, maps, faction, ...) lives in a `DockWindow_c` (`FUN_1003bdd4`, a `DockTabbedWindow`) whose dtor
//!   (`FUN_1003b672`) adds `selected_tab` (`Window::GetTabSelection`) to the same message; the `DockingController_c` (`FUN_1003a0a1`) writes every dock as
//!   `<CharPrefsPath>/DockAreas/<dock_name>.xml` = `{dock_name, dock_type, dock_config}` (`FUN_1003a005` builds the directory, `FUN_1003a5e6` reads every file
//!   of it back) -> [`Store::Dock`]. The dock's `docked_view_identities` is the dvalue name of the view (`inventory_window`, `team_view`, ...).
//!
//! The wear / stat / nano / friends pages are `RollupArea` pages (`hud_rollup.rs`); the `CharBarWindow_c`, compass and shortcut bar windows are created
//! with the window flag 0x8 (not movable: flags 0xe3c / 0xd3c / 0x183c, `WndBorder::HitTest` 0x101593d6), so their frame never changes.
//!
//! Plain windows are tracked by [`WinCfgs::update`]. Dock windows are saved by the controller through
//! [`WinCfgs::save_docks`], retaining ordered identities, selected tabs and rollup node configuration.
//! Template dock frames are not applied on first login; character-owned frames are restored.

use std::path::{Path, PathBuf};

use ao_gui::xml::{self, Element};
use ao_gui::{Gui, WindowId};

use super::dvalue::{element_xml, DValues, Variant, CAT_CHAR};
use super::hud::WindowKind;

/// What `SaveWndConfig` writes (plus the dock's `selected_tab`): the outer frame as `Rect(l, t, r, b)`, the pin and the tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Cfg {
    pub frame: Option<[f32; 4]>,
    pub pin: Option<bool>,
    pub tab: Option<i64>,
}

/// A controller-owned dock, including closed views retained for subsequent opens.
#[derive(Clone, Debug)]
pub(super) struct DockState {
    pub name: String,
    pub identities: Vec<String>,
    pub selected: i64,
    pub frame: Option<(i32, i32, u32, u32)>,
    pub pin: bool,
    pub nodes: Vec<Element>,
    pub scroll: Option<f32>,
}

/// Where a window's message is kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Store {
    /// A DValue archive of the character prefs (`SkillConfig`).
    Value(&'static str),
    /// `DockAreas/<dock_name>.xml` of the dock holding this view identity.
    Dock(&'static str),
}

/// The store of a window kind; `None` for the rollup pages, the options window (own archive, `options.rs`) and the fixed windows.
pub(super) fn store_of(kind: WindowKind) -> Option<Store> {
    Some(match kind {
        WindowKind::Skills => Store::Value("SkillConfig"),
        WindowKind::Perks => Store::Value("PerkWindowConfig"),
        WindowKind::Pet => Store::Value("PetWindowConfig"),
        WindowKind::Inventory | WindowKind::Team | WindowKind::Ncu | WindowKind::Mission | WindowKind::Map | WindowKind::PlanetMap | WindowKind::Faction | WindowKind::Actions => {
            Store::Dock(kind.dvalue())
        }
        _ => return None,
    })
}

/// `DockWindow_c` creates flags-0x1000 windows without the no-resize flags.
pub(super) fn resizable(kind: WindowKind) -> bool {
    matches!(store_of(kind), Some(Store::Dock(_)))
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// `Rect(l,t,r,b)` of a `Rect` message value.
fn parse_rect(v: &str) -> Option<[f32; 4]> {
    let n: Option<Vec<f32>> = v.trim().strip_prefix("Rect(")?.strip_suffix(')')?.split(',').map(|p| p.trim().parse().ok()).collect();
    <[f32; 4]>::try_from(n?).ok()
}

/// The message children live in the `dock_config` archive of a dock file and directly in a DValue archive.
fn body_index(e: &Element) -> Option<usize> {
    e.children.iter().position(|c| c.name == "Archive" && c.attr("name") == Some("dock_config"))
}

fn child<'a>(e: &'a Element, name: &str) -> Option<&'a str> {
    e.children.iter().find(|c| c.attr("name") == Some(name)).and_then(|c| c.attr("value"))
}

impl Cfg {
    pub fn parse(root: &Element) -> Cfg {
        let e = body_index(root).map_or(root, |i| &root.children[i]);
        Cfg {
            frame: child(e, "WindowFrame").and_then(parse_rect),
            pin: child(e, "WindowPinButtonState").map(|v| v.eq_ignore_ascii_case("true")),
            tab: child(e, "selected_tab").and_then(|v| v.trim().parse().ok()),
        }
    }

    /// Stores the fields into the message children, keeping every other child (`dock_node_configs`, `listview_config`, ...).
    pub fn write_into(&self, root: &mut Element) {
        let i = body_index(root);
        let e = match i {
            Some(i) => &mut root.children[i],
            None => root,
        };
        let mut set = |tag: &str, name: &str, value: String| match e.children.iter_mut().find(|c| c.attr("name") == Some(name)) {
            Some(c) => c.attrs.iter_mut().filter(|(k, _)| k == "value").for_each(|(_, v)| *v = value.clone()),
            None => e.children.push(Element { name: tag.into(), attrs: vec![("name".into(), name.into()), ("value".into(), value)], children: vec![] }),
        };
        if let Some([l, t, r, b]) = self.frame {
            set("Rect", "WindowFrame", format!("Rect({l:.6},{t:.6},{r:.6},{b:.6})"));
        }
        if let Some(p) = self.pin {
            set("Bool", "WindowPinButtonState", p.to_string());
        }
        if let Some(t) = self.tab {
            set("Int32", "selected_tab", t.to_string());
        }
    }
}

/// `Window::MoveInsideScreen(false, true, true)` 0x10154abc on the outer frame `(x, y, w, h)`: shifts the window (never resizes) so that the left / top edge is
/// on screen, else the right / bottom edge is.
pub(super) fn inside_screen((x, y, w, h): (i32, i32, u32, u32), screen: (u32, u32)) -> (i32, i32) {
    let fit = |p: i32, len: u32, max: u32| if p < 0 { 0 } else if p as i64 + len as i64 > max as i64 { max as i32 - len as i32 } else { p };
    (fit(x, w, screen.0), fit(y, h, screen.1))
}

/// The `docked_view_identities` of a dock file: a single `String` for a `DockTabbedWindow`, an `Array` for the rollup.
fn identities(root: &Element) -> Vec<String> {
    let e = body_index(root).map_or(root, |i| &root.children[i]);
    let Some(ids) = e.children.iter().find(|c| c.attr("name") == Some("docked_view_identities")) else { return vec![] };
    match ids.attr("value") {
        Some(v) => vec![unquote(v).to_string()],
        None => ids.children.iter().filter_map(|c| c.attr("value")).map(|v| unquote(v).to_string()).collect(),
    }
}

fn dock_name(root: &Element) -> Option<String> {
    child(root, "dock_name").map(|v| unquote(v).to_string())
}

/// Every `*.xml` dock file of `dir` as `(path, parsed)`.
fn dock_files(dir: &Path) -> Vec<(PathBuf, Element)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "xml"))
        .filter_map(|p| Some((p.clone(), xml::parse(&std::fs::read_to_string(&p).ok()?).ok()?)))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// A dock file: `{dock_config, dock_type, dock_name}` around one view identity (`DockingController_c` 0x1003a0a1 layout of the shipped template).
fn new_dock(name: &str, identity: &str) -> Element {
    let s = |n: &str, v: &str| Element { name: "String".into(), attrs: vec![("name".into(), n.into()), ("value".into(), format!("\"{v}\""))], children: vec![] };
    let archive = |n: Option<&str>, children| {
        let mut attrs = vec![("code".to_string(), "0".to_string())];
        attrs.extend(n.map(|n| ("name".to_string(), n.to_string())));
        Element { name: "Archive".into(), attrs, children }
    };
    let nodes = archive(Some("dock_node_configs"), vec![]);
    archive(None, vec![archive(Some("dock_config"), vec![nodes, s("docked_view_identities", identity)]), s("dock_type", "DockTabbedWindow"), s("dock_name", name)])
}

/// The character's dock file of `identity`, else the template's (`client/prefs/NewChar/DockAreas`); the `bool` tells it is the user's.
fn find_dock(chr: Option<&Path>, client: &Path, identity: &str) -> Option<(PathBuf, Element, bool)> {
    let user = chr.map(|c| dock_files(&c.join("DockAreas"))).unwrap_or_default();
    if let Some((p, e)) = user.into_iter().find(|(_, e)| identities(e).iter().any(|i| i == identity)) {
        return Some((p, e, true));
    }
    let (p, e) = dock_files(&client.join("prefs/NewChar/DockAreas")).into_iter().find(|(_, e)| identities(e).iter().any(|i| i == identity))?;
    Some((p, e, false))
}

/// The file to write for `identity`: the character's own, else the template's name (a copy), else the lowest free `DockArea<N>`
/// (`FUN_1003a26f` counts `DockArea%u` up from 0 until the name is unused).
#[cfg(test)]
fn dock_target(chr: &Path, client: &Path, identity: &str) -> (PathBuf, Element) {
    let dir = chr.join("DockAreas");
    match find_dock(Some(chr), client, identity) {
        Some((p, e, true)) => (p, e),
        Some((p, e, false)) => (dir.join(p.file_name().unwrap_or_default()), e),
        None => {
            let used: Vec<String> = dock_files(&dir).iter().chain(dock_files(&client.join("prefs/NewChar/DockAreas")).iter()).filter_map(|(_, e)| dock_name(e)).collect();
            let name = (0..).map(|n| format!("DockArea{n}")).find(|n| !used.contains(n)).unwrap_or_default();
            (dir.join(format!("{name}.xml")), new_dock(&name, identity))
        }
    }
}

fn view_archive(identity: &str) -> Option<&'static str> {
    Some(match identity {
        "friends_window" => "FriendsWindowConfig",
        "wear_window" => "WearViewConfig",
        "nano_window" => "NanoViewConfig",
        "stat_window" => "StatViewConfig",
        "team_view" => "TeamViewConfig",
        "ncu_window" => "NCUWindowConfig",
        "mission_window" => "MissionViewConfig",
        "map_window" => "PFMapWindowConfig",
        "planetmap_window" => "PlanetMapViewConfig",
        "faction_window" => "FactionWindowConfig",
        _ => return None,
    })
}

fn set_child(root: &mut Element, value: Element) {
    if let Some(i) = root.children.iter().position(|c| c.attr("name") == value.attr("name")) {
        root.children[i] = value;
    } else {
        root.children.push(value);
    }
}

fn scalar(tag: &str, name: &str, value: String) -> Element {
    Element { name: tag.into(), attrs: vec![("name".into(), name.into()), ("value".into(), value)], children: vec![] }
}

fn write_dock(state: &DockState, root: &mut Element) {
    if body_index(root).is_none() {
        root.children.push(archive("dock_config"));
    }
    let index = body_index(root).unwrap();
    let body = &mut root.children[index];
    set_child(body, Element {
        name: "Array".into(), attrs: vec![("name".into(), "docked_view_identities".into())],
        children: state.identities.iter().map(|id| Element {
            name: "String".into(), attrs: vec![("value".into(), format!("\"{id}\""))], children: vec![],
        }).collect(),
    });
    set_child(body, Element { name: "Array".into(), attrs: vec![("name".into(), "dock_node_configs".into())], children: state.nodes.clone() });
    if let Some(scroll) = state.scroll {
        set_child(body, scalar("Float", "scroll_offset", format!("{scroll:.6}")));
    }
    Cfg {
        frame: state.frame.map(|(x, y, w, h)| [x as f32, y as f32, (x + w as i32 - 1) as f32, (y + h as i32 - 1) as f32]),
        pin: Some(state.pin), tab: Some(state.selected),
    }.write_into(root);
    set_child(root, scalar("String", "dock_name", format!("\"{}\"", state.name)));
    set_child(root, scalar("String", "dock_type", format!("\"{}\"", if state.name == "RollupArea" { "RollupController" } else { "DockTabbedWindow" })));
}

struct Tracked {
    kind: WindowKind,
    id: WindowId,
    store: Store,
    last: Cfg,
}

/// The HUD's windows whose place is saved.
pub(super) struct WinCfgs {
    client: PathBuf,
    tracked: Vec<Tracked>,
}

/// Current state of a window as `SaveWndConfig` sees it.
fn current(gui: &Gui, id: WindowId, tab: Option<i64>) -> Option<Cfg> {
    let (x, y, w, h) = gui.window_outer_frame(id)?;
    Some(Cfg { frame: Some([x as f32, y as f32, (x + w as i32 - 1) as f32, (y + h as i32 - 1) as f32]), pin: Some(gui.window_pinned(id)), tab })
}

impl WinCfgs {
    pub fn new(client: &Path) -> Self {
        Self { client: client.to_path_buf(), tracked: vec![] }
    }

    pub fn load_docks(&self, d: &DValues) -> Vec<DockState> {
        let mut files = dock_files(&self.client.join("prefs/NewChar/DockAreas"));
        if let Some(chr) = d.char_dir() {
            for (path, root) in dock_files(&chr.join("DockAreas")) {
                let name = dock_name(&root);
                files.retain(|(_, e)| dock_name(e) != name);
                files.push((path, root));
            }
        }
        // A view belongs to one dock. Character assignments suppress its template membership.
        let user_ids: Vec<_> = d.char_dir().map(|chr| dock_files(&chr.join("DockAreas")).iter().flat_map(|(_, e)| identities(e)).collect()).unwrap_or_default();
        files.into_iter().filter_map(|(path, root)| {
            let name = dock_name(&root)?;
            let user = d.char_dir().is_some_and(|chr| path.starts_with(chr));
            let original = identities(&root);
            let body = body_index(&root).map_or(&root, |i| &root.children[i]);
            let nodes = body.children.iter().find(|c| c.attr("name") == Some("dock_node_configs"));
            let keep: Vec<_> = original.iter().enumerate().filter(|(_, id)| user || !user_ids.contains(id)).collect();
            let cfg = Cfg::parse(&root);
            let saved_tab = cfg.tab.unwrap_or(0);
            let selected_id = original.get(saved_tab.max(0) as usize);
            let selected = if saved_tab < 0 {
                saved_tab
            } else {
                keep.iter().position(|(_, id)| Some(*id) == selected_id).unwrap_or(0) as i64
            };
            Some(DockState {
                name, identities: keep.iter().map(|(_, id)| (*id).clone()).collect(), selected,
                frame: if user { cfg.frame.map(|[l,t,r,b]| (l as i32,t as i32,(r-l).max(0.0) as u32 + 1,(b-t).max(0.0) as u32 + 1)) } else { None },
                pin: cfg.pin.unwrap_or(false),
                nodes: keep.iter().filter_map(|(i, _)| nodes.and_then(|n| n.children.get(*i)).cloned()).collect(),
                scroll: child(body, "scroll_offset").and_then(|v| v.parse().ok()),
            })
        }).collect()
    }

    pub fn save_docks(&self, d: &mut DValues, states: &[DockState]) {
        let Some(chr) = d.char_dir().map(Path::to_path_buf) else { return };
        let dir = chr.join("DockAreas");
        if let Err(err) = std::fs::create_dir_all(&dir) {
            eprintln!("prefs: {}: {err}", dir.display());
            return;
        }
        for state in states {
            // Dock names are file stems, never paths supplied by preferences.
            if state.name.is_empty() || state.name.contains(['/', '\\']) || state.name == "." || state.name == ".." { continue; }
            let path = dir.join(format!("{}.xml", state.name));
            let mut root = std::fs::read_to_string(&path).ok().and_then(|s| xml::parse(&s).ok())
                .or_else(|| dock_files(&self.client.join("prefs/NewChar/DockAreas")).into_iter().find(|(_, e)| dock_name(e).as_deref() == Some(&state.name)).map(|(_, e)| e))
                .unwrap_or_else(|| new_dock(&state.name, ""));
            write_dock(state, &mut root);
            let text = element_xml(&root);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(&text) {
                if let Err(err) = std::fs::write(&path, text) {
                    eprintln!("prefs: {}: {err}", path.display());
                    continue;
                }
            }
            for identity in &state.identities {
                let Some(name) = view_archive(identity) else { continue };
                let mut cfg = match d.get(name) {
                    Some(Variant::Archive(s)) => xml::parse(s).unwrap_or_else(|_| archive(name)),
                    _ => archive(name),
                };
                set_child(&mut cfg, scalar("String", "DockableViewDockName", format!("\"{}\"", state.name)));
                let value = Variant::Archive(element_xml(&cfg));
                if d.exists(name) {
                    d.set(name, value);
                } else {
                    d.add(name, value, true, CAT_CHAR, None, None, false);
                }
            }
        }
    }

    /// The saved message of a store and whether it is the character's own (a template carries no frame to apply).
    fn load(&self, d: &DValues, store: Store) -> (Cfg, bool) {
        match store {
            Store::Value(name) => {
                let cfg = match d.get(name) {
                    Some(Variant::Archive(t)) => xml::parse(t).map(|e| Cfg::parse(&e)).unwrap_or_default(),
                    _ => Cfg::default(),
                };
                (cfg, true)
            }
            Store::Dock(id) => find_dock(d.char_dir(), &self.client, id).map_or((Cfg::default(), false), |(_, e, user)| (Cfg::parse(&e), user)),
        }
    }

    /// `Window::LoadWndConfig` for a window the HUD just opened: the character's saved frame (resizable windows take the size, the others the position),
    /// `MoveInsideScreen`, the pin state; the window becomes movable (`WndBorder::HitTest`, flags without 0x8). Windows without a store are left alone.
    pub fn attach(&mut self, gui: &mut Gui, d: &DValues, kind: WindowKind, id: WindowId, screen: (u32, u32)) {
        let Some(store) = store_of(kind) else { return };
        self.tracked.retain(|t| t.kind != kind);
        let size = resizable(kind);
        gui.set_window_frame(id, true, size);
        self.apply(gui, d, kind, id, store, screen);
        // a dock stores `selected_tab` (`~DockWindow_c` 0x1003b672: `Window::GetTabSelection`, 0 for the single tab), a plain window does not
        let tab = matches!(store, Store::Dock(_)).then(|| self.load(d, store).0.tab.unwrap_or(0));
        if let Some(last) = current(gui, id, tab) {
            self.tracked.push(Tracked { kind, id, store, last });
        }
    }

    fn apply(&self, gui: &mut Gui, d: &DValues, kind: WindowKind, id: WindowId, store: Store, screen: (u32, u32)) {
        let (cfg, user) = self.load(d, store);
        if let (Some([l, t, r, b]), true) = (cfg.frame, user) {
            let (x, y, w, h) = (l as i32, t as i32, (r - l) as u32 + 1, (b - t) as u32 + 1);
            if resizable(kind) {
                gui.set_window_outer_frame(id, (x, y, w, h));
            }
            let (w, h) = gui.outer_size(id);
            gui.set_window_pos(id, inside_screen((x, y, w, h), screen));
        }
        if let Some(p) = cfg.pin {
            gui.set_window_pinned(id, p);
        }
    }

    /// After the character's prefs were loaded (the HUD is built before them): applies the saved messages to the windows already open.
    pub fn reload(&mut self, gui: &mut Gui, d: &DValues, screen: (u32, u32)) {
        let open: Vec<_> = self.tracked.iter().map(|t| (t.kind, t.id, t.store)).collect();
        for (kind, id, store) in open {
            self.apply(gui, d, kind, id, store, screen);
            let tab = matches!(store, Store::Dock(_)).then(|| self.load(d, store).0.tab.unwrap_or(0));
            if let (Some(last), Some(t)) = (current(gui, id, tab), self.tracked.iter_mut().find(|t| t.id == id)) {
                t.last = last;
            }
        }
    }

    /// Keep windows touching a screen edge attached to that edge when the
    /// viewport changes, then apply `MoveInsideScreen` without changing size.
    pub fn resize_screen(&mut self, gui: &mut Gui, old: (u32, u32), new: (u32, u32)) {
        for t in &self.tracked {
            let Some((x, y, w, h)) = gui.window_outer_frame(t.id) else { continue };
            let x = if x + w as i32 == old.0 as i32 { new.0 as i32 - w as i32 } else { x };
            let y = if y + h as i32 == old.1 as i32 { new.1 as i32 - h as i32 } else { y };
            gui.set_window_pos(t.id, inside_screen((x, y, w, h), new));
        }
    }
    /// Saves every window whose frame / pin changed since the last call (not while a frame drag is running).
    pub fn update(&mut self, gui: &Gui, d: &mut DValues) {
        if gui.interacting() {
            return;
        }
        self.tracked.retain(|t| gui.window_outer_frame(t.id).is_some());
        for i in 0..self.tracked.len() {
            let t = &self.tracked[i];
            let Some(cur) = current(gui, t.id, t.last.tab) else { continue };
            if cur != t.last {
                self.save(d, i, &cur);
                self.tracked[i].last = cur;
            }
        }
    }

    /// `~DockWindow_c` / the window dtor: the final state goes to the store, the window is no longer tracked.
    pub fn detach(&mut self, gui: &Gui, d: &mut DValues, kind: WindowKind) {
        let Some(i) = self.tracked.iter().position(|t| t.kind == kind) else { return };
        if let Some(cur) = current(gui, self.tracked[i].id, self.tracked[i].last.tab) {
            if cur != self.tracked[i].last {
                self.save(d, i, &cur);
            }
        }
        self.tracked.remove(i);
    }

    /// Leaving the world: every window saves its final state.
    pub fn detach_all(&mut self, gui: &Gui, d: &mut DValues) {
        for k in self.tracked.iter().map(|t| t.kind).collect::<Vec<_>>() {
            self.detach(gui, d, k);
        }
    }

    fn save(&self, d: &mut DValues, i: usize, cfg: &Cfg) {
        match self.tracked[i].store {
            Store::Value(name) => {
                let mut e = match d.get(name) {
                    Some(Variant::Archive(t)) => xml::parse(t).unwrap_or_else(|_| archive(name)),
                    _ => archive(name),
                };
                cfg.write_into(&mut e);
                d.set(name, Variant::Archive(element_xml(&e)));
            }
            // DockingController owns group placement; a singleton must not overwrite it.
            Store::Dock(_) => {}
        }
    }
}

fn archive(name: &str) -> Element {
    Element { name: "Archive".into(), attrs: vec![("name".into(), name.into()), ("code".into(), "0".into())], children: vec![] }
}

/// `ItemListViewBase_c` config (GUI 100407b4 / 1004094e), also nested in `ShopViewConfig`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ListCfg {
    pub list: Option<bool>,
    pub columns: Vec<(i32, f32)>,
    /// Original `col_flags` parallel to `columns`; only visibility bit 0 is restored.
    pub col_flags: Vec<u32>,
    pub list_sort: Option<(i32, i32)>,
    pub grid_sort: Option<(i32, i32)>,
}

impl ListCfg {
    pub fn parse(root: &Element) -> Self {
        let array = |name| root.children.iter().find(|c| c.attr("name") == Some(name));
        let ids = array("col_id").into_iter().flat_map(|a| &a.children);
        let widths = array("col_width").into_iter().flat_map(|a| &a.children);
        let flags = array("col_flags");
        let parsed: Vec<_> = ids.zip(widths).enumerate().filter_map(|(i, (id, width))| {
            let id = id.attr("value")?.parse::<i32>().ok()?;
            let width = width.attr("value")?.parse::<f32>().ok()?;
            let flag = flags.and_then(|a| a.children.get(i)).and_then(|c| c.attr("value")).and_then(|v| v.parse().ok()).unwrap_or(0);
            (width.is_finite() && width >= 0.0).then_some((id, width, flag))
        }).collect();
        let sort = |prefix: &str| Some((child(root, &format!("{prefix}_sort_column"))?.parse().ok()?, child(root, &format!("{prefix}_sort_order"))?.parse().ok()?));
        Self {
            list: child(root, "listview_mode").and_then(|v| match v { "true" => Some(true), "false" => Some(false), _ => None }),
            columns: parsed.iter().map(|&(id, width, _)| (id, width)).collect(),
            col_flags: parsed.iter().map(|&(_, _, flags)| flags).collect(),
            list_sort: sort("list"),
            grid_sort: sort("grid"),
        }
    }

    /// Retain fields not owned by the list (e.g. window/dock settings).
    pub fn write_into(&self, root: &mut Element) {
        if let Some(list) = self.list {
            set_child(root, scalar("Bool", "listview_mode", list.to_string()));
        }
        for (name, tag, values) in [
            ("col_id", "Int32", self.columns.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>()),
            ("col_width", "Float", self.columns.iter().map(|(_, w)| format!("{w:.6}")).collect()),
            ("col_flags", "Int32", self.columns.iter().enumerate().map(|(i, _)| self.col_flags.get(i).copied().unwrap_or(0).to_string()).collect()),
        ] {
            set_child(root, Element { name: "Array".into(), attrs: vec![("name".into(), name.into())], children: values.into_iter().map(|v| Element {
                name: tag.into(), attrs: vec![("value".into(), v)], children: vec![],
            }).collect() });
        }
        for (prefix, sort) in [("list", self.list_sort), ("grid", self.grid_sort)] {
            if let Some((column, order)) = sort {
                set_child(root, scalar("Int32", &format!("{prefix}_sort_column"), column.to_string()));
                set_child(root, scalar("Int32", &format!("{prefix}_sort_order"), order.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_list_config_roundtrips_original_fields_without_losing_window_fields() {
        let mut root = archive("shop_listview_config");
        root.children.push(scalar("Rect", "WindowFrame", "Rect(1,2,3,4)".into()));
        let cfg = ListCfg { list: Some(false), columns: vec![(0, 16.0), (1, 200.0), (2, 30.0), (3, 100.0), (4, 100.0)], col_flags: vec![8, 14, 15, 14, 14], list_sort: Some((1, 0)), grid_sort: Some((1, 1)) };
        cfg.write_into(&mut root);
        assert_eq!(ListCfg::parse(&xml::parse(&element_xml(&root)).unwrap()), cfg);
        assert_eq!(child(&root, "WindowFrame"), Some("Rect(1,2,3,4)"));
        let bad = xml::parse("<Archive><Array name=\"col_id\"><Int32 value=\"bad\"/><Int32 value=\"4\"/></Array><Array name=\"col_width\"><Float value=\"200\"/><Float value=\"100\"/></Array></Archive>").unwrap();
        assert_eq!(ListCfg::parse(&bad).columns, vec![(4, 100.0)]);
    }

    const DOCK: &str = r#"<Archive code="0"><Archive code="0" name="dock_config"><Archive code="0" name="dock_node_configs" /><String name="docked_view_identities" value='&quot;ncu_window&quot;' /><Bool name="WindowPinButtonState" value="true" /><Rect name="WindowFrame" value="Rect(1838.000000,1078.000000,2030.000000,1171.000000)" /><Int32 name="selected_tab" value="0" /></Archive><String name="dock_type" value='&quot;DockTabbedWindow&quot;' /><String name="dock_name" value='&quot;DockArea2&quot;' /></Archive>"#;

    #[test]
    fn grouped_dock_roundtrips_order_selection_placement_and_rollup_nodes() {
        let mut root = xml::parse(DOCK).unwrap();
        let state = DockState {
            name: "DockArea2".into(), identities: vec!["ncu_window".into(), "team_view".into()],
            selected: 1, frame: Some((20, 30, 200, 100)), pin: false,
            nodes: vec![archive("kept_node")], scroll: Some(14.0),
        };
        write_dock(&state, &mut root);
        let back = xml::parse(&element_xml(&root)).unwrap();
        assert_eq!(identities(&back), state.identities);
        assert_eq!(Cfg::parse(&back), Cfg { frame: Some([20.0,30.0,219.0,129.0]), pin: Some(false), tab: Some(1) });
        let body = &back.children[body_index(&back).unwrap()];
        assert_eq!(child(body, "scroll_offset"), Some("14.000000"));
        assert_eq!(element_xml(&body.children.iter().find(|e| e.attr("name") == Some("dock_node_configs")).unwrap().children[0]), element_xml(&state.nodes[0]));
        let rollup = DockState { name: "RollupArea".into(), selected: -1, ..state };
        write_dock(&rollup, &mut root);
        assert_eq!(child(&root, "dock_type"), Some("\"RollupController\""));
        assert_eq!(Cfg::parse(&root).tab, Some(-1));
    }

    #[test]
    fn grouped_save_load_keeps_closed_members_and_archive_children() {
        let tmp = std::env::temp_dir().join(format!("aomac-dock-groups-{}", std::process::id()));
        let client = tmp.join("client");
        let cfgs = WinCfgs::new(&client);
        let mut d = DValues::new(&client);
        d.open_user(&tmp.join("prefs"), "test", 7);
        d.add("TeamViewConfig", Variant::Archive("<Archive name=\"TeamViewConfig\" code=\"0\"><Bool name=\"foreign\" value=\"true\"/></Archive>".into()), true, CAT_CHAR, None, None, false);
        let state = DockState {
            name: "DockArea3".into(), identities: vec!["ncu_window".into(), "team_view".into()],
            selected: 1, frame: Some((50, 60, 210, 120)), pin: true, nodes: vec![], scroll: None,
        };
        cfgs.save_docks(&mut d, &[state]);
        let loaded = cfgs.load_docks(&d);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].identities, ["ncu_window", "team_view"]);
        assert_eq!(loaded[0].selected, 1);
        assert_eq!(loaded[0].frame, Some((50, 60, 210, 120)));
        let Some(Variant::Archive(text)) = d.get("TeamViewConfig") else { panic!("missing view archive") };
        let archive = xml::parse(text).unwrap();
        assert_eq!(child(&archive, "DockableViewDockName"), Some("\"DockArea3\""));
        assert_eq!(child(&archive, "foreign"), Some("true"));
        assert!(matches!(d.get("NCUWindowConfig"), Some(Variant::Archive(_))));
        std::fs::remove_dir_all(tmp).unwrap();
    }
    #[test]
    fn reads_the_shipped_dock_file_and_keeps_foreign_children() {
        let mut e = xml::parse(DOCK).unwrap();
        let c = Cfg::parse(&e);
        assert_eq!(c, Cfg { frame: Some([1838.0, 1078.0, 2030.0, 1171.0]), pin: Some(true), tab: Some(0) });
        assert_eq!(identities(&e), ["ncu_window"]);
        assert_eq!(dock_name(&e).as_deref(), Some("DockArea2"));
        Cfg { frame: Some([10.0, 20.0, 109.0, 89.0]), pin: Some(false), tab: Some(0) }.write_into(&mut e);
        let back = xml::parse(&element_xml(&e)).unwrap();
        assert_eq!(Cfg::parse(&back), Cfg { frame: Some([10.0, 20.0, 109.0, 89.0]), pin: Some(false), tab: Some(0) });
        // the dock config keeps its node list, the file its type / name
        assert!(element_xml(&back).contains("dock_node_configs") && element_xml(&back).contains("DockTabbedWindow") && element_xml(&back).contains("DockArea2"));
    }

    #[test]
    fn value_archive_gets_the_two_children_added() {
        let mut e = xml::parse(r#"<Archive name="SkillConfig" code="0"/>"#).unwrap();
        assert_eq!(Cfg::parse(&e), Cfg::default());
        Cfg { frame: Some([200.0, 180.0, 850.0, 700.0]), pin: Some(true), tab: None }.write_into(&mut e);
        let t = element_xml(&e);
        assert!(t.contains(r#"name="WindowFrame""#) && t.contains("Rect(200.000000,180.000000,850.000000,700.000000)") && t.contains(r#"name="WindowPinButtonState" value="true""#), "{t}");
        assert!(!t.contains("selected_tab"));
        assert_eq!(Cfg::parse(&xml::parse(&t).unwrap()).pin, Some(true));
    }

    #[test]
    fn move_inside_screen_shifts_without_resizing() {
        let f = |x, y| inside_screen((x, y, 200, 100), (1280, 800));
        assert_eq!(f(-30, 50), (0, 50));
        assert_eq!(f(1200, 780), (1080, 700));
        assert_eq!(f(100, 100), (100, 100));
        // the saved frame of a bigger screen
        assert_eq!(f(1838, 1078), (1080, 700));
        // wider than the screen: the left edge wins (the first branch of 0x10154abc)
        assert_eq!(inside_screen((-5, 0, 2000, 10), (1280, 800)), (0, 0));
    }

    #[test]
    fn dock_files_pick_template_names_then_the_lowest_free_one() {
        let tmp = std::env::temp_dir().join(format!("aomac-wincfg-{}", std::process::id()));
        let (client, chr) = (tmp.join("client"), tmp.join("chr"));
        std::fs::create_dir_all(client.join("prefs/NewChar/DockAreas")).unwrap();
        std::fs::write(client.join("prefs/NewChar/DockAreas/DockArea2.xml"), DOCK).unwrap();
        // the template's dock of an identity is a copy target of the same name under the character
        let (p, _) = dock_target(&chr, &client, "ncu_window");
        assert_eq!(p, chr.join("DockAreas/DockArea2.xml"));
        // an identity nobody docks takes the lowest name neither the template nor the character uses
        let (p, e) = dock_target(&chr, &client, "faction_window");
        assert_eq!(p, chr.join("DockAreas/DockArea0.xml"));
        assert_eq!((dock_name(&e).as_deref(), identities(&e)), (Some("DockArea0"), vec!["faction_window".to_string()]));
        std::fs::create_dir_all(chr.join("DockAreas")).unwrap();
        std::fs::write(&p, element_xml(&e)).unwrap();
        assert_eq!(dock_target(&chr, &client, "map_window").0, chr.join("DockAreas/DockArea1.xml"));
        // the character's own file wins over the template and is found by identity
        let (_, _, user) = find_dock(Some(&chr), &client, "faction_window").unwrap();
        assert!(user);
        assert!(!find_dock(Some(&chr), &client, "ncu_window").unwrap().2);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
