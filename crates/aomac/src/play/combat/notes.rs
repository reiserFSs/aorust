//! Animation notes and the sounds they start (docs/zone/combat-anim.md section 6).
//!
//! The original plays no weapon sound when an `AttackInfo` arrives: `FUN_1006a239` only starts the swing clip with the attack slot (`Play(.., slot)`),
//! and the sounds are started by the **notes of that clip**: every frame the holder (`FUN_1003c036` [GC 0x1003c036]) walks the clips it plays and
//! calls `FUN_1003bccb` -> `FUN_10045069` [GC 0x10045069] for each note (the clip's named events, `CatAnim::events`) whose time has been reached.
//! The note id comes from the event *name* ([`note_id`], DisplaySystem `FUN_10075c84` [DS 0x10075c84], `strncmp` prefixes).


/// A holder marker carries its own playback slot; combat state is read when it is dispatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FiredNote {
    pub id: u32,
    pub slot: i32,
}
/// Note ids the fight code reacts to (`FUN_10045069`).
pub mod id {
    /// `attack` (also `attack\r\n`): the moment of the blow / shot of a swing clip.
    pub const ATTACK: u32 = 0xb;
    /// `swish_punch` / `swish_kick` / `swish_whip` (tail) / `swish_huge`.
    pub const SWISH_PUNCH: u32 = 0x73;
    pub const SWISH_HUGE: u32 = 0x76;
    /// `attack_start_1..9`: creature attack sounds (NPC record sound lists).
    pub const ATTACK_START_1: u32 = 0x77;
    pub const ATTACK_START_9: u32 = 0x7f;
    /// `attack_effect_1..4` (= the special-attack stat ids 142..145): the same weapon sound path as `attack`.
    pub const ATTACK_EFFECT_1: u32 = 0x8e;
    pub const ATTACK_EFFECT_4: u32 = 0x91;
}

/// `FUN_10075c84` [DS 0x10075c84]: the note id of an event name, in the function's order with its `strncmp(name, literal, len(literal))` prefix tests
/// (so `stepfast` is `step`, `idle_combat` is `idle`, `leftattack` is `left`, `attack_start_1` is tested before `attack`). 0 = no note.
pub fn note_id(name: &str) -> u32 {
    let is = |p: &str| name.starts_with(p);
    if is("loopstart") || is("loopend") {
        return 0;
    }
    if is("right") || is("left") || is("step") {
        return 0x26;
    }
    if is("land") {
        return 0x85;
    }
    const TABLE: &[(&str, u32)] = &[
        ("wingflap", 0x25),
        ("swim", 0x38),
        ("othermove", 0x3a),
        ("idle", 0xf),
        ("attack_start_1", 0x77),
        ("attack_start_2", 0x78),
        ("attack_start_3", 0x79),
        ("attack_start_4", 0x7a),
        ("attack_start_5", 0x7b),
        ("attack_start_6", 0x7c),
        ("attack_start_7", 0x7d),
        ("attack_start_8", 0x7e),
        ("attack_start_9", 0x7f),
        ("attack_effect_1", 0x8e),
        ("attack_effect_2", 0x8f),
        ("attack_effect_3", 0x90),
        ("attack_effect_4", 0x91),
        ("attack", 0xb),
        ("get", 1),
        ("drop", 2),
        ("use", 3),
        ("repair", 4),
        ("wear", 6),
        ("unwear", 7),
        ("wield", 8),
        ("unwield", 9),
        ("die", 0x1e),
        ("impact", 0x1f),
        ("doubleattack", 0xe),
        ("aimedshot", 0x15),
        ("burst", 0x16),
        ("fullauto", 0x17),
        ("quickattack", 0x19),
        ("flingshot", 0x1c),
        ("sneakattack", 0x1d),
        ("dimach", 0x24),
        ("brawl", 0x23),
        ("blurstart", 0x2d),
        ("blurend", 0x35),
        ("itemhide_l", 0x3c),
        ("itemhide_r", 0x3d),
        ("itemhide_b", 0x3e),
        ("itemshow_l", 0x3f),
        ("itemshow_r", 0x40),
        ("itemshow_b", 0x41),
        ("effect1start", 0x42),
        ("effect1stop", 0x43),
        ("effect2start", 0x44),
        ("effect2stop", 0x45),
        ("swish_punch", 0x73),
        ("swish_kick", 0x74),
        ("swish_whip", 0x75),
        ("swish_huge", 0x76),
        ("enter_combat", 0x80),
        ("exit_combat", 0x81),
    ];
    TABLE.iter().find(|(p, _)| is(p)).map_or(0, |&(_, n)| n)
}

