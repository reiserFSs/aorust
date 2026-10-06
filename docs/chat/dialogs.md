# Local dialogs reachable from chat commands (InfoView, DialogBox, camp/quit, window commands)

Code: `crates/aomac/src/play/chat/info.rs` (InfoView), `dialog.rs` (DialogBox / AFK dialog), the glue in `chat.rs` (`perform`, `answer`,
`window_cmd`, `run_line`) and `cmd.rs` (parsing). Addresses are GUI.dll unless noted; names without a symbol are Ghidra `FUN_<addr>`.
Items marked **GUESS** / **UNRESOLVED** are not read from the binary. Table of what each GUI-local command of `GLOBAL_CMDS` does: §6.

## 1. InfoView (`/help`, `/showfile`, `/tipoftheday`, `text://` links)

### 1.1 Entry points

`InfoViewModule_c::GetInstance()->ShowURL(String)` (0x100ef153) and `ShowText` (0x100ef22a, `text://` + text) both set the DValue
`info_window` = true and emit the module signal (`this+8`) with the URL and flag **1**; `SlotGUIShuttingDown` (0x100ef0ea) clears the history list
(`FUN_100ef032`, `DAT_10276618` = head) and sets `info_window` false. Callers: `/help` 0x100b6d56, `/showfile` 0x100b6e66 (`"file://" + token1`),
`/tipoftheday` 0x100b6f0d, `ChatGUIModule_c::ShowItemRefLink` 0x10085cb5 (`itemref://`, `itemid://`, `charref://`, `text://`; anything else returns).

`/help` (0x100b6d56): no argument → `helpcommands.html`; else the topic map lookup (map `this+0x70` of the dispatcher singleton `FUN_10085abb`,
`FUN_100a4977` fills it, `cmd::HELP_TOPICS`); unknown → `Error: no help topic named '<x>'.` (colour 0x51); found → `ShowURL("file://" + file)`.
`/showfile` without argument: `Usage: /showfile filename` (0x51). `/tipoftheday` (below). Port: `ChatAction::ShowUrl(url)`.

### 1.2 The window

`InfoView_c` ctor `FUN_100ee972`: base `FUN_10038b47(Rect(), "info_window", "Info", style 5, flags 0)` (the DValue-bound window ctor: the window is
shown while the DValue is true), view XML `GUIPath/Views/InfoView.xml`, a vertical view: `TextView BrowserView` (font LARGE, flags
`TVF_MULTILINE|TVF_WORD_WRAP|TVF_ACCEPT_MOUSE_INPUT|TVF_ALLOW_TEXT_SELECTION`, no h-scrollbar, v-scrollbar AUTO, borders 3, `use_macros="true"`)
over a row `HLayoutSpacer, Button "#Back" BackButton, Button "#Forward" ForwardButton` (borders (5,0,5,5) / (0,0,5,5)). Failure to load the XML
shows a `TextView` "Failed to load GUI definition!". The text renderer gets flag 0x80; BackButton / ForwardButton start **disabled**.
The client size is `FUN_100eeedb` = `Point(400, 500)` (0x101b1840 / 0x101b172c; used as the client size = **GUESS**, the creation `Rect()` is empty). The saved
`InfoViewConfig` DValue (CharPrefs.xml archive) is empty on a fresh install. `esc_infoview` (LoginPrefs.xml, default `true`; Options → "InfoView") connects
Esc to close. Port: `Gui::open_tabbed_window("InfoView", "Info", ...)`. **GUESS**: style 5 is drawn with the style-0 tab frame (`WndBorder` style 5
is not decoded), and the placement is the screen centre.

### 1.3 URL handling (`FUN_100ee05c(url, flag)`; flag 1 from `ShowURL`, 0 from link clicks via `FUN_100ee961`)

* `text://` and `charref://` skip the "same URL" test below. Otherwise, **flag set and `url` equals the shown URL (`this+0x1a0`) → the window toggles shut**
  (vtable `+0xfc`); else the URL is remembered. So `/help` twice closes the window.
