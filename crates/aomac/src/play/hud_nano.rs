//! The Programs window `nano_window` (`NanoView_c`, GUI.dll ctor `FUN_100d496c` 0x100d496c; slot of the module `InventoryGUIModule_c::SlotNanoViewActivated`
//! 0x100c6d2a; window title text.mdb 10000 "Programs", Ctrl+8). docs/gui.md §11.12.
//!
//! Layout (`FUN_100d496c`): a vertical layout with the `SchoolSelector_c` (`FUN_100d65c5`: toggle `Button_c`s Favorites, Combat, Medical, Prot, Psi, Space, flowed
//! over rows by `FUN_100d56d9`) over a `TabView` of seven `MultiListView_c`s (grid icon size 0 = 32 px, columns Icon / Name / NanoCost / NCUcost / Time / Radius /
//! AtkRange / AtkDelay / RcgDelay / StackingLine, sort column 1). The page of a program is its `School` stat (0x195) minus 1 (`FUN_100d3eed`); programs without a
//! school are on page 5 (Favorites). "HideNano" moves a program to page 6, which has no selector button.
//!
//! Interactions: double click = `N3Msg_CastNanoSpell(program, target)` (`FUN_100d3b11`, the target is the selected one), right click = the popup of `FUN_100d3fe4`
//! ("LookAtNano" -> InfoView `itemid://`, "HideNano" / "UnHideNano"), drag = hotbar (`FUN_100d3807`).
//! Shift-click inspects: `FUN_100d496c` constructs `ItemListViewBase_c` via `FUN_100419f8`, which connects `FUN_10040a17` (`GetQualifiers() & 3`).
//! Ctrl alone remains ordinary selection / double-click casting (`FUN_100bc5cf`: Shift = 3, Ctrl = 0xc, Alt = 0x30).
//! UNRESOLVED: recharge overlays and Programs' free icon-position / school / list-mode archives.

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
const MENU_INFO: u32 = MENU_BASE + 0x42;
/// Padding of a selector button around its label and the gap between two (UNRESOLVED GUESS: the `Button_c` art's own insets; `FUN_100d6478` adds `Point(8, 2)` to the
/// preferred size and `FUN_100d56d9` flows the buttons).
const BUTTON_PAD: f32 = 24.0;
const BUTTON_GAP: f32 = 2.0;
const BUTTON_H: f32 = 22.0;

/// Native vehicle state used by GC1004f720..1004f7ce; published by Player,
/// not reconstructed from keyboard input or animation.
#[derive(Clone, Copy, Debug)]
pub struct NanoMotion {
    pub pos: [f32; 3],
    pub radius: f32,
    pub mode: u32,
    pub moving: bool,
    pub falling: bool,
    pub input_locked: bool,
}

