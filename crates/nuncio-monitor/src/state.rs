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

use nuncio_proto::v1::{AccountConfig, GetHealthResponse, GetStatusResponse};

use crate::engine::EngineState;
use crate::log_tail::LogRecord;
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
        }
    }
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
}
