//! Pure domain entities owned by Nuncio (Email, Attachment, Folder, CalendarEvent, Contact, DaemonTelemetry).

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Real-time daemon health, performance metrics, and telemetry payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonTelemetry {
    /// Overall daemon status ("HEALTHY", "DEGRADED", "RECOVERING", "SYNCING").
    pub status: String,
    /// Operating system Process ID.
    pub pid: u32,
    /// Daemon uptime in seconds.
    pub uptime_seconds: u64,
    /// Memory RSS usage in bytes.
    pub memory_rss_bytes: u64,
    /// Main SQLite database file size in bytes.
    pub db_size_bytes: u64,
    /// SQLite Write-Ahead Log (.db-wal) size in bytes.
    pub wal_size_bytes: u64,
    /// SQLite FTS5 trigram index size in bytes.
    pub fts_index_size_bytes: u64,
    /// Total index email messages.
    pub total_emails: u64,
    /// Total index calendar events.
    pub total_events: u64,
    /// Database integrity status ("OK" or "CORRUPTED").
    pub integrity_status: String,
    /// Active IPC socket client connections.
    pub active_ipc_clients: usize,
    /// Active background IMAP IDLE connection count.
    pub active_imap_idle_connections: usize,
    /// Active SMTP delivery connection count.
    pub active_smtp_connections: usize,
    /// Outbox job queue depth.
    pub pending_outbox_jobs: usize,
    /// Total NSQL filter rules executed.
    pub filter_rules_executed: u64,
    /// Total NSQL filter rules matched.
    pub filter_rules_matched: u64,
    /// Total external protocol API calls executed.
    pub api_calls_total: u64,
    /// Total external protocol API errors encountered.
    pub api_errors_total: u64,
    /// Average protocol API call latency in milliseconds.
    pub avg_api_latency_ms: f64,
    /// Circular log stream buffer entries (last 50 log lines).
    pub recent_logs: Vec<String>,
}

/// Email attachment metadata and payload buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Attachment filename.
    pub filename: String,
    /// MIME content type (e.g. "application/pdf").
    pub mime_type: String,
    /// Raw attachment content buffer.
    #[serde(skip)]
    pub content: Bytes,
}

/// Email message domain entity owned by Nuncio core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Email {
    /// Opaque, deterministic surrogate identifier for the message.
    ///
    /// Derived from [`Email::surrogate_id`] over the account, folder,
    /// UIDVALIDITY, and protocol-native id. It carries no decodable internal
    /// encoding (callers must treat it as opaque) and is stable across
    /// re-syncs of the same message, so persisting it upserts the same row
    /// rather than duplicating. Distinct messages that happen to share a
    /// protocol id across folders or accounts hash to distinct surrogates, so
    /// one can never silently overwrite another.
    pub id: String,
    /// Account identifier owning the message.
    pub account_id: String,
    /// Mailbox folder identifier (e.g. "inbox").
    pub folder_id: String,
    /// Protocol-native message id used to address the message on its server:
    /// the IMAP UID as a decimal string, or the JMAP Email object id. This is
    /// what a remote mutation must use to identify the message on the wire --
    /// never the opaque [`Email::id`].
    pub remote_id: String,
    /// The addressing scope the `remote_id` is only meaningful within: the IMAP
    /// folder UIDVALIDITY as a decimal string. Protocols without a UIDVALIDITY
    /// (JMAP) carry a stable sentinel instead, so the surrogate id stays
    /// deterministic.
    pub uid_validity: String,
    /// Subject line.
    pub subject: String,
    /// Sender address (e.g. "alice@nuncio.mx").
    pub sender: String,
    /// Recipient address (e.g. "bob@nuncio.mx").
    pub recipient: String,
    /// Unix timestamp of message arrival.
    pub received_at: i64,
    /// Read/unread flag status.
    pub read: bool,
    /// Plaintext message body.
    pub body_plain: Option<String>,
    /// HTML message body.
    pub body_html: Option<String>,
    /// List of attached files.
    pub attachments: Vec<Attachment>,
    /// The message's RFC 5322 `Message-ID`, normalized by
    /// [`Email::normalize_message_id`], or `None` when the message carried
    /// none (the header is `SHOULD`, not `MUST`) or the transport did not
    /// surface it.
    ///
    /// Captured but not yet used for identity. It is a *hint* toward a
    /// folder-independent identity, never an identity on its own: the value is
    /// public (it is echoed in the `References` of every reply) and forgeable,
    /// so keying storage on it alone would let a crafted message address, and
    /// overwrite, one the user already trusts.
    pub message_id: Option<String>,
    /// Hex SHA-256 over the full RFC822 octets the message was parsed from, or
    /// `None` when only headers or an envelope were available.
    ///
    /// Pairs with [`Email::message_id`]: the same `Message-ID` carrying
    /// different content is two messages, not one. That case is ordinary --
    /// a mailing-list copy and a direct copy of the same mail share a
    /// `Message-ID` while differing in headers and footer, and a draft keeps
    /// one `Message-ID` across every save.
    pub content_hash: Option<String>,
}

