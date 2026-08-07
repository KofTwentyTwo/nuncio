//! Detects whether `nunciod` is running and controls its start/stop lifecycle.
//!
//! The daemon holds an exclusive OS advisory lock on `<db_path>.lock` for its
//! entire lifetime (see `nunciod::lock::InstanceLock`). That lock -- not the
//! mere existence of the lock file -- is the only signal this module trusts
//! for "is a daemon running": a lock file can exist on disk long after the
//! process that created it has exited, so [`EngineController::liveness`]
//! always attempts its own non-blocking shared lock on the file rather than
//! checking whether the path exists. This module never depends on the
//! `nunciod` crate; it re-implements the same advisory-lock probe against
//! the same sidecar file path convention.

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};

/// Default loopback address `nunciod` binds its gRPC API to.
const DEFAULT_GRPC_ADDR: &str = "127.0.0.1:9420";

/// How long [`EngineController::stop`] waits for the daemon to actually exit
/// (the lock to be released) after `System/Shutdown` returns, before giving
/// up and reporting an error.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// How often [`EngineController::stop`] re-checks liveness while waiting for
/// the daemon to exit.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Observed state of the `nunciod` daemon for one database path.
///
/// `Running` and `NotResponding` are deliberately distinct states, not
/// synonyms: a held instance lock only proves *some* process has the
/// database open, not that it is answering requests. This controller's own
/// [`EngineController::liveness`] decides purely from the lock (never over
/// gRPC, so it never touches the network or the OS keyring), which is
/// enough to tell `Stopped`/`Starting`/`Running` apart. A later health/status
/// component that layers a `GetStatus` probe on top of this lock signal is
/// what turns "lock held but the daemon refuses to answer" into
/// `NotResponding` rather than a falsely healthy `Running` -- the UI must be
/// able to represent that distinction, so it belongs on this enum from the
/// start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    /// No process holds the instance lock.
    Stopped,
    /// [`EngineController::start`] has been called and the instance lock is
    /// not yet held -- the daemon process was launched but has not reached
    /// the point of opening its database.
    Starting,
    /// The instance lock is held. Nothing about the gRPC endpoint has been
    /// checked; a held lock alone is treated as healthy until a health
    /// probe says otherwise.
    Running,
    /// The instance lock's file could not be probed at all (e.g. a
    /// permissions problem opening the sidecar file), so `Stopped` cannot be
    /// asserted -- and, once a health probe is layered on top of this
    /// controller, the state such a probe reports when the lock is held but
    /// the daemon does not answer.
    NotResponding,
}

