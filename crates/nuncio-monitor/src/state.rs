//! The single application state the UI reads.
//!
//! `AppState` holds no I/O and no business logic: it is the destination for
//! what [`crate::status::StatusPoller`] and [`crate::log_tail::LogTailer`]
//! produce, and the source the (future) UI layer renders from. Composing a
//! `StatusUpdate` into `AppState` is a plain field assignment, never a
//! reconstruction from a lower-fidelity signal -- in particular, `engine`
//! must always come from `StatusUpdate::engine_state` (Task 9's composite of
//! the advisory lock plus RPC reachability), never from
//! `EngineController::liveness()` directly, which cannot see a daemon that
//! holds its lock but does not answer.
//!
//! `status` and `health` stay `Option` end to end: a failed poll cycle must
//! render as "unknown" in the UI, not silently as a zeroed status. Nothing
//! in this module ever unwraps them into a default.

use std::collections::VecDeque;

use nuncio_proto::v1::{AccountConfig, AccountSyncState, GetHealthResponse, GetStatusResponse};

use crate::engine::EngineState;
use crate::log_tail::{LogAvailability, LogRecord};
use crate::status::StatusUpdate;

/// Cap on how many parsed log lines [`AppState`] retains. A debug-level sync
/// can produce a large volume of log output; without a bound the buffer
/// would grow for as long as the monitor stays open. Oldest lines are
/// dropped first once the cap is reached.
pub struct AppState {
    /// The composite engine state the UI renders (see the module docs for
    /// why this must come from [`StatusUpdate::engine_state`] rather than
    /// the raw advisory-lock signal).
    pub engine: EngineState,
    /// Most recent `GetStatus` response, or `None` if the last poll cycle
    /// could not reach the daemon.
    pub status: Option<GetStatusResponse>,
    /// Most recent `GetHealth` response, or `None` if the last poll cycle
    /// could not reach the daemon.
    pub health: Option<GetHealthResponse>,
    /// Most recently fetched account list; empty if the last poll cycle
    /// could not reach the daemon (there is no numeric field here that a
    /// real zero could be confused with).
    pub accounts: Vec<AccountConfig>,
    /// Bounded ring of parsed log lines, oldest first. Bounded by
    /// [`AppState::MAX_LOG_LINES`]; use [`AppState::push_log`] to append so
    /// the bound is enforced consistently.
    pub logs: VecDeque<LogRecord>,
    /// Current log view filter, applied by [`AppState::visible_logs`].
    pub log_filter: LogFilter,
    /// Whether the `Subscribe` push stream is currently known-good.
    pub stream_stale: bool,
    /// Whether the background log tailer currently has a usable,
    /// JSON-formatted log to read (mirrors
    /// [`crate::log_tail::LogTailer::availability`]). The UI surfaces this
    /// directly -- e.g. `NotJson` means the daemon needs
    /// `NUNCIO_LOG_FORMAT=json` before log filtering can work at all -- so a
    /// user is never left staring at a silently-empty log pane with no
    /// explanation.
    pub log_availability: LogAvailability,
    /// Most recent failure the monitor itself hit outside a poll cycle --
    /// background-runtime startup, tray creation/update, or a Start/Stop
    /// Engine command -- or `None` if nothing has failed. `tracing::error!`/
    /// `tracing::warn!` alone is not a user-visible surface in a GUI with no
    /// visible console, so every one of those call sites also routes its
    /// message here. Sticky by design: it is only ever replaced by a NEWER
    /// failure, never cleared by an unrelated successful poll cycle, so a
    /// real problem cannot scroll off screen just because `GetStatus`
    /// happened to succeed on the next tick.
    pub last_error: Option<String>,
}

impl AppState {
    /// Maximum number of log lines retained in [`AppState::logs`]. Chosen so
    /// a debug-level sync cannot exhaust memory over a long-running monitor
    /// session.
    pub const MAX_LOG_LINES: usize = 5000;

    /// Appends `record`, dropping the oldest line first if the buffer is at
    /// [`AppState::MAX_LOG_LINES`].
    pub fn push_log(&mut self, record: LogRecord) {
        self.logs.push_back(record);
        while self.logs.len() > Self::MAX_LOG_LINES {
            self.logs.pop_front();
        }
    }

