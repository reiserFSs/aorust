//! The unit every chat source produces and the chat windows consume.

/// Where a line comes from; selects the `TextColors.xml` colour ([`ChatKind::color_name`]) and the default window/tab.
/// Variants are additive: add one when the original distinguishes a new type (announce in docs/chat.md).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChatKind {
    Error,
    System,
    /// Outgoing tell (we sent it).
    TellOut,
    /// Combat log / feedback classes (`CCMeHitOtherColor` ...), named by their TextColors entry.
    Other(&'static str),
}

impl ChatKind {
    /// `TextColors.xml` colour name for the line (`ctch_*`, `ct_*`, `CC*`).
    pub fn color_name(&self) -> &str {
        match self {
            ChatKind::Error => "ct_error",
            ChatKind::System => "ct_system",
            ChatKind::TellOut => "ct_otell",
            ChatKind::Other(n) => n,
        }
    }
}

/// One line of chat. `text` is the final display text in the `TextView_c` HTML subset (`<a href=..>`, `<font color=..>`, `<br>`),
/// exactly what the original's `ChatGUIModule_c::Handle*Message` hands to its window (prefix such as "[Vicinity]" included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub kind: ChatKind,
    pub text: String,
}

impl ChatLine {
    pub fn new(kind: ChatKind, text: impl Into<String>) -> Self {
        Self { kind, text: text.into() }
    }
}

/// A message as `ChatGUIModule_c::Handle{Vicinity,Private,Group,PrivateGroup}Message` (GUI 0x10086728 / 0x1008792e / 0x100863ff)
/// hands it to the window layer (`FUN_10084f9e` = add message to the window of `group`): the window layer does the visible formatting
/// (sender link, prefix, colour). Owner of the formatting: `win.rs` (`ChatWindows::push_msg`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatMsg {
    /// `GetGroupIdentifier` id: chat-server group `(kind << 32) | id`, or a local fixed id (vicinity `0x40000002`, shout `0x41000000`,
    /// whisper `0x41000001`, `0x4200001b`, `0x4200001a` for vicinity message kinds 4..7 -- see docs/chat/net.md).
    pub group: u64,
    /// Name announced by S2C_GROUP_JOIN (empty for local groups).
    pub group_name: String,
    /// Sender character id (0 = none) and name (resolved via S2C_USER_NAME / lookup, empty if unknown).
    pub from_id: u32,
    pub from_name: String,
    /// Text with the server markup already reduced like the handlers do (`RemoteFormat::ParseString` + `HTMLParser_c::ExtractText`).
    pub text: String,
    /// First byte of the message's data block (vicinity: the message kind 2..7), 0 if none.
    pub kind: u8,
    /// Remaining flags of the data block (bit 0 = "ignore system bypass", see Handle*Message).
    pub flags: u8,
    /// Incoming tell (`HandlePrivateMessage`): `group` is 0 and the window is the sender's tell window.
    pub tell: bool,
    /// The data block's voice extras (`TextMacro_t` of `/voice`, `FUN_10085b4a` tag 1): played by the hub.
    pub voice: Option<super::voice::Voice>,
}
