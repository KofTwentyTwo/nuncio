//! Nuncio Monitor: a development instrument for watching a running `nunciod`.
//!
//! This binary owns three cooperating pieces, wired together here:
//!
//! - A dedicated background thread runs a Tokio runtime that drives
//!   [`status::StatusPoller`] (authenticated gRPC polling) and a blocking
//!   log-tailing task built on [`log_tail::LogTailer`]. Both feed channels
//!   the GUI thread drains every frame.
//! - `eframe` owns the main thread's platform event loop and renders
//!   [`ui::draw`] against the single [`state::AppState`] those channels feed.
//! - The Windows tray icon is created lazily, from inside the running
//!   `eframe` app's first `logic()` call -- never during setup and never on
//!   a spawned thread -- because `tray_icon`'s own docs require the tray
//!   icon to be built on the same thread as the platform event loop, after
//!   that loop is already running. See `tray.rs`'s module doc comment for
//!   the full constraint and how menu clicks are forwarded back into the
//!   loop without a second event loop.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod engine;
mod fmt;
mod log_tail;
mod state;
mod status;
mod tray;
mod ui;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use engine::{EngineController, EngineState, Launcher};
use log_tail::{LogAvailability, LogTailer};
use nuncio_store::vault::SecretManager;
use state::AppState;
use status::StatusPoller;

/// How often the background log tailer re-polls the daemon's log directory.
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long the app can go without an external wakeup before it polls its
/// channels again on its own. Keeps status/log updates flowing promptly even
/// when nothing else (user input, a tray event) has requested a repaint.
const IDLE_REPAINT_INTERVAL: Duration = Duration::from_millis(500);

/// Filled once `eframe`'s app-creation callback runs, so the menu-event
/// handler registered before `run_native` is called (and therefore before
/// any `egui::Context` exists) can wake the event loop once one does. A
/// handler firing before this is set simply skips the wakeup -- egui's own
/// first frame runs regardless, so no click is lost, only its wakeup delayed
/// to the next scheduled repaint.
static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

fn main() -> eframe::Result {
    install_tracing_subscriber();

    let db_path = resolve_db_path();
    let log_dir = log_dir_for(&db_path);

    let controller = Arc::new(EngineController::new(
        db_path,
        Launcher::process(daemon_exe_path()),
    ));
    let secrets = Arc::new(SecretManager::production());

    let background = spawn_background(
        engine::DEFAULT_GRPC_ADDR,
        secrets,
        Arc::clone(&controller),
        log_dir,
    );

    let menu_events: Arc<Mutex<VecDeque<tray_icon::menu::MenuEvent>>> =
        Arc::new(Mutex::new(VecDeque::new()));
    install_menu_event_forwarding(Arc::clone(&menu_events));

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([980.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Nuncio Monitor",
        native_options,
        Box::new(move |cc| {
            let _ = EGUI_CTX.set(cc.egui_ctx.clone());
            Ok(Box::new(MonitorApp::new(
                controller,
                background.status_rx,
                background.log_rx,
                background.log_availability_rx,
                menu_events,
                background.startup_error,
            )))
        }),
    )
}

/// Registers the process-global `muda` menu-event handler that forwards
/// every tray menu click into `menu_events` and wakes the `eframe` event
/// loop via `egui::Context::request_repaint` -- `eframe`'s own documented
/// thread-safe wakeup, which forwards to the same winit `EventLoopProxy`
/// `tray-icon`'s docs recommend using directly. This is the "forward events
/// with an `EventLoopProxy`" half of the constraint described in `tray.rs`;
/// the other half (creating the tray icon only after the loop is running)
/// is handled in [`MonitorApp::ensure_tray_created`].
fn install_menu_event_forwarding(menu_events: Arc<Mutex<VecDeque<tray_icon::menu::MenuEvent>>>) {
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |event| {
        if let Ok(mut queue) = menu_events.lock() {
            queue.push_back(event);
        }
        if let Some(ctx) = EGUI_CTX.get() {
            ctx.request_repaint();
        }
    }));
}

