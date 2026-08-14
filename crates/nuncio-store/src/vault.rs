use aes_gcm::aead::{rand_core::RngCore, OsRng};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Stable OS keyring service name under which all Nuncio-managed secrets are stored.
///
/// Changing this value would orphan previously-provisioned keys on user machines, so it
/// MUST remain stable across releases.
pub const KEYRING_SERVICE: &str = "mx.nuncio.vault";

/// Keyring account name for the AES-256-GCM key used to encrypt message bodies at rest
/// (see [`crate::cipher::PayloadCipher::encrypt_text_at_rest`]).
pub const STORAGE_KEY_ACCOUNT: &str = "storage-encryption-key";

/// Keyring account name for the HMAC-SHA256 key used to sign the WORM audit log chain
/// (see `nuncio_core::WormAuditRecord`).
pub const WORM_KEY_ACCOUNT: &str = "worm-audit-hmac-key";

/// Keyring account name for the HMAC-SHA256 key used to sign the filter execution log
/// ledger (see [`crate::db::DatabaseEngine::save_filter_execution_log`]).
pub const LEDGER_KEY_ACCOUNT: &str = "ledger-hmac-key";

/// Keyring account name for the bearer token used to authenticate loopback gRPC calls
/// against the `nuncio.v1` API server exposed by `nunciod`. The token is minted on first use via [`SecretManager::get_or_create_key_bytes`]
/// and hex-encoded for use as an `authorization: Bearer <token>` header value.
pub const GRPC_TOKEN_ACCOUNT: &str = "grpc-bearer-token";

/// Keyring account name for the HMAC-SHA256 key that signs outbound filter
/// `CALL WEBHOOK` payloads (the `X-Nuncio-Signature` header produced by
/// `nuncio_filter::WebhookDispatcher`). Minted on first use via
/// [`SecretManager::get_or_create_key_bytes`] and hex-encoded; a single
/// daemon-wide signing key lets a webhook receiver verify that a delivery
/// genuinely originated from this daemon.
pub const WEBHOOK_SIGNING_KEY_ACCOUNT: &str = "webhook-signing-key";

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

/// Production `SecretVault` backed by the native OS credential store: Windows Credential
/// Manager, macOS/iOS Keychain, or the Linux Secret Service / kernel keyutils (via the
/// `keyring` crate). This is the ONLY vault provider that should ever be wired into a
/// running `nunciod`, `nuncio-cli`, `nuncio-tui`, `nuncio-gui`, or `nuncio-mcp` process
/// operating on real user data. Tests MUST use [`MockKeyring`] instead so CI (which runs
/// headless, with no OS keyring available) never depends on it.
#[derive(Debug, Clone)]
pub struct OsKeyring {
    service: String,
}

