use crate::chat::ChatError;
use crate::model::{Message, PageToken, Space};

pub type RequestId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    MissingConfiguration,
    VaultSetup,
    Unlock,
    Login,
    LoadingSpaces,
    Ready,
    LoadingMessages,
    Refreshing,
    Empty,
    RecoverableError,
    Reauthentication,
    FatalError,
}

#[derive(Debug)]
pub enum ProductMessage {
    SpacesLoaded {
        request_id: RequestId,
        result: Result<(Vec<Space>, Option<PageToken>), ChatError>,
    },
    MessagesLoaded {
        request_id: RequestId,
        space_name: String,
        result: Result<(Vec<Message>, Option<PageToken>), ChatError>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    LoadSpaces {
        request_id: RequestId,
    },
    LoadMessages {
        request_id: RequestId,
        space_name: String,
        page_token: Option<PageToken>,
    },
}

#[derive(Debug)]
pub struct ProductState {
    pub phase: Phase,
    recency: std::collections::BTreeMap<String, String>,
    pub new_activity: std::collections::BTreeSet<String>,
    activity_baselines: std::collections::BTreeMap<String, Option<crate::model::MessageId>>,
    pub spaces: Vec<Space>,
    pub messages: Vec<Message>,
    pub selected_space: Option<String>,
    pub next_message_page: Option<PageToken>,
    next_request_id: RequestId,
    active_spaces_request: Option<RequestId>,
    active_messages_request: Option<(RequestId, String)>,
}

impl Default for ProductState {
    fn default() -> Self {
        Self {
            phase: Phase::MissingConfiguration,
            recency: std::collections::BTreeMap::new(),
            new_activity: std::collections::BTreeSet::new(),
            activity_baselines: std::collections::BTreeMap::new(),
            spaces: Vec::new(),
            messages: Vec::new(),
            selected_space: None,
            next_message_page: None,
            next_request_id: 1,
            active_spaces_request: None,
            active_messages_request: None,
        }
    }
}

impl ProductState {
    pub fn observe_activity(&mut self, space: &str, latest: Option<&crate::model::MessageId>) {
        if let Some(previous) = self
            .activity_baselines
            .insert(space.to_string(), latest.cloned())
            && previous.as_ref() != latest
            && latest.is_some()
        {
            self.new_activity.insert(space.to_string());
        }
    }

    pub fn observe_recency(&mut self, space: &str, timestamp: &str) {
        let timestamp = timestamp.trim_end_matches('Z');
        let (seconds, fraction) = timestamp.split_once('.').unwrap_or((timestamp, ""));
        let timestamp = format!("{seconds}.{fraction:0<9}");
        let previous = self.recency.entry(space.to_string()).or_default();
        if timestamp > *previous {
            *previous = timestamp;
        }
    }

    pub fn sort_spaces_by_recency(&mut self) {
        self.spaces.sort_by(|left, right| {
            self.recency
                .get(&right.id.0)
                .cmp(&self.recency.get(&left.id.0))
        });
    }

    pub fn reconcile_spaces(&mut self, mut spaces: Vec<Space>) {
        for space in &mut spaces {
            if space.display_name.trim().is_empty()
                && let Some(old) = self.spaces.iter().find(|old| old.id == space.id)
            {
                space.display_name.clone_from(&old.display_name);
            }
        }
        self.new_activity
            .retain(|id| spaces.iter().any(|space| &space.id.0 == id));
        self.activity_baselines
            .retain(|id, _| spaces.iter().any(|space| &space.id.0 == id));
        self.recency
            .retain(|id, _| spaces.iter().any(|space| &space.id.0 == id));
        self.spaces = spaces;
        self.sort_spaces_by_recency();
    }

    pub const fn load_spaces(&mut self) -> Effect {
        let request_id = self.allocate_request_id();
        self.active_spaces_request = Some(request_id);
        self.phase = Phase::LoadingSpaces;
        Effect::LoadSpaces { request_id }
    }

    pub fn select_space(&mut self, space_name: String) -> Effect {
        let request_id = self.allocate_request_id();
        self.selected_space = Some(space_name.clone());
        self.messages.clear();
        self.next_message_page = None;
        self.active_messages_request = Some((request_id, space_name.clone()));
        self.phase = Phase::LoadingMessages;
        Effect::LoadMessages {
            request_id,
            space_name,
            page_token: None,
        }
    }

