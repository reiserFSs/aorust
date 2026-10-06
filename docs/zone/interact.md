# In-game interaction: NPC dialogue (KnuBot) and using objects

Code: `crates/ao-net/src/n3/knubot.rs` (codecs), `crates/aomac/src/play/interact.rs` (state), `interact_chat.rs` (the NPC chat window),
`interact_play.rs` (hooks of `flow.rs`). `[GC]` = Gamecode.dll, `[GUI]` = GUI.dll, `[IF]` = Interfaces.dll (addresses of the 32-bit DLLs, PE base 0x10000000);
Ghidra projects `/tmp/aomac-ghidra/playfield` (Gamecode, N3) and `/tmp/aomac-ghidra/dsky/gui` (GUI). `[CODE]` = read from the decompile, `[LIVE]` = observed
on the PRK server, `[INFERENCE]` / `[UNRESOLVED]` = not proven.

## 1. The KnuBot messages (`Knubot*IIR_c`)

All ten are `n3InfoItemRemote_t` subclasses of `KnubotBaseIIR_c` (ctor `FUN_1012782a` [GC]). Common wire layout [CODE]:

| part | bytes | meaning |
|---|---|---|
| `u32 key` | 4 | `MapToKey(class name)` (`message_key`) |
| header `Identity` | 8 | the character the message is for / from (own character `{0xC350, id}` for everything the client sends) |
| flag | 1 | **0** (`ClearToBePassedOn`, ctor `FUN_1012782a`; the other client messages in `outgoing.rs` use 1) |
| `i16 2` | 2 | `FUN_101278d6` (write) / `FUN_10127893` (read): only version 2 carries the identity |
| NPC `Identity` | 8 | `+0x18`: the dynel of the dialogue |
| body | | per class below; strings are `i32 len` + raw bytes, no terminator |

| class | key | direction | body | `Activate` (client side effect) |
|---|---|---|---|---|
| `KnubotOpenChatWindowIIR_c` | `3B132D64` | both | `i32 b21, i32 b20` (each `== 1`) | `FUN_10127ea0`: GlobalSignals `+0xec` (npc, b20, b21) -> `NPCChatModule` builds the window |
| `KnubotCloseChatWindowIIR_c` | `270A4C62` | both | `i32 value, string` (len < 0x3e9) | `FUN_10127a59`: `+0xf0` (string, int64 value) |
| `KnubotAppendTextIIR_c` | `5D70532A` | S->C | `i32 type, string` (len 0..=10000) | `FUN_101275d3`: `+0xf4` (string, type) -> `NPCChatView_c::AddText` `FUN_100586fd` |
| `KnubotAnswerListIIR_c` | `55704D31` | S->C | `i32 n` (1..=1000), n x string (1..=1000) | `FUN_10127228`: `+0xf8` (vector<string>) -> `FUN_10058dc4` |
| `KnubotAnswerIIR_c` | `2103247D` | C->S | `i32 index` | none |
| `KnubotNPCDescriptionIIR_c` | `000A0C5A` | C->S | none | none |
| `KnubotStartTradeIIR_c` | `7864401D` | both | `i32 value, string` (len 1..=10000 on read) | `FUN_10128554`: `+0xfc` (string, value) -> `FUN_10058a4c` (trade bar) |
| `KnubotFinishTradeIIR_c` | `55682B24` | C->S | `i32 flag (==1), i32 value` | none |
| `KnubotTradeIIR_c` | `3A1B2C0C` | C->S | `i32 op (0 add, 1 remove), Identity, Identity` | none |
| `KnubotRejectedItemsIIR_c` | `2D212407` | S->C | `i32 n, n x (Identity, i32, i32), i32 value` | `FUN_101281c6`: re-lays the inventory, `+0x100` |

Client senders [GC]: `N3Msg_NPCChatRequestDescription` 0x10017e56, `N3Msg_NPCChatAddTradeItem` 0x10017ea0 / `RemoveTradeItem` 0x10017f31 (identities: first zero, second the
item), `N3Msg_NPCChatEndTrade` 0x10017fb1 (flag = `!param_4`), `N3Msg_SendNPCChatAnswer` 0x1001902a, `N3Msg_NPCChatCloseWindow` 0x1001ce62 (value 0, empty string),
`N3Msg_NPCChatStartTrade` 0x1001cee9. Tests: `n3::knubot::tests` (round trips, truncation, key hashes, the answer wire bytes).

## 2. The NPC chat window (`NPCChatWindow_c` / `NPCChatView_c`, GUI.dll)

* Created by a `NPCChatModule` slot (code at GUI 0x1002d36x; it deletes the previous NPC chat window first -- the destructor sends `NPCChatCloseWindow`) with
  `FUN_10059e8a` [GUI]: `Window(Rect(0, 0, 399.0, 299.0), "", "", style 0, flags 0x1000)` (`_DAT_101b1d18/1c`), one tab titled with `N3Msg_GetName(npc)`,
  `MoveToCenter`, `LoadWndConfig` of the DValue `NPCChatWindowConfig` (empty on a fresh install), `esc_npcchat` (LoginPrefs default false) decides whether Esc closes it.
  Our client area is 400 x 300 and the window is centred.
* `NPCChatView_c` (`FUN_10058ed8`): vertical layout of two `TextView_c(Rect, "", "", FontID 8 = LARGE, 5, 0, false)` [CODE: the ctor `??0TextView_c@@QAE@ABVRect@@PBDABV..@W4FontID_e@@II_N@Z`
  is `(rect, name, text, font, uint, uint, bool)`] (HTML parser features `0x6c`, vertical scrollbar mode 2 = auto): the text at `+0x128`, the answer
  list at `+0x12c`, with a draggable splitter bitmap between them (`BitmapView_c(Rect, "", 0x120 = GFX_GUI_NPCCHAT_SEPERATOR (32 x 3), 0, 4)` at `+0x130`, `ExtendMaxSize((16000, -1))`). The
  text view has borders (5,5,5,0), the answer view (5,5,5,5). The text view's preferred height is `floor(split * (Height + 1) - 1)` with `Height + 1` = the view's pixel height and
  `text_split_proportion` = 0.7 (`FUN_10058100`, `_DAT_101af644`, `Message::FindFloat` of the window config), applied as min `Point(0, h)` / max `Point(16000, h)` by `FUN_1005824d`
  (also on every frame change, vtable slot 27 `FUN_10058458`).
* **Splitter drag** [CODE]: vtable slot 15 `FUN_10058186` (mouse down, button 1) -- when the point is inside the splitter's frame (`+0x130`): `this+0x180 = Point(0, FUN_10058100()) - point`
  (the grab offset), `this+0x17c = 1`, the mouse is captured (vtable `+0xac`); slot 17 `FUN_10058460` (mouse move): without a drag, over the splitter frame
  `View::SetMousePointer(this, 10)` (the pointer row 10 = `GFX_GUI_POINTER_VER_DRAG` 0x13f, hotspot (6,16), docs/gui.md §13.2), otherwise the default pointer; while dragging
  `FUN_10058371(y = offset.y + point.y)`: `split = y / (Height + 1.0)`; `split < 0.1` (`_DAT_101b2058` = the double 0x3fb99999a0000000 = 0.1f) -> `split = 0.1f` (`_DAT_101b2054`);
  else `split > 0.9` (`_DAT_101af630`, double 0.9f) -> `0.9f` (`_DAT_101b2050`); then `FUN_1005824d` re-lays the view out; slot 16 `FUN_10058230` (mouse up) ends the drag. Port:
  `interact_chat::NpcChat::{drag_start, drag_by, drag_end, pointer_over}` + `split_from`, the splitter being a `CanvasView` (`CanvasPress/Drag/Release`) painted with the separator art
  ([INFERENCE]: stretched over the width; `BitmapView_c` drawing not read); `Play::interact_pointer` draws the pointer sprite over the splitter and hides the OS pointer.
  Note the `- 1` of `FUN_10058100`: the dragged text height is `floor(y - 1)` (a press without movement keeps 209, the first pixel of movement lowers it by 1 more).
* Text composition (`FUN_100586fd`, the `AddText` slot; ported as `interact_chat::Composer`, tests in that file). `type` = `NPCChatTextType_e`:

| type | HTML prefix | note |
|---|---|---|
| 0 | `<font color=CCNPCChatText>` | when the previous text was not type 0: `<br>` (only if the view has text) + NPC name + `": "` |
| 1 | `<font color=CCNPCOOCText>` | `<br>` unless the previous text was type 1 or the view is empty |
| 2 | `<br><font color=CCNPCChatQuestion>` | the echoed answer: only with the pref `ShowNPCQuestions` (IndependentPrefs login int, default **1** in `SetDefaultLoginPrefs`, `dvalue/indep.rs`; `Hud::dvalues.prefs.int_any`) as `<own name>: <text>`; otherwise nothing but the empty font tags |
| 3 | `<br><font color=CCNPCChatSystem>` | |
| 4 | `<br><font color=CCNPCChatEmote>` | NPC name + `" "` + text |
| 5 | `<br><font color=CCNPCChatDescription>` | |
| 6 | `<br>` (if not empty) + `<font color=CCNPCChatText>` + NPC name + `": "` | counts as type 0 afterwards |
| other | none | |

  Every text ends with `</font>`; a literal backslash + `n` in the text becomes `<br>`. The view then scrolls to the bottom. Colours: `TextColors.xml` (`CCNPCChat*`).
