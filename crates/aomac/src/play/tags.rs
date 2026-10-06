//! World-space name tags and target indicators (`Indicator_t`, docs/zone/motion.md §6): every tag is a 32 px high sprite (name line,
//! optional organisation line, plate and health bar for selection/attack indicators) drawn into an RGBA image exactly like
//! `FUN_10024e14` / `FUN_10024c03` (GUI.dll), shown as a camera-facing quad `(width/128) m` wide and `0.3 m` high.
//! `[3] TargetIndicatorMat` enables alpha blending (randy31 material 0x10040645); `RSprite` 0x10013575 adds
//! alpha testing `>30/255` and depth writes without disabling that blending. Filtered edge alpha is retained,
//! not inflated into an opaque cutout. Perspective shrinks the actor-layer quad with distance.
//! Size is not a screen-space font clamp: GUI 0x10024e14 passes these dimensions to DisplaySystem 0x1006ec9b,
//! which forwards them unchanged to `RSprite(..., SpriteMode 1)`; randy31 0x10013ab2 replaces only the rotation.

use super::zone::Zone;
use ao_formats::stats;
use ao_gui::{GfxId, Gui};
use ao_net::n3::nametag::{self as nt, IndicatorKind};
use ao_render::Host;
use ao_scene::{ActorFrame, Blend, Mesh, Scene, Submesh, Texture, TextureKey, Vertex};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Seconds between two rebuilds of the nametag set (`HandleNametags` timer, [`nt::NAME_TAG_REFRESH_SECS`]).
const REFRESH: f32 = nt::NAME_TAG_REFRESH_SECS;
/// Sprite height in pixels (`SpriteInfo_t(w, 0x20, 2)`).
const SPRITE_H: usize = nt::NAME_TAG_SPRITE_HEIGHT_PX as usize;
/// Renderer model key / actor id of slot `n` of [`TagLayer`]: the model key carries a `0x7a67` marker in bits 8..24 next to bit 62 (dynel
/// models are 64-bit hashes, a collision is as unlikely as between two of them), the actor id has bit 31 set (characters stay far
/// below, props start at 0x4000_0000).
const MODEL_BASE: u64 = 0x4000_0000_0000_0000 | 0x007a_6700;
const ACTOR_BASE: u32 = 0x8000_0000;
/// Tags the renderer holds at a time (nearest first beyond that; the original has no limit).
const SLOTS: usize = 96;

/// What one tag shows. `centre` is the quad centre in scene space.
#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub id: i32,
    pub kind: IndicatorKind,
    pub text: String,
    pub rgb: u32,
    pub org: Option<(String, u32)>,
    /// Health bar `(filled px, 0xRRGGBB)` of selection / attack indicators.
    pub bar: Option<(u32, u32)>,
    pub centre: [f32; 3],
}

/// Who gets an indicator over its head besides the nametag set: the selected target (`SetTarget`) and the attacked dynel
/// (`FrameProcess`), with the inputs of the health bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Indicator {
    pub id: i32,
    pub kind: IndicatorKind,
    /// `(Health, MaxHealth)` and the bar colour from `FUN_10024af1` (`Consider`), `None` = no bar (`Consider_e == 0`).
    pub bar: Option<((i32, i32), u32)>,
}

/// The 2 s refresh of the nametag set (`HandleNametags`): `Timer_t` frame time accumulates, at [`REFRESH`] the list is rebuilt from the
/// dynels in range; between rebuilds a listed dynel stays tagged wherever it goes, and a dynel that disappeared loses its tag.
#[derive(Default)]
pub struct Listing {
    timer: f32,
    ids: HashSet<i32>,
}

impl Listing {
    /// A rebuild is due now (`ShowAllNamesCallback` / `ShowOrgNamesCallback` set the timer to 100).
    #[cfg(test)]
    pub fn force(&mut self) {
        self.timer = f32::MAX;
    }

    /// Advances by `dt`; `in_range` yields the ids that qualify now (called only at a rebuild); returns the listed ids that still `exist`.
    pub fn update(&mut self, dt: f32, in_range: impl FnOnce() -> Vec<i32>, exist: impl Fn(i32) -> bool) -> &HashSet<i32> {
        self.ids.retain(|&i| exist(i));
        self.timer += dt;
        if self.timer >= REFRESH {
            self.timer = 0.0;
            self.ids = in_range().into_iter().collect();
        }
        &self.ids
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.timer = f32::MAX;
    }
}

/// An RGBA glyph source for [`raster`]: `(advance, width, height, coverage)`, coverage 0 = keyed, 1 = full, 2.. = grey.
type Glyph = (i32, i32, i32, Vec<u8>);

