//! Core event bus, state management, and command orchestration for Nuncio.

#![forbid(unsafe_code)]

pub mod config;
pub mod ipc;
pub mod mcp_policy;
pub mod model;
pub mod update;
pub mod worm_audit;

pub use config::{AccountConfig, AccountProtocol, ConfigError, TlsMode};
pub use mcp_policy::{AgentPermissions, DataType, McpAgentPolicy};
pub use model::{Attachment, CalendarEvent, Contact, DaemonTelemetry, Email, Folder};
pub use update::{ReleaseInfo, UpdateCheckResult, UpdateEngine, UpdateError};
pub use worm_audit::{verify_worm_chain, WormAuditError, WormAuditRecord, DEFAULT_WORM_KEY};


use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, watch};

/// Errors emitted by the [`EventBus`].
#[derive(Error, Debug, PartialEq, Eq)]
pub enum EventBusError {
    /// Attempted to send a command when the command receiver was closed.
    #[error("command channel closed")]
    CommandChannelClosed,
}

/// Commands dispatched from presentation shells to `nuncio-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreCommand {
    /// Trigger synchronization for all configured accounts.
    SyncAll,
    /// Trigger synchronization for a specific account.
    SyncAccount {
        /// Unique account identifier.
        account_id: String,
    },
    /// Mark a message read or unread.
    MarkRead {
        /// Message identifier.
        message_id: String,
        /// Desired read state.
        read: bool,
    },
    /// Record an error originating from an external subsystem.
    ReportError {
        /// Human-readable error description.
        message: String,
    },
    /// Reload filter rules cache in memory.
    ReloadFilters,
    /// Request graceful shutdown of the core engine.
    Shutdown,
}

/// Discrete events published by `nuncio-core` to subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEvent {
    /// Synchronization started.
    SyncStarted {
        /// Account ID if scoped to a specific account.
        account_id: Option<String>,
    },
    /// Synchronization completed.
    SyncCompleted {
        /// Account ID if scoped to a specific account.
        account_id: Option<String>,
    },
    /// A message's read status changed.
    MessageFlagsChanged {
        /// Message identifier.
        message_id: String,
        /// New read state.
        read: bool,
    },
    /// Filter rule executed on a message.
    FilterExecuted {
        /// Filter rule identifier.
        rule_id: String,
        /// Email message identifier.
        message_id: String,
        /// Action description.
        action_taken: String,
    },
    /// Progress notification during bulk filter execution over a batch of emails.
    BatchFilterProgress {
        /// Messages processed so far in the batch.
        processed: usize,
        /// Total message count in batch.
        total: usize,
        /// Total matching rule executions.
        matched: usize,
    },
    /// Database recovery event emitted when a corrupted DB is backed up.
    DatabaseRecovered {
        /// Forensic backup path.
        backup_path: String,
        /// Number of filter rules salvaged from corrupted DB.
        salvaged_rules_count: usize,
        /// Whether automated resync was triggered.
        resync_triggered: bool,
    },
    /// Software update available on GitHub Releases.
    UpdateAvailable {
        /// Latest available release version (e.g. "0.2.0").
        version: String,
        /// Release notes / changelog.
        release_notes: String,
    },
    /// A non-fatal engine error occurred.

    Error {
        /// Error message text.
        message: String,
    },
    /// Core engine is shutting down.
    ShuttingDown,
}

/// Operational state of the engine core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineStatus {
    /// Core is idle and awaiting commands.
    #[default]
    Idle,
    /// Core is actively synchronizing mail or calendar data.
    Syncing,
    /// Core is in the process of shutting down.
    ShuttingDown,
}

/// Continuous application state snapshot broadcast to presentation layers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppState {
    /// Current operational status.
    pub status: EngineStatus,
    /// Total number of loaded accounts.
    pub accounts_loaded: usize,
    /// Total unread message count across loaded accounts.
    pub unread_count: usize,
    /// Last error message, if any.
    pub last_error: Option<String>,
}

/// Central event bus orchestrating command routing, state streams, and discrete events.
pub struct EventBus {
    command_tx: mpsc::Sender<CoreCommand>,
    command_rx: Option<mpsc::Receiver<CoreCommand>>,
    state_tx: watch::Sender<AppState>,
    event_tx: broadcast::Sender<CoreEvent>,
    receiver_taken: Arc<AtomicBool>,
}

impl EventBus {
    /// Command channel capacity boundary (prevents memory spikes under UI input floods).
    pub const COMMAND_CAPACITY: usize = 100;
    /// Event broadcast channel capacity boundary.
    pub const EVENT_CAPACITY: usize = 100;

    /// Create a new `EventBus` initialized with default state.
    pub fn new() -> Self {
        Self::with_initial_state(AppState::default())
    }

