# Attack / Reload / Nano / Item HUD timer bars

Code: `play/combat/arms.rs` (`round_delay`, `item_round_seconds`, `attack_slots`), `play/dynels.rs` (`update_attack_bars`,
`restart_attack_bar`, `attack_bar_progress`, `start_reload_bar`, `reload_bar_progress`, `nano_bar_progress`,
`nano_cast_delay`), `play/combat/glue.rs` (per-frame wiring), `play/hud_use.rs` (Reload trigger), `play/hud_stats.rs`
(`update_attack_timers`, `update_reload_timer`, `update_nano_timer`, `free_timer_slot`), `play/hud_timerbar.rs`
(`COLOR_*`/`LABEL_*` per kind, `pos_slot`).

RE tooling: three new scripts were added to `~/aomac-ghidra/scripts` for this investigation (reusable): `FindScalar.java`
(scan every instruction for a literal operand value), `DumpAsm.java` (raw disassembly of an address range, needed
whenever the decompiler drops register-passed arguments), `ListSymRange.java` (every symbol in an address range, used
to resolve an unnamed vtable slot to its owning class by position).

## 1. The widget and the `TimerEntry_t` construction chain (hard evidence)

`hud_timerbar.rs` already carries the faithful RE of the generic timer-bar widget (`TimerBarBase_c`/`GameTimerBar_c`,
GUI.dll) from the "Logout" countdown and the Item wear/unwear delay bar. `TimerSystemModule_t::StartTimerBar`
[GUI 0x10051a27] colours/labels the bar by `TimerEntry_t+0x14` kind: 1 Attack (`0xaaffaa` "Attack"), 2 Special
(`0xaaffff` "Special"), 3 Nano (`0xffaaaa` "Nano program"), 4 Item (`0xaaaaff` "Item"), 5 Reload (untinted `0xffffff`
"Reload") -- English strings read directly from the installed `cd_image/text/text.mdb` (`TextDb::by_key(10000, key)`).
Position: `TimerSystemModule_t::FindNextFreePos` [GUI 0x10051779] stacks concurrent bars 20 px apart from
`IPoint(0x28, (n+2)*0x14)`, taking the lowest free slot (`hud_timerbar::pos_slot`, `HudStats::free_timer_slot`).

**Every `TimerEntry_t` in Gamecode.dll is unambiguously identifiable**, independent of which kind it ends up as: its
shared_ptr control block (`FUN_1003d9ec`) is built with `boost::detail::sp_counted_impl_p<TimerEntry_t>::vftable`
(RTTI-typed, not a guess -- `~/aomac-ghidra/re.sh Gamecode DumpRefs.java "TimerEntry_t"` finds this exact mangled
name). `FUN_1003da5d` (assign + wrap) has **three** callers: `FUN_1003d7ce`, plus weapon-slot constructors
`FUN_1009b37e` (dummy weapon, called by `FUN_10082512`) and `FUN_1009b415` (wielded weapon, called by
`FUN_1009c0ca`). The latter two retain an entry in the slot object rather than constructing one on each start.
The earlier assertion that `FUN_1003d7ce` was its only caller missed the Attack path.
`FUN_1003d7ce` is the construct-and-start helper; `FUN_1003d6b1` registers/starts an existing entry, sets its
kind at `+0x14`, and calls `FUN_1003d85b` to emit the start signal for the client character.
`DumpCallers 1003d7ce` finds six call sites. The kind argument must be checked at each caller:
the item-use state machine `FUN_1003b947` passes literal **4 (Item)**, not 2 (Special).
The generic GUI's kind-2 label alone does not identify a special-attack trigger.

## 2. Attack (kind 1): confirmed-fight trigger traced

