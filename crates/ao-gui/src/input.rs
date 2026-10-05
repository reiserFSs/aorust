//! Plain input/event structs (no winit types).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Enter,
    Tab,
    Escape,
    /// `A`..`Z` letters for Ctrl shortcuts (select all / copy / cut / paste).
    Letter(char),
}

#[derive(Clone, Debug)]
pub enum InputEvent {
    MouseMove { x: f32, y: f32 },
    MouseDown { x: f32, y: f32, button: MouseButton },
    MouseUp { x: f32, y: f32, button: MouseButton },
    /// `dy` in wheel notches (positive = up).
    Wheel { x: f32, y: f32, dy: f32 },
    Key { key: Key, pressed: bool, mods: Modifiers },
    /// Text produced by the key press (already layout/IME translated).
    Text(String),
    /// Clipboard contents for a paste the app performed (Ctrl+V).
    Paste(String),
}

pub type WindowId = usize;
/// Handle of a view subtree created with `Gui::add_view` (e.g. one `CharacterSelectionItem`).
pub type ViewHandle = usize;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A `Button`/`TextButton` was clicked (mouse released inside, or activated through the default button).
    Clicked { window: WindowId, view: String, item: Option<ViewHandle> },
    TextChanged { window: WindowId, view: String, text: String },
    /// Enter pressed in a `TextInputView` (`TextInputView_c::SlotEnterPressed`).
    EnterPressed { window: WindowId, view: String },
    ComboChanged { window: WindowId, view: String, index: usize, text: String },
    /// Text the app should put on the clipboard (Ctrl+C / Ctrl+X).
    Copy(String),
    /// Ctrl+V pressed: the app should answer with `InputEvent::Paste`.
    PasteRequested,
    Escape { window: WindowId },
    /// The frame's close button was clicked.
    CloseRequested { window: WindowId },
}
