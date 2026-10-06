# Chat window GUI (`crates/aomac/src/play/chat/win.rs`)

Faithful port of the window layer of `ChatGUIModule_c` (GUI.dll `0x1008751b`). Addresses are GUI.dll; function names without a symbol are
Ghidra `FUN_<addr>`. Anything not read from the binary is marked **GUESS** / **UNRESOLVED**. The chat-server wire layer is
`docs/chat/net.md` (`ChatMsg` / `ChatLine` come from the hub); this file is what happens to a message once it reaches the windows.

Decompile dumps used (not committed): `ChatGUIModule_c::Initialize` 0x100872bd, `AddGroup` 0x10085f91, `HandleGroupMessage` 0x100863ff,
`HandleVicinityMessage` 0x10086728, and the unnamed window classes listed below.

## 1. Object model

| class (RTTI name) | ctor | role |
|---|---|---|
| `ChatGroupController_c` | `FUN_10083d84` (singleton `FUN_10083e10`, `DAT_10276274`) | registry of groups (`FUN_10083be4(name, id, 0, kind, 1)`) |
| `ChatGroupNode_c` | `FUN_1008526c` | one group: name `+0x28`, id lo/hi `+0x48/+0x4c`, window count `+0x58` |
| window object (`FUN_1009dc28`, 0x218 bytes, config `Chat/Windows/WindowN/Config.xml`) | `FUN_1009427a(…, name, isStartup)` | one chat window "document": group selection, output group, flags |
| `ChatWindow_c` (a `Window`) | `FUN_10097ae3` | the GUI window: `Window::Window(Rect(0,0,400,200), "", "", style 0, flags 0)` (400 = `_DAT_101b1840`, 200 = `_DAT_101a959c`), tabs |
| `GroupChatView_c` (a `BorderView_c`) | `FUN_100abfa3` | one tab; `SetGfx(0 ×9)` (no art), `SetClient(chatview, 3,3,3,3)` (`_DAT_101a96d4` = 3.0), local colour `DEFAULT` 0x1000000 |
| `ChatView_c` | `FUN_10090893` | text area + input bar |
| `ChatTextView` (a ScrollView) | `FUN_100925ff` | `ScrollView` with `SetVScrollBarMode(3 = always)`; text view flags **0xe6c** = ENABLE_SHADOW\|FILL_BOTTOM_UP\|DISABLE_RC_MENU\|WORD_WRAP\|MULTILINE\|ALLOW_TEXT_SELECTION\|ACCEPT_MOUSE_INPUT; view flag 0x80; `SetShadowOffset(FUN_1008d673)` |
| `InputBar_c` | `FUN_100919ab` | editor `TextView` (flags **0x800f**, `+0x178`) and a prompt overlay `TextView` (flags 0x800, alpha `_DAT_101b83f0` = 0.6, `+0x17c`), both shadow offset `FUN_1008d673` |

## 2. Window frame and visual modes

`visual_mode` (`+0x16c`, menu strings `ChatWindowMenu_Style_Mode_Normal` / `_Border`, setter `FUN_100993c0` → `FUN_10096ec5` 0x10096ec5):