/// Failure modes for [`EngineController::start`] and [`EngineController::stop`].
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The launcher could not spawn the `nunciod` process.
    #[error("failed to launch nunciod: {0}")]
    Spawn(#[source] std::io::Error),

    /// Could not build the local async runtime used to drive the gRPC call.
    #[error("failed to start async runtime: {0}")]
    Runtime(#[source] std::io::Error),

    /// The gRPC bearer token could not be read or minted from the OS
    /// keyring vault.
    #[error("failed to read gRPC bearer token from vault: {0}")]
    Token(String),

    /// The gRPC endpoint could not be reached at all.
    #[error("nunciod gRPC endpoint unreachable: {0}")]
    Connect(#[from] nuncio_proto::client::ConnectError),

    /// `System/Shutdown` was reachable but rejected the request.
    #[error("nunciod rejected shutdown request: {0}")]
    ShutdownRejected(String),

    /// `System/Shutdown` was requested (and accepted) but the daemon did not
    /// release its instance lock within [`SHUTDOWN_TIMEOUT`].
    ///
    /// Deliberately not followed by a force-kill: `stop` only ever asks the
    /// daemon to shut down gracefully. A silent kill-on-timeout fallback
    /// would reintroduce the ungraceful exits the shutdown RPC exists to
    /// eliminate.
    #[error("nunciod did not stop within {0:?} of requesting shutdown")]
    ShutdownTimedOut(Duration),
}

/// A process launcher for starting `nunciod`, injected so tests never spawn
/// a real `nunciod.exe`.
trait Launch: Send + Sync {
    fn spawn(&self) -> Result<(), EngineError>;
}

/// Injected launcher used by [`EngineController::start`].
///
/// Wraps a boxed [`Launch`] implementation rather than exposing the trait
/// directly, so callers write `Launcher::noop()` / `Launcher::process(..)`
/// without naming a generic parameter on [`EngineController`].
pub struct Launcher(Box<dyn Launch>);

impl Launcher {
    /// A launcher that does nothing and always succeeds -- for tests, so no
    /// real `nunciod.exe` is ever spawned.
    pub fn noop() -> Self {
        Launcher(Box::new(NoopLaunch))
    }

    /// The production launcher: spawns `exe_path` detached, with
    /// `NUNCIO_LOG_FORMAT=json` set so the log viewer gets structured lines.
    /// The daemon must outlive the monitor -- closing the tray must never
    /// stop mail sync -- so the child handle is dropped immediately rather
    /// than waited on or attached to any construct that would tie its
    /// lifetime to this process.
    pub fn process(exe_path: PathBuf) -> Self {
        Launcher(Box::new(ProcessLaunch { exe_path }))
    }

    fn spawn(&self) -> Result<(), EngineError> {
        self.0.spawn()
    }
}

struct NoopLaunch;

impl Launch for NoopLaunch {
    fn spawn(&self) -> Result<(), EngineError> {
        Ok(())
    }
}

struct ProcessLaunch {
    exe_path: PathBuf,
}

impl Launch for ProcessLaunch {
    fn spawn(&self) -> Result<(), EngineError> {
        Command::new(&self.exe_path)
            .env("NUNCIO_LOG_FORMAT", "json")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_child| ())
            .map_err(EngineError::Spawn)
    }
}

/// Detects and controls the lifecycle of the `nunciod` daemon for one
/// database path.
pub struct EngineController {
    db_path: PathBuf,
    launcher: Launcher,
    grpc_addr: String,
    /// Set by `start`, cleared the moment the instance lock is observed
    /// held. Tracks the "launched but hasn't opened its database yet"
    /// window that a lock-only probe cannot otherwise see.
    starting: AtomicBool,
}

impl EngineController {
    /// Creates a controller for the daemon backing `db_path`, using
    /// `launcher` to start it.
    pub fn new(db_path: PathBuf, launcher: Launcher) -> Self {
        Self {
            db_path,
            launcher,
            grpc_addr: DEFAULT_GRPC_ADDR.to_string(),
            starting: AtomicBool::new(false),
        }
    }

    /// Current observed [`EngineState`].
    ///
    /// Liveness is decided purely by attempting a non-blocking *shared*
    /// lock on `<db_path>.lock`, the exact sidecar path
    /// `nunciod::lock::InstanceLock` uses -- this method never touches the
    /// network or the OS keyring, so it is cheap enough to poll and safe to
    /// call from tests. Success means the lock is free -- nothing holds it,
    /// so the daemon is not running, no matter what the file's mere
    /// existence might suggest (a stale file left behind after a crash is
    /// expected: the OS releases the lock the instant the holding
    /// process's handle closes, so a stale file can never itself be
    /// locked). Failure to acquire means some process currently holds the
    /// exclusive lock.
    pub fn liveness(&self) -> EngineState {
        match self.probe_lock() {
            LockProbe::Free => {
                if self.starting.load(Ordering::SeqCst) {
                    EngineState::Starting
                } else {
                    EngineState::Stopped
                }
            }
            LockProbe::Held => {
                self.starting.store(false, Ordering::SeqCst);
                EngineState::Running
            }
            LockProbe::Error => {
                // Can't prove the lock is free, so this must not be reported
                // as Stopped -- that would tell the UI it is safe to start a
                // second instance against a database another process may
                // still have open.
                EngineState::NotResponding
            }
        }
    }

    /// Launches `nunciod` via the injected [`Launcher`] and marks the
    /// controller as `Starting` until the instance lock is observed held.
    pub fn start(&self) -> Result<(), EngineError> {
        self.launcher.spawn()?;
        self.starting.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Requests a graceful shutdown over `System/Shutdown` and waits for the
    /// instance lock to be released, bounded by [`SHUTDOWN_TIMEOUT`].
    ///
    /// Never falls back to killing the process: on timeout this returns
    /// [`EngineError::ShutdownTimedOut`] and leaves the daemon running.
    pub fn stop(&self) -> Result<(), EngineError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(EngineError::Runtime)?;

        runtime.block_on(self.request_shutdown())?;

        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while Instant::now() < deadline {
            if self.liveness() == EngineState::Stopped {
                return Ok(());
            }
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
        }
        Err(EngineError::ShutdownTimedOut(SHUTDOWN_TIMEOUT))
    }

    async fn request_shutdown(&self) -> Result<(), EngineError> {
        let token = self.bearer_token()?;
        let mut client = nuncio_proto::client::connect_system(&self.grpc_addr, &token).await?;
        client
            .shutdown(nuncio_proto::v1::ShutdownRequest {})
            .await
            .map_err(|status| EngineError::ShutdownRejected(status.to_string()))?;
        Ok(())
    }

    /// Resolves the gRPC bearer token from the real OS keyring vault. Only
    /// called from [`Self::request_shutdown`] -- i.e. only when a caller has
    /// actually invoked [`Self::stop`] against a real daemon -- never from
    /// [`Self::liveness`], which must stay free of any keyring or network
    /// access.
    fn bearer_token(&self) -> Result<String, EngineError> {
        let secrets = SecretManager::production();
        let bytes = secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|err| EngineError::Token(err.to_string()))?;
        Ok(hex::encode(bytes))
    }

    fn probe_lock(&self) -> LockProbe {
        let lock_path = lock_path_for(&self.db_path);
        let file = match OpenOptions::new().read(true).write(true).open(&lock_path) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => return LockProbe::Free,
            Err(_) => return LockProbe::Error,
        };
        match FileExt::try_lock_shared(&file) {
            Ok(true) => LockProbe::Free,
            Ok(false) => LockProbe::Held,
            Err(_) => LockProbe::Error,
        }
        // `file` drops here, releasing whatever shared lock the `Ok(true)`
        // branch just took -- this is a point-in-time probe, never a hold.
    }
}

