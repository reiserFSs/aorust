# Anarchy Online client <-> login server protocol (login -> character select -> zone hand-off)

Scope: everything between "client opens TCP to the login server" and "client opens TCP to the zone
server and presents its cookies", for the **PRK client 00.7.2_EP1** (`version.id`, `patch.version` = `0.7.2`).
Implemented as pure encode/decode in `crates/ao-net` (no sockets). **Nothing here was captured from a live
server** — every claim is from client disassembly (Ghidra 12.1.4 decompiler) or from CellAO / AOChat
sources. Evidence tags:

| tag | meaning |
|---|---|
| `[IF 0x...]` | `Interfaces.dll` image VA (base `0x10000000`) |
| `[CN 0x...]` | `Connection.dll` VA |
| `[MP 0x...]` | `MessageProtocol.dll` VA |
| `[BS 0x...]` | `BinaryStream.dll` VA |
| `[DC 0x...]` | `DatabaseController.dll` VA |
| `[AO 0x...]` | `AnarchyOnline.exe` VA (base `0x400000`), `[LA 0x...]` = `Anarchy.exe` |
| `[CellAO file:line]` | `gitlab.com/CellAO/CellAO` @ `e7c1a93` (BSD-3), path relative to `CellAO/` (inner dir) |
| `[AOChat line]` | Budabot `core/AOChat.class.php` (GPLv2, Auno) — read only to cross-check the algorithm; no code copied |
| `[PRK file:line]` | decompiled PRK launcher `PRK.Launcher.Lib/GameClientService.cs` |
| `[inference]` | not directly observed |

Analysed binaries (SHA-256 of the files as imported, equal to the files on disk when this was written):
`Connection.dll 69b116bf…a29007`, `MessageProtocol.dll bd1c4a54…89394`, `Interfaces.dll 8995f83e…d2105`,
`BinaryStream.dll b90daf1c…c8e33`, `DatabaseController.dll 513040ab…b276`, `Gamecode.dll 2c349e73…a9201`,
`AnarchyOnline.exe b4afb033…8ced`, `Anarchy.exe f4128054…c8027`.
Ghidra projects: `/tmp/aomac-ghidra/proto/` (not in repo).

## 1. Endpoint

* The PRK launcher starts `AnarchyOnline.exe` with the argument string `IA <u32> IP <port> UI`, where `<u32>` =
  `BitConverter.ToUInt32(IPAddress.GetAddressBytes())`, i.e. the four address bytes read as a **little-endian
  u32** (199.241.136.157 -> `0x9D88F1C7` = 2642440647) `[PRK GameClientService.cs:211-219]`.
* `AnarchyOnline.exe` parses it with `sscanf(cmdline, "IA%u IP%hu DU%s", &ip, &port, dimensionUrl)` and requires
  >= 2 matches `[AO 0x405f33]`; `%u`/`%hu` skip the space, and `DU%s` simply fails to match `UI` (the third field is
  optional). It stores `Client_t::s_nLHIPAddr`/`s_nLHPort` (LH = login host). `Anarchy.exe` (the stock launcher)
  writes the same `IA`/`IP`/`DU` triple to `nolaunch.arg` `[LA 0x4035db]`.
* `Client_t::ConnectToLH` `[IF 0x1000297a]` passes `htonl(ip)` into `ACE_INET_Addr::set(port, ip, encode=1)`, which swaps
  again, so the socket address bytes are the in-memory bytes of the `IA` value (a.b.c.d in
  network order). Plain TCP, no TLS, no pre-handshake (`TcpHandler_t::handle_input` -> `Connection_t::Receive`
  `[CN 0x100070e8]`).
* **The client speaks first.** `ConnectToLH` -> `OpenConnection` `[IF 0x10002861]` -> `InitAuth` `[IF 0x100025a9]`
  immediately sends `UserLogin`. The server sends nothing unsolicited.
* Username/password are *not* on the command line; they come from the in-client login UI
  (`Client_t::s_cPlayerName` / `s_cPlayerPasswd`, used in `MakeChallengeResponse` `[IF 0x10002054]`).

## 2. Transport framing

