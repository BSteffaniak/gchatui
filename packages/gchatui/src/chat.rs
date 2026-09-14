use reqwest::{StatusCode, Url};
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::model::{
    Message, MessageId, Page, PageToken, Sender, SenderKind, Space, SpaceId, SpaceKind, ThreadId,
};

const CHAT_API: &str = "https://chat.googleapis.com/v1/";
const MAX_PAGE_SIZE: u16 = 1000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ChatError {
    #[error("Google Chat authorization expired")]
    Unauthorized,
    #[error("Google Chat access was denied")]
    Forbidden,
    #[error("Google Chat resource was not found")]
    NotFound,
    #[error("Google Chat rate limit was reached")]
    RateLimited,
    #[error("Google Chat is temporarily unavailable")]
    Transient,
    #[error("Google Chat response was malformed")]
    Malformed,
    #[error("Google Chat request was cancelled")]
    Cancelled,
    #[error("Google Chat request failed")]
    Transport,
}

pub struct ChatClient {
    http: crate::google_http::GoogleHttp,
    base_url: Url,
    retry: RetryPolicy,
}

#[derive(Debug, Clone, Copy)]
struct RetryPolicy {
    max_attempts: u8,
    base_delay: std::time::Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: std::time::Duration::from_millis(200),
        }
    }
}

