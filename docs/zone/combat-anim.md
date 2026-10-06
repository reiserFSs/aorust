# Combat animations, fight sounds, death and corpses

Code: `crates/aomac/src/play/combat/anim.rs` (pure lookups + state; tests `cargo test --release -p aomac combat::anim`, real-data
tests skip without the client). Labels: **[CODE]** read in the Ghidra decompile of `Gamecode.dll` ("GC"), **[DATA]** decoded from
`rdb.db` / `cd_image/sound` / captures (`docs/captures/zone_ithaca.rec`), **[INFERENCE]**, **[UNRESOLVED]**.

## 1. Pipeline: AbstractAnimID -> clip

1. The game code never names clips. It asks for an **`AbstractAnimID_e`** (a small int) and the animation holder
   (`AnimHolder_t`, `SimpleChar+0x1dc`; ctor `FUN_1003c525` [GC 0x1003c525], vtable 0x1015d580: `[1]` idle update `FUN_1003cad0`,
   `[2]` `FUN_1003cc15`, `[3]`/`[4]` start/stop stance `FUN_1003c930`/`FUN_1003c9b2`, `[5]` **Play** `FUN_1003ca30`, `[6]` `FUN_1003c392`)
   turns it into a clip with `FUN_1003c8b7` -> `FUN_1003c802` [GC 0x1003c802]:
   * loop `id -> FUN_10010ad1(id) -> ...` (stops on a cycle), at every step first **`FUN_1004570c(id)`** on the character's own
     anim multimap (NPC records: `NpcRecord.anims`, `docs/zone/npc.md`; random variant), then - only if `sex != 0 && breed != 6`
     and the char's `idle-stand` (0x78) clip has the human skeleton hash `0xbd945cbe` (`-0x426ba342` test) - the **name rule**
     `FUN_10010c57` [GC 0x10010c57].
2. **Name rule** (`clip_candidates`): ids `< 0x48` are social actions: `<set>_social-<name>.ani`, then `<set>_<name>.ani`
   (`FUN_100bfef6` [GC 0x100bfef6] social table; its `alt` column (`allah`, `ass`, `crotch`, `kiss_01_01`, ...) is preferred when non-null);
   all others `<set>_<name>_01_01.ani`, then `<set>_<name>.ani`, `<name>` from the table `FUN_100c01c9` [GC 0x100c01c9] (197 entries, `ANIMS`).
   `<set>`: breed 4 (Atrox) `athrox`; `sex == 1` or breed 5 `male`; else the sex name (`male`/`female`; opifex and nanomage use these)
   [CODE `FUN_10010c57` + DATA name table]. The table's second field is the **layer**: 3 -> `Layer_e` 0 (whole body), 2 -> 1
   (overlay: `op1h-button`, the 8 `wield`, `opself-firstaid`) (`*out = ~kind & 3`).
3. **Fallback chain** `FUN_10010ad1` [GC 0x10010ad1] (`fallback`, same as `ao_formats::character::fallback_key`): rifle-start 0x3fc->0x3f2,
   idle-rifle 0x3fd->0x3f3, rifle-stop 0x3fe->0x3f4, rifle-shot 0x3ff->0x3f5 (smallarms), idle-2h 0x41e / idle-hover 0xb2 -> idle-stand,
   walk-2h 0x421 / 0xb3 / sneakcool 0x66 -> walk, run-2h 0x422 / hover-fast 0xb4 -> run, idle-bazooka 0x424 -> idle-rifle, bazooka-shot
   0x426 -> rifle-shot, imp-back 0x7e and 0x80..0x84 -> imp-chest 0x7f, **die-crawl 0xcc and die-* 500..505 -> 6000 (`die-pain`)**,
   turn-left/right 0xc4/0xc5 -> walk-left/right 0x86/0x87 only while the client char is in a vehicle (stat 0x296).
