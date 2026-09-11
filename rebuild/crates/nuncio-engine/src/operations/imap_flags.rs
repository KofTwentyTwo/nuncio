use super::Service;
use crate::{
    accounts::AccountError,
    providers::imap::{
        flags::{self, Observation},
        MailError,
    },
    store::{AttemptKind, AttemptOutcome, ImapFlagPayload, StoreError},
};

impl Service {
    pub(super) async fn imap_flags(
        &self,
        account: &str,
        payload: &ImapFlagPayload,
        kind: AttemptKind,
        ordinal: u32,
    ) -> Result<AttemptOutcome, StoreError> {
        let base = (1000_i64 << ordinal.saturating_sub(1).min(8)).min(240_000);
        let retry = self
            .accounts
            .http
            .clock
            .now_ms()
            .saturating_add(base + i64::from(rand::random::<u32>()) % (base / 4 + 1));
        let unavailable = |code: &str, transient: bool| match kind {
            AttemptKind::Reconcile if ordinal < 8 => AttemptOutcome::ReconcileLater {
                code: code.into(),
                retry_at_ms: retry,
            },
            AttemptKind::Reconcile => AttemptOutcome::Uncertain { code: code.into() },
            AttemptKind::Dispatch => AttemptOutcome::Rejected {
                code: code.into(),
                retry_at_ms: (transient && ordinal < 8).then_some(retry),
            },
        };
        let mut connection = match self.accounts.imap_session(account).await {
            Ok(connection) => connection,
            Err(error) => {
                return Ok(unavailable(
                    error.code(),
                    matches!(
                        error,
                        AccountError::Unavailable
                            | AccountError::Authorization
                            | AccountError::Secret
                    ),
                ))
            }
        };
        let observed =
            match flags::observe(&mut connection, payload, kind == AttemptKind::Dispatch).await {
                Ok(value) => value,
                Err(error) => {
                    return Ok(unavailable(
                        "imap_preflight_unavailable",
                        matches!(error, MailError::Unavailable),
                    ))
                }
            };
        match observed {
            Observation::EpochChanged => {
                return Ok(AttemptOutcome::Conflict {
                    code: "imap_uid_validity_changed".into(),
                })
            }
            Observation::Missing => {
                return Ok(AttemptOutcome::Conflict {
                    code: "remote_message_missing".into(),
                })
            }
            Observation::Message(message) if payload.satisfied_by(&message) => {
                return Ok(AttemptOutcome::AppliedImapFlags {
                    source: "positive_read".into(),
                    message,
                })
            }
            Observation::Message(_) => {}
        }
        if kind == AttemptKind::Reconcile {
            // An authenticated positive read established current desired-state
            // absence. Only this idempotent flag operation may be dispatched again.
            return Ok(if ordinal < 8 {
                AttemptOutcome::Repeatable {
                    code: "desired_state_not_observed".into(),
                    retry_at_ms: retry,
                }
            } else {
                AttemptOutcome::Conflict {
                    code: "imap_flags_unconfirmed".into(),
                }
            });
        }
        Ok(match flags::change(&mut connection, payload).await {
            Ok(Observation::Message(message)) if payload.satisfied_by(&message) => {
                AttemptOutcome::AppliedImapFlags {
                    source: "acknowledgement".into(),
                    message,
                }
            }
            _ => AttemptOutcome::Uncertain {
                code: "imap_flags_acknowledgement_unknown".into(),
            },
        })
    }
}
