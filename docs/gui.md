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
