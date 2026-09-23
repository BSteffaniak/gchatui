use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpaceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub id: SpaceId,
    pub display_name: String,
    pub kind: SpaceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceKind {
    Space,
    GroupChat,
    DirectMessage,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    pub resource_name: String,
    pub display_name: Option<String>,
    pub kind: SenderKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderKind {
    Human,
    Bot,
    Anonymous,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub thread_id: Option<ThreadId>,
    pub sender: Option<Sender>,
    pub text: String,
    pub create_time: String,
    pub is_thread_reply: bool,
    pub unsupported_content: bool,
    pub rich_content: Vec<RichContent>,
}

/// Read-only content independent of Google wire formats and terminal widgets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichContent {
    pub title: String,
    pub text: String,
    pub image_url: Option<String>,
    pub links: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page_token: Option<PageToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageToken(pub String);
