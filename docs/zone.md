# Zone server traffic (after ZoneLogin) — what the real PRK server sends and what `aomac` does with it

Live captures (Ithaca, 2026-10-06, approved test account; credentials/cookies redacted, see `docs/protocol.md` §8):
`docs/captures/zone_ithaca.rec` (80 s, existing character "Testy", playfield 4582), `zone_newchar_ithaca.rec` (first 10 s of a character
created through the app, playfield 4604), `zone_enter_ithaca.rec` (a session where the app sent `CharInPlay`). Format `<ms> <'>' sent | '<' received> <hex>`,
one decoded (inflated, unpadded) frame per line. Every capture is replayed by unit tests (`ao-net` `n3::*`, `aomac` `play::zone`).
Per-message RE (class names, layouts, addresses, unresolved fields) is in `docs/zone/{world,dynel,misc,outgoing}.md`; this page is the index plus
what the app does. Decoders: `crates/ao-net/src/n3/{mod,world,dynel,misc,outgoing}.rs`; app state: `crates/aomac/src/play/zone.rs`.

## 1. Transport (see protocol.md §8 for the bytes)
* Server speaks first after `ZoneLogin`: a `7f 00` control frame, then **one zlib stream** (receive side only, unpadded frames). `ao_net::conn::Conn` handles it.
* The first data frames follow ~35 ms later in one burst (≈75 frames within 30 ms for a new character, ≈100 for an old one), then a trickle (≈12 frames/s in a busy hub: movement, combat, stat updates).
* Pings every 30 s (answered in `ao_net::client`); the session stayed open for 117 s (app) / 80 s (probe) with nothing but ping replies and one `CharInPlay`.

## 2. N3 frame (ptype 0xA)
`u32 key | Identity (i32 kind, i32 instance) | u8 pass_on | body`. The **key is a hash of the C++ class name** (`n3InfoItemRemote_t::MapToKey`, N3.dll 0x10009826:
`key ^= (i8)c << ((i & 3) * 8)` over the RTTI name), so every id has a name; `outgoing::REGISTRY` lists all 138 classes the client registers (Gamecode.dll 135, N3.dll 3).
Frame `sender` = acting dynel instance (1 = server), `receiver` = our character id. Identity kinds seen: `0xC350` character/NPC dynel, `0x9C50` playfield,
`0xC74A` weapon item, `0xC75B` vending machine, `0xC76A` corpse/object.

## 3. Message inventory (counts: 80 s existing character | 10 s new character)

