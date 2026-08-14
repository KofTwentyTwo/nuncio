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

/// Which tier of the identity precedence produced a [`MessageIdentity`].
///
/// Persisted alongside the key so a later pass can tell a server-assigned
/// identity from a locally-derived fallback without recomputing it, and so a
/// store can be audited for how much of it rests on the weakest tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentitySource {
    /// RFC 8474 OBJECTID `EMAILID` -- server-assigned and stable across folders.
    EmailId,
    /// Gmail's `X-GM-MSGID`, the same guarantee by a vendor extension.
    GmailMsgId,
    /// `Message-ID` **and** a content hash together. Never the header alone.
    MessageIdContent,
    /// Folder-scoped last resort: the message is only identified by where it
    /// currently sits, so the same mail in another folder is a second message.
    Surrogate,
}

impl IdentitySource {
    /// Stable token persisted in `messages.identity_source`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EmailId => "emailid",
            Self::GmailMsgId => "gmsgid",
            Self::MessageIdContent => "msgid_content",
            Self::Surrogate => "surrogate",
        }
    }

    /// Parse a token written by [`Self::as_str`]. Returns `None` for anything
    /// else so an unrecognised value fails loudly rather than defaulting to a
    /// tier that would overstate how trustworthy the key is.
    ///
    /// Deliberately not `std::str::FromStr`: that trait forces a `Result` and
    /// therefore an error type, when the only failure here is "not one of four
    /// known tokens" and `None` already says exactly that.
    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "emailid" => Some(Self::EmailId),
            "gmsgid" => Some(Self::GmailMsgId),
            "msgid_content" => Some(Self::MessageIdContent),
            "surrogate" => Some(Self::Surrogate),
            _ => None,
        }
    }
}

/// A content-addressed message key together with the tier that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageIdentity {
    /// Hex SHA-256 key identifying the message within its account.
    pub key: String,
    /// Which precedence tier produced `key`.
    pub source: IdentitySource,
}

/// Server-supplied identity hints for one message, in precedence order.
///
/// Borrowed rather than owned because every caller already holds these as
/// fields on a parsed response and only needs them for the duration of the
/// key derivation.
#[derive(Debug, Clone, Copy, Default)]
pub struct RemoteIdentity<'a> {
    /// RFC 8474 `EMAILID` from a `FETCH ... (EMAILID)`, if the server has OBJECTID.
    pub email_id: Option<&'a str>,
    /// Gmail `X-GM-MSGID`, if the server advertises `X-GM-EXT-1`.
    pub gm_msgid: Option<&'a str>,
    /// Normalized `Message-ID` (see [`Email::normalize_message_id`]).
    pub message_id: Option<&'a str>,
    /// Hex SHA-256 over the full RFC822 octets (see [`Email::content_hash_of`]).
    pub content_hash: Option<&'a str>,
}

/// One occupancy of a message in one mailbox.
///
/// A message can sit in several folders at once (an IMAP `COPY`, a Gmail label,
/// a message that is both in a thread's folder and in `\All`). The addressing
/// coordinates and the read flag are properties of the *occupancy*, not of the
/// message: IMAP `\Seen` is per-mailbox, and a UID is only meaningful inside one
/// `uid_validity` scope of one folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// Account owning the mailbox.
    pub account_id: String,
    /// Mailbox folder identifier (e.g. "inbox").
    pub folder_id: String,
    /// The IMAP UIDVALIDITY the `remote_id` was captured under, as a decimal
    /// string; a stable sentinel for protocols without one (JMAP). In the
    /// primary key because UIDs restart at 1 after a bump and would otherwise
    /// collide with rows written before it.
    pub uid_validity: String,
    /// Protocol-native id addressing the message in this mailbox: the IMAP UID
    /// as a decimal string, or the JMAP Email object id.
    pub remote_id: String,
    /// Read/unread state **in this mailbox**.
    pub read: bool,
}

/// The four coordinates that address one mailbox occupancy.
///
/// The addressing half of a [`Placement`], without the read flag: enough to
/// name an occupancy, not enough to describe it. A protocol backend reports
/// these when a folder stops mentioning a message, and the store deletes
/// exactly the rows they name -- which is why the type lives here in `core`
/// rather than in either crate. What a folder stops reporting is an occupancy,
/// never a message; a message goes only when its last placement does.
///
/// `Ord` is derived because the sync path sorts and dedups batches of these
/// before deleting; `Hash`/`Eq` because it is also the key of the
/// already-present set used to classify a fetched chunk.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlacementKey {
    /// Account owning the mailbox.
    pub account_id: String,
    /// Mailbox folder identifier.
    pub folder_id: String,
    /// The UIDVALIDITY generation `remote_id` was captured under.
    pub uid_validity: String,
    /// Protocol-native id addressing the message in this mailbox.
    pub remote_id: String,
}

