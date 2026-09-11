use super::{backup, changes, migrations, worker, BackupInspection, StoreError};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use std::{fs::File, path::Path};
use zeroize::Zeroizing;
mod directory;
use directory::StagingDirectory;

#[derive(Clone, Debug, Serialize)]
pub struct RestoreReport {
    pub backup: BackupInspection,
    pub schema_version: u32,
    pub revision: u64,
    pub held_operations: u64,
}

/// Owns an encrypted, disconnected store until its new profile is initialized.
/// Dropping it before activation removes only the private staging directory.
pub struct StagedRestore {
    directory: StagingDirectory,
    report: RestoreReport,
}
impl StagedRestore {
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
    /// Activates only within the staging parent, with an atomic no-replace rename.
    /// The caller must finish and close any profile files before activation.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn activate(self, target: &Path) -> Result<RestoreReport, StoreError> {
        let Self {
            mut directory,
            report,
        } = self;
        directory.activate(target)?;
        Ok(report)
    }
}

pub fn stage_restore(
    source: &Path,
    passphrase: Zeroizing<String>,
    database_key: Zeroizing<Vec<u8>>,
    parent: &Path,
    now: i64,
) -> Result<StagedRestore, StoreError> {
    stage_restore_with(source, passphrase, database_key, parent, now, |_, _, _| {
        Ok(())
    })
}

pub(crate) fn stage_restore_with(
    source: &Path,
    passphrase: Zeroizing<String>,
    database_key: Zeroizing<Vec<u8>>,
    parent: &Path,
    now: i64,
    record: impl FnOnce(
        &Path,
        super::DirectoryIdentity,
        super::DirectoryIdentity,
    ) -> Result<(), StoreError>,
) -> Result<StagedRestore, StoreError> {
    if database_key.len() != 32 {
        return Err(StoreError::InvalidKey);
    }
    if now < 0 {
        return Err(StoreError::InvalidInput);
    }
    let directory = StagingDirectory::new(parent)?;
    let (parent_id, stage_id) = directory.identities()?;
    record(directory.path(), parent_id, stage_id)?;
    // Inspect and export an owned copy, so a caller's changes and SQLCipher
    // sidecars cannot change the bytes between validation and export.
    let input = directory.path().join("source.nuncio");
    copy_input(source, &input)?;
    let inspection = backup::inspect_backup(&input, passphrase.clone())?;
    let output = directory.path().join("store.db");
    export(
        &input,
        &output,
        passphrase,
        &database_key,
        inspection.schema_version,
    )?;
    std::fs::remove_file(&input)?;
    let (mut c, lock) = worker::open(directory.path(), &database_key)?;
    drop(database_key);
    let held_operations = sanitize(&mut c, &inspection.sha256, now)?;
    backup::check_integrity(&c)?;
    let status = worker::status(&c)?;
    c.close().map_err(|(_, e)| StoreError::from(e))?;
    drop(lock);
    File::open(&output)?.sync_all()?;
    File::open(directory.path())?.sync_all()?;
    Ok(StagedRestore {
        directory,
        report: RestoreReport {
            backup: inspection,
            schema_version: status.schema_version,
            revision: status.revision,
            held_operations,
        },
    })
}

fn private_file(path: &Path) -> Result<File, StoreError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn copy_input(source: &Path, target: &Path) -> Result<(), StoreError> {
    let input = rustix::fs::open(
        source,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let mut input = File::from(input);
    let metadata = input.metadata()?;
    if !metadata.is_file() {
        return Err(StoreError::InvalidPath);
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = source.as_os_str().to_os_string();
        sidecar.push(suffix);
        match std::fs::symlink_metadata(Path::new(&sidecar)) {
            Ok(_) => return Err(StoreError::InvalidInput),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    super::backup_preflight(
        target.parent().ok_or(StoreError::InvalidPath)?,
        metadata.len(),
        3,
    )?;
    let mut output = private_file(target)?;
    super::backup_io::copy_bounded(&mut input, &mut output, metadata.len())?;
    if input.metadata()?.len() != metadata.len() {
        return Err(StoreError::InvalidInput);
    }
    output.sync_all()?;
    Ok(())
}

fn export(
    source: &Path,
    target: &Path,
    passphrase: Zeroizing<String>,
    key: &[u8],
    version: u32,
) -> Result<(), StoreError> {
    private_file(target)?;
    // SQLite applies a read-only main connection to ATTACH as well. This is the
    // private input copy; the caller's backup is never opened for writing.
    let mut c = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.pragma_update(None, "key", passphrase.as_str())?;
    c.pragma_update(None, "cipher_memory_security", true)?;
    c.pragma_update(None, "temp_store", "MEMORY")?;
    migrations::check_version(&c)?;
    let raw_key = Zeroizing::new(format!("x'{}'", hex::encode(key)));
    c.execute(
        "ATTACH DATABASE ?1 AS restored KEY ?2",
        params![
            target.to_str().ok_or(StoreError::InvalidPath)?,
            raw_key.as_str()
        ],
    )?;
    super::backup_io::limit_output(&c, "restored", super::MAX_BACKUP_BYTES)?;
    let tx = c.transaction()?;
    tx.query_row("SELECT sqlcipher_export('restored')", [], |_| Ok(()))?;
    tx.execute_batch(&format!("PRAGMA restored.user_version={version}; PRAGMA restored.application_id=0; DROP TABLE restored.nuncio_backup_metadata;"))?;
    tx.commit()?;
    c.execute_batch("DETACH DATABASE restored")?;
    c.close().map_err(|(_, e)| StoreError::from(e))?;
    Ok(())
}

fn sanitize(c: &mut Connection, hash: &str, now: i64) -> Result<u64, StoreError> {
    let tx = c.transaction()?;
    // Old references belong to another profile. In particular, replaying its
    // cleanup queue could delete secrets still needed by the original profile.
    tx.execute_batch("UPDATE accounts SET state='disconnected',credential_ref=NULL; DELETE FROM credential_cleanup; DELETE FROM restore_cleanup_jobs;")?;
    let pending =
        "state IN ('queued','running','retry_wait','conflict','uncertain') AND disposition IS NULL";
    tx.execute(&format!("INSERT INTO restored_operations(account_id,operation_id,backup_sha256,source_state,source_version,restored_at_ms) SELECT account_id,id,?1,state,version,?2 FROM operations WHERE {pending}"), params![hash, now])?;
    tx.execute("UPDATE operation_attempts SET finished_at_ms=max(?2,started_at_ms),outcome='uncertain',error_code='restored_snapshot_interrupted' WHERE finished_at_ms IS NULL AND EXISTS(SELECT 1 FROM restored_operations r WHERE r.account_id=operation_attempts.account_id AND r.operation_id=operation_attempts.operation_id AND r.backup_sha256=?1)", params![hash, now])?;
    let count = tx.execute(&format!("UPDATE operations SET state='uncertain',needs_reconciliation=0,next_attempt_at_ms=NULL,error_code='restored_snapshot_unreconciled',version=version+1,updated_at_ms=max(updated_at_ms,?1) WHERE {pending}"), [now])?;
    changes::record(&tx, None, "profile_restored", None)?;
    tx.commit()?;
    Ok(count as u64)
}
