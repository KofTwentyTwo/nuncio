use nuncio_engine::engine::{Engine, EngineError};
use nuncio_proto::v2::{
    system_server::{System, SystemServer},
    GetStatusRequest, GetStatusResponse, ShutdownRequest, ShutdownResponse, StoreStatus,
};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::watch};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{service::Interceptor, Request, Response, Status};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct BearerAuth {
    expected: Zeroizing<String>,
}

impl BearerAuth {
    pub fn new(expected: Zeroizing<String>) -> Self {
        Self { expected }
    }
}

impl Interceptor for BearerAuth {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let value = request
            .metadata()
            .get("authorization")
            .map(|value| value.as_encoded_bytes())
            .unwrap_or_default();
        if !bool::from(value.ct_eq(self.expected.as_bytes())) {
            return Err(Status::unauthenticated("Profile authorization required"));
        }
        Ok(request)
    }
}

struct SystemService {
    engine: Arc<Engine>,
    stop: watch::Sender<bool>,
}

#[tonic::async_trait]
impl System for SystemService {
    type WatchChangesStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<nuncio_proto::v2::ChangeEvent, Status>> + Send>,
    >;
    async fn watch_changes(
        &self,
        request: Request<nuncio_proto::v2::WatchChangesRequest>,
    ) -> Result<Response<Self::WatchChangesStream>, Status> {
        let reader = self
            .engine
            .watch_changes(request.into_inner().after_revision)
            .await
            .map_err(crate::mail::storage_error)?;
        let stopped = self.stop.subscribe();
        let stream = futures_util::stream::unfold(
            (Some(reader), stopped),
            |(reader, mut stopped)| async move {
                let mut reader = reader?;
                if *stopped.borrow() {
                    return None;
                }
                let result =
                    tokio::select! {result=reader.next()=>result,_=stopped.changed()=>return None};
                match result {
                    Ok(c) => Some((
                        Ok(nuncio_proto::v2::ChangeEvent {
                            revision: c.revision,
                            kind: c.kind,
                            account_id: c.account_id,
                            resource_id: c.resource_id,
                        }),
                        (Some(reader), stopped),
                    )),
                    Err(e) => Some((Err(crate::mail::storage_error(e)), (None, stopped))),
                }
            },
        );
        Ok(Response::new(Box::pin(stream)))
    }
    async fn start_sync(
        &self,
        request: Request<nuncio_proto::v2::StartSyncRequest>,
    ) -> Result<Response<nuncio_proto::v2::SyncRun>, Status> {
        let q = request.into_inner();
        Ok(Response::new(crate::mail::sync_run(
            self.engine
                .sync_account(q.account_id, q.full, q.fetch_message_id)
                .await
                .map_err(crate::mail::error)?,
        )))
    }
    async fn get_sync_run(
        &self,
        request: Request<nuncio_proto::v2::SyncRunRequest>,
    ) -> Result<Response<nuncio_proto::v2::SyncRun>, Status> {
        let q = request.into_inner();
        Ok(Response::new(crate::mail::sync_run(
            self.engine
                .sync_run(q.account_id, q.run_id)
                .await
                .map_err(crate::mail::storage_error)?,
        )))
    }
    async fn cancel_sync(
        &self,
        request: Request<nuncio_proto::v2::SyncRunRequest>,
    ) -> Result<Response<nuncio_proto::v2::SyncRun>, Status> {
        let q = request.into_inner();
        Ok(Response::new(crate::mail::sync_run(
            self.engine
                .cancel_sync(&q.account_id, &q.run_id)
                .await
                .map_err(crate::mail::error)?,
        )))
    }
    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let status = self
            .engine
            .status()
            .await
            .map_err(|_| Status::unavailable("Engine status unavailable"))?;
        Ok(Response::new(GetStatusResponse {
            resources: Some(nuncio_proto::v2::ResourceStatus {
                requests_active: status.resources.requests_active,
                requests_waiting: status.resources.requests_waiting,
                requests_peak: status.resources.requests_peak,
                requests_started: status.resources.requests_started,
                request_limit: status.resources.request_limit,
                bytes_received: status.resources.bytes_received,
                background_jobs: status.resources.background_jobs,
                background_job_limit: status.resources.background_job_limit,
                account_requests: status.resources.account_requests,
                account_request_limit: status.resources.account_request_limit,
                store_queue_depth: status.resources.store_queue_depth,
                store_queue_limit: status.resources.store_queue_limit,
                storage_page_batches: status.resources.storage_page_batches,
            }),
            sync: status
                .sync
                .into_iter()
                .map(|s| nuncio_proto::v2::SyncScopeStatus {
                    account_id: s.account_id,
                    scope: s.scope,
                    phase: s.phase,
                    run_id: s.run_id,
                    processed: s.processed,
                    last_success_at_ms: s.last_success_at_ms,
                    age_ms: s.age_ms,
                    next_attempt_at_ms: s.next_attempt_at_ms,
                    error_code: s.error_code,
                    coverage_state: s.coverage_state,
                })
                .collect(),
            background_sync: status.background_sync,
            scheduler_error: status.scheduler_error,
            operation_worker_error: status.operation_worker_error,
            version: status.version,
            api_version: status.api_version,
            profile_id: status.profile_id,
            storage: Some(StoreStatus {
                schema_version: status.storage.schema_version,
                account_count: status.storage.account_count,
                revision: status.storage.revision,
            }),
            protected_paths: status
                .protected_paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        }))
    }
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        self.stop
            .send(true)
            .map_err(|_| Status::unavailable("Daemon is stopping"))?;
        Ok(Response::new(ShutdownResponse {}))
    }
}

