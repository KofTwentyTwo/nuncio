use thiserror::Error;

#[derive(Error, Debug)]
pub enum CalendarError {
    #[error("Calendar sync error: {0}")]
    Sync(String),
}

pub struct CalendarEngine;

impl CalendarEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CalendarEngine {
    fn default() -> Self {
        Self::new()
    }
}
