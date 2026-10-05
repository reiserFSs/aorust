# Chat input line: slash commands and default targets

Code: `crates/aomac/src/play/chat/cmd.rs` (`parse(line, &CmdCtx) -> Vec<ChatAction>`). All addresses are GUI.dll
(`ChatGUIModule_c` / `TextInputModule_t` / command dispatcher) unless noted. Decompiles are Ghidra "FUN_" names (the binary
is stripped for these); strings are quoted from the binary.

## Flow

```
Enter in a chat window  ->  FUN_1009a2f8(window, text)
   FUN_10085abb()                      create the dispatcher singleton (FUN_100a4977 fills the /help topic map)
   FUN_100a39e5(window, text)          command dispatcher; returns 0 for text that does not start with '/'
   if 0 && FUN_1009a26c()              window's output group (+0xb4) found in the group map, !read-only && shown in this
                                       window (FUN_1009989e)  ->  group.Send(ExpandChatTextArgs(text), mode 0)  (FUN_10085538)
                                       else: nothing happens (no feedback)
```

`FUN_100a39e5` for a line starting with `/`:

1. `FUN_100a3436`: the command word = text up to the first whitespace at index >= 1 (so `/` alone is a word, `//x` is word `//x`).
2. Lookup, **case-insensitive, exact, no prefix/abbreviation matching**: first the window map (`FUN_10099fa9`, map at
   window+0x204, filled by `FUN_1009dc28` 0x1009e2c5..0x1009e721), then the global map (`FUN_100a3699`, map at dispatcher+0xc);
   `FUN_1009f618` is the case-insensitive `std::map::find` (`String::CompareNoCase`). A miss falls back to a file
   `<scripts path>/<word without slash>` (`AnarchyPath_t::GetScriptsPath`, `stat64`); no file ->
   `ChatCmdFeedback_CommandNotFound` (`%s` = word without the slash), colour 0x51.
3. GM-only commands (`FUN_100a38cc` param 5 -> condition string `stat:gmlevel != 0`, parsed with `ExpressionParser_c`):
   false -> `ChatCmdFeedback_CommandNotAuthorized`.
4. `String::Tokenize(line, cmd.argc)` (Utils.dll 0x1000e2a6, see below). `cmd.argc` is the 3rd argument of the registrar
   `FUN_100a38cc(name, argc, flag, gm)` (stored at cmd+8) and is the *maximum number of tokens*: the last token is the rest of the line.
5. `/<cmd> help` (second token `help`, case-insensitive; for argc < 3 tokens are re-split without limit) is rewritten to
   `/help <cmd>` when `<cmd>` is a key of the help-topic map (`this+0x70`, lookup `FUN_1006db0c`). **Guess**: what exactly
   `*piVar11 != this+0x74` tests (decompile: `find(map, name) != end`); the topic map is the one built by `FUN_100a4977`.
6. The command's execute signal is emitted with `(window, line, tokens)`.

Colours of feedback lines: `FUN_1009b37f(text, ColorCode_e)` expands `%`-args (`ExpandChatTextArgs`), maps the code with
`ChatGUIModule_c::ColorCodeToHTMLColor` (0x10087860; table at 0x10268d38: 0x51 = `CCChatCmdFeedbackError`,
0x52 = `CCChatCmdFeedbackInfo`, both in `TextColors.xml`) and adds `<div><font color=NAME>text</font></div>`. `feedback_line()` does exactly that.
Which of 0x51/0x52 the calls without a visible argument use is a **guess** (the decompile dropped the argument; usage and
error texts are 0x51, the AFK/"no tells yet"/lft texts 0x52).

### `String::Tokenize(s, max)` (Utils.dll 0x1000e2a6) — `cmd::tokenize`

Whitespace separated (`iswspace`); leading blanks of a token are skipped; a token starting with `"` runs to the next `"`
that is followed by whitespace or the end and **keeps its quotes** (handlers that need a bare name strip them: `/ch`, `/g`);
a `"` anywhere else is an ordinary character. When `max-1` tokens exist the **rest of the line, verbatim** (including extra
leading blanks) becomes the last token; `"/tell Bob "` (trailing blank) gives an empty last token. `/tell "Bob Smith" hi`
therefore passes the quoted name *with* quotes to the tell code (no stripping there).

