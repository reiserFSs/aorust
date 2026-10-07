# Chat window: combat / feedback lines (`crates/aomac/src/play/chat/log.rs`)

How the original client turns game events (hits, misses, XP, heals, level-ups, server feedback ...) into chat-window lines other than
player chat. Address tags: `[GC]` Gamecode.dll, `[GUI]` GUI.dll, `[LDB]` ldb.dll (the text database / `LDBformat` / `RemoteFormat` library,
imported into the Ghidra copy for this work), image-base VAs. Code: `classify` / `from_n3` / `events` in `log.rs`.

## 1. Pipeline

```
N3 IIR apply (AttackInfo, Missed, ...)  ->  FUN_10012bd5 [GC 0x10012bd5]  "combat-log formatter" (category switch 0x1a..0x47)
                                                |  text = LDBformat(template).Feed(args).Dump()
                                                v
                                       FUN_10012b05 [GC 0x10012b05](class, text, ColorCode)  = emit GlobalSignals+0x17c
                                                v
                              slot FUN_10083898 [GUI 0x10083898](class, text, ColorCode)
                                  class < 0x40000000: window by legacy number (FUN_10093b33) -> FUN_1009b37f directly
                                  else GetGroupIdentifier(class) -> subscribed window(s) via FUN_10084f9e [GUI] (chat-filter rules, §7)
                                  class without a window -> System class 0x40000001
                                                v
                              FUN_1009b37f [GUI 0x1009b37f]: "<div>" + ("<font color=NAME>" if code != 0) + ExpandChatTextArgs(text)
                                                               + "</font>" + "</div>"   -> appended to the window
```

[LIVE fix] A `ChatLine` built with [`window_html`] (camp / teleport / feedback lines) is final: `win::ChatWindows::push` appends it as it is. It used to wrap it in a second `<div indent=wrapped><font>` with the window's `(HH:MM)` stamp, which
put the stamp on a row of its own above the red text (seen live on the camp line and the teleport lines). Plain `ChatLine` texts (combat lines of `log.rs`) still get the wrapper.

No timestamp, no "[class]" prefix and no link markup is added on this path (the `(%H:%M) ` stamp and `user://` links belong to
`FUN_1009b4cf` [GUI 0x1009b4cf], the player-chat line builder). `ChatLine.text` produced by `log.rs` is the text *before* `ExpandChatTextArgs`;
`ChatLine.kind = ChatKind::Other(NAME)` with NAME from `ColorCode` (`color_name`); **NAME is `""` for code 0 = no `<font>` at all**.
`window_html(color, text)` builds the wrapper. `LogLine.class` is the routing class (`class::*`).

### 1.1 Message classes (chat-window filter categories)
Registered in `FUN_10083e53` [GUI 0x10083e53] as `FUN_10083be4(name, id, 0, 2, 1)` (shown by the chat-config window; defaults in
`FUN_10094c58` [GUI 0x10094c58]):

| id | name | id | name |
|---|---|---|---|
| 0x40000001 | System | 0x42000008 | You hit other |
| 0x40000002 | Vicinity | 0x42000009 | Your pet hit by other |
| 0x40000003 | Tell Messages | 0x4200000a | Other hit by other |
| 0x41000000 / 01 | Your Pets / Other Pets | 0x4200000b | Me got XP |
| 0x42000001 | Me hit by environment | 0x4200000c | Me got SK |
| 0x42000002 | Me hit by nano | 0x42000011 | Your pet hit by monster |
| 0x42000003 | Your pet hit by nano | 0x42000012 / 13 | Your misses / Other misses |
| 0x42000004 | Other hit by nano | 0x42000014 / 15 | You gave health / Me got health |
| 0x42000005 | You hit other with nano | 0x42000016 / 17 | Me got nano / You gave nano |
| 0x42000006 | Me hit by monster | 0x42000018 | Me Cast Nano |
| 0x42000007 | Me hit by player | 0x4200001a / 1b | Team Loot / Vicinity Loot Messages |
| 0x4200001c | Research | | |

