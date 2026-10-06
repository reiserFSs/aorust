//! `MultiListView_c` (GUI.dll `MultiListView_c::MultiListView_c` 0x10136423): the icon grid / column list the Programs, NCU and Mission windows are made of.
//! The client's view is a `ScrollView_c` over a client view that lays `MultiListViewItem_c`s out on a grid (`LayoutGrid`) or as rows (`SetLayoutMode`); this port
//! paints the grid on a `CanvasView` and builds the rows from XML views, inside the same scroll view the inventory uses. docs/gui.md §11.12.
//!
//! RE'd constants: grid icon `IconSize_e` 0 = 31 (+1) px, 1 = 47 (+1) px (the ctor default), 2 = 39 x 23, 3 = 19 x 11 (`SetGridIconSize` 0x10135719); the
//! ctor's grid spacing is (10, 10) and its border sizes (6, 6, 6, 6) (`+0x188`, `+0x170`); the slot art behind an icon is `GFX_GUI_MULTILISTVIEW_SLOT_<size>_CLOSED`.
//! UNRESOLVED GUESS: the list header's look (art not located in the draw code; a dark bar with the column names) and the colours of the row states.

use ao_gui::{CanvasItem, CanvasTip, GfxId, Gui, WindowId};

/// 32 px icons (`IconSize_e` 0, used by the Programs window and the NCU window's grid).
pub(super) const ICON_32: f32 = 32.0;
/// Grid spacing (ctor default `Point(10, 10)` at `+0x188`).
pub(super) const SPACING: f32 = 10.0;
/// Grid border sizes (ctor default `Rect(6, 6, 6, 6)` at `+0x170`).
pub(super) const BORDER: f32 = 6.0;
/// Width of the icon column (`AddColumn(0, "Icon", 16.0, 8)`).
pub(super) const ICON_COL: f32 = 16.0;
const SLOT_32: &str = "GFX_GUI_MULTILISTVIEW_SLOT_32_CLOSED";
const SLOT_32_OPEN: &str = "GFX_GUI_MULTILISTVIEW_SLOT_32_OPEN";
/// Row height of the list layout is the icon column's 16 px or the text height.
const ROW_ICON: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Grid,
    List,
}

/// One text column (`MultiListView_c::AddColumn(id, label, width, flags)`); the icon column is implicit.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Column {
    pub id: u32,
    pub label: String,
    pub width: f32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Row {
    /// The item's id (the nano / quest id).
    pub key: i32,
    pub icon: Option<(GfxId, u32, u32)>,
    /// One text per [`Column`].
    pub cells: Vec<String>,
    pub tip_title: String,
    pub tip_body: String,
}

/// What the user did to a row.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Hit {
    Click(i32),
    Double(i32),
    Context(i32, (i32, i32)),
}

pub(super) struct ListView {
    pub mode: Mode,
    pub columns: Vec<Column>,
    /// Grid columns; the window width decides ([`grid_cols_for`]).
    pub cols: usize,
    /// Rows of empty slots shown at least.
    pub min_rows: usize,
    pub selected: Option<i32>,
    /// `(column id, descending)` of the list layout (`SetListSortColumn`).
    pub sort: (u32, bool),
    rows: Vec<Row>,
    /// Row keys in grid cell order / list row order of the last [`ListView::paint`].
    order: Vec<i32>,
    /// Last Clicked time per row for the list layout's double click.
    last_click: Option<(i32, f32)>,
    /// Recharge overlays of grid icons: `(row key, progress)` (see [`ListView::set_fades`]).
    fades: Vec<(i32, f32)>,
    /// Painted state, to skip identical repaints.
    painted: Option<(Mode, Vec<i32>, Option<i32>)>,
}

/// Columns of a grid that fits `client_w` px: `n` icons and `n - 1` spacings between the borders (`RecalcCellCount`).
pub(super) fn grid_cols_for(client_w: f32, icon: f32) -> usize {
    (((client_w - 2.0 * BORDER + SPACING) / (icon + SPACING)).floor() as usize).max(1)
}