* `charid://%d/%d` ("<center><font color=CCRed>Please wait<br>Transferring information</font></center>" and a server request), `itemid://`, `shopitemid://`,
  `itemref://%d/%d/%d`, `skillid://%d`, `charref://%u/%u/<text>`: item / skill / character info pages built from game data (`FUN_100384f3`,
  `N3Msg_CreateDummyItemID`, `TipSystem_t::Event("OnLookatItem", name)`). **Not ported** (the port logs it).
* `file://<name>[?query]`: `<name>` = text between `file://` and `?`. `String::LoadTextFile` tried in this order: the name as given; `<ScriptsPath>/text/<name>`; `<CDPath>/text/help/<name>`
  (CDPath = the client's `cd_image/`). The port tries the name as given, then `cd_image/text/help/<name>` (no scripts directory exists). Failure: the
  line `Infoview failed to load and show file: <%s>` on `GlobalSignals+0x17c` with colour code 0xc (CCRed) → the System window (`ChatKind::System`, `system_line(.., 12)`).
* `chatcmd://<rest>`: `GlobalSignals+0x180(rest)` = run as if typed. **That signal is the macro/command runner `FUN_100a3f43`**: `ExpandChatTextArgs(rest)`, split at every literal
  3-character `\n ` (backslash, `n`, blank), each part `Strip`ped and handed to the command dispatcher `FUN_100a39e5`; the dispatcher's result is ignored, so parts without a
  leading `/` do nothing. Port: `Chat::run_line` (used by hotbar macros too; before this it did not split and sent plain text).
  Used by the tip pages: `chatcmd:///TipOfTheDay`, `chatcmd:///close infoview`, `chatcmd:///setoption ShowTipOfTheDay 0\n /close infoview\n /messagebox ...`.
* `text://<html>`: the shown URL becomes `text://<sum>` where `sum` = the signed-`char` byte sum of the whole URL from offset 7 (`"text://%u"`); same-URL test with the flag as above.
* All pages go through `FUN_100eda84(url, html)`.

### 1.4 `FUN_100eda84`: sections and history

* `?section=N` after `?` (pairs `key=value`, `&` separated, `atol`): the HTML is tokenised with `HTMLParser_c`; the **N-th (0-based) `<section>` tag** starts the text, the next
  `</section>` ends it (text between them); an unclosed section leaves the page **empty**. No `section` key: the whole file. (`TipOfTheDay.html` has 39 sections whose `<section>`/`</font>` pairs
  carry the colours: `Tip of the Day:` in `CCInfoHeadline`, the body in `CCInfoText`, both from TextColors.xml.)
* History = static `std::list` of `{url (+8), html (+0x24), scroll location (+0x40)}` with the current iterator (`DAT_10276618`): same URL as the current entry → only its html is replaced;
  otherwise the scroll location of the current entry is stored, **everything after it is erased**, the new entry is appended and becomes current, Back enabled iff current ≠ first, Forward disabled.
  Then `SetText(html)` and the view scrolls to the stored location of the current entry. Port: `info::History` (tested).
* Back (`FUN_100ed627`) / Forward (`FUN_100ed757`): store the scroll location, move the iterator, `url` = that entry's, `SetText`, restore the location, enable/disable the buttons by position.
* The list outlives the window: constructing the window (tail of `FUN_100ee972`) selects the last entry, shows its html and enables Back when there is more than one. Port: same.
* Port scroll location = the pixel scroll offset (the original stores a text buffer position; **GUESS** equivalent). `Gui::scroll_offset` / `set_scroll_offset` were added to ao-gui.
* Link tooltips: `FUN_100ed87d` answers the TextView's tooltip query: unless the page is a `file://…TipOfTheDay.html`, a hyperlink under the pointer that starts with `chatcmd://` and whose command is not `/inspect`
  shows `GetText(10000,"ChatCommand") + <command>`. **Not ported** (ao-gui has no tooltips for text runs).

### 1.5 `/tipoftheday [prev]` (0x100b6f0d)

Requires stat 0x36 (Level) > 3, else nothing happens. `n` = DValue `CurrentTipOfTheDay` (CharPrefs.xml default **-1**); `prev` (case-insensitive, after `Strip`) → `n-1`, anything else → `n+1`; below 0 → **0** (asm 0x100b6ffb);
no upper wrap (a section past the last gives an empty page). URL = `"file://" + CDPath + "text\" + "TipOfTheDay.html?section=" + n`; `ShowURL`; DValue stored. Port: `ChatAction::TipOfTheDay` →
`Chat::tip`, level from `zone.stat(0x36)`. The "Tip of the day" popup at login is `TipView.xml`/the tip system (not decoded here; the Options "Show tip of the day" DValue `ShowTipOfTheDay` has no port store).

## 2. DialogBox_c (message / confirmation boxes)

`DialogBox_c(Window* parent, const String& name, String text, const char* button, ..., 0)` (ctor 0x1012a477, variants 0x1012a555 / 0x1012a62f with an explicit `View*` form):
`Window(Rect(), "dialogbox_window"?, <name>, style 1, flags 0x900)`, `_Init` 0x1012a2f6 builds `DialogBoxView_c` and `AddModalWindow(parent)` when there is a parent, `MoveToMouse(true)`.
`Go` 0x1012a3bf: reads DValue `esc_dialogs` (LoginPrefs.xml default **true**) → connects `SlotEscPressed`; `Show(true)`, `MakeFocus`. Callers follow with `Window::MoveToCenter`.
`SlotSelected(int)` 0x10129fe9: forwards to the form (`DialogBoxForm_c`), emits the answer signal `(index, dialog data)`, then closes (or only hides + disconnects Esc when auto-close is off, `SetAutoClose`, `+0x90`, default on).
`SlotEscPressed` = `SlotSelected(-1)`; `OkToQuit` (window close) = the same with -1. `SetCloseTimeout(ms)` starts an `EventTimer` that closes the window.

`DialogBoxView_c` 0x1012aa23: view `"dialog_box_view"` (vertical layout) = a `TextView_c` (HTML, `HTMLParser_c::SetFeatureFlags(0x60)`, `TextRenderer_c::SetAspectRatio(2.0)` = `_DAT_101ae17c`; borders **15, 5, 15, 20**
= `_DAT_101b00e0 / _DAT_101a8b98 / _DAT_101b00e0 / _DAT_101b4e08`) and `DialogButtons_c` (borders (0,0,0,5)). `DialogButtons_c::_Initialize` 0x1012ad5f: horizontal row, a leading `HLayoutSpacer(0, 16000, 1.0)`, one `Button_c` per label
(id = its index, borders 8,0,8,0 = `_DAT_101b0ed8`), and a **trailing spacer when there is exactly one button** (so one button is centred, several are right-packed). The answer is the button id.
Port (`dialog.rs`): view XML with the same borders / spacers in an `open_framed_window_xml` frame, centred. **GUESS**: the text wrap width (the aspect-ratio algorithm is not decoded; ours is `sqrt(2·chars·7·14)` clamped 160..520 px) and the frame (style 1 as the other `DialogBox`es of the login flow, docs/screens.md §9).
Esc / frame close answer -1.

| dialog | opener | text / labels (text.mdb) | answer |
|---|---|---|---|
| `/messagebox <text>` | `FUN_100b7a6f`: no argument → `Usage: /messagebox <message>` (0x51); else `DialogBox_c(0, "", ExpandChatTextArgs(token1), GetText(10000,"MsgBox_OK"))`, `Go`, `MoveToCenter` | body = the expanded text; button `MsgBox_OK` | none |
| `/org leave` | `OrganizationGUIModule_c::LeaveOrg` 0x10052568 (signal from the `/org` text command, `Local::OrgLeaveDialog`) | cat 10000 `ReallyLeaveOrg` (body), `Warning`, buttons `MsgBox_Yes`, `MsgBox_No` (index 0 = Yes). Which of `ReallyLeaveOrg` / `Warning` is the title and which the body is **not read** (stack order of the by-value argument): body = `ReallyLeaveOrg` | Yes → `LeaveOrgConfirmed` 0x10052032 → `N3Msg_OrgLeaveConfirmed` (Gamecode 0x1001a7c1): `OrgClientIIR_c` code **0x10**, id `{0,0}`, empty text |
| `/org disband` | `OpenDisbandDialog` 0x100520dd (`Local::OrgDisbandDialog`), window name `OrgDisband` | cat **501** (0x1f5) `Org_ConfirmDisband` (body), `MsgBox_Yes` / `MsgBox_No` | Yes → `DisbandDialogClosed` 0x10051ffc → `N3Msg_OrgDisbandConfirmed` (Gamecode 0x1001a611): `OrgClientIIR_c` code **6**, id = the engine target (`FUN_10058816()+0x5c`, [INFERENCE] = GUI target), empty text, flag 0 |

Other `DialogBox_c` users seen (not chat-command reachable, not ported here): `OpenLeavePayTaxQuery` 0x1005221d, `OpenTaxChangeLeaveQuery` 0x100523d1, `OpenPromotionDialog` 0x100526e6 (`ConfirmPromotion`),
`OpenInvitationDialog` 0x10052978 (`OrgJoin`), `ConfirmUseItemDialogue` 0x1003012f, `StartPvPFightDialogue` 0x1002fa8e, duel dialogs (`DuelChallengeReceivedResult` 0x1002f802), `GuiSystem_c::ShowMessageBox` 0x1002f852 ("Information").

## 2.1 The `/afk` message dialog (`AFKMessageDialog_c`)

Bare `/afk` (not AFK yet or AFK with another text path, `n < 2`) opens it at 0x100b7fa1: `FUN_10082a6f` = `Window(Rect(), "", name, style 1, flags 0x800)` containing the form `FUN_1008308d` (`AFKDialogView_c`) and a `DialogButtons_c("Ok")` (literal `Ok`, 0x101bb970);
`InputConfig_t+0x1c8` = 1. The form (vertical): body `TextView` (HTML flags 0x20, text = `ChatCmdFeedback_AFKDialogBody`, bottom border 10), a countdown `TextView` showing **`30`**, and an input (`BorderView` client margins 3 / 2, max width 16000) prefilled
with `DefaultAFKReply` (cursor at the end). Form borders (15, 10, 15, 15). A frame timer (`FUN_10082e78`) shows `30 - elapsed` (whole seconds from `_time64`; the format string at 0x101a99a4 is **GUESS** `%d`) and, once **more than 30 s** passed, answers **0**.
Keys inside the input (`FUN_10082f50`, key codes 0x18 / 0xe): 0x18 → answer 0, 0xe → answer -1 (**GUESS**: Enter / Esc). The answer `FUN_100829a1` passes `(index, input text)` to the callback `FUN_100b6576`:

* index 0 and text non-empty: if it differs from the current AFK message → `ChatCmdFeedback_ChangedAFKMessageTo(old, new)` (cat 10001) on `GlobalSignals+0x17c`, then `SetAFK(text)`;
  empty text → the line `Using default AFK message.` (hard-coded);
* index ≠ 0 (Esc): `SetAFK("")` and the line `AFK off.` (hard-coded).

Both after `/afk` already switched AFK on with the default message (cmd.md §`/afk`; the `AFK_AFKOn` line and the `/me` announcement are not undone by Esc). Port: `dialog::Kind::Afk`, `Chat::answer`.

## 3. `/camp`, `/quit`, `/open` `/close` `/toggle`

* `/camp` (0x100b587d): `AFCM::Send(10, 0x134)`. 0x134 is registered at 0x1002aef8 for `FlowControlModule_t::StartQuitToLoginMessage` (0x10027c74): unless `m_eLoggingOutTimed == 2`, state 0 → `N3Msg_StartCamping`
  (refusals as chat feedback; docs/zone/actions.md §3), success → state 2; the camp timer (`m_pcCampTimer`, `TimerBarBase_c`) ends in `ActivateGameClosing(2)` (0x10028194 → back to login). Port: `GameAction::Camp` →
  `Play::camp()` (owner: Avatar.SitServer, `play/hud_use.rs`).
* `/quit` (0x100b5895): `AFCM::Send(10, 0x133)` → `StartQuitToSystemMessage` (0x10029a0d): emits `GlobalSignals+0x294` and `+0x20c`; if `m_eLoggingOutTimed == 1` **or a previous `/quit` was less than 3000 ms ago** → `AFCM::Send(10, 0x109)` =
  `QuitGameToSystemMessage` (0x10028d47, `ActivateGameClosing(1)`: immediate quit); otherwise it stores the time, prints a text of category 200 (`GetText(0xc8, …)`, key not read) in colour 0xc, calls `N3Msg_StartCamping`, state 1
  (the timed logout then ends in `QuitGameToSystem`). Port: `Chat::take_quit()` → `host.quit = true` immediately (the 3-second double-/quit rule and the camp-then-quit path are **not** ported: they depend on the
  camp timer, owner Avatar.SitServer; the port's Esc key already quits at once).
* `/open`, `/close`, `/toggle <window>` (all `FUN_100b77b6`; no argument → `Usage: /open window`; quotes around the name are stripped): sense 0 open / 1 close / 2 toggle (open = `CompareNoCase(tok0,"/open")`, close/toggle by `"/close"`, asm 0x100b77f0). The name is looked up case-insensitively in the table at 0x1026c4a0
  (`cmd::WINDOW_NAMES`, 24 pairs, `ChatConfig → chat_group_window` … `ItemStore → itemshop_window`); the paired **DValue** is set (toggle = current value negated; `pet_window` additionally tests stat 0x1ca, [UNRESOLVED] what happens at 0).
  A name not in the table selects a chat window by its title (`FUN_100945ae` / `FUN_10093c1f`, `FUN_1009adae`): **not ported**. Port: `InfoView` is ours; every other row is handed to the HUD through `Chat::take_windows()` →
  `hud::WindowKind::from_dvalue` → `Hud::open/close_kind/toggle` (rows without a HUD window are ignored by the flow).

## 4. Esc

Esc closes the topmost dialog box first (`DialogBox_c::SlotEscPressed`), else the InfoView (`esc_infoview`); otherwise the flow's existing Esc handling runs (`Chat::esc_closes` / `Chat::escape`).

## 5. Gating of GM-only commands (documented, not implemented)

`FUN_100a38cc(name, argc, flag, gm)`: a registration with the 4th argument set stores the condition string `stat:gmlevel != 0` (parsed by `ExpressionParser_c`); false → `ChatCmdFeedback_CommandNotAuthorized`. The
`FUN_100b24e4` third loop (0x100b272c) uses `stat:gmlevel & 0x0001` for `/clone /criterialocal /damagemult /joycamacc /spelllocal /resetskill`. Further GM checks sit inside `FUN_1003fba6` (`N3Msg_GetSkill(0xd7,2)`).
The Fanatic group (`/anon /stuck /list /shop /teleport /tp /monster /npc /spawn /reload /weather /perks /perk /gethash /item /dumphash /framerate /lazyreload /spawnacgentrance /spawnquest /syncdisplay /teleportdynel /getfull`),
`/command` (0x100b379b, `Fanatic::ClientInterface_c::Command` with an identity from `t <id> <id2>`; "Error: Invalid ID." / "Error: To few arguments."), `/gfx enable|disable <n>` (0x100b3258 → `N3Msg_EnableGFX`),
`/fxscript <script> [params] #` (0x100b6acf → `SignalFXSScript`), `/terminate` (0x100b345e, a `"Terminate"` window) are developer tools: no GM string at their registration, but they act on debug/GM-only subsystems; not implemented.

## 6. Every GUI-local row of `GLOBAL_CMDS`

| command | handler | status |
|---|---|---|
| `/help` | 0x100b6d56 | implemented (§1) |
| `/showfile` | 0x100b6e66 | implemented |
| `/tipoftheday` | 0x100b6f0d | implemented (§1.5) |
| `/messagebox` | 0x100b7a6f | implemented (§2) |
| `/text <t>` | 0x100b5913: `FUN_1009b37f(token1, 0x52)` | implemented (info-coloured local line) |
| `/funcom` | 0x100b59d3: "Funcom made this excellent product :)\nThank you for playing Anarchy Online." (0x52) | implemented |
| `/camp` `/quit` | AFCM 0x134 / 0x133 | `/camp` via `GameAction::Camp`; `/quit` quits at once (§3) |
| `/open` `/close` `/toggle` | 0x100b77b6 | implemented for the InfoView and HUD windows (§3) |
| `/afk` dialog | 0x100b7cb8 + `FUN_10082a6f` | implemented (§2.1) |
| `/org leave`, `/org disband` dialogs | 0x10052568 / 0x100520dd | implemented (§2) |
| `/start <url>` | 0x100b9a84: no argument → usage; if `DisplaySystem+9` (full screen) a `DialogBox_c` "Warning:" asks first (`FUN_100bbd8e` callback → `FUN_100b94e0`), else `FUN_100b94e0` directly: only `http://` / `https://` are started, `ShellExecuteA("open", first token, rest as parameters)` | not wired (needs no module but the confirm texts of the full-screen branch are not read; the URL start is the same `open` call `Play::show_error` uses) — **gap** |
| `/petition` | 0x100b58ad: DValue `petition_window` = true → `BrowserWindow_c` type 3 ("Petition", an embedded CEF page, `OkToQuit` 0x1010f402 clears the DValue) | **not implemented**: no embedded browser, and the petition URL is not located |
| `/option` `/setoption` `/dvalue` | 0x100b5adc / 0x100b7127: `Invalid syntax: %s . Use /option <OptionName> [value]`, `Can't find option <%s>.`, `Variable <name> = <value>` / `Changed variable <name> from … to …`, `Failed to parse expression <%s>.` (colour 0x51/0x52), `IndependentPrefs_t::GetPrefEasy/SetPrefEasy`, `DistributedValue_c` get/set with min/max clamp | **not implemented**: needs the generic DValue / IndependentPrefs registry (the port only has `Hud::dvalues` for window flags) |
| `/chardist <n>` `/viewdist <f>` `/char&viewdist <n> <f>` | 0x100b62ed / 0x100b63b8 / 0x100b6465: DValue `DisplayCharViewDistance` = `atol(tok1)`; pref `ViewDistance` = `atof(tok) * _DAT_101bb7a0`; "Error: To few arguments" | **not implemented**: the port has no consumer of either value (far plane is a constant, docs/formats.md) |
| `/voice <sound>` | 0x100b82c2: needs stat 0x185 bit 1 (else "This function requires %s."), sends the `<sound name>` as a vicinity message built by `FUN_1008c012 / 1008c762 / 1008cb83 / 1008c17b` (voice chat) | **not implemented** (voice-chat sound table not decoded) |
| `/macro <name> <command>` | 0x100b8693: emits `GlobalSignals+0x1a0(name, command)` (creates a `TextMacro_t`) | not implemented (macro system not ported) |
| `/duel`, `/petduel` | 0x100b8a10 / 0x100b8789: `N3Msg_PetDuel_Challenge/Accept/Refuse/Stop` (`/petduel [accept\|reject\|stop]`, target must be a player: "You need to target a player first."); `/duel` similar | not implemented: combat module owns the duel messages |
| `/filter [list\|del\|add\|enable\|disable\|clear]` | 0x100b8d4e: `ChatFilterRules` DValue archive (`%3d: %s` rows) | not implemented (chat filter is a documented gap, gui.md §4) |
| `/waypoint x z playfield` | 0x100b9377: `GlobalSignals+0x158(Point, (x,z), pf)`; "Usage: /waypoint x z playfield" | not implemented: the receiver of that signal was not identified |
| `/rp` | 0x100b21dd: `N3Msg_EventFeedback(0x46, …, stat 0x2a1 ^ 0x200)` | not implemented (event feedback / roleplay flag toggle on an unlocated stat) |
| `/reclaim` | 0x100b21ca: `N3InterfaceModule_t` call with argument 0 | not implemented (target not identified) |
| `/selectself` `/bug` `/assist` | 0x100b593a (`AFCM::Send(0x1e, 0x126, own id)`), 0x100b5a2a (`AFCM::Send(10, 0x121, text)`, ≥ 20 chars else "Bug description to short. Should be at least 20 characters."), 0x100b7bc5 | `/assist` is done elsewhere; the other two are listed in cmd.md |
| `/command` `/gfx` `/terminate` `/fxscript` | see §5 | developer tools, not implemented |

## 7. Verification

`cargo test --release -p aomac chat::info chat::dialog chat::cmd` (history, section extraction, `text://` key, real help files, Tip of the Day section 3, dialog answers / AFK countdown, command parsing).