### 1.2 `ColorCode_e` -> `TextColors.xml`
`ChatGUIModule_c::ColorCodeToHTMLColor` [GUI 0x10087860] indexes the `{code, name}` table at [GUI 0x10268d38] (0 = `CCNoneColor`, skipped by
`FUN_1009b37f`): 12 CCRed, 16 CCYellow, 21 CCMeHitByNanoColor, 22 CCOtherHitByNanoColor, 23 CCMonsterHitMeColor, 24 CCPlayerHitMeColor,
25 CCMeHitOtherColor, 26 CCOtherHitOtherColor, 27 CCOtherHitOtherMyPetColor, 28 CCMeHealedColor, 29 CCMeGotXpColor, 30 CCSkillColor,
35 CCMeCastNano (full table in `color_name`). The RGB values are `cd_image/gui/Default/TextColors.xml` (e.g. CCMeHitByNanoColor 0xffffff,
CCMonsterHitMeColor 0xff0000, CCMeGotXpColor 0xffff00).

## 2. The formatter `FUN_10012bd5`

`FUN_10012bd5(cat, dynel, amount, other, text, damage_type, mode, nano)` (stack slots `[8] [0xc] [0x10] [0x14] [0x18] [0x1c] [0x20] [0x24]`; the
caller builds them in `FUN_10012a1e`). Rules common to all categories:

* `amount == 0` -> nothing. `damage_type == 0` -> `0x1b`. Template = text database category **110** (`LDBface::GetText(110, ElfHash(key))`, `FUN_1003807c`),
  keys below. `nano != 0` -> nano object by instance id (`FUN_10082998`, `{id, kind 1000000}`), name = vtable `+0x34`.
* `FUN_10036adf` [GC 0x10036adf] = damage-type name by stat id (table built in `FUN_100324d2` [GC 0x10033a85]): 90 projectile, 91 melee, 92 energy,
  93 chemical, 94 radiation, 95 cold, 96 poison, 97 fire, 168 nano, 27 unknown, 474 fall, 489 backstab, else `Missing damagetype: %d`.
* "`other` distinct" = `other != 0 && other != dynel`. `+0x140` = `IsClientChar`, `+0x21c != 0` = non-player character.
* Tail [GC 0x10014b92]: `mode == 4` -> text + `" "` + `Feedback_CriticalHit` ("Critical hit!"); `mode == 2` -> + `Feedback_GlancingHit` ("Glancing hit.").
  Then `FUN_10012b05(class, text, color)`; the damage number floating over the head (`sprintf("%d")` -> `FUN_100044c2` / `FUN_10011108`) is not chat.

Per category (`d` = dynel `[0xc]`, `o` = other `[0x14]`, `T` = damage-type name; text keys are `Feedback_*` unless noted; arguments in `Feed` order):