impl OsKeyring {
    /// Create an `OsKeyring` provider scoped to the given keyring service name. Production
    /// callers should use [`KEYRING_SERVICE`].
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl SecretVault for OsKeyring {
    fn get_secret(&self, key: &str) -> Result<String, VaultError> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| VaultError::StorageFailed(e.to_string()))?;
        match entry.get_password() {
            Ok(secret) => Ok(secret),
            Err(keyring::Error::NoEntry) => Err(VaultError::NotFound(key.to_string())),
            Err(e) => Err(VaultError::StorageFailed(e.to_string())),
        }
    }

    fn set_secret(&self, key: &str, secret: &str) -> Result<(), VaultError> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| VaultError::StorageFailed(e.to_string()))?;
        entry
            .set_password(secret)
            .map_err(|e| VaultError::StorageFailed(e.to_string()))
    }

    fn delete_secret(&self, key: &str) -> Result<(), VaultError> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| VaultError::StorageFailed(e.to_string()))?;
        match entry.delete_password() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Err(VaultError::NotFound(key.to_string())),
            Err(e) => Err(VaultError::StorageFailed(e.to_string())),
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

    /// Create a `SecretManager` backed by the real OS keyring under [`KEYRING_SERVICE`].
    /// This is the production entry point used by `nunciod` and any other process that
    /// must persist cryptographic key material across restarts.
    pub fn production() -> Self {
        Self::new(OsKeyring::new(KEYRING_SERVICE))
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

    /// Retrieve raw key material stored under `account`, generating and persisting
    /// `len` bytes of cryptographically random key material via a CSPRNG (`OsRng`) on
    /// first use if none exists yet.
    ///
    /// This implements the mandatory key lifecycle: on first run a random key is minted
    /// and written to the vault; on every subsequent run the same key is loaded back.
    /// It NEVER falls back to a compiled-in default — if the vault is unavailable or
    /// contains corrupt key material, this fails closed with a [`VaultError`].
    pub fn get_or_create_key_bytes(
        &self,
        account: &str,
        len: usize,
    ) -> Result<Vec<u8>, VaultError> {
        match self.get_secret(account) {
            Ok(hex_encoded) => hex::decode(&hex_encoded).map_err(|e| {
                VaultError::StorageFailed(format!("corrupt key material for '{account}': {e}"))
            }),
            Err(VaultError::NotFound(_)) => {
                let mut bytes = vec![0u8; len];
                OsRng.fill_bytes(&mut bytes);
                self.set_secret(account, &hex::encode(&bytes))?;
                // Lifecycle visibility only: which purpose got a key minted, and how long it
                // is, never the key material itself.
                tracing::info!(purpose = account, len, "minted key material on first use");
                Ok(bytes)
            }
            Err(e) => Err(e),
        }
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

    #[test]
    fn get_or_create_key_bytes_generates_random_material_on_first_use() {
        let manager = SecretManager::mock();

        // Nothing provisioned yet: proves there is no compiled-in default key.
        assert_eq!(
            manager.get_secret(STORAGE_KEY_ACCOUNT).unwrap_err(),
            VaultError::NotFound(STORAGE_KEY_ACCOUNT.to_string())
        );

        let key = manager
            .get_or_create_key_bytes(STORAGE_KEY_ACCOUNT, 32)
            .expect("key provisioned from vault");
        assert_eq!(key.len(), 32);

        // A freshly-generated key must not be all-zero (would indicate a weak/no-op RNG).
        assert!(key.iter().any(|&b| b != 0));
    }

    #[test]
    fn get_or_create_key_bytes_is_stable_across_calls() {
        let manager = SecretManager::mock();

        let first = manager
            .get_or_create_key_bytes(WORM_KEY_ACCOUNT, 32)
            .expect("first provisioning succeeds");
        let second = manager
            .get_or_create_key_bytes(WORM_KEY_ACCOUNT, 32)
            .expect("second lookup succeeds");

        assert_eq!(
            first, second,
            "key must be loaded, not regenerated, on subsequent runs"
        );
    }

    #[test]
    fn get_or_create_key_bytes_two_accounts_yield_different_keys() {
        let manager = SecretManager::mock();

        let a = manager
            .get_or_create_key_bytes(STORAGE_KEY_ACCOUNT, 32)
            .unwrap();
        let b = manager
            .get_or_create_key_bytes(LEDGER_KEY_ACCOUNT, 32)
            .unwrap();

        assert_ne!(a, b);
    }

    #[test]
    fn get_or_create_key_bytes_fails_closed_on_corrupt_material() {
        let manager = SecretManager::mock();
        manager
            .set_secret(STORAGE_KEY_ACCOUNT, "not-valid-hex!!")
            .unwrap();

        let err = manager
            .get_or_create_key_bytes(STORAGE_KEY_ACCOUNT, 32)
            .expect_err("corrupt key material must fail closed, never fall back to a default");
        assert!(matches!(err, VaultError::StorageFailed(_)));
    }

    #[test]
    fn get_or_create_key_bytes_logs_minting_without_leaking_the_key() {
        let manager = SecretManager::mock();
        let logs = crate::test_tracing::capture_logs();
        let key = manager
            .get_or_create_key_bytes(WEBHOOK_SIGNING_KEY_ACCOUNT, 32)
            .expect("key provisioned from vault");

        let captured = logs.text();
        assert!(
            captured.contains(WEBHOOK_SIGNING_KEY_ACCOUNT),
            "expected the key's purpose to be named in the INFO log, got: {captured}"
        );
        assert!(captured.to_ascii_uppercase().contains("INFO"));

        // The minted key bytes must never appear in the log, in any encoding.
        let hex_key = hex::encode(&key);
        assert!(!captured.contains(&hex_key));
    }

    #[test]
    fn os_keyring_provider_can_be_constructed_for_production_use() {
        // Construction alone must not touch the OS credential store (lazy `Entry` creation
        // happens per-call), so this is safe to run in headless CI.
        let provider = OsKeyring::new(KEYRING_SERVICE);
        assert_eq!(provider.service, KEYRING_SERVICE);

        let manager = SecretManager::production();
        // `SecretManager::production()` must wrap an `OsKeyring`, never a mock, in the
        // default production path. We only assert successful construction here; actually
        // exercising get/set against the real OS keyring is intentionally left untested so
        // headless CI never depends on a real credential store being available.
        let _ = manager;
    }
}
