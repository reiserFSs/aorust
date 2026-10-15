//! Native mech-order guard (`Gamecode.dll` 0x10079707).
//! Unknown server/resource state is an error, never permission to send Spawn.
use anyhow::{Context as _, Result};

#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    /// Locks_t membership for skill 662 (GC 0x10063be4), not expiry comparison.
    pub skill_locked: Option<bool>,
    pub dungeon: Option<bool>,
    /// Vehicle_t +0x50: zero is the native falling refusal.
    pub falling_enabled: Option<bool>,
    /// Playfield resource +0x5b bit 0 clear AND !IsShadowlandPF.
    pub vehicles_allowed: Option<bool>,
    /// Own GetStat(247, 2), GC 0x10062526.
    pub resurrection_shock: Option<i32>,
    pub battlestation: Option<bool>,
    /// GC 0x101295f3: own-position district exists; LC min/max (+0x44/+0x48) both nonzero.
    pub own_lca: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Silent,
    Feedback(&'static str),
}

/// Checks in native order; `vehicle == 0` is the leave-mech path.
/// The caller must separately perform the native perk lookup and Spawn acceptance.
pub fn check(vehicle: i32, perk_flags: u32, state: &Context) -> Result<Verdict> {
    if state.skill_locked.context("vehicle activation requires the native incoming Locks_t state for skill 662")? {
        return Ok(Verdict::Silent);
    }
    if vehicle == 0 { return Ok(Verdict::Allowed); }
    if state.dungeon.context("vehicle activation requires the loaded native playfield dungeon state")? {
        return Ok(Verdict::Feedback("Feedback_MustBeOutside"));
    }
    if !state.falling_enabled.context("vehicle activation requires the own native movement falling-enabled state")? {
        return Ok(Verdict::Feedback("Feedback_YouCantDoThisWhileFalling"));
    }
    if !state.vehicles_allowed.context("vehicle activation requires playfield resource +0x5b and native shadowland classification")? {
        return Ok(Verdict::Feedback("Feedback_MechCantBeOrdered"));
    }
    if state.resurrection_shock.context("vehicle activation requires own GetStat(247, 2)")? != 0 {
        return Ok(Verdict::Feedback("Feedback_NotWhileInResurrectionShock"));
    }
    if perk_flags & 0x100 != 0
        && !state.battlestation.context("vehicle activation requires native battlestation classification")?
        && !state.own_lca.context("vehicle activation requires the native own-position district LC min/max query")?
    {
        return Ok(Verdict::Feedback("Feedback_OnlyInOwnLCArea"));
    }
    Ok(Verdict::Allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> Context {
        Context { skill_locked: Some(false), dungeon: Some(false), falling_enabled: Some(true), vehicles_allowed: Some(true), resurrection_shock: Some(0), battlestation: Some(false), own_lca: Some(true) }
    }
    #[test]
    fn native_refusal_order_and_missing_prerequisites() {
        let mut s = Context { skill_locked: Some(true), ..Context::default() };
        assert_eq!(check(1, 0x100, &s).unwrap(), Verdict::Silent);
        s.skill_locked = Some(false);
        assert_eq!(check(0, 0x100, &s).unwrap(), Verdict::Allowed);
        assert!(check(1, 0, &s).is_err());
        s.dungeon = Some(true);
        assert_eq!(check(1, 0, &s).unwrap(), Verdict::Feedback("Feedback_MustBeOutside"));
        s = ready(); s.falling_enabled = Some(false);
        assert_eq!(check(1, 0, &s).unwrap(), Verdict::Feedback("Feedback_YouCantDoThisWhileFalling"));
        s = ready(); s.vehicles_allowed = Some(false);
        assert_eq!(check(1, 0, &s).unwrap(), Verdict::Feedback("Feedback_MechCantBeOrdered"));
        s = ready(); s.resurrection_shock = Some(1);
        assert_eq!(check(1, 0, &s).unwrap(), Verdict::Feedback("Feedback_NotWhileInResurrectionShock"));
        s = ready(); s.own_lca = Some(false);
        assert_eq!(check(1, 0x100, &s).unwrap(), Verdict::Feedback("Feedback_OnlyInOwnLCArea"));
        s.own_lca = None;
        assert!(check(1, 0x100, &s).is_err());
        s.battlestation = Some(true);
        assert_eq!(check(1, 0x100, &s).unwrap(), Verdict::Allowed);
        s.battlestation = None;
        assert_eq!(check(1, 0, &s).unwrap(), Verdict::Allowed);
        assert!(check(1, 0, &Context::default()).is_err());
    }
}
