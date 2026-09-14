use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use age::secrecy::SecretString;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use sshenv_vault::{SshenvStore, SshenvStoreConfig, Vault};
use thiserror::Error;
use zeroize::Zeroizing;

const REQUIRED_GRANT_VERSION: u8 = 4;
const AUTH_PROFILE: &str = "authentication";
const REFRESH_TOKEN_KEY: &str = "oauth_refresh_token";

pub type Secret = Zeroizing<String>;

fn encode_refresh_token(token: &Secret) -> Secret {
    Zeroizing::new(format!("v{REQUIRED_GRANT_VERSION}:{}", token.as_str()))
}

fn decode_refresh_token(value: &Secret) -> Option<Secret> {
    value
        .strip_prefix(&format!("v{REQUIRED_GRANT_VERSION}:"))
        .map(|token| Zeroizing::new(token.to_string()))
}

pub trait CredentialStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, CredentialError>;
    fn save_refresh_token(&self, token: Secret) -> Result<(), CredentialError>;
    fn delete_refresh_token(&self) -> Result<bool, CredentialError>;
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("credential identity operation failed")]
    Identity(#[source] ssh_key::Error),
    #[error("credential identity is unavailable or the passphrase was rejected")]
    Unlock,
    #[error("credential state could not be written")]
    Write(#[source] std::io::Error),
    #[error("credential vault operation failed")]
    Vault(#[source] anyhow::Error),
}

pub struct SshenvCredentialStore {
    vault_path: PathBuf,
    identity_path: PathBuf,
    store: SshenvStore,
    passphrase: Option<Secret>,
}

impl SshenvCredentialStore {
    pub fn new(vault_path: impl Into<PathBuf>, identity_path: impl Into<PathBuf>) -> Self {
        let vault_path = vault_path.into();
        let identity_path = identity_path.into();
        Self {
            store: SshenvStore::new(
                SshenvStoreConfig::new(vault_path.clone())
                    .with_private_key_paths(vec![identity_path.clone()]),
            ),
            vault_path,
            identity_path,
            passphrase: None,
        }
    }

    pub fn bootstrap(
        vault_path: impl Into<PathBuf>,
        identity_path: impl Into<PathBuf>,
        passphrase: &Secret,
    ) -> Result<Self, CredentialError> {
        let store = Self::new(vault_path, identity_path);
        if !store.identity_path.exists() {
            let identity = generate_encrypted_identity(&store.identity_path, passphrase)?;
            store.initialize(&identity.public_key)?;
        } else if !store.vault_path.exists() {
            store.initialize(&read_identity_public_key(&store.identity_path)?)?;
        }
        Ok(store)
    }

    pub fn bootstrap_unencrypted(
        vault_path: impl Into<PathBuf>,
        identity_path: impl Into<PathBuf>,
    ) -> Result<Self, CredentialError> {
        let store = Self::new(vault_path, identity_path);
        if !store.identity_path.exists() {
            let identity = generate_unencrypted_identity(&store.identity_path)?;
            store.initialize(&identity.public_key)?;
        } else if !store.vault_path.exists() {
            store.initialize(&read_identity_public_key(&store.identity_path)?)?;
        }
        Ok(store)
    }

    pub fn reset(self) -> Result<(), CredentialError> {
        remove_if_present(&self.vault_path)?;
        remove_if_present(&self.identity_path)?;
        remove_if_present(&self.identity_path.with_extension("pub"))
    }

    pub fn with_passphrase(
        vault_path: impl Into<PathBuf>,
        identity_path: impl Into<PathBuf>,
        passphrase: &Secret,
    ) -> Result<Self, CredentialError> {
        let vault_path = vault_path.into();
        let identity_path = identity_path.into();
        let _ = identities_from_encrypted_key(&identity_path, passphrase)?;
        Ok(Self {
            store: SshenvStore::new(
                SshenvStoreConfig::new(vault_path.clone())
                    .with_private_key_paths(vec![identity_path.clone()]),
            ),
            vault_path,
            identity_path,
            passphrase: Some(Zeroizing::new(passphrase.to_string())),
        })
    }

    pub fn initialize(&self, recipient_public_key: &str) -> Result<bool, CredentialError> {
        self.store
            .init_if_missing(recipient_public_key)
            .map_err(CredentialError::Vault)
    }
}

impl CredentialStore for SshenvCredentialStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, CredentialError> {
        if let Some(passphrase) = self.passphrase.as_ref() {
            let identities = identities_from_encrypted_key(&self.identity_path, passphrase)?;
            let ciphertext =
                Vault::load_ciphertext(&self.vault_path).map_err(CredentialError::Vault)?;
            let (mut vault, data_key) =
                Vault::unlock(ciphertext, identities.as_slice()).map_err(CredentialError::Vault)?;
            if vault.profiles.profile_entries.contains_key(AUTH_PROFILE) {
                vault
                    .unlock_profile_with_passphrase(AUTH_PROFILE, &data_key, None)
                    .map_err(CredentialError::Vault)?;
            }
            return Ok(vault
                .profiles
                .get(AUTH_PROFILE)
                .and_then(|values| values.get(REFRESH_TOKEN_KEY))
                .map(|value| Zeroizing::new(value.clone()))
                .and_then(|value| decode_refresh_token(&value)));
        }
        self.store
            .get_secret(AUTH_PROFILE, REFRESH_TOKEN_KEY)
            .map_err(CredentialError::Vault)
            .map(|value| value.and_then(|value| decode_refresh_token(&value)))
    }

    fn save_refresh_token(&self, token: Secret) -> Result<(), CredentialError> {
        if let Some(passphrase) = self.passphrase.as_ref() {
            let identities = identities_from_encrypted_key(&self.identity_path, passphrase)?;
            let ciphertext =
                Vault::load_ciphertext(&self.vault_path).map_err(CredentialError::Vault)?;
            let (mut vault, data_key) =
                Vault::unlock(ciphertext, identities.as_slice()).map_err(CredentialError::Vault)?;
            if vault.profiles.profile_entries.contains_key(AUTH_PROFILE) {
                vault
                    .unlock_profile_with_passphrase(AUTH_PROFILE, &data_key, None)
                    .map_err(CredentialError::Vault)?;
            }
            vault.profiles.set(
                AUTH_PROFILE,
                REFRESH_TOKEN_KEY,
                encode_refresh_token(&token).to_string(),
            );
            return vault
                .save(&self.vault_path, &data_key)
                .map_err(CredentialError::Vault);
        }
        self.store
            .set_secret(
                AUTH_PROFILE,
                REFRESH_TOKEN_KEY,
                encode_refresh_token(&token),
            )
            .map_err(CredentialError::Vault)
    }

    fn delete_refresh_token(&self) -> Result<bool, CredentialError> {
        if let Some(passphrase) = self.passphrase.as_ref() {
            let identities = identities_from_encrypted_key(&self.identity_path, passphrase)?;
            let ciphertext =
                Vault::load_ciphertext(&self.vault_path).map_err(CredentialError::Vault)?;
            let (mut vault, data_key) =
                Vault::unlock(ciphertext, identities.as_slice()).map_err(CredentialError::Vault)?;
            if vault.profiles.profile_entries.contains_key(AUTH_PROFILE) {
                vault
                    .unlock_profile_with_passphrase(AUTH_PROFILE, &data_key, None)
                    .map_err(CredentialError::Vault)?;
            }
            let removed = vault.profiles.unset(AUTH_PROFILE, REFRESH_TOKEN_KEY);
            if removed {
                vault
                    .save(&self.vault_path, &data_key)
                    .map_err(CredentialError::Vault)?;
            }
            return Ok(removed);
        }
        self.store
            .unset_secret(AUTH_PROFILE, REFRESH_TOKEN_KEY)
            .map_err(CredentialError::Vault)
    }
}

#[derive(Default)]
pub struct SessionCredentialStore {
    refresh_token: std::sync::Mutex<Option<Secret>>,
}

impl CredentialStore for SessionCredentialStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, CredentialError> {
        Ok(self
            .refresh_token
            .lock()
            .expect("session credential lock poisoned")
            .as_ref()
            .map(|token| Zeroizing::new(token.to_string())))
    }

    fn save_refresh_token(&self, token: Secret) -> Result<(), CredentialError> {
        *self
            .refresh_token
            .lock()
            .expect("session credential lock poisoned") = Some(token);
        Ok(())
    }

    fn delete_refresh_token(&self) -> Result<bool, CredentialError> {
        Ok(self
            .refresh_token
            .lock()
            .expect("session credential lock poisoned")
            .take()
            .is_some())
    }
}

