//! The character information window `stat_window` (`StatView_c`, ctor `FUN_1007ff6e` GUI 0x1007ff6e, title "Stats"): name / level header, health,
//! nano, XP, alien XP and PvP score bars and the nine ability / armour-class rows. Evidence: docs/gui.md §11.9.
//!
//! Layout (all from the ctor): root `VLayout` = header `HLayout` [name text (borders 3), flexible spacer, level text (borders 0,3,3,0)], the bars
//! (borders 3,2,3,0; `PowerbarView_c(bg 0xdb, full, dir right)` with a centred "cur / max" label), then an `HLayout` of two `VLayout`s
//! (labels left-aligned, borders 3,0,0,0 | spacer | values right-aligned, borders 0,0,3,0).

use super::super::hud_rollup::Rollup;
use super::super::zone::Zone;
use ao_formats::{screens::TextDb, stats};
use ao_gui::{Gui, WindowId, WindowSize};
use std::collections::HashMap;

/// The nine rows, in the order of the ctor's `GetText(0x7d3, stat)` pushes: 22 `AMS`, then 91 / 90 / 92 / 96 / 93 / 94 / 95 / 97 (Melee, Projectile,
/// Energy, Poison, Chemical, Radiation, Cold, Fire armour classes). The values are `N3Msg_GetSkill(stat, 2)` (handlers `FUN_1007fa7a`, `FUN_1007fad8`).
/// Dock identity of the view (`DockableViewDockName`, `docked_view_identities` of `RollupArea.xml`).
pub(super) const KEY: &str = "stat_window";

pub(super) const ROWS: [u32; 9] = [22, 91, 90, 92, 96, 93, 94, 95, 97];

/// One power bar: view name, fill art (`GFX_GUI_HOR_BAR_*`, ids 0xdd 0xda 0xde 0xdc 0xdf), tooltip key in text.mdb 10000.
struct Bar {
    name: &'static str,
    full: &'static str,
    tip: &'static str,
}

/// Creation order of the ctor: health 0xdd, nano 0xda, XP 0xde, alien XP 0xdc, then the three PvP bars (0xdf) of stats 0x2ac, 0x2aa, 0x2ab
/// (684 `PVPDuelScore`, 682 `PVPSoloScore`, 683 `PVPTeamScore`; `FUN_1007fe8d`).
const BARS: [Bar; 7] = [
    Bar { name: "health", full: "GFX_GUI_HOR_BAR_RED", tip: "TooltipHealth" },
    Bar { name: "nano", full: "GFX_GUI_HOR_BAR_BLUE", tip: "TooltipNano" },
    Bar { name: "xp", full: "GFX_GUI_HOR_BAR_YELLOW", tip: "TooltipExperience" },
    Bar { name: "alienxp", full: "GFX_GUI_HOR_BAR_GREEN", tip: "TooltipAlienExperience" },
    Bar { name: "duel", full: "GFX_GUI_HOR_BAR_PURPLE", tip: "TooltipPVPDuelScore" },
    Bar { name: "solo", full: "GFX_GUI_HOR_BAR_PURPLE", tip: "TooltipPVPSoloScore" },
    Bar { name: "team", full: "GFX_GUI_HOR_BAR_PURPLE", tip: "TooltipPVPTeamScore" },
];
const PVP_STATS: [u32; 3] = [684, 682, 683];

/// Archive/menu order of `FUN_10082714`, not the bar creation order.
const FIELDS: [&str; 5] = ["AIXPBar", "XPBar", "DuelScoreBar", "TeamScoreBar", "SoloScoreBar"];
const LABELS: [&str; 5] = ["AI XP Bar", "XP Bar", "Duel Score Bar", "Team Score Bar", "Solo Score Bar"];
const MENU_ID: u32 = 0x5354_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Config(pub(super) [bool; 5]);

impl Default for Config {
    fn default() -> Self {
        // Retail-screenshot-derived four-bar count; Solo/Team identities are inferred:
        // all three zero-score PvP bars have the same 0 / 2000 label and art.
        // The original reads uninitialized flags when the default archive is empty
        // (GUI 0x1007ff6e; Utils::FindBool 0x10008fe6 leaves missing outputs untouched).
        Self([false, false, false, true, true])
    }
}

