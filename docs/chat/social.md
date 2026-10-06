# Buddy list, tell windows, private groups, looking-for-team

Code: `crates/ao-net/src/chat.rs` (wire), `crates/aomac/src/play/chat/social.rs` (state + decisions, pure), `social_win.rs` (Friends window,
tell windows, dialogs, LFT window), `social_hub.rs` (routing into the hub). Addresses are GUI.dll; names without symbol are Ghidra `FUN_<addr>`.
Dumps used (not committed): `ppj::Client_c` dispatcher `FUN_1016d6dc`, `ChatGUIModule_c::HandlePPJActions` 0x10087fd4 and its handlers,
`FUN_100a5e11..100a9c58` (friend list), `FUN_100ef8ac..100f03fb` (LFT window).

## 1. How the original is built

`ppj::Client_c` turns every chat-server packet into an `Action_t` (kind at `+4`); `ChatGUIModule_c::HandlePPJActions` [0x10087fd4] routes them:

| kind | class | handler |
|---|---|---|
| 0 | `SystemMessage_t` | `HandleSystemMessage` |
| 1 | `PrivateMessage_t` (tell, 0x1e) | `HandlePrivateMessage` 0x1008792e |
| 2 | `VisinityMessage_t` | `HandleVicinityMessage` 0x10086728 |
| 4 | `PrivateGroupAction_t` (0x32 / 0x33 / 0x37) | `HandlePrivateGroupAction` 0x10085d5d (**only sub-type 0 = invited does anything**) |
| 5 | `GroupMessage_t` (0x41, 0x39 -> kind 0xE, 0x3a ...) | `HandleGroupMessage` 0x100863ff |
| 6 | `GroupAction_t` (0x3c / 0x3d, private group join / part) | `HandleGroupAction` |
| 10 | `LftQueryReply_t` (0x5dd) | `HandleLFTMessage` 0x10087069 |

The buddy list is not an action: the client keeps `vector<Buddy_t>` (0x34 bytes: `+0` id, `+4` name, `+0x20` online flag, `+0x24` data bytes,
element `0xd` words) and, when it changed (`Client_c+0x138` flag, poll bit 0x800 in `FUN_100899fc`), calls
`ChatGUIModule_c::UpdateBuddyList(vector<Buddy_t>)` 0x10085f17 which emits GlobalSignals+0xc0 -> slot `FUN_100a5f74` (friend list refresh).

## 2. Wire (verified against the dispatcher)

| S2C | name (client string) | fields | notes |
|---|---|---|---|
| 0x28 | S2C_ADD_BUDDY | `I D` id, data | known id: only `Buddy_t+0x20` (online flag, `I` is the second field of the packet) is updated; unknown id: appended (`FUN_1016ddc3`). Sets the dirty flag. **Live**: `id u32, online u32, data = u16 len + bytes`; the server's answer to a permanent add (`D = {1}`) carries an EMPTY data block (`0028 000a 00006584 00000000 0000`), online = 0 / 1 as the character's real state. |
| 0x29 | S2C_REM_BUDDY | `I` | removes the entry. **Live**: echoed as `0029 0004 <id>` right after our `0x29 I` (also for an id that never had an add reply) |
| 0x32 | PRIVGRP_INVITED | `I` | `PrivateGroupAction{0, id, name(id), id, name}` |
| 0x33 | PRIVGRP_KICKED | `I` | PGA(1) + `GroupAction(part, 0xE:id)` + `GroupMessage` text `LeftPrivateGroup` (+ name) |
| 0x37 | PRIVGRP_JOINED | `I I` group, who | who == own id: `GroupAction(join, 0xE:group, name(group), flag 1)` + text `YouJoinedPrivateChat` + name; else text name + `JoinedGroup` |
| 0x38 | PRIVGRP_PARTED | `I I` | text name + `LeftGroup` |
| 0x39 | PRIVGRP_MESSAGE | `I I S D` group, from, text, data | `GroupMessage_t` with group `0xE:id`, kind field `0xe` |
| 0x3a | PRIVGRP_DECLINED | `I I` | text name + `DeclinedInvite` |
| **0x5dd** | **S2C_LFT_QUERY_RESULT** | `B I S I I B B S` | status, id, name, level, playfield, side, profession, description (below) |
| 0x44c | S2C_ADM_MUX_INFO (**not** LFT: earlier docs mixed this up) | three lists (format string 0x101ca0d8) | admin multiplexer info, ignored by this client's UI; not decoded |

