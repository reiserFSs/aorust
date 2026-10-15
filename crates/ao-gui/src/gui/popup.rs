//! `PopupMenu_c`: the context / icon menus of the chat windows (docs/chat/gui.md §11).
//!
//! `PopupMenu_c::Go` 1013bf93 uses WINDOW border art over WINDOW_BACKGROUND, white tint,
//! alpha 1; item highlight 1013dbc0 is solid 2020c0. Boolean/submenu tails use 117/118.

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
    generation: u64,
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

/// BorderView::SetGfx in PopupMenu_c::Go (GUI 1013bf93).
pub(super) const BORDER: [u32; 9] = [0xf9, 0xfb, 0xf4, 0xf6, 0xf7, 0xfa, 0xf8, 0xf5, 0x1bf];

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
    /// Opens a popup menu and returns its generation. Replacing a menu queues its
    /// `MenuClosed` event; owners must ignore closures of older generations.
    pub fn open_menu(&mut self, at: (i32, i32), bounds: (i32, i32), items: Vec<MenuItem>) -> u64 {
        self.close_menu();
        self.ix.menu_generation += 1;
        let generation = self.ix.menu_generation;
        self.ix.menu = Some(Menu { generation, items, at, bounds, path: vec![], hover: None, drag: None });
        generation
    }

    /// Closes the current menu and queues its generation-scoped dismissal event.
    pub fn close_menu(&mut self) {
        if let Some(menu) = self.ix.menu.take() {
            self.widgets_sync();
            self.events.push(Event::MenuClosed { generation: menu.generation });
        }
    }

    pub fn menu_open(&self) -> bool {
        self.ix.menu.is_some()
    }

    /// Generation of the currently open popup, if any.
    pub fn menu_generation(&self) -> Option<u64> {
        self.ix.menu.as_ref().map(|menu| menu.generation)
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
                self.menu_picked(it.id);
                self.close_menu();
            }
        } else if !on {
            self.close_menu();
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
            self.events.push(Event::MenuSlider { id, value: v, finished: false });
        }
    }

    /// `Slider_c::MouseUp` (101443b3) emits the current value with finished=true, without closing the popup.
    pub(super) fn menu_mouse_up(&mut self) {
        if let Some(m) = &mut self.ix.menu {
            let Some((lv, i)) = m.drag.take() else { return };
            let Some(it) = menu_item_mut(m, lv, i) else { return };
            if let Some(value) = it.slider {
                self.events.push(Event::MenuSlider { id: it.id, value, finished: true });
            }
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
        let skin = BORDER.map(|id| Some(GfxId(id)));
        for (lv, pn) in panels.iter().enumerate() {
            self.draw_border(out, &skin, pn.rect, [255; 3], 1.0);
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
                    out.push(DrawCmd::Solid { dst: [r.l, r.t, r.r + 1.0, r.b + 1.0], color: [0x20, 0x20, 0xc0], alpha: 1.0 });
                }
                let tint = if it.enabled { [255; 3] } else { [0xa0; 3] };
                self.draw_string(out, FontId::Normal, &it.text, (r.l + COL + PAD - 2.0) as i32, r.t as i32 + 1, tint, 1.0, false);
                let tail = if it.checked == Some(true) { Some(GfxId(0x117)) } else if !it.sub.is_empty() { Some(GfxId(0x118)) } else { None };
                if let Some(id) = tail {
                    let (w, h) = self.gfx.size(id);
                    let x = r.r - w as f32 + 1.0;
                    let y = r.t + ((r.height() + 1.0 - h as f32) * 0.5).floor();
                    self.push_gfx(out, id, Rect::new(x, y, r.r, y + h as f32 - 1.0), if it.enabled { [255; 3] } else { [0x90; 3] }, 1.0);
                }
            }
        }
    }
}

fn m_path_has(m: &Option<Menu>, lv: usize, i: usize) -> bool {
    m.as_ref().is_some_and(|m| m.path.get(lv) == Some(&i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_uses_native_border_and_opaque_blue_highlight() {
        let Ok(mut gui) = Gui::new(&crate::client_dir(), None) else { return };
        gui.open_menu((10, 10), (800, 600), vec![MenuItem::entry(1, "Entry")]);
        gui.ix.menu.as_mut().unwrap().hover = Some((0, 0));
        let mut cmds = Vec::new();
        gui.draw_menu(&mut cmds);
        let rect = gui.menu_panels()[0].rect;
        let mut expected = Vec::new();
        gui.draw_border(&mut expected, &BORDER.map(|id| Some(GfxId(id))), rect, [255; 3], 1.0);
        assert!(!expected.is_empty());
        assert_eq!(format!("{:?}", &cmds[..expected.len()]), format!("{expected:?}"));
        assert!(cmds.iter().any(|cmd| matches!(cmd, DrawCmd::Solid { color: [0x20, 0x20, 0xc0], alpha: 1.0, .. })));
    }
}