| id | class | old | new | decoded meaning (details: docs/zone/*.md) |
|---|---|---:|---:|---|
| `5F4B1A39` | `PlayfieldAnarchyFIIR_t` | 1 | 1 | **which playfield to load** (RDB `0xF4241`/`proxy.exit_door_id.instance`) and a start position (cell-monitor centre) |
| `271B3A6B` | `SimpleCharFullUpdateIIR_t` | 81 | 21 | a character/NPC dynel appears: name, breed/sex, position, quaternion, level/health, head mesh, cloth, attractor meshes; the player's own one carries the start position + heading |
| `29304349` | `FullCharacterIIR_t` | 1 | 1 | own character: ~250 stats (Level, Profession, Cash 1000, IP 1500, abilities ...), skills; items/perks empty for a new char |
| `54111123` | `CharDCMoveIIR_t` | 144 | 20 | movement/position update (type, quaternion, position) |
| `2B333D6E` | `StatIIR_t` | 69 | 24 | stat changes (Health, ...) |
| `260F3671` | `FollowTargetIIR_c` | 173 | 0 | server-driven position + waypoint path (NPC wandering) |
| `46002F16` / `4A41203E` / `28494070` | `AttackInfoIIR_t` / `StopFightIIR_t` / `AttackIIR_t` | 134/128/128 | 0 | combat (damage numbers, attack start/stop) |
| `60201D0E` | `SetWantedDirectionIIR_t` | 72 | 0 | unit direction vector (XZ) |
| `1D3C0F1C` | `SpecialAttackWeaponIIR_t` | 58 | 1 | special attack list + initiative stats |
| `36510078` | `n3ToClientQuitIIR_t` | 27 | 0 | dynel despawn |
| `5E477770` | `CharacterActionIIR_t` | 22 | 1 | character action: id table, sit/stand, emotes → [zone/actions.md](zone/actions.md) |
| `3B1D2268` | `WeaponItemFullUpdateIIR_t` | 10 | 1 | a weapon item |
| `5C654B28`, `754F1115`, `51492120`, `52526858`, `39343C68`, `25314D6D` | Missed/SpecialAttackInfo, CharSecSpecAttack, GenericCmd_t, Buff, CastNanoSpell | 11/3/3/3/3/3 | 0 | combat log, item use, buffs, nano casts |
| `4F474E05` | `CorpseFullUpdateIIR_t` | 7 | 0 | corpse (tail raw) |
| `41624F0D` | `AppearanceUpdateIIR_c` | 2 | 1 | appearance (visual flags, cloth/attractor changes) |
| `5F52412E` | `GameTimeIIR_t` | 1 | 1 | game clock (67170.0 s) + server unix time |
| `2E2A4A6B`, `465A4061` | `OrgInfoPacketIIR_t`, `QuestFullUpdateIIR_t` | 1 | 1 | org info, quest list (empty) |
| `7F544905` | `VendingMachineFullUpdateIIR_t` | 1 | 0 | a vending machine object |
| `570C2039` | `CharInPlayIIR_t` | 2 | 0 | "this dynel is in play" (relayed from other players; the server also relays ours) |
| system `0x43` | chat server info | 1 | 1 | `199.241.136.157:7005`, consumed by the GUI chat module |

No ptype 5 (text) frame was received: server chat arrives as N3. Nothing is undecoded: `n3::tests::every_captured_message_decodes`.

## 4. Coordinates and the player's start
* Positions are `f32 (x, y, z)`, **Y up**, AO world metres. Scene coordinates are `(x, y, -z)` (`zone::scene_pos`): the scene bounds of playfield 4582 span z ∈ [-17885, 0],
  the server put the player at z = +742.7, so only the mirrored z lies inside the playfield. Verified in the app: playfield 4604 (Arrival Hall), player at server
  (205.2, 1.0, 255.8) → scene (205.2, 1.0, -255.8), camera (eye +1.7 m) inside the arrival hall facing the EXIT corridor (window capture inspected).
* Rotations are quaternions `(x, y, z, w)`; live ones are about Y only; heading = `2*atan2(y, w)`. Forward vector convention (`zone::scene_forward`) is a **[GUESS]**:
  rotation `yaw` about +Y takes +Z to `(sin, 0, cos)` in server space, mirrored like positions. It produced a sensible view in 4604 (facing the hall exit), but the
  handedness was not derived from the client's camera code.
* The `PlayfieldAnarchyF` position is only the cell-monitor centre (`AddCellMonitor`, GC 0x10121754); the player's placement is the own `SimpleCharFullUpdate`.
* New character start: playfield 4604 "Arrival Hall" (the list shows "Arrival Hall (4604)" after creation; before the first zone login the playfield of the
  `CharacterList` row is 4582 for "Testy" because that character was made earlier). The `CharacterCreated` reply carries no playfield: it comes only from the zone messages.

## 5. What the app does (aomac play)
1. `ZoneInfo` → loading screen (4 s) → zone connect → `ZoneLogin` → `LoginEvent::ZoneFrame(Frame)` for every non-ping frame.
2. `Zone::on_frame` decodes each N3 frame, tracks all announced dynels (name, position, yaw; removed on `n3ToClientQuit`).
3. `PlayfieldAnarchyF` → background load of that playfield (replaces the old "use the character list's playfield" logic; correct for freshly created characters too).
4. When the loading screen dissolves the camera is put at the **own dynel's server position** (eye 1.7 m above the feet, facing the server heading); the old density-based
   spawn is only a fallback. Free-fly (WASD/right mouse) is then active as before.
5. After 10 world frames the app sends `CharInPlayIIR_t` once (`ptype 0xA`, sender = charId, receiver 2, payload `570c2039 0000c350 <charId> 01`), like
   `WaitingToStartGame` (GUI 0x10027d73) after the `TeleportEnded` countdown (docs/zone/outgoing.md §3). The server answers by relaying `CharInPlay` to observers; it kept streaming and
   pinging afterwards.
6. The server needs **nothing but `ZoneLogin`** to start streaming (probe session), and nothing but ping replies to keep the connection (117 s observed). What happens if `CharInPlay`
   is never sent for a long time (visibility to others, timeouts) is unknown.

## 6. Plan for the in-game features (RE anchors)
Character actions (`CharacterActionIIR_t` ids, sit/stand/camp logic, `SocialActionCmd_t` emotes): [zone/actions.md](zone/actions.md) (`ao_net::n3::action`, `play/combat/actions.rs`).

Movement of other dynels, animation roles and name tags: [zone/motion.md](zone/motion.md) (`ao_net::n3::motion`, `ao_net::n3::nametag`).

| feature | next step | anchors |
|---|---|---|
| own character render | build `Player` from `SimpleCharFullUpdate` (breed, sex, `head_mesh` rdb 1010001 id, `cloth[]` texture ids per part, attractor meshes = weapons); place at `scene_pos(pos)`, yaw from `scene_forward`; camera third-person behind it | read GC 0x10078c24, apply 0x10077e13, construct `SimpleChar_t` 0x10077a84; `ao-formats/src/character.rs` (`Player::new`, `Equipment::wear`); docs/zone/dynel.md §1.3 |
| movement sync | send `CharDCMoveIIR_t` (`outgoing::char_dc_move`) on every movement-state change (action byte, quaternion, position, elapsed ms since previous send); apply received ones with the movement state machine | CharDCMove read GC 0x1006b96f / write 0x1006b9d6 / apply 0x1006b84b; jump table 0x1006d0ee (move types, partly unresolved); `SetWantedDirection`/`FollowTarget` for NPC/other-player interpolation |
| other players / NPCs | render `Zone::dynels`: NPC meshes come from `MonsterData` (stat 0x167) + `MonsterScale`; `textures[]` for NPC skins; despawn on `n3ToClientQuit` | docs/zone/dynel.md §1.4; `Die` handling; `FollowTarget` apply (docs/zone/misc.md §2) |
| terrain/ground following | server y is absolute: snap the avatar to the playfield height under it (`ao_formats::playfield::floor_below`) | spawn.rs |
| chat | incoming: N3 text IIRs (not yet identified — only 0x43 chat-server info seen); outgoing vicinity/whisper/shout via `ptype 5` `TextMessage` (`outgoing::text_payload/text_frame`); connect the chat server `199.241.136.157:7005` (0x43) — protocol not captured | Client_t::SendVicinityMessage IF 0x10001464; docs/zone/misc.md §14–15 |
| HUD / stats | `FullCharacter` stats (Health 34, Level 1, Cash, IP, abilities) + `StatIIR_t` deltas → HUD bars/windows; names via the client's own stat table (GC 0x10027483 / 0x1002f009, 524 ids) | docs/zone/world.md §2, docs/zone/dynel.md §3 |
| target / selection | client-local (`TargetingModule_t::SetTarget` GUI 0x100257b0, nothing sent to the server): `Zone.target`, click-to-select by a camera ray against per-dynel capsules, Tab cycling, target bars (`play/hud_target.rs`) | docs/gui.md §13.2–13.3 |
| combat | `Attack/AttackInfo/StopFight/MissedAttackInfo/SpecialAttack*` decoders exist; effects/animations need the character animation state machine | docs/zone/misc.md §3–11 |
| combat input / outgoing | `combat::{attack, stop_fight, sec_spec_attack}` encoders (flag byte 0), `can_attack`, `AttackGate` (`+0x79` guard), `default_attack`; keys/clicks → N3Msg chain | docs/zone/combat-net.md |
| time of day | `GameTimeIIR_t.time` (67170.0) → sky clock (`GameDayTime` wraps at 6480 s; 67170 mod 6480 = 2370) | docs/zone/world.md §4 |
| playfield changes | `PlayfieldAnarchyF` again (+ `ZoneRedirection` 0x3C when the zone server changes) | docs/zone/world.md §1, protocol.md §5 |

## 7. Open questions
* Heading handedness and the exact eye height / third-person camera of the original (see §4).
* Stat id semantics beyond the client's name table; the three `FullCharacter` entry groups; item/equipment layouts (empty for new characters, no live data yet).
* `CharacterCreated`'s extra `u32` (0x520eb100), `GameTime.arg3`, movement type names, `FollowTarget.mode` values, attack-info slot fields.
* Whether the server expects other client messages in play (e.g. `n3ToServerUnBlock`, locality updates) — none were needed in 2 minutes of play.
