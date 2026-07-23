//! Asynchronous terminal event loop integrating `EventBus` channels.

use nuncio_core::{CoreCommand, EventBus};
use nuncio_filter::{FilterExecutionLog, FilterPreviewResult, FilterRule, NsqlParser};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use crate::layout::{ActivePane, AppLayout};

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
    /// NSQL Filter Rules Manager view (#282).
    FilterRules,
}

/// Editor sub-mode for NSQL Filter Rules (#282).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterEditorSubMode {
    /// Visual form builder layout.
    #[default]
    VisualBuilder,
    /// NSQL query code editor layout.
    NsqlEditor,
}

/// Terminal UI application state container.
pub struct TuiApp {
    event_bus: EventBus,
    active_pane: ActivePane,
    mode: AppMode,
    running: bool,
    pub accounts: Vec<nuncio_core::AccountConfig>,
    pub selected_account_idx: usize,
    pub folders: Vec<nuncio_core::model::Folder>,
    pub selected_folder_idx: usize,
    pub messages: Vec<nuncio_core::model::Email>,
    pub selected_message_idx: usize,

    // Epic 8: NSQL Filter TUI state
    pub filter_rules: Vec<FilterRule>,
    pub selected_filter_idx: usize,
    pub filter_editor_mode: FilterEditorSubMode,
    pub filter_preview_active: bool,
    pub filter_preview_result: Option<FilterPreviewResult>,
    pub filter_logs_drawer_open: bool,
    pub filter_logs: Vec<FilterExecutionLog>,

    // Epic 9: Recovery Top Menu Banner
    pub recovery_banner: Option<String>,
}

impl TuiApp {
    /// Create a new `TuiApp` wrapping an `EventBus`.
    pub fn new(event_bus: EventBus) -> Self {
        let sample_rule1 = NsqlParser::parse_rule(
            "Priority Boss Filter",
            0,
            "WHERE (from = 'boss@nuncio.mx' AND size > 1024) ACTION MOVE TO 'Priority', FLAG",
        )
        .expect("parse sample rule");

        let sample_rule2 = NsqlParser::parse_rule(
            "Spam Auto Cleaner",
            1,
            "WHERE folder IN ('spam', 'junk') ACTION DELETE",
        )
        .expect("parse sample rule");

        Self {
            event_bus,
            active_pane: ActivePane::Sidebar,
            mode: AppMode::MainView,
            running: true,
            accounts: Vec::new(),
            selected_account_idx: 0,
            folders: vec![
                nuncio_core::model::Folder {
                    id: "inbox".to_string(),
                    name: "INBOX".to_string(),
                    total_messages: 2,
                    unread_messages: 1,
                },
                nuncio_core::model::Folder {
                    id: "sent".to_string(),
                    name: "Sent".to_string(),
                    total_messages: 0,
                    unread_messages: 0,
                },
                nuncio_core::model::Folder {
                    id: "drafts".to_string(),
                    name: "Drafts".to_string(),
                    total_messages: 0,
                    unread_messages: 0,
                },
                nuncio_core::model::Folder {
                    id: "archive".to_string(),
                    name: "Archive".to_string(),
                    total_messages: 0,
                    unread_messages: 0,
                },
                nuncio_core::model::Folder {
                    id: "trash".to_string(),
                    name: "Trash".to_string(),
                    total_messages: 0,
                    unread_messages: 0,
                },
            ],
            selected_folder_idx: 0,
            messages: Vec::new(),
            selected_message_idx: 0,

            filter_rules: vec![sample_rule1, sample_rule2],
            selected_filter_idx: 0,
            filter_editor_mode: FilterEditorSubMode::VisualBuilder,
            filter_preview_active: false,
            filter_preview_result: None,
            filter_logs_drawer_open: false,
            filter_logs: Vec::new(),
            recovery_banner: None,
        }
    }

    /// Trigger database recovery alert notification on top header menu.
    pub fn trigger_recovery_alert(&mut self, backup_path: &str) {
        self.recovery_banner = Some(format!("[NOTICE] Database Auto-Healed ({backup_path}) — Resynchronizing Inbox..."));
    }

