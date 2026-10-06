# Chat: live results (Ithaca, 2026-10-05, character "Aomacvolk", app + probe)

Captures (credentials/username redacted, public-channel text of other players kept as is):
`docs/captures/chat_login_ithaca.rec` (standalone login, probe), `docs/captures/chat_session_ithaca.rec` (app session: login, groups, MOTD, own Global message, tell to an offline character).
Replay tests: `ao_net::chat::tests::live_login_capture_decodes`, `play::chat::net::tests::live_session_replay`.

## What was exercised

| step | result |
|---|---|
| zone sends 0x43 -> app connects to `199.241.136.157:7005` | login accepted (type 0 `IISS` with char id, user, DH/TEA key), type 5 LOGIN_OK 150 ms later |
| own name | `0x14` USER_NAME `33512 Aomacvolk` |
| MOTD | three anonymous vicinity packets (0x23), shown in the Default Window in the vicinity colour (`ctch_vicinity`) |
| name table | ~700 `0x14` USER_NAME packets right after login (all known/online characters, ~450 B/s for the first seconds), kept in `ChatNet::names` |
| channel list | seven `0x3c` GROUP_JOIN: `Neutral` (87/0), `Omni-Tek` (87/2), `Clan` (87/1), `Global` (05/20, flags 0x11), `Global Trade` (86/21, 0x11), `IRRK News Wire` (0c/2000, 0x13), `Server Announcements` (0c/2001, 0x13). PRK has no "OOC"/"Newbie Help" groups on this server; `Global` is the public chat channel. Group id key = `kind << 32 | id`. |
| others' messages | `0x41` GROUP_MESSAGE in `Global`; text carries HTML (`<a href="itemref://…">`), shown in the group colour with `[Global]` link prefix and sender link |
| we say `/g Global aomac client test` | sent `0x41 GSD` (`05 00000014`, text, data `00 01 00`), the server **echoes it back to us** as a normal GROUP_MESSAGE from our own id (data block empty): `[Global] Aomacvolk: aomac client test` is shown from the echo, not drawn locally |
| `/tell Testy aomac client test` | `0x15` lookup `Testy` -> `0x15 {id 0x6584, "Testy"}`, `0x1e ISD` tell, server answers with an anonymous 0x23 line "This player is currently offline and will not receive your message."; the local echo is the client's own text-db template `ChatTellMsgToField` (cat 10001, `"To [%s]: "`, pool offset 269621; verified by `play::chat::tests::tell_template_key_is_in_the_real_db`): `To [Testy]: …` |
| vicinity `aomac client test` (ptype 5 kind 3, 52-byte frame, target `{0,0}`) | accepted; ~7 s later the chat server echoes it as `0x22` (`ISD`, sender = our id, text, data `00 01 00`?) -- the first app build decoded 0x22 as `SSD` and dropped it, fixed; whether other players see it was not verifiable with one session |
| Space key in the input bar | was dropped by `ao-render` (named key without text); fixed in `viewer.rs` |

Outgoing ptype-5 frame as sent (hex, seq 4):
`0004 0005 0001 0034 000082e8 00000002 | 00000003 00000000 00000000 00000014 0011 "aomac client test" 00` (header `ptype 5, size 0x34, sender = char id, receiver = 2`; payload `kind 3, identity {0,0}, len 0x14, u16 len 0x11 + text + kind byte 0`).

## Re-run 2026-10-06 (offscreen live harness, step `say=<line>`)

`say=/say aomac vicinity test` after 4 s in the world: zone frame sent (`net.rec`: ptype 5, size 0x36: ` 000082e8 00000002 | 00000003 ... 0013 "aomac vicinity test"`, kind 3 text frame);
the chat server answered within the 12 s wait with `0x22` `000082e8 0013 "aomac vicinity test" 0001 00` (own id, data = `00`; test `decodes_vicinity_echo_of_own_text`).
The offscreen screenshot shows the line `(03:31) Aomacvolk: aomac vicinity test` in the Default Window in the vicinity colour below the three MOTD lines (and a pink server announcement line).
Note: a plain line (no `/say`) from `run_line` without a window output group sends nothing (as in the original: no feedback).

## Gaps (labelled)

* Vicinity/shout/whisper from other players arrive on the chat server as 0x22/0x23 (decoded, group routing per GUI 0x10086728); the zone N3 text classes (`ChatTextIIR_t` …) did not occur in the session.
* Outgoing-tell echo text, tell windows (per-sender windows) and the reply list: tells are routed to the "Tell Messages" group.
* Private groups (`/invite` …), buddy list, LFT: UI exists (docs/chat/social.md); live results in section 9.1 there. Only the S2C private group packets and non-empty LFT rows are not live-verified (need a second character), `/cc` is not verified.

## Reconnect after a dropped connection (live, 2026-10-06)

Live step `chatdrop` closes our end of the chat socket (`ChatNet::drop_connection`, `Quit` to the session thread; the thread reports `Disconnected("closed")`).
Timestamps (s since test start, `AOMAC_LIVE_STEPS=wait=20,chatdrop,wait=14,say=/say after reconnect,wait=10,shot=recon`):

| t | event |
|---|---|
| 6.1 / 6.5 | first `connect attempt 0`, `logged in` |
| 33.4 | `disconnected: closed` |
| 37.5 | `connect attempt 1` after **4.1 s** (measured before the pacing fix below: our own `backoff(0) = 1 << 12` ms) |
| 37.8 | `logged in` (attempts reset to 0) |

After the reconnect the chat server replays the MOTD (shown again in the Default Window) and our `/say` is echoed (`0x22`). Later back-offs 8.2 s / 16.4 s / 32.8 s are covered by `backoff_matches_client` only.
Correction from the full decompile of the loop (docs/chat/net.md): the client's first attempt after a drop is **immediate** (`attempts == 0`), the waits are 8.2 / 16.4 / 32.8 s from the second attempt on, and the loop has no attempt bound (the old 10-attempt limit is gone). The table above predates this fix; the new pacing is covered by `backoff_matches_client` and `retries_are_unbounded_and_every_16th_attempt_switches_server` and was not re-run live.

## Social layer re-run 2026-10-06 (buddy list, LFT, private group requests)

Live steps `buddyadd=<name>`, `buddyrm=<name>`, `lftsearch=<side>:<profession>:<location>`, `friendswin=on|off`, `lftwin=on|off` (plus `say=/lft ..`, `say=/invite|kick|leave ..`) drive the same `Req`s / command lines as the windows.
Capture `docs/captures/chat_social_ithaca.rec`, test `play::chat::social::tests::live_social_capture_replay`, details and the table of observed frames: docs/chat/social.md section 9.1.
Findings: S2C 0x28 = `id, online, empty data`; 0x29 echoes; /lft on / off get no reply; an empty LFT search is one all-zero status-0 0x5dd (id 0 = end marker, decoder logic fixed).
