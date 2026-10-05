# GUI engine (`crates/ao-gui`, `ao_render::GuiRenderer`)

Native re-implementation of the original client's GUI.dll view system, used for the login-flow screens
(`docs/screens.md`). Evidence is from the decompiled client DLLs (addresses `0x1xxxxxxx` = GUI.dll unless noted);
anything not read from the binary is labelled **UNRESOLVED** or **[INFERENCE]**.

Pipeline: `Views/<name>.xml` → `view::build` (tree) → `layout` (preferred sizes + HV nodes) → `Gui::frame()` →
`DrawList` (`Gfx`/`Solid`/glyph commands in GUI pixels) → `GuiRenderer::draw`. Input: `Gui::input(InputEvent)` →
`Vec<Event>`. Coordinates are logical window px (the app handles display scale).
Reproduce renders: `cargo run --release -p ao-gui --example dump_view -- LoginWindow out.png --frame --sample`
(`--size WxH` for full-screen views such as `CharacterSelectionWindow`).

## 1. Skin archive (`gfx.rs`)

* `cd_image/gui/Default/Graphics.uvgi`: text; line 1 = entry count, then `name offset length`. `Graphics.uvga` = concatenated PNGs
  (Interfaces.dll `GuiResourceManager_t::ParseFile` 0x1000b86d).
* Ids: AFCM.dll `DynamicID_t` static table @0x10016060; the id of a `GFX_*` name is its index in the 487-name built-in list
  (`crates/ao-gui/data/gfx_ids.txt`, line number − 1); names only present in the uvgi are appended. GUI.dll hard-codes numeric ids
  (e.g. `Button_c::Initialize` 0x10128994: 0x1e..0x26 = `GFX_GUI_BORDER01_*`).
* Colour key: pure green `0x00FF00` → transparent, every other pixel opaque (DisplaySystem.dll `SpriteInfo_t::ConvertImage` 0x1007b8e2).
  PNGs may be RGB or RGBA.

## 2. Fonts (`font.rs`)

`FontSystem_t` table 0x10272df0: TOOLTIP/SHELL/CLOCK bitmap `.fnt` fonts (256×256 skin image, 255 glyph rects + widths, advance = `x1−x0+1`);
NORMAL (Verdana 13), BOLD (13 bold), SMALL (12), LARGE (16), HUGE (24 bold), TT_MIN12 (14), ITALIC (16 italic), CHAT (prefs) are GDI
TrueType, aliased 1-bpp glyphs (`CreateFontA`, `TextOutW`). We rasterise with skrifa mono hinting from the host Verdana
(client does not ship it). **UNRESOLVED**: exact GDI dropout rules; CC17 height.

## 3. Colours and text (`text.rs`)

`GUIColors.xml` ids `DEFAULT 0x1000000, SELECTED 0x2000000, HOVER 0x3000000, TEXT 0x4000000, TEXT_SELECTED 0x5000000, TEXT_HOVER 0x6000000`
(`GUIConfig_c::GetColorID` 0x1012f3e2) map to the palette (defaults from `GUIConfig_c` 0x1012f342: `80e9f3, ffffcc, a5ffdb, 99ccaa …`);
other colour attributes are literal `0xRRGGBB`. Button/frame tints multiply the surface by the palette colour.
`TextView_c` = HTML subset (`br, center, font color, a, p, div, b, i, u`; others ignored) laid out by `_ReWrap` 0x10161ba4 / `_RenderLine` 0x10161112.
Feature flags (`feature_flags="TVF_…|…"`): ACCEPT_TXT_INPUT 1, ACCEPT_MOUSE_INPUT 4, ALLOW_TEXT_SELECTION 8, **PASSWORD 0x10**, MULTILINE 0x20,
WORD_WRAP 0x40, WORD_SPLIT 0x80, IGNORE_NEWLINES 0x100, DISABLE_RC_MENU 0x200, FILL_BOTTOM_UP 0x400, ENABLE_SHADOW 0x800, RENDER_SHADOW 0x1000,
NUMERIC 0x2000, ALT_ENTER_MODE 0x4000.

**Password mask**: `TextRenderer_c::_RenderString` 0x1015ffdd: `if (flags & 0x100 != 0 …) { if (flags & 0x10) ch = 0x2a; }` → every glyph of the
field is drawn as **`*`** (ASCII 0x2a) in the field font, advance of `*`. Copy/Cut are disabled for password fields.
`TextView_c::SlotTabPressed` 0x10002a83: a text field that gains focus by Tab selects all of its text.
**UNRESOLVED**: line pitch of `_AddLineDesc` (we use the font height).

## 4. View XML (`view.rs`, `xml.rs`)

Root `<root>` → first child is the window root view. Elements: `View`, `HLayoutSpacer`/`VLayoutSpacer`, `BorderView`, `TextView`, `Button`,
`TextButton`, `PowerBar`, `BitmapView`, `TextInputView`, `ComboBox`, `ScrollView`/`ScrollViewChild`, `CheckBox`, `RadioButton`, `RadioButtonGroup`;
unknown elements/attributes are collected in `Gui::warnings`.
Common attributes (`XMLObject_c` / `View::LoadFromXML`): `name`, `view_layout` (`horizontal|vertical`), `layout_borders="Rect(l,t,r,b)"`,
`min_size`, `max_size`, `max_size_limit` (`Point(x,y)`), `h_alignment`/`v_alignment` (`LEFT|CENTER|RIGHT|…`), `view_flags`, `tab_order`.
Text: `value` (`#Key` = text.mdb label via the `localize` callback, see `docs/screens.md` §8.2), `font`, `color`, `feature_flags`.
Buttons: `label`/`text`, `color`/`hover_color`/`pressed_color` (TextButton), `bg_gfx`/`full_gfx` (PowerBar).
Integers accept decimal or `0x…` (`GetAttrInt`).

