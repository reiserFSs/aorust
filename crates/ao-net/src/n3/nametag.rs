//! Overhead name tags and target indicators, as drawn by GUI.dll `TargetingModule_t` / `Indicator_t`. Pure rules (text
//! composition, colours, geometry, distances); evidence and addresses in `docs/zone/motion.md` §6 (GUI = GUI.dll).

/// Nametags exist only while the pref `ShowAllNames` (IndependentPrefs, `OptionPanel/Root.xml` line 331, category `Login`, int,
/// absent from every prefs xml = 0) is 1; `HandleNametags` [GUI 0x10025d96] then rebuilds the set every 2 s.
pub const PREF_SHOW_ALL_NAMES: &str = "ShowAllNames";
/// Second line with the organisation name: pref `IsOrgNameShownOverHead` (category `Char`, `Root.xml` line 332), 0 by default.
pub const PREF_SHOW_ORG_NAME: &str = "IsOrgNameShownOverHead";
/// Seconds between two rebuilds of the nametag set (`_DAT_101ae17c`, the timer accumulates `Timer_t` frame time).
pub const NAME_TAG_REFRESH_SECS: f32 = 2.0;
/// `N3Msg_GetDynelsInVicinity` radius [GC 0x1001df09] (`_DAT_10157500`): characters (kind 50000) within 30 m of the client
/// character, not the client character itself, with stat `InPlay` (0xC2) ≠ 0 and Features bit 7 (0x80) clear.
pub const NAME_TAG_RADIUS: f32 = 30.0;
/// Bitmap font of the tag text: `TextOutput_t(font 2, …)` = `FontGameShell12.fnt` (`textures/fonts/`, table @ GUI 0x10272e18).
pub const NAME_TAG_FONT: &str = "FontGameShell12.fnt";
/// Backing sprite: 32 px high, 128 px wide (256 when the text is wider than 128 px); drawn as a world-space billboard of
/// `width/128` × 0.3 world units (`VisualSprite_t(w·0.0078125, 0.3, material "[3] TargetIndicatorMat")`, priority 6).
pub const NAME_TAG_SPRITE_HEIGHT_PX: u32 = 32;
pub const NAME_TAG_SPRITE_HEIGHT: f32 = 0.3;

/// `FontSystem_t::SetColorsIntoMap` [GUI 0x1012e316]: the ColorCode_e → 0xRRGGBB table that `ATextString_t::InsertColor` codes
/// index (control byte 0x10 + code in the string).
pub fn color_code_rgb(code: u8) -> Option<u32> {
    Some(match code {
        0 => 0x00ff00,
        1 | 2 => 0x30d2ff,
        3 => 0xffffc9,
        4 => 0x00ffff,
        5 => 0xffff45,
        6 => 0x00a651,
        7 => 0x63e689,
        8 => 0x0ff20b,
        9 => 0xff0099,
        10 => 0x2299ff,
        11 => 0x000000,
        12 => 0xff0000,
        13 => 0x00f000,
        14 => 0x0000ff,
        15 => 0xffffff,
        16 => 0xffff00,
        17 => 0xccaa44,
        18 => 0xdddd44,
        19 => 0x00dd44,
        20 => 0x66aa66,
        21 => 0xffffff,
        22 => 0x9ad5d9,
        23 | 24 => 0xff0000,
        25 => 0xd9d9d2,
        26 => 0x999926,
        27 => 0xff7718,
        28 => 0x8cb6ff,
        29 => 0xffff00,
        30 => 0xffe3a1,
        31 => 0x00ee00,
        32 => 0xeeeeee,
        33 => 0xcccccc,
        34 => 0xff63ff,
        35 => 0xffffff,
        0x32 => 0xff8cfc,
        0x33 => 0xffffff,
        0x34 => 0xff61a6,
        0x35 => 0x6699ff,
        0x36 => 0x66ff99,
        0x37 => 0x00f000,
        0x50 => 0xeeeeee,
        _ => return None,
    })
}

/// Stat 0 (`Flags`) bits 22/23 → colour code of a player name: 0 none, 1 → 31 (`CCShowFullNameColor` 0x00ee00),
/// 2 and 3 → 10 (`CCLinkColor` 0x2299ff) (table @ GUI 0x1026349c). Bit 22 also means "has first and last name strings".
pub fn flags_color_code(flags: i32) -> Option<u8> {
    let b = (flags as u32) >> 22;
    match (b & 2) + (b & 1) {
        0 => None,
        1 => Some(31),
        _ => Some(10),
    }
}