| cat | class | color | text |
|---|---|---|---|
| 0x1a me hit by nano / DoT | 0x42000002 | 21 | dtype 0x1da: `FallDamage`(amt); nano: `AttackedByForPointsOfDamage`(nano,amt,T); other: `YouWereAttackedByNanobotsFrom`(name o,amt,T); else `AttackedByNanobotsForPointsOfDamage`(amt,T) |
| 0x1b other / 0x1c my pet hit by nano | 0x42000004 / 03 | 22 / 27 | fall: `TookPointsOfFallDamage`(d,amt); nano: `WasAttackedByForPointsOfDamage`(d,nano,amt,T) or `...From`(d,nano,o,amt,T) when `o` set; else `WasAttackedByNanobots`(d,amt,T) / `WasAttackedByNanobotsFrom`(d,o,amt,T) |
| 0x1d hit by monster | 0x42000006 | 23 | no other: `WereHitForPointsOfDamage`(amt); `HitYouForPointsOfDamage`(o,amt,T) |
| 0x1e hit by player | 0x42000007 | 24 | no other: `PlayerHitYou`(amt); `PlayerHitYouForPointsOfDamage`(o,amt,T) |
| 0x1f you hit | 0x42000008 | 25 | `HitWithSpecial`(d,amt,T) = "You hit %s for %u points of %s damage." |
| 0x20 / 0x21 others (pet involved) | 0x4200000a / 09 | 26 / 27 | no other: `SomethingHitOther`(d,amt,T); `OtherHitOther`(o,d,amt,T) |
| 0x22 healed | 0x42000015 | 28 | `HealedForPoints`(amt) |
| 0x24 xp | 0x4200000b | 29 | amt<0: `LostXP`(abs); else `ReceivedXP`(amt) |
| 0x25 / 0x27 damage / reflect shield hit me | 0x42000006 (7 if `o` is a player) | 23 | no other: `SomeonesDamageShieldHitYou` / `SomeonesReflectShieldHitYou`(amt); `YouWereHitByDamageShield` / `...ReflectShield`(amt, name o) |
| 0x26 / 0x28 shield hit other | 0x4200000a, 0x42000008 if `o` is the client | 26 | no other: `SomethingHitOtherWithDamageShield`(d,amt); `OtherDamageShieldHitOther`(o,d,amt); `YourDamageShieldHitOther`(d,amt) (reflect: same with `Reflect`) |
| 0x2c toxic on me | 0x42000001 | 26 | `MeHitByToxic`(amt) |
| 0x30 special on me | 0x42000007 | 23 | `HitYouForPointsOfDamage`(o,amt,special name) |
| 0x31 / 0x33 special, others / pet | 0x4200000a / 09 | 26 / 27 | no other: `SomethingHitOther`(d,amt,name); `MonsterHitWithSpecial`(o,d,amt,name) |
| 0x32 you special | 0x42000008 | 25 | `HitWithSpecial`(d,amt,name) |
| 0x34 / 0x35 absorbed (me / other) | 0x42000006 / 0x4200000a | 23 / 26 | `YouAbsorbedDamage`(amt,T); `SomeoneAbsorbedDamage`(amt,T) or `OtherAbsorbedDamage`(o,amt,T) |
| 0x3b miss | 0x42000012 (we miss) / 0x42000013 (we are missed) | 0 | we miss: `YouTriedToHitMissed`(d) or `TryToAttackWithSpecialButMiss`(d,name); we are missed: `OtherTriedToHitMissed`(o) or `OtherTriesToAttackWithSpecialButMisses`(o,name); nothing if neither is the client |
| 0x3e shadowknowledge | 0x4200000c | 29 | `LostSK`(abs) / `GainedSK`(amt) |
| 0x3f pet toxic | 0x42000011 | 26 | `PetHitByToxic`(d,amt) |
| 0x40 you hit with nano | 0x42000005 | 22 | nano: `YouHitOtherWith`(d,nano,amt,T); else `YouHitOtherByNanobots`(d,amt,T) |
| 0x41 / 0x42 | 0x42000014 / 15 | 0 | `YouHealed`(d,amt) / `GotHealedByPlayer`(d,amt) |
| 0x43 / 0x44 | 0x42000017 / 16 | 0 | `PlayerIncreasedNano`(d,amt) / `GotNanoIncreaseFrom`(d,amt) |
| 0x45 alien xp | 0x4200000b | 29 | amt>=1: `GainedAlienXP`(amt) |
| 0x47 cast nano | 0x42000018 | 35 | `ExecutingNanoProgram`(name) |