* **0 = Normal**: `Window::SetStyle(0, flags & 0x300)` (style 0: border Rect(3,7,3,3), tab strip with the window's title, docs/gui.md §6) and `FadeTo(1.0)`.
* **1 / 2 = Border**: `Window::SetStyle(3, (flags & 0x300) | 0xc3c)` — style 3 has **no frame art**, flag 0x4 → no icon/close/pin buttons
  (docs/screens.md §9); the window is a borderless rectangle. **2 is the shipped default** (both default windows).
  What it shows is decided by the *mode-dependent look of the `GroupChatView_c`* — `FUN_100aab08(mode)`, called from the mode setter (`FUN_10096ec5`) for a one-tab window:
  mode 0: `BorderView_c::SetGfx(0 ×9)`, client borders 3 (`_DAT_101a96d4`), `ChatView::SetBorder(true)` (`FUN_1008daa0(1)`); mode 1: `SetGfx(0x1bc,0x1be,0x1b7,0x1b9,0x1ba,0x1bd,0x1bb,0x1b8,0x1bf)`
  = `GFX_GUI_WINDOW3_BORDER_*` + `GFX_GUI_WINDOW_BACKGROUND`, client borders 3, `SetLocalAlpha(0.4)` (`_DAT_101b9e78`), `SetBorder(true)`; **mode 2: `SetGfx(0 ×9)`, client borders 0,
  `SetBorder(false)`**; all three then call `FUN_1008e4f0(1)` (stores `+0x1b8`, forwards to the text view `FUN_100929ae`). `ChatView::SetBorder` (`FUN_1008daa0`, field `+0x1b9`, ctor default true) reconfigures
  the text area's `BorderView_c` (`+0x1a0`) **and** the input bar's (`+0x1a4`): `true` → `GFX_GUI_INSET_{TL,TR,BL,BR,LEFT,TOP,RIGHT,BOTTOM}` (0xf9,0xfb,0xf4,0xf6,0xf7,0xfa,0xf8,0xf5, bg 0), client margin 5
  (`_DAT_101a8b98`); `false` → **no border art, bg `GFX_GUI_WINDOW_BACKGROUND` (0x1bf, black), client margin 0**. So the shipped borderless windows are two black panels (text area, input bar)
  that the window alpha (0.8 active / 0.3 inactive) fades together with the text; the port used to draw the inset outline without any fill there (the "transparent chat" of the live harness).
* Window alpha (modes ≠ 0): `FUN_10096b23`: the selected tab's window is *active* (`+0xda`) → `Window::FadeTo(window_transparency_active, 0.2 s)`
  else `FadeTo(window_transparency_inactive, 1 s)` (200000 / 1000000 µs). Defaults 0.8 / 0.3 (`_DAT_101b8a54` = 0.8, `_DAT_101ae0ec` = 0.3);
  the menu's two sliders write them (`FUN_10096db3` / `FUN_10096de4`).
  *Port*: `Gui::set_window_alpha` multiplies everything drawn in the window; active = the window's input bar has the keyboard focus. The fade scope is the *whole* window, text included:
  `Window::FadeTo` 0x10155856 stores the target, `Window::UpdateFadeLevel` 0x10155653 interpolates and calls `View::SetAlpha(rootView, base × level)` on the window's root view (and the tab views),
  so an inactive default window really is 30 % text on a 30 % black panel (checked, not a port artefact). Mode 0 calls `FadeTo(1.0)` (no fade).
* `is_frontmost` / `is_backmost` (chat_window_config) set window flags 0x100 / 0x200 (`FUN_10097ae3`): ported as `Gui::set_window_layer` (1 / -1), toggled by the menu entries *AlwaysOnTop* / *AlwaysBehind* (§12) and persisted.
* Mode 0 (framed "Normal") is ported: a style-0 `ao_gui` window with the tab strip (docs/gui.md §6.1), movable / resizable by its frame, one tab per chat window (§10, §11). The Window menu's *Normal* entry switches to it.

## 3. Layout inside the window (`ChatView_c::FUN_1008d728`, input mode set by `FUN_10090442`)

Input mode = `is_textinput_enabled ? 1 : 0` (`FUN_100abfa3`); mode 2 (multi-line, flags |= 0x860, `input_height` default **50** = `_DAT_101b20a8`) is a menu option.
Mode 1: input bar height = `InputBar_c` preferred height + 2·5 border; text frame bottom = input top − **5.0** (`_DAT_101a9da0`, double).
`TextView` margins inside the BorderView: 5. Port: `win.rs::open` builds this as view XML (`open_window_xml`), input height = CHAT font height + 10.

## 4. Group selection: which lines go into which window

Window config keys (`FUN_1009a77b` writer, `FUN_1009dc28` reader): `name` (title/tab: `FUN_100ab980` = `name`, red if `+0x160`, optional `" <font color=green>[outputgroup]</font>"`
when `ChatShowOGrpInTitleBar`), `window_name` ("WindowN", dir name), `is_startup_window`, `is_default_window`, `tab_index`, `is_window_open`,
`is_autosubscribe_window`, `is_message_fading_enabled`, `is_logged` (appends `<window dir>/Log.txt`), `is_clickthrough`, `is_textinput_enabled`,
`deactivate_on_send` (default **1**), `hide_input_when_inactive`, `show_timestamps`, `window_transparency_active/inactive`, `output_group`
(`#%016I64x#`, `GetGroupIdentifier` 0x1001b5d5), `visual_mode`, `selected_group_names` + `selected_group_ids` (Int64), `chat_window_config/WindowFrame`.

**Selection semantics** (`FUN_1009dc28` 0x1009dc28, `FUN_1009d129`, `FUN_1009ce7b`): every registered group gets flag = `autosubscribe`; each *listed* group
flips it to `!autosubscribe`; afterwards flag false → `FUN_1009d129` (select), true → `FUN_1009ce7b`; both insert into the window's set exactly
for the listed groups, so the set always equals the file's list, and the two functions differ only in the sign convention:
`FUN_1009d129`/`ce7b` toggle membership **inverted for autosubscribe windows**. Result (consistent with the shipped files, below):

> a window shows group *g* ⇔ `(g ∈ selected_group_ids) XOR is_autosubscribe_window`.

An autosubscribe window therefore shows **every group not in its list**, including channels announced later (`FUN_10083be4` → new group → `FUN_1009d589`
auto-adds to those windows); a normal window shows only its list.

**Default windows.** (a) Shipped: `client/prefs/NewChar/Chat/Windows/Window1|2/Config.xml` (read-only data; the first run copies them):
* `Window1` "Default Window": startup, autosubscribe, `output_group #0000000040000002#` (Vicinity), text input on, `show_timestamps` **true**, mode 2, 0.8/0.3,
  excludes `0x41000001` (Other Pets) and 21 combat groups (0x42000001…0x42000017 minus 4/0x10/0x13, 0x4200001b, 0x42000018, …) — i.e. it shows Vicinity, System,
  Tell Messages, Your Pets, Research, Loot-team, and every chat-server channel. Frame `Rect(277,1206,1273,1439)`.
* `Window2` "Combat": not autosubscribe, no text input, no output group, timestamps off, lists 17 combat groups. Frame `Rect(1273,1206,2303,1440)`.
(b) Code defaults when no window exists (`FUN_10094c58` 0x10094c58): "Default Window" = `FUN_1009427a(0, name, 1)` + `FUN_1009d129` on `0x41000001, 0x42000001..0x4200000a, 0x4200000d..0x42000017,
0x4200001b, 0x42000018`; "Combat" = `FUN_1009427a(0, name, 0)` + `FUN_1009d100` on `0x42000001-3, 5-9, 0xd-0x12, 0x14-0x18` (no frame → `MoveToCenter`, 400×200).
`ChatLastActiveWindow` (CharPrefs.xml) names the window Enter activates (`FUN_10094ac9` / `FUN_10094cfa`).

**Local group table** (`FUN_10083e53` 0x10083e53; id → name; high byte = type): Vicinity `40000002`, Research `4200001c`, System `40000001`, Tell Messages `40000003`,
Your Pets `41000000`, Other Pets `41000001`, Me hit by nano `42000002`, Your pet hit by nano `42000003`, Other hit by nano `42000004`, You hit other with nano `42000005`,
Me hit by monster `42000006`, Me hit by player `42000007`, You hit other `42000008`, Your pet hit by other `42000009`, Other hit by other `4200000a`, Me got XP `4200000b`,
Me got SK `4200000c`, Me hit by environment `42000001`, Your pet hit by monster `42000011`, Your misses `42000012`, Other misses `42000013`, You gave health `42000014`,
Me got health `42000015`, Me got nano `42000016`, You gave nano `42000017`, Team Loot Messages `4200001a`, Vicinity Loot Messages `4200001b`, Me Cast Nano `42000018`
(`win.rs::LOCAL_GROUPS`). `MutedChatGroups` (CharPrefs archive) and `ChatFilterEnabled`/`ChatFilterRules` (MainPrefs, V8 regexp over the text, default off) filter in `FUN_10084f9e`: ported (`filter.rs`, `/filter`, docs/chat/dialogs.md §6); `MutedChatGroups`: **not ported**.
Chat-server groups: `(type << 32) | id` (hub), type byte = `hi & 0xff`.

**Message routing in the handlers** (`HandleVicinityMessage` 0x10086728): data byte 4 → group `0x41000000`, 5 → `0x41000001`, 6 → `0x4200001b`, 7 → `0x4200001a`, otherwise
`0x40000002` with the byte (1 whisper, 2 shout, 3 emote, else plain) kept as `kind`; `HandleGroupMessage` 0x100863ff uses the announced group id; both call
`FUN_10084f9e(senderId, senderName, text, kind, 0, flags)`; flags bit 0 = **GM** sender (not "ignore bypass": `FUN_1009b4cf` prints `(GM)`; the ignore check
`IgnoreSystem_t::IsCharacterIgnored` is skipped for flagged senders). `ChatIndicator` (IndependentPrefs) flashes the head (`N3Msg_FlashHead`); voice sounds
`VoiceSndFxHear{Vicinity,Guild,Team}On` are separate.

## 5. Line format (`FUN_1009b4cf` 0x1009b4cf = window "add message")

Arguments: group node, sender name, text, kind (1 whisper, 2 shout, 3 emote), colour code (0 = group colour), flags. Output HTML:

```
<div indent=wrapped><font color=COLOUR>[(HH:MM) ]PREFIX TEXT</font></div>
```
* `COLOUR`: colour code ≠ 0 → `ColorCodeToHTMLColor` 0x10087860 (table `CCNoneColor…` at 0x10268d38, names of TextColors.xml `CC*`, unknown → `white`), else **`FUN_10085320`**:
  `40000001 ct_system`, `40000002` by kind (`ctch_whisper`/`ctch_shout`/`ctch_emote`/`ctch_vicinity`), `40000003 ctch_tell`, `41000000 ctch_mypet`, `41000001 ctch_otherpet`,
  `4200001c ctch_research`; otherwise by type byte: 1 `ctch_admin`, 3 `ctch_clan`, 4 `ctch_misc`, 5 `ctch_gm`, 8 `ctch_news`, 10 `ctch_tower`, 0x0e `ctch_pgroup`, 0x82 `ctch_team`,
  0x86 `ctch_seekingteam`, 0x87 `ctch_newbie`, 0x8f `ctch_raid`, anything else `white`. (`win.rs::group_color`; `TextColors.xml` values: ct_system white, ct_error red, ct_cmd_feedback / ct_otell yellow,
  ct_itell 00ffff, ctch_clan 0ff20b, ctch_emote ff0099, ctch_gm ff61a6, ctch_misc ffffff, ctch_newbie 66ff99, ctch_news 00f000, ctch_pgroup ffffff, ctch_seekingteam 6699ff,
  ctch_shout ffffc9, ctch_team 63e689, ctch_raid ff9966, ctch_tell 00ffff, ctch_vicinity ffff45, ctch_whisper 30d2ff, ctch_tower ff63ff, ctch_mypet ff7718, ctch_otherpet df6718, ctch_research yellow, ctch_admin ff8cfc.)
  OOC is a type-4 group → `ctch_misc`; "Newbie Help" type 0x87 → `ctch_newbie`.
* Timestamp: if the window's `show_timestamps`, `strftime("(%H:%M) ")` of the local time, directly after the colour tag. **GUESS**: the exact slot of the stamp string in the concatenation
  (`FUN_10040762("<font color=", colour)` + stamp + …) — it is built before the prefix and the Ghidra stack slots are ambiguous; it is rendered first.
* sender link `S` = `<a style="text-decoration:none" href="user://NAME">NAME</a>`; GM (flags&1): `<font color="#FF0000"><a …>NAME (GM)</a></font>`.
* group link `G` = `<a style="text-decoration:none" href="chatgroup://#%016x#">GROUPNAME</a>`.
* PREFIX by group (string literals: `DAT_101b8f60` = `[`, `101b8f5c` = `] `, `101b8f54` = `]: `, `101b22ac` = `: `, `101aa270` = ` `):
  * `40000001` System, `42000000..42000019`, `4200001c` Research: none;
  * `40000002` Vicinity: no sender → none; kind 1: `S` + text-db **10001 key `Whispers`** (" whispers: "), kind 2: `S` + key `Shouts` (" shouts: "), kind 3: `S` + " ", else `S` + ": ";
  * `40000003` Tell: `[` `S` `]: `;
  * `41000000`, `41000001` (pets): `S` + ": ";
  * everything else (chat-server groups, `4200001a/1b` loot groups): `[` `G` `] ` and, if there is a sender, `S` + `: `.
* TEXT is inserted unescaped (the handlers pass `RemoteFormat::ParseString` + `HTMLParser_c::ExtractText` output).
* Each window keeps at most **100** lines (`FUN_10088e92` pops the oldest when `size > 100`), then scrolls to the bottom; window logging (`is_logged`) writes `<window>/Log.txt`
  (`"\n"`→`<br>` escaped lines) — **not ported**.
* Link rendering: `TextRenderer_c` ctor 0x10163412 sets link colour `0xff2299ff` (+0x1c0), shadow offset Point(1,1) (+0x254), default colour 0xff000000 = view colour.
  `<a>` runs use the link colour when their attribute word has bit 4 (`_RenderLine` 0x10161112: `attrib & 4 → +0x1c0`); `HTMLParser_c::_ParseTag` 0x1015c9ad sets that word to 6 (underline + link colour) only when the
  `style` attribute is absent or not `text-decoration:none`, so names / group names written as `<a style="text-decoration:none" href=..>` keep the line's font colour (corrected: they were blue before;
  `ao-gui` `layout_text` now follows this, docs/zone/interact.md §2).
  Shadow = a clone of the text surface with colour 0 (black), alpha 1, behind the text (`_AllocateBitmap` 0x1016095b, only with flag 0x1000 = `ChatView::shadow`, `+0x1ba`, default **off**,
  setter `FUN_1008e04f`; offset from pref `ChatTextShadowOffset`: 0 → 0, 1 → 1, 2 → 2, else 1; shipped value 1).
