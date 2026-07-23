//! Asynchronous terminal event loop integrating `EventBus` channels.

use nuncio_core::{AppState, CoreCommand, CoreEvent, EventBus};
use tokio::sync::{broadcast, watch};

use crate::layout::ActivePane;

/// Active UI view mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Standard 3-pane split (Folders, Messages, Reader).
    #[default]
    MainView,
    /// Keybindings help menu modal overlay.
    HelpModal,
    /// Account settings & switcher view.
    AccountSettings,
    /// Welcome branding splash screen.
    SplashScreen,
}

/// Terminal UI application state container.
pub struct TuiApp {
    event_bus: EventBus,
    active_pane: ActivePane,
    mode: AppMode,
    running: bool,
}

impl TuiApp {
    /// Create a new `TuiApp` wrapping an `EventBus`.
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            event_bus,
            active_pane: ActivePane::Sidebar,
            mode: AppMode::MainView,
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

    /// Access current active view mode.
    pub fn mode(&self) -> AppMode {
        self.mode
    }

    /// Set active view mode.
    pub fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
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
    #[allow(dead_code)]
    pub fn subscribe_state(&self) -> watch::Receiver<AppState> {
        self.event_bus.subscribe_state()
    }

    /// Subscribe to core discrete transactional events.
    #[allow(dead_code)]
    pub fn subscribe_events(&self) -> broadcast::Receiver<CoreEvent> {
        self.event_bus.subscribe_events()
    }

