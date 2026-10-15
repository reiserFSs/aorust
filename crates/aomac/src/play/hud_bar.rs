//! The shortcut bar (hotbar): `ShortcutBarWindow_c` (GUI.dll 0x100d94e9, style 3 window, flags 0x183c) holding a `ShortcutBarView_c`
//! (0x100d8c12): HLayout of the bar-select radio button (`GFX_GUI_RADIOBUTTON_*`, ids 0x159 / 0x15a), the toolbar control
//! (`GFX_GUI_TOOLBAR_CONTROL_H` 0x1b2 with the up / down buttons 0x1b0 / 0x1aa) and a 10 x 1 `MultiListView` of 32 px slots.
//! Slot contents, icons and interaction: docs/gui.md §10 "Shortcut bar".

use ao_gui::view::CanvasItem;
use super::hud_special::SpecialList;
use ao_gui::{Gui, GfxId, InputEvent, MouseButton, WindowId, WindowSize};
use std::collections::HashMap;
use std::path::Path;

/// `MultiListView_c::SetViewCellCounts(10, 1)` (0x100d8c12).
const SLOTS: usize = 10;
/// Rows of slots a bar holds. The use / row handlers address a slot as `n + row * 10` (`FUN_100d7cef` [GUI 0x100d7cef]) and the row keys
/// `SHORTUCT_BAR_ROW_0..9` scroll the list to row `n` (`FUN_100d85fb`: `Scroll(-(cell height * n))`). UNRESOLVED GUESS: the row count 10 (the
/// ctor `FUN_100d8c12` sets the view cell count (10, 1) and never calls `SetMaxItemCount`; the content height that clamps the scroll was not
/// traced), taken from the ten row providers.
const ROWS: usize = 10;
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
/// `MouseButtonHoldDelay` (`LoginPrefs.xml` default 0.3 s): how long a slot must be held before it is
/// dragged (`FUN_100410a0` starts the `EventTimer_c` at slot +0x340 with it, its timer slot `FUN_1004173a` emits the drag-begin signal).
const HOLD_DELAY: f32 = 0.3;

/// `Identity_t` kinds of slot items (`FUN_100d78e5`): 6 = special action, 7 = text macro.
const KIND_SPECIAL_ACTION: u32 = 0xdeb0;
const KIND_MACRO: u32 = 0xc789;
/// rdb 1010008 (`e_RDB_Res_Icon`, `GuiResourceManager_t::GetRDBTexture` [IF 0x1000b6e7]); the item / action templates are rdb 1000020
/// (read by `ao_formats::dynel_visual::item_template`).
const ICON_TYPE: u32 = 1_010_008;
/// Stat `Icon` (0x4f): `FUN_1003eb30` reads it from the identity's template for every slot type except macros (GFX 0xe3).
const STAT_ICON: u32 = 0x4f;

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
    /// `N3Msg_PerformSpecialAction(identity)` [GC 0x100272fd] -> `FUN_1004256c`: the `Action_e` the slot's entry in the special-action list shows.
    SpecialAction(u32),
    /// The slot's identity is in no list entry: `Feedback_ActionIsNotAvailable` (`FUN_1004256c` first branch).
    Unavailable,
    /// A text macro: the macro text is emitted on GlobalSignals +0x180 and runs as if typed into the chat input.
    Macro(String),
    /// Inventory item identity used by `N3Msg_UseItem` (GUI `FUN_100d79c9`, type 1).
    Item(u32),
    /// Nano identity of a type 4 shortcut.
    Nano(i32),
}

#[derive(Clone, Debug)]
pub(super) struct Slot {
    kind: u32,
    instance: u32,
    /// `N3Msg_GetName` (item template name, macro name): the tooltip title.
    pub(super) name: String,
    pub(super) icon: Option<(GfxId, u32, u32)>,
    /// Macro text (`TextMacro_t+0x24`).
    text: String,
}

struct Drag {
    /// Source slot; `None` for a new macro dragged from the chat.
    from: Option<usize>,
    slot: Slot,
    ghost: WindowId,
    button: MouseButton,
}

pub(super) struct ShortcutBar {
    pub(super) window: WindowId,
    primary: bool,
    /// The visible row.
    slots: [Option<Slot>; SLOTS],
    /// Every row; the entry of the visible row is empty (its slots are in `slots`, see [`Self::set_row`]).
    rows: Vec<[Option<Slot>; SLOTS]>,
    row: usize,
    /// A captured slot press; the hold interval and button are sampled when the timer is armed.
    press: Option<Press>,
    drag: Option<Drag>,
    uses: Vec<SlotUse>,
    store: Option<ao_rdb::RecordStore>,
    /// Per slot: the recharge of the action it shows `(remaining / total, seconds)` (`FUN_1003d3ff`), painted by [`Self::paint`].
    timers: [Option<(f32, i32)>; SLOTS],
    mouse: (f32, f32),
    /// Slot data of the special-action templates shown so far (the icon textures are made once).
    icons: HashMap<u32, Slot>,
    /// DValue `IconTimerText` (`CharPrefs.xml` default true): the recharge seconds are printed over the icon.
    timer_text: bool,
    pub(super) hold_delay: f32,
    pub(super) new_mouse: bool,
    tooltip_key: Option<(usize, u32, u32, bool)>,
    tooltip_body: String,
    info_urls: Vec<String>,
    released: Option<(Slot, f32, f32)>,
    item_templates: HashMap<u32, (i32, super::hud_stats::ItemInfo)>,
}

struct Press {
    slot: usize,
    held: f32,
    button: MouseButton,
    delay: Option<f32>,
}

/// `window / slot` XML: the painted `bar` canvas under a row of 36 x 38 `slotN` canvases (clicks, tooltips) with a label view
/// per slot for macro names and a bold `timerN` view for the recharge seconds (`FUN_1003d3ff` / `FUN_1003efe6`: `<center><font color=yellow>`, font 6 = BOLD).
fn window_xml(label_h: f32, timer_h: f32) -> String {
    let (w, h) = (BAR_W - 1.0, BAR_H - 1.0);
    let mut cells = String::new();
    for i in 0..SLOTS {
        // a `TextView` draws from the top of its frame, so the label sits in a vertical layout that centres its line-high frame
        cells += &format!(
            "<View view_layout=\"stacked\" min_size=\"Point({p},{h})\" max_size=\"Point({p},{h})\">\
             <CanvasView name=\"slot{i}\" min_size=\"Point({p},{h})\" max_size=\"Point({p},{h})\"/>\
             <View view_layout=\"vertical\"><TextView name=\"label{i}\" value=\"\" font=\"SMALL\" color=\"0xffff00\" h_alignment=\"center\" min_size=\"Point(0,{label_h})\" max_size=\"Point({p},{label_h})\"/></View>\
             <View view_layout=\"vertical\"><TextView name=\"timer{i}\" value=\"\" font=\"BOLD\" color=\"0xffff00\" h_alignment=\"center\" min_size=\"Point(0,{timer_h})\" max_size=\"Point({p},{timer_h})\"/></View></View>",
            p = PITCH - 1.0,
            h = h,
        );
    }
    // Native page centre is spinner-local 19 (`101beda0` f64), not 16: a 6px left border shifts the preferred label centre by 3.
    format!(
        "<root><View view_layout=\"stacked\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\">\
         <CanvasView name=\"bar\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\"/>\
         <View view_layout=\"horizontal\" h_alignment=\"left\"><View min_size=\"Point({radio},{h})\" max_size=\"Point({radio},{h})\"/>\
         <View view_layout=\"vertical\" h_alignment=\"center\" min_size=\"Point({control},{h})\" max_size=\"Point({control},{h})\"><TextView name=\"page\" value=\"1\" font=\"NORMAL\" color=\"0xffff00\" layout_borders=\"Rect(6,0,0,0)\"/></View>{cells}</View></View></root>",
        radio = RADIO_W - 1.0,
        control = CONTROL_W - 1.0,
    )
}

