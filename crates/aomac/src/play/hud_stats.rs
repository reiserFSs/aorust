//! Skills, inventory, wear and stat ("character / equipment") windows of the in-world interface. Evidence: docs/gui.md §11.
//!
//! * Skills: the client's own `Views/Skills.xml` (`SkillWindow` ctor `FUN_100fc18e` GUI 0x100fc18e); the per-group stat rows are
//!   built in code by the original (`StatRow` ctor `FUN_100fe956`) and by us as view XML with the same widgets; the IP arithmetic is [`skill_model`].
//! * Wear: `WearView_c` (`FUN_100e1bc9`): a tab view of item grids drawn over `GFX_GUI_WEARVIEW_{WEAPON,CLOTHING,IMPLANTS}`.
//! * Inventory: `InventoryView_c` (`FUN_100cc2ca`): a `MultiListView` slot grid / list, 30 slots for the character's own inventory.
//! * Stat: `StatView_c` (`FUN_1007ff6e`), [`stat_view`].

mod buffs;
pub(in crate::play) mod items;
mod skill_model;
mod stat_view;
mod zone_inv;
pub(in crate::play) mod inv_grid;
mod item_dnd;
mod item_ui;

use super::hud::WindowKind;
use super::hud_rollup::Rollup;
use super::zone::Zone;
use ao_formats::{screens::TextDb, stats};
use ao_gui::{CanvasItem, CanvasTip, Event, Gui, ViewHandle, WindowId, WindowSize};
use ao_net::frame::Frame;
use items::Items;
use skill_model::{is_disabled, Model};
use stat_view::StatView;
use std::path::Path;

/// `Window(Rect(200,180)..(850,700))` of `SkillWindow` (`FUN_100fc18e`: floats 0x101a959c = 200, 0x101a95a0 = 180, 0x101c10b0 = 850,
/// 0x101a95a8 = 700). Rects are inclusive: 651 x 521 outer.
const SKILLS_POS: (i32, i32) = (200, 180);
const SKILLS_OUTER: (u32, u32) = (651, 521);
/// `GFX_GUI_WEARVIEW_*` art size; the three tab headers are read off the art (x ranges below).
const WEAR_ART: (u32, u32) = (192, 320);
/// Tab header x ranges inside the art (measured at row y = 3: separators at x = 72 and 122, outer edges 4 and 188), height 17. The fourth tab,
/// "Social" (`TabView::AppendTab("Social")`, `FUN_100e1bc9`), has no art: UNRESOLVED, it is a plain text button under the art and shows the
/// clothes art (the social grid has the same 15 cells).
const WEAR_TABS: [(&str, &str, i32, i32); 4] = [
    ("weapons", "GFX_GUI_WEARVIEW_WEAPON", 4, 72),
    ("clothes", "GFX_GUI_WEARVIEW_CLOTHING", 72, 122),
    ("implants", "GFX_GUI_WEARVIEW_IMPLANTS", 122, 188),
    ("social", "GFX_GUI_WEARVIEW_CLOTHING", 0, 0),
];
const SLOT_GFX: &str = "GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED";
const SLOT_OPEN_GFX: &str = "GFX_GUI_MULTILISTVIEW_SLOT_48_OPEN";
/// UNRESOLVED GUESS: the vertical gap of the inventory grid. `RecalcCellCount` spaces the rows by `(height + 1 - borders - rows * 48) / (rows - 1)` of the
/// scrolled client view, a value that depends on the view's own content height (circular, not derivable); the wear grids use `SetGridIconSpacing(11, 9)`, so 9.
/// The horizontal count / gap come from [`inv_grid::columns`] (verified).
const INVENTORY_GAP_Y: f32 = 9.0;
/// Width of a `ScrollView` scrollbar (`SCROLLBAR_W`) the inventory's grid leaves room for.
const SCROLLBAR: u32 = 13;
/// The saved frame of the inventory (`prefs/NewChar/DockAreas/DockArea0.xml`, a `DockTabbedWindow`): 187 x 176 inclusive, used as the outer size.
const INVENTORY_FALLBACK: (i32, i32, u32, u32) = (300, 150, 188, 177);
/// Dock identity of the wear view (`WearViewConfig` `DockableViewDockName = "RollupArea"`, page `wear_window` of `RollupArea.xml`).
const WEAR_KEY: &str = "wear_window";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `FUN_100f8f93`: colour of a skill's name by `GetSkillCostLevel` (the cost factor): the cheaper the skill for the profession, the greener.
fn cost_color(level: i32) -> u32 {
    match level {
        ..=5 => 0x7dcc5e,
        6..=10 => 0x00cc88,
        11..=15 => 0x00cccc,
        16..=20 => 0x0088cc,
        _ => 0x075aff,
    }
}

/// One skill row (`StatRow`): the view instance and what it last showed.
struct SkillRow {
    stat: u32,
    h: ViewHandle,
    shown: i32,
    /// Colour last given to the value text (`FUN_100fde49`: white / green / red, [`skill_model::Row::color`]).
    color: u32,
    pending: i32,
    frac: f32,
}

struct Skills {
    window: WindowId,
    /// group index -> rows
    rows: Vec<Vec<SkillRow>>,
    group: Option<usize>,
    selected: Option<u32>,
    ip: i32,
    reset: i32,
    /// The rows exist (they are built by the first [`HudStats::update`] after opening: their colours need the own profession).
    built: bool,
    /// Arrow art: dec, dec changed, inc, inc changed (`FUN_100f8ff1`).
    arrows: [(ao_gui::GfxId, u32, u32); 4],
    /// What the details panel showed last (refreshed only on change).
    details: String,
    /// The "Reset" confirmation (`SkillResetAll` dialog): window and the stat to reset (0 = all).
    confirm: Option<(WindowId, u32)>,
    status_shown: String,
}

struct Tabbed {
    window: WindowId,
    tab: usize,
    /// (slot, low_id) of what the item layer shows.
    drawn: Vec<(u32, i32)>,
}

struct Inventory {
    window: WindowId,
    /// The `listview_mode` pref of the NewChar template (list instead of grid) and the grid's column count.
    list: bool,
    drawn: Vec<(u32, i32)>,
    cols: usize,
    /// Grid cell gaps (x from [`inv_grid::columns`], y [`INVENTORY_GAP_Y`]) and the grid view's width in px.
    gap: (f32, f32),
    view_w: f32,
    /// Client side cells of the bag items (`item_position_map`) and the cell the last item dropped from outside was released on.
    positions: inv_grid::PositionMap,
    pending_drop: Option<(usize, usize)>,
}

pub(super) struct HudStats {
    db: TextDb,
    model: Model,
    items: Items,
    skills: Option<Skills>,
    wear: Option<Tabbed>,
    inventory: Option<Inventory>,
    stat: Option<StatView>,
    /// Windows closed by their frame button since the last [`HudStats::take_closed`].
    closed: Vec<WindowKind>,
    /// Zone frames produced by the windows (`SkillIIR_t`, `ResetSkill`); the HUD forwards them.
    outbox: Vec<Frame>,
    screen: (u32, u32),
    /// `listview_mode` of `prefs/NewChar/Containers/Inventory.xml` and the saved `WindowFrame` of `DockAreas/DockArea0.xml`.
    inventory_list: bool,
    inventory_frame: (i32, i32, u32, u32),
    /// Feedback line of the skill window (`status` view): `Skill_LackIP`.
    status: String,
    /// Pointer state of the item drag and drop, and the sequence number of our `GenericCmd_t`s (docs/gui.md §11.12).
    dnd: item_dnd::Dnd,
    use_seq: i32,
}

/// Value of the first `<Bool name=NAME value=..>` below `e`.
fn find_bool(e: &ao_gui::xml::Element, name: &str) -> Option<bool> {
    e.children.iter().find_map(|c| if c.name == "Bool" && c.attr("name") == Some(name) { c.attr("value").map(|v| v == "true") } else { find_bool(c, name) })
}

/// `WindowFrame` `Rect(l,t,r,b)` below `e`, as (x, y, outer width, outer height) of the inclusive rectangle.
fn find_frame(e: &ao_gui::xml::Element) -> Option<(i32, i32, u32, u32)> {
    e.children.iter().find_map(|c| {
        if c.name == "Rect" && c.attr("name") == Some("WindowFrame") {
            let n: Vec<f32> = c.attr("value")?.trim_start_matches("Rect(").trim_end_matches(')').split(',').filter_map(|p| p.trim().parse().ok()).collect();
            (n.len() == 4).then(|| (n[0] as i32, n[1] as i32, (n[2] - n[0]) as u32 + 1, (n[3] - n[1]) as u32 + 1))
        } else {
            find_frame(c)
        }
    })
}

