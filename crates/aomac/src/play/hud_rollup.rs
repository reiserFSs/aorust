//! The `RollupArea` dock of the control centre (`RollupController_c : DockArea_c`, GUI 0x100487df ctor/dtor, `RollupPage_c` 0x10049389,
//! `PageHeaderView_c` 0x1004a07d, `RollupController_c` add-view 0x10049897, layout 0x10048232). Docs: docs/gui.md §11.13.
//!
//! The area is `Rect(W - 191, 20, W, H - 225)` (`InitialiseMessage` 0x1006a968: `FSUB double [0x101b4e00 = 191.0]` from the screen width for the left
//! edge, `[0x101b4e08 = 20.0]` top, `[0x101b4e10 = 225.0]` from the height for the bottom; asm 0x1006ab32..0x1006ab75), i.e. 192 px wide inclusive: the width of
//! `GFX_GUI_WEARVIEW_*` (192 x 320). The docked views are stacked top to bottom in the order of `DockAreas/RollupArea.xml` (`docked_view_identities`:
//! friends, wear, nano, stat); each is a `RollupPage_c` = header (`PageHeaderView_c`) + the view in a page of `page_height` px
//! (`dock_node_configs`), expandable (`is_page_expanded`). A page is `header + (expanded ? page_height + 1 + 3 + 5 : 1)` high (`FUN_10047d99`:
//! `+0x164` header height, `+0x168` page height, `+0x158` / `+0x160` page borders 3 / 5, `_DAT_101a87e8` = 1.0) and the pages are `1` px apart.
//!
//! Our pages are frameless windows (the Gui engine has no nested windows): [`Rollup::open_page`] builds the header and the body
//! around the view XML of the owner, [`Rollup::layout`] places them. Other HUD windows dock with `Rollup::open_page` (key = the window's dvalue
//! name, e.g. `nano_window`) and take [`RollupEvent::Closed`] to close their state.
//!
//! UNRESOLVED: the scrolling of the controller (`scroll_offset` of the config, mouse wheel) is ours (wheel over the area, pages that end up
//! completely outside the screen are hidden: no window clipping); the header's popup menu (the "i" button, `FUN_10048736`: one entry of `LDBface::GetText(0x2710, ..)`
//! whose key is not decompiled), drag to reorder / undock (`FUN_10048d3f`: `dockableview/view` drag object) and the header art colours.

use ao_gui::{CanvasItem, Event, GfxId, Gui, InputEvent, WindowId, WindowSize};
use std::path::Path;

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
/// Wheel step of the scrolling (UNRESOLVED, ours).
const WHEEL_STEP: f32 = 30.0;

const ICON: &str = "GFX_GUI_WINDOW_ICON_I";
const EXPAND: &str = "GFX_GUI_ROLLUP_EXPAND_STATE1";
const COLLAPSE: &str = "GFX_GUI_ROLLUP_COLLAPSE_STATE1";
const CLOSE: &str = "GFX_GUI_WINDOW_CLOSE_X";
const BG: &str = "GFX_GUI_TAB_BACKGROUND";

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
    /// Scroll offset in px (`scroll_offset` of the config, 0 at the first login).
    scroll: f32,
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

impl Rollup {
    pub fn new(dir: &Path, screen: (u32, u32)) -> Self {
        let config = std::fs::read_to_string(dir.join("prefs/NewChar/DockAreas/RollupArea.xml")).map(|s| read_config(&s)).unwrap_or_default();
        Self { config, pages: vec![], screen, scroll: 0.0 }
    }

    pub fn set_screen(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        self.screen = screen;
        self.layout(gui);
    }

    /// The area's left edge and bottom for the screen (`Rect(W - 191, 20, W, H - 225)`).
    pub fn area(&self) -> (i32, i32, i32) {
        (self.screen.0 as i32 - (AREA_W as i32 - 1), AREA_TOP, self.screen.1 as i32 - AREA_BOTTOM)
    }

