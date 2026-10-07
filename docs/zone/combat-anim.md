# Combat animations, fight sounds, death and corpses

Code: `crates/aomac/src/play/combat/anim.rs` (pure lookups + state; tests `cargo test --release -p aomac combat::anim`, real-data
tests skip without the client). Labels: **[CODE]** read in the Ghidra decompile of `Gamecode.dll` ("GC"), **[DATA]** decoded from
`rdb.db` / `cd_image/sound` / captures (`docs/captures/zone_ithaca.rec`), **[INFERENCE]**, **[UNRESOLVED]**.

## 1. Pipeline: AbstractAnimID -> clip

1. The game code never names clips. It asks for an **`AbstractAnimID_e`** (a small int) and the animation holder
   (`AnimHolder_t`, `SimpleChar+0x1dc`; ctor `FUN_1003c525` [GC 0x1003c525], vtable 0x1015d580: `[1]` idle update / fight start `FUN_1003cad0` (lists 0x10, 0x1a),
   `[2]` fight stop `FUN_1003cc15` (list 0x1b), `[3]` wield pair `FUN_1003c930`, `[4]` `FUN_1003c9b2` (never called), `[5]` **Play** `FUN_1003ca30`, `[6]` `FUN_1003c392`; section 4)
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
  of the **weapon's list 0xb**. For a queued special attack it first calls `FUN_1003c594` (section 3.2). If that list is absent,
  `FUN_10069acb` writes `0xb` back through its list-key output; `FUN_1006a239` passes this resolved key to both `FUN_1003bfd2` and `Play`.
* The id then goes through the holder (section 1). Holder `Play(clip, 1.0f, -1, 0, listKey, 1, 0, 0, slot)`; list keys already
  playing (`FUN_1003bfd2`) are not restarted. If no clip is found and `char+0x21c == 0`: `FUN_10010d83(char, 0x3e, 1)` (the social
  `wave`!) [CODE, taken literally].
* **Speed** (`swing_speed_scale`): if `char+0x21c == 0` and the mesh exists: `SetSpeedScale(clip, clamp(noteTime_ms / (ItemDelay*10), 1, 2))`
  (`ItemDelay` = weapon stat 0x126 = 294, `_DAT_101600f0` = 10.0 double, cap `_DAT_10158794` = 2.0f): the swing is only ever sped up so
  its first note lands within the weapon delay. **[CODE, DisplaySystem.dll]** `VisualCATMesh_t::GetNoteTime(clip, 0)` [DS 0x10073386] returns
  entry 0 of the clip's `AnimMetadata` (stride 0x28, float at +0x24; 0 when the clip has none); `SetSpeedScale` [DS 0x100737c2] stores the scale
  at `AnimStruct+0x20`. The metadata entries are the CAT clip's named events (`CatAnim::events`, `(time ms, name)`), so entry 0 = the clip's first event.

### 3.0 What the client runs (`combat/glue.rs::swing`, `Dynels::pick_swing`)
Every `Hit` / `Miss` / `SpecialAttack` event of any character (a miss runs `FUN_1006a8f3` with damage 0, section 4):
1. **Weapon swing**: the holder's wielded weapon (`Dynels::wield`, the `WeaponItemFullUpdate` children in body slot 6 = right hand, 8 = left;
   `AnimSet` 0x161 and `ItemDelay` 0x126 from the item template under the message stats) -> `weapon_list(AnimSet, left, crawl = false, key)`
   (`key` = `0xb`, or the special's list key, else `0xb` when the weapon has no such key) -> random value (the CRT-rand stream of `Dynels`)
   -> `(AbstractAnimID, ItemDelay, resolved list key)` -> the own avatar plays `Role::Clip(anim_name(id))` (`Player::swing_list`,
   rate x `swing_speed_scale(first event time, ItemDelay)`), every other character `Dynels::play_swing(id,anim,resolved_key)` (NPC record table
   or the set's file name through `resolve_clip`). Both holder gates use the resolved key, including `0xb` after a missing special list;
   remote `ActionAnim::advance` applies the same first-event speed scale. [DATA] all weapon attack ids exist as clips for male / female / athrox (test `fight_clips_exist_in_the_data`).
2. **Item lists**: Brawl, Dimach and bow special resolve the list on the special item's record (`FUN_100686d0(stat)+0xe4`).
   Bare hands resolve the martial-arts item delivered under key 100. `dynel_visual::animation_map` decodes element `{0xe,0x13}` using the same validated multimap size words as the sound parser.
   The martial-arts fixture (rdb 1000020:43712) asserts list `0xb = [1034,1035,1037,1033]`; Dimach 42033 has `0xb=[163]`, Brawl 70292 has `0xb=[1036]` (element offsets 203 and 184). Special keys missing on their own item fall back to its `0xb` (`FUN_1003c594`). Missing lists retain the creature-path `0x40a` fallback;
   this missing-player-list fallback remains **[UNRESOLVED]**, not a fabricated special clip. The crawl lists (`crawl = true`) are not used: stat 0x1ae is not tracked per dynel.
3. **Text above the character** for special attacks (`FUN_10011108(char, "Burst!\n", 0xd)`): `Module::on_frame` pushes a world-space `Number` (category 0xd = colour
   `0x00f000`, effect `0x2f5a` path, `combat-log.md` section 6) with the text without its trailing newline. The sound of Brawl / Dimach is in section 6.
* `CharSecSpecAttack` queues only when empty; `SpecialAttackInfo` starts the front's swing before feedback and clears the deque afterward (`FUN_1006a9c5`, `FUN_1005548b`). Active list keys are suppressed (`FUN_1003bfd2`, its list node `+0xc` comparison), not clip names:
  distinct special keys resolving to the same clip restart the own avatar clock/notes and replace a busy dynel swing. Social and reaction `play_once` calls retain their busy gate.
* Same-frame event order is preserved by `combat_animations`: `CharFight_t` constructor
  (`1007b816`, §4) draw precedes the later `SpecialAttackInfo` swing (`1006a9c5`).
  The former split loops played all swings first and then all stance transitions,
  replacing a start+special rifle clip with `rifle-start`. Regression
  `fight_start_does_not_replace_same_frame_rifle_special` checks the actual own
  `Player` transient remains Burst 1024 / Fling 1023. Added, not run here.
  `AOMAC_COMBAT_LOG` records selected list/abstract id/name and the sampled own
  clip's source id, duration, rate and authored events.
* Pre-fix capture `artifact://21482` received own33512 normal slot6 hit (damage3,
  flags3) and note0xb, Burst148 slot6 damage29, Fling150 slot6 damage6.
  No pre-spawn effect diagnostic appeared. Root found in glue: these four flags
  belong to `IndependentPrefs` (`dvalue/indep.rs`, login defaults all 1), not
  DValue nodes. `DValues::flag` only reads nodes, so HUD-backed category masks
  were always zero. Glue now reads `prefs.get_int(..., Kind::Login)` (DS
  `1005fd60`; GC `100ce1fa` packs nano bits4/32); default weapon/nano masks
  are10/36. Regression `effect_categories_read_independent_login_preferences`
  checks defaults and each individual preference off. Temporary early-return
  instrumentation removed; authored spawn diagnostics remain. Added, not run here.
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
43712, 43713), many single-entry `{1034}` / `{1033}` records (45605, 56180, 100237, 120637...). The item-record animation multimap is now decoded as `{0xe,0x13}`.
Unarmed fists use the `DummyWeapon_t` martial-arts item delivered by `SpecialAttackWeaponIIR` under key 100 (docs/zone/combat-log.md §2.1.1), not a client-made item.
The char's `UnarmedTemplateInstance` (stat 418) is 0 for the own char in the capture.

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
(Brawl, Dimach and Bow special look the key up on the special attack's own item `FUN_100686d0(stat)+0xe4`. A fresh decompile of `FUN_1003c594` corrects the earlier Backstab claim: its `0x89` branch does **not** set the own-item flag, so it uses the wielded weapon.)

## 4. Idle / fight stance, hit, miss
* **AnimHolder stance entry points** (traced with Ghidra xrefs over Gamecode; `FUN_1003c930` / `FUN_1003c9b2` are only reachable through the vtable
  0x1015d580 slots `[3]` / `[4]`, and a scan of every `CALL [reg+0xc]` / `[reg+0x10]` finds **one** caller function of `[3]` (`FUN_1009c858`, three call sites) and **none** of `[4]`
  (`FUN_1003c9b2` is dead code); the lists 0x1a / 0x1b belong to the *other* two slots, which is what draws and holsters):
  | slot | function | list | called by |
  |---|---|---|---|
  | `[1]` idle update | `FUN_1003cad0` | 0x10 idle, **0x1a start** | `CharFight_t` ctor `FUN_1007b816` (a character enters fight state 10, unconditionally), `FUN_1006a700` / `FUN_1006a772` (a weapon is wielded / unwielded while `WeaponHolder+0x44 != 1`, i.e. fighting) |
  | `[2]` | `FUN_1003cc15` | **0x1b stop** | `FUN_10068b7f` (stop fight, state 2 -> 1, when `char+0x80 == 0` and `FUN_10059ac4()` is false) |
  | `[3]` | `FUN_1003c930(first, second)` | 0x27 + 0x29, or 0x28 + 0x78 | `FUN_1009c858` = `WeaponItem_t` attach (`param_3 == 0`: only when `char+0x1d4 +0x44 == 1` (not fighting), char state not 4) and detach (always, char state not 4) |
  `+0x44` of the fight controller is 1 = not fighting, 2 = fighting (`FUN_10069c68` sets 2, `FUN_10068b7f` 1). `FUN_1009c858` is run by `WeaponItem_t` vtable `+0xa4` =
  `FUN_1009e301` (wield: `FUN_10047873` on a `WeaponItemFullUpdate` for slot 6 / 8 / 0x3d / 0x3f with stat 0x37 in {1, 0xe}; `FUN_1006ad94` = `CharacterAction` 0x83)
  and `+0xa8` = `FUN_1009ce50` (unwield: `FUN_1006a857` = action 0x61, and the first step of a second wield) and by the `AppearanceUpdate` apply `FUN_10071679` for the
  holster slots 0x3d / 0x3f.
* **Fight start** (`CharFight_t` ctor -> `[1]`): if the weapon's list 0x10 clip is not already the AnimHolder's current idle (`+0x34`; the bazooka's is), the clip of list **0x1a**
  (`idle-stand` 0x78 when absent) plays **once** (`FUN_1003bc11(clip, 1, 1, 0)` + `FUN_1003c216` = `SetAnimation` count 1, layer 0) and the idle (list 0x10, else 0x78) is started
  with `VisualCATMesh_t::SetAnimationDelay(handle, GetDuration(draw clip))`, so it takes over when the draw ends. The weapon is the first present of body slots 6, 8, 0. A target switch
  (`FUN_10069c68` while fighting) calls `FUN_10068b7f` first and then sets state 2 again without a new ctor: **holster clip, no new draw** (the idle that follows is the
  equip routine's, [INFERENCE] from the code order; unverified live).
* **Fight stop** (`FUN_10068b7f` -> `[2]`): list **0x1b** of the weapon plays once, then the AnimHolder idle (`+0x10`: the equip routine's idle, `+0x18` while crawling) starts delayed by its duration.
* **Idle out of a fight = `FUN_1009c858`** (weapon attach, `param_3 == 0`): for **AnimSet 3 only** the AnimHolder run (`+4`) = 0x422 and walk (`+0xc`) = 0x421 (constants, **lists 0x2a / 0x2b are
  never read**: a scan of every `FUN_1004570c(imm)` caller finds keys 0x10, 0x1a, 0x1b, 0x27, 0x28, 0x29 only), and for AnimSets 3 and 8 the idle (`FUN_1003cd3d`, `+0x10` / `+0x34`) = list **0x29**
  (rifle idle-2h 0x41e, bazooka 0x424); every other weapon keeps `idle-stand` 0x78. The detach restores run 0x65, walk 100, idle 0x78. So a pistol / blade / bow wielder out of a fight stands
  in plain `idle-stand`; the weapon's list-0x10 idle is a **fight** idle (`combat::anim::{peace_idle, fight_idle, wield_walk_run}`; the earlier port used list 0x10 for every idle and lists
  0x2a / 0x2b for walk / run of the bazooka as well: a deviation, fixed).
* **Wear / unwear while not fighting** (`[3]`, `FUN_1003c930(first, second)`): `first` = `c8b7` of the weapon's list 0x27 (rifle 0x41f, bow 0xb5), `second` = list 0x29 (rifle 0x41e, bow 0xb6, bazooka
  0x424); without list 0x29 `(list 0x28, 0x78)`, which for a pistol / blade is `(none, idle-stand)`. `first` plays once (count 1, layer 0) and `second` is started **in the same call** as a loop on the
  idle's own layer (also 0) with no delay. `VisualCATMesh_t::SetAnimation` [DS 0x10073f23] keeps one active animation per layer (`CATAnimBlend_t` anim1 / anim2, new clip fades in over 200..300 ms,
  floats at DS 0x100af89c / 0x100af8a0), so the draw clip of `[3]` is replaced within the frame; **[INFERENCE]** it is never seen (the delayed idle of `[1]` exists precisely to show the draw), unverified
  live. The unwear's `[3]` (list 0x28 `rifle-stop-2h` 0x420 then 0x78) is replaced the same way. Nothing is played for them.
* **Wield gesture 0x6d**: `FUN_1006a857` (action 0x61) calls `FUN_10081e74(char, 3)` after the unwield (the character's own anim list 3, else 0x6d = the first `wield` entry, `<set>_wield_01_01.ani`;
  `Play(id, 1.0, count 1, layer 0)`); a fight then runs `[1]` through `FUN_1006a772`, which replaces it in the same frame. Out of a fight the gesture is the only thing the unwear shows
  (`combat/module.rs`: `Module::take_anims`, queued only for an occupied hand slot of a character that is not fighting). `FUN_1006ad94` (action 0x83, the same wear / unwear moment,
  `identity_b` = the slot) plays it as well, but the wield that follows (`FUN_1009e301`: unwield if wielded, then attach) overwrites it on a wear; on an unwear (`identity_b` = bag slot 0x41) the attach
  returns early for a slot above 0x2f unless bit 5 of stat 0x2a1 is set, so [INFERENCE] the gesture of the 0x83 stays (a restart of the same clip within the same millisecond).
  **Wired**: `Module::on_frame` (0x61 -> gesture 0x6d), `glue.rs::stance` (`FightStarted` / `FightStopped` -> `draw_clip` / `holster_clip` + the fight idle flag: `Dynels::set_fighting`, `Player::fighting`;
  a weapon that resolves during a fight runs the draw again: `Dynels::take_wielded`), `combat::anim::{peace_idle, fight_idle, wield_walk_run, draw_clip, holster_clip}`, `Avatar::set_stance`
  (own: idle / fight idle / walk / run), `Dynels::update_with_collision` (others, same). Not wired: the martial-arts item's lists 0x1a / 0x1b / 0x10 (bare hands; record layout not decoded, §3.1), the crawl
  variants, char state 4 / 8 / 9 gates and `char+0x80` / `FUN_10059ac4` (assumed 0 / false for a live character).
  Stance ids per weapon type: section 3.1 (`blade-start/idle-blade/blade-stop` 1000-1002, `smallarms` 1010-1012, `rifle` 1020-1022,
  `unarmed` 1030-1032, `2h` 1055/1054/1056, `bow` 0xb5-0xb8).
* **Being hit** (`CharacterActionIIR_t` action **0xd1**, `FUN_1005d0d8` case 0x5b [GC 0x1005eada]): if `identity_a.instance` and `identity_b.instance` (the handler's two `Identity*` arguments; `param` is not read) are both `> 0`, play the hit sound at the
  victim's position (section 6).
* **Hit reaction** [CODE] (`FUN_1009b4ac` [GC 0x1009b4ac], run by the swing's `attack` note, section 6.1): hit kind (`AttackInfo::unk_30`) > 1, the victim's holder is not already playing a list-`0x1f` clip and
  the victim's movement mode is not 4 / 8: `Play(GetImpactAnim(victim), rate)`. `GetImpactAnim` is the `SimpleChar` vtable slot `+0x90` = `FUN_10058cfa` [GC 0x10058cfa] (vtable 0x1015fa94, the ctor
  `FUN_1005cb6a` stores it): stat `0x1ae == 0xe` (crawling) -> `0xd0`, else `rand() % 5` of `{0x81, 0x82, 0x80, 0x84, 0x7f}` (the `imp-*` clips; creatures resolve them through their record's
  parent chain `0x80..0x84 -> 0x7f`, npc.md section 3). Rate `1.0` when the hit kind is 4 (crit), else `_DAT_1015d0a4` = 0.5. The same `GetImpactAnim` id also picks the impact effect location (`0x3e9 / 0x3ef /
  0x3f0 / 1000 / 0x3ee`, `FUN_1009ad7d`). **Wired**: `Dynels::impact_anim`, `Dynels::react_to_hit` (others), `Player::react` (own avatar, `Avatar::set_clip_scale`), from `combat/glue.rs`. The crawl case is not
  tracked (stat 0x1ae); "already playing" is "the character is busy with another one-shot clip" (`play_once` / `Player::react` ignore the call); the movement mode 4 / 8 test is only done for the own character.
* **Miss** (`MissedAttackInfoIIR_t`, apply `FUN_1006ae50`): the combat-log text **and the same swing as a hit**: `FUN_1006ae50` ends with `FUN_1006a8f3(slot, 0, value_1c, 0, 1, 0)` (damage 0, hit kind 1; a
  zero amount prints no log line, `FUN_10012bd5` returns on it), i.e. `FUN_1006a239` starts the swing clip and its notes fire; with hit kind 1 `FUN_1009b4ac` stops before the reaction and the impact
  sound, so a miss only plays the swing sounds of section 6.1 (**fixed**: the old note here said "no animation or sound"; `combat/notes.rs::hit_of`, `glue.rs` swings on `CombatEvent::Miss` too).
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

### 6.1 Attack sounds: swing / shot, swish, impact (resolved)
**The sounds are started by the animation notes of the swing clip, not by the `AttackInfo`.** [CODE] `FUN_1006a239` only `Play`s the clip with the attack slot (the 9th argument); every frame the
holder (`FUN_1003c036` [GC 0x1003c036]) walks its playing clips and, for each metadata entry (stride 0x28: name, id `+0x20`, time `+0x24`; = `CatAnim::events`, `(ms, name)`) whose time was reached
(`lo <= t <= hi` of the frame interval, once per play: bit mask `entry+0x34`), calls `FUN_1003bccb` -> `FUN_10045069` [GC 0x10045069] (`param_2` = note id, `param_3` = the clip's slot, `-1` for clips not
started by a swing). The note id comes from the event **name** by DisplaySystem `FUN_10075c84` [DS 0x10075c84] (`strncmp` prefixes in this order; `combat/notes.rs::note_id`): `attack*` 0xb (after
`attack_start_1..9` 0x77..0x7f and `attack_effect_1..4` 0x8e..0x91), `swish_punch/kick/whip/huge` 0x73..0x76, `step`/`stepfast`/`left`/`right` 0x26, `land` 0x85, `idle*` 0xf, `enter_combat` 0x80, ... (the data's
`swich_kick` typo, 15 clips, matches nothing). [DATA] 53 of the 57 swing clips of the male / female / athrox sets (weapon `attack` lists + bare-hand 1033..1037) have an `attack` event (never at t = 0), 15 a `swish_*`.

`FUN_10045069` for a swing: notes **0x73..0x76** play the holder's own ids `SM_Sandy_Swish_punch / kick / tail / huge` (`AnimHolder_t` ctor `FUN_10044702` members `+0x10..+0x1c`) when the character has no NPC
record, else a value of the record's list `<note>`; **0x77..0x7f** the record's list `<note>` (creatures; nothing for players); both at the character, plain `PlayGameSound`. Notes **0xb and 0x8e..0x91** (slot >= 0)
call `FUN_100688f9` [GC 0x100688f9] (`this` = the fight controller `char+0x1d4`): slot object `FUN_10068072(slot)` (`+0x14` the wielded `WeaponItem_t`, `+0x10` the `DummyWeapon_t` of the martial-arts / creature
attack item, section 3.1), target `FUN_100676e0` (the fight target), then `FUN_1009b84b(key = note id, attacker, victim)`:
1. `FUN_1009cd68` (wielded weapon only): the item's sound list `key` at the attacker (`PlayGameSound(id, pos, 0, 1.0, 0, 0, 100, 1)`). (Its `0x2c` "empty" branch needs item stat 0x1a == 0; `FUN_1006a8f3` has stored
   `value_20` = -1 there first, so it is not reachable with the live data and is not modelled.)
2. `FUN_1009b4ac` (item or dummy): the same list `key`, else `0xb`, at the attacker - **only when the hit kind > 1**; then, for damage > 0, the victim part below.
3. `FUN_1009cc50` (`WeaponItem_t` vtable `+0x94`, wielded only): list `key` once more, and for `0xb` without a record sound the default of the item's `AmmoType` (stat 420; -1 -> `0x0148e160`, 1 `0x7634c942`, 2 `0x2d2cb134`,
   4 `0x7a77a8dd`, 5 `0xdf8167d6`, 6 `0x4c790a5d`, 10 `0xba94da9b`, others none: `DAT_102e339c` is never written).
Identical ids of the three calls start once (a sound that is still playing is not restarted, docs/formats.md `PlaySample` gating). Bare hands play **no weapon sound for a miss or a hit kind 1**; only the swish note. [DATA] The bare-hand clips: male `unarmed-rswing` (1034) `swish_punch` @33 ms, `attack` @266 ms; 1035 swish @133 / attack @233; 1036 (double punch) swish 33, attack 200, swish 400, attack 533 (**two** `attack` notes = two weapon-sound calls); 1037 133 / 233; kick 1033 `swish_kick` 533, attack 866 (female / athrox: other times, same names).

**Victim part of `FUN_1009b4ac`** (damage > 0, hit kind > 1): size class `damage >= 60/10 -> 2`, `>= 60/20 -> 1`, else 0 (`SimpleChar+0x218`, only the ctor writes it: 60). Victim with an NPC record: material =
record stat 41 `FabricType` (`FUN_1004d8e6(0x29)`), creature sound = record list `0x1f`; if the material is 1..=17 the **weapon's list `0x1f`** is played at the victim (material, size). Material 0 (players, and
the 1341 of 1360 creature records without `FabricType`): Breed (stat 4) 1, 2, 3, 4 or 7 -> material 7 and `SM_Sandy_Game_MaleGetsHit` (Sex 1 / 2) / `FemaleGetsHit` (Sex 3). The record / player sound plays when the
material is 1..=17: `PlayGameSound(id, victim pos, 0, 1.0, material, _DAT_101663d4 = 0.4 (a delay), 100, size)`: **0.4 s later**, with the material argument, see docs/formats.md `## audio` "material variants".
So a hit on a player (the common case: creatures hitting the own character, own hits on other players) plays `Male/FemaleGetsHit`'s flesh variant 0.4 s after the swing sound.

