//! Outbound HTTP Webhook Execution Engine with HMAC-SHA256 Signatures and SSRF Defense.

use crate::ast::RuleAction;
use crate::validator::{is_blocked_webhook_target, NsqlValidator, ValidationOptions};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::json;
use sha2::Sha256;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::net::lookup_host;
use tracing::{debug, warn, Instrument};
use zeroize::Zeroizing;

/// Extract just the host for logging (never the path/query, which may carry
/// caller-supplied tokens), falling back to a placeholder when the URL itself
/// cannot be parsed.
fn webhook_host_for_log(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_string())
        .unwrap_or_else(|| "unparsable-url".to_string())
}

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

/// Resolves `host` to its candidate addresses. Kept separate from the policy
/// check so the address list a rebinding DNS server could return is visible
/// to callers before any of it is trusted.
async fn resolve_host(host: &str, port: u16) -> Result<Vec<IpAddr>, WebhookError> {
    let resolved: Vec<IpAddr> = lookup_host((host, port))
        .await
        .map_err(|e| WebhookError::NetworkError(e.to_string()))?
        .map(|addr| addr.ip())
        .collect();
    if resolved.is_empty() {
        return Err(WebhookError::NetworkError(format!(
            "webhook host '{host}' did not resolve to any address"
        )));
    }
    Ok(resolved)
}

/// Builds an HTTP client whose connection to `host` is pinned to `addr`.
/// `reqwest`'s DNS override maps only the transport-level connect address; it
/// still sends the original hostname as the TLS SNI and `Host` header, so
/// certificate validation is unaffected. This is what stops the dispatcher
/// from re-resolving `host` after it has been checked against the SSRF
/// policy: the checked address is the only one the connection can use.
fn pinned_client(host: &str, addr: IpAddr, port: u16) -> Result<Client, WebhookError> {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, SocketAddr::new(addr, port))
        .build()
        .map_err(|e| WebhookError::NetworkError(e.to_string()))
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
///
/// Each dispatch builds its own client pinned to the address it just
/// resolved and checked (see [`pinned_client`]), so there is no shared
/// client to cache here.
pub struct WebhookDispatcher {
    secret_key: Zeroizing<String>,
}

