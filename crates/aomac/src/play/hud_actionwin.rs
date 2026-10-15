//! The Actions window `specialaction_window` (`SpecialActionView_c`, GUI.dll 0x100dc082; Ctrl+2, `WindowKind::Actions`). docs/gui.md §12.2.
//!
//! `DockableView` "Actions" around an `ItemListViewBase_c` grid: icon size 0 (32 px), cell `i` at `(i % 3, i / 3)` for the 21 positions of
//! `SpecialActions.xml`'s `item_position_map` (the default `IconPositionSystem` map type 5), view 167 x 167 (`_DAT_101bd710`), docked in the
//! `RollupArea`. The items are the entries of the own special-action list ([`SpecialList`], docs/gui.md §10.5) showing their current action; a click
//! (the release of a short press, as on the hotbar `FUN_10040a17`) is `N3Msg_PerformSpecialAction`, a press held for the mouse hold delay
//! (default 0.3 s, left button in new mouse mode / right in legacy mode) drags the action (`FUN_100410a0` / `FUN_1004173a`); dropping on a hotbar slot puts it there (`FUN_100d8047`).
//! Hosted by [`ListWindow`] / [`Rollup::open_page`](super::hud_rollup::Rollup::open_page).
//!
//! UNRESOLVED / not ported: the saved `item_position_map` (free rearranging by drag inside the window; the icons stay in list order).
//! Shift-click opens the identity's InfoView. Native right-click emits the unconnected +0x2e8 signal: there is no Actions item popup.

use super::hud::WindowKind;
use super::hud_bar::{self, ShortcutBar, SlotUse};
use super::hud_listview::{self as lv, ListView, ListWindow, MenuTexts, Mode, Row, Spec};
use super::hud_rollup::Rollup;
use super::hud_special::SpecialList;
use ao_gui::view::CanvasItem;
use ao_gui::{Gui, InputEvent, MouseButton, WindowSize};
use std::collections::HashMap;
use std::path::Path;

/// Rollup page key = the window's dvalue.
const KEY: &str = "specialaction_window";
/// Columns and cells of the position map.
const COLS: usize = 3;
const CELLS: usize = 21;
/// `SpecialActionView_c` size (`_DAT_101bd710`).
const VIEW: u32 = 167;
/// `MouseButtonHoldDelay` (see `hud_bar.rs`).
const HOLD_DELAY: f32 = 0.3;

struct Press {
    key: i32,
    held: f32,
    button: MouseButton,
    delay: Option<f32>,
}

struct Drag {
    instance: u32,
    ghost: ao_gui::WindowId,
    button: MouseButton,
}

pub(super) struct HudActionWin {
    store: Option<ao_rdb::RecordStore>,
    screen: (u32, u32),
    win: Option<ListWindow>,
    /// `(name, icon)` of the action templates shown so far (the textures are made once).
    cache: HashMap<u32, hud_bar::Slot>,
    /// What the grid was built from: per cell `(template, fade bits, optional signed timer seconds)`.
    built: Option<Vec<(u32, u32, Option<i32>)>>,
    press: Option<Press>,
    drag: Option<Drag>,
    mouse: (f32, f32),
    uses: Vec<SlotUse>,
    pub(super) hold_delay: f32,
    pub(super) new_mouse: bool,
    info_urls: Vec<String>,
}

