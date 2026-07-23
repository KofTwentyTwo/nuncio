use crossterm::event::{Event, EventStream};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures::StreamExt;
use nuncio_core::EventBus;
use nuncio_tui::keybindings::{UserAction, VimLeaderStateMachine};
use nuncio_tui::{app, TuiApp};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = EventBus::new();
    let mut app = TuiApp::new(bus);
    let mut leader_sm = VimLeaderStateMachine::new();

    // Initialize terminal in alternate screen with raw mode
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut event_stream = EventStream::new();

    // High-performance async event loop (<16ms frame target)
    while app.is_running() {
        terminal.draw(|f| app.render_frame(f))?;

        tokio::select! {
            maybe_event = event_stream.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    let action = leader_sm.process_key(key);
                    match action {
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
                        UserAction::ToggleFilterRules => {
                            if app.mode() == app::AppMode::FilterRules {
                                app.set_mode(app::AppMode::MainView);
                            } else {
                                app.set_mode(app::AppMode::FilterRules);
                            }
                        }
                        UserAction::ToggleFilterSyntax => {
                            if app.mode() == app::AppMode::FilterRules {
                                app.toggle_filter_editor_mode();
                            } else {
                                app.dispatch_command(nuncio_core::CoreCommand::SyncAll);
                            }
                        }
                        UserAction::TestFilterPreview => {
                            if app.mode() == app::AppMode::FilterRules {
                                app.toggle_dry_run_preview();
                            }
                        }
                        UserAction::ReorderPriorityUp => {
                            if app.mode() == app::AppMode::FilterRules {
                                app.reorder_filter_priority_up();
                            }
                        }
                        UserAction::ReorderPriorityDown => {
                            if app.mode() == app::AppMode::FilterRules {
                                app.reorder_filter_priority_down();
                            }
                        }
                        UserAction::ToggleFilterLogs => {
                            if app.mode() == app::AppMode::FilterRules {
                                app.toggle_filter_logs_drawer();
                            }
                        }
                        UserAction::TriggerUpdate => {
                            app.trigger_update();
                        }
                        UserAction::Sync => {
                            app.dispatch_command(nuncio_core::CoreCommand::SyncAll);
                        }
                        UserAction::MoveDown => {
                            app.move_selection_down();
                        }
                        UserAction::MoveUp => {
                            app.move_selection_up();
                        }
                        UserAction::JumpTop => {
                            app.set_mode(app::AppMode::MainView);
                        }
                        UserAction::JumpInbox => {
                            app.set_mode(app::AppMode::MainView);
                            app.set_active_pane(nuncio_tui::ActivePane::MessageList);
                        }
                        UserAction::JumpSent | UserAction::JumpArchive | UserAction::JumpBottom
                        | UserAction::Search | UserAction::Compose | UserAction::Reply | UserAction::None => {}
                    }
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
