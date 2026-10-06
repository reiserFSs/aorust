# Zone-server chat (N3 text in, ptype-5 text out)

Code: `crates/ao-net/src/n3/chat.rs` (wire), `crates/aomac/src/play/chat/zone.rs` (lines, `ldb`, outgoing frames).
Addresses: `[GC]` Gamecode.dll, `[GUI]` GUI.dll, `[IF]` Interfaces.dll, `[ldb]` ldb.dll (image base 0x10000000; ldb.dll was imported into a scratch Ghidra project for this).

**Test data.** None of the three incoming classes is in `docs/captures/*.rec` (no chat was recorded; counted by message type over all five
recordings). The layouts are therefore derived from the classes' `ReadSubClass`/`WriteSubClass` code only; unit tests encode/decode round-trip and use
hand-built bytes checked against the read order. `every_captured_message_decodes` is unaffected (the new decoder only claims its three ids).

## 1. How text reaches the chat windows

There is one sink: `FUN_10012b05` [GC] (a method of the singleton made by `FUN_10012a1e`, which also hooks the `3rdPersonCamera` pref) emits the
`GlobalSignals_c` signal at `+0x17c` with `(uint channel, std::string& text, uint colorCode)`. Its slot in every chat window is `FUN_10083898` [GUI]
(connected in `FUN_10083e53`, disconnected in `FUN_100846c1`):

* `channel < 0x40000000`: the window whose id at `window+0x1ec` equals `channel` (`FUN_10093b33`) prints it (`FUN_1009b37f`);
* otherwise `ChatGUIModule_c::GetGroupIdentifier(channel, "")` (`"#%016I64x#"`, GUI 0x1001b5d5) finds the group; unknown group -> group `0x40000001`.
  Group `0x40000002` is the vicinity group (see §4);
* then `FUN_10084f9e` applies the `ChatFilterEnabled` / `ChatFilterRules` regexp filter and signals the windows.

The printing function `FUN_1009b37f` [GUI] builds `"<div>" [+ "<font color=" + ColorCodeToHTMLColor(code) + ">" if code != 0] + ExpandChatTextArgs(text)
[+ "</font>"] + "</div>"`, so **colour code 0 means "no `<font>`"**. `ColorCodeToHTMLColor` [GUI 0x10087860] maps the code through the table at
`0x10268d38` (`{code, name}` x45, the `CC*` names of `TextColors.xml`; unknown code -> `white`): see `zone::color_code_name`
(0 CCNoneColor, 1 CCMenubarColor, 2 CCWhisperColor, 3 CCShoutColor, 4 CCTellColor, 5 CCVicinityColor, 6 CCCommColor, 7 CCTeamColor, 8 CCClanColor,
9 CCEmoteColor, 10 CCLinkColor, 11 CCToolTipColor, 12 CCRed, 13 CCGreen, 14 CCBlue, 15 CCWhite, 16 CCYellow, 17 CCCashColor, 18..20 CCInfoHeadline/Header/Text,
21..35 the combat/skill colours, 50..55 CCAdmin/Misc/GM/SeekingTeam/Newbie/News, 80 CCInfoTextBolb, 81/82 CCChatCmdFeedbackError/Info).

`ChatGUIModule_c::ExpandChatTextArgs` [GUI 0x1008619b] runs on **every line a window prints** and on **the text the input commands send**:
`%%` -> `%`, `%t` -> name of the selected target (`InputConfig_t+0xc0/c4`; `&lt;no target&gt;` if 0:0), `%f` -> name of the dynel we fight
(`N3Msg_GetAttackingID`; `&lt;no fighting target&gt;`), `%m` -> name of the dynel under the mouse (`+0xd8/dc`); any other `%x` stays. Implemented as
`zone::expand_chat_text_args`. **`ChatLine.text` produced by `zone::lines` is the text *before* this step**; whoever renders (ChatGui) must apply it
(or call the function) when it prints, and the input path must apply it before sending (`FUN_1009caf6` does).

## 2. The three N3 classes that feed it

Header as for every IIR (`u32 key`, `Identity`, `u8 flag`, docs/zone.md §2). Keys = `MapToKey(class)` (unit-tested).

