# Zone N3: combat — outgoing messages, attack preconditions, timing, controls

Code: `crates/ao-net/src/n3/combat.rs`. Addresses are VAs of the 32-bit DLLs in `~/Games/ProjectRubiKa/client`
(`[GC]` Gamecode.dll, `[N3]` N3.dll, `[GUI]` GUI.dll). Receive-side layouts: `docs/zone/misc.md`. Capture used: `docs/captures/zone_ithaca.rec`.
Tags: **[RE]** read from decompile/disassembly, **[obs]** observed in a capture, **[INFERENCE]** not proven.

## 1. Outgoing messages (wire-exact)

Header of every IIR: `u32 key, Identity (i32 kind, i32 instance), u8 to_be_passed_on, body` (`n3InfoItemRemote_t::Write` [N3 0x100098f4]
writes `(this+0xc == 1)`). The header identity is the character's own `Identity_t` (`SimpleChar+0x14` = `{0xC350, char_id}`).
Frame wrapper is `outgoing::n3_frame` (ptype 10, receiver 2).

| fn | class (key) | constructor / writer | body | bytes |
|---|---|---|---|---|
| `combat::attack(char, target, flag)` | `AttackIIR_t` `0x28494070` | ctor `FUN_1007c641` [GC], writer `FUN_1007c5c0`: `Identity` @+0x18, `u8` @+0x20 | target `Identity`, `i8 flag` | 22 |
| `combat::stop_fight(char)` | `StopFightIIR_t` `0x4A41203E` | ctor `FUN_10079dd8`, writer `FUN_10079d95`: `operator<<(uint)(byte @+0x18)` | `i32 1` (`00 00 00 01`) | 17 |
| `combat::sec_spec_attack(char, target, stat)` | `CharSecSpecAttackIIR_t` `0x51492120` | ctor `FUN_1007280c`, writer `FUN_100727a3`: `Identity` @+0x18, `i32` @+0x20 | target `Identity`, `i32 stat` | 25 |

* **`to_be_passed_on` is 0 for all three.** All three constructors end in `n3InfoItemRemote_t::ClearToBePassedOn` [N3 0x100097fe] (`this[0xc] = 0`). [RE]
  This differs from `CharInPlay`/`CharDCMove` (flag 1, `outgoing::iir`), which is why `combat.rs` has its own 5-line header writer.
* **Round trip [obs]:** `combat::tests::encoders_reproduce_captured_relays` re-encodes every captured server relay of the three classes
  (all `Attack`, all 128 `StopFight{1}`, all 3 `CharSecSpecAttack`) from the decoded fields and gets the payload byte-for-byte: the relays carry
  header flag 0 and the same body layout. No client-originated combat frame exists in any capture (the capture client never attacked), so the
  client side is established from the constructors, not from client bytes.
* `flag` of `AttackIIR_t`: `N3Msg_DefaultAttack` [GC 0x10027fbc] passes 2 when the character was already fighting (target switch) and 0 otherwise;
  every captured relay has 0. [RE+obs]
* Special-attack `stat` values: `N3Msg_SecondarySpecialAttack` [GC 0x10028071] is called from `FUN_1004256c` with 142 (Brawl), 144 (Dimach) and
  with the action id itself for 121 BowSpecialAttack, 146 SneakAttack, 147 FastAttack, 148 Burst, 150 FlingShot, 151 AimedShot, 167 FullAuto, 489 Backstab. The 3 captured relays are 142.

### 1.1 Does the client apply anything before the server echo?

No. `n3Dynel_t::SendIIRToObservers` [N3 0x10004e43]: if the dynel is in the tree (`this[0x6b] != 0`) it calls `n3EngineClient_t::SendIIRToServer`
[N3 0x10007762] (`Write` + `ACE_Data_Block`, send) and nothing else; otherwise it prints "ERROR: an IIR … was attempted sent to dynel … not in the tree"
and destroys the IIR. [RE] The local fight state changes only when the server's relay is executed:
`AttackIIR_t` execute `FUN_1007c545` [GC] → `FUN_10069c68` (start fight: sets state `+0x44 = 2`, target `+0x4c/+0x50`, clears the `+0x79` guard, log text,
`GlobalSignals` +0x6c) and `StopFightIIR_t` execute `FUN_10079d75` → `FUN_10068b7f(1,0)` (stop fight: state `+0x44 = 1`, target cleared, signals +0x1fc/+0x6c). [RE]
Consequently the attack key pressed while fighting sends `StopFight` and the character stays "fighting" until the relay comes back.

## 2. Preconditions: `can_attack` (port of `FUN_10069556` [GC])

