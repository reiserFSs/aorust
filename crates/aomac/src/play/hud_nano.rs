//! The Programs window `nano_window` (`NanoView_c`, GUI.dll ctor `FUN_100d496c` 0x100d496c; slot of the module `InventoryGUIModule_c::SlotNanoViewActivated`
//! 0x100c6d2a; window title text.mdb 10000 "Programs", Ctrl+8). docs/gui.md §11.12.
//!
//! Layout (`FUN_100d496c`): a vertical layout with the `SchoolSelector_c` (`FUN_100d65c5`: toggle `Button_c`s Favorites, Combat, Medical, Prot, Psi, Space, flowed
//! over rows by `FUN_100d56d9`) over a `TabView` of seven `MultiListView_c`s (grid icon size 0 = 32 px, columns Icon / Name / NanoCost / NCUcost / Time / Radius /
//! AtkRange / AtkDelay / RcgDelay / StackingLine, sort column 1). The page of a program is its `School` stat (0x195) minus 1 (`FUN_100d3eed`); programs without a
//! school are on page 5 (Favorites). "HideNano" moves a program to page 6, which has no selector button.
//!
//! Interactions: double click = `N3Msg_CastNanoSpell(program, target)` (`FUN_100d3b11`, the target is the selected one), right click = the popup of `FUN_100d3fe4`
//! (entries "LookAtNano" -> InfoView `itemid://`, "HideNano" / "UnHideNano"), drag = hotbar (`FUN_100d3807`). UNRESOLVED / not ported: the InfoView window ("LookAtNano"
//! is omitted), dragging a program onto the shortcut bar, the recharge overlay of an icon (`FUN_1003d3ff`), the saved `item_position_map` (free icon positions) and
//! `selected_school` / `listview_mode` persistence (the prefs store is not wired for window configs).

use super::hud::WindowKind;
use super::hud_listview::{self as lv, Column, Hit, ListView, ListWindow, MenuTexts, Mode, Row, Spec};
use super::hud_nanodb::{stat, NanoDb, NanoInfo};
use super::zone::Zone;
use ao_formats::screens::{TextDb, CAT_GUI};
use ao_gui::{Event, FontId, Gui};
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::{nano, outgoing};
use std::collections::HashMap;
use std::path::Path;

/// Rollup page key (the dvalue of the window).
const KEY: &str = "nano_window";
/// Page of the programs without a school (the "Favorites" selector button, value 5).
pub(super) const FAVORITES: usize = 5;
/// Page "HideNano" moves programs to (no selector button).
pub(super) const HIDDEN: usize = 6;
/// Selector buttons in the order `FUN_100d65c5` creates them: (button value = page, text key).
const SCHOOLS: [(usize, &str); 6] = [(5, "Favorites"), (0, "NanoSchool_Combat"), (1, "NanoSchool_Medical"), (2, "NanoSchool_Prot"), (3, "NanoSchool_Psi"), (4, "NanoSchool_Space")];
/// `selected_school` of the NewChar template (`<Int32 name="selected_school" value="5"/>`).
const DEFAULT_PAGE: usize = 5;
/// Visible grid rows (3 in the retail screenshot).
const ROWS: usize = 3;
const MENU_BASE: u32 = 0x4000;
const MENU_HIDE: u32 = MENU_BASE + 0x40;
const MENU_UNHIDE: u32 = MENU_BASE + 0x41;
/// Padding of a selector button around its label and the gap between two (UNRESOLVED GUESS: the `Button_c` art's own insets; `FUN_100d6478` adds `Point(8, 2)` to the
/// preferred size and `FUN_100d56d9` flows the buttons).
const BUTTON_PAD: f32 = 24.0;
const BUTTON_GAP: f32 = 2.0;
const BUTTON_H: f32 = 22.0;

pub(super) struct HudNano {
    db: NanoDb,
    texts: TextDb,
    screen: (u32, u32),
    win: Option<Win>,
    /// Programs the user moved: id -> page (`HideNano` -> [`HIDDEN`]).
    moved: HashMap<i32, usize>,
    page: usize,
    closed: Vec<WindowKind>,
    outbox: Vec<Frame>,
    /// Names of the programs cast since the last call (`N3Msg_CastNanoSpell`'s chat line, [`super::chat::Chat::cast_nano`]).
    casts: Vec<String>,
    clock: f32,
    /// What the rows were built from: (own list serial, page, moved count, mode).
    built: Option<(u32, usize, usize, Mode, usize)>,
    /// Row of the open context menu.
    ctx: Option<i32>,
}

struct Win {
    w: ListWindow,
}

fn client_width() -> f32 {
    lv::grid_width(4, lv::ICON_32)
}

