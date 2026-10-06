//! The shortcut bar (hotbar): `ShortcutBarWindow_c` (GUI.dll 0x100d94e9, style 3 window, flags 0x183c) holding a `ShortcutBarView_c`
//! (0x100d8c12): HLayout of the bar-select radio button (`GFX_GUI_RADIOBUTTON_*`, ids 0x159 / 0x15a), the toolbar control
//! (`GFX_GUI_TOOLBAR_CONTROL_H` 0x1b2 with the up / down buttons 0x1b0 / 0x1aa) and a 10 x 1 `MultiListView` of 32 px slots.
//! Slot contents, icons and interaction: docs/gui.md §10 "Shortcut bar".

use ao_gui::view::CanvasItem;
use ao_gui::{Event, Gui, GfxId, InputEvent, MouseButton, WindowId, WindowSize};
use std::collections::HashMap;
use std::path::Path;

/// `MultiListView_c::SetViewCellCounts(10, 1)` (0x100d8c12).
const SLOTS: usize = 10;
/// Grid cell pitch: 32 px icon + `SetGridIconSpacing(4, 4)` (`_DAT_101b0840` = 4.0); the slot art `GFX_GUI_MULTILISTVIEW_SLOT_32_CLOSED`
/// is 38 x 38 (3 px border) and is centred on the 32 px icon, so neighbouring slots overlap by 2 px. The saved frames of
/// `prefs/NewChar/Containers/ShortcutBar_*.xml` (402 x 37 inclusive = 403 x 38) = radio 11 + control 32 + 10 x 36.
const PITCH: f32 = 36.0;
const RADIO_W: f32 = 11.0;
const CONTROL_W: f32 = 32.0;
const CHROME_W: f32 = RADIO_W + CONTROL_W;
const BAR_W: f32 = CHROME_W + PITCH * SLOTS as f32;
const BAR_H: f32 = 38.0;
/// Icon size (`MultiListView_c::SetGridIconSize`, 32 px) and its offset inside the 36 px cell (the 38 px art is centred on it).
const ICON: f32 = 32.0;
const ICON_X: f32 = 2.0;
const ICON_Y: f32 = 3.0;
/// Pixels the mouse must move after the press before a slot is dragged (`CanvasClick` uses the same 3 px for a click).
const DRAG_THRESHOLD: f32 = 3.0;

/// `Identity_t` kinds of slot items (`FUN_100d78e5`): 6 = special action, 7 = text macro.
const KIND_SPECIAL_ACTION: u32 = 0xdeb0;
const KIND_MACRO: u32 = 0xc789;
/// rdb 1010008 (`e_RDB_Res_Icon`, `GuiResourceManager_t::GetRDBTexture` [IF 0x1000b6e7]); the item / action templates are rdb 1000020
/// (read by `ao_formats::dynel_visual::item_template`).
const ICON_TYPE: u32 = 1_010_008;
/// Stat `Icon` (0x4f): `FUN_1003eb30` reads it from the identity's template for every slot type except macros (GFX 0xe3).
const STAT_ICON: u32 = 0x4f;
/// Stat `Action` (59) of an action template: its `Action_e` (see [`special_action`]).
const STAT_ACTION: u32 = 59;

/// The first-login slots (`FUN_100d94e9` after `IsFirstTime`): `(slot, identity kind, instance)`; the macro instance is the id
/// `TextMacroSystem_t::CreateMacro("Follow", "/follow", 0, true)` returns (the new-character template's `TextMacro.bin` holds
/// exactly this macro as id 1).
const FIRST_LOGIN: [(usize, u32, u32); 5] = [
    (0, KIND_SPECIAL_ACTION, 0xc1a5),
    (1, KIND_SPECIAL_ACTION, 0xc1a2),
    (2, KIND_SPECIAL_ACTION, 0xc1a8),
    (3, KIND_MACRO, 1),
    (9, KIND_SPECIAL_ACTION, 0x14124),
];
const FOLLOW_MACRO: (&str, &str) = ("Follow", "/follow");

/// What activating a slot asks the game for (`FUN_100d79c9`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SlotUse {
    /// `N3Msg_PerformSpecialAction(identity)` [GC 0x100272fd] -> `FUN_1004256c`: the `Action_e` of the slot's action template.
    SpecialAction(u32),
    /// A text macro: the macro text is emitted on GlobalSignals +0x180 and runs as if typed into the chat input.
    Macro(String),
}

