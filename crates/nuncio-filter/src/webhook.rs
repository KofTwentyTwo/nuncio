//! Outbound HTTP Webhook Execution Engine with HMAC-SHA256 Signatures and SSRF Defense.

use crate::ast::RuleAction;
use crate::validator::{NsqlValidator, ValidationOptions};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::json;
use sha2::Sha256;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Errors emitted during HTTP webhook dispatch execution.
#[derive(Error, Debug)]
pub enum WebhookError {
    /// Validation security check failure.
    #[error("Security policy violation: {0}")]
    SecurityViolation(String),
    /// HMAC cryptographic signature error.
    #[error("Cryptographic signing error: {0}")]
    SigningError(String),
    /// Network HTTP dispatch error.
    #[error("HTTP dispatch failure: {0}")]
    NetworkError(String),
}

/// Outbound Webhook Dispatcher executing authenticated HTTP POST requests.
pub struct WebhookDispatcher {
    client: Client,
    secret_key: String,
}

impl WebhookDispatcher {
    /// Create a new `WebhookDispatcher` with an HMAC signing key.
    pub fn new(secret_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();

        Self {
            client,
            secret_key: secret_key.into(),
        }
    }

    /// Dispatch a `CALL WEBHOOK` action with options and payload.
    pub async fn dispatch_with_options(
        &self,
        url: &str,
        rule_id: &str,
        message_id: &str,
        subject: &str,
        sender: &str,
        opts: &ValidationOptions,
    ) -> Result<u16, WebhookError> {
        let action = RuleAction::CallWebhook(url.to_string());
        NsqlValidator::pass6_action_security(&[action], opts)
            .map_err(|e| WebhookError::SecurityViolation(e.to_string()))?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let payload = json!({
            "event": "nuncio.filter.matched",
            "timestamp": timestamp,
            "rule_id": rule_id,
            "message_id": message_id,
            "subject": subject,
            "sender": sender,
        });

        let payload_str = payload.to_string();

        let mut mac = HmacSha256::new_from_slice(self.secret_key.as_bytes())
            .map_err(|e| WebhookError::SigningError(e.to_string()))?;
        mac.update(format!("{timestamp}.{payload_str}").as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("X-Nuncio-Signature", format!("t={timestamp},v1={signature}"))
            .body(payload_str)
            .send()
            .await
            .map_err(|e| WebhookError::NetworkError(e.to_string()))?;

        Ok(response.status().as_u16())
    }

    /// Dispatch a `CALL WEBHOOK` action with default production security options.
    pub async fn dispatch(
        &self,
        url: &str,
        rule_id: &str,
        message_id: &str,
        subject: &str,
        sender: &str,
    ) -> Result<u16, WebhookError> {
        self.dispatch_with_options(url, rule_id, message_id, subject, sender, &ValidationOptions::default())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_blocked_private_ip_webhook() {
        let dispatcher = WebhookDispatcher::new("secret_key_123");
        let result = dispatcher
            .dispatch("http://127.0.0.1/steal", "rule_1", "msg_1", "Test", "a@b.com")
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WebhookError::SecurityViolation(_)));
    }
}