impl ChatClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: crate::google_http::GoogleHttp::new(None),
            base_url: Url::parse(CHAT_API).expect("static Chat API URL should be valid"),
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_auth(auth: std::sync::Arc<crate::auth::AuthManager>) -> Self {
        let mut client = Self::new();
        client.http = crate::google_http::GoogleHttp::new(Some(auth));
        client
    }

    #[cfg(test)]
    fn with_base_url(base_url: Url) -> Self {
        Self {
            http: crate::google_http::GoogleHttp::new(None),
            base_url,
            retry: RetryPolicy {
                max_attempts: 1,
                base_delay: std::time::Duration::ZERO,
            },
        }
    }

    #[cfg(test)]
    fn with_retry(base_url: Url, max_attempts: u8) -> Self {
        Self {
            http: crate::google_http::GoogleHttp::new(None),
            base_url,
            retry: RetryPolicy {
                max_attempts,
                base_delay: std::time::Duration::from_millis(1),
            },
        }
    }

    pub async fn infer_direct_message_titles(
        &self,
        access_token: &Zeroizing<String>,
        spaces: &mut [Space],
        people: &crate::people::PeopleClient,
        aliases: Option<&crate::sender_alias::SenderAliases>,
    ) {
        use futures_util::stream::{self, StreamExt};

        let current_user = people.current_user(access_token).await.ok();
        let current_people = current_user
            .as_ref()
            .map(|(resource_names, _)| resource_names)
            .cloned()
            .unwrap_or_default();
        let unresolved = spaces
            .iter()
            .enumerate()
            .filter(|(_, space)| {
                space.kind == SpaceKind::DirectMessage && space.display_name.trim().is_empty()
            })
            .map(|(index, space)| (index, space.id.clone()))
            .collect::<Vec<_>>();
        let resolved = stream::iter(unresolved)
            .map(|(index, space_id)| {
                let current_people = current_people.clone();
                async move {
                    let members = self.list_members(access_token, &space_id).await.ok()?;
                    let mut candidates = members
                        .into_iter()
                        .filter(|member| {
                            chat_user_to_person(&member.resource_name)
                                .is_none_or(|person| !current_people.contains(&person))
                                && !matches!(member.kind, SenderKind::Bot | SenderKind::Anonymous)
                        })
                        .collect::<Vec<_>>();
                    if candidates.len() != 1 {
                        return None;
                    }
                    let _ = people.resolve_senders(access_token, &mut candidates).await;
                    let other = candidates.pop()?;
                    let alias = aliases.and_then(|aliases| aliases.get(&other.resource_name));
                    let name = alias.or(other.display_name).or_else(|| {
                        other
                            .resource_name
                            .strip_prefix("users/")
                            .map(|id| format!("User {id}"))
                    })?;
                    Some((index, name))
                }
            })
            .buffer_unordered(8)
            .filter_map(std::future::ready)
            .collect::<Vec<_>>()
            .await;
        for (index, name) in resolved {
            if let Some(space) = spaces.get_mut(index) {
                space.display_name = name;
            }
        }
    }

    async fn list_members(
        &self,
        access_token: &Zeroizing<String>,
        space: &SpaceId,
    ) -> Result<Vec<Sender>, ChatError> {
        let path = format!("{}/members", space.0.trim_start_matches('/'));
        let mut url = self
            .base_url
            .join(&path)
            .map_err(|_| ChatError::Malformed)?;
        url.query_pairs_mut().append_pair("pageSize", "1000");
        let response = self.get_with_retry(url, access_token).await?;
        let response = ensure_success(response)?;
        let payload: ListMembershipsResponse =
            response.json().await.map_err(|_| ChatError::Malformed)?;
        Ok(payload
            .memberships
            .into_iter()
            .filter_map(|membership| membership.member.map(Into::into))
            .collect())
    }

    pub async fn list_spaces_cancelable(
        &self,
        access_token: &Zeroizing<String>,
        page_size: u16,
        page_token: Option<&PageToken>,
        cancelled: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<Page<Space>, ChatError> {
        tokio::select! {
            biased;
            changed = cancelled.changed() => {
                let _ = changed;
                Err(ChatError::Cancelled)
            }
            result = self.list_spaces(access_token, page_size, page_token) => result,
        }
    }

    pub async fn list_spaces(
        &self,
        access_token: &Zeroizing<String>,
        page_size: u16,
        page_token: Option<&PageToken>,
    ) -> Result<Page<Space>, ChatError> {
        let mut url = self
            .base_url
            .join("spaces")
            .map_err(|_| ChatError::Malformed)?;
        url.query_pairs_mut()
            .append_pair("pageSize", &page_size.clamp(1, MAX_PAGE_SIZE).to_string());
        if let Some(token) = page_token {
            url.query_pairs_mut().append_pair("pageToken", &token.0);
        }
        let response = self.get_with_retry(url, access_token).await?;
        let response = ensure_success(response)?;
        let payload: ListSpacesResponse =
            response.json().await.map_err(|_| ChatError::Malformed)?;
        Ok(Page {
            items: payload.spaces.into_iter().map(Into::into).collect(),
            next_page_token: payload.next_page_token.map(PageToken),
        })
    }

    pub async fn list_all_spaces(
        &self,
        access_token: &Zeroizing<String>,
    ) -> Result<Vec<Space>, ChatError> {
        let mut spaces = Vec::new();
        let mut next = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let page = self.list_spaces(access_token, 1000, next.as_ref()).await?;
            spaces.extend(page.items);
            let Some(token) = page.next_page_token else {
                break;
            };
            if !seen.insert(token.0.clone()) {
                return Err(ChatError::Malformed);
            }
            next = Some(token);
        }
        let mut ids = std::collections::BTreeSet::new();
        spaces.retain(|space| ids.insert(space.id.clone()));
        Ok(spaces)
    }

    pub async fn list_messages(
        &self,
        access_token: &Zeroizing<String>,
        space: &SpaceId,
        page_size: u16,
        page_token: Option<&PageToken>,
    ) -> Result<Page<Message>, ChatError> {
        let path = format!("{}/messages", space.0.trim_start_matches('/'));
        let mut url = self
            .base_url
            .join(&path)
            .map_err(|_| ChatError::Malformed)?;
        url.query_pairs_mut()
            .append_pair("pageSize", &page_size.clamp(1, MAX_PAGE_SIZE).to_string())
            .append_pair("orderBy", "createTime DESC");
        if let Some(token) = page_token {
            url.query_pairs_mut().append_pair("pageToken", &token.0);
        }
        let response = self.get_with_retry(url, access_token).await?;
        let response = ensure_success(response)?;
        let payload: ListMessagesResponse =
            response.json().await.map_err(|_| ChatError::Malformed)?;
        let mut messages = payload
            .messages
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();
        messages.sort_by(|left: &Message, right: &Message| {
            left.create_time
                .cmp(&right.create_time)
                .then_with(|| left.id.cmp(&right.id))
        });
        messages.dedup_by(|left, right| left.id == right.id);
        Ok(Page {
            items: messages,
            next_page_token: payload.next_page_token.map(PageToken),
        })
    }

    async fn get_with_retry(
        &self,
        url: Url,
        access_token: &Zeroizing<String>,
    ) -> Result<reqwest::Response, ChatError> {
        let attempts = self.retry.max_attempts.max(1);
        for attempt in 0..attempts {
            match self.http.get(url.clone(), access_token).await {
                Ok(response)
                    if response.status().is_server_error()
                        || response.status() == StatusCode::TOO_MANY_REQUESTS =>
                {
                    if attempt.saturating_add(1) == attempts {
                        return Ok(response);
                    }
                    let delay = retry_delay(self.retry.base_delay, attempt, response.headers());
                    tokio::time::sleep(delay).await;
                }
                Ok(response) => return Ok(response),
                Err(error) if error.is_timeout() || error.is_connect() => {
                    if attempt.saturating_add(1) == attempts {
                        return Err(ChatError::Transport);
                    }
                    tokio::time::sleep(retry_delay(
                        self.retry.base_delay,
                        attempt,
                        &reqwest::header::HeaderMap::new(),
                    ))
                    .await;
                }
                Err(crate::google_http::RequestError::Authorization) => {
                    return Err(ChatError::Unauthorized);
                }
                Err(_) => return Err(ChatError::Transport),
            }
        }
        Err(ChatError::Transport)
    }
}