/// Stat value the client treats as "not set" (`N3Msg_GetSkill` result 0x499602D2).
pub const INVALID_STAT: i32 = 0x4996_02D2;

/// Everything the tag text depends on (`FUN_10024e14` [GUI 0x10024e14]); strings are as the N3 interface returns them.
#[derive(Debug, Clone, Default)]
pub struct NameTagInput<'a> {
    /// `N3Msg_GetName`.
    pub name: &'a str,
    /// `N3Msg_GetTitleName`, `N3Msg_GetFirstName` / `GetLastName` (used when `flags` bit 22 is set).
    pub title: &'a str,
    pub first_name: &'a str,
    pub last_name: &'a str,
    /// `N3Msg_IsNpc`.
    pub is_npc: bool,
    /// Stat `Flags` (0).
    pub flags: i32,
    /// Stat `Features` (224) or [`INVALID_STAT`].
    pub features: i32,
    /// Stat `VisualFlags` (673); bit 9 = "= name =".
    pub visual_flags: i32,
    /// Stat `ShadowBreed` (532) and `N3Msg_GetBreedStr`.
    pub shadow_breed: i32,
    pub breed_str: &'a str,
    /// Stat 345 (`HasAlwaysLootable` in the client's table): a valid non-zero value colours a player name red.
    pub stat_345: i32,
    /// Organisation name (`N3Msg_GetClanString`) and the pref [`PREF_SHOW_ORG_NAME`].
    pub clan: Option<&'a str>,
    pub show_org: bool,
    /// Stat `Side` (0x21): 0 neutral, 1 clan, 2 omni.
    pub side: i32,
    /// Battle-station zones: `(own side stat 0x29C, this dynel's)`; Some only while `N3Msg_IsBattleStation`.
    pub battlestation: Option<(i32, i32)>,
    /// `IgnoreSystem_t::IsCharacterIgnored`: the ignored icon (`GFX_GUI_IGNOREDICON`) is stamped over the plate.
    pub ignored: bool,
}

/// Result of [`name_tag`].
#[derive(Debug, Clone, PartialEq)]
pub struct NameTag {
    /// The tag line, e.g. `"Dr. John Doe the Atrox"`, `"** Name **"`, `"= Name ="`.
    pub text: String,
    /// `ATextString_t::InsertColor(0, code)` colour code of the whole line, if any ([`color_code_rgb`]).
    pub color_code: Option<u8>,
    /// Colour of the line when `color_code` is `None`: 0xffffff, red 0xff0000 in a battle-station zone vs. another side.
    pub base_rgb: u32,
    /// Organisation line (text, rgb): shown when [`NameTagInput::show_org`] and the dynel has an org name.
    pub org: Option<(String, u32)>,
    pub ignored: bool,
}

impl NameTag {
    /// The colour the line is drawn in.
    pub fn rgb(&self) -> u32 {
        self.color_code.and_then(color_code_rgb).unwrap_or(self.base_rgb)
    }
}

fn valid(v: i32) -> bool {
    v != INVALID_STAT
}

/// Compose the tag exactly like `FUN_10024e14`.
pub fn name_tag(i: &NameTagInput) -> NameTag {
    let mut t = String::new();
    if !i.title.is_empty() {
        t.push_str(i.title);
        t.push(' ');
    }
    let two_names = (i.flags as u32) >> 22 & 1 != 0;
    if two_names && !i.first_name.is_empty() {
        t.push_str(i.first_name);
        t.push(' ');
    }
    if i.is_npc {
        t.push_str(i.name);
    } else if valid(i.features) && i.features & 0x4000001 != 0 {
        t.push_str("** ");
        t.push_str(i.name);
        t.push_str(" **");
    } else if i.visual_flags >> 9 & 1 != 0 {
        t.push_str("= ");
        t.push_str(i.name);
        t.push_str(" =");
    } else {
        t.push_str(i.name);
    }
    if two_names && !i.last_name.is_empty() {
        t.push(' ');
        t.push_str(i.last_name);
    }
    if !i.is_npc && i.shadow_breed != 0 && valid(i.shadow_breed) {
        t.push_str(" the ");
        t.push_str(i.breed_str);
    }
    let mut color_code = flags_color_code(i.flags);
    if color_code.is_none() && !i.is_npc && valid(i.stat_345) && i.stat_345 != 0 {
        color_code = Some(12);
    }
    let base_rgb = match i.battlestation {
        Some((own, other)) if !i.is_npc && own != other => 0xff0000,
        _ => 0xffffff,
    };
    let org = if i.show_org {
        i.clan.map(|c| (c.to_string(), org_color(i.side)))
    } else {
        None
    };
    NameTag { text: t, color_code, base_rgb, org, ignored: i.ignored }
}

