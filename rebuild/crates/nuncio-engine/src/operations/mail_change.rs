use super::Service;
use crate::{
    accounts::AccountError,
    providers::google::send::WriteResult,
    store::{AttemptKind, AttemptOutcome, MailChangePayload, MailChangeReceipt, StoreError},
};

impl Service {
    pub(super) async fn change(
        &self,
        account: &str,
        payload: &MailChangePayload,
        kind: AttemptKind,
        ordinal: u32,
    ) -> Result<AttemptOutcome, StoreError> {
        let now = self.accounts.http.clock.now_ms();
        let base = (1000_i64 << ordinal.saturating_sub(1).min(8)).min(240_000);
        let retry = now.saturating_add(base + i64::from(rand::random::<u32>()) % (base / 4 + 1));
        let repeat = |code: &str, at: i64| {
            if ordinal < 8 {
                AttemptOutcome::Repeatable {
                    code: code.into(),
                    retry_at_ms: at,
                }
            } else {
                AttemptOutcome::Conflict {
                    code: "mail_change_unconfirmed".into(),
                }
            }
        };
        if kind == AttemptKind::Reconcile {
            return Ok(
                match self
                    .accounts
                    .gmail_get(
                        account,
                        &["messages", &payload.provider_message_id],
                        &[("format", "minimal")],
                        262144,
                    )
                    .await
                {
                    Ok(value) => match MailChangeReceipt::from_google(&value) {
                        Ok(message) if payload.satisfied_by(&message) => {
                            AttemptOutcome::AppliedMail {
                                source: "positive_read".into(),
                                message,
                            }
                        }
                        _ => repeat("desired_state_not_observed", retry),
                    },
                    Err(crate::sync_error::SyncError::NotFound) => AttemptOutcome::Conflict {
                        code: "remote_message_missing".into(),
                    },
                    Err(crate::sync_error::SyncError::RetryAfter(at)) => repeat("rate_limited", at),
                    Err(_) => repeat("reconciliation_unavailable", retry),
                },
            );
        }
        let body = if payload.endpoint == "modify" {
            serde_json::to_vec(&serde_json::json!({"addLabelIds":payload.add_label_ids,"removeLabelIds":payload.remove_label_ids})).map_err(|_|StoreError::InvalidInput)?
        } else {
            Vec::new()
        };
        Ok(
            match self
                .accounts
                .gmail_write(
                    account,
                    None,
                    &["messages", &payload.provider_message_id, &payload.endpoint],
                    body,
                    retry,
                )
                .await
            {
                Ok(WriteResult::Acknowledged(value)) => {
                    match MailChangeReceipt::from_google(&value) {
                        Ok(message) if payload.satisfied_by(&message) => {
                            AttemptOutcome::AppliedMail {
                                source: "acknowledgement".into(),
                                message,
                            }
                        }
                        _ => AttemptOutcome::Uncertain {
                            code: "invalid_mutation_response".into(),
                        },
                    }
                }
                Ok(WriteResult::Uncertain { code, .. }) => {
                    AttemptOutcome::Uncertain { code: code.into() }
                }
                Ok(WriteResult::Rejected {
                    code: "resource_not_found",
                    ..
                }) => AttemptOutcome::Conflict {
                    code: "remote_message_missing".into(),
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
}
