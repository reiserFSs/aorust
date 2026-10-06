//! `faction_window` (`FactionView_c`, GUI.dll `FUN_1005dc02`, `FactionBar_c` `FUN_1005d94e` / refresh `FUN_1005d68c`): the Faction window, menu entry
//! `faction_window` of `CharMenu.xml` (criteria `stat:level>=15`). Evidence, addresses and what is unresolved: docs/gui.md §11.13.

use super::zone::Zone;
use ao_gui::{Event, Gui, WindowId, WindowSize};
use ao_rdb::RecordStore;
use std::path::Path;

/// The seven faction stats of the window (`FUN_1005dc02`: bars for `0x236 .. 0x23c`).
pub(super) const STATS: [u32; 7] = [0x236, 0x237, 0x238, 0x239, 0x23a, 0x23b, 0x23c];
/// `GetFactionStr(stat, false)`: the short names (map at Gamecode 0x102e2690 built by `FUN_10035c99`), one per stat of [`STATS`].
pub(super) const SHORT_NAMES: [&str; 7] = ["Guardian", "Followers", "Operators", "Unredeemed", "Devoted", "Conservers", "Redeemed"];
/// The faction titles by band (`GetFactionTitle`: `FUN_10036bbe(FUN_100c1ed0(value))`, map 0x102e26a0 of `FUN_10035c99`), bands 1..=12.
pub(super) const TITLES: [&str; 12] =
    ["Dreaded", "Hated", "Enemy", "Wanted", "Disliked", "Indifferent", "Neutral", "Liked", "Ally", "Friend", "Close friend", "Family"];
/// Bar art (`FUN_1005d68c`): background `0xdb`; fill `0xdc` (value >= 0, direction right), `0xdd` (value < 0, direction left); stat `0x236` always `0xde` (right).
const BG: u32 = 0xdb;
/// `GetFactionRange` fallback when no band holds the value (`local_20 = -50000`, `local_28 = 50000`).
const DEFAULT_RANGE: (i32, i32) = (-50000, 50000);
/// The type of the faction list resource (`FUN_100c63bd`: `GetIdentityVec(0xf4265)`), records `1..=12` in band order.
const FACTION_LIST: u32 = 1_000_037;

/// One band of the faction list: `[min, max]` (record: `i32 min, i32 max, string, string, ...`, little endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Band {
    pub min: i32,
    pub max: i32,
}

/// The bands of rdb 1000037, ordered by `min` (the std::map the client walks in key order, `FUN_100c1ed0`).
pub(super) fn load_bands(dir: &Path) -> Vec<Band> {
    let Ok(store) = RecordStore::open(dir) else { return vec![] };
    let mut v: Vec<Band> = store
        .ids(FACTION_LIST)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| store.get(FACTION_LIST, id).ok().flatten())
        .filter_map(|r| Some(Band { min: i32::from_le_bytes(r.get(0..4)?.try_into().ok()?), max: i32::from_le_bytes(r.get(4..8)?.try_into().ok()?) }))
        .collect();
    v.sort_by_key(|b| b.min);
    v
}

/// `FUN_100c1ed0`: 1-based index of the first band holding `value`, -1 when none does.
pub(super) fn band_index(bands: &[Band], value: i32) -> i32 {
    bands.iter().position(|b| b.min <= value && value <= b.max).map_or(-1, |i| i as i32 + 1)
}

/// `N3Msg_GetFactionRange`: the band's `(min, max)` or the `-50000 .. 50000` default.
pub(super) fn range(bands: &[Band], value: i32) -> (i32, i32) {
    bands.iter().find(|b| b.min <= value && value <= b.max).map_or(DEFAULT_RANGE, |b| (b.min, b.max))
}

/// The bar's fill fraction (`FUN_1005d68c`): `(value - min) / (max - min)`, mirrored (`1 - x`) for negative values.
pub(super) fn fraction(bands: &[Band], value: i32) -> f32 {
    let (min, max) = range(bands, value);
    let f = (value - min) as f32 / (max - min) as f32;
    if value < 0 { 1.0 - f } else { f }
}