/// Outcome of a single, immediate attempt to acquire the instance lock.
enum LockProbe {
    /// The lock was free (successfully, momentarily, acquired and released).
    Free,
    /// The lock is held by another process.
    Held,
    /// The lock file could not be opened or queried for reasons other than
    /// not existing (e.g. a permissions problem). Must not be conflated
    /// with `Free`.
    Error,
}

/// Mirrors `nunciod::lock`'s sidecar file path convention: `<db_path>.lock`.
fn lock_path_for(db_path: &Path) -> PathBuf {
    let mut file_name = db_path.as_os_str().to_owned();
    file_name.push(".lock");
    PathBuf::from(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_lock_file_means_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("nuncio.db");
        let ctl = EngineController::new(db, Launcher::noop());
        assert_eq!(ctl.liveness(), EngineState::Stopped);
    }

    #[test]
    fn a_held_lock_file_means_not_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("nuncio.db");

        // Hold the lock exactly as the daemon does, via the same type, so this
        // test breaks if the daemon's locking strategy ever changes.
        let _held = nunciod::lock::InstanceLock::acquire(&db).expect("acquire lock");

        let ctl = EngineController::new(db, Launcher::noop());
        assert_ne!(ctl.liveness(), EngineState::Stopped);
    }

    #[test]
    fn a_released_lock_returns_to_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("nuncio.db");

        {
            let _held = nunciod::lock::InstanceLock::acquire(&db).expect("acquire lock");
        } // guard dropped: the OS releases the lock here

        let ctl = EngineController::new(db, Launcher::noop());
        assert_eq!(ctl.liveness(), EngineState::Stopped);
    }

    #[test]
    fn start_reports_starting_while_the_lock_stays_free() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("nuncio.db");
        let ctl = EngineController::new(db, Launcher::noop());

        ctl.start().expect("noop launcher never fails");
        assert_eq!(ctl.liveness(), EngineState::Starting);
    }

    #[test]
    fn starting_clears_once_the_lock_is_observed_held() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("nuncio.db");
        let ctl = EngineController::new(db.clone(), Launcher::noop());

        ctl.start().expect("noop launcher never fails");
        assert_eq!(ctl.liveness(), EngineState::Starting);

        let _held = nunciod::lock::InstanceLock::acquire(&db).expect("acquire lock");
        assert_ne!(ctl.liveness(), EngineState::Starting);
    }

    // `stop()` is intentionally not exercised here: it always resolves the
    // gRPC bearer token from the real OS keyring vault before dialing the
    // daemon's default loopback address, and this crate has no injected
    // `SecretManager`/address override to redirect that at a mock. Covering
    // it would mean either touching the real keyring from a unit test (this
    // workspace mocks the keyring in every other crate's tests via
    // `MockKeyring`) or dialing the real default gRPC port, which could
    // reach an actual `nunciod` a developer happens to have running.
}
