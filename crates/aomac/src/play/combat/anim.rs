//! Combat animations, fight sounds, death and corpses of the original client (docs/zone/combat-anim.md).
//!
//! Everything here is a pure lookup / state machine; the data tables are transcribed from `Gamecode.dll`:
//! * [`ANIMS`] = the `AbstractAnimID_e` table built by `FUN_100c01c9` [GC 0x100c01c9] (197 entries, id -> clip name
//!   + layer), [`SOCIAL`] = `FUN_100bfef6` [GC 0x100bfef6] (ids 1..=0x46).
//! * [`clip_candidates`] = the file-name rule of `FUN_10010c57` [GC 0x10010c57]; [`fallback`] = `FUN_10010ad1`.
//! * [`weapon_anims`] = `FUN_1009d41a` [GC 0x1009d41a], the weapon item's action lists keyed by `stat AnimSet (0x161)`.
//! * Death: `CharacterAction 99` + `CharDie_t` [GC 0x1007b2ba]; sounds `SM_Sandy_Game_{Male,Female}{Dies,GetsHit}`.


use ao_formats::character::{fallback_key, NameTable};

/// rdb type of the animation clips (name table section 1010003).
pub const CLIP_TYPE: u32 = 1_010_003;
/// Blend layer of an animation (`Layer_e`, written by `FUN_10010c57` as `~kind & 3`): the table's second field 3 = layer 0
/// (whole body), 2 = layer 1 (overlay: `op1h-button`, the eight `wield` entries, `opself-firstaid`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Body,
    Upper,
}

/// `AbstractAnimID_e` of the attack animation of a creature without weapon attractors (`FUN_1006a239` [GC 0x1006a239]:
/// `local_14 = 0x40a`): `unarmed-rswing`, resolved through the NPC record's animation table.
pub const UNARMED_RSWING: u16 = 1034;

