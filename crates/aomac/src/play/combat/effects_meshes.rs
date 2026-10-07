//! Native GfxVisualRockList (GC 10101591/101017b6, DS 100616e4).
use super::Template;
use anyhow::{ensure, Context, Result};
use ao_formats::character::NameTable;
use ao_formats::weather::R250;
use ao_formats::mesh::{load_mesh, MESH_TYPE};
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Scene};
use glam::{Mat4, Quat, Vec3};
use std::{collections::HashMap, sync::Arc};

/// InstanceManager_t::GetTypeInstance(1010001, name), DS 100616e4.
fn resource_name(selector: u32) -> Option<String> {
    Some(match selector {
        4 | 11 => "rock01.abiff".into(),
        12..=17 => format!("rock{:02}.abiff", selector - 10),
        18 => "gib_skull.abiff".into(),
        19..=25 => format!("gib{:02}.abiff", selector - 18),
        26 => "gib_skull_ice.abiff".into(),
        27..=33 => format!("gib{:02}_ice.abiff", selector - 26),
        34 => "gib_skull_slime.abiff".into(),
        35..=41 => format!("gib{:02}_slime.abiff", selector - 34),
        46 => "shell_casing.abiff".into(),
        47..=53 => format!("tower_shrapnel{}.abiff", selector - 46),
        _ => return None,
    })
}

pub(super) fn load_resource(store: &RecordStore, names: &NameTable, selector: u32) -> Result<Scene> {
    let name = resource_name(selector).with_context(|| format!("non-mesh rock selector {selector}"))?;
    let id = names.id(MESH_TYPE, &name).with_context(|| format!("missing rock resource {name}"))?;
    load_mesh(store, id).with_context(|| format!("rock selector {selector}: {name} ({id})"))
}

struct Rock {
    group: usize,
    position: Vec3,
    velocity: Vec3,
    axis: Vec3,
    angle: f32,
    spin: f32,
    bounces: u32,
}

pub(super) struct MeshEffect {
    scenes: Vec<(u32, Arc<Scene>)>,
    selectors: Vec<usize>,
    rocks: Vec<Rock>,
    source: Vec3,
    target: Vec3,
    speed: f32,
    rate: f32,
    duration: f32,
    capacity: usize,
    elapsed: f32,
    emitted: u32,
    stopped: u32,
    vulcan: Option<(Template, Mat4, bool)>,
    pub(super) sequence:u64,
}

// GC 1013d97c rounds the signed word to float before its unsigned correction.
fn native_fraction(rng: &mut R250) -> f64 {
    let word = rng.next_u32() as i32;
    let value = word as f32;
    let value = if word < 0 { value + 4294967296.0 } else { value };
    f64::from(value) / 4294967296.0
}
fn fraction(rng: &mut R250) -> f32 { native_fraction(rng) as f32 }

fn tumble(rng: &mut R250) -> (Vec3, f32, f32) {
    let axis = Vec3::new(fraction(rng)*2.0-1.0, fraction(rng)*2.0-1.0, fraction(rng)*2.0-1.0);
    let axis = axis.try_normalize().unwrap_or(Vec3::Y);
    // Axis is a pseudovector: reflecting AO Z negates X/Y, not Z.
    let axis = Vec3::new(-axis.x, -axis.y, axis.z);
    let angle = fraction(rng) * std::f32::consts::TAU - std::f32::consts::PI;
    let spin = (fraction(rng) * std::f32::consts::TAU - std::f32::consts::PI) * 8.0;
    (axis, angle, spin)
}

