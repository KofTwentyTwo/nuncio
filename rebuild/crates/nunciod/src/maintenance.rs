use nuncio_engine::{
    engine::{BackupInput, Engine, EngineError},
    store::{BackupArtifact, BackupInspection, StoreError},
};
use nuncio_proto::v2::{self, maintenance_server::Maintenance};
use std::{fs::File, io::Read, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::watch;
use tonic::{Request, Response, Status};
use zeroize::Zeroizing;

pub(crate) struct MaintenanceService(pub Arc<Engine>, pub watch::Receiver<bool>);
type DownloadStream =
    Pin<Box<dyn futures_util::Stream<Item = Result<v2::BackupChunk, Status>> + Send>>;

#[tonic::async_trait]
impl Maintenance for MaintenanceService {
    type CreateBackupStream = DownloadStream;
    async fn repair_projection(
        &self,
        request: Request<v2::RepairProjectionRequest>,
    ) -> Result<Response<v2::RepairProjectionResponse>, Status> {
        if *self.1.borrow() {
            return Err(Status::cancelled("Daemon is stopping"));
        }
        let r = request.into_inner();
        let scope = match v2::ProjectionScope::try_from(r.scope).ok() {
            Some(v2::ProjectionScope::Mail) => nuncio_engine::store::ProjectionScope::Mail,
            Some(v2::ProjectionScope::Calendar) => nuncio_engine::store::ProjectionScope::Calendar,
            _ => {
                return Err(Status::invalid_argument(
                    "Select mail or calendar projection",
                ))
            }
        };
        let window = r
            .window
            .map(|w| nuncio_engine::domain::calendar::AgendaWindow::new(&w.from, &w.to))
            .transpose()
            .map_err(|_| Status::invalid_argument("Invalid repair window"))?;
        let result = self
            .0
            .repair_projection(r.account_id, scope, window, r.dry_run)
            .await
            .map_err(crate::mail::error)?;
        let p = result.preview.projection;
        let kept = result.preview.preserved;
        Ok(Response::new(v2::RepairProjectionResponse {
            account_id: result.preview.account_id,
            scope: r.scope,
            dry_run: result.dry_run,
            revision: result.preview.revision,
            projection: Some(v2::ProjectionCounts {
                messages: p.messages,
                collections: p.collections,
                attachments: p.attachments,
                search_entries: p.search_entries,
                calendars: p.calendars,
                events: p.events,
                occurrences: p.occurrences,
            }),
            preserved: Some(v2::PreservedStateCounts {
                drafts: kept.drafts,
                draft_attachments: kept.draft_attachments,
                operations: kept.operations,
                attempts: kept.attempts,
                receipts: kept.receipts,
            }),
            run: result.run.map(crate::mail::sync_run),
            window: result.window.map(|w| v2::AgendaWindow {
                from: w.from,
                to: w.to,
            }),
        }))
    }

    async fn create_backup(
        &self,
        request: Request<v2::CreateBackupRequest>,
    ) -> Result<Response<Self::CreateBackupStream>, Status> {
        let passphrase = secret(request.into_inner().secret)?;
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::cancelled("Daemon is stopping"));
        }
        let artifact = tokio::select! {
            result=self.0.create_backup(passphrase)=>result.map_err(storage_error)?,
            _=stopped.changed()=>return Err(Status::cancelled("Daemon is stopping")),
        };
        let state = Download {
            artifact,
            file: None,
            offset: 0,
            header_sent: false,
        };
        let stream = futures_util::stream::unfold(
            (Some(state), stopped),
            |(state, mut stopped)| async move {
                let state = state?;
                if *stopped.borrow() {
                    return None;
                }
                let job = tokio::task::spawn_blocking(move || state.next());
                let result = tokio::select! {
                    result=job=>result.map_err(|_|Status::internal("Backup reader unavailable")).and_then(|r|r.map_err(storage_error)),
                    _=stopped.changed()=>return None,
                };
                match result {
                    Ok((frame, next)) => Some((Ok(frame), (next, stopped))),
                    Err(error) => Some((Err(error), (None, stopped))),
                }
            },
        );
        Ok(Response::new(Box::pin(stream)))
    }
    async fn inspect_backup(
        &self,
        request: Request<tonic::Streaming<v2::BackupUploadChunk>>,
    ) -> Result<Response<v2::BackupInspection>, Status> {
        let (input, passphrase, _) = self.upload(request.into_inner(), false).await?;
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::cancelled("Daemon is stopping"));
        }
        let result = tokio::select! {
            result=self.0.inspect_backup(input,passphrase)=>result.map_err(storage_error)?,
            _=stopped.changed()=>return Err(Status::cancelled("Daemon is stopping")),
        };
        Ok(Response::new(inspection(result)))
    }
    async fn restore_backup(
        &self,
        request: Request<tonic::Streaming<v2::BackupUploadChunk>>,
    ) -> Result<Response<v2::RestoreBackupResponse>, Status> {
        let (input, passphrase, name) = self.upload(request.into_inner(), true).await?;
        let name = name.ok_or_else(|| Status::invalid_argument("New profile name is required"))?;
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::cancelled("Daemon is stopping"));
        }
        let result = tokio::select! {
            result=self.0.restore_backup(input,passphrase,name)=>result.map_err(engine_error)?,
            _=stopped.changed()=>return Err(Status::cancelled("Daemon is stopping")),
        };
        Ok(Response::new(v2::RestoreBackupResponse {
            profile_id: result.profile_id,
            directory: result
                .directory
                .to_str()
                .ok_or_else(|| Status::internal("Restored profile path is not UTF-8"))?
                .into(),
            backup: Some(inspection(result.restore.backup)),
            schema_version: result.restore.schema_version,
            revision: result.restore.revision,
            held_operations: result.restore.held_operations,
        }))
    }
}
impl MaintenanceService {
    async fn upload(
        &self,
        mut stream: tonic::Streaming<v2::BackupUploadChunk>,
        restoring: bool,
    ) -> Result<(BackupInput, Zeroizing<String>, Option<String>), Status> {
        use v2::backup_upload_chunk::Content;
        let mut stopped = self.1.clone();
        let Some(v2::BackupUploadChunk {
            content: Some(Content::Header(header)),
        }) = next(&mut stream, &mut stopped).await?
        else {
            return Err(Status::invalid_argument("Backup header must be first"));
        };
        if header.new_profile.is_some() != restoring
            || header.new_profile.as_ref().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 64
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            })
        {
            return Err(Status::invalid_argument(
                "Invalid new profile name for this operation",
            ));
        }
        let passphrase = secret(header.secret)?;
        let mut upload = self
            .0
            .begin_backup_upload(header.byte_length, header.sha256)
            .await
            .map_err(storage_error)?;
        while let Some(chunk) = next(&mut stream, &mut stopped).await? {
            let Some(Content::Data(data)) = chunk.content else {
                return Err(Status::invalid_argument(
                    "Expected an ordered backup data chunk",
                ));
            };
            upload = upload
                .append(data.offset, data.data)
                .await
                .map_err(storage_error)?;
        }
        let input = upload.finish().await.map_err(storage_error)?;
        Ok((input, passphrase, header.new_profile))
    }
}
async fn next(
    stream: &mut tonic::Streaming<v2::BackupUploadChunk>,
    stopped: &mut watch::Receiver<bool>,
) -> Result<Option<v2::BackupUploadChunk>, Status> {
    if *stopped.borrow() {
        return Err(Status::cancelled("Daemon is stopping"));
    }
    tokio::select! {
        result=tokio::time::timeout(Duration::from_secs(30),stream.message())=>result.map_err(|_|Status::deadline_exceeded("Backup upload was idle"))?,
        _=stopped.changed()=>Err(Status::cancelled("Daemon is stopping")),
    }
}
fn secret(value: Option<v2::RecoverySecret>) -> Result<Zeroizing<String>, Status> {
    let mut value =
        value.ok_or_else(|| Status::invalid_argument("Recovery passphrase is required"))?;
    if value.passphrase.len() > 4096
        || value.passphrase.chars().count() < 12
        || value.passphrase.trim().is_empty()
        || value.passphrase.contains('\0')
    {
        return Err(Status::invalid_argument("Invalid recovery passphrase"));
    }
    Ok(Zeroizing::new(std::mem::take(&mut value.passphrase)))
}

