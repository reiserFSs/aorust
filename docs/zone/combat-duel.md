# Duel, PetDuel, PvP prompt (`crates/aomac/src/play/combat/duel.rs`, `ao_net::n3::action::duel`)

Evidence: Gamecode.dll (GC) and GUI.dll (GUI), Ghidra projects /tmp/aomac-ghidra/{playfield,proto}.

## Commands (GUI)
`/duel` = `FUN_100b8a10`, `/petduel` = `FUN_100b8789` (registered by `FUN_100badf1`). Tokens = whitespace words, command included. 1 token: the selection (`InputConfig+0xc0`) must be a character
(kind 50000); `/duel` also refuses the own character and NPCs (`N3Msg_IsNpc`) -> `N3Msg_Duel_Challenge(target)`; `/petduel` -> `N3Msg_PetDuel_Challenge(target)`. Otherwise the red (colour 0x51) line
`You need to target a player first.` 2 tokens (case-insensitive): `accept`, `reject`, `stop`, `draw` (duel only) -> the matching `N3Msg_*`. Anything else prints the usage lines (4 spaces indent
`DAT_101bbb60`; pet duel has no `draw` line and `stop -- stop current duel..`). The command takes **no player name**: the target is the selection. Code: chat/cmd.rs `duel_cmd`, `Module::duel_command`.

## Frames (all `CharacterActionIIR_t`, `SendIIRToServer` / `Observers`, pass-on byte 0)
Duel 0x106: `identity_b.kind` = sub-op (0 challenge with `identity_a` = target, 1 accept, 2 refuse, 3 stop, 4 draw). Pet duel: 0xef challenge (`identity_a` target), 0xf0 answer (`identity_b.kind` 1 accept /
0 refuse), 0xf1 stop. `N3Msg_Duel_Accept` / `_Refuse` also emit `GlobalSignals+0x268 / +0x264` = `GuiSystem_c::CloseDuelWindows`. Live server answers were **not** tested (no live run).

## Received 0x106 (`FUN_1005b821`, only for the own character)
sub-op 0, instance 0: challenger = `identity_a`. `AutoRejectDuel` DValue (LoginPrefs default false) set: reply `identity_b = {2, 1}` and nothing else; else the System line `Feedback_DuelChallenge` (cat 110, name fed) and
`GuiSystem_c::DuelChallengeReceived` (GUI 0x1002fd9c): dialog "Duel Challenge" (window `DuelChallenge`), text the same string, buttons `MsgBox_Accept` / `MsgBox_Reject` (cat 10000); Accept -> `N3Msg_Duel_Accept`,
anything else (Esc too) -> `_Refuse` (0x1002f802). instance 1: `DuelChallengeSent` (0x1002ff80), one `MsgBox_Cancel` button -> `N3Msg_Duel_Refuse` (0x1002f825), text `Feedback_DuelChallengeSent`.
sub-op 1: close dialog + `Feedback_DuelAccepted`; 2: close + `DuelRefused` / `DuelRetracted` / `DuelAutoRefused` (instance 0/1/2); 3: `DuelDraw` (2) / `DuelWon` (1) / `DuelLost` (0);
4: `DuelDrawProposed` (1) / `DuelProposeDraw` (0). Unknown challenger dynel: nothing.

## Received pet duel (`FUN_1005c514`, cat 110 keys)
0xef `PetDuelChallenge`(name); 0xf0 by `identity_b.kind`: 0 `XRejectedDuel` / `OpponentRejectedDuel`, 1 `XAcceptedDuel` / `OpponentAcceptedDuel`, 2 `XinvalidOpponentDuel` / `TargetNotValidOpponent`,
3 `XbusyInDuel` / `TargetBusy` (named variant when `identity_a` is a known character), 4 `NotChallenged`, 5 `NeedDuelPet`, 6 `OpponentNeedsDuelPet`; 0xf3: `UwonPetDuel` / `UlostPetDuel` / `OpponentWithdrew` (0/1/2);
0xf8 `ChallengedX2duel`(name) / `ChallengedSomeone`; 0xf1 is ignored on receipt.

## State / PvP rules
The client keeps **no duel state**: `CanAttack` (`FUN_10069556`) has no duel test, the server answers a forbidden attack with action 0x76. (`InDuel` in Gamecode is an item-criteria text, `ConvertCriteria`.)
PvP stats `PVPDuel*` (stats 674..677, 684) are shown by the stat window (HudFinish).

## PvP confirmation, action 0x7b
`FUN_1005db15`: clears the `+0x79` guard of the char `identity_a` names, text `Combat_PvPTargetLvl` (`identity_b.instance == 0`) / `...Team` (cat 101), `GlobalSignals+0x54` =
`GuiSystem_c::StartPvPFightDialogue` (GUI 0x1002fa8e): dialog with `MsgBox_Yes` / `MsgBox_No`; Yes -> `N3Msg_StartPvP(identity_a)` [GC 0x10018276] = `FUN_10067c34(target, 0)` = AttackIIR flag 0 behind the guard.
**Unresolved**: whether the live server puts the own id or the target in `identity_a` (the original clears the guard of that character only).

## Other actions wired here
0x99 death cause -> `Combat::die`; 0xd0 drain (`SetStat(identity_b)`, "You drained" line); 0x64 play animation id; 0x84 "Unable to perform action" line. Table of all 106 ids: actions.md §7.
