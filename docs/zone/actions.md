# Zone N3: character actions, sit/stand, emotes (`crates/ao-net/src/n3/action.rs`, `crates/aomac/src/play/combat/actions.rs`)

Sources: Gamecode.dll (GC), N3.dll (N3), GUI.dll (GUI); live capture `docs/captures/zone_ithaca.rec` (+ `zone_enter_ithaca.rec`, `zone_newchar_ithaca.rec`). Everything below was read from the client
unless marked **[INFERENCE]** / **[UNRESOLVED]**. Movement transition names: `FUN_1006c60f` [GC] (see also docs/zone/motion.md).

## 1. Wire layouts

### 1.1 `CharacterActionIIR_t` `5E477770`

Ctor `FUN_1007253f(hdr, identity_a, param, action, identity_b, text)` [GC 0x1007253f] stores `action` -> `+0x18`, `param` -> `+0x1c`, `identity_a` -> `+0x20`, `identity_b` -> `+0x28`,
`text` -> `+0x30`, and sets the *to-be-passed-on* byte `this[0xc] = 0` (unlike `CharDCMove`, which sets 1). Reader `FUN_100724e8` [GC]: `i32 action; i32 param; Identity a; Identity b; i16 len + text`.
Header identity of client messages = the client's control dynel `{0xC350, char_id}` (`this_engine+0x84`, `+0x14`). All `N3Msg_*` senders pass `param = 0` except the text commands
(which pass the number they parsed). Sent with `SendIIRToObservers(client dynel, ..)` (sit/camp) or `SendIIRToServer` (duel, team, items, ...).

```
5e477770 0000c350 00006584 00 | 00000057 00000000 | 00000000 00000000 | 00000000 00000000 | 0000      (client "stand up", flag 0)
```

Server relays (22 live, sender 1) have flag **0** and re-encode byte-exact with `action::character_action_for` (test `captured_relays_reencode_byte_exact`).
Live ids: `0x63` x15 (NPCs at spawn), `0x62` x3, `0xAD` x3, `0xA7` x3 (own char at spawn, one per capture file).

### 1.2 `SocialActionCmd_t` `3B290771` (an `n3Command_t`, not an IIR action)

Class `SocialActionCmd_t` (vftable GC 0x10155f0c, registered by `FUN_1000e5f2`, factory `FUN_1000bdf9`, builder `FUN_1007aab7`). `n3Command_t` ctor [N3 0x10003895] sets to-be-passed-on **1**,
`state` (`+0x18`) 0 and `counter` (`+0x1c`) = `s_nCommandRefCntr++` (global, incremented per command). Body: `WriteSubClass` [N3 0x100037c5] = `i32 state; i32 counter`, then `FUN_1007aa03` [GC] adds
`i32 anim` (`+0x20`); reader `FUN_1007aa23` accepts only `1 <= anim <= 0x47` (else IIR status 1 = error).

