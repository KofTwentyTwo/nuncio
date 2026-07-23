//! Keyboard navigation, Vim-style shortcuts, and command dispatch engine.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Action events mapped from terminal key inputs.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    /// Move selection down (j / Down).
    MoveDown,
    /// Move selection up (k / Up).
    MoveUp,
    /// Jump to top of list (gg).
    JumpTop,
    /// Jump to bottom of list (G).
    JumpBottom,
    /// Jump to Inbox folder (gi).
    JumpInbox,
    /// Jump to Sent folder (gs).
    JumpSent,
    /// Jump to Archive folder (ga).
    JumpArchive,
    /// Switch to next pane (Tab).
    NextPane,
    /// Switch to previous pane (BackTab).
    PreviousPane,
    /// Open search mode (/).
    Search,
    /// Compose new email message (c).
    Compose,
    /// Reply to selected message (r).
    Reply,
    /// Refresh / sync (s).
    Sync,
    /// Toggle help modal overlay (? / h).
    ToggleHelp,
    /// Toggle accounts / settings view (a).
    ToggleAccounts,
    /// Toggle splash screen view (p).
    ToggleSplash,
    /// Toggle filter rules manager view (f).
    ToggleFilterRules,
    /// Toggle NSQL syntax / editor mode (s).
    ToggleFilterSyntax,
    /// Test dry-run filter preview (t).
    TestFilterPreview,
    /// Re-order rule priority up (K).
    ReorderPriorityUp,
    /// Re-order rule priority down (J).
    ReorderPriorityDown,
    /// Toggle filter execution logs drawer (l).
    ToggleFilterLogs,
    /// Trigger software update check and application (u).
    TriggerUpdate,
    /// Exit application or modal (q / Esc).
    Quit,
    /// Unmapped key action.
    None,
}

/// Keybinding translation engine mapping crossterm input events to [`UserAction`].
#[allow(dead_code)]
pub struct KeybindingEngine;

impl KeybindingEngine {
    /// Translate a crossterm [`KeyEvent`] into a [`UserAction`].
    #[allow(dead_code)]
    pub fn handle_key(key: KeyEvent) -> UserAction {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) | (KeyCode::Esc, _) => UserAction::Quit,
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => UserAction::MoveDown,
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => UserAction::MoveUp,
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                UserAction::PreviousPane
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => UserAction::NextPane,
            (KeyCode::Char('g'), KeyModifiers::NONE) => UserAction::JumpTop,
            (KeyCode::Char('G'), KeyModifiers::SHIFT) => UserAction::JumpBottom,
            (KeyCode::Tab, KeyModifiers::NONE) => UserAction::NextPane,
            (KeyCode::BackTab, _) => UserAction::PreviousPane,
            (KeyCode::Char('?'), KeyModifiers::NONE) => UserAction::ToggleHelp,
            (KeyCode::Char('a'), KeyModifiers::NONE) => UserAction::ToggleAccounts,
            (KeyCode::Char('f'), KeyModifiers::NONE) => UserAction::ToggleFilterRules,
            (KeyCode::Char('p'), KeyModifiers::NONE) => UserAction::ToggleSplash,
            (KeyCode::Char('u'), KeyModifiers::NONE) => UserAction::TriggerUpdate,
            (KeyCode::Char('/'), KeyModifiers::NONE) => UserAction::Search,
            (KeyCode::Char('c'), KeyModifiers::NONE) => UserAction::Compose,
            (KeyCode::Char('r'), KeyModifiers::NONE) => UserAction::Reply,
            (KeyCode::Char('s'), KeyModifiers::NONE) => UserAction::ToggleFilterSyntax,
            (KeyCode::Char('t'), KeyModifiers::NONE) => UserAction::TestFilterPreview,
            (KeyCode::Char('K'), KeyModifiers::SHIFT) => UserAction::ReorderPriorityUp,
            (KeyCode::Char('J'), KeyModifiers::SHIFT) => UserAction::ReorderPriorityDown,
            _ => UserAction::None,
        }
    }
}

/// Vim leader chord state machine for multi-key sequences (`gg`, `gi`, `gs`, `ga`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeaderState {
    #[default]
    Idle,
    PendingG,
}

/// Stateful Vim leader chord key processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VimLeaderStateMachine {
    state: LeaderState,
}

impl VimLeaderStateMachine {
    /// Construct a new `VimLeaderStateMachine`.
    pub fn new() -> Self {
        Self {
            state: LeaderState::Idle,
        }
    }

    /// Access current leader state.
    pub fn state(&self) -> LeaderState {
        self.state
    }