### `ExpandChatTextArgs` (0x1008619b) — `cmd::expand`

`%%` -> `%`; `%f` -> name of `N3Msg_GetAttackingID` or `&lt;no fighting target&gt;`; `%m` -> own name (`InputConfig_t+0xd8`);
`%t` -> name of the current target (`InputConfig_t+0xc0`) or `&lt;no target&gt;`; every other `%x` is left as typed. Applied to
tell target and text, group/vicinity text, plain text, `/afk` message, forwarded game commands and feedback.
(The `&lt;` / `&gt;` are literally in the binary: the text goes through the HTML `TextView`.)

## Command tables

Window map (`FUN_1009dc28`; registrar `FUN_1009d5fe(name, argc, flag)`; handler pushed with `FUN_1009efd9`):

| word | argc | handler | meaning |
|---|---|---|---|
| `/ch` | 2 | 0x1009be67 | select input group |
| `/group` `/g` | 3 | 0x1009c226 | `/g <group> <text>` |
| `/o` | 2 | 0x1009c6ad | organisation group (first *active* group with type byte 3) |
| `/t` | 2 | 0x1009c8d7 | team group (type byte 0x82) |
| `/` `/v` `/say` | 2 | 0x1009caf6 mode 0 | vicinity say |
| `/w` `/whisper` | 2 | 0x1009caf6 mode 1 | whisper |
| `/s` `/shout` | 2 | 0x1009caf6 mode 2 | shout |
| `/me` | 2 | 0x1009caf6 mode 3 | emote |
| `/script` | 2 | 0x1009cd98 | run script (`FUN_100b0ee2`); no argument -> text `Error2FewArgs` (cat 10001) |

The mode is chosen inside 0x1009caf6 by `CompareNoCase(token0, "/whisper"|"/w"|"/shout"|"/s"|"/me")`, else 0.
`FUN_10085538(window, text, mode, macro)` is `Group::Send`; for the vicinity group (id `0x40000002`) modes 0/1/2/3 are
say/whisper/shout/emote (zone ptype 5, docs/zone/outgoing.md §6); an inactive group prints
`"Error: Chat group %s is currently not available."` (0x101b78e8).
There is **no `/channel`** in the binary although `Chatting With Others.html` documents it: `/channel x` answers CommandNotFound.

Global map (`FUN_100badf1` 0x100badf1..0x100bbc30; handler address in the 2nd column):

