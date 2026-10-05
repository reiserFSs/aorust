//! Target selection and the target controls of the in-world interface (docs/gui.md §11, docs/zone.md §target).
//!
//! Original: the selection is client-local state. `TargetingModule_t::SetTarget` (GUI.dll 0x100257b0) stores the identity in
//! `InputConfig_t+0xc0` (`SetCurrentTarget` 0x10019df0), broadcasts it on the AFCM bus (`Send(0x13, 0x126, id)`; `N3InterfaceModule_t::
//! SetTargetMessage` Interfaces 0x1000907a forwards it to `n3EngineClientAnarchy_t::N3Msg_SelectedTarget`, which only points the
//! camera's `SetSelectedTarget`), creates the selection `Indicator_t` (a ground marker unless the target has skill-flag 0x400) and
//! raises the `OnSelecting*` tips. **No message is sent to the server** on selection (no outgoing `TargetMessage` exists in
//! `docs/zone/outgoing.md`). `TargetingModule_t::FrameProcess` 0x10025fa4 drops the target when the dynel is gone
//! (`N3Msg_GetPos` fails or it has a parent) unless it was forced.
//!
//! The pick: the GUI hands the normalised mouse position to `N3Msg_SetMousePos` (Gamecode 0x1001613b) → `n3Camera_t::SetMousePos`
//! (N3 0x10020571): ray direction `(tan(fov/2)·x, tan(fov/2)/aspect·y, 1)` rotated by the camera, length `VisualCamera_t::
//! GetLengthOfViewcone`, cast into the collision world; the hit's identity is "the object under the mouse"
//! (`InputConfig_t::CheckObjectUnderMouse` 0x10019f00 decides the pointer from it). The collision meshes of the dynels are not
//! available here, so [`pick`] intersects the ray with one capsule per dynel (UNRESOLVED GUESS: [`CAPSULE_HEIGHT`], [`CAPSULE_RADIUS`]).

use super::zone::{scene_pos, DynelState, Zone};
use ao_formats::stats;
use ao_gui::{Gui, InputEvent, MouseButton, WindowId, WindowSize};
use ao_render::{Camera, Vec3};
use ao_scene::Lens;
use std::collections::HashMap;

/// Pick capsule of a character: feet to head (m). UNRESOLVED GUESS (the original ray-casts the collision mesh).
pub const CAPSULE_HEIGHT: f32 = 1.8;
/// Pick capsule radius (m). UNRESOLVED GUESS, slightly generous so that a click on the body always hits.
pub const CAPSULE_RADIUS: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

/// `n3Camera_t::SetMousePos` 0x10020571: the ray through pixel `mouse` of a `vp` sized viewport. `Lens::fov` is the horizontal
/// angle for the original engine (`RCamera_t` takes `tan(fov/2)` as the half width of the view plane at distance 1).
pub fn pick_ray(cam: &Camera, lens: &Lens, vp: (f32, f32), mouse: (f32, f32)) -> Ray {
    let aspect = vp.0 / vp.1;
    let (nx, ny) = (mouse.0 / vp.0 * 2.0 - 1.0, 1.0 - mouse.1 / vp.1 * 2.0);
    let half_w = if lens.horizontal { (lens.fov * 0.5).tan() } else { (lens.fov * 0.5).tan() * aspect };
    let dir = cam.forward() + cam.right() * (half_w * nx) + cam.up() * (half_w / aspect * ny);
    Ray { origin: cam.pos, dir: dir.normalize() }
}

/// Distance along the ray (`dir` normalised) to the capsule `a`–`b` of `radius`, if it is hit in front of the origin.
pub fn ray_capsule(ray: &Ray, a: Vec3, b: Vec3, radius: f32) -> Option<f32> {
    let (u, v, w0) = (ray.dir, b - a, ray.origin - a);
    let (bb, c, d, e) = (u.dot(v), v.dot(v), u.dot(w0), v.dot(w0));
    let denom = c - bb * bb;
    // closest points of the ray (s >= 0) and the segment (t in 0..=1)
    let mut t = if denom > 1e-6 { ((e - bb * d) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let mut s = (t * bb - d).max(0.0);
    if c > 1e-9 {
        t = ((e + s * bb) / c).clamp(0.0, 1.0);
        s = (t * bb - d).max(0.0);
    }
    let dist = (ray.origin + u * s - (a + v * t)).length();
    (dist <= radius).then_some(s)
}

/// Capsule (feet, head) of a dynel in scene space.
pub fn capsule(d: &DynelState) -> (Vec3, Vec3) {
    let foot = Vec3::from(scene_pos(d.pos));
    (foot + Vec3::Y * CAPSULE_RADIUS, foot + Vec3::Y * (CAPSULE_HEIGHT - CAPSULE_RADIUS))
}

/// The nearest dynel hit by `ray` (instance id).
pub fn pick(ray: &Ray, dynels: &HashMap<i32, DynelState>) -> Option<i32> {
    dynels
        .iter()
        .filter_map(|(id, d)| {
            let (a, b) = capsule(d);
            ray_capsule(ray, a, b, CAPSULE_RADIUS).map(|s| (s, *id))
        })
        .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)))
        .map(|p| p.1)
}