/// `AbstractAnimID_e` -> clip name and layer (`FUN_100c01c9` [GC 0x100c01c9], sorted by id). Several ids share a name
/// (`wield` x8, `sneakcool` x2, `jump-forward` x2, `op1h-button`/`op2h-wheel` x2, `die-pain` x2): kept as in the DLL.
pub const ANIMS: &[(u16, &str, Layer)] = &[
    (100, "walk", Layer::Body),
    (101, "run", Layer::Body),
    (102, "sneakcool", Layer::Body),
    (103, "crawl", Layer::Body),
    (104, "crawl_start", Layer::Body),
    (105, "crawl_stop", Layer::Body),
    (106, "climb", Layer::Body),
    (107, "op1h-button", Layer::Upper),
    (108, "op2h-wheel", Layer::Upper),
    (109, "wield", Layer::Upper),
    (110, "wield", Layer::Upper),
    (111, "wield", Layer::Upper),
    (112, "wield", Layer::Upper),
    (113, "wield", Layer::Upper),
    (114, "wield", Layer::Upper),
    (115, "wield", Layer::Upper),
    (116, "wield", Layer::Upper),
    (117, "wield", Layer::Upper),
    (118, "push", Layer::Body),
    (119, "pull", Layer::Body),
    (120, "idle-stand", Layer::Body),
    (124, "fall-back", Layer::Body),
    (125, "onback-stop", Layer::Body),
    (126, "imp-back", Layer::Body),
    (127, "imp-chest", Layer::Body),
    (128, "imp-head", Layer::Body),
    (129, "imp-larm", Layer::Body),
    (130, "imp-rarm", Layer::Body),
    (131, "imp-legs", Layer::Body),
    (132, "imp-stomach", Layer::Body),
    (133, "swim", Layer::Body),
    (134, "walk-left", Layer::Body),
    (135, "walk-right", Layer::Body),
    (136, "walk-back", Layer::Body),
    (137, "sneakcool", Layer::Body),
    (138, "sneakgoof", Layer::Body),
    (139, "hidecool", Layer::Body),
    (140, "hidecool-start", Layer::Body),
    (141, "hidecool-stop", Layer::Body),
    (142, "hidegoof", Layer::Body),
    (143, "hidegoof-start", Layer::Body),
    (144, "hidegoof-stop", Layer::Body),
    (145, "op1h-button", Layer::Body),
    (146, "op1h-keypad", Layer::Body),
    (147, "op1h-lever", Layer::Body),
    (148, "op2h-lever", Layer::Body),
    (149, "op2h-wheel", Layer::Body),
    (150, "op2h-firstaid", Layer::Body),
    (151, "opself-firstaid", Layer::Upper),
    (152, "throw-gren-1h", Layer::Body),
    (153, "throw-gren", Layer::Body),
    (154, "idle-crawl", Layer::Body),
    (155, "wield-crawl", Layer::Body),
    (156, "jump-stand", Layer::Body),
    (157, "jump-forward", Layer::Body),
    (158, "jump-forward", Layer::Body),
    (159, "action-stand", Layer::Body),
    (160, "unarmed-cranekick", Layer::Body),
    (161, "unarmed-cranepunch", Layer::Body),
    (162, "unarmed-frontkick-low", Layer::Body),
    (163, "unarmed-palmpunch-double", Layer::Body),
    (164, "unarmed-roundkick-double", Layer::Body),
    (165, "unarmed-roundkick-jump", Layer::Body),
    (166, "unarmed-saltokick", Layer::Body),
    (167, "unarmed-sidepunch", Layer::Body),
    (168, "unarmed-sidestrike", Layer::Body),
    (169, "unarmed-spinkick", Layer::Body),
    (170, "unarmed-spinkick-double", Layer::Body),
    (171, "unarmed-swipekick-double", Layer::Body),
    (172, "unarmed-capoeira", Layer::Body),
    (173, "unarmed-snakestrike", Layer::Body),
    (174, "unarmed-punch-oneinch", Layer::Body),
    (175, "unarmed-flyingkick", Layer::Body),
    (176, "unarmed-flyingkick-double", Layer::Body),
    (177, "unarmed-powersidekick", Layer::Body),
    (178, "idle-hover", Layer::Body),
    (179, "hover-norm", Layer::Body),
    (180, "hover-fast", Layer::Body),
    (181, "bow-start", Layer::Body),
    (182, "idle-bow", Layer::Body),
    (183, "bow-shot", Layer::Body),
    (184, "bow-stop", Layer::Body),
    (185, "jump-land-walk", Layer::Body),
    (186, "jump-land-run", Layer::Body),
    (187, "jump-land-idle", Layer::Body),
    (188, "jump-land-walk-2h", Layer::Body),
    (189, "jump-land-run-2h", Layer::Body),
    (190, "jump-land-idle-2h", Layer::Body),
    (195, "idle-swim", Layer::Body),
    (196, "turn-left", Layer::Body),
    (197, "turn-right", Layer::Body),
    (200, "spell-gen", Layer::Body),
    (201, "spell-dir", Layer::Body),
    (202, "spell-self", Layer::Body),
    (203, "spell-sys", Layer::Body),
    (204, "die-crawl", Layer::Body),
    (205, "crawl_enter-rifle", Layer::Body),
    (206, "crawl_idle-rifle", Layer::Body),
    (207, "crawl_exit-rifle", Layer::Body),
    (208, "crawl_impact-rifle", Layer::Body),
    (209, "crawl_shoot-rifle", Layer::Body),
    (210, "fallback-start", Layer::Body),
    (211, "fallback-stop", Layer::Body),
    (212, "idle-fallback", Layer::Body),
    (213, "ground-start", Layer::Body),
    (214, "ground-stop", Layer::Body),
    (215, "idle-ground", Layer::Body),
    (216, "chair-start", Layer::Body),
    (217, "chair-stop", Layer::Body),
    (218, "idle-chair", Layer::Body),
    (220, "run-left", Layer::Body),
    (221, "run-right", Layer::Body),
    (222, "run-back", Layer::Body),
    (223, "spell-ant", Layer::Body),
    (224, "unarmed-backstab", Layer::Body),
    (225, "unarmed-charge", Layer::Body),
    (226, "unarmed-groinkick", Layer::Body),
    (227, "unarmed-headbutt", Layer::Body),
    (228, "unarmed-spell1", Layer::Body),
    (229, "unarmed-spell2", Layer::Body),
    (230, "unarmed-spell3", Layer::Body),
    (231, "crawl_enter-smallarms", Layer::Body),
    (232, "crawl_exit-smallarms", Layer::Body),
    (233, "crawl_idle-smallarms", Layer::Body),
    (234, "crawl_shoot-smallarmsLh", Layer::Body),
    (235, "crawl_shoot-smallarmsRh", Layer::Body),
    (236, "crawl_impact-smallarms", Layer::Body),
    (237, "sleep-ground", Layer::Body),
    (238, "idle-sleep-ground", Layer::Body),
    (239, "lounging", Layer::Body),
    (240, "idle-lounging", Layer::Body),
    (241, "blade2h_EP3_area_attack", Layer::Body),
    (242, "blade2h_EP3_1-2-chop", Layer::Body),
    (243, "blade2h_EP3_dual_attack", Layer::Body),
    (244, "blade2h_EP3_side_sweep", Layer::Body),
    (245, "blade2h_EP3_front_attack", Layer::Body),
    (246, "blade1h_EP3_heavystrike", Layer::Body),
    (247, "blade1h_EP3_sidesweep", Layer::Body),
    (248, "unarmed_EP3_faceslap", Layer::Body),
    (249, "unarmed_EP3_leg_kick", Layer::Body),
    (250, "unarmed_EP3_slam_heavy", Layer::Body),
    (251, "unarmed-EP3_bodysmash", Layer::Body),
    (252, "rifle-burst_EP3_front", Layer::Body),
    (253, "rifle-burst_EP3_spread", Layer::Body),
    (254, "smallarms-burst_EP3_2Xpistol_spread", Layer::Body),
    (255, "smallarms-burst_EP3_1Xpistol_front", Layer::Body),
    (256, "smallarms-burst_EP3_2Xpistol_front", Layer::Body),
    (500, "die-knees", Layer::Body),
    (501, "die-pain", Layer::Body),
    (502, "die-poison", Layer::Body),
    (503, "die-shot", Layer::Body),
    (504, "die-ground", Layer::Body),
    (505, "die-float", Layer::Body),
    (1000, "blade-start", Layer::Body),
    (1001, "idle-blade", Layer::Body),
    (1002, "blade-stop", Layer::Body),
    (1003, "blade1h-slash", Layer::Body),
    (1004, "blade1h-stab", Layer::Body),
    (1005, "blade1hlr-stab", Layer::Body),
    (1006, "blade1hl-slash", Layer::Body),
    (1007, "blade1hl-stab", Layer::Body),
    (1010, "smallarms-start", Layer::Body),
    (1011, "idle-smallarms", Layer::Body),
    (1012, "smallarms-stop", Layer::Body),
    (1013, "smallarms-shot", Layer::Body),
    (1014, "smallarms-burst", Layer::Body),
    (1015, "smallarms-auto", Layer::Body),
    (1016, "smallarms-shotl", Layer::Body),
    (1017, "smallarms-burstl", Layer::Body),
    (1018, "smallarms-autol", Layer::Body),
    (1020, "rifle-start", Layer::Body),
    (1021, "idle-rifle", Layer::Body),
    (1022, "rifle-stop", Layer::Body),
    (1023, "rifle-shot", Layer::Body),
    (1024, "rifle-burst", Layer::Body),
    (1025, "rifle-auto", Layer::Body),
    (1030, "unarmed-start", Layer::Body),
    (1031, "idle-unarmed", Layer::Body),
    (1032, "unarmed-stop", Layer::Body),
    (1033, "unarmed-kick", Layer::Body),
    (1034, "unarmed-rswing", Layer::Body),
    (1035, "unarmed-uppercut", Layer::Body),
    (1036, "unarmed-twopunch", Layer::Body),
    (1037, "unarmed-lswing", Layer::Body),
    (1050, "blade2h-chop", Layer::Body),
    (1051, "blade2h-downcut", Layer::Body),
    (1052, "blade2h-slash", Layer::Body),
    (1053, "blade2h-stab", Layer::Body),
    (1054, "idle-2h", Layer::Body),
    (1055, "2h-start", Layer::Body),
    (1056, "2h-stop", Layer::Body),
    (1057, "walk-2h", Layer::Body),
    (1058, "run-2h", Layer::Body),
    (1060, "idle-bazooka", Layer::Body),
    (1062, "bazooka-shot", Layer::Body),
    (6000, "die-pain", Layer::Body),
    (6666, "noanim", Layer::Body),
];

