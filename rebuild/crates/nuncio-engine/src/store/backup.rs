use super::{migrations, Store, StoreError};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const APPLICATION_ID: u32 = 0x4e554e42;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupInspection {
    pub format_version: u32,
    pub schema_version: u32,
    pub created_at_ms: i64,
    pub revision: u64,
    pub accounts: u64,
    pub drafts: u64,
    pub operations: u64,
    pub byte_length: u64,
    pub sha256: String,
}

pub struct BackupArtifact {
    path: PathBuf,
    inspection: BackupInspection,
    _directory: tempfile::TempDir,
    _admission: Option<crate::maintenance::MaintenanceLease>,
}
impl BackupArtifact {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn inspection(&self) -> &BackupInspection {
        &self.inspection
    }
}

fn validate_passphrase(value: &str) -> Result<(), StoreError> {
    // SQLCipher recognizes these specially as raw keys, bypassing passphrase
    // derivation. Recovery inputs always use its passphrase KDF.
    let raw_key = value
        .strip_prefix("x'")
        .or_else(|| value.strip_prefix("X'"))
        .and_then(|v| v.strip_suffix('\''))
        .is_some_and(|v| {
            matches!(v.len(), 64 | 96 | 160) && v.bytes().all(|b| b.is_ascii_hexdigit())
        });
    if value.len() > 4096
        || value.chars().count() < 12
        || value.trim().is_empty()
        || value.contains('\0')
        || raw_key
    {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}

impl Store {
    pub async fn create_backup(
        &self,
        passphrase: Zeroizing<String>,
        now: i64,
    ) -> Result<BackupArtifact, StoreError> {
        self.create_backup_with_guard(passphrase, now, None).await
    }

    pub(crate) async fn create_backup_with_guard(
        &self,
        passphrase: Zeroizing<String>,
        now: i64,
        admission: Option<crate::maintenance::MaintenanceLease>,
    ) -> Result<BackupArtifact, StoreError> {
        validate_passphrase(&passphrase)?;
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move |c| {
            let parent = Path::new(c.path().ok_or(StoreError::InvalidPath)?)
                .parent()
                .ok_or(StoreError::InvalidPath)?;
            let pages: i64 = c.pragma_query_value(None, "page_count", |r| r.get(0))?;
            let page_size = super::backup_io::page_size(c, None)?;
            let bytes = u64::try_from(pages)
                .map_err(|_| StoreError::InvalidInput)?
                .checked_mul(page_size)
                .ok_or(StoreError::ResultTooLarge)?;
            super::backup_preflight(parent, bytes, 2)?;
            let directory = tempfile::Builder::new()
                .prefix(".backup-")
                .tempdir_in(parent)?;
            let path = directory.path().join("snapshot.nuncio");
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&path)?;
            c.execute(
                "ATTACH DATABASE ?1 AS nuncio_backup KEY ?2",
                params![
                    path.to_str().ok_or(StoreError::InvalidPath)?,
                    passphrase.as_str()
                ],
            )?;
            let result = (|| -> Result<(), StoreError> {
                super::backup_io::limit_output(c, "nuncio_backup", super::MAX_BACKUP_BYTES)?;
                let tx = c.transaction()?;
                let version = migrations::check_version(&tx)?;
                let revision: i64 = tx.query_row(
                    "SELECT revision FROM main.store_meta WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )?;
                tx.query_row("SELECT sqlcipher_export('nuncio_backup')", [], |_| Ok(()))?;
                // Cleanup authority is local to the owning profile directory.
                tx.execute_batch("DELETE FROM nuncio_backup.restore_cleanup_jobs")?;
                tx.execute_batch(&format!(
                    "PRAGMA nuncio_backup.user_version={version};
                     PRAGMA nuncio_backup.application_id={APPLICATION_ID};
                     CREATE TABLE nuncio_backup.nuncio_backup_metadata (
                         singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                         format_version INTEGER NOT NULL,
                         schema_version INTEGER NOT NULL,
                         created_at_ms INTEGER NOT NULL,
                         revision INTEGER NOT NULL
                     );"
                ))?;
                tx.execute(
                    "INSERT INTO nuncio_backup.nuncio_backup_metadata VALUES(1,?1,?2,?3,?4)",
                    params![FORMAT_VERSION, version, now, revision],
                )?;
                tx.commit()?;
                Ok(())
            })();
            // Finalize the transaction before detach, including on export failure.
            let detached = c
                .execute_batch("DETACH DATABASE nuncio_backup")
                .map_err(StoreError::from);
            result?;
            detached?;
            File::open(&path)?.sync_all()?;
            File::open(directory.path())?.sync_all()?;
            let inspection = inspect_backup(&path, passphrase)?;
            Ok(BackupArtifact {
                path,
                inspection,
                _directory: directory,
                _admission: admission,
            })
        })
        .await
    }
}

