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
    /// The lower-case character of a character key (`a`..`z`, `0`..`9`): Ctrl shortcuts (select all / copy / cut / paste) and the window hotkeys.
    /// A key that types text is delivered as `Key` (press) followed by `Text`.
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
    /// The style-1 frame's close button was released over itself (`WndBorder::SlotCloseButton` 0x10159705
    /// posts message 0x98968b to the window; the application decides: quit for LoginWindow, ignore for the progress window).
    CloseRequested { window: WindowId },
    /// A read-only `TextView` was clicked on an `<a href=..>` run (`TextRenderer_c::GetHyperLink` 0x10162c36); activation on mouse-down is a guess.
    LinkClicked { window: WindowId, view: String, href: String },
    /// The left button was dragged over a `CanvasView` (mouse delta since the last step, pixels; `x`,`y` = the mouse relative to the view).
    CanvasDrag { window: WindowId, view: String, dx: f32, dy: f32, x: f32, y: f32 },
    /// A mouse button went down on a `CanvasView` (`x`,`y` relative to the view); `clicks` is 2 for the second press of a double click
    /// (same view and button, within [`crate::DOUBLE_CLICK_TIME`] seconds and 4 px; UNRESOLVED: the original's threshold).
    CanvasPress { window: WindowId, view: String, x: f32, y: f32, button: MouseButton, clicks: u8 },
    /// The left button that went down on a `CanvasView` was released (anywhere): the canvas's pressed state ends.
    CanvasRelease { window: WindowId, view: String },
    /// Left click (press and release within 3 px) on a `CanvasView`; `x`,`y` relative to the view.
    CanvasClick { window: WindowId, view: String, x: f32, y: f32 },
    /// Mouse wheel over a `CanvasView` (`dy` notches, positive = up; `x`,`y` relative to the view).
    CanvasWheel { window: WindowId, view: String, dy: f32, x: f32, y: f32 },
    /// The left button went down on the icon button of a style-0 frame that is movable (`WndBorder::SlotIconButton` 0x1015a74e opens the window's
    /// icon menu; `x`,`y` = the button's bottom-left, where the menu goes).
    FrameIcon { window: WindowId, x: i32, y: i32 },
    /// A tab of a style-0 frame was pressed (`TabView` selection): the engine already switched `Gui::window_tabs`'s selected index.
    TabSelected { window: WindowId, index: usize },
    /// A tab dragged beyond 4 px (**GUESS** threshold) was released: `target` = the tab strip of a (possibly the same) window under the pointer with the
    /// insert index, else `None` (dropped outside every strip; `FUN_10097d0b` tears the tab out into a new window).
    TabDropped { window: WindowId, tab: usize, x: i32, y: i32, target: Option<(WindowId, usize)> },
    /// The user moved or resized a style-0 frame (`WndBorder::MouseMove` 0x10159c27 -> `DoSetFrame` 0x10159888); read it with `Gui::window_outer_frame`.
    WindowFrame { window: WindowId },
    /// Right button went down in a window marked with `Gui::set_window_context` (`ChatView_c::MouseDown` 0x1008f5dd button 2); `link` = the `<a href>` under the pointer.
    ContextMenu { window: WindowId, x: i32, y: i32, link: Option<String> },
    /// An entry of the popup menu opened with `Gui::open_menu` was chosen.
    MenuPicked { id: u32 },
    /// A slider item of the popup menu changed (`value` 0.0..=1.0); the menu stays open.
    MenuSlider { id: u32, value: f32 },
}