**Starting an attack request is not starting its timer.** Gamecode `FUN_10067c34` only sets pending byte
`ctrl+0x79`, constructs `AttackIIR_t`, and calls `SendIIRToObservers`; it has no timer call.
Received `AttackIIR_t::Execute` (`FUN_1007c545`) calls `FUN_10069c68(target, 0)`. After `CanAttack`
(`FUN_10069556`) accepts the target, this calls `FUN_100681e5`, which visits the weapon slots and calls
`FUN_10067de8(slot, force)`. That function gets the slot's retained entry with `FUN_1006b0a3` and starts it
through **`FUN_1003d6b1(..., kind = 1)`**. `DumpAsm 10067de8 10067e8d` pins `EBX = 1`
(`0x10067e1c`/`0x10067e1e`), `PUSH EBX` (`0x10067e60`), then the recharge/attack durations,
the entry shared pointer, and the call at `0x10067e88`. At `0x10069d9d`–`0x10069db6`, fight setup
forces a fresh start when no kind-1 timer exists (`FUN_1003ceb9`); the slot helper otherwise respects its
retained entry's flags. Opening the first entry belongs to accepted fight setup, before the first
`AttackInfoIIR_t`, not a sent request or its first hit.
The same setup is refreshed during fighting equipment changes (`FUN_1006a700`, `FUN_1006a772`).
`StopFight`'s `FUN_10068b7f` emits the own character's timer-stop signal at `GlobalSignals+0x1fc`.