    /// The subset of [`AppState::logs`] passing [`AppState::log_filter`],
    /// applying the level, free-text, and `request_id` filters in that
    /// order. Borrows rather than clones: the UI renders these references
    /// directly against the buffer's lifetime.
    pub fn visible_logs(&self) -> Vec<&LogRecord> {
        self.logs
            .iter()
            .filter(|record| self.log_filter.matches_level(record))
            .filter(|record| self.log_filter.matches_text(record))
            .filter(|record| self.log_filter.matches_request_id(record))
            .collect()
    }

    /// One row of the accounts pane, fully resolved from this snapshot of
    /// `AppState` for a single configured account. Deliberately NOT a
    /// reconstruction of the wire types: it is the join the UI needs
    /// (account identity plus this cycle's sync state and queue depth)
    /// pre-computed here so `ui.rs` never has to reason about which poll
    /// cycle a field came from.
    pub fn account_rows(&self) -> Vec<AccountRow<'_>> {
        self.accounts.iter().map(|a| self.account_row(a)).collect()
    }

    /// Resolves one [`AccountRow`] for `account`.
    ///
    /// `sync_state` is `None` whenever the whole `GetStatus` poll cycle that
    /// would carry it failed (`self.status` is `None`) -- never a stand-in
    /// for a genuine "never synced" account, which is instead
    /// `Some(&AccountSyncState)` with its own empty `last_synced`/
    /// `last_error` fields (see `GetStatusResponse::account_sync_states`'s
    /// contract: every configured account gets an entry).
    ///
    /// `pending`/`failed` are `None` together whenever the whole `GetHealth`
    /// cycle failed (`self.health` is `None`). When health data IS present,
    /// an account with no matching `AccountQueueDepth` entry gets an
    /// explicit `Some(0)` for both -- the daemon's `GetHealth` always
    /// reports one entry per configured account (see
    /// `nunciod::grpc`'s `get_health`), so a genuinely reachable daemon
    /// reporting nothing queued for this account is a real, known zero, not
    /// an unknown. This is what lets an idle account still render a row of
    /// explicit zeros rather than looking indistinguishable from one this
    /// poll cycle never heard about.
    fn account_row<'a>(&'a self, account: &'a AccountConfig) -> AccountRow<'a> {
        let sync_state = self.status.as_ref().and_then(|status| {
            status
                .account_sync_states
                .iter()
                .find(|s| s.account_id == account.id)
        });

        let (pending, failed) = match &self.health {
            None => (None, None),
            Some(health) => match health
                .account_queues
                .iter()
                .find(|q| q.account_id == account.id)
            {
                Some(queue) => (Some(queue.pending), Some(queue.failed)),
                None => (Some(0), Some(0)),
            },
        };

        AccountRow {
            account,
            sync_state,
            pending,
            failed,
        }
    }

    /// Applies one [`StatusUpdate`] cycle to this state. A plain field
    /// assignment -- the composite engine state, and the `status`/`health`
    /// options, are taken verbatim, never reinterpreted or defaulted here.
    pub fn apply_status_update(&mut self, update: StatusUpdate) {
        self.engine = update.engine_state;
        self.status = update.status;
        self.health = update.health;
        self.accounts = update.accounts;
        self.stream_stale = update.stream_stale;
    }
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            engine: EngineState::Stopped,
            status: None,
            health: None,
            accounts: Vec::new(),
            logs: VecDeque::new(),
            log_filter: LogFilter::default(),
            // No poll cycle has run yet, so the stream is not known-good --
            // matches `status.rs`'s own rule that a never-connected stream
            // must never look like a quiet, healthy one.
            stream_stale: true,
            // Matches `LogTailer::new`'s own initial value: no poll has run
            // yet, so nothing is known about the log directory.
            log_availability: LogAvailability::DirectoryMissing,
            last_error: None,
        }
    }
}

/// One resolved row of the accounts pane; see [`AppState::account_rows`].
#[derive(Debug)]
pub struct AccountRow<'a> {
    /// The account this row describes.
    pub account: &'a AccountConfig,
    /// This cycle's live sync state, or `None` if the whole `GetStatus` poll
    /// cycle that would carry it failed -- never a fabricated "idle".
    pub sync_state: Option<&'a AccountSyncState>,
    /// Pending outbox mutations for this account, or `None` if the whole
    /// `GetHealth` poll cycle failed. `Some(0)` is a genuine, known zero.
    pub pending: Option<u64>,
    /// Failed outbox mutations for this account, or `None` if the whole
    /// `GetHealth` poll cycle failed. `Some(0)` is a genuine, known zero.
    pub failed: Option<u64>,
}