fn runtime_admission(info: &NanoInfo, zone: &Zone, target: Identity) -> anyhow::Result<bool> {
    if zone.nano_motion.map_or_else(|| zone.own_update.as_ref().is_some_and(|u| u.flag_32f), |m| m.input_locked) { return Ok(false); }
    if zone.nano_motion.is_some_and(|m| m.moving || m.falling || matches!(m.mode, 4 | 5 | 8 | 9 | 11 | 12)) { return Ok(false); }
    let own = zone.char_id as i32;
    if zone.own_update.is_some() {
        anyhow::ensure!(zone.nano_motion.is_some(), "nano admission needs the initialized own vehicle");
        if !zone.dynels.contains_key(&own) { return Ok(false); } // Removed control dynel is no longer cast-capable.
    }
    let target = info.cast_target(zone.char_id, target);
    if zone.stat_of(target.instance, 662).is_some_and(|v| v != 0) { return Ok(false); }
    let hostile = info.stat(stat::FLAGS).unwrap_or(1) & 0x8000 != 0;
    let pet = zone.pets.contains(&target);
    // +21c is the native NPC discriminator; 100523c3 checks the pet list.
    if !hostile && target.instance != own && zone.dynels.get(&target.instance).is_some_and(|d| d.npc) && !pet {
        return Ok(false);
    }
    if zone.world.nano_school_busy(own, info.stat(stat::SCHOOL).unwrap_or(1)) { return Ok(false); }
    if zone.world.nano_program_locked(own, info.id) { return Ok(false); }
    let families = [75, 546, 547, 548, 549, 550].map(|s| info.stat(s).unwrap_or(0));
    if zone.world.nano_family_locked(own, families) { return Ok(false); }
    let pending_conflict = |who, no_replace| zone.world.nano_pending_self(who).any(|(id, existing)| {
        let existing_families = [75, 546, 547, 548, 549, 550].map(|s| existing.stat(s).unwrap_or(if s == 75 { id } else { 0 }));
        super::own_nanos::same_family(existing_families, families)
            && (no_replace || existing.stat(551).unwrap_or(0) > info.stat(551).unwrap_or(0))
    });
    if pending_conflict(target.instance, false) { return Ok(false); }
    let effects = if target.instance == own { Some(&zone.nanos) } else { zone.world.target_nanos(target.instance) };
    if let Some(effects) = effects {
        if !effects.permits_cast(info, false)? { return Ok(false); }
    }
    // The no-replace check deliberately uses SOURCE effects (1004fab1), not
    // the target list used by the stronger-program check (1004fa91).
    if info.stat(stat::FLAGS).unwrap_or(1) & 0x800000 != 0
        && (!zone.nanos.permits_cast(info, true)? || pending_conflict(own, true)) { return Ok(false); }
    if let Some(motion) = zone.nano_motion {
        let (pos, radius) = if target.instance == own { (motion.pos, motion.radius) } else {
            let source_pf = zone.character_playfields.get(&own)
                .ok_or_else(|| anyhow::anyhow!("nano source playfield identity is unavailable"))?;
            let target_pf = zone.character_playfields.get(&target.instance)
                .ok_or_else(|| anyhow::anyhow!("nano target playfield identity is unavailable"))?;
            if source_pf != target_pf { return Ok(false); }
            zone.world.nano_target_geometry(target.instance)
                .ok_or_else(|| anyhow::anyhow!("nano target {} has no native collision sphere yet", target.instance))?
        };
        let range_bonus = super::hud_nanodb::effective_nano_stat(zone, 381);
        let range = nano_range(info.stat(stat::ATTACK_RANGE).unwrap_or(20), range_bonus);
        if !nano_in_range(motion.pos, motion.radius, pos, radius, range) { return Ok(false); }
        if target.instance != own {
            let visible = zone.world.nano_line_of_sight(motion.pos, pos)
                .ok_or_else(|| anyhow::anyhow!("nano visibility needs loaded playfield collision"))?;
            if !visible { return Ok(false); }
        }
    }
    if target.instance == own { return Ok(true); }
    if hostile {
        return Ok(super::combat::module::can_attack(zone, target, zone.nano_motion.map_or(3, |m| m.mode), false).allowed);
    }
    let target_side = zone.stat_of(target.instance, 33).unwrap_or(ao_formats::stats::INVALID);
    let side = zone.stat(33).unwrap_or(ao_formats::stats::INVALID);
    if !pet && target_side == 3 && side != 3 { return Ok(false); }
    if pet { return Ok(true); }
    let source_level = zone.character_fight_levels.get(&own)
        .ok_or_else(|| anyhow::anyhow!("friendly nano needs the source district query"))?;
    let target_level = zone.character_fight_levels.get(&target.instance)
        .ok_or_else(|| anyhow::anyhow!("friendly nano needs the target district query"))?;
    if *source_level != 2 && *target_level != 2 { return Ok(true); }
    if zone.target_team.target_member(target) { return Ok(true); }
    let playfield = zone.playfield.ok_or_else(|| anyhow::anyhow!("friendly nano needs the source playfield"))?;
    if (0x111c..0x1125).contains(&playfield) {
        // Actual SimpleChar ctor1005cb6a seeds BattlestationSide668=2.
        return Ok(zone.stat(668).unwrap_or(ao_formats::stats::INVALID) == zone.stat_of(target.instance, 668).unwrap_or(ao_formats::stats::INVALID));
    }
    if zone.target_team.in_team() && zone.stat(213).unwrap_or(ao_formats::stats::INVALID) != target_side { return Ok(false); }
    Ok(side == target_side)
}

