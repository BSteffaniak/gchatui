use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::keybind::{KeybindingError, KeybindingOverrides, KeybindingRegistry};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    oauth_client_path: Option<PathBuf>,
    #[serde(default)]
    session_only: bool,
    #[serde(default = "default_vault_passphrase")]
    vault_passphrase: bool,
    #[serde(default)]
    keybindings: KeybindingOverrides,
}

const fn default_vault_passphrase() -> bool {
    true
}

pub struct AppConfig {
    pub oauth_client_path: Option<PathBuf>,
    pub session_only: bool,
    pub vault_passphrase: bool,
    pub keybindings: KeybindingRegistry,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read configuration at {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("configuration at {path} is invalid: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("keybinding configuration is invalid: {0}")]
    Keybindings(#[from] KeybindingError),
}

pub fn load(path: Option<&Path>) -> Result<AppConfig, ConfigError> {
    let Some(path) = path else {
        return Ok(AppConfig {
            oauth_client_path: None,
            session_only: false,
            vault_passphrase: true,
            keybindings: KeybindingRegistry::default(),
        });
    };
    if !path.exists() {
        return Ok(AppConfig {
            oauth_client_path: None,
            session_only: false,
            vault_passphrase: true,
            keybindings: KeybindingRegistry::default(),
        });
    }
    let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let config: ConfigFile = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(AppConfig {
        oauth_client_path: config.oauth_client_path,
        session_only: config.session_only,
        vault_passphrase: config.vault_passphrase,
        keybindings: KeybindingRegistry::with_overrides(&config.keybindings)?,
    })
}

#[cfg(test)]
pub fn load_keybindings(path: Option<&Path>) -> Result<KeybindingRegistry, ConfigError> {
    load(path).map(|config| config.keybindings)
}

pub fn default_state_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .or_else(dirs::data_dir)
        .map(|directory| directory.join("gchatui"))
}

pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("gchatui").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybind::Action;
    use tempfile::tempdir;

    #[test]
    fn macos_has_a_state_directory_fallback() {
        #[cfg(target_os = "macos")]
        assert!(default_state_dir().is_some());
    }

    #[test]
    fn absent_file_uses_defaults() {
        let directory = tempdir().unwrap();
        let registry = load_keybindings(Some(&directory.path().join("missing.toml"))).unwrap();
        assert_eq!(registry.labels_for(Action::Refresh), ["r"]);
    }

    #[test]
    fn file_overrides_and_unbinds() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"
            vault_passphrase = false

            [keybindings]
            unbind = ["help"]

            [keybindings.bind]
            refresh = ["Ctrl+r"]
            "#,
        )
        .unwrap();
        let config = load(Some(&path)).unwrap();
        assert!(!config.vault_passphrase);
        let registry = config.keybindings;
        assert_eq!(registry.labels_for(Action::Refresh), ["Ctrl+r"]);
        assert!(registry.labels_for(Action::Help).is_empty());
    }
}
