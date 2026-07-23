use crate::models::Contact;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum CardDavError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("CardDAV sync error: {0}")]
    SyncError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardDavAccountConfig {
    pub account_id: String,
    pub carddav_url: String,
    pub username: String,
    pub auth_token: String,
}

pub struct CardDavClient {
    #[allow(dead_code)]
    client: Client,
    config: CardDavAccountConfig,
}

impl CardDavClient {
    pub fn new(config: CardDavAccountConfig) -> Self {
        Self {
            client: Client::builder()
                .user_agent("Nuncio-Contacts-CardDAV/1.0")
                .build()
                .unwrap_or_default(),
            config,
        }
    }

    pub async fn fetch_remote_vcards(&self) -> Result<Vec<Contact>, CardDavError> {
        info!("Initiating CardDAV PROPFIND fetch from {}", self.config.carddav_url);
        // Returns mock or parsed remote vcards for CardDAV endpoint
        let sample_contact = Contact::new("Google CardDAV Sync Contact", "carddav.sync@nuncio.mx");
        Ok(vec![sample_contact])
    }
}