    /// Render ratatui 3-pane split widgets onto frame.
    pub fn render_frame(&self, frame: &mut ratatui::Frame) {
        use crate::html::HtmlRenderer;
        use crate::layout::AppLayout;
        use ratatui::layout::{Constraint, Direction, Layout};
        use ratatui::style::{Color, Style};
        use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(frame.area());

        // Header status bar
        let state = self.event_bus.current_state();
        let header_text = format!(
            " NUNCIO v1.0.0 │ Status: {:?} │ Unread: {} ",
            state.status, state.unread_count
        );
        let header =
            Paragraph::new(header_text).style(Style::default().bg(Color::Blue).fg(Color::White));
        frame.render_widget(header, main_chunks[0]);

        // Render body area based on AppMode
        match self.mode {
            AppMode::SplashScreen => {
                let splash_text = r#"
██╗  ██╗██╗   ██╗███╗   ██╗██████╗██╗ ██████╗ 
████╗██║██║   ██║████╗  ██║██╔════╝██║██╔═══██╗
██╔████║██║   ██║██╔██╗ ██║██║     ██║██║   ██║
██║╚███║██║   ██║██║╚██╗██║██║     ██║██║   ██║
██║ ╚██║╚██████╔╝██║ ╚████║╚██████╗██║╚██████╔╝
╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝ ╚═════╝╚═╝ ╚═════╝ 
                 nuncio.mx v1.0.0
   The Modern Keyboard-First Mail & Calendar Suite

  [?] Keybinding Help Menu
  [a] Account Settings & Switching
  [Tab / Arrows] Navigate Panes
  [q] Quit Nuncio
"#;
                let splash_block = Block::default()
                    .title(" Welcome to Nuncio ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan));
                let splash_p = Paragraph::new(splash_text)
                    .block(splash_block)
                    .style(Style::default().fg(Color::LightCyan));
                frame.render_widget(splash_p, main_chunks[1]);
            }
            AppMode::AccountSettings => {
                let acct_items = vec![
                    ListItem::new(" * [ACTIVE] james.maes@kof22.com (mail.kof22.com:993 / 465 - Implicit TLS)"),
                    ListItem::new("   [IDLE]   work@nuncio.mx (mail.nuncio.mx:993 / 465 - Implicit TLS)"),
                ];
                let acct_block = Block::default()
                    .title(" Account Settings & Switching [a] ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow));
                let acct_list = List::new(acct_items).block(acct_block);
                frame.render_widget(acct_list, main_chunks[1]);
            }
            AppMode::HelpModal => {
                let help_text = r#"
 ╔═════════════════════════════════════════════════════════════╗
 ║                  NUNCIO TUI KEYBINDINGS HELP                ║
 ╠═════════════════════════════════════════════════════════════╣
 ║  Navigation:                                                ║
 ║    j / Down Arrow     Move selection down                   ║
 ║    k / Up Arrow       Move selection up                     ║
 ║    h / Left Arrow     Focus previous pane                   ║
 ║    l / Right Arrow    Focus next pane                       ║
 ║    Tab / BackTab      Cycle active pane                     ║
 ║    gg / G             Jump to Top / Bottom                  ║
 ║                                                             ║
 ║  Actions & Views:                                           ║
 ║    ? or h             Toggle Help Menu Modal                ║
 ║    a                  Account Settings & Switcher           ║
 ║    p                  Splash Screen                         ║
 ║    c                  Compose New Email                     ║
 ║    r                  Reply to Selected Message             ║
 ║    s                  Sync Mail & Calendar Cache            ║
 ║    /                  Search Messages / Events              ║
 ║    q or Esc           Close Modal / Quit Nuncio             ║
 ╚═════════════════════════════════════════════════════════════╝
"#;
                let help_block = Block::default()
                    .title(" Keyboard Help Menu (?) ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green));
                let help_p = Paragraph::new(help_text)
                    .block(help_block)
                    .style(Style::default().fg(Color::Green));
                frame.render_widget(help_p, main_chunks[1]);
            }
            AppMode::MainView => {
                // 3-Pane split
                let (sidebar_area, list_area, reader_area) =
                    AppLayout::compute_layout(main_chunks[1]);

                // Sidebar widget
                let sidebar_items = vec![
                    ListItem::new(" > INBOX (2)"),
                    ListItem::new("   Sent"),
                    ListItem::new("   Drafts"),
                    ListItem::new("   Archive"),
                    ListItem::new("   Trash"),
                ];
                let sidebar_block = Block::default()
                    .title(" Folders (20%) ")
                    .borders(Borders::ALL)
                    .border_style(AppLayout::border_style(
                        ActivePane::Sidebar,
                        self.active_pane,
                    ));
                let sidebar_list = List::new(sidebar_items).block(sidebar_block);
                frame.render_widget(sidebar_list, sidebar_area);

                // Message List widget
                let msg_items = vec![
                    ListItem::new(" * Alice Maes   Q3 Architecture Roadmap"),
                    ListItem::new("   Bob Smith    Server Migration Update"),
                    ListItem::new("   Charlie      PDF Attachment Invoice"),
                ];
                let list_block = Block::default()
                    .title(" Messages (35%) ")
                    .borders(Borders::ALL)
                    .border_style(AppLayout::border_style(
                        ActivePane::MessageList,
                        self.active_pane,
                    ));
                let msg_list = List::new(msg_items).block(list_block);
                frame.render_widget(msg_list, list_area);

                // Reader widget
                let reader_text = HtmlRenderer::render_html(
                    "From: Alice Maes <alice@nuncio.mx>\nSubject: Q3 Architecture Roadmap\n\nHey James,\nHere is the architecture roadmap.",
                    reader_area.width as usize,
                );
                let reader_block = Block::default()
                    .title(" Reader (45%) ")
                    .borders(Borders::ALL)
                    .border_style(AppLayout::border_style(
                        ActivePane::Reader,
                        self.active_pane,
                    ));
                let reader_p = Paragraph::new(reader_text).block(reader_block);
                frame.render_widget(reader_p, reader_area);
            }
        }

        // Bottom hotkey status bar
        let footer_text = " [?] Help │ [a] Accounts │ [p] Splash │ [Tab/Arrows] Navigate │ [c] Compose │ [s] Sync │ [q] Quit ";
        let footer = Paragraph::new(footer_text)
            .style(Style::default().bg(Color::DarkGray).fg(Color::White));
        frame.render_widget(footer, main_chunks[2]);
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
        assert_eq!(app.mode(), AppMode::MainView);

        app.set_active_pane(ActivePane::Reader);
        assert_eq!(app.active_pane(), ActivePane::Reader);

        app.set_mode(AppMode::HelpModal);
        assert_eq!(app.mode(), AppMode::HelpModal);

        app.set_mode(AppMode::AccountSettings);
        assert_eq!(app.mode(), AppMode::AccountSettings);

        app.set_mode(AppMode::SplashScreen);
        assert_eq!(app.mode(), AppMode::SplashScreen);

        let mut state_rx = app.subscribe_state();
        let mut event_rx = app.subscribe_events();

        app.dispatch_command(CoreCommand::SyncAll);
        assert_eq!(
            state_rx.borrow_and_update().status,
            nuncio_core::EngineStatus::Syncing
        );
        assert_eq!(
            event_rx.recv().await.unwrap(),
            CoreEvent::SyncStarted { account_id: None }
        );

        app.quit();
        assert!(!app.is_running());
    }
}
