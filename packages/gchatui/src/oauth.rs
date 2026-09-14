use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::credential::Secret;

pub const CHAT_SPACES_READONLY: &str = "https://www.googleapis.com/auth/chat.spaces.readonly";
pub const CHAT_MESSAGES_READONLY: &str = "https://www.googleapis.com/auth/chat.messages.readonly";
pub const DIRECTORY_READONLY: &str = "https://www.googleapis.com/auth/directory.readonly";
pub const CONTACTS_READONLY: &str = "https://www.googleapis.com/auth/contacts.readonly";
pub const USERINFO_PROFILE: &str = "https://www.googleapis.com/auth/userinfo.profile";
pub const CHAT_MEMBERSHIPS_READONLY: &str =
    "https://www.googleapis.com/auth/chat.memberships.readonly";
pub const REQUIRED_SCOPES: [&str; 6] = [
    CHAT_SPACES_READONLY,
    CHAT_MESSAGES_READONLY,
    DIRECTORY_READONLY,
    CONTACTS_READONLY,
    CHAT_MEMBERSHIPS_READONLY,
    USERINFO_PROFILE,
];
const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const LEGACY_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const REVOCATION_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
#[cfg(test)]
const REDIRECT_URI: &str = "http://127.0.0.1";

#[derive(Clone, Deserialize)]
pub struct InstalledClient {
    pub client_id: String,
    #[serde(deserialize_with = "deserialize_secret")]
    pub client_secret: Secret,
    pub auth_uri: String,
    pub token_uri: String,
}

impl std::fmt::Debug for InstalledClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstalledClient")
            .field("client_id", &"[redacted]")
            .field("client_secret", &"[redacted]")
            .field("auth_uri", &self.auth_uri)
            .field("token_uri", &self.token_uri)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientFile {
    installed: InstalledClient,
}

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("could not read OAuth desktop client configuration at {path}")]
    Read { path: PathBuf },
    #[error("OAuth client configuration must contain installed desktop credentials")]
    InvalidClient,
    #[error("OAuth client endpoint is not approved: {0}")]
    InvalidEndpoint(String),
    #[error("OAuth authorization response did not match this login request")]
    StateMismatch,
    #[error("OAuth authorization was denied: {0}")]
    ConsentDenied(String),
    #[error("OAuth authorization response did not include a code")]
    MissingCode,
    #[error("OAuth callback timed out")]
    CallbackTimeout,
    #[error("OAuth callback was cancelled")]
    CallbackCancelled,
    #[error("OAuth callback listener failed")]
    CallbackListener,
    #[error("OAuth browser could not be opened; open the displayed URL manually")]
    BrowserLaunch,
    #[error(
        "Google did not grant all required permissions. Restart gchatui and select all requested permissions on Google's consent screen; your organization may require administrator approval."
    )]
    MissingPermissions,
    #[error("OAuth token request failed")]
    TokenRequest,
    #[error("OAuth token response was malformed")]
    MalformedToken,
}

pub struct LoopbackCallback {
    listener: TcpListener,
    redirect_uri: String,
}

impl LoopbackCallback {
    pub async fn bind() -> Result<Self, OAuthError> {
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .map_err(|_| OAuthError::CallbackListener)?;
        let address = listener
            .local_addr()
            .map_err(|_| OAuthError::CallbackListener)?;
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{}/callback", address.port()),
        })
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub async fn wait(
        self,
        timeout: Duration,
        mut cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> Result<Url, OAuthError> {
        tokio::select! {
            biased;
            changed = cancelled.changed() => {
                let _ = changed;
                Err(OAuthError::CallbackCancelled)
            }
            accepted = tokio::time::timeout(timeout, self.listener.accept()) => {
                let (mut stream, _) = accepted
                    .map_err(|_| OAuthError::CallbackTimeout)?
                    .map_err(|_| OAuthError::CallbackListener)?;
                let mut request = vec![0_u8; 8192];
                let count = stream.read(&mut request).await.map_err(|_| OAuthError::CallbackListener)?;
                let first_line = String::from_utf8_lossy(&request[..count])
                    .lines()
                    .next()
                    .ok_or(OAuthError::CallbackListener)?
                    .to_string();
                let target = first_line
                    .strip_prefix("GET ")
                    .and_then(|line| line.split_once(' ').map(|(target, _)| target))
                    .ok_or(OAuthError::CallbackListener)?;
                let redirect = Url::parse(&format!("{}{}", self.redirect_uri.trim_end_matches("/callback"), target))
                    .map_err(|_| OAuthError::CallbackListener)?;
                let body = "Authorization response received. Return to gchatui to check the result.\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.map_err(|_| OAuthError::CallbackListener)?;
                stream.shutdown().await.map_err(|_| OAuthError::CallbackListener)?;
                Ok(redirect)
            }
        }
    }
}

