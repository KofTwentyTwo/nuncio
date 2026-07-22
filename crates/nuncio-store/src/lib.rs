use thiserror::Error;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Database error: {0}")]
    Database(String),
}

pub struct StorageEngine;

impl StorageEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::new()
    }
}