/// Channels the GUI thread drains every frame; owned by [`MonitorApp`].
struct BackgroundChannels {
    status_rx: tokio::sync::mpsc::Receiver<status::StatusUpdate>,
    log_rx: std::sync::mpsc::Receiver<log_tail::LogRecord>,
    log_availability_rx: std::sync::mpsc::Receiver<LogAvailability>,
    /// `Some(message)` when the background thread could not even start its
    /// Tokio runtime, so `MonitorApp` can seed [`AppState::last_error`] and
    /// [`AppState::engine`] (as [`EngineState::Unknown`], never `Stopped`)
    /// instead of opening the GUI with a confident-looking "Stopped" that
    /// nothing actually verified.
    startup_error: Option<String>,
}

/// Starts the dedicated background thread that owns a Tokio runtime driving
/// [`StatusPoller`] and the log tailer, and returns the channels the GUI
/// thread reads from. The runtime lives for the process's lifetime: the
/// spawned thread blocks forever once setup completes, keeping every task
/// scheduled on it alive without needing `main`'s own thread (which drives
/// `eframe` instead) to hold onto anything.
fn spawn_background(
    addr: &'static str,
    secrets: Arc<SecretManager>,
    controller: Arc<EngineController>,
    log_dir: PathBuf,
) -> BackgroundChannels {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    // Shared with the spawned thread so the exact runtime-build error
    // survives past that thread returning early -- `ready_rx.recv()`
    // failing (sender dropped without ever sending) only tells the caller
    // THAT startup failed, not why.
    let startup_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let startup_error_for_thread = Arc::clone(&startup_error);

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::error!(error = %err, "failed to start monitor's background tokio runtime");
                if let Ok(mut guard) = startup_error_for_thread.lock() {
                    *guard = Some(format!(
                        "failed to start monitor's background tokio runtime: {err}"
                    ));
                }
                return;
            }
        };
        let _guard = runtime.enter();

        let lock_state = move || controller.liveness();
        let status_rx = StatusPoller::spawn(addr, secrets, lock_state);

        let (log_tx, log_rx) = std::sync::mpsc::channel();
        let (log_availability_tx, log_availability_rx) = std::sync::mpsc::channel();
        tokio::task::spawn_blocking(move || {
            run_log_tailer(log_dir, log_tx, log_availability_tx);
        });

        if ready_tx
            .send(BackgroundChannels {
                status_rx,
                log_rx,
                log_availability_rx,
                startup_error: None,
            })
            .is_err()
        {
            return;
        }

        runtime.block_on(std::future::pending::<()>());
    });

    // If the worker thread failed to even start its runtime, fall back to
    // a set of already-closed channels: the GUI still opens and renders
    // "unknown" everywhere (see `fmt.rs`) rather than the whole app failing
    // to launch over a background setup problem. `startup_error` (set by
    // the thread above before it returned) rides along so the GUI can show
    // it and start in `EngineState::Unknown` rather than a fabricated
    // `Stopped`.
    ready_rx.recv().unwrap_or_else(|_| {
        let (_log_tx, log_rx) = std::sync::mpsc::channel();
        let (_log_availability_tx, log_availability_rx) = std::sync::mpsc::channel();
        let (_status_tx, status_rx) = tokio::sync::mpsc::channel(1);
        let startup_error = startup_error
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .or_else(|| {
                Some("monitor's background thread exited before completing startup".to_string())
            });
        BackgroundChannels {
            status_rx,
            log_rx,
            log_availability_rx,
            startup_error,
        }
    })
}

/// Repeatedly polls `dir` for new daemon log lines, forwarding each parsed
/// record to `log_tx` and this cycle's [`LogAvailability`] to
/// `availability_tx` (sent every cycle, not just on change, so the very
/// first snapshot -- e.g. "directory missing" -- reaches the GUI without
/// waiting for a transition). Runs until the receiving end (the GUI thread)
/// is gone.
fn run_log_tailer(
    dir: PathBuf,
    log_tx: std::sync::mpsc::Sender<log_tail::LogRecord>,
    availability_tx: std::sync::mpsc::Sender<LogAvailability>,
) {
    let mut tailer = LogTailer::new(dir);
    loop {
        for record in tailer.poll() {
            if log_tx.send(record).is_err() {
                return;
            }
        }
        if availability_tx.send(tailer.availability()).is_err() {
            return;
        }
        std::thread::sleep(LOG_POLL_INTERVAL);
    }
}

