use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use reqwest::{StatusCode, Url};
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::model::{Message, SenderKind};

const PEOPLE_API: &str = "https://people.googleapis.com/v1/";
const MAX_BATCH_SIZE: usize = 200;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PeopleError {
    #[error("Google People authorization expired")]
    Unauthorized,
    #[error("Google People directory access was denied")]
    Forbidden,
    #[error("Google People response was malformed")]
    Malformed,
    #[error("Google People request failed")]
    Transport,
}

impl From<crate::google_http::RequestError> for PeopleError {
    fn from(error: crate::google_http::RequestError) -> Self {
        match error {
            crate::google_http::RequestError::Authorization => Self::Unauthorized,
            crate::google_http::RequestError::Transport(_) => Self::Transport,
        }
    }
}

pub struct PeopleClient {
    http: crate::google_http::GoogleHttp,
    base_url: Url,
    directory: tokio::sync::Mutex<Option<Arc<BTreeMap<String, String>>>>,
    current_user: tokio::sync::Mutex<Option<Arc<CurrentUser>>>,
    contacts_cache: tokio::sync::Mutex<Option<BTreeMap<String, String>>>,
    contacts_enabled: bool,
}

#[derive(Debug)]
struct CurrentUser {
    resource_names: BTreeSet<String>,
    display_name: Option<String>,
}

