# Fight controller, damage / heal numbers and combat-log lines

Code: `crates/aomac/src/play/combat/state.rs` (controller state machine, health bookkeeping, event stream), `log.rs` (the client's feedback
formatter `FUN_10012bd5`, `LDBformat`, colours, floating-number constants), `stat_names.rs` (`fStatToString` table). Evidence:
Ghidra projects `/tmp/aomac-ghidra/{playfield,proto}`; tags `[GC]` Gamecode.dll, `[GUI]` GUI.dll, `[LDB]` ldb.dll (imported into a private
Ghidra project; it is not part of the shared ones), image-base VAs. Captured data: `docs/captures/zone_ithaca.rec`. Wire layouts of the
messages: `docs/zone/misc.md` §3-§11, `docs/zone/dynel.md` §3; outgoing side and `CanAttack`: `docs/zone/combat-net.md`; animations and death:
`docs/zone/combat-anim.md`.

## 1. The fight controller (`SimpleChar_t+0x1d4`)

| off | meaning |
|---|---|
| `+8` | owner `SimpleChar_t` |
| `+0x44` | state: **1 idle, 2 fighting** (`FUN_10067fa6` [GC 0x10067fa6] also keeps the previous state at `+0x48` and emits `GlobalSignals+0x68` for the client char) |
| `+0x4c/+0x50` | target `Identity`; `FUN_100676bd` [GC 0x100676bd] = `GetDynel(+0x4c)` cast to `SimpleChar_t` |
| `+0x70/+0x74` | target of the last start call (never cleared) |
| `+0x79` | byte: "previous action pending" (`Feedback_PleaseWaitUntilPreviousAction`, outgoing side), cleared by start-fight |
| `+0x7c` | pending death cause, see §4 |
| `+0x9c`, queue | `CharSecSpecAttack` queue (§5) |
| `ctrl+0x38` / map `+0x1c` | weapon-slot objects, `FUN_10068072` [GC 0x10068072]: slots `0..=15`, `0x3d`, `0x3f` only (`ctrl+0x38`, when non-zero, answers slot 6 only) |

SimpleChar flags used below: `+0x140` = the client's own character (`n3Dynel_t::IsClientChar`), `+0x21c` = NPC, `dynel+0x138` bit 4 = dead.

### 1.1 Start fight `FUN_10069c68(target, 0)` [GC 0x10069c68]
Callers: `AttackIIR_t` apply `FUN_1007c545` [GC 0x1007c586], the `SimpleCharFullUpdate` apply `FUN_10077af2` [GC 0x10077e08] when the update carries a target
(`u.target`, object `+0xa4`; wire flag bit 10 — confirms the `[GUESS]` of dynel.md: it is the fight target), and the `CharFight_t` ctor `FUN_1007b816` [GC 0x1007b866] (target set while idle).
Order:
1. `CanAttack(target, report = 0)` `FUN_10069556` [GC 0x10069556]: false → return, nothing happens. Ported subset (`Combat::can_attack`): target must be a known
   `SimpleChar_t` (in the tree), not the attacker, not dead. Not ported (need movement/team/playfield data): `Features` flags, swimming (movement mode 4), falling,
   surrender, district fight-mode levels, team members — see combat-net.md §2.
2. Client char and crawling (mode 5, stat `0x112` bit 4 clear): text `Feedback_NeedToFightWithRangedWeapons` (not ported; movement state).
3. State != 1 → `FUN_10068b7f` (stop) first. `ctrl+0x79 = 0`.
4. Client char: `SandyInterfaceModule_t::SetCombatMusicMode(true)` (`FUN_100111e1` [GC 0x100111e1] → SandyInterface) and the GUI fight notification
   `FUN_100112fb(1, target, 0, "")` [GC 0x100112fb] (`[GUI-interface]+0x90` object, `FUN_1000482e`).