impl HudStats {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> anyhow::Result<Self> {
        let model = match ao_rdb::RecordStore::open(dir).and_then(|s| ao_formats::stats::skills::SkillTables::load(&s)) {
            Ok(t) => Model::new(t, stats::load_ipdist(dir).map_err(|e| eprintln!("hud: ipdist: {e:#}")).unwrap_or_default()),
            Err(e) => {
                eprintln!("hud: skill tables: {e:#}");
                Model::default()
            }
        };
        let read = |f: &str| std::fs::read_to_string(dir.join("prefs/NewChar").join(f)).ok().and_then(|t| ao_gui::xml::parse(&t).ok());
        let inventory_list = read("Containers/Inventory.xml").and_then(|e| find_bool(&e, "listview_mode")).unwrap_or(false);
        let inventory_frame = read("DockAreas/DockArea0.xml").and_then(|e| find_frame(&e)).unwrap_or(INVENTORY_FALLBACK);
        Ok(Self {
            db: TextDb::load(dir)?,
            model,
            items: Items::new(dir),
            skills: None,
            wear: None,
            inventory: None,
            stat: None,
            closed: vec![],
            outbox: vec![],
            screen,
            inventory_list,
            inventory_frame,
            status: String::new(),
            dnd: Default::default(),
            use_seq: 0,
        })
    }

    pub(super) fn set_screen(&mut self, size: (u32, u32)) {
        self.screen = size;
    }

    pub(super) fn is_open(&self, kind: WindowKind) -> bool {
        match kind {
            WindowKind::Skills => self.skills.is_some(),
            WindowKind::Inventory => self.inventory.is_some(),
            WindowKind::Character => self.wear.is_some(),
            WindowKind::Stat => self.stat.is_some(),
            _ => false,
        }
    }

    #[cfg(test)]
    pub(super) fn window(&self, kind: WindowKind) -> Option<WindowId> {
        match kind {
            WindowKind::Skills => self.skills.as_ref().map(|s| s.window),
            WindowKind::Inventory => self.inventory.as_ref().map(|i| i.window),
            WindowKind::Character => self.wear.as_ref().map(|t| t.window),
            WindowKind::Stat => self.stat.as_ref().map(|s| s.window),
            _ => None,
        }
    }

    /// Kinds whose frame close button was pressed (the caller clears their menu state).
    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> {
        std::mem::take(&mut self.closed)
    }

    /// Zone frames queued by the windows since the last call.
    pub(super) fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// Inventory items dropped over a foreign window since the last call: `(slot, x, y)` (the NPC trade window takes them).
    #[allow(dead_code)] // consumed by interact_trade.rs / interact_ptrade.rs
    pub(in crate::play) fn take_drops(&mut self) -> Vec<(u32, f32, f32)> {
        std::mem::take(&mut self.dnd.dropped)
    }

    #[allow(dead_code)] // consumed by interact_trade.rs / interact_ptrade.rs
    pub(in crate::play) fn requeue_drops(&mut self, drops: Vec<(u32, f32, f32)>) {
        self.dnd.dropped.extend(drops);
    }

    /// Name and icon of the item template `low_id` (`items.rs` cache).
    #[allow(dead_code)] // consumed by interact_trade.rs / interact_ptrade.rs
    pub(in crate::play) fn item_info(&mut self, gui: &mut Gui, low_id: i32) -> Option<(String, Option<(ao_gui::GfxId, u32, u32)>)> {
        self.items.info(gui, low_id).map(|i| (i.name.clone(), i.icon))
    }

    pub(super) fn open(&mut self, gui: &mut Gui, rollup: &mut Rollup, kind: WindowKind) {
        if self.is_open(kind) {
            return;
        }
        let r = match kind {
            WindowKind::Skills => self.open_skills(gui),
            WindowKind::Inventory => self.open_inventory(gui),
            WindowKind::Character => self.open_wear(gui, rollup),
            WindowKind::Stat => self.open_stat(gui, rollup),
            _ => return,
        };
        if let Err(e) = r {
            eprintln!("hud: opening {kind:?}: {e:#}");
        }
    }

    /// Closes the skills window (own style-0 frame).
    fn close_skills(&mut self, gui: &mut Gui) {
        if let Some(s) = self.skills.take() {
            if let Some((c, _)) = s.confirm {
                gui.close_window(c);
            }
            gui.close_window(s.window);
            // leaving the window drops the unsaved points (`FUN_100f91d7` runs at save; closing destroys the rows)
            self.model.clear();
        }
    }