/// Slot contents of the first login with names and icons from the install's rdb (no fallbacks: a missing record leaves its slot empty).
fn first_login(gui: &mut Gui, store: Option<&ao_rdb::RecordStore>) -> [Option<Slot>; SLOTS] {
    let mut slots: [Option<Slot>; SLOTS] = Default::default();
    for (i, kind, instance) in FIRST_LOGIN {
        slots[i] = match kind {
            KIND_MACRO => Some(macro_slot(gui, instance, FOLLOW_MACRO.0, FOLLOW_MACRO.1)),
            _ => store.and_then(|s| special_action(gui, s, instance)),
        };
    }
    slots
}

/// A text macro slot `{0xc789, id}`: `FUN_1003eb30(7)` icon GFX_GUI_ICON_MACRO (0xe3), the macro name as label, the command as use text.
fn macro_slot(gui: &mut Gui, id: u32, name: &str, command: &str) -> Slot {
    let icon = gui.gfx_id("GFX_GUI_ICON_MACRO").map(GfxId).map(|g| (g, gui.gfx().size(g).0, gui.gfx().size(g).1));
    Slot { kind: KIND_MACRO, instance: id, name: name.to_string(), icon, text: command.to_string() }
}

/// `N3Msg_GetName` + stat `Icon` of the rdb 1000020 record `{0xF4254, instance}` (`GetItemByTemplate` maps kind 0xdeb0 to the item
/// template type, GC 0x17aae), icon image from rdb 1010008. The `Action_e` is the key of the template in `FUN_100428ad`'s map
/// (the list entry showing it, [`SpecialList::find`], names the `Action_e`; the records' stat 59 is only right for the base actions, the perk / skill templates 0x14120.. all hold the default 0x11).
pub(super) fn special_action(gui: &mut Gui, store: &ao_rdb::RecordStore, instance: u32) -> Option<Slot> {
    let t = ao_formats::dynel_visual::item_template(store, instance).ok()??;
    let icon = t.stat(STAT_ICON).filter(|&i| i > 0).and_then(|i| icon_image(gui, store, i as u32));
    Some(Slot { kind: KIND_SPECIAL_ACTION, instance, name: t.name.unwrap_or_default(), icon, text: String::new() })
}