/// Client width of a grid of `cols` columns.
pub(super) fn grid_width(cols: usize, icon: f32) -> f32 {
    2.0 * BORDER + cols as f32 * icon + (cols as f32 - 1.0) * SPACING
}

/// The `ScrollView` + content skeleton of a list window: the part the window XML embeds.
pub(super) fn scroll_xml(min_h: f32, w: f32) -> String {
    format!(
        "<ScrollView name=\"lv_scroll\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"auto\" min_size=\"Point({w},{min_h})\" max_size=\"Point({w},{min_h})\">\
         <ScrollViewChild view_layout=\"vertical\" name=\"lv_scroller\"><View view_layout=\"vertical\" name=\"lv_content\"/></ScrollViewChild></ScrollView>"
    )
}

impl ListView {
    pub fn new(mode: Mode, columns: Vec<Column>, cols: usize, min_rows: usize, sort: (u32, bool)) -> Self {
        Self { mode, columns, cols, min_rows, selected: None, sort, rows: vec![], order: vec![], last_click: None, fades: vec![], painted: None }
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.painted = None;
        }
    }

    /// Replaces the items. Grid: server order (`GetFirstFreePos` per arrival); list: sorted by [`ListView::sort`].
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        if self.mode == Mode::List {
            let Some(ci) = self.columns.iter().position(|c| c.id == self.sort.0) else { return };
            let desc = self.sort.1;
            self.rows.sort_by(|a, b| {
                let (x, y) = (&a.cells[ci], &b.cells[ci]);
                let o = match (number(x), number(y)) {
                    (Some(p), Some(q)) => p.total_cmp(&q),
                    _ => x.to_lowercase().cmp(&y.to_lowercase()),
                };
                if desc {
                    o.reverse()
                } else {
                    o
                }
            });
        }
    }

    /// The disabled overlay of icons whose action recharges (`FUN_10040001`: `GFX_GUI_ICONFADER32` tinted 0, source and destination shrunk from both
    /// sides by `W * 0.5 * (1 - progress)`, `FUN_1003efe6`; the same as the hotbar's), progress = remaining / total. Repaints when it changed.
    pub fn set_fades(&mut self, fades: Vec<(i32, f32)>) {
        if self.fades != fades {
            self.fades = fades;
            self.painted = None;
        }
    }

    pub fn row(&self, key: i32) -> Option<&Row> {
        self.rows.iter().find(|r| r.key == key)
    }

    fn cell_pos(&self, i: usize, icon: f32) -> (f32, f32) {
        let p = icon + SPACING;
        (BORDER + (i % self.cols) as f32 * p, BORDER + (i / self.cols) as f32 * p)
    }

    /// Number of grid cells shown: the items rounded up to full rows, at least `min_rows`.
    fn cells(&self) -> usize {
        self.rows.len().div_ceil(self.cols).max(self.min_rows) * self.cols
    }

    /// Rebuilds the content of window `w` (grid canvas or rows) when the items, the mode or the selection changed.
    pub fn paint(&mut self, gui: &mut Gui, w: WindowId, icon: f32) {
        let sig = (self.mode, self.rows.iter().map(|r| r.key).collect::<Vec<_>>(), self.selected);
        if self.painted.as_ref() == Some(&sig) {
            return;
        }
        self.order = sig.1.clone();
        gui.remove_children(w, "lv_content");
        match self.mode {
            Mode::Grid => self.paint_grid(gui, w, icon),
            Mode::List => self.paint_list(gui, w),
        }
        gui.relayout_window(w);
        self.painted = Some(sig);
    }

    /// Forces the next [`ListView::paint`] to rebuild (a row's text changed).
    pub fn invalidate(&mut self) {
        self.painted = None;
    }

    fn paint_grid(&self, gui: &mut Gui, w: WindowId, icon: f32) {
        let cells = self.cells();
        let rows = cells / self.cols;
        let (cw, ch) = (grid_width(self.cols, icon), 2.0 * BORDER + rows as f32 * icon + (rows as f32 - 1.0) * SPACING);
        let xml = format!("<root><CanvasView name=\"grid\" min_size=\"Point({cw},{ch})\" max_size=\"Point({cw},{ch})\"/></root>");
        if let Err(e) = gui.add_view_xml(w, "lv_content", "Grid", &xml) {
            eprintln!("hud: list grid: {e:#}");
            return;
        }
        let art = |gui: &Gui, n: &str| gui.gfx_id(n).map(GfxId).map(|g| (g, gui.gfx().size(g)));
        let slot = art(gui, SLOT_32);
        let open = art(gui, SLOT_32_OPEN);
        let (mut items, mut tips) = (vec![], vec![]);
        for i in 0..cells {
            let (x, y) = self.cell_pos(i, icon);
            let row = self.order.get(i).and_then(|k| self.row(*k));
            // the slot art is centred on the icon square
            let put = |items: &mut Vec<CanvasItem>, a: Option<(GfxId, (u32, u32))>| {
                if let Some((g, (aw, ah))) = a {
                    let (ax, ay) = (x + ((icon - aw as f32) / 2.0).floor(), y + ((icon - ah as f32) / 2.0).floor());
                    items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, aw as f32, ah as f32], dst: [ax, ay, ax + aw as f32, ay + ah as f32], alpha: 1.0 });
                }
            };
            put(&mut items, slot);
            let Some(row) = row else { continue };
            if let Some((g, gw, gh)) = row.icon {
                items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, gw as f32, gh as f32], dst: [x, y, x + icon, y + icon], alpha: 1.0 });
            }
            if let (Some((_, p)), Some(g)) = (self.fades.iter().find(|f| f.0 == row.key), gui.gfx_id("GFX_GUI_ICONFADER32").map(GfxId)) {
                let shrink = (icon - 1.0) * 0.5 * (1.0 - p.clamp(0.0, 1.0));
                items.push(CanvasItem::ImageTint { id: g, src: [shrink, 0.0, icon - 2.0 * shrink, icon], dst: [x + shrink, y, x + icon - shrink, y + icon], color: 0, alpha: 1.0 });
            }
            if self.selected == Some(row.key) {
                put(&mut items, open);
            }
            tips.push(CanvasTip { rect: [x, y, x + icon, y + icon], title: row.tip_title.clone(), body: row.tip_body.clone() });
        }
        gui.set_canvas(w, "grid", items);
        gui.set_canvas_tips(w, "grid", tips);
    }

    fn paint_list(&self, gui: &mut Gui, w: WindowId) {
        let text_h = gui.font_height(ao_gui::FontId::Normal) as f32;
        let cell = |text: &str, wd: f32, color: &str| {
            format!("<TextView value=\"{}\" color=\"{color}\" min_size=\"Point({wd},-1)\" max_size=\"Point({wd},-1)\"/>", esc(text))
        };
        let mut head = format!("<View min_size=\"Point({ICON_COL},{text_h})\" max_size=\"Point({ICON_COL},{text_h})\"/>");
        for c in &self.columns {
            head += &cell(&c.label, c.width, "0xFFFFFF");
        }
        let mut xml = format!("<root><View view_layout=\"vertical\"><View view_layout=\"stacked\"><CanvasView name=\"lv_head\" min_size=\"Point(1,{text_h})\"/><View view_layout=\"horizontal\">{head}</View></View>");
        for (i, key) in self.order.iter().enumerate() {
            let Some(r) = self.row(*key) else { continue };
            let sel = self.selected == Some(r.key);
            let color = if sel { "TEXT_SELECTED" } else { "TEXT" };
            let mut line = format!("<CanvasView name=\"lv_icon{i}\" min_size=\"Point({ICON_COL},{ROW_ICON})\" max_size=\"Point({ICON_COL},{ROW_ICON})\"/>");
            for (n, c) in self.columns.iter().enumerate() {
                let text = r.cells.get(n).map_or("", String::as_str);
                if n == 0 {
                    // the first text column is the clickable name (`Clicked` row<i>)
                    line += &format!(
                        "<TextButton name=\"lv_row{i}\" text=\"{}\" color=\"{color}\" hover_color=\"TEXT_HOVER\" pressed_color=\"TEXT_SELECTED\" min_size=\"Point({wd},-1)\" max_size=\"Point({wd},-1)\"/>",
                        esc(text),
                        wd = c.width
                    );
                } else {
                    line += &cell(text, c.width, color);
                }
            }
            xml += &format!("<View view_layout=\"horizontal\" min_size=\"Point(1,{ROW_ICON})\">{line}</View>");
        }
        xml += "<VLayoutSpacer/></View></root>";
        if let Err(e) = gui.add_view_xml(w, "lv_content", "List", &xml) {
            eprintln!("hud: list rows: {e:#}");
            return;
        }
        gui.set_canvas(w, "lv_head", vec![CanvasItem::Solid { dst: [0.0, 0.0, 4000.0, text_h], color: 0x1a2a30, alpha: 0.9 }]);
        for (i, key) in self.order.iter().enumerate() {
            let Some(r) = self.row(*key) else { continue };
            if let Some((g, gw, gh)) = r.icon {
                gui.set_canvas(w, &format!("lv_icon{i}"), vec![CanvasItem::Image { id: g, src: [0.0, 0.0, gw as f32, gh as f32], dst: [0.0, 0.0, ROW_ICON, ROW_ICON], alpha: 1.0 }]);
            }
            gui.set_tooltip(w, &format!("lv_row{i}"), &r.tip_title, &r.tip_body);
        }
    }

    /// The row under a screen point (grid cells or list rows), `None` over empty slots / outside.
    pub fn row_at(&self, gui: &Gui, w: WindowId, x: f32, y: f32, icon: f32) -> Option<i32> {
        match self.mode {
            Mode::Grid => {
                let r = gui.view_rect(w, "grid")?;
                let (lx, ly) = (x - r.l, y - r.t);
                (0..self.cells()).find(|&i| {
                    let (cx, cy) = self.cell_pos(i, icon);
                    lx >= cx && lx < cx + icon && ly >= cy && ly < cy + icon
                })
                .and_then(|i| self.order.get(i).copied())
            }
            Mode::List => (0..self.order.len()).find(|&i| gui.view_rect(w, &format!("lv_row{i}")).is_some_and(|r| y >= r.t && y <= r.b && x >= r.l - ICON_COL && x <= r.r + 1.0 + self.columns.iter().skip(1).map(|c| c.width).sum::<f32>())).map(|i| self.order[i]),
        }
    }

    /// Turns a GUI event of window `w` into a row action. `now` is the clock of the list layout's double click (grid presses carry the engine's own count).
    pub fn event(&mut self, gui: &Gui, w: WindowId, ev: &ao_gui::Event, now: f32, icon: f32) -> Option<Hit> {
        use ao_gui::{Event, MouseButton};
        match ev {
            Event::CanvasPress { window, view, x, y, button: MouseButton::Left, clicks } if *window == w && view == "grid" => {
                let r = gui.view_rect(w, "grid")?;
                let key = self.row_at(gui, w, x + r.l, y + r.t, icon)?;
                self.selected = Some(key);
                Some(if *clicks >= 2 { Hit::Double(key) } else { Hit::Click(key) })
            }
            Event::Clicked { window, view, .. } if *window == w && view.starts_with("lv_row") => {
                let i: usize = view["lv_row".len()..].parse().ok()?;
                let key = *self.order.get(i)?;
                self.selected = Some(key);
                let double = self.last_click.is_some_and(|(k, t)| k == key && now - t <= ao_gui::DOUBLE_CLICK_TIME);
                self.last_click = (!double).then_some((key, now));
                Some(if double { Hit::Double(key) } else { Hit::Click(key) })
            }
            Event::ContextMenu { window, x, y, .. } if *window == w => {
                let key = self.row_at(gui, w, *x as f32, *y as f32, icon)?;
                self.selected = Some(key);
                Some(Hit::Context(key, (*x, *y)))
            }
            _ => None,
        }
    }
}

