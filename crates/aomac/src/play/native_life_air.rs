//! Gamecode air-life steering: 10118330, 101198dc, 10119e57, 1011cace.
use super::{Context, Species, Steering};
use ao_formats::wildlife::LifeKind;
use glam::Vec3;

pub(super) fn configure(s: &mut Species) {
    s.state = 0;
    s.timer = 0.0;
    s.target = Vec3::ZERO;
    s.surface_hug = false;
    s.falling = false;
    s.animation = 1;
    match s.kind {
        LifeKind::Butterfly | LifeKind::Pollen => {
            s.max_speed = 1.8; s.max_force = 16.0; s.mass = 0.1; s.brake_distance = 0.1;
        }
        LifeKind::Fly => {
            s.max_speed = 1.8; s.max_force = 14.0; s.mass = 0.1; s.brake_distance = 0.1;
        }
        LifeKind::Hoverbot => {
            s.max_speed = 1.6; s.max_force = 10.0; s.mass = 2.0; s.brake_distance = 1.0;
            s.counter = ((s.random() * 32767.0) as u32) % 5;
        }
        _ => unreachable!(),
    }
}

pub(super) fn steer(s: &mut Species, c: &Context<'_>) -> Steering {
    match s.kind {
        LifeKind::Butterfly => butterfly(s, c),
        LifeKind::Fly => fly(s, c),
        LifeKind::Hoverbot => hoverbot(s, c),
        LifeKind::Pollen => pollen(s, c),
        _ => unreachable!(),
    }
}

fn boundary(s: &mut Species, c: &Context<'_>, force: &mut Vec3) -> bool {
    if let Some(v) = s.constrain_district(c) {
        s.wander = v * 0.5;
        return s.accumulate(force, v) > 0.99;
    }
    false
}

fn butterfly(s: &mut Species, c: &Context<'_>) -> Steering {
    let mut force = Vec3::ZERO;
    match s.state {
        0 => {
            s.timer += c.dt;
            if s.timer > 0.5 {
                s.timer = 0.0;
                let ground = c.environment.ground(s.pos).map(|v| v.0).unwrap_or(Vec3::ZERO);
                let height = s.pos.y - ground.y;
                if c.environment.liquid(s.pos).is_some_and(|water| water > 0.0 && s.pos.y - (water + 0.2) < 0.9) {
                    s.wander.x *= 0.5; s.wander.y = s.max_force * 0.2; s.wander.z *= 0.5;
                } else if height < 0.3 {
                    s.target = ground; s.state = 1;
                } else if height > 3.0 {
                    s.wander.y = (3.0 - height) * 0.25;
                }
            }
        }
        1 => {
            let result = s.arrive(s.target, s.body_radius + 0.05);
            if matches!(result, Steering::Halt) { s.animate(0, false); s.state = 2; }
            return result;
        }
        2 => {
            s.timer += c.dt;
            if s.timer > 0.5 {
                s.timer = 0.0;
                if s.random() < 0.1 || s.flee(c, 2.0).is_some() {
                    s.animate(1, false); s.state = 3; s.wander = Vec3::Y;
                }
            }
            return Steering::None;
        }
        3 => {
            s.timer += c.dt;
            if s.timer > 0.5 {
                s.timer = 0.0;
                let ground = c.environment.ground(s.pos).map(|v| v.0).unwrap_or(Vec3::ZERO);
                if s.pos.y - ground.y > 0.3 { s.state = 0; s.target = ground; }
            }
            s.accumulate(&mut force, Vec3::Y * (s.max_force * 0.2));
            if s.wander.y <= 0.0 { s.wander.y = 1.0; }
        }
        _ => {}
    }
    if !boundary(s, c, &mut force) {
        let wander = s.wander3(6.5, 1.7, 1.4);
        s.accumulate(&mut force, wander);
    }
    Steering::Force(force * s.max_force)
}

