//! Target selection and the target controls of the in-world interface (docs/gui.md §11, docs/zone.md §target).
//!
//! Original: the selection is client-local state. `TargetingModule_t::SetTarget` (GUI.dll 0x100257b0) stores the identity in
//! `InputConfig_t+0xc0` (`SetCurrentTarget` 0x10019df0), broadcasts it on the AFCM bus (`Send(0x13, 0x126, id)`; `N3InterfaceModule_t::
//! SetTargetMessage` Interfaces 0x1000907a forwards it to `n3EngineClientAnarchy_t::N3Msg_SelectedTarget`, which only points the
//! camera's `SetSelectedTarget`), creates the selection `Indicator_t` (a ground marker unless the target has skill-flag 0x400) and
//! raises the `OnSelecting*` tips. The combat consumer announces this identity with `LookAtIIR_t` (`FUN_1003fb35`, mode 1 for
//! characters, 0 otherwise; docs/zone/combat.md). `TargetingModule_t::FrameProcess` 0x10025fa4 drops the target when the dynel is gone
//! (`N3Msg_GetPos` fails or it has a parent) unless it was forced.
//!
//! The pick: the GUI hands the normalised mouse position to `N3Msg_SetMousePos` (Gamecode 0x1001613b) → `n3Camera_t::SetMousePos`
//! (N3 0x10020571): ray direction `(tan(fov/2)·x, tan(fov/2)/aspect·y, 1)` rotated by the camera, length `VisualCamera_t::
//! GetLengthOfViewcone`, cast against the dynels' bounding boxes (`hud_pick.rs`: `n3VisualDynel_t::FineCollisionCheck`); the hit
//! list's entry under `GetObjectUnderColLine` is "the object under the mouse" (`InputConfig_t::CheckObjectUnderMouse` 0x10019f00
//! decides the pointer from it, `hud_cursor.rs`).

use super::options::keys::FixedKeys;
use super::zone::{DynelState, Zone};
use ao_formats::stats;
use ao_gui::{Gui, InputEvent, MouseButton, WindowId, WindowSize};
use ao_render::{Camera, Vec3};
use ao_scene::Lens;
use ao_net::msg::Identity;
use ao_net::n3::outgoing::DYNEL_CHAR;

/// `GetLengthOfViewcone` (far − near) at the default view distance: 800 − 0.5 m (docs/formats.md, statel LOD).
const VIEW_LENGTH: f32 = 799.5;
/// Pointer path length (px) up to which a press and release on the world still is a click: `ActionViewMouseHandler_c`
/// accumulates the mouse-look movement in `+0x1c` and `FUN_1002c469` requires it `< 0.02` (GUI `_DAT_101aeaf4`) in the units of
/// `InputConfig_t::FrameProcess` (raw counts / 1000), i.e. 20 counts; counts and GUI pixels are taken as equal.
const CLICK_PATH: f32 = 20.0;

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
    /// Length of the line: `VisualCamera_t::GetLengthOfViewcone` = far − near.
    pub len: f32,
}

/// `n3Camera_t::SetMousePos` 0x10020571: the ray through pixel `mouse` of a `vp` sized viewport. `Lens::fov` is the horizontal
/// angle for the original engine (`RCamera_t` takes `tan(fov/2)` as the half width of the view plane at distance 1).
pub fn pick_ray(cam: &Camera, lens: &Lens, vp: (f32, f32), mouse: (f32, f32)) -> Ray {
    let aspect = vp.0 / vp.1;
    let (nx, ny) = (mouse.0 / vp.0 * 2.0 - 1.0, 1.0 - mouse.1 / vp.1 * 2.0);
    let half_w = if lens.horizontal { (lens.fov * 0.5).tan() } else { (lens.fov * 0.5).tan() * aspect };
    let dir = cam.forward() + cam.right() * (half_w * nx) + cam.up() * (half_w / aspect * ny);
    Ray { origin: cam.pos, dir: dir.normalize(), len: lens.far.map_or(VIEW_LENGTH, |f| f - lens.near) }
}

/// `GetObjectUnderColLine` (N3 0x1002069c): the current target when it is in the hit list, else the first entry (void without hits).
pub fn object_under<T: Copy + PartialEq>(list: &[T], current: Option<T>) -> Option<T> {
    current.filter(|c| list.contains(c)).or_else(|| list.first().copied())
}

/// What a plain left click selects: `n3Camera_t::GetNextTarget` (N3 0x10020723) on the hit list, `ActionViewMouseHandler_c`'s release
/// slot `FUN_1002c469` (GUI 0x1002c469) sends it as `SetTargetMessage`. The object after the current target in the list (wrapping)
/// or, when the current target is not in it, the first one: clicking again on a stack of overlapping dynels walks through them.
/// An empty list gives `None` (the original's reference is void and the handler does nothing: **no click on the ground deselects**).
pub fn click_target<T: Copy + PartialEq>(list: &[T], current: Option<T>) -> Option<T> {
    match current.and_then(|c| list.iter().position(|i| *i == c)) {
        Some(i) => Some(list[(i + 1) % list.len()]),
        None => list.first().copied(),
    }
}

/// What a world click did ([`HudTarget::world_click`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorldClick {
    /// The character became the target.
    Select(i32),
    /// Shift + click: the info page of the character was requested; the selection is unchanged.
    Info(i32),
    ObjectInfo(Identity),
}

/// Stat `PetMaster` (196 = 0xc4) and `TowerType` (388 = 0x184), read by `N3Msg_CanClickTargetTarget`.
const PET_MASTER: u32 = 0xc4;
const TOWER_TYPE: u32 = 0x184;

/// `N3Msg_CanClickTargetTarget(target, targetsTarget)` (Gamecode 0x10016451): both dynels must exist. A dynel with `char+0x21c` set
/// (the client character: its record has "fewer stats", docs/zone/dynel.md §2) is *plain* when it has a `PetMaster`, every other one
/// is plain; the click is refused when both are plain, otherwise allowed unless the target has a non-zero `TowerType`. Only the own
/// character's stats are known, the others' stats count as absent. [INFERENCE] for what `+0x21c` stands for.
pub fn can_click_target_target(zone: &Zone, target: i32, targets_target: i32) -> bool {
    let me = zone.char_id as i32;
    if !zone.dynels.contains_key(&target) || !zone.dynels.contains_key(&targets_target) {
        return false;
    }
    let plain = |id: i32| id != me || zone.stat(PET_MASTER).is_some();
    !(plain(target) && plain(targets_target)) && (target != me || zone.stat(TOWER_TYPE).unwrap_or(0) == 0)
}

/// What the target controls show about the selected dynel (`N3Msg_GetName` / `GetSkill` of the target).
#[derive(Clone, Debug, PartialEq)]
pub struct TargetInfo {
    pub id: i32,
    pub kind: i32,
    pub name: String,
    pub level: i32,
    /// `Health / MaxHealth`, 0..1.
    pub health: f32,
    /// Retail `N3Msg_Consider` gradient used by `TargetHealthBar_c::SetTarget` (GUI 0x1007313a).
    pub color: u32,
    pub max_health: i32,
    /// `FUN_100744ae` (GUI 0x100744ae): an NPC is attackable iff its `Side` (0x21) differs from the own one; players need
    /// `N3Msg_CanAttack` (PvP rules, not modelled: never).
    pub hostile: bool,
    pub is_self: bool,
}

/// `StripSpecialChars` + the HTML the text views would otherwise interpret.
fn clean(name: &str) -> String {
    name.chars().filter(|c| !c.is_control() && *c != '<' && *c != '>').collect()
}

pub fn info(zone: &Zone, id: i32) -> Option<TargetInfo> {
    let d = zone.dynels.get(&id)?;
    let is_self = id == zone.char_id as i32;
    let (health, max, level, side) = if is_self {
        (zone.skill_value(stats::HEALTH).unwrap_or(d.health), zone.skill_value(stats::LIFE).unwrap_or(d.max_health), zone.skill_value(stats::LEVEL).unwrap_or(d.level), d.side)
    } else {
        (d.health, d.max_health, d.level, d.side)
    };
    let own_side = zone.skill_value(stats::SIDE).map_or_else(|| zone.own().map_or(0, |o| o.side), |s| s as u8);
    let own_level = zone.skill_value(stats::LEVEL).or_else(|| zone.own().map(|o| o.level)).unwrap_or(0);
    let range = zone.skill_value(0x113).unwrap_or(ao_net::n3::nametag::INVALID_STAT);
    let [r, g, b, _] = ao_net::n3::nametag::con_color(ao_net::n3::nametag::consider_ratio(level, own_level, range));
    Some(TargetInfo {
        id,
        kind: DYNEL_CHAR,
        name: clean(&d.name),
        level,
        health: if max > 0 { (health as f32 / max as f32).clamp(0.0, 1.0) } else { 0.0 },
        color: u32::from_be_bytes([0, r, g, b]),
        max_health: max,
        hostile: d.npc && !is_self && side != own_side,
        is_self,
    })
}