5. State = 2, target stored at `+0x4c` and `+0x70`; signals `GlobalSignals+0x6c` (target changed) / `+0x68`; animation `FUN_10061dec(…, 0xc8)` for non-NPC (`+0x21c == 0`).
6. Target resolved: **client attacker** → chat line `Feedback_Attacking` = "Attacking %s..." (name of the target) via `FUN_10012b05(0, text, 0xc)`, then if stat `Level` (0x36) < 2
   `Feedback_UseAggDefSlider` via `FUN_10058b00(key, 0)` (style 0, category 0); **attacker is not the client and the target is** → `Feedback_AttackedBy` "Attacked by %s!" (name of the attacker), category `0xc`.
   (Category `0xc` = `ColorCode` 0xff0000.) Events: `FightStarted{who, target, switched}`, `CombatMusic(true)`, `GuiFight{started: true}`, `Log`.

### 1.2 Stop fight `FUN_10068b7f(1, 0)` [GC 0x10068b7f]
Callers: `StopFightIIR_t` apply [GC 0x10079d75] (when the header is a live `SimpleChar_t`), `CharDie_t` `FUN_1007b2ba` (CharacterAction 99), start-fight step 3.
Client char (always, even when already idle): `FUN_100112fb(2, 0, 0, "")` (GUI), signal `GlobalSignals+0x1fc`, `SetCombatMusicMode(false)`, and when stat `Health` ≤ 0
`SetDeathMusicMode(true)` (`FUN_100111f2` [GC 0x100111f2]). Then, if the dynel is not dying (`+0x80 == 0`) and state != 1: animation `FUN_10061dec(…, 0x64)` for non-NPC,
state = 1, target = null, `FUN_10067fa6(1)`. Events: `GuiFight{started:false}`, `CombatMusic(false)`, `DeathMusic(true)`, `FightStopped{who}`.

## 2. Hit messages

### 2.1 `AttackInfoIIR_t` apply `FUN_1009ed0d` [GC 0x1009ed0d]
```
H = header char
if H resolves:      FUN_1006a8f3(ECX = H.ctrl)(slot, damage, value_20, unk_2c, unk_30, unk_34)
else if other is the client char:  FUN_100693a3(ECX = other.ctrl)(damage); if unk_2c: FUN_1005ae91(unk_2c) on other
```
**Roles (evidence, not the earlier doc guess):** the header is the **attacker**, the victim is the header controller's *target* (`ctrl+0x4c`), not an
independent field. Replay of the capture: for all 122 `AttackInfo` whose header controller has a target, `other` equals that target (122/122, test
`replay_counts` is built on it); `other` is only used by the fallback branch (header unknown → `other` = the hit client char).

`FUN_1006a8f3` [GC 0x1006a8f3] (`this` = attacker controller): weapon-slot object `FUN_10068072(slot)` (null → whole routine skipped, i.e. no line, no health change; when `unk_34 != 0`
the object is `FUN_100686d0(unk_34)->+0xe4` instead, §2.1.1); if the object's item has stat `0x1a` ≥ 0 it is set to `value_20` (always -1 live → never) and `FUN_10012a1e(0x65, slot, 0, 0)`
→ signal `GlobalSignals+0x90`; stores `+0x30 = damage`, `+0x2c = unk_30` in the slot object; **`FUN_1009b170(victim, attacker, damage, unk_30)`**; `FUN_1006a239(slot, 0)` (hit animation, combat-anim.md);
finally `victim.ctrl+0x7c = unk_2c`.

`FUN_1009b170` [GC 0x1009b170] (`this` = slot object): damage type `FUN_1009afde` [GC 0x1009afde] = stat `0x1b4` (`GetStat(0x1b4, 2)`) of `slotobj+0x14` (the wielded `WeaponItem_t`), else of
`slotobj+0x10` (the `DummyWeapon_t` of the item list, §2.1.1), if in `0x5a..=0x61` or `0xa8`, else `0x5a` (`log::weapon_damage_type`; a valid stat `0x153` of the *attacker* char overrides); feedback type: victim is the client char → `0x1e` (player attacker: `attacker+0x21c == 0` and `FUN_10058a05` [GC 0x10058a05] = area `Features` bit `0x800000` clear) else `0x1d`;
attacker is the client char → `0x1f`; otherwise `0x20` (`0x21` when a flagged PvP pair, `FUN_100523c3`, not ported); call `FUN_10012bd5(type, victim, damage, attacker, 0, damageType, unk_30, 0)`
(§3); **victim `Health` (27) := Health − damage** (`FUN_10062349` [GC 0x10062349] = set stat); then if `victim.ctrl+0x7c != 0` → `FUN_1005ae91(+0x7c)`.
Quirk reproduced by the port: `+0x7c` is read here *before* `FUN_1006a8f3` stores this message's `unk_2c`, so a death cause takes effect with the **next** hit on that victim.
A `SimpleItem_t` victim (`param_1` not a char): type `0x2f`, `FUN_10014e3d` (not ported).

