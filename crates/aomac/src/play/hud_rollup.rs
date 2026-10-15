//! The `RollupArea` dock of the control centre (`RollupController_c : DockArea_c`, GUI 0x100487df ctor/dtor, `RollupPage_c` 0x10049389,
//! `PageHeaderView_c` 0x1004a07d, `RollupController_c` add-view 0x10049897, layout 0x10048232). Docs: docs/gui.md §11.13.
//!
//! The initial area is `Rect(W - 191, 20, W, H - 225)` (`InitialiseMessage` 0x1006a968: `FSUB double [0x101b4e00 = 191.0]` from the screen width for the left
//! edge, `[0x101b4e08 = 20.0]` top, `[0x101b4e10 = 225.0]` from the height for the bottom; asm 0x1006ab32..0x1006ab75), i.e. 192 px wide inclusive: the width of
//! `GFX_GUI_WEARVIEW_*` (192 x 320). Its final top follows `ControlCenter.xml`'s optional mini-toolbar, not that initial rectangle. The docked views are stacked top to bottom in the order of `DockAreas/RollupArea.xml` (`docked_view_identities`:
//! friends, wear, nano, stat); each is a `RollupPage_c` = header (`PageHeaderView_c`) + the view in a page of `page_height` px
//! (`dock_node_configs`), expandable (`is_page_expanded`). A page is `header + (expanded ? page_height + 1 + 3 + 5 : 1)` high (`FUN_10047d99`:
//! `+0x164` header height, `+0x168` page height, `+0x158` / `+0x160` page borders 3 / 5, `_DAT_101a87e8` = 1.0) and the pages are `1` px apart.
//!
//! Our pages are frameless windows (the Gui engine has no nested windows): [`Rollup::open_page`] builds the header and the body
//! around the view XML of the owner, [`Rollup::layout`] places them. Other HUD windows dock with `Rollup::open_page` (key = the window's dvalue
//! name, e.g. `nano_window`) and take [`RollupEvent::Closed`] to close their state.
//!
//! Shared clipping is traced in `View::_CallRender` 0x1014d2e3: inherited/screen rect intersection, recursive propagation,
//! flag 0x1 viewport application and restoration. Rollup-area parenting / clip-flag setup remains [INFERENCE].
//! UNRESOLVED: the header's popup menu (`FUN_10048736`: one entry of `LDBface::GetText(0x2710, ..)`
//! whose key is not decompiled) and the header art colours. Header reorder/undock and free-window tab grouping live in `docks.rs`.

use ao_gui::{CanvasItem, Event, GfxId, Gui, InputEvent, WindowId, WindowSize};
use std::path::Path;

mod docks;
use docks::{DockGroup, PageDrag};

/// Width of the area, inclusive pixels (`Rect(W - 191, ..., W, ...)`).
pub const AREA_W: u32 = 192;
/// Top of the area (`_DAT_101b4e08` = 20.0).
pub const AREA_TOP: i32 = 20;
/// Pixels the area leaves at the bottom of the screen (`_DAT_101b4e10` = 225.0).
pub const AREA_BOTTOM: i32 = 225;
/// Page borders of the body (`Rect(0, 3, 0, 5)` in `RollupPage_c`, `_DAT_101a96d4` / `_DAT_101a8b98`).
const BODY_TOP: u32 = 3;
const BODY_BOTTOM: u32 = 5;
/// Gap between two pages / under a collapsed page (`_DAT_101a87e8` = 1.0 as a double).
const GAP: i32 = 1;
/// GUI 0x100480f7: wheel delta times double `_DAT_101b0bd8` = 40.
const WHEEL_STEP: f32 = 40.0;

const ICON: &str = "GFX_GUI_WINDOW_ICON_I";
const EXPAND: &str = "GFX_GUI_ROLLUP_EXPAND_STATE1";
const COLLAPSE: &str = "GFX_GUI_ROLLUP_COLLAPSE_STATE1";
const CLOSE: &str = "GFX_GUI_WINDOW_CLOSE_X";
const BG: &str = "GFX_GUI_TAB_BACKGROUND";

/// `MiniToolbar_c` 0x100726b3: ordered view identity, DValue, tooltip and native art.
pub(super) const TOOLBAR: [(&str, &str, &str, &str); 10] = [
    ("mini_wear", "wear_window", "Wear", "GFX_GUI_SB_WEAR"),
    ("mini_action", "specialaction_window", "Controls", "GFX_GUI_SB_ACTION"),
    ("mini_ref", "knowledge_window", "Knowledge", "GFX_GUI_SB_REF"),
    ("mini_mission", "mission_window", "Mission", "GFX_GUI_SB_MISSION"),
    ("mini_team", "team_view", "Team", "GFX_GUI_SB_TEAM"),
    ("mini_map", "map_window", "Map", "GFX_GUI_SB_MAP"),
    ("mini_friends", "friends_window", "Friends", "GFX_GUI_SB_FRIENDS"),
    ("mini_nano", "nano_window", "Programs", "GFX_GUI_SB_NANO"),
    ("mini_stat", "stat_window", "Stats", "GFX_GUI_SB_STAT"),
    ("mini_ncu", "ncu_window", "NCU", "GFX_GUI_SB_NCU"),
];