/// Texts of the `MultiListView_c` options menu (`FUN_10040b7b` "ListMode" check item; `FUN_10040c33` submenu "AutoArrange": column checks, "Sort Ascending" /
/// "Sort Descending" literals), resolved from the GUI category of text.mdb.
#[derive(Clone, Debug, Default)]
pub(super) struct MenuTexts {
    pub list_mode: String,
    pub auto_arrange: String,
}

/// A tabbed window (`DockWindow_c` style 0) around a [`ListView`]: the Programs, NCU and Mission windows.
pub(super) struct ListWindow {
    pub win: WindowId,
    pub view: ListView,
    /// Every column the window can show, in the client's order; `view.columns` is the visible subset (`col_id` of the saved `listview_config`).
    pub all: Vec<Column>,
    pub texts: MenuTexts,
    /// Menu ids of this window are `menu_base + n`.
    pub menu_base: u32,
    pub screen: (u32, u32),
}

const MENU_LIST_MODE: u32 = 1;
const MENU_SORT_ASC: u32 = 2;
const MENU_SORT_DESC: u32 = 3;
const MENU_COLUMN: u32 = 0x10;

/// How a [`ListWindow`] is opened: `top_xml` = children of the vertical client view above the scroll view (`top_h` px high), `client` = the client size.
pub(super) struct Spec<'a> {
    pub name: &'a str,
    pub title: &'a str,
    pub pos: (i32, i32),
    pub client: (u32, u32),
    pub top_xml: &'a str,
    pub top_h: f32,
    pub texts: MenuTexts,
    pub menu_base: u32,
    pub screen: (u32, u32),
}