/// What the target controls show about the selected dynel (`N3Msg_GetName` / `GetSkill` of the target).
#[derive(Clone, Debug, PartialEq)]
pub struct TargetInfo {
    pub id: i32,
    pub name: String,
    pub level: i32,
    /// `Health / MaxHealth`, 0..1.
    pub health: f32,
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
        (zone.stat(stats::HEALTH).unwrap_or(d.health), zone.stat(stats::LIFE).unwrap_or(d.max_health), zone.stat(stats::LEVEL).unwrap_or(d.level), d.side)
    } else {
        (d.health, d.max_health, d.level, d.side)
    };
    let own_side = zone.stat(stats::SIDE).map_or_else(|| zone.own().map_or(0, |o| o.side), |s| s as u8);
    Some(TargetInfo {
        id,
        name: clean(&d.name),
        level,
        health: if max > 0 { (health as f32 / max as f32).clamp(0.0, 1.0) } else { 0.0 },
        hostile: d.npc && !is_self && side != own_side,
        is_self,
    })
}

/// `N3Msg_GetCloseTarget(current, friendly, forward)` (Gamecode, used by `TargetingModule_t::Get{Next,Prev}{Friendly,Hostile}
/// TargetMessage` 0x10025a52…): [INFERENCE] the candidates are ordered by distance from the own character, the next / previous one
/// after the current target wraps around.
pub fn cycle(zone: &Zone, hostile: bool, forward: bool) -> Option<i32> {
    let me = zone.own()?;
    let dist = |d: &DynelState| {
        let (a, b) = (Vec3::from(d.pos), Vec3::from(me.pos));
        a.distance(b)
    };
    let mut cands: Vec<(f32, i32)> = zone
        .dynels
        .iter()
        .filter(|(id, _)| **id != zone.char_id as i32)
        .filter(|(id, _)| info(zone, **id).is_some_and(|i| i.hostile == hostile))
        .map(|(id, d)| (dist(d), *id))
        .collect();
    cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let n = cands.len();
    if n == 0 {
        return None;
    }
    let cur = zone.target.and_then(|t| cands.iter().position(|c| c.1 == t));
    let i = match (cur, forward) {
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    };
    Some(cands[i].1)
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
    last: Option<i32>,
    mouse: (f32, f32),
    pressed: Option<(bool, Part)>,
    /// Left button went down on the world (not on the GUI) at this position: a click is a press + release close together.
    world_down: Option<(f32, f32)>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl HudTarget {
    /// Creates the two health-bar windows (`CCFriendlyHealthBar` / `CCHostileHealthBar`) and fills the control-centre target docks.
    pub(super) fn new(gui: &mut Gui, cc: WindowId, size: (u32, u32)) -> anyhow::Result<Self> {
        let mut t = HudTarget { cc, size, bars: vec![], docks: [Dock { hostile: false }, Dock { hostile: true }], last: None, mouse: (0.0, 0.0), pressed: None, world_down: None };
        t.create_bars(gui)?;
        for (dock, d) in [("LeftTargetCtrlDock", &t.docks[0]), ("RightTargetCtrlDock", &t.docks[1])] {
            let src = format!(
                "<root><View view_layout=\"horizontal\">\
                 <CanvasView name=\"{p}\" min_size=\"Point(7,40)\" max_size=\"Point(7,40)\" layout_borders=\"Rect(0,0,5,0)\"/>\
                 <CanvasView name=\"{b}\" min_size=\"Point(47,47)\" max_size=\"Point(47,47)\"/>\
                 <CanvasView name=\"{n}\" min_size=\"Point(7,40)\" max_size=\"Point(7,40)\" layout_borders=\"Rect(5,0,0,0)\"/>\
                 </View></root>",
                p = d.view(Part::Prev),
                b = d.view(Part::Button),
                n = d.view(Part::Next)
            );
            gui.add_view_xml(cc, dock, dock, &src)?;
        }
        Ok(t)
    }

    fn bar_width(size: (u32, u32)) -> f32 {
        ((size.0 as f32 - SIDE_MARGIN) * 0.5 - BAR_MARGIN).floor().max(64.0)
    }

    fn create_bars(&mut self, gui: &mut Gui) -> anyhow::Result<()> {
        let w = Self::bar_width(self.size);
        // `TargetHeader_c` (`FUN_10073884`): corner-only `BorderView` (GFX_GUI_CC_TARGET_FRAME_TL/TR/BL/BR = 0x88,0x89,0x86,0x87),
        // colour DEFAULT, a bold title and the name below. Its width is the widest of the four captions.
        let cap = ["Selection", "Nano Target", "Fighting Target", "Nano / Fighting"].iter().map(|c| gui.text_width(ao_gui::FontId::Bold, c)).max().unwrap_or(0);
        for hostile in [false, true] {
            let src = format!(
                "<root><View view_layout=\"vertical\">\
                 <CanvasView name=\"bar\" min_size=\"Point({bw},10)\" max_size=\"Point({bw},10)\"/>\
                 <BorderView name=\"header\" view_layout=\"vertical\" min_size=\"Point({hw},0)\" tl_gfx=\"GFX_GUI_CC_TARGET_FRAME_TL\" tr_gfx=\"GFX_GUI_CC_TARGET_FRAME_TR\" \
                 bl_gfx=\"GFX_GUI_CC_TARGET_FRAME_BL\" br_gfx=\"GFX_GUI_CC_TARGET_FRAME_BR\" left_gfx=\"\" top_gfx=\"\" right_gfx=\"\" bottom_gfx=\"\" color=\"DEFAULT\" layout_borders=\"Rect(0,0,0,10)\">\
                 <TextView name=\"title\" font=\"BOLD\" value=\"{title}\" layout_borders=\"Rect(8,5,8,0)\"/>\
                 <TextView name=\"name\" font=\"NORMAL\" value=\"\" layout_borders=\"Rect(8,0,8,4)\"/>\
                 </BorderView></View></root>",
                bw = w as i32 - 1,
                hw = cap + 16,
                title = esc("<center>Selection</center>")
            );
            let name = if hostile { "CCHostileHealthBar" } else { "CCFriendlyHealthBar" };
            let window = gui.open_window_xml(name, &src, (0, 5), WindowSize::Preferred)?;
            gui.set_window_visible(window, false);
            self.bars.push(Bar { window, hostile, width: w, shown: None });
        }
        self.place(gui);
        Ok(())
    }

    fn place(&mut self, gui: &mut Gui) {
        let sw = self.size.0 as f32;
        for b in &self.bars {
            let (ww, _) = gui.window_size(b.window);
            let k = if b.hostile { 1.5 } else { 0.5 };
            let x = ((sw - SIDE_MARGIN) * 0.5 * k - ww as f32 * 0.5).floor();
            gui.set_window_pos(b.window, (x as i32, 5));
        }
    }

    pub(super) fn resize(&mut self, gui: &mut Gui, size: (u32, u32)) {
        if size != self.size {
            self.size = size;
            let w = Self::bar_width(size);
            for b in &mut self.bars {
                b.width = w;
            }
            self.place(gui);
        }
    }

    /// `SetTarget`: remembers the previous selection for `SelectSelf` and stores the new one.
    pub(super) fn select(&mut self, zone: &mut Zone, id: Option<i32>) {
        if zone.target != id {
            if let Some(old) = zone.target {
                self.last = Some(old);
            }
            zone.target = id;
        }
    }

    /// `TargetingModule_t::SelectSelf` 0x100259b1: no target → select self; target is self → back to the previous target.
    fn select_self(&mut self, zone: &mut Zone) {
        let me = zone.char_id as i32;
        match zone.target {
            None => self.select(zone, Some(me)),
            Some(t) if t == me => {
                let back = self.last.filter(|l| zone.dynels.contains_key(l) && *l != me);
                self.select(zone, back);
            }
            Some(_) => self.select(zone, Some(me)),
        }
    }

    /// A left click on the world (not on the GUI): the dynel under the mouse becomes the target. A click on empty ground leaves
    /// the selection alone: nothing in `TargetingModule_t` removes it except `FrameProcess` (dynel gone) and `RemoveTarget`
    /// (UNRESOLVED: the original's click-to-deselect, if any, was not found; the input bindings of GUI.dll's default
    /// hotkeys list contain no mouse command for it).
    pub(super) fn world_click(&mut self, zone: &mut Zone, cam: &Camera, lens: &Lens, vp: (u32, u32), mouse: (f32, f32)) -> bool {
        let ray = pick_ray(cam, lens, (vp.0 as f32, vp.1 as f32), mouse);
        match pick(&ray, &zone.dynels) {
            Some(id) => {
                self.select(zone, Some(id));
                true
            }
            None => false,
        }
    }

    /// Raw mouse input before the GUI: dock buttons (hit-tested on their canvases) and world clicks. Returns `Some(pos)` when a
    /// world click (press and release at nearly the same pixel, the pointer not over the GUI) completed.
    pub(super) fn input(&mut self, gui: &Gui, zone: &mut Zone, ev: &InputEvent) -> Option<(f32, f32)> {
        match *ev {
            InputEvent::MouseMove { x, y } => self.mouse = (x, y),
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                self.mouse = (x, y);
                self.pressed = self.dock_at(gui, x, y);
                self.world_down = (self.pressed.is_none() && !gui.wants_mouse(x, y)).then_some((x, y));
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                self.mouse = (x, y);
                let press = self.pressed.take();
                if let Some((hostile, part)) = press.filter(|p| self.dock_at(gui, x, y) == Some(*p)) {
                    self.dock_click(zone, hostile, part);
                    return None;
                }
                if let Some((dx, dy)) = self.world_down.take() {
                    if (dx - x).abs() <= 4.0 && (dy - y).abs() <= 4.0 && !gui.wants_mouse(x, y) {
                        return Some((x, y));
                    }
                }
            }
            _ => {}
        }
        None
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

    /// Arrows: previous / next friendly or hostile target (the four `TargetingModule_t::Get{Next,Prev}…TargetMessage` handlers;
    /// [INFERENCE] bound to the dock arrows). Friendly button: `SelectSelf`. Hostile button: UNRESOLVED (`FUN_1007303d` only toggles
    /// its enabled state with a `GlobalSignals` value).
    fn dock_click(&mut self, zone: &mut Zone, hostile: bool, part: Part) {
        match part {
            Part::Prev | Part::Next => {
                if let Some(id) = cycle(zone, hostile, part == Part::Next) {
                    self.select(zone, Some(id));
                }
            }
            Part::Button if !hostile => self.select_self(zone),
            Part::Button => {}
        }
    }

    /// Tab / Shift+Tab / Ctrl+Tab (`COMMAND_NEXT_HOSTILE_TARGET` … in GUI.dll's default hotkeys: `TAB`, `SHIFT + TAB`,
    /// `CTRL + TAB`, `CTRL + SHIFT + TAB`).
    pub(super) fn key(&mut self, zone: &mut Zone, key: ao_gui::Key, mods: ao_gui::Modifiers) -> bool {
        if key != ao_gui::Key::Tab {
            return false;
        }
        if let Some(id) = cycle(zone, !mods.ctrl, !mods.shift) {
            self.select(zone, Some(id));
        }
        true
    }

    /// `TargetingModule_t::FrameProcess`: the target goes away with its dynel; then the controls follow the selection.
    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, _dt: f32) {
        if zone.target.is_some_and(|t| !zone.dynels.contains_key(&t)) {
            zone.target = None;
        }
        let sel = zone.target.and_then(|t| info(zone, t));
        for b in &mut self.bars {
            let mine = sel.clone().filter(|i| i.hostile == b.hostile);
            if mine != b.shown {
                gui.set_window_visible(b.window, mine.is_some());
                if let Some(i) = &mine {
                    gui.set_text(b.window, "name", &format!("<center><font color=0xffffff>{}</font></center>", i.name));
                    gui.relayout_window(b.window);
                }
                b.shown = mine;
            }
            let mut items = vec![];
            if let Some(i) = &b.shown {
                bar_items(b.width as i32, i.health, &mut items);
            }
            gui.set_canvas(b.window, "bar", items);
        }
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
/// (DEFAULT) and the slider over the background up to `ratio` of its width; the slider texture (16 px) repeats along the bar (its
/// source frame is the destination size). Red caps (`0xff2222`, `FUN_10072e49` when `+0x161`/`+0x162` is set) are UNRESOLVED:
/// the writers of those flags were not identified.
fn bar_items(width: i32, ratio: f32, out: &mut Vec<ao_gui::view::CanvasItem>) {
    use ao_gui::view::CanvasItem::ImageTint;
    use ao_gui::GfxId;
    let (cap_w, h) = (9.0, 11.0);
    let w = width as f32;
    let default = 0x1000000;
    out.push(ImageTint { id: GfxId(HB_LEFT), src: [0.0, 0.0, cap_w, h], dst: [0.0, 0.0, cap_w, h], color: default, alpha: 1.0 });
    out.push(ImageTint { id: GfxId(HB_RIGHT), src: [0.0, 0.0, cap_w, h], dst: [w - cap_w, 0.0, w, h], color: default, alpha: 1.0 });
    let (l, r) = (cap_w, w - cap_w);
    out.push(ImageTint { id: GfxId(HB_BACKGROUND), src: [0.0, 0.0, 16.0, h], dst: [l, 0.0, r, h], color: default, alpha: 1.0 });
    let fill = ((r - l) * ratio).floor();
    let mut x = 0.0;
    while x < fill {
        let tw = (fill - x).min(16.0);
        out.push(ao_gui::view::CanvasItem::Image { id: GfxId(HB_SLIDER), src: [0.0, 0.0, tw, h], dst: [l + x, 0.0, l + x + tw, h], alpha: 1.0 });
        x += 16.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn capsule_hits_and_misses() {
        let ray = Ray { origin: Vec3::new(0.0, 1.0, 10.0), dir: Vec3::new(0.0, 0.0, -1.0) };
        let (a, b) = (Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.3, 0.0));
        assert!((ray_capsule(&ray, a, b, 0.5).unwrap() - 10.0).abs() < 1e-4);
        assert!(ray_capsule(&Ray { origin: Vec3::new(0.7, 1.0, 10.0), ..ray }, a, b, 0.5).is_none());
        // behind the origin
        assert!(ray_capsule(&Ray { dir: Vec3::new(0.0, 0.0, 1.0), ..ray }, a, b, 0.5).is_none());
        // above the capsule top
        assert!(ray_capsule(&Ray { origin: Vec3::new(0.0, 2.5, 10.0), ..ray }, a, b, 0.5).is_none());
    }

    #[test]
    fn pick_takes_the_nearest_and_world_click_selects() {
        let mut z = Zone::new(1);
        // server z is mirrored: scene z = -server z, so these stand at scene z = -5 and z = -12
        z.dynels.insert(1, dyn_at("Me", [20.0, 0.0, 0.0], false, 1));
        z.dynels.insert(2, dyn_at("Near", [0.0, 0.0, 5.0], true, 2));
        z.dynels.insert(3, dyn_at("Far", [0.0, 0.0, 12.0], true, 2));
        let cam = Camera::look_at(Vec3::new(0.0, 1.2, 1.0), Vec3::new(0.0, 1.2, -10.0));
        let lens = Lens::default();
        let ray = pick_ray(&cam, &lens, (800.0, 600.0), (400.0, 300.0));
        assert_eq!(pick(&ray, &z.dynels), Some(2));
        // a click at the far left of the screen misses everything
        let ray = pick_ray(&cam, &lens, (800.0, 600.0), (5.0, 300.0));
        assert_eq!(pick(&ray, &z.dynels), None);
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
        let (w, _) = fe.gui.window_size(ho.window);
        let sw = size.0 as f32;
        assert_eq!(fe.gui.window_pos(ho.window), ((((sw - 192.0) * 0.5 * 1.5) - w as f32 * 0.5).floor() as i32, 5));
        assert_eq!(fe.gui.window_pos(fr.window).0, (((sw - 192.0) * 0.5 * 0.5) - w as f32 * 0.5).floor() as i32);
        // the dynel leaves: the target and the window go
        fe.zone.dynels.remove(&2);
        fe.ht.update(&mut fe.gui, &mut fe.zone, 0.0);
        assert_eq!(fe.zone.target, None);
        assert!(!fe.gui.window_visible(fe.ht.bars[1].window));
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
            HudTargetLite(HudTarget { cc: 0, size: (0, 0), bars: vec![], docks: [Dock { hostile: false }, Dock { hostile: true }], last: None, mouse: (0.0, 0.0), pressed: None, world_down: None })
        }
    }
}