/// `Action_e` of the perk / skill special actions (templates 0x14120.. of rdb 1000020). Their stat 59 is the default 0x11 (shared by
/// Fling Shot ... Tutor), i.e. not the action; the character's special-action list (`FUN_1003f121`, `+0x84`, server-filled, in no
/// capture) carries the real `Action_e`. UNRESOLVED except by name: "Suspended Animation" is camping, `FUN_1004256c` `0x51`
/// (docs/zone/combat-net.md §5.3).
pub(super) fn perk_action(instance: u32) -> Option<u32> {
    (instance == 0x14124).then_some(0x51)
}

#[derive(Clone, Debug)]
struct Slot {
    kind: u32,
    instance: u32,
    /// `N3Msg_GetName` (item template name, macro name): the tooltip title.
    name: String,
    icon: Option<(GfxId, u32, u32)>,
    /// Macro text (`TextMacro_t+0x24`).
    text: String,
    /// `Action_e` of a special action.
    action: Option<u32>,
}

struct Drag {
    from: usize,
    slot: Slot,
    ghost: WindowId,
}

pub(super) struct ShortcutBar {
    pub(super) window: WindowId,
    primary: bool,
    slots: [Option<Slot>; SLOTS],
    /// Left button went down on this slot at `(x, y)`.
    press: Option<(usize, f32, f32)>,
    drag: Option<Drag>,
    uses: Vec<SlotUse>,
}

/// `Containers/ShortcutBar_<n>.xml` `WindowFrame` of the install's new-character template.
fn template_origin(dir: &Path, n: usize) -> Option<(f32, f32)> {
    let t = std::fs::read_to_string(dir.join(format!("prefs/NewChar/Containers/ShortcutBar_{n}.xml"))).ok()?;
    let v = ao_gui::xml::parse(&t).ok()?;
    let r = v.children.iter().find(|c| c.attr("name") == Some("WindowFrame"))?.attr("value")?;
    let nums: Vec<f32> = r.trim_start_matches("Rect(").trim_end_matches(')').split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (nums.len() == 4).then(|| (nums[0], nums[1]))
}

/// `window / slot` XML: the painted `bar` canvas under a row of 36 x 38 `slotN` canvases (clicks, tooltips) with a label view
/// per slot for macro names.
fn window_xml(label_h: f32) -> String {
    let (w, h) = (BAR_W - 1.0, BAR_H - 1.0);
    let mut cells = String::new();
    for i in 0..SLOTS {
        // a `TextView` draws from the top of its frame, so the label sits in a vertical layout that centres its line-high frame
        cells += &format!(
            "<View view_layout=\"stacked\" min_size=\"Point({p},{h})\" max_size=\"Point({p},{h})\">\
             <CanvasView name=\"slot{i}\" min_size=\"Point({p},{h})\" max_size=\"Point({p},{h})\"/>\
             <View view_layout=\"vertical\"><TextView name=\"label{i}\" value=\"\" font=\"SMALL\" color=\"0xffff00\" h_alignment=\"center\" min_size=\"Point(0,{label_h})\" max_size=\"Point({p},{label_h})\"/></View></View>",
            p = PITCH - 1.0,
            h = h,
        );
    }
    format!(
        "<root><View view_layout=\"stacked\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\">\
         <CanvasView name=\"bar\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/>\
         <View view_layout=\"horizontal\" h_alignment=\"left\"><View min_size=\"Point({sp},{h})\" max_size=\"Point({sp},{h})\"/>{cells}</View></View></root>",
        sp = CHROME_W - 1.0,
    )
}

/// Slot contents of the first login with names and icons from the install's rdb (no fallbacks: a missing record leaves its slot empty).
fn first_login(gui: &mut Gui, dir: &Path) -> [Option<Slot>; SLOTS] {
    let mut slots: [Option<Slot>; SLOTS] = Default::default();
    let store = ao_rdb::RecordStore::open(dir).ok();
    for (i, kind, instance) in FIRST_LOGIN {
        slots[i] = match kind {
            KIND_MACRO => Some(Slot {
                kind,
                instance,
                name: FOLLOW_MACRO.0.to_string(),
                // `FUN_1003eb30(7)`: GFX_GUI_ICON_MACRO (0xe3)
                icon: gui.gfx_id("GFX_GUI_ICON_MACRO").map(GfxId).map(|g| (g, gui.gfx().size(g).0, gui.gfx().size(g).1)),
                text: FOLLOW_MACRO.1.to_string(),
                action: None,
            }),
            _ => store.as_ref().and_then(|s| special_action(gui, s, instance)),
        };
    }
    slots
}

