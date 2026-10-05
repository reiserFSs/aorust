//! Renderer-agnostic Anarchy Online GUI engine (see `docs/gui.md`).
pub mod draw;
pub mod expr;
pub mod font;
pub mod geom;
pub mod gfx;
pub mod gui;
pub mod input;
pub mod layout;
pub mod text;
pub mod view;
pub mod xml;

pub use draw::{DrawCmd, DrawList, GlyphAtlas};
pub use font::FontId;
pub use geom::{Point, Rect};
pub use gfx::{Atlas, AtlasEntry, GfxId, GfxSet};
pub use gui::{ExtraImage, Gui, Localize, WindowSize, EXTRA_BASE};
pub use input::{Event, InputEvent, Key, Modifiers, MouseButton, ViewHandle, WindowId};
pub use view::tvf;

use std::path::PathBuf;

/// Client root used by real-data tests and examples (`AO_CLIENT_DIR`, default `~/Games/ProjectRubiKa/client`).
pub fn client_dir() -> PathBuf {
    std::env::var_os("AO_CLIENT_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Games/ProjectRubiKa/client")
    })
}
