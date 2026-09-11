use super::Service;
use crate::{
    providers::imap::transfers::{self, TransferError},
    store::{
        AttemptKind, AttemptOutcome, ImapTransferMode, ImapTransferPayload, ImapTransferStep,
        StoreError,
    },
};

impl Service {
    pub(super) async fn transfer(
        &self,
        account: &str,
        id: &str,
        payload: &ImapTransferPayload,
        attempt: (AttemptKind, u32, crate::store::ExecutionPolicy),
        stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<AttemptOutcome, StoreError> {
        let (kind, ordinal, policy) = attempt;
        let retry_ordinal = policy.retry_ordinal(ordinal)?;
        let progress = self
            .store
            .imap_transfer_progress(account.into(), id.into())
            .await?;
        let same_move = payload.mode == ImapTransferMode::Move
            && payload.source.mailbox_id == payload.destination;
        if progress.phase == ImapTransferStep::Prepared
            && !same_move
            && (policy.restored || policy.observes_only())
        {
            return Ok(AttemptOutcome::Uncertain {
                code: if policy.restored {
                    "restored_imap_copy_identity_unknown"
                } else {
                    "reconciliation_resume_required"
                }
                .into(),
            });
        }
        if progress.phase == ImapTransferStep::Prepared
            && !same_move
            && kind == AttemptKind::Reconcile
        {
            return Ok(AttemptOutcome::RetryUnstartedImapTransfer {
                retry_at_ms: self.accounts.http.clock.now_ms(),
            });
        }
        let base = (1000_i64 << retry_ordinal.saturating_sub(1).min(8)).min(240_000);
        let retry = self
            .accounts
            .http
            .clock
            .now_ms()
            .saturating_add(base + i64::from(rand::random::<u32>()) % (base / 4 + 1));
        let uncertain = |code: &str| {
            if kind == AttemptKind::Reconcile && retry_ordinal < 8 {
                AttemptOutcome::ReconcileLater {
                    code: code.into(),
                    retry_at_ms: retry,
                }
            } else {
                AttemptOutcome::Uncertain { code: code.into() }
            }
        };
        let mut connection = match self.accounts.imap_session(account).await {
            Ok(connection) => connection,
            Err(error) => {
                return Ok(if progress.phase == ImapTransferStep::Prepared {
                    AttemptOutcome::Rejected {
                        code: error.code().into(),
                        retry_at_ms: (retry_ordinal < 8).then_some(retry),
                    }
                } else {
                    uncertain(error.code())
                })
            }
        };
        if progress.phase == ImapTransferStep::Prepared
            && payload.mode == ImapTransferMode::Move
            && payload.source.mailbox_id == payload.destination
        {
            let proof = crate::store::ImapCopyProof {
                source: payload.source,
                destination: payload.source,
            };
            return Ok(
                match transfers::fetch_destination(
                    &mut connection,
                    payload,
                    proof,
                    self.accounts.http.max_payload_bytes,
                )
                .await
                {
                    Ok(result) => AttemptOutcome::AppliedImapTransfer {
                        source: "positive_read".into(),
                        result: Box::new(result),
                    },
                    Err(TransferError::Conflict(code)) => {
                        AttemptOutcome::Conflict { code: code.into() }
                    }
                    Err(_) => AttemptOutcome::Rejected {
                        code: "imap_transfer_preflight_unavailable".into(),
                        retry_at_ms: (retry_ordinal < 8).then_some(retry),
                    },
                },
            );
        }
        let mut progress = progress;
        if progress.phase == ImapTransferStep::Prepared {
            let mode = if payload.mode == ImapTransferMode::Move
                && connection.capabilities.move_messages
            {
                ImapTransferMode::Move
            } else {
                ImapTransferMode::Copy
            };
            if payload.mode == ImapTransferMode::Move
                && mode == ImapTransferMode::Copy
                && !connection.capabilities.uidplus
            {
                return Ok(AttemptOutcome::Rejected {
                    code: "imap_safe_move_unsupported".into(),
                    retry_at_ms: None,
                });
            }
            let preflight = async {
                transfers::destination(&mut connection, payload).await?;
                if transfers::source(&mut connection, payload, true)
                    .await?
                    .is_none()
                {
                    return Err(TransferError::Conflict("remote_message_missing"));
                }
                Ok::<_, TransferError>(())
            }
            .await;
            if let Err(error) = preflight {
                return Ok(match error {
                    TransferError::Conflict(code) => AttemptOutcome::Conflict { code: code.into() },
                    TransferError::Wire(_) => AttemptOutcome::Rejected {
                        code: "imap_transfer_preflight_unavailable".into(),
                        retry_at_ms: (retry_ordinal < 8).then_some(retry),
                    },
                });
            }
            self.store
                .start_imap_transfer(
                    account.into(),
                    id.into(),
                    ordinal,
                    mode,
                    self.accounts.http.clock.now_ms(),
                )
                .await?;
            let copied = transfers::copy_or_move(&mut connection, payload, mode).await;
            if let Some(proof) = copied.proof {
                self.store
                    .record_imap_copy(
                        account.into(),
                        id.into(),
                        ordinal,
                        proof,
                        self.accounts.http.clock.now_ms(),
                    )
                    .await?;
                self.checkpoint("operation_after_imap_copy", stop.clone())
                    .await?;
                if mode == ImapTransferMode::Move && copied.acknowledged {
                    self.store
                        .advance_imap_transfer(
                            account.into(),
                            id.into(),
                            ordinal,
                            ImapTransferStep::SourceRemoved,
                            self.accounts.http.clock.now_ms(),
                        )
                        .await?;
                }
            }
            if !copied.acknowledged {
                return Ok(uncertain("imap_transfer_acknowledgement_unknown"));
            }
            progress = self
                .store
                .imap_transfer_progress(account.into(), id.into())
                .await?;
        }
        let Some(proof) = progress.copy.clone() else {
            return Ok(AttemptOutcome::Uncertain {
                code: "imap_copy_identity_unknown".into(),
            });
        };
        // Positively observe the retained target before a fallback removes the source.
        let observed = match transfers::fetch_destination(
            &mut connection,
            payload,
            proof,
            self.accounts.http.max_payload_bytes,
        )
        .await
        {
            Ok(observed) => observed,
            Err(TransferError::Conflict(code)) => {
                return Ok(AttemptOutcome::Conflict { code: code.into() })
            }
            Err(_) => return Ok(uncertain("imap_copy_observation_unavailable")),
        };
        if payload.mode == ImapTransferMode::Move
            && progress.phase != ImapTransferStep::SourceRemoved
        {
            let source =
                match transfers::source(&mut connection, payload, !policy.observes_only()).await {
                    Ok(source) => source,
                    Err(TransferError::Conflict(code)) => {
                        return Ok(AttemptOutcome::Conflict { code: code.into() })
                    }
                    Err(_) => return Ok(uncertain("imap_source_observation_unavailable")),
                };
            if let Some(flags) = source {
                if policy.observes_only() {
                    return Ok(AttemptOutcome::Uncertain {
                        code: "imap_source_removal_requires_resume".into(),
                    });
                }
                if !connection.capabilities.uidplus {
                    return Ok(uncertain("imap_safe_expunge_unavailable"));
                }
                if progress.phase == ImapTransferStep::Copied {
                    self.store
                        .advance_imap_transfer(
                            account.into(),
                            id.into(),
                            ordinal,
                            ImapTransferStep::DeletingSource,
                            self.accounts.http.clock.now_ms(),
                        )
                        .await?;
                }
                if !flags.values().iter().any(|f| f == "\\Deleted")
                    && transfers::mark_deleted(&mut connection, payload.source.uid.get())
                        .await
                        .is_err()
                {
                    return Ok(uncertain("imap_source_flag_acknowledgement_unknown"));
                }
                self.store
                    .advance_imap_transfer(
                        account.into(),
                        id.into(),
                        ordinal,
                        ImapTransferStep::ExpungingSource,
                        self.accounts.http.clock.now_ms(),
                    )
                    .await?;
                if transfers::expunge(&mut connection, payload.source.uid.get())
                    .await
                    .is_err()
                {
                    return Ok(uncertain("imap_expunge_acknowledgement_unknown"));
                }
                match transfers::source(&mut connection, payload, false).await {
                    Ok(None) => {}
                    _ => return Ok(uncertain("imap_source_removal_unconfirmed")),
                }
            }
            self.store
                .advance_imap_transfer(
                    account.into(),
                    id.into(),
                    ordinal,
                    ImapTransferStep::SourceRemoved,
                    self.accounts.http.clock.now_ms(),
                )
                .await?;
        }
        Ok(AttemptOutcome::AppliedImapTransfer {
            source: if kind == AttemptKind::Dispatch {
                "acknowledgement"
            } else {
                "positive_read"
            }
            .into(),
            result: Box::new(observed),
        })
    }
}