* The `<div indent=wrapped>` hanging indent of wrapped lines (10 px, `_AddLineDesc` 0x10161b44) is ported (docs/zone/interact.md §2). Text selection/copy, right-click menus, window drag/resize and tabs: §10-§13.

### 5.1 Message fading (`is_message_fading_enabled`, `ChatTextFadeDelay` 8.0 s, `ChatTextFadeTime` 0.3 s) — ported, `ao-gui` `gui/textfade.rs`
The chat text area (`FUN_100925ff`, 0x170 bytes, around the `TextView` at `+0x128`) owns a queue of fade lines (`+0x12c`, count `+0x130`, delay `+0x158`, time `+0x160`, both 64-bit µs).
* **Switch**: `FUN_1008fb36(enabled)` (window option): on → `FUN_1008f432` reads `ChatTextFadeDelay` / `ChatTextFadeTime` (`AsDouble * _DAT_101ae2f8` = 1e6, truncated) and observes both DValues, so a pref change applies at once;
  off → `FUN_1009349d(0, 0)`. `FUN_1009349d(delay, time)`: both 0 → timer stopped, queue deleted, text view shown + scrolled to the bottom; otherwise the text view (scrollbar with it) is **hidden**,
  the frame timer starts, and with an empty queue and a non-empty text view `FUN_10092f69` makes one fade line out of the *whole* old text. Port: `ChatWindows::update` (`Win::fade`),
  `Gui::set_text_fade(w, scroll, Some((delay, time)), existing_html)` hides the `ScrollView`.
