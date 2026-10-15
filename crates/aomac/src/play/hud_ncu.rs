//! The NCU window `ncu_window` (`NCUView_c`, GUI.dll ctor `FUN_100d6dd6` 0x100d6dd6; module slot `InventoryGUIModule_c::SlotNCUViewActivated` 0x100c6f7d;
//! Ctrl+0). docs/gui.md §11.12.
//!
//! `DockableView_c(rect, "NCU", "ncu_window", flags 5)` around an `NCUListView_c` (a `MultiListView_c`: columns Icon 16 / Name 200 / Type 50 / NanoCost 60 / NCUcost 60 /
//! Time 55 / Remaining 55 / Radius 60 / AtkRange 60 / AtkDelay 55 / RcgDelay 55, sort column 12 = Remaining, grid layout from the template's `listview_mode=false`).
//! Its rows are the own timed nano effects (`N3Msg_GetNanoTemplateInfoList`); the title is text.mdb 10000 `NCU_usage_x/y` formatted with the stats `CurrentNCU` (0xb4) and
//! `MaxNCU` (0xb5) (`FUN_100d6d14`, run on both stat signals). Template placement: the `DockTabbedWindow` `DockArea2` (`prefs/NewChar/DockAreas/DockArea2.xml`,
//! `WindowFrame` Rect(1838, 1078, 2030, 1171), authored on a larger screen: `Window::MoveInsideScreen` pulls it to the bottom right).
//!
//! Interactions: right click or double click = `N3Msg_RemoveBuff` (`FUN_10044c31`): a friendly effect (`Flags & 0xc100 == 0`) requests cancellation and
//! "Deactivating friendly timed nanoprogram." goes to the System window, otherwise "Can't remove hostile timed nanoprogram."; the tooltip (`FUN_1004518b`) is
//! the type, "NCU usage: %d", "Remaining: hh:mm:ss" and the event-0 modifier list (`N3Msg_GetModifierString`).
//! Effect timing and removal belong to `OwnNanos`; opening this window never expires or removes effects.

use super::hud::WindowKind;
use super::hud_listview::{self as lv, Column, Hit, ListView, ListWindow, MenuTexts, Mode, Row, Spec};
use super::hud_nano::HudNano;
use super::hud_nanodb::{stat, NanoDb};
use super::zone::Zone;
use ao_formats::screens::{TextDb, CAT_GUI};
use ao_gui::{xml, Event, Gui};
use ao_net::frame::Frame;
use ao_net::n3::{nano, outgoing};
use std::path::Path;

const MENU_BASE: u32 = 0x5000;
const STAT_NCU_USED: u32 = 0xb4;
const STAT_NCU_MAX: u32 = 0xb5;

pub(super) struct HudNcu {
    db: NanoDb,
    texts: TextDb,
    screen: (u32, u32),
    frame: (i32, i32, u32, u32),
    win: Option<ListWindow>,
    closed: Vec<WindowKind>,
    outbox: Vec<Frame>,
    lines: Vec<String>,
    clock: f32,
    built: Option<(u32, i64, Mode, usize)>,
    title: String,
}

/// `Rect name="WindowFrame"` of a `prefs/NewChar/DockAreas/<name>.xml` as `(x, y, outer w, outer h)` (the rect is inclusive: `Rect(l, t, r, b)`).
pub(super) fn dock_frame(dir: &Path, name: &str) -> Option<(i32, i32, u32, u32)> {
    let root = xml::parse(&std::fs::read_to_string(dir.join(format!("prefs/NewChar/DockAreas/{name}.xml"))).ok()?).ok()?;
    fn find(e: &xml::Element) -> Option<&str> {
        if e.name == "Rect" && e.attr("name") == Some("WindowFrame") {
            return e.attr("value");
        }
        e.children.iter().find_map(find)
    }
    let v = find(&root)?;
    let n: Vec<f32> = v.trim_start_matches("Rect(").trim_end_matches(')').split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (n.len() == 4).then(|| (n[0] as i32, n[1] as i32, (n[2] - n[0]) as u32, (n[3] - n[1]) as u32))
}

