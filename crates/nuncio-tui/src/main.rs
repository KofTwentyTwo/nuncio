//! Nuncio Terminal UI application entry point.

mod app;
mod html;
mod keybindings;
mod layout;

use app::TuiApp;
use crossterm::event::{self, Event};
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
                use keybindings::{KeybindingEngine, UserAction};
                match KeybindingEngine::handle_key(key) {
                    UserAction::Quit => {
                        if app.mode() != app::AppMode::MainView {
                            app.set_mode(app::AppMode::MainView);
                        } else {
                            app.quit();
                        }
                    }
                    UserAction::NextPane => {
                        app.set_active_pane(app.active_pane().next());
                    }
                    UserAction::PreviousPane => {
                        app.set_active_pane(app.active_pane().previous());
                    }
                    UserAction::ToggleHelp => {
                        if app.mode() == app::AppMode::HelpModal {
                            app.set_mode(app::AppMode::MainView);
                        } else {
                            app.set_mode(app::AppMode::HelpModal);
                        }
                    }
                    UserAction::ToggleAccounts => {
                        if app.mode() == app::AppMode::AccountSettings {
                            app.set_mode(app::AppMode::MainView);
                        } else {
                            app.set_mode(app::AppMode::AccountSettings);
                        }
                    }
                    UserAction::ToggleSplash => {
                        if app.mode() == app::AppMode::SplashScreen {
                            app.set_mode(app::AppMode::MainView);
                        } else {
                            app.set_mode(app::AppMode::SplashScreen);
                        }
                    }
                    UserAction::Sync => {
                        app.dispatch_command(nuncio_core::CoreCommand::SyncAll);
                    }
                    UserAction::MoveDown
                    | UserAction::MoveUp
                    | UserAction::JumpTop
                    | UserAction::JumpBottom
                    | UserAction::Search
                    | UserAction::Compose
                    | UserAction::Reply
                    | UserAction::None => {}
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