fn identities_from_encrypted_key(
    identity_path: &Path,
    passphrase: &Secret,
) -> Result<Vec<Box<dyn age::Identity>>, CredentialError> {
    let encoded = fs::read(identity_path).map_err(CredentialError::Write)?;
    let identity = age::ssh::Identity::from_buffer(
        Cursor::new(encoded),
        Some(identity_path.display().to_string()),
    )
    .map_err(CredentialError::Write)?;
    let unencrypted = match identity {
        age::ssh::Identity::Encrypted(encrypted) => encrypted
            .decrypt(SecretString::from(passphrase.to_string()))
            .map_err(|_| CredentialError::Unlock)?,
        age::ssh::Identity::Unencrypted(_) | age::ssh::Identity::Unsupported(_) => {
            return Err(CredentialError::Unlock);
        }
    };
    Ok(vec![Box::new(age::ssh::Identity::from(unencrypted))])
}

pub fn verify_identity_passphrase(
    identity_path: &Path,
    passphrase: &Secret,
) -> Result<(), CredentialError> {
    let encoded = fs::read_to_string(identity_path).map_err(CredentialError::Write)?;
    let private = PrivateKey::from_openssh(&encoded).map_err(CredentialError::Identity)?;
    private
        .decrypt(passphrase.as_bytes())
        .map(|_| ())
        .map_err(|_| CredentialError::Unlock)
}

