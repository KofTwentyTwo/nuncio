mod secret;
mod upload;
use crate::{args::BackupCommand, output::AppError};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, maintenance_client::MaintenanceClient, system_client::SystemClient},
};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
type Transport = InterceptedService<Channel, TokenInjector>;
const LIMIT: u64 = 1 << 40;

pub async fn run(
    command: BackupCommand,
    channel: Channel,
    injector: TokenInjector,
) -> Result<serde_json::Value, AppError> {
    let mut client = MaintenanceClient::with_interceptor(channel.clone(), injector.clone());
    match command {
        BackupCommand::Create { output } => {
            let status = SystemClient::with_interceptor(channel, injector)
                .get_status(v2::GetStatusRequest {})
                .await
                .map_err(crate::rpc_error)?
                .into_inner();
            let output = destination(&output, &status.protected_paths)?;
            let secret = secret::read()?;
            create(&mut client, secret, output).await
        }
        BackupCommand::Inspect { file } => {
            let (stream, failed) = upload::prepare(&file, secret::read()?, None)?;
            let result = client.inspect_backup(stream).await;
            if failed.load(Ordering::Relaxed) {
                return Err(file_error());
            }
            let info = result.map_err(|e| rpc_error(e, None))?.into_inner();
            validate_info(&info)?;
            crate::mail::json(info)
        }
        BackupCommand::Restore { file, new_profile } => {
            let (stream, failed) =
                upload::prepare(&file, secret::read()?, Some(new_profile.clone()))?;
            let result = client.restore_backup(stream).await;
            if failed.load(Ordering::Relaxed) {
                return Err(file_error());
            }
            let report = result
                .map_err(|e| rpc_error(e, Some(&new_profile)))?
                .into_inner();
            if uuid::Uuid::parse_str(&report.profile_id).is_err()
                || !Path::new(&report.directory).is_absolute()
                || report.schema_version == 0
                || report
                    .backup
                    .as_ref()
                    .is_none_or(|v| validate_info(v).is_err())
            {
                return Err(invalid_response());
            }
            crate::mail::json(report)
        }
    }
}

async fn create(
    client: &mut MaintenanceClient<Transport>,
    secret: v2::RecoverySecret,
    output: PathBuf,
) -> Result<serde_json::Value, AppError> {
    use v2::backup_chunk::Content;
    let parent = output.parent().ok_or_else(file_error)?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| file_error())?;
    let mut stream = client
        .create_backup(v2::CreateBackupRequest {
            secret: Some(secret),
        })
        .await
        .map_err(|e| rpc_error(e, None))?
        .into_inner();
    let Some(v2::BackupChunk {
        content: Some(Content::Info(info)),
    }) = stream.message().await.map_err(|e| rpc_error(e, None))?
    else {
        return Err(invalid_response());
    };
    validate_info(&info)?;
    let mut received = 0_u64;
    let mut hash = Sha256::new();
    let mut complete = false;
    while let Some(frame) = stream.message().await.map_err(|e| rpc_error(e, None))? {
        if complete {
            return Err(invalid_response());
        }
        match frame.content {
            Some(Content::Data(data)) => {
                if data.offset != received
                    || data.data.is_empty()
                    || data.data.len() > 262144
                    || received
                        .checked_add(data.data.len() as u64)
                        .is_none_or(|n| n > info.byte_length)
                {
                    return Err(invalid_response());
                }
                file.write_all(&data.data).map_err(|_| file_error())?;
                hash.update(&data.data);
                received += data.data.len() as u64;
            }
            Some(Content::Complete(digest)) => {
                if digest.byte_length != received
                    || digest.byte_length != info.byte_length
                    || digest.sha256 != info.sha256
                {
                    return Err(invalid_response());
                }
                complete = true;
            }
            _ => return Err(invalid_response()),
        }
    }
    if !complete || received != info.byte_length || format!("{:x}", hash.finalize()) != info.sha256
    {
        return Err(invalid_response());
    }
    file.as_file().sync_all().map_err(|_| file_error())?;
    file.persist_noclobber(&output).map_err(|_| file_error())?;
    std::fs::File::open(parent).and_then(|f|f.sync_all()).map_err(|_|AppError {code:"backup_output_uncertain",message:"Backup file was completed but directory sync failed; inspect the output before retrying",exit:5,operation:None,sync_run:None,recovery:Some(serde_json::json!({"output":output}))})?;
    Ok(serde_json::json!({"output":output,"backup":info}))
}