/// Installs a stderr `tracing-subscriber` so this crate's `tracing::warn!`/
/// `tracing::error!` calls -- its ONLY error-reporting mechanism for
/// failures that happen off the UI thread or before `AppState` exists (e.g.
/// the background runtime failing to start) -- actually reach a developer
/// instead of being silently discarded. `NUNCIO_MONITOR_LOG` (falling back
/// to `RUST_LOG`, then `info`) sets the filter directive, mirroring
/// `nunciod::logging`'s env-var convention without duplicating its
/// file-rotation/JSON machinery, which this dev-only GUI has no need for.
fn install_tracing_subscriber() {
    let filter = std::env::var("NUNCIO_MONITOR_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
}

/// Resolves the daemon's database path exactly as `nunciod`'s own `main`
/// does: `NUNCIO_DB_PATH` if set, otherwise `~/.nuncio/nuncio.db`. Duplicated
/// rather than depending on the `nunciod` binary crate in production code --
/// see `engine.rs`'s module doc comment for why this crate never does that.
fn resolve_db_path() -> PathBuf {
    std::env::var("NUNCIO_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_db_path())
}

/// Mirrors `nunciod::default_db_path`'s `~/.nuncio/nuncio.db` convention.
fn default_db_path() -> PathBuf {
    match std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        Ok(home) => PathBuf::from(home).join(".nuncio").join("nuncio.db"),
        Err(_) => PathBuf::from(".nuncio").join("nuncio.db"),
    }
}

/// Mirrors `nunciod::logging`'s `<data_dir>/logs/` convention, deriving the
/// log directory from the resolved database path the same way the daemon
/// does.
fn log_dir_for(db_path: &Path) -> PathBuf {
    match db_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("logs"),
        _ => PathBuf::from("logs"),
    }
}

/// Locates the `nunciod` executable expected to sit alongside this monitor
/// binary in the same build output directory (see `docs/RUNNING.md`).
fn daemon_exe_path() -> PathBuf {
    let exe_name = if cfg!(windows) {
        "nunciod.exe"
    } else {
        "nunciod"
    };
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join(exe_name)))
        .unwrap_or_else(|| PathBuf::from(exe_name))
}

/// The `eframe` application: owns [`AppState`] and everything that feeds it.
struct MonitorApp {
    state: AppState,
    controller: Arc<EngineController>,
    status_rx: tokio::sync::mpsc::Receiver<status::StatusUpdate>,
    log_rx: std::sync::mpsc::Receiver<log_tail::LogRecord>,
    log_availability_rx: std::sync::mpsc::Receiver<LogAvailability>,
    menu_events: Arc<Mutex<VecDeque<tray_icon::menu::MenuEvent>>>,
    tray: Option<tray::TrayController>,
    /// Set once tray creation fails, so a permanently unavailable tray
    /// (e.g. no desktop session) is not retried every single frame.
    tray_init_failed: bool,
    /// Sending half of `error_rx`, cloned into anything (e.g. the `Stop`
    /// command's spawned thread) that needs to surface a failure into
    /// [`AppState::last_error`] from off the GUI thread. Code already
    /// running ON the GUI thread sets `self.state.last_error` directly
    /// instead of routing through this channel.
    error_tx: std::sync::mpsc::Sender<String>,
    error_rx: std::sync::mpsc::Receiver<String>,
}

impl MonitorApp {
    fn new(
        controller: Arc<EngineController>,
        status_rx: tokio::sync::mpsc::Receiver<status::StatusUpdate>,
        log_rx: std::sync::mpsc::Receiver<log_tail::LogRecord>,
        log_availability_rx: std::sync::mpsc::Receiver<LogAvailability>,
        menu_events: Arc<Mutex<VecDeque<tray_icon::menu::MenuEvent>>>,
        startup_error: Option<String>,
    ) -> Self {
        let (error_tx, error_rx) = std::sync::mpsc::channel();
        let mut state = AppState::default();
        if let Some(err) = startup_error {
            // The background thread that would otherwise tell us anything
            // failed before it could -- report that honestly as `Unknown`
            // rather than the default `Stopped`, which would claim a fact
            // (the daemon is not running) nothing has verified.
            state.engine = EngineState::Unknown;
            state.last_error = Some(err);
        }
        Self {
            state,
            controller,
            status_rx,
            log_rx,
            log_availability_rx,
            menu_events,
            tray: None,
            tray_init_failed: false,
            error_tx,
            error_rx,
        }
    }