pub fn vault_version(vault_path: &Path) -> Result<u8, CredentialError> {
    Vault::load_ciphertext(vault_path)
        .map(|vault| vault.header.version)
        .map_err(CredentialError::Vault)
}

pub struct GeneratedIdentity {
    pub public_key: String,
    pub private_key_path: PathBuf,
}

pub fn generate_unencrypted_identity(
    identity_path: &Path,
) -> Result<GeneratedIdentity, CredentialError> {
    prepare_identity_parent(identity_path)?;
    let private = PrivateKey::random(&mut ssh_key::rand_core::OsRng, Algorithm::Ed25519)
        .map_err(CredentialError::Identity)?;
    write_identity(identity_path, &private)
}

pub fn generate_encrypted_identity(
    identity_path: &Path,
    passphrase: &Secret,
) -> Result<GeneratedIdentity, CredentialError> {
    prepare_identity_parent(identity_path)?;

    let private = PrivateKey::random(&mut ssh_key::rand_core::OsRng, Algorithm::Ed25519)
        .map_err(CredentialError::Identity)?;
    let encrypted = private
        .encrypt(&mut ssh_key::rand_core::OsRng, passphrase.as_bytes())
        .map_err(CredentialError::Identity)?;
    write_identity(identity_path, &encrypted)
}

fn prepare_identity_parent(identity_path: &Path) -> Result<(), CredentialError> {
    let parent = identity_path.parent().ok_or_else(|| {
        CredentialError::Write(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "identity path has no parent",
        ))
    })?;
    fs::create_dir_all(parent).map_err(CredentialError::Write)
}

fn write_identity(
    identity_path: &Path,
    private: &PrivateKey,
) -> Result<GeneratedIdentity, CredentialError> {
    let public_key = private
        .public_key()
        .to_openssh()
        .map_err(CredentialError::Identity)?;
    let encoded = private
        .to_openssh(LineEnding::LF)
        .map_err(CredentialError::Identity)?;

    write_private_file(identity_path, encoded.as_bytes())?;
    fs::write(
        identity_path.with_extension("pub"),
        format!("{public_key}\n"),
    )
    .map_err(CredentialError::Write)?;
    Ok(GeneratedIdentity {
        public_key,
        private_key_path: identity_path.to_path_buf(),
    })
}

#[cfg(unix)]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), CredentialError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(CredentialError::Write)?;
    file.write_all(contents).map_err(CredentialError::Write)
}

#[cfg(windows)]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), CredentialError> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(path)
        .map_err(CredentialError::Write)?;
    std::io::Write::write_all(&mut file, contents).map_err(CredentialError::Write)
}

fn read_identity_public_key(identity_path: &Path) -> Result<String, CredentialError> {
    let public_path = identity_path.with_extension("pub");
    if public_path.exists() {
        return fs::read_to_string(public_path)
            .map(|value| value.trim().to_string())
            .map_err(CredentialError::Write);
    }
    let private =
        PrivateKey::read_openssh_file(identity_path).map_err(CredentialError::Identity)?;
    private
        .public_key()
        .to_openssh()
        .map_err(CredentialError::Identity)
}

fn remove_if_present(path: &Path) -> Result<(), CredentialError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CredentialError::Write(error)),
    }
}

#[must_use]
pub fn client_auth_state_paths(state_root: &Path, client_id: &str) -> (PathBuf, PathBuf) {
    use sha2::{Digest, Sha256};
    let fingerprint = format!("{:x}", Sha256::digest(client_id.as_bytes()));
    auth_state_paths(&state_root.join("oauth-clients").join(fingerprint))
}

