//! React/Vite Webview component layout model and state bindings.

use nuncio_core::model::{Email, Folder};
use serde::{Deserialize, Serialize};

/// Frontend split-view layout state container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiViewState {
    /// Available folders list.
    pub folders: Vec<Folder>,
    /// Active selected folder ID.
    pub active_folder_id: String,
    /// Loaded email message list.
    pub messages: Vec<Email>,
    /// Currently selected email ID for reading preview.
    pub selected_message_id: Option<String>,
    /// Dark mode UI theme preference flag.
    pub dark_mode: bool,
}

impl GuiViewState {
    /// Create a new default GUI view state.
    pub fn new() -> Self {
        Self {
            folders: vec![
                Folder {
                    id: "inbox".to_string(),
                    name: "Inbox".to_string(),
                    total_messages: 5,
                    unread_messages: 2,
                },
                Folder {
                    id: "sent".to_string(),
                    name: "Sent".to_string(),
                    total_messages: 3,
                    unread_messages: 0,
                },
            ],
            active_folder_id: "inbox".to_string(),
            messages: Vec::new(),
            selected_message_id: None,
            dark_mode: true,
        }
    }

    /// Select active folder.
    #[allow(dead_code)]
    pub fn select_folder(&mut self, folder_id: &str) {
        self.active_folder_id = folder_id.to_string();
        self.selected_message_id = None;
    }

    /// Select active message preview.
    #[allow(dead_code)]
    pub fn select_message(&mut self, message_id: &str) {
        self.selected_message_id = Some(message_id.to_string());
    }
}

impl Default for GuiViewState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_view_state_navigation() {
        let mut state = GuiViewState::default();

        assert_eq!(state.active_folder_id, "inbox");
        assert!(state.dark_mode);

        state.select_folder("sent");
        assert_eq!(state.active_folder_id, "sent");
        assert!(state.selected_message_id.is_none());

        state.select_message("msg-101");
        assert_eq!(state.selected_message_id, Some("msg-101".to_string()));
    }
}
