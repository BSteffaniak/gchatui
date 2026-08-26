use std::collections::{BTreeMap, BTreeSet};

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

pub struct PeopleClient {
    http: reqwest::Client,
    base_url: Url,
}

impl PeopleClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("static HTTP configuration should be valid"),
            base_url: Url::parse(PEOPLE_API).expect("static People API URL should be valid"),
        }
    }

    #[cfg(test)]
    fn with_base_url(base_url: Url) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    pub async fn resolve_message_senders(
        &self,
        access_token: &Zeroizing<String>,
        messages: &mut [Message],
    ) -> Result<(), PeopleError> {
        let resource_names = messages
            .iter()
            .filter_map(|message| message.sender.as_ref())
            .filter(|sender| sender.display_name.is_none() && sender.kind == SenderKind::Human)
            .filter_map(|sender| chat_user_to_person(&sender.resource_name))
            .collect::<BTreeSet<_>>();
        if resource_names.is_empty() {
            return Ok(());
        }

        let mut resolved = BTreeMap::new();
        for batch in resource_names
            .into_iter()
            .collect::<Vec<_>>()
            .chunks(MAX_BATCH_SIZE)
        {
            resolved.extend(self.resolve_batch(access_token, batch).await?);
        }
        for message in messages {
            if let Some(sender) = message.sender.as_mut()
                && sender.display_name.is_none()
                && let Some(person_name) = chat_user_to_person(&sender.resource_name)
                && let Some(display_name) = resolved.get(&person_name)
            {
                sender.display_name = Some(display_name.clone());
            }
        }
        Ok(())
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
            query.append_pair("sources", "READ_SOURCE_TYPE_DIRECTORY");
            for resource_name in resource_names {
                query.append_pair("resourceNames", resource_name);
            }
        }
        let response = self
            .http
            .get(url)
            .bearer_auth(access_token.as_str())
            .send()
            .await
            .map_err(|_| PeopleError::Transport)?;
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
                let person = response.person?;
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
                (!display_name.is_empty()).then_some((person.resource_name, display_name))
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
struct BatchResponse {
    #[serde(default)]
    responses: Vec<PersonResponse>,
}

#[derive(Deserialize)]
struct PersonResponse {
    person: Option<PersonDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersonDto {
    resource_name: String,
    #[serde(default)]
    names: Vec<NameDto>,
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
            let body = r#"{"responses":[{"person":{"resourceName":"people/123","names":[{"displayName":"Example Person","metadata":{"primary":true}}]}}]}"#;
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