#[must_use]
pub fn auth_state_paths(state_root: &Path) -> (PathBuf, PathBuf) {
    (state_root.join("auth.vault"), state_root.join("identity"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn client_namespaces_do_not_reuse_legacy_or_other_client_credentials() {
        let directory = tempdir().unwrap();
        let first = client_auth_state_paths(directory.path(), "synthetic-client-a");
        let second = client_auth_state_paths(directory.path(), "synthetic-client-b");
        assert_ne!(first, second);
        assert_ne!(first, auth_state_paths(directory.path()));
        assert_eq!(
            first,
            client_auth_state_paths(directory.path(), "synthetic-client-a")
        );
        assert!(first.0.starts_with(directory.path()));
        assert!(!first.0.to_string_lossy().contains("synthetic-client-a"));
        let store = SshenvCredentialStore::bootstrap_unencrypted(&first.0, &first.1).unwrap();
        store
            .save_refresh_token(Zeroizing::new("synthetic-token".to_string()))
            .unwrap();
        let other = SshenvCredentialStore::bootstrap_unencrypted(&second.0, &second.1).unwrap();
        assert!(other.load_refresh_token().unwrap().is_none());
        assert!(store.load_refresh_token().unwrap().is_some());
    }

    #[test]
    fn unencrypted_identity_enables_prompt_free_persistent_lifecycle() {
        let directory = tempdir().unwrap();
        let (vault, identity) = auth_state_paths(directory.path());
        let store = SshenvCredentialStore::bootstrap_unencrypted(&vault, &identity).unwrap();
        let private = PrivateKey::read_openssh_file(&identity).unwrap();
        assert!(!private.is_encrypted());
        store
            .save_refresh_token(Zeroizing::new("synthetic-convenience-token".to_string()))
            .unwrap();
        let reopened = SshenvCredentialStore::new(&vault, &identity);
        assert_eq!(
            reopened
                .load_refresh_token()
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("synthetic-convenience-token")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&identity).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn persistent_store_uses_app_supplied_passphrase_for_full_lifecycle() {
        let directory = tempdir().unwrap();
        let (vault, identity) = auth_state_paths(directory.path());
        let passphrase = Zeroizing::new("synthetic-persistent-passphrase".to_string());
        let bootstrap = SshenvCredentialStore::bootstrap(&vault, &identity, &passphrase).unwrap();
        drop(bootstrap);
        let store = SshenvCredentialStore::with_passphrase(&vault, &identity, &passphrase).unwrap();
        assert!(store.load_refresh_token().unwrap().is_none());
        store
            .save_refresh_token(Zeroizing::new("synthetic-persistent-token".to_string()))
            .unwrap();
        assert_eq!(
            store
                .load_refresh_token()
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("synthetic-persistent-token")
        );
        assert!(store.delete_refresh_token().unwrap());
        assert!(store.load_refresh_token().unwrap().is_none());
    }

    #[test]
    fn bootstrap_creates_app_owned_identity_and_vault_then_reset_removes_them() {
        let directory = tempdir().unwrap();
        let (vault, identity) = auth_state_paths(directory.path());
        let passphrase = Zeroizing::new("synthetic-bootstrap-passphrase".to_string());
        let store = SshenvCredentialStore::bootstrap(&vault, &identity, &passphrase).unwrap();
        assert!(vault.exists());
        assert!(identity.exists());
        assert!(identity.with_extension("pub").exists());
        assert!(vault_version(&vault).unwrap() >= 1);
        assert!(verify_identity_passphrase(&identity, &passphrase).is_ok());
        let wrong = Zeroizing::new("wrong-synthetic-passphrase".to_string());
        assert!(matches!(
            verify_identity_passphrase(&identity, &wrong),
            Err(CredentialError::Unlock)
        ));
        store.reset().unwrap();
        assert!(!vault.exists());
        assert!(!identity.exists());
        assert!(!identity.with_extension("pub").exists());
    }

    #[test]
    fn generated_identity_is_encrypted_and_private() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("identity");
        let passphrase = Zeroizing::new("synthetic-passphrase-value".to_string());
        let generated = generate_encrypted_identity(&path, &passphrase).unwrap();
        assert_eq!(generated.private_key_path, path);
        assert!(generated.public_key.starts_with("ssh-ed25519 "));
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("OPENSSH PRIVATE KEY"));
        assert!(!contents.contains("synthetic-passphrase-value"));
        let parsed = PrivateKey::from_openssh(&contents).unwrap();
        assert!(parsed.is_encrypted());
        assert!(parsed.decrypt(passphrase.as_bytes()).is_ok());
        assert!(parsed.decrypt(b"wrong-passphrase").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn session_store_lifecycle_is_in_memory() {
        let store = SessionCredentialStore::default();
        assert!(store.load_refresh_token().unwrap().is_none());
        store
            .save_refresh_token(Zeroizing::new("synthetic-refresh-value".to_string()))
            .unwrap();
        assert_eq!(
            store
                .load_refresh_token()
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("synthetic-refresh-value")
        );
        assert!(store.delete_refresh_token().unwrap());
        assert!(store.load_refresh_token().unwrap().is_none());
    }

    #[test]
    fn auth_paths_are_app_specific() {
        let (vault, identity) = auth_state_paths(Path::new("/tmp/gchatui-test"));
        assert!(vault.ends_with("gchatui-test/auth.vault"));
        assert!(identity.ends_with("gchatui-test/identity"));
    }
}