fn selected_info(zone: &Zone) -> Option<TargetInfo> {
    let id = zone.selected_target()?;
    if id.kind == DYNEL_CHAR {
        return info(zone, id.instance);
    }
    let max = zone.world.stat_of(id.kind, id.instance, stats::LIFE).unwrap_or(0);
    let health = zone.world.stat_of(id.kind, id.instance, stats::HEALTH).unwrap_or(0);
    Some(TargetInfo {
        id: id.instance,
        kind: id.kind,
        name: clean(zone.world.name_of(id.kind, id.instance).unwrap_or_default()),
        level: zone.world.stat_of(id.kind, id.instance, stats::LEVEL).unwrap_or(0),
        health: if max > 0 { (health as f32 / max as f32).clamp(0.0, 1.0) } else { 0.0 },
        color: 0xffffff,
        max_health: max,
        hostile: false,
        is_self: false,
    })
}

/// Radius of the `N3Msg_GetCloseTarget` candidate query (`(**(playfield+0x58)+0x4c)(ownPos, 100.0, …)`, `_DAT_10155eb0`).
const CLOSE_TARGET_RADIUS: f32 = 100.0;
/// `_DAT_10157870` (1000.0): added to a negative / tied distance delta so that the farther candidates come first.
const CLOSE_TARGET_WRAP: f32 = 1000.0;

/// `N3Msg_GetCloseTarget(current, friendly, forward)` (Gamecode 0x1001c411, called by `TargetingModule_t::Get{Next,Prev}{Friendly,
/// Hostile}TargetMessage` 0x10025a52…, bound to Tab and the dock arrows). Candidates: dynels within 100 m of the own character
/// except it and the current target; *hostile* = a `Side` (stat 0x21) different from the own one, *friendly* = the same (players
/// of the own team and pets of the own team count as friendly: team state not modelled; the `InPlay` stat 0xc2 test is skipped).
/// Ordering: `m = d² − d²(current)` (0 without a current target), `+1000` when negative, and `+1000` when 0 for a candidate listed
/// before the current one; `forward` takes the smallest `m`, backward the largest (starting at `f32::MIN_POSITIVE`, so `m = 0`
/// never wins backwards). The original's list order (the locality query's) is unknown: candidates are listed by id.
pub fn cycle(zone: &Zone, hostile: bool, forward: bool) -> Option<i32> {
    let me = zone.own()?;
    let my_id = zone.char_id as i32;
    let own_side = zone.stat(stats::SIDE).map_or(me.side, |s| s as u8);
    let d2 = |d: &DynelState| Vec3::from(d.pos).distance_squared(Vec3::from(me.pos));
    let cur = zone.target.filter(|t| *t != my_id && zone.dynels.contains_key(t));
    let base = cur.map_or(0.0, |t| d2(&zone.dynels[&t]));
    let mut ids: Vec<i32> = zone.dynels.keys().copied().collect();
    ids.sort_unstable();
    let cur_at = cur.and_then(|t| ids.iter().position(|i| *i == t)).unwrap_or(0);
    let mut best = (None, if forward { f32::MAX } else { f32::MIN_POSITIVE });
    for (n, id) in ids.iter().enumerate() {
        let d = &zone.dynels[id];
        if *id == my_id || Some(*id) == cur || d2(d) > CLOSE_TARGET_RADIUS * CLOSE_TARGET_RADIUS || (d.side != own_side) != hostile {
            continue;
        }
        let mut m = d2(d) - base;
        if m < 0.0 || (m == 0.0 && n < cur_at) {
            m += CLOSE_TARGET_WRAP;
        }
        if if forward { m < best.1 } else { best.1 < m } {
            best = (Some(*id), m);
        }
    }
    best.0
}

// ---------------------------------------------------------------------------------------------------------------------
// The target controls (`CCTargetControl_c`, GUI 0x100746ec)
// ---------------------------------------------------------------------------------------------------------------------

/// Gfx ids of `TargetHealthBar_c` (`FUN_10073598`): `ViewSurface_c::Load(id, flags)` 0x1a5/0xd left cap, 0x1a6/0xe right cap,
/// 0x1a4/0x1f background (colour DEFAULT), 0x1a7/0x10 slider.
const HB_BACKGROUND: u32 = 0x1a4;
const HB_LEFT: u32 = 0x1a5;
const HB_RIGHT: u32 = 0x1a6;
const HB_SLIDER: u32 = 0x1a7;
/// `FUN_100746ec` buttons: `Button_c::SetGfx(btn, state, id)` states (normal, pressed, hover): target button 0xaa/0xac/0xab,
/// left arrow 0x93/0x95/0x94, right arrow 0x96/0x98/0x97; icon `0xad + (friendly)`: 0xad `…TARGET_ICON_OTHER` (hostile control),
/// 0xae `…TARGET_ICON_SELF` (friendly control).
const BTN: [u32; 3] = [0xaa, 0xac, 0xab];
const ARROW_L: [u32; 3] = [0x93, 0x95, 0x94];
const ARROW_R: [u32; 3] = [0x96, 0x98, 0x97];
const ICON_OTHER: u32 = 0xad;
const ICON_SELF: u32 = 0xae;
/// `CreateTargetMenus` 0x1006a0d4: `x = (sw − 192)/2 · {½, 3/2} − width/2`, `y = 5`; bar width `(sw − 192)·½ − 50` (`FUN_100746ec`).
const SIDE_MARGIN: f32 = 192.0;
const BAR_MARGIN: f32 = 50.0;