/// `N3Msg_GetName` + stat `Icon` of the rdb 1000020 record `{0xF4254, instance}` (`GetItemByTemplate` maps kind 0xdeb0 to the item
/// template type, GC 0x17aae), icon image from rdb 1010008.
fn special_action(gui: &mut Gui, store: &ao_rdb::RecordStore, instance: u32) -> Option<Slot> {
    let t = ao_formats::dynel_visual::item_template(store, instance).ok()??;
    let icon = t.stat(STAT_ICON).filter(|&i| i > 0).and_then(|i| icon_image(gui, store, i as u32));
    // stat `Action` (59) of the base action templates is their `Action_e` (rdb 1000020: Use 3, Start/End Combat 0xb, Walk 0x11, Run 0x12,
    // Sneak 0x13, Sit 0x4c, Trade 0x4b, Stand 0x4d); the skill/perk templates 0x14120.. all carry the default 0x11 and are mapped by [`perk_action`]
    let action = perk_action(instance).or_else(|| t.stat(STAT_ACTION).and_then(|a| u32::try_from(a).ok()));
    Some(Slot { kind: KIND_SPECIAL_ACTION, instance, name: t.name.unwrap_or_default(), icon, text: String::new(), action })
}

/// rdb 1010008 PNG as a GUI texture (`Format_e 2`: pure green is the colour key, `SpriteInfo_t::ConvertImage` [DS 0x1007b8e2]).
fn icon_image(gui: &mut Gui, store: &ao_rdb::RecordStore, id: u32) -> Option<(GfxId, u32, u32)> {
    let png = store.get(ICON_TYPE, id).ok()??;
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut rgba = img.into_raw();
    for p in rgba.chunks_exact_mut(4) {
        if p[..3] == [0, 255, 0] {
            p[3] = 0;
        }
    }
    Some((gui.add_image("shortcut icon", rgba, w, h, true), w, h))
}

impl ShortcutBar {
    /// Bar `n` (the only default bar is bar 0, `NumHotbars` = 1 in `CharPrefs.xml`): primary; bar 0 gets the first-login contents
    /// (`IsFirstTime`, the block of `FUN_100d94e9` runs in the first constructed window).
    pub(super) fn new(gui: &mut Gui, dir: &Path, n: usize, screen: (u32, u32)) -> anyhow::Result<Self> {
        let label_h = gui.font_height(ao_gui::FontId::Small) as f32;
        let window = gui.open_window_xml("ShortcutBar", &window_xml(label_h), (0, 0), WindowSize::Preferred)?;
        let (x, y) = template_origin(dir, n).unwrap_or((20.0, 20.0));
        let slots = if n == 0 { first_login(gui, dir) } else { Default::default() };
        let mut bar = ShortcutBar { window, primary: n == 0, slots, press: None, drag: None, uses: vec![] };
        bar.place(gui, (x as i32, y as i32), screen);
        bar.paint(gui);
        Ok(bar)
    }

    /// `Window::MoveInsideScreen`.
    fn place(&mut self, gui: &mut Gui, (x, y): (i32, i32), screen: (u32, u32)) {
        let (w, h) = gui.window_size(self.window);
        gui.set_window_pos(self.window, (x.clamp(0, screen.0.saturating_sub(w) as i32), y.clamp(0, screen.1.saturating_sub(h) as i32)));
    }

    pub(super) fn resize(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        let p = gui.window_pos(self.window);
        self.place(gui, p, screen);
    }