pub(super) fn fill_toolbar(gui: &mut Gui, window: WindowId) -> anyhow::Result<()> {
    let mut src = String::from("<root><View view_layout=\"horizontal\" v_alignment=\"top\">");
    // 0x10072b33: raised = OFF; pressed and hover = ON; native tooltip has no body.
    // 0x100724e8 advances by inclusive button width + 1, with no extra gap.
    for (view, _, tooltip, art) in TOOLBAR {
        src += &format!("<Button name=\"{view}\" gfxid_raised=\"{art}_OFF\" gfxid_pressed=\"{art}\" gfxid_hover=\"{art}\" tooltip=\"{tooltip}\"/>");
    }
    src += "</View></root>";
    gui.add_view_xml(window, "CCMiniToolbar", "MiniToolbarButtons", &src)?;
    Ok(())
}

/// `page_height` / `is_page_expanded` of a docked view (`dock_node_configs`).
#[derive(Clone, Debug, PartialEq)]
pub struct PageConfig {
    pub key: String,
    pub height: f32,
    pub expanded: bool,
}

struct Page {
    key: String,
    window: WindowId,
    expanded: bool,
    title: String,
    docked: bool,
    wrapped: bool,
}

pub enum RollupEvent {
    /// The page's close button; the owner closes the window state (`WindowKind`).
    Closed(String),
    /// The click was a page's own (expand / collapse, icon).
    Handled,
}

pub struct Rollup {
    /// Order and sizes of the template (`DockAreas/RollupArea.xml`).
    config: Vec<PageConfig>,
    pages: Vec<Page>,
    screen: (u32, u32),
    /// Laid-out ControlCenter.xml dock top, after the optional mini-toolbar.
    dock_top: i32,
    /// Scroll offset in px (`scroll_offset` of the config, 0 at the first login).
    scroll: f32,
    /// The dock config changed since [`Rollup::take_dirty`].
    dirty: bool,
    groups: Vec<DockGroup>,
    drag: Option<PageDrag>,
}

/// Strips the quotes around a dvalue string (`value='"wear_window"'`).
fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// Reads `DockAreas/RollupArea.xml`: the docked view identities in order with their `page_height` / `is_page_expanded`.
pub fn read_config(src: &str) -> Vec<PageConfig> {
    let Ok(root) = ao_gui::xml::parse(src) else { return vec![] };
    let Some(dock) = root.children.iter().find(|c| c.attr("name") == Some("dock_config")) else { return vec![] };
    let ids = dock.children.iter().find(|c| c.attr("name") == Some("docked_view_identities"));
    let nodes = dock.children.iter().find(|c| c.attr("name") == Some("dock_node_configs"));
    let (Some(ids), Some(nodes)) = (ids, nodes) else { return vec![] };
    ids.children
        .iter()
        .zip(&nodes.children)
        .filter_map(|(id, n)| {
            let key = unquote(id.attr("value")?).to_string();
            let float = |name: &str| n.children.iter().find(|c| c.attr("name") == Some(name)).and_then(|c| c.attr("value")).and_then(|v| v.parse::<f32>().ok());
            let expanded = n.children.iter().find(|c| c.attr("name") == Some("is_page_expanded")).and_then(|c| c.attr("value")).map(|v| v == "true");
            Some(PageConfig { key, height: float("page_height")?, expanded: expanded.unwrap_or(true) })
        })
        .collect()
}

/// `scroll_offset` of a `RollupArea.xml` dock config.
pub fn read_scroll(src: &str) -> Option<f32> {
    let root = ao_gui::xml::parse(src).ok()?;
    let dock = root.children.iter().find(|c| c.attr("name") == Some("dock_config"))?;
    dock.children.iter().find(|c| c.attr("name") == Some("scroll_offset"))?.attr("value")?.parse().ok()
}

impl Rollup {
    pub fn new(dir: &Path, screen: (u32, u32)) -> Self {
        let config = std::fs::read_to_string(dir.join("prefs/NewChar/DockAreas/RollupArea.xml")).map(|s| read_config(&s)).unwrap_or_default();
        Self { config, pages: vec![], screen, dock_top: AREA_TOP, scroll: 0.0, dirty: false, groups: vec![], drag: None }
    }

    pub fn set_screen(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        self.resize_groups(gui, screen);
        self.screen = screen;
        self.layout(gui);
    }

    /// The area's left edge, XML-laid-out top and bottom for the screen.
    pub fn area(&self) -> (i32, i32, i32) {
        (self.screen.0 as i32 - (AREA_W as i32 - 1), self.dock_top, self.screen.1 as i32 - AREA_BOTTOM)
    }

    /// ControlCenter.xml places the rollup directly after CCMiniToolbar (zero top borders).
    /// The initial module rectangle is not the final dock position after XML layout.
    pub fn sync_dock(&mut self, gui: &mut Gui, control_center: WindowId) {
        if let Some(rect) = gui.view_rect(control_center, "RollupArea") {
            let top = rect.t as i32;
            if self.dock_top != top {
                self.dock_top = top;
                self.layout(gui);
            }
        }
    }

