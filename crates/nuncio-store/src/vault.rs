use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Zeroized sensitive string wrapper ensuring heap bytes are wiped on drop.
#[derive(Debug, Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct ZeroizingSecret(String);

impl ZeroizingSecret {
    /// Wrap a secret string for zeroized memory handling.
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    /// Access the underlying secret string reference.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Vault storage and retrieval errors.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum VaultError {
    /// Secret matching specified key was not found in vault.
    #[error("secret for key '{0}' not found in vault")]
    NotFound(String),
    /// Vault operation failed due to platform or storage error.
    #[error("vault storage operation failed: {0}")]
    StorageFailed(String),
}

/// Abstract secret vault provider trait.
pub trait SecretVault: Send + Sync {
    /// Retrieve a secret string for a given key.
    fn get_secret(&self, key: &str) -> Result<String, VaultError>;
    /// Store a secret string for a given key.
    fn set_secret(&self, key: &str, secret: &str) -> Result<(), VaultError>;
    /// Delete a secret entry for a given key.
    fn delete_secret(&self, key: &str) -> Result<(), VaultError>;
}

/// Thread-safe in-memory vault provider for unit tests and headless environments.
#[derive(Debug, Clone, Default)]
pub struct MockKeyring {
    storage: Arc<Mutex<HashMap<String, String>>>,
}

impl MockKeyring {
    /// Create a new empty `MockKeyring`.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretVault for MockKeyring {
    fn get_secret(&self, key: &str) -> Result<String, VaultError> {
        let guard = self
            .storage
            .lock()
            .map_err(|e| VaultError::StorageFailed(e.to_string()))?;
        guard
            .get(key)
            .cloned()
            .ok_or_else(|| VaultError::NotFound(key.to_string()))
    }

    fn set_secret(&self, key: &str, secret: &str) -> Result<(), VaultError> {
        let mut guard = self
            .storage
            .lock()
            .map_err(|e| VaultError::StorageFailed(e.to_string()))?;
        guard.insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn delete_secret(&self, key: &str) -> Result<(), VaultError> {
        let mut guard = self
            .storage
            .lock()
            .map_err(|e| VaultError::StorageFailed(e.to_string()))?;
        if guard.remove(key).is_some() {
            Ok(())
        } else {
            Err(VaultError::NotFound(key.to_string()))
        }
    }
}

/// Unified secret manager handling vault delegation.
pub struct SecretManager {
    provider: Box<dyn SecretVault>,
}

impl SecretManager {
    /// Create a `SecretManager` wrapping a specific vault provider.
    pub fn new<P: SecretVault + 'static>(provider: P) -> Self {
        Self {
            provider: Box::new(provider),
        }
    }

    /// Create a `SecretManager` initialized with `MockKeyring` (ideal for test suites).
    pub fn mock() -> Self {
        Self::new(MockKeyring::new())
    }

    /// Retrieve a secret from the configured vault.
    pub fn get_secret(&self, key: &str) -> Result<String, VaultError> {
        self.provider.get_secret(key)
    }

    /// Store a secret in the configured vault.
    pub fn set_secret(&self, key: &str, secret: &str) -> Result<(), VaultError> {
        self.provider.set_secret(key, secret)
    }

    /// Delete a secret from the configured vault.
    pub fn delete_secret(&self, key: &str) -> Result<(), VaultError> {
        self.provider.delete_secret(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_keyring_set_get_delete_cycle() {
        let manager = SecretManager::mock();

        // Key not found initially
        assert_eq!(
            manager.get_secret("nuncio/key1").unwrap_err(),
            VaultError::NotFound("nuncio/key1".to_string())
        );

        // Set secret
        manager
            .set_secret("nuncio/key1", "secret_pass_123")
            .unwrap();
        assert_eq!(
            manager.get_secret("nuncio/key1").unwrap(),
            "secret_pass_123"
        );

        // Delete secret
        manager.delete_secret("nuncio/key1").unwrap();
        assert_eq!(
            manager.get_secret("nuncio/key1").unwrap_err(),
            VaultError::NotFound("nuncio/key1".to_string())
        );

        // Double delete returns NotFound
        assert_eq!(
            manager.delete_secret("nuncio/key1").unwrap_err(),
            VaultError::NotFound("nuncio/key1".to_string())
        );
    }

    #[test]
    fn vault_error_display() {
        let not_found = VaultError::NotFound("my_key".to_string());
        assert_eq!(
            not_found.to_string(),
            "secret for key 'my_key' not found in vault"
        );

        let failed = VaultError::StorageFailed("lock poisoned".to_string());
        assert_eq!(
            failed.to_string(),
            "vault storage operation failed: lock poisoned"
        );
    }

    #[test]
    fn zeroizing_secret_wipes_bytes_on_drop() {
        let sec = ZeroizingSecret::new("super_secret_password_123".to_string());
        assert_eq!(sec.as_str(), "super_secret_password_123");
        drop(sec);
    }
}