/// Social-action names (`FUN_100bfef6` [GC 0x100bfef6], struct stride 12: `name`, `id`, `alt`). `FUN_10010c57` prefers
/// `alt` when it is set. Ids 1..=0x46; below `0x48` the file is `<set>_social-<name>.ani`.
pub const SOCIAL: &[(u16, &str, Option<&str>)] = &[
    (1, "prostrate", Some("allah")),
    (2, "angry", None),
    (3, "apachi", None),
    (4, "applause", None),
    (5, "itch", Some("ass")),
    (6, "backflip", None),
    (7, "ballet", None),
    (8, "blowkiss", None),
    (9, "bow", None),
    (10, "bulge", None),
    (11, "chicken", None),
    (12, "cross", None),
    (13, "crossarm", None),
    (14, "adjust", Some("crotch")),
    (15, "curt", None),
    (16, "disco", None),
    (17, "drink", None),
    (18, "eat", None),
    (19, "fblock", None),
    (20, "fishsize", None),
    (21, "flamenco", None),
    (22, "flip", None),
    (23, "giggle", None),
    (24, "gloat", None),
    (25, "greet", None),
    (26, "italian", None),
    (27, "kneel", None),
    (28, "laugh-b", None),
    (29, "laugh-s", None),
    (30, "legshake", None),
    (31, "lookout", None),
    (32, "moon", None),
    (33, "nod", None),
    (34, "nono", None),
    (35, "pointba", None),
    (36, "pointfor", None),
    (37, "pointlef", None),
    (38, "pointrig", None),
    (39, "pointup", None),
    (40, "pray", None),
    (41, "puke", None),
    (42, "pulp", None),
    (43, "read", None),
    (44, "rocky", None),
    (45, "salute", None),
    (46, "scared", None),
    (47, "scratch", None),
    (48, "shake", None),
    (49, "shrug", None),
    (50, "slap", None),
    (51, "speech", None),
    (52, "spit", None),
    (53, "strong1", None),
    (54, "strong2", None),
    (55, "strong3", None),
    (56, "strong4", None),
    (57, "surprised", None),
    (58, "surrender", None),
    (59, "swroyal", None),
    (60, "thinker", None),
    (61, "thumbs", None),
    (62, "wave", None),
    (63, "ymca", None),
    (64, "kiss", Some("kiss_01_01")),
    (65, "kisslow", Some("kissdown_01_01")),
    (66, "kisshigh", Some("kissup_01_01")),
    (67, "hug", Some("hug_01_01")),
    (68, "sleep", None),
    (69, "lounge", None),
    (70, "facepalm", None),
];

/// Table entry of an animation id.
pub fn anim(id: u16) -> Option<(&'static str, Layer)> {
    ANIMS.binary_search_by_key(&id, |e| e.0).ok().map(|i| (ANIMS[i].1, ANIMS[i].2))
}