impl PeopleClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: crate::google_http::GoogleHttp::new(None),
            base_url: Url::parse(PEOPLE_API).expect("static People API URL should be valid"),
            directory: tokio::sync::Mutex::new(None),
            current_user: tokio::sync::Mutex::new(None),
            contacts_cache: tokio::sync::Mutex::new(None),
            contacts_enabled: true,
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
            directory: tokio::sync::Mutex::new(Some(Arc::new(BTreeMap::new()))),
            current_user: tokio::sync::Mutex::new(None),
            contacts_cache: tokio::sync::Mutex::new(None),
            contacts_enabled: false,
        }
    }

    #[cfg(test)]
    fn with_directory_base_url(base_url: Url) -> Self {
        Self {
            http: crate::google_http::GoogleHttp::new(None),
            base_url,
            directory: tokio::sync::Mutex::new(None),
            current_user: tokio::sync::Mutex::new(None),
            contacts_cache: tokio::sync::Mutex::new(None),
            contacts_enabled: false,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_test_current_user(resource_name: &str) -> Self {
        Self {
            http: crate::google_http::GoogleHttp::new(None),
            base_url: Url::parse(PEOPLE_API).expect("static People API URL should be valid"),
            directory: tokio::sync::Mutex::new(Some(Arc::new(BTreeMap::new()))),
            current_user: tokio::sync::Mutex::new(Some(Arc::new(CurrentUser {
                resource_names: BTreeSet::from([resource_name.to_string()]),
                display_name: Some("Me".to_string()),
            }))),
            contacts_cache: tokio::sync::Mutex::new(None),
            contacts_enabled: false,
        }
    }

    pub async fn current_user(
        &self,
        access_token: &Zeroizing<String>,
    ) -> Result<(BTreeSet<String>, Option<String>), PeopleError> {
        let mut cache = self.current_user.lock().await;
        if let Some(user) = cache.as_ref() {
            return Ok((user.resource_names.clone(), user.display_name.clone()));
        }
        let (mut resource_names, mut display_name) = self.userinfo_identity(access_token).await?;
        if let Ok((people_names, people_display_name)) = self.current_person(access_token).await {
            resource_names.extend(people_names);
            if display_name.is_none() {
                display_name = people_display_name;
            }
        }
        *cache = Some(Arc::new(CurrentUser {
            resource_names: resource_names.clone(),
            display_name: display_name.clone(),
        }));
        drop(cache);
        Ok((resource_names, display_name))
    }

    async fn userinfo_identity(
        &self,
        access_token: &Zeroizing<String>,
    ) -> Result<(BTreeSet<String>, Option<String>), PeopleError> {
        #[derive(Deserialize)]
        struct UserInfo {
            sub: String,
            name: Option<String>,
        }
        let response = self
            .http
            .get(
                Url::parse("https://openidconnect.googleapis.com/v1/userinfo")
                    .expect("static userinfo URL"),
                access_token,
            )
            .await
            .map_err(PeopleError::from)?;
        match response.status() {
            status if status.is_success() => {}
            StatusCode::UNAUTHORIZED => return Err(PeopleError::Unauthorized),
            StatusCode::FORBIDDEN => return Err(PeopleError::Forbidden),
            _ => return Err(PeopleError::Transport),
        }
        let info: UserInfo = response.json().await.map_err(|_| PeopleError::Malformed)?;
        Ok((BTreeSet::from([format!("people/{}", info.sub)]), info.name))
    }

    async fn current_person(
        &self,
        access_token: &Zeroizing<String>,
    ) -> Result<(BTreeSet<String>, Option<String>), PeopleError> {
        let mut url = Url::parse(&format!("{}people/me", self.base_url.as_str()))
            .map_err(|_| PeopleError::Malformed)?;
        url.query_pairs_mut()
            .append_pair("personFields", "names,metadata");
        let response = self
            .http
            .get(url, access_token)
            .await
            .map_err(PeopleError::from)?;
        match response.status() {
            status if status.is_success() => {}
            StatusCode::UNAUTHORIZED => return Err(PeopleError::Unauthorized),
            StatusCode::FORBIDDEN => return Err(PeopleError::Forbidden),
            _ => return Err(PeopleError::Transport),
        }
        let person: PersonDto = response.json().await.map_err(|_| PeopleError::Malformed)?;
        let display_name = primary_display_name(&person.names);
        let mut resource_names = BTreeSet::from([person.resource_name]);
        for source in person.metadata.sources {
            if !source.id.is_empty() {
                resource_names.insert(format!("people/{}", source.id));
            }
        }
        Ok((resource_names, display_name))
    }

    pub async fn resolve_message_senders(
        &self,
        access_token: &Zeroizing<String>,
        messages: &mut [Message],
    ) -> Result<(), PeopleError> {
        let mut senders = messages
            .iter()
            .filter_map(|message| message.sender.clone())
            .collect::<Vec<_>>();
        self.resolve_senders(access_token, &mut senders).await?;
        let resolved = senders
            .into_iter()
            .map(|sender| (sender.resource_name.clone(), sender))
            .collect::<BTreeMap<_, _>>();
        for message in messages {
            if let Some(sender) = message.sender.as_mut()
                && let Some(value) = resolved.get(&sender.resource_name)
            {
                sender.display_name.clone_from(&value.display_name);
                sender.kind = value.kind;
            }
        }
        Ok(())
    }

    pub async fn resolve_senders(
        &self,
        access_token: &Zeroizing<String>,
        senders: &mut [crate::model::Sender],
    ) -> Result<(), PeopleError> {
        let resource_names = senders
            .iter()
            .filter(|sender| {
                sender.display_name.is_none()
                    && matches!(sender.kind, SenderKind::Human | SenderKind::Unknown)
            })
            .filter_map(|sender| chat_user_to_person(&sender.resource_name))
            .collect::<BTreeSet<_>>();
        if resource_names.is_empty() {
            return Ok(());
        }

        let mut identity_index = match self.directory_index(access_token).await {
            Ok(directory) => (*directory).clone(),
            Err(PeopleError::Unauthorized | PeopleError::Forbidden | PeopleError::Transport) => {
                BTreeMap::new()
            }
            Err(PeopleError::Malformed) => return Err(PeopleError::Malformed),
        };
        if self.contacts_enabled {
            match self.contacts_index(access_token).await {
                Ok(contacts) => identity_index.extend(contacts),
                Err(
                    PeopleError::Unauthorized | PeopleError::Forbidden | PeopleError::Transport,
                ) => {}
                Err(PeopleError::Malformed) => return Err(PeopleError::Malformed),
            }
        }
        let directory = Arc::new(identity_index);
        let mut unresolved = BTreeSet::new();
        for sender in senders.iter_mut() {
            if sender.display_name.is_none()
                && let Some(person_name) = chat_user_to_person(&sender.resource_name)
            {
                if let Some(display_name) = directory.get(&person_name) {
                    sender.display_name = Some(display_name.clone());
                } else {
                    unresolved.insert(person_name);
                }
            }
        }

        let mut resolved = BTreeMap::new();
        for batch in unresolved
            .into_iter()
            .collect::<Vec<_>>()
            .chunks(MAX_BATCH_SIZE)
        {
            resolved.extend(self.resolve_batch(access_token, batch).await?);
        }
        for sender in senders {
            if sender.display_name.is_none()
                && let Some(person_name) = chat_user_to_person(&sender.resource_name)
                && let Some(display_name) = resolved.get(&person_name)
            {
                sender.display_name = Some(display_name.clone());
            }
        }
        Ok(())
    }

    async fn contacts_index(
        &self,
        access_token: &Zeroizing<String>,
    ) -> Result<BTreeMap<String, String>, PeopleError> {
        let mut cache = self.contacts_cache.lock().await;
        if let Some(index) = cache.as_ref() {
            return Ok(index.clone());
        }
        let mut index = BTreeMap::new();
        let mut page_token = None;
        loop {
            let mut url = Url::parse(&format!("{}people/me/connections", self.base_url.as_str()))
                .map_err(|_| PeopleError::Malformed)?;
            {
                let mut query = url.query_pairs_mut();
                query.append_pair("personFields", "names,metadata");
                query.append_pair("pageSize", "1000");
                if let Some(token) = page_token.as_deref() {
                    query.append_pair("pageToken", token);
                }
            }
            let response = self
                .http
                .get(url, access_token)
                .await
                .map_err(PeopleError::from)?;
            match response.status() {
                status if status.is_success() => {}
                StatusCode::UNAUTHORIZED => return Err(PeopleError::Unauthorized),
                StatusCode::FORBIDDEN => return Err(PeopleError::Forbidden),
                _ => return Err(PeopleError::Transport),
            }
            let payload: ConnectionsResponse =
                response.json().await.map_err(|_| PeopleError::Malformed)?;
            for person in payload.connections {
                if let Some(display_name) = primary_display_name(&person.names) {
                    index_person(&mut index, &person, &display_name);
                }
            }
            page_token = payload.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
        *cache = Some(index.clone());
        drop(cache);
        Ok(index)
    }

    async fn directory_index(
        &self,
        access_token: &Zeroizing<String>,
    ) -> Result<Arc<BTreeMap<String, String>>, PeopleError> {
        let mut cache = self.directory.lock().await;
        if let Some(index) = cache.as_ref() {
            return Ok(Arc::clone(index));
        }
        let mut index = BTreeMap::new();
        let mut page_token = None;
        loop {
            let mut url = Url::parse(&format!(
                "{}people:listDirectoryPeople",
                self.base_url.as_str()
            ))
            .map_err(|_| PeopleError::Malformed)?;
            {
                let mut query = url.query_pairs_mut();
                query.append_pair("readMask", "names,metadata");
                query.append_pair("sources", "DIRECTORY_SOURCE_TYPE_DOMAIN_PROFILE");
                query.append_pair("sources", "DIRECTORY_SOURCE_TYPE_DOMAIN_CONTACT");
                query.append_pair("pageSize", "1000");
                if let Some(token) = page_token.as_deref() {
                    query.append_pair("pageToken", token);
                }
            }
            let response = self
                .http
                .get(url, access_token)
                .await
                .map_err(PeopleError::from)?;
            match response.status() {
                status if status.is_success() => {}
                StatusCode::UNAUTHORIZED => return Err(PeopleError::Unauthorized),
                StatusCode::FORBIDDEN => return Err(PeopleError::Forbidden),
                _ => return Err(PeopleError::Transport),
            }
            let payload: DirectoryResponse =
                response.json().await.map_err(|_| PeopleError::Malformed)?;
            for person in payload.people {
                if let Some(display_name) = primary_display_name(&person.names) {
                    index_person(&mut index, &person, &display_name);
                }
            }
            page_token = payload.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
        let index = Arc::new(index);
        *cache = Some(Arc::clone(&index));
        drop(cache);
        Ok(index)
    }

    async fn resolve_batch(
        &self,
        access_token: &Zeroizing<String>,
        resource_names: &[String],
    ) -> Result<BTreeMap<String, String>, PeopleError> {
        let mut url = Url::parse(&format!("{}people:batchGet", self.base_url.as_str()))
            .map_err(|_| PeopleError::Malformed)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("personFields", "names");
            for resource_name in resource_names {
                query.append_pair("resourceNames", resource_name);
            }
        }
        let response = self
            .http
            .get(url, access_token)
            .await
            .map_err(PeopleError::from)?;
        match response.status() {
            status if status.is_success() => {}
            StatusCode::UNAUTHORIZED => return Err(PeopleError::Unauthorized),
            StatusCode::FORBIDDEN => return Err(PeopleError::Forbidden),
            _ => return Err(PeopleError::Transport),
        }
        let payload: BatchResponse = response.json().await.map_err(|_| PeopleError::Malformed)?;
        Ok(payload
            .responses
            .into_iter()
            .filter_map(|response| {
                let requested_resource_name = response.requested_resource_name;
                let person = response.person?;
                let resolved_resource_name = requested_resource_name
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| person.resource_name.clone());
                let mut names = person.names.into_iter();
                let first = names.next()?;
                let display_name = if first
                    .metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.primary)
                {
                    first.display_name
                } else {
                    names
                        .find(|name| {
                            name.metadata
                                .as_ref()
                                .is_some_and(|metadata| metadata.primary)
                        })
                        .map_or(first.display_name, |name| name.display_name)
                };
                (!display_name.is_empty()).then_some((resolved_resource_name, display_name))
            })
            .collect())
    }
}

