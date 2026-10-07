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
  The constructor `FUN_10058ed8` binds the answer view's link signal through `FUN_10059579` to this numeric-answer handler; that trace does not establish transcript URL routing.
  The bullets and the hanging indent are the HTML parser's: `ao-gui` `text::layout_text` now implements them [CODE: `HTMLParser_c::_ParseTag` 0x1015c9ad, `FUN_1015ed05`, `TextRenderer_c::_ReWrap`
  0x10161ba4 / `_AddLineDesc` 0x10161b44 / `_RenderLine` 0x10161112]: `<img src=tdb://id:NAME>` (or `rdb://`; token 3, the sprite of `GuiResourceManager_t::GetGuiTexture`) is an inline sprite copied
  top-aligned at the pen (`SpriteInfo_t::Copy`), its width advances the pen and counts as a word; `<div>` / `<center>` are tokens 0xc / 0xd, `indent=wrapped` sets bit 1 of the div's attribute
  word: `_ReWrap` runs a line counter (-1 outside, 0 in the div, +1 per `_AddLineDesc`) and gives every line with counter > 0 the indent `LineDesc+4 = 10` px (`TextLine::indent`, included in
  its width). A div is a block (a line of its own); the chat window's `<br>` joins right behind a `</div>` add nothing, so the chat keeps its look. `<a href style=text-decoration:none>`:
  `_ParseTag` sets the attribute bits 6 (underline + link colour `+0x1c0` = 0x2299ff) **only when the style is absent or something else** -- with `text-decoration:none` the run keeps the
  font colour, which is why the answers are `CCNPCChatQuestion` green (0x4fd553) and not link blue. Tests: `ao-gui/tests/html_text.rs`.
* **User-requested extension:** links clicked in `npc_text` emit `ChatOut::Link(href)` unchanged, enter the existing `trade.info_urls` queue, and reach `Chat::show_url` via `interact_trade_frame`.
  This is requested transcript support, **not a claim of proven retail NPC transcript behavior**. `npc_answers` remains a separate numeric-index path, including its answer echo, answer-list clearing and wire message.
  Regression coverage in `interact_chat::tests` clicks rendered glyphs with GUI mouse input for an answer and transcript URL schemes (including a numeric transcript href); `interact_trade::tests` covers queue routing without an answer packet.
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
  is built; ours reads `Zone::stat_of` from the shared full-update / `StatIIR_t` character skills, without a separate dialogue cache), info always. The bar is its own window
  `Window(Rect(50, 50, 100, 100), "", "", style 2, flags 0x183c)` with one tab titled `GetText(10000, "Tab_Tools")` (never shown: style 2 = the black 0.85 alpha background, no frame), shown and docked
  to the chat window with `Window::DockWindow(slot 5)`: `Window::LayoutDockedWindows` [GUI 0x101550ed] puts slot 5 left aligned below the window (`x = L, y = B + 1`, inclusive rect); slot 2
  (the trade window) at `x = R, y = T` stacked downwards. Port: `interact_chat::{ChatTexts, BarFlags, dock_below, dock_right}`, tests in `interact_chat` / `interact_trade`.
* Closing the window with its close button runs the destructor `FUN_10059d7f`: `N3Msg_NPCChatCloseWindow(own, npc)` unless `this[0x98]` is set (the server closed it).

## 3. `N3Msg_DefaultActionOnDynel` (right click, left double click)

[GC 0x100291da], called from `ActionViewMouseHandler_c`'s release (docs/zone/combat-net.md §5.2) with the identity under the pointer. For a **character** [CODE]:

1. `HasStat(0x8000000)` of the target -> nothing; the own character's stat `0x296` ( = in a vehicle) non-zero -> chat feedback `Feedback_NotInVehicle`.
2. For a living target (`SimpleChar+0x138` bit 4 clear), stat `0x300` bit 0 opens `KnubotOpenChatWindowIIR_c` (header = own character, NPC identity, `0, 0`; `FUN_10127f4d`).
3. Otherwise a living target that is a player (`+0x21c == 0`) or lacks Flags bit `0x200000` starts player trade only when the **target's** fight controller (`+0x1d4`, `+0x44`) is idle (1).
4. All remaining characters use the same target-idle gate. With target Flags `0x200000` and own Flags bit 8, abort the trade (`N3Msg_TradeAbort(true)`); otherwise `N3Msg_UseItem(target, false)`.

The target controller in steps 3–4 is explicit in the decompile of 0x100291da; the previous description incorrectly called this the own character's fight state. `+0x21c` is NPC and `+0x138` bit 4 is dead (docs/zone/combat-log.md §1; death action 99, combat-anim.md). Self identity is rejected before all branches.

For a non-character `Identity` (item / world object): stat `Can` (0x1e) bit 0 -> `N3Msg_GetItem`, bit 3 (8) -> `N3Msg_UseItem(identity, false)`.