`FUN_10069556(controller /*ECX, SimpleChar+0x1d4*/, Identity target, bool report)` (stack args `[ebp+8]`, `[ebp+0xc]`; verified in the disassembly:
`N3Msg_DefaultAttack` passes report=1, `FUN_10069c68` passes 0). `controller+8` = the character, `+0x44` fight state (1 idle), `+0x4c/+0x50` target.
Feedback texts are printed with `FUN_10058b00(key)` only when `char+0x140` (is client char) and `report` (the exceptions are marked).

Order (all [RE], `combat::can_attack`):

1. If `controller+0x38 == 0` and the controller map `+0x1c` holds key `0x3d` or `0x3f` with non-zero value, and `VisualFlags` (673) `& 0x20` → `false`, silent.
   (What the map/`+0x38` are is unresolved; field `Attacker::status_blocked`.)
2. `GetDynel(target)`; dynamic casts to `SimpleChar_t` and `SimpleItem_t`.
   * `SimpleItem`: if `vtable+0x14(0x10000000)` → `FUN_100811ac(0xb, …) != 0`; else `Feedback_YouCantAttackThisItem`.
   * not a `SimpleChar`, or not `IsInTree` → `false`, silent.
3. target == self → `Feedback_YouCantAttackYourself`.
4. `Features`(224)`& 0x20000000` of the **target's** area object (`FUN_10044b6e(this = target+0x1ec, mask)` reads stat `0xe0` of the dynel at `obj+0x40`):
   `false`; `Feedback_CantbeAttacked` printed unconditionally unless `char+0x21c`.
5. GM override: (attacker area `Features & 0x80000000` and attacker `GmLevel`(215) ≠ 0) or the same for the target → `true`.
6. If `!char+0x21c` and movement mode == 4 (`FUN_100574e2`=`char+0x50` vehicle → `FUN_1006ed98`=`+0x178` controller → `FUN_100704e6`=`+4` mode) → `Feedback_YouCantAttackWhileSwimming`.
7. Attacker area `Features & 0x400` or mode == 7 ([INFERENCE] falling: `FUN_1006dd0c` calls `Vehicle_t::EnableFalling` for mode 7) → `Feedback_YoureUnableToAttack`.
8. Target `dynel+0x138` bit 4 (dead): `Health`(27) ≠ 0 → `false` silent, else `Feedback_TargetIsAlreadyDead`.
9. Target `char+0x21c` and `NPCIsSurrendering`(449) ∈ {1,2}; `NPCSurrenderInstance`(451) names the surrender partner:
   partner == self and state 2 → `Feedback_YouAcceptedSurrender`; partner ≠ self: state 1 → `Feedback_TargetIsSurrendering`, state 2 → `Feedback_TargetHasSurrendered`; otherwise fall through.
10. Tail (0x1006991e): the target's own fight target is the attacker → `true`. Else both district fight-mode levels (`FUN_1003e1d0` [GC], default 2, from the playfield's `FightModeHandler`/`PlayfieldDistrictInfo`; override `char+0xf4` of the state object when `char+0x13a & 1`);
    target level < attacker level prints `Feedback_TargetIsInADistrictWithHigherSuppression` **but does not deny**. If `char+0x21c` and `Side`(33)==3 (neutral): allowed iff both levels ≠ 0. Else if the attacker has a team record (`char+0x1e0`) and the target is a member of the active team or of any of the 6 sub-teams
    (`FUN_1006581f`/`FUN_10065865`, team identity kind `0xDEA9`) → `false` (+ `FUN_10068d6b(5)`, a CharacterAction id `0x76` request, when `report`; not modelled); otherwise `true`.

`FormatFeedbackIIR` category `0x23` (sub-codes 1…14) prints the server-side refusal texts, in the order the strings are pushed in `FUN_1005d0d8` [GC]
(`CombatIsNotPossibleInThisDistrict`, `AttackNotAllowedSinceYouAreOnSameSide`, `YouCannotAttackThisPlayerTooFarAwayInLevel`, `…ThisTowerTooFarAwayInLevel`, `YouCannotAttackYourPet`,
`NotAllowedToAttackTeamMembers`, `PvpNotAllowedInThisDistrict`, `PvpNotAllowedSinceYouAreNeutral`, `PvpNotAllowedSinceYourTeamIsNeutral`, `CantAttackTargetIsInPvpGrace`, `DefenseShieldEnabled`,
`CantAttackTargetYouAreInMixedTeam`, `TowersCanOnlyBeAttackedWhenGaslevelBelow75`, `CantAttackTargetYouAreInMixedTeamBattlestation`). [INFERENCE for the sub-code → text pairing: the switch table was not walked; only the string order is known.]

