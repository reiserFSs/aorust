# Combat and actions: how the pieces fit

Evidence for each piece is in its own document: outgoing messages / bindings [combat-net.md](combat-net.md), state machine, combat log
and floating numbers [combat-log.md](combat-log.md), animations / sounds / death / corpses [combat-anim.md](combat-anim.md),
`CharacterActionIIR_t` / sit / emotes [actions.md](actions.md), combat music `docs/formats.md` `### Combat music`. This file only
describes the glue (`crates/aomac/src/play/combat/{module,glue}.rs`).

## Data flow

```
zone frame ──► Module::on_frame ──► Combat::on_frame (state.rs)  ─► CombatEvent (Hit, Floating, FightStarted, Died, Health ...)
   │                         └────► Actions::on_frame (actions.rs) ─► Pose / Emote events
   └──► Zone::on_frame (stats, dynel table), Chat::on_zone_frame (combat-log lines in the chat windows: chat/log.rs)

keys / hotbar / chat ─► Cmd::{Attack,SwitchTarget,Special(stat),Sit} (controls.rs) | SlotUse::SpecialAction (hud) | GameAction::{Social,Assist}
                      ─► Play::fight_frame (glue.rs) ─► Module::{command,special_action,social,assist}
                      ─► frames (ao_net::n3::combat / action, ptype 0xA) ─► LoginSession::send_zone
Module::update ─► ao_audio::Audio::set_combat_char for every dynel (combat music, `FUN_10059736`) ; floating numbers age out ;
                  own death timer (`CharDie_t`, 3 s, then `CharacterAction` 0x98)
glue: Hit/SpecialAttack ─► `swing()` (glue.rs): weapon list 0xb / special list key of the wielded weapon's AnimSet (combat-anim.md §3) ─►
      Player::swing (own) | Dynels::play_once / attack (others; creatures = record key 0x40a) ; death/hit sounds ─► Dynels::char_sound
      Died(own) ─► Player::play(Role::Clip(anim_name(death anim)), hold) ; Health>0 ─► Player::stand ; fighting ─► `idle-unarmed` stance
      Emote ─► Dynels::play_once(id, social id) | Player::play(Role::Emote(name))
      Floating ─► `fight_draw`: HUD number (own char) or the number above the head (others) on the draw list
```

The fight state of every dynel changes **only when the server's relay arrives** (`SendIIRToObservers` has no local apply, combat-net.md §2):
the attack key sends `AttackIIR_t`, the echo starts the fight, the next press sends `StopFightIIR_t`, its echo ends it. `AttackGate` is the
`+0x79` "please wait until previous action" guard (no timer in the client, so an attack the server never answers blocks later ones).

## Controls (CharPrefs.xml `KeyBindings`, provider = `provider_hash(name)`)

| key | provider | action |
|---|---|---|
| Q | `ACTION_ATTACK` | `FUN_1004256c(0xb)` → `DefaultAttack(selected target, false)` (toggles: pressed while fighting sends StopFight) |
| SHIFT+Q | `ACTION_SWITCHTARGET` | `N3Msg_SwitchTarget` = `DefaultAttack(target, true)` |
| X | `ACTION_SIT` | `N3Msg_SitToggle` (stop attack, then sit / `CharacterAction` 0x57) |
| B K J N M L O , . | `ACTION_BRAWL` `DIMACH` `SNEAKATTACK` `FASTATTACK` `BURST` `FLINGSHOT` `AIMEDSHOT` `FULLAUTO` `BOWSPECIALATTACK` | `N3Msg_SecondarySpecialAttack(target, stat)` with stat 0x8e 0x90 0x92 0x93 0x94 0x96 0x97 0xa7 0x79 |
| TAB … | (fixed, GUI.dll) | target cycling, owned by `hud_target.rs` |
| `/<emote>`, `/emote <name>` | chat | `SocialActionCmd_t` (`Module::social`), refused while swimming |
| `/assist` | chat | selects the selected character's fight target (`Module::assist`) |

Key ids were computed from the shipped CharPrefs.xml (`provider_hash` of each `ACTION_*` name matched against the `KeyBindings` entries);
`controls.rs` has the same table as `DEFAULT_BINDINGS`.

## Unresolved / not ported (honest list)

