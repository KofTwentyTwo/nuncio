//! Outbound HTTP Webhook Execution Engine with HMAC-SHA256 Signatures and SSRF Defense.

use crate::ast::RuleAction;
use crate::validator::{is_blocked_webhook_target, NsqlValidator, ValidationOptions};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::json;
use sha2::Sha256;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::net::lookup_host;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

/// Returns the first address in `addrs` that is a blocked webhook target, if
/// any. Split out from the DNS lookup so the egress policy can be exercised
/// against synthetic address lists without touching the network.
fn first_blocked_address(addrs: &[IpAddr]) -> Option<IpAddr> {
    addrs
        .iter()
        .copied()
        .find(|ip| is_blocked_webhook_target(*ip))
}

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
    secret_key: Zeroizing<String>,
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
            secret_key: Zeroizing::new(secret_key.into()),
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

        if opts.block_private_webhooks {
            // Pass 6 only sees literal-IP hosts. Resolve the target here and
            // reject it if a hostname points into a blocked range, closing the
            // DNS-rebind class of SSRF bypass. This is not airtight: reqwest
            // re-resolves and connects on its own after this check, so a narrow
            // TOCTOU window remains where DNS could flip between our lookup and
            // the actual connection. Fully closing it needs a pinned
            // resolver/connector, which is out of scope here.
            if let Ok(parsed) = reqwest::Url::parse(url) {
                if let Some(host) = parsed.host_str() {
                    // `host_str` brackets IPv6 literals (`[::1]`); strip them so
                    // the pair passes to the resolver cleanly.
                    let host = host.trim_start_matches('[').trim_end_matches(']');
                    let port = parsed.port_or_known_default().unwrap_or(0);
                    let resolved: Vec<IpAddr> = lookup_host((host, port))
                        .await
                        .map_err(|e| WebhookError::NetworkError(e.to_string()))?
                        .map(|addr| addr.ip())
                        .collect();
                    if let Some(blocked) = first_blocked_address(&resolved) {
                        return Err(WebhookError::SecurityViolation(format!(
                            "webhook host '{host}' resolves to blocked address {blocked}"
                        )));
                    }
                }
            }
        }

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
            .header(
                "X-Nuncio-Signature",
                format!("t={timestamp},v1={signature}"),
            )
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
        self.dispatch_with_options(
            url,
            rule_id,
            message_id,
            subject,
            sender,
            &ValidationOptions::default(),
        )
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
            .dispatch(
                "http://127.0.0.1/steal",
                "rule_1",
                "msg_1",
                "Test",
                "a@b.com",
            )
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WebhookError::SecurityViolation(_)
        ));
    }

    #[test]
    fn test_first_blocked_address_flags_private_in_mixed_list() {
        // Synthetic resolver output: a public address alongside a private one,
        // as a rebinding host might return. The private one must be caught.
        let addrs: Vec<IpAddr> = vec![
            "93.184.216.34".parse().unwrap(),
            "10.0.0.5".parse().unwrap(),
        ];
        assert_eq!(
            first_blocked_address(&addrs),
            Some("10.0.0.5".parse().unwrap())
        );
    }

    #[test]
    fn test_first_blocked_address_allows_all_public() {
        let addrs: Vec<IpAddr> = vec![
            "93.184.216.34".parse().unwrap(),
            "172.32.0.1".parse().unwrap(),
        ];
        assert_eq!(first_blocked_address(&addrs), None);
    }
}