impl Default for PeopleClient {
    fn default() -> Self {
        Self::new()
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
struct ConnectionsResponse {
    #[serde(default)]
    connections: Vec<PersonDto>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryResponse {
    #[serde(default)]
    people: Vec<PersonDto>,
    next_page_token: Option<String>,
}

fn primary_display_name(names: &[NameDto]) -> Option<String> {
    names
        .iter()
        .find(|name| {
            name.metadata
                .as_ref()
                .is_some_and(|metadata| metadata.primary)
        })
        .or_else(|| names.first())
        .map(|name| name.display_name.clone())
        .filter(|name| !name.is_empty())
}

fn index_person(index: &mut BTreeMap<String, String>, person: &PersonDto, display_name: &str) {
    index.insert(person.resource_name.clone(), display_name.to_string());
    for source in &person.metadata.sources {
        if !source.id.is_empty() {
            index.insert(format!("people/{}", source.id), display_name.to_string());
        }
    }
}

#[derive(Deserialize)]
struct BatchResponse {
    #[serde(default)]
    responses: Vec<PersonResponse>,
}

#[derive(Deserialize)]
struct PersonResponse {
    #[serde(rename = "requestedResourceName")]
    requested_resource_name: Option<String>,
    person: Option<PersonDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersonDto {
    resource_name: String,
    #[serde(default)]
    names: Vec<NameDto>,
    #[serde(default)]
    metadata: PersonMetadata,
}

#[derive(Default, Deserialize)]
struct PersonMetadata {
    #[serde(default)]
    sources: Vec<PersonSource>,
}

#[derive(Deserialize)]
struct PersonSource {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameDto {
    #[serde(default)]
    display_name: String,
    metadata: Option<FieldMetadata>,
}

#[derive(Deserialize)]
struct FieldMetadata {
    #[serde(default)]
    primary: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessageId, Sender, ThreadId};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn resolves_directory_source_id_to_chat_user_id() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let count = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
            assert!(request.contains("people:listdirectorypeople"));
            assert!(request.contains("directory_source_type_domain_profile"));
            let body = r#"{"people":[{"resourceName":"people/canonical","names":[{"displayName":"Directory Person","metadata":{"primary":true}}],"metadata":{"sources":[{"type":"DOMAIN_PROFILE","id":"789"}]}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let mut messages = vec![Message {
            id: MessageId("messages/example".to_string()),
            thread_id: None,
            sender: Some(Sender {
                resource_name: "users/789".to_string(),
                display_name: None,
                kind: SenderKind::Human,
            }),
            text: "Synthetic message".to_string(),
            create_time: "2026-01-01T00:00:00Z".to_string(),
            is_thread_reply: false,
            unsupported_content: false,
        }];
        PeopleClient::with_directory_base_url(Url::parse(&format!("http://{address}/")).unwrap())
            .resolve_message_senders(&Zeroizing::new("test".to_string()), &mut messages)
            .await
            .unwrap();
        assert_eq!(
            messages[0].sender.as_ref().unwrap().display_name.as_deref(),
            Some("Directory Person")
        );
    }

    #[tokio::test]
    async fn resolves_sender_when_chat_omits_user_type() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let body = r#"{"responses":[{"requestedResourceName":"people/456","person":{"resourceName":"people/456","names":[{"displayName":"Example Unknown Type"}]}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let mut messages = vec![Message {
            id: MessageId("messages/example".to_string()),
            thread_id: None,
            sender: Some(Sender {
                resource_name: "users/456".to_string(),
                display_name: None,
                kind: SenderKind::Unknown,
            }),
            text: "Synthetic message".to_string(),
            create_time: "2026-01-01T00:00:00Z".to_string(),
            is_thread_reply: false,
            unsupported_content: false,
        }];
        PeopleClient::with_base_url(Url::parse(&format!("http://{address}/")).unwrap())
            .resolve_message_senders(&Zeroizing::new("test".to_string()), &mut messages)
            .await
            .unwrap();
        assert_eq!(
            messages[0].sender.as_ref().unwrap().display_name.as_deref(),
            Some("Example Unknown Type")
        );
    }

    #[tokio::test]
    async fn resolves_chat_user_ids_to_people_display_names() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let count = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            let request_lower = request.to_ascii_lowercase();
            assert!(request_lower.contains("resourcenames=people%2f123"));
            assert!(request_lower.contains("personfields=names"));
            assert!(!request_lower.contains("read_source_type_directory"));
            let body = r#"{"responses":[{"requestedResourceName":"people/123","person":{"resourceName":"people/canonical-profile","names":[{"displayName":"Example Person","metadata":{"primary":true}}]}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let mut messages = vec![Message {
            id: MessageId("messages/example".to_string()),
            thread_id: Some(ThreadId("threads/example".to_string())),
            sender: Some(Sender {
                resource_name: "users/123".to_string(),
                display_name: None,
                kind: SenderKind::Human,
            }),
            text: "Synthetic message".to_string(),
            create_time: "2026-01-01T00:00:00Z".to_string(),
            is_thread_reply: false,
            unsupported_content: false,
        }];
        PeopleClient::with_base_url(Url::parse(&format!("http://{address}/")).unwrap())
            .resolve_message_senders(&Zeroizing::new("test".to_string()), &mut messages)
            .await
            .unwrap();
        assert_eq!(
            messages[0].sender.as_ref().unwrap().display_name.as_deref(),
            Some("Example Person")
        );
    }

    #[test]
    fn translates_stable_chat_resource_name() {
        assert_eq!(
            chat_user_to_person("users/123").as_deref(),
            Some("people/123")
        );
        assert!(chat_user_to_person("users/app").is_none());
    }
}