impl HudNcu {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> anyhow::Result<Self> {
        Ok(Self {
            db: NanoDb::new(dir),
            texts: TextDb::load(dir)?,
            screen,
            // the install's frame; without it the 4 x 3 grid of the retail screenshot
            frame: dock_frame(dir, "DockArea2").unwrap_or((0, 0, 192, 93)),
            win: None,
            closed: vec![],
            outbox: vec![],
            lines: vec![],
            clock: 0.0,
            built: None,
            title: String::new(),
        })
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        kind == WindowKind::Ncu
    }

    /// Whether `(x, y)` sits over an NCU buff icon in the grid (`MP_Spell_Icon`, shift-to-inspect cursor).
    pub(super) fn icon_at(&self, gui: &Gui, x: f32, y: f32) -> bool {
        self.win.as_ref().is_some_and(|w| w.view.row_at(gui, w.win, x, y, lv::ICON_32).is_some())
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
        if let Some(w) = &mut self.win {
            w.screen = screen;
        }
    }

    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// System-window lines the cancellation request produced (`GlobalSignals+0x17c`).
    pub(super) fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }

    fn text(&self, key: &str) -> String {
        self.texts.by_key(CAT_GUI, key).unwrap_or_default()
    }

    /// `AddColumn` calls of `FUN_100d6dd6` (all visible: the template's `col_id` list is 0, 1, 8, 9, 10, 13, 12, 11, 5, 6, 7).
    fn columns(&self) -> Vec<Column> {
        [(1, "Tab_Name", 200.0), (8, "Tab_Type", 50.0), (9, "Tab_NanoCost", 60.0), (10, "Tab_NCUcost", 60.0), (13, "Tab_Time", 55.0), (12, "Tab_Remaning", 55.0), (11, "Tab_Radius", 60.0), (5, "Tab_AtkRange", 60.0), (6, "Tab_AtkDelay", 55.0), (7, "Tab_RcgDelay", 55.0)]
            .into_iter()
            .map(|(id, key, width)| Column { id, label: self.text(key), width })
            .collect()
    }

    pub(super) fn open(&mut self, gui: &mut Gui) {
        if self.win.is_some() {
            return;
        }
        let (x, y, ow, oh) = self.frame;
        // the frame's outer size minus the style-0 insets (5, 26, 5, 5)
        let client = (ow.saturating_sub(10).max(60), oh.saturating_sub(31).max(40));
        let all = self.columns();
        let cols = lv::grid_cols_for(client.0 as f32, lv::ICON_32);
        let mut view = ListView::new(Mode::Grid, all.clone(), cols, 3, (12, false));
        // `MultiListView_c::RecalcCellCount` 0x101349f6: fill the initial window width, not a blank strip from a plain 10 px
        // gap (`hud_listview.rs::fill_spacing`); `Event::WindowFrame` keeps it filled as the window resizes.
        view.spacing = lv::fill_spacing(client.0 as f32, cols, lv::ICON_32);
        let texts = MenuTexts { list_mode: self.text("ListMode"), auto_arrange: self.text("AutoArrange") };
        match ListWindow::open(gui, Spec { name: "NCUView", title: "NCU", pos: (x, y), client, top_xml: "", top_h: 0.0, texts, menu_base: MENU_BASE, screen: self.screen }, view, all, None) {
            Ok(w) => {
                self.win = Some(w);
                self.built = None;
                self.title.clear();
            }
            Err(e) => eprintln!("hud: ncu window: {e:#}"),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui) {
        if let Some(w) = self.win.take() {
            w.close(gui);
        }
        self.built = None;
    }

    /// `FUN_100d6d14`: "NCU (used/max)" from the text `NCU_usage_x/y` ("NCU (%u/%u)").
    pub(super) fn title_for(texts: &TextDb, used: i32, max: i32) -> String {
        let fmt = texts.by_key(CAT_GUI, "NCU_usage_x/y").unwrap_or_default();
        fmt.replacen("%u", &used.to_string(), 1).replacen("%u", &max.to_string(), 1)
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, dt: f32) {
        self.clock += dt;
        let Some(win) = self.win.as_ref() else { return };
        let w = win.win;
        let title = Self::title_for(&self.texts, zone.skill_value(STAT_NCU_USED).unwrap_or(0), zone.skill_value(STAT_NCU_MAX).unwrap_or(0));
        if title != self.title {
            win.set_title(gui, &title);
            self.title = title;
        }
        let now = zone.nanos.time;
        let second = now as i64;
        let win = self.win.as_ref().expect("open");
        let sig = (zone.nanos.serial, second, win.view.mode, win.view.columns.len() * 31 + win.view.sort.0 as usize + usize::from(win.view.sort.1));
        if self.built == Some(sig) {
            return;
        }
        let columns = win.view.columns.clone();
        let mut rows = vec![];
        for b in &zone.nanos.buffs {
            let Some(info) = self.db.info(gui, b.nano).cloned() else { continue };
            let rem = b.remaining_cs(now);
            let mut cells = HudNano::cells(&info, &columns);
            for (c, cell) in columns.iter().zip(cells.iter_mut()) {
                match c.id {
                    12 => *cell = lv::hms(rem),
                    13 => *cell = lv::hms(b.total_cs),
                    _ => {}
                }
            }
            // `FUN_1004518b`
            let mut body = format!("{}<br>NCU usage:&nbsp;{}<br>Remaining:&nbsp;{}", info.type_text(), info.stat(stat::NCU).unwrap_or(0), lv::hms(rem));
            match self.db.modifiers(&info, zone, &self.texts) {
                Ok(modifiers) if !modifiers.is_empty() => {
                    body.push_str("<br>Modifies:<br>");
                    body.push_str(&modifiers.replace('\n', "<br>"));
                }
                Ok(_) => {}
                Err(e) => eprintln!("hud: ncu modifiers {}: {e:#}", b.nano),
            }
            rows.push(Row { key: b.nano, icon: info.icon, cells, tip_title: info.name.clone(), tip_body: body });
        }
        let win = self.win.as_mut().expect("open");
        win.view.set_rows(rows);
        win.view.paint(gui, w, lv::ICON_32);
        self.built = Some(sig);
    }

    /// `FUN_10044c31` -> `N3Msg_RemoveBuff` [GC 0x1001be60].
    pub(super) fn remove(&mut self, gui: &mut Gui, zone: &Zone, id: i32) {
        let flags = self.db.info(gui, id).and_then(|i| i.stat(stat::FLAGS)).unwrap_or(0);
        if !nano::can_remove_buff(flags) {
            self.lines.push("Can't remove hostile timed nanoprogram.".into());
        } else {
            let payload = nano::remove_buff(zone.char_id as i32, id);
            self.outbox.push(outgoing::n3_frame(0, zone.char_id, payload));
            self.lines.push("Deactivating friendly timed nanoprogram.".into());
        }
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let Some(win) = self.win.as_mut() else { return false };
        let w = win.win;
        match ev {
            Event::CloseRequested { window } if *window == w => {
                self.close(gui);
                self.closed.push(WindowKind::Ncu);
                return true;
            }
            Event::FrameIcon { window, x, y } if *window == w => {
                win.open_menu(gui, (*x, *y));
                return true;
            }
            Event::WindowFrame { window } if *window == w => {
                if win.reflow_grid(gui, lv::ICON_32) {
                    self.built = None;
                }
                return true;
            }
            Event::MenuPicked { id } if win.owns_menu(*id) => {
                win.picked(*id);
                self.built = None;
                return true;
            }
            _ => {}
        }
        let now = self.clock;
        match win.view.event(gui, w, ev, now, lv::ICON_32) {
            Some(Hit::Double(id) | Hit::Context(id, _)) => {
                self.remove(gui, zone, id);
                true
            }
            Some(Hit::Click(_)) => {
                self.built = None;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
impl HudNcu {
    pub(super) fn shown(&self) -> Vec<i32> {
        self.win.as_ref().map(|w| w.view.rows().iter().map(|r| r.key).collect()).unwrap_or_default()
    }

    /// Live harness: screen position of the first row's icon (the NCU window has no named per-row views).
    pub(super) fn first_point(&self, gui: &Gui) -> Option<(f32, f32)> {
        let win = self.win.as_ref()?;
        let r = gui.view_rect(win.win, "grid")?;
        Some((r.l + lv::BORDER + 16.0, r.t + lv::BORDER + 16.0))
    }

    pub(super) fn title(&self) -> &str {
        &self.title
    }

    pub(super) fn window(&self) -> Option<ao_gui::WindowId> {
        self.win.as_ref().map(|w| w.win)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::own_nanos::Buff;

    #[test]
    fn real_friendly_and_hostile_rows_share_type_radius_and_modifiers() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut ncu = HudNcu::new(&dir, (1100, 760)).unwrap();
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let friendly = store.ids(super::super::hud_nanodb::NANO_RDB_TYPE).unwrap().into_iter().find_map(|id| {
            let id = i32::try_from(id).ok()?;
            let info = ncu.db.info(&mut gui, id)?;
            (info.stat(stat::FLAGS).unwrap_or(1) & 0x8000 == 0
                && info.spells.iter().any(|s| s.function == 0xcf14 && s.stat(39) != 0)).then_some(id)
        }).expect("installed friendly modifier nano");
        // GUI1003eacb tests only Flags&0x8000 for Type. The cancellation
        // mask0xc100 is different: timed25982 is not removable but is Friendly.
        let timed = ncu.db.info(&mut gui, 25982).unwrap();
        assert_eq!(timed.stat(stat::FLAGS).unwrap_or(1) & 0x8000, 0);
        assert!(!nano::can_remove_buff(timed.stat(stat::FLAGS).unwrap_or(0)));
        assert_eq!(timed.type_text(), "<font color=\"white\">Friendly</font>");
        let hostile = 28612; // Ice Flechette, an actual Flags&0x8000 nano.
        let mut zone = Zone::new(7);
        zone.nanos.buffs = [friendly, hostile].into_iter().map(|nano| Buff { nano, total_cs: 90000, ..Default::default() }).collect();
        ncu.open(&mut gui);
        ncu.update(&mut gui, &zone, 0.0);
        for id in [friendly, hostile] {
            let info = ncu.db.info(&mut gui, id).unwrap().clone();
            let columns = ncu.columns();
            let cells = HudNano::cells(&info, &columns);
            let row = ncu.win.as_ref().unwrap().view.rows().iter().find(|r| r.key == id).unwrap();
            for (column, cell) in columns.iter().zip(&cells) {
                if column.id == 8 { assert_eq!(cell, info.type_text()); }
                if column.id == 11 { assert_eq!(cell, "0", "static NanoItem+b8"); }
            }
            assert_eq!(row.cells, cells.iter().enumerate().map(|(i, cell)| match columns[i].id {
                12 | 13 => lv::hms(90000), _ => cell.clone(),
            }).collect::<Vec<_>>());
            assert!(row.tip_body.starts_with(info.type_text()));
            let modifiers = ncu.db.modifiers(&info, &zone, &ncu.texts).unwrap();
            if id == friendly { assert!(!modifiers.is_empty()); }
            assert_eq!(row.tip_body.contains("<br>Modifies:<br>"), !modifiers.is_empty());
            if !modifiers.is_empty() { assert!(row.tip_body.ends_with(&modifiers.replace('\n', "<br>"))); }
            if id == hostile {
                assert_ne!(info.stat(stat::FLAGS).unwrap_or(0) & 0x8000, 0, "GUI1003eacb hostile Type predicate");
                assert_eq!(info.type_text(), "<font color=\"red\">Hostile</font>");
            }
        }
    }

    #[test]
    fn right_click_requests_cancellation_and_waits_for_server_removal() {
        use ao_net::msg::Identity;
        use ao_net::n3::{misc::{self, Misc}, N3};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut ncu = HudNcu::new(&dir, (1100, 760)).unwrap();
        // The existing duration fixture 25982 is not necessarily removable.
        // Select a real friendly timed template rather than treating timing as friendliness.
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let id = store.ids(super::super::hud_nanodb::NANO_RDB_TYPE).unwrap().into_iter().find_map(|id| {
            let id = i32::try_from(id).ok()?;
            let info = ncu.db.info(&mut gui, id)?;
            (info.total_time() > 0 && nano::can_remove_buff(info.stat(stat::FLAGS).unwrap_or(0))).then_some(id)
        }).expect("installed friendly timed nanoprogram");
        let mut zone = Zone::new(7);
        zone.nanos.buffs.push(Buff { nano: id, total_cs: 90000, ..Default::default() });
        ncu.open(&mut gui);
        ncu.update(&mut gui, &zone, 0.0);
        let w = ncu.window().unwrap();
        let r = gui.view_rect(w, "grid").unwrap();
        assert!(ncu.event(&mut gui, &Event::ContextMenu { window: w, x: r.l as i32 + lv::BORDER as i32 + 2, y: r.t as i32 + lv::BORDER as i32 + 2, link: None }, &zone));
        assert_eq!(ncu.take_outbox(), [outgoing::n3_frame(0, 7, nano::remove_buff(7, id))]);
        assert_eq!(ncu.take_lines(), ["Deactivating friendly timed nanoprogram."]);
        ncu.update(&mut gui, &zone, 0.0);
        assert_eq!(ncu.shown(), [id], "request must not optimistically remove the icon");
        let own = Identity { kind: 0xc350, instance: 7 };
        let removal = N3::Misc(Misc::Buff(misc::Buff { kind: 0, nano: Some(nano::nano(id)), rest: vec![] }));
        zone.nanos.on_message(own, own, &removal, 100);
        ncu.update(&mut gui, &zone, 0.0);
        assert!(ncu.shown().is_empty(), "server removal refreshes the NCU list");
        zone.nanos.buffs.push(Buff { nano: 25982, total_cs: 90000, ..Default::default() });
        zone.nanos.serial += 1;
        ncu.update(&mut gui, &zone, 0.0);
        let r = gui.view_rect(w, "grid").unwrap();
        assert!(ncu.event(&mut gui, &Event::ContextMenu { window: w, x: r.l as i32 + lv::BORDER as i32 + 2, y: r.t as i32 + lv::BORDER as i32 + 2, link: None }, &zone));
        assert!(ncu.take_outbox().is_empty(), "hostile timed template cannot send cancellation");
        assert_eq!(ncu.take_lines(), ["Can't remove hostile timed nanoprogram."]);
        assert_eq!(ncu.shown(), [25982]);
    }

    /// Dragging the NCU window's bottom-right corner wider reflows the icon grid to more columns (`MultiListView_c::RecalcCellCount`
    /// 0x101349f6, `hud_listview.rs::reflow_grid`), instead of leaving empty space to the right of a fixed 4-column grid.
    #[test]
    fn widening_the_window_adds_grid_columns() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        gui.set_screen_size(2000, 1200);
        let mut ncu = HudNcu::new(&dir, (1100, 760)).unwrap();
        ncu.open(&mut gui);
        let w = ncu.window().unwrap();
        // `Hud::open`'s `wincfg.attach` makes every `Store::Dock` window (NCU included) movable/resizable; exercised directly
        // here since this focused rig does not go through the full `Hud`.
        gui.set_window_frame(w, true, true);
        gui.set_window_size_limits(w, (60, 40), (2000, 1200));
        let before = ncu.win.as_ref().unwrap().view.cols;
        let (ox, oy, ow, oh) = gui.window_outer_frame(w).unwrap();
        gui.set_window_outer_frame(w, (ox, oy, ow + 200, oh));
        ncu.event(&mut gui, &Event::WindowFrame { window: w }, &Zone::new(7));
        let after = ncu.win.as_ref().unwrap().view.cols;
        assert!(after > before, "widening the window must add grid columns: {before} -> {after}");
    }

    #[test]
    fn window_reads_runtime_duration_without_owning_expiry() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut ncu = HudNcu::new(&dir, (1100, 760)).unwrap();
        let mut zone = Zone::new(7);
        zone.nanos.buffs = vec![
            Buff { nano: 25982, total_cs: 90000, ..Default::default() },
            Buff { nano: -1, total_cs: 100, ..Default::default() },
        ];
        zone.nanos.time = 500.0;
        ncu.open(&mut gui);
        ncu.update(&mut gui, &zone, 0.0);
        assert_eq!(zone.nanos.buffs.len(), 2, "missing records and elapsed template durations cannot remove lifecycle entries");
        assert_eq!(ncu.shown(), [25982]);
        let view = &ncu.win.as_ref().unwrap().view;
        let row = &view.rows()[0];
        assert!(row.tip_body.contains(&format!("Remaining:&nbsp;{}", lv::hms(40000))));
        for (column, cell) in view.columns.iter().zip(&row.cells) {
            match column.id {
                12 => assert_eq!(cell, &lv::hms(40000)),
                13 => assert_eq!(cell, &lv::hms(90000)),
                _ => {}
            }
        }
    }
}