* **New line** (`FUN_100935ba`, the add-text path): the text view always gets the line; with fading on `FUN_10092f69(html)` also creates a `TextRenderer_c` (flags `0xa60`, view flag `0x80`), laid out at the
  bounds width, bottom-aligned on the bounds bottom; every older line moves up by `height + 1 - shadow_y` (shadow offset Point of `FUN_1008d673` = `ChatTextShadowOffset`, shipped 1 → exactly the line height);
  front lines whose bottom is above the bounds top are deleted. Port: `Gui::add_fade_line`, called by `ChatWindows::fill`.
* **Per frame** (`FUN_10092241`, timer handler registered in `FUN_100925ff`): front lines with `now > born + delay + time` are deleted; every line gets `View::SetAlpha((born + delay - now + time) / time)` while that is
  < 1 → opaque for 8 s, then a linear 0.3 s fade. View flag `0x80` makes `View::_CallRender` reset the inherited alpha to 1.0, so the window transparency (0.3 inactive) does **not** dim the lines: this is what makes the
  option useful with transparent windows. Port: `Gui::tick_text_fades` / `draw_fade_lines`, lines are drawn with their own alpha only.
* Shipped default: off (`is_message_fading_enabled` false); menu *Style_FadeMessages* toggles it. Not ported: the `ChatView` shadow flag (`this[0x16c]` → `RENDER_SHADOW` on the lines; the shipped client never sets it, §5), the
  timer period (we tick every frame; `StartFrameTimer`). Tests: `cargo test --release -p ao-gui --test textfade`, `cargo test --release -p aomac chat::win::tests::message_fading`.

## 6. Input bar, links, activation

* Enter in the editor → `ChatView` signal → the hub gets `WinOut::Submit { text, window_group }`; `deactivate_on_send` (default true) drops the focus afterwards; empty lines are not submitted.
  Commands `/ch /group /g /o /t /v /say /w /whisper /s /shout /me /script` are parsed by the window object (strings in `FUN_1009dc28` 0x1009dc28, e.g. `ChatCmdFeedback_*`, `ChatWarnWhenSpeakingToUnsubGroups`): hub's job.
* Prompt overlay (`InputBar_c` ctor `FUN_100919ab`, updater `FUN_10090f0b`, DValue `ChatShowOGrpInInputBar`, shipped default **true**, LoginPrefs.xml): a second read-only `TextView` (flags 0x800, `View::SetAlpha(0.6)` =
  `_DAT_101b83f0`) over the editor's bounds, visible while the DValue is on **and the editor is empty** (`*(editor + 0x15c) == 0`), moved 4.0 px (`_DAT_101b0840`) right while the editor has the keyboard focus. Its
  text is the output group's name; the colour is the view default (white × 0.6 = the grey "Clan OOC" of the retail screenshot, so *not* the group colour as an earlier guess said). Port: `TextData::hint` +
  `Gui::set_text_hint` (the hint is drawn by the empty editor itself instead of by a sibling view), set by `ChatWindows::sync_decor` every frame from the window's output group. **UNRESOLVED**: which function writes the
  prompt text (we use the same group name as the title); `InputHistory.xml` (`TextLine text=…`, `cursor_pos`) per window: not ported.
