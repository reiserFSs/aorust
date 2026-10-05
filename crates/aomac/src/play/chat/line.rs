//! The unit every chat source produces and the chat windows consume.

/// Where a line comes from; selects the `TextColors.xml` colour ([`ChatKind::color_name`]) and the default window/tab.
/// Variants are additive: add one when the original distinguishes a new type (announce in docs/chat.md).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChatKind {
    Error,
    System,
    CmdFeedback,
    /// Outgoing tell (we sent it).
    TellOut,
    /// Incoming tell.
    TellIn,
    Vicinity,
    Shout,
    Whisper,
    /// Public/private chat-server group by its group name (OOC, Newbie Help, Clan, Team, ...).
    Group(String),
    Emote,
    /// Combat log / feedback classes (`CCMeHitOtherColor` ...), named by their TextColors entry.
    Other(&'static str),
}

impl ChatKind {
    /// `TextColors.xml` colour name for the line (`ctch_*`, `ct_*`, `CC*`).
    pub fn color_name(&self) -> &str {
        match self {
            ChatKind::Error => "ct_error",
            ChatKind::System => "ct_system",
            ChatKind::CmdFeedback => "ct_cmd_feedback",
            ChatKind::TellOut => "ct_otell",
            ChatKind::TellIn => "ct_itell",
            ChatKind::Vicinity => "ctch_vicinity",
            ChatKind::Shout => "ctch_shout",
            ChatKind::Whisper => "ctch_whisper",
            ChatKind::Emote => "ctch_emote",
            ChatKind::Group(_) => "ctch_misc", // refined by the Gui owner: per-group colour table of ChatGUIModule_c
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