impl Email {
    /// Compute the deterministic, collision-resistant surrogate id for a
    /// message from its addressing coordinates.
    ///
    /// The four coordinates together uniquely identify a stored message: a
    /// protocol-native `remote_id` (IMAP UID / JMAP object id) is only unique
    /// within one `uid_validity` scope of one `folder_id` of one `account_id`,
    /// so all four must participate. Each field is fed length-prefixed so that
    /// no two distinct coordinate tuples can serialize to the same byte stream
    /// (e.g. `("a", "bc")` and `("ab", "c")` must not collide). The SHA-256
    /// digest is hex-encoded, yielding a stable opaque token that leaks none of
    /// its inputs' encoding on the wire.
    pub fn surrogate_id(
        account_id: &str,
        folder_id: &str,
        uid_validity: &str,
        remote_id: &str,
    ) -> String {
        let mut hasher = Sha256::new();
        for field in [account_id, folder_id, uid_validity, remote_id] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    /// Canonicalize a raw `Message-ID` header value.
    ///
    /// Every engine syncing an account must derive byte-identical values from
    /// the same header, or two caches that agree in substance will disagree in
    /// bytes and no comparison between them can be trusted. The rules are
    /// therefore fixed and deliberately small:
    ///
    /// 1. Remove **all** ASCII whitespace. A `msg-id` cannot legally contain
    ///    any, so this unfolds a wrapped header without needing to know how the
    ///    transport folded it -- and transports differ.
    /// 2. Strip one surrounding pair of angle brackets, which servers include
    ///    or omit inconsistently between `ENVELOPE` and a raw header.
    /// 3. Preserve case. Only the domain half is case-insensitive per RFC 5322
    ///    §3.6.4; lowercasing the whole value would merge ids that the
    ///    originating server considers distinct, and the same server returns
    ///    the same bytes to every engine anyway.
    ///
    /// Returns `None` for an absent or empty value so a missing header stays
    /// distinguishable from a present-but-empty one.
    pub fn normalize_message_id(raw: &str) -> Option<String> {
        let stripped: String = raw.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let stripped = stripped
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .unwrap_or(&stripped);
        if stripped.is_empty() {
            None
        } else {
            Some(stripped.to_string())
        }
    }

    /// Hex SHA-256 over a message's full RFC822 octets, for the
    /// `content_hash` field.
    ///
    /// Deterministic for identical bytes, which is what makes it usable as the
    /// tiebreaker against a duplicated `Message-ID`. It is *not* stable across
    /// servers that re-serialize a message, so it distinguishes content within
    /// an account rather than identifying a message globally.
    pub fn content_hash_of(raw_bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw_bytes);
        hex::encode(hasher.finalize())
    }
}

/// Mailbox folder entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// Folder identifier.
    pub id: String,
    /// Folder display name.
    pub name: String,
    /// Total message count.
    pub total_messages: usize,
    /// Unread message count.
    pub unread_messages: usize,
}

/// Calendar event domain entity owned by Nuncio core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Event identifier.
    pub id: String,
    /// Account identifier owning the event.
    pub account_id: String,
    /// Calendar collection identifier.
    pub calendar_id: String,
    /// Event title or summary.
    pub summary: String,
    /// Unix timestamp for event start.
    pub start_time: i64,
    /// Unix timestamp for event end.
    pub end_time: i64,
    /// Recurrence rule string (RFC 5545 RRULE format).
    pub rrule: Option<String>,
    /// Location text.
    pub location: Option<String>,
}

