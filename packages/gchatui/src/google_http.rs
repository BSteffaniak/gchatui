//! Shared authorization boundary for read-only Google API requests.
use std::sync::Arc;

use crate::{auth::AuthManager, credential::Secret};

pub struct GoogleHttp {
    http: reqwest::Client,
    auth: Option<Arc<AuthManager>>,
}

#[derive(Debug)]
pub enum RequestError {
    Authorization,
    Transport(reqwest::Error),
}

impl RequestError {
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Transport(error) if error.is_timeout())
    }
    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Transport(error) if error.is_connect())
    }
}

impl GoogleHttp {
    pub fn new(auth: Option<Arc<AuthManager>>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("static HTTP configuration"),
            auth,
        }
    }

    pub async fn get(
        &self,
        url: reqwest::Url,
        fallback: &Secret,
    ) -> Result<reqwest::Response, RequestError> {
        // Production clients always have a manager. Unauthenticated fixture clients
        // retain explicit synthetic tokens for local adapter tests.
        let token = match &self.auth {
            Some(auth) => match auth.access_token().await {
                Ok(token) => token,
                Err(_) => return Err(RequestError::Authorization),
            },
            None => zeroize::Zeroizing::new(fallback.to_string()),
        };
        let response = self
            .http
            .get(url.clone())
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(RequestError::Transport)?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(auth) = &self.auth else {
            return Ok(response);
        };
        drop(response);
        let replacement = auth
            .refresh_rejected_token(&token)
            .await
            .map_err(|_| RequestError::Authorization)?;
        // Exactly one replay, and only GET. Never recurse through this method.
        self.http
            .get(url)
            .bearer_auth(replacement.as_str())
            .send()
            .await
            .map_err(RequestError::Transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::{CredentialStore, SessionCredentialStore};
    use crate::oauth::{InstalledClient, OAuthTokens};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zeroize::Zeroizing;

    #[tokio::test]
    async fn unauthorized_replays_once_but_forbidden_never_refreshes() {
        for initial_status in [401, 403] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let count = if initial_status == 401 { 3 } else { 1 };
                for index in 0..count {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut buffer = [0; 8192];
                    let size = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    let (status, body) = if index == 1 {
                        assert!(request.starts_with("POST /token"));
                        (200, r#"{"access_token":"new","expires_in":3600}"#)
                    } else {
                        assert!(request.starts_with("GET /api"));
                        let expected = if index == 0 { "old" } else { "new" };
                        assert!(
                            request
                                .to_ascii_lowercase()
                                .contains(&format!("authorization: bearer {expected}"))
                        );
                        (initial_status, "{}")
                    };
                    stream.write_all(format!("HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            });
            let auth = Arc::new(AuthManager::new(
                InstalledClient {
                    client_id: "synthetic".to_string(),
                    client_secret: Zeroizing::new("test".to_string()),
                    auth_uri: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                    token_uri: format!("http://{address}/token"),
                },
                Arc::new(SessionCredentialStore::default()),
            ));
            auth.accept_login(OAuthTokens {
                access_token: Zeroizing::new("old".to_string()),
                refresh_token: Some(Zeroizing::new("synthetic-refresh".to_string())),
                expires_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
            })
            .await
            .unwrap();
            let response = GoogleHttp::new(Some(auth))
                .get(
                    reqwest::Url::parse(&format!("http://{address}/api")).unwrap(),
                    &Zeroizing::new(String::new()),
                )
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), initial_status);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn requests_refresh_lazily_and_preserve_session_refresh_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut refreshes = 0;
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 8192];
                let count = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                let body = if request.starts_with("POST /token") {
                    refreshes += 1;
                    assert!(request.contains("refresh_token=synthetic-refresh"));
                    r#"{"access_token":"test","expires_in":0}"#
                } else {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("authorization: bearer test")
                    );
                    "{}"
                };
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
            refreshes
        });
        let store = Arc::new(SessionCredentialStore::default());
        let auth = Arc::new(AuthManager::new(
            InstalledClient {
                client_id: "synthetic".to_string(),
                client_secret: Zeroizing::new("synthetic".to_string()),
                auth_uri: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                token_uri: format!("http://{address}/token"),
            },
            store.clone(),
        ));
        auth.accept_login(OAuthTokens {
            access_token: Zeroizing::new("expired".to_string()),
            refresh_token: Some(Zeroizing::new("synthetic-refresh".to_string())),
            expires_at: std::time::Instant::now(),
        })
        .await
        .unwrap();
        store.delete_refresh_token().unwrap();
        let http = GoogleHttp::new(Some(auth));
        for _ in 0..2 {
            http.get(
                reqwest::Url::parse(&format!("http://{address}/api")).unwrap(),
                &Zeroizing::new(String::new()),
            )
            .await
            .unwrap();
        }
        assert_eq!(server.await.unwrap(), 2);
    }
}