/// `FUN_100c013c`: the name of an id (table, else the social table with layer 0).
pub fn anim_name(id: u16) -> Option<(&'static str, Layer)> {
    anim(id).or_else(|| {
        let i = id.checked_sub(1).filter(|i| *i < 0x46)? as usize;
        let e = SOCIAL.get(i)?;
        Some((e.2.unwrap_or(e.1), Layer::Body))
    })
}

/// Animation set prefix of a character (`FUN_10010c57` args `breed`, `sex` = stats 4 and 0x3b): Atrox (breed 4) uses its
/// own `athrox` clips, `sex == 1` (neuter) or breed 5 use `male`, otherwise the sex name (`2` male, `3` female;
/// opifex / nanomage share the `male` / `female` sets) [DATA: name table `athrox_*`, `male_*`, `female_*`].
pub fn clip_set(breed: u8, sex: u8) -> &'static str {
    if breed == 4 {
        "athrox"
    } else if sex == 1 || breed == 5 || sex == 2 {
        "male"
    } else {
        "female"
    }
}

/// File names `FUN_10010c57` tries for `id`, in order. Ids below `0x48` are social actions
/// (`<set>_social-<name>.ani`, then `<set>_<name>.ani`), all others `<set>_<name>_01_01.ani`, then `<set>_<name>.ani`.
pub fn clip_candidates(set: &str, id: u16) -> Vec<String> {
    let Some((name, _)) = anim_name(id) else { return Vec::new() };
    if id < 0x48 {
        vec![format!("{set}_social-{name}.ani"), format!("{set}_{name}.ani")]
    } else {
        vec![format!("{set}_{name}_01_01.ani"), format!("{set}_{name}.ani")]
    }
}

/// `FUN_10010ad1` [GC 0x10010ad1]: the parent of an animation id (0 = none). `turn` ids 0xc4/0xc5 map to walk-left/right
/// only for a character that is in a vehicle (stat 0x296 != 0), pass that flag as `in_vehicle`.
pub fn fallback(id: u16, in_vehicle: bool) -> u16 {
    match id {
        0xc4 if in_vehicle => 0x86,
        0xc5 if in_vehicle => 0x87,
        _ => fallback_key(id as u32) as u16,
    }
}

/// `FUN_1003c802` [GC 0x1003c802], name branch: the first existing clip along `id -> fallback(id) -> ...`
/// (stops on a cycle). Returns `(rdb 1010003 id, resolved AbstractAnimID, layer)`.
pub fn resolve_clip(names: &NameTable, set: &str, id: u16, in_vehicle: bool) -> Option<(u32, u16, Layer)> {
    let mut cur = id;
    loop {
        for cand in clip_candidates(set, cur) {
            if let Some(clip) = names.id(CLIP_TYPE, &cand) {
                return Some((clip, cur, anim_name(cur)?.1));
            }
        }
        let next = fallback(cur, in_vehicle);
        if next == cur || next == 0 || next == id {
            return None;
        }
        cur = next;
    }
}

// ---------------------------------------------------------------- weapon action lists

/// Action-list keys of the weapon item's multimap (`FUN_1004570c(key)` picks a random value). Meaning derived from the
/// values filed under each key by [`weapon_anims`] and from the callers.
pub mod list {
    /// Primary attack / swing (`FUN_10069acb` [GC 0x10069acb], `FUN_1006a239`).
    pub const ATTACK: u16 = 0xb;
    /// Idle stance clip (AnimHolder idle update `FUN_1003cad0`: `FUN_1004570c(0x10)`).
    pub const IDLE: u16 = 0x10;
    /// Stance start (draw) / stop (holster); the AnimHolder `Wield`/`Unwield` pair `FUN_1003c930`/`FUN_1003c9b2`.
    pub const START: u16 = 0x1a;
    pub const STOP: u16 = 0x1b;
    /// Special-attack swings (`FUN_1003c594` [GC 0x1003c594], see [`super::special_swing`]): Aimed Shot, Burst, Full Auto, Fast Attack,
    /// Fling Shot, Sneak Attack. A weapon without the key falls back to [`ATTACK`].
    pub const AIMED_SHOT: u16 = 0x15;
    pub const BURST: u16 = 0x16;
    pub const FULL_AUTO: u16 = 0x17;
    pub const FAST_ATTACK: u16 = 0x19;
    pub const FLING_SHOT: u16 = 0x1c;
    pub const SNEAK_ATTACK: u16 = 0x1d;
    /// Brawl / Dimach (martial-arts item records) and the keys used for Backstab / Bow special.
    pub const BRAWL: u16 = 0x23;
    pub const DIMACH: u16 = 0x24;
    pub const BOW_SPECIAL: u16 = 0x87;
    pub const BACKSTAB: u16 = 0x89;
    /// Two-handed / rifle / bazooka overrides of start / idle / stop / walk / run (`AnimSet` 3, 6, 8).
    pub const START_2H: u16 = 0x27;
    pub const STOP_2H: u16 = 0x28;
    pub const IDLE_2H: u16 = 0x29;
    pub const WALK_2H: u16 = 0x2a;
    pub const RUN_2H: u16 = 0x2b;
}