pub fn open_browser(url: &Url) -> Result<(), OAuthError> {
    webbrowser::open(url.as_str()).map_err(|_| OAuthError::BrowserLaunch)
}

#[derive(Debug)]
pub struct AuthorizationRequest {
    pub url: Url,
    pub state: Secret,
    verifier: Secret,
}

impl AuthorizationRequest {
    pub fn accept_redirect(
        &self,
        redirect: &Url,
        client: &InstalledClient,
        redirect_uri: &str,
    ) -> Result<TokenExchange, OAuthError> {
        let mut state = None;
        let mut code = None;
        let mut error = None;
        for (key, value) in redirect.query_pairs() {
            match key.as_ref() {
                "state" => state = Some(value.into_owned()),
                "code" => code = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                _ => {}
            }
        }
        if state.as_deref() != Some(self.state.as_str()) {
            return Err(OAuthError::StateMismatch);
        }
        if let Some(error) = error {
            return Err(OAuthError::ConsentDenied(error));
        }
        let code = code.ok_or(OAuthError::MissingCode)?;
        if let Some((_, scopes)) = redirect.query_pairs().find(|(key, _)| key == "scope") {
            validate_granted_scopes(&scopes)?;
        }
        Ok(TokenExchange {
            endpoint: client.token_uri.clone(),
            client_id: client.client_id.clone(),
            client_secret: Zeroizing::new(client.client_secret.to_string()),
            code: Zeroizing::new(code),
            verifier: Zeroizing::new(self.verifier.to_string()),
            redirect_uri: redirect_uri.to_string(),
        })
    }
}

#[derive(Debug)]
pub struct RefreshRequest {
    endpoint: String,
    client_id: String,
    client_secret: Secret,
    refresh_token: Secret,
}

impl RefreshRequest {
    #[must_use]
    pub fn new(client: &InstalledClient, refresh_token: Secret) -> Self {
        Self {
            endpoint: client.token_uri.clone(),
            client_id: client.client_id.clone(),
            client_secret: Zeroizing::new(client.client_secret.to_string()),
            refresh_token,
        }
    }

    pub async fn execute(self, client: &reqwest::Client) -> Result<OAuthTokens, OAuthError> {
        let response = client
            .post(&self.endpoint)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|_| OAuthError::TokenRequest)?;
        if !response.status().is_success() {
            return Err(OAuthError::TokenRequest);
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|_| OAuthError::MalformedToken)?;
        if let Some(scopes) = &token.scope {
            validate_granted_scopes(scopes)?;
        }
        Ok(OAuthTokens {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: Instant::now() + Duration::from_secs(token.expires_in),
        })
    }
}

pub async fn revoke(token: &Secret, client: &reqwest::Client) -> Result<(), OAuthError> {
    revoke_at(REVOCATION_ENDPOINT, token, client).await
}

async fn revoke_at(
    endpoint: &str,
    token: &Secret,
    client: &reqwest::Client,
) -> Result<(), OAuthError> {
    let response = client
        .post(endpoint)
        .form(&[("token", token.as_str())])
        .send()
        .await
        .map_err(|_| OAuthError::TokenRequest)?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(OAuthError::TokenRequest)
    }
}

#[derive(Debug)]
pub struct TokenExchange {
    endpoint: String,
    client_id: String,
    client_secret: Secret,
    code: Secret,
    verifier: Secret,
    redirect_uri: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(deserialize_with = "deserialize_secret")]
    access_token: Secret,
    expires_in: u64,
    scope: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_secret")]
    refresh_token: Option<Secret>,
}

