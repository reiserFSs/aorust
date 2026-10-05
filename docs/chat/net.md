# Chat server protocol (system message 0x43 -> `199.241.136.157:7005`)

Code: `crates/ao-net/src/chat.rs` (codec, `ChatSession`), probe `crates/ao-net/examples/chat_probe.rs`, app side `crates/aomac/src/play/chat/net.rs`.
Captures: `docs/captures/chat_login_ithaca.rec` (live login of "Aomacvolk", username redacted, login key redacted).

## Where the client does it

* The zone server sends system message **0x43** right after the 0x7F control frame (docs/zone/misc.md §15): `i32 n; n x { i32 len; host; i32 port; f32 }`
  (`199.241.136.157`, `7005`, `0.0`). `Client_t::ProcessMessage` [IF 0x10002a9e] emits GlobalSignals+0x1c8; the consumer is
  `ChatGUIModule_c` [GUI 0x10089dfc]: if it has no connection yet and `N3InterfaceModule::GetClientInst()` (own character id) is valid it creates
  a `ppj::Client_c` (0x160 bytes, ctor `FUN_1016cefb(host, port, char_id, cPlayerName, cPlayerPasswd)` [GUI 0x1016cefb]) and calls `Connect` [0x1016d36a].
  So the **chat login uses the same account name and password as the login server**, kept in the exported globals `cPlayerName`/`cPlayerPasswd`.
* `ppj::Client_c` is statically linked into GUI.dll (0x1016c000..0x10173bff, with GMP, `rand()`-seeded). Socket calls are WinSock ordinals
  (23 socket, 4 connect, 16 recv, 19 send, 10 ioctlsocket FIONBIO). Non-blocking; poll function `FUN_1016f6cb`.
* Reconnect pacing in `ChatGUIModule_c` [0x10089dfc]: wait `1 << (attempt + 12)` ms (attempt < 3, else 0x8000 ms) between attempts; "Lost connection to chat
  server. Attempts to reconnect... (GM msg only)" is only printed for GMs (`InputConfig_t::CheckMode(0x38)`). `ChatNet` copies the pacing (max 10 attempts is our own bound).
* Keep-alive [0x1016f6cb]: > 59 s without receive -> send ping (type 100, `D` = `{2}`) every 30 s; > 299 s without receive -> drop.

## Framing and field codes

Packet: `u16 type, u16 length, payload` big endian (parse loop `FUN_1016f577`: recv 0x800 chunks -> `FUN_10171a0f` splits -> `FUN_1016d6dc` dispatches).
Field codes (pack `FUN_1017161f`, unpack `FUN_10171ae5`):

| code | wire |
|---|---|
| `I` | u32 |
| `S`, `D` | u16 length + bytes (`D` = the message attribute/data block, kept as bytes) |
| `G` | u8 kind + u32 id (group id; the client keeps it as `(kind << 32) | id`) |
| `B` | u8 |
| `M` | u8 count, entries `{ u8 (keylen << 4 | len_hi), u8 len_lo, key, value }` (forward data, 0x6e) |
| `i` / `s` | u16 count + u32 list / string list |

## Login (verified live, 2026-10-05)

1. Server: type 0 `S` = 32 **raw random bytes** (a C string for the client: cut at the first NUL).
2. Client (state 2 -> 3) [GUI 0x1016db52]: `key = "<dhX hex>-<TEA-CBC hex>"` = exactly `ao_net::crypto::make_challenge_response` with the
   login-server public key (string 0x101ca928 = `90b8ce5f…`, prime 0x101ca820 = AOChat `$dhN`, g = 5, 128-bit exponent), plaintext `"%s|%s|%s"` = `user|seed|password`
   (GUI 0x10173360 / 0x1017349c / 0x10173613). Sent as **type 0 `IISS` = (0, char_id, user, key)** when a character id is known
   (`charid != -1`); type 1 `ISSS` = (0, "", user, key) otherwise (charlist flavour, unused here).