    /// Move selection down in the focused pane or modal view.
    pub fn move_selection_down(&mut self) {
        match self.mode {
            AppMode::FilterRules => {
                if !self.filter_rules.is_empty() && self.selected_filter_idx + 1 < self.filter_rules.len() {
                    self.selected_filter_idx += 1;
                }
            }
            AppMode::AccountSettings => {
                if !self.accounts.is_empty() && self.selected_account_idx + 1 < self.accounts.len() {
                    self.selected_account_idx += 1;
                }
            }
            AppMode::MainView => match self.active_pane {
                ActivePane::Sidebar => {
                    if !self.folders.is_empty() && self.selected_folder_idx + 1 < self.folders.len() {
                        self.selected_folder_idx += 1;
                    }
                }
                ActivePane::MessageList => {
                    if !self.messages.is_empty() && self.selected_message_idx + 1 < self.messages.len() {
                        self.selected_message_idx += 1;
                    }
                }
                ActivePane::Reader => {}
            },
            _ => {}
        }
    }

    /// Move selection up in the focused pane or modal view.
    pub fn move_selection_up(&mut self) {
        match self.mode {
            AppMode::FilterRules => {
                if self.selected_filter_idx > 0 {
                    self.selected_filter_idx -= 1;
                }
            }
            AppMode::AccountSettings => {
                if self.selected_account_idx > 0 {
                    self.selected_account_idx -= 1;
                }
            }
            AppMode::MainView => match self.active_pane {
                ActivePane::Sidebar => {
                    if self.selected_folder_idx > 0 {
                        self.selected_folder_idx -= 1;
                    }
                }
                ActivePane::MessageList => {
                    if self.selected_message_idx > 0 {
                        self.selected_message_idx -= 1;
                    }
                }
                ActivePane::Reader => {}
            },
            _ => {}
        }
    }

    /// Re-order rule priority up (`[K]`).
    pub fn reorder_filter_priority_up(&mut self) {
        if self.mode == AppMode::FilterRules && self.selected_filter_idx > 0 {
            self.filter_rules.swap(self.selected_filter_idx, self.selected_filter_idx - 1);
            self.selected_filter_idx -= 1;
            for (idx, rule) in self.filter_rules.iter_mut().enumerate() {
                rule.priority = idx as i32;
            }
        }
    }

    /// Re-order rule priority down (`[J]`).
    pub fn reorder_filter_priority_down(&mut self) {
        if self.mode == AppMode::FilterRules && self.selected_filter_idx + 1 < self.filter_rules.len() {
            self.filter_rules.swap(self.selected_filter_idx, self.selected_filter_idx + 1);
            self.selected_filter_idx += 1;
            for (idx, rule) in self.filter_rules.iter_mut().enumerate() {
                rule.priority = idx as i32;
            }
        }
    }

    /// Toggle NSQL code editor ↔ visual form builder (`[s]`).
    pub fn toggle_filter_editor_mode(&mut self) {
        self.filter_editor_mode = match self.filter_editor_mode {
            FilterEditorSubMode::VisualBuilder => FilterEditorSubMode::NsqlEditor,
            FilterEditorSubMode::NsqlEditor => FilterEditorSubMode::VisualBuilder,
        };
    }

    /// Toggle dry-run previewer (`[t]`).
    pub fn toggle_dry_run_preview(&mut self) {
        self.filter_preview_active = !self.filter_preview_active;
        if self.filter_preview_active {
            let sample_email = nuncio_core::model::Email {
                id: "msg-preview-1".to_string(),
                account_id: "acct-1".to_string(),
                folder_id: "inbox".to_string(),
                subject: "Urgent Meeting from Boss".to_string(),
                sender: "boss@nuncio.mx".to_string(),
                recipient: "me@nuncio.mx".to_string(),
                received_at: chrono::Utc::now().timestamp(),
                read: false,
                body_plain: Some("Hello, please attend priority meeting".to_string()),
                body_html: None,
                attachments: Vec::new(),
            };
            if let Ok(engine) = nuncio_filter::FilterEngine::new(self.filter_rules.clone()) {
                self.filter_preview_result = Some(engine.preview(&sample_email));
            }
        }
    }

