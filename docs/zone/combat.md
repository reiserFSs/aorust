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
glue: Hit/SpecialAttack ─► Dynels::attack(id) (swing clip) ; own char ─► Player::play(Role::Clip("unarmed-rswing"))
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
* Own swing animation: always `unarmed-rswing`; the weapon swing lists (`combat-anim.md` §3) are not applied to the own avatar yet.
* The HUD floating-number reference y (`DAT_102761c0`) and the effect `0x2f5a` bitmap font (material 40): the numbers are drawn with the
  GUI `Shell` font at the lower third of the screen (own) / above the head (others), colours and life/rise speed from the client.
* Combat music `flag` input (`dynel+0x21c`, writer unknown) is fed as false; stat 421 as 0 (stored, never read by a decision).
* Hit reaction (`imp-*`) and miss/dodge animations: selector not found (combat-anim.md §8).