All integers **big-endian** on the wire. `BinaryStream` defaults to big-endian stream / little-endian machine, so
every `<<`/`>>` of int/short byte-swaps `[BS 0x10001b88 Init: machine=0, stream=1]`, `[BS 0x10001237 operator<<(int)]`,
`[BS 0x1000193f operator>>(int&)]`.

Header (16 bytes) `[CN 0x100016f3 Connection_t::Send]`, `[MP 0x10001da7 Message_t::CreateDataBlock]`,
`[MP 0x10001cfc Message_t ctor from wire]`, `[CN 0x100019ba Connection_t::Receive]`:

```
off  size  field
0    u16   seq        sender's counter. Send pre-increments (first frame = 1), 0xFFFF wraps to 0 then 1.
2    u16   ptype      1 system, 5 text, 10 (0xA) N3, 0xB ping, 0xE operator, 0x7F compression control
4    u16   version    always 1 on send (htons(1)); not validated on receive
6    u16   size       16 + payload length, *unpadded*
8    u32   sender
12   u32   receiver
16   ...   payload    zero-padded on the wire to a multiple of 4 (not when compression is active)
```

* Per-ptype fixed header incl. type-specific part (`Message_t::HeaderSize` `[MP 0x10001cd1]`): system/text/operator
  `0x14` (= 16 + `u32` message type), N3 `0x10`, ping `0x28`. A system message whose `size` < 0x14 is rejected
  (`SystemMessage_t(size,data)` `[MP 0x10002c43]`, `Message_t` error 1); ptype mismatch is error 2.
* Receive validation: `size` must be in `[0x10, 0x20000]` (field is u16, so effectively `<= 0xFFFF`); `seq` must be
  strictly greater than the previous non-0x7F frame, except after `0xFFFF` where any non-zero value is accepted;
  type `0x7F` with `seq == 0xDFDF` is exempt. Violation -> `Receive` returns -1 and the connection is dropped
  `[CN 0x100019ba]`. **The server's first frame must have `seq >= 1`.** CellAO starts at 1 `[CellAO Server/LoginEngine/CoreClient/Client.cs:73,175-177]`.
* Buffering is stream-oriented: header (16 bytes) is accumulated first, then `padded(size)-16` more bytes; frames may
  be split/coalesced arbitrarily across TCP reads.
* Type `0x7F` (CellAO `InitiateCompressionMessage`): `receiver == 0` -> header bytes 8..10 / 10..12 are raw compression
  parameters passed to `SetCompressionReceive`/`SetCompression`; `receiver != 0` -> the client re-targets an `ACE_INET_Addr`
  (ip = `receiver`, port = BE u16 at bytes 8..10) `[CN 0x100019ba]`. Not needed for login; the compressed framing is **not** reverse-engineered here.
* Header `sender`/`receiver` as sent by the original: client -> login server `(0, 1)` for UserLogin/UserCredentials/
  SelectCharacter/DeleteCharacter/CreateCharacter; ZoneLogin `(charId, 2)` (`SystemMessage_t(type, sender, receiver, size, data)`
  ctor `[MP 0x10002d20]`; call sites `InitAuth 0x100025a9`, `AuthClient 0x10001b65`, `LoginCharacter 0x10001c5b`, `SendClientCookie 0x100013b0`). The client does not validate the
  values of server frames (it only stores `sender` in `Client_t+0xac`, `ProcessMessage` `[IF 0x10002a9e]`); CellAO
  sends `sender=1` and arbitrary `receiver`s (0x2B3F, 0x615B, 0x1F83, 0xFFFF) `[CellAO Server/LoginEngine/CoreClient/Client.cs:162-170]`.
* System message payload = `u32 msgType` + body. Dispatch is on `msgType` in `Client_t::ProcessMessage` `[IF 0x10002a9e]`
  (anything not handled there goes to `InvalidResponseHandle` `[IF 0x10001e06]`).

### Primitive encodings (`BinaryStream`)