/// Colour of the organisation line by `Side` (stat 0x21): omni `0xaa88aa`, clan `0x8888aa`, anything else (neutral) `0x88aaaa`.
pub fn org_color(side: i32) -> u32 {
    match side {
        2 => 0xaa88aa,
        1 => 0x8888aa,
        _ => 0x88aaaa,
    }
}

/// `name_tag_color`: the text colour as RGBA (alpha 255).
pub fn name_tag_color(tag: &NameTag) -> [u8; 4] {
    let c = tag.rgb();
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
}

/// Sprite width in pixels for a text of `text_px` pixels (`FontGameShell12`): 128, or 256 above 128.
pub fn sprite_width_px(text_px: u32) -> u32 {
    if text_px <= 128 { 128 } else { 256 }
}

/// World size `(w, h)` of the billboard for a sprite of `width_px`.
pub fn sprite_world_size(width_px: u32) -> (f32, f32) {
    (width_px as f32 / 128.0, NAME_TAG_SPRITE_HEIGHT)
}

/// Where the tag hangs, in the dynel's model space: the position of attractor 0 (`Attractor01_head`,
/// `AttractorMesh::GetName(0)` [DS 0x10071ca2]) plus 0.5 m up (`VisualCATMesh_t::GetIndicatorPosition` [DS 0x100731e2]); a
/// missing/zero attractor gives `(0, 1.5, 0)` + 0.5. The result is transformed by the mesh's world matrix (rotation, body
/// scale `MonsterScale/100`, translation), i.e. the tag height scales with the character.
pub fn name_tag_offset(head_attractor: Option<[f32; 3]>) -> [f32; 3] {
    let a = match head_attractor {
        Some(p) if p != [0.0; 3] => p,
        _ => [0.0, 1.5, 0.0],
    };
    [a[0], a[1] + 0.5, a[2]]
}

/// World position of the tag of a dynel at `pos` with heading `yaw` and `scale` (= `MonsterScale/100`).
pub fn name_tag_world_pos(pos: [f32; 3], yaw: f32, scale: f32, head_attractor: Option<[f32; 3]>) -> [f32; 3] {
    let o = name_tag_offset(head_attractor);
    let (s, c) = yaw.sin_cos();
    [pos[0] + scale * (c * o[0] + s * o[2]), pos[1] + scale * o[1], pos[2] + scale * (-s * o[0] + c * o[2])]
}

/// Mesh without a CAT skeleton (`VisualMesh_t::GetIndicatorPosition` [DS 0x1006c3b0]): twice the tallest sub-mesh extent + 0.3 m.
pub fn mesh_indicator_height(mesh_height: f32) -> f32 {
    mesh_height + 0.3
}

/// `n3EngineClientAnarchy_t::N3Msg_Consider` [GC 0x17496] ratio for a SimpleChar: `(range − own + target) / (2·range)`
/// with `range` = stat 0x113 of the client character, or −1 when `own − range > target` or `range` is unset.
pub fn consider_ratio(target_level: i32, own_level: i32, range: i32) -> f32 {
    if range == INVALID_STAT || range <= 0 || own_level - range > target_level {
        return -1.0;
    }
    (((range - own_level) + target_level) as f32 / (range as f32 * 2.0)).min(1.0)
}

/// Fill colour of the health bar (`FUN_10024af1` [GUI 0x10024af1]): grey 0xaaaaaa for a negative ratio, green→yellow
/// `(255·2r, 255, 0)` below 0.5, yellow→red `(255, 240·2(1−r), 0)` from 0.5.
pub fn con_color(ratio: f32) -> [u8; 4] {
    if ratio.is_nan() || ratio < 0.0 {
        return [0xaa, 0xaa, 0xaa, 255];
    }
    let r = ratio.min(1.0);
    if r < 0.5 {
        [(r as f64 * 255.0 * 2.0) as u8, 255, 0, 255]
    } else {
        [255, ((1.0 - r as f64) * 240.0 * 2.0) as u8, 0, 255]
    }
}

