use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::keybind::{KeybindingError, KeybindingOverrides, KeybindingRegistry};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    clock_format: crate::date_display::ClockFormat,
    oauth_client_path: Option<PathBuf>,
    #[serde(default = "default_session_only")]
    session_only: bool,
    #[serde(default = "default_vault_passphrase")]
    vault_passphrase: bool,
    #[serde(default)]
    keybindings: KeybindingOverrides,
}

const fn default_session_only() -> bool {
    true
}

const fn default_vault_passphrase() -> bool {
    true
}

pub struct AppConfig {
    pub clock_format: crate::date_display::ClockFormat,
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
            clock_format: crate::date_display::ClockFormat::default(),
            oauth_client_path: None,
            session_only: true,
            vault_passphrase: true,
            keybindings: KeybindingRegistry::default(),
        });
    };
    if !path.exists() {
        return Ok(AppConfig {
            clock_format: crate::date_display::ClockFormat::default(),
            oauth_client_path: None,
            session_only: true,
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
        clock_format: config.clock_format,
        oauth_client_path: config.oauth_client_path,
        session_only: config.session_only,
        vault_passphrase: config.vault_passphrase,
        keybindings: KeybindingRegistry::with_overrides(&config.keybindings)?,
    })
}

pub fn save_storage_choice(
    path: &Path,
    session_only: bool,
    vault_passphrase: bool,
) -> anyhow::Result<()> {
    use std::io::Write;

    // Validate the current file before modifying it. Preserve all other values,
    // including custom client selection and keybindings.
    load(Some(path))?;
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut table: toml::Table = toml::from_str(&contents)?;
    table.insert("session_only".to_string(), session_only.into());
    table.insert("vault_passphrase".to_string(), vault_passphrase.into());
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(toml::to_string_pretty(&table)?.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
pub fn load_keybindings(path: Option<&Path>) -> Result<KeybindingRegistry, ConfigError> {
    load(path).map(|config| config.keybindings)
}

pub fn sender_alias_path(state_root: &Path) -> PathBuf {
    state_root.join("sender-aliases.toml")
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
    fn clock_format_is_configurable_and_preserved_by_storage_changes() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "clock_format = '12h'\n").unwrap();
        assert!(matches!(
            load(Some(&path)).unwrap().clock_format,
            crate::date_display::ClockFormat::TwelveHour
        ));
        save_storage_choice(&path, false, true).unwrap();
        assert!(matches!(
            load(Some(&path)).unwrap().clock_format,
            crate::date_display::ClockFormat::TwelveHour
        ));
        fs::write(&path, "clock_format = 'invalid'\n").unwrap();
        assert!(load(Some(&path)).is_err());
    }

    #[test]
    fn storage_choice_survives_reload_and_preserves_other_settings() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "oauth_client_path = '/synthetic/client.json'\n[keybindings.bind]\nrefresh = ['Ctrl+r']\n").unwrap();
        for (session, passphrase) in [(false, false), (false, true), (true, true)] {
            save_storage_choice(&path, session, passphrase).unwrap();
            let config = load(Some(&path)).unwrap();
            assert_eq!(config.session_only, session);
            assert_eq!(config.vault_passphrase, passphrase);
            assert_eq!(
                config.oauth_client_path,
                Some(PathBuf::from("/synthetic/client.json"))
            );
            assert_eq!(config.keybindings.labels_for(Action::Refresh), ["Ctrl+r"]);
        }
    }

    #[test]
    fn storage_choice_creates_config_and_leaves_invalid_config_untouched() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("new/config.toml");
        save_storage_choice(&path, false, false).unwrap();
        assert!(!load(Some(&path)).unwrap().session_only);
        fs::write(&path, "not valid toml").unwrap();
        assert!(save_storage_choice(&path, true, true).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not valid toml");
    }

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