struct Bar {
    window: WindowId,
    hostile: bool,
    width: f32,
    shown: Option<TargetInfo>,
    rows: [Option<(TargetInfo, &'static str, bool)>; 2],
}

/// `FUN_1007313a`: stat 1 feeds sqrt(2 * sqrt(MaxHealth)); 64 is `_DAT_101b5ebc`,
/// 0.01 is the double `_DAT_101b5c60`. The screen-derived width is an upper bound, not the displayed width.
fn health_bar_width(max_width: f32, max_health: i32) -> f32 {
    let scale = ((max_health as u32 as f64).sqrt() * 2.0).sqrt() * 0.01;
    (64.0 + (max_width - 64.0) * scale as f32).floor().clamp(64.0, max_width)
}

/// `FUN_10073d0f`: selection is a nano target for characters; fight always lives on the hostile control.
fn control_targets(selection: Option<&TargetInfo>, fight: Option<&TargetInfo>, hostile: bool) -> [Option<(i32, &'static str, bool)>; 2] {
    if !hostile {
        return [selection.filter(|s| !s.hostile && fight.is_none_or(|f| f.id != s.id || f.kind != s.kind)).map(|s| (s.id, if s.kind == DYNEL_CHAR { "Nano Target" } else { "Selection" }, false)), None];
    }
    let nano = selection.filter(|s| s.hostile);
    match (nano, fight) {
        (Some(s), Some(f)) if s.id == f.id => [Some((f.id, "Nano / Fighting<br>Target", true)), None],
        (s, Some(f)) => [Some((f.id, "Fighting Target", true)), s.map(|s| (s.id, "Nano Target", false))],
        (s, None) => [s.map(|s| (s.id, "Nano Target", false)), None],
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Prev,
    Button,
    Next,
}

struct Dock {
    hostile: bool,
}

impl Dock {
    fn view(&self, p: Part) -> &'static str {
        match (self.hostile, p) {
            (false, Part::Prev) => "ft_prev",
            (false, Part::Button) => "ft_btn",
            (false, Part::Next) => "ft_next",
            (true, Part::Prev) => "ht_prev",
            (true, Part::Button) => "ht_btn",
            (true, Part::Next) => "ht_next",
        }
    }
}

pub(super) struct HudTarget {
    cc: WindowId,
    size: (u32, u32),
    bars: Vec<Bar>,
    docks: [Dock; 2],
    /// `m_cLastTarget` of `TargetingModule_t` (restored by a second `SelectSelf`).
    last: Option<Identity>,
    mouse: (f32, f32),
    pressed: Option<(bool, Part)>,
    /// A left press on the world (not on the GUI) is pending: `(accumulated pointer path length, last position)`; the release
    /// is a click while the path is within [`CLICK_PATH`] (`ActionViewMouseHandler_c` `+0x1c`).
    world_down: Option<(f32, (f32, f32))>,
    /// The `Targetstarget` preference ("Show Target's Target", `LoginPrefs.xml`, default false): the hostile window shows the
    /// target-of-target button.
    pub(super) targets_target: bool,
    /// `[friendly, hostile]` health bar windows allowed by their criteria (`dvalue:cc_section1 && dvalue:cc_friendly_health_bar` / `..._hostile_health_bar`, docs/gui.md §10).
    pub(super) bars_enabled: [bool; 2],
    /// The target of the selected dynel as shown in that button (`CCTargetControl_c+0x13c`).
    tot: Option<i32>,
    /// The left press went down on the target-of-target button.
    tot_down: bool,
    /// The hostile dock's Attack button was clicked (`PerformSpecialAction(0xb)`), taken by the HUD.
    pub(super) attack: bool,
    /// A Shift + click asked the info page of this character (`InfoViewModule_c::ShowURL("charid://50000/<id>")`), taken by the HUD.
    pub(super) info: Option<i32>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl HudTarget {
    /// Creates the two health-bar windows (`CCFriendlyHealthBar` / `CCHostileHealthBar`) and fills the control-centre target docks.
    pub(super) fn new(gui: &mut Gui, cc: WindowId, size: (u32, u32)) -> anyhow::Result<Self> {
        let mut t = HudTarget { cc, size, bars: vec![], docks: [Dock { hostile: false }, Dock { hostile: true }], last: None, mouse: (-1.0, -1.0), pressed: None, world_down: None, targets_target: false, bars_enabled: [true; 2], tot: None, tot_down: false, attack: false, info: None };
        t.create_bars(gui)?;
        // `TargetHeader_c` 0x10073884 measures all four captions before clearing the initial text.
        let cap = ["Selection", "Nano Target", "Fighting Target", "Nano / Fighting", "Target"]
            .into_iter().map(|caption| gui.text_width(ao_gui::FontId::Bold, caption)).max().unwrap_or(0);
        for (dock, d) in [("LeftTargetCtrlDock", &t.docks[0]), ("RightTargetCtrlDock", &t.docks[1])] {
            let prefix = if d.hostile { "ht" } else { "ft" };
            let mut headers = String::new();
            for row in 0..2 {
                headers.push_str(&format!("<BorderView name=\"{prefix}_header{row}\" min_size=\"Point({cap},0)\" view_flags=\"0x100\" view_layout=\"vertical\" tl_gfx=\"GFX_GUI_CC_TARGET_FRAME_TL\" tr_gfx=\"GFX_GUI_CC_TARGET_FRAME_TR\" bl_gfx=\"GFX_GUI_CC_TARGET_FRAME_BL\" br_gfx=\"GFX_GUI_CC_TARGET_FRAME_BR\" left_gfx=\"\" top_gfx=\"\" right_gfx=\"\" bottom_gfx=\"\" color=\"DEFAULT\" layout_borders=\"Rect(0,0,0,10)\"><TextView name=\"{prefix}_title{row}\" font=\"BOLD\" value=\"\"/><TextView name=\"{prefix}_name{row}\" font=\"NORMAL\" value=\"\"/></BorderView>"));
            }
            let src = format!(
                "<root><View view_layout=\"vertical\">{headers}<View view_layout=\"horizontal\">\
                 <CanvasView name=\"{p}\" min_size=\"Point(7,40)\" max_size=\"Point(7,40)\" layout_borders=\"Rect(0,0,5,0)\"/>\
                 <CanvasView name=\"{b}\" min_size=\"Point(47,47)\" max_size=\"Point(47,47)\"/>\
                 <CanvasView name=\"{n}\" min_size=\"Point(7,40)\" max_size=\"Point(7,40)\" layout_borders=\"Rect(5,0,0,0)\"/>\
                 </View></View></root>",
                p = d.view(Part::Prev),
                b = d.view(Part::Button),
                n = d.view(Part::Next)
            );
            gui.add_view_xml(cc, dock, dock, &src)?;
            for row in 0..2 {
                gui.set_visible(cc, &format!("{prefix}_header{row}"), false);
            }
        }
        Ok(t)
    }
    pub(super) fn persistent_windows(&self) -> impl Iterator<Item = (WindowId, &'static str)> + '_ {
        self.bars.iter().map(|b| (b.window, if b.hostile { "CCHostileHealthBarConfig" } else { "CCFriendlyHealthBarConfig" }))
    }


    fn bar_width(size: (u32, u32)) -> f32 {
        ((size.0 as f32 - SIDE_MARGIN) * 0.5 - BAR_MARGIN).floor().max(64.0)
    }

    fn create_bars(&mut self, gui: &mut Gui) -> anyhow::Result<()> {
        let w = Self::bar_width(self.size);
        for hostile in [false, true] {
            // `TargetTargetButton_c` (`FUN_10075342`), first child of the hostile window: a corner `BorderView` (the header's frame
            // gfx, colour 0x7fffff) with the caption "Fighting Target:" and the target's target name, borders 3 px, 5 px below.
            let tot = if hostile {
                "<BorderView name=\"tot\" view_layout=\"vertical\" tl_gfx=\"GFX_GUI_CC_TARGET_FRAME_TL\" tr_gfx=\"GFX_GUI_CC_TARGET_FRAME_TR\" \
                 bl_gfx=\"GFX_GUI_CC_TARGET_FRAME_BL\" br_gfx=\"GFX_GUI_CC_TARGET_FRAME_BR\" left_gfx=\"\" top_gfx=\"\" right_gfx=\"\" bottom_gfx=\"\" color=\"0x7fffff\" layout_borders=\"Rect(0,0,0,5)\">\
                 <TextView name=\"tot_caption\" font=\"NORMAL\" value=\"Fighting Target:\" layout_borders=\"Rect(3,3,3,0)\"/>\
                 <TextView name=\"tot_name\" font=\"NORMAL\" value=\"\" layout_borders=\"Rect(3,0,3,3)\"/>\
                 </BorderView>"
            } else {
                ""
            };
            let src = format!(
                "<root><View view_layout=\"vertical\">\
                 <View name=\"row0\" view_flags=\"0x100\" view_layout=\"vertical\"><CanvasView name=\"bar\" min_size=\"Point(63,10)\" max_size=\"Point(63,10)\"/><TextView name=\"name\" font=\"NORMAL\" value=\"\" layout_borders=\"Rect(0,0,0,10)\"/></View>\
                 {tot}<View name=\"row1\" view_flags=\"0x100\" view_layout=\"vertical\"><CanvasView name=\"bar1\" min_size=\"Point(63,10)\" max_size=\"Point(63,10)\"/><TextView name=\"name1\" font=\"NORMAL\" value=\"\" layout_borders=\"Rect(0,0,0,10)\"/></View>\
                 </View></root>"
            );
            let name = if hostile { "CCHostileHealthBar" } else { "CCFriendlyHealthBar" };
            let window = gui.open_window_xml(name, &src, (0, 5), WindowSize::Preferred)?;
            if hostile {
                gui.set_visible(window, "tot", false);
            }
            gui.set_window_visible(window, false);
            self.bars.push(Bar { window, hostile, width: w, shown: None, rows: [None, None] });
        }
        self.place(gui);
        Ok(())
    }

    /// Leaving the world: the health-bar windows go.
    pub(super) fn close(&mut self, gui: &mut Gui) {
        for b in self.bars.drain(..) {
            gui.close_window(b.window);
        }
    }

    fn place(&mut self, gui: &mut Gui) {
        let sw = self.size.0 as f32;
        for b in &mut self.bars {
            let (ww, hh) = gui.window_size(b.window);
            // CharBarWindow's child mouse signals move its style-3 frame (GUI 10067b8c/10067bf4).
            gui.set_window_move_region(b.window, Some(ao_gui::Rect::new(0.0, 0.0, ww.saturating_sub(1) as f32, hh.saturating_sub(1) as f32)));
            if gui.window_moved(b.window) { continue; }
            let k = if b.hostile { 1.5 } else { 0.5 };
            let x = ((sw - SIDE_MARGIN) * 0.5 * k - ww as f32 * 0.5).floor();
            let pos = (x as i32, 5);
            gui.set_window_pos(b.window, pos);
        }
    }

    pub(super) fn resize(&mut self, gui: &mut Gui, size: (u32, u32)) {
        if size != self.size {
            self.size = size;
            let w = Self::bar_width(size);
            for b in &mut self.bars {
                b.width = w;
                b.rows = [None, None];
            }
            self.place(gui);
        }
    }

    /// `SetTarget`: remembers the previous selection for `SelectSelf` and stores the new one.
    pub(super) fn select(&mut self, zone: &mut Zone, id: Option<i32>) {
        self.select_identity(zone, id.map(|instance| Identity { kind: DYNEL_CHAR, instance }));
    }

    fn select_identity(&mut self, zone: &mut Zone, id: Option<Identity>) {
        if zone.selected_target() != id && id.is_none_or(|id| zone.target_on_ground(id)) {
            if let Some(old) = zone.selected_target() {
                self.last = Some(old);
            }
            zone.set_target(id);
        }
    }

    /// `TargetingModule_t::SelectSelf` 0x100259b1: no target → select self; target is self → back to the previous target.
    fn select_self(&mut self, zone: &mut Zone) {
        let me = zone.char_id as i32;
        match zone.target {
            None => self.select(zone, Some(me)),
            Some(t) if t == me => {
                let back = self.last.filter(|id| zone.target_on_ground(*id) && *id != Identity { kind: DYNEL_CHAR, instance: me });
                self.select_identity(zone, back);
            }
            Some(_) => self.select(zone, Some(me)),
        }
    }

    /// A left click on the world (not on the GUI), `ActionViewMouseHandler_c`'s release slot `FUN_1002c469` (GUI 0x1002c469): the hit
    /// list under the pointer merges characters and props via [`super::interact_use::pick_objects`] (no hit = nothing happens, no deselect). Plain click: [`click_target`] chooses and the
    /// choice becomes the target (`Send(0x1e, 0x126)` → `TargetingModule_t::SetTargetMessage`). **Shift**: the object under the pointer
    /// ([`object_under`]) is not selected, its info page is requested (`charid://50000/id` for characters, `itemid://kind/id` otherwise).
    /// **Ctrl / Alt** (the `& 0xc` qualifier, which bit is which is unresolved) on a character: that object
    /// itself is selected, not the next one in the list, and the caller attacks it (`N3Msg_SwitchTarget` = `DefaultAttack(target,
    /// true)`, `flow.rs` via `Hud::take_click`). The right button and the double click (`N3Msg_DefaultActionOnDynel`) are
    /// `interact_play.rs`.
    pub(super) fn world_click(&mut self, zone: &mut Zone, cam: &Camera, lens: &Lens, vp: (u32, u32), mouse: (f32, f32), mods: ao_gui::Modifiers) -> Option<WorldClick> {
        let ray = pick_ray(cam, lens, (vp.0 as f32, vp.1 as f32), mouse);
        let list = super::interact_use::pick_objects(&ray, zone);
        self.click_identities(zone, &list, mods)
    }

    /// [`HudTarget::world_click`] on a given hit list.
    fn click_identities(&mut self, zone: &mut Zone, list: &[Identity], mods: ao_gui::Modifiers) -> Option<WorldClick> {
        let current = zone.selected_target();
        let under = object_under(list, current)?;
        if mods.shift {
            return Some(if under.kind == DYNEL_CHAR { WorldClick::Info(under.instance) } else { WorldClick::ObjectInfo(under) });
        }
        let id = if (mods.ctrl || mods.alt) && under.kind == DYNEL_CHAR { under } else { click_target(list, current)? };
        self.select_identity(zone, Some(id));
        (id.kind == DYNEL_CHAR).then_some(WorldClick::Select(id.instance))
    }

    #[cfg(test)]
    fn click_list(&mut self, zone: &mut Zone, list: &[i32], mods: ao_gui::Modifiers) -> Option<WorldClick> {
        let ids: Vec<_> = list.iter().map(|&instance| Identity { kind: DYNEL_CHAR, instance }).collect();
        self.click_identities(zone, &ids, mods)
    }

    /// Raw mouse input before the GUI: dock buttons (hit-tested on their canvases) and world clicks. Returns `Some(pos)` when a
    /// world click completed: the left press was on the world (the pointer not over the GUI) and the pointer travelled at most
    /// [`CLICK_PATH`] up to the release.
    pub(super) fn input(&mut self, gui: &Gui, zone: &mut Zone, ev: &InputEvent) -> Option<(f32, f32)> {
        match *ev {
            InputEvent::MouseMove { x, y } => {
                self.mouse = (x, y);
                if let Some((path, last)) = &mut self.world_down {
                    *path += ((x - last.0).powi(2) + (y - last.1).powi(2)).sqrt();
                    *last = (x, y);
                }
            }
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                self.mouse = (x, y);
                self.pressed = self.dock_at(gui, x, y);
                self.tot_down = self.tot_at(gui, x, y);
                self.world_down = (self.pressed.is_none() && !self.tot_down && !gui.wants_mouse(x, y)).then_some((0.0, (x, y)));
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                self.mouse = (x, y);
                let press = self.pressed.take();
                if let Some((hostile, part)) = press.filter(|p| self.dock_at(gui, x, y) == Some(*p)) {
                    self.dock_click(zone, hostile, part);
                    return None;
                }
                if std::mem::take(&mut self.tot_down) && self.tot_at(gui, x, y) {
                    self.tot_click(zone);
                    return None;
                }
                if let Some((path, _)) = self.world_down.take() {
                    if path <= CLICK_PATH && !gui.wants_mouse(x, y) {
                        return Some((x, y));
                    }
                }
            }
            _ => {}
        }
        None
    }

    /// The pointer is over the visible target-of-target button of the hostile window.
    fn tot_at(&self, gui: &Gui, x: f32, y: f32) -> bool {
        self.tot.is_some() && self.bars.iter().filter(|b| b.hostile).any(|b| gui.view_rect(b.window, "tot").is_some_and(|r| x >= r.l && x <= r.r + 1.0 && y >= r.t && y <= r.b + 1.0))
    }

    /// Click handler of `TargetTargetButton_c` (`LAB_10073554`): when `N3Msg_CanClickTargetTarget(target, targetsTarget)` the
    /// target's target is selected (`Send(0x1e, 0x126)`).
    fn tot_click(&mut self, zone: &mut Zone) {
        let current = self.bars.iter().find(|b| b.hostile).and_then(|b| b.shown.as_ref()).map(|i| i.id);
        if let (Some(cur), Some(tot)) = (current, self.tot) {
            if can_click_target_target(zone, cur, tot) {
                self.select(zone, Some(tot));
            }
        }
    }

    fn dock_at(&self, gui: &Gui, x: f32, y: f32) -> Option<(bool, Part)> {
        for d in &self.docks {
            for p in [Part::Prev, Part::Button, Part::Next] {
                let hit = gui.view_rect(self.cc, d.view(p)).is_some_and(|r| x >= r.l && x <= r.r + 1.0 && y >= r.t && y <= r.b + 1.0);
                if hit {
                    return Some((d.hostile, p));
                }
            }
        }
        None
    }

    /// The dock handlers (`LAB_10072fcb` / `LAB_10072ff1` / `LAB_10073017`, GUI): the arrows send `GetPrev{Friendly,Hostile}Target`
    /// (AFCM 0x1e, 0x105 / 0x106) and `GetNext…` (0xe8 / 0xe9), i.e. [`cycle`]; the friendly button is `TargetingModule_t::
    /// SelectSelf`; the hostile button is the Attack button, `N3Msg_PerformSpecialAction(0xb)` = `DefaultAttack(target, false)`
    /// (docs/zone/combat-net.md §5.3; queued in [`Self::attack`], the HUD turns it into a `SlotUse::SpecialAction`). None of these views has a tooltip: the
    /// `CCTargetControl_c` constructor `FUN_100746ec` and `TargetHealthBar_c` / `TargetHeader_c` never call `View::SetToolTip`.
    fn dock_click(&mut self, zone: &mut Zone, hostile: bool, part: Part) {
        match part {
            Part::Prev | Part::Next => {
                if let Some(id) = cycle(zone, hostile, part == Part::Next) {
                    self.select(zone, Some(id));
                }
            }
            Part::Button if !hostile => self.select_self(zone),
            Part::Button => self.attack = true,
        }
    }

    /// The target keys `KEY_NEXT_HOSTILE_TARGET` (Tab), `KEY_PREV_HOSTILE_TARGET` (Shift+Tab), `KEY_NEXT_FRIENDLY_TARGET` (Ctrl+Tab),
    /// `KEY_PREV_FRIENDLY_TARGET` (Ctrl+Shift+Tab): the fixed keys of GUI.dll's hot key table, read from [`FixedKeys`] (the `Login.cfg` `KEY_*` ints).
    pub(super) fn key(&mut self, zone: &mut Zone, key: ao_gui::Key, mods: ao_gui::Modifiers, fixed: &FixedKeys) -> bool {
        let Some(kid) = gui_key_id(key) else { return false };
        let input = super::options::keys::key_input(kid, mods);
        let step = [
            ("KEY_NEXT_HOSTILE_TARGET", true, true),
            ("KEY_PREV_HOSTILE_TARGET", true, false),
            ("KEY_NEXT_FRIENDLY_TARGET", false, true),
            ("KEY_PREV_FRIENDLY_TARGET", false, false),
        ]
        .into_iter()
        .find(|(n, ..)| fixed.get(n) == input);
        let Some((_, hostile, next)) = step else { return false };
        if let Some(id) = cycle(zone, hostile, next) {
            self.select(zone, Some(id));
        }
        true
    }

    /// `MousePointerModule_t` for this frame: the pointer sprites for the dynel under the mouse ([`object_under`] of the hit list,
    /// `CheckObjectUnderMouse`), appended to `list`, and the request to hide the OS cursor. The pointer is the game's own only over
    /// the world (not while the camera looks and not over a GUI window), where the OS cursor stays hidden.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_cursor(&self, gui: &Gui, zone: &Zone, host: &mut ao_render::Host, list: &mut ao_gui::DrawList, vp: (u32, u32), mode: i64, dblclick: bool) {
        let (x, y) = self.mouse;
        host.hide_cursor = false;
        if x < 0.0 || y < 0.0 || host.look || gui.wants_mouse(x, y) {
            return;
        }
        let ray = pick_ray(&host.camera, &host.lens.unwrap_or_default(), (vp.0 as f32, vp.1 as f32), (x, y));
        let hits = super::interact_use::pick_objects(&ray, zone);
        let target = zone.selected_target();
        let mouse = target.filter(|id| hits.contains(id)).or_else(|| hits.first().copied()).and_then(|id| {
            if id.kind != ao_net::n3::outgoing::DYNEL_CHAR {
                let can = super::interact::Interact::can_of(zone, id)?;
                return Some(super::hud_cursor::choose_object(can, host.mods.shift, dblclick));
            }
            let d = zone.dynels.get(&id.instance)?;
            let (flags, features) = zone.world.pointer_stats(id.instance)?;
            let flags = zone.stat_of(id.instance, 0).unwrap_or(flags);
            let features = zone.stat_of(id.instance, 0xe0).or(features);
            let own_side = zone.own().map_or(0, |o| o.side as i32);
            let hover = super::hud_cursor::Hover { npc: d.npc, flags, features, side: d.side as i32, own_side, team: zone.stat_of(id.instance, 6), own_team: zone.stat(6).unwrap_or(0), vulnerable: false };
            Some(super::hud_cursor::choose(&hover, host.mods.shift, host.mods.ctrl || host.mods.alt, dblclick))
        });
        super::hud_cursor::draw(|g| gui.gfx().size(g), mode, (x, y), mouse.unwrap_or_default(), list);
        host.hide_cursor = true;
    }

    /// `TargetingModule_t::FrameProcess`: the target goes away with its dynel; then the controls follow the selection.
    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, _dt: f32) {
        if zone.selected_target().is_some_and(|id| !zone.target_on_ground(id)) {
            zone.set_target(None);
        }
        let sel = selected_info(zone);
        let fight = zone.fight_target.get(&(zone.char_id as i32)).and_then(|t| info(zone, *t));
        let mut layout_changed = false;
        for b in &mut self.bars {
            let targets = control_targets(sel.as_ref(), fight.as_ref(), b.hostile)
                .map(|t| t.and_then(|(id, caption, attacking)| {
                    let value = if !attacking { sel.as_ref().filter(|s| s.id == id).cloned() } else { info(zone, id) };
                    value.map(|i| (i, caption, attacking))
                }));
            let prefix = if b.hostile { "ht" } else { "ft" };
            let enabled = self.bars_enabled[usize::from(b.hostile)];
            b.shown = targets[0].as_ref().map(|(i, _, _)| i.clone()).filter(|_| enabled);
            gui.set_window_visible(b.window, enabled && targets.iter().any(Option::is_some));
            if targets == b.rows {
                continue;
            }
            layout_changed = true;
            for (row, target) in targets.iter().enumerate() {
                let dock_row = if b.hostile && targets[1].is_some() { 1 - row } else { row };
                gui.set_visible(self.cc, &format!("{prefix}_header{dock_row}"), target.is_some());
                gui.set_visible(b.window, &format!("row{row}"), target.is_some());
                let (bar, name) = if row == 0 { ("bar", "name") } else { ("bar1", "name1") };
                let mut items = Vec::new();
                if let Some((i, caption, attacking)) = target {
                    let color = if *attacking { 0xff4444 } else { 0xffffff };
                    let text = format!("<center><font color=0x{color:06x}>{}</font></center>", esc(&i.name));
                    gui.set_text(b.window, name, &text);
                    gui.set_text(self.cc, &format!("{prefix}_name{dock_row}"), &text);
                    gui.set_text(self.cc, &format!("{prefix}_title{dock_row}"), &format!("<center>{caption}</center>"));
                    let width = health_bar_width(b.width, i.max_health) + 18.0;
                    gui.set_view_pref_size(b.window, bar, (width - 1.0, 10.0), (width - 1.0, 10.0));
                    bar_items(width as i32, i.health, i.color, &mut items);
                }
                gui.set_canvas(b.window, bar, items);
            }
            gui.resize_window(b.window, WindowSize::Preferred);
            b.rows = targets;
        }
        if layout_changed {
            gui.relayout_window(self.cc);
        }
        // `FUN_10073b9e`: the target-of-target button of the shown hostile window (pref `Targetstarget`, `N3Msg_GetTargetTarget`)
        let tot = self
            .targets_target
            .then(|| self.bars.iter().find(|b| b.hostile).and_then(|b| b.shown.as_ref()).and_then(|t| zone.fight_target.get(&t.id).copied()))
            .flatten()
            .filter(|t| zone.dynels.contains_key(t))
            .filter(|_| self.bars.iter().any(|b| b.hostile && b.shown.is_some()));
        if tot != self.tot {
            self.tot = tot;
            if let Some(w) = self.bars.iter().find(|b| b.hostile).map(|b| b.window) {
                gui.set_visible(w, "tot", tot.is_some());
                if let Some(t) = tot {
                    gui.set_text(w, "tot_name", &format!("<font color=0xffffff>{}</font>", clean(&zone.dynels[&t].name)));
                }
                gui.resize_window(w, WindowSize::Preferred);
            }
        }
        self.place(gui);
        let hover = self.dock_at(gui, self.mouse.0, self.mouse.1);
        for d in &self.docks {
            for p in [Part::Prev, Part::Button, Part::Next] {
                let this = Some((d.hostile, p));
                let toggled = p == Part::Button && !d.hostile && zone.target == Some(zone.char_id as i32);
                let state = if self.pressed == this && hover == this || toggled { 1 } else if hover == this { 2 } else { 0 };
                let (ids, icon) = match p {
                    Part::Prev => (ARROW_L, None),
                    Part::Next => (ARROW_R, None),
                    Part::Button => (BTN, Some(if d.hostile { ICON_OTHER } else { ICON_SELF })),
                };
                let g = ao_gui::GfxId(ids[state]);
                let (w, h) = gui.gfx().size(g);
                let mut items = vec![ao_gui::view::CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, w as f32, h as f32], alpha: 1.0 }];
                if let Some(ic) = icon {
                    // `Button_c::SetIconView(btn, bitmap, true)`: the icon is centred on the button
                    let ig = ao_gui::GfxId(ic);
                    let (iw, ih) = gui.gfx().size(ig);
                    let (x, y) = (((w - iw) / 2) as f32, ((h - ih) / 2) as f32);
                    items.push(ao_gui::view::CanvasItem::Image { id: ig, src: [0.0, 0.0, iw as f32, ih as f32], dst: [x, y, x + iw as f32, y + ih as f32], alpha: 1.0 });
                }
                gui.set_canvas(self.cc, d.view(p), items);
            }
        }
    }
}