fn validate_info(info: &v2::BackupInspection) -> Result<(), AppError> {
    if info.format_version != 1
        || info.schema_version == 0
        || info.created_at_ms < 0
        || info.byte_length == 0
        || info.byte_length > LIMIT
        || !valid_hash(&info.sha256)
    {
        return Err(invalid_response());
    }
    Ok(())
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn destination(path: &Path, protected: &[String]) -> Result<PathBuf, AppError> {
    crate::output_file::destination(path, protected).map_err(|e| match e {
        crate::output_file::OutputPathError::Destination => file_error(),
        crate::output_file::OutputPathError::Protection => invalid_response(),
    })
}
fn file_error() -> AppError {
    AppError{code:"backup_file_io",message:"Backup input must be a stable regular file and output must be a new unprotected writable path",exit:1,operation:None,sync_run:None,recovery:None}
}
fn invalid_response() -> AppError {
    AppError {
        code: "invalid_backup_response",
        message: "Daemon returned an incomplete or invalid backup response",
        exit: 1,
        operation: None,
        sync_run: None,
        recovery: None,
    }
}
fn rpc_error(status: tonic::Status, new_profile: Option<&str>) -> AppError {
    let code = status
        .metadata()
        .get("nuncio-recovery-code")
        .and_then(|v| v.to_str().ok());
    if code == Some("cleanup_incomplete") {
        if let Some(id) = status
            .metadata()
            .get("nuncio-restored-profile-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
        {
            return AppError{code:"restore_cleanup_incomplete",message:"Restore failed; remove only the reported new profile's database/API key entries when secure storage is available",exit:1,operation:None,sync_run:None,recovery:Some(serde_json::json!({"profile_id":id.to_string(),"new_profile":new_profile}))};
        }
    }
    if code == Some("activation_uncertain")
        || (new_profile.is_some()
            && matches!(
                status.code(),
                tonic::Code::Unavailable
                    | tonic::Code::DeadlineExceeded
                    | tonic::Code::Cancelled
                    | tonic::Code::Unknown
            ))
    {
        return AppError {
            code: "restore_outcome_uncertain",
            message: "Restore may have activated; inspect the named target profile before retrying",
            exit: 5,
            operation: None,
            sync_run: None,
            recovery: Some(serde_json::json!({"new_profile":new_profile})),
        };
    }
    if status.code() == tonic::Code::AlreadyExists {
        return AppError {
            code: "restore_target_exists",
            message: "Restore target already exists; originals were preserved",
            exit: 5,
            operation: None,
            sync_run: None,
            recovery: None,
        };
    }
    crate::rpc_error(status)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn restore_transport_loss_reports_target_inspection_without_blind_retry() {
        for code in [
            tonic::Code::Unknown,
            tonic::Code::Cancelled,
            tonic::Code::Unavailable,
            tonic::Code::DeadlineExceeded,
        ] {
            let error = super::rpc_error(
                tonic::Status::new(code, "synthetic lost response"),
                Some("recovered"),
            );
            assert_eq!(error.code, "restore_outcome_uncertain");
            assert_eq!(error.exit, 5);
            assert_eq!(
                error.recovery,
                Some(serde_json::json!({"new_profile":"recovered"}))
            );
        }
        let ordinary = super::rpc_error(tonic::Status::unknown("synthetic transport error"), None);
        assert_eq!(ordinary.code, "request_outcome_unknown");
        assert_eq!(ordinary.exit, 4);
        let invalid = super::rpc_error(
            tonic::Status::invalid_argument("synthetic invalid input"),
            Some("recovered"),
        );
        assert_eq!(invalid.exit, 2);
    }
    #[test]
    fn output_refuses_existing_files_links_and_future_protected_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("store.db");
        std::fs::write(&original, b"original").unwrap();
        let protected = vec![
            original.to_string_lossy().into_owned(),
            temp.path()
                .join("store.db-wal")
                .to_string_lossy()
                .into_owned(),
        ];
        assert!(super::destination(&original, &protected).is_err());
        assert!(super::destination(&temp.path().join("store.db-wal"), &protected).is_err());
        let hard = temp.path().join("hardlink");
        std::fs::hard_link(&original, &hard).unwrap();
        assert!(super::destination(&hard, &protected).is_err());
        #[cfg(unix)]
        {
            let alias = temp.path().join("alias");
            std::os::unix::fs::symlink(temp.path(), &alias).unwrap();
            assert!(super::destination(&alias.join("store.db-wal"), &protected).is_err());
            let dangling = temp.path().join("dangling");
            std::os::unix::fs::symlink(temp.path().join("missing"), &dangling).unwrap();
            assert!(super::destination(&dangling, &protected).is_err());
        }
        assert!(super::destination(&temp.path().join("new.nuncio"), &protected).is_ok());
        assert_eq!(std::fs::read(original).unwrap(), b"original");
    }
    #[test]
    fn cleanup_failure_retains_only_a_valid_profile_identifier_for_recovery() {
        let mut status = tonic::Status::internal("server text must not be printed");
        status.metadata_mut().insert(
            "nuncio-recovery-code",
            "cleanup_incomplete".parse().unwrap(),
        );
        let id = uuid::Uuid::new_v4().to_string();
        status
            .metadata_mut()
            .insert("nuncio-restored-profile-id", id.parse().unwrap());
        let error = super::rpc_error(status, Some("recovered"));
        assert_eq!(error.exit, 1);
        assert_eq!(error.code, "restore_cleanup_incomplete");
        assert_eq!(error.recovery.unwrap()["profile_id"], id);
        assert!(!error.message.contains("server text"));
    }
}