| key | class | ctor / read / write / apply [GC] |
|---|---|---|
| `5F4B442A` | `ChatTextIIR_t` | `0x1003864f` / `0x1003871a` / `0x10038608` / `0x10038677` |
| `50544D19` | `FeedbackIIR_t` | `0x10072dff` / `0x10072e1d` / (inherited) / `0x10072e81` |
| `206B4B73` | `FormatFeedbackIIR_t` | `0x100391ff` / `0x100392ff` / `0x10039289` / `0x10039341` |

(vtable slots: 2 = apply, 7 = read, 8 = write.)

**ChatTextIIR_t** body: `u16 len` + text (`FUN_100388e2`; `len >= 0x8000` marks the stream bad), `u8 color` (object `+0x38`), `u8 screen` (`+0x3c`),
`u32 channel` (`+0x18`). The read function stores `color`/`screen` only when `screen < 2` (decoder: error otherwise).
Apply: `screen == 1` -> the raw text goes to AFCM message `0x19` (`FUN_100044e1`: `AFCM::Send(0x19, 4, text)`, `SetData(0, 0xf)`, three floats
6.0/0.5/0.57 from `0x1015d0a0`), the handler of which is `RenderTextModule_t` [GUI 0x1004b335]: on-screen text, **not** a chat line. Otherwise
`RemoteFormat::ParseString(text)` (§3) is emitted as `(channel, text, color)`.

**FeedbackIIR_t** body: `i32 channel` (`+0x18`), `i32 category` (`+0x1c`), `i32 id` (`+0x20`). Apply: `LDBface::GetText(category, id)` (asm at 0x10072e9c:
`PUSH [esi+0x20]; PUSH [esi+0x1c]`) emitted as `(channel, text, 0)`: always colour 0 (no `<font>`).

**FormatFeedbackIIR_t** body: `i32 channel` (`+0x18`), `u16 len` + `RemoteFormat` dump (`+0x20`), `i32 mode` (`+0x3c`). Apply: only if the header dynel
resolves to a `SimpleChar_t` whose byte `+0x80` is 0 (same test as in `FUN_10038c4b`); then `FUN_1005aaa9(mode, channel, remote)` [GC]:
`mode == 1` -> signal `GlobalSignals_c+0x1d8(string)` via `FUN_1001505a` (on-screen; consumer not identified); `mode == 2` -> the same AFCM `0x19`
on-screen text as above; any other mode -> chat `(channel, ParseString(dump), 0x10 = CCYellow)`.

`zone::lines(&N3Chat, &ZoneChatCtx) -> Vec<ChatLine>` (kind = `ChatKind::Other(color_code_name(code))`; code 0 -> `Other("CCNoneColor")`, which the
renderer must draw **without** a font colour), `zone::routed` additionally returns the `channel`, `zone::screen_text` the on-screen variants.
`ZoneChatCtx.header_is_char` must be set by the caller from the N3 header (the `SimpleChar_t`/`+0x80` condition above).

Other client text producers that go through `FUN_10012b05` (about 70 call sites: slash-command feedback `FUN_1003fba6`, trade, teleport, item-use errors,
combat-log producers ...) are *client-side* texts or other IIR classes (combat log: separate agent); only the three classes above carry text as such.
**Vicinity / shout / whisper from other players is not delivered through any of them** (§5).

## 3. `RemoteFormat` / `LDBformat` (ldb.dll)

A `FormatFeedbackIIR_t` dump and any `~&...~` inside a `ChatTextIIR_t` text is a `RemoteFormat`:

```
"~&" b85(category) b85(id)           12 bytes   (RemoteFormat(uint,uint) [ldb 0x1000428e]; (uint,char*) hashes the key with ElfHash)
  'i' b85  int      'u' b85  uint     'f' b85  float bits           each 6 bytes
  'R' b85(cat) b85(id)                text-db entry fed as its text      11 bytes
  's' <len+1 as one UTF-8 code point> bytes     string (may itself be a blob; fed expanded if it is, else raw)
  'F' <len+1 code point> bytes                  nested blob, always fed expanded (empty if not a blob)
'~'                                              MarkEnd [ldb 0x10004192]
```