/// `TargetHealthBar_c` surfaces (`FUN_10073598`, `FUN_10072ead`): caps at both ends tinted DEFAULT, the background between them
/// (DEFAULT) and the slider over the background up to `ratio` of its width. Both 16×11 textures tile (Load flag 0x10);
/// source rectangles must stay inside each atlas entry. Red caps require attacking `+0x161` and in-attack-range `+0x162`,
/// delivered by `FUN_1007353f` from `GlobalSignals+0x64` (GC 0x10068969 / 0x100679c1).
/// The port lacks the slot's effective attack range and both retail collision radii; caps retain DEFAULT rather than guessing.
fn bar_items(width: i32, ratio: f32, color: u32, out: &mut Vec<ao_gui::view::CanvasItem>) {
    use ao_gui::view::CanvasItem::ImageTint;
    use ao_gui::GfxId;
    let (cap_w, h) = (9.0, 11.0);
    let w = width as f32;
    let default = 0x1000000;
    out.push(ImageTint { id: GfxId(HB_LEFT), src: [0.0, 0.0, cap_w, h], dst: [0.0, 0.0, cap_w, h], color: default, alpha: 1.0 });
    out.push(ImageTint { id: GfxId(HB_RIGHT), src: [0.0, 0.0, cap_w, h], dst: [w - cap_w, 0.0, w, h], color: default, alpha: 1.0 });
    let (l, r) = (cap_w + 1.0, w - cap_w - 1.0);
    for (id, length, tint) in [(HB_BACKGROUND, r - l, default), (HB_SLIDER, (r - l) * ratio, color)] {
        let mut x = 0.0;
        while x < length {
            let tw = (length - x).min(16.0);
            out.push(ImageTint { id: GfxId(id), src: [0.0, 0.0, tw, h], dst: [l + x, 0.0, l + x + tw, h], color: tint, alpha: 1.0 });
            x += 16.0;
        }
    }
}