impl Placement {
    /// The addressing coordinates of this occupancy, dropping the read flag.
    pub fn key(&self) -> PlacementKey {
        PlacementKey {
            account_id: self.account_id.clone(),
            folder_id: self.folder_id.clone(),
            uid_validity: self.uid_validity.clone(),
            remote_id: self.remote_id.clone(),
        }
    }
}

/// Email message domain entity owned by Nuncio core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Email {
    /// Opaque, account-scoped identity for the message itself.
    ///
    /// Derived by [`Email::derive_message_key`], so it is independent of which
    /// folder the message currently occupies: moving a message between folders
    /// does not change it. Where the message actually sits, and its read state
    /// there, live in [`Placement`] rows instead. Callers must treat this as an
    /// opaque token -- it carries no decodable structure.
    pub id: String,
    /// Account identifier owning the message.
    pub account_id: String,
    /// Subject line.
    pub subject: String,
    /// Sender address (e.g. "alice@nuncio.mx").
    pub sender: String,
    /// Recipient address (e.g. "bob@nuncio.mx").
    pub recipient: String,
    /// Unix timestamp of message arrival.
    pub received_at: i64,
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

    /// Domain-separated, length-prefixed digest over an identity tier's inputs.
    ///
    /// The tier label participates in the hash so that a value appearing in two
    /// different tiers (a server whose `EMAILID` happens to equal another's
    /// `X-GM-MSGID`) cannot produce one key. Length prefixes stop two distinct
    /// field tuples serializing to the same byte stream, exactly as in
    /// [`Email::surrogate_id`].
    fn identity_digest(tier: &str, account_id: &str, parts: &[&str]) -> String {
        let mut hasher = Sha256::new();
        for field in [tier, account_id].into_iter().chain(parts.iter().copied()) {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    /// Derive the account-scoped message key by identity precedence.
    ///
    /// `EMAILID` → `X-GM-MSGID` → (`Message-ID` + content hash) → surrogate.
    /// Every tier is account-scoped: identity is only ever claimed within the
    /// account that fetched it, so one account's server cannot mint a key that
    /// addresses another account's mail.
    ///
    /// The third tier requires **both** a `Message-ID` and a content hash. A
    /// `Message-ID` is public -- it is echoed in the `References` of every reply
    /// -- and forgeable, so keying on it alone would let a crafted message
    /// address, and with an upsert overwrite, a message the user already trusts,
    /// and would evade filtering by never being classified as new. Pairing it
    /// with the content hash also gets the ordinary cases right: a mailing-list
    /// copy and a direct copy share a `Message-ID` but differ in content, and so
    /// are correctly two messages.
    ///
    /// The surrogate tier is folder-scoped and therefore *not* stable across a
    /// move; it is the honest answer when the server offered nothing better.
    ///
    /// # Keys do not yet converge across engine types
    ///
    /// Precedence makes a key reproducible from the same inputs, not from the
    /// same *message*. The IMAP engine does not currently request `EMAILID` or
    /// `X-GM-MSGID`, so it can only ever reach the `Message-ID`+content tier or
    /// the surrogate, while JMAP supplies a server id and enters at the top
    /// tier. Two engines syncing one account therefore produce disjoint key
    /// spaces: the same message is two identities, one per engine. This is a
    /// known gap in the convergent multi-engine model, not a property to rely
    /// on -- do not assume a key derived by one engine addresses the row
    /// another engine wrote.
    pub fn derive_message_key(
        account_id: &str,
        remote: RemoteIdentity<'_>,
        folder_id: &str,
        uid_validity: &str,
        remote_id: &str,
    ) -> MessageIdentity {
        if let Some(email_id) = remote.email_id.filter(|v| !v.is_empty()) {
            return MessageIdentity {
                key: Self::identity_digest("emailid", account_id, &[email_id]),
                source: IdentitySource::EmailId,
            };
        }
        if let Some(gm_msgid) = remote.gm_msgid.filter(|v| !v.is_empty()) {
            return MessageIdentity {
                key: Self::identity_digest("gmsgid", account_id, &[gm_msgid]),
                source: IdentitySource::GmailMsgId,
            };
        }
        if let (Some(message_id), Some(content_hash)) = (
            remote.message_id.filter(|v| !v.is_empty()),
            remote.content_hash.filter(|v| !v.is_empty()),
        ) {
            return MessageIdentity {
                key: Self::identity_digest(
                    "msgid_content",
                    account_id,
                    &[message_id, content_hash],
                ),
                source: IdentitySource::MessageIdContent,
            };
        }
        MessageIdentity {
            key: Self::identity_digest(
                "surrogate",
                account_id,
                &[folder_id, uid_validity, remote_id],
            ),
            source: IdentitySource::Surrogate,
        }
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

    #[test]
    fn message_key_precedence_prefers_emailid_over_everything_below_it() {
        let remote = RemoteIdentity {
            email_id: Some("M00000001"),
            gm_msgid: Some("1234567890"),
            message_id: Some("a@b.example"),
            content_hash: Some("deadbeef"),
        };
        let id = Email::derive_message_key("acct-1", remote, "INBOX", "42", "5");
        assert_eq!(id.source, IdentitySource::EmailId);

        // Same EMAILID in a different folder is the SAME message.
        let elsewhere = Email::derive_message_key(
            "acct-1",
            RemoteIdentity {
                email_id: Some("M00000001"),
                gm_msgid: None,
                message_id: None,
                content_hash: None,
            },
            "Archive",
            "99",
            "7",
        );
        assert_eq!(id.key, elsewhere.key);
    }

    #[test]
    fn message_key_is_account_scoped_at_every_tier() {
        let emailid = || RemoteIdentity {
            email_id: Some("M00000001"),
            gm_msgid: None,
            message_id: None,
            content_hash: None,
        };
        let a = Email::derive_message_key("acct-1", emailid(), "INBOX", "42", "5");
        let b = Email::derive_message_key("acct-2", emailid(), "INBOX", "42", "5");
        assert_ne!(
            a.key, b.key,
            "the same EMAILID in two accounts must not collide"
        );

        let gmsgid = || RemoteIdentity {
            email_id: None,
            gm_msgid: Some("1234567890"),
            message_id: None,
            content_hash: None,
        };
        let a = Email::derive_message_key("acct-1", gmsgid(), "INBOX", "42", "5");
        let b = Email::derive_message_key("acct-2", gmsgid(), "INBOX", "42", "5");
        assert_ne!(
            a.key, b.key,
            "the same X-GM-MSGID in two accounts must not collide"
        );

        let msgid_content = || RemoteIdentity {
            email_id: None,
            gm_msgid: None,
            message_id: Some("a@b.example"),
            content_hash: Some("deadbeef"),
        };
        let a = Email::derive_message_key("acct-1", msgid_content(), "INBOX", "42", "5");
        let b = Email::derive_message_key("acct-2", msgid_content(), "INBOX", "42", "5");
        assert_ne!(
            a.key, b.key,
            "the same Message-ID and content hash in two accounts must not collide"
        );

        let surrogate = RemoteIdentity {
            email_id: None,
            gm_msgid: None,
            message_id: None,
            content_hash: None,
        };
        let a = Email::derive_message_key("acct-1", surrogate, "INBOX", "42", "5");
        let b = Email::derive_message_key("acct-2", surrogate, "INBOX", "42", "5");
        assert_ne!(
            a.key, b.key,
            "the same folder/uidvalidity/remote-id in two accounts must not collide"
        );
    }

    #[test]
    fn a_message_id_without_a_content_hash_never_reaches_the_content_tier() {
        // The non-negotiable: a Message-ID is public and forgeable, so it must
        // never key storage on its own. Missing content hash falls through to the
        // folder-scoped surrogate rather than trusting the header alone.
        let id = Email::derive_message_key(
            "acct-1",
            RemoteIdentity {
                email_id: None,
                gm_msgid: None,
                message_id: Some("a@b.example"),
                content_hash: None,
            },
            "INBOX",
            "42",
            "5",
        );
        assert_eq!(id.source, IdentitySource::Surrogate);

        let surrogate_only = Email::derive_message_key(
            "acct-1",
            RemoteIdentity {
                email_id: None,
                gm_msgid: None,
                message_id: None,
                content_hash: None,
            },
            "INBOX",
            "42",
            "5",
        );
        assert_eq!(id.key, surrogate_only.key);
    }

    #[test]
    fn the_same_message_id_with_different_content_is_two_messages() {
        let mk = |hash: &str| {
            Email::derive_message_key(
                "acct-1",
                RemoteIdentity {
                    email_id: None,
                    gm_msgid: None,
                    message_id: Some("a@b.example"),
                    content_hash: Some(hash),
                },
                "INBOX",
                "42",
                "5",
            )
        };
        let list_copy = mk("1111");
        let direct_copy = mk("2222");
        assert_eq!(list_copy.source, IdentitySource::MessageIdContent);
        assert_ne!(list_copy.key, direct_copy.key);
    }

    #[test]
    fn tiers_are_domain_separated_so_a_shared_value_cannot_collide_across_them() {
        let as_emailid = Email::derive_message_key(
            "acct-1",
            RemoteIdentity {
                email_id: Some("X"),
                gm_msgid: None,
                message_id: None,
                content_hash: None,
            },
            "INBOX",
            "42",
            "5",
        );
        let as_gmsgid = Email::derive_message_key(
            "acct-1",
            RemoteIdentity {
                email_id: None,
                gm_msgid: Some("X"),
                message_id: None,
                content_hash: None,
            },
            "INBOX",
            "42",
            "5",
        );
        assert_ne!(as_emailid.key, as_gmsgid.key);
    }

    fn sample_placement() -> Placement {
        Placement {
            account_id: "acct-1".to_string(),
            folder_id: "INBOX".to_string(),
            uid_validity: "42".to_string(),
            remote_id: "5".to_string(),
            read: true,
        }
    }

    #[test]
    fn placement_key_carries_every_addressing_coordinate_and_drops_the_read_flag() {
        // The key must reproduce all four coordinates exactly. A key that
        // dropped or reordered one would address a different occupancy, and a
        // sync diffing reported-against-stored would delete live mail.
        let placement = sample_placement();
        let key = placement.key();
        assert_eq!(key.account_id, "acct-1");
        assert_eq!(key.folder_id, "INBOX");
        assert_eq!(key.uid_validity, "42");
        assert_eq!(key.remote_id, "5");

        // The read flag is the one field a key must NOT carry: it describes an
        // occupancy rather than naming it, so including it would make the same
        // occupancy compare unequal to itself after the user opened the message.
        let mut unread = placement.clone();
        unread.read = false;
        assert_ne!(placement, unread);
        assert_eq!(placement.key(), unread.key());
    }

    #[test]
    fn placement_keys_distinguish_every_coordinate_that_can_differ() {
        let base = sample_placement().key();
        for mutated in [
            PlacementKey {
                account_id: "acct-2".to_string(),
                ..base.clone()
            },
            PlacementKey {
                folder_id: "Archive".to_string(),
                ..base.clone()
            },
            PlacementKey {
                uid_validity: "43".to_string(),
                ..base.clone()
            },
            PlacementKey {
                remote_id: "6".to_string(),
                ..base.clone()
            },
        ] {
            assert_ne!(
                base, mutated,
                "changing one coordinate must yield a different occupancy"
            );
        }

        // Usable as a set key and sortable, which is what the sync path needs
        // to diff a reported UID set against what the store holds.
        let mut set = std::collections::HashSet::new();
        assert!(set.insert(base.clone()));
        assert!(!set.insert(base.clone()), "an equal key must not re-insert");
        let mut keys = [
            PlacementKey {
                remote_id: "6".to_string(),
                ..base.clone()
            },
            base.clone(),
        ];
        keys.sort();
        assert_eq!(keys[0], base);
    }

    #[test]
    fn placement_key_round_trips_through_serde() {
        let key = sample_placement().key();
        let json = serde_json::to_string(&key).unwrap();
        let parsed: PlacementKey = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn placement_round_trips_through_serde_including_its_read_flag() {
        let placement = sample_placement();
        let json = serde_json::to_string(&placement).unwrap();
        let parsed: Placement = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, placement);
        assert!(parsed.read);
    }

    #[test]
    fn identity_source_round_trips_through_its_stored_string() {
        for source in [
            IdentitySource::EmailId,
            IdentitySource::GmailMsgId,
            IdentitySource::MessageIdContent,
            IdentitySource::Surrogate,
        ] {
            assert_eq!(IdentitySource::from_token(source.as_str()), Some(source));
        }
        assert_eq!(
            IdentitySource::from_token("not_a_tier"),
            None,
            "an unrecognised token must fail loudly, never default to a tier"
        );
    }
}