impl ListWindow {
    /// Opens the window described by `spec`; `dock` = the rollup page to be built instead of a free window.
    pub fn open(gui: &mut Gui, spec: Spec, view: ListView, all: Vec<Column>, dock: Option<(&mut super::hud_rollup::Rollup, &str)>) -> anyhow::Result<Self> {
        let Spec { name, title, pos, client, top_xml, top_h, texts, menu_base, screen } = spec;
        let xml = format!(
            "<root><View view_layout=\"vertical\" h_alignment=\"left\">{top_xml}{}</View></root>",
            scroll_xml(client.1 as f32 - top_h, client.0 as f32)
        );
        if let Some((rollup, key)) = dock {
            // a `RollupPage_c` of the dock: the page window carries the header; the list window is that page
            let win = rollup.open_page(gui, key, title, &xml, client.1 as f32)?;
            gui.set_window_context(win, true);
            return Ok(Self { win, view, all, texts, menu_base, screen });
        }
        let win = gui.open_tabbed_window_xml(name, title, &xml, (0, 0), ao_gui::WindowSize::Fixed(client.0, client.1))?;
        gui.set_window_frame(win, true, false);
        gui.set_window_context(win, true);
        let mut w = Self { win, view, all, texts, menu_base, screen };
        w.place(gui, pos);
        Ok(w)
    }