(0x23 only sets colour 28, no text; 0x2f is the `SimpleItem_t` variant `FUN_10014e3d` called from `FUN_1009b170`, not ported.)
Example, template + live capture: `%s hit %s for %u points of %s damage.` with the first `AttackInfoIIR_t` of `zone_ithaca.rec` ->
`npcfa8d7 hit npcf4a4c for 17 points of melee damage.` (test `captured_combat_messages`; the item default, the guard's slot table is not known yet at that hit — docs/zone/combat-log.md §2.1.1).

## 3. Events and which category they use

| message (id) | apply | category selection | `log.rs` event |
|---|---|---|---|
| `AttackInfoIIR_t` 46002F16 | [GC 0x1009ed0d] -> `FUN_1006a8f3` [GC 0x1006a8f3] -> `FUN_1009b170` [GC 0x1009b170] | victim = client: attacker is a player ? 0x1e : 0x1d; attacker = client: 0x1f; else pet involved ? 0x21 : 0x20. `mode` = `unk_30` (live 3, 4) | `Hit` |
| `MissedAttackInfoIIR_t` 5C654B28 | `FUN_1006ae50` [GC 0x1006ae50] | 0x3b with amount 1, text = `fStatToString(stat)` (internal name, "" if 0) | `Miss` |
| `SpecialAttackInfoIIR_t` 754F1115 | `FUN_1006a9c5` [GC 0x1006a9c5] | attacker not client: victim client ? 0x30 : pet ? 0x33 : 0x31; attacker client: 0x32; name = text category **2003** id = special stat (`GetText(0x7d3, special)`) | `SpecialHit` |
| `AbsorbIIR_t` 264E5F61 | [GC 0x1009ea2e] | header client ? 0x34 : 0x35 | `Absorb` |
| `HealthDamageIIR_t` 3710256C | [GC 0x100a00c8] | delta<0: header client 0x1a; header my pet 0x1c; attacker client 0x40; else 0x1b. delta>=0 and header client: 0x22 | `HealthDamage` |
| `ReflectAttackIIR_t` 1C3A4F77 / `ShieldAttackIIR_t` 25192476 | [GC 0x100a0c6b] / [GC 0x100a0df6] | header client: 0x27 / 0x25 (other forced 0); else 0x28 / 0x26 | `Reflect`, `DamageShield` |
| `StatIIR_t` 2B333D6E | [GC 0x100a1aaf] | per pair, `diff = new - old`: 0x1b: diff<0 -> 0x1a (client) / 0x1c (my pet) / 0x1b, diff>0 and client -> 0x22; 0x34 -> 0x24; 0x28 -> 0x45; 0x23d -> 0x3e; 0x2aa..0x2ac (PVP scores) -> direct line `Feedback_GotPVPScore`(diff, text category 2002 id stat) class 0x4200000b colour 29 | `Stat` (`events()` expands the pairs) |
| `NewLevelIIR_t` 7F405A16 | [GC 0x10075a0c] | client only: cat 0x24 amount `f[7]`; direct `Feedback_NewLevel`(f[0]) (class 0, colour 0); if `f[5]>0` direct `Feedback_CongratulationsReachedLevel`(title) | `NewLevel` |
| `ShadowLevelIIR_t` 3C1E2803 | [GC 0x1007759e] | client only: cat 0x3e amount `f[6]`; direct text category **101** `Format_WelcomeToSKLevel`(f[0]); if `f[7]>0` the congratulation line | `ShadowLevel` |
| `CharacterActionIIR_t` 5E477770 | `FUN_1005d0d8` [GC 0x1005d0d8] | §4 | `Action` |
| `FeedbackIIR_t` 50544D19 | [GC 0x10072e81] | direct `(group, GetText(cat, id), colour 0)` | `Feedback` |
| `FormatFeedbackIIR_t` 206B4B73 | [GC 0x10039341] -> `FUN_1005aaa9` [GC 0x1005aaa9] | mode 1 / 2: floating text (not chat); else direct `(group, RemoteFormat::ParseString(text), colour 16 = CCYellow)` | `FormatFeedback` |
| `N3Msg_CastNanoSpell` (local) | [GC 0x1001b54b] | client casts: cat 0x47 with the nano name | `CastNano` (hub builds it) |

Resolution of the dynels: `FUN_10058e36` returns a dynel only for identity kind 50000 (0xC350). A missing dynel drops the event (`LogCtx::name`
returning `None`). **Direction (code wins over the earlier guess in docs/zone/misc.md §4/§7):** in `AttackInfoIIR_t` the header dynel is the
*attacker* and `other` (+0x24) the *victim* (`FUN_1009ed0d` prints "You were hit" when the header is absent and `other` is the client; Health is
subtracted from the fight target; `MissedAttackInfo.source` equals the header in all 11 captured messages). `log.rs` maps header -> attacker,
`other` -> victim.

### Received-frame health ordering

`HealthDamageIIR_t::Apply` [GC 0x100a00c8] formats the packet's nominal delta, then writes its absolute Health before the next received IIR. `StatIIR_t::Apply` [GC 0x100a1aaf] therefore compares against that updated baseline, not the health from the beginning of the render tick. The incoming-only fixture `docs/captures/health_damage_stat_clamp.rec` preserves `clamp.rec` lines 1029–1031 (tick 20751): own `50000:33588`, HealthDamage Health=66/delta=+50, action 0xaa, then Stat Health=66. Starting at Health=46, this emits only the nominal 50-point heal; the following Stat difference is zero, not another 20-point heal. No login or credentials are retained.

The port applies this absolute write inside `Zone::on_frame` to own stats, character stats and an existing dynel, including known foreign characters. The normal dispatch remains pre-apply for Stat/XP chat and post-apply for Info pages; nano removal's Life undo remains after `Zone::on_frame`. `captured_health_damage_applies_before_same_batch_stat_feedback` queues the actual captured incoming batch as `LoginEvent::ZoneFrame` events in one `Play::pump`, checks all health views and the single 50-point line, then checks that a later standalone Stat increase still emits its genuine difference. It also checks the known foreign-character path using the captured packet layout.


### 3.1 Wire layouts of the IIRs `ao_net` does not decode (after the 13-byte N3 header, big-endian `i32`, `Identity` = 2x`i32`)

| class | `ReadSubClass` | layout |
|---|---|---|
| Absorb | [GC 0x1009e9db] | `+0x18 amount`, `+0x1c damage type` |
| HealthDamage | [GC 0x100a002d] | `+0x18 health after`, `+0x1c delta`, `+0x20 damage type`, `+0x24 id` (-> `FUN_1005ae91`), `Identity +0x28 attacker`, `+0x30 nano id` |
| ReflectAttack / ShieldAttack | [GC 0x100a0bfe] / [GC 0x100a0d89] | `+0x20 amount`, `Identity +0x18 attacker`, `+0x24 id` |
| NewLevel / ShadowLevel | [GC 0x100758c8] / [GC 0x1007745a] | 8 x `i32` (+0x18..+0x34); apply sets stats 0x36,0x34/0x23d,0x35,0x39,0x15e,0x113/..., 0x25 |
| Feedback | [GC 0x10072e1d] | `i32 group (+0x18)`, `i32 category (+0x1c, ctor default 110)`, `i32 id (+0x20 = ElfHash(key))` |
| FormatFeedback | [GC 0x100392ff] | `i32 group`, `u16 len + bytes` (`FUN_100388e2`, len >= 0x8000 = empty + stream error), `i32 mode (+0x3c)` |

None of these occur in `docs/captures/*.rec` (only AttackInfo / Missed / SpecialAttackInfo / Stat / CharacterAction do); the layouts are from the
read functions and covered by synthetic-byte tests (`raw_wire_layouts`), **not verified live**.

## 4. `CharacterActionIIR_t` text actions (`FUN_1005d0d8`)

Fields `(action, param, identity_a, identity_b)`. The action id indexes the byte table at [GC 0x1005efde] (index = action) that selects one of 0x67
cases; most cases are non-text game actions (UI, anims, ...). Text cases implemented (`param` = destination class of the *direct* lines;
"tail" lines are emitted by [GC 0x1005edd6] only when the header dynel is the client's):

| action | case | text |
|---|---|---|
| 0x01 | 0 | tail: text category 1000 `KilledMissionTarget` |
| 0x2b | 10 | tail: category 1000 `RecievedTeamBonus`; client: cat 0x24 with `b.instance` |
| 0x4f | 0x12 | tail: category 1000 `YouAreInsured` |
| 0x77 | 0x24 | direct (class `param`, colour 0): `TeamMemberLinkdead`(name a) |
| 0x81 / 0x82 | 0x27 / 0x28 | cat 0x42 / 0x41 with dynel = a, amount `b.instance`, other = header |
| 0x8a / 0x8b | 0x2c / 0x2d | cat 0x44 / 0x43 (0x8b only when `b.instance > 0`) |
| 0x8c | 0x2e | tail: `IncreasedNanoPool`(b.instance) when a is a dynel and `b.instance > 0` |
| 0x9b | 0x33 | direct, colour 12 (CCRed): `YouHaveBeenDetected`(name a) |
| 0x9c | 0x34 | tail: `YouWereDrained`(abs b.instance, name a) |
| 0x9d | 0x35 | direct: text category 200 `VersionsInfo`(10, b.instance); if `b.instance != 10` also `ClientServerVersMM` |
| 0xa4 | 0x38 | direct, colour 30: `SkillAvailable`(text category 2002) |
| 0xc4 / 0xc5 | 0x50 / 0x51 | direct: `StuckResolved` / `StuckAvailable`(b.instance) |
| 0xd1 | 0x5b | toxic damage when `a.instance > 0 && b.instance > 0`: client cat 0x2c, client's pet cat 0x3f (amount `b.instance`) |

Not ported (no chat line or not decoded): 0xce `PerkAvailable` (name via `FUN_1005366e`), 0xd0 `DrainedHealth/Nano`, 0x105 `InspectRejected` (popup),
the `PvPTargetLvl` signal (0x54), every other case. Live captures only contain actions 0xA7, 0x63, 0x62, 0xAD, which print nothing.

## 5. `LDBformat` (ldb.dll) — the template language

Implemented as `ldb_format(template, args)`:

* `LDBformat::Init` [LDB 0x100053f2]: `%[-+ 0#1-9.hlL]*conv` is token N (N counts `%` conversions from 1), `%%` is a literal `%`,
  `#N{...}` is a variant token bound to argument N (`#N:` is swallowed, `##` quirks reproduced).
* `Feed(x)` [LDB 0x10004c8b/da7/fd1]: tokens of the current argument number are replaced: `%` tokens by C `snprintf` (a `%s` fed an integer prints
  `int_value<%d>`), `#N{...}` by `FeedInternal` [LDB 0x100046ab].
* `FeedInternal`: `{FLAGS:text|FLAGS:text}` (a text after a space starts at the space): flags = OR of `MakeFlagBit` [LDB 0x100040c8] (A..Z = bits 0..25,
  digits 0..5 = bits 26..31); `Feed(int)` raises bit 26 for 0 and bit 27 for 1; the option with the highest popcount of `flags & fed` wins, later options
  win ties. So `%u #1{1: credit was| credits were}` -> "1 credit was" / "2 credits were". (`#1{ 1:buddy | buddies }` in text category 20000 never selects
  "buddy": the first option has no flag.)
* `Dump` [LDB 0x10004885]: concatenates the tokens, drops leading spaces and collapses runs of spaces into one.

`RemoteFormat::ParseString` [LDB 0x1000593d] / `Init` [LDB 0x10005565] (`remote_parse`): text with `~&` spans: 5 base-85 chars template category, 5 for
the id (`GetText(cat, id)`), then arguments `i`/`u` + 5 chars (int), `f` + 5 chars (float bits), `R` + 10 chars (nested text id), `s`/`F` + length prefix + nested
string (`FUN_10004013`/`FUN_1000403f`: UTF-8-coded `len + 1`; byte 0x92 = 0x27), terminated by `~`. A span without terminator is copied literally.
It is also run on the chat-server vicinity text by `HandleVicinityMessage` [GUI 0x10086728] (vicinity message `data[0]` selects the class there:
4 Your Pets 0x41000000, 5 Other Pets 0x41000001, 6 Vicinity Loot 0x4200001b, 7 Team Loot 0x4200001a, else Vicinity 0x40000002).

## 6. What the hub has to supply (`LogCtx`)

`own` (the client's character identity), `name(id)` (dynel name or `None` = not in world), `text(cat, id)` (`TextDb::by_id`), `is_npc(id)` (`+0x21c`),
`is_own_pet(id)` (`+0x21c && stat flag 0x8000000 && FUN_100523c3`), `nano_name(instance)` (RDB nano record name), `stat(id, stat)` (current stat,
used for the old value of `Stat` and the nano damage-type override 0x153 `DamageOverrideType` of the attacker, valid values 90..97 and 168 via
`FUN_1009a709`), `weapon(attacker, slot, special)` (stat 0x1b4 `DamageType` of the item behind the `AttackInfo` weapon slot, `combat::arms::Armory`; `FUN_1009afde`), and the `ChatFilter`. The hub applies the stat changes itself (Health `-= damage`, `FUN_10062349`).

Live dispatch in `play/flow.rs::pump` sends game-event feedback through `Chat::on_zone_frame` before
`Zone::on_frame` applies the packet, so `StatIIR_t` 0x34 (decimal 52, XP) compares the new total against
the old total rather than itself (§3, GC 0x100a1aaf). `Chat::on_zone_applied` separately builds InfoPacket
pages after zone application (GC 0x10045fba), retaining current packet skills on the first response and refresh.
The headless dispatch regression `stat_xp_feedback_uses_old_value_before_dispatch_applies_update` delivers
totals 100 then 137 through `LoginEvent::ZoneFrame`/`pump` and expects one 37-XP line plus stored total 137;
`character_info_first_response_and_refresh_use_current_packet_stats` covers the post-apply page ordering.


## 7. Filters

* Class subscription: which window shows a class is the window's group list (win owner).
* `/chatfilter [list|del|add|enable|disable|clear]` [GUI `FUN_100b8d4e`]: preferences `ChatFilterRules` (a message of `"<index>" -> rule string`) and
  `ChatFilterEnabled` (bool). `FUN_10084f9e` [GUI 0x10084f9e], on the path of §1, drops the line when any rule matches its text with the client's `RegExp`
  (Utils.dll `RegExp::Compare`). `RegExp` is Spencer's V8 `regexp(3)`, ported as `filter::V8Regex` (docs/chat/dialogs.md §6); `ChatFilter::drops` uses it. The hub's own state is `filter::FilterState`
  (the combat-log path of this module still gets the default, disabled filter: it is not one of the 5 callers of `FUN_10084f9e`).
* There is no per-colour or per-event "show combat text" switch on this path other than the class subscription (searched the GUI.dll strings
  `ChatFilter*`, `ShowTimestamps`, `ChatShow*`, `ChatWindowMenu_*`; no combat-text option found).

## 8. Unresolved / guesses

* `unk_30` of `AttackInfo` as the mode: 3 normal, 4 critical, 2 glancing is derived from the `mode == 4` / `mode == 2` comparisons in `FUN_10012bd5`;
  live values are only 3 and 4 (4 = critical presumed, never seen with text).
* Victim/attacker direction of `AttackInfo` (§3): derived from `FUN_1009ed0d` and `FUN_1009b170`; the live capture has no message involving the client.
* Damage type of a hit: `FUN_1009afde` reads stat 0x1b4 of the item behind the attacker's weapon slot (resolved, docs/zone/combat-log.md §2.1.1); `FUN_10058a05`
  (pvp flag in the 0x1e/0x1d choice) is treated as false.
* `Stat` 0x28 (AlienXP) has a second branch using stats 0xa9/0xb2 that is not modelled (diff only).
* CharacterAction 0xa4: the id given to `GetText(2002, ..)` is `b.instance` (register pattern identical to the Stuck cases, not proven).
* `AttackInfo` damage type default `0x1b`/`unknown` for stat-driven DoT (cat 0x1a / 0x1b with no damage type): text "... points of unknown damage." is what the code produces.
* Loot lines (class 0x4200001a/1b) come from the chat server's vicinity messages (§5), not from an N3 IIR; no Gamecode code emits those classes.
* The text for a missing database entry: `GetText` is not traced for "not found" (the port prints nothing).

## 9. Verification

`cargo test` of `log.rs` (private scratch crate pulling `line.rs` + `log.rs` unchanged; client data from `~/Games/ProjectRubiKa/client`, skipped without it):
`ldb_templates`, `remote_format_spans`, `combat_lines_from_the_originals_templates` (every cat above against the client's real `text.mdb` templates),
`raw_wire_layouts`, `captured_combat_messages` (134 AttackInfo / 11 Missed / 3 SpecialAttackInfo of `docs/captures/zone_ithaca.rec` decode to events and format),
`feedback_keys_exist` (every key the formatter uses exists in category 110), `chat_filter_drops_matching_lines`. Clippy: zero warnings in `log.rs`.
