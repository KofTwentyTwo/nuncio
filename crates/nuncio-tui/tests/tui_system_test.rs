//! E2E System Test Suite for nuncio-tui.

use nuncio_core::EventBus;
use nuncio_tui::{ActivePane, AppMode, TuiApp};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[tokio::test]
async fn system_test_tui_rendering_and_mode_transitions() {
    let bus = EventBus::new();
    let mut app = TuiApp::new(bus);

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal init");

    // 1. Render MainView (3-pane split)
    terminal
        .draw(|f| app.render_frame(f))
        .expect("draw frame succeeds");
    let buffer = terminal.backend().buffer();
    assert!(buffer.content().iter().any(|c| c.symbol() == "N"));

    // 2. Switch mode to HelpModal and draw
    app.set_mode(AppMode::HelpModal);
    terminal
        .draw(|f| app.render_frame(f))
        .expect("draw frame succeeds");
    let buffer = terminal.backend().buffer();
    assert!(buffer.content().iter().any(|c| c.symbol() == "?"));

    // 3. Switch mode to AccountSettings and draw
    app.set_mode(AppMode::AccountSettings);
    terminal
        .draw(|f| app.render_frame(f))
        .expect("draw frame succeeds");

    // 4. Switch mode to SplashScreen and draw
    app.set_mode(AppMode::SplashScreen);
    terminal
        .draw(|f| app.render_frame(f))
        .expect("draw frame succeeds");

    // 5. Pane focus switching
    app.set_mode(AppMode::MainView);
    app.set_active_pane(ActivePane::MessageList);
    assert_eq!(app.active_pane(), ActivePane::MessageList);

    app.set_active_pane(ActivePane::Reader);
    assert_eq!(app.active_pane(), ActivePane::Reader);
}
