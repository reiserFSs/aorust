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
* Ids: AFCM.dll `DynamicID_t` static table @0x10016060 (a table of `char*` pointing at the full names); the id of a `GFX_*` name is
  its index in the 487-name built-in list (`crates/ao-gui/data/gfx_ids.txt`, line number − 1, read from that table in full: an earlier
  copy cut at 40 characters gave 13 wrong names, e.g. 0x93–0x98 `…BIGARROW_{LEFT,RIGHT}_STATE1..3` were identical and their
  images unreachable; also 0x92 `AGGDEF_SLIDER_BACKGROUND`, the golden button slices 0x9e/0xa0/0xa7/0xa9 (`BUTTON01/03_MIDDLE/RIGHT_GOLD`)
  and 0xab/0xac `TARGET_BUTTON_STATE2/3`; test `gfx::tests::id_names_are_unique_and_arrows_resolve`); names only present in the uvgi are appended.
  GUI.dll hard-codes numeric ids (e.g. `Button_c::Initialize` 0x10128994: 0x1e..0x26 = `GFX_GUI_BORDER01_*`).
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
* **UNRESOLVED**: hit-testing/dragging/resizing (`HitTest` 0x101593d6 flags 0x10/0x20/0x8 → not resizable / not movable); the layer-1 alpha value is the
  0.33 default (`GUIConfig_c`), not read from prefs. The window title text is §6.1 (style 0).

### 6.1 Style-0 window with a tab strip (`Gui::open_tabbed_window[_xml]`)
The in-world windows are `DockWindow_c` (`FUN_1003bdd4`: `Window(Rect(), "", "", style 0, flags 0x1000)`), and a `DockableView_c` (`FUN_10038b47`) is a *tab*: `Window::InsertTab(index,
title, view)` with `title` = the `DockableView_c` constructor argument (`FUN_100389bd`). `Window::SetTitle` (0x1015427b) only stores a string, nothing draws it. The title texts are therefore
the constructor literals: `"Planet Map"` (`PlanetMapView_c` 0x1004d26a), `"PF Map"` (`PlayfieldMapView_c` 0x100eb905), `"Skills"` (`SkillWindow` Window ctor), `"Stats"`, `"Friends"`, `"Knowledge"`, `"Missions"`, `"NCU"`,
`"Actions"`, `"Faction"`, `"Trade"`, `"Tradeskill Kit"`, `"Info"`, `"Tips"`; `GetText(10000, "Wear")` (`WearView_c` 0x100e1bc9, string at 0x101b59ac), `"Inspect"` for the inspect variant; the inventory
passes `""` (its tab title is set later, UNRESOLVED). **`#WindowMap` / `#WindowPlanetMap` of `ControlCenterModule_c::SetupProviders` 0x10068c38 are not titles**: the four strings of each provider
(`FUN_10018788`) are dvalue name, key-binding option label (`#WindowPlanetMap`), group (`#Window`) and the provider key (`WINDOW_PLANETMAP`, §12.1).
* Style 0 (`WndBorder::SetBorderGfx` 0x1015a82d): the same outer `GFX_GUI_WINDOW3_BORDER_*` art as style 1 (alpha 0.33) but `TabView::SetRenderFlags(7)` and no inner `top_level_border`. Outer border
  `Rect(3, _DAT_101b6e54 = 7, 3, 3)` (`UpdateBorderSizes` 0x10159f98) + `TabView` borders `(2, strip + 1, 2, 2)` (`LayoutBorders` 0x10145be5: `+= _DAT_101a8b90 = 2.0` on all four, then the top is `+0x158` =
  tab-bar height + 1) → **client insets (5, 26, 5, 5)** (strip = 17 px tab + 1). The `TabView` is the outer rect minus the outer border.