/// Weapon item stat `AnimSet` (0x161 = 353) values handled by `FUN_1009d41a`; any other value (4, 5, ...) registers nothing.
pub mod anim_set {
    pub const PISTOL: i32 = 0;
    pub const BLADE_1H: i32 = 1;
    pub const BLADE_2H: i32 = 2;
    pub const RIFLE: i32 = 3;
    pub const BOW: i32 = 6;
    pub const TOOL: i32 = 7;
    pub const BAZOOKA: i32 = 8;
}

/// `FUN_1009d41a` [GC 0x1009d41a] (`param_2 == 8` = left-hand weapon slot, crawl = the wielder's stat 0x1ae == 0xe): the
/// `(list key, AbstractAnimID)` pairs the client files into the weapon item's multimap, in insertion order.
pub fn weapon_anims(set: i32, left: bool, crawl: bool) -> Vec<(u16, u16)> {
    use list::*;
    let stance = |plain: [u16; 3]| -> [(u16, u16); 3] {
        let v = if crawl { [0xe7, 0xe9, 0xe8] } else { plain };
        [(START, v[0]), (IDLE, v[1]), (STOP, v[2])]
    };
    let mut out = Vec::new();
    match set {
        anim_set::PISTOL => {
            out.extend(stance([0x3f2, 0x3f3, 0x3f4]));
            let (shot, burst, auto) = match (crawl, left) {
                (true, false) => (0xeb, 0xeb, 0xeb),
                (true, true) => (0xea, 0xea, 0xea),
                (false, false) => (0x3f5, 0x3f6, 0x3f7),
                (false, true) => (0x3f8, 0x3f9, 0x3fa),
            };
            out.extend([(ATTACK, shot), (BURST, burst), (AIMED_SHOT, shot), (FULL_AUTO, auto), (FLING_SHOT, shot)]);
        }
        anim_set::BLADE_1H => {
            out.extend(stance([1000, 0x3e9, 0x3ea]));
            let (a, b) = if left { (0x3ee, 0x3ef) } else { (0x3eb, 0x3ec) };
            out.extend([(ATTACK, a), (ATTACK, b), (ATTACK, a), (FAST_ATTACK, a), (SNEAK_ATTACK, b)]);
        }
        anim_set::BLADE_2H => {
            out.extend(stance([1000, 0x3e9, 0x3ea]));
            out.extend([(ATTACK, 0x41a), (ATTACK, 0x41b), (ATTACK, 0x41c), (ATTACK, 0x41d), (SNEAK_ATTACK, 0x41b), (FAST_ATTACK, 0x41d)]);
        }
        anim_set::RIFLE => {
            let (st, shot) = if crawl { ([(START, 0xcd), (IDLE, 0xce), (STOP, 0xcf)], 0xd1) } else { ([(START, 0x3fc), (IDLE, 0x3fd), (STOP, 0x3fe)], 0x3ff) };
            out.extend(st);
            let (burst, auto) = if crawl { (0xd1, 0xd1) } else { (0x400, 0x401) };
            out.extend([(ATTACK, shot), (BURST, burst), (FULL_AUTO, auto), (FLING_SHOT, shot), (AIMED_SHOT, shot)]);
            out.extend([(START_2H, 0x41f), (IDLE_2H, 0x41e), (STOP_2H, 0x420), (WALK_2H, 0x421), (RUN_2H, 0x422)]);
        }
        anim_set::BOW => {
            out.extend(stance([0xb5, 0xb6, 0xb8]));
            out.extend([(START_2H, 0xb5), (IDLE_2H, 0xb6), (STOP_2H, 0xb8), (ATTACK, 0xb7)]);
        }
        anim_set::TOOL => out.extend([(IDLE, 0xcb), (ATTACK, 0xcb)]),
        anim_set::BAZOOKA => out.extend([(IDLE, 0x424), (ATTACK, 0x426), (IDLE_2H, 0x424), (WALK_2H, 0x421), (RUN_2H, 0x422)]),
        _ => {}
    }
    out
}

/// Values of one list key of [`weapon_anims`] (the client picks one at random: `FUN_1004570c` [GC 0x1004570c]).
pub fn weapon_list(set: i32, left: bool, crawl: bool, key: u16) -> Vec<u16> {
    weapon_anims(set, left, crawl).into_iter().filter(|e| e.0 == key).map(|e| e.1).collect()
}

/// Result of `FUN_1003c594` [GC 0x1003c594] for a queued special attack (`FUN_1006855a` = its skill stat): which weapon list
/// key to play, the floating text printed above the character (`FUN_10011108(char, text, 0xd)`), the sound played at its
/// position, and whether the key is looked up on the special attack's own item (`FUN_100686d0(stat)+0xe4`) instead of the weapon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecialSwing {
    pub list: u16,
    pub text: Option<&'static str>,
    pub sound: Option<&'static str>,
    pub own_item: bool,
}

