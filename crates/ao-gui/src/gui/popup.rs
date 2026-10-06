//! `PopupMenu_c`: the context / icon menus of the chat windows (docs/chat/gui.md §11).
//!
//! RE: entries `PopupMenu_c::AppendMenuEntry(Variant, text, -1, submenu)`, `AppendSeparatorEntry`, `PopupMenuItem_c::MakeBoolean` + `Check` (a check mark),
//! `View::Enable(item, false)` (greyed). **UNRESOLVED**: the menu skin (the combo-box popup's raised border art is reused), the check mark art
//! (a 5 px square is drawn) and the sub-menu arrow glyph.

use super::*;

/// One entry of a popup menu.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuItem {
    /// Reported by [`Event::MenuPicked`].
    pub id: u32,
    pub text: String,
    /// `Some(on)` = a boolean item (`MakeBoolean` + `Check`).
    pub checked: Option<bool>,
    pub enabled: bool,
    pub sep: bool,
    /// Non-empty = a sub-menu opens beside the item.
    pub sub: Vec<MenuItem>,
    /// `Some(v)` = a slider item (0.0..=1.0).
    pub slider: Option<f32>,
}

impl MenuItem {
    pub fn entry(id: u32, text: &str) -> Self {
        MenuItem { id, text: text.into(), enabled: true, ..Default::default() }
    }
    pub fn check(id: u32, text: &str, on: bool) -> Self {
        MenuItem { checked: Some(on), ..Self::entry(id, text) }
    }
    pub fn separator() -> Self {
        MenuItem { sep: true, ..Default::default() }
    }
    pub fn submenu(text: &str, sub: Vec<MenuItem>) -> Self {
        MenuItem { sub, ..Self::entry(0, text) }
    }
    /// A `PopupMenuSliderItem_c` with value `v` in 0.0..=1.0; changes arrive as [`Event::MenuSlider`].
    pub fn slider(id: u32, v: f32) -> Self {
        MenuItem { slider: Some(v), ..Self::entry(id, "") }
    }
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

pub(super) struct Menu {
    items: Vec<MenuItem>,
    at: (i32, i32),
    bounds: (i32, i32),
    /// Indices of the open sub-menu chain (`path[k]` = the item of level k whose sub-menu is shown).
    path: Vec<usize>,
    hover: Option<(usize, usize)>,
    /// Slider item being dragged: (level, item).
    drag: Option<(usize, usize)>,
}

fn menu_item_mut(m: &mut Menu, lv: usize, i: usize) -> Option<&mut MenuItem> {
    let mut items = &mut m.items;
    for &p in m.path.iter().take(lv) {
        items = &mut items.get_mut(p)?.sub;
    }
    items.get_mut(i)
}

const PAD: f32 = 6.0;
/// Left column of check marks / right column of sub-menu arrows.
const COL: f32 = 14.0;
const SEP_H: f32 = 5.0;

/// A laid-out panel: its rectangle and the rectangle of every item (with the item's level index).
struct Panel {
    rect: Rect,
    items: Vec<(Rect, usize)>,
}

impl Gui {
    /// Opens a popup menu with its top-left at `at` (kept inside `bounds` = the screen size). Replaces an open one.
    pub fn open_menu(&mut self, at: (i32, i32), bounds: (i32, i32), items: Vec<MenuItem>) {
        self.ix.menu = Some(Menu { items, at, bounds, path: vec![], hover: None, drag: None });
    }

    pub fn close_menu(&mut self) {
        self.ix.menu = None;
    }

    pub fn menu_open(&self) -> bool {
        self.ix.menu.is_some()
    }

    fn menu_levels(m: &Menu) -> Vec<&[MenuItem]> {
        let mut v: Vec<&[MenuItem]> = vec![&m.items];
        for &i in &m.path {
            match v.last().and_then(|l| l.get(i)) {
                Some(it) if !it.sub.is_empty() => v.push(&it.sub),
                _ => break,
            }
        }
        v
    }