    /// The configured page height of a view (`page_height`); `fallback` for views the template does not list.
    /// Docks a view: `view_xml` is the XML root (`<root>..</root>`) of the view's content, laid out in a body of `page_height` px
    /// (`dock_node_configs`, or the view's own height when unlisted). Returns the page's window; named views inside it are reached through it as usual.
    pub fn open_page(&mut self, gui: &mut Gui, key: &str, title: &str, view_xml: &str, own_height: f32) -> anyhow::Result<WindowId> {
        let cfg = match self.config.iter().find(|c| c.key == key) {
            Some(c) => c.clone(),
            None => {
                // a view the dock did not list yet is docked now (`DockArea_c` appends it to `docked_view_identities`)
                let c = PageConfig { key: key.to_string(), height: own_height, expanded: true };
                self.config.push(c.clone());
                self.dirty = true;
                c
            }
        };
        let size = |gui: &Gui, n: &str| gui.gfx_id(n).map_or((15, 15), |g| gui.gfx().size(GfxId(g)));
        let (icon, arrow, close) = (size(gui, ICON), size(gui, EXPAND), size(gui, CLOSE));
        // `PageHeaderView_c`: icon button (borders 2, 2, 0, 2), title (8, 2, 0, 2), spacer, arrow (2, 2, 0, 2), close (2, 2, 2, 2)
        let header = [icon.1 + 4, arrow.1 + 4, close.1 + 4, 19].into_iter().max().unwrap_or(19);
        let body_h = cfg.height as u32;
        let canvas = |name: &str, s: (u32, u32), b: &str| format!("<CanvasView name=\"{name}\" layout_borders=\"Rect({b})\" min_size=\"Point({0},{1})\" max_size=\"Point({0},{1})\"/>", s.0, s.1);
        let w = AREA_W;
        let src = format!(
            "<root><View view_layout=\"vertical\" h_alignment=\"left\">\
             <View name=\"rollup_header\" view_layout=\"stacked\" min_size=\"Point({w},{header})\" max_size=\"Point({w},{header})\"><CanvasView name=\"header_bg\" min_size=\"Point({w},{header})\" max_size=\"Point({w},{header})\"/>\
             <View view_layout=\"horizontal\" v_alignment=\"top\">{}<TextView name=\"title\" value=\"{}\" layout_borders=\"Rect(8,2,0,2)\"/><HLayoutSpacer/>{}{}</View></View>\
             <CanvasView name=\"body\" view_layout=\"vertical\" h_alignment=\"left\" min_size=\"Point({w},{bh})\" max_size=\"Point({w},{bh})\" max_size_limit=\"Point({w},{bh})\">{view_xml_inner}</CanvasView>\
             </View></root>",
            canvas("icon", icon, "2,2,0,2"),
            esc(title),
            canvas("arrow", arrow, "2,2,0,2"),
            canvas("close", close, "2,2,2,2"),
            bh = body_h + BODY_TOP + BODY_BOTTOM,
            view_xml_inner = inner_xml(view_xml),
        );
        let window = gui.open_window_xml(&format!("Rollup_{key}"), &src, (0, 0), WindowSize::Preferred)?;
        let image = |gui: &Gui, n: &str, dst: [f32; 4]| gui.gfx_id(n).map(GfxId).map(|g| {
            let (w, h) = gui.gfx().size(g);
            // `PageHeaderView_c` 0x1004a07d builds its buttons like `WndBorder`'s `BorderButton_c`: raised view in DEFAULT at the layer-2 alpha
            CanvasItem::ImageTint { id: g, src: [0.0, 0.0, w as f32, h as f32], dst, color: 0x1000000, alpha: 0.85 }
        });
        // header: dark band, the icon / arrow / close art (the arrow is `ROLLUP_EXPAND_STATE1` when collapsed, `COLLAPSE_STATE1` when expanded)
        gui.set_canvas(window, "header_bg", vec![CanvasItem::Solid { dst: [0.0, 0.0, w as f32, header as f32], color: 0x000000, alpha: 0.85 }]);
        gui.set_canvas(window, "icon", image(gui, ICON, [0.0, 0.0, icon.0 as f32, icon.1 as f32]).into_iter().collect());
        gui.set_canvas(window, "close", image(gui, CLOSE, [0.0, 0.0, close.0 as f32, close.1 as f32]).into_iter().collect());
        self.paint_body_background(gui, window, "body");
        let mut page = Page { key: key.to_string(), window, expanded: cfg.expanded, title: title.to_string(), docked: true, wrapped: false };
        self.paint_arrow(gui, &mut page, arrow);
        if !cfg.expanded {
            gui.show_collapsing(window, "body", false);
            gui.resize_window(window, WindowSize::Preferred);
        }
        // keep the template's order
        let rank = |k: &str| self.config.iter().position(|c| c.key == k).unwrap_or(usize::MAX);
        let at = self.pages.iter().position(|p| rank(&p.key) > rank(key)).unwrap_or(self.pages.len());
        self.pages.insert(at, page);
        self.layout(gui);
        Ok(window)
    }

