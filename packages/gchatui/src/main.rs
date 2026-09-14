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
mod official_oauth;
pub mod people;
pub mod product;
pub mod sender_alias;
pub mod transcript_projection;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let path = config::default_config_path();
    let mut config = config::load(path.as_deref())?;
    let mut access_token = if config.session_only {
        None
    } else {
        startup_access_token(&config, false).await.ok().flatten()
    };
    let aliases = config::default_state_dir()
        .map(|state| {
            sender_alias::SenderAliases::load(config::sender_alias_path(&state))
                .map(std::sync::Arc::new)
        })
        .transpose()?;
    loop {
        let show_auth = access_token.is_none();
        let selection = app::run(
            config.keybindings.clone(),
            access_token.take(),
            aliases.clone(),
            show_auth,
        )
        .await?;
        let Some(selection) = selection else {
            return Ok(());
        };
        config.session_only = selection == 0;
        config.vault_passphrase = selection != 2;
        match startup_access_token(&config, true).await {
            Ok(token) => {
                access_token = token;
                match path.as_deref() {
                    Some(path) => {
                        if config::save_storage_choice(
                            path,
                            config.session_only,
                            config.vault_passphrase,
                        )
                        .is_err()
                        {
                            eprintln!(
                                "Signed in, but could not save the storage preference. The next launch will use the previous setting."
                            );
                        }
                    }
                    None => eprintln!(
                        "Signed in, but no configuration directory is available to remember the storage preference."
                    ),
                }
            }
            Err(error) => eprintln!(
                "Sign-in failed: {error}. Choose Sign in / storage to retry. Workspace policy may require administrator approval."
            ),
        }
    }
}

async fn startup_access_token(
    config: &config::AppConfig,
    reauthorize: bool,
) -> anyhow::Result<Option<credential::Secret>> {
    let client = oauth::resolve_client(config.oauth_client_path.as_deref())?;
    let store: std::sync::Arc<dyn credential::CredentialStore + Send + Sync> = if config
        .session_only
    {
        std::sync::Arc::new(credential::SessionCredentialStore::default())
    } else {
        let state = config::default_state_dir()
            .ok_or_else(|| anyhow::anyhow!("could not resolve gchatui state directory"))?;
        let (vault, identity) = credential::client_auth_state_paths(&state, &client.client_id);
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
    let manager = if reauthorize {
        auth_command::login_client(client, store).await?
    } else {
        auth_command::ensure_client_authorized(client, store).await?
    };
    let token = manager.access_token().await?;
    Ok(Some(token))
}
