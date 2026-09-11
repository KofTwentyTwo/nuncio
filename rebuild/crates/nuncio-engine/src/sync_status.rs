use crate::store::SyncSchedule;
use serde::Serialize;

#[derive(Serialize)]
pub struct SyncScopeStatus {
    pub account_id: String,
    pub scope: String,
    pub phase: String,
    pub run_id: Option<String>,
    pub processed: u64,
    pub last_success_at_ms: Option<i64>,
    pub age_ms: Option<u64>,
    pub next_attempt_at_ms: Option<i64>,
    pub error_code: Option<String>,
    pub coverage_state: String,
}
impl SyncScopeStatus {
    pub(crate) fn from_schedule(s: SyncSchedule, now: i64, poll: u64, enabled: bool) -> Self {
        let age = s
            .last_success_at_ms
            .map(|at| now.saturating_sub(at).max(0) as u64);
        let phase = if s.account_state != "connected" {
            "paused"
        } else if matches!(s.run_state.as_deref(), Some("queued" | "running")) {
            s.run_state.as_deref().unwrap_or("queued")
        } else if !enabled {
            "manual"
        } else if s.provider_retry_after_ms.is_some_and(|at| at > now)
            || s.error_code.as_deref().is_some_and(|e| e != "cancelled")
                && s.next_attempt_at_ms > now
        {
            "backoff"
        } else {
            "waiting"
        }
        .to_owned();
        let coverage = if age.is_none() {
            "unavailable"
        } else if age.is_some_and(|age| age > poll.saturating_mul(2)) || s.error_code.is_some() {
            "stale"
        } else {
            "current"
        }
        .to_owned();
        let next = if enabled
            && s.account_state == "connected"
            && !matches!(s.run_state.as_deref(), Some("queued" | "running"))
        {
            Some(s.next_attempt_at_ms.max(now))
        } else {
            None
        };
        Self {
            account_id: s.account_id,
            scope: s.scope,
            phase,
            run_id: s.last_run_id,
            processed: s.processed,
            last_success_at_ms: s.last_success_at_ms,
            age_ms: age,
            next_attempt_at_ms: next,
            error_code: s.error_code,
            coverage_state: coverage,
        }
    }
}