## 5. Layout (`layout.rs`, `geom.rs`)

Rects/points are **inclusive extents** (pixels − 1; empty = −1). `View::UpdatePreferredSize` 0x1014aac8 clamps by `min_size`/`max_size`/`max_size_limit`;
`HLayoutNode` 0x1012ff61/0x101300d7, `VLayoutNode` 0x101304c1/0x10130631 (children get `min…max`, the surplus is distributed by `View::SpaceOut`
0x1014a238 weights; spacers have weight); `layout_borders` shrink each child. Hidden views keep their layout space unless `VF_COLLAPSE_WHEN_HIDDEN` (0x100).
`TextRenderer_c::CalculatePreferredSize` 0x101623ea: if the min/max pref point has `x>0 && y>=0` it is returned as is; otherwise the text is laid out
(word wrap width = `min.x+1` or the current frame).
`Button` (XML) preferred size 0x10127ee4: label size with borders Rect(8,1,8,4), +10 width.

**Initial window size**: `LoginWindow_c` 0x10015331, `ProgressWindow_c` 0x100164cc and `LimboWindow_c` 0x10010311 call the root's
`GetPreferredSize(true)` (vtable +0x9c, arg 1 = *max*) and `Window::SetFrame(Rect(0,0,pref))` + `MoveToCenter`. `WindowSize::Preferred` does exactly this.

## 6. Window frame, style 1 (`Gui::open_framed_window`)

Original: `Window(rect, title, name, style 1, flags)` → `WndBorder::SetStyle` → `SetBorderGfx` 0x1015a82d (see `docs/screens.md` §9 for the ids).

* **Metrics**: `UpdateBorderSizes` 0x10159f98: style 0 → border Rect(3,7,3,3), **style 1 → Rect(3, 24, 3, 3)** (constants 0x101a96d4=3, 0x101b8238=24),
  others 0; plus the embedded `TabView` border sizes (+0x188..), which are all 0 for style 1 (`TabView::LayoutBorders` 0x10145be5 with
  render flags 0; `SetBorderGfx` calls `TabView::SetRenderFlags(0)`). `CalculatePreferredSize` 0x101596bb = client preferred size (tab view
  pref minus tab borders); `WindowToClient` 0x10158cd2 shrinks by those borders. So: outer = client + (3, 24, 3, 3).
* **Art**: outer `BorderView_c` `GFX_GUI_WINDOW3_BORDER_{TL,TR,BL,BR,LEFT,TOP,RIGHT,BOTTOM}` + `GFX_GUI_WINDOW_BACKGROUND` (corners 3×8, top 16×8),
  local alpha = layer-1 alpha 0.33; geometry from `BorderView_c::CreateGfx` 0x10125d3b. Inner border (`top_level_border`, local colour DEFAULT
  0x1000000) `GFX_GUI_TAB_BORDER_*` + `GFX_GUI_TAB_BACKGROUND`, its frame = **client frame grown by 1 px on every side** (`DoSetFrame` 0x10159888:
  `Rect::Resize(-1,-1,+1,+1)`).
* **Buttons** (`CreateBorderIcons` 0x1015aba4, only for styles 0/1/3 and `flags & 4 == 0`): icon button `GFX_GUI_WINDOW_ICON_I` (BorderID 0)
  and close button `GFX_GUI_WINDOW_CLOSE_X` / `_STATE2` / `_STATE3` (BorderID 1); the pin button only without flag 0x800 (not for the login flow).
  `Layout` 0x1015a1d9: every border button is placed at **y = 5** from the window top, the left list from x = 0 (accumulating width), the right list
  right-aligned (`x = bounds.r − width`); no extra margin. Sprites are 15×15. The icon button is created and added unconditionally (so it is drawn on
  the login windows; `Window::SetIcon` 0x101542d9 is not called by them – callers are other windows), and its click only acts when an icon
  popup menu (`+0x208`) exists, i.e. never here.
* **Close**: `SlotCloseButton` 0x10159705 posts message 0x98968b to the window. Engine: `Event::CloseRequested{window}` when the mouse is released
  over the close button (the application decides: LoginWindow → quit, ProgressDialog → ignore). Hover shows `_STATE2`, pressed `_STATE3`
  (**UNRESOLVED**: which `Button_c` state index maps hover/pressed; the three sprites are pixel-identical in this skin, so it is invisible).
* `set_window_pos` / `open_framed_window` take the **outer** top-left; `outer_size`, `window_size` (client).
* **UNRESOLVED**: the window title text (TabView tab strip, only drawn when a title is set; the login-flow windows have an empty title);
  hit-testing/dragging/resizing (`HitTest` 0x101593d6 flags 0x10/0x20/0x8 → not resizable / not movable); the layer-1 alpha value is the
  0.33 default (`GUIConfig_c`), not read from prefs.

## 7. Widgets (`gui.rs`)

* **Button** (`Button_c`): raised border set 0x1e..0x26 (`GFX_GUI_BORDER01_*`, colour DEFAULT), pressed set 0x2f..0x37 (colour SELECTED),
  hover overlay 0x27..0x2e (colour HOVER) – `Initialize` 0x10128994 / `StateChanged` 0x10128338; layer-2 alpha 0.85; disabled tint 0x909090
  (enabled 0xffffff); label centred (`Rect::TranslateCenter`). Click = release inside; `Event::Clicked{window, view, item}`.
