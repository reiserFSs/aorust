//! The character information window `stat_window` (`StatView_c`, ctor `FUN_1007ff6e` GUI 0x1007ff6e, title "Stats"): name / level header, health,
//! nano, XP, alien XP and PvP score bars and the nine ability / armour-class rows. Evidence: docs/gui.md §11.9.
//!
//! Layout (all from the ctor): root `VLayout` = header `HLayout` [name text (borders 3), flexible spacer, level text (borders 0,3,3,0)], the bars
//! (borders 3,2,3,0; `PowerbarView_c(bg 0xdb, full, dir right)` with a centred "cur / max" label), then an `HLayout` of two `VLayout`s
//! (labels left-aligned, borders 3,0,0,0 | spacer | values right-aligned, borders 0,0,3,0).

use super::super::zone::Zone;
use ao_formats::{screens::TextDb, stats};
use ao_gui::{Gui, WindowId, WindowSize};
use std::collections::HashMap;

/// The nine rows, in the order of the ctor's `GetText(0x7d3, stat)` pushes: 22 `AMS`, then 91 / 90 / 92 / 96 / 93 / 94 / 95 / 97 (Melee, Projectile,
/// Energy, Poison, Chemical, Radiation, Cold, Fire armour classes). The values are `N3Msg_GetSkill(stat, 2)` (handlers `FUN_1007fa7a`, `FUN_1007fad8`).
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

/// `StatViewConfig` booleans of the NewChar template (`AIXPBar`, `XPBar`, `DuelScoreBar`, `TeamScoreBar`, `SoloScoreBar`, all true; the ctor reads
/// them from the `StatViewConfig` dvalue, `Message::FindBool` defaulting to false when the archive is empty).
const SHOW_XP: bool = true;
const SHOW_AIXP: bool = true;
const SHOW_PVP: bool = true;

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
}

impl StatView {
    pub(super) fn open(gui: &mut Gui, db: &TextDb, pos: (i32, i32)) -> anyhow::Result<Self> {
        let mut xml = String::from("<root><View view_layout=\"vertical\" name=\"StatView\">");
        xml += "<View view_layout=\"horizontal\"><TextView name=\"name\" layout_borders=\"Rect(3,3,3,3)\"/><HLayoutSpacer name=\"spacer\"/>";
        xml += "<TextView name=\"level\" layout_borders=\"Rect(0,3,3,0)\"/></View>";
        // a stacked view has no layout node and so no preferred size of its own: it is the size of the bar art
        let (bw, bh) = gui.gfx_id("GFX_GUI_HOR_BAR_EMPTY").map(ao_gui::GfxId).map_or((100, 12), |g| gui.gfx().size(g));
        let (bw, bh) = (bw - 1, bh - 1);
        for b in &BARS {
            xml += &format!(
                "<View view_layout=\"stacked\" name=\"{n}_box\" layout_borders=\"Rect(3,2,3,0)\" min_size=\"Point({bw},{bh})\" max_size=\"Point({bw},{bh})\"><PowerBar name=\"{n}\" bg_gfx=\"GFX_GUI_HOR_BAR_EMPTY\" full_gfx=\"{}\" direction=\"right\"/>\
                 <TextView name=\"{n}_label\" h_alignment=\"center\" v_alignment=\"center\"/></View>",
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
        xml += "</View></View></View></root>";
        let window = gui.open_framed_window_xml("StatView", &xml, pos, WindowSize::Preferred)?;
        for b in &BARS {
            if let Some(t) = db.by_key(10000, b.tip) {
                gui.set_tooltip(window, b.name, &t, "");
            }
        }
        Ok(Self { window, shown: HashMap::new(), fills: HashMap::new(), dirty: true })
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
        let st = |id: u32| zone.stat(id).unwrap_or(0);
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
        let show = [true, true, SHOW_XP, SHOW_AIXP && st(EXPANSION) & 0x18 != 0, SHOW_PVP, SHOW_PVP, SHOW_PVP];
        let mut changed = false;
        for (b, v) in BARS.iter().zip(show) {
            let key = format!("{}_shown", b.name);
            if self.shown.get(&key).map(String::as_str) != Some(if v { "1" } else { "0" }) {
                gui.show_collapsing(self.window, &format!("{}_box", b.name), v);
                self.shown.insert(key, if v { "1" } else { "0" }.into());
                changed = true;
            }
        }
        // the window is as wide as its widest label and as high as its visible rows (it is re-fitted when a text or a bar's visibility changed)
        if changed || self.dirty {
            gui.resize_window(self.window, WindowSize::Preferred);
            self.dirty = false;
        }
    }
}