    fn menu_panels(&mut self) -> Vec<Panel> {
        let Some(m) = &self.ix.menu else { return vec![] };
        let levels: Vec<Vec<MenuItem>> = Self::menu_levels(m).into_iter().map(|l| l.to_vec()).collect();
        let (at, bounds) = (m.at, m.bounds);
        let path = m.path.clone();
        let fh = self.fonts.font(FontId::Normal).height as f32 + 2.0;
        let mut panels: Vec<Panel> = vec![];
        for (lv, items) in levels.iter().enumerate() {
            let slider_w = if items.iter().any(|i| i.slider.is_some()) { 120.0 } else { 0.0 };
            let w = (items.iter().map(|i| i.text.chars().map(|c| self.fonts.font(FontId::Normal).advance(c)).sum::<i32>()).max().unwrap_or(0) as f32 + 2.0 * PAD + 2.0 * COL).max(slider_w);
            let h: f32 = items.iter().map(|i| if i.sep { SEP_H } else { fh }).sum::<f32>() + 4.0;
            let (mut x, mut y) = match panels.last() {
                None => (at.0 as f32, at.1 as f32),
                Some(p) => {
                    let pi = path[lv - 1];
                    let ir = p.items.iter().find(|(_, i)| *i == pi).map_or(p.rect, |(r, _)| *r);
                    (p.rect.r + 1.0, ir.t - 2.0)
                }
            };
            if x + w > bounds.0 as f32 {
                x = match panels.last() {
                    Some(p) => (p.rect.l - w).max(0.0),
                    None => (bounds.0 as f32 - w).max(0.0),
                };
            }
            if y + h > bounds.1 as f32 {
                y = (bounds.1 as f32 - h).max(0.0);
            }
            let rect = Rect::new(x, y, x + w - 1.0, y + h - 1.0);
            let mut cy = y + 2.0;
            let mut rs = vec![];
            for (i, it) in items.iter().enumerate() {
                let ih = if it.sep { SEP_H } else { fh };
                rs.push((Rect::new(x + 2.0, cy, x + w - 3.0, cy + ih - 1.0), i));
                cy += ih;
            }
            panels.push(Panel { rect, items: rs });
        }
        panels
    }

    /// (level, item) under the pointer.
    fn menu_item_at(&mut self, x: f32, y: f32) -> Option<(usize, usize)> {
        let p = Point::new(x, y);
        let panels = self.menu_panels();
        for (lv, pn) in panels.iter().enumerate().rev() {
            if let Some((_, i)) = pn.items.iter().find(|(r, _)| r.contains(p)) {
                return Some((lv, *i));
            }
        }
        None
    }

    fn menu_on_panel(&mut self, x: f32, y: f32) -> bool {
        let p = Point::new(x, y);
        self.menu_panels().iter().any(|pn| pn.rect.contains(p))
    }

    /// Left press while a menu is open. Always consumed.
    pub(super) fn menu_mouse_down(&mut self, x: f32, y: f32) {
        let hit = self.menu_item_at(x, y);
        let on = self.menu_on_panel(x, y);
        let Some(m) = &self.ix.menu else { return };
        if let Some((lv, i)) = hit {
            let it = Self::menu_levels(m)[lv][i].clone();
            if it.slider.is_some() && it.enabled {
                if let Some(m) = &mut self.ix.menu {
                    m.drag = Some((lv, i));
                }
                self.menu_slide(x);
            } else if it.enabled && !it.sep && it.sub.is_empty() {
                self.events.push(Event::MenuPicked { id: it.id });
                self.ix.menu = None;
            }
        } else if !on {
            self.ix.menu = None;
        }
    }

    /// Drag of a slider item: the value follows the pointer across the item (`PopupMenuSliderItem_c`), [`Event::MenuSlider`] on every change.
    fn menu_slide(&mut self, x: f32) {
        let Some((lv, i)) = self.ix.menu.as_ref().and_then(|m| m.drag) else { return };
        let panels = self.menu_panels();
        let Some(r) = panels.get(lv).and_then(|p| p.items.iter().find(|(_, k)| *k == i)).map(|(r, _)| *r) else { return };
        let v = ((x - (r.l + 4.0)) / (r.width() - 8.0)).clamp(0.0, 1.0);
        let Some(m) = &mut self.ix.menu else { return };
        let Some(it) = menu_item_mut(m, lv, i) else { return };
        if it.slider != Some(v) {
            it.slider = Some(v);
            let id = it.id;
            self.events.push(Event::MenuSlider { id, value: v });
        }
    }