The port already follows that trigger end to end: keys/action shortcuts reach `Module::default_attack`,
which sets `AttackGate` and queues the request only. `Combat::on_frame` starts combat for received `Attack`
(or a full update's active target); `Module::attacking()` reads that confirmed state. `combat/glue.rs`
passes it to `Dynels::update_attack_bars`; `HudStats::update_attack_timers` mirrors those entries into windows.
Server refusal actions `0x93`/`0x76` do not start combat or bars. No behavioral send-vs-confirm change is
warranted by this trace. The reported refused-Attack association was an inference; its deleted logs/screenshots
cannot establish otherwise, and the surviving successful fights do not supply retail widget timing.

Offscreen live acceptance (2026-10-15, Aomacfixr 33588, ICC4582) corroborates the ordering:
sent Attack at **18017 ms**, received Attack at **18222 ms**, first own hit at **19203 ms**.
Before sending, `attacking=false`, Attack entries empty; the post-confirmation/pre-hit sample had
`attacking=true`, slot6 progress **0.35511562**, total **2.2333333 s**, and the inspected frame showed
Attack. After the second hit/StopFight it was absent. No rejected server request was exercised live;
the refusal regression covers that path. Relevant original frames are retained in
`docs/captures/zone_tab_rifle_fixr_ithaca.rec` (a combat-only subset, no login credentials).

**Remaining reconstruction, not established by this trigger trace:** the port displays a summed
initiative-scaled `AttackDelay`(294) + `RechargeDelay`(210) cycle (`item_round_seconds`) and resets it on
each own `CombatEvent::Hit`/`Miss`. Retail `FUN_10067de8` separately obtains those two durations through
`FUN_10067bef` -> `FUN_1006772b` and `FUN_10067c0d` -> `FUN_10067849`; its entry flags and cycle update
are not yet traced into equivalent HUD progress. This investigation establishes the opening trigger, not
the phase/restart model. The former special-attack corroboration was wrong: `FUN_1003b947` is item use (§5).

## 3. Reload (kind 5): native trigger and Solar rifle ammunition evidence

### Solar-Powered Assault Rifle does not have a finite clip

Read-only inspection of installed `cd_image/rdb.db`, table `rdb_1000020`, id **121569**
("Solar-Powered Assault Rifle", 705-byte record) establishes:

| Stat | Value | Little-endian stat-pair offset in record |
| --- | --- | --- |
| `AmmoMag` (212 / `0xd4`) | **-1** (no finite magazine) | `0x84` |
| `AmmoType` (420 / `0x1a4`) | **10** (`SelfSupplied`) | `0xc4` |
| `AttackDelay` (294 / `0x126`) | 100 (1.00 s raw) | `0xac` |
| `RechargeDelay` (210 / `0xd2`) | 150 (1.50 s raw) | `0x74` |
| Current ammo (26 / `0x1a`) | absent from the static template | — |

The record's own description says its slugs are made by nanobots eating from its casing.
The client's ammunition display distinguishes ammo type 10 as `SelfSupplied`
(`chat/item_info.rs::weapon`); `ammo` reads capacity from **212**, not Health stat 27.
Thus there is **no finite shots-per-clip count** to exhaust for this weapon, not a large conventional
clip that merely needed more shots. Do not infer consumption from its bullet muzzle effect or firing animation.

Surviving capture `docs/captures/zone_wear_rifle_borealis.rec` supplies the instance values:
at **19373, 25452 and 28594 ms** (lines 3, 7, 13), weapon identity `{0xc74a, 13948944}`,
held by character 33512, has `StaticInstance` 23 = **121569** and current ammo 26 = **0**.
Those are wear/unwear/re-wear snapshots, **not** a shot-depletion experiment.
`zone_special_burst_fixr_ithaca.rec` at 17690 ms records character 33588's one Burst result
with ammo field **-1**, not a clip decrement. Neither capture contains `ReloadIIR_t`.
The deleted real-window logs/screenshots cannot establish a shot count or an omitted reload;
the reported Island Reet kill alone does not imply this self-supplied weapon should reload.
Runtime behavior is intentionally unchanged for this report.

### Native Reload message and timer

* **Message class**: `ReloadIIR_t` **0x26515E61** is decoded as `InventoryMsg::Reload`.
  Fresh Gamecode `ReadSubClass` **0x100766e1**, corroborated by writer **0x1007673a**, establishes a
  **28-byte body**: ammo-source `Identity`, weapon `Identity`, remaining-source-count `i32`,
  new-clip-count `i32`, and flag **`i32`** (nonzero becomes true, despite the in-memory one-byte bool).
  The common 13-byte N3 header is separate. Constructor **0x100767ca** corroborates the object fields.
* Apply **0x10076780** resolves the header character, then calls **0x10069274** with both identities
  and counts. The ammo-source is updated/removed through **0x10049036** / **0x10049073** according to
  the flag. If weapon current ammo 26 is nonnegative, it sets that stat to the new count,
  calls **0x10068a89(weapon)** and prints `Feedback_Reloading`.
* `FUN_10068a89` is one of the six `FUN_1003d7ce` callers and explicitly passes literal **kind 5**.
  Its first instructions (`~/aomac-ghidra re.sh Gamecode DumpAsm.java 10068a89 10068ac7`) push `(0xd2, 2)` into the item's
  effective-stat accessor (`(**vtable+0x3c)(statId, mode)`, the same accessor used for `(0x1a,2)` ammo and `(0x126,2)`
  AttackDelay elsewhere) before constructing the `TimerEntry_t` -- i.e. **the duration passed to kind 5 is the
  weapon's own `RechargeDelay` stat (210/0xd2), alone, with no `AttackDelay` term and no initiative scaling** (no
  `GameTime_t`/initiative-slope code runs anywhere in this function). `Dynels::start_reload_bar` uses
  `item.recharge as f32 / 100.0` (the hundredths-to-seconds divisor is `StartTimerBar`'s own kind-4 branch constant
  `_DAT_101adfa8 = 100.0`, read directly from GUI.dll).
* **Native trigger is resolved:** `DumpCallers 10068a89` finds only **0x1006931b** inside the
  Reload apply helper **0x10069274**. The manual Reload action **0x6e**, dispatched by **0x1004256c**,
  reaches sender **0x1006949a**, which only gates and sends `CharacterActionIIR_t` action **0xd2**.
  It does **not** start the timer on send. Ordinary hit application **0x1006a8f3** assigns the server's
  ammo field to stat 26 for ammo-bearing weapons; it does not locally decrement a clip or send Reload.