    fn close_inventory(&mut self, gui: &mut Gui) {
        if let Some(i) = self.inventory.take() {
            gui.close_window(i.window);
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, rollup: &mut Rollup, kind: WindowKind) {
        match kind {
            WindowKind::Skills => self.close_skills(gui),
            WindowKind::Inventory => self.close_inventory(gui),
            WindowKind::Character => {
                if self.wear.take().is_some() {
                    rollup.close_page(gui, WEAR_KEY);
                }
            }
            WindowKind::Stat if self.stat.take().is_some() => rollup.close_page(gui, stat_view::KEY),
            _ => {}
        }
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui, rollup: &mut Rollup) {
        for k in [WindowKind::Skills, WindowKind::Inventory, WindowKind::Character, WindowKind::Stat] {
            self.close(gui, rollup, k);
        }
    }

    // ------------------------------------------------------------------------------------------------------------ stat

    /// `stat_window` (`StatView_c`): docked in the rollup column (`StatViewConfig` `DockableViewDockName = "RollupArea"`, 4th page of the template, 279 px).
    fn open_stat(&mut self, gui: &mut Gui, rollup: &mut Rollup) -> anyhow::Result<()> {
        self.stat = Some(StatView::open(gui, &self.db, rollup)?);
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------------------ skills

    fn open_skills(&mut self, gui: &mut Gui) -> anyhow::Result<()> {
        // The original window is `Window(Rect(200,180,850,700), "", "Skills", style 0, flags 0x1000)` with one tab "Skills": the style-0 frame with the
        // tab strip (docs/gui.md §6.1, client insets 5, 26, 5, 5) of the same outer size.
        let client = (SKILLS_OUTER.0 - 10, SKILLS_OUTER.1 - 31);
        let w = gui.open_tabbed_window("Skills", "Skills", SKILLS_POS, WindowSize::Fixed(client.0, client.1))?;
        for g in &stats::SKILL_GROUPS {
            gui.set_text(w, g.prefix, &self.db.by_id(10010, g.label).unwrap_or_default());
            gui.show_collapsing(w, &format!("{}_view", g.prefix), false);
            gui.show_collapsing(w, &format!("{}_group", g.prefix), false);
        }
        gui.select_child(w, "infoselect", Some(0));
        gui.select_child(w, "groupselect", None);
        gui.show_collapsing(w, "statdetails", false);
        gui.show_collapsing(w, "disabledLbl", false);
        gui.select_child(w, "resetselect", None);
        let art = |n: &str| gui.gfx_id(n).map(ao_gui::GfxId).map(|g| (g, gui.gfx().size(g).0, gui.gfx().size(g).1)).unwrap_or((ao_gui::GfxId(0), 0, 0));
        let arrows = ["GFX_GUI_DEC_SKILL", "GFX_GUI_DEC_SKILL_CHANGED", "GFX_GUI_INC_SKILL", "GFX_GUI_INC_SKILL_CHANGED"].map(art);
        self.skills = Some(Skills {
            window: w,
            rows: vec![],
            group: None,
            selected: None,
            ip: i32::MIN,
            reset: i32::MIN,
            built: false,
            arrows,
            details: String::new(),
            confirm: None,
            status_shown: String::new(),
        });
        Ok(())
    }

    /// The `StatRow` of every skill (`FUN_100fe956`): name button (coloured by the cost level), value, the two arrow buttons and the power bar.
    /// Built on the first update, when the own profession is known.
    fn build_skill_rows(&mut self, gui: &mut Gui, zone: &Zone) -> anyhow::Result<()> {
        let Some(s) = self.skills.as_mut() else { return Ok(()) };
        let get = |id: u32| zone.stat(id);
        let ch = stats::skills::Character::from_stats(get);
        let (dw, dh) = (s.arrows[0].1, s.arrows[0].2.max(s.arrows[2].2));
        let w = s.window;
        for g in &stats::SKILL_GROUPS {
            let mut r = vec![];
            for &st in g.stats {
                let st = st as u32;
                let color = cost_color(self.model.tables.cost_level(st, &ch));
                let xml = format!(
                    "<root><View view_layout=\"horizontal\">\
                     <TextButton name=\"name\" text=\"{}\" color=\"0x{color:06X}\" hover_color=\"TEXT_HOVER\" pressed_color=\"TEXT_SELECTED\" layout_borders=\"Rect(3,0,0,0)\"/>\
                     <HLayoutSpacer/><TextView name=\"value\" value=\"0\" layout_borders=\"Rect(0,0,3,0)\"/>\
                     <CanvasView name=\"btn{st}\" min_size=\"Point({bw},{dh})\" max_size=\"Point({bw},{dh})\" layout_borders=\"Rect(3,3,3,3)\"/>\
                     <PowerBar name=\"bar{st}\" bg_gfx=\"GFX_GUI_HOR_BAR_SMALL_EMPTY\" full_gfx=\"GFX_GUI_HOR_BAR_SMALL_BLUE\" direction=\"right\" layout_borders=\"Rect(0,0,5,0)\"/>\
                     </View></root>",
                    esc(&stats::short_name(&self.db, st)),
                    bw = dw + s.arrows[2].1,
                );
                let h = gui.add_view_xml(w, &format!("{}_group", g.prefix), "StatRow", &xml)?;
                r.push(SkillRow { stat: st, h, shown: i32::MIN, color: u32::MAX, pending: i32::MIN, frac: -1.0 });
            }
            s.rows.push(r);
        }
        s.built = true;
        // word-wrapped text sizes itself from its current frame (`TextRenderer_c::CalculatePreferredSize`): a second pass uses the frames of the first
        gui.relayout_window(w);
        Ok(())
    }

    /// Opens the confirmation of "Reset all skills" (`stat` 0) / "Reset this skill": the texts are text.mdb 502 (`SkillResetAll` dialog text of the
    /// full IP reset, "Do you really want to reset the skill %s?"); the dialog's layout itself is UNRESOLVED (a `DialogBox` built in the DLL).
    fn ask_reset(&mut self, gui: &mut Gui, stat: u32) {
        let Some(s) = self.skills.as_mut() else { return };
        if s.confirm.is_some() {
            return;
        }
        let text = if stat == 0 {
            self.db.by_id(502, 109178077).unwrap_or_default()
        } else {
            self.db.by_id(502, 89070509).unwrap_or_default().replace("%s", &stats::long_name(&self.db, stat))
        };
        let xml = format!(
            "<root><View view_layout=\"vertical\" layout_borders=\"Rect(10,10,10,10)\">\
             <TextView name=\"text\" value=\"{}\" feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP\" min_size=\"Point(300,-1)\" max_size=\"Point(400,-1)\" layout_borders=\"Rect(5,5,5,0)\"/>\
             <View view_layout=\"horizontal\" layout_borders=\"Rect(0,10,0,0)\">\
             <Button label=\"#MsgBox_OK\" name=\"reset_ok\" layout_borders=\"Rect(5,5,5,5)\"/>\
             <Button label=\"#MsgBox_Cancel\" name=\"reset_cancel\" layout_borders=\"Rect(5,5,5,5)\"/></View></View></root>",
            esc(&text.replace("\r\n", "\n")),
        );
        match gui.open_framed_window_xml("SkillResetDialog", &xml, (self.screen.0 as i32 / 2 - 190, self.screen.1 as i32 / 2 - 80), WindowSize::Preferred) {
            Ok(w) => {
                gui.relayout_window(w);
                s.confirm = Some((w, stat));
            }
            Err(e) => eprintln!("hud: reset dialog: {e:#}"),
        }
    }

    fn skills_event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let Some(s) = self.skills.as_mut() else { return false };
        let get = |id: u32| zone.stat(id);
        let char_id = zone.char_id;
        // the reset confirmation
        if let Some((cw, stat)) = s.confirm {
            match ev {
                Event::Clicked { window, view, .. } if *window == cw && (view == "reset_ok" || view == "reset_cancel") => {
                    if view == "reset_ok" {
                        self.outbox.push(ao_net::n3::outgoing::n3_frame(0, char_id, ao_net::n3::action::reset_skill(char_id as i32, stat as i32)));
                    }
                    gui.close_window(cw);
                    s.confirm = None;
                    return true;
                }
                Event::CloseRequested { window } if *window == cw => {
                    gui.close_window(cw);
                    s.confirm = None;
                    return true;
                }
                _ => {}
            }
        }
        match ev {
            Event::CloseRequested { window } if *window == s.window => {
                self.close_skills(gui);
                self.closed.push(WindowKind::Skills);
                true
            }
            // the arrow buttons (`FUN_100fe0ca` / `FUN_100fe261`): the left half of the canvas decreases, the right half increases
            Event::CanvasClick { window, view, x, .. } if *window == s.window && view.starts_with("btn") => {
                let Some(stat) = view[3..].parse::<u32>().ok() else { return false };
                if is_disabled(stat) {
                    // "This skill has been disabled and can not be increased / decreased." (literal in `FUN_100fe261` / `FUN_100fe0ca`)
                    self.status = format!("This skill has been disabled and can not be {}.", if *x < s.arrows[0].1 as f32 { "decreased" } else { "increased" });
                } else if *x < s.arrows[0].1 as f32 {
                    if self.model.pending(stat) > 0 {
                        self.model.adjust(&get, stat, -1);
                    }
                } else if self.model.at_max(&get, stat) {
                    // `FUN_100fd5b9` -> `N3Msg_SkillMaxFeedback`: the feedback texts (`Feedback_MaximumIncreaseInAbility` ...) are UNRESOLVED, nothing is said
                } else {
                    let r = self.model.adjust(&get, stat, 1);
                    self.status = if r.lack_ip { self.db.by_id(502, 126196928).unwrap_or_default() } else { String::new() };
                }
                if s.selected == Some(stat) {
                    s.details.clear();
                }
                true
            }
            Event::Clicked { window, view, item } if *window == s.window => {
                if let Some(g) = stats::SKILL_GROUPS.iter().position(|g| g.prefix == view) {
                    // `FUN_100f9552` (not minimised): the info panel shows this group's rows.
                    s.group = Some(g);
                    gui.select_child(s.window, "infoselect", Some(1));
                    gui.select_child(s.window, "groupselect", Some(g));
                } else if view == "name" {
                    let stat = item.and_then(|h| s.rows.iter().flatten().find(|r| r.h == h)).map(|r| r.stat);
                    if let Some(stat) = stat {
                        s.selected = Some(stat);
                        s.details.clear();
                        gui.show_collapsing(s.window, "statdetails", true);
                    }
                } else if view == "Accept" {
                    // `FUN_100fa5dd`: `SkillIIR_t` with `pending + raw` of every changed row
                    let map = self.model.save_map(&get);
                    if !map.is_empty() {
                        self.outbox.push(ao_net::n3::outgoing::n3_frame(0, char_id, ao_net::n3::outgoing::skill_ip_adjust(char_id as i32, &map)));
                    }
                    self.model.clear();
                    s.details.clear();
                } else if view == "suggest_ip" {
                    self.suggest(gui, zone);
                } else if view == "resetAll" {
                    self.ask_reset(gui, 0);
                } else if view == "reset" || view == "resetFree" {
                    if let Some(st) = self.skills.as_ref().and_then(|s| s.selected) {
                        self.ask_reset(gui, st);
                    }
                } else if view == "Quit" {
                    self.close_skills(gui);
                    self.closed.push(WindowKind::Skills);
                } else {
                    return false;
                }
                true
            }
            _ => false,
        }
    }

    /// `suggest_ip` (`FUN_100faec0` → `FUN_100fac49`): the spend loop, then the `autodist` panel listing the modified skills in two columns.
    fn suggest(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(s) = self.skills.as_mut() else { return };
        let get = |id: u32| zone.stat(id);
        let profession = self.db.by_id(2004, zone.stat(stats::PROFESSION).unwrap_or(0).max(0) as u32).unwrap_or_default();
        let raised = self.model.suggest(&get, &profession, |st| stats::short_name(&self.db, st));
        gui.remove_children(s.window, "col1");
        gui.remove_children(s.window, "col2");
        for (i, &st) in raised.iter().enumerate() {
            let col = if i < raised.len().div_ceil(2) { "col1" } else { "col2" };
            let xml = format!("<root><TextView value=\"{}\" layout_borders=\"Rect(5,2,5,0)\"/></root>", esc(&stats::short_name(&self.db, st)));
            if let Err(e) = gui.add_view_xml(s.window, col, "ModifiedSkill", &xml) {
                eprintln!("hud: autodist list: {e:#}");
            }
        }
        gui.select_child(s.window, "infoselect", Some(2));
        gui.relayout_window(s.window);
        s.details.clear();
    }

    fn update_skills(&mut self, gui: &mut Gui, zone: &Zone) {
        if self.skills.as_ref().is_some_and(|s| !s.built) {
            if let Err(e) = self.build_skill_rows(gui, zone) {
                eprintln!("hud: skill rows: {e:#}");
            }
        }
        let Some(s) = self.skills.as_mut() else { return };
        let get = |id: u32| zone.stat(id);
        self.model.refresh_buffs(&get, &zone.active_spells);
        // "Suggested IP distribution" is available until level 20 (inittext).
        gui.set_enabled(s.window, "suggest_ip", zone.stat(stats::LEVEL).unwrap_or(0) < 20);
        // `FUN_100fa45c`: "Reset all skills (n)", n = (stat 0x15c bit 2 clear) + FullIPRPoints (0x2b3); enabled when n >= 1
        let reset = i32::from(zone.stat(0x15c).unwrap_or(0) & 4 == 0) + zone.stat(0x2b3).unwrap_or(0);
        if reset != s.reset {
            s.reset = reset;
            let label = format!("{} ({reset})", self.db.by_id(502, 96620988).unwrap_or_default());
            gui.set_text(s.window, "resetAll", &label);
            gui.set_enabled(s.window, "resetAll", reset >= 1);
        }
        // a lowered ability can shrink other maxima: `FUN_100fe478` takes the surplus pending points back
        for r in s.rows.iter().flatten() {
            if self.model.pending(r.stat) != 0 {
                self.model.clamp(&get, r.stat);
            }
        }
        let ip = self.model.remaining(&get);
        if ip != s.ip {
            s.ip = ip;
            gui.set_text(s.window, "remaining_ip", &ip.to_string());
        }
        for r in s.rows.iter_mut().flatten() {
            let row = self.model.row(&get, r.stat);
            if row.shown() != r.shown {
                r.shown = row.shown();
                gui.set_text_in(r.h, "value", &r.shown.to_string());
            }
            if row.color() != r.color {
                r.color = row.color();
                gui.set_color_in(r.h, "value", r.color);
            }
            let (frac, changed) = (row.fraction(), row.pending != 0);
            if frac != r.frac {
                r.frac = frac;
                gui.set_progress(s.window, &format!("bar{}", r.stat), frac);
            }
            if row.pending != r.pending {
                r.pending = row.pending;
                // `FUN_100f8ff1`: the arrow art of a changed row; decrease on the left, increase on the right
                let (dec, inc) = (s.arrows[usize::from(changed)], s.arrows[2 + usize::from(changed)]);
                let at = |g: (ao_gui::GfxId, u32, u32), x: f32| CanvasItem::Image { id: g.0, src: [0.0, 0.0, g.1 as f32, g.2 as f32], dst: [x, 0.0, x + g.1 as f32, g.2 as f32], alpha: 1.0 };
                gui.set_canvas(s.window, &format!("btn{}", r.stat), vec![at(dec, 0.0), at(inc, dec.1 as f32)]);
            }
        }
        if let Some(stat) = s.selected {
            let row = self.model.row(&get, stat);
            let cost_next = self.model.tables.cost(stat, row.raw + row.pending, &stats::skills::Character::from_stats(get));
            let ch = stats::skills::Character::from_stats(get);
            let total = self.model.tables.cost_of_points(stat, row.raw, row.pending, &ch);
            // `FUN_100f97a6`: base = GetSkill(stat, 1) + pending, buffed = value + pending, maximum = GetSkill(stat, 1) + (max - raw)
            let text = format!("{stat}|{}|{}|{}|{cost_next}|{total}|{}|{}", row.shown(), row.base, row.pending, row.max, row.raw);
            if text != s.details {
                s.details = text;
                gui.set_text(s.window, "statName", &format!("<center>{}</center>", stats::long_name(&self.db, stat)));
                gui.set_text(s.window, "statdesc", &stats::description(&self.db, stat).unwrap_or_default());
                gui.set_text(s.window, "base", &format!("<div align=\"right\">{}</div>", row.base + row.pending));
                // `buffed` = `StatRow+0x1a8` (`GetSkill(stat, 2)`) + pending, `base` = `GetSkill(stat, 1)` + pending (`Row::value` / `Row::base`)
                gui.set_text(s.window, "buffed", &format!("<div align=\"right\">{}</div>", row.shown()));
                gui.set_text(s.window, "maxskill", &format!("<div align=\"right\">{}</div>", row.base + (row.max - row.raw)));
                gui.set_text(s.window, "pointdelta", &format!("<div align=\"right\">{}</div>", row.pending));
                gui.set_text(s.window, "unitcost", &format!("<div align=\"right\">{cost_next}</div>"));
                gui.set_text(s.window, "totalcost", &format!("<div align=\"right\">{total}</div>"));
                let disabled = is_disabled(stat);
                gui.show_collapsing(s.window, "disabledLbl", disabled);
                // `resetselect`: page 0 shows the remaining reset points of the skill (stat 199 `RP`), page 1 the free reset of a deprecated skill
                gui.select_child(s.window, "resetselect", Some(usize::from(disabled)));
                gui.set_text(s.window, "rp", &zone.stat(199).unwrap_or(0).to_string());
            }
        }
        if s.status_shown != self.status {
            s.status_shown = self.status.clone();
            gui.set_text(s.window, "status", &self.status);
        }
    }

    // ---------------------------------------------------------------------------------------------------------- wear / inventory

    fn open_wear(&mut self, gui: &mut Gui, rollup: &mut Rollup) -> anyhow::Result<()> {
        let (aw, ah) = WEAR_ART;
        let mut xml = String::from("<root><View view_layout=\"vertical\" h_alignment=\"left\">");
        xml += &format!("<View view_layout=\"stacked\" min_size=\"Point({aw},{ah})\" max_size=\"Point({aw},{ah})\">");
        for (name, gfx, ..) in WEAR_TABS.iter().take(3) {
            xml += &format!("<BitmapView name=\"{name}\" bitmap_id=\"{gfx}\"/>");
        }
        // the clothes art of the social tab: a second view of it (the three names above toggle visibility one by one)
        xml += &format!("<BitmapView name=\"social\" bitmap_id=\"{}\"/>", WEAR_TABS[3].1);
        xml += &format!("<CanvasView name=\"items\" min_size=\"Point({aw},{ah})\" max_size=\"Point({aw},{ah})\"/>");
        // tab header hit areas laid over the art
        xml += "<View view_layout=\"horizontal\" h_alignment=\"left\" v_alignment=\"top\">";
        xml += &format!("<View min_size=\"Point({0},17)\" max_size=\"Point({0},17)\"/>", WEAR_TABS[0].2);
        for (name, _, l, r) in WEAR_TABS.iter().take(3) {
            xml += &format!("<TextButton name=\"tab_{name}\" text=\"\" min_size=\"Point({0},17)\" max_size=\"Point({0},17)\"/>", r - l);
        }
        xml += "<HLayoutSpacer/></View></View>";
        xml += "<View view_layout=\"horizontal\" h_alignment=\"left\"><TextButton name=\"tab_social\" text=\"Social\" color=\"0xFFFFFF\" hover_color=\"TEXT_HOVER\" pressed_color=\"TEXT_SELECTED\" layout_borders=\"Rect(4,3,4,0)\"/></View>";
        xml += "</View></root>";
        // `WearView_c` (`FUN_100e1bc9`) is a `DockableView_c` titled `GetText(10000, "Wear")`, docked in the rollup column (`WearViewConfig`: `DockableViewDockName = "RollupArea"`, page 317 px)
        let w = rollup.open_page(gui, WEAR_KEY, &self.db.by_key(10000, "Wear").unwrap_or_default(), &xml, ah as f32)?;
        self.wear = Some(Tabbed { window: w, tab: 0, drawn: vec![(u32::MAX, 0)] });
        self.select_wear_tab(gui, 0);
        Ok(())
    }

    fn select_wear_tab(&mut self, gui: &mut Gui, tab: usize) {
        let Some(t) = self.wear.as_mut() else { return };
        t.tab = tab;
        for (i, (name, ..)) in WEAR_TABS.iter().enumerate() {
            gui.set_visible(t.window, name, i == tab);
        }
        t.drawn = vec![(u32::MAX, 0)];
    }

    /// The item layer of the wear window: the picture of every worn item in the cell of its slot (the frames and labels are in the art).
    fn update_wear(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(t) = self.wear.as_mut() else { return };
        let mut sig = signature(zone);
        sig.push((u32::MAX - 2, hover_code(self.dnd.hover)));
        if t.drawn == sig {
            return;
        }
        for e in zone.inventory.values() {
            self.items.info(gui, e.item.low_id);
        }
        let (mut cmds, mut tips) = (vec![], vec![]);
        for cell in 0..15 {
            let Some(slot) = items::wear_slot(t.tab, cell) else { continue };
            let (c, r) = items::wear_cell(t.tab, cell);
            let (x, y) = items::wear_origin(c, r);
            if let Some(e) = zone.inventory.get(&slot) {
                cmds.extend(self.items.picture(gui, e, x, y));
                if let Some(i) = self.items.info(gui, e.item.low_id) {
                    tips.push(CanvasTip { rect: [x, y, x + items::SLOT, y + items::SLOT], title: i.name.clone(), body: String::new() });
                }
            }
            // drop target under the dragged item (UNRESOLVED GUESS: the original's highlight was not located; a light veil over the accepting cell)
            if let Some((item_dnd::Place::Wear { tab, slot: hs, .. }, true)) = self.dnd.hover {
                if tab == t.tab && hs == slot {
                    cmds.push(CanvasItem::Solid { dst: [x, y, x + items::SLOT, y + items::SLOT], color: 0xFFFFFF, alpha: 0.3 });
                }
            }
        }
        gui.set_canvas(t.window, "items", cmds);
        gui.set_canvas_tips(t.window, "items", tips);
        t.drawn = sig;
    }

    fn open_inventory(&mut self, gui: &mut Gui) -> anyhow::Result<()> {
        let (x, y, ow, oh) = self.inventory_frame;
        let xml = "<root><View view_layout=\"vertical\"><ScrollView name=\"scrollview\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"auto\"><ScrollViewChild view_layout=\"vertical\" name=\"scroller\">\
                   <View view_layout=\"vertical\" name=\"content\"/></ScrollViewChild></ScrollView></View></root>";
        // `DockTabbedWindow` = a style-0 window (client insets 5, 26, 5, 5, docs §6.1) whose only tab is titled `GetText(10000, "Inventory")`
        // (`FUN_100c9fb7`, string at GUI 0x101bb518; retail screenshot: tab "Inventory" with the "i" icon, pin and close buttons).
        let title = self.db.by_key(10000, "Inventory").unwrap_or_default();
        // the grid view is the frame's client width; its column count and spacing follow `RecalcCellCount` (`Rect::Width` = inclusive width - 1);
        // the scrollbar is added beside it (UNRESOLVED: the original's scrollbar / grid width interplay; retail shows 3 columns next to the scrollbar)
        let view_w = ow as f32 - 10.0;
        let (cols, gap_x) = inv_grid::columns(view_w - 1.0);
        let w = gui.open_tabbed_window_xml("InventoryView", &title, xml, (x, y), WindowSize::Fixed(view_w as u32 + SCROLLBAR, oh.saturating_sub(31).max(40)))?;
        // `Window::MoveInsideScreen`: the saved frame comes from a bigger screen
        let (ow, oh) = gui.outer_size(w);
        gui.set_window_pos(w, (x.min(self.screen.0.saturating_sub(ow) as i32).max(0), y.min(self.screen.1.saturating_sub(oh) as i32).max(0)));
        self.inventory = Some(Inventory {
            window: w,
            list: self.inventory_list,
            drawn: vec![(u32::MAX, 0)],
            cols,
            gap: (gap_x, INVENTORY_GAP_Y),
            view_w,
            positions: Default::default(),
            pending_drop: None,
        });
        Ok(())
    }

    /// Toggles between the grid and the list of the inventory (`InventoryViewMode` pref; where the original toggles it is UNRESOLVED).
    #[cfg(test)]
    pub(super) fn set_inventory_list(&mut self, list: bool) {
        if let Some(i) = self.inventory.as_mut() {
            i.list = list;
            i.drawn = vec![(u32::MAX, 0)];
        }
    }

    /// Grid: 30 slot frames (`SetMaxItemCount(0x1e)`) with the item pictures in bag slots `0x40..`; list: a row per item with Icon / Name / Count / Quality
    /// (columns `AddColumn` 0 "Icon" 16 px, 1 "Name" 200 px, 2 "Count" 30 px, 4 "Quality" 100 px of the template's `listview_config`).
    fn update_inventory(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(inv) = self.inventory.as_mut() else { return };
        let mut slots: Vec<u32> = zone.inventory.keys().copied().filter(|s| (items::BAG_FIRST..items::BAG_FIRST + items::BAG_SLOTS).contains(s)).collect();
        slots.sort_unstable();
        inv.positions.sync(&slots, inv.cols, &mut inv.pending_drop);
        let mut sig = signature(zone);
        sig.push((u32::MAX - 1, i32::from(inv.list)));
        sig.push((u32::MAX - 2, hover_code(self.dnd.hover)));
        sig.extend(slots.iter().filter_map(|&s| inv.positions.get(s).map(|c| (0x8000_0000 | s, (c.0 * 100 + c.1) as i32))));
        if inv.drawn == sig {
            return;
        }
        for e in zone.inventory.values() {
            self.items.info(gui, e.item.low_id);
        }
        let slot_gfx = gui.gfx_id(SLOT_GFX).map(ao_gui::GfxId);
        let open_gfx = gui.gfx_id(SLOT_OPEN_GFX).map(ao_gui::GfxId);
        let w = inv.window;
        if inv.list {
            gui.remove_children(w, "content");
            let col = |text: &str, w: u32| format!("<TextView value=\"{text}\" min_size=\"Point({w},-1)\" max_size=\"Point({w},-1)\"/>");
            // one vertical view (header + a row per bag item) ending in a spacer, so the rows stay at the top
            let mut xml = format!("<root><View view_layout=\"vertical\"><View view_layout=\"horizontal\">{}{}{}{}</View>", col("", 16), col("Name", 200), col("Count", 30), col("Quality", 100));
            let mut bag: Vec<_> = zone.inventory.values().filter(|e| (items::BAG_FIRST..items::BAG_FIRST + items::BAG_SLOTS).contains(&e.slot)).copied().collect();
            bag.sort_by_key(|e| e.slot);
            let mut icons = vec![];
            for (n, e) in bag.iter().enumerate() {
                let Some(i) = self.items.info(gui, e.item.low_id) else { continue };
                xml += &format!(
                    "<View view_layout=\"horizontal\"><CanvasView name=\"icon{n}\" min_size=\"Point(16,16)\" max_size=\"Point(16,16)\"/>{}{}{}</View>",
                    col(&esc(&i.name), 200),
                    col(&i.count.to_string(), 30),
                    col(&e.item.level.to_string(), 100)
                );
                icons.push((n, i.icon));
            }
            xml += "<VLayoutSpacer/></View></root>";
            match gui.add_view_xml(w, "content", "List", &xml) {
                Ok(_) => {
                    for (n, icon) in icons {
                        if let Some((g, gw, gh)) = icon {
                            gui.set_canvas(w, &format!("icon{n}"), vec![CanvasItem::Image { id: g, src: [0.0, 0.0, gw as f32, gh as f32], dst: [0.0, 0.0, 16.0, 16.0], alpha: 1.0 }]);
                        }
                    }
                }
                Err(e) => eprintln!("hud: inventory list: {e:#}"),
            }
        } else {
            gui.remove_children(w, "content");
            let (cols, rows) = (inv.cols, inv_grid::MAX_ITEMS.div_ceil(inv.cols));
            let gap = inv.gap;
            let (cw, ch) = (inv.view_w, 2.0 * inv_grid::BORDER + rows as f32 * (inv_grid::ICON + 1.0) + (rows - 1) as f32 * gap.1);
            let (mut cmds, mut tips) = (vec![], vec![]);
            let veil = match self.dnd.hover {
                Some((item_dnd::Place::Bag { cell }, true)) => Some(cell),
                _ => None,
            };
            for n in 0..inv_grid::MAX_ITEMS {
                let cell = (n % cols, n / cols);
                let (ox, oy) = inv_grid::cell_origin(cell.0, cell.1, gap);
                // the 54 px slot art is centred on the 48 px cell
                let (x, y) = (ox - (items::SLOT - inv_grid::ICON - 1.0) / 2.0, oy - (items::SLOT - inv_grid::ICON - 1.0) / 2.0);
                let item = inv.positions.at(cell).and_then(|s| zone.inventory.get(&s));
                // `UpdateBackgroundSurfaces` (GUI 0x10134023) picks `0x11d + (max items != 0)`, i.e. SLOT_48_OPEN (a keyed, hollow frame) for a bounded
                // container, but the retail screenshot shows every slot as the filled SLOT_48_CLOSED art, which we follow (UNRESOLVED: id numbering off by one?)
                if let Some(g) = slot_gfx.or(open_gfx) {
                    cmds.push(CanvasItem::Image { id: g, src: [0.0, 0.0, items::SLOT, items::SLOT], dst: [x, y, x + items::SLOT, y + items::SLOT], alpha: 1.0 });
                }
                if let Some(e) = item {
                    cmds.extend(self.items.picture(gui, e, x, y));
                    if let Some(info) = self.items.info(gui, e.item.low_id) {
                        tips.push(CanvasTip { rect: [x, y, x + items::SLOT, y + items::SLOT], title: info.name.clone(), body: String::new() });
                    }
                }
                if veil == Some(cell) {
                    cmds.push(CanvasItem::Solid { dst: [x, y, x + items::SLOT, y + items::SLOT], color: 0xFFFFFF, alpha: 0.3 });
                }
            }
            let xml = format!("<root><CanvasView name=\"grid\" min_size=\"Point({0},{1})\" max_size=\"Point({0},{1})\"/></root>", cw as u32, ch as u32);
            match gui.add_view_xml(w, "content", "Grid", &xml) {
                Ok(_) => {
                    gui.set_canvas(w, "grid", cmds);
                    gui.set_canvas_tips(w, "grid", tips);
                }
                Err(e) => eprintln!("hud: inventory grid: {e:#}"),
            }
        }
        gui.relayout_window(w);
        inv.drawn = sig;
    }

    // ------------------------------------------------------------------------------------------------------------ dispatch

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, _dt: f32) {
        self.dnd.clock += _dt;
        self.update_skills(gui, zone);
        self.update_wear(gui, zone);
        self.update_inventory(gui, zone);
        if let Some(s) = self.stat.as_mut() {
            s.update(gui, zone);
        }
    }