/// The log view filter applied by [`AppState::visible_logs`]. Each field is
/// independently optional; a `None` field imposes no constraint.
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    /// Case-insensitive exact match against [`LogRecord::level`].
    pub level: Option<String>,
    /// Free-text substring match against [`LogRecord::message`] or
    /// [`LogRecord::raw`] (so opaque `RAW` lines remain searchable).
    pub text: Option<String>,
    /// Exact match against [`LogRecord::request_id`].
    pub request_id: Option<String>,
}

impl LogFilter {
    fn matches_level(&self, record: &LogRecord) -> bool {
        match &self.level {
            Some(level) => record.level.eq_ignore_ascii_case(level),
            None => true,
        }
    }

    fn matches_text(&self, record: &LogRecord) -> bool {
        match &self.text {
            Some(text) => {
                record.message.contains(text.as_str()) || record.raw.contains(text.as_str())
            }
            None => true,
        }
    }

    fn matches_request_id(&self, record: &LogRecord) -> bool {
        match &self.request_id {
            Some(id) => record.request_id.as_deref() == Some(id.as_str()),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec_with_request_id(id: &str) -> LogRecord {
        LogRecord {
            timestamp: "2026-08-07T12:00:00Z".to_string(),
            level: "INFO".to_string(),
            target: "nunciod".to_string(),
            request_id: Some(id.to_string()),
            message: "test".to_string(),
            raw: String::new(),
        }
    }

    fn rec_with_message(message: &str) -> LogRecord {
        LogRecord {
            timestamp: "2026-08-07T12:00:00Z".to_string(),
            level: "INFO".to_string(),
            target: "nunciod".to_string(),
            request_id: None,
            message: message.to_string(),
            raw: String::new(),
        }
    }

    #[test]
    fn filtering_by_request_id_returns_only_that_requests_lines() {
        let mut st = AppState::default();
        st.push_log(rec_with_request_id("a"));
        st.push_log(rec_with_request_id("b"));
        st.log_filter.request_id = Some("a".into());
        assert_eq!(st.visible_logs().len(), 1);
    }

    #[test]
    fn the_log_buffer_is_bounded_and_drops_oldest_first() {
        let mut st = AppState::default();
        for i in 0..(AppState::MAX_LOG_LINES + 10) {
            st.push_log(rec_with_message(&i.to_string()));
        }
        assert_eq!(st.logs.len(), AppState::MAX_LOG_LINES);
        assert_eq!(st.logs.front().map(|r| r.message.as_str()), Some("10"));
    }

    #[test]
    fn filtering_by_level_is_case_insensitive() {
        let mut st = AppState::default();
        st.push_log(rec_with_message("one"));
        st.log_filter.level = Some("info".into());
        assert_eq!(st.visible_logs().len(), 1);
        st.log_filter.level = Some("error".into());
        assert_eq!(st.visible_logs().len(), 0);
    }

    #[test]
    fn filtering_by_text_matches_message_or_raw() {
        let mut st = AppState::default();
        st.push_log(rec_with_message("hello world"));
        st.log_filter.text = Some("world".into());
        assert_eq!(st.visible_logs().len(), 1);
        st.log_filter.text = Some("nope".into());
        assert_eq!(st.visible_logs().len(), 0);
    }

    #[test]
    fn filters_compose_in_order_level_then_text_then_request_id() {
        let mut st = AppState::default();
        st.push_log(rec_with_request_id("a"));
        st.push_log(rec_with_request_id("b"));
        st.log_filter.level = Some("info".into());
        st.log_filter.text = Some("test".into());
        st.log_filter.request_id = Some("b".into());
        let visible = st.visible_logs();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].request_id.as_deref(), Some("b"));
    }

    #[test]
    fn apply_status_update_never_defaults_missing_status_or_health() {
        let mut st = AppState::default();
        st.apply_status_update(StatusUpdate {
            status: None,
            health: None,
            accounts: Vec::new(),
            stream_stale: true,
            engine_state: EngineState::NotResponding,
        });
        assert!(st.status.is_none());
        assert!(st.health.is_none());
        assert_eq!(st.engine, EngineState::NotResponding);
    }