* **TextButton** (`CharacterSelectionItem` `name_btn`): text drawn in `color`, `hover_color` while hovered, `pressed_color` while pressed **or toggled on**;
  `ButtonBase_c::SetToggleButton` → click switches it on. `Gui::set_item_selected(h, bool)` mirrors `CharSelectItem_c::SetSelected` 0x1000cb1a
  (toggle + `detailed_view` instead of `summary_view`; the client swaps them in the row container, we collapse the hidden one).
  `set_toggle_in`, `set_color_in` (status colour 0xEE4444).
* **TextInputView / ComboBox editor**: caret (blink), selection (shift+arrows, mouse drag, double click not implemented), Home/End, Delete/Backspace,
  Ctrl+A/C/X/V (events `Copy(text)` / `PasteRequested` → `InputEvent::Paste`), password mask `*`, horizontal scrolling to keep the caret visible,
  click-to-focus + caret placement, `TextChanged`, `EnterPressed`. **Tab / Shift-Tab** cycles the enabled, visible text-input fields of the top
  windows in tree order (original `TextView_c::SlotTabPressed`; buttons are not tab stops – **[INFERENCE]**, no focus ring exists in the art);
  Enter activates the window default button (`set_default_button`, `Window::SetDefaultButton`); Escape → `Event::Escape`.
* **ComboBox** (`ComboBox_c`, editable): clicking the editor opens the popup (`ComboBox_c::MouseDown` 0x10001ec0) with the items below the field (arrow
  glyph changes while open), picking sets the editor text and emits `ComboChanged`; a click elsewhere or any key closes the popup and is processed
  normally. Items via `combo_set_items`, `combo_selected`. Removing an entry is application logic (the `Remove` button of LoginWindow).
  **UNRESOLVED**: `PopupMenu_c` skin (drawn with the raised button border).
* **ScrollView** (`characters_view`): vertical auto scrollbar (`GFX_GUI_SCROLLBAR_GRAY_*`, 10 px extent) only when the content exceeds the view;
  wheel, thumb drag, track click paging; contents clipped.
* **PowerBar** (`ProgressDialog`, `LimboWindow`): `bg_gfx` stretched, `full_gfx` clipped to `value` (`set_progress`, 0..1).
* **BitmapView, CheckBox, RadioButton(Group)**: static/basic; not used by the login flow.
* Images: `add_image(name, rgba, w, h, smooth)` for runtime textures (e.g. the loading screen).

## 8. Application API summary

`Gui::new(client_dir, localize)`, `open_window` / `open_framed_window(view, pos, WindowSize::{Preferred,Fixed})`, `frame(dt) -> DrawList`,
`input(ev) -> Vec<Event>`, text/visibility/enable setters by name (`set_text`, `set_visible`, `set_enabled`, `set_progress`, `focus`),
instance API for repeated rows (`add_view`, `remove_children`, `*_in(handle, name, …)` – **every view of that name in the instance** is affected),
`wants_mouse` (does the GUI own this pointer position), `resize_window`, `relayout_window`.
Events: `Clicked`, `TextChanged`, `EnterPressed`, `ComboChanged`, `Copy`, `PasteRequested`, `Escape`, `CloseRequested`.

## 9. UNRESOLVED (summary)

Title text of framed windows; frame hover/pressed state index; window drag/resize hit-testing; `PopupMenu_c` skin; `_AddLineDesc` line pitch;
GDI dropout rules; layer-alpha values from `GUIColors.xml`/prefs (defaults used); tooltips (`View::SetToolTip` texts exist, not shown);
double-click word selection; IME.

## 11. Stat tables and the skills / inventory / wear windows (`ao_formats::stats`, `play/hud_stats.rs`)

Evidence: `GUI.dll` (GUI) and `Gamecode.dll` (GC) addresses, decoded with Ghidra headless (`/tmp/aomac-ghidra/dsky/gui` = GUI.dll, `/tmp/aomac-ghidra/proto` = Gamecode.dll);
anything not read from the binaries is marked **UNRESOLVED**.

### 11.1 Stat id → name table (`crates/ao-formats/data/stat_names.txt`, `stats::name`)
`n3EngineClientAnarchy_t::N3Msg_GetStatNameMap` [GC 0x10027483] is a `std::map<int, const char*>` filled by **`FUN_1002f009` [GC 0x1002f009–0x100321a9]** as a straight line of
`MOV [EBP-4], id` / `CALL 0x1008a3e8` (map `operator[]`) / `MOV [EAX], &name` triples (a few ids are cleared with `AND [EBP-4], 0`, the store of the string then follows
the *next* instruction; the extraction attributes the string to the id live at the `CALL`). `FUN_1003227a`, `FUN_100324d2`, `FUN_10035c99` are only the readers (`fStatToString`),
not table builders. Result: **523 inserts, 521 distinct ids** (id 0x85 `LR_EnergyWeapon` and 0x2b2 `EquippedRHWeapon` are inserted twice with the same name), max id 1002; earlier docs said 524.
Corrections to earlier notes: id **0 = `Flags`** and id **26 = `Energy`** (docs/zone/world.md listed "0 Energy").
The file (9 KB, id + internal name, no game art) is committed; the extraction script (`StatTab.java`) walks the Ghidra function body (not committed, ~20 lines).
Display strings come from `text.mdb` (`TextDb::by_id(cat, stat)`): **2000** internal CamelCase, **2001** description (shown in the skill details), **2002** long name ("Body Development"),
**2003** short name ("Body Dev.", "Matt.Metam", "Time&Space") – the `StatRow` ctor `FUN_100fe956` reads 2003 (`LDBface::GetText(0x7d3, stat)`); category 10010 = skill group labels
(`#10010:n` in `Skills.xml`, now resolved by `TextDb::label`: `#<category>:<id>` = `GetText(category, id)`).