impl MeshEffect {
    pub(super) fn new(template: &Template, store: &RecordStore, names: &NameTable, source: Mat4, target: Vec3, resources: &mut HashMap<u32, Arc<Scene>>) -> Result<Self> {
        ensure!(matches!(template.kind,1027|1029), "not a native rock-list template");
        let count = template.word(22)? as usize;
        ensure!((1..200).contains(&count), "invalid rock selector count {count}");
        let capacity = template.float(20)?.trunc();
        ensure!(capacity >= 0.0, "negative rock capacity");
        let rate = template.float(21)?;
        let speed = template.float(10)?;
        if template.kind==1029 {
            for i in 11..=16 {template.float(i)?;}
        }
        ensure!(rate >= 0.0 && speed > 0.0, "invalid rock emission rate or speed");
        let mut scenes = Vec::new();
        let mut selectors = Vec::with_capacity(count);
        for index in 0..count {
            let selector = template.word(23 + index)?;
            ensure!(selector < 200, "invalid rock resource selector {selector}");
            // DS100616e4 creates material-only environment entries for 5..10;
            // GetNew1001c4f9 rejects those (no mesh from100612c3).
            if (5..=10).contains(&selector) || (42..=45).contains(&selector) {selectors.push(usize::MAX);continue;}
            let group = match scenes.iter().position(|(s, _)| *s == selector) {
                Some(group) => group,
                None => {
                    let scene = match resources.entry(selector) {
                        std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
                        std::collections::hash_map::Entry::Vacant(entry) => Arc::clone(entry.insert(Arc::new(load_resource(store, names, selector)?))),
                    };
                    scenes.push((selector, scene));
                    scenes.len() - 1
                }
            };
            selectors.push(group);
        }
        let capacity = capacity.min(128.0) as usize;
        let source=if template.kind==1029 {super::sprites::connector(template,source)?} else {source};
        Ok(Self { scenes, selectors, rocks: Vec::with_capacity(capacity), source: source.w_axis.truncate(), target,
            speed, rate, duration: template.float(8)?, capacity, elapsed: 0.0, emitted: 0, stopped: 0,
            vulcan: if template.kind==1029 {Some((template.clone(),source,template.words.get(23+count).copied().unwrap_or(0)!=0))} else {None},sequence:0 })
    }

    pub(super) fn scenes(&self) -> &[(u32, Arc<Scene>)] { &self.scenes }
    pub(super) fn actor_count(&self) -> usize { self.capacity }
    pub(super) fn rock_count(&self)->usize {self.rocks.len()}
    pub(super) fn set_duration(&mut self, duration: f32) { self.duration = duration; }
    pub(super) fn update_source(&mut self, source:Mat4)->Result<()> {
        if let Some((t,matrix,_))=&mut self.vulcan {
            *matrix=super::sprites::connector(t,source)?;
            self.source=matrix.w_axis.truncate();
        }
        Ok(())
    }

    /// GC 101017b6: cumulative ceil emission, semi-implicit gravity, closest-surface bounce.
    #[cfg(test)]
    pub(super) fn frame(&mut self, dt: f32, rng: &mut R250, terrain: &mut dyn FnMut(Vec3) -> Option<(Vec3, Vec3)>) -> Result<bool> {
        let mut live_rocks=self.rocks.len();
        self.frame_with_pool(dt,rng,terrain,&mut live_rocks)
    }
    pub(super) fn frame_with_pool(&mut self, dt:f32, rng:&mut R250, terrain:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>, live_rocks:&mut usize)->Result<bool> {
        ensure!(dt.is_finite() && dt >= 0.0, "invalid rock timestep");
        self.elapsed += dt;
        if self.duration > 0.0 && self.elapsed > self.duration { return Ok(false); }
        let desired = (self.rate * self.elapsed).ceil();
        ensure!(desired.is_finite() && desired <= u32::MAX as f32, "rock emission overflow");
        while self.emitted < desired as u32 {
            self.emitted += 1;
            let selector = (native_fraction(rng) * (self.selectors.len() as f64 - 0.0001)) as usize;
            if self.rocks.len() == self.capacity || *live_rocks>=512 || self.selectors[selector]==usize::MAX { continue; }
            let velocity = if let Some((t,matrix,_))=&self.vulcan {
                let azimuth=fraction(rng)*std::f32::consts::TAU;
                let elevation=t.float(11)?+(t.float(12)?-t.float(11)?)*fraction(rng);
                // GC1010366b: connector X*cos(phi)*cos(theta) +
                // Y*sin(theta) + Z*sin(phi)*cos(theta), then authored speed.
                matrix.transform_vector3(Vec3::new(azimuth.cos()*elevation.cos(),elevation.sin(),-azimuth.sin()*elevation.cos()))*self.speed
            } else {
                let delta = self.target - self.source;
                let flight = ((delta.length() + fraction(rng)*2.0-1.0) / self.speed).max(0.02);
                let horizontal = Vec3::new(delta.x, 0.0, delta.z).normalize_or_zero();
                let perpendicular = Vec3::new(horizontal.z, 0.0, -horizontal.x);
                let mut velocity = horizontal*self.speed + perpendicular*((fraction(rng)*2.0-1.0)/flight);
                velocity.y = flight*4.9 + delta.y/flight;
                velocity
            };
            let (axis, angle, spin) = tumble(rng);
            self.rocks.push(Rock { group: self.selectors[selector], position: self.source, velocity, axis, angle, spin, bounces: 0 });
            *live_rocks+=1;
        }
        for rock in &mut self.rocks {
            rock.velocity.y -= dt*9.8;
            rock.position += rock.velocity*dt;
            if let Some((point, normal)) = terrain(rock.position) {
                if rock.position.y < point.y {
                    rock.position = point;
                    rock.velocity = (rock.velocity - 2.0*rock.velocity.dot(normal)*normal)*0.75;
                    if rock.bounces < 4 && rock.velocity.length() >= 0.1 {
                        (rock.axis, rock.angle, rock.spin) = tumble(rng);
                        rock.bounces += 1;
                    } else {
                        rock.velocity = Vec3::ZERO;
                        rock.spin = 0.0;
                        if self.vulcan.as_ref().is_none_or(|(_,_,delete)| !delete) {self.stopped += 1;}
                    }
                }
            }
            rock.angle += rock.spin*dt;
        }
        Ok(self.stopped < self.capacity as u32)
    }