/// Target/attacker indicator health bar (`FUN_10024c03` [GUI 0x10024c03]; only indicators with the health-bar flag, i.e.
/// the selection and attack indicators, never the plain nametags): a 64×4 px bar at y = 14..18 centred in the sprite;
/// returns the filled width in pixels (`min(health, max)·64/max`, max 0 counts as 1). Background 0x333333.
pub fn health_bar_fill_px(health: i32, max_health: i32) -> u32 {
    let m = max_health.max(1) as f64;
    (health.min(max_health.max(1)).max(0) as f64 * 64.0 / m) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> NameTagInput<'static> {
        NameTagInput { name: "Stanko", features: INVALID_STAT, stat_345: INVALID_STAT, shadow_breed: INVALID_STAT, ..Default::default() }
    }

    #[test]
    fn plain_and_decorated_names() {
        assert_eq!(name_tag(&input()).text, "Stanko");
        let npc = name_tag(&NameTagInput { is_npc: true, name: "green lizard", ..input() });
        assert_eq!((npc.text.as_str(), npc.color_code, npc.rgb()), ("green lizard", None, 0xffffff));
        assert_eq!(name_tag(&NameTagInput { features: 1, ..input() }).text, "** Stanko **");
        assert_eq!(name_tag(&NameTagInput { visual_flags: 0x200, ..input() }).text, "= Stanko =");
        // bit 22 of Flags: first and last name, green "full name" colour
        let full = name_tag(&NameTagInput { flags: 1 << 22, first_name: "Stan", last_name: "Ko", ..input() });
        assert_eq!((full.text.as_str(), full.color_code, full.rgb()), ("Stan Stanko Ko", Some(31), 0x00ee00));
        let breed = name_tag(&NameTagInput { shadow_breed: 3, breed_str: "Nanomage", ..input() });
        assert_eq!(breed.text, "Stanko the Nanomage");
        assert_eq!(name_tag(&NameTagInput { stat_345: 1, ..input() }).rgb(), 0xff0000);
    }

    #[test]
    fn org_line_and_battlestation() {
        let t = name_tag(&NameTagInput { clan: Some("Rebels"), show_org: true, side: 2, ..input() });
        assert_eq!(t.org, Some(("Rebels".into(), 0xaa88aa)));
        assert_eq!(org_color(1), 0x8888aa);
        assert_eq!(org_color(0), 0x88aaaa);
        assert_eq!(name_tag(&NameTagInput { clan: Some("x"), show_org: false, ..input() }).org, None);
        let bs = name_tag(&NameTagInput { battlestation: Some((1, 2)), ..input() });
        assert_eq!(name_tag_color(&bs), [255, 0, 0, 255]);
    }

    #[test]
    fn con_colours_and_bar() {
        assert_eq!(con_color(-1.0), [0xaa, 0xaa, 0xaa, 255]);
        assert_eq!(con_color(0.0), [0, 255, 0, 255]);
        assert_eq!(con_color(0.25), [127, 255, 0, 255]);
        assert_eq!(con_color(0.5), [255, 240, 0, 255]);
        assert_eq!(con_color(1.0), [255, 0, 0, 255]);
        assert_eq!(consider_ratio(10, 10, 20), 0.5);
        assert_eq!(consider_ratio(1, 30, 20), -1.0);
        assert_eq!(health_bar_fill_px(50, 100), 32);
        assert_eq!(health_bar_fill_px(500, 100), 64);
    }

    #[test]
    fn geometry() {
        assert_eq!(name_tag_offset(None), [0.0, 2.0, 0.0]);
        assert_eq!(name_tag_offset(Some([0.0, 1.7, 0.1])), [0.0, 2.2, 0.1]);
        let p = name_tag_world_pos([10.0, 5.0, 20.0], 1.0, 2.0, Some([0.0, 1.0, 0.0]));
        assert_eq!(p, [10.0, 8.0, 20.0]);
        assert_eq!((sprite_width_px(100), sprite_width_px(129)), (128, 256));
        assert_eq!(sprite_world_size(256), (2.0, 0.3));
        assert_eq!(color_code_rgb(12), Some(0xff0000));
        assert_eq!(color_code_rgb(10), Some(0x2299ff));
    }
}