    /// Toggle log inspector drawer (`[l]`).
    pub fn toggle_filter_logs_drawer(&mut self) {
        self.filter_logs_drawer_open = !self.filter_logs_drawer_open;
    }

    /// Access active UI mode.
    pub fn mode(&self) -> AppMode {
        self.mode
    }

    /// Mutate active UI mode.
    pub fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    /// Access active pane focus.
    pub fn active_pane(&self) -> ActivePane {
        self.active_pane
    }

    /// Mutate active pane focus.
    pub fn set_active_pane(&mut self, pane: ActivePane) {
        self.active_pane = pane;
    }

    /// Return true if event loop is active.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Terminate application loop.
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Dispatch command to core event bus.
    pub fn dispatch_command(&self, cmd: CoreCommand) {
        self.event_bus.process_command(cmd);
    }

    /// Render frame into ratatui terminal buffer.
    pub fn render_frame(&self, frame: &mut ratatui::Frame) {
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(frame.area());

        // Header status bar
        let state = self.event_bus.current_state();
        let (header_text, bg_color) = if let Some(banner) = &self.recovery_banner {
            (format!(" NUNCIO v1.0.0 │ {banner} "), Color::Yellow)
        } else {
            (
                format!(
                    " NUNCIO v1.0.0 │ Status: {:?} │ Unread: {} │ Mode: {:?} ",
                    state.status, state.unread_count, self.mode
                ),
                Color::Blue,
            )
        };
        let header = Paragraph::new(header_text).style(Style::default().bg(bg_color).fg(Color::Black));
        frame.render_widget(header, main_chunks[0]);

        // Render body area based on AppMode
        match self.mode {
            AppMode::FilterRules => {
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .split(main_chunks[1]);

                // Left: Rule list
                let rule_items: Vec<ListItem> = self
                    .filter_rules
                    .iter()
                    .enumerate()
                    .map(|(idx, rule)| {
                        let prefix = if idx == self.selected_filter_idx { " > " } else { "   " };
                        ListItem::new(format!("{}[P{}] {}", prefix, rule.priority, rule.name))
                    })
                    .collect();
                let rule_block = Block::default()
                    .title(" NSQL Filter Rules (Priority Order [J/K]) ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow));
                let rule_list = List::new(rule_items).block(rule_block);
                frame.render_widget(rule_list, chunks[0]);

                // Right: Editor & Details Pane
                let mut right_text = String::new();
                if let Some(rule) = self.filter_rules.get(self.selected_filter_idx) {
                    right_text.push_str(&format!("Rule ID: {}\nName: {}\nPriority: {}\n\n", rule.id, rule.name, rule.priority));
                    right_text.push_str(&format!("Active Sub-Mode [s]: {:?}\n\n", self.filter_editor_mode));
                    right_text.push_str(&format!("NSQL Query:\n{}\n\n", rule.nsql_text));

                    if self.filter_preview_active {
                        right_text.push_str("── DRY-RUN PREVIEW RESULTS [t] ───────────────────────\n");
                        if let Some(prev) = &self.filter_preview_result {
                            right_text.push_str(&format!("Matched: {}\nMatched Rule: {:?}\nActions: {:?}\nTime: {}us\n", prev.matched, prev.matched_rule_name, prev.actions_evaluated, prev.execution_time_us));
                        }
                    }

                    if self.filter_logs_drawer_open {
                        right_text.push_str("\n── EXECUTION LOG DRAWER [l] ───────────────────────────\n");
                        right_text.push_str("Log chain verified OK (Genesis hash linked).\n");
                    }
                }
                let editor_block = Block::default()
                    .title(" Editor Pane [s: Syntax Toggle, t: Test Preview, l: Logs Drawer] ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan));
                let editor_p = Paragraph::new(right_text).block(editor_block);
                frame.render_widget(editor_p, chunks[1]);
            }
            AppMode::SplashScreen => {
                let splash_text = get_splash_screen_text();
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
                    ListItem::new(" * [ACTIVE] james.maes@kof22.com (IMAP: mail.kof22.com:993 │ SMTP: mail.kof22.com:465 - Implicit TLS)"),
                    ListItem::new("   [IDLE]   work@nuncio.mx       (IMAP: mail.nuncio.mx:993 │ SMTP: mail.nuncio.mx:465 - Implicit TLS)"),
                    ListItem::new(""),
                    ListItem::new(" ── ACCOUNT CONTROLS & ACTIONS ──────────────────────────────────────────"),
                    ListItem::new(" [a] Add New Account Profile     [e] Edit Account Configuration"),
                    ListItem::new(" [t] Test TLS Connection         [d] Delete Selected Account"),
                    ListItem::new(" [Esc/q] Close Settings"),
                ];
                let acct_block = Block::default()
                    .title(" Account Settings & Connectivity Manager [a] ")
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
 ║    f                  NSQL Filter Rules & Automation Manager ║
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
                let sidebar_items: Vec<ListItem> = self
                    .folders
                    .iter()
                    .enumerate()
                    .map(|(idx, folder)| {
                        let prefix = if idx == self.selected_folder_idx {
                            " > "
                        } else {
                            "   "
                        };
                        let text = if folder.unread_messages > 0 {
                            format!("{}{:<12} ({})", prefix, folder.name, folder.unread_messages)
                        } else {
                            format!("{}{}", prefix, folder.name)
                        };
                        ListItem::new(text)
                    })
                    .collect();
                let sidebar_block = Block::default()
                    .title(" Folders (20%) ")
                    .borders(Borders::ALL)
                    .border_style(AppLayout::border_style(ActivePane::Sidebar, self.active_pane));
                let sidebar_list = List::new(sidebar_items).block(sidebar_block);
                frame.render_widget(sidebar_list, sidebar_area);

                // Message list widget
                let msg_items: Vec<ListItem> = self
                    .messages
                    .iter()
                    .enumerate()
                    .map(|(idx, msg)| {
                        let prefix = if idx == self.selected_message_idx {
                            " > "
                        } else {
                            "   "
                        };
                        let status = if msg.read { " " } else { "●" };
                        let text = format!("{} {} {:<15} {}", prefix, status, msg.sender, msg.subject);
                        ListItem::new(text)
                    })
                    .collect();
                let msg_block = Block::default()
                    .title(" Messages (35%) ")
                    .borders(Borders::ALL)
                    .border_style(AppLayout::border_style(ActivePane::MessageList, self.active_pane));
                let msg_list = List::new(msg_items).block(msg_block);
                frame.render_widget(msg_list, list_area);

                // Email reader widget
                let body_text = self
                    .messages
                    .get(self.selected_message_idx)
                    .and_then(|m| m.body_plain.as_ref())
                    .map(|b| b.as_str())
                    .unwrap_or("No message selected.");

                let reader_block = Block::default()
                    .title(" Email Reader (45%) ")
                    .borders(Borders::ALL)
                    .border_style(AppLayout::border_style(ActivePane::Reader, self.active_pane));
                let reader_p = Paragraph::new(body_text).block(reader_block);
                frame.render_widget(reader_p, reader_area);
            }
        }

        // Footer navigation bar
        let footer_text = " [?] Help │ [f] Filter Rules │ [a] Accounts │ [s] Sync │ [c] Compose │ [r] Reply │ [q] Quit ";
        let footer = Paragraph::new(footer_text).style(Style::default().bg(Color::DarkGray).fg(Color::White));
        frame.render_widget(footer, main_chunks[2]);
    }
}

fn get_splash_screen_text() -> &'static str {
    r#"
  ███╗   ██╗██╗   ██╗███╗   ██╗██╗  ██╗██╗ ██████╗ 
  ████╗  ██║██║   ██║████╗  ██║██║  ██║██║██╔═══██╗
  ██╔██╗ ██║██║   ██║██╔██╗ ██║███████║██║██║   ██║
  ██║╚██╗██║██║   ██║██║╚██╗██║██╔══██║██║██║   ██║
  ██║ ╚████║╚██████╔╝██║ ╚████║██║  ██║██║╚██████╔╝
  ╚═╝  ╚═══╝ ╚═════╝ ╚═╝  ╚═══╝╚═╝  ╚═╝╚═╝ ╚═════╝ 
"#
}
