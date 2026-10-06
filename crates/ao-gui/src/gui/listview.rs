//! `DropdownMenu_c`, `ListViewBase_c` (+ `StringListViewItem_c`, XML `StringListView`) and `MultiListView_c` (list layout) of GUI.dll (docs/gui.md §13).
//!
//! RE evidence (GUI.dll): DropdownMenu 0x1012b7ae..0x1012c6ca (`Initialize` 0x1012bf10, `MouseDown` 0x1012be94, `SlotMenuOpening` 0x1012bb68,
//! `SlotMenuOpened` 0x1012b7df, `SelectByIndex` 0x1012bcda, `InsertItem` 0x1012c578, `DeleteItem` 0x1012c3f9); ListViewBase 0x10130f7d..0x10131f45
//! (`MouseDown` 0x101312c6, `ItemSelected` 0x1013118d, `_Layout` 0x10131a12), `ListViewBaseItem_c` 0x10132123..0x10132b72, `StringListViewItem_c`
//! 0x1014502d / `FUN_10144aab` (item view); MultiListView 0x10133350..0x10137a98 (`AddColumn` 0x101378a7, `AddItem` 0x10135b85, `LayoutList` 0x1013520e,
//! `MouseDown` 0x10133b63, `ItemSelectionStateChanged` 0x1013476a, `Sort` 0x10136bb6, `SlotColumnSelected` 0x1013746c), `ColumnHeaderView_c`
//! 0x1013a8c3 / `ColumnHeaderButton_c` 0x10139a17, `LFTCandidateItem_c` 0x100f1456 + vtable 0x101c0ef0 (`StringListColView_c` row view).

use super::*;

/// Row geometry constants of `MultiListView_c` list mode: item height `_DAT_101b00e0` = 15 (`LFTCandidateItem_c` vtable +0x18, 0x100ef49a), +1 for the
/// extent, `+1.0` (`_DAT_101a87e8`, a double) and the row gap `+0x1a4` = `_DAT_101a96d4` = 3 (`AddItem` 0x10135b85).
const ROW_H: f32 = 15.0;
const ROW_PITCH: f32 = ROW_H + 1.0 + 3.0;
/// Column gap `+0x1a0` = `_DAT_101a8b98` = 5 (`AddColumn`: `width + 1.0 + gap`).
const COL_GAP: f32 = 5.0;
/// Gap between the icon and the label of a `StringListViewItem_c` view (`_DAT_101a8b98` = 5).
const ICON_GAP: f32 = 5.0;
/// Selected-row `ViewSurface_c` colour (`FUN_100f15ef`).
const ROW_SEL: u32 = 0x88aadd;
/// HTML `aqua` (the key texts of the options window's "Fixed keys" page: `KeyListItemView_c`'s `<font color=aqua>`).
const AQUA: [u8; 3] = [0, 255, 255];
/// Icon blink half period (`EventTimer_c::Start(500000)` of `FUN_10144aab`).
const FLASH_HALF: f32 = 0.5;
/// Edge zone of a header cell that starts a resize (`_DAT_101a8b90` = 2) and drag distance (`_DAT_101a96d4` = 3).
const EDGE: f32 = 2.0;
const DRAG: f32 = 3.0;
/// Popup menu ids of the column-visibility menu (`FUN_1013af86`).
const COLMENU_BASE: u32 = 0x5E00_0000;

#[derive(Default)]
pub(super) struct Wx {
    /// Horizontal scroll-bar thumb drag: (scroll view, grab offset).
    pub hdrag: Option<(ViewId, f32)>,
    /// Dropdown whose popup menu is open, and the dropdown whose arrow currently shows the open art.
    dd_open: Option<ViewId>,
    dd_arrow: Option<ViewId>,
    /// Header interaction: (rows view, mode 1 = resize / 2 = click or drag, column id, press x, original width).
    head: Option<(ViewId, u8, i32, f32, f32)>,
    /// Rows view whose column menu is open.
    col_menu: Option<ViewId>,
    /// Last mouse press on a list row: (view, row key, button, time) for double clicks.
    last: Option<(ViewId, String, u8, f32)>,
}