#[test]
fn health_bar_tiles_stay_inside_the_atlas_entry() {
    use ao_gui::view::CanvasItem::ImageTint;
    for ratio in [1.0, 0.5, 0.1, 0.0] {
        let mut items = Vec::new();
        bar_items(100, ratio, 0xffffff, &mut items);
        for (id, length) in [(HB_BACKGROUND, 80.0), (HB_SLIDER, 80.0 * ratio)] {
            let mut edge = 10.0;
            for item in &items {
                if let ImageTint { id: gfx, src, dst, .. } = item {
                    if gfx.0 == id {
                        assert_eq!(src[0..2], [0.0, 0.0]);
                        assert!(src[2] > 0.0 && src[2] <= 16.0);
                        assert_eq!(src[3], 11.0);
                        assert_eq!(dst[0], edge, "tiles must meet without gaps");
                        assert_eq!(dst[2] - dst[0], src[2]);
                        edge = dst[2];
                    }
                }
            }
            assert_eq!(edge, 10.0 + length);
        }
    }
}

/// The client key id of a key the GUI layer reports (`InputConfig_t` key table, `controls::key_id`).
pub(super) fn gui_key_id(key: ao_gui::Key) -> Option<u32> {
    use ao_gui::Key::*;
    Some(match key {
        Escape => 14,
        Tab => 15,
        Enter => 24,
        Backspace => 25,
        Delete => 27,
        Home => 28,
        End => 29,
        Left => 32,
        Right => 33,
        Up => 34,
        Down => 35,
        Letter('/') => 118,
        Letter(c) => super::controls::char_key_id(c)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_health_width_tint_and_control_routing() {
        let mut z = Zone::new(1);
        z.stats.insert(stats::LEVEL, 5);
        z.stats.insert(0x113, 5);
        z.stats.insert(stats::SIDE, 1);
        z.dynels.insert(2, dyn_at("Selected", [0.0; 3], true, 0));
        z.dynels.insert(3, dyn_at("Fighter", [0.0; 3], true, 0));
        let selected = info(&z, 2).unwrap();
        let fight = info(&z, 3).unwrap();
        assert_eq!(selected.color, 0xfff000);
        assert_eq!(control_targets(Some(&selected), None, true), [Some((2, "Nano Target", false)), None]);
        assert_eq!(control_targets(Some(&selected), Some(&selected), true), [Some((2, "Nano / Fighting<br>Target", true)), None]);
        assert_eq!(control_targets(Some(&selected), Some(&fight), true), [Some((3, "Fighting Target", true)), Some((2, "Nano Target", false))]);
        assert_eq!(control_targets(Some(&selected), Some(&fight), false), [None, None]);
        assert_eq!(health_bar_width(494.0, 0), 64.0);
        assert_eq!(health_bar_width(494.0, 10_000), 124.0);
        assert_eq!(health_bar_width(494.0, 100_000_000), 494.0);
        let mut items = Vec::new();
        bar_items(100, selected.health, selected.color, &mut items);
        assert!(items.iter().any(|i| matches!(i, ao_gui::view::CanvasItem::ImageTint { id, color: 0xfff000, .. } if id.0 == HB_SLIDER)));
        z.dynels.get_mut(&2).unwrap().health = 0;
        assert_eq!(info(&z, 2).unwrap().health, 0.0, "presentation consumes the central health projection");
    }
    use ao_gui::{Gui, InputEvent, WindowSize};

    fn dyn_at(name: &str, p: [f32; 3], npc: bool, side: u8) -> DynelState {
        DynelState { name: name.into(), pos: p, yaw: None, npc, side, level: 5, health: 30, max_health: 60 }
    }

    #[test]
    fn ray_through_the_screen_centre_is_the_view_axis() {
        let cam = Camera::look_at(Vec3::new(0.0, 1.7, 0.0), Vec3::new(0.0, 1.7, -10.0));
        let lens = Lens { fov: std::f32::consts::FRAC_PI_2, horizontal: true, near: 0.2, far: None };
        let r = pick_ray(&cam, &lens, (800.0, 600.0), (400.0, 300.0));
        assert!((r.dir - cam.forward()).length() < 1e-5);
        // right edge, vertical centre: 45 degrees to the right (horizontal fov 90)
        let r = pick_ray(&cam, &lens, (800.0, 600.0), (800.0, 300.0));
        assert!((r.dir.dot(cam.forward()) - 0.5f32.sqrt()).abs() < 1e-4 && r.dir.dot(cam.right()) > 0.0);
        // top edge: the vertical half angle is atan(tan(45)/aspect)
        let r = pick_ray(&cam, &lens, (800.0, 600.0), (400.0, 0.0));
        let up = r.dir.dot(cam.up()) / r.dir.dot(cam.forward());
        assert!((up - 0.75).abs() < 1e-4, "{up}");
    }

    use super::super::hud_pick::{self, PickBody};
    use ao_gui::Modifiers;

    fn body(id: i32, scene_z: f32) -> PickBody {
        PickBody { id, bounds: ([-0.4, 0.0, -0.3], [0.4, 1.8, 0.3]), pos: [0.0, 0.0, scene_z], yaw: 0.0, scale: 1.0 }
    }

    /// The ray/box primitive used by the merged `interact_use::pick_objects` selection path.
    fn list(cam: &Camera, lens: &Lens, bodies: &[PickBody], mouse: (f32, f32)) -> Vec<i32> {
        let ray = pick_ray(cam, lens, (800.0, 600.0), mouse);
        hud_pick::hits(bodies, ray.origin, ray.dir, ray.len)
    }

    #[test]
    fn clicks_walk_through_the_hit_list_and_ground_keeps_the_target() {
        let mut z = Zone::new(1);
        z.dynels.insert(2, dyn_at("Near", [0.0, 0.0, 5.0], true, 2));
        z.dynels.insert(3, dyn_at("Far", [0.0, 0.0, 12.0], true, 2));
        let bodies = [body(3, -12.0), body(2, -5.0)];
        let cam = Camera::look_at(Vec3::new(0.0, 1.2, 1.0), Vec3::new(0.0, 1.2, -10.0));
        let lens = Lens::default();
        let centre = list(&cam, &lens, &bodies, (400.0, 300.0));
        assert_eq!(centre, vec![2, 3], "nearest box first");
        // a click at the far left of the screen misses everything
        let ground = list(&cam, &lens, &bodies, (5.0, 300.0));
        assert!(ground.is_empty());
        // `n3Camera_t::GetNextTarget`: first hit, then the one after the current target, wrapping; a stranger restarts the list
        assert_eq!(click_target(&[2, 3], None), Some(2));
        assert_eq!(click_target(&[2, 3], Some(2)), Some(3));
        assert_eq!(click_target(&[2, 3], Some(3)), Some(2));
        assert_eq!(click_target(&[2, 3], Some(9)), Some(2));
        assert_eq!(click_target(&[], Some(2)), None);
        // `GetObjectUnderColLine`: the current target when it is hit, else the first entry
        assert_eq!(object_under(&[2, 3], Some(3)), Some(3));
        assert_eq!(object_under(&[2, 3], Some(9)), Some(2));
        assert_eq!(object_under(&[], Some(2)), None);
        // the first click selects the near one, the second the far one, the ground changes nothing
        let mut h = HudTargetLite::default();
        let none = Modifiers::default();
        assert_eq!(h.0.click_list(&mut z, &centre, none), Some(WorldClick::Select(2)));
        assert_eq!(z.target, Some(2));
        assert_eq!(h.0.click_list(&mut z, &centre, none), Some(WorldClick::Select(3)));
        assert_eq!(z.target, Some(3));
        assert_eq!(h.0.click_list(&mut z, &ground, none), None);
        assert_eq!(z.target, Some(3));
    }

    #[test]
    fn modifier_clicks_follow_the_release_handler() {
        let mut z = Zone::new(1);
        z.dynels.insert(2, dyn_at("Near", [0.0, 0.0, 5.0], true, 2));
        z.dynels.insert(3, dyn_at("Far", [0.0, 0.0, 12.0], true, 2));
        let mut h = HudTargetLite::default();
        z.target = Some(2);
        // Shift: the info page of the object under the pointer (the current target if it is hit); the selection stays
        assert_eq!(h.0.click_list(&mut z, &[2, 3], Modifiers { shift: true, ..Default::default() }), Some(WorldClick::Info(2)));
        assert_eq!(h.0.click_list(&mut z, &[3], Modifiers { shift: true, ..Default::default() }), Some(WorldClick::Info(3)));
        assert_eq!(h.0.click_list(&mut z, &[], Modifiers { shift: true, ..Default::default() }), None);
        assert_eq!(z.target, Some(2));
        // Ctrl / Alt: that object itself (not the next one), the caller attacks it
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(h.0.click_list(&mut z, &[2, 3], ctrl), Some(WorldClick::Select(2)));
        assert_eq!(h.0.click_list(&mut z, &[3], Modifiers { alt: true, ..Default::default() }), Some(WorldClick::Select(3)));
        assert_eq!(z.target, Some(3));
    }

    #[test]
    fn object_selection_keeps_identity_and_uses_the_retail_selection_header() {
        let mut z = Zone::new(1);
        z.dynels.insert(1, dyn_at("Me", [0.0; 3], false, 1));
        z.dynels.insert(2, dyn_at("Fighter", [0.0; 3], true, 0));
        let corpse = Identity { kind: 0xc76a, instance: 2 };
        let item = Identity { kind: 0xc748, instance: 3 };
        for id in [corpse, item] {
            z.world.test_prop(id, vec![(stats::LIFE, 10), (stats::HEALTH, 3)]);
        }
        let mut h = HudTargetLite::default();
        for id in [corpse, item] {
            assert_eq!(h.0.click_identities(&mut z, &[id], Default::default()), None);
            assert_eq!(z.selected_target(), Some(id));
            assert_eq!(z.target, None, "objects must never enter character combat consumers");
            let selected = selected_info(&z).unwrap();
            assert_eq!((selected.kind, selected.health, selected.color), (id.kind, 0.3, 0xffffff));
            let fighting = info(&z, 2).unwrap();
            assert_eq!(control_targets(Some(&selected), Some(&fighting), false), [Some((id.instance, "Selection", false)), None]);
            assert_eq!(control_targets(Some(&selected), Some(&fighting), true), [Some((2, "Fighting Target", true)), None]);
            assert_eq!(h.0.click_identities(&mut z, &[], Default::default()), None);
            assert_eq!(z.selected_target(), Some(id), "ground clicks keep the object selected");
            assert_eq!(h.0.click_identities(&mut z, &[id], ao_gui::Modifiers { shift: true, ..Default::default() }), Some(WorldClick::ObjectInfo(id)));
            h.0.select_self(&mut z);
            assert_eq!(z.target, Some(1));
            h.0.select_self(&mut z);
            assert_eq!(z.selected_target(), Some(id), "select-self restores complete object identity");
        }
        assert_eq!(h.0.click_identities(&mut z, &[item, corpse], Default::default()), None);
        assert_eq!(z.selected_target(), Some(corpse), "overlapping objects cycle in merged hit order");
        z.reset_world();
        assert_eq!(z.selected_target(), None);
    }

    #[test]
    fn info_and_cycling() {
        let mut z = Zone::new(1);
        z.stats.insert(stats::SIDE, 1);
        z.dynels.insert(1, dyn_at("Me", [0.0; 3], false, 1));
        z.dynels.insert(2, dyn_at("Wolf", [0.0, 0.0, 5.0], true, 0));
        z.dynels.insert(3, dyn_at("Boar", [0.0, 0.0, 9.0], true, 0));
        z.dynels.insert(4, dyn_at("Friend", [0.0, 0.0, 3.0], false, 1));
        let i = info(&z, 2).unwrap();
        assert!(i.hostile && (i.health - 0.5).abs() < 1e-6 && i.level == 5);
        assert!(!info(&z, 4).unwrap().hostile);
        assert_eq!(cycle(&z, true, true), Some(2));
        z.target = Some(2);
        assert_eq!(cycle(&z, true, true), Some(3));
        z.target = Some(3);
        assert_eq!(cycle(&z, true, true), Some(2));
        assert_eq!(cycle(&z, true, false), Some(2));
        assert_eq!(cycle(&z, false, true), Some(4));
    }

    /// `N3Msg_GetCloseTarget`: `d² − d²(current)`, `+1000` when negative, only within 100 m, any other-side character is hostile.
    #[test]
    fn close_target_order_follows_the_original_metric() {
        let mut z = Zone::new(1);
        z.stats.insert(stats::SIDE, 1);
        z.dynels.insert(1, dyn_at("Me", [0.0; 3], false, 1));
        z.dynels.insert(2, dyn_at("A", [0.0, 0.0, 5.0], true, 0));
        z.dynels.insert(3, dyn_at("B", [0.0, 0.0, 9.0], true, 0));
        z.dynels.insert(5, dyn_at("C", [0.0, 0.0, 40.0], true, 0));
        z.dynels.insert(6, dyn_at("Out of range", [0.0, 0.0, 150.0], true, 0));
        z.dynels.insert(7, dyn_at("Omni player", [0.0, 0.0, 20.0], false, 2));
        // nothing selected: forward = nearest, backward = farthest in range
        assert_eq!(cycle(&z, true, true), Some(2));
        assert_eq!(cycle(&z, true, false), Some(5));
        // the next farther one (the player of the other side is hostile too)
        z.target = Some(3);
        assert_eq!(cycle(&z, true, true), Some(7));
        assert_eq!(cycle(&z, true, false), Some(5), "backwards takes the largest metric: the nearer ones wrapped by +1000 lose to C (1519)");
        // from the farthest the wrap (+1000 once) keeps the nearer ones negative: the smallest is the nearest
        z.target = Some(5);
        assert_eq!(cycle(&z, true, true), Some(2));
    }

    #[test]
    fn target_of_target_click_rule() {
        let mut z = Zone::new(1);
        for (id, name) in [(1, "Me"), (2, "Wolf"), (3, "Boar")] {
            z.dynels.insert(id, dyn_at(name, [0.0; 3], id > 1, 0));
        }
        assert!(!can_click_target_target(&z, 2, 3), "two plain dynels");
        assert!(can_click_target_target(&z, 2, 1), "the own character is not plain");
        assert!(can_click_target_target(&z, 1, 2));
        z.stats.insert(TOWER_TYPE, 1);
        assert!(!can_click_target_target(&z, 1, 2), "a TowerType on the own character refuses");
        assert!(can_click_target_target(&z, 2, 1));
        assert!(!can_click_target_target(&z, 2, 9), "unknown dynel");
    }

    #[test]
    fn select_self_toggles_back() {
        let mut z = Zone::new(1);
        z.dynels.insert(1, dyn_at("Me", [0.0; 3], false, 1));
        z.dynels.insert(2, dyn_at("Wolf", [0.0, 0.0, 5.0], true, 0));
        let mut gui_less = HudTargetLite::default();
        gui_less.0.select(&mut z, Some(2));
        gui_less.0.select_self(&mut z);
        assert_eq!(z.target, Some(1));
        gui_less.0.select_self(&mut z);
        assert_eq!(z.target, Some(2));
    }



    /// The target controls over the control centre, rendered through `ao_render::Offscreen` (client install only;
    /// PNG written when `AOMAC_SHOT_DIR` is set).
    struct Fe {
        gui: Gui,
        ht: HudTarget,
        zone: Zone,
    }

    impl ao_render::Frontend for Fe {
        fn gui(&self) -> &Gui {
            &self.gui
        }
        fn input(&mut self, ev: InputEvent, _host: &mut ao_render::Host) {
            self.ht.input(&self.gui, &mut self.zone, &ev);
            self.gui.input(ev);
        }
        fn frame(&mut self, dt: f32, size: (u32, u32), _host: &mut ao_render::Host) -> ao_gui::DrawList {
            self.ht.resize(&mut self.gui, size);
            self.ht.update(&mut self.gui, &mut self.zone, dt);
            self.gui.frame(dt)
        }
    }

    fn fe(size: (u32, u32)) -> Option<Fe> {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return None;
        }
        let mut gui = Gui::new(&dir, None).ok()?;
        let cc = gui.open_window("ControlCenter", (0, 0), WindowSize::Fixed(size.0, size.1)).ok()?;
        gui.apply_criteria(cc, &|_, _| Some(1));
        let ht = HudTarget::new(&mut gui, cc, size).ok()?;
        let mut zone = Zone::new(1);
        zone.stats.insert(stats::SIDE, 1);
        zone.dynels.insert(1, dyn_at("Aomacvolk", [0.0; 3], false, 1));
        zone.dynels.insert(2, DynelState { health: 45, max_health: 60, ..dyn_at("Surf Lizard", [0.0, 0.0, 5.0], true, 0) });
        Some(Fe { gui, ht, zone })
    }

    #[test]
    fn moved_target_bars_survive_updates_and_resize() {
        let Some(mut fe) = fe((1280, 800)) else { return };
        let ids: Vec<_> = fe.ht.persistent_windows().map(|(id, _)| id).collect();
        for (i, id) in ids.iter().enumerate() {
            fe.gui.set_window_pos(*id, (200 + i as i32 * 220, 90));
            fe.gui.set_window_moved(*id, true);
        }
        fe.zone.target = Some(2);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        fe.ht.resize(&mut fe.gui, (1920, 1080));
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(fe.gui.window_pos(*id), (200 + i as i32 * 220, 90));
        }
    }

    #[test]
    fn target_bar_window_adopts_live_width_and_visible_rows() {
        let Some(mut fe) = fe((1280, 800)) else { return };
        fe.zone.target = Some(2);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        let w = fe.ht.bars[1].window;
        let single = fe.gui.window_size(w);
        let bar = fe.gui.view_rect(w, "bar").unwrap();
        let pos = fe.gui.window_pos(w);
        assert_eq!(bar.t, pos.1 as f32, "a collapsed second row must not centre the first row below its native origin");
        assert!(bar.l >= pos.0 as f32 && bar.r < pos.0 as f32 + single.0 as f32, "the live bar width must fit its window");
        fe.zone.dynels.insert(3, dyn_at("Boar", [0.0, 0.0, 9.0], true, 0));
        fe.zone.fight_target.insert(1, 3);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert!(fe.gui.window_size(w).1 > single.1);
        fe.zone.fight_target.remove(&1);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.gui.window_size(w), single);
    }

    #[test]
    fn target_windows_follow_the_selection() {
        let size = (1280, 800);
        let Some(mut fe) = fe(size) else { return };
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert!(fe.ht.bars.iter().all(|b| b.shown.is_none()));
        // a hostile NPC fills the right window, the friendly one stays hidden; positions follow `CreateTargetMenus`
        fe.zone.target = Some(2);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        let (fr, ho) = (&fe.ht.bars[0], &fe.ht.bars[1]);
        assert!(fr.shown.is_none() && ho.shown.as_ref().is_some_and(|i| i.name == "Surf Lizard"));
        assert!(!fe.gui.window_visible(fr.window) && fe.gui.window_visible(ho.window));
        assert!(fe.gui.view_rect(ho.window, "header").is_none(), "top control has no dock header box");
        let bar = fe.gui.view_rect(ho.window, "bar").unwrap();
        let name = fe.gui.view_rect(ho.window, "name").unwrap();
        assert!(name.t > bar.b, "name is directly below the health bar");
        assert!(bar.width() < 150.0, "small-health targets use the derived active width, not the screen maximum");
        assert!(fe.gui.view_rect(fe.ht.cc, "ht_header0").is_some(), "caption/frame belongs above the dock");
        let (w, _) = fe.gui.window_size(ho.window);
        let sw = size.0 as f32;
        assert_eq!(fe.gui.window_pos(ho.window), ((((sw - 192.0) * 0.5 * 1.5) - w as f32 * 0.5).floor() as i32, 5));
        let (friendly_width, _) = fe.gui.window_size(fr.window);
        assert_eq!(fe.gui.window_pos(fr.window).0, (((sw - 192.0) * 0.5 * 0.5) - friendly_width as f32 * 0.5).floor() as i32);
        // the dynel leaves: the target and the window go
        fe.zone.dynels.remove(&2);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.zone.target, None);
        assert!(!fe.gui.window_visible(fe.ht.bars[1].window));
        let corpse = Identity { kind: 0xc76a, instance: 2 };
        fe.zone.world.test_prop(corpse, vec![(stats::LIFE, 10), (stats::HEALTH, 0)]);
        fe.zone.set_target(Some(corpse));
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.ht.bars[0].rows[0].as_ref().map(|(i, caption, _)| (i.kind, *caption, i.color)), Some((corpse.kind, "Selection", 0xffffff)));
        assert!(fe.gui.window_visible(fe.ht.bars[0].window));
        assert!(!fe.gui.window_visible(fe.ht.bars[1].window));
        fe.zone.world.clear();
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.zone.selected_target(), None);
        assert!(!fe.gui.window_visible(fe.ht.bars[0].window));
    }

    /// `FUN_10073b9e` / `LAB_10073554`: the button needs the `Targetstarget` pref and a shown hostile window; a click selects the
    /// target's target when `CanClickTargetTarget` holds (here the own character).
    #[test]
    fn target_of_target_button_follows_the_pref_and_selects() {
        let Some(mut fe) = fe((1280, 800)) else { return };
        fe.zone.target = Some(2);
        fe.zone.fight_target.insert(2, 1);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.ht.tot, None, "Targetstarget defaults to false");
        fe.ht.targets_target = true;
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.ht.tot, Some(1));
        let r = fe.gui.view_rect(fe.ht.bars[1].window, "tot").unwrap();
        assert!(r.r > r.l && r.b > r.t);
        let (x, y) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        for ev in [InputEvent::MouseDown { x, y, button: MouseButton::Left }, InputEvent::MouseUp { x, y, button: MouseButton::Left }] {
            assert_eq!(fe.ht.input(&fe.gui, &mut fe.zone, &ev), None, "not a world click");
        }
        assert_eq!(fe.zone.target, Some(1));
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.ht.tot, None, "the hostile window is gone with its target");
    }

    #[test]
    fn target_screenshot() {
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(std::path::PathBuf::from) else { return };
        let Some(mut fe) = fe((1280, 800)) else { return };
        std::fs::create_dir_all(&out).unwrap();
        let mut o = ao_render::Offscreen::new(&fe, (1280, 800)).unwrap();
        fe.zone.dynels.insert(3, DynelState { health: 60, max_health: 60, ..dyn_at("Aomacvolk's friend", [0.0, 0.0, 3.0], false, 1) });
        for (name, t) in [("target-hostile", 2), ("target-friendly", 3)] {
            fe.zone.target = Some(t);
            o.frame(&mut fe, 0.016);
            let list = o.frame(&mut fe, 0.016);
            o.png(&fe, &list, &out.join(format!("{name}.png"))).unwrap();
        }
    }

    /// `HudTarget` without windows, for the state-machine tests.
    struct HudTargetLite(HudTarget);
    impl Default for HudTargetLite {
        fn default() -> Self {
            HudTargetLite(HudTarget { cc: 0, size: (0, 0), bars: vec![], docks: [Dock { hostile: false }, Dock { hostile: true }], last: None, mouse: (0.0, 0.0), pressed: None, world_down: None, targets_target: false, bars_enabled: [true; 2], tot: None, tot_down: false, attack: false, info: None })
        }
    }
}
