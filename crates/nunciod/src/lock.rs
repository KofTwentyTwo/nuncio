//! Exclusive, cross-platform advisory lock on the daemon's database path.
//!
//! Nothing about starting a second `nunciod` process today prevents it from
//! opening the same SQLite database another instance already has open --
//! the only accidental protection is a port-bind collision on the gRPC
//! listener, and that only fires if both instances also happen to pick the
//! same port. Two writers on one SQLite file corrupt state.
//!
//! [`InstanceLock::acquire`] takes an OS-level exclusive lock (`flock` on
//! Unix, `LockFileEx` on Windows via the `fs4` crate) on a sidecar file next
//! to the database, `<db_path>.lock`. The lock is scoped to the OS file
//! handle, not tracked by a PID written into the file, so it cannot go
//! stale: if the holding process dies for any reason (crash, kill -9), the
//! OS releases the lock the moment the file descriptor/handle closes.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

/// Failure to acquire the single-instance lock.
#[derive(Debug, thiserror::Error)]
pub enum InstanceLockError {
    /// The lock is already held by another live process. The caller must
    /// treat this as fatal and refuse to open the store.
    #[error("another nunciod instance is already running for this database at {0}")]
    AlreadyRunning(PathBuf),

    /// The lock file itself could not be created/opened/locked, e.g. a
    /// permissions problem or an unwritable parent directory.
    #[error("failed to acquire instance lock at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Holds the exclusive lock on the database path for as long as this guard
/// is alive. Drop it (or let it fall out of scope at process exit) to
/// release the lock; a later call to [`InstanceLock::acquire`] for the same
/// path then succeeds again.
#[derive(Debug)]
pub struct InstanceLock {
    file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Acquires the exclusive lock for `db_path`, creating the sidecar lock
    /// file (`<db_path>.lock`) if it does not already exist.
    ///
    /// Returns [`InstanceLockError::AlreadyRunning`] if another process
    /// already holds it -- callers must fail startup on this rather than
    /// proceed to open the store, since a second writer on the same SQLite
    /// file is exactly the corruption this guards against.
    pub fn acquire(db_path: &Path) -> Result<Self, InstanceLockError> {
        let lock_path = lock_path_for(db_path);

        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| InstanceLockError::Io {
                path: lock_path.clone(),
                source,
            })?;
        }

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|source| InstanceLockError::Io {
                path: lock_path.clone(),
                source,
            })?;

        let acquired = file
            .try_lock_exclusive()
            .map_err(|source| InstanceLockError::Io {
                path: lock_path.clone(),
                source,
            })?;

        if !acquired {
            return Err(InstanceLockError::AlreadyRunning(db_path.to_path_buf()));
        }

        Ok(Self {
            file,
            path: lock_path,
        })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // The OS releases a `flock`/`LockFileEx` lock automatically when the
        // underlying handle closes, so a failure here is not a leaked lock
        // (the process exiting, or `self.file` being dropped right after,
        // clears it regardless) -- it is only logged so an operator sees
        // anything unexpected in the unlock call itself.
        if let Err(err) = self.file.unlock() {
            tracing::warn!(
                path = %self.path.display(),
                error = %err,
                "failed to explicitly release nunciod instance lock; it will still be \
                 released when the process exits and the underlying file handle closes"
            );
        }
    }
}

fn lock_path_for(db_path: &Path) -> PathBuf {
    let mut file_name = db_path.as_os_str().to_owned();
    file_name.push(".lock");
    PathBuf::from(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_for_same_path_fails_while_first_guard_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nuncio.db");

        let first = InstanceLock::acquire(&db_path).unwrap();

        let second = InstanceLock::acquire(&db_path);
        assert!(
            matches!(second, Err(InstanceLockError::AlreadyRunning(ref p)) if p == &db_path),
            "expected AlreadyRunning({db_path:?}), got {second:?}"
        );

        drop(first);
    }

    #[test]
    fn acquire_succeeds_again_after_the_first_guard_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nuncio.db");

        let first = InstanceLock::acquire(&db_path).unwrap();
        drop(first);

        let second = InstanceLock::acquire(&db_path);
        assert!(
            second.is_ok(),
            "acquiring after the prior guard dropped must succeed, got {:?}",
            second.err()
        );
    }

    #[test]
    fn error_message_names_the_database_path() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nuncio.db");

        let first = InstanceLock::acquire(&db_path).unwrap();
        let second = InstanceLock::acquire(&db_path).unwrap_err();

        let message = second.to_string();
        assert!(message.contains("already running"));
        assert!(message.contains(&db_path.display().to_string()));

        drop(first);
    }

    #[test]
    fn lock_file_is_created_beside_the_database_path() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nuncio.db");

        let guard = InstanceLock::acquire(&db_path).unwrap();
        assert!(dir.path().join("nuncio.db.lock").exists());

        drop(guard);
    }
}