| type | wire |
|---|---|
| int / uint | 4 bytes BE |
| short | 2 bytes BE |
| fixed string `N` | `strncpy` into a zeroed `N`-byte field (`InitAuth`, `AuthClient`) |
| string (i32) | `i32 len` + bytes, no NUL (CharacterInfo_c name/area/ban reason, ChatServerInfo host) |
| string (i16) | `i16 len` + bytes (`FUN_10004a34` `[IF 0x10004a34]`, SuggestName 0x56) |
| Identity | `i32 type`, `i32 instance` `[MP 0x10003412]` |
| IPv4 | 4 raw bytes in network order (read as a BE int -> host-order `ACE_INET_Addr::set(port, ip, encode=1)`) |

## 3. Session flow

```
C                                            S (login server)
|--- TCP connect ---------------------------->|
|--- 0x22 UserLogin(name, version.id) ------->|   [IF 0x100025a9 InitAuth]
|<-- 0x24 ServerSalt(32 raw bytes) -----------|   [IF 0x10002a9e case 0x24]
|--- 0x25 UserCredentials(name, DH+TEA) ----->|   [IF 0x10001b65 AuthClient]
|<-- 0x0E CharacterList ----------------------|   [IF 0x10002a9e case 0x0E]     (or 0x0D LoginError, then close)
|   (UI)                                      |
|--- 0x16 SelectCharacter(charId) ----------->|   [IF 0x10001c5b LoginCharacter]  (0x14 DeleteCharacter, 0x0F CreateCharacter optional)
|<-- 0x17 ZoneInfo(charId, ip, port, cookies)-|   [IF 0x10002a9e case 0x17]       (or 0x0D LoginError)
|--- TCP connect ip:port (zone server) ------>|   [IF 0x1000291d RedirectToServer]
|--- 0x1B ZoneLogin(charId, cookie1, cookie2)->   [IF 0x100013b0 SendClientCookie, triggered by NetworkModule_t::N3ActivatedMessage 0x100077a7]
```

* On zone-hand-off the client stores `s_nServerIPAddr/s_nServerPort/s_nCharID/s_nCookie1/s_nCookie2/s_nPlayerID/
  s_nEventServerType` (statics) and signals the game layer; `Client_t::RedirectToServer` (state 4, new
  `TcpConnection_t`, start `PingManager`) then connects to the zone server `[IF 0x1000291d]`. `SendClientCookie` is
  called from `NetworkModule_t::N3ActivatedMessage` `[IF 0x100077a7]`, i.e. once the N3 engine reports "activated"
  — the exact moment relative to the TCP connect is `[inference]` (call chain through a signal/slot table at
  `0x10027148`/`0x10027168` was not resolved).
* The same `SendClientCookie` is used for **in-session zone changes** after message `0x3C ZoneRedirection`
  (new ip/port only; the cookies are the ones stored from the previous `ZoneInfo`) `[IF 0x10002a9e case 0x3C]`.
* Zone-server traffic after `ZoneLogin` (N3 messages, ping manager) is out of scope.
* `Client_t` connection states used here: 1 = login host connected `[IF 0x1000297a]`, 4 = redirected `[IF 0x1000291d]`.

## 4. Login crypto (`UserCredentials` response)

Flow `[IF 0x10002a9e case 0x24]` -> `Client_t::MakeChallengeResponse(salt)` `[IF 0x10002054]` -> `FUN_100129a1`
`[IF 0x100129a1]` -> `AuthClient` `[IF 0x10001b65]`.

1. **Salt.** The 32 salt bytes are read raw, a NUL is appended at index 32 and the buffer is handled as a C string —
   an embedded `0x00` truncates the salt. CellAO therefore replaces `0x00` salt bytes with `42`
   `[CellAO Server/LoginEngine/MessageHandlers/UserLoginHandler.cs:78-86]`.