/// The notes of a playing clip whose event time (`CatAnim::events`, ms) has been reached at `clip_ms`; every event fires once per play (`fired` is the
/// per-clip bit mask, `piVar7[0xd]`, cleared when the clip starts). Events without a note id are marked but not returned.
pub fn fire<'a>(events: &'a [(u32, String)], clip_ms: f32, fired: &'a mut u32) -> impl Iterator<Item = u32> + 'a {
    events.iter().enumerate().filter_map(move |(i, (t, name))| {
        let bit = 1u32 << (i & 31);
        if *fired & bit != 0 || *t as f32 > clip_ms { return None }
        *fired |= bit;
        let note = note_id(name);
        (note != 0).then_some(note)
    })
}

/// GC 0x1003c036 flushes unfired attack-related markers when the CAT handle disappears.
pub fn finish<'a>(events: &'a [(u32, String)], fired: &'a mut u32) -> impl Iterator<Item = u32> + 'a {
    events.iter().enumerate().filter_map(move |(i, (_, name))| {
        let bit = 1u32 << (i & 31);
        let note = note_id(name);
        if *fired & bit != 0 || !matches!(note, id::ATTACK | 0x3c..=0x41 | id::ATTACK_EFFECT_1..=id::ATTACK_EFFECT_4) { return None }
        *fired |= bit;
        Some(note)
    })
}

/// `SM_Sandy_Swish_*` of a player (`AnimHolder_t` ctor `FUN_10044702` members `+0x10..+0x1c`, played by notes 0x73..0x76 when the character has no NPC record).
pub fn swish_name(note: u32) -> Option<&'static str> {
    Some(match note {
        0x73 => "SM_Sandy_Swish_punch",
        0x74 => "SM_Sandy_Swish_kick",
        0x75 => "SM_Sandy_Swish_tail",
        0x76 => "SM_Sandy_Swish_huge",
        _ => return None,
    })
}

/// The sound `FUN_1009cc50` [GC 0x1009cc50] (`WeaponItem_t` vtable `+0x94`) plays for an `attack` note when the weapon record has no `0xb` sound,
/// by the weapon's `AmmoType` (stat 420; -1 = none / melee). `0x148e160`, ... are sound ids (their names are not in the client).
/// Other ammo types: `DAT_102e339c`, which no code writes (0 in the DLL image): silence.
pub fn ammo_default(ammo: i32) -> Option<u32> {
    Some(match ammo {
        -1 => 0x0148_e160,
        1 => 0x7634_c942,
        2 => 0x2d2c_b134,
        4 => 0x7a77_a8dd,
        5 => 0xdf81_67d6,
        6 => 0x4c79_0a5d,
        10 => 0xba94_da9b,
        _ => return None,
    })
}

/// `FUN_1009b4ac` [GC 0x1009b4ac]: the impact size passed to `PlayGameSound`: damage >= `max / 10` is 2, >= `max / 20` is 1, else 0, with `max` =
/// `SimpleChar+0x218`, which only the constructor writes (`FUN_1005cb6a`: 60).
pub fn impact_size(damage: i32) -> i32 {
    if damage >= 60 / 10 {
        2
    } else if damage >= 60 / 20 {
        1
    } else {
        0
    }
}

/// Seconds the material impact sound waits: `PlayGameSound`'s delay argument `_DAT_101663d4` = 0.4f (`SandyInterface_t::PlaySample` queues it,
/// `Frameprocess` @SI 0x10003f61 counts it down).
pub const IMPACT_DELAY_S: f32 = 0.4;
/// `NpcRecord` stat 41 `FabricType`: the material of a creature's impact sounds (`FUN_1004d8e6(0x29)`).
pub const STAT_FABRIC_TYPE: u32 = 41;