impl WebhookDispatcher {
    /// Create a new `WebhookDispatcher` with an HMAC signing key.
    pub fn new(secret_key: impl Into<String>) -> Self {
        Self {
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
        let host_for_log = webhook_host_for_log(url);
        let span =
            tracing::info_span!("webhook_dispatch", host = %host_for_log, rule_id = %rule_id);

        async move {
            let action = RuleAction::CallWebhook(url.to_string());
            if let Err(e) = NsqlValidator::pass6_action_security(&[action], opts) {
                warn!(
                    host = %host_for_log,
                    reason = %e,
                    "webhook egress blocked by SSRF/security policy"
                );
                return Err(WebhookError::SecurityViolation(e.to_string()));
            }

            // Pass 6 only sees literal-IP hosts. Resolve the target here and,
            // when the policy is enabled, reject it if it points into a blocked
            // range - closing the DNS-rebind class of SSRF bypass that a
            // substring/literal check on the URL cannot catch. The resolved
            // address is then pinned below for the actual connection, so the
            // dispatch cannot re-resolve the hostname and land on a different,
            // unchecked address: the address that was checked is the only one
            // `reqwest` is permitted to connect to.
            let parsed = reqwest::Url::parse(url).map_err(|e| {
                WebhookError::SecurityViolation(format!("invalid webhook URL: {e}"))
            })?;
            let host = parsed
                .host_str()
                .ok_or_else(|| {
                    WebhookError::SecurityViolation("webhook URL has no host".to_string())
                })?
                // `host_str` brackets IPv6 literals (`[::1]`); strip them so the
                // pair passes to the resolver cleanly.
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            let port = parsed.port_or_known_default().unwrap_or(0);

            let resolved = resolve_host(&host, port).await?;
            if opts.block_private_webhooks {
                if let Some(blocked) = first_blocked_address(&resolved) {
                    warn!(
                        host = %host,
                        blocked_ip = %blocked,
                        "webhook egress blocked: resolved address is in a disallowed range (DNS-rebind guard)"
                    );
                    return Err(WebhookError::SecurityViolation(format!(
                        "webhook host '{host}' resolves to blocked address {blocked}"
                    )));
                }
            }
            let pinned_addr = resolved[0];
            debug!(
                host = %host,
                resolved_ip = %pinned_addr,
                "webhook egress allowed by SSRF/security policy"
            );
            let client = pinned_client(&host, pinned_addr, port)?;

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

            // Stable across every retry of this rule/message pair, so a
            // receiver can discard a redelivery caused by a lost response
            // rather than acting twice. It is NOT yet stable across daemons:
            // `rule_id` is random per daemon, so two daemons executing the
            // same rule produce different keys. Single-owner filter execution
            // is what prevents that case today; a rule key derived from the
            // normalized NSQL would make it dedupable directly.
            let idempotency_key = {
                let mut hasher = <Sha256 as sha2::Digest>::new();
                for field in [rule_id, message_id] {
                    sha2::Digest::update(&mut hasher, (field.len() as u64).to_le_bytes());
                    sha2::Digest::update(&mut hasher, field.as_bytes());
                }
                hex::encode(sha2::Digest::finalize(hasher))
            };

            let send_result = client
                .post(url)
                .header("Content-Type", "application/json")
                .header("Idempotency-Key", &idempotency_key)
                .header(
                    "X-Nuncio-Signature",
                    format!("t={timestamp},v1={signature}"),
                )
                .body(payload_str)
                .send()
                .await;

            match send_result {
                Ok(response) => {
                    let status = response.status().as_u16();
                    debug!(host = %host, status, "webhook dispatched");
                    Ok(status)
                }
                Err(e) => {
                    warn!(host = %host, error = %e, "webhook dispatch network error");
                    Err(WebhookError::NetworkError(e.to_string()))
                }
            }
        }
        .instrument(span)
        .await
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_pinned_client_connects_to_checked_ip_not_dns() {
        // `.invalid` is reserved by RFC 2606 to never resolve: any real DNS
        // lookup for it fails. The mock server only listens on its loopback
        // address, which has no relationship to that hostname in any
        // resolver. If the request below reaches the mock server, it can
        // only be because `pinned_client`'s resolve override - not DNS -
        // supplied the connect address, proving the dispatcher connects to
        // exactly the address it already checked.
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let addr = *mock_server.address();
        let host = "checked-ip-pin.invalid";
        let client = pinned_client(host, addr.ip(), addr.port())
            .expect("building a client with a resolve override must succeed");

        let url = format!("http://{host}:{}/hook", addr.port());
        let response = client
            .post(&url)
            .send()
            .await
            .expect("the pinned connection must reach the mock server, not a DNS lookup");

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_dispatch_reaches_pinned_loopback_mock_server() {
        // End-to-end proof that `dispatch_with_options` itself (not just the
        // `pinned_client` helper) drives the request to the address it
        // resolved and checked. `block_private_webhooks` is disabled here
        // only to allow the test to target a loopback mock server; it does
        // not affect whether the connection is pinned.
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&mock_server)
            .await;

        let dispatcher = WebhookDispatcher::new("secret_key_123");
        let opts = ValidationOptions {
            available_folders: None,
            allowed_forward_domains: None,
            block_private_webhooks: false,
        };
        let url = format!("{}/hook", mock_server.uri());
        let status = dispatcher
            .dispatch_with_options(&url, "rule_1", "msg_1", "Test", "a@b.com", &opts)
            .await
            .expect("dispatch to the mock server must succeed");

        assert_eq!(status, 202);
    }

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

    #[tokio::test]
    async fn test_blocked_webhook_logs_warn_with_host_and_reason() {
        let logs = crate::test_tracing::capture_logs();

        let dispatcher = WebhookDispatcher::new("secret_key_123");
        let result = dispatcher
            .dispatch(
                "http://169.254.169.254/latest/meta-data",
                "rule_1",
                "msg_1",
                "Test",
                "a@b.com",
            )
            .await;

        assert!(matches!(result, Err(WebhookError::SecurityViolation(_))));

        let captured = logs.lines();
        assert!(
            captured.iter().any(|line| line.contains("WARN")
                && line.contains("blocked")
                && line.contains("169.254.169.254")),
            "expected a WARN log carrying the blocked host and reason, got: {captured:?}"
        );
        assert!(
            !captured.iter().any(|line| line.contains("secret_key_123")),
            "the webhook HMAC secret must never appear in a log line"
        );
    }

    #[tokio::test]
    async fn test_allowed_webhook_logs_debug_not_warn() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let logs = crate::test_tracing::capture_logs();

        let dispatcher = WebhookDispatcher::new("secret_key_123");
        let opts = ValidationOptions {
            available_folders: None,
            allowed_forward_domains: None,
            block_private_webhooks: false,
        };
        let url = format!("{}/hook", mock_server.uri());
        let status = dispatcher
            .dispatch_with_options(&url, "rule_1", "msg_1", "Test", "a@b.com", &opts)
            .await
            .expect("dispatch to the mock server must succeed");
        assert_eq!(status, 200);

        let captured = logs.lines();
        assert!(
            captured
                .iter()
                .any(|line| line.contains("DEBUG") && line.contains("allowed")),
            "expected a DEBUG log for the allowed egress decision, got: {captured:?}"
        );
        assert!(
            !captured.iter().any(|line| line.contains("WARN")),
            "an allowed dispatch must not emit a WARN, got: {captured:?}"
        );
    }
}
