//! The character-selection preview, as `CharacterViewer_c`/`CCCharacter_t` do it (docs/screens.md §5.5): the default
//! character of the entry's breed/sex with its first head (or the cached appearance, worn cloth included), `idle-stand` played once, then after every finished clip
//! `rand() % 115 < 23` picks one of the 23 socials, else idle again.
//!
//! Poses are CPU-skinned on a worker thread (cached appearances reuse one rig/assets); the UI plays the cached frames. Socials are
//! built on demand: a clip that is not cached yet is requested and idle plays meanwhile (the original has no such delay).

use ao_formats::character::{self, Player, Role};
use ao_formats::screens::{self, CharSelectLook};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Pose samples per second of the cached clips.
pub const FPS: f32 = 20.0;

pub enum Out {
    /// The textured bind/idle pose to upload.
    First(Box<Scene>),
    /// All frames of a clip (`None` = idle).
    Clip(Option<usize>, Vec<Scene>),
    Failed(String),
}

pub struct Worker {
    req: Sender<Option<usize>>,
    pub rx: Receiver<Out>,
}

impl Worker {
    /// Starts building the preview of `(breed, sex)`; dropping the worker stops the thread.
    pub fn start(dir: PathBuf, breed: i32, sex: i32, char_id: i32, head: i32) -> Worker {
        let (req, req_rx) = channel::<Option<usize>>();
        let (tx, rx) = channel();
        let prefs = super::prefs::dir();
        std::thread::spawn(move || {
            if let Err(e) = run(&dir, breed, sex, (char_id, head), prefs.as_deref(), &req_rx, &tx) {
                let _ = tx.send(Out::Failed(format!("{e:#}")));
            }
        });
        Worker { req, rx }
    }

    #[cfg(test)]
    pub(super) fn queued_first(scene: Scene) -> Worker {
        let (req, _) = channel();
        let (tx, rx) = channel();
        tx.send(Out::First(Box::new(scene))).unwrap();
        Worker { req, rx }
    }

    /// Asks for the frames of social `i` (index into `screens::SOCIALS`) or idle (`None`).
    pub fn request(&self, clip: Option<usize>) {
        let _ = self.req.send(clip);
    }
}

/// `CharacterViewerModule_c::GetData(charId)` without a live world (docs/screens.md §5.5): the appearance cache entry of the
/// character if there is one — first aomac's `CharacterViewer.xml`, then the original client's `prefs/` — else `None`
/// (= `CCCharacter_t::SetBreed`, the default character of the entry's breed/sex).
fn cached(client: &std::path::Path, prefs: Option<&std::path::Path>, char_id: i32) -> Option<character::CachedCharacter> {
    let retail = client.join("prefs");
    prefs.into_iter().chain(std::iter::once(retail.as_path()))
        .find_map(|d| character::ViewerCache::load(d).0.remove(&char_id))
}

fn body_mesh(store: &RecordStore, breed: i32, sex: i32, fatness: i32) -> anyhow::Result<i32> {
    let (breed, gender) = screens::wire_breed_sex(breed, sex)?;
    let build = match fatness { 0 => 0, 2 => 2, _ => 1 };
    Ok(character::player_model_build(store, breed, gender, build)? as i32)
}

/// The creation reply supplies the identity, not the appearance. Keep the submitted mesh ids in the existing viewer cache.
pub(super) fn remember_created(client: &std::path::Path, id: i32, req: &ao_net::msg::CreateCharacterRequest) -> anyhow::Result<()> {
    let dir = super::prefs::dir().ok_or_else(|| anyhow::anyhow!("preferences directory unavailable"))?;
    let store = RecordStore::open(client)?;
    let mesh = body_mesh(&store, req.breed, req.gender, req.width.clamp(0, 2))?;
    remember_created_in(&dir, id, req, mesh)
}

fn remember_created_in(dir: &std::path::Path, id: i32, req: &ao_net::msg::CreateCharacterRequest, mesh_id: i32) -> anyhow::Result<()> {
    let mut cache = character::ViewerCache::load(dir);
    cache.update(character::CachedCharacter {
        id,
        time: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs().min(i32::MAX as u64) as i32,
        mesh_id,
        head_id: req.head,
        breed: req.breed,
        sex: req.gender,
        fatness: req.width,
        ..Default::default()
    });
    cache.save(dir)?;
    Ok(())
}

