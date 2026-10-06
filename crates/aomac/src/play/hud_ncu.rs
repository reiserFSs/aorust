//! The NCU window `ncu_window` (`NCUView_c`, GUI.dll ctor `FUN_100d6dd6` 0x100d6dd6; module slot `InventoryGUIModule_c::SlotNCUViewActivated` 0x100c6f7d;
//! Ctrl+0). docs/gui.md §11.12.
//!
//! `DockableView_c(rect, "NCU", "ncu_window", flags 5)` around an `NCUListView_c` (a `MultiListView_c`: columns Icon 16 / Name 200 / Type 50 / NanoCost 60 / NCUcost 60 /
//! Time 55 / Remaining 55 / Radius 60 / AtkRange 60 / AtkDelay 55 / RcgDelay 55, sort column 12 = Remaining, grid layout from the template's `listview_mode=false`).
//! Its rows are the own timed nano effects (`N3Msg_GetNanoTemplateInfoList`); the title is text.mdb 10000 `NCU_usage_x/y` formatted with the stats `CurrentNCU` (0xb4) and
//! `MaxNCU` (0xb5) (`FUN_100d6d14`, run on both stat signals). Template placement: the `DockTabbedWindow` `DockArea2` (`prefs/NewChar/DockAreas/DockArea2.xml`,
//! `WindowFrame` Rect(1838, 1078, 2030, 1171), authored on a larger screen: `Window::MoveInsideScreen` pulls it to the bottom right).
//!
//! Interactions: double click = `N3Msg_RemoveBuff` (`FUN_10044c31`): a friendly effect (`Flags & 0xc100 == 0`) is cancelled and "Deactivating friendly timed nanoprogram."
//! goes to the System window, otherwise "Can't remove hostile timed nanoprogram."; the tooltip (`FUN_1004518b`) is the name, "NCU usage: %d", "Remaining: hh:mm:ss" and the
//! modifier list. UNRESOLVED / not ported: the "Modifies:" text (`N3Msg_GetModifierString`), the right-click menu (only "LookAtNano" -> InfoView), the Type column (8) text and
//! how the original learns that an effect ended (the server's action 0x1b / 0x66 paths, `FUN_1004f504`): an effect ends here when its `TimeExist` has run out.

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
/// `Flags` bits of a hostile program (`N3Msg_RemoveBuff` [GC 0x1001be60]: `GetSkill(nano, 0, 2) & 0xc100 == 0` is removable).
const HOSTILE_FLAGS: i32 = 0xc100;

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

    /// System-window lines the double click produced (`GlobalSignals+0x17c`).
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
        let view = ListView::new(Mode::Grid, all.clone(), cols, 3, (12, false));
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

    /// Remaining time of an effect in 1/100 s (`FUN_1004eb5a`: start + total - now).
    fn remaining(total_cs: i32, started: f32, now: f32) -> i32 {
        total_cs - ((now - started) * 100.0) as i32
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, dt: f32) {
        self.clock += dt;
        let Some(win) = self.win.as_ref() else { return };
        let w = win.win;
        let title = Self::title_for(&self.texts, zone.stat(STAT_NCU_USED).unwrap_or(0), zone.stat(STAT_NCU_MAX).unwrap_or(0));
        if title != self.title {
            win.set_title(gui, &title);
            self.title = title;
        }
        // effects whose time ran out end (UNRESOLVED how the original learns it, see the module doc)
        let now = zone.nanos.time;
        let expired: Vec<i32> = zone
            .nanos
            .buffs
            .iter()
            .filter(|b| self.db.info(gui, b.nano).is_none_or(|i| i.total_time() > 0 && Self::remaining(i.total_time(), b.started, now) <= 0))
            .map(|b| b.nano)
            .collect();
        zone.nanos.remove_buffs(&expired);
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
            let total = info.total_time();
            let rem = Self::remaining(total, b.started, now);
            let mut cells = HudNano::cells(&info, &columns);
            for (c, cell) in columns.iter().zip(cells.iter_mut()) {
                if c.id == 12 {
                    *cell = lv::hms(rem);
                }
            }
            // `FUN_1004518b`
            let body = format!("<br>NCU usage:&nbsp;{}<br>Remaining:&nbsp;{}", info.stat(stat::NCU).unwrap_or(0), lv::hms(rem));
            rows.push(Row { key: b.nano, icon: info.icon, cells, tip_title: info.name.clone(), tip_body: body });
        }
        let win = self.win.as_mut().expect("open");
        win.view.invalidate();
        win.view.set_rows(rows);
        win.view.paint(gui, w, lv::ICON_32);
        self.built = Some(sig);
    }

    /// `FUN_10044c31` -> `N3Msg_RemoveBuff` [GC 0x1001be60].
    fn remove(&mut self, gui: &mut Gui, zone: &Zone, id: i32) {
        let hostile = self.db.info(gui, id).is_some_and(|i| i.stat(stat::FLAGS).unwrap_or(0) & HOSTILE_FLAGS != 0);
        if hostile {
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
            Event::MenuPicked { id } if win.owns_menu(*id) => {
                win.picked(*id);
                self.built = None;
                return true;
            }
            _ => {}
        }
        let now = self.clock;
        match win.view.event(gui, w, ev, now, lv::ICON_32) {
            Some(Hit::Double(id)) => {
                self.remove(gui, zone, id);
                true
            }
            Some(Hit::Click(_)) => {
                self.built = None;
                true
            }
            Some(Hit::Context(..)) => true,
            None => false,
        }
    }
}

#[cfg(test)]
impl HudNcu {
    pub(super) fn shown(&self) -> Vec<i32> {
        self.win.as_ref().map(|w| w.view.rows().iter().map(|r| r.key).collect()).unwrap_or_default()
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

    #[test]
    fn remaining_counts_down_from_the_start() {
        assert_eq!(HudNcu::remaining(45000, 3.0, 3.0), 45000);
        assert_eq!(HudNcu::remaining(45000, 3.0, 13.5), 43950);
        assert!(HudNcu::remaining(100, 0.0, 1.0) <= 0);
    }
}