    /// The configured page height of a view (`page_height`); `fallback` for views the template does not list.
    /// Docks a view: `view_xml` is the XML root (`<root>..</root>`) of the view's content, laid out in a body of `page_height` px
    /// (`dock_node_configs`, or the view's own height when unlisted). Returns the page's window; named views inside it are reached through it as usual.
    pub fn open_page(&mut self, gui: &mut Gui, key: &str, title: &str, view_xml: &str, own_height: f32) -> anyhow::Result<WindowId> {
        let cfg = self.config.iter().find(|c| c.key == key).cloned().unwrap_or(PageConfig { key: key.to_string(), height: own_height, expanded: true });
        let size = |gui: &Gui, n: &str| gui.gfx_id(n).map_or((15, 15), |g| gui.gfx().size(GfxId(g)));
        let (icon, arrow, close) = (size(gui, ICON), size(gui, EXPAND), size(gui, CLOSE));
        // `PageHeaderView_c`: icon button (borders 2, 2, 0, 2), title (8, 2, 0, 2), spacer, arrow (2, 2, 0, 2), close (2, 2, 2, 2)
        let header = [icon.1 + 4, arrow.1 + 4, close.1 + 4, 19].into_iter().max().unwrap_or(19);
        let body_h = cfg.height as u32;
        let canvas = |name: &str, s: (u32, u32), b: &str| format!("<CanvasView name=\"{name}\" layout_borders=\"Rect({b})\" min_size=\"Point({0},{1})\" max_size=\"Point({0},{1})\"/>", s.0, s.1);
        let w = AREA_W;
        let src = format!(
            "<root><View view_layout=\"vertical\" h_alignment=\"left\">\
             <View view_layout=\"stacked\" min_size=\"Point({w},{header})\" max_size=\"Point({w},{header})\"><CanvasView name=\"header_bg\" min_size=\"Point({w},{header})\" max_size=\"Point({w},{header})\"/>\
             <View view_layout=\"horizontal\" v_alignment=\"top\">{}<TextView name=\"title\" value=\"{}\" layout_borders=\"Rect(8,2,0,2)\"/><HLayoutSpacer/>{}{}</View></View>\
             <View view_layout=\"stacked\" name=\"body\" h_alignment=\"left\" min_size=\"Point({w},{bh})\" max_size=\"Point({w},{bh})\"><CanvasView name=\"body_bg\" min_size=\"Point({w},{bh})\" max_size=\"Point({w},{bh})\"/>{view_xml_inner}</View>\
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
        // body: `RollupPage_c` renders `GFX_GUI_TAB_BACKGROUND` (0x198) over a 0x404040 surface
        let bh = (body_h + BODY_TOP + BODY_BOTTOM) as f32;
        let mut bg = vec![CanvasItem::Solid { dst: [0.0, 0.0, w as f32, bh], color: 0x404040, alpha: 1.0 }];
        bg.extend(image(gui, BG, [0.0, 0.0, w as f32, bh]));
        gui.set_canvas(window, "body_bg", bg);
        let mut page = Page { key: key.to_string(), window, expanded: cfg.expanded };
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
            // a toggle button: the pressed view (SELECTED) while expanded, DEFAULT when collapsed (**GUESS** for the value; the retail shot shows the expanded pages' button cream)
            CanvasItem::ImageTint { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, size.0 as f32, size.1 as f32], color: if page.expanded { 0x2000000 } else { 0x1000000 }, alpha: 0.85 }
        });
        gui.set_canvas(page.window, "arrow", items.into_iter().collect());
    }

    pub fn close_page(&mut self, gui: &mut Gui, key: &str) {
        if let Some(i) = self.pages.iter().position(|p| p.key == key) {
            let p = self.pages.remove(i);
            gui.close_window(p.window);
            self.layout(gui);
        }
    }

    pub fn close_all(&mut self, gui: &mut Gui) {
        for p in self.pages.drain(..) {
            gui.close_window(p.window);
        }
    }

    /// Places the pages (`FUN_10048232`): top to bottom from the area top minus the scroll offset, [`GAP`] px apart.
    pub fn layout(&mut self, gui: &mut Gui) {
        let (x, top, _) = self.area();
        let total: i32 = self.pages.iter().map(|p| gui.window_size(p.window).1 as i32 + GAP).sum();
        let max_scroll = (total - (self.screen.1 as i32 - AREA_BOTTOM - top)).max(0) as f32;
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let mut y = top - self.scroll as i32;
        for p in &self.pages {
            let h = gui.window_size(p.window).1 as i32;
            gui.set_window_pos(p.window, (x, y));
            // no clipping of windows: a page entirely outside the screen is hidden
            gui.set_window_visible(p.window, y + h > 0 && y < self.screen.1 as i32);
            y += h + GAP;
        }
    }

    /// Header clicks and the wheel over the area.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event) -> Option<RollupEvent> {
        let Event::CanvasClick { window, view, .. } = ev else { return None };
        let i = self.pages.iter().position(|p| p.window == *window)?;
        match view.as_str() {
            "arrow" => {
                let size = gui.gfx_id(EXPAND).map_or((15, 15), |g| gui.gfx().size(GfxId(g)));
                let mut p = self.pages.remove(i);
                p.expanded = !p.expanded;
                gui.show_collapsing(p.window, "body", p.expanded);
                gui.resize_window(p.window, WindowSize::Preferred);
                self.paint_arrow(gui, &mut p, size);
                self.pages.insert(i, p);
                self.layout(gui);
                Some(RollupEvent::Handled)
            }
            "close" => Some(RollupEvent::Closed(self.pages[i].key.clone())),
            _ => Some(RollupEvent::Handled),
        }
    }

    /// Mouse wheel over the area scrolls the column.
    pub fn input(&mut self, gui: &mut Gui, ev: &InputEvent) {
        if let InputEvent::Wheel { x, y, dy } = *ev {
            let (left, top, bottom) = self.area();
            if x >= left as f32 && y >= top as f32 && y < bottom as f32 && !self.pages.is_empty() {
                self.scroll -= dy * WHEEL_STEP;
                self.layout(gui);
            }
        }
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