* The weapon-specific parts of `N3Msg_SecondarySpecialAttack` (`FUN_10063be4` recharge, `FUN_100686fb` weapon slot availability,
  `FUN_100679c1` range, `FUN_10058908` line of sight): the server refuses such attacks itself; the client text for the refusal arrives as a
  `FormatFeedback`/`CharacterAction` 0x76 and is printed by the chat layer.
* Own swing animation: the wielded weapon's list (combat-anim.md §3); bare hands, own-item specials and AnimSet 4/5 play `unarmed-rswing` (item record multimap layout not decoded, labelled GUESS).
* `/duel`, `/petduel`, the received PvP / duel action ids: [combat-duel.md](combat-duel.md), [actions.md](actions.md) §7.
* The HUD floating-number reference y (`DAT_102761c0`) and the effect `0x2f5a` bitmap font (material 40): the numbers are drawn with the
  GUI `Shell` font at the lower third of the screen (own) / above the head (others), colours and life/rise speed from the client.
* Combat music `flag` input (`dynel+0x21c`, writer unknown) is fed as false; stat 421 as 0 (stored, never read by a decision).
* Hit reaction (`imp-*`) and miss/dodge animations: selector not found (combat-anim.md §8).

## Live result (Ithaca, pf 4582 beach; headless `live_walk` sessions, no desktop capture)

**Selection must be announced first.** The original client sends `LookAtIIR_t` (`2252445F`, body `Identity target, i32 mode`, 25 bytes,
header flag 0) whenever the selection changes: `FUN_1003fb35` [GC] (the SetTarget handler) -> `FUN_1003b73e` (characters, mode 1; also
sets own stats 0x1af = 3 and 0x18d = a relation bitmask) or `FUN_1003b0db` (anything else / no target, mode 0; stats 0x1af = 4, 0x18d = 8), writer
`FUN_10074f41`, ctor `FUN_10074f69`. Without it the server refuses every attack: it answers `CharacterAction` 0x93 then 0x76 with instance 6
(`Feedback_PvpNotAllowedInThisDistrict`, jump table 0x1005f0e7; capture `zone_attack_refused_ithaca.rec`). `combat::look_at` +
`Module::announce` (before any command and every frame) fix that.

With the announce: Aomacvolk (lvl 1) vs a Beach Leet (12 HP), capture `docs/captures/zone_fight_ithaca.rec` (test `live_fight_capture`):
`> LookAt(leet,1)`, `> Attack(leet,0)`, `< Attack` echo (fight starts), AttackInfo hits of 4 / 4 / 5 (crit, unk_30 = 4) and one miss, the leet hits
back (7), `< StopFight`, `< StatIIR` 0x34 (XP) = 145 ("You received 145 xp." and the yellow 145), "You can loot these remains." (corpse), then
the deselect `> LookAt(none,0)`. Window captures inspected: world damage number above the target, own HUD numbers at the left edge, the combat
log lines ("You hit Beach Leet for 4 points of projectile damage" — that was the old hard-coded type, the original prints the slot item's type, bare hands = melee: docs/zone/combat-log.md §2.1.1; "You tried to hit Beach Leet, but missed!"), the XP bar filling.
Observed gaps of that capture, status after the wiring pass: the maximum health was `1` because the own `FullCharacter` carries `Life` (1) = 1 and
`Combat` stored it over the header's value (now skipped, `state.rs` test `full_character_life_does_not_replace_the_header_max_health`; the Hud bar
shows `40 / 40` for the lvl 2 Aomacvolk live, the maximum itself is computed by `hud_pools`); the dead leet standing drawn with the selection box:
the death clip holds until the server's `n3ToClientQuit` ~3 s later (`Dynels::die`, `Zone` removal, `hud_target` clears the selection), no extra
state found in the capture replay (`dynels::tests::replayed_kill_plays_the_death_clip`).

Live re-check (this build, headless harness with the real audio engine, `AOMAC_AUDIO_LOG=1`): login, HUD `40 / 40`, `LookAt` + `Attack` accepted
from the cliff top above the beach (the server did not deal hits at 15 m height difference), world numbers / swings of other dynels and door/fight
sound calls (`game sound <id> ...: N voice(s)`) appeared. **Not re-run:** the kill of a Beach Leet with own swing / hit sound / corpse / XP in this build,
because the saved character position kept being moved by other sessions (beach reachable only by a ~1000 m swim and a ledge, then the position
was moved to another playfield); the previous capture above remains the evidence for kill, corpse and XP.