    /// DS 1001c435: authored mesh plus world translation and axis-angle orientation.
    pub(super) fn actors(&self, actor_base: u32, model_base: u64) -> Vec<ActorFrame> {
        self.rocks.iter().enumerate().map(|(index, rock)| ActorFrame {
            id: actor_base + index as u32, model: model_base | self.scenes[rock.group].0 as u64,
            transform: Mat4::from_rotation_translation(Quat::from_axis_angle(rock.axis, rock.angle), rock.position).to_cols_array_2d(),
            parts: Vec::new(), skin: None, always: false, alpha: 1.0,
            ..Default::default()
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_45083_renderer_lifecycle()->Result<()> {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() {return Ok(());}
        let mut renderer=super::super::Renderer::open(&dir)?;
        let binding=super::super::Binding {group:0,attractor:0,effect:45083,note:0,color:0};
        let identity=(50000,1029);
        renderer.prepare_anchor(identity,3000,Some(Mat4::IDENTITY));
        let config=super::super::EffectConfig {creation:super::super::Creation::Dynel,source_identity:Some(identity),track_source:true,..Default::default()};
        let handle=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,config)?;
        let mut host=ao_render::Host::headless();
        let mut ground=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
        renderer.frame(0.016,&mut host,Some(&mut ground));
        renderer.frame(0.016,&mut host,Some(&mut ground));
        assert!(renderer.is_active(handle));
        assert!(host.actors.iter().any(|a|a.id==handle));
        renderer.update_position(handle,Vec3::X);
        renderer.terminate_gracefully(handle);
        assert!(!renderer.is_active(handle));
        let handle=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,config)?;
        renderer.source_deleted(identity);
        assert!(!renderer.is_active(handle));
        let handle=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,Default::default())?;
        renderer.delete(handle);
        assert!(!renderer.is_active(handle));
        Ok(())
    }
    #[test]
    fn authored_45083_vulcan_uses_slime_mesh_and_connector()->Result<()> {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() {return Ok(());}
        let store=RecordStore::open(&dir)?;
        let names=NameTable::load(&store)?;
        let templates=super::super::Templates::open(&dir)?;
        let t=&templates.by_id[&45083];
        assert_eq!(t.kind,1029);
        assert_eq!(&t.words[20..25],&[1065353216,1132462080,1,39,5]);
        let mut e=MeshEffect::new(t,&store,&names,Mat4::IDENTITY,Vec3::ZERO,&mut HashMap::new())?;
        assert_eq!(e.capacity,1);
        assert_eq!(e.scenes[0].0,39);
        let mut rng=R250::new(0xe6f1);
        assert!(e.frame(0.0,&mut rng,&mut |_|None)?);
        assert!(e.rocks.is_empty());
        assert!(e.frame(0.016,&mut rng,&mut |_|None)?);
        assert_eq!(e.rocks.len(),1);
        assert!((e.rocks[0].velocity.length()-6.4).abs()<0.2);
        assert!(e.rocks[0].velocity.y>0.0);
        let initial=e.source;
        e.update_source(Mat4::from_translation(Vec3::X*3.0))?;
        assert!((e.source-initial-Vec3::X*3.0).length()<1e-5);
        assert_eq!(e.actors(1,1).len(),1);
        Ok(())
    }
    #[test]
    fn native_resource_selector_ranges() {
        assert_eq!(resource_name(4), resource_name(11));
        assert_eq!(resource_name(17).as_deref(), Some("rock07.abiff"));
        assert_eq!(resource_name(33).as_deref(), Some("gib07_ice.abiff"));
        assert_eq!(resource_name(41).as_deref(), Some("gib07_slime.abiff"));
        assert_eq!(resource_name(53).as_deref(), Some("tower_shrapnel7.abiff"));
        assert!(resource_name(42).is_none());
    }