pub struct OAuthTokens {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    pub expires_at: Instant,
}

impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthTokens")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl TokenExchange {
    pub async fn execute(self, client: &reqwest::Client) -> Result<OAuthTokens, OAuthError> {
        let response = client
            .post(&self.endpoint)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", self.code.as_str()),
                ("code_verifier", self.verifier.as_str()),
                ("grant_type", "authorization_code"),
                ("redirect_uri", self.redirect_uri.as_str()),
            ])
            .send()
            .await
            .map_err(|_| OAuthError::TokenRequest)?;
        if !response.status().is_success() {
            return Err(OAuthError::TokenRequest);
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|_| OAuthError::MalformedToken)?;
        if let Some(scopes) = &token.scope {
            validate_granted_scopes(scopes)?;
        }
        Ok(OAuthTokens {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: Instant::now() + Duration::from_secs(token.expires_in),
        })
    }
}

fn validate_granted_scopes(scopes: &str) -> Result<(), OAuthError> {
    if REQUIRED_SCOPES.iter().all(|required| {
        scopes.split_ascii_whitespace().any(|granted| {
            granted == *required || (*required == USERINFO_PROFILE && granted == "profile")
        })
    }) {
        Ok(())
    } else {
        Err(OAuthError::MissingPermissions)
    }
}

pub fn resolve_client(path: Option<&Path>) -> Result<InstalledClient, OAuthError> {
    path.map_or_else(
        || {
            Ok(InstalledClient {
                client_id: crate::official_oauth::CLIENT_ID.to_string(),
                client_secret: Zeroizing::new(
                    crate::official_oauth::DESKTOP_CLIENT_VALUE.to_string(),
                ),
                auth_uri: AUTH_ENDPOINT.to_string(),
                token_uri: TOKEN_ENDPOINT.to_string(),
            })
        },
        load_desktop_client,
    )
}

pub fn load_desktop_client(path: &Path) -> Result<InstalledClient, OAuthError> {
    let contents = fs::read_to_string(path).map_err(|_| OAuthError::Read {
        path: path.to_path_buf(),
    })?;
    let client: ClientFile =
        serde_json::from_str(&contents).map_err(|_| OAuthError::InvalidClient)?;
    validate_auth_endpoint(&client.installed.auth_uri)?;
    validate_endpoint(&client.installed.token_uri, TOKEN_ENDPOINT)?;
    if client.installed.client_id.is_empty() || client.installed.client_secret.is_empty() {
        return Err(OAuthError::InvalidClient);
    }
    Ok(client.installed)
}

pub fn authorization_request(
    client: &InstalledClient,
    redirect_uri: &str,
) -> Result<AuthorizationRequest, OAuthError> {
    let verifier = random_secret(32);
    let state = random_secret(24);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&client.auth_uri)
        .map_err(|_| OAuthError::InvalidEndpoint(client.auth_uri.clone()))?;
    url.query_pairs_mut()
        .append_pair("client_id", &client.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("scope", &REQUIRED_SCOPES.join(" "))
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    Ok(AuthorizationRequest {
        url,
        state,
        verifier,
    })
}

fn deserialize_secret<'de, D>(deserializer: D) -> Result<Secret, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Zeroizing::new)
}

fn deserialize_optional_secret<'de, D>(deserializer: D) -> Result<Option<Secret>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(|value| value.map(Zeroizing::new))
}

fn validate_auth_endpoint(actual: &str) -> Result<(), OAuthError> {
    if matches!(actual, AUTH_ENDPOINT | LEGACY_AUTH_ENDPOINT) {
        Ok(())
    } else {
        Err(OAuthError::InvalidEndpoint(actual.to_string()))
    }
}

fn validate_endpoint(actual: &str, expected: &str) -> Result<(), OAuthError> {
    if actual == expected {
        Ok(())
    } else {
        Err(OAuthError::InvalidEndpoint(actual.to_string()))
    }
}