/// The material and sound of a player struck by a weapon (`FUN_1009b4ac` after the creature test): Breed (stat 4) 1, 2, 3, 4 or 7 is material 7,
/// with `SM_Sandy_Game_MaleGetsHit` for Sex (stat 59) 1 / 2 and `FemaleGetsHit` for 3 (any other Sex: material without a sound).
pub fn player_impact(breed: u8, sex: u8) -> Option<(i32, Option<&'static str>)> {
    matches!(breed, 1 | 2 | 3 | 4 | 7).then(|| {
        let sound = match sex {
            1 | 2 => Some(super::anim::sound::MALE_GETS_HIT),
            3 => Some(super::anim::sound::FEMALE_GETS_HIT),
            _ => None,
        };
        (7, sound)
    })
}

/// What the last `AttackInfo` of an attacker left in its slot object (`FUN_1006a8f3`: `+0x2c` hit kind, `+0x30` damage) and who it hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitCtx {
    pub victim: i32,
    pub slot: i32,
    pub damage: i32,
    /// `AttackInfo::unk_30` (live 3 / 4); the impact part of the note needs `> 1`.
    pub flags: i32,
}

/// What an `AttackInfo` / `MissedAttackInfo` leaves for the attack notes of its swing: attacker and [`HitCtx`]. A miss runs the same routine
/// (`FUN_1006ae50` -> `FUN_1006a8f3(slot, 0, value_1c, 0, 1, 0)`): damage 0, hit kind 1, so the swing and its notes play, the impact does not.
pub fn hit_of(e: &super::state::CombatEvent) -> Option<(i32, HitCtx)> {
    use super::state::CombatEvent as E;
    match *e {
        E::Hit { attacker, victim, damage, slot, flags } => Some((attacker, HitCtx { victim, slot, damage, flags })),
        E::Miss { attacker, target, slot } => Some((attacker, HitCtx { victim: target, slot, damage: 0, flags: 1 })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_and_misses_both_start_a_swing_with_notes() {
        use super::super::state::CombatEvent as E;
        assert_eq!(hit_of(&E::Hit { attacker: 1, victim: 2, damage: 7, slot: 3, flags: 4 }), Some((1, HitCtx { victim: 2, slot: 3, damage: 7, flags: 4 })));
        assert_eq!(hit_of(&E::Miss { attacker: 1, target: 2, slot: 3 }), Some((1, HitCtx { victim: 2, slot: 3, damage: 0, flags: 1 })));
        assert_eq!(hit_of(&E::Died { dynel: 1, cause: 0 }), None);
    }

    #[test]
    fn names_map_to_note_ids_with_the_clients_prefix_order() {
        for (n, want) in [
            ("attack", 0xb),
            ("attack\r\n", 0xb),
            ("attack_start_1", 0x77),
            ("attack_start_9", 0x7f),
            ("attack_effect_3", 0x90),
            ("swish_punch", 0x73),
            ("swish_kick", 0x74),
            ("swish_whip", 0x75),
            ("swish_huge", 0x76),
            ("swich_kick", 0), // the data's misspelling matches nothing
            ("step", 0x26),
            ("stepfast", 0x26),
            ("left", 0x26),
            ("right", 0x26),
            ("land", 0x85),
            ("idle_combat", 0xf),
            ("enter_combat", 0x80),
            ("itemshow_b", 0x41),
            ("loopstart\r\n", 0),
            ("loopend", 0),
            ("", 0),
        ] {
            assert_eq!(note_id(n), want, "{n:?}");
        }
    }

    #[test]
    fn a_note_fires_once_when_its_time_is_reached() {
        let ev = vec![(100, "attack_start_1".to_string()), (400, "attack".to_string()), (400, "swish_punch".to_string()), (900, "loopend".to_string())];
        let mut fired = 0;
        assert_eq!(fire(&ev, 50.0, &mut fired).collect::<Vec<_>>(), Vec::<u32>::new());
        assert_eq!(fire(&ev, 100.0, &mut fired).collect::<Vec<_>>(), [0x77]);
        assert_eq!(fire(&ev, 399.0, &mut fired).collect::<Vec<_>>(), Vec::<u32>::new());
        assert_eq!(fire(&ev, 420.0, &mut fired).collect::<Vec<_>>(), [0xb, 0x73]);
        assert_eq!(fire(&ev, 2000.0, &mut fired).collect::<Vec<_>>(), Vec::<u32>::new(), "loopend has no note and nothing fires twice");
    }

    #[test]
    fn interrupted_holder_flushes_only_pending_attack_markers() {
        let events = vec![(133, "attack_effect_1".into()), (233, "attack_effect_2".into()), (333, "attack_effect_3".into()), (433, "attack_effect_4".into()), (500, "swish_punch".into())];
        let mut fired = 0;
        assert_eq!(fire(&events, 150.0, &mut fired).collect::<Vec<_>>(), [id::ATTACK_EFFECT_1]);
        assert_eq!(finish(&events, &mut fired).collect::<Vec<_>>(), [id::ATTACK_EFFECT_1 + 1, id::ATTACK_EFFECT_1 + 2, id::ATTACK_EFFECT_4]);
        assert_eq!(finish(&events, &mut fired).count(), 0);
    }

    #[test]
    fn impact_rules() {
        assert_eq!([1, 2, 3, 5, 6, 40].map(impact_size), [0, 0, 1, 1, 2, 2]);
        assert_eq!(ammo_default(-1), Some(0x0148e160));
        assert_eq!(ammo_default(3), None);
        assert_eq!(player_impact(4, 3), Some((7, Some("SM_Sandy_Game_FemaleGetsHit"))));
        assert_eq!(player_impact(1, 1), Some((7, Some("SM_Sandy_Game_MaleGetsHit"))));
        assert_eq!(player_impact(2, 0), Some((7, None)));
        assert_eq!(player_impact(5, 1), None, "creature breeds have no player flesh sound");
        assert_eq!(swish_name(0x75), Some("SM_Sandy_Swish_tail"));
        assert_eq!(swish_name(0xb), None);
    }

    /// Real clips: every swing clip a player can be shown (the weapon `attack` lists of every `AnimSet`, left and right hand, and the bare-hand attacks
    /// 1033..1037) of the male / female / athrox sets carries an `attack` or a `swish_*` note, which is what starts its sound (docs/zone/combat-anim.md
    /// section 6); the clip's `attack` note is not at time 0.
    #[test]
    fn player_swing_clips_carry_the_notes_that_start_the_sounds() {
        use super::super::anim::{resolve_clip, weapon_list, CLIP_TYPE};
        use ao_formats::character::{CatAnim, NameTable};
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")).filter(|d| d.join("cd_image/rdb.db").exists()) else { return };
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let names = NameTable::load(&store).unwrap();
        let mut ids: Vec<u16> = (1033..=1037).collect();
        for set in [0, 1, 2, 3, 6, 7, 8] {
            for left in [false, true] {
                ids.extend(weapon_list(set, left, false, super::super::anim::list::ATTACK));
            }
        }
        ids.sort_unstable();
        ids.dedup();
        let (mut with_attack, mut with_swish, mut total, mut silent) = (0, 0, 0, vec![]);
        for set in ["male", "female", "athrox"] {
            for &id in &ids {
                let Some((clip, ..)) = resolve_clip(&names, set, id, false) else { continue };
                let a = CatAnim::parse(&store.get(CLIP_TYPE, clip).unwrap().unwrap()).unwrap();
                let notes: Vec<u32> = a.events.iter().map(|e| note_id(&e.1)).collect();
                total += 1;
                let attack = a.events.iter().find(|e| note_id(&e.1) == id::ATTACK);
                with_attack += usize::from(attack.is_some());
                with_swish += usize::from(notes.iter().any(|n| (id::SWISH_PUNCH..=id::SWISH_HUGE).contains(n)));
                if attack.is_none() && !notes.iter().any(|n| (id::SWISH_PUNCH..=id::SWISH_HUGE).contains(n)) {
                    silent.push((set, id, a.events.iter().map(|e| e.1.clone()).collect::<Vec<_>>()));
                }
                if let Some(e) = attack {
                    assert!(e.0 > 0 && (e.0 as f32) < a.duration, "{set} {id}: attack note at {} of {}", e.0, a.duration);
                }
            }
        }
        eprintln!("{total} swing clips, {with_attack} with an attack note, {with_swish} with a swish note, without either: {silent:?}");
        assert!(total > 30 && with_attack * 10 > total * 8, "{with_attack} of {total}");
    }
}