* Tab strip (`TopBorderView_c` ctor `FUN_10147935`, flags 7, layer-2 alpha 0.85): the tabs sit in a container starting `TabView::SetLeftMargin(_DAT_101b4e08 = 20)` from the left edge (the 15 px icon button fills the margin); the box
  `GFX_GUI_TAB_BORDER_*` + `GFX_GUI_TAB_BACKGROUND` (the same 9 pieces as style 1's inner border) starts at strip row 16 so its 2 px top line (`TAB_BORDER_TOP/TL/TR`, `FUN_10146e94`) is the strip's last two rows.
* `Tab` (ctor `FUN_10146785`): `BorderView_c::SetGfx(l, m, r)` (3-slice, `0x195/0x196/0x197` = `GFX_GUI_TAB_ACTIVE_LEFT/MIDDLE/RIGHT` 6/16/21 × 17 selected; `0x1a1..0x1a3` inactive) over a `TextView_c(font NORMAL)`;
  `FUN_101461dc` preferred width = title width + 1 + `Rect(5,0,16,0)` at `Tab+0x1e4` (5 + 16), height = art height; `Tab::SetSelected` `FUN_10146110`: selected = active art, alpha = layer 2, text colour `0xffffff`; unselected = inactive art
  at alpha × `_DAT_101c4978`, colour 0. Only the selected tab is used (one tab per window).
* **UNRESOLVED**: the title's vertical offset inside the tab (we draw it at the tab top like every `TextView`), the left offset 5 (the `Rect(5,0,16,0)` field is only read in the preferred-size function), ±1 px of the strip
  height (`Point` extents vs counts in `FUN_10147c93` = tab height + 1, `LayoutBorders` +1), tab hover/drag, several tabs. No retail screenshot of a style-0 window was available to compare.
* Used by: Planet Map, Playfield Map, Skills, Wear (`play/hud_*.rs`); the Inventory window keeps the style-1 frame until its title is known.

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

Several tabs / tab vertical text offset (§6.1); frame hover/pressed state index; window drag/resize hit-testing; `PopupMenu_c` skin; `_AddLineDesc` line pitch;
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
* Bars: `CharacterBar_c` 0x10066af6 = `PowerbarView_c(bg, full, left cap, right cap, Direction 3 = up)` in a style-3 `CharBarWindow_c` 0x1006d7c7; art `GFX_GUI_ACTIONVIEW_{HEALTH,NANO,XP,ALIENXP}BAR[_BACKGROUND|_LEFT|_RIGHT]` (11×107 + 9 px caps = 11×125, matching the saved frames 10×124 inclusive). Values: health stat 27/1 (0x100666b6), nano 214/221 (0x100667a8), XP (stat 0x34 − 0x39)/(0x15e − 0x39), level ≥200 uses 0x23d/0x240 (0x100668aa), alien XP 0x28/0xb2 (0x100669f7, created only if stat 0x185 (Expansion) & 0x18). Tooltip `View::SetToolTip(LDBface::GetText(0x2710, key), "<tvoptions wordwrap=\"no\">%d&nbsp;/&nbsp;%d")` with the keys `Health` (0x101b4300), `Nano` (0x101b4334), `Experience` (0x101af930, shared literal, referenced at 0x100669a4) and `AlienExperience` (0x101b433c, referenced at 0x10066aa5) = text.mdb category 10000 `by_key` → "Health", "Nano", "Experience", "Alien Experience" (`Hud::bar_titles`, test `bar_titles_come_from_the_text_db`).
* Menus: `CCMenu` → `ControlMenu_c` 0x10071524, entries `CCMenuEntry_c` 0x100660f2 (attrs `label bgicon invoke active_value criteria button_mode golden_button tooltip tooltip_body sub_script`); entries are stacked vertically, pitch = height + 4 (0x1007086f), invisible (criteria) entries skipped. Sub menus open in a style-3 window (flags 0xd3c) placed by 0x100653b6 (right of the button unless its centre is in the right half, bottom 7 px above the button bottom). Entry = `Button_c` with 3-slice art raised `BUTTON01_*` (0x9b/9d/9f, golden 0x9c/9e/a0), pressed `BUTTON03_*`, hover `BUTTON02_*` (0x10065b4e); `bgicon` 48×22 at layer alpha 0.85; `UpdateBgIconFade` 0x10127bdd: icon tint = 0x20 + (1−fade)·223, label shown when fade > 0.8, fade ±0.075 per `SlotFadeTimer`.
* Shortcut bar: `ShortcutBarWindow_c` 0x100d94e9 / `ShortcutBarView_c` 0x100d8c12: radio (0x159/0x15a) + `TOOLBAR_CONTROL_H` 32×38 with up/down buttons + 10×1 `MultiListView` of `SLOT_32_CLOSED` (38×38, pitch 36) = 403×38. Config path `…/Containers/ShortcutBar_<n>.xml`.
* `prefs/NewChar/*` (CCHealthBarConfig frames, ShortcutBar frames, DockAreas, Chat windows) is the install's new-character template; **no DLL contains the string `NewChar`** (searched all DLL/EXE, ASCII and UTF-16), so its consumer is UNRESOLVED; we use its `WindowFrame`s (clamped like `Window::MoveInsideScreen` 0x10154abc) as the first-login layout. Bars at (1075/1085, 794) sit right of the hotbar (662..1064, y 1017), consistent with a coherent default.

Not done / UNRESOLVED: `CCMiniToolbar` (hidden by default); fade timer period (50 Hz assumed); label-only entry width; RollupArea page windows (dock layout only reserved); alien bar default position; the compass / slider / hotbar gaps are listed in 10.1 – 10.3. Shots: `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac hud::tests` writes `hud-1280.png`, `hud-1920.png`, `compass-*.png`, `hud-aggdef-*.png`, `hud-hotbar*.png`.

### 10.1 Compass (`play/hud_compass.rs`)
* `CompassWindow_c` factory `FUN_1006d433` (GUI 0x1006d433; the dtor `FUN_1006d697` saves the window into the DValue `CompassWindowConfig`): `Window(Rect(0,0,0,0), "compass_window", style 3, flags 0xd3c)`, criteria `dvalue:cc_section1 && dvalue:cc_compass` (`CriteriaMonitor_c`, both true in `CharPrefs.xml`; `FUN_10067c45` shows / hides the window when the criteria flip), child `CompassView_c` (`FUN_100677b3`, 0x160 bytes). Position: `MoveTo(floor(screenW · (double)0.73f − Width(frame)/2), 5)` (`_PTR_101b45b0` = 0x3fe75c29_00000000, `_DAT_101a8b98` = 5.0; `Width` = inclusive extent 136), then `LoadWndConfig` (nothing saved on a first login) and `MoveInsideScreen`. Check: the install's template `prefs/NewChar/Prefs.xml` `CompassWindowConfig` `Rect(1800,5,1936,26)` is exactly this formula for a 2560 px screen (`floor(2560·0.73 − 68) = 1800`), so the template is *not* an extra input; per resolution: 1280 → x 866, 1920 → x 1333 (test `origin_follows_the_screen_width`).
* Surfaces (`ViewSurface_c::Load(id, flags)`): frame `GFX_GUI_COMPASS_FRAME` 0x8e (137 × 22, green-keyed centre), strip `GFX_GUI_COMPASS_SCALE` 0x8f with flags 0x10 (256 × 14, tiled: N at x = 0, E 64, S 128, W 192, tick marks every 16 px), waypoint marker `GFX_GUI_COMPASS_WAYPOINT` 0x90 (7 × 9). `GFX_GUI_COMPASS` (256 × 16) and `_BORDER` (134 × 20) are **not referenced** by this view (ids 0x8b / 0x8c; other users in the DLLs were not searched). Strip and frame get `SetColor(DEFAULT 0x1000000)`, the view `SetAlpha(GUIConfig::GetAlphaValue(layer 2))` = 0.85. Strip is vertically centred in the frame (`MoveDstTo(0, floor(Height(frame)/2 − Height(strip)/2))` = y 4); paint order = `AddRenderSurface` order: strip, (waypoint), frame (`MoveToFront` after the waypoint is created).
* Scrolling (`FUN_100670f8`, per frame): `N3Msg_GetGlobalCharacterRotation` → quaternion `q`; `v = q · (0,0,1)` (`FUN_10043401`); `a = atan2(v.z, v.x)` (`_CIatan2` 0x10173c64, first operand z), `a += 2π` while `a < 0`; `frac = −(a − π/2)/2π`; strip `MoveSrcTo(GetTextureSize.x · frac − Width(frameDst)/2, 0)` with `GetTextureSize` = inclusive extent 255, i.e. the window's centre column is `255·frac`. Handedness: a character walks along `(sin yaw, cos yaw)` in server x/z (zone.rs `scene_forward`, verified against NPC walks), so `v = (sin yaw, 0, cos yaw)`, `a = π/2 − yaw`, `frac = yaw/2π`: heading 0 (+z) centres N, +π/2 (+x) centres E. The engine's global rotation is server-space (not the mirrored scene space). The one-pixel inconsistency of 255 vs 256 is the original's (test `strip_is_centred_on_the_heading`: S / W land 0.5 / 0.75 px beside the exact column). We feed it `zone.own().yaw` (0 when the server sent no rotation).
* Waypoint: GlobalSignals +0x158 `(Identity playfield, Vector3&, uint)` → `FUN_1006749b` stores it in the view (+0x144 identity, +0x14c vector, +0x158 value, +0x15c flag) and emits two texts of `LDBface::GetText(0x2710)` on GlobalSignals +0x17c (feedback lines; their wording was not read); value 0 clears it and emits one text. GlobalSignals +0x15c `(Identity)` clears it when the identity equals the waypoint's or is 0 (`FUN_10067092`). +0x4c `(int, int, Identity)` stores the current playfield instance (`FUN_100670e5`). The marker is created only while the waypoint's playfield equals the current one. Position (all inclusive extents): `d = waypoint − own position` (`FUN_100679da`), `b = atan2(d.z, d.x)` wrapped ≥ 0, `x = floor(256 · (a − b)/2π + 68)` wrapped into [0, 256] (`+= 256` while `< 0`, `−= 256` while `> 256`), marker `MoveDstTo(floor(x − (Width+1)/2), floor((Height(view)+1)/2 − (Height+1)/2))` = y 6; clipped by the view (only x in 0..137 is visible; a target 90° right sits at 132, left at 4, behind at ≈ 196 = invisible). In compass-bearing terms `a − b = bearing − heading` (positive = clockwise = right). **UNRESOLVED**: who emits the waypoint signal (`/waypoint` is a GUI-local command, `docs/chat/cmd.md`; its handler and the emitting call of GlobalSignals +0x158 were not found: the command table entry has no direct xref and the 0x158-offset scan yields only the connecting `MapData_c` / `CompassView_c` sites), so `Compass::set_waypoint` has no caller outside the tests.
* Tests: `cargo test --release -p aomac hud_compass` (maths) and `hud::tests::compass_shots` (4× crops `compass-{north,east,south,west}.png`, `compass-wp-{ahead,right,behind,other-playfield}.png`).

### 10.2 AGG/DEF slider (`play/hud_aggdef.rs`, `ao_net::n3::outgoing::set_stat`)
* `RightBarView_c` ctor `FUN_1006ff09`: `BitmapView(BOTTOM_RIGHT 0x9a)`, HLayout `[TextView "DEF"][Slider_c][TextView "AGG"]` (`SetBorders` 5 px between), the slider created with `Slider_c(Rect, "", −1, min −100.0 (_DAT_101b5358), max 100.0 (_DAT_101adfa8), 0, 0)` (ctor 0x1014448a; its value starts at `min`), slot `FUN_1006e9c5` connected to its change signal, and `GlobalSignals::GetCharStatSignal(0x33 = AggDef)` → `FUN_1006e968` = `slider.SetValue(N3Msg_GetAggDef(), notify = false)`, called once at construction and on every change of the stat. `N3Msg_GetAggDef` [IF 0x100087bd] → `n3EngineClientAnarchy_t::N3Msg_GetAggDef` [GC 0x10026c2f] = own character `GetStat(0x33, mode 2)`, 0 without a client character. **Default = the stat the server sent** (0 until then); knob at 0 sits at x = 58 (`floor((0+100)·117/200)`), as the old static layout had.
* `Slider_c` internals: background `BorderView` (gfx 0x92 = `AGGDEF_SLIDER_BACKGROUND`, 128 × 18), knob `BitmapView` (0x91 = `AGGDEF_SLIDER`, 11 × 18, tinted DEFAULT). `_Layout` 0x10144141: knob x = `floor((v − min)·((Width+1 − borders) − (Width(knob)+1)) / (max − min) + border)`, y centred. `MouseDown` 0x10143efc: a press inside the knob frame sets the dragging flag (+0x170) and the grab offset = pointer − knob left-top, captures the mouse; `MouseMove` 0x10143f8a: `v = (max − min)·(x − grab − border)/travel + min` → `SetValue(v, true)` (clamped to [min, max], `_Layout`, emits `(v, !dragging)`); `MouseUp` 0x101443b3 clears the flag and emits `(v, true)`. Slot `FUN_1006e9c5(value, …, bool)`: flag false (while dragging) → `SetValue(floor(value), false)` snaps the knob to whole numbers without a signal; flag true (release) → `N3InterfaceModule_t::N3Msg_SetAggDef((float)value)` [IF 0x10008797] = `N3Msg_SetStat(ftol(v), 0x33)` [GC 0x10026c49]. So nothing is sent while dragging, one message on release.
* Message: `SetStatIIR_t` (key `0x6E5F566E`, vtable GC 0x10161370; ctor `FUN_1007737e` stores `+0x18 = value`, `+0x1c = stat`; `Write` `FUN_1007730c` = `i32 value, i32 stat`; `Read` `FUN_1007732f` rejects `stat 0x33` outside −100..100). `N3Msg_SetStat` → `EnqueueSelfMessage` [N3 0x1000624f] → `n3Engine_t::DoExternalInput` [N3 0x100065d1] applies it to the own dynel (we write `Zone::stats[0x33]` at once) and, since the IIR's pass-on byte is 1 (`n3InfoItemRemote_t` ctor 0x10009860 default), `SendIIRToObservers` → `SendIIRToServer` [N3 0x10007762]. `ao_net::n3::outgoing::set_stat(char_id, stat, value)` encodes `6E5F566E 0000C350 <char> 01 <value> <stat>` (test `set_stat_bytes`); `Hud` queues the frame (`take_outbox`), the flow sends it. Verified: `hud::tests::aggdef_drag_sends_set_stat_on_release` (press knob, drag 30 px → value 50, one frame on release, clamp at 100, press beside the knob grabs nothing). The server's reply to a stat change (echoed `StatIIR_t`) is whatever the existing stat path applies. **Not captured live**: no capture of this message exists; the layout is from the DLL only.

### 10.3 Hotbar contents and interaction (`play/hud_bar.rs`)
* First login (`FUN_100d94e9`, GUI 0x100d94e9; the block is guarded by the IndependentPrefs int `IsFirstTime` ≠ 0, then sets it 0 and saves): slots (IPoint x, 0): 0 = `{0xdeb0, 0xc1a5}`, 1 = `{0xdeb0, 0xc1a2}`, 2 = `{0xdeb0, 0xc1a8}`, 3 = a text macro created by `TextMacroSystem_t::CreateMacro("Follow", "/follow", 0, true)` → `{0xc789, id}` (the template's `prefs/NewChar/TextMacro.bin` = `00000001 00000001 00000006 "Follow" 00000007 "/follow"`: id 1), 9 = `{0xdeb0, 0x14124}`. Slot item class `InventoryListViewItem_c` (`FUN_1003d830(type, identity)`), type by identity kind in `FUN_100d78e5`: 1 item, 2 = 0xdac3, 4 nano 0xcf1b, **6 = 0xdeb0 special action**, **7 = 0xc789 macro**.
* Identity kind 0xdeb0 is a special action and resolves to **rdb 1000020** (`GetItemByTemplate` GC 0x10017aae: `if (kind == 0xdeb0) kind = 0xF4254`), i.e. the same template records as items; name = `N3Msg_GetName`, icon id = stat `Icon` (0x4f, `FUN_1003eb30`; fallback GFX 0xb9 `DEFAULT_ICON48`), the image = **rdb 1010008** record `Icon` (`ViewSurface_c::Load(id, 0x80)` → `GuiResourceManager_t::GetRDBTexture` [IF 0x1000b6e7]: "Unable to find the image with id e_RDB_Res_Icon:%d"), PNG 48 × 48 keyed like all GUI textures, stretched to the item view's 32 × 32 bounds (`FUN_1003f2ea`). Records: `0xc1a5` "Start Combat" icon 49565, `0xc1a2` "Walk" 49560, `0xc1a8` "Sit" 49564, `0x14124` "Suspended Animation" 82199 (every record also has the description text; stat 588 = 3/2). Macro: icon GFX 0xe3 (`GFX_GUI_ICON_MACRO`, `FUN_1003eb30` type 7), label `<font color=yellow>Follow</font>` in a `TextView_c` of font 7 (SMALL) centred (`FUN_1003f8e4`, `FUN_1003f2ea`). Icons sit at +2 / +3 px of the 36 px cell (centred in the 38 px art).
* Slot signals (ctor wires `MultiListView_c` signals +0x2d8 drag-begin → `FUN_100d7ec1`, +0x2dc drop → `FUN_100d8047`, +0x2e0 use → `FUN_100d79c9`, +0x2e4 info → `FUN_100d765d`; emitted by `MultiListView_c` item handler `FUN_10040a17`): **use** `FUN_100d79c9`: type 1 `N3Msg_UseItem`, 4 `N3Msg_CastNanoSpell`, **6 `N3Msg_PerformSpecialAction(identity)`** [GC 0x100272fd] → `FUN_1004256c` (looks the identity up in the character's special-action list `+0x84`, `Feedback_ActionIsNotAvailable` when absent; then by the entry's `Action_e`: 0x4e / 0xb `DefaultAttack`, 0x4c / 0x4d `SitToggle`, 0x4f `MovementChanged(0x24)`, 0x51 / 0x52 camping, …, docs/zone/combat-net.md §5.3), **7** emits the macro text on GlobalSignals +0x180 (typed into the chat input). **Info** (Shift+click) `itemid://kind/instance` into `InfoViewModule_c::ShowURL` (not for macros). **Drag** begins unless DValue `LockHotbars` (false in `CharPrefs.xml`); drop on a slot of this bar = `MultiListView_c::MoveItem`, the displaced item starts a new drag; after any drag `FUN_100d76cf` deletes the item if it still sits in its source slot, so dropping outside the bar removes the shortcut. Icon timers (`IconTimers`, `IconTimerText` true): `N3Msg_GetActionProgress` overlay for type 6; item disabled tint from `N3Msg_GetSpecialActionState` (`FUN_1003ebd0`).
* Ours: `ShortcutBar` paints the bar canvas (chrome + 10 slot frames + icons) under ten 36 × 38 `slotN` canvases (+ `labelN` text views) that carry tooltip (item name, the `ShortcutShortTooltips` = true form; long form UNRESOLVED), click and the drag source/target. A left press+release within 3 px on a slot = **use** (`SlotUse` queue → the flow runs macros through `Chat::run_line`; special actions are queued with their `Action_e`). Drag past 3 px with the left button starts a drag (ghost window with the icon), drop on another slot swaps them, drop outside deletes (`LockHotbars` honoured). Tests: `hud::tests::hotbar_first_login_and_interaction`.
* **Remaining gaps**: (1) the `Action_e` of each first-login identity (`bar::action_of`: 0xc1a5 → 0x4e, 0xc1a2 → 0x4f, 0xc1a8 → 0x4c, 0x14124 → 0x51) is an **inference from the names** and the `FUN_1004256c` table — the character's special-action list (`+0x84`, server-filled, which also decides enabled / disabled) is in no capture and `N3Msg_GetCorrectActionID` (`FUN_100d7575` rewrites type-6 identities with it on load) is not evaluated; (2) special actions have **no consumer yet**: the flow logs `hud: hotbar special action …` because movement (`SitToggle`, `MovementChanged`) and combat (`DefaultAttack`) are not plumbed into `flow.rs` (their messages would be `CharDCMove` / the attack IIR, docs/zone/outgoing.md); (3) use fires on press+release (`CanvasClick`), the DLL's press/release split in `FUN_10040a17` (`UseNewMouseFunc` true, a `+0x319` flag) was not fully resolved; (4) displaced-item re-drag, Shift+click info window (no info view), icon timers / disabled tint, bar up/down buttons, radio bar selection and persistence (`position_map` / `IconPositions.bin` / `TextMacro.bin` writing) are not implemented; (5) item (type 1) and nano (type 4) slots need an inventory / nano list we do not have, so only types 6 and 7 render.

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
* Keys (GUI.dll default hotkeys, `InputConfig_t::SetDefaultHotkeys` 0x1001ad88 text, `ParseFile` 0x1001ac19 parses the built-in table, the file name `hotkeys.txt` is not read): `TAB` next hostile,
  `SHIFT+TAB` previous hostile, `CTRL+TAB` next friendly, `CTRL+SHIFT+TAB` previous friendly → `TargetingModule_t::Get{Next,Prev}{Hostile,Friendly}TargetMessage` 0x10025adc… →
  `N3Msg_GetCloseTarget(current, friendly, forward)` (Gamecode 0x1001c411) → `SetTarget(id, false)`. **Order** (decompiled): candidates = dynels within **100 m** (`_DAT_10155eb0`) of the own
  character except it and the current target, visible, stat `InPlay` (0xc2) ≠ 0; class = `Side` (0x21, `0x29c` in a battle station), players in the own team and pets (stat `PetMaster` 0xc4) of
  the own team count as the own class; *friendly* = same class, *hostile* = different. Metric `m = d² − d²(current)` (`FUN_10023a81` is the squared length; 0 without a current character
  target), `+1000` (`_DAT_10157870`) once if negative or if 0 for a candidate listed before the current one; next = the smallest `m` (start `FLT_MAX`), previous = the largest (start
  `FLT_MIN`). So Tab walks outwards by distance and wraps. `hud_target::cycle`; not modelled: team/pet exceptions, `InPlay`, the list order of the locality query (ids are used).
* **The click** (found): `ActionViewMouseHandler_c` (ctor `FUN_1002c66b`, GUI) connects `WindowController_c` mouse signals: press `FUN_1002c2ee(Point, button, clicks)`, release
  `FUN_1002c469(Point, button)` and `FUN_1002c14d` (ends the look, `FUN_1002c0e5`), and `GlobalSignals` mouse-move `FUN_1002c17b` (accumulates the pointer path in `+0x1c` while a look is
  active, `MouseTurnSensitivity`). Release, with no drag object and `(look inactive || path < 0.02)` (`_DAT_101aeaf4`; units raw counts / 1000 = 20 counts; `play/controls.rs` has the same rule):
  the object under the mouse is `InputConfig_t+0xb0` (set by `CheckObjectUnderMouse`); void (`0x9c47`, `0x9c52`, 0) = **nothing happens (no click-on-ground deselect exists)**. Left button, no
  Ctrl-on-a-character: no Shift → `Send(0x1e, 0x126, N3Msg_GetNextTarget(current target))` (= `n3Camera_t::GetNextTarget` N3 0x10020723: the object after the current target in the camera's
  hit list `+0x244`, wrapping; the first one if the current target is not in it; void → the hovered object), Shift → `InfoViewModule_c::ShowURL("charid://50000/<id>")` / `itemid://` (no
  selection); Ctrl on a character → `Send(0x1e, 0x126, id)` + `N3Msg_SwitchTarget(id)` (= `DefaultAttack(id, true)`). Right button: character → `N3Msg_DefaultActionOnDynel` (Gamecode
  0x100291da: item/corpse pickup, trade, use), else `N3Msg_UseItem`. Press: a left **double click** on an object other than the own character runs `DefaultActionOnDynel` when
  `DoubleclickAction` (LoginPrefs, default true); with `LMBMouseLook` (default true) the left press arms the camera look. Message 0x126 on module 0x1e = `TargetingModule_t::SetTargetMessage`
  (`SetTarget(id, false)`: only if `N3Msg_isIDOnGround(id)`). The deselect `SetTarget(0,0)` (`RemoveTarget` 0x2598b) comes from `FrameProcess` (dynel gone) and from the AFCM messages 0x112/0x113, which
  `FlowControlModule_t::StopAllActionsMessage` (GUI 0x10027de4), `TeleportStartedMessage` (0x1002910e) and Gamecode (`FUN_1006880d`, `FUN_1002f009`, `FUN_1005cb6a`) send; no mouse path.
  `hud_target::{pick_all, click_target, world_click}`; the modifier branches and right/double click need modifier state in mouse events (not reported yet) and the combat sends: not wired.
* The pick: `InputConverterModule_t::InputGameSubmodeActionViewONMessage` 0x1001bcae (every frame) → `N3Msg_SetMousePos` (Gamecode 0x1001613b) → `n3Camera_t::SetMousePos` (N3 0x10020571:
  ray `(tan(fov/2)·x, tan(fov/2)/aspect·y, 1)` through the camera matrix, scaled by `VisualCamera_t::GetLengthOfViewcone`) → its `n3CameraCollLine_t` (`+0x180`) collides with every visible
  `n3VisualDynel_t` (`FUN_10020a3c`: `vtable+0x94(line, BoundingBoxTargeting, …)`, pref **`BoundingBoxTargeting` default true** = the dynels' bounding boxes, the client character excluded
  unless the camera is third-person) → the hit list `+0x244`; `GetObjectUnderColLine` = the current target if it is in the list, else the first entry. The mouse world position is the
  line's hit (`+0x190`). We have no bounding volumes of the dynels: `hud_target::pick_all` intersects the ray with one capsule per dynel ([`CAPSULE_HEIGHT`] 1.8 m, [`CAPSULE_RADIUS`] 0.5 m,
  UNRESOLVED GUESS) and orders the hits by distance ([INFERENCE]).
* **Hover pointer** (`InputConfig_t::CheckObjectUnderMouse` 0x10019f00 → `MousePointerModule_t::SetMousePointer(Pointer_e, DetailLeft, DetailRight)`, drawn by the software cursor
  `MousePointerModule_t::FrameProcess` 0x100202dd / `FUN_10020120` at `InputConfig_t+0x118`, honouring `MouseCursorMode` 2 detailed / 1 simple / 0 none): the sprite table @0x10263178
  (`GFX_GUI_POINTER_STANDARD` 0x135, `_BLUE` 0x136, `_GRAY` 0x137, `_PURPLE` 0x138, `_YELLOW` 0x139, `_GREEN` 0x13a, `_RED` 0x13b, `4WAY_DRAG` 0x13c … `WAIT` 0x143), details @0x102632a8
  (`POINTERDETAIL_` OPERATE 0x144, DENIED 0x145, ATTACK 0x146, PICKUP 0x147, LOOKAT 0x148, QUESTION 0x149, TALK 0x14a, TRADE 0x14b), overlays @0x1026313c (`POINTEROVERLAY_LEFTCLICK/RIGHTCLICK/
  BOTHCLICK` 0x14c–0x14e); table rows are `{hotspot a, colour 0xffffff, gfx id, sprite, hotspot b}`. `CheckObjectUnderMouse` runs every frame on the object under the mouse: type `0x9c47` sets a flag
  and leaves; `0x9c52` / 0 leave; a character (50000): the own character → `SetMousePointer(2, -1, -1)`, others → `Pointer_e` 1…5 and details 6 / 7 chosen from `IsNpc`, `GetSkill(0xe0)`
  flags (0x20000000 / 0x4000001), `HasVulnerableFightMode`, the `Side` comparison (stat 0x21) and the own-team test, the right detail 7 only with `DoubleclickAction`, cleared by Ctrl; any other
  object (items, doors): by its skill 0x1e flags (bits 0 / 3 / 0x100000 → `Pointer_e` 1 with a detail or a `Send(0x1e, 0x126)`, see the click). The exact `Pointer_e` → colour sprite and detail → art
  mapping was not traced (UNRESOLVED). **Not implemented**: this client uses the OS cursor on every screen and `ao_render::Host` has no way to hide it for a software cursor.
* The **selection indicator** (found): not a ground decal. `SetTarget` creates `Indicator_t(id, attacking = 0, health = 1)` (`FUN_100255be`, vftable 0x101ae0d4) unless the target has skill flag
  0x400: a world billboard `VisualSprite_t(width/128, 0.3, "[3] TargetIndicatorMat")` (priority 6) over the head anchor `N3Msg_GetIndicatorPosition` (same anchor and text rules as the name tags,
  docs/zone/motion.md §6) showing a 128×32 (256 wide for long text) plate: the left and right half of `GFX_GUI_INDICATOR_SELECTED` (0xe6, corner brackets; 0xe5 `…_ATTACKING` for the attacked
  dynel) at its ends, the tag line (`FUN_10024e14`: `** name **` for GM flags, `= name =`, titles, breed, colour by flags / `Consider`) in font 2, and the 64×4 health bar of `FUN_10024c03`
  (y 14..18, `ftol(Health/MaxHealth·64)` px, rest 0x333333). Implemented as `hud_target::selection_indicator` (called from `flow.rs` after the name tags): drawn 1:1 in GUI pixels centred on the
  projected anchor, not perspective-scaled; bar colour white (UNRESOLVED `Consider` gradient); no clan line; the attacking indicator needs the fight state.

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
  `N3Msg_CanAttack` (not modelled).
* **Dock handlers** (disassembled, GUI): left arrow `LAB_10072fcb` = `AFCM::Send(0x1e, flag ? 0x106 : 0x105)` (`GetPrev{Hostile,Friendly}TargetMessage`), right arrow
  `LAB_10072ff1` = `Send(0x1e, flag ? 0xe9 : 0xe8)` (`GetNext…`), target button `LAB_10073017` = flag ? `N3Msg_PerformSpecialAction(0xb)` (**Attack**,
  docs/zone/combat-net.md §5.3; the HUD does not send it yet) : `TargetingModule_t::SelectSelf`. (`FUN_1007303d` only sets the button's toggle value from a `GlobalSignals` int.)
  `hud_target::cycle` is `N3Msg_GetCloseTarget` (Gamecode 0x1001c411, see §13.2).
* **No tooltips**: `FUN_100746ec` (`CCTargetControl_c`), `FUN_10073598` (`TargetHealthBar_c`), `FUN_10073884` (`TargetHeader_c`) and `FUN_10075342` (`TargetTargetButton_c`) never call
  `View::SetToolTip` (searched every decompiled function 0x10072000–0x10075fff: the only `SetToolTip` is `ToolbarButton_c` `FUN_10072b33`), so the target windows show none.
* **Target-of-target button** (`TargetTargetButton_c`, `FUN_10075342`; first child of the *hostile* bar window, `View` flags 0x100): `BorderView` with the header's frame gfx
  (0x88/0x89/0x86/0x87), `SetColor(0x7fffff)`, a vertical layout, caption `"Fighting Target:"` (`TextView`, borders 3,3,3,0) and the name (borders 3,0,3,3), window border 5 below.
  `FUN_10073b9e(show)`: only when the pref `Targetstarget` (`LoginPrefs.xml`, "Show Target's Target", default **false**) is set and `N3Msg_GetTargetTarget(target)` (the fight
  controller target of the selected dynel, `SimpleChar+0x1d4` `+0x4c/0x50`; Gamecode 0x1001641d) is non-zero: name = `N3Msg_GetName`, button shown with the window. Click `LAB_10073554`:
  if `N3Msg_CanClickTargetTarget(target, targetsTarget)` (Gamecode 0x10016451, `hud_target::can_click_target_target`) → `Send(0x1e, 0x126, targetsTarget)` = select it. Implemented
  (`HudTarget::targets_target`, `tot`, `Zone::fight_target` from the relayed `AttackIIR_t` / `StopFightIIR_t`). Caption colour: the `0xff4444` of `FUN_10073d0f` belongs to the
  selection/fight header logic of the whole `CCTargetControl_c` (UNRESOLVED), the button's caption keeps the default text colour.
* NOT implemented: the NCU windows (`NanoTargetNCUWindowConfig`, `FightTargetNCUWindowConfig`), the fight-target variant of the bar windows (`FUN_10073d0f`: the second bar/header pair
  `+0x130/+0x134`, captions "Fighting Target", "Nano Target", red caps), the attack send of the hostile button.
* Data: `Zone.target` (instance id), `Zone.fight_target`, `DynelState { side, level, health, max_health }` (from `SimpleCharFullUpdate` and other characters' `StatIIR_t`). Tests in
  `hud_target.rs` (ray math, capsule pick, hit-list click cycling, close-target metric, `SelectSelf`, target-of-target, windows follow the selection, `AOMAC_SHOT_DIR` screenshots
  `target-hostile.png`, `target-friendly.png`; the dock arrows are drawn since the id table fix, §1).


## 12. Map windows (`play/hud_map.rs`, `ao_formats::{planetmap, topdown, landcontrol}`, `ao-gui` `CanvasView`)

Both windows are built in code in GUI.dll (no `Views/*.xml`): `ControlCenterModule_c::SetupProviders` 0x10068c38 registers `planetmap_window` (text `#WindowPlanetMap`, `WINDOW_PLANETMAP`) and `map_window` (`#WindowMap`, `WINDOW_MAP`); menu entries `ActionMenu/MapMenu.xml`. Help text (`text/help/The Map Window.html`, `The PlanetMap Window.html`): Map = **Ctrl+6**, Planet Map = **P**; yellow dot = you; map upgrades turn it into an arrow and show machines/monsters/people; drag pans, moving the character re-centres; zoom in/out and "centre on character/mission/shop" buttons on the planet map. (Hotkeys NOT wired: `ao_gui::Key` has no digit key and `Hud::input` has no key map yet.)

**Planet map** (`PlanetMapView_c` ctor 0x1004d26a): default client 300×400 (`_DAT_101b183c/1840`), first-time frame `Rect(100,100,500,500)`; buttons `ZoomIn`/`ZoomOut`/`Character`/`Quest` (`Button_c`, art `GFX_GUI_PLANETMAP_ZOOM_IN/_OUT/CENTER_PLAYER/CENTER_MISSION`, 27×27), marker `GFX_GUI_PLANETMAP_PLAYER_MARKER`. Index file option `PlanetMapIndexFile` default `Normal/PlanetMapIndexNormal.txt` (`ShadowlandMapIndexFile` → `Shadowlands/ShadowlandsMap.txt`). `FUN_1004bc1f`: zoom level clamped to `0..levels`, ZoomIn disabled at the last level, ZoomOut at 0; initial level 0. Marker position `FUN_1004b81a` (see formats.md *Planet map*); marker hidden when the playfield has no coordinates entry (indoor playfields: 4604, 4582-style).
**Playfield map** (`PlayfieldMapView_c` 0x100eb905 → `ScrollView_c` + `PFMapRenderer_c` 0x100eaebe + `MapControlView_c` 0x100ec4e0): default client 179×199; "Map Not Available" text (`FUN_100eacda`) when no ground map and no `GetMapCharacters` success. Icon atlas (`FUN_100e9ba2`): 16×16 cells of gfx 0x10a.. (MAP_ARROW, ARROW_SELECTED, BIG_ARROW, towers). `MapData_c` asks `GetMapCharacters(range 500)` (`FUN_100425eb`), `GetLandControlBitmap`, `GetCurrentRoom` (`FUN_100428e1`/`FUN_100427a1`: current room white, visited grey 50 %, unexplored black — implemented). Heading = `atan2` of `GetGlobalCharacterRotation` (`FUN_100e9f73`).
**Implemented**: CanvasView-based windows, planet tiles/zoom/pan/recentre/markers/Shadowlands pick, playfield ground image (top-down raster, worker thread) with exploration lighting, yellow dot / heading arrow (when stat `MapNavigation`≠0) and nearby-character squares. Tests: `cargo test --release -p aomac hud_map` (+`AOMAC_SHOT_DIR`: `map-arrival-hall*.png`, `map-icc-shuttleport*.png`, `map-newland-city*.png`, `planet-566-level0/1.png`, `planet-shadowlands.png` inspected: tiles/outlines line up, marker at Newland, buttons row, dark exploration shading in the dungeon).
**UNRESOLVED GUESSES**: playfield map screen scale (1 px/m; renderer scale `FUN_100e923a` not decoded); PF window first-time position; whether `MapNavigation` (140) is what makes `GetMapCharacters` succeed; rule for owning an outdoor map (MapAreaPart1–4 stats vs rdb 1000008) — outdoor maps are always shown, so "Map Not Available" is unreachable; planet buttons: pressed art, tooltips ("Zoom in" text.mdb 10000 key 31206174 / body 193692387; "Zoom out" 230869204 / 159921171) and double-click zoom not wired; `Quest`/shop centring needs mission data; window title text; ShowButtons default (shown here); wheel zoom is our addition.
**Engine additions**: `<CanvasView>` / `Kind::Canvas` / `Gui::set_canvas` / `canvas_size` / `open_framed_window_xml`, events `CanvasDrag`, `CanvasWheel`, `CanvasClick` (`gui/canvas.rs`).

## 11.8 Skill rules (`ao_formats::stats::skills`, tested against the client's rdb) — HudStats2
`GetSkillMax` `FUN_100651b5`, `GetSkillCost` `FUN_10061fdb`, `GetSkillCostLevel` `FUN_10062398`, ability max `FUN_100626d8`, trickle-down `FUN_10064ac2`, `N3Msg_GetSkill` mode 1 `FUN_100654e1` [GC] are ported; the float math is read from the asm (`FUN_1013ecf0` = truncation). Tables (little-endian i32 streams, loaders `FUN_100c646d` dispatch): rdb 1000206[prof] = 77 cost factors in the stat order `COST_ORDER` (`FUN_100c4afd`); 1000209[class 1..5] = `T1..T7`, `B`; 1000200[prof] = title-level start levels `s[4(t-1)]` = 1,15,50,100,150,190,205; 1000204[stat 100..168] = 6 trickle percentages; 1000027[breed] = base/cap abilities; 1000210[breed] = ability cost class (stream order Str,Int,Agi,Sen,Sta,Psy). Cost of one more point from value n: skills `trunc(f·n/10)`, abilities `class·n` (`FUN_100f90ef`). Max skill = min of the T/B/title-level terms, `8·trickle`, … (see code). **UNRESOLVED**: modifier/buff containers (`FUN_1006460b`, `FUN_10063d39`, mask-4 map `FUN_1008a3e8`) – buffed = base.
* **Save Changes** (`FUN_100fa5dd`) → `N3Msg_ClientIPAdjust` [GC 0x10026e7e] → `SkillIIR_t` `3E205660`, pass-on byte 0, body `i32 n; (i32 stat, i32 pending+raw)*` ascending: `ao_net::n3::outgoing::skill_ip_adjust` (byte test). **Reset**: "Reset all skills" opens dialog `SkillResetAll`, accept (`FUN_100f9203`, index 0) → `N3Msg_ResetSkill(0)`; "Reset this skill" (`FUN_100f9220`) → `N3Msg_ResetSkill(stat)` = `CharacterActionIIR_t` action `0x9a` [GC 0x1001cdae]; param placement (`{0,stat}`) not verified, encoder not added.
* **Suggested IP distribution** (`FUN_100fac49`): rows grouped by `ipdist.xml` `pri` (`ip_priorities`; pri 0 bucket skipped, ascending: Soldier pri 1 = Str/Agi/Sta/BodyDev/Assault Rifle…), round-robin +1 per row (`FUN_100fde49(1)`, allowed iff `raw+pending+1 ≤ max` and IP left) until IP is out. Row UI (power bar `0xe0/0xe1`, `GFX_GUI_INC_SKILL[_CHANGED]`/`DEC`, value green/red when pending) traced (`FUN_100fe956`, `FUN_100fde49`) but **not wired into the window yet**.

## 11.9 Stat window `stat_window` (`StatView_c` `FUN_1007ff6e`) and inventory data — HudStats2
Menu `CommandMenu.xml` entry `stat_window` (SetValue toggle), key Ctrl+9 (`WINDOW_STAT`); NewChar template: open, docked 4th page (279 px) of `RollupArea`. Layout/handlers decoded in `play/hud_stats/stat_view.rs` (header "Name:/Level:", bars health 27/1, nano 214/221, XP, alien XP 40/178 if Expansion&0x18, PvP 684/682/683 with `pvp_score_for_rank`, rows 22,91,90,92,96,93,94,95,97); the file is **written but not wired into `HudStats`/`WindowKind::Stat` and not built or screenshot-tested**.
Inventory: `FullCharacter` inventory is now decoded (`InventoryEntry{slot,a,b,id,AcgItem{low,high,level}}`, `Zone.inventory`; layout from `FUN_1002a41a/1002a04a`, `GameData::operator>>(ACGItem_t)` 0x1000e9d7; no capture has items → synthetic-bytes test). Slots: 0..0x3f equipment pages, bag from 0x40 (30 slots); cell id→slot: weapons `id` (Right/Left hand 6/8), armor `0x10+id`, implants `0x20+id` (loop 0x20..0x2f `FUN_10046e35`), social `0x30+id` (hands 13/15 → 0x3d/0x3f, `FUN_10047873`). Wear tab tables (corrects §11.4): weapons = order `1,15,2,3..14`, armor `1..15`, implants `1..13`, social `1..15`. Item icon = rdb 1000020[low_id] stat `Icon` (79) → rdb 1010008 PNG (48×48/32×32). Not done: icon drawing, list mode, Social tab, inventory columns, window positions (NewChar: inventory DockArea0 Rect(1196,636,1382,811); wear/stat in RollupArea).