4. [DATA] verified against the real name table (`fight_clips_exist_in_the_data`): every id used below resolves for `male`, `female`
   and `athrox`; all but two **directly** (idle-stand, walk, run, die-*, imp-* except legs, every weapon stance/attack id, unarmed
   0x409/0x40a). Only **imp-legs 0x83** (no clip) resolves through the chain to imp-chest, and **die-crawl 0xcc** to 6000.

## 2. Character state machine (`CharStateBase_t`, `SimpleChar+0x1c0`)

Factory `FUN_1007be9f` [GC 0x1007be9f]: id 0 `CharIdle_t` (update `FUN_1007b88a`), 4 `CharMove_t`, 5 `CharCastNano_t`
(`FUN_1007b084` / update `FUN_1007ac9a`; plays the nano item's animations `0x178/0x179/0x17a`, sounds `0x10d/0x10f/0x110`),
7 `CharTurn_t` (`FUN_1007c366`), 10 `CharFight_t` (ctor `FUN_1007b816`, update `FUN_1007bb81`), 0xb `CharDie_t` (ctor `FUN_1007b2ba`,
update `FUN_1007b58c`). Fight controller `SimpleChar+0x1d4`: `+0x44` fight state (1 = not fighting), `+0x4c/+0x50` target.
Transition to death: any of idle / fight / nano / turn updates tests **dynel stat-flag word `+0x138` bit 4 (0x10)** -> state 0xb.
Fighting and not dead -> 10 (`CharFight_t` ctor: with a target set while fight state is 1 it calls the start-fight routine `FUN_10069c68`, then the holder's idle update).
The app has no per-character state objects, so the state ids (`CharState`), the dead flag mask (0x10) and the human skeleton hash (`0xbd945cbe`, section 1) are **not mirrored in code**
(they were unused tables and are deleted): death is the `CharacterAction` 99 -> `Dynels::die` / `Dying` path (section 5), the clip names come from the name table of the model's own set.

## 3. Attack swing

`FUN_1006a239(slot, 0)` [GC 0x1006a239] runs per hit of `AttackInfoIIR_t` (apply `FUN_1009ed0d` -> `FUN_1006a8f3` -> `FUN_1006a239`):
* **Creature** (mesh without weapon attractors, `VisualCATMesh_t::HasWeaponAttractors` false, `FUN_100679a0`): `AbstractAnimID 0x40a`
  = `unarmed-rswing` (1034), resolved through the NPC record's anim table.
* **Wielded weapon** (`FUN_10068072(slot)` entry, `+0x14` = the item): `FUN_10069acb` returns `FUN_1004570c(0xb)` = a random value
  of the **weapon's list 0xb**. For a queued special attack it first calls `FUN_1003c594` (section 3.2).
* The id then goes through the holder (section 1). Holder `Play(clip, 1.0f, -1, 0, listKey, 1, 0, 0, slot)`; list keys already
  playing (`FUN_1003bfd2`) are not restarted. If no clip is found and `char+0x21c == 0`: `FUN_10010d83(char, 0x3e, 1)` (the social
  `wave`!) [CODE, taken literally].
* **Speed** (`swing_speed_scale`): if `char+0x21c == 0` and the mesh exists: `SetSpeedScale(clip, clamp(noteTime_ms / (ItemDelay*10), 1, 2))`
  (`ItemDelay` = weapon stat 0x126 = 294, `_DAT_101600f0` = 10.0 double, cap `_DAT_10158794` = 2.0f): the swing is only ever sped up so
  its first note lands within the weapon delay. **[CODE, DisplaySystem.dll]** `VisualCATMesh_t::GetNoteTime(clip, 0)` [DS 0x10073386] returns
  entry 0 of the clip's `AnimMetadata` (stride 0x28, float at +0x24; 0 when the clip has none); `SetSpeedScale` [DS 0x100737c2] stores the scale
  at `AnimStruct+0x20`. The metadata entries are the CAT clip's named events (`CatAnim::events`, `(time ms, name)`), so entry 0 = the clip's first event.

### 3.0 What the client runs (`combat/glue.rs::swing`, `Dynels::pick_swing`)
Every `Hit` / `SpecialAttack` event of any character:
1. **Weapon swing**: the holder's wielded weapon (`Dynels::wield`, the `WeaponItemFullUpdate` children in body slot 6 = right hand, 8 = left;
   `AnimSet` 0x161 and `ItemDelay` 0x126 from the item template under the message stats) -> `weapon_list(AnimSet, left, crawl = false, key)`
   (`key` = `0xb`, or the special's list key, else `0xb` when the weapon has no such key) -> random value (the CRT-rand stream of `Dynels`)
   -> AbstractAnimID -> the own avatar plays `Role::Clip(anim_name(id))` (`Player::swing`, rate x `swing_speed_scale(first event time, ItemDelay)`,
   `Avatar::set_swing_delay`), every other character `Dynels::play_once(id, anim)` (NPC record table or the set's file name through
   `resolve_clip`, same speed scale in `Dynels::update`). [DATA] all weapon attack ids exist as clips for male / female / athrox (test `fight_clips_exist_in_the_data`).
2. **No weapon list** (nothing wielded, `AnimSet` 4/5/other, or a special whose list lives on its own item: Brawl, Dimach, Backstab, bow special): creatures
   play key `0x40a` = `UNARMED_RSWING` (1034) of their NPC record (`Dynels::attack`; the earlier code used 1033 = `unarmed-kick`/`0x409`, which is the
   *spray* clip of creature records, npc.md section 3), the own player plays `unarmed-rswing`. **[UNRESOLVED]** the original's bare-hand / martial-arts list lives in the
   special's / template item record (`+0xe4`, `UnarmedTemplateInstance`), whose layout is not decoded; `unarmed-rswing` is the creature path of the same function, not a verified
   player choice. The crawl lists (`crawl = true`) are not used: stat 0x1ae is not tracked per dynel.
3. **Text above the character** for special attacks (`FUN_10011108(char, "Burst!\n", 0xd)`): `Module::on_frame` pushes a world-space `Number` (category 0xd = colour
   `0x00f000`, effect `0x2f5a` path, `combat-log.md` section 6) with the text without its trailing newline. The sound of Brawl / Dimach is in section 6.
* A 0/1/2 "height" variant (`param_3` of `FUN_1003c8b7`) is computed for `AnimSet == 0` weapons from the muzzle attractor
  (`AttractorMesh::GetName(0)`) vs the target's height and distance < 6 m (`_DAT_1015d0a0`; < 3 m `_DAT_1015d69c`); what it selects inside
  `FUN_1003c802` is **[UNRESOLVED]** (not modelled).

### 3.1 Weapon action lists (`weapon_anims`, `FUN_1009d41a` [GC 0x1009d41a])
Built when a weapon is wielded: `(list key -> AbstractAnimID)` multimap, selected by the item's stat **`AnimSet` (0x161 = 353)**;
`left` = slot 8, `crawl` = wielder stat 0x1ae == 0xe (start/idle/stop -> `0xe7/0xe9/0xe8`; rifle `0xcd/0xce/0xcf`, attacks 0xd1 / pistol 0xeb(R) 0xea(L)).

| AnimSet | weapon | start 0x1a / idle 0x10 / stop 0x1b | attack 0xb (random) | other keys |
|---|---|---|---|---|
| 0 | pistol / smallarms | 0x3f2 / 0x3f3 / 0x3f4 | 0x3f5 (L 0x3f8) | burst 0x16: 0x3f6 (L 0x3f9); full auto 0x17: 0x3f7 (L 0x3fa); aimed 0x15, fling 0x1c: shot |
| 1 | 1H blade | 1000 / 0x3e9 / 0x3ea | R 0x3eb, 0x3ec, 0x3eb; L 0x3ee, 0x3ef, 0x3ee | fast 0x19: 0x3eb (L 0x3ee); sneak 0x1d: 0x3ec (L 0x3ef) |
| 2 | 2H blade | same as 1 | 0x41a, 0x41b, 0x41c, 0x41d | sneak 0x1d: 0x41b; fast 0x19: 0x41d |
| 3 | rifle | 0x3fc / 0x3fd / 0x3fe | 0x3ff | burst 0x400; auto 0x401; 0x27/0x29/0x28/0x2a/0x2b = 0x41f/0x41e/0x420/0x421/0x422 |
| 6 | bow | 0xb5 / 0xb6 / 0xb8 | 0xb7 | 0x27/0x29/0x28 = 0xb5/0xb6/0xb8 |
| 7 | (tool / system) | idle 0xcb | 0xcb | |
| 8 | bazooka | idle 0x424 | 0x426 | 0x29 = 0x424, walk 0x2a = 0x421, run 0x2b = 0x422 |
| 4, 5, other | - | nothing is registered | | lists come from the item record |

[DATA] Weapons whose **record carries its own multimap** use key 0xb directly: 119 540 item records (rdb 1000020) were scanned for
`{key 0xb, (n+1)*1009, values}`: martial-arts items list `{1034, 1035, 1037, 1033}` (unarmed-rswing / uppercut / lswing / kick; e.g. records
43712, 43713), many single-entry `{1034}` / `{1033}` records (45605, 56180, 100237, 120637...). The item-record layout is **not decoded**.
Unarmed fists: the char's `UnarmedTemplateInstance` (stat 418) is 0 for the own char in the capture, so which list a bare-handed
player uses is **[UNRESOLVED]** (by the code: empty list -> id 0 -> the `wave` fallback above, which cannot be intended; the entry `+0x10` of
the slot probably holds a client-made template item).

### 3.2 Special attacks (`special_swing`, `FUN_1003c594` [GC 0x1003c594])
`FUN_1006855a` (queued special's skill stat) -> list key; the key is looked up on the weapon (`FUN_1004570c(key)`, else 0xb):

| stat | name | key | text above the char (`FUN_10011108(char, text, 0xd)`) | sound |
|---|---|---|---|---|
| 148 | Burst | 0x16 | `Burst!` | |
| 147 / 146 | Fast Attack / Sneak Attack | 0x19 / 0x1d | `Fast attack!` / `Sneak Attack!` | |
| 150 / 151 / 167 | Fling Shot / Aimed Shot / Full Auto | 0x1c / 0x15 / 0x17 | `Fling Shot!` / `Aimed Shot!` / `Full Auto!` | |
| 142 | Brawl | 0x23 | `Brawl!` | `SM_Sandy_Game_Brawl` at the char (AnimHolder `+0x40`) |
| 144 | Dimach | 0x24 | `Dimach!` | `SM_Sandy_Game_Dimach` (`+0x44`) |
| 489 | Backstab | 0x89 | `Backstab!` | |
| 121 | Bow special | 0x87 | | |
(Brawl, Dimach, Backstab, Bow special look the key up on the special attack's own item `FUN_100686d0(stat)+0xe4`.)

## 4. Idle / fight stance, hit, miss
* Idle clip of a wielding char: AnimHolder vt[1] `FUN_1003cad0`: first present of the weapon slots 6, 8, 0 -> list **0x10** (random), else 0x78;
  skipped unless `FUN_10059ac4()` is false; the clip is started at a time offset derived from the length of the list-0x1a clip. Stance start/stop (`FUN_1003c930`/`c9b2`): lists 0x1a/0x1b
  (draw / holster; `FUN_1003c930`/`c9b2` take two ids each: play the first once, start the second). Walk/run with a 2H stance: lists 0x2a/0x2b (rifle/bazooka).
  Stance ids per weapon type: section 3.1 (`blade-start/idle-blade/blade-stop` 1000-1002, `smallarms` 1010-1012, `rifle` 1020-1022,
  `unarmed` 1030-1032, `2h` 1055/1054/1056, `bow` 0xb5-0xb8). **There is no separate "fight stance" clip**: being in `CharFight_t` only keeps the weapon
  idle (list 0x10); a character that is not fighting plays plain 0x78/walk/run.
* **Being hit** (`CharacterActionIIR_t` action **0xd1**, `FUN_1005d0d8` case 0x5b [GC 0x1005eada]): if `identity_a.instance` and `identity_b.instance` (the handler's two `Identity*` arguments; `param` is not read) are both `> 0`, play the hit sound at the
  victim's position (section 6). **No hit-reaction animation was found**: the `imp-*` clips (0x7e..0x84) exist in the data and the engine
  has `n3VisualDynel_t::GetImpactAnim` (base returns 0, [N3 0x10007572]; Gamecode export thunk 0x10131f02) but the override that picks an
  `imp-*` id was not located. **[UNRESOLVED]** (searched: immediate-id scans of Gamecode.dll, callers of the thunk).
* **Miss** (`MissedAttackInfoIIR_t`, apply `FUN_1006ae50`): combat-log text only; no dodge/miss animation or sound exists in the code paths read.
* **Pose enter / stop clips** (sit, sleep, lounge, crawl; `actions::Pose::transition_anim`, glue `ActionEvent::Pose`, `Player::update`): the FSM entry handlers call
  `FUN_1006d330(holder, idle, enter, ...)` [GC 0x1006d330] = store `idle` as the holder's current idle (`FUN_1003c4e1`), `Play(enter)` once, start the idle when the enter clip is nearly over:
  SwitchToSitGround `FUN_1006e2be` (idle 0xd7 `idle-ground`, enter 0xd5 `ground-start`), LeaveSit `FUN_1006e372` (0xd6 `ground-stop`), sleep `FUN_1006e6ff`
  (0xee, enter 0xed `sleep-ground`), lounge `FUN_1006e8a9` (0xf0, 0xef `lounging`), crawl enter `FUN_1006e646` (0x9a `idle-crawl`, 0x68 `crawl_start`), crawl leave `FUN_1006e3ff`
  (0x69 `crawl_stop`); `FUN_1006c065` re-applies the idle per mode (7 / 8 / 5 / 0xb / 0xc). Sitting on an item uses the same mode-8 handler: the `chair-*` ids (0xd8..0xda)
  are pushed by no handler (immediate scan of Gamecode.dll for 0xd8/0xd9/0xda), so there is no chair clip. The idle clips are the movement state's own (`AnimState` / the avatar's
  `Role`), the enter / stop clip is played over them for other dynels (`Dynels::play_once`) and the own avatar (a `Role::Clip` transient). **[UNRESOLVED]** leaving sleep / lounge to
  sitting (`FUN_1006e79f` / `FUN_1006e949`) calls the same helper with `param_5 = 1` and the *enter* clip (0xed / 0xef), taken as backwards playback: not reproduced (neither
  `Dynels` nor `Avatar` plays a clip backwards), the character goes straight to `idle-ground`.

## 5. Death
### 5.1 NPCs and other characters (live, 15 of 15 in the capture)
Sequence **[DATA]**: `CharacterActionIIR_t` (header = the dying char) **action 99 (0x63)**, `param 0`, `identity_a {0,0}`, `identity_b {0, 503}`
-> **2.92 - 2.99 s later** (`n3ToClientQuitIIR_t` for that dynel) the server removes it. If the player killed it, a `CorpseFullUpdateIIR_t` follows
**1 ms after the quit** (owner = the dead dynel; 1 corpse for 15 deaths: only own kills). No `Health = 0` stat update precedes it.
Apply **[CODE]** `FUN_1005d0d8` case 0x19: `statsys->vt+0x18(0x10)` (set stat flag **0x10** = dynel `+0x138` bit 4, the "dead" test of all states;
bytes `6a 10 .. ff 50 18` @0x1005d835), then `SetStat(0x183, identity_b.instance)` (`push 0x183` @0x1005d845; stat 0x183 is unnamed in the client table) and
`FUN_10059ae5`. `CharDie_t` (`FUN_1007b2ba`) then: `FUN_10068b7f(1,0)` stop fighting; **sound** = NPC-record sound list key `0x1e` if `FUN_10051f6e()` is non-zero (**[INFERENCE]**: a char with an NPC record), else `SM_Sandy_Game_FemaleDies` if Sex stat (0x3b) == 3 else `SM_Sandy_Game_MaleDies`, positional at the char (`PlayGameSound`
with the char's global position); **animation** = AbstractAnimID `GetSkill(0x183)` played with `Play(id, 1.0f, -3, 3, -1)` (hold the last frame;
503 = `die-shot` live; 500 knees, 501 pain, 502 poison, 504 ground, 505 float, 0xcc crawl; all fall back to 6000 `die-pain`); stat flag 0x20 set; effect
`FUN_10002e6a(9)`. Update `FUN_1007b58c`: timer `+= dt`; when `> 3.0 s` (`_DAT_1015d69c`) and the char is the control char (`+0x140`) it sends
`CharacterActionIIR_t` action **0x98** (empty text) and returns to idle; otherwise the char waits for the server's quit (2.92-2.99 s < 3.0 s, so the quit always wins for NPCs).
`DIE_WAIT_S`, `ACTION_DEATH_DONE`, `Dying` / `DieEvent` (own character, `Module::update`), `die_sound`, `death_anim_from_action` (also stores stat 0x183 in `Combat`, `STAT_DEATH_ANIM`, read back by `Module` when the own char dies; `Dynels::die` plays the clip) implement exactly this.
(Other CharDie branches - effect 3000 `CreateEffect2`, `SetStat(Health, 1)` after the clip ends for chars with `+0x21c == 0` - are read but their purpose is **[UNRESOLVED]**.)

### 5.2 The own character
No separate "you died" message was seen in the capture (the own char did not die). By the code the same path runs (flag 0x10 -> `CharDie_t`, control char sends action
0x98 after 3 s). Related hooks **[CODE]**:
* `n3EngineClientAnarchy_t::ToClientDynelDead` [GC 0x10015bf2] -> `FUN_100045b3` = `AFCM::Send(10, 0xa6)`; the GUI's `FlowControlModule_t::DeadMessage` [GUI 0x100288fa]
  emits `GlobalSignals+0x24`, sets `InputConfig_t::m_isAlive = false` and clears static input mode 8 (movement/input lock). Who calls `ToClientDynelDead` (N3 side,
  vtable ref 0x1004e484) was not traced.
* `ResurrectIIR_t` (0x445F2A0B, vtable 0x10161290, apply `FUN_100769bd`): body `i32 health, i32 nano` -> `SetStat(Health 0x1b, health)`, `SetStat(CurrentNano 0xd6, nano)`,
  then `FUN_10058816` / `FUN_1003ea0d` (purpose **[UNRESOLVED]**). `FlowControlModule_t::OnCharResurrectedMessage` [GUI 0x10027d47] raises the tip event `OnResurrect`; the death raises `OnDeath`
  (`FUN_100331d3`, TipSystem). Feedback strings: `Feedback_DeathByWeaponDamage/SpellDamage/FallDamage/LiquidDamage/ReflectDamage/ShieldDamage/Terminate`,
  `Feedback_NotWhileInResurrectionShock`, `Feedback_ResurrectionShockFillsYourBody`. The music hook is `SandyInterfaceModule_t::SetDeathMusicMode` (SI export 0x15a).

## 6. Sounds
All through `SandyInterfaceModule_t::PlayGameSound(soundId, pos...)` [GC `FUN_1001117d`], ids from `GetSoundID(name)`; attenuation rules in docs/formats.md `## audio`
(linear to `max_dist`, positional only by distance, no pan). [DATA] (`sounds_exist_in_the_data`):

| name | when | definition |
|---|---|---|
| `SM_Sandy_Game_MaleDies` | death, Sex != 3 | file `sfx/breeds/male_die.wav` |
| `SM_Sandy_Game_FemaleDies` | death, Sex == 3 | 2 children (`sfx/breeds/` has `female_die.wav`, `female_die_02.wav`) |
| `SM_Sandy_Game_MaleGetsHit` | CharacterAction 0xd1 (hit), Sex != 3 | `random_child`, 5 children (`sfx/breeds/male_hit_01..05.wav` exist) |
| `SM_Sandy_Game_FemaleGetsHit` | same, Sex == 3 | 4 children (`sfx/breeds/female_hit_01..04.wav` exist) |
| `SM_Sandy_Game_Brawl` / `_Dimach` | special attack Brawl / Dimach | positional at the char |
| creatures | death / hit | NPC-record sound multimap keys `0x1e` / `0x1f` (`npc_sound`), sound ids = `CreateSoundID` hashes |
| `SM_Sandy_Game_Explo_Big/Med/small` | explosions (`FUN_1009d182`, grenade) | `sfx/weapons/explotions/*` (defined; no caller in the combat code, grenade effect path unresolved) |

**Wired** (`combat/glue.rs` -> `Dynels::char_sound` / `sound_at` -> `GameSound` queue -> `Audio::play_game_sound(id, pos, camera)`, the same path as the door sounds): death on `CharacterAction` 99
(`CombatEvent::Died` cause 0), hit on `0xd1` (`Module::take_struck`), Brawl / Dimach from `CombatEvent::SpecialAttack` through `special_swing(stat).sound`. A creature (look with NPC record) plays a
random value of `NpcRecord::sounds` key 0x1e / 0x1f (`FUN_1004570c`: `rand() % count`, no RNG call for one value; the multimap is decoded, docs/zone/npc.md §4); other characters the
Male / Female name by the look's sex (3 = female). **[INFERENCE]** `FUN_10051f6e() != 0` is read as "has an NPC record" (look.npc). The own character plays at the camera (the avatar is not a dynel model here).

Weapon firing / impact sounds of real weapons are **not** played by this combat code: they belong to effect scripts (`_EffectHandler_t::CreateEffect2(effectId)`, weapon stats
`EffectType` 413 / `ImpactEffectType` 414 read in `FUN_1007ac9a`) - **[UNRESOLVED]**, see §8. The five weapon-class names (`FlameThrowerFire`, `PistolSingle/MultiShot*`) have no constants in the code
(not defined in either .sbf, nothing to play).
**Correction to docs/formats.md ("168 `sfx/player/*.txt` unused")**: the files (`<breed>_<sex>_<cool|distunguished|military|simple>_<heal|help|inc|no|run|yes>_NN.wav` + subtitle `.txt`) are the
**chat voice commands** (GUI.dll strings `sound/sfx/player/`, `VoiceSndFxType`, `VoiceSndFxHear{Team,Guild,Vicinity}On`, `Voicecommands.html`), not per-animation FX; no combat code uses them.

## 7. Corpses
`CorpseFullUpdateIIR_t` (0x4F474E05, header kind **0xC76A**, decoder `ao_net::n3::world::Corpse`, reader `FUN_1009f502`, ctor `FUN_1009f7b0`, vtable 0x10166a5c): a **separate dynel**
(`Corpse_t` : `Chest_t` : `SimpleItem_t` family [GC 0x101622d4, docs/zone/static.md §1]; its mesh / cloth / look resolution is `ao_formats::dynel_visual`, docs/zone/static.md; name `Remains of <owner name>`), created with stats `Flags`(0) 0x181805, `CATMesh`(42) = the model, `MonsterScale`(360), `Sex`, `Breed`, `Cash`(61) (loot money),
**`DeadTimer`(34) = 600**, **`TimeExist`(8) = 18000 / 180000**, `CorpseType`(415) = 50000, `CorpseInstance`(416) = owner id, `MultipleCount`(412) = 1; 5 cloth slots, no textures
[DATA: 7 corpses]. Position/rotation = the dead char's last position (live: identical to its `FollowTarget` position). The corpse has no animation key in the capture (`CorpseAnimKey` 417
absent; code `FUN_100a4dcc` reads it only for the spell-effect "play animation on item" path, `FUN_10010e36(item, key < 100, 1)`), so the corpse plays **no clip**: it is the unanimated
CAT mesh (bind pose), evidence in docs/zone/static.md §5 (not the owner's death clip held on its last frame).
* Looting: `GenericCmd_t` (state 1, cmd 3, `Item{actor = own, item = {0xC76A, id}}`) 1.1 s after the corpse appeared (capture, 44741 ms vs 43632 ms) is the loot request; **[INFERENCE]** it opens the corpse inventory
  (`CORPSE_INVENTORY`, `N3Msg_SetLootAccess`, `Feedback_NotAllowedToLoot`, team loot strings); `BankCorpseIIR_t`, `ReclaimBooth_t` (the "Reclaim" window) are the player-corpse side.
* Despawn: server driven (`n3ToClientQuitIIR_t` for the corpse; live corpse 5163 left 40 s after it arrived while `TimeExist` = 180000). The units of `DeadTimer` / `TimeExist`
  are **[UNRESOLVED]** (no code reads stat 34 / 8 by number; `DeadTimer` appears only in the stat table).
* **Implementation** (`Dynels::on_message`, `ao_formats::dynel_visual::corpse_visual`): the corpse dynel is a prop keyed `{0xC76A, instance}`, drawn unanimated. Nothing else of the update is
  read by any client code found (no reader of `DeadTimer` / `TimeExist` / `CorpseType` / `CorpseInstance` by number, owner link unused), so the combat layer keeps **no** corpse record: the former `CorpseInfo` /
  `corpse_stat` / `CORPSE_KIND` were deleted (no consumer). The dying NPC is removed by its own `n3ToClientQuit` (2.92-2.99 s after action 99, capture `zone_fight_ithaca.rec`), the corpse
  appears 1 ms later as an independent dynel; nothing hides or links the owner's dynel.
* Regression tests: `dynels::variant_tests::replayed_kill_plays_the_death_clip` (action 99 -> `Special::Die(503)` holds the clip, record sound at its position, quit removes the char),
  `combat::module::death_tests::{captured_kill_stores_the_death_animation, own_death_holds_the_animation_and_reports_after_three_seconds}`. The selection leaves with the dynel
  (`TargetingModule::update` clears `Zone::target` once `Zone::dynels` lost it; `Zone` drops it on `ToClientQuit`). The "dead leet stayed standing" sighting could not be reproduced from the capture:
  the quit arrives after 2.92-2.99 s and the death clip is held until then.

## 8. Not found / open
* The `imp-*` hit-reaction selector (section 4); the bare-hand attack list (3.1); `ToClientDynelDead` caller; action 0x98 server-side meaning; stat 0x183 name.
* Weapon firing/impact FX + their sounds (effect scripts); `PlaySoundIIR_c` (0x455D2938), `GfxTriggerIIR_t` (0x7A222202) and `HealthDamageIIR_t` (0x3710256C) are registered
  message ids that never occur in the capture - server-driven sounds/effects may arrive through them.
* Weapon sounds: `SM_Sandy_Game_*` definitions under `sfx/weapons/{guns 169, impacts 120, swish 42, explotions 32, lost_eden/*, missile 8, swords/*}` exist in the .sbf, but nothing in the
  fight code reads a weapon -> sound id; the selector is the effect scripts of `EffectType` 413 / `ImpactEffectType` 414. Scanning every rdb record type for the effect ids `0x2f5a` / `0x2ced`
  finds only records of types 1000046 / 1010001 with that number (1010001 = meshes; not effect scripts **[INFERENCE]**), `twk/` has no effect table, so the id -> script -> sound chain is
  unresolved and the client port plays no weapon/impact sound yet.
* `FUN_1005d0d8` case 0x5b also calls `vtable+0x40` of the stat system and, for a non-control char, `FUN_100523c3` (purpose not read); `FUN_10012a1e(soundId, pos)` runs just before `PlayGameSound` (not traced).
* The own character's `Dying` default animation when no action 99 arrived (death computed by `FUN_1005ae91`): 503 is a **guess** (`DEFAULT_DEATH_ANIM`).