    /// Drains every background channel into `self.state`. Never blocks: a
    /// closed or momentarily empty channel simply ends the loop for this
    /// frame, exactly like a poll cycle that has not produced anything new
    /// yet.
    fn drain_channels(&mut self) {
        while let Ok(update) = self.status_rx.try_recv() {
            self.state.apply_status_update(update);
        }
        while let Ok(record) = self.log_rx.try_recv() {
            self.state.push_log(record);
        }
        while let Ok(availability) = self.log_availability_rx.try_recv() {
            self.state.log_availability = availability;
        }
        // Last, so a startup/tray/start/stop failure reported this frame
        // wins over whatever `apply_status_update` may have cleared: a
        // successful poll cycle does not mean the error the user just saw
        // stopped being true.
        while let Ok(message) = self.error_rx.try_recv() {
            self.state.last_error = Some(message);
        }
    }

    /// Creates the tray icon on the FIRST call made after `eframe`'s event
    /// loop is already running -- `logic()` is only ever invoked from
    /// inside that running loop, never during app setup and never from a
    /// spawned thread, satisfying the constraint `tray.rs` documents.
    fn ensure_tray_created(&mut self) {
        if self.tray.is_some() || self.tray_init_failed {
            return;
        }
        match tray::TrayController::new(self.state.engine) {
            Ok(tray) => self.tray = Some(tray),
            Err(err) => {
                tracing::error!(error = %err, "failed to create tray icon");
                self.state.last_error = Some(format!("failed to create tray icon: {err}"));
                self.tray_init_failed = true;
            }
        }
    }

    /// Refreshes the tray icon/menu for the current engine state, then
    /// drains and acts on every menu click queued since the last frame.
    fn handle_tray_commands(&mut self, ctx: &egui::Context) {
        if let Some(tray) = self.tray.as_mut() {
            if let Some(err) = tray.set_engine_state(self.state.engine) {
                self.state.last_error = Some(err);
            }
        }

        let commands: Vec<tray::TrayCommand> = match self.menu_events.lock() {
            Ok(mut queue) => queue
                .drain(..)
                .filter_map(|event| tray::command_for_menu_event(&event))
                .collect(),
            Err(_) => Vec::new(),
        };

        for command in commands {
            self.run_command(command, ctx);
        }
    }

    fn run_command(&mut self, command: tray::TrayCommand, ctx: &egui::Context) {
        match command {
            tray::TrayCommand::Show => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            // Start/Stop are disabled in the menu itself (see
            // `TrayController::set_engine_state`) whenever they do not
            // apply; these checks are a second, authoritative guard against
            // a click that raced a state change between frames, never a
            // substitute for disabling the item.
            tray::TrayCommand::Start => {
                if self.state.engine.allows_start() {
                    if let Err(err) = self.controller.start() {
                        tracing::error!(error = %err, "failed to start nunciod");
                        self.state.last_error = Some(format!("failed to start nunciod: {err}"));
                    }
                }
            }
            tray::TrayCommand::Stop => {
                if self.state.engine.allows_stop() {
                    // `EngineController::stop` blocks for up to its shutdown
                    // timeout waiting on the daemon to exit; running it
                    // directly on the UI thread would freeze the window for
                    // that whole span, so it goes on its own thread instead.
                    // That thread has no `&mut self`, so it reports failure
                    // back through `error_tx` rather than touching
                    // `self.state` directly.
                    let controller = Arc::clone(&self.controller);
                    let error_tx = self.error_tx.clone();
                    std::thread::spawn(move || {
                        if let Err(err) = controller.stop() {
                            tracing::error!(error = %err, "failed to stop nunciod");
                            let _ = error_tx.send(format!("failed to stop nunciod: {err}"));
                        }
                    });
                }
            }
            tray::TrayCommand::Quit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for MonitorApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_channels();
        self.ensure_tray_created();
        self.handle_tray_commands(ctx);
        ctx.request_repaint_after(IDLE_REPAINT_INTERVAL);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui::draw(ui, &mut self.state);
    }
}