    /// Create a new `EventBus` seeded with specific initial state.
    pub fn with_initial_state(initial: AppState) -> Self {
        let (command_tx, command_rx) = mpsc::channel(Self::COMMAND_CAPACITY);
        let (state_tx, _state_rx) = watch::channel(initial);
        let (event_tx, _event_rx) = broadcast::channel(Self::EVENT_CAPACITY);

        Self {
            command_tx,
            command_rx: Some(command_rx),
            state_tx,
            event_tx,
            receiver_taken: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Retrieve a command sender clone to dispatch commands to the core event loop.
    pub fn command_sender(&self) -> mpsc::Sender<CoreCommand> {
        self.command_tx.clone()
    }

    /// Take exclusive ownership of the command receiver (callable once).
    pub fn take_command_receiver(&mut self) -> Option<mpsc::Receiver<CoreCommand>> {
        if self.receiver_taken.swap(true, Ordering::SeqCst) {
            None
        } else {
            self.command_rx.take()
        }
    }

    /// Send a command to the event bus asynchronously.
    pub async fn send_command(&self, cmd: CoreCommand) -> Result<(), EventBusError> {
        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| EventBusError::CommandChannelClosed)
    }

    /// Subscribe to continuous application state snapshot updates.
    pub fn subscribe_state(&self) -> watch::Receiver<AppState> {
        self.state_tx.subscribe()
    }

    /// Get the current application state snapshot.
    pub fn current_state(&self) -> AppState {
        self.state_tx.borrow().clone()
    }

    /// Mutate the current application state and notify state subscribers.
    pub fn update_state<F>(&self, mutator: F)
    where
        F: FnOnce(&mut AppState),
    {
        self.state_tx.send_modify(mutator);
    }

    /// Subscribe to discrete transactional engine events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<CoreEvent> {
        self.event_tx.subscribe()
    }

    /// Publish a discrete event to all active event subscribers.
    /// Returns the number of active subscribers that received the event.
    pub fn publish_event(&self, event: CoreEvent) -> usize {
        self.event_tx.send(event).unwrap_or(0)
    }

    /// Process a command, updating state snapshots and publishing corresponding events.
    pub fn process_command(&self, cmd: CoreCommand) {
        match cmd {
            CoreCommand::SyncAll => {
                self.update_state(|s| s.status = EngineStatus::Syncing);
                self.publish_event(CoreEvent::SyncStarted { account_id: None });
            }
            CoreCommand::SyncAccount { account_id } => {
                self.update_state(|s| s.status = EngineStatus::Syncing);
                self.publish_event(CoreEvent::SyncStarted {
                    account_id: Some(account_id),
                });
            }
            CoreCommand::MarkRead { message_id, read } => {
                self.update_state(|s| {
                    if read {
                        s.unread_count = s.unread_count.saturating_sub(1);
                    } else {
                        s.unread_count = s.unread_count.saturating_add(1);
                    }
                });
                self.publish_event(CoreEvent::MessageFlagsChanged { message_id, read });
            }
            CoreCommand::ReportError { message } => {
                self.update_state(|s| s.last_error = Some(message.clone()));
                self.publish_event(CoreEvent::Error { message });
            }
            CoreCommand::ReloadFilters => {}
            CoreCommand::Shutdown => {
                self.update_state(|s| s.status = EngineStatus::ShuttingDown);
                self.publish_event(CoreEvent::ShuttingDown);
            }
        }
    }

    /// Mark a sync operation complete, setting status back to Idle and notifying subscribers.
    pub fn complete_sync(&self, account_id: Option<String>) -> usize {
        self.update_state(|s| s.status = EngineStatus::Idle);
        self.publish_event(CoreEvent::SyncCompleted { account_id })
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_event_bus_initializes_with_default_state() {
        let bus = EventBus::default();
        assert_eq!(bus.current_state(), AppState::default());
    }

    #[test]
    fn with_initial_state_seeds_snapshot() {
        let seed = AppState {
            status: EngineStatus::Idle,
            accounts_loaded: 3,
            unread_count: 5,
            last_error: None,
        };
        let bus = EventBus::with_initial_state(seed.clone());
        assert_eq!(bus.current_state(), seed);
    }

    #[test]
    fn take_command_receiver_yields_once() {
        let mut bus = EventBus::new();
        assert!(bus.take_command_receiver().is_some());
        assert!(bus.take_command_receiver().is_none());
    }

    #[tokio::test]
    async fn send_command_delivers_to_receiver() {
        let mut bus = EventBus::new();
        let mut rx = bus.take_command_receiver().expect("receiver available");
        let sender = bus.command_sender();
        sender
            .send(CoreCommand::SyncAll)
            .await
            .expect("send succeeds");
        assert_eq!(rx.recv().await, Some(CoreCommand::SyncAll));

        bus.send_command(CoreCommand::Shutdown)
            .await
            .expect("send succeeds");
        assert_eq!(rx.recv().await, Some(CoreCommand::Shutdown));
    }

    #[tokio::test]
    async fn send_command_errors_when_receiver_dropped() {
        let mut bus = EventBus::new();
        let rx = bus.take_command_receiver().expect("receiver available");
        drop(rx);
        let err = bus
            .send_command(CoreCommand::SyncAll)
            .await
            .expect_err("send fails without a receiver");
        assert!(matches!(err, EventBusError::CommandChannelClosed));
        assert!(!format!("{err:?}").is_empty());
        assert!(err.to_string().contains("command channel closed"));
    }

    #[test]
    fn subscribe_state_observes_updates() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe_state();
        bus.update_state(|state| state.accounts_loaded = 4);
        assert!(rx.has_changed().expect("channel open"));
        assert_eq!(rx.borrow_and_update().accounts_loaded, 4);
    }

