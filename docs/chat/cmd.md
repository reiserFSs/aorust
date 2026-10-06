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
| `/chr /getlocal /setlocal /getlocalfull /monsterdata /clearunique /tplocal` | -1 | 0x100b2278 | forwarded as above (no GUI gate; `FUN_1003fba6` itself checks GmLevel, see §Zone commands) |
| GM: `/clone /criterialocal /damagemult /joycamacc /spelllocal /resetskill` | -1 | 0x100b2278 | same, GUI gate `stat:gmlevel & 0x0001` (string 0x101baa04, assigned in the 3rd registration loop of FUN_100b24e4 at 0x100b272c); false -> `CommandNotAuthorized` |
| `/inspect` | 2 | 0x100b2228 | `N3Msg_Inspect(character 50000, strtoul(tok1))` |
| `/anim` `/emote` | 2 | 0x100b2be7 / 0x100b2aba | `N3Msg_DoSocialAction(id)`; `/emote` looks the name up in the 70-entry table (0x101badc4+12*id), else "Error: No emote named 'X'." |
| `/<emote>` (70 names, `EMOTES`) | 1 | 0x100b2aba | same; with argc 1 the token is the whole line so `/wave now` does nothing (faithful quirk) |
| `/anon /stuck /list /shop /teleport /tp /monster /npc /spawn /reload /weather /perks /perk /gethash /item /dumphash /framerate /lazyreload /spawnacgentrance /spawnquest /syncdisplay /teleportdynel /getfull` | 2 | 0x100b30cf | `Fanatic::ClientInterface_c::Command(window, target, line without slash)` = `FanaticIIR_t` (not `N3Msg_TextCommand`; line NOT expanded) |
| `/tower` | 2 | 0x100b314f | `create` (and `terminate` on a tower target) -> Fanatic; everything else `N3Msg_TextCommand(line)` (unexpanded) |
| `/command /gfx /terminate /reloadgfxtweak /togglegroundlightingfix /rp /reclaim` | | 0x100b379b / 0x100b3258 / 0x100b345e / 0x100b3317 / 0x100b30ad / 0x100b21dd / 0x100b21ca | GUI-local (dialogs, reloads); not decoded -> `ClientCommand` |

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

## Zone commands (`N3Msg_TextCommand`, `FUN_1003fba6`) — `ao_net::n3::textcmd`

Chain: GUI wrapper 0x100b2278 (`ExpandChatTextArgs(line[1..])`, target = `InputConfig_t+0xc0`) -> Interfaces 0x10008a72 -> Gamecode
`N3Msg_TextCommand` 0x176db: `GetClientControlDynel` must exist, `FUN_10058816` (lazy singleton at `this+0x1d0`, no wire effect), then
`FUN_1003fba6(window, text, target)`. It reads the first word with `istringstream >> string` (`FUN_10043f4d`) and compares with
`std::string::compare` (`FUN_10043dea`): **case-sensitive** (the GUI map before it is not); unknown words do nothing at all.
Everything sent goes through `n3Dynel_t::SendIIRToObservers(control dynel, iir)` = zone connection, ptype 10; all header identities are
`{0xC350, own char id}`, "to be passed on" byte 0.

`CharacterActionIIR_t` ctor `FUN_1007253f(target, idA, param, action, idB, text)` (asm push order: `text, idB, action, window, idA, target`;
stored at `+0x20/+0x1c/+0x18/+0x28/+0x30`). **`param` = the window id** for text commands (0 for `items` and `Inspect`).

