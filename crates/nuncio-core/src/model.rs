//! Pure domain entities owned by Nuncio (Email, Attachment, Folder, CalendarEvent, Contact, DaemonTelemetry).

use bytes::Bytes;
use serde::{Deserialize, Serialize};

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
    /// Unique message identifier.
    pub id: String,
    /// Account identifier owning the message.
    pub account_id: String,
    /// Mailbox folder identifier (e.g. "inbox").
    pub folder_id: String,
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
}