/// Special attack skill stat -> [`SpecialSwing`] (stat names from the client's table).
pub fn special_swing(stat: i32) -> Option<SpecialSwing> {
    let s = |list, text, sound, own_item| Some(SpecialSwing { list, text, sound, own_item });
    match stat {
        148 => s(list::BURST, Some("Burst!\n"), None, false),
        147 => s(list::FAST_ATTACK, Some("Fast attack!\n"), None, false),
        146 => s(list::SNEAK_ATTACK, Some("Sneak Attack!\n"), None, false),
        150 => s(list::FLING_SHOT, Some("Fling Shot!\n"), None, false),
        151 => s(list::AIMED_SHOT, Some("Aimed Shot!\n"), None, false),
        167 => s(list::FULL_AUTO, Some("Full Auto!\n"), None, false),
        142 => s(list::BRAWL, Some("Brawl!\n"), Some(sound::BRAWL), true),
        144 => s(list::DIMACH, Some("Dimach!\n"), Some(sound::DIMACH), true),
        489 => s(list::BACKSTAB, Some("Backstab!\n"), None, true),
        121 => s(list::BOW_SPECIAL, None, None, true),
        _ => None,
    }
}

/// Swing speed scale (`FUN_1006a239`): `clamp(noteTime_ms / (ItemDelay * 10), 1.0, 2.0)` where `noteTime` is the clip's
/// attack-note time (`VisualCATMesh_t::GetNoteTime`) and `ItemDelay` the weapon's stat 0x126 (centiseconds), constants
/// `_DAT_101600f0 = 10.0` (double) and `_DAT_10158794 = 2.0f`. Only applied to characters with `char+0x21c == 0`; `1.0` = no scaling.
pub fn swing_speed_scale(note_time_ms: f32, item_delay_cs: i32) -> f32 {
    let delay_ms = item_delay_cs as f32 * 10.0;
    if delay_ms > 0.0 && note_time_ms > 0.0 {
        (note_time_ms / delay_ms).clamp(1.0, 2.0)
    } else {
        1.0
    }
}

// ---------------------------------------------------------------- death

/// `CharacterActionIIR_t` action 99 (0x63): `FUN_1005d0d8` [GC 0x1005d0d8] case 0x19 sets stat flag 0x10
/// (`statsys->vt+0x18(0x10)`, code `6a 10 .. ff 50 18` @0x1005d835) and stat `0x183` := `identity_b.instance`
/// (`push 0x183` @0x1005d845). Live: 15 of 15 NPC deaths carry `identity_b = {0, 503}`.
pub const ACTION_DIE: i32 = 0x63;
/// Stat the dying character's animation id is stored in (`CharDie_t` ctor reads `GetSkill(0x183)`).
pub const STAT_DEATH_ANIM: u32 = 0x183;
/// Death animation of the own character when its stat 0x183 is 0 (a death the client computed itself, `FUN_1005ae91`, no server action 99):
/// **[UNRESOLVED GUESS]** 503 (`die-shot`), the only value seen in the capture.
pub const DEFAULT_DEATH_ANIM: u16 = 503;
/// `CharacterActionIIR_t` action of a character being struck (`FUN_1005d0d8` case 0x5b).
pub const ACTION_HIT: i32 = 0xd1;
/// Seconds `CharDie_t` waits (`_DAT_1015d69c` = 3.0f): after that the control character sends `CharacterAction 0x98`
/// and the state returns to idle. NPCs are removed earlier by the server's `n3ToClientQuit` (2.92 - 2.99 s after the action).
pub const DIE_WAIT_S: f32 = 3.0;
/// Outgoing `CharacterActionIIR_t` action id sent by the control character when its death wait is over.
pub const ACTION_DEATH_DONE: i32 = 0x98;

/// Death animation requested by a received `CharacterAction`: `Some(AbstractAnimID)` for action 99.
pub fn death_anim_from_action(action: i32, identity_b_instance: u32) -> Option<u16> {
    (action == ACTION_DIE).then_some(identity_b_instance as u16)
}

/// A dying character (`CharDie_t`): the animation to hold and the elapsed time.
#[derive(Clone, Debug, PartialEq)]
pub struct Dying {
    pub anim: u16,
    pub elapsed: f32,
    /// The control (own) character: sends [`ACTION_DEATH_DONE`] when the wait is over.
    pub own: bool,
    done_sent: bool,
}

/// What a [`Dying`] tick asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DieEvent {
    /// Own character, wait over: send `CharacterActionIIR_t(action 0x98, no identities, "")` (`FUN_1007b58c`).
    SendDeathDone,
}

impl Dying {
    pub fn new(anim: u16, own: bool) -> Dying {
        Dying { anim, elapsed: 0.0, own, done_sent: false }
    }

    /// `FUN_1007b58c`: advance the timer; at [`DIE_WAIT_S`] the own character emits [`DieEvent::SendDeathDone`] once.
    pub fn tick(&mut self, dt: f32) -> Option<DieEvent> {
        self.elapsed += dt;
        if self.own && !self.done_sent && self.elapsed > DIE_WAIT_S {
            self.done_sent = true;
            return Some(DieEvent::SendDeathDone);
        }
        None
    }
}

// ---------------------------------------------------------------- sounds

/// Sex stat value of a female character (`CharDie_t` ctor and `FUN_1005d0d8` case 0x5b compare `GetSkill(0x3b) == 3`).
pub const SEX_FEMALE: u8 = 3;