    #[test]
    #[ignore = "requires authored retail ABIFF records"]
    fn authored_native_rock_resources() {
        let dir = ao_gui::client_dir();
        let store = RecordStore::open(&dir).unwrap();
        let names = NameTable::load(&store).unwrap();
        for selector in [4, 11].into_iter().chain(12..=41).chain(46..=53) {
            let scene = load_resource(&store, &names, selector).unwrap();
            assert!(!scene.meshes.is_empty(), "selector {selector}");
            assert!(scene.meshes.iter().any(|m| !m.vertices.is_empty() && m.submeshes.iter().any(|s| !s.indices.is_empty())), "selector {selector}");
        }
        let templates = super::super::Templates::open(&dir).unwrap();
        let mut resources = HashMap::new();
        for (id, capacity, speed) in [(2780, 16, 30.0), (2781, 32, 20.0), (2782, 64, 10.0)] {
            let template = &templates.by_id[&id];
            let mut rng = R250::new(0xe6f1);
            let mut effect = MeshEffect::new(template, &store, &names, Mat4::IDENTITY, Vec3::new(4.0, 0.0, 0.0), &mut resources).unwrap();
            assert_eq!(effect.actor_count(), capacity);
            assert_eq!(effect.speed, speed);
            assert_eq!(effect.scenes.len(), 16);
            assert!(effect.frame(1.0/60.0, &mut rng, &mut |_| None).unwrap());
            assert_eq!(effect.rocks.len(), 3, "ceil(128 / 60)");
            for actor in effect.actors(1000, 0x100) {
                assert!(effect.scenes.iter().any(|(selector, _)| actor.model == (0x100 | u64::from(*selector))));
                assert!(actor.skin.is_none(), "authored rock mesh never replaced by CPU ribbon vertices");
                assert!(actor.transform.iter().flatten().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn native_bounce_and_settle() {
        let mut effect = MeshEffect { scenes: Vec::new(), selectors: vec![0], rocks: vec![Rock {
            group: 0, position: Vec3::new(1.0, -0.1, 2.0), velocity: Vec3::new(2.0, -3.0, 0.0),
            axis: Vec3::Y, angle: 0.0, spin: 1.0, bounces: 0,
        }], source: Vec3::ZERO, target: Vec3::X, speed: 1.0, rate: 0.0, duration: -1.0,
            capacity: 1, elapsed: 0.0, emitted: 0, stopped: 0, vulcan:None,sequence:0 };
        let mut rng = R250::new(0xe6f1);
        let mut floor = |p: Vec3| Some((Vec3::new(p.x, 0.0, p.z), Vec3::Y));
        assert!(effect.frame(0.0, &mut rng, &mut floor).unwrap());
        assert_eq!(effect.rocks[0].position, Vec3::new(1.0, 0.0, 2.0));
        assert_eq!(effect.rocks[0].velocity, Vec3::new(1.5, 2.25, 0.0));
        assert_eq!(effect.rocks[0].bounces, 1);
        effect.rocks[0].bounces = 4;
        effect.rocks[0].position.y = -0.1;
        assert!(!effect.frame(0.0, &mut rng, &mut floor).unwrap());
        assert_eq!(effect.rocks[0].velocity, Vec3::ZERO);
        assert_eq!(effect.rocks[0].spin, 0.0);
    }

    #[test]
    #[ignore = "retail assets and offscreen Metal rendering"]
    fn authored_native_rock_frames() {
        let dir = ao_gui::client_dir();
        let store = RecordStore::open(&dir).unwrap();
        let templates = super::super::Templates::open(&dir).unwrap();
        let names = NameTable::load(&store).unwrap();
        let mut resources = HashMap::new();
        let out = std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp/FxRest/frames".into());
        std::fs::create_dir_all(&out).unwrap();
        let mut ids: Vec<_> = templates.by_id.iter().filter(|(_, t)| matches!(t.kind,1027|1029)).collect();
        ids.sort_by_key(|(id, _)| **id);
        assert!(!ids.is_empty());
        for required in [2780, 2781, 2782,45083] {
            assert!(ids.iter().any(|(id, _)| **id == required), "missing retail rock template {required}");
        }
        for (&id, template) in ids {
            let mut rng = R250::new(0xe6f1);
            let mut effect = MeshEffect::new(template, &store, &names, Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0)), Vec3::new(4.0, 0.0, 0.0), &mut resources).unwrap();
            let models: Vec<_> = effect.scenes().iter().map(|(selector, s)| (0x100 | u64::from(*selector), s.as_ref().clone())).collect();
            let mut floor = |p: Vec3| Some((Vec3::new(p.x, 0.0, p.z), Vec3::Y));
            let mut rendered_frames = 0;
            for frame in 1..=120 {
                if !effect.frame(1.0/60.0, &mut rng, &mut floor).unwrap() { break; }
                if [15, 30, 60, 120].contains(&frame) {
                    let actors = effect.actors(1000, 0x100);
                    assert!(!actors.is_empty(), "template {id}, frame {frame}");
                    // Frame the rendered resource vertices, not the emission origin or
                    // an assumed flight envelope: native rocks can travel past that camera.
                    let mut lo = Vec3::splat(f32::MAX);
                    let mut hi = Vec3::splat(f32::MIN);
                    for actor in &actors {
                        let model = &models.iter().find(|(key, _)| *key == actor.model).unwrap().1;
                        let transform = Mat4::from_cols_array_2d(&actor.transform);
                        for mesh in &model.meshes {
                            for index in mesh.submeshes.iter().flat_map(|s| &s.indices) {
                                let point = transform.transform_point3(Vec3::from(mesh.vertices[*index as usize].pos));
                                assert!(point.is_finite(), "template {id}: nonfinite resource vertex");
                                lo = lo.min(point);
                                hi = hi.max(point);
                            }
                        }
                    }
                    assert!(lo.cmple(hi).all(), "template {id}: no drawable resource bounds");
                    let center = (lo + hi) * 0.5;
                    let radius = ((hi - lo).length() * 0.5).max(0.5);
                    // Same 60-degree-FOV framing convention as ao_render::default_view.
                    let eye = center + Vec3::new(0.6, 0.5, 1.0).normalize() * radius * 2.4;
                    let path = out.join(format!("rock_{id}_{frame}.png"));
                    let blank = out.join(format!("rock_{id}_{frame}_background.png"));
                    let time = frame as f32 / 60.0;
                    ao_render::render_to_png_actors(&Scene::default(), &models, actors,
                        eye.to_array(), center.to_array(), 640, 480, &path, time).unwrap();
                    ao_render::render_to_png_at(&Scene::default(), eye.to_array(), center.to_array(),
                        640, 480, &blank, time).unwrap();
                    let rendered = image::open(&path).unwrap().to_rgba8();
                    let background = image::open(&blank).unwrap().to_rgba8();
                    std::fs::remove_file(blank).unwrap();
                    let visible = rendered.pixels().zip(background.pixels()).filter(|(a, b)| a != b).count();
                    assert!(visible > 0, "template {id}, frame {frame}: no actual resource pixels");
                    eprintln!("rock {id} frame {frame}: bounds {lo:?}..{hi:?}, {visible} visible resource pixels, {}", path.display());
                    rendered_frames += 1;
                }
            }
            assert!(rendered_frames > 0, "template {id}: expired before any visible frame");
        }
    }
}
