use thiserror::Error;

#[derive(Error, Debug)]
pub enum MailError {
    #[error("Connection error: {0}")]
    Connection(String),
}

pub struct MailEngine;

impl MailEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MailEngine {
    fn default() -> Self {
        Self::new()
    }
}