    /// `true` when the event belonged to one of these windows.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        if self.skills_event(gui, ev, zone) {
            return true;
        }
        match ev {
            // the wear and stat windows are rollup pages: their close button is the page header's (`Rollup::event`)
            Event::CloseRequested { window } if self.inventory.as_ref().is_some_and(|i| i.window == *window) => {
                self.close_inventory(gui);
                self.closed.push(WindowKind::Inventory);
                true
            }
            Event::Clicked { window, view, .. } if self.wear.as_ref().is_some_and(|t| t.window == *window) => {
                if let Some(i) = WEAR_TABS.iter().position(|t| view == &format!("tab_{}", t.0)) {
                    self.select_wear_tab(gui, i);
                }
                true
            }
            Event::CanvasClick { window, view, x, y } if (view == "items" && self.wear.as_ref().is_some_and(|t| t.window == *window)) || (view == "grid" && self.inventory.as_ref().is_some_and(|i| i.window == *window)) => {
                let place = if view == "items" { self.wear.as_ref().and_then(|t| self.wear_cell_at(t.tab, *x, *y)) } else { self.bag_cell_at(*x, *y) };
                if let Some(p) = place {
                    self.item_click(zone, p);
                }
                true
            }
            _ => false,
        }
    }
}

/// Integer form of the drop target (and whether it would be accepted) for the repaint signatures of the item layers.
fn hover_code(h: Option<(item_dnd::Place, bool)>) -> i32 {
    match h {
        None => 0,
        Some((item_dnd::Place::Wear { slot, .. }, ok)) => 1 + slot as i32 * 2 + i32::from(ok),
        Some((item_dnd::Place::Bag { cell }, ok)) => 10_000 + (cell.0 * 64 + cell.1) as i32 * 2 + i32::from(ok),
    }
}