* Answers (`FUN_10058dc4`): the answer view is cleared and each answer becomes
  `<div indent=wrapped><img src=tdb://id:GFX_GUI_NPCCHAT_BULLET> <a href=%u style=text-decoration:none><font color=CCNPCChatQuestion>%s</font></a></div>` (`%u` = index,
  `%s` = `String::Escape(answer)`). The link slot `FUN_10058d1a`: `atol(href)`, in range -> adds the answer as a type-2 text, `N3Msg_SendNPCChatAnswer(own, npc, index)`,
  clears the answer view and the vector.
  The bullets and the hanging indent are the HTML parser's: `ao-gui` `text::layout_text` now implements them [CODE: `HTMLParser_c::_ParseTag` 0x1015c9ad, `FUN_1015ed05`, `TextRenderer_c::_ReWrap`
  0x10161ba4 / `_AddLineDesc` 0x10161b44 / `_RenderLine` 0x10161112]: `<img src=tdb://id:NAME>` (or `rdb://`; token 3, the sprite of `GuiResourceManager_t::GetGuiTexture`) is an inline sprite copied
  top-aligned at the pen (`SpriteInfo_t::Copy`), its width advances the pen and counts as a word; `<div>` / `<center>` are tokens 0xc / 0xd, `indent=wrapped` sets bit 1 of the div's attribute
  word: `_ReWrap` runs a line counter (-1 outside, 0 in the div, +1 per `_AddLineDesc`) and gives every line with counter > 0 the indent `LineDesc+4 = 10` px (`TextLine::indent`, included in
  its width). A div is a block (a line of its own); the chat window's `<br>` joins right behind a `</div>` add nothing, so the chat keeps its look. `<a href style=text-decoration:none>`:
  `_ParseTag` sets the attribute bits 6 (underline + link colour `+0x1c0` = 0x2299ff) **only when the style is absent or something else** -- with `text-decoration:none` the run keeps the
  font colour, which is why the answers are `CCNPCChatQuestion` green (0x4fd553) and not link blue. Tests: `ao-gui/tests/html_text.rs`.
* The button bar (`ButtonBar_c`, `FUN_10059704`; ctor `BorderView_c(Rect, "", 0, 4)` + `HLayoutNode`): four `Button_c` (`Button_c(Rect, "", "", -1, 0, 0, 0, 0)`, `Button_c::SetGfx(state, id)`
  states 0 raised / 1 pressed / 2 hover = highlight art, `StateChanged` 0x10128338): description `GFX_GUI_BUTTON_DESC_NORMAL/PRESSED` (0x3f/0x40), info `INFO_*` (0x44/0x45), trade `GIVE_*` (0x42/0x43),
  use `SHOP_*` (0x49/0x4a) (37 x 31 px each, `SetBorders` (5,5,0,5) for the first three, (5,5,5,5) for the last), tooltips `LDBface::GetText(10000, key)` = `RequestNPCdescription`, `RequestNPCinfo`, `GiveItems`, `Shop`
  (keys read off the `PUSH` operands at 0x10059a90.. -- the decompile drops them). Handlers (connected in `FUN_10058ed8` in this order):
  1. `FUN_100584f1` -> `N3Msg_NPCChatRequestDescription(own, npc)` (the server answers with `AppendText` type 5);
  2. `FUN_100582eb` -> `InfoViewModule_c::ShowURL("charid://%u/%u", npc.kind, npc.instance)` (the info page of the NPC; ours: `Interact::take_info_urls` -> `ChatWindows::show_url`);
  3. `FUN_1005852c` -> `N3Msg_NPCChatStartTrade` when no trade bar exists (`this+0x140 == 0`), else `FUN_10058408` (cancel the trade, §11);
  4. `LAB_1005834a` -> `N3Msg_UseItem(npc, false)` (`GetInstance` 0x101a772c + `N3Msg_UseItem` 0x101a75f0; for a character target [GC 0x100286f8] builds the `ItemActionData{own, target}`
     `GenericCmd_t` command 3 of §8, `Interact::use_object`).
  Enable flags (`FUN_10058ed8`): description = `b20` of `KnubotOpenChatWindow`, trade = `b21`, use = bit 21 of the NPC's stat 0 (`N3Msg_GetSkill(npc, 0, 2) >> 21 & 1`, read when the window
  is built; ours tracks stat 0 of every dynel from the `StatIIR_t` frames, `Interact::trade.flags0` -- [INFERENCE] that the server sends stat 0 for NPCs), info always. The bar is its own window
  `Window(Rect(50, 50, 100, 100), "", "", style 2, flags 0x183c)` with one tab titled `GetText(10000, "Tab_Tools")` (never shown: style 2 = the black 0.85 alpha background, no frame), shown and docked
  to the chat window with `Window::DockWindow(slot 5)`: `Window::LayoutDockedWindows` [GUI 0x101550ed] puts slot 5 left aligned below the window (`x = L, y = B + 1`, inclusive rect); slot 2
  (the trade window) at `x = R, y = T` stacked downwards. Port: `interact_chat::{ChatTexts, BarFlags, dock_below, dock_right}`, tests in `interact_chat` / `interact_trade`.
* Closing the window with its close button runs the destructor `FUN_10059d7f`: `N3Msg_NPCChatCloseWindow(own, npc)` unless `this[0x98]` is set (the server closed it).

## 3. `N3Msg_DefaultActionOnDynel` (right click, left double click)

[GC 0x100291da], called from `ActionViewMouseHandler_c`'s release (docs/zone/combat-net.md §5.2) with the identity under the pointer. For a **character** [CODE]:

1. `HasStat(0x8000000)` of the target -> nothing; the own character's stat `0x296` ( = in a vehicle) non-zero -> chat feedback `Feedback_NotInVehicle`.
2. Stat `0x300` of the target (no name in the client's table; the server sends it as `StatIIR_t` pair `(0x300, 0|1)` per NPC, docs/zone/dynel.md §3) with **bit 0** -> `KnubotOpenChatWindowIIR_c`
   (header = own character, NPC identity, `0, 0`; `FUN_10127f4d`) -- this is the dialogue.
3. Otherwise, outside a fight (`controller+0x44 == 1`): `N3Msg_TradeStart(target)` (player trade; **not ported**), or `N3Msg_UseItem` / `TradeAbort`.

For a non-character `Identity` (item / world object): stat `Can` (0x1e) bit 0 -> `N3Msg_GetItem`, bit 3 (8) -> `N3Msg_UseItem(identity, false)`.

`N3Msg_UseItem` [GC 0x100286f8] for a world object: not an inventory item, not a character -> `GenericCmd_t` (`52526858`) `state 0, seq, cmd 3` with
`ItemActionData{flag 0, actor = own character, item = the object}` (`FUN_1003aa0f`, `FUN_1007c95c(actor, data, 3)`), queued with the engine's `vtable+0x2c`
(docs/zone/misc.md §8 has the layout; the server's confirmations are the `state 1` copies in the capture).

## 7. Grid / whompah / shuttle (`GridDestinationSelectIIR_t`, `GridSelectedIIR_t`, `TeleportTarget_c`)

Code: `crates/ao-net/src/n3/grid.rs` (codecs), `crates/aomac/src/play/interact_grid.rs` (window, `GridUi`), hooks in `Interact::on_frame` / `event` / `close_all`.

**What triggers it** [INFERENCE]: nothing client-side. The client never asks for a destination list; the server sends `GridDestinationSelect` (probably as the answer to a
`UseItem` on a grid terminal / whompah / shuttle object, or a dialogue action; the client has no code that requests it). The reply is `GridSelected`. [UNRESOLVED] which live
objects produce it: not yet seen on the wire.

### 7.1 Wire format
Both derive from `n3InfoItemRemote_t`: header = key (`MapToKey(class name)`), the client character's identity, flag byte **0** (`ClearToBePassedOn`, `FUN_10128fd9` / `FUN_10129398`
[GC]); no version word.

| message | key | dir | evidence |
|---|---|---|---|
| `GridDestinationSelectIIR_t` | `0639474D` | S->C | vtable 0x10170684: read slot 7 `FUN_101290a0`, write slot 8 `FUN_101290cc`, Activate slot 2 `FUN_10128fd9` [GC] |
| `GridSelectedIIR_t` | `3A322A4A` | C->S | vtable 0x101706ac: read `FUN_101293a4`, write `FUN_101293ec`, ctor `FUN_1012942a` [GC] |

* `GridDestinationSelect` body: `i32 (n + 1) * 0x3f1`, `n` entries, the token. `FUN_10128f1e`: the word must be a multiple of 0x3f1 with quotient 1..=0x7531 (else the stream is
  flagged bad), so `n <= 0x7530`.
* Entry (0x30 bytes in memory, `FUN_10128b07` read / `FUN_10128aac` write): `i32 playfield` (+0), `Identity` (+4, two `i32`: kind, instance), string (+0xc) = `i16` length + bytes
  (`FUN_100388e2` / `FUN_1003889d`; length >= 0x8000 is a bad stream), `i32` (+0x2c), `i32` (+0x28). The last two are never read by the window [UNRESOLVED meaning, ported as `v2c` / `v28`].
* Token (`Token_c`, vtable 0x10171690, write `FUN_1013d0f3`, read `FUN_1013d12d`): byte `'a'` (anything else throws), `i32` length, the bytes. Opaque; the client stores it and sends it back.
* `GridSelected` body: token, `i32` (+0x24), `i32` (+0x28), `Identity` (+0x2c). `N3Msg_GridDestinationSelected(int index, uint playfield, const Identity&)` [GC 0x1001817b] builds it
  with header target = the control dynel's identity (`dynel + 0x14`) and the token the engine stored at `n3EngineClient + 0xb4`. Call site: `FUN_100ffa12` passes
  `(row's Variant int = position in the list, entry +0, entry identity)`.
* Tests: `ao-net` `n3::grid` (key hashes, round trips, truncation at every byte, exact `GridSelected` and list bytes, bad count word / tag / length).

### 7.2 Activate and the module
`GridDestinationSelect::Activate` (`FUN_10128fd9`): `GetDynel(header target)` must be a `SimpleChar_t`; then `N3Msg_SetGridDestinationList(list, token)` [GC 0x100239da]
(replaces the engine's list and token), `DistributedValue teleport_target_window = true`, `ClearToBePassedOn`. The DValue belongs to a `GUIModuleBase_c` (at `+0xc58` of the GUI module
object, built in `FUN_1002e537`); its show slot **`FUN_1002e313` [GUI]**:
* show: `N3Msg_GetSkill(0xe0 Features, 2) & 0x800`; **clear -> the DValue is reset to false and nothing opens**; set -> if no window exists yet (`FUN_1002f1a6`) the window is created
  (`FUN_100ffa7c`). An already open window is left alone (its rows stay the old list; the token is the new one).
* hide (DValue false): the window is deleted (vtable slot 0 with 1).
* [LIVE] **gate**: a stock character's own `Features` is `0x6` (capture `zone_newchar_ithaca.rec`, test `new_character_features_lack_the_gate_bit`), so unless the server sets bit 0x800 the
  original client never shows this window. Ported faithfully (`eprintln` "grid window refused"); [UNRESOLVED] whether the PRK server grants the bit (movement.md: `Features` has grant counters).

### 7.3 The window (`TeleportTarget_c`, `FUN_100ffa7c` [GUI])
* `Window(Rect(200, 180, 700, 600), "", "Vehicle", 0, 0x1000)` (`_DAT_101a959c/95a0/95a8`, `_DAT_101b3db8`; the name is a copy-paste leftover) -> client 501 x 421, one tab titled
  **"Target List"** (`AppendTab`), `MoveToCenter` (ported: centred), `LoadWndConfig(DValue TeleportTargetConfig)` (empty on a fresh install; **not persisted** here), `Show(true)`.
  Help file "The teleport target window.html".
* View from `Views/TeleportTarget.xml`: vertical `View` of `TargetView` (min 1x1, max 10000) and a row with `Go` ("Go!") left, spacer, `Quit` ("#Close") right, borders 5.
* A `MultiListView_c(Rect(), 0x40 = row selection, 0, 0)` in list layout (`SetLayoutMode(1)`), scrollbars `ScrollView_c` modes 2/2 (auto), placed in `TargetView`. Columns
  `AddColumn(0, "Location", w, 0xe)` and `(1, "Playfield", w, 0xe)` (flags: resizable | label | sortable), `w` = `FindFloat("LocationColWidth" / "PlayfieldColWidth", 200.0)`.
  The destructor `FUN_100ff82c` saves both widths + the frame into the DValue `TeleportTargetConfig` (not ported).
* Rows (loop of `FUN_100ffa7c`, one `TeleportTargetItem_c` per entry, `FUN_10100123`, `AddItem(IPoint(0, i), item, sorted = true)`): item key `Variant(i)` (list position),
  `+0x88` = entry playfield, `+0x80/84` = entry identity, `+0x48` = entry name, `+0x64` = `N3Msg_GetPFName(playfield)` or **"unknown pf"**. Cell 0 shows `+0x48` (the name), cell 1 `+0x64`
  (item view `FUN_10100203`, cell width `FUN_1010030b`). **Compare** (`FUN_10100340`, virtual +4): column 0 compares `+0x64`, column 1 `+0x48` (the opposite of what is shown, [INFERENCE]
  an original quirk), so the list starts sorted by playfield name (stable on ties, list order); ported with rank keys. Names: `pfnrmap.dat` (as `LFT`).
  Rows are not enabled/disabled, there are no cost / level columns and no double-click action.
* Slots: the list's mouse signal (`+0x134`, `FUN_100ff922`) selects the row under the pointer (`MultiListViewItem_c::Select(true, true)`); `Go` (button signal `+0x148`) -> `FUN_100ffa12`;
  `Quit` -> a `Window` method thunk [GUI 0x1012a778] (the close path, [INFERENCE]).
* `Go` (`FUN_100ffa12`): no selection = nothing. Else `N3Msg_GridDestinationSelected(row, playfield, identity)`, then the **close virtual** (vtable slot 7 = `FUN_100ff939`).
* Close virtual (`FUN_100ff939`): if the DValue `teleport_target_window` is still true: `N3Msg_GridDestinationSelected(-1, 0, Identity(0, 0))` (cancel) and the DValue := false (the module then
  deletes the window). So **`Go` sends the selection and then a cancel** (nothing in `FUN_100ffa12` clears the DValue first); `Quit` and the frame's close button [INFERENCE] send only the cancel.
  Ported exactly (test `window_lists_the_destinations_and_go_sends_the_selected_one`: two frames).
* Zone change / disconnect: `Interact::close_all` removes the window without a message [INFERENCE: the original's behaviour was not found].
* `FUN_10031462` [GUI] (not ported): builds a string from a *per-dynel* `N3Msg_GetGridDestinationList(Identity)` (names joined by `DAT_101af164`, wrapped in the text LDB 0x1fa "MastersOf");
  [UNRESOLVED] where it is shown.

### 7.4 Live-harness API (`Interact`, `cfg(test)`)
`grid_dump(&gui) -> String` (rows as shown, display order: `[index] location | playfield (pf id, identity)`) and `grid_select(&mut gui, index) -> bool` (selects list entry `index` and presses
Go; the `GridSelected` and the cancel go to `take_outbox`). Open window state is also in `GridUi::dump`.

## 8. Using objects (doors, terminals, vending machines, items on the ground, corpses)

Code: `crates/aomac/src/play/interact_use.rs` (rules, `UseUi`, picking), `interact_play.rs` (clicks, per-frame pump), `dynels.rs` (`Dynels::stat_of`, `pick_props`,
`Built::stats`, `Prop::stats`). Tests: `interact_use::tests` (decision table, message bytes, refusals), `dynels::tests::captured_dynels_become_actors` (real client: the captured
corpse / vending machine have Can 8, pick boxes, a ray at a corpse hits it).

### 8.1 Which click does what (`FUN_1002c469` / `FUN_1002c2ee` [GUI], decompiled)
* **Right button, release** (`FUN_1002c469`, `InputConfig_t+0xb0` = the object under the pointer): `Identity.kind == 50000` (a character) -> `N3Msg_DefaultActionOnDynel`; **anything else
  -> `N3Msg_UseItem(id, false)` directly** (not `DefaultActionOnDynel`; the earlier note in docs/gui.md §13.2 had it right). [CODE]
* **Left button, second click** (`FUN_1002c2ee`, press with click count 2, the pref `DoubleclickAction`, LoginPrefs default true): `N3Msg_DefaultActionOnDynel(id)` for every object except
  the own character (`InputConfig_t+0xd8` = own identity), including non-characters. [CODE] Ours: the second release on the same object within `DOUBLE_CLICK_TIME`
  (`Interact::double_click`, keyed by identity).
* The keybind "Use" (hotbar special action 3, `FUN_1004256c`) is `N3Msg_UseItem(target, false)` on the current target (`hud_use.rs::use_target`); targets are characters here, so only the
  character branch of `UseItem` is reachable that way. Objects are not selectable (`Zone::target` holds character ids), see 8.5.

### 8.2 `N3Msg_DefaultActionOnDynel` for a non-character [GC 0x100291da, CODE]
`FUN_10058e36(id)` is "kind == 50000 and the dynel exists"; otherwise `FUN_1008720e(id)` = `GetDynel` cast to `SimpleItem_t` (all of corpses, vending machines, doors, terminals, ground items;
a dynel we do not know = nothing happens), then `Can` = `GetStat(0x1e, 2)`:

| `Can` | action |
|---|---|
| bit 0 (1) | `N3Msg_GetItem(id)` |
| else bit 3 (8) | `N3Msg_UseItem(id, false)` |
| else | nothing |

`interact_use::decide(can)` is that table (`CAN_PICK_UP` / `CAN_USE`); `Interact::default_action_on(zone, identity)` is the whole function (character -> `default_action`).
`Can` per object: `Dynels::stat_of(kind, instance, 0x1e)` = the message stats (full update, `StatIIR_t` for a non-character) over the template's (`Built::stats` = `effective_stats` of the
rdb 1000020 record, built on the worker); `Corpse_t`'s constructor [GC 0x1007e652] presets **Can = 8** (`FUN_10088d80(0x1e, 8)`) and stat `0x1b3` = 1. The captured vending machine's template has Can 8.

### 8.3 `N3Msg_GetItem` [GC 0x10027beb, CODE]
`FUN_1002a1b0(0x40) == -1` (no free bag slot `0x40..0x5e`) -> chat feedback `Feedback_InventoryFull`; else, if the dynel is a `SimpleItem_t` that is not already in a bag (`+0x130`) and its pick-up
check returns 2 (`vtable +0xc4` = `FUN_10088529`, see below), the local pick-up animation `FUN_10081e74(char, 1)` plays; `FUN_10015258(own + 0x14, id)` builds **`ClientGetItemIIR_t`**
(`n3InfoItemRemote_t` ctor with the class name; to-be-passed-on byte 0, vftable 0x10157320, `Write` = `FUN_1013cd89` = two `i32` of the identity) and `n3Dynel_t::SendIIRToObservers` sends it.
Wire: `u32 MapToKey("ClientGetItemIIR_t")`, header `{0xC350, own}`, byte 0, `Identity` kind + instance (`ao_net::n3::inventory::get_item`, test `get_item_is_header_plus_one_identity`;
`interact_use::tests::get_item_message_and_full_bag` runs it through `Interact`). `FUN_10088529` (the item's check, returns 6 already held / 2 pick up / 7 `Feedback_CantCarryThat` (Can bit 0 clear) /
10 `Feedback_AlreadyGotUniqueItem` (unique item, stat 0x17 / flag 0x8000000) / 1 `Feedback_CantTakeFixtureFromBuilding` (own-building rule `PlayfieldAnarchy_t::IsOwnedBuilding`)) is **not
ported** and neither is the pick-up animation; the server enforces the rules. [UNRESOLVED: whether the server answers a refused pick-up with a feedback text.]

### 8.4 `N3Msg_UseItem(id, bool)` for what is not an inventory item [GC 0x100286f8, CODE]
Identity kinds: `0x69` -> `Feedback_ItemCantBeUsedFromBank`, `0x6a` -> `Feedback_ItemsCantBeUsedFromCorpse`, `0x6e` -> `Feedback_MoveItemToInventory` (the own instance: container open, not ported),
`50000` (a character): the own instance does nothing, else `GenericCmd_t`; any other kind > 50000 (a world object): if `!param_2` and `Can & 0x10` -> **`GlobalSignals +0xa0` =
`GuiSystem_c::ConfirmUseItemDialogue(id)`** [GUI 0x1003012f]: a `DialogBox_c` named "UseItem", text `LDB(0x2715, stat 0x2af of the object)` or, when the object has no stat 0x2af,
`LDB(0x3e8, "Item_ConfirmUse")`, buttons `MsgBox_Yes` / `MsgBox_No` (category 10000, as the team invite); the result slot `ConfirmUseItemResult` [GUI 0x1002f759]: button 0 -> `N3Msg_UseItem(id, true)`.
Otherwise (and after Yes) the mesh's `VisualMesh_t::AdvertisingUseAction` (billboards: client side, nothing sent; **not ported**, UNRESOLVED) and then
`GenericCmd_t(state 0, seq, cmd 3, ItemActionData{flag 0, actor = own, item = id})` (`FUN_1003aa0f` + `FUN_1007c95c`, queued with `vtable +0x2c`): `Interact::use_object`
(test `world_object_use_is_a_generic_cmd_3` decodes it with `Misc::decode`). Inventory items (kinds 0x65..0x68, 0x6b, 0x73, `0xdead`, `0xdeae`, `0xdac3`, `0xdeaa`: wear / unwear, bank,
reclaim, special actions) are the item windows' (`hud_stats/item_ui.rs`); `Interact::use_item` ignores kinds below 50000. Our dialog is `hud_dialog::Dialogs` (the HUD's `DialogBox_c`), the text
ids are from the code, whether the live LDB has category 0x2715 / key `Item_ConfirmUse` is not checked (empty body otherwise).