| word(s) | argc | handler | what it becomes |
|---|---|---|---|
| `/tell` | 3 | 0x100ba2e6 | queued action 2: target `Expand(tok1)`, text `Expand(tok2)` or empty; `< 2` tokens: silently nothing. Empty text opens the tell window (0x1008947b: `OpenTellWindow`) |
| `/reply` `/r` | 2 | 0x100ba42f | target = head of reply list (+0x288); empty list: info "You have not received any tell messages yet." (hardcoded, 0x52) |
| `/afk` | 2 | 0x100b7cb8 | see below |
| `/invite` `/kick` `/leave` | 2 | 0x100ba6db / 0x100ba7e6 / 0x100ba8f1 | queued actions 3 / 4 / 5 with `Expand(tok1)`; usage `Usage: /invite &lt;nick&gt;` |
| `/ignore` | 2 | 0x100ba9fc | see below |
| `/name` | 2 | 0x100ba5b7 | action 2 / sub 5 "name-request"; usage `&lt;new name&gt;` |
| `/cc` | -1 | 0x100ba105 | `info <name>` (exactly 3 tokens) -> action; `< 2` tokens "Invalid syntax"; chat server down "Error: Not connected to chat-server."; else all tokens expanded and handed to `Client_c` (0x1016cadf) |
| `/lft` | 2 | 0x100b69b0 | `FUN_100f01a3(on, text)`: with text on, bare toggles |
| `/help` | 2 | 0x100b6d56 | topic map lookup -> `file://<file>` in InfoView; none: "Error: no help topic named 'X'."; no argument: `helpcommands.html` |
| `/petition /fxscript /selectself /funcom /bug /showfile /tipoftheday /option /setoption /dvalue /open /toggle /close /messagebox /assist /text /start /camp /quit /chardist /viewdist /char&viewdist /voice /macro /petduel /duel /filter /waypoint /rp /reclaim` | see `GLOBAL_CMDS` | GUI-local | `ChatAction::ClientCommand(line)` (not chat) |
| `/played` | 1 | 0x100b2321 | prints "Time: %02d:%02d local (%02d:%02d GMT), %02d:%02d game<br>" + "Date: ..." then `N3Msg_TextCommand("played")` |
| `/version /bank /team /org /born /pet /follow /items /raid` | -1 | 0x100b2278 | `N3Msg_TextCommand(window, Expand(line[1..]), target)` (Interfaces 0x10008a72 -> Gamecode 0x176db -> FUN_1003fba6) |
| GM: `/chr /getlocal /setlocal /getlocalfull /monsterdata /clearunique /tplocal /clone /criterialocal /damagemult /joycamacc /spelllocal /resetskill` | -1 | 0x100b2278 | same, gated by `stat:gmlevel != 0` (FUN_100b24e4 0x100b25eb..) |
| `/inspect` | 2 | 0x100b2228 | `N3Msg_Inspect(character 50000, strtoul(tok1))` |
| `/anim` `/emote` | 2 | 0x100b2be7 / 0x100b2aba | `N3Msg_DoSocialAction(id)`; `/emote` looks the name up in the 70-entry table (0x101badc4+12*id), else "Error: No emote named 'X'." |
| `/<emote>` (70 names, `EMOTES`) | 1 | 0x100b2aba | same; with argc 1 the token is the whole line so `/wave now` does nothing (faithful quirk) |
| `/command /gfx /tower /terminate /getfull /anon /stuck /list /shop /teleport /tp /monster /npc /spawn /reload /weather /perks /perk /gethash /item /dumphash /framerate /lazyreload /spawnacgentrance /spawnquest /syncdisplay /teleportdynel /reloadgfxtweak /togglegroundlightingfix` | 2..4 | 0x100b30cf, 0x100b314f ... | forwarded as above (FUN_100b39f6) |

### `/g` `/group` `/ch` group matching (`FUN_10083814`)

Names are stripped of leading/trailing `"`. A group matches when its name starts with the typed text (`CompareNoCase` over the
typed length). A group of exactly the typed length wins alone (the vector is cleared). No match: `No chat-group named 'X'.`
(hardcoded 0x101b90d0 + `'.` 0x101b90cc).
* `/g`: one match: read-only -> `GroupIsReadOnly(%s=name)`; several: the non-read-only ones (a second one ->
  `AmbiguousGroupName`, none -> `AllMatchingGroupsAreReadOnly`). Send, then if pref `ChatWarnWhenSpeakingToUnsubGroups` and the
  window does not show the group: `TalkToUnsubscribedChannel`.
* `/ch`: exactly 2 tokens else `Usage: /ch &lt;group name&gt;`. One match is taken as is; several: skip read-only and not-shown
  ones, a second candidate -> `AmbiguousGroupName`, none -> `Ch_NoMatchingGroupAreSelectable`. The pick must be shown in this
  window (`Ch_CanOnlySelectSubscribedGroups`) and writable (`GroupIsReadOnly`); then `FUN_1009a06f` sets the window's output group
  (the identifier: `GetGroupIdentifier` 0x1001b5d5 = `"#%016I64x#"` for private ids, the name for named groups).
* `AmbiguousGroupName` args (`FUN_1009a9f3`): the candidate list (each `<font color=red|white|silver>` for read-only | shown |
  other, joined by ", "), then token0 and the raw token1 as typed.

### `/afk` (0x100b7cb8)