/// (slot, low_id) of every inventory item, ascending: what the item layers depend on.
fn signature(zone: &Zone) -> Vec<(u32, i32)> {
    let mut v: Vec<_> = zone.inventory.values().map(|e| (e.slot, e.item.low_id)).collect();
    v.sort_unstable();
    v
}
#[cfg(test)]
mod tests {
    use super::*;
    use ao_gui::{DrawList, InputEvent};
    use ao_net::frame::Frame;
    use ao_net::n3::world::{AcgItem, InventoryEntry};
    use ao_net::msg::Identity;
    use ao_render::{Frontend, Host, Offscreen};

    struct Shot {
        gui: Gui,
        hud: HudStats,
        zone: Zone,
        rollup: Rollup,
    }

    impl Frontend for Shot {
        fn gui(&self) -> &Gui {
            &self.gui
        }
        fn input(&mut self, ev: InputEvent, _host: &mut Host) {
            self.hud.input(&mut self.gui, &self.zone, &ev);
            self.rollup.input(&mut self.gui, &ev);
            for e in self.gui.input(ev) {
                self.rollup.event(&mut self.gui, &e);
                self.hud.event(&mut self.gui, &e, &self.zone);
            }
        }
        fn frame(&mut self, dt: f32, _size: (u32, u32), _host: &mut Host) -> DrawList {
            self.hud.update(&mut self.gui, &self.zone, dt);
            self.gui.frame(dt)
        }
    }