    pub(super) fn menu_mouse_up(&mut self) {
        if let Some(m) = &mut self.ix.menu {
            m.drag = None;
        }
    }

    pub(super) fn menu_mouse_move(&mut self, x: f32, y: f32) {
        if self.ix.menu.as_ref().is_some_and(|m| m.drag.is_some()) {
            self.menu_slide(x);
            return;
        }
        let hit = self.menu_item_at(x, y);
        let Some(m) = &mut self.ix.menu else { return };
        m.hover = hit;
        if let Some((lv, i)) = hit {
            m.path.truncate(lv);
            let levels = Self::menu_levels(m);
            let has_sub = levels.get(lv).and_then(|l| l.get(i)).is_some_and(|it| it.enabled && !it.sub.is_empty());
            if has_sub {
                m.path.push(i);
            }
        }
    }

    pub(super) fn draw_menu(&mut self, out: &mut Vec<DrawCmd>) {
        let panels = self.menu_panels();
        let Some(m) = &self.ix.menu else { return };
        let levels: Vec<Vec<MenuItem>> = Self::menu_levels(m).into_iter().map(|l| l.to_vec()).collect();
        let hover = m.hover;
        let col = self.map_color(0x1000000);
        // UNRESOLVED: PopupMenu_c skin; the raised button border set, like the combo popup
        let raised: [Option<GfxId>; 9] = [0x24, 0x26, 0x1f, 0x21, 0x22, 0x25, 0x23, 0x20, 0x1e].map(|i| Some(GfxId(i)));
        for (lv, pn) in panels.iter().enumerate() {
            self.draw_border(out, &raised, pn.rect, col, 0.95);
            for (r, i) in &pn.items {
                let it = &levels[lv][*i];
                if it.sep {
                    out.push(DrawCmd::Solid { dst: [r.l + 2.0, r.t + 2.0, r.r - 1.0, r.t + 3.0], color: [0x80; 3], alpha: 1.0 });
                    continue;
                }
                if let Some(v) = it.slider {
                    // `PopupMenuSliderItem_c` (0.0..1.0): UNRESOLVED skin, drawn as a bar with a knob
                    let (l, r2, mid) = (r.l + 4.0, r.r - 4.0, (r.t + r.b) * 0.5);
                    out.push(DrawCmd::Solid { dst: [l, mid - 1.0, r2, mid + 1.0], color: [0x80; 3], alpha: 1.0 });
                    let kx = l + (r2 - l) * v.clamp(0.0, 1.0);
                    out.push(DrawCmd::Solid { dst: [kx - 2.0, r.t + 2.0, kx + 2.0, r.b - 1.0], color: [255; 3], alpha: 1.0 });
                    continue;
                }
                if it.enabled && hover == Some((lv, *i)) || (it.enabled && !it.sub.is_empty() && m_path_has(&self.ix.menu, lv, *i)) {
                    out.push(DrawCmd::Solid { dst: [r.l, r.t, r.r + 1.0, r.b + 1.0], color: rgb(self.palette[2]), alpha: 0.5 });
                }
                let tint = if it.enabled { [255; 3] } else { [0x90; 3] };
                if it.checked == Some(true) {
                    out.push(DrawCmd::Solid { dst: [r.l + 4.0, r.t + 4.0, r.l + 9.0, r.t + 9.0], color: tint, alpha: 1.0 });
                }
                self.draw_string(out, FontId::Normal, &it.text, (r.l + COL + PAD - 2.0) as i32, r.t as i32 + 1, tint, 1.0, false);
                if !it.sub.is_empty() {
                    self.draw_string(out, FontId::Normal, ">", (r.r - COL + 4.0) as i32, r.t as i32 + 1, tint, 1.0, false);
                }
            }
        }
    }
}

fn m_path_has(m: &Option<Menu>, lv: usize, i: usize) -> bool {
    m.as_ref().is_some_and(|m| m.path.get(lv) == Some(&i))
}
