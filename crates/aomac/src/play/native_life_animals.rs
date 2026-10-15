//! Gamecode.dll Bird 1011798c, Rabbit 1011d0be, Fish 10118fc3.
use super::{Context, Species, Steering};
use ao_formats::wildlife::LifeKind;
use glam::Vec3;

pub(super) fn configure(s: &mut Species) {
    let (speed, force, brake, ground) = match s.kind {
        LifeKind::Bird => (9.0, 16.0, 4.0, false),
        LifeKind::Rabbit => (2.3, 20.0, 0.5, true),
        LifeKind::Fish => (3.0, 14.0, 1.0, false),
        _ => unreachable!(),
    };
    s.max_speed = speed;
    s.max_force = force;
    s.mass = 1.0;
    s.brake_distance = brake;
    s.surface_hug = ground;
    s.falling = ground;
    s.state = if ground { 2 } else { 0 };
    // Mesh attachment slots40: Bird101174ee, Rabbit1011cea4, Fish10118dcf.
    s.animation = if ground { 0 } else { 1 };
    s.counter = if s.kind == LifeKind::Fish { 0 } else { (s.random() * 32767.0) as u32 % 5 };
}

pub(super) fn steer(s: &mut Species, c: &Context<'_>) -> Steering {
    match s.kind {
        LifeKind::Bird => bird(s, c),
        LifeKind::Rabbit => rabbit(s, c),
        LifeKind::Fish => fish(s, c),
        _ => unreachable!(),
    }
}

fn add(s: &Species, force: &mut Vec3, candidate: Option<Vec3>) -> bool {
    candidate.is_some_and(|v| s.accumulate(force, v) > 0.9900000095367432_f64 as f32)
}
fn result(s: &Species, force: Vec3) -> Steering { Steering::Force(force * s.max_force) }
fn bird_animation(s: &mut Species) {
    let descending = s.vel.y < 0.0;
    if descending != s.descending_animation {
        s.animate(if descending { 2 } else { 1 }, true);
        s.descending_animation = descending;
    }
}
fn takeoff(s: &mut Species) {
    s.state = 3;
    s.wander = Vec3::Y;
    s.animate(1, false);
}

fn bird(s: &mut Species, c: &Context<'_>) -> Steering {
    let mut force = Vec3::ZERO;
    s.counter += 1;
    match s.state {
        0 => {
            s.timer += c.dt;
            if s.timer > 0.5 {
                s.timer = 0.0;
                {
                    let ground = c.environment.ground(s.pos).map_or(Vec3::ZERO, |(point, _)| point);
                    let height = s.pos.y - ground.y;
                    if c.environment.liquid(Vec3::new(s.pos.x, 0.01, s.pos.z)).is_some_and(|water| water > 0.0 && s.pos.y - (water + 0.2) < 4.5) {
                        s.liquid_escape = true;
                        s.wander.x *= 0.2;
                        s.wander.y = s.max_force;
                        s.wander.z *= 0.2;
                    } else if height < 1.5 {
                        s.target = ground;
                        s.liquid_escape = false;
                        s.state = 1;
                    } else if height > 55.0 {
                        s.liquid_escape = false;
                        s.wander.y = (55.0 - height) * 0.25;
                    } else if s.random() < 0.07 {
                        if let Some(target) = c.environment.landing(s.pos, 18.0) {
                            if c.environment.liquid(target).unwrap_or(0.0) <= 0.0 {
                                s.target = target;
                                s.liquid_escape = false;
                                s.state = 1;
                            }
                        }
                    } else {
                        s.liquid_escape = false;
                    }
                }
            }
            bird_animation(s);
        }
        1 => {
            let mut steering = s.arrive(s.target, s.body_radius + 0.05000000074505806_f64 as f32);
            if matches!(steering, Steering::Halt) {
                if s.vel.length_squared() != 0.0 {
                    // Vehicle::Halt 1000a688 dispatches Bird slot26 1011759f.
                    s.halt();
                    s.animate(0, false);
                    s.state = 2;
                } else {
                    steering = Steering::None;
                }
            }
            bird_animation(s);
            return steering;
        }
        2 => {
            s.timer += c.dt;
            if s.timer > 0.25 {
                s.timer = 0.0;
                let random = s.random();
                let startled = random < 0.004 || s.flee(c, 4.0).is_some()
                    || c.environment.dangers().iter().any(|danger| s.pos.distance_squared(danger.pos) < danger.radius * danger.radius);
                let nearby = |n: &&super::Neighbor| n.group == s.group && n.pos != s.pos && n.pos.distance_squared(s.pos) < 18.0 * 18.0;
                let count = c.neighbors.iter().filter(nearby).count();
                let flock_takeoff = s.group.is_some() && count != 0 && random < 2.0 / count as f32
                    && c.neighbors.iter().filter(nearby).any(|n| n.state == 3);
                if startled || flock_takeoff { takeoff(s); }
            }
            return Steering::None;
        }
        3 => {
            s.timer += c.dt;
            if s.timer > 0.5 {
                s.timer = 0.0;
                if s.pos.y - c.environment.ground(s.pos).map_or(0.0, |(ground, _)| ground.y) > 1.5 { s.state = 0; }
            }
            if s.vel.y < 0.0 { s.accumulate(&mut force, Vec3::Y * s.max_force * 0.30000001192092896_f64 as f32); }
            if s.wander.y <= 0.0 { s.wander.y = 1.0; }
        }
        _ => unreachable!(),
    }
    if s.counter > 4 {
        s.counter = 0;
        if let Some(v) = s.obstacle(c) {
            if v.length_squared() > s.max_force * s.max_force * 0.800000011920929_f64 as f32 { s.wander = v; }
            if add(s, &mut force, Some(v)) { return result(s, force); }
        }
    }
    if s.liquid_escape && s.vel.y < 0.0 && add(s, &mut force, Some(Vec3::Y * s.vel.y * -1.5)) { return result(s, force); }
    if add(s, &mut force, s.flee(c, 4.0)) { return result(s, force); }
    if let Some(v) = s.constrain_district(c) {
        s.wander = v * 0.5;
        if add(s, &mut force, Some(v)) { return result(s, force); }
    }
    if s.group.is_some() {
        if add(s, &mut force, s.separation(c, 1.5, 2.4)) { return result(s, force); }
        if add(s, &mut force, s.cohesion(c, 18.0, 2.4)) { return result(s, force); }
        if add(s, &mut force, s.alignment(c, 12.0, 2.4)) { return result(s, force); }
    }
    let v = s.wander3(16.0, 2.0, 3.0);
    s.accumulate(&mut force, v);
    result(s, force)
}

