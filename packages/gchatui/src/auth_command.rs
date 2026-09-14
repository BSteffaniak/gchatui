use std::path::Path;
use std::sync::Arc;

use thiserror::Error;
use zeroize::Zeroizing;

use crate::auth::{AuthError, AuthManager, AuthStatus, LogoutOutcome};
use crate::credential::{CredentialError, CredentialStore, Secret};
use crate::oauth::{
    LoopbackCallback, OAuthError, authorization_request, load_desktop_client, open_browser,
};

#[derive(Debug, Error)]
pub enum AuthCommandError {
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error("passphrase input failed")]
    Passphrase,
}

pub async fn ensure_authorized(
    client_path: &Path,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
) -> Result<Arc<AuthManager>, AuthCommandError> {
    let client = load_desktop_client(client_path)?;
    ensure_client_authorized(client, credentials).await
}

pub async fn ensure_client_authorized(
    client: crate::oauth::InstalledClient,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
) -> Result<Arc<AuthManager>, AuthCommandError> {
    let manager = Arc::new(AuthManager::new(client.clone(), Arc::clone(&credentials)));
    match manager.status().await? {
        AuthStatus::SignedOut => login_client(client, credentials).await,
        AuthStatus::Authorized | AuthStatus::Expiring => {
            manager.access_token().await?;
            Ok(manager)
        }
    }
}

pub async fn login(
    client_path: &Path,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
    passphrase: Option<Secret>,
) -> Result<Arc<AuthManager>, AuthCommandError> {
    // Passphrase custody belongs to the caller's credential-store construction.
    // Keeping it in this signature makes the product entry point explicit and
    // prevents hidden environment or argument fallbacks.
    drop(passphrase);
    let client = load_desktop_client(client_path)?;
    login_client(client, credentials).await
}

async fn login_client(
    client: crate::oauth::InstalledClient,
    credentials: Arc<dyn CredentialStore + Send + Sync>,
) -> Result<Arc<AuthManager>, AuthCommandError> {
    let callback = LoopbackCallback::bind().await?;
    let request = authorization_request(&client, callback.redirect_uri())?;
    if open_browser(&request.url).is_err() {
        eprintln!("Open this URL in your browser:\n{}", request.url);
    }
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let redirect = callback
        .wait(std::time::Duration::from_mins(3), cancel_rx)
        .await?;
    let exchange =
        request.accept_redirect(&redirect, &client, redirect_origin(&redirect).as_str())?;
    let tokens = exchange.execute(&reqwest::Client::new()).await?;
    let manager = Arc::new(AuthManager::new(client, credentials));
    manager.accept_login(tokens).await?;
    Ok(manager)
}

pub async fn status(manager: &AuthManager) -> Result<AuthStatus, AuthCommandError> {
    manager.status().await.map_err(Into::into)
}

pub async fn logout(manager: &AuthManager) -> Result<LogoutOutcome, AuthCommandError> {
    manager.logout().await.map_err(Into::into)
}

pub fn reset_authorization(store: &dyn CredentialStore) -> Result<bool, AuthCommandError> {
    store.delete_refresh_token().map_err(Into::into)
}

pub fn prompt_passphrase(confirm: bool) -> Result<Secret, AuthCommandError> {
    let first = rpassword::prompt_password("gchatui vault passphrase: ")
        .map_err(|_| AuthCommandError::Passphrase)?;
    let first = Zeroizing::new(first);
    if confirm {
        let second = rpassword::prompt_password("confirm passphrase: ")
            .map_err(|_| AuthCommandError::Passphrase)?;
        let second = Zeroizing::new(second);
        if first.as_str() != second.as_str() {
            return Err(AuthCommandError::Passphrase);
        }
    }
    Ok(first)
}

fn redirect_origin(redirect: &reqwest::Url) -> String {
    format!(
        "{}://{}:{}{}",
        redirect.scheme(),
        redirect.host_str().unwrap_or("127.0.0.1"),
        redirect.port().unwrap_or(80),
        redirect.path()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::SessionCredentialStore;

    #[test]
    fn reset_removes_local_authorization() {
        let store = SessionCredentialStore::default();
        store
            .save_refresh_token(Zeroizing::new("test-refresh".to_string()))
            .unwrap();
        assert!(reset_authorization(&store).unwrap());
        assert!(store.load_refresh_token().unwrap().is_none());
    }

    #[test]
    fn redirect_origin_excludes_query_secrets() {
        let url = reqwest::Url::parse(
            "http://127.0.0.1:4321/callback?code=private-code&state=private-state",
        )
        .unwrap();
        let origin = redirect_origin(&url);
        assert_eq!(origin, "http://127.0.0.1:4321/callback");
        assert!(!origin.contains("private-code"));
    }
}