/// Sound definition names loaded by the fight code (`SandyInterfaceModule_t::GetSoundID(name)`).
pub mod sound {
    /// `CharDie_t` ctor [GC 0x1007b2ba]: played once at the dying character's position.
    pub const MALE_DIES: &str = "SM_Sandy_Game_MaleDies";
    pub const FEMALE_DIES: &str = "SM_Sandy_Game_FemaleDies";
    /// `FUN_1005d0d8` case 0x5b [GC 0x1005eada]: played at the hit character's position when both message values are > 0.
    pub const MALE_GETS_HIT: &str = "SM_Sandy_Game_MaleGetsHit";
    pub const FEMALE_GETS_HIT: &str = "SM_Sandy_Game_FemaleGetsHit";
    /// Loaded by `AnimHolder_t` ctor [GC 0x1003c525] into members `+0x40` / `+0x44`; played at the character by the special swing
    /// `FUN_1003c594` ([`special_swing`]).
    pub const BRAWL: &str = "SM_Sandy_Game_Brawl";
    pub const DIMACH: &str = "SM_Sandy_Game_Dimach";
}

/// NPC-record sound multimap keys used by the fight code (`FUN_1004570c(key)` on `record+0x24`, picks a random value).
pub mod npc_sound {
    /// `CharDie_t` ctor: death sound of a character with an NPC record (`FUN_10051f6e` != 0), instead of Male/FemaleDies.
    pub const DEATH: u32 = 0x1e;
    /// `FUN_1005d0d8` case 0x5b [GC 0x1005eb1b `6a 1f`]: hit sound.
    pub const HIT: u32 = 0x1f;
}

/// Death sound name for a character without NPC record sounds (`female` = Sex stat == 3).
pub fn die_sound(sex: u8) -> &'static str {
    if sex == SEX_FEMALE {
        sound::FEMALE_DIES
    } else {
        sound::MALE_DIES
    }
}

/// Hit ("gets hit") sound name for a character without NPC record sounds.
pub fn hit_sound(sex: u8) -> &'static str {
    if sex == SEX_FEMALE {
        sound::FEMALE_GETS_HIT
    } else {
        sound::MALE_GETS_HIT
    }
}

