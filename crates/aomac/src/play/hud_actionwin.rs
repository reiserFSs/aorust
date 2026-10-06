//! The Actions window `specialaction_window` (`SpecialActionView_c`, GUI.dll 0x100dc082; Ctrl+2, `WindowKind::Actions`). docs/gui.md §12.2.
//!
//! `DockableView` "Actions" around an `ItemListViewBase_c` grid: icon size 0 (32 px), cell `i` at `(i % 3, i / 3)` for the 21 positions of
//! `SpecialActions.xml`'s `item_position_map` (the default `IconPositionSystem` map type 5), view 167 x 167 (`_DAT_101bd710`), docked in the
//! `RollupArea`. The items are the entries of the own special-action list ([`SpecialList`], docs/gui.md §10.5) showing their current action; a click
//! (the release of a short press, as on the hotbar `FUN_10040a17`) is `N3Msg_PerformSpecialAction`, a press held for the mouse hold delay
//! (0.3 s) drags the action (its `FUN_100410a0` / `FUN_1004173a`) and a drop on a hotbar slot puts it there (`FUN_100d8047`).
//! Hosted by [`ListWindow`] / [`Rollup::open_page`](super::hud_rollup::Rollup::open_page).
//!
//! UNRESOLVED / not ported: the saved `item_position_map` (free rearranging by drag inside the window; the icons stay in list order), the info
//! signal (`itemid://k/i`, Shift / Ctrl click: no InfoView here, the click does nothing), the recharge seconds text over an icon (the grid canvas
//! has no text item; the overlay shrinks like the hotbar's), the right-click popup.

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
}

struct Drag {
    instance: u32,
    ghost: ao_gui::WindowId,
}

pub(super) struct HudActionWin {
    store: Option<ao_rdb::RecordStore>,
    screen: (u32, u32),
    win: Option<ListWindow>,
    /// `(name, icon)` of the action templates shown so far (the textures are made once).
    cache: HashMap<u32, hud_bar::Slot>,
    /// What the grid was built from: per cell `(template, fade bits)`.
    built: Option<Vec<(u32, u32)>>,
    press: Option<Press>,
    drag: Option<Drag>,
    mouse: (f32, f32),
    uses: Vec<SlotUse>,
}

impl HudActionWin {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        Self { store: ao_rdb::RecordStore::open(dir).ok(), screen, win: None, cache: HashMap::new(), built: None, press: None, drag: None, mouse: (0.0, 0.0), uses: vec![] }
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

    pub(super) fn open(&mut self, gui: &mut Gui, rollup: &mut Rollup) {
        if self.win.is_some() {
            return;
        }
        let view = ListView::new(Mode::Grid, vec![], COLS, CELLS / COLS, (0, false));
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

    /// Per frame: the hold timer and the grid (entries in list order, the recharge overlay of each). `timers` = `IconTimers`.
    pub(super) fn update(&mut self, gui: &mut Gui, list: &SpecialList, dt: f32, timers: bool) {
        let mouse = self.mouse;
        if let Some(p) = &mut self.press {
            p.held += dt;
            if p.held >= HOLD_DELAY {
                let key = p.key;
                self.press = None;
                self.begin_drag(gui, key as u32, mouse);
            }
        }
        if self.win.is_none() {
            return;
        }
        let shown: Vec<(u32, u32, f32)> = list
            .entries()
            .iter()
            .filter_map(|e| e.template().map(|t| (t, e.shown)))
            .take(CELLS)
            .map(|(t, a)| {
                let p = list.progress(a).map_or(-1.0, |(p, _)| if timers { p } else { 1.0 });
                (t, a, p)
            })
            .collect();
        let sig: Vec<(u32, u32)> = shown.iter().map(|&(t, _, p)| (t, p.to_bits())).collect();
        if self.built.as_ref() == Some(&sig) {
            return;
        }
        let mut rows = vec![];
        let mut fades = vec![];
        for &(t, _, p) in &shown {
            let Some(s) = self.slot(gui, t) else { continue };
            rows.push(Row { key: t as i32, icon: s.icon, cells: vec![], tip_title: s.name.clone(), tip_body: String::new() });
            if p >= 0.0 {
                fades.push((t as i32, p));
            }
        }
        let win = self.win.as_mut().expect("open");
        win.view.set_fades(fades);
        win.view.set_rows(rows);
        win.view.paint(gui, win.win, lv::ICON_32);
        self.built = Some(sig);
    }

    fn begin_drag(&mut self, gui: &mut Gui, instance: u32, (x, y): (f32, f32)) {
        let icon = self.cache.get(&instance).and_then(|s| s.icon);
        let src = "<root><CanvasView name=\"icon\" min_size=\"Point(31,31)\" max_size=\"Point(31,31)\"/></root>";
        let Ok(ghost) = gui.open_window_xml("ActionDrag", src, (x as i32 - 16, y as i32 - 16), WindowSize::Preferred) else { return };
        if let Some((g, w, h)) = icon {
            gui.set_canvas(ghost, "icon", vec![CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, lv::ICON_32, lv::ICON_32], alpha: 1.0 }]);
        }
        self.drag = Some(Drag { instance, ghost });
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
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                self.mouse = (x, y);
                self.press = self.row_at(gui, x, y).map(|key| Press { key, held: 0.0 });
            }
            InputEvent::MouseMove { x, y } => {
                self.mouse = (x, y);
                if let Some(d) = &self.drag {
                    gui.set_window_pos(d.ghost, (x as i32 - 16, y as i32 - 16));
                }
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                if let Some(d) = self.drag.take() {
                    gui.close_window(d.ghost);
                    for b in bars.iter_mut() {
                        if b.drop_special(gui, d.instance, x, y) {
                            break;
                        }
                    }
                } else if let Some(p) = self.press.take() {
                    if self.row_at(gui, x, y) == Some(p.key) && !(mods.shift || mods.ctrl) {
                        // `N3Msg_PerformSpecialAction(identity)`: not listed = `Feedback_ActionIsNotAvailable`, else the action the entry shows
                        self.uses.push(list.find(p.key as u32).map_or(SlotUse::Unavailable, |e| SlotUse::SpecialAction(e.shown)));
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

    /// Screen position of grid cell `i`'s centre (tests).
    #[cfg(test)]
    pub(super) fn cell_centre(&self, gui: &Gui, i: usize) -> Option<(f32, f32)> {
        let r = gui.view_rect(self.win.as_ref()?.win, "grid")?;
        let p = lv::ICON_32 + lv::SPACING;
        Some((r.l + lv::BORDER + (i % COLS) as f32 * p + 16.0, r.t + lv::BORDER + (i / COLS) as f32 * p + 16.0))
    }
}