### 8.5 Picking non-characters
`Dynels::pick_props` gives the visible, built props (corpses, vending machines, doors and terminals placed by rdb 1000026, items) a `hud_pick::PickBody`: the box over the vertices of the built model
(`Built::held` for CAT models, else the mesh) with the prop's `ActorFrame` transform. [INFERENCE] The original tests plain `VisualMesh_t` bodies against a bounding sphere and their
triangles (`FUN_1006bb2a`, docs/gui.md §13.2); the box stands in for it (the box of a door that has swung open is its closed box). `interact_use::pick_objects` merges them with the characters
(one `hud_pick::hits` list, nearest first) and maps the ids back to identities. **Selection is not extended:** `Zone::target` and the target bars stay character-only (a left click on an object does
nothing but arm the double click), so the original's `SetTarget` on an item (`N3Msg_isIDOnGround`) is not reproduced.

### 8.6 Harness API (`Interact`)
`default_action_on(&zone, identity) -> Action` (`None / Talk / Get / Use / Confirm / Refused(key)`), `default_action(instance)` (characters), `use_item(&zone, identity, confirmed)`, `get_item(&zone, identity)`,
`use_object(identity)` (the raw `GenericCmd` 3, used after Yes), `Interact::can_of(&zone, identity) -> Option<i32>`, `take_feedback()` (refusal keys; `Play::interact_frame` prints them from chat category 110),
`take_outbox()`, and (`cfg(test)`) `loot_dump(&mut gui)`. `interact_use::decide(can)` / `CAN_PICK_UP` / `CAN_USE` / `CAN_CONFIRM`.

### 8.7 An item released over a world object (`FUN_100cb081` [GUI], `N3Msg_UseItemOnItem` [GC 0x100267e0] / `UseItemOnCharacter` [GC 0x100268af])
Object under the pointer when a carried bag item is released (`InputConfig_t+0xb0`): ground (`0x9c47`) -> `N3Msg_DropItem`; a character (50000) -> `GenericCmd_t` cmd 0x20; any other object -> cmd 5, both with
`UseItemOnItemActionData_t{flag 0, actor = own, item, target}`. Ours: `Play::interact_mouse` stores the pick, `hud_stats/item_ui.rs` `Action::UseOn`. [LIVE] works (§12.1).

## 9. The container (loot) window of corpses
Code: `interact_loot.rs`, `interact_use.rs::{watch_objects, use_out}`. Test: `interact_use::tests::corpse_loot_window_opens_with_the_flag_and_a_double_click_takes_an_item`, `interact_loot::tests`.

**Proven flow [CODE]:**
1. Using the corpse sends `GenericCmd_t` 3 (§8.4). A `Corpse_t` has Can 8, so a double click / right click uses it.
2. The server answers with **`InventoryUpdateIIR_t`** (`0x4E536976`, decoded by `ao_net::n3::inventory`): header = the own character, body `capacity, kind, items, container identity (the
   `Chest_t`: kind 0xC76A corpse, 0xC749 chest), word, flag`. `Activate` `FUN_100a040e` [GC]: the header must be a `SimpleChar_t`; a `Chest_t` container gets the new item list (`FUN_1002aeca`), its
   `+0x1dc` = word, `FUN_1004af97` (container view refresh) and, **when flag != 0**, the chest's flags `&= ~0x40`, `vtable +0x40` and **`vtable +0xac(char)` = `FUN_1007e11e`**, then `FUN_100116d5` =
   `GlobalSignals +0x8c` (container changed). `FUN_1007e11e` calls `FUN_1003f5b2(identity, stat 0x1b3 == 0, 0)`: registers the open container and emits **`GlobalSignals +0x84`** (`FUN_10011754`:
   `Identity, bool, bool`) -> `InventoryGUIModule_c::SlotContainerOpened` [GUI 0x100c71a3] (connected in `SlotInitialize` 0x100c722f) -> `FUN_100cc2ca(1, id)`: **`InventoryView_c` of type 1** (a window
   unless one is open already). A SimpleChar container (our own bag etc.) takes the other branch (`FUN_1004775e`, bank / overflow); our own `Zone::apply_inventory` ignores `Update`.