| word (case-sensitive) | gate | wire | evidence |
|---|---|---|---|
| `version` | – | action 0x9d, idA/idB 0 | 0x1003fc56..0x1003fcb5 |
| `items` / `items list` | – | action 0xf4 (`FUN_1004b1ab`), param 0 | 0x1003fd1a |
| `items delete <n>` | – | action 0xf5 (`FUN_1004b254`), idB `{0,n}`; a non-numeric word leaves `n` = the window id (`sscanf` into the argument slot) | 0x1003fd6b..0x1003fdd4 |
| `resetskill <stat>` | GmLevel (`N3Msg_GetSkill(0xd7,2)`) | 0x9a, idB `{0,n}`; `n` = digits (`%u`) else `FUN_1002edf3` stat-name lookup (`_stricmp`; unknown 0x499602d2) | 0x1003fdf9 |
| `clearunique <stat>` | GM | 0xae, idB `{0,n}` (same parse) | 0x1004038e |
| `clone` | GM | 0x96 | 0x100400c5 |
| `damagemult [list\|<n>]` | GM | 0x67, idB `{0,n}`, default n 100, `list` -> idB 0 | 0x10040221 |
| `played` | – | 0x85 (after the GUI printed the time lines, below) | 0x10040187 |
| `born` | – | 0x86 | 0x10041c3a |
| `bank open` / `close` / `info` | open: GM | 0x24 / local close (`FUN_10046bdd`) / 0x5c; sub-word list `bankcmd` 0x102e2630: open 1, close 2, info 3 (`_stricmp`) | 0x1003ff2b |
| `team` | in team (`FUN_100657d1`) | not in team: `Feedback_YouAreNotMemberOfTeam`; no word: `Feedback_AvailableTeamCommands` + `Feedback_TeamLoot` + `Feedback_TeamLootAll`; `team loot` -> 0x91; `team loot <x>` -> own id == team leader id: 0x7f idA `{0, teamcmd(x)}`, else `Feedback_OnlyTeamLeaderCanChangeLootOrder`. `teamcmd` list 0x102e2660: team 1, loot 2, all 3, leader 4, alpha 5 | 0x100406df..0x100408d9 |
| `raid <sub>` | – | `RaidCmdIIR_c` (below); subs (map 0x102e2720): create 1, list 2, listlocal 3, move 4, lootaccess 5, locks 6 | 0x100404bb..0x100406d4 |
| `org <sub> <rest>` | – | `OrgClientIIR_c` (below) | 0x100408e3..0x10041491 |
| `pet <..>` / `tower <..>` | | `PetCommandIIR_c` (below); gates `Feedback_YouHaveNoPet` / `YouHaveNoServiceTower` (own pet list empty), `InvalidPetcommand` / `InvalidTowerCommand` | 0x10041ca8 / 0x10041df4 |
| `follow` | | `FollowTargetIIR_c` toward the current target (below) | 0x10041f02 |
| `getlocal setlocal getlocalfull criterialocal spelllocal monsterdata joycamacc tplocal` | GM | client-side debug (`DebugSpellListToChat`, `SetRelPos` ..): not decoded | 0x10041525.. |

`ao_net::n3::textcmd::text_command(line, &TextState) -> TextResult` implements the table (byte-layout tests in the module); feedback keys are text.mdb
**category 110** (`LDBface::GetTextPtr(0x6e, key)`; e.g. `Feedback_YouAreNotMemberOfTeam` = "You are not a member of a team!"), emitted through
GlobalSignals+0x17c with colour code 0 (the window default; our `ChatKind::System` is a guess).

### `/pet`, `/tower` — `PetCommandIIR_c` (key 6B333303)

Dispatch (`FUN_1003fba6`, `std::string::compare`, case-sensitive): `pet` 0x10041ca8 (string 0x1015d910), `tower` 0x10041df4 (0x1015d8c8). Both: the own pet list
(`dynel+0x1d8` -> `+0x1c`, `FUN_10051fa2` = its size; service towers live in the same list, docs/zone/pets.md) must be non-empty, else `Feedback_YouHaveNoPet` /
`Feedback_YouHaveNoServiceTower`; `strlen(line) > 4`, else `Feedback_InvalidPetcommand` / `Feedback_InvalidTowerCommand`; then `FUN_10053d69(line + 4 | line + 6, own identity,
list<Identity>* out)` returns the command entry `{code, arg, char* text}` (12 bytes, `FUN_10053d17`) or NULL (-> the Invalid feedback). Byte offsets 4 / 6 are fixed, so a bare
`tower` (5 chars) reads past the terminator in the original [UNDEFINED; we treat it as no words]. `code 0x11` (`script`) sends nothing: `sprintf("scripts/%s", text)` +
`FUN_10052230(path, pets, flag = 1 for tower)` queue a pet script (`Local::PetScript`; **UNRESOLVED**: script runner `FUN_10055681` not ported, the hub logs it). Otherwise
`PetCommandIIR_c` ctor 0x10076260 `(window, own identity, entry, pets, tower_byte)` is sent via `SendIIRToObservers` (pass-on flag 0: ctor ends `this[0xc] = 0`).

`FUN_10053d69` (the parser; maps built on first use at 0x10053da5..0x100541e1, `std::map<std::string,int>`, lookups case-sensitive [INFERENCE: default `std::less`]):