/// `SlotShuttingDown` / `SlotConfigurationSaved`: update the active live visuals, then save the global cache.
impl super::Play {
    pub(super) fn save_viewer_cache(&self) {
        let Some(u) = self.zone.own_update.as_deref() else { return };
        // `GetData` refuses mech/morph visuals rather than overwriting the normal appearance.
        if self.zone.stat(0x296).unwrap_or(0) != 0 || self.zone.stat(0x167).unwrap_or(u.monster_data) != 0 { return; }
        let result = (|| -> anyhow::Result<()> {
            let dir = super::prefs::dir().ok_or_else(|| anyhow::anyhow!("preferences directory unavailable"))?;
            let store = RecordStore::open(&self.dir)?;
            let mesh = body_mesh(&store, u.breed as i32, u.sex as i32, u.fatness as i32)?;
            remember_live_in(&dir, self.zone.char_id as i32, u, mesh)?;
            Ok(())
        })();
        if let Err(e) = result { eprintln!("live character appearance cache: {e:#}"); }
    }
}

impl Drop for super::Play {
    fn drop(&mut self) {
        self.save_viewer_cache();
    }
}

fn remember_live_in(dir: &std::path::Path, id: i32, u: &ao_net::n3::dynel::SimpleCharFullUpdate, mesh_id: i32) -> anyhow::Result<()> {
    let look = super::avatar::AvatarLook::from_update(u, |_| character::Skin::Caucasian)?;
    let mut c = character::CachedCharacter {
        id,
        time: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs().min(i32::MAX as u64) as i32,
        mesh_id,
        head_id: look.look.head.map_or(0, |h| h as i32),
        breed: u.breed as i32,
        sex: u.sex as i32,
        fatness: u.fatness as i32,
        side: u.side as i32,
        meshes: u.attractors.iter().filter(|_| u.flags & ao_net::n3::dynel::flag::SET_DYNEL_800 == 0)
            .map(|a| character::MeshEntry { attractor: a.place as i8, flags: a.byte as i8, mesh_id: a.mesh, texture_id: a.field }).collect(),
        ..Default::default()
    };
    c.set_equipment(&look.look.equipment);
    let mut cache = character::ViewerCache::load(dir);
    cache.update(c);
    cache.save(dir)?;
    Ok(())
}

fn listed_appearance(id: i32, breed: i32, sex: i32, mesh_id: i32, head: i32) -> Option<character::CachedCharacter> {
    (head > 0).then_some(character::CachedCharacter { id, mesh_id, head_id: head, breed, sex, fatness: 1, ..Default::default() })
}

fn scene(store: &RecordStore, look: &CharSelectLook, pose: (Role, f32)) -> anyhow::Result<Scene> {
    character::load_player(store, &Player::new(look.breed, look.gender, look.skin, Some(look.head.0)), Some(pose))
}

fn frames(store: &RecordStore, look: &CharSelectLook, role: Role) -> anyhow::Result<Vec<Scene>> {
    let model = character::player_model(store, look.breed, look.gender)?;
    let anim = character::load_anim(store, character::role_anim(store, model, &role)?)?;
    let n = ((anim.duration / 1000.0 * FPS).round() as usize).max(1);
    (0..n)
        .map(|k| {
            let mut s = scene(store, look, (role.clone(), k as f32 / FPS))?;
            s.textures.clear();
            Ok(s)
        })
        .collect()
}