3. Server: type 5 (LOGIN_OK), type 6 on error. Then pushes `0x14 USER_NAME` (own id/name) and the message of the day as three anonymous vicinity
   messages (type 0x23, empty name): "Welcome to Project Rubi-Ka! It is …", "Remember: Usage of Automation Software …", "If you encounter any bugs, please use the .bug command …".
   Standalone login (without being in the zone) is accepted.

## Server -> client (`FUN_1016d6dc` switch; names are the client's own error strings)

| type | name | fields |
|---|---|---|
| 0x00 | S2C_LOGIN_CHALLENGE | `S` |
| 0x05 / 0x06 | S2C_LOGIN_OK / S2C_LOGIN_ERROR | – |
| 0x14 | S2C_USER_NAME | `IS` id, name |
| 0x15 | S2C_LOOKUP_NAME_RES | `IS` id (-1 unknown), name |
| 0x16 | S2C_USER_FLAGS | `IB` |
| 0x1e | S2C_MESSAGE (tell) | `ISD` sender, text, data |
| 0x22 / 0x23 | S2C_VIS_MESSAGE_FMT / S2C_VIS_ANON_MESSAGE | `SSD` name, text, data (0x23: name dropped) |
| 0x24 | S2C_SYS_MESSAGE | `S` |
| 0x25 | S2C_SYS_MESSAGE_LOCAL_FMT | `IIID`: sender, kind, text id (LDB category 20000), type string (`I`/`S`/`l`) followed by the arguments in packet order; `l` = u32 text id |
| 0x28 / 0x29 | S2C_ADD_BUDDY / S2C_REM_BUDDY | `IID` / `I` |
| 0x32 / 0x33 | PRIVGRP_INVITED / KICKED | `I` |
| 0x37 / 0x38 / 0x3a | PRIVGRP_JOINED / PARTED / DECLINED | `II` |
| 0x39 | PRIVGRP_MESSAGE | `IISD` |
| 0x3c | S2C_GROUP_JOIN | `GSID` group, name, flags, data |
| 0x3d | S2C_GROUP_PART | `G` |
| 0x41 | S2C_GROUP_MESSAGE | `GISD` group, sender, text, data |
| 0x64 | S2C_PONG | `D` |
| 0x6e | S2C_FORWARD_DATA | `IM` |
| 0x44c, 0x5dd | LFT query / reply | not decoded |
| 0x3f2.. | S2C_ADM_MUX_INFO | `BISIIBBS` not decoded |

## Client -> server (senders GUI 0x1016c8c9..0x1016cdab)

`0x15 S` lookup name; `0x1e ISD` tell; `0x28 ID` / `0x29 I` buddy add / remove; `0x33 I` / `0x34 I` / `0x35 I` private group join / leave / …;
`0x39 ISD` private group message; `0x40 GID` group flags; `0x41 GSD` group message (`FUN_1016c9f4` picks 0x39 when the group kind is 0xE);
`0x46 IG`, `0x47 IIII`, `0x578 IS`, `0x579 S`, `0x5dc S`, `0x5dd (empty)`, `0x5de IIII`, `0x3e9..0x406` (LFT / mail / misc, not implemented).
`D` of outgoing text is `(ptr, len)` supplied by the GUI; we send a single NUL like AOChat [UNVERIFIED which the original sends].

## Message data block (`D`)

`HandleVicinityMessage` [GUI 0x10086728] reads `data[0]` = message kind (4 shout, 5 whisper, 6 / 7 special groups, else vicinity) and passes the rest to
`FUN_10085b4a` (macro/flags). Group id -> window: local ids `0x40000002` (vicinity), `0x41000000` (kind 4), `0x41000001` (kind 5), `0x4200001b` (6), `0x4200001a` (7).
`GetGroupIdentifier` [GUI 0x1001b5d5] formats a group id as `#%016I64x#`.

## Unresolved

* Group announcements (0x3c) were **not** seen in the standalone login (12 s); expected once the character is in the zone (see live notes in docs/chat/live.md).
* Exact content of the outgoing `D` block, and the reduction applied to incoming text (`RemoteFormat::ParseString`, `HTMLParser_c::ExtractText`).
