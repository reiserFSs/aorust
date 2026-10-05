//! Skills, inventory and wear ("character / equipment") windows of the in-world interface. Evidence: docs/gui.md §11.
//!
//! * Skills: the client's own `Views/Skills.xml` (`SkillWindow` ctor `FUN_100fc18e` GUI 0x100fc18e); the per-group stat rows are
//!   built in code by the original (`StatRow` ctor `FUN_100fe956`) and by us as view XML with the same widgets.
//! * Wear: `WearView_c` (`FUN_100e1bc9`): a tab view of three item grids drawn over `GFX_GUI_WEARVIEW_{WEAPON,CLOTHING,IMPLANTS}`.
//! * Inventory: `InventoryView_c` (`FUN_100cc2ca`): a `MultiListView` slot grid, 30 slots for the character's own inventory.

use super::hud::WindowKind;
use super::zone::Zone;
use ao_formats::{screens::TextDb, stats};
use ao_gui::{Event, Gui, ViewHandle, WindowId, WindowSize};

/// `Window(Rect(200,180)..(850,700))` of `SkillWindow` (`FUN_100fc18e`: floats 0x101a959c = 200, 0x101a95a0 = 180, 0x101c10b0 = 850,
/// 0x101a95a8 = 700). Rects are inclusive: 651 x 521 outer.
const SKILLS_POS: (i32, i32) = (200, 180);
const SKILLS_OUTER: (u32, u32) = (651, 521);
/// `GFX_GUI_WEARVIEW_*` art size; the three tab headers are read off the art (x ranges below).
const WEAR_ART: (u32, u32) = (192, 320);
/// Tab header x ranges inside the art (measured at row y = 3: separators at x = 72 and 122, outer edges 4 and 188), height 17.
const WEAR_TABS: [(&str, &str, i32, i32); 3] = [
    ("weapons", "GFX_GUI_WEARVIEW_WEAPON", 4, 72),
    ("clothes", "GFX_GUI_WEARVIEW_CLOTHING", 72, 122),
    ("implants", "GFX_GUI_WEARVIEW_IMPLANTS", 122, 188),
];
/// `GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED` is 54 x 54; `MultiListView_c::SetGridIconSpacing(11, 9)` (`FUN_100e1bc9`, floats 0x101bee04 / 0x101bfc88).
const SLOT_GFX: &str = "GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED";
const SLOT_SPACING: (i32, i32) = (11, 9);
/// `MultiListView_c::SetMaxItemCount(0x1e)` for the character's own inventory (`ItemContainerView_c` ctor `FUN_100cdb3a`).
const INVENTORY_SLOTS: usize = 30;
/// UNRESOLVED GUESS: grid columns of the inventory (the wear grids are 3 x 5; the inventory's `SetViewCellCounts` was not traced).
const INVENTORY_COLS: usize = 5;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

struct Skills {
    window: WindowId,
    /// group index -> rows (stat, view instance, last shown value)
    rows: Vec<Vec<(u32, ViewHandle, i32)>>,
    group: Option<usize>,
    selected: Option<u32>,
    ip: i32,
    reset: i32,
    /// The first [`HudStats::update`] after opening applies the stat dependent visibility.
    init: bool,
}

struct Tabbed {
    window: WindowId,
    tab: usize,
}

pub(super) struct HudStats {
    db: TextDb,
    skills: Option<Skills>,
    wear: Option<Tabbed>,
    inventory: Option<WindowId>,
    /// Windows closed by their frame button since the last [`HudStats::take_closed`].
    closed: Vec<WindowKind>,
}

