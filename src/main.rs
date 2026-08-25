//! gchatui executable entry point.

mod app;
pub mod auth;
pub mod auth_command;
pub mod chat;
mod config;
pub mod credential;
mod keybind;
pub mod model;
pub mod oauth;
pub mod product;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let path = config::default_config_path();
    let config = config::load(path.as_deref())?;
    let access_token = startup_access_token(&config).await?;
    app::run(config.keybindings, access_token).await
}

async fn startup_access_token(
    config: &config::AppConfig,
) -> anyhow::Result<Option<credential::Secret>> {
    let Some(client_path) = config.oauth_client_path.as_deref() else {
        return Ok(None);
    };
    let store: std::sync::Arc<dyn credential::CredentialStore + Send + Sync> = if config
        .session_only
    {
        std::sync::Arc::new(credential::SessionCredentialStore::default())
    } else {
        let state = config::default_state_dir()
            .ok_or_else(|| anyhow::anyhow!("could not resolve gchatui state directory"))?;
        let (vault, identity) = credential::auth_state_paths(&state);
        if config.vault_passphrase {
            let passphrase = auth_command::prompt_passphrase(!identity.exists())?;
            if !identity.exists() {
                let bootstrap =
                    credential::SshenvCredentialStore::bootstrap(&vault, &identity, &passphrase)?;
                drop(bootstrap);
            }
            std::sync::Arc::new(credential::SshenvCredentialStore::with_passphrase(
                vault,
                identity,
                &passphrase,
            )?)
        } else {
            std::sync::Arc::new(credential::SshenvCredentialStore::bootstrap_unencrypted(
                vault, identity,
            )?)
        }
    };
    let manager = auth_command::ensure_authorized(client_path, store).await?;
    manager.access_token().await.map(Some).map_err(Into::into)
}