**Item sound multimaps** [DATA]: rdb 1000020 element `{0x14, sub}` (reader `FUN_1007d6d5`, the NPC record's format): `ao_formats::dynel_visual::sound_map` (the elements before it - spell / skill / event lists - are skipped by
finding the header, no record has two candidates). 16 049 of the 17 298 `WeaponItem` records have one (31 843 ids, 75 not in the sbf): keys `0xb` swing / shot (2 795 ids, 63 distinct sounds), `0x1f` impact (4 059, 32 sounds,
mostly `0x113cfb49`: no file, only material variants), `0x73` / `0x74`, and keys 0x15 0x16 0x17 0x1c 0x1d 0x50 0x87 whose consumer was not found (no reader in the weapon code; **[UNRESOLVED]**, they would be the special-attack
notes `aimedshot/burst/fullauto/flingshot/sneakattack`, which `FUN_10045069` ignores: ids < 0x25 except 0xb/0xf return). Wield / unwield: `FUN_1009e301` (vtable `+0xa4`) plays list `8`, `FUN_1009ce50` list `9` at the wielder;
`FUN_1009d182` (grenade) list `0x31`; found, **not wired** (the callers' flag arguments were not traced; the wielded-at-login case would play them for every wielder). The `WeaponItem_t` ctor's members
`+0x1d0..+0x1ec` (`FlameThrowerFire`, `Female/MaleGetsHit`, `PistolSingleShotHitFlesh/Ground`, `PistolMultiShotFire`, `PistolSingleShotFire`, `Explo_Big`) have **no reader** in the weapon code (the only other accesses
to those offsets are `SimpleChar` fields): dead data of an earlier design, which is why most names are not in the .sbf.

**Wired** (`combat/notes.rs`, `Dynels::{note_sounds, weapon_hit, swish, record_note}`): the avatar (own, `Avatar::take_notes` while `Player::swing` marks the clip as a swing, bare hands included) and the dynels
(`Dynels::swing_mark` + the clip time in `update_with_collision`) fire notes once per play; `glue.rs` hands them to `note_sounds`, `note_reaction` and `note_effects`. `Dynels::hit_seen` retains damage/hit kind per (attacker, slot); `special_hit_seen` changes only the active swing target/slot. The note resolves the current fight target (or the controller's retained last target), then reads that slot's values (`FUN_100688f9` → `FUN_1009b84b` → `FUN_1009b7e8` → `FUN_1009b4ac`). Reactions now start at qualifying attack notes, never at `Hit` message arrival. Specials after a hit retain reaction rate, impact effects and old-damage sound size; after a miss or on constructor-zero slots there is no impact. Wielded weapon swing sounds remain ungated. Own-item special animation selection remains separate from slot-item note sounds/effects (resolved contradiction: combat-log.md §2.2).
Sounds go through the `GameSound` queue (`material`, `size`, delayed ones through `Dynels::update_with_collision`) to `Audio::play_game_sound_with` (flow.rs logs `game sound <id> at <pos>: N voice(s) (material m, size s)` with `AOMAC_AUDIO_LOG=1`). Positions: the character; the own character at the camera.
Tests: `notes::tests::*` (note ids, once-per-play firing, size / ammo / player-impact rules, real swing clips), `dynels::variant_tests::{a_bare_handed_hit_plays_the_weapon_swing_and_the_material_impact,
swish_and_attack_start_notes, a_marked_swing_clip_reports_its_notes, creature_records_carry_a_fabric_type, a_struck_creature_plays_an_impact_clip, special_notes_retain_only_their_attacker_and_slot_flags, special_sound_impact_uses_retained_damage_not_the_result}`, `arms::tests::real_records`, ao-formats
`a_weapon_record_finds_its_sound_multimap_behind_the_unwalked_elements`, ao-audio `game_material_maps_to_a_variant_slot` and `weapon_sounds_resolve` (all 31 843 ids but 75 are in the sbf; the flesh variant plays).
Notes fire from the swing clip clock, not at `AttackInfo` arrival; impact sounds/effects and reactions resolve the active fight target at note time. Unknown victims/items (no slot object, `FUN_10068072` null) are silent as in the original; creature impacts need the NPC model to be built. The visual `EffectType` 413 / `ImpactEffectType` 414 scripts (`FUN_1009ad7d`) use the same retained slot hit-kind gate, independently of special-result damage.
**Correction to docs/formats.md ("168 `sfx/player/*.txt` unused")**: the files (`<breed>_<sex>_<cool|distunguished|military|simple>_<heal|help|inc|no|run|yes>_NN.wav` + subtitle `.txt`) are the
**chat voice commands** (GUI.dll strings `sound/sfx/player/`, `VoiceSndFxType`, `VoiceSndFxHear{Team,Guild,Vicinity}On`, `Voicecommands.html`), not per-animation FX; no combat code uses them.

## 7. Corpses
`CorpseFullUpdateIIR_t` (0x4F474E05, header kind **0xC76A**, decoder `ao_net::n3::world::Corpse`, reader `FUN_1009f502`, ctor `FUN_1009f7b0`, vtable 0x10166a5c): a **separate dynel**
(`Corpse_t` : `Chest_t` : `SimpleItem_t` family [GC 0x101622d4, docs/zone/static.md §1]; its mesh / cloth / look resolution is `ao_formats::dynel_visual`, docs/zone/static.md; name `Remains of <owner name>`), created with stats `Flags`(0) 0x181805, `CATMesh`(42) = the model, `MonsterScale`(360), `Sex`, `Breed`, `Cash`(61) (loot money),
**`DeadTimer`(34) = 600**, **`TimeExist`(8) = 18000 / 180000**, `CorpseType`(415) = 50000, `CorpseInstance`(416) = owner id, `MultipleCount`(412) = 1; 5 cloth slots, no textures
[DATA: 7 corpses]. Position/rotation = the dead char's last position. The play-animation spell supplies key 503 and its NPC record:
the four standard arguments had previously been mistaken for type arguments, hiding the non-social branch of `FUN_100a4dcc`.
The corpse holds the resulting death pose, rather than bind pose; corrected field mapping and addresses are in docs/zone/static.md §5.
* Looting: `GenericCmd_t` (state 1, cmd 3, `Item{actor = own, item = {0xC76A, id}}`) 1.1 s after the corpse appeared (capture, 44741 ms vs 43632 ms) is the loot request; **[INFERENCE]** it opens the corpse inventory
  (`CORPSE_INVENTORY`, `N3Msg_SetLootAccess`, `Feedback_NotAllowedToLoot`, team loot strings); `BankCorpseIIR_t`, `ReclaimBooth_t` (the "Reclaim" window) are the player-corpse side.
* Despawn: server driven (`n3ToClientQuitIIR_t` for the corpse; live corpse 5163 left 40 s after it arrived while `TimeExist` = 180000). The units of `DeadTimer` / `TimeExist`
  are **[UNRESOLVED]** (no code reads stat 34 / 8 by number; `DeadTimer` appears only in the stat table).
* **Implementation** (`Dynels::on_message`, `ao_formats::dynel_visual::corpse_visual`): the corpse is an independent prop keyed `{0xC76A, instance}`.
  Its NPC-record death clip is resolved from the spell and its terminal pose held. The character's death freezes movement at its death position;
  health-zero stat updates cannot replace an already selected death clip. Corpse creation removes its owner actor even if it precedes the owner's quit.
  The corpse remains until its own server despawn; loot updates do not remove it.
* **Full kill evidence** (`docs/captures/zone_kill_ithaca.rec`, lines 382–397): character `0xf7f82` gets action99/key503 at 216107 ms;
  at 219058 ms the character quit precedes corpse `{0xc76a,0xcf3}`, then a late StopFight still targets the removed character.
  The corpse spell's standard fields are `[1,0,0,0]`, type fields `[0,0,503,1,4,17655,0]`.
  At 246428 ms the server sends its inventory, confirming the distinct corpse identity is the loot target.
* Regression: `captured_kill_replaces_character_with_persistent_corpse` replays every received capture frame, asserting selected death clears,
  death movement freezes, owner removal, correct spell alignment/position, and persistence through the late fight/loot frames.
  `corpse_holds_death_pose` compares the held vertices to the actual NPC-record terminal animation pose (real-data test).
* **Health boundary [CODE]**: computed combat health/death-cause (`FUN_1005ae91`) is not itself the `CharDie` flag transition.
  The renderer's death state responds to server action99 or explicit health-zero stat/full updates; no extra computed-hit-to-animation bridge is invented.

### 7.1 Effect geometry anchors

* Numeric dispatch is **Gamecode `FUN_10105917`**, not the similarly named DisplaySystem API.
  Table `0x102c53a8` maps 1000–1018 to `Bip01 Pelvis_ac`, Spine, Spine1, Spine2, Spine3,
  Neck, Head, L/R UpperArm, L/R Forearm, L/R Thigh, L/R Calf, L/R Foot, L/R Hand
  (all names carry `_ac`). In particular 1000–1007 end at **L UpperArm**, not a generic torso.
  Table `0x102c53f8` maps 2000/2001 to `Attractor02_righthand`/`Attractor03_lefthand`,
  2002 to head, 2003 back, 2004 left shoulder, 2005 right shoulder; the remaining entries
  are named special/attack/destruction/flare/smoke/sparks/flames/beam attractors.
  Both valid numeric ranges explicitly retry authored `Attractor01_head` if the requested geometry
  is absent. Unknown IDs return no anchor; no guessed position is introduced.
* 3000 is **weapon-object muzzle world geometry**: `FUN_10105917` casts to WeaponItem and
  calls `FUN_1009bded`. That follows the parent character, selects hand place 1 (slot 6)
  or 2 (slot 8), finds the stat-209 WeaponMesh child, and asks `VisualMesh_t::GetAttrMatrix(0)`.
  3001 is the equivalent attractor of a plain static visual mesh. `FUN_1010603b` treats
  these two results as already world-space; other anchors compose with the actor world frame.
* DisplaySystem `VisualMesh_t::GetAttrMatrix` **0x1006c475** searches graph connectors in the
  exact table order at `0x100af3a4`: `Attractor01_weaponfire`, `Attractor01`, `Attractor02`,
  `Attractor01_weaponfire01` through `06`, then null. CAT's named lookup
  `GetAttractorMatrix` **0x10072b4e** delegates to `RCATMesh_t::GetAttractor`.
  The static archive connector's `originator` references its frame; every ancestor's
  `anim_matrix * local` must be composed, not just its local translation.
* Actual rdb **1010001/15839** rifle and **262556** guard rifle contain
  `Attractor01_weaponfire`; **7796** shotgun contains `Attractor01_weaponfire01`.
  Read-only archive inspection gives composed AO muzzle translations respectively
  `(0.002563557, 0.02921438, 0.8932234)`, `(0.08988604, 0.13462976, 1.0123588)`,
  `(0.0001522300, 0.05340150, 0.2358599)`.
* `ActorRig::effect_anchor` returns column-major **model scene-space** matrices;
  Avatar/Player passthroughs compose heading/position/body scale to **world scene-space**.
  `weapon_effect_anchor(place)` explicitly composes the cached graph connector through the
  currently animated hand attachment, then mirrors Z exactly like rendered mount parts.
  Rig/Avatar/Player `effect_anchor(3000)` is the right-hand convenience; consumers resolving
  a weapon identity must use the explicit place method for its actual slot.
  Missing weapon connectors return `None`, not a sound-camera position or arbitrary offset.
  The existing static-mesh caller owns 3001; a CAT actor does not invent one.
* Regression `real_rifle_effect_anchor_uses_authored_muzzle_and_animated_hand` checks both actual
  rifle records, numeric bone/hand anchors and animated mount composition including the terminal clip
  time. Added, **not executed** in this assignment; Main owns build/test/live verification.

### 7.2 Authored weapon visuals

* `FUN_1009ad7d` consumes three four-tuple tables, **not stat 413/414 as direct
  script IDs**. Event-10 item `WeaponEffect` spells `0xcf49`, `0xcf53`, `0xcf54`
  supply `(attractor=stat86, effect=87, note=73, color=89)` via `FUN_100a761f`,
  `100a7803`, `100a7863` and `1009ad2c`. Repeated notes replace their tuple.
  Stat 413 is the weapon category (`1009dd32`, including grenade 9); stat 414
  defaults to 49999 (`10085bf8`/`1009b913`). It is not substituted for those tables.
* The groups are muzzle, tracer and **successful-hit impact**, not critical-only:
  `1009b4ac` calls `1009ad7d(..., 1 < hit_kind)`. Empty impact tables use 62002;
  legacy tuple effect 2710 is likewise mapped to 62002. Misses still render firing
  and tracers. Muzzle's note-zero default means attack 0xb; the other defaults
  match each qualifying attack note. `GetImpactAnim` maps 0x7f/81/82/84/default
  to target attractors 1001/1007/1008/1000/1006 respectively.
* `Setupf/gfxtweak.bin` is a bounded little-endian count followed by
  `(id,class,payload-word-count,payload)` CMSBlocks (`10106be2`): actual file
  333132 bytes, 2687 records. The renderer reads authored flare class1005 and
  moving-cord class1025, not generated generic flashes. Material metadata comes
  from the complete 102-entry `10106f39::GetMaterial` switch; `10106e2e` resolves
  its texture names as type1010004 through NameTable.
* Actual Solar-Powered Assault Rifle item121569 binds muzzle2005,
  cord2750 and impact2710→62002. Muzzle2005 is `x_smoke.png`, 64 sprites,
  radius0.1 and lifetime0.1; cord2750 is `s_bullet.png`, speed25, length1.25,
  width0.0625. Impact62002 uses the same bullet texture for 128 authored streaks,
  lifetime0.01..0.2 and radius0.01. These values were read from the installed
  table, not fitted from screenshots.
* Solar Pistol muzzle2000 is **class1006**, while tracer2601 is **class1013**;
  they are not two instances of the same starburst class. The star's parameters
  are loaded by `100dd9b8`, its bursts by `100de106`, and its empty-pool/repeat
  timer by `100de9fb`. The flare family uses the authored emission rate and
  sprite capacity, rather than emitting every sprite only at construction.
* Cylinder2601 keeps material index −1 as the native untextured cylinder:
  GC `100fd699`/`100fd754`/`100fd855`, DS `100109a4`/`10010472`. Its sixteen
  radial segments, cap/open-tail flag and layers come from the native visual,
  not a camera-facing substitute quad. DS uses ONE/INVSRCALPHA blending.
* Cord4 class1024 uses three twenty-link ribbons (`100ff756`, `100ff811`,
  `100ff525`; DS `1000e3e8`), including native phase/random evolution and
  averaged ribbon joins. Class1026 uses Cord4 plus the native 64-slot Sprite3
  pool (`1010073d`, `10100855`, `10100b27`; DS `10028966`, `100288f3`,
  `10028b9c`). Its sprite geometry and atlas come from that separate visual.
* Segmented flare class1019 uses `100fde5a`/`100fdfef`/`100fe2ef`/
  `100fe4a3` and the native speed100 constant at `10155eb0`; its word10
  is not projectile speed. PathBlur class1021 uses `100fe985`/`100feacd`,
  DS `10031830`/`100311a8`/`10031253`, and texture1010004:8406 for its two
  strips. Recursive tracer1022 is an authored child wrapper, not another quad.
* Ballistic class1027 (`2780`/`2781`/`2782`) uses native ABIFF resources:
  DS `100616e4` resolves selectors to type1010001 names, including rock01–07,
  the authored gib/ice/slime families and shell/tower fragments. GC
  `10101591`/`101016cb`/`101017b6` supplies the pool/emission/motion parameters;
  DS `1001c4a6` bounds the visual pool to128 and `1001c435` updates rotations.
  Native fractional random uses the shared R250 seed0xe6f1 (`10152aac`),
  separately from the CRT random stream.
* `100dcd93` derives paired sprite endpoint velocities, angular ranges,
  lifetime, radius and ARGB interpolation from payloads. DisplaySystem
  `GfxVisualFlareType0::NewSprite`/`ProcessSprites` uses radius directly (not half),
  P/Q-aligned quads and SRCALPHA/ONE additive blending. Material atlas modulo
  columns / division rows is retained as observed in `1001379b`/`100137aa`.
  `10100104` advances cords and clamps their ends to the actual hit location.
* Integration: real `AttackInfo`/`SpecialAttackInfo` slot context, animation notes,
  actual actor/weapon geometry (§7.1), dynamic actor model/frame upload, and
  existing `MuzzleFlashFX` (category8) / `TracersFX` (category2) preferences.
  Special results preserve their own item/slot/damage; no stale normal-hit context
  or camera-position effect anchor is used. Zone reset clears active effects.
* Effect preference categories are the retail six-bit mask, not a shared
  “nano/buff” toggle: Buffs=1, Tracers=2, NanoEffect=4, MuzzleFlash=8,
  Environment=16, Others=32. Gamecode `100ce1fa` packs preference offsets
  8–13; DisplaySystem `1005fd60` installs their callbacks. Cast visuals use
  category4, while visual ApplySpells handlers use category32.
  Categories belong to the spawning handler, not CMSBlock word 1: native
  Sequencer parameters GC `100ec81f` use word 1 as entry count
  (`effects_buff300x.rs`, decoder), and Highlight uses it as mode. The unused
  generic word-1 category accessor was removed; the explicit caller gates remain.
* Bounded-parser, tuple replacement/miss-hit gating, real rifle-art,
  projectile-parameter and real item-binding regressions are covered by the
  release workspace checks: build, **1292 passing tests**, and
  `cargo clippy --release --workspace --all-targets -- -D warnings`.
* The renderer now uses an UNORM target and gamma-space shader output, so native
  DisplaySystem blend factors operate in the same space as D3D. Offscreen
  before/after captures preserve the two HUD fixtures byte-for-byte. Mean
  absolute RGB-byte changes are 2.1446 for playfield566 daytime, 1.9849 at night,
  0.1565 for dungeon127, 1.0374 for login and 0.9799 for character selection;
  these comparisons include changed blended surfaces and MSAA edge resolves.
  Geometry, layouts, textures and filtering remain unchanged. These are
  offline comparisons, not a claim of retail/live-window equivalence.
* Six installed-asset effect checks pass, including actual GPU frames for
  starburst2000, cylinder2601, continuous flare6200, projectile2750, rock
  resources2780–2782, rocket mesh71520, particle strips71512/71342, and
  particle dependencies71516/71904–71906/71340/71341/71343. The mesh and
  particle regressions compare rendered pixels with an empty pass, rather
  than accepting a background-only screenshot. Rock captures follow actual
  transformed resource bounds; particle captures retain authored transparent
  birth frames and cover the later atlas/alpha rise. No live session was used.

* Fallback swings use the preloaded `ATTACK_KEY` NPC record variants
  (`dynels::build_char`, `anim_key_variants`) as independent positive-count holder nodes.
  `play_swing` suppresses an already active list key but preserves distinct-list
  nodes, their authored lifetimes and retained attack slots. The regression
  `distinct_swing_lists_preserve_parallel_holder_nodes` checks Burst followed by a normal swing
  at 50 ms, an actual group-0 rifle holster inserted after Fling at 133 ms, dynamic target/retained flags,
  and missing weapon-special list fallback to `0xb` suppressing a later normal swing.
  `arms::real_records` uses a separate creature holder
  for innate attacks: reusing the rifle holder already fills one slot, so
  `FUN_1006ac03` / `FUN_10067fbe` allocate its innate entries at 1 and 2,
  not 0 and 1. Melee/projectile assertions remain unchanged. These repairs have
  not been executed here.
  The fallback is selected explicitly by `play_swing(None)`, not by comparing
  numeric clip IDs: authored clip 1034 shares `ATTACK_KEY` and must retain its
  normal one-shot path. The death replay fixture awaits `Model::Ready` with a
  bounded deadline before advancing its unchanged thirty-second simulation;
  all death selector, phase and sound assertions remain intact.

#### Fixed-step live frame capture (2026-10-07)

The retained offscreen harness runs the same `Play` and renderer as the window.
`arm=shot:1:note,Q=0.1,capturewait=30` saves the actual own-note processing frame
as `shot-0000.png`, then every fixed-1/60-second frame (60 PNGs).
`arm=burst:1:special,M=0.1,capturewait=30` and the equivalent `L`/Fling recipe
start at SpecialAttack processing. Use `AOMAC_LIVE_SHOTS=/tmp/<owner>/` and
the live lock; keep the same-target fight active between specials.
`frames=prefix:2` saves 120 frames; NPC phase/clock recipes are in `npc.md`.
GPU readback can take longer than simulation time. This is offscreen evidence,
not a real-window or retail-footage claim.

Observed Aomacvolk rifle frames: artifact21482 had no visuals before the preference
fix; artifact21528 `diag-normal-0000..0059` showed muzzle2005 at authored3000,
the cord2750 traveling toward the target, and successful-hit impact62002 in
`diag-burst-0094..` at target1007. Muzzle emission ended after its authored0.1s.
Artifact21582 `final-burst-0000..0059` retained rifle-burst1024/source14770
(1000ms, rate1): attack notes at133/233/333/433ms appeared at frames8/14/20/26.
Fling resolves rifle-shot1023/source14773 (866ms, rate1, attack200ms), as observed
in artifact21528. Artifact21651 additionally confirmed Special150/list0x1c;
the target's subsequent normal-hit death at frame8 incorrectly discarded that
clip's pending attack note. Retail holder nodes survive holster/idle and later
swings: GC `1003c216` inserts independent nodes, `1003bfd2` suppresses an active
list key, and `1003c036` flushes remaining attack notes when a CAT handle ends.
DS `10074228` cannot remove group-1 swings with group0 holster/idle. The missing
Fling effect is a holder-lifetime mismatch, not a faithful interruption.
The owned patch passed clean-origin/main workspace tests (1300 passed,
13 ignored), release build and strict workspace/all-target clippy.

### 7.3 Nano visual controls

* The two-dynel cast API `CreateEffect2` at GC `100d1de5` dispatches through
  `100d11b1` to **class1010 `_GfxControlSpell1_t`**, constructor `100f3497`.
  It does not dispatch to class1001. Parameters: `100f2234`; `NextState`:
  `100f20c2`; source-child phases: `100f2495`/`100f2656`/`100f2711`;
  traveling/impact phases: `100f280e`/`100f2b1b`/`100f290c`.
  DS `10027df8`/`10028206`/`100282b6` draws its own authored-material sprite
  strips with SRCALPHA/ONE, without depth writes.
* Class1001 is the separate periodic body-profile FSM: dispatch `100d0102`,
  constructor `100d3cce`, parameters `100d39ba`, process `100d3b0e`.
  It emits the authored children in words10/11, uses source-identity shared
  five-second throttling (`101601f8`) and random retry (`1016b338=1/16384`),
  and preserves word21's −1/infinite, zero/no-emission distinction.
  Cancellation (`100d385e`, vtable `1016b404` slot6) sets only `+0x14=1`.
  Deleting destructor `100d4130` calls `100d3a93`, which frees the source
  anchor (`+0x34`) and decrements/removes the source throttle entry; it never
  deletes emitted effects. `100d3b0e` keeps child handles only in locals for
  colour setters, not in controller fields. Thus Body Boost cancellation
  stops future pulses but lets an already emitted pulse finish.
  Authored `gfxtweak.bin`1070 (class1001, payload offset `0x588`) has
  word8=−1, words10/11=20091/20096, word20=4, word21=−1. Both children
  are class1002 with word8=1.5 seconds (payload offsets `0xad04`/`0xaf34`);
  that finite visual tail is retail behavior, not a stuck buff controller.
  Class1002's deleting destructor `100d59e3`→`100d5133` deletes both
  owned descendants (`+0x7c/+0x80`): flare6203 and cord20092/20097.
  The flare's authored20s lifetime cannot extend its owning1.5s pulse.
  Regression `body_boost_cancel_preserves_emitted_children` checks the
  authored records, throttle release and immediate child retention, then
  actual renderer expiry: descendants and Host submissions disappear
  within120 frames at60Hz and no pulse resumes over another360 frames.
  Red cast particles are separate:29091 starts46129 (class1010, mode1,
  source phases0/6/8s) and finishes43421 (class2004,3s plus350ms particles).
  Successful cast completion `1007ac9a` emits finish effects and returns
  idle without DeleteEffect; destructor `1007b290`→`1007b27f` also does
  not delete that handle. Removing the buff soon after casting therefore
  cannot establish a1070 leak from red particles alone.
  Renderless controller/orbit `vertices=None` means no geometry, not expiry:
  the renderer retains these instances until their process step ends them.
  `captured_body_boost_zone_route_emits_and_retires_visible_pulse` replays the
  four real frames in `docs/captures/body_boost_pulse.rec` through `Zone::on_frame`,
  then the normal buff/anchor/renderer update route for18seconds. Its connector
  callback uses the captured own avatar's authored rig, including Spine3/id1004,
  rather than supplying only the root. It requires nontransparent cord geometry
  near Spine3, then expiry following the actual BuffIIR removal. This is offline
  route evidence, not real-window visibility evidence.
  The four bare-pulse GPU frames inspected on2026-10-07 at approximately
  0.05/0.2/0.5/1seconds show only tiny white dots/clusters on blue background,
  not an obvious orbit or cord. Nonzero vertex RGB alone does not establish
  perceptual visibility; these empty-world frames also omit avatar/world
  depth. The temporary GPU capture hook was removed after inspection.
  Root cause: class1001's children and class1002's cord child carried the Dynel
  identity but disabled source tracking, bypassing authored connector lookup
  and placing the pulse at the root. GC `100d3b0e` calls CreateEffect2 with the
  resolved Dynel and attractor0 for both children; `100d57bb` then selects
  word7/id1004 and the body profile. `100d52dc` creates the flare from a world
  vector but the cord from the Dynel. Tracking is now enabled only on those
  Dynel routes; flare/vector placement and authored sizes remain unchanged.
  Actual1070 profiles are20093/20098 (payloads `0xae1c`/`0xb04c`), with
  breed1/male/shape1 radius0.20; flare6203 radius remains0.01–0.02.
* Class1002's orbiting children (`100d4f72`/`100d52dc`/`100d53e3`/
  `100d57bb`/`100d4d6b`) use their actual class0 body-profile records
  (20013/20018,42words), selected by Breed/Sex/BodyShape/MonsterScale,
  rather than a generic character radius. Native signed angular subdivision
  is0.45 radians (`10167f88`). Class1003 (`100d5b7d`/`100d5d22`/
  `100d5dcb`/`100d5a02`/`100d6480`) uses the separate256-link Cord4
  overwrite ring, newest-first ordering, local point/velocity updates and
  authored link lifetime; DS `1000e9b7`/`1000df03`/`1000e99b`/`1000e9f9`.
* Fire1004 (`100dc296`/`100dbedb`/`100dc044`), Nano0/1007
  (`100e6e95`/`100e62a3`/`100e6bf1`), Nano1/1008
  (`100e827a`/`100e750e`/`100e8421`), Smoke1009
  (`100f09e6`/`100f0616`/`100eff68`) and Sprite1012
  (`100f62a7`/`100f5f2e`/`100f67d6`) remain distinct native controls.
  DS `100267c1` supplies their wind/update modes; Nano1's staged emissions
  use raw authored bone matrices (`10105b22`), not one generic source point.
  Sprite1012 selectors1/2 explicitly end in native `100f5f2e`; only0/3
  construct visuals. Native invalid selectors are not remapped to valid ones.
* Cast templates that omit their optional mode word33 retain the native zero
  value: CMSBlock integer accessor `10106872` returns zero for a missing
  parameter. This is an observed accessor default, not replacement artwork.
* Tracer wrapper3026 (`10114717`/`10114932`/`10114622`/`101145f1`)
  moves its real child at min(authored speed,distance×5), forwards the native
  source matrix, and starts its real impact child on termination. Actual71001
  references71520(class3025) and71004(class2007); the latter lists71340–71345.
  Meta class2007 (`100e59b2`/`100e5a25`/`100e57f3`/`100e5799`) maintains
  those actual child handles, including its native lifetime marker and cleanup.
* Mesh child71520 is the authored `EP03_shoulder_rocket.abiff`, selector0,
  scale2.5, not sprite artwork. Class3025 dispatch/loader/process:
  `100ce4f7`/`1010c5a6`/`1010cf05`/`1010c94a`; its opacity envelope uses
  `101161a1`/`1011634c`. Native vehicle/camera/oscillation and rendering-state
  variants are separately implemented, including actual ABIFF node animation under0x1000.
* Class3020 mode0, used by71512/71342, is the separate TParticle visual:
  GC `1011277c`/`101125fd`/`10112bb0`; DS `10029de9`/`10029c81`/
  `1002a350`/`10029aa7`. Its crossed tapered strips, constructor random walk,
  local Euler motion and packed piecewise colours come from that visual.
  Modes1–3 now retain their own native endpoint/RNG behavior (§7.9).
* BParticle2/3028 (`1010bf1f`/`1010ac44`/`1010ba11`/`1010af47`;
  DS constructor `1000b432`, draw `1000b4fc`) keeps its native burst/recycling and strict
  colour-knot intervals (`1011623a`). Payloads with36/42/44words carry
  zero/three/four knots respectively; absent CMS fields return native zero.
  BParticle/3024 mode8, used by71343, is a different control/visual:
  `1010a6ac`/`1010a0f0`/`1010a3b5`, DS `10009938`/`10008bce`/`1000a70f`.
  The remaining3024 initialization modes0–12 and3028 terrain/environment/body-scale
  branches now have separate native implementations; their addresses are recorded below.
* Cast start loops the authored stat0x178/default203 at speed1 (`1007b084`);
  release uses stat0x179/0x17a/default201/202 once. Timed and instant casts
  both wait for actual release completion (`1007ac9a`/`1003c4d5`) before
  target visuals. Repeated same-clip starts reset time to zero, matching
  `1003ca30`→`1003c392`→`1003c216`→`1003bd7a`→`100108be`.
  Own and foreign anchors refresh after their pose clocks, before effect draw.
* GroundShake3032 is a camera control, not geometry: `1010ed0d`/
  `1010eaa3`/`1010ebd8` computes signed R250 offsets and distance/envelope
  attenuation. N3 `1001ff2f` runs on Camera+0xa4, reads its +0x130
  (=Camera+0x1d4), adds that offset to VisualCamera position and preserves
  rotation. This indirect consumer establishes eye-only shake; scanning only
  direct Camera+0x1d4 references would incorrectly conclude it is unused.
* Audio4000 (`100d3190`/`100d30c1`/`100d3012`) sends AFCM0x1a/0x103;
  SandyInterface `100071ed` interprets volume/radius/duration/delay.
  Actual71345 selects `SM_Sandy_Game_Explo_Med` from table102c3f28, index12,
  with volume1,radius120,duration0,delay0,velocity0,probability100.
  Duration, delay, sequence and radius now retain native SI behavior (§7.9),
  rather than rejecting or discarding those arguments.

### 7.4 Template cast sounds

`CharCastNano_t` ctor GC `1007b084` plays stat269 (`0x10d`) at the
caster's global position, duration override0.2s and volume0.6. Update
`1007ac9a` refreshes269 during casting (assembly `1007af13`), then271
(`0x10f`, assembly `1007aeac`) while the release clip is unfinished,
both duration0.2s/volume1. The constants are `1015e41c=0.2` and
`10161740=0.6`. After successful release (`state+0x28==1`), stat272
(`0x110`) plays once at the target, duration0/volume1. Cancellation,
death and disappearance stop refreshes and do not synthesize a finish.

Incoming `CastNanoSpell` GC `10072306` forwards its flag to
`10051754`→`10050ed9`, stored as queue byte+0x2c. Queue result+0x20
is independently initialized to1 (`10050f17`); `1004ee7e` returns it
to `CharCastNano+0x28`. Thus timed received casts start successful;
the instant `100500ca` path additionally gates its finish visuals on
byte+0x2c. The port stores explicit `finish_enabled`, not a visual-ID
heuristic. Cast/audio stages run without the FX renderer or posed
connectors; visual starts wait for connectors while casting, and finish
visuals wait for target connectors without repeating the finish sound.
Foreign nano charge/release use the existing single-clip `play_once`
loader: charge becomes `Special::Cast`, release remains `Special::Once`.
They do not create the combat holder's independent positive-count list
histories. Transition resets discard stale pending single clips; cancel
also discards outstanding charge/release loads so a late worker reply
cannot resume a canceled nano.

Wire cancellation is distinct from removing buffs: the compact action
map at `1005efdf` maps action0x66 to case0x1b, which calls
`1004f504(0,0)` at `1005d260` (first pending cast), and action0x6c
to case0x1e, calling the same function at `1005d8e7` with
`identity_b.instance` as result and `identity_b.kind` as spell.
Action0x75/case0x22 calls `1004fdad` at `1005d921`; after resolving
the interrupting character and nano it calls `1004f504(4, spell)`
at `1004fe5c`, using `identity_b.instance` as spell. `1004f504`
removes the selected pending spell and sets holder+0x10 if it was
first (`1004f575`), causing `CharCastNano`'s cancellation branch.
These routes remove pending sounds/animations/effects but leave
already applied buffs alone. Regression coverage exercises all three
actions, own/foreign selectors, missing271, actual Shadow Touch bytes,
and a real looping PCM surviving its sample end then expiring after
the duration override plus authored fade-out.

Action0x89/case0x2b instead calls queue-clear `1004f274` at
`1005d24e`, without setting holder+0x10. Stage0's next update finds no
pending cast (`1004ed14`) and enters release rather than cancellation;
there is no successful result to copy. An already released controller
retains its cached result. This distinction is covered separately:
queue clear can play271 while releasing but does not invent272 for
an uncompleted cast. The other native callers of `1004f504` are
`10050622` (queue failure processing) and `1005942e` (actor transition
`100593d3`, conditional on its cancellation argument). The other
`1004f274` call is the death ctor at `1007b514`. Death/disappearance
already route through the shared cast termination path.

Zero XYZ is SI `10002d98`'s non-positional sentinel, not a world-space
source at the origin. The shared `play_game_sound_with` path, nano
duration path and authored-effect path all preserve that convention;
an offline actual-bank regression uses a distant listener to distinguish it.

SI `PlaySample` `10002d98` re-arms the sound-definition handle to
`fade_out + duration`; `FrameProcessSound` `10003b70` counts it down,
fades during its final authored fade-out and stops at zero. The native
key is the sound definition, not a new voice each frame. The port uses
the existing looping mixer/keepalive mechanism, including positional
attenuation; `AOMAC_AUDIO_LOG` records emitter, selector, ID, duration,
volume and actual voice IDs even with muted output.

Body Boost29091 has269=`35a9ce7d`
(`sfx/spells/chant_base_pos_lo_loop.wav`, authored fade-out1s),
270=`94bb7805`,272=`80d5111a`, but no271. This controller does **not**
read270 or substitute it for missing271. The absent stat accessor's
unset ID `499602d2` has no sound definition. Actual `zone_ithaca.rec`
casts at23825/29898/74495ms are NPCShadowTouch163449, an instant nano
with no269–272: its template-sound path is silent, not a Body Boost
stand-in. Its authored effect2710 may independently emit effect audio.

**Own-character audio position correction (2026-10-07).** The own avatar
is simulated by `Player`, while `Dynels::chars[own].pose` belongs to the
remote-character mover. Nano269/271 and self-target272 now use the own
avatar's world-space origin (`Player::effect_anchor(0)`; CAT anchor0 is
identity before the avatar transform), passed through the existing
`nano_visual_frame` anchor callback. Foreign sources retain their mover
position. Scene conversion changes only Z (`zone::scene_pos`); a height
difference is not an axis swap. The existing stage regression deliberately
separates the live avatar from the remote pose and checks all three selectors.