impl HudStats {
    pub(super) fn new(dir: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self { db: TextDb::load(dir)?, skills: None, wear: None, inventory: None, closed: vec![] })
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        matches!(kind, WindowKind::Skills | WindowKind::Inventory | WindowKind::Character)
    }

    pub(super) fn is_open(&self, kind: WindowKind) -> bool {
        match kind {
            WindowKind::Skills => self.skills.is_some(),
            WindowKind::Inventory => self.inventory.is_some(),
            WindowKind::Character => self.wear.is_some(),
            _ => false,
        }
    }

    /// Kinds whose frame close button was pressed (the caller clears their menu state).
    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn open(&mut self, gui: &mut Gui, kind: WindowKind) {
        if self.is_open(kind) {
            return;
        }
        let r = match kind {
            WindowKind::Skills => self.open_skills(gui),
            WindowKind::Inventory => self.open_inventory(gui),
            WindowKind::Character => self.open_wear(gui),
            _ => return,
        };
        if let Err(e) = r {
            eprintln!("hud: opening {kind:?}: {e:#}");
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, kind: WindowKind) {
        match kind {
            WindowKind::Skills => self.skills.take().map(|s| gui.close_window(s.window)),
            WindowKind::Inventory => self.inventory.take().map(|w| gui.close_window(w)),
            WindowKind::Character => self.wear.take().map(|w| gui.close_window(w.window)),
            _ => None,
        };
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui) {
        for k in [WindowKind::Skills, WindowKind::Inventory, WindowKind::Character] {
            self.close(gui, k);
        }
    }

    // ------------------------------------------------------------------------------------------------------------ skills

    fn open_skills(&mut self, gui: &mut Gui) -> anyhow::Result<()> {
        // The original window is `Window(Rect(200,180,850,700), style 0, flags 0x1000)` with one tab "Skills"; the style-0 frame (3,7,3,3 + tab
        // strip) is not reproduced: UNRESOLVED, the style-1 frame (docs/gui.md §6) of the same outer size is used.
        let client = (SKILLS_OUTER.0 - 6, SKILLS_OUTER.1 - 27);
        let w = gui.open_framed_window("Skills", SKILLS_POS, WindowSize::Fixed(client.0, client.1))?;
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
        let mut rows = vec![];
        for g in &stats::SKILL_GROUPS {
            let mut r = vec![];
            for &s in g.stats {
                let v = 0;
                let xml = format!(
                    "<root><View view_layout=\"horizontal\">\
                     <TextButton name=\"name\" text=\"{}\" color=\"0xFFFFFF\" hover_color=\"TEXT_HOVER\" pressed_color=\"TEXT_SELECTED\" layout_borders=\"Rect(3,0,0,0)\"/>\
                     <HLayoutSpacer/><TextView name=\"value\" value=\"{v}\" layout_borders=\"Rect(0,0,3,0)\"/></View></root>",
                    esc(&stats::short_name(&self.db, s as u32))
                );
                let h = gui.add_view_xml(w, &format!("{}_group", g.prefix), "StatRow", &xml)?;
                r.push((s as u32, h, i32::MIN));
            }
            rows.push(r);
        }
        // word-wrapped text sizes itself from its current frame (`TextRenderer_c::CalculatePreferredSize`): a second pass uses the frames of the first
        gui.relayout_window(w);
        self.skills = Some(Skills { window: w, rows, group: None, selected: None, ip: i32::MIN, reset: i32::MIN, init: false });
        Ok(())
    }

    fn skills_event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let Some(s) = self.skills.as_mut() else { return false };
        match ev {
            Event::CloseRequested { window } if *window == s.window => {
                self.close(gui, WindowKind::Skills);
                self.closed.push(WindowKind::Skills);
                true
            }
            Event::Clicked { window, view, item } if *window == s.window => {
                if let Some(g) = stats::SKILL_GROUPS.iter().position(|g| g.prefix == view) {
                    // `FUN_100f9552` (not minimised): the info panel shows this group's rows.
                    s.group = Some(g);
                    gui.select_child(s.window, "infoselect", Some(1));
                    gui.select_child(s.window, "groupselect", Some(g));
                } else if view == "name" {
                    let stat = item.and_then(|h| s.rows.iter().flatten().find(|r| r.1 == h)).map(|r| r.0);
                    if let Some(stat) = stat {
                        s.selected = Some(stat);
                        let v = zone.stat(stat).unwrap_or(0).to_string();
                        gui.set_text(s.window, "statName", &stats::long_name(&self.db, stat));
                        gui.set_text(s.window, "statdesc", &stats::description(&self.db, stat).unwrap_or_default());
                        gui.set_text(s.window, "base", &v);
                        // UNRESOLVED: the buffed value is `N3Msg_GetSkill(stat, 0)` (stat modifiers, SimpleChar+0x1bc, not decoded) -> equal to base.
                        gui.set_text(s.window, "buffed", &v);
                        gui.show_collapsing(s.window, "statdetails", true);
                    }
                } else if view == "Quit" {
                    self.close(gui, WindowKind::Skills);
                    self.closed.push(WindowKind::Skills);
                } else {
                    return false;
                }
                true
            }
            _ => false,
        }
    }

    fn update_skills(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(s) = self.skills.as_mut() else { return };
        if !s.init {
            s.init = true;
            // "Suggested IP distribution" is available until level 20 (inittext).
            gui.set_enabled(s.window, "suggest_ip", zone.stat(stats::LEVEL).unwrap_or(0) < 20);
        }
        // `FUN_100fa45c`: "Reset all skills (n)", n = (stat 0x15c bit 2 clear) + FullIPRPoints (0x2b3); enabled when n >= 1
        let reset = i32::from(zone.stat(0x15c).unwrap_or(0) & 4 == 0) + zone.stat(0x2b3).unwrap_or(0);
        if reset != s.reset {
            s.reset = reset;
            let label = format!("{} ({reset})", self.db.by_id(502, 96620988).unwrap_or_default());
            gui.set_text(s.window, "resetAll", &label);
            gui.set_enabled(s.window, "resetAll", reset >= 1);
        }
        let ip = zone.stat(stats::IP).unwrap_or(0);
        if ip != s.ip {
            s.ip = ip;
            gui.set_text(s.window, "remaining_ip", &ip.to_string());
        }
        for (stat, h, last) in s.rows.iter_mut().flatten() {
            let v = zone.stat(*stat).unwrap_or(0);
            if v != *last {
                *last = v;
                gui.set_text_in(*h, "value", &v.to_string());
            }
            if Some(*stat) == s.selected {
                let v = v.to_string();
                gui.set_text(s.window, "base", &v);
                gui.set_text(s.window, "buffed", &v);
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------------- wear / inventory

    fn open_wear(&mut self, gui: &mut Gui) -> anyhow::Result<()> {
        let mut xml = String::from("<root><View view_layout=\"vertical\"><View view_layout=\"stacked\" layout_borders=\"Rect(5,5,5,5)\"");
        xml += &format!(" min_size=\"Point({},{})\" max_size=\"Point({},{})\">", WEAR_ART.0, WEAR_ART.1, WEAR_ART.0, WEAR_ART.1);
        for (name, gfx, ..) in WEAR_TABS {
            xml += &format!("<BitmapView name=\"{name}\" bitmap_id=\"{gfx}\"/>");
        }
        // tab header hit areas laid over the art
        xml += "<View view_layout=\"horizontal\" h_alignment=\"left\" v_alignment=\"top\">";
        xml += &format!("<View min_size=\"Point({},17)\" max_size=\"Point({},17)\"/>", WEAR_TABS[0].2, WEAR_TABS[0].2);
        for (name, _, l, r) in WEAR_TABS {
            xml += &format!("<TextButton name=\"tab_{name}\" text=\"\" min_size=\"Point({},17)\" max_size=\"Point({},17)\"/>", r - l, r - l);
        }
        xml += "<HLayoutSpacer/></View></View></View></root>";
        let w = gui.open_framed_window_xml("WearView", &xml, (30, 150), WindowSize::Preferred)?;
        self.wear = Some(Tabbed { window: w, tab: 0 });
        self.select_wear_tab(gui, 0);
        Ok(())
    }

    fn select_wear_tab(&mut self, gui: &mut Gui, tab: usize) {
        let Some(t) = self.wear.as_mut() else { return };
        t.tab = tab;
        for (i, (name, ..)) in WEAR_TABS.iter().enumerate() {
            gui.set_visible(t.window, name, i == tab);
        }
    }

    fn open_inventory(&mut self, gui: &mut Gui) -> anyhow::Result<()> {
        let mut xml = String::from("<root><View view_layout=\"vertical\" layout_borders=\"Rect(5,5,5,5)\">");
        let rows = INVENTORY_SLOTS.div_ceil(INVENTORY_COLS);
        for r in 0..rows {
            xml += "<View view_layout=\"horizontal\" h_alignment=\"left\">";
            for c in 0..INVENTORY_COLS {
                let last = (c + 1 == INVENTORY_COLS, r + 1 == rows);
                xml += &format!(
                    "<BitmapView bitmap_id=\"{SLOT_GFX}\" layout_borders=\"Rect(0,0,{},{})\"/>",
                    if last.0 { 0 } else { SLOT_SPACING.0 },
                    if last.1 { 0 } else { SLOT_SPACING.1 }
                );
            }
            xml += "</View>";
        }
        xml += "</View></root>";
        self.inventory = Some(gui.open_framed_window_xml("InventoryView", &xml, (300, 150), WindowSize::Preferred)?);
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------------------ dispatch

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, _dt: f32) {
        self.update_skills(gui, zone);
    }

    /// `true` when the event belonged to one of these windows.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        if self.skills_event(gui, ev, zone) {
            return true;
        }
        match ev {
            Event::CloseRequested { window } if self.wear.as_ref().is_some_and(|t| t.window == *window) => {
                self.close(gui, WindowKind::Character);
                self.closed.push(WindowKind::Character);
                true
            }
            Event::CloseRequested { window } if self.inventory == Some(*window) => {
                self.close(gui, WindowKind::Inventory);
                self.closed.push(WindowKind::Inventory);
                true
            }
            Event::Clicked { window, view, .. } if self.wear.as_ref().is_some_and(|t| t.window == *window) => {
                if let Some(i) = WEAR_TABS.iter().position(|t| view == &format!("tab_{}", t.0)) {
                    self.select_wear_tab(gui, i);
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_gui::{DrawList, InputEvent};
    use ao_net::frame::Frame;
    use ao_render::{Frontend, Host, Offscreen};

    struct Shot {
        gui: Gui,
        hud: HudStats,
        zone: Zone,
    }

    impl Frontend for Shot {
        fn gui(&self) -> &Gui {
            &self.gui
        }
        fn input(&mut self, ev: InputEvent, _host: &mut Host) {
            for e in self.gui.input(ev) {
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
        let hud = HudStats::new(&dir).unwrap();
        let shot = Shot { gui, hud, zone };
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

    #[test]
    fn skills_window_shows_stats_and_follows_stat_deltas() {
        let Some((mut s, mut o)) = shot() else { return };
        s.hud.open(&mut s.gui, WindowKind::Skills);
        let w = s.hud.skills.as_ref().unwrap().window;
        o.frame(&mut s, 0.016);
        assert!(s.gui.warnings.is_empty(), "{:?}", s.gui.warnings);
        assert_eq!(s.gui.text(w, "remaining_ip"), "1500");
        png(&mut s, &mut o, "skills-0-initial");
        // open "Abilities": its rows show the six abilities from the FullCharacter stats
        click(&mut s, &mut o, w, "abilities");
        let strength = s.hud.skills.as_ref().unwrap().rows[0][0];
        assert_eq!((strength.0, s.gui.text_in(strength.1, "value")), (16, "6".to_string()));
        png(&mut s, &mut o, "skills-1-abilities");
        // live delta: the server raises Strength to 15 and spends 10 IP
        s.zone.on_frame(&stat_frame(33512, &[(16, 15), (53, 1490)]));
        o.frame(&mut s, 0.016);
        assert_eq!(s.gui.text_in(strength.1, "value"), "15");
        assert_eq!(s.gui.text(w, "remaining_ip"), "1490");
        // another dynel's StatIIR must not change our values
        s.zone.on_frame(&stat_frame(777, &[(16, 99)]));
        o.frame(&mut s, 0.016);
        assert_eq!(s.gui.text_in(strength.1, "value"), "15");
        png(&mut s, &mut o, "skills-2-after-delta");
        // the Nano group and a skill
        click(&mut s, &mut o, w, "nanocast");
        png(&mut s, &mut o, "skills-3-nano");
        s.hud.close(&mut s.gui, WindowKind::Skills);
        assert!(!s.hud.is_open(WindowKind::Skills));
    }

    #[test]
    fn wear_and_inventory_windows_open_empty() {
        let Some((mut s, mut o)) = shot() else { return };
        s.hud.open(&mut s.gui, WindowKind::Character);
        s.hud.open(&mut s.gui, WindowKind::Inventory);
        assert!(s.hud.is_open(WindowKind::Character) && s.hud.is_open(WindowKind::Inventory));
        png(&mut s, &mut o, "inventory-wear-both");
        assert!(s.gui.warnings.is_empty(), "{:?}", s.gui.warnings);
        let w = s.hud.wear.as_ref().unwrap().window;
        png(&mut s, &mut o, "wear-0-weapons");
        click(&mut s, &mut o, w, "tab_clothes");
        assert_eq!(s.hud.wear.as_ref().unwrap().tab, 1);
        png(&mut s, &mut o, "wear-1-clothes");
        click(&mut s, &mut o, w, "tab_implants");
        assert_eq!(s.hud.wear.as_ref().unwrap().tab, 2);
        png(&mut s, &mut o, "wear-2-implants");
    }
}
