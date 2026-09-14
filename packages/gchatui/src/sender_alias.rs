use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AliasError {
    #[error("sender alias file could not be read")]
    Read(#[source] std::io::Error),
    #[error("sender alias file is invalid")]
    Parse(#[source] toml::de::Error),
    #[error("sender alias file could not be written")]
    Write(#[source] std::io::Error),
    #[error("sender alias name cannot be empty")]
    Empty,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AliasFile {
    #[serde(default)]
    sender_aliases: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct SenderAliases {
    path: PathBuf,
    values: Arc<RwLock<BTreeMap<String, String>>>,
}

impl SenderAliases {
    pub fn load(path: PathBuf) -> Result<Self, AliasError> {
        let values = if path.exists() {
            let contents = fs::read_to_string(&path).map_err(AliasError::Read)?;
            toml::from_str::<AliasFile>(&contents)
                .map_err(AliasError::Parse)?
                .sender_aliases
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path,
            values: Arc::new(RwLock::new(values)),
        })
    }

    #[must_use]
    pub fn get(&self, resource_name: &str) -> Option<String> {
        self.values
            .read()
            .expect("sender alias lock poisoned")
            .get(resource_name)
            .cloned()
    }

    pub fn set(&self, resource_name: &str, display_name: &str) -> Result<(), AliasError> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            return Err(AliasError::Empty);
        }
        self.values
            .write()
            .expect("sender alias lock poisoned")
            .insert(resource_name.to_string(), display_name.to_string());
        self.save()
    }

    #[must_use]
    pub fn apply(&self, resource_name: &str, current: Option<&str>) -> Option<String> {
        self.get(resource_name).or_else(|| {
            current
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
        })
    }

    fn save(&self) -> Result<(), AliasError> {
        let parent = self.path.parent().ok_or_else(|| {
            AliasError::Write(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "sender alias path has no parent",
            ))
        })?;
        fs::create_dir_all(parent).map_err(AliasError::Write)?;
        let contents = toml::to_string_pretty(&AliasFile {
            sender_aliases: self
                .values
                .read()
                .expect("sender alias lock poisoned")
                .clone(),
        })
        .map_err(|error| AliasError::Write(std::io::Error::other(error)))?;
        write_private_file(&self.path, contents.as_bytes())
    }
}

#[cfg(unix)]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), AliasError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let temporary = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(AliasError::Write)?;
    file.write_all(contents).map_err(AliasError::Write)?;
    fs::rename(temporary, path).map_err(AliasError::Write)
}

#[cfg(windows)]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), AliasError> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents).map_err(AliasError::Write)?;
    fs::rename(temporary, path).map_err(AliasError::Write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_persist_and_override_remote_names() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sender-aliases.toml");
        let aliases = SenderAliases::load(path.clone()).unwrap();
        aliases.set("users/123", "Local Name").unwrap();
        assert_eq!(
            aliases.apply("users/123", Some("Remote Name")).as_deref(),
            Some("Local Name")
        );
        let reopened = SenderAliases::load(path).unwrap();
        assert_eq!(reopened.get("users/123").as_deref(), Some("Local Name"));
    }
}