`N3Msg_UseItem` [GC 0x100286f8] for a world object: not an inventory item, not a character -> `GenericCmd_t` (`52526858`) `state 0, seq, cmd 3` with
`ItemActionData{flag 0, actor = own character, item = the object}` (`FUN_1003aa0f`, `FUN_1007c95c(actor, data, 3)`), queued with the engine's `vtable+0x2c`
(docs/zone/misc.md §8 has the layout; the server's confirmations are the `state 1` copies in the capture).

## 4. Interaction inventory and original access paths

This client version does **not** open a world/dynel popup for right-click: GUI `FUN_1002c469` button 2 calls DefaultActionOnDynel for kind 50000, UseItem otherwise (§8.1). A new popup with Info/Follow/Assist/Trade/Invite would not reproduce that handler. GUI import cross-references corroborate the access paths below: `TeamJoinRequest` pointer 0x101a77c0 is used by `SlotForceInviteDlg` 0x1007b2c9, selected-target invite 0x10077c8e, and LFT invite 0x100eff95; `AssistFight` pointer 0x101a78a8 is used by the chat handler 0x100b7bc5. These are not `AppendMenuEntry` builders.

| Target / action | Original path | Message / window |
|---|---|---|
| Quest giver / conversational NPC | Right-click or left double-click, stat 0x300 bit 0 | KnuBot open, append text, answer list; NPCChatWindow (§1–2) |
| NPC give-items trade | NPC chat Give button, server b21 | KnuBot StartTrade / Trade / FinishTrade; NPC trade bar (§11) |
| NPC shop/use | NPC Flags bit 21; chat Shop button or default action | UseItem GenericCmd 3; server-selected trade/shop window (§13) |
| Player trade | World default action, living idle player | TradeIIR START; TradeView (§10) |
| NPC / player Info | Shift+left click; NPC chat Info button; team-row InfoOn menu | charid URL, RequestInfoPacket; InfoView |
| Player Follow | `/follow`, including initial hotbar Follow macro | FollowTargetIIR_c, key 0x260f3671 (chat/cmd.md) |
| Player Assist | `/assist` | N3Msg_AssistFight 0x10027374 reads target's fight target |
| Team invite | Team Recruit button; normal special action; chat/LFT invite | TeamJoinRequest CharacterAction 0x1a; team window (hud_team.rs) |
| Pet commands | CommandMenu → PetMenu.xml | RunChatScript `/pet follow/behind/wait/guard/attack/terminate/report/heal`; text-command codecs |
| Door / terminal / ground item / corpse | Right-click UseItem; left double-click Can bit 0 GetItem, else bit 3 UseItem | ClientGetItem or GenericCmd 3; server response decides container/shop/mission/bank/grid UI |
| Item onto NPC / object | Inventory drag released over dynel | GenericCmd 0x20 character, 5 object (§8.7) |

Features stat 0xe0 controls attackability pointers (NPC 0x20000000; player 0x4000001), follow permission, and grid window bit 0x800 (§7). It does not imply an invented world menu. The pet menu's XML Actions and special-action categories are the existing ControlMenu subsystem, not world right-click popups.

## 5. Bank and reclaim

`BankIIR_t` key 0x343c287f reads the inventory vector (`FUN_10071fcf` → `FUN_10125657` → `FUN_1002a41a`); `BankCorpseIIR_t` key 0x52213420 reads the same vector followed by an i32 reclaim instance (`FUN_10071e42`). Bank activation 0x10072002 assigns own instance to `{0xdead, own}`, registers it and opens the container through GlobalSignals +0x84; reclaim activation 0x10071e95 registers `{0xdeae, wire_instance}` without the open flag.

GUI `SlotContainerOpened` 0x100c71a3 builds the same `InventoryView_c` type 1 as a corpse (0x100cc2ca), with `bank_window` and bank IconPosition map type 3. `ItemContainerView_c` 0x100cdb3a gives banks **102 cells**, ordinary chests 21; banks are not registered for Esc closure. `interact_bank.rs` / `interact_loot.rs` reuse the container renderer with variable capacity and scrolling. The reused 3×7-cell visible viewport is [INFERENCE], not a retail screenshot measurement; saved bank frame and icon-position persistence remain the container renderer's unresolved configuration path.

Bank items have identity `{0x69, slot}`. Deposit is `ContainerAddItemIIR_t` to the bank; confirmation assigns the first free bank cell 0..101 (`FUN_100492fa`). Withdrawal uses MoveItemToInventory and the first free bag cell (`FUN_10046a3d`, ignoring the confirmation's requested slot). `/bank close` is local (`FUN_10046bdd`), routed through the chat game action to `Interact::bank_close`; it sends no command. Regression checks cover bank/reclaim wire bounds, 102-cell window behavior and confirmed inventory transfers.

## 6. NPC and player information

`charid://kind/instance` is not the item generator 0x100384f3 (which returns empty for character kinds). GUI 0x100ee05c → 0x10031011 requests the character packet. GC `N3Msg_RequestInfoPacket` 0x10027271 → 0x1003f5fc builds **CharacterActionIIR_t action 0x69**, identity_a = requested character, param = 0, identity_b = zero, empty text; header = own and passed-on byte 0 (constructor 0x1007253f).

`InfoPacketIIR_t` key 0x4d38242e (registration 0x1000f4f2, constructor 0x1007495e) reads via 0x100749bf → 0x10045d02. The full conditional layout is implemented in `ao-net/n3/info.rs`, including grid destination lists (0x10128f1e) and ACG items (0x1004662a). Activation 0x10074a1d → 0x10045fba applies the packet's numeric skills and 0x1003e694 caches it in the info holder at +0x7c and emits GlobalSignals +0x34. `Zone::info_packets` retains those packets; the existing InfoView request/response path invokes the character HTML renderer instead of discarding charid URLs. GUI formatter dispatcher 0x1003843f selects NPC 0x100336fb or player 0x10033db4.



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
* The keybind "Use" (hotbar special action 3, `FUN_1004256c`) is `N3Msg_UseItem(target, false)` on the complete current target identity (`hud_use.rs::use_target`).
  Characters and selected world objects share `Interact::use_item`, including the object's confirmation gate; Pick-Up likewise retains the selected identity.

### 8.2 `N3Msg_DefaultActionOnDynel` for a non-character [GC 0x100291da, CODE]
`FUN_10058e36(id)` is "kind == 50000 and the dynel exists"; otherwise `FUN_1008720e(id)` = `GetDynel` cast to `SimpleItem_t` (all of corpses, vending machines, doors, terminals, ground items;
a dynel we do not know = nothing happens), then `Can` = `GetStat(0x1e, 2)`:

| `Can` | action |
|---|---|
| bit 0 (1) | `N3Msg_GetItem(id)` |
| else bit 3 (8) | `N3Msg_UseItem(id, false)` |
| else | nothing |

`interact_use::decide(can)` is that table (`CAN_PICK_UP` / `CAN_USE`); `Interact::default_action_on(zone, identity)` also implements the character branches of §3.
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
(one `hud_pick::hits` list, nearest first) and maps the ids back to identities. Plain left release now cycles that merged identity list (`N3Msg_GetNextTarget`, GUI `FUN_1002c469`)
and stores the entire identity via `Zone::set_target`; the old `target` field is only the character projection for character-only combat/skill consumers.
The captured `zone_ithaca.rec` actor regression must not assume its aimed corpse is the first collision:
the observed overlapping ray returned character `50000:1026268` before corpse `51050:5628`.
N3 `0x1000fc8d` → `0x1000f8e5` sorts by collision distance in model units (see `hud_pick.rs`), not aimed identity.
`captured_dynels_become_actors` retains that ray and all candidates, checks the complete identity order against
the individual body collision distances, and requires the aimed corpse exactly once.
`SetTarget` [GUI 0x100257b0] refuses a non-forced identity unless `N3Msg_isIDOnGround` succeeds; selection is cleared when the dynel disappears or gains a parent
(`FrameProcess` 0x10025fa4). Empty world hits do not deselect. Shift requests `itemid://kind/instance` without changing the selection; Ctrl/Alt attack applies only to characters.
Retail `FUN_100744ae` marks non-characters nonattackable, and `FUN_10073d0f` uses the friendly control caption **Selection**, not **Nano Target**; the object name is white,
the health slider's default tint is white (`FUN_1007313a`, `Consider != 3`), and MaxHealth/Health come from the selected object's stats. A simultaneous character fight remains
on the hostile control; complete identities are compared, so equal instance numbers in different kinds never merge. `SelectSelf` restores the complete previous object identity.
`LookAtIIR_t` announces non-character selections with mode 0 (`FUN_1003fb35` → `FUN_1003b0db`, docs/zone/combat.md).
Regression checks: `hud_target::tests::object_selection_keeps_identity_and_uses_the_retail_selection_header` and
`combat::module::tests::object_selection_announces_complete_identity_with_mode_zero`. The existing renderer's missing ground-ring/non-character overhead indicator remains
outside the target-window implementation (GUI 0x100257b0 still creates that original `Indicator_t`; docs/zone/motion.md §6).

[LIVE, 2026-10-06, Fix8World] Offscreen `cargo test --release -p aomac live_walk -- --ignored --nocapture`, Aomacfixr with
`AOMAC_LIVE_STEPS='approach=corpse,S=1,clickcorpse,shot=corpse-selected'`, passed (51.78 s). Corpse identity `51050:11930` was on-ground;
release hits contained that same identity and selection remained `Some(51050:11930)` both immediately after release and after the next frame.
Fix8World inspected the frame: friendly dock caption **Selection** and **Remains of Redevefoot the Agitated Wreck**, not **Nano Target**.
An earlier harness run clicked the first edge pixel but ticked the settling camera between mouse events and missed at release; the harness now dispatches ordinary
move/down/up against the camera/pose used for the pick in one frame. No production fallback, forced selection or retry was added.

### 8.6 Harness API (`Interact`)
`default_action_on(&zone, identity) -> Action` (`None / Talk / Get / Use / Trade / Abort / Confirm / Refused(key)`), `use_item(&zone, identity, confirmed)`, `get_item(&zone, identity)`,
`use_object(identity)` (the raw `GenericCmd` 3, used after Yes), `Interact::can_of(&zone, identity) -> Option<i32>`, `take_feedback()` (refusal keys; `Play::interact_frame` prints them from chat category 110),
`take_outbox()`, and (`cfg(test)`) `loot_dump(&mut gui)`. `interact_use::decide(can)` / `CAN_PICK_UP` / `CAN_USE` / `CAN_CONFIRM`.
The live harness also accepts `clickobj=<kind>:<instance>` and `rightobj=<kind>:<instance>` (decimal identity components): it finds an unobstructed on-screen point
whose merged pick list starts with that identity, then sends the ordinary mouse events through `Play::input`; diagnostics print the complete clicked and selected identities.
`approach=corpse` walks toward the nearest existing corpse's x/z via the existing approach servo, without starting combat; `clickcorpse` then exercises its real left-click selection.

### 8.7 An item released over a world object (`FUN_100cb081` [GUI], `N3Msg_UseItemOnItem` [GC 0x100267e0] / `UseItemOnCharacter` [GC 0x100268af])
Object under the pointer when a carried bag item is released (`InputConfig_t+0xb0`): ground (`0x9c47`) -> `N3Msg_DropItem`; a character (50000) -> `GenericCmd_t` cmd 0x20; any other object -> cmd 5, both with
`UseItemOnItemActionData_t{flag 0, actor = own, item, target}`. Ours: `Play::interact_mouse` stores the pick, `hud_stats/item_ui.rs` `Action::UseOn`. [LIVE] works (§12.1).

### 8.8 Authored item-use gestures and sounds

`combat/use_actions.rs` exposes `confirmed(Message)`, `requested(Message)` (only after the real local use gates),
`gesture` and `sound_key`, plus `Uses` for the runtime queue. There is no category/name heuristic:
the existing RDB helpers decode the authored maps and the existing CRT `pick_variant` selects variants.
`Interact::observe_use_request` receives actual outgoing frames after the existing use gates,
`advance_use_actions(dt, zone)` advances timers, and `take_use_actions()` returns actor/animation/sound
descriptors for the shared own/remote playback path. `Interact::on_frame` observes confirmations and aborts;
closing the zone clears all pending requests, sounds and timers.
The hotbar item route flushes `HudStats`'s outbox immediately in `Hud::activate_item`, matching the
mouse/event routes; it no longer waits for an unrelated inventory mouse event to send the use.
`hotbar_item_activation_flushes_without_a_mouse_event` covers the carried slot, one-shot drain and empty-slot gate.
Variant picking uses `Dynels::pick_variant` and the existing shared character CRT stream, also used by
equipment and character notes; `Uses` owns no separate seed or picker. Gamecode `1011bb4a` seeds from
time on playfield load; DisplaySystem effects can reseed the CRT, so exact effect-driven retail reseeding
remains outside this item's selection implementation.


**Fresh Gamecode trace (2026-10-07):** `1007c8b7` validates the own state-0 request with `1007c7fd == 2`,
then calls `1003b066`. `1007c76a` state 1 calls the same gesture routine only when `1007c749(actor)` says
the actor is foreign; the own acknowledgement must not replay its request gesture. State 2 calls
`1003b21d`, which emits own UI refresh signals, not another gesture. No sequence-deduplication branch was
found in these handlers; the helper does not invent one. The decoded action-data actor is retained for
remote replay; an inventory identity belongs to that actor, never implicitly to our bag.

**Important correction to the earlier decompile interpretation:** `10081e74` is a method on the **item**,
with `(actor, command)` as stack arguments. ASM `1003b0b2 MOV ECX,EAX` passes the resolved SimpleItem;
`10081e7f LEA ESI,[ECX+0x78]` selects its authored animation map, then `10081e85` reads the command key.
Missing/zero values and values above 2000 fall back to `0x6d` (wield); the actor's stat `0x1ae == 0xe`
overrides it with `0x9b` (wield-crawl). This is **not** a character action-key fallback. The selected
AbstractAnimID subsequently resolves through the character/NPC clip map or the ordinary player-set name rule.
`1003b066` resolves the used identity to a SimpleItem before starting a gesture; an unresolved identity must
not produce a fallback. `100267e0` (UseItemOnItem) also starts item key 5 after `1004a70d` succeeds.
`100268af` (UseItemOnCharacter) has no explicit sender-side gesture call.

Authored RDB 1000020 evidence from the installed client:

| Category | Actual template/map evidence | Gesture selection |
|---|---|---|
| kit | First-Aid Kit 23314 has no `{0xe,0x13}`; Startup Coil of Health 215423 has key 3 `0x97`; Weak Coil 218351 has `0x96` | item map, else retail `0x6d` |
| lab | Treatment Laboratory 25812 has no map; Standard Emergency Treatment Laboratory 154327 has key 3 `0x29` | absent-map fallback or authored social-puke |
| eat | Hacked Pill 88395, Blister Pack 93708: key 3 `0x12` | authored social-eat |
| upload | Nano Crystal 26522/27280 and Shadow Crystal 220409 have no map | absent-map fallback; upload success remains server-driven |
| terminal | terminal templates have no use animation map | resolved runtime item gets retail fallback; do not guess a keypad clip |
| loot | corpse has no template use map; chests 23163 have sound key 3 | resolved corpse item gets retail fallback; taking inventory entries adds no gesture |

The installed-data regression asserts authored eat/coil/emergency-lab ids and absent kit/lab/crystal maps.
The capture regression replays `zone_use_object_ithaca.rec`, then replays its confirmations with a different
action-data actor while retaining the original frame target. That replay is synthetic remote coverage,
**not** evidence that Ithaca relayed a foreign actor in this capture.

Sounds are runtime callback-specific, not simply animation-command aliases. `10080f1c` plays SimpleItem
key 3 **only for the own actor**; stat `0x1a` zero (but not -1) selects key `0x32` instead.
`1008983a` plays use-on-item key 5 at the actor for both own and foreign actors, except when source equals
target. `1008035a` use-on-character has no sound-map call. Missing lists mean silence. The positional
sound request uses the existing `GameSound`/audio path.
The depleted key `0x32` branch passes zero XYZ to PlayGameSound; its playback descriptor sets
`sound_at_origin` instead of substituting the actor's position.

Authored visual callbacks use the same confirmed lifetime, not an outgoing cosmetic trigger.
`10002175(event)` is a non-null/nonempty event-list check; `100020fd(event)` returns that list.
`10080f1c` takes event 0 after the depleted-charge branch (zero charges returns before it).
`Uses` reads that authored list with the existing item-template spell parser and queues its `0xcf26`
visual spells in `Playback.visuals`, even when no sound map exists. The descriptor contains an owned
`ApplySpells` application for `Dynels::apply_nano_visuals`, which takes it by value.
Template 116628, **Startup Treatment Laboratory**, has no animation/sound map but event 0 contains
`0xcf26`, effect 13600, duration stat `0x31 = 100` and RGBA 255. Its other event-0 functions
(`0xcf0a` health/nano modifiers and `0xcf29` modifier) are not duplicated as local gameplay changes.
`starter_laboratory_confirmed_callback_queues_authored_stars_for_own_actor` uses the installed authored
record to cover confirmed own-actor playback, absent mapped sound and effect selection.

The cmd-3 foreign-actor branch at `10080fbd -> 10081021` skips only the own sound:
event 0's nonempty check (`10081047..10081051`) and callback (`100810a5`) remain outside it.
The remote laboratory regression resolves a world item's real template and retains the action-data
actor despite a frame addressed to the own character. Zero charges returns before that callback.

For cmd 5 (`1008983a`, source != target) and cmd `0x20` (`1008035a`), the authored callback is event 4.
The callback's `Beholder` is the source item, but this does **not** make every spell item-targeted:
SimpleItem's primary table `10163b4c` has `+0x20 = 10003d7b` (sets context identity at `+0x3c/+0x40`)
and `+0x30 = 100026d0`. The callbacks set that context to the actor before applying the list.
`Target=2` uses `10001ec4` to resolve that context and therefore acts on the actor; `Target=3` uses the
supplied Beholder directly (the actor for cmd 3, the source item for cmd 5/`0x20`). The ordinary
self/default branch uses the source item. `Uses` preserves those distinct receivers in separate
applications; it does not reinterpret an inventory slot as a visible actor. Owner/fighting/pet
selectors `0xe`/`0x17` require runtime relations not exposed by this module and are not guessed.

Read-only authored evidence after the CF34 format correction: Corrupted Crystal variants
275468–275471 have event-4 `0xcf26`, `Target=3`, effect 73001, duration 100; Empty Power Core Slot
287981 has event-4 `Target=1`, effect 72336; Alien Distress Beacon 288073 has event-4 `Target=1`,
effect 71214. `crystal_use_on_item_keeps_authored_source_item_receiver` covers an actual authored
crystal callback and source-equals-target rejection. Event 5 is not substituted for event 4.
The candidate census fully walked 585 CF26-bearing records; 87 further records had unsupported
spell framing and 61 had unsupported item elements, so this is not a claim of a full-RDB census.

`Dynels` preserves the complete receiver identity through the visual queue, handle ownership,
undo, connector refresh and dynel deletion. The old character-only queue guard discarded
source-item applications before rendering. Located item effects now use the built prop's
world frame for attractor 0: GC `10105917` returns success without a connector lookup for
that attractor. It does not substitute the actor, a character bone, or the item origin for
static-mesh connector 3001. Installed tweak evidence is effect 71214 class 3020, flags 515,
attractor 0; Stars 13600 is class 2004, flags 5, attractor 1000. Both use their existing real
renderer classes; `authored_beacon_and_laboratory_spawn_on_their_real_receivers` exercises
spawn and undo for these receivers. The separate queue regression guards kind collisions.
GC `100a5083` tries unlocated creation, then located creation on the receiver, then a
hit-location overload only when the returned handle is zero. Beacon's located constructor
`10112f9c` marks an unsuccessful `InitDynelTemplate` as terminating (`this+0x14 = 1`)
but returns the allocated control, so `100d1b1c` still obtains a nonzero handle:
an inventory SimpleItem without an `n3VisualDynel` does not fall back to the actor.
The no-anchor path intentionally emits no visible effect instead of inventing that fallback.
The located chain is `100d1b1c → 100d0102 → 10112f9c → 100d2a03 → 1010668c`:
the last function re-resolves the receiver identity and requires an `n3VisualDynel`
before locating even attractor 0. Crystal effect 73001's distinct class 3031 constructor
`101143bb` likewise returns the allocated control after failed initialization, so its
missing inventory visual does not reach an owner fallback either.
General 3001 mesh-connector/hit-location allocation is outside this located-class dispatch;
no guessed connector origin is introduced.

Animation and sound maps are decoded independently from the raw record using the existing multimap
helpers. An unrelated event parse failure cannot replace a sound map with invented silence.
If neither a valid template's constructor defaults nor explicit runtime stats establish attack/
recharge timing, no callback is queued with a fabricated zero delay; the authored gesture remains
independent of that unavailable timing.
`parse_item_template` is already a strict leading-stat reader, not a full event parser: it stops at
the first non-name element after the stat prefix. Therefore unsupported later event/skill elements
do not discard known delay/charge stats. Its sound vector comes from the raw multimap helper;
`Uses` moves that vector when the prefix is valid and reads the raw map independently otherwise.

**Timing:** `1003b947` executes cmd 3 immediately when item stats `0xd2 + 0x126 < 31`
or the identity kind is `0xc749`; otherwise it registers the item action timer (`1003d7ce`, `1003d6b1`).
`1003d305` subtracts `dt * _DAT10158670`, whose double words are `00000000 40590000` = 100.0;
the attack stage starts at stat `0x126` and recharge is stat `0xd2`. `Uses` delays mapped callback sounds
by that attack stage, using the same immediate-use exception, and cancels pending sounds on state 2.
`100877a6` invokes the runtime callback only after slot `+0x98` returns 2; it also remembers the item identity
at actor controller `+0x190/+0x194`. Fresh `1003b3a2` trace rejects a dead actor, stat `0x296 != 0`
(vehicle), current controller state 5 (program activation), missing source/target runtime dynels and an
unparented cmd-3 object that fails `10059ca0`'s range gate; inventory kinds 101..249 delegate to `10049850`.
The runtime queue rechecks known source availability, death and vehicle state at callback completion.
**[UNRESOLVED]** actor controller state 5, the exact `10059ca0` parent/range rule and runtime item slot `+0x98`
skill criteria are not exposed by the existing item/zone helpers; server confirmation is required but does not
replace those local post-timer gates. A local item change can still change retail's callback decision.
Remote inventory items require the server-provided actor-relative item/template; absent data must not borrow
our similarly numbered inventory slot. No foreign inventory template is present in the object-use capture.
`10049850` excludes bank/corpse inventories and requires a real carried item; cmd 3 invokes that item's
virtual `+0x50`, cmd 5 invokes `+0x54` (with a weapon-target special case), and cmd `0x20` resolves the
character target then calls `1008177c`. Existing item-requirement helpers format criteria but do not expose
these runtime skill checks as a boolean evaluator.


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
4. Taking an item: a double click on a cell (`FUN_100ca1e7` [GUI]; not on the own inventory view `+0x14c`, which uses `N3Msg_UseItem`; with Shift / Ctrl and a trade view `N3Msg_TradeAddItem`) calls
   `N3InterfaceModule_t::MoveItemToInventory(item)` [Interfaces 0x10008197] = `n3EngineClientAnarchy_t::N3Msg_MoveItemToInventory(item, InventoryId 1, 0x6f)` [GC 0x10027a30] = `ClientMoveItemToInventoryIIR_t
   (item, ANY_BAG_SLOT)` (`FUN_1001534a`: `+0x20/+0x24` = the item identity, `+0x28` = 0x6f). **`item` is the identity stored in the list row (`FUN_1003d830(1, id)` in `FUN_100cdb3a`), i.e. the id of
   the `InventoryEntry_t` list `N3Msg_GetContainerInventoryList` [GC 0x1001753e] returns for a `Chest_t` (vtable `+0x104` = `FUN_1007de99`): `{0x6b, chest[+0x1dc] * 0x10000 + slot}`**, where
   `chest[+0x1dc]` is the **`word` of the container's last `InventoryUpdateIIR_t`** (`FUN_100a040e`) and `slot` the index in the container's inventory (the entry's `slot` field). `FUN_100a040e` also
   registers `word -> container identity` (`FUN_1004af97`, lookup `FUN_10048644`); every consumer decodes the item back as `key = (short)(instance >> 16)`, `slot = instance & 0xffff` (`FUN_10048829`,
   `FUN_1004a7b3`, `FUN_10048678`, `FUN_100490cd`, `FUN_10049e8f`, the source check `FUN_1004b80a` with kind `0x6b`). Kind `0x6a` is the **own overflow/reclaim container** (`0xdeae`, `FUN_10046b0b`),
   NOT a corpse item, and the entry's own `id` (`{0x09000001, 0x44b2c5}`) is a different thing (the item's dynel identity) the client never sends. **Own kill (capture `zone_loot_own_kill_ithaca.rec`):
   update word 0x70, container `{0xC76A, 0xe22}` -> items `{0x6b, 0x00700000}` and `{0x6b, 0x00700001}`; wire `5469373f 0000c350 <own> 00 0000006b 00700001 0000006f`** (test
   `inventory::tests::corpse_item_identity_is_kind_6b_with_the_update_word_as_key`, `interact_captures::own_kill_take_identity_is_kind_6b_word_and_slot`). The earlier sends `{0x6a, cell}`, `{0x6a, 1}`,
   `{0xC76A, 0}` and the entry id were all wrong, hence ignored. `N3Msg_IsItemPossibleToUnWear` [GC 0x10026763] passes kind 0x6b, `FUN_1004b80a` is the client-side refusal table (own bag full:
   `Feedback_InventoryFull` before sending; `Feedback_NotAllowedToLoot`, `Feedback_YouCantLootNoDropItems` [not evaluated]).
   **Live-proven** (`zone_loot_take_ithaca.rec`): the server answers the take with `ContainerAddItemIIR_t` `{item {0x6b, 0x00700000}, container = own, slot 0x6f}`; the client applies it as `FUN_1004a7b3`
   (container cell emptied via `FUN_1002a64f`, item into the first free bag slot `FUN_1002a1b0(0x40)`): `Zone::containers` keeps each corpse list from its `InventoryUpdate`, `Zone::apply_inventory`
   moves the entry into the bag and `LootUi::taken` empties the window cell (tests `zone_inv::tests::captured_corpse_take_moves_the_item_into_the_bag`, `interact_use::tests::captured_take_empties_the_loot_window`). [UNRESOLVED] the stack merge of stackable items (`FUN_1002a5ff` / `FUN_1002a2bc(0x16)`).

**Ours:** `LootUi` opens a tabbed window with a 3 x 7 grid canvas (cells and spacing of the inventory grid, 54 px slot art, 48 px item pictures from rdb 1010008 through `hud_stats::items::Items`,
tooltip = item name) when an `Update` for a non-character container with flag != 0 arrives, refreshes it on later `Update`s (flag 0), closes it with its close button / Esc; double click on an item sends
the move. The title is the corpse's name from `CorpseFullUpdateIIR_t`'s name blob ("Remains of ..."), centred on the screen.

**[UNRESOLVED]:** (a) the order / flag of the live server's answer is now captured (one `InventoryUpdateIIR_t`, flag 1, word 0x70); (b) the window title
(`String::Format` of `FUN_100cc2ca` was not read; the saved window position `container_position` / `container_id` and `TempContainer_%u.xml` are not stored); (c) the original's
list / grid mode switch (`InventoryViewMode`) and the scrollbar of the container view; (d) chest (0xC749) items use the same `Chest_t` path (`FUN_1007de99`, kind 0x6b) but were never captured; (e) team loot (`Feedback_TeamLoot*`, "Random Looter" / "Looter" columns of
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

`N3Msg_DefaultActionOnDynel` [GC 0x100291da] dispatches to player trade for a living non-dialogue character without the NPC use flag, subject to the **target's** idle fight controller (§3). The fight state is `Zone::fight_target` (relayed `AttackIIR_t` / `StopFightIIR_t`).
`N3Msg_TradeStart` also checks TowerType and range (§10.1). Per-character skills are retained by `Zone::character_stats` / `stat_of` from `SimpleCharFullUpdateIIR_t` and `StatIIR_t`; full updates supply NPC `TowerType`. The bounding sphere radius of characters is still unresolved: the existing range check uses [GUESS] 0.5 m each, permitting centres 6.0 m apart. `Feedback_TargetOutsideRangeForTrade` goes to the chat.

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
* Harness API (`#[cfg(test)]`): `Interact::trade_dump(gui)`, `press_button(gui, i)` (0 description, 1 info, 2 trade, 3 use; false when the button is disabled), `trade_add(gui, slot, item_info)` (resolved inventory name/icon, routed through the production drop handler), `trade_accept(gui, zone,
  credits)`, `trade_decline(gui, zone)`; non-test: `trade_drop`, `trade_wants`, `take_cash_delta`, `take_info_urls`. Tests: `interact_trade::tests` (flow with synthetic Knubot frames, wire frames, drops, double click,
  Cash clamp, windows closed with the dialogue), `interact_chat::tests` (bar art ids, enable flags, clicks, splitter drag through real pointer events and the clamp), `n3::knubot::tests::trade_wire_layout`.
  `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac npc_dialogue_render` writes `npc_answers.png` (bullets, green answers, 10 px hanging indent, bar below, splitter line) and `npc_trade.png`.

## 12. Live results (PRK "Ithaca", Aomacvolk, 2026-10-06) [LIVE]

Harness steps (`flow/live.rs`): `props=<r>`, `use=` / `ruse=<kind>:<inst>` (default action / right click), `useon=<slot hex>:<kind>:<inst>`, `talk`, `answer`, `btn=<i>`, `tadd=slot:<hex>`, `taccept`, `loottake=<cell>`, `lootid=`, `grid`, `inv`.
`tadd` resolves the selected inventory entry and its retail template name/icon through the same HUD item cache as dragging; it refuses unknown entries/templates rather than constructing an empty display item. The previous helper sent the correct `{0x68, slot}` wire identity but inserted an empty name/icon, producing visually empty slots in the Brandon live trade despite inventory template 248323 being Spinal Section. The container regression now checks that retail name/icon reach the draw list alongside the original KnuBot add payload.
Captures (login traffic excluded): `docs/captures/zone_npc_dialogue_ithaca.rec`, `zone_npc_trade_ithaca.rec`, `zone_use_object_ithaca.rec`; replay tests `crates/ao-net/tests/interact_captures.rs`.
Movement harness `approach` now ends and releases its held key on the same death/playfield-change conditions as `goto`, or when the own dynel/destination is removed after a frame. The ICC detour from `(802.37,20.29,720.64)` toward `(800,800)` exposed the former unconditional own-dynel unwrap; the available run log did not identify whether removal was death or transition. The offline regression covers own/destination removal without inventing a replacement position or destination.

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
* Corpse (0xC76A, "Remains of Uncle Pumpkin-Head"): echo + `InventoryUpdateIIR_t` (flag set) -> loot window (screenshot checked). The first double click sent `MoveItemToInventory({0x6a, cell}, any)`; the server ignored it, as it
  did `{0x6a,1}`, `{0xC76A,0}` and the entry id, also on our own kill (`zone_loot_own_kill_ithaca.rec`). The identity the original client builds is `{0x6b, update word << 16 | slot}` (section 9, step 4) [UNRESOLVED-live until re-run].
* Borealis exit (movement.md §4.2): the door prop 0xC748 at (684, 74, 534) is a zone line to pf 790 "Stret West Bank" (walking into it), its gate door at (1273, 1, 2887) leads back; no Grid window opened.
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
`TradeIIR_t` op 0 with a **non-zero `b`** (the player trade has `b == 0`, §10.1). Header = own character: `FUN_100663e4` computes the type: GC `1009959d` reads ShopType stat 0x9c, missing/zero defaults to 0x3d Cash; `100995e0` is true exactly for Cash, selecting type 1 `ShopTrade`, otherwise type 2 `ShopBuy`. It emits `GlobalSignals +0xd4 (type, own, a, b)` ->
`InventoryGUIModule_c::SlotStartTrade` [GUI 0x100c6ff4]: an existing trade view is deleted, a new `TradeView_c(type, own, partner = machine, b)` [GUI 0x100e092f] is built (`+0x1a4` type, `+0x1c8` partner, `+0x1d0` own, `+0x1d8` b). The copy with header = the machine (`FUN_1009a23c` case 0) creates the machine's per-player session record (`{trade list, a, b, min(own stat 0xa1, 3000)}`).
An open trade refuses with `Feedback_YouAreAlreadyInATrade` (shop or player trade).

### 13.3 The window (`TradeView_c` type 2, view `ShopBuy` of `Views/TradeGUI.xml`) [CODE]
* Title "Trade", client 192 x 453 like the player trade. GUI `100e092f` forces the transient dockable view into `RollupArea` with an empty persistent identity; the port registers it with the existing rollup controller. Types 1 and 2 share config DValue `ShopViewConfig`; `esc_shops` (`LoginPrefs.xml`: true) = Esc declines.
* View XML `ShopBuy`: `PartnerName` (the machine's name: `N3Msg_GetName`, or its parent's), "Shop" + `ShopInventoryDock`, "Bought Items" + `PartnerInventoryDock`, "Credits" + `PartnerCashView` (read-only, "0", green 0x44dd44, `FUN_100e039a`), `AcceptButton`, `DeclineButton`. There is **no** own credits field and no status light in this view (those are `PlayerTrade` / `ShopTrade`).
* `ShopInventoryDock` (`FUN_100dfeca`): `ItemContainerView_c` `FUN_100cdb3a(msg, mode 1, machine, flags 0xf, 0x15, 6, 0)`: mode 1 = `N3Msg_GetContainerInventoryList(machine)` = the stock; preferred cell counts (1,1)..(1000,1000), horizontal scrollbar off, vertical auto. `PartnerInventoryDock` (`FUN_100e0053`): mode 2 = `N3Msg_TradeGetInventory(machine)` = `FUN_1009991a` (the session's list, the bought items), flags 0xf in shop types (0xb in player trade), preferred counts (1,1)..(1000,1). This caps **preferred height**, not item row count: `SetViewCellCounts` `10132d68` writes preferred-size bounds (and clamps 1000 to 50); `GetClientPreferredSize` `10133255` consumes them. Auto-arranged `GridPosToViewPos` `10132fdb` wraps item index `x` to `(x % viewport_columns, y + x / viewport_columns)`, so bought items wrap and scroll vertically instead of overflowing a horizontal tray.
* Columns by flags (`FUN_100cdb3a`): Icon (id 0, 16 px), Name (1, 200), Count (2, 30; flag 1), Price (3, 100; flag 4), Quality (4, 100; flag 8); list sorted by Name. Row cells (`FUN_100404a9`, `FUN_100cd4b6`): count = stat 0x19c (0 -> 1), **price = `N3Msg_GetShopItemStat(item, 0x4a)`** (`n3EngineClientAnarchy_t` 0x10017a38: the item's `VendingMachine_t` virtual +0x108; shown empty when 0 or 0x499602d2), quality = level; the tooltip of the price (`FUN_10031562`) is `FormatNumeric(price)` in the text LDB 0x1fa.
* Credits (`FUN_100dfbf3`, GlobalSignals `+0xd8`, emitted after every item add / remove): `PartnerCashView` = `FormatNumeric` of the cost of the list (`FUN_10099d56`: sum of price x count over the bought items); the machine's `TradeIIR_t` op 7 sets it directly (`FUN_100672c7` -> `+0xdc` -> `FUN_100dfe5e`).
* Buttons: Accept (`FUN_100dfd33`) disables itself and sends **`N3Msg_TradeAccept`** (`TradeIIR_t` op 1; the player trade sends `TradeConfirm`); Decline (`FUN_100df810`) = `N3Msg_TradeAbort(true)` (op 2, `a = {0, 1}`); the frame's close button and Esc decline too.
* Items: a double click on a list item is `FUN_100ca1e7`: with Shift / Ctrl (`View::GetQualifiers() & 0xc`) and a trade view open **`N3Msg_TradeAddItem(own, item)`** (`TradeIIR_t` op 5 `a = own, b = {0x6f, index}`; `N3Msg_TradeAddItem` [GC 0x191f6] treats `kind 0x6f` as a shop item and, before sending, refuses with a credits message when `FUN_100662b7` (cost so far + price - own credits) is positive), otherwise **`MoveItemToInventory(item)`** = `ClientMoveItemToInventoryIIR_t({0x6f, index}, any bag slot 0x6f)` (what the loot window sends for a corpse item, §9; `N3Msg_MoveItemToInventory` [GC 0x10027a30] with the source check `FUN_1004b80a`, whose 0x6f branches were not decoded). A double click on a bought item removes it (`FUN_100df67e` -> `N3Msg_TradeRemoveItem(own, item)`, op 6; [INFERENCE] that the handler is wired to this list).
* Server messages the window handles (`FUN_100674ab` cases): op 5 / 8 (`FUN_10066cf7`, `FUN_10067109`: vending add, `FUN_10047268(0x21, item)`) add `b` to the bought list, op 6 / 9 remove it, op 7 from the machine sets the credits, **op 4 (complete)** with header = own closes the window (`SlotTradeCompleted`), **op 2 (abort)** closes it (`Feedback_TradeCancelled` when sent by the machine [`FUN_1009a23c` case 2 -> `FUN_100666a3(1)`] or `a.instance != 0`), a zone change closes it silently (`Interact::close_all`).

### 13.4 Port
`Shop::open` selects `ShopTrade` for Cash and `ShopBuy` for other currencies from the original XML. Type 1 includes the own sold-items dock and calculated, noneditable own cash (`GUI 100e039a` clears input feature flags 0xd in shop mode). Drops onto any of its three lists send `TradeAddItem` (`100df870`); server ops 5/8 and 6/9 add/remove stock identities in bought items and bag identities in sold items. Sold items snapshot inventory templates before ownership changes; double click removes the item, with the shared bag-room rule. Accept sends `TradeAccept`; op 3 uses the existing trade confirmation dialog, RESET restores Accept, abort/complete/zone change close the window and confirmation. Names use nonempty `VendingMachineFullUpdateIIR_t` names, otherwise the effective retail template name.
Shop inputs use the original text-editor flags rather than disabling the control: `100e039a` clears 0xd and sets green `0x44dd44`; `100dfbf3` sets the own balance to red `0xdd4444` when negative, green otherwise. Stock and tray scrollbars follow `100dfeca`/`100e0053`/`100e0211`: horizontal **none**, vertical **auto**, in list and grid modes. Wide list columns clip to the original viewport, while column resizing/reordering and saved visibility remain available.
The shop list/grid wrapper carries the same explicit min/max dock size as its children: stacked views have no layout node and `View::CalculatePreferredSize` returns (-1,-1) (`ao-gui/layout.rs::node_calc`; the same wrapper constraint is documented in gui.md §6.5). Child sizes alone do not reserve its area. A collapsed wrapper gives its active grid an inverted/empty drop rectangle; the cash-shop regression requires positive tray geometry before checking the unchanged `TradeAddItem` bytes.
`Interact::shop.quick` = Shift or Ctrl held (`host.mods`, set per input event). Harness (`cfg(test)`): `shop_dump(&gui)` (name, stock rows `[i] Name | Count | Price | Quality`, bought rows, credits, the Accept state, the trade log), `shop_buy(&mut gui, index) -> bool` (plain double click on stock item `index`: `MoveItemToInventory({0x6f, index})`), `shop_add(&mut gui, index, &zone)` (Shift double click: original affordability-checked `TradeAddItem`), `shop_remove(&mut gui, bought_index)`, `shop_press(&mut gui, accept)`; live steps `shop`, `shopbuy=<i>`, `shopadd=<i>`, `shoprm=<i>`, `shopaccept`, `shopdecline`.

### 13.5 Not ported / unresolved
* Original affordability is enforced **before adding a stock item**, not by disabling Accept. GC `100662b7` computes the machine's net session cost (`10099d56`, bought price × count minus sale proceeds), adds the proposed item's price (machine virtual `+0x108`), then subtracts the own stat selected by the machine's ShopType. `N3Msg_TradeAddItem` `100191f6` refuses positive shortages with LDB category `0x6e`, key `CannotAffordThisItem` (string `10157530`, pushed at `100194ca`), feeding shortage then the currency's `fStatToString` name (`100194e4..100194f9`); receive handler `10066cf7` also rejects the local addition when shortage is positive. The port uses the existing ACG item-info cache and effective currency stat for this guard, retaining CL captured at trade start.
* Buy pricing (`GC 10099954`, x87 instructions): truncate Value × BuyPrice(0x1ab)/100, then Cash discount `min(truncate(min(CL,3000)/10×.25),90)` and add truncate(discount×base/-100). Sell proceeds (`GC 10099d56`, assembly 10099dc0–10099e42): truncate Value × SellPrice(0x1aa)/100, sentinel Value 0x499602d2 → zero, then add truncate(truncate(CL/10×.25)×base/100). CL is snapshotted at trade start (`1009a23c`). GUI `100dfbf3` displays sold proceeds minus bought cost in own cash and does not change Accept's enabled state. GUI `100dfd33` disables Accept and sends direct op 1 with zero identities (`GC 10015bfd`); shop acceptance does not send calculated `TradeSetCash`. `10066598` clears internal accepted byte `+0x18` but emits own GlobalSignals `+0xe0(1)`; this is distinct from RESET op 10's false-state signal, which GUI `100df971` uses to re-enable Accept.
* Shop callbacks carry the complete `AcgItem` through bought and sold pricing. `Items::shop_info` loads both rdb endpoints and uses the shared `interpolate_item_templates` path (below), including same-ID QL normalization; results cache by `(low, high, QL)`. Missing records produce no template; malformed interpolation emits an error and never substitutes the low endpoint. Effective machine stats are required too: GC `1009959d` queries the machine's effective `HasStat(0x9c)` / `GetStat(0x9c,2)` via virtual `+0x48` / `+0x3c`, caching Cash only if absent/zero; `100995e0` identifies Cash exactly. Vending overrides `10003d90` / `10003db4` forward into the effective stat object, not just the streamed full-update pairs. Therefore missing streamed ShopType/BuyPrice/SellPrice must not erase retail template values.
  Original load/overlay order is explicit in GC `100a15f8`: find full-update stat 23 (`StaticInstance`), load rdb `1000020` through `10080169`, then add/set every packet stat. Retail template `258794` ("Veteran Rewards Vendor") has bytes at offsets 72/80/88 `9c000000 44000000`, `aa010000 05000000`, `ab010000 64000000`: VeteranPoints (68), SellPrice 5, BuyPrice 100. The pf800 placed entry maps this template to `(687.89,72.81,518.49)`, the live machine 75 location (identity remapping is inferred from matching position). Template `248371` ("Newcomer's Nano Programs", captured ICC machine 100) omits ShopType, has SellPrice 4 / BuyPrice 105, and therefore uses Cash. The previous veteran ShopTrade/zero-price presentation was a confirmed template-fallback bug, not evidence that VeteranPoints purchases should spend cash.
* NPC vendor carts are real **parented vending dynels without an absolute position**. The Antonio capture `zone_antonio_shop_ithaca.rec` contains full update `51035:15`, parent `50000:1000188`, StaticInstance `248368`, no absolute position, no Mesh/ShopType/price-factor wire pairs, then 33 stock items and the normal own/machine START pair. Retail template `248368` ("Basic Startup Equipment - EP02") supplies Mesh 6546, SellPrice 4 and BuyPrice 105, with Cash as the missing-ShopType default. At `(940,47.02,874.78)` the dialogue stayed open; its first answer explains that the Shopping Cart button opens stock, and `btn=3` actually produced those shop frames. Dropping this child because `position == None` prevented effective template registration and therefore the cash shop from opening. The child is now retained through the existing template/model path; its displayed name resolves through the parent character.
* Completion must come from the server: own op 4 `100668d1` clears trade state and deletes the GUI through `+0xe4(true)`. Vending partner completion virtual `+0x10c` (`10099f15`) reconciles already-populated own inventory trade entries and deletes the machine session (`10099e9f`); it does not construct stock templates in arbitrary free bag slots. The real purchase capture `zone_shop_cash_purchase_ithaca.rec` supplies the missing authoritative item path **before** op 4: `TemplateActionIIR_t` key `35505644`, then `ContainerAddItemIIR_t` key `47537a24`, then COMPLETE and Cash. GC reader `1007a424` / writer `1007a4a7` use ACG `(low,high,QL)`, count, action, two identities (36-byte body); unlike full-update ACG readers the fourth word is a real count. Activation `1007a4fb` → `1004d148` action `0x57` creates an own temporary inventory entry (`1002a8c9`, type `0x21`, first free temporary index from zero). ContainerAdd → `10038f67` / `1004ad44` / `100475ae` transfers that real descriptor/count from `{0x6e,index}` to the first free bag slot `>=0x40`, deleting the temporary source only after successful placement. No stock-based synthetic insertion or speculative `TradeSetCash` is needed.
* List/grid presentation uses original Icon 16 / Name 200 / Count 30 / Price 100 / Quality 100 widths, native list image cells, and the inventory's existing Canvas grid layout (`inv_grid`, 48 px cells, original slot art). All auto-arranged grids wrap to viewport columns (`10132fdb`); bought/sold docks prefer one cell row in height, not one item row. `100dfad8` adds the `ListMode` check item to the trade window menu: for shop types it controls **stock only**, not the bought/sold trays. `10040b7b` creates the check, `100408d1` toggles `SetLayoutMode(0/1)`. `100cda29` is **inventory refresh**, not a mode toggle; the old attribution to `InventoryViewMode` was incorrect. The GUI binary contains the `InventoryViewMode` string at `101aaa2c`, referenced only by a data pointer at `10262a68` in the inspected snapshot; no executable mode-pref path was found there. Gamecode string searches found neither `InventoryViewMode` nor `ShopViewConfig`.
* Persistence: `100407b4` stores column config, list/grid sort column/order and (when requested) `listview_mode`; `1004094e` restores them. `StoreColumnInfo` `10133579` serializes **all** columns as parallel `col_id` / `col_flags` / `col_width` arrays, including hidden columns. `LoadColumnInfo` `10136991` restores saved order, widths and **only flag bit 0 (hidden)**, preserving other constructor flags; unmatched existing columns append in their existing state, rather than being implicitly hidden. `100e092f` restores `shop_listview_config` and `partner_listview_config` from `ShopViewConfig`. Destructor `100df4d9` writes **only `shop_listview_config` for shop types** (partner config is saved only for player-trade type 0). The port follows that distinction and retains other archive children; regressions cover stock mode/sort round-trip, column flags and untouched partner mode. Item info (`itemid://`) remains unresolved; no retail screenshot comparison was performed for this change.

### ACG dummy-template interpolation (tower info and shared template values)
`N3Msg_CreateDummyItemID` GC `10016621` calls `10082a3d`: load both rdb `1000020` records, select the nearest template QL (stat54; equal distance selects high), preserve its metadata, set ACG fields, then call `100cc270` → `100cba71`. Invalid ACG QL (unsigned16 outside1..511) becomes1. `100cb689` selectively interpolates item stats; other identifiers, names and flags remain from the selected template. Scalar `100cb3b2` uses unsigned16 QLs without clamping, adds0.5 to the interpolated distance from the smaller endpoint value, truncates, then adds that smaller value. This differs from rounding the signed final value. Value stat74 additionally applies `100cb611`: normalize the rounded value between min/max endpoint Values, square that ratio, rescale and truncate (intermediate bounds/ratio are float32). Implemented in `ao-formats::dynel_visual`.
Tower spell lists24/25/26 follow `100cba71`: criterion values interpolate with equal count/key checks (`100cb934`); scalar arguments interpolate only for the opcode field tables at `1016b168..1016b1f8` (`100cb4ba`/`100cb44d`). ModifyStat/ModifySkill/ModifyTowerValue functions cf14/cf35/cfaa interpolate argument39; cf76 has no scalar interpolation selector. Different-stat modifier entries are matched by function/stat0, falling back to the selected spell when an endpoint lacks a match. `info_tower_interpolation.rs` supplies these values to the tower HTML formatter. GC pointer `10154580` is **Color_t::Interpolate**, not GameData item interpolation (callsite `100d6874`).

## 14. Mission terminals

QuestBooth templates have runtime class **0xdac1** (static.md). The placed `DynelData` wire identity is preserved, not remapped: `CreateFromTemplate(template, identity)` chooses the runtime class from the template. Only confirmed `GenericCmd_t state 1, cmd 3` with the own actor invokes their use callback: GC 0x1007c76a → 0x1003bac9 → 0x1003b947 → 0x1003b6e0 → SimpleItem action slot +0x9c (0x100877a6), then runtime use slot +0x58 → QuestBooth 0x10086571. `Dynels::item_class_of` resolves this template class separately from the supplied identity; an unknown/unbuilt prop does not open a mission window. It emits GlobalSignals +0x164 through 0x10011875 with terminal identity and origin type: valid stat 0x1ea in 1..8, otherwise 1. A request/state-0 packet or another player's use does not open our window.

GUI `MissionSelectionView_c` 0x100d06af is constructed natively, not from an XML file. Baseline preferred Point(167,112), inclusive 168×113; 3×2 mission icon list; difficulty slider 0..100/default50; six advanced sliders -100..100/default0. Endpoint labels come from QuestSel_* texts, common width = widest label +5. `Slider_c` 0x1014448a uses gfx 0x91 (11×18 knob) and 0x92 background; drag starts on the knob and preserves its grab offset. Expand/collapse gfx 0x47/0x46, buttons RequestMissions / AcceptMission / MsgBox_Cancel. The existing canvas input renderer reproduces these controls.

Generate handler 0x100cff7e checks cash against the own level and requires two free bag slots; accept 0x100d0214 also requires two slots. The request is `QuestAlternativeIIR_t` key 0x5c436609, version byte4, difficulty band `floor(ui_difficulty×0.10891088843345642 +1)`, six signed dimension bytes, seed0, origin type and identity, and zero alternatives (GC 0x1001620e / 0x100c9b9a / 0x100cadfa). The response includes generated Quest bodies and seed (0x100cacdb); selection is `CreateQuestIIR_t` key 0x291f361b with the selected mission identity (0x1001738d / 0x100cab36). All client headers use own identity and passed-on byte0.

`interact_mission.rs` builds the original list, slider and button structure, preserves MissionSelectionViewConfig (`show_advanced_options`, `difficulty`, `dim_good/ctrl/secret/mystery/stealth/reward`) and obeys `esc_missionselection`. Alternative quests are retained separately from accepted quests for the original itemid information page; list/context actions feed the existing InfoView and HUD mission map/compass marker. Checks cover exact request/select/response bytes and truncation, original defaults, UI request/selection/cancel, and own-confirmed-use activation.

The confirmed-use regression loads real retail template `1000020:41568` (class bytes `c1 da 00 00`) through the production prop builder. It supplies a different unchanged placed identity kind `0xc748`, rejects an unbuilt prop, and verifies that only the own confirmed use invokes the ready QuestBooth runtime callback. This is fixture verification, not evidence of a live terminal session.


### 14.1 Quest information pages

The actual quest identity kind is **0xdac3 = 56003**, not 56000. GUI item-information dispatcher 0x100384f3 routes it to 0x1003565d. `quest_info.rs` reproduces title, description, remaining time, team label, and Reward section with cash, XP/SK and itemref links/icons (or NoReward), in the original order; the formatter does not invent a criteria/actions table. The first two reward integers are cash/XP (GC 0x100172f9 / 0x1001732c); a nonzero single reward item replaces the list (0x1001b481).

Shadowknowledge replaces XP only at levels 200..220 on a playfield whose N3ReadBlob flags have bit 1 (file +0x34, version ≥9; 0x1001c115). GC 0x1006263f multiplies XP by the float32 factor `faction×0.0010000000474974513/50000`, truncates and clamps; faction stat is 572 for side1, 569 for side2, 566 otherwise. `Zone::quests` and `mission_alternatives` share the original InfoView page path; `server_now` advances the synchronized GameTime unix clock without float32 epoch precision loss.


### 14.2 Pointer-path live checks

The harness accepts `rightdyn=<instance or exact name>` and `doubledyn=<instance or exact name>` alongside `clickdyn` / `hoverdyn`; these locate a visible pick point outside GUI windows and send actual mouse move/down/up events. `goto` accepts a dynel instance, exact name, or x:z. These interaction checks use the same picking/default-action path as the windowed app, rather than calling the dialogue method directly. `clickdyn=shift+<id>` exercises character Info; `props=<radius>` inventories available doors/terminals/shops before using them.


### 14.3 Live interaction evidence (2026-10-06, Ithaca)

Muted offscreen `live_walk` sessions used the shared live lock and credentials on stdin. `props` now prints the production READY model's runtime `item_class_of`, separately from its unchanged wire kind.

* **NPC give:** Aomacrceg / ICC playfield 4582, Brandon Thorn `50000:1000182`: actual `rightdyn` opened the dialogue; Give (`btn=2`) received `StartTrade` with maximum three items. Offering bag slot `0x40` and accepting received `RejectedItems` for retail template `248323` (Spinal Section), followed by “Sorry. No can do”; inventory retained the item. The first offered frame exposed a harness-only missing name/icon: `tadd` bypassed production `trade_drop`. After routing the harness through resolved inventory/template info, a fresh 52.18-second session (`approach=1000182,talk=1000182,btn=2,tadd=slot:40,tdump,shot=npc_repaired,tdecline`) printed `(104,64,"Spinal Section")`; the inspected frame showed the spine icon in the first GIVE ITEMS slot. Decline received the server rejection and closed the tray. This is a rejection/cancel check, not an accepted quest-item exchange.
* **Mission terminal:** Aomacvolk / Borealis 800, READY QuestBooth `56001:-1073741024` at `(632.6,72.8,545.5)`, runtime class `0xdac1`. In a 40.10-second session, `ruse=56001:-1073741024` opened the Missions selector. Actual mouse clicks `581:447` (Request), `596:347` (first alternative), `581:479` (Accept) generated five populated mission icons, deducted two credits (visible server chat), then closed the selector with server chat “Mission accepted.” The open/generated/accepted offscreen frames were inspected. The server also warned that RK missions are currently in development. No mission completion was exercised.
* **Veteran vending:** Borealis `51035:75` at `(687.9,72.8,518.5)` opened a 36-item Trade shop; inspected frames showed stock icons, bought/sold trays, Credits and Accept/Decline. `shopbuy=0` produced no observed inventory change. `shopadd=0` received own `TradeIIR_t` op 5 with `{111,0}` and displayed the Veteran Nano Recharger Upgrade Kit in the bought tray. Accept disabled itself, but no op 3/4 or new inventory item was observed; Decline closed the shop. This does **not** establish a successful purchase or its currency requirements.
* **Verified cash purchase:** Aomacrceg reached and persisted on the ICC vendor platform at `(940,47.02,874.78)`. Antonio's cart (`ruse=50000:1000188,btn=3`) opened the correctly named Cash shop. An initial Tiny Dirk purchase (stock 6, 262 credits) charged Cash `1005 → 743` and exposed the missing TemplateAction/temporary-container transfer (§13.5); its real messages are replayed by both codec and zone inventory tests. After implementing that authoritative transfer, a fresh muted, locked 47.81-second session (`stats,inv,ruse=50000:1000188,btn=3,shopadd=12,shopaccept,wait=2,stats,inv`) bought **Oak Bo** for 262 credits. Before purchase bag slots `0x40..0x45` held six existing items and Cash was 743. Immediately afterwards, without reconnecting, bag slot **`0x46`** contained real template **121565**, QL 1, count 1, inventory identity `{0x68,0x46}`, and Cash was **481**. The tray showed Oak Bo / 262 with Accept enabled; op 1 completed and closed the shop. No speculative SetCash or completion-time stock insertion was used.
  Focused wire evidence for the final delivery is `docs/captures/zone_shop_oak_bo_ithaca.rec` (zone shop/item/credit frames only, login traffic excluded). The earlier Tiny Dirk tray and post-accept offscreen frames were inspected: the real dagger icon and 262-credit cost were visible before acceptance, and the shop disappeared with 743 credits afterwards. Those frames revealed the missing immediate inventory path before its correction; the final Oak Bo inventory result above comes from the production zone state printed in the same live session.
* **Remaining unrelated live prerequisites:** The nearby Borealis store-door approach stopped 5.7 m away without a transition. Testy was on ICC, offshore at `(1010.4,20.3,663.1)`, not Arrival Hall. No owned pet was demonstrated on the exercised Soldier characters; an Anger Manifestation appeared near another player, but its ownership was not established and it was not used for a pet-command check.