/// `FUN_1005d0d8` case 0x5b (`CharacterAction 0xd1`, GC 0x1005eada): the hit sound plays when `identity_a.instance` and `identity_b.instance`
/// are both positive (the handler's two `Identity*` arguments, `+4` of each: `[ebp+0xc]` / `[ebp+0x14]`; `param` is not read there).
pub fn plays_hit_sound(value_a: i32, value_b: i32) -> bool {
    value_a > 0 && value_b > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_rdb::RecordStore;

    fn client() -> Option<std::path::PathBuf> {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        dir.join("cd_image/rdb.db").exists().then_some(dir)
    }

    #[test]
    fn table_is_sorted_and_complete() {
        assert_eq!(ANIMS.len(), 197);
        assert!(ANIMS.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(anim(0x3eb), Some(("blade1h-slash", Layer::Body)));
        assert_eq!(anim(0x97), Some(("opself-firstaid", Layer::Upper)));
        assert_eq!(anim(503), Some(("die-shot", Layer::Body)));
        assert_eq!(anim_name(62).unwrap().0, "wave");
        assert_eq!(anim_name(1).unwrap().0, "allah");
        assert_eq!(SOCIAL.len(), 70);
    }

    #[test]
    fn file_name_rules() {
        assert_eq!(clip_candidates("male", 0x3eb), ["male_blade1h-slash_01_01.ani", "male_blade1h-slash.ani"]);
        assert_eq!(clip_candidates("female", 62)[0], "female_social-wave.ani");
        assert_eq!(clip_set(4, 2), "athrox");
        assert_eq!(clip_set(1, 3), "female");
        assert_eq!(clip_set(3, 2), "male");
        assert_eq!(fallback(503, false), 6000);
        assert_eq!(fallback(0x3fd, false), 0x3f3);
        assert_eq!(fallback(0xc4, true), 0x86);
        assert_eq!(fallback(0xc4, false), 0);
    }

    #[test]
    fn weapon_tables_follow_fun_1009d41a() {
        assert_eq!(weapon_list(1, false, false, list::ATTACK), [0x3eb, 0x3ec, 0x3eb]);
        assert_eq!(weapon_list(1, true, false, list::ATTACK), [0x3ee, 0x3ef, 0x3ee]);
        assert_eq!(weapon_list(2, false, false, list::ATTACK), [0x41a, 0x41b, 0x41c, 0x41d]);
        assert_eq!(weapon_list(0, false, false, list::ATTACK), [0x3f5]);
        assert_eq!(weapon_list(0, true, true, list::ATTACK), [0xea]);
        assert_eq!(weapon_list(3, false, false, list::BURST), [0x400]);
        assert_eq!(weapon_list(3, false, false, list::RUN_2H), [0x422]);
        assert_eq!(weapon_list(6, false, false, list::ATTACK), [0xb7]);
        assert_eq!(weapon_list(8, false, false, list::ATTACK), [0x426]);
        assert!(weapon_anims(4, false, false).is_empty());
        assert_eq!(anim_name(UNARMED_RSWING).map(|a| a.0), Some("unarmed-rswing"));
        assert_eq!(special_swing(148).unwrap().list, 0x16);
        assert_eq!(special_swing(144).unwrap().sound, Some(sound::DIMACH));
        assert!(special_swing(1).is_none());
        assert_eq!(weapon_list(0, false, false, list::FULL_AUTO), [0x3f7]);
    }

    #[test]
    fn swing_speed_is_clamped() {
        assert_eq!(swing_speed_scale(500.0, 100), 1.0); // slower note than delay: no slow-down
        assert_eq!(swing_speed_scale(1500.0, 100), 1.5);
        assert_eq!(swing_speed_scale(5000.0, 100), 2.0);
        assert_eq!(swing_speed_scale(0.0, 100), 1.0);
    }

    #[test]
    fn death_flow() {
        assert_eq!(death_anim_from_action(99, 503), Some(503));
        assert_eq!(death_anim_from_action(98, 503), None);
        let mut d = Dying::new(503, true);
        assert_eq!(d.tick(2.9), None);
        assert_eq!(d.tick(0.2), Some(DieEvent::SendDeathDone));
        assert_eq!(d.tick(1.0), None);
        assert_eq!(Dying::new(503, false).tick(10.0), None);
        assert_eq!(die_sound(3), sound::FEMALE_DIES);
        assert_eq!(hit_sound(2), sound::MALE_GETS_HIT);
        assert!(plays_hit_sound(5, 1) && !plays_hit_sound(5, 0));
    }

    /// Every clip the fight code can request exists for the `male`, `female` and `athrox` sets of the real data
    /// (except the ones listed as absent in docs/zone/combat-anim.md).
    #[test]
    fn fight_clips_exist_in_the_data() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let names = NameTable::load(&store).unwrap();
        let mut wanted: Vec<u16> = vec![0x78, 100, 101, 0x7e, 0x7f, 0x80, 0x81, 0x82, 0x83, 0x84, 500, 501, 502, 503, 504, 505, 0xcc, 6000, 1034];
        for set in [0, 1, 2, 3, 6, 7, 8] {
            for (left, crawl) in [(false, false), (true, false), (false, true)] {
                wanted.extend(weapon_anims(set, left, crawl).iter().map(|e| e.1));
            }
        }
        wanted.sort_unstable();
        wanted.dedup();
        let mut missing = Vec::new();
        for set in ["male", "female", "athrox"] {
            for &w in &wanted {
                if resolve_clip(&names, set, w, false).is_none() {
                    missing.push((set, w, anim_name(w).map(|n| n.0)));
                }
            }
        }
        // direct hits (no fallback needed) for the stance / swing clips of the human sets
        for set in ["male", "female", "athrox"] {
            for w in [0x78u16, 100, 101, 503, 0x3eb, 0x3f5, 0x3ff, 0x41a, 0x409, 0x40a] {
                let direct = clip_candidates(set, w).iter().any(|c| names.id(CLIP_TYPE, c).is_some());
                assert!(direct, "{set} {w:#x} {:?}", anim_name(w));
            }
        }
        assert!(missing.is_empty(), "no clip: {missing:?}");
        for set in ["male", "female", "athrox"] {
            let via_fallback: Vec<_> = wanted
                .iter()
                .filter(|w| resolve_clip(&names, set, **w, false).is_some_and(|r| r.1 != **w))
                .map(|w| format!("{w:#x}->{:#x}", resolve_clip(&names, set, *w, false).unwrap().1))
                .collect();
            println!("{set}: ids resolved through the fallback chain: {via_fallback:?}");
        }
    }

    #[test]
    fn sounds_exist_in_the_data() {
        let Some(dir) = client() else { return };
        let lib = ao_audio::Library::load(&dir.join("cd_image/sound")).unwrap();
        // the weapon-class names `FUN_1009b913` loads (members +0x74..+0x7c, no reader found) are absent from both .sbf files (GetSoundID finds
        // nothing): there is nothing to play, so the code has no constants for them (docs/zone/combat-anim.md §6)
        for n in ["FlameThrowerFire", "PistolSingleShotHitFlesh", "PistolSingleShotHitGround", "PistolMultiShotFire", "PistolSingleShotFire"] {
            assert!(lib.sounds.by_name(&format!("SM_Sandy_Game_{n}")).is_none(), "{n}");
        }
        assert!(lib.sounds.by_name(sound::BRAWL).is_some() && lib.sounds.by_name(sound::DIMACH).is_some());
        let die = lib.sounds.by_name(sound::MALE_DIES).unwrap();
        assert_eq!(die.file.as_deref(), Some("sfx/breeds/male_die.wav"));
        assert!(!lib.sounds.by_name(sound::FEMALE_DIES).unwrap().children.is_empty());
        assert!(lib.sounds.by_name(sound::FEMALE_GETS_HIT).unwrap().children.len() == 4);
        let d = lib.sounds.by_name(sound::MALE_GETS_HIT).unwrap();
        assert!(d.random_child && d.children.len() == 5);
    }
}
