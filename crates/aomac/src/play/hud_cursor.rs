//! The game's mouse pointer over the world (docs/gui.md §13.2 "Hover pointer"): the rule that picks the pointer colour and its
//! detail icons for the dynel under the mouse and the sprites the original draws.
//!
//! Original (GUI.dll): `InputConfig_t::CheckObjectUnderMouse` 0x10019f00 runs every frame on the object under the mouse and calls
//! `MousePointerModule_t::SetMousePointer(Pointer_e, DetailLeft, DetailRight)`; `MousePointerModule_t::FrameProcess` 0x100202dd →
//! `FUN_10020120` applies the `MouseCursorMode` pref (LoginPrefs.xml default 2), `FUN_1001fddd` places the sprites at the
//! `InputConfig_t` mouse position (`+0x118/+0x11c`), then the request is reset (`Pointer_e` 0, details −1) for the next frame.
//! * Sprite tables (rows of 5 dwords `{hotspot x, hotspot y, colour 0xffffff, gfx id, sprite*}`): pointers @0x10263168, details
//!   @0x10263298, overlays @0x10263128. `Pointer_e` is the row: 0 `STANDARD` 0x135, 1 `_BLUE` 0x136, 2 `_GREEN` 0x13a, 3 `_YELLOW`
//!   0x139, 4 `_PURPLE` 0x138, 5 `_RED` 0x13b, 6 `_GRAY` 0x137, then the system pointers (4WAY_DRAG 0x13c … WAIT 0x143, hotspots
//!   (16,16) (12,12) (12,12) (6,16) (16,6) (5,8) (5,1) (16,16)); all colour pointers have hotspot (0,0). `Detail_e` is the row of the
//!   details: 0 ATTACK 0x146, 1 PICKUP 0x147, 2 OPERATE 0x144, 3 LOOKAT 0x148, 4 QUESTION 0x149, 5 DENIED 0x145, 6 TALK 0x14a,
//!   7 TRADE 0x14b, hotspots (0,0). Overlays: LEFTCLICK 0x14c and RIGHTCLICK 0x14d (hotspot columns (10,20)), BOTHCLICK 0x14e (unused).
//! * Placement (`FUN_1001fddd`): pointer at `pos − hotspot`; the left detail 18 px to the right of that (`IPoint(0x12, 0)`), the
//!   right detail likewise, 27 px further right (`IPoint(0x1b, 0)`) when both exist and differ; the click overlays at the detail
//!   plus the overlay row's (10, 20); with the same detail left and right the right overlay sits at that spot and the left one 9 px
//!   to its left (`IPoint(-9, 0)`). Overlays are not drawn for DENIED, nor in the "simple" mode. The operands of the decompile's `IPoint`
//!   sums share stack slots, so the overlay offset (the row's hotspot columns) is [INFERENCE] from the two function-local statics
//!   `IPoint(hotspot)` built from the overlay rows.
//! * `FUN_10020120`: `MouseCursorMode` 0 clears both details, 1 ("simple") keeps only the left one when both are set and draws no
//!   overlays, 2 ("detailed") draws everything. A negative mouse position hides the pointer.
//! * The character rule (`CheckObjectUnderMouse`, character branch): see [`choose`].
//!
//! UNRESOLVED: the branch for items / doors / corpses (skill `0x1e` flags bits 0, 3, 0x100000: pointer BLUE with PICKUP / OPERATE
//! details) is traced but unreachable: `Zone::dynels` holds characters only, so no other object can be under the pointer here.
//! The own character is never picked (`hud_pick.rs`), so its GREEN `(2, −1, −1)` case does not occur. Others' `Team` (stat 6) and
//! `HasVulnerableFightMode` (Gamecode 0x10016fea: a playfield PvP rule `FUN_1003e228` — 2: sides differ, 3: teams differ and nonzero,
//! 4: always — whose source was not traced) are not known: team counts as different and the rule as 0 (never vulnerable by it).

use ao_gui::{DrawCmd, DrawList, GfxId};