**[DATA]** Installed `SM_Sandy_Game_Dummy.sbf` records at offsets
`0x18a6ae` (269/`35a9ce7d`) and `0x18c01d` (272/`80d5111a`) both
have min/max distance0/15m, volume bytes127/127 and probability100.
Both decoded files exist;272 uses
`sfx/spells/cast_base_pos_target_med.wav` and has one child.
`AOMAC_AUDIO_LOG` now records the runtime definition, resolved sample,
source/listener scene positions, distance, FX gain and caller parameters;
rejections distinguish missing runtime/definition, distance, decoding,
empty sample, keepalive sample and mixer pool. Muting still acts only
after mixer statistics. An instrumentation-only offscreen live capture on
2026-10-07 logged120 own-character requests (33588), all refused:
source `(930.0051,9.733654,-759.66864)`, listener
`(932.2918,27.584023,-755.5124)`, distance18.469948m versus authored15m.
Both269 and272 explicitly reported `reason=distance`; definitions and
sample paths resolved. The own server/avatar height was24.21m.
The fixed offscreen muted replay logged107 own requests at avatar origin
`(930.0051,24.21451,-759.66864)`:269 sustained voice5 and272 allocated
voice7. During casting the mixer had4 voices and RMS0.1902 (output gain0);
the request volume0.6→1 and repeated269 calls reused voice5. Body Boost
has no271, so no invented release/loop sound is expected.