/// The text on the right of a bar: the title in `<font color=#ffff88>` or the number (pref `IsFactionTitleShown`).
pub(super) fn value_text(bands: &[Band], value: i32, title_shown: bool) -> String {
    if title_shown {
        let t = usize::try_from(band_index(bands, value) - 1).ok().and_then(|i| TITLES.get(i)).copied().unwrap_or("Error");
        format!("<font color=#ffff88>{t}</font>")
    } else {
        format!("<font color=#ffff88>{value}</font>")
    }
}

struct Row {
    /// The value the bar was built for (`+0x12c`), `None` before the first refresh. A sign change rebuilds the bar (`FUN_1005d68c`).
    value: Option<i32>,
    /// Whether the right label showed the title when it was drawn.
    titled: bool,
}

pub(super) struct HudFaction {
    window: Option<(WindowId, Vec<Row>)>,
    bands: Vec<Band>,
    /// `IsFactionTitleShown` (`GetPrefInt` default 1, `indep.rs`); clicking a bar toggles it (`FUN_1005d647`).
    title_shown: bool,
    screen: (u32, u32),
    closed: bool,
}

impl HudFaction {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        Self { window: None, bands: load_bands(dir), title_shown: true, screen, closed: false }
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
    }

    pub(super) fn take_closed(&mut self) -> bool {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn open(&mut self, gui: &mut Gui) {
        if self.window.is_some() {
            return;
        }
        match self.build(gui) {
            Ok(w) => self.window = Some(w),
            Err(e) => eprintln!("hud: faction_window: {e:#}"),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui) {
        if let Some((w, _)) = self.window.take() {
            gui.close_window(w);
        }
    }

    /// `FUN_1005dc02`: seven `FactionBar_c`s in a vertical layout, borders `(2,10,2,1)` for the first, `(2,1,2,1)` and `(2,1,2,2)` for the last.
    /// `FactionWindowConfig` has no saved frame in the template and the window is not docked: the place is UNRESOLVED GUESS (screen centre).
    fn build(&mut self, gui: &mut Gui) -> anyhow::Result<(WindowId, Vec<Row>)> {
        let xml = "<root><View view_layout=\"vertical\" name=\"faction_root\"/></root>";
        let w = gui.open_tabbed_window_xml("FactionView", "Faction", xml, (0, 0), WindowSize::Preferred)?;
        // a stacked view has no layout node, so the bar's cell carries the size of its background art (`GFX_GUI_HOR_BAR_EMPTY`)
        let bg = gui.gfx().size(ao_gui::GfxId(BG));
        let mut rows = vec![];
        for (i, name) in SHORT_NAMES.iter().enumerate() {
            let b = match i {
                0 => "Rect(2,10,2,1)",
                6 => "Rect(2,1,2,2)",
                _ => "Rect(2,1,2,1)",
            };
            let src = format!("<root><View view_layout=\"horizontal\" name=\"factionbar{i}\" layout_borders=\"{b}\"><View name=\"bar{i}\" view_layout=\"stacked\" min_size=\"Point({},{})\"/></View></root>", bg.0 - 1, bg.1 - 1);
            gui.add_view_xml(w, "faction_root", name, &src)?;
            rows.push(Row { value: None, titled: self.title_shown });
        }
        Ok((w, rows))
    }

    /// `FUN_1005d68c` for every bar whose stat changed.
    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some((w, rows)) = &mut self.window else { return };
        let first = rows.iter().any(|r| r.value.is_none());
        let mut rebuilt = false;
        for (i, row) in rows.iter_mut().enumerate() {
            let value = zone.skill_value(STATS[i]).unwrap_or(0);
            let fill = if i == 0 { 0xde } else if value < 0 { 0xdd } else { 0xdc };
            let dir = if i != 0 && value < 0 { "left" } else { "right" };
            let sign_changed = row.value.is_none_or(|v| (v < 0) != (value < 0));
            if sign_changed {
                rebuilt = true;
                // the bar is rebuilt with the other fill art / direction (`View::RemoveChild`, new `PowerbarView_c`)
                gui.remove_children(*w, &format!("bar{i}"));
                let name = |id: u32| gui.gfx().name(ao_gui::GfxId(id)).unwrap_or_default().to_string();
                let src = format!(
                    "<root><PowerBar name=\"fill{i}\" bg_gfx=\"{}\" full_gfx=\"{}\" direction=\"{dir}\"/></root>",
                    name(BG),
                    name(fill)
                );
                let lab = format!(
                    "<root><View view_layout=\"horizontal\"><TextView name=\"left{i}\" value=\"{}\" layout_borders=\"Rect(3,0,3,0)\"/><HLayoutSpacer/><TextView name=\"right{i}\" value=\"\" layout_borders=\"Rect(3,0,3,0)\"/></View></root>",
                    SHORT_NAMES[i]
                );
                // the click target of `FUN_1005d647` (a mouse event on the bar toggles the title display)
                let hit = format!("<root><CanvasView name=\"click{i}\"/></root>");
                for s in [src, lab, hit] {
                    if let Err(e) = gui.add_view_xml(*w, &format!("bar{i}"), "FactionBar", &s) {
                        eprintln!("hud: faction bar {i}: {e:#}");
                    }
                }
            }
            if sign_changed || row.value != Some(value) || row.titled != self.title_shown {
                gui.set_text(*w, &format!("right{i}"), &value_text(&self.bands, value, self.title_shown));
                gui.set_progress(*w, &format!("fill{i}"), fraction(&self.bands, value));
            }
            row.value = Some(value);
            row.titled = self.title_shown;
        }
        if rebuilt {
            // the window is sized by its content, which exists only now; the first time it is also centred
            gui.resize_window(*w, WindowSize::Preferred);
            if first {
                let (ow, oh) = gui.outer_size(*w);
                gui.set_window_pos(*w, ((self.screen.0 as i32 - ow as i32) / 2, (self.screen.1 as i32 - oh as i32) / 2));
            }
        }
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event) -> bool {
        let Some((w, _)) = &self.window else { return false };
        match ev {
            Event::CloseRequested { window } if window == w => {
                self.close(gui);
                self.closed = true;
                true
            }
            // `FUN_1005d647`: the bar toggles `IsFactionTitleShown`
            Event::CanvasClick { window, view, .. } if window == w && view.starts_with("click") => {
                self.title_shown = !self.title_shown;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands() -> Vec<Band> {
        vec![Band { min: -50000, max: -30001 }, Band { min: -1000, max: -1 }, Band { min: 0, max: 999 }, Band { min: 1000, max: 4999 }]
    }

    /// The window is sized by its seven bars once they exist (a stacked cell has no layout node: it was a 14 x 62 sliver) and is centred.
    #[test]
    fn window_holds_seven_bars_and_is_centred() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut f = HudFaction::new(&dir, (1280, 800));
        f.open(&mut gui);
        f.update(&mut gui, &Zone::default());
        let (w, _) = f.window.as_ref().unwrap();
        let (x, y, ow, oh) = gui.window_outer_frame(*w).unwrap();
        assert!(ow >= 179 && oh >= 7 * 23, "{ow}x{oh}");
        assert_eq!((x, y), ((1280 - ow as i32) / 2, (800 - oh as i32) / 2));
    }

    #[test]
    fn bands_and_fractions() {
        let b = bands();
        assert_eq!(band_index(&b, -40000), 1);
        assert_eq!(band_index(&b, 500), 3);
        assert_eq!(band_index(&b, 7000), -1);
        assert_eq!(range(&b, 7000), DEFAULT_RANGE);
        assert!((fraction(&b, 500) - 500.0 / 999.0).abs() < 1e-6);
        // negative values are mirrored
        assert!((fraction(&b, -1000) - 1.0).abs() < 1e-6);
        assert_eq!(value_text(&b, 500, false), "<font color=#ffff88>500</font>");
        assert_eq!(value_text(&b, 500, true), "<font color=#ffff88>Enemy</font>"); // third band of this 4-band fixture; the real list puts 0 in band 7 (Neutral)
    }

    #[test]
    fn real_bands_match_the_titles() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let b = load_bands(&dir);
        assert_eq!(b.len(), 12);
        assert_eq!((b[0].min, b[11].max), (-50000, 50000));
        assert_eq!(band_index(&b, 0), 7);
    }
}