`b85` = `RemoteFormat::ToBase85/FromBase85` [ldb 0x1000410e/0x1000415d]: 5 chars, `'!' + digit`, most significant first (digits 0..84). The length of
`'s'`/`'F'` args is `FUN_1000403f` (UTF-8 decode of 1..4 bytes; lead byte `0x92` reads as `0x27`; invalid sequence -> the lead byte). `Init`
[ldb 0x10005565] needs `len >= 12`, `~&`, both base-85 words valid and a terminating `~` (`*consumed = index+1`); anything else (bad char, no `~`)
leaves `consumed == 0` and `ParseString` [ldb 0x1000593d] copies the `~` literally and goes on with the next byte. `ao_net::n3::chat::parse_remote`
/ `encode_remote` implement exactly this.

The template is `LDBface::GetText(category, id)` (`ao_formats::screens::TextDb::by_id`; missing entry -> `no LDBintern (cat:id)`, string at
[ldb 0x10001622]); text-db entries that start with NUL are reference lists (`InsertReference`) -- only one such entry exists in `text.mdb` (700/45), ignored.
The args are fed to `LDBformat` [ldb 0x100054da] (`zone::ldb::Format`, a port of `Init` 0x100053f2, `CreateToken` 0x1000535a, `Feed*` 0x10004c8b..,
`FeedInternal` 0x100046ab, `Dump` 0x10004885):

* `%[flags][width][.prec]conv` (flag set `-+ 0#123456789.hlL`) consumes the next argument; `%%` is a literal `%`.
* `Feed(int)` prints through `snprintf(spec, v)`; `%s` with a number prints `int_value<v>`, with a float `float_value<%f>`.
* `#N{alt|alt|...}` is a choice token for argument N. Each alternative is `flags` (letters `A-Z` = bit 0-25, digits `0-5` = bit 26-31) up to `:` or a blank,
  then the text up to `|`/`}`. `Feed(int)` gives flags `0x4000000` (value 0) or `0x8000000` (value 1), otherwise 0; strings/floats 0. The alternative
  with the most common bits wins, **the later one on ties**; an alternative that has no `:`/blank terminator aborts the choice and leaves the token raw.
  So `"#1{1: item| items}"` pluralises, but the shipped `"Removing %d #1{ 1:buddy | buddies }."` (text-db 20000/18838393, blank after `{`) always picks the last
  alternative, whose trailing blank survives `Dump` ("Removing 1 buddies .") -- reproduced as is (original data quirk). `"#1:{1: credit was| credits were}"` (1000/743316) with `:` before `{`
  goes down the `':'` branch of `Init` and prints the braces literally; also reproduced as decompiled.
* `##` is a literal `#`.
* `Dump` joins all tokens, drops leading blanks and collapses runs of blanks into one.

**Unresolved**: `Feed(uint)` / `Feed(std::string)` were not decompiled separately (assumed to mirror `Feed(int)` / `Feed(char const*)`; the `%s`
placeholder for `uint` is a guess `uint_value<v>`). The flag-bit table at `0x1000b1f8` was only read for entries 0..32 (taken as `1 << index`, entry 32 = 0).
`strchr(spec_chars, 0)` in `Init` would run past a template that ends inside a `%` spec; the port stops at the end.

## 4. Outgoing vicinity / shout / whisper

