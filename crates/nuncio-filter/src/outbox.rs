//! Transactional Outbox Pattern for Remote IMAP/JMAP Mutations.

use crate::ast::PendingRemoteMutation;
use nuncio_core::model::PlacementKey;
use serde::{Deserialize, Serialize};

/// Payload details stored for pending remote mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationPayload {
    /// Action type (`MOVE`, `FLAG`, `UNFLAG`, `MARK_READ`, `MARK_UNREAD`, `DELETE`, `FORWARD`).
    pub action_type: String,
    /// Target parameter (e.g. folder name, target email address).
    pub target: Option<String>,
    /// The mailbox occupancy this mutation was formed against.
    ///
    /// Message identity is content-addressed and folder-independent, so a single
    /// message can occupy several mailboxes at once (Gmail labels make that
    /// ordinary). `MOVE` and `DELETE` act on *one* occupancy, and acting on the
    /// wrong one is destructive and irreversible, so the caller that formed the
    /// intent records which occupancy it meant. `None` preserves the older
    /// rows' meaning -- "unrecorded" -- which the executor may resolve only when
    /// the message occupies exactly one mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementKey>,
}

/// Transactional outbox helper managing retry backoffs and mutation payloads.
pub struct OutboxManager;

impl OutboxManager {
    /// Maximum allowed retry attempts before failing permanently.
    pub const MAX_RETRIES: i32 = 5;
    /// Base backoff duration in milliseconds.
    pub const BASE_BACKOFF_MS: u64 = 500;

    /// Calculate next retry backoff delay with exponential scaling.
    pub fn calculate_backoff_ms(retry_count: i32) -> u64 {
        let exponent = (retry_count as u32).min(10);
        let factor = 2u64.pow(exponent);
        let base = Self::BASE_BACKOFF_MS * factor;
        // Pseudo-jitter using simple deterministic shift
        let jitter = (base / 4) % 100;
        base + jitter
    }

    /// Construct a new `PendingRemoteMutation` record whose target occupancy is
    /// left unrecorded. The executor may still address it when the message
    /// occupies exactly one mailbox, and refuses when it occupies several.
    pub fn create_mutation(
        rule_id: impl Into<String>,
        message_id: impl Into<String>,
        action_type: impl Into<String>,
        target: Option<String>,
    ) -> PendingRemoteMutation {
        Self::create_mutation_at(rule_id, message_id, action_type, target, None)
    }

    /// Construct a new `PendingRemoteMutation` record that names the mailbox
    /// occupancy it was formed against, so a message occupying several mailboxes
    /// is still unambiguously addressable.
    pub fn create_mutation_at(
        rule_id: impl Into<String>,
        message_id: impl Into<String>,
        action_type: impl Into<String>,
        target: Option<String>,
        placement: Option<PlacementKey>,
    ) -> PendingRemoteMutation {
        let action_str = action_type.into();
        let payload_struct = MutationPayload {
            action_type: action_str.clone(),
            target,
            placement,
        };
        let payload_json = serde_json::to_string(&payload_struct).unwrap_or_default();
        let now = chrono::Utc::now().timestamp();

        PendingRemoteMutation {
            id: format!("mut-{}", uuid_simple()),
            rule_id: rule_id.into(),
            message_id: message_id.into(),
            mutation_type: action_str,
            payload: payload_json,
            status: "pending".to_string(),
            retry_count: 0,
            created_at: now,
        }
    }
}

fn uuid_simple() -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(
        chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .to_le_bytes(),
    );
    hex::encode(&hasher.finalize()[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_calculation() {
        let delay0 = OutboxManager::calculate_backoff_ms(0);
        let delay1 = OutboxManager::calculate_backoff_ms(1);
        let delay2 = OutboxManager::calculate_backoff_ms(2);

        assert!(delay1 > delay0);
        assert!(delay2 > delay1);
    }

    #[test]
    fn test_create_mutation() {
        let mut_item = OutboxManager::create_mutation(
            "rule-1",
            "msg-100",
            "MOVE",
            Some("Archive".to_string()),
        );
        assert_eq!(mut_item.rule_id, "rule-1");
        assert_eq!(mut_item.message_id, "msg-100");
        assert_eq!(mut_item.mutation_type, "MOVE");
        assert_eq!(mut_item.status, "pending");
    }
}
