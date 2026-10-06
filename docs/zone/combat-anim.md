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
Every `Hit` / `Miss` / `SpecialAttack` event of any character (a miss runs `FUN_1006a8f3` with damage 0, section 4):
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
the slot holds the `DummyWeapon_t` of the martial-arts item the `SpecialAttackWeaponIIR` list delivers under key 100 (docs/zone/combat-log.md §2.1.1; record 43712 etc.), not a client-made item; its animation multimap layout is still undecoded).

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
  (own: idle / fight idle / walk / run), `Dynels::update` (others, same). Not wired: the martial-arts item's lists 0x1a / 0x1b / 0x10 (bare hands; record layout not decoded, §3.1), the crawl
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
(`Dynels::swing_mark` + the clip time in `update`) fire notes once per play; `glue.rs` hands them to `note_sounds` with the attacker's last `AttackInfo` / `MissedAttackInfo` (`hit_of`, `Dynels::hit_seen`: victim,
slot, damage, hit kind) and the item behind that slot (`Armory::slot_item`: record sounds, `AmmoType`, wielded); sounds go through the `GameSound` queue (`material`, `size`, delayed ones through `Dynels::update`) to
`Audio::play_game_sound_with` (flow.rs logs `game sound <id> at <pos>: N voice(s) (material m, size s)` with `AOMAC_AUDIO_LOG=1`). Positions: the character; the own character at the camera.
Tests: `notes::tests::*` (note ids, once-per-play firing, size / ammo / player-impact rules, real swing clips), `dynels::variant_tests::{a_bare_handed_hit_plays_the_weapon_swing_and_the_material_impact,
swish_and_attack_start_notes, a_marked_swing_clip_reports_its_notes, creature_records_carry_a_fabric_type, a_struck_creature_plays_an_impact_clip}`, `arms::tests::real_records`, ao-formats
`a_weapon_record_finds_its_sound_multimap_behind_the_unwalked_elements`, ao-audio `game_material_maps_to_a_variant_slot` and `weapon_sounds_resolve` (all 31 843 ids but 75 are in the sbf; the flesh variant plays).
Deviations: the notes fire from the clip time of the swing, which restarts like the original's (not at the `AttackInfo`); the impact uses the attacker's target at the `AttackInfo` (the original reads the fight target at note
time); unknown victims / items (no slot object, `FUN_10068072` null) are silent as in the original; creature impacts need the NPC model to be built.
The `EffectType` 413 / `ImpactEffectType` 414 effect scripts (`FUN_1009ad7d`) are visual only and stay unresolved.
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

## 8. Not found / open
* The `imp-*` hit-reaction selector (section 4); the bare-hand attack list (3.1); `ToClientDynelDead` caller; action 0x98 server-side meaning; stat 0x183 name.
* Weapon firing/impact **effects** (effect scripts `CreateEffect2(EffectType 413 / ImpactEffectType 414)`, `FUN_1009ad7d`): visual, unresolved. Their **sounds** are resolved (section 6.1). `PlaySoundIIR_c` (0x455D2938),
  `GfxTriggerIIR_t` (0x7A222202) and `HealthDamageIIR_t` (0x3710256C) are registered message ids that never occur in the capture - server-driven sounds/effects may arrive through them.
* Sound-map keys 0x15 0x16 0x17 0x1c 0x1d 0x50 0x87 of the weapon records (no consumer found), wield / unwield / grenade sounds (keys 8 / 9 / 0x31, found, not wired), the `0x2c` empty-weapon click.
* `FUN_1005d0d8` case 0x5b also calls `vtable+0x40` of the stat system and, for a non-control char, `FUN_100523c3` (purpose not read); `FUN_10012a1e(..)` before every `PlayGameSound` is only the lazy creation of the `SandyInterfaceModule` singleton (`DAT_102e063c`), not a sound step.
* The own character's `Dying` default animation when no action 99 arrived (death computed by `FUN_1005ae91`): 503 is a **guess** (`DEFAULT_DEATH_ANIM`).
