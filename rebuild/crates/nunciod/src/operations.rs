use nuncio_engine::{engine::Engine, store};
use nuncio_proto::v2::{self, operations_server::Operations};
use std::sync::Arc;
use tonic::{Request, Response, Status};
pub(crate) struct OperationsService(pub Arc<Engine>, pub tokio::sync::watch::Receiver<bool>);
#[tonic::async_trait]
impl Operations for OperationsService {
    async fn reconcile_operation(
        &self,
        request: Request<v2::ReconcileOperationRequest>,
    ) -> Result<Response<v2::Operation>, Status> {
        let q = request.into_inner();
        let mode = match v2::ReconciliationMode::try_from(q.mode) {
            Ok(v2::ReconciliationMode::Observe) => store::ReconciliationMode::Observe,
            Ok(v2::ReconciliationMode::ResumeSafe) => store::ReconciliationMode::ResumeSafe,
            _ => {
                return Err(Status::invalid_argument(
                    "An explicit reconciliation mode is required",
                ))
            }
        };
        let input = store::ReconcileOperation {
            account_id: q.account_id,
            operation_id: q.operation_id,
            request_id: q.request_id,
            expected_version: q.expected_version,
            mode,
        };
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::unavailable("Daemon is stopping"));
        }
        tokio::select! {
            result=self.0.reconcile_operation(input)=>result.map(|op|Response::new(operation(op))).map_err(crate::mail::storage_error),
            _=stopped.changed()=>Err(Status::unavailable("Daemon is stopping; retry the identical reconciliation request")),
            _=tokio::time::sleep(std::time::Duration::from_secs(30))=>Err(Status::deadline_exceeded("Reconciliation admission exceeded 30 seconds; retry the identical request")),
        }
    }
    async fn resolve_operation(
        &self,
        request: Request<v2::ResolveOperationRequest>,
    ) -> Result<Response<v2::Operation>, Status> {
        use v2::resolve_operation_request::Decision;
        let q = request.into_inner();
        let decision = match q.decision.ok_or_else(|| {
            Status::invalid_argument("An explicit resolution decision is required")
        })? {
            Decision::Abandon(q) => store::ResolutionDecision::Abandon { reason: q.reason },
            Decision::ConfirmApplied(q) => store::ResolutionDecision::ConfirmApplied {
                evidence: store::ConfirmationEvidence {
                    provider_id: q.provider_id,
                    message_id: q.message_id,
                    etag: q.etag,
                    observed_at_ms: q.observed_at_ms,
                    note: q.note,
                    smtp_acceptance_note: q.smtp_acceptance_note,
                },
            },
            Decision::Resend(q) => store::ResolutionDecision::Resend {
                request_id: q.request_id,
                accept_duplicate_risk: q.accept_duplicate_risk,
                reason: q.reason,
            },
        };
        let input = store::ResolveOperation {
            account_id: q.account_id,
            operation_id: q.operation_id,
            expected_version: q.expected_version,
            decision,
        };
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::unavailable("Daemon is stopping"));
        }
        tokio::select! {
            result=self.0.resolve_operation(input)=>result.map(|op|Response::new(operation(op))).map_err(crate::mail::storage_error),
            _=stopped.changed()=>Err(Status::unavailable("Daemon is stopping; inspect the resolution after restart")),
            _=tokio::time::sleep(std::time::Duration::from_secs(30))=>Err(Status::deadline_exceeded("Resolution exceeded 30 seconds; retry the identical decision")),
        }
    }
    async fn list_operations(
        &self,
        request: Request<v2::ListOperationsRequest>,
    ) -> Result<Response<v2::ListOperationsResponse>, Status> {
        let q = request.into_inner();
        let page = self
            .0
            .list_operations(
                q.account_id,
                if q.page_size == 0 { 100 } else { q.page_size },
                q.page_token,
            )
            .await
            .map_err(crate::mail::storage_error)?;
        Ok(Response::new(v2::ListOperationsResponse {
            revision: page.revision,
            next_page_token: page.next_page_token,
            items: page
                .items
                .into_iter()
                .map(|o| v2::OperationSummary {
                    id: o.id,
                    account_id: o.account_id,
                    request_id: o.request_id,
                    kind: o.kind,
                    resource_id: o.resource_id,
                    state: o.state,
                    version: o.version,
                    created_at_ms: o.created_at_ms,
                    updated_at_ms: o.updated_at_ms,
                    next_attempt_at_ms: o.next_attempt_at_ms,
                    needs_reconciliation: o.needs_reconciliation,
                    error_code: o.error_code,
                    disposition: o.disposition,
                })
                .collect(),
        }))
    }
    async fn list_attempts(
        &self,
        request: Request<v2::ListOperationAttemptsRequest>,
    ) -> Result<Response<v2::ListOperationAttemptsResponse>, Status> {
        let q = request.into_inner();
        let page = self
            .0
            .operation_attempts(
                q.account_id,
                q.operation_id,
                if q.page_size == 0 { 25 } else { q.page_size },
                q.page_token,
            )
            .await
            .map_err(crate::mail::storage_error)?;
        Ok(Response::new(v2::ListOperationAttemptsResponse {
            revision: page.revision,
            next_page_token: page.next_page_token,
            items: page
                .items
                .into_iter()
                .map(|a| v2::OperationAttempt {
                    ordinal: a.ordinal,
                    kind: a.kind,
                    started_at_ms: a.started_at_ms,
                    finished_at_ms: a.finished_at_ms,
                    outcome: a.outcome,
                    error_code: a.error_code,
                    receipts: a
                        .receipts
                        .into_iter()
                        .map(|r| v2::OperationReceipt {
                            kind: r.kind,
                            source: r.source,
                            provider_id: r.provider_id,
                            etag: r.etag,
                            observed_at_ms: r.observed_at_ms,
                        })
                        .collect(),
                })
                .collect(),
        }))
    }
    async fn get_operation(
        &self,
        request: Request<v2::OperationRequest>,
    ) -> Result<Response<v2::Operation>, Status> {
        let q = request.into_inner();
        Ok(Response::new(operation(
            self.0
                .operation(q.account_id, q.operation_id)
                .await
                .map_err(crate::mail::storage_error)?,
        )))
    }
    async fn cancel_operation(
        &self,
        request: Request<v2::OperationRequest>,
    ) -> Result<Response<v2::Operation>, Status> {
        let q = request.into_inner();
        Ok(Response::new(operation(
            self.0
                .cancel_operation(q.account_id, q.operation_id)
                .await
                .map_err(crate::mail::storage_error)?,
        )))
    }
}
pub(crate) fn operation(op: store::Operation) -> v2::Operation {
    v2::Operation {
        id: op.id,
        account_id: op.account_id,
        request_id: op.request_id,
        kind: op.kind,
        resource_id: op.resource_id,
        fingerprint: op.fingerprint,
        state: op.state,
        version: op.version,
        created_at_ms: op.created_at_ms,
        updated_at_ms: op.updated_at_ms,
        next_attempt_at_ms: op.next_attempt_at_ms,
        needs_reconciliation: op.needs_reconciliation,
        error_code: op.error_code,
        disposition: op.disposition,
        desired_state_json: op.desired_state.to_string(),
        reconciliation: op.reconciliation.map(|r| v2::OperationReconciliation {
            request_id: r.request_id,
            expected_version: r.expected_version,
            mode: match r.mode {
                store::ReconciliationMode::Observe => "observe",
                store::ReconciliationMode::ResumeSafe => "resume_safe",
            }
            .into(),
            requested_at_ms: r.requested_at_ms,
            first_attempt_ordinal: r.first_attempt_ordinal,
            active: r.active,
        }),
        resolutions: op
            .resolutions
            .into_iter()
            .map(|r| v2::OperationResolution {
                decision: r.decision,
                decided_at_ms: r.decided_at_ms,
                expected_version: r.expected_version,
                evidence_json: r.evidence.to_string(),
                replacement_id: r.replacement_id,
            })
            .collect(),
    }
}
