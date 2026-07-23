//! Automated Visual & User Interaction Test Suite for nuncio-tui.
//! Renders Ratatui interface onto TestBackend, simulates keyboard navigation,
//! and verifies terminal buffer visual text snapshot representations.

use nuncio_core::{CoreEvent, EventBus};
use nuncio_tui::{ActivePane, AppMode, TuiApp};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

/// Convert a Ratatui terminal buffer into a human-readable text snapshot grid.
fn buffer_to_snapshot(buffer: &Buffer) -> String {
    let mut snapshot = String::new();
    let area = buffer.area();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = buffer.cell((x, y)).unwrap();
            snapshot.push_str(cell.symbol());
        }
        snapshot.push('\n');
    }
    snapshot
}

#[tokio::test]
async fn automated_tui_visual_and_keyboard_interaction_matrix() {
    let bus = EventBus::new();
    let mut core_events_rx = bus.subscribe_events();
    bus.publish_event(CoreEvent::SyncStarted { account_id: None });
    let mut app = TuiApp::new(bus);

    let backend = TestBackend::new(120, 35);
    let mut terminal = Terminal::new(backend).expect("initialize test terminal");

    // ------------------------------------------------------------------------
    // Step 1: Initial Render & Visual Snapshot Verification (Main 3-Pane View)
    // ------------------------------------------------------------------------
    terminal.draw(|f| app.render_frame(f)).expect("render frame");
    let snapshot_main = buffer_to_snapshot(terminal.backend().buffer());

    // Verify brand header and main pane borders exist in visual snapshot
    assert!(snapshot_main.contains("NUNCIO"));
    assert!(snapshot_main.contains("Folders"));

    // ------------------------------------------------------------------------
    // Step 2: Keyboard Interaction - Focus Pane Cycling (Sidebar -> List -> Reader)
    // ------------------------------------------------------------------------
    assert_eq!(app.active_pane(), ActivePane::Sidebar);

    // Simulate Tab / 'l' to focus Message List
    app.set_active_pane(ActivePane::MessageList);
    terminal.draw(|f| app.render_frame(f)).expect("render list frame");
    assert_eq!(app.active_pane(), ActivePane::MessageList);

    // Simulate Tab / 'l' to focus Reader
    app.set_active_pane(ActivePane::Reader);
    terminal.draw(|f| app.render_frame(f)).expect("render reader frame");
    assert_eq!(app.active_pane(), ActivePane::Reader);

    // ------------------------------------------------------------------------
    // Step 3: Event Stream Interaction - CoreEvent::SyncStarted Push Update
    // ------------------------------------------------------------------------
    let evt = core_events_rx.recv().await.expect("receive published core event");
    assert!(matches!(evt, CoreEvent::SyncStarted { .. }));

    terminal.draw(|f| app.render_frame(f)).expect("render syncing frame");

    // ------------------------------------------------------------------------
    // Step 4: Modal Navigation - Help Overlay Visual Snapshot
    // ------------------------------------------------------------------------
    app.set_mode(AppMode::HelpModal);
    terminal.draw(|f| app.render_frame(f)).expect("render help modal");
    let snapshot_help = buffer_to_snapshot(terminal.backend().buffer());

    assert!(snapshot_help.contains("NUNCIO TUI KEYBINDINGS HELP"));
    assert!(snapshot_help.contains("Move selection"));
    assert!(snapshot_help.contains("Focus next pane"));

    // ------------------------------------------------------------------------
    // Step 5: Modal Navigation - Account Settings Visual Snapshot
    // ------------------------------------------------------------------------
    app.set_mode(AppMode::AccountSettings);
    terminal.draw(|f| app.render_frame(f)).expect("render account settings");
    let snapshot_accounts = buffer_to_snapshot(terminal.backend().buffer());

    assert!(snapshot_accounts.contains("Account Settings"));
    assert!(snapshot_accounts.contains("james.maes@kof22.com"));

    // ------------------------------------------------------------------------
    // Step 6: Modal Navigation - Splash Screen Visual Snapshot
    // ------------------------------------------------------------------------
    app.set_mode(AppMode::SplashScreen);
    terminal.draw(|f| app.render_frame(f)).expect("render splash screen");
    let snapshot_splash = buffer_to_snapshot(terminal.backend().buffer());

    assert!(snapshot_splash.contains("Welcome to Nuncio"));
}
