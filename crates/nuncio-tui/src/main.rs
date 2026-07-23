//! Nuncio Terminal UI application entry point.

mod app;
mod html;
mod keybindings;
mod layout;

use app::TuiApp;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use nuncio_core::EventBus;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = EventBus::new();
    let mut app = TuiApp::new(bus);

    // Initialize terminal in alternate screen with raw mode
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    // Interactive event loop
    while app.is_running() {
        terminal.draw(|f| app.render_frame(f))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), KeyModifiers::NONE) | (KeyCode::Esc, _) => app.quit(),
                    (KeyCode::Tab, KeyModifiers::NONE) => {
                        let next_pane = app.active_pane().next();
                        app.set_active_pane(next_pane);
                    }
                    (KeyCode::BackTab, _) => {
                        let prev_pane = app.active_pane().previous();
                        app.set_active_pane(prev_pane);
                    }
                    (KeyCode::Char('s'), KeyModifiers::NONE) => {
                        app.dispatch_command(nuncio_core::CoreCommand::SyncAll);
                    }
                    _ => {}
                }
            }
        }
    }

    // Clean terminal restore on exit
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    println!("Nuncio TUI exited cleanly.");

    Ok(())
}