struct Download {
    artifact: BackupArtifact,
    file: Option<File>,
    offset: u64,
    header_sent: bool,
}
impl Download {
    fn next(mut self) -> Result<(v2::BackupChunk, Option<Self>), StoreError> {
        use v2::backup_chunk::Content;
        if !self.header_sent {
            self.header_sent = true;
            let frame = v2::BackupChunk {
                content: Some(Content::Info(inspection(
                    self.artifact.inspection().clone(),
                ))),
            };
            return Ok((frame, Some(self)));
        }
        if self.file.is_none() {
            self.file = Some(File::open(self.artifact.path())?);
        }
        let file = self.file.as_mut().ok_or(StoreError::Unavailable)?;
        let remaining = self
            .artifact
            .inspection()
            .byte_length
            .checked_sub(self.offset)
            .ok_or(StoreError::KeyOrCorrupt)?;
        if remaining == 0 {
            if file.read(&mut [0_u8; 1])? != 0 {
                return Err(StoreError::KeyOrCorrupt);
            }
            let complete = v2::BackupDigest {
                byte_length: self.offset,
                sha256: self.artifact.inspection().sha256.clone(),
            };
            return Ok((
                v2::BackupChunk {
                    content: Some(Content::Complete(complete)),
                },
                None,
            ));
        }
        let mut data = vec![0; remaining.min(262144) as usize];
        file.read_exact(&mut data)?;
        let offset = self.offset;
        self.offset += data.len() as u64;
        Ok((
            v2::BackupChunk {
                content: Some(Content::Data(v2::BackupData { offset, data })),
            },
            Some(self),
        ))
    }
}
fn inspection(value: BackupInspection) -> v2::BackupInspection {
    v2::BackupInspection {
        format_version: value.format_version,
        schema_version: value.schema_version,
        created_at_ms: value.created_at_ms,
        revision: value.revision,
        accounts: value.accounts,
        drafts: value.drafts,
        operations: value.operations,
        byte_length: value.byte_length,
        sha256: value.sha256,
    }
}
fn storage_error(error: StoreError) -> Status {
    match error {
        StoreError::Io(std::io::ErrorKind::AlreadyExists) => {
            Status::already_exists("Restore target already exists")
        }
        StoreError::InvalidPath | StoreError::InvalidKey | StoreError::KeyOrCorrupt => {
            Status::invalid_argument(error.to_string())
        }
        StoreError::FutureSchema => Status::failed_precondition(error.to_string()),
        StoreError::RestoreActivationUncertain => {
            let mut status = Status::failed_precondition(error.to_string());
            status.metadata_mut().insert(
                "nuncio-recovery-code",
                tonic::metadata::MetadataValue::from_static("activation_uncertain"),
            );
            status
        }
        _ => crate::mail::storage_error(error),
    }
}
fn engine_error(error: EngineError) -> Status {
    match error {
        EngineError::Storage(error) => storage_error(error),
        EngineError::RestoreCleanupIncomplete { profile_id } => {
            let mut status =
                Status::internal("Restore failed and new profile key cleanup is incomplete");
            status.metadata_mut().insert(
                "nuncio-recovery-code",
                tonic::metadata::MetadataValue::from_static("cleanup_incomplete"),
            );
            if let Ok(value) = profile_id.parse() {
                status
                    .metadata_mut()
                    .insert("nuncio-restored-profile-id", value);
            }
            status
        }
        _ => Status::internal(error.to_string()),
    }
}