pub async fn serve(engine: Engine, listener: TcpListener) -> Result<(), EngineError> {
    if !listener
        .local_addr()
        .map_err(|_| EngineError::Unavailable)?
        .ip()
        .is_loopback()
    {
        return Err(EngineError::Unavailable);
    }
    let auth = BearerAuth::new(engine.authorization());
    let engine = Arc::new(engine);
    let (stop, mut stopped) = watch::channel(false);
    let service = SystemService {
        engine: engine.clone(),
        stop: stop.clone(),
    };
    let result = tonic::transport::Server::builder()
        .add_service(tonic::service::interceptor::InterceptedService::new(
            SystemServer::new(service).max_decoding_message_size(65536),
            auth.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            nuncio_proto::v2::accounts_server::AccountsServer::new(
                crate::accounts::AccountsService(engine.clone()),
            )
            .max_decoding_message_size(65536),
            auth.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            nuncio_proto::v2::mail_server::MailServer::new(crate::mail::MailService(
                engine.clone(),
                stop.subscribe(),
            ))
            .max_decoding_message_size(2 * 1024 * 1024),
            auth.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            nuncio_proto::v2::operations_server::OperationsServer::new(
                crate::operations::OperationsService(engine.clone(), stop.subscribe()),
            )
            .max_decoding_message_size(65536)
            .max_encoding_message_size(8 * 1024 * 1024),
            auth.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            nuncio_proto::v2::maintenance_server::MaintenanceServer::new(
                crate::maintenance::MaintenanceService(engine.clone(), stop.subscribe()),
            )
            .max_decoding_message_size(512 * 1024)
            .max_encoding_message_size(512 * 1024),
            auth.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            nuncio_proto::v2::calendar_server::CalendarServer::new(
                crate::calendar::CalendarService(engine.clone(), stop.subscribe()),
            )
            .max_decoding_message_size(2 * 1024 * 1024)
            .max_encoding_message_size(16 * 1024 * 1024),
            auth,
        ))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
            tokio::select! {
                _ = stopped.changed() => {},
                _ = tokio::signal::ctrl_c() => {},
            }
            stop.send_replace(true);
        })
        .await;
    let engine = Arc::try_unwrap(engine).map_err(|_| EngineError::Unavailable)?;
    engine.shutdown().await?;
    result.map_err(|_| EngineError::Unavailable)
}