fn pollen(s: &mut Species, c: &Context<'_>) -> Steering {
    s.timer += c.dt;
    if s.timer > 0.5 {
        s.timer = 0.0;
        let height = s.pos.y - c.environment.ground(s.pos).map(|v| v.0.y).unwrap_or(0.0);
        if c.environment.liquid(s.pos).is_some_and(|water| water > 0.0 && s.pos.y - (water + 0.2) < 0.9) {
            s.wander.x *= 0.5; s.wander.y = s.max_force * 0.2; s.wander.z *= 0.5;
        } else if height < 0.3 || height > 3.0 {
            s.wander.y = ((if height < 0.3 { 0.3 } else { 3.0 }) - height) * 0.25;
        }
    }
    let mut force = Vec3::ZERO;
    if !boundary(s, c, &mut force) {
        let wander = s.wander3(6.5, 1.7, 1.4);
        s.accumulate(&mut force, wander);
    }
    Steering::Force(force * s.max_force)
}

fn fly(s: &mut Species, c: &Context<'_>) -> Steering {
    let mut force = Vec3::ZERO;
    s.timer += c.dt;
    if s.timer > 1.0 {
        s.timer = 0.0;
        let height = s.pos.y - c.environment.ground(s.pos).map(|v| v.0.y).unwrap_or(0.0);
        if c.environment.liquid(s.pos).is_some_and(|water| water > 0.0 && s.pos.y - (water + 0.2) < 0.4) {
            s.wander.x *= 0.2; s.wander.y = s.max_force; s.wander.z *= 0.2;
            if s.accumulate(&mut force, s.wander) > 0.99 { return Steering::Force(force * s.max_force); }
        } else if height < 0.16 {
            s.wander.y = 6.0;
        } else if height > 3.0 {
            s.wander.y = (3.0 - height) * 2.0;
        } else {
            s.liquid_escape = false;
            for &corpse in c.environment.corpses() {
                if corpse.distance_squared(s.pos) <= 18.0 * 18.0 && c.environment.district(corpse) == Some(s.district) {
                    s.target = corpse + Vec3::Y * 0.3; s.liquid_escape = true; break;
                }
            }
        }
    }
    if boundary(s, c, &mut force) { return Steering::Force(force * s.max_force); }
    if s.group.is_some() {
        if let Some(v) = s.separation(c, 0.1, std::f32::consts::PI) {
            if s.accumulate(&mut force, v) > 0.99 { return Steering::Force(force * s.max_force); }
        }
        if let Some(v) = s.cohesion(c, 12.0, std::f32::consts::PI) {
            if s.accumulate(&mut force, v) > 0.99 { return Steering::Force(force * s.max_force); }
        }
    }
    let mut wander = s.wander3(6.6, 1.0, 1.2);
    wander.y *= 0.25;
    s.accumulate(&mut force, wander);
    if s.liquid_escape {
        force *= 0.6;
        s.accumulate(&mut force, s.target - s.pos);
    }
    Steering::Force(force * s.max_force)
}

fn hover_height(height: f32) -> f32 {
    // 10119f4e deliberately uses max-height, not min-height, below the lower bound.
    if height <= 3.0 { 12.0 } else if height > 6.0 { (6.0 - height) * 2.0 } else { (4.5 - height) * 2.0 }
}