    /// Process a incoming key event through the leader chord state machine.
    pub fn process_key(&mut self, key: KeyEvent) -> UserAction {
        match (self.state, key.code, key.modifiers) {
            (LeaderState::Idle, KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.state = LeaderState::PendingG;
                UserAction::None
            }
            (LeaderState::PendingG, KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.state = LeaderState::Idle;
                UserAction::JumpTop
            }
            (LeaderState::PendingG, KeyCode::Char('i'), KeyModifiers::NONE) => {
                self.state = LeaderState::Idle;
                UserAction::JumpInbox
            }
            (LeaderState::PendingG, KeyCode::Char('s'), KeyModifiers::NONE) => {
                self.state = LeaderState::Idle;
                UserAction::JumpSent
            }
            (LeaderState::PendingG, KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.state = LeaderState::Idle;
                UserAction::JumpArchive
            }
            (LeaderState::PendingG, _, _) => {
                self.state = LeaderState::Idle;
                KeybindingEngine::handle_key(key)
            }
            (LeaderState::Idle, _, _) => KeybindingEngine::handle_key(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn translate_vim_motion_keys() {
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('j'), KeyModifiers::NONE)),
            UserAction::MoveDown
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('k'), KeyModifiers::NONE)),
            UserAction::MoveUp
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('g'), KeyModifiers::NONE)),
            UserAction::JumpTop
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            UserAction::JumpBottom
        );
    }

    #[test]
    fn vim_leader_chords_gg_gi_gs_ga() {
        let mut sm = VimLeaderStateMachine::new();

        // Single 'g' puts state machine in PendingG
        assert_eq!(sm.process_key(make_key(KeyCode::Char('g'), KeyModifiers::NONE)), UserAction::None);
        assert_eq!(sm.state(), LeaderState::PendingG);

        // 'g' + 'g' -> JumpTop
        assert_eq!(sm.process_key(make_key(KeyCode::Char('g'), KeyModifiers::NONE)), UserAction::JumpTop);
        assert_eq!(sm.state(), LeaderState::Idle);

        // 'g' + 'i' -> JumpInbox
        assert_eq!(sm.process_key(make_key(KeyCode::Char('g'), KeyModifiers::NONE)), UserAction::None);
        assert_eq!(sm.process_key(make_key(KeyCode::Char('i'), KeyModifiers::NONE)), UserAction::JumpInbox);

        // 'g' + 's' -> JumpSent
        assert_eq!(sm.process_key(make_key(KeyCode::Char('g'), KeyModifiers::NONE)), UserAction::None);
        assert_eq!(sm.process_key(make_key(KeyCode::Char('s'), KeyModifiers::NONE)), UserAction::JumpSent);

        // 'g' + 'a' -> JumpArchive
        assert_eq!(sm.process_key(make_key(KeyCode::Char('g'), KeyModifiers::NONE)), UserAction::None);
        assert_eq!(sm.process_key(make_key(KeyCode::Char('a'), KeyModifiers::NONE)), UserAction::JumpArchive);
    }

    #[test]
    fn translate_pane_navigation_and_quit() {
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Tab, KeyModifiers::NONE)),
            UserAction::NextPane
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::BackTab, KeyModifiers::NONE)),
            UserAction::PreviousPane
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('q'), KeyModifiers::NONE)),
            UserAction::Quit
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Esc, KeyModifiers::NONE)),
            UserAction::Quit
        );
    }

    #[test]
    fn translate_command_shortcuts() {
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('/'), KeyModifiers::NONE)),
            UserAction::Search
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('c'), KeyModifiers::NONE)),
            UserAction::Compose
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('r'), KeyModifiers::NONE)),
            UserAction::Reply
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('z'), KeyModifiers::NONE)),
            UserAction::None
        );
    }

    #[test]
    fn translate_arrow_keys_and_modal_shortcuts() {
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Down, KeyModifiers::NONE)),
            UserAction::MoveDown
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Up, KeyModifiers::NONE)),
            UserAction::MoveUp
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Left, KeyModifiers::NONE)),
            UserAction::PreviousPane
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Right, KeyModifiers::NONE)),
            UserAction::NextPane
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('?'), KeyModifiers::NONE)),
            UserAction::ToggleHelp
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('a'), KeyModifiers::NONE)),
            UserAction::ToggleAccounts
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('p'), KeyModifiers::NONE)),
            UserAction::ToggleSplash
        );
        assert_eq!(
            KeybindingEngine::handle_key(make_key(KeyCode::Char('u'), KeyModifiers::NONE)),
            UserAction::TriggerUpdate
        );
    }
}
