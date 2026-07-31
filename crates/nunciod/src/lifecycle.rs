//! Coordinates graceful shutdown across every long-running task `nunciod`
//! spawns: the gRPC server, the outbox worker, the auto-update-check loop,
//! and the real-sync `CoreCommand` consumer.
//!
//! Previously the daemon installed no signal handler at all: every real
//! exit was an OS kill (SIGKILL/SIGTERM without a handler, a terminated
//! console), which races [`nuncio_store::DatabaseEngine::close`]'s WAL
//! checkpoint and can corrupt the database. A single [`ShutdownSignal`],
//! shared by every task, lets each one stop accepting new work and finish
//! cleanly instead.

use nuncio_core::{CoreCommand, EventBus};
use std::sync::Arc;
use tokio::sync::watch;

/// Read-only handle to the shared shutdown flag. Cloned into every
/// long-running task; `wait` resolves once shutdown has been requested,
/// including if it was already requested before this clone started
/// waiting.
#[derive(Clone)]
pub struct ShutdownSignal {
    rx: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// Resolves once shutdown has been requested. Safe to call from
    /// `tokio::select!` on every loop iteration: a task that observes
    /// shutdown mid-iteration finishes its current unit of work (the other
    /// `select!` branch) before this is polled again, so no in-flight
    /// database write is abandoned half-done.
    pub async fn wait(&mut self) {
        // `watch::Receiver::wait_for` returns immediately if the current
        // value already satisfies the predicate, so a clone created after
        // shutdown was already requested does not hang waiting for a
        // change that already happened.
        let _ = self.rx.wait_for(|requested| *requested).await;
    }

    /// Non-blocking check for loops that need to poll rather than await.
    pub fn is_requested(&self) -> bool {
        *self.rx.borrow()
    }
}

/// Owns the shutdown trigger. Production wires [`ShutdownController::trigger`]
/// to OS signal delivery via [`install_signal_handlers`]; tests call it
/// directly to drive the shutdown path deterministically without sending a
/// real OS signal.
pub struct ShutdownController {
    tx: watch::Sender<bool>,
    event_bus: Arc<EventBus>,
}

impl ShutdownController {
    /// Creates a controller/signal pair. `event_bus` is the SAME bus every
    /// other subsystem shares, so [`trigger`](Self::trigger) drives the
    /// existing `CoreCommand::Shutdown` / `CoreEvent::ShuttingDown` path
    /// visibly (state flips to `ShuttingDown`, subscribers are notified)
    /// instead of that path staying reachable only from tests.
    pub fn new(event_bus: Arc<EventBus>) -> (Self, ShutdownSignal) {
        let (tx, rx) = watch::channel(false);
        (Self { tx, event_bus }, ShutdownSignal { rx })
    }

    /// Initiates shutdown. Idempotent: calling it more than once (e.g. a
    /// second Ctrl+C while already draining) is a no-op beyond the first.
    pub fn trigger(&self) {
        self.event_bus.process_command(CoreCommand::Shutdown);
        let _ = self.tx.send(true);
    }
}

/// Waits for Ctrl+C, or (on Unix) SIGTERM, then triggers shutdown. Intended
/// to be spawned once at startup and left running for the process lifetime.
pub async fn install_signal_handlers(controller: Arc<ShutdownController>) {
    wait_for_os_signal().await;
    controller.trigger();
}

#[cfg(unix)]
async fn wait_for_os_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    match signal(SignalKind::terminate()) {
        Ok(mut sigterm) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("received Ctrl+C; starting graceful shutdown");
                }
                _ = sigterm.recv() => {
                    tracing::info!("received SIGTERM; starting graceful shutdown");
                }
            }
        }
        Err(err) => {
            // Ctrl+C still works even if SIGTERM registration failed (e.g.
            // an exotic sandboxed environment); log so the gap is visible
            // rather than silently dropping SIGTERM handling.
            tracing::error!("failed to install SIGTERM handler: {err}; SIGTERM will kill the process without a clean shutdown");
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("received Ctrl+C; starting graceful shutdown");
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_os_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("received Ctrl+C; starting graceful shutdown");
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::EngineStatus;

    #[tokio::test]
    async fn trigger_flips_signal_and_event_bus_state() {
        let event_bus = Arc::new(EventBus::new());
        let (controller, mut signal) = ShutdownController::new(event_bus.clone());

        assert!(!signal.is_requested());
        assert_eq!(event_bus.current_state().status, EngineStatus::Idle);

        controller.trigger();

        assert!(signal.is_requested());
        assert_eq!(event_bus.current_state().status, EngineStatus::ShuttingDown);
        // Resolves immediately: the signal was already flipped above.
        signal.wait().await;
    }

    #[tokio::test]
    async fn signal_clone_observes_a_trigger_that_happened_before_it_was_created() {
        let event_bus = Arc::new(EventBus::new());
        let (controller, signal) = ShutdownController::new(event_bus);
        controller.trigger();

        // A clone taken AFTER the trigger must still resolve `wait`
        // immediately rather than hang waiting for a fresh `changed()`.
        let mut late_clone = signal.clone();
        late_clone.wait().await;
        assert!(late_clone.is_requested());
    }

    #[tokio::test]
    async fn trigger_is_idempotent() {
        let event_bus = Arc::new(EventBus::new());
        let (controller, signal) = ShutdownController::new(event_bus.clone());

        controller.trigger();
        controller.trigger();

        assert!(signal.is_requested());
        assert_eq!(event_bus.current_state().status, EngineStatus::ShuttingDown);
    }
}
