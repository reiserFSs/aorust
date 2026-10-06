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
* `NPCChatView_c` (`FUN_10058ed8`): vertical layout of two `TextView_c` (HTML parser features `0x6c`, vertical scrollbar mode 2 = auto): the text at `+0x128`, the answer
  list at `+0x12c`, with a draggable splitter bitmap between them (`BitmapView_c`, mouse pointer 10). Both have borders of 5 px. The text view's height is
  `floor(split * (height + 1) - 1)` with `text_split_proportion` = 0.7 (`FUN_10058100`, `_DAT_101af644`); the splitter drag (`FUN_10058371`) stores a new proportion
  clamped to a range (constants `_DAT_101b2050/54/58`, not read) -- **the drag is not ported**.
* Text composition (`FUN_100586fd`, the `AddText` slot; ported as `interact_chat::Composer`, tests in that file). `type` = `NPCChatTextType_e`:

| type | HTML prefix | note |
|---|---|---|
| 0 | `<font color=CCNPCChatText>` | when the previous text was not type 0: `<br>` (only if the view has text) + NPC name + `": "` |
| 1 | `<font color=CCNPCOOCText>` | `<br>` unless the previous text was type 1 or the view is empty |
| 2 | `<br><font color=CCNPCChatQuestion>` | the echoed answer: only with the pref `ShowNPCQuestions` (IndependentPrefs, absent from every prefs xml = 0) as `<own name>: <text>`; otherwise nothing but the empty font tags |
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
* The button bar (`ButtonBar_c`, `FUN_10059704`): four `Button_c` (gfx 0x3f/0x40, 0x44/0x45, 0x42/0x43, 0x49/0x4a, tooltips LDB 0x2710): request description
  (`FUN_100584f1` -> `N3Msg_NPCChatRequestDescription`), `FUN_100582eb`, trade (`FUN_1005852c` -> `N3Msg_NPCChatStartTrade`, or end of a running trade), and the fourth
  (`LAB_1005834a`). Enable flags: description = `b20` of `KnubotOpenChatWindow`, trade = `b21`, the fourth = bit 21 of the NPC's stat 0. The bar is its own window docked
  to the chat window (`FUN_10058577`, dock position 5, tab title LDB 0x2710). **Not ported** (see §6).
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