The same replay exercised116628 at inventory slot0x42 while seated:
60 captured60Hz use frames showed the authored13600 bright pink-white
Stars expanding around the actor, then fading. Nano rose20/36→36/36.
Its template has no animation/sound map (see `interact.md`); the seated
pose plus authored visual callback, not an invented sound, is the contract.
Body Boost cast frames showed red hand effects. After waiting10s beyond
the one-second cast capture, NCU was1/8; removal's120 captured frames
and a further2s wait left NCU0/8 and no cast/torso particles.


### 7.5 Persistent buff selector census

The ignored `retail_persistent_buff_census` parses stat413 from every installed
type1040005 nano record (not a raw byte-pattern scan). The 2026-10-07 census
observed11,160 records, no malformed records, and6,366 positive bindings:
154 distinct installed selectors in21 classes, including the no-effect sentinel
49999. The existing authored visual inventory is therefore153 IDs, not the
earlier rough160-ID estimate. Three additional selectors have no installed
gfxtweak template:16451(one binding),39606(seven),39745(sixteen). The census
asserts this exact known missing set and fails on any changed gap; no other
artwork is substituted.

Exact class/selector inventory (the runnable census also prints source nano IDs):

| Class | Authored selectors |
| --- | --- |
| 1001 | 1000/1001,1010/1011,1020/1021,1030/1031,1040/1041,1050/1051,1060/1061,1070/1071,1080/1081,1090/1091,1100/1101,1110/1111,1120/1121,1130/1131,1140/1141,1150/1151,1160/1161,1170/1171 |
| 1005 | 43233,43243 |
| 1009 | 43657,80000,80001,80002 |
| 1010 | 46262 |
| 1012 | 43607,61085,61086,61087,80006,80007,80008,80009 |
| 1018 | 2710 |
| 2004 | 43024,43044,43046,43072,43121,43123,43141,43299,43653,43713,45004,45008,45025 |
| 2005 | 14400,43068,43307,43768 |
| 2006 | 43452,43455,43457,43458,43462 |
| 2007 | 18200,43681,43682,43683,43684,43685,43686,43687,47382,47396,47398,49999,71002,72303,72304,72305,72306,72307,72308,72309,72310,72626,80004 |
| 2011 | 11506 |
| 3003 | 12350,12351,43054,43062,43151,43152,43473,43474,43608,43609,43610,43611,43711,43722 |
| 3004 | 43613,43614,43615,43616,43617,43618,43619,43620,43621,43622,43629,43630,43646,43647,43648,43649,43650,43651,43666,43712,43725,45079,45081,47500,47501,47502,61090,71015,72381,73007 |
| 3006 | 43108 |
| 3020 | 71512 |
| 3028 | 72027 |
| 3029 | 73006 |
| 3032 | 72380 |
| 3034 | 72260 |
| 3038 | 72233,72360,72361,72362,72363 |
| 3039 | 72422 |

#### Native persistent-control evidence

* Stars2004 uses loader `100f6d54` (duration word26 overrides word8),
  constructor/init `100f7a63`, all29 process modes `100f7f3c`, and graceful
  termination `100f6c27`, which stops emission through control+1648.
  Its visual is DiaBill DS `1001105e`, not `GfxVisualStar`.
  Integer helpers `1013ecf0`/`1013ed26` truncate; they are not RNG calls.
  Mode13 uses reversed-Hamilton multiplication `1007c09c`; mode26 uses
  locator cubic helpers `101051e7`/`101050d0`; modes21/22 use the actual
  twelve-entry bone connector chain at `102c4fe8`; mode15 samples the
  live CAT surface in callback `100f6eb6`; mode25 consumes actual ground
  height. Missing terrain, body geometry or named attachments are not
  replaced with a synthetic plane, character radius or source point.
* Graceful virtual slot6 is distinct from `NextState`: BPHFSM1001
  `100d385e`, Spell1/1010 `100f20bd`, Sprite1012 `100f5a3c`, and base
  `100a719a` for3020/3032 mark the control terminated. Smoke1009
  `100f0352` and Sparks1018 `100f0f33` instead set duration to elapsed
  plus their authored maximum particle lifetime (word26/word35).
  Meta2007 `100e5888` forwards graceful termination to all ten real
  child handles. BParticle2/3028 `1010aa16` sets its stop byte; process
  `1010af47` immediately terminates ordinary particles, while held-life
  flag0x200000 releases their remaining lifetimes.