### 2.1 `N3Msg_DefaultAttack(target, switch)` (`combat::default_attack`)

1. `same = (target == controller.target)`; `was_fighting = state != 1`.
2. If fighting → `N3Msg_StopAttack` [GC 0x10027f55] (sends `StopFightIIR_t` only when `N3Msg_IsAttacking` and `char+0x21d == 0`).
3. Movement mode 4 (swimming) → `Feedback_CantAttackInThisState`.
4. State is still "fighting" (the echo has not arrived): `!switch` → return; `switch && same` → return. ⇒ attack key = toggle; `SwitchTarget` onto the same target = stop.
5. `can_attack(target, report = 1)` else `Feedback_UnableToAttackTarget`.
6. `FUN_10067c34(target, was_fighting ? 2 : 0)`.

`N3Msg_SwitchTarget` [GC 0x10028297] = `DefaultAttack(target, true)`. The `PerformSpecialAction` caller (`FUN_1004256c`) uses `switch = false`.

## 3. The `+0x79` guard ("please wait until previous action") and timing

Byte `+0x79` of the `CharFight` controller. All writers of `[reg+0x79]` in Gamecode.dll (instruction scan for `+ 0x79]`: six hits) [RE]:

| addr | fn | effect |
|---|---|---|
| 0x10067c6d | `FUN_10067c34` | **set 1** when sending `AttackIIR_t`; guard `+0x79 == 0 && char+0x21d == 0`, else `Feedback_PleaseWaitUntilPreviousAction` |
| 0x10069cdb | `FUN_10069c68` (apply attack echo) | clear, but only after `can_attack(.., report=0)` succeeded; callers: `AttackIIR_t` execute `FUN_1007c545`, `CharFight` ctor `FUN_1007b816` (restore on construct), `SimpleCharFullUpdate` apply `FUN_10077af2` (fight target at `+0xa4`) |
| 0x1005d93e, 0x1005db20 | `FUN_1005d0d8` (`FormatFeedbackIIR_t` execute `FUN_10072410`, categories `0x23` and `0x26`) | clear (refusal / generic combat-failed texts) |
| 0x1006797a | `FUN_10067978` (category `0x31`) | clear + `Feedback_StartingAttackFailed` |
| 0x100599f5 | `FUN_10059949` (SimpleChar flag-event handler, called from `FUN_1005b016`) | clear when event bit `0x20000000` |

* **There is no timer and no timeout.** Nothing else touches the byte; if the server never answers (no `AttackIIR` relay, no feedback) the guard stays set and every
  later attack press prints `Feedback_PleaseWaitUntilPreviousAction` until one of the rows above runs. `combat::AttackGate` is that state machine.
* **No client-side attack repeat.** Combat rounds are the server's: after the `AttackIIR` relay the server streams `AttackInfo`/`MissedAttackInfo`/`SpecialAttackInfo` (docs/zone/misc.md);
  the client sends nothing per round. The attack key toggles (§2.1). [RE for "client sends nothing": the only callers of `FUN_10067c34` are `N3Msg_DefaultAttack` and `N3Msg_StartPvP` [GC 0x10018276].]
* Special attacks (`N3Msg_SecondarySpecialAttack`) do not use the guard. Target: the current fight target (`+0x4c/+0x50`) when fighting, else the selected target
  (`FUN_10058816()+0x5c/+0x60`); gated by `can_attack(target, 1)`, `FUN_10029e66`, the skill's recharge (`FUN_10063be4/10063c09/10064301`), `Feedback_SpecialAttackIsUnavailable`,
  `Feedback_TargetIsOutsideSpecialAttackRange`, line of sight `FUN_10058908` (`Feedback_NoLineOfSight`); then `CharSecSpecAttackIIR_t` is sent. [RE; not ported: depends on the skill/recharge tables.]

## 4. Who stops the fight when the target dies / is gone

The server; the client never sends `StopFight` for that. [RE] Every local transition to "idle" goes through `FUN_10068b7f(1,0)` [GC], whose callers are:
`StopFightIIR_t` execute (`FUN_10079d75`), the char-die constructor `CharDie_t` (`FUN_1007b2ba`), the movement-state changes `FUN_1006dd0c`/`FUN_1006e2be`, `FUN_1003e2fe` (sets char flag `0x20` first), `FUN_100459ca`,
and `FUN_10069c68` (switching target). `N3Msg_StopAttack` is called only by `DefaultAttack`, `SitToggle` and `StartCamping`. When the target despawns the server relays `StopFightIIR_t` to us ([obs] 128 captured relays;
`docs/zone/misc.md`). What the client *does* on its own is only selection housekeeping: `TargetingModule_t::FrameProcess` [GUI 0x10025fa4] drops the **selection** (`RemoveTarget`) when the selected dynel disappears (`N3Msg_GetPos` fails / has a parent) and unless the target was forced;
the attack-target indicator (`m_pcAttackingIndicator`) follows `N3Msg_GetAttackingID` every frame.