3. The window (`FUN_100cc2ca`): a `MultiListView_c` item view (`FUN_100cc1b3`) with `SetMaxItemCount(0x15)` = **21 cells, 3 per row** (the `item_position_map` loop uses `n % 3`, `n / 3`, `n < 0x15`), for a corpse
   with the pref `esc_corpses` (Esc closes) and the saved config `TempContainer_%u.xml`; the cell art is the inventory's (`GFX_GUI_MULTILISTVIEW_SLOT_48_*`).
4. Taking an item: a double click on a cell (`FUN_100ca1e7` [GUI]; not on the own inventory view `+0x14c`) calls `MoveItemToInventory(item identity)`; the item identity of a corpse window is
   **`{0x6a, container slot}`** [INFERENCE: the dispatcher `FUN_1004ad44` [GC] routes kind `0x6a` to `FUN_10046b0b` (`param_1[0x60]` = the open container -> the own bag, then the +0x8c / +0x8d signals), and
   `N3Msg_UseItem` names kind 0x6a "items cannot be used from a corpse"]. The move is sent like the unequip of `hud_stats` (`N3Msg_MoveItemToInventory(item, bag, 0x6f)` =
   `ClientMoveItemToInventoryIIR_t(item, ANY_BAG_SLOT)`); `Feedback_InventoryFull` without a free bag slot. The server-side checks the client shows are `Feedback_NotAllowedToLoot` and
   `Feedback_YouCantLootNoDropItems` (`FUN_1004b80a`, not evaluated here).

**Ours:** `LootUi` opens a tabbed window with a 3 x 7 grid canvas (cells and spacing of the inventory grid, 54 px slot art, 48 px item pictures from rdb 1010008 through `hud_stats::items::Items`,
tooltip = item name) when an `Update` for a non-character container with flag != 0 arrives, refreshes it on later `Update`s (flag 0), closes it with its close button / Esc; double click on an item sends
the move. The title is the corpse's name from `CorpseFullUpdateIIR_t`'s name blob ("Remains of ..."), centred on the screen.

**[UNRESOLVED]:** (a) no capture of a loot answer exists: the order / flag of the live server's `InventoryUpdateIIR_t` and that `flag` is set on the first answer are from the code only; (b) the window title
(`String::Format` of `FUN_100cc2ca` was not read; the saved window position `container_position` / `container_id` and `TempContainer_%u.xml` are not stored); (c) the original's
list / grid mode switch (`InventoryViewMode`) and the scrollbar of the container view; (d) the identity kind of chest (0xC749) items; (e) team loot (`Feedback_TeamLoot*`, "Random Looter" / "Looter" columns of
`FUN_100ca2d3`) and the "take all" (none found in the view); (f) the window closing message: the original's close path (`SlotContainerClosed`, `GlobalSignals +0x88`, emitted by `FUN_100117f6`
from the bank / reclaim / ... activations) was not traced for corpses, nothing is sent when our window is closed; (g) the look of the window was not compared with a retail screenshot.

## 10. Player trade (`TradeIIR_t`, `TradeView_c`)

Code: `crates/ao-net/src/n3/trade.rs` (codec), `crates/aomac/src/play/interact_ptrade.rs` (window + state). Tests: `n3::trade::tests` (key hash, wire bytes of every client message,
round trips, truncation, bad version / op), `play::interact_ptrade::tests` (default action rules, window layout, op handlers, frames sent by every button, drag & drop, a screenshot with
`AOMAC_SHOT_DIR`). Harness: `Interact::ptrade_dump(&gui)` (cfg(test)), `Interact::trade_action(&zone, id)`.

### 10.1 The message (`TradeIIR_t`, key `36284F6E` = `MapToKey("TradeIIR_t")`)

[CODE] One `n3InfoItemRemote_t` subclass (ctor `FUN_1007a733` [GC], vftable 0x101616e8) for everything. Wire: N3 header (target = the character the state belongs to; for everything
the client sends, the own character), **flag byte 0** (`this[0xc] = 0` after the ctor), `i32 2` (`DAT_101c069c`; read `FUN_1007a60b` rejects any other value), `i8 op`, `Identity a` (`+0x1c`),
`Identity b` (`+0x24`); an op `> 10` (unsigned) fails the read; write `FUN_1007a674`. `Activate` (`FUN_1007a6b6`) takes the header's dynel: a `SimpleChar_t` gets `FUN_100587c7` (lazily creates its
`Trade_c`, `FUN_1006618c`, at `SimpleChar+0x1c8`) and `FUN_100674ab(op, a, b)`; a `VendingMachine_t` goes to the shop path (`FUN_1009a23c`, §13).

| op | client sender [GC] | `a` / `b` | received (`FUN_100674ab` case) |
|---|---|---|---|
| 0 start | `N3Msg_TradeStart` 0x100190e0 | partner / 0 | `FUN_100663e4`: only for the client character; open trade or `Feedback_YouAreAlreadyInATrade`; `GlobalSignals +0xd4 (type, own, a, b)`, type 0 when `b` is zero (player trade), else 1 / 2 (shops, vending) |
| 1 accept | `N3Msg_TradeAccept` 0x10015bfd | 0 / 0 | `FUN_10066598`: client character only, `+0xe0(1)` + `+0x70` |
| 2 abort | `N3Msg_TradeAbort(flag)` 0x10015ccf | `{0, flag}` / 0 | `FUN_100666a3`: clears the state, `Feedback_TradeCancelled` when `flag` (`a.instance`) and the owner is the client character, `+0x70`, `+0xe4(0)`, then the same for the partner's state with flag 1 |
| 3 confirm | `N3Msg_TradeConfirm` 0x10015c66 | 0 / 0 | `FUN_100673a0`: client character only, `+0xe8` |
| 4 complete | - | - | `FUN_100668d1`: `+0xe4(1)`, the items of the trade containers change hands in the client's inventory model (`FUN_1002ada2`, `FUN_1002abc0`), `+0x70` |
| 5 add item | `N3Msg_TradeAddItem` 0x100191f6 | character / item `{0x68, bag slot}` | `FUN_10066cf7`: `+0xd8` |
| 6 remove item | `N3Msg_TradeRemoveItem` 0x1001721c | character / item | `FUN_10066faf`: `+0xd8` |
| 7 cash | `N3Msg_TradeSetCash` 0x10015dbc | `{0, cash}` / 0 | `FUN_100672c7`: `cash >= 0`; `+0xdc(cash)` only when the header is the partner of the client character |
| 8, 9 | - | - | vending machine items add / remove (`FUN_10067109`, `FUN_100671e8`), §13 |
| 10 | - | - | `FUN_1006741b`: client character only, `+0xe0(0)` (accepted state reset) |

`N3Msg_TradeStart` also checks (before sending): the target is a `SimpleChar_t` whose stat `0x184` (`TowerType`) is 0, `FUN_10059ca0` (bounding-sphere distance `<= _DAT_101574fc` = **5.0 m**, dungeon
door test), else `Feedback_TargetOutsideRangeForTrade`; afterwards the local event `0x19` is posted (`FUN_10012a1e`, no network effect found). `N3Msg_TradeRemoveItem` of the own container
refuses with `Feedback_NoRoomInInventory` when the bag has no free slot (`FUN_1002a1b0(0x40) == -1`). `N3Msg_TradeAddItem` refuses by item flags (`vtable+0x14` of the item, with `Feedback_*` texts whose keys the
decompile hides): **not ported** [UNRESOLVED].

### 10.2 Default action

`N3Msg_DefaultActionOnDynel` [GC 0x100291da] on a character that is not talkable (stat `0x300` bit 0), the own character not fighting (`controller+0x44 == 1`): `Interact::trade_action`. The fight state is
`Zone::fight_target` (the relayed `AttackIIR_t` / `StopFightIIR_t` model). The bounding sphere radius of the characters is per model and not known: [GUESS] 0.5 m each (the body collision default), i.e.
the centres may be 6.0 m apart. The `TowerType` of the target is not tracked (assumed 0). `Feedback_TargetOutsideRangeForTrade` goes to the chat.

### 10.3 The window (`TradeView_c`, GUI.dll 0x100e092f; `PlayerTrade` of `Views/TradeGUI.xml`)

[CODE] `InventoryGUIModule_c::SlotStartTrade` [GUI 0x100c6ff4] (signal `+0xd4`): an ignored partner (`IgnoreSystem_t`) -> `N3Msg_TradeAbort(false)`; else an existing trade view is deleted and
`TradeView_c(type, own, partner, b)` is built (`+0x1a4` mode 0 `PlayerTrade`, `+0x1c8` partner, `+0x1d0` own). The view is a `DockableView_c` titled "Trade", preferred `Rect(0, 0, 119.0, 452.0)`
(`_DAT_101bfa98/9c`), docked into `RollupArea` (`FUN_10038c98`). Built from the XML: `PartnerName` (`ScrollingTextView`, text = the partner's name), "Bought Items" (the partner's items), `PartnerInventoryDock`,
"Credits" + `PartnerCashView` (read-only, "0", green `0x44dd44`), "Sold Items", `OurInventoryDock`, "Credits" + `OurCashView` (editable, green), `AcceptButton`, `DeclineButton`, `PartnerStatusDock`
(a `BitmapView_c` with gfx `0x15b` `GFX_GUI_RED_LIGHT` and `0xd9` `GFX_GUI_GREEN_LIGHT`: index 0 / 1). The docks hold `MultiListView_c`s (`FUN_100cdb3a`), view cell counts `(1, 1)..(1000, ..)`, vertical scrollbar.