    fn paint(&mut self, gui: &mut Gui) {
        let mut items = vec![];
        let mut put = |gui: &Gui, name: &str, x: f32, y: f32| {
            if let Some(g) = gui.gfx_id(name).map(GfxId) {
                let (w, h) = gui.gfx().size(g);
                items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x, y, x + w as f32, y + h as f32], alpha: 1.0 });
            }
        };
        // radio (`SetGfx(0, 0x15a)` unchecked, `SetGfx(1, 0x159)` checked: the primary bar shows the checked art), vertically centred
        put(gui, if self.primary { "GFX_GUI_RADIOBUTTON_CHECKED" } else { "GFX_GUI_RADIOBUTTON_UNCHECKED" }, 0.0, ((BAR_H - 11.0) / 2.0).floor());
        // control: background art + up / down buttons at x = 11 (`_DAT_101bee04`), 1 px from the top / bottom
        put(gui, "GFX_GUI_TOOLBAR_CONTROL_H", RADIO_W, 0.0);
        put(gui, "GFX_GUI_TOOLBAR_BUTTON_UP", RADIO_W + 11.0, 1.0);
        put(gui, "GFX_GUI_TOOLBAR_BUTTON_DOWN", RADIO_W + 11.0, BAR_H - 10.0 - 1.0);
        for i in 0..SLOTS {
            put(gui, "GFX_GUI_MULTILISTVIEW_SLOT_32_CLOSED", CHROME_W + i as f32 * PITCH - 1.0, 0.0);
        }
        // `FUN_1003f2ea`: the icon surface's destination frame is the item view's bounds (32 x 32), the source the whole image
        for (i, s) in self.slots.iter().enumerate() {
            let Some((g, w, h)) = s.as_ref().and_then(|s| s.icon) else { continue };
            let x = CHROME_W + i as f32 * PITCH + ICON_X;
            items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x, ICON_Y, x + ICON, ICON_Y + ICON], alpha: 1.0 });
        }
        gui.set_canvas(self.window, "bar", items);
        for (i, s) in self.slots.iter().enumerate() {
            // short tooltips (`ShortcutShortTooltips` = true in LoginPrefs.xml): the item name. UNRESOLVED: the long form.
            gui.set_tooltip(self.window, &format!("slot{i}"), s.as_ref().map_or("", |s| s.name.as_str()), "");
            // `FUN_1003f8e4`: macros show `<font color=yellow>name</font>` (font 7 = SMALL) centred over the macro icon
            gui.set_text(self.window, &format!("label{i}"), s.as_ref().filter(|s| s.kind == KIND_MACRO).map_or("", |s| s.name.as_str()));
        }
    }

    /// Slot under a screen position.
    fn slot_at(&self, gui: &Gui, x: f32, y: f32) -> Option<usize> {
        let (px, py) = gui.window_pos(self.window);
        let (lx, ly) = (x - px as f32 - CHROME_W, y - py as f32);
        (lx >= 0.0 && ly >= 0.0 && ly < BAR_H && lx < PITCH * SLOTS as f32).then(|| (lx / PITCH) as usize)
    }

    fn name_of(view: &str) -> Option<usize> {
        view.strip_prefix("slot")?.parse().ok().filter(|&i| i < SLOTS)
    }

    /// Use of a slot (`FUN_100d79c9`).
    fn activate(&mut self, i: usize) {
        let Some(s) = &self.slots[i] else { return };
        match s.kind {
            KIND_SPECIAL_ACTION => self.uses.extend(s.action.map(SlotUse::SpecialAction)),
            KIND_MACRO => self.uses.push(SlotUse::Macro(s.text.clone())),
            _ => {}
        }
    }

    /// A click on a slot canvas (`MultiListView` item click -> `FUN_100d79c9`). [INFERENCE] The use fires on the release of a
    /// press that did not drag; the DLL's click handler `FUN_10040a17` emits the "use" signal for a left press without shift when
    /// `UseNewMouseFunc` (default true) is set, the drag signal only after the mouse moved.
    pub(super) fn event(&mut self, ev: &Event) -> bool {
        match ev {
            Event::CanvasClick { window, view, .. } if *window == self.window => {
                if let Some(i) = Self::name_of(view) {
                    self.activate(i);
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Slot press / drag / drop. `locked` = `LockHotbars`. The drag follows `FUN_100d7ec1` (begin: not when locked), `FUN_100d8047`
    /// (drop on a slot of this bar: `MoveItem`) and `FUN_100d76cf` (after the drag ended the item is deleted if it still sits in its
    /// source slot: a drop outside the bar removes the shortcut).
    pub(super) fn input(&mut self, gui: &mut Gui, ev: &InputEvent, locked: bool) {
        match *ev {
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                self.press = self.slot_at(gui, x, y).filter(|&i| self.slots[i].is_some()).map(|i| (i, x, y));
            }
            InputEvent::MouseMove { x, y } => {
                if let Some(d) = &self.drag {
                    gui.set_window_pos(d.ghost, (x as i32 - ICON as i32 / 2, y as i32 - ICON as i32 / 2));
                } else if let Some((i, px, py)) = self.press {
                    if !locked && (x - px).hypot(y - py) > DRAG_THRESHOLD {
                        self.begin_drag(gui, i, x, y);
                    }
                }
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                self.press = None;
                if let Some(d) = self.drag.take() {
                    gui.close_window(d.ghost);
                    self.drop(gui, d, x, y);
                }
            }
            _ => {}
        }
    }

    fn begin_drag(&mut self, gui: &mut Gui, from: usize, x: f32, y: f32) {
        let Some(slot) = self.slots[from].clone() else { return };
        let src = "<root><CanvasView name=\"icon\" min_size=\"Point(31,31)\" max_size=\"Point(31,31)\"/></root>";
        let Ok(ghost) = gui.open_window_xml("ShortcutDrag", src, (x as i32 - 16, y as i32 - 16), WindowSize::Preferred) else { return };
        if let Some((g, w, h)) = slot.icon {
            gui.set_canvas(ghost, "icon", vec![CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, ICON, ICON], alpha: 1.0 }]);
        }
        self.drag = Some(Drag { from, slot, ghost });
    }

    /// `FUN_100d8047` / `FUN_100d76cf`. UNRESOLVED: dropping on an occupied slot, the original moves the item and starts a new drag
    /// with the displaced one; here the two swap places.
    fn drop(&mut self, gui: &mut Gui, d: Drag, x: f32, y: f32) {
        match self.slot_at(gui, x, y) {
            Some(to) if to == d.from => {}
            Some(to) => {
                self.slots[d.from] = self.slots[to].take();
                self.slots[to] = Some(d.slot);
            }
            None => self.slots[d.from] = None,
        }
        self.paint(gui);
    }

    #[cfg(test)]
    pub(super) fn slot_names(&self) -> Vec<String> {
        self.slots.iter().map(|s| s.as_ref().map_or(String::new(), |s| s.name.clone())).collect()
    }

    #[cfg(test)]
    pub(super) fn icon_count(&self) -> usize {
        self.slots.iter().flatten().filter(|s| s.icon.is_some()).count()
    }

    pub(super) fn take_uses(&mut self) -> Vec<SlotUse> {
        std::mem::take(&mut self.uses)
    }

    pub(super) fn close(self, gui: &mut Gui) {
        if let Some(d) = self.drag {
            gui.close_window(d.ghost);
        }
        gui.close_window(self.window);
    }
}