### 11.2 Own stats in `Zone` (`play/zone.rs`)
`Zone.stats: HashMap<u32,i32>` is filled from the own `FullCharacterIIR_t` (groups `stats_a`, `stats_b`, `stats_u8`, `stats_i16`, `stat_map`, the `0x499602D2` marker skipped, as GC 0x10073a2f does)
and updated by every own `StatIIR_t` (`who.instance == char_id`); other dynels' `StatIIR` never touch it. Test `own_stats_from_full_character_and_stat_deltas` (capture `zone_newchar_ithaca.rec`:
Level 1, IP 1500, Cash 1000, 6 in every ability, 5 in every skill).

### 11.3 Skills window (`Views/Skills.xml`, `SkillWindow` ctor `FUN_100fc18e` GUI 0x100fc18e)
* Window: `Window(Rect(200,180)..(850,700), "", "Skills", style 0, flags 0x1000)`, one tab "Skills", help file "The Skill Window.html", loads `%sViews/Skills.xml`. We open a style-1 frame of
  the same outer size (651×521); the style-0 frame + tab strip is **UNRESOLVED** (docs §6).
* Groups `FUN_100fb596` [GUI 0x100fb596]: 11 groups (`abilities body meleew melees rangedw rangeds nanocast exploring combatheal traderepair disabled`, text `10010:0..9, 9999`) and the 75 skill
  ids pushed per group (`FUN_100ff558(11)`, then `FUN_10064bee` with ECX = `window+0x8c + 0x10·group`; decoded from the asm, `stats::SKILL_GROUPS`; 6+7+10+5+11+6+7+5+6+10+2 = 75, each once).
  The skill names equal the `ipdist.xml` stat names, except `ipdist` still says "Parry" for id 145 "Deflect".
* Rows: class `StatRow` (`FUN_100fe956`): a `ButtonBase` over a `BorderView` (borders hidden) holding name `TextView` (border left 3), spacer, value `TextView`; the *info* rows (flag 1) add a `PowerbarView`
  (`GFX_GUI_HOR_BAR_SMALL_EMPTY/BLUE` = ids 0xe0/0xe1) and two arrow `Button_c`; value = `N3Msg_GetSkill(stat, 2)`. Per group two row sets are created: `<g>_view` rows (compact/"minimised" mode, accordion
  via the group button) and `<g>_group` rows (normal mode). `FUN_100f9552`: in normal mode a group button sets `infoselect` = 1 and `groupselect` = group index (right panel shows the rows);
  `Minimized` mode toggles the `<g>_view` instead. We build the `_group` rows (name button + value text) as XML at run time (`Gui::add_view_xml`).
* Other code: remaining IP `remaining_ip` = `N3Msg_GetSkill(0x35, 0)` (`FUN_100f96c2`); "Reset all skills (n)" `FUN_100fa45c`: `n = (GetSkill(0x15c) & 4 == 0) + GetSkill(0x2b3 FullIPRPoints)`, label
  text.mdb 502/96620988 ("Reset all skills") + " (" n ")", enabled iff n ≥ 1; "Suggested IP distribution" (`suggest_ip`, handler `FUN_100faec0` → `FUN_100fac49`, `RecommendIPUse` `FUN_100f8d69` loads `%sdata/ipdist.xml`) is
  available until level 20 (inittext). `ipdist.xml` = 14 professions × 75 stats `{levelrange min max pri 0..3}`; parser `stats::parse_ipdist` (tested against the client file).
* **N3Msg_GetSkill(stat, mode)** [GC 0x10026d66]: mode 0 (and unknown) → stat object `vtable+0x3c(stat, 2)`; 1 → `FUN_100654e1`; 2 → `FUN_1006554c(stat, 1, 0)`; 3 → `FUN_10064800`; 4 → `FUN_1006554c(stat, 1, 1)`.
  `GetSkillMax` `FUN_100651b5`, `GetSkillCost` `FUN_10061fdb` (float), `GetSkillCostLevel` `FUN_10062398`; they depend on profession (stat 0x3c), level (0x36), breed and `GameData` cost tables (`FUN_1013ecf0`,
  `FUN_100c4ab1`, `FUN_100c499b`) – **not traced**.