fn hoverbot(s: &mut Species, c: &Context<'_>) -> Steering {
    s.counter += 1;
    match s.state {
        0 => {
            s.timer += c.dt;
            if s.timer > 0.25 {
                s.timer = 0.0;
                s.wander.y = hover_height(s.pos.y - c.environment.ground(s.pos).map(|v| v.0.y).unwrap_or(0.0));
                if s.random() < 0.005 { s.target = s.pos; s.state = 2; }
            }
        }
        1 => {
            if s.random() < 0.008 { s.animate(1, true); s.state = 0; }
            return Steering::None;
        }
        2 => {
            let result = s.arrive(s.target, s.body_radius + 0.05);
            if matches!(result, Steering::Halt) { s.animate(0, true); s.state = 1; }
            return result;
        }
        _ => {}
    }
    let mut force = Vec3::ZERO;
    if s.counter >= 5 {
        s.counter = 0;
        if let Some(v) = s.obstacle(c) {
            if v.length_squared() > s.max_force * s.max_force * 0.8 { s.wander = v; }
            if s.accumulate(&mut force, v) > 0.99 { return Steering::Force(force * s.max_force); }
        }
    }
    if !boundary(s, c, &mut force) {
        let wander = s.wander3(8.0, 0.8, 1.0);
        s.accumulate(&mut force, wander);
    }
    Steering::Force(force * s.max_force)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_hover_height_bounds() {
        assert_eq!(hover_height(3.0), 12.0);
        assert_eq!(hover_height(3.5), 2.0);
        assert_eq!(hover_height(4.5), 0.0);
        assert_eq!(hover_height(6.0), -3.0);
        assert_eq!(hover_height(7.0), -2.0);
        assert_eq!(LifeKind::from_native(2), Some(LifeKind::Butterfly));
        assert_eq!(LifeKind::from_native(3), Some(LifeKind::Fly));
        assert_eq!(LifeKind::from_native(7), Some(LifeKind::Hoverbot));
        assert_eq!(LifeKind::from_native(10), Some(LifeKind::Pollen));
    }
    struct Sensors { ground: f32, water: Option<f32>, corpses: Vec<Vec3> }
    impl super::super::Environment for Sensors {
        fn align(&self, old: Vec3, new: Vec3, body: &super::super::Body, state: &mut super::super::SurfaceState) -> super::super::Aligned {
            super::super::Environment::align(&super::super::tests::Flat, old, new, body, state)
        }
        fn ground(&self, p: Vec3) -> Option<(Vec3, Vec3)> { Some((Vec3::new(p.x, self.ground, p.z), Vec3::Y)) }
        fn ground_ray(&self, p: Vec3) -> Option<(Vec3, Vec3)> { self.ground(p) }
        fn alignment(&self, _: Vec3) -> Vec3 { Vec3::Y }
        fn liquid(&self, _: Vec3) -> Option<f32> { self.water }
        fn district(&self, _: Vec3) -> Option<usize> { Some(0) }
        fn obstacle(&self, _: Vec3, _: Vec3) -> Option<(Vec3, Vec3)> { None }
        fn characters(&self) -> &[super::super::CharacterSensor] { &[] }
        fn dangers(&self) -> &[super::super::CharacterSensor] { &[] }
        fn corpses(&self) -> &[Vec3] { &self.corpses }
        fn wind(&self) -> Vec3 { Vec3::ZERO }
        fn landing(&self, _: Vec3, _: f32) -> Option<Vec3> { None }
    }
    #[test]
    fn native_air_states_and_sensors() {
        let env = Sensors { ground: 0.0, water: None, corpses: vec![Vec3::new(1.0, 0.0, 0.0)] };
        let c = Context { dt: 0.51, environment: &env, neighbors: &[] };
        let mut butterfly = Species::new(LifeKind::Butterfly, 0, Vec3::Y * 0.2, 0, 1);
        steer(&mut butterfly, &c);
        assert_eq!(butterfly.state, 1);
        butterfly.pos = butterfly.target;
        assert!(matches!(steer(&mut butterfly, &c), Steering::Halt));
        assert_eq!((butterfly.state, butterfly.animation), (2, 0));
        butterfly.state = 3; butterfly.pos.y = 0.4;
        steer(&mut butterfly, &c);
        assert_eq!(butterfly.state, 0);
        let mut hover = Species::new(LifeKind::Hoverbot, 0, Vec3::Y * 4.5, 0, 1);
        hover.state = 2; hover.target = hover.pos;
        assert!(matches!(steer(&mut hover, &c), Steering::Halt));
        assert_eq!((hover.state, hover.pending_animation), (1, Some(0)));
        let mut fly = Species::new(LifeKind::Fly, 0, Vec3::Y, 0, 1);
        let fly_context = Context { dt: 1.01, ..c };
        steer(&mut fly, &fly_context);
        assert!(fly.liquid_escape);
        assert_eq!(fly.target, Vec3::new(1.0, 0.3, 0.0));
        for kind in [LifeKind::Butterfly, LifeKind::Fly, LifeKind::Hoverbot, LifeKind::Pollen] {
            let s = Species::new(kind, 0, Vec3::ZERO, 0, 1);
            assert!(!s.surface_hug && !s.falling);
            assert_eq!(s.animation, 1);
        }
    }
}