fn rgb_(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

/// Icon shown by `FUN_10144aab` (0 = none).
fn list_icon(it: &ListItem) -> u32 {
    if it.icon == 0 {
        return 0;
    }
    if it.icon2 == 0 {
        return it.icon;
    }
    let first = if it.folder { it.open } else { it.selected };
    if first {
        it.icon
    } else {
        it.icon2
    }
}

struct LRow {
    item: usize,
    x: f32,
    y: f32,
}

/// Visible rows of the tree in display order (`_FindVisibleItems`: an item's children follow it while it is open; x = sum of the ancestors' indents).
fn list_rows(l: &ListData) -> Vec<LRow> {
    fn rec(l: &ListData, parent: usize, base: f32, y: &mut f32, out: &mut Vec<LRow>) {
        if !l.items[parent].open {
            return;
        }
        for &c in &l.items[parent].children {
            out.push(LRow { item: c, x: base, y: *y });
            *y += l.items[c].size.y + 1.0;
            rec(l, c, base + l.items[c].indent, y, out);
        }
    }
    let mut out = vec![];
    let mut y = 0.0;
    rec(l, 0, l.items[0].indent, &mut y, &mut out);
    out
}

impl Gui {
    // ------------------------------------------------------------------ shared helpers

    /// Name of the closest named ancestor (the application's handle of a composite widget).
    fn owner_name(&self, v: ViewId) -> String {
        let mut cur = self.tree.views[v].parent;
        while let Some(c) = cur {
            if !self.tree.views[c].name.is_empty() {
                return self.tree.views[c].name.clone();
            }
            cur = self.tree.views[c].parent;
        }
        String::new()
    }

    /// First view of the subtree of the view called `name` (itself included) accepted by `f`.
    fn widget_in(&self, w: WindowId, name: &str, f: impl Fn(&Kind) -> bool) -> Option<ViewId> {
        let root = self.find(w, name)?;
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            if f(&self.tree.views[v].kind) {
                return Some(v);
            }
            stack.extend(self.tree.views[v].children.iter().rev().copied());
        }
        None
    }

    /// Mouse position relative to the top-left of `v`.
    fn local_pos(&self, v: ViewId, x: f32, y: f32) -> (f32, f32) {
        let o = self.origin(v);
        let win = self.window_of(v).and_then(|w| self.windows[w].as_ref()).map_or((0, 0), |w| w.pos);
        (x - win.0 as f32 - o.0, y - win.1 as f32 - o.1)
    }

    /// Press count of a row press (2 = second press of the same row and button within [`DOUBLE_CLICK_TIME`]).
    fn row_clicks(&mut self, v: ViewId, key: &str, button: u8) -> u8 {
        let now = self.time;
        let dbl = matches!(&self.wx.last, Some((lv, k, b, t)) if *lv == v && k == key && *b == button && now - *t <= DOUBLE_CLICK_TIME);
        self.wx.last = if dbl { None } else { Some((v, key.to_string(), button, now)) };
        if dbl {
            2
        } else {
            1
        }
    }

    /// Releases the open-state art of the dropdown arrows once the popup menu is gone (called every frame and after input).
    pub(super) fn widgets_sync(&mut self) {
        if self.wx.dd_open.is_some() && self.ix.menu.is_none() {
            self.wx.dd_open = None;
        }
        if self.wx.col_menu.is_some() && self.ix.menu.is_none() {
            self.wx.col_menu = None;
        }
        if self.wx.dd_arrow != self.wx.dd_open {
            for (dd, open) in [(self.wx.dd_arrow, false), (self.wx.dd_open, true)] {
                if let Some(dd) = dd {
                    self.dropdown_arrow(dd, open);
                }
            }
            self.wx.dd_arrow = self.wx.dd_open;
        }
    }

    /// A popup-menu entry was chosen: dropdown picks and column-menu picks stay inside the widgets, everything else is [`Event::MenuPicked`].
    pub(super) fn menu_picked(&mut self, id: u32) {
        if let Some(dd) = self.wx.dd_open.take() {
            // `SlotMenuItemSelected` 0x1012bed8: `SelectByIndex(Variant::AsInt32(id), true)` when in range
            self.dropdown_select(dd, Some(id as usize), true);
            return;
        }
        if let Some(rows) = self.wx.col_menu.take() {
            self.multi_toggle_column(rows, (id - COLMENU_BASE) as i32);
            return;
        }
        self.events.push(Event::MenuPicked { id });
    }

    // ------------------------------------------------------------------ DropdownMenu_c

    fn dropdown_view(&self, w: WindowId, name: &str) -> Option<ViewId> {
        self.widget_in(w, name, |k| matches!(k, Kind::Dropdown(_)))
    }

    fn dd_data(&self, v: ViewId) -> Option<&DropdownData> {
        match &self.tree.views[v].kind {
            Kind::Dropdown(d) => Some(d),
            _ => None,
        }
    }

    fn dropdown_arrow(&mut self, dd: ViewId, open: bool) {
        if let Some(a) = self.tree.find(dd, "_arrow") {
            if let Kind::Bitmap { index, .. } = &mut self.tree.views[a].kind {
                *index = open as usize;
            }
        }
    }

    /// `RecalcMaxStringWidth` 0x1012ba12 / `SetMaxStringWidth` 0x1012bc2f: the label's min / max preferred width (70 while empty).
    fn dropdown_fit(&mut self, dd: ViewId) {
        let Some(d) = self.dd_data(dd).cloned() else { return };
        let w = match d.fixed_width {
            Some(w) => w,
            None if d.items.is_empty() => 70.0,
            None => d.items.iter().map(|(_, s)| self.fonts.font(FontId::Normal).text_width(s)).max().unwrap_or(0) as f32,
        };
        let label = d.selected.and_then(|i| d.items.get(i)).map(|(_, s)| s.clone()).unwrap_or_default();
        if let Some(t) = self.tree.find(dd, "_text") {
            if let Kind::Text(td) = &mut self.tree.views[t].kind {
                td.min_pref = Point::new(w, -1.0);
                td.max_pref = Point::new(w, -1.0);
                td.text = label;
            }
        }
        if let Some(win) = self.window_of(dd) {
            self.relayout_window(win);
        }
    }

    /// `DropdownMenu_c::SelectByIndex(index, emit)`: updates the label; `emit` raises [`Event::DropdownChanged`] (the `+0x128` signal).
    fn dropdown_select(&mut self, dd: ViewId, index: Option<usize>, emit: bool) {
        let Some(d) = self.dd_data(dd) else { return };
        let index = index.filter(|i| *i < d.items.len());
        if index == d.selected {
            return;
        }
        if let Kind::Dropdown(d) = &mut self.tree.views[dd].kind {
            d.selected = index;
        }
        self.dropdown_fit(dd);
        if emit {
            if let (Some(win), Some(i)) = (self.window_of(dd), index) {
                let id = self.dd_data(dd).map_or(0, |d| d.items[i].0);
                let view = self.tree.views[dd].name.clone();
                self.events.push(Event::DropdownChanged { window: win, view, index: i, id });
            }
        }
    }

    /// `DropdownMenu_c::InsertItem(index, id, text)`: `index` is clamped to the item count; the first insert selects item 0 with the signal.
    pub fn dropdown_insert(&mut self, w: WindowId, name: &str, index: usize, id: i64, text: &str) {
        let Some(dd) = self.dropdown_view(w, name) else { return };
        if let Kind::Dropdown(d) = &mut self.tree.views[dd].kind {
            let at = index.min(d.items.len());
            d.items.insert(at, (id, text.to_string()));
        }
        self.dropdown_fit(dd);
        if self.dd_data(dd).is_some_and(|d| d.selected.is_none()) {
            self.dropdown_select(dd, Some(0), true);
        }
    }

    /// `AppendItem`: returns the new index.
    pub fn dropdown_append(&mut self, w: WindowId, name: &str, id: i64, text: &str) -> usize {
        let n = self.dropdown_view(w, name).and_then(|d| self.dd_data(d)).map_or(0, |d| d.items.len());
        self.dropdown_insert(w, name, n, id, text);
        n
    }

    /// `DropdownMenu_c::Clear`.
    pub fn dropdown_clear(&mut self, w: WindowId, name: &str) {
        let Some(dd) = self.dropdown_view(w, name) else { return };
        if let Kind::Dropdown(d) = &mut self.tree.views[dd].kind {
            d.items.clear();
        }
        if self.dd_data(dd).is_some_and(|d| d.selected.is_some()) {
            self.dropdown_select(dd, None, true);
        }
        self.dropdown_fit(dd);
    }

    /// `DropdownMenu_c::DeleteItem`.
    pub fn dropdown_delete(&mut self, w: WindowId, name: &str, index: usize) {
        let Some(dd) = self.dropdown_view(w, name) else { return };
        let Some(d) = self.dd_data(dd) else { return };
        if index >= d.items.len() {
            return;
        }
        let sel = d.selected;
        if let Kind::Dropdown(d) = &mut self.tree.views[dd].kind {
            d.items.remove(index);
            d.selected = sel; // the raw `+0x144` is untouched by the erase; fixed below like the original
        }
        let len = self.dd_data(dd).map_or(0, |d| d.items.len());
        let next = match sel {
            Some(s) if s == index => {
                if let Kind::Dropdown(d) = &mut self.tree.views[dd].kind {
                    d.selected = None;
                }
                Some(if index == len { index.wrapping_sub(1) } else { index })
            }
            Some(s) if s > index => Some(s - 1),
            _ => None,
        };
        self.dropdown_fit(dd);
        if let Some(n) = next {
            if let Kind::Dropdown(d) = &mut self.tree.views[dd].kind {
                d.selected = None;
            }
            self.dropdown_select(dd, Some(n), true);
        }
    }

    /// `SelectByIndex(index, emit)`; `None` clears.
    pub fn dropdown_select_index(&mut self, w: WindowId, name: &str, index: Option<usize>, emit: bool) {
        if let Some(dd) = self.dropdown_view(w, name) {
            self.dropdown_select(dd, index, emit);
        }
    }

    /// `SelectByID(id, emit)` (`FindItemByID` first match; unknown ids clear the selection).
    pub fn dropdown_select_id(&mut self, w: WindowId, name: &str, id: i64, emit: bool) {
        let Some(dd) = self.dropdown_view(w, name) else { return };
        let i = self.dd_data(dd).and_then(|d| d.items.iter().position(|(i, _)| *i == id));
        self.dropdown_select(dd, i, emit);
    }

    /// `GetSelection`.
    pub fn dropdown_selected(&self, w: WindowId, name: &str) -> Option<usize> {
        self.dropdown_view(w, name).and_then(|d| self.dd_data(d)).and_then(|d| d.selected)
    }

    /// `GetItemID(GetSelection())`.
    pub fn dropdown_selected_id(&self, w: WindowId, name: &str) -> Option<i64> {
        let d = self.dropdown_view(w, name).and_then(|d| self.dd_data(d))?;
        d.selected.and_then(|i| d.items.get(i)).map(|(id, _)| *id)
    }

    /// `GetCurrentText`.
    pub fn dropdown_text(&self, w: WindowId, name: &str) -> String {
        let Some(d) = self.dropdown_view(w, name).and_then(|d| self.dd_data(d)) else { return String::new() };
        d.selected.and_then(|i| d.items.get(i)).map(|(_, s)| s.clone()).unwrap_or_default()
    }

    /// `SetMaxStringWidth(width)`: `Some(w)` fixes the label width, `None` (the original's negative value) follows the items again.
    pub fn dropdown_set_max_width(&mut self, w: WindowId, name: &str, width: Option<f32>) {
        let Some(dd) = self.dropdown_view(w, name) else { return };
        if let Kind::Dropdown(d) = &mut self.tree.views[dd].kind {
            d.fixed_width = width.filter(|w| *w >= 0.0);
        }
        self.dropdown_fit(dd);
    }

    /// `DropdownMenu_c::MouseDown` 0x1012be94: a left press opens the popup (`PopupMenu_c(1)` with the items as entries 0..n, placed at the view's
    /// (left + 5, bottom + 1) by `SlotMenuOpened` 0x1012b7df); a press while it is open closes it (the menu consumes the press).
    pub(super) fn dropdown_down(&mut self, dd: ViewId) {
        if !self.tree.views[dd].enabled {
            return;
        }
        let Some(d) = self.dd_data(dd) else { return };
        let items: Vec<MenuItem> = d.items.iter().enumerate().map(|(i, (_, s))| MenuItem::entry(i as u32, s)).collect();
        if items.is_empty() {
            return;
        }
        let o = self.origin(dd);
        let win = self.window_of(dd).and_then(|w| self.windows[w].as_ref()).map_or((0, 0), |w| w.pos);
        let at = ((o.0 + win.0 as f32 + 5.0) as i32, (o.1 + win.1 as f32 + self.tree.views[dd].frame.height() + 1.0) as i32);
        let screen = (self.tip.screen.0 as i32, self.tip.screen.1 as i32);
        self.open_menu(at, screen, items);
        self.wx.dd_open = Some(dd);
        self.widgets_sync();
    }

    // ------------------------------------------------------------------ ListViewBase_c

    fn list_view(&self, w: WindowId, name: &str) -> Option<ViewId> {
        self.widget_in(w, name, |k| matches!(k, Kind::List(_)))
    }

    fn list_data(&self, v: ViewId) -> Option<&ListData> {
        match &self.tree.views[v].kind {
            Kind::List(l) => Some(l),
            _ => None,
        }
    }

    fn list_find(l: &ListData, id: &str) -> Option<usize> {
        // `FindItemByID`: depth first, a child before its parent's own id check, first match wins
        fn rec(l: &ListData, node: usize, id: &str) -> Option<usize> {
            for &c in &l.items[node].children {
                if let Some(f) = rec(l, c, id) {
                    return Some(f);
                }
                if l.items[c].id == id {
                    return Some(c);
                }
            }
            None
        }
        rec(l, 0, id)
    }

    /// Re-measures every item (`StringListViewItem_c::SetSize` of the item view's preferred size), then resizes the view to its content.
    fn list_layout(&mut self, v: ViewId) {
        let Some(mut l) = self.list_data(v).cloned() else { return };
        let fh = self.fonts.font(FontId::Normal).height as f32;
        for i in 1..l.items.len() {
            let it = &l.items[i];
            let icon = list_icon(it);
            let (iw, ih) = if it.icon > 0 { self.gfx.size(GfxId(it.icon)) } else { (0, 0) };
            let _ = icon;
            let tw = if it.label.contains('<') { self.tab_title_width(&it.label) as f32 } else { self.fonts.font(FontId::Normal).text_width(&it.label) as f32 };
            let gap = if it.icon > 0 { ICON_GAP } else { 0.0 };
            let size = Point::new(iw as f32 + gap + tw - 1.0, (fh - 1.0).max(if ih > 0 { ih as f32 - 1.0 } else { 0.0 }));
            l.items[i].size = size;
        }
        let rows = list_rows(&l);
        let mut cw = 0.0f32;
        let mut ch = 0.0f32;
        for r in &rows {
            cw = cw.max(r.x + l.items[r.item].size.x + 1.0);
            let it = &l.items[r.item];
            if !it.aux.is_empty() {
                cw = cw.max(it.aux_x + self.fonts.font(FontId::Normal).text_width(&it.aux) as f32 + 1.0);
            }
            ch = ch.max(r.y + l.items[r.item].size.y + 1.0);
        }
        l.content = Point::new(cw, ch);
        let view = &mut self.tree.views[v];
        view.min_size = Point::new(cw - 1.0, ch - 1.0);
        view.kind = Kind::List(l);
        if let Some(win) = self.window_of(v) {
            self.relayout_window(win);
        }
    }

    /// `ListViewBase_c::SetRoot(new root)`: drops every item.
    pub fn list_clear(&mut self, w: WindowId, name: &str) {
        let Some(v) = self.list_view(w, name) else { return };
        if let Kind::List(l) = &mut self.tree.views[v].kind {
            let multi = l.multi;
            *l = ListData::default();
            l.multi = multi;
        }
        self.list_layout(v);
    }

    /// `ListViewBase_c::MakeMultiSelect`; switching to single selection keeps only the last selected item.
    pub fn list_set_multi(&mut self, w: WindowId, name: &str, multi: bool) {
        let Some(v) = self.list_view(w, name) else { return };
        if let Kind::List(l) = &mut self.tree.views[v].kind {
            l.multi = multi;
        }
        if !multi {
            let sel = self.list_selected(w, name);
            for id in sel.iter().take(sel.len().saturating_sub(1)) {
                self.list_select(w, name, id, false, true);
            }
        }
    }

    /// `ListViewBase_c::AppendItem` / `ListViewBaseItem_c::AppendChild`: adds `item` under the item called `parent` (`None` = the root); false when
    /// the parent does not exist.
    pub fn list_add(&mut self, w: WindowId, name: &str, parent: Option<&str>, item: ListItem) -> bool {
        let Some(v) = self.list_view(w, name) else { return false };
        let Kind::List(l) = &mut self.tree.views[v].kind else { return false };
        let p = match parent {
            None => 0,
            Some(id) => match Self::list_find(l, id) {
                Some(p) => p,
                None => return false,
            },
        };
        let idx = l.items.len();
        let mut item = item;
        item.parent = Some(p);
        item.children.clear();
        l.items.push(item);
        l.items[p].children.push(idx);
        self.list_layout(v);
        true
    }

    /// `ListViewBaseItem_c::DeleteAllChildrens`.
    pub fn list_remove_children(&mut self, w: WindowId, name: &str, id: &str) {
        let Some(v) = self.list_view(w, name) else { return };
        let Kind::List(l) = &mut self.tree.views[v].kind else { return };
        if let Some(i) = Self::list_find(l, id) {
            l.items[i].children.clear();
        }
        self.list_layout(v);
    }

    /// Applies `f` to the item called `id` (`SetLabel`, `SetLabelColor`, `FlashIcon`, `HideIcon`, `SetIsFolder`, `MakeSelectable`, ...) and re-measures.
    pub fn list_update(&mut self, w: WindowId, name: &str, id: &str, f: impl FnOnce(&mut ListItem)) {
        let Some(v) = self.list_view(w, name) else { return };
        let Kind::List(l) = &mut self.tree.views[v].kind else { return };
        if let Some(i) = Self::list_find(l, id) {
            f(&mut l.items[i]);
            let it = &mut l.items[i];
            if !it.selectable {
                it.selected = false;
            }
        }
        self.list_layout(v);
    }

    pub fn list_item(&self, w: WindowId, name: &str, id: &str) -> Option<&ListItem> {
        let v = self.list_view(w, name)?;
        let l = self.list_data(v)?;
        Self::list_find(l, id).map(|i| &l.items[i])
    }

    /// `ListViewBaseItem_c::OpenFolder`.
    pub fn list_open_folder(&mut self, w: WindowId, name: &str, id: &str, open: bool) {
        let Some(v) = self.list_view(w, name) else { return };
        if let Some(i) = self.list_data(v).and_then(|l| Self::list_find(l, id)) {
            self.list_open(v, i, open);
        }
    }

    fn list_open(&mut self, v: ViewId, i: usize, open: bool) {
        if let Kind::List(l) = &mut self.tree.views[v].kind {
            if l.items[i].open != open {
                l.items[i].open = open;
            }
        }
        self.list_layout(v);
    }

    /// Ids of the selected items in display order (`+0x16c` selection list).
    pub fn list_selected(&self, w: WindowId, name: &str) -> Vec<String> {
        let Some(l) = self.list_view(w, name).and_then(|v| self.list_data(v)) else { return vec![] };
        l.items.iter().skip(1).filter(|i| i.selected).map(|i| i.id.clone()).collect()
    }

    /// `ListViewBaseItem_c::Select(selected, emit)` of the item called `id`.
    pub fn list_select(&mut self, w: WindowId, name: &str, id: &str, selected: bool, emit: bool) {
        let Some(v) = self.list_view(w, name) else { return };
        if let Some(i) = self.list_data(v).and_then(|l| Self::list_find(l, id)) {
            self.list_select_item(v, i, selected, emit);
        }
    }

    fn list_select_item(&mut self, v: ViewId, i: usize, selected: bool, emit: bool) {
        let Some(l) = self.list_data(v) else { return };
        if !l.items[i].selectable || l.items[i].selected == selected {
            return;
        }
        if selected && !l.multi {
            // single selection: every other selected item is deselected (with its signal) first
            let others: Vec<usize> = (1..l.items.len()).filter(|k| l.items[*k].selected).collect();
            for k in others {
                self.list_select_item(v, k, false, true);
            }
        }
        let id = {
            let Kind::List(l) = &mut self.tree.views[v].kind else { return };
            l.items[i].selected = selected;
            l.items[i].id.clone()
        };
        if emit {
            if let Some(window) = self.window_of(v) {
                let view = self.owner_name(v);
                self.events.push(Event::ListSelected { window, view, id, selected });
            }
        }
        self.list_layout(v);
    }

    /// `ListViewBase_c::MouseDown` 0x101312c6 (any button): the row under the pointer toggles its folder, selects (multi select toggles) and raises the
    /// item-mouse signal (`+0x14c`) with the button.
    pub(super) fn list_down(&mut self, v: ViewId, x: f32, y: f32, button: u8) {
        let (_, ly) = self.local_pos(v, x, y);
        let Some(l) = self.list_data(v) else { return };
        let hit = list_rows(l).into_iter().find(|r| ly >= r.y && ly < r.y + l.items[r.item].size.y + 1.0);
        let Some(r) = hit else { return };
        let (folder, open, selected, multi, id) = {
            let it = &l.items[r.item];
            (it.folder, it.open, it.selected, l.multi, it.id.clone())
        };
        if folder {
            self.list_open(v, r.item, !open);
        }
        self.list_select_item(v, r.item, if multi { !selected } else { true }, true);
        let clicks = self.row_clicks(v, &id, button);
        if let Some(window) = self.window_of(v) {
            let view = self.owner_name(v);
            self.events.push(Event::ListItemMouse { window, view, id, button, clicks, x: x as i32, y: y as i32 });
        }
    }

    /// `StringListViewItem_c` item view (`FUN_10144d0f`: icon, 5 px gap, label, spacer) of every visible row.
    pub(super) fn draw_list(&mut self, out: &mut Vec<DrawCmd>, l: &ListData, rect: Rect, tint: [u8; 3], alpha: f32) {
        let phase = ((self.time / FLASH_HALF) as u64).is_multiple_of(2);
        for r in list_rows(l) {
            let it = &l.items[r.item];
            let (x, y) = (rect.l + r.x, rect.t + r.y);
            let row_h = it.size.y + 1.0;
            let mut tx = x;
            let icon = list_icon(it);
            if icon > 0 {
                let (iw, ih) = self.gfx.size(GfxId(icon));
                tx += iw as f32 + ICON_GAP;
                // `FUN_10144aab`: HideIcon hides the bitmap, FlashIcon toggles it every 0.5 s
                let shown = !it.hide_icon && (!it.flash || phase);
                if shown {
                    let t = y + ((row_h - ih as f32) / 2.0).floor();
                    self.push_gfx(out, GfxId(icon), Rect::new(x, t, x + iw as f32 - 1.0, t + ih as f32 - 1.0), tint, alpha);
                }
            }
            let c = if !it.selectable || it.selected { it.color_a } else { it.color_b };
            self.draw_markup(out, &it.label, tx as i32, y as i32, mul(tint, rgb_(c)), alpha);
            if !it.aux.is_empty() {
                self.draw_string(out, FontId::Normal, &it.aux, (rect.l + it.aux_x) as i32, y as i32, mul(tint, AQUA), alpha, false);
            }
        }
    }

    // ------------------------------------------------------------------ MultiListView_c (list layout)

    fn multi_view(&self, w: WindowId, name: &str) -> Option<ViewId> {
        self.widget_in(w, name, |k| matches!(k, Kind::Multi(_)))
    }

    fn multi_data(&self, v: ViewId) -> Option<&MultiData> {
        match &self.tree.views[v].kind {
            Kind::Multi(m) => Some(m),
            _ => None,
        }
    }

    fn multi_mut(&mut self, v: ViewId) -> Option<&mut MultiData> {
        match &mut self.tree.views[v].kind {
            Kind::Multi(m) => Some(m),
            _ => None,
        }
    }

    /// (column index, x, width) of the visible columns (`AddColumn`: x advances by `width + 1 + 5`).
    fn multi_cols(m: &MultiData) -> Vec<(usize, f32, f32)> {
        let mut x = 0.0;
        let mut out = vec![];
        for (i, c) in m.cols.iter().enumerate().filter(|(_, c)| c.flags & 1 == 0) {
            out.push((i, x, c.width));
            x += c.width + 1.0 + COL_GAP;
        }
        out
    }

    /// Total row width extent (`+0x260`).
    fn multi_width(m: &MultiData) -> f32 {
        Self::multi_cols(m).last().map_or(-1.0, |(_, x, w)| x + w)
    }

    fn multi_layout(&mut self, v: ViewId) {
        let Some(m) = self.multi_data(v) else { return };
        let (w, n) = (Self::multi_width(m), m.rows.len());
        let h = if n == 0 { 0.0 } else { n as f32 * ROW_PITCH - (ROW_PITCH - ROW_H - 1.0) };
        self.tree.views[v].min_size = Point::new(w, h - 1.0);
        let fh = self.fonts.font(FontId::Normal).height as f32;
        for hv in self.tree.views.iter_mut().filter(|x| matches!(x.kind, Kind::MultiHeader { rows } if rows == v)) {
            hv.min_size = Point::new(-1.0, fh - 1.0 + 6.0);
            hv.max_size = Point::new(16000.0, fh - 1.0 + 6.0);
        }
        if let Some(win) = self.window_of(v) {
            self.relayout_window(win);
        }
    }

    /// `MultiListView_c::SetFeatureFlags` (0x40 selection, 0x80 multiple selection).
    pub fn multi_set_flags(&mut self, w: WindowId, name: &str, flags: u32) {
        if let Some(m) = self.multi_view(w, name).and_then(|v| self.multi_mut(v)) {
            m.flags = flags;
        }
    }

    /// `MultiListView_c::AddColumn(id, label, width, flags)` (flags: 1 hidden, 2 resizable, 4 label, 8 sortable); the first sortable column becomes the
    /// sort column.
    pub fn multi_add_column(&mut self, w: WindowId, name: &str, id: i32, label: &str, width: f32, flags: u32) {
        let Some(v) = self.multi_view(w, name) else { return };
        if let Some(m) = self.multi_mut(v) {
            m.cols.push(MultiCol { id, label: label.into(), width, flags });
            if m.sort_col == -1 && flags & 8 != 0 {
                m.sort_col = id;
                m.sort_desc = false;
            }
        }
        self.multi_layout(v);
    }

    /// `MultiListView_c::Clear`.
    pub fn multi_clear(&mut self, w: WindowId, name: &str) {
        let Some(v) = self.multi_view(w, name) else { return };
        if let Some(m) = self.multi_mut(v) {
            m.rows.clear();
        }
        self.multi_layout(v);
    }

    fn cmp_rows(a: &MultiRow, b: &MultiRow, col: usize) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (a.cells.get(col), b.cells.get(col)) {
            (Some(x), Some(y)) => match (x.key, y.key) {
                (MultiKey::Num(p), MultiKey::Num(q)) => p.cmp(&q),
                (MultiKey::Text, MultiKey::Text) => x.text.as_bytes().cmp(y.text.as_bytes()),
                _ => Ordering::Equal,
            },
            _ => Ordering::Equal,
        }
    }

    fn sort_col_index(m: &MultiData) -> Option<usize> {
        m.cols.iter().position(|c| c.id == m.sort_col && c.flags & 8 != 0)
    }

    /// `MultiListView_c::AddItem(pos, item, sorted)`: with `sorted` and a sorted list the row goes behind the equal ones of its sort position, otherwise at
    /// the end (and the list is no longer in sort order).
    pub fn multi_add_row(&mut self, w: WindowId, name: &str, id: i64, cells: Vec<MultiCell>, sorted: bool) {
        let Some(v) = self.multi_view(w, name) else { return };
        let Some(m) = self.multi_mut(v) else { return };
        let row = MultiRow { id, cells, selected: false };
        let col = Self::sort_col_index(m);
        let at = match (col, m.sorted && sorted) {
            (Some(c), true) => {
                let desc = m.sort_desc;
                m.rows.iter().position(|r| {
                    let o = Self::cmp_rows(r, &row, c);
                    if desc {
                        o == std::cmp::Ordering::Less
                    } else {
                        o == std::cmp::Ordering::Greater
                    }
                })
            }
            _ => {
                if !sorted {
                    m.sorted = false;
                }
                None
            }
        };
        match at {
            Some(i) => m.rows.insert(i, row),
            None => m.rows.push(row),
        }
        self.multi_layout(v);
    }


    /// `MultiListView_c::SetListSortColumn(col, order)` + `Sort`: false when the column is unknown or not sortable.
    pub fn multi_sort(&mut self, w: WindowId, name: &str, col: i32, desc: bool) -> bool {
        let Some(v) = self.multi_view(w, name) else { return false };
        self.multi_sort_by(v, col, desc)
    }

    fn multi_sort_by(&mut self, v: ViewId, col: i32, desc: bool) -> bool {
        let Some(m) = self.multi_mut(v) else { return false };
        if !m.cols.iter().any(|c| c.id == col && c.flags & 8 != 0) {
            return false;
        }
        m.sort_col = col;
        m.sort_desc = desc;
        let c = Self::sort_col_index(m).unwrap_or(0);
        // ascending: stable sort; descending: the reverse compare (equal rows keep their order)
        m.rows.sort_by(|a, b| {
            let o = Self::cmp_rows(a, b, c);
            if desc {
                o.reverse()
            } else {
                o
            }
        });
        m.sorted = true;
        self.multi_layout(v);
        true
    }

    /// Row ids in display order.
    pub fn multi_row_ids(&self, w: WindowId, name: &str) -> Vec<i64> {
        self.multi_view(w, name).and_then(|v| self.multi_data(v)).map_or(vec![], |m| m.rows.iter().map(|r| r.id).collect())
    }

    /// Ids of the selected rows.
    pub fn multi_selected(&self, w: WindowId, name: &str) -> Vec<i64> {
        self.multi_view(w, name).and_then(|v| self.multi_data(v)).map_or(vec![], |m| m.rows.iter().filter(|r| r.selected).map(|r| r.id).collect())
    }

    /// Active sort column and order (`GetActiveSortColumn` / `GetActiveSortOrder`).
    pub fn multi_sort_state(&self, w: WindowId, name: &str) -> Option<(i32, bool)> {
        self.multi_view(w, name).and_then(|v| self.multi_data(v)).map(|m| (m.sort_col, m.sort_desc))
    }

    /// Column ids in their current order with width and hidden flag.
    pub fn multi_columns(&self, w: WindowId, name: &str) -> Vec<(i32, f32, bool)> {
        self.multi_view(w, name).and_then(|v| self.multi_data(v)).map_or(vec![], |m| m.cols.iter().map(|c| (c.id, c.width, c.flags & 1 != 0)).collect())
    }

    /// `MultiListViewItem_c::Select(selected, emit)` + `ItemSelectionStateChanged` 0x1013476a (needs the selection feature flags 0x40 / 0x80; single
    /// selection deselects the others without a signal).
    pub fn multi_select(&mut self, w: WindowId, name: &str, id: i64, selected: bool, emit: bool) {
        let Some(v) = self.multi_view(w, name) else { return };
        let Some(m) = self.multi_mut(v) else { return };
        let Some(i) = m.rows.iter().position(|r| r.id == id) else { return };
        if m.rows[i].selected == selected {
            return;
        }
        if m.flags & 0xc0 == 0 {
            // the row's own flag changes, the view's bookkeeping ignores it (`Select` sets `+0x30` before the callback)
            m.rows[i].selected = selected;
            return;
        }
        m.rows[i].selected = selected;
        if selected && m.flags & 0x80 == 0 {
            for (k, r) in m.rows.iter_mut().enumerate() {
                if k != i {
                    r.selected = false;
                }
            }
        }
        if emit {
            if let Some(window) = self.window_of(v) {
                let view = self.owner_name(v);
                self.events.push(Event::MultiSelected { window, view, id, selected });
            }
        }
    }

    /// `MultiListView_c::SetColumnWidth` + the header's resize signal.
    pub fn multi_set_column_width(&mut self, w: WindowId, name: &str, col: i32, width: f32) {
        let Some(v) = self.multi_view(w, name) else { return };
        self.multi_resize(v, col, width);
    }

    fn multi_resize(&mut self, v: ViewId, col: i32, width: f32) {
        if let Some(c) = self.multi_mut(v).and_then(|m| m.cols.iter_mut().find(|c| c.id == col)) {
            c.width = width.max(0.0);
        }
        self.multi_layout(v);
    }

    /// `MultiListView_c::EnableColumn` through the header menu.
    fn multi_toggle_column(&mut self, v: ViewId, col: i32) {
        if let Some(c) = self.multi_mut(v).and_then(|m| m.cols.iter_mut().find(|c| c.id == col)) {
            c.flags ^= 1;
        }
        self.multi_layout(v);
    }

    /// `FUN_1013af86`: right press on the header opens the column menu (`PopupMenu_c(2)`: one check item per column, checked while visible).
    fn column_menu(&mut self, rows: ViewId, x: f32, y: f32) {
        let Some(m) = self.multi_data(rows) else { return };
        let items: Vec<MenuItem> = m.cols.iter().map(|c| MenuItem::check(COLMENU_BASE + c.id as u32, &c.label, c.flags & 1 == 0)).collect();
        let screen = (self.tip.screen.0 as i32, self.tip.screen.1 as i32);
        self.open_menu((x as i32, y as i32), screen, items);
        self.wx.col_menu = Some(rows);
    }

    /// Left / right press on a row (`MultiListView_c::MouseDown` 0x10133b63): the mouse signal carries the row under the pointer (rows are selected by
    /// the application's slot, e.g. `FUN_100efacd` of the LFT window calls `Select(true, true)`).
    pub(super) fn multi_down(&mut self, v: ViewId, x: f32, y: f32, button: u8) {
        let (_, ly) = self.local_pos(v, x, y);
        let Some(m) = self.multi_data(v) else { return };
        let row = (0..m.rows.len()).find(|i| {
            let t = *i as f32 * ROW_PITCH;
            ly >= t && ly < t + ROW_H + 1.0
        });
        let id = row.map(|i| m.rows[i].id);
        let key = id.map_or(String::from("-"), |i| i.to_string());
        let clicks = self.row_clicks(v, &key, button);
        if let Some(window) = self.window_of(v) {
            let view = self.owner_name(v);
            self.events.push(Event::MultiMouse { window, view, id, button, clicks, x: x as i32, y: y as i32 });
        }
    }

    /// Cell rectangles of the header (x relative to the header's left edge, scrolled with the rows).
    fn header_cells(&self, hv: ViewId, rows: ViewId) -> Vec<(i32, f32, f32)> {
        let Some(m) = self.multi_data(rows) else { return vec![] };
        let off = self.header_offset(hv);
        Self::multi_cols(m).into_iter().map(|(i, x, w)| (m.cols[i].id, x - off, w)).collect()
    }

    fn header_offset(&self, hv: ViewId) -> f32 {
        let Some(p) = self.tree.views[hv].parent else { return 0.0 };
        self.tree.views[p].children.iter().find_map(|c| match &self.tree.views[*c].kind {
            Kind::ScrollView(sd) => Some(sd.offset.x),
            _ => None,
        })
        .unwrap_or(0.0)
    }

    /// `FUN_1013b098` (mouse down on `ColumnHeaderView_c`): a press within 2 px of a cell edge starts a resize of that column (needs flag 2; a double
    /// click fits the column to its content), anywhere else in a cell a click / drag; a right press opens the column menu.
    pub(super) fn header_down(&mut self, hv: ViewId, x: f32, y: f32, button: u8) {
        let Kind::MultiHeader { rows } = self.tree.views[hv].kind else { return };
        if button == 2 {
            self.column_menu(rows, x, y);
            return;
        }
        let (lx, _) = self.local_pos(hv, x, y);
        let cells = self.header_cells(hv, rows);
        let flags = |m: &MultiData, id: i32| m.cols.iter().find(|c| c.id == id).map_or(0, |c| c.flags);
        let mut prev: Option<i32> = None;
        for (id, cx, cw) in cells {
            if lx >= cx && lx <= cx + cw {
                let Some(m) = self.multi_data(rows) else { return };
                let key = format!("hdr{id}");
                if lx < cx + EDGE && prev.is_some() {
                    let p = prev.unwrap_or(-1);
                    if flags(m, p) & 2 != 0 {
                        self.header_resize_start(hv, rows, p, x, &key);
                    }
                } else if lx > cx + cw - EDGE {
                    if flags(m, id) & 2 != 0 {
                        self.header_resize_start(hv, rows, id, x, &key);
                    }
                } else {
                    let w0 = m.cols.iter().find(|c| c.id == id).map_or(0.0, |c| c.width);
                    self.wx.head = Some((rows, 2, id, x, w0));
                }
                return;
            }
            prev = Some(id);
        }
    }

    fn header_resize_start(&mut self, hv: ViewId, rows: ViewId, col: i32, lx: f32, key: &str) {
        let clicks = self.row_clicks(hv, key, 1);
        let w0 = self.multi_data(rows).and_then(|m| m.cols.iter().find(|c| c.id == col)).map_or(0.0, |c| c.width);
        if clicks == 2 {
            // `ResizeColumnToFit`: the widest cell of the column (GUESS: 0x10135a0b not read; the label is not counted)
            let texts: Vec<String> = self.multi_data(rows).map_or(vec![], |m| {
                let ci = m.cols.iter().position(|c| c.id == col).unwrap_or(0);
                m.rows.iter().filter_map(|r| r.cells.get(ci)).map(|c| c.text.clone()).collect()
            });
            let fit = texts.iter().map(|t| self.fonts.font(FontId::Normal).text_width(t)).max().unwrap_or(0) as f32;
            self.multi_resize(rows, col, fit);
            self.header_event_resized(rows, col);
            return;
        }
        self.wx.head = Some((rows, 1, col, lx, w0));
    }

    fn header_event_resized(&mut self, rows: ViewId, col: i32) {
        let width = self.multi_data(rows).and_then(|m| m.cols.iter().find(|c| c.id == col)).map_or(0.0, |c| c.width);
        if let Some(window) = self.window_of(rows) {
            let view = self.owner_name(rows);
            self.events.push(Event::MultiColumnResized { window, view, col, width });
        }
    }

    /// Mouse move while a header press is held: resize drags the column width (`SlotColumnResized` -> `SetColumnWidth`).
    pub(super) fn header_drag(&mut self, x: f32) {
        let Some((rows, 1, col, x0, w0)) = self.wx.head else { return };
        self.multi_resize(rows, col, (w0 + x - x0).max(2.0));
    }

    /// Release: a press that stayed (< 3 px) is `SlotColumnSelected` (sort), a drag over another cell swaps the columns (`SwapColumns`), a resize ends.
    pub(super) fn header_up(&mut self) {
        let Some((rows, mode, col, x0, _)) = self.wx.head.take() else { return };
        let Some(hv) = self.tree.views.iter().position(|v| matches!(v.kind, Kind::MultiHeader { rows: r } if r == rows)) else { return };
        if mode == 1 {
            self.header_event_resized(rows, col);
            return;
        }
        let win_x = self.window_of(hv).and_then(|w| self.windows[w].as_ref()).map_or(0, |w| w.pos.0) as f32;
        let lx = self.mouse.x - win_x - self.origin(hv).0;
        if (self.mouse.x - x0).abs() <= DRAG {
            // `SlotColumnSelected` 0x1013746c
            let Some(m) = self.multi_data(rows) else { return };
            let (cur, desc, sorted) = (m.sort_col, m.sort_desc, m.sorted);
            if col == cur && !sorted {
                self.multi_sort_by(rows, col, desc);
            } else if col == cur {
                self.multi_sort_by(rows, col, !desc);
            } else {
                self.multi_sort_by(rows, col, false);
            }
            return;
        }
        let target = self.header_cells(hv, rows).into_iter().find(|(_, cx, cw)| lx >= *cx && lx <= cx + cw).map(|c| c.0);
        if let (Some(t), Some(m)) = (target, self.multi_mut(rows)) {
            let (a, b) = (m.cols.iter().position(|c| c.id == col), m.cols.iter().position(|c| c.id == t));
            if let (Some(a), Some(b)) = (a, b) {
                m.cols.swap(a, b);
                for r in &mut m.rows {
                    if a < r.cells.len() && b < r.cells.len() {
                        r.cells.swap(a, b);
                    }
                }
            }
            self.multi_layout(rows);
        }
    }

    fn fit_text(&mut self, text: &str, max: f32) -> String {
        let mut s = text.to_string();
        while !s.is_empty() && self.fonts.font(FontId::Normal).text_width(&s) as f32 > max + 1.0 {
            s.pop();
        }
        s
    }

    /// Text with `<font color=..>` runs (the colours of `TextColors.xml`); plain text is a single string.
    fn draw_markup(&mut self, out: &mut Vec<DrawCmd>, text: &str, x: i32, y: i32, tint: [u8; 3], alpha: f32) {
        if !text.contains('<') {
            self.draw_string(out, FontId::Normal, text, x, y, tint, alpha, false);
            return;
        }
        let mut pen = x;
        for run in self.tab_runs(text) {
            let c = run.color.map_or(tint, |c| mul(tint, rgb_(c)));
            pen += self.draw_string(out, FontId::Normal, &run.text, pen, y, c, alpha, false);
        }
    }

    /// `LayoutList` + `StringListColView_c` rows (`FUN_100f171e`): a text cell per visible column, a selected row has the 0x88aadd `ViewSurface_c` behind it.
    pub(super) fn draw_multi(&mut self, out: &mut Vec<DrawCmd>, m: &MultiData, rect: Rect, tint: [u8; 3], alpha: f32) {
        let cols = Self::multi_cols(m);
        let w = Self::multi_width(m);
        for (i, r) in m.rows.iter().enumerate() {
            let y = rect.t + i as f32 * ROW_PITCH;
            if r.selected {
                out.push(DrawCmd::Solid { dst: [rect.l, y, rect.l + w + 1.0, y + ROW_H + 1.0], color: rgb_(ROW_SEL), alpha });
            }
            for (ci, x, cw) in &cols {
                if let Some(c) = r.cells.get(*ci) {
                    if c.text.contains('<') {
                        // the item's text view renders the HTML subset (`<font color=aqua>` keys, `<font color=red>NONE</font>` of the key-bindings page)
                        self.draw_markup(out, &c.text, (rect.l + x) as i32, y as i32, tint, alpha);
                    } else {
                        let t = self.fit_text(&c.text, *cw);
                        self.draw_string(out, FontId::Normal, &t, (rect.l + x) as i32, y as i32, tint, alpha, false);
                    }
                }
            }
        }
    }

    /// `ColumnHeaderView_c` (`FUN_1013a516`): a `ColumnHeaderButton_c` per visible column, `GFX_GUI_PROGRESS01_*` 9-slice art (0x14f..0x157), label at
    /// (3, 3); a last empty button (id -1) fills the rest.
    pub(super) fn draw_multi_header(&mut self, out: &mut Vec<DrawCmd>, hv: ViewId, rows: ViewId, rect: Rect, tint: [u8; 3], alpha: f32) {
        let Some(m) = self.multi_data(rows).cloned() else { return };
        let off = self.header_offset(hv);
        let art: [Option<GfxId>; 9] = [0x155, 0x157, 0x150, 0x152, 0x153, 0x156, 0x154, 0x151, 0x14f].map(|i| self.gfx.image(GfxId(i)).map(|_| GfxId(i)));
        out.push(DrawCmd::Clip(Some([rect.l as i32, rect.t as i32, rect.r as i32 + 1, rect.b as i32 + 1])));
        let mut end = 0.0f32;
        for (i, x, cw) in Self::multi_cols(&m) {
            let cell = Rect::new(rect.l + x - off, rect.t, rect.l + x - off + cw, rect.b);
            end = x + cw + 1.0 + COL_GAP;
            self.draw_border(out, &art, cell, tint, alpha);
            let label = self.fit_text(&m.cols[i].label, cw - 6.0);
            self.draw_string(out, FontId::Normal, &label, (cell.l + 3.0) as i32, (cell.t + 3.0) as i32, tint, alpha, false);
        }
        let filler = Rect::new(rect.l + end - off, rect.t, rect.r, rect.b);
        if filler.r > filler.l {
            self.draw_border(out, &art, filler, tint, alpha);
        }
        out.push(DrawCmd::Clip(None));
    }

    /// Right press: rows of lists and multi lists raise their mouse signal with button 2; the header opens its column menu.
    pub(super) fn widget_right_down(&mut self, x: f32, y: f32) {
        let Some((_, v)) = self.hit(x, y) else { return };
        match self.tree.views[v].kind {
            Kind::List(_) => self.list_down(v, x, y, 2),
            Kind::Multi(_) => self.multi_down(v, x, y, 2),
            Kind::MultiHeader { .. } => self.header_down(v, x, y, 2),
            _ => {}
        }
    }

    /// Left press dispatch of the widgets of this file.
    pub(super) fn widget_down(&mut self, v: ViewId, x: f32, y: f32) -> bool {
        match self.tree.views[v].kind {
            Kind::Dropdown(_) => self.dropdown_down(v),
            Kind::List(_) => self.list_down(v, x, y, 1),
            Kind::Multi(_) => self.multi_down(v, x, y, 1),
            Kind::MultiHeader { .. } => self.header_down(v, x, y, 1),
            _ => return false,
        }
        true
    }
}