/// `Pointer_e`: the row of the pointer sprite table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pointer {
    #[default]
    Standard,
    Blue,
    Green,
    Yellow,
    Purple,
    Red,
}

impl Pointer {
    /// `GFX_GUI_POINTER_STANDARD*` id of the row.
    pub fn gfx(self) -> GfxId {
        GfxId(match self {
            Pointer::Standard => 0x135,
            Pointer::Blue => 0x136,
            Pointer::Green => 0x13a,
            Pointer::Yellow => 0x139,
            Pointer::Purple => 0x138,
            Pointer::Red => 0x13b,
        })
    }
}

/// `Detail_e`: the row of the detail sprite table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Attack,
    Lookat,
    Talk,
    Trade,
}
// Rows PICKUP 0x147, OPERATE 0x144, QUESTION 0x149 and DENIED 0x145 exist in the table but only the item / door branch of
// `CheckObjectUnderMouse` (unreachable here, see the header) and the overlay rule "no badge for DENIED" use them: not modelled.

impl Detail {
    pub fn gfx(self) -> GfxId {
        GfxId(match self {
            Detail::Attack => 0x146,
            Detail::Lookat => 0x148,
            Detail::Talk => 0x14a,
            Detail::Trade => 0x14b,
        })
    }
}

const LEFT_CLICK: GfxId = GfxId(0x14c);
const RIGHT_CLICK: GfxId = GfxId(0x14d);
/// `IPoint(0x12, 0)`: detail offset from the pointer origin; `IPoint(0x1b, 0)`: second detail; overlay row hotspot columns.
const DETAIL_AT: (i32, i32) = (18, 0);
const SECOND_DETAIL: i32 = 27;
const OVERLAY_AT: (i32, i32) = (10, 20);
const BOTH_SHIFT: i32 = -9;

/// What `SetMousePointer(Pointer_e, DetailLeft, DetailRight)` asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mouse {
    pub pointer: Pointer,
    pub left: Option<Detail>,
    pub right: Option<Detail>,
}

/// The facts `CheckObjectUnderMouse` reads about a hovered character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hover {
    /// `N3Msg_IsNpc`.
    pub npc: bool,
    /// Stat `Flags` (0): bit 22 / 23 are summed as `(f >> 22 & 2) + (f >> 22 & 1)`; the value 2 (bit 23) shows TALK on an NPC.
    pub flags: i32,
    /// Stat `Features` (0xe0) when known (an NPC record's): bit 0x20000000 = not attackable (NPC), 0x4000001 = attackable (player).
    pub features: Option<i32>,
    pub side: i32,
    pub own_side: i32,
    /// Stat `Team` (6) of the character / of the own character, when known (0 = none).
    pub team: Option<i32>,
    pub own_team: i32,
    /// `N3Msg_HasVulnerableFightMode`.
    pub vulnerable: bool,
}

/// The character branch of `CheckObjectUnderMouse` for the character under the pointer (not the own one). `shift` / `ctrl` =
/// `IsShiftDown` / `IsCtrlDown`, `dblclick` = pref `DoubleclickAction`.
/// * An NPC that is not attackable (`Features & 0x20000000`) or has `Flags` value 2: BLUE, with the TALK detail (right) for value 2;
///   any other NPC RED.
/// * A player: TRADE as the right detail and, with `DoubleclickAction`, as the left one. GREEN for a team mate (same nonzero
///   `Team`); else with the same `Side`: PURPLE when attackable (`Features & 0x4000001` or a vulnerable fight mode) else YELLOW; a
///   different `Side`: RED when attackable else YELLOW.
/// * Ctrl: the left detail becomes ATTACK; Shift (after that): LOOKAT.
pub fn choose(h: &Hover, shift: bool, ctrl: bool, dblclick: bool) -> Mouse {
    let talk = ((h.flags >> 22) & 2) + ((h.flags >> 22) & 1);
    let mut m = Mouse::default();
    if h.npc {
        let attackable = h.features.is_none_or(|f| f & 0x2000_0000 == 0);
        if !attackable || talk == 2 {
            m.pointer = Pointer::Blue;
            if talk == 2 {
                m.right = Some(Detail::Talk);
            }
        } else {
            m.pointer = Pointer::Red;
        }
    } else {
        m.right = Some(Detail::Trade);
        m.left = dblclick.then_some(Detail::Trade);
        let attackable = h.vulnerable || h.features.is_some_and(|f| f & 0x400_0001 != 0);
        m.pointer = if h.own_team != 0 && h.team == Some(h.own_team) {
            Pointer::Green
        } else if h.side == h.own_side {
            if attackable { Pointer::Purple } else { Pointer::Yellow }
        } else if attackable {
            Pointer::Red
        } else {
            Pointer::Yellow
        };
    }
    if ctrl {
        m.left = Some(Detail::Attack);
    }
    if shift {
        m.left = Some(Detail::Lookat);
    }
    m
}