Who sends: **`FUN_100891a4`** [GUI 0x100891a4] -- called from the chat-server queue pump `FUN_10089dfc` [GUI] (`ChatServerInterface_c`, singleton
`FUN_1002bae5`), case `msg.type == 0` of the queued message. Producers: `FUN_10085538` [GUI] (called by the windows' send paths) queues
`type = 0` when the destination group id is `0x40000002` (the vicinity group), else `type = 1` (chat-server group message); fields: `+4 kind`, `+0x48/+0x4c` group
id, `+0x50` text (`std::string`, length `+0x60`), `+0x6c` an extras block `{int a, int b, string S1, string S2}` (a `TextMacro_t`, copied by `FUN_10085876`).

The slash-command handler `FUN_1009caf6` [GUI] (registered by `FUN_1009dc28` for `/v /say /w /whisper /s /shout /me /script`) selects the **kind**:
`/whisper`, `/w` -> 1; `/shout`, `/s` -> 2; `/me` -> 3; everything else (also `/say`, `/v`, `/script`) -> 0; it calls
`FUN_10085538(window id, ExpandChatTextArgs(message), kind, TextMacro)` with the vicinity group. Without arguments it prints `Usage: <cmd> &lt;message&gt;`
(colour `0x51`). The `ChatWarnWhenSpeakingToUnsubGroups` dvalue and `FUN_1009985f` only add a warning line (text id 0x2711).

`FUN_100891a4(msg)`:

```
if len(text) > 0x400: return                      // silently not sent
buf = u16 BE len | text bytes | u8 msg.kind        // NO NUL; len counted without it
buf += BBBSS(1, a, b, S1, S2)  only if S2 != ""    // FUN_10089163 -> FUN_1017161f packer, B = byte, S = u16 BE len + bytes
id   = msg.kind==1 ? 0x170 : msg.kind==2 ? 0x129 : 0x16d        // kind 0 and 3 -> vicinity
AFCM::Send(0x18, id, buf, total); SetData(Identity{InputConfig_t+0xc0, +0xc4}); SetData(total)
if pref "ChatIndicator": N3Msg_FlashHead(own char, InputConfig_t+0xd8/dc)
```

The `NetworkModule_t` handlers (§6 of docs/zone/outgoing.md; `VicinityTextMessage` 0x100076b4 ...) read `(Identity, uint len, char* buf)` and
`Client_t::Send*Message` writes `identity, i32 len, buf` into the `TextMessage_t` (type 3 vicinity / 4 shout / 2 whisper). So on the wire:

| payload off | field |
|---|---|
| 0 | `u32` type (3 / 4 / 2) |
| 4 | `i32,i32` identity = **the currently selected target** (`InputConfig_t+0xc0/+0xc4`), `{0,0}` if nothing is selected -- for all three types |
| 12 | `i32` total = `2 + textlen + 1 [+ extras]` |
| 16 | `u16` textlen, text bytes (no NUL), `u8` kind (0 say / 1 whisper / 2 shout / 3 emote) [, extras] |

Resolved questions from docs/zone/outgoing.md §6: (a) the identity is the selection (it is what `%t` expands, `InputConfig_t+0xc0`), whisper has **no name
target**: it goes to whoever is selected; (b) no NUL is counted or sent; `len` is the length of the whole `u16 | text | kind` buffer.
`outgoing::text_payload` is unchanged; `chat::chat_buffer` builds the buffer and `zone::{vicinity,shout,whisper}_frame` / `zone::frame(Speech::Emote ..)` the frame
(all return `None` for text over 1024 bytes).

**Unresolved / guesses**
* The extras block (`BBBSS`) is present when the queued message's `TextMacro` has a second string. Producer found: `/voice` (`FUN_100b82c2`): `a` = breed (stat 4),
  `b` = sex (stat 0x3b), `S1` = voice fx type ("simple" / "distunguished" / "cool" / "military"), `S2` = sound name (docs/chat/dialogs.md §6, `voice::extras`); plain typing never sets it.
* Text encoding of the typed text: Latin-1 when every char fits else UTF-8 (`zone::text_bytes`) -- [GUESS]; the original passes the bytes of a `std::string`.
* `/script` is registered on the same handler `FUN_1009caf6` (kind stays 0); whether another handler consumes it first was not checked.
* What typing plain text in a non-vicinity window does (group message to the chat server) is the lead's `net.rs` path (`type = 1`), not covered here.

## 5. Who delivers other players' vicinity / shout / whisper

The original client has **no ptype-5 receive path** (docs/zone/misc.md §14) and the three classes above carry only server/system texts. Text from other players
is delivered by the chat server: `ChatGUIModule_c::HandleVicinityMessage` [GUI 0x10086728] (chat-server message 0x22), i.e. docs/chat/net.md, not this module.
[INFERENCE] The zone server relays the ptype-5 message to the chat server, which fans it out.

## 6. Open items

* `SimpleChar_t+0x80` (condition for `FormatFeedbackIIR_t`): meaning unknown (`header_is_char` is the caller's decision).
* Consumer of signal `GlobalSignals_c+0x1d8` (FormatFeedback mode 1) and the overlay behaviour of AFCM `0x19` (`RenderTextModule_t`).
* Real traffic: no capture contains these messages; ids/colour codes the server actually uses (and the channel values) are unverified.