## Live: own death in Borealis (pf 800, lvl 2 Aomacvolk, 40 HP, vs Fresh Engineer lvl 7 / 160 HP)

Capture `docs/captures/zone_death_borealis.rec` (redacted excerpt: only frames addressed to the two characters; test `live_death_capture`). Harness
run with `AOMAC_COMBAT_LOG=1` (prints the fight events), `AOMAC_AUDIO_LOG=1` (real audio engine, **muted** unless `AOMAC_AUDIO_UNMUTE` is set:
`Mixer::output` is applied after the level statistics, so voices / RMS are still logged), window shots inspected:
* `Q` -> `LookAt` + `Attack`; `CombatMusic(true)`, the music switched `MN03.wav` -> `MN10.wav` -> `MN08.wav` while fighting (`audio` step).
* Lines: "You hit Fresh Engineer for 3 points of projectile damage." (old hard-coded type, now melee for bare hands), "You tried to hit Fresh Engineer, but missed!", red "Fresh Engineer hit you for 11 / 12 / 17 points ...";
  world damage number "3" above the target; the own avatar plays its swing between the engineer's hits (shots a3..a6).
* Health 8 -> -4 (`Health` -12): `FightStopped`, `CombatMusic(false)`, `DeathMusic(true)`, then `Died { cause: 0 }` (server `CharacterAction` 99): the
  death sound plays at the camera (1 voice), the avatar lies on the ground holding the death clip (shot a8), the stats window shows the unsigned
  `4294967292 / 40` (`FUN_1007f8c4` formats `%u`, stat_view.rs: the original behaviour as read, the bar is full).
* ~2 s after the death the server sends "This XP was added to the pool of unsaved experience points ...", "Locating next playfield server", a zone
  redirection and "NEW LOC: 679.6 72.8 476.7 / Entering 'Borealis'": the respawn at the playfield's start with 4 -> 10 HP (regen +3 per tick). The
  client's own `CharDie_t` timer (3 s, action 0x98) had not elapsed when the zone changed, so no 0x98 was sent (the `own_death...` unit test covers
  the timer).
* **Not confirmed live**: own kill, corpse, XP and loot. A lvl 2 character does 3 damage per hit against 160 HP mobs (the lowest attackable mob in
  reach; Uncle Pumpkin-Head 332 HP), and the engineer kills it in ~4 rounds. The kill / corpse / XP evidence remains `zone_fight_ithaca.rec`.

## Live: own kill on the ICC beach (new character Aomacrceg, lvl 1 Solitus Soldier, bare hands)
Harness: `AOMAC_LIVE_NEW=<name>:<CC breed>:<CC profession>` creates a character (the creation module's request, `create/scenes.rs::live_create`; Atrox (CC breed 7) is
refused by PRK with login code 7 `CharacterProblem`, Solitus male Soldier is accepted), `goto=193:157` leaves the Arrival Hall (pf 4604) through the shuttleport tunnel
into pf 4582 at (931, 20.6, 729), `goto=hunt` walks to the weakest hostile (Beach Leet first) and selects it, `Q` attacks, `ruse=corpse` / `lootid=` / `loottake=` loot.
Frames inspected (`/tmp` shots, deleted): own avatar mid-swing with the arm raised (f5), the target frame and its yellow health bar, the leet's death: the selection box and the standing
model are gone at once, the corpse lies at the feet; chat lines "You hit Beach Leet for 5 points of **melee** damage." (bare hands = slot 0, melee: docs/zone/combat-log.md §2.1.1),
"You received 145 xp." (XP bar 145 / 1450, second kill 290), "You can loot these remains."; the second kill gave the same. Music `MN01/MN03/MN09` switched on `CombatMusic(true)`
and `LosingMed10` when the fight ended. Muted run: the voices (1-3) and the death sound at the camera are logged (`game sound ...: 1 voice(s)`).
Bare hands hit chance is low (about 1 hit in 3) and the character died three times against lvl 1 mobs (death flow as in Borealis, respawn at the beach start with
low health); two kills were made with the sit-rest between fights (`X`).
Capture `docs/captures/zone_kill_ithaca.rec`, test `module::death_tests::live_kill_capture`.
**Loot**: the corpse opens (2 items + Cash +1) but every `MoveItemToInventory` variant is ignored (`docs/captures/zone_loot_own_kill_ithaca.rec`, docs/zone/interact.md §9, §12.3): open.
**Weapons**: a new character has an empty inventory (no starter items), so wielding / unwielding and the weapon swing lists could not be tried live.