1. Words: blanks are only `' '`; a word starting with `"` runs to the next `"` (or the end), another word ends at the next `' '` or `"` (the delimiter is consumed) — `ao_net::n3::textcmd::pet_words`.
2. Last word in **map 1** (command alone): `follow 1, behind 2, survive 3, wait 4, guard 6, attack 7, terminate 10, free 11, heal 12, report 14`.
3. Else, with >= 2 words: the last word is the argument, the one before must be in **map 2**: `cycle 5` (arg `a`... -> 1, `w`... -> 0, first letter via `tolower`, else invalid), `social 9`
   (arg = emote id by `_stricmp`, `FUN_10053c73` over the table 0x1015eb98, = `action::emote_by_name`; unknown -> invalid), `rename 15`, `chat 16`, `script 17` (arg = the entry text, kept <= 255 bytes `FUN_10053ccd`).
4. The remaining words are pet names. None: all pets (empty list) — except `rename`, which pushes `FUN_10058816(own)+0x5c` (the current target [INFERENCE: that helper field is the selected target]).
   First word `all` (`_stricmp`): all pets. Otherwise each name is matched against every pet of the list: `String::StripSpecialChars(x, false)` [Utils 0x1000d764: drops `0x10` + the next char and everything from
   `0x11` to `0x12`] on both, `String::CompareNoCase(.., -1)` [Utils 0x1000de83: `towupper` per code point]; matches are appended (duplicates possible). No match at all -> NULL.

Wire (write slot 8 0x100760b7, read 0x100761a3; the reader accepts `code` 1..=0x10 and `len` <= 250), after the 13-byte N3 header (`u32 key`, own `{0xC350, char}`, `u8 0`):

| type | field |
|---|---|
| i32 | window (`ChatWindowNode+0x1ec`) |
| i32 | code (above) |
| i32 | arg (cycle 0/1, social emote id, else 0) |
| i32 + n x Identity | pets: `(n + 1) * 0x3f1` then the identities (`FUN_1003a527`); n = 0 = all pets |
| i32 | 0 = `/pet`, 1 = `/tower` |
| i32 + bytes | `strlen(text)`, text (rename/chat name or text, empty otherwise) |

`ao_net::n3::textcmd::pet_command`; byte-layout tests `pet_tests`. All `/tower <word>` lines reach here except `create` (and `terminate` with a tower target), which the GUI sends as Fanatic (§Fanatic).

### `/follow` — `FollowTargetIIR_c` (key 260F3671), Gamecode 0x10041f02

Gates in order: target dynel (`FUN_10058e36(target)`, kind 0xC350) missing -> silent. For the own dynel and for the target: `FUN_1003e228(FUN_10058816(dynel)) > 1` (district fight-mode level, see below) or
`Features` (stat 0xE0, `FUN_10044b6e`) bit 0 or bit 26 (`0x4000000`) -> `Feedback_CantFollow`. Own vehicle (`dynel+0x50`) missing, or its `vtbl[0x90]()` false, or the movement FSM mode (`vehicle+0x178` -> `+4`) in
{1, 8, 9, 0xb, 0xc} -> `Feedback_YouCantMove`. Else the IIR is sent (ctor 0x100734f8 at 0x1004206b) and `Feedback "FollowingX"` (`%s` = the target's name, `vtbl[0xe8]+0x34`) is printed.
The ctor arguments are `(own identity, target identity, speed = 2.5 [0x1015d87c], mode 0, position 0,0,0, no path)`, the ctor ends with `EnablePassOn` (pass-on byte **1**). The writer (0x10073030) takes the long
form because the speed is non-zero: `u8 2, u8 mode 0, Identity target, f32 2.5, Vec3 0 0 0, u8 count 0` (`textcmd::follow_target`, test `follow_sends_the_long_form_and_gates`). No local effect: the movement
only happens when the server answers with its own `FollowTargetIIR_c` (docs/zone/misc.md §2).
GM branches: none in `/follow`, `/pet`, `/tower` (the only GM-gated words of `FUN_1003fba6` are listed above). **Unresolved inputs** (hub passes defaults, `play/chat.rs::zone_action`): the district fight-mode level
(`FUN_1003e1d0`, default 2 without `PlayfieldDistrictInfo` data; the hub passes 2 like `Player::follow_gated`, so `/follow` currently answers `Feedback_CantFollow`), the target's `Features` (not tracked, 0),
the meaning of `vtbl[0x90]` of the own vehicle (hub: true) and the FSM mode (`ZoneCmdCtx::move_mode`, 0 = unknown).

### `OrgClientIIR_c` — key `MapToKey` = 7F4B3108

Ctor 0x10126164 `(target, window, code, Identity*, text, flag)`; write 0x10125fef: `u8 code`, `Identity id`, `i32 window`, then `i16 len + text`
for codes {1,7,9,0xd,0x11,0x13,0x14,0x17,0x18,0x19,0x1a,0x1b,0x1c}, or a `u8` flag (0) for code 10. (The ctor's `"&amp;"` -> `"&"` replacement for codes
1/0x1a starts its `find` at `npos` and therefore never runs: dead code in the original.) `id` = the engine's current target (`n3EngineClientAnarchy_t+0x5c`,
[INFERENCE: same as the GUI target]), `{0,0}` for ranks/contract/debt/city.
The word after `org` is looked up in the map at 0x102e2710 (`help` 1, `create` 2, `ranks` 3, `governingform` 4, `info` 5, `promote` 6, `demote` 7, `name` 8, `history` 9,
`description` 10, `objective` 11, `leave` 12, `invite` 13, `disband` 14, `kick` 15, `contract` 16, `tax` 17, `bank` 18, `startvote` 19, `vote` 20, `stopvote` 21,
`debt` 22, `city` 23; case-insensitivity of this `std::map<String,int>` is [INFERENCE]). The rest of the line (after `ws`) must be printable ASCII, else
`CannotUseLettersX` (`%s` = the offending characters). Jump table 0x1004218f (index `value - 2`):