* Highlight2011's installed persistent selector11506 uses mode2:
  loader `100e286a`, process `100e29a7`, envelope
  `10108089`/`101081a5`, and graceful transition `100e283c`.
  Its two-second authored ramp is separate from the duration; mode2's
  native default infinite duration is −1 at `10155dd0`.
  Graceful termination sets duration to elapsed plus that ramp.
  The process writes live root/held transparency and emissive, not
  replacement geometry. Native RGB is gamma-space; the scene's material
  override stores its linear equivalent. It never changes shininess.
  Mode3 is a separate head-attractor-only specular path, not the mode2
  root effect. Cleanup `100e2bde` refreshes alpha and clears emissive
  and specular on the applicable native visual frames.
* Shield2/3034 `10110b31` and Trail2/3039 `10114b20` immediately end
  when their source dynel is actually deleted. This is not equivalent to
  BuffIIR removal or death: those preserve their native graceful path.
  An omitted fresh CPU pose retains the last actual posed surface rather
  than being treated as a deleted dynel or replaced with the bind pose.
  Source CAT model replacement invokes Shield2's native Stop path
  (`10111147`/DS `1001dffe`).
  Ordinary installed-data renderer regressions cover lifecycle teardown
  and deleted-source versus unchanged-pose behavior; these newly added
  checks were not run during implementation.
* Static buff geometry shares GPU models by authored effect selector.
  CAT surface controls3003/3034 instead own private GPU models because
  their topology comes from the affected actor. Delete, expiry and clear
  retire those models through `Host::actor_model_removals`.
  Actual own-avatar appearance replacement forces fresh pose, topology
  and source-material input even when `MODEL_KEY` is unchanged, and
  invokes3034's native Stop path. A vertex-count comparison is not an
  appearance-change detector.
  The regression
  `buff_refresh_retires_gpu_models_and_same_key_appearance_stops_surface`
  was added but not run during implementation.
* The ordinary `own_nanos::tests::synthetic_lifecycle` regression also
  asserts raw visual duration (not stat464-scaled NCU time), remove-before-add
  ordering on refresh, no visual removal at timer zero, and a single removal
  event for repeated authoritative BuffIIR removal. It requires no retail
  assets or GPU. The three installed-data lifecycle tests above and
  `authored_9010_parent_delete_and_target_loss_delete_actual_children`
  also run ordinarily when RDB and gfxtweak are present, following the
  existing asset-presence early-return convention; they do not require GPU.
  Census and offscreen frame capture tests retain their explicit ignored gates.


### 7.6 Vector-created replicated classes

Fresh read-only Gamecode constructor evidence corrects the legacy “Spell2”
label: class1011 installs `_GfxControlSpinningShot_t::vftable` at `1016cc04`.
The vector/vector gate `100d0eba` permits1011 and3007; vector/dynel gate
`100d1018` permits only1011. Installed templates are9010 (1011,26words)
and36000 (3007,18words), not generic replacement effects.

* SpinningShot constructors `100f41c5`/`100f45e2`, loader `100f3b0b`,
  initialization `100f4053`, process `100f3d16`, delete `100f4741`→
  `100f3cab`: words10/11 own two real child controls; words12–19 supply
  start/stop ARGB components,20 is center speed,21 orbit radius,22 initial
  angle and23 frame angular speed. Assembly `100f3e6c..100f3e72` multiplies
  angular speed by this frame's delta, not total age. The children are moved
  together; destruction deletes both immediately, without a fade substitute.
  The target connector uses authored1–7 and flags0, through `10106259`
  (vector) or `1010668c` (dynel). Flag bit0 enables target tracking; missing
  tracked targets terminate through `1010603b`. Flag bit1 returns zero
  connector position (`10105eb4`). Source-vector effects do not require a
  source dynel identity.
* FallSteam vector constructor `100da927`, loader `100da828`,
  visual setup `100da71f`, process `100da6da`, delete `100dab12`→
  `100da691`: material9, direction bias4–6, position offset1–3 and particle
  parameters11/16/17/14/13/15 feed the actual DisplaySystem visual.
  DS constructor `100394d1`, particle parameters `10038ef9`, activation
  `10039792`, simulation `1003983e` and draw `10038f5d` establish its bounded
  particle pool, native DisplaySystem RNG, atlas and alpha-blended billboards.
  The native pool factor is double `1008af80` =
  `1.2000000476837158`; capacity is trunc(length×rate×life×factor).
  Frame milliseconds truncate independently before accumulated emission;
  DS assembly `1003986c` is `DC C9` (multiply ST1 by ST0), establishing the
  delta×1000 conversion, not truncation of seconds. Nine DisplaySystem R250
  samples initialize each emitted particle; native atlas draw `10038f5d`
  preserves its alpha envelope and billboard sizes. Deletion destroys the
  visual directly; no independent invented fade lifecycle is added.
* Base `100d2531` uses seconds from engine+0x68: first process has delta0,
  the first advancing step is capped at0.033, then normal frame deltas apply.

### 7.7 Corrupted Crystal TParticle2

Effect73001 is class3031, flags515 (`0x203`), attr0, material74,
80-particle continuous pool; words12/15 emit at most three expired slots after
0.001 seconds. GC located dispatcher `100d0102` selects constructor
`101143bb` (allocation0xd8), not class3020 or3038. Parameter loader
`101131ec`, initialization `10113dbf`, process `10113604` and velocity
rotation `1010a9a0` establish randomized lifetimes0.15–0.4, widths4–8,
velocity axes−5–5, per-process width multiplier1.01 and trail length1.
The two independent colour curves begin at word34 and47; `1011623a`
uses strict interval bounds and the last colour at exact knots.
DisplaySystem `GfxVisualTParticle2` constructor `1002a9e7`, draw `1002ac11`
forms two crossed velocity-aligned strips with tail width divided by
word33 (0.5), never camera-facing sprites. Native dispatch now reaches this
separate implementation through the shared buff renderer and
`Renderer::spawn_configured`; duration and identity/attractor remain caller inputs.
The terrain, animated-atlas, UV and camera-facing-alpha branches are separately
implemented from the same native process; see the additional evidence below.
Inventory SimpleItem has no visual dynel: GC `1010668c` requires its
`n3VisualDynel` cast before locator setup. Constructor `101143bb` marks failed
initialization terminated but still returns the allocated control, so CF26
does not take an actor-origin fallback merely because the visual is absent.
Built props with an actual visual/attr0 can reach the renderer normally.
Unlocated gate `100ce3be` excludes3031; the actual supported path is the
located overload. Vtable `1016e9f4` graceful slot6 is base `100a719a`,
not a particle-drain override; process slot1 is `10113604`.


### 7.8 VulcanRocks (class1029), including effect45083

GC dynel dispatch `100d0102` allocates0x98 and calls `101034c2`;
the vector dispatch `100ce4f7` calls `10103147`. Both install
`_GfxControlVulcanRocks_t` vtable `1016d234`, load parameters at
`10102ef2`, initialize at `1010303f`, and process at `1010366b`.
The unlocated overload `100ce3be` does not accept this class.
The loader reads flags0, connector1–7, duration8, speed10,
elevation endpoints11/12, colour13–16, capacity20, rate21,
selector count22 and selectors23 onward; the next word is the
delete-rocks flag (`10106872` returns zero if absent; trailing unused authored words are retained).
Process uses cumulative ceil emission, uniform azimuth, authored elevation,
connector basis/position, gravity9.8, terrain/closest-surface reflection
with0.75 damping and at most four bounce retumbles. Controls older than
the newest50 Vulcan creations terminate. Slot5 (`10102ee1`) updates the
connector matrix; slot6 (`10102eed`) terminates immediately. Deleting
destructor `10103b8f` calls `101035f7`, releasing locator, visual list and
selector array; the authored delete flag selects `DeleteRocks`.

DS `GfxVisualRockList` ctor `1001c5d0`/capacity setup `1001c4a6`
caps capacity at128; the global rock handler `1001c0ea` has512 slots.
`GetNew` (`1001c4f9`) obtains the authored
VisualEnvFX resource through `1001c199`; `ProcessRocks` (`1001c435`)
updates actual mesh translation and axis-angle quaternion, not billboards.
VisualEnvFX loader `100616e4` resolves selector39 to
`gib05_slime.abiff` (1010001); selectors5–10 and42 are material-only entries,
43–45 have no resource, so the native mesh getter `100612c3` cannot create rocks from them.
List destruction `1001c68f`/`1001c602` releases objects to the global
handler; handler `1001c130` removes released zero-lifetime meshes,
while explicit `DeleteRocks` (`1001c659`/`1001c097`) deletes immediately.

Installed effect45083 has flags1, attractor3000, connector X rotation
0.125663713 and Z rotation4.83805275, duration−1, speed6.4,
elevation1.25663710–1.57079637, capacity1, rate256, selector count1,
selector39 and delete flag5. These authored fields select the actual
slime-gib mesh and its original material/textures. The mesh renderer is
shared with class1027, but emission direction is the native Vulcan cone,
not1027's target ballistic path. All eleven installed1029 records are
covered by the existing ignored mesh-frame regression.

Isolated origin/main gate checks: the first `cargo test --release --workspace`
exposed the source-deletion omission (the other765 aomac tests passed,
13 ignored); the shared source-deletion predicate now includes attached1029
unless flag0x400 is set. Both authored45083 tests passed after that fix,
then the final `cargo test --release --workspace` passed1381 tests
(37 suites,15 ignored).
`cargo clippy --release --workspace --all-targets -- -D warnings` passed.
`AOMAC_EFFECT_FRAMES=/tmp/FxClasses/frames1029 cargo test --release -p aomac authored_native_rock_frames -- --ignored --nocapture`
passed. All44 frames for the eleven1029 records were inspected at frames
15/30/60/120: textured rock cones and actual gib/casing geometry, with no
substitute sprites. Effect45083 had3658/3072/3522/2977 visible resource
pixels respectively, showing the authored textured slime-gib tumbling.
This is offscreen evidence with the harness floor, not a retail-reference
comparison or a real-window/live-server pass.

### 7.9 Additional native class and creation evidence

The following records document implemented native branches, not a claim of
retail-frame equivalence. New checks named here have not yet been exercised
by the implementation workers; the parent owns the final verification evidence.
GC means Gamecode.dll and DS means DisplaySystem.dll; all addresses are hexadecimal.

#### Creation overloads and native nulls

`effects_dispatch.rs::Creation` preserves the fifteen exported native factory
forms, rather than attempting any class through whichever constructor happens
to be implemented. Source: decompile artifact22586, `/tmp/BuffFxCoverage/base.c`
and `/tmp/BuffFx200x.c`; lookup is GC `10106b12` and dispatch reads template+4.

| Form | GC factory |
| --- | --- |
| Unlocated / vector / matrix / RConnector | `100ce3be` / `100ce4f7` / `100cefaa` / `100cf872` |
| Hit-location integer / located dynel / tracer | `100d145c` / `100d0102` / `100d1218` |
| Vector→vector / matrix / connector / dynel | `100d0eba` / `100d0f50` / `100d0fb4` / `100d1018` |
| Dynel→vector / matrix / connector / dynel | `100d107c` / `100d10e3` / `100d114a` / `100d11b1` |

An absent class/form case returns native null before implementation support is
checked. A native-constructible but unimplemented case instead reports an
implementation error. Class0's twenty body-profile records are inputs to
class1002 (§7.3), not drawable controls: all fifteen factories return null.
Class2003 likewise has no constructor case in any factory. Neither is an
implementation gap. Classes1016/2000/3011 have native cases but no installed
template. The RConnector factory additionally rejects a constructed control
whose terminated byte+0x14 is set; dynel class1002 rejects before allocation
when GC `100e2cb4` finds that identity in Highlight registry `102e988c`.
Highlight ctor `100e2e10` inserts via `100ac37e`; destructor `100e2cf4`
removes via `1002df93`. Duplicate Highlight ends at construction; Stars
`100f751f` applies the registry rejection specifically to mode15.

Key restrictions:1010 accepts only the four dynel→target forms;1011 only the
four vector→target forms;3007 only vector→vector or dynel;1020 only unlocated;
2002/2013 only hit-location;5000 only RConnector;3017 accepts matrix,
RConnector, hit-location and dynel but not vector. Classes1013/1019/1021/
1022/1024–1027/3026 accept hit-location or tracer only. Regression:
`native_overload_cases_and_nulls` (not run here).

Complete class/form cases below use N=unlocated,V=vector,M=matrix,
R=RConnector,I=hit-location,D=dynel,T=tracer and two-letter source/target
forms from the factory table. Every omitted pairing returns native null.

| Classes | Native creation forms |
| --- | --- |
| 1000/1014/1015/1016/1020/3037 | N |
| 1001/1017/2011/2012/2014/3001/3003/3008/3034 | D |
| 1002/1003/1004/1006/1007/1008/1009/1012/1023/1029/3004/3005/3006/3014/3022/3023/3024/3025/3027/3029/3030/3031/3032/3033/3035/3036/3038/3039 | V/M/R/D |
| 1005/1018/3015/3019/3020/3028/4000 | V/M/R/I/D |
| 1010 | DV/DM/DR/DD |
| 1011 | VV/VM/VR/VD |
| 1013/1019/1021/1022/1024/1025/1026/1027/3026 | I/T |
| 1028/2000 | V |
| 2001/2008/3002/3013/3018 | V/D |
| 2002/2010/2013 | I |
| 2004/3011 | V/I/D |
| 2005 | I/D |
| 2006 | R/D |
| 2007 | V/R/I/D |
| 2009/3000/3009/3010/3012/3016 | V/M/D |
| 2015 | V/R/D |
| 3007 | VV/D |
| 3017 | M/R/I/D |
| 3021/5000 | R |
| 0/2003 | None |


Child creation retains its native form:1001 process `100d3b0e` uses dynel;
1002 init `100d52dc` creates vector orbit and dynel cord children;1011 init
`100f4053` and1022 init `100ff18c` use vector;3026 `10114793` creates its
primary through matrix and destructor `101145f1` its impact through vector.
Spell1 phases `100f2495`/`100f280e` use vector and `100f290c` dynel.
Sequencer3004 `100ec640` forwards stored mode0/1/2/3 as vector/matrix/
dynel/RConnector. Required hit-location and connector inputs are not invented.

#### Global, geometry and particle controls