    fn paint_arrow(&self, gui: &mut Gui, page: &mut Page, size: (u32, u32)) {
        let art = if page.expanded { COLLAPSE } else { EXPAND };
        let items = gui.gfx_id(art).map(GfxId).map(|g| {
            let (w, h) = gui.gfx().size(g);
            // RollupPage 10049389 initializes SetValue(Variant(expanded)); 1004903b repeats it on expansion changes.
            CanvasItem::ImageTint { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, size.0 as f32, size.1 as f32], color: if page.expanded { 0x2000000 } else { 0x1000000 }, alpha: 0.85 }
        });
        gui.set_canvas(page.window, "arrow", items.into_iter().collect());
    }

    pub fn close_page(&mut self, gui: &mut Gui, key: &str) {
        if let Some(i) = self.pages.iter().position(|p| p.key == key) {
            let p = self.pages.remove(i);
            gui.close_window(p.window);
            self.layout(gui);
            self.sync_groups(gui);
        }
    }

    pub fn close_all(&mut self, gui: &mut Gui) {
        for p in self.pages.drain(..) {
            gui.close_window(p.window);
        }
    }

    /// RollupPage_c surface 0x198 covers the page; its 0x404040 surface
    /// covers only the bottom border after the child (GUI 0x10049389/0x100490e4).
    fn paint_body_background(&self, gui: &mut Gui, window: WindowId, name: &str) {
        let (width, height) = gui.canvas_size(window, name);
        let (w, h) = (width as f32, height as f32);
        let mut items = vec![];
        if let Some(id) = gui.gfx_id(BG).map(GfxId) {
            let (iw, ih) = gui.gfx().size(id);
            items.push(CanvasItem::ImageTint { id, src: [0.0, 0.0, iw as f32, ih as f32], dst: [0.0, 0.0, w, h], color: 0x1000000, alpha: 1.0 });
        }
        items.push(CanvasItem::Solid { dst: [0.0, (h - BODY_BOTTOM as f32).max(0.0), w, h], color: 0x404040, alpha: 1.0 });
        gui.set_canvas(window, name, items);
    }

    /// Size an owner wrapper from its page height, not its free-resize ceiling.
    fn resize_page(&self, gui: &mut Gui, page: &Page) {
        let width = if page.docked { AREA_W } else { gui.window_size(page.window).0 };
        if !page.docked {
            let size = if page.wrapped && !self.config.iter().any(|c| c.key == page.key) {
                WindowSize::Fixed(width, gui.window_size(page.window).1)
            } else {
                WindowSize::Preferred
            };
            // Release only the page's constraints; the owner's size limits still apply.
            gui.set_view_max_size_limit(page.window, "body", (16000.0, 16000.0));
            gui.set_view_pref_size(page.window, "body", (-1.0, -1.0), (-1.0, -1.0));
            gui.resize_window(page.window, size);
            if page.wrapped {
                // Restore the wrapper's elastic range only after choosing its preferred height.
                gui.set_view_pref_size(page.window, "body", (-1.0, -1.0), (16000.0, 16000.0));
            }
        } else if page.wrapped {
            let Some(config) = self.config.iter().find(|c| c.key == page.key) else {
                // No saved page height: retain the current content size, rather than
                // deriving a new preferred height from the wrapper's elastic body.
                let height = gui.window_size(page.window).1;
                gui.resize_window(page.window, WindowSize::Fixed(width, height));
                return;
            };
            let height = config.height;
            gui.set_view_max_size_limit(page.window, "body", (16000.0, height));
            gui.set_view_pref_size(page.window, "body", (0.0, height), (16000.0, height));
            gui.resize_window(page.window, WindowSize::Preferred);
        } else {
            if let Some(config) = self.config.iter().find(|c| c.key == page.key) {
                let height = config.height + (BODY_TOP + BODY_BOTTOM) as f32;
                gui.set_view_max_size_limit(page.window, "body", (AREA_W as f32, height));
                gui.set_view_pref_size(page.window, "body", (AREA_W as f32, height), (AREA_W as f32, height));
            }
            gui.resize_window(page.window, WindowSize::Preferred);
        }
        let height = gui.window_size(page.window).1;
        gui.resize_window(page.window, WindowSize::Fixed(width, height));
        if page.docked {
            self.paint_body_background(gui, page.window, if page.wrapped { "docked_body_bg" } else { "body" });
        }
    }

    /// Places the pages (`FUN_10048232`): top to bottom from the area top minus the scroll offset, [`GAP`] px apart.
    pub fn layout(&mut self, gui: &mut Gui) {
        let (x, top, bottom) = self.area();
        let total: i32 = self.pages.iter().filter(|p| p.docked).map(|p| gui.window_size(p.window).1 as i32 + GAP).sum();
        let max_scroll = (total - (self.screen.1 as i32 - AREA_BOTTOM - top)).max(0) as f32;
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let mut y = top - self.scroll as i32;
        for p in &self.pages {
            gui.set_window_clip(p.window, p.docked.then_some([x, top, self.screen.0 as i32, (bottom + 1).min(self.screen.1 as i32)]));
        }
        for p in self.pages.iter().filter(|p| p.docked) {
            let h = gui.window_size(p.window).1 as i32;
            gui.set_window_pos(p.window, (x, y));
            // Cull pages wholly outside the viewport; partial pages retain the area scissor.
            gui.set_window_visible(p.window, y + h > top && y <= bottom);
            y += h + GAP;
        }
    }

    /// `PageHeaderView_c` arrow: expands / collapses a page (`is_page_expanded`).
    fn set_expanded(&mut self, gui: &mut Gui, i: usize, on: bool, size: (u32, u32)) {
        let mut p = self.pages.remove(i);
        p.expanded = on;
        gui.show_collapsing(p.window, "body", on);
        self.resize_page(gui, &p);
        self.paint_arrow(gui, &mut p, size);
        if let Some(c) = self.config.iter_mut().find(|c| c.key == p.key) {
            c.expanded = on;
        }
        self.pages.insert(i, p);
        self.layout(gui);
        self.dirty = true;
    }

    /// Header clicks and the wheel over the area.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event) -> Option<RollupEvent> {
        let Event::CanvasClick { window, view, .. } = ev else { return None };
        if self.pages.iter().any(|p| p.window == *window && !p.docked) {
            return None;
        }
        let i = self.pages.iter().position(|p| p.window == *window)?;
        match view.as_str() {
            "arrow" => {
                let size = gui.gfx_id(EXPAND).map_or((15, 15), |g| gui.gfx().size(GfxId(g)));
                let on = !self.pages[i].expanded;
                self.set_expanded(gui, i, on, size);
                Some(RollupEvent::Handled)
            }
            "close" => Some(RollupEvent::Closed(self.pages[i].key.clone())),
            "icon" | "header_bg" => Some(RollupEvent::Handled),
            _ => None,
        }
    }

    /// The hovered area scrolls normally; MouseWheel=1 also connects the global wheel signal (GUI 0x1004816f).
    pub fn input(&mut self, gui: &mut Gui, ev: &InputEvent, mouse_wheel: i64) {
        self.drag_input(gui, ev);
        if let InputEvent::Wheel { x, y, dy } = *ev {
            let (left, top, bottom) = self.area();
            if (mouse_wheel == 1 || (x >= left as f32 && y >= top as f32 && y < bottom as f32)) && !self.pages.is_empty() {
                self.scroll -= dy * WHEEL_STEP;
                self.layout(gui);
                self.dirty = true;
            }
        }
    }


    /// The character's own `DockAreas/RollupArea.xml` (the HUD is built before the prefs are read): order, page state and scroll replace the template's.
    pub fn load_user(&mut self, gui: &mut Gui, src: &str) {
        let cfg = read_config(src);
        if cfg.is_empty() {
            return;
        }
        self.scroll = read_scroll(src).unwrap_or(0.0);
        self.config = cfg;
        let size = gui.gfx_id(EXPAND).map_or((15, 15), |g| gui.gfx().size(GfxId(g)));
        for i in 0..self.pages.len() {
            let want = self.config.iter().find(|c| c.key == self.pages[i].key).is_none_or(|c| c.expanded);
            if want != self.pages[i].expanded {
                self.set_expanded(gui, i, want, size);
            }
        }
        let rank = |k: &str| self.config.iter().position(|c| c.key == k).unwrap_or(usize::MAX);
        let mut pages = std::mem::take(&mut self.pages);
        pages.sort_by_key(|p| rank(&p.key));
        self.pages = pages;
        self.layout(gui);
        self.dirty = false;
    }

    /// Dock snapshots are written only once the user's interaction finishes.
    pub fn take_dock_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The children of `<root>..</root>`.
