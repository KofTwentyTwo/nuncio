//! Bounded provisioning of the daemon's own key material.
//!
//! `nunciod` normally *mints* the keys it uses, so the OS keyring hands them
//! back without ceremony. That is a habit, not a guarantee: a keychain locked
//! by screen-lock, a restored or migrated keychain whose ACL no longer trusts
//! this binary, a re-signed or rebuilt `nunciod`, or a Linux Secret Service
//! with no unlocked collection all turn a keyring read into a blocking wait on
//! something nobody can answer. Unbounded, that wait becomes a daemon that
//! hangs at startup with no error and no log line.
//!
//! Every read here therefore goes through
//! [`nuncio_store::vault::get_or_create_key_bytes_bounded`], so the worst case
//! is an honest, logged startup failure instead of a silent stall. Errors name
//! the key material's purpose and the likely cause; they never carry the key.

use nuncio_store::vault::{
    get_or_create_key_bytes_bounded, SecretManager, GRPC_TOKEN_ACCOUNT, WEBHOOK_SIGNING_KEY_ACCOUNT,
};
use std::sync::Arc;
use std::time::Duration;

/// Byte length of the daemon's symmetric key material (256 bits).
const KEY_LEN: usize = 32;

/// Provision the loopback gRPC bearer token, minting it on first run and
/// loading it back on every subsequent one.
///
/// Fails closed: the daemon must never serve its API with a token it could not
/// confirm, and must never invent one.
pub async fn provision_grpc_token(
    secrets: Arc<SecretManager>,
    timeout: Duration,
) -> Result<String, String> {
    get_or_create_key_bytes_bounded(secrets, GRPC_TOKEN_ACCOUNT, KEY_LEN, timeout)
        .await
        .map(hex::encode)
        .map_err(|e| {
            format!(
                "failed to provision the gRPC bearer token that nunciod needs to authenticate \
                 its loopback API: {e}"
            )
        })
}

/// Provision the daemon-wide HMAC key that signs outbound filter webhooks.
///
/// Fails closed: a `CALL WEBHOOK` is never dispatched unsigned.
pub async fn provision_webhook_signing_key(
    secrets: Arc<SecretManager>,
    timeout: Duration,
) -> Result<String, String> {
    get_or_create_key_bytes_bounded(secrets, WEBHOOK_SIGNING_KEY_ACCOUNT, KEY_LEN, timeout)
        .await
        .map(hex::encode)
        .map_err(|e| {
            format!(
                "failed to provision the webhook signing key that nunciod needs to sign outbound \
                 filter webhooks: {e}"
            )
        })
}