/// Hotbars of the first login: `NumHotbars` (CharPrefs.xml) = 1.
pub(super) fn default_count(dvalues: &HashMap<String, i64>) -> usize {
    dvalues.get("NumHotbars").copied().unwrap_or(1).clamp(1, 10) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_login_layout_and_actions() {
        assert_eq!(FIRST_LOGIN.iter().map(|s| s.0).collect::<Vec<_>>(), vec![0, 1, 2, 3, 9]);
        assert_eq!(perk_action(0x14124), Some(0x51));
        assert_eq!(perk_action(0xc1a8), None);
        assert_eq!(ShortcutBar::name_of("slot7"), Some(7));
        assert_eq!(ShortcutBar::name_of("slot10"), None);
        assert_eq!(ShortcutBar::name_of("bar"), None);
    }

    /// The first-login slots carry the `Action_e` of their rdb templates: Start Combat 0xb, Walk 0x11, Sit 0x4c, Suspended Animation camp 0x51.
    #[test]
    fn first_login_actions_come_from_the_templates() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let slots = first_login(&mut gui, &dir);
        let actions: Vec<_> = slots.iter().flatten().map(|s| s.action).collect();
        assert_eq!(actions, [Some(0xb), Some(0x11), Some(0x4c), None, Some(0x51)]);
    }

    /// The slot canvases sit on the painted 36 px grid (the same cells the icons are drawn in) and each macro label is centred in its
    /// slot (the label is centred over the macro icon, `FUN_1003f8e4`).
    #[test]
    fn slot_views_follow_the_painted_grid() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let bar = ShortcutBar::new(&mut gui, &dir, 0, (1280, 800)).unwrap();
        for i in 0..SLOTS {
            let (s, l) = (gui.view_frame(bar.window, &format!("slot{i}")).unwrap(), gui.view_frame(bar.window, &format!("label{i}")).unwrap());
            assert!((s.t + s.b - l.t - l.b).abs() <= 1.0 && (s.l + s.r - l.l - l.r).abs() <= 1.0, "label{i} {l:?} centred in slot{i} {s:?}");
            assert_eq!(s.l, CHROME_W + i as f32 * PITCH, "slot{i} x");
        }
    }
}