* **Port implementation (2026-10-15):** the manual action now only sends action 0xd2; no predicted timer.
  Server acknowledgement replaces the source stack count or removes the source, then resolves the exact
  weapon identity (including bag weapons), updates nonnegative Ammo26, starts its raw RechargeDelay timer,
  and emits `Feedback_Reloading`. No template/item exceptions and no fabricated automatic reload.
  Fresh `FUN_10048829` resolves own pages, registered container high words, bank and temporary trade inventory;
  `FUN_1002ad45` stores an unsigned 16-bit count and converts values outside 0..65535 to zero.
  Regression `reload_wire_counts_and_i32_flag` covers the 28-byte body, noncanonical true, truncation and
  trailing bytes; `reload_ack_updates_source_and_exact_weapon_before_feedback` feeds actual framed synthetic
  bytes through `Zone::on_frame`, covering dual-wield exact selection, source replacement/removal,
  signed container registration, count bounds and negative-Ammo rejection. These are native-derived
  synthetic checks, **not** a finite-magazine live capture; that independent server acceptance remains unverified.
* **Offscreen live evidence (2026-10-15):** Aomacfixr 33588 wielded template **121569**, weapon
  **24763172**. Both received weapon updates at **5291 ms** carried current ammo26 **0**.
  The two normal own hits at **19203 / 21389 ms** dealt **11 / 2** damage in slot6 and each supplied
  ammo field **-1**; the 12-HP Beach Leet died. Across the full recorded 86.81 s session there were
  **zero received ReloadIIR (0x26515E61)**. Reload was absent from the inspected fight/post-kill
  frames and timer samples. The relevant weapon/start/hit/stop/death/quit frames are retained in
  `zone_tab_rifle_fixr_ithaca.rec`; that subset alone is not the full-session absence proof.
  With AmmoMag=-1 and SelfSupplied=10, no automatic Reload was due; runtime is unchanged.

## 4. Nano (kind 3): reuses an already-correct, independently-traced formula

* **Trigger**: `CastNanoSpellIIR_t` 0x25314D6D apply [GC 0x10072306] -> `FUN_10051754(spell, &target, 1, flag, 1, 1,
  &source, 0, 0)` (the exact call already documented in `docs/zone/dynel.md` §5, independently of this
  investigation) -> tail `FUN_10050ed9`, which creates the cast timer for an active **non-instant** cast.
  Its address range (`asm 10050f85-10051064`) is already `nano_cast_delay`'s GC evidence. `DumpCallers 10051754`
  finds the nano apply (0x10072306), the `CharacterActionIIR_t` dispatch's own case for `identity_b`-bearing nano
  actions, and `FUN_100a9543`/`FUN_10071be4` (a `BuffIIR_c`-adjacent context, same shared helper reused for buffs).
* **Duration**: `nano_cast_delay(template.stat(0x126), template.stat(0x20b), stat(who, 0x95), stat(who, 0x33),
  template.stat(0))` was *already* ported (`dynels.rs`, predating this change) and already drives the nano casting
  animation's `NanoCast.remaining` countdown -- nano templates reuse the item stat schema, `AttackDelay`(294) as
  "cast time" and `AttackDelayCap`(523, stat `0x20b`) as its floor, scaled by the caster's own stat `0x95`
  initiative and `0x33` AggDef. `NanoCast` now also stores `total` (the value `remaining` started at), so
  `Dynels::nano_bar_progress(own)` can report `(1 - remaining/total, total)` without recomputing the formula.