impl HudNano {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> anyhow::Result<Self> {
        Ok(Self {
            db: NanoDb::new(dir),
            texts: TextDb::load(dir)?,
            screen,
            win: None,
            moved: HashMap::new(),
            page: DEFAULT_PAGE,
            closed: vec![],
            outbox: vec![],
            casts: vec![],
            clock: 0.0,
            built: None,
            ctx: None,
        })
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        kind == WindowKind::Nano
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
        if let Some(w) = &mut self.win {
            w.w.screen = screen;
        }
    }

    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    pub(super) fn take_casts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.casts)
    }

    fn text(&self, key: &str) -> String {
        self.texts.by_key(CAT_GUI, key).unwrap_or_default()
    }

    /// The columns of `NanoView_c` (`AddColumn` ids and widths of `FUN_100d496c` / the template's `col_width`).
    fn columns(&self) -> Vec<Column> {
        [(1, "Tab_Name", 200.0), (9, "Tab_NanoCost", 60.0), (10, "Tab_NCUcost", 60.0), (13, "Tab_Time", 55.0), (11, "Tab_Radius", 60.0), (5, "Tab_AtkRange", 60.0), (6, "Tab_AtkDelay", 55.0), (7, "Tab_RcgDelay", 55.0), (16, "Tab_StackingLine", 200.0)]
            .into_iter()
            .map(|(id, key, width)| Column { id, label: self.text(key), width })
            .collect()
    }

    /// Opens the window (a no-op when it is open). Placed left of the rollup column (UNRESOLVED: the rollup dock is HudItems' work; the NewChar template docks the
    /// page in `RollupArea`).
    pub(super) fn open(&mut self, gui: &mut Gui, rollup: &mut super::hud_rollup::Rollup) {
        if self.win.is_some() {
            return;
        }
        match self.build(gui, rollup) {
            Ok(w) => {
                self.win = Some(w);
                self.built = None;
            }
            Err(e) => eprintln!("hud: nano window: {e:#}"),
        }
    }

    fn build(&mut self, gui: &mut Gui, rollup: &mut super::hud_rollup::Rollup) -> anyhow::Result<Win> {
        let cw = client_width();
        let (selector, selector_h) = self.selector_xml(gui, cw);
        let rows_h = 2.0 * lv::BORDER + ROWS as f32 * lv::ICON_32 + (ROWS as f32 - 1.0) * lv::SPACING;
        let client = (cw as u32, (selector_h + rows_h) as u32);
        let all = self.columns();
        let view = ListView::new(Mode::Grid, all.clone(), lv::grid_cols_for(cw, lv::ICON_32), ROWS, (1, false));
        let texts = MenuTexts { list_mode: self.text("ListMode"), auto_arrange: self.text("AutoArrange") };
        let title = self.text("Programs");
        let w = ListWindow::open(gui, Spec { name: "NanoView", title: &title, pos: (0, 0), client, top_xml: &selector, top_h: selector_h, texts, menu_base: MENU_BASE, screen: self.screen }, view, all, Some((rollup, KEY)))?;
        Ok(Win { w })
    }

    /// `FUN_100d56d9`: buttons keep their natural width and flow over rows of the client width.
    fn selector_xml(&self, gui: &mut Gui, cw: f32) -> (String, f32) {
        let mut rows: Vec<Vec<(usize, String, f32)>> = vec![vec![]];
        let mut used = 0.0;
        for (page, key) in SCHOOLS {
            let label = self.text(key);
            let wd = gui.text_width(FontId::Normal, &label) as f32 + BUTTON_PAD;
            if !rows.last().unwrap().is_empty() && used + BUTTON_GAP + wd > cw {
                rows.push(vec![]);
                used = 0.0;
            }
            used += wd + if rows.last().unwrap().is_empty() { 0.0 } else { BUTTON_GAP };
            rows.last_mut().unwrap().push((page, label, wd));
        }
        let mut xml = String::new();
        for r in &rows {
            // retail: a row with several pills spreads them to both edges, a lone pill is centred (`ref.png` Programs page)
            xml += &format!("<View view_layout=\"horizontal\" min_size=\"Point(1,{BUTTON_H})\" layout_borders=\"Rect(8,0,8,0)\">");
            if r.len() == 1 {
                xml += "<HLayoutSpacer min_size=\"0\" max_size=\"16000\"/>";
            }
            for (i, (page, label, wd)) in r.iter().enumerate() {
                if i > 0 {
                    xml += "<HLayoutSpacer min_size=\"0\" max_size=\"16000\"/>";
                }
                xml += &format!("<Button name=\"school{page}\" label=\"{}\" min_size=\"Point({wd},{BUTTON_H})\" layout_borders=\"Rect(1,0,1,0)\"/>", lv::esc(label));
            }
            if r.len() == 1 {
                xml += "<HLayoutSpacer min_size=\"0\" max_size=\"16000\"/>";
            }
            xml += "</View>";
        }
        (xml, rows.len() as f32 * BUTTON_H)
    }

    pub(super) fn close(&mut self, gui: &mut Gui, rollup: &mut super::hud_rollup::Rollup) {
        if self.win.take().is_some() {
            rollup.close_page(gui, KEY);
        }
        self.built = None;
    }

    /// Row cells of a program (`FUN_1003d50c`: columns are `N3Msg_GetSkill(nano, stat, 2)`; UNRESOLVED: column 11 "Radius" is `N3Msg_GetFormularRadius`, a
    /// parse of the nano's formula, shown as 0).
    pub(super) fn cells(info: &NanoInfo, columns: &[Column]) -> Vec<String> {
        columns
            .iter()
            .map(|c| match c.id {
                1 => info.name.clone(),
                5 => num(info.stat(stat::ATTACK_RANGE)),
                6 => num(info.stat(stat::ATTACK_DELAY)),
                7 => num(info.stat(stat::RECHARGE_DELAY)),
                9 => num(info.stat(stat::NANO_POINTS)),
                10 => num(info.stat(stat::NCU)),
                11 => "0".into(),
                13 => lv::hms(info.total_time()),
                16 => (info.stat(stat::STACKING_LINE).unwrap_or(0) & 0xffff).to_string(),
                _ => String::new(),
            })
            .collect()
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, dt: f32) {
        self.clock += dt;
        let Some(win) = self.win.as_mut() else { return };
        let sig = (zone.nanos.serial, self.page, self.moved.len(), win.w.view.mode, win.w.view.columns.len() * 31 + win.w.view.sort.0 as usize + usize::from(win.w.view.sort.1));
        for (page, _) in SCHOOLS {
            gui.set_button_pressed(win.w.win, &format!("school{page}"), page == self.page);
        }
        if self.built == Some(sig) {
            return;
        }
        let columns = win.w.view.columns.clone();
        let mut rows = vec![];
        for &id in &zone.nanos.programs {
            let Some(info) = self.db.info(gui, id).cloned() else { continue };
            if self.moved.get(&id).copied().unwrap_or_else(|| info.school().unwrap_or(FAVORITES)) != self.page {
                continue;
            }
            rows.push(Row { key: id, icon: info.icon, cells: Self::cells(&info, &columns), tip_title: info.name.clone(), tip_body: String::new() });
        }
        let win = self.win.as_mut().expect("open");
        win.w.view.set_rows(rows);
        win.w.view.paint(gui, win.w.win, lv::ICON_32);
        self.built = Some(sig);
    }

    /// `FUN_100d3b11`: cast the program on the selected target (self when there is none and the program needs no target: `FUN_1004f6a0`'s
    /// `target == {0,0} && flags & 0x8000 == 0` rewrite).
    fn cast(&mut self, gui: &mut Gui, zone: &Zone, id: i32) {
        let own = Identity { kind: outgoing::DYNEL_CHAR, instance: zone.char_id as i32 };
        let info = self.db.info(gui, id);
        let needs_target = info.is_some_and(|i| i.stat(stat::FLAGS).unwrap_or(0) & 0x8000 != 0);
        let name = info.map(|i| i.name.clone());
        let target = match zone.target {
            Some(t) => Identity { kind: outgoing::DYNEL_CHAR, instance: t },
            None if !needs_target => own,
            // `FUN_1004f6a0` then refuses ("Feedback_UnableToExecuteOnThisTarget"): nothing is sent
            None => return,
        };
        let payload = nano::cast_nano(zone.char_id as i32, id, target);
        self.outbox.push(outgoing::n3_frame(0, zone.char_id, payload));
        self.casts.extend(name);
    }

    /// `true` when the event belonged to this window.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let Some(win) = self.win.as_mut() else { return false };
        let w = win.w.win;
        match ev {
            Event::FrameIcon { window, x, y } if *window == w => {
                win.w.open_menu(gui, (*x, *y));
                return true;
            }
            Event::MenuPicked { id } if win.w.owns_menu(*id) => {
                match *id {
                    MENU_HIDE | MENU_UNHIDE => {
                        if let Some(row) = self.ctx.take() {
                            // `FUN_100d39c4` moves the item to page 6, `FUN_100d3a42` back to the page of its School stat
                            if *id == MENU_HIDE {
                                self.moved.insert(row, HIDDEN);
                            } else {
                                self.moved.remove(&row);
                            }
                        }
                    }
                    id => {
                        win.w.picked(id);
                    }
                }
                self.built = None;
                return true;
            }
            Event::Clicked { window, view, .. } if *window == w && view.starts_with("school") => {
                if let Ok(p) = view["school".len()..].parse::<usize>() {
                    self.page = p;
                    win.w.view.selected = None;
                    self.built = None;
                }
                return true;
            }
            _ => {}
        }
        let now = self.clock;
        match win.w.view.event(gui, w, ev, now, lv::ICON_32) {
            Some(Hit::Double(id)) => {
                self.cast(gui, zone, id);
                true
            }
            Some(Hit::Click(_)) => {
                self.built = None;
                true
            }
            Some(Hit::Context(id, at)) => {
                self.ctx = Some(id);
                let hidden = self.moved.get(&id) == Some(&HIDDEN);
                let (key, mid) = if hidden { ("UnHideNano", MENU_UNHIDE) } else { ("HideNano", MENU_HIDE) };
                let label = self.text(key);
                gui.open_menu((at.0, at.1), (self.screen.0 as i32, self.screen.1 as i32), vec![ao_gui::MenuItem::entry(mid, &label)]);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
impl HudNano {
    pub(super) fn set_page(&mut self, page: usize) {
        self.page = page;
        self.built = None;
    }

    pub(super) fn shown(&self) -> Vec<i32> {
        self.win.as_ref().map(|w| w.w.view.rows().iter().map(|r| r.key).collect()).unwrap_or_default()
    }
}

fn num(v: Option<i32>) -> String {
    v.unwrap_or(0).to_string()
}

#[cfg(test)]
mod tests {
    use super::super::hud_ncu::HudNcu;
    use super::super::hud_rollup::Rollup;
    use super::*;
    use ao_gui::{DrawList, InputEvent};
    use ao_render::{Frontend, Host, Offscreen};

    struct Shot {
        gui: Gui,
        nano: HudNano,
        ncu: HudNcu,
        zone: Zone,
    }

    impl Frontend for Shot {
        fn gui(&self) -> &Gui {
            &self.gui
        }
        fn input(&mut self, ev: InputEvent, _host: &mut Host) {
            for e in self.gui.input(ev) {
                self.nano.event(&mut self.gui, &e, &self.zone);
                self.ncu.event(&mut self.gui, &e, &self.zone);
            }
        }
        fn frame(&mut self, dt: f32, _size: (u32, u32), _host: &mut Host) -> DrawList {
            self.nano.update(&mut self.gui, &self.zone, dt);
            self.ncu.update(&mut self.gui, &mut self.zone, dt);
            self.gui.frame(dt)
        }
    }

    fn png(s: &mut Shot, o: &mut Offscreen, name: &str) {
        let mut list = DrawList::default();
        for _ in 0..3 {
            list = o.frame(s, 0.016);
        }
        if let Some(dir) = std::env::var_os("AOMAC_SHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            o.png(s, &list, &std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    }

    /// The Programs and NCU windows with real nano records: pages by school, effects with their remaining time.
    #[test]
    fn programs_and_ncu_windows_show_real_records() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let size = (1100, 760);
        let labels = TextDb::load(&dir).unwrap();
        let mut gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let mut rollup = Rollup::new(&dir, size);
        let mut nano = HudNano::new(&dir, size).unwrap();
        let mut ncu = HudNcu::new(&dir, size).unwrap();
        let mut zone = Zone::new(7);
        // 163449 Shadow Touch (school 5 = Space), 25982 (timed 45000 = 7:30)
        zone.nanos.programs = vec![163449, 25982];
        zone.nanos.buffs = vec![super::super::own_nanos::Buff { nano: 25982, started: 0.0 }];
        zone.nanos.serial += 1;
        zone.stats.insert(0xb4, 14);
        zone.stats.insert(0xb5, 40);
        nano.open(&mut gui, &mut rollup);
        ncu.open(&mut gui);
        let mut s = Shot { gui, nano, ncu, zone };
        let mut o = Offscreen::new(&s, size).unwrap();
        // Space page = 4
        s.nano.set_page(4);
        png(&mut s, &mut o, "nano-space");
        assert_eq!(s.nano.shown(), vec![163449]);
        png(&mut s, &mut o, "ncu");
        assert_eq!(s.ncu.shown(), vec![25982]);
        assert_eq!(s.ncu.title(), "NCU (14/40)");
        // double click on the nano casts it on self (Shadow Touch needs no target)
        let frames = s.nano.take_outbox();
        assert!(frames.is_empty());
    }
}
