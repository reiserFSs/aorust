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
log lines ("You hit Beach Leet for 4 points of projectile damage", "You tried to hit Beach Leet, but missed!"), the XP bar filling.
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