    /// `Window::MoveInsideScreen` of the saved / default outer position.
    pub fn place(&mut self, gui: &mut Gui, (x, y): (i32, i32)) {
        let (ow, oh) = gui.outer_size(self.win);
        gui.set_window_pos(self.win, (x.clamp(0, self.screen.0.saturating_sub(ow) as i32), y.clamp(0, self.screen.1.saturating_sub(oh) as i32)));
    }

    pub fn close(self, gui: &mut Gui) {
        gui.close_window(self.win);
    }

    /// The window title tab text (`DockableView::SetTitle`).
    pub fn set_title(&self, gui: &mut Gui, title: &str) {
        gui.set_window_tabs(self.win, &[title.to_string()], 0);
    }

    /// Opens the options menu of the frame's icon button at `at`.
    pub fn open_menu(&self, gui: &mut Gui, at: (i32, i32)) {
        use ao_gui::MenuItem;
        let b = self.menu_base;
        let mut sub: Vec<MenuItem> = vec![];
        for c in self.all.iter().filter(|c| c.id != 0) {
            sub.push(MenuItem::check(b + MENU_COLUMN + c.id, &c.label, self.view.columns.iter().any(|v| v.id == c.id)));
        }
        sub.push(MenuItem::separator());
        sub.push(MenuItem::entry(b + MENU_SORT_ASC, "Sort Ascending"));
        sub.push(MenuItem::entry(b + MENU_SORT_DESC, "Sort Descending"));
        let items = vec![MenuItem::check(b + MENU_LIST_MODE, &self.texts.list_mode, self.view.mode == Mode::List), MenuItem::submenu(&self.texts.auto_arrange, sub)];
        gui.open_menu(at, (self.screen.0 as i32, self.screen.1 as i32), items);
    }