`unk_30` = hit flags (live 3 ×131, 4 ×3): 4 → " Critical hit!" appended (`Feedback_CriticalHit`), 2 → " Glancing hit." (`Feedback_GlancingHit`), see §3. `unk_2c` = death cause (live 0 / 4).

### 2.1.1 Which item's stat `0x1b4` (root cause of the old "projectile" lines) — `combat/arms.rs`
The wire has no damage type; the old port fed `0` (→ `0x5a`) for every hit. The client reads it off the **item behind the slot**, and every item class presets `0x5b` (melee):
* `DummyWeapon_t` ctor `FUN_10082512` [GC 0x10082512]: `SetStat(0x1b4, 0x5b)`, `0x162 = 0x5b`, `0x1b8 = 0x76`, `0x126/0xd2/0xd3 = 200`, `0x1b = 10` …, then `FUN_10080169` [GC 0x10080169] loads the rdb 1000020 record
  over those defaults (`(vt+0x70)(stream)`), then the message stats; `WeaponItem_t` ctor `FUN_1009c0ca` → `FUN_1009b913` [GC 0x1009b913]: `SetStat(0x1b4, 0x5b)` (also `0x161 AnimSet = 0`, `0x1b8 = 0x77`), record + message stats on top.
  So a record without stat 436 = `DamageType` stays **melee** (martial-arts items 43712/144745: no stat 436; 296 of the 17 282 `0xc74a` item records lack it); weapons carry it (`Polished Eliminator` 0x3ca19 / `Ofab Shark Mk 5` 265090: 0x5a;
  `Baseball Bat` 121564: 0x5b; `Dull E-Blade` 122159: 0x5c; creature items `Monster Melee Primary Wpn` 56180 / `Generic Innate Weapon` 45605: 0x5b, `Generic Monster Distance Weapon` 44007: 0x5a, `BileswarmSpit_001`: 0x5d …).
