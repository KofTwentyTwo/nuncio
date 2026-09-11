use base64::{engine::general_purpose::STANDARD, Engine as _};
use nuncio_test_support::{imap::MockMailPlus, TestError};
use serde_json::json;

#[tokio::test]
async fn independent_mail_service_controller_reproduces_provider_state() -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&["MOVE", "UIDPLUS"]).await?;
    assert_ne!(mock.ready.ports.imaps, mock.ready.ports.imap);
    assert_ne!(mock.ready.ports.smtp, mock.ready.ports.smtps);
    assert!(mock.ready.ca_file.is_file());
    assert!(mock.ready.credentials_file.is_file());
    assert_eq!(mock.ready.hidden_capabilities, ["MOVE", "UIDPLUS"]);
    let inbox = json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"});
    let initial = mock.control(inbox.clone()).await?;
    assert_eq!(initial["uidvalidity"], 9001);
    assert_eq!(initial["messages"].as_array().map(Vec::len), Some(2));
    let raw = mock
        .control(json!({"command":"raw","account":"alpha@example.test","mailbox":"INBOX","uid":1}))
        .await?;
    let encoded = raw["raw_base64"].as_str().ok_or("missing raw fixture")?;
    let bytes = STANDARD.decode(encoded)?;
    assert!(bytes
        .windows(b"application/pdf".len())
        .any(|v| v == b"application/pdf"));
    mock.control(json!({"command":"flags","account":"alpha@example.test","mailbox":"INBOX","uid":1,"add":["\\Seen"],"remove":[]})).await?;
    let changed = mock.control(inbox.clone()).await?;
    assert_eq!(changed["messages"][0]["flags"], json!(["\\Seen"]));
    assert!(mock
        .control(
            json!({"command":"mailbox","account":"unconfigured@example.test","mailbox":"INBOX"})
        )
        .await
        .is_err());
    mock.control(json!({"command":"seed"})).await?;
    assert_eq!(mock.control(inbox).await?, initial);
    for transport in ["starttls", "tls"] {
        assert_eq!(
            mock.control(json!({"command":"smtp","transport":transport}))
                .await?["total"],
            0
        );
    }
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn independent_server_protocol_and_fault_contracts() -> Result<(), TestError> {
    nuncio_test_support::imap::verify_independent_contracts().await
}