2. **Half Diffie-Hellman** (GMP `mpz_powm`; mpir.dll). `x` = 128-bit random (16 `rand()>>8` bytes printed `%.2X`,
   `FUN_1001283d`/`FUN_1001295a` `[IF 0x1001283d,0x1001295a]`).
   * `N` = 1024-bit prime `eca2e8c8…f5e2a6f` (256 hex), file offset `0x16988` of `Interfaces.dll`, pointer `0x100328e0`.
     Identical to AOChat `$dhN` and CellAO's `Prime` `[AOChat 682]`, `[CellAO Libraries/Source/CellAO.Core/Encryption/LoginEncryption.cs:98-102]`.
   * `G` = `5` (`.data` `0x100328e4`). `dhX = G^x mod N` (`FUN_100128bc`), `K = Y^x mod N` (`FUN_1001279e`).
   * `Y` (login server public key) = `90b8ce5fe64f466678a7a8589023be89c64e34358b0e0165cd5e381c75f3f5e7…eb2ec7ffa5fba24ecbbb8ae1`
     (256 hex; file offset `0x16a90`, pointer `0x100328dc`). It is **not** AOChat's `9c32cc23…` and not derivable
     from CellAO's private key (`5^0x7ad852c6… mod N` = `0x26b5a3b4…`, checked) — a server must hold the private half
     for this `Y`. Full strings are in `ao_net::crypto`.
   * Hex rendering is lowercase, no leading zeros (`mpz_get_str`, base 16).
3. **Key.** First 32 hex chars of `hex(K)` -> 16 bytes (`sscanf("%2x")`); if `hex(K)` has < 32 chars the client prints
   "input key too short." and fails (a 1024-bit secret essentially never has fewer than 32 hex digits). The 16 bytes are used as
   four **little-endian** u32 key words `k0..k3` `[IF 0x10012adc]`.
4. **Plaintext** `S = name || '|' || salt(32 raw bytes) || '|' || password` (`sprintf("%s|%s|%s")` `[IF 0x100129a1]`), then
   `P = prefix[8] || be32(len(S)) || S || 0x20 padding`, where `prefix` = 8 `rand()` bytes and padded length =
   `t + (8 - t % 8)` with `t = 12 + len(S)` — **always at least one pad byte, a full 8-byte block when `t % 8 == 0`**
   (AOChat PHP pads 0 in that case `[AOChat 699]`; CellAO decrypts either way because it uses the embedded length
   `[CellAO ...LoginEncryption.cs:118-135]`).
5. **TEA** (`FUN_10012c53` `[IF 0x10012c53]`): 32 rounds, `delta = 0x9E3779B9`, sum incremented before each round;
   `v0 += ((v1<<4)+k0) ^ (v1+sum) ^ ((v1>>5)+k1); v1 += ((v0<<4)+k2) ^ (v0+sum) ^ ((v0>>5)+k3)`.
   Inverse = CellAO `DecryptTeaRound` (`sum = 0xC6EF3720`) `[CellAO ...LoginEncryption.cs:364-376]`.
6. **CBC**, IV = 0, over 8-byte blocks read as two LE u32. Original quirk: the chaining XOR is skipped when the previous
   ciphertext's first word is 0 `[IF 0x10012adc]` (p = 2^-32 per block, yields undecryptable data); `ao-net` always XORs.
7. **Output** `"<hex(dhX)>-<hex(ciphertext bytes, %.2x, lowercase)>"` (`sprintf("%s-%s")`).
8. **Server side** (CellAO): `K = dhX^serverPriv mod N`, same key rule, CBC-decrypt, skip 8 prefix bytes, `be32` length,
   split at the first `|`, salt = next 32 bytes, `|`, password = the rest `[CellAO ...LoginEncryption.cs:88-136]`. Server
   checks username == `UserLogin` username, password hash, salt == the one it sent.

Known-answer vectors (in `crates/ao-net/src/crypto.rs` tests). **Source: independent Python reimplementation of the
client algorithm above plus a port of CellAO's TEA decrypt, run offline — not captured from any server.**
`/tmp/aomac-ghidra/proto/kat/kat.py`. E.g. TEA key `000102…0f`, block `(0x01234567,0x89abcdef)` -> `(0x6847d0b4,0xd158787c)`;
x = `0123456789abcdef0123456789abcdef` -> `dhX = 8cc3b8c6…5a9c45`, `K` starts `24d742e39b1367d4699a636dd3911f35`.

## 5. Messages

`(dir)` C = client->server, S = server->client. "client:" = what the client reads/writes, with evidence; "CellAO:"
= differences from the emulator's definitions `[CellAO Libraries/Source/CellAO.Messages/SystemMessages/*.cs]`.