fn run(dir: &std::path::Path, breed: i32, sex: i32, (char_id, head): (i32, i32), prefs: Option<&std::path::Path>, req: &Receiver<Option<usize>>, tx: &Sender<Out>) -> anyhow::Result<()> {
    let store = RecordStore::open(dir)?;
    let look = screens::char_select_look(&store, breed, sex)?;
    let cache = cached(dir, prefs, char_id).or_else(|| listed_appearance(char_id, breed, sex, look.model as i32, head));
    let mut assets = cache.as_ref().map(|_| character::actor::ActorAssets::new(&store)).transpose()?;
    let rig = cache.as_ref().map(|c| character::load_cached_character_rig(&store, assets.as_ref().unwrap(), c)).transpose()?;
    let first = match rig.as_ref() {
        Some((model, rig)) => {
            let anim = assets.as_mut().unwrap().role(&store, *model, &Role::Idle)?.ok_or_else(|| anyhow::anyhow!("no idle clip"))?;
            character::cached_character_scene(rig, Some((&anim, 0.0)))
        }
        None => scene(&store, &look, (Role::Idle, 0.0))?,
    };
    if tx.send(Out::First(Box::new(first))).is_err() {
        return Ok(());
    }
    let mut done = vec![];
    let mut queue = vec![None];
    loop {
        let clip = match queue.pop() {
            Some(c) => c,
            None => match req.recv() {
                Ok(c) => c,
                Err(_) => return Ok(()),
            },
        };
        while let Ok(more) = req.try_recv() {
            queue.push(more);
        }
        if done.contains(&clip) {
            continue;
        }
        done.push(clip);
        let role = clip.map_or(Role::Idle, |i| Role::Emote(screens::SOCIALS[i].to_string()));
        let t0 = std::time::Instant::now();
        let f = match rig.as_ref() {
            Some((model, rig)) => {
                let anim = assets.as_mut().unwrap().role(&store, *model, &role)?.ok_or_else(|| anyhow::anyhow!("no clip {}", role.clip_name()))?;
                anyhow::ensure!(anim.signature == rig.cat().signature, "animation does not fit cached model");
                let n = ((anim.duration / 1000.0 * FPS).round() as usize).max(1);
                (0..n).map(|k| {
                    let mut s = character::cached_character_scene(rig, Some((&anim, k as f32 * 1000.0 / FPS)));
                    s.textures.clear();
                    s
                }).collect()
            }
            None => frames(&store, &look, role)?,
        };
        if std::env::var_os("AOMAC_PERF").is_some() {
            eprintln!("preview clip {clip:?}: {} frames in {:.0} ms", f.len(), t0.elapsed().as_secs_f32() * 1000.0);
        }
        if tx.send(Out::Clip(clip, f)).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_cached_bodies_resolve_per_character_without_changing_heads_or_equipment() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let client = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !client.join("cd_image/rdb.db").exists() { return; }
        let store = RecordStore::open(&client).unwrap();
        let dir = std::env::temp_dir().join(format!("aomac-body-preview-{}", std::process::id()));
        let mut cache = character::ViewerCache::default();
        for (id, breed, sex, fatness, head) in [(71, 1, 2, 0, 40681), (72, 1, 3, 1, 40803), (73, 4, 1, 2, 40099)] {
            let mut c = character::CachedCharacter { id, breed, sex, fatness, mesh_id: 17530, head_id: head - 1, ..Default::default() };
            c.meshes.push(character::MeshEntry { attractor: 0, mesh_id: head, ..Default::default() });
            let mut equipment = character::Equipment::default();
            equipment.wear(character::ClothPart::Body, 154207);
            c.set_equipment(&equipment);
            cache.update(c);
        }
        cache.save(&dir).unwrap();
        let mut bodies = Vec::new();
        for id in [71, 72, 73] {
            let original = &cache.0[&id];
            let expected = body_mesh(&store, original.breed, original.sex, original.fatness).unwrap();
            let resolved = cached(&client, Some(&dir), id).unwrap();
            assert_eq!(&resolved, original, "cache lookup preserves stored appearance");
            let assets = character::actor::ActorAssets::new(&store).unwrap();
            let (model, rig) = character::load_cached_character_rig(&store, &assets, &resolved).unwrap();
            assert_eq!(model, expected as u32, "shared loader repairs legacy body");
            assert_eq!(resolved.head_mesh(), original.head_mesh(), "attractor-0 head keeps precedence");
            assert_ne!(model, 17530);
            let anim = character::load_anim(&store, character::role_anim(&store, model, &Role::Idle).unwrap()).unwrap();
            for ms in [0.0, 50.0] {
                let scene = character::cached_character_scene(&rig, Some((&anim, ms)));
                assert_eq!(scene.meshes.len(), rig.model().meshes.len());
                assert_eq!(scene.instances.len(), scene.meshes.len());
            }
            bodies.push(model);
        }
        assert!(bodies[0] != bodies[1] && bodies[1] != bodies[2] && bodies[0] != bodies[2]);
        assert!(cached(&client, Some(&dir), i32::MAX).is_none());
        assert_eq!(character::ViewerCache::load(&dir).0, cache.0, "read repair does not rewrite other entries");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn live_existing_head_and_equipment_replace_creation_cache() {
        let mut u = include_str!("../../../../docs/captures/zone_newchar_ithaca.rec").lines().filter_map(|line| {
            let mut p = line.split(' ');
            let (_, direction, hex) = (p.next()?, p.next()?, p.next()?);
            if direction != "<" { return None; }
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
            let (frame, _) = ao_net::frame::Frame::decode_with(&bytes, false).ok()??;
            match ao_net::n3::decode(&frame).ok()?.body {
                ao_net::n3::N3::Dynel(ao_net::n3::dynel::Dynel::SimpleCharFullUpdate(u)) if u.name == "Aomacvolk" => Some(*u),
                _ => None,
            }
        }).next().expect("captured own update");
        let dir = std::env::temp_dir().join(format!("aomac-live-preview-{}", std::process::id()));
        remember_created_in(&dir, 17, &ao_net::msg::CreateCharacterRequest { head: 40682, ..Default::default() }, 5907).unwrap();
        u.flags &= !ao_net::n3::dynel::flag::SET_DYNEL_800;
        u.head_mesh = Some(40099);
        u.attractors = vec![ao_net::n3::dynel::AttractorMesh { place: 0, mesh: 40099, field: 0, byte: 4 },
            ao_net::n3::dynel::AttractorMesh { place: 1, mesh: 7796, field: 0, byte: 2 }];
        u.cloth.push(ao_net::n3::dynel::ClothData { raw: 1, texture: 154207, page: 0, extra: None });
        remember_live_in(&dir, 18, &u, 5900).unwrap();
        let cache = character::ViewerCache::load(&dir);
        assert_eq!(cache.0[&17].head_mesh(), 40682);
        let c = &cache.0[&18];
        assert_eq!((c.head_id, c.head_mesh(), c.mesh_id), (40099, 40099, 5900));
        assert_eq!(c.equipment().0[1], Some(154207));
        assert!(c.meshes.iter().any(|m| m.attractor == 1 && m.mesh_id == 7796 && m.flags == 2));
        u.cloth.clear();
        remember_live_in(&dir, 18, &u, 5900).unwrap();
        assert!(character::ViewerCache::load(&dir).0[&18].equipment().0.iter().all(Option::is_none));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn created_head_survives_reload_under_acknowledged_identity() {
        let dir = std::env::temp_dir().join(format!("aomac-created-preview-{}", std::process::id()));
        let req = ao_net::msg::CreateCharacterRequest { breed: 4, gender: 2, head: 40682, width: 2, ..Default::default() };
        remember_created_in(&dir, 33512, &req, 5907).unwrap();
        let req2 = ao_net::msg::CreateCharacterRequest { head: 40683, ..req.clone() };
        remember_created_in(&dir, 33513, &req2, 5908).unwrap();
        let cache = character::ViewerCache::load(&dir);
        assert_eq!(cache.0[&33512].head_mesh(), 40682);
        assert_eq!(cache.0[&33513].head_mesh(), 40683);
        assert_eq!((cache.0[&33512].mesh_id, cache.0[&33512].fatness), (5907, 2));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod listed_tests {
    #[test]
    fn supplied_list_head_is_used_and_missing_legacy_head_falls_back() {
        let c = super::listed_appearance(9, 4, 2, 5907, 40683).unwrap();
        assert_eq!((c.id, c.head_mesh()), (9, 40683));
        assert!(super::listed_appearance(9, 4, 2, 5907, 0).is_none());
        assert!(super::listed_appearance(9, 4, 2, 5907, -1).is_none());
    }
}
