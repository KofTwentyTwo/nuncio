use nuncio_filter::{ValidationOptions, WebhookDispatcher, WebhookError};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_webhook_dispatch_success_with_hmac_and_headers() {
    // 1. Start mock HTTP server
    let mock_server = MockServer::start().await;

    // 2. Configure mock endpoint expectation
    Mock::given(method("POST"))
        .and(path("/hooks/nuncio"))
        .and(header("Content-Type", "application/json"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let target_url = format!("{}/hooks/nuncio", mock_server.uri());

    // 3. Instantiate WebhookDispatcher with secret key
    let dispatcher = WebhookDispatcher::new("super_secret_hmac_key_99");

    let test_opts = ValidationOptions {
        block_private_webhooks: false, // allow 127.0.0.1 mock server in unit test
        ..Default::default()
    };

    // 4. Dispatch webhook action
    let status = dispatcher
        .dispatch_with_options(
            &target_url,
            "rule_01VIP",
            "msg_9901",
            "Urgent Architecture Review",
            "alice@kof22.com",
            &test_opts,
        )
        .await
        .expect("webhook dispatch success");

    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_webhook_ssrf_blocked_address() {
    let dispatcher = WebhookDispatcher::new("secret_key");

    let blocked_urls = vec![
        "http://127.0.0.1:8080/steal",
        "http://localhost:3000/api",
        "http://169.254.169.254/latest/meta-data",
        "http://0.0.0.0/internal",
    ];

    for url in blocked_urls {
        let err = dispatcher
            .dispatch(url, "rule_1", "msg_1", "Test", "a@b.com")
            .await
            .unwrap_err();

        assert!(
            matches!(err, WebhookError::SecurityViolation(_)),
            "Expected SecurityViolation for {url}"
        );
    }
}