* Links: `FUN_1008e322` → `user://NAME` → signal +0x130 (→ `OpenTellWindow` 0x10085df8 = `FUN_100a6568` tell window), `chatgroup://ID` → signal +0x134 (sets that window's output group),
  anything else → `ChatGUIModule_c::ShowItemRefLink` 0x10085cb5 (`itemref://`, `charref://`, `chatcmd:///…`). Port: `ao_gui::Event::LinkClicked` (activation on mouse-down: confirmed, `TextRenderer_c::MouseDown` 0x101637ef, §13) →
  `WinOut::OpenTell` / output group change / `WinOut::LinkClicked`. Tell windows (`OpenTellWindow` 0x10085df8): docs/chat/social.md §4 (`WinOut::OpenTell` opens the tell window; per-user config files of `FUN_100a658b` are not ported).
* Activation: `SlotGroupWindowActivated` 0x10086fb0; `StartChatCmdMessage` 0x10021f18 / `StartChatReplyMessage` 0x10021fd0 (Enter / reply keys) → `focus_input()` focuses the last active window's editor.

## 7. Persistence and screen placement

`ChatGUIModule_c::GetChatConfigPath` 0x10087217 = `<CharPrefsPath>/Chat` (`mkdir`); windows live in `Chat/Windows/<window_name>/Config.xml` (+ `InputHistory.xml`), written on shutdown by `FUN_10094a28`
and read by `FUN_10094c58` (directory scan; names are `sscanf("Window%d")`). Port: reads `<aomac prefs dir>/Chat/Windows/*/Config.xml` (stand-in for the character prefs dir, `AOMAC_PREFS_DIR`), else the client
template above, else the code defaults; `ChatWindows::save()` writes the same schema (round-trip tested).

**Positioning (read from the binary).** `Window::LoadWndConfig` 0x10154d6e: `Message::FindRect("WindowFrame")` → `WndBorder::SetClientFrame(rect)` (absolute screen
coordinates, top-left origin, inclusive `Rect`; style 3 has no border so client = outer), then `Window::MoveInsideScreen(false, true, true)` 0x10154abc: *translate* (never
resize) so the frame lies inside `WindowController_c` +0x80 (the display size `Window::GetScreenSize` 0x10154850): if `left < 0` shift right, else if `right > screen` shift left (same for the
vertical axis). `Window::SaveWndConfig` 0x1548a7 writes `GetFrame` back (`WindowFrame`, `WindowPinButtonState` unless flag 0x800). **No chat code avoids the ControlCenter/HUD**: the original places the windows
exactly where the saved frame says, and the HUD bar windows are *backmost* (the control-centre bar windows are created with flags 0xe3c, whose 0x200 bit is the `SetStyle` backmost flag that `ChatWindow_c`
reads from `is_backmost`), so chat windows draw above them. Port: `place()` (tested: `(277,1206)-(1273,1439)` on a 1280x828 screen → `(277,594)` 997x234; negative/overflowing frames are translated);
`ao_gui::Gui::set_window_layer(w, -1|0|1)` (-1 backmost, 1 frontmost; draw and hit-test order, creation order inside a layer) — **the HUD must mark its ControlCenter/bar windows backmost (-1)** or create the
chat windows after them (normal stacking = creation order).

**GUESS (no original evidence)** for the *shipped template only* (`prefs/NewChar`, authored on a 2304x1440 screen; applying the rule above on a 1280-wide screen would pile Window2 on top of Window1 and put
the input bar under the bottom HUD row): template frames are scaled horizontally by `free width / 2304` (free width = screen minus the HUD's wings, `Reserved{left,right}`), keep their height, and are
bottom-anchored `Reserved.bottom` px above the screen bottom (`ChatWindows::set_reserved`, the hub passes the HUD's footprint; test screenshot used left 190 / right 65 / bottom 38 measured from the HUD art).
Once saved (`ChatWindows::save` writes the *placed* rectangle) the windows load as ordinary absolute frames. Without a frame: centred, 400x200 (`FUN_10097ae3`).
Window drag/resize: see §10 (a mode-2 window is fixed in the original too; the menu switches it to the movable style-0 frame). `ChatWindows::set_visible(false/true)` hides/shows all windows (they keep collecting lines; focus is dropped) so HUD hide/show does not disturb them.

## 8. ao-gui additions (additive)

`Gui::set_window_alpha/window_alpha` (`Window::FadeTo` target), `Gui::set_window_layer` (backmost/frontmost), `Gui::set_text_shadow_offset` + `TVF_RENDER_SHADOW` drawing (black copy behind), `TVF_FILL_BOTTOM_UP` (short content sits at the
bottom of its `ScrollView`), `Gui::scroll_to_bottom`, `Gui::clear_focus`, `Event::LinkClicked { window, view, href }` for read-only `TextView`s (`TextRun::href`). Window frame, tabs, popup menus and text selection (§10-§13):
`set_window_frame` / `set_window_size_limits` / `set_window_tabs` / `set_window_context` / `window_outer_frame` / `set_window_outer_frame` / `interacting`, `open_menu` / `close_menu` / `MenuItem`, `selected_text` / `clear_selection`,
events `WindowFrame`, `TabSelected`, `TabDropped`, `FrameIcon`, `ContextMenu`, `MenuPicked`, `MenuSlider`; modules `gui/{frame,popup,select}.rs`.

## 9. Verification

`cargo test --release -p aomac chat::win` (line formats, colours, subscription rule, shipped template parse/round-trip, routing, fades, submit, 100-line cap; GUI tests skip without the client).
Screenshots: `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac chat_win_shot -- --nocapture` → `chat-inactive.png`, `chat-active.png` (see the observations in the final report of the change).

## 10. Window frame: how a chat window is moved and resized (RE, GUI.dll)

* A chat window in **mode 2 cannot be moved or resized** by the mouse. `FUN_10096ec5` (mode setter) calls `Window::SetStyle(3, (flags & 0x300) | 0xc3c)`: 0x8 = not movable, 0x10 | 0x20 = not resizable,
  0x4 = no border buttons. `WndBorder::HitTest` 0x101593d6 returns hit items: 1 move, 2 left, 3 top, 4 right, 5 bottom, 6 TL, 7 TR, 8 BL, 9 BR (0 when window flag 0x1; the resize items only when `flags & 0x30 == 0`,
  the move item only when `flags & 0x8 == 0`). The original's way to a movable window is the **menu**: right button anywhere in the `ChatView` (`ChatView_c::MouseDown` 0x1008f5dd, button 2 →
  `Window::GetIconMenu` (vtable +0x3c) → `PopupMenu_c::Go`) or the frame's icon button (`WndBorder::SlotIconButton` 0x1015a74e) → "Visual > Mode > Normal" (`FUN_100998bc`, setter `FUN_100993c0` → `FUN_10096ec5`
  → `Window::SetStyle(0, flags & 0x300)`), after which the window is a style-0 frame (3, 7, 3, 3 border + TabView strip, §2).
* **Hit zones** (`WndBorder::Layout` 0x1015a1d9, rects at `this+0x23c..0x2bc`): the four corner rects are `Rect::TranslateTL/TR/BL/BR` of the bounds, the edges `TranslateBorderLeft/Right/Top/Bottom` between the
  corners, the **move zone** (`+0x23c`) is the top-border strip between the corners from the row below the top border (`+0x2c8 + 1.0`) down to row **18.0** (`_DAT_101c894c`), i.e. the tab strip. Corner/edge thickness = the
  border sizes (3, 7, 3, 3): the `Translate*` helpers are imported from Utils.dll, which is not in the Ghidra project, so the corner size is a **GUESS** (border-sized squares). Port: `ao_gui::hit_item`.
* **Drag** (`MouseDown` 0x101595f7 / `MouseMove` 0x10159c27 / `MouseUp` 0x101596a7): MouseDown stores the hit item (`+0x1d4`), the grab offset (pointer − `GetHitPointBase`: the dragged corner's position) and the
  frame; MouseMove sets the dragged edges to `pointer − grab` (item 1: the whole frame is translated), then `DoSetFrame` 0x10159888: the *client* size is clamped to `SetSizeLimits` (default min (0, 0), max (INT_MAX, INT_MAX),
  ctor 0x1015b44b), the edge opposite to the dragged one stays fixed (top moves for items 3/6/7, left for 2/6/8), and the outer width is floored at `+0x2ec − 1` (the width of the border-button row: each button
  adds 5 + 13 px → icon + close = 36 px, so 35 as an extent; **GUESS** that no pin/help button is present). `UpdateMousePointer` 0x101594fc: move = pointer 7, horizontal resize 0xb, vertical 10, diagonals 9 / 8
  (the port uses the OS cursor; shapes **not applied**). Dragging does **not** call `MoveInsideScreen`; `SaveWndConfig` writes `GetFrame` as `WindowFrame`.
* Port: `Gui::set_window_frame(w, movable, resizable)`, `set_window_size_limits`, `window_outer_frame`, `set_window_outer_frame`, `Event::WindowFrame`; `ChatWindows` keeps `Frame::placed` in sync, writes the frame into every
  tab's `Config.xml` (`WindowFrame`, `tab_index`), and saves when the pointer is idle (the original saves at shutdown, `FUN_10094a28`; the port has no shutdown hook). `MIN_CLIENT` (50 × 60) is a **GUESS**:
  `ChatWindow_c` sets no size limits, so the original lets the client shrink to 0 and relies on view minimum sizes.

## 11. Tabs (`Window::InsertTab`)

* A `ChatWindow_c` holds several `GroupChatView_c` tabs, each one window document (`Chat/Windows/WindowN`). `FUN_100974b5` (`Window::InsertTab(index, FUN_100ab980 title, view)`) inserts before the first tab with a
  greater `tab_index` (document `+0x1e8`) and selects it. **Title** = `FUN_100ab980`: `name` (wrapped in `<font color=red>` when document flag `+0x160` = **unread**: `FUN_100989fa(bool)` (a setter that emits its signal) is called with 1 by `FUN_1009b37f` / `FUN_1009b4cf` when a line is added while `View::IsVisible(view +0xd0)` is false, i.e. the tab is not the selected one, and with 0 by `FUN_1009adae` (view created visible) and `FUN_100aac81(selected)` (tab selection); ported as `Win::unread`, `ChatWindows::title`, cleared in `select_tab`) +, when
  the DValue `ChatShowOGrpInTitleBar` (shipped default **true**) is on and the output group exists (`FUN_1009a26c`), ` <font color=green>[<group name>]</font>`; the title is HTML (`green` = 0x008000 of
  TextColors.xml): the retail "Default Window [Clan OOC]" tab. Port: `ChatWindows::title`, re-evaluated every frame (`sync_decor`), tab widths measure the visible text (`Gui::tab_title_width`), runs with an explicit
  colour keep it (`Tab::SetSelected` only sets the *default* text colour). Tab press = `TabView` selection; `FUN_10096b23` takes the alpha of the selected tab's window.
* **Drag**: `ChatWindow_c` connects `TabView` signals: `FUN_10097340` (drag start: `TabView::CreateDragImage`, `DragObject_c(mime "chat_gui/group_chat_view", Message{tab_index})`, `View::BeginDrag`),
  `FUN_10097881` (drop on a `ChatWindow`'s `TabView`, accepts only that mime: the dropped tab goes to the position the pointer is at in the target; every other document's `tab_index` ≥ the new index is incremented in
  the target and (`FUN_10097636`) decremented in the source; the source window closes when it has no tab left) and `FUN_10097d0b` (drop on nothing: with **fewer than 2 tabs** `DragObject_c::Cancel`; otherwise a
  new `ChatWindow_c` (`FUN_10097ae3`) at `GetBounds() + Point` with `MoveInsideScreen(true, true, true)`, the tab moved there with `tab_index` 0).
* Unselected tab art `GFX_GUI_TAB_INACTIVE_LEFT/MIDDLE/RIGHT` (0x1a1..0x1a3), text colour 0 (`Tab::SetSelected` 0x10146110), alpha × `_DAT_101c4978` = 2.0: we cannot multiply 0.85, so the port draws the inactive tab at
  half the layer alpha (**GUESS**); the tab drag image is drawn at half alpha. Drag threshold 4 px (**GUESS**), tear-out offset 20 px (**GUESS**; the `Point` of `FUN_10097d0b` is not readable), tabs side by side without a gap (**GUESS**).
* **Persistence (GUESS)**: the config keys contain only `tab_index` and the per-window `WindowFrame`; the document-to-window grouping (`+0x98`, assigned in `FUN_100abfa3`'s loop and `FUN_100974b5`) is not written by
  `FUN_1009a77b`. The port groups windows in mode 0 whose saved `WindowFrame` is identical into one frame, ordered by `tab_index`, and writes the same frame into every tab of a frame.
* Port: `Gui::set_window_tabs`, `window_tabs`, `Event::TabSelected`, `Event::TabDropped { window, tab, x, y, target }`; `ChatWindows::{select_tab, tab_dropped}` (reorder, dock, tear-out; rebuilds the GUI windows).
  A window with several tabs cannot switch to Borderless (its menu entry is disabled: **GUESS**; the original's `SetStyle` is per window while `visual_mode` is per document).

## 12. Menus (`PopupMenu_c`)

* **Window menu** `FUN_100998bc`: submenu *Style_Mode* (boolean *Mode_Normal* = `visual_mode == 0`, *Mode_Borderless* = `visual_mode == 2`; each calls `FUN_100993c0`); for modes ≠ 0 a submenu
  *Style_Transparancy* with two `PopupMenuSliderItem_c` (0..1; first = inactive `+0x174`, key `…Transparancy_InactiveToolTip`, second = active `+0x170`); separator; booleans *ShowTimestamps* (`+0x16a`),
  *DisableTextInput* (`+0x167 == 0`), *Style_HideInputBarWhenInactive* (`+0x169`, disabled when text input is off), *Style_FadeMessages*. `FUN_10096f9f` appends separator + *Style_AlwaysBehind* (window flag 0x200)
  and *Style_AlwaysOnTop* (0x100). Strings are text-db category 10001 keys (`LDBface::GetText(0x2711, key)`; the keys above are read from the DLL, the texts from `text.mdb`).
* **Icon / right-click menu** `FUN_10097289` (opened by `FUN_100983b0` on the window's icon menu): the window menu as the first entry (header *ChatWindowMenu_Visual*) + separator;
  `GroupChatView_c::FUN_100ab4dc` inserts *TalkToChannel* ▸, *ChannelSubscribeMenu* ▸, boolean *AutoSubscribeChannels* (`+0x163`), separator, then *ChatConfiguration*, *RenameWindow*, *DeleteWindow*, *NewWindow*
  (the last four open dialogs and are **not ported**; the relative order of the two inserters is a **GUESS**). Group lists: sorted by name (**GUESS**; the original iterates its group map).
* **User-link menu** `FUN_1008e135` (a `user://NAME` link under a right press, `FUN_1008f5dd`): *IgnoreUser*, *OpenChat*, *SendTell* (each present when its `ChatView` flag bit 0x1/0x2/0x4 is set; **GUESS** all three);
  the slots re-emit `NAME` on signals +0x148 / +0x14c / +0x150. Port: IgnoreUser → `/ignore NAME`, OpenChat and SendTell → the tell window / `/tell NAME ` (`WinOut::IgnoreUser` / `OpenTell`).
* The text view itself shows no menu (`DISABLE_RC_MENU` 0x200 → `TextRenderer_c::MouseDown` button 2 falls through to the parent `View::MouseDown`), so the right press reaches `ChatView`.
* Port: `ao_gui::MenuItem` (entry / check / separator / submenu / slider), `Gui::open_menu`, `Event::{ContextMenu, FrameIcon, MenuPicked, MenuSlider}`. **UNRESOLVED**: the `PopupMenu_c` skin (the combo popup's
  raised border art is used), the check mark and sub-menu arrow art (a 5 px square and `>`), slider art.
  Not ported (need dialogs / other windows): *ChatConfiguration*, *RenameWindow*, *DeleteWindow*, *NewWindow*, `chat_group_window`.

## 13. Text selection and copy

* `TextRenderer_c::MouseDown` 0x101637ef: left button with flag 0x4 (ACCEPT_MOUSE_INPUT): a hyperlink attribute under the pointer fires the link signal **on mouse-down** (this resolves the earlier GUESS about link activation); else
  `SetCursorPosition` + `BeginSelection` 0x1016108a (clears every other `TextRenderer`'s selection: one selection at a time) + mouse capture. `MouseMove` 0x101639c6: outside the view a 20 ms timer (`SlotScrollTimer`) scrolls;
  inside, `SetCursorPosition` + `ExpandSelection`. `SelectAll` 0x101610cd. The highlight is `Clear(rect, 0xc0c0c0)` (`_RenderString`). There is **no double-click word selection** in `MouseDown` (nothing to port).
* Copy: `CopyActiveSelectionToClipboard` 0x10160f84 → `CopyToClipboard` 0x10160d15: `HTMLParser_c::ExtractText(range)`, `\n` → CRLF, `CF_UNICODETEXT` + `CF_TEXT`, then `ClearSelection`. Port: Ctrl+C / Cmd+C
  (`Modifiers::ctrl` = Ctrl or Cmd) with a selection → `Event::Copy(plain text)` (the app writes the clipboard through `arboard`, `flow.rs`), then the selection is cleared; soft-wrapped lines join without a break,
  real breaks give `\n` (LF on macOS). A press anywhere else clears the selection (**GUESS**: `SlotGlobalMouseDown` 0x1016083d is read as "lose focus" only). The auto-scroll step is one line per 20 ms (**GUESS**).
* Port: `ao_gui` `select.rs` (`Gui::selected_text`, `clear_selection`), `TextLine::hard_break`; `TVF_FILL_BOTTOM_UP` text is now hit-testable where it is drawn (links in short logs were unreachable before).

## 14. Verification of §10-§13

`cargo test --release -p ao-gui --test frame` (hit zones, drag/resize clamps, tab select/drop, popup menu, selection + Ctrl+C) and `cargo test --release -p aomac chat::win` (menu → frame → drag →
save/reload, border windows fixed, dock / tear-out / reload grouping, selection copy, right-click settings). Screenshots: `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac chat_frame_shot -- --nocapture` →
`frame-tabs.png` (docked tabs "Combat" selected, "Default Window" inactive, icon and close buttons), `frame-selection.png` (grey `0xc0c0c0` highlight over lines of the bottom-filled text),
`frame-menu.png` (right-click menu with the Visual sub-menu open).

## 15. Retail comparison and live settings (GuiFidelity pass)

**What the retail screenshot is.** `/tmp/GuiFidelity/ref.png` (an older retail build than 0.7.2, user supplied) shows the chat window in visual mode **0** (style-0 frame, `i` icon, tab strip with two tabs, pin + X).
That is not a contradiction of the shipped data: the template windows say `visual_mode 2` (client/prefs/NewChar/Chat/Windows/Window{1,2}/Config.xml), the screenshot's user chose *Visual > Mode > Normal* and
dragged the "Combat" tab onto the "Default Window" strip (§10, §11). The tab titles `Default Window [Clan OOC]` / `Combat [Atlantean Pact *AP*]` are `FUN_100ab980` with `ChatShowOGrpInTitleBar` = its shipped
default **true** (LoginPrefs.xml line 8). So Normal mode is the thing to be pixel-faithful in, and the borderless default must be what `FUN_100aab08` / `FUN_1008daa0` / `Window::UpdateFadeLevel` say (§2).

Defects found and fixed (all verified against the decompiles above):

1. Borderless (mode 2) windows had no fill. `FUN_1008daa0(false)` gives the text area and the input bar `GFX_GUI_WINDOW_BACKGROUND` panels with margin 0; the port drew the inset outline of mode 0. Now `ChatWindows::doc_xml` builds per mode (0 inset, 1 window3 border α 0.4, 2 black panels).
2. Framed (mode 0) windows were faded like borderless ones (the port set the window alpha 0.3 inactive for every mode): `FUN_10096ec5` mode 0 calls `FadeTo(1.0)`, so a framed window never fades (`alpha_of`).
3. Tab titles were the bare window name: now `name` + ` <font color=green>[group]</font>` (HTML tab titles, `Gui::tab_title_width`, colour runs in `draw_tab`).
4. The empty input bar had no prompt: now the grey group-name prompt (`TextData::hint`).
5. The CHAT font ignored `ChatFontName/Style/Size`: now `Gui::set_chat_font` (face looked up in the host font dir like Verdana; an unknown face keeps the current font), applied on change.

| aspect | retail screenshot (mode 0) | port, `frame-default.png` | notes |
|---|---|---|---|
| frame | style-0, 1 px light outline, `i` left, pin + X right, tab strip above the client | same structure; icons by the FrameButtons slice (docs/gui.md) | art colour = GUIColors DEFAULT cyan (0.7.2) vs the older build's grey |
| tab strip | selected tab dark with white name + dim green `[group]`, unselected tab pale/translucent with its own colours | selected dark/white + `0x008000` group, unselected `GFX_GUI_TAB_INACTIVE_*` at half alpha, black name, green group | unselected art/text tint differs with the palette (older build); alpha ×2.0 of `Tab::SetSelected` is a GUESS (§11) |
| background | dark blue, world visible through (~0.7-0.85) | `TAB_BACKGROUND` / `WINDOW_BACKGROUND` at the layer alphas of docs/gui.md §6, world visible through | visual comparison only: the reference crop has no known backdrop pixel values, so a numeric match was not made |
| inset border | 1 px light inset around text and input | `GFX_GUI_INSET_*`, margin 5 | same ids as `FUN_1008daa0(true)` |
| scrollbar | up/down arrows + thumb | `GFX_GUI_SCROLLBAR_GRAY_*` arrows + thumb | `SetVScrollBarMode(3)` |
| input line | grey prompt "Clan OOC" while empty | grey (white × 0.6) group name | `FUN_10090f0b` |
| fade | none (mode 0) | alpha 1.0 | `FUN_10096ec5` |

**Live settings.** `ChatShowOGrpInTitleBar`, `ChatShowOGrpInInputBar`, `ChatFontName`, `ChatFontStyle`, `ChatFontSize` (tenths of a point; `lfHeight = size / 10`, default 140 = 14 px) are read from the HUD's
`DValues` every frame (`WinPrefs::from_dvalues`, `flow.rs` → `Chat::set_window_prefs` → `ChatWindows::set_prefs`); a change re-titles the tabs / shows or hides the prompt next frame, a font change rebuilds the windows
(input bar height = line height + margins). Defaults equal the client's (tested against the shipped templates). `window_transparency_active/inactive`, `show_timestamps` and `visual_mode` are per-window document settings
(Config.xml, the Window menu) and apply immediately (`alpha_of`, `deliver`). `ChatTextShadowOffset`: the shadow is off in the shipped client (`ChatView::shadow` false, §5). `ChatTextFadeDelay` / `ChatTextFadeTime` are read from the DValues every frame for windows with
`is_message_fading_enabled` (§5.1).

**Verification.** `cargo test --release -p ao-gui --test frame` (tab title width, chat font, hint) and `cargo test --release -p aomac chat::win` (titles, mode XML, prompt, alpha rules, prefs defaults).
Shots with a world-like backdrop (sky gradient over mottled pavement): `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac -- chat_win_shot chat_frame_shot` → `chat-inactive.png` / `chat-active.png`
(shipped borderless: black panels, 0.3 / 0.8), `frame-tabs.png`, `frame-default.png` (Normal mode, tabs, prompt), `frame-selection.png`, `frame-menu.png`.