| sub | wire code / text | sub | wire code / text |
|---|---|---|---|
| create | 0x01 rest | kick | rest empty: 0x0c; else 0x0d rest |
| ranks | 0x02 | contract | 0x03 |
| governingform | 0x1b rest | tax | 0x11 rest |
| info | 0x05 | bank | none: 0x12; `add <x>`: 0x13 x; `remove <x>`: 0x14 x; else `OrgCommandHelp` |
| promote | 0x0a (+flag 0) | startvote | rest empty: `OrgVoteHelp`; else 0x07 rest |
| demote | 0x0b | vote | empty: `OrgVoteHelpInfo`; rest == "info": 0x08; else 0x09 rest |
| name | 0x1a rest | stopvote | 0x1c rest |
| history | 0x17 rest | debt | 0x16 |
| description | 0x19 rest | city | 0x1f |
| objective | 0x18 rest | leave / disband | local confirmation dialogs (no wire) |
| invite | 0x0e | unknown / `help` | `OrgCommandHelp` |

### `RaidCmdIIR_c`

Ctor 0x100a3531, write 0x100a34ea: `i32 cmd`, `Identity a`, and only for cmd 5 `Identity b, Identity c`. `move <slot> <group>`: a = `{strtoul(slot), atoi(group)}`
(group > 6 aborts silently); `lootaccess <x> <y> <z>`: a = `{0xC76A, atoi x}`, b = `{atoi y, 0}`, c = `{0xC350, strtoul z}`; create/list/locks: a = 0;
`listlocal` is local (`FUN_100586da` + `FUN_10065f00`).

### Fanatic commands — `FanaticIIR_t` (Fanatic.dll)

`Fanatic::ClientInterface_c::Command(window, target, text)` 0x10001066: header `{0xC350, char}` (control dynel, `vtable+0x1c`), passed-on 0, body
(write 0x10001112): `i32 window, Identity target, i32 len, bytes` (no terminator). GUI handler 0x100b30cf passes `target = InputConfig_t+0xc0` and the line without
its slash, not expanded.

### Other client-side commands handled in `perform` (`play/chat/zonecmd.rs`)

* `/inspect <id>`: `N3Msg_Inspect(Identity{50000,id})` 0x1001dc58 = action 0x105, param 0, idA = the identity, idB 0.
* `/<emote>`, `/emote <name>`, `/anim <name>`: `N3Msg_DoSocialAction(anim)` 0x100269d3 -> `SocialActionCmd_t` (counter `s_nCommandRefCntr++`). Refused (cat 110 keys):
  FSM state 4 (swimming) `Feedback_CantDoSocialActionsWhileSwimming`; vehicle equipped and anim 0x44/0x45 (sleep/lounge) `Feedback_YouCanNotDoThisWithAVehicleEquipped`;
  anim 0x44/0x45 while not sitting/sleeping/lounging (state 8/0xb/0xc) `Feedback_MustSitToLoungeOrSleep`.
* `/played`: GUI 0x100b2321 first prints `Time: %02d:%02d local (%02d:%02d GMT), %02d:%02d game<br>` + `Date: %02d. %s %d local` (colour 0x52; local = `_localtime64`,
  GMT = `_gmtime64`, game clock = `N3Msg_GetCurrentHour/Minute`, month names `Jan..Dec` from 0x101ba9d4), then `N3Msg_TextCommand("played")`.