/// `FUN_10020120` for `MouseCursorMode` `mode`: the details to show and whether the click overlays are drawn.
pub fn apply_mode(mode: i64, m: Mouse) -> (Mouse, bool) {
    match mode {
        0 => (Mouse { left: None, right: None, ..m }, true),
        1 if m.left.is_some() && m.right.is_some() => (Mouse { right: None, ..m }, false),
        1 => (m, false),
        _ => (m, true),
    }
}

/// `FUN_1001fddd`: the sprites at the pointer position `pos` (GUI pixels), `size` = image size of a gfx id. Nothing for a negative
/// position.
pub fn draw(size: impl Fn(GfxId) -> (u32, u32), mode: i64, pos: (f32, f32), m: Mouse, out: &mut DrawList) {
    if pos.0 < 0.0 || pos.1 < 0.0 {
        return;
    }
    let (m, overlays) = apply_mode(mode, m);
    let mut put = |id: GfxId, x: i32, y: i32| {
        let (w, h) = size(id);
        if w > 0 && h > 0 {
            out.cmds.push(DrawCmd::Gfx { id, src: [0.0, 0.0, w as f32, h as f32], dst: [x as f32, y as f32, (x + w as i32) as f32, (y + h as i32) as f32], tint: [255; 3], alpha: 1.0 });
        }
    };
    let (px, py) = (pos.0 as i32, pos.1 as i32);
    put(m.pointer.gfx(), px, py);
    let (dx, dy) = (px + DETAIL_AT.0, py + DETAIL_AT.1);
    if let Some(l) = m.left {
        put(l.gfx(), dx, dy);
        if overlays {
            put(LEFT_CLICK, dx + OVERLAY_AT.0 + if m.right == Some(l) { BOTH_SHIFT } else { 0 }, dy + OVERLAY_AT.1);
        }
    }
    if let Some(r) = m.right {
        let rx = if m.left.is_some() && m.left != Some(r) { dx + SECOND_DETAIL } else { dx };
        if m.left != Some(r) {
            put(r.gfx(), rx, dy);
        }
        if overlays {
            put(RIGHT_CLICK, rx + OVERLAY_AT.0, dy + OVERLAY_AT.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> Hover {
        Hover { side: 2, own_side: 1, ..Default::default() }
    }

    #[test]
    fn npc_pointer_colours() {
        let npc = Hover { npc: true, ..Default::default() };
        // attackable monster: red, no detail
        assert_eq!(choose(&npc, false, false, true), Mouse { pointer: Pointer::Red, left: None, right: None });
        // "not attackable" feature bit: blue
        assert_eq!(choose(&Hover { features: Some(0x2000_0000), ..npc }, false, false, true).pointer, Pointer::Blue);
        // talkable (Flags bit 23): blue with TALK on the right
        let talk = choose(&Hover { flags: 1 << 23, ..npc }, false, false, true);
        assert_eq!((talk.pointer, talk.right), (Pointer::Blue, Some(Detail::Talk)));
        // bit 22 alone is value 1: no talk
        assert_eq!(choose(&Hover { flags: 1 << 22, ..npc }, false, false, true).pointer, Pointer::Red);
        // Ctrl: left detail ATTACK; Shift wins afterwards: LOOKAT
        assert_eq!(choose(&npc, false, true, true).left, Some(Detail::Attack));
        assert_eq!(choose(&npc, true, true, true).left, Some(Detail::Lookat));
    }

    #[test]
    fn player_pointer_colours_and_trade_details() {
        let p = player();
        let m = choose(&p, false, false, true);
        assert_eq!((m.pointer, m.left, m.right), (Pointer::Yellow, Some(Detail::Trade), Some(Detail::Trade)));
        // without DoubleclickAction only the right button trades
        assert_eq!(choose(&p, false, false, false).left, None);
        // other side + attackable (Features bit): red; same side + attackable: purple; same side peaceful: yellow
        assert_eq!(choose(&Hover { features: Some(1), ..p }, false, false, true).pointer, Pointer::Red);
        assert_eq!(choose(&Hover { side: 1, features: Some(0x400_0000), ..p }, false, false, true).pointer, Pointer::Purple);
        assert_eq!(choose(&Hover { side: 1, ..p }, false, false, true).pointer, Pointer::Yellow);
        assert_eq!(choose(&Hover { vulnerable: true, ..p }, false, false, true).pointer, Pointer::Red);
        // a team mate: green whatever the side; teams of 0 never match
        assert_eq!(choose(&Hover { team: Some(7), own_team: 7, ..p }, false, false, true).pointer, Pointer::Green);
        assert_eq!(choose(&Hover { team: Some(0), own_team: 0, ..p }, false, false, true).pointer, Pointer::Yellow);
    }

    fn sized(id: GfxId) -> (u32, u32) {
        if (0x14c..=0x14e).contains(&id.0) { (16, 16) } else { (32, 32) }
    }

    fn dst(l: &DrawList) -> Vec<(u32, [i32; 2])> {
        l.cmds.iter().map(|c| if let DrawCmd::Gfx { id, dst, .. } = c { (id.0, [dst[0] as i32, dst[1] as i32]) } else { panic!() }).collect()
    }

    #[test]
    fn sprites_are_placed_like_fun_1001fddd() {
        let mut l = DrawList::default();
        // player under the pointer at (100, 50): pointer, TRADE left + overlay, TRADE right shares the sprite, overlays side by side
        draw(sized, 2, (100.0, 50.0), choose(&player(), false, false, true), &mut l);
        assert_eq!(dst(&l), vec![(0x139, [100, 50]), (0x14b, [118, 50]), (0x14c, [119, 70]), (0x14d, [128, 70])]);
        // Ctrl: ATTACK left (27 px left of the TRADE right detail) with its own overlays
        let mut l = DrawList::default();
        draw(sized, 2, (100.0, 50.0), choose(&player(), false, true, true), &mut l);
        assert_eq!(dst(&l), vec![(0x139, [100, 50]), (0x146, [118, 50]), (0x14c, [128, 70]), (0x14b, [145, 50]), (0x14d, [155, 70])]);
        // negative position: nothing
        let mut l = DrawList::default();
        draw(sized, 2, (-1.0, 5.0), Mouse::default(), &mut l);
        assert!(l.cmds.is_empty());
    }

    #[test]
    fn cursor_mode_trims_details_and_overlays() {
        let m = choose(&player(), false, true, true);
        // none: pointer only (details cleared); simple: left detail only, no overlays; detailed: all
        let (none, _) = apply_mode(0, m);
        assert_eq!((none.left, none.right), (None, None));
        assert_eq!(apply_mode(1, m), (Mouse { right: None, ..m }, false));
        assert_eq!(apply_mode(2, m), (m, true));
        let mut l = DrawList::default();
        draw(sized, 1, (0.0, 0.0), m, &mut l);
        assert_eq!(dst(&l), vec![(0x139, [0, 0]), (0x146, [18, 0])]);
    }
}