| id | name | dir | body |
|---|---|---|---|
| 0x22 | UserLogin | C | `i32 2`, `char name[40]`, `char version[20]` |
| 0x24 | ServerSalt | S | `u8 salt[32]` |
| 0x25 | UserCredentials | C | `char name[40]`, `i32 n`, `u8 response[n]` (n = strlen+1, **includes the trailing NUL**) |
| 0x0D | LoginError | S | `i32 code` |
| 0x0E | CharacterList | S | `i32 count`, `count x (CharacterData ‖ i32 status)`, `i32 allowedChars`, `i32 expansions`, `i32 slProfs` |
| 0x16 | SelectCharacter | C | `i32 charId` |
| 0x17 | ZoneInfo | S | `i32 charId`, `u8 ip[4]`, `u16 port`, `u32 cookie1`, `u32 cookie2`, `u32 eventServerType`, `u32 playerId` |
| 0x1B | ZoneLogin | C (to zone) | `i32 charId`, `u32 cookie1`, `u32 cookie2` |
| 0x3C | ZoneRedirection | S | `u8 ip[4]`, `u16 port` |
| 0x14 | DeleteCharacter | C | `i32 charId` |
| 0x15 | CharacterDeleted | S | (client reads nothing; resets `s_nCharID=0`; CellAO sends `i32 charId`) |
| 0x11 | CharacterCreated | S | `i32 charId` (client stores it as `s_nCharID` and immediately sends 0x16) |
| 0x10 | NameInUse | S | `i32` (CellAO: 0x1E) |

Not implemented in `ao-net` (outside the connect→charselect→zone path): 0x0F CreateCharacter
(`CharacterData_t` + `i32` `[IF 0x10001928]`), 0x55 RandomNameRequest / 0x56 SuggestName (`i16`-prefixed string
`[IF 0x10002a9e case 0x56]`), 0x43 chat-server list, 0x4E character info push, 0x20/0x21/0x23/0x30 (see below).