    const SIZE: (u32, u32) = (1100, 760);

    fn shot() -> Option<(Shot, Offscreen)> {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return None;
        }
        let labels = TextDb::load(&dir).unwrap();
        let mut gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        gui.warnings.clear();
        let mut zone = Zone::new(33512);
        for l in include_str!("../../../../docs/captures/zone_newchar_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir == "<" {
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                if let Some((f, _)) = Frame::decode_with(&b, false).unwrap() {
                    zone.on_frame(&f);
                }
            }
        }
        let hud = HudStats::new(&dir, SIZE).unwrap();
        let rollup = Rollup::new(&dir, SIZE);
        let shot = Shot { gui, hud, zone, rollup };
        let off = Offscreen::new(&shot, SIZE).unwrap();
        Some((shot, off))
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

    /// An own `StatIIR_t` frame: `key | {0xC350, id} | pass_on 0 | n | (stat, value)*` (docs/zone/dynel.md §3).
    fn stat_frame(id: u32, pairs: &[(u32, i32)]) -> Frame {
        let mut p = vec![];
        for v in [0x2B333D6Eu32, 0xC350, id] {
            p.extend(v.to_be_bytes());
        }
        p.push(0);
        p.extend((pairs.len() as u32).to_be_bytes());
        for (s, v) in pairs {
            p.extend(s.to_be_bytes());
            p.extend(v.to_be_bytes());
        }
        Frame { seq: 0, ptype: ao_net::frame::PT_N3, sender: id, receiver: 0, payload: p }
    }

    fn click(s: &mut Shot, o: &mut Offscreen, window: WindowId, view: &str) {
        let r = s.gui.view_rect(window, view).unwrap_or_else(|| panic!("no view {view}"));
        let (x, y) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        for ev in [InputEvent::MouseMove { x, y }, InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left }, InputEvent::MouseUp { x, y, button: ao_gui::MouseButton::Left }] {
            s.input(ev, &mut Host::headless());
        }
        o.frame(s, 0.016);
    }


    /// Left button press + release at a point.
    fn click_at(s: &mut Shot, o: &mut Offscreen, x: f32, y: f32) {
        for ev in [InputEvent::MouseMove { x, y }, InputEvent::MouseDown { x, y, button: ao_gui::MouseButton::Left }, InputEvent::MouseUp { x, y, button: ao_gui::MouseButton::Left }] {
            s.input(ev, &mut Host::headless());
        }
        o.frame(s, 0.016);
    }

    /// Clicks the left (`inc` = false) / right half of the arrow canvas of a skill row (`FUN_100fe0ca` / `FUN_100fe261`).
    fn arrow(s: &mut Shot, o: &mut Offscreen, stat: u32, inc: bool) {
        let w = s.hud.skills.as_ref().unwrap().window;
        let r = s.gui.view_rect(w, &format!("btn{stat}")).unwrap_or_else(|| panic!("no btn{stat}"));
        let (a, b) = (s.hud.skills.as_ref().unwrap().arrows[0].1 as f32, s.hud.skills.as_ref().unwrap().arrows[2].1 as f32);
        click_at(s, o, r.l + if inc { a + b / 2.0 } else { a / 2.0 }, (r.t + r.b) / 2.0);
    }

    fn row_value(s: &Shot, stat: u32) -> String {
        let r = s.hud.skills.as_ref().unwrap().rows.iter().flatten().find(|r| r.stat == stat).unwrap();
        s.gui.text_in(r.h, "value")
    }

    #[test]
    fn skills_window_shows_stats_and_follows_stat_deltas() {
        let Some((mut s, mut o)) = shot() else { return };
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Skills);
        let w = s.hud.skills.as_ref().unwrap().window;
        o.frame(&mut s, 0.016);
        assert!(s.gui.warnings.is_empty(), "{:?}", s.gui.warnings);
        assert_eq!(s.gui.text(w, "remaining_ip"), "1500");
        png(&mut s, &mut o, "skills-0-initial");
        // open "Abilities": its rows show the six abilities from the FullCharacter stats
        click(&mut s, &mut o, w, "abilities");
        assert_eq!(row_value(&s, 16), "6");
        png(&mut s, &mut o, "skills-1-abilities");
        // live delta: the server raises Strength to 15 and spends 10 IP
        s.zone.on_frame(&stat_frame(33512, &[(16, 15), (53, 1490)]));
        o.frame(&mut s, 0.016);
        assert_eq!(row_value(&s, 16), "15");
        assert_eq!(s.gui.text(w, "remaining_ip"), "1490");
        // another dynel's StatIIR must not change our values
        s.zone.on_frame(&stat_frame(777, &[(16, 99)]));
        o.frame(&mut s, 0.016);
        assert_eq!(row_value(&s, 16), "15");
        png(&mut s, &mut o, "skills-2-after-delta");
        // the Nano group and a skill
        click(&mut s, &mut o, w, "nanocast");
        png(&mut s, &mut o, "skills-3-nano");
        s.hud.close(&mut s.gui, &mut s.rollup, WindowKind::Skills);
        assert!(!s.hud.is_open(WindowKind::Skills));
    }

    /// The arrow buttons move pending points (price, maximum, value, bar), "Save Changes" sends `SkillIIR_t`, the reset buttons ask first.
    #[test]
    fn skills_pending_points_save_and_reset() {
        let Some((mut s, mut o)) = shot() else { return };
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Skills);
        let w = s.hud.skills.as_ref().unwrap().window;
        o.frame(&mut s, 0.016);
        click(&mut s, &mut o, w, "abilities");
        // Strength 6 -> 7 -> 8: `GetSkillCost(16, 6)` = ability cost class * 6, the IP drop by `trunc(cost)` per point
        let get = |id: u32| s.zone.stat(id);
        let ch = stats::skills::Character::from_stats(get);
        let (p6, p7) = (s.hud.model.tables.cost(16, 6, &ch) as i32, s.hud.model.tables.cost(16, 7, &ch) as i32);
        assert!(p6 > 0 && p7 > p6, "{p6} {p7}");
        arrow(&mut s, &mut o, 16, true);
        arrow(&mut s, &mut o, 16, true);
        assert_eq!((row_value(&s, 16), s.hud.model.pending(16)), ("8".to_string(), 2));
        assert_eq!(s.gui.text(w, "remaining_ip"), (1500 - p6 - p7).to_string());
        png(&mut s, &mut o, "skills-4-pending");
        // the left half gives one back, and never below the saved value
        arrow(&mut s, &mut o, 16, false);
        assert_eq!((row_value(&s, 16), s.gui.text(w, "remaining_ip")), ("7".to_string(), (1500 - p6).to_string()));
        arrow(&mut s, &mut o, 16, false);
        arrow(&mut s, &mut o, 16, false);
        assert_eq!((row_value(&s, 16), s.gui.text(w, "remaining_ip")), ("6".to_string(), "1500".to_string()));
        // the maximum of Strength at level 1 is 9: one more than the 3 points above 6 is refused
        for _ in 0..5 {
            arrow(&mut s, &mut o, 16, true);
        }
        assert_eq!(row_value(&s, 16), "9");
        // select the row: the details panel (base / maximum / delta / unit cost / total cost)
        click(&mut s, &mut o, w, "name");
        o.frame(&mut s, 0.016);
        assert_eq!(s.gui.text(w, "pointdelta"), "<div align=\"right\">3</div>");
        png(&mut s, &mut o, "skills-5-details");
        // Save Changes: stat -> pending + raw
        assert!(s.hud.take_outbox().is_empty());
        click(&mut s, &mut o, w, "Accept");
        let out = s.hud.take_outbox();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, ao_net::n3::outgoing::skill_ip_adjust(33512, &std::collections::BTreeMap::from([(16, 9)])));
        assert_eq!(s.hud.model.pending(16), 0);
        // "Reset all skills (1)" asks for confirmation, cancel sends nothing, OK sends `N3Msg_ResetSkill(0)`
        click(&mut s, &mut o, w, "resetAll");
        let dlg = s.hud.skills.as_ref().unwrap().confirm.unwrap().0;
        png(&mut s, &mut o, "skills-6-reset-dialog");
        click(&mut s, &mut o, dlg, "reset_cancel");
        assert!(s.hud.take_outbox().is_empty() && s.hud.skills.as_ref().unwrap().confirm.is_none());
        click(&mut s, &mut o, w, "resetAll");
        let dlg = s.hud.skills.as_ref().unwrap().confirm.unwrap().0;
        click(&mut s, &mut o, dlg, "reset_ok");
        let out = s.hud.take_outbox();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, ao_net::n3::action::reset_skill(33512, 0));
    }

    /// "Suggested IP distribution" on the captured level 1 Soldier: the priority-1 skills are raised first, round robin, until the IP are used.
    #[test]
    fn suggested_ip_distribution_spends_by_priority() {
        let Some((mut s, mut o)) = shot() else { return };
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Skills);
        let w = s.hud.skills.as_ref().unwrap().window;
        o.frame(&mut s, 0.016);
        click(&mut s, &mut o, w, "suggest_ip");
        let get = |id: u32| s.zone.stat(id);
        let rem = s.hud.model.remaining(&get);
        assert!(rem > 0 && rem < 1500, "{rem}");
        // Strength / Agility / Stamina / Body Dev. are `pri 1` for a Soldier at level 1 and were raised
        for stat in [16u32, 17, 18, 152] {
            assert!(s.hud.model.pending(stat) > 0, "stat {stat}");
        }
        // every raised row stays within its maximum
        for r in s.hud.skills.as_ref().unwrap().rows.iter().flatten() {
            let row = s.hud.model.row(&get, r.stat);
            assert!(row.pending == 0 || row.raw + row.pending <= row.max, "{} {row:?}", r.stat);
        }
        png(&mut s, &mut o, "skills-7-suggested");
        eprintln!("pending: {:?}, remaining {rem}", (16..=21).map(|a| (a, s.hud.model.pending(a))).collect::<Vec<_>>());
    }

    #[test]
    fn stat_window_follows_the_own_stats() {
        let Some((mut s, mut o)) = shot() else { return };
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Stat);
        assert!(s.hud.is_open(WindowKind::Stat));
        o.frame(&mut s, 0.016);
        let w = s.hud.window(WindowKind::Stat).unwrap();
        assert!(s.gui.warnings.is_empty(), "{:?}", s.gui.warnings);
        let health = (s.zone.stat(stats::HEALTH).unwrap(), s.zone.stat(stats::LIFE).unwrap());
        assert_eq!(s.gui.text(w, "health_label"), format!("<center>{} / {}</center>", health.0, health.1));
        assert!(s.gui.text(w, "level").contains("Level:"), "{}", s.gui.text(w, "level"));
        png(&mut s, &mut o, "stat-0");
        // the server hurts us: health 50 of its maximum
        s.zone.on_frame(&stat_frame(33512, &[(stats::HEALTH, 50), (22, 77)]));
        o.frame(&mut s, 0.016);
        assert_eq!(s.gui.text(w, "health_label"), format!("<center>50 / {}</center>", health.1));
        assert!(s.gui.text(w, "value0").contains("77"), "AMS row: {}", s.gui.text(w, "value0"));
        png(&mut s, &mut o, "stat-1-hurt");
        assert!(s.hud.window(WindowKind::Stat).is_some());
        s.hud.close(&mut s.gui, &mut s.rollup, WindowKind::Stat);
        assert!(!s.hud.is_open(WindowKind::Stat));
    }

    /// An inventory entry of the rdb record `low_id` in `slot`.
    fn item(slot: u32, low_id: i32, level: i32) -> InventoryEntry {
        InventoryEntry { slot, a: 0, b: 0, id: Identity { kind: 0, instance: 0 }, item: AcgItem { low_id, high_id: low_id, level } }
    }

    /// Worn items appear in their cells of the four wear tabs, bag items in the inventory grid and list (icons from rdb 1010008).
    #[test]
    fn wear_and_inventory_show_items() {
        let Some((mut s, mut o)) = shot() else { return };
        // rdb 1000020 records with an `Icon`: the special actions of the first-login hotbar (0xc1a5 Start Combat, 0xc1a2 Walk, 0xc1a8 Sit)
        s.zone.inventory.insert(6, item(6, 0xc1a5, 1)); // Right Hand
        s.zone.inventory.insert(0x11, item(0x11, 0xc1a2, 5)); // Neck
        s.zone.inventory.insert(0x2d, item(0x2d, 0xc1a8, 9)); // Feet implant
        s.zone.inventory.insert(0x31, item(0x31, 0xc1a5, 1)); // social Neck
        s.zone.inventory.insert(0x40, item(0x40, 0xc1a2, 3));
        s.zone.inventory.insert(0x41, item(0x41, 0xc1a8, 7));
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Character);
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Inventory);
        let w = s.hud.window(WindowKind::Character).unwrap();
        o.frame(&mut s, 0.016);
        assert!(s.gui.warnings.is_empty(), "{:?}", s.gui.warnings);
        // weapons tab: Right Hand = slot 6 is cell 6 (column 0, row 2)
        assert_eq!(s.gui.canvas_items(w, "items").len(), 1);
        png(&mut s, &mut o, "wear-0-weapons-items");
        click(&mut s, &mut o, w, "tab_clothes");
        assert_eq!(s.gui.canvas_items(w, "items").len(), 1);
        png(&mut s, &mut o, "wear-1-clothes-items");
        click(&mut s, &mut o, w, "tab_implants");
        assert_eq!(s.gui.canvas_items(w, "items").len(), 1);
        png(&mut s, &mut o, "wear-2-implants-items");
        click(&mut s, &mut o, w, "tab_social");
        assert_eq!((s.hud.wear.as_ref().unwrap().tab, s.gui.canvas_items(w, "items").len()), (3, 1));
        png(&mut s, &mut o, "wear-3-social-items");
        // inventory grid: 30 slot frames + the 2 pictures
        let iw = s.hud.window(WindowKind::Inventory).unwrap();
        assert_eq!(s.gui.canvas_items(iw, "grid").len(), 32);
        png(&mut s, &mut o, "inventory-grid");
        s.hud.set_inventory_list(true);
        o.frame(&mut s, 0.016);
        assert!(s.gui.has_view(iw, "icon1") && !s.gui.has_view(iw, "icon2") && !s.gui.has_view(iw, "grid"));
        png(&mut s, &mut o, "inventory-list");
        // a new item arrives in the live zone: the layers redraw
        s.zone.inventory.insert(0x42, item(0x42, 0xc1a5, 2));
        o.frame(&mut s, 0.016);
        assert!(s.gui.has_view(iw, "icon2"));
        // moving the item out of the bag updates it
        s.zone.inventory.remove(&0x40);
        s.hud.set_inventory_list(false);
        o.frame(&mut s, 0.016);
        assert_eq!(s.gui.canvas_items(iw, "grid").len(), 32);
    }

    /// Press, drag over the windows, release.
    fn drag(s: &mut Shot, o: &mut Offscreen, from: (f32, f32), to: (f32, f32)) {
        let mid = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
        for ev in [
            InputEvent::MouseMove { x: from.0, y: from.1 },
            InputEvent::MouseDown { x: from.0, y: from.1, button: ao_gui::MouseButton::Left },
            InputEvent::MouseMove { x: mid.0, y: mid.1 },
            InputEvent::MouseMove { x: to.0, y: to.1 },
        ] {
            s.input(ev, &mut Host::headless());
            o.frame(s, 0.016);
        }
        s.input(InputEvent::MouseUp { x: to.0, y: to.1, button: ao_gui::MouseButton::Left }, &mut Host::headless());
        o.frame(s, 0.016);
    }

    /// `drag` with both end points computed from the shot first (they borrow it).
    macro_rules! dnd {
        ($s:ident, $o:ident, $from:expr, $to:expr) => {{
            let (f, t) = ($from, $to);
            drag(&mut $s, &mut $o, f, t)
        }};
    }

    /// Screen centre of a bag grid cell / a wear cell.
    fn bag_xy(s: &Shot, cell: (usize, usize)) -> (f32, f32) {
        let iw = s.hud.window(WindowKind::Inventory).unwrap();
        let (r, i) = (s.gui.view_rect(iw, "grid").unwrap(), s.hud.inventory.as_ref().unwrap());
        let (ox, oy) = inv_grid::cell_origin(cell.0, cell.1, i.gap);
        (r.l + ox + 24.0, r.t + oy + 24.0)
    }
    fn wear_xy(s: &Shot, tab: usize, id: u32) -> (f32, f32) {
        let w = s.hud.window(WindowKind::Character).unwrap();
        let r = s.gui.view_rect(w, "items").unwrap();
        let cell = (0..15).find(|&c| items::wear_slot(tab, c).is_some_and(|sl| sl % 16 == id)).unwrap();
        let (c, row) = items::wear_cell(tab, cell);
        let (ox, oy) = items::wear_origin(c, row);
        (r.l + ox + 27.0, r.t + oy + 27.0)
    }

    /// Drag and drop with real item templates (rdb 1000020: 21797 "Augmented Nano Armor Cloak" = class 2, `Placement` bit 3 (Back), `DefaultPos` 3;
    /// 31837 "Floating Torch" = class 1): equip, refuse, unequip, double click, drop on the ground; the server's answers move the items.
    #[test]
    fn items_move_by_drag_and_drop() {
        use ao_net::n3::inventory as inv;
        let Some((mut s, mut o)) = shot() else { return };
        if ao_rdb::RecordStore::open(&ao_gui::client_dir()).is_err() {
            return;
        }
        let me = s.zone.char_id as i32;
        let mine = Identity { kind: 0xC350, instance: me };
        s.zone.inventory.insert(0x40, item(0x40, 21797, 1));
        s.zone.inventory.insert(0x41, item(0x41, 31837, 1));
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Character);
        s.hud.open(&mut s.gui, &mut s.rollup, WindowKind::Inventory);
        let w = s.hud.window(WindowKind::Character).unwrap();
        o.frame(&mut s, 0.016);
        click(&mut s, &mut o, w, "tab_clothes");
        o.frame(&mut s, 0.016);
        let payload = |s: &mut Shot| s.hud.take_outbox().into_iter().map(|f| f.payload).collect::<Vec<_>>();

        // the cloak onto the Back slot of the clothes tab: a `ClientMoveItemToInventoryIIR_t` to slot 0x10 + 3, nothing changes locally
        dnd!(s, o, bag_xy(&s, (0, 0)), wear_xy(&s, 1, 3));
        assert_eq!(payload(&mut s), [inv::move_item_to_inventory(me, inv::item_identity(0x40), 0x13)]);
        assert!(s.zone.inventory.contains_key(&0x40) && !s.zone.inventory.contains_key(&0x13));
        // the torch (class 1) is refused by the clothes tab, and by a slot it does not fit
        dnd!(s, o, bag_xy(&s, (1, 0)), wear_xy(&s, 1, 3));
        assert!(payload(&mut s).is_empty());
        // the server answers: the cloak is worn
        s.zone.apply_inventory(&inv::InventoryMsg::ContainerAdd { item: inv::item_identity(0x40), container: mine, slot: 0x13 });
        o.frame(&mut s, 0.016);
        assert_eq!((s.gui.canvas_items(w, "items").len(), s.hud.inventory.as_ref().unwrap().positions.get(0x40)), (1, None));
        png(&mut s, &mut o, "dnd-worn");
        // the worn cloak onto the bag cell (2, 1): unequip to "any free slot", the item appears in the cell it was dropped on
        dnd!(s, o, wear_xy(&s, 1, 3), bag_xy(&s, (2, 1)));
        assert_eq!(payload(&mut s), [inv::move_item_to_inventory(me, inv::item_identity(0x13), inv::ANY_BAG_SLOT)]);
        s.zone.apply_inventory(&inv::InventoryMsg::ContainerAdd { item: inv::item_identity(0x13), container: mine, slot: inv::ANY_BAG_SLOT });
        o.frame(&mut s, 0.016);
        assert_eq!(s.hud.inventory.as_ref().unwrap().positions.get(0x40), Some((2, 1)));
        // inside the bag only the client side cell changes: no frame (the default window shows rows 0 and 1; row 2 is scrolled out of the viewport)
        dnd!(s, o, bag_xy(&s, (2, 1)), bag_xy(&s, (0, 1)));
        assert!(payload(&mut s).is_empty());
        assert_eq!(s.hud.inventory.as_ref().unwrap().positions.get(0x40), Some((0, 1)));
        // a double click wears it at its `DefaultPos`
        let p = bag_xy(&s, (0, 1));
        click_at(&mut s, &mut o, p.0, p.1);
        click_at(&mut s, &mut o, p.0, p.1);
        assert_eq!(payload(&mut s), [inv::move_item_to_inventory(me, inv::item_identity(0x40), 0x13)]);
        // released over the world: `DropTemplateIIR_t` at the player's position
        dnd!(s, o, bag_xy(&s, (1, 0)), (20.0, 300.0));
        assert_eq!(payload(&mut s), [inv::drop_item(me, inv::item_identity(0x41), s.zone.own().map_or([0.0; 3], |d| d.pos))]);
    }

    /// The real `Hud`: the menu toggles open the windows, the frame close button (and `Close`) clears the menu state again; the NewChar template's
    /// windows (wear, stat) are open from the start; Ctrl+9 toggles the stat window.
    #[test]
    fn hud_opens_and_closes_the_windows() {
        let Some((mut s, mut o)) = shot() else { return };
        let mut hud = crate::play::hud::Hud::new(&mut s.gui, &ao_gui::client_dir(), SIZE).unwrap();
        assert!(hud.is_open(WindowKind::Character) && hud.is_open(WindowKind::Stat) && !hud.is_open(WindowKind::Skills));
        for k in [WindowKind::Skills, WindowKind::Inventory] {
            hud.toggle(&mut s.gui, k);
            assert!(hud.is_open(k) && !s.hud.is_open(k), "{k:?}");
        }
        hud.update(&mut s.gui, &mut s.zone, 0.016);
        o.frame(&mut s, 0.016);
        let skills = hud.stats_window(WindowKind::Skills).unwrap();
        assert_eq!(s.gui.text(skills, "remaining_ip"), "1500");
        // frame close button → CloseRequested → the menu state is cleared
        assert!(hud.event(&mut s.gui, &Event::CloseRequested { window: skills }, &s.zone));
        assert!(!hud.is_open(WindowKind::Skills) && hud.is_open(WindowKind::Inventory));
        hud.toggle(&mut s.gui, WindowKind::Inventory);
        assert!(!hud.is_open(WindowKind::Inventory));
        let stat = hud.stats_window(WindowKind::Stat).unwrap();
        // the stat window is a frameless rollup page: its close button is the page header's canvas `close`
        assert!(hud.event(&mut s.gui, &Event::CanvasClick { window: stat, view: "close".into(), x: 5.0, y: 5.0 }, &s.zone));
        assert!(!hud.is_open(WindowKind::Stat) && hud.stats_window(WindowKind::Stat).is_none());
        hud.toggle(&mut s.gui, WindowKind::Stat);
        assert!(hud.is_open(WindowKind::Stat));
        hud.close(&mut s.gui);
    }

    /// Real rdb numbers: pending points never exceed the maximum, each point costs `trunc(GetSkillCost)`, no profession-specific panic.
    #[test]
    fn model_on_real_tables() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let mut h = HudStats::new(&dir, SIZE).unwrap();
        let stats_map: std::collections::HashMap<u32, i32> = [(stats::IP, 1500), (stats::LEVEL, 1), (stats::PROFESSION, 1), (stats::BREED, 1), (37, 1), (16, 6), (17, 6), (18, 6), (19, 6), (20, 6), (21, 6), (152, 5)].into();
        let get = |id: u32| stats_map.get(&id).copied();
        // Body Development (152) of a Soldier at level 1: max, unit cost and the price of three points as the client sums them
        let r = h.model.row(&get, 152);
        assert_eq!((r.raw, r.pending), (5, 0));
        assert!(r.max > 5, "{r:?}");
        let ch = stats::skills::Character::from_stats(get);
        let unit = h.model.tables.cost(152, 5, &ch);
        let a = h.model.adjust(&get, 152, 3);
        assert_eq!(a.applied, 3);
        let expect: i32 = (0..3).map(|i| h.model.tables.cost(152, 5 + i, &ch) as i32).sum();
        assert_eq!(h.model.remaining(&get), 1500 - expect);
        eprintln!("Body Dev.: raw 5 max {} unit {unit} 3 points {expect} IP, remaining {}", r.max, h.model.remaining(&get));
        // a 40-point bulk increase is cut at the maximum by the caller (`can_increase`), but the IP slot alone cuts it at the IP left
        h.model.clear();
        let a = h.model.adjust(&get, 152, 400);
        assert!(a.applied > 0 && a.applied < 400 && h.model.remaining(&get) > 0, "{a:?} {}", h.model.remaining(&get));
    }
}
