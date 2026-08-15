//! The daemon's own keyring reads must always terminate.
//!
//! `nunciod` provisions its bearer token and webhook signing key at startup and
//! on first webhook dispatch. A keyring that never answers -- a locked keychain,
//! an ACL that no longer trusts this binary, a Secret Service with no unlocked
//! collection -- used to leave the daemon parked on that read with no error and
//! no log line. These tests pin the bounded behaviour: an honest error, named
//! key material, and never the key itself.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_store::vault::{SecretManager, SecretVault, VaultError};
use nunciod::secrets::{provision_grpc_token, provision_webhook_signing_key};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Sentinel standing in for real key material: if any of it ever reaches an
/// error message, the assertions below catch it.
const SENTINEL_KEY_HEX: &str = "53454e54494e454c6b65796d6174657269616c4141424243434444454546464747";

/// Vault whose reads never return, standing in for a locked keychain or one
/// waiting on an access-consent prompt nobody can answer.
struct BlockingVault;

impl SecretVault for BlockingVault {
    fn get_secret(&self, _key: &str) -> Result<String, VaultError> {
        loop {
            std::thread::park();
        }
    }
    fn set_secret(&self, _key: &str, _secret: &str) -> Result<(), VaultError> {
        Ok(())
    }
    fn delete_secret(&self, _key: &str) -> Result<(), VaultError> {
        Ok(())
    }
}

fn blocked_secrets() -> Arc<SecretManager> {
    Arc::new(SecretManager::new(BlockingVault))
}

#[tokio::test]
async fn blocked_bearer_token_read_fails_instead_of_hanging_startup() {
    let started = Instant::now();
    let err = provision_grpc_token(blocked_secrets(), Duration::from_millis(150))
        .await
        .expect_err("a keyring that never answers must not yield a token");

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "startup must give up on the read, not wait on it"
    );
    assert!(
        err.contains("gRPC bearer token"),
        "the error must name which key material could not be read: {err}"
    );
    assert!(
        err.contains("locked") || err.contains("consent"),
        "the error must suggest the likely cause: {err}"
    );
    assert!(
        !err.contains(SENTINEL_KEY_HEX),
        "an error must never carry key material: {err}"
    );
}

#[tokio::test]
async fn blocked_webhook_key_read_fails_instead_of_hanging_the_outbox() {
    let started = Instant::now();
    let err = provision_webhook_signing_key(blocked_secrets(), Duration::from_millis(150))
        .await
        .expect_err("a keyring that never answers must not yield a signing key");

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the outbox must give up on the read, not wait on it"
    );
    assert!(
        err.contains("webhook signing key"),
        "the error must name which key material could not be read: {err}"
    );
    assert!(
        err.contains("locked") || err.contains("consent"),
        "the error must suggest the likely cause: {err}"
    );
    assert!(
        !err.contains(SENTINEL_KEY_HEX),
        "an error must never carry key material: {err}"
    );
}

#[tokio::test]
async fn a_working_keyring_still_mints_and_reloads_the_same_key_material() {
    let secrets = Arc::new(SecretManager::mock());
    let timeout = Duration::from_secs(5);

    let token = provision_grpc_token(Arc::clone(&secrets), timeout)
        .await
        .expect("the mock vault mints a token");
    // 32 bytes, hex-encoded.
    assert_eq!(token.len(), 64);
    assert_eq!(
        token,
        provision_grpc_token(Arc::clone(&secrets), timeout)
            .await
            .expect("the token reloads across reads"),
        "bounding the read must not change the token the daemon serves with"
    );

    let webhook_key = provision_webhook_signing_key(Arc::clone(&secrets), timeout)
        .await
        .expect("the mock vault mints a signing key");
    assert_eq!(webhook_key.len(), 64);
    assert_ne!(
        token, webhook_key,
        "each purpose must get its own key material"
    );
}

#[tokio::test]
async fn a_failing_keyring_reports_the_vault_failure_not_a_timeout() {
    struct FailingVault;
    impl SecretVault for FailingVault {
        fn get_secret(&self, _key: &str) -> Result<String, VaultError> {
            Err(VaultError::StorageFailed("no unlocked collection".into()))
        }
        fn set_secret(&self, _key: &str, _secret: &str) -> Result<(), VaultError> {
            Ok(())
        }
        fn delete_secret(&self, _key: &str) -> Result<(), VaultError> {
            Ok(())
        }
    }

    let err = provision_grpc_token(
        Arc::new(SecretManager::new(FailingVault)),
        Duration::from_secs(5),
    )
    .await
    .expect_err("a failing vault must fail closed");

    assert!(
        err.contains("no unlocked collection"),
        "a real vault failure must surface verbatim: {err}"
    );
    assert!(
        !err.contains("timed out"),
        "a real vault failure must not be reported as a timeout: {err}"
    );
}
