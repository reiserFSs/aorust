//! Traffic ships of the tweak scripts (`Template_Spaceship_*`): `UniversePosition [N]` waypoint loops flown with the stateful
//! `GS_*` / `HS_*` smoothing, as [`ao_scene::Mover`]s (the dynamics are `ao_scene::mover`, decoded from FXS.dll, see
//! `docs/formats.md` § Traffic ships).
//!
//! Only objects whose formulas are literally the template ones are accepted (each field is compared after whitespace
//! normalisation); anything else is skipped rather than guessed.

use super::layers::{mesh_index, object_scale, position_expr};
use super::script::{self, Ctx, Obj};
use crate::character::NameTable;
use crate::mesh::{decode_generic_mesh, MESH_TYPE};
use ao_rdb::RecordStore;
use ao_scene::{Counter, Instance, Mover, MoverState, RotTerm, Scene};
use std::collections::HashMap;

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `GAME.DayTimeFactor [* k]`, `<Object>.Counter + c`, `c * n + <Object>.Counter`, ... `[% 1]`, left to right, as
/// `frac(DayTimeFactor * rate + offset)`. `None` for counters that integrate `GameDeltaTime` (`This.Counter + 0.001`).
pub fn counter_of(objs: &[Obj], o: &Obj, depth: u32) -> Option<Counter> {
    let operand = |t: &str| -> Option<Counter> {
        match t {
            "GAME.DayTimeFactor" => Some(Counter { rate: 1.0, offset: 0.0, wrap: false }),
            _ if t.ends_with(".Counter") && !t.starts_with("This.") && depth < 8 => {
                let other = objs.iter().find(|x| x.name == t.trim_end_matches(".Counter"))?;
                counter_of(objs, other, depth + 1)
            }
            _ => Some(Counter { rate: 0.0, offset: script::eval(t, &|_| None)?, wrap: false }),
        }
    };
    let expr = norm(o.field("Counter")?);
    let mut t = expr.split(' ');
    let mut c = operand(t.next()?)?;
    while let Some(op) = t.next() {
        let rhs = t.next()?;
        match op {
            "+" | "-" => {
                let r = operand(rhs)?;
                // frac(frac(a) + b) = frac(a + b) only when the sum is wrapped again
                if (c.wrap || r.wrap) && !expr.ends_with("% 1") {
                    return None;
                }
                let s = if op == "+" { 1.0 } else { -1.0 };
                c = Counter { rate: c.rate + s * r.rate, offset: c.offset + s * r.offset, wrap: false };
            }
            "*" => {
                let k = script::eval(rhs, &|_| None)?;
                c = Counter { rate: c.rate * k, offset: c.offset * k, wrap: false };
            }
            "%" if rhs == "1" && t.next().is_none() => c.wrap = true,
            _ => return None,
        }
    }
    Some(c)
}