* The **slot object** (0x68 bytes, ctors `FUN_1009b37e` [GC 0x1009b37e] for `DummyWeapon_t`, `FUN_1009b415` [GC 0x1009b415] for `WeaponItem_t`): `+0x10` = the `DummyWeapon_t` itself (`+0x14 = 0`), resp. `+0x10 = 0`, `+0x14` = the `WeaponItem_t`.
  Stored in the controller's map (`FUN_1006a0d1(slot, obj)` [GC 0x1006a0d1], slots `0..=15`, `0x3d`, `0x3f`) by exactly these paths:
  * **wield** (`FUN_10047873` [GC 0x10047873] on stat `0xdc`/`WeaponItemFullUpdate` apply → `FUN_1006a700` [GC 0x1006a700], `FUN_1006ad94`): `map[slot] = weapon.+0x1f4 obj`; with dynel flag `0x800` clear (CharacterAction `0xa7`, never set live) `map[0]` is dropped;
    unwield (`FUN_1006a857` → `FUN_1006a772`): `map[slot] = 0` and, when no slot `< 16` is left (`FUN_10067fbe`), `map[0] = item(key 100)->+0xe4` again;
  * **`SpecialAttackWeaponIIR_t` apply `FUN_1007989a` [GC 0x1007989a]** (only when the controller's list is still empty, `FUN_10067c2b`): `FUN_1006ac03` [GC 0x1006ac03] builds, per list entry `(lowid=f0, highid=f1, key=f3)`, an
    `ACGItem_t(f0, f1, ql = min(char Level, 0x1ff))` → `FUN_100cc04a` [GC 0x100cc04a] = a `DummyWeapon_t` of rdb record `f0` (kind `0xc74a`; `FUN_100cba71` interpolates the `0x36`-dependent stats towards `f1`), registers it under key `f3`
    in the `ctrl+0x10` map (`FUN_10069c1d`, first one wins) and in the slot map: **players** (`+0x21c == 0`): only key **100** (the martial-arts item, `43712` for the own char of the capture) → `map[0]`, iff nothing is wielded (or `map[0]` exists);
    the other entries (keys 144, 142, 1 = Brawl, Dimach …) stay special-attack items. **Creatures** (`+0x21c != 0`): entry *n* → `map[FUN_10067fbe()]`, i.e. the next free slot below 16 in list order (a wielded rifle in slot 6 counts).
  * `ctrl+0x38` (set by `FUN_10068587` from `FUN_100750fd` / `FUN_1005b016`, a weapon mounted through stat `0x296`) answers slot 6 only; not ported.
* Result for an `AttackInfo`: own char bare-handed (`slot 0`) = the martial-arts item = **melee**; wielded weapons = their record's stat 436 per hand (`slot 6` right, `8` left); a creature's `slot n` = its n-th list item (the ICC Shuttle Guards of the capture:
  `{1: projectile, 2: melee, 3: projectile, 4: melee, 5: melee, 6: rifle = projectile}`); `unk_34 != 0` = the list item with that key. In the capture 117 of the 134 `AttackInfo` find their slot object (12 have no target; the other 5 arrive *before* the
  attacker's list, which the original drops silently), with four different types (projectile / melee / chemical / radiation).
* Port: `combat::arms::Armory` (slot tables fed by `WeaponItemFullUpdate`, `SpecialAttackWeapon`, `ToClientQuit`; item stat = rdb record under the message stats, default `0x5b`), used by `combat::state::Combat::hit` and, for the chat lines, `chat::log` (`LogCtx::weapon` ←
  `Zone::world.arms`). **[GUESS]** deliberate divergence: a slot the table does not know is printed with the item default `0x5b` instead of being dropped (the port cannot prove its table complete: flag `0x800`, `ctrl+0x38`, list entries interpolated between
  `f0` and `f1` are not modelled — stat 436 is not among the interpolated stats as far as `FUN_100cba71` was read, [INFERENCE]).

### 2.2 `SpecialAttackInfoIIR_t` → `FUN_1006a9c5(slot, damage, value_28, target, special, unk_30)` [GC 0x1006a9c5] (`this` = attacker controller)
Target must resolve. Name `%s` = `GetText(0x7d3 = 2003, special)` (the localized stat name, 142 → "Brawling"; **not** `fStatToString`). Type: attacker is the client char → `0x32`; else the target is → `0x30`;
else `0x31` (`0x33` for flagged PvP). `FUN_10012bd5(type, target, damage, attacker, name, 0, 0, 0)`; `Health(target) -= damage`; `unk_30 != 0` → `FUN_1005ae91(unk_30)` on the target (immediately);
`FUN_10068320(slot, value_28)` (weapon slot stat 0x1a); `FUN_1006a239(slot, 1)` (special animation).

### 2.3 `MissedAttackInfoIIR_t` → `FUN_1006ae50(slot, value_1c, &source, &target, stat)` [GC 0x1006ae50]
`source` (`+0x20`) = **B**, the one that missed; `target` (`+0x28`) = **A**, the one missed (apply [GC 0x100a0b20] passes `&+0x20, &+0x28` in this order; the formatter's own rules agree: B client → "You tried to hit A, but missed!").
Both must be `SimpleChar_t`. `stat != 0` → `fStatToString(stat)` (English enum name, `Missing stat: N` if absent; table in `stat_names.rs`) as `extra`. `FUN_10012bd5(0x3b, A, 1, B, extra, 0, 0, 0)`; `FUN_10068320`; then `FUN_1006a8f3(slot, 0, value_1c, 0, 1, 0)` which is a no-op for the text (damage 0: the formatter returns at once).
Live: 11 messages, all `stat == 0`, none involves the capturing player → the formatter prints nothing (§3 case `0x3b`).

### 2.4 `CharSecSpecAttackIIR_t` → `FUN_10068790(ctrl; target, special)` [GC 0x10068790]
If `ctrl+0x9c == 0` and `special` is not yet queued (`FUN_10063be4` linear search) → append to the controller's deque (`FUN_1006b582`); otherwise `FUN_10058816(special)`/`FUN_1003f065(special)` (special-attack
action activation in the GUI). Port: `pending_specials` queue + `SpecialAttack` event. [UNRESOLVED] what consumes the queue.

## 3. `FUN_10012bd5` — the feedback formatter [GC 0x10012bd5] (`log::render`)
Called as `FUN_10012a1e(type, A, value, B, extra, stat, hit, ident)` (`FUN_10012a1e` is only the singleton getter; the eight stack arguments are consumed by `FUN_10012bd5`, `ret 0x20`).
Frame offsets: `+8` type, `+0xc` **A** (subject; owner of the number), `+0x10` value, `+0x14` **B** (second char), `+0x18` extra string, `+0x1c` stat (0 → `0x1b`), `+0x20` hit flags, `+0x24` identity (resolved to a dynel,
name only). `value == 0` → nothing at all. Result: `style` (`+0x10` after the switch, `0x42000001..0x42000018`; passed as the first argument of the chat signal — [UNRESOLVED] meaning), `category` (`[ebp-0x10]`,
a `ColorCode_e`, initial value 0), the `LDBformat` text, and the number.

Tail [GC 0x10014bb6..]: `hit == 4` and text non-empty → text += `" "` (`0x10154e50`) + `Feedback_CriticalHit`; `hit == 2` → `Feedback_GlancingHit`. Non-empty text →
`FUN_10012b05(style, text, category)` [GC 0x10012b05] (signal `GlobalSignals+0x17c`, the chat window). Value ≠ 0 → `sprintf("%d", value)` and the **floating number** (§6): `A+0x140` (client char) → `FUN_100044c2` (HUD) unless
`DAT_102e0640` (written by `FUN_100110ab` [GC 0x100110ab] = `arg != 0`, called from the singleton ctor `FUN_1001134a`; value not traced, assumed 0), else `FUN_10011108(A, text, category)` (world effect).

Texts: `FUN_1003807c(key)` [GC 0x1003807c] = `LDBface::GetText(0x6e = 110, key)`; the keys are string-hashed (`LDBface::ElfHash`, `TextDb::by_key`). Feeds (`LDBformat::Feed`; `n` = `Name(x)` = `(*(x+0xe8))->vt+0x34`, `dmg` = `FUN_10036adf(stat)`
[GC 0x10036adf], §7; `v` = value as `unsigned`). Per type (`log::render`), "style / category" in hex:

| type | meaning | style / cat | key (English) and feeds |
|---|---|---|---|
| 0x1a | client char lost health (StatIIR) | 02 / 15 | stat `0x1da` → `FallDamage` (v); ident → `YouWereAttackedByNanobotsFrom` (n(ident), v, dmg); `B==0 or B==A` → `AttackedByNanobotsForPointsOfDamage` (v, dmg) "You were attacked with nanobots for %u points of %s damage."; else `AttackedByForPointsOfDamage` (n(B), v, dmg) |
| 0x1b / 0x1c | other / pet lost health | 04 / 16, 03 / 1b | `TookPointsOfFallDamage` (n(A), v); `WasAttackedByNanobots` (n(A), v, dmg); `WasAttackedByForPointsOfDamage` (n(A), n(B), v, dmg); `WasAttackedByNanobotsFrom` (n(A), n(ident), v, dmg); `WasAttackedByForPointsOfDamageFrom` (n(A), n(ident), n(B), v, dmg) |
| 0x1d | client char hit (named) | 06 / 17 | `B==0 or B==A` → `WereHitForPointsOfDamage` (v); else `HitYouForPointsOfDamage` (n(B), v, dmg) "%s hit you for %u points of %s damage." |
| 0x1e | client char hit by a player | 07 / 18 | `B==0` → `PlayerHitYou` (v); else `PlayerHitYouForPointsOfDamage` (n(B), v, dmg) |
| 0x1f | client char hit A | 08 / 19 | `HitWithSpecial` (n(A), v, dmg) "You hit %s for %u points of %s damage." |
| 0x20 / 0x21 | A hit by B (others) | 0a / 1a, 09 / 1b | `B==0` → `SomethingHitOther` (n(A), v) — the third `%s` is **never fed and stays in the text** (client bug, reproduced; Dump keeps unfed conversions); else `OtherHitOther` (n(B), n(A), v, dmg) |
| 0x22 | client healed | 15 / 1c | `HealedForPoints` (v) |
| 0x24 | XP | 0b / 1d | value < 0 → `LostXP` (|v|) else `ReceivedXP` (|v|) |
| 0x3e | shadowknowledge | 0c / 1d | `LostSK` / `GainedSK` (|v|) |
| 0x45 | alien XP | 0b / 1d | value ≥ 1 → `GainedAlienXP` (v) |
| 0x30 | special hit on the client char | 07 / 17 | extra ≠ 0 → `HitYouForPointsOfDamage` (n(B), v, extra) |
| 0x31 / 0x33 | special, others | 0a / 1a, 09 / 1b | extra ≠ 0: `B==0` → `SomethingHitOther` (n(A), v, extra); else `MonsterHitWithSpecial` (n(B), n(A), v, extra) |
| 0x32 | client char's special | 08 / 19 | extra ≠ 0 → `HitWithSpecial` (n(A), v, extra) |
| 0x3b | missed | 12 or 13 / 0 | B is the client char → `YouTriedToHitMissed` (n(A)) or, with extra, `TryToAttackWithSpecialButMiss` (n(A), extra); else A is the client char → `OtherTriedToHitMissed` (n(B)) / `OtherTriesToAttackWithSpecialButMisses` (n(B), extra); else nothing. The value is zeroed (no number). |

Other types of the switch (shield, reflect, toxic, absorb, nano heal/increase, execute-nano, `0x2f` item hit …; jump table [GC 0x10014d97] + byte index table [GC 0x10014e0f]) belong to other messages and are not ported.
Category numbers 0x15.. are the chat-window categories whose colours are `GUI_COLORS` (§6). Text ids: **no numeric LDB id is used on these paths** (`text.mdb` category 100 has legacy numeric
"You hit %s for %u points of damage." (421..455) strings; the client never reads them in the code above).

`LDBformat` [LDB]: `Init` [LDB 0x100053f2] cuts the format at every `%` conversion (flags/width `"-+ 0#123456789.hlL"`, `%%` literal) and numbers the conversions from 1; `Feed(int|uint|const char*)` [LDB 0x10004c8b/0x10004da7/0x10004fd1]
snprintf's the token with the value (a `%s` fed a number prints `int_value<N>` / `uint_value<N>`) and advances the feed counter; `Dump` [LDB 0x10004885] concatenates all tokens, drops leading spaces and collapses space runs. Unfed tokens stay as written.

## 4. Death `FUN_1005ae91(cause)` [GC 0x1005ae91] (on the dying char)
Client char and cause 1..7 → `FUN_10058b00(key, 0)` [GC 0x10058b00] (style 0, category 0) with `DeathByTerminate / ReflectDamage / ShieldDamage / WeaponDamage / SpellDamage / FallDamage / LiquidDamage`
(`death_key`); `Health := 0`; non-NPC client char with stat `0x36 ≥ 1000` → `Feedback_ItemsWillBeReclaimed` fed with stat `0x22` (≤ 0 → 75). Causes live: 4 (weapon). The server's own death signal is
`CharacterAction` 99 (`Died{cause: 0}` event; stops the fight like `CharDie_t`).

## 5. `StatIIR_t` apply [GC 0x100a1aaf] (`Combat::stats`)
Pairs are applied ascending by stat id (std::map, duplicates overwritten). For each: `delta = new − current`, then
* `Health` (27): `delta < 0` → type `0x1a` (client char) / `0x1c` (pet of the local player: `FUN_100523c3`, not ported) / `0x1b`, value `|delta|`; `delta ≥ 1` and client → `0x22`;
* `0x34` XP (client) → `0x24`; `0x28` alien XP (client) → `0x45` (value `stat(0xb2) + delta` when the same message raises stat `0xa9` AlienLevel, else `delta`); `0xa9` sets DValue `got_perk`;
  `0x23d` → `0x3e`; `0x2aa..0x2ac` → `Feedback_GotPVPScore` (not ported); `0x34/0x28/0x23d` texts only for the client char;
* the value is stored (`FUN_10062349`). A health change that the preceding `AttackInfo` already applied therefore has `delta == 0` and prints nothing.
Live: 69 StatIIR messages with one pair each, of which only a few are `Health` (`215 | 190`) — see dynel.md §3.

`combat/glue.rs` forwards every `CombatEvent::Health` into `Zone.dynels` and
`Zone.character_stats`, not just the own `Zone.stats[27]`. This keeps target,
team and nametag bars on the same applied health value. Own Life remains the
pool layer's computed maximum. The compact-packet regression
`npc_hit_and_authoritative_health_reach_all_zone_views` uses the existing
`Misc::encode` Attack/AttackInfo layouts and the HealthDamage reader layout
([GC 0x100a002d], `docs/chat/log.md` wire table): NPC 50 → hit 40 →
authoritative 37, repeated authoritative 37 stays 37. The HealthDamage
`+0x18` field is the new absolute health; `+0x1c` is feedback, never a second
subtraction from the hit's already-applied health.

HealthDamage apply [GC 0x100a00c8] sets Health from `+0x18`, then calls
`FUN_1005ae91` if the cause at `+0x24` is nonzero. NewLevel apply
[GC 0x10075a0c, assembly 0x10075a4d..0x10075a92] sets
`Level=f[0]`, `XP=f[2]`, `IP=f[1]`, `LastXP=f[3]`,
`NextXP=f[4]`, `XPKillRange=f[6]` for every SimpleChar. The own-character
gate is later at `0x10075aab`; positive `TitleLevel=f[5]` is applied inside
that gate (`0x10075cd3..0x10075ce1`). The combat layer applies these absolute
values without synthesizing StatIIR XP feedback; chat retains its existing
NewLevel formatting. The same compact-packet regression covers both own and
NPC level/range updates and HealthDamage-before-death ordering.

## 6. Floating numbers
Created by the formatter tail for every message with a non-zero value; the number is `sprintf("%d", value)` (signed, no sign character added, e.g. XP loss `-5`), the colour is the line's category.

**HUD path (client char)** — `FUN_100044c2(value, category)` [GC 0x100044c2] = `AFCM::Send(0x19, 0x3b, value, category)` → `RenderTextModule_t::DamageTextMessage` [GUI 0x1004ae2f] → `RenderText_t` ctor `FUN_1004ac94` [GUI 0x1004ac94]:
text `"%d"`, **font `FontID_e 2`**, position centred on `(50 + r, DAT_102761c0 − 20)` (`r = round(rand()/32767 · 40 − 20)` from the second of two `rand()` calls; `DAT_102761c0` [UNRESOLVED]), colour = `ATextString_t::InsertColor` escape `0x10 <category>` →
`FontSystem_t::GetColor(category)` [GUI 0x1012e2d1] (`GUI_COLORS`, built by `SetColorsIntoMap` [GUI 0x1012e316]), **life 2.3 s** (`DAT_101b1198`), flags = 1 (move only). Per frame `FUN_1004aa4f` [GUI 0x1004aa4f]: `remaining −= dt`, `y = y0 − 70 · (life − remaining)/life` (`70.0` = `DAT_101b1190`;
**30.4 px/s upwards**), removed when `remaining ≤ 0`; the alpha branch (`255 · remaining/life`, bit 1) exists but is not enabled by `DamageTextMessage` → no fade.

**World path (everything else)** — `FUN_10011108(A, text, category)` [GC 0x10011108]: `_EffectHandler_t::CreateEffect2(0x2f5a, dynel A, 0)`, `SetColor(table[category] | 0xff000000)` (`WORLD_COLORS` = `DAT_10156b08`, entries ≥ 0x1e are 0 =
black, category > 0x52 → `0xffff0000`), `SetText(text)`. Effect `0x2f5a` = record in `Setupf/gfxtweak.bin` (`i32 id, i32 type, i32 n, n × i32`; 2687 records; this one: type **0x7de** = `_GfxControlFont_t`,
ctor `FUN_100df42d` [GC 0x100df42d], params `FUN_100decc3` [GC 0x100decc3], 32 params at file offset 22128): **duration 1.3 s** (param 0x1a), glyph size `0.01` (param 0x1c), **rise `0.4 · elapsed`** (param 0x1d; `FUN_100deb04` [GC 0x100deb04] sets the billboard offset `(0, 0.4 · t, 0)`
every `Process`), bitmap-font **material 40** (param 9, `_EffectHandler_t::MMGetMaterial`; glyph advances `DAT_102c460a`, 3 bytes per char; `GfxVisualDiaBill`, render priority 7), constant colour (`SetColor` → start = stop colour).
Anchored with `InitDynelTemplate` on the dynel (locator template params 0..7: `7, 0,0,0,0,0,0, 1000` …); the exact height above the head is **[UNRESOLVED]** (locator offset not decoded).
Exposed as data: `log::HUD_NUMBER`, `log::WORLD_NUMBER`, `floating_number(space, category)` (`FloatingNumber{color, rise_speed, life, font, space}`).

## 7. Tables
* Damage type names `FUN_10036adf` (map built [GC 0x10033a76..0x10033ba7]): `0x5a` projectile, `0x5b` melee, `0x5c` energy, `0x5d` chemical, `0x5e` radiation, `0x5f` cold, `0x60` poison, `0x61` fire, `0xa8` nano, `0x1b` "unknown"; else `Missing damagetype: N`.
  The hit message carries no damage type: it comes from stat `0x1b4` of the item behind the attacker's weapon slot (§2.1.1; item default `0x5b` melee, `FUN_1009afde` maps anything else outside the table to `0x5a`).
* `fStatToString` = `stat_names.rs` (520 ids, `FUN_1002f009` [GC 0x1002f009]; 142 "Brawl", 0x1b "Health", 1 "Life").
* Stat ids used: `Health` 27, `MaxHealth` 1, `Level` 0x36, `XP` 0x34, `AlienXP` 0x28, `AlienLevel` 0xa9, `ShadowKnowledge` 0x23d, `VisualFlags` 0x2a1.

## 8. Capture check (`state.rs` tests)
Observer view (own char 25988 "Testy" does not fight): 134 `AttackInfo` → **122 hits** (12 have a header whose controller has no target at that time: the `Attack`/full-update start was refused by `CanAttack`
because the target dynel had not been announced yet, or no `Attack` was sent; the client behaves the same and prints nothing), 11 misses (no text: bystanders), 3 special hits, 3 `CharSecSpecAttack`,
127 starts / 118 stops of a fight (some starts are target switches), 15 deaths (`CharacterAction` 99). Lines with the real `text.mdb` strings, e.g. `Scout - Jaax'Sinuh hit ICC Shuttle Guard for 17 points of melee damage.` (this first hit arrives before the guard's `SpecialAttackWeapon` list, so the port prints the item default; the original drops it)
(type `0x20`: B = the attacker named, style `0x4200000a`, category `0x1a`), two `... Critical hit!` lines (the third `unk_30 == 4` AttackInfo is one of the 12 dropped), three `Bergdoktor hit <NPC> for N points of Brawling damage.`
Replayed as player 33402 ("Bergdoktor", who attacks in the stream) the client-char paths fire: combat music, `Attacking %s...`, `You hit %s for %u points of <type> damage.` (type from the slot item), HUD numbers.

## 9. Unresolved
* Height / locator of the world number above the head; material 40 texture; what `DAT_102761c0` is (HUD number anchor y) and `DAT_102e0640`.
* Meaning of the first chat-signal argument `0x42000001..0x18` (`GlobalSignals+0x17c` receiver in GUI.dll not traced).
* The `ctrl+0x38` mounted-weapon override and the dynel flag `0x800` (CharacterAction `0xa7`, not tracked); whether the original really drops hits whose slot object is missing live (the port prints them with the melee default);
  PvP-flag helper `FUN_100523c3`; `Features` flag `0x800000` source. (The weapon-slot objects and the damage type of a hit are resolved, §2.1.1.)
* Consumer of the `CharSecSpecAttack` queue; `Feedback_GotPVPScore` (stats `0x2aa..0x2ac`), types of the formatter not reachable from these messages.