pub fn inspect_backup(
    path: &Path,
    passphrase: Zeroizing<String>,
) -> Result<BackupInspection, StoreError> {
    validate_passphrase(&passphrase)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StoreError::InvalidPath);
    }
    if metadata.len() == 0 {
        return Err(StoreError::InvalidInput);
    }
    if metadata.len() > super::MAX_BACKUP_BYTES {
        return Err(StoreError::ResultTooLarge);
    }
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.pragma_update(None, "key", passphrase.as_str())?;
    let cipher: String = c
        .pragma_query_value(None, "cipher_version", |r| r.get(0))
        .map_err(|_| StoreError::CipherUnavailable)?;
    if cipher.is_empty() {
        return Err(StoreError::CipherUnavailable);
    }
    c.pragma_update(None, "cipher_memory_security", true)?;
    c.pragma_update(None, "temp_store", "MEMORY")?;
    c.query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
        r.get::<_, i64>(0)
    })
    .map_err(|_| StoreError::KeyOrCorrupt)?;
    let application: u32 = c
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|_| StoreError::KeyOrCorrupt)?;
    if application != APPLICATION_ID {
        return Err(StoreError::InvalidInput);
    }
    let version = migrations::check_version(&c)?;
    check_integrity(&c)?;
    let (format,schema,created,revision):(u32,u32,i64,i64)=c.query_row("SELECT format_version,schema_version,created_at_ms,revision FROM nuncio_backup_metadata WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|_|StoreError::KeyOrCorrupt)?;
    let recorded: i64 = c
        .query_row(
            "SELECT revision FROM store_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| StoreError::KeyOrCorrupt)?;
    if format != FORMAT_VERSION
        || schema != version
        || version == 0
        || created < 0
        || revision != recorded
    {
        return Err(StoreError::InvalidInput);
    }
    let accounts: i64 = c.query_row("SELECT count(*) FROM accounts", [], |r| r.get(0))?;
    let drafts: i64 = if version >= 7 {
        c.query_row("SELECT count(*) FROM drafts", [], |r| r.get(0))?
    } else {
        0
    };
    let operations: i64 = if version >= 10 {
        c.query_row("SELECT count(*) FROM operations", [], |r| r.get(0))?
    } else {
        0
    };
    c.close().map_err(|(_, e)| StoreError::from(e))?;
    let mut file = File::open(path)?.take(metadata.len() + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    let mut byte_length = 0;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        byte_length += n as u64;
    }
    if byte_length != metadata.len() {
        return Err(StoreError::InvalidInput);
    }
    Ok(BackupInspection {
        format_version: format,
        schema_version: schema,
        created_at_ms: created,
        revision: u64::try_from(revision).map_err(|_| StoreError::KeyOrCorrupt)?,
        accounts: u64::try_from(accounts).map_err(|_| StoreError::KeyOrCorrupt)?,
        drafts: u64::try_from(drafts).map_err(|_| StoreError::KeyOrCorrupt)?,
        operations: u64::try_from(operations).map_err(|_| StoreError::KeyOrCorrupt)?,
        byte_length,
        sha256: format!("{:x}", hash.finalize()),
    })
}

pub(super) fn check_integrity(c: &Connection) -> Result<(), StoreError> {
    for (pragma, expected) in [
        ("PRAGMA cipher_integrity_check", None),
        ("PRAGMA integrity_check", Some("ok")),
        ("PRAGMA foreign_key_check", None),
    ] {
        let mut statement = c.prepare(pragma).map_err(|_| StoreError::KeyOrCorrupt)?;
        let mut rows = statement.query([]).map_err(|_| StoreError::KeyOrCorrupt)?;
        match (rows.next().map_err(|_| StoreError::KeyOrCorrupt)?, expected) {
            (None, None) => {}
            (Some(row), Some(value))
                if row.get::<_, String>(0).is_ok_and(|actual| actual == value) => {}
            _ => return Err(StoreError::KeyOrCorrupt),
        }
        if rows.next().map_err(|_| StoreError::KeyOrCorrupt)?.is_some() {
            return Err(StoreError::KeyOrCorrupt);
        }
    }
    Ok(())
}