fn sound_profile(o: &Obj, prefix: &str) -> Option<ao_scene::mover::SoundProfile> {
    let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let sound = compact(o.field("EngineSound")?);
    let engine = [
        "SM_Sandy_ShipEngine_Freighter", "SM_Sandy_ShipEngine_Mothership",
        "SM_Sandy_ShipEngine_Cruiser", "SM_Sandy_ShipEngine_SmallTransporter",
        "SM_Sandy_Env_SandWind",
    ].into_iter().find(|name| sound.starts_with(&format!("s({name},")))?;
    let vip = engine == "SM_Sandy_Env_SandWind";
    let pitch = if vip { 200.0 } else { 100.0 };
    let volume = if vip { "0.15f" } else { "This.EngineSpeed" };
    let radius = if vip { 120.0 } else { 0.0 };
    let velocity_suffix = if vip { "*0.1f" } else { "" };
    let expected = format!("s({engine},This.Position,1.0f,{volume}*This.Visibility,e_SoundMaterialDefault,0.0f,{pitch:.1}f,1.0f,{radius:.1}f,This.{prefix}PreviousPos-This.Position/GAME.GameDeltaTime{velocity_suffix})");
    if sound != expected { return None; }
    let mut divide_delta = false;
    let mut speed_scale = 1.0;
    if !vip {
        let speed = compact(o.field("EngineSpeed")?);
        let rest = speed.strip_prefix("This.Position-This.PrevActualPos[LENGTH]1.0f")?.strip_suffix("<|1.0f")?;
        match rest {
            "" => {}
            "/GAME.GameDeltaTime" => { divide_delta = true; }
            "/GAME.GameDeltaTime*0.00000001f" => { divide_delta = true; speed_scale = 0.00000001; }
            "/GAME.GameDeltaTime*0.00000001f*0.01f" => { divide_delta = true; speed_scale = 0.00000001 * 0.01; }
            _ => return None,
        }
    }
    let transport_triggers = o.field("Rotation")?.split("<|").map(str::trim).collect::<Vec<_>>().ends_with(&["This.EngineSound", "This.ThrustSound", "This.LandingSound"]);
    if transport_triggers {
        for (field, expected) in [
            ("Trigger1", "This.EngineSpeed-0.5f|>0.0f*1000.0f<|1.0f+This.Trigger2-This.Trigger2"),
            ("Trigger2", "This.Trigger1"),
            ("StartTrigger", "This.Trigger1*0.0f+1.0f-This.Trigger2*This.Trigger1"),
            ("StopTrigger", "1.0f-This.Trigger1*This.Trigger2"),
            ("ThrustSound", "s(SM_Sandy_ShipTakeoff_Small,This.Position,0.0f,This.StartTrigger*This.Visibility,e_SoundMaterialDefault,0.0f,100.0f,1.0f,0.0f,v(0,0,0))"),
            ("LandingSound", "s(SM_Sandy_ShipLand_Clank,This.Position,0.0f,This.StopTrigger*This.Visibility,e_SoundMaterialDefault,0.0f,100.0f,1.0f,0.0f,v(0,0,0))"),
        ] {
            if compact(o.field(field)?) != expected { return None; }
        }
    } else if !o.field("Rotation")?.trim().ends_with("<| This.EngineSound") {
        return None;
    }
    Some(ao_scene::mover::SoundProfile { engine, speed_scale, divide_delta, constant_volume: vip.then_some(0.15), pitch, radius, velocity_scale: if vip { 0.1 } else { 1.0 }, transport_triggers })
}

/// The mover of scenery object `o`, `None` unless it is a template traffic ship (instance index left 0).
pub fn mover_of(objs: &[Obj], o: &Obj) -> Option<Mover> {
    let ahead = o.fields.iter().find(|(_, e)| e.contains("This.UniversePosition[This.Counter]"))?.0;
    let p = ahead.strip_suffix("AheadTimePos")?;
    let is = |field: &str, want: String| o.field(field).is_some_and(|e| norm(e) == want);
    let ok = is(ahead, format!("This.UniversePosition[This.Counter] + This.{p}PreviousPos - This.{p}PreviousPos"))
        && is(&format!("{p}PreviousPos"), format!("This.{p}AheadTimePos"))
        && is(&format!("{p}AccVec"), format!("This.{p}AheadTimePos - This.{p}PreviousPos * GAME.CutY"))
        && is(&format!("{p}AccCurrent"), format!("This.{p}AccCurrent + This.{p}AheadTimePos - This.Position * 0.15"))
        && (is("Position", format!("This.{p}AccCurrent + This.Position + This.PrevActualPos - This.PrevActualPos"))
            || is("Position", format!("This.{p}AccCurrent + This.Position")))
        && (o.field("PrevActualPos").is_none() || is("PrevActualPos", "This.Position".into()));
    if !ok {
        return None;
    }
    // `This.GS_ForwardVec * gain + This.GS_AccVec [NORMALIZE] 1.0`
    let fwd = norm(o.field(&format!("{p}ForwardVec"))?);
    let gain = fwd.strip_prefix(&format!("This.{p}ForwardVec * "))?.strip_suffix(&format!(" + This.{p}AccVec [NORMALIZE] 1.0"))?;
    let forward_gain = script::eval(gain, &|_| None)?;
    let banking_ok = p == "GS_" && is("GS_AccMod", "This.GS_AccMod * 10.0 + GAME.UpDirection * 10.0 + This.GS_AccVec [NORMALIZE] 1.0".into());

    let game = objs.iter().find(|g| g.name == "GAME");
    let ctx = Ctx::at(0.0);
    let game_vec = |name: &str, default: [f32; 3]| game.and_then(|g| script::vector(g, &ctx, g.field(name)?, 0)).unwrap_or(default);
    let (forward_dir, up_dir, cut_y) = (game_vec("ForwardDirection", [0.0, 0.0, 1.0]), game_vec("UpDirection", [0.0, 1.0, 0.0]), game_vec("CutY", [1.0, 0.0, 1.0]));

    let mut rotation = vec![];
    for term in o.field("Rotation")?.split("<|").next()?.split("[ROT]") {
        let field = term.trim().strip_prefix("This.")?;
        let expr = norm(o.field(field)?);
        rotation.push(if expr == format!("GAME.ForwardDirection [ROT] This.{p}ForwardVec") {
            RotTerm::Forward
        } else if banking_ok && expr == "GAME.UpDirection [ROT] This.GS_AccMod" {
            RotTerm::Banking
        } else {
            let q = script::rotation(o, &ctx, &expr, 0)?;
            RotTerm::Fixed([q.x, q.y, q.z, q.w])
        });
    }

    let entries: Vec<&str> = o.field("UniversePosition")?.split('\n').collect();
    if o.field("UniversePosition[]")?.parse::<usize>().ok()? != entries.len() {
        return None;
    }
    let waypoints = entries.iter().map(|e| position_expr(objs, o, e)).collect::<Option<Vec<_>>>()?;
    Some(Mover {
        instance: 0,
        waypoints,
        counter: counter_of(objs, o, 0)?,
        forward_gain,
        forward_dir,
        up_dir,
        cut_y,
        rotation,
        scale: object_scale(o, &ctx),
        sound: sound_profile(o, p),
    })
}