## 5. Controls: key / click / button → N3Msg chain

Sources: `GUI.dll` embeds the key-table text at file offset `0x1a9720` ("`KEY_… COMMAND_… [ & AnyPlayMode ! TextInputMode ] = TAB`", loaded into `InputConfig_t`);
`Setupf/commands.txt` maps `COMMAND_*` → `DESTINATION_*`/`PACKET_TYPE_*`; `cd_image/gui/Default/OptionPanel/HotKeys.xml` lists the user-rebindable groups; in-client help (`cd_image/text/help/{Targeting,combat,Action window,ActionbarHelp}.html`).

### 5.1 Keys

| key | COMMAND → handler | evidence |
|---|---|---|
| **TAB** / SHIFT+TAB | `COMMAND_NEXT/PREV_HOSTILE_TARGET` → `TargetingModule_t::GetNext/PrevHostileTargetMessage` [GUI 0x10025adc/0x10025b21] → `N3Msg_GetCloseTarget(cur, forward, friendly=false)` [GC 0x1001c411] → `SetTarget` [GUI 0x100257b0] | GUI.dll key table [RE] |
| CTRL+TAB / CTRL+SHIFT+TAB | `COMMAND_NEXT/PREV_FRIENDLY_TARGET` → `…FriendlyTargetMessage` [GUI 0x10025a52/0x10025a97] (`friendly=true`) | GUI.dll key table [RE]. The help page "Targeting" says SHIFT+TAB = friendly: the binary table wins (help is older). |
| **Q** | start/stop attack ("press Q … press the Q key again") = `COMMAND_DEFAULT_ATTACK` (`commands.txt`: `DESTINATION_N3_INTERFACE`, `PACKET_TYPE_DEFAULT_ATTACK`) | help `combat.html` [obs: text]; the key is **not** in the GUI.dll table. The user key list comes from `Setupf/hotkeys.txt` (`InputConfig_t::SetDefaultHotkeys` [GUI 0x1001ad88]) which this install does not contain, so "Q" is evidence from help text only. **Unresolved:** the receiver of `PACKET_TYPE_DEFAULT_ATTACK`; the only callers of `N3Msg_DefaultAttack` are `SwitchTarget` and `FUN_1004256c`, so it must end in `PerformSpecialAction(ACTION_ATTACK = 0xb)` ([INFERENCE]). |
| 1…9, 0 | shortcut-bar slot of the current layer (SHIFT+digit changes layer); an Attack slot holds `ACTION_ATTACK` | help `ActionbarHelp.html` |
| F1 | select self (`TargetingModule_t::SlotSelectSelf`/`SelectSelf` [GUI 0x100259b1]); F2…F6 teammates, SHIFT+F1…F4 pets (`SlotTargetPet` [GUI 0x10025cd5]) | help `Targeting.html` |
| ESC | remove target selection | help `Targeting.html` |
| `/assist [name]` (macro `%t`) | `FUN_100b7bc5` [GUI] → `N3Msg_AssistFight` [GC 0x10027374]: `Feedback_NoTargetToAssist`/`CantAssistYourself`/`TargetIsNotInFight`; otherwise **selects the assisted char's fight target** (`SetTarget` signal path); it does not start an attack | [RE] |
| X | sit toggle (`N3Msg_SitToggle` [GC 0x10028e0a], stops the attack first) | help "Controlling and Moving my character" |

### 5.2 Mouse in the Action View (`ActionViewMouseHandler_c`, ctor `FUN_1002c66b` [GUI])

`InputConfig_t` `+0xb0/+0xb4` = identity under the mouse, `+0xc0/+0xc4` = current selection; kinds `0x9c47`/`0x9c52` (ground) are ignored.
Release handler `FUN_1002c469` (event 1 = left, 2 = right) [RE]:

* **Left click, no qualifier** (`GetQualifiers & 3 == 0`, and not `&0xc`): select. `N3Msg_GetNextTarget` [GC 0x10016deb] is consulted; if it returns nothing/ground the clicked id is used; `AFCM::Send(0x1e, 0x126, id)` → targeting selects it.
* **SHIFT + left click** (`& 3 != 0`): info window (`charid://50000/N` or `itemid://k/i` into `InfoViewModule_c::ShowURL`).
* **Qualifier `& 0xc` (CTRL/ALT; which bit is which was not resolved) + left click on a character (kind 50000)**: select (`AFCM::Send 0x1e,0x126`) **and `N3Msg_SwitchTarget(id)`** = attack that target (`DefaultAttack(id, switch=true)`). This is the only mouse path that attacks.
* **Right click**: character → `N3Msg_DefaultActionOnDynel` [GC 0x100291da] (trade/loot/use; **no attack**); item → `N3Msg_UseItem`. Double-click (press handler `FUN_1002c2ee`) does the same when the DValue `DoubleclickAction` is on; `LMBMouseLook` starts mouse-look.

### 5.3 Buttons / actions

* **Target indicator** (`CCTargetControl_c`, ctor `FUN_100746ec` [GUI]; flag `+0x154` = hostile/right-hand indicator): click handler at `0x10073017`: flag set → `N3Msg_PerformSpecialAction(0xb)` (**the Attack button**); flag clear → `TargetingModule_t::SelectSelf` (friendly/left indicator; help: "left-click the target indicator" selects yourself).
* **Special-action providers** `ControlCenterModule_c::SetupProviders` [GUI 0x10068c38] → `SlotSpecialAction` [GUI 0x10067f32] → `N3Msg_PerformSpecialAction(int)` [GC 0x100272d8] → `FUN_10042d5f` → `FUN_1004256c` [GC] (`Feedback_ActionIsNotAvailable` when not found). Action ids (`combat::action`):
  `ATTACK 0xb`, `USE 3`, `SNEAK 0x13`, `SIT 0x4c`, `RELOAD 0x6e`, `BOW_SPECIAL 0x79`, `BRAWL 0x8e`, `DIMACH 0x90`, `SNEAK_ATTACK 0x92`, `FAST_ATTACK 0x93`, `BURST 0x94`, `FLING_SHOT 0x96`, `AIMED_SHOT 0x97`, `FULL_AUTO 0xa7`.
  `FUN_1004256c` (own-character actions): `0xb` and `0x4e` → `DefaultAttack(selected target, switch=false)`; `0x4c/0x4d` → `SitToggle`; `0x13` sneak; `0x14/0x8d` crawl; `0x11/0x12` → `MovementChanged(0x18/0x19)`; `0x4f` → `MovementChanged(0x24)`; `0x51/0x52` camp; `0x89` forage; `0x8e/0x90` and the skill ids above → `SecondarySpecialAttack`.
  Slot types (`FUN_100d79c9` [GUI]): 1 item (`UseItem`), 4 nano (`CastNanoSpell`), 6 special action (`PerformSpecialAction`), 7 macro.
* **Action menu** (`gui/Default/ActionMenu/CommandMenu.xml`): "Attack Actions" (`AM_CATEGORY_ATTACK`) and "Perk Actions" are filled from the same providers; the Control Panel is `specialaction_window`.

### 5.4 Targeting rules

* `GetCloseTarget(cur, forward, friendly)` [GC 0x1001c411] scans the locality dynels around the character (radius constants `0x10157878/0x1015787c`), keeps only `InPlay`(194) ≠ 0, renderable dynels other than self and the current target, classifies friendly vs hostile
  (non-NPC: team membership via `FUN_1006581f/FUN_10065865`; NPC: its `PetMaster`(196) when it names a friendly), and picks the next/previous one by the angle offset from the current target's bearing (wrapping). [RE, summarised; not ported.]
* `SetTarget(id, forced)` [GUI 0x100257b0]: a non-zero id must be `isIDOnGround` unless forced; stores it in `InputConfig`, sends `AFCM 0x13/0x126`, creates the selection indicator unless `Skill 0x400`-like flag, and fires the tip events `OnSelectingNPC`/`OnSelecting`/`OnSelectingPlayer`.
  `FightingTargetMessage` [GUI 0x10025947] feeds `N3Msg_Consider` (the red/yellow/green con colour).

## 6. Unresolved

* Receiver of `PACKET_TYPE_DEFAULT_ATTACK` and the default key list (`Setupf/hotkeys.txt`, missing from this install; the Q key rests on the in-client help text).
* Which of the two `& 0xc` qualifier bits is CTRL and which ALT.
* Meaning of `controller+0x38` / the condition map keys `0x3d`/`0x3f` in the opening gate of `can_attack`.
* Sub-code → text pairing of `FormatFeedback` category `0x23`; whether categories `0x26` / `0x31` have fixed texts beyond `Feedback_StartingAttackFailed`.
* Movement mode 7 meaning (falling is inferred).