## Attack sounds, hit reactions (offline-verified, live check pending)
`docs/zone/combat-anim.md` sections 4 and 6.1: the swing / swish / impact sounds come from the animation notes of the swing clip (`combat/notes.rs`), a miss swings like a hit, a struck character
plays an `imp-*` clip. Offline evidence: the unit tests listed there (real item records, real swing clips, the captured Beach Leet / player).
**Live check** (muted, `AOMAC_AUDIO_LOG=1 AOMAC_COMBAT_LOG=1`, steps `goto=hunt`, `Q=0.1`, as in the kill run above): per swing the log must show `combat: note 0x77` / `0xb` (a creature's `attack_start_1`
and `attack`, the own bare-hand `swish_punch` 0x73 and `attack` 0xb) and, for the own bare hands, `game sound 3238527353 at [..]: N voice(s)` (= `0xc1080179`, the martial-arts swing list 0xb) at the own
position on a hit (hit kind 3 / 4), only the swish (`SM_Sandy_Swish_punch`) on a miss; 0.4 s after a creature's hit on the own character `game sound <MaleGetsHit / FemaleGetsHit id> at <camera>: N voice(s)
(material 7, size <0/1/2>)` (size 2 from 6 damage, 1 from 3). A Beach Leet has no `FabricType`, so the own hits on it have no impact sound (a swing sound only); the own `0xd1` hit cue still plays
`char_sound` without material.

## Live: Atrox, equip, rifle fight, fight sounds (2026-10-06)
* **Atrox**: the request carried sex 2; the original's `ConvertCCBreedToGCBreedAndSex` (GUI 0x101225f7) sends sex 1 (unisex) for Atrox. With `gender: 1` the server accepted `Aomacvktq` (Atrox Enforcer: `breed 4, gender 1, profession 9, head 40111`).
* **Equip** (Aomacvolk, Borealis): `dclick=41` wore the "Solar-Powered Assault Rifle" (bag 0x41 -> right hand 6): `ContainerAddItem`, `WeaponItemFullUpdate`, an `AppearanceUpdate` with the rifle mesh; Wear window shows it, the avatar holds the rifle in the rifle stance;
  `dclick=06` unwears (CharacterAction 0x61), `dclick=41` wears it again; capture `docs/captures/zone_wear_rifle_borealis.rec`.
* **Rifle fight vs Fresh Engineer**: lines "You hit Fresh Engineer for 4 points of **projectile** damage." (weapon stat 0x1b4 = 0x5a; bare hands print melee), own `combat: note 0xb` + `game sound 3130317467 ... 2 voice(s)`
  (the weapon's attack sound) per shot and the victim impact `game sound 2934702682 ... (material 7, size 1|2)` 0.4 s after a hit. The character died again (lvl 2, 40 HP vs a lvl 7 engineer).
* Bare-hands sounds (`0xc1080179` swing sound, swish notes) were not re-run live after the wiring; rifle shots were.

## Live: creation straight into the world, draw / holster (2026-10-06)
* **Post-create flow fixed** (docs/screens.md §7): `ZoneHandoff` was swallowed by the creation module so `Zone::new` never ran, and `show_loading` discarded the loaded world. Live: `AOMAC_LIVE_NEW=Aomacmtfa:2:4` (Solitus Soldier) created and reached "in world" (Arrival Hall 205.2, 1.0, 255.8) in 20 s;
  `AOMAC_LIVE_CHAR=Aomacvktq` (Atrox Enforcer, created earlier with sex 1) logs in, 60 / 60 HP, Atrox model in the Arrival Hall (frame inspected). After a session the login server answers error 106 for a few minutes (wait ~4 min).
* **Wield stance** (live frames): out of a fight the worn rifle hangs at the side (`idle-2h`, 0x41e), unwearing returns to the plain idle with empty hands; draw / holster (`rifle-start/stop` lists 0x1a/0x1b) belong to fight start / stop (docs/zone/combat-anim.md §4), too short to catch at the harness frame rate.
* **Other characters' `AppearanceUpdate`** (weapons, cloth, heads) now rebuild their model (docs/zone/avatar.md §5); only headless tests (`other_player_wields_and_unwields_live`).