* Implemented: all of `Skills.xml` with the client skin, group labels, rows with live values from `Zone.stats` (test drives an own `StatIIR`: Strength 6 → 15 and IP 1500 → 1490 change the labels; a
  foreign dynel's `StatIIR` does not), group selection, row click → details panel (`statName` long name, `statdesc` text 2001, `base`; frame close and `Close`.
* **UNRESOLVED / not implemented**: the buffed value (modifier container `SimpleChar+0x1bc`, filled by Buff/Appearance messages) is shown equal to base; maximum skill, cost to improve, total cost, the
  per-row power bars and arrow buttons (need `GetSkillMax`/`GetSkillCost` and the IP-spend messages); "Suggested IP distribution" (`FUN_100fac49`: the IP simulation over `ipdist.xml` priorities);
  "Save Changes" / "Reset" (outgoing skill messages not identified); whether the "Disabled / Legacy" button is hidden unless a deprecated skill has points (the ctor does not hide it; later signal handlers
  `FUN_100fa7ba` were not traced); row hover/selected colours (`TEXT_HOVER`/`TEXT_SELECTED` used); window position persistence (`LoadWndConfig`).

### 11.4 Wear window (`WearView_c` `FUN_100e1bc9` GUI 0x100e1bc9, window name `wear_window`, title text.mdb 10000 "Wear")
A `TabView` with four `MultiListView_c` item grids built by `FUN_100cc1b3` (`InventoryViewBase_c`, base `FUN_100cdb3a`): tabs "Weapon", "Armor"/"Clothes", "Implant" (text.mdb 10000/10003 strings) and a literal
"Social". Every grid is `SetViewCellCounts(3,5)`, `SetGridIconSpacing(11, 9)` (floats 0x101bee04 / 0x101bfc88) with background `GFX_GUI_WEARVIEW_BG1` (0x1b4) / `BG2` (0x1b5, implants) and slot
frames `GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED` (54×54). Slot tables (static initialisers `101a5464/101a5684/101a5860/101a5a80`, entries `{id, IPoint(x,y)}`): armor 15 cells row-major `1..15`
(`100+id-1` = text.mdb 505 "Neck, Head, Back, Right Shoulder, Chest, Left Shoulder, Right Arm, Hands, Left Arm, Right Wrist, Legs, Left Wrist, Right Finger, Feet, Left Finger"), implants 13 cells (`200..212`,
last at (1,4) = Feet), weapons 15 cells (`300..314` Hud 1-3, Utils 1-3, Right Hand, Deck, Left Hand, Deck 1-6), and the "Social" grid whose cells are ordered `1,15,2,3..14`.
The client ships the finished tab art `GFX_GUI_WEARVIEW_WEAPON / CLOTHING / IMPLANTS` (192×320, tab strip "Weapons | Clothes | Implants" and the labelled slot frames baked in; tab header x ranges 4–72 /
72–122 / 122–188, height 17, measured from the PNG): our window shows that art for the selected tab (three `BitmapView`s stacked, `TextButton` hit areas over the strip) in a 5 px border.
Empty-safe: no item icons are drawn (`FullCharacter` inventory/equipment blocks are not decoded, docs/zone/world.md §2). **UNRESOLVED**: the fourth "Social" tab (art not found; not shown), the window frame/title
(style, `TabView` strip), default window position, item icons/tooltips (`ACGItem_t` layout).

### 11.5 Inventory window (`InventoryView_c` `FUN_100cc2ca` GUI 0x100cc2ca, `inventory_window`)
`FUN_100cc2ca(type 0)` reads `inventory_window` window config + `item_position_map` (slots `i` → `IPoint(i%3, i/3)`, 21 positions), builds `FUN_100cc1b3 → ItemContainerView_c` (`FUN_100cdb3a`): a
`MultiListView_c` with columns Icon / Name / Count (the `InventoryViewMode` pref selects list vs grid), `SetMaxItemCount(0x1e)` = **30 slots** for the character's own inventory (`0x15`/`100`/`0x66`
for container / bank / reclaim kinds). We draw the grid mode with 30 empty slot frames (`GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED`, spacing 11×9 as the wear grids).
**UNRESOLVED GUESS**: 5 columns × 6 rows (the inventory's `SetViewCellCounts`/default `InventoryViewMode` were not traced); the list mode; the window frame/position; item contents.

### 11.6 Engine additions (ao-gui)
`ViewSelector` (children share the bounds, `Gui::select_child(w, name, Some(i))` shows one and collapses the rest), `Gui::show_collapsing`, `Gui::add_view_xml`, **width groups**
(`width_group` / `width_group_owner`: every member under the owner ancestor gets the widest preferred width of the group; `layout::group_width`), a `ScrollViewChild` with several children
(the skill window makes the child itself the scroll client, `FUN_100fc18e`: implicit inner view), XML numeric character references (`&#8216;`), `TextDb::label("#cat:id")`.
Word-wrapped text sizes itself from its current frame, so the skills window is laid out twice after opening.

### 11.7 Reproduce
`AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac hud_stats` writes `skills-0-initial/1-abilities/2-after-delta/3-nano.png`, `wear-0..2-*.png`, `inventory-wear-both.png`
(real client XML + skin, stats from the `zone_newchar_ithaca.rec` capture). Other stat users: the HUD bars (`play/hud.rs`), docs/zone.md §6.

## 10. In-world control centre (`play/hud.rs`, `play/hud_bar.rs`, `ao-gui` `expr.rs` / `gui/cc.rs`)

Evidence (GUI.dll, project copy of `/tmp/aomac-ghidra/dsky/gui`):
* `ControlCenterModule_c` ctor 0x1006c64d; `LoadMainConfig` 0x1006c371 loads `Variables.xml`, `MainPrefs.xml`, `LoginPrefs.xml`, `CharPrefs.xml` (defaults of every `dvalue:cc_*`: all true except `cc_mini_toolbar`; `NumHotbars` 1; windows false) then the user prefs. `SlotPlayerCharacterAlive` 0x1006afed creates the bars once the character is alive.
* `FUN_1006f098` (`ControlCenterWindow_c`) loads `Views/ControlCenter.xml`, finds the docks by name and fills them: Left/RightWingDock = `CollapsingBitmapView_c` (`GFX_GUI_CONTROLCENTER_WING_LEFT` 0xb5 / `_RIGHT`), LeftBarDock = `LeftBarView_c` 0x1006f7c5 (bg `BOTTOM_LEFT` 0x99, HLayout `NCU`, `used/max` = stats 0xb4/0xb5, spacer, `CRED`, cash = stat 0x3d via `FormatNumeric`), RightBarDock = `RightBarView_c` 0x1006ff09 (`BOTTOM_RIGHT` 0x9a, `DEF`, `Slider_c` AGGDEF −100..100, `AGG`), Left/RightTargetCtrlDock = `CCTargetControl_c` 0x100746ec (HudTarget), RollupControllerDock = `RollupArea` `DockArea_c` (`InitialiseMessage` 0x1006a968: Rect(W−225, 20, W, H−191)). `FUN_1006ee1e` shows each view whose `activate_criteria` holds (`View::Show(CriteriaMonitor_c::Evaluate())`) → `Gui::apply_criteria` + `ao_gui::expr` (`dvalue:`, `stat:`/`s:`, `id:`, `&& || ! == != < > <= >= & | + -`).
* Bars: `CharacterBar_c` 0x10066af6 = `PowerbarView_c(bg, full, left cap, right cap, Direction 3 = up)` in a style-3 `CharBarWindow_c` 0x1006d7c7; art `GFX_GUI_ACTIONVIEW_{HEALTH,NANO,XP,ALIENXP}BAR[_BACKGROUND|_LEFT|_RIGHT]` (11×107 + 9 px caps = 11×125, matching the saved frames 10×124 inclusive). Values: health stat 27/1 (0x100666b6), nano 214/221 (0x100667a8), XP (stat 0x34 − 0x39)/(0x15e − 0x39), level ≥200 uses 0x23d/0x240 (0x100668aa), alien XP 0x28/0xb2 (0x100669f7, created only if stat 0x185 (Expansion) & 0x18). Tooltip `View::SetToolTip(LDB text (cat 0x2710, key at 0x101b4300…), "%d / %d")` (key strings not resolved: UNRESOLVED, no tooltip set).
* Menus: `CCMenu` → `ControlMenu_c` 0x10071524, entries `CCMenuEntry_c` 0x100660f2 (attrs `label bgicon invoke active_value criteria button_mode golden_button tooltip tooltip_body sub_script`); entries are stacked vertically, pitch = height + 4 (0x1007086f), invisible (criteria) entries skipped. Sub menus open in a style-3 window (flags 0xd3c) placed by 0x100653b6 (right of the button unless its centre is in the right half, bottom 7 px above the button bottom). Entry = `Button_c` with 3-slice art raised `BUTTON01_*` (0x9b/9d/9f, golden 0x9c/9e/a0), pressed `BUTTON03_*`, hover `BUTTON02_*` (0x10065b4e); `bgicon` 48×22 at layer alpha 0.85; `UpdateBgIconFade` 0x10127bdd: icon tint = 0x20 + (1−fade)·223, label shown when fade > 0.8, fade ±0.075 per `SlotFadeTimer`.
* Shortcut bar: `ShortcutBarWindow_c` 0x100d94e9 / `ShortcutBarView_c` 0x100d8c12: radio (0x159/0x15a) + `TOOLBAR_CONTROL_H` 32×38 with up/down buttons + 10×1 `MultiListView` of `SLOT_32_CLOSED` (38×38, pitch 36) = 403×38. Config path `…/Containers/ShortcutBar_<n>.xml`.
* `prefs/NewChar/*` (CCHealthBarConfig frames, ShortcutBar frames, DockAreas, Chat windows) is the install's new-character template; **no DLL contains the string `NewChar`** (searched all DLL/EXE, ASCII and UTF-16), so its consumer is UNRESOLVED; we use its `WindowFrame`s (clamped like `Window::MoveInsideScreen` 0x10154abc) as the first-login layout. Bars at (1075/1085, 794) sit right of the hotbar (662..1064, y 1017), consistent with a coherent default.

Not done / UNRESOLVED: compass window (`CompassWindowConfig` Rect(1800,5,…) art `GFX_GUI_COMPASS*` not ported); `CCMiniToolbar` (hidden by default); AGG/DEF slider is static (default value of `N3Msg_GetAggDef` and dragging not traced); hotbar first-login contents (identities {0xdeb0: 0xc1a5, 0xc1a2, 0xc1a8, 0x14124} slots 0-2,9, `/follow` macro slot 3: no icon source offline) and slot interaction; fade timer period (50 Hz assumed); label-only entry width; RollupArea page windows (dock layout only reserved); bar tooltips; alien bar default position. Shots: `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac hud::tests` writes `hud-1280.png`, `hud-1920.png`.

## 13. Tooltips (`ao-gui` `gui/tooltip.rs`) and the target controls (`play/hud_target.rs`)

### 13.1 Tooltips (GUI.dll)
Evidence (decompiled GUI.dll, all addresses `0x1xxxxxxx`):
* **Source of the texts**: `View::SetToolTip(title, body)` 0x1014dadf stores two strings in the view's extra data (`+0x140` title, `+0x15c` body); `View::GetToolTipText`
  0x1014db1a returns them (or asks the virtual `ProvideToolTipText` 0x1014a750, which is `false` for plain views; `PowerBar_t::ToolTipCallback` 0x10020686,
  `MultiListView_c::SlotProvideItemTooltip` 0x100092ce and `TextRenderer_c::ProvideToolTipText` 0x1016317a are the dynamic providers). XML views carry `tooltip="#Key"` /
  `tooltip_body="#Key"` (ActionMenu entries, `OptionPanel/Root.xml`); `#` keys resolve through text.mdb like every other attribute.
* **Timing**: `WindowController_c::UpdateToolTip(false)` 0x101577fd (mouse moved) walks the *topmost window under the pointer* (`FUN_10156d66`: children from the top child
  down, then the view itself) to the deepest view whose `GetToolTipText` is non-empty; if it differs from the stored view (`+0xa0`) the shown tip is closed; then the texts
  are stored and the deadline `timeGetTime() + 500` ms re-armed — **every mouse move over a tipped view restarts the 500 ms rest timer**. `WindowController_c::Render`
  0x101572a1 creates `new ToolTip_c(title, body)` when the deadline passed, closing a tooltip that is still shown (so after the pointer rests again the tooltip jumps to the new
  position). `HandleMouseDown/Up` 0x10157ab0 / 0x10157c34 call `UpdateToolTip(true)` (close + reset); `ToolTip_c::SlotGlobalMouseDown` 0x101499ab hides it on a click outside its
  frame. `InputConfig_t::GetTooltipTime` (0x1000b5d2, a float at `InputConfig_t+0x20`) has no caller in the paths above (item tooltips): not used here.
* **Window**: `ToolTip_c::ToolTip_c` 0x10149a0d = `Window(Rect(0,0,10,10), "", "info_window", style 2, flags 0x105)`; style 2 draws `BorderView_c::SetGfx(…, 0x1bf)` =
  `GFX_GUI_WINDOW_BACKGROUND` (black, 15×15) at the layer-2 alpha **0.85** (`WndBorder::SetBorderGfx` 0x1015a82d) and no frame/buttons; the view is `InfoContainer_c`
  (`FUN_101492c6`, view name `info_view`), inset by 1 px on every side when a body exists (`Rect::Resize(1,1,-1,-1)`).
* **InfoContainer layout** (inclusive extents, padding `p = 1` without a body, `p = 2` with one): title `TextView_c("title_view", font 5 = NORMAL)`, body `TextView_c("text_view",
  NORMAL)` with HTML flags `0x60` (MULTILINE | WORD_WRAP) and `SetAspectRatio(2.0)` (wrap width `ftol(sqrt(textArea) · 2)`, at least the title width: `TextRenderer_c::
  CalculatePreferredSize` 0x101623ea). Preferred size: `x = max(titleW, bodyW) + 4p`, `y = titleH + 4p` (no body) or `titleH + bodyH + 1 + 7p`. `ViewSurface_c`s
  (`ViewSurface_c(Rect, flags)`, flags = anchoring: bit0 left, bit1 right, bit2 top, bit3 bottom, `_LayoutSelf` 0x101509e1): four edge lines of thickness `p` (flags 0xd left, 7 top,
  0xe right, 0xb bottom) in the **HOVER** colour, plus – with a body – a separator line `p` thick under the title (flags 7, HOVER), without a body a solid black fill (flags 0xf).
  Title frame `(2p, 2p, R−2p, titleH+2p−1)`, body frame `(2p, titleH+5p, R−2p, B−2p)`. Without a body the title is drawn in the DEFAULT colour
  (`TextRenderer_c::SetDefaultColor`), with a body both texts use the default text colour (UNRESOLVED GUESS: TEXT).
* **Placement** `Window::MoveToMouse(false)` 0x10154e16: `x = mouse.x + 16`, or `mouse.x − (width+1)` if `mouse.x ≥ screenW/2`; `y = mouse.y + 32`, or `mouse.y − (height+1)` if
  `mouse.y ≥ screenH/2`; then `MoveInsideScreen`.
* Not decided from the binary (UNRESOLVED): the window size beyond the view (`pref + (2,2)` hits the argument list of `Window::ResizeTo` only through a mangled decompile; we use
  `pref` + 2 only with a body, since the view is inset by 1), the title colour with a body, the sqrt operand of the aspect rule (we use unwrapped width × line height).

Engine API (`ao_gui::Gui`): `tooltip=`/`tooltip_body=` on any XML element → `View::tip`; `set_tooltip(window, view, title, body)` / `set_tooltip_in(handle, …)` for dynamic
texts; `set_screen_size` (default: largest visible window); `show_tooltip(title, body)` shows a `ToolTip_c` at the pointer immediately (`tooltip_shown`, `tooltip_rect` for
tests). The timer runs on `Gui::frame(dt)`; `input()` feeds `UpdateToolTip`. Tests: `crates/ao-gui/tests/tooltip.rs` (500 ms rest, closing, placement, flip). Reproduce:
`cargo run --release -p ao-gui --example dump_view -- LoginWindow out.png --size 640x400 --tooltip "Skills|Body text" --mouse 100,60`.

### 13.2 Target selection (GUI.dll `TargetingModule_t`, `InputConfig_t`)
* The selection is **client-local**: `TargetingModule_t::SetTarget(id, forced)` 0x100257b0 → `InputConfig_t::SetCurrentTarget` 0x10019df0 (identity at `InputConfig_t+0xc0`,
  `GlobalSignals` emit) + `AFCM::Send(0x13, 0x126, id)` (received by `TargetingModule_t::SetTargetMessage` 0x10025c14 and `N3InterfaceModule_t::SetTargetMessage` (Interfaces
  0x1000907a) → `n3EngineClientAnarchy_t::N3Msg_SelectedTarget` Gamecode 0x10017728 → `n3Camera_t::SetSelectedTarget` N3 0x10020358) + the selection `Indicator_t` (`FUN_100255be`,
  not for targets with skill-flag 0x400) + the tip events `OnSelecting` / `OnSelectingNPC` / `OnSelectingPlayer`. **Nothing is sent to the server.**
  `FrameProcess` 0x10025fa4 drops the target when the dynel disappeared (`N3Msg_GetPos` fails or `N3Msg_GetParent` ≠ 0) unless it was forced; `RemoveTarget` 0x2598b remembers it
  as `m_cLastTarget`; `SelectSelf` 0x100259b1: no target → self; target is self → previous target; else remember + self.
* Keys (GUI.dll default hotkeys, `InputConfig_t::SetDefaultHotkeys` 0x1001ad88 text): `TAB` next hostile, `SHIFT+TAB` previous hostile, `CTRL+TAB` next friendly,
  `CTRL+SHIFT+TAB` previous friendly (`COMMAND_{NEXT,PREV}_{HOSTILE,FRIENDLY}_TARGET` → `N3Msg_GetCloseTarget(current, friendly, forward)`; the ordering is [INFERENCE]: by distance).
* The pick: `InputConverterModule_t::InputGameSubmodeActionViewONMessage` 0x1001bcae → `N3Msg_SetMousePos` (Gamecode 0x1001613b) → `n3Camera_t::SetMousePos` (N3 0x10020571:
  ray `(tan(fov/2)·x, tan(fov/2)/aspect·y, 1)` through the camera matrix, scaled by `VisualCamera_t::GetLengthOfViewcone`, cast into the collision world) → identity under the
  mouse → `InputConfig_t::CheckObjectUnderMouse` 0x10019f00 picks the mouse pointer (`Pointer_e` 1–5 by `GetSkill(0x1e)` flags / NPC / `CanAttack`; cursor art not ported).
  The mouse *click* → `SetTarget` dispatch itself was not found (the default hotkey list has no mouse command except debug ones; `HandleMouseDown` only emits `GlobalSignals`), so
  UNRESOLVED: which signal turns a left click into `SetTargetMessage`, and any click-on-ground deselect. We select on a left click without drag, and a click on empty ground keeps the target.
  We have no collision meshes of the dynels: `hud_target::pick` intersects the ray with one capsule per dynel ([`CAPSULE_HEIGHT`] 1.8 m, [`CAPSULE_RADIUS`] 0.5 m, UNRESOLVED GUESS).
* The ground **selection indicator** and the hover pointer change need renderer support (ground decal / cursor art) and are not implemented.

### 13.3 Target controls (`CCTargetControl_c`, GUI 0x100746ec; `play/hud_target.rs`)
* `ControlCenterModule_c::CreateTargetMenus` 0x1006a0d4 builds two controls (friendly = `0x154` flag 0, hostile = 1) twice: the **dock** version (`LeftTargetCtrlDock` /
  `RightTargetCtrlDock` of `ControlCenter.xml`, `Rect(10,0,0,36)`/`Rect(0,0,10,36)`) = `HLayout[left arrow 8×41 (0x93/0x95/0x94), target button 48×48 toggle (0xaa/0xac/0xab) with a
  centred icon (friendly 0xae `TARGET_ICON_SELF` 38², hostile 0xad `…_OTHER` 40²), right arrow (0x96/0x98/0x97)]` (`Button_c::SetGfx(state 0 normal, 1 pressed = …STATE3, 2 hover =
  …STATE2)`), and the **bar window** `CCFriendlyHealthBar` / `CCHostileHealthBar` (`CharBarWindow_c`, style 3, flags 0xe3c, criteria `dvalue:cc_section1 &&
  dvalue:cc_{friendly,hostile}_health_bar`) at `y = 5`, `x = (screenW − 192)·{½·½, ½·3/2} − width/2`, bar width `(screenW − 192)/2 − 50` (verified: the install's `prefs/NewChar/Prefs.xml`
  frames `Rect(25,5,1158,65)` / `Rect(1208,5,2341,52)` are exactly these for a 2558 px screen).
* Bar window = `TargetHealthBar_c` (`FUN_10073598`; surfaces `GFX_GUI_TARGET_HB_LEFT/RIGHT` 9×11 caps tinted DEFAULT, `…_BACKGROUND` tinted DEFAULT, `…_SLIDER` untinted, repeating
  16 px, covering `ratio` of the background) above a `TargetHeader_c` (`FUN_10073884`: corner-only `BorderView` `GFX_GUI_CC_TARGET_FRAME_TL/TR/BL/BR`, colour DEFAULT, bold caption
  `<center>Selection</center>` — the other captions "Nano Target", "Fighting Target", "Nano / Fighting Target" belong to the NCU/fight variants — and the target's name in white, from
  `N3Msg_GetName` + `String::StripSpecialChars`). `FUN_10072e49` tints the caps `0xff2222` when `+0x161`/`+0x162` is set (writers not identified: UNRESOLVED, caps stay DEFAULT).
* Which control shows a target: attackable → hostile, otherwise friendly. Attackable (`FUN_100744ae`): NPC and `Side` (stat 0x21) differs from the own side; a player needs
  `N3Msg_CanAttack` (not modelled). The dock arrows are bound to `TargetingModule_t::Get{Next,Prev}{Friendly,Hostile}TargetMessage` ([INFERENCE]), the friendly button to `SelectSelf`;
  the hostile button's action (`FUN_1007303d`) and the target-of-target button (`FUN_10075342`, "Fighting Target:" in 0xff4444, `N3Msg_GetTargetTarget`), the NCU windows
  (`NanoTargetNCUWindowConfig`, `FightTargetNCUWindowConfig`) are not implemented (UNRESOLVED).
* Data: `Zone.target` (instance id), `DynelState { side, level, health, max_health }` (from `SimpleCharFullUpdate` and other characters' `StatIIR_t`). Tests in
  `hud_target.rs` (ray math, capsule pick, cycling, `SelectSelf`, windows follow the selection, `AOMAC_SHOT_DIR` screenshots `target-hostile.png`, `target-friendly.png`).

**Known gap (13.3)**: the dock arrows are not visible. `gfx_ids.txt` holds the AFCM `DynamicID_t` names truncated to 40 characters, so ids 0x93–0x95 / 0x96–0x98 (`…BIGARROW_{LEFT,RIGHT}_STATE1..3`) have three identical names and `GfxSet::size(GfxId(0x93))` is (0,0); the full uvgi names only exist under appended ids. Fix for the GuiEngine owner: map the n-th table id of a truncated duplicate to the n-th full uvgi name in sorted order (STATE1, STATE2, STATE3; consistent with `SetGfx(1, …STATE3 = pressed, 2, …STATE2 = hover)`). The target button, icons and health bars use unaffected ids.