    pub fn load_older_messages(&mut self) -> Option<Effect> {
        let space_name = self.selected_space.clone()?;
        let page_token = self.next_message_page.clone()?;
        let request_id = self.allocate_request_id();
        self.active_messages_request = Some((request_id, space_name.clone()));
        Some(Effect::LoadMessages {
            request_id,
            space_name,
            page_token: Some(page_token),
        })
    }

    pub fn refresh(&mut self) -> Option<Effect> {
        let space_name = self.selected_space.clone()?;
        let request_id = self.allocate_request_id();
        self.active_messages_request = Some((request_id, space_name.clone()));
        self.phase = Phase::Refreshing;
        Some(Effect::LoadMessages {
            request_id,
            space_name,
            page_token: None,
        })
    }

    #[must_use]
    pub fn accepts(&self, message: &ProductMessage) -> bool {
        match message {
            ProductMessage::MessagesLoaded {
                request_id,
                space_name,
                ..
            } => {
                self.active_messages_request.as_ref() == Some(&(*request_id, space_name.clone()))
                    && self.selected_space.as_ref() == Some(space_name)
            }
            ProductMessage::SpacesLoaded { request_id, .. } => {
                self.active_spaces_request == Some(*request_id)
            }
        }
    }

    pub fn poll(&mut self) -> Option<Effect> {
        if self.active_messages_request.is_some()
            || self.active_spaces_request.is_some()
            || !matches!(
                self.phase,
                Phase::Ready | Phase::Empty | Phase::RecoverableError
            )
        {
            return None;
        }
        self.refresh()
    }

    pub fn update(&mut self, message: ProductMessage) {
        match message {
            ProductMessage::SpacesLoaded { request_id, result }
                if self.active_spaces_request == Some(request_id) =>
            {
                self.active_spaces_request = None;
                match result {
                    Ok((spaces, _)) => {
                        self.spaces = spaces;
                        self.phase = if self.spaces.is_empty() {
                            Phase::Empty
                        } else {
                            Phase::Ready
                        };
                    }
                    Err(ChatError::Unauthorized) => self.phase = Phase::Reauthentication,
                    Err(_) => self.phase = Phase::RecoverableError,
                }
            }
            ProductMessage::MessagesLoaded {
                request_id,
                space_name,
                result,
            } if self.active_messages_request.as_ref()
                == Some(&(request_id, space_name.clone()))
                && self.selected_space.as_deref() == Some(space_name.as_str()) =>
            {
                self.active_messages_request = None;
                match result {
                    Ok((messages, next_page)) => {
                        if let Some(latest) = messages.last() {
                            self.observe_recency(&space_name, &latest.create_time);
                            self.sort_spaces_by_recency();
                        }
                        self.new_activity.remove(&space_name);
                        self.merge_messages(messages);
                        self.next_message_page = next_page;
                        self.phase = if self.messages.is_empty() {
                            Phase::Empty
                        } else {
                            Phase::Ready
                        };
                    }
                    Err(ChatError::Unauthorized) => self.phase = Phase::Reauthentication,
                    Err(_) => self.phase = Phase::RecoverableError,
                }
            }
            ProductMessage::SpacesLoaded { .. } | ProductMessage::MessagesLoaded { .. } => {}
        }
    }

    const fn allocate_request_id(&mut self) -> RequestId {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        request_id
    }

