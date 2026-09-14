use crate::{accounts::AccountError, store::StoreError};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Account(#[from] AccountError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error("Provider is unavailable")]
    Unavailable,
    #[error("IMAP mailbox changed during synchronization; retry the sync")]
    MailboxChanged,
    #[error("Google provider requested a later attempt")]
    RetryAfter(i64),
    #[error("Provider returned an invalid response")]
    Provider,
    #[error("Provider resource was not found")]
    NotFound,
    #[error("Content exceeds the configured payload limit")]
    TooLarge,
    #[error("Sync was cancelled")]
    Cancelled,
    #[error("Gmail history expired during reconciliation")]
    HistoryExpired,
    #[error("Google Calendar synchronization cursor expired")]
    CalendarExpired,
}
impl SyncError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Account(e) => e.code(),
            Self::Storage(_) => "storage",
            Self::Unavailable | Self::RetryAfter(_) => "unavailable",
            Self::MailboxChanged => "mailbox_changed",
            Self::Provider => "invalid_provider_response",
            Self::NotFound => "not_found",
            Self::TooLarge => "too_large",
            Self::Cancelled => "cancelled",
            Self::HistoryExpired => "history_expired",
            Self::CalendarExpired => "calendar_cursor_expired",
        }
    }
}