fn random_secret(length: usize) -> Secret {
    let mut bytes = vec![0_u8; length];
    rand::rng().fill_bytes(&mut bytes);
    Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn client() -> InstalledClient {
        InstalledClient {
            client_id: "synthetic-client-id".to_string(),
            client_secret: Zeroizing::new("synthetic-client-secret".to_string()),
            auth_uri: AUTH_ENDPOINT.to_string(),
            token_uri: TOKEN_ENDPOINT.to_string(),
        }
    }

    #[tokio::test]
    async fn refresh_and_revocation_use_form_posts_without_debug_leaks() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for body in [r#"{"access_token":"test","expires_in":3600}"#, ""] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![0_u8; 4096];
                let count = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                if body.is_empty() {
                    assert!(request.contains("token=synthetic-refresh"));
                } else {
                    assert!(request.contains("grant_type=refresh_token"));
                    assert!(request.contains("refresh_token=synthetic-refresh"));
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let endpoint = format!("http://{address}/token");
        let mut installed = client();
        installed.token_uri.clone_from(&endpoint);
        let tokens =
            RefreshRequest::new(&installed, Zeroizing::new("synthetic-refresh".to_string()))
                .execute(&reqwest::Client::new())
                .await
                .unwrap();
        assert_eq!(tokens.access_token.as_str(), "test");
        revoke_at(
            &endpoint,
            &Zeroizing::new("synthetic-refresh".to_string()),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn loopback_accepts_callback_and_supports_timeout_and_cancellation() {
        let callback = LoopbackCallback::bind().await.unwrap();
        let redirect_uri = callback.redirect_uri().to_string();
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let waiter = tokio::spawn(callback.wait(Duration::from_secs(2), cancel_rx));
        let target = format!("{redirect_uri}?code=synthetic&state=synthetic");
        let url = Url::parse(&target).unwrap();
        let address = format!("127.0.0.1:{}", url.port().unwrap());
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let request_target = format!(
            "{}?{}",
            url.path(),
            url.query().expect("callback query should exist")
        );
        stream
            .write_all(
                format!("GET {request_target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let received = waiter.await.unwrap().unwrap();
        assert_eq!(received.query(), Some("code=synthetic&state=synthetic"));
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.contains(&format!("Content-Length: {}", body.len())));
        assert!(headers.contains("Cache-Control: no-store"));
        assert!(!body.contains("synthetic"));

        let callback = LoopbackCallback::bind().await.unwrap();
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        assert!(matches!(
            callback.wait(Duration::from_millis(1), cancel_rx).await,
            Err(OAuthError::CallbackTimeout)
        ));

        let callback = LoopbackCallback::bind().await.unwrap();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        cancel_tx.send(true).unwrap();
        assert!(matches!(
            callback.wait(Duration::from_secs(2), cancel_rx).await,
            Err(OAuthError::CallbackCancelled)
        ));
    }

    #[test]
    fn google_legacy_authorization_endpoint_is_accepted() {
        assert!(validate_auth_endpoint(LEGACY_AUTH_ENDPOINT).is_ok());
        assert!(validate_auth_endpoint(AUTH_ENDPOINT).is_ok());
        assert!(validate_auth_endpoint("https://example.com/auth").is_err());
    }

    #[test]
    fn desktop_client_is_validated_without_copying() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("desktop-client.json");
        fs::write(
            &path,
            format!(
                r#"{{"installed":{{"client_id":"synthetic-id","client_secret":"test","auth_uri":"{AUTH_ENDPOINT}","token_uri":"{TOKEN_ENDPOINT}"}}}}"#
            ),
        )
        .unwrap();
        let loaded = load_desktop_client(&path).unwrap();
        assert_eq!(loaded.client_id, "synthetic-id");
        assert!(!directory.path().join("client-copy.json").exists());
    }

    #[test]
    fn authorization_uses_exact_read_only_scopes_and_pkce() {
        let request = authorization_request(&client(), REDIRECT_URI).unwrap();
        let query = request.url.query().unwrap();
        assert!(query.contains("chat.spaces.readonly"));
        assert!(query.contains("chat.messages.readonly"));
        assert!(query.contains("directory.readonly"));
        assert!(query.contains("contacts.readonly"));
        assert!(query.contains("chat.memberships.readonly"));
        assert!(query.contains("userinfo.profile"));
        assert!(query.contains("code_challenge_method=S256"));
        assert!(!query.contains("chat.spaces+"));
    }

    #[test]
    fn redirect_requires_matching_state() {
        let request = authorization_request(&client(), REDIRECT_URI).unwrap();
        let redirect = Url::parse("http://127.0.0.1/?code=synthetic&state=wrong").unwrap();
        assert!(matches!(
            request.accept_redirect(&redirect, &client(), REDIRECT_URI),
            Err(OAuthError::StateMismatch)
        ));
    }

    #[test]
    fn consent_requires_every_existing_scope() {
        assert!(validate_granted_scopes(&REQUIRED_SCOPES.join(" ")).is_ok());
        assert!(
            validate_granted_scopes(
                &REQUIRED_SCOPES
                    .join(" ")
                    .replace(USERINFO_PROFILE, "profile")
            )
            .is_ok()
        );
        for omitted in REQUIRED_SCOPES {
            let scopes = REQUIRED_SCOPES
                .iter()
                .copied()
                .filter(|scope| *scope != omitted)
                .collect::<Vec<_>>()
                .join(" ");
            assert!(matches!(
                validate_granted_scopes(&scopes),
                Err(OAuthError::MissingPermissions)
            ));
        }
    }

    #[test]
    fn partial_consent_is_rejected_before_token_exchange() {
        let request = authorization_request(&client(), REDIRECT_URI).unwrap();
        let mut redirect = Url::parse(REDIRECT_URI).unwrap();
        redirect
            .query_pairs_mut()
            .append_pair("state", &request.state)
            .append_pair("code", "synthetic-code")
            .append_pair("scope", "profile");
        assert!(matches!(
            request.accept_redirect(&redirect, &client(), REDIRECT_URI),
            Err(OAuthError::MissingPermissions)
        ));
        redirect.set_query(None);
        redirect
            .query_pairs_mut()
            .append_pair("state", &request.state)
            .append_pair("code", "synthetic-code")
            .append_pair("scope", &REQUIRED_SCOPES.join(" "));
        assert!(
            request
                .accept_redirect(&redirect, &client(), REDIRECT_URI)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn token_exchange_rejects_partial_permissions() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).await.unwrap();
            assert!(count > 0);
            let body = r#"{"access_token":"test","expires_in":3600,"scope":"profile"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let mut installed = client();
        installed.token_uri = format!("http://{address}/token");
        let request = authorization_request(&installed, REDIRECT_URI).unwrap();
        let mut redirect = Url::parse(REDIRECT_URI).unwrap();
        redirect
            .query_pairs_mut()
            .append_pair("state", &request.state)
            .append_pair("code", "synthetic");
        let exchange = request
            .accept_redirect(&redirect, &installed, REDIRECT_URI)
            .unwrap();
        assert!(matches!(
            exchange.execute(&reqwest::Client::new()).await,
            Err(OAuthError::MissingPermissions)
        ));
        server.await.unwrap();
    }

    #[test]
    fn official_client_is_default_and_invalid_override_never_falls_back() {
        let official = resolve_client(None).unwrap();
        assert_eq!(official.auth_uri, AUTH_ENDPOINT);
        assert_eq!(official.token_uri, TOKEN_ENDPOINT);
        assert!(!official.client_id.is_empty());
        let directory = tempfile::tempdir().unwrap();
        assert!(resolve_client(Some(&directory.path().join("missing"))).is_err());
    }

    #[test]
    fn debug_output_redacts_credentials_and_tokens() {
        let client = client();
        let rendered = format!("{client:?}");
        assert!(!rendered.contains("synthetic-client-secret"));
        let tokens = OAuthTokens {
            access_token: Zeroizing::new("synthetic-access-value".to_string()),
            refresh_token: Some(Zeroizing::new("synthetic-refresh-value".to_string())),
            expires_at: Instant::now(),
        };
        let rendered = format!("{tokens:?}");
        assert!(!rendered.contains("synthetic-access-value"));
        assert!(!rendered.contains("synthetic-refresh-value"));
    }
}