/// Contact card entity owned by Nuncio core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// Contact identifier.
    pub id: String,
    /// Full display name.
    pub name: String,
    /// Primary email address.
    pub email: String,
    /// Phone number.
    pub phone: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_telemetry_creation_and_serde() {
        let telemetry = DaemonTelemetry {
            status: "HEALTHY".to_string(),
            pid: 12345,
            uptime_seconds: 3600,
            memory_rss_bytes: 42000000,
            db_size_bytes: 1048576,
            wal_size_bytes: 32768,
            fts_index_size_bytes: 524288,
            total_emails: 15000,
            total_events: 340,
            integrity_status: "OK".to_string(),
            active_ipc_clients: 2,
            active_imap_idle_connections: 4,
            active_smtp_connections: 0,
            pending_outbox_jobs: 0,
            filter_rules_executed: 1420,
            filter_rules_matched: 88,
            api_calls_total: 9500,
            api_errors_total: 0,
            avg_api_latency_ms: 14.2,
            recent_logs: vec!["[INFO] nunciod initialized".to_string()],
        };

        let json = serde_json::to_string(&telemetry).unwrap();
        let parsed: DaemonTelemetry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, "HEALTHY");
        assert_eq!(parsed.total_emails, 15000);
    }

    #[test]
    fn surrogate_id_is_deterministic_and_opaque() {
        let a = Email::surrogate_id("acct-1", "INBOX", "42", "5");
        let b = Email::surrogate_id("acct-1", "INBOX", "42", "5");
        assert_eq!(a, b, "identical coordinates must hash identically");
        // A 32-byte SHA-256 digest hex-encodes to 64 chars, and the token must
        // not leak the protocol-native id it was built from.
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!a.contains("imap-uid-"));
    }

    #[test]
    fn surrogate_id_distinguishes_the_cross_folder_and_cross_account_collision() {
        // The same protocol UID (5) in two different folders of one account, or
        // in two different accounts, must NOT collapse to the same surrogate.
        let inbox = Email::surrogate_id("acct-1", "INBOX", "42", "5");
        let sent = Email::surrogate_id("acct-1", "Sent", "7", "5");
        let other_account = Email::surrogate_id("acct-2", "INBOX", "42", "5");
        assert_ne!(inbox, sent, "same UID in two folders must not collide");
        assert_ne!(
            inbox, other_account,
            "same UID in two accounts must not collide"
        );

        // Field boundaries are unambiguous: shifting a delimiter-like split
        // across two adjacent fields yields a different id.
        assert_ne!(
            Email::surrogate_id("a", "bc", "1", "1"),
            Email::surrogate_id("ab", "c", "1", "1"),
        );
    }

    #[test]
    fn normalize_message_id_unfolds_and_unwraps_to_one_canonical_form() {
        // Every spelling a transport might hand us for the same id must
        // collapse to identical bytes, or two engines holding the same message
        // cannot be shown to agree.
        let canonical = "abc123@mail.example.com";
        for raw in [
            "<abc123@mail.example.com>",
            "abc123@mail.example.com",
            "  <abc123@mail.example.com>  ",
            "<abc123@mail.\r\n example.com>",
            "<abc123@mail.\t example.com>",
        ] {
            assert_eq!(
                Email::normalize_message_id(raw).as_deref(),
                Some(canonical),
                "input {raw:?} did not canonicalize"
            );
        }
    }

    #[test]
    fn normalize_message_id_preserves_case_and_rejects_empty() {
        // Only the domain half is case-insensitive per RFC 5322 3.6.4;
        // lowercasing wholesale would merge ids the origin considers distinct.
        assert_eq!(
            Email::normalize_message_id("<AbC@Example.COM>").as_deref(),
            Some("AbC@Example.COM")
        );
        assert_eq!(Email::normalize_message_id(""), None);
        assert_eq!(Email::normalize_message_id("<>"), None);
        assert_eq!(Email::normalize_message_id("   "), None);
    }

    #[test]
    fn content_hash_distinguishes_identical_message_ids_with_different_bodies() {
        // The case the hash exists for: a list copy and a direct copy share a
        // Message-ID but are not the same message.
        let direct = b"Message-ID: <shared@example.com>\r\n\r\nbody";
        let via_list = b"Message-ID: <shared@example.com>\r\nList-Id: l\r\n\r\nbody\n--\nfooter";
        assert_ne!(
            Email::content_hash_of(direct),
            Email::content_hash_of(via_list)
        );
        assert_eq!(
            Email::content_hash_of(direct),
            Email::content_hash_of(direct),
            "the hash must be deterministic for identical octets"
        );
    }
}
