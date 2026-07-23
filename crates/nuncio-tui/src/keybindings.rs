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
            (KeyCode::Char('p'), KeyModifiers::NONE) => UserAction::ToggleSplash,
            (KeyCode::Char('/'), KeyModifiers::NONE) => UserAction::Search,
            (KeyCode::Char('c'), KeyModifiers::NONE) => UserAction::Compose,
            (KeyCode::Char('r'), KeyModifiers::NONE) => UserAction::Reply,
            (KeyCode::Char('s'), KeyModifiers::NONE) => UserAction::Sync,
            _ => UserAction::None,
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
    }
}