impl Config {
    pub(super) fn load(d: &super::super::dvalue::DValues) -> Self {
        let mut config = Self::default();
        if let Some(super::super::dvalue::Variant::Archive(src)) = d.get("StatViewConfig") {
            if let Ok(e) = ao_gui::xml::parse(src) {
                for (i, key) in FIELDS.iter().enumerate() {
                    if let Some(v) = super::find_bool(&e, key) { config.0[i] = v; }
                }
            }
        }
        config
    }

    fn save(self, d: &mut super::super::dvalue::DValues) {
        use super::super::dvalue::{element_xml, Variant};
        let mut e = match d.get("StatViewConfig") {
            Some(Variant::Archive(src)) => ao_gui::xml::parse(src).ok(),
            _ => None,
        }.unwrap_or_else(|| ao_gui::xml::Element { name: "Archive".into(), attrs: vec![("name".into(), "StatViewConfig".into()), ("code".into(), "0".into())], children: vec![] });
        for (key, value) in FIELDS.iter().zip(self.0) {
            let child = ao_gui::xml::Element { name: "Bool".into(), attrs: vec![("name".into(), (*key).into()), ("value".into(), value.to_string())], children: vec![] };
            if let Some(old) = e.children.iter_mut().find(|c| c.attr("name") == Some(*key)) { *old = child; }
            else { e.children.push(child); }
        }
        d.set("StatViewConfig", Variant::Archive(element_xml(&e)));
    }
}

/// `Expansion` (stat 389 = `GetSkill(0x185)`): the alien XP bar is only created when `Expansion & 0x18` is non-zero (ctor).
const EXPANSION: u32 = 389;
const XP: u32 = 52;
const LAST_XP: u32 = 57;
const NEXT_XP: u32 = 350;
const ALIEN_XP: u32 = 40;
const ALIEN_XP_NEXT: u32 = 178;
const NANO: u32 = 214;
const MAX_NANO: u32 = 221;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub(super) struct StatView {
    pub(super) window: WindowId,
    shown: HashMap<String, String>,
    fills: HashMap<&'static str, f32>,
    /// A text changed since the last re-fit of the window.
    dirty: bool,
    config: Config,
    menu: bool,
    config_source: Option<String>,
}

impl StatView {
    /// Docks the view in the rollup column (`StatViewConfig` `DockableViewDockName = "RollupArea"`, page `stat_window` of `RollupArea.xml`); tab title "Stats".
    pub(super) fn open(gui: &mut Gui, db: &TextDb, rollup: &mut Rollup) -> anyhow::Result<Self> {
        // The stacked rollup body assigns its full frame to its child. A vertical
        // wrapper lets the capped owner retain its natural height while the spacer
        // absorbs any excess saved page height.
        let mut xml = String::from("<root><View view_layout=\"vertical\"><View view_layout=\"vertical\" name=\"StatView\">");
        xml += "<View view_layout=\"horizontal\"><TextView name=\"name\" layout_borders=\"Rect(3,3,3,3)\"/><HLayoutSpacer name=\"spacer\"/>";
        xml += "<TextView name=\"level\" layout_borders=\"Rect(0,3,3,0)\"/></View>";
        // a stacked view has no layout node and so no preferred size of its own: it is the size of the bar art
        let (bw, bh) = gui.gfx_id("GFX_GUI_HOR_BAR_EMPTY").map(ao_gui::GfxId).map_or((100, 12), |g| gui.gfx().size(g));
        let (bw, bh) = (bw - 1, bh - 1);
        for b in &BARS {
            xml += &format!(
                "<View view_layout=\"stacked\" name=\"{n}_box\" layout_borders=\"Rect(3,2,3,0)\" min_size=\"Point({bw},{bh})\" max_size=\"Point({bw},{bh})\"><PowerBar name=\"{n}\" bg_gfx=\"GFX_GUI_HOR_BAR_EMPTY\" full_gfx=\"{}\" direction=\"right\"/>\
                 <View view_layout=\"horizontal\" h_alignment=\"center\" v_alignment=\"center\"><TextView name=\"{n}_label\"/></View></View>",
                b.full,
                n = b.name
            );
        }
        xml += "<View view_layout=\"horizontal\"><View view_layout=\"vertical\" h_alignment=\"left\">";
        for (i, &s) in ROWS.iter().enumerate() {
            xml += &format!("<TextView name=\"label{i}\" value=\"{}\" layout_borders=\"Rect(3,0,0,0)\"/>", esc(&format!("<FONT color=#ffffff>{}:</font>", stats::short_name(db, s))));
        }
        xml += "</View><HLayoutSpacer/><View view_layout=\"vertical\" h_alignment=\"right\">";
        for i in 0..ROWS.len() {
            xml += &format!("<TextView name=\"value{i}\" layout_borders=\"Rect(0,0,3,0)\"/>");
        }
        xml += "</View></View></View><VLayoutSpacer/></View></root>";
        let window = rollup.open_page(gui, KEY, "Stats", &xml, 279.0)?;
        for b in &BARS {
            if let Some(t) = db.by_key(10000, b.tip) {
                gui.set_tooltip(window, b.name, &t, "");
            }
        }
        Ok(Self { window, shown: HashMap::new(), fills: HashMap::new(), dirty: true, config: Config::default(), menu: false, config_source: None })
    }

