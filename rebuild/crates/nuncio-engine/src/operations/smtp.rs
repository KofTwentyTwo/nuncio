use super::Service;
use crate::{
    domain::imap_account::SentPolicy,
    providers::imap::{
        sent,
        smtp::submission::{DataResult, SubmissionError},
        MailError,
    },
    store::{AttemptKind, AttemptOutcome, SendPayload, SmtpIntent, SmtpStep, StoreError},
};

impl Service {
    pub(super) async fn smtp(
        &self,
        id: &str,
        intent: &SmtpIntent,
        payload: &SendPayload,
        raw: &[u8],
        attempt: (AttemptKind, u32, crate::store::ExecutionPolicy),
        stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<AttemptOutcome, StoreError> {
        let account = intent.account_id.to_string();
        let (kind, ordinal, policy) = attempt;
        let retry_ordinal = policy.retry_ordinal(ordinal)?;
        let mut progress = self.store.smtp_progress(account.clone(), id.into()).await?;
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
        if progress.step == SmtpStep::Prepared && (policy.restored || policy.observes_only()) {
            return Ok(AttemptOutcome::Uncertain {
                code: if policy.restored {
                    "restored_smtp_acceptance_unknown"
                } else {
                    "reconciliation_resume_required"
                }
                .into(),
            });
        }
        if progress.step == SmtpStep::Prepared && kind == AttemptKind::Reconcile {
            return Ok(AttemptOutcome::RetryUnstartedSmtp {
                retry_at_ms: self.accounts.http.clock.now_ms(),
            });
        }
        if progress.step == SmtpStep::Started {
            return Ok(AttemptOutcome::Uncertain {
                code: "smtp_acceptance_unknown".into(),
            });
        }
        if progress.step == SmtpStep::Appending && policy.mode.is_none() {
            return Ok(AttemptOutcome::Uncertain {
                code: "imap_sent_copy_identity_unknown".into(),
            });
        }
        let mut connection = match self.accounts.imap_session(&account).await {
            Ok(value) => value,
            Err(error) => {
                return Ok(if progress.step == SmtpStep::Prepared {
                    AttemptOutcome::Rejected {
                        code: error.code().into(),
                        retry_at_ms: (retry_ordinal < 8).then_some(retry),
                    }
                } else {
                    uncertain(error.code())
                })
            }
        };
        if progress.step == SmtpStep::Prepared {
            let observed = match sent::mailbox(&mut connection, intent).await {
                Ok(state) => state,
                Err(error) => {
                    return Ok(match error {
                        crate::providers::imap::transfers::TransferError::Conflict(code) => {
                            AttemptOutcome::Rejected {
                                code: code.into(),
                                retry_at_ms: None,
                            }
                        }
                        _ => AttemptOutcome::Rejected {
                            code: "imap_sent_preflight_unavailable".into(),
                            retry_at_ms: (retry_ordinal < 8).then_some(retry),
                        },
                    })
                }
            };
            if intent.sent_policy == SentPolicy::ClientAppend && !connection.capabilities.uidplus {
                return Ok(AttemptOutcome::Rejected {
                    code: "imap_sent_uidplus_required".into(),
                    retry_at_ms: None,
                });
            }
            let smtp = match self.accounts.smtp_session(&account, &intent.endpoint).await {
                Ok(session) => session,
                Err(error) => {
                    return Ok(AttemptOutcome::Rejected {
                        code: error.code().into(),
                        retry_at_ms: (retry_ordinal < 8).then_some(retry),
                    })
                }
            };
            let ready = match smtp
                .prepare(&payload.sender, &payload.recipients, raw)
                .await
            {
                Ok(ready) => ready,
                Err(SubmissionError::Rejected(code)) => {
                    return Ok(AttemptOutcome::Rejected {
                        code: format!("smtp_envelope_{code}"),
                        retry_at_ms: (code < 500 && retry_ordinal < 8).then_some(retry),
                    })
                }
                Err(SubmissionError::Wire(error)) => {
                    return Ok(AttemptOutcome::Rejected {
                        code: "smtp_preflight_failed".into(),
                        retry_at_ms: (matches!(error, MailError::Unavailable) && retry_ordinal < 8)
                            .then_some(retry),
                    })
                }
            };
            // SMTP authentication and recipient negotiation may take time. For
            // server Sent discovery, record a new floor just before DATA bytes.
            let observed = if intent.sent_policy == SentPolicy::Server {
                match sent::mailbox(&mut connection, intent).await {
                    Ok(state) => state,
                    Err(crate::providers::imap::transfers::TransferError::Conflict(code)) => {
                        return Ok(AttemptOutcome::Rejected {
                            code: code.into(),
                            retry_at_ms: None,
                        })
                    }
                    Err(_) => {
                        return Ok(AttemptOutcome::Rejected {
                            code: "imap_sent_preflight_unavailable".into(),
                            retry_at_ms: (retry_ordinal < 8).then_some(retry),
                        })
                    }
                }
            } else {
                observed
            };
            self.store
                .start_smtp_data(
                    account.clone(),
                    id.into(),
                    ordinal,
                    observed,
                    self.accounts.http.clock.now_ms(),
                )
                .await?;
            self.checkpoint("operation_after_smtp_start", stop.clone())
                .await?;
            match ready.transmit().await {
                DataResult::Accepted(_) => {
                    self.store
                        .record_smtp_acceptance(
                            account.clone(),
                            id.into(),
                            ordinal,
                            None,
                            self.accounts.http.clock.now_ms(),
                        )
                        .await?;
                    self.checkpoint("operation_after_smtp_acceptance", stop.clone())
                        .await?;
                }
                DataResult::Rejected(code) => {
                    return Ok(AttemptOutcome::SmtpDataRejected {
                        reply_code: code,
                        retry_at_ms: (code < 500 && retry_ordinal < 8).then_some(retry),
                    })
                }
                DataResult::Unknown => return Ok(uncertain("smtp_acceptance_unknown")),
            }
            progress = self.store.smtp_progress(account.clone(), id.into()).await?;
        }
        if intent.sent_policy == SentPolicy::ClientAppend
            && policy.mode.is_some()
            && matches!(progress.step, SmtpStep::Accepted | SmtpStep::Appending)
        {
            let Some(floor) = progress.sent_floor else {
                return Ok(AttemptOutcome::Uncertain {
                    code: "smtp_sent_observation_floor_unavailable".into(),
                });
            };
            let expected = payload.sent_copy.as_ref().unwrap_or(&payload.wire);
            return Ok(
                match sent::find_client(
                    &mut connection,
                    intent,
                    floor,
                    &payload.message_id,
                    expected,
                    self.accounts.http.max_payload_bytes,
                )
                .await
                {
                    Ok(Some(result)) => AttemptOutcome::ObservedClientSent {
                        result: Box::new(result),
                    },
                    Ok(None) => AttemptOutcome::Uncertain {
                        code: "imap_client_sent_unconfirmed".into(),
                    },
                    Err(crate::providers::imap::transfers::TransferError::Conflict(code)) => {
                        AttemptOutcome::Uncertain { code: code.into() }
                    }
                    Err(_) => uncertain("imap_client_sent_observation_unavailable"),
                },
            );
        }
        if progress.step == SmtpStep::Accepted {
            if intent.sent_policy == SentPolicy::Server {
                let Some(floor) = progress.sent_floor else {
                    return Ok(AttemptOutcome::Uncertain {
                        code: "smtp_sent_observation_floor_unavailable".into(),
                    });
                };
                return Ok(
                    match sent::find_server(
                        &mut connection,
                        intent,
                        floor,
                        &payload.message_id,
                        self.accounts.http.max_payload_bytes,
                    )
                    .await
                    {
                        Ok(Some((result, fingerprint))) => {
                            self.store
                                .record_server_sent(
                                    account.clone(),
                                    id.into(),
                                    ordinal,
                                    crate::store::ServerSentEvidence {
                                        placement: result.placement,
                                        fingerprint,
                                    },
                                    self.accounts.http.clock.now_ms(),
                                )
                                .await?;
                            self.checkpoint("operation_after_server_sent", stop.clone())
                                .await?;
                            AttemptOutcome::AppliedSentCopy {
                                result: Box::new(result),
                            }
                        }
                        Ok(None) => uncertain("smtp_server_sent_unconfirmed"),
                        Err(crate::providers::imap::transfers::TransferError::Conflict(code)) => {
                            AttemptOutcome::Uncertain { code: code.into() }
                        }
                        Err(_) => uncertain("smtp_server_sent_observation_unavailable"),
                    },
                );
            }
            if policy.restored || policy.observes_only() {
                // An accepted send snapshot can predate an unrecorded Sent copy.
                // Without its placement, another APPEND could duplicate that copy.
                return Ok(AttemptOutcome::Uncertain {
                    code: "imap_sent_copy_identity_unknown".into(),
                });
            }
            if sent::mailbox(&mut connection, intent).await.is_err() {
                return Ok(uncertain("imap_sent_preflight_unavailable"));
            }
            let sent_bytes = if let Some(blob) = &payload.sent_copy {
                Some(self.store.read_blob(account.clone(), blob.clone()).await?)
            } else {
                None
            };
            let sent_bytes = sent_bytes.as_deref().unwrap_or(raw);
            self.store
                .start_sent_append(
                    account.clone(),
                    id.into(),
                    ordinal,
                    self.accounts.http.clock.now_ms(),
                )
                .await?;
            self.checkpoint("operation_after_sent_append_start", stop.clone())
                .await?;
            let appended = sent::append(&mut connection, intent, sent_bytes).await;
            if appended.rejected {
                self.store
                    .record_sent_rejection(
                        account.clone(),
                        id.into(),
                        ordinal,
                        self.accounts.http.clock.now_ms(),
                    )
                    .await?;
                self.checkpoint("operation_after_sent_rejection", stop.clone())
                    .await?;
                return Ok(uncertain("imap_sent_append_rejected"));
            }
            if let Some(placement) = appended.placement {
                self.store
                    .record_sent_append(
                        account.clone(),
                        id.into(),
                        ordinal,
                        placement,
                        self.accounts.http.clock.now_ms(),
                    )
                    .await?;
                self.checkpoint("operation_after_sent_append", stop.clone())
                    .await?;
            }
            if !appended.acknowledged {
                return Ok(uncertain("imap_sent_append_acknowledgement_unknown"));
            }
            progress = self.store.smtp_progress(account.clone(), id.into()).await?;
        }
        let Some(placement) = progress.placement else {
            return Ok(AttemptOutcome::Uncertain {
                code: "imap_sent_copy_identity_unknown".into(),
            });
        };
        Ok(
            match sent::observe(
                &mut connection,
                intent,
                placement,
                self.accounts.http.max_payload_bytes,
            )
            .await
            {
                Ok(result) => AttemptOutcome::AppliedSentCopy {
                    result: Box::new(result),
                },
                Err(_) => uncertain("imap_sent_observation_unavailable"),
            },
        )
    }
}