| Class/family | Native implementation evidence |
| --- | --- |
| 1000 BlackOut | GC ctor `100d37ed`, loader `100d3667`, init `100d367e`, process `100d3494`, delete `100d3444`; record3100. |
| 1014 WhiteIn /1015 WhiteOut | GC ctor `10104a94`/`10104d5c`, loader `101048d0`/`10104c09`, init `10104907`/`10104c30`, process `101046c8`/`10104b44`, delete `10104678`/`10104b05`; WhiteIn graceful `10104a78`. Records3200/3000 retain native missing-field zero accessor `10106893`. |
| 3037 VisionTint | GC ctor `101158d8`, loader `10115741`, init `1011544e`, process `10115394`, delete/fog restore `10115311`; records71121/71122 use actual noise.PNG/fog_particle.png, three/five layers. DS `10023929` fullscreen repetition; `1002eec0` textured tint/UV scroll, flag0x400 jitter once,0x800 V-flip; priority7 `1002ece0`; WhiteIn DS `10023662` uses blend5/2. |
| 1017 Nano2 | GC loader `100e8c4f`, init `100e8cf2`, process `100e88bb` queries actual rig anchors1006–1014/2001/2000/1000; words10–17 are inert. DS ctor `10018c08`, eighteen-block update `1001975e`, perspective ribbon joins `10018de7`; CRT reseeding and native point-buffer aliasing retained. Artifacts22818/22860/22892. |
| 3000 ShockWave | GC vector/matrix/dynel ctors `100ee7f0`/`100ee8a6`/`100ee96b`, loader `100ede77`, init `100ee576`, process `100edfd0`, delete `100ee45e`; DS ring `10016e84`/`100172bd`, cone `1000bd0a`/`1000c0ec`. Delayed terrain-ring edges, UV flags and optional cones; records30101/30102,43100–43104/43106,45084,71226,97140. |
| 3001 Deformer | Dynel ctor `100d7ed6`, loader `100d7ce7`, process `100d7752`, vertex callback `100d7217`, graceful `100d71ea`, delete `100d7bf5`. Record31101 moving attractor spheres;31201/43764/45057 spatial/time sin² normal deformation;12276 uses real CAT42886/animation42892 collapse/restore. Sources artifacts22612/22670/22725. |
| 3024 BParticle | GC loader `1010a0f0`, visual construction `10109ebe`, process `1010a3b5`; DS ctor/process/draw `10009938`/`10008bce`/`1000a70f`, priority6 `100089f4`. Modes0–12, shared atlas-rate pingpong, pulse8 sentinel999, emissions9/11/12, geometry/size/spin, velocity/gravity, colour/width, distance alpha and terrain0x400. Assembly `10109f34`–`10109ffe` fixes width words31/33/35 versus heights32/34/36. Artifacts22594/22600. |
| 3028 BParticle2 | Vector/dynel ctors `1010bf1f`/`1010c06b`, loader/init/process `1010ac44`/`1010ba11`/`1010af47`, graceful/delete `1010aa16`/`1010c277`. DS `1000b432`/`1000b4fc`: quad/triangle/asymmetric doubling; geometry>2 retains native degenerate fallthrough. Ground0x400/0x8000/0x10000, height0x800000 fade3–6, environment0x400000 raw day seconds/3240 folded2−f with minimum0.3, body0x1000000 scale² emitter; independent RNG streams. |
| 3030 GroundGrid | Vector/dynel ctors `1010e86a`/`1010e96a`, loader/init/process `1010e66b`/`1010e3ff`/`1010e1b2`, duration `10102eb5`, graceful `100a719a`, delete `1010e7ea`. DS ctor/update/alpha/states/draw/colour `10016c68`/`100164b4`/`1001692d`/`10016961`/`10016b9e`/`10016e15`: fixed/moving terrain grid, native row strips, UV, radial/Manhattan attenuation and sine32 phase. Records71222,71230–71237,71303,71370; artifacts22594/22615. |
| 3031 TParticle2 extensions | Vector/dynel ctors `10114265`/`101143bb`, delete `101145d2`, inherited graceful `100a719a`; §7.7 retains core addresses. Terrain/atlas branches,0x100/0x80000 UV and0x40000 camera-facing alpha; angular RNG still sampled although visual ignores angle. Sources22594/22600/22919/22691/22750, shared with3028. |

Installed3024 record72340 uses mode13. DS `10009938` zeroes its state,
switches only0–12 and has no default: mode13 consumes no mode RNG and
draws no geometry, a native zero-state fallthrough rather than malformed
authored data (artifact22600). Sprite1012 selectors1/2 similarly terminate
in GC `100f5f2e`, not parser rejection (§7.3).

Native30001 regression expectations, not runtime formulas, were corrected
from RE: ShockWave30101 word22=`0.209999993` raises terrainY7 to7.21
(`100ede77`/`100edfd0`). Deformer31101 callback `100d7217` mutates
vertices sequentially with attractors as the outer loop: displacements
0.03/0.0225/0.016875 total0.069375, not a summed0.09 (artifact22670).
31201 graceful (`100d71ea`/`100d7752`) keeps native float elapsed:
after0.05 then two0.5 steps fade age is0.99999994 and remains alive;
the next0.01 ends it. No test execution is claimed by this documentation note.

Shared curve cursor follows GC `101161a1`'s declared count/pairs, while
CMS accessors `10106872`/`10106893` return zero beyond the record.
Shield71320's40-word payload declares offset count9 at28 and therefore
has an implicit zero tail, not malformed curves; compressed zero-tail
storage bounds allocation. Shield71319 retains stop child71360 at39.
Class1002 `100d52dc` asks `10105c2e` for the source dynel before cord
creation: only a nonnull result creates its dynel child. Vector/matrix/
RConnector parents retain the orbit child but do not invent a dynel cord
identity (`100d52dc`/`100d53e3`/`100d57bb` constructor export).

3028 GC `1010aa21`/`1010abc3`/`1010ac02` preserves actual source
visibility: invisible controls bypass particle emission/lifetime processing,
while base elapsed time advances, without catch-up on visibility restoration.
BodyScale starts at native1 (`1010ae67`) and updates from the actual dynel.

Runnable unit checks (not run here): `authored_global_fades`,
`authored_tint_layers_and_uv_motion`, `authored_8012_nano2_trails`,
`authored_30101_terrain_rings_and_cones`,
`authored_31101_localized_attractor_deformation`,
`authored_31201_envelope_and_callback`, `authored_12276_morph_phase`,
`authored_bparticle_modes`, `authored_bparticle_native_branch_matrix`,
`authored_bparticle2_modes_preserve_display_and_crt_streams`,
`authored_crystal_73001_terrain_modes`,
`authored_71230_radial_ground_strip_lifecycle`, `retail_groundgrid_authored_modes`.
Ignored installed-asset frame checks require `AOMAC_EFFECT_FRAMES`:
`retail_global_frames`, `authored_nano2_actor_frames`,
`retail_native30001_authored_frames`, `authored_bparticle_offscreen`,
`retail_particle_dependency_frames`, `retail_crystal_authored_modes_frames`,
`retail_groundgrid_frames`. Their existence is not an observed frame result.

#### Additional installed modes and surface controls

| Class/family | Native implementation evidence |
| --- | --- |
| 1020 NightVision1 | GC ctor `100eaddd`, loader `100ea6a1`, init `100ea813`, process `100ea652`, duration `100ea694`, graceful `100a719a`, destructor/fog restore `100ea5c5`, scalar delete `100eae3b`. Up to three viewport layers with independent blend/repeat; DS priority7 `10023885`, fullscreen strip `10023929`. Optional distortion DS `10011597`/`1001165a` calls empty `10007a2f`: native no draw, not missing geometry. Nine records3400/3401/3410/3411/3422/3423/3430/43652/43733. |
| 2001 Spiral | GC vector/dynel ctors `100f4ab6`/`100f4e53`, loader/init/process `100f499c`/`100f49f0`/`100f4760`, graceful/delete `100f488e`/`100f4f87`; DS `10021eb4`/`10021924`: two twelve-segment strips, native sweep/radius/pitch/UV, independent Y rotation, endpoint clipping. Records11200/43010/43011. |
| 2002 Plasma | Hit-location-only ctor `100ec059` via `100d145c`, loader `100ebf73` (duration18 overrides8), init/process `100ebfcd`/`100ebd91`, hit endpoints `10104fa8`/`10104fde`, graceful/delete `100ebe66`/`100ec194`; DS `1001b8f7`/`1001bbf2`:75 segments, four sine-cubed waves and CRT phase perturbation, two-sided camera ribbon. Records11201/17500/17600/17912–17914; located creation of17600 is native null. Sources22557/22571/22592/22660/22744/22757. |
| 2011 Highlight extensions | GC ctor/load/process/delete `100e2e10`/`100e286a`/`100e29a7`/`100e2bde`: modes1/3 use1−(2t/duration−1)²; mode3 excludes the CAT root and selects only VisualAttractorMesh place0 (head), including alpha/emissive/specular. GC `100e29a7` tests `VisualAttractorMesh+4 == 0`; DS `1007347e` (`GetAttractorMesh`) compares that field to `AttractorPlace_e`, `10071cce` (`AddAttractorMesh`) sorts by it, and `10071ca2` (`GetName`) maps zero to `Attractor01_head`. The previous place≠0 weapon predicate was reversed and is corrected. Records11507/11508 now use an actual mounted head40629 fixture (Solitus female, weapon7796 unchanged); zero ambient/sun isolates its material envelope, not retail lighting equivalence. Flag0x400 in61110/61112 sets root priority−1 (`100e292b`) and restores on deletion. Word11 suppresses restoration, but all installed records have0. |
| 3015 LavaBall | GC ctor/load/init/process/graceful/delete `100e435c`/`100e4031`/`100e413e`/`100e3005`/`100e3f04`/`100e4cd9`; DS `10010d70`/`1001105e` and render bucket100 `100078c8` (not near clipping). Mode0 ballistic terrain/collision/audio, mode1 camera arc, other modes fixed camera hemisphere; two128-slot DiaBill pools, CRT wait/azimuth,64-frame atlas, per-frame fire velocity×0.8 and30-impact cap. Records12300/12301/12560/12580; artifacts22826/22935. |
| 3018 SkyRise | GC vector/dynel ctors `100efd12`/`100efe2c`, loader/init/process `100efb10`/`100ef7a5`/`100ef462`; DS ctor/hemisphere/two-pass strips/flat height-colour/endpoints `100203c9`/`1001ff05`/`1002016d`/`1001f39d`/`1001fb2b`. Record12520 mode0;12521 mode1 flags5 radius+pulse;12522 mode1 flags3 radius+colour. |
| 3019 Trail | GC ctors `101020db`/`101021ad`/`10102301`/`10102455`/`1010252d`, loader/init/process `10101f88`/`10102010`/`10101ddb`; graceful `10101e67` only writes field0x40. DS `1002b486`:50ms sampling,1000-slot ring,50 points, spatial2.5;12570 absent mode defaults0,71000/71003 mode1 four oriented strips. |
| 3020 TParticle extensions | GC init/load/process `101125fd`/`1011277c`/`10112bb0`, DS `10029de9`/`10029c81`/`1002a350`: mode1 static box endpoints consumes three DS RNG samples; mode2 ring radius12, Y=random×600 and tangent+2 consumes two; mode3 zero endpoints/no rendering/no samples. Constants DS `1008b898`=600, `1008a128`=2; terrain0x400 snaps initial position only. Sources22594/22600. |
| 3021 Buffer | RConnector-only ctor `1010c32c`, process `1010c296` uses live connector; record71012 has empty payload. DS `1000bc0b` invokes viewport depth-only clear; Randy `1004b75a` confirms D3DCLEAR_ZBUFFER2 with target=false. Ordered attachment clear preserves colour and resets depth1.0. |
| 3023 Energy | GC ctors `1010dfa5`/`1010e01a`/`1010e08f`/`1010e107`, loader/init/process `1010d937`/`1010d7ed`/`1010dae7`; DS `100123c5`/`10012264`/`10011e31` draws three rotated quads per angular step; installed modes0/2. |
| 3027 MParticle | GC ctors `1010fea9`/`1010ff76`/`10110046`/`10110119`, loader/init/process `1010f09d`/`1010fcd8`/`1010f3da`, selector `1010ef6f`, embedded mesh attributes `1010eeef`: one-shot0/continuous1, gravity/bounce/fade/scale/spin and CRT mesh choice. |
| 3029 Scatter | GC loader/init/process/finish `101107b5`/`10110864`/`10110357`/`10110771`: mode1 is random XYZ×word9 plus progressive native Z word8/count, not a polar ring; mode0 random XYZ×word8. Terrain0x1000 precedes authored offset;0x2000 preserves source creation form, otherwise vector. Progress does not reset on repeat; graceful forwards to children and clears repeat0x400; delete destroys children. Records71305/71306/71310. |
| 3034 Shield2 extensions | GC loader/init/process/ctor `10110f3a`/`10111326`/`10110bbf`/`101114fc`, CAT/mech callbacks `10111147`/`10110dc3`. Word20 selects actual EP03_*_simple.abiff0–9, not *_statel; CAT uses posed surface/source material under0x10000. DS `1001d569` provides cylindrical/source UV and sin² normal displacement. CAT Stop under0x20000 hides and emits actual child71319→71360; graceful is separately two seconds. Flag0x8000 priority3. |
| 3038 VolGrid /3039 Trail2 | Grid GC loader/init/process `10115a3b`/`10115964`/`10115d41`, DS `1002f86d`/`1002f69e`: actual plane counts and five curves; words10–12 loaded but inert. Trail GC load/process `10114c39`/`101150b4`, DS `1002c918`: actual2006/2007 connector, fixed sample ring/cadence/four strips/thrust/fluctuation, immediate graceful finish. |

Surface sources: artifacts22617/22621/22646 and
`/tmp/FxClasses/surface-doc.txt`. Nano2 clarification: geometry consumes792
DS R250 samples per process (XOR lag147/ring250, DS `100844df`);
the CRT srand42/time calls are separate side effects, not geometry seeds.

Additional runnable checks, not run here:
`authored_nightvision_layers_and_native_lifetime`,
`authored_43010_43011_spiral_lifecycle`, `authored_17500_plasma_hit_lifetime_and_rng`,
`authored_11507_head_specular_has_parabolic_envelope`,
`authored_71320_mech_cylindrical_uv_and_71319_stop_child`,
`authored_modes_preserve_native_rng_and_motion`, `ground_flag_snaps_only_initial_position`,
`all_authored_class3020_records`, `authored_lavaball_lifecycle_and_atlas`,
`ground_collision_caps_native_impact_emission`,
`authored_legacy302x_geometry_and_lifecycle`,
`authored_12570_trail_missing_mode_defaults_to_zero`,
`authored_71250_mesh_particle_reset_preserves_random_order`.
Installed frame checks: `retail_nightvision_frames`, `retail_native2001_frames`,
`retail_native2002_hit_frames`, `retail_highlight_head_authored_frames`,
`retail_surface_authored_modes_frames`, `retail_class3020_frames`,
`all_authored_lavaball_frames`; no frame outcome is inferred from these names.

`retail_class3020_frames` now enumerates every installed class3020 record,
including the newly supported endpoint modes, with a terrain callback.
`retail_surface_authored_modes_frames` also enumerates class3036 Toggle:
its harness supplies an authored-list-compatible playfield/mask context and
a moving-to-stopped source transition, without rewriting templates or children.
These additions have not been run or visually inspected.

Contact inspection of snapshot7816a52 found blank Cylinder2601 captures:
`retail_effect_frames` requested Vector although class1013 admits only
hit-location/tracer creation. The fixture now supplies its existing hit endpoints
through HitLocation. The legacy301x capture fixture now includes private buff
and mesh resources, matching the legacy302x resource list.
Trail12570/71000/71003 were blank despite moving sources: `Trail::vertices`
returned variable-length strips (at most102 vertices) against104-vertex models,
so `ao-render/src/actors.rs`'s exact skin-length gate discarded their skins.
Each strip now retains104 vertices with transparent degenerate tail padding;
the existing12570 regression asserts model/skin length before and after motion.
These corrections have not been built, tested or recaptured here.

The same fixed-camera limitation affected class3020/3024 inspection.
Installed3020 records71033–71038 use mode1 endpoints in a ±130-unit cube;
97100/97105 use ±5 endpoints and widths1.5→3→5 (their ±50 velocity words
are inert in mode1). Record71104 mode2 occupies native Y0–600, whereas
71105 mode3 is intentionally nonvisual. The3020 fixture now frames authored
endpoint/emission, movement, trail and width bounds; mode2 also retains a
close-up framed from the first actual particle's vertices.
Installed3024 pulse71352 has half-width/height175;71115 has150 and
71225/71228 have40, all additive and viewed previously from distance√33.
The3024 fixture now frames authored width/height, geometry and motion bounds,
but preserves any valid authored distance-visibility interval instead of
moving beyond it. No native size/colour/geometry was changed; new captures
remain unrun/uninspected by this worker.

Crystal frame inspection of `crystal3031_0.png` exposed a harness camera
inside the authored geometry: records71558/72365 have width words27/28=4/8,
aspect33=0.5 (tail half-width8–16), growth30≈1.01 per process, versus the
old camera distance√33≈5.74. Their flags0x203 select additive blending;
overlapping near-camera strips saturated the sampled view. The harness now
frames conservative authored emission/velocity/acceleration/trail/width bounds
using `ao_render::default_view`'s sphere-framing factor2.4, without changing
particle size, colour or native update logic. Updated frames are not yet inspected.

#### Legacy geometry, mesh and audio variants