/// GC10086044: additive effective NanoRange381 percentage, truncate, cap at40m.
fn nano_range(base: i32, bonus: i32) -> i32 {
    ((f64::from(base.wrapping_mul(bonus)) / 100.0 + f64::from(base)) as i64 as i32).min(40)
}

/// Final GC1004e7eb uses the unrounded surface-to-surface distance.
fn nano_in_range(from: [f32; 3], source_radius: f32, to: [f32; 3], target_radius: f32, range: i32) -> bool {
    let distance = (0..3).map(|i| (from[i] - to[i]).powi(2)).sum::<f32>().sqrt() - target_radius - source_radius;
    distance <= range as f32
}

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
    initiations: Vec<(i32, i32)>,
    feedback: Vec<&'static str>,
    clock: f32,
    /// What the rows were built from: (own list serial, page, moved count, mode).
    built: Option<(u32, usize, usize, Mode, usize)>,
    /// Row of the open context menu.
    ctx: Option<i32>,
    press: Option<(i32, f32, f32, f32)>,
    drag: Option<i32>,
    qualified_click: bool,
    urls: Vec<String>,
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
            initiations: vec![],
            feedback: vec![],
            clock: 0.0,
            built: None,
            ctx: None,
            press: None,
            drag: None,
            qualified_click: false,
            urls: vec![],
        })
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        kind == WindowKind::Nano
    }

    /// Whether `(x, y)` sits over a nano/program icon in the Programs grid (`MP_Spell_Icon`, shift-to-inspect cursor).
    pub(super) fn icon_at(&self, gui: &Gui, x: f32, y: f32) -> bool {
        self.win.as_ref().is_some_and(|w| w.w.view.row_at(gui, w.w.win, x, y, lv::ICON_32).is_some())
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

    pub(super) fn take_initiations(&mut self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.initiations.drain(..)
    }

    pub(super) fn take_feedback(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.feedback)
    }

    pub(super) fn take_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.urls)
    }

    pub(super) fn take_drag(&mut self) -> Option<i32> {
        self.drag.take()
    }

    /// Native Shift clicks inspect; ordinary held/moved presses emit the nano drag signal.
    pub(super) fn input(&mut self, gui: &Gui, ev: &ao_gui::InputEvent, mods: ao_gui::Modifiers) {
        use ao_gui::{InputEvent, MouseButton};
        match *ev {
            InputEvent::MouseDown { x, y, .. } if !self.win.as_ref().is_some_and(|w| gui.pointer_owner(x, y) == Some(w.w.win)) => {}
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                self.qualified_click = false;
                self.press = None;
                let Some(win) = self.win.as_ref() else { return };
                let Some(id) = win.w.view.row_at(gui, win.w.win, x, y, lv::ICON_32) else { return };
                self.qualified_click = mods.shift;
                if self.qualified_click {
                    self.urls.push(format!("itemid://{}/{}", nano::NANO_KIND, id));
                } else {
                    self.press = Some((id, x, y, 0.0));
                }
            }
            InputEvent::MouseMove { x, y } => {
                if let Some((id, px, py, _)) = self.press {
                    if (x - px).hypot(y - py) > 3.0 {
                        self.press = None;
                        self.drag = Some(id);
                        self.qualified_click = true;
                    }
                }
            }
            InputEvent::MouseUp { button: MouseButton::Left, .. } => self.press = None,
            _ => {}
        }
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
        let cols = lv::grid_cols_for(cw, lv::ICON_32);
        let mut view = ListView::new(Mode::Grid, all.clone(), cols, ROWS, (1, false));
        // `MultiListView_c::RecalcCellCount` 0x101349f6: fill the fixed RollupArea page width, not a blank strip from a plain
        // 10 px gap (`hud_listview.rs::fill_spacing`, same bug/fix as the Actions page).
        view.spacing = lv::fill_spacing(cw, cols, lv::ICON_32);
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
        self.press = None;
        self.drag = None;
        self.qualified_click = false;
    }

    /// GUI1003d50c. Radius is NanoItem+b8 (GC100164f2), initialized to zero
    /// by GC10085bf8 and unchanged by static template loading GC1008622a.
    pub(super) fn cells(info: &NanoInfo, columns: &[Column]) -> Vec<String> {
        columns
            .iter()
            .map(|c| match c.id {
                1 => info.name.clone(),
                5 => num(info.stat(stat::ATTACK_RANGE)),
                6 => num(info.stat(stat::ATTACK_DELAY)),
                7 => num(info.stat(stat::RECHARGE_DELAY)),
                8 => info.type_text().into(),
                9 => num(info.stat(stat::NANO_POINTS)),
                10 => num(info.stat(stat::NCU)),
                11 => "0".into(),
                13 => lv::hms(info.total_time()),
                16 => (info.stat(stat::STACKING_LINE).unwrap_or(0) & 0xffff).to_string(),
                _ => String::new(),
            })
            .collect()
    }

    pub(super) fn shortcut_info(&mut self, gui: &mut Gui, id: i32) -> Option<super::hud_stats::ItemInfo> {
        self.db.info(gui, id).map(|info| (info.name.clone(), info.icon))
    }

    pub(super) fn update_item_tooltip(&mut self, gui: &mut Gui, zone: &Zone, stats: &super::hud_stats::HudStats, descriptions: bool) {
        if let Some(w) = &mut self.win {
            w.w.view.update_item_tooltip(gui, w.w.win, lv::ICON_32, descriptions, |id| stats.tooltip_body(zone, ao_net::msg::Identity { kind: 0xcf1b, instance: id }));
        }
    }

    /// GlobalSignals +0x110: `FUN_100d4319` / `FUN_100d435b` select the uploaded program's page.
    pub(super) fn select_learned(&mut self, gui: &mut Gui, id: i32) {
        if self.win.is_none() {
            return;
        }
        let Some(info) = self.db.info(gui, id) else { return };
        self.page = self.moved.get(&id).copied().unwrap_or_else(|| info.school().unwrap_or(FAVORITES));
        self.built = None;
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, dt: f32) {
        self.clock += dt;
        if let Some((id, _, _, held)) = self.press.as_mut() {
            *held += dt;
            if *held >= 0.3 {
                self.drag = Some(*id);
                self.press = None;
                self.qualified_click = true;
            }
        }
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

    /// GC1001b54b validates through1004f6a0 but sends the ORIGINAL target.
    /// The fallback to own identity belongs only to local validation.
    pub(super) fn cast(&mut self, gui: &mut Gui, zone: &Zone, id: i32) {
        let target = zone.selected_target().unwrap_or_default();
        let accepted = zone.nanos.programs.contains(&id) && self.db.info(gui, id).is_some_and(|info| {
            match info.can_cast(zone, target).and_then(|accepted| if accepted { runtime_admission(info, zone, target) } else { Ok(false) }) {
                Ok(accepted) => accepted,
                Err(error) => {
                    eprintln!("nano {id}: {error:#}");
                    false
                }
            }
        });
        if !accepted {
            self.feedback.push("Feedback_UnableToExecuteOnThisTarget");
            return;
        }
        let info = self.db.info(gui, id).expect("admitted known nano");
        let name = info.name.clone();
        let effect = info.stat(366).unwrap_or(49999);
        if effect != 0 { self.initiations.push((id, effect)); }
        let payload = nano::cast_nano(zone.char_id as i32, id, target);
        self.outbox.push(outgoing::n3_frame(0, zone.char_id, payload));
        self.casts.push(name);
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
            Event::WindowFrame { window } if *window == w => {
                if win.w.reflow_grid(gui, lv::ICON_32) {
                    self.built = None;
                }
                return true;
            }
            Event::MenuPicked { id } if win.w.owns_menu(*id) => {
                match *id {
                    MENU_INFO => {
                        if let Some(row) = self.ctx.take() {
                            self.urls.push(format!("itemid://{}/{}", nano::NANO_KIND, row));
                        }
                    }
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
        if self.qualified_click && matches!(ev, Event::CanvasPress { .. } | Event::Clicked { .. }) {
            return false;
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
                gui.open_menu((at.0, at.1), (self.screen.0 as i32, self.screen.1 as i32), vec![ao_gui::MenuItem::entry(MENU_INFO, &self.text("LookAtNano")), ao_gui::MenuItem::entry(mid, &label)]);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
impl HudNano {
    pub(super) fn first_point(&self, gui: &Gui) -> (f32, f32) {
        let win = self.win.as_ref().unwrap();
        let r = gui.view_rect(win.w.win, "grid").unwrap();
        (r.l + lv::BORDER + 16.0, r.t + lv::BORDER + 16.0)
    }

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

    #[test]
    fn native_runtime_motion_target_and_exact_range_guards() {
        let mut zone = Zone::new(7);
        let mut info = NanoInfo { id: 1, name: String::new(), stats: vec![(stat::FLAGS, 0)], criteria: vec![], spells: vec![], icon: None };
        assert!(runtime_admission(&info, &zone, Identity::default()).unwrap());
        let idle = NanoMotion { pos: [0.0; 3], radius: 0.5, mode: 3, moving: false, falling: false, input_locked: false };
        for mode in [4, 5, 8, 9, 11, 12] {
            zone.nano_motion = Some(NanoMotion { mode, ..idle });
            assert!(!runtime_admission(&info, &zone, Identity::default()).unwrap());
        }
        for motion in [NanoMotion { moving: true, ..idle }, NanoMotion { falling: true, ..idle }, NanoMotion { input_locked: true, ..idle }] {
            zone.nano_motion = Some(motion);
            assert!(!runtime_admission(&info, &zone, Identity::default()).unwrap());
        }
        zone.nano_motion = None;
        let target = Identity { kind: outgoing::DYNEL_CHAR, instance: 8 };
        zone.character_stats.entry(8).or_default().insert(662, 1);
        assert!(!runtime_admission(&info, &zone, target).unwrap());
        zone.character_stats.get_mut(&8).unwrap().insert(662, 0);
        zone.dynels.insert(8, super::super::zone::DynelState { name: String::new(), pos: [0.0; 3], yaw: None, npc: true, side: 0, level: 1, health: 10, max_health: 10 });
        zone.dynels.insert(7, super::super::zone::DynelState { npc: false, ..zone.dynels[&8].clone() });
        zone.character_fight_levels.extend([(7, 2), (8, 2)]);
        zone.playfield = Some(4582);
        assert!(!runtime_admission(&info, &zone, target).unwrap(), "friendly player nano cannot assist an unrelated NPC");
        zone.pets.push(target);
        assert!(runtime_admission(&info, &zone, target).unwrap());
        zone.pets.clear();
        info.stats[0].1 = 0x8000;
        assert!(runtime_admission(&info, &zone, target).unwrap(), "hostile damage/debuff may target an NPC");
        info.stats[0].1 = 0;
        zone.dynels.get_mut(&8).unwrap().npc = false;
        zone.stats.insert(33, 1);
        zone.character_stats.get_mut(&8).unwrap().insert(33, 2);
        assert!(!runtime_admission(&info, &zone, target).unwrap(), "friendly opposite-side assistance in level2 requires team authorization");
        zone.target_team.team = Some(Identity { kind: ao_net::n3::team::TEAM_KIND, instance: 9 });
        zone.target_team.members.push(super::super::hud_team::Member {
            id: target, name: String::new(), level: 1, sub_team: 0, health: (1, 1), nano: (1, 1),
        });
        assert!(runtime_admission(&info, &zone, target).unwrap(), "native ordinary-team member bypass precedes side comparison");
        zone.target_team.team = None;
        zone.playfield = Some(0x111c);
        zone.stats.insert(668, 7);
        zone.character_stats.get_mut(&8).unwrap().insert(668, 7);
        assert!(runtime_admission(&info, &zone, target).unwrap(), "battlestation admission compares BattlestationSide668");
        zone.character_stats.get_mut(&8).unwrap().insert(668, 8);
        assert!(!runtime_admission(&info, &zone, target).unwrap());
        assert_eq!(nano_range(20, 25), 25);
        assert_eq!(nano_range(25, 3), 25);
        assert_eq!(nano_range(30, 100), 40);
        assert_eq!(nano_range(20, ao_formats::stats::INVALID), -10_784_439, "native IMUL wraps before floating conversion");
        assert!(nano_in_range([0.0; 3], 0.5, [21.25, 0.0, 0.0], 0.75, 20));
        assert!(!nano_in_range([0.0; 3], 0.5, [21.26, 0.0, 0.0], 0.75, 20), "the final native check is not integer-truncated distance");
        assert!(!nano_in_range([0.0; 3], 0.5, [0.0, 22.0, 0.0], 0.75, 20), "range is three dimensional");
    }

    #[test]
    fn captured_own_body_boost_uses_native_presets_and_actual_motion() {
        let line = include_str!("../../../../docs/captures/zone_own_full_character_ithaca.rec");
        let hex = line.split_whitespace().nth(2).unwrap();
        let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let (frame, _) = ao_net::frame::Frame::decode_with(&bytes, false).unwrap().unwrap();
        let mut zone = Zone::new(33588);
        zone.on_frame(&frame);
        assert!(!zone.stats.contains_key(&318));
        assert!(!zone.stats.contains_key(&381));
        assert_eq!((zone.stat(318), zone.stat(381), zone.stat(214)), (Some(100), Some(0), Some(40)));
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let record = store.get(super::super::hud_nanodb::NANO_RDB_TYPE, 29091).unwrap().unwrap();
        let template = ao_formats::dynel_visual::parse_item_template(&record).unwrap();
        let data = super::super::chat::info::template_spells::data(&record, 0).unwrap();
        let criteria = data.criteria.into_iter().rfind(|entry| entry.1 == 3).map(|entry| entry.2).unwrap_or_default();
        let info = NanoInfo { id: 29091, name: template.name.unwrap(), stats: template.stats, criteria, spells: data.spells, icon: None };
        assert_eq!(info.name, "Body Boost");
        let multiplier = super::super::hud_nanodb::nano_cost_multiplier(&zone);
        assert_eq!(info.stat(stat::NANO_POINTS).unwrap() * multiplier / 100, 11);
        assert_eq!(nano_range(info.stat(stat::ATTACK_RANGE).unwrap(), super::super::hud_nanodb::effective_nano_stat(&zone, 381)), 20);
        zone.nano_motion = Some(NanoMotion {
            pos: [0.0; 3], radius: 0.5, mode: 3, moving: false, falling: false, input_locked: false,
        });
        assert!(info.can_cast(&zone, Identity::default()).unwrap());
        assert!(runtime_admission(&info, &zone, Identity::default()).unwrap());
    }

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
            self.ncu.update(&mut self.gui, &self.zone, dt);
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

    /// Explicit synthetic standing caster snapshot for UI tests. The selected
    /// own identity is required by real Shadow Touch Flags0x8c000.
    fn standing_self_caster() -> Zone {
        let mut zone = Zone::new(7);
        zone.stats.insert(214, 10000);
        zone.nano_motion = Some(NanoMotion {
            pos: [0.0; 3], radius: 0.5, mode: 3, moving: false, falling: false, input_locked: false,
        });
        zone.set_target(Some(Identity { kind: outgoing::DYNEL_CHAR, instance: 7 }));
        zone
    }

    #[test]
    fn native_cast_guards_real_templates_and_original_wire_target() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut programs = HudNano::new(&dir, (1100, 760)).unwrap();
        let mut zone = Zone::new(7);
        let own = Identity { kind: outgoing::DYNEL_CHAR, instance: 7 };
        zone.stats.extend([(60, 5), (129, 662), (128, 223), (127, 199), (130, 169), (131, 74), (122, 35), (251, 0), (359, 0), (214, 10000)]);
        let other = Identity { kind: outgoing::DYNEL_CHAR, instance: 8 };
        zone.dynels.insert(8, super::super::zone::DynelState { name: "Target".into(), pos: [0.0; 3], yaw: None, npc: false, side: 0, level: 1, health: 10, max_health: 10 });
        zone.dynels.insert(7, zone.dynels[&8].clone());
        zone.character_fight_levels.extend([(7, 2), (8, 2)]);
        zone.playfield = Some(4582);
        for (id, requires_target, requirement, threshold) in [
            (163449, true, None, 0), (25980, true, Some(129), 661), (25982, false, Some(128), 222),
            (26465, false, Some(127), 18), (28612, true, Some(130), 4),
            (43324, false, Some(131), 3), (56027, true, Some(131), 73), (26707, true, Some(122), 34),
        ] {
            zone.stats.insert(368, if id == 43324 { 12 } else { 11 });
            // Each row begins with sufficient skills; strict threshold tests below
            // must not leak reduced values into the next real program.
            zone.stats.extend([(129, 662), (128, 223), (127, 199), (130, 169), (131, 74), (122, 35)]);
            programs.cast(&mut gui, &zone, id);
            assert!(programs.take_outbox().is_empty(), "unlearned real program {id}");
            assert!(programs.take_casts().is_empty());
            assert_eq!(programs.take_feedback(), ["Feedback_UnableToExecuteOnThisTarget"]);
            zone.nanos.programs.push(id);
            for target in [None, Some(own), Some(other)] {
                zone.set_target(target);
                programs.cast(&mut gui, &zone, id);
                let frames = programs.take_outbox();
                if requires_target && target.is_none() {
                    assert!(frames.is_empty());
                    assert!(programs.take_casts().is_empty());
                    assert_eq!(programs.take_feedback(), ["Feedback_UnableToExecuteOnThisTarget"]);
                    assert_eq!(programs.take_initiations().count(), 0);
                } else {
                    assert_eq!(frames.len(), 1, "real template {id}");
                    assert_eq!(frames[0].payload, nano::cast_nano(7, id, target.unwrap_or_default()));
                    assert_eq!(programs.take_casts().len(), 1);
                    assert!(programs.take_feedback().is_empty());
                    let effect = programs.db.info(&mut gui, id).unwrap().stat(366).unwrap_or(49999);
                    let expected = if effect != 0 { vec![(id, effect)] } else { vec![] };
                    assert_eq!(programs.take_initiations().collect::<Vec<_>>(), expected, "real native initiation selector, independent of relay effect428");
                }
            }
            if let Some(stat) = requirement {
                zone.stats.insert(stat, threshold);
                programs.cast(&mut gui, &zone, id);
                assert!(programs.take_outbox().is_empty(), "strict real Use3 requirement");
                assert!(programs.take_casts().is_empty());
                assert_eq!(programs.take_feedback(), ["Feedback_UnableToExecuteOnThisTarget"]);
                zone.stats.insert(stat, threshold + 1);
            }
            if id != 163449 {
                zone.stats.insert(214, 0);
                programs.cast(&mut gui, &zone, id);
                assert!(programs.take_outbox().is_empty(), "real template nano-point cost");
                assert!(programs.take_casts().is_empty());
                assert_eq!(programs.take_feedback(), ["Feedback_UnableToExecuteOnThisTarget"]);
                zone.stats.insert(214, 10000);
            }
            if id == 43324 {
                zone.stats.insert(251, 1);
                programs.cast(&mut gui, &zone, id);
                assert!(programs.take_outbox().is_empty(), "real pet-count requirement forbids a second attack pet");
                assert!(programs.take_casts().is_empty());
                assert_eq!(programs.take_feedback(), ["Feedback_UnableToExecuteOnThisTarget"]);
                zone.stats.insert(251, 0);
            }
            if matches!(id, 28612 | 43324 | 56027) {
                zone.stats.insert(368, 0);
                programs.cast(&mut gui, &zone, id);
                assert!(programs.take_outbox().is_empty(), "real VisualProfession requirement");
                assert!(programs.take_casts().is_empty());
                assert_eq!(programs.take_feedback(), ["Feedback_UnableToExecuteOnThisTarget"]);
                zone.stats.insert(368, if id == 43324 { 12 } else { 11 });
            }
            zone.set_target(Some(Identity { kind: 51035, instance: 42 }));
            programs.cast(&mut gui, &zone, id);
            assert!(programs.take_outbox().is_empty(), "selected objects are not character targets");
            assert!(programs.take_casts().is_empty());
            assert_eq!(programs.take_feedback(), ["Feedback_UnableToExecuteOnThisTarget"]);
            zone.set_target(None);
        }
    }

    #[test]
    fn server_upload_refreshes_open_programs_and_casts() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, (1100, 760));
        let mut programs = HudNano::new(&dir, (1100, 760)).unwrap();
        let mut zone = standing_self_caster();
        programs.open(&mut gui, &mut rollup);
        assert_eq!(programs.page, FAVORITES);
        programs.update(&mut gui, &zone, 0.0);
        assert!(programs.shown().is_empty());
        let learned = ao_net::n3::action::simple(nano::action::LEARNED, Identity::default(), nano::nano(163449));
        let frame = outgoing::n3_frame(0, 7, ao_net::n3::action::character_action(7, &learned));
        zone.on_frame(&frame);
        for id in zone.nanos.take_learned() {
            programs.select_learned(&mut gui, id);
        }
        assert_eq!(programs.page, 4, "upload selects Space rather than leaving the empty Favorites page");
        programs.update(&mut gui, &zone, 0.0);
        assert_eq!(zone.nanos.programs, [163449]);
        assert_eq!(programs.shown(), [163449]);
        zone.on_frame(&frame);
        assert_eq!(zone.nanos.programs, [163449], "duplicate server upload is idempotent");
        assert!(zone.nanos.take_learned().is_empty());
        programs.cast(&mut gui, &zone, 163449);
        assert_eq!(programs.take_outbox()[0].payload, nano::cast_nano(7, 163449, Identity { kind: outgoing::DYNEL_CHAR, instance: 7 }));
    }

    #[test]
    fn programs_raw_ctrl_selects_and_casts_but_shift_inspects() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        for (mods, inspect) in [(ao_gui::Modifiers { ctrl: true, ..Default::default() }, false), (ao_gui::Modifiers { shift: true, ..Default::default() }, true)] {
            let size = (1100, 760);
            let mut gui = Gui::new(&dir, None).unwrap();
            let mut rollup = Rollup::new(&dir, size);
            let mut programs = HudNano::new(&dir, size).unwrap();
            let mut zone = standing_self_caster();
            zone.nanos.programs = vec![163449];
            programs.open(&mut gui, &mut rollup);
            programs.set_page(4);
            programs.update(&mut gui, &zone, 0.0);
            gui.frame(0.0);
            let (x, y) = programs.first_point(&gui);
            for click in 0..2 {
                let mut double_press = false;
                for ev in [InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left }, InputEvent::MouseUp { x, y, button: ao_gui::MouseButton::Left }] {
                    programs.input(&gui, &ev, mods);
                    for event in gui.input(ev) {
                        double_press |= matches!(&event, Event::CanvasPress { clicks: 2, .. });
                        programs.event(&mut gui, &event, &zone);
                    }
                }
                assert_eq!(double_press, click == 1, "raw input must produce the grid double press");
                assert_eq!(programs.win.as_ref().unwrap().w.view.selected, (!inspect).then_some(163449));
                assert_eq!(programs.take_urls(), if inspect { vec!["itemid://53019/163449".to_owned()] } else { vec![] });
                assert!(programs.take_drag().is_none());
                let frames = programs.take_outbox();
                if !inspect && click == 1 {
                    assert_eq!(frames.len(), 1, "Ctrl double click must still cast");
                    assert_eq!(frames[0].payload, nano::cast_nano(7, 163449, Identity { kind: outgoing::DYNEL_CHAR, instance: 7 }));
                } else {
                    assert!(frames.is_empty(), "only an ordinary double click casts");
                }
                gui.frame(0.01);
            }
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
        zone.nanos.buffs = vec![super::super::own_nanos::Buff { nano: 25982, started: 0.0, total_cs: 45000, ncu_cost: 14, ..Default::default() }];
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
        // Merely displaying a program does not cast it.
        let frames = s.nano.take_outbox();
        assert!(frames.is_empty());
    }

    /// `MP_Spell_Icon`: `icon_at` finds the Programs grid icon and misses empty space (regression for the shift-to-inspect cursor).
    #[test]
    fn icon_at_finds_programs_grid_icon() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let size = (1100, 760);
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, size);
        let mut programs = HudNano::new(&dir, size).unwrap();
        let mut zone = Zone::new(7);
        zone.nanos.programs = vec![163449];
        zone.nanos.serial += 1;
        programs.open(&mut gui, &mut rollup);
        programs.set_page(4);
        programs.update(&mut gui, &zone, 0.0);
        assert_eq!(programs.shown(), vec![163449]);
        let win = programs.win.as_ref().unwrap();
        let r = gui.view_rect(win.w.win, "grid").unwrap();
        assert!(programs.icon_at(&gui, r.l + lv::BORDER + 2.0, r.t + lv::BORDER + 2.0));
        assert!(!programs.icon_at(&gui, 0.0, 0.0));
    }
}