/// rdb 1010008 PNG as a GUI texture (`Format_e 2`: pure green is the colour key, `SpriteInfo_t::ConvertImage` [DS 0x1007b8e2]).
pub(super) fn icon_image(gui: &mut Gui, store: &ao_rdb::RecordStore, id: u32) -> Option<(GfxId, u32, u32)> {
    let png = store.get(ICON_TYPE, id).ok()??;
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut rgba = img.into_raw();
    for p in rgba.as_chunks_mut::<4>().0 {
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
        let timer_h = gui.font_height(ao_gui::FontId::Bold) as f32;
        let window = gui.open_window_xml("ShortcutBar", &window_xml(label_h, timer_h), (0, 0), WindowSize::Preferred)?;
        // `ShortcutBarWindow_c` ctor (GUI 0x100d94e9): `Window(Rect(20.0, 20.0, 215.0, 300.0), style 3, flags 0x183c)` then `LoadWndConfig` (nothing saved on a first login);
        // the install's `prefs/NewChar/Containers/ShortcutBar_<n>.xml` frames (authored on a 2304 x 1440 screen) are not used, docs/gui.md 10.4.
        let (x, y) = (20, 20);
        let store = ao_rdb::RecordStore::open(dir).ok();
        let slots = if n == 0 { first_login(gui, store.as_ref()) } else { Default::default() };
        let mut bar = ShortcutBar { window, primary: n == 0, slots, rows: vec![Default::default(); ROWS], row: 0, press: None, drag: None, uses: vec![], store, timers: [None; SLOTS], mouse: (0.0, 0.0), icons: HashMap::new(), timer_text: true, hold_delay: HOLD_DELAY, new_mouse: true, tooltip_key: None, tooltip_body: String::new(), info_urls: vec![], released: None, item_templates: HashMap::new() };
        bar.place(gui, (x, y), screen);
        bar.paint(gui);
        Ok(bar)
    }

    /// Retail `FUN_100d8c12` restores paired item_id/item_pos entries; `FUN_100d7db1` writes the same archive.
    pub(super) fn load_contents(&mut self, gui: &mut Gui, client: &Path, character: &Path, n: usize) {
        let path = character.join(format!("Containers/ShortcutBar_{n}.xml"));
        let src = std::fs::read_to_string(&path).or_else(|err| {
            if n == 0 && err.kind() == std::io::ErrorKind::NotFound {
                std::fs::read_to_string(character.join("Containers/ShortcutBar.xml"))
            } else { Err(err) }
        });
        let result = (|| -> anyhow::Result<()> {
            let root = match src {
                Ok(src) => Some(ao_gui::xml::parse(&src)?),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => return Err(err.into()),
            };
            let entries = if let Some(map) = root.as_ref().and_then(|root| root.children.iter().find(|e| e.attr("name") == Some("position_map"))) {
                let ids: Vec<_> = map.children.iter().filter(|e| e.attr("name") == Some("item_id")).collect();
                let positions: Vec<_> = map.children.iter().filter(|e| e.attr("name") == Some("item_pos")).collect();
                anyhow::ensure!(ids.len() == positions.len(), "unpaired shortcut positions");
                let mut entries = Vec::with_capacity(ids.len());
                for (id, pos) in ids.into_iter().zip(positions) {
                    let [kind, instance] = archive_pair(id.attr("value"), "Identity(")?;
                    let [x, y] = archive_pair(pos.attr("value"), "IPoint(")?;
                    anyhow::ensure!(x < SLOTS as u32 && y < ROWS as u32, "shortcut position outside bar");
                    entries.push((kind, instance, x as usize, y as usize));
                }
                entries
            } else {
                match std::fs::read(character.join("IconPositions.bin")) {
                    Ok(bytes) => legacy_positions(&bytes)?,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(err) => return Err(err.into()),
                }
            };
            let mut nanos = super::hud_nanodb::NanoDb::new(client);
            let macros = super::chat::macros::TextMacros::open();
            self.slots = Default::default();
            self.rows.fill_with(Default::default);
            for (kind, instance, x, y) in entries {
                let slot = if kind == KIND_MACRO {
                    macros.get(instance as i32).map(|m| macro_slot(gui, instance, &m.name, &m.command))
                } else if kind as i32 == ao_net::n3::nano::NANO_KIND {
                    nanos.info(gui, instance as i32).map(|info| Slot { kind, instance, name: info.name.clone(), icon: info.icon, text: String::new() })
                } else if kind == KIND_SPECIAL_ACTION {
                    self.store.as_ref().and_then(|store| special_action(gui, store, instance))
                } else { None };
                let slot = slot.unwrap_or(Slot { kind, instance, name: String::new(), icon: None, text: String::new() });
                if y == self.row { self.slots[x] = Some(slot); } else { self.rows[y][x] = Some(slot); }
            }
            if let Some(primary) = root.as_ref().and_then(|root| root.children.iter().find(|e| e.attr("name") == Some("IsPrimary"))).and_then(|e| e.attr("value")) {
                self.primary = primary == "true";
            }
            if let Some(row) = root.as_ref().and_then(|root| root.children.iter().find(|e| e.attr("name") == Some("selected_bar"))).and_then(|e| e.attr("value")).and_then(|s| s.parse::<usize>().ok()).filter(|&r| r < ROWS) {
                self.set_row(gui, row);
            }
            self.paint(gui);
            Ok(())
        })();
        if let Err(err) = result { eprintln!("prefs: {}: {err}", path.display()); }
    }

    pub(super) fn save_contents(&self, character: &Path, n: usize) {
        use ao_gui::xml::Element;
        let path = character.join(format!("Containers/ShortcutBar_{n}.xml"));
        let result = (|| -> anyhow::Result<()> {
            let mut root = match std::fs::read_to_string(&path) {
                Ok(src) => ao_gui::xml::parse(&src)?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Element { name: "Archive".into(), attrs: vec![("code".into(), "0".into())], children: vec![] },
                Err(err) => return Err(err.into()),
            };
            let scalar = |tag: &str, name: &str, value: String| Element { name: tag.into(), attrs: vec![("name".into(), name.into()), ("value".into(), value)], children: vec![] };
            let mut map = Element { name: "Archive".into(), attrs: vec![("code".into(), "0".into()), ("name".into(), "position_map".into())], children: vec![] };
            let mut macros = super::chat::macros::TextMacros::open();
            for y in 0..ROWS {
                let slots = if y == self.row { &self.slots } else { &self.rows[y] };
                for (x, slot) in slots.iter().enumerate().filter_map(|(x, s)| s.as_ref().map(|s| (x, s))) {
                    map.children.push(scalar("Identity", "item_id", format!("Identity({},{})", slot.kind, slot.instance)));
                    map.children.push(scalar("IPoint", "item_pos", format!("IPoint({x},{y})")));
                    if slot.kind == KIND_MACRO && !slot.text.is_empty() { macros.create(&slot.name, &slot.text, slot.instance as i32, true); }
                }
            }
            root.children.retain(|e| !matches!(e.attr("name"), Some("position_map" | "selected_bar" | "IsPrimary")));
            root.children.extend([map, scalar("Int32", "selected_bar", self.row.to_string()), scalar("Bool", "IsPrimary", self.primary.to_string())]);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, super::dvalue::element_xml(&root))?;
            Ok(())
        })();
        if let Err(err) = result { eprintln!("prefs: {}: {err}", path.display()); }
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
        // `FUN_10040001` (item disabled): `GFX_GUI_ICONFADER32` (0xe2, a black 50 % stipple) tinted `0`, its source and destination frames shrunk
        // from both sides by `Width * 0.5 * (1 - progress)` (`FUN_1003efe6`): the stipple closes in on the centre while the action recharges
        for (i, t) in self.timers.iter().enumerate() {
            let (Some((p, _)), Some(g)) = (t, gui.gfx_id("GFX_GUI_ICONFADER32").map(GfxId)) else { continue };
            let x = CHROME_W + i as f32 * PITCH + ICON_X;
            let shrink = (ICON - 1.0) * 0.5 * (1.0 - p.clamp(0.0, 1.0));
            items.push(CanvasItem::ImageTint { id: g, src: [shrink, 0.0, ICON - 2.0 * shrink, ICON], dst: [x + shrink, ICON_Y, x + ICON - shrink, ICON_Y + ICON], color: 0, alpha: 1.0 });
        }
        gui.set_canvas(self.window, "bar", items);
        // Native spinner `FUN_100da8eb`: preferred-sized NORMAL (font 5) yellow label, rows 1..9,0.
        gui.set_text(self.window, "page", &((self.row + 1) % ROWS).to_string());
        for (i, s) in self.slots.iter().enumerate() {
            let body = if self.tooltip_key.is_some_and(|(slot, kind, instance, _)| slot == i && s.as_ref().is_some_and(|s| s.kind == kind && s.instance == instance)) { self.tooltip_body.as_str() } else { "" };
            gui.set_tooltip(self.window, &format!("slot{i}"), s.as_ref().map_or("", |s| s.name.as_str()), body);
            // `FUN_1003f8e4`: macros show `<font color=yellow>name</font>` (font 7 = SMALL) centred over the macro icon
            gui.set_text(self.window, &format!("label{i}"), s.as_ref().filter(|s| s.kind == KIND_MACRO).map_or("", |s| s.name.as_str()));
            // `FUN_1003eebe`: the remaining seconds (`IconTimerText`) in yellow bold, centred over the icon
            gui.set_text(self.window, &format!("timer{i}"), &self.timers[i].filter(|_| self.timer_text).map_or(String::new(), |(_, secs)| super::hud_special::timer_text(secs)));
        }
    }

    /// GUI `100d77b3` sets description mode to `!ShortcutShortTooltips`; `1004178f` additionally gates it with ItemDescriptionTooltips.
    pub(super) fn update_tooltip(&mut self, gui: &mut Gui, zone: &super::zone::Zone, stats: &super::hud_stats::HudStats, long: bool) {
        let key = self.slot_at(gui, self.mouse.0, self.mouse.1).and_then(|i| self.slots[i].as_ref().map(|s| (i, s.kind, s.instance, long)));
        if key == self.tooltip_key { return; }
        self.tooltip_key = key;
        let Some((i, kind, instance, long)) = key else { return };
        let slot = self.slots[i].as_ref().unwrap();
        self.tooltip_body = if kind == KIND_MACRO {
            slot.text.clone()
        } else if long {
            stats.tooltip_body(zone, ao_net::msg::Identity { kind: kind as i32, instance: instance as i32 })
        } else {
            String::new()
        };
        gui.set_tooltip(self.window, &format!("slot{i}"), &slot.name, &self.tooltip_body);
    }

    /// Resolve retained inventory identities against the current owned template.
    pub(super) fn refresh_items(&mut self, gui: &mut Gui, zone: &super::zone::Zone, stats: &mut super::hud_stats::HudStats) {
        let mut dirty = false;
        for slot in self.slots.iter_mut().chain(self.rows.iter_mut().flat_map(|row| row.iter_mut())).flatten() {
            if ao_net::n3::inventory::item_identity(slot.instance).kind as u32 != slot.kind { continue; }
            let Some(entry) = zone.inventory.get(&slot.instance) else { continue };
            if self.item_templates.get(&slot.instance).is_none_or(|(low, _)| *low != entry.item.low_id) {
                let Some(info) = stats.item_info(gui, entry.item.low_id) else { continue };
                self.item_templates.insert(slot.instance, (entry.item.low_id, info));
            }
            let (_, (name, icon)) = &self.item_templates[&slot.instance];
            if slot.name != *name || slot.icon != *icon {
                slot.name.clone_from(name);
                slot.icon = *icon;
                dirty = true;
            }
        }
        if dirty {
            self.tooltip_key = None;
            self.paint(gui);
        }
    }

    pub(super) fn take_info_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.info_urls)
    }

    /// Slot under a screen position.
    pub(super) fn slot_at(&self, gui: &Gui, x: f32, y: f32) -> Option<usize> {
        if !gui.window_visible(self.window) { return None; }
        let (px, py) = gui.window_pos(self.window);
        let (lx, ly) = (x - px as f32 - CHROME_W, y - py as f32);
        ((0.0..PITCH * SLOTS as f32).contains(&lx) && (0.0..BAR_H).contains(&ly)).then(|| (lx / PITCH) as usize)
    }

    /// Use of a slot (`FUN_100d79c9`): a special action is looked up by its identity in the character's list (`FUN_1004256c` via
    /// `N3Msg_PerformSpecialAction(identity)`): not listed = `Feedback_ActionIsNotAvailable`, else the action the entry shows.
    fn activate(&mut self, i: usize, list: &SpecialList) {
        let Some(s) = &self.slots[i] else { return };
        match s.kind {
            KIND_SPECIAL_ACTION => self.uses.push(list.find(s.instance).map_or(SlotUse::Unavailable, |e| SlotUse::SpecialAction(e.shown))),
            KIND_MACRO => self.uses.push(SlotUse::Macro(s.text.clone())),
            kind if kind as i32 == ao_net::n3::nano::NANO_KIND => self.uses.push(SlotUse::Nano(s.instance as i32)),
            kind if matches!(kind as i32, ao_net::n3::inventory::KIND_WEAPON_PAGE | ao_net::n3::inventory::KIND_ARMOR_PAGE | ao_net::n3::inventory::KIND_IMPLANT_PAGE | ao_net::n3::inventory::KIND_BAG | ao_net::n3::inventory::KIND_SOCIAL_PAGE) => self.uses.push(SlotUse::Item(s.instance)),
            _ => {}
        }
    }

    /// `SHORTUCT_BAR_ACTIVE_n` (digit keys, `FUN_100d85cf` -> `FUN_100d7cef`): slot `n` of the active bar's current row, on the key *press*.
    pub(super) fn use_slot(&mut self, n: usize, list: &SpecialList) {
        if n < SLOTS {
            self.activate(n, list);
        }
    }

    /// The radio button of the bar is checked: the digit keys act on this bar.
    pub(super) fn is_active(&self) -> bool {
        self.primary
    }

    /// `SHORTUCT_BAR_SELECT_n` (Alt + digit, `FUN_100d7d8f`): bar `n` becomes the active one (the radio button is checked).
    pub(super) fn set_primary(&mut self, gui: &mut Gui, on: bool) {
        if self.primary != on {
            self.primary = on;
            self.paint(gui);
        }
    }

    /// Per frame: the hold timer, the identities following their list entries, the recharge overlays (`FUN_1003d3ff`).
    /// `changes` = [`SpecialList::take_changes`] of this frame, `locked` = `LockHotbars`, `timers` = `IconTimers` / `IconTimerText`.
    pub(super) fn update(&mut self, gui: &mut Gui, dt: f32, list: &SpecialList, changes: &[(u32, u32)], locked: bool, timers: (bool, bool)) {
        // Spinner capture/MoveBy (`100db4d6` / `100db335`), not the slot list; LockHotbars gates movement.
        gui.set_window_move_region(self.window, (!locked).then(|| ao_gui::Rect::new(RADIO_W, 0.0, CHROME_W - 1.0, BAR_H - 1.0)));
        let mut dirty = self.timer_text != timers.1;
        self.timer_text = timers.1;
        for &(old, new) in changes {
            for r in 0..self.rows.len() {
                for i in 0..SLOTS {
                    if self.rows[r][i].as_ref().is_some_and(|s| s.kind == KIND_SPECIAL_ACTION && s.instance == old) {
                        self.rows[r][i] = self.icon_slot(gui, new);
                    }
                }
            }
            for i in 0..SLOTS {
                if self.slots[i].as_ref().is_some_and(|s| s.kind == KIND_SPECIAL_ACTION && s.instance == old) {
                    self.slots[i] = self.icon_slot(gui, new);
                    dirty = true;
                }
            }
        }
        // the hold: `EndMouseClick`, then the drag-begin signal (which does nothing while the bars are locked)
        if let Some(p) = &mut self.press {
            p.held += dt;
            if p.delay.is_some_and(|delay| p.held >= delay) {
                let (slot, button) = (p.slot, p.button);
                self.press = None;
                if !locked {
                    let (x, y) = self.mouse;
                    self.begin_drag(gui, slot, x, y, button);
                }
            }
        }
        let mut t = [None; SLOTS];
        for (i, s) in self.slots.iter().enumerate() {
            let Some(e) = s.as_ref().filter(|s| s.kind == KIND_SPECIAL_ACTION).and_then(|s| list.find(s.instance)) else { continue };
            // `IconTimers` off: the progress is a constant 1.0 (`FUN_1003ea09`), the stipple stays full
            t[i] = list.progress(e.shown).map(|(p, secs)| (if timers.0 { p } else { 1.0 }, secs));
        }
        if t != self.timers {
            self.timers = t;
            dirty = true;
        }
        if dirty {
            self.paint(gui);
        }
    }

    /// The slot of a special-action template (icon textures are cached per template).
    fn icon_slot(&mut self, gui: &mut Gui, instance: u32) -> Option<Slot> {
        if let Some(s) = self.icons.get(&instance) {
            return Some(s.clone());
        }
        let s = special_action(gui, self.store.as_ref()?, instance)?;
        self.icons.insert(instance, s.clone());
        Some(s)
    }

    /// Slot press / drag / drop with the client's "new mouse function" (`UseNewMouseFunc` default true, the hotbar item list sets its flag +0x319,
    /// `FUN_100d8c12` 0x100d8eb8). `MultiListView_c::MouseDown` 0x10133b63 captures the mouse and records the cell, `FUN_100410a0` (the down
    /// handler) arms the hold timer for a left press on an item; `MouseUp` 0x10133c28 emits the click only when the release is in the same cell
    /// (and the capture was still held, i.e. the timer had not fired), `FUN_10040a17`: `GetQualifiers() & 3` (Shift only, not Ctrl;
    /// masks in `FUN_100bc5cf`) -> the info signal (`FUN_100d765d`, itemid:// inspection except macros), else the **use** signal.
    /// Use therefore fires on the release, never on the press; a slot held for
    /// `MouseButtonHoldDelay` starts its drag without moving (see [`Self::update`]); the legacy mode instead arms the timer on a right press.
    /// In new mode the right-click signal is not connected by `ShortcutBarView_c`. The drag follows `FUN_100d7ec1` (begin: not when locked), `FUN_100d8047`
    /// (drop on a slot of this bar: `MoveItem`) and `FUN_100d76cf` (after the drag ended the item is deleted if it still sits in its
    /// source slot: a drop outside the bar removes the shortcut).
    pub(super) fn input(&mut self, gui: &mut Gui, ev: &InputEvent, list: &SpecialList, mods: ao_gui::Modifiers) {
        match *ev {
            InputEvent::MouseDown { x, y, .. } if gui.pointer_owner(x, y) != Some(self.window) => {}
            InputEvent::MouseDown { x, y, button: button @ (MouseButton::Left | MouseButton::Right) } => {
                self.mouse = (x, y);
                if let Some(d) = (button == MouseButton::Left).then(|| self.row_button(gui, x, y)).flatten() {
                    // HUD input precedes Gui::input for this same event: arrows page without capturing the window.
                    gui.set_window_move_region(self.window, None);
                    self.set_row(gui, (self.row as isize + d).rem_euclid(ROWS as isize) as usize);
                    return;
                }
                let drag_button = if self.new_mouse { MouseButton::Left } else { MouseButton::Right };
                self.press = self.slot_at(gui, x, y).filter(|&i| self.slots[i].is_some()).map(|slot| Press { slot, held: 0.0, button, delay: (button == drag_button).then_some(self.hold_delay) });
            }
            InputEvent::MouseMove { x, y } => {
                self.mouse = (x, y);
                if let Some(d) = &self.drag {
                    gui.set_window_pos(d.ghost, (x as i32 - ICON as i32 / 2, y as i32 - ICON as i32 / 2));
                }
            }
            InputEvent::MouseUp { x, y, button: button @ (MouseButton::Left | MouseButton::Right) } => {
                if self.drag.as_ref().is_some_and(|d| d.button != button) || self.press.as_ref().is_some_and(|p| p.button != button) { return; }
                if let Some(d) = self.drag.take() {
                    gui.close_window(d.ghost);
                    self.drop(gui, d, x, y);
                } else if let Some(p) = self.press.take() {
                    if button == MouseButton::Left && self.slot_at(gui, x, y) == Some(p.slot) {
                        if mods.shift {
                            if let Some(slot) = self.slots[p.slot].as_ref().filter(|s| s.kind != KIND_MACRO) {
                                self.info_urls.push(format!("itemid://{}/{}", slot.kind, slot.instance));
                            }
                        } else {
                            self.activate(p.slot, list);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The toolbar control's up / down buttons (`GFX_GUI_TOOLBAR_BUTTON_UP` / `_DOWN` at x = 11 of the control, 1 px from its top / bottom, see
    /// [`Self::paint`]): `-1` = previous row, `+1` = next row. Native arrow handlers (`100dae98..100daec5`) wrap
    /// with `(row + 9) % 10` / `(row + 1) % 10`, then call `100daa9e`.
    fn row_button(&self, gui: &Gui, x: f32, y: f32) -> Option<isize> {
        if !gui.window_visible(self.window) { return None; }
        let (px, py) = gui.window_pos(self.window);
        let (lx, ly) = (x - px as f32 - RADIO_W - 11.0, y - py as f32);
        let size = |n: &str| gui.gfx_id(n).map_or((0, 0), |g| gui.gfx().size(GfxId(g)));
        let (up, down) = (size("GFX_GUI_TOOLBAR_BUTTON_UP"), size("GFX_GUI_TOOLBAR_BUTTON_DOWN"));
        let hit = |s: (u32, u32), top: f32| lx >= 0.0 && lx < s.0 as f32 && ly >= top && ly < top + s.1 as f32;
        if hit(up, 1.0) {
            Some(-1)
        } else if hit(down, BAR_H - 10.0 - 1.0) {
            Some(1)
        } else {
            None
        }
    }

    /// `SHORTUCT_BAR_ROW_n` (`FUN_100d85fb`) / the up and down buttons: row `n` becomes the visible one.
    pub(super) fn set_row(&mut self, gui: &mut Gui, n: usize) {
        if n >= ROWS || n == self.row {
            return;
        }
        self.press = None;
        let shown = std::mem::take(&mut self.slots);
        self.rows[self.row] = shown;
        self.slots = std::mem::take(&mut self.rows[n]);
        self.row = n;
        self.timers = [None; SLOTS];
        self.paint(gui);
    }

    #[cfg(test)]
    pub(super) fn row(&self) -> usize {
        self.row
    }

    /// `FUN_100d8047` for an item dragged from the Actions window: the special action `instance` goes to the slot under the point.
    /// `true` when the drop landed on this bar.
    pub(super) fn drop_special(&mut self, gui: &mut Gui, instance: u32, x: f32, y: f32) -> bool {
        let Some(to) = self.slot_at(gui, x, y) else { return false };
        let Some(slot) = self.icon_slot(gui, instance) else { return false };
        self.replace_shortcut(gui, to, slot, x, y);
        self.paint(gui);
        true
    }

    /// An inventory drag creates a shortcut, not an inventory move (`FUN_100d8047`).
    pub(super) fn drop_item(&mut self, gui: &mut Gui, slot: u32, name: &str, icon: Option<(GfxId, u32, u32)>, x: f32, y: f32) -> bool {
        let Some(to) = self.slot_at(gui, x, y) else { return false };
        let id = ao_net::n3::inventory::item_identity(slot);
        self.replace_shortcut(gui, to, Slot { kind: id.kind as u32, instance: slot, name: name.to_owned(), icon, text: String::new() }, x, y);
        self.paint(gui);
        true
    }

    fn begin_drag(&mut self, gui: &mut Gui, from: usize, x: f32, y: f32, button: MouseButton) {
        let Some(slot) = self.slots[from].clone() else { return };
        self.start_drag(gui, Some(from), slot, x, y);
        if let Some(d) = &mut self.drag { d.button = button; }
    }

    /// Learned nano signal (`FUN_100d7999`): active bar, first free cell across all rows.
    pub(super) fn add_learned_nano(&mut self, gui: &mut Gui, id: i32, name: &str, icon: Option<(GfxId, u32, u32)>) {
        if !self.primary { return; }
        for row in 0..ROWS {
            let slots = if row == self.row { &mut self.slots } else { &mut self.rows[row] };
            if let Some(to) = slots.iter().position(Option::is_none) {
                slots[to] = Some(Slot { kind: ao_net::n3::nano::NANO_KIND as u32, instance: id as u32, name: name.to_owned(), icon, text: String::new() });
                if row == self.row { self.paint(gui); }
                return;
            }
        }
    }

    /// `NanoView_c::FUN_100d3807`: drag a known nano as an inventory/nano identity.
    pub(super) fn begin_nano_drag(&mut self, gui: &mut Gui, id: i32, name: &str, icon: Option<(GfxId, u32, u32)>) {
        let slot = Slot { kind: ao_net::n3::nano::NANO_KIND as u32, instance: id as u32, name: name.to_owned(), icon, text: String::new() };
        let (x, y) = self.mouse;
        self.start_drag(gui, None, slot, x, y);
    }

    /// `FUN_100d82d4` (`/macro`): the new macro `{0xc789, id}` is dragged from no slot; a drop on a slot puts it there, a drop elsewhere discards it.
    pub(super) fn begin_macro_drag(&mut self, gui: &mut Gui, id: i32, name: &str, command: &str) {
        let slot = macro_slot(gui, id as u32, name, command);
        let (x, y) = self.mouse;
        self.start_drag(gui, None, slot, x, y);
    }

    fn start_drag(&mut self, gui: &mut Gui, from: Option<usize>, slot: Slot, x: f32, y: f32) {
        if let Some(old) = self.drag.take() {
            gui.close_window(old.ghost);
        }
        let src = "<root><CanvasView name=\"icon\" min_size=\"Point(31,31)\" max_size=\"Point(31,31)\"/></root>";
        let Ok(ghost) = gui.open_window_xml("ShortcutDrag", src, (x as i32 - 16, y as i32 - 16), WindowSize::Preferred) else { return };
        if let Some((g, w, h)) = slot.icon {
            gui.set_canvas(ghost, "icon", vec![CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [0.0, 0.0, ICON, ICON], alpha: 1.0 }]);
        }
        self.drag = Some(Drag { from, slot, ghost, button: MouseButton::Left });
    }

    fn replace_shortcut(&mut self, gui: &mut Gui, to: usize, slot: Slot, x: f32, y: f32) {
        let macro_drop = slot.kind == KIND_MACRO;
        let displaced = self.slots[to].replace(slot);
        if let Some(slot) = displaced.filter(|_| !macro_drop) {
            self.start_drag(gui, None, slot, x, y);
        }
    }

    pub(super) fn take_drop(&mut self) -> Option<(Slot, f32, f32)> {
        self.released.take()
    }

    pub(super) fn accept_drop(&mut self, gui: &mut Gui, to: usize, slot: Slot, x: f32, y: f32) {
        self.replace_shortcut(gui, to, slot, x, y);
        self.paint(gui);
    }

    /// Native `100d8047`: re-drag displaced shortcuts; macro drops delete their occupant.
    fn drop(&mut self, gui: &mut Gui, d: Drag, x: f32, y: f32) {
        let to = self.slot_at(gui, x, y);
        if to == d.from && to.is_some() { return; }
        if let Some(from) = d.from { self.slots[from] = None; }
        if let Some(to) = to {
            self.replace_shortcut(gui, to, d.slot, x, y);
        } else {
            self.released = Some((d.slot, x, y));
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

#[cfg(test)]
impl ShortcutBar {
    /// Live harness: screen position of the centre of slot `n` of the current row.
    pub(super) fn slot_screen_pos(&self, gui: &Gui, n: usize) -> (f32, f32) {
        let (wx, wy) = gui.window_pos(self.window);
        (wx as f32 + CHROME_W + PITCH * n as f32 + PITCH / 2.0, wy as f32 + BAR_H / 2.0)
    }
}

/// Native IconPositionSystem_t archive, GUI 1001e53a / 1001e148.
fn legacy_positions(bytes: &[u8]) -> anyhow::Result<Vec<(u32, u32, usize, usize)>> {
    let mut words = bytes.chunks_exact(4);
    anyhow::ensure!(words.remainder().is_empty(), "unaligned IconPositions.bin");
    let mut next = || -> anyhow::Result<u32> {
        Ok(u32::from_be_bytes(words.next().ok_or_else(|| anyhow::anyhow!("truncated IconPositions.bin"))?.try_into()?))
    };
    anyhow::ensure!(next()? as usize == bytes.len(), "IconPositions.bin size mismatch");
    let maps = next()? as usize;
    anyhow::ensure!(maps <= bytes.len().saturating_sub(8) / 16, "invalid icon map count");
    let mut entries = Vec::new();
    for _ in 0..maps {
        let (owner_kind, owner_id, map_type, count) = (next()?, next()?, next()?, next()? as usize);
        anyhow::ensure!(count <= bytes.len() / 12, "invalid icon position count");
        for _ in 0..count {
            let (pos, kind, instance) = (next()?, next()?, next()?);
            if (owner_kind, owner_id, map_type) == (0, 0, 6) {
                anyhow::ensure!((pos as usize) < SLOTS * ROWS, "shortcut position outside bar");
                entries.push((kind, instance, pos as usize % SLOTS, pos as usize / SLOTS));
            }
        }
    }
    anyhow::ensure!(words.next().is_none(), "trailing IconPositions.bin data");
    Ok(entries)
}

fn archive_pair(value: Option<&str>, prefix: &str) -> anyhow::Result<[u32; 2]> {
    let body = value.and_then(|v| v.strip_prefix(prefix)).and_then(|v| v.strip_suffix(')')).ok_or_else(|| anyhow::anyhow!("invalid shortcut value"))?;
    let (a, b) = body.split_once(',').ok_or_else(|| anyhow::anyhow!("invalid shortcut pair"))?;
    Ok([a.trim().parse()?, b.trim().parse()?])
}

/// Hotbars of the first login: `NumHotbars` (CharPrefs.xml) = 1.
pub(super) fn default_count(dvalues: &super::dvalue::DValues) -> usize {
    dvalues.get_i64("NumHotbars").unwrap_or(1).clamp(1, 10) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_shortcut_fallback_defers_to_xml_position_map() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() { return; }
        let character = std::env::temp_dir().join(format!("aomac-shortcut-legacy-{}", std::process::id()));
        std::fs::create_dir_all(character.join("Containers")).unwrap();
        let words: [u32; 9] = [36, 1, 0, 0, 6, 1, 14, KIND_SPECIAL_ACTION, 0xc1a2];
        let bytes: Vec<_> = words.into_iter().flat_map(u32::to_be_bytes).collect();
        std::fs::write(character.join("IconPositions.bin"), bytes).unwrap();
        let mut gui = Gui::new(&client, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &client, 1, (1280, 800)).unwrap();
        bar.load_contents(&mut gui, &client, &character, 1);
        assert_eq!(bar.rows[1][4].as_ref().unwrap().instance, 0xc1a2);
        std::fs::write(character.join("Containers/ShortcutBar_1.xml"), "<Archive><Archive name=\"position_map\"/></Archive>").unwrap();
        bar.load_contents(&mut gui, &client, &character, 1);
        assert!(bar.rows.iter().flatten().all(Option::is_none), "an empty XML map overrides legacy contents");
        bar.close(&mut gui);
        std::fs::remove_dir_all(character).unwrap();
    }

    #[test]
    fn legacy_positions_validate_and_select_native_shortcut_map() {
        let words: [u32; 19] = [76, 2, 0, 5, 5, 1, 0, KIND_SPECIAL_ACTION, 0xc1a5, 0, 0, 6, 2, 14, KIND_SPECIAL_ACTION, 0xc1a2, 99, KIND_SPECIAL_ACTION, 0xc1a8];
        let bytes: Vec<_> = words.into_iter().flat_map(u32::to_be_bytes).collect();
        assert_eq!(legacy_positions(&bytes).unwrap(), [(KIND_SPECIAL_ACTION, 0xc1a2, 4, 1), (KIND_SPECIAL_ACTION, 0xc1a8, 9, 9)]);
        for n in 0..bytes.len() { assert!(legacy_positions(&bytes[..n]).is_err()); }
        let mut malformed = bytes.clone();
        malformed[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(legacy_positions(&malformed).is_err());
        malformed = bytes.clone();
        malformed[52..56].copy_from_slice(&100u32.to_be_bytes());
        assert!(legacy_positions(&malformed).is_err());
    }

    #[test]
    fn persisted_inventory_shortcuts_refresh_every_row_after_template_replacement() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() { return; }
        let character = std::env::temp_dir().join(format!("aomac-shortcut-items-{}", std::process::id()));
        std::fs::create_dir_all(character.join("Containers")).unwrap();
        let slot = ao_net::n3::inventory::BAG_FIRST;
        let id = ao_net::n3::inventory::item_identity(slot);
        std::fs::write(character.join("Containers/ShortcutBar_1.xml"), format!("<Archive><Archive name=\"position_map\"><Identity name=\"item_id\" value=\"Identity({},{slot})\"/><IPoint name=\"item_pos\" value=\"IPoint(0,0)\"/><Identity name=\"item_id\" value=\"Identity({},{slot})\"/><IPoint name=\"item_pos\" value=\"IPoint(1,2)\"/></Archive></Archive>", id.kind, id.kind)).unwrap();
        let mut gui = Gui::new(&client, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &client, 1, (1280, 800)).unwrap();
        bar.load_contents(&mut gui, &client, &character, 1);
        assert!(bar.slots[0].as_ref().unwrap().name.is_empty());
        let mut stats = super::super::hud_stats::HudStats::new(&client, (1280, 800)).unwrap();
        let mut zone = super::super::zone::Zone::new(7);
        for low_id in [0xc1a5, 0xc1a2] {
            zone.inventory.insert(slot, ao_net::n3::world::InventoryEntry { slot, a: 0, b: 1, id, item: ao_net::n3::world::AcgItem { low_id, high_id: low_id, level: 1 } });
            let expected = stats.item_info(&mut gui, low_id).unwrap();
            bar.refresh_items(&mut gui, &zone, &mut stats);
            for actual in [&bar.slots[0], &bar.rows[2][1]] {
                let actual = actual.as_ref().unwrap();
                assert_eq!((&actual.name, actual.icon), (&expected.0, expected.1));
            }
        }
        bar.close(&mut gui);
        std::fs::remove_dir_all(character).unwrap();
    }

    #[test]
    fn occupied_shortcut_redrags_displaced_item_except_macros() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &dir, 0, (1280, 800)).unwrap();
        let slot = |id: u32| Slot { kind: ao_net::n3::nano::NANO_KIND as u32, instance: id, name: id.to_string(), icon: None, text: String::new() };
        bar.slots[0] = Some(slot(1));
        bar.slots[1] = Some(slot(2));
        let (x, y) = bar.slot_screen_pos(&gui, 1);
        bar.begin_drag(&mut gui, 0, x, y, MouseButton::Left);
        let drag = bar.drag.take().unwrap();
        gui.close_window(drag.ghost);
        bar.drop(&mut gui, drag, x, y);
        assert!(bar.slots[0].is_none());
        assert_eq!(bar.slots[1].as_ref().unwrap().instance, 1);
        assert_eq!(bar.drag.as_ref().unwrap().slot.instance, 2);
        let (x, y) = bar.slot_screen_pos(&gui, 2);
        let drag = bar.drag.take().unwrap();
        gui.close_window(drag.ghost);
        bar.drop(&mut gui, drag, x, y);
        assert_eq!(bar.slots[2].as_ref().unwrap().instance, 2);
        assert!(bar.drag.is_none());
        bar.begin_macro_drag(&mut gui, 1, "Macro", "/sit");
        let drag = bar.drag.take().unwrap();
        gui.close_window(drag.ghost);
        let (x, y) = bar.slot_screen_pos(&gui, 1);
        bar.drop(&mut gui, drag, x, y);
        assert_eq!(bar.slots[1].as_ref().unwrap().kind, KIND_MACRO);
        assert!(bar.drag.is_none(), "native inventory/textmacro branch deletes the occupant");
        let mut other = ShortcutBar::new(&mut gui, &dir, 1, (1280, 800)).unwrap();
        gui.set_window_pos(other.window, (20, 100));
        other.slots[0] = Some(slot(3));
        let (x, y) = other.slot_screen_pos(&gui, 0);
        bar.begin_drag(&mut gui, 2, x, y, MouseButton::Left);
        let drag = bar.drag.take().unwrap();
        gui.close_window(drag.ghost);
        bar.drop(&mut gui, drag, x, y);
        let (slot, x, y) = bar.take_drop().unwrap();
        other.accept_drop(&mut gui, 0, slot, x, y);
        assert!(bar.slots[2].is_none());
        assert_eq!(other.slots[0].as_ref().unwrap().instance, 2);
        assert_eq!(other.drag.as_ref().unwrap().slot.instance, 3);
        other.close(&mut gui);
        bar.close(&mut gui);
    }

    #[test]
    fn ctrl_click_uses_hotbar_slot_while_shift_click_does_not() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &dir, 0, (1280, 800)).unwrap();
        bar.slots[0] = Some(Slot { kind: ao_net::n3::nano::NANO_KIND as u32, instance: 163449, name: "Shadow Touch".into(), icon: None, text: String::new() });
        let list = SpecialList::new();
        let (x, y) = bar.slot_screen_pos(&gui, 0);
        for new_mouse in [false, true] {
            bar.new_mouse = new_mouse;
            for (shift, ctrl) in [(false, false), (false, true), (true, false), (true, true)] {
                let mods = ao_gui::Modifiers { shift, ctrl, ..Default::default() };
                bar.input(&mut gui, &InputEvent::MouseDown { x, y, button: MouseButton::Left }, &list, mods);
                assert!(bar.press.is_some());
                assert!(bar.take_uses().is_empty(), "use waits for release");
                bar.input(&mut gui, &InputEvent::MouseUp { x, y, button: MouseButton::Left }, &list, mods);
                assert_eq!(bar.take_uses(), if shift { vec![] } else { vec![SlotUse::Nano(163449)] });
                assert_eq!(bar.take_info_urls(), if shift { vec![format!("itemid://{}/163449", ao_net::n3::nano::NANO_KIND)] } else { vec![] });
                assert!(bar.press.is_none());
            }
        }
    }

    #[test]
    fn slot_hold_uses_the_configured_delay() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &dir, 0, (1280, 800)).unwrap();
        let list = SpecialList::new();
        bar.hold_delay = 1.0;
        bar.press = Some(Press { slot: 0, held: 0.0, button: MouseButton::Left, delay: Some(bar.hold_delay) });
        bar.update(&mut gui, 0.5, &list, &[], false, (true, true));
        assert!(bar.press.is_some() && bar.drag.is_none());
        bar.hold_delay = 0.1; // An already running native timer keeps the interval captured on press.
        bar.update(&mut gui, 0.5, &list, &[], false, (true, true));
        assert!(bar.press.is_none() && bar.drag.is_some());
    }

    #[test]
    fn legacy_mouse_moves_hold_drag_to_the_right_button() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &dir, 0, (1280, 800)).unwrap();
        let list = SpecialList::new();
        let (x, y) = gui.window_pos(bar.window);
        let (x, y) = (x as f32 + CHROME_W + 10.0, y as f32 + 10.0);
        bar.new_mouse = false;
        bar.input(&mut gui, &InputEvent::MouseDown { x, y, button: MouseButton::Left }, &list, Default::default());
        bar.update(&mut gui, 1.0, &list, &[], false, (true, true));
        assert!(bar.drag.is_none());
        bar.input(&mut gui, &InputEvent::MouseUp { x, y, button: MouseButton::Left }, &list, Default::default());
        bar.input(&mut gui, &InputEvent::MouseDown { x, y, button: MouseButton::Right }, &list, Default::default());
        bar.update(&mut gui, 1.0, &list, &[], false, (true, true));
        assert!(bar.drag.is_some());
        bar.input(&mut gui, &InputEvent::MouseUp { x, y, button: MouseButton::Left }, &list, Default::default());
        assert!(bar.drag.is_some(), "other button release does not end the captured drag");
        bar.input(&mut gui, &InputEvent::MouseUp { x, y, button: MouseButton::Right }, &list, Default::default());
        assert!(bar.drag.is_none());
    }

    /// `MP_Action_Icon` / `MP_Macro_Icon`: `slot_at` finds a bar slot and misses empty space (regression for the
    /// shift-to-inspect cursor; this port has no separate macro bar, macros share the shortcut bar's slots).
    #[test]
    fn slot_at_finds_a_bar_slot() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let bar = ShortcutBar::new(&mut gui, &dir, 0, (1280, 800)).unwrap();
        let (x, y) = gui.window_pos(bar.window);
        let (x, y) = (x as f32 + CHROME_W + 10.0, y as f32 + 10.0);
        assert_eq!(bar.slot_at(&gui, x, y), Some(0));
        assert_eq!(bar.slot_at(&gui, 0.0, 0.0), None);
    }

    #[test]
    fn long_shortcut_tooltips_share_the_item_info_formatter() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &dir, 0, (1280, 800)).unwrap();
        let stats = super::super::hud_stats::HudStats::new(&dir, (1280, 800)).unwrap();
        let zone = super::super::zone::Zone::default();
        let (x, y) = gui.window_pos(bar.window);
        bar.mouse = (x as f32 + CHROME_W + 10.0, y as f32 + 10.0);
        bar.update_tooltip(&mut gui, &zone, &stats, false);
        assert!(bar.tooltip_body.is_empty());
        bar.update_tooltip(&mut gui, &zone, &stats, true);
        assert!(!bar.tooltip_body.is_empty());
        bar.paint(&mut gui);
        assert!(bar.tooltip_key.is_some(), "recharge repaint retains the cached item information");
        bar.update_tooltip(&mut gui, &zone, &stats, false);
        assert!(bar.tooltip_body.is_empty());
    }

    #[test]
    fn nano_contents_survive_restart_and_activate() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() { return; }
        let character = std::env::temp_dir().join(format!("aomac-shortcut-nano-{}", std::process::id()));
        std::fs::create_dir_all(character.join("Containers")).unwrap();
        let path = character.join("Containers/ShortcutBar_1.xml");
        std::fs::write(&path, "<Archive code=\"0\"><Rect name=\"WindowFrame\" value=\"Rect(1,2,403,39)\"/><Int32 name=\"bar_orientation\" value=\"0\"/></Archive>").unwrap();
        let mut gui = Gui::new(&client, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &client, 1, (1280, 800)).unwrap();
        bar.set_row(&mut gui, 7);
        bar.slots[4] = Some(Slot { kind: ao_net::n3::nano::NANO_KIND as u32, instance: 163449, name: "Shadow Touch".into(), icon: None, text: String::new() });
        bar.save_contents(&character, 1);
        bar.close(&mut gui);
        let mut restored = ShortcutBar::new(&mut gui, &client, 1, (1280, 800)).unwrap();
        restored.load_contents(&mut gui, &client, &character, 1);
        assert_eq!(restored.row, 7);
        let slot = restored.slots[4].as_ref().unwrap();
        assert_eq!((slot.kind, slot.instance), (ao_net::n3::nano::NANO_KIND as u32, 163449));
        restored.use_slot(4, &SpecialList::new());
        assert_eq!(restored.take_uses(), vec![SlotUse::Nano(163449)]);
        let root = ao_gui::xml::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(root.children.iter().any(|e| e.attr("name") == Some("WindowFrame")));
        assert!(root.children.iter().any(|e| e.attr("name") == Some("bar_orientation")));
        restored.close(&mut gui);
        std::fs::remove_dir_all(character).unwrap();
    }

    #[test]
    fn first_login_layout_and_actions() {
        assert_eq!(FIRST_LOGIN.iter().map(|s| s.0).collect::<Vec<_>>(), vec![0, 1, 2, 3, 9]);
    }

    /// The first-login slots carry the `Action_e` of their rdb templates: Start Combat 0xb, Walk 0x11, Sit 0x4c, Suspended Animation camp 0x51.
    #[test]
    fn first_login_actions_come_from_the_templates() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let store = ao_rdb::RecordStore::open(&dir).ok();
        let slots = first_login(&mut gui, store.as_ref());
        let list = SpecialList::new();
        let actions: Vec<_> = slots.iter().flatten().map(|s| list.find(s.instance).filter(|_| s.kind == KIND_SPECIAL_ACTION).map(|e| e.shown)).collect();
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

    #[test]
    fn spinner_page_and_plain_drag_respect_lock_and_arrows() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut bar = ShortcutBar::new(&mut gui, &dir, 1, (1280, 800)).unwrap();
        let list = SpecialList::new();
        assert_eq!(gui.text(bar.window, "page"), "1");
        for row in [1, 8, 9, 0] {
            bar.set_row(&mut gui, row);
            assert_eq!(gui.text(bar.window, "page"), ((row + 1) % 10).to_string());
            let label = gui.view_frame(bar.window, "page").unwrap();
            assert_eq!(label.height() + 1.0, gui.font_height(ao_gui::FontId::Normal) as f32, "spinner uses native font 5");
            assert_eq!(label.l, RADIO_W + (19.0 - (label.width() + 1.0) * 0.5).floor(), "native spinner-local centre is 19");
            assert!((label.t + label.b - (BAR_H - 1.0)).abs() <= 1.0);
        }
        // Follow the actual HUD-before-GUI event order with no Shift/Alt modifiers.
        let drag = |bar: &mut ShortcutBar, gui: &mut Gui, local: (f32, f32)| {
            let (px, py) = gui.window_pos(bar.window);
            let (x, y) = (px as f32 + local.0, py as f32 + local.1);
            for ev in [
                InputEvent::MouseDown { x, y, button: MouseButton::Left },
                InputEvent::MouseMove { x: x + 12.0, y: y + 7.0 },
                InputEvent::MouseUp { x: x + 12.0, y: y + 7.0, button: MouseButton::Left },
            ] {
                bar.input(gui, &ev, &list, Default::default());
                gui.input(ev);
            }
        };
        let original = gui.window_pos(bar.window);
        bar.update(&mut gui, 0.0, &list, &[], true, (true, true));
        drag(&mut bar, &mut gui, (RADIO_W + 2.0, BAR_H / 2.0));
        assert_eq!(gui.window_pos(bar.window), original, "locked spinner cannot move");
        bar.update(&mut gui, 0.0, &list, &[], false, (true, true));
        drag(&mut bar, &mut gui, (CHROME_W + 5.0, BAR_H / 2.0));
        assert_eq!(gui.window_pos(bar.window), original, "slots cannot move the bar");
        drag(&mut bar, &mut gui, (RADIO_W + 12.0, BAR_H - 10.0));
        assert_eq!(gui.text(bar.window, "page"), "2");
        assert_eq!(gui.window_pos(bar.window), original, "arrow pages without dragging");
        bar.set_row(&mut gui, 0);
        drag(&mut bar, &mut gui, (RADIO_W + 12.0, 2.0));
        assert_eq!(gui.text(bar.window, "page"), "0", "up wraps from first to last");
        drag(&mut bar, &mut gui, (RADIO_W + 12.0, BAR_H - 10.0));
        assert_eq!(gui.text(bar.window, "page"), "1", "down wraps from last to first");
        assert_eq!(gui.window_pos(bar.window), original, "wrap arrows do not drag");
        bar.update(&mut gui, 0.0, &list, &[], false, (true, true));
        drag(&mut bar, &mut gui, (RADIO_W + 2.0, BAR_H / 2.0));
        assert_eq!(gui.window_pos(bar.window), (original.0 + 12, original.1 + 7), "plain spinner drag moves");
    }
}