| Class/family | Native implementation evidence |
| --- | --- |
| 1023 Nano3 | Record8020, GC vector ctor `100e9d86` through dynel `100ea146`, loader/init/process `100e90d7`/`100e9ad7`/`100e9855`, emission `100e92a9`/`100e9583`: Sprite2 hundred-particle burst initialized before setters, mode0, alpha-blend0x800. |
| 1028 Explosion | Record4000 vector ctor `100e564b`, loader/init/process `100e528c`/`100e54ad`/`100e52cf`; DS rock list `1001c435`:32 rocks, selectors(rand&7)+11, R250 velocity×15, spin8, gravity9.8, bounce0.5 and stop below0.5 speed; ctor overrides duration to−1. |
| 2008 Cone | Record15400 vector/dynel ctors `100e2781`/`100e26ed`, loader/init/process `100e1ed9`/`100e2590`/`100e2069`; DS Cone `1000bd0a`: six staggered rings×four cones, actual terrain, packed growth/fade, radius1.4/spin. |
| 2009 Drip | Record25318 vector/matrix/dynel ctors `100d8c5b`/`100d8d6d`/`100d8f04`, loader/init/process `100d8a2f`/`100d8aec`/`100d8661`; DS DiaBill `1001105e`: eight drops grow/fall/ground/shrink, acceleration0.01 per process, not delta-scaled. Graceful `100d8902` writes an unread field; expiry extends five seconds while visual exists. Sources22818/22860/22871/22902/22996. |
| 3012 GroundRing | Vector/matrix/dynel ctors `100e1d53`/`100e1dc2`/`100e1e31`, loader/init/process/delete `100e1ba1`/`100e19cc`/`100e1729`/`100e1ce7`; DS `10016e84` ring strip, `10017051` SRCALPHA/ONE. Flag0x800 samples terrain only initially, otherwise per frame;0x1000 preserves locator loss. Material100000 is native null (`10106f39`), not a missing-texture substitute. |
| 3013 Delay | Vector/dynel ctors `100d851e`/`100d857d`, loader/process/delete `100d8499`/`100d830b`/`100d82c5`, graceful `100d83ba` forwards only active child; duration `100d83d7` child duration+15. CRT15-bit/32768 delay, strict<0 spawn, vector child with captured dynel locator; setters forward only after spawn. |
| 3014 Vein | GC ctors `10102cde`/`10102d53`/`10102dc8`/`10102e40`, loader/init/process/delete `10102ad0`/`101028c4`/`10102731`/`10102c39`; graceful `100a719a` immediate, duration `10102eb5` lifetime only. DS ctor/draw/geometry/second pass `1002e885`/`1002e6d1`/`1002def9`/`1002d79f`: nine animated controls, clamped order4 cubic basis (`10004328`/`100041ff`/`10004373`/`100043c3`), not linear interpolation. Cumulative growth `1002dd59`, packed curves `1002db0e`/`1002dc61`, native facing/log-distance alpha, V-scroll−3 `1016d1d0`, owned children28/29. Records12200–12203/12206,96002–96005; artifacts22826/22997/23001/23023/23043. |
| 3016 Bubble | GC vector/matrix/dynel ctors `100d48d4`/`100d49fd`/`100d4bab`, loader/init/process/delete/graceful `100d46a0`/`100d476c`/`100d41f9`/`100d4cd7`/`100d4573`:25 slots, per-frame jitter, interval22/burst24, optional mode25 zero default, mode1 burst2 versus5; age atlas trunc(age×16)&7 mirrored, lifetime21 overrides8 plus5-second drain. DS `1001105e` uses2×2 UV. Sources22826/22861/23045. |
| 3033 Spiral2 | GC vector/matrix/dynel/connector ctors `10111cec`/`10111d54`/`10111dbc`/`10111e7f`, loader/init/process/graceful/duration/delete `1011194c`/`10111a8a`/`101116ef`/`10111824`/`1011193f`/`10111bfc`; DS ctor/states/draw `1002270b`/`10022256`/`10022407`/`10022687`. Authored strip/segment/spin/acceleration/curves, random999 angle, one-second graceful fade/radius+1; dynel factor uses raw CAT sphere (`10072c22`/`1007494b`) divided by0.25075000524520874 times CAT scale. |
| 3035 AParticle | GC loader/init/process/delete `1010829c`/`10108a48`/`101084f5`/`1010847a`, duration `10102eb5`: actual camera-centred wrapping cube, reseed, mode1/2 oscillation, per-particle size/frame/spin/frequency/amplitude, distance colour and atlas loop/clamp/pingpong; DS BParticle2 visual. Environment+0x90/+0x94 are oscillator channels, not weather wind. Camera source confirmed by GC `100b19e4`→`100b005c`, n3Camera+0x1ac–0x1b4. |
| 3036 Toggle | GC loader/init/process/graceful/delete `10112109`/`101121c0`/`10112401`/`10111f72`/`10111f06`: actual TILEMAP resource+0x1c include/exclude0x800 and mask+0x50, unlocated override0x1000, movement edges0x2000/0x8000 (`1011202f`); owned child lifetime/termination/deletion. |
| 3025 mesh variants | GC `1010c825` defaults, `1010c477` visibility, `1010c526` attractors, `1010c94a` process; oscillator assembly `1010cb4e`. Flags0x200 body scale,0x100 terrain,0x400 camera facing,0x800 first-person/distance opacity,0x2000 vehicle gain,0x4000 initial gain0,0x8000 breed4 scale1.45,0x10000 authored Atrox names. DS `1006ce7d`→`1006c61d`, callback `1006c005`/exit `1006b795`: Holo textures28022/28023 with local elapsed UV, lit/unlit/fog/depth-only clear/additive/z-bias states; no repeated mode6 draws. Flag0x1000 uses actual ABIFF animation as described below. |
| 4000 Audio extensions | GC `100d3190`/`100d30c1`/`100d3012` forwards all nine AFCM0x103 words once. SI `100071ed` radius ROUND(r−0.49999), `10002d98` playback, `100025fc` duration loops, `10003b70` hold/fade expiry; delay `10003f61` restarts only first due entry per frame, resetting probability100/radius0/velocity0. Definition duration inheritance `10003520`; velocity affects Doppler, not position integration. Sequential children start index1/advance once per frame; random child avoids last after frame. |

Exact authored boundary corrections: GroundRing60005 begins U at
`15/16=0.9375` and ends at15 (DS `10016e84`, artifact22725);
Bubble12400 emits only when elapsed is strictly greater than its
word22 interval `0.899999976`, not0.81 (GC `100d41f9`, artifact22826).

Mesh-animation0x1000 now uses the existing ABIFF `NodeRig` through
`load_animated_mesh`/`pose_parts`, looping at authored total_time, with actual
sampled object-space node matrices and per-part CPU poses. Initial velocity
and acceleration transform once through the source matrix
(`1010cf05`→`1010a9a0`). Mesh deletion `1010d759`→`1010c895`,
position/matrix updates `1010c508`/`1010c517` and immediate graceful
`100a719a` retain their separate native paths. Audio graceful `100d30b4`
is empty: cancel preserves still-undispatched4000 audio; source setters
`100d3085`/`100d309b` update its pending position.

Mesh contexts use actual camera/source/body/vehicle values. Dynel-locator mesh
controls terminate on deleted/missing dynels; non-dynel constructors retain
native defaults (scale1/breed0/no vehicle/direction0/visible). No fake actor
is supplied. Actor priority−1 suppression precedes alpha priority6 override
(Randy `1004cc35`); native render-list ordering is `1004c9c2`/`1004bfff`,
DS `100796d9`. These rendering contracts are described in dynels.md.

Further checks, not exercised here:
`authored_25318_drip_four_state_lifecycle`, `installed_legacy_authored_record_shapes`,
`authored_60005_groundring_terrain_uv_envelope`,
`authored_61101_groundring_null_material_frozen_terrain`,
`authored_12400_bubble_emission_atlas_and_drain`,
`authored_100385_bubble_mode_one_and_optional_mode_zero`,
`authored_62010_delay_random_interval_and_capture`,
`authored_12201_vein_cubic_periodicity_and_two_passes`,
`authored_legacy303x_records_and_lifecycle`,
`native_oscillation_and_vehicle_camera_gain`,
`authored_class4000_sequence_and_delay_contract`.
Ignored frame/PCM checks require `AOMAC_EFFECT_FRAMES`:
`retail_legacy_sprite_authored_frames`, `retail_legacy301x_authored_frames`,
`retail_vein3014_frames`, `retail_legacy302x_authored_frames`,
`retail_legacy303x_frames`, `all_authored_mesh_modes_frames`,
`authored_native_tracer_mesh_frames`, `authored_class4000_sustained_audio_frames`.

#### GlobalSmoke, Beam and world controls

| Class/family | Native implementation evidence |
| --- | --- |
| 3017 GlobalSmoke | GC resource/dynel ctors `100e066f`/`100e0835`, loader/init/process/graceful/delete `100e00ba`/`100e01e3`/`100df6e3`/`100dff8d`/`100e0b52`; DS ctor/draw `1002061d`/`10020836`, priority6, SRCALPHA/(mode5 INVSRCALPHA else ONE).64 sprites; all modes0–6, including92001 mode6 negative-up; mode1 terrain-steered five-sample trail, mode2 angled gravity, mode3 directional/80-unit camera-distance gate, mode4 gravity, mode5 authored rate/divisor. Graceful extends duration five seconds once native base expires. |
| 3022 Beam | GC vector/matrix/dynel ctors `10109c7a`/`10109cf5`/`10109d70`, loader/init/process/graceful/delete `101092b7`/`10109199`/`101096b7`/`1010916d`/`1010953f`; DS ctor/states/vertices/draw `10007ef4`/`10007d2b`/`10008056`/`100087c1`. Authored crossed-diameter frustum facets from45° at180/n increments, optional cap39/height40, rise/hold/fall radius/ARGB, jitter/rotation, terrain/tracking, mode0 gravity−800, repeat delay41, atlas/UV/view-normal/horizontal flags. Graceful caps remaining/fall to three seconds. |
| 2012 Fence | GC ctor `100db6f8`, singleton `102e9888`, loader/init/process/graceful `100dac76`/`100db3d4`/`100dadcd`/`100dab49`: actual type1000021 playfield polygons and destination flags&0x8000ffff, incremental nearest-edge squared distance,64 spark pool. |
| 2014 Font | GC ctor/load/init/process/graceful `100df42d`/`100decc3`/`100dedc8`/`100deb04`/`100deba5`, SetText `100dee29`, metrics `102c4608`, mapping `100dede6`; DS DiaBill `1001105e`, priority7/material40; actual record12122 uses world text, not a GUI label substitute. |
| 2015 Notum | GC vector/connector/dynel ctors `100eb368`/`100eb42d`/`100eb4a8`, loader/init/process/graceful `100eaeb9`/`100eb20e`/`100eaee9`/`100eaea2`. Empty payloads12120/12121/12123 retain native zero CMS fields; initial offset trunc(X×0.25) (`100eb227`–`100eb24c`), actual GameTime mod270 gives three-second cone pulse. DS `1000bd0a`/`1000c0ec`, eighteen strip vertices, material13/priority6/additive; graceful no-op is native. Sources22812/22862, installed table23207. |

Unexercised checks: `authored_global_smoke_gravity_slots_and_graceful_extension`,
`authored_beam_facets_cap_and_native_fade_termination`,
`relative_smoke_births_follow_attractor_existing_particles_follow_root`,
`installed_all_global_smoke_and_beam_modes`,
`native_font_metrics_spacing_and_lifecycle`,
`authored_notum_empty_payload_and_position_clock`,
`native_boundary_destination_flags_and_truncation`.
Ignored `authored_native3017_3022_frames` enumerates all51 installed templates;
`retail_legacy_buffs_all_authored_frames` covers the world-control records.
Both require `AOMAC_EFFECT_FRAMES`; neither was run by its implementation worker.

Fence check `authored_11600_fence_pool_and_graceful_drain` uses actual11600
words and playfield566's424-byte polygon record (one polygon,25 points);
it was added but not run by its worker.

#### Remaining native mesh and specialized factories

| Class/family | Native implementation evidence |
| --- | --- |
| 3002 Splash | GC vector/dynel ctors `100f5707`/`100f587e`, loader/liquid init/process/delete `100f5081`/`100f5522`/`100f522e`/`100f5a05`: staggered parabolic cone strips; liquid kind2 or source above liquid suppresses output. All six34101–34103/34201–34203 records have unused duration word8=0xffffffff (NaN), not a substituted finite timer. |
| 3005 WaterRipples | GC ctors `10103d0a`–`10103fe4`, loader/init/emission/process/graceful `10103c1c`/`10103bd4`/`10104162`/`101042de`/`10103bcc`; DS `100307b0`/`1003049f`/`100303d9`: actual liquid depth>0.01 and flags&0x1e≠2, speed-dependent minimum emission delay, expanding annular strips; record33000. |
| 3008 Shadow | GC ctor/init/process/callback `100ece3a`/`100ecdb5`/`100eccc9`/`100ecee4`/`100ecb6b`; DS `1001f0e9`/`1001e556`/`1001e75d`/`1001e90e`: CAT every-fourth-vertex bounds, adaptive≤300 segments,50-unit camera cutoff, outdoor0x5f000000/dungeon0x2f000000 soft ellipse, actual terrain warp+0.02. Actual sun/collision projection is integrated from assembly `100ed0b1`–`100ed4a1` and terrain helper `100ed5a9`, not a guessed projection. |
| 3009 CrazyCone | GC vector/matrix/dynel ctors `100d6fbf`/`100d703c`/`100d70b9`, loader/init/process/delete `100d6517`/`100d6e5a`/`100d6874`/`100d7165`:29-word staged accelerated cone geometry/UV and byte-colour pulse; eleven records. |
| 3010 Mesh | GC vector/matrix/dynel ctors `100e50e2`/`100e5151`/`100e51c0`, loader/init/process/delete `100e4fad`/`100e4e96`/`100e4d9e`/`100e5232`: four actual tower_destroyed_* ABIFF selectors, rise/hold/fall transparency, Y rotation and terrain0x100; five records. Active `TowerMesh` dispatch caches/uploads authored resources, retains source/position and tracked-root setters, source-loss0x200 and actual terrain callback without generic-mesh substitution. Sources22865/23093. Added, not run here: `authored_tower_dispatch_resources_and_cached_root`, `native_tower_source_terrain_and_lifecycle`. |
| 2013 Projectile | Only hit-location factory `100d145c`→ctor `100ec41a`, vtable `1016c824`, loader/init/process/graceful/delete `100ec307`/`100ec363`/`100ec273`/`100ec2ec`/`100ec218`→`100ec559`. Actual2693/2694 use arrow_short/arrow_long.abiff (names `102c4c20`), speed7; motion=start+delta×speed×elapsed/max(distance,1), expires strictly fraction>1. Base `100d2531` starts elapsed0, second delta caps0.033; no fabricated source/target for absent hit location. |
| 5000 GroundImpact | Only RConnector factory `100cf872`→ctor `100e103e`, vtable `1016bffc`, loader/init/process/delete `100e0f3b`/`100e0fe1`/`100e0ec0`/`100e0f8f`→`100e12db`. Record90000's four floats are read then discarded natively. ForceSword setup `100e11e0`, DS init/draw `10015730`/`10015f21`: CRT%10+5 cross sections/five trails/jitter/history, actual material1 circle.png, priority6/additive/no fog/depth-write/cull. Connector validity/visibility remains live input; graceful base `100a719a` marks termination. Sources22586/22622. |

Shadow uses real VisualEnvFX sun direction, twenty-metre upper/lower rays
and native fallback ray selection; discrepancy>0.2 disables terrain warp.
Dungeon uses−Y/direct ground plane without rays; degenerate projection
normal yields no shadow. WaterRipples' unused light setter `10030917` has
no GC caller: constructor `100307fd`–`1003080d` zeroes light direction,
and palette `100303d9` retains native0x7e7e7e, not invented lighting.
Additional unexercised unit check `native_shadow_ray_selection_and_dungeon_projection`;
ignored `authored_legacy300x_surface_frames` covers3002/3005/3008/3009
including outdoor/dungeon shadows and requires `AOMAC_EFFECT_FRAMES`.