* `/afk status`: `AFK_Status_AFKIsCurrentlyOn(%s=message)` / `...Off`.
* Already AFK and bare `/afk`: `SetAFK("")`, `AFK_AFKOff`, and `AFK_IsBack` sent to the vicinity group with mode **3 (/me)** (asm 0x100b7f19 `PUSH 3`).
* Otherwise message = `Expand(tok1)` or `DefaultAFKReply` (bare `/afk` also opens the custom-message dialog:
  `AFKDialogBody`, 0x100b7fa1 `FUN_10082a6f`); not AFK yet: `AFK_AFKOn` + `AFK_IsAFK` as `/me` (0x100b8171 `PUSH 3`);
  already AFK with another text: `ChangedAFKMessageTo(old, new)`; then `SetAFK(message)`.
  **Guess**: whether the dialog callback replaces the default message (the decompile continues straight into SetAFK).
* Auto reply to incoming tells (`ChatCmdFeedback_AFKReplyText`) lives in `HandlePrivateMessage` (0x1008792e), not here.

### `/ignore` (0x100ba9fc)

Needs `IgnoreSystem_t`. `/ignore list` (case-sensitive) prints "Ignored characters:" and `%d\t%s` per entry. A first
character that is a digit means a character id (`Identity{50000, strtoul}`); another word is queued as action 9 (by name);
bare `/ignore` uses the current target. Then: nothing -> `Ignore_NoTargetOrNick`; type != 50000 ->
`Ignore_CanOnlyIgnoreCharacters`; own id (`InputConfig_t+0xd4`) -> `Ignore_CantIgnoreYourself`; else toggle with
`Ignore_IgnoringCharacter` / `Ignore_UnignoringCharacter` (name via `N3Msg_GetName`). `Ignore_ToManyArguments` is unreachable with argc 2.

### Usage lines

`"Usage: " + token0 + suffix`, hardcoded (0x101b90fc): ` &lt;message&gt;` (`/o /t /say /s /w /me`), ` &lt;group name&gt; message` (`/g`),
` &lt;group name&gt;` (`/ch`), ` &lt;nick&gt;` (`/invite /kick /leave`), ` &lt;new name&gt;` (`/name`), ` &lt;emote-name&gt;`, ` &lt;animation-name&gt;`.

## Text db

`ChatCmdFeedback_*` keys are `LDBface::GetText` string keys of category **10001** (`TextDb::by_key(10001, key)`; `Error2FewArgs` has
no prefix). `%s`/`%d` are filled by `LDBformat::Feed` left to right. Only hardcoded English strings: usage lines, "No chat-group
named", "Error: no help topic named", "Error: No emote named", "Invalid syntax", "Error: Not connected to chat-server.",
"You have not received any tell messages yet.", "Error: Chat group %s is currently not available.". `ChatCmdFeedback_FailedToSendPrivateMessage`,
`Petition_Sending`, `Name_SendingName`, `Ignore_CantInviteIgnored`, `UnknownUser` are produced where the queued actions are
executed (0x1008947b / 0x10089dfc), i.e. by the hub, not by `parse`.

## Input bar hot keys

`TextInputModule_t::StartChatCmdMessage` (0x10021f18) opens the bar with `"/"` (`CMD_PREFILL`); `StartChatReplyMessage`
(0x10021fd0, Shift+R) opens it with `"/tell " + <head of reply list> + " "` (handler 0x1009494e, strings 0x101b8850 `"/tell "`,
0x101aa270 `" "`); with an empty reply list the bar opens empty (`reply_prefill`).

## Gaps / guesses

* The `/help <cmd>` redirect condition and the keys marked `*` in `HELP_TOPICS` (org, pet, chat, team, misc, list, perk, raid share
  their string with another literal; inferred from chatcommands.html).
* Colour code (0x51 vs 0x52) of the feedback calls whose argument the decompile lost.
* GM gating flags of the debug commands registered by 0x100b39f6 (their registrar arguments depend on a register the listing
  filter hid); they are forwarded ungated, the server enforces.
* `/tower` (0x100b314f: `create`/`terminate` subcommands go to `Fanatic::ClientInterface_c::Command`), `/terminate`, `/command`, `/gfx`
  handlers not decoded; all become `ZoneCommand` of the full line.
* Tell target capitalisation: the client does **no** case folding (`ExpandChatTextArgs` only); the name is resolved on the chat
  server (lookup 0x15). "You can't send private messages to yourself." (0x1008947b) compares ids after the lookup: hub's job.
* A script file fallback passes only its name (`RunScript`); how arguments reach the script is not decoded.