```
3b290771 0000c350 00006584 01 | 00000000 00000007 0000003e        (wave, counter 7)
```
`action::social_action(char_id, counter, anim)`, `parse_social_action`, `parse_social_body`. **No social command is in any capture**, so the bytes are the code-derived layout only
(test `social_action_bytes` checks key hash, layout and rejection of ids 0, 0x48, -1). How the sender finishes (`(*this->vtable[0x2c/4])(cmd)` = the engine's command dispatcher) is not decoded.

## 2. Action ids

`FUN_1005d0d8` [GC] is the apply routine. Called from `FUN_10072410` [GC] only if the header identity resolves to a `SimpleChar_t` (`this_02` below = that char). Prologue: `other = identity_a` as a `SimpleChar_t` if `identity_a.kind == 50000`, else null. Dispatch:
`if (action - 1 > 0x105) default; case = byte[0x1005efdf + action - 1]` (**106 ids** have a case, everything else -> case 0x66 = ignored); jump table at [GC 0x1005ee43]. Both tables are in
`action::RECEIVED` `(id, case, handler address)`. The senders are `action::SENT`. The enum has no symbols; names below are the sender function names, or the feedback key a handler prints.
Rows with an empty "client applies" column are ids the client only *sends* (the server consumes them).

| id | client sends (sender) | client applies (case @ handler) | notes |
|---|---|---|---|
| `0x1` |  | 0x0 @ 0x1005d187 | recv text `KilledMissionTarget` (LDB cat 1000) |
| `0x12` |  | 0x1 @ 0x1005d1e4 |  |
| `0x13` | CastNanoSpell | default (ignored) |  |
| `0x14` |  | 0x2 @ 0x1005e2cd |  |
| `0x15` |  | 0x3 @ 0x1005d284 |  |
| `0x16` | KickTeamMember | default (ignored) |  |
| `0x18` | LeaveTeam | 0x4 @ 0x1005d2a2 |  |
| `0x19` | TransferTeamLeadership | default (ignored) |  |
| `0x1a` | TeamJoinRequest | 0x5 @ 0x1005d2ff |  |
| `0x1b` |  | 0x6 @ 0x1005d2b5 |  |
| `0x1c` | RequestReply | default (ignored) |  |
| `0x1e` |  | 0x7 @ 0x1005d235 |  |
| `0x20` |  | 0x8 @ 0x1005d348 |  |
| `0x23` |  | 0x9 @ 0x1005d384 |  |
| `0x24` | text cmd (?) | default (ignored) |  |
| `0x2b` |  | 0xa @ 0x1005d45c | recv text `RecievedTeamBonus` |
| `0x2e` |  | 0xb @ 0x1005d4c3 | recv text `Feedback_ItWasNotPossibleToAddItemToContract` |
| `0x2f` |  | 0xc @ 0x1005d4e5 |  |
| `0x34` | SplitItem | 0xd @ 0x1005d4f6 |  |
| `0x35` | JoinItems | 0xe @ 0x1005d50e |  |
| `0x36` | HideAgainstOpponent | default (ignored) |  |
| `0x39` | UseItem | default (ignored) |  |
| `0x3b` |  | 0xf @ 0x1005d522 |  |
| `0x3c` |  | 0x10 @ 0x1005d65c |  |
| `0x41` | RemoveBuff | default (ignored) |  |
| `0x42` | FUN_1003fa2c | default (ignored) |  |
| `0x46` | UseSkill | default (ignored) |  |
| `0x49` |  | 0x11 @ 0x1005d674 |  |
| `0x4f` |  | 0x12 @ 0x1005d695 | recv text `YouAreInsured` |
| `0x50` |  | 0x13 @ 0x1005ebe7 |  |
| `0x51` | TradeskillCombine | default (ignored) |  |
| `0x54` |  | 0x14 @ 0x1005d70f |  |
| `0x55` | SitOnItem | default (ignored) | **sit on item**: `identity_a` = item identity (`item+0xc4`) |
| `0x56` |  | 0x15 @ 0x1005d723 | **sit relay**: FSM `vtable[6](0x1e)` = SwitchToSitGroundMode |
| `0x57` | StandUp | 0x16 @ 0x1005d72f | **stand up**: FSM `vtable[6](0x29 / 0x2a / 0x25)` for WaitState `0xf` / `0x10` / else |
| `0x5c` | text cmd (?) | default (ignored) |  |
| `0x61` |  | 0x17 @ 0x1005d790 |  |
| `0x62` | add nano effect | 0x18 @ 0x1005d7de | live x3: `identity_a` = nano {0xCF1B,0x27E79}; `identity_b.kind` = caster instance, `.instance` = duration centiseconds; `FUN_100512af` replaces conflicts then adds |
| `0x63` |  | 0x19 @ 0x1005d827 | live x15 (NPCs at spawn, `identity_b = {0,503}`): sets flag 0x10 on the stat holder, **stat 0x183 := identity_b.instance**, `FUN_10059ae5(1)` |
| `0x64` |  | 0x1a @ 0x1005d873 | stores `identity_b.instance` in the object at `char+0x1dc` (`FUN_1003c47c`, the same setter the emote path uses) |
| `0x66` |  | 0x1b @ 0x1005d258 | recv text `Feedback_TargetIsOutsideRange` |
| `0x67` | text cmd damagemult | default (ignored) |  |
| `0x69` | FUN_1003f5fc | default (ignored) |  |
| `0x6a` | UseItem(2) | 0x1c @ 0x1005d887 |  |
| `0x6b` |  | 0x1d @ 0x1005d8bd |  |
| `0x6c` | FUN_1004fcf9 | 0x1e @ 0x1005d8d7 |  |
| `0x6d` | ToggleReclaim(on) | 0x1f @ 0x1005d897 |  |
| `0x6e` | ToggleReclaim(off) | 0x20 @ 0x1005d8ad |  |
| `0x70` | DeleteItem | 0x21 @ 0x1005d8f1 |  |
| `0x75` |  | 0x22 @ 0x1005d913 |  |
| `0x76` | FUN_10068d6b | 0x23 @ 0x1005d92b | recv **attack refused**: `identity_a.instance` 1..14 selects a `Feedback_*` text (14 strings in address order: CombatIsNotPossibleInThisDistrict, AttackNotAllowedSinceYouAreOnSameSide, YouCannotAttackThisPlayerTooFarAwayInLevel, ...TowerTooFarAwayInLevel, YouCannotAttackYourPet, NotAllowedToAttackTeamMembers, PvpNotAllowedInThisDistrict, ...SinceYouAreNeutral, ...SinceYourTeamIsNeutral, CantAttackTargetIsInPvpGrace, DefenseShieldEnabled, CantAttackTargetYouAreInMixedTeam, TowersCanOnlyBeAttackedWhenGaslevelBelow75, CantAttackTargetYouAreInMixedTeamBattlestation) [order by code address = INFERENCE]; also clears the attack gate `controller+0x79 = 0` |
| `0x77` |  | 0x24 @ 0x1005da11 | recv text `Feedback_TeamMemberLinkdead` / `...WentLinkDead` |
| `0x78` | StartCamping | default (ignored) | StartCamping (client) |
| `0x79` | StopCamping | default (ignored) | StopCamping (client) |
| `0x7a` |  | 0x25 @ 0x1005dae8 |  |
| `0x7b` |  | 0x26 @ 0x1005db15 | recv `Combat_PvPTargetLvl` / `...Team`; clears `controller+0x79` |
| `0x7f` | text cmd (?) | default (ignored) |  |
| `0x80` | FUN_10065bc5 | default (ignored) |  |
| `0x81` |  | 0x27 @ 0x1005dc31 |  |
| `0x82` |  | 0x28 @ 0x1005dc12 |  |
| `0x83` |  | 0x29 @ 0x1005d7b7 |  |
| `0x84` |  | 0x2a @ 0x1005ddbc |  |
| `0x85` | text cmd played | default (ignored) |  |
| `0x86` | text cmd (?) | default (ignored) |  |
| `0x89` |  | 0x2b @ 0x1005d248 |  |
| `0x8a` |  | 0x2c @ 0x1005dc77 |  |
| `0x8b` |  | 0x2d @ 0x1005dc50 |  |
| `0x8c` |  | 0x2e @ 0x1005dc96 | recv text `Feedback_IncreasedNanoPool` |
| `0x8e` | FUN_1003f692 | default (ignored) |  |
| `0x91` | text cmd (team loot) | 0x2f @ 0x1005ddda |  |
| `0x92` |  | 0x30 @ 0x1005ddf3 |  |
| `0x93` |  | 0x31 @ 0x1005de04 |  |
| `0x96` | text cmd clone | default (ignored) |  |
| `0x98` | FUN_1007b58c/FUN_1007b88a | default (ignored) |  |
| `0x99` |  | 0x32 @ 0x1005d861 |  |
| `0x9a` | ResetSkill | default (ignored) |  |
| `0x9b` |  | 0x33 @ 0x1005de1a | recv text `Feedback_YouHaveBeenDetected` |
| `0x9c` |  | 0x34 @ 0x1005dd18 | recv text `Feedback_YouWereDrained` |
| `0x9d` | text cmd version | 0x35 @ 0x1005deef | text command `/version` -> server answers with this id: texts `VersionsInfo`, `ClientServerVersMM` |
| `0x9e` |  | 0x36 @ 0x1005dfe0 |  |
| `0xa2` |  | 0x37 @ 0x1005e006 |  |
| `0xa3` | TryEnterSneakMode | default (ignored) |  |
| `0xa4` |  | 0x38 @ 0x1005e016 | recv text `Feedback_SkillAvailable` |
| `0xa5` | EventFeedback(a) | default (ignored) |  |
| `0xa6` | EventFeedback(b) | default (ignored) |  |
| `0xa7` |  | 0x39 @ 0x1005e3d2 | live x3 (own char at spawn, `identity_b = {0,0}`): `identity_b.instance == 0` clears flag 0x800 of `dynel+0x138`, else sets it on the stat holder |
| `0xa8` |  | 0x3a @ 0x1005d320 |  |
| `0xa9` |  | 0x3b @ 0x1005d334 |  |
| `0xaa` |  | 0x3c @ 0x1005e350 |  |
| `0xab` |  | 0x3d @ 0x1005e41e |  |
| `0xac` |  | 0x3e @ 0x1005e479 |  |
| `0xad` |  | 0x3f @ 0x1005e489 | live x3: `FUN_10044e14(1)` on `char+0x1ec`: if FSM mode == 6 (sneak) runs transition 0x24 (LeaveSneak) [GC 0x1005e489] |
| `0xae` | text cmd clearunique | default (ignored) |  |
| `0xaf` | RequestChecklist | default (ignored) |  |
| `0xb0` |  | 0x40 @ 0x1005d213 |  |
| `0xb1` | refresh nano effect | 0x41 @ 0x1005d7f5 | same operands/add routine as 0x62, fourth argument 1 instead of 0; replacement and accounting are identical |
| `0xb2` |  | 0x42 @ 0x1005e2b4 |  |
| `0xb3` | FUN_1004256c | default (ignored) |  |
| `0xb4` |  | 0x43 @ 0x1005e49b |  |
| `0xb5` |  | 0x44 @ 0x1005e4dd |  |
| `0xb6` |  | 0x45 @ 0x1005e4bc |  |
| `0xb8` | SetPlayerOption | default (ignored) |  |
| `0xb9` |  | 0x46 @ 0x1005e69c |  |
| `0xba` |  | 0x47 @ 0x1005e6b9 |  |
| `0xbb` | FUN_10052ede | 0x48 @ 0x1005e4f9 |  |
| `0xbc` | FUN_10052f8e | 0x49 @ 0x1005e512 |  |
| `0xbd` |  | 0x4a @ 0x1005e6cb |  |
| `0xbe` |  | 0x4b @ 0x1005e6e5 | recv text `Feedback_TrapDetected` + effect 0x2cf3 |
| `0xc0` |  | 0x4c @ 0x1005e738 |  |
| `0xc1` |  | 0x4d @ 0x1005e752 |  |
| `0xc2` |  | 0x4e @ 0x1005e76c |  |
| `0xc3` |  | 0x4f @ 0x1005e786 |  |
| `0xc4` |  | 0x50 @ 0x1005e52b | recv text `Feedback_StuckResolved` |
| `0xc5` |  | 0x51 @ 0x1005e5db | recv text `Feedback_StuckAvailable` |
| `0xc6` | StartAltState | 0x52 @ 0x1005e3fe |  |
| `0xc7` | StopAltState | 0x53 @ 0x1005e40e |  |
| `0xc9` |  | 0x54 @ 0x1005e7a0 |  |
| `0xca` | Forage | default (ignored) |  |
| `0xcb` |  | 0x55 @ 0x1005e7c6 |  |
| `0xcc` |  | 0x56 @ 0x1005e7e6 |  |
| `0xcd` |  | 0x57 @ 0x1005e846 |  |
| `0xce` |  | 0x58 @ 0x1005e18a | recv text `Feedback_PerkAvailable` |
| `0xcf` |  | 0x59 @ 0x1005e297 |  |
| `0xd0` |  | 0x5a @ 0x1005e8b5 | recv texts `Feedback_DrainedHealth` / `Feedback_DrainedNano` |
| `0xd1` |  | 0x5b @ 0x1005ea42 |  |
| `0xd2` | FUN_1006949a | default (ignored) |  |
| `0xd3` | DeleteNano | default (ignored) |  |
| `0xdc` | InsertSourceAnalyzerItem | default (ignored) |  |
| `0xdd` | InsertTargetAnalyzerItem | default (ignored) |  |
| `0xde` | BuildAnalyzerItem | default (ignored) |  |
| `0xdf` |  | 0x5c @ 0x1005ec5a |  |
| `0xe0` |  | 0x5d @ 0x1005ec71 |  |
| `0xe1` |  | 0x5e @ 0x1005ec88 |  |
| `0xe2` |  | 0x5f @ 0x1005ec99 |  |
| `0xe3` |  | 0x60 @ 0x1005ecb0 |  |
| `0xe4` |  | 0x61 @ 0x1005eccb |  |
| `0xef` | PetDuel_Challenge | 0x62 @ 0x1005eced |  |
| `0xf0` | PetDuel_Accept/Refuse | 0x62 @ 0x1005eced | PetDuel: `identity_b.kind` 1 = accept, 0 = refuse |
| `0xf1` | PetDuel_Stop | 0x62 @ 0x1005eced |  |
| `0xf3` |  | 0x62 @ 0x1005eced |  |
| `0xf4` | FUN_1004b1ab | default (ignored) |  |
| `0xf5` | FUN_1004b254 | default (ignored) |  |
| `0xf8` |  | 0x62 @ 0x1005eced |  |
| `0xfc` |  | 0x63 @ 0x1005e7b3 |  |
| `0xfd` | AddToQueue | default (ignored) |  |
| `0xfe` | GetInfo | default (ignored) |  |
| `0xff` | LeaveQueue | default (ignored) |  |
| `0x100` | RefreshLaserTags | default (ignored) |  |
| `0x101` | ArtilleryAttack | default (ignored) |  |
| `0x102` | OrbitalAttack | default (ignored) |  |
| `0x103` | Airstrike | default (ignored) |  |
| `0x104` | GetPointLocations | default (ignored) |  |
| `0x105` | Inspect | 0x64 @ 0x1005ed1a | Inspect (client); recv: `identity_b.instance == 2` -> text `Feedback_InspectRejected` |
| `0x106` | Duel_* | 0x65 @ 0x1005ed04 | Duel_*: `identity_b.kind` = sub-op (0 challenge [identity_a = target], 1 accept, 2 refuse, 3 stop, 4 draw); recv case 0x65 = `FUN_1005b821` (`AutoRejectDuel` DValue) |
| `0x107` | RequestClaims | default (ignored) |  |

Text commands (`N3Msg_TextCommand` [GC 0x176db] -> `FUN_1003fba6`, debug/GM commands, not needed for play): `/version` 0x9d, `/resetskill` 0x9a, `/clone` 0x96, `/played` 0x85, `/damagemult` 0x67,
`/clearunique` 0xae (command = the nearest literal before the call site, **[INFERENCE]** except 0x9d/0x9a which match `N3Msg_ResetSkill` and the reply handler); 0x5c, 0x24, 0x7f, 0x86, 0x91 are text-command
carriers whose command name was not pinned down.

## 3. Sit / stand / camping (client side)

Stat 430 is **`WaitState`** (not a movement mode; `stat_names.txt`). Values the client tests: **1** = sitting on an item, **0xf** = sleeping, **0x10** = lounging. The FSM
(`char+0x50` = `Vehicle_t`, `+0x178` = movement FSM, FSM`+4` = mode, `motion::Mode`): 4 swim, 8 sit ground, 9 / 0xb / 0xc see below.

`N3Msg_SitToggle` [GC 0x10028e0a] (`action::sit_toggle(&SitInput) -> Vec<Outgoing>`):

1. no client char -> nothing.
2. fight state `char+0x1d4 -> +0x44 != 1` -> `N3Msg_StopAttack` (always, before anything else).
3. `char+0x50` virtual `+0x9c` true -> return: **resolved**, it is the movement FSM's `IsMoving` (vehicle vtable slot 39 `FUN_1006efe1` -> `fsm.vtable[7]`, docs/zone/movement.md §10); `SitInput::blocked` = `Movement::sit_input`.
4. `WaitState` in {0xf, 0x10} -> `CharacterAction 0x57` (all identities zero), done.
5. selected item (`FUN_1008720e(FUN_10058816()+0x5c)`: the identity stored in the targeting singleton `ecx+0x1d0` at `+0x5c`, cast to `SimpleItem_t`; **[INFERENCE]** it is the current target) whose stat `Can` (0x1e)
   has bit value 2 [INFERENCE: "can sit"]: if `WaitState == 1` -> `0x57` else -> `0x55` with `identity_a` = the item's identity (`item+0xc4`); done.
6. FSM mode (`FUN_100704e6` = `fsm+4`) `!= 8` -> `N3Msg_MovementChanged(0x1e, 0, 0, true)` (CharDCMove `move_type` 0x1e = SwitchToSitGroundMode); `== 8` -> `0x57`.

`N3Msg_StartCamping` [GC 0x1001c93d] (`start_camping`): no char -> false. If `GmLevel` (stat 0xd7) == 0: `IsFightingMe` (0x19a) > 0 -> text `Feedback_CantLogOutInAFight`, false; if FSM mode not 8/9: `fsm.vtable[3](0x1e)` false
-> `Feedback_YouMustBeSitting`, false, else `MovementChanged(0x1e)` (sit). Then `StopAttack` if attacking, `CharacterAction 0x78` to observers, GUI signal, `FUN_10042da4(0x51)`. `N3Msg_StopCamping` [0x1001cada]: action `0x79`, `FUN_10042da4(0x52)`.
`N3Msg_CrawlToggle` [GC 0x100278c9] is the CharDCMove `move_type` 0x1b (SwitchToCrawl; leave = 0x28), not an action; its guards were not read.

`N3Msg_DoSocialAction` [GC 0x100269d3] (`social_allowed`): FSM mode 4 (swim) -> `Feedback_CantDoSocialActionsWhileSwimming`. For the sleep (0x44) / lounge (0x45) emotes: a vehicle equipped (`char+0x2c8 != 0` and `FUN_1002e347`) ->
`Feedback_YouCanNotDoThisWithAVehicleEquipped`; FSM mode not in {8, 0xb, 0xc} -> `Feedback_MustSitToLoungeOrSleep`. Otherwise it builds `SocialActionCmd_t({50000, char_id}, anim)` and dispatches it.
Modes 0xb / 0xc are sleep / lounge (`motion::Mode`); mode 9 (camping allowed, `FollowTarget` refused) is **[INFERENCE]** sitting on a chair.

## 4. Client reaction to incoming actions (`FUN_1005d0d8`)

* `0x56` [GC 0x1005d723]: `char+0x50 -> +0x178 (FSM) -> vtable[6](0x1e)` = SwitchToSitGroundMode (same call as a relayed `CharDCMove` type 0x1e; guard needs `Features & 4`, `Status::transition`).
* `0x57` [GC 0x1005d72f]: reads WaitState; `0xf` -> transition `0x29` LeaveSleepMode, `0x10` -> `0x2a` LeaveLoungeMode, else `0x25` LeaveSitMode. `motion::Status::transition` already models the effect
  (leave sleep/lounge -> sit ground; leave sit -> previous mode).
* sit gate: `SwitchToSitGroundMode` and the `Leave*` guards read `FUN_10044b6e(4)` [GC 0x1006cd51]: stat `Features` (224) of the dynel **& 4**. The captured NPCs (Features `0x8003` / `0x8007`) fail it, which is why their
  type-30 placements after spawn leave them standing. Players need mask 4 ([INFERENCE] they have it).
* `0x64` stores `identity_b.instance` into `char+0x1dc` (`FUN_1003c47c`), `0x63` sets stat `0x183`, `0xA7` flag `0x800`, `0xAD` leaves sneak; see the table.
* Feedback texts (own char only unless the handler tests `dynel+0x140`): see the table; texts are `LDBface::GetText(1000 / 101 / 200 / 2002, key)` with the keys listed.
* Nano / team / duel / trade / mission ids (`0x13..0x1c`, `0x35`, `0x6d`..`0x6e`, `0xef`..`0xf1`, `0xfd`..`0x107`) are only listed; their handlers are UI plumbing (`FUN_10046bdd`, `FUN_10046c10`, `FUN_1004effe`, ...).
* Emotes (`SocialActionCmd_t`): the verify slot `FUN_1007a91e` [GC] (state 0) plays the clip when the char exists and `dynel+0x138` bit 5 is clear (`FUN_1003c47c(anim)` stores the id at the char's emote object);
  the execute slot `FUN_1007a977` (state 1) does the same for a foreign dynel; `FUN_1007aa66` runs transition `0x21` (SwitchToSleep) for `0x44` and `0x22` (SwitchToLounge) for `0x45`.

## 5. Emotes

Table at [GC 0x1015eb98] (`FUN_10053c73`, pet emote command) and [GUI 0x101badd0] (`FUN_100b29be`, the chat command): 70 rows of 12 bytes `{ char* name, i32 id, char* third }`, id = row + 1, looked up with `_stricmp` on `name` only.
GUI handler `FUN_100b2aba` [GUI]: `/emote <name>` or `/<name>` (leading `/` skipped); accepts `0 < id < 0x47` and sends `N3Msg_DoSocialAction(id)`, else prints `Error: No emote named '<name>'`
(`/emote` without a name: `Usage: /emote <emote-name>`). `/anim <name>` [GUI 0x100b2be7] sends an arbitrary *animation* id from the clip-name table `FUN_100c01c9` (ids >= 100; the server only accepts 1..0x47).
The Action menu page `cd_image/gui/Default/ActionMenu/SocialMenu.xml` runs the same `/<name>` chat scripts (Greeting, Gestures, Approval, Dislike, Dancing, Athlete, Relaxing, Directions pages);
`cd_image/text/help/Social Moves Commands.html` lists them.

Clip: `social-<name>` in the character clip set (`%s_%s_01_01.ani`, docs/formats.md; `Role::Emote` in `ao-formats`). The third field is another name (`allah`, `ass`, `crotch`) or a `*_01_01` animation name; its consumer was not found
**[UNRESOLVED]** (the two `_stricmp` users ignore it).

| id | `/command` | 3rd field |
|---|---|---|
| `0x01` | prostrate | allah |
| `0x02` | angry |  |
| `0x03` | apachi |  |
| `0x04` | applause |  |
| `0x05` | itch | ass |
| `0x06` | backflip |  |
| `0x07` | ballet |  |
| `0x08` | blowkiss |  |
| `0x09` | bow |  |
| `0x0a` | bulge |  |
| `0x0b` | chicken |  |
| `0x0c` | cross |  |
| `0x0d` | crossarm |  |
| `0x0e` | adjust | crotch |
| `0x0f` | curt |  |
| `0x10` | disco |  |
| `0x11` | drink |  |
| `0x12` | eat |  |
| `0x13` | fblock |  |
| `0x14` | fishsize |  |
| `0x15` | flamenco |  |
| `0x16` | flip |  |
| `0x17` | giggle |  |
| `0x18` | gloat |  |
| `0x19` | greet |  |
| `0x1a` | italian |  |
| `0x1b` | kneel |  |
| `0x1c` | laugh-b |  |
| `0x1d` | laugh-s |  |
| `0x1e` | legshake |  |
| `0x1f` | lookout |  |
| `0x20` | moon |  |
| `0x21` | nod |  |
| `0x22` | nono |  |
| `0x23` | pointba |  |
| `0x24` | pointfor |  |
| `0x25` | pointlef |  |
| `0x26` | pointrig |  |
| `0x27` | pointup |  |
| `0x28` | pray |  |
| `0x29` | puke |  |
| `0x2a` | pulp |  |
| `0x2b` | read |  |
| `0x2c` | rocky |  |
| `0x2d` | salute |  |
| `0x2e` | scared |  |
| `0x2f` | scratch |  |
| `0x30` | shake |  |
| `0x31` | shrug |  |
| `0x32` | slap |  |
| `0x33` | speech |  |
| `0x34` | spit |  |
| `0x35` | strong1 |  |
| `0x36` | strong2 |  |
| `0x37` | strong3 |  |
| `0x38` | strong4 |  |
| `0x39` | surprised |  |
| `0x3a` | surrender |  |
| `0x3b` | swroyal |  |
| `0x3c` | thinker |  |
| `0x3d` | thumbs |  |
| `0x3e` | wave |  |
| `0x3f` | ymca |  |
| `0x40` | kiss | kiss_01_01 |
| `0x41` | kisslow | kissdown_01_01 |
| `0x42` | kisshigh | kissup_01_01 |
| `0x43` | hug | hug_01_01 |
| `0x44` | sleep |  |
| `0x45` | lounge |  |
| `0x46` | facepalm |  |

## 6. App state (`play/combat/actions.rs`)

`Actions::on_frame(&Frame) -> Vec<Event>` decodes with `ao_net::n3::decode` and tracks, per dynel, a `motion::Status` (the movement FSM), the dynel's `Features` and `WaitState`:
* `SimpleCharFullUpdate`: status from the blob (`Status::from_blob`), Features from the full update's stat pairs else NPC 2 / player 4 **[INFERENCE]**.
* `CharDCMove`: `Features & 4` drives every move type, `& 2` only the turn types 9..=14 (`FUN_1006b84b`), sit/leave guards need `& 4`.
* `CharacterAction 0x56 / 0x57` (section 4); stat pairs `WaitState` (430) and `Features` (224); `SocialActionCmd_t` with state 0 or 1 -> `Event::Emote { clip: "social-<name>" }` plus, for the accepted copy (state 1), the sleep/lounge transition.
* `Pose {Standing, SitGround, SitItem (SitGround while WaitState == 1), Sleeping, Lounging, Crawling}`; `Pose::transition_anim(from, to)` = the one-shot enter / stop `AbstractAnimID` the original plays over the new pose's idle clip
  (`ground-start` 0xd5, `ground-stop` 0xd6, `sleep-ground` 0xed, `lounging` 0xef, `crawl_start` 0x68, `crawl_stop` 0x69; read from the FSM entry handlers, `combat-anim.md` section 4: no chair clip, the
  way out of sleep / lounge is **[UNRESOLVED]**); `Pose::from_role` maps the own avatar's movement `Role`. The idle clips (`idle-ground`, `idle-sleep-ground`, `idle-lounging`, `idle-crawl`) are the movement state's own.
* `Event::Pose { dynel, from, to }` only when the pose changes (played on other dynels by `combat/glue.rs`; the own avatar does it from its movement role in `Player::update`). `ToClientQuit` of a character drops its tracked state.
  The chat line `/<emote>` is parsed by the chat layer (`chat/cmd.rs::emote_id`), not here. The live capture produces no pose events (nobody sits).

## 7. Unresolved

* Server-side behaviour for sit-on-item (`0x55`): what the server relays back (`0x56` + `WaitState = 1`?) is not captured.
* The `Vehicle_t` virtual `+0x9c` blocker in SitToggle; the exact object behind `FUN_10058816()+0x5c`; `Can` bit 2 = "sit" is inferred from use.
* Meaning of live actions `0x62` (list at `char+0x1c0`), `0x63` (stat 0x183 = 503), the third emote field, the numbering of mode 9, and the command names of text commands 0x5c/0x24/0x7f/0x86/0x91.
* A real `SocialActionCmd_t` capture (counter start value, server's relayed `state`).

## 6. Duel / PetDuel (CwDuel; full evidence: docs/zone/combat-duel.md)

Senders `n3EngineClientAnarchy_t::N3Msg_Duel_*` [GC 0x1001d2ac challenge, 0x1001d349 accept, 0x1001d467 refuse, 0x1001d585 stop, 0x1001d634 draw] and `N3Msg_PetDuel_*` [GC 0x1001cffa challenge,
0x1001d09b accept, 0x1001d14e refuse, 0x1001d1fd stop] = `FUN_1007253f(client dynel + 0x14, identity_a, 0, action, identity_b, "")`; builders `action::duel`. Receivers: `FUN_1005b821` (0x106) and `FUN_1005c514` (0xef/0xf0/0xf3/0xf8).


## 7. Every received id: who handles it (CwDuel walk)

Rows without a note are UI / GlobalSignals plumbing of another feature (team, containers, perks, effect list, missions ...): the callee was read, there is no fight state in it and no consumer in this client. **wired** = added with the duel slice.

| id | handler | what | ours |
|---|---|---|---|
| `0x1` | 0x1005d187 | text `KilledMissionTarget` | chat/log.rs |
| `0x12` | 0x1005d1e4 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x14` | 0x1005e2cd | special-action recharge extend | zone.rs `recharge_feed` -> hud_special (HudFinish) |
| `0x15` | 0x1005d284 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x18` | 0x1005d2a2 | team: leave | hud_team.rs |
| `0x1a` | 0x1005d2ff | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x1b` | 0x1005d2b5 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x1e` | 0x1005d235 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x20` | 0x1005d348 | team: member left | hud_team.rs |
| `0x23` | 0x1005d384 | team: leader | hud_team.rs |
| `0x2b` | 0x1005d45c | text `RecievedTeamBonus` | chat/log.rs |
| `0x2e` | 0x1005d4c3 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x2f` | 0x1005d4e5 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x34` | 0x1005d4f6 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x35` | 0x1005d50e | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x3b` | 0x1005d522 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x3c` | 0x1005d65c | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x49` | 0x1005d674 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x4f` | 0x1005d695 | text `YouAreInsured` | chat/log.rs |
| `0x50` | 0x1005ebe7 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x54` | 0x1005d70f | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x56` | 0x1005d723 | sit relay | combat/actions.rs + player.rs |
| `0x57` | 0x1005d72f | stand up | combat/actions.rs + player.rs |
| `0x61` | 0x1005d790 | unwield of body slot `identity_b.instance` of the header char (`FUN_1006a857` -> `FUN_1006a772`, stats 0x112/0x2b2), then `FUN_10081e74(char, 3)` = the wield gesture 0x6d; live: the rifle's unwear, `identity_b = {0, 6}`, 1 ms before the weapon's `WeaponItemFullUpdate` names the bag slot 0x41 | **wired**: `Armory::unwield_slot`, `Dynels::unwield_slot` (docs/zone/avatar.md §6); the gesture: `Module::on_frame` -> `take_anims` (docs/zone/combat-anim.md §4) |
| `0x62` | 0x1005d7de | buff/effect entry add (`FUN_100512af`) | **wired**: shared `OwnNanos` lifecycle + `Dynels` stat413 target-attached persistent visuals for own/foreign characters (docs/zone/misc.md §9) |
| `0x63` | 0x1005d827 | death: flag 0x10, stat 0x183 | combat/state.rs + player.rs; sound CwSound |
| `0x64` | 0x1005d873 | anim holder id := `identity_b.instance` | **wired**: `Module::take_anims` -> glue |
| `0x66` | 0x1005d258 | nano cast out of range (effect entry + own text) | not driven (nano casting) |
| `0x6a` | 0x1005d887 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x6b` | 0x1005d8bd | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x6c` | 0x1005d8d7 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x6d` | 0x1005d897 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x6e` | 0x1005d8ad | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x70` | 0x1005d8f1 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x75` | 0x1005d913 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x76` | 0x1005d92b | attack refused, clears +0x79 | combat/module.rs |
| `0x77` | 0x1005da11 | text TeamMemberLinkdead | chat/log.rs |
| `0x7a` | 0x1005dae8 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x7b` | 0x1005db15 | PvP confirmation, clears +0x79 | **wired**: duel.rs `PvpPrompt`, dialog, `Module::start_pvp` |
| `0x81` | 0x1005dc31 | combat-log line | chat/log.rs |
| `0x82` | 0x1005dc12 | combat-log line | chat/log.rs |
| `0x83` | 0x1005d7b7 | weapon slot map (`FUN_1006ad94`: `FUN_10081e74(char, 3)` gesture 0x6d, then `WeaponItem_t` vtable `+0xa4` = `FUN_1009e301` wield); live: right after every wear / unwear, `identity_a` = the weapon item `{0xC74A, id}`, `identity_b = {0, slot}` (6 on the wear, the bag slot 0x41 on the unwear) | **not implemented**: the `WeaponItemFullUpdate` of the same moment already carries what the slot tables need (a bag slot is no valid weapon slot); its repeat of the wield / gesture is invisible ([INFERENCE], combat-anim.md §4) |
| `0x84` | 0x1005ddbc | special action locked "Unable to perform action, able in hh:mm:ss" | **wired** chat/log.rs (perk branch: perk name unresolved, not printed) |
| `0x89` | 0x1005d248 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x8a` | 0x1005dc77 | combat-log line | chat/log.rs |
| `0x8b` | 0x1005dc50 | combat-log line | chat/log.rs |
| `0x8c` | 0x1005dc96 | text IncreasedNanoPool | chat/log.rs |
| `0x91` | 0x1005ddda | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x92` | 0x1005ddf3 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x93` | 0x1005de04 | attack failed, clears +0x79 | combat/module.rs |
| `0x99` | 0x1005d861 | death cause `FUN_1005ae91` | **wired**: state.rs `ACTION_DEATH_CAUSE` |
| `0x9b` | 0x1005de1a | text YouHaveBeenDetected | chat/log.rs |
| `0x9c` | 0x1005dd18 | text YouWereDrained | chat/log.rs |
| `0x9d` | 0x1005deef | version texts | chat/log.rs |
| `0x9e` | 0x1005dfe0 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xa2` | 0x1005e006 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xa4` | 0x1005e016 | text SkillAvailable | chat/log.rs |
| `0xa7` | 0x1005e3d2 | flag 0x800 of `dynel+0x138` | not tracked (reader only `FUN_10051f6e`) |
| `0xa8` | 0x1005d320 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xa9` | 0x1005d334 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xaa` | 0x1005e350 | recharge entry insert-if-absent (`FUN_10064a6e`) | **not fed**; handed to HudFinish |
| `0xab` | 0x1005e41e | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xac` | 0x1005e479 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xad` | 0x1005e489 | leave sneak | player.rs |
| `0xb0` | 0x1005d213 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xb1` | 0x1005d7f5 | same as 0x62 | not fight state; not driven |
| `0xb2` | 0x1005e2b4 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xb4` | 0x1005e49b | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xb5` | 0x1005e4dd | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xb6` | 0x1005e4bc | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xb9` | 0x1005e69c | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xba` | 0x1005e6b9 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xbb` | 0x1005e4f9 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xbc` | 0x1005e512 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xbd` | 0x1005e6cb | shadow knowledge dialog | not fight; not driven |
| `0xbe` | 0x1005e6e5 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xc0` | 0x1005e738 | faction text (cat 101 id 9) | not fight; not printed |
| `0xc1` | 0x1005e752 | faction text id 10 | not fight; not printed |
| `0xc2` | 0x1005e76c | faction text id 11 | not fight; not printed |
| `0xc3` | 0x1005e786 | faction text id 12 | not fight; not printed |
| `0xc4` | 0x1005e52b | text StuckResolved | chat/log.rs |
| `0xc5` | 0x1005e5db | text StuckAvailable | chat/log.rs |
| `0xc6` | 0x1005e3fe | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xc7` | 0x1005e40e | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xc9` | 0x1005e7a0 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xcb` | 0x1005e7c6 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xcc` | 0x1005e7e6 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xcd` | 0x1005e846 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xce` | 0x1005e18a | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xcf` | 0x1005e297 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xd0` | 0x1005e8b5 | drain: `SetStat(identity_b)` + drainer line | **wired**: state.rs `ACTION_DRAIN`, zone.rs, chat/log.rs |
| `0xd1` | 0x1005ea42 | struck: toxic number + hit sound | chat/log.rs, CwSound |
| `0xdf` | 0x1005ec5a | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xe0` | 0x1005ec71 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xe1` | 0x1005ec88 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xe2` | 0x1005ec99 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xe3` | 0x1005ecb0 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xe4` | 0x1005eccb | UI / GlobalSignals plumbing | not driven, not fight state |
| `0xef` | 0x1005eced | pet duel challenged | **wired** duel.rs |
| `0xf0` | 0x1005eced | pet duel answer | **wired** |
| `0xf1` | 0x1005eced | pet duel stop (no effect on receipt) | n/a, sent only |
| `0xf3` | 0x1005eced | pet duel result | **wired** |
| `0xf8` | 0x1005eced | pet duel announce | **wired** |
| `0xfc` | 0x1005e7b3 | UI / GlobalSignals plumbing | not driven, not fight state |
| `0x105` | 0x1005ed1a | Inspect rejected text | chat (Inspect) |
| `0x106` | 0x1005ed04 | duel | **wired** duel.rs |

Nano lifecycle detail (`100512af`, `1004fc8d`, `1004e488`, `1005195a`): runtime start is integer GameTime seconds ×100;
duration is scaled by own stat464 percent. Conflict comparison first rejects an existing stat551 greater than incoming551.
Families are stats75,546..550: when both sums of546..550 are zero, compare75 (including zero); otherwise any nonzero
incoming family matching any existing family conflicts. Login `SimpleCharFullUpdate.effects` (`10051b40`,
`10051741`) uses `source` as nano identity, ignores `a`, restores totalcs=`b`, remainingcs=`c`,
startcs=`nowcs-b+c`, and disables replacement. Actual `zone_ithaca.rec` evidence has three action62/BuffIIR removal
pairs at 23825/23920, 29898,74495 ms; no captured b1 or nonempty login list, so those regressions are synthetic.

## Live action frame sequences

The existing `live_walk` harness accepts `AOMAC_LIVE_SHOTS=<directory>` and
`arm=<prefix>:<seconds>[:note|special|either|equipment|use|level]`.
`either` remains the default and selects only own attack notes or SpecialAttack results.
The other selectors wait for the own equipment event, item-use playback, or NewLevel action to be processed.
The processing frame is `<prefix>-0000.png`; recording continues at fixed 60 Hz for the requested duration.
Arm **before** the action: `dclick` and `invuse` already tick while waiting internally.
For example, `arm=equip:2:equipment,dclick=40,capturewait=30` or
`arm=item-use:2:use,invuse=0x40,capturewait=30`; use a suitable item/slot for the selected action.
`dclick=item:<template id>` selects that actual inventory item at its current slot (for example,
`dclick=item:121569` for the captured rifle), avoiding guesses about the free bag slot after unwearing.
For level-up, arm `arm=level-up:2:level` before the action earning the level, then `capturewait=30`.
`frames=<prefix>:<seconds>` still records immediately without an event trigger.

