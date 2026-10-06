//! Data of the list widgets `DropdownMenu_c`, `ListViewBase_c` (`StringListViewItem_c`) and `MultiListView_c` (docs/gui.md §13).

use crate::geom::Point;

/// `DropdownMenu_c` (GUI.dll ctor 0x1012c30c, `Initialize` 0x1012bf10): a [`crate::view::Kind::Dropdown`] outer view (HLayoutNode) holding a
/// `BorderView` (`SetGfx(GFX_GUI_DROPDOWNMENU_LEFT/MIDDLE/RIGHT)`, client = the read-only label with client borders (6,3,6,3)) and the arrow
/// `BitmapView` (`AddBitmap(GFX_GUI_DROPDOWNMENU_CLOSED/OPEN)`). Items are `(Variant id, label)` records (0x30 bytes each in the original).
#[derive(Clone, Debug, Default)]
pub struct DropdownData {
    /// `(id, label)`; the Variant id of the original is an integer in every caller (LFT window).
    pub items: Vec<(i64, String)>,
    /// `+0x144` (-1 = none).
    pub selected: Option<usize>,
    /// `SetMaxStringWidth(w >= 0)` (`+0x140` = fixed): the label width no longer follows the items.
    pub fixed_width: Option<f32>,
}

/// One `StringListViewItem_c` (GUI.dll ctor 0x1014502d) inside a [`ListData`] tree (`ListViewBaseItem_c`, ctor 0x10132339).
#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// The item's Variant id (`+0x30`).
    pub id: String,
    pub label: String,
    /// `+0x90` / `+0x94`: gfx ids (0 = none). Plain items show `icon` while selected and `icon2` otherwise (when `icon2 > 0`); folders show `icon` while open
    /// and `icon2` while closed (`FUN_10144aab`).
    pub icon: u32,
    pub icon2: u32,
    /// `+0x40`: clicking toggles `open`.
    pub folder: bool,
    /// `+0x41`.
    pub open: bool,
    /// `+0x42` (`MakeSelectable`).
    pub selectable: bool,
    /// `+0x43`.
    pub selected: bool,
    /// `SetLabelColor(a, b)` (`+0x84`, `+0x88`): the label colour while selected or not selectable / while selectable and not selected.
    pub color_a: u32,
    pub color_b: u32,
    /// `FlashIcon` (`+0x8c`) / `HideIcon` (`+0x8d`).
    pub flash: bool,
    pub hide_icon: bool,
    /// `+0x58`: the indent the *children* of this item get (`_DAT_101b00e0` = 15).
    pub indent: f32,
    /// Item extent (`+0x48`, `+0x4c`: width, height as inclusive extents), set from the item view's preferred size (`SetSize`).
    pub size: Point,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

impl ListItem {
    /// `StringListViewItem_c(Variant, String, icon, icon2)` defaults.
    pub fn new(id: &str, label: &str, icon: u32, icon2: u32) -> ListItem {
        ListItem {
            id: id.into(),
            label: label.into(),
            icon,
            icon2,
            folder: false,
            open: false,
            selectable: true,
            selected: false,
            color_a: 0xffffff,
            color_b: 0xc0c0c0,
            flash: false,
            hide_icon: false,
            indent: 15.0,
            size: Point::new(0.0, 0.0),
            parent: None,
            children: vec![],
        }
    }
}

/// `ListViewBase_c` (GUI.dll ctor 0x1013163b): a tree of items; `items[0]` is the always-open root (`SetRoot`).
#[derive(Clone, Debug)]
pub struct ListData {
    pub items: Vec<ListItem>,
    /// `+0x154` (`MakeMultiSelect`); `+0x155` (select on mouse down) is always set by `Initialize` 0x130f7d.
    pub multi: bool,
    /// Content size in pixels from the last layout (`ListViewBase_c::GetContentSize`).
    pub content: Point,
}

impl Default for ListData {
    fn default() -> Self {
        let mut root = ListItem::new("", "", 0, 0);
        root.open = true;
        root.indent = 0.0;
        ListData { items: vec![root], multi: false, content: Point::new(0.0, 0.0) }
    }
}

/// How a column compares two rows (`MultiListViewItem_c` virtual `+4`, e.g. `LFTCandidateItem_c` 0x100ef4a1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiKey {
    /// `std::string::compare` of the text.
    Text,
    /// Integer compare.
    Num(i64),
    /// The column has no ordering (the compare returns 0).
    Equal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCell {
    pub text: String,
    pub key: MultiKey,
}

impl MultiCell {
    pub fn text(s: &str) -> Self {
        MultiCell { text: s.into(), key: MultiKey::Text }
    }
    pub fn num(n: i64) -> Self {
        MultiCell { text: n.to_string(), key: MultiKey::Num(n) }
    }
    pub fn unsorted(s: &str) -> Self {
        MultiCell { text: s.into(), key: MultiKey::Equal }
    }
}

/// One list-mode column (`MultiListView_c::AddColumn(id, label, width, flags)`, descriptor of 0x28 bytes at `+0x220`).
#[derive(Clone, Debug, PartialEq)]
pub struct MultiCol {
    pub id: i32,
    pub label: String,
    pub width: f32,
    /// bit 0 = hidden, bit 1 (2) = resizable, bit 2 (4) = has a label, bit 3 (8) = sortable.
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MultiRow {
    pub id: i64,
    pub cells: Vec<MultiCell>,
    pub selected: bool,
}

/// `MultiListView_c` in list layout mode (GUI.dll ctor 0x10136423, `LayoutList` 0x1013520e). The header is [`crate::view::Kind::MultiHeader`], the rows
/// are drawn by the [`crate::view::Kind::Multi`] view that owns this data.
#[derive(Clone, Debug)]
pub struct MultiData {
    pub cols: Vec<MultiCol>,
    pub rows: Vec<MultiRow>,
    /// `+0x150` feature flags: 0x40 selection allowed (single), 0x80 multiple selection.
    pub flags: u32,
    /// `+0x15c` / `+0x160` active list sort column (-1 = none) and order (false = ascending); `+0x16c` the rows are currently in sort order.
    pub sort_col: i32,
    pub sort_desc: bool,
    pub sorted: bool,
}

impl Default for MultiData {
    fn default() -> Self {
        MultiData { cols: vec![], rows: vec![], flags: 0, sort_col: -1, sort_desc: false, sorted: true }
    }
}
