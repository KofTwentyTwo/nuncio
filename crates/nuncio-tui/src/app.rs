//! Asynchronous terminal event loop integrating `EventBus` channels.

use nuncio_core::{AppState, CoreCommand, CoreEvent, EventBus};
use tokio::sync::{broadcast, watch};

use crate::layout::ActivePane;

/// Terminal UI application state container.
pub struct TuiApp {
    event_bus: EventBus,
    active_pane: ActivePane,
    running: bool,
}

impl TuiApp {
    /// Create a new `TuiApp` wrapping an `EventBus`.
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            event_bus,
            active_pane: ActivePane::Sidebar,
            running: true,
        }
    }

    /// Access active focused pane.
    pub fn active_pane(&self) -> ActivePane {
        self.active_pane
    }

    /// Set active focused pane.
    pub fn set_active_pane(&mut self, pane: ActivePane) {
        self.active_pane = pane;
    }

    /// Check if event loop is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Stop the event loop.
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Dispatch command to core `EventBus`.
    pub fn dispatch_command(&self, cmd: CoreCommand) {
        self.event_bus.process_command(cmd);
    }

    /// Subscribe to core application state snapshots.
    pub fn subscribe_state(&self) -> watch::Receiver<AppState> {
        self.event_bus.subscribe_state()
    }

    /// Subscribe to core discrete transactional events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<CoreEvent> {
        self.event_bus.subscribe_events()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tui_app_lifecycle_and_channel_subscriptions() {
        let bus = EventBus::new();
        let mut app = TuiApp::new(bus);

        assert!(app.is_running());
        assert_eq!(app.active_pane(), ActivePane::Sidebar);

        app.set_active_pane(ActivePane::Reader);
        assert_eq!(app.active_pane(), ActivePane::Reader);

        let mut state_rx = app.subscribe_state();
        let mut event_rx = app.subscribe_events();

        app.dispatch_command(CoreCommand::SyncAll);
        assert_eq!(state_rx.borrow_and_update().status, nuncio_core::EngineStatus::Syncing);
        assert_eq!(
            event_rx.recv().await.unwrap(),
            CoreEvent::SyncStarted { account_id: None }
        );

        app.quit();
        assert!(!app.is_running());
    }
}