Texts (text.mdb category 10001, verified against the real file by `social::tests::texts_match_the_real_db`): `YouJoinedPrivateChat` = "You joined private
group: ", `LeftPrivateGroup` = "You left private group: ", `JoinedGroup` = " joined the group.", `LeftGroup` = " left the group.", `DeclinedInvite` = " declined the invitation."
(a name precedes the last three, follows the first two). Group name = the owner's character name (group id = owner id).

C2S: `0x28 I D` (D = 1 menu "Befriend", 0 temporary entry created when a tell window opens, `FUN_100a5e7a` -> `FUN_1016c91a(id, {0}, 1)`), `0x29 I`, `0x32 I` invite,
`0x33 I` kick, `0x34 I` join (`FUN_100a6e0c`, Yes), `0x35 I` part (No / auto decline / ignored inviter), `0x39 ISD` private group text, `0x5dc S` LFT on,
`0x5dd` LFT off, **`0x5de IIII` LFT query** (`FUN_1016cb36`; below). `ChatCmd::LftQuery`, `ChatEvent::LftReply` are encoded / decoded by `ao_net::chat` with byte-layout tests.

## 3. Friends window (`FriendListView_c`, ctor `FUN_100a9c58`, `friends_window`)

* Window "Friends", `Rect(0,0,169,215)` (`_DAT_101b9e70` = 169.0, `_DAT_101b9e74` = 215.0), a `ListViewBase_c` (multi select) in a `ScrollView` (vertical layout, `RollupArea` message
  config). Four folder items (`StringListViewItem_c(variant, text, 0xd5, 0xd4)`, `SetIsFolder`, not selectable): **Chat Windows** (`ChatWindows`), **Online Friends**
  (`OnlineFriends`), **Offline Friends** (`OfflineFriends`), **Recent Messages** (`RecentMessages`) in that order; texts are category 10001 keys. Open flags persist as
  `FriendsWindowConfig` (`is_chat_window_list_open`, `is_online_list_open`, `is_offline_list_open`, `is_recent_list_open`, default open).
* `FriendListNode_c` (`FUN_100a6aa7`, 0xb0 bytes; map `FUN_10085b01()+0x14`): `+0xc` name, `+0x28` id, `+0x2c` state, `+0x30` raw online, `+0x34` unread count, `+0x39` invite pending.
  `FUN_100a5f74` (UpdateBuddyList slot): state = **2** when the buddy's data is exactly one 0 byte (temporary), else `online != 1` (0 online, 1 offline); nodes missing from
  the list are dropped unless state 3 (ignored). Folder by state: 0 Online, 1 / 3 Offline, else Recent (`FUN_100a8141`). Item icon gfx `0xd8 - (raw_online != 0)` (0xd7 online,
  0xd8 offline); entries sorted by name (`String::CompareNoCase`); `FlashIcon(unread > 0 || invite)`; `SetLabelColor(0xff6666, 0xc04c4c)` for state 3 and unread > 0.
* Context menu (`FUN_100a930e`, `PopupMenu_c`, message `user_id` / `user_name`): "Befriend X" (`BefriendX`, only state 2), "Invite X" (`InviteX`, greyed for state 3),
  "Delete X" (`DeleteX` -> `FUN_100a8d40`: `DialogBox_c` "Warning" + `ReallyWantToRemoveXfromFriends`, Yes sends 0x29), "Ignore X" / "Unignore X" (`ReallyWant2IgnoreX`
  dialog first), "Mail X" (`MailX`, only while `MailWindow` exists: **not ported**, the MailWindow is not). Actions: Befriend -> `FUN_100a6940` (0x28 D=1).
  Texts are category 10001: `Befriend %s`, `Invite %s`, `Delete %s`, `Ignore %s`, `Unignore %s`.
