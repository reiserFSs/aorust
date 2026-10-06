//! Values of the health / nano / XP / alien XP bars (`CharacterBar_c` slots `FUN_100666b6`, `FUN_100667a8`, `FUN_100668aa`, `FUN_100669f7`, GUI.dll).
//!
//! Every slot reads `cur` and `max` through `N3Msg_GetSkill(stat, 2)`, computes `r = (float)cur / (float)max` and, only when `1.0 < r`, replaces it by
//! 1.0 (`SetValue(Variant(r))` on the `PowerbarView_c`, tooltip `"%d / %d"` = `cur`, `max`). Nothing clamps a negative value (we clamp to 0, UNRESOLVED), nothing guards `max == 0`
//! (`cur / 0` = +inf → the bar is full, `0 / 0` = NaN → UNRESOLVED how `PowerbarView_c::SetValue` paints it; we draw an empty bar).
//! PRK's `FullCharacter` carries `Life` (1) = 1 and `MaxNanoEnergy` (221) = 1; the client overwrites both raw stats using its own formula
//! (`FUN_1006208d`, [`ao_formats::stats::pools`], docs/gui.md 10.4) whenever BodyDevelopment / NanoPool change. [`Pools::apply`] uses the shared
//! modifier-aware skill values before the own skill projection and all HUD bars are refreshed.

use super::zone::Zone;
use ao_formats::stats::{pools::{self, PoolTables}, skills::Character};
use std::path::Path;

/// The tables of `FUN_1006208d` (empty when the client's rdb is missing: the stats stay as the server sent them).
pub(super) struct Pools(Option<PoolTables>);

impl Pools {
    pub(super) fn new(dir: &Path) -> Self {
        let load = || -> anyhow::Result<_> {
            let store = ao_rdb::RecordStore::open(dir)?;
            PoolTables::load(&store)
        };
        Self(load().map_err(|e| eprintln!("hud: pool tables: {e:#}")).ok())
    }

    /// Stores the computed raw maxima using the same buffed BodyDevelopment / NanoPool as `GetStat(skill, 0)` (`FUN_1006208d`).
    pub(super) fn apply(&self, zone: &mut Zone, skill: &dyn Fn(&Zone, u32) -> i32) {
        let Some(pool) = &self.0 else { return };
        if zone.stat(0x36).is_none() {
            return;
        }
        let (life, nano) = pool.max_pools(&Character::from_stats(|s| zone.stat(s)), skill(zone, pools::BODY_DEV), skill(zone, pools::NANO_POOL));
        zone.stats.insert(pools::LIFE, life);
        zone.stats.insert(pools::MAX_NANO, nano);
    }
}

/// `r = cur / max` of the slots; see the module docs.
pub(super) fn ratio(cur: i32, max: i32) -> f32 {
    let r = cur as f32 / max as f32;
    if r.is_nan() {
        0.0
    } else {
        r.clamp(0.0, 1.0)
    }
}

/// The XP bar's `(cur, max)` (`FUN_100668aa`): `max = NextXP(0x15e) - base`, `base = LevelStartXP (0x39)`; below level 200 `cur = XP (0x34) - base`;
/// from level 200 on `cur = 0x23d` and `base` is 0 unless `stat(0x240) & 1`.
pub(super) fn xp(stat: impl Fn(u32) -> i32) -> (i32, i32) {
    let (next, mut base) = (stat(0x15e), stat(0x39));
    let cur = if stat(0x36) < 200 {
        stat(0x34)
    } else {
        if stat(0x240) & 1 == 0 {
            base = 0;
        }
        stat(0x23d)
    };
    (cur - base, next - base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn life_one_fills_the_bar_and_zero_max_does_not_panic() {
        assert_eq!(ratio(34, 1), 1.0);
        assert_eq!(ratio(34, 0), 1.0, "cur / 0 = +inf is clamped by `1.0 < r`");
        assert_eq!(ratio(0, 0), 0.0);
        assert_eq!(ratio(25, 100), 0.25);
    }

    #[test]
    fn xp_follows_the_level_200_branch() {
        let low = |id: u32| match id {
            0x15e => 1500,
            0x39 => 500,
            0x36 => 10,
            0x34 => 750,
            _ => 0,
        };
        assert_eq!(xp(low), (250, 1000));
        let high = |id: u32| match id {
            0x15e => 9000,
            0x39 => 4000,
            0x36 => 200,
            0x23d => 6000,
            0x240 => 1,
            _ => 0,
        };
        assert_eq!(xp(high), (2000, 5000));
        let no_base = |id: u32| if id == 0x240 { 0 } else { high(id) };
        assert_eq!(xp(no_base), (6000, 9000));
    }

    /// The live fight capture (Aomacvolk, level 1): the server's `Life` / `MaxNanoEnergy` are 1, the client formula gives the 34 / 32 of the dynel header
    /// and `CurrentNano`, so the bars read 34 / 34 and 32 / 32 and a hit of 7 drops the health bar to 27 / 34.
    #[test]
    fn fight_capture_pools_are_34_and_32() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let mut z = Zone::new(0x82e8);
        for l in include_str!("../../../../docs/captures/zone_fight_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir == "<" {
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                z.on_frame(&ao_net::frame::Frame::decode_with(&b, false).unwrap().unwrap().0);
            }
        }
        assert_eq!((z.stat(1), z.stat(221)), (Some(1), Some(1)), "what PRK sends");
        let skills = ao_formats::stats::skills::SkillTables::load(&ao_rdb::RecordStore::open(&dir).unwrap()).unwrap();
        let skill = |z: &Zone, id| z.stat(id).unwrap_or(0) + skills.trickle(id, Character::from_stats(|s| z.stat(s)).abilities) as i32;
        Pools::new(&dir).apply(&mut z, &skill);
        assert_eq!((z.stat(1), z.stat(27), z.stat(221), z.stat(214)), (Some(34), Some(34), Some(32), Some(32)));
        assert_eq!(ratio(z.stat(27).unwrap(), z.stat(1).unwrap()), 1.0);
        z.stats.insert(27, 27);
        Pools::new(&dir).apply(&mut z, &skill);
        assert!((ratio(z.stat(27).unwrap(), z.stat(1).unwrap()) - 27.0 / 34.0).abs() < 1e-6, "a hit of 7 must show");
        Pools::new(&dir).apply(&mut z, &|z, id| skill(z, id) + if id == pools::BODY_DEV { 10 } else { 0 });
        assert_eq!((z.stat(1), z.stat(221)), (Some(64), Some(32)), "buffed BodyDevelopment adds three Life per point for Solitus");
    }
}
