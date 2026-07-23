//! Write Once, Read Many (WORM) Cryptographic Audit Log Engine.
//!
//! Provides tamper-evident HMAC-SHA256 block linking for all system mutations,
//! rule executions, account additions, and AI MCP agent tool calls.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Default HMAC key used for WORM audit log block linking when no custom key is specified.
pub const DEFAULT_WORM_KEY: &[u8] = b"nuncio-worm-cryptographic-audit-key-v1";

/// Error types emitted during WORM audit log creation and verification.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum WormAuditError {
    /// Cryptographic signature verification failed for a record in the chain.
    #[error("tampering detected at sequence {sequence}: expected HMAC {expected_hmac}, found {actual_hmac}")]
    TamperingDetected {
        /// Sequence index where tampering occurred.
        sequence: u64,
        /// Expected cryptographic HMAC.
        expected_hmac: String,
        /// Actual cryptographic HMAC found in storage.
        actual_hmac: String,
    },
    /// Cryptographic signing failure.
    #[error("cryptographic signing error: {0}")]
    SigningError(String),
}

/// A single immutable WORM audit log record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WormAuditRecord {
    /// Monotonically increasing sequence index.
    pub sequence: u64,
    /// UTC timestamp in nanoseconds since Unix epoch.
    pub timestamp_ns: i64,
    /// Subsystem or actor executing the action (e.g. "system", "user_gui", "mcp_agent:claude").
    pub actor: String,
    /// Action identifier (e.g. "mail.send", "rule.execute", "account.add").
    pub action: String,
    /// SHA-256 hash of the target entity payload or data payload.
    pub data_hash: String,
    /// Previous block's HMAC signature ("GENESIS" for sequence 1).
    pub previous_block_hash: String,
    /// HMAC-SHA256 signature sealing this block.
    pub record_hmac: String,
}

impl WormAuditRecord {
    /// Compute the HMAC-SHA256 signature for a record payload.
    pub fn compute_hmac(
        secret_key: &[u8],
        sequence: u64,
        timestamp_ns: i64,
        actor: &str,
        action: &str,
        data_hash: &str,
        previous_block_hash: &str,
    ) -> Result<String, WormAuditError> {
        let mut mac = HmacSha256::new_from_slice(secret_key)
            .map_err(|e| WormAuditError::SigningError(e.to_string()))?;

        mac.update(&sequence.to_be_bytes());
        mac.update(&timestamp_ns.to_be_bytes());
        mac.update(actor.as_bytes());
        mac.update(action.as_bytes());
        mac.update(data_hash.as_bytes());
        mac.update(previous_block_hash.as_bytes());

        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Create and sign a new WORM audit record.
    pub fn create_signed(
        secret_key: &[u8],
        sequence: u64,
        timestamp_ns: i64,
        actor: impl Into<String>,
        action: impl Into<String>,
        data_payload: &[u8],
        previous_block_hash: impl Into<String>,
    ) -> Result<Self, WormAuditError> {
        let actor = actor.into();
        let action = action.into();
        let previous_block_hash = previous_block_hash.into();

        // Compute SHA-256 hash of the data payload
        let data_hash = hex::encode(sha2::Sha256::digest(data_payload));

        let record_hmac = Self::compute_hmac(
            secret_key,
            sequence,
            timestamp_ns,
            &actor,
            &action,
            &data_hash,
            &previous_block_hash,
        )?;

        Ok(Self {
            sequence,
            timestamp_ns,
            actor,
            action,
            data_hash,
            previous_block_hash,
            record_hmac,
        })
    }

    /// Verify the cryptographic HMAC integrity of this record.
    pub fn verify_integrity(&self, secret_key: &[u8]) -> Result<(), WormAuditError> {
        let expected = Self::compute_hmac(
            secret_key,
            self.sequence,
            self.timestamp_ns,
            &self.actor,
            &self.action,
            &self.data_hash,
            &self.previous_block_hash,
        )?;

        if expected == self.record_hmac {
            Ok(())
        } else {
            Err(WormAuditError::TamperingDetected {
                sequence: self.sequence,
                expected_hmac: expected,
                actual_hmac: self.record_hmac.clone(),
            })
        }
    }
}

/// Verify an entire sequence chain of WORM audit records.
pub fn verify_worm_chain(records: &[WormAuditRecord], secret_key: &[u8]) -> Result<(), WormAuditError> {
    let mut last_hash = "GENESIS".to_string();

    for record in records {
        if record.previous_block_hash != last_hash {
            return Err(WormAuditError::TamperingDetected {
                sequence: record.sequence,
                expected_hmac: last_hash,
                actual_hmac: record.previous_block_hash.clone(),
            });
        }

        record.verify_integrity(secret_key)?;
        last_hash = record.record_hmac.clone();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_verify_single_worm_record() {
        let key = b"secret-test-key";
        let record = WormAuditRecord::create_signed(
            key,
            1,
            1688140800000000000,
            "user_gui",
            "mail.send",
            b"hello world email payload",
            "GENESIS",
        )
        .expect("signed record creation");

        assert_eq!(record.sequence, 1);
        assert_eq!(record.previous_block_hash, "GENESIS");
        assert!(record.verify_integrity(key).is_ok());
    }

    #[test]
    fn test_tampered_record_fails_verification() {
        let key = b"secret-test-key";
        let mut record = WormAuditRecord::create_signed(
            key,
            1,
            1688140800000000000,
            "user_gui",
            "mail.send",
            b"hello world email payload",
            "GENESIS",
        )
        .expect("signed record creation");

        // Tamper with action payload
        record.action = "mail.delete".to_string();

        let err = record.verify_integrity(key).expect_err("tampering caught");
        assert!(matches!(err, WormAuditError::TamperingDetected { .. }));
    }

    #[test]
    fn test_worm_chain_verification_passes() {
        let key = b"secret-test-key";
        let r1 = WormAuditRecord::create_signed(
            key,
            1,
            100,
            "system",
            "db.init",
            b"data1",
            "GENESIS",
        )
        .unwrap();

        let r2 = WormAuditRecord::create_signed(
            key,
            2,
            200,
            "mcp_agent:claude",
            "mcp.tool_call",
            b"data2",
            &r1.record_hmac,
        )
        .unwrap();

        let chain = vec![r1, r2];
        assert!(verify_worm_chain(&chain, key).is_ok());
    }
}