* **Port**: `hud_stats::update_nano_timer` opens/updates/closes the "Nano program" bar for the own character's
  active, non-instant, unreleased cast. Regression `combat_timer_bars_stack_and_show_mid_fight` casts the real
  installed Body Boost (template 29091, the same fixture `dynels.rs`'s own nano tests already use) through the real
  `CastNanoSpellIIR_t` apply path (`Dynels::on_message` + `nano_visual_frame`) and checks the bar opens with label
  "Nano program" at a free stacking slot alongside the Attack/Reload bars, shown in the offscreen screenshot.

## 5. Item (kind 4), not a special-attack timer (bug 12)

Fresh `Gamecode DumpFn 1003b947` shows **both** `FUN_1005863c(..., 4)` and
`FUN_1003d7ce(..., 4)`. `DumpAsm` pins the kind-4 pushes at `0x1003b9cf` before the timer call at
`0x1003b9e5`, and `0x1003ba5b` before the timer call at `0x1003ba71`.
This is the item-use state machine (also traced in `interact.md`), not a
weapon-special timer. The previous documentation incorrectly inferred a Special bar and a special-list/
wielded-weapon timer fallback from this function. That inference and the timer-only implementation introduced
by `4b054bf` have been removed from both hotbar/action-menu and keyboard dispatch.

`docs/captures/zone_special_burst_fixr_ithaca.rec` remains network evidence: the player sends
`CharSecSpecAttackIIR_t {target: 1080207, special: 148}` and receives
`SpecialAttackInfoIIR_t {slot: 6, damage: 43, target: 1080207, special: 148}`.
The capture proves that Burst exchange, not a timer widget or its duration. No timer fallback can be inferred
from Burst's absence in the captured special-weapon item list.

Actual special attacks, their animation/item resolution and the hotbar icon-fader recharge feed
(`CharacterActionIIR_t` action `0x14`, `hud_bar.rs`/`hud_special.rs`) remain intact.
The existing kind-4 Item wear/unwear bar remains intact as well.

## 6. Regressions

* `combat::arms::tests::round_delay_matches_the_item_info_dps_cell` / `item_round_seconds_sums_attack_and_recharge_in_
  hundredths` / `attack_slots_follow_wielded_hands`: the Attack-bar formula against the tooltip's own fixture.
* `combat::arms::tests::captured_solar_rifle_keeps_zero_ammo_across_wear_unwear` checks all three real
  rifle replications (template, holder, weapon identity, ammo zero and absence of a Reload message).
  `solar_assault_rifle_has_self_supplied_ammo_and_no_finite_clip` pins the installed template's
  `AmmoType = 10`, `AmmoMag = -1`, absent static current count and exact raw delays, then checks the
  same template under the capture's ammo-zero override through `Armory`. The latter skips without
  the installed database; the capture-only regression does not.
* `dynels::tests::attack_and_reload_bars_track_hands_swings_and_fight_state`: the state machine (open on fight
  start, restart on swing, ignore an untracked slot, clear on fight stop) and the exact Reload duration (raw
  `RechargeDelay`/100, pinned to `5.0` for stat `0xd2 = 500`, no initiative term, no `AttackDelay` sum).
* `combat::glue::health_tests::captured_fight_drives_the_attack_bar`: replays `docs/captures/zone_fight_ithaca.rec`
  through a real `Module` + `Zone`, checking that the outgoing Attack has no bar, the received own Attack
  opens it before any own hit/miss, and the existing swing/reset/stop reconstruction remains covered.
* `combat::module::tests::attack_key_round_trip` / `server_refusal_prints_text_and_clears_the_gate`: exercise
  actual command send/pending, confirmation before any hit, stop, and refusal through the timer state.
  These assertions use message-stat weapons and do not require the installed client database.
  The capture regression above does require that database for its existing bare-hands item fixture.
* `combat::glue::health_tests::captured_burst_matches_the_server_result`: preserves the capture's matching sent
  `CharSecSpecAttackIIR_t`/received `SpecialAttackInfoIIR_t` special ids without asserting timer behaviour.
* `hud_stats::tests::combat_timer_bars_stack_and_show_mid_fight`: dispatches actual Burst/Brawl actions and checks
  that only two Attack bars, Reload and a real Body-Boost-driven Nano bar occupy the stack, with native labels
  and distinct slots; no Special timer is added. Its existing offscreen screenshot is `combat-timer-bars.png`.
* `hud_stats::tests::ncu_belt_equip_starts_and_counts_down_the_item_timer_bar`: retains Item-bar coverage.