/// Adds the ship `o` (with its native generic visual frame, shared through `meshes`) to `scene.instances` / `scene.movers`,
/// posed at `day_time` with the clock running at 1 game second per second.
pub fn emit(objs: &[Obj], o: &Obj, store: &RecordStore, names: &NameTable, scene: &mut Scene, meshes: &mut HashMap<String, HashMap<Option<usize>, usize>>, day_time: f32) {
    let Some(mut mover) = mover_of(objs, o) else { return };
    let Some(name) = o.string("Mesh") else { return };
    let index = mesh_index(o, &Ctx::at(day_time));
    let mesh = match meshes.get(name).and_then(|versions| versions.get(&index)) {
        Some(&m) => m,
        None => {
            let mut tmp = Scene::default();
            let Some(id) = names.id(MESH_TYPE, name) else { return };
            if !matches!(decode_generic_mesh(store, id, index, false, &super::layers::mesh_states(o), &mut tmp), Ok(Some(_))) {
                return;
            }
            let Some(m) = tmp.meshes.pop() else { return };
            scene.textures.extend(tmp.textures);
            scene.meshes.push(m);
            meshes.entry(name.to_owned()).or_default().insert(index, scene.meshes.len() - 1);
            scene.meshes.len() - 1
        }
    };
    mover.instance = scene.instances.len();
    scene.instances.push(Instance { mesh, transform: mover.transform(&MoverState::settled(&mover, day_time, 1.0)) });
    if super::layers::is_far_away(o) {
        scene.far_away.push(mover.instance);
    }
    scene.movers.push(mover);
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = "Object GAME\n{\n  Vector UpDirection v( 0,1,0 )\n  Vector ForwardDirection v( 0,0,1 )\n  Vector CutY v( 1,0,1 )\n}\n";
    const RKPP: &str = "Object RKPP\n{\n  Vector A v( 0, 0, 0 )\n  Vector B v( 100, 0, 0 )\n}\nObject PlayfieldData\n{\n  Vector UniversePosition RKPP.A\n}\n";
    const SHIP: &str = "Object Ship
{
  Vector GS_AheadTimePos: This.UniversePosition[This.Counter] + This.GS_PreviousPos - This.GS_PreviousPos
  Vector GS_PreviousPos: This.GS_AheadTimePos
  Vector GS_AccVec: This.GS_AheadTimePos - This.GS_PreviousPos * GAME.CutY
  Vector GS_AccMod: This.GS_AccMod * 10.0 + GAME.UpDirection * 10.0 + This.GS_AccVec [NORMALIZE] 1.0
  Vector GS_ForwardVec: This.GS_ForwardVec * 20 + This.GS_AccVec [NORMALIZE] 1.0
  Vector GS_AccCurrent: This.GS_AccCurrent + This.GS_AheadTimePos - This.Position * 0.15
  Vector Position This.GS_AccCurrent + This.Position + This.PrevActualPos - This.PrevActualPos
  Vector PrevActualPos This.Position
  Quaternion Rot0: v( 0,1,0 ), 90
  Quaternion Rot1: GAME.ForwardDirection [ROT] This.GS_ForwardVec
  Quaternion Rot2: GAME.UpDirection [ROT] This.GS_AccMod
  Quaternion Rotation: This.Rot0 [ROT] This.Rot1 [ROT] This.Rot2 <| This.EngineSound
  Float Scale: 1.0
  Float Counter: GAME.DayTimeFactor * 120 % 1
  Vector UniversePosition [3]
      RKPP.A - PlayfieldData.UniversePosition + v( 0, 10, 0 )
      RKPP.B - PlayfieldData.UniversePosition
      , RKPP.B - PlayfieldData.UniversePosition + v( 0, 0, 50 )
}
Object Next
{
  Float Counter: Ship.Counter + 0.2 % 1
}
Object Third
{
  Float Counter: 0.1 * 3 + Ship.Counter % 1
}
Object Integrator
{
  Float Counter: This.Counter + 0.001 % 1
}
";

    fn objs() -> Vec<Obj> {
        script::parse_objects(&format!("{GAME}{RKPP}{SHIP}"))
    }

    #[test]
    fn template_ship_becomes_a_mover() {
        let objs = objs();
        let m = mover_of(&objs, objs.iter().find(|o| o.name == "Ship").unwrap()).unwrap();
        assert_eq!(m.waypoints, vec![[0.0, 10.0, 0.0], [100.0, 0.0, 0.0], [100.0, 0.0, 50.0]]);
        assert_eq!(m.forward_gain, 20.0);
        assert_eq!(m.counter, Counter { rate: 120.0, offset: 0.0, wrap: true });
        assert!(matches!(m.rotation[..], [RotTerm::Fixed(_), RotTerm::Forward, RotTerm::Banking]));
    }

    #[test]
    fn counters_compose_left_to_right() {
        let objs = objs();
        let c = |n: &str| counter_of(&objs, objs.iter().find(|o| o.name == n).unwrap(), 0);
        let next = c("Next").unwrap();
        assert!(next.wrap && (next.rate - 120.0).abs() < 1e-6 && (next.offset - 0.2).abs() < 1e-6);
        let third = c("Third").unwrap();
        assert!((third.offset - 0.3).abs() < 1e-6 && third.wrap);
        assert_eq!(c("Integrator"), None, "per-frame integrators are not time driven");
    }

    #[test]
    fn a_modified_template_is_rejected_not_guessed() {
        let text = format!("{GAME}{RKPP}{}", SHIP.replace("* 0.15", "* 0.25"));
        let objs = script::parse_objects(&text);
        assert!(mover_of(&objs, objs.iter().find(|o| o.name == "Ship").unwrap()).is_none());
    }

    #[test]
    fn installed_traffic_templates_retain_authored_sound_profiles() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client/cd_image/twk");
        if !dir.is_dir() { return; }
        for template in ["Freighter", "ICC_Shuttle", "ICC_Tanker", "OmniCruiser", "OmniMothership", "Transporter", "VIP"] {
            let text = std::fs::read_to_string(dir.join(format!("Template_Spaceship_{template}.txt"))).unwrap();
            let text = text.lines().map(|line| line.split('#').next().unwrap_or("")).collect::<Vec<_>>().join("\n");
            let objects = script::parse_objects(&text);
            let object = objects.iter().find(|object| object.field("EngineSound").is_some()).unwrap();
            let prefix = if matches!(template, "Transporter" | "VIP") { "GS_" } else { "HS_" };
            let profile = sound_profile(object, prefix).unwrap_or_else(|| panic!("missing authored sound for {template}"));
            assert_eq!(profile.transport_triggers, template == "Transporter");
            if template == "VIP" {
                assert_eq!((profile.radius, profile.pitch, profile.constant_volume), (120.0, 200.0, Some(0.15)));
            }
        }
    }

    #[test]
    fn real_newland_has_a_transporter_loop_in_view_of_the_city() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        let (Some(t), Ok(store)) = (super::super::Tweaks::load(&dir, 566, true), RecordStore::open(&dir)) else { return };
        let mut scene = Scene::default();
        super::super::emit_distant(&t, &store, &mut scene, 2648.69);
        // Tweak_Spaceship_RubiKa_Traffic_Transporters_Newland: 7 waypoints, 54 s loop
        let m = scene.movers.iter().find(|m| m.waypoints.len() == 7 && m.counter.rate == 120.0).expect("Newland transporter");
        assert!(scene.instances[m.instance].mesh < scene.meshes.len());
        assert!(scene.spaceships.contains(&m.instance), "native GameShowSpaceship gates authored traffic");
        assert_eq!(scene.spaceships, (0..scene.instances.len()).collect::<Vec<_>>(), "all authored distant scenery shares the native visibility gate");
        assert!(m.sound.is_some(), "the live mover retains its authored audio profile");
        let s = MoverState::settled(m, 2648.69, 1.0);
        assert!((0..3).all(|k| (s.pos[k] - m.target(2648.69)[k]).abs() < 50.0), "{:?} {:?}", s.pos, m.target(2648.69));
    }
}