    fn account(id: &str) -> AccountConfig {
        AccountConfig {
            id: id.to_string(),
            name: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn account_row_has_no_sync_state_when_status_poll_failed() {
        let st = AppState {
            accounts: vec![account("acct-1")],
            // `status` stays `None` (default): the whole poll cycle failed.
            ..Default::default()
        };
        let rows = st.account_rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].sync_state.is_none());
    }

    #[test]
    fn account_row_has_no_queue_depth_when_health_poll_failed() {
        let st = AppState {
            accounts: vec![account("acct-1")],
            // `health` stays `None` (default): the whole poll cycle failed.
            ..Default::default()
        };
        let rows = st.account_rows();
        assert_eq!(rows[0].pending, None);
        assert_eq!(rows[0].failed, None);
    }

    #[test]
    fn account_row_reports_explicit_zero_when_health_present_but_account_has_no_queue_entry() {
        let st = AppState {
            accounts: vec![account("acct-1")],
            health: Some(GetHealthResponse {
                account_queues: Vec::new(),
                wal_size_bytes: 0,
                db_healthy: true,
            }),
            ..Default::default()
        };
        let rows = st.account_rows();
        // Present health data with no matching entry is a genuine, known
        // zero -- distinct from the `None`/`None` case above.
        assert_eq!(rows[0].pending, Some(0));
        assert_eq!(rows[0].failed, Some(0));
    }

    #[test]
    fn account_row_reports_real_queue_depth_when_present() {
        use nuncio_proto::v1::AccountQueueDepth;

        let st = AppState {
            accounts: vec![account("acct-1")],
            health: Some(GetHealthResponse {
                account_queues: vec![AccountQueueDepth {
                    account_id: "acct-1".to_string(),
                    pending: 7,
                    failed: 2,
                }],
                wal_size_bytes: 0,
                db_healthy: true,
            }),
            ..Default::default()
        };
        let rows = st.account_rows();
        assert_eq!(rows[0].pending, Some(7));
        assert_eq!(rows[0].failed, Some(2));
    }

    #[test]
    fn account_row_finds_its_own_sync_state_among_several_accounts() {
        let st = AppState {
            accounts: vec![account("acct-1"), account("acct-2")],
            status: Some(GetStatusResponse {
                engine_status: "Ready".to_string(),
                version: "9.9.9".to_string(),
                uptime: None,
                accounts_loaded: 2,
                unread_count: 0,
                last_error: None,
                outbox_depth: 0,
                account_sync_states: vec![AccountSyncState {
                    account_id: "acct-2".to_string(),
                    state: 2, // SYNC_STATE_SYNCING
                    last_synced: None,
                    last_error: None,
                }],
                ready: true,
                db_healthy: true,
            }),
            ..Default::default()
        };

        let rows = st.account_rows();
        assert!(rows[0].sync_state.is_none());
        let acct2 = rows[1].sync_state.expect("acct-2 has a sync state entry");
        assert_eq!(acct2.state, 2);
    }

    #[test]
    fn last_error_survives_a_later_successful_poll_cycle() {
        let mut st = AppState {
            last_error: Some("failed to stop nunciod: timed out".to_string()),
            ..Default::default()
        };

        // A later, otherwise-successful poll cycle must not silently erase
        // the error the user already saw: `apply_status_update` never
        // touches `last_error`, since only a NEWER failure (or an explicit
        // future "dismiss" action, not yet implemented) should replace it.
        st.apply_status_update(StatusUpdate {
            status: Some(GetStatusResponse {
                engine_status: "Ready".to_string(),
                version: "9.9.9".to_string(),
                uptime: None,
                accounts_loaded: 0,
                unread_count: 0,
                last_error: None,
                outbox_depth: 0,
                account_sync_states: Vec::new(),
                ready: true,
                db_healthy: true,
            }),
            health: None,
            accounts: Vec::new(),
            stream_stale: false,
            engine_state: EngineState::Running,
        });

        assert_eq!(
            st.last_error,
            Some("failed to stop nunciod: timed out".to_string())
        );
    }

    #[test]
    fn every_configured_account_gets_a_row_even_with_no_poll_data_at_all() {
        let st = AppState {
            accounts: vec![account("acct-1"), account("acct-2")],
            ..Default::default()
        };
        let rows = st.account_rows();
        assert_eq!(rows.len(), 2);
    }
}