* Opening: `SlotFriendWindowActivated` 0x100871a0 creates the window (class size 0x1b8) when the HUD's `friends_window` dvalue turns true and closes it when false
  (`WindowKind::Friends`, Ctrl+R); the HUD button pops out when the user closes the window (`Chat::take_closed_windows`).
* Fresh constructor trace (2026-10-06): `FUN_100a9c58` calls the `DockableView` base `FUN_10038b47(Rect, "friends_window", "Friends", 5, 0)` before installing the `FriendListView_c` vtable; `SlotFriendWindowActivated` allocates the view and passes it to `FUN_1000c930`, not to a `Window` constructor. The port exposes this view's owner WindowId through `Chat::dock_windows` so the HUD controller registers `friends_window`; LFT is not included.


## 4. Tell windows (`OpenTellWindow` 0x10085df8 = `FUN_10085b01` + `FUN_100a6568` = `FUN_100a5e7a` + `FUN_100a75e8(1,1)`)

* `FUN_100a5e7a(name, id)`: find-or-create the `FriendListNode_c` (state 2); a **new** node sends `0x28 {id, D = {0}}` (temporary buddy, so the character's online state is followed)
  when the chat connection exists. `HandlePrivateMessage` does the same for every incoming tell (`FUN_100a6210`): sender node, then `FUN_100a6cd5` appends the line
  `[<a href=user://NAME>NAME</a>]: text` (GM senders: `[<font color="#FF0000"><a ...>NAME</a></font>]: `) to the tell window; the node's unread count rises while the window is
  closed (red label + flashing icon). Tell windows persist per user id (`<id>.xml` config, `<id>.log`, directory "Friends"; `FUN_100a658b` reloads them at startup): **not ported**.
* A click on a `user://NAME` link (chat windows, tell windows) opens the tell window (`ChatView` link signal -> `OpenTellWindow`); `/tell NAME` without text does the same.
* **GUESS**: `TellWindow_c` (`WeakPointer<TellWindow_c>` at node `+0x3c`) was not decompiled: the port uses a style-0 window titled with the name, the chat view's
  text area + input bar (docs/chat/gui.md section 3), Enter sends the tell and echoes `ChatTellMsgToField` ("To [%s]: ") in the window. The branch structure of `FUN_100a6210`
  (which of tell window / "Tell Messages" group gets the text, preferences `FUN_10084f33(1|2)`) is not resolved: the port keeps delivering tells to the Tell Messages group
  window (existing behaviour) *and* the tell window logic.

## 5. Private groups

* Invitation (`FUN_100a72c7(true)`, node `+0x39`): inviter ignored -> `0x35` silently. Pref `ChatPGInviteAction` (OptionPanel radio group; CharPrefs.xml default **1**):
  **0** always decline: line `ChatInviteAutoDeclined` ("Chat group invitation from %s was auto declined.") + `0x35`; **1** "DisplayInviteDialog": dialog (`FUN_100a700a`: title `PrivateGroupInvitation`
  "Private Group Invitation", body `ChatFriendList_InvitedToPrivateGroupDialogText`, buttons `MsgBox_Yes` / `MsgBox_No`) + flashing node + line `ChatInvitePending` ("You were invited
  to a private chat group by %s."); **2** "FlashUserIn..." only the flashing node and the line. The pref store (option panel) is not ported: the default 1 is used.
* Dialog answer (`FUN_100a6e0c`): Yes = `0x34 I` + the group (kind 0xE, id) is created and assigned to the chat windows selected in the dialog (the dialog lists the chat windows as
  check boxes; `FUN_1009d129` / `FUN_1009ce7b` per selected window), No = `0x35 I`. Port: Yes/No send the packets; the group appears with its `PRIVGRP_JOINED` and the
  windows' own subscription rule (autosubscribe windows) applies. **GAP**: the per-window assignment check boxes (needs `ChatWindows::subscribe_group`, ChatWinFrame).
* Sending: the group is an ordinary output group (`GetGroupIdentifier(owner, 0xe)`); `FUN_1016c9f4` sends 0x39 for kind 0xE (docs/chat/net.md).
* `/invite`, `/kick`, `/leave` (docs/chat/cmd.md) lookup + 0x32 / 0x33 / 0x35 as before.

## 6. Looking for team (`LFTWindow_c`, `FUN_100f03fb`, `lft_window`, RightMenu entry `#FindTeam`)

* Window: style 0, `Rect(200, 180, 849, 700)` (`_DAT_101a959c`/`101a95a0`/`101c10b0`/`101a95a8`), flags 0x1000, help file "The LFT Window.html", view `Views/LFTView.xml`, tab title
  `GetText(100, "Team Search")`, `MoveToCenter` + `LoadWndConfig("LFTWindowConfig")` (SelectedSide / SelectedLocation / SelectedProfession ids, TeamDesc, six column widths:
  Name 120, Level 30, Side 50, Profession 60, Location 180, Description 180 = `_DAT_101ae4d8`, `101ae5c8`, `101b20a8`, `101aec64`, `101a95a0` x2); closes on Esc when `esc_lft`.
* `CandidateView` holds a `MultiListView_c` (6 columns `Name`, `Level` (2003:0x36), `Side` (2003:0x21), `Profession` (2003:0x3c), `Location`, `Description`; both scroll bars auto).
* Dropdowns (`DropdownMenu_c`, items inserted at their id as index): Side `neutral(0) clan(1) omni(2)` (category 2005) + `any` (7, default); Location `this playfield(0)`,
  `anywhere(1)`, `Rubi-Ka(2, default)`, `Shadowlands(3)` (literals); Profession ids 1..15 without 13 (category 2004) + `any` (0x10, text `GetText(100,"any")`, default).
* **Search** (`FUN_100ef912`): ignored while busy (`+0x7c`) or while the 3 s `EventTimer` (`Start(3000000)`) runs; clears the list; with a chat client: busy = true, timer, `0x5de IIII`
  = (side item id, 7 -> -1; `1 << profession id`, 0x10 -> -1; Location *selected index*; -1). Busy ends with the status-2 reply.
* Reply (`HandleLFTMessage`): status 0 -> candidate signal (GlobalSignals+0x1f0): id, name, level, profession (`byte +0x35`), playfield, side (`byte +0x34`), description; status 2 -> the same
  signal with id 0 and empty fields; other statuses ignored. The slot `FUN_100efe4b` clears busy (`+0x7c`) when **id == 0** and adds a row otherwise (so status 0 / id 0 ends the search too;
  live, section 9: an empty search is answered by exactly one status-0 packet, every field zero; fixed in `Lft::on_reply`, before the fix the port showed an empty row and stayed busy). Handler `FUN_100efe4b` builds `LFTCandidateItem_c` (`FUN_100f1456`): name, level, `GetText(2005, side)`, `GetText(2004, profession)`,
  `N3Msg_GetPFName(playfield)` ("Not found"), description.
* **Invite** (`FUN_100eff95`): `N3Msg_TeamJoinRequest(Identity(50000, id), false)` then the line `JoinTeamRequestSentTo` + name (category 100) as System text; the button is disabled
  when in a team and not its leader (`FUN_100efb0a`). **Tell** (`FUN_100efa55`): `OpenTellWindow(name, id)`.
* **LFT checkbox / team description** (`FUN_100f0313` -> `FUN_100f01a3(on, desc)`): off = request kind 0xb (`0x5dd`), on = kind 10 (`0x5dc S`) and `LFTWindowConfig.TeamDesc` updated; `DAT_10276620` = own flag
  (`/lft` toggles the same flag, docs/chat/cmd.md). `FUN_1007850a` (a Gamecode state reset) also calls `FUN_100f01a3(0, "")`: when exactly is unresolved, not ported.
* Ported: the window from the client's own `LFTView.xml` (labels, layout, buttons, check box, input) with the **real widgets** (docs/gui.md §14): the three `DropdownMenu` (items
  inserted at their id as index, `SelectByID(saved id, true)`, Search reads the selected item ids and the selected *index* of Location) and the `MultiListView` (list mode, flags
  0x40, 6 columns `AddColumn(i, label, width, 0xe)`, rows `AddItem(.., sorted = true)` ordered by the Name column, compare per column as `FUN_100ef4a1`, header click re-sorts,
  header drag resizes, selected row = `ViewSurface` 0x88aadd; the row is selected by the application slot `FUN_100efacd` = `Select(true, true)` on the mouse-down signal). Column
  widths and the three selected ids persist in `<character prefs>/LFTWindowConfig.xml` (same `Message` archive schema as the chat window configs).
* Geometry/pin persistence: `FUN_100f03fb` constructs style 0 with flags `0x1000`, then calls `MoveToCenter` and `Window::LoadWndConfig`; destructor `FUN_100efb97` calls `Window::SaveWndConfig` before writing `LFTWindowConfig`. The port now enables frame movement/resizing and restores/saves inclusive `WindowFrame` and `WindowPinButtonState`, reusing `hud_wincfg::Cfg`/`inside_screen`. Friends' fallback owner frame also preserves geometry; the registered dock controller owns its dock/rollup placement. Character directories are supplied once through `SocialWin::set_character_dir`; legacy shared Friends/LFT files migrate once without overwriting a character's own archive. Regression `social_frames_and_pin_persist_per_character` covers geometry/pin reload and character isolation. Viewport changes retain right/bottom-edge attachment and move windows inside the new screen without changing size.


## 7. Not faithful / unresolved (each labelled in code)

* The list widgets are real now (docs/gui.md §14): the label colours (selected / not selectable `0xffffff`, selectable `0xc0c0c0`, red `0xff6666` / `0xc04c4c`), the folder icons
  (`0xd5` open, `0xd4` closed), the 15 px indent and the 0.5 s icon blink come from `StringListViewItem_c` / `ListViewBaseItem_c`. A click on a friend toggles its tell window
  (`FUN_100a9b4b`: `FUN_100a75e8(1 - is_open)`, 0 closes it); a click on a chat-window item (show / hide that chat window, `FUN_1009adae`, and its menu `FUN_100a83ac`) is not ported.
  UNRESOLVED left in the widgets: the horizontal scroll-bar art, the popup menu skin (shared `PopupMenu_c`), `ResizeColumnToFit` (fits the widest cell), a sort marker in the header.
* Friends window initial position (centred fallback until dock registration), tell window layout, tell routing preferences (section 4), the "Chat Windows" folder content (needs `ChatWindows` names), chat window
  assignment check boxes (section 5), `ChatPGInviteAction` pref store, Mail entry, LFT reset at `FUN_1007850a`, team state for the LFT Invite button (port has no team state yet).
* Kick: the original queues `GroupAction(part)` before the "left private group" text (order delivered to a removed group is unknown); the port prints the text first.

## 8. Verification

`cargo test --release -p ao-net chat::` (0x5de / 0x5dd layouts), `cargo test --release -p aomac chat::social` (buddy folders, temporary entries, tell nodes, invitation prefs,
private group texts, LFT search / reply rules, real text db strings; GUI tests skip without the client).

## 9. Live check (Ithaca, 2026-10-06, offscreen harness, `say=/tell Testy` = empty tell)

`> 0015 0007 "Testy"` lookup, `< 0015 {0x6584, "Testy"}`, then `> 0028 0007 00006584 0001 00` = **S2C-visible C2S buddy add with D = {0}** (the temporary entry, bytes exactly as `ChatCmd::BuddyAdd{permanent:false}`); the server answered
no `0x28` (temporary entries are not echoed; the permanent add of 9.1 is, so the `S2C_ADD_BUDDY` layout is confirmed live). The in-world screenshot shows the tell window "Testy" (tab title, text area, input bar) at its default position.

### 9.1 Confirmation run (2026-10-06, steps `buddyadd` `buddyrm` `lftsearch` `friendswin` `lftwin` `say=/lft ..` `say=/invite ..`, two sessions of ~55 s)

Capture (social frames only, no credentials): `docs/captures/chat_social_ithaca.rec`; replay test `play::chat::social::tests::live_social_capture_replay` (re-encodes every sent frame, decodes every
received one, feeds `Social` / `Lft`). Observed (ms since start; `>` sent, `<` received):

| step | frames | result |
|---|---|---|
| `buddyadd=Testy` (offline), after the temporary add (`D = {0}`) of section 9 | `> 0028 0007 00006584 0001 01`, `< 0028 000a 00006584 00000000 0000` | online 0, data empty -> "Offline Friends (1)", red icon 0xd8. The earlier temporary add (`D={0}`) never got an answer. |
| `buddyadd=Beinrangel` / `Battle` (online players of the name table) | `< 0028 000a 00007dfe 00000001 0000`, `< ... 00007bf4 00000001 0000` | online 1 -> "Online Friends (2)", green icons 0xd7, sorted by name (Battle, Beinrangel) |
| `buddyadd=Aomacvolk` (own id 0x82e8) | `> 0028 0007 000082e8 0001 01` | **no answer** for the 26 s until the session's next step, no list entry; the following remove was still echoed |
| `buddyrm=<id>` | `> 0029 0004 <id>`, `< 0029 0004 <id>` (~110 ms) | entry removed, folder counts back to 0 |
| `say=/lft aomac test` / `say=/lft` | `> 05dc 000c 000a "aomac test"`, `> 05dd 0000` | **no server reply** to either; local lines `Looking for team: ON` / `OFF` (screenshot) |
| `lftsearch=7:16:2` (any, any, Rubi-Ka) | `> 05de 0010 ffffffff ffffffff 00000002 ffffffff` | `< 05dd 0013 00 00000000 0000 00000000 00000000 00 00 0000`: **one** status-0 packet, id 0, empty name / description = no candidates (nobody else was LFT) |
| `lftsearch=1:6:1` (clan, Adventurer, "anywhere") | `> 05de 0010 00000001 00000040 00000001 ffffffff` | the same single empty reply |
| `say=/invite Testy` (offline) / `/kick Testy` / `/leave Testy` | `> 0032 0004 00006584`, `> 0033 0004 ...`, `> 0035 0004 ...` | no reply of any kind (no 0x32..0x3a, no system text) within 3 s each; `/invite Aomacvolk` (own name) is rejected locally (`CantInviteYourself`), nothing is sent |

Screenshots (offscreen harness, inspected): the Friends window (169x215, title tab "Friends", the four folders with counts, the entries with red / green icons) and the Team Search window (labels, three
dropdowns, Search button, column headers, `Invite to team` / `Send tell` buttons, "Looking for team" check box + description field) match the layouts of section 3 / 6. Before the end-marker fix the empty reply
produced one bogus row (`0 neutral ... Not found`) and left the Search button greyed; after it the list stays empty and the button is enabled again.

**Not verifiable with one account** (documented, not guessed): the S2C private group packets (0x32 invited / 0x33 kicked / 0x37 joined / 0x38 parted / 0x39 message / 0x3a declined) need a second
online character; the server sent none for an offline target, and inviting a stranger was not done (rude, and a stranger's accept would not be controllable). Their layouts stay as read from the client's dispatcher
(`FUN_1016d6dc`) with the unit tests of `ao_net::chat`. A non-empty LFT result row (levels / playfield / side / profession bytes) was likewise not seen because no other character had LFT on; the
`BISIIBBS` row decoding stays covered by the client's format string and the `ao_net` byte test. The data block of a permanent buddy is empty on the server's side, so `Buddy_t.data == {0}` as the
temporary marker is only ever produced by the server for the temporary add we send (which it did not answer at all).