fn retry_delay(
    base: std::time::Duration,
    attempt: u8,
    headers: &reqwest::header::HeaderMap,
) -> std::time::Duration {
    if let Some(seconds) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        return std::time::Duration::from_secs(seconds.min(30));
    }
    let multiplier = 1_u32 << u32::from(attempt.min(6));
    let jitter = std::time::Duration::from_millis(u64::from(attempt) * 17);
    base.saturating_mul(multiplier).saturating_add(jitter)
}

impl Default for ChatClient {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response, ChatError> {
    match response.status() {
        status if status.is_success() => Ok(response),
        StatusCode::UNAUTHORIZED => Err(ChatError::Unauthorized),
        StatusCode::FORBIDDEN => Err(ChatError::Forbidden),
        StatusCode::NOT_FOUND => Err(ChatError::NotFound),
        StatusCode::TOO_MANY_REQUESTS => Err(ChatError::RateLimited),
        status if status.is_server_error() => Err(ChatError::Transient),
        _ => Err(ChatError::Transport),
    }
}

fn chat_user_to_person(resource_name: &str) -> Option<String> {
    resource_name
        .strip_prefix("users/")
        .filter(|id| !id.is_empty() && *id != "app")
        .map(|id| format!("people/{id}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMembershipsResponse {
    #[serde(default)]
    memberships: Vec<MembershipDto>,
}

#[derive(Deserialize)]
struct MembershipDto {
    member: Option<UserDto>,
}

impl From<UserDto> for Sender {
    fn from(user: UserDto) -> Self {
        Self {
            resource_name: user.name,
            display_name: (!user.display_name.is_empty()).then_some(user.display_name),
            kind: if user.is_anonymous {
                SenderKind::Anonymous
            } else {
                match user.r#type.as_str() {
                    "HUMAN" => SenderKind::Human,
                    "BOT" => SenderKind::Bot,
                    _ => SenderKind::Unknown,
                }
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSpacesResponse {
    #[serde(default)]
    spaces: Vec<SpaceDto>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpaceDto {
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    space_type: String,
}

impl From<SpaceDto> for Space {
    fn from(value: SpaceDto) -> Self {
        let kind = match value.space_type.as_str() {
            "SPACE" => SpaceKind::Space,
            "GROUP_CHAT" => SpaceKind::GroupChat,
            "DIRECT_MESSAGE" => SpaceKind::DirectMessage,
            _ => SpaceKind::Unknown,
        };
        Self {
            id: SpaceId(value.name),
            display_name: value.display_name,
            kind,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMessagesResponse {
    #[serde(default)]
    messages: Vec<MessageDto>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageDto {
    name: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    create_time: String,
    thread: Option<ThreadDto>,
    sender: Option<UserDto>,
    #[serde(default)]
    thread_reply: bool,
    #[serde(default)]
    cards_v2: Vec<serde_json::Value>,
    #[serde(default)]
    attachment: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct ThreadDto {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserDto {
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    is_anonymous: bool,
}

impl From<MessageDto> for Message {
    fn from(value: MessageDto) -> Self {
        Self {
            id: MessageId(value.name),
            thread_id: value.thread.map(|thread| ThreadId(thread.name)),
            sender: value.sender.map(Into::into),
            text: value.text,
            create_time: value.create_time,
            is_thread_reply: value.thread_reply,
            unsupported_content: !value.cards_v2.is_empty() || !value.attachment.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn server(status: &str, body: &str) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let count = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer synthetic-access")
            );
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        Url::parse(&format!("http://{address}/")).unwrap()
    }

    async fn sequence_server(responses: Vec<(&'static str, &'static str)>) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 4096];
                let _ = stream.read(&mut request).await.unwrap();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Url::parse(&format!("http://{address}/")).unwrap()
    }

    #[tokio::test]
    async fn cancellation_wins_before_request_admission() {
        let client = ChatClient::with_base_url(Url::parse("http://127.0.0.1:1/").unwrap());
        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
        cancel_tx.send(true).unwrap();
        let error = client
            .list_spaces_cancelable(
                &Zeroizing::new("synthetic-access".to_string()),
                50,
                None,
                &mut cancel_rx,
            )
            .await
            .unwrap_err();
        assert_eq!(error, ChatError::Cancelled);
    }

    #[tokio::test]
    async fn transient_failure_is_retried_but_permanent_failure_is_not() {
        let base = sequence_server(vec![
            ("503 Service Unavailable", r#"{"error":"temporary"}"#),
            ("200 OK", r#"{"spaces":[]}"#),
        ])
        .await;
        let page = ChatClient::with_retry(base, 2)
            .list_spaces(&Zeroizing::new("synthetic-access".to_string()), 50, None)
            .await
            .unwrap();
        assert!(page.items.is_empty());

        let base = sequence_server(vec![("401 Unauthorized", r#"{"error":"expired"}"#)]).await;
        let error = ChatClient::with_retry(base, 3)
            .list_spaces(&Zeroizing::new("synthetic-access".to_string()), 50, None)
            .await
            .unwrap_err();
        assert_eq!(error, ChatError::Unauthorized);
    }

    #[tokio::test]
    async fn malformed_payload_is_rejected() {
        let base = server("200 OK", "not-json").await;
        let error = ChatClient::with_base_url(base)
            .list_spaces(&Zeroizing::new("synthetic-access".to_string()), 50, None)
            .await
            .unwrap_err();
        assert_eq!(error, ChatError::Malformed);
    }

    #[tokio::test]
    async fn blank_dm_title_is_inferred_from_other_membership_before_publish() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let body = r#"{"memberships":[{"member":{"name":"users/me","displayName":"Me","type":"HUMAN"}},{"member":{"name":"users/other","displayName":"Other Person","type":"HUMAN"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let chat = ChatClient::with_base_url(Url::parse(&format!("http://{address}/")).unwrap());
        let people = crate::people::PeopleClient::with_test_current_user("people/me");
        let mut spaces = vec![Space {
            id: SpaceId("spaces/dm".to_string()),
            display_name: String::new(),
            kind: SpaceKind::DirectMessage,
        }];
        chat.infer_direct_message_titles(
            &Zeroizing::new("synthetic-access".to_string()),
            &mut spaces,
            &people,
            None,
        )
        .await;
        assert_eq!(spaces[0].display_name, "Other Person");
    }

    #[tokio::test]
    async fn spaces_are_typed_and_paginated() {
        let base = server(
            "200 OK",
            r#"{"spaces":[{"name":"spaces/example","displayName":"Example Space","spaceType":"SPACE"}],"nextPageToken":"next-example"}"#,
        )
        .await;
        let client = ChatClient::with_base_url(base);
        let page = client
            .list_spaces(&Zeroizing::new("synthetic-access".to_string()), 50, None)
            .await
            .unwrap();
        assert_eq!(page.items[0].id, SpaceId("spaces/example".to_string()));
        assert_eq!(
            page.next_page_token,
            Some(PageToken("next-example".to_string()))
        );
    }

    #[tokio::test]
    async fn messages_are_ordered_deduplicated_and_mark_unsupported_content() {
        let base = server(
            "200 OK",
            r#"{"messages":[{"name":"spaces/example/messages/two","text":"Second","createTime":"2026-01-01T00:00:02Z","thread":{"name":"spaces/example/threads/thread"},"cardsV2":[{}]},{"name":"spaces/example/messages/one","text":"First","createTime":"2026-01-01T00:00:01Z"},{"name":"spaces/example/messages/one","text":"First","createTime":"2026-01-01T00:00:01Z"}]}"#,
        )
        .await;
        let client = ChatClient::with_base_url(base);
        let page = client
            .list_messages(
                &Zeroizing::new("synthetic-access".to_string()),
                &SpaceId("spaces/example".to_string()),
                50,
                None,
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].text, "First");
        assert!(page.items[1].unsupported_content);
        assert!(page.items[1].thread_id.is_some());
    }

    #[tokio::test]
    async fn response_body_is_not_exposed_by_errors() {
        let base = server("403 Forbidden", r#"{"private":"synthetic-private-body"}"#).await;
        let error = ChatClient::with_base_url(base)
            .list_spaces(&Zeroizing::new("synthetic-access".to_string()), 50, None)
            .await
            .unwrap_err();
        assert_eq!(error, ChatError::Forbidden);
        assert!(!error.to_string().contains("synthetic-private-body"));
    }
}