fn rabbit(s: &mut Species, c: &Context<'_>) -> Steering {
    let mut force = Vec3::ZERO;
    s.counter += 1;
    match s.state {
        0 => {
            s.timer += c.dt;
            if s.timer > 0.5 {
                s.timer = 0.0;
                if c.environment.liquid(s.pos).unwrap_or(0.0) <= 0.0 {
                    if !s.liquid_escape && s.random() < 0.18 { s.animate(0, true); s.state = 1; }
                    if s.lifespan != -1.0 { s.lifespan = -1.0; s.elapsed = 0.0; }
                    s.last_in_district = s.pos;
                } else {
                    if s.lifespan != 10.0 { s.lifespan = 10.0; s.elapsed = 0.0; }
                    s.wander = (s.last_in_district - s.pos) * s.max_force;
                    if add(s, &mut force, Some(s.wander)) { return result(s, force); }
                }
            }
        }
        1 if s.animation_finished() => { s.state = 2; s.halt(); return Steering::None; }
        2 => {
            s.timer += c.dt;
            if s.timer > 0.5 {
                s.timer = 0.0;
                if s.random() < 0.08 { s.animate(1, false); s.state = 0; }
                else if s.flee(c, 5.0).is_some() { s.animate(1, false); s.liquid_escape = true; s.state = 0; }
                // 1011d0be returns here when the native danger list is nonempty.
                else if c.environment.dangers().is_empty() {
                    if let Some(character) = c.environment.dangers().iter().find(|danger| s.pos.distance_squared(danger.pos) < danger.radius * danger.radius) {
                        s.wander = (s.pos - character.pos) * s.max_force;
                        s.state = 0;
                        s.animate(1, false);
                    }
                }
            }
            return Steering::None;
        }
        _ => {}
    }
    if s.counter >= 5 {
        s.counter = 0;
        if let Some(v) = s.obstacle(c) {
            if v.length_squared() > s.max_force * s.max_force * 0.800000011920929_f64 as f32 { s.wander = v; }
            if add(s, &mut force, Some(v)) { return result(s, force); }
        }
    }
    if s.liquid_escape {
        if let Some(v) = s.flee(c, 5.0) {
            if add(s, &mut force, Some(v)) { return result(s, force); }
        } else { s.liquid_escape = false; }
    }
    if let Some(v) = s.constrain_district(c) {
        s.wander = v * 0.5;
        if add(s, &mut force, Some(v)) { return result(s, force); }
    }
    let v = s.wander2(8.0, 1.0, 1.5);
    s.accumulate(&mut force, v);
    result(s, force)
}