    pub(super) fn configure(&mut self, d: &super::super::dvalue::DValues) {
        let source = match d.get("StatViewConfig") {
            Some(super::super::dvalue::Variant::Archive(src)) => Some(src.as_str()),
            _ => None,
        };
        if self.config_source.as_deref() != source {
            self.config = Config::load(d);
            self.config_source = source.map(str::to_owned);
        }
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &ao_gui::Event, zone: &Zone, d: &mut super::super::dvalue::DValues, screen: (u32, u32)) -> bool {
        match ev {
            ao_gui::Event::ContextMenu { window, x, y, .. } | ao_gui::Event::FrameIcon { window, x, y } if *window == self.window => {
                self.configure(d);
                let items = LABELS.iter().enumerate().filter(|(i, _)| *i != 0 || zone.skill_value(EXPANSION).unwrap_or(0) & 0x18 != 0)
                    .map(|(i, label)| ao_gui::MenuItem::check(MENU_ID | i as u32, label, self.config.0[i])).collect();
                gui.open_menu((*x, *y), (screen.0 as i32, screen.1 as i32), items);
                self.menu = true;
                true
            }
            ao_gui::Event::MenuPicked { id } if self.menu && *id & 0xffff_0000 == MENU_ID => {
                self.menu = false;
                if let Some(value) = self.config.0.get_mut((*id & 0xffff) as usize) {
                    *value = !*value;
                    self.config.save(d);
                    self.update(gui, zone);
                }
                true
            }
            _ => false,
        }
    }

    fn text(&mut self, gui: &mut Gui, name: &str, text: String) {
        if self.shown.get(name) != Some(&text) {
            gui.set_text(self.window, name, &text);
            self.shown.insert(name.to_string(), text);
            self.dirty = true;
        }
    }

    /// `FUN_1007f8c4(max, cur)`: label `"%u / %u"` and the bar value `cur / max` (unsigned conversion of negative ints, capped at 1).
    fn bar(&mut self, gui: &mut Gui, b: &'static Bar, cur: i32, max: i32) {
        let (c, m) = (cur as u32, max as u32);
        self.text(gui, &format!("{}_label", b.name), format!("<center>{c} / {m}</center>"));
        let f = c as f32 / m as f32;
        let f = if f.is_nan() { 0.0 } else { f.min(1.0) };
        if self.fills.get(b.name) != Some(&f) {
            gui.set_progress(self.window, b.name, f);
            self.fills.insert(b.name, f);
        }
    }