Special effects preserve hit registry GC `100cda79`/`101056b4`,
ctor `10104dca`, start/end `10104fa8`/`10104fde`: misses lock target position
minus nativeE2×10 (`1015f168`), connector override `101050a4`/`101050ba`,
locator invalidation `1010603b` freezes the last position, and retention
`10105621` uses strict10,000-clock-unit expiry excluding the newest entry.
Weapon groups1/2 in `1009ad7d` use NewHitLocation/integer factory, while
GroundImpact's stack/start list is `100d1ecf`.

Unexercised checks: `authored_legacy300x_records`, `native_mesh_resource_names`,
`native_mesh_alpha_envelope`, `authored_ground_impact_90000_uses_native_generated_mesh`,
`authored_special_factory_payloads`. Ignored `authored_legacy300x_tower_frames`
and `authored_special_effect_frames` require `AOMAC_EFFECT_FRAMES`;
the latter uses actual2693/2694/90000. No visual equivalence is claimed here.

Runtime environment inputs are also native-derived: GC `100b005c`/`100b061b`
supplies GameTime `1000b44c` divided by constructor `1000af71`'s15 to DS
`10062a66`, so3028 receives raw day time0–6480, not normalized weather.
BodyScale is N3 `10019622`'s dynel+0xac, not inferred matrix length.
3035 oscillator channels use GC `100b0125`–`100b01b3`, phase+=delta×50
mod360,90°/180° offsets and native3.140000104904175/180 cosine factor.
3036 resource TILEMAP/flags come from N3 blob reader `1001c115`, not zone
instance. Sources22987/23022/23112/23188. Added, not run:
`effect_inputs_retain_native_vehicle_direction_and_speed`,
`environment_effect_inputs_follow_native_oscillator_not_weather`,
`native_effect_resource_keeps_tilemap_and_flags`.

Hit-registry clock is native truncated frame-delta×1000 milliseconds:
GC `100d2202`–`100d2216`, double `10157870`=1000, then prune
`10105621`; the strict retention boundary above is therefore ten seconds.
Missing identities permanently change locator mode to0 but retain cached
actual position; DeleteEffect does not delete registry entries. Additional
unexercised checks:
`native_miss_locks_target_forward_and_shared_connector_overrides`,
`native_hits_track_and_missing_dynel_freezes_locator_until_registry_expiry`,
`native_miss_zero_delta_does_not_lock_and_retention_boundary_is_strict`.

#### Shared hit-meta, authored mesh children and fog

Class2010 uses actual hit-location Meta, not a dynel fallback: ctor
`100e5c3f`, shared process/load/delete `100e57f3`/`100e59b2`/`100e5799`.
Words10/11 override the original shared hit locators via `101050a4`/
`101050ba`, then `100e5cdc` creates all ten children through the integer
factory with that same handle. Records17950–17953 reference actual17000/
17600/17912–17914 and3001/1006/1003; wrapper has no geometry.
Graceful forwards to children, deletion cancels all, NextState is no-op,
and duration override adds15. Check `authored_hit_meta_children_and_locator_overrides`
and ignored `authored_hit_meta_frames` were added, not run.

Authored ABIFF `eff_` children use DS name parser `1006d49a`, tables
`100af3d0`/`100af450` and effect-data32-byte getter `1006bb1a`/
`1006bb02`. GC `1010eeef` attaches actual metadata; N3 `10007fe5`
starts with handle0, enable `1000806d`→GC `100cdb89` creates through
RConnector or enables retained handle, disable `10008085`→`100cdc3d`
retains processing, destructor `10008021` deletes. No constructor-only
child substitutes the authored enable lifecycle. Checks
`native_mesh_effect_name_table`, `native_mesh_children_start_disabled_without_handles`,
ignored `installed_effect_connector_metadata`/`authored_mesh_effect_children_frames`
are unexercised here. Animated UV consumer Randy `1004d84a` applies
scale/offset to stages0/1; DS ctor/evaluator `100295ab`/`10028fde`;
renderer per-part UV matches that formula without copying vertex UVs.

Native fog is shared logical state with capture/process/destructor restore,
not an OR of active effect flags: DS GetFogMode `10058011`, NoFog
`10058015`, SetFogMode `10058409` (input0 sets logical3/weight0.1),
process `10058443`; Randy `10041876` re-enables linear fog and ignores
the mode booleans. NightVision process `100ea652` reasserts mode0;
VisionTint `10115394` reasserts NoFog while alive/visible. Deletes
`100ea5c5`/`10115311` restore their last captured mode, not preferences.
New `native_overlap_preference_and_destructor_order` and ignored
`native_fog_mode_frames` were not run. Native ordering is now traced:
GC playfield `10016e2c` appends EffectHandler before Environment;
N3 `1000d02b`→`10007df4`→`1002999e` appends children, and
`10007e07` runs them in that order. Environment `100b19e4`→
`100b8ad7`→`100be767` runs DS fog `10058443` after effect processing,
before EXE `0040383a`'s DS render. Normal initialized indoor/outdoor
weather paths re-enable hardware fog; no ordinary missing-record
exception is asserted. Source artifact23398 and native child-order exports.

Caller fallback also preserves native null versus allocated terminated
control: CF26/CFD4 GC `100a5083`/`100a8c03` try unlocated→dynel→
hit-location only on zero, not on an error or a nonzero terminated handle.
Their final hit-location uses source application dynel/controlled caster,
attractors3001/1006 and hit=true; CF57 remains its separate dynel path.
`spell_visual_fallback_only_retries_native_null` was added, not run.
An absent visual does not universally imply factory null: Highlight2011
waits, Stars2004 marks done;1023/2009 dynel ctors unconditionally
initialize their visual after locator failure, and2009 can extend its drain.
No synthetic actor position is supplied to hide those distinctions.

GroundGrid native clamp flag0x10000 uses DS `10016961` texture-stage
ADDRESSU/V/W=3; default1 is wrap. The renderer retains actual sampler
state rather than CPU-clamping UVs. Checks
`native_texture_clamp_uses_distinct_cached_sampler_materials` and
`authored_groundgrid_clamp_material_state` were added, not run.
Notum time is actual GameTime `100050a6` seconds in its27-hour day,
from server clock×15, absent until a GameTime message; it is not epoch
or effect elapsed time. The extended `game_time_sets_the_sky_clock`
check was not run by its implementation worker.

## 8. Not found / open

### Observed verification boundary (2026-10-07)

The final all-class `retail_authored_effect_census` and final integrated
workspace gate have **not run**. Consequently there is no observed final
per-class record/mode/reachability table to publish: the former rough
unsupported-class and malformed-record counts are not retained as current
coverage. The runnable census prints every installed gfxtweak record,
all fifteen creation forms, native null versus construction/configuration
failure, source bindings, deferred/impact/profile child reachability and
native-rejected versus unresolved malformed source records:
`cargo test --release -p aomac retail_authored_effect_census -- --ignored --nocapture`.
Its synthetic constructor fixtures are not live-frame coverage.

#### Native creation modes and unsupported boundaries

The creation-mode matrix in §7.9 is the exact native factory eligibility
table (`effects_dispatch.rs`, GC `100ce3be`–`100d145c`), not an assertion
that every mode drew a verified frame. The final census probes all fifteen
forms independently; modes outside that matrix must return native null.

| Class / mode boundary | Concrete reason | Coverage interpretation |
| --- | --- | --- |
| 0, all fifteen forms | Body-profile data; no native factory constructor. | Authored data can be reachable without a drawable control; not an unsupported port class. |
| 2003, all fifteen forms | No constructor case in any native factory. | Native null, not a missing implementation. |
| 1016, Unlocated | Native case exists, but no installed gfxtweak template and no renderer implementation. | Unsupported, asset-unexercisable class; no authored configuration can be surveyed. |
| 2000, Vector | Native case exists, but no installed gfxtweak template and no renderer implementation. | Unsupported, asset-unexercisable class; no authored configuration can be surveyed. |
| 3011, Vector / HitLocation / Dynel | Native cases exist, but no installed gfxtweak template and no renderer implementation. | Unsupported, asset-unexercisable class; no authored configuration can be surveyed. |
| Any class in a form absent from §7.9's matrix | Native factory has no case for that class/form. | Native null, not a failed supported creation. |

An installed class marked `supported=true` in the census means renderer
dispatch exists. `constructed` means its synthetic constructor succeeded;
neither flag certifies runtime animation, GPU output or live combat.

Short-template handling follows native CMS access semantics: absent scalar
words/floats are zero, consistently in constructor, frame and configuration
paths rather than guarded reads followed by unchecked indexing. This includes
TParticle/sprite model, flags and colour inputs and LavaBall's wait value.
Explicit packed-array bounds and nonfinite-value rejection remain errors.
The new short-record regressions await the parent's integrated gate; this
source correction is not yet a reported verification result.

Further native frame corrections distinguish absent textures from absent
geometry: GC `10106e2e` creates a material even when `the_wave.png` is
unavailable, retaining geometry with a null texture. BParticle3024
effect72219's zero interval is legal: GC `1010af47` tests `>=0` and emits
its quota once per process call. The source/anchor harness now supplies
real birth-pose mesh inputs before native3000/3001 creation, including
71226's missing-root fixture. Their regressions await the parent's gate.
The Vein3014 fixture likewise now provides required terrain for12203/12206
flag0x4000 and fits the camera to the authored cubic-control hull and native
logarithmic-distance growth. This repairs cropped/saturated test captures,
not the native geometry or alpha; `vein_fixture_camera_contains_authored_geometry`
and `retail_vein3014_frames` await the central gate.

Concrete installed-art gap (not an unsupported class): TowerMesh3010
effect61042 selects nameID201713, `tower_destroyed_buff&debuff_LL.abiff`,
whose installed payload is absent; sibling selector IDs201710/201714/
201717 have payloads (observed counts1/0/1/1). GC `100e4e96` allocates
VisualMesh before DS `SetMesh` `1006b623` / `AsyncMesh` `1007125c`:
the missing asset preserves an allocated nondrawing control and its
native timer, rather than factory failure or substitute geometry.

Projectile2013 effect2693's `arrow_short` record27728 is likewise absent,
but its native path is synchronous: GC `100ec363` asks ResourceManager
GetSync; a null result leaves clone+0x48=0. Process `100ec273` advances
base state but performs movement/distance expiry only with that clone,
so the missing arrow retains a stationary nondrawing base control.
TracerMesh3025 effect71123's `EP03_mech_heal_effect.abiff` is missing
from the name directory, not merely missing a named record's payload.
GC `1010cf05` sets done byte+0x14 and allocates no VisualMesh for an
unresolved name. A resolved name with absent payload instead retains the
allocated VisualMesh/void SetMesh, pending `1010c526` callback and timer.
These are distinct native lifecycles, not interchangeable asset fallbacks.
MParticle `1010fcd8` allocates every VisualMesh0xc0 before SetMesh/disable;
a resolved name with absent payload registers `1010eeef` callback rather
than deleting slots, and `1010f3da` processes the visual pointer.
Its missing-resource regression is not an authored3027 census gap.
Malformed present resources still propagate errors. GroundImpact
`100e11e0` builds ForceSword geometry, not an absent mesh fallback.
Added, not run by implementation workers:
`installed_2693_missing_arrow_retains_stationary_control`,
`installed_71123_missing_heal_terminates_control`,
`installed_mparticle_missing_heal_keeps_slots_and_lifetime`.

Executable provenance matters: the available
`/tmp/FxClasses/target/release/deps/aomac-193ed793dfd6d465` lists779 tests
and lacks `retail_authored_effect_census` (listing artifact23504).
Its older `retail_effect_census` was exercised against installed assets
(artifact23507), but its parser/support mapping predates this integration;
neither its failures nor its supported flags are final coverage facts.
`aomac-102fbe89827e5220` is a CLI executable, not a libtest binary.
After the parent integration gate rebuild, run
`CARGO_TARGET_DIR=/tmp/FxClasses/target cargo test --release -p aomac retail_authored_effect_census -- --ignored --nocapture`.
The `coverage:` rows count distinct authored IDs per class and distinct
IDs reachable in the union of weapon event10, nano visuals, all item-effect
spells and persistent stat413 roots, transitively including deferred and
destructor children. A class's configuration failures count distinct IDs,
not failed creation forms. Missing49999 is the native runtime meta sentinel,
not missing artwork. These inventory counts do not certify frames, live
dispatch or the workspace gates.

Separately, the parent's isolated class1029 gate reports workspace
**1381 passing tests** (artifact23317), strict clippy (artifact23124),
and all44 generated frames inspected (artifact23134); its first-class
implementation was pushed as `48cc58d`. These are isolated1029 evidence,
not a final all-class gate or retail/live equivalence claim. Live effect45083
was **not verified**: the window closed without login. §7.8's native addresses
and record values remain unchanged.

The parent's later live observation used one muted `Aomacfixr` session
under the live lock on clean snapshot `7816a52`: ICC4582,
position930.01/24.21/759.67, a thirty-second wait and120 fixed frames.
`/camp` returned to login with session=false and exit0. Effect45083 and
a new combat effect were **not observed**. This was offscreen live evidence,
not a retail comparison or real-window PASS. Partial rock occlusion and
alternating foreground at static-camera frames50/102 were assigned to
the separate CameraFlicker investigation; they are not effect-coverage
successes and no second authentication was attempted.

* The `imp-*` hit-reaction selector (section 4); the bare-hand attack list (3.1); `ToClientDynelDead` caller; action 0x98 server-side meaning; stat 0x183 name.
* Native nulls and implementation gaps are separate (§7.9): class0 body
  profiles and class2003 cannot be constructed by any factory; missing
  class/form cases are likewise native null, not missing port artwork.
  Actual native-constructible failures retain their ID/class/configuration
  error rather than substituting another visual.
* Source-record rejection is also distinct from malformed parsing:
  GC spell-element dispatcher `1002b297` has no type0x17 branch and rejects
  item kinds below10000; installed records213967/213968 begin with kind0.
  GD `IsValid` `1000ccf6` rejects CF0A when stats2 and0x25 are both0;
  reader `1000d686` throws DataStreamException. Twenty-one installed nano
  records exhibit that exact rejection (including26349, CF0A offset283).
  GC `1002b297`'s type0 falls through `LAB_1002b337` and returns0 without
  consuming payload. Installed285827/286243 have declared type0/sub0 at412;
  284388 has type0/sub2 at371, after the complete CF8E payload at367–371.
  These are real rejected element headers, not text misalignment.
  The census preserves only these exact native rejection signatures separately
  from unresolved malformed records. Source: `FxClasses.Parser` native export.
  Parser boundary corrections restore GD `1000fb0a` CF41 string stat0/int39
  in native format46; the former format45 assignment omitted other string
  overloads and is superseded. The complete constructor evidence is22543.
  Additional evidence:
  TextureSpellFormat CF2F ctor/read/write/default
  `10014440`/`1001474b`/`1001455d`/`100144f1`, vtable `10020c90`.
  Sources: artifact22543 and the parser worker's native dispatcher export.
  Added, not exercised here: `texture_and_chat_payload_boundaries`,
  `installed_texture_and_chat_elements`, `native_unsupported_element_is_not_skipped`.
* Server `GfxTriggerIIR_t` (0x7A222202)/`HealthDamageIIR_t` (0x3710256C)
  visual dispatch remains separate from the authored weapon/nano paths.
  `PlaySoundIIR_c` (0x455D2938) never occurs in the capture.
  Weapon firing/impact sounds are resolved in §6.1.
* Sound-map keys 0x15 0x16 0x17 0x1c 0x1d 0x50 0x87 of the weapon records (no consumer found), wield / unwield / grenade sounds (keys 8 / 9 / 0x31, found, not wired), the `0x2c` empty-weapon click.
* `FUN_1005d0d8` case 0x5b also calls `vtable+0x40` of the stat system and, for a non-control char, `FUN_100523c3` (purpose not read); `FUN_10012a1e(..)` before every `PlayGameSound` is only the lazy creation of the `SandyInterfaceModule` singleton (`DAT_102e063c`), not a sound step.
* The own character's `Dying` default animation when no action 99 arrived (death computed by `FUN_1005ae91`): 503 is a **guess** (`DEFAULT_DEATH_ANIM`).