fn fish(s: &mut Species, c: &Context<'_>) -> Steering {
    let mut force = Vec3::ZERO;
    s.timer += c.dt;
    let update = s.timer > 0.25;
    if update {
        s.timer = 0.0;
        let water = c.environment.liquid(Vec3::new(s.pos.x, 0.01, s.pos.z)).unwrap_or(0.0);
        let ground = c.environment.ground_ray(Vec3::new(s.pos.x, water, s.pos.z)).map_or(Vec3::ZERO, |(p, _)| p);
        let mut safe = true;
        if !s.liquid_escape && s.pos.y > water - 0.2 {
            s.wander.y = s.max_force * -0.5;
            s.accumulate(&mut force, s.wander);
            safe = false;
            s.liquid_escape = true;
        }
        if !s.liquid_escape && s.pos.y <= ground.y + 0.2 {
            s.wander.y = s.max_force * 0.5;
            s.accumulate(&mut force, s.wander);
            safe = false;
            s.liquid_escape = true;
        }
        if water - (0.23 + 0.2) - ground.y < 0.0 {
            let retreat = s.last_in_district - s.pos;
            if retreat.length() != 0.0 {
                let v = retreat.normalize() * s.max_force * s.max_force;
                if !s.liquid_escape { s.lifespan = 10.0; s.elapsed = 0.0; }
                s.wander = v;
                s.liquid_escape = true;
                if add(s, &mut force, Some(v)) { return result(s, force); }
            } else {
                if !s.liquid_escape { s.lifespan = 10.0; s.elapsed = 0.0; }
                safe = false;
                s.liquid_escape = true;
            }
        }
        if water - (0.23 + 0.2) - ground.y < 0.0 { safe = false; }
        if safe {
            s.last_in_district = s.pos;
            if s.liquid_escape { s.lifespan = -1.0; s.elapsed = 0.0; }
            s.liquid_escape = false;
        }
    }
    if !s.liquid_escape {
        if let Some(v) = s.constrain_district(c) {
            s.wander = v * 0.5;
            if add(s, &mut force, Some(v)) { return result(s, force); }
        }
        if s.group.is_some() {
            if add(s, &mut force, s.separation(c, 0.4, f32::INFINITY)) { return result(s, force); }
            if add(s, &mut force, s.cohesion(c, 14.0, f32::INFINITY)) { return result(s, force); }
            if add(s, &mut force, s.alignment(c, 10.0, f32::INFINITY)) { return result(s, force); }
        }
        if !update { return Steering::None; }
        let v = s.wander3(12.0, 1.3, 2.0);
        s.accumulate(&mut force, v);
    } else {
        if !update { return Steering::None; }
        s.accumulate(&mut force, s.wander);
    }
    result(s, force)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{CharacterSensor, Environment};

    struct Water;
    impl Environment for Water {
        fn align(&self, old: Vec3, new: Vec3, body: &super::super::Body, state: &mut super::super::SurfaceState) -> super::super::Aligned {
            super::super::Environment::align(&super::super::tests::Flat, old, new, body, state)
        }
        fn ground(&self, p: Vec3) -> Option<(Vec3, Vec3)> { Some((Vec3::new(p.x, 0.0, p.z), Vec3::Y)) }
        fn ground_ray(&self, p: Vec3) -> Option<(Vec3, Vec3)> { self.ground(p) }
        fn liquid(&self, _: Vec3) -> Option<f32> { Some(3.0) }
        fn district(&self, _: Vec3) -> Option<usize> { Some(0) }
        fn alignment(&self, _: Vec3) -> Vec3 { Vec3::Y }
        fn obstacle(&self, _: Vec3, _: Vec3) -> Option<(Vec3, Vec3)> { None }
        fn characters(&self) -> &[CharacterSensor] { &[] }
        fn dangers(&self) -> &[CharacterSensor] { &[] }
        fn corpses(&self) -> &[Vec3] { &[] }
        fn wind(&self) -> Vec3 { Vec3::ZERO }
        fn landing(&self, _: Vec3, _: f32) -> Option<Vec3> { None }
    }

    #[test]
    fn native_species_state_and_water_math() {
        let c = Context { dt: 0.25, environment: &Water, neighbors: &[] };
        let mut fish = Species::new(LifeKind::Fish, 0, Vec3::new(0.0, 2.9, 0.0), 0, 1);
        assert!(matches!(steer(&mut fish, &c), Steering::None));
        assert!(!fish.liquid_escape);
        let Steering::Force(force) = steer(&mut fish, &c) else { panic!("surface avoidance must steer") };
        assert!(fish.liquid_escape);
        assert_eq!(fish.wander.y, -7.0);
        assert_eq!(force, Vec3::new(0.0, -14.0, 0.0));
        let mut rabbit = Species::new(LifeKind::Rabbit, 0, Vec3::ZERO, 0, 1);
        rabbit.state = 1;
        rabbit.animation = 0;
        rabbit.vel = Vec3::X;
        assert!(matches!(steer(&mut rabbit, &c), Steering::None));
        assert_eq!(rabbit.state, 2);
        assert_eq!(rabbit.vel, Vec3::ZERO);
        let mut bird = Species::new(LifeKind::Bird, 0, Vec3::new(0.0, 2.0, 0.0), 0, 1);
        bird.state = 3;
        bird.timer = 0.5;
        steer(&mut bird, &c);
        assert_eq!(bird.state, 0);
        bird.vel.y = -1.0;
        bird_animation(&mut bird);
        assert_eq!(bird.pending_animation, Some(2));
        bird.vel.y = 0.0;
        bird_animation(&mut bird);
        assert_eq!(bird.pending_animation, Some(1));
        bird.state = 1;
        bird.target = bird.pos;
        assert!(matches!(steer(&mut bird, &c), Steering::None));
        assert_eq!(bird.state, 1);
        bird.vel = Vec3::X;
        assert!(matches!(steer(&mut bird, &c), Steering::Halt));
        assert_eq!(bird.state, 2);
        assert_eq!(bird.animation, 0);
        assert_eq!(bird.vel, Vec3::ZERO);
    }
}
