//! The grid of the inventory window: `MultiListView_c::RecalcCellCount` (GUI 0x101349f6) over the defaults of its ctor (GUI 0x10136423) and the
//! client side icon positions (`item_position_map`, `IconPositionMap_t`). Docs: docs/gui.md §11.12.
//!
//! The inventory view never calls `SetViewCellCounts` / `SetGridIconSpacing` / `SetGridIconSize` (searched: callers of those three in GUI.dll
//! are the shortcut bar, the wear views, the nano / perk views; none in `0x100c9000..0x100ce500`). The grid therefore uses the ctor defaults:
//! icon `Point(47, 47)` (`_DAT_101b0288`), default spacing `Point(10, 10)` (`_DAT_101a98e4`), view borders `Rect(6, 6, 6, 6)` (`_DAT_101b5234`)
//! and the cell pitch `47 + 10 + 1` = 58 for the column count; flags `0` (the view is created with `FUN_100cc1b3(.., 0)`), so the horizontal
//! spacing is recomputed to fill the width (`flags & 0x10 == 0`).

/// Icon size of the grid (`+0x180`); a cell is this plus one pixel wide.
pub const ICON: f32 = 47.0;
/// Default spacing (`+0x188`), also the minimum used for the column count.
pub const SPACING: f32 = 10.0;
/// View borders (`+0x170..+0x17c`).
pub const BORDER: f32 = 6.0;
/// `SetMaxItemCount(0x1e)` of the own inventory.
pub const MAX_ITEMS: usize = 30;

/// Column count and horizontal spacing for a grid view whose `Rect::Width` is `width` (inclusive pixels - 1).
/// `RecalcCellCount`: `cols = max(1, trunc(width) / (ICON + SPACING + 1))`; with two or more columns
/// `spacing = floor((width + 1 - BORDER - BORDER - cols * (ICON + 1)) / (cols - 1))`, otherwise the default spacing.
pub fn columns(width: f32) -> (usize, f32) {
    let cols = ((width.trunc() / (ICON + SPACING + 1.0)).floor() as i64).max(1) as usize;
    if cols < 2 {
        return (cols, SPACING);
    }
    let gap = ((width + 1.0 - 2.0 * BORDER - cols as f32 * (ICON + 1.0)) / (cols as f32 - 1.0)).floor();
    (cols, gap.max(0.0))
}

/// Top-left corner of grid cell (`col`, `row`) in the view: `GridPosToViewPos` = `pos * (cell pitch) + border` with the pitch
/// `ICON + 1 + spacing` (`+0x198` after `RecalcCellCount`).
pub fn cell_origin(col: usize, row: usize, gap: (f32, f32)) -> (f32, f32) {
    (BORDER + col as f32 * (ICON + 1.0 + gap.0), BORDER + row as f32 * (ICON + 1.0 + gap.1))
}

/// Client side placement of the bag items: item identity instance (the bag slot) -> grid cell (`IPoint` column, row). Items without a saved cell
/// take the first free one (row-major; `MultiListView_c::GetFirstFreePos`, not decompiled: UNRESOLVED whether it scans row-major).
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct PositionMap {
    cells: std::collections::HashMap<u32, (usize, usize)>,
}

impl PositionMap {
    /// Keeps the cells of the `slots` that are still present, drops the others and places the new ones in the first free cell of a `cols`-wide grid;
    /// the first new item takes `prefer` (the cell an item was dropped on, `FUN_100cbc21` stores it in the view) when that is free.
    pub fn sync(&mut self, slots: &[u32], cols: usize, prefer: &mut Option<(usize, usize)>) {
        self.cells.retain(|s, _| slots.contains(s));
        for &s in slots {
            if !self.cells.contains_key(&s) {
                let taken = |c: &(usize, usize), m: &Self| m.cells.values().any(|v| v == c);
                let free = match prefer.take().filter(|c| !taken(c, self)) {
                    Some(c) => c,
                    None => (0usize..).map(|i| (i % cols, i / cols)).find(|c| !taken(c, self)).expect("endless iterator"),
                };
                self.cells.insert(s, free);
            }
        }
    }

    pub fn get(&self, slot: u32) -> Option<(usize, usize)> {
        self.cells.get(&slot).copied()
    }

    pub fn at(&self, cell: (usize, usize)) -> Option<u32> {
        self.cells.iter().find(|(_, v)| **v == cell).map(|(s, _)| *s)
    }

    /// `MultiListView_c::MoveItem(from, to)`: the item of `from` goes to `to`; an item already there goes to `from` (the original starts a new
    /// drag with the displaced item, `FUN_100cbc21`; we swap).
    pub fn move_cell(&mut self, from: (usize, usize), to: (usize, usize)) {
        let other = self.at(to);
        if let Some(s) = self.at(from) {
            self.cells.insert(s, to);
            if let Some(o) = other {
                self.cells.insert(o, from);
            }
        }
    }

    /// An item changes its bag slot (server `ContainerAddItemIIR_t`): its cell follows the identity of the new slot.
    pub fn rename(&mut self, from: u32, to: u32) {
        if let Some(c) = self.cells.remove(&from) {
            self.cells.insert(to, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The saved frame of the inventory (`DockArea0.xml`: `Rect(1196, 636, 1382, 811)` = 187 px wide, `Rect::Width` 186) minus the style-0 frame's
    /// client insets (5 + 5) gives a view of `Rect::Width` 176: 3 columns (matching the `i % 3` of `item_position_map` in `FUN_100cc2ca`, and
    /// the retail screenshot), spacing floor(21 / 2) = 10.
    #[test]
    fn default_window_is_three_columns_with_ten_pixels() {
        assert_eq!(columns(176.0), (3, 10.0));
        // narrower: two columns; one column keeps the default spacing; wider windows add columns
        assert_eq!(columns(116.0).0, 2);
        assert_eq!(columns(57.0), (1, SPACING));
        assert_eq!(columns(0.0).0, 1);
        assert_eq!(columns(58.0 * 5.0).0, 5);
    }

    #[test]
    fn positions_are_first_free_and_follow_moves() {
        let mut m = PositionMap::default();
        let mut none = None;
        m.sync(&[0x40, 0x41, 0x43, 0x44], 3, &mut none);
        assert_eq!((m.get(0x40), m.get(0x41), m.get(0x43), m.get(0x44)), (Some((0, 0)), Some((1, 0)), Some((2, 0)), Some((0, 1))));
        // an item leaves, the next one takes the hole; existing items keep their cell
        m.sync(&[0x40, 0x43, 0x44, 0x45], 3, &mut none);
        assert_eq!((m.get(0x43), m.get(0x45)), (Some((2, 0)), Some((1, 0))));
        // a drop target is used by the next new item, once
        let mut prefer = Some((2, 3));
        m.sync(&[0x40, 0x43, 0x44, 0x45, 0x46, 0x47], 3, &mut prefer);
        assert_eq!((m.get(0x46), prefer), (Some((2, 3)), None));
        assert_eq!(m.get(0x47), Some((1, 1)));
        m.sync(&[0x40, 0x43, 0x44, 0x45], 3, &mut none);
        m.move_cell((2, 0), (1, 2));
        assert_eq!((m.get(0x43), m.at((2, 0))), (Some((1, 2)), None));
        m.move_cell((0, 0), (1, 2));
        assert_eq!((m.get(0x40), m.get(0x43)), (Some((1, 2)), Some((0, 0))));
        m.rename(0x40, 0x50);
        assert_eq!((m.get(0x40), m.get(0x50)), (None, Some((1, 2))));
    }
}