    /// The handlers of the ctor's `GetCharStatSignal` connections, run once per frame over the own stats.
    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone) {
        let st = |id: u32| zone.skill_value(id).unwrap_or(0);
        let name = zone.own().map(|d| d.name.as_str()).unwrap_or_default();
        // `FUN_1007f99b` / `FUN_1007fa1c`
        self.text(gui, "name", format!("<FONT color=#ffffff>Name:</font> <FONT color=#bbbbff>{}</font>", esc(name)));
        self.text(gui, "level", format!("<FONT color=#ffffff>Level:</font> <FONT color=#bbbbff>{}</font>", st(stats::LEVEL)));
        for (i, &s) in ROWS.iter().enumerate() {
            self.text(gui, &format!("value{i}"), format!("<FONT color=#bbbbff>{}</font>", st(s)));
        }
        self.bar(gui, &BARS[0], st(stats::HEALTH), st(stats::LIFE)); // FUN_1007fd0a
        self.bar(gui, &BARS[1], st(NANO), st(MAX_NANO)); // FUN_1007fd42
        // FUN_1007fd80: below level 200 the XP bar is XP - LastXP of NextXP - LastXP; from 200 the shadow-level pair 0x23d / 0x240
        let (next, mut last, level) = (st(NEXT_XP), st(LAST_XP), st(stats::LEVEL));
        let cur = if level < 200 {
            st(XP)
        } else {
            if st(0x240) & 1 == 0 {
                last = 0;
            }
            st(0x23d)
        };
        self.bar(gui, &BARS[2], cur - last, next - last);
        self.bar(gui, &BARS[3], st(ALIEN_XP), st(ALIEN_XP_NEXT)); // FUN_1007fe52
        for (b, stat) in BARS[4..].iter().zip(PVP_STATS) {
            // FUN_1007fe8d: max = score of the next rank, cur = the score stat
            let v = st(stat);
            self.bar(gui, b, v, stats::pvp_score_for_rank(stats::pvp_rank(v) + 1));
        }
        let [aixp, xp, duel, team, solo] = self.config.0;
        let show = [true, true, xp, aixp && st(EXPANSION) & 0x18 != 0, duel, solo, team];
        let mut changed = false;
        for (b, v) in BARS.iter().zip(show) {
            let key = format!("{}_shown", b.name);
            if self.shown.get(&key).map(String::as_str) != Some(if v { "1" } else { "0" }) {
                gui.show_collapsing(self.window, &format!("{}_box", b.name), v);
                self.shown.insert(key, if v { "1" } else { "0" }.into());
                changed = true;
            }
        }
        // Fit the owner, not the saved-height rollup wrapper: unused page height
        // must not stretch the Stats rows (`FUN_1007f7f8` content refit).
        if changed || self.dirty {
            gui.fit_view_height(self.window, "StatView");
            gui.resize_window(self.window, WindowSize::Preferred);
            self.dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::dvalue::{DValues, Variant, CAT_CHAR};

    #[test]
    fn stat_view_archive_roundtrip_preserves_independent_flags_and_dock_fields() {
        let mut d = DValues::default();
        d.load_config("<Root><Archive name=\"StatViewConfig\"><Bool name=\"XPBar\" value=\"true\"/><Bool name=\"TeamScoreBar\" value=\"false\"/><String name=\"DockableViewDockName\" value=\"RollupArea\"/></Archive></Root>", CAT_CHAR, true);
        let prefs = std::env::temp_dir().join(format!("aomac-stat-view-config-{}", std::process::id()));
        d.open_user(&prefs, "test", 7);
        assert_eq!(Config::load(&d).0, [false, true, false, false, true]);
        let config = Config([true, false, true, false, true]);
        config.save(&mut d);
        assert_eq!(Config::load(&d), config);
        let Some(Variant::Archive(src)) = d.get("StatViewConfig") else { panic!("missing archive") };
        assert!(src.contains("DockableViewDockName") && src.contains("RollupArea"));
        let mut restored = DValues::default();
        restored.load_config("<Root><Archive name=\"StatViewConfig\"/></Root>", CAT_CHAR, true);
        restored.load_config(&d.save_config(CAT_CHAR), CAT_CHAR, false);
        assert_eq!(Config::load(&restored), config);
        d.save_user();
        restored.open_user(&prefs, "test", 7);
        assert_eq!(Config::load(&restored), config);
        std::fs::remove_dir_all(prefs).unwrap();
    }
}