    #[test]
    fn publish_event_reports_subscriber_count() {
        let bus = EventBus::new();
        assert_eq!(bus.publish_event(CoreEvent::ShuttingDown), 0);

        let _rx = bus.subscribe_events();
        assert_eq!(bus.publish_event(CoreEvent::ShuttingDown), 1);
    }

    #[tokio::test]
    async fn process_sync_all_sets_syncing_and_emits_event() {
        let bus = EventBus::new();
        let mut events = bus.subscribe_events();
        bus.process_command(CoreCommand::SyncAll);
        assert_eq!(bus.current_state().status, EngineStatus::Syncing);
        assert_eq!(
            events.recv().await.expect("event received"),
            CoreEvent::SyncStarted { account_id: None }
        );
    }

    #[tokio::test]
    async fn process_sync_account_carries_account_id() {
        let bus = EventBus::new();
        let mut events = bus.subscribe_events();
        bus.process_command(CoreCommand::SyncAccount {
            account_id: "acct-1".to_string(),
        });
        assert_eq!(bus.current_state().status, EngineStatus::Syncing);
        assert_eq!(
            events.recv().await.expect("event received"),
            CoreEvent::SyncStarted {
                account_id: Some("acct-1".to_string())
            }
        );
    }

    #[tokio::test]
    async fn process_mark_read_adjusts_unread_count_both_directions() {
        let bus = EventBus::with_initial_state(AppState {
            unread_count: 1,
            ..AppState::default()
        });
        let mut events = bus.subscribe_events();

        bus.process_command(CoreCommand::MarkRead {
            message_id: "msg-1".to_string(),
            read: false,
        });
        assert_eq!(bus.current_state().unread_count, 2);
        assert_eq!(
            events.recv().await.expect("event received"),
            CoreEvent::MessageFlagsChanged {
                message_id: "msg-1".to_string(),
                read: false
            }
        );

        bus.process_command(CoreCommand::MarkRead {
            message_id: "msg-1".to_string(),
            read: true,
        });
        bus.process_command(CoreCommand::MarkRead {
            message_id: "msg-1".to_string(),
            read: true,
        });
        bus.process_command(CoreCommand::MarkRead {
            message_id: "msg-1".to_string(),
            read: true,
        });
        assert_eq!(bus.current_state().unread_count, 0);
    }

    #[tokio::test]
    async fn process_report_error_records_and_emits() {
        let bus = EventBus::new();
        let mut events = bus.subscribe_events();
        bus.process_command(CoreCommand::ReportError {
            message: "boom".to_string(),
        });
        assert_eq!(bus.current_state().last_error, Some("boom".to_string()));
        assert_eq!(
            events.recv().await.expect("event received"),
            CoreEvent::Error {
                message: "boom".to_string()
            }
        );
    }

    #[tokio::test]
    async fn process_shutdown_sets_status_and_emits() {
        let bus = EventBus::new();
        let mut events = bus.subscribe_events();
        bus.process_command(CoreCommand::Shutdown);
        assert_eq!(bus.current_state().status, EngineStatus::ShuttingDown);
        assert_eq!(
            events.recv().await.expect("event received"),
            CoreEvent::ShuttingDown
        );
    }

    #[tokio::test]
    async fn complete_sync_returns_to_idle_and_emits() {
        let bus = EventBus::new();
        let mut events = bus.subscribe_events();
        let notified = bus.complete_sync(Some("acct-1".to_string()));
        assert_eq!(notified, 1);
        assert_eq!(bus.current_state().status, EngineStatus::Idle);
        assert_eq!(
            events.recv().await.expect("event received"),
            CoreEvent::SyncCompleted {
                account_id: Some("acct-1".to_string())
            }
        );
    }
}