/// Draws one tag into a `width × 32` RGBA image: transparent (keyed) background, plate halves at both ends, the text lines
/// (`lines`: x, y, 0xRRGGBB, text), then the health bar (`bar`: filled px, fill colour) over the plate. Pixels are copied, not blended
/// (format-2 `SpriteInfo::Copy` 0x1007ae11): every nonkeyed bitmap pixel takes the full tint, not `rgb · intensity`. Keyed pixels are alpha 0 but carry the colour of the
/// first line, so that mip levels / bilinear filtering do not darken the edges of the text.
pub fn raster(layout: &nt::TagLayout, lines: &[(i32, i32, u32, &str)], glyph: &mut dyn FnMut(char) -> Glyph, plate: Option<(u32, u32, &[u8])>, bar: Option<(u32, u32)>) -> Vec<u8> {
    let w = layout.width as usize;
    let bleed = lines.first().map_or(0, |l| l.2);
    let mut img: Vec<u8> = (0..w * SPRITE_H).flat_map(|_| [(bleed >> 16) as u8, (bleed >> 8) as u8, bleed as u8, 0]).collect();
    let mut put = |x: i32, y: i32, rgb: [u8; 3]| {
        if (0..w as i32).contains(&x) && (0..SPRITE_H as i32).contains(&y) {
            let o = (y as usize * w + x as usize) * 4;
            img[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    };
    // `SpriteInfo_t::Copy(sprite, (0,0,pw/2,ph), (0,0), plate, key)` then the right half at `width - pw/2`
    if let Some((pw, ph, px)) = plate {
        let half = pw as i32 / 2;
        for (src_x, dst_x) in [(0, 0), (half, w as i32 - half)] {
            for y in 0..ph as i32 {
                for x in 0..half {
                    let o = ((y as usize * pw as usize) + (src_x + x) as usize) * 4;
                    if px[o + 3] != 0 {
                        put(dst_x + x, y, [px[o], px[o + 1], px[o + 2]]);
                    }
                }
            }
        }
    }
    for &(x0, y0, rgb, text) in lines {
        let mut pen = x0;
        for ch in text.chars() {
            let (adv, gw, gh, bits) = glyph(ch);
            for gy in 0..gh {
                for gx in 0..gw {
                    if bits[(gy * gw + gx) as usize] != 0 {
                        put(pen + gx, y0 + gy, [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]);
                    }
                }
            }
            pen += adv;
        }
    }
    if let Some((fill, rgb)) = bar {
        for y in nt::BAR_Y0..nt::BAR_Y1 {
            for x in 0..nt::BAR_W {
                let c = if (x as u32) < fill { rgb } else { nt::BAR_BACKGROUND };
                put(layout.bar_x + x, y, [(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            }
        }
    }
    img
}

/// Four-vertex quad model of a `width_px` tag (corners as [`nt::billboard_corners`] with the unit camera axes, upright texture) with
/// the unlit alpha-tested/blended, depth-writing material of `[3] TargetIndicatorMat`.
fn quad_model(width_px: u32, rgba: Vec<u8>) -> Scene {
    let key = TextureKey { rdb_type: 0, id: 0x7a67 };
    let corners = nt::billboard_corners([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], width_px);
    let vertices = [(0, [0.0, 0.0]), (1, [1.0, 0.0]), (2, [0.0, 1.0]), (3, [1.0, 1.0])].map(|(i, uv)| Vertex { pos: corners[i], normal: [0.0, 0.0, 1.0], uv, ..Default::default() });
    let mut sub = Submesh::new(vec![0, 2, 3, 0, 3, 1], Some(key));
    sub.blend = Blend::AlphaBlend;
    sub.sprite_alpha_test = true;
    sub.two_sided = true;
    sub.emissive = [1.0; 3];
    let mut scene = Scene::default();
    scene.textures.insert(key, Texture { width: width_px, height: SPRITE_H as u32, rgba });
    scene.meshes.push(Mesh { vertices: vertices.to_vec(), submeshes: vec![sub] });
    scene
}

/// The tags currently held by the renderer: one slot (= model key + actor id) per `(kind, dynel)`, re-uploaded only when what the
/// tag shows changes.
#[derive(Default)]
pub struct TagLayer {
    slots: HashMap<(IndicatorKind, i32), usize>,
    shown: HashMap<usize, u64>,
    scene_generation: u64,
}

fn hash_of(t: &Tag) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&t.text, t.rgb, &t.org, t.bar, t.kind).hash(&mut h);
    h.finish()
}

impl TagLayer {
    fn slot(&mut self, k: (IndicatorKind, i32)) -> Option<usize> {
        if let Some(s) = self.slots.get(&k) {
            return Some(*s);
        }
        let s = (0..SLOTS).find(|n| !self.slots.values().any(|v| v == n))?;
        self.slots.insert(k, s);
        Some(s)
    }

    /// Pushes the quads of `tags` (earlier entries win a full pool) for the camera of `host`; uploads changed sprites.
    pub fn frame(&mut self, gui: &mut Gui, host: &mut Host, tags: &[Tag]) {
        if self.scene_generation != host.scene_generation() {
            self.scene_generation = host.scene_generation();
            self.shown.clear();
        }
        let keep: HashSet<_> = tags.iter().map(|t| (t.kind, t.id)).collect();
        self.slots.retain(|k, _| keep.contains(k));
        let (right, up) = (host.camera.right(), host.camera.up());
        let normal = right.cross(up);
        for t in tags {
            let Some(slot) = self.slot((t.kind, t.id)) else { continue };
            let key = MODEL_BASE + slot as u64;
            let h = hash_of(t);
            if self.shown.get(&slot) != Some(&h) {
                self.shown.insert(slot, h);
                let (layout, rgba) = render(gui, t);
                host.actor_models.push((key, quad_model(layout.width, rgba)));
            }
            let c = |v: ao_render::Vec3| [v.x, v.y, v.z, 0.0];
            host.actors.push(ActorFrame {
                id: ACTOR_BASE + slot as u32,
                model: key,
                transform: [c(right), c(up), c(normal), [t.centre[0], t.centre[1], t.centre[2], 1.0]],
                parts: vec![],
                skin: None,
                always: false,
                alpha: 1.0,
            });
        }
    }
}

/// The indicators the targeting module keeps (`TargetingModule_t`, GUI 0x10025fa4 / 0x100257b0): the selection indicator of the selected
/// target (`zone.target`) and the attack indicator of the dynel the client character attacks (`N3Msg_GetAttackingID` GC 0x10026964 =
/// its fight target while the fight state is not idle = [`Zone::fight_target`]). The health bar needs `Health / MaxHealth` of the
/// dynel (stats 27 / 1; own character: own stats) and the `Consider` colour ([`nt::consider_ratio`] with the target's level, the own
/// level and own stat 0x113, [`nt::con_color`]).
pub fn indicators(zone: &Zone) -> Vec<Indicator> {
    let me = zone.char_id as i32;
    let own_level = zone.skill_value(stats::LEVEL).or_else(|| zone.own().map(|o| o.level)).unwrap_or(0);
    let range = zone.skill_value(0x113).unwrap_or(nt::INVALID_STAT);
    let one = |id: i32, kind| {
        let d = zone.dynels.get(&id)?;
        let (health, max) = if id == me { (zone.skill_value(stats::HEALTH).unwrap_or(d.health), zone.skill_value(stats::LIFE).unwrap_or(d.max_health)) } else { (d.health, d.max_health) };
        let level = if id == me { own_level } else { d.level };
        let [r, g, b, _] = nt::con_color(nt::consider_ratio(level, own_level, range));
        Some(Indicator { id, kind, bar: Some(((health, max), u32::from_be_bytes([0, r, g, b]))) })
    };
    let sel = zone.target.and_then(|id| one(id, IndicatorKind::Selection));
    let att = zone.fight_target.get(&me).and_then(|&id| one(id, IndicatorKind::Attacking));
    sel.into_iter().chain(att).collect()
}

/// Rasterises `t` with the GUI's `FontGameShell12` and plate art.
fn render(gui: &mut Gui, t: &Tag) -> (nt::TagLayout, Vec<u8>) {
    let font = ao_gui::FontId::Shell;
    let org_px = t.org.as_ref().map_or(0, |(s, _)| gui.text_width(font, s));
    let layout = nt::tag_layout(gui.text_width(font, &t.text), org_px);
    let plate = t.kind.plate_gfx().and_then(|g| gui.gfx().image(GfxId(g)).map(|i| (i.width, i.height, i.rgba.clone())));
    let mut glyph = |ch: char| {
        let g = gui.glyph(font, ch);
        (g.advance, g.width, g.height, g.bits.clone())
    };
    let mut lines = vec![(layout.text_x, nt::TEXT_Y, t.rgb, t.text.as_str())];
    if let Some((s, rgb)) = &t.org {
        lines.push((layout.org_x, nt::ORG_Y, *rgb, s.as_str()));
    }
    let rgba = raster(&layout, &lines, &mut glyph, plate.as_ref().map(|p| (p.0, p.1, p.2.as_slice())), t.bar);
    (layout, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 solid glyphs, advance 3; `'g'` is grey (coverage 128).
    fn glyph(ch: char) -> Glyph {
        (3, 2, 2, vec![if ch == 'g' { 128 } else { 1 }; 4])
    }

    fn px(img: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let o = (y * w + x) * 4;
        img[o..o + 4].try_into().unwrap()
    }

    #[test]
    fn text_plate_and_bar_land_where_the_client_puts_them() {
        let layout = nt::tag_layout(6, 0);
        assert_eq!((layout.width, layout.text_x), (128, 61));
        // 4×2 plate: left half red, right half blue, one keyed pixel
        let plate: Vec<u8> = (0..8).flat_map(|i| if i == 0 { [0, 255, 0, 0] } else if i % 4 < 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] }).collect();
        let img = raster(&layout, &[(layout.text_x, nt::TEXT_Y, 0x00ee00, "ab")], &mut glyph, Some((4, 2, &plate)), Some((10, 0xff0000)));
        let w = 128;
        // plate halves at both ends; the keyed pixel stays transparent
        assert_eq!(px(&img, w, 0, 0)[3], 0);
        assert_eq!(px(&img, w, 1, 0), [255, 0, 0, 255]);
        assert_eq!(px(&img, w, 126, 1), [0, 0, 255, 255]);
        // text: y = 1, centred, colour as given; second glyph one advance further
        assert_eq!(px(&img, w, 61, 1), [0, 0xee, 0, 255]);
        assert_eq!(px(&img, w, 64, 2), [0, 0xee, 0, 255]);
        assert_eq!(px(&img, w, 60, 1), [0, 0xee, 0, 0], "keyed pixels carry the text colour");
        // bar: 64 px at x 32.., rows 14..18, 10 px filled, rest 0x333333, nothing outside
        assert_eq!(px(&img, w, 32, 14), [255, 0, 0, 255]);
        assert_eq!(px(&img, w, 41, 17), [255, 0, 0, 255]);
        assert_eq!(px(&img, w, 42, 14), [0x33, 0x33, 0x33, 255]);
        assert_eq!(px(&img, w, 95, 17), [0x33, 0x33, 0x33, 255]);
        assert_eq!((px(&img, w, 96, 14)[3], px(&img, w, 32, 18)[3]), (0, 0));
        // Format-2 bitmap Copy tests the source opacity bit, not its grey intensity.
        let g = raster(&layout, &[(0, 0, 0xffffff, "g")], &mut glyph, None, None);
        assert_eq!(px(&g, w, 0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn listing_refreshes_every_two_seconds() {
        let mut l = Listing::default();
        l.force();
        assert_eq!(l.update(0.016, || vec![1, 2], |_| true).len(), 2);
        // 1.9 s later the in-range query is not asked again; ids that vanished are dropped at once
        assert_eq!(l.update(1.9, || panic!("too early"), |i| i != 2).len(), 1);
        // 2.0 s after the rebuild a new set replaces the old one
        assert!(l.update(0.2, || vec![3], |_| true).contains(&3));
        l.clear();
        assert!(l.update(0.0, Vec::new, |_| true).is_empty());
    }

    #[test]
    fn slots_are_stable_and_recycled() {
        let mut layer = TagLayer::default();
        let a = layer.slot((IndicatorKind::Nametag, 10)).unwrap();
        let b = layer.slot((IndicatorKind::Selection, 10)).unwrap();
        assert_ne!(a, b);
        assert_eq!(layer.slot((IndicatorKind::Nametag, 10)), Some(a));
        layer.slots.remove(&(IndicatorKind::Nametag, 10));
        assert_eq!(layer.slot((IndicatorKind::Attacking, 7)), Some(a));
        for i in 0..SLOTS as i32 {
            layer.slot((IndicatorKind::Nametag, 1000 + i));
        }
        assert_eq!(layer.slot((IndicatorKind::Nametag, 9999)), None);
    }

    #[test]
    fn quad_model_is_an_alpha_tested_unlit_billboard() {
        let m = quad_model(256, vec![0; 256 * SPRITE_H * 4]);
        let s = &m.meshes[0].submeshes[0];
        assert!(s.blend == Blend::AlphaBlend && s.sprite_alpha_test && s.emissive == [1.0; 3] && s.two_sided);
        assert_eq!(s.base_color[3], 1.0, "filtered sprite alpha must remain unscaled for blending");
        let v = &m.meshes[0].vertices;
        assert_eq!((v[1].pos[0] - v[0].pos[0], v[0].pos[1] - v[2].pos[1]), (2.0, 0.3));
        assert_eq!((v[0].uv, v[3].uv), ([0.0, 0.0], [1.0, 1.0]));
        let small = quad_model(128, vec![0; 128 * SPRITE_H * 4]);
        let v = &small.meshes[0].vertices;
        assert_eq!((v[1].pos[0] - v[0].pos[0], v[0].pos[1] - v[2].pos[1]), (1.0, 0.3));
    }
    #[test]
    fn indicators_of_target_and_attacked_dynel() {
        use super::super::zone::DynelState;
        let d = |level, health| DynelState { name: "x".into(), pos: [5.0; 3], yaw: None, npc: true, side: 0, level, health, max_health: 60 };
        let mut z = Zone::new(1);
        z.dynels.insert(1, d(10, 60));
        z.dynels.insert(2, d(20, 30));
        z.dynels.insert(3, d(5, 15));
        assert!(indicators(&z).is_empty());
        z.stats.insert(stats::LEVEL, 10);
        z.stats.insert(0x113, 10);
        z.target = Some(2);
        z.fight_target.insert(1, 3);
        let i = indicators(&z);
        // consider ratio (range - own + target) / (2 range) = 1.0 -> red; (10 - 10 + 5) / 20 = 0.25 -> (127, 255, 0)
        assert_eq!(i, vec![
            Indicator { id: 2, kind: IndicatorKind::Selection, bar: Some(((30, 60), 0xff0000)) },
            Indicator { id: 3, kind: IndicatorKind::Attacking, bar: Some(((15, 60), 0x7fff00)) },
        ]);
        // a target that is not in the world has no indicator
        z.target = Some(99);
        assert_eq!(indicators(&z).len(), 1);
    }

    /// Real data: the sprite of a selection indicator and a nametag is built once and again only when it changes; with
    /// `AOMAC_TAG_SHOT=<png>` three tags at 2, 6 and 12 m are rendered offscreen.
    #[test]
    fn real_sprites_upload_once_and_shrink() {
        let dir = ao_gui::client_dir();
        let Ok(mut gui) = Gui::new(&dir, None) else { return eprintln!("skipping: no client") };
        let tag = |id, kind, text: &str, x: f32, z: f32, bar| Tag { id, kind, text: text.into(), rgb: 0x00ee00, org: None, bar, centre: [x, 0.0, z] };
        let tags = [
            tag(1, IndicatorKind::Selection, "Surf Lizard", -1.0, -2.0, Some((32, 0xff0000))),
            tag(2, IndicatorKind::Attacking, "Aomacvolk the Nanomage, a very long name indeed", 0.0, -6.0, Some((10, 0x00ff00))),
            tag(3, IndicatorKind::Nametag, "Stanko", 1.0, -4.0, None),
        ];
        let (mut layer, mut host) = (TagLayer::default(), Host::headless());
        layer.frame(&mut gui, &mut host, &tags);
        assert_eq!((host.actor_models.len(), host.actors.len()), (3, 3));
        let widths: Vec<u32> = host.actor_models.iter().map(|m| m.1.textures.values().next().unwrap().width).collect();
        assert_eq!(widths, vec![128, 256, 128]);
        // the nametag has no plate or bar: only text pixels; the indicator has the plate corners
        let opaque = |m: &(u64, Scene)| m.1.textures.values().next().unwrap().rgba.chunks(4).filter(|p| p[3] == 255).count();
        assert!(opaque(&host.actor_models[0]) > opaque(&host.actor_models[2]) && opaque(&host.actor_models[2]) > 20);
        let models = host.actor_models.clone();
        let actors = host.actors.clone();
        host.actor_models.clear();
        host.actors.clear();
        layer.frame(&mut gui, &mut host, &tags);
        assert!(host.actor_models.is_empty() && host.actors.len() == 3, "unchanged tags are not uploaded again");
        host.set_scene(Scene::default());
        layer.frame(&mut gui, &mut host, &tags);
        assert_eq!((host.actor_models.len(), host.actors.len()), (3, 3), "all unchanged tag kinds reupload after scene replacement");
        host.actor_models.clear();
        host.actors.clear();
        let mut changed = tags.clone();
        changed[0].bar = Some((40, 0xff0000));
        layer.frame(&mut gui, &mut host, &changed);
        assert_eq!(host.actor_models.len(), 1);
        if let Some(out) = std::env::var_os("AOMAC_TAG_SHOT") {
            ao_render::render_to_png_actors(&Scene::default(), &models, actors, [0.0; 3], [0.0, 0.0, -1.0], 640, 360, std::path::Path::new(&out), 0.0).unwrap();
        }
    }
}
