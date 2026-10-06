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
  (docs/screens.md §9); the window is a transparent borderless rectangle. **2 is the shipped default** (both default windows).
  The art one sees is the ChatView's own: text area in a `BorderView_c` with `GFX_GUI_INSET_{TL,TR,BL,BR,LEFT,TOP,RIGHT,BOTTOM}` (ids
  0xf9,0xfb,0xf4,0xf6,0xf7,0xfa,0xf8,0xf5, bg 0), client margin 5 (`_DAT_101a8b98`), when `ChatView::border` (`+0x1b9`, default **true**, setter
  `FUN_1008daa0`); with border false: `GFX_GUI_WINDOW_BACKGROUND` (0x1bf) and margin 0. The input bar sits in the same kind of BorderView.
* Window alpha (modes ≠ 0): `FUN_10096b23`: the selected tab's window is *active* (`+0xda`) → `Window::FadeTo(window_transparency_active, 0.2 s)`
  else `FadeTo(window_transparency_inactive, 1 s)` (200000 / 1000000 µs). Defaults 0.8 / 0.3 (`_DAT_101b8a54` = 0.8, `_DAT_101ae0ec` = 0.3);
  the menu's two sliders write them (`FUN_10096db3` / `FUN_10096de4`).
  *Port*: `Gui::set_window_alpha` multiplies everything drawn in the window; active = the window's input bar has the keyboard focus.
* `is_frontmost` / `is_backmost` (chat_window_config) set window flags 0x100 / 0x200 (`FUN_10097ae3`). Not ported (no z-order layer API).
* Mode 0 (framed "Normal") is **not ported**: it is drawn like mode 2 (GAP; needs the style-0 TabView frame in ao-gui).

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
(`win.rs::LOCAL_GROUPS`). `MutedChatGroups` (CharPrefs archive) and `ChatFilterEnabled`/`ChatFilterRules` (MainPrefs, regex over the text, default off) filter in `FUN_10084f9e`: **not ported**.
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
  `<a>` runs always use the link colour (`_RenderLine` 0x10161112: `attrib & 4 → +0x1c0`), so names/group names are blue; `text-decoration:none` only suppresses the underline.
  Shadow = a clone of the text surface with colour 0 (black), alpha 1, behind the text (`_AllocateBitmap` 0x1016095b, only with flag 0x1000 = `ChatView::shadow`, `+0x1ba`, default **off**,
  setter `FUN_1008e04f`; offset from pref `ChatTextShadowOffset`: 0 → 0, 1 → 1, 2 → 2, else 1; shipped value 1).
* **Not ported**: `<div indent=wrapped>` hanging indent of wrapped lines; per-line fading (`is_message_fading_enabled`, prefs `ChatTextFadeDelay`/`ChatTextFadeTime`, `FUN_1008f432`
  → `FUN_1009349d`; shipped false, prefs absent from CharPrefs/MainPrefs); text selection/copy from the read-only chat text (ao-gui selects editable fields only);
  right-click menus (`ChatWindowMenu_*`, `FUN_100998bc`, `FUN_1008e135`); window drag/resize (ao-gui has no movable windows yet); tabs (`Window::InsertTab`; each port window has one tab).

## 6. Input bar, links, activation

* Enter in the editor → `ChatView` signal → the hub gets `WinOut::Submit { text, window_group }`; `deactivate_on_send` (default true) drops the focus afterwards; empty lines are not submitted.
  Commands `/ch /group /g /o /t /v /say /w /whisper /s /shout /me /script` are parsed by the window object (strings in `FUN_1009dc28` 0x1009dc28, e.g. `ChatCmdFeedback_*`, `ChatWarnWhenSpeakingToUnsubGroups`): hub's job.
* Prompt overlay (`InputBar_c`, `ChatShowOGrpInInputBar`): output-group name in the output group's colour at alpha 0.6, shifted 4.0 px (`_DAT_101b0840`) right while the editor has focus. **Not ported** (pref default
  unknown; `win.rs` exposes `active_output_group()` instead). `InputHistory.xml` (`TextLine text=…`, `cursor_pos`) per window: not ported.
* Links: `FUN_1008e322` → `user://NAME` → signal +0x130 (→ `OpenTellWindow` 0x10085df8 = `FUN_100a6568` tell window), `chatgroup://ID` → signal +0x134 (sets that window's output group),
  anything else → `ChatGUIModule_c::ShowItemRefLink` 0x10085cb5 (`itemref://`, `charref://`, `chatcmd:///…`). Port: `ao_gui::Event::LinkClicked` (activation on mouse-down is a **GUESS**) →
  `WinOut::OpenTell` / output group change / `WinOut::LinkClicked`. Tell windows themselves (`FUN_100a658b`, per-user config files) are not ported: the hub opens/routes them.
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
Window drag/resize is **not ported**: the original's borderless style-3 chat window has no frame handles (`HitTest` 0x101593d6 is the frame's; moving a visual_mode 2 window needs the frame/menu mode),
ao-gui has no movable windows. `ChatWindows::set_visible(false/true)` hides/shows all windows (they keep collecting lines; focus is dropped) so HUD hide/show does not disturb them.

## 8. ao-gui additions (additive)

`Gui::set_window_alpha/window_alpha` (`Window::FadeTo` target), `Gui::set_window_layer` (backmost/frontmost), `Gui::set_text_shadow_offset` + `TVF_RENDER_SHADOW` drawing (black copy behind), `TVF_FILL_BOTTOM_UP` (short content sits at the
bottom of its `ScrollView`), `Gui::scroll_to_bottom`, `Gui::clear_focus`, `Event::LinkClicked { window, view, href }` for read-only `TextView`s (`TextRun::href`).

## 9. Verification

`cargo test --release -p aomac chat::win` (line formats, colours, subscription rule, shipped template parse/round-trip, routing, fades, submit, 100-line cap; GUI tests skip without the client).
Screenshots: `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac chat_win_shot -- --nocapture` → `chat-inactive.png`, `chat-active.png` (see the observations in the final report of the change).