impl HudActionWin {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        Self { store: ao_rdb::RecordStore::open(dir).ok(), screen, win: None, cache: HashMap::new(), built: None, press: None, drag: None, mouse: (0.0, 0.0), uses: vec![], hold_delay: HOLD_DELAY, new_mouse: true, info_urls: vec![] }
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        kind == WindowKind::Actions
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
    }

    pub(super) fn take_uses(&mut self) -> Vec<SlotUse> {
        std::mem::take(&mut self.uses)
    }

    pub(super) fn take_info_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.info_urls)
    }

    pub(super) fn open(&mut self, gui: &mut Gui, rollup: &mut Rollup) {
        if self.win.is_some() {
            return;
        }
        let mut view = ListView::new(Mode::Grid, vec![], COLS, CELLS / COLS, (0, false));
        // `MultiListView_c::RecalcCellCount` 0x101349f6 stretches the inter-icon gap to fill the view even at a fixed size
        // (the RollupArea page width never changes): the plain 10 px default gap left a 39 px blank strip at the 167 px view's
        // right edge (3 columns only need 128 px); the Programs/Actions "doesn't reflow with window width" bug is this, not
        // only the resizable NCU/Mission/Programs free windows (`hud_listview.rs::fill_spacing`).
        view.spacing = lv::fill_spacing(VIEW as f32, COLS, lv::ICON_32);
        let spec = Spec { name: "SpecialActionView", title: "Actions", pos: (0, 0), client: (VIEW, VIEW), top_xml: "", top_h: 0.0, texts: MenuTexts::default(), menu_base: 0x6800, screen: self.screen };
        match ListWindow::open(gui, spec, view, vec![], Some((rollup, KEY))) {
            Ok(w) => {
                self.win = Some(w);
                self.built = None;
            }
            Err(e) => eprintln!("hud: actions window: {e:#}"),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, rollup: &mut Rollup) {
        if self.win.take().is_some() {
            rollup.close_page(gui, KEY);
        }
        self.built = None;
        self.press = None;
        if let Some(d) = self.drag.take() {
            gui.close_window(d.ghost);
        }
    }

    /// Name and icon of an action template (`N3Msg_GetName`, stat `Icon`), cached.
    fn slot(&mut self, gui: &mut Gui, instance: u32) -> Option<&hud_bar::Slot> {
        if !self.cache.contains_key(&instance) {
            let s = hud_bar::special_action(gui, self.store.as_ref()?, instance)?;
            self.cache.insert(instance, s);
        }
        self.cache.get(&instance)
    }

    /// Per frame: the hold timer and grid; `timers` = (`IconTimers`, `IconTimerText`).
    pub(super) fn update(&mut self, gui: &mut Gui, list: &SpecialList, dt: f32, timers: (bool, bool)) {
        let mouse = self.mouse;
        if let Some(p) = &mut self.press {
            p.held += dt;
            if p.delay.is_some_and(|delay| p.held >= delay) {
                let (key, button) = (p.key, p.button);
                self.press = None;
                self.begin_drag(gui, key as u32, mouse, button);
            }
        }
        if self.win.is_none() {
            return;
        }
        let shown: Vec<(u32, f32, Option<i32>)> = list
            .entries()
            .iter()
            .filter_map(|e| e.template().map(|t| (t, e.shown)))
            .take(CELLS)
            .map(|(t, a)| {
                let progress = list.progress(a);
                let p = if list.disabled(a) { progress.filter(|_| timers.0).map_or(1.0, |(p, _)| p) } else { -1.0 };
                let seconds = progress.filter(|(p, _)| timers.1 && *p != 1.0).map(|(_, seconds)| seconds);
                (t, p, seconds)
            })
            .collect();
        let sig: Vec<(u32, u32, Option<i32>)> = shown.iter().map(|&(t, p, seconds)| (t, p.to_bits(), seconds)).collect();
        if self.built.as_ref() == Some(&sig) {
            return;
        }
        let mut rows = vec![];
        let mut fades = vec![];
        let mut labels = vec![];
        for &(t, p, seconds) in &shown {
            let Some(s) = self.slot(gui, t) else { continue };
            rows.push(Row { key: t as i32, icon: s.icon, cells: vec![], tip_title: s.name.clone(), tip_body: String::new() });
            if let Some(seconds) = seconds { labels.push((t as i32, super::hud_special::timer_text(seconds))); }
            if p >= 0.0 {
                fades.push((t as i32, p));
            }
        }
        let win = self.win.as_mut().expect("open");
        win.view.set_fades(fades);
        win.view.set_icon_labels(labels);
        win.view.set_rows(rows);
        win.view.paint(gui, win.win, lv::ICON_32);
        self.built = Some(sig);
    }

    pub(super) fn update_item_tooltip(&mut self, gui: &mut Gui, zone: &super::zone::Zone, stats: &super::hud_stats::HudStats, descriptions: bool) {
        if let Some(w) = &mut self.win {
            w.view.update_item_tooltip(gui, w.win, lv::ICON_32, descriptions, |id| stats.tooltip_body(zone, ao_net::msg::Identity { kind: 0xdeb0, instance: id }));
        }
    }

    fn begin_drag(&mut self, gui: &mut Gui, instance: u32, (x, y): (f32, f32), button: MouseButton) {
        let icon = self.cache.get(&instance).and_then(|s| s.icon);
        let src = "<root><CanvasView name=\"icon\" min_size=\"Point(31,31)\" max_size=\"Point(31,31)\"/></root>";
        let Ok(ghost) = gui.open_window_xml("ActionDrag", src, (x as i32 - 16, y as i32 - 16), WindowSize::Preferred) else { return };
        if let Some((g, w, h)) = icon {
            gui.set_canvas(ghost, "icon", vec![CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, lv::ICON_32, lv::ICON_32], alpha: 1.0 }]);
        }
        self.drag = Some(Drag { instance, ghost, button });
    }

    /// The action icon under a screen point, inside the window.
    fn row_at(&self, gui: &Gui, x: f32, y: f32) -> Option<i32> {
        let win = self.win.as_ref()?;
        let (px, py) = gui.window_pos(win.win);
        let (w, h) = gui.window_size(win.win);
        if x < px as f32 || y < py as f32 || x >= (px + w as i32) as f32 || y >= (py + h as i32) as f32 {
            return None;
        }
        win.view.row_at(gui, win.win, x, y, lv::ICON_32)
    }

    /// Press / hold / release of the grid; a drop on a hotbar slot goes to `bars`.
    pub(super) fn input(&mut self, gui: &mut Gui, ev: &InputEvent, list: &SpecialList, mods: ao_gui::Modifiers, bars: &mut [ShortcutBar]) {
        match *ev {
            InputEvent::MouseDown { x, y, .. } if !self.win.as_ref().is_some_and(|w| gui.pointer_owner(x, y) == Some(w.win)) => {}
            InputEvent::MouseDown { x, y, button: button @ (MouseButton::Left | MouseButton::Right) } => {
                self.mouse = (x, y);
                let drag_button = if self.new_mouse { MouseButton::Left } else { MouseButton::Right };
                self.press = self.row_at(gui, x, y).map(|key| Press { key, held: 0.0, button, delay: (button == drag_button).then_some(self.hold_delay) });
            }
            InputEvent::MouseMove { x, y } => {
                self.mouse = (x, y);
                if let Some(d) = &self.drag {
                    gui.set_window_pos(d.ghost, (x as i32 - 16, y as i32 - 16));
                }
            }
            InputEvent::MouseUp { x, y, button: button @ (MouseButton::Left | MouseButton::Right) } => {
                if self.drag.as_ref().is_some_and(|d| d.button != button) || self.press.as_ref().is_some_and(|p| p.button != button) { return; }
                if let Some(d) = self.drag.take() {
                    gui.close_window(d.ghost);
                    for b in bars.iter_mut() {
                        if b.drop_special(gui, d.instance, x, y) {
                            break;
                        }
                    }
                } else if let Some(p) = self.press.take() {
                    if button == MouseButton::Left && self.row_at(gui, x, y) == Some(p.key) {
                        if mods.shift {
                            self.info_urls.push(format!("itemid://{}/{}", 0xdeb0, p.key));
                        } else {
                            self.uses.push(list.find(p.key as u32).map_or(SlotUse::Unavailable, |e| SlotUse::SpecialAction(e.shown)));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    pub(super) fn shown(&self) -> Vec<i32> {
        self.win.as_ref().map(|w| w.view.rows().iter().map(|r| r.key).collect()).unwrap_or_default()
    }

    /// Screen position of grid cell `i`'s centre (tests): the same `cell_pos` the real click hit-test (`ListView::row_at`)
    /// uses, so this can never drift from the actual rendered layout the way a separately hard-coded pitch did.
    #[cfg(test)]
    pub(super) fn cell_centre(&self, gui: &Gui, i: usize) -> Option<(f32, f32)> {
        let win = self.win.as_ref()?;
        let r = gui.view_rect(win.win, "grid")?;
        let (cx, cy) = win.view.cell_pos(i, lv::ICON_32);
        Some((r.l + cx + lv::ICON_32 / 2.0, r.t + cy + lv::ICON_32 / 2.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_click_uses_action_while_shift_click_does_not() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, (1280, 800));
        let mut actions = HudActionWin::new(&dir, (1280, 800));
        actions.open(&mut gui, &mut rollup);
        let list = SpecialList::new();
        let entry = &list.entries()[0];
        let win = actions.win.as_mut().unwrap();
        win.view.set_rows(vec![Row { key: entry.template().unwrap() as i32, ..Default::default() }]);
        win.view.paint(&mut gui, win.win, lv::ICON_32);
        let (x, y) = actions.cell_centre(&gui, 0).unwrap();
        for new_mouse in [false, true] {
            actions.new_mouse = new_mouse;
            for (shift, ctrl) in [(false, false), (false, true), (true, false), (true, true)] {
                let mods = ao_gui::Modifiers { shift, ctrl, ..Default::default() };
                actions.input(&mut gui, &InputEvent::MouseDown { x, y, button: MouseButton::Left }, &list, mods, &mut []);
                assert!(actions.press.is_some());
                assert!(actions.take_uses().is_empty(), "use waits for release");
                actions.input(&mut gui, &InputEvent::MouseUp { x, y, button: MouseButton::Left }, &list, mods, &mut []);
                assert_eq!(actions.take_uses(), if shift { vec![] } else { vec![SlotUse::SpecialAction(entry.shown)] });
                assert_eq!(actions.take_info_urls(), if shift { vec![format!("itemid://{}/{}", 0xdeb0, entry.template().unwrap())] } else { vec![] });
                assert!(actions.press.is_none());
            }
        }
    }

    #[test]
    fn action_recharge_labels_follow_text_setting_independently_of_fader() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, (1280, 800));
        let mut actions = HudActionWin::new(&dir, (1280, 800));
        actions.open(&mut gui, &mut rollup);
        let mut list = SpecialList::new();
        list.sync_equipment(&[(0x800, -1)], 0);
        list.sync_recharge([super::super::hud_special::Recharge { action: 0x94, total: 30, remaining: 29 }].into_iter(), std::iter::empty());
        actions.update(&mut gui, &list, 0.0, (false, true));
        let window = actions.win.as_ref().unwrap().win;
        let cell = actions.shown().iter().position(|id| *id == 0x14123).unwrap();
        let label = format!("lv_icon_label{cell}");
        assert_eq!(gui.text(window, &label), super::super::hud_special::timer_text(29));
        for cell in 0..actions.shown().len() {
            let (x, y) = actions.cell_centre(&gui, cell).unwrap();
            let frame = gui.view_rect(window, &format!("lv_icon_label{cell}")).unwrap();
            assert!((frame.l + frame.r - 2.0 * x + 1.0).abs() <= 1.0, "cell{cell}: label{frame:?}, icon centre({x},{y})");
            assert!((frame.t + frame.b - 2.0 * y + 1.0).abs() <= 1.0, "cell{cell}: label{frame:?}, icon centre({x},{y})");
        }
        actions.update(&mut gui, &list, 0.0, (true, false));
        assert!(gui.text(window, &label).is_empty());
        list.sync_recharge([super::super::hud_special::Recharge { action: 0x94, total: 30, remaining: 30 }].into_iter(), std::iter::empty());
        actions.update(&mut gui, &list, 0.0, (true, true));
        assert!(gui.text(window, &label).is_empty(), "native progress 1.0 has no timer text, even while locked");
        list.set_disabled(3, true);
        actions.update(&mut gui, &list, 0.0, (false, true));
        assert_eq!(actions.built.as_ref().unwrap()[0], (0xc1a3, 1.0f32.to_bits(), None), "flag-only action has a full fader without timer text");
        list.set_disabled(3, false);
        actions.update(&mut gui, &list, 0.0, (true, true));
        assert_eq!(actions.built.as_ref().unwrap()[0], (0xc1a3, (-1.0f32).to_bits(), None));
        list.sync_recharge([super::super::hud_special::Recharge { action: 0x94, total: 30, remaining: -1 }].into_iter(), std::iter::empty());
        actions.update(&mut gui, &list, 0.0, (true, true));
        assert_eq!(gui.text(window, &label), super::super::hud_special::timer_text(-1), "signed seconds never serve as a hidden-label sentinel");
        actions.close(&mut gui, &mut rollup);
    }
}
