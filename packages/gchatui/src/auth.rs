use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::credential::{CredentialError, CredentialStore, Secret};
use crate::oauth::{InstalledClient, OAuthError, OAuthTokens, RefreshRequest};

const EXPIRY_SKEW: Duration = Duration::from_mins(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    SignedOut,
    Authorized,
    Expiring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoutOutcome {
    RevokedAndDeleted,
    LocalDeletedAfterRevocationFailure,
    AlreadySignedOut,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("stored authorization could not be accessed")]
    Credentials(#[from] CredentialError),
    #[error("authorization must be completed again")]
    ReauthenticationRequired,
    #[error("authorization refresh failed")]
    Refresh(#[source] OAuthError),
}

pub struct AuthManager {
    client: InstalledClient,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
    http: reqwest::Client,
    tokens: Mutex<Option<OAuthTokens>>,
}

impl AuthManager {
    #[must_use]
    pub fn new(
        client: InstalledClient,
        credentials: Arc<dyn CredentialStore + Send + Sync>,
    ) -> Self {
        Self {
            client,
            credentials,
            http: reqwest::Client::new(),
            tokens: Mutex::new(None),
        }
    }

    pub async fn accept_login(&self, tokens: OAuthTokens) -> Result<(), AuthError> {
        if let Some(refresh_token) = tokens.refresh_token.as_ref() {
            self.credentials
                .save_refresh_token(Zeroizing::new(refresh_token.to_string()))?;
        }
        *self.tokens.lock().await = Some(tokens);
        Ok(())
    }

    pub async fn status(&self) -> Result<AuthStatus, AuthError> {
        let guard = self.tokens.lock().await;
        if let Some(tokens) = guard.as_ref() {
            return Ok(if tokens.expires_at <= Instant::now() + EXPIRY_SKEW {
                AuthStatus::Expiring
            } else {
                AuthStatus::Authorized
            });
        }
        drop(guard);
        Ok(if self.credentials.load_refresh_token()?.is_some() {
            AuthStatus::Expiring
        } else {
            AuthStatus::SignedOut
        })
    }

    pub async fn access_token(&self) -> Result<Secret, AuthError> {
        // The lock intentionally covers refresh. Concurrent callers queue here and
        // observe the single refreshed value rather than issuing duplicate grants.
        let mut guard = self.tokens.lock().await;
        if let Some(tokens) = guard.as_ref()
            && tokens.expires_at > Instant::now() + EXPIRY_SKEW
        {
            return Ok(Zeroizing::new(tokens.access_token.to_string()));
        }
        let refresh_token = match guard
            .as_ref()
            .and_then(|tokens| tokens.refresh_token.as_ref())
        {
            Some(token) => Zeroizing::new(token.to_string()),
            None => self
                .credentials
                .load_refresh_token()?
                .ok_or(AuthError::ReauthenticationRequired)?,
        };
        let mut refreshed = RefreshRequest::new(&self.client, refresh_token.clone())
            .execute(&self.http)
            .await
            .map_err(AuthError::Refresh)?;
        if let Some(rotated) = &refreshed.refresh_token {
            self.credentials.save_refresh_token(rotated.clone())?;
        } else {
            refreshed.refresh_token = Some(refresh_token);
        }
        let access = Zeroizing::new(refreshed.access_token.to_string());
        *guard = Some(refreshed);
        drop(guard);
        Ok(access)
    }

    pub async fn logout(&self) -> Result<LogoutOutcome, AuthError> {
        let token = self.credentials.load_refresh_token()?;
        let Some(token) = token else {
            *self.tokens.lock().await = None;
            return Ok(LogoutOutcome::AlreadySignedOut);
        };
        let revoked = crate::oauth::revoke(&token, &self.http).await.is_ok();
        self.credentials.delete_refresh_token()?;
        *self.tokens.lock().await = None;
        Ok(if revoked {
            LogoutOutcome::RevokedAndDeleted
        } else {
            LogoutOutcome::LocalDeletedAfterRevocationFailure
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::SessionCredentialStore;

    fn client(endpoint: String) -> InstalledClient {
        InstalledClient {
            client_id: "test-client".to_string(),
            client_secret: Zeroizing::new("test".to_string()),
            auth_uri: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_uri: endpoint,
        }
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_refresh() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let body = r#"{"access_token":"test","expires_in":3600}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let store = Arc::new(SessionCredentialStore::default());
        store
            .save_refresh_token(Zeroizing::new("test-refresh".to_string()))
            .unwrap();
        let manager = Arc::new(AuthManager::new(
            client(format!("http://{address}/token")),
            store,
        ));
        let (left, right) = tokio::join!(manager.access_token(), manager.access_token());
        assert_eq!(left.unwrap().as_str(), "test");
        assert_eq!(right.unwrap().as_str(), "test");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn logout_deletes_local_token_when_revocation_fails() {
        let store = Arc::new(SessionCredentialStore::default());
        store
            .save_refresh_token(Zeroizing::new("test-refresh".to_string()))
            .unwrap();
        let manager = AuthManager::new(
            client("http://127.0.0.1:1/token".to_string()),
            store.clone(),
        );
        assert_eq!(
            manager.logout().await.unwrap(),
            LogoutOutcome::LocalDeletedAfterRevocationFailure
        );
        assert!(store.load_refresh_token().unwrap().is_none());
    }
}