| signal | slot | effect |
|---|---|---|
| `+0xd8` | `FUN_100dfbf3` | cost display (shop mode only; the lists repaint) |
| `+0xdc (cash)` | `FUN_100dfe5e` | `PartnerCashView` = `String::FormatNumeric(cash)` |
| `+0xe0 (b)` | `FUN_100df971` | 1: own cash field read-only and formatted, status light green; 0: editable again, light red, Accept enabled, `TradeDoubleConfirmDlg` closed |
| `+0xe4 (b)` | `InventoryGUIModule_c::SlotTradeCompleted` [GUI 0x100c6bc6] | the trade view is deleted |
| `+0xe8` | `FUN_100e066c` | `DialogBox_c` "Trade" (name `TradeDoubleConfirmDlg`) with the literal text "Are you sure you want to complete this trade?"; its result `FUN_100df843`: button 0 = `TradeAccept`, 1 = `FUN_100df810` = `TradeAbort(true)` |

Buttons / input: Accept `FUN_100dfd33` = (pending cash sent first) the field is shown formatted, Accept disabled, light green, **`N3Msg_TradeConfirm`** (`N3Msg_TradeAccept` in shop mode); Decline `FUN_100df810` = `TradeAbort(true)`;
the own cash field: each edit starts an `EventTimer_c` (`FUN_100df82d`: `Start(0xf4240, 0, 1)`), when it fires `FUN_100df942` sends `TradeSetCash(atol(text))`; Esc = decline when the option `esc_trades`
(`LoginPrefs.xml`: true) is set; a drag object of mime `inventory/item` without `split_count` dropped on the own list = `TradeAddItem(own, item)` (`FUN_100df870`); item double click on the own list =
`TradeRemoveItem(own, item)` (`FUN_100df67e`); partner list items open the item info page (`itemid://%d/%d`, `FUN_100df73e`, **not ported**).

**Ported:** all of the above in `interact_ptrade.rs`; the inventory drag comes from `HudStats::take_drops` (an inventory item released over a window); lists: items of the own side show the
template read from the inventory when the server adds them; the status light; formatted cash (`hud::group`: ',' [INFERENCE]); the confirmation box is `hud_dialog::Dialogs` (title "Trade" is not shown).
**GUI engine:** `ScrollingTextView` is built as a `TextView` (no marquee scrolling).

### 10.4 Unresolved

* [UNRESOLVED] **What the PRK server sends.** The handlers are the original's, keyed on the header identity (the character the state belongs to): our op 0 reply is expected with the own character as header and
  `a` = the partner; the partner's cash with the partner as header. Not seen live yet (no second account); if the server uses other headers, only `Interact::on_trade` changes.
* [UNRESOLVED] How the partner's item templates reach the client: op 5 carries only an item identity of the *partner's* bag. We show the template when an `InventoryUpdateIIR_t` with the partner's identity
  arrives ([INFERENCE], `Interact::ptrade_inventory`), else an empty slot.