fn inner_xml(src: &str) -> &str {
    let s = src.trim();
    let s = s.strip_prefix("<root>").unwrap_or(s);
    s.strip_suffix("</root>").unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mouse_wheel_one_scrolls_sidebar_from_the_world() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, (1280, 600));
        rollup.open_page(&mut gui, "wheel_a", "A", "<root><View/></root>", 400.0).unwrap();
        rollup.open_page(&mut gui, "wheel_b", "B", "<root><View/></root>", 400.0).unwrap();
        let wheel = InputEvent::Wheel { x: 100.0, y: 100.0, dy: -1.0 };
        rollup.input(&mut gui, &wheel, 0);
        assert_eq!(rollup.scroll, 0.0);
        rollup.input(&mut gui, &wheel, 1);
        assert_eq!(rollup.scroll, 40.0);
        rollup.input(&mut gui, &wheel, 2);
        assert_eq!(rollup.scroll, 40.0);
    }

    #[test]
    fn mini_toolbar_and_rollup_share_the_xml_boundary() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let size = (640, 600);
        let mut gui = Gui::new(&dir, None).unwrap();
        let cc = gui.open_window("ControlCenter", (0, 0), WindowSize::Fixed(size.0, size.1)).unwrap();
        fill_toolbar(&mut gui, cc).unwrap();
        gui.add_view_xml(cc, "RollupControllerDock", "RollupArea", "<root><View name=\"RollupArea\" min_size=\"Point(192,600)\" max_size=\"Point(192,600)\"/></root>").unwrap();
        // Compare hidden height to omission of the native XML row, not screen y=0:
        // SpaceOut distributes residual space even when this fixture overfills the column.
        let xml = std::fs::read_to_string(dir.join("cd_image/gui/Default/Views/ControlCenter.xml")).unwrap();
        let without_toolbar = xml.lines().filter(|line| !line.contains("<CCMiniToolbar ")).collect::<Vec<_>>().join("\n");
        let baseline = gui.open_window_xml("ControlCenterWithoutToolbar", &without_toolbar, (0, 0), WindowSize::Fixed(size.0, size.1)).unwrap();
        gui.add_view_xml(baseline, "RollupControllerDock", "RollupArea", "<root><View name=\"RollupArea\" min_size=\"Point(192,600)\" max_size=\"Point(192,600)\"/></root>").unwrap();
        gui.apply_criteria(baseline, &|_, _| Some(1));
        let without_toolbar_top = gui.view_rect(baseline, "RollupArea").unwrap().t;
        let mut rollup = Rollup::new(&dir, size);
        let page = rollup.open_page(&mut gui, "wear_window", "Wear", "<root><View/></root>", 317.0).unwrap();
        for toolbar in [true, false, true] {
            gui.apply_criteria(cc, &|_, key| Some(i64::from(key != "cc_mini_toolbar" || toolbar)));
            rollup.sync_dock(&mut gui, cc);
            let dock = gui.view_rect(cc, "RollupArea").unwrap();
            assert_eq!(gui.window_outer_frame(page).unwrap().1, dock.t as i32);
            if toolbar {
                let strip = gui.view_rect(cc, "CCMiniToolbar").unwrap();
                assert_eq!(dock.t, strip.b + 1.0, "zero XML borders leave no toolbar-to-Wear gap");
                assert_eq!(strip.t, 0.0);
            } else {
                assert!(!gui.is_visible(cc, "CCMiniToolbar"));
                assert_eq!(dock.t, without_toolbar_top, "hidden toolbar consumes no layout height");
            }
        }
    }

    #[test]
    fn stacked_programs_wear_actions_respect_page_heights() {
        use crate::play::{hud::WindowKind, hud_actionwin::HudActionWin, hud_nano::HudNano, hud_stats::HudStats, zone::Zone};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let size = (1280, 1024);
        let mut gui = Gui::new(&dir, None).unwrap();
        gui.set_screen_size(size.0, size.1);
        let mut rollup = Rollup::new(&dir, size);
        // Preserve the original saved/natural heights: all three expanded pages
        // fit this taller viewport; a shorter viewport must scroll, not stretch.
        rollup.config = vec![
            PageConfig { key: "nano_window".into(), height: 205.0, expanded: true },
            PageConfig { key: "wear_window".into(), height: 317.0, expanded: true },
        ];
        let mut programs = HudNano::new(&dir, size).unwrap();
        let mut wear = HudStats::new(&dir, size).unwrap();
        let mut actions = HudActionWin::new(&dir, size);
        programs.open(&mut gui, &mut rollup);
        wear.open(&mut gui, &mut rollup, WindowKind::Character);
        actions.open(&mut gui, &mut rollup);
        let mut zone = Zone::new(7);
        // The same real Shadow Touch record/school fixture as hud_nano's tests.
        zone.nanos.programs = vec![163449];
        zone.nanos.serial += 1;
        programs.set_page(4);
        programs.update(&mut gui, &zone, 0.016);
        let mut shot = ClipShot { gui };
        // Render before geometry assertions so a historical failure still leaves
        // an actual before-frame. Without this env no GPU is needed.
        if let Some(out) = std::env::var_os("AOMAC_SHOT_DIR") {
            // Reproduce f78c6e5's missing ceiling with these same real owners.
            let saved = rollup.dock_states(&shot.gui);
            rollup.set_screen(&mut shot.gui, (1280, 800));
            shot.gui.set_screen_size(1280, 800);
            for page in &rollup.pages {
                shot.gui.set_view_max_size_limit(page.window, "body", (16000.0, 16000.0));
                shot.gui.resize_window(page.window, WindowSize::Preferred);
                rollup.paint_body_background(&mut shot.gui, page.window, "body");
            }
            rollup.layout(&mut shot.gui);
            let mut before = ao_render::Offscreen::new(&shot, (1280, 800)).unwrap();
            let list = before.frame(&mut shot, 0.016);
            std::fs::create_dir_all(&out).unwrap();
            before.png(&shot, &list, &Path::new(&out).join("rollup-before.png")).unwrap();
            rollup.set_screen(&mut shot.gui, size);
            shot.gui.set_screen_size(size.0, size.1);
            rollup.restore_docks(&mut shot.gui, &saved);
            let mut off = ao_render::Offscreen::new(&shot, size).unwrap();
            let list = off.frame(&mut shot, 0.016);
            std::fs::create_dir_all(&out).unwrap();
            off.png(&shot, &list, &Path::new(&out).join("rollup-stacked.png")).unwrap();
        } else {
            shot.gui.frame(0.016);
        }
        let gui = &mut shot.gui;
        let assert_stack = |rollup: &Rollup, gui: &Gui, all_fit: bool| {
            assert_eq!(rollup.pages.len(), 3, "all real owners opened successfully");
            let (left, top, bottom) = rollup.area();
            let mut y = top - rollup.scroll as i32;
            for (page, (key, height)) in rollup.pages.iter().zip([
                ("nano_window", 205), ("wear_window", 317), ("specialaction_window", 167),
            ]) {
                assert_eq!(page.key, key);
                assert!(page.docked && page.expanded);
                let header = gui.view_rect(page.window, "rollup_header").unwrap();
                let body = gui.view_rect(page.window, "body").unwrap();
                let expected = height + BODY_TOP + BODY_BOTTOM + 1;
                assert_eq!(body.height() + 1.0, expected as f32, "{key} respects its page height");
                let (x, actual_y, width, h) = gui.window_outer_frame(page.window).unwrap();
                assert_eq!((x, actual_y), (left, y), "{key} stacks directly below its predecessor");
                assert!((AREA_W..=AREA_W + 1).contains(&width), "{key} retains the inclusive dock width");
                assert_eq!(h, expected + (header.height() + 1.0) as u32);
                if all_fit {
                    assert!(gui.window_visible(page.window), "{key} remains visible");
                    assert!(actual_y >= top && actual_y + h as i32 <= bottom + 1, "{key} fits inside the visible column");
                }
                y += h as i32 + GAP;
            }
            if all_fit {
                assert_eq!(rollup.scroll, 0.0, "the complete stack needs no scrolling");
            }
        };
        assert_eq!(programs.shown(), [163449]);
        assert_stack(&rollup, gui, true);
        let saved = rollup.dock_states(gui);
        rollup.restore_docks(gui, &saved);
        assert_stack(&rollup, gui, true);
        // Use the real header drag path to free Programs, then its public tab
        // drop event to redock it. Owner content must survive both transitions.
        let window = rollup.pages[0].window;
        let header = gui.view_rect(window, "rollup_header").unwrap();
        let (x, y) = (header.l + 80.0, header.t + 5.0);
        for ev in [
            InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left },
            InputEvent::MouseMove { x: 300.0, y: 200.0 },
            InputEvent::MouseUp { x: 300.0, y: 200.0, button: ao_gui::MouseButton::Left },
        ] {
            rollup.input(gui, &ev, 0);
        }
        assert!(!rollup.pages.iter().find(|p| p.window == window).unwrap().docked);
        assert!(gui.view_rect(window, "body").unwrap().height() + 1.0 > 214.0, "free Programs releases the saved dock height");
        let (left, top, _) = rollup.area();
        assert!(rollup.dock_event(gui, &Event::TabDropped { window, tab: 0, x: left + 80, y: top, target: None }));
        assert_stack(&rollup, gui, true);
        rollup.restore_docks(gui, &saved);
        programs.update(gui, &zone, 0.016);
        assert_eq!(programs.shown(), [163449], "real content survives restore/free/redock");
        assert_stack(&rollup, gui, true);
        rollup.set_screen(gui, (1280, 800));
        gui.set_screen_size(1280, 800);
        assert_stack(&rollup, gui, false);
        rollup.scroll = 10000.0;
        rollup.layout(gui);
        assert!(rollup.scroll > 0.0, "natural pages scroll in a shorter viewport instead of stretching");
        assert_stack(&rollup, gui, false);
        let last = rollup.pages.last().unwrap().window;
        let (_, y, _, h) = gui.window_outer_frame(last).unwrap();
        assert!(gui.window_visible(last) && y + h as i32 <= rollup.area().2 + 1);
        if let Some(out) = std::env::var_os("AOMAC_SHOT_DIR") {
            let mut off = ao_render::Offscreen::new(&shot, (1280, 800)).unwrap();
            let list = off.frame(&mut shot, 0.016);
            off.png(&shot, &list, &Path::new(&out).join("rollup-stacked-800.png")).unwrap();
        }
        rollup.set_screen(&mut shot.gui, size);
        shot.gui.set_screen_size(size.0, size.1);
        rollup.restore_docks(&mut shot.gui, &saved);
        assert_stack(&rollup, &shot.gui, true);
    }

    #[test]
    fn historical_rollup_body_layout_regression() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        // Freeze both sides of f78c6e5's XML-only body change. A Programs-shaped
        // scroll child contributes an elastic maximum, not a hard size.
        let content = r#"<View view_layout="vertical"><ScrollView min_size="Point(167,96)" max_size="Point(16000,16000)"><ScrollViewChild view_layout="vertical" max_size="Point(16000,16000)"><CanvasView min_size="Point(167,96)" max_size="Point(16000,16000)"/></ScrollViewChild></ScrollView></View>"#;
        let before = format!(r#"<root><View view_layout="stacked" name="body" min_size="Point(192,213)" max_size="Point(192,213)"><CanvasView name="body_bg" min_size="Point(192,213)" max_size="Point(192,213)"/>{content}</View></root>"#);
        let after = format!(r#"<root><CanvasView view_layout="vertical" name="body" min_size="Point(192,213)" max_size="Point(192,213)">{content}</CanvasView></root>"#);
        let old = gui.open_window_xml("HistoricalStackedRollup", &before, (0, 0), WindowSize::Preferred).unwrap();
        let new = gui.open_window_xml("HistoricalVerticalRollup", &after, (0, 0), WindowSize::Preferred).unwrap();
        assert!(gui.window_size(old).1 <= 240, "the fixed background sibling bounds stacked preferred height");
        assert!(gui.window_size(new).1 >= 16000, "the vertical Canvas leaks the elastic child maximum");
    }

    struct ClipShot { gui: Gui }

    impl ao_render::Frontend for ClipShot {
        fn gui(&self) -> &Gui { &self.gui }
        fn input(&mut self, ev: InputEvent, _host: &mut ao_render::Host) { self.gui.input(ev); }
        fn frame(&mut self, dt: f32, _size: (u32, u32), _host: &mut ao_render::Host) -> ao_gui::DrawList {
            self.gui.frame(dt)
        }
    }

    #[test]
    fn scrolled_appended_page_offscreen_clip() {
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR") else { return };
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let size = (640, 600);
        let mut gui = Gui::new(&dir, None).unwrap();
        gui.set_screen_size(size.0, size.1);
        let mut rollup = Rollup::new(&dir, size);
        rollup.open_page(&mut gui, "first", "First", "<root><View/></root>", 200.0).unwrap();
        let xml = r#"<root><View view_layout="vertical"><ScrollView max_size="Point(16000,16000)"><ScrollViewChild view_layout="vertical" max_size="Point(16000,16000)"><CanvasView name="paint" min_size="Point(185,800)" max_size="Point(185,800)"/></ScrollViewChild></ScrollView></View></root>"#;
        let page = rollup.open_page(&mut gui, "shop_clip", "Shop-like appended page", xml, 800.0).unwrap();
        gui.set_canvas(page, "paint", vec![CanvasItem::Solid { dst: [0.0, 0.0, 185.0, 800.0], color: 0x00ff00, alpha: 1.0 }]);
        rollup.scroll = 10000.0;
        rollup.layout(&mut gui);
        assert!(gui.window_outer_frame(page).unwrap().1 < AREA_TOP, "page crosses the top viewport edge");
        assert!(!gui.wants_mouse(500.0, 10.0));
        let mut shot = ClipShot { gui };
        let mut off = ao_render::Offscreen::new(&shot, size).unwrap();
        let list = off.frame(&mut shot, 0.016);
        std::fs::create_dir_all(&out).unwrap();
        let path = Path::new(&out).join("rollup-clipped.png");
        off.png(&shot, &list, &path).unwrap();
        let image = image::open(path).unwrap().to_rgba8();
        let green = |p: &image::Rgba<u8>| p[1] > 200 && p[0] < 30 && p[2] < 30;
        assert!((0..AREA_TOP as u32).all(|y| (449..640).all(|x| !green(image.get_pixel(x, y)))), "nested canvas must not paint above RollupArea");
        assert!((AREA_TOP as u32..375).any(|y| green(image.get_pixel(500, y))), "visible part of nested canvas is rendered");
    }

    const XML: &str = r#"<Archive code="0"><Archive code="0" name="dock_config"><Array name="dock_node_configs">
        <Archive code="0"><Float name="page_height" value="215.000000" /><Bool name="is_page_expanded" value="true" /></Archive>
        <Archive code="0"><Bool name="is_page_expanded" value="false" /><Float name="page_height" value="317.000000" /></Archive></Array>
        <Array name="docked_view_identities"><String value='&quot;friends_window&quot;' /><String value='&quot;wear_window&quot;' /></Array></Archive></Archive>"#;

    #[test]
    fn template_order_heights_and_state() {
        let c = read_config(XML);
        assert_eq!(
            c,
            vec![PageConfig { key: "friends_window".into(), height: 215.0, expanded: true }, PageConfig { key: "wear_window".into(), height: 317.0, expanded: false }]
        );
        assert!(read_config("<nonsense").is_empty());
    }

    #[test]
    fn client_template_is_friends_wear_nano_stat() {
        let dir = ao_gui::client_dir();
        let Ok(s) = std::fs::read_to_string(dir.join("prefs/NewChar/DockAreas/RollupArea.xml")) else { return };
        let c = read_config(&s);
        assert_eq!(c.iter().map(|c| (c.key.as_str(), c.height as u32)).collect::<Vec<_>>(), [("friends_window", 215), ("wear_window", 317), ("nano_window", 205), ("stat_window", 279)]);
        assert!(c.iter().all(|c| c.expanded));
    }
}