    pub fn owns_menu(&self, id: u32) -> bool {
        (self.menu_base..self.menu_base + 0x100).contains(&id)
    }

    /// Applies a picked menu entry; `true` when the rows have to be rebuilt.
    pub fn picked(&mut self, id: u32) -> bool {
        let n = id - self.menu_base;
        match n {
            MENU_LIST_MODE => {
                let m = if self.view.mode == Mode::List { Mode::Grid } else { Mode::List };
                self.view.set_mode(m);
            }
            MENU_SORT_ASC | MENU_SORT_DESC => {
                self.view.sort.1 = n == MENU_SORT_DESC;
            }
            n if n >= MENU_COLUMN => {
                let cid = n - MENU_COLUMN;
                if let Some(i) = self.view.columns.iter().position(|c| c.id == cid) {
                    // a list keeps at least its name column
                    if self.view.columns.len() > 1 {
                        self.view.columns.remove(i);
                    }
                } else if let Some(c) = self.all.iter().find(|c| c.id == cid).cloned() {
                    self.view.columns.push(c);
                    let order: Vec<u32> = self.all.iter().map(|c| c.id).collect();
                    self.view.columns.sort_by_key(|c| order.iter().position(|o| *o == c.id));
                }
            }
            _ => return false,
        }
        self.view.invalidate();
        true
    }
}

fn number(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

pub(super) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `%02d:%02d:%02d` of a duration in 1/100 s (`FUN_1004518b`'s "Remaining:" line: `t / 100` seconds → h:m:s).
pub(super) fn hms(cs: i32) -> String {
    let s = (cs / 100).max(0);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_geometry_follows_the_ctor_constants() {
        // 4 icons of 32 px, 10 px spacing, 6 px borders
        assert_eq!(grid_width(4, ICON_32), 170.0);
        assert_eq!(grid_cols_for(170.0, ICON_32), 4);
        assert_eq!(grid_cols_for(169.0, ICON_32), 3);
        // the install's NCU frame: 192 px outer, 10 px frame
        assert_eq!(grid_cols_for(182.0, ICON_32), 4);
    }

    #[test]
    fn rows_sort_by_the_sort_column() {
        let col = |id, l: &str| Column { id, label: l.into(), width: 50.0 };
        let mut v = ListView::new(Mode::List, vec![col(1, "Name"), col(9, "Cost")], 4, 3, (9, false));
        let row = |k, n: &str, c: &str| Row { key: k, cells: vec![n.into(), c.into()], ..Default::default() };
        v.set_rows(vec![row(1, "b", "10"), row(2, "a", "9"), row(3, "c", "100")]);
        assert_eq!(v.rows().iter().map(|r| r.key).collect::<Vec<_>>(), [2, 1, 3]);
        v.sort = (1, true);
        v.set_rows(vec![row(1, "b", "10"), row(2, "a", "9"), row(3, "c", "100")]);
        assert_eq!(v.rows().iter().map(|r| r.key).collect::<Vec<_>>(), [3, 1, 2]);
    }

    #[test]
    fn times_are_h_m_s() {
        assert_eq!(hms(45000), "00:07:30");
        assert_eq!(hms(180000), "00:30:00");
        assert_eq!(hms(-5), "00:00:00");
    }
}
