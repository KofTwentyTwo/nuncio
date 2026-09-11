use super::Service;
use crate::{
    accounts::AccountError,
    domain::calendar::CalendarObject,
    providers::google::send::WriteResult,
    store::{
        AttemptKind, AttemptOutcome, CalendarChangePayload, CalendarChangeReceipt,
        CalendarRetryEvidence, CalendarWriteKind, StoreError,
    },
    sync_error::SyncError,
};

impl Service {
    pub(super) async fn change_calendar(
        &self,
        account: &str,
        payload: &CalendarChangePayload,
        kind: AttemptKind,
        ordinal: u32,
    ) -> Result<AttemptOutcome, StoreError> {
        let now = self.accounts.http.clock.now_ms();
        let base = (1000_i64 << ordinal.saturating_sub(1).min(8)).min(240_000);
        let retry = now.saturating_add(base + i64::from(rand::random::<u32>()) % (base / 4 + 1));
        if kind == AttemptKind::Reconcile {
            return Ok(self
                .reconcile_calendar(account, payload, ordinal, retry)
                .await);
        }
        Ok(
            match self.accounts.calendar_write(account, payload, retry).await {
                Ok(WriteResult::Acknowledged(value)) => {
                    let receipt = if payload.kind == CalendarWriteKind::Delete && value.is_null() {
                        CalendarChangeReceipt::Deleted {
                            provider_event_id: payload.provider_event_id.clone(),
                        }
                    } else {
                        CalendarChangeReceipt::Event(value)
                    };
                    if valid_receipt(payload, &receipt) {
                        AttemptOutcome::AppliedCalendar {
                            source: "acknowledgement".into(),
                            receipt,
                        }
                    } else {
                        AttemptOutcome::Uncertain {
                            code: "invalid_calendar_response".into(),
                        }
                    }
                }
                Ok(WriteResult::Uncertain { code, .. }) => {
                    AttemptOutcome::Uncertain { code: code.into() }
                }
                Ok(WriteResult::Rejected {
                    code: "calendar_precondition_failed",
                    ..
                }) => match self.read_calendar(account, payload).await {
                    Ok(observed) if valid_event(payload, &observed) => {
                        AttemptOutcome::ObservedCalendarConflict {
                            code: "calendar_precondition_failed".into(),
                            observed,
                        }
                    }
                    _ => AttemptOutcome::Conflict {
                        code: "calendar_precondition_failed".into(),
                    },
                },
                Ok(WriteResult::Rejected {
                    code: "resource_not_found",
                    ..
                }) => AttemptOutcome::Conflict {
                    code: "calendar_resource_missing".into(),
                },
                Ok(WriteResult::Rejected {
                    code,
                    retry_at_ms,
                    authorization,
                }) => AttemptOutcome::Rejected {
                    code: code.into(),
                    retry_at_ms: if ordinal < 8 || authorization {
                        retry_at_ms
                    } else {
                        None
                    },
                },
                Err(error) => AttemptOutcome::Rejected {
                    code: error.code().into(),
                    retry_at_ms: match error {
                        AccountError::RetryAfter(at) => Some(at),
                        AccountError::Authorization | AccountError::ScopeDenied => Some(retry),
                        AccountError::Unavailable | AccountError::Secret if ordinal < 8 => {
                            Some(retry)
                        }
                        _ => None,
                    },
                },
            },
        )
    }
    async fn read_calendar(
        &self,
        account: &str,
        payload: &CalendarChangePayload,
    ) -> Result<serde_json::Value, SyncError> {
        self.accounts
            .calendar_get(
                account,
                &[
                    "calendars",
                    &payload.provider_calendar_id,
                    "events",
                    &payload.provider_event_id,
                ],
                &[],
                1024 * 1024,
            )
            .await
    }
    async fn reconcile_calendar(
        &self,
        account: &str,
        payload: &CalendarChangePayload,
        ordinal: u32,
        retry: i64,
    ) -> AttemptOutcome {
        let defer = |code: &str, at: i64| {
            if ordinal < 8 {
                AttemptOutcome::ReconcileLater {
                    code: code.into(),
                    retry_at_ms: at,
                }
            } else {
                AttemptOutcome::Uncertain { code: code.into() }
            }
        };
        match self.read_calendar(account, payload).await {
            Ok(value) if valid_event(payload, &value) => {
                let receipt = CalendarChangeReceipt::Event(value.clone());
                if payload.satisfied_by(&receipt) {
                    return AttemptOutcome::AppliedCalendar {
                        source: "positive_read".into(),
                        receipt,
                    };
                }
                let evidence = CalendarRetryEvidence::Unchanged(value.clone());
                if payload.permits_retry(&evidence) && ordinal < 8 {
                    return AttemptOutcome::RepeatableCalendar {
                        code: "unchanged_remote_version".into(),
                        retry_at_ms: retry,
                        evidence,
                    };
                }
                AttemptOutcome::ObservedCalendarConflict {
                    code: "calendar_remote_changed".into(),
                    observed: value,
                }
            }
            Err(SyncError::NotFound) => match payload.kind {
                CalendarWriteKind::Create if ordinal < 8 => AttemptOutcome::RepeatableCalendar {
                    code: "stable_create_id_absent".into(),
                    retry_at_ms: retry,
                    evidence: CalendarRetryEvidence::Missing {
                        provider_event_id: payload.provider_event_id.clone(),
                    },
                },
                CalendarWriteKind::Delete => AttemptOutcome::AppliedCalendar {
                    source: "positive_read".into(),
                    receipt: CalendarChangeReceipt::Deleted {
                        provider_event_id: payload.provider_event_id.clone(),
                    },
                },
                _ => AttemptOutcome::Conflict {
                    code: "calendar_resource_missing".into(),
                },
            },
            Err(SyncError::RetryAfter(at)) => defer("calendar_reconciliation_delayed", at),
            _ => defer("calendar_reconciliation_unavailable", retry),
        }
    }
}
fn valid_event(payload: &CalendarChangePayload, value: &serde_json::Value) -> bool {
    value["id"] == payload.provider_event_id
        && CalendarObject::from_google(value.clone(), &payload.time_zone).is_ok()
}
fn valid_receipt(payload: &CalendarChangePayload, receipt: &CalendarChangeReceipt) -> bool {
    payload.satisfied_by(receipt)
        && match receipt {
            CalendarChangeReceipt::Event(value) => valid_event(payload, value),
            CalendarChangeReceipt::Deleted { .. } => true,
        }
}
