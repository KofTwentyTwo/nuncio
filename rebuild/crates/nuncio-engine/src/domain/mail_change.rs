use serde::{Deserialize, Serialize};

pub struct MailCapabilities {
    pub account_id: String,
    pub provider: String,
    pub placement_model: String,
    pub read_unread: bool,
    pub star_unstar: bool,
    pub archive: bool,
    pub trash_restore: bool,
    pub existing_labels: bool,
    pub move_copy: bool,
}

/// Explicit desired states, never toggles based on a potentially stale cache.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum MailAction {
    Read {
        read: bool,
    },
    Star {
        starred: bool,
    },
    Archive {},
    Move {
        destination_collection_id: String,
    },
    Copy {
        destination_collection_id: String,
    },
    Trash {
        trashed: bool,
    },
    Label {
        collection_id: String,
        present: bool,
    },
}