* `/help <topic>` -> `file://` + the topic's file (0x100b6d56): the file name is returned and the hub resolves `text/help/<file>` (the directory prefix is built in
  `InfoViewModule_c::ShowURL`, not decoded).

## Chat-server requests (`ChatCmd`; GUI senders 0x1016c8c9..0x1016cdd7)

The input line queues an `Action_t` (`FUN_10085a6a(FUN_1002bae5()+0x14, action)`); the chat connection handler (`FUN_10089dfc`, switch at 0x1008a43b on `action+0`)
performs it. Names are resolved by lookup 0x15 first (`ChatCmdFeedback_UnknownUser` otherwise, 0x1008a2c2).

| class | origin | sender | packet | `ChatCmd` |
|---|---|---|---|---|
| 2 / sub 0 | `/tell` (`FUN_100ba2e6`) | 0x1016c8c9 | 0x1e `ISD` | `Tell` |
| 2 / sub 1 | `/cc info <name>` | 0x1016cac2 | 0x6e `IM` (`{"commane": "ccinfo", "destination": "chatserver"}`, key/value roles [GUESS]) | `Forward` |
| 2 / sub 5 | `/name <x>` = tell to the name "name-request"; prints `ChatCmdFeedback_Name_SendingNameChangeRequest` | 0x1016c8c9 | 0x1e `ISD` | `Tell` |
| 2 / sub 4 | `/petition` (position + playfield text, opens `petition_window`) | 0x1016c8c9 | 0x1e | not implemented |
| 3 | `/invite <nick>` (`FUN_100ba6db`) | 0x1016d57d | 0x32 `I`; own id -> `ChatCmdFeedback_Ignore_CantInviteYourself` | `PrivInvite` |
| 4 | `/kick <nick>` | 0x1016c954 | 0x33 `I` | `PrivKick` |
| 5 | `/leave <nick>` | 0x1016c988 | 0x35 `I` (group id = owner id) | `PrivPart` |
| – | accept an invite (0x100a6e0c) | 0x1016c96e | 0x34 `I` (declining sends 0x35) | `PrivJoin` |
| 9 | `/ignore <nick>` | local `IgnoreSystem_t` | own id -> `Ignore_CantIgnoreYourself`; else toggle, `Ignore_(Un)IgnoringCharacter` | hub |
| 10 / 11 | `/lft [text]` (`FUN_100b69b0` -> `FUN_100f01a3`) | 0x1016cafc / 0x1016cb22 | 0x5dc `S` (team description) / 0x5dd (none); prints text.mdb cat 100 `LFTon`/`LFToff` | `LftOn`/`LftOff` |
| – | `/cc <args..>` (`FUN_100ba105` -> 0x1016cadf) | 0x1016cadf | 0x78 `sI`: argument strings (pack code `s`: u16 count + `S`s), window id | `Cc` |

The earlier `ChatCmd::PrivJoin/PrivPart` ids (0x33/0x34) were wrong and are corrected, with `PrivInvite`/`PrivKick` added. `LFTWindowConfig.TeamDesc` (a DValue) is stored locally by
`FUN_100f01a3`; not ported.

## Gaps / guesses

* The GM debug commands, `/petition`, `/rp`, `/reclaim`, `/terminate`, `/command`, `/gfx` are not decoded (see the tables). `/pet`, `/tower`, `/follow` are decoded; open inputs: pet script runner, district fight-mode level, vehicle `vtbl[0x90]`, `rename` default target (see their sections).
* The engine target used by `OrgClientIIR_c` (`+0x5c`) is taken to be the GUI target; the colour of `Feedback_*` lines (code 0 through GlobalSignals+0x17c); case-insensitivity of the
  org/raid sub-word maps; the key/value roles of the `/cc info` forward map.
* The `/<cmd> help` redirect condition and the keys marked `*` in `HELP_TOPICS` (org, pet, chat, team, misc, list, perk, raid share their string with another literal; inferred from chatcommands.html).
* Colour code (0x51 vs 0x52) of the feedback calls whose argument the decompile lost.
* Tell target capitalisation: the client does **no** case folding (`ExpandChatTextArgs` only); the name is resolved on the chat server (lookup 0x15). "You can't send private messages to
  yourself." (0x1008947b) compares ids after the lookup: hub's job.
* A script file fallback passes only its name (`RunScript`); how arguments reach the script is not decoded.