### 0x22 UserLogin `[IF 0x100025a9 InitAuth]`
`stream << 2; write(name,0x28); write(version,0x14)`; sent as `SystemMessage_t(0x22, 0, 1, size, buf)`. `name` =
`strncpy(buf, s_cPlayerName, 0x28)` (zero padded). `version` = first line of **`<client dir>/version.id`** read with
`getline(buf, 0x14)` (so <= 19 chars), i.e. `00.7.2_EP1` for this client (strings "version.id", "Cannot open version.id
for reading" `[IF 0x10002687]`). CellAO matches (`Unknown=2`, 40/20 fixed) `[CellAO .../UserLoginMessage.cs]`.
If `version.id` cannot be opened `InitAuth` returns -1 and `ConnectToLH` returns -2 (no UserLogin is sent).

### 0x24 ServerSalt `[IF 0x10002a9e case 0x24]`
`stream.read(buf, 0x20); buf[0x20]=0;` -> `MakeChallengeResponse` -> `AuthClient` (0x25). CellAO same.

### 0x25 UserCredentials `[IF 0x10001b65 AuthClient]`
`write(name,0x28); << (len(resp)+1); write(resp, len(resp)+1)`. `SystemMessage_t(0x25, 0, 1, ...)`. CellAO reads `string[40]`
+ `Int32`-prefixed string (NUL trimmed) `[CellAO .../UserCredentialsMessage.cs]`.

### 0x0D LoginError `[IF 0x10002a9e case 0x0D; 0x10001e06]`
`i32 code` forwarded to two signals (UI). Known codes (CellAO): `0x14` already logged in, `0x6A` invalid user/password,
`0x6C` banned / not paid `[CellAO .../LoginError.cs]`. After an error CellAO disconnects. The client-side text for each
code is looked up in the UI layer (`AFCM::Send(10, 0xff, code)` `[IF 0x10001e06]`), not enumerated here.

### 0x0E CharacterList `[IF 0x10002a9e case 0x0E]`, entry = `CharacterData_t::ReadStream` `[MP 0x10001291]`
```
i32 count
repeat count:
    -- CharacterData_t::ReadDataStream [MP 0x10001130]
    i32 dataVersion          (written as 4; <4 selects legacy branches, unsupported here)
    i32 charId
    u8  'a' (0x61)           PlayfieldProxy version, else "Invalid playfieldproxy version" [MP 0x1000322e]
    Identity playfield       (CellAO sends IdentityType.Playfield = 0xC79D + playfield id [CellAO Libraries/Source/CellAO.Enums/IdentityType.cs:90]; the client's own legacy default is 0xC79C / 0x9C50 [MP 0x10001130])
    i32 attribute
    i32 exitDoor
    Identity exitDoorId
    i32 created              (dataVersion>=2; !=0 -> created)
    -- CharacterData_t::ReadBlobStream -> CharacterInfo_c::ReadStream [MP 0x1000196f]
    i32 infoVersion          (written as 5; ==2 legacy branch unsupported)
    i32 id                   (CharacterInfo.Instance; then overwritten with charId above [MP 0x100011c5])
    i32 orgInstance          (infoVersion > 4)
    i32 nameLen; u8 name[nameLen]      (nameLen <= 0xFE, else the client substitutes "ERROR-CHANGE-NAME")
    i32 breed; i32 gender; i32 profession; i32 level      (offsets +0x24,+0x28,+0x2c,+0x30 [MP 0x10001039,0x10001057,0x10001068])
    i32 areaLen; u8 area[areaLen]      (<= 0xFE, else "ERROR-CHANGE-AREA")
    i32 banned
    i32 banReasonLen; u8 banReason[..]
    i32 head; i32 height; i32 width    (infoVersion > 3) [MP 0x10001079,0x1000108a,0x1000109b]
    -- back in ProcessMessage
    i32 status               (pushed to a parallel vector; CellAO CharacterStatus.Active)
i32 allowedChars; i32 expansions; i32 slProfs   (3rd stored as s_nSLProfsEnabled = (v != 0); the first two are read and signalled)
```
CellAO's `LoginCharacterInfo` sends exactly this for `infoVersion=5, dataVersion=4, proxy=0x61`, with two trailing
ints (`AllowedCharacters`, `Expansions`) — **this client reads a third int** (`slProfs`). A two-int tail from CellAO
would leave the third read short (the stream read leaves the zero-initialised value, `[BS 0x1000193f]`; `[inference]`
for the over-read behaviour), and `ao-net` decoding rejects it — send 12 trailing bytes when emulating. `expansions`
bit meaning is `[inference]` (CellAO passes the `login.Expansions` DB column); `breed/gender/profession` are CellAO's
`Breed/Gender/Profession` enums.

### 0x16 SelectCharacter `[IF 0x10001c5b LoginCharacter]`
`i32 s_nCharID`, then **iff** `DatabaseInterfaceModule_t::DataSet()` begins with `"aordb"`, an `i16`-length-prefixed copy
of it `[IF 0x10004a34]`. `DataSet()` returns `ResourceDatabase_t::GetRdbDSN()` `[IF 0x1000b2aa]` which in this build returns the
3-byte string `"Abc"` `[DC 0x10004770; bytes at 0x10118934]` -> condition false -> **body is just the 4-byte id**.
(`"none"` if no RDB is open — also false.) Sent as `SystemMessage_t(0x16, 0, 1, ...)`.

### 0x17 ZoneInfo `[IF 0x10002a9e case 0x17]`
Read order: `charId(i32) ip(i32 -> a.b.c.d network order) port(i16) cookie1 cookie2 eventServerType playerId` (all remaining i32).
Stored into `s_nCharID, s_nServerIPAddr, s_nServerPort, s_nCookie1, s_nCookie2, s_nEventServerType, s_nPlayerID`.
CellAO sends only up to `cookie2` (22 bytes) `[CellAO .../ZoneInfoMessage.cs]`; the client tolerates that (missing ints read as 0).
Next: `RedirectToServer` `[IF 0x1000291d]` connects `ACE_INET_Addr::set(port, ip)`.

### 0x1B ZoneLogin `[IF 0x100013b0 SendClientCookie]`
`<< s_nCharID << s_nCookie1 << s_nCookie2`, `SystemMessage_t(0x1B, charId, 2, ...)` (sender = charId, receiver = 2). CellAO's
`ZoneLoginMessage` has only `charId` — this client always appends both cookies.

### 0x3C ZoneRedirection `[IF 0x10002a9e case 0x3C]`
`i32 ip`, `i16 port` -> `OpenConnection` to it, then `SendClientCookie` (0x1B). CellAO: `IPAddress` + `ushort` `[.../ZoneRedirectionMessage.cs]`.

### Other client-handled system messages (not implemented)
* 0x20 -> fatal "Connection error: Protocol Not OK."; 0x21 -> `AFCM::Send(10, 0x11f)` (+ signal with an `i32`); 0x30 ->
  three ints; 0x23 -> ignored; others -> "Protocol error: Got unknown system message %u as reply" `[IF 0x10001e06]`, `[IF 0x10002a9e]`.
* 0x43: `i32 n`, `n x { i32 len; u8 str[len]; i32; f32 }` -> signal at `GlobalSignals+0x1c8` `[IF 0x10002a9e case 0x43]`. CellAO's
  `ChatServerInfoMessage` is the `n=1` case: `i32 1, str host, i32 port, i32 unknown` `[CellAO .../ChatServerInfoMessage.cs]`.
* 0x4E: `i32` + `CharacterInfo_c` blob -> emitted on the `Client_t+0x6c` / `+0x68` signal lists.

## 6. Differences from CellAO (do not copy its server blindly)
1. ZoneInfo: client reads 26 bytes (two extra ints). 2. ZoneLogin: client sends `charId + cookie1 + cookie2`. 3. CharacterList: three trailing
ints. 4. Credentials plaintext pad rule (full block when aligned). 5. Server public key differs from AOChat/CellAO era keys.
6. `SelectCharacter` carries no DSN on this build.

## 7. `ao-net` map
`frame::Frame{encode,decode,system,system_parts}`, `frame::RecvSeq` (§2); `crypto::{make_challenge_response[_with],
open_challenge_response, tea_*}` (§4); `msg::Message{to_frame,from_frame,encode_body,decode}` and the structs for §5.
`cargo test -p ao-net`: framing byte vectors (hand-derived from §2), TEA/DH/credentials known answers (§4 provenance),
round trips for every message, malformed input rejection. Randomness (DH exponent, 8-byte prefix) is a caller input.

## 8. Open questions (need a live capture / live connection — NOT done, permission pending)
1. Does Ithaca's login server hold the private key for client key `Y` (§4)? Expected yes (client is PRK's), but any mismatch would only surface as `LoginError 0x6A`.
2. Does PRK's server expect the account password in the `S` plaintext, or a launcher-issued token? (Client reads `s_cPlayerPasswd` from the UI; launcher passes none.)
3. Exact `sender`/`receiver` header values PRK sends in server frames and whether the first server frame can be anything other than `ServerSalt`/`LoginError`.
4. Real values of `status`, `allowedChars`, `expansions`, `slProfs`, `eventServerType`, `playerId`, and the `PlayfieldProxy` identity types in live `CharacterList`/`ZoneInfo`.
5. Whether PRK sends extra system messages (0x43 chat server list, 0x4E, 0x21, 0x30) between 0x25 and 0x0E.
6. Zone side: does the server speak first on the zone connection (ping/N3), and when exactly does the client emit 0x1B relative to connect (`N3ActivatedMessage` timing)? Ping (ptype 0xB, 0x28-byte header) layout and whether keepalive is required on the login connection were not analysed (`StartPingManager` only runs after `RedirectToServer`, `[IF 0x1000291d]`).
7. Whether PRK enables `0x7F` compression on login or zone connections and its exact framing (zlib stream boundaries).
8. Whether `version.id` content (`00.7.2_EP1`) is validated server-side and what the server answers on mismatch (0x20/0x21?).
9. Meaning/encoding of the numeric error codes beyond the three CellAO names.

## 9. Sources used
* This repo's analysis of the binaries above (Ghidra 12.1.4, headless; scripts in `/tmp/aomac-ghidra/proto/scripts`).
* CellAO (BSD-3-Clause) — message layouts and server-side decrypt; no code copied.
* Budabot/AOChat `generate_login_key` (GPLv2) — algorithm cross-check only (`$dhN`, `aochat_crypt`/`aocrypt_permute`); no code copied.
* Decompiled PRK launcher (`/tmp/prk/src`) — argument format.