    fn merge_messages(&mut self, incoming: Vec<Message>) {
        self.messages
            .retain(|existing| !incoming.iter().any(|message| message.id == existing.id));
        self.messages.extend(incoming);
        self.messages.sort_by(|left, right| {
            left.create_time
                .cmp(&right.create_time)
                .then_with(|| left.id.cmp(&right.id))
        });
        self.messages.dedup_by(|left, right| left.id == right.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessageId, SpaceId, SpaceKind};

    #[test]
    fn recency_orders_newest_first_and_older_pages_do_not_demote() {
        let mut state = ProductState::default();
        state.reconcile_spaces(vec![space("a"), space("b"), space("unknown")]);
        state.observe_recency("a", "2026-01-01T00:00:00Z");
        state.observe_recency("b", "2026-01-01T00:00:00.1Z");
        state.sort_spaces_by_recency();
        assert_eq!(state.spaces[0].id.0, "b");
        state.observe_recency("b", "2025-01-01T00:00:00Z");
        state.reconcile_spaces(vec![space("unknown"), space("a"), space("b")]);
        assert_eq!(state.spaces[0].id.0, "b");
        assert_eq!(state.spaces[2].id.0, "unknown");
    }

    #[test]
    fn activity_is_baselined_then_marked_and_pruned() {
        let mut state = ProductState::default();
        state.reconcile_spaces(vec![space("one"), space("two")]);
        let first = MessageId("first".to_string());
        let second = MessageId("second".to_string());
        state.observe_activity("two", Some(&first));
        assert!(state.new_activity.is_empty());
        state.observe_activity("two", Some(&second));
        assert!(state.new_activity.contains("two"));
        state.reconcile_spaces(vec![space("one")]);
        assert!(state.new_activity.is_empty());
    }

    #[test]
    fn polling_waits_for_load_and_updates_existing_messages() {
        let mut state = ProductState::default();
        let Effect::LoadMessages {
            request_id,
            space_name,
            ..
        } = state.select_space("synthetic-space".to_string())
        else {
            unreachable!()
        };
        assert!(state.poll().is_none());
        state.update(ProductMessage::MessagesLoaded {
            request_id,
            space_name,
            result: Ok((vec![message("one", "1")], None)),
        });
        let Effect::LoadMessages {
            request_id,
            space_name,
            ..
        } = state.poll().unwrap()
        else {
            unreachable!()
        };
        assert!(state.poll().is_none());
        state.update(ProductMessage::MessagesLoaded {
            request_id,
            space_name,
            result: Ok((vec![message("one", "2")], None)),
        });
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].create_time, "2");
        state.phase = Phase::Reauthentication;
        assert!(state.poll().is_none());
    }

    fn space(name: &str) -> Space {
        Space {
            id: SpaceId(name.to_string()),
            display_name: "Example Space".to_string(),
            kind: SpaceKind::Space,
        }
    }

    fn message(name: &str, time: &str) -> Message {
        Message {
            id: MessageId(name.to_string()),
            thread_id: None,
            sender: None,
            text: "Synthetic message".to_string(),
            create_time: time.to_string(),
            is_thread_reply: false,
            unsupported_content: false,
        }
    }

    #[test]
    fn stale_space_response_is_ignored() {
        let mut state = ProductState::default();
        let Effect::LoadSpaces { request_id: old } = state.load_spaces() else {
            unreachable!()
        };
        let Effect::LoadSpaces {
            request_id: current,
        } = state.load_spaces()
        else {
            unreachable!()
        };
        state.update(ProductMessage::SpacesLoaded {
            request_id: old,
            result: Ok((vec![space("spaces/old")], None)),
        });
        assert!(state.spaces.is_empty());
        state.update(ProductMessage::SpacesLoaded {
            request_id: current,
            result: Ok((vec![space("spaces/current")], None)),
        });
        assert_eq!(state.spaces[0].id, SpaceId("spaces/current".to_string()));
    }

    #[test]
    fn switched_space_rejects_old_messages() {
        let mut state = ProductState::default();
        let Effect::LoadMessages {
            request_id: old, ..
        } = state.select_space("spaces/old".to_string())
        else {
            unreachable!()
        };
        let Effect::LoadMessages {
            request_id: current,
            ..
        } = state.select_space("spaces/current".to_string())
        else {
            unreachable!()
        };
        state.update(ProductMessage::MessagesLoaded {
            request_id: old,
            space_name: "spaces/old".to_string(),
            result: Ok((vec![message("messages/old", "2")], None)),
        });
        assert!(state.messages.is_empty());
        state.update(ProductMessage::MessagesLoaded {
            request_id: current,
            space_name: "spaces/current".to_string(),
            result: Ok((vec![message("messages/current", "1")], None)),
        });
        assert_eq!(
            state.messages[0].id,
            MessageId("messages/current".to_string())
        );
    }

    #[test]
    fn older_pages_merge_without_duplicates() {
        let mut state = ProductState::default();
        let Effect::LoadMessages { request_id, .. } =
            state.select_space("spaces/example".to_string())
        else {
            unreachable!()
        };
        state.update(ProductMessage::MessagesLoaded {
            request_id,
            space_name: "spaces/example".to_string(),
            result: Ok((
                vec![message("messages/two", "2")],
                Some(PageToken("older".to_string())),
            )),
        });
        let Effect::LoadMessages { request_id, .. } = state.load_older_messages().unwrap() else {
            unreachable!()
        };
        state.update(ProductMessage::MessagesLoaded {
            request_id,
            space_name: "spaces/example".to_string(),
            result: Ok((
                vec![message("messages/one", "1"), message("messages/two", "2")],
                None,
            )),
        });
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].id, MessageId("messages/one".to_string()));
    }
}