* [UNRESOLVED] Op 4 (complete): the original moves the traded items between the inventories itself; we leave `Zone::inventory` to the server's own inventory messages.
* [UNRESOLVED] Docking: the window is free (left of the rollup column, 192 px wide = the dock's width) instead of a `RollupArea` page; its close button declines. List geometry (3 columns of 54 px slots, `(1,1)..(1000,..)` cell counts
  not mapped), the "Credits" label colour (the engine's `TextView` default), the 1 s timer unit (µs [INFERENCE]), digits-only cash field (feature flag `0x2000` of `FUN_100e039a` not decoded), `Feedback_*`
  refusals of `N3Msg_TradeAddItem`, item info page, `TowerType` of the target and the characters' sphere radii.

## 11. NPC trade (`NPCChatTradeBar_c`, `KnubotStartTrade` / `KnubotTrade` / `KnubotFinishTrade` / `KnubotRejectedItems`)

Code: `interact_trade.rs` (window + flow), `interact_chat.rs` (bar, dock slots), `n3/knubot.rs` (`end_trade`, `trade_item`, `start_trade`). `[GUI]` / `[GC]` addresses of the 32-bit DLLs.

* **Start.** The trade button (§2) sends `KnubotStartTrade(value 0, "")` (`N3Msg_NPCChatStartTrade` [GC 0x1001cee9]). The server's `KnubotStartTrade(value, text)` runs `FUN_10128554` -> GlobalSignals
  `+0xfc` -> `FUN_10058a4c` [GUI] (value = the number of items the NPC takes, text = the NPC's trade text; its format is only known from the live server: [UNRESOLVED-live], we show it verbatim). If no trade bar
  exists (`this+0x140 == 0`): `new NPCChatTradeBar_c(npc, value)` (`FUN_100575a1`), connect its `+0x1a8` signal to `FUN_10058408`, create its window (the same `Window(Rect(50,50,100,100), "", "", style 2, 0x183c)`,
  tab title `GetText(10000, "Trade")`), show it, `DockWindow(slot 2)` of the chat window; then the answer view (`this+300`) gets `<font color=CCNPCChatTrade>text</font>` (`CCNPCChatTrade` = 0x00dadfba).
  Whatever the answer list held is replaced; a second `StartTrade` with a bar open does nothing.
* **The bar** (`FUN_100575a1`, a `BorderView_c` with a `VLayoutNode`, children in this order, all borders 5 unless noted): `TextView_c` `"GIVE ITEM"` (`value == 1`) / `"GIVE ITEMS"` (hard-coded English, font 5 =
  NORMAL); the item container `FUN_100cdb3a` = `ItemContainerView_c(msg, 2, own identity, 0xb, 0x15, 0xe, 0)` (a `MultiListView_c` with an `Icon` column; `SetMaxItemCount(value)`,
  `SetViewCellCounts((value != 1) + 1, (value + 1) / 2)` = 1 column for one item, else 2 columns and `ceil(value / 2)` rows, no scrollbars); `TextView_c` `"GIVE CREDITS"` (borders 5,5,5,0); an inset `BorderView_c`
  (borders 5,5,5,0) around a credits `TextView_c` (HTML flags `0x200d` = ACCEPT_TXT_INPUT | ACCEPT_MOUSE_INPUT | ALLOW_TEXT_SELECTION | NUMERIC, min size `(0, -1)`, max `(16000, -1)`, client borders 3);
  a horizontal row of two buttons: accept `GFX_GUI_BUTTON_V_NORMAL/PRESSED` (0x4b/0x4c, 43 x 25) and decline `GFX_GUI_BUTTON_X_*` (0x4d/0x4e), borders (5,5,5,5) / (0,5,5,5).
  The item cells are our `CanvasView` of 54 px cells (`GFX_GUI_MULTILISTVIEW_SLOT_48_CLOSED` with the 48 px item icon, the inventory grid's metrics: [INFERENCE], `ItemContainerView_c` cell metrics not read).
  The window shows on the black 0.85 alpha style-2 background (`GFX_GUI_WINDOW_BACKGROUND`).
* **Adding an item** (`FUN_100572f2`, the container's drop signal `+0x2dc`): when the container holds fewer than `value` items and the `DragObject_c` has mime `inventory/item` with `container_id`, `item_id`
  and no (or zero) `split_count`: `N3Msg_NPCChatAddTradeItem(own, npc, item)` [GC 0x10017ea0] and `DragObject_c::Accept(2)`; otherwise `Cancel`. The Gamecode side checks the item may be given away
  (`FUN_1004b80a(.., 0x70)` = the "delete item" action check: nodrop / bank / vehicle rules, not ported: the server validates) and moves it to the trade container (`FUN_1004aab1`: an inventory event 0x11 into slot
  0 of container type 0, the client-side effect of the `ItemContainerView_c`). Wire: `KnubotTrade(op 0, Identity 0:0, item)`. Ours: the inventory drag of `hud_stats/item_ui.rs` reports a drop over a
  foreign window (`Dnd::dropped` -> `Hud::take_item_drops`), `Play::interact_trade_frame` hands the ones over the container to `Interact::trade_drop`, which mirrors the item in the container
  (the original mirrors the server-side container: [UNRESOLVED-live] what the server sends back, nothing is expected: `KnubotTrade` has no `Activate`).
* **Removing** (`FUN_10057421`, signal `+0x2e0`) -> `N3Msg_NPCChatRemoveTradeItem(own, npc, item)` [GC 0x10017f31] = `KnubotTrade(op 1, 0:0, item)`; the other signal `+0x2e4` (`FUN_10057473`) ->
  `ShowURL("itemid://%d/%d")`. [UNRESOLVED] which gesture raises which signal; ours: left double click removes, right click shows the info.
* **Accept** (`FUN_100574dc`): `cash = GetSkill(0x3d (Cash), 2)`, `n = atoi(credits text)`; when `n > cash` the field is rewritten to `cash` and `n = cash`; `N3Msg_NPCChatEndTrade(own, npc, n, true)` [GC 0x10017fb1],
  which lowers the own Cash (stat `0x3d`) by `n` locally (`SetStat(0x3d, cash - n)`) and sends `KnubotFinishTrade(flag = !accepted = 0, value = n)`. **Decline**, the trade button of a running trade, and the
  bar's `+0x1a8` signal (`FUN_10057289`, also connected to a GlobalSignals slot) run `FUN_10058408`: `this+0x188 = 3` (the composer's last text type), `N3Msg_NPCChatEndTrade(own, npc, 0, false)` (the
  `accepted == false` branch calls `FUN_100587c7` / vtable `+0x10(8)` / `FUN_100666a3(0)`, which shows `Feedback_TradeCancelled` when a player trade was active; ours: nothing) =
  `KnubotFinishTrade(flag 1, value 0)`, then `FUN_100582b7` deletes the bar and its window. (The previous doc of `end_trade` had the flag inverted.)
* **End.** `KnubotFinishTrade` S->C has no `Activate`. `KnubotRejectedItems(items, value)` `Activate` `FUN_101281c6`: walks the own inventory's slots and re-lays the items that came back (`FUN_1002ada2` /
  `FUN_1002a946` per slot; ours: the item positions are kept by the server's `InventoryUpdated` messages, [UNRESOLVED] the local re-layout), `Cash += value` (`SetStat(0x3d, cash + value)`: the credits that were not
  taken), then emits GlobalSignals `+0x100` -> `LAB_10058362`: `this+0x188 = 3`, `FUN_100582b7` (the trade window is deleted). Ours: `Interact::on_trade_knubot`.
* Closing the dialogue (player or server) deletes the trade window with it (`NPCChatView_c` owns the bar: `FUN_10059d7f` destructor).
* Harness API (`#[cfg(test)]`): `Interact::trade_dump(gui)`, `press_button(gui, i)` (0 description, 1 info, 2 trade, 3 use; false when the button is disabled), `trade_add(gui, item)`, `trade_accept(gui, zone,
  credits)`, `trade_decline(gui, zone)`; non-test: `trade_drop`, `trade_wants`, `take_cash_delta`, `take_info_urls`. Tests: `interact_trade::tests` (flow with synthetic Knubot frames, wire frames, drops, double click,
  Cash clamp, windows closed with the dialogue), `interact_chat::tests` (bar art ids, enable flags, clicks, splitter drag through real pointer events and the clamp), `n3::knubot::tests::trade_wire_layout`.
  `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac npc_dialogue_render` writes `npc_answers.png` (bullets, green answers, 10 px hanging indent, bar below, splitter line) and `npc_trade.png`.

## 12. Live results (PRK "Ithaca", Aomacvolk, 2026-10-06) [LIVE]

Harness steps (`flow/live.rs`): `props=<r>`, `use=` / `ruse=<kind>:<inst>` (default action / right click), `useon=<slot hex>:<kind>:<inst>`, `talk`, `answer`, `btn=<i>`, `tadd=slot:<hex>`, `taccept`, `loottake=<cell>`, `lootid=`, `grid`, `inv`.
Captures (login traffic excluded): `docs/captures/zone_npc_dialogue_ithaca.rec`, `zone_npc_trade_ithaca.rec`, `zone_use_object_ithaca.rec`; replay tests `crates/ao-net/tests/interact_captures.rs`.

### 12.1 Off the ICC beach (pf 4582 -> 4833 -> pf 800 Borealis)
1. Plateau door (`Door_t` 0xC748, Can 1032, (936.1, 47.6, 888.4)): right click = `UseItem` (echo), walking through it enters **pf 4833 "ICC Shuttleport"** (its door at (185, 6.1, 144) leads back).
2. Dialogues: Teleporter Technician (4833, instance 1001786) "use it on the portal to Borealis"; Travis Molen -> recruiters / Neutral Observer; Neutral Observer "use this access card to activate the teleporter".
3. The card is the starting-bag **key item 249721** (slot 0x44). Released over a teleporter prop (0xC73D, pf 4833) it is `UseItemOnItem` (GenericCmd cmd 5, §8.7). Wrong teleporter: `FormatFeedback`
   "This is not the correct key"; wrong item: "You can not use this item on the teleporter"; the right one (instance -1073474847, (189.5, 6.3, 190.6)) redirects to **pf 800 (Borealis)** at (679.6, 72.8, 476.7). ICC cannot be re-entered.
   Newland is not offered to a level-1 character; Borealis is (three faction recruiters).
4. Other 0xC73D objects are billboards (echo only); 0xC773 is a mail terminal (chat line "Unknown entity used: MailTerminal").

### 12.2 NPC dialogue and trade
* Server Knubot frames carry header flag byte 1, the client's own 0 (`encode` writes 0; the replay tests patch the byte).
* `Open` flags: Technician, Travis, Observer, Clan Recruiter b20=b21=false; vendors Antonio Stacklund (plateau) and Aleksei Innokenti (Borealis) b21=true (trade button). b20 never set.
* Trade button -> `StartTrade` (empty text) -> server `StartTrade{1, "Place your items in the trade window."}` -> trade window (screenshot checked). Add item `Trade{op 0, zero, item}`; accept `FinishTrade{flag 0, value 0}`;
  Aleksei answers `RejectedItems{[({low,low}, 3, 1234567890)], 0}` + "I don't think you have anything I want.".
* Walking away: `Close{5, "You are too far away from <npc> to continue this conversation."}`.

### 12.3 Objects
* Vending machine (0xC75B): echo, then `ShopUpdateIIR_t` (key 58362220, 36 items) and a `TradeIIR_t` **op 0** (START, not op 2: version word `00000002`, op byte `00`) pair: header = own, `a` = the machine, `b` = `{0xC767, 0x116f7753}` (a session identity); then header = the machine, `a` = own, the same `b` (buy UI: section 13).
* Corpse (0xC76A, "Remains of Uncle Pumpkin-Head"): echo + `InventoryUpdateIIR_t` (flag set) -> loot window (screenshot checked). Double click sends `MoveItemToInventory({0x6a, cell}, any)`; the server ignored it and
  the `entry.id` identity for a corpse another player killed. [UNRESOLVED] taking from our own kill is untested (no kill was reachable).
* Grid terminal / whompah: none in pf 4582/4833/800, `GridDestinationSelect` never received [UNRESOLVED-live]. Player trade needs a second player [UNRESOLVED-live].

## 13. Vending machines / shops

Code: `crates/ao-net/src/n3/shop.rs` (codec), `crates/aomac/src/play/interact_shop.rs` (window + flow, hooks in `Interact::{on_frame, event, close_all}`, `on_trade` of `interact_ptrade.rs`, `interact_mouse`), harness steps in `flow/live.rs`.
Tests: `n3::shop::tests`, `interact_captures::vending_machine_stock_and_trade_start` (the captured 465-byte frame is decoded and re-encoded byte for byte), `interact_shop::tests` (incl. `live_capture_opens_the_window`, a replay of frames 100825..100830 of `zone_use_object_ithaca.rec`), screenshot `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac shop_window_screenshot` (`shop.png`, inspected: title "Trade", "Shop" list, "Bought Items" list, Credits, Accept / Decline).

### 13.1 `ShopUpdateIIR_t` [CODE + LIVE]
An `n3InfoItemRemote_t` (`FUN_100a0fb4` [GC], key `MapToKey("ShopUpdateIIR_t")` = `58362220`, vftable 0x10166c88: read slot 7 `FUN_100a0ef9` -> `FUN_1009a55e`, write slot 8 `FUN_100a0f18` -> `FUN_100995ee`, Activate slot 2 `FUN_100a0f2c`).
Header target = the machine, flag byte 0. Body: `i32 (n + 1) * 0x3f1` (`FUN_1009a55e` accepts a positive multiple of 0x3f1 whose quotient - 1 is < 1000, else the stream is flagged bad), `n` x (`i32 low_id`, `i32 high_id`, `i32 ql`) = the arguments of `ACGItem_t(low, high, ql)`.
[LIVE] The Borealis machine sends 36 entries (`low == high`, ql 1 / 100 / 150 / 200). `Activate` (`FUN_100a0f2c`): the header dynel must be a `VendingMachine_t`; `FUN_1009a4b2(list)` rebuilds the machine's stock (`FUN_1009a3c9` clears `+0x1fc`; per entry an inventory event for the identity
**`{0x6f, index}`** (`FUN_100153eb(&{0x6f, i}, ...)`, appended to the list at `+0x1e4`)); an empty result says **`Feedback_ShopContainsNoEntries`**. These are the only two shop classes the client registers (`s_ShopUpdateIIR_t` 0x101568e8 and the already known `VendingMachineFullUpdateIIR_t` 0x101569fc: grep of the symbol table for `Shop` / `Vending`).

### 13.2 Starting the trade [CODE + LIVE]
`TradeIIR_t` op 0 with a **non-zero `b`** (the player trade has `b == 0`, §10.1). Header = own character: `FUN_100663e4` (`a` = the machine, a `VendingMachine_t`) computes the type (`FUN_1009959d` = the currency stat id, default 0x3d Cash; `FUN_100995e0` selects type 1 `ShopTrade` or **2 `ShopBuy`**: the plain machine is type 2 [INFERENCE: the function's body is a thin getter that was not followed]) and emits `GlobalSignals +0xd4 (type, own, a, b)` ->
`InventoryGUIModule_c::SlotStartTrade` [GUI 0x100c6ff4]: an existing trade view is deleted, a new `TradeView_c(type, own, partner = machine, b)` [GUI 0x100e092f] is built (`+0x1a4` type, `+0x1c8` partner, `+0x1d0` own, `+0x1d8` b). The copy with header = the machine (`FUN_1009a23c` case 0) creates the machine's per-player session record (`{trade list, a, b, min(own stat 0xa1, 3000)}`).
An open trade refuses with `Feedback_YouAreAlreadyInATrade` (shop or player trade).

### 13.3 The window (`TradeView_c` type 2, view `ShopBuy` of `Views/TradeGUI.xml`) [CODE]
* Title "Trade", client 192 x 453 like the player trade (§10.3: the dockable view, docking into the `RollupArea` is not ported: a free window left of the rollup column). Config DValue `ShopViewConfig` (not persisted), `esc_shops` (`LoginPrefs.xml`: true) = Esc declines.
* View XML `ShopBuy`: `PartnerName` (the machine's name: `N3Msg_GetName`, or its parent's), "Shop" + `ShopInventoryDock`, "Bought Items" + `PartnerInventoryDock`, "Credits" + `PartnerCashView` (read-only, "0", green 0x44dd44, `FUN_100e039a`), `AcceptButton`, `DeclineButton`. There is **no** own credits field and no status light in this view (those are `PlayerTrade` / `ShopTrade`).
* `ShopInventoryDock` (`FUN_100dfeca`): `ItemContainerView_c` `FUN_100cdb3a(msg, mode 1, machine, flags 0xf, 0x15, 6, 0)`: mode 1 = `N3Msg_GetContainerInventoryList(machine)` = the stock; cell counts (1,1)..(1000,1000), horizontal scrollbar off, vertical auto. `PartnerInventoryDock` (`FUN_100e0053`): mode 2 = `N3Msg_TradeGetInventory(machine)` = `FUN_1009991a` (the session's list, the bought items), flags 0xf in shop types (0xb in the player trade), cell counts (1,1)..(1000,1) (one row).
* Columns by flags (`FUN_100cdb3a`): Icon (id 0, 16 px), Name (1, 200), Count (2, 30; flag 1), Price (3, 100; flag 4), Quality (4, 100; flag 8); list sorted by Name. Row cells (`FUN_100404a9`, `FUN_100cd4b6`): count = stat 0x19c (0 -> 1), **price = `N3Msg_GetShopItemStat(item, 0x4a)`** (`n3EngineClientAnarchy_t` 0x10017a38: the item's `VendingMachine_t` virtual +0x108; shown empty when 0 or 0x499602d2), quality = level; the tooltip of the price (`FUN_10031562`) is `FormatNumeric(price)` in the text LDB 0x1fa.
* Credits (`FUN_100dfbf3`, GlobalSignals `+0xd8`, emitted after every item add / remove): `PartnerCashView` = `FormatNumeric` of the cost of the list (`FUN_10099d56`: sum of price x count over the bought items); the machine's `TradeIIR_t` op 7 sets it directly (`FUN_100672c7` -> `+0xdc` -> `FUN_100dfe5e`).
* Buttons: Accept (`FUN_100dfd33`) disables itself and sends **`N3Msg_TradeAccept`** (`TradeIIR_t` op 1; the player trade sends `TradeConfirm`); Decline (`FUN_100df810`) = `N3Msg_TradeAbort(true)` (op 2, `a = {0, 1}`); the frame's close button and Esc decline too.
* Items: a double click on a list item is `FUN_100ca1e7`: with Shift / Ctrl (`View::GetQualifiers() & 0xc`) and a trade view open **`N3Msg_TradeAddItem(own, item)`** (`TradeIIR_t` op 5 `a = own, b = {0x6f, index}`; `N3Msg_TradeAddItem` [GC 0x191f6] treats `kind 0x6f` as a shop item and, before sending, refuses with a credits message when `FUN_100662b7` (cost so far + price - own credits) is positive), otherwise **`MoveItemToInventory(item)`** = `ClientMoveItemToInventoryIIR_t({0x6f, index}, any bag slot 0x6f)` (what the loot window sends for a corpse item, §9; `N3Msg_MoveItemToInventory` [GC 0x10027a30] with the source check `FUN_1004b80a`, whose 0x6f branches were not decoded). A double click on a bought item removes it (`FUN_100df67e` -> `N3Msg_TradeRemoveItem(own, item)`, op 6; [INFERENCE] that the handler is wired to this list).
* Server messages the window handles (`FUN_100674ab` cases): op 5 / 8 (`FUN_10066cf7`, `FUN_10067109`: vending add, `FUN_10047268(0x21, item)`) add `b` to the bought list, op 6 / 9 remove it, op 7 from the machine sets the credits, **op 4 (complete)** with header = own closes the window (`SlotTradeCompleted`), **op 2 (abort)** closes it (`Feedback_TradeCancelled` when sent by the machine [`FUN_1009a23c` case 2 -> `FUN_100666a3(1)`] or `a.instance != 0`), a zone change closes it silently (`Interact::close_all`).

### 13.4 Port
`Shop::open` builds the window from the `ShopBuy` element of the client's `TradeGUI.xml` (the engine's loader opens only the first view of a file), puts one `MultiListView` per dock and fills them lazily per frame (`Play::interact_shop_frame`: the item templates come from the hud's `Items`; price = the template's stat 0x4a `Value`). Names of machines come from `VendingMachineFullUpdateIIR_t`'s name blob (empty when it was not seen).
`Interact::shop.quick` = Shift or Ctrl held (`host.mods`, set per input event). Harness (`cfg(test)`): `shop_dump(&gui)` (name, stock rows `[i] Name | Count | Price | Quality`, bought rows, credits, the Accept state, the trade log), `shop_buy(&mut gui, index) -> bool` (plain double click on stock item `index`: `MoveItemToInventory({0x6f, index})`), `shop_add(&mut gui, index)` (Shift double click: `TradeAddItem`), `shop_remove(&mut gui, bought_index)`, `shop_press(&mut gui, accept)`; live steps `shop`, `shopbuy=<i>`, `shopadd=<i>`, `shoprm=<i>`, `shopaccept`, `shopdecline`.

### 13.5 Not ported / unresolved
* [UNRESOLVED-live] Which of the two buy gestures the PRK server answers: after `shopadd` the server should echo `TradeIIR_t` op 5 (or 8) into the bought list and Accept (`op 1`) end with op 4; `shopbuy` (the corpse's `MoveItemToInventory` was ignored for a foreign corpse, §12.3) may be ignored the same way. Whether the server sends the bought items as an `InventoryUpdateIIR_t` / sets the credits with op 7 is not known: the port tracks ops 5/6/8/9/7 only.
* [UNRESOLVED] **The exact price.** `N3Msg_GetShopItemStat` calls `VendingMachine_t` virtual +0x108 (stat 0x4a) with the client character; the vftable slot could not be isolated (the class has five vftables, slot 66 of the SimpleItem base is an unrelated check), so the template's `Value` (stat 0x4a of the rdb 1000020 record `low_id`, no QL interpolation) is shown. The client-side credits check of `N3Msg_TradeAddItem` (`FUN_100662b7`, message text LDB 0x6e) is not ported: the server validates.
* [UNRESOLVED] Type 1 `ShopTrade` (player shops / the machine with `FUN_100995e0` true; has the own items dock and cash field, and `FUN_100df870` drops on all three docks) is not built: no such server message was seen. The type decision for a plain machine is [INFERENCE].
* [UNRESOLVED] List look: the engine's `MultiListView` has no picture cells (the Icon column is missing), the column widths are squeezed to the dock (Name 56, Count 40, Price 46, Quality 44 instead of 200 / 30 / 100 / 100: [GUESS]), grid vs list mode (`FUN_100cda29` / `InventoryViewMode`) is not mapped, the bought list's one-row cell counts are not reproduced; the `RollupArea` docking, the item info page (`itemid://`) and the saved `shop_listview_config` / `partner_listview_config` are not ported. The machine's name is empty without a captured `VendingMachineFullUpdate`.
